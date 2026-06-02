//! MSC management gRPC server and client.
//!
//! The MSC hosts its own management gRPC endpoint so that `initiate_call`,
//! `list_calls`, and `send_sms` no longer proxy through the BSC.

use tonic::{Request, Response, Status};

use crate::management::InitiateCallRequest;

pub mod bsc {
    pub mod v1 {
        tonic::include_proto!("bsc.v1");
    }
}

pub mod events {
    pub mod v1 {
        tonic::include_proto!("events.v1");
    }
}

// Backwards-compatible alias used by `crate::otasp::*` modules.
pub use events as events_proto;

pub mod msc_management {
    pub mod v1 {
        tonic::include_proto!("msc_management.v1");
    }
}

pub mod voice_gateway {
    pub mod v1 {
        tonic::include_proto!("voice_gateway.v1");
    }
}

use bsc::v1 as proto;
use msc_management::v1 as mgmt_proto;
use msc_management::v1::CallList;
use msc_management::v1::msc_management_service_server::MscManagementService;

/// MSC-side gRPC management service backed by a channel into the MSC runtime.
pub struct MscManagementServiceImpl {
    mgmt_tx: tokio::sync::mpsc::Sender<crate::management::PendingControlRequest>,
    otasp_event_tx: Option<tokio::sync::broadcast::Sender<events_proto::v1::MscNetworkEvent>>,
}

impl MscManagementServiceImpl {
    /// Creates a gRPC service that feeds management requests into the MSC runtime channel.
    pub fn from_channel(
        mgmt_tx: tokio::sync::mpsc::Sender<crate::management::PendingControlRequest>,
    ) -> Self {
        Self {
            mgmt_tx,
            otasp_event_tx: None,
        }
    }

    /// Attach the live OTASP event broadcast channel produced by the MSC
    /// runtime so the `StreamOtaspEvents` RPC can fan events out to
    /// connected dashboards.
    pub fn with_otasp(
        mut self,
        event_tx: tokio::sync::broadcast::Sender<events_proto::v1::MscNetworkEvent>,
    ) -> Self {
        self.otasp_event_tx = Some(event_tx);
        self
    }
}

#[tonic::async_trait]
impl MscManagementService for MscManagementServiceImpl {
    async fn initiate_call(
        &self,
        request: Request<proto::InitiateCallRequest>,
    ) -> Result<Response<proto::InitiateCallResponse>, Status> {
        let inner = request.into_inner();
        let subscriber_id = uuid::Uuid::parse_str(&inner.subscriber_id)
            .map_err(|e| Status::invalid_argument(format!("invalid subscriber_id: {e}")))?;
        let (response_tx, response_rx) = tokio::sync::oneshot::channel();
        self.mgmt_tx
            .send(crate::management::PendingControlRequest::InitiateCall {
                request: InitiateCallRequest {
                    subscriber_id,
                    audio_file: inner.audio_file,
                    caller_number: inner.caller_number,
                },
                response_tx,
            })
            .await
            .map_err(|_| Status::unavailable("MSC runtime is shutting down"))?;
        let result = response_rx
            .await
            .map_err(|_| Status::internal("MSC runtime dropped response channel"))?
            .map_err(|e| Status::internal(e.to_string()))?;
        Ok(Response::new(proto::InitiateCallResponse {
            accepted: true,
            message: format!("call_id={}", result.call_id.0),
        }))
    }

    async fn send_sms(
        &self,
        request: Request<mgmt_proto::SendSmsRequest>,
    ) -> Result<Response<mgmt_proto::SendSmsResponse>, Status> {
        let inner = request.into_inner();
        let destination = match inner.destination {
            Some(mgmt_proto::send_sms_request::Destination::DestinationNumber(num)) => {
                crate::sms::SmsDestinationKey::PhoneNumber(num)
            }
            Some(mgmt_proto::send_sms_request::Destination::DestinationImsi(imsi)) => {
                crate::sms::SmsDestinationKey::Imsi(imsi)
            }
            None => {
                return Err(Status::invalid_argument(
                    "destination_number or destination_imsi is required",
                ));
            }
        };
        let teleservice_id = inner
            .teleservice_id
            .map(|v| {
                u16::try_from(v).map_err(|_| Status::invalid_argument("teleservice_id > 65535"))
            })
            .transpose()?;
        let (response_tx, response_rx) = tokio::sync::oneshot::channel();
        self.mgmt_tx
            .send(crate::management::PendingControlRequest::SendSms {
                request: crate::sms::SmsSendRequest {
                    originating_number: inner.originating_number,
                    text: inner.text,
                    destination,
                    timeout_ms: inner.timeout_ms.unwrap_or(30_000),
                    teleservice_id,
                    raw_user_data: inner.raw_user_data,
                },
                response_tx,
            })
            .await
            .map_err(|_| Status::unavailable("MSC runtime is shutting down"))?;
        let sms_id = response_rx
            .await
            .map_err(|_| Status::internal("MSC runtime dropped response channel"))?;
        Ok(Response::new(mgmt_proto::SendSmsResponse {
            accepted: sms_id.is_some(),
            message: sms_id.map(|id| format!("sms_id={id}")).unwrap_or_else(|| {
                "SMS delivery failed (mobile unreachable or SMSC unavailable)".to_string()
            }),
        }))
    }

    type StreamOtaspEventsStream = std::pin::Pin<
        Box<
            dyn tokio_stream::Stream<Item = Result<events_proto::v1::MscNetworkEvent, Status>>
                + Send
                + 'static,
        >,
    >;

    async fn stream_otasp_events(
        &self,
        _: Request<()>,
    ) -> Result<Response<Self::StreamOtaspEventsStream>, Status> {
        let Some(tx) = self.otasp_event_tx.as_ref() else {
            return Err(Status::unavailable("OTASP event stream not configured"));
        };
        let mut rx = tx.subscribe();
        let stream = async_stream::stream! {
            loop {
                match rx.recv().await {
                    Ok(ev) => yield Ok(ev),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        };
        Ok(Response::new(Box::pin(stream)))
    }

    async fn list_calls(&self, _: Request<()>) -> Result<Response<CallList>, Status> {
        let (response_tx, response_rx) = tokio::sync::oneshot::channel();
        self.mgmt_tx
            .send(crate::management::PendingControlRequest::ListCalls { response_tx })
            .await
            .map_err(|_| Status::unavailable("MSC runtime is shutting down"))?;
        let snapshots = response_rx
            .await
            .map_err(|_| Status::internal("MSC runtime dropped response channel"))?
            .map_err(|e| Status::internal(e.to_string()))?;
        Ok(Response::new(CallList {
            call_ids: snapshots.iter().map(|s| s.id.0.to_string()).collect(),
        }))
    }
}
