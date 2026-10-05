use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};

use cdma_bts::channels::ftch_rc3::ForwardTrafficChannelRc3;
use cdma_bts::phy::coding::long_code::LongCodeGenerator;
use cdma_bts::phy::walsh::WalshGenerator;
use cdma_bts::receiver::access::DedicatedFrameReader;
use cdma_bts::receiver::access_layer3::{FdschMessage, FdschPdu};
use cdma_bts::receiver::ffch_rc3::decode_forward_rc3_signaling_frame_from_lc_state;
use cdma_bts::receiver::pipelined::{PipelineEmitter, PipelineProcessor, SampleBlock};
use cdma_common::bits::Bitstream;
use num_complex::Complex32;
use parking_lot::Mutex;

use crate::traffic::next_traffic_frame_chip;

pub const FORWARD_WALSH_CODES: usize = 64;
const FRAME_CHIPS: usize = 24_576;
const CHIPS_PER_SYMBOL: usize = 64;
const SYMBOLS_PER_FRAME: usize = FRAME_CHIPS / CHIPS_PER_SYMBOL;
const PCGS_PER_FRAME: usize = 16;
const PCG_CHIPS: usize = FRAME_CHIPS / PCGS_PER_FRAME;
const QPSK_SYMBOLS_PER_PCG: usize = SYMBOLS_PER_FRAME / PCGS_PER_FRAME;
const FPC_MOD_SYMBOLS: usize = 4;
const FPC_MIN_COHERENCE: f32 = 0.6;
const SIGNALING_MUX_PREFIX: [u8; 4] = [1, 0, 1, 1];

#[derive(Debug, Clone)]
pub struct UnsupportedForwardPdu {
    pub msg_type: u8,
    pub ack_seq: u8,
    pub msg_seq: u8,
    pub ack_req: bool,
}

enum DecodedForwardSignaling {
    Pdu(FdschPdu),
    Unsupported(UnsupportedForwardPdu),
}

fn decode_signaling_fragment(
    reader: &mut DedicatedFrameReader,
    info_bits: &[u8],
) -> Option<DecodedForwardSignaling> {
    if !info_bits.starts_with(&SIGNALING_MUX_PREFIX) {
        reader.reset();
        return None;
    }
    let mut fragment = Bitstream::new_init(&info_bits[SIGNALING_MUX_PREFIX.len()..]);
    match reader.process(&mut fragment) {
        Ok(Some(frame)) if frame.crc_valid => match FdschPdu::decode(&frame.data) {
            Ok(pdu) => Some(DecodedForwardSignaling::Pdu(pdu)),
            Err(error) => {
                log::debug!(
                    "ms_ftraffic: f-dsch decode: {error} payload={:02x?}",
                    frame.data.to_packed_bytes()
                );
                if !error.starts_with("unsupported f-dsch MSG_TYPE") {
                    return None;
                }
                let mut bits = frame.data;
                let msg_type = bits.read_bits(8).ok()? as u8;
                let ack_seq = bits.read_bits(3).ok()? as u8;
                let msg_seq = bits.read_bits(3).ok()? as u8;
                let ack_req = bits.read_bits(1).ok()? != 0;
                Some(DecodedForwardSignaling::Unsupported(
                    UnsupportedForwardPdu {
                        msg_type,
                        ack_seq,
                        msg_seq,
                        ack_req,
                    },
                ))
            }
        },
        Ok(_) => None,
        Err(error) => {
            log::debug!("ms_ftraffic: SAR decode: {error}");
            reader.reset();
            None
        }
    }
}

