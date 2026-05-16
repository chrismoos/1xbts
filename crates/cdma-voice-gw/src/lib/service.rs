use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;

use tokio::sync::mpsc;
use tokio::sync::mpsc::error::TrySendError;
use tokio_stream::wrappers::ReceiverStream;
use tokio_stream::{Stream, StreamExt};
use tonic::transport::Server;
use tonic::{Request, Response, Status, Streaming};

use crate::config::{LoggingConfig, QueueConfig, VoiceGatewayConfig};
use crate::proto::gateway_to_msc_event::Event as GatewayEvent;
use crate::proto::msc_to_gateway_event::Event as MscEvent;
use crate::proto::voice_gateway_server::{VoiceGateway, VoiceGatewayServer};
use crate::proto::{
    AirVoiceFrame, GatewayAnswered, GatewayFailed, GatewayReleased, GatewayRinging,
    GatewayToMscEvent, GatewayVoiceFrame, MscToGatewayEvent, ReleaseReason,
};
use crate::sip::{
    LibreSipBackend, OutboundSipCall, SipBackend, SipBackendError, SipBackendEvent,
    SipBackendEventSender, UnavailableSipBackend,
};

type ControlResponseStream =
    Pin<Box<dyn Stream<Item = Result<GatewayToMscEvent, Status>> + Send + 'static>>;
type MediaResponseStream =
    Pin<Box<dyn Stream<Item = Result<GatewayVoiceFrame, Status>> + Send + 'static>>;

#[derive(Clone)]
pub struct VoiceGatewayService {
    sip_backend: Arc<dyn SipBackend>,
    logging: LoggingConfig,
    media_stream_capacity: usize,
}

impl VoiceGatewayService {
    pub fn new() -> Self {
        Self {
            sip_backend: Arc::new(UnavailableSipBackend),
            logging: LoggingConfig::default(),
            media_stream_capacity: QueueConfig::default().media_stream_frames,
        }
    }

    pub fn with_sip_backend(sip_backend: Arc<dyn SipBackend>) -> Self {
        Self {
            sip_backend,
            logging: LoggingConfig::default(),
            media_stream_capacity: QueueConfig::default().media_stream_frames,
        }
    }

    pub fn try_new_with_libre(config: VoiceGatewayConfig) -> Result<Self, SipBackendError> {
        let logging = config.logging.clone();
        let media_stream_capacity = config.queues.media_stream_frames;
        Ok(Self {
            sip_backend: Arc::new(LibreSipBackend::new(config)?),
            logging,
            media_stream_capacity,
        })
    }

    pub fn with_logging(mut self, logging: LoggingConfig) -> Self {
        self.logging = logging;
        self
    }
}

impl Default for VoiceGatewayService {
    fn default() -> Self {
        Self::new()
    }
}

#[tonic::async_trait]
impl VoiceGateway for VoiceGatewayService {
    type ControlStream = ControlResponseStream;
    type MediaStream = MediaResponseStream;

    async fn control(
        &self,
        request: Request<Streaming<MscToGatewayEvent>>,
    ) -> Result<Response<Self::ControlStream>, Status> {
        let mut inbound = request.into_inner();
        let (outbound_tx, outbound_rx) = mpsc::channel(32);
        let (sip_event_tx, mut sip_event_rx) = mpsc::unbounded_channel();
        let sip_backend = self.sip_backend.clone();
        let sip_outbound_tx = outbound_tx.clone();
        let logging = self.logging.clone();

        tokio::spawn(async move {
            while let Some(event) = sip_event_rx.recv().await {
                if logging.control_events {
                    log::debug!(
                        "VoiceGW control event to MSC: {}",
                        describe_gateway_event(&event)
                    );
                }
                let gateway_event = sip_backend_event_to_gateway_event(event);
                if sip_outbound_tx.send(Ok(gateway_event)).await.is_err() {
                    break;
                }
            }
        });

        let logging = self.logging.clone();
        tokio::spawn(async move {
            while let Some(next) = inbound.next().await {
                match next {
                    Ok(event) => {
                        if logging.control_events {
                            log::debug!(
                                "VoiceGW control event from MSC: {}",
                                describe_msc_event(&event)
                            );
                        }
                        handle_control_event(
                            event,
                            sip_backend.as_ref(),
                            sip_event_tx.clone(),
                            &outbound_tx,
                        )
                        .await;
                    }
                    Err(status) => {
                        log::warn!("VoiceGW control stream error from MSC: {}", status);
                        break;
                    }
                }
            }
        });

        Ok(Response::new(Box::pin(ReceiverStream::new(outbound_rx))))
    }

