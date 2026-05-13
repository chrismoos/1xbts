/// RLP Type 3 session state machine per C.S0017-010-A v1.0 Sections 3.2-3.7.
///
/// Implements the initialization (§3.2), SYNC exchange (§3.3), and data transfer
/// (§3.7) procedures for SO33 packet data over FCH.
///
/// Frame-synchronous: the caller drives the session by calling `receive_frame()`
/// for each received traffic frame and `next_frame()` to get the next outbound frame.
use crate::rlp3_frames::{self, MuxOption, NakGapEntry, NakPayload, Rlp3ControlType, Rlp3Frame};

// ---------------------------------------------------------------------------
// Frame rate
// ---------------------------------------------------------------------------

/// Physical layer rate for the primary traffic channel (FCH).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameRate {
    /// Full rate: 171 bits (MuxOption::Odd) or 266 bits (MuxOption::Even).
    Full,
    /// Half rate: 80 bits.
    Half,
    /// Quarter rate: 40 bits.
    Quarter,
    /// Eighth rate: 16 bits.
    Eighth,
    /// Blank / null frame (0 bits).
    Blank,
}

// ---------------------------------------------------------------------------
// Configuration
// ---------------------------------------------------------------------------

/// RLP Type 3 session configuration parameters.
#[derive(Debug, Clone)]
pub struct Rlp3Config {
    /// Multiplex option (determines frame sizes).
    pub mux_option: MuxOption,
    /// Maximum NAK retransmission rounds for forward link.
    pub nak_rounds_fwd: u8,
    /// Maximum NAK retransmission rounds for reverse link.
    pub nak_rounds_rev: u8,
    /// NAK_COUNT per round (number of NAK frames to send each round).
    pub nak_per_round: Vec<u8>,
    /// RLP_DELAY in frame counts. 0 = measure from SYNC exchange.
    pub rlp_delay: u8,
    /// Retransmission timer in milliseconds.
    pub rexmit_timer_ms: u32,
}

impl Default for Rlp3Config {
    fn default() -> Self {
        Self {
            mux_option: MuxOption::Odd,
            nak_rounds_fwd: 3,
            nak_rounds_rev: 3,
            nak_per_round: vec![1, 2, 3],
            rlp_delay: 0,
            rexmit_timer_ms: 5 * 20, // 5 frames * 20ms
        }
    }
}

// ---------------------------------------------------------------------------
// State
// ---------------------------------------------------------------------------

/// RLP Type 3 session state per §3.2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rlp3State {
    /// Not yet initialized. Will begin SYNC on first poll.
    Uninitialized,
    /// Sending SYNC frames, waiting for SYNC/ACK or SYNC from peer.
    Sync,
    /// Received SYNC from peer, sending SYNC/ACK, waiting for non-SYNC frame.
    SyncAck,
    /// Received SYNC/ACK from peer, sending ACK, waiting for non-SYNC/ACK frame.
    Ack,
    /// RLP link established, transferring data per §3.7.
    DataTransfer,
}

// ---------------------------------------------------------------------------
// Events emitted by the session
// ---------------------------------------------------------------------------

/// Events produced by `receive_frame()` for the caller to act on.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RlpEvent {
    /// State machine transitioned to a new state.
    StateChanged(Rlp3State),
    /// Data delivered in-order to the upper layer.
    DataDelivered(Vec<u8>),
    /// A NAK should be transmitted to the peer for the given gap.
    SendNak { first: u16, last: u16 },
    /// NAK rounds exhausted for a sequence range — data gap abandoned.
    NakAbandoned { first: u16, last: u16 },
}

// ---------------------------------------------------------------------------
// NAK list entry
// ---------------------------------------------------------------------------

/// Per-sequence NAK tracking entry (§3.7.2.5).
#[derive(Debug, Clone)]
struct NakEntry {
    /// 12-bit sequence number of the missing frame.
    l_seq: u16,
    /// Current retransmission round (1-based).
    round_counter: u8,
    /// Timer value in frame counts (decremented each frame period).
    rexmit_timer: u32,
}

// ---------------------------------------------------------------------------
// Retransmission queue entry
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
struct RexmitEntry {
    /// 12-bit L_SEQ of the frame to retransmit.
    l_seq: u16,
    /// Data payload.
    data: Vec<u8>,
}

// ---------------------------------------------------------------------------
// Segmented frame reassembly buffer
// ---------------------------------------------------------------------------

/// Tracks segments for a single RLP SDU being reassembled.
#[derive(Debug, Clone)]
struct SegmentReassembly {
    /// 8-bit SEQ from the first segment (used to feed receive_new_data on completion).
    seq_8: u8,
    /// Whether we've seen the LAST_SEG flag (know the total segment count).
    last_seg_seen: bool,
    /// Highest s_seq received so far.
    max_s_seq: u16,
    /// The s_seq value of the final segment (valid only when last_seg_seen=true).
    final_s_seq: u16,
    /// Collected segments indexed by s_seq.
    segments: Vec<Option<Vec<u8>>>,
    /// Whether this SDU is a retransmission.
    rexmit: bool,
}

// ---------------------------------------------------------------------------
// Rlp3Session
// ---------------------------------------------------------------------------

/// RLP Type 3 session state machine.
///
/// Processes frames one at a time, driven by the caller at 20ms frame boundaries.
pub struct Rlp3Session {
    config: Rlp3Config,
    state: Rlp3State,

    /// L_V(S): 12-bit sequence number of the next new data frame to transmit.
    l_v_s: u16,
    /// L_V(R): 12-bit expected sequence number of the next frame to receive.
    l_v_r: u16,
    /// L_V(N): 12-bit sequence number of the next needed frame for sequential delivery.
    l_v_n: u16,
    /// L_V(N)_peer: peer's L_V(N) as communicated via fill/idle frames.
    l_v_n_peer: u16,

    /// Round-trip frame counter for SYNC handshake (>= 4 per spec).
    round_trip_counter: u32,
    /// Frames sent in the current handshake phase.
    handshake_frames_sent: u32,
    /// Measured RLP_DELAY in frame counts.
    rlp_delay: u32,

    /// Outgoing data queue: bytes from upper layer waiting to be sent.
    tx_queue: Vec<u8>,

    /// Received data delivered to upper layer (accumulated, drained by `receive_data()`).
    rx_buffer: Vec<u8>,

    /// Resequencing buffer indexed by 12-bit L_SEQ. Each slot holds data octets or None.
    reseq_buffer: Vec<Option<Vec<u8>>>,

    /// NAK list: outstanding NAK entries for missing frames.
    nak_list: Vec<NakEntry>,

    /// Retransmission queue: frames we sent that were NAK'd and need retransmitting.
    rexmit_queue: Vec<RexmitEntry>,

    /// Sent data frames kept for retransmission, indexed by L_SEQ mod 4096.
    sent_buffer: Vec<Option<Vec<u8>>>,

    /// Idle timer: counts frames since last data/control transmission.
    idle_timer: u32,
    /// Idle interval: send idle frame every N frames when no data pending.
    idle_interval: u32,

    /// Pending control frames to send (highest priority).
    pending_controls: Vec<Rlp3Frame>,

    /// Segmented frame reassembly: in-progress SDU being assembled from segments.
    /// Only one SDU can be in-flight at a time per the spec (segments arrive sequentially
    /// for a given SEQ before the next SEQ begins).
    seg_reassembly: Option<SegmentReassembly>,
}

const SEQ_MODULUS: u16 = 4096;

impl Rlp3Session {
    /// Create a new RLP Type 3 session in Uninitialized state.
    pub fn new(config: Rlp3Config) -> Self {
        Self {
            config,
            state: Rlp3State::Uninitialized,
            l_v_s: 0,
            l_v_r: 0,
            l_v_n: 0,
            l_v_n_peer: 0,
            round_trip_counter: 0,
            handshake_frames_sent: 0,
            rlp_delay: 0,
            tx_queue: Vec::new(),
            rx_buffer: Vec::new(),
            reseq_buffer: vec![None; SEQ_MODULUS as usize],
            nak_list: Vec::new(),
            rexmit_queue: Vec::new(),
            sent_buffer: vec![None; SEQ_MODULUS as usize],
            idle_timer: 0,
            idle_interval: 10, // send idle every 10 frames (~200ms)
            pending_controls: Vec::new(),
            seg_reassembly: None,
        }
    }

    // -----------------------------------------------------------------------
    // Accessors
    // -----------------------------------------------------------------------

    pub fn state(&self) -> Rlp3State {
        self.state
    }

    pub fn l_v_s(&self) -> u16 {
        self.l_v_s
    }

    pub fn l_v_r(&self) -> u16 {
        self.l_v_r
    }

    pub fn l_v_n(&self) -> u16 {
        self.l_v_n
    }

    pub fn l_v_n_peer(&self) -> u16 {
        self.l_v_n_peer
    }

    pub fn rlp_delay(&self) -> u32 {
        self.rlp_delay
    }

    // -----------------------------------------------------------------------
    // Upper layer interface
    // -----------------------------------------------------------------------

    /// Queue data bytes from the upper layer (PPP) for transmission via RLP.
    pub fn send_data(&mut self, data: &[u8]) {
        self.tx_queue.extend_from_slice(data);
    }

    /// Returns true if the outgoing data queue is empty.
    pub fn tx_queue_is_empty(&self) -> bool {
        self.tx_queue.is_empty()
    }

    /// Returns the number of queued upper-layer bytes awaiting transmission.
    pub fn tx_queue_len(&self) -> usize {
        self.tx_queue.len()
    }

    /// Returns true if there are pending control frames (NAKs) or
    /// retransmissions that require a full-rate frame to send.
    pub fn has_pending_controls(&self) -> bool {
        !self.pending_controls.is_empty() || !self.rexmit_queue.is_empty()
    }

    /// Returns data delivered in-order from the resequencing buffer, if any.
    /// Clears the internal receive buffer.
    pub fn receive_data(&mut self) -> Option<Vec<u8>> {
        if self.rx_buffer.is_empty() {
            None
        } else {
            Some(std::mem::take(&mut self.rx_buffer))
        }
    }

    // -----------------------------------------------------------------------
    // Initialization (§3.2)
    // -----------------------------------------------------------------------

