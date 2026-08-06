//! Paging-channel TX, retries, overhead, and supplier installation.
//!
//! Directed F-PCH messages (orders, data bursts, channel assignments) are
//! sent here. The BSC does **not** allocate MSG_SEQ for directed PCH — the
//! BTS paging supplier owns L2 termination and assigns on-air ARQ fields.

use std::{
    collections::HashMap,
    ops::{Deref, DerefMut},
    time::{Duration, Instant},
};

use cdma_abis::control::typed::pch_message_transfer_ack_cause::SMS_MESSAGE_TOO_LONG;
use cdma_common::consts::{SR1_CHIP_RATE_HZ, SR1_CHIPS_PER_80MS};
use cdma_common::error::Error;
use cdma_common::lac::MessageControlStatusBlock;
use cdma_common::lac::message_types::MessageId;
use cdma_common::lac::paging_messages::{
    GeneralPageRecord, MsAddress, MsPageAddress, PagingChannelMessage, PagingMessageKind,
};
use cdma_common::mac::ChannelType;
use log::{debug, info, trace, warn};
use uuid::Uuid;

use crate::abis_edge::PchTransferAckEvent;
use crate::addressing::format_ms_address;

use super::{
    Bsc, MsState, PAGE_RETRY_GUARD_MS, SmsAckKey, SmsRequest, VoiceLegRole,
    build_scheduled_message, next_bsc_event_id, next_pch_correlation_id,
};

pub(crate) fn mobile_identity_for_ms_address(
    addr: &MsAddress,
) -> cdma_abis::control::typed::MobileIdentity {
    use cdma_abis::control::typed::MobileIdentity;

    match addr {
        MsAddress::Esn(esn) => MobileIdentity::Esn(*esn),
        MsAddress::ImsiS {
            imsi_m_s1,
            imsi_m_s2,
        } => MobileIdentity::Imsi(cdma_common::paging::imsi_s_to_digits(
            *imsi_m_s1, *imsi_m_s2,
        )),
        MsAddress::ImsiClass0 {
            imsi_m_s1,
            imsi_m_s2,
            mcc,
            imsi_11_12,
        } => {
            let imsi = cdma_common::paging::mcc_to_digits(*mcc)
                .zip(cdma_common::paging::imsi_11_12_to_digits(*imsi_11_12))
                .map(|(mcc_digits, imsi_11_12_digits)| {
                    format!(
                        "{}{}{}",
                        mcc_digits,
                        imsi_11_12_digits,
                        cdma_common::paging::imsi_s_to_digits(*imsi_m_s1, *imsi_m_s2)
                    )
                })
                .unwrap_or_else(|| cdma_common::paging::imsi_s_to_digits(*imsi_m_s1, *imsi_m_s2));
            MobileIdentity::Imsi(imsi)
        }
    }
}

fn mobile_identity_for_page_address(
    page_addr: &MsPageAddress,
) -> cdma_abis::control::typed::MobileIdentity {
    use cdma_abis::control::typed::MobileIdentity;

    match page_addr {
        MsPageAddress::Esn(esn) => MobileIdentity::Esn(*esn),
        MsPageAddress::ImsiS {
            imsi_m_s1,
            imsi_m_s2,
            mcc,
            imsi_11_12,
        } => match (mcc, imsi_11_12) {
            (Some(mcc), Some(imsi_11_12)) => {
                mobile_identity_for_ms_address(&MsAddress::ImsiClass0 {
                    imsi_m_s1: *imsi_m_s1,
                    imsi_m_s2: *imsi_m_s2,
                    mcc: *mcc,
                    imsi_11_12: *imsi_11_12,
                })
            }
            _ => MobileIdentity::Imsi(cdma_common::paging::imsi_s_to_digits(
                *imsi_m_s1, *imsi_m_s2,
            )),
        },
    }
}

/// Forward-link paging channel event for gRPC/UI streaming.
/// Carries the full structured message + MCSB metadata.
#[derive(Debug, Clone)]
pub struct PagingEvent {
    pub event_id: String,
    pub message: PagingChannelMessage,
    pub mcsb: MessageControlStatusBlock,
    /// Microseconds since Unix epoch when this event was emitted.
    pub timestamp_us: u64,
}

/// Convert a BTS `PchTransmitEvent` into a `PagingChannelMessage` for the
/// BSC paging broadcast. Decode failures are returned to the caller; callers
/// must not synthesize a placeholder message because that hides wire bugs.
pub fn pch_transmit_event_to_paging_message(
    evt: &cdma_bts::bts::paging_supplier::PchTransmitEvent,
) -> Result<PagingChannelMessage, Error> {
    let mut bs = cdma_common::bits::Bitstream::new_bytes(&evt.sdu_bytes);
    if evt.length_bits > bs.len() {
        return Err(format!(
            "PCH {} SDU length_bits={} exceeds packed payload bits={}",
            evt.message_id.tag(),
            evt.length_bits,
            bs.len()
        )
        .into());
    }
    if evt.length_bits < bs.len() {
        let _ = bs.drain(evt.length_bits..bs.len());
    }
    PagingChannelMessage::from_sdu(evt.message_id, &mut bs).map_err(|e| {
        format!(
            "PCH {} body decode failed: {e}; sdu={}",
            evt.message_id.tag(),
            evt.sdu_bytes
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<Vec<_>>()
                .join("")
        )
        .into()
    })
}

/// Convert a BTS `PchTransmitEvent` into a BSC-domain `PagingEvent`.
pub fn pch_transmit_event_to_paging_event(
    evt: &cdma_bts::bts::paging_supplier::PchTransmitEvent,
) -> Result<PagingEvent, Error> {
    let mcsb = MessageControlStatusBlock {
        channel: ChannelType::FPch,
        length_bits: evt.length_bits,
        mobile_p_rev: None,
        extended_encryption: false,
        message_id: evt.message_id,
        requested_tx_time: None,
        tx_deadline: None,
        address: evt.address.clone(),
        ack_seq: evt.ack_seq,
        msg_seq: evt.msg_seq,
        ack_req: evt.ack_req,
        valid_ack: true,
        overhead_mcc: evt.overhead_mcc,
        overhead_imsi_11_12: evt.overhead_imsi_11_12,
    };
    Ok(PagingEvent {
        event_id: next_bsc_event_id("paging"),
        message: pch_transmit_event_to_paging_message(evt)?,
        mcsb,
        timestamp_us: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros() as u64,
    })
}

/// A page that is actively being retried until the MS responds or the timeout
/// expires.
pub(crate) struct PendingPage {
    pub(crate) sms: SmsRequest,
    pub(crate) page_address: MsPageAddress,
    pub(crate) fwd_address: MsAddress,
    pub(crate) pgslot: Option<u16>,
    pub(crate) slot_cycle_index: u8,
    pub(crate) started_at: Instant,
    pub(crate) timeout: Duration,
    pub(crate) retry_count: u32,
    pub(crate) next_retry_at: tokio::time::Instant,
    /// Chip at which the last GPM was scheduled, so the next retry targets
    /// a distinct assigned slot (not the same one again).
    pub(crate) last_target_chip: Option<u64>,
    /// Stored msg_seq from the first GPM send, reused on retries.
    pub(crate) page_msg_seq: Option<u8>,
    /// Abis PchMessageTransfer correlation for the BTS-owned GPM page record.
    pub(crate) page_correlation_id: Option<u32>,
}

