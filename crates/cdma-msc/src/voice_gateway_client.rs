use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use log::{info, trace, warn};
use tokio::sync::mpsc::error::TrySendError;
use tokio::sync::{Mutex as AsyncMutex, mpsc, watch};
use tokio_stream::wrappers::ReceiverStream;
use tonic::transport::Channel;

use crate::config::VoiceGatewayConfig;
use crate::grpc::voice_gateway::v1 as proto;
use crate::media_gateway::{
    CallHandle, CreateCallRequest, MediaGatewayClient, MediaGatewayEvent, MgwError, ReleaseCause,
    VocoderFrame,
};

const RECONNECT_DELAY: Duration = Duration::from_secs(1);
#[derive(Clone)]
pub struct VoiceGatewayClient {
    command_tx: mpsc::UnboundedSender<VoiceGatewayCommand>,
    event_rx: Arc<AsyncMutex<mpsc::UnboundedReceiver<MediaGatewayEvent>>>,
    ready_rx: watch::Receiver<bool>,
    next_handle: Arc<AtomicU64>,
    handles: Arc<Mutex<HashMap<CallHandle, GatewayCallState>>>,
}

#[derive(Clone, Debug)]
struct GatewayCallState {
    session_id: String,
    service_option: u16,
    next_sequence: u64,
}

#[derive(Clone, Debug)]
enum VoiceGatewayCommand {
    StartOutboundCall {
        handle: CallHandle,
        session_id: String,
        called_number: String,
        caller_number: String,
        service_option: u16,
    },
    ReleaseCall {
        session_id: String,
        cause: ReleaseCause,
    },
    AirFrame {
        session_id: String,
        bits: Vec<u8>,
        rate_bps: u32,
        sequence: u64,
        service_option: u16,
    },
    InboundProgress {
        session_id: String,
    },
    InboundAnswer {
        session_id: String,
        codec: String,
    },
    InboundReject {
        session_id: String,
        sip_status: u16,
    },
    SendDtmf {
        session_id: String,
        event_code: u8,
        volume: u8,
        duration_samples: u16,
        end: bool,
        start_of_event: bool,
    },
}

pub fn spawn_voice_gateway_client(config: VoiceGatewayConfig) -> Arc<VoiceGatewayClient> {
    let (command_tx, command_rx) = mpsc::unbounded_channel();
    let (event_tx, event_rx) = mpsc::unbounded_channel();
    let (ready_tx, ready_rx) = watch::channel(false);
    let handles = Arc::new(Mutex::new(HashMap::new()));

    tokio::spawn(run_voice_gateway_client(
        config,
        command_rx,
        event_tx,
        ready_tx,
        handles.clone(),
    ));

    Arc::new(VoiceGatewayClient {
        command_tx,
        event_rx: Arc::new(AsyncMutex::new(event_rx)),
        ready_rx,
        next_handle: Arc::new(AtomicU64::new(1)),
        handles,
    })
}

#[async_trait]
impl MediaGatewayClient for VoiceGatewayClient {
    async fn create_call(&self, req: CreateCallRequest) -> Result<CallHandle, MgwError> {
        if !*self.ready_rx.borrow() {
            return Err(MgwError::Unavailable);
        }
        let handle = CallHandle(self.next_handle.fetch_add(1, Ordering::Relaxed));
        let session_id = handle.0.to_string();
        self.handles
            .lock()
            .map_err(|_| MgwError::Transport("voice gateway handle map poisoned".to_string()))?
            .insert(
                handle,
                GatewayCallState {
                    session_id: session_id.clone(),
                    service_option: req.service_option,
                    next_sequence: 0,
                },
            );
        self.command_tx
            .send(VoiceGatewayCommand::StartOutboundCall {
                handle,
                session_id,
                called_number: req.called_party.unwrap_or_default(),
                caller_number: req.calling_party.unwrap_or_else(|| "anonymous".to_string()),
                service_option: req.service_option,
            })
            .map_err(|_| MgwError::Unavailable)?;
        Ok(handle)
    }

    async fn answer_call(&self, _handle: CallHandle) -> Result<(), MgwError> {
        Ok(())
    }