fn rc3_power_control_bits(
    symbols: &[Complex32],
    esn: u32,
    anchor_chip: usize,
    anchor_state: u64,
    frame_chip: usize,
) -> Option<Vec<u8>> {
    if symbols.len() != SYMBOLS_PER_FRAME {
        return None;
    }
    let previous_pcg = frame_chip.checked_sub(PCG_CHIPS)?;
    let delta = previous_pcg.checked_sub(anchor_chip)?;
    let mut lc = LongCodeGenerator::new_traffic_channel(esn);
    lc.set_state(anchor_state);
    lc.advance_chips(delta);
    let mut start = ForwardTrafficChannelRc3::pc_start_from_pcg_lc(&mut lc);
    let mut bits = Vec::with_capacity(PCGS_PER_FRAME);
    for pcg in 0..PCGS_PER_FRAME {
        let group = &symbols[pcg * QPSK_SYMBOLS_PER_PCG..(pcg + 1) * QPSK_SYMBOLS_PER_PCG];
        let mut sum = 0.0f32;
        let mut magnitude = 0.0f32;
        for index in start..start + FPC_MOD_SYMBOLS {
            let symbol = group[index / 2];
            let value = if index % 2 == 0 { symbol.re } else { symbol.im };
            sum += value;
            magnitude += value.abs();
        }
        if magnitude == 0.0 || sum.abs() / magnitude < FPC_MIN_COHERENCE {
            return None;
        }
        bits.push((sum < 0.0) as u8);
        start = ForwardTrafficChannelRc3::pc_start_from_pcg_lc(&mut lc);
    }
    Some(bits)
}
pub const TRAFFIC_SIGNALING_EVENT: &str = "ms_traffic_signaling";
pub const TRAFFIC_POWER_CONTROL_EVENT: &str = "ms_traffic_power_control";
pub const TRAFFIC_VOICE_EVENT: &str = "ms_traffic_voice";
pub const TRAFFIC_SIGNALING_BURST_TYPE: &str = "ms_traffic_burst_type";

#[derive(Default)]
pub struct DecodedSignaling {
    pdus: std::sync::Mutex<Vec<FdschPdu>>,
    unsupported: std::sync::Mutex<Vec<UnsupportedForwardPdu>>,
}

impl DecodedSignaling {
    pub fn push(&self, pdu: FdschPdu) {
        self.pdus.lock().unwrap().push(pdu);
    }

    pub fn drain(&self) -> Vec<FdschPdu> {
        std::mem::take(&mut *self.pdus.lock().unwrap())
    }

    pub fn push_unsupported(&self, header: UnsupportedForwardPdu) {
        self.unsupported.lock().unwrap().push(header);
    }

    pub fn drain_unsupported(&self) -> Vec<UnsupportedForwardPdu> {
        std::mem::take(&mut *self.unsupported.lock().unwrap())
    }
}

#[derive(Default)]
pub struct ForwardTrafficConfig {
    pub active: AtomicBool,
    pub walsh: AtomicU32,
    pub frame_offset: AtomicU32,
    pub esn: AtomicU32,
    /// Sync anchor established: `paging_start_chip` + `lc_state` are valid.
    pub anchor_set: AtomicBool,
    /// Stream chip at which the sync long-code register is valid.
    pub paging_start_chip: AtomicI64,
    /// Sync long-code register (LFSR state) valid at `paging_start_chip`.
    pub lc_state: AtomicI64,
    /// Input-sample index of the PN epoch, from the aligner. Ties the
    /// post-alignment chip timeline to the receive-stream samples.
    pub pn_epoch_sample: AtomicI64,
    pub pn_epoch_set: AtomicBool,
    /// Sample slews the pilot tracker has applied since it locked. The PN
    /// epoch is latched once, so this carries how far the network's chip
    /// clock has since run from the receive-stream sample counter.
    pub pilot_slew_samples: AtomicI64,
    /// Consecutive good frames decoded on the assigned forward traffic
    /// channel. The mobile sends its traffic preamble until this reaches
    /// N5m, which is what ends the Traffic Channel Initialization Substate
    /// (C.S0005-E §2.6.4.1).
    pub good_forward_frames: AtomicU32,
    pub forward_confirmed: AtomicBool,
    pub frames_since_good_trigger: AtomicU32,
    pub forward_lost: AtomicBool,
    measured_frames: AtomicU32,
    measured_bad_frames: AtomicU32,
    pub signaling: DecodedSignaling,
    pub power_control: Mutex<Vec<(u8, u8)>>,
    pub voice: Mutex<Vec<(u32, Vec<u8>)>>,
}

impl ForwardTrafficConfig {
    pub fn set_pn_epoch(&self, sample: i64) {
        self.pn_epoch_sample.store(sample, Ordering::Relaxed);
        self.pn_epoch_set.store(true, Ordering::Relaxed);
    }

    pub fn pn_epoch(&self) -> Option<i64> {
        self.pn_epoch_set
            .load(Ordering::Relaxed)
            .then(|| self.pn_epoch_sample.load(Ordering::Relaxed))
    }

    pub fn set_pilot_slew(&self, samples: i64) {
        self.pilot_slew_samples.store(samples, Ordering::Relaxed);
    }

    pub fn pilot_slew(&self) -> i64 {
        self.pilot_slew_samples.load(Ordering::Relaxed)
    }
}

