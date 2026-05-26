use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use log::{debug, info, trace, warn};
use parking_lot::Mutex;

use crate::lac::message_types::{MessageId, WireChannel};
use crate::lac::paging_messages::{
    GeneralPageMessage, GeneralPageRecord, MsAddress, MsPageAddress, PagingChannelMessage,
    PagingMessageKind,
};
use crate::lac::{DataRequest, Layer2Lac, MessageControlStatusBlock, PagingSupplierFn};
use crate::mac::types::ChannelType;

use super::settings::{OverheadParameters, PagingChannelSettings, build_scheduled_message};

const PENDING_PAGE_RECORD_ASSIGNED_SLOT_ATTEMPTS: u16 = 4;
const PENDING_PAGE_RECORD_FAILURE_GUARD_MS: u64 = 10_000;

/// Outcome of `queue_directed_pch`. `Oversize` means the encapsulated F-PCH
/// capsule would not fit in the 8-bit MSG_LENGTH field.
#[derive(Debug, Clone)]
pub enum DirectedPchQueueError {
    InvalidIdentity,
    Oversize(String),
}

/// Event emitted when the BTS paging supplier transmits a directed SDU
/// or GPM on F-PCH. Carries the real on-air MSG_SEQ assigned by the BTS.
#[derive(Clone, Debug)]
pub struct PchTransmitEvent {
    pub message_id: MessageId,
    pub address: Option<MsAddress>,
    pub msg_seq: u8,
    pub ack_seq: u8,
    pub ack_req: bool,
    pub sdu_bytes: Vec<u8>,
    pub length_bits: usize,
    pub overhead_mcc: u16,
    pub overhead_imsi_11_12: u8,
}

/// A page record waiting to be folded into the next supplier-generated GPM.
#[derive(Clone, Debug)]
pub struct PendingPageRecord {
    pub record: GeneralPageRecord,
    pub page_address: MsPageAddress,
    pub remaining_assigned_slot_attempts: u16,
    pub correlation_id: Option<u32>,
    exhausted_at_chip: Option<u64>,
}

impl PendingPageRecord {
    pub fn new(record: GeneralPageRecord, page_address: MsPageAddress) -> Self {
        Self::new_with_correlation(record, page_address, None)
    }

    pub fn new_with_correlation(
        record: GeneralPageRecord,
        page_address: MsPageAddress,
        correlation_id: Option<u32>,
    ) -> Self {
        Self {
            record,
            page_address,
            remaining_assigned_slot_attempts: PENDING_PAGE_RECORD_ASSIGNED_SLOT_ATTEMPTS,
            correlation_id,
            exhausted_at_chip: None,
        }
    }
}

/// Configuration for BTS-side paging channel retransmission.
#[derive(Clone, Debug)]
pub struct PagingRetryConfig {
    /// How long to wait for an MS ACK before reporting Abis L2 failure.
    pub ack_timeout_ms: u64,
    /// Maximum number of slot-aligned OTA retransmissions.
    pub max_retries: u32,
}

impl Default for PagingRetryConfig {
    fn default() -> Self {
        Self {
            ack_timeout_ms: 1000,
            max_retries: 3,
        }
    }
}

/// A directed SDU awaiting MS acknowledgement, with retry tracking.
#[derive(Clone, Debug)]
struct PendingDirectedRetry {
    data_request: DataRequest,
    correlation_id: Option<u32>,
    pgslot: Option<u16>,
    first_tx_chip: Option<u64>,
    retry_count: u32,
}

/// Events produced by the paging retry tick.
#[derive(Debug)]
pub enum PagingRetryEvent {
    /// The MS acknowledged the message. Contains the correlation_id.
    Acknowledged { correlation_id: u32 },
    /// All retries exhausted without MS ACK. Contains the correlation_id
    /// so the BTS can send a PchMessageTransferAck with a cause code.
    Failed { correlation_id: u32 },
}

/// Shared state for the BTS-local paging supplier.
pub struct PagingSupplierState {
    pub pending_page_records: Vec<PendingPageRecord>,
    /// Directed SDUs received via Abis PchMessageTransfer that should be
    /// transmitted on the next available paging channel frame.
    pub pending_directed_sdus: VecDeque<DataRequest>,
    /// Per-mobile ARQ MSG_SEQ counters, keyed by (address_tracking_key, ack_req).
    /// Separate sequences for assured vs non-assured per C.S0004-E §3.1.2.1.1.2.
    msg_seq_counters: HashMap<(u8, Vec<u8>, bool), u8>,
    /// Pending Abis ack-notify requests: maps (addr_type, addr_bytes, msg_seq) →
    /// correlation_id. When the MS ACKs with ack_seq matching msg_seq for the
    /// address, the BTS sends a PchMessageTransferAck with bts_l2_termination=true.
    pending_ack_notifies: HashMap<(u8, Vec<u8>, u8), u32>,
    /// Directed SDUs awaiting MS ACK, keyed by (addr_type, addr_bytes, msg_seq).
    pending_retries: HashMap<(u8, Vec<u8>, u8), PendingDirectedRetry>,
    /// Retry configuration (timeouts, max attempts).
    retry_config: PagingRetryConfig,
    /// Current overhead MCC for IMSI class-0 OTA compression.
    overhead_mcc: u16,
    /// Current overhead IMSI_11_12 for IMSI class-0 OTA compression.
    overhead_imsi_11_12: u8,
    /// Last received ARQ msg_seq per mobile, keyed by ack_identity_key.
    /// Updated on every access probe; stamped as valid_ack + ack_seq on
    /// the next directed SDU sent to that mobile.
    last_received_msg_seq: HashMap<(u8, Vec<u8>), u8>,
    /// Buffered retry events (failures) produced by slot-aligned retry checks,
    /// drained by `drain_retry_events()`.
    pending_retry_events: Vec<PagingRetryEvent>,
    /// Broadcast sender for PCH transmit events.
    pch_transmit_tx: Option<tokio::sync::broadcast::Sender<PchTransmitEvent>>,
}

