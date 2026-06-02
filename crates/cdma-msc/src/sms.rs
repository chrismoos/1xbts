//! MSC SMS coordinator — owns SMSC interaction for MT and MO SMS.
//!
//! The MSC coordinates all SMS delivery:
//! - MT SMS: creates SMSC submissions and delivery attempts, sends ADDS Page or
//!   ADDS Deliver to the BS via A1, then updates SMSC state on ack.
//! - MO SMS: receives ADDS Transfer or ADDS Deliver (BS→MSC direction) from the BS,
//!   decodes the C.S0015-B payload, and records the submission in the SMSC.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use log::{info, warn};
use uuid::Uuid;

use cdma_hlr::repository::HlrRepository;
use cdma_ios::{
    AddsDeliverAckMessage, AddsDeliverMessage, AddsPageAckMessage, AddsPageMessage,
    AddsTransferMessage, AddsUserPart, MobileIdentity, Tag,
};
use cdma_smsc::model::{
    DeliveryAttemptState, MoSmsFingerprint, SmsDestination, SmsState, SmsSubmission,
};
use cdma_smsc::repository::SmscRepository;

use crate::runtime::MscA1Endpoint;

/// Encode the C.S0015-B Transport Layer payload for an SMSC submission.
/// Raw user data takes precedence over text so WAP Push PDUs round-trip
/// verbatim.
fn encode_submission_payload(submission: &SmsSubmission, message_id: u16) -> Vec<u8> {
    use cdma_common::sms::{
        TELESERVICE_WMT, UserData, encode_sms_deliver, encode_sms_deliver_typed,
    };

    let teleservice = submission.teleservice_id.unwrap_or(TELESERVICE_WMT);
    if teleservice == TELESERVICE_WMT && submission.raw_user_data.is_none() {
        return encode_sms_deliver(&submission.originating_number, &submission.text, message_id);
    }
    let user_data = match submission.raw_user_data.as_deref() {
        Some(bytes) => UserData::Octet(bytes.to_vec()),
        None => UserData::Ascii7(submission.text.clone()),
    };
    encode_sms_deliver_typed(
        &submission.originating_number,
        teleservice,
        &user_data,
        message_id,
    )
}

/// Correlation between a Tag value and the SMSC submission being delivered.
struct SmsCorrelation {
    sms_id: Uuid,
    delivery_attempt_id: Uuid,
    sent_at: Instant,
}

/// MSC-side SMS coordinator.
///
/// Holds the SMSC repository client and a per-delivery tag correlation map. The
/// coordinator is driven by management requests (`send_sms`) and A1 ADDS messages
/// (`handle_*`).
pub(crate) struct MscSmsCoordinator {
    smsc: Arc<dyn SmscRepository>,
    hlr: Arc<dyn HlrRepository>,
    /// Monotonically-increasing u32 tag allocated per delivery attempt.
    next_tag: u32,
    /// Outstanding deliveries waiting for ADDS Page Ack or ADDS Deliver Ack.
    pending: HashMap<u32, SmsCorrelation>,
}

/// Outcome of an HLR phone-number lookup used by SMS send/retry paths.
enum ResolveResult {
    /// Subscriber is provisioned and currently registered — page now.
    Ready {
        destination: SmsDestination,
        mobile_identity: MobileIdentity,
        subscriber_id: Option<Uuid>,
    },
    /// Subscriber is provisioned but not currently registered. The SMSC
    /// accepts the submission and the retry sweep delivers once the MS
    /// re-registers.
    Deferred {
        destination: SmsDestination,
        subscriber_id: Uuid,
    },
    /// No provisioned subscriber for this phone number, or the HLR
    /// lookup itself failed. The submission is rejected.
    Unknown,
}

/// Where to deliver an MT SMS.
#[derive(Debug, Clone)]
pub enum SmsDestinationKey {
    /// Resolve the destination via HLR by phone number (subscriber-only).
    PhoneNumber(String),
    /// Address the mobile by IMSI directly. No HLR lookup; used for
    /// non-subscriber mobiles seen on the air.
    Imsi(String),
}

