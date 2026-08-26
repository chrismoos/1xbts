/// RLP Type 1 session state machine per IS-707-A.2 Section 3.1.
///
/// Implements the non-encrypted mode initialization/reset (3.1.1.1) and
/// data transfer (3.1.2) procedures for the BS/MSC side.
///
/// Non-encrypted mode only.
use std::collections::VecDeque;

use crate::rlp::{self, ControlType, RlpFrame, SegmentType};

/// RLP session state per IS-707-A.2 Section 3.1.1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RlpState {
    /// Not yet initialized. Will begin sending SYNC on first poll.
    Uninitialized,
    /// Sending SYNC frames, waiting for SYNC/ACK.
    Sync,
    /// Received SYNC from peer, sending SYNC/ACK, waiting for non-SYNC frame.
    SyncAck,
    /// Received SYNC/ACK from peer, sending ACK, waiting for non-SYNC/ACK frame.
    Ack,
    /// RLP link established, transferring data.
    DataTransfer,
}

/// Output action from the RLP session for one frame period.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RlpOutput {
    /// Send this frame to the peer (encode and transmit on traffic channel).
    SendFrame(RlpFrame),
    /// No frame to send this period (should not happen for primary traffic;
    /// the caller should send an idle frame if needed).
    Nothing,
}

/// Uplink data delivered by RLP to the upper layer (PPP).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RlpDelivery {
    /// Data octets extracted from RLP data frames, in order.
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SegmentAssembly {
    seq: u8,
    data: Vec<u8>,
    expect_second_or_last: bool,
}

/// RLP Type 1 session state machine (BS/MSC side).
///
/// Frame-synchronous: the caller drives the session by calling `receive_frame()`
/// for each received uplink frame and `next_frame()` to get the next downlink frame.
pub struct RlpSession {
    state: RlpState,

    /// V(S): sequence number of the next data frame to be transmitted.
    v_s: u8,
    /// V(R): expected sequence number of the next data frame to be received.
    v_r: u8,
    /// V(N): sequence number of the next needed frame for sequential delivery.
    v_n: u8,

    /// Round-trip frame counter for SYNC handshake (>= 4 required per spec).
    round_trip_counter: u32,
    /// RLP_DELAY_s measured during handshake (in frame counts).
    rlp_delay: u32,
    /// Counter of frames sent in current handshake phase.
    handshake_frames_sent: u32,

    /// Consecutive erasure count (3.1.3). Reset on valid frame.
    consecutive_erasures: u32,

    /// Outgoing data queue: bytes from upper layer waiting to be sent.
    tx_queue: Vec<u8>,

    /// Received data delivered to upper layer (accumulated per frame period).
    rx_buffer: Vec<u8>,

    /// Resequencing buffer for out-of-order frames (indexed by SEQ).
    /// Each slot holds the data octets for that SEQ, or None if not yet received.
    reseq_buffer: Vec<Option<Vec<u8>>>,

    /// NAK requests waiting to be transmitted as control frames.
    pending_controls: VecDeque<RlpFrame>,

    /// Retransmissions requested by received NAKs.
    pending_retransmissions: VecDeque<RlpFrame>,

    /// Copies of recently transmitted data frames, indexed by 8-bit SEQ.
    tx_history: Vec<Option<RlpFrame>>,

    /// Missing received SEQs already requested by a NAK.
    nak_outstanding: Vec<bool>,

    /// In-progress reassembly for a segmented retransmission.
    segment_assembly: Option<SegmentAssembly>,
}

impl RlpSession {
    /// Create a new RLP session in Uninitialized state.
    pub fn new() -> Self {
        Self {
            state: RlpState::Uninitialized,
            v_s: 0,
            v_r: 0,
            v_n: 0,
            round_trip_counter: 0,
            rlp_delay: 0,
            handshake_frames_sent: 0,
            consecutive_erasures: 0,
            tx_queue: Vec::new(),
            rx_buffer: Vec::new(),
            reseq_buffer: vec![None; 256],
            pending_controls: VecDeque::new(),
            pending_retransmissions: VecDeque::new(),
            tx_history: vec![None; 256],
            nak_outstanding: vec![false; 256],
            segment_assembly: None,
        }
    }