impl PagingSupplierState {
    pub fn new(overhead_mcc: u16, overhead_imsi_11_12: u8) -> Self {
        Self::new_with_retry_config(
            PagingRetryConfig::default(),
            overhead_mcc,
            overhead_imsi_11_12,
        )
    }

    /// Creates a new state with explicit retry configuration.
    pub fn new_with_retry_config(
        retry_config: PagingRetryConfig,
        overhead_mcc: u16,
        overhead_imsi_11_12: u8,
    ) -> Self {
        Self {
            pending_page_records: Vec::new(),
            pending_directed_sdus: VecDeque::new(),
            msg_seq_counters: HashMap::new(),
            pending_ack_notifies: HashMap::new(),
            pending_retries: HashMap::new(),
            retry_config,
            overhead_mcc,
            overhead_imsi_11_12,
            last_received_msg_seq: HashMap::new(),
            pending_retry_events: Vec::new(),
            pch_transmit_tx: None,
        }
    }

    /// Install a broadcast sender for PCH transmit events.
    pub fn set_pch_transmit_tx(&mut self, tx: tokio::sync::broadcast::Sender<PchTransmitEvent>) {
        self.pch_transmit_tx = Some(tx);
    }

    /// Emit a PCH transmit event for a directed SDU.
    fn emit_pch_transmit(&self, dr: &DataRequest) {
        if let Some(ref tx) = self.pch_transmit_tx {
            let sdu_bytes = dr.sdu.to_packed_bytes();
            let _ = tx.send(PchTransmitEvent {
                message_id: dr.mcsb.message_id,
                address: dr.mcsb.address.clone(),
                msg_seq: dr.mcsb.msg_seq,
                ack_seq: dr.mcsb.ack_seq,
                ack_req: dr.mcsb.ack_req,
                sdu_bytes,
                length_bits: dr.mcsb.length_bits,
                overhead_mcc: self.overhead_mcc,
                overhead_imsi_11_12: self.overhead_imsi_11_12,
            });
        }
    }

    /// Remove pending page records for a specific page address.
    /// Returns the number of records removed.
    pub fn cancel_pages_for_address(&mut self, addr: &MsPageAddress) -> usize {
        let before = self.pending_page_records.len();
        self.pending_page_records
            .retain(|p| p.page_address != *addr);
        before - self.pending_page_records.len()
    }

    /// Complete pending page records for a mobile that sent a Page Response.
    /// Returns Abis correlation IDs that need final positive PCH transfer acks.
    pub fn complete_pages_for_address(&mut self, addr: &MsPageAddress) -> Vec<u32> {
        let mut correlations = Vec::new();
        self.pending_page_records.retain(|pending| {
            if pending.page_address == *addr {
                if let Some(correlation_id) = pending.correlation_id {
                    correlations.push(correlation_id);
                }
                false
            } else {
                true
            }
        });
        correlations
    }

    /// Record the ARQ msg_seq from an incoming access probe so that the
    /// next directed SDU to this mobile carries valid_ack=true + ack_seq.
    pub fn record_received_msg_seq(&mut self, addr: &MsAddress, msg_seq: u8) {
        let key = addr.ack_identity_key(self.overhead_mcc, self.overhead_imsi_11_12);
        debug!(
            "BTS paging supplier: recorded last_received_msg_seq={} for addr={:?}",
            msg_seq, addr
        );
        self.last_received_msg_seq.insert(key, msg_seq);
    }

    fn next_msg_seq(&mut self, addr: &MsAddress, ack_req: bool) -> u8 {
        let (addr_type, addr_bytes) = addr.tracking_key();
        let key = (addr_type, addr_bytes, ack_req);
        let entry = self.msg_seq_counters.entry(key).or_insert(0);
        let seq = *entry;
        *entry = (seq + 1) % 8;
        seq
    }