impl ForwardTrafficConfig {
    pub fn set_anchor(&self, paging_start_chip: i64, lc_state: u64) {
        // Re-latching the sync anchor would move the frame grid ahead of the decode position.
        if self.anchor_set.load(Ordering::Relaxed) {
            return;
        }
        log::debug!(
            "ms_ftraffic: anchor set paging_start_chip={} lc_state=0x{:x}",
            paging_start_chip,
            lc_state
        );
        self.paging_start_chip
            .store(paging_start_chip, Ordering::Relaxed);
        self.lc_state.store(lc_state as i64, Ordering::Relaxed);
        self.anchor_set.store(true, Ordering::Relaxed);
    }

    pub const FORWARD_CONFIRM_FRAMES: u32 = 2;
    const FADE_TIMER_FRAMES: u32 = 250;

    pub fn note_forward_frame(&self, good: bool) {
        self.measured_frames.fetch_add(1, Ordering::Relaxed);
        if !good {
            self.measured_bad_frames.fetch_add(1, Ordering::Relaxed);
            self.good_forward_frames.store(0, Ordering::Relaxed);
            let elapsed = self
                .frames_since_good_trigger
                .fetch_add(1, Ordering::Relaxed)
                + 1;
            if elapsed >= Self::FADE_TIMER_FRAMES {
                self.forward_lost.store(true, Ordering::Relaxed);
            }
            return;
        }
        let seen = self.good_forward_frames.fetch_add(1, Ordering::Relaxed) + 1;
        if seen >= Self::FORWARD_CONFIRM_FRAMES {
            self.frames_since_good_trigger.store(0, Ordering::Relaxed);
        }
        if seen >= Self::FORWARD_CONFIRM_FRAMES
            && !self.forward_confirmed.swap(true, Ordering::Relaxed)
        {
            log::info!("cdma-ms: forward traffic confirmed after {seen} good frames");
        }
    }

    pub fn forward_confirmed(&self) -> bool {
        self.forward_confirmed.load(Ordering::Relaxed)
    }

    pub fn forward_lost(&self) -> bool {
        self.forward_lost.load(Ordering::Relaxed)
    }

    pub fn take_power_measurements(&self) -> (u32, u32) {
        (
            self.measured_frames.swap(0, Ordering::Relaxed),
            self.measured_bad_frames.swap(0, Ordering::Relaxed),
        )
    }

    pub fn activate(&self, walsh: u8, frame_offset: u8, esn: u32) {
        log::debug!(
            "ms_ftraffic: activated walsh={} frame_offset={} esn=0x{:08x}",
            walsh,
            frame_offset,
            esn
        );
        self.walsh.store(walsh as u32, Ordering::Relaxed);
        self.frame_offset
            .store(frame_offset as u32, Ordering::Relaxed);
        self.esn.store(esn, Ordering::Relaxed);
        self.good_forward_frames.store(0, Ordering::Relaxed);
        self.forward_confirmed.store(false, Ordering::Relaxed);
        self.frames_since_good_trigger.store(0, Ordering::Relaxed);
        self.forward_lost.store(false, Ordering::Relaxed);
        self.measured_frames.store(0, Ordering::Relaxed);
        self.measured_bad_frames.store(0, Ordering::Relaxed);
        self.active.store(true, Ordering::Relaxed);
    }

    fn ready(&self) -> bool {
        self.active.load(Ordering::Relaxed) && self.anchor_set.load(Ordering::Relaxed)
    }
}

pub struct ForwardTrafficRc3Processor {
    config: Arc<ForwardTrafficConfig>,
    buf: Vec<Complex32>,
    buf_start_chip: usize,
    lc_cache: Option<(usize, u64)>,
    /// Diagnostic chip-offset search upper bound past the nominal frame boundary
    /// (0 = decode only at the nominal boundary).
    search: usize,
    search_step: usize,
    ack_chip: usize,
    reg_ack: Option<u64>,
    search_frames: usize,
    frames_active: usize,
    fpc_frames: usize,
    last_transform: Option<u8>,
    sar_reader: DedicatedFrameReader,
}

impl ForwardTrafficRc3Processor {
    pub fn new(config: Arc<ForwardTrafficConfig>) -> Self {
        let search = std::env::var("TEST_FTRAFFIC_SEARCH")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let search_step = std::env::var("TEST_FTRAFFIC_STEP")
            .ok()
            .and_then(|v| v.parse().ok())
            .filter(|&s| s > 0)
            .unwrap_or(1);
        let ack_chip = std::env::var("TEST_FTRAFFIC_ACK_CHIP")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        let search_frames = std::env::var("TEST_FTRAFFIC_SEARCH_FRAMES")
            .ok()
            .and_then(|v| v.parse().ok())
            .unwrap_or(0);
        Self {
            config,
            buf: Vec::new(),
            buf_start_chip: 0,
            lc_cache: None,
            search,
            search_step,
            ack_chip,
            reg_ack: None,
            search_frames,
            frames_active: 0,
            fpc_frames: 0,
            last_transform: None,
            sar_reader: DedicatedFrameReader::new(),
        }
    }

