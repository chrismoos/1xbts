//! Forward Supplemental Channel (F-SCH) for RC3.
//!
//! Per IS-2000 C.S0002-E, the RC3 F-SCH uses:
//!   N info + 16 CRC-16 + 8 tail bits
//!   → R=1/4 K=9 convolutional encode
//!   → interleave using the selected rate's block size
//!   → LC scramble (32-chip pair extractor, same PLCM as F-FCH)
//!   → signal-point map → I/Q demux → rate-specific Walsh spread
//!
//! The modulation symbol rate scales with the selected Walsh length:
//! W(32)=19.2 kbps, W(16)=38.4 kbps, W(8)=76.8 kbps, and W(4)=153.6 kbps.

use parking_lot::Mutex;
use std::collections::VecDeque;

use cdma_common::sch::Rc3FschProfile;
use cdma_common::time::CdmaSystemTime;
use log::trace;
use num::complex::Complex32;

use crate::phy::coding::{
    block_interleaver::{
        ForwardBackwardsBitReversalInterleaver, InterleaverParams, SR1_PARAMS_1536,
        SR1_PARAMS_3072, SR1_PARAMS_6144, SR1_PARAMS_12288,
    },
    convolutional::{Encoder, get_1_4_k9_encoder},
    long_code::LongCodeGenerator,
};

use super::Channel;
use cdma_common::consts::SR1_PCGS_PER_FRAME;

// Re-use crc16 from ftch_rc3
use super::ftch_rc3::crc16;

/// CRC bits for F-SCH (CRC-16 for all SCH rates).
const SCH_CRC_BITS: usize = 16;

/// Encoder tail bits.
const SCH_TAIL_BITS: usize = 8;

const LONG_CODE_PERIOD: u64 = (1u64 << 42) - 1;

/// Chips per PCG on Spreading Rate 1.
const PCG_CHIPS: usize = 1536;

pub fn interleaver_params(profile: Rc3FschProfile) -> InterleaverParams {
    match profile.rate_bps {
        19_200 => SR1_PARAMS_1536,
        38_400 => SR1_PARAMS_3072,
        76_800 => SR1_PARAMS_6144,
        153_600 => SR1_PARAMS_12288,
        _ => SR1_PARAMS_1536,
    }
}

struct PreparedSchFrame {
    interleaved: Vec<u8>,
    next_pcg: usize,
    frame_start_chip: u64,
}

struct SchTxState {
    symbol_buffer: VecDeque<Complex32>,
    pcg_scratch: Vec<Complex32>,
    prepared_frame: Option<PreparedSchFrame>,
}

pub struct SchConfigRc3 {
    pub profile: Rc3FschProfile,
    pub encoder: Encoder<9, 4>,
    pub interleaver: ForwardBackwardsBitReversalInterleaver,
    pub scrambling_lc: LongCodeGenerator,
    pub puncture_lc: LongCodeGenerator,
    pub lc_chip_cursor: u64,
    /// Gain for the SCH relative to pilot, from FPC_SCH_INIT_SETPT.
    pub sch_gain_linear: f32,
    pub prev_frame_last_chip: u8,
    pub frame_pcg_index: usize,
    pub disable_lc_scrambling: bool,
}

/// SCH frame prep state.
struct PrepEngine {
    profile: Rc3FschProfile,
    encoder: Encoder<9, 4>,
    interleaver: ForwardBackwardsBitReversalInterleaver,
}

/// RC3 Forward Supplemental Channel (F-SCH).
///
/// Operates on a separate supplemental Walsh code from the F-FCH but shares
/// the same long-code mask (PLCM). Shorter Walsh lengths carry proportionally
/// more QPSK symbols per 20 ms frame.
pub struct ForwardSupplementalChannelRc3 {
    config: Mutex<SchConfigRc3>,
    tx_state: Mutex<SchTxState>,
    prep: Mutex<PrepEngine>,
    frames: Mutex<VecDeque<PreparedSchFrame>>,
}