    /// Queue a directed SDU from a PchMessageTransfer for air-interface
    /// transmission. Converts MobileIdentity to MsAddress, assigns ARQ
    /// fields, and builds the DataRequest with proper addressing.
    ///
    /// When `ack_notify` is true and `correlation_id` is provided, the BTS
    /// tracks the assigned msg_seq so that when the MS ACKs, a second
    /// PchMessageTransferAck with `bts_l2_termination=true` can be sent.
    pub fn queue_directed_pch(
        &mut self,
        mobile_identity: &cdma_abis::control::typed::MobileIdentity,
        air_interface_message: &cdma_abis::control::typed::AirInterfaceMessagePayload,
        ack_req: bool,
        correlation_id: Option<u32>,
        ack_notify: bool,
    ) -> Result<(), DirectedPchQueueError> {
        let addr = match mobile_identity_to_ms_address(mobile_identity) {
            Some(a) => a,
            None => {
                warn!(
                    "BTS paging supplier: cannot convert MobileIdentity to MsAddress, dropping SDU"
                );
                return Err(DirectedPchQueueError::InvalidIdentity);
            }
        };
        let sdu = cdma_common::bits::Bitstream::new_bytes(&air_interface_message.message);
        let message_id = MessageId::from_wire(
            WireChannel::ForwardCommon,
            air_interface_message.message_type,
        )
        .unwrap_or(MessageId::ExtChannelAssignment);

        // Ensure the encapsulated PDU fits before mutating msg_seq / retry
        // state. ARQ widths are fixed, so probe seq/ack values don't affect
        // the capsule size.
        let probe_dr = DataRequest {
            sdu: sdu.clone(),
            mcsb: MessageControlStatusBlock {
                channel: ChannelType::FPch,
                length_bits: sdu.len(),
                mobile_p_rev: None,
                extended_encryption: false,
                message_id,
                requested_tx_time: None,
                tx_deadline: None,
                address: Some(addr.clone()),
                ack_seq: 0,
                msg_seq: 0,
                ack_req,
                valid_ack: false,
                overhead_mcc: self.overhead_mcc,
                overhead_imsi_11_12: self.overhead_imsi_11_12,
            },
        };
        if let Err(e) = Layer2Lac::assemble_pdu(probe_dr) {
            warn!(
                "BTS paging supplier: directed PCH would overflow MSG_LENGTH, dropping (corr={:?} sdu_bytes={}): {}",
                correlation_id,
                air_interface_message.message.len(),
                e,
            );
            return Err(DirectedPchQueueError::Oversize(e.to_string()));
        }

        let msg_seq = self.next_msg_seq(&addr, ack_req);
        let (ack_addr_type, ack_addr_bytes) =
            addr.ack_identity_key(self.overhead_mcc, self.overhead_imsi_11_12);
        let ack_key = (ack_addr_type, ack_addr_bytes.clone());
        let (valid_ack, ack_seq) = match self.last_received_msg_seq.get(&ack_key) {
            Some(&seq) => (true, seq),
            None => (false, 0),
        };

        if ack_notify {
            if let Some(corr_id) = correlation_id {
                self.pending_ack_notifies
                    .insert((ack_addr_type, ack_addr_bytes.clone(), msg_seq), corr_id);
                info!(
                    "BTS paging supplier: ack-notify registered msg_seq={} correlation_id={}",
                    msg_seq, corr_id
                );
            }
        }

        let dr = DataRequest {
            sdu: sdu.clone(),
            mcsb: MessageControlStatusBlock {
                channel: ChannelType::FPch,
                length_bits: sdu.len(),
                mobile_p_rev: None,
                extended_encryption: false,
                message_id,
                requested_tx_time: None,
                tx_deadline: None,
                address: Some(addr),
                ack_seq,
                msg_seq,
                ack_req,
                valid_ack,
                overhead_mcc: self.overhead_mcc,
                overhead_imsi_11_12: self.overhead_imsi_11_12,
            },
        };
        info!(
            "BTS paging supplier: queuing directed SDU msg_id={:?} wire_type=0x{:02X} msg_seq={} ack_seq={} ack_req={} valid_ack={} addr={:?} sdu_bytes={:02X?} ({} bits)",
            message_id,
            air_interface_message.message_type,
            msg_seq,
            dr.mcsb.ack_seq,
            ack_req,
            dr.mcsb.valid_ack,
            dr.mcsb.address,
            &air_interface_message.message,
            dr.mcsb.length_bits,
        );

        if ack_req {
            let pgslot = dr.mcsb.address.as_ref().and_then(pgslot_from_address);
            self.pending_retries.insert(
                (ack_addr_type, ack_addr_bytes.clone(), msg_seq),
                PendingDirectedRetry {
                    data_request: dr.clone(),
                    correlation_id,
                    pgslot,
                    first_tx_chip: None,
                    retry_count: 0,
                },
            );
        }

        self.pending_directed_sdus.push_back(dr);
        Ok(())
    }

    /// Check if an MS ACK matches a pending ack-notify request.
    /// Returns the correlation_id if found and removes the entry.
    /// Also cancels the pending retry for that address/msg_seq.
    pub fn check_ack_notify(&mut self, addr: &MsAddress, ack_seq: u8) -> Option<u32> {
        let (addr_type, addr_bytes) =
            addr.ack_identity_key(self.overhead_mcc, self.overhead_imsi_11_12);
        let key = (addr_type, addr_bytes.clone(), ack_seq);
        let removed = self.pending_retries.remove(&key);
        if removed.is_some() {
            debug!(
                "BTS paging supplier: cancelled retry for ack_seq={} (MS ACK received)",
                ack_seq
            );
        }
        self.pending_ack_notifies.remove(&key)
    }

    /// Check pending retries against the current paging slot. Re-queues
    /// directed SDUs whose pgslot matches, and buffers failure events when
    /// ack_timeout_ms expires. Called from the paging supplier closure on
    /// each new slot boundary.
    pub fn check_slot_retries(&mut self, chip_cursor: u64, max_sci: u8, chip_rate_hz: u64) {
        let max_retries = self.retry_config.max_retries;
        let timeout_chips = self
            .retry_config
            .ack_timeout_ms
            .saturating_mul(chip_rate_hz)
            / 1000;
        let mut expired_keys = Vec::new();
        let mut retransmit = Vec::new();

        for (key, pending) in &self.pending_retries {
            if let Some(first_tx_chip) = pending.first_tx_chip {
                if chip_cursor.saturating_sub(first_tx_chip) >= timeout_chips {
                    if let Some(corr_id) = pending.correlation_id {
                        info!(
                            "BTS paging supplier: ack timeout ({} ms) expired for correlation_id={}",
                            self.retry_config.ack_timeout_ms, corr_id
                        );
                        self.pending_retry_events.push(PagingRetryEvent::Failed {
                            correlation_id: corr_id,
                        });
                    }
                    expired_keys.push(key.clone());
                    continue;
                }
            }

            let slot_match = match pending.pgslot {
                Some(pg) => {
                    cdma_common::paging::is_assigned_slot(chip_cursor, pg, max_sci, chip_rate_hz)
                }
                None => true,
            };
            if !slot_match {
                continue;
            }
            if self.pending_directed_sdu_has_retry_key(key) {
                trace!(
                    "BTS paging supplier: retry deferred for msg_seq={} because TX is still queued",
                    key.2
                );
                continue;
            }
            if pending.retry_count < max_retries {
                retransmit.push(key.clone());
            }
        }

        for key in &expired_keys {
            self.pending_retries.remove(key);
            self.pending_ack_notifies.remove(key);
        }

        for key in retransmit {
            if let Some(pending) = self.pending_retries.get_mut(&key) {
                pending.retry_count += 1;
                let dr = pending.data_request.clone();
                info!(
                    "BTS paging supplier: slot-aligned retransmit #{} msg_seq={} addr={:?} pgslot={:?}",
                    pending.retry_count, key.2, dr.mcsb.address, pending.pgslot
                );
                self.pending_directed_sdus.push_back(dr);
            }
        }
    }