impl std::fmt::Display for SmsDestinationKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SmsDestinationKey::PhoneNumber(num) => write!(f, "phone={num}"),
            SmsDestinationKey::Imsi(imsi) => write!(f, "imsi={imsi}"),
        }
    }
}

/// Inputs to a management-plane SMS send request.
#[derive(Debug)]
pub struct SmsSendRequest {
    pub originating_number: String,
    pub text: String,
    pub destination: SmsDestinationKey,
    pub timeout_ms: u64,
    /// C.S0015-B teleservice ID. `None` selects WMT (0x1002).
    pub teleservice_id: Option<u16>,
    /// Opaque User Data bytes for non-text teleservices. When set, the BSC
    /// emits these verbatim as the bearer-data User Data sub-parameter
    /// (MSG_ENCODING=0x00 octet) instead of encoding `text`. Used to carry
    /// WAP Push PDUs end to end.
    pub raw_user_data: Option<Vec<u8>>,
}

impl MscSmsCoordinator {
    /// Creates a new coordinator backed by the given SMSC and HLR repositories.
    pub(crate) fn new(smsc: Arc<dyn SmscRepository>, hlr: Arc<dyn HlrRepository>) -> Self {
        Self {
            smsc,
            hlr,
            next_tag: 1,
            pending: HashMap::new(),
        }
    }

    /// Initiates MT SMS delivery: creates SMSC records and sends ADDS Page to BS.
    ///
    /// Returns `Some(sms_id)` on acceptance, or `None` if the destination cannot
    /// be resolved or the SMSC cannot be reached.
    pub(crate) async fn send_sms(
        &mut self,
        req: SmsSendRequest,
        a1: &dyn MscA1Endpoint,
    ) -> Option<Uuid> {
        // ── Resolve destination ──────────────────────────────────────────────
        let (destination, mobile_identity, destination_subscriber_id) = match &req.destination {
            SmsDestinationKey::PhoneNumber(phone_number) => {
                match self.resolve_by_phone_number(phone_number).await {
                    ResolveResult::Ready {
                        destination,
                        mobile_identity,
                        subscriber_id,
                    } => (destination, Some(mobile_identity), subscriber_id),
                    ResolveResult::Deferred {
                        destination,
                        subscriber_id,
                    } => (destination, None, Some(subscriber_id)),
                    ResolveResult::Unknown => return None,
                }
            }
            SmsDestinationKey::Imsi(imsi) => (
                SmsDestination::Imsi(imsi.clone()),
                Some(MobileIdentity::Imsi(imsi.clone())),
                None,
            ),
        };

        // ── Create SMSC submission ───────────────────────────────────────────
        let submission = match self
            .smsc
            .create_submission(
                &req.originating_number,
                destination,
                &req.text,
                None,
                destination_subscriber_id,
                cdma_smsc::repository::CreateSubmissionOptions {
                    teleservice_id: req.teleservice_id,
                    raw_user_data: req.raw_user_data.as_deref(),
                },
            )
            .await
        {
            Ok(s) => s,
            Err(e) => {
                warn!("MSC SMS: failed to create SMSC submission: {e}");
                return None;
            }
        };
        let sms_id = submission.sms_id;

        // ── Create delivery attempt ──────────────────────────────────────────
        let attempt = match self
            .smsc
            .create_delivery_attempt(sms_id, destination_subscriber_id)
            .await
        {
            Ok(a) => a,
            Err(e) => {
                warn!("MSC SMS: failed to create SMSC delivery attempt: {e}");
                return None;
            }
        };

        // ── Deferred path: subscriber offline, leave for the retry sweep ────
        let Some(mobile_identity) = mobile_identity else {
            let _ = self
                .smsc
                .update_delivery_attempt_state(
                    attempt.sms_delivery_attempt_id,
                    DeliveryAttemptState::Failed,
                    Some("subscriber not registered".to_string()),
                )
                .await;
            // Submission stays at its default `Accepted` state so the
            // periodic retry sweep picks it up once the binding appears.
            info!(
                "MSC SMS: accepted sms_id={sms_id} for offline subscriber — retry sweep will deliver on next registration"
            );
            return Some(sms_id);
        };

        let _ = self
            .smsc
            .update_delivery_attempt_state(
                attempt.sms_delivery_attempt_id,
                DeliveryAttemptState::Paging,
                None,
            )
            .await;
        let _ = self
            .smsc
            .update_submission_state(sms_id, SmsState::Paging, None)
            .await;

        // ── Encode SMS Deliver payload ───────────────────────────────────────
        let message_id = self.alloc_tag() as u16;
        let tag_value = self.alloc_tag();
        let encoded_payload = encode_submission_payload(&submission, message_id);

        // ── Build and send ADDS Page ─────────────────────────────────────────
        let adds_page = AddsPageMessage {
            mobile_identity,
            adds_user_part: AddsUserPart {
                burst_type: 0x03, // SMS
                data: encoded_payload,
            },
            tag: Some(Tag(tag_value)),
            slot_cycle_index: None,
        };
        let payload = match adds_page.encode() {
            Ok(p) => p,
            Err(e) => {
                let cause = format!("payload too large for A1 ADDS Page: {e}");
                warn!("MSC SMS: {cause} — marking sms_id={sms_id} Failed permanently");
                let _ = self
                    .smsc
                    .update_delivery_attempt_state(
                        attempt.sms_delivery_attempt_id,
                        DeliveryAttemptState::Failed,
                        Some(cause.clone()),
                    )
                    .await;
                let _ = self
                    .smsc
                    .update_submission_state(sms_id, SmsState::Failed, Some(cause))
                    .await;
                return None;
            }
        };
        let encoded = cdma_ios::EncodedA1Message::from_message(&cdma_ios::Message::new(
            cdma_ios::MessageType::AddsPage,
            payload,
        ));
        if let Err(e) = a1.send_to_bsc(encoded).await {
            warn!("MSC SMS: failed to send ADDS Page to BS: {e}");
            return None;
        }

        // ── Track pending delivery ───────────────────────────────────────────
        self.pending.insert(
            tag_value,
            SmsCorrelation {
                sms_id,
                delivery_attempt_id: attempt.sms_delivery_attempt_id,
                sent_at: Instant::now(),
            },
        );
        info!(
            "MSC SMS: sent ADDS Page sms_id={} tag={} attempt={}",
            sms_id, tag_value, attempt.sms_delivery_attempt_id
        );
        Some(sms_id)
    }