/// Voice page that is actively being retried until the MS responds or times out.
#[derive(Clone)]
pub(crate) struct PendingVoicePage {
    pub(crate) session_id: Uuid,
    pub(crate) page_address: MsPageAddress,
    pub(crate) fwd_address: MsAddress,
    pub(crate) pgslot: Option<u16>,
    pub(crate) slot_cycle_index: u8,
    pub(crate) started_at: Instant,
    pub(crate) timeout: Duration,
    pub(crate) retry_count: u32,
    pub(crate) next_retry_at: tokio::time::Instant,
    pub(crate) last_target_chip: Option<u64>,
    pub(crate) service_option: u16,
    pub(crate) leg_role: VoiceLegRole,
    pub(crate) a1_tag: Option<cdma_ios::Tag>,
    pub(crate) a1_call_id: Option<u64>,
    pub(crate) imsi: Option<String>,
    /// Stored msg_seq from the first GPM send, reused on retries.
    pub(crate) page_msg_seq: Option<u8>,
    /// Abis PchMessageTransfer correlation for the BTS-owned GPM page record.
    pub(crate) page_correlation_id: Option<u32>,
}

pub(crate) struct SmsPageRetry {
    pub(crate) page_address: MsPageAddress,
    pub(crate) fwd_address: MsAddress,
    pub(crate) pgslot: Option<u16>,
    pub(crate) slot_cycle_index: u8,
    pub(crate) retry_count: u32,
    pub(crate) last_target_chip: Option<u64>,
    pub(crate) page_msg_seq: Option<u8>,
}

pub(crate) struct VoicePageRetry {
    pub(crate) page_address: MsPageAddress,
    pub(crate) fwd_address: MsAddress,
    pub(crate) pgslot: Option<u16>,
    pub(crate) slot_cycle_index: u8,
    pub(crate) retry_count: u32,
    pub(crate) last_target_chip: Option<u64>,
    pub(crate) service_option: u16,
    pub(crate) page_msg_seq: Option<u8>,
}

pub(crate) struct ScheduledPageSlot {
    pub(crate) target_chip: u64,
    pub(crate) wait_ms: u64,
    pub(crate) effective_slot_cycle_index: u8,
}

pub(crate) struct PagingSlotPlanner {
    max_slot_cycle_index: u8,
}

enum GeneralPageRecordLog {
    Class0 {
        subclass: u8,
        imsi_s: u64,
        mcc: Option<u16>,
        imsi_11_12: Option<u8>,
    },
    Class1 {
        esn: u32,
    },
}

struct BuiltGeneralPageRecord {
    record: GeneralPageRecord,
    log: GeneralPageRecordLog,
}

impl PagingSlotPlanner {
    pub(crate) fn new(max_slot_cycle_index: u8) -> Self {
        Self {
            max_slot_cycle_index,
        }
    }

    pub(crate) fn effective_slot_cycle_index(&self, slot_cycle_index: u8) -> u8 {
        // Per C.S0005-E 3.6.2.1.3, when the BS knows the mobile's preferred
        // slot cycle index, it shall use max(0, min(preferred, maximum)) if it
        // does not support negative slot cycle indices. Our BS currently
        // advertises only MAX_SLOT_CYCLE_INDEX, so clamp the MS-reported SCI
        // to that ceiling before computing assigned paging slots.
        slot_cycle_index.min(self.max_slot_cycle_index)
    }

    pub(crate) fn assigned_paging_slot_chip(
        &self,
        search_from: u64,
        pgslot: u16,
        slot_cycle_index: u8,
        chip_rate_hz: u64,
    ) -> u64 {
        cdma_common::paging::next_assigned_slot_chip(
            search_from,
            pgslot,
            self.effective_slot_cycle_index(slot_cycle_index),
            chip_rate_hz,
        )
    }

    pub(crate) fn next_retry_at(
        &self,
        pgslot: Option<u16>,
        slot_cycle_index: u8,
        last_target_chip: Option<u64>,
    ) -> tokio::time::Instant {
        let Some(pg) = pgslot else {
            // No slot info: retry in 1 second.
            return tokio::time::Instant::now() + Duration::from_secs(1);
        };

        let now_chips = cdma_common::time::chips_since_epoch(chrono::Utc::now(), SR1_CHIP_RATE_HZ);
        let search_from = self.search_from(now_chips, last_target_chip);
        let target_chip =
            self.assigned_paging_slot_chip(search_from, pg, slot_cycle_index, SR1_CHIP_RATE_HZ);
        let wait_ms = target_chip.saturating_sub(now_chips) * 1000 / SR1_CHIP_RATE_HZ;
        let enqueue_wait_ms = wait_ms.saturating_sub(PAGE_RETRY_GUARD_MS);
        tokio::time::Instant::now() + Duration::from_millis(enqueue_wait_ms)
    }

    pub(crate) fn scheduled_slot(
        &self,
        pgslot: Option<u16>,
        slot_cycle_index: u8,
        after_chip: Option<u64>,
    ) -> Option<ScheduledPageSlot> {
        let pg = pgslot?;
        let now_chips = cdma_common::time::chips_since_epoch(chrono::Utc::now(), SR1_CHIP_RATE_HZ);
        let search_from = self.search_from(now_chips, after_chip);
        let target_chip =
            self.assigned_paging_slot_chip(search_from, pg, slot_cycle_index, SR1_CHIP_RATE_HZ);
        let wait_ms = target_chip.saturating_sub(now_chips) * 1000 / SR1_CHIP_RATE_HZ;

        Some(ScheduledPageSlot {
            target_chip,
            wait_ms,
            effective_slot_cycle_index: self.effective_slot_cycle_index(slot_cycle_index),
        })
    }

    fn search_from(&self, now_chips: u64, last_target_chip: Option<u64>) -> u64 {
        match last_target_chip {
            Some(last) => now_chips.max(last + SR1_CHIPS_PER_80MS),
            None => now_chips,
        }
    }
}