    async fn release_call(&self, handle: CallHandle, cause: ReleaseCause) -> Result<(), MgwError> {
        let session_id = self.remove_call_state(handle)?.session_id;
        self.command_tx
            .send(VoiceGatewayCommand::ReleaseCall { session_id, cause })
            .map_err(|_| MgwError::Unavailable)
    }

    async fn forward_payload(
        &self,
        handle: CallHandle,
        payload: VocoderFrame,
    ) -> Result<(), MgwError> {
        let state = self.next_frame_state(handle)?;
        self.command_tx
            .send(VoiceGatewayCommand::AirFrame {
                session_id: state.session_id,
                bits: payload.payload,
                rate_bps: payload.rate_bps,
                sequence: state.next_sequence,
                service_option: state.service_option,
            })
            .map_err(|_| MgwError::Unavailable)
    }

    async fn recv_event(&self) -> Option<MediaGatewayEvent> {
        self.event_rx.lock().await.recv().await
    }

    async fn register_inbound_session(
        &self,
        session_id: String,
        service_option: u16,
    ) -> Result<CallHandle, MgwError> {
        if !*self.ready_rx.borrow() {
            return Err(MgwError::Unavailable);
        }
        let handle = CallHandle(self.next_handle.fetch_add(1, Ordering::Relaxed));
        self.handles
            .lock()
            .map_err(|_| MgwError::Transport("voice gateway handle map poisoned".to_string()))?
            .insert(
                handle,
                GatewayCallState {
                    session_id,
                    service_option,
                    next_sequence: 0,
                },
            );
        Ok(handle)
    }

    async fn inbound_progress(&self, session_id: &str) -> Result<(), MgwError> {
        if !*self.ready_rx.borrow() {
            return Err(MgwError::Unavailable);
        }
        self.command_tx
            .send(VoiceGatewayCommand::InboundProgress {
                session_id: session_id.to_string(),
            })
            .map_err(|_| MgwError::Unavailable)
    }

    async fn inbound_answer(&self, session_id: &str, codec: &str) -> Result<(), MgwError> {
        if !*self.ready_rx.borrow() {
            return Err(MgwError::Unavailable);
        }
        self.command_tx
            .send(VoiceGatewayCommand::InboundAnswer {
                session_id: session_id.to_string(),
                codec: codec.to_string(),
            })
            .map_err(|_| MgwError::Unavailable)
    }

    async fn inbound_reject(&self, session_id: &str, sip_status: u16) -> Result<(), MgwError> {
        if !*self.ready_rx.borrow() {
            return Err(MgwError::Unavailable);
        }
        self.command_tx
            .send(VoiceGatewayCommand::InboundReject {
                session_id: session_id.to_string(),
                sip_status,
            })
            .map_err(|_| MgwError::Unavailable)
    }

    async fn send_dtmf(
        &self,
        handle: CallHandle,
        event_code: u8,
        volume: u8,
        duration_samples: u16,
        end: bool,
        start_of_event: bool,
    ) -> Result<(), MgwError> {
        if !*self.ready_rx.borrow() {
            return Err(MgwError::Unavailable);
        }
        let session_id = self
            .handles
            .lock()
            .map_err(|_| MgwError::Transport("voice gateway handle map poisoned".to_string()))?
            .get(&handle)
            .ok_or(MgwError::UnknownCall(handle))?
            .session_id
            .clone();
        self.command_tx
            .send(VoiceGatewayCommand::SendDtmf {
                session_id,
                event_code,
                volume,
                duration_samples,
                end,
                start_of_event,
            })
            .map_err(|_| MgwError::Unavailable)
    }
}

impl VoiceGatewayClient {
    fn remove_call_state(&self, handle: CallHandle) -> Result<GatewayCallState, MgwError> {
        self.handles
            .lock()
            .map_err(|_| MgwError::Transport("voice gateway handle map poisoned".to_string()))?
            .remove(&handle)
            .ok_or(MgwError::UnknownCall(handle))
    }

    fn next_frame_state(&self, handle: CallHandle) -> Result<GatewayCallState, MgwError> {
        let mut handles = self
            .handles
            .lock()
            .map_err(|_| MgwError::Transport("voice gateway handle map poisoned".to_string()))?;
        let state = handles
            .get_mut(&handle)
            .ok_or(MgwError::UnknownCall(handle))?;
        let frame_state = state.clone();
        state.next_sequence = state.next_sequence.wrapping_add(1);
        Ok(frame_state)
    }
}

