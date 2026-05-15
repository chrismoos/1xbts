pub mod message_types;
pub mod paging_messages;

use std::{
    collections::{HashMap, VecDeque},
    sync::{Arc, mpsc},
    time::{Duration, Instant},
};

use parking_lot::Mutex;

pub use cdma_common::lac::{DataRequest, MessageControlStatusBlock, Sdu};

use cdma_common::{
    bits::Bitstream,
    error::Error,
    time::{self, CdmaSystemTime},
};
use log::{debug, error, info, trace, warn};
use message_types::MessageId;
use paging_messages::{MsAddress, MsPageAddress};

use crate::{
    mac::{
        self,
        types::{AvailabilityIndication, ChannelType, MacMessage},
    },
    phy::coding::long_code::LongCodeGenerator,
    receiver::sync::SyncChannelMessage,
};

use cdma_common::consts::{SR1_CHIP_RATE_HZ, SR1_CHIPS_320MS, SR1_CHIPS_PER_80MS};
const SYNC_FRAMES_PER_SUPERFRAME: usize = 3;
const PAGING_CHANNEL_NUMBER_SR1: u8 = 1;
const FPCH_HALF_FRAME_BITS_9600: usize = 96;
const FPCH_HALF_FRAME_CHIPS: u64 = SR1_CHIP_RATE_HZ / 100;
const FPCH_HALF_FRAME_PAYLOAD_BITS_9600: usize = FPCH_HALF_FRAME_BITS_9600 - 1;
const FPCH_SLOT_CHIPS: u64 = FPCH_HALF_FRAME_CHIPS * 8;

pub struct LinkAccessControl {}

#[derive(Debug)]
pub enum LacMessage {
    DataRequest(DataRequest),
    SyncChannelTemplate(SyncChannelMessage),
    SupervisionRequest(MessageControlStatusBlock),
    DataConfirm(MessageControlStatusBlock),
    DataIndication(Sdu, MessageControlStatusBlock),
    ConditionNotification(MessageControlStatusBlock),
}

impl LinkAccessControl {}

#[derive(Debug)]
pub enum ForwardChannel {
    Sync,
    Broadcast,
    GeneralSignaling,
}

#[derive(Debug)]
pub struct EncapsulatedPdu {
    pub message: DataRequest,
    pub e_pdu: Bitstream,
    pub frame_start_sent: bool,
}

impl EncapsulatedPdu {
    pub fn get_fragment(&mut self, max_size: usize) -> Bitstream {
        let mut bs = Bitstream::new();

        if self.message.mcsb.channel != ChannelType::FSync
            && self.message.mcsb.channel != ChannelType::FPch
            && self.message.mcsb.channel != ChannelType::FTch
        {
            panic!(
                "pdu fragmentation not supported for {:?}",
                self.message.mcsb.channel
            );
        }

        if !self.frame_start_sent {
            if self.message.mcsb.channel == ChannelType::FSync {
                trace!("sync channel whole message bits={}", self.e_pdu.len());

                // add extra padding to round up to a superframe
                let max_frame_data_bits = 31;
                let superframe_bits = max_frame_data_bits * 3;

                if self.e_pdu.len() % superframe_bits != 0 {
                    trace!(
                        "sync channel pad {}",
                        superframe_bits - (self.e_pdu.len() % superframe_bits)
                    );
                    self.e_pdu
                        .write_u8(0, superframe_bits - (self.e_pdu.len() % superframe_bits));
                }
            }
        }

        // SOM bit for sync channel, SCI bit for paging channel
        bs.write_u8(if self.frame_start_sent { 0 } else { 1 }, 1);
        self.frame_start_sent = true;

        let sendable_size = max_size - 1;

        if self.e_pdu.len() >= sendable_size {
            let fragment = self.e_pdu.drain(0..sendable_size);
            bs.extend(&fragment);
            if self.message.mcsb.channel == ChannelType::FSync {
                trace!("SYNC FULL BLOCK: {}", bs);
            }
        } else {
            let padding = sendable_size - self.e_pdu.len();
            bs.extend(&self.e_pdu.drain(0..self.e_pdu.len()));
            if self.message.mcsb.channel == ChannelType::FSync {
                trace!("SYNC PARTIAL BLOCK: {}", bs);
            }

            for _ in 0..padding {
                bs.write_u8(0, 1);
            }
        }

        bs
    }
}

/// T4m retransmit window per C.S0004-E Annex A: 2.2 seconds.
pub const T4M_RETRANSMIT_WINDOW: Duration = Duration::from_millis(2_200);

#[derive(Debug, Clone, PartialEq)]
pub struct PendingDirectedPduKey {
    pub addr: MsAddress,
    pub message_id: MessageId,
    pub ack_seq: u8,
    pub ack_req: bool,
    pub valid_ack: bool,
    pub sdu_bits: Vec<u8>,
}

#[derive(Debug, Clone)]
pub struct PendingDirectedPdu {
    pub key: PendingDirectedPduKey,
    pub msg_seq: u8,
    pub first_tx_at: Option<Instant>,
    pub last_tx_at: Option<Instant>,
}

#[derive(Debug, Default)]
pub struct PendingDirectedPduTracker {
    pub entries: Vec<PendingDirectedPdu>,
}

impl PendingDirectedPduTracker {
    pub fn retain_unexpired(&mut self) {
        let now = Instant::now();
        self.entries.retain(|entry| {
            entry
                .first_tx_at
                .is_none_or(|first_tx_at| now.duration_since(first_tx_at) <= T4M_RETRANSMIT_WINDOW)
        });
    }

    pub fn reserve_msg_seq(
        &mut self,
        tracker: &mut MsgSeqTracker,
        addr: &MsAddress,
        message_id: MessageId,
        sdu: &Bitstream,
        ack_seq: u8,
        ack_req: bool,
        valid_ack: bool,
    ) -> Result<u8, Error> {
        self.retain_unexpired();
        let key = PendingDirectedPduKey {
            addr: addr.clone(),
            message_id,
            ack_seq,
            ack_req,
            valid_ack,
            sdu_bits: sdu.bits().to_vec(),
        };
        if let Some(existing) = self.entries.iter_mut().find(|entry| entry.key == key) {
            return Ok(existing.msg_seq);
        }

        let mut pending_seqs = [false; 8];
        for entry in &self.entries {
            if entry.key.addr == *addr && entry.key.ack_req == ack_req {
                pending_seqs[entry.msg_seq as usize] = true;
            }
        }

        let msg_seq =
            tracker.next_seq_excluding(addr, ack_req, |seq| pending_seqs[seq as usize])?;
        self.entries.push(PendingDirectedPdu {
            key,
            msg_seq,
            first_tx_at: None,
            last_tx_at: None,
        });
        Ok(msg_seq)
    }

    pub fn mark_transmitted(&mut self, addr: &MsAddress, ack_req: bool, msg_seq: u8) {
        self.mark_transmitted_at(addr, ack_req, msg_seq, Instant::now());
    }

    fn mark_transmitted_at(
        &mut self,
        addr: &MsAddress,
        ack_req: bool,
        msg_seq: u8,
        transmitted_at: Instant,
    ) {
        if let Some(entry) = self.entries.iter_mut().find(|entry| {
            entry.key.addr == *addr && entry.key.ack_req == ack_req && entry.msg_seq == msg_seq
        }) {
            if entry.first_tx_at.is_none() {
                entry.first_tx_at = Some(transmitted_at);
            }
            entry.last_tx_at = Some(transmitted_at);
        }
    }

    pub fn acknowledge(&mut self, addr: &MsAddress, msg_seq: u8) -> bool {
        self.retain_unexpired();
        let before = self.entries.len();
        self.entries
            .retain(|entry| !(entry.key.addr == *addr && entry.msg_seq == msg_seq));
        self.entries.len() != before
    }
}

/// Request to send a directed SDU through the LAC layer with ARQ.
pub struct DirectedSduRequest {
    pub sdu: Bitstream,
    pub channel: ChannelType,
    pub address: MsAddress,
    pub message_id: MessageId,
    pub ack_seq: u8,
    pub ack_req: bool,
    pub valid_ack: bool,
    pub requested_tx_time: Option<CdmaSystemTime>,
    pub tx_deadline: Option<CdmaSystemTime>,
    /// Overhead MCC for IMSI class-0 OTA address compression.
    pub overhead_mcc: u16,
    /// Overhead IMSI_11_12 for IMSI class-0 OTA address compression.
    pub overhead_imsi_11_12: u8,
}

/// Handle returned from `send_directed_sdu` with the assigned ARQ parameters.
pub struct ArqHandle {
    pub msg_seq: u8,
    pub mcsb: MessageControlStatusBlock,
}

struct State {
    pub message_queue: HashMap<ChannelType, VecDeque<EncapsulatedPdu>>,
    pub sync_template: Option<SyncChannelMessage>,
    pub paging_channel_number: u8,
    pub cancel_future_general_pages: bool,
    pub fpch_queue_checked_out: bool,
    /// Per-mobile GPM page record msg_seq tracking.
    pub gpm_page_seq: HashMap<MsPageAddress, u8>,
    /// MSG_SEQ tracker for paging channel directed PDUs.
    pub paging_msg_seq_tracker: MsgSeqTracker,
    /// Pending directed PDU tracker for deduplication and T4m enforcement.
    pub pending_directed_pdus: PendingDirectedPduTracker,
}

/// Paging supplier callback.  The `u64` argument is the current chip cursor
/// (absolute chips since CDMA epoch) so the supplier can implement slot-aware
/// scheduling.
pub type PagingSupplierFn = Box<dyn FnMut(u64) -> Option<DataRequest> + Send>;

pub type Layer2LacRef = Arc<Layer2Lac>;

pub struct Layer2Lac {
    state: Mutex<State>,
    lac_to_mac_tx: mpsc::Sender<MacMessage>,
    mac_to_lac_rx: Mutex<mpsc::Receiver<MacMessage>>,
    paging_supplier: Mutex<Option<PagingSupplierFn>>,
}

impl Layer2Lac {
    pub fn new(
        lac_to_mac_tx: mpsc::Sender<MacMessage>,
        mac_to_lac_rx: mpsc::Receiver<MacMessage>,
    ) -> Layer2LacRef {
        Arc::new(Layer2Lac {
            state: Mutex::new(State {
                message_queue: HashMap::new(),
                sync_template: None,
                paging_channel_number: PAGING_CHANNEL_NUMBER_SR1,
                cancel_future_general_pages: false,
                fpch_queue_checked_out: false,
                gpm_page_seq: HashMap::new(),
                paging_msg_seq_tracker: MsgSeqTracker::new(ChannelType::FPch),
                pending_directed_pdus: PendingDirectedPduTracker::default(),
            }),
            lac_to_mac_tx,
            mac_to_lac_rx: Mutex::new(mac_to_lac_rx),
            paging_supplier: Mutex::new(None),
        })
    }

    pub fn start(&self) -> Result<(), Error> {
        debug!("Starting LAC layer listener...");
        let rx = self.mac_to_lac_rx.lock();
        loop {
            let msg = rx.recv()?;
            trace!("MAC -> LAC: {:?}", msg);

            match msg {
                MacMessage::AvailabilityIndication(availability_indication) => {
                    self.handle_availability_indication(availability_indication)?;
                }
                _ => {
                    error!("Unsupported message: {:?}", msg);
                }
            }
        }
    }

    pub fn run_for(&self, max_messages: usize, recv_timeout: Duration) -> Result<usize, Error> {
        debug!("Starting bounded LAC listener...");
        let rx = self.mac_to_lac_rx.lock();
        let mut processed = 0usize;

        while processed < max_messages {
            let msg = match rx.recv_timeout(recv_timeout) {
                Ok(msg) => msg,
                Err(mpsc::RecvTimeoutError::Timeout) => break,
                Err(mpsc::RecvTimeoutError::Disconnected) => break,
            };
            trace!("MAC -> LAC: {:?}", msg);

            match msg {
                MacMessage::AvailabilityIndication(availability_indication) => {
                    self.handle_availability_indication(availability_indication)?;
                    processed += 1;
                }
                _ => {
                    error!("Unsupported message: {:?}", msg);
                    processed += 1;
                }
            }
        }

        Ok(processed)
    }

    fn next_paging_data_request(&self, chip_cursor: u64) -> Option<DataRequest> {
        self.paging_supplier
            .lock()
            .as_mut()
            .and_then(|f| f(chip_cursor))
    }

    fn is_empty_general_page_request(request: &DataRequest) -> bool {
        request.mcsb.channel == ChannelType::FPch
            && request.mcsb.message_id == MessageId::GeneralPage
            && request.mcsb.length_bits <= 24
    }

    fn next_paging_pdu_for_fragment(
        &self,
        chip_cursor: u64,
    ) -> Result<Option<EncapsulatedPdu>, Error> {
        loop {
            let Some(data_request) = self.next_paging_data_request(chip_cursor) else {
                return Ok(None);
            };
            if !Self::is_fpch_slot_start(chip_cursor)
                && Self::is_empty_general_page_request(&data_request)
            {
                trace!(
                    "lac_fpch_skip_empty_gpm_inside_slot: chip={} slot_num={}",
                    chip_cursor,
                    cdma_common::paging::slot_num_from_chips(chip_cursor, 1_228_800),
                );
                continue;
            }
            return Ok(Some(Self::assemble_pdu(data_request)?));
        }
    }

    fn is_fpch_slot_start(chip_cursor: u64) -> bool {
        chip_cursor.is_multiple_of(FPCH_SLOT_CHIPS)
    }

    fn is_fpch_general_page_pdu(pdu: &EncapsulatedPdu) -> bool {
        pdu.message.mcsb.channel == ChannelType::FPch
            && pdu.message.mcsb.message_id == MessageId::GeneralPage
    }

    fn is_unaddressed_fpch_overhead_pdu(pdu: &EncapsulatedPdu) -> bool {
        pdu.message.mcsb.channel == ChannelType::FPch
            && pdu.message.mcsb.address.is_none()
            && pdu.message.mcsb.message_id != MessageId::GeneralPage
    }

    fn fpch_payload_bits_until_slot_end(
        chip_cursor: u64,
        current_half_frame_payload_bits: usize,
    ) -> usize {
        let slot_offset_chips = chip_cursor % FPCH_SLOT_CHIPS;
        let half_frame_idx = (slot_offset_chips / FPCH_HALF_FRAME_CHIPS).min(7) as usize;
        let later_half_frames = 7usize.saturating_sub(half_frame_idx);
        current_half_frame_payload_bits
            + later_half_frames.saturating_mul(FPCH_HALF_FRAME_PAYLOAD_BITS_9600)
    }

    fn unstarted_pdu_fits_before_fpch_slot_end(
        pdu: &EncapsulatedPdu,
        chip_cursor: u64,
        current_half_frame_payload_bits: usize,
    ) -> bool {
        pdu.e_pdu.len()
            <= Self::fpch_payload_bits_until_slot_end(chip_cursor, current_half_frame_payload_bits)
    }

    fn inject_slot_first_gpm(
        &self,
        queue: &mut VecDeque<EncapsulatedPdu>,
        _current_system_time: CdmaSystemTime,
        chip_cursor: u64,
    ) -> Result<(), Error> {
        if !Self::is_fpch_slot_start(chip_cursor) {
            return Ok(());
        }

        if let Some(front) = queue.front() {
            if front.frame_start_sent {
                warn!(
                    "lac_fpch_slot_first_gpm_conflict: chip={} slot_num={} front_tag={} front_started=true",
                    chip_cursor,
                    cdma_common::paging::slot_num_from_chips(chip_cursor, 1_228_800),
                    front.message.mcsb.message_id.tag(),
                );
                return Ok(());
            }
            if Self::is_fpch_general_page_pdu(front) {
                return Ok(());
            }
        }

        let Some(data_request) = self.next_paging_data_request(chip_cursor) else {
            return Ok(());
        };
        let pdu = Self::assemble_pdu(data_request)?;
        let tag = pdu.message.mcsb.message_id.tag();
        if Self::is_fpch_general_page_pdu(&pdu) {
            queue.push_front(pdu);
        } else {
            warn!(
                "lac_fpch_slot_first_gpm_missing: chip={} slot_num={} supplier_tag={}",
                chip_cursor,
                cdma_common::paging::slot_num_from_chips(chip_cursor, 1_228_800),
                tag,
            );
            queue.push_front(pdu);
        }
        Ok(())
    }