fn build_general_page_record(
    page_addr: &MsPageAddress,
    page_seq: u8,
    service_option: Option<u16>,
    overhead_mcc: u16,
    overhead_imsi_11_12: u8,
) -> BuiltGeneralPageRecord {
    let special_service = service_option.is_some();
    match page_addr {
        MsPageAddress::ImsiS {
            imsi_m_s1,
            imsi_m_s2,
            mcc,
            imsi_11_12,
        } => {
            let imsi_s = ((*imsi_m_s2 as u64) << 24) | (*imsi_m_s1 as u64);
            // Page address stores fully-resolved values. Compare against
            // current overhead to select the minimum subclass:
            // subclass 0 = IMSI_S only (both implied by overhead)
            // subclass 1 = IMSI_S + IMSI_11_12 (MCC implied)
            // subclass 2 = IMSI_S + MCC (IMSI_11_12 implied; roamer)
            // subclass 3 = IMSI_S + MCC + IMSI_11_12 (both differ)
            let (subclass, use_mcc, use_11_12) = match (mcc, imsi_11_12) {
                (Some(ms_mcc), Some(ms_11_12)) => {
                    let mcc_implied = overhead_mcc == 0x03ff || overhead_mcc == *ms_mcc;
                    let imsi_11_12_implied =
                        overhead_imsi_11_12 == 0x7f || overhead_imsi_11_12 == *ms_11_12;
                    match (mcc_implied, imsi_11_12_implied) {
                        (true, true) => (0, None, None),
                        (true, false) => (1, None, Some(*ms_11_12)),
                        (false, true) => (2, Some(*ms_mcc), None),
                        (false, false) => (3, Some(*ms_mcc), Some(*ms_11_12)),
                    }
                }
                // Legacy fallback for page addresses without resolved fields.
                (Some(_), None) => (2, *mcc, None),
                (None, Some(_)) => (1, None, *imsi_11_12),
                (None, None) => (0, None, None),
            };

            BuiltGeneralPageRecord {
                record: GeneralPageRecord::Class0 {
                    page_subclass: subclass,
                    msg_seq: page_seq,
                    imsi_s: Some(imsi_s),
                    imsi_11_12: use_11_12,
                    mcc: use_mcc,
                    imsi_addr_num: None,
                    imsi_m_s1: Some(*imsi_m_s1),
                    imsi_m_s2: Some(*imsi_m_s2),
                    special_service,
                    service_option,
                },
                log: GeneralPageRecordLog::Class0 {
                    subclass,
                    imsi_s,
                    mcc: use_mcc,
                    imsi_11_12: use_11_12,
                },
            }
        }
        MsPageAddress::Esn(esn) => BuiltGeneralPageRecord {
            record: GeneralPageRecord::Class1 {
                msg_seq: page_seq,
                esn: *esn,
                special_service,
                service_option,
            },
            log: GeneralPageRecordLog::Class1 { esn: *esn },
        },
    }
}

#[derive(Default)]
pub(crate) struct PagingState {
    pending_page: Option<PendingPage>,
    pending_voice_page: Option<PendingVoicePage>,
}

#[derive(Default)]
pub(crate) struct PagingService {
    state: PagingState,
    #[allow(dead_code)]
    next_paging_message_index: usize,
    gpm_page_seq: parking_lot::Mutex<HashMap<MsPageAddress, u8>>,
}

impl PagingService {
    pub(crate) fn new() -> Self {
        Self::default()
    }

    pub(crate) fn next_gpm_page_seq(&self, page_addr: &MsPageAddress) -> u8 {
        let mut map = self.gpm_page_seq.lock();
        let entry = map.entry(page_addr.clone()).or_insert(0);
        let seq = *entry;
        *entry = (seq + 1) % 8;
        seq
    }

    #[allow(dead_code)]
    pub(crate) fn next_default_message_kind(
        &mut self,
        schedule: &[PagingMessageKind],
    ) -> PagingMessageKind {
        let kind = schedule[self.next_paging_message_index % schedule.len()];
        self.next_paging_message_index += 1;
        kind
    }
}

impl Deref for PagingService {
    type Target = PagingState;

    fn deref(&self) -> &Self::Target {
        &self.state
    }
}

impl DerefMut for PagingService {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.state
    }
}

impl PagingState {
    pub(crate) fn has_pending_page(&self) -> bool {
        self.pending_page.is_some() || self.pending_voice_page.is_some()
    }

    pub(crate) fn has_pending_sms_page(&self) -> bool {
        self.pending_page.is_some()
    }

    /// Is there a pending SMS page in flight to this specific MS?
    pub(crate) fn pending_sms_page_for_address(&self, addr: &MsAddress) -> bool {
        self.pending_page
            .as_ref()
            .is_some_and(|p| p.fwd_address == *addr)
    }

    pub(crate) fn has_pending_voice_page(&self) -> bool {
        self.pending_voice_page.is_some()
    }

    pub(crate) fn next_retry_at(&self) -> Option<tokio::time::Instant> {
        fn timeout_deadline(started_at: Instant, timeout: Duration) -> tokio::time::Instant {
            tokio::time::Instant::now() + timeout.saturating_sub(started_at.elapsed())
        }

        match (
            self.pending_page
                .as_ref()
                .map(|p| timeout_deadline(p.started_at, p.timeout)),
            self.pending_voice_page
                .as_ref()
                .map(|p| timeout_deadline(p.started_at, p.timeout)),
        ) {
            (Some(a), Some(b)) => Some(a.min(b)),
            (Some(a), None) => Some(a),
            (None, Some(b)) => Some(b),
            (None, None) => None,
        }
    }

    pub(crate) fn queue_sms_page(&mut self, pending: PendingPage) {
        self.pending_page = Some(pending);
    }

    pub(crate) fn queue_voice_page(&mut self, pending: PendingVoicePage) {
        self.pending_voice_page = Some(pending);
    }

    pub(crate) fn record_sms_page_sent(
        &mut self,
        target_chip: Option<u64>,
        next_retry_at: tokio::time::Instant,
        page_msg_seq: u8,
        page_correlation_id: Option<u32>,
    ) {
        if let Some(pending) = self.pending_page.as_mut() {
            pending.last_target_chip = target_chip;
            pending.next_retry_at = next_retry_at;
            pending.page_msg_seq = Some(page_msg_seq);
            pending.page_correlation_id = page_correlation_id;
        }
    }

    pub(crate) fn record_voice_page_sent(
        &mut self,
        target_chip: Option<u64>,
        next_retry_at: tokio::time::Instant,
        page_msg_seq: u8,
        page_correlation_id: Option<u32>,
    ) {
        if let Some(pending) = self.pending_voice_page.as_mut() {
            pending.last_target_chip = target_chip;
            pending.next_retry_at = next_retry_at;
            pending.page_msg_seq = Some(page_msg_seq);
            pending.page_correlation_id = page_correlation_id;
        }
    }

    pub(crate) fn record_sms_retry_scheduled(
        &mut self,
        target_chip: Option<u64>,
        next_retry_at: tokio::time::Instant,
    ) {
        if let Some(pending) = self.pending_page.as_mut() {
            pending.last_target_chip = target_chip;
            pending.next_retry_at = next_retry_at;
        }
    }

    pub(crate) fn record_voice_retry_scheduled(
        &mut self,
        target_chip: Option<u64>,
        next_retry_at: tokio::time::Instant,
    ) {
        if let Some(pending) = self.pending_voice_page.as_mut() {
            pending.last_target_chip = target_chip;
            pending.next_retry_at = next_retry_at;
        }
    }

    pub(crate) fn restore_sms_page(&mut self, pending: PendingPage) {
        self.pending_page = Some(pending);
    }