/// Build the complete encoder input (info + CRC-16 + tail) for an SCH frame.
fn build_sch_frame_bits(profile: Rc3FschProfile, data: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(profile.frame_bits());

    // Info bits (pad or truncate to the selected profile).
    let info_len = data.len().min(profile.info_bits);
    frame.extend_from_slice(&data[..info_len]);
    for _ in info_len..profile.info_bits {
        frame.push(0);
    }

    // CRC-16 over info bits, MSB first
    let crc = crc16(&frame[..profile.info_bits]);
    for bit in (0..SCH_CRC_BITS).rev() {
        frame.push(((crc >> bit) & 1) as u8);
    }

    // Encoder tail bits (8 zeros)
    for _ in 0..SCH_TAIL_BITS {
        frame.push(0);
    }

    debug_assert_eq!(frame.len(), profile.frame_bits());
    frame
}

impl ForwardSupplementalChannelRc3 {
    pub fn new(config: SchConfigRc3) -> Self {
        let profile = config.profile;
        let prep = PrepEngine {
            profile,
            encoder: get_1_4_k9_encoder(),
            interleaver: ForwardBackwardsBitReversalInterleaver::new(interleaver_params(profile)),
        };
        ForwardSupplementalChannelRc3 {
            config: Mutex::new(config),
            tx_state: Mutex::new(SchTxState {
                symbol_buffer: VecDeque::new(),
                pcg_scratch: Vec::new(),
                prepared_frame: None,
            }),
            prep: Mutex::new(prep),
            frames: Mutex::new(VecDeque::new()),
        }
    }

    /// Create an F-SCH channel with default configuration.
    pub fn new_default(esn: u32) -> Self {
        let profile = Rc3FschProfile::default_19k2();
        Self::new(SchConfigRc3 {
            encoder: get_1_4_k9_encoder(),
            interleaver: ForwardBackwardsBitReversalInterleaver::new(interleaver_params(profile)),
            scrambling_lc: LongCodeGenerator::new_traffic_channel(esn),
            puncture_lc: LongCodeGenerator::new_traffic_channel(esn),
            profile,
            lc_chip_cursor: 0,
            sch_gain_linear: 1.0,
            prev_frame_last_chip: 0,
            frame_pcg_index: 0,
            disable_lc_scrambling: false,
        })
    }

    /// Queue SCH MAC SDU content. The active profile pads/truncates to its
    /// configured info-bit count.
    pub fn send_frame(&self, data: Vec<u8>) {
        let prepared = {
            let mut prep = self.prep.lock();
            Self::prepare_frame_static(&mut prep, &data)
        };
        self.frames.lock().push_back(prepared);
    }

    fn prepare_frame_static(prep: &mut PrepEngine, data: &[u8]) -> PreparedSchFrame {
        let profile = prep.profile;
        let frame_data = build_sch_frame_bits(profile, data);

        prep.encoder.reset();
        let mut encoded = Vec::with_capacity(profile.coded_symbols());
        for &bit in &frame_data {
            for &sym in prep.encoder.encode(bit).iter() {
                encoded.push(sym);
            }
        }
        debug_assert_eq!(encoded.len(), profile.coded_symbols());

        let interleaved = prep.interleaver.encode(&encoded);
        PreparedSchFrame {
            interleaved,
            next_pcg: 0,
            frame_start_chip: 0,
        }
    }

    /// Seed both long-code generators at the given CDMA chip position.
    pub fn advance_lc_to_chip(&self, chip: u64) {
        let mut config = self.config.lock();

        let previous_chip_start = if chip == 0 {
            LONG_CODE_PERIOD - 1
        } else {
            chip - 1
        };
        let mut probe = config.scrambling_lc.clone();
        let probe_delta = previous_chip_start.saturating_sub(config.lc_chip_cursor);
        probe.advance_chips(probe_delta as usize);
        config.prev_frame_last_chip = probe.next_chip();

        let delta = chip.saturating_sub(config.lc_chip_cursor);
        config.scrambling_lc.advance_chips(delta as usize);
        config.puncture_lc.advance_chips(delta as usize);
        config.lc_chip_cursor = chip;
        config.frame_pcg_index = 0;
    }

