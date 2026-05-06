//! BTS-side traffic channel ARQ state machine.
//!
//! When BTS L2 Termination is active (Type A architecture per A.S0003-A §7.29),
//! the BSC sends L3 SDUs via Abis-IS-2000 FCH Fwd messages and the BTS handles
//! F-DSCH assembly, SAR fragmentation, and ARQ retransmission locally.

use std::collections::VecDeque;
use std::time::{Duration, Instant};

use cdma_common::bits::Bitstream;
use log::{info, warn};

use crate::lac::sar_fragment_ftch_pdu_dsch;

const FDSCH_MAX_OUTSTANDING: usize = 4;
const T3M_COOLDOWN: Duration = Duration::from_millis(320);

/// Events emitted by the traffic LAC to the AbisAgent.
#[derive(Debug)]
pub enum TrafficLacEvent {
    /// One or more 172-bit frames are ready to send on the air interface.
    FramesReady { frames: Vec<Bitstream> },
    /// An assured PDU was acknowledged by the MS.
    Delivered { correlation_id: u32 },
    /// An assured PDU exhausted retries without acknowledgment.
    Failed { correlation_id: u32 },
}

struct PendingRetry {
    correlation_id: u32,
    msg_seq: u8,
    wire_msg_type: u8,
    sdu_body: Bitstream,
    #[allow(dead_code)]
    first_tx_at: Instant,
    last_tx_at: Instant,
    retry_count: u32,
}

struct DeferredSend {
    correlation_id: u32,
    wire_msg_type: u8,
    sdu_body: Bitstream,
    ack_seq: u8,
    ack_req: bool,
}

/// Per-traffic-channel ARQ state managed by the BTS.
pub struct TrafficChannelArqState {
    forward_msg_seq_ack: u8,
    forward_msg_seq_noack: u8,
    noack_seq_last_used: [Option<Instant>; 8],
    pending_retries: Vec<PendingRetry>,
    deferred_sends: VecDeque<DeferredSend>,
    ack_timeout: Duration,
    max_retries: u32,
    current_reverse_ack_seq: u8,
}

/// Configuration for BTS traffic ARQ.
pub struct TrafficArqConfig {
    pub ack_timeout: Duration,
    pub max_retries: u32,
}

impl Default for TrafficArqConfig {
    fn default() -> Self {
        Self {
            ack_timeout: Duration::from_millis(3000),
            max_retries: 5,
        }
    }
}

impl TrafficChannelArqState {
    /// Creates a new ARQ state with the given configuration.
    pub fn new(config: TrafficArqConfig) -> Self {
        Self {
            forward_msg_seq_ack: 0,
            forward_msg_seq_noack: 0,
            noack_seq_last_used: [None; 8],
            pending_retries: Vec::new(),
            deferred_sends: VecDeque::new(),
            ack_timeout: config.ack_timeout,
            max_retries: config.max_retries,
            current_reverse_ack_seq: 0,
        }
    }

    /// Returns the last received reverse ACK_SEQ value.
    pub fn current_reverse_ack_seq(&self) -> u8 {
        self.current_reverse_ack_seq
    }

    /// Submits an L3 SDU for transmission on the forward traffic channel.
    ///
    /// `wire_msg_type` is the IS-2000 MSG_TYPE from the Air Interface Message IE.
    /// `sdu_body` is the encoded message body (without MSG_TYPE prefix or ARQ).
    /// The BTS adds ARQ fields (ACK_SEQ, MSG_SEQ, ACK_REQ), ENCRYPTION, and
    /// performs SAR fragmentation into 172-bit MuxPDU frames.
    pub fn submit_l3_sdu(
        &mut self,
        wire_msg_type: u8,
        sdu_body: Bitstream,
        ack_seq: u8,
        ack_req: bool,
        correlation_id: u32,
    ) -> Vec<TrafficLacEvent> {
        if ack_req && self.pending_retries.len() >= FDSCH_MAX_OUTSTANDING {
            info!(
                "traffic_lac: ARQ window full, deferring SDU corr={}",
                correlation_id
            );
            self.deferred_sends.push_back(DeferredSend {
                correlation_id,
                wire_msg_type,
                sdu_body,
                ack_seq,
                ack_req,
            });
            return Vec::new();
        }

        self.send_sdu(wire_msg_type, sdu_body, ack_seq, ack_req, correlation_id)
    }