    /// Handles an incoming ADDS Page Ack from the BS (paging-channel delivery result).
    pub(crate) async fn handle_adds_page_ack(&mut self, msg: &AddsPageAckMessage) {
        let tag = match msg.tag {
            Some(t) => t.0,
            None => {
                warn!("MSC SMS: ADDS Page Ack missing tag — cannot correlate");
                return;
            }
        };
        let correlation = match self.pending.remove(&tag) {
            Some(c) => c,
            None => {
                warn!("MSC SMS: ADDS Page Ack tag={tag} unknown — dropped");
                return;
            }
        };
        let success = msg.cause.is_none();
        let (attempt_state, submission_state, failure_reason) = if success {
            (DeliveryAttemptState::ForwardSent, SmsState::Delivered, None)
        } else {
            // Transient per-attempt failure: record the attempt as Failed but
            // leave the submission as Accepted so the retry sweep can issue
            // another delivery attempt.
            let reason = msg
                .cause
                .map(|c| format!("ADDS Page Ack cause=0x{:02X}", c.0));
            (DeliveryAttemptState::Failed, SmsState::Accepted, reason)
        };
        let _ = self
            .smsc
            .update_delivery_attempt_state(
                correlation.delivery_attempt_id,
                attempt_state,
                failure_reason.clone(),
            )
            .await;
        let _ = self
            .smsc
            .update_submission_state(correlation.sms_id, submission_state, failure_reason)
            .await;
        info!(
            "MSC SMS: ADDS Page Ack sms_id={} success={success} tag={tag}",
            correlation.sms_id
        );
    }