    fn has_slot_first_gpm_at_front(queue: &VecDeque<EncapsulatedPdu>, chip_cursor: u64) -> bool {
        Self::is_fpch_slot_start(chip_cursor)
            && queue
                .front()
                .is_some_and(|pdu| !pdu.frame_start_sent && Self::is_fpch_general_page_pdu(pdu))
    }

    fn fpch_queue_candidate_key(
        pdu: &EncapsulatedPdu,
        current_system_time: CdmaSystemTime,
    ) -> (u8, u8, Option<CdmaSystemTime>, u8) {
        let sendable_now = pdu
            .message
            .mcsb
            .requested_tx_time
            .map(|ts| ts <= current_system_time)
            .unwrap_or(true);
        let has_deadline = pdu.message.mcsb.tx_deadline.is_some();
        let is_directed = pdu.message.mcsb.address.is_some();
        (
            if sendable_now { 0 } else { 1 },
            if has_deadline { 0 } else { 1 },
            pdu.message.mcsb.tx_deadline,
            if is_directed { 0 } else { 1 },
        )
    }

    fn reprioritize_fpch_queue(
        queue: &mut VecDeque<EncapsulatedPdu>,
        current_system_time: CdmaSystemTime,
    ) {
        if queue.len() < 2 || queue.front().is_some_and(|pdu| pdu.frame_start_sent) {
            return;
        }

        let mut best_idx = 0usize;
        let mut best_key = Self::fpch_queue_candidate_key(&queue[0], current_system_time);
        for (idx, pdu) in queue.iter().enumerate().skip(1) {
            let key = Self::fpch_queue_candidate_key(pdu, current_system_time);
            if key < best_key {
                best_idx = idx;
                best_key = key;
            }
        }

        if best_idx != 0
            && let Some(pdu) = queue.remove(best_idx)
        {
            queue.push_front(pdu);
        }
    }

    fn build_paging_fragment_request(
        &self,
        queue: &mut VecDeque<EncapsulatedPdu>,
        max_size: usize,
        current_system_time: CdmaSystemTime,
        chip_cursor: u64,
    ) -> Result<Option<mac::types::DataRequest>, Error> {
        assert!(max_size > 1);

        let mut fragment = Bitstream::new();
        let mut remaining_payload_bits = max_size - 1;
        let mut first_mcsb = None;

        self.inject_slot_first_gpm(queue, current_system_time, chip_cursor)?;

        loop {
            self.apply_pending_future_general_page_cancellation(queue);
            if !Self::has_slot_first_gpm_at_front(queue, chip_cursor) {
                Self::reprioritize_fpch_queue(queue, current_system_time);
            }
            if queue.is_empty() {
                let Some(pdu) = self.next_paging_pdu_for_fragment(chip_cursor)? else {
                    break;
                };
                queue.push_back(pdu);
                if !Self::has_slot_first_gpm_at_front(queue, chip_cursor) {
                    Self::reprioritize_fpch_queue(queue, current_system_time);
                }
            }

            let front = queue.front_mut().unwrap();

            // Don't transmit a PDU before its requested_tx_time — the MS may
            // be in slotted mode and only listening during its assigned slot.
            // If the front PDU isn't sendable yet, try the supplier for
            // overhead filler so we don't starve the paging channel.
            if !front.frame_start_sent {
                if let Some(tx_time) = front.message.mcsb.requested_tx_time {
                    if tx_time > current_system_time {
                        if let Some(pdu) = self.next_paging_pdu_for_fragment(chip_cursor)? {
                            queue.push_back(pdu);
                            Self::reprioritize_fpch_queue(queue, current_system_time);
                            continue;
                        }
                        break;
                    }
                }
            }

            let frame_bits_before_pdu = fragment.len();
            if !front.frame_start_sent
                && Self::is_unaddressed_fpch_overhead_pdu(front)
                && !Self::unstarted_pdu_fits_before_fpch_slot_end(
                    front,
                    chip_cursor,
                    remaining_payload_bits,
                )
            {
                debug!(
                    "lac_fpch_defer_overhead_for_slot_gpm: chip={} slot_num={} tag={} epdu_bits={} payload_until_slot_end={}",
                    chip_cursor,
                    cdma_common::paging::slot_num_from_chips(chip_cursor, 1_228_800),
                    front.message.mcsb.message_id.tag(),
                    front.e_pdu.len(),
                    Self::fpch_payload_bits_until_slot_end(chip_cursor, remaining_payload_bits),
                );
                break;
            }
            if fragment.len() == 0 {
                fragment.write_u8(if front.frame_start_sent { 0 } else { 1 }, 1);
                first_mcsb = Some(front.message.mcsb.clone());
            }

            // Log GPMs with page records when their PDU actually starts
            // entering an F-PCH frame. A GPM may be packed after another PDU,
            // so this cannot be limited to the first PDU in the frame.
            if !front.frame_start_sent
                && front.message.mcsb.message_id == MessageId::GeneralPage
                && front.message.mcsb.length_bits > 24
            {
                let slot_num = cdma_common::paging::slot_num_from_chips(chip_cursor, 1_228_800);
                let requested_tx_chip = front
                    .message
                    .mcsb
                    .requested_tx_time
                    .map(|ts| time::chips_since_epoch(ts, 1_228_800));
                info!(
                    "lac_fpch_gpm_start: tx_chip={} slot_num={} sdu_bits={} epdu_bits_remaining={} frame_bits_before_pdu={} packed_after_prior_pdu={} req_chip={:?}",
                    chip_cursor,
                    slot_num,
                    front.message.mcsb.length_bits,
                    front.e_pdu.len(),
                    frame_bits_before_pdu,
                    frame_bits_before_pdu > 0,
                    requested_tx_chip,
                );
            }

            let pdu_start_mcsb = (!front.frame_start_sent).then(|| front.message.mcsb.clone());
            if let Some(mcsb) = pdu_start_mcsb.as_ref() {
                self.mark_directed_pdu_transmitted(mcsb)?;
            }
            front.frame_start_sent = true;

            let bits_to_take = remaining_payload_bits.min(front.e_pdu.len());
            let payload = front.e_pdu.drain(0..bits_to_take);
            fragment.extend(&payload);
            remaining_payload_bits -= bits_to_take;

            let encapsulated_done = front.e_pdu.len() == 0;
            if encapsulated_done {
                let is_non_empty_gpm = front.message.mcsb.message_id == MessageId::GeneralPage
                    && front.message.mcsb.length_bits > 24;
                if front.message.mcsb.address.is_some() || is_non_empty_gpm {
                    let requested_tx_chip = front
                        .message
                        .mcsb
                        .requested_tx_time
                        .map(|ts| time::chips_since_epoch(ts, 1_228_800));
                    let deadline_chip = front
                        .message
                        .mcsb
                        .tx_deadline
                        .map(|ts| time::chips_since_epoch(ts, 1_228_800));
                    let turnaround = front.message.mcsb.requested_tx_time.map(|rx_time| {
                        let delta = current_system_time - rx_time;
                        delta.num_microseconds().unwrap_or(0)
                    });
                    let deadline_margin_us = front.message.mcsb.tx_deadline.map(|deadline| {
                        (deadline - current_system_time)
                            .num_microseconds()
                            .unwrap_or(0)
                    });
                    let tx_minus_req_ms = requested_tx_chip
                        .map(|chip| (chip_cursor as i128 - chip as i128) as f64 / 1228.8);
                    let tx_minus_deadline_ms = deadline_chip
                        .map(|chip| (chip_cursor as i128 - chip as i128) as f64 / 1228.8);
                    info!(
                        "lac_fpch_pdu_done: tag={} tx_chip={} slot_num={} sdu_bits={} req_chip={:?} deadline_chip={:?} turnaround_us={:?} deadline_margin_us={:?} tx_minus_req_ms={:?} tx_minus_deadline_ms={:?}",
                        front.message.mcsb.message_id.tag(),
                        chip_cursor,
                        cdma_common::paging::slot_num_from_chips(chip_cursor, 1_228_800),
                        front.message.mcsb.length_bits,
                        requested_tx_chip,
                        deadline_chip,
                        turnaround,
                        deadline_margin_us,
                        tx_minus_req_ms,
                        tx_minus_deadline_ms,
                    );
                }
                queue.pop_front();
            }

            if remaining_payload_bits == 0 {
                break;
            }
            if !encapsulated_done {
                break;
            }
            if remaining_payload_bits < 8 {
                break;
            }
        }

        let Some(mcsb) = first_mcsb else {
            return Ok(None);
        };

        if let Some(deadline) = mcsb.tx_deadline
            && current_system_time > deadline
        {
            let deadline_chip = time::chips_since_epoch(deadline, 1_228_800);
            let late_us = (current_system_time - deadline)
                .num_microseconds()
                .unwrap_or(0);
            warn!(
                "lac_fpch_deadline_missed: tag={} channel={:?} deadline={} now={} deadline_chip={} chip_cursor={} late_us={} late_ms={:.3}",
                mcsb.message_id.tag(),
                mcsb.channel,
                deadline,
                current_system_time,
                deadline_chip,
                chip_cursor,
                late_us,
                late_us as f64 / 1000.0,
            );
        }

        if remaining_payload_bits > 0 {
            fragment.write_u8(0, remaining_payload_bits);
        }

        Ok(Some(mac::types::DataRequest {
            channel_type: ChannelType::FPch,
            size: fragment.len(),
            mcsb,
            data: fragment,
        }))
    }

    fn build_paging_frame_request(
        &self,
        queue: &mut VecDeque<EncapsulatedPdu>,
        max_size: usize,
        current_system_time: CdmaSystemTime,
        chip_cursor: u64,
    ) -> Result<Option<mac::types::DataRequest>, Error> {
        if max_size <= FPCH_HALF_FRAME_BITS_9600 {
            return self.build_paging_fragment_request(
                queue,
                max_size,
                current_system_time,
                chip_cursor,
            );
        }

        assert_eq!(
            max_size % FPCH_HALF_FRAME_BITS_9600,
            0,
            "F-PCH frame availability must be whole half-frames"
        );

        let mut frame = Bitstream::new();
        let mut first_mcsb = None;
        let mut first_addressed_mcsb = None;

        for half_idx in 0..(max_size / FPCH_HALF_FRAME_BITS_9600) {
            let half_chip =
                chip_cursor.saturating_add(FPCH_HALF_FRAME_CHIPS.saturating_mul(half_idx as u64));
            let half_system_time = if half_idx == 0 {
                current_system_time
            } else {
                time::system_time_from_chips(half_chip, SR1_CHIP_RATE_HZ)
            };

            match self.build_paging_fragment_request(
                queue,
                FPCH_HALF_FRAME_BITS_9600,
                half_system_time,
                half_chip,
            )? {
                Some(data_request) => {
                    if first_mcsb.is_none() {
                        first_mcsb = Some(data_request.mcsb.clone());
                    }
                    if first_addressed_mcsb.is_none() && data_request.mcsb.address.is_some() {
                        first_addressed_mcsb = Some(data_request.mcsb.clone());
                    }
                    frame.extend(&data_request.data);
                }
                None => frame.write_u8(0, FPCH_HALF_FRAME_BITS_9600),
            }
        }

        let Some(mut mcsb) = first_addressed_mcsb.or(first_mcsb) else {
            return Ok(None);
        };
        // The full 20 ms frame has already been scheduled half-frame by
        // half-frame above. Do not let the physical channel delay the whole
        // 192-bit block because the representative MCSB came from half-frame 2.
        mcsb.requested_tx_time = None;

        Ok(Some(mac::types::DataRequest {
            channel_type: ChannelType::FPch,
            size: frame.len(),
            mcsb,
            data: frame,
        }))
    }

    fn is_cancelable_future_general_page(pdu: &EncapsulatedPdu) -> bool {
        !pdu.frame_start_sent
            && pdu.message.mcsb.channel == ChannelType::FPch
            && pdu.message.mcsb.message_id == MessageId::GeneralPage
            && pdu.message.mcsb.requested_tx_time.is_some()
            && pdu.message.mcsb.address.is_none()
    }

    fn apply_pending_future_general_page_cancellation(
        &self,
        queue: &mut VecDeque<EncapsulatedPdu>,
    ) {
        let should_cancel = {
            let mut state = self.state.lock();
            let flag = state.cancel_future_general_pages;
            if flag {
                state.cancel_future_general_pages = false;
            }
            flag
        };
        if !should_cancel {
            return;
        }

        queue.retain(|pdu| !Self::is_cancelable_future_general_page(pdu));
    }

    // When MAC grants an availability window and no SAR data is sendable, emit
    // an explicit zero-length MAC-Data.Request. This preserves the spec-level
    // primitive while allowing the channel encoder to fill the window with idle
    // zero bits.
    fn handle_availability_indication(
        &self,
        indication: AvailabilityIndication,
    ) -> Result<(), Error> {
        trace!(
            "Channel {:?} is now sendable (as of {:?})",
            indication.channel_type, indication.system_time
        );

        assert!(indication.max_size > 0);

        match indication.channel_type {
            ChannelType::FSync => {
                let mut state = self.state.lock();
                let sync_template = state.sync_template.clone();
                let pch_num = state.paging_channel_number;
                let queue = state.message_queue.entry(ChannelType::FSync).or_default();

                if indication.sync_superframe_start
                    && queue.is_empty()
                    && let Some(template) = sync_template
                {
                    let stamped = Self::stamp_and_serialize_sync(
                        template,
                        indication.chip_cursor,
                        indication.max_size,
                        pch_num,
                    )?;
                    queue.push_back(Self::assemble_pdu(stamped)?);
                }

                trace!("Able to send on {:?}", ChannelType::FSync);
                let can_send = if let Some(front) = queue.front() {
                    !(!indication.sync_superframe_start && !front.frame_start_sent)
                } else {
                    false
                };

                let msg = if can_send {
                    let front = queue.front_mut().unwrap();
                    let fragment = front.get_fragment(indication.max_size);
                    let msg = MacMessage::DataRequest(mac::types::DataRequest {
                        channel_type: ChannelType::FSync,
                        size: fragment.len(),
                        mcsb: front.message.mcsb.clone(),
                        data: fragment,
                    });

                    if front.e_pdu.len() == 0 {
                        trace!("Finished sending Encapsulated PDU");
                        queue.pop_front();
                    }
                    msg
                } else {
                    MacMessage::DataRequest(mac::types::DataRequest {
                        channel_type: ChannelType::FSync,
                        size: 0,
                        mcsb: MessageControlStatusBlock {
                            channel: ChannelType::FSync,
                            mobile_p_rev: None,
                            extended_encryption: false,
                            message_id: MessageId::SyncChannelMessage,
                            length_bits: 0,
                            requested_tx_time: None,
                            tx_deadline: None,
                            address: None,
                            ack_seq: 0,
                            msg_seq: 0,
                            ack_req: false,
                            valid_ack: false,
                            overhead_mcc: 0x03ff,
                            overhead_imsi_11_12: 0x7f,
                        },
                        data: Bitstream::new(),
                    })
                };
                trace!("LAC -> MAC: {:?}", msg);
                self.lac_to_mac_tx.send(msg).unwrap();
            }
            ChannelType::FPch => {
                let mut queue = {
                    let mut state = self.state.lock();
                    state.fpch_queue_checked_out = true;
                    std::mem::take(state.message_queue.entry(ChannelType::FPch).or_default())
                };

                let queued_count = queue.len();
                let has_addressed = queue.iter().any(|p| p.message.mcsb.address.is_some());
                if has_addressed {
                    // Build a compact summary of all queued PDU tags
                    let queue_summary: String = queue
                        .iter()
                        .enumerate()
                        .map(|(i, p)| {
                            let tag = p.message.mcsb.message_id.tag();
                            let addr = if p.message.mcsb.address.is_some() {
                                "*"
                            } else {
                                ""
                            };
                            let started = if p.frame_start_sent { "S" } else { "" };
                            format!("[{}]{}{}{}", i, tag, addr, started)
                        })
                        .collect::<Vec<_>>()
                        .join(" ");
                    // Keep addressed PCH scheduling visible in normal live
                    // runs. ECAM/CAM access responses are timing-sensitive,
                    // and this shows whether they are blocked behind queued
                    // PDUs, a requested_tx_time, or an already-started frame.
                    for pdu in queue.iter().filter(|p| p.message.mcsb.address.is_some()) {
                        let sendable_now = pdu
                            .message
                            .mcsb
                            .requested_tx_time
                            .map(|ts| ts <= indication.system_time)
                            .unwrap_or(true);
                        if !sendable_now && !pdu.frame_start_sent {
                            // Skip verbose logging while waiting for TX time
                            continue;
                        }
                        let req_chip = pdu
                            .message
                            .mcsb
                            .requested_tx_time
                            .map(|ts| cdma_common::time::chips_since_epoch(ts, 1_228_800));
                        let deadline_chip = pdu
                            .message
                            .mcsb
                            .tx_deadline
                            .map(|ts| cdma_common::time::chips_since_epoch(ts, 1_228_800));
                        let indication_chip =
                            cdma_common::time::chips_since_epoch(indication.system_time, 1_228_800);
                        info!(
                            "lac_fpch_avail: queued_pdus={} chip={} tag={} sendable_now={} req_chip={:?} deadline_chip={:?} indication_chip={} frame_start_sent={} queue={}",
                            queued_count,
                            indication.chip_cursor,
                            pdu.message.mcsb.message_id.tag(),
                            sendable_now,
                            req_chip,
                            deadline_chip,
                            indication_chip,
                            pdu.frame_start_sent,
                            queue_summary,
                        );
                    }
                }
                let msg = match self.build_paging_frame_request(
                    &mut queue,
                    indication.max_size,
                    indication.system_time,
                    indication.chip_cursor,
                )? {
                    Some(data_request) => MacMessage::DataRequest(data_request),
                    None => MacMessage::DataRequest(mac::types::DataRequest {
                        channel_type: ChannelType::FPch,
                        size: 0,
                        mcsb: MessageControlStatusBlock {
                            channel: ChannelType::FPch,
                            mobile_p_rev: None,
                            extended_encryption: false,
                            message_id: MessageId::SystemParameters,
                            length_bits: 0,
                            requested_tx_time: None,
                            tx_deadline: None,
                            address: None,
                            ack_seq: 0,
                            msg_seq: 0,
                            ack_req: false,
                            valid_ack: false,
                            overhead_mcc: 0x03ff,
                            overhead_imsi_11_12: 0x7f,
                        },
                        data: Bitstream::new(),
                    }),
                };

                {
                    let mut state = self.state.lock();
                    state.fpch_queue_checked_out = false;
                    let live = state.message_queue.entry(ChannelType::FPch).or_default();
                    // Prepend any remaining items from the working queue in
                    // front of items that may have been enqueued by the BSC
                    // while we were building the fragment (avoids dropping
                    // PDUs that arrived during processing).
                    for pdu in queue.into_iter().rev() {
                        live.push_front(pdu);
                    }
                }

                trace!("LAC -> MAC: {:?}", msg);
                self.lac_to_mac_tx.send(msg).unwrap();
            }
            _ => {}
        }

        Ok(())
    }

