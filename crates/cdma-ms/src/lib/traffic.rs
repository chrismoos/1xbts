use std::collections::VecDeque;

use cdma_bts::channels::rtch_rc3::{self, Rc3Frame, Rc3Rate};
use cdma_bts::lac::sar_fragment_ftch_pdu_dsch;
use cdma_bts::receiver::access_layer3::{FdschMessage, FdschPdu};
use cdma_bts::receiver::layer3::SystemParametersMessage;
use cdma_common::access::{
    AccessMessage, AccessMessageHeader, BdtmfmOffLength, BdtmfmOnLength,
    PowerMeasurementReportMessage, SendBurstDtmfMessage, bdtmfm_digit,
};
use cdma_common::bits::Bitstream;
use cdma_voice::{SAMPLES_PER_FRAME, VoiceCodec, VoiceDecoder, VoiceEncoder, VoiceRate};

use crate::forward_traffic_rx::UnsupportedForwardPdu;
use cdma_common::consts::{BURST_TYPE_SMS, SERVICE_OPTION_SMS};
use cdma_common::lac::message_types::{MessageId, WireChannel};
use cdma_common::sms::encode_mo_sms_submit;
use num_complex::Complex32;

use crate::forward_rx::directed::{
    ORDER_BS_ACK, ORDER_RELEASE, ORDER_SERVICE_OPTION_REQUEST, ORDER_SERVICE_OPTION_RESPONSE,
    order_name,
};
use crate::lac::{
    MsgSeqCounters, N1M_MAX_TRANSMISSIONS, PendingRdsch, ReceivedSeqs, T1M_RETRANSMIT_MS,
};
use crate::ms::ChannelAssignment;
use crate::traffic_tx::build_reverse_data_burst_pdu;

pub const FRAME_CHIPS: u64 = rtch_rc3::RC3_FRAME_CHIPS as u64;
/// Chips per 1.25 ms traffic frame offset.
const FRAME_OFFSET_CHIPS: u64 = FRAME_CHIPS / 16;

pub(crate) fn next_traffic_frame_chip(
    at_or_after_chip: u64,
    grid_origin_chip: u64,
    frame_offset: u8,
) -> u64 {
    let first_chip = grid_origin_chip + u64::from(frame_offset) * FRAME_OFFSET_CHIPS;
    first_chip
        + at_or_after_chip
            .saturating_sub(first_chip)
            .div_ceil(FRAME_CHIPS)
            * FRAME_CHIPS
}

const PN_PERIOD: u64 = 32_768;
const CHUNK_FRAMES: usize = 5;
pub const PREAMBLE_FRAMES: u64 = 30;
/// T50m bounds forward traffic acquisition when no F-CPCCH is assigned.
const TRAFFIC_INITIALIZATION_CHIPS: u64 = 1_228_800;
/// T65m wait for a Service Connect Message after traffic initialization.
const SERVICE_WAIT_FRAMES: u64 = 250;
/// ACK_SEQ carried while no forward PDU has asked for an acknowledgment.
const ACK_SEQ_NONE: u8 = 0b111;
/// Frames the transmitter keeps running after the last frame of a release,
/// so the base station decodes it before the carrier drops.
const RELEASE_TAIL_FRAMES: u64 = 4;
/// T55m bounds the Release Substate while the MS waits for the BS Release Order.
const RELEASE_SUBSTATE_FRAMES: u64 = 100;
/// Reverse order codes (C.S0005-E Table 2.7.3-1).
const ORDER_MS_ACK: u8 = 0b010000;
const ORDER_MS_RELEASE: u8 = 0b010101;
const ORDER_CONNECT: u8 = 0b011000;
const NO_ORDER_SPECIFIC_FIELDS: u8 = 0;
const RESERVED_BIT: u8 = 0;
const SINGLE_BIT: usize = 1;
const SERVICE_CONNECT_SEQUENCE_BITS: usize = 3;
const SERVICE_CONNECT_SEQUENCE_MASK: u8 = (1 << SERVICE_CONNECT_SEQUENCE_BITS) - 1;
const SMS_SESSION_MAX_FRAMES: u64 = 750;
/// C.S0015-B §3.3.1 gives the base station 18 seconds to return an SMS acknowledgment.
const SMS_CAUSE_WAIT_FRAMES: u64 = 1_000;
const SMS_MESSAGE_ID: u16 = 1;
/// Frames per T1m at 20 ms per frame.
const T1M_FRAMES: u64 = T1M_RETRANSMIT_MS / 20;
const LIVE_IQ_POLARITY: f32 = -1.0;
const OVERSAMPLE: usize = 4;
const POWER_REPORT_DELAY_UNIT_FRAMES: u64 = 4;