async fn run_voice_gateway_client(
    config: VoiceGatewayConfig,
    mut command_rx: mpsc::UnboundedReceiver<VoiceGatewayCommand>,
    event_tx: mpsc::UnboundedSender<MediaGatewayEvent>,
    ready_tx: watch::Sender<bool>,
    handles: Arc<Mutex<HashMap<CallHandle, GatewayCallState>>>,
) {
    let mut warned_unavailable = false;
    loop {
        ready_tx.send_replace(false);
        while command_rx.try_recv().is_ok() {}

        match connect_and_run(&config, &mut command_rx, &event_tx, &ready_tx, &handles).await {
            Ok(()) => {
                warned_unavailable = false;
                warn!("MSC: voice gateway stream ended");
            }
            Err(err) => {
                if !warned_unavailable {
                    warned_unavailable = true;
                    warn!(
                        "MSC: voice gateway connection to {} unavailable: {}",
                        config.endpoint, err
                    );
                } else {
                    trace!(
                        "MSC: voice gateway connection to {} unavailable: {}",
                        config.endpoint, err
                    );
                }
            }
        }

        ready_tx.send_replace(false);
        tokio::time::sleep(RECONNECT_DELAY).await;
    }
}

async fn connect_and_run(
    config: &VoiceGatewayConfig,
    command_rx: &mut mpsc::UnboundedReceiver<VoiceGatewayCommand>,
    event_tx: &mpsc::UnboundedSender<MediaGatewayEvent>,
    ready_tx: &watch::Sender<bool>,
    handles: &Arc<Mutex<HashMap<CallHandle, GatewayCallState>>>,
) -> Result<(), String> {
    let client = proto::voice_gateway_client::VoiceGatewayClient::connect(config.endpoint.clone())
        .await
        .map_err(|err| err.to_string())?;
    info!("MSC: connected to voice gateway at {}", config.endpoint);
    run_streams(client, command_rx, event_tx, ready_tx, handles).await
}

async fn run_streams(
    client: proto::voice_gateway_client::VoiceGatewayClient<Channel>,
    command_rx: &mut mpsc::UnboundedReceiver<VoiceGatewayCommand>,
    event_tx: &mpsc::UnboundedSender<MediaGatewayEvent>,
    ready_tx: &watch::Sender<bool>,
    handles: &Arc<Mutex<HashMap<CallHandle, GatewayCallState>>>,
) -> Result<(), String> {
    let (control_tx, control_rx) = mpsc::channel(64);
    let (media_tx, media_rx) = mpsc::channel(256);

    let mut control_client = client.clone();
    let mut media_client = client;
    let control_response = control_client
        .control(ReceiverStream::new(control_rx))
        .await
        .map_err(|err| format!("control stream failed: {err}"))?;
    let media_response = media_client
        .media(ReceiverStream::new(media_rx))
        .await
        .map_err(|err| format!("media stream failed: {err}"))?;

    let mut control_inbound = control_response.into_inner();
    let mut media_inbound = media_response.into_inner();
    ready_tx.send_replace(true);

    loop {
        tokio::select! {
            Some(command) = command_rx.recv() => {
                send_command(command, &control_tx, &media_tx).await?;
            }
            event = control_inbound.message() => {
                match event {
                    Ok(Some(event)) => {
                        if let Some(event) = convert_gateway_event(event, handles) {
                            let _ = event_tx.send(event);
                        }
                    }
                    Ok(None) => return Err("control stream closed by gateway".to_string()),
                    Err(status) => return Err(format!("control stream error: {status}")),
                }
            }
            frame = media_inbound.message() => {
                match frame {
                    Ok(Some(frame)) => {
                        if let Some(event) = convert_gateway_frame(frame, handles) {
                            let _ = event_tx.send(event);
                        }
                    }
                    Ok(None) => return Err("media stream closed by gateway".to_string()),
                    Err(status) => return Err(format!("media stream error: {status}")),
                }
            }
        }
    }
}