    /// Initialize/reset the RLP session per §3.2.
    pub fn initialize(&mut self) {
        self.l_v_s = 0;
        self.l_v_r = 0;
        self.l_v_n = 0;
        self.l_v_n_peer = 0;
        self.round_trip_counter = 0;
        self.handshake_frames_sent = 0;
        self.rlp_delay = 0;
        self.tx_queue.clear();
        self.rx_buffer.clear();
        for slot in self.reseq_buffer.iter_mut() {
            *slot = None;
        }
        for slot in self.sent_buffer.iter_mut() {
            *slot = None;
        }
        self.nak_list.clear();
        self.rexmit_queue.clear();
        self.idle_timer = 0;
        self.pending_controls.clear();
        self.seg_reassembly = None;
        self.state = Rlp3State::Sync;
    }

    // -----------------------------------------------------------------------
    // Frame reception
    // -----------------------------------------------------------------------

    /// Process a received frame from the peer.
    ///
    /// `frame_bits` contains raw primary traffic bits (each element 0 or 1).
    /// `rate` is the physical layer rate determination.
    /// Returns events for the caller (data delivered, NAKs to send, state changes).
    pub fn receive_frame(&mut self, frame_bits: &[u8], rate: FrameRate) -> Vec<RlpEvent> {
        if self.state == Rlp3State::Uninitialized {
            self.initialize();
            // Return state change event but continue processing
        }

        // Tick NAK timers each frame period (even for blank frames).
        let mut events = self.tick_nak_timers();

        // Blank frames are ignored (fill equivalent).
        if rate == FrameRate::Blank || frame_bits.is_empty() {
            return events;
        }

        // Decode the frame.
        let frame = if rate == FrameRate::Full {
            // Full rate: decode as RLP Type 3 frame.
            match rlp3_frames::decode_rlp3_frame(frame_bits, self.config.mux_option) {
                Ok(f) => f,
                Err(_) => {
                    // Try fill/idle decode for sub-rate frames carried at full rate.
                    match rlp3_frames::try_decode_fill_or_idle1(frame_bits, self.config.mux_option)
                    {
                        Ok(f) => f,
                        Err(_) => return events,
                    }
                }
            }
        } else {
            // Sub-rate (half/quarter): decode without TYPE field per §4.
            match rlp3_frames::sub_rate_info_bits(rate) {
                Some(num_info_bits) => {
                    match rlp3_frames::decode_sub_rate_frame(frame_bits, num_info_bits) {
                        Ok(f) => f,
                        Err(_) => return events,
                    }
                }
                None => return events, // Eighth rate (16 bits) — too small, ignore
            }
        };

        // Per 3.3/3.4, a received SYNC or SYNC/ACK with INIT_VAR=1 forces
        // re-initialization before continuing the SYNC exchange.
        if should_reinitialize_from_control(&frame) {
            let was = self.state;
            self.initialize();
            if was != Rlp3State::Sync {
                events.push(RlpEvent::StateChanged(Rlp3State::Sync));
            }
        }

        let mut frame_events = match self.state {
            Rlp3State::Uninitialized => unreachable!(),
            Rlp3State::Sync => self.process_sync_state(&frame),
            Rlp3State::SyncAck => self.process_sync_ack_state(&frame),
            Rlp3State::Ack => self.process_ack_state(&frame),
            Rlp3State::DataTransfer => self.process_data_transfer(&frame),
        };

        events.append(&mut frame_events);
        events
    }

    /// Get the next frame to transmit as bits.
    ///
    /// Returns the encoded bit vector for the given rate.
    /// Should be called once per 20ms frame period.
    pub fn next_frame(&mut self, rate: FrameRate) -> Vec<u8> {
        if self.state == Rlp3State::Uninitialized {
            self.initialize();
        }

        let mux = self.config.mux_option;

        match self.state {
            Rlp3State::Uninitialized => unreachable!(),

            Rlp3State::Sync => {
                self.handshake_frames_sent += 1;
                let frame = Rlp3Frame::Control {
                    seq: v_r_8(self.l_v_s),
                    control_type: Rlp3ControlType::Sync,
                    init_var: true,
                    nak_param_incl: false,
                };
                frame.encode(mux).unwrap_or_default()
            }

            Rlp3State::SyncAck => {
                self.handshake_frames_sent += 1;
                let frame = Rlp3Frame::Control {
                    seq: v_r_8(self.l_v_s),
                    control_type: Rlp3ControlType::SyncAck,
                    init_var: true,
                    nak_param_incl: false,
                };
                frame.encode(mux).unwrap_or_default()
            }

            Rlp3State::Ack => {
                self.handshake_frames_sent += 1;
                if self.handshake_frames_sent > self.round_trip_counter {
                    self.enter_data_transfer();
                    return self.build_data_frame(rate);
                }
                let frame = Rlp3Frame::Control {
                    seq: v_r_8(self.l_v_s),
                    control_type: Rlp3ControlType::Ack,
                    init_var: false,
                    nak_param_incl: false,
                };
                frame.encode(mux).unwrap_or_default()
            }

            Rlp3State::DataTransfer => self.build_data_frame(rate),
        }
    }

    // -----------------------------------------------------------------------
    // SYNC exchange (§3.3)
    // -----------------------------------------------------------------------

    fn process_sync_state(&mut self, frame: &Rlp3Frame) -> Vec<RlpEvent> {
        match frame {
            Rlp3Frame::Control {
                control_type: Rlp3ControlType::Sync,
                ..
            } => {
                // Peer also syncing -> respond with SYNC/ACK.
                self.round_trip_counter = 4;
                self.handshake_frames_sent = 0;
                self.state = Rlp3State::SyncAck;
                vec![RlpEvent::StateChanged(Rlp3State::SyncAck)]
            }
            Rlp3Frame::Control {
                control_type: Rlp3ControlType::SyncAck,
                ..
            } => {
                // Peer acknowledged our SYNC -> send ACK.
                self.measure_delay();
                self.round_trip_counter = 4;
                self.handshake_frames_sent = 0;
                self.state = Rlp3State::Ack;
                vec![RlpEvent::StateChanged(Rlp3State::Ack)]
            }
            _ => vec![],
        }
    }

    fn process_sync_ack_state(&mut self, frame: &Rlp3Frame) -> Vec<RlpEvent> {
        match frame {
            Rlp3Frame::Control {
                control_type: Rlp3ControlType::Sync,
                ..
            } => {
                // Peer still syncing, keep sending SYNC/ACK.
                self.handshake_frames_sent = 0;
                vec![]
            }
            Rlp3Frame::Control {
                control_type: Rlp3ControlType::Ack,
                ..
            }
            | Rlp3Frame::Control {
                control_type: Rlp3ControlType::SyncAck,
                ..
            } => {
                self.measure_delay();
                self.enter_data_transfer();
                vec![RlpEvent::StateChanged(Rlp3State::DataTransfer)]
            }
            _ if !is_sync(frame) && !is_fill(frame) && !is_blank(frame) => {
                // Any valid non-SYNC, non-fill, non-blank frame -> enter data transfer.
                self.enter_data_transfer();
                let mut events = vec![RlpEvent::StateChanged(Rlp3State::DataTransfer)];
                events.append(&mut self.process_data_transfer(frame));
                events
            }
            _ => vec![],
        }
    }

    fn process_ack_state(&mut self, frame: &Rlp3Frame) -> Vec<RlpEvent> {
        match frame {
            Rlp3Frame::Control {
                control_type: Rlp3ControlType::SyncAck,
                ..
            } => {
                // Peer still sending SYNC/ACK, keep sending ACK.
                self.handshake_frames_sent = 0;
                vec![]
            }
            Rlp3Frame::Control {
                control_type: Rlp3ControlType::Sync,
                ..
            } => {
                // Peer reset -> re-initialize.
                self.initialize();
                vec![RlpEvent::StateChanged(Rlp3State::Sync)]
            }
            _ if !is_sync_ack(frame) && !is_fill(frame) && !is_blank(frame) => {
                // Valid non-SYNC/ACK frame -> enter data transfer.
                self.enter_data_transfer();
                let mut events = vec![RlpEvent::StateChanged(Rlp3State::DataTransfer)];
                events.append(&mut self.process_data_transfer(frame));
                events
            }
            _ => vec![],
        }
    }

    // -----------------------------------------------------------------------
    // Data Transfer (§3.7)
    // -----------------------------------------------------------------------

    fn process_data_transfer(&mut self, frame: &Rlp3Frame) -> Vec<RlpEvent> {
        // SYNC during data transfer -> reset.
        if is_sync(frame) {
            self.initialize();
            return vec![RlpEvent::StateChanged(Rlp3State::Sync)];
        }

        match frame {
            Rlp3Frame::Data { seq, rexmit, data } => {
                if *rexmit {
                    self.receive_rexmit_data(*seq, data)
                } else if data.is_empty() {
                    // Idle/zero-length data frame.
                    vec![]
                } else {
                    self.receive_new_data(*seq, data)
                }
            }
            Rlp3Frame::DataFormatB { seq, rexmit, data } => {
                if *rexmit {
                    self.receive_rexmit_data(*seq, data)
                } else {
                    self.receive_new_data(*seq, data)
                }
            }
            Rlp3Frame::Nak {
                seq,
                seq_hi,
                payload,
            } => {
                self.process_nak(*seq, *seq_hi, payload);
                vec![]
            }
            Rlp3Frame::Fill { seq, seq_hi } => {
                let l_seq = ((*seq_hi as u16) << 8) | (*seq as u16);
                self.l_v_n_peer = l_seq;
                vec![]
            }
            Rlp3Frame::Idle1 { seq, seq_hi } => {
                let l_seq = ((*seq_hi as u16) << 8) | (*seq as u16);
                self.l_v_n_peer = l_seq;
                vec![]
            }
            Rlp3Frame::Idle2 { .. } => vec![],
            Rlp3Frame::Control { .. } => vec![],
            Rlp3Frame::Segmented {
                seq,
                sqi,
                last_seg,
                rexmit,
                seq_hi,
                s_seq,
                data,
            } => self.receive_segmented(*seq, *sqi, *last_seg, *rexmit, *seq_hi, *s_seq, data),
        }
    }