    /// Handles an incoming ADDS Deliver Ack from the BS (traffic-channel delivery result).
    pub(crate) async fn handle_adds_deliver_ack(&mut self, msg: &AddsDeliverAckMessage) {
        let tag = match msg.tag {
            Some(t) => t.0,
            None => {
                warn!("MSC SMS: ADDS Deliver Ack missing tag — cannot correlate");
                return;
            }
        };
        let correlation = match self.pending.remove(&tag) {
            Some(c) => c,
            None => {
                warn!("MSC SMS: ADDS Deliver Ack tag={tag} unknown — dropped");
                return;
            }
        };
        let success = msg.cause.is_none();
        let (attempt_state, submission_state, failure_reason) = if success {
            (DeliveryAttemptState::ForwardSent, SmsState::Delivered, None)
        } else {
            // Transient per-attempt failure: record the attempt as Failed but
            // leave the submission as Accepted so the retry sweep can issue
            // another delivery attempt.
            let reason = msg
                .cause
                .map(|c| format!("ADDS Deliver Ack cause=0x{:02X}", c.0));
            (DeliveryAttemptState::Failed, SmsState::Accepted, reason)
        };
        let _ = self
            .smsc
            .update_delivery_attempt_state(
                correlation.delivery_attempt_id,
                attempt_state,
                failure_reason.clone(),
            )
            .await;
        let _ = self
            .smsc
            .update_submission_state(correlation.sms_id, submission_state, failure_reason)
            .await;
        info!(
            "MSC SMS: ADDS Deliver Ack sms_id={} success={success} tag={tag}",
            correlation.sms_id
        );
    }

    /// Handles an ADDS Transfer from BS (MO SMS received on access channel).
    pub(crate) async fn handle_adds_transfer(
        &mut self,
        msg: &AddsTransferMessage,
        a1: &dyn MscA1Endpoint,
    ) {
        self.record_mo_sms(
            &msg.mobile_identity_imsi,
            msg.mobile_identity_esn.as_ref(),
            &msg.adds_user_part,
            a1,
        )
        .await;
    }

    /// Handles an ADDS Deliver from BS (MO SMS received on traffic channel).
    pub(crate) async fn handle_adds_deliver_mo(
        &mut self,
        msg: &AddsDeliverMessage,
        mobile_identity: &MobileIdentity,
        mobile_identity_esn: Option<&MobileIdentity>,
        a1: &dyn MscA1Endpoint,
    ) {
        self.record_mo_sms(
            mobile_identity,
            mobile_identity_esn,
            &msg.adds_user_part,
            a1,
        )
        .await;
    }

    /// Expires pending deliveries older than `timeout`, restoring SMSC state to Accepted.
    pub(crate) async fn expire_pending(&mut self, timeout: Duration) {
        let now = Instant::now();
        let expired_tags: Vec<u32> = self
            .pending
            .iter()
            .filter(|(_, c)| now.duration_since(c.sent_at) > timeout)
            .map(|(k, _)| *k)
            .collect();
        for tag in expired_tags {
            if let Some(c) = self.pending.remove(&tag) {
                warn!(
                    "MSC SMS: delivery tag={} expired after {}s — restoring sms_id={} to Accepted",
                    tag,
                    timeout.as_secs(),
                    c.sms_id
                );
                let _ = self
                    .smsc
                    .update_delivery_attempt_state(
                        c.delivery_attempt_id,
                        DeliveryAttemptState::Failed,
                        Some("delivery ack timeout".to_string()),
                    )
                    .await;
                let _ = self
                    .smsc
                    .update_submission_state(c.sms_id, SmsState::Accepted, None)
                    .await;
            }
        }
    }