async fn send_command(
    command: VoiceGatewayCommand,
    control_tx: &mpsc::Sender<proto::MscToGatewayEvent>,
    media_tx: &mpsc::Sender<proto::AirVoiceFrame>,
) -> Result<(), String> {
    match command {
        VoiceGatewayCommand::StartOutboundCall {
            handle,
            session_id,
            called_number,
            caller_number,
            service_option,
        } => control_tx
            .send(proto::MscToGatewayEvent {
                event_id: handle.0.to_string(),
                event: Some(proto::msc_to_gateway_event::Event::StartOutboundCall(
                    proto::StartOutboundCall {
                        session_id,
                        called_number,
                        caller_number,
                        service_option: u32::from(service_option),
                    },
                )),
            })
            .await
            .map_err(|_| "voice gateway control stream closed".to_string()),
        VoiceGatewayCommand::ReleaseCall { session_id, cause } => control_tx
            .send(proto::MscToGatewayEvent {
                event_id: uuid::Uuid::new_v4().to_string(),
                event: Some(proto::msc_to_gateway_event::Event::ReleaseCall(
                    proto::ReleaseCall {
                        session_id,
                        reason: release_cause_to_proto(cause) as i32,
                    },
                )),
            })
            .await
            .map_err(|_| "voice gateway control stream closed".to_string()),
        VoiceGatewayCommand::AirFrame {
            session_id,
            bits,
            rate_bps,
            sequence,
            service_option,
        } => match media_tx.try_send(proto::AirVoiceFrame {
            session_id,
            num_bits: bits.len() as u32,
            bits,
            rate_bps,
            sequence,
            service_option: u32::from(service_option),
        }) {
            Ok(()) => Ok(()),
            Err(TrySendError::Full(_)) => Ok(()),
            Err(TrySendError::Closed(_)) => Err("voice gateway media stream closed".to_string()),
        },
        VoiceGatewayCommand::InboundProgress { session_id } => control_tx
            .send(proto::MscToGatewayEvent {
                event_id: uuid::Uuid::new_v4().to_string(),
                event: Some(proto::msc_to_gateway_event::Event::InboundCallProgress(
                    proto::InboundCallProgress { session_id },
                )),
            })
            .await
            .map_err(|_| "voice gateway control stream closed".to_string()),
        VoiceGatewayCommand::InboundAnswer { session_id, codec } => control_tx
            .send(proto::MscToGatewayEvent {
                event_id: uuid::Uuid::new_v4().to_string(),
                event: Some(proto::msc_to_gateway_event::Event::InboundCallAnswer(
                    proto::InboundCallAnswer { session_id, codec },
                )),
            })
            .await
            .map_err(|_| "voice gateway control stream closed".to_string()),
        VoiceGatewayCommand::InboundReject {
            session_id,
            sip_status,
        } => control_tx
            .send(proto::MscToGatewayEvent {
                event_id: uuid::Uuid::new_v4().to_string(),
                event: Some(proto::msc_to_gateway_event::Event::InboundCallReject(
                    proto::InboundCallReject {
                        session_id,
                        sip_status: u32::from(sip_status),
                    },
                )),
            })
            .await
            .map_err(|_| "voice gateway control stream closed".to_string()),
        VoiceGatewayCommand::SendDtmf {
            session_id,
            event_code,
            volume,
            duration_samples,
            end,
            start_of_event,
        } => control_tx
            .send(proto::MscToGatewayEvent {
                event_id: uuid::Uuid::new_v4().to_string(),
                event: Some(proto::msc_to_gateway_event::Event::SendDtmf(
                    proto::SendDtmf {
                        session_id,
                        event_code: u32::from(event_code),
                        volume: u32::from(volume),
                        duration_samples: u32::from(duration_samples),
                        end,
                        start_of_event,
                    },
                )),
            })
            .await
            .map_err(|_| "voice gateway control stream closed".to_string()),
    }
}