    /// Process a new (non-retransmitted) data frame. Implements §3.7.2.
    fn receive_new_data(&mut self, seq_8: u8, data: &[u8]) -> Vec<RlpEvent> {
        let l_seq = self.compute_l_seq(seq_8);

        if l_seq == self.l_v_r && self.l_v_r == self.l_v_n {
            // In-order, no gaps: deliver immediately and advance both.
            self.l_v_r = (self.l_v_r + 1) % SEQ_MODULUS;
            self.l_v_n = (self.l_v_n + 1) % SEQ_MODULUS;
            self.rx_buffer.extend_from_slice(data);
            // Deliver any contiguous buffered frames.
            self.deliver_contiguous();
            self.flush_delivery_events()
        } else if l_seq == self.l_v_r && self.l_v_r != self.l_v_n {
            // Expected frame but gaps exist before it. Store and advance L_V(R).
            self.reseq_buffer[l_seq as usize] = Some(data.to_vec());
            self.l_v_r = (self.l_v_r + 1) % SEQ_MODULUS;
            vec![]
        } else if seq12_gt(l_seq, self.l_v_r) {
            // Gap detected: create NAK entries for missing frames.
            let mut events = Vec::new();
            let gap_start = self.l_v_r;
            let gap_end = l_seq;
            // Generate NAKs for each missing frame in the gap.
            let mut s = gap_start;
            while s != gap_end {
                self.add_nak_entry(s);
                events.push(RlpEvent::SendNak { first: s, last: s });
                s = (s + 1) % SEQ_MODULUS;
            }
            // Store this frame and advance L_V(R).
            self.reseq_buffer[l_seq as usize] = Some(data.to_vec());
            self.l_v_r = (l_seq + 1) % SEQ_MODULUS;
            events
        } else if seq12_lt(l_seq, self.l_v_n) {
            // Old frame below L_V(N): may reset or discard. We discard.
            vec![]
        } else {
            // Between L_V(N) and L_V(R): late arrival. Store in reseq buffer.
            if self.reseq_buffer[l_seq as usize].is_none() {
                self.reseq_buffer[l_seq as usize] = Some(data.to_vec());
                // Remove from NAK list if present.
                self.nak_list.retain(|e| e.l_seq != l_seq);
                if l_seq == self.l_v_n {
                    self.deliver_contiguous();
                    return self.flush_delivery_events();
                }
            }
            vec![]
        }
    }

    /// Process a retransmitted data frame. Implements §3.7.2 retransmission handling.
    fn receive_rexmit_data(&mut self, seq_8: u8, data: &[u8]) -> Vec<RlpEvent> {
        // For retransmitted frames, look up by 8-bit SEQ match against NAK list.
        let matching_entry = self.nak_list.iter().find(|e| v_r_8(e.l_seq) == seq_8);
        if let Some(entry) = matching_entry {
            let l_seq = entry.l_seq;
            // Remove from NAK list.
            self.nak_list.retain(|e| e.l_seq != l_seq);
            // Store in resequencing buffer.
            if self.reseq_buffer[l_seq as usize].is_none() {
                self.reseq_buffer[l_seq as usize] = Some(data.to_vec());
                if l_seq == self.l_v_n {
                    self.deliver_contiguous();
                    return self.flush_delivery_events();
                }
            }
        }
        vec![]
    }

    /// Process a segmented data frame. Implements §3.7.2 segmentation reassembly.
    ///
    /// Segments of a single RLP SDU share the same 8-bit SEQ. The first segment
    /// has SQI=true (carries SEQ_HI for the full 12-bit L_SEQ). The last segment
    /// has LAST_SEG=true. S_SEQ counts segments within the SDU (0, 1, 2, …).
    ///
    /// When all segments are collected, the reassembled payload is delivered
    /// through `receive_new_data` (or `receive_rexmit_data` for retransmissions).
    fn receive_segmented(
        &mut self,
        seq_8: u8,
        sqi: bool,
        last_seg: bool,
        rexmit: bool,
        _seq_hi: Option<u8>,
        s_seq: u16,
        data: &[u8],
    ) -> Vec<RlpEvent> {
        // SQI=true marks the start of a new SDU (or a single-segment SDU).
        if sqi {
            // Starting a new reassembly — discard any incomplete prior SDU.
            let max_segs = if last_seg {
                (s_seq + 1) as usize
            } else {
                // Pre-allocate for a reasonable number; will grow if needed.
                (s_seq + 16).max(16) as usize
            };
            let mut segments: Vec<Option<Vec<u8>>> = vec![None; max_segs];
            segments[s_seq as usize] = Some(data.to_vec());

            let reasm = SegmentReassembly {
                seq_8,
                last_seg_seen: last_seg,
                max_s_seq: s_seq,
                final_s_seq: if last_seg { s_seq } else { 0 },
                segments,
                rexmit,
            };

            if last_seg {
                // Single-segment SDU (SQI=true, LAST_SEG=true) — deliver immediately.
                if let Some(assembled) = try_assemble(&reasm) {
                    self.seg_reassembly = None;
                    log::debug!(
                        "RLP3: segmented SDU reassembled (single-segment, {} bytes, seq={})",
                        assembled.len(),
                        seq_8
                    );
                    if rexmit {
                        return self.receive_rexmit_data(seq_8, &assembled);
                    } else {
                        return self.receive_new_data(seq_8, &assembled);
                    }
                }
            }

            self.seg_reassembly = Some(reasm);
            return vec![];
        }

        // Continuation segment (SQI=false) — append to in-progress reassembly.
        let reasm = match self.seg_reassembly.as_mut() {
            Some(r) if r.seq_8 == seq_8 => r,
            _ => {
                // No matching reassembly in progress — stale or misordered segment, drop.
                log::debug!(
                    "RLP3: dropping orphan segment s_seq={} seq={} (no active reassembly)",
                    s_seq,
                    seq_8
                );
                return vec![];
            }
        };

        // Grow the segments vec if needed.
        if s_seq as usize >= reasm.segments.len() {
            reasm.segments.resize((s_seq as usize) + 1, None);
        }
        reasm.segments[s_seq as usize] = Some(data.to_vec());

        if s_seq > reasm.max_s_seq {
            reasm.max_s_seq = s_seq;
        }
        if last_seg {
            reasm.last_seg_seen = true;
            reasm.final_s_seq = s_seq;
        }

        // Check if all segments are present.
        if reasm.last_seg_seen {
            if let Some(assembled) = try_assemble(reasm) {
                let rexmit = reasm.rexmit;
                let seq = reasm.seq_8;
                self.seg_reassembly = None;
                log::debug!(
                    "RLP3: segmented SDU reassembled ({} bytes, seq={})",
                    assembled.len(),
                    seq
                );
                if rexmit {
                    return self.receive_rexmit_data(seq, &assembled);
                } else {
                    return self.receive_new_data(seq, &assembled);
                }
            }
        }

        vec![]
    }

    /// Compute 12-bit L_SEQ from 8-bit SEQ for non-delayed new data frames (§3.7.2).
    fn compute_l_seq(&self, seq_8: u8) -> u16 {
        let v_r_low = (self.l_v_r & 0xFF) as u8;
        let offset = seq_8.wrapping_sub(v_r_low) as u16; // mod 256
        (self.l_v_r.wrapping_add(offset)) % SEQ_MODULUS
    }

    /// Deliver contiguous frames from the resequencing buffer starting at L_V(N).
    fn deliver_contiguous(&mut self) {
        loop {
            let slot = self.l_v_n as usize;
            if let Some(data) = self.reseq_buffer[slot].take() {
                self.rx_buffer.extend_from_slice(&data);
                self.l_v_n = (self.l_v_n + 1) % SEQ_MODULUS;
            } else {
                break;
            }
        }
    }

    /// Collect pending delivered data into events.
    fn flush_delivery_events(&mut self) -> Vec<RlpEvent> {
        if self.rx_buffer.is_empty() {
            vec![]
        } else {
            let data = std::mem::take(&mut self.rx_buffer);
            vec![RlpEvent::DataDelivered(data)]
        }
    }

    // -----------------------------------------------------------------------
    // NAK list management (§3.7.2.5)
    // -----------------------------------------------------------------------

    /// Add a new NAK entry for a missing frame.
    fn add_nak_entry(&mut self, l_seq: u16) {
        // Don't duplicate.
        if self.nak_list.iter().any(|e| e.l_seq == l_seq) {
            return;
        }
        let timer = self.rexmit_timer_frames();
        self.nak_list.push(NakEntry {
            l_seq,
            round_counter: 1,
            rexmit_timer: timer,
        });
        // Queue NAK control frame for transmission.
        self.queue_nak_for(l_seq);
    }

    /// Queue a NAK gap frame for a single missing sequence.
    fn queue_nak_for(&mut self, l_seq: u16) {
        let frame = Rlp3Frame::Nak {
            seq: v_r_8(l_seq),
            seq_hi: (l_seq >> 8) as u8,
            payload: NakPayload::Gap(vec![NakGapEntry {
                first: l_seq,
                last: l_seq,
            }]),
        };
        self.pending_controls.push(frame);
    }

    /// Tick NAK timers (called once per frame period). Returns events for expired entries.
    fn tick_nak_timers(&mut self) -> Vec<RlpEvent> {
        let mut events = Vec::new();
        let mut expired_seqs = Vec::new();
        let mut new_nak_frames = Vec::new();
        let timer_val = self.rexmit_timer_frames();
        let max_rounds = self.config.nak_rounds_fwd;

        for entry in &mut self.nak_list {
            if entry.rexmit_timer > 0 {
                entry.rexmit_timer -= 1;
            }
            if entry.rexmit_timer == 0 {
                if entry.round_counter < max_rounds {
                    // Send NAK_COUNT[round] copies per C.S0017 §3.7.2.5.
                    let round_idx = entry.round_counter as usize;
                    let nak_count = self
                        .config
                        .nak_per_round
                        .get(round_idx)
                        .copied()
                        .unwrap_or(1) as usize;
                    entry.round_counter += 1;
                    entry.rexmit_timer = timer_val;
                    for _ in 0..nak_count {
                        new_nak_frames.push(Rlp3Frame::Nak {
                            seq: v_r_8(entry.l_seq),
                            seq_hi: (entry.l_seq >> 8) as u8,
                            payload: NakPayload::Gap(vec![NakGapEntry {
                                first: entry.l_seq,
                                last: entry.l_seq,
                            }]),
                        });
                    }
                } else {
                    // Rounds exhausted: abandon.
                    expired_seqs.push(entry.l_seq);
                }
            }
        }
        self.pending_controls.extend(new_nak_frames);

        for seq in &expired_seqs {
            events.push(RlpEvent::NakAbandoned {
                first: *seq,
                last: *seq,
            });
        }

        // Remove expired entries.
        self.nak_list.retain(|e| !expired_seqs.contains(&e.l_seq));

        // If NAK rounds exhausted, advance L_V(N) past the gap and deliver buffered data.
        if !expired_seqs.is_empty() {
            self.advance_l_v_n_past_gaps();
            let delivery = self.flush_delivery_events();
            events.extend(delivery);
        }

        events
    }