    async fn media(
        &self,
        request: Request<Streaming<AirVoiceFrame>>,
    ) -> Result<Response<Self::MediaStream>, Status> {
        let mut inbound = request.into_inner();
        let (outbound_tx, outbound_rx) = mpsc::channel(self.media_stream_capacity);
        let sip_backend = self.sip_backend.clone();
        let mut gateway_voice_rx = sip_backend.subscribe_gateway_voice_frames();
        let logging = self.logging.clone();

        tokio::spawn(async move {
            loop {
                tokio::select! {
                    next = inbound.next() => {
                        match next {
                            Some(Ok(frame)) => {
                                if logging.media_frames {
                                    log::trace!(
                                        "VoiceGW media frame from MSC: session={} bits={} rate_bps={} seq={}",
                                        frame.session_id,
                                        frame.num_bits,
                                        frame.rate_bps,
                                        frame.sequence
                                    );
                                }
                                if let Err(err) = sip_backend.handle_air_frame(frame).await {
                                    log::warn!("VoiceGW failed to handle MSC media frame: {}", err);
                                }
                            }
                            Some(Err(status)) => {
                                log::warn!("VoiceGW media stream error from MSC: {}", status);
                                break;
                            }
                            None => break,
                        }
                    }
                    frame = gateway_voice_rx.recv() => {
                        match frame {
                            Ok(frame) => {
                                if logging.media_frames {
                                    log::trace!(
                                        "VoiceGW media frame to MSC: session={} bits={} rate={:?} seq={}",
                                        frame.session_id,
                                        frame.num_bits,
                                        frame.rate,
                                        frame.sequence
                                    );
                                }
                                match outbound_tx.try_send(Ok(frame)) {
                                    Ok(()) => {}
                                    Err(TrySendError::Full(_)) => {
                                        log::warn!("VoiceGW media stream to MSC is full; dropping RTP frame");
                                    }
                                    Err(TrySendError::Closed(_)) => break,
                                }
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(skipped)) => {
                                log::warn!("VoiceGW media stream lagged by {} frame(s)", skipped);
                            }
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        }
                    }
                }
            }
        });

        Ok(Response::new(Box::pin(ReceiverStream::new(outbound_rx))))
    }
}

async fn handle_control_event(
    event: MscToGatewayEvent,
    sip_backend: &dyn SipBackend,
    sip_event_tx: SipBackendEventSender,
    outbound_tx: &mpsc::Sender<Result<GatewayToMscEvent, Status>>,
) {
    match event.event {
        Some(MscEvent::StartOutboundCall(start)) => {
            let session_id = start.session_id.clone();
            let service_option = match start_service_option_to_u16(start.service_option) {
                Ok(service_option) => service_option,
                Err(err) => {
                    send_gateway_failed(outbound_tx, session_id, err).await;
                    return;
                }
            };
            let call = OutboundSipCall {
                session_id: start.session_id,
                called_number: start.called_number,
                caller_number: start.caller_number,
                service_option,
            };

            if let Err(err) = sip_backend.start_outbound_call(call, sip_event_tx).await {
                send_gateway_failed(outbound_tx, session_id, err).await;
            }
        }
        Some(MscEvent::ReleaseCall(release)) => {
            let reason =
                ReleaseReason::try_from(release.reason).unwrap_or(ReleaseReason::MobileReleased);

            if let Err(err) = sip_backend.release_call(&release.session_id, reason).await {
                log::warn!(
                    "failed to release SIP session {} after MSC release: {}",
                    release.session_id,
                    err
                );
            }

            let _ = outbound_tx
                .send(Ok(GatewayToMscEvent {
                    event_id: uuid::Uuid::new_v4().to_string(),
                    event: Some(GatewayEvent::Released(GatewayReleased {
                        session_id: release.session_id,
                        reason: reason as i32,
                    })),
                }))
                .await;
        }
        None => {
            log::warn!("received empty VoiceGW control event from MSC");
        }
    }
}

fn describe_msc_event(event: &MscToGatewayEvent) -> String {
    match event.event.as_ref() {
        Some(MscEvent::StartOutboundCall(start)) => {
            format!(
                "StartOutboundCall session={} caller={} called={} service_option={}",
                start.session_id, start.caller_number, start.called_number, start.service_option
            )
        }
        Some(MscEvent::ReleaseCall(release)) => format!(
            "ReleaseCall session={} reason={:?}",
            release.session_id,
            ReleaseReason::try_from(release.reason).unwrap_or(ReleaseReason::Unspecified)
        ),
        None => "Empty".to_string(),
    }
}

