//! MSC-owned management-plane control surface.
//!
//! Operator actions such as "initiate a call" belong to the MSC. This module
//! defines the control seam used by higher layers while the codebase is still
//! running in a mostly in-process topology.

use std::fmt::{Display, Formatter};

use cdma_rpc_error::{Domain, ErrorReason};
use tokio::sync::oneshot;
use tonic::Status;
use uuid::Uuid;

use crate::call_control::{CallId, CallSessionSnapshot};

/// Management-plane request to initiate a mobile-terminated voice call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InitiateCallRequest {
    /// HLR subscriber identifier for the called mobile station.
    pub subscriber_id: Uuid,
    /// Optional local test-media file while the voice path is still migrating.
    pub audio_file: Option<String>,
    /// Optional caller-ID digits presented to the called mobile.
    pub caller_number: Option<String>,
    /// Voice service option to page with. `None` selects the configured default.
    pub service_option: Option<u16>,
}

/// Successful MSC acceptance of an operator-originated call request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InitiateCallAccepted {
    /// MSC-local call identifier allocated for the request.
    pub call_id: CallId,
}

/// Errors returned by the MSC management control surface.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ManagementError {
    UnknownSubscriber(Uuid),
    UnknownCall(CallId),
    HlrUnavailable(String),
    Unavailable {
        reason: ErrorReason,
        message: String,
        metadata: Vec<(&'static str, String)>,
    },
    Rejected {
        reason: ErrorReason,
        message: String,
        metadata: Vec<(&'static str, String)>,
    },
}

pub(crate) const MSC: Domain = Domain("msc");

impl ManagementError {
    pub(crate) fn rejected(reason: ErrorReason, message: impl Into<String>) -> Self {
        Self::Rejected {
            reason,
            message: message.into(),
            metadata: Vec::new(),
        }
    }

    pub(crate) fn unavailable(reason: ErrorReason, message: impl Into<String>) -> Self {
        Self::Unavailable {
            reason,
            message: message.into(),
            metadata: Vec::new(),
        }
    }

    pub(crate) fn with(mut self, key: &'static str, value: impl ToString) -> Self {
        if let Self::Unavailable { metadata, .. } | Self::Rejected { metadata, .. } = &mut self {
            metadata.push((key, value.to_string()));
        }
        self
    }

    pub fn into_status(self) -> Status {
        match self {
            Self::UnknownSubscriber(id) => MSC
                .not_found(ErrorReason::SubscriberNotFound, "Subscriber not found")
                .with("subscriber_id", id)
                .status(),
            Self::UnknownCall(call_id) => MSC
                .not_found(
                    ErrorReason::CallNotFound,
                    format!("No active call {}", call_id.0),
                )
                .with("call_id", call_id.0)
                .status(),
            Self::HlrUnavailable(detail) => {
                log::warn!("MSC: HLR lookup failed: {detail}");
                MSC.unavailable(ErrorReason::HlrUnavailable, "The HLR could not be reached")
                    .status()
            }
            Self::Unavailable {
                reason,
                message,
                metadata,
            } => with_metadata(MSC.unavailable(reason, message), metadata),
            Self::Rejected {
                reason,
                message,
                metadata,
            } => with_metadata(MSC.precondition(reason, message), metadata),
        }
    }
}

fn with_metadata(error: cdma_rpc_error::RpcError, metadata: Vec<(&'static str, String)>) -> Status {
    metadata
        .into_iter()
        .fold(error, |error, (key, value)| error.with(key, value))
        .status()
}

impl Display for ManagementError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::UnknownSubscriber(id) => write!(f, "subscriber {id} not found"),
            Self::UnknownCall(call_id) => write!(f, "no active call {}", call_id.0),
            Self::HlrUnavailable(detail) => write!(f, "HLR lookup failed: {detail}"),
            Self::Unavailable { message, .. } | Self::Rejected { message, .. } => {
                f.write_str(message)
            }
        }
    }
}

impl std::error::Error for ManagementError {}

/// MSC-owned execution metadata for one pending mobile-terminated call.
///
/// This data is not part of the A1 wire format. It exists only so the MSC can
/// stage local policy and media choices while the BSC remains responsible for
/// radio execution.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MtCallPlan {
    /// HLR subscriber identifier for the called mobile station.
    pub subscriber_id: Uuid,
    /// Authoritative IMSI chosen by the MSC for A1 paging.
    pub imsi: String,
    /// Optional local test-media file while the voice path is still migrating.
    pub audio_file: Option<String>,
    /// Optional caller-ID digits presented to the called mobile.
    pub caller_number: Option<String>,
    pub service_option: u16,
}