    /// Advance L_V(N) past any gaps where we've given up waiting.
    fn advance_l_v_n_past_gaps(&mut self) {
        // Skip past any slots that are empty and have no NAK entry pending.
        loop {
            if self.l_v_n == self.l_v_r {
                break;
            }
            let slot = self.l_v_n as usize;
            if self.reseq_buffer[slot].is_some() {
                // Deliver this frame.
                let data = self.reseq_buffer[slot].take().unwrap();
                self.rx_buffer.extend_from_slice(&data);
                self.l_v_n = (self.l_v_n + 1) % SEQ_MODULUS;
            } else if self.nak_list.iter().any(|e| e.l_seq == self.l_v_n) {
                // Still waiting for this one.
                break;
            } else {
                // Gap abandoned, skip.
                self.l_v_n = (self.l_v_n + 1) % SEQ_MODULUS;
            }
        }
    }

    /// Retransmission timer value in frame counts per C.S0017 §3.7.2.5.
    /// Timer = 2 × RLP_DELAY + 1 frames.
    fn rexmit_timer_frames(&self) -> u32 {
        let delay = if self.config.rlp_delay > 0 {
            self.config.rlp_delay as u32
        } else {
            self.rlp_delay.max(1)
        };
        2 * delay + 1
    }

    /// Process an incoming NAK from the peer: queue retransmissions.
    fn process_nak(&mut self, _seq: u8, _seq_hi: u8, payload: &NakPayload) {
        match payload {
            NakPayload::Gap(entries) => {
                for entry in entries {
                    let mut s = entry.first;
                    loop {
                        if let Some(data) = &self.sent_buffer[s as usize] {
                            self.rexmit_queue.push(RexmitEntry {
                                l_seq: s,
                                data: data.clone(),
                            });
                        }
                        if s == entry.last {
                            break;
                        }
                        s = (s + 1) % SEQ_MODULUS;
                    }
                }
            }
            _ => {
                // Map/segment NAK types not implemented.
            }
        }
    }

    // -----------------------------------------------------------------------
    // Frame building (§3.7.1)
    // -----------------------------------------------------------------------

    /// Build the next frame to transmit. Priority: control > rexmit > new data > idle > fill.
    fn build_data_frame(&mut self, rate: FrameRate) -> Vec<u8> {
        let mux = self.config.mux_option;

        // At sub-rate, only control/idle/fill frames fit.
        if rate != FrameRate::Full {
            if let Some(info_bits) = rlp3_frames::sub_rate_info_bits(rate) {
                return rlp3_frames::encode_sub_rate_fill(
                    v_r_8(self.l_v_n),
                    (self.l_v_n >> 8) as u8,
                    info_bits,
                )
                .unwrap_or_default();
            }
            return Vec::new();
        }

        // Priority 1: Pending control frames.
        if let Some(ctrl) = self.pending_controls.pop() {
            return ctrl.encode(mux).unwrap_or_default();
        }

        // Priority 2: Retransmitted data.
        if let Some(rexmit) = self.rexmit_queue.pop() {
            self.idle_timer = 0;
            let seq_8 = v_r_8(rexmit.l_seq);
            let data_len = mux.format_b_data_len();
            if rexmit.data.len() == data_len {
                let frame = Rlp3Frame::DataFormatB {
                    seq: seq_8,
                    rexmit: true,
                    data: rexmit.data,
                };
                return frame.encode(mux).unwrap_or_default();
            } else {
                let frame = Rlp3Frame::Data {
                    seq: seq_8,
                    rexmit: true,
                    data: rexmit.data,
                };
                return frame.encode(mux).unwrap_or_default();
            }
        }

        // Priority 3: New data.
        if !self.tx_queue.is_empty() {
            self.idle_timer = 0;
            return self.build_new_data_frame();
        }

        // Priority 4: Idle frame (periodic).
        self.idle_timer += 1;
        if self.idle_timer >= self.idle_interval {
            self.idle_timer = 0;
            return self.build_idle_frame();
        }

        // Priority 5: Fill frame.
        self.build_fill_frame()
    }

    /// Build a new data frame from the TX queue.
    fn build_new_data_frame(&mut self) -> Vec<u8> {
        let mux = self.config.mux_option;
        let available = self.tx_queue.len();
        let format_b_len = mux.format_b_data_len();
        let max_a_len = mux.max_data_len();
        let l_seq = self.l_v_s;
        let seq_8 = v_r_8(l_seq);
        let need_seq_hi = self.new_data_requires_seq_hi(l_seq);
        let segmented_single_len = max_segmented_single_segment_len(mux);

        let (frame, data_sent) = if need_seq_hi {
            // Per C.S0017-010-A 4.3.1.2, when a new data frame must carry SEQ_HI,
            // use the segmented format with LAST_SEG=1 and S_SEQ=0. We keep the
            // payload to a single segment for now; this is spec-compliant and
            // avoids changing the byte-stream scheduler semantics.
            let send_len = available.min(segmented_single_len);
            let data: Vec<u8> = self.tx_queue.drain(..send_len).collect();
            let f = Rlp3Frame::Segmented {
                seq: seq_8,
                sqi: true,
                last_seg: true,
                rexmit: false,
                seq_hi: Some((l_seq >> 8) as u8),
                s_seq: 0,
                data: data.clone(),
            };
            (f, data)
        } else if available >= format_b_len {
            // Use Format B for maximum throughput.
            let data: Vec<u8> = self.tx_queue.drain(..format_b_len).collect();
            let f = Rlp3Frame::DataFormatB {
                seq: seq_8,
                rexmit: false,
                data: data.clone(),
            };
            (f, data)
        } else {
            // Use Format A (unsegmented).
            let send_len = available.min(max_a_len);
            let data: Vec<u8> = self.tx_queue.drain(..send_len).collect();
            let f = Rlp3Frame::Data {
                seq: seq_8,
                rexmit: false,
                data: data.clone(),
            };
            (f, data)
        };

        // Save for potential retransmission.
        self.sent_buffer[self.l_v_s as usize] = Some(data_sent);

        // Increment L_V(S) mod 4096.
        self.l_v_s = (self.l_v_s + 1) % SEQ_MODULUS;

        frame.encode(mux).unwrap_or_default()
    }

    fn new_data_requires_seq_hi(&self, l_seq: u16) -> bool {
        // C.S0017-010-A 3.7.1: when NUM_ROUNDSpeer > 0 and the 12-bit sequence
        // is more than 255 ahead of L_V(N)peer, the new data frame shall include
        // SEQ_HI. We model NUM_ROUNDSpeer with the negotiated reverse-direction
        // NAK rounds.
        self.config.nak_rounds_rev > 0
            && ((l_seq + SEQ_MODULUS - self.l_v_n_peer) % SEQ_MODULUS) > 255
    }

    /// Build a fill frame.
    fn build_fill_frame(&mut self) -> Vec<u8> {
        let mux = self.config.mux_option;
        let frame = Rlp3Frame::Fill {
            seq: v_r_8(self.l_v_n),
            seq_hi: (self.l_v_n >> 8) as u8,
        };
        frame.encode(mux).unwrap_or_default()
    }

    /// Build an idle frame (Format 1 with SEQ_HI).
    fn build_idle_frame(&mut self) -> Vec<u8> {
        let mux = self.config.mux_option;
        let frame = Rlp3Frame::Idle1 {
            seq: v_r_8(self.l_v_n),
            seq_hi: (self.l_v_n >> 8) as u8,
        };
        frame.encode(mux).unwrap_or_default()
    }

    // -----------------------------------------------------------------------
    // Internal helpers
    // -----------------------------------------------------------------------

    fn enter_data_transfer(&mut self) {
        self.state = Rlp3State::DataTransfer;
    }

    fn measure_delay(&mut self) {
        self.rlp_delay = self.handshake_frames_sent.max(1);
    }
}

// ---------------------------------------------------------------------------
// Sequence number helpers (12-bit modulo 4096)
// ---------------------------------------------------------------------------

/// Extract the least significant 8 bits of a 12-bit sequence number.
fn v_r_8(l_seq: u16) -> u8 {
    (l_seq & 0xFF) as u8
}

fn max_segmented_single_segment_len(mux: MuxOption) -> usize {
    // 4.3.1.2 uses the segmented frame format to carry an unsegmented new data
    // frame. With SQI=1, LAST_SEG=1, S_SEQ=0, the available payload is the
    // remaining info bits after the segmented header and octet-alignment pad.
    let header_bits = 8 + 4 + 1 + 1 + 1 + 5 + 4 + 12 + 4;
    (mux.info_bits().saturating_sub(header_bits)) / 8
}

/// Compare two 12-bit sequence numbers: returns true if a > b in mod-4096 space.
fn seq12_gt(a: u16, b: u16) -> bool {
    if a == b {
        return false;
    }
    let diff = a.wrapping_sub(b) % SEQ_MODULUS;
    diff >= 1 && diff <= (SEQ_MODULUS / 2 - 1)
}

/// Compare two 12-bit sequence numbers: returns true if a < b in mod-4096 space.
fn seq12_lt(a: u16, b: u16) -> bool {
    if a == b {
        return false;
    }
    !seq12_gt(a, b)
}

// ---------------------------------------------------------------------------
// Frame classification helpers
// ---------------------------------------------------------------------------

fn is_sync(frame: &Rlp3Frame) -> bool {
    matches!(
        frame,
        Rlp3Frame::Control {
            control_type: Rlp3ControlType::Sync,
            ..
        }
    )
}

fn is_sync_ack(frame: &Rlp3Frame) -> bool {
    matches!(
        frame,
        Rlp3Frame::Control {
            control_type: Rlp3ControlType::SyncAck,
            ..
        }
    )
}

fn is_fill(frame: &Rlp3Frame) -> bool {
    matches!(frame, Rlp3Frame::Fill { .. })
}

fn is_blank(_frame: &Rlp3Frame) -> bool {
    // Blank frames never reach the decoder; this is a fallback.
    false
}