    /// One-shot recovery for submissions left in `Paging` across a restart.
    ///
    /// The in-flight ADDS Page correlation lives only in the in-memory
    /// `pending` map. If the MSC process restarts (crash or operator
    /// redeploy) while a page is outstanding, the map is lost: there is
    /// nothing left to match an incoming ack against, and no `sent_at`
    /// Instant for `expire_pending` to compare. The submission and its
    /// attempt remain in `Paging` forever — the regular retry sweep
    /// explicitly skips that state.
    ///
    /// On startup, scan for submissions in `Paging` whose latest attempt
    /// is also in `Paging` and was last updated more than `stale_after`
    /// ago. Transition the attempt to `Failed` and the submission back to
    /// `Accepted` so the retry sweep can pick them up.
    pub(crate) async fn recover_stuck_paging(&mut self, stale_after: Duration) {
        const PAGE_SIZE: u32 = 100;
        let cutoff = chrono::Utc::now()
            - chrono::Duration::from_std(stale_after).unwrap_or(chrono::Duration::seconds(120));
        let mut offset: u32 = 0;
        let mut recovered: u32 = 0;
        loop {
            let (submissions, total) = match self
                .smsc
                .list_submissions(PAGE_SIZE, offset, None, None, None, Some("paging"), None)
                .await
            {
                Ok(p) => p,
                Err(e) => {
                    warn!("MSC SMS startup recovery: list_submissions failed: {e}");
                    return;
                }
            };
            for submission in &submissions {
                let attempts = match self.smsc.get_delivery_attempts(submission.sms_id).await {
                    Ok(a) => a,
                    Err(e) => {
                        warn!(
                            "MSC SMS startup recovery: get_delivery_attempts({}) failed: {e}",
                            submission.sms_id
                        );
                        continue;
                    }
                };
                let Some(latest) = attempts.iter().max_by_key(|a| a.attempt_number) else {
                    continue;
                };
                if latest.state != DeliveryAttemptState::Paging {
                    continue;
                }
                if latest.updated_at >= cutoff {
                    continue;
                }
                if let Err(e) = self
                    .smsc
                    .update_delivery_attempt_state(
                        latest.sms_delivery_attempt_id,
                        DeliveryAttemptState::Failed,
                        Some(
                            "recovered after MSC restart — in-memory pending entry lost"
                                .to_string(),
                        ),
                    )
                    .await
                {
                    warn!(
                        "MSC SMS startup recovery: failed to update attempt {}: {e}",
                        latest.sms_delivery_attempt_id
                    );
                    continue;
                }
                if let Err(e) = self
                    .smsc
                    .update_submission_state(submission.sms_id, SmsState::Accepted, None)
                    .await
                {
                    warn!(
                        "MSC SMS startup recovery: failed to update submission {}: {e}",
                        submission.sms_id
                    );
                    continue;
                }
                recovered += 1;
                info!(
                    "MSC SMS: recovered stuck-paging sms_id={} attempt_id={} (last updated {})",
                    submission.sms_id, latest.sms_delivery_attempt_id, latest.updated_at
                );
            }
            offset += submissions.len() as u32;
            if offset >= total || submissions.is_empty() {
                break;
            }
        }
        if recovered > 0 {
            info!(
                "MSC SMS: startup recovery transitioned {recovered} stuck-paging submission(s) back to Accepted"
            );
        }
    }