    /// Allocate and return the next per-mobile GPM page record msg_seq (mod 8).
    pub fn next_gpm_page_seq(&self, page_addr: &MsPageAddress) -> u8 {
        let mut state = self.state.lock();
        let entry = state.gpm_page_seq.entry(page_addr.clone()).or_insert(0);
        let seq = *entry;
        *entry = (seq + 1) % 8;
        seq
    }

    fn mark_directed_pdu_transmitted(&self, mcsb: &MessageControlStatusBlock) -> Result<(), Error> {
        if mcsb.channel != ChannelType::FPch {
            return Ok(());
        }
        let Some(addr) = mcsb.address.as_ref() else {
            return Ok(());
        };

        let mut state = self.state.lock();
        state
            .paging_msg_seq_tracker
            .mark_transmitted(addr, mcsb.ack_req, mcsb.msg_seq)?;
        state
            .pending_directed_pdus
            .mark_transmitted(addr, mcsb.ack_req, mcsb.msg_seq);
        Ok(())
    }

    /// Send a directed SDU on the paging channel, assigning MSG_SEQ via the
    /// LAC's internal ARQ tracker.
    pub fn send_directed_sdu(&self, req: DirectedSduRequest) -> Result<ArqHandle, Error> {
        let (msg_seq, mcsb, data_request) = {
            let mut state = self.state.lock();
            // Split borrows: take tracker out temporarily to satisfy the borrow checker
            let mut tracker = std::mem::replace(
                &mut state.paging_msg_seq_tracker,
                MsgSeqTracker::new(ChannelType::FPch),
            );
            let msg_seq = state.pending_directed_pdus.reserve_msg_seq(
                &mut tracker,
                &req.address,
                req.message_id,
                &req.sdu,
                req.ack_seq,
                req.ack_req,
                req.valid_ack,
            )?;
            state.paging_msg_seq_tracker = tracker;
            let mcsb = MessageControlStatusBlock {
                channel: req.channel,
                length_bits: req.sdu.len(),
                mobile_p_rev: None,
                extended_encryption: false,
                message_id: req.message_id,
                requested_tx_time: req.requested_tx_time,
                tx_deadline: req.tx_deadline,
                address: Some(req.address),
                ack_seq: req.ack_seq,
                msg_seq,
                ack_req: req.ack_req,
                valid_ack: req.valid_ack,
                overhead_mcc: req.overhead_mcc,
                overhead_imsi_11_12: req.overhead_imsi_11_12,
            };
            let data_request = DataRequest {
                sdu: req.sdu,
                mcsb: mcsb.clone(),
            };
            (msg_seq, mcsb, data_request)
        };

        // Enqueue the PDU via the existing send_message path
        self.send_message(LacMessage::DataRequest(data_request))?;

        Ok(ArqHandle { msg_seq, mcsb })
    }

    /// Acknowledge a previously sent directed PDU.
    pub fn acknowledge_pdu(&self, addr: &MsAddress, msg_seq: u8) -> bool {
        let mut state = self.state.lock();
        state.pending_directed_pdus.acknowledge(addr, msg_seq)
    }

    /// Register a callback that supplies the next scheduled paging message
    /// on demand. Called by the LAC whenever the paging queue is empty and
    /// the BTS requests a fragment.
    pub fn set_paging_supplier(&self, supplier: PagingSupplierFn) {
        *self.paging_supplier.lock() = Some(supplier);
    }

    pub fn send_message(&self, message: LacMessage) -> Result<(), Error> {
        trace!("L3 -> LAC: {:?}", message);

        match message {
            LacMessage::SyncChannelTemplate(sync_msg) => {
                let mut state = self.state.lock();
                state.sync_template = Some(sync_msg);
            }
            LacMessage::DataRequest(data_request) => {
                let tag = data_request.mcsb.message_id;
                let ch = data_request.mcsb.channel;
                let has_addr = data_request.mcsb.address.is_some();
                let mut state = self.state.lock();
                let chan = state
                    .message_queue
                    .entry(data_request.mcsb.channel)
                    .or_insert_with(|| VecDeque::new());

                let pdu = Self::assemble_pdu(data_request)?;
                let has_deadline = pdu.message.mcsb.tx_deadline.is_some();
                let requested_tx_time = pdu.message.mcsb.requested_tx_time;
                let tx_deadline = pdu.message.mcsb.tx_deadline;
                let enqueue_now = chrono::Utc::now();
                let req_chip = requested_tx_time.map(|ts| time::chips_since_epoch(ts, 1_228_800));
                let deadline_chip = tx_deadline.map(|ts| time::chips_since_epoch(ts, 1_228_800));
                let enqueue_now_chip = time::chips_since_epoch(enqueue_now, 1_228_800);
                let enqueue_age_us =
                    requested_tx_time.and_then(|ts| (enqueue_now - ts).num_microseconds());
                let enqueue_deadline_margin_us =
                    tx_deadline.and_then(|ts| (ts - enqueue_now).num_microseconds());

                // Priority insertion: if the new PDU has a deadline (e.g. an
                // ack to a recent access probe), insert it ahead of any
                // not-yet-started, unaddressed (overhead) PDUs so it doesn't
                // get stuck behind broadcast filler.
                if has_deadline {
                    let insert_pos = chan
                        .iter()
                        .position(|p| {
                            !p.frame_start_sent
                                && p.message.mcsb.address.is_none()
                                && p.message.mcsb.tx_deadline.is_none()
                        })
                        .unwrap_or(chan.len());
                    chan.insert(insert_pos, pdu);
                } else {
                    chan.push_back(pdu);
                }

                if has_addr {
                    info!(
                        "lac_enqueue: channel={:?} tag={} queue_depth={} addressed={} priority_insert={} req_chip={:?} deadline_chip={:?} enqueue_now_chip={} enqueue_age_us={:?} enqueue_deadline_margin_us={:?}",
                        ch,
                        tag,
                        chan.len(),
                        has_addr,
                        has_deadline,
                        req_chip,
                        deadline_chip,
                        enqueue_now_chip,
                        enqueue_age_us,
                        enqueue_deadline_margin_us,
                    );
                }
            }
            _ => error!("unsupported message: {:?}", message),
        }

        Ok(())
    }

    /// Drop queued future-timed broadcast GPMs that were scheduled for an
    /// idle-page flow. Used when the target MS becomes active on the access
    /// channel and the BSC switches to direct delivery instead.
    pub fn cancel_future_general_pages(&self) -> usize {
        let mut state = self.state.lock();
        let queue = state.message_queue.entry(ChannelType::FPch).or_default();
        let before = queue.len();
        queue.retain(|pdu| !Self::is_cancelable_future_general_page(pdu));
        let removed = before.saturating_sub(queue.len());
        state.cancel_future_general_pages = removed == 0 && state.fpch_queue_checked_out;
        removed
    }

    pub fn set_paging_channel_number(&self, pch_num: u8) {
        self.state.lock().paging_channel_number = pch_num;
    }

    pub fn receive_message(&self) -> Result<LacMessage, Error> {
        todo!()
    }

    /// Assemble a `DataRequest` into an encapsulated forward-link PDU.
    ///
    /// ARQ fields are assigned by the caller and carried in the MCSB; this
    /// function only serializes the LAC PDU and applies SAR encapsulation.
    pub fn assemble_pdu(data_request: DataRequest) -> Result<EncapsulatedPdu, Error> {
        match &data_request.mcsb.channel {
            ChannelType::FSync | ChannelType::FPch => {
                let pdu = utility_assemble_f_csch(&data_request)?;
                if data_request.mcsb.address.is_some() {
                    let pdu_hex = pdu.to_packed_bytes();
                    info!(
                        "LAC assemble_pdu: directed f-csch msg_id={:?} msg_type=0x{:02X} addr={:?} ack_seq={} msg_seq={} ack_req={} valid_ack={} pdu={:02X?} ({} bits, sdu={} bits)",
                        data_request.mcsb.message_id,
                        pdu_hex.first().copied().unwrap_or(0),
                        data_request.mcsb.address,
                        data_request.mcsb.ack_seq,
                        data_request.mcsb.msg_seq,
                        data_request.mcsb.ack_req,
                        data_request.mcsb.valid_ack,
                        pdu_hex,
                        pdu.len(),
                        data_request.sdu.len(),
                    );
                } else if data_request.mcsb.message_id == MessageId::GeneralPage
                    && data_request.mcsb.length_bits > 24
                {
                    let pdu_hex = pdu.to_packed_bytes();
                    info!(
                        "LAC assemble_pdu: broadcast f-csch GPM msg_type=0x{:02X} sdu_bits={} pdu={:02X?} ({} bits)",
                        pdu_hex.first().copied().unwrap_or(0),
                        data_request.sdu.len(),
                        pdu_hex,
                        pdu.len(),
                    );
                }
                let encapsulated = sar_encapsulate_pdu(data_request, pdu)?;
                Ok(encapsulated)
            }
            ChannelType::FTch => {
                // Forward dedicated signaling channel (f-dsch) PDU format
                // per C.S0004-E 3.2.2: MSG_TYPE(8) + ARQ(7) + ENCRYPTION(2)
                // + SDU + PDU_PADDING, wrapped with MSG_LENGTH(8) + CRC-16.
                let pdu = utility_assemble_f_dsch(&data_request);
                let encapsulated = sar_encapsulate_ftch_pdu_dsch(data_request, pdu);
                Ok(encapsulated)
            }
            _ => todo!(),
        }
    }

    /// Stamp timing fields on a `SyncChannelMessage` and serialize it into a
    /// `DataRequest` ready for PDU assembly. LC_STATE and SYS_TIME are computed
    /// from the superframe boundary time provided by the BTS.
    pub fn stamp_and_serialize_sync(
        mut sync_msg: SyncChannelMessage,
        som_chip: u64,
        sync_fragment_bits: usize,
        paging_channel_number: u8,
    ) -> Result<DataRequest, Error> {
        let pilot_offset_chips = (sync_msg.pilot_pn as u64).saturating_mul(64);
        debug_assert!(
            som_chip >= pilot_offset_chips
                && (som_chip - pilot_offset_chips) % SR1_CHIPS_PER_80MS == 0,
            "sync SOM chip must align to pilot-offset superframe boundary"
        );
        if sync_fragment_bits <= 1 {
            return Err("invalid sync fragment size: must be > 1".into());
        }

        let provisional_sdu = sync_msg.to_sdu();
        let provisional_data_request = DataRequest {
            sdu: provisional_sdu,
            mcsb: MessageControlStatusBlock {
                channel: ChannelType::FSync,
                length_bits: 0,
                mobile_p_rev: None,
                extended_encryption: false,
                message_id: MessageId::SyncChannelMessage,
                requested_tx_time: None,
                tx_deadline: None,
                address: None,
                ack_seq: 0,
                msg_seq: 0,
                ack_req: false,
                valid_ack: false,
                overhead_mcc: 0x03ff,
                overhead_imsi_11_12: 0x7f,
            },
        };
        let pdu_bits = utility_assemble_f_csch(&provisional_data_request)?.len();
        let encapsulated_bits = 8 + pdu_bits + 30; // MSG_LENGTH(8) + PDU + CRC30
        let payload_bits_per_sync_frame = sync_fragment_bits - 1; // SOM consumes 1 bit

        let sync_frames_needed = encapsulated_bits.div_ceil(payload_bits_per_sync_frame);
        let sync_superframes_needed = sync_frames_needed.div_ceil(SYNC_FRAMES_PER_SUPERFRAME);
        let last_superframe_end = som_chip + (sync_superframes_needed as u64 * SR1_CHIPS_PER_80MS);
        let paging_start_chip =
            (last_superframe_end + SR1_CHIPS_320MS).saturating_sub(pilot_offset_chips);

        let mut lc_gen =
            LongCodeGenerator::new_paging_channel(paging_channel_number, sync_msg.pilot_pn);
        lc_gen.advance_chips(paging_start_chip as usize);

        sync_msg.lc_state = lc_gen.state();
        sync_msg.sys_time = paging_start_chip / SR1_CHIPS_PER_80MS;
        trace!(
            "tx_sync_stamp: som_chip={} sync_frag_bits={} encap_bits={} sync_frames={} sync_superframes={} last_superframe_end={} paging_start_chip={} pilot_pn={} lc_state=0x{:x} sys_time={}",
            som_chip,
            sync_fragment_bits,
            encapsulated_bits,
            sync_frames_needed,
            sync_superframes_needed,
            last_superframe_end,
            paging_start_chip,
            sync_msg.pilot_pn,
            sync_msg.lc_state,
            sync_msg.sys_time
        );

        let sdu = sync_msg.to_sdu();
        let length_bits = sdu.len();
        trace!("sdu_len={}", length_bits);

        Ok(DataRequest {
            sdu,
            mcsb: MessageControlStatusBlock {
                channel: ChannelType::FSync,
                length_bits,
                mobile_p_rev: None,
                extended_encryption: false,
                message_id: MessageId::SyncChannelMessage,
                requested_tx_time: None,
                tx_deadline: None,
                address: None,
                ack_seq: 0,
                msg_seq: 0,
                ack_req: false,
                valid_ack: false,
                overhead_mcc: 0x03ff,
                overhead_imsi_11_12: 0x7f,
            },
        })
    }
}