fn describe_gateway_event(event: &SipBackendEvent) -> String {
    match event {
        SipBackendEvent::Ringing {
            session_id,
            sip_status,
            codec,
        } => format!("Ringing session={session_id} sip_status={sip_status} codec={codec}"),
        SipBackendEvent::Answered {
            session_id,
            sip_status,
            codec,
        } => format!("Answered session={session_id} sip_status={sip_status} codec={codec}"),
        SipBackendEvent::Failed {
            session_id,
            sip_status,
            reason,
        } => format!("Failed session={session_id} sip_status={sip_status:?} reason={reason}"),
        SipBackendEvent::Released { session_id, reason } => {
            format!("Released session={session_id} reason={reason:?}")
        }
    }
}

fn start_service_option_to_u16(service_option: u32) -> Result<u16, SipBackendError> {
    let service_option = if service_option == 0 {
        3
    } else {
        u16::try_from(service_option).map_err(|_| {
            SipBackendError::Media(format!("invalid voice service option {service_option}"))
        })?
    };
    Ok(service_option)
}

fn sip_backend_event_to_gateway_event(event: SipBackendEvent) -> GatewayToMscEvent {
    let event = match event {
        SipBackendEvent::Ringing {
            session_id,
            sip_status,
            codec,
        } => GatewayEvent::Ringing(GatewayRinging {
            session_id,
            sip_status: u32::from(sip_status),
            codec,
        }),
        SipBackendEvent::Answered {
            session_id,
            sip_status,
            codec,
        } => GatewayEvent::Answered(GatewayAnswered {
            session_id,
            sip_status: u32::from(sip_status),
            codec,
        }),
        SipBackendEvent::Failed {
            session_id,
            sip_status,
            reason,
        } => GatewayEvent::Failed(GatewayFailed {
            session_id,
            sip_status: sip_status.map(u32::from).unwrap_or(0),
            reason,
            release_reason: ReleaseReason::SipFailure as i32,
        }),
        SipBackendEvent::Released { session_id, reason } => {
            GatewayEvent::Released(GatewayReleased {
                session_id,
                reason: reason as i32,
            })
        }
    };

    GatewayToMscEvent {
        event_id: uuid::Uuid::new_v4().to_string(),
        event: Some(event),
    }
}

async fn send_gateway_failed(
    outbound_tx: &mpsc::Sender<Result<GatewayToMscEvent, Status>>,
    session_id: String,
    err: SipBackendError,
) {
    let reason = err.to_string();

    log::warn!(
        "failing outbound SIP session {} before answer: {}",
        session_id,
        reason
    );

    let _ = outbound_tx
        .send(Ok(GatewayToMscEvent {
            event_id: uuid::Uuid::new_v4().to_string(),
            event: Some(GatewayEvent::Failed(GatewayFailed {
                session_id,
                sip_status: 0,
                reason,
                release_reason: ReleaseReason::SetupFailed as i32,
            })),
        }))
        .await;
}