    pub fn state(&self) -> RlpState {
        self.state
    }

    pub fn v_s(&self) -> u8 {
        self.v_s
    }

    pub fn v_r(&self) -> u8 {
        self.v_r
    }

    pub fn v_n(&self) -> u8 {
        self.v_n
    }

    pub fn rlp_delay(&self) -> u32 {
        self.rlp_delay
    }

    /// Queue data bytes from the upper layer (PPP) for transmission via RLP.
    pub fn enqueue_data(&mut self, data: &[u8]) {
        self.tx_queue.extend_from_slice(data);
    }

    pub fn tx_queue_len(&self) -> usize {
        self.tx_queue.len()
    }

    /// Take any data bytes received from the peer and delivered to the upper layer.
    /// Clears the internal receive buffer.
    pub fn take_received_data(&mut self) -> Vec<u8> {
        std::mem::take(&mut self.rx_buffer)
    }

    /// Initialize/reset the RLP session (3.1.1.1).
    ///
    /// Called on startup or when a reset condition is detected.
    pub fn initialize(&mut self) {
        self.v_s = 0;
        self.v_r = 0;
        self.v_n = 0;
        self.round_trip_counter = 0;
        self.rlp_delay = 0;
        self.handshake_frames_sent = 0;
        self.consecutive_erasures = 0;
        self.tx_queue.clear();
        self.rx_buffer.clear();
        self.pending_controls.clear();
        self.pending_retransmissions.clear();
        self.segment_assembly = None;
        for slot in self.reseq_buffer.iter_mut() {
            *slot = None;
        }
        for slot in self.tx_history.iter_mut() {
            *slot = None;
        }
        for nak in self.nak_outstanding.iter_mut() {
            *nak = false;
        }
        self.state = RlpState::Sync;
    }

    /// Process a received frame from the peer (uplink from mobile).
    ///
    /// `frame` is `None` for an erasure (bad CRC or no frame received).
    /// Returns any data delivered to the upper layer.
    pub fn receive_frame(&mut self, frame: Option<&RlpFrame>) -> Option<RlpDelivery> {
        if self.state == RlpState::Uninitialized {
            self.initialize();
        }

        let frame = match frame {
            Some(f) => f,
            None => {
                self.consecutive_erasures += 1;
                if self.consecutive_erasures > 127 {
                    self.initialize();
                }
                return None;
            }
        };

        // Valid frame resets erasure counter
        self.consecutive_erasures = 0;

        match self.state {
            RlpState::Uninitialized => unreachable!(),

            RlpState::Sync => {
                // We are sending SYNC, waiting for SYNC/ACK or SYNC from peer.
                match frame {
                    RlpFrame::Control {
                        control_type: ControlType::Sync,
                        ..
                    } => {
                        // Peer also syncing — respond with SYNC/ACK
                        log::info!("RLP: received SYNC from peer, sending SYNC/ACK");
                        self.round_trip_counter = 4; // set >= 4 per spec
                        self.handshake_frames_sent = 0;
                        self.state = RlpState::SyncAck;
                    }
                    RlpFrame::Control {
                        control_type: ControlType::SyncAck,
                        ..
                    } => {
                        // Peer acknowledged our SYNC — respond with ACK
                        log::info!("RLP: received SYNC/ACK from peer, sending ACK");
                        self.measure_delay();
                        self.round_trip_counter = 4;
                        self.handshake_frames_sent = 0;
                        self.state = RlpState::Ack;
                    }
                    _ => {
                        // Ignore other frames during sync
                    }
                }
                None
            }

            RlpState::SyncAck => {
                // We are sending SYNC/ACK, waiting for ACK or non-SYNC frame.
                match frame {
                    RlpFrame::Control {
                        control_type: ControlType::Sync,
                        ..
                    } => {
                        // Peer still syncing, keep sending SYNC/ACK
                        self.handshake_frames_sent = 0;
                    }
                    RlpFrame::Control {
                        control_type: ControlType::Ack,
                        ..
                    }
                    | RlpFrame::Control {
                        control_type: ControlType::SyncAck,
                        ..
                    } => {
                        self.measure_delay();
                        self.enter_data_transfer();
                    }
                    _ if !is_sync_frame(frame) => {
                        // Any valid non-SYNC frame means we can enter data transfer
                        self.enter_data_transfer();
                        return self.process_data_frame(frame);
                    }
                    _ => {}
                }
                None
            }

            RlpState::Ack => {
                // We are sending ACK, waiting for non-SYNC/ACK frame.
                match frame {
                    RlpFrame::Control {
                        control_type: ControlType::SyncAck,
                        ..
                    } => {
                        // Peer still sending SYNC/ACK, keep sending ACK
                        self.handshake_frames_sent = 0;
                    }
                    RlpFrame::Control {
                        control_type: ControlType::Sync,
                        ..
                    } => {
                        // Peer reset — re-initialize
                        self.initialize();
                    }
                    _ if !is_sync_ack_frame(frame) => {
                        // Valid non-control frame or ACK — enter data transfer
                        self.enter_data_transfer();
                        return self.process_data_frame(frame);
                    }
                    _ => {}
                }
                None
            }

            RlpState::DataTransfer => {
                // Check for SYNC (reset condition per 3.1.1)
                if is_sync_frame(frame) {
                    self.initialize();
                    return None;
                }
                self.process_data_frame(frame)
            }
        }
    }