    /// Number of frames currently queued.
    pub fn queue_len(&self) -> usize {
        self.frames.lock().len()
    }

    pub fn profile(&self) -> Rc3FschProfile {
        self.config.lock().profile
    }

    /// Produce one 20ms frame of 768 complex (QPSK) symbols.
    pub fn next(&self, current_system_time: CdmaSystemTime) -> Vec<Complex32> {
        let profile = self.config.lock().profile;
        self.next_block(profile.qpsk_symbols(), current_system_time)
    }

    fn pop_next_frame(&self) -> Option<PreparedSchFrame> {
        self.frames.lock().pop_front()
    }

    fn emit_dtx_pcg_into(config: &mut SchConfigRc3, out: &mut Vec<Complex32>) {
        let mut probe = config.scrambling_lc.clone();
        probe.advance_chips(PCG_CHIPS - 1);
        config.prev_frame_last_chip = probe.next_chip();

        config.scrambling_lc.advance_chips(PCG_CHIPS);
        config.puncture_lc.advance_chips(PCG_CHIPS);
        config.lc_chip_cursor = config.lc_chip_cursor.saturating_add(PCG_CHIPS as u64);
        config.frame_pcg_index = (config.frame_pcg_index + 1) % SR1_PCGS_PER_FRAME;

        out.extend(std::iter::repeat_n(
            Complex32::new(0.0, 0.0),
            config.profile.symbols_per_pcg() / 2,
        ));
    }

    fn emit_next_pcg_into(
        &self,
        config: &mut SchConfigRc3,
        tx_state: &mut SchTxState,
        out: &mut Vec<Complex32>,
    ) {
        if tx_state.prepared_frame.is_none() {
            if config.frame_pcg_index != 0 {
                Self::emit_dtx_pcg_into(config, out);
                return;
            }
            let Some(mut prepared) = self.pop_next_frame() else {
                Self::emit_dtx_pcg_into(config, out);
                return;
            };
            prepared.frame_start_chip = config.lc_chip_cursor;
            tx_state.prepared_frame = Some(prepared);
        }

        let prepared = tx_state
            .prepared_frame
            .as_mut()
            .expect("prepared SCH frame must exist");
        let pcg_index = prepared.next_pcg;
        let profile = config.profile;
        let symbols_per_pcg = profile.symbols_per_pcg();
        let start = pcg_index * symbols_per_pcg;
        let end = start + symbols_per_pcg;

        // LC scramble + signal-point map. Reverse power-control bits are
        // transmitted on F-FCH/F-DCCH only, not punctured into F-SCH.
        let lc_decimation = profile.lc_decimation();
        let mut previous_chip = config.prev_frame_last_chip;
        out.reserve(symbols_per_pcg / 2);
        for pair in prepared.interleaved[start..end].chunks_exact(2) {
            let q_chip = previous_chip;
            let i_chip = config.scrambling_lc.next_chip();
            previous_chip = i_chip;
            // Advance LC through the remaining chips of this 2-symbol group
            // Group = 2 mod symbols × LC decimation chips.
            for _ in 0..((2 * lc_decimation) - 1) {
                previous_chip = config.scrambling_lc.next_chip();
            }

            let i_bit = if config.disable_lc_scrambling {
                pair[0]
            } else {
                pair[0] ^ i_chip
            };
            let q_bit = if config.disable_lc_scrambling {
                pair[1]
            } else {
                pair[1] ^ q_chip
            };
            let i = if i_bit == 0 { 1.0f32 } else { -1.0f32 };
            let q = if q_bit == 0 { 1.0f32 } else { -1.0f32 };
            out.push(Complex32::new(i, q));
        }

        config.prev_frame_last_chip = previous_chip;
        config.lc_chip_cursor = config.lc_chip_cursor.saturating_add(PCG_CHIPS as u64);
        config.frame_pcg_index = (config.frame_pcg_index + 1) % SR1_PCGS_PER_FRAME;
        prepared.next_pcg += 1;
        if prepared.next_pcg == SR1_PCGS_PER_FRAME {
            trace!(
                "tx_fsch_rc3_frame: start_chip={} end_chip={} mod_symbols={} qpsk_symbols={}",
                prepared.frame_start_chip,
                config.lc_chip_cursor,
                profile.coded_symbols(),
                profile.qpsk_symbols(),
            );
            tx_state.prepared_frame = None;
        }
    }
}