    /// Sweep retry-eligible submissions and create a fresh delivery attempt
    /// for each. A submission is eligible iff it is in `Accepted` and its
    /// most recent delivery attempt is `Failed` with `completed_at`
    /// (falling back to `updated_at`) older than `retry_after`.
    ///
    /// Submissions still in flight (`Paging`, `PageResponseReceived`),
    /// already delivered (`Delivered`), expired (`Expired`), or
    /// terminally `Failed` are skipped.
    pub(crate) async fn retry_eligible_sweep(
        &mut self,
        a1: &dyn MscA1Endpoint,
        retry_after: Duration,
    ) {
        // Page through Accepted submissions. 100 per page is generous for a
        // home-lab MSC; larger deployments would want pagination but the
        // cost is bounded by Postgres index scans.
        const PAGE_SIZE: u32 = 100;
        let mut offset: u32 = 0;
        let cutoff = chrono::Utc::now()
            - chrono::Duration::from_std(retry_after).unwrap_or(chrono::Duration::seconds(10));
        loop {
            let (submissions, total) = match self
                .smsc
                .list_submissions(PAGE_SIZE, offset, None, None, None, Some("accepted"), None)
                .await
            {
                Ok(p) => p,
                Err(e) => {
                    warn!("MSC SMS retry sweep: list_submissions failed: {e}");
                    return;
                }
            };
            for submission in &submissions {
                let attempts = match self.smsc.get_delivery_attempts(submission.sms_id).await {
                    Ok(a) => a,
                    Err(e) => {
                        warn!(
                            "MSC SMS retry sweep: get_delivery_attempts({}) failed: {e}",
                            submission.sms_id
                        );
                        continue;
                    }
                };
                // Skip submissions whose tag is still active in `pending` —
                // they were re-sent in a previous tick and we are waiting on
                // the BS ack.
                if self.pending.values().any(|c| c.sms_id == submission.sms_id) {
                    continue;
                }
                let Some(latest) = attempts.iter().max_by_key(|a| a.attempt_number) else {
                    continue;
                };
                if latest.state != DeliveryAttemptState::Failed {
                    continue;
                }
                let completed = latest.completed_at.unwrap_or(latest.updated_at);
                if completed > cutoff {
                    continue;
                }
                self.retry_submission(submission, a1).await;
            }
            offset += submissions.len() as u32;
            if offset >= total || submissions.is_empty() {
                break;
            }
        }
    }

    /// Issue a fresh delivery attempt for an existing submission, re-sending
    /// the ADDS Page. The submission row already carries the destination
    /// addressing, so this skips destination resolution.
    async fn retry_submission(
        &mut self,
        submission: &cdma_smsc::model::SmsSubmission,
        a1: &dyn MscA1Endpoint,
    ) {
        // Reconstruct destination from the submission row.
        let mobile_identity = if let Some(imsi) = submission.destination_imsi.as_deref() {
            MobileIdentity::Imsi(imsi.to_string())
        } else if let Some(esn) = submission.destination_esn {
            MobileIdentity::Esn(esn)
        } else if let Some(phone_number) = submission.destination_number.as_deref() {
            // Phone-number-addressed submissions need a fresh HLR lookup
            // because the registration binding (IMSI) may have rotated.
            match self.resolve_by_phone_number(phone_number).await {
                ResolveResult::Ready {
                    mobile_identity, ..
                } => mobile_identity,
                ResolveResult::Deferred { .. } => {
                    info!(
                        "MSC SMS retry: subscriber for {phone_number} still offline — skipping this tick (sms_id={})",
                        submission.sms_id
                    );
                    return;
                }
                ResolveResult::Unknown => {
                    let cause =
                        format!("destination subscriber not provisioned (phone={phone_number})");
                    warn!(
                        "MSC SMS retry: cannot resolve {phone_number} for sms_id={} — marking Failed permanently",
                        submission.sms_id
                    );
                    let _ = self
                        .smsc
                        .update_submission_state(submission.sms_id, SmsState::Failed, Some(cause))
                        .await;
                    return;
                }
            }
        } else {
            warn!(
                "MSC SMS retry: submission {} has no addressable identity — marking Failed",
                submission.sms_id
            );
            let _ = self
                .smsc
                .update_submission_state(
                    submission.sms_id,
                    SmsState::Failed,
                    Some("submission has no destination identity".to_string()),
                )
                .await;
            return;
        };

        let attempt = match self
            .smsc
            .create_delivery_attempt(submission.sms_id, submission.destination_subscriber_id)
            .await
        {
            Ok(a) => a,
            Err(e) => {
                warn!(
                    "MSC SMS retry: create_delivery_attempt({}) failed: {e}",
                    submission.sms_id
                );
                return;
            }
        };
        let _ = self
            .smsc
            .update_delivery_attempt_state(
                attempt.sms_delivery_attempt_id,
                DeliveryAttemptState::Paging,
                None,
            )
            .await;
        let _ = self
            .smsc
            .update_submission_state(submission.sms_id, SmsState::Paging, None)
            .await;

        let message_id = self.alloc_tag() as u16;
        let tag_value = self.alloc_tag();
        let encoded_payload = encode_submission_payload(submission, message_id);
        let adds_page = AddsPageMessage {
            mobile_identity,
            adds_user_part: AddsUserPart {
                burst_type: 0x03, // SMS
                data: encoded_payload,
            },
            tag: Some(Tag(tag_value)),
            slot_cycle_index: None,
        };
        let payload = match adds_page.encode() {
            Ok(p) => p,
            Err(e) => {
                let cause = format!("payload too large for A1 ADDS Page: {e}");
                warn!(
                    "MSC SMS retry: {cause} — marking sms_id={} Failed permanently",
                    submission.sms_id
                );
                let _ = self
                    .smsc
                    .update_delivery_attempt_state(
                        attempt.sms_delivery_attempt_id,
                        DeliveryAttemptState::Failed,
                        Some(cause.clone()),
                    )
                    .await;
                let _ = self
                    .smsc
                    .update_submission_state(submission.sms_id, SmsState::Failed, Some(cause))
                    .await;
                return;
            }
        };
        let encoded = cdma_ios::EncodedA1Message::from_message(&cdma_ios::Message::new(
            cdma_ios::MessageType::AddsPage,
            payload,
        ));
        if let Err(e) = a1.send_to_bsc(encoded).await {
            warn!("MSC SMS retry: send_to_bsc failed: {e}");
            return;
        }
        self.pending.insert(
            tag_value,
            SmsCorrelation {
                sms_id: submission.sms_id,
                delivery_attempt_id: attempt.sms_delivery_attempt_id,
                sent_at: Instant::now(),
            },
        );
        info!(
            "MSC SMS retry: re-sent ADDS Page sms_id={} tag={} attempt={} (#{}) ",
            submission.sms_id, tag_value, attempt.sms_delivery_attempt_id, attempt.attempt_number
        );
    }