    pub(crate) fn take_sms_page(&mut self) -> Option<PendingPage> {
        self.pending_page.take()
    }

    pub(crate) fn take_voice_page(&mut self) -> Option<PendingVoicePage> {
        self.pending_voice_page.take()
    }

    pub(crate) fn pending_sms_page_correlation_matches(&self, correlation_id: u32) -> bool {
        self.pending_page
            .as_ref()
            .is_some_and(|pending| pending.page_correlation_id == Some(correlation_id))
    }

    pub(crate) fn pending_voice_page_correlation_matches(&self, correlation_id: u32) -> bool {
        self.pending_voice_page
            .as_ref()
            .is_some_and(|pending| pending.page_correlation_id == Some(correlation_id))
    }

    pub(crate) fn take_sms_page_by_correlation(
        &mut self,
        correlation_id: u32,
    ) -> Option<PendingPage> {
        self.pending_sms_page_correlation_matches(correlation_id)
            .then(|| self.pending_page.take())
            .flatten()
    }

    pub(crate) fn take_voice_page_by_correlation(
        &mut self,
        correlation_id: u32,
    ) -> Option<PendingVoicePage> {
        self.pending_voice_page_correlation_matches(correlation_id)
            .then(|| self.pending_voice_page.take())
            .flatten()
    }

    pub(crate) fn cancel_sms_page(&mut self) {
        self.pending_page = None;
    }

    pub(crate) fn cancel_voice_page(&mut self) {
        self.pending_voice_page = None;
    }

    pub(crate) fn take_timed_out_sms_page(&mut self) -> Option<PendingPage> {
        self.pending_page
            .as_ref()
            .is_some_and(|pending| pending.started_at.elapsed() >= pending.timeout)
            .then(|| self.pending_page.take())
            .flatten()
    }

    pub(crate) fn take_timed_out_voice_page(&mut self) -> Option<PendingVoicePage> {
        self.pending_voice_page
            .as_ref()
            .is_some_and(|pending| pending.started_at.elapsed() >= pending.timeout)
            .then(|| self.pending_voice_page.take())
            .flatten()
    }

    pub(crate) fn prepare_sms_retry(&mut self) -> Option<SmsPageRetry> {
        let pending = self.pending_page.as_mut()?;
        pending.retry_count += 1;
        Some(SmsPageRetry {
            page_address: pending.page_address.clone(),
            fwd_address: pending.fwd_address.clone(),
            pgslot: pending.pgslot,
            slot_cycle_index: pending.slot_cycle_index,
            retry_count: pending.retry_count,
            last_target_chip: pending.last_target_chip,
            page_msg_seq: pending.page_msg_seq,
        })
    }

    pub(crate) fn prepare_voice_retry(&mut self) -> Option<VoicePageRetry> {
        let pending = self.pending_voice_page.as_mut()?;
        pending.retry_count += 1;
        Some(VoicePageRetry {
            page_address: pending.page_address.clone(),
            fwd_address: pending.fwd_address.clone(),
            pgslot: pending.pgslot,
            slot_cycle_index: pending.slot_cycle_index,
            retry_count: pending.retry_count,
            last_target_chip: pending.last_target_chip,
            service_option: pending.service_option,
            page_msg_seq: pending.page_msg_seq,
        })
    }

    pub(crate) fn pending_sms_matches(&self, fwd_address: &MsAddress) -> Option<bool> {
        self.pending_page
            .as_ref()
            .map(|pending| pending.fwd_address == *fwd_address)
    }

    pub(crate) fn take_matching_sms_page_for_access(
        &mut self,
        fwd_address: &MsAddress,
    ) -> Option<PendingPage> {
        self.pending_sms_matches(fwd_address)?
            .then(|| self.take_sms_page())
            .flatten()
    }

    pub(crate) fn take_voice_page_for_a1_call(&mut self, call_id: u64) -> Option<PendingVoicePage> {
        self.pending_voice_page
            .as_ref()
            .is_some_and(|pending| pending.a1_call_id == Some(call_id))
            .then(|| self.pending_voice_page.take())
            .flatten()
    }

    pub(crate) fn take_voice_page_for_a1_call_or_session(
        &mut self,
        call_id: u64,
        session_id: Uuid,
    ) -> Option<PendingVoicePage> {
        self.pending_voice_page
            .as_ref()
            .is_some_and(|pending| {
                pending.a1_call_id == Some(call_id) || pending.session_id == session_id
            })
            .then(|| self.pending_voice_page.take())
            .flatten()
    }
}

impl Bsc {
    pub(crate) fn paging_slot_planner(&self) -> PagingSlotPlanner {
        PagingSlotPlanner::new(self.config.overhead.max_slot_cycle_index)
    }

    pub(crate) async fn drain_pch_transfer_acks(&mut self) {
        let Some(bts_client) = self.config.bts_client.clone() else {
            return;
        };
        for ack in bts_client.drain_pch_transfer_acks() {
            self.handle_pch_transfer_ack(ack).await;
        }
    }

    fn mobile_identity_for_adds_page_ack(&self, addr: &MsAddress) -> cdma_ios::MobileIdentity {
        self.mobiles
            .iter()
            .find(|ms| ms.fwd_address == *addr)
            .and_then(|ms| {
                ms.imsi
                    .as_ref()
                    .map(|imsi| cdma_ios::MobileIdentity::Imsi(imsi.clone()))
                    .or_else(|| ms.esn.map(cdma_ios::MobileIdentity::Esn))
            })
            .unwrap_or_else(|| cdma_ios::MobileIdentity::Imsi("UNKNOWN".to_string()))
    }

    fn send_adds_page_ack_to_msc(
        &self,
        addr: &MsAddress,
        a1_tag: u32,
        cause: Option<u8>,
        context: &'static str,
    ) {
        let client = self.a1.msc_client.clone();
        let mobile_identity = self.mobile_identity_for_adds_page_ack(addr);
        let esn = self
            .mobiles
            .iter()
            .find(|ms| ms.fwd_address == *addr)
            .and_then(|ms| ms.esn);
        tokio::spawn(async move {
            let ack_msg = cdma_ios::AddsPageAckMessage {
                mobile_identity,
                tag: Some(cdma_ios::Tag(a1_tag)),
                mobile_identity_esn: esn.map(cdma_ios::MobileIdentity::Esn),
                cause: cause.map(cdma_ios::Cause),
            };
            match ack_msg.encode() {
                Ok(payload) => {
                    let msg = cdma_ios::EncodedA1Message::from_message(&cdma_ios::Message::new(
                        cdma_ios::MessageType::AddsPageAck,
                        payload,
                    ));
                    if let Err(e) = client.send_a1(msg).await {
                        log::warn!("BSC: failed to send ADDS Page Ack ({context}) to MSC: {e}");
                    }
                }
                Err(e) => log::warn!("BSC: failed to encode ADDS Page Ack ({context}): {e}"),
            }
        });
    }