    /// Pilot-reference and W(n,64)-decover one 24576-chip frame into 384 QPSK
    /// symbols: per symbol the Walsh-0 pilot sum sets the phase reference and the
    /// assigned-Walsh sum carries the data.
    fn dewalsh_frame(&self, chips: &[Complex32], walsh: u8) -> Vec<Complex32> {
        let pilot_row = WalshGenerator::generate_matrix::<FORWARD_WALSH_CODES>()[0];
        let data_row = WalshGenerator::generate_matrix::<FORWARD_WALSH_CODES>()[walsh as usize];
        chips
            .chunks_exact(CHIPS_PER_SYMBOL)
            .take(SYMBOLS_PER_FRAME)
            .map(|chunk| {
                let mut pilot = Complex32::new(0.0, 0.0);
                let mut data = Complex32::new(0.0, 0.0);
                for (i, s) in chunk.iter().enumerate() {
                    pilot += *s * pilot_row[i] as f32;
                    data += *s * data_row[i] as f32;
                }
                data * pilot.conj()
            })
            .collect()
    }

    fn try_frames(&mut self, emitter: &mut dyn PipelineEmitter) {
        let paging_start = self.config.paging_start_chip.load(Ordering::Relaxed);
        if paging_start < 0 {
            return;
        }
        let paging_start = paging_start as usize;
        let walsh = self.config.walsh.load(Ordering::Relaxed) as u8;
        let frame_offset = self.config.frame_offset.load(Ordering::Relaxed) as u8;
        let esn = self.config.esn.load(Ordering::Relaxed);
        let lc_state = self.config.lc_state.load(Ordering::Relaxed) as u64;

        if self.ack_chip > 0 && self.reg_ack.is_none() {
            let mut lc = LongCodeGenerator::new_traffic_channel(esn);
            lc.advance_chips(self.ack_chip - 1);
            self.reg_ack = Some(lc.state());
            log::debug!(
                "ms_ftraffic: reg_ack for ack_chip={} = 0x{:x}",
                self.ack_chip,
                lc.state()
            );
        }

        loop {
            let start = self.buf_start_chip;
            let aligned =
                next_traffic_frame_chip(start as u64, paging_start as u64, frame_offset) as usize;
            let offset = aligned - start;
            let sweep = if self.ack_chip > 0 {
                FRAME_CHIPS
            } else {
                self.search
            };
            if offset + FRAME_CHIPS + sweep > self.buf.len() {
                let trim = offset.min(self.buf.len());
                if trim > 0 {
                    self.buf.drain(..trim);
                    self.buf_start_chip += trim;
                }
                return;
            }

            self.frames_active += 1;
            if self.frames_active <= 3 || self.frames_active % 4096 == 0 {
                log::debug!(
                    "ms_ftraffic: progress frame={} aligned={}",
                    self.frames_active,
                    aligned
                );
            }

            // Seed the long code one chip before the nominal frame boundary.
            let lc_nominal = match self.lc_cache {
                Some((prev_chip, prev_state)) if aligned == prev_chip + FRAME_CHIPS => {
                    let mut lc = LongCodeGenerator::new_traffic_channel(esn);
                    lc.set_state(prev_state);
                    lc.advance_chips(FRAME_CHIPS);
                    lc.state()
                }
                _ => {
                    let delta = aligned - paging_start;
                    let mut lc = LongCodeGenerator::new_traffic_channel(esn);
                    lc.set_state(lc_state);
                    lc.advance_chips(delta.saturating_sub(1));
                    lc.state()
                }
            };
            self.lc_cache = Some((aligned, lc_nominal));

            let (search, step) = if self.ack_chip > 0
                || (self.search_frames > 0 && self.frames_active <= self.search_frames)
            {
                (FRAME_CHIPS - CHIPS_PER_SYMBOL, CHIPS_PER_SYMBOL)
            } else {
                (self.search, self.search_step)
            };

            let mut lc_delta = LongCodeGenerator::new_traffic_channel(esn);
            lc_delta.set_state(lc_nominal);
            let mut delta = 0usize;
            let mut fpc_candidate = None;
            let mut decoded_good = false;
            let mut voice_this_frame = false;
            'search: while delta <= search {
                let base = offset + delta;
                if base + FRAME_CHIPS > self.buf.len() {
                    break;
                }
                let lc_prev = lc_delta.state();
                for _ in 0..step {
                    lc_delta.next_chip();
                }
                if let Some(reg) = self.reg_ack {
                    if lc_prev != reg {
                        delta += step;
                        continue;
                    }
                    log::debug!(
                        "ms_ftraffic: reg match at aligned={} delta={}",
                        aligned,
                        delta
                    );
                }
                let frame = &self.buf[base..base + FRAME_CHIPS];
                let qpsk = self.dewalsh_frame(frame, walsh);
                if delta == 0 {
                    if let Some(xf) = self.last_transform {
                        fpc_candidate = Some((
                            qpsk.iter().map(|s| transform_qpsk(*s, xf)).collect(),
                            aligned,
                        ));
                    }
                }

                if self.reg_ack.is_some() {
                    let mat = WalshGenerator::generate_matrix::<FORWARD_WALSH_CODES>();
                    let chunk = &frame[4 * CHIPS_PER_SYMBOL..5 * CHIPS_PER_SYMBOL];
                    let spectrum: Vec<i32> = (0..64)
                        .map(|k| {
                            let mut acc = Complex32::new(0.0, 0.0);
                            for (i, s) in chunk.iter().enumerate() {
                                acc += *s * mat[k][i] as f32;
                            }
                            acc.norm() as i32
                        })
                        .collect();
                    let (argmax, peak) = spectrum
                        .iter()
                        .enumerate()
                        .max_by_key(|(_, v)| **v)
                        .map(|(i, v)| (i, *v))
                        .unwrap_or((0, 0));
                    log::debug!(
                        "ms_ftraffic: reg walsh W{}={} peak=W{}({}) spectrum={:?}",
                        walsh,
                        spectrum[walsh as usize],
                        argmax,
                        peak,
                        spectrum
                    );
                }

                // A short-rate CRC can pass under the wrong I/Q convention.
                // Prefer the convention that decoded the preceding frame.
                let preferred_transform = self.last_transform;
                let transforms = preferred_transform
                    .into_iter()
                    .chain((0u8..8).filter(|candidate| Some(*candidate) != preferred_transform));
                for xf in transforms {
                    let symbols: Vec<Complex32> =
                        qpsk.iter().map(|s| transform_qpsk(*s, xf)).collect();
                    if let Some(decoded) =
                        decode_forward_rc3_signaling_frame_from_lc_state(&symbols, esn, lc_prev)
                    {
                        if decoded.fqi_valid || self.reg_ack.is_some() {
                            log::debug!(
                                "ms_ftraffic: decode chip={} delta={} xf={} fqi={} prefix={:?} fdsch={}",
                                aligned + delta,
                                delta,
                                xf,
                                decoded.fqi_valid,
                                &decoded.info_bits[..5.min(decoded.info_bits.len())],
                                decoded.fdsch.is_some(),
                            );
                        }
                        if decoded.fqi_valid {
                            decoded_good = true;
                            self.last_transform = Some(xf);
                            fpc_candidate = Some((symbols, aligned + delta));
                            let decoded_signaling =
                                decode_signaling_fragment(&mut self.sar_reader, &decoded.info_bits);
                            match decoded_signaling {
                                Some(DecodedForwardSignaling::Pdu(pdu)) => {
                                    emit_signaling(emitter, &self.config, &pdu);
                                }
                                Some(DecodedForwardSignaling::Unsupported(header)) => {
                                    self.config.signaling.push_unsupported(header);
                                    let mut block = SampleBlock::new(Vec::new(), 0);
                                    block.tags.insert(TRAFFIC_SIGNALING_EVENT, 1);
                                    emitter.emit(block);
                                }
                                None => {}
                            }
                            let voice_bits = match decoded.rate {
                                cdma_common::channel::TrafficRate::Full
                                    if decoded.info_bits.first() == Some(&0) =>
                                {
                                    Some(decoded.info_bits[1..].to_vec())
                                }
                                cdma_common::channel::TrafficRate::Full => None,
                                _ => Some(decoded.info_bits.clone()),
                            };
                            if let Some(voice_bits) = voice_bits {
                                let rate_bps = match decoded.rate {
                                    cdma_common::channel::TrafficRate::Full => 9_600,
                                    cdma_common::channel::TrafficRate::Half => 4_800,
                                    cdma_common::channel::TrafficRate::Quarter => 2_700,
                                    cdma_common::channel::TrafficRate::Eighth => 1_500,
                                };
                                self.config.voice.lock().push((rate_bps, voice_bits));
                                voice_this_frame = true;
                                let mut block = SampleBlock::new(Vec::new(), 0);
                                block.tags.insert(TRAFFIC_VOICE_EVENT, 1);
                                emitter.emit(block);
                            }
                            break 'search;
                        }
                    }
                }
                delta += step;
            }
            self.config.note_forward_frame(decoded_good);
            // A 20 ms interval that produced no speech frame (lost, or a
            // signaling-only frame with the primary blanked) is an erasure. The
            // decoder conceals it downstream so playout stays aligned to air
            // time. rate_bps 0 with no bits is the marker.
            if !voice_this_frame {
                self.config.voice.lock().push((0, Vec::new()));
                let mut block = SampleBlock::new(Vec::new(), 0);
                block.tags.insert(TRAFFIC_VOICE_EVENT, 1);
                emitter.emit(block);
            }
            if let Some((symbols, chip)) = fpc_candidate {
                if let Some(bits) =
                    rc3_power_control_bits(&symbols, esn, paging_start, lc_state, chip)
                {
                    let down = bits.iter().filter(|&&bit| bit == 1).count() as u8;
                    let up = bits.len() as u8 - down;
                    if self.fpc_frames < 5 || self.fpc_frames % 50 == 0 {
                        log::debug!(
                            "ms_ftraffic: fpc frame={} up={} down={} xf={:?}",
                            self.fpc_frames,
                            up,
                            down,
                            self.last_transform,
                        );
                    }
                    self.config.power_control.lock().push((up, down));
                    self.fpc_frames += 1;
                    let mut block = SampleBlock::new(Vec::new(), 0);
                    block.tags.insert(TRAFFIC_POWER_CONTROL_EVENT, 1);
                    emitter.emit(block);
                }
            }
            if self.frames_active % 50 == 0 {
                log::debug!(
                    "ms_ftraffic: frames={} fpc_frames={} transform={:?}",
                    self.frames_active,
                    self.fpc_frames,
                    self.last_transform,
                );
            }

            let consumed = offset + FRAME_CHIPS;
            self.buf.drain(..consumed);
            self.buf_start_chip += consumed;
        }
    }
}