pub async fn run_grpc_server(
    addr: SocketAddr,
    service: VoiceGatewayService,
) -> Result<(), tonic::transport::Error> {
    Server::builder()
        .add_service(VoiceGatewayServer::new(service))
        .serve(addr)
        .await
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Mutex;

    use tokio::sync::broadcast;

    #[derive(Default)]
    struct RecordingBackend {
        starts: Mutex<Vec<OutboundSipCall>>,
        releases: Mutex<Vec<(String, ReleaseReason)>>,
        start_error: Option<String>,
    }

    #[async_trait::async_trait]
    impl SipBackend for RecordingBackend {
        async fn start_outbound_call(
            &self,
            call: OutboundSipCall,
            _events: SipBackendEventSender,
        ) -> crate::sip::Result<()> {
            self.starts
                .lock()
                .expect("start recorder mutex should not be poisoned")
                .push(call);

            if let Some(reason) = &self.start_error {
                Err(SipBackendError::Unavailable(reason.clone()))
            } else {
                Ok(())
            }
        }

        async fn release_call(
            &self,
            session_id: &str,
            reason: ReleaseReason,
        ) -> crate::sip::Result<()> {
            self.releases
                .lock()
                .expect("release recorder mutex should not be poisoned")
                .push((session_id.to_string(), reason));
            Ok(())
        }

        async fn handle_air_frame(&self, _frame: AirVoiceFrame) -> crate::sip::Result<()> {
            Ok(())
        }

        fn subscribe_gateway_voice_frames(&self) -> broadcast::Receiver<GatewayVoiceFrame> {
            let (_tx, rx) = broadcast::channel(1);
            rx
        }
    }

    #[test]
    fn maps_sip_answer_event_to_gateway_event() {
        let event = sip_backend_event_to_gateway_event(SipBackendEvent::Answered {
            session_id: "session-1".to_string(),
            sip_status: 200,
            codec: "PCMU".to_string(),
        });

        assert!(matches!(
            event.event,
            Some(GatewayEvent::Answered(answered))
                if answered.session_id == "session-1"
                    && answered.sip_status == 200
                    && answered.codec == "PCMU"
        ));
    }

    #[tokio::test]
    async fn start_outbound_call_is_forwarded_to_backend() {
        let backend = RecordingBackend::default();
        let (outbound_tx, mut outbound_rx) = mpsc::channel(1);
        let (sip_event_tx, _sip_event_rx) = mpsc::unbounded_channel();

        handle_control_event(
            MscToGatewayEvent {
                event_id: "event-1".to_string(),
                event: Some(MscEvent::StartOutboundCall(
                    crate::proto::StartOutboundCall {
                        session_id: "session-1".to_string(),
                        called_number: "18005550199".to_string(),
                        caller_number: "15551230000".to_string(),
                        service_option: 68,
                    },
                )),
            },
            &backend,
            sip_event_tx,
            &outbound_tx,
        )
        .await;

        let starts = backend
            .starts
            .lock()
            .expect("start recorder mutex should not be poisoned");
        assert_eq!(
            starts.as_slice(),
            [OutboundSipCall {
                session_id: "session-1".to_string(),
                called_number: "18005550199".to_string(),
                caller_number: "15551230000".to_string(),
                service_option: 68,
            }]
        );
        assert!(outbound_rx.try_recv().is_err());
    }

    #[tokio::test]
    async fn start_outbound_call_error_emits_gateway_failed() {
        let backend = RecordingBackend {
            start_error: Some("backend down".to_string()),
            ..RecordingBackend::default()
        };
        let (outbound_tx, mut outbound_rx) = mpsc::channel(1);
        let (sip_event_tx, _sip_event_rx) = mpsc::unbounded_channel();

        handle_control_event(
            MscToGatewayEvent {
                event_id: "event-1".to_string(),
                event: Some(MscEvent::StartOutboundCall(
                    crate::proto::StartOutboundCall {
                        session_id: "session-1".to_string(),
                        called_number: "18005550199".to_string(),
                        caller_number: "15551230000".to_string(),
                        service_option: 3,
                    },
                )),
            },
            &backend,
            sip_event_tx,
            &outbound_tx,
        )
        .await;

        let event = outbound_rx.recv().await.unwrap().unwrap();
        assert!(matches!(
            event.event,
            Some(GatewayEvent::Failed(failed))
                if failed.session_id == "session-1"
                    && failed.sip_status == 0
                    && failed.release_reason == ReleaseReason::SetupFailed as i32
                    && failed.reason.contains("backend down")
        ));
    }

    #[tokio::test]
    async fn release_call_releases_backend_and_echoes_released() {
        let backend = RecordingBackend::default();
        let (outbound_tx, mut outbound_rx) = mpsc::channel(1);
        let (sip_event_tx, _sip_event_rx) = mpsc::unbounded_channel();

        handle_control_event(
            MscToGatewayEvent {
                event_id: "event-1".to_string(),
                event: Some(MscEvent::ReleaseCall(crate::proto::ReleaseCall {
                    session_id: "session-1".to_string(),
                    reason: ReleaseReason::MscTeardown as i32,
                })),
            },
            &backend,
            sip_event_tx,
            &outbound_tx,
        )
        .await;

        let releases = backend
            .releases
            .lock()
            .expect("release recorder mutex should not be poisoned");
        assert_eq!(
            releases.as_slice(),
            [("session-1".to_string(), ReleaseReason::MscTeardown)]
        );

        let event = outbound_rx.recv().await.unwrap().unwrap();
        assert!(matches!(
            event.event,
                Some(GatewayEvent::Released(released))
                    if released.session_id == "session-1"
                    && released.reason == ReleaseReason::MscTeardown as i32
        ));
    }
}