    pub(crate) async fn handle_pch_transfer_ack(&mut self, ack: PchTransferAckEvent) {
        let Some(correlation_id) = ack.correlation_id else {
            debug!(
                "BSC: ignoring PchMsgTransferAck without correlation_id cause={:?} bts_l2_termination={:?}",
                ack.cause, ack.bts_l2_termination
            );
            return;
        };

        let key = SmsAckKey::PchCorrelation(correlation_id);
        if ack.bts_l2_termination == Some(true) {
            if self
                .paging
                .pending_sms_page_correlation_matches(correlation_id)
                || self
                    .paging
                    .pending_voice_page_correlation_matches(correlation_id)
            {
                debug!(
                    "BSC: page response observed for GPM correlation_id={} - waiting for ACH Page Response",
                    correlation_id
                );
                return;
            }
            match self.sms.complete_delivery(&key) {
                None => debug!(
                    "BSC: PchMsgTransferAck L2 termination for untracked correlation_id={}",
                    correlation_id
                ),
                Some(pending) => {
                    if let Some(a1_tag) = pending.a1_tag {
                        info!(
                            "BSC: sending ADDS Page Ack to MSC tag={} addr={}",
                            a1_tag,
                            format_ms_address(&pending.addr)
                        );
                        self.send_adds_page_ack_to_msc(&pending.addr, a1_tag, None, "success");
                    }
                    if let Err(e) = self.access_tx.send_release_order(&pending.addr, None, None) {
                        warn!(
                            "BSC: failed to send Release Order after SMS delivery ack for {}: {}",
                            format_ms_address(&pending.addr),
                            e
                        );
                    }
                    self.mobiles.mark_registered(&pending.addr);
                    self.publish_mobiles();
                    let addr = pending.addr.clone();
                    self.dispatch_next_queued_sms_for(&addr);
                }
            }
            return;
        }

        // Oversized SMS: set up an SO6 traffic channel and re-deliver on
        // F-DSCH instead of failing.
        if ack.cause == Some(SMS_MESSAGE_TOO_LONG)
            && self.try_escalate_oversized_sms_to_so6(correlation_id).await
        {
            return;
        }

        if let Some(cause) = ack.cause {
            if let Some(pending) = self.paging.take_sms_page_by_correlation(correlation_id) {
                self.clear_pending_page_records_for(&pending.page_address);
                warn!(
                    "BSC: page record delivery failed correlation_id={} cause=0x{:02X}; giving up on SMS to {}",
                    correlation_id,
                    cause,
                    format_ms_address(&pending.fwd_address),
                );
                if let Some(a1_tag) = pending.sms.a1_tag {
                    self.send_adds_page_ack_to_msc(
                        &pending.fwd_address,
                        a1_tag,
                        Some(cause),
                        "page failure",
                    );
                }
                self.mobiles
                    .set_state(&pending.fwd_address, MsState::Registered);
                self.publish_mobiles();
                let addr = pending.fwd_address.clone();
                self.dispatch_next_queued_sms_for(&addr);
                return;
            }
            if let Some(pending) = self.paging.take_voice_page_by_correlation(correlation_id) {
                self.clear_pending_page_records_for(&pending.page_address);
                warn!(
                    "BSC: voice page record delivery failed correlation_id={} cause=0x{:02X}; giving up on {}",
                    correlation_id,
                    cause,
                    format_ms_address(&pending.fwd_address),
                );
                self.mobiles
                    .set_state(&pending.fwd_address, MsState::Registered);
                let caller_addr = self
                    .mobiles
                    .get_by_session_leg(pending.session_id, VoiceLegRole::Caller)
                    .map(|ms| ms.fwd_address.clone());
                if let Some(addr) = caller_addr {
                    self.begin_voice_release(
                        &addr,
                        super::DEFAULT_TRAFFIC_ACK_SEQ,
                        "voice page failure",
                    );
                }
                // A.S0014 cause 0x6E = "Paging response not received".
                if let Some(call_id) = pending.a1_call_id {
                    self.a1.send_clear_request(call_id, 0x6E);
                }
                self.voice
                    .retain_sessions(|session| session.id != pending.session_id);
                self.publish_mobiles();
                return;
            }
            match self.sms.fail_delivery(&key, cause) {
                None => debug!(
                    "BSC: PchMsgTransferAck failure for untracked correlation_id={} cause=0x{:02X}",
                    correlation_id, cause
                ),
                Some(pending) => {
                    if let Some(a1_tag) = pending.a1_tag {
                        self.send_adds_page_ack_to_msc(
                            &pending.addr,
                            a1_tag,
                            Some(cause),
                            "failure",
                        );
                    }
                    self.mobiles.mark_registered(&pending.addr);
                    self.publish_mobiles();
                    let addr = pending.addr.clone();
                    self.dispatch_next_queued_sms_for(&addr);
                }
            }
            return;
        }

        trace!(
            "BSC: PchMsgTransferAck accepted correlation_id={} with no L2 result",
            correlation_id
        );
    }

    pub(crate) fn emit_paging_event(
        &self,
        message: &PagingChannelMessage,
        mcsb: &MessageControlStatusBlock,
    ) {
        let now = chrono::Utc::now();
        let ts_us = now.timestamp_micros() as u64;
        self.events.publish_paging_event(PagingEvent {
            event_id: next_bsc_event_id("paging"),
            message: message.clone(),
            mcsb: mcsb.clone(),
            timestamp_us: ts_us,
        });
    }

    /// Compute the tokio::time::Instant for the next assigned paging slot
    /// that is strictly after `last_target_chip` (if provided). This ensures
    /// each retry targets a distinct slot rather than re-queuing for the same
    /// one. The returned wake time intentionally fires before the slot start so
    /// the GPM can be enqueued with a future requested_tx_time and still land
    /// in the intended slot even if the runtime wakes slightly late.
    pub(crate) fn effective_slot_cycle_index(&self, slot_cycle_index: u8) -> u8 {
        self.paging_slot_planner()
            .effective_slot_cycle_index(slot_cycle_index)
    }

    pub(crate) fn assigned_paging_slot_chip(
        &self,
        search_from: u64,
        pgslot: u16,
        slot_cycle_index: u8,
        chip_rate_hz: u64,
    ) -> u64 {
        self.paging_slot_planner().assigned_paging_slot_chip(
            search_from,
            pgslot,
            slot_cycle_index,
            chip_rate_hz,
        )
    }

    pub(crate) fn compute_next_retry_at(
        &self,
        pgslot: Option<u16>,
        slot_cycle_index: u8,
        last_target_chip: Option<u64>,
    ) -> tokio::time::Instant {
        self.paging_slot_planner()
            .next_retry_at(pgslot, slot_cycle_index, last_target_chip)
    }

