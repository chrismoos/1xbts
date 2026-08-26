//! RLP Type 2 session state machine per TIA/EIA/IS-707-A.8 Section 3.
//!
//! Implements the non-encrypted initialization/reset handshake
//! (SYNC → SYNC/ACK → ACK → data transfer) and primary-traffic octet
//! transfer for the BS/MSC side, using the Type 2 frame codec in
//! `crate::rlp2_frames`.
//!
//! Sequence counters are 12-bit (mod 4096) per IS-707-A.8; only the low 8
//! bits ride in the on-wire SEQ field. Selective retransmission (NAK
//! processing and receiver resequencing) is not yet implemented: received
//! data is delivered in arrival order and losses are left to the upper
//! layer. This is sufficient to establish the link and carry PPP over a
//! low-error channel; NAK-driven recovery is a follow-up.

use crate::rlp::RlpRate;
use crate::rlp2_frames::{self, Rlp2ControlType, Rlp2Frame};

/// 12-bit sequence-number modulus (IS-707-A.8).
const SEQ_MODULUS: u16 = 4096;
/// Round-trip frame counters required before advancing handshake phases:
/// SYNC→SYNC/ACK uses ≥45, SYNC/ACK→ACK uses ≥4 (IS-707-A.8 3.1.1).
const ROUND_TRIP_SYNC_ACK: u32 = 45;
const ROUND_TRIP_ACK: u32 = 4;
/// Consecutive erasures that force a reset (IS-707-A.8 3.1.3, E > 255).
const MAX_CONSECUTIVE_ERASURES: u32 = 255;
/// Full-rate unsegmented Format A payload budget for Rate Set 2.
const RS2_FULL_MAX_DATA_OCTETS: usize = 31;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rlp2State {
    Uninitialized,
    Sync,
    SyncAck,
    Ack,
    DataTransfer,
}

/// RLP Type 2 session (BS/MSC side), driven one 20 ms frame at a time.
pub struct Rlp2Session {
    state: Rlp2State,
    /// 12-bit send/receive/needed counters.
    l_v_s: u16,
    l_v_r: u16,
    handshake_frames_sent: u32,
    round_trip_counter: u32,
    consecutive_erasures: u32,
    tx_queue: Vec<u8>,
}

impl Default for Rlp2Session {
    fn default() -> Self {
        Self::new()
    }
}

impl Rlp2Session {
    pub fn new() -> Self {
        Self {
            state: Rlp2State::Uninitialized,
            l_v_s: 0,
            l_v_r: 0,
            handshake_frames_sent: 0,
            round_trip_counter: 0,
            consecutive_erasures: 0,
            tx_queue: Vec::new(),
        }
    }

    pub fn state(&self) -> Rlp2State {
        self.state
    }

    pub fn is_data_transfer(&self) -> bool {
        self.state == Rlp2State::DataTransfer
    }

    pub fn enqueue_data(&mut self, data: &[u8]) {
        self.tx_queue.extend_from_slice(data);
    }

    pub fn tx_queue_len(&self) -> usize {
        self.tx_queue.len()
    }

    fn seq8(&self) -> u8 {
        (self.l_v_s & 0xFF) as u8
    }

    fn initialize(&mut self) {
        self.l_v_s = 0;
        self.l_v_r = 0;
        self.handshake_frames_sent = 0;
        self.round_trip_counter = 0;
        self.consecutive_erasures = 0;
        self.tx_queue.clear();
        self.state = Rlp2State::Sync;
    }