    /// Processes a reverse ACK_SEQ from the MS, retiring acknowledged PDUs.
    pub fn on_reverse_ack_seq(&mut self, ack_seq: u8) -> Vec<TrafficLacEvent> {
        self.current_reverse_ack_seq = ack_seq;
        let mut events = Vec::new();

        self.pending_retries.retain(|entry| {
            if entry.msg_seq == ack_seq {
                info!(
                    "traffic_lac: PDU msg_seq={} acked, corr={}",
                    entry.msg_seq, entry.correlation_id
                );
                events.push(TrafficLacEvent::Delivered {
                    correlation_id: entry.correlation_id,
                });
                false
            } else {
                true
            }
        });

        if !events.is_empty() {
            events.extend(self.flush_deferred());
        }

        events
    }

    /// Checks for timed-out retries and retransmits or fails them.
    pub fn tick_retries(&mut self, now: Instant) -> Vec<TrafficLacEvent> {
        let mut events = Vec::new();
        let mut keep = Vec::new();

        for mut entry in self.pending_retries.drain(..) {
            if now.duration_since(entry.last_tx_at) < self.ack_timeout {
                keep.push(entry);
                continue;
            }

            if entry.retry_count >= self.max_retries {
                warn!(
                    "traffic_lac: PDU msg_seq={} exhausted retries, corr={}",
                    entry.msg_seq, entry.correlation_id
                );
                events.push(TrafficLacEvent::Failed {
                    correlation_id: entry.correlation_id,
                });
                continue;
            }

            let ack_seq = self.current_reverse_ack_seq;
            let pdu = assemble_f_dsch_pdu(
                entry.wire_msg_type,
                &entry.sdu_body,
                ack_seq,
                entry.msg_seq,
                true,
            );
            let frames = sar_fragment_ftch_pdu_dsch(&pdu);

            entry.retry_count += 1;
            entry.last_tx_at = now;

            info!(
                "traffic_lac: retransmit msg_seq={} attempt={} corr={}",
                entry.msg_seq, entry.retry_count, entry.correlation_id
            );

            events.push(TrafficLacEvent::FramesReady { frames });
            keep.push(entry);
        }

        self.pending_retries = keep;
        events
    }

    fn send_sdu(
        &mut self,
        wire_msg_type: u8,
        sdu_body: Bitstream,
        ack_seq: u8,
        ack_req: bool,
        correlation_id: u32,
    ) -> Vec<TrafficLacEvent> {
        let msg_seq = self.next_forward_msg_seq(ack_req);
        let pdu = assemble_f_dsch_pdu(wire_msg_type, &sdu_body, ack_seq, msg_seq, ack_req);
        let frames = sar_fragment_ftch_pdu_dsch(&pdu);

        if ack_req {
            let now = Instant::now();
            self.pending_retries.push(PendingRetry {
                correlation_id,
                msg_seq,
                wire_msg_type,
                sdu_body,
                first_tx_at: now,
                last_tx_at: now,
                retry_count: 0,
            });
        }

        vec![TrafficLacEvent::FramesReady { frames }]
    }

    fn flush_deferred(&mut self) -> Vec<TrafficLacEvent> {
        let mut events = Vec::new();
        while self.pending_retries.len() < FDSCH_MAX_OUTSTANDING {
            let Some(deferred) = self.deferred_sends.pop_front() else {
                break;
            };
            info!(
                "traffic_lac: flushing deferred SDU corr={}",
                deferred.correlation_id
            );
            events.extend(self.send_sdu(
                deferred.wire_msg_type,
                deferred.sdu_body,
                deferred.ack_seq,
                deferred.ack_req,
                deferred.correlation_id,
            ));
        }
        events
    }

    fn next_forward_msg_seq(&mut self, ack_req: bool) -> u8 {
        if ack_req {
            let seq = self.forward_msg_seq_ack;
            self.forward_msg_seq_ack = (seq + 1) % 8;
            seq
        } else {
            let now = Instant::now();
            let mut seq = self.forward_msg_seq_noack;
            for _ in 0..8 {
                if let Some(last) = self.noack_seq_last_used[seq as usize] {
                    if now.duration_since(last) < T3M_COOLDOWN {
                        seq = (seq + 1) % 8;
                        continue;
                    }
                }
                break;
            }
            self.noack_seq_last_used[seq as usize] = Some(now);
            self.forward_msg_seq_noack = (seq + 1) % 8;
            seq
        }
    }
}

/// Assembles an F-DSCH regular PDU from a wire MSG_TYPE and SDU body.
///
/// Per C.S0004-E 3.2.2.2.2 (P_REV_IN_USE < 9):
/// MSG_TYPE(8) + ACK_SEQ(3) + MSG_SEQ(3) + ACK_REQ(1) + ENCRYPTION(2) + SDU + PDU_PADDING
fn assemble_f_dsch_pdu(
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
