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
use cdma_smsc::model::{DeliveryAttemptState, MoSmsFingerprint, SmsDestination, SmsState};
use cdma_smsc::repository::SmscRepository;

use crate::runtime::MscA1Endpoint;

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
                    Some(result) => result,
                    None => return None,
                }
            }
            SmsDestinationKey::Imsi(imsi) => (
                SmsDestination::Imsi(imsi.clone()),
                MobileIdentity::Imsi(imsi.clone()),
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
        let encoded_payload =
            cdma_common::sms::encode_sms_deliver(&req.originating_number, &req.text, message_id);

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
                warn!("MSC SMS: failed to encode ADDS Page: {e}");
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
    pub(crate) async fn handle_adds_transfer(&self, msg: &AddsTransferMessage) {
        self.record_mo_sms(
            &msg.mobile_identity_imsi,
            msg.mobile_identity_esn.as_ref(),
            &msg.adds_user_part,
        )
        .await;
    }

    /// Handles an ADDS Deliver from BS (MO SMS received on traffic channel).
    pub(crate) async fn handle_adds_deliver_mo(
        &self,
        msg: &AddsDeliverMessage,
        mobile_identity: &MobileIdentity,
    ) {
        self.record_mo_sms(mobile_identity, None, &msg.adds_user_part)
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
                .list_submissions(PAGE_SIZE, offset, None, None, None, Some("accepted"))
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
                Some((_, mid, _)) => mid,
                None => {
                    info!(
                        "MSC SMS retry: cannot resolve {phone_number} for sms_id={} — skipping this tick",
                        submission.sms_id
                    );
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
        let encoded_payload = cdma_common::sms::encode_sms_deliver(
            &submission.originating_number,
            &submission.text,
            message_id,
        );
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
                warn!("MSC SMS retry: failed to encode ADDS Page: {e}");
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

    /// Resolves a phone number to a mobile identity (IMSI) via HLR, returning
    /// (SmsDestination, MobileIdentity, subscriber_id) or None on failure.
    async fn resolve_by_phone_number(
        &self,
        phone_number: &str,
    ) -> Option<(SmsDestination, MobileIdentity, Option<Uuid>)> {
        let subscriber = match self.hlr.get_subscriber_by_phone_number(phone_number).await {
            Ok(Some(s)) => s,
            Ok(None) => {
                info!("MSC SMS: no HLR subscriber for phone number {phone_number}");
                return None;
            }
            Err(e) => {
                warn!("MSC SMS: HLR lookup failed for {phone_number}: {e}");
                return None;
            }
        };
        let binding = match self
            .hlr
            .get_registration_binding(subscriber.subscriber_id)
            .await
        {
            Ok(Some(b)) => b,
            Ok(None) => {
                info!(
                    "MSC SMS: subscriber {} ({phone_number}) not registered",
                    subscriber.subscriber_id
                );
                return None;
            }
            Err(e) => {
                warn!("MSC SMS: registration binding lookup failed: {e}");
                return None;
            }
        };

        // Prefer IMSI for ADDS Page (spec requires IMSI in Mobile Identity IE).
        if let Some(ref imsi) = binding.imsi {
            let mobile_identity = MobileIdentity::Imsi(imsi.clone());
            return Some((
                SmsDestination::PhoneNumber(phone_number.to_string()),
                mobile_identity,
                Some(subscriber.subscriber_id),
            ));
        }

        // ESN-only mobile: ADDS Page spec requires IMSI. Cannot deliver for now.
        warn!(
            "MSC SMS: subscriber {} ({phone_number}) has no IMSI — cannot send ADDS Page",
            subscriber.subscriber_id
        );
        None
    }

    /// Decodes a MO SMS user part and records it in the SMSC.
    async fn record_mo_sms(
        &self,
        mobile_identity_imsi: &MobileIdentity,
        mobile_identity_esn: Option<&MobileIdentity>,
        adds_user_part: &AddsUserPart,
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
                // Access channel path: try ESN
                if let Some(MobileIdentity::Esn(esn)) = mobile_identity_esn {
                    // Use ESN directly
                    let esn_val = *esn;
                    self.record_mo_sms_by_esn(esn_val, &decoded).await;
                    return;
                }
                warn!("MSC SMS: ADDS Transfer with ESN-only identity, no IMSI");
                return;
            }
        };

        let originating_subscriber = match self.hlr.resolve_by_identity(None, Some(imsi)).await {
            Ok(Some(s)) => s,
            Ok(None) => {
                warn!("MSC SMS: IMSI {imsi} not in HLR — recording MO SMS with unknown originator");
                // Still record the SMS with IMSI as originator placeholder
                let _ = self
                    .smsc
                    .create_or_get_recent_mo_submission(
                        imsi, // use IMSI as originating_number placeholder
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
                return;
            }
            Err(e) => {
                warn!("MSC SMS: HLR resolve_by_identity failed: {e}");
                return;
            }
        };

        let originating_number = &originating_subscriber.phone_number;
        info!(
            "MSC SMS: MO SMS from {} to {} text=\"{}\"",
            originating_number, decoded.destination_number, decoded.text
        );

        let _ = self
            .smsc
            .create_or_get_recent_mo_submission(
                originating_number,
                &decoded.destination_number,
                &decoded.text,
                Some(originating_subscriber.subscriber_id),
                None,
                &MoSmsFingerprint {
                    teleservice_id: decoded.teleservice_id,
                    message_type: decoded.message_type,
                    message_id: decoded.message_id,
                },
            )
            .await;
    }

    async fn record_mo_sms_by_esn(&self, esn: u32, decoded: &cdma_common::sms::DecodedMoSms) {
        let originating_subscriber = match self.hlr.resolve_by_identity(Some(esn), None).await {
            Ok(Some(s)) => s,
            Ok(None) => {
                let esn_str = format!("ESN:0x{esn:08X}");
                let _ = self
                    .smsc
                    .create_or_get_recent_mo_submission(
                        &esn_str,
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
                return;
            }
            Err(e) => {
                warn!("MSC SMS: HLR resolve_by_identity(ESN) failed: {e}");
                return;
            }
        };

        let originating_number = &originating_subscriber.phone_number;
        let _ = self
            .smsc
            .create_or_get_recent_mo_submission(
                originating_number,
                &decoded.destination_number,
                &decoded.text,
                Some(originating_subscriber.subscriber_id),
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