    /// Process a received frame (uplink). `None` = erasure. Returns delivered
    /// upper-layer octets, if any.
    pub fn receive_frame(&mut self, frame: Option<&Rlp2Frame>) -> Option<Vec<u8>> {
        if self.state == Rlp2State::Uninitialized {
            self.initialize();
        }

        let frame = match frame {
            Some(f) => f,
            None => {
                self.consecutive_erasures += 1;
                if self.consecutive_erasures > MAX_CONSECUTIVE_ERASURES {
                    self.initialize();
                }
                return None;
            }
        };
        self.consecutive_erasures = 0;

        match self.state {
            Rlp2State::Uninitialized => None,

            Rlp2State::Sync => {
                match frame {
                    Rlp2Frame::Control {
                        control_type: Rlp2ControlType::Sync,
                        ..
                    } => {
                        log::info!("RLP2: received SYNC from peer, sending SYNC/ACK");
                        self.round_trip_counter = ROUND_TRIP_SYNC_ACK;
                        self.handshake_frames_sent = 0;
                        self.state = Rlp2State::SyncAck;
                    }
                    Rlp2Frame::Control {
                        control_type: Rlp2ControlType::SyncAck,
                        ..
                    } => {
                        log::info!("RLP2: received SYNC/ACK from peer, sending ACK");
                        self.round_trip_counter = ROUND_TRIP_ACK;
                        self.handshake_frames_sent = 0;
                        self.state = Rlp2State::Ack;
                    }
                    _ => {}
                }
                None
            }

            Rlp2State::SyncAck => match frame {
                Rlp2Frame::Control {
                    control_type: Rlp2ControlType::Sync,
                    ..
                } => {
                    self.handshake_frames_sent = 0;
                    None
                }
                Rlp2Frame::Control {
                    control_type: Rlp2ControlType::Ack | Rlp2ControlType::SyncAck,
                    ..
                } => {
                    self.enter_data_transfer();
                    None
                }
                other if !is_sync(other) => {
                    self.enter_data_transfer();
                    self.deliver(other)
                }
                _ => None,
            },

            Rlp2State::Ack => match frame {
                Rlp2Frame::Control {
                    control_type: Rlp2ControlType::SyncAck,
                    ..
                } => {
                    self.handshake_frames_sent = 0;
                    None
                }
                Rlp2Frame::Control {
                    control_type: Rlp2ControlType::Sync,
                    ..
                } => {
                    self.initialize();
                    None
                }
                other if !is_sync_ack(other) => {
                    self.enter_data_transfer();
                    self.deliver(other)
                }
                _ => None,
            },

            Rlp2State::DataTransfer => {
                if is_sync(frame) {
                    self.initialize();
                    return None;
                }
                self.deliver(frame)
            }
        }
    }

    /// Produce the next downlink frame and the rate to encode it at.
    pub fn next_frame(&mut self) -> (Rlp2Frame, RlpRate) {
        if self.state == Rlp2State::Uninitialized {
            self.initialize();
        }
        match self.state {
            Rlp2State::Uninitialized => (rlp2_frames::idle_frame(0), RlpRate::Eighth),
            Rlp2State::Sync => {
                self.handshake_frames_sent += 1;
                (rlp2_frames::sync_frame(self.seq8()), RlpRate::Full)
            }
            Rlp2State::SyncAck => {
                self.handshake_frames_sent += 1;
                (rlp2_frames::sync_ack_frame(self.seq8()), RlpRate::Full)
            }
            Rlp2State::Ack => {
                self.handshake_frames_sent += 1;
                if self.handshake_frames_sent > self.round_trip_counter {
                    self.enter_data_transfer();
                    return self.build_data_frame();
                }
                (rlp2_frames::ack_frame(self.seq8()), RlpRate::Full)
            }
            Rlp2State::DataTransfer => self.build_data_frame(),
        }
    }

    fn build_data_frame(&mut self) -> (Rlp2Frame, RlpRate) {
        if self.tx_queue.is_empty() {
            return (rlp2_frames::idle_frame(self.seq8()), RlpRate::Eighth);
        }
        let take = self.tx_queue.len().min(RS2_FULL_MAX_DATA_OCTETS);
        let chunk: Vec<u8> = self.tx_queue.drain(..take).collect();
        let frame = rlp2_frames::data_frame(self.seq8(), &chunk);
        // L_V(S) advances only after a data frame with non-zero payload.
        self.l_v_s = (self.l_v_s + 1) % SEQ_MODULUS;
        (frame, RlpRate::Full)
    }

    fn enter_data_transfer(&mut self) {
        if self.state != Rlp2State::DataTransfer {
            log::info!("RLP2: entering DataTransfer state");
        }
        self.state = Rlp2State::DataTransfer;
    }