fn transform_qpsk(s: Complex32, xf: u8) -> Complex32 {
    let (re, im) = (s.re, s.im);
    match xf {
        0 => Complex32::new(re, im),
        1 => Complex32::new(re, -im),
        2 => Complex32::new(-re, im),
        3 => Complex32::new(-re, -im),
        4 => Complex32::new(im, re),
        5 => Complex32::new(im, -re),
        6 => Complex32::new(-im, re),
        _ => Complex32::new(-im, -re),
    }
}

fn emit_signaling(
    emitter: &mut dyn PipelineEmitter,
    config: &ForwardTrafficConfig,
    pdu: &FdschPdu,
) {
    config.signaling.push(pdu.clone());
    let mut block = SampleBlock::new(Vec::new(), 0);
    block.tags.insert(TRAFFIC_SIGNALING_EVENT, 1);
    if let FdschMessage::DataBurst(m) = &pdu.body {
        block
            .tags
            .insert(TRAFFIC_SIGNALING_BURST_TYPE, m.burst_type as i64);
        // Carry the CHARi bytes as one byte per sample (real axis).
        block.samples = m
            .fields
            .iter()
            .map(|&b| Complex32::new(b as f32, 0.0))
            .collect();
    }
    emitter.emit(block);
}

#[cfg(test)]
mod power_control_tests {
    use super::*;
    use cdma_bts::channels::ftch_rc3::{ConfigRc3, Rc3PcgPcbScheduler};
    use cdma_bts::lac::paging_messages::{
        NonNegServiceConfig, ServiceConnectConnectionRecord, ServiceConnectParams,
    };
    use cdma_bts::lac::{assemble_f_dsch_pdu, sar_fragment_ftch_pdu_dsch};
    use cdma_bts::phy::coding::block_interleaver::{
        ForwardBackwardsBitReversalInterleaver, SR1_PARAMS_768,
    };
    use cdma_bts::phy::coding::convolutional::get_1_4_k9_encoder;
    use cdma_common::consts::RC3_GATED_REV_PWR_CNTL_DELAY;
    use cdma_common::lac::message_types::{MessageId, WireChannel};
    use cdma_common::time::CdmaSystemTime;

