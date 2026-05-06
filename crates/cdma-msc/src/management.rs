//! MSC-owned management-plane control surface.
//!
//! Operator actions such as "initiate a call" belong to the MSC. This module
//! defines the control seam used by higher layers while the codebase is still
//! running in a mostly in-process topology.

use std::fmt::{Display, Formatter};
use tokio::sync::oneshot;
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
    /// The target subscriber is unknown to the MSC/HLR.
    UnknownSubscriber(Uuid),
    /// The management link is unavailable.
    Unavailable(&'static str),
    /// The request was rejected for a local policy reason.
    Rejected(String),
}

impl Display for ManagementError {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
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
    /// Voice service option selected by MSC call policy.
    pub service_option: u16,
}

/// Server-side pending control request awaiting a reply from MSC logic.
#[derive(Debug)]
pub enum PendingControlRequest {
    /// Pending request to initiate a mobile-terminated call.
    InitiateCall {
        /// Request payload.
        request: InitiateCallRequest,
        /// Reply path to the waiting client.
        response_tx: oneshot::Sender<Result<InitiateCallAccepted, ManagementError>>,
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
        /// Reply path to the waiting client: `Some(sms_id)` on acceptance, `None` on failure.
        response_tx: oneshot::Sender<Option<uuid::Uuid>>,
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
            media_gateway_handle: None,
        };
        reply_tx.send(Ok(vec![snapshot.clone()])).unwrap();

        let result = response_rx.await.unwrap().unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].id, CallId(1));
    }
}