/// Server-side pending control request awaiting a reply from MSC logic.
#[derive(Debug)]
pub enum PendingControlRequest {
    InitiateDataCall {
        subscriber_id: Uuid,
        service_option: u16,
        response_tx: oneshot::Sender<Result<InitiateCallAccepted, ManagementError>>,
    },
    InitiateOtasp {
        subscriber_id: Uuid,
        imsi: Option<String>,
        response_tx: oneshot::Sender<Result<InitiateCallAccepted, ManagementError>>,
    },
    /// Pending request to initiate a mobile-terminated call.
    InitiateCall {
        /// Request payload.
        request: InitiateCallRequest,
        /// Reply path to the waiting client.
        response_tx: oneshot::Sender<Result<InitiateCallAccepted, ManagementError>>,
    },
    ReleaseCall {
        call_id: CallId,
        response_tx: oneshot::Sender<Result<(), ManagementError>>,
    },
    /// Pending request to list all active call sessions.
    ListCalls {
        /// Reply path to the waiting client.
        response_tx: oneshot::Sender<Result<Vec<CallSessionSnapshot>, ManagementError>>,
    },
    /// Pending request to send a mobile-terminated SMS.
    SendSms {
        /// Request payload.
        request: crate::sms::SmsSendRequest,
        response_tx: oneshot::Sender<Result<uuid::Uuid, ManagementError>>,
    },
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    #[tokio::test]
    async fn channel_roundtrips_initiate_call() {
        let (tx, mut rx) = mpsc::channel::<PendingControlRequest>(4);
        let req = InitiateCallRequest {
            subscriber_id: Uuid::parse_str("00000000-0000-0000-0000-000000000042").unwrap(),
            audio_file: Some("ring.wav".to_string()),
            caller_number: Some("5551234567".to_string()),
            service_option: None,
        };

        let (response_tx, response_rx) = oneshot::channel();
        tx.send(PendingControlRequest::InitiateCall {
            request: req,
            response_tx,
        })
        .await
        .unwrap();

        let Some(PendingControlRequest::InitiateCall {
            request,
            response_tx: reply_tx,
        }) = rx.recv().await
        else {
            panic!("expected initiate-call request");
        };
        let accepted = InitiateCallAccepted {
            call_id: CallId(42),
        };
        assert_eq!(request.audio_file.as_deref(), Some("ring.wav"));
        reply_tx.send(Ok(accepted)).unwrap();

        assert_eq!(response_rx.await.unwrap().unwrap(), accepted);
    }

    #[tokio::test]
    async fn channel_roundtrips_list_calls() {
        use cdma_ios::CallControlState;

        let (tx, mut rx) = mpsc::channel::<PendingControlRequest>(4);

        let (response_tx, response_rx) = oneshot::channel();
        tx.send(PendingControlRequest::ListCalls { response_tx })
            .await
            .unwrap();

        let Some(PendingControlRequest::ListCalls {
            response_tx: reply_tx,
        }) = rx.recv().await
        else {
            panic!("expected list-calls request");
        };
        let snapshot = CallSessionSnapshot {
            id: CallId(1),
            direction: crate::call_control::CallDirection::MobileOriginated,
            state: CallControlState::Idle,
            mobile_identity: None,
            mobile_identity_esn: None,
            media_gateway_handle: None,
        };
        reply_tx.send(Ok(vec![snapshot.clone()])).unwrap();

        let result = response_rx.await.unwrap().unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, CallId(1));
    }

    #[test]
    fn a_rejection_keeps_its_reason_and_metadata() {
        let status = ManagementError::rejected(
            ErrorReason::SubscriberNotRegistered,
            "The subscriber is not registered",
        )
        .with("subscriber_id", "abc")
        .into_status();
        assert_eq!(status.code(), tonic::Code::FailedPrecondition);
        assert_eq!(status.message(), "The subscriber is not registered");
        let info = cdma_rpc_error::error_info(&status).expect("error info");
        assert_eq!(info.reason, ErrorReason::SubscriberNotRegistered);
        assert_eq!(info.domain, "msc");
        assert_eq!(info.metadata["subscriber_id"], "abc");
    }

    #[test]
    fn an_unknown_call_is_not_found() {
        let status = ManagementError::UnknownCall(CallId(9)).into_status();
        assert_eq!(status.code(), tonic::Code::NotFound);
        assert_eq!(
            cdma_rpc_error::error_info(&status).map(|info| info.reason),
            Some(ErrorReason::CallNotFound)
        );
    }
}