    #[test]
    fn forward_acknowledgments_survive_iq_convention_search() {
        use crate::forward_rx::directed::ORDER_BS_ACK;
        use cdma_bts::receiver::pipelined::VecEmitter;
        use cdma_common::lac::paging_messages::OrderMessage;
        const ESN: u32 = 0x1234_5678;
        const WALSH: u8 = 4;
        const SEQUENCES: u8 = 8;
        let config = Arc::new(ForwardTrafficConfig::default());
        config.set_anchor(0, LongCodeGenerator::new_traffic_channel(ESN).state());
        config.activate(WALSH, 0, ESN);
        let mut receiver = ForwardTrafficRc3Processor::new(config.clone());
        let channel = ForwardTrafficChannelRc3::new(ConfigRc3 {
            encoder: get_1_4_k9_encoder(),
            interleaver: ForwardBackwardsBitReversalInterleaver::new(SR1_PARAMS_768),
            scrambling_lc: LongCodeGenerator::new_traffic_channel(ESN),
            puncture_lc: LongCodeGenerator::new_traffic_channel(ESN),
            lc_chip_cursor: 0,
            previous_pcg_pc_start: 0,
            pcb_scheduler: Rc3PcgPcbScheduler::new(RC3_GATED_REV_PWR_CNTL_DELAY),
            fpc_subchan_gain_linear: 1.0,
            prev_frame_last_chip: 0,
            disable_lc_scrambling: false,
        });
        channel.advance_lc_to_chip(FRAME_CHIPS as u64);
        let cover = WalshGenerator::generate_matrix::<CHIPS_PER_SYMBOL>()[WALSH as usize];
        let order = OrderMessage {
            order: ORDER_BS_ACK,
            ordq: 0,
            order_specific_fields: Vec::new(),
        };
        let wire = MessageId::Order
            .wire_type(WireChannel::ForwardDedicated)
            .unwrap();
        let mut frame_chip = FRAME_CHIPS;
        let mut expected = Vec::new();
        for ack_seq in 0..SEQUENCES {
            for msg_seq in 0..SEQUENCES {
                let pdu = assemble_f_dsch_pdu(wire, &order.to_ftch_sdu(), ack_seq, msg_seq, false);
                let frames = sar_fragment_ftch_pdu_dsch(&pdu);
                assert_eq!(frames.len(), 1);
                channel.send_signaling_bits(frames[0].bits().to_vec());
                let symbols = channel.next(CdmaSystemTime::default());
                let chips = symbols
                    .into_iter()
                    .flat_map(|symbol| {
                        cover
                            .into_iter()
                            .map(move |chip| Complex32::new(1.0, 0.0) + symbol.conj() * chip as f32)
                    })
                    .collect();
                receiver.process_block_emitting(
                    SampleBlock::new(chips, frame_chip),
                    &mut VecEmitter::new(),
                );
                expected.push((ack_seq, msg_seq));
                frame_chip += FRAME_CHIPS;
            }
        }
        let actual: Vec<_> = config
            .signaling
            .drain()
            .into_iter()
            .map(|pdu| (pdu.arq.ack_seq, pdu.arq.msg_seq))
            .collect();
        assert_eq!(actual, expected);
    }