pub fn sar_encapsulate_pdu(
    data_request: DataRequest,
    pdu: Bitstream,
) -> Result<EncapsulatedPdu, Error> {
    if data_request.mcsb.channel != ChannelType::FSync
        && data_request.mcsb.channel != ChannelType::FPch
    {
        return Err(format!(
            "common-channel SAR encapsulation not supported for {:?}",
            data_request.mcsb.channel
        )
        .into());
    }
    let mut encapsulated = Bitstream::new();
    let msg_length_octets = sar_message_length_octets(&pdu);
    sar_write_msg_length8(&mut encapsulated, msg_length_octets);
    encapsulated.extend(&pdu);
    sar_write_crc30(&mut encapsulated, msg_length_octets, &pdu);

    Ok(EncapsulatedPdu {
        message: data_request,
        e_pdu: encapsulated,
        frame_start_sent: false,
    })
}

/// Encapsulate a forward traffic channel PDU.
///
/// Traffic channel signaling uses the same MSG_LENGTH(8) + PDU + CRC30
/// format as paging, but the entire encapsulated bitstream is delivered in
/// a single full-rate traffic frame (172 info bits). The frame is padded
/// to 172 bits (the remaining bits after CRC are zero-filled).
pub fn sar_encapsulate_ftch_pdu(data_request: DataRequest, pdu: Bitstream) -> EncapsulatedPdu {
    let mut encapsulated = Bitstream::new();
    let msg_length_octets = sar_message_length_octets(&pdu);
    sar_write_msg_length8(&mut encapsulated, msg_length_octets);
    encapsulated.extend(&pdu);
    sar_write_crc30(&mut encapsulated, msg_length_octets, &pdu);

    // Pad to 172 info bits (full-rate traffic frame)
    const FTCH_FULL_RATE_INFO_BITS: usize = 172;
    if encapsulated.len() < FTCH_FULL_RATE_INFO_BITS {
        let pad = FTCH_FULL_RATE_INFO_BITS - encapsulated.len();
        for _ in 0..pad {
            encapsulated.write_u8(0, 1);
        }
    }

    EncapsulatedPdu {
        message: data_request,
        e_pdu: encapsulated,
        frame_start_sent: false,
    }
}

/// Assemble a forward dedicated signaling channel (f-dsch) regular PDU.
///
/// Per C.S0004-E 3.2.2.2.2, for P_REV_IN_USE < 9:
/// MSG_TYPE(8) + ARQ(7: ACK_SEQ(3)+MSG_SEQ(3)+ACK_REQ(1)) + ENCRYPTION(2) + SDU + PDU_PADDING
///
/// No VALID_ACK, no addressing fields.
pub fn utility_assemble_f_dsch(data_request: &DataRequest) -> Bitstream {
    let msg_type = data_request
        .mcsb
        .message_id
        .wire_type(message_types::WireChannel::ForwardDedicated)
        .unwrap_or_else(|| {
            panic!(
                "utility_assemble_f_dsch: {:?} has no f-dsch wire encoding",
                data_request.mcsb.message_id
            );
        });

    let mut pdu = Bitstream::new();

    // MSG_TYPE(8)
    pdu.write_u8(msg_type, 8);

    // ARQ: ACK_SEQ(3) + MSG_SEQ(3) + ACK_REQ(1) = 7 bits (no VALID_ACK)
    pdu.write_u8(data_request.mcsb.ack_seq, 3);
    pdu.write_u8(data_request.mcsb.msg_seq, 3);
    pdu.write_u8(data_request.mcsb.ack_req as u8, 1);

    // ENCRYPTION(2) = 00 (no encryption)
    pdu.write_u8(0, 2);

    // SDU
    pdu.extend(&data_request.sdu);

    // PDU_PADDING: pad to byte boundary
    let remainder = pdu.len() % 8;
    if remainder != 0 {
        let pad = 8 - remainder;
        for _ in 0..pad {
            pdu.write_u8(0, 1);
        }
    }

    pdu
}

/// CRC-16 for f-dsch SAR encapsulation.
///
/// CRC-16 for dedicated regular PDUs per C.S0004-E 2.2.1.3.1.2.
/// g(x) = x^16 + x^12 + x^5 + 1 = 0x1021.
///
/// The spec describes an encoder that is initialized to all ones, then flushed
/// for 16 clocks with the output inverted. The transmitted CRC bits are
/// equivalent to the non-reflected CRC-CCITT remainder with init=0xFFFF and a
/// final xor of 0xFFFF over MSG_LENGTH(8) + PDU body.
fn crc16_fdsch(data: &Bitstream) -> u16 {
    cdma_common::crc::crc16_ccitt(data.bits())
}

/// SAR encapsulation for f-dsch PDUs on the forward traffic channel,
/// wrapped in MuxPDU Type 1 blank-and-burst frame(s).
///
/// Per C.S0003-E Table 2-29 (MuxPDU Type 1 at 9600 bps), a blank-and-burst
/// frame has a 4-bit header (MM=1, TT=0, TM=11) followed by 168 bits of
/// signaling data.
///
/// Per C.S0004-E Section 2.2.1.3.1.1, the SAR sublayer prepends a 1-bit SOM
/// (Start of Message) indicator to each fragment:
///   - SOM=1 for the first fragment (start of message)
///   - SOM=0 for continuation fragments
///
/// Each frame carries: MuxPDU header(4) + SOM(1) + up to 167 SAR data bits,
/// zero-padded to 168 total signaling bits.
///
/// For small PDUs (e.g. BS Ack Order), a single 172-bit frame suffices.
/// For larger PDUs (e.g. Service Connect), the SAR data is fragmented across
/// multiple consecutive full-rate frames.
pub fn sar_encapsulate_ftch_pdu_dsch(data_request: DataRequest, pdu: Bitstream) -> EncapsulatedPdu {
    let frames = sar_fragment_ftch_pdu_dsch(&pdu);

    // For backwards compatibility, return the first frame in e_pdu.
    // Multi-frame PDUs must use sar_fragment_ftch_pdu_dsch directly.
    debug_assert_eq!(
        frames.len(),
        1,
        "sar_encapsulate_ftch_pdu_dsch: PDU too large for single frame, use sar_fragment_ftch_pdu_dsch"
    );

    EncapsulatedPdu {
        message: data_request,
        e_pdu: frames.into_iter().next().unwrap(),
        frame_start_sent: false,
    }
}

/// Assembles an F-DSCH regular PDU from a wire MSG_TYPE and SDU body bitstream.
///
/// Per C.S0004-E 3.2.2.2.2 (P_REV_IN_USE < 9):
/// MSG_TYPE(8) + ACK_SEQ(3) + MSG_SEQ(3) + ACK_REQ(1) + ENCRYPTION(2) + SDU + PDU_PADDING
pub fn assemble_f_dsch_pdu(
    wire_msg_type: u8,
    sdu_body: &Bitstream,
    ack_seq: u8,
    msg_seq: u8,
    ack_req: bool,
) -> Bitstream {
    let mut pdu = Bitstream::new();
    pdu.write_u8(wire_msg_type, 8);
    pdu.write_u8(ack_seq, 3);
    pdu.write_u8(msg_seq, 3);
    pdu.write_u8(ack_req as u8, 1);
    pdu.write_u8(0, 2); // ENCRYPTION = 00
    pdu.extend(sdu_body);
    let remainder = pdu.len() % 8;
    if remainder != 0 {
        pdu.write_u8(0, 8 - remainder);
    }
    pdu
}

/// Fragment an f-dsch PDU into one or more 172-bit MuxPDU Type 1 frames.
///
/// Returns a `Vec<Bitstream>` where each element is a complete 172-bit frame
/// ready to be sent as a full-rate traffic frame. Frames are ordered — the
/// first has SOM=1, subsequent frames have SOM=0.
pub fn sar_fragment_ftch_pdu_dsch(pdu: &Bitstream) -> Vec<Bitstream> {
    // MSG_LENGTH = ceil((8 + pdu_bits + 16) / 8)
    let total_bits = 8 + pdu.len() + 16;
    let msg_length_octets = total_bits.div_ceil(8) as u8;

    // Build SAR payload: MSG_LENGTH(8) + PDU + CRC-16
    let mut sar_data = Bitstream::new();
    sar_data.write_u8(msg_length_octets, 8);
    sar_data.extend(pdu);
    let mut crc_scope = Bitstream::new();
    crc_scope.write_u8(msg_length_octets, 8);
    crc_scope.extend(pdu);
    sar_data.write_u32(crc16_fdsch(&crc_scope) as u32, 16);

    const SIGNALING_BITS: usize = 168; // Per frame
    const SAR_DATA_PER_FRAME: usize = SIGNALING_BITS - 1; // 167 bits (after SOM)
    const FTCH_FULL_RATE_INFO_BITS: usize = 172;

    let mut frames = Vec::new();
    let sar_bits = sar_data.bits().to_vec();
    let mut offset = 0usize;
    let mut is_first = true;

    while offset < sar_bits.len() {
        let remaining = sar_bits.len() - offset;
        let chunk_len = remaining.min(SAR_DATA_PER_FRAME);

        // Signaling block: SOM(1) + SAR data chunk + zero-pad to 168 bits
        let mut signaling = Bitstream::new();
        signaling.write_u8(if is_first { 1 } else { 0 }, 1); // SOM
        for i in 0..chunk_len {
            signaling.write_u8(sar_bits[offset + i], 1);
        }
        // Zero-pad to 168 signaling bits
        if signaling.len() < SIGNALING_BITS {
            let pad = SIGNALING_BITS - signaling.len();
            signaling.write_u8(0, pad);
        }

        // MuxPDU Type 1 blank-and-burst header: MM=1, TT=0, TM=11
        let mut mux_pdu = Bitstream::new();
        mux_pdu.write_u8(1, 1); // MM = 1
        mux_pdu.write_u8(0, 1); // TT = 0
        mux_pdu.write_u8(0b11, 2); // TM = 11
        mux_pdu.extend(&signaling);
        debug_assert_eq!(mux_pdu.len(), FTCH_FULL_RATE_INFO_BITS);

        frames.push(mux_pdu);
        offset += chunk_len;
        is_first = false;
    }

    frames
}

fn sar_message_length_octets(pdu: &Bitstream) -> u8 {
    ((8 + pdu.len() + 30) / 8) as u8
}

fn sar_write_crc30(bs: &mut Bitstream, msg_length_octets: u8, pdu: &Bitstream) {
    let mut crc_scope = Bitstream::new();
    crc_scope.write_u8(msg_length_octets, 8);
    crc_scope.extend(pdu);
    bs.write_u32(crc30(&crc_scope), 30);
}

pub fn crc30(data: &Bitstream) -> u32 {
    let mut register: u32 = 0x3fffffff;
    let polynomial: u32 = 0b100000001100001011100111000111;

    let bits = data.bits();

    for x in 0..bits.len() {
        let input = ((register >> 29) & 1) ^ (bits[x] as u32);
        if input & 1 == 1 {
            register = (register << 1) ^ polynomial;
        } else {
            register <<= 1;
        }
    }

    (register & 0x3fffffff) ^ 0x3fffffff
}

fn sar_write_msg_length8(bs: &mut Bitstream, length_octets: u8) {
    bs.write_u8(length_octets, 8);
}

/*
12 3.1.2.3.1.1.2 Requirements for Setting Message Type Fields
13 The base station shall set the PD field as follows:
14 • 15
16
17
18 19
20 • 21
22
23
24 •
25 The base station shall set the MSG_ID field in PDUs transmitted on the f-csch as shown in
26 Table 3.1.2.3.1.1.2-1.
27
50 If PD is ‘10’, both the MACI_INCL and the ENC_FIELDS_INCL fields are present in the PDU.
If the message is addressed to a mobile station with MOB_P_REV greater than or equal to nine and the message is sent on the Paging Channel, the Forward Common Control Channel or the non-primary Broadcast Channel and the message contains the Extended-Encryption Fields and the Message Integrity Fields50, the base station shall set the PD field to ‘10’. The base station shall not send PDUs containing the Message Integrity Fields, but not the Extended-Encryption Fields.
Otherwise, if the message is addressed to a mobile station with MOB_P_REV greater than or equal to seven and the message is sent on the Paging Channel and the message contains Extended-Encryption Fields, the base station shall set the PD field to ‘01’.
Otherwise, the base station shall set the PD field to ‘00’.
*/
// PD=01/10 support currently covers the no-encryption/no-integrity indicator
// form: ENC_FIELDS_INCL=0 and, for PD=10, MACI_INCL=0. Non-zero encryption
// fields, message integrity metadata, MACI generation, and multi-record MACI
// placement require typed LAC security metadata before they can be enabled.
//
// ---------------------------------------------------------------------------
// Forward Common Signaling Channel PDU Assembly
// C.S0004-E 3.1.2.3.2 — PDU formats by channel and PD value
// ---------------------------------------------------------------------------
//
// Sync/Primary BCCH (3.1.2.3.2.1):
//   MSG_TYPE | SDU | PDU_PADDING
//
// Paging Channel, PD='00' (3.1.2.3.2.2):
//   Broadcast (SPM, APM, NLM, CCLM, ESPM, etc.):
//     MSG_TYPE | SDU | PDU_PADDING
//   GPM: custom record-based format (assembled in paging_messages.rs)
//   ECAM/MECAM (multi-record capable):
//     MSG_TYPE | { ARQ | Addressing | RESERVED_1(1) | ADD_RECORD_LEN(8) | SDU }* | PDU_PADDING
//   Order/CAM (multi-record capable):
//     MSG_TYPE | { ARQ | Addressing | SDU }* | PDU_PADDING
//   Any other addressed:
//     MSG_TYPE | ARQ | Addressing | SDU | PDU_PADDING
//
// Paging Channel, PD='01' (3.1.2.3.2.3) — Extended-Encryption, no integrity:
//   Inserts Extended-Encryption Fields after Addressing (before RESERVED_1 for ECAM/MECAM).
//   Implemented for ENC_FIELDS_INCL=0. Encrypted SDUs are not implemented.
//
// Paging Channel, PD='10' (3.1.2.3.2.3) — Extended-Encryption + Message Integrity:
//   Inserts MsgIntegrity + Extended-Encryption after Addressing.
//   Appends 32-bit MACI. For multi-record messages (Order/CAM/ECAM/MECAM),
//   MACI comes BEFORE PDU_PADDING. For single-record, MACI comes AFTER PDU_PADDING.
//   Implemented for MACI_INCL=0 and ENC_FIELDS_INCL=0. MACI generation is not implemented.
//
// FCCH (3.1.2.3.2.4.1) — always includes Extended-Encryption Fields, even at PD='00':
//   NOT IMPLEMENTED — we only use Paging Channel currently.
//
// Current implementation: single-record Paging Channel PDUs. PD=01/10 emit
// zero-valued security indicator fields only. Multi-record, encrypted SDUs,
// non-zero Message Integrity Fields, generated MACI, and FCCH are deferred.
// ---------------------------------------------------------------------------
pub fn utility_assemble_f_csch(data_request: &DataRequest) -> Result<Bitstream, Error> {
    let mut pd = 0;
    if let Some(mobile_p_rev) = data_request.mcsb.mobile_p_rev {
        // todo - non primary bcch?
        // todo - The base station shall not send PDUs containing the Message Integrity Fields, but not the Extended-Encryption Fields.
        if data_request.mcsb.address.is_some()
            && mobile_p_rev >= 9
            && (data_request.mcsb.channel == ChannelType::FPch
                || data_request.mcsb.channel == ChannelType::FCcch
                || data_request.mcsb.channel == ChannelType::FBcch)
            && data_request.mcsb.extended_encryption
        {
            pd = 0b10;
        } else if data_request.mcsb.address.is_some()
            && mobile_p_rev >= 7
            && data_request.mcsb.channel == ChannelType::FPch
            && data_request.mcsb.extended_encryption
        {
            pd = 0b01;
        }
    }

    let wire_channel = if data_request.mcsb.channel == ChannelType::FSync {
        message_types::WireChannel::Sync
    } else {
        message_types::WireChannel::ForwardCommon
    };
    let wire_tag = data_request
        .mcsb
        .message_id
        .wire_type(wire_channel)
        .ok_or_else(|| {
            format!(
                "utility_assemble_f_csch: {:?} has no {:?} wire encoding",
                data_request.mcsb.message_id, wire_channel
            )
        })?;
    let message_type = (pd << 6) | wire_tag;

    let is_fcch = data_request.mcsb.channel == ChannelType::FCcch;

    if data_request.mcsb.channel == ChannelType::FBcch
        || data_request.mcsb.channel == ChannelType::FSync
        || data_request.mcsb.channel == ChannelType::FPch
        || is_fcch
        || data_request.mcsb.channel == ChannelType::FTch
    {
        let mut pdu = Bitstream::new();

        if let Some(ref addr) = data_request.mcsb.address {
            // Addressed PDU on Paging Channel per C.S0005-E 3.7.2.3.2:
            // MSG_TYPE(8) + ARQ(8) + Addressing + SDU
            //
            // MSG_TYPE must come first so the mobile can determine the
            // message type before parsing the ARQ and addressing fields.
            // MSG_TYPE: PD(2) + MSG_ID(6) = 8 bits
            pdu.write_u8(message_type, 8);
            // ARQ: ACK_SEQ(3) + MSG_SEQ(3) + ACK_REQ(1) + VALID_ACK(1) = 8 bits
            pdu.write_u8(data_request.mcsb.ack_seq, 3);
            pdu.write_u8(data_request.mcsb.msg_seq, 3);
            pdu.write_u8(data_request.mcsb.ack_req as u8, 1);
            pdu.write_u8(data_request.mcsb.valid_ack as u8, 1);
            // Addressing: ADDR_TYPE(3) + ADDR_LEN(4) + ADDRESS(variable)
            addr.write_to(
                &mut pdu,
                data_request.mcsb.overhead_mcc,
                data_request.mcsb.overhead_imsi_11_12,
            );
            if pd == 0b10 {
                // Message Integrity Fields: MACI_INCL=0, so the remaining
                // integrity fields and trailing MACI are omitted.
                pdu.write_u8(0, 1);
            }
            if pd == 0b01 || pd == 0b10 || is_fcch {
                // Extended-Encryption Fields: ENC_FIELDS_INCL=0, so
                // SDU_ENCRYPT_MODE and ENC_SEQ are omitted. FCCH carries
                // this indicator even for non-encrypted PD='00' PDUs.
                pdu.write_u8(0, 1);
            }

            // Per C.S0004-E 3.1.2.3.2.2: PDUs carrying the Extended Channel
            // Assignment Message (ECAM) or MEID Extended Channel Assignment
            // Message (MECAM) include RESERVED_1(1) + ADD_RECORD_LEN(8)
            // between the addressing fields and the SDU.
            //
            // When PD != '00', the security indicator fields are inserted
            // above before RESERVED_1, matching the PD=01/10 ECAM/MECAM
            // record layouts in C.S0004-E 3.1.2.3.2.3.
            if data_request.mcsb.message_id == MessageId::ExtChannelAssignment
                || data_request.mcsb.message_id == MessageId::MeidExtChannelAssignment
            {
                let sdu_len_octets = data_request.sdu.len().div_ceil(8);
                pdu.write_u8(0, 1); // RESERVED_1
                pdu.write_u8(sdu_len_octets as u8, 8); // ADD_RECORD_LEN
            }

            // SDU
            pdu.extend(&data_request.sdu);
        } else {
            // Broadcast PDU: MSG_TYPE(8) + SDU (no ARQ, no addressing)
            pdu.write_u8(message_type, 8);
            pdu.extend(&data_request.sdu);
        }

        pad_8k2(&mut pdu);
        Ok(pdu)
    } else {
        Err(format!("unsupported channel: {:?}", data_request.mcsb.channel).into())
    }
}