    /// Get the next frame to transmit to the peer (downlink to mobile).
    ///
    /// Should be called once per 20ms frame period.
    pub fn next_frame(&mut self) -> RlpOutput {
        self.next_frame_for_mux(rlp::RlpMuxOption::One)
    }

    pub fn next_frame_for_mux(&mut self, mux_option: rlp::RlpMuxOption) -> RlpOutput {
        if self.state == RlpState::Uninitialized {
            self.initialize();
        }

        match self.state {
            RlpState::Uninitialized => unreachable!(),

            RlpState::Sync => {
                self.handshake_frames_sent += 1;
                RlpOutput::SendFrame(rlp::sync_frame(self.v_s))
            }

            RlpState::SyncAck => {
                self.handshake_frames_sent += 1;
                RlpOutput::SendFrame(rlp::sync_ack_frame(self.v_s))
            }

            RlpState::Ack => {
                self.handshake_frames_sent += 1;
                if self.handshake_frames_sent > self.round_trip_counter {
                    // Sent enough ACKs, transition to data transfer
                    self.enter_data_transfer();
                    return self.build_data_frame(mux_option);
                }
                RlpOutput::SendFrame(rlp::ack_frame(self.v_s))
            }

            RlpState::DataTransfer => self.build_data_frame(mux_option),
        }
    }

    // Internal helpers

    fn enter_data_transfer(&mut self) {
        log::info!(
            "RLP: entering DataTransfer state (delay={})",
            self.rlp_delay
        );
        self.state = RlpState::DataTransfer;
    }

    fn measure_delay(&mut self) {
        // RLP_DELAY_s = number of frames between sending SYNC/SYNC-ACK and
        // receiving the first valid non-blank response. Approximate with
        // handshake_frames_sent.
        self.rlp_delay = self.handshake_frames_sent.max(1);
    }

    fn process_data_frame(&mut self, frame: &RlpFrame) -> Option<RlpDelivery> {
        match frame {
            RlpFrame::Data { seq, data } => {
                if data.is_empty() {
                    // Idle frame (LEN=0) — update V(R) if SEQ > V(R)
                    self.update_vr_for_idle(*seq);
                    return None;
                }
                self.receive_data(*seq, data)
            }
            RlpFrame::DataFormatB { seq, data } => self.receive_data(*seq, data),
            RlpFrame::Idle { seq } => {
                self.update_vr_for_idle(*seq);
                None
            }
            RlpFrame::Control {
                control_type: ControlType::Nak,
                first,
                last,
                ..
            } => {
                self.process_nak(*first, *last);
                None
            }
            RlpFrame::Control { .. } => {
                // ACK/SYNC-ACK in data transfer — ignore (already connected)
                None
            }
            RlpFrame::Segmented {
                seq,
                segment_type,
                data,
            } => self.receive_segmented(*seq, *segment_type, data),
        }
    }