    // ── Internal helpers ─────────────────────────────────────────────────────

    fn alloc_tag(&mut self) -> u32 {
        let tag = self.next_tag;
        self.next_tag = self.next_tag.wrapping_add(1).max(1);
        tag
    }

    /// Resolves a phone number against the HLR into a `ResolveResult`.
    async fn resolve_by_phone_number(&self, phone_number: &str) -> ResolveResult {
        let resolved = match self.hlr.get_subscriber_by_phone_number(phone_number).await {
            Ok(Some(r)) => r,
            Ok(None) => {
                info!("MSC SMS: no HLR subscriber for phone number {phone_number}");
                return ResolveResult::Unknown;
            }
            Err(e) => {
                warn!("MSC SMS: HLR lookup failed for {phone_number}: {e}");
                return ResolveResult::Unknown;
            }
        };
        let subscriber_id = resolved.subscriber.subscriber_id;

        let Some(binding) = resolved.binding.as_ref() else {
            info!(
                "MSC SMS: subscriber {subscriber_id} ({phone_number}) not registered — deferring for retry sweep",
            );
            return ResolveResult::Deferred {
                destination: SmsDestination::PhoneNumber(phone_number.to_string()),
                subscriber_id,
            };
        };

        // Prefer IMSI for ADDS Page (spec requires IMSI in Mobile Identity IE).
        if let Some(ref imsi) = binding.imsi {
            return ResolveResult::Ready {
                destination: SmsDestination::PhoneNumber(phone_number.to_string()),
                mobile_identity: MobileIdentity::Imsi(imsi.clone()),
                subscriber_id: Some(subscriber_id),
            };
        }

        // Provisioned + registered but no IMSI on the binding — ADDS Page
        // requires IMSI. Treat as deferred so the sweep can pick up an
        // updated binding rather than rejecting outright.
        warn!(
            "MSC SMS: subscriber {subscriber_id} ({phone_number}) has no IMSI on current binding — deferring"
        );
        ResolveResult::Deferred {
            destination: SmsDestination::PhoneNumber(phone_number.to_string()),
            subscriber_id,
        }
    }