// ---------------------------------------------------------------------------
// MSG_SEQ Tracker (C.S0004-E 3.1.2.1.2.2)
// ---------------------------------------------------------------------------

/// Per-destination MSG_SEQ counter for directed forward PDUs.
///
/// The sequence space depends on the channel:
/// - **f-csch (paging)**: modulo 8 (0..=7)
/// - **f-dsch (traffic)**: modulo 8 (0..=7)
///
/// On reverse traffic, ACK_SEQ='111' only means "no forward ACK pending" when
/// the mobile has not yet received an ACK-required forward PDU since channel
/// acquisition/reset. It is still a valid ACK for forward MSG_SEQ=7.
///
/// Per C.S0004-E §3.1.2.1.1.2, f-csch MSG_SEQ numbering is maintained per
/// (address type, address, ack_req class). The `ack_req` parameter is
/// included in the key so that assured and unassured PDUs have independent
/// sequence streams. GPM/UPM and other non-directed overhead are handled
/// separately and do not use this tracker.
pub struct MsgSeqTracker {
    counters: HashMap<(u8, Vec<u8>, bool), MsgSeqEntry>,
    modulus: u8,
    /// T4m reuse cooldown (C.S0004-E §3.1.2.1.2.2). Only applies to f-csch.
    t4m: Option<Duration>,
}

struct MsgSeqEntry {
    next_seq: u8,
    /// Per-MSG_SEQ last-transmission time for T4m reuse enforcement.
    /// Index = MSG_SEQ value (0..modulus).
    seq_last_transmitted: [Option<Instant>; 8],
}

/// T4m per C.S0004-E Annex A: 2.2 seconds.
const T4M_DURATION: Duration = Duration::from_millis(2_200);

impl MsgSeqTracker {
    pub fn new(channel: ChannelType) -> Self {
        let modulus = match channel {
            ChannelType::FTch => 8,
            _ => 8,
        };
        let t4m = match channel {
            ChannelType::FPch => Some(T4M_DURATION),
            _ => None,
        };
        MsgSeqTracker {
            counters: HashMap::new(),
            modulus,
            t4m,
        }
    }

    /// Allocate the next MSG_SEQ for the given directed destination.
    ///
    /// Per C.S0004-E §3.1.2.1.1.2, separate numbering sequences are
    /// maintained for messages requiring acknowledgment (`ack_req=true`)
    /// and messages not requiring acknowledgment (`ack_req=false`).
    ///
    /// Per C.S0004-E §3.1.2.1.2.2, on f-csch the BS shall wait at least
    /// T4m (2.2s) after transmitting a MSG_SEQ number before using the
    /// same number in a *different* PDU. If the next candidate is still
    /// in cooldown, we advance to the next available value.
    pub fn next_seq(&mut self, addr: &MsAddress, ack_req: bool) -> Result<u8, Error> {
        self.next_seq_excluding(addr, ack_req, |_| false)
    }

    fn next_seq_excluding<F>(
        &mut self,
        addr: &MsAddress,
        ack_req: bool,
        mut excluded: F,
    ) -> Result<u8, Error>
    where
        F: FnMut(u8) -> bool,
    {
        let (addr_type, addr_bytes) = addr.tracking_key();
        let key = (addr_type, addr_bytes, ack_req);
        let modulus = self.modulus;
        let t4m = self.t4m;
        let entry = self.counters.entry(key).or_insert(MsgSeqEntry {
            next_seq: 0,
            seq_last_transmitted: [None; 8],
        });

        let now = Instant::now();
        let mut seq = entry.next_seq;

        for _ in 0..modulus {
            let in_cooldown = t4m
                .and_then(|t4m_dur| {
                    entry.seq_last_transmitted[seq as usize]
                        .map(|last_tx| now.duration_since(last_tx) < t4m_dur)
                })
                .unwrap_or(false);
            if !excluded(seq) && !in_cooldown {
                entry.next_seq = (seq + 1) % modulus;
                return Ok(seq);
            }
            seq = (seq + 1) % modulus;
        }

        if t4m.is_some() {
            return Err("f-csch MSG_SEQ space exhausted within T4m cooldown".into());
        }

        Err("MSG_SEQ space exhausted by pending PDUs".into())
    }

    pub fn mark_transmitted(
        &mut self,
        addr: &MsAddress,
        ack_req: bool,
        msg_seq: u8,
    ) -> Result<(), Error> {
        self.mark_transmitted_at(addr, ack_req, msg_seq, Instant::now())
    }

    fn mark_transmitted_at(
        &mut self,
        addr: &MsAddress,
        ack_req: bool,
        msg_seq: u8,
        transmitted_at: Instant,
    ) -> Result<(), Error> {
        if msg_seq >= self.modulus {
            return Err(format!("MSG_SEQ {msg_seq} exceeds modulo {}", self.modulus).into());
        }

        let (addr_type, addr_bytes) = addr.tracking_key();
        let key = (addr_type, addr_bytes, ack_req);
        let entry = self.counters.entry(key).or_insert(MsgSeqEntry {
            next_seq: 0,
            seq_last_transmitted: [None; 8],
        });
        entry.seq_last_transmitted[msg_seq as usize] = Some(transmitted_at);
        Ok(())
    }
}

fn pad_8k2(bits: &mut Bitstream) {
    while bits.len() % 8 != 2 {
        bits.write_u8(0, 1);
    }
}

#[cfg(test)]
mod tests {
    use std::{
        collections::VecDeque,
        sync::{Arc, mpsc},
        time::{Duration, Instant},
    };

    use cdma_common::bits::Bitstream;
    use cdma_common::consts::SERVICE_OPTION_SMS;
    use chrono::Utc;
    use parking_lot::Mutex;

    use crate::{
        bts::{
            paging_supplier::{PagingSupplierState, PendingPageRecord, build_bts_paging_supplier},
            settings::{OverheadParameters, PagingChannelSettings},
        },
        lac::{
            DataRequest, MessageControlStatusBlock, MsAddress,
            message_types::{MessageId, WireChannel},
            pad_8k2,
            paging_messages::{GeneralPageMessage, GeneralPageRecord, MsPageAddress},
        },
        mac::types::ChannelType,
        receiver::paging::{PagingChannelRate, PagingFrameReader},
    };

    use super::{
        DirectedSduRequest, FPCH_HALF_FRAME_CHIPS, FPCH_HALF_FRAME_PAYLOAD_BITS_9600,
        FPCH_SLOT_CHIPS, Layer2Lac, MsgSeqTracker, T4M_DURATION, crc16_fdsch, crc30,
        sar_encapsulate_ftch_pdu_dsch, sar_encapsulate_pdu, sar_fragment_ftch_pdu_dsch,
        utility_assemble_f_csch, utility_assemble_f_dsch,
    };

    #[test]
    fn test_crc16_fdsch_matches_real_trace() {
        // Real BS Ack Order trace: MSG_LENGTH=8, MSG_TYPE=0x01,
        // ACK_SEQ=7, MSG_SEQ=0, ACK_REQ=1, ENCRYPTION=00,
        // USE_TIME=0, ACTION_TIME=000000, ORDER=010000, ADD_RECORD_LEN=000,
        // PDU_PADDING=0000000, CRC=0x32B2
        let mut bs = Bitstream::new();
        bs.write_u8(0x08, 8); // MSG_LENGTH
        bs.write_u8(0x01, 8); // MSG_TYPE
        bs.write_u8(7, 3); // ACK_SEQ
        bs.write_u8(0, 3); // MSG_SEQ
        bs.write_u8(1, 1); // ACK_REQ
        bs.write_u8(0, 2); // ENCRYPTION
        bs.write_u8(0, 1); // USE_TIME
        bs.write_u8(0, 6); // ACTION_TIME
        bs.write_u8(0b010000, 6); // ORDER = 16 (BS Ack)
        bs.write_u8(0, 3); // ADD_RECORD_LEN
        bs.write_u8(0, 7); // PDU_PADDING
        let crc = crc16_fdsch(&bs);
        assert_eq!(
            crc, 0x32B2,
            "CRC-16 mismatch: got 0x{:04X}, expected 0x32B2",
            crc
        );
    }

    #[test]
    fn test_bs_ack_order_fdsch_pdu_matches_real_trace() {
        // Reconstruct the exact BS Ack Order MuxPDU.
        // Layout: MuxPDU header(4) + SOM(1) + MSG_LENGTH(8) + MSG_TYPE(8) +
        //   ACK_SEQ(3) + MSG_SEQ(3) + ACK_REQ(1) + ENCRYPTION(2) +
        //   USE_TIME(1) + ACTION_TIME(6) + ORDER(6) + ADD_RECORD_LEN(3) +
        //   PDU_PADDING(7) + CRC-16(16) + zero-pad = 172 bits

        use crate::lac::paging_messages::OrderMessage;

        let order_msg = OrderMessage {
            order: 0b010000,
            ordq: 0,
            order_specific_fields: Vec::new(),
        };
        let sdu = order_msg.to_ftch_sdu();

        // SDU should be: USE_TIME(1)=0 + ACTION_TIME(6)=000000 + ORDER(6)=010000 + ADD_RECORD_LEN(3)=000 = 16 bits
        assert_eq!(sdu.len(), 16, "SDU length");

        let data_request = DataRequest {
            sdu: sdu.clone(),
            mcsb: MessageControlStatusBlock {
                channel: ChannelType::FTch,
                length_bits: sdu.len(),
                mobile_p_rev: None,
                extended_encryption: false,
                message_id: MessageId::Order,
                requested_tx_time: None,
                tx_deadline: None,
                address: None,
                ack_seq: 7,
                msg_seq: 0,
                ack_req: true,
                valid_ack: true,
                overhead_mcc: 0x03ff,
                overhead_imsi_11_12: 0x7f,
            },
        };

        // Assemble f-dsch PDU
        let pdu = utility_assemble_f_dsch(&data_request);
        // PDU = MSG_TYPE(8) + ACK_SEQ(3) + MSG_SEQ(3) + ACK_REQ(1) + ENCRYPTION(2) + SDU(16) + PAD(7) = 40 bits
        assert_eq!(pdu.len(), 40, "PDU length");

        // Encapsulate with SAR + MuxPDU framing
        let encapsulated = sar_encapsulate_ftch_pdu_dsch(data_request, pdu);
        let bits = encapsulated.e_pdu.bits().to_vec();

        // Total should be 172 info bits (full-rate frame)
        assert_eq!(bits.len(), 172, "encapsulated length");

        // MuxPDU Type 1 header (4 bits): MM=1, TT=0, TM=11
        assert_eq!(bits[0], 1, "MM");
        assert_eq!(bits[1], 0, "TT");
        assert_eq!(bits[2], 1, "TM[0]");
        assert_eq!(bits[3], 1, "TM[1]");

        // SOM = 1 (start of message)
        assert_eq!(bits[4], 1, "SOM");

        // All subsequent fields are offset by 5 bits (4 header + 1 SOM)
        let o = 5usize;

        // MSG_LENGTH = 8
        let msg_length: u8 = (o..o + 8).fold(0u8, |acc, i| (acc << 1) | bits[i]);
        assert_eq!(msg_length, 8, "MSG_LENGTH");

        // MSG_TYPE = 0x01
        let msg_type: u8 = (o + 8..o + 16).fold(0u8, |acc, i| (acc << 1) | bits[i]);
        assert_eq!(msg_type, 0x01, "MSG_TYPE");

        // ACK_SEQ(3) = 7
        let ack_seq: u8 = (o + 16..o + 19).fold(0u8, |acc, i| (acc << 1) | bits[i]);
        assert_eq!(ack_seq, 7, "ACK_SEQ");

        // MSG_SEQ(3) = 0
        let msg_seq: u8 = (o + 19..o + 22).fold(0u8, |acc, i| (acc << 1) | bits[i]);
        assert_eq!(msg_seq, 0, "MSG_SEQ");

        // ACK_REQ(1) = 1
        assert_eq!(bits[o + 22], 1, "ACK_REQ");

        // ENCRYPTION(2) = 00
        assert_eq!(bits[o + 23], 0, "ENCRYPTION[0]");
        assert_eq!(bits[o + 24], 0, "ENCRYPTION[1]");

        // USE_TIME(1) = 0
        assert_eq!(bits[o + 25], 0, "USE_TIME");

        // ACTION_TIME(6) = 000000
        for i in 0..6 {
            assert_eq!(bits[o + 26 + i], 0, "ACTION_TIME bit {}", i);
        }

        // ORDER(6) = 010000
        let order: u8 = (o + 32..o + 38).fold(0u8, |acc, i| (acc << 1) | bits[i]);
        assert_eq!(order, 0b010000, "ORDER");

        // ADD_RECORD_LEN(3) = 000
        let arl: u8 = (o + 38..o + 41).fold(0u8, |acc, i| (acc << 1) | bits[i]);
        assert_eq!(arl, 0, "ADD_RECORD_LEN");

        // PDU_PADDING(7) = 0000000
        for i in 0..7 {
            assert_eq!(bits[o + 41 + i], 0, "PDU_PADDING bit {}", i);
        }

        // CRC-16 at bits (o+48)..(o+64) = 0x32B2
        let crc: u16 = (o + 48..o + 64).fold(0u16, |acc, i| (acc << 1) | bits[i] as u16);
        assert_eq!(crc, 0x32B2, "CRC-16: got 0x{:04X}, expected 0x32B2", crc);
    }