    /// Process a received data frame with the given SEQ and data.
    /// Implements the receive logic from 3.1.2.
    fn receive_data(&mut self, seq: u8, data: &[u8]) -> Option<RlpDelivery> {
        let cmp = seq_compare(seq, self.v_r);
        self.nak_outstanding[seq as usize] = false;

        if cmp == SeqCmp::Equal {
            // SEQ == V(R): expected frame
            if self.v_r == self.v_n {
                // In-order delivery
                self.v_r = self.v_r.wrapping_add(1);
                self.v_n = self.v_n.wrapping_add(1);
                self.rx_buffer.extend_from_slice(data);
                // Deliver any buffered contiguous frames
                self.deliver_contiguous();
            } else {
                // V(R) != V(N): there are gaps. Store and advance V(R).
                self.v_r = self.v_r.wrapping_add(1);
                self.reseq_buffer[seq as usize] = Some(data.to_vec());
                // Check overflow
                if seq_gt(self.v_r.wrapping_sub(128), self.v_n) {
                    self.initialize();
                    return None;
                }
            }
        } else if cmp == SeqCmp::Greater {
            // SEQ > V(R): gap detected
            // Store the frame and advance V(R) to SEQ+1
            self.reseq_buffer[seq as usize] = Some(data.to_vec());
            self.queue_naks_for_missing_range(self.v_n, seq.wrapping_sub(1));
            self.v_r = seq.wrapping_add(1);
            // Check overflow: if V(R)-128 > V(N), reset
            if seq_gt(self.v_r.wrapping_sub(128), self.v_n) {
                self.initialize();
                return None;
            }
        } else {
            // SEQ < V(R): possibly old/duplicate
            if seq_lt(seq, self.v_n) {
                // Duplicate — discard
                return None;
            }
            // SEQ >= V(N) and < V(R): late arrival, store in reseq buffer
            if self.reseq_buffer[seq as usize].is_none() {
                self.reseq_buffer[seq as usize] = Some(data.to_vec());
                self.nak_outstanding[seq as usize] = false;
                // Check if V(N) can now advance
                if seq == self.v_n {
                    self.deliver_contiguous();
                }
            }
        }

        if !self.rx_buffer.is_empty() {
            Some(RlpDelivery {
                data: self.take_received_data(),
            })
        } else {
            None
        }
    }

    /// Deliver contiguous frames from the resequencing buffer starting at V(N).
    fn deliver_contiguous(&mut self) {
        loop {
            let slot = self.v_n as usize;
            if let Some(data) = self.reseq_buffer[slot].take() {
                self.rx_buffer.extend_from_slice(&data);
                self.v_n = self.v_n.wrapping_add(1);
            } else {
                break;
            }
        }
    }

    /// Update V(R) when receiving an idle frame with SEQ > V(R).
    fn update_vr_for_idle(&mut self, seq: u8) {
        if seq_gt(seq, self.v_r) {
            // Idle frame with SEQ > V(R) means we missed frames.
            self.queue_naks_for_missing_range(self.v_n, seq.wrapping_sub(1));
            self.v_r = seq;
        }
    }

    fn queue_naks_for_missing_range(&mut self, first: u8, last: u8) {
        let mut seq = first;
        loop {
            if self.reseq_buffer[seq as usize].is_none() && !self.nak_outstanding[seq as usize] {
                self.pending_controls
                    .push_back(rlp::nak_frame(self.v_s, seq, seq));
                self.nak_outstanding[seq as usize] = true;
            }
            if seq == last {
                break;
            }
            seq = seq.wrapping_add(1);
        }
    }

    fn process_nak(&mut self, first: u8, last: u8) {
        let mut seq = first;
        loop {
            if seq == self.v_s || seq_gt(seq, self.v_s) {
                self.initialize();
                return;
            }
            if let Some(frame) = self.tx_history[seq as usize].clone() {
                self.pending_retransmissions.push_back(frame);
            }
            if seq == last {
                break;
            }
            seq = seq.wrapping_add(1);
        }
    }