    pub fn check_page_record_failures(&mut self, chip_cursor: u64, chip_rate_hz: u64) {
        let guard_chips = PENDING_PAGE_RECORD_FAILURE_GUARD_MS.saturating_mul(chip_rate_hz) / 1000;
        let mut failed_correlations = Vec::new();
        self.pending_page_records.retain(|pending| {
            let Some(exhausted_at_chip) = pending.exhausted_at_chip else {
                return true;
            };
            if chip_cursor.saturating_sub(exhausted_at_chip) < guard_chips {
                return true;
            }
            if let Some(correlation_id) = pending.correlation_id {
                info!(
                    "BTS paging supplier: page record attempts exhausted for correlation_id={}",
                    correlation_id
                );
                failed_correlations.push(correlation_id);
            }
            false
        });
        self.pending_retry_events.extend(
            failed_correlations
                .into_iter()
                .map(|correlation_id| PagingRetryEvent::Failed { correlation_id }),
        );
    }

    /// Drain buffered retry events (failures produced by `check_slot_retries`).
    pub fn drain_retry_events(&mut self) -> Vec<PagingRetryEvent> {
        std::mem::take(&mut self.pending_retry_events)
    }

    fn record_directed_tx(&mut self, dr: &DataRequest, chip_cursor: u64) {
        if !dr.mcsb.ack_req {
            return;
        }
        let Some(addr) = dr.mcsb.address.as_ref() else {
            return;
        };
        let (addr_type, addr_bytes) =
            addr.ack_identity_key(self.overhead_mcc, self.overhead_imsi_11_12);
        let key = (addr_type, addr_bytes, dr.mcsb.msg_seq);
        if let Some(pending) = self.pending_retries.get_mut(&key) {
            pending.first_tx_chip.get_or_insert(chip_cursor);
        }
    }

    fn pending_directed_sdu_has_retry_key(&self, key: &(u8, Vec<u8>, u8)) -> bool {
        self.pending_directed_sdus.iter().any(|dr| {
            let Some(addr) = dr.mcsb.address.as_ref() else {
                return false;
            };
            let (addr_type, addr_bytes) =
                addr.ack_identity_key(self.overhead_mcc, self.overhead_imsi_11_12);
            (addr_type, addr_bytes, dr.mcsb.msg_seq) == *key
        })
    }
}

/// Compute the MS's paging slot from its address, if possible.
fn pgslot_from_address(addr: &MsAddress) -> Option<u16> {
    match addr {
        MsAddress::ImsiS {
            imsi_m_s1,
            imsi_m_s2,
        }
        | MsAddress::ImsiClass0 {
            imsi_m_s1,
            imsi_m_s2,
            ..
        } => Some(cdma_common::paging::compute_pgslot(*imsi_m_s1, *imsi_m_s2)),
        MsAddress::Esn(_) => None,
    }
}

fn pending_page_record_active_in_slot(
    pending: &PendingPageRecord,
    chip_cursor: u64,
    max_sci: u8,
    chip_rate_hz: u64,
) -> bool {
    match &pending.page_address {
        MsPageAddress::ImsiS {
            imsi_m_s1,
            imsi_m_s2,
            ..
        } => {
            let pgslot = cdma_common::paging::compute_pgslot(*imsi_m_s1, *imsi_m_s2);
            cdma_common::paging::is_assigned_slot(chip_cursor, pgslot, max_sci, chip_rate_hz)
        }
        MsPageAddress::Esn(_) => true,
    }
}

/// Convert an Abis MobileIdentity to an air-interface MsAddress.
///
/// For ESN: direct mapping. For IMSI: derive IMSI_M_S1/S2 from the IMSI
/// string per C.S0005-E 2.3.1.1.
fn mobile_identity_to_ms_address(
    identity: &cdma_abis::control::typed::MobileIdentity,
) -> Option<MsAddress> {
    match identity {
        cdma_abis::control::typed::MobileIdentity::Esn(esn) => Some(MsAddress::Esn(*esn)),
        cdma_abis::control::typed::MobileIdentity::Imsi(imsi_str) => {
            let (imsi_m_s1, imsi_m_s2) = cdma_common::paging::imsi_s_from_imsi(imsi_str)?;
            let digits: String = imsi_str.chars().filter(|c| c.is_ascii_digit()).collect();
            if digits.len() == 15 {
                let mcc = cdma_common::paging::mcc_from_digits(&digits[0..3])?;
                let imsi_11_12 = cdma_common::paging::imsi_11_12_from_digits(&digits[3..5])?;
                Some(MsAddress::ImsiClass0 {
                    imsi_m_s1,
                    imsi_m_s2,
                    mcc,
                    imsi_11_12,
                })
            } else {
                Some(MsAddress::ImsiS {
                    imsi_m_s1,
                    imsi_m_s2,
                })
            }
        }
    }
}