    /// Records a MO SMS in the SMSC; if the destination is a known
    /// subscriber, also queues MT delivery to them.
    async fn record_mo_sms(
        &mut self,
        mobile_identity_imsi: &MobileIdentity,
        mobile_identity_esn: Option<&MobileIdentity>,
        adds_user_part: &AddsUserPart,
        a1: &dyn MscA1Endpoint,
    ) {
        if adds_user_part.burst_type != 0x03 {
            warn!(
                "MSC SMS: ADDS MO message burst_type=0x{:02X} (not SMS) — dropped",
                adds_user_part.burst_type
            );
            return;
        }

        let decoded = match cdma_common::sms::decode_mo_sms(&adds_user_part.data) {
            Some(d) => d,
            None => {
                warn!("MSC SMS: failed to decode C.S0015-B MO SMS payload");
                return;
            }
        };

        // Resolve originating phone number from IMSI via HLR.
        let imsi = match mobile_identity_imsi {
            MobileIdentity::Imsi(s) => s.as_str(),
            MobileIdentity::Esn(_) => {
                if let Some(MobileIdentity::Esn(esn)) = mobile_identity_esn {
                    let esn_originator = format!("ESN:0x{esn:08X}");
                    self.record_mo_sms_unknown_originator(&esn_originator, &decoded)
                        .await;
                    return;
                }
                warn!("MSC SMS: ADDS Transfer with ESN-only identity, no IMSI");
                return;
            }
            MobileIdentity::Meid(_) => {
                warn!("MSC SMS: ADDS Transfer with MEID-only identity, no IMSI");
                return;
            }
        };
        let esn = match mobile_identity_esn {
            Some(MobileIdentity::Esn(esn)) => Some(*esn),
            _ => None,
        };
        let identity_key =
            match cdma_hlr::model::MobileIdentityKey::from_parts(Some(imsi), esn, None) {
                Ok(identity_key) => identity_key,
                Err(_) => {
                    warn!(
                        "MSC SMS: MO SMS from IMSI {imsi} has no hardware identity for HLR lookup"
                    );
                    self.record_mo_sms_unknown_originator(imsi, &decoded).await;
                    return;
                }
            };

        let originating = match self.hlr.resolve_by_identity(&identity_key).await {
            Ok(Some(r)) => r.subscriber,
            Ok(None) => {
                warn!("MSC SMS: IMSI {imsi} not in HLR — recording MO SMS with unknown originator");
                self.record_mo_sms_unknown_originator(imsi, &decoded).await;
                return;
            }
            Err(e) => {
                warn!("MSC SMS: HLR resolve_by_identity failed: {e}");
                return;
            }
        };

        let originating_number = originating.phone_number.clone();
        info!(
            "MSC SMS: MO SMS from {} to {} text=\"{}\"",
            originating_number, decoded.destination_number, decoded.text
        );

        let dest_subscriber_id = match self
            .hlr
            .get_subscriber_by_phone_number(&decoded.destination_number)
            .await
        {
            Ok(Some(r)) => Some(r.subscriber.subscriber_id),
            _ => None,
        };

        let mo = self
            .smsc
            .create_or_get_recent_mo_submission(
                &originating_number,
                &decoded.destination_number,
                &decoded.text,
                Some(originating.subscriber_id),
                dest_subscriber_id,
                &MoSmsFingerprint {
                    teleservice_id: decoded.teleservice_id,
                    message_type: decoded.message_type,
                    message_id: decoded.message_id,
                },
            )
            .await;

        if let (Ok((submission, _)), Some(_)) = (mo, dest_subscriber_id) {
            self.retry_submission(&submission, a1).await;
        }
    }

    async fn record_mo_sms_unknown_originator(
        &self,
        originating_number: &str,
        decoded: &cdma_common::sms::DecodedMoSms,
    ) {
        let _ = self
            .smsc
            .create_or_get_recent_mo_submission(
                originating_number,
                &decoded.destination_number,
                &decoded.text,
                None,
                None,
                &MoSmsFingerprint {
                    teleservice_id: decoded.teleservice_id,
                    message_type: decoded.message_type,
                    message_id: decoded.message_id,
                },
            )
            .await;
    }
}