    fn receive_segmented(
        &mut self,
        seq: u8,
        segment_type: SegmentType,
        data: &[u8],
    ) -> Option<RlpDelivery> {
        match segment_type {
            SegmentType::IntersegmentFill => None,
            SegmentType::First => {
                self.segment_assembly = Some(SegmentAssembly {
                    seq,
                    data: data.to_vec(),
                    expect_second_or_last: true,
                });
                None
            }
            SegmentType::Second => {
                let Some(assembly) = self.segment_assembly.as_mut() else {
                    return None;
                };
                if assembly.seq != seq || !assembly.expect_second_or_last {
                    self.segment_assembly = None;
                    return None;
                }
                assembly.data.extend_from_slice(data);
                assembly.expect_second_or_last = false;
                None
            }
            SegmentType::Last => {
                let Some(mut assembly) = self.segment_assembly.take() else {
                    return None;
                };
                if assembly.seq != seq {
                    return None;
                }
                assembly.data.extend_from_slice(data);
                self.receive_data(seq, &assembly.data)
            }
        }
    }

    /// Build the next data frame to send (or idle if no data queued).
    fn build_data_frame(&mut self, mux_option: rlp::RlpMuxOption) -> RlpOutput {
        if let Some(frame) = self.pending_controls.pop_front() {
            return RlpOutput::SendFrame(frame);
        }
        if let Some(frame) = self.pending_retransmissions.pop_front() {
            return RlpOutput::SendFrame(frame);
        }

        if self.tx_queue.is_empty() {
            // Send idle frame at Rate 1/8
            RlpOutput::SendFrame(rlp::idle_frame(self.v_s))
        } else {
            let available = self.tx_queue.len();
            let format_b_octets = mux_option.format_b_octets();
            let format_a_octets = mux_option.full_format_a_octets();

            if available >= format_b_octets {
                let data: Vec<u8> = self.tx_queue.drain(..format_b_octets).collect();
                let frame = rlp::data_format_b_frame(self.v_s, &data);
                self.tx_history[self.v_s as usize] = Some(frame.clone());
                self.v_s = self.v_s.wrapping_add(1);
                RlpOutput::SendFrame(frame)
            } else if available > 8 {
                let send_len = available.min(format_a_octets);
                let data: Vec<u8> = self.tx_queue.drain(..send_len).collect();
                let frame = rlp::data_frame(self.v_s, &data);
                self.tx_history[self.v_s as usize] = Some(frame.clone());
                self.v_s = self.v_s.wrapping_add(1);
                RlpOutput::SendFrame(frame)
            } else if available > 0 {
                // Use Format A at half rate (up to 8 octets)
                let send_len = available.min(8);
                let data: Vec<u8> = self.tx_queue.drain(..send_len).collect();
                let frame = rlp::data_frame(self.v_s, &data);
                self.tx_history[self.v_s as usize] = Some(frame.clone());
                self.v_s = self.v_s.wrapping_add(1);
                RlpOutput::SendFrame(frame)
            } else {
                RlpOutput::SendFrame(rlp::idle_frame(self.v_s))
            }
        }
    }
}

// Sequence number comparison helpers (modulo 256 arithmetic per 3.1.2)

#[derive(Debug, PartialEq, Eq)]
enum SeqCmp {
    Less,
    Equal,
    Greater,
}

/// Compare two sequence numbers using modulo-256 arithmetic (3.1.2):
/// For any N, (N+1)...(N+127) are "greater", (N-128)...(N-1) are "less".
fn seq_compare(a: u8, b: u8) -> SeqCmp {
    if a == b {
        return SeqCmp::Equal;
    }
    let diff = a.wrapping_sub(b);
    if diff >= 1 && diff <= 127 {
        SeqCmp::Greater
    } else {
        SeqCmp::Less
    }
}

/// Returns true if a > b in modulo-256 sequence space.
fn seq_gt(a: u8, b: u8) -> bool {
    seq_compare(a, b) == SeqCmp::Greater
}

/// Returns true if a < b in modulo-256 sequence space.
fn seq_lt(a: u8, b: u8) -> bool {
    seq_compare(a, b) == SeqCmp::Less
}

fn is_sync_frame(frame: &RlpFrame) -> bool {
    matches!(
        frame,
        RlpFrame::Control {
            control_type: ControlType::Sync,
            ..
        }
    )
}

fn is_sync_ack_frame(frame: &RlpFrame) -> bool {
    matches!(
        frame,
        RlpFrame::Control {
            control_type: ControlType::SyncAck,
            ..
        }
    )
}