    /// Called when the paging timeout timer fires. GPM page-record repeats are
    /// owned by the BTS pending page-record queue, so the BSC only handles
    /// timeout/cancel state here.
    pub(crate) fn handle_page_retry(&mut self) {
        if self.paging.has_pending_voice_page() {
            if let Some(pending) = self.paging.take_timed_out_voice_page() {
                self.clear_pending_page_records_for(&pending.page_address);
                warn!(
                    "BSC: voice page timeout after {} retries ({:.0}ms) for {}",
                    pending.retry_count,
                    pending.started_at.elapsed().as_millis(),
                    format_ms_address(&pending.fwd_address),
                );
                self.mobiles
                    .set_state(&pending.fwd_address, MsState::Registered);
                let caller_addr = self
                    .mobiles
                    .get_by_session_leg(pending.session_id, VoiceLegRole::Caller)
                    .map(|ms| ms.fwd_address.clone());
                if let Some(addr) = caller_addr {
                    self.begin_voice_release(
                        &addr,
                        super::DEFAULT_TRAFFIC_ACK_SEQ,
                        "voice page timeout",
                    );
                }
                // A.S0014 cause 0x6E = "Paging response not received".
                if let Some(call_id) = pending.a1_call_id {
                    self.a1.send_clear_request(call_id, 0x6E);
                }
                self.voice
                    .retain_sessions(|session| session.id != pending.session_id);
                self.publish_mobiles();
                return;
            }
            return;
        }

        if let Some(pending) = self.paging.take_timed_out_sms_page() {
            self.clear_pending_page_records_for(&pending.page_address);
            warn!(
                "BSC: page timeout after {} retries ({:.0}ms) — giving up on SMS to {}",
                pending.retry_count,
                pending.started_at.elapsed().as_millis(),
                format_ms_address(&pending.fwd_address),
            );
            // Reset MS state back to Registered if it still exists
            self.mobiles
                .set_state(&pending.fwd_address, MsState::Registered);
            if let Some(a1_tag) = pending.sms.a1_tag {
                self.send_adds_page_ack_to_msc(
                    &pending.fwd_address,
                    a1_tag,
                    Some(0x07),
                    "page timeout",
                );
            }
            self.publish_mobiles();
        }
    }

    pub(crate) fn send_general_page(
        &self,
        page_addr: &MsPageAddress,
        pgslot: Option<u16>,
        slot_cycle_index: u8,
        after_chip: Option<u64>,
        service_option: Option<u16>,
        purpose: &str,
        override_msg_seq: Option<u8>,
    ) -> Result<(Option<u64>, u8, Option<u32>), Error> {
        let page_seq = override_msg_seq.unwrap_or_else(|| self.paging.next_gpm_page_seq(page_addr));
        // Current overhead for subclass selection at page-send time
        // (C.S0004-E 3.1.2.2.1.1.1.2: BS picks shortest format that
        // uniquely identifies the MS given current overhead).
        let esp = &self
            .config
            .paging
            .message_defaults
            .extended_system_parameters;
        let overhead_mcc = esp.mcc;
        let overhead_imsi_11_12 = esp.imsi_11_12;

        let built_record = build_general_page_record(
            page_addr,
            page_seq,
            service_option,
            overhead_mcc,
            overhead_imsi_11_12,
        );
        match &built_record.log {
            GeneralPageRecordLog::Class0 {
                subclass,
                imsi_s,
                mcc,
                imsi_11_12,
            } => {
                info!(
                    "BSC: page record subclass={} imsi_s=0x{:09X} mcc={:?} imsi_11_12={:?}",
                    subclass, imsi_s, mcc, imsi_11_12,
                );
            }
            GeneralPageRecordLog::Class1 { esn } => {
                info!("BSC: page record class1 esn=0x{:08X}", esn);
            }
        }
        let record = built_record.record;

        info!("BSC: page record for {}: {:?}", purpose, record);

        let page_correlation_id = self.send_gpm_via_abis(page_addr, record, purpose);

        // Compute BSC-local retry scheduling: find the next assigned paging
        // slot so the retry timer can wake up before the next slot boundary.
        // The BTS independently derives slot timing from the IMSI in the record.
        let mut used_target_chip = None;
        if let Some(pg) = pgslot
            && let Some(slot) =
                self.paging_slot_planner()
                    .scheduled_slot(pgslot, slot_cycle_index, after_chip)
        {
            info!(
                "BSC: scheduling page record for PGSLOT={} sci={} effective_sci={} target_chip={} (in ~{}ms)",
                pg,
                slot_cycle_index,
                slot.effective_slot_cycle_index,
                slot.target_chip,
                slot.wait_ms,
            );
            used_target_chip = Some(slot.target_chip);
        }

        Ok((used_target_chip, page_seq, page_correlation_id))
    }

    /// Send a General Page Message for voice call delivery.
    pub(crate) fn send_page_for_voice(
        &self,
        page_addr: &MsPageAddress,
        pgslot: Option<u16>,
        slot_cycle_index: u8,
        after_chip: Option<u64>,
        service_option: u16,
        override_msg_seq: Option<u8>,
    ) -> Result<(Option<u64>, u8, Option<u32>), Error> {
        self.send_general_page(
            page_addr,
            pgslot,
            slot_cycle_index,
            after_chip,
            Some(service_option),
            "voice call delivery",
            override_msg_seq,
        )
    }

    /// Remove pending page records for a specific page address from the BTS supplier queue.
    /// Returns the number of records removed.
    pub(crate) fn clear_pending_page_records_for(&self, page_addr: &MsPageAddress) -> usize {
        if let Some(ref bts_state) = self.config.bts_paging_state {
            bts_state.lock().cancel_pages_for_address(page_addr)
        } else {
            0
        }
    }

    /// Build a GPM containing a single page record and send it to the BTS
    /// via Abis PchMessageTransfer. The BTS decodes the GPM, extracts the
    /// record, and adds it to its paging supplier queue.
    fn send_gpm_via_abis(
        &self,
        page_addr: &MsPageAddress,
        record: GeneralPageRecord,
        purpose: &str,
    ) -> Option<u32> {
        use cdma_abis::control::typed::{
            AirInterfaceMessagePayload, CorrelationId, PchMessageTransferMessage,
        };
        use cdma_common::lac::paging_messages::GeneralPageMessage;

        let gpm = GeneralPageMessage {
            config_msg_seq: self.config.overhead.config_seq,
            acc_msg_seq: self.config.overhead.acc_config_seq,
            class_0_done: true,
            class_1_done: true,
            tmsi_done: true,
            ordered_tmsis: false,
            broadcast_done: true,
            reserved: 0,
            add_pfield: Vec::new(),
            page_records: vec![record],
        };
        let sdu = gpm.to_sdu();
        let sdu_bytes = sdu.to_packed_bytes();
        let wire_msg_type = MessageId::GeneralPage
            .wire_type(cdma_common::lac::message_types::WireChannel::ForwardCommon)
            .unwrap_or(0x11);
        let aim = match AirInterfaceMessagePayload::new(wire_msg_type, sdu_bytes) {
            Ok(aim) => aim,
            Err(e) => {
                warn!(
                    "BSC: failed to build GPM Air Interface Message for {}: {}",
                    purpose, e
                );
                return None;
            }
        };
        let mobile_id = mobile_identity_for_page_address(page_addr);
        let correlation_id = next_pch_correlation_id();
        let pch = PchMessageTransferMessage {
            correlation_id: Some(CorrelationId(correlation_id)),
            mobile_identities: vec![mobile_id],
            cell_identifier_list: None,
            air_interface_message: Some(aim),
            layer2_ack_request_results: None,
            abis_ack_notify: None,
        };
        if let Some(ref bts_client) = self.config.bts_client {
            if let Err(e) = bts_client.send_pch_message(pch) {
                warn!("BSC: send GPM via Abis failed for {}: {}", purpose, e);
                None
            } else {
                info!(
                    "BSC: sent GPM page record via Abis for {} correlation_id={}",
                    purpose, correlation_id
                );
                Some(correlation_id)
            }
        } else {
            warn!("BSC: no bts_client — cannot send GPM for {}", purpose);
            None
        }
    }