    /// Build a Service Connect Message using the reference trace params (RC3, SO6),
    /// run it through the full f-dsch PDU assembly + SAR encapsulation pipeline,
    /// verify the CRC-16 is valid, and decode it back to verify all fields.
    #[test]
    fn test_service_connect_full_pdu_crc_and_decode() {
        use crate::lac::paging_messages::{
            NonNegServiceConfig, ServiceConnectConnectionRecord, ServiceConnectParams,
        };
        use crate::receiver::access_layer3::{FdschMessage, FdschPdu, ServiceConnectRecord};

        let params = ServiceConnectParams {
            serv_con_seq: 0,
            use_old_serv_config: 0,
            for_mux_option: 0x0001,
            rev_mux_option: 0x0001,
            for_rates: 0xF0,
            rev_rates: 0xF0,
            sync_id: None,
            connections: vec![ServiceConnectConnectionRecord {
                con_ref: 0,
                service_option: 6,
                for_traffic: 1,
                rev_traffic: 1,
                ui_encrypt_mode: 0,
                sr_id: 1,
                rlp_info_incl: false,
                rlp_blob: None,
                qos_parms: None,
            }],
            fch_frame_size: 0,
            for_fch_rc: 3,
            rev_fch_rc: 3,
            call_assignments: Vec::new(),
            use_type0_plcm: false,
            non_neg: Some(NonNegServiceConfig::rc3_default()),
            for_sch_config: None,
        };

        let sdu = params.to_ftch_sdu();

        let data_request = DataRequest {
            sdu: sdu.clone(),
            mcsb: MessageControlStatusBlock {
                channel: ChannelType::FTch,
                length_bits: sdu.len(),
                mobile_p_rev: None,
                extended_encryption: false,
                message_id: MessageId::ServiceConnect,
                requested_tx_time: None,
                tx_deadline: None,
                address: None,
                ack_seq: 7,
                msg_seq: 1,
                ack_req: true,
                valid_ack: true,
                overhead_mcc: 0x03ff,
                overhead_imsi_11_12: 0x7f,
            },
        };

        // Step 1: Assemble f-dsch PDU (MSG_TYPE + ARQ + ENCRYPTION + SDU + PDU_PADDING)
        let pdu = utility_assemble_f_dsch(&data_request);
        // PDU should be byte-aligned
        assert_eq!(pdu.len() % 8, 0, "PDU should be byte-aligned");

        // Step 2: Verify CRC-16 over MSG_LENGTH + PDU
        let msg_length_octets = (8 + pdu.len() + 16).div_ceil(8) as u8;
        let mut crc_scope = Bitstream::new();
        crc_scope.write_u8(msg_length_octets, 8);
        crc_scope.extend(&pdu);
        let crc = crc16_fdsch(&crc_scope);
        // CRC must not be 0x0000 or 0xFFFF (sanity check)
        assert!(crc != 0x0000, "CRC should not be zero");

        // Step 3: Fragment into multiple frames via SAR
        let frames = sar_fragment_ftch_pdu_dsch(&pdu);
        assert!(
            frames.len() >= 2,
            "Service Connect should require multiple frames, got {}",
            frames.len()
        );

        // Verify each frame is exactly 172 bits
        for (i, frame) in frames.iter().enumerate() {
            assert_eq!(frame.len(), 172, "frame {} should be 172 bits", i);
        }

        // Verify first frame has SOM=1, subsequent frames have SOM=0
        let bits0 = frames[0].bits().to_vec();
        assert_eq!(bits0[0], 1, "frame 0 MM=1");
        assert_eq!(bits0[1], 0, "frame 0 TT=0");
        assert_eq!(bits0[2], 1, "frame 0 TM[0]=1");
        assert_eq!(bits0[3], 1, "frame 0 TM[1]=1");
        assert_eq!(bits0[4], 1, "frame 0 SOM=1 (start of message)");

        let bits1 = frames[1].bits().to_vec();
        assert_eq!(bits1[0], 1, "frame 1 MM=1");
        assert_eq!(bits1[1], 0, "frame 1 TT=0");
        assert_eq!(bits1[2], 1, "frame 1 TM[0]=1");
        assert_eq!(bits1[3], 1, "frame 1 TM[1]=1");
        assert_eq!(bits1[4], 0, "frame 1 SOM=0 (continuation)");

        // Reassemble SAR data from all frames and verify CRC
        let mut reassembled = Bitstream::new();
        for frame in &frames {
            let fbits = frame.bits();
            // Skip MuxPDU header (4 bits) + SOM (1 bit), take 167 data bits
            for i in 5..fbits.len() {
                reassembled.write_u8(fbits[i], 1);
            }
        }
        // Extract MSG_LENGTH from reassembled data
        let rbits = reassembled.bits().to_vec();
        let reassembled_ml: u8 = (0..8).fold(0u8, |acc, i| (acc << 1) | rbits[i]);
        assert_eq!(reassembled_ml, msg_length_octets, "reassembled MSG_LENGTH");

        // Verify CRC in reassembled stream
        let mut verify_scope = Bitstream::new();
        for i in 0..(8 + pdu.len()) {
            verify_scope.write_u8(rbits[i], 1);
        }
        assert_eq!(crc16_fdsch(&verify_scope), crc, "reassembled CRC matches");

        // Step 4: Decode the PDU and verify all fields match
        let decoded = FdschPdu::decode(&pdu).expect("decode Service Connect PDU");
        assert_eq!(decoded.raw_msg_type, 0x14);
        assert_eq!(decoded.arq.ack_seq, 7);
        assert_eq!(decoded.arq.msg_seq, 1);
        assert!(decoded.arq.ack_req);

        let FdschMessage::ServiceConnect(ref sc) = decoded.body else {
            panic!("expected ServiceConnect body, got {:?}", decoded.body);
        };
        assert_eq!(sc.serv_con_seq, 0);
        assert_eq!(sc.records.len(), 2);

        let ServiceConnectRecord::ServiceConfig(ref cfg) = sc.records[0] else {
            panic!("expected ServiceConfig record");
        };
        assert_eq!(cfg.for_mux_option, 0x0001);
        assert_eq!(cfg.rev_mux_option, 0x0001);
        assert_eq!(cfg.for_rates, 0xF0);
        assert_eq!(cfg.rev_rates, 0xF0);
        assert_eq!(cfg.connection_records.len(), 1);
        assert_eq!(cfg.connection_records[0].con_ref, 0);
        assert_eq!(cfg.connection_records[0].service_option, SERVICE_OPTION_SMS);
        assert_eq!(cfg.connection_records[0].for_traffic, 1);
        assert_eq!(cfg.connection_records[0].rev_traffic, 1);
        assert_eq!(cfg.connection_records[0].ui_encrypt_mode, 0);
        assert_eq!(cfg.connection_records[0].sr_id, 1);
        assert!(!cfg.connection_records[0].rlp_info_incl);
        assert!(cfg.fch_cc_incl);
        assert_eq!(cfg.fch_frame_size, Some(0));
        assert_eq!(cfg.for_fch_rc, Some(3));
        assert_eq!(cfg.rev_fch_rc, Some(3));

        let ServiceConnectRecord::NonNegServiceConfig(ref nn) = sc.records[1] else {
            panic!("expected NonNegServiceConfig record");
        };
        assert_eq!(nn.raw_bytes, vec![0x84, 0x40, 0x0A, 0x02, 0x00]);
    }

    #[test]
    pub fn test_pad8k2() {
        let mut buf = Bitstream::new_init(&[1]);
        pad_8k2(&mut buf);
        assert_eq!(&[1, 0], buf.bits());

        buf = Bitstream::new_init(&[1, 1, 1, 1, 1, 1, 1, 1]);
        pad_8k2(&mut buf);
        assert_eq!(&[1, 1, 1, 1, 1, 1, 1, 1, 0, 0], buf.bits());

        buf = Bitstream::new_init(&[1, 1, 1, 1, 1, 1, 1, 1, 1]);
        pad_8k2(&mut buf);
        assert_eq!(&[1, 1, 1, 1, 1, 1, 1, 1, 1, 0], buf.bits());

        buf = Bitstream::new_init(&[1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1]);
        pad_8k2(&mut buf);
        assert_eq!(
            &[1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 1, 0, 0, 0, 0, 0],
            buf.bits()
        );
    }

    #[test]
    pub fn test_crc30() {
        let bs = Bitstream::new_bytes("123456789".as_bytes());
        assert_eq!(0x04C34ABF, crc30(&bs));

        let mut bs = Bitstream::new_bytes("123456789".as_bytes());
        bs.write_u32(0x04C34ABF, 30);
        assert_eq!(0x34efa55a ^ 0x3fffffff, crc30(&bs));
    }

    fn make_paging_request(sdu_bits: &[u8], message_id: MessageId) -> DataRequest {
        DataRequest {
            sdu: Bitstream::new_init(sdu_bits),
            mcsb: MessageControlStatusBlock {
                channel: ChannelType::FPch,
                mobile_p_rev: None,
                extended_encryption: false,
                message_id,
                length_bits: sdu_bits.len(),
                requested_tx_time: None,
                tx_deadline: None,
                address: None,
                ack_seq: 0,
                msg_seq: 0,
                ack_req: false,
                valid_ack: false,
                overhead_mcc: 0x03ff,
                overhead_imsi_11_12: 0x7f,
            },
        }
    }