// Tests

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rlp;

    #[test]
    fn initial_state_is_uninitialized() {
        let session = RlpSession::new();
        assert_eq!(session.state(), RlpState::Uninitialized);
    }

    #[test]
    fn sync_handshake_bs_initiates() {
        // BS/MSC side initiates: sends SYNC, mobile responds with SYNC/ACK,
        // BS sends ACK, then data transfer.
        let mut bs = RlpSession::new();

        // First next_frame triggers initialize -> Sync
        let out = bs.next_frame();
        assert_eq!(bs.state(), RlpState::Sync);
        assert!(matches!(
            out,
            RlpOutput::SendFrame(RlpFrame::Control {
                control_type: ControlType::Sync,
                ..
            })
        ));

        // Send a few more SYNCs
        for _ in 0..3 {
            let out = bs.next_frame();
            assert!(matches!(
                out,
                RlpOutput::SendFrame(RlpFrame::Control {
                    control_type: ControlType::Sync,
                    ..
                })
            ));
        }

        // Mobile sends SYNC/ACK
        let mobile_sync_ack = rlp::sync_ack_frame(0);
        bs.receive_frame(Some(&mobile_sync_ack));
        assert_eq!(bs.state(), RlpState::Ack);

        // BS sends ACK frames (>= 4)
        for i in 0..4 {
            let out = bs.next_frame();
            assert!(
                matches!(
                    out,
                    RlpOutput::SendFrame(RlpFrame::Control {
                        control_type: ControlType::Ack,
                        ..
                    })
                ),
                "frame {i} should be ACK"
            );
        }

        // After round_trip_counter ACKs, next_frame transitions to DataTransfer
        let out = bs.next_frame();
        assert_eq!(bs.state(), RlpState::DataTransfer);
        // Should be an idle frame (no data queued)
        assert!(match &out {
            RlpOutput::SendFrame(f) => f.is_idle(),
            _ => false,
        });
    }

    #[test]
    fn sync_handshake_both_sync() {
        // Both sides send SYNC simultaneously. BS receives SYNC from mobile,
        // transitions to SyncAck.
        let mut bs = RlpSession::new();
        bs.next_frame(); // triggers init, sends SYNC

        // Mobile also sends SYNC
        let mobile_sync = rlp::sync_frame(0);
        bs.receive_frame(Some(&mobile_sync));
        assert_eq!(bs.state(), RlpState::SyncAck);

        // BS sends SYNC/ACK
        let out = bs.next_frame();
        assert!(matches!(
            out,
            RlpOutput::SendFrame(RlpFrame::Control {
                control_type: ControlType::SyncAck,
                ..
            })
        ));

        // Mobile sends ACK
        let mobile_ack = rlp::ack_frame(0);
        bs.receive_frame(Some(&mobile_ack));
        assert_eq!(bs.state(), RlpState::DataTransfer);
    }

    #[test]
    fn data_transfer_sequential_delivery() {
        let mut bs = setup_connected_session();

        // Receive sequential data frames
        let frame1 = rlp::data_frame(0, &[0x01, 0x02, 0x03]);
        let delivery = bs.receive_frame(Some(&frame1));
        assert!(delivery.is_some());
        assert_eq!(delivery.unwrap().data, vec![0x01, 0x02, 0x03]);
        assert_eq!(bs.v_r(), 1);

        let frame2 = rlp::data_frame(1, &[0x04, 0x05]);
        let delivery = bs.receive_frame(Some(&frame2));
        assert!(delivery.is_some());
        assert_eq!(delivery.unwrap().data, vec![0x04, 0x05]);
        assert_eq!(bs.v_r(), 2);
    }

    #[test]
    fn data_transfer_out_of_order_resequencing() {
        let mut bs = setup_connected_session();

        // Receive frame 1 first (skipping frame 0)
        let frame1 = rlp::data_frame(1, &[0xBB]);
        let delivery = bs.receive_frame(Some(&frame1));
        // Frame 1 buffered, frame 0 missing — no delivery yet
        assert!(delivery.is_none());
        assert_eq!(bs.v_r(), 2); // V(R) advanced past the gap

        // Now receive frame 0
        let frame0 = rlp::data_frame(0, &[0xAA]);
        let delivery = bs.receive_frame(Some(&frame0));
        // Both frames delivered in order
        assert!(delivery.is_some());
        let data = delivery.unwrap().data;
        assert_eq!(data, vec![0xAA, 0xBB]);
        assert_eq!(bs.v_n(), 2);
    }

    #[test]
    fn data_transfer_send_data() {
        let mut bs = setup_connected_session();

        // Enqueue data
        bs.enqueue_data(&[0x01, 0x02, 0x03, 0x04, 0x05]);

        // next_frame should produce a data frame
        let out = bs.next_frame();
        match out {
            RlpOutput::SendFrame(RlpFrame::Data { seq, data }) => {
                assert_eq!(seq, 0);
                assert_eq!(data, vec![0x01, 0x02, 0x03, 0x04, 0x05]);
            }
            _ => panic!("expected data frame"),
        }
        assert_eq!(bs.v_s(), 1);

        // No more data — should send idle
        let out = bs.next_frame();
        assert!(match &out {
            RlpOutput::SendFrame(f) => f.is_idle(),
            _ => false,
        });
        // V(S) should NOT increment for idle
        assert_eq!(bs.v_s(), 1);
    }

    #[test]
    fn data_transfer_format_b_for_large_data() {
        let mut bs = setup_connected_session();

        // Enqueue 25 bytes — should use Format B (20) then Format A (5)
        bs.enqueue_data(&(0..25).collect::<Vec<u8>>());

        let out1 = bs.next_frame();
        match out1 {
            RlpOutput::SendFrame(RlpFrame::DataFormatB { seq, data }) => {
                assert_eq!(seq, 0);
                assert_eq!(data.len(), 20);
                assert_eq!(&data[..20], &(0..20).collect::<Vec<u8>>());
            }
            _ => panic!("expected Format B"),
        }

        let out2 = bs.next_frame();
        match out2 {
            RlpOutput::SendFrame(RlpFrame::Data { seq, data }) => {
                assert_eq!(seq, 1);
                assert_eq!(data, vec![20, 21, 22, 23, 24]);
            }
            _ => panic!("expected Format A data"),
        }
    }

    #[test]
    fn seq_number_wraps() {
        let mut bs = setup_connected_session();

        // Advance V(S) to 255
        for i in 0..255u16 {
            bs.enqueue_data(&[i as u8]);
            let out = bs.next_frame();
            match out {
                RlpOutput::SendFrame(f) => assert_eq!(f.seq(), i as u8),
                _ => panic!("expected frame"),
            }
        }
        assert_eq!(bs.v_s(), 255);

        // Next frame should wrap to 0
        bs.enqueue_data(&[0xFF]);
        let out = bs.next_frame();
        match out {
            RlpOutput::SendFrame(f) => assert_eq!(f.seq(), 255),
            _ => panic!("expected frame"),
        }
        assert_eq!(bs.v_s(), 0); // wrapped
    }

    #[test]
    fn sync_in_data_transfer_causes_reset() {
        let mut bs = setup_connected_session();
        assert_eq!(bs.state(), RlpState::DataTransfer);

        // Receiving SYNC during data transfer triggers reset
        let sync = rlp::sync_frame(0);
        bs.receive_frame(Some(&sync));
        assert_eq!(bs.state(), RlpState::Sync);
        assert_eq!(bs.v_s(), 0);
        assert_eq!(bs.v_r(), 0);
    }

    #[test]
    fn consecutive_erasures_cause_reset() {
        let mut bs = setup_connected_session();

        // 127 erasures: no reset
        for _ in 0..127 {
            bs.receive_frame(None);
        }
        assert_eq!(bs.state(), RlpState::DataTransfer);

        // 128th erasure: reset
        bs.receive_frame(None);
        assert_eq!(bs.state(), RlpState::Sync);
    }

    #[test]
    fn idle_frame_does_not_deliver_data() {
        let mut bs = setup_connected_session();

        let idle = rlp::idle_frame(0);
        let delivery = bs.receive_frame(Some(&idle));
        assert!(delivery.is_none());
    }

    #[test]
    fn gap_queues_nak_control_frame() {
        let mut bs = setup_connected_session();

        let frame1 = rlp::data_frame(1, &[0xBB]);
        let delivery = bs.receive_frame(Some(&frame1));
        assert!(delivery.is_none());

        match bs.next_frame() {
            RlpOutput::SendFrame(RlpFrame::Control {
                control_type: ControlType::Nak,
                first,
                last,
                ..
            }) => {
                assert_eq!(first, 0);
                assert_eq!(last, 0);
            }
            other => panic!("expected NAK for missing seq 0, got {other:?}"),
        }
    }

    #[test]
    fn received_nak_retransmits_stored_data_frame() {
        let mut bs = setup_connected_session();
        bs.enqueue_data(&[0x11, 0x22, 0x33]);

        let original = match bs.next_frame() {
            RlpOutput::SendFrame(frame @ RlpFrame::Data { seq: 0, .. }) => frame,
            other => panic!("expected original data frame, got {other:?}"),
        };

        let nak = rlp::nak_frame(0, 0, 0);
        assert!(bs.receive_frame(Some(&nak)).is_none());

        match bs.next_frame() {
            RlpOutput::SendFrame(frame) => assert_eq!(frame, original),
            other => panic!("expected retransmitted data frame, got {other:?}"),
        }
    }

    #[test]
    fn nak_for_unsent_frame_resets_session() {
        let mut bs = setup_connected_session();

        let nak = rlp::nak_frame(0, 0, 0);
        assert!(bs.receive_frame(Some(&nak)).is_none());

        assert_eq!(bs.state(), RlpState::Sync);
        assert_eq!(bs.v_s(), 0);
    }

    #[test]
    fn segmented_retransmission_is_reassembled_and_delivered() {
        let mut bs = setup_connected_session();

        let frame1 = rlp::data_frame(1, &[0xCC]);
        assert!(bs.receive_frame(Some(&frame1)).is_none());

        let first = RlpFrame::Segmented {
            seq: 0,
            segment_type: SegmentType::First,
            data: vec![0xAA],
        };
        let last = RlpFrame::Segmented {
            seq: 0,
            segment_type: SegmentType::Last,
            data: vec![0xBB],
        };

        assert!(bs.receive_frame(Some(&first)).is_none());
        let delivery = bs.receive_frame(Some(&last)).unwrap();

        assert_eq!(delivery.data, vec![0xAA, 0xBB, 0xCC]);
        assert_eq!(bs.v_n(), 2);
    }

    #[test]
    fn byte_stream_round_trip() {
        // Simulate a full round-trip: BS sends data, "mobile" receives and echoes back.
        let mut bs = setup_connected_session();
        let mut mobile = setup_connected_session();

        let payload = b"Hello, CDMA2000 packet data!";
        bs.enqueue_data(payload);

        // BS generates frames, mobile receives them
        let mut received_data = Vec::new();
        for _ in 0..10 {
            let out = bs.next_frame();
            if let RlpOutput::SendFrame(frame) = out {
                if let Some(delivery) = mobile.receive_frame(Some(&frame)) {
                    received_data.extend_from_slice(&delivery.data);
                }
            }
        }

        assert_eq!(received_data, payload.to_vec());
    }

    #[test]
    fn seq_compare_modulo_256() {
        assert_eq!(seq_compare(0, 0), SeqCmp::Equal);
        assert_eq!(seq_compare(1, 0), SeqCmp::Greater);
        assert_eq!(seq_compare(127, 0), SeqCmp::Greater);
        assert_eq!(seq_compare(128, 0), SeqCmp::Less); // 128 = N-128 from 0
        assert_eq!(seq_compare(255, 0), SeqCmp::Less);
        assert_eq!(seq_compare(0, 255), SeqCmp::Greater); // 0 = 255+1
        assert_eq!(seq_compare(0, 1), SeqCmp::Less);
    }

    // Helper: create a session that's already in DataTransfer state

    fn setup_connected_session() -> RlpSession {
        let mut session = RlpSession::new();
        // Initialize
        session.next_frame(); // -> Sync
        // Mobile sends SYNC/ACK
        session.receive_frame(Some(&rlp::sync_ack_frame(0)));
        // BS sends ACK frames
        for _ in 0..5 {
            session.next_frame();
        }
        // Should be in DataTransfer now
        assert_eq!(session.state(), RlpState::DataTransfer);
        session
    }
}