    pub(crate) fn send_paging_message(
        &mut self,
        message: PagingChannelMessage,
    ) -> Result<(), Error> {
        debug!(
            "Transmit paging message ({:?}).",
            self.current_message_kind_name(&message)
        );
        let dr = message.to_data_request();
        self.emit_paging_event(&message, &dr.mcsb);
        Ok(())
    }

    pub(crate) fn send_next_default_paging_message(&mut self) -> Result<(), Error> {
        let schedule = &self.config.paging.message_defaults.schedule;
        if schedule.is_empty() {
            return self.send_paging_message(self.build_system_parameters_message());
        }

        // This path has no resolved EV-DO advertisement, so it cannot build a
        // real ATIM (the BTS overhead builder emits one when configured). Skip
        // ATIM slots and advance to the next scheduled message rather than
        // broadcast a duplicate SPM. Bounded by the schedule length so an
        // all-ATIM schedule still falls back to an SPM instead of looping.
        let mut kind = self.paging.next_default_message_kind(schedule);
        for _ in 1..schedule.len() {
            if kind != PagingMessageKind::AlternativeTechnologiesInformation {
                break;
            }
            kind = self.paging.next_default_message_kind(schedule);
        }

        let message = match kind {
            PagingMessageKind::SystemParameters => self.build_system_parameters_message(),
            PagingMessageKind::AccessParameters => self.build_access_parameters_message(),
            PagingMessageKind::NeighborList => self.build_neighbor_list_message(),
            PagingMessageKind::CdmaChannelList => self.build_cdma_channel_list_message(),
            PagingMessageKind::ExtendedSystemParameters => {
                self.build_extended_system_parameters_message()
            }
            PagingMessageKind::GeneralPage => self.build_general_page_message(),
            PagingMessageKind::Order => self.build_order_message(),
            PagingMessageKind::AlternativeTechnologiesInformation => {
                // Reached only for an all-ATIM schedule with no advertisement.
                self.build_system_parameters_message()
            }
        };

        self.send_paging_message(message)
    }

    pub(crate) fn build_system_parameters_message(&self) -> PagingChannelMessage {
        build_scheduled_message(
            PagingMessageKind::SystemParameters,
            self.config.pilot_offset,
            &self.config.overhead,
            &self.config.paging,
            None,
        )
    }

    pub(crate) fn build_access_parameters_message(&self) -> PagingChannelMessage {
        build_scheduled_message(
            PagingMessageKind::AccessParameters,
            self.config.pilot_offset,
            &self.config.overhead,
            &self.config.paging,
            None,
        )
    }

    /// Print a one-time summary of the open-loop reverse TX power
    /// parameters the mobile will see in the Access Parameters Message,
    /// so an operator can sanity-check that the broadcast values
    /// produce a reasonable initial transmit power for their RF setup.
    ///
    /// The mobile's open-loop formula (Band Class 0, IS-95 / IS-2000):
    ///
    /// ```text
    /// Tx_dBm = -Rx_dBm - 73 + NOM_PWR + INIT_PWR + (n-1)*PWR_STEP
    /// ```
    ///
    /// where Rx is the total received power at the mobile (dominated by
    /// our forward pilot at short range), and `n` is the access probe
    /// number (1..NUM_STEP).
    pub(crate) fn log_open_loop_power_init(&self) {
        let d = &self.config.paging.message_defaults.access_parameters;
        let base_offset = i32::from(d.nom_pwr) + i32::from(d.init_pwr);
        let max_ramp = i32::from(d.pwr_step) * i32::from(d.num_step.saturating_sub(1));
        info!(
            "BSC: open-loop reverse TX init: NOM_PWR={} dB INIT_PWR={} dB \
             → Tx = -Rx - {} dBm (probe 1); ramp +{} dB/probe over {} probes \
             (final probe Tx = -Rx - {} dBm)",
            d.nom_pwr,
            d.init_pwr,
            73 - base_offset,
            d.pwr_step,
            d.num_step,
            73 - base_offset - max_ramp,
        );
    }

    pub(crate) fn build_neighbor_list_message(&self) -> PagingChannelMessage {
        build_scheduled_message(
            PagingMessageKind::NeighborList,
            self.config.pilot_offset,
            &self.config.overhead,
            &self.config.paging,
            None,
        )
    }

    pub(crate) fn build_cdma_channel_list_message(&self) -> PagingChannelMessage {
        build_scheduled_message(
            PagingMessageKind::CdmaChannelList,
            self.config.pilot_offset,
            &self.config.overhead,
            &self.config.paging,
            None,
        )
    }

    pub(crate) fn build_extended_system_parameters_message(&self) -> PagingChannelMessage {
        build_scheduled_message(
            PagingMessageKind::ExtendedSystemParameters,
            self.config.pilot_offset,
            &self.config.overhead,
            &self.config.paging,
            None,
        )
    }

    pub(crate) fn build_general_page_message(&self) -> PagingChannelMessage {
        build_scheduled_message(
            PagingMessageKind::GeneralPage,
            self.config.pilot_offset,
            &self.config.overhead,
            &self.config.paging,
            None,
        )
    }

    pub(crate) fn build_order_message(&self) -> PagingChannelMessage {
        build_scheduled_message(
            PagingMessageKind::Order,
            self.config.pilot_offset,
            &self.config.overhead,
            &self.config.paging,
            None,
        )
    }