    #[test]
    fn common_channel_sar_rejects_unsupported_channel() {
        let mut request = make_paging_request(&[1, 0, 1, 1], MessageId::Order);
        request.mcsb.channel = ChannelType::FCcch;
        let pdu = Bitstream::new_init(&[1, 0, 1, 1]);

        let err = sar_encapsulate_pdu(request, pdu).unwrap_err();
        assert!(
            err.to_string().contains("SAR encapsulation not supported"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn f_csch_pd01_writes_extended_encryption_indicator() {
        let mut request = make_paging_request(&[1, 0, 1, 0], MessageId::Order);
        request.mcsb.mobile_p_rev = Some(7);
        request.mcsb.extended_encryption = true;
        request.mcsb.address = Some(MsAddress::Esn(0x1234_5678));

        let mut pdu = utility_assemble_f_csch(&request).unwrap();
        assert_eq!(pdu.read_bits(2).unwrap(), 0b01); // PD
        assert_eq!(pdu.read_bits(6).unwrap(), 0x07); // ORDM MSG_ID
        let _arq = pdu.read_bits(8).unwrap();
        let _addr_type = pdu.read_bits(3).unwrap();
        let _addr_len = pdu.read_bits(4).unwrap();
        let _esn = pdu.read_bits(32).unwrap();
        assert_eq!(pdu.read_bits(1).unwrap(), 0); // ENC_FIELDS_INCL
        assert_eq!(pdu.read_bits(4).unwrap(), 0b1010); // SDU begins after ENC_FIELDS_INCL
    }

    #[test]
    fn f_csch_pd10_writes_integrity_and_extended_encryption_indicators() {
        let mut request = make_paging_request(&[1, 1, 0, 0], MessageId::Order);
        request.mcsb.mobile_p_rev = Some(9);
        request.mcsb.extended_encryption = true;
        request.mcsb.address = Some(MsAddress::Esn(0x1234_5678));

        let mut pdu = utility_assemble_f_csch(&request).unwrap();
        assert_eq!(pdu.read_bits(2).unwrap(), 0b10); // PD
        assert_eq!(pdu.read_bits(6).unwrap(), 0x07); // ORDM MSG_ID
        let _arq = pdu.read_bits(8).unwrap();
        let _addr_type = pdu.read_bits(3).unwrap();
        let _addr_len = pdu.read_bits(4).unwrap();
        let _esn = pdu.read_bits(32).unwrap();
        assert_eq!(pdu.read_bits(1).unwrap(), 0); // MACI_INCL
        assert_eq!(pdu.read_bits(1).unwrap(), 0); // ENC_FIELDS_INCL
        assert_eq!(pdu.read_bits(4).unwrap(), 0b1100); // SDU begins after both indicators
    }

    #[test]
    fn f_csch_fcch_pd00_writes_extended_encryption_indicator() {
        let mut request = make_paging_request(&[1, 0, 1, 1], MessageId::Order);
        request.mcsb.channel = ChannelType::FCcch;
        request.mcsb.address = Some(MsAddress::Esn(0x1234_5678));
        request.mcsb.ack_seq = 5;
        request.mcsb.msg_seq = 3;
        request.mcsb.ack_req = true;
        request.mcsb.valid_ack = false;

        let mut expected = Bitstream::new();
        expected.write_u8(0x07, 8); // PD=00, ORDM MSG_ID
        expected.write_u8(5, 3);
        expected.write_u8(3, 3);
        expected.write_u8(1, 1);
        expected.write_u8(0, 1);
        request.mcsb.address.as_ref().unwrap().write_to(
            &mut expected,
            request.mcsb.overhead_mcc,
            request.mcsb.overhead_imsi_11_12,
        );
        expected.write_u8(0, 1); // ENC_FIELDS_INCL
        expected.extend(&request.sdu);
        pad_8k2(&mut expected);

        let pdu = utility_assemble_f_csch(&request).unwrap();
        assert_eq!(pdu.bits(), expected.bits());
    }

    #[test]
    fn f_csch_rejects_unsupported_channel() {
        let mut request = make_paging_request(&[1, 0, 1, 1], MessageId::Order);
        request.mcsb.channel = ChannelType::RAch;

        let err = utility_assemble_f_csch(&request).unwrap_err();
        assert!(
            err.to_string().contains("unsupported channel"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn paging_fragment_uses_remaining_half_frame_bits_for_next_pdu_start() {
        let (lac_to_mac_tx, _lac_to_mac_rx) = mpsc::channel();
        let (_mac_to_lac_tx, mac_to_lac_rx) = mpsc::channel();
        let lac = Layer2Lac::new(lac_to_mac_tx, mac_to_lac_rx);

        let first_request = make_paging_request(&[1, 0], MessageId::GeneralPage);
        let second_request = make_paging_request(&[0, 1], MessageId::GlobalServiceRedirection);
        let first_encapsulated = Layer2Lac::assemble_pdu(first_request.clone()).unwrap();
        let second_encapsulated = Layer2Lac::assemble_pdu(second_request.clone()).unwrap();
        let second_bits_used = 95usize - first_encapsulated.e_pdu.len();

        let mut scheduled = VecDeque::from([first_request, second_request]);
        lac.set_paging_supplier(Box::new(move |_chip| scheduled.pop_front()));

        let mut queue = VecDeque::new();
        let request = lac
            .build_paging_fragment_request(&mut queue, 96, Utc::now(), 0)
            .unwrap()
            .unwrap();

        let mut expected_bits = Bitstream::new();
        expected_bits.write_u8(1, 1);
        expected_bits.extend(&first_encapsulated.e_pdu);
        expected_bits.extend_n(&second_encapsulated.e_pdu, second_bits_used);

        assert_eq!(request.channel_type, ChannelType::FPch);
        assert_eq!(request.size, 96);
        assert_eq!(request.data.bits(), expected_bits.bits());
        assert_eq!(queue.len(), 1);
        assert!(queue.front().unwrap().frame_start_sent);
        assert_eq!(queue.front().unwrap().e_pdu.len(), 1);
    }

    #[test]
    fn paging_slot_boundary_inserts_gpm_before_queued_overhead() {
        let (lac_to_mac_tx, _lac_to_mac_rx) = mpsc::channel();
        let (_mac_to_lac_tx, mac_to_lac_rx) = mpsc::channel();
        let lac = Layer2Lac::new(lac_to_mac_tx, mac_to_lac_rx);

        let gpm_request = make_paging_request(&[1, 0], MessageId::GeneralPage);
        let overhead_request =
            make_paging_request(&vec![0; 90], MessageId::GlobalServiceRedirection);
        let mut scheduled = VecDeque::from([gpm_request.clone()]);
        lac.set_paging_supplier(Box::new(move |_chip| scheduled.pop_front()));

        let mut queue = VecDeque::from([Layer2Lac::assemble_pdu(overhead_request).unwrap()]);
        let request = lac
            .build_paging_fragment_request(&mut queue, 96, Utc::now(), 0)
            .unwrap()
            .unwrap();

        assert_eq!(request.channel_type, ChannelType::FPch);
        assert_eq!(request.mcsb.message_id, MessageId::GeneralPage);
        assert_eq!(request.data.bits()[0], 1, "slot-leading GPM must be SCI=1");
        assert!(
            queue
                .iter()
                .any(|pdu| pdu.message.mcsb.message_id == MessageId::GlobalServiceRedirection),
            "queued overhead should be preserved after slot-leading GPM",
        );
    }

    #[test]
    fn paging_overhead_that_would_cross_slot_boundary_is_deferred() {
        let (lac_to_mac_tx, _lac_to_mac_rx) = mpsc::channel();
        let (_mac_to_lac_tx, mac_to_lac_rx) = mpsc::channel();
        let lac = Layer2Lac::new(lac_to_mac_tx, mac_to_lac_rx);

        let overhead_request =
            make_paging_request(&vec![1; 90], MessageId::GlobalServiceRedirection);
        let mut queue = VecDeque::from([Layer2Lac::assemble_pdu(overhead_request).unwrap()]);
        assert!(
            queue.front().unwrap().e_pdu.len() > FPCH_HALF_FRAME_PAYLOAD_BITS_9600,
            "test overhead must not fit in one remaining half-frame",
        );

        let last_half_frame_in_slot = FPCH_SLOT_CHIPS - FPCH_HALF_FRAME_CHIPS;
        let request = lac
            .build_paging_fragment_request(&mut queue, 96, Utc::now(), last_half_frame_in_slot)
            .unwrap();

        assert!(
            request.is_none(),
            "overhead should not start when it would continue into the next slot",
        );
        assert_eq!(queue.len(), 1);
        assert!(
            !queue.front().unwrap().frame_start_sent,
            "deferred overhead must remain unstarted",
        );
    }

    #[test]
    fn deferred_overhead_resumes_after_next_slot_gpm() {
        let (lac_to_mac_tx, _lac_to_mac_rx) = mpsc::channel();
        let (_mac_to_lac_tx, mac_to_lac_rx) = mpsc::channel();
        let lac = Layer2Lac::new(lac_to_mac_tx, mac_to_lac_rx);

        let gpm_request = make_paging_request(&[1, 0], MessageId::GeneralPage);
        let overhead_request =
            make_paging_request(&vec![1; 90], MessageId::GlobalServiceRedirection);
        let mut queue = VecDeque::from([Layer2Lac::assemble_pdu(overhead_request).unwrap()]);

        let last_half_frame_in_slot = FPCH_SLOT_CHIPS - FPCH_HALF_FRAME_CHIPS;
        let deferred = lac
            .build_paging_fragment_request(&mut queue, 96, Utc::now(), last_half_frame_in_slot)
            .unwrap();
        assert!(deferred.is_none());

        let mut scheduled = VecDeque::from([gpm_request]);
        lac.set_paging_supplier(Box::new(move |_chip| scheduled.pop_front()));

        let request = lac
            .build_paging_frame_request(&mut queue, 192, Utc::now(), FPCH_SLOT_CHIPS)
            .unwrap()
            .unwrap();

        assert_eq!(
            request.mcsb.message_id,
            MessageId::GeneralPage,
            "next slot must start with GPM before deferred overhead resumes",
        );
        assert!(
            queue.iter().all(|pdu| {
                pdu.message.mcsb.message_id != MessageId::GlobalServiceRedirection
                    || pdu.frame_start_sent
            }),
            "deferred overhead must not remain queued and unstarted after the slot-leading GPM",
        );
    }

    #[test]
    fn overhead_is_not_started_if_it_would_overflow_next_slot() {
        let (lac_to_mac_tx, _lac_to_mac_rx) = mpsc::channel();
        let (_mac_to_lac_tx, mac_to_lac_rx) = mpsc::channel();
        let lac = Layer2Lac::new(lac_to_mac_tx, mac_to_lac_rx);

        let first_request = make_paging_request(&[1, 0], MessageId::GlobalServiceRedirection);
        let first_encapsulated = Layer2Lac::assemble_pdu(first_request.clone()).unwrap();
        let remaining_after_first =
            FPCH_HALF_FRAME_PAYLOAD_BITS_9600 - first_encapsulated.e_pdu.len();
        let overflow_bits = remaining_after_first + FPCH_HALF_FRAME_PAYLOAD_BITS_9600 + 1;
        let overflow_request =
            make_paging_request(&vec![1; overflow_bits], MessageId::GlobalServiceRedirection);
        let overflow_encapsulated = Layer2Lac::assemble_pdu(overflow_request.clone()).unwrap();
        assert!(
            overflow_encapsulated.e_pdu.len()
                > remaining_after_first + FPCH_HALF_FRAME_PAYLOAD_BITS_9600,
            "test overhead must overflow the next slot after prior packed overhead",
        );

        let mut scheduled = VecDeque::from([first_request, overflow_request]);
        lac.set_paging_supplier(Box::new(move |_chip| scheduled.pop_front()));

        let chip_one_half_before_slot = FPCH_SLOT_CHIPS - FPCH_HALF_FRAME_CHIPS;
        let mut queue = VecDeque::new();
        let request = lac
            .build_paging_fragment_request(&mut queue, 96, Utc::now(), chip_one_half_before_slot)
            .unwrap()
            .unwrap();

        assert_eq!(request.mcsb.message_id, MessageId::GlobalServiceRedirection);
        assert!(
            queue.front().is_some_and(|pdu| !pdu.frame_start_sent
                && pdu.message.mcsb.message_id == MessageId::GlobalServiceRedirection),
            "overflowing overhead must remain queued and unstarted",
        );
    }

    #[test]
    fn stock_paging_supplier_gpm_starts_each_slot() {
        let (lac_to_mac_tx, _lac_to_mac_rx) = mpsc::channel();
        let (_mac_to_lac_tx, mac_to_lac_rx) = mpsc::channel();
        let lac = Layer2Lac::new(lac_to_mac_tx, mac_to_lac_rx);

        let overhead = OverheadParameters::default();
        let paging = PagingChannelSettings::default();
        let paging_state = Arc::new(Mutex::new(PagingSupplierState::new(0x03ff, 0x7f)));
        lac.set_paging_supplier(build_bts_paging_supplier(overhead, paging, 0, paging_state));

        let general_page_type = MessageId::GeneralPage
            .wire_type(WireChannel::ForwardCommon)
            .unwrap();
        let mut queue = VecDeque::new();
        let mut reader = PagingFrameReader::new_with_rate(PagingChannelRate::Rate9600);
        let mut message_start_chip = None::<u64>;
        let mut gpm_starts = Vec::new();

        for frame_idx in 0..220u64 {
            let chip = frame_idx * FPCH_HALF_FRAME_CHIPS * 2;
            let Some(request) = lac
                .build_paging_frame_request(&mut queue, 192, Utc::now(), chip)
                .unwrap()
            else {
                continue;
            };

            for (half_idx, chunk) in request.data.bits().chunks_exact(96).enumerate() {
                let half_chip = chip + (half_idx as u64 * FPCH_HALF_FRAME_CHIPS);
                if chunk.first() == Some(&1) {
                    message_start_chip = Some(half_chip);
                }
                let mut half_frame = Bitstream::new_init(chunk);
                let mut frames = Vec::new();
                if let Some(frame) = reader.process(&mut half_frame).unwrap() {
                    frames.push(frame);
                }
                while let Some(frame) = reader.take_completed_frame() {
                    frames.push(frame);
                }
                for (completed_idx, frame) in frames.into_iter().enumerate() {
                    if !frame.crc_valid {
                        continue;
                    }
                    let mut payload = frame.data.clone();
                    let msg_type = payload.read_bits(8).unwrap() as u8;
                    if msg_type == general_page_type {
                        let start_chip = if completed_idx == 0 {
                            message_start_chip.unwrap_or(half_chip)
                        } else {
                            half_chip
                        };
                        gpm_starts.push(start_chip);
                    }
                    message_start_chip = None;
                }
            }
        }

        assert!(
            gpm_starts.len() >= 50,
            "expected one GPM per 80 ms slot, got starts={:?}",
            gpm_starts,
        );
        assert!(
            gpm_starts.iter().all(|chip| chip % FPCH_SLOT_CHIPS == 0),
            "GPM starts must align to 80 ms slots: {:?}",
            gpm_starts,
        );
    }

    #[test]
    fn stock_paging_supplier_injects_page_record_in_assigned_slot_gpm() {
        let (lac_to_mac_tx, _lac_to_mac_rx) = mpsc::channel();
        let (_mac_to_lac_tx, mac_to_lac_rx) = mpsc::channel();
        let lac = Layer2Lac::new(lac_to_mac_tx, mac_to_lac_rx);

        let target_slot_num = 11u16;
        let (imsi_m_s1, imsi_m_s2) = (0..20_000u32)
            .find_map(|s1| {
                let s2 = 123u16;
                (cdma_common::paging::compute_pgslot(s1, s2) == target_slot_num).then_some((s1, s2))
            })
            .expect("test IMSI search should find a matching PGSLOT");
        let imsi_s = ((imsi_m_s2 as u64) << 24) | imsi_m_s1 as u64;
        let page_record = GeneralPageRecord::Class0 {
            page_subclass: 0,
            msg_seq: 5,
            imsi_s: Some(imsi_s),
            imsi_11_12: None,
            mcc: None,
            imsi_addr_num: None,
            imsi_m_s1: Some(imsi_m_s1),
            imsi_m_s2: Some(imsi_m_s2),
            special_service: false,
            service_option: None,
        };
        let page_address = MsPageAddress::ImsiS {
            imsi_m_s1,
            imsi_m_s2,
            mcc: None,
            imsi_11_12: None,
        };

        let overhead = OverheadParameters::default();
        let paging = PagingChannelSettings::default();
        let paging_state = Arc::new(Mutex::new(PagingSupplierState::new(0x03ff, 0x7f)));
        paging_state
            .lock()
            .pending_page_records
            .push(PendingPageRecord::new(page_record.clone(), page_address));
        lac.set_paging_supplier(build_bts_paging_supplier(
            overhead,
            paging,
            0,
            paging_state.clone(),
        ));

        let general_page_type = MessageId::GeneralPage
            .wire_type(WireChannel::ForwardCommon)
            .unwrap();
        let mut queue = VecDeque::new();
        let mut reader = PagingFrameReader::new_with_rate(PagingChannelRate::Rate9600);
        let mut message_start_chip = None::<u64>;
        let mut decoded_gpms = Vec::<(u64, GeneralPageMessage)>::new();

        for frame_idx in 0..260u64 {
            let chip = frame_idx * FPCH_HALF_FRAME_CHIPS * 2;
            let Some(request) = lac
                .build_paging_frame_request(&mut queue, 192, Utc::now(), chip)
                .unwrap()
            else {
                continue;
            };

            for (half_idx, chunk) in request.data.bits().chunks_exact(96).enumerate() {
                let half_chip = chip + (half_idx as u64 * FPCH_HALF_FRAME_CHIPS);
                if chunk.first() == Some(&1) {
                    message_start_chip = Some(half_chip);
                }
                let mut half_frame = Bitstream::new_init(chunk);
                let mut frames = Vec::new();
                if let Some(frame) = reader.process(&mut half_frame).unwrap() {
                    frames.push(frame);
                }
                while let Some(frame) = reader.take_completed_frame() {
                    frames.push(frame);
                }

                for (completed_idx, frame) in frames.into_iter().enumerate() {
                    if !frame.crc_valid {
                        continue;
                    }
                    let start_chip = if completed_idx == 0 {
                        message_start_chip.unwrap_or(half_chip)
                    } else {
                        half_chip
                    };
                    let mut payload = frame.data.clone();
                    let msg_type = payload.read_bits(8).unwrap() as u8;
                    if msg_type == general_page_type {
                        decoded_gpms.push((
                            start_chip,
                            GeneralPageMessage::from_sdu(&mut payload).unwrap(),
                        ));
                    }
                    message_start_chip = None;
                }
            }
        }

        let expected_chip = target_slot_num as u64 * FPCH_SLOT_CHIPS;
        let matching = decoded_gpms
            .iter()
            .filter(|(_, gpm)| gpm.page_records.contains(&page_record))
            .collect::<Vec<_>>();

        assert_eq!(
            matching.len(),
            4,
            "page record must be injected into four assigned-slot GPMs: {:?}",
            decoded_gpms
                .iter()
                .map(|(chip, gpm)| (*chip, gpm.page_records.clone()))
                .collect::<Vec<_>>(),
        );
        assert!(
            decoded_gpms.iter().all(|(_, gpm)| gpm.class_0_done),
            "CLASS_0_DONE is slot-local; a future-slot Class 0 page must not hold it low: {:?}",
            decoded_gpms
                .iter()
                .map(|(chip, gpm)| (*chip, gpm.class_0_done, gpm.page_records.clone()))
                .collect::<Vec<_>>(),
        );
        let expected_chips = (0..4)
            .map(|attempt| expected_chip + attempt * 16 * FPCH_SLOT_CHIPS)
            .collect::<Vec<_>>();
        assert_eq!(
            matching.iter().map(|(chip, _)| *chip).collect::<Vec<_>>(),
            expected_chips,
            "page record GPMs must start in assigned slots",
        );
        assert!(
            decoded_gpms
                .iter()
                .all(|(chip, _)| chip % FPCH_SLOT_CHIPS == 0),
            "all decoded GPMs should still start at slot boundaries",
        );
        assert!(
            paging_state.lock().pending_page_records.is_empty(),
            "page record should be consumed after four assigned-slot GPMs",
        );
    }

    #[test]
    fn paging_frame_availability_keeps_long_pdu_contiguous_across_half_frames() {
        let (lac_to_mac_tx, _lac_to_mac_rx) = mpsc::channel();
        let (_mac_to_lac_tx, mac_to_lac_rx) = mpsc::channel();
        let lac = Layer2Lac::new(lac_to_mac_tx, mac_to_lac_rx);

        let sdu = vec![1; 90];
        let request = make_paging_request(&sdu, MessageId::Order);
        let encapsulated = Layer2Lac::assemble_pdu(request).unwrap();
        assert!(encapsulated.e_pdu.len() > 95);
        assert!(encapsulated.e_pdu.len() < 190);
        let encapsulated_bits = encapsulated.e_pdu.bits().to_vec();

        let mut queue = VecDeque::from([encapsulated]);
        let request = lac
            .build_paging_frame_request(&mut queue, 192, Utc::now(), 0)
            .unwrap()
            .unwrap();

        let mut expected_bits = Bitstream::new();
        expected_bits.write_u8(1, 1);
        expected_bits.extend(&Bitstream::new_init(&encapsulated_bits[..95]));
        expected_bits.write_u8(0, 1);
        expected_bits.extend(&Bitstream::new_init(&encapsulated_bits[95..]));
        if expected_bits.len() < 192 {
            expected_bits.write_u8(0, 192 - expected_bits.len());
        }

        assert_eq!(request.channel_type, ChannelType::FPch);
        assert_eq!(request.size, 192);
        assert_eq!(request.data.bits(), expected_bits.bits());
        assert!(queue.is_empty());
    }

    #[test]
    fn traffic_msg_seq_tracker_uses_full_three_bit_space() {
        let mut tracker = MsgSeqTracker::new(ChannelType::FTch);
        let addr = MsAddress::Esn(0x1234_5678);

        let seqs = (0..9)
            .map(|_| tracker.next_seq(&addr, true).unwrap())
            .collect::<Vec<_>>();

        assert_eq!(seqs, vec![0, 1, 2, 3, 4, 5, 6, 7, 0]);
    }

    #[test]
    fn f_csch_msg_seq_tracker_separates_address_and_ack_streams() {
        let mut tracker = MsgSeqTracker::new(ChannelType::FPch);
        let addresses = [
            MsAddress::Esn(0x1234_5678),
            MsAddress::ImsiS {
                imsi_m_s1: 0x11_22_33,
                imsi_m_s2: 0x155,
            },
            MsAddress::ImsiClass0 {
                imsi_m_s1: 0x22_33_44,
                imsi_m_s2: 0x2aa,
                mcc: 310,
                imsi_11_12: 45,
            },
        ];

        for addr in addresses {
            assert_eq!(tracker.next_seq(&addr, false).unwrap(), 0);
            assert_eq!(tracker.next_seq(&addr, true).unwrap(), 0);
            assert_eq!(tracker.next_seq(&addr, false).unwrap(), 1);
            assert_eq!(tracker.next_seq(&addr, true).unwrap(), 1);
        }
    }

    #[test]
    fn f_csch_msg_seq_tracker_errors_when_t4m_space_is_exhausted() {
        let mut tracker = MsgSeqTracker::new(ChannelType::FPch);
        let addr = MsAddress::Esn(0x1234_5678);

        let seqs = (0..8)
            .map(|_| {
                let seq = tracker.next_seq(&addr, true).unwrap();
                tracker.mark_transmitted(&addr, true, seq).unwrap();
                seq
            })
            .collect::<Vec<_>>();
        assert_eq!(seqs, vec![0, 1, 2, 3, 4, 5, 6, 7]);

        let err = tracker.next_seq(&addr, true).unwrap_err();
        assert!(
            err.to_string().contains("MSG_SEQ space exhausted"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn f_csch_msg_seq_reservation_does_not_start_t4m_cooldown() {
        let mut tracker = MsgSeqTracker::new(ChannelType::FPch);
        let addr = MsAddress::Esn(0x1234_5678);

        let seqs = (0..9)
            .map(|_| tracker.next_seq(&addr, true).unwrap())
            .collect::<Vec<_>>();

        assert_eq!(seqs, vec![0, 1, 2, 3, 4, 5, 6, 7, 0]);
    }

    #[test]
    fn f_csch_msg_seq_retransmission_refreshes_t4m_cooldown() {
        let mut tracker = MsgSeqTracker::new(ChannelType::FPch);
        let addr = MsAddress::Esn(0x1234_5678);
        let now = Instant::now();
        let expired = now - T4M_DURATION - Duration::from_millis(1);

        tracker
            .mark_transmitted_at(&addr, true, 0, expired)
            .unwrap();
        for seq in 1..8 {
            tracker.mark_transmitted_at(&addr, true, seq, now).unwrap();
        }
        assert_eq!(tracker.next_seq(&addr, true).unwrap(), 0);

        tracker.mark_transmitted_at(&addr, true, 0, now).unwrap();
        let err = tracker.next_seq(&addr, true).unwrap_err();
        assert!(
            err.to_string().contains("MSG_SEQ space exhausted"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn paging_fragment_start_marks_directed_pdu_transmitted() {
        let (lac_to_mac_tx, _lac_to_mac_rx) = mpsc::channel();
        let (_mac_to_lac_tx, mac_to_lac_rx) = mpsc::channel();
        let lac = Layer2Lac::new(lac_to_mac_tx, mac_to_lac_rx);
        let addr = MsAddress::Esn(0x1234_5678);

        let make_req = |sdu: Vec<u8>| DirectedSduRequest {
            sdu: Bitstream::new_init(&sdu),
            channel: ChannelType::FPch,
            address: addr.clone(),
            message_id: MessageId::Order,
            ack_seq: 6,
            ack_req: true,
            valid_ack: true,
            requested_tx_time: None,
            tx_deadline: None,
            overhead_mcc: 310,
            overhead_imsi_11_12: 45,
        };

        let handle = lac
            .send_directed_sdu(make_req(vec![1, 0, 1, 0, 1, 1]))
            .unwrap();
        assert_eq!(handle.msg_seq, 0);
        {
            let state = lac.state.lock();
            let entry = state.pending_directed_pdus.entries.first().unwrap();
            assert_eq!(entry.first_tx_at, None);
            assert_eq!(entry.last_tx_at, None);
        }

        let mut queue = {
            let mut state = lac.state.lock();
            state.message_queue.remove(&ChannelType::FPch).unwrap()
        };
        lac.build_paging_fragment_request(&mut queue, 96, Utc::now(), 0)
            .unwrap()
            .unwrap();
        {
            let state = lac.state.lock();
            let entry = state.pending_directed_pdus.entries.first().unwrap();
            assert!(entry.first_tx_at.is_some());
            assert!(entry.last_tx_at.is_some());
        }

        for idx in 1u8..8 {
            let handle = lac
                .send_directed_sdu(make_req(vec![idx & 1, (idx >> 1) & 1, (idx >> 2) & 1, 1]))
                .unwrap();
            assert_eq!(handle.msg_seq, idx);
        }

        let err = match lac.send_directed_sdu(make_req(vec![1, 1, 1, 1, 1])) {
            Ok(handle) => panic!("expected MSG_SEQ exhaustion, got {}", handle.msg_seq),
            Err(err) => err,
        };
        assert!(
            err.to_string().contains("MSG_SEQ space exhausted"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn paging_fragment_prioritizes_directed_deadline_pdu() {
        let (lac_to_mac_tx, _lac_to_mac_rx) = mpsc::channel();
        let (_mac_to_lac_tx, mac_to_lac_rx) = mpsc::channel();
        let lac = Layer2Lac::new(lac_to_mac_tx, mac_to_lac_rx);
        let now = Utc::now();

        let broadcast = DataRequest {
            sdu: Bitstream::new_init(&[1, 0, 1, 0]),
            mcsb: MessageControlStatusBlock {
                channel: ChannelType::FPch,
                mobile_p_rev: None,
                extended_encryption: false,
                message_id: MessageId::GeneralPage,
                length_bits: 4,
                requested_tx_time: None,
                tx_deadline: None,
                address: None,
                ack_seq: 0,
                msg_seq: 0,
                ack_req: false,
                valid_ack: false,
                overhead_mcc: 0x03ff,
                overhead_imsi_11_12: 0x7f,
            },
        };
        let directed = DataRequest {
            sdu: Bitstream::new_init(&[0, 1, 0, 1]),
            mcsb: MessageControlStatusBlock {
                channel: ChannelType::FPch,
                mobile_p_rev: None,
                extended_encryption: false,
                message_id: MessageId::Order,
                length_bits: 4,
                requested_tx_time: Some(now),
                tx_deadline: Some(now),
                address: Some(MsAddress::Esn(0x1234_5678)),
                ack_seq: 6,
                msg_seq: 2,
                ack_req: false,
                valid_ack: true,
                overhead_mcc: 0x03ff,
                overhead_imsi_11_12: 0x7f,
            },
        };

        let mut queue = VecDeque::from([
            Layer2Lac::assemble_pdu(broadcast).unwrap(),
            Layer2Lac::assemble_pdu(directed).unwrap(),
        ]);
        let request = lac
            .build_paging_fragment_request(&mut queue, 96, now, FPCH_HALF_FRAME_CHIPS)
            .unwrap()
            .unwrap();

        assert_eq!(request.mcsb.message_id, MessageId::Order);
        assert!(request.mcsb.address.is_some());
    }

    #[test]
    fn cancel_future_general_pages_removes_only_future_timed_gpm() {
        use super::{
            LacMessage,
            paging_messages::{GeneralPageMessage, GeneralPageRecord, PagingChannelMessage},
        };

        let (lac_to_mac_tx, _lac_to_mac_rx) = mpsc::channel();
        let (_mac_to_lac_tx, mac_to_lac_rx) = mpsc::channel();
        let lac = Layer2Lac::new(lac_to_mac_tx, mac_to_lac_rx);
        let now = Utc::now();

        let mut future_gpm = PagingChannelMessage::GeneralPage(GeneralPageMessage {
            config_msg_seq: 1,
            acc_msg_seq: 2,
            class_0_done: false,
            class_1_done: false,
            tmsi_done: false,
            ordered_tmsis: false,
            broadcast_done: false,
            reserved: 0,
            add_pfield: Vec::new(),
            page_records: vec![GeneralPageRecord::Class1 {
                msg_seq: 1,
                esn: 0x1234_5678,
                special_service: false,
                service_option: None,
            }],
        })
        .to_data_request();
        future_gpm.mcsb.requested_tx_time = Some(now + chrono::Duration::seconds(10));

        let immediate_order = DataRequest {
            sdu: Bitstream::new_init(&[1, 0, 1, 0]),
            mcsb: MessageControlStatusBlock {
                channel: ChannelType::FPch,
                mobile_p_rev: None,
                extended_encryption: false,
                message_id: MessageId::Order,
                length_bits: 4,
                requested_tx_time: None,
                tx_deadline: None,
                address: Some(MsAddress::Esn(0x1234_5678)),
                ack_seq: 3,
                msg_seq: 1,
                ack_req: false,
                valid_ack: true,
                overhead_mcc: 0x03ff,
                overhead_imsi_11_12: 0x7f,
            },
        };

        lac.send_message(LacMessage::DataRequest(future_gpm))
            .unwrap();
        lac.send_message(LacMessage::DataRequest(immediate_order))
            .unwrap();

        let removed = lac.cancel_future_general_pages();
        assert_eq!(removed, 1);

        let state = lac.state.lock();
        let queue = state.message_queue.get(&ChannelType::FPch).unwrap();
        assert_eq!(queue.len(), 1);
        assert_eq!(
            queue.front().unwrap().message.mcsb.message_id,
            MessageId::Order
        );
    }

    #[test]
    fn cancel_future_general_pages_does_not_arm_when_queue_not_checked_out() {
        let (lac_to_mac_tx, _lac_to_mac_rx) = mpsc::channel();
        let (_mac_to_lac_tx, mac_to_lac_rx) = mpsc::channel();
        let lac = Layer2Lac::new(lac_to_mac_tx, mac_to_lac_rx);

        let removed = lac.cancel_future_general_pages();
        assert_eq!(removed, 0);

        let state = lac.state.lock();
        assert!(!state.cancel_future_general_pages);
        assert!(!state.fpch_queue_checked_out);
    }

    #[test]
    fn pending_directed_pdu_reuses_msg_seq_for_same_pdu() {
        use super::PendingDirectedPduTracker;

        let addr = MsAddress::Esn(0x1234_5678);
        let mut msg_seq_tracker = MsgSeqTracker::new(ChannelType::FPch);
        let mut pending = PendingDirectedPduTracker::default();
        let sdu = Bitstream::new_init(&[1, 0, 1, 0, 1, 1]);

        let first = pending
            .reserve_msg_seq(
                &mut msg_seq_tracker,
                &addr,
                MessageId::Order,
                &sdu,
                6,
                false,
                true,
            )
            .unwrap();
        let second = pending
            .reserve_msg_seq(
                &mut msg_seq_tracker,
                &addr,
                MessageId::Order,
                &sdu,
                6,
                false,
                true,
            )
            .unwrap();

        assert_eq!(first, second);

        let different_sdu = Bitstream::new_init(&[1, 1, 0, 0, 1, 0]);
        let third = pending
            .reserve_msg_seq(
                &mut msg_seq_tracker,
                &addr,
                MessageId::Order,
                &different_sdu,
                6,
                false,
                true,
            )
            .unwrap();

        assert_ne!(first, third);
        assert!(pending.acknowledge(&addr, first));

        let fourth = pending
            .reserve_msg_seq(
                &mut msg_seq_tracker,
                &addr,
                MessageId::Order,
                &sdu,
                6,
                false,
                true,
            )
            .unwrap();
        assert_ne!(first, fourth);
    }

    #[test]
    fn pending_directed_pdu_blocks_reserved_seq_before_transmission() {
        use super::PendingDirectedPduTracker;

        let addr = MsAddress::Esn(0x1234_5678);
        let mut msg_seq_tracker = MsgSeqTracker::new(ChannelType::FPch);
        let mut pending = PendingDirectedPduTracker::default();

        let seqs = (0..8)
            .map(|idx| {
                let sdu = Bitstream::new_init(&[idx & 1, (idx >> 1) & 1, (idx >> 2) & 1]);
                pending
                    .reserve_msg_seq(
                        &mut msg_seq_tracker,
                        &addr,
                        MessageId::Order,
                        &sdu,
                        6,
                        false,
                        true,
                    )
                    .unwrap()
            })
            .collect::<Vec<_>>();
        assert_eq!(seqs, vec![0, 1, 2, 3, 4, 5, 6, 7]);

        let sdu = Bitstream::new_init(&[1, 1, 1, 1]);
        let err = pending
            .reserve_msg_seq(
                &mut msg_seq_tracker,
                &addr,
                MessageId::Order,
                &sdu,
                6,
                false,
                true,
            )
            .unwrap_err();
        assert!(
            err.to_string().contains("MSG_SEQ space exhausted"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn msg_seq_streams_are_independent_for_ack_req_classes() {
        use super::PendingDirectedPduTracker;

        // Per C.S0004-E 3.1.2.1.1.2: separate MSG_SEQ streams for
        // assured (ack_req=true) and unassured (ack_req=false) PDUs.
        // Both streams start at 0 independently.
        let addr = MsAddress::Esn(0x1234_5678);
        let mut msg_seq_tracker = MsgSeqTracker::new(ChannelType::FPch);
        let mut pending = PendingDirectedPduTracker::default();
        let sdu_a = Bitstream::new_init(&[1, 0, 1, 0, 1, 1]);
        let sdu_b = Bitstream::new_init(&[0, 1, 0, 1, 0, 0]);

        // First unassured PDU
        let noack_0 = pending
            .reserve_msg_seq(
                &mut msg_seq_tracker,
                &addr,
                MessageId::Order,
                &sdu_a,
                4,
                false,
                true,
            )
            .unwrap();
        // First assured PDU — should also start at 0 (independent stream)
        let ack_0 = pending
            .reserve_msg_seq(
                &mut msg_seq_tracker,
                &addr,
                MessageId::Order,
                &sdu_a,
                4,
                true,
                true,
            )
            .unwrap();
        assert_eq!(noack_0, 0, "first unassured MSG_SEQ should be 0");
        assert_eq!(ack_0, 0, "first assured MSG_SEQ should be 0");

        // Second unassured PDU (different content)
        std::thread::sleep(std::time::Duration::from_millis(2_300)); // wait for T4m
        let noack_1 = pending
            .reserve_msg_seq(
                &mut msg_seq_tracker,
                &addr,
                MessageId::Order,
                &sdu_b,
                4,
                false,
                true,
            )
            .unwrap();
        // Second assured PDU (different content)
        let ack_1 = pending
            .reserve_msg_seq(
                &mut msg_seq_tracker,
                &addr,
                MessageId::Order,
                &sdu_b,
                4,
                true,
                true,
            )
            .unwrap();
        assert_eq!(noack_1, 1, "second unassured MSG_SEQ should be 1");
        assert_eq!(ack_1, 1, "second assured MSG_SEQ should be 1");
    }
}