fn convert_gateway_event(
    event: proto::GatewayToMscEvent,
    handles: &Arc<Mutex<HashMap<CallHandle, GatewayCallState>>>,
) -> Option<MediaGatewayEvent> {
    match event.event? {
        proto::gateway_to_msc_event::Event::Ringing(ringing) => Some(MediaGatewayEvent::Ringing {
            handle: handle_for_session(handles, &ringing.session_id)?,
            sip_status: ringing.sip_status,
            codec: ringing.codec,
        }),
        proto::gateway_to_msc_event::Event::Answered(answered) => {
            Some(MediaGatewayEvent::Answered {
                handle: handle_for_session(handles, &answered.session_id)?,
                sip_status: answered.sip_status,
                codec: answered.codec,
            })
        }
        proto::gateway_to_msc_event::Event::Failed(failed) => Some(MediaGatewayEvent::Failed {
            handle: handle_for_session(handles, &failed.session_id)?,
            sip_status: (failed.sip_status != 0).then_some(failed.sip_status),
            cause: release_cause_from_i32(failed.release_reason),
            reason: failed.reason,
        }),
        proto::gateway_to_msc_event::Event::Released(released) => {
            Some(MediaGatewayEvent::Released {
                handle: handle_for_session(handles, &released.session_id)?,
                cause: release_cause_from_i32(released.reason),
            })
        }
        proto::gateway_to_msc_event::Event::InboundCall(call) => {
            Some(MediaGatewayEvent::InboundCall {
                session_id: call.session_id,
                called_number: call.called_number,
                caller_number: call.caller_number,
                caller_display: call.caller_display,
                offered_codecs: call.offered_codecs,
            })
        }
        proto::gateway_to_msc_event::Event::InboundCancel(cancel) => {
            Some(MediaGatewayEvent::InboundCancel {
                session_id: cancel.session_id,
            })
        }
    }
}

fn convert_gateway_frame(
    frame: proto::GatewayVoiceFrame,
    handles: &Arc<Mutex<HashMap<CallHandle, GatewayCallState>>>,
) -> Option<MediaGatewayEvent> {
    let rate_bps = match proto::VoiceFrameRate::try_from(frame.rate).ok()? {
        proto::VoiceFrameRate::Full => 9600,
        proto::VoiceFrameRate::Half => 4800,
        proto::VoiceFrameRate::Quarter => 2400,
        proto::VoiceFrameRate::Eighth => 1200,
        proto::VoiceFrameRate::Unspecified => return None,
    };
    Some(MediaGatewayEvent::MediaFrame {
        handle: handle_for_session(handles, &frame.session_id)?,
        payload: VocoderFrame {
            payload: frame.bits,
            rate_bps,
        },
        sequence: frame.sequence,
        service_option: u16::try_from(frame.service_option).unwrap_or(0),
    })
}

fn handle_for_session(
    handles: &Arc<Mutex<HashMap<CallHandle, GatewayCallState>>>,
    session_id: &str,
) -> Option<CallHandle> {
    handles.lock().ok()?.iter().find_map(|(handle, state)| {
        if state.session_id == session_id {
            Some(*handle)
        } else {
            None
        }
    })
}

fn release_cause_to_proto(cause: ReleaseCause) -> proto::ReleaseReason {
    match cause {
        ReleaseCause::RadioReleased => proto::ReleaseReason::MobileReleased,
        ReleaseCause::RemoteReleased => proto::ReleaseReason::SipReleased,
        ReleaseCause::SipFailure => proto::ReleaseReason::SipFailure,
        ReleaseCause::GatewayTimeout => proto::ReleaseReason::GatewayTimeout,
        ReleaseCause::MediaError => proto::ReleaseReason::MediaError,
        ReleaseCause::Administrative => proto::ReleaseReason::MscTeardown,
        ReleaseCause::SetupFailed => proto::ReleaseReason::SetupFailed,
    }
}

fn release_cause_from_i32(reason: i32) -> ReleaseCause {
    match proto::ReleaseReason::try_from(reason).unwrap_or(proto::ReleaseReason::Unspecified) {
        proto::ReleaseReason::MobileReleased => ReleaseCause::RadioReleased,
        proto::ReleaseReason::SipReleased => ReleaseCause::RemoteReleased,
        proto::ReleaseReason::SipFailure => ReleaseCause::SipFailure,
        proto::ReleaseReason::GatewayTimeout => ReleaseCause::GatewayTimeout,
        proto::ReleaseReason::MediaError => ReleaseCause::MediaError,
        proto::ReleaseReason::MscTeardown | proto::ReleaseReason::BscTeardown => {
            ReleaseCause::Administrative
        }
        proto::ReleaseReason::SetupFailed => ReleaseCause::SetupFailed,
        _ => ReleaseCause::RemoteReleased,
    }
}