    #[test]
    fn decodes_rc3_fpc_bits_from_transmitted_symbols() {
        let esn = 0x1234_5678;
        let frame_chip = (FRAME_CHIPS * 100) as u64;
        let channel = ForwardTrafficChannelRc3::new(ConfigRc3 {
            encoder: get_1_4_k9_encoder(),
            interleaver: ForwardBackwardsBitReversalInterleaver::new(SR1_PARAMS_768),
            scrambling_lc: LongCodeGenerator::new_traffic_channel(esn),
            puncture_lc: LongCodeGenerator::new_traffic_channel(esn),
            lc_chip_cursor: 0,
            previous_pcg_pc_start: 0,
            pcb_scheduler: Rc3PcgPcbScheduler::new(3),
            fpc_subchan_gain_linear: 1.4,
            prev_frame_last_chip: 0,
            disable_lc_scrambling: false,
        });
        channel.advance_lc_to_chip(frame_chip);
        let expected: Vec<u8> = (0..PCGS_PER_FRAME)
            .map(|pcg| (pcg % 3 == 0) as u8)
            .collect();
        for (pcg, bit) in expected.iter().enumerate() {
            assert!(
                channel
                    .schedule_power_control_bit(frame_chip / PCG_CHIPS as u64 + pcg as u64, *bit,)
            );
        }
        let symbols = channel.next(CdmaSystemTime::default());
        let anchor_state = LongCodeGenerator::new_traffic_channel(esn).state();
        assert_eq!(
            rc3_power_control_bits(&symbols, esn, 0, anchor_state, frame_chip as usize),
            Some(expected),
        );
    }