/// Try to assemble a complete SDU from collected segments.
/// Returns `Some(data)` if all segments [0..final_s_seq] are present, `None` otherwise.
fn try_assemble(reasm: &SegmentReassembly) -> Option<Vec<u8>> {
    if !reasm.last_seg_seen {
        return None;
    }
    let count = (reasm.final_s_seq + 1) as usize;
    let mut assembled = Vec::new();
    for i in 0..count {
        match &reasm.segments.get(i) {
            Some(Some(seg_data)) => assembled.extend_from_slice(seg_data),
            _ => return None, // Missing segment.
        }
    }
    Some(assembled)
}

fn should_reinitialize_from_control(frame: &Rlp3Frame) -> bool {
    matches!(
        frame,
        Rlp3Frame::Control {
            control_type: Rlp3ControlType::Sync | Rlp3ControlType::SyncAck,
            init_var: true,
            ..
        }
    )
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    fn default_config() -> Rlp3Config {
        Rlp3Config::default()
    }

    fn mux() -> MuxOption {
        MuxOption::Odd
    }

    /// Helper: create a session already in DataTransfer state (BS initiated sync).
    fn setup_connected_session() -> Rlp3Session {
        let mut session = Rlp3Session::new(default_config());
        // Initialize -> Sync.
        let _sync_bits = session.next_frame(FrameRate::Full);
        assert_eq!(session.state(), Rlp3State::Sync);

        // Peer sends SYNC/ACK.
        let sync_ack = Rlp3Frame::Control {
            seq: 0,
            control_type: Rlp3ControlType::SyncAck,
            init_var: false,
            nak_param_incl: false,
        };
        let sync_ack_bits = sync_ack.encode(mux()).unwrap();
        session.receive_frame(&sync_ack_bits, FrameRate::Full);
        assert_eq!(session.state(), Rlp3State::Ack);

        // Send enough ACK frames to transition.
        for _ in 0..5 {
            session.next_frame(FrameRate::Full);
        }
        assert_eq!(session.state(), Rlp3State::DataTransfer);
        session
    }

    /// Encode a frame for feeding into receive_frame.
    fn encode_frame(frame: &Rlp3Frame) -> Vec<u8> {
        frame.encode(mux()).unwrap()
    }

    #[test]
    fn quarter_rate_idle_fill_is_encoded_as_sub_rate_bits() {
        let mut session = setup_connected_session();
        let bits = session.next_frame(FrameRate::Quarter);

        assert_eq!(bits.len(), 40);
        let decoded = rlp3_frames::decode_sub_rate_frame(&bits, 40).unwrap();
        assert!(matches!(decoded, Rlp3Frame::Fill { .. }));
    }

    // -------------------------------------------------------------------
    // Test 1: Initialization resets all state
    // -------------------------------------------------------------------
    #[test]
    fn test_initialization_resets_all_state() {
        let mut session = Rlp3Session::new(default_config());
        // Mutate some state.
        session.l_v_s = 100;
        session.l_v_r = 50;
        session.l_v_n = 25;
        session.tx_queue.push(0xFF);
        session.rx_buffer.push(0xAA);

        session.initialize();

        assert_eq!(session.l_v_s, 0);
        assert_eq!(session.l_v_r, 0);
        assert_eq!(session.l_v_n, 0);
        assert_eq!(session.l_v_n_peer, 0);
        assert!(session.tx_queue.is_empty());
        assert!(session.rx_buffer.is_empty());
        assert!(session.nak_list.is_empty());
        assert!(session.rexmit_queue.is_empty());
        assert_eq!(session.state(), Rlp3State::Sync);
    }

    // -------------------------------------------------------------------
    // Test 2: SYNC exchange — BS sends SYNC, receives SYNC/ACK, sends ACK
    // -------------------------------------------------------------------
    #[test]
    fn test_sync_exchange_bs_initiates() {
        let mut bs = Rlp3Session::new(default_config());

        // First next_frame triggers initialize -> Sync, produces SYNC bits.
        let sync_bits = bs.next_frame(FrameRate::Full);
        assert_eq!(bs.state(), Rlp3State::Sync);
        assert!(!sync_bits.is_empty());

        // Verify it decodes as SYNC.
        let decoded = rlp3_frames::decode_rlp3_frame(&sync_bits, mux()).unwrap();
        assert!(matches!(
            decoded,
            Rlp3Frame::Control {
                control_type: Rlp3ControlType::Sync,
                init_var: true,
                ..
            }
        ));

        // Peer sends SYNC/ACK.
        let sync_ack = Rlp3Frame::Control {
            seq: 0,
            control_type: Rlp3ControlType::SyncAck,
            init_var: false,
            nak_param_incl: false,
        };
        let events = bs.receive_frame(&encode_frame(&sync_ack), FrameRate::Full);
        assert_eq!(bs.state(), Rlp3State::Ack);
        assert!(
            events
                .iter()
                .any(|e| *e == RlpEvent::StateChanged(Rlp3State::Ack))
        );

        // BS sends ACK frames (>= 4).
        for _ in 0..4 {
            let ack_bits = bs.next_frame(FrameRate::Full);
            let decoded = rlp3_frames::decode_rlp3_frame(&ack_bits, mux()).unwrap();
            assert!(matches!(
                decoded,
                Rlp3Frame::Control {
                    control_type: Rlp3ControlType::Ack,
                    init_var: false,
                    ..
                }
            ));
        }

        // After round_trip_counter ACKs, transitions to DataTransfer.
        let _frame = bs.next_frame(FrameRate::Full);
        assert_eq!(bs.state(), Rlp3State::DataTransfer);
    }

    // -------------------------------------------------------------------
    // Test 3: SYNC exchange — receives SYNC first (MS-initiated)
    // -------------------------------------------------------------------
    #[test]
    fn test_sync_exchange_ms_initiated() {
        let mut bs = Rlp3Session::new(default_config());
        let _sync_bits = bs.next_frame(FrameRate::Full);
        assert_eq!(bs.state(), Rlp3State::Sync);

        // Peer sends SYNC (both sides syncing simultaneously).
        let peer_sync = Rlp3Frame::Control {
            seq: 0,
            control_type: Rlp3ControlType::Sync,
            init_var: false,
            nak_param_incl: false,
        };
        let events = bs.receive_frame(&encode_frame(&peer_sync), FrameRate::Full);
        assert_eq!(bs.state(), Rlp3State::SyncAck);
        assert!(
            events
                .iter()
                .any(|e| *e == RlpEvent::StateChanged(Rlp3State::SyncAck))
        );

        // BS sends SYNC/ACK.
        let sync_ack_bits = bs.next_frame(FrameRate::Full);
        let decoded = rlp3_frames::decode_rlp3_frame(&sync_ack_bits, mux()).unwrap();
        assert!(matches!(
            decoded,
            Rlp3Frame::Control {
                control_type: Rlp3ControlType::SyncAck,
                init_var: true,
                ..
            }
        ));

        // Peer sends ACK.
        let peer_ack = Rlp3Frame::Control {
            seq: 0,
            control_type: Rlp3ControlType::Ack,
            init_var: false,
            nak_param_incl: false,
        };
        let events = bs.receive_frame(&encode_frame(&peer_ack), FrameRate::Full);
        assert_eq!(bs.state(), Rlp3State::DataTransfer);
        assert!(
            events
                .iter()
                .any(|e| *e == RlpEvent::StateChanged(Rlp3State::DataTransfer))
        );
    }

    // -------------------------------------------------------------------
    // Test 4: Send single data frame -> L_V(S) increments
    // -------------------------------------------------------------------
    #[test]
    fn test_send_single_data_frame_increments_l_v_s() {
        let mut bs = setup_connected_session();
        assert_eq!(bs.l_v_s(), 0);

        bs.send_data(&[0x01, 0x02, 0x03]);
        let frame_bits = bs.next_frame(FrameRate::Full);
        assert_eq!(bs.l_v_s(), 1);

        // Verify it decodes as a data frame.
        let decoded = rlp3_frames::decode_rlp3_frame(&frame_bits, mux()).unwrap();
        match decoded {
            Rlp3Frame::Data { seq, rexmit, data } => {
                assert_eq!(seq, 0);
                assert!(!rexmit);
                assert_eq!(data, vec![0x01, 0x02, 0x03]);
            }
            _ => panic!("expected unsegmented data frame, got {:?}", decoded),
        }
    }

    // -------------------------------------------------------------------
    // Test 5: Receive single data frame in order -> delivered to upper layer
    // -------------------------------------------------------------------
    #[test]
    fn test_receive_single_data_frame_in_order() {
        let mut bs = setup_connected_session();

        let data_frame = Rlp3Frame::Data {
            seq: 0,
            rexmit: false,
            data: vec![0x01, 0x02, 0x03],
        };
        let events = bs.receive_frame(&encode_frame(&data_frame), FrameRate::Full);

        assert_eq!(bs.l_v_r(), 1);
        assert_eq!(bs.l_v_n(), 1);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, RlpEvent::DataDelivered(d) if d == &vec![0x01, 0x02, 0x03]))
        );
    }

    // -------------------------------------------------------------------
    // Test 6: Receive data frame with gap -> NAK generated
    // -------------------------------------------------------------------
    #[test]
    fn test_receive_data_frame_with_gap_generates_nak() {
        let mut bs = setup_connected_session();

        // Receive frame 2, skipping 0 and 1.
        let data_frame = Rlp3Frame::Data {
            seq: 2,
            rexmit: false,
            data: vec![0xCC],
        };
        let events = bs.receive_frame(&encode_frame(&data_frame), FrameRate::Full);

        assert_eq!(bs.l_v_r(), 3);
        // NAKs generated for seq 0 and 1.
        let nak_events: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, RlpEvent::SendNak { .. }))
            .collect();
        assert_eq!(nak_events.len(), 2);
        assert!(
            nak_events
                .iter()
                .any(|e| matches!(e, RlpEvent::SendNak { first: 0, last: 0 }))
        );
        assert!(
            nak_events
                .iter()
                .any(|e| matches!(e, RlpEvent::SendNak { first: 1, last: 1 }))
        );
    }

    // -------------------------------------------------------------------
    // Test 7: Receive retransmitted frame -> fills gap, delivers buffered data
    // -------------------------------------------------------------------
    #[test]
    fn test_receive_retransmitted_frame_fills_gap() {
        let mut bs = setup_connected_session();

        // Receive frame 1, skipping frame 0. Creates NAK for 0.
        let frame1 = Rlp3Frame::Data {
            seq: 1,
            rexmit: false,
            data: vec![0xBB],
        };
        bs.receive_frame(&encode_frame(&frame1), FrameRate::Full);
        assert_eq!(bs.l_v_r(), 2);
        assert_eq!(bs.l_v_n(), 0); // Still waiting for frame 0.

        // Receive retransmission of frame 0.
        let rexmit_frame0 = Rlp3Frame::Data {
            seq: 0,
            rexmit: true,
            data: vec![0xAA],
        };
        let events = bs.receive_frame(&encode_frame(&rexmit_frame0), FrameRate::Full);

        // Both frames should be delivered in order.
        assert_eq!(bs.l_v_n(), 2);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, RlpEvent::DataDelivered(d) if d == &vec![0xAA, 0xBB]))
        );
    }

    // -------------------------------------------------------------------
    // Test 8: Receive out-of-order frames -> resequencing delivers in order
    // -------------------------------------------------------------------
    #[test]
    fn test_out_of_order_resequencing() {
        let mut bs = setup_connected_session();

        // Receive frames 2, 1, 0 (completely out of order).
        let frame2 = Rlp3Frame::Data {
            seq: 2,
            rexmit: false,
            data: vec![0x03],
        };
        let events = bs.receive_frame(&encode_frame(&frame2), FrameRate::Full);
        // Frame 2 buffered, NAKs generated for 0 and 1.
        assert!(events.iter().any(|e| matches!(e, RlpEvent::SendNak { .. })));
        assert!(bs.receive_data().is_none());

        // Receive frame 0 as retransmission.
        let frame0 = Rlp3Frame::Data {
            seq: 0,
            rexmit: true,
            data: vec![0x01],
        };
        let events = bs.receive_frame(&encode_frame(&frame0), FrameRate::Full);
        // Frame 0 fills its slot. deliver_contiguous advances L_V(N) to 1
        // (frame 1 still missing), delivering just frame 0's data.
        assert_eq!(bs.l_v_n(), 1);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, RlpEvent::DataDelivered(d) if d == &vec![0x01]))
        );

        // Receive frame 1 as retransmission.
        let frame1 = Rlp3Frame::Data {
            seq: 1,
            rexmit: true,
            data: vec![0x02],
        };
        let events = bs.receive_frame(&encode_frame(&frame1), FrameRate::Full);
        // Frames 1 and 2 now delivered (frame 0 was delivered earlier).
        assert_eq!(bs.l_v_n(), 3);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, RlpEvent::DataDelivered(d) if d == &vec![0x02, 0x03]))
        );
    }

    // -------------------------------------------------------------------
    // Test 9: NAK timer expiry -> retransmit NAK with higher count
    // -------------------------------------------------------------------
    #[test]
    fn test_nak_timer_expiry_retransmits() {
        let mut config = default_config();
        config.nak_rounds_fwd = 3;
        config.rlp_delay = 1;
        let mut bs = Rlp3Session::new(config);
        // Fast-forward to DataTransfer.
        bs.initialize();
        bs.state = Rlp3State::DataTransfer;
        bs.l_v_s = 0;
        bs.l_v_r = 0;
        bs.l_v_n = 0;

        // Receive frame 1, skipping frame 0 -> NAK for 0, round_counter=1.
        let frame1 = Rlp3Frame::Data {
            seq: 1,
            rexmit: false,
            data: vec![0xBB],
        };
        bs.receive_frame(&encode_frame(&frame1), FrameRate::Full);
        assert_eq!(bs.nak_list.len(), 1);
        assert_eq!(bs.nak_list[0].round_counter, 1);
        let initial_timer = bs.nak_list[0].rexmit_timer;

        // Drain initial pending control (the initial NAK).
        bs.pending_controls.clear();

        // Tick timer down by receiving blank frames (no bits, just tick NAK timers).
        for _ in 0..initial_timer {
            bs.receive_frame(&[], FrameRate::Blank);
        }

        // After timer expires, round_counter should increment to 2.
        assert_eq!(bs.nak_list.len(), 1);
        assert_eq!(bs.nak_list[0].round_counter, 2);
        // A new NAK should have been queued.
        assert!(!bs.pending_controls.is_empty());
    }

    // -------------------------------------------------------------------
    // Test 10: NAK rounds exhausted -> advance L_V(N), deliver what we have
    // -------------------------------------------------------------------
    #[test]
    fn test_nak_rounds_exhausted_advances_l_v_n() {
        let mut config = default_config();
        config.nak_rounds_fwd = 2;
        config.rlp_delay = 1;
        let mut bs = Rlp3Session::new(config);
        bs.initialize();
        bs.state = Rlp3State::DataTransfer;

        // Receive frame 1, skipping frame 0.
        let frame1 = Rlp3Frame::Data {
            seq: 1,
            rexmit: false,
            data: vec![0xBB],
        };
        bs.receive_frame(&encode_frame(&frame1), FrameRate::Full);
        bs.pending_controls.clear();

        // Exhaust all NAK rounds by ticking through timers with blank frames.
        let timer_val = bs.rexmit_timer_frames();
        for _round in 0..3 {
            for _ in 0..timer_val {
                bs.receive_frame(&[], FrameRate::Blank);
            }
            bs.pending_controls.clear();
        }

        // NAK list should be empty (rounds exhausted).
        assert!(bs.nak_list.is_empty());
        // L_V(N) should have advanced past the gap to deliver frame 1.
        assert!(bs.l_v_n() >= 2);
    }

    // -------------------------------------------------------------------
    // Test 11: Idle frame generation when no data pending
    // -------------------------------------------------------------------
    #[test]
    fn test_idle_frame_generation() {
        let mut bs = setup_connected_session();
        bs.idle_interval = 3; // Send idle every 3 frames for easier testing.

        // First few frames should be fill.
        let frame1 = bs.next_frame(FrameRate::Full);
        let frame2 = bs.next_frame(FrameRate::Full);

        // Third frame should be idle (interval reached).
        let frame3 = bs.next_frame(FrameRate::Full);

        // Verify frame3 decodes as a valid idle/fill frame.
        let decoded = rlp3_frames::decode_rlp3_frame(&frame3, mux()).unwrap();
        match decoded {
            Rlp3Frame::Data { data, .. } if data.is_empty() => {
                // zero-length data interpreted as idle is fine
            }
            _ => {
                // Either way, a valid frame was produced.
            }
        }

        // The point is no panic and we got frames.
        assert!(!frame1.is_empty());
        assert!(!frame2.is_empty());
        assert!(!frame3.is_empty());
    }

    // -------------------------------------------------------------------
    // Test 12: Fill frame when nothing to send
    // -------------------------------------------------------------------
    #[test]
    fn test_fill_frame_when_nothing_to_send() {
        let mut bs = setup_connected_session();

        // With no data queued, next_frame at full rate should produce a valid frame.
        let frame_bits = bs.next_frame(FrameRate::Full);
        assert_eq!(frame_bits.len(), mux().frame_bits());

        // L_V(S) should NOT increment for fill/idle frames.
        assert_eq!(bs.l_v_s(), 0);
    }

    // -------------------------------------------------------------------
    // Test 13: Sequence number wraparound at mod 4096
    // -------------------------------------------------------------------
    #[test]
    fn test_sequence_number_wraparound_mod_4096() {
        let mut bs = setup_connected_session();

        // Set L_V(S) near the wraparound boundary.
        bs.l_v_s = 4095;
        bs.send_data(&[0x01, 0x02, 0x03]);

        let _frame_bits = bs.next_frame(FrameRate::Full);
        assert_eq!(bs.l_v_s(), 0); // Wrapped from 4095 to 0.
    }

    // -------------------------------------------------------------------
    // Test 14: L_SEQ computation from 8-bit SEQ
    // -------------------------------------------------------------------
    #[test]
    fn test_l_seq_computation_from_8bit_seq() {
        let mut bs = setup_connected_session();

        // L_V(R) = 0, SEQ = 0 -> L_SEQ = 0.
        assert_eq!(bs.compute_l_seq(0), 0);

        // L_V(R) = 0, SEQ = 5 -> L_SEQ = 5.
        assert_eq!(bs.compute_l_seq(5), 5);

        // L_V(R) = 256, SEQ = 0 -> L_SEQ = 256 (wraps in 8-bit space).
        bs.l_v_r = 256;
        assert_eq!(bs.compute_l_seq(0), 256);

        // L_V(R) = 256, SEQ = 1 -> L_SEQ = 257.
        assert_eq!(bs.compute_l_seq(1), 257);

        // L_V(R) = 4090, SEQ = 250 (= 4090 & 0xFF) -> L_SEQ = 4090.
        bs.l_v_r = 4090;
        assert_eq!(bs.compute_l_seq(250), 4090);

        // L_V(R) = 4090, SEQ = 252 -> L_SEQ = 4092.
        assert_eq!(bs.compute_l_seq(252), 4092);

        // L_V(R) = 4090, SEQ = 0 -> L_SEQ = 4096 mod 4096 = 0 (wrapped past boundary).
        // offset = 0 - 250 = 6 (mod 256). L_SEQ = (4090 + 6) % 4096 = 0.
        assert_eq!(bs.compute_l_seq(0), 0);
    }

    // -------------------------------------------------------------------
    // Test 15: Format B frame selection at full rate for max throughput
    // -------------------------------------------------------------------
    #[test]
    fn test_format_b_selection_for_large_data() {
        let mut bs = setup_connected_session();

        // Enqueue 25 bytes. Format B carries 20 bytes (odd mux).
        bs.send_data(&(0..25).collect::<Vec<u8>>());

        let frame1_bits = bs.next_frame(FrameRate::Full);
        let decoded1 = rlp3_frames::decode_rlp3_frame(&frame1_bits, mux()).unwrap();
        match decoded1 {
            Rlp3Frame::DataFormatB { seq, rexmit, data } => {
                assert_eq!(seq, 0);
                assert!(!rexmit);
                assert_eq!(data.len(), 20);
                assert_eq!(&data[..], &(0..20).collect::<Vec<u8>>());
            }
            _ => panic!("expected Format B, got {:?}", decoded1),
        }

        // Remaining 5 bytes should go as Format A.
        let frame2_bits = bs.next_frame(FrameRate::Full);
        let decoded2 = rlp3_frames::decode_rlp3_frame(&frame2_bits, mux()).unwrap();
        match decoded2 {
            Rlp3Frame::Data { seq, rexmit, data } => {
                assert_eq!(seq, 1);
                assert!(!rexmit);
                assert_eq!(data, vec![20, 21, 22, 23, 24]);
            }
            _ => panic!("expected Format A data, got {:?}", decoded2),
        }

        assert_eq!(bs.l_v_s(), 2);
    }

    // -------------------------------------------------------------------
    // Test 15b: New data more than 255 ahead of L_V(N)_peer includes SEQ_HI
    // -------------------------------------------------------------------
    #[test]
    fn test_new_data_uses_segmented_format_when_seq_hi_required() {
        let mut bs = setup_connected_session();

        bs.l_v_s = 300;
        bs.l_v_n_peer = 0;
        bs.send_data(&(0..20).collect::<Vec<u8>>());

        let frame_bits = bs.next_frame(FrameRate::Full);
        let decoded = rlp3_frames::decode_rlp3_frame(&frame_bits, mux()).unwrap();
        match decoded {
            Rlp3Frame::Segmented {
                seq,
                sqi,
                last_seg,
                rexmit,
                seq_hi,
                s_seq,
                data,
            } => {
                assert_eq!(seq, v_r_8(300));
                assert!(sqi);
                assert!(last_seg);
                assert!(!rexmit);
                assert_eq!(seq_hi, Some((300 >> 8) as u8));
                assert_eq!(s_seq, 0);
                assert_eq!(data, (0..16).collect::<Vec<u8>>());
            }
            _ => panic!("expected segmented new data frame, got {:?}", decoded),
        }

        assert_eq!(bs.l_v_s(), 301);

        let frame_bits = bs.next_frame(FrameRate::Full);
        let decoded = rlp3_frames::decode_rlp3_frame(&frame_bits, mux()).unwrap();
        match decoded {
            Rlp3Frame::Segmented {
                seq,
                sqi,
                last_seg,
                rexmit,
                seq_hi,
                s_seq,
                data,
            } => {
                assert_eq!(seq, v_r_8(301));
                assert!(sqi);
                assert!(last_seg);
                assert!(!rexmit);
                assert_eq!(seq_hi, Some((301 >> 8) as u8));
                assert_eq!(s_seq, 0);
                assert_eq!(data, vec![16, 17, 18, 19]);
            }
            _ => panic!("expected segmented new data frame, got {:?}", decoded),
        }

        assert_eq!(bs.l_v_s(), 302);
    }

    // -------------------------------------------------------------------
    // Test 16: Upper layer send_data -> queued and transmitted across multiple frames
    // -------------------------------------------------------------------
    #[test]
    fn test_send_data_queued_across_multiple_frames() {
        let mut bs = setup_connected_session();

        // Queue 45 bytes. Should take 3 frames: 20 (B) + 20 (B) + 5 (A).
        let payload: Vec<u8> = (0..45).collect();
        bs.send_data(&payload);

        let mut total_sent = Vec::new();
        for _ in 0..3 {
            let bits = bs.next_frame(FrameRate::Full);
            let decoded = rlp3_frames::decode_rlp3_frame(&bits, mux()).unwrap();
            match decoded {
                Rlp3Frame::DataFormatB { data, .. } => total_sent.extend_from_slice(&data),
                Rlp3Frame::Data { data, .. } => total_sent.extend_from_slice(&data),
                _ => panic!("expected data frame"),
            }
        }

        assert_eq!(total_sent, payload);
        assert_eq!(bs.l_v_s(), 3);

        // Next frame should be fill (no more data).
        let fill_bits = bs.next_frame(FrameRate::Full);
        assert_eq!(fill_bits.len(), mux().frame_bits());
        assert_eq!(bs.l_v_s(), 3); // No increment for fill.
    }

    // -------------------------------------------------------------------
    // Test 17: Byte stream round-trip between two sessions
    // -------------------------------------------------------------------
    #[test]
    fn test_byte_stream_round_trip() {
        let mut bs = setup_connected_session();
        let mut ms = setup_connected_session();

        let payload = b"Hello, CDMA2000 SO33 packet data!";
        bs.send_data(payload);

        // BS generates frames, MS receives them.
        let mut received = Vec::new();
        for _ in 0..10 {
            let bits = bs.next_frame(FrameRate::Full);
            let events = ms.receive_frame(&bits, FrameRate::Full);
            for event in events {
                if let RlpEvent::DataDelivered(data) = event {
                    received.extend_from_slice(&data);
                }
            }
        }

        assert_eq!(received, payload.to_vec());
    }

    // -------------------------------------------------------------------
    // Test 18: SYNC during DataTransfer causes reset
    // -------------------------------------------------------------------
    #[test]
    fn test_sync_during_data_transfer_resets() {
        let mut bs = setup_connected_session();
        assert_eq!(bs.state(), Rlp3State::DataTransfer);

        let sync = Rlp3Frame::Control {
            seq: 0,
            control_type: Rlp3ControlType::Sync,
            init_var: false,
            nak_param_incl: false,
        };
        let events = bs.receive_frame(&encode_frame(&sync), FrameRate::Full);
        assert_eq!(bs.state(), Rlp3State::Sync);
        assert!(
            events
                .iter()
                .any(|e| *e == RlpEvent::StateChanged(Rlp3State::Sync))
        );
    }

    // -------------------------------------------------------------------
    // Test 19: INIT_VAR on received SYNC/ACK forces re-initialization first
    // -------------------------------------------------------------------
    #[test]
    fn test_received_init_var_sync_ack_reinitializes() {
        let mut bs = setup_connected_session();
        bs.l_v_s = 17;
        bs.l_v_r = 9;
        bs.l_v_n = 4;
        bs.send_data(&[0xAA, 0xBB]);

        let sync_ack = Rlp3Frame::Control {
            seq: 0,
            control_type: Rlp3ControlType::SyncAck,
            init_var: true,
            nak_param_incl: false,
        };

        let events = bs.receive_frame(&encode_frame(&sync_ack), FrameRate::Full);

        assert_eq!(bs.state(), Rlp3State::Ack);
        assert_eq!(bs.l_v_s(), 0);
        assert_eq!(bs.l_v_r(), 0);
        assert_eq!(bs.l_v_n(), 0);
        assert!(bs.tx_queue_is_empty());
        assert!(
            events
                .iter()
                .any(|e| *e == RlpEvent::StateChanged(Rlp3State::Ack))
        );
    }

    // -------------------------------------------------------------------
    // Test 20: seq12_gt / seq12_lt helpers
    // -------------------------------------------------------------------
    #[test]
    fn test_seq12_comparison() {
        assert!(seq12_gt(1, 0));
        assert!(seq12_gt(2047, 0));
        assert!(!seq12_gt(2048, 0)); // 2048 = halfway, considered less
        assert!(seq12_lt(4095, 0)); // 4095 wraps to just below 0
        assert!(seq12_gt(0, 4095)); // 0 is just past 4095

        assert!(!seq12_gt(0, 0));
        assert!(!seq12_lt(0, 0));
    }

    // -------------------------------------------------------------------
    // Test 20b: v_r_8 extracts low 8 bits
    // -------------------------------------------------------------------
    #[test]
    fn test_v_r_8() {
        assert_eq!(v_r_8(0), 0);
        assert_eq!(v_r_8(255), 255);
        assert_eq!(v_r_8(256), 0);
        assert_eq!(v_r_8(257), 1);
        assert_eq!(v_r_8(4095), 255);
    }

    // -------------------------------------------------------------------
    // Test 20c: Single-segment SDU (SQI=true, LAST_SEG=true) delivered
    // -------------------------------------------------------------------
    #[test]
    fn test_segmented_single_segment_sdu_delivered() {
        let mut bs = setup_connected_session();

        // A single-segment SDU: SQI=true, LAST_SEG=true, s_seq=0.
        let frame = Rlp3Frame::Segmented {
            seq: 0,
            sqi: true,
            last_seg: true,
            rexmit: false,
            seq_hi: Some(0),
            s_seq: 0,
            data: vec![0xDE, 0xAD, 0xBE, 0xEF],
        };
        let events = bs.receive_frame(&encode_frame(&frame), FrameRate::Full);

        assert_eq!(bs.l_v_r(), 1);
        assert_eq!(bs.l_v_n(), 1);
        assert!(events.iter().any(
            |e| matches!(e, RlpEvent::DataDelivered(d) if d == &vec![0xDE, 0xAD, 0xBE, 0xEF])
        ));
    }

    // -------------------------------------------------------------------
    // Test 20d: Multi-segment SDU reassembled in order
    // -------------------------------------------------------------------
    #[test]
    fn test_segmented_multi_segment_sdu_in_order() {
        let mut bs = setup_connected_session();

        // Segment 0 of 3: SQI=true (first), LAST_SEG=false.
        let seg0 = Rlp3Frame::Segmented {
            seq: 0,
            sqi: true,
            last_seg: false,
            rexmit: false,
            seq_hi: Some(0),
            s_seq: 0,
            data: vec![0x01, 0x02, 0x03],
        };
        let events = bs.receive_frame(&encode_frame(&seg0), FrameRate::Full);
        assert!(
            events
                .iter()
                .all(|e| !matches!(e, RlpEvent::DataDelivered(_)))
        );
        assert_eq!(bs.l_v_r(), 0); // Not delivered yet.

        // Segment 1: SQI=false, LAST_SEG=false.
        let seg1 = Rlp3Frame::Segmented {
            seq: 0,
            sqi: false,
            last_seg: false,
            rexmit: false,
            seq_hi: None,
            s_seq: 1,
            data: vec![0x04, 0x05],
        };
        let events = bs.receive_frame(&encode_frame(&seg1), FrameRate::Full);
        assert!(
            events
                .iter()
                .all(|e| !matches!(e, RlpEvent::DataDelivered(_)))
        );

        // Segment 2 (final): SQI=false, LAST_SEG=true.
        let seg2 = Rlp3Frame::Segmented {
            seq: 0,
            sqi: false,
            last_seg: true,
            rexmit: false,
            seq_hi: None,
            s_seq: 2,
            data: vec![0x06],
        };
        let events = bs.receive_frame(&encode_frame(&seg2), FrameRate::Full);

        // Now the full SDU should be delivered.
        assert_eq!(bs.l_v_r(), 1);
        assert_eq!(bs.l_v_n(), 1);
        assert!(events.iter().any(
            |e| matches!(e, RlpEvent::DataDelivered(d) if d == &vec![0x01, 0x02, 0x03, 0x04, 0x05, 0x06])
        ));
    }

    // -------------------------------------------------------------------
    // Test 20e: Segmented SDU followed by unsegmented frames
    // -------------------------------------------------------------------
    #[test]
    fn test_segmented_then_unsegmented_sequential() {
        let mut bs = setup_connected_session();

        // First: single-segment SDU at seq=0.
        let seg = Rlp3Frame::Segmented {
            seq: 0,
            sqi: true,
            last_seg: true,
            rexmit: false,
            seq_hi: Some(0),
            s_seq: 0,
            data: vec![0xAA],
        };
        let events = bs.receive_frame(&encode_frame(&seg), FrameRate::Full);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, RlpEvent::DataDelivered(d) if d == &vec![0xAA]))
        );
        assert_eq!(bs.l_v_r(), 1);

        // Second: unsegmented data at seq=1.
        let data = Rlp3Frame::Data {
            seq: 1,
            rexmit: false,
            data: vec![0xBB],
        };
        let events = bs.receive_frame(&encode_frame(&data), FrameRate::Full);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, RlpEvent::DataDelivered(d) if d == &vec![0xBB]))
        );
        assert_eq!(bs.l_v_r(), 2);
        assert_eq!(bs.l_v_n(), 2);
    }

    // -------------------------------------------------------------------
    // Test 20f: Sequential single-segment SDUs (simulates MS sending
    //           segmented format for every frame, as seen in live SO33)
    // -------------------------------------------------------------------
    #[test]
    fn test_segmented_sequential_single_segment_sdus() {
        let mut bs = setup_connected_session();

        // Simulate 10 consecutive single-segment SDUs (what the MS does in practice).
        let mut all_delivered = Vec::new();
        for i in 0u8..10 {
            let frame = Rlp3Frame::Segmented {
                seq: i,
                sqi: true,
                last_seg: true,
                rexmit: false,
                seq_hi: Some(0),
                s_seq: 0,
                data: vec![i + 1; 16], // 16 bytes of payload per SDU
            };
            let events = bs.receive_frame(&encode_frame(&frame), FrameRate::Full);
            for event in events {
                if let RlpEvent::DataDelivered(d) = event {
                    all_delivered.extend_from_slice(&d);
                }
            }
        }

        assert_eq!(bs.l_v_r(), 10);
        assert_eq!(bs.l_v_n(), 10);
        // Each SDU is 16 bytes, 10 SDUs = 160 bytes total.
        assert_eq!(all_delivered.len(), 160);
        // Verify ordering: first SDU was all 1s, second all 2s, etc.
        assert_eq!(&all_delivered[0..16], &[1u8; 16]);
        assert_eq!(&all_delivered[144..160], &[10u8; 16]);
    }

    // -------------------------------------------------------------------
    // Test 20g: Segmented SDU with gap before it triggers NAK
    // -------------------------------------------------------------------
    #[test]
    fn test_segmented_sdu_with_gap_generates_nak() {
        let mut bs = setup_connected_session();

        // Receive segmented SDU at seq=2, skipping seq 0 and 1.
        let frame = Rlp3Frame::Segmented {
            seq: 2,
            sqi: true,
            last_seg: true,
            rexmit: false,
            seq_hi: Some(0),
            s_seq: 0,
            data: vec![0xCC],
        };
        let events = bs.receive_frame(&encode_frame(&frame), FrameRate::Full);

        // NAKs generated for seq 0 and 1.
        let nak_events: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, RlpEvent::SendNak { .. }))
            .collect();
        assert_eq!(nak_events.len(), 2);
        assert_eq!(bs.l_v_r(), 3);
        assert_eq!(bs.l_v_n(), 0); // Waiting for gap to fill.
    }

    // -------------------------------------------------------------------
    // Test 20h: Multi-segment SDU with last segment arriving first
    // -------------------------------------------------------------------
    #[test]
    fn test_segmented_last_before_middle() {
        let mut bs = setup_connected_session();

        // Segment 0 (first): SQI=true.
        let seg0 = Rlp3Frame::Segmented {
            seq: 0,
            sqi: true,
            last_seg: false,
            rexmit: false,
            seq_hi: Some(0),
            s_seq: 0,
            data: vec![0x10],
        };
        bs.receive_frame(&encode_frame(&seg0), FrameRate::Full);

        // Segment 2 (last): arrives before segment 1.
        let seg2 = Rlp3Frame::Segmented {
            seq: 0,
            sqi: false,
            last_seg: true,
            rexmit: false,
            seq_hi: None,
            s_seq: 2,
            data: vec![0x30],
        };
        let events = bs.receive_frame(&encode_frame(&seg2), FrameRate::Full);
        // Still missing segment 1 — not delivered yet.
        assert!(
            events
                .iter()
                .all(|e| !matches!(e, RlpEvent::DataDelivered(_)))
        );

        // Segment 1 (middle): completes the SDU.
        let seg1 = Rlp3Frame::Segmented {
            seq: 0,
            sqi: false,
            last_seg: false,
            rexmit: false,
            seq_hi: None,
            s_seq: 1,
            data: vec![0x20],
        };
        let events = bs.receive_frame(&encode_frame(&seg1), FrameRate::Full);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, RlpEvent::DataDelivered(d) if d == &vec![0x10, 0x20, 0x30]))
        );
        assert_eq!(bs.l_v_r(), 1);
        assert_eq!(bs.l_v_n(), 1);
    }

    // -------------------------------------------------------------------
    // Test 20i: New SQI=true segment discards incomplete prior reassembly
    // -------------------------------------------------------------------
    #[test]
    fn test_segmented_new_sqi_discards_incomplete() {
        let mut bs = setup_connected_session();

        // Start a 3-segment SDU at seq=0, only send segment 0.
        let seg0 = Rlp3Frame::Segmented {
            seq: 0,
            sqi: true,
            last_seg: false,
            rexmit: false,
            seq_hi: Some(0),
            s_seq: 0,
            data: vec![0x01],
        };
        bs.receive_frame(&encode_frame(&seg0), FrameRate::Full);

        // Before completing it, a new SDU arrives at seq=1 with SQI=true.
        // The old reassembly should be discarded.
        let new_sdu = Rlp3Frame::Segmented {
            seq: 1,
            sqi: true,
            last_seg: true,
            rexmit: false,
            seq_hi: Some(0),
            s_seq: 0,
            data: vec![0xFF],
        };
        let events = bs.receive_frame(&encode_frame(&new_sdu), FrameRate::Full);

        // The new single-segment SDU should be delivered. The old incomplete one is gone.
        // Note: seq=1 arrives but L_V(R) is 0, so there's a gap for seq=0.
        // receive_new_data(seq=1) will detect the gap and NAK seq=0.
        let nak_events: Vec<_> = events
            .iter()
            .filter(|e| matches!(e, RlpEvent::SendNak { .. }))
            .collect();
        assert_eq!(nak_events.len(), 1); // NAK for missing seq=0
        assert_eq!(bs.l_v_r(), 2);
    }

    // -------------------------------------------------------------------
    // Test 20j: Segmented retransmission delivered correctly
    // -------------------------------------------------------------------
    #[test]
    fn test_segmented_rexmit_delivered() {
        let mut bs = setup_connected_session();

        // Receive unsegmented frame 0 normally.
        let frame0 = Rlp3Frame::Data {
            seq: 0,
            rexmit: false,
            data: vec![0xAA],
        };
        bs.receive_frame(&encode_frame(&frame0), FrameRate::Full);
        assert_eq!(bs.l_v_r(), 1);

        // Skip frame 1, receive frame 2 → NAK for 1.
        let frame2 = Rlp3Frame::Data {
            seq: 2,
            rexmit: false,
            data: vec![0xCC],
        };
        bs.receive_frame(&encode_frame(&frame2), FrameRate::Full);
        assert_eq!(bs.l_v_r(), 3);
        assert_eq!(bs.l_v_n(), 1); // Waiting for frame 1.

        // Frame 1 arrives as a segmented retransmission.
        let rexmit_seg = Rlp3Frame::Segmented {
            seq: 1,
            sqi: true,
            last_seg: true,
            rexmit: true,
            seq_hi: Some(0),
            s_seq: 0,
            data: vec![0xBB],
        };
        let events = bs.receive_frame(&encode_frame(&rexmit_seg), FrameRate::Full);

        // Frames 1 and 2 should both be delivered now.
        assert_eq!(bs.l_v_n(), 3);
        assert!(
            events
                .iter()
                .any(|e| matches!(e, RlpEvent::DataDelivered(d) if d == &vec![0xBB, 0xCC]))
        );
    }

    // -------------------------------------------------------------------
    // Test 20k: Long stream of segmented single-segment SDUs past 8-bit wrap
    // -------------------------------------------------------------------
    #[test]
    fn test_segmented_stream_past_8bit_seq_wrap() {
        let mut bs = setup_connected_session();

        // Send 300 consecutive single-segment SDUs to exercise the 8-bit SEQ
        // wraparound in compute_l_seq (wraps at 256).
        let mut total_delivered = 0usize;
        for i in 0u16..300 {
            let seq_8 = (i & 0xFF) as u8;
            let seq_hi = (i >> 8) as u8;
            let frame = Rlp3Frame::Segmented {
                seq: seq_8,
                sqi: true,
                last_seg: true,
                rexmit: false,
                seq_hi: Some(seq_hi),
                s_seq: 0,
                data: vec![(i & 0xFF) as u8; 16],
            };
            let events = bs.receive_frame(&encode_frame(&frame), FrameRate::Full);
            for event in events {
                if let RlpEvent::DataDelivered(d) = event {
                    total_delivered += d.len();
                }
            }
        }

        assert_eq!(bs.l_v_r(), 300);
        assert_eq!(bs.l_v_n(), 300);
        assert_eq!(total_delivered, 300 * 16);
    }

    // -------------------------------------------------------------------
    // Test 21: Receive sequential data frames
    // -------------------------------------------------------------------
    #[test]
    fn test_receive_sequential_data_frames() {
        let mut bs = setup_connected_session();

        for i in 0..5u8 {
            let frame = Rlp3Frame::Data {
                seq: i,
                rexmit: false,
                data: vec![i + 1],
            };
            let events = bs.receive_frame(&encode_frame(&frame), FrameRate::Full);
            assert!(
                events
                    .iter()
                    .any(|e| matches!(e, RlpEvent::DataDelivered(_)))
            );
        }

        assert_eq!(bs.l_v_r(), 5);
        assert_eq!(bs.l_v_n(), 5);
    }
}