    pub(crate) fn current_message_kind_name(&self, message: &PagingChannelMessage) -> &'static str {
        match message {
            PagingChannelMessage::SystemParameters(_) => "system_parameters",
            PagingChannelMessage::AccessParameters(_) => "access_parameters",
            PagingChannelMessage::NeighborList(_) => "neighbor_list",
            PagingChannelMessage::CdmaChannelList(_) => "cdma_channel_list",
            PagingChannelMessage::ExtendedSystemParameters(_) => "extended_system_parameters",
            PagingChannelMessage::GeneralPage(_) => "general_page",
            PagingChannelMessage::Order(_) => "order",
            PagingChannelMessage::DataBurst(_) => "data_burst",
            PagingChannelMessage::AuthenticationChallenge(_) => "authentication_challenge",
            PagingChannelMessage::SsdUpdate(_) => "ssd_update",
            PagingChannelMessage::FeatureNotification(_) => "feature_notification",
            PagingChannelMessage::ExtendedNeighborList(_) => "extended_neighbor_list",
            PagingChannelMessage::StatusRequest(_) => "status_request",
            PagingChannelMessage::ServiceRedirection(_) => "service_redirection",
            PagingChannelMessage::GlobalServiceRedirection(_) => "global_service_redirection",
            PagingChannelMessage::TmsiAssignment(_) => "tmsi_assignment",
            PagingChannelMessage::Paca(_) => "paca",
            PagingChannelMessage::GeneralNeighborList(_) => "general_neighbor_list",
            PagingChannelMessage::UserZoneIdentification(_) => "user_zone_identification",
            PagingChannelMessage::PrivateNeighborList(_) => "private_neighbor_list",
            PagingChannelMessage::ExtendedGlobalServiceRedirection(_) => {
                "extended_global_service_redirection"
            }
            PagingChannelMessage::ExtendedCdmaChannelList(_) => "extended_cdma_channel_list",
            PagingChannelMessage::UserZoneReject(_) => "user_zone_reject",
            PagingChannelMessage::Ansi41SystemParameters(_) => "ansi41_system_parameters",
            PagingChannelMessage::McRrParameters(_) => "mc_rr_parameters",
            PagingChannelMessage::Ansi41Rand(_) => "ansi41_rand",
            PagingChannelMessage::EnhancedAccessParameters(_) => "enhanced_access_parameters",
            PagingChannelMessage::UniversalNeighborList(_) => "universal_neighbor_list",
            PagingChannelMessage::SecurityModeCommand(_) => "security_mode_command",
            PagingChannelMessage::UniversalPage(_) => "universal_page",
            PagingChannelMessage::UniversalPageFirstSegment(_) => "universal_page_first_segment",
            PagingChannelMessage::UniversalPageMiddleSegment(_) => "universal_page_middle_segment",
            PagingChannelMessage::UniversalPageFinalSegment(_) => "universal_page_final_segment",
            PagingChannelMessage::AuthenticationRequest(_) => "authentication_request",
            PagingChannelMessage::AlternativeTechnologiesInformation(_) => {
                "alternative_technologies_information"
            }
            PagingChannelMessage::GeneralExtension(_) => "general_extension",
            PagingChannelMessage::GeneralOverheadInformation(_) => "general_overhead_information",
            PagingChannelMessage::AccessPointIdentifier(_) => "access_point_identifier",
            PagingChannelMessage::AccessPointIdentifierText(_) => "access_point_identifier_text",
            PagingChannelMessage::AccessPointPilotInformation(_) => {
                "access_point_pilot_information"
            }
            PagingChannelMessage::FlexDuplexCdmaChannelList(_) => "flex_duplex_cdma_channel_list",
            PagingChannelMessage::BroadcastServiceParameters(_) => "broadcast_service_parameters",
            PagingChannelMessage::ChannelAssignment(_) => "channel_assignment",
            PagingChannelMessage::ExtendedChannelAssignment(_) => "extended_channel_assignment",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cdma_abis::control::typed::MobileIdentity;

    #[test]
    fn directed_abis_identity_for_class0_address_is_full_imsi() {
        let (imsi_m_s1, imsi_m_s2) =
            cdma_common::paging::imsi_s_from_imsi("999990123456789").unwrap();
        let addr = MsAddress::ImsiClass0 {
            imsi_m_s1,
            imsi_m_s2,
            mcc: cdma_common::paging::mcc_from_digits("999").unwrap(),
            imsi_11_12: cdma_common::paging::imsi_11_12_from_digits("99").unwrap(),
        };

        let identity = mobile_identity_for_ms_address(&addr);

        assert_eq!(
            identity,
            MobileIdentity::Imsi("999990123456789".to_string())
        );
    }

    #[test]
    fn directed_abis_identity_for_partial_imsi_s_stays_partial() {
        let (imsi_m_s1, imsi_m_s2) =
            cdma_common::paging::imsi_s_from_imsi("999990123456789").unwrap();
        let addr = MsAddress::ImsiS {
            imsi_m_s1,
            imsi_m_s2,
        };

        let identity = mobile_identity_for_ms_address(&addr);

        assert_eq!(identity, MobileIdentity::Imsi("0123456789".to_string()));
    }

    fn pch_event(
        message_id: MessageId,
        sdu: cdma_common::bits::Bitstream,
    ) -> cdma_bts::bts::paging_supplier::PchTransmitEvent {
        cdma_bts::bts::paging_supplier::PchTransmitEvent {
            message_id,
            address: None,
            msg_seq: 0,
            ack_seq: 0,
            ack_req: false,
            sdu_bytes: sdu.to_packed_bytes(),
            length_bits: sdu.len(),
            overhead_mcc: 0x03ff,
            overhead_imsi_11_12: 0x7f,
        }
    }

    #[test]
    fn pch_reconstruction_returns_error_on_decode_failure() {
        let evt = cdma_bts::bts::paging_supplier::PchTransmitEvent {
            message_id: MessageId::Order,
            address: None,
            msg_seq: 0,
            ack_seq: 0,
            ack_req: false,
            sdu_bytes: Vec::new(),
            length_bits: 0,
            overhead_mcc: 0x03ff,
            overhead_imsi_11_12: 0x7f,
        };

        let err = pch_transmit_event_to_paging_message(&evt).unwrap_err();

        assert!(err.to_string().contains("PCH ORDM body decode failed"));
    }

    #[test]
    fn pch_reconstruction_decodes_authentication_challenge() {
        let original = cdma_common::lac::paging_messages::AuthenticationChallengeMessage {
            randu: 0x0012_3456,
            gen_cmea_key: true,
        };
        let evt = pch_event(MessageId::AuthChallenge, original.to_sdu());

        let decoded = pch_transmit_event_to_paging_message(&evt).unwrap();

        match decoded {
            PagingChannelMessage::AuthenticationChallenge(m) => {
                assert_eq!(m.randu, 0x0012_3456);
                assert!(m.gen_cmea_key);
            }
            _ => panic!("unexpected reconstructed message"),
        }
    }

    #[test]
    fn pch_reconstruction_uses_length_bits_not_packed_padding() {
        let original = cdma_common::lac::paging_messages::StatusRequestMessage {
            qual_info: cdma_common::lac::paging_messages::StatusQualificationInfo::None,
            record_types: Vec::new(),
        };
        let evt = pch_event(MessageId::StatusRequest, original.to_sdu());
        assert_eq!(evt.length_bits, 19);
        assert_eq!(evt.sdu_bytes.len(), 3);

        let decoded = pch_transmit_event_to_paging_message(&evt).unwrap();

        match decoded {
            PagingChannelMessage::StatusRequest(m) => {
                assert_eq!(
                    m.qual_info,
                    cdma_common::lac::paging_messages::StatusQualificationInfo::None
                );
                assert!(m.record_types.is_empty());
            }
            _ => panic!("unexpected reconstructed message"),
        }
    }
}