const POWER_CONTROL_STEP_ONE_DB: u8 = 0b000;
const POWER_CONTROL_STEP_HALF_DB: u8 = 0b001;
const POWER_CONTROL_STEP_QUARTER_DB: u8 = 0b010;
const POWER_CONTROL_ONE_DB: f32 = 1.0;
const POWER_CONTROL_HALF_DB: f32 = 0.5;
const POWER_CONTROL_QUARTER_DB: f32 = 0.25;
const DEFAULT_POWER_CONTROL_STEP_DB: f32 = POWER_CONTROL_ONE_DB;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutgoingSms {
    pub destination: String,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum TrafficOutcome {
    Message {
        name: String,
        order: Option<String>,
        msg_seq: u8,
        ack_seq: u8,
        ack_req: bool,
    },
    ServiceConnected {
        service_option: u16,
    },
    Ringing {
        caller_number: Option<String>,
    },
    DataBurst {
        burst_type: u8,
        fields: Vec<u8>,
    },
    StatusRequested {
        qualification_type: u8,
        qualification: Vec<u8>,
        record_types: Vec<u8>,
    },
    ReversePowerStep {
        db: f32,
    },
    Released {
        by: &'static str,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrafficTxNotice {
    pub name: &'static str,
    pub msg_seq: u8,
    pub ack_seq: u8,
    pub ack_req: bool,
    pub retransmission: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PowerReportParameters {
    threshold: Option<u8>,
    measurement_frames: u64,
    periodic: bool,
    delay_frames: u64,
}

impl PowerReportParameters {
    pub fn from_system_parameters(parameters: &SystemParametersMessage) -> Option<Self> {
        if !parameters.pwr_thresh_enable && !parameters.pwr_period_enable {
            return None;
        }
        Some(Self {
            threshold: parameters
                .pwr_thresh_enable
                .then_some(parameters.pwr_rep_thresh),
            measurement_frames: power_report_frame_count(parameters.pwr_rep_frames),
            periodic: parameters.pwr_period_enable,
            delay_frames: u64::from(parameters.pwr_rep_delay) * POWER_REPORT_DELAY_UNIT_FRAMES,
        })
    }

    fn from_traffic_message(
        parameters: &cdma_common::access::PowerControlParametersMessage,
    ) -> Option<Self> {
        if !parameters.threshold_enabled && !parameters.periodic_enabled {
            return None;
        }
        Some(Self {
            threshold: parameters
                .threshold_enabled
                .then_some(parameters.report_threshold),
            measurement_frames: power_report_frame_count(parameters.report_frames),
            periodic: parameters.periodic_enabled,
            delay_frames: u64::from(parameters.report_delay) * POWER_REPORT_DELAY_UNIT_FRAMES,
        })
    }
}

fn power_report_frame_count(encoded: u8) -> u64 {
    const COUNTS: [u64; 16] = [
        5, 7, 10, 14, 20, 28, 40, 56, 80, 113, 160, 226, 320, 452, 640, 905,
    ];
    COUNTS[usize::from(encoded.min(15))]
}

#[derive(Debug, Clone)]
pub struct TrafficChunk {
    pub chip: u64,
    pub samples: Vec<Complex32>,
    pub end_of_burst: bool,
}

struct OutPdu {
    name: &'static str,
    bits: Bitstream,
    ack_seq: u8,
    msg_seq: u8,
    ack_req: bool,
}

const SEND_BURST_DTMF: &str = "Send Burst DTMF";

pub struct DtmfBurst {
    body: Bitstream,
}

impl DtmfBurst {
    /// Accepts 1–255 digits (0–9, *, #), with 95 ms tones and 60 ms gaps.
    pub fn new(digits: &str) -> Result<Self, String> {
        if digits.is_empty() || digits.len() > u8::MAX as usize {
            return Err("DTMF requires 1–255 digits".into());
        }
        let digits = digits
            .bytes()
            .map(|digit| match digit {
                b'1'..=b'9' => Ok(digit - b'0'),
                b'0' => Ok(bdtmfm_digit::ZERO),
                b'*' => Ok(bdtmfm_digit::STAR),
                b'#' => Ok(bdtmfm_digit::POUND),
                _ => Err("DTMF accepts only 0–9, *, and #".to_string()),
            })
            .collect::<Result<Vec<_>, _>>()?;
        let body = AccessMessage::SendBurstDtmf(SendBurstDtmfMessage {
            header: AccessMessageHeader {
                pd: 0,
                message_id: MessageId::SendBurstDtmf,
            },
            digits,
            dtmf_on_length: BdtmfmOnLength::Ms95 as u8,
            dtmf_off_length: BdtmfmOffLength::Ms60 as u8,
            con_ref: None,
            remaining_bits: 0,
        })
        .to_rdsch_sdu()?;
        Ok(Self { body })
    }
}

pub struct TrafficSession {
    esn: u32,
    assignment: ChannelAssignment,
    service_option: u16,
    next_frame_chip: u64,
    frames_sent: u64,
    outbox: VecDeque<OutPdu>,
    frame_queue: VecDeque<Vec<u8>>,
    seq: MsgSeqCounters,
    rcvd: ReceivedSeqs,
    pending: Option<PendingRdsch>,
    last_rx_ack_seq: u8,
    sms: Option<OutgoingSms>,
    sms_sent: bool,
    sms_cause_deadline_frame: Option<u64>,
    forward_confirmed: bool,
    initialization_deadline_chip: u64,
    /// MSG_SEQ of the Release Order, once asked for. The transmission runs
    /// until the base station acknowledges it.
    release_msg_seq: Option<u8>,
    release_deadline_chip: Option<u64>,
    /// Already acknowledged MSG_SEQ. An in-flight copy must not re-arm its retransmit timer.
    acked_msg_seq: Option<u8>,
    connected: bool,
    incoming: bool,
    ringing: bool,
    answered: bool,
    end_chip: Option<u64>,
    released_by: Option<&'static str>,
    ended: bool,
    microphone: VecDeque<[i16; SAMPLES_PER_FRAME]>,
    voice_encoder: Option<VoiceEncoder>,
    voice_decoder: Option<VoiceDecoder>,
    voice_started: bool,
    tx_notices: VecDeque<TrafficTxNotice>,
    power_report: Option<PowerReportParameters>,
    measured_frames: u32,
    measured_bad_frames: u32,
    active_pilot_strength: u8,
    last_power_report_frame: Option<u64>,
}

impl TrafficSession {
    pub fn new(
        esn: u32,
        assignment: ChannelAssignment,
        service_option: u16,
        sms: Option<OutgoingSms>,
        incoming: bool,
        power_report: Option<PowerReportParameters>,
        now: u64,
        start_delay: u64,
    ) -> Self {
        let first = next_traffic_frame_chip(now + start_delay, 0, assignment.frame_offset);
        let voice_codec = VoiceCodec::from_service_option(service_option);
        TrafficSession {
            esn,
            assignment,
            service_option,
            next_frame_chip: first,
            frames_sent: 0,
            outbox: VecDeque::new(),
            frame_queue: VecDeque::new(),
            seq: MsgSeqCounters::default(),
            rcvd: ReceivedSeqs::default(),
            pending: None,
            last_rx_ack_seq: ACK_SEQ_NONE,
            sms,
            sms_sent: false,
            sms_cause_deadline_frame: None,
            forward_confirmed: false,
            initialization_deadline_chip: now.saturating_add(TRAFFIC_INITIALIZATION_CHIPS),
            release_msg_seq: None,
            release_deadline_chip: None,
            acked_msg_seq: None,
            connected: false,
            incoming,
            ringing: false,
            answered: false,
            end_chip: None,
            released_by: None,
            ended: false,
            microphone: VecDeque::new(),
            voice_encoder: voice_codec.and_then(|codec| VoiceEncoder::new(codec).ok()),
            voice_decoder: voice_codec.and_then(|codec| VoiceDecoder::new(codec).ok()),
            voice_started: false,
            tx_notices: VecDeque::new(),
            power_report,
            measured_frames: 0,
            measured_bad_frames: 0,
            active_pilot_strength: 0,
            last_power_report_frame: None,
        }
    }

    pub fn assignment(&self) -> &ChannelAssignment {
        &self.assignment
    }

    pub fn start_chip(&self) -> u64 {
        self.next_frame_chip - self.frames_sent * FRAME_CHIPS
    }

    pub fn up(&self) -> bool {
        self.forward_confirmed && !self.in_preamble()
    }

    /// Reverse acquisition is independent of forward N5m. Preserve the assigned preamble length.
    fn in_preamble(&self) -> bool {
        self.frames_sent < PREAMBLE_FRAMES || !self.forward_confirmed
    }

    pub fn set_forward_confirmed(&mut self, confirmed: bool) {
        self.forward_confirmed = confirmed;
    }

    pub fn ended(&self) -> bool {
        self.ended
    }

    pub fn released_by(&self) -> Option<&'static str> {
        self.released_by
    }

    pub fn connected(&self) -> bool {
        self.connected
    }

    pub fn send_dtmf_burst(&mut self, burst: DtmfBurst) -> Result<(), String> {
        if !self.connected
            || VoiceCodec::from_service_option(self.service_option).is_none()
            || (self.incoming && !self.answered)
            || self.release_msg_seq.is_some()
            || self.end_chip.is_some()
            || self.ended
        {
            return Err("DTMF requires an active voice call".into());
        }
        if self
            .pending
            .as_ref()
            .is_some_and(|pdu| pdu.name == SEND_BURST_DTMF)
            || self.outbox.iter().any(|pdu| pdu.name == SEND_BURST_DTMF)
        {
            return Err("a DTMF burst is still awaiting acknowledgment".into());
        }
        self.queue_pdu(SEND_BURST_DTMF, burst.body, true);
        Ok(())
    }

    pub fn answer(&mut self) -> bool {
        if !self.incoming || !self.ringing || self.answered {
            return false;
        }
        self.queue_pdu("Connect Order", connect_order_body(), true);
        self.answered = true;
        true
    }

    pub fn push_microphone(&mut self, pcm: [i16; SAMPLES_PER_FRAME]) {
        const MAX_QUEUED_FRAMES: usize = 10;
        if self.microphone.len() == MAX_QUEUED_FRAMES {
            self.microphone.pop_front();
        }
        self.microphone.push_back(pcm);
    }

    pub fn decode_voice(&mut self, rate_bps: u32, bits: &[u8]) -> Option<[i16; SAMPLES_PER_FRAME]> {
        let codec = VoiceCodec::from_service_option(self.service_option)?;
        let rate = codec.rate_from_bps(rate_bps)?;
        let payload = cdma_voice::pack_voice_bits(bits, codec.primary_traffic_bits(rate));
        let decoder = self.voice_decoder.as_mut()?;
        let decoded = decoder.decode(rate, &payload);
        match decoded {
            Ok(pcm) => {
                self.voice_started = true;
                Some(pcm)
            }
            Err(error) => {
                log::warn!("cdma-ms: forward voice decode failed: {error}");
                None
            }
        }
    }

    pub fn decode_erasure(&mut self) -> Option<[i16; SAMPLES_PER_FRAME]> {
        if !self.voice_started {
            return None;
        }
        self.voice_decoder.as_mut()?.decode_erasure()
    }

    /// Keep transmitting until the Release Order is acknowledged, or the cell may retain the channel.
    pub fn release(&mut self) {
        if self.end_chip.is_none() && self.release_msg_seq.is_none() {
            self.pending = None;
            self.outbox.clear();
            self.frame_queue.clear();
            let msg_seq = self.queue_pdu("Release Order", release_order_body(), true);
            self.release_msg_seq = Some(msg_seq);
            self.release_deadline_chip = Some(
                self.next_frame_chip
                    .saturating_add(RELEASE_SUBSTATE_FRAMES * FRAME_CHIPS),
            );
            self.released_by = Some("mobile");
        }
    }

    pub fn complete_sms(&mut self) {
        if !self.sms_sent || self.release_msg_seq.is_some() {
            return;
        }
        self.pending = None;
        self.release();
    }

    fn end_after_outbox(&mut self) {
        let queued_frames = self.frame_queue.len() as u64
            + self
                .outbox
                .iter()
                .map(|p| sar_fragment_ftch_pdu_dsch(&byte_aligned(&p.bits)).len() as u64)
                .sum::<u64>();
        self.end_chip =
            Some(self.next_frame_chip + (queued_frames + RELEASE_TAIL_FRAMES) * FRAME_CHIPS);
    }

    fn queue_pdu(&mut self, name: &'static str, body: Bitstream, ack_req: bool) -> u8 {
        let msg_seq = self.seq.next(ack_req);
        let bits = rdsch_pdu(name, &body, self.last_rx_ack_seq, msg_seq, ack_req);
        self.outbox.push_back(OutPdu {
            name,
            bits,
            ack_seq: self.last_rx_ack_seq,
            msg_seq,
            ack_req,
        });
        self.tx_notices.push_back(TrafficTxNotice {
            name,
            msg_seq,
            ack_seq: self.last_rx_ack_seq,
            ack_req,
            retransmission: false,
        });
        msg_seq
    }

    pub fn drain_tx_notices(&mut self) -> impl Iterator<Item = TrafficTxNotice> + '_ {
        self.tx_notices.drain(..)
    }

    pub fn record_forward_measurements(
        &mut self,
        frames: u32,
        bad_frames: u32,
        active_pilot_strength: u8,
    ) {
        self.active_pilot_strength = active_pilot_strength;
        if self.power_report.is_some_and(|parameters| {
            self.last_power_report_frame
                .is_some_and(|last| self.frames_sent.saturating_sub(last) < parameters.delay_frames)
        }) {
            return;
        }
        self.measured_frames = self.measured_frames.saturating_add(frames);
        self.measured_bad_frames = self.measured_bad_frames.saturating_add(bad_frames);
    }

    pub fn poll(
        &mut self,
        now: u64,
        lead: u64,
        lc_state_at: &dyn Fn(u64) -> u64,
    ) -> Vec<TrafficChunk> {
        let mut chunks = Vec::new();
        if self.ended {
            return chunks;
        }
        if self
            .release_deadline_chip
            .is_some_and(|deadline| now >= deadline)
            && self.end_chip.is_none()
        {
            log::info!("cdma-ms: T55m expired while waiting for the base-station Release Order");
            self.released_by = Some("release timeout");
            self.end_after_outbox();
        }
        if !self.forward_confirmed && now >= self.initialization_deadline_chip {
            log::warn!("cdma-ms: forward traffic channel not acquired within T50m");
            self.released_by = Some("traffic initialization timeout");
            self.ended = true;
            chunks.push(TrafficChunk {
                chip: now,
                samples: Vec::new(),
                end_of_burst: true,
            });
            return chunks;
        }
        self.release_if_sms_session_overran();
        self.retransmit_if_due();
        self.maybe_send_power_report();
        self.maybe_send_sms();
        log::debug!(
            "cdma-ms: traffic answers {:.0} ms out",
            self.next_frame_chip.saturating_sub(now) as f64 / 1228.8
        );
        let horizon = now + lead + CHUNK_FRAMES as u64 * FRAME_CHIPS;
        while self.next_frame_chip < horizon && !self.ended {
            let chip = self.next_frame_chip;
            let mut samples = Vec::with_capacity(CHUNK_FRAMES * FRAME_CHIPS as usize);
            let mut end_of_burst = false;
            for _ in 0..CHUNK_FRAMES {
                let frame_chip = self.next_frame_chip;
                if self.end_chip.is_some_and(|end| frame_chip >= end) {
                    end_of_burst = true;
                    self.ended = true;
                    break;
                }
                let frame = self.next_frame();
                // The HPSK spreader takes one long code chip as its delayed
                // reference before chip zero, so the register is seeded one
                // chip early.
                let lc_state = lc_state_at(frame_chip.saturating_sub(1));
                samples.extend(rtch_rc3::build_reverse_rc3_frame_samples(
                    &frame,
                    self.esn,
                    lc_state,
                    (frame_chip % PN_PERIOD) as usize,
                    OVERSAMPLE,
                    LIVE_IQ_POLARITY,
                ));
                self.next_frame_chip += FRAME_CHIPS;
                self.frames_sent += 1;
            }
            if !samples.is_empty() || end_of_burst {
                chunks.push(TrafficChunk {
                    chip,
                    samples,
                    end_of_burst,
                });
            }
        }
        chunks
    }

    fn next_frame(&mut self) -> Rc3Frame {
        if self.in_preamble() {
            return Rc3Frame::PilotOnly;
        }
        if self.frame_queue.is_empty() {
            let next_index = if let Some(pending) = &self.pending {
                self.outbox
                    .iter()
                    .position(|pdu| !pdu.ack_req || pdu.msg_seq == pending.msg_seq)
            } else {
                (!self.outbox.is_empty()).then_some(0)
            };
            if let Some(pdu) = next_index.and_then(|index| self.outbox.remove(index)) {
                let frames = sar_fragment_ftch_pdu_dsch(&byte_aligned(&pdu.bits));
                log::info!(
                    "cdma-ms: traffic tx {} msg_seq={} ack_seq={} ack_req={} ({} frame{})",
                    pdu.name,
                    pdu.msg_seq,
                    pdu.ack_seq,
                    pdu.ack_req,
                    frames.len(),
                    if frames.len() == 1 { "" } else { "s" }
                );
                for f in &frames {
                    self.frame_queue.push_back(f.bits().to_vec());
                }
                if pdu.ack_req && self.acked_msg_seq != Some(pdu.msg_seq) {
                    let transmissions = self
                        .pending
                        .as_ref()
                        .filter(|p| p.msg_seq == pdu.msg_seq)
                        .map(|p| p.transmissions)
                        .unwrap_or(0)
                        + 1;
                    self.pending = Some(PendingRdsch {
                        name: pdu.name,
                        ack_seq: pdu.ack_seq,
                        msg_seq: pdu.msg_seq,
                        pdu_bits: pdu.bits.bits().to_vec(),
                        transmissions,
                        last_tx_chip: self.next_frame_chip,
                    });
                }
            }
        }
        match self.frame_queue.pop_front() {
            Some(info_bits) => Rc3Frame::Traffic {
                rate: Rc3Rate::Full,
                info_bits,
            },
            // Eighth-rate null frames keep R-FCH continuous between signaling messages.
            None => self
                .next_voice_frame()
                .unwrap_or_else(|| Rc3Frame::Traffic {
                    rate: Rc3Rate::Eighth,
                    info_bits: vec![0; Rc3Rate::Eighth.info_bits()],
                }),
        }
    }

    fn next_voice_frame(&mut self) -> Option<Rc3Frame> {
        let encoder = self.voice_encoder.as_mut()?;
        let pcm = self
            .microphone
            .pop_front()
            .unwrap_or([0; SAMPLES_PER_FRAME]);
        let (voice_rate, payload) = encoder.encode(&pcm).ok()?;
        let codec = encoder.codec();
        let mut info_bits =
            cdma_voice::unpack_voice_bits(&payload, codec.primary_traffic_bits(voice_rate));
        let rate = match voice_rate {
            VoiceRate::Full => {
                info_bits.insert(0, 0);
                Rc3Rate::Full
            }
            VoiceRate::Half => Rc3Rate::Half,
            VoiceRate::Quarter => Rc3Rate::Quarter,
            VoiceRate::Eighth => Rc3Rate::Eighth,
        };
        Some(Rc3Frame::Traffic { rate, info_bits })
    }

    fn release_if_sms_session_overran(&mut self) {
        let deadline = self
            .sms_cause_deadline_frame
            .unwrap_or(SMS_SESSION_MAX_FRAMES);
        if self.service_option != SERVICE_OPTION_SMS
            || self.frames_sent < deadline
            || self.end_chip.is_some()
            || self.release_msg_seq.is_some()
        {
            return;
        }
        log::warn!(
            "cdma-ms: SMS traffic session ran {} frames with no release, releasing it",
            self.frames_sent
        );
        self.release();
    }

    fn retransmit_if_due(&mut self) {
        let Some(pending) = self.pending.clone() else {
            return;
        };
        if self
            .outbox
            .iter()
            .any(|p| p.msg_seq == pending.msg_seq && p.ack_req)
        {
            return;
        }
        if self.next_frame_chip < pending.last_tx_chip + T1M_FRAMES * FRAME_CHIPS {
            return;
        }
        if pending.transmissions >= N1M_MAX_TRANSMISSIONS {
            log::warn!(
                "cdma-ms: {} unacknowledged after {} transmissions, releasing",
                pending.name,
                pending.transmissions
            );
            let was_release = Some(pending.msg_seq) == self.release_msg_seq;
            self.pending = None;
            if was_release {
                self.end_after_outbox();
            } else {
                self.release();
            }
            return;
        }
        log::info!(
            "cdma-ms: retransmitting {} msg_seq={} (attempt {})",
            pending.name,
            pending.msg_seq,
            pending.transmissions + 1
        );
        self.outbox.push_back(OutPdu {
            name: pending.name,
            bits: Bitstream::new_init(&pending.pdu_bits),
            ack_seq: pending.ack_seq,
            msg_seq: pending.msg_seq,
            ack_req: true,
        });
        self.tx_notices.push_back(TrafficTxNotice {
            name: pending.name,
            msg_seq: pending.msg_seq,
            ack_seq: self.last_rx_ack_seq,
            ack_req: true,
            retransmission: true,
        });
    }

    fn maybe_send_sms(&mut self) {
        if self.sms_sent || !self.up() || self.end_chip.is_some() {
            return;
        }
        if !self.connected && self.frames_sent < PREAMBLE_FRAMES + SERVICE_WAIT_FRAMES {
            return;
        }
        let Some(sms) = self.sms.clone() else {
            return;
        };
        if !self.connected {
            log::info!("cdma-ms: no service connection yet, sending the SMS anyway");
        }
        let sms_bytes = encode_mo_sms_submit(&sms.destination, &sms.text, SMS_MESSAGE_ID);
        let msg_seq = self.seq.next(true);
        let pdu = build_reverse_data_burst_pdu(
            &sms_bytes,
            BURST_TYPE_SMS,
            1,
            msg_seq,
            self.last_rx_ack_seq,
            true,
        );
        self.outbox.push_back(OutPdu {
            name: "Data Burst (SMS)",
            bits: pdu,
            ack_seq: self.last_rx_ack_seq,
            msg_seq,
            ack_req: true,
        });
        self.tx_notices.push_back(TrafficTxNotice {
            name: "Data Burst (SMS)",
            msg_seq,
            ack_seq: self.last_rx_ack_seq,
            ack_req: true,
            retransmission: false,
        });
        self.sms_sent = true;
        self.sms_cause_deadline_frame =
            Some(self.frames_sent.saturating_add(SMS_CAUSE_WAIT_FRAMES));
    }

    fn maybe_send_power_report(&mut self) {
        let Some(parameters) = self.power_report else {
            return;
        };
        if !self.up()
            || self.measured_frames == 0
            || self.end_chip.is_some()
            || self.release_msg_seq.is_some()
        {
            return;
        }
        let since_last = self
            .last_power_report_frame
            .map_or(u64::MAX, |last| self.frames_sent.saturating_sub(last));
        if since_last < parameters.delay_frames {
            return;
        }
        let periodic =
            parameters.periodic && self.measured_frames >= parameters.measurement_frames as u32;
        let threshold = parameters
            .threshold
            .is_some_and(|threshold| self.measured_bad_frames >= u32::from(threshold));
        if !periodic && !threshold {
            if self.measured_frames >= parameters.measurement_frames as u32 {
                self.measured_frames = 0;
                self.measured_bad_frames = 0;
            }
            return;
        }
        let report = PowerMeasurementReportMessage {
            errors_detected: self.measured_bad_frames.min(31) as u8,
            pwr_meas_frames: self.measured_frames.min(1_023) as u16,
            last_hdm_seq: 3,
            pilot_strengths: vec![self.active_pilot_strength],
            dcch_pwr_meas_incl: false,
            dcch_pwr_meas_frames: None,
            dcch_errors_detected: None,
            sch_pwr_meas_incl: false,
            sch_id: None,
            sch_pwr_meas_frames: None,
            sch_errors_detected: None,
        };
        let body = AccessMessage::PowerMeasurementReport(report)
            .to_sdu()
            .expect("PMRM fields are bounded");
        self.queue_pdu("Power Measurement Report", body, false);
        self.measured_frames = 0;
        self.measured_bad_frames = 0;
        self.last_power_report_frame = Some(self.frames_sent);
    }

    pub fn on_forward(&mut self, pdu: &FdschPdu) -> Vec<TrafficOutcome> {
        let mut out = Vec::new();
        let order = match &pdu.body {
            FdschMessage::Order(o) => Some(order_name(o.order).to_string()),
            _ => None,
        };
        out.push(TrafficOutcome::Message {
            name: pdu.message_id.tag().to_string(),
            order: order.clone(),
            msg_seq: pdu.arq.msg_seq,
            ack_seq: pdu.arq.ack_seq,
            ack_req: pdu.arq.ack_req,
        });
        log::debug!(
            "cdma-ms: traffic rx {} msg_seq={} ack_seq={} ack_req={} pending={:?}",
            pdu.message_id.tag(),
            pdu.arq.msg_seq,
            pdu.arq.ack_seq,
            pdu.arq.ack_req,
            self.pending.as_ref().map(|p| (p.name, p.msg_seq)),
        );
        if let Some(pending) = &self.pending {
            if pdu.arq.ack_seq == pending.msg_seq {
                log::info!(
                    "cdma-ms: {} msg_seq={} acknowledged",
                    pending.name,
                    pending.msg_seq
                );
                let acked = pending.msg_seq;
                self.acked_msg_seq = Some(acked);
                self.pending = None;
                self.outbox.retain(|p| !(p.ack_req && p.msg_seq == acked));
                if Some(acked) == self.release_msg_seq {
                    log::info!("cdma-ms: Release Order acknowledged");
                    self.release_deadline_chip = None;
                    self.end_after_outbox();
                }
            }
        }
        if pdu.arq.ack_req {
            self.last_rx_ack_seq = pdu.arq.msg_seq;
        }
        let duplicate = pdu.arq.ack_req && self.rcvd.mark(pdu.arq.msg_seq);
        if duplicate {
            self.queue_ack_if_idle();
            return out;
        }
        let mut answered = false;
        match &pdu.body {
            FdschMessage::Order(o) => {
                let ordq = o.order_specific.first().copied().unwrap_or(0);
                match o.order {
                    ORDER_BS_ACK => {}
                    ORDER_SERVICE_OPTION_RESPONSE => {
                        self.connected = true;
                        out.push(TrafficOutcome::ServiceConnected {
                            service_option: ordq as u16,
                        });
                    }
                    ORDER_SERVICE_OPTION_REQUEST => {
                        self.queue_pdu(
                            "Service Option Response Order",
                            service_option_response_body(ordq),
                            true,
                        );
                        self.connected = true;
                        answered = true;
                        out.push(TrafficOutcome::ServiceConnected {
                            service_option: ordq as u16,
                        });
                    }
                    ORDER_RELEASE => {
                        if self.end_chip.is_none() {
                            self.queue_pdu("Release Order", release_order_body(), false);
                            if self.release_msg_seq.is_none() {
                                self.released_by = Some("base_station");
                            }
                            self.end_after_outbox();
                            answered = true;
                            out.push(TrafficOutcome::Released { by: "base_station" });
                        }
                    }
                    _ => {}
                }
            }
            FdschMessage::AlertWithInformation(alert) => {
                if self.incoming && !self.answered {
                    self.ringing = true;
                    out.push(TrafficOutcome::Ringing {
                        caller_number: alert
                            .calling_party
                            .as_ref()
                            .map(|record| record.digits.clone()),
                    });
                }
            }
            FdschMessage::ServiceConnect(sc) => {
                self.queue_pdu(
                    "Service Connect Completion",
                    service_connect_completion_body(sc.serv_con_seq),
                    true,
                );
                self.connected = true;
                answered = true;
                out.push(TrafficOutcome::ServiceConnected {
                    service_option: self.service_option,
                });
            }
            FdschMessage::DataBurst(db) => out.push(TrafficOutcome::DataBurst {
                burst_type: db.burst_type,
                fields: db.fields.clone(),
            }),
            FdschMessage::PowerControlParameters(parameters) => {
                self.power_report = PowerReportParameters::from_traffic_message(parameters);
                self.measured_frames = 0;
                self.measured_bad_frames = 0;
                self.last_power_report_frame = None;
            }
            FdschMessage::StatusRequest(request) => {
                out.push(TrafficOutcome::StatusRequested {
                    qualification_type: request.qualification_type,
                    qualification: request.qualification.clone(),
                    record_types: request.record_types.clone(),
                });
                answered = true;
            }
            FdschMessage::PowerControl(message) => {
                let db = match message.reverse_step {
                    POWER_CONTROL_STEP_ONE_DB => POWER_CONTROL_ONE_DB,
                    POWER_CONTROL_STEP_HALF_DB => POWER_CONTROL_HALF_DB,
                    POWER_CONTROL_STEP_QUARTER_DB => POWER_CONTROL_QUARTER_DB,
                    _ => DEFAULT_POWER_CONTROL_STEP_DB,
                };
                out.push(TrafficOutcome::ReversePowerStep { db });
            }
            FdschMessage::InTrafficSystemParameters(_) => {}
            FdschMessage::ServiceRequest(_) | FdschMessage::ServiceResponse(_) => {}
        }
        if pdu.arq.ack_req && !answered {
            self.queue_ack_if_idle();
        }
        out
    }

    pub fn queue_status_response(&mut self, body: Bitstream) {
        self.queue_pdu("Status Response", body, true);
    }

    pub fn on_unsupported_forward(&mut self, header: &UnsupportedForwardPdu) -> TrafficOutcome {
        if let Some(pending) = &self.pending {
            if header.ack_seq == pending.msg_seq {
                self.pending = None;
            }
        }
        if header.ack_req {
            self.last_rx_ack_seq = header.msg_seq;
            self.rcvd.mark(header.msg_seq);
            self.queue_ack_if_idle();
            log::debug!(
                "cdma-ms: unhandled traffic message type 0x{:02x} msg_seq={} ack requested",
                header.msg_type,
                header.msg_seq
            );
        }
        TrafficOutcome::Message {
            name: format!("Unknown 0x{:02X}", header.msg_type),
            order: None,
            msg_seq: header.msg_seq,
            ack_seq: header.ack_seq,
            ack_req: header.ack_req,
        }
    }

    fn queue_ack_if_idle(&mut self) {
        if self.outbox.is_empty() {
            self.queue_pdu("MS Ack Order", ms_ack_order_body(), false);
        }
    }
}

/// `MSG_TYPE(8) | ACK_SEQ(3) MSG_SEQ(3) ACK_REQ(1) ENCRYPTION(2) | body`
/// for a reverse dedicated PDU.
fn rdsch_pdu(name: &str, body: &Bitstream, ack_seq: u8, msg_seq: u8, ack_req: bool) -> Bitstream {
    let id = match name {
        "Release Order" | "MS Ack Order" | "Connect Order" | "Service Option Response Order" => {
            MessageId::Order
        }
        SEND_BURST_DTMF => MessageId::SendBurstDtmf,
        "Service Connect Completion" => MessageId::ServiceConnectCompletion,
        "Power Measurement Report" => MessageId::PowerMeasurementReport,
        "Status Response" => MessageId::StatusResponse,
        _ => MessageId::Order,
    };
    let msg_type = id
        .wire_type(WireChannel::ReverseDedicated)
        .expect("reverse dedicated wire type");
    let mut pdu = Bitstream::new();
    pdu.write_u8(msg_type, 8);
    pdu.write_u8(ack_seq & 0x07, 3);
    pdu.write_u8(msg_seq & 0x07, 3);
    pdu.write_u8(ack_req as u8, 1);
    pdu.write_u8(0, 2);
    pdu.extend(body);
    pdu
}

/// The reassembler keys the CRC position off MSG_LENGTH in whole octets, so
/// a PDU is padded to a byte boundary before SAR fragmentation.
fn byte_aligned(pdu: &Bitstream) -> Bitstream {
    let mut out = pdu.clone();
    while out.len() % 8 != 0 {
        out.write_u8(0, 1);
    }
    out
}

/// ORDER(6) ADD_RECORD_LEN(3) with no fields.
fn ms_ack_order_body() -> Bitstream {
    let mut b = Bitstream::new();
    b.write_u8(ORDER_MS_ACK, 6);
    b.write_u8(NO_ORDER_SPECIFIC_FIELDS, 3);
    b
}

/// Normal Release has an implicit zero ORDQ and no order-specific fields.
fn release_order_body() -> Bitstream {
    let mut b = Bitstream::new();
    b.write_u8(ORDER_MS_RELEASE, 6);
    b.write_u8(NO_ORDER_SPECIFIC_FIELDS, 3);
    b
}

fn connect_order_body() -> Bitstream {
    let mut b = Bitstream::new();
    b.write_u8(ORDER_CONNECT, 6);
    b.write_u8(0, 3);
    b
}

/// Service Option Response Order whose ORDQ names the accepted option.
fn service_option_response_body(ordq: u8) -> Bitstream {
    let mut b = Bitstream::new();
    b.write_u8(ORDER_SERVICE_OPTION_RESPONSE, 6);
    b.write_u8(1, 3);
    b.write_u8(ordq, 8);
    b
}

/// Service Connect Completion Message: RESERVED(1), SERV_CON_SEQ(3)
/// (C.S0005-E §2.7.2.3.2.14).
fn service_connect_completion_body(serv_con_seq: u8) -> Bitstream {
    let mut b = Bitstream::new();
    b.write_u8(RESERVED_BIT, SINGLE_BIT);
    b.write_u8(
        serv_con_seq & SERVICE_CONNECT_SEQUENCE_MASK,
        SERVICE_CONNECT_SEQUENCE_BITS,
    );
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use cdma_common::access::{FdschOrderMessage, RdschArqHeader, RdschPdu};
    use cdma_common::lac::paging_messages::{
        AlertWithInformationMessage, CallingPartyNumberRecord, SignalInfoRecord,
    };

    fn assignment() -> ChannelAssignment {
        ChannelAssignment {
            walsh_code: 10,
            frame_offset: 0,
            for_rc: 3,
            rev_rc: 3,
            pilot_pn: 0,
        }
    }

    fn order_pdu(order: u8, ordq: Option<u8>, msg_seq: u8, ack_req: bool, ack_seq: u8) -> FdschPdu {
        FdschPdu {
            message_id: MessageId::Order,
            raw_msg_type: 0,
            arq: RdschArqHeader {
                ack_seq,
                msg_seq,
                ack_req,
                encryption: 0,
            },
            body: FdschMessage::Order(FdschOrderMessage {
                use_time: false,
                action_time: 0,
                order,
                add_record_len: ordq.map(|_| 1).unwrap_or(0),
                order_specific: ordq.map(|q| vec![q]).unwrap_or_default(),
                con_ref_incl: None,
                con_ref: None,
            }),
        }
    }

    fn lc(_chip: u64) -> u64 {
        1u64 << 41
    }

    fn connected_voice_session() -> TrafficSession {
        let mut session = TrafficSession::new(
            7,
            assignment(),
            cdma_common::consts::SERVICE_OPTION_EVRC_A,
            None,
            false,
            None,
            0,
            0,
        );
        session.connected = true;
        session.frames_sent = PREAMBLE_FRAMES;
        session.set_forward_confirmed(true);
        session
    }

    #[test]
    fn dtmf_burst_encodes_digits_and_timing_on_reverse_dedicated_channel() {
        for digits in ["1234567890*#".to_string(), "9".repeat(u8::MAX as usize)] {
            let mut session = connected_voice_session();
            session
                .send_dtmf_burst(DtmfBurst::new(&digits).unwrap())
                .unwrap();
            let mut reader = cdma_bts::receiver::access::DedicatedFrameReader::new();
            let mut decoded = None;
            const MUX1_HEADER_BITS: usize = 4;
            loop {
                let Rc3Frame::Traffic { rate, info_bits } = session.next_frame() else {
                    panic!("expected traffic frame")
                };
                assert_eq!(rate, Rc3Rate::Full);
                let mut signaling = Bitstream::new_init(&info_bits[MUX1_HEADER_BITS..]);
                if let Some(frame) = reader.process(&mut signaling).unwrap() {
                    assert!(frame.crc_valid);
                    assert!(decoded.is_none());
                    decoded = Some(RdschPdu::decode(&frame.data).unwrap());
                }
                if session.frame_queue.is_empty() {
                    break;
                }
            }
            let pdu = decoded.expect("BTS reassembles the DTMF burst");
            assert!(pdu.arq.ack_req);
            let AccessMessage::SendBurstDtmf(message) = pdu.l3 else {
                panic!("expected Send Burst DTMF")
            };
            assert_eq!(
                cdma_common::formatting::format_dtmf_digits(&message.digits, false),
                digits
            );
            assert_eq!(message.dtmf_on_length, BdtmfmOnLength::Ms95 as u8);
            assert_eq!(message.dtmf_off_length, BdtmfmOffLength::Ms60 as u8);
            assert_eq!(message.con_ref, None);
        }
        for digits in ["", "12A", "1 2", "１２", &"1".repeat(u8::MAX as usize + 1)] {
            assert!(DtmfBurst::new(digits).is_err(), "accepted {digits:?}");
        }
    }

    #[test]
    fn dtmf_burst_requires_connected_voice_and_stops_at_release() {
        let burst = || DtmfBurst::new("1").unwrap();
        let mut session = connected_voice_session();
        session.connected = false;
        assert!(session.send_dtmf_burst(burst()).is_err());
        session.connected = true;
        session.service_option = SERVICE_OPTION_SMS;
        assert!(session.send_dtmf_burst(burst()).is_err());
        session.service_option = cdma_common::consts::SERVICE_OPTION_EVRC_A;
        session.incoming = true;
        assert!(session.send_dtmf_burst(burst()).is_err());
        session.answered = true;
        session.send_dtmf_burst(burst()).unwrap();
        session.release();
        assert!(session.send_dtmf_burst(burst()).is_err());
        assert!(session.outbox.iter().all(|pdu| pdu.name != SEND_BURST_DTMF));
        let mut session = connected_voice_session();
        session.end_chip = Some(FRAME_CHIPS);
        assert!(session.send_dtmf_burst(burst()).is_err());
        let mut session = connected_voice_session();
        session.ended = true;
        assert!(session.send_dtmf_burst(burst()).is_err());
    }

    #[test]
    fn dtmf_burst_retries_until_ack_then_allows_another_burst() {
        let mut session = connected_voice_session();
        let burst = || DtmfBurst::new("123#").unwrap();
        session.send_dtmf_burst(burst()).unwrap();
        assert!(session.send_dtmf_burst(burst()).is_err());
        let _frame = session.next_frame();
        let first = session.pending.clone().unwrap();
        assert_eq!(first.name, SEND_BURST_DTMF);
        assert!(session.send_dtmf_burst(burst()).is_err());
        session.next_frame_chip += T1M_FRAMES * FRAME_CHIPS;
        session.retransmit_if_due();
        assert_eq!(session.outbox[0].bits.bits(), first.pdu_bits);
        let _retry = session.next_frame();
        assert_eq!(session.pending.as_ref().unwrap().transmissions, 2);
        session.on_forward(&order_pdu(ORDER_BS_ACK, None, 0, false, first.msg_seq));
        assert!(session.pending.is_none());
        assert!(session.connected());
        assert!(session.release_msg_seq.is_none());
        session.send_dtmf_burst(burst()).unwrap();
        assert_ne!(session.outbox[0].msg_seq, first.msg_seq);
    }

    #[test]
    fn traffic_frame_grid_follows_assignment_offset() {
        assert_eq!(next_traffic_frame_chip(24_576, 0, 0), 24_576);
        assert_eq!(next_traffic_frame_chip(24_576, 0, 7), 35_328);
        assert_eq!(next_traffic_frame_chip(25_000, 10_000, 7), 45_328);

        let mut assigned = assignment();
        assigned.frame_offset = 7;
        let session = TrafficSession::new(7, assigned, 6, None, false, None, 24_576, 0);
        assert_eq!(session.start_chip(), 35_328);
    }

    #[test]
    fn preamble_then_sms_then_release_ends_the_transmission() {
        let sms = OutgoingSms {
            destination: "5551234".into(),
            text: "hi".into(),
        };
        let mut s = TrafficSession::new(
            7,
            assignment(),
            6,
            Some(sms),
            false,
            None,
            1_000,
            FRAME_CHIPS,
        );
        s.set_forward_confirmed(true);
        assert_eq!(s.start_chip() % FRAME_CHIPS, 0);
        let mut now = 0;
        let mut chunks = Vec::new();
        while s.frames_sent < PREAMBLE_FRAMES + SERVICE_WAIT_FRAMES + 2 {
            chunks.extend(s.poll(now, FRAME_CHIPS, &lc));
            now += CHUNK_FRAMES as u64 * FRAME_CHIPS;
        }
        assert!(s.up());
        assert!(s.sms_sent, "the SMS goes out without a service connection");
        assert!(s.pending.is_some(), "the data burst awaits a BS Ack");
        let acked = s.pending.as_ref().unwrap().msg_seq;
        let out = s.on_forward(&order_pdu(ORDER_BS_ACK, None, 0, false, acked));
        assert!(matches!(out[0], TrafficOutcome::Message { .. }));
        assert!(s.pending.is_none());
        assert!(s.release_msg_seq.is_none());
        s.on_forward(&order_pdu(ORDER_BS_ACK, None, 1, true, 7));
        s.complete_sms();
        assert_eq!(s.outbox.len(), 1);
        assert_eq!(s.outbox[0].name, "Release Order");
        let release_seq = s.outbox[0].msg_seq;
        assert_eq!(
            s.release_msg_seq,
            Some(release_seq),
            "a repeated network acknowledgment must not ask for a second release"
        );
        let mut ended = false;
        for _ in 0..10 {
            for c in s.poll(now, FRAME_CHIPS, &lc) {
                if c.end_of_burst {
                    ended = true;
                }
            }
            now += CHUNK_FRAMES as u64 * FRAME_CHIPS;
        }
        assert!(!ended, "the release was not acknowledged yet");
        s.on_forward(&order_pdu(ORDER_BS_ACK, None, 2, false, release_seq));
        for _ in 0..10 {
            for c in s.poll(now, FRAME_CHIPS, &lc) {
                if c.end_of_burst {
                    ended = true;
                }
            }
            now += CHUNK_FRAMES as u64 * FRAME_CHIPS;
        }
        assert!(ended && s.ended());
        assert_eq!(s.released_by(), Some("mobile"));
    }

    #[test]
    fn an_sms_session_with_nothing_to_send_releases_itself() {
        let mut s = TrafficSession::new(7, assignment(), 6, None, false, None, 1_000, FRAME_CHIPS);
        s.set_forward_confirmed(true);
        let mut now = 0;
        while s.frames_sent < SMS_SESSION_MAX_FRAMES {
            s.poll(now, FRAME_CHIPS, &lc);
            now += CHUNK_FRAMES as u64 * FRAME_CHIPS;
            assert!(
                s.release_msg_seq.is_none(),
                "released before the session ran its length"
            );
        }
        s.poll(now, FRAME_CHIPS, &lc);
        assert!(s.release_msg_seq.is_some());
        assert_eq!(s.released_by(), Some("mobile"));
    }

    #[test]
    fn sms_cause_wait_starts_when_the_burst_is_sent() {
        let sms = OutgoingSms {
            destination: "5551234".into(),
            text: "hi".into(),
        };
        let mut s = TrafficSession::new(
            7,
            assignment(),
            6,
            Some(sms),
            false,
            None,
            1_000,
            FRAME_CHIPS,
        );
        s.set_forward_confirmed(true);
        let mut now = 0;
        while s.sms_cause_deadline_frame.is_none() {
            s.poll(now, FRAME_CHIPS, &lc);
            now += CHUNK_FRAMES as u64 * FRAME_CHIPS;
        }
        let deadline = s.sms_cause_deadline_frame.unwrap();
        assert!(deadline >= s.frames_sent + SMS_CAUSE_WAIT_FRAMES - CHUNK_FRAMES as u64);
        s.frames_sent = deadline - 1;
        s.release_if_sms_session_overran();
        assert!(s.release_msg_seq.is_none());
        s.frames_sent = deadline;
        s.release_if_sms_session_overran();
        assert!(s.release_msg_seq.is_some());
    }

    #[test]
    fn an_unacknowledged_release_still_stops_the_transmission() {
        let lc = |_chip: u64| 0x1234_5678_9abcu64;
        let mut s = TrafficSession::new(7, assignment(), 6, None, false, None, 0, FRAME_CHIPS);
        s.set_forward_confirmed(true);
        s.release();
        let mut now = 0;
        let mut ended = false;
        for _ in 0..(N1M_MAX_TRANSMISSIONS as usize + 2) * 30 {
            for c in s.poll(now, FRAME_CHIPS, &lc) {
                if c.end_of_burst {
                    ended = true;
                }
            }
            now += CHUNK_FRAMES as u64 * FRAME_CHIPS;
        }
        assert!(ended && s.ended());
        assert_eq!(s.released_by(), Some("release timeout"));
    }

    #[test]
    fn service_connect_is_completed_and_acks_ride_on_it() {
        let mut s = TrafficSession::new(7, assignment(), 6, None, false, None, 0, FRAME_CHIPS);
        let sc = FdschPdu {
            message_id: MessageId::ServiceConnect,
            raw_msg_type: 0,
            arq: RdschArqHeader {
                ack_seq: 7,
                msg_seq: 2,
                ack_req: true,
                encryption: 0,
            },
            body: FdschMessage::ServiceConnect(cdma_common::access::ServiceConnectMessage {
                use_time: false,
                action_time: 0,
                serv_con_seq: 5,
                use_old_serv_config: 0,
                sync_id: None,
                records: vec![],
                call_assignments: vec![],
                use_type0_plcm: false,
            }),
        };
        let out = s.on_forward(&sc);
        assert!(
            out.iter()
                .any(|o| matches!(o, TrafficOutcome::ServiceConnected { service_option: 6 }))
        );
        assert_eq!(
            s.outbox.len(),
            1,
            "only the completion goes out, it carries the ack"
        );
        assert_eq!(s.outbox[0].name, "Service Connect Completion");
        assert!(s.outbox[0].ack_req);
        assert_eq!(s.last_rx_ack_seq, 2);
        let completion = RdschPdu::decode(&byte_aligned(&s.outbox[0].bits))
            .expect("Service Connect Completion should decode");
        assert_eq!(completion.l3.serv_con_seq(), Some(5));
    }

    #[test]
    fn incoming_alert_rings_and_answer_queues_connect_order() {
        let mut session = TrafficSession::new(7, assignment(), 3, None, true, None, 0, FRAME_CHIPS);
        let alert = FdschPdu {
            message_id: MessageId::AlertWithInformation,
            raw_msg_type: 0,
            arq: RdschArqHeader {
                ack_seq: 7,
                msg_seq: 2,
                ack_req: true,
                encryption: 0,
            },
            body: FdschMessage::AlertWithInformation(AlertWithInformationMessage {
                signal_info: Some(SignalInfoRecord {
                    signal_type: 2,
                    alert_pitch: 0,
                    signal: 1,
                }),
                calling_party: Some(CallingPartyNumberRecord {
                    number_type: 3,
                    number_plan: 1,
                    presentation_indicator: 0,
                    screening_indicator: 3,
                    digits: "5551212".to_string(),
                }),
            }),
        };

        let outcomes = session.on_forward(&alert);
        assert!(outcomes.iter().any(|outcome| matches!(
            outcome,
            TrafficOutcome::Ringing {
                caller_number: Some(number)
            } if number == "5551212"
        )));
        assert!(session.answer());
        assert!(!session.answer(), "answer is idempotent");
        let connect = session
            .outbox
            .iter()
            .find(|pdu| pdu.name == "Connect Order")
            .expect("answer must queue the Connect Order");
        assert_eq!(&connect.bits.bits()[17..23], &[0, 1, 1, 0, 0, 0]);
    }

    #[test]
    fn mobile_originated_alert_does_not_ring() {
        let mut session =
            TrafficSession::new(7, assignment(), 3, None, false, None, 0, FRAME_CHIPS);
        let alert = FdschPdu {
            message_id: MessageId::AlertWithInformation,
            raw_msg_type: 0,
            arq: RdschArqHeader {
                ack_seq: 7,
                msg_seq: 2,
                ack_req: false,
                encryption: 0,
            },
            body: FdschMessage::AlertWithInformation(AlertWithInformationMessage::ringback()),
        };

        assert!(
            !session
                .on_forward(&alert)
                .iter()
                .any(|outcome| matches!(outcome, TrafficOutcome::Ringing { .. }))
        );
        assert!(!session.answer());
    }

    #[test]
    fn unacknowledged_pdu_is_retransmitted_then_released() {
        let mut s = TrafficSession::new(7, assignment(), 6, None, false, None, 0, FRAME_CHIPS);
        s.set_forward_confirmed(true);
        s.queue_pdu(
            "Service Connect Completion",
            service_connect_completion_body(1),
            true,
        );
        let mut now = 0;
        let mut transmissions = 0;
        for _ in 0..2_000 {
            if s.ended() {
                break;
            }
            s.poll(now, FRAME_CHIPS, &lc);
            if let Some(p) = &s.pending {
                transmissions = transmissions.max(p.transmissions);
            }
            now += CHUNK_FRAMES as u64 * FRAME_CHIPS;
        }
        assert_eq!(transmissions, N1M_MAX_TRANSMISSIONS);
        assert!(s.ended());
        assert_eq!(s.released_by(), Some("release timeout"));
    }

    #[test]
    fn traffic_initialization_stops_at_t50m_without_forward_acquisition() {
        let mut s = TrafficSession::new(7, assignment(), 6, None, false, None, 100, 0);

        let chunks = s.poll(100 + TRAFFIC_INITIALIZATION_CHIPS, 0, &lc);
        assert!(s.ended());
        assert_eq!(s.released_by(), Some("traffic initialization timeout"));
        assert_eq!(chunks.len(), 1);
        assert!(chunks[0].end_of_burst);
        assert!(chunks[0].samples.is_empty());
    }

    #[test]
    fn requested_periodic_power_measurement_report_is_queued() {
        let parameters = PowerReportParameters {
            threshold: None,
            measurement_frames: 2,
            periodic: true,
            delay_frames: 1,
        };
        let mut session =
            TrafficSession::new(7, assignment(), 3, None, false, Some(parameters), 0, 0);
        session.frames_sent = PREAMBLE_FRAMES;
        session.set_forward_confirmed(true);
        session.record_forward_measurements(20, 3, 7);

        session.maybe_send_power_report();

        let queued = session.outbox.back().expect("PMRM queued");
        assert_eq!(queued.name, "Power Measurement Report");
        let decoded = cdma_common::access::RdschPdu::decode(&queued.bits).expect("decode PMRM");
        let AccessMessage::PowerMeasurementReport(report) = decoded.l3 else {
            panic!("expected PMRM")
        };
        assert_eq!(report.pwr_meas_frames, 20);
        assert_eq!(report.errors_detected, 3);
        assert_eq!(report.pilot_strengths, vec![7]);
        assert_eq!(session.drain_tx_notices().count(), 1);
    }

    #[test]
    fn release_discards_other_signaling_and_suppresses_power_reports() {
        let parameters = PowerReportParameters {
            threshold: None,
            measurement_frames: 2,
            periodic: true,
            delay_frames: 1,
        };
        let mut session =
            TrafficSession::new(7, assignment(), 3, None, false, Some(parameters), 0, 0);
        session.frames_sent = PREAMBLE_FRAMES;
        session.set_forward_confirmed(true);
        session.record_forward_measurements(20, 3, 7);
        session.maybe_send_power_report();

        session.release();
        session.record_forward_measurements(20, 3, 7);
        session.maybe_send_power_report();

        assert_eq!(session.outbox.len(), 1);
        assert_eq!(session.outbox[0].name, "Release Order");
        let decoded = RdschPdu::decode(&session.outbox[0].bits).expect("decode Release Order");
        let AccessMessage::Order(order) = decoded.l3 else {
            panic!("expected Release Order")
        };
        assert_eq!(order.order, ORDER_MS_RELEASE);
        assert_eq!(order.add_record_len, NO_ORDER_SPECIFIC_FIELDS);
        assert!(order.order_specific.is_empty());
    }

    #[test]
    fn traffic_status_request_queues_assured_status_response() {
        let mut session =
            TrafficSession::new(7, assignment(), 3, None, false, None, 0, FRAME_CHIPS);
        let request = FdschPdu {
            message_id: MessageId::StatusRequest,
            raw_msg_type: MessageId::StatusRequest
                .wire_type(WireChannel::ForwardDedicated)
                .unwrap(),
            arq: cdma_common::access::RdschArqHeader {
                ack_seq: ACK_SEQ_NONE,
                msg_seq: 2,
                ack_req: true,
                encryption: 0,
            },
            body: FdschMessage::StatusRequest(cdma_common::access::FdschStatusRequestMessage {
                qualification_type: 0,
                qualification: Vec::new(),
                record_types: vec![0x0e, 0x1a],
            }),
        };

        let outcomes = session.on_forward(&request);
        let Some(TrafficOutcome::StatusRequested { record_types, .. }) = outcomes.last() else {
            panic!("expected status request outcome")
        };
        assert_eq!(record_types, &[0x0e, 0x1a]);

        session.queue_status_response(Bitstream::new());
        let response = session.outbox.back().expect("status response queued");
        assert_eq!(response.name, "Status Response");
        assert!(response.ack_req);
    }

    #[test]
    fn assured_status_responses_wait_for_the_previous_ack() {
        let mut session =
            TrafficSession::new(7, assignment(), 3, None, false, None, 0, FRAME_CHIPS);
        session.frames_sent = PREAMBLE_FRAMES;
        session.set_forward_confirmed(true);
        session.queue_status_response(Bitstream::new());
        session.queue_status_response(Bitstream::new());

        let _first = session.next_frame();
        assert_eq!(session.pending.as_ref().map(|pdu| pdu.msg_seq), Some(0));
        assert_eq!(session.outbox.len(), 1);

        let _idle = session.next_frame();
        assert_eq!(session.pending.as_ref().map(|pdu| pdu.msg_seq), Some(0));
        assert_eq!(session.outbox.len(), 1);

        session.on_forward(&order_pdu(ORDER_BS_ACK, None, 0, false, 0));
        let _second = session.next_frame();
        assert_eq!(session.pending.as_ref().map(|pdu| pdu.msg_seq), Some(1));
        assert!(session.outbox.is_empty());
    }

    #[test]
    fn traffic_power_parameters_replace_report_policy() {
        let mut session =
            TrafficSession::new(7, assignment(), 3, None, false, None, 0, FRAME_CHIPS);
        let pdu = FdschPdu {
            message_id: MessageId::PowerControlParameters,
            raw_msg_type: MessageId::PowerControlParameters
                .wire_type(WireChannel::ForwardDedicated)
                .unwrap(),
            arq: cdma_common::access::RdschArqHeader {
                ack_seq: ACK_SEQ_NONE,
                msg_seq: 3,
                ack_req: false,
                encryption: 0,
            },
            body: FdschMessage::PowerControlParameters(
                cdma_common::access::PowerControlParametersMessage {
                    report_threshold: 4,
                    report_frames: 6,
                    threshold_enabled: true,
                    periodic_enabled: true,
                    report_delay: 3,
                },
            ),
        };

        session.on_forward(&pdu);

        let policy = session.power_report.expect("power report policy");
        assert_eq!(policy.threshold, Some(4));
        assert_eq!(policy.measurement_frames, power_report_frame_count(6));
        assert!(policy.periodic);
        assert_eq!(policy.delay_frames, 3 * POWER_REPORT_DELAY_UNIT_FRAMES);
    }

    #[test]
    fn traffic_power_control_reports_quarter_db_reverse_step() {
        let mut session =
            TrafficSession::new(7, assignment(), 3, None, false, None, 0, FRAME_CHIPS);
        let pdu = FdschPdu {
            message_id: MessageId::PowerControl,
            raw_msg_type: MessageId::PowerControl
                .wire_type(WireChannel::ForwardDedicated)
                .unwrap(),
            arq: cdma_common::access::RdschArqHeader {
                ack_seq: ACK_SEQ_NONE,
                msg_seq: 4,
                ack_req: false,
                encryption: 0,
            },
            body: FdschMessage::PowerControl(cdma_common::access::PowerControlMessage {
                reverse_step: POWER_CONTROL_STEP_QUARTER_DB,
                use_time: false,
                action_time: None,
                forward_parameters_included: false,
                forward_mode: None,
                reverse_parameters_included: false,
            }),
        };

        let outcomes = session.on_forward(&pdu);

        assert!(outcomes.contains(&TrafficOutcome::ReversePowerStep {
            db: POWER_CONTROL_QUARTER_DB,
        }));
    }

    #[test]
    fn evrc_microphone_frame_uses_the_matching_rc3_rate() {
        let mut session = TrafficSession::new(7, assignment(), 3, None, false, None, 0, 0);
        session.frames_sent = PREAMBLE_FRAMES;
        session.set_forward_confirmed(true);
        let mut pcm = [0i16; SAMPLES_PER_FRAME];
        for (index, sample) in pcm.iter_mut().enumerate() {
            let phase = 2.0 * std::f32::consts::PI * 440.0 * index as f32 / 8_000.0;
            *sample = (phase.sin() * 12_000.0) as i16;
        }
        session.push_microphone(pcm);

        let Rc3Frame::Traffic { rate, info_bits } = session.next_frame() else {
            panic!("voice frame must enable the R-FCH")
        };
        assert_eq!(info_bits.len(), rate.info_bits());
        let primary_bits = if rate == Rc3Rate::Full {
            assert_eq!(info_bits[0], 0, "full-rate voice needs the MuxPDU header");
            &info_bits[1..]
        } else {
            &info_bits
        };
        let decoded = session
            .decode_voice(rate.rate_bps() as u32, primary_bits)
            .expect("encoded microphone frame should decode");
        assert!(
            decoded.iter().any(|sample| sample.unsigned_abs() > 100),
            "decoded tone should contain audible energy"
        );
    }
}