impl Channel for ForwardSupplementalChannelRc3 {
    fn next_block(&self, num_samples: usize, system_time: CdmaSystemTime) -> Vec<Complex32> {
        let mut out = Vec::with_capacity(num_samples);
        self.next_block_into(&mut out, num_samples, system_time);
        out
    }

    fn next_block_into(
        &self,
        out: &mut Vec<Complex32>,
        num_samples: usize,
        _system_time: CdmaSystemTime,
    ) {
        let mut config = self.config.lock();
        let mut tx_state = self.tx_state.lock();
        let target_len = out.len() + num_samples;
        out.reserve(num_samples);

        while out.len() < target_len {
            if let Some(sample) = tx_state.symbol_buffer.pop_front() {
                out.push(sample);
                continue;
            }

            let pcg_samples = config.profile.symbols_per_pcg() / 2;
            if target_len - out.len() >= pcg_samples {
                self.emit_next_pcg_into(&mut config, &mut tx_state, out);
                continue;
            }

            let mut scratch = std::mem::take(&mut tx_state.pcg_scratch);
            scratch.clear();
            self.emit_next_pcg_into(&mut config, &mut tx_state, &mut scratch);

            let needed = target_len - out.len();
            out.extend(scratch.iter().take(needed).copied());
            tx_state
                .symbol_buffer
                .extend(scratch.iter().skip(needed).copied());
            tx_state.pcg_scratch = scratch;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sch_frame_produces_correct_symbol_count() {
        let sch = ForwardSupplementalChannelRc3::new_default(0xDEADBEEF);
        let profile = Rc3FschProfile::default_19k2();
        // Send a data frame
        sch.send_frame(vec![1u8; profile.info_bits]);
        let symbols = sch.next(CdmaSystemTime::default());
        assert_eq!(
            symbols.len(),
            profile.qpsk_symbols(),
            "F-SCH should produce {} QPSK symbols per frame",
            profile.qpsk_symbols()
        );
    }

    #[test]
    fn sch_153k6_frame_produces_correct_symbol_count() {
        let profile = Rc3FschProfile::from_rate_bps(153_600).unwrap();
        let sch = ForwardSupplementalChannelRc3::new(SchConfigRc3 {
            encoder: get_1_4_k9_encoder(),
            interleaver: ForwardBackwardsBitReversalInterleaver::new(interleaver_params(profile)),
            scrambling_lc: LongCodeGenerator::new_traffic_channel(0xDEADBEEF),
            puncture_lc: LongCodeGenerator::new_traffic_channel(0xDEADBEEF),
            profile,
            lc_chip_cursor: 0,
            sch_gain_linear: 1.0,
            prev_frame_last_chip: 0,
            frame_pcg_index: 0,
            disable_lc_scrambling: false,
        });
        sch.send_frame(vec![1u8; profile.info_bits]);

        let symbols = sch.next(CdmaSystemTime::default());

        assert_eq!(profile.frame_bits(), 3072);
        assert_eq!(profile.qpsk_symbols(), 6144);
        assert_eq!(symbols.len(), profile.qpsk_symbols());
    }

    #[test]
    fn sch_blank_frame_produces_correct_symbol_count() {
        let sch = ForwardSupplementalChannelRc3::new_default(0);
        let profile = Rc3FschProfile::default_19k2();
        // No queued SCH MAC SDU means DTX for this PCG/frame.
        let symbols = sch.next(CdmaSystemTime::default());
        assert_eq!(symbols.len(), profile.qpsk_symbols());
        assert!(symbols.iter().all(|s| s.re == 0.0 && s.im == 0.0));
    }

    #[test]
    fn sch_symbols_are_unit_magnitude() {
        let sch = ForwardSupplementalChannelRc3::new_default(0xAABBCCDD);
        let profile = Rc3FschProfile::default_19k2();
        sch.send_frame(vec![0u8; profile.info_bits]);
        let symbols = sch.next(CdmaSystemTime::default());
        for (i, s) in symbols.iter().enumerate() {
            // Each component should be ±1.0 (data) or ±gain (PC puncture)
            assert!(
                s.re.abs() > 0.0 && s.re.abs() <= 1.01,
                "symbol[{}].re = {} out of range",
                i,
                s.re
            );
            assert!(
                s.im.abs() > 0.0 && s.im.abs() <= 1.01,
                "symbol[{}].im = {} out of range",
                i,
                s.im
            );
        }
    }

    #[test]
    fn sch_lc_advances_correctly() {
        let sch = ForwardSupplementalChannelRc3::new_default(0x12345678);
        sch.advance_lc_to_chip(0);
        let _ = sch.next(CdmaSystemTime::default());
        // After one frame: should advance by SR1_PCGS_PER_FRAME * PCG_CHIPS chips
        let config = sch.config.lock();
        let expected_chips = (SR1_PCGS_PER_FRAME * PCG_CHIPS) as u64;
        assert_eq!(
            config.lc_chip_cursor, expected_chips,
            "LC should advance {} chips per frame (got {})",
            expected_chips, config.lc_chip_cursor
        );
        // 16 PCGs × 1536 chips/PCG = 24576 chips = one 20ms frame
        assert_eq!(expected_chips, 24576);
    }

    #[test]
    fn sch_queued_data_waits_for_next_20ms_boundary() {
        let sch = ForwardSupplementalChannelRc3::new_default(0xAABBCCDD);
        let profile = Rc3FschProfile::default_19k2();
        let qpsk_per_pcg = profile.symbols_per_pcg() / 2;

        let first_pcg = sch.next_block(qpsk_per_pcg, CdmaSystemTime::default());
        assert!(first_pcg.iter().all(|s| s.re == 0.0 && s.im == 0.0));

        sch.send_frame(vec![1u8; profile.info_bits]);

        let rest_of_frame = sch.next_block(qpsk_per_pcg * 15, CdmaSystemTime::default());
        assert!(rest_of_frame.iter().all(|s| s.re == 0.0 && s.im == 0.0));

        let first_data_pcg = sch.next_block(qpsk_per_pcg, CdmaSystemTime::default());
        assert!(
            first_data_pcg.iter().any(|s| s.re != 0.0 || s.im != 0.0),
            "queued SCH frame should start at the next 20ms boundary"
        );
    }

    #[test]
    fn build_sch_frame_bits_correct_length() {
        let profile = Rc3FschProfile::default_19k2();
        let data = vec![0u8; profile.info_bits];
        let frame = build_sch_frame_bits(profile, &data);
        assert_eq!(frame.len(), profile.frame_bits());
    }

    #[test]
    fn crc16_nonzero() {
        let data = vec![1u8; 360];
        let crc = crc16(&data);
        assert_ne!(crc, 0, "CRC-16 of non-trivial data should be non-zero");
    }

    #[test]
    fn crc16_all_zeros() {
        let data = vec![0u8; 360];
        let crc = crc16(&data);
        // CRC of all-zeros with init=0xFFFF should be non-zero
        assert_ne!(crc, 0);
    }
}