    #[test]
    fn reassembles_multiframe_service_connect() {
        let params = ServiceConnectParams {
            serv_con_seq: 3,
            use_old_serv_config: 0,
            for_mux_option: 1,
            rev_mux_option: 1,
            for_rates: 0xf0,
            rev_rates: 0xf0,
            sync_id: None,
            connections: vec![ServiceConnectConnectionRecord {
                con_ref: 0,
                service_option: 3,
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
        let wire = MessageId::ServiceConnect
            .wire_type(WireChannel::ForwardDedicated)
            .expect("Service Connect wire type");
        let pdu = assemble_f_dsch_pdu(wire, &params.to_ftch_sdu(), 0, 2, true);
        let frames = sar_fragment_ftch_pdu_dsch(&pdu);
        assert!(frames.len() > 1);

        let mut reader = DedicatedFrameReader::new();
        let mut decoded = None;
        for frame in &frames {
            decoded = decode_signaling_fragment(&mut reader, frame.bits());
        }
        let DecodedForwardSignaling::Pdu(decoded) = decoded.expect("complete Service Connect")
        else {
            panic!("expected decoded Service Connect");
        };
        assert!(matches!(decoded.body, FdschMessage::ServiceConnect(_)));
        assert_eq!(decoded.arq.msg_seq, 2);
    }

    #[test]
    fn status_request_retains_arq_header() {
        let pdu = Bitstream::new_bytes(&[0x10, 0x0a, 0x01, 0x20, 0x80, 0x01, 0x08]);
        let frames = sar_fragment_ftch_pdu_dsch(&pdu);
        let mut reader = DedicatedFrameReader::new();
        let DecodedForwardSignaling::Pdu(decoded) =
            decode_signaling_fragment(&mut reader, frames[0].bits())
                .expect("CRC-valid Status Request")
        else {
            panic!("expected decoded Status Request");
        };
        assert_eq!(decoded.message_id, MessageId::StatusRequest);
        assert_eq!(decoded.arq.msg_seq, 2);
        assert!(decoded.arq.ack_req);
        assert!(matches!(decoded.body, FdschMessage::StatusRequest(_)));
    }

    #[test]
    fn forward_fade_expires_after_t5m_without_a_good_trigger() {
        let config = ForwardTrafficConfig::default();
        config.note_forward_frame(true);
        config.note_forward_frame(true);
        for _ in 0..ForwardTrafficConfig::FADE_TIMER_FRAMES - 1 {
            config.note_forward_frame(false);
        }
        assert!(!config.forward_lost());
        config.note_forward_frame(false);
        assert!(config.forward_lost());
    }
}

impl PipelineProcessor for ForwardTrafficRc3Processor {
    fn process_block(&mut self, block: SampleBlock) -> Vec<SampleBlock> {
        vec![block]
    }

    fn process_block_emitting(
        &mut self,
        block: SampleBlock,
        emitter: &mut dyn PipelineEmitter,
    ) -> Vec<SampleBlock> {
        if let Some(&epoch) = block.tags.get("pn_epoch_sample") {
            self.config.set_pn_epoch(epoch);
        }
        if let Some(&slew) = block.tags.get("pilot_slew_samples") {
            self.config.set_pilot_slew(slew);
        }
        if self.config.ready() {
            let expected = self.buf_start_chip + self.buf.len();
            if !self.buf.is_empty() && block.chip_start != expected {
                log::debug!(
                    "ms_ftraffic: stream gap: expected chip {} got {} (jump {})",
                    expected,
                    block.chip_start,
                    block.chip_start as i64 - expected as i64,
                );
                self.buf.clear();
                self.sar_reader.reset();
            }
            if self.buf.is_empty() {
                self.buf_start_chip = block.chip_start;
            }
            self.buf.extend_from_slice(&block.samples);
            self.try_frames(emitter);
        } else {
            self.last_transform = None;
            self.sar_reader.reset();
        }
        vec![block]
    }

    fn name(&self) -> &'static str {
        "ForwardTrafficRc3Processor"
    }
}