/// Build a `PagingSupplierFn` closure that generates the overhead train
/// and GPMs locally on the BTS, with no BSC involvement.
///
/// The returned closure follows the slot-aware pattern:
/// - First message in each 80 ms paging slot is always a GPM (with any
///   pending page records folded in)
/// - Remaining frames are filled with overhead rotation (SPM, APM, NLM, etc.)
pub fn build_bts_paging_supplier(
    overhead: OverheadParameters,
    paging: PagingChannelSettings,
    pilot_offset: usize,
    state: Arc<Mutex<PagingSupplierState>>,
) -> PagingSupplierFn {
    let chip_rate_hz = 1_228_800u64; // SR1

    let overhead_schedule: Vec<PagingMessageKind> = paging
        .message_defaults
        .schedule
        .iter()
        .copied()
        .filter(|k| *k != PagingMessageKind::GeneralPage)
        .collect();

    let mut current_slot_num: Option<u16> = None;
    let mut gpm_sent_this_slot = false;
    let mut overhead_index = 0usize;

    Box::new(move |chip_cursor: u64| {
        let slot_num = cdma_common::paging::slot_num_from_chips(chip_cursor, chip_rate_hz);

        if current_slot_num != Some(slot_num) {
            current_slot_num = Some(slot_num);
            gpm_sent_this_slot = false;
            {
                let mut guard = state.lock();
                guard.check_page_record_failures(chip_cursor, chip_rate_hz);
                guard.check_slot_retries(chip_cursor, overhead.max_slot_cycle_index, chip_rate_hz);
            }
        }

        if !gpm_sent_this_slot {
            gpm_sent_this_slot = true;

            let mut records_for_slot = Vec::new();
            {
                let mut guard = state.lock();
                let max_sci = overhead.max_slot_cycle_index;
                guard.pending_page_records.retain_mut(|pending| {
                    if pending.exhausted_at_chip.is_some() {
                        return true;
                    }
                    let include = pending_page_record_active_in_slot(
                        pending,
                        chip_cursor,
                        max_sci,
                        chip_rate_hz,
                    );
                    if include {
                        if !records_for_slot
                            .iter()
                            .any(|r: &GeneralPageRecord| *r == pending.record)
                        {
                            records_for_slot.push(pending.record.clone());
                        }
                        pending.remaining_assigned_slot_attempts =
                            pending.remaining_assigned_slot_attempts.saturating_sub(1);
                        if pending.remaining_assigned_slot_attempts > 0 {
                            true
                        } else if pending.correlation_id.is_some() {
                            pending.exhausted_at_chip = Some(chip_cursor);
                            true
                        } else {
                            false
                        }
                    } else {
                        true
                    }
                });
            }

            let (has_remaining_class0, has_remaining_class1, has_remaining_tmsi) = {
                let guard = state.lock();
                let max_sci = overhead.max_slot_cycle_index;
                (
                    guard
                        .pending_page_records
                        .iter()
                        .filter(|pending| pending.exhausted_at_chip.is_none())
                        .filter(|pending| {
                            pending_page_record_active_in_slot(
                                pending,
                                chip_cursor,
                                max_sci,
                                chip_rate_hz,
                            )
                        })
                        .filter(|pending| !records_for_slot.contains(&pending.record))
                        .any(|p| matches!(p.record, GeneralPageRecord::Class0 { .. })),
                    guard
                        .pending_page_records
                        .iter()
                        .filter(|pending| pending.exhausted_at_chip.is_none())
                        .filter(|pending| {
                            pending_page_record_active_in_slot(
                                pending,
                                chip_cursor,
                                max_sci,
                                chip_rate_hz,
                            )
                        })
                        .filter(|pending| !records_for_slot.contains(&pending.record))
                        .any(|p| matches!(p.record, GeneralPageRecord::Class1 { .. })),
                    guard
                        .pending_page_records
                        .iter()
                        .filter(|pending| pending.exhausted_at_chip.is_none())
                        .filter(|pending| {
                            pending_page_record_active_in_slot(
                                pending,
                                chip_cursor,
                                max_sci,
                                chip_rate_hz,
                            )
                        })
                        .filter(|pending| !records_for_slot.contains(&pending.record))
                        .any(|p| matches!(p.record, GeneralPageRecord::Tmsi { .. })),
                )
            };

            let defaults = &paging.message_defaults.general_page;
            let message = PagingChannelMessage::GeneralPage(GeneralPageMessage {
                config_msg_seq: overhead.config_seq,
                acc_msg_seq: overhead.acc_config_seq,
                class_0_done: defaults.class_0_done && !has_remaining_class0,
                class_1_done: defaults.class_1_done && !has_remaining_class1,
                tmsi_done: defaults.tmsi_done && !has_remaining_tmsi,
                ordered_tmsis: defaults.ordered_tmsis,
                broadcast_done: defaults.broadcast_done,
                reserved: defaults.reserved,
                add_pfield: defaults.add_pfield.clone(),
                page_records: records_for_slot,
            });

            let num_records = match &message {
                PagingChannelMessage::GeneralPage(gpm) => gpm.page_records.len(),
                _ => 0,
            };
            let dr = message.to_data_request();
            if num_records > 0 {
                info!(
                    "BTS paging supplier: GPM TX with {} page record(s) (slot_num={}, chip={})",
                    num_records, slot_num, chip_cursor
                );
                let guard = state.lock();
                guard.emit_pch_transmit(&dr);
            } else {
                trace!(
                    "BTS paging supplier: GPM (empty, slot_num={}, chip={})",
                    slot_num, chip_cursor
                );
            }
            return Some(dr);
        }

        {
            let mut guard = state.lock();
            if let Some(dr) = guard.pending_directed_sdus.pop_front() {
                let sdu_hex = dr.sdu.to_packed_bytes();
                info!(
                    "BTS paging supplier: directed SDU TX msg_id={:?} addr={:?} ack_seq={} msg_seq={} ack_req={} valid_ack={} sdu={:02X?} ({} bits) slot_num={}",
                    dr.mcsb.message_id,
                    dr.mcsb.address,
                    dr.mcsb.ack_seq,
                    dr.mcsb.msg_seq,
                    dr.mcsb.ack_req,
                    dr.mcsb.valid_ack,
                    sdu_hex,
                    dr.mcsb.length_bits,
                    slot_num,
                );
                guard.record_directed_tx(&dr, chip_cursor);
                guard.emit_pch_transmit(&dr);
                return Some(dr);
            }
        }

        if overhead_schedule.is_empty() {
            return None;
        }
        let kind = overhead_schedule[overhead_index % overhead_schedule.len()];
        overhead_index += 1;
        let message = build_scheduled_message(kind, pilot_offset, &overhead, &paging);
        trace!(
            "BTS paging supplier: {:?} (overhead_index={}, slot_num={})",
            kind, overhead_index, slot_num
        );
        Some(message.to_data_request())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use cdma_abis::control::typed::{AirInterfaceMessagePayload, MobileIdentity};
    use cdma_common::consts::SR1_CHIP_RATE_HZ;

    #[test]
    fn directed_pch_queue_is_fifo() {
        let mut state = PagingSupplierState::new(0x03ff, 0x7f);
        let identity = MobileIdentity::Esn(0x1234_5678);
        let wire_type = MessageId::ExtChannelAssignment
            .wire_type(WireChannel::ForwardCommon)
            .unwrap();
        let first = AirInterfaceMessagePayload::new(wire_type, [0xAA]).unwrap();
        let second = AirInterfaceMessagePayload::new(wire_type, [0xBB]).unwrap();

        state
            .queue_directed_pch(&identity, &first, true, Some(1), true)
            .unwrap();
        state
            .queue_directed_pch(&identity, &second, true, Some(2), true)
            .unwrap();

        let first_tx = state.pending_directed_sdus.pop_front().unwrap();
        let second_tx = state.pending_directed_sdus.pop_front().unwrap();
        assert_eq!(first_tx.mcsb.msg_seq, 0);
        assert_eq!(second_tx.mcsb.msg_seq, 1);
    }

    /// When an access probe has been received (msg_seq recorded), the next
    /// directed SDU queued for that mobile must carry valid_ack=true and
    /// ack_seq matching the probe's msg_seq.
    #[test]
    fn queue_directed_pch_stamps_ack_from_last_access_probe() {
        let overhead_mcc = cdma_common::paging::mcc_from_digits("999").unwrap();
        let overhead_imsi_11_12 = cdma_common::paging::imsi_11_12_from_digits("99").unwrap();
        let mut state = PagingSupplierState::new(overhead_mcc, overhead_imsi_11_12);

        // Simulate an access probe with msg_seq=5 from an ImsiClass0 address.
        let (imsi_m_s1, imsi_m_s2) =
            cdma_common::paging::imsi_s_from_imsi("999990123456789").unwrap();
        let access_addr = MsAddress::ImsiClass0 {
            imsi_m_s1,
            imsi_m_s2,
            mcc: overhead_mcc,
            imsi_11_12: overhead_imsi_11_12,
        };
        state.record_received_msg_seq(&access_addr, 5);

        // Queue a directed SDU via full Abis IMSI identity.
        let identity = MobileIdentity::Imsi("999990123456789".to_string());
        let wire_type = MessageId::Order
            .wire_type(WireChannel::ForwardCommon)
            .unwrap();
        let payload = AirInterfaceMessagePayload::new(wire_type, [0xCC]).unwrap();
        state
            .queue_directed_pch(&identity, &payload, true, Some(10), true)
            .unwrap();

        let dr = state.pending_directed_sdus.pop_front().unwrap();
        assert!(
            dr.mcsb.valid_ack,
            "directed SDU must have valid_ack=true after access probe"
        );
        assert_eq!(
            dr.mcsb.ack_seq, 5,
            "directed SDU ack_seq must match last access probe msg_seq"
        );
    }

    /// Without any prior access probe, directed SDUs default to
    /// valid_ack=false, ack_seq=0.
    #[test]
    fn queue_directed_pch_defaults_no_ack_without_prior_probe() {
        let mut state = PagingSupplierState::new(0x03ff, 0x7f);
        let identity = MobileIdentity::Esn(0xDEAD_BEEF);
        let wire_type = MessageId::ExtChannelAssignment
            .wire_type(WireChannel::ForwardCommon)
            .unwrap();
        let payload = AirInterfaceMessagePayload::new(wire_type, [0xAA]).unwrap();
        state
            .queue_directed_pch(&identity, &payload, true, Some(1), false)
            .unwrap();

        let dr = state.pending_directed_sdus.pop_front().unwrap();
        assert!(!dr.mcsb.valid_ack);
        assert_eq!(dr.mcsb.ack_seq, 0);
    }

    /// Oversized SDU on F-PCH must be rejected before any state mutation so
    /// the abis_agent can ack with SMS_MESSAGE_TOO_LONG.
    #[test]
    fn queue_directed_pch_rejects_oversize_without_mutating_state() {
        let mut state = PagingSupplierState::new(0x03ff, 0x7f);
        let identity = MobileIdentity::Esn(0xDEAD_BEEF);
        let wire_type = MessageId::DataBurst
            .wire_type(WireChannel::ForwardCommon)
            .unwrap();
        // ~250 SDU bytes blows past the 255-octet MSG_LENGTH cap once
        // wrap overhead (MSG_LENGTH + MSG_TYPE + ARQ + address + CRC30) is
        // added.
        let big = vec![0x5Au8; 250];
        let payload = AirInterfaceMessagePayload::new(wire_type, big).unwrap();
        let result = state.queue_directed_pch(&identity, &payload, true, Some(1), false);
        assert!(matches!(result, Err(DirectedPchQueueError::Oversize(_))));
        assert!(state.pending_directed_sdus.is_empty());
        assert!(state.pending_retries.is_empty());
        assert!(state.pending_ack_notifies.is_empty());
    }

    #[test]
    fn exhausted_correlated_page_record_emits_failure_after_guard() {
        let mut state = PagingSupplierState::new(0x03ff, 0x7f);
        let mut record = PendingPageRecord::new_with_correlation(
            GeneralPageRecord::Class1 {
                esn: 0x1234_5678,
                msg_seq: 0,
                special_service: false,
                service_option: None,
            },
            MsPageAddress::Esn(0x1234_5678),
            Some(99),
        );
        record.remaining_assigned_slot_attempts = 0;
        record.exhausted_at_chip = Some(0);
        state.pending_page_records.push(record);

        let guard_chips =
            PENDING_PAGE_RECORD_FAILURE_GUARD_MS.saturating_mul(SR1_CHIP_RATE_HZ) / 1000;

        state.check_page_record_failures(guard_chips - 1, SR1_CHIP_RATE_HZ);
        assert_eq!(state.pending_page_records.len(), 1);
        assert!(state.drain_retry_events().is_empty());

        state.check_page_record_failures(guard_chips, SR1_CHIP_RATE_HZ);
        assert!(state.pending_page_records.is_empty());
        assert!(matches!(
            state.drain_retry_events().as_slice(),
            [PagingRetryEvent::Failed { correlation_id: 99 }]
        ));
    }

    /// The ECAM is queued via Abis with a partial IMSI_S identity, which the
    /// paging supplier stores as MsAddress::ImsiS.
    /// The MS ACKs on the access channel with the full IMSI class-0 identity
    /// (MCC + IMSI_11_12 + S2 + S1).  check_ack_notify must match across
    /// these two address forms — otherwise the retry is never cancelled.
    #[test]
    fn check_ack_notify_matches_imsi_class0_against_imsi_s() {
        let overhead_mcc = cdma_common::paging::mcc_from_digits("999").unwrap();
        let overhead_imsi_11_12 = cdma_common::paging::imsi_11_12_from_digits("99").unwrap();
        let mut state = PagingSupplierState::new(overhead_mcc, overhead_imsi_11_12);

        // Queue an ECAM via partial Abis IMSI_S identity — stored internally as ImsiS.
        let identity = MobileIdentity::Imsi("0123456789".to_string());
        let wire_type = MessageId::ExtChannelAssignment
            .wire_type(WireChannel::ForwardCommon)
            .unwrap();
        let ecam_payload = AirInterfaceMessagePayload::new(wire_type, [0xAA]).unwrap();
        state
            .queue_directed_pch(&identity, &ecam_payload, true, Some(42), true)
            .unwrap();

        // The ECAM got msg_seq=0.
        assert_eq!(state.pending_retries.len(), 1);
        assert_eq!(state.pending_ack_notifies.len(), 1);

        // MS ACKs on the access channel with the full ImsiClass0 address.
        let (imsi_m_s1, imsi_m_s2) =
            cdma_common::paging::imsi_s_from_imsi("999990123456789").unwrap();
        let access_addr = MsAddress::ImsiClass0 {
            imsi_m_s1,
            imsi_m_s2,
            mcc: overhead_mcc,
            imsi_11_12: overhead_imsi_11_12,
        };

        let corr = state.check_ack_notify(&access_addr, 0);

        // Must find and cancel the pending retry.
        assert_eq!(
            corr,
            Some(42),
            "check_ack_notify must match ImsiClass0 against stored ImsiS"
        );
        assert_eq!(state.pending_retries.len(), 0);
        assert_eq!(state.pending_ack_notifies.len(), 0);
    }

    /// A full IMSI from Abis must be stored as the full mobile identity for
    /// ACK matching, not collapsed to IMSI_S plus current overhead values.
    #[test]
    fn check_ack_notify_uses_full_imsi_from_abis_identity() {
        let overhead_mcc = cdma_common::paging::mcc_from_digits("310").unwrap();
        let overhead_imsi_11_12 = cdma_common::paging::imsi_11_12_from_digits("26").unwrap();
        let actual_mcc = cdma_common::paging::mcc_from_digits("999").unwrap();
        let actual_imsi_11_12 = cdma_common::paging::imsi_11_12_from_digits("99").unwrap();
        let mut state = PagingSupplierState::new(overhead_mcc, overhead_imsi_11_12);

        let identity = MobileIdentity::Imsi("999990123456789".to_string());
        let wire_type = MessageId::Order
            .wire_type(WireChannel::ForwardCommon)
            .unwrap();
        let payload = AirInterfaceMessagePayload::new(wire_type, [0x6C, 0x00]).unwrap();
        state
            .queue_directed_pch(&identity, &payload, true, Some(77), true)
            .unwrap();

        assert_eq!(state.pending_retries.len(), 1);
        assert_eq!(state.pending_ack_notifies.len(), 1);

        let (imsi_m_s1, imsi_m_s2) =
            cdma_common::paging::imsi_s_from_imsi("999990123456789").unwrap();
        let access_addr = MsAddress::ImsiClass0 {
            imsi_m_s1,
            imsi_m_s2,
            mcc: actual_mcc,
            imsi_11_12: actual_imsi_11_12,
        };

        let corr = state.check_ack_notify(&access_addr, 0);

        assert_eq!(corr, Some(77));
        assert_eq!(state.pending_retries.len(), 0);
        assert_eq!(state.pending_ack_notifies.len(), 0);
    }

    /// Slot-aligned retries only fire when the current slot matches the MS's
    /// pgslot, and stop after max_retries.
    #[test]
    fn check_slot_retries_fires_on_assigned_pgslot() {
        let chip_rate_hz = SR1_CHIP_RATE_HZ;
        let chips_per_slot: u64 = chip_rate_hz / 50 * 4; // 80ms slot
        let max_sci: u8 = 0; // T=1, cycle=16 slots

        let mut state = PagingSupplierState::new_with_retry_config(
            PagingRetryConfig {
                ack_timeout_ms: 10_000,
                max_retries: 2,
            },
            0x03ff,
            0x7f,
        );

        let identity = MobileIdentity::Imsi("999990123456789".to_string());
        let wire_type = MessageId::Order
            .wire_type(WireChannel::ForwardCommon)
            .unwrap();
        let payload = AirInterfaceMessagePayload::new(wire_type, [0xDD]).unwrap();
        state
            .queue_directed_pch(&identity, &payload, true, Some(99), true)
            .unwrap();

        // Drain the initial TX and mark it as transmitted.
        assert_eq!(state.pending_directed_sdus.len(), 1);
        let initial_dr = state.pending_directed_sdus.pop_front().unwrap();

        // Find the pgslot for this IMSI.
        let (s1, s2) = cdma_common::paging::imsi_s_from_imsi("999990123456789").unwrap();
        let pgslot = cdma_common::paging::compute_pgslot(s1, s2);

        // Tick at a non-matching slot — no retransmit.
        let wrong_slot = (pgslot as u64 + 1) % 2048;
        state.check_slot_retries(wrong_slot * chips_per_slot, max_sci, chip_rate_hz);
        assert_eq!(
            state.pending_directed_sdus.len(),
            0,
            "no retry on wrong slot"
        );

        // Tick at the matching pgslot — retry #1.
        let matching_chip = pgslot as u64 * chips_per_slot;
        state.record_directed_tx(&initial_dr, matching_chip.saturating_sub(chips_per_slot));
        state.check_slot_retries(matching_chip, max_sci, chip_rate_hz);
        assert_eq!(
            state.pending_directed_sdus.len(),
            1,
            "retry #1 on matching slot"
        );
        state.pending_directed_sdus.clear();

        // Next matching slot (16 slots later) — retry #2.
        let next_matching = (pgslot as u64 + 16) * chips_per_slot;
        state.check_slot_retries(next_matching, max_sci, chip_rate_hz);
        assert_eq!(
            state.pending_directed_sdus.len(),
            1,
            "retry #2 on matching slot"
        );
        state.pending_directed_sdus.clear();

        // Next matching slot — max_retries=2 exhausted, but Abis failure waits
        // for ack_timeout_ms.
        let third_matching = (pgslot as u64 + 32) * chips_per_slot;
        state.check_slot_retries(third_matching, max_sci, chip_rate_hz);
        assert_eq!(
            state.pending_directed_sdus.len(),
            0,
            "no more retries after max"
        );

        let events = state.drain_retry_events();
        assert_eq!(events.len(), 0);
        assert_eq!(state.pending_retries.len(), 1, "retry entry remains");

        let timeout_chip =
            matching_chip.saturating_sub(chips_per_slot) + (10_000 * chip_rate_hz / 1000) + 1;
        state.check_slot_retries(timeout_chip, max_sci, chip_rate_hz);
        let events = state.drain_retry_events();
        assert_eq!(events.len(), 1);
        assert!(
            matches!(events[0], PagingRetryEvent::Failed { correlation_id: 99 }),
            "expected failure event with correlation_id=99"
        );
        assert_eq!(state.pending_retries.len(), 0, "retry entry cleaned up");
    }

    #[test]
    fn check_slot_retries_does_not_duplicate_initial_pending_tx() {
        let chip_rate_hz = SR1_CHIP_RATE_HZ;
        let chips_per_slot: u64 = chip_rate_hz / 50 * 4;
        let max_sci: u8 = 0;

        let mut state = PagingSupplierState::new_with_retry_config(
            PagingRetryConfig {
                ack_timeout_ms: 10_000,
                max_retries: 2,
            },
            0x03ff,
            0x7f,
        );

        let identity = MobileIdentity::Imsi("999990123456789".to_string());
        let wire_type = MessageId::Order
            .wire_type(WireChannel::ForwardCommon)
            .unwrap();
        let payload = AirInterfaceMessagePayload::new(wire_type, [0xDD]).unwrap();
        state
            .queue_directed_pch(&identity, &payload, true, Some(99), true)
            .unwrap();

        let (s1, s2) = cdma_common::paging::imsi_s_from_imsi("999990123456789").unwrap();
        let pgslot = cdma_common::paging::compute_pgslot(s1, s2);
        let matching_chip = pgslot as u64 * chips_per_slot;

        state.check_slot_retries(matching_chip, max_sci, chip_rate_hz);
        assert_eq!(
            state.pending_directed_sdus.len(),
            1,
            "retry scheduler must not enqueue a duplicate before the initial TX drains"
        );

        state.pending_directed_sdus.pop_front().unwrap();

        let next_matching = (pgslot as u64 + 16) * chips_per_slot;
        state.check_slot_retries(next_matching, max_sci, chip_rate_hz);
        assert_eq!(
            state.pending_directed_sdus.len(),
            1,
            "first retry should be queued on the next assigned slot after initial TX"
        );
    }

    #[test]
    fn zero_max_retries_does_not_fail_on_first_retry_slot() {
        let chip_rate_hz = SR1_CHIP_RATE_HZ;
        let chips_per_slot: u64 = chip_rate_hz / 50 * 4;
        let max_sci: u8 = 0;

        let mut state = PagingSupplierState::new_with_retry_config(
            PagingRetryConfig {
                ack_timeout_ms: 2_000,
                max_retries: 0,
            },
            0x03ff,
            0x7f,
        );

        let identity = MobileIdentity::Imsi("999990123456789".to_string());
        let wire_type = MessageId::Order
            .wire_type(WireChannel::ForwardCommon)
            .unwrap();
        let payload = AirInterfaceMessagePayload::new(wire_type, [0xDD]).unwrap();
        state
            .queue_directed_pch(&identity, &payload, true, Some(99), true)
            .unwrap();

        assert_eq!(state.pending_directed_sdus.len(), 1);
        let initial_dr = state.pending_directed_sdus.pop_front().unwrap();

        let (s1, s2) = cdma_common::paging::imsi_s_from_imsi("999990123456789").unwrap();
        let pgslot = cdma_common::paging::compute_pgslot(s1, s2);
        let matching_chip = pgslot as u64 * chips_per_slot;
        let first_tx_chip = matching_chip.saturating_sub(chips_per_slot);
        state.record_directed_tx(&initial_dr, first_tx_chip);

        state.check_slot_retries(matching_chip, max_sci, chip_rate_hz);

        assert_eq!(state.pending_directed_sdus.len(), 0);
        assert!(
            state.drain_retry_events().is_empty(),
            "no-retry mode must wait for ack_timeout_ms before failing to Abis"
        );
        assert_eq!(
            state.pending_retries.len(),
            1,
            "pending retry must remain available for a later MS ACK"
        );

        let timeout_chip = first_tx_chip + (2_000 * chip_rate_hz / 1000) + 1;
        state.check_slot_retries(timeout_chip, max_sci, chip_rate_hz);
        let events = state.drain_retry_events();
        assert_eq!(events.len(), 1);
        assert!(
            matches!(events[0], PagingRetryEvent::Failed { correlation_id: 99 }),
            "ack_timeout_ms should fail to Abis after ACK window expires"
        );
        assert_eq!(state.pending_retries.len(), 0);
        assert_eq!(state.pending_ack_notifies.len(), 0);
    }
}