    /// Deliver the payload of a data frame to the upper layer, advancing the
    /// 12-bit receive counter. In-order delivery only (no resequencing yet).
    fn deliver(&mut self, frame: &Rlp2Frame) -> Option<Vec<u8>> {
        match frame {
            Rlp2Frame::Data { data, .. } | Rlp2Frame::DataFormatB { data, .. }
                if !data.is_empty() =>
            {
                self.l_v_r = (self.l_v_r + 1) % SEQ_MODULUS;
                Some(data.clone())
            }
            _ => None,
        }
    }
}

fn is_sync(frame: &Rlp2Frame) -> bool {
    matches!(
        frame,
        Rlp2Frame::Control {
            control_type: Rlp2ControlType::Sync,
            ..
        }
    )
}

fn is_sync_ack(frame: &Rlp2Frame) -> bool {
    matches!(
        frame,
        Rlp2Frame::Control {
            control_type: Rlp2ControlType::SyncAck,
            ..
        }
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::rlp::RlpMuxOption;
    use crate::rlp2_frames;

    // Encode a frame the way the peer would, then decode it back the way the
    // session's backend does, to exercise the real wire path.
    fn wire(frame: &Rlp2Frame, rate: RlpRate) -> Rlp2Frame {
        let bits = rlp2_frames::encode_frame_for_mux(frame, rate, RlpMuxOption::Two).unwrap();
        rlp2_frames::decode_frame_for_mux(&bits, rate, RlpMuxOption::Two).unwrap()
    }

    #[test]
    fn bs_initiated_handshake_reaches_data_transfer() {
        let mut s = Rlp2Session::new();
        // First downlink is a SYNC.
        let (f, rate) = s.next_frame();
        assert!(matches!(
            f,
            Rlp2Frame::Control {
                control_type: Rlp2ControlType::Sync,
                ..
            }
        ));
        assert_eq!(rate, RlpRate::Full);
        // Mobile answers SYNC/ACK -> we go to Ack.
        s.receive_frame(Some(&wire(&rlp2_frames::sync_ack_frame(0), RlpRate::Full)));
        assert_eq!(s.state(), Rlp2State::Ack);
        // We emit ACKs; a mobile data/idle frame completes the handshake.
        let _ = s.next_frame();
        s.receive_frame(Some(&wire(&rlp2_frames::idle_frame(0), RlpRate::Eighth)));
        assert!(s.is_data_transfer());
    }

    #[test]
    fn mobile_initiated_handshake_reaches_data_transfer() {
        let mut s = Rlp2Session::new();
        s.receive_frame(Some(&wire(&rlp2_frames::sync_frame(0), RlpRate::Full)));
        assert_eq!(s.state(), Rlp2State::SyncAck);
        s.receive_frame(Some(&wire(&rlp2_frames::ack_frame(0), RlpRate::Full)));
        assert!(s.is_data_transfer());
    }

    #[test]
    fn data_transfer_carries_octets_both_ways() {
        let mut s = Rlp2Session::new();
        s.receive_frame(Some(&wire(&rlp2_frames::sync_frame(0), RlpRate::Full)));
        s.receive_frame(Some(&wire(&rlp2_frames::ack_frame(0), RlpRate::Full)));
        assert!(s.is_data_transfer());
        // Downlink octets come back out as a data frame.
        s.enqueue_data(b"hello");
        let (f, rate) = s.next_frame();
        assert_eq!(rate, RlpRate::Full);
        assert!(matches!(f, Rlp2Frame::Data { ref data, .. } if data == b"hello"));
        // Uplink data frame is delivered up.
        let up = wire(&rlp2_frames::data_frame(1, b"world"), RlpRate::Full);
        assert_eq!(s.receive_frame(Some(&up)), Some(b"world".to_vec()));
    }

    #[test]
    fn erasures_do_not_reset_handshake_prematurely() {
        let mut s = Rlp2Session::new();
        s.receive_frame(Some(&wire(&rlp2_frames::sync_ack_frame(0), RlpRate::Full)));
        assert_eq!(s.state(), Rlp2State::Ack);
        for _ in 0..100 {
            s.receive_frame(None);
        }
        assert_eq!(s.state(), Rlp2State::Ack);
    }
}
