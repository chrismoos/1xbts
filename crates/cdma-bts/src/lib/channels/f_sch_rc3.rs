//! Forward Supplemental Channel (F-SCH) for RC3 at 19.2 kbps.
//!
//! Per IS-2000 C.S0002-E, the F-SCH at 19.2 kbps uses:
//!   360 info + 16 CRC-16 + 8 tail = 384 bits
//!   → R=1/4 K=9 convolutional encode → 1536 code symbols
//!   → no repetition (1×)
//!   → interleave (1536 fwd-bwd bit-reversal)
//!   → LC scramble (32-chip pair extractor, same PLCM as F-FCH)
//!   → PC puncture (4 symbols per PCG, LC-derived position)
//!   → signal-point map → I/Q demux → W(n,32) Walsh spread
//!
//! The modulation symbol rate is 76,800 sps (vs 38,400 for F-FCH),
//! so each mod symbol spans 16 chips (vs 32 for F-FCH).

use parking_lot::Mutex;
use std::collections::VecDeque;

use cdma_common::time::CdmaSystemTime;
use log::trace;
use num::complex::Complex32;

use crate::phy::coding::{
    block_interleaver::{ForwardBackwardsBitReversalInterleaver, SR1_PARAMS_1536},
    convolutional::{Encoder, get_1_4_k9_encoder},
    long_code::LongCodeGenerator,
};

use super::{Channel, PcgPcbSchedulerHandle};
use cdma_common::consts::SR1_PCGS_PER_FRAME;

// Re-use crc16 from ftch_rc3
use super::ftch_rc3::crc16;

/// F-SCH info bits per 20ms frame at 19.2 kbps.
const SCH_INFO_BITS: usize = 360;

/// CRC bits for F-SCH (CRC-16 for all SCH rates).
const SCH_CRC_BITS: usize = 16;

/// Encoder tail bits.
const SCH_TAIL_BITS: usize = 8;

/// Total frame bits before encoding: 360 + 16 + 8 = 384.
const SCH_FRAME_BITS: usize = SCH_INFO_BITS + SCH_CRC_BITS + SCH_TAIL_BITS;

/// Modulation symbols per 20ms frame at 19.2 kbps.
/// 384 bits × 4 (R=1/4) = 1536 symbols.
const MOD_SYMBOLS_PER_FRAME: usize = 1536;

/// QPSK output symbols per 20ms frame after I/Q demux.
/// 1536 mod symbols → 768 complex (I+jQ) symbols.
const OUTPUT_SYMBOLS_PER_FRAME: usize = MOD_SYMBOLS_PER_FRAME / 2;

/// Modulation symbols per PCG (pre-demux).
const SYMBOLS_PER_PCG: usize = MOD_SYMBOLS_PER_FRAME / SR1_PCGS_PER_FRAME; // = 96

/// Number of modulation symbols punctured per PCG for power control.
const PC_PUNCTURE_SYMBOLS: usize = 4;

/// Chips per modulation symbol at 19.2 kbps SCH.
/// 1,228,800 chips/sec / 76,800 symbols/sec = 16 chips/symbol.
const LC_DECIMATION: usize = 16;

const LONG_CODE_PERIOD: u64 = (1u64 << 42) - 1;

/// Chips per PCG: 96 symbols × 16 chips/symbol = 1536 chips.
const PCG_CHIPS: usize = SYMBOLS_PER_PCG * LC_DECIMATION;

struct PreparedSchFrame {
    interleaved: Vec<u8>,
    next_pcg: usize,
    frame_start_chip: u64,
}

struct SchTxState {
    symbol_buffer: VecDeque<Complex32>,
    prepared_frame: Option<PreparedSchFrame>,
}

pub struct SchConfigRc3 {
    pub encoder: Encoder<9, 4>,
    pub interleaver: ForwardBackwardsBitReversalInterleaver,
    pub scrambling_lc: LongCodeGenerator,
    pub puncture_lc: LongCodeGenerator,
    pub lc_chip_cursor: u64,
    pub pcb_scheduler: PcgPcbSchedulerHandle,
    /// Gain for the SCH relative to pilot, from FPC_SCH_INIT_SETPT.
    pub sch_gain_linear: f32,
    pub prev_frame_last_chip: u8,
    pub disable_lc_scrambling: bool,
}

/// RC3 Forward Supplemental Channel (F-SCH) at 19.2 kbps.
///
/// Operates on a separate W(32) Walsh code from the F-FCH but shares the
/// same long-code mask (PLCM). Produces 768 complex QPSK symbols per 20ms
/// frame (twice the F-FCH output, since shorter Walsh = higher symbol rate).
pub struct ForwardSupplementalChannelRc3 {
    config: Mutex<SchConfigRc3>,
    tx_state: Mutex<SchTxState>,
    frames: Mutex<VecDeque<Vec<u8>>>,
    pcb_scheduler: PcgPcbSchedulerHandle,
}

/// Build the complete encoder input (info + CRC-16 + tail) for an SCH frame.
fn build_sch_frame_bits(data: &[u8]) -> Vec<u8> {
    let mut frame = Vec::with_capacity(SCH_FRAME_BITS);

    // Info bits (pad or truncate to SCH_INFO_BITS)
    let info_len = data.len().min(SCH_INFO_BITS);
    frame.extend_from_slice(&data[..info_len]);
    for _ in info_len..SCH_INFO_BITS {
        frame.push(0);
    }

    // CRC-16 over info bits, MSB first
    let crc = crc16(&frame[..SCH_INFO_BITS]);
    for bit in (0..SCH_CRC_BITS).rev() {
        frame.push(((crc >> bit) & 1) as u8);
    }

    // Encoder tail bits (8 zeros)
    for _ in 0..SCH_TAIL_BITS {
        frame.push(0);
    }

    debug_assert_eq!(frame.len(), SCH_FRAME_BITS);
    frame
}

impl ForwardSupplementalChannelRc3 {
    pub fn new(config: SchConfigRc3) -> Self {
        let pcb_scheduler = config.pcb_scheduler.clone();
        ForwardSupplementalChannelRc3 {
            config: Mutex::new(config),
            tx_state: Mutex::new(SchTxState {
                symbol_buffer: VecDeque::new(),
                prepared_frame: None,
            }),
            frames: Mutex::new(VecDeque::new()),
            pcb_scheduler,
        }
    }

    /// Create an F-SCH channel with default configuration.
    pub fn new_default(esn: u32) -> Self {
        Self::new(SchConfigRc3 {
            encoder: get_1_4_k9_encoder(),
            interleaver: ForwardBackwardsBitReversalInterleaver::new(SR1_PARAMS_1536),
            scrambling_lc: LongCodeGenerator::new_traffic_channel(esn),
            puncture_lc: LongCodeGenerator::new_traffic_channel(esn),
            lc_chip_cursor: 0,
            pcb_scheduler: crate::channels::PcgPcbScheduler::new(0),
            sch_gain_linear: 1.0,
            prev_frame_last_chip: 0,
            disable_lc_scrambling: false,
        })
    }

    /// Queue a frame (info bits) for transmission.
    /// The data should be SCH_INFO_BITS (360) bits of MUX PDU Type 2 content.
    pub fn send_frame(&self, data: Vec<u8>) {
        self.frames.lock().push_back(data);
    }

    /// Schedule a single power-control bit for an absolute PCG index.
    pub fn schedule_power_control_bit(&self, abs_pcg: u64, bit: u8) {
        self.pcb_scheduler.lock().schedule(abs_pcg, bit);
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
    }

    /// Number of frames currently queued.
    pub fn queue_len(&self) -> usize {
        self.frames.lock().len()
    }

    /// Produce one 20ms frame of 768 complex (QPSK) symbols.
    pub fn next(&self, current_system_time: CdmaSystemTime) -> Vec<Complex32> {
        self.next_block(OUTPUT_SYMBOLS_PER_FRAME, current_system_time)
    }

    fn pop_next_frame(&self) -> Vec<u8> {
        self.frames.lock().pop_front().unwrap_or_else(|| {
            // Blank/fill frame: MUX header = 0, rest zeros
            vec![0u8; SCH_INFO_BITS]
        })
    }

    fn prepare_frame(&self, config: &mut SchConfigRc3, data: Vec<u8>) -> PreparedSchFrame {
        // Step 1: Build complete frame (info + CRC-16 + tail)
        let frame_data = build_sch_frame_bits(&data);
        let frame_start_chip = config.lc_chip_cursor;

        // Step 2: Convolutional encode (R=1/4, K=9) — each bit → 4 symbols
        config.encoder.reset();
        let mut encoded = Vec::with_capacity(SCH_FRAME_BITS * 4);
        for &bit in &frame_data {
            for &sym in config.encoder.encode(bit).iter() {
                encoded.push(sym);
            }
        }
        debug_assert_eq!(encoded.len(), MOD_SYMBOLS_PER_FRAME);

        // Step 3: No repetition at 19.2 kbps (1×)
        // Step 4: No puncturing (1536 symbols = target)

        // Step 5: Forward-backwards bit-reversal interleave (1536 symbols)
        let interleaved = config.interleaver.encode(&encoded);

        PreparedSchFrame {
            interleaved,
            next_pcg: 0,
            frame_start_chip,
        }
    }

    fn emit_next_pcg(&self, config: &mut SchConfigRc3, tx_state: &mut SchTxState) {
        if tx_state.prepared_frame.is_none() {
            let data = self.pop_next_frame();
            tx_state.prepared_frame = Some(self.prepare_frame(config, data));
        }

        let prepared = tx_state
            .prepared_frame
            .as_mut()
            .expect("prepared SCH frame must exist");
        let pcg_index = prepared.next_pcg;
        let start = pcg_index * SYMBOLS_PER_PCG;
        let end = start + SYMBOLS_PER_PCG;

        let abs_pcg = config.lc_chip_cursor / PCG_CHIPS as u64;
        let pcb = self.pcb_scheduler.lock().read(abs_pcg);

        // PC puncture position decimator: extract one LC chip per mod symbol
        // (every LC_DECIMATION=16 chips), then use the last 4 bits of the PCG
        // to select the puncture start position.
        let mut pcg_bits = vec![0u8; SYMBOLS_PER_PCG];
        for bit in pcg_bits.iter_mut() {
            *bit = config.puncture_lc.next_chip();
            for _ in 0..(LC_DECIMATION - 1) {
                config.puncture_lc.next_chip();
            }
        }
        // Last 4 decimator outputs of the PCG select the position
        let b3 = pcg_bits[SYMBOLS_PER_PCG - 1] as usize;
        let b2 = pcg_bits[SYMBOLS_PER_PCG - 2] as usize;
        let b1 = pcg_bits[SYMBOLS_PER_PCG - 3] as usize;
        let b0 = pcg_bits[SYMBOLS_PER_PCG - 4] as usize;
        let pc_start = ((b3 << 3) | (b2 << 2) | (b1 << 1) | b0) * 2;

        // LC scramble + signal-point map + PC puncture
        let mut previous_chip = config.prev_frame_last_chip;
        let mut mapped = vec![0.0f32; SYMBOLS_PER_PCG];
        for (pair_idx, pair) in prepared.interleaved[start..end].chunks_exact(2).enumerate() {
            let q_chip = previous_chip;
            let i_chip = config.scrambling_lc.next_chip();
            previous_chip = i_chip;
            // Advance LC through the remaining chips of this 2-symbol group
            // Group = 2 mod symbols × LC_DECIMATION chips = 32 chips total
            for _ in 0..((2 * LC_DECIMATION) - 1) {
                previous_chip = config.scrambling_lc.next_chip();
            }

            let mod_index = pair_idx * 2;
            for lane in 0..2 {
                let symbol_in_pcg = mod_index + lane;
                let lc_scr = if lane == 0 { i_chip } else { q_chip };
                let scrambled = if config.disable_lc_scrambling {
                    pair[lane]
                } else {
                    pair[lane] ^ lc_scr
                };
                let is_pc =
                    symbol_in_pcg >= pc_start && symbol_in_pcg < pc_start + PC_PUNCTURE_SYMBOLS;
                mapped[symbol_in_pcg] = if is_pc {
                    let sign = if pcb == 0 { 1.0f32 } else { -1.0f32 };
                    sign * config.sch_gain_linear
                } else if scrambled == 0 {
                    1.0f32
                } else {
                    -1.0f32
                };
            }
        }

        // I/Q demux: consecutive pairs → complex QPSK symbols
        for pair in mapped.chunks_exact(2) {
            tx_state
                .symbol_buffer
                .push_back(Complex32::new(pair[0], pair[1]));
        }

        config.prev_frame_last_chip = previous_chip;
        config.lc_chip_cursor = config.lc_chip_cursor.saturating_add(PCG_CHIPS as u64);
        prepared.next_pcg += 1;
        if prepared.next_pcg == SR1_PCGS_PER_FRAME {
            trace!(
                "tx_fsch_rc3_frame: start_chip={} end_chip={} mod_symbols={} qpsk_symbols={}",
                prepared.frame_start_chip,
                config.lc_chip_cursor,
                MOD_SYMBOLS_PER_FRAME,
                OUTPUT_SYMBOLS_PER_FRAME,
            );
            tx_state.prepared_frame = None;
        }
    }
}

impl Channel for ForwardSupplementalChannelRc3 {
    fn next_block(&self, num_samples: usize, system_time: CdmaSystemTime) -> Vec<Complex32> {
        let _ = system_time;
        let mut config = self.config.lock();
        let mut tx_state = self.tx_state.lock();
        while tx_state.symbol_buffer.len() < num_samples {
            self.emit_next_pcg(&mut config, &mut tx_state);
        }
        tx_state
            .symbol_buffer
            .drain(..num_samples)
            .collect::<Vec<_>>()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sch_frame_produces_correct_symbol_count() {
        let sch = ForwardSupplementalChannelRc3::new_default(0xDEADBEEF);
        // Send a data frame
        sch.send_frame(vec![1u8; SCH_INFO_BITS]);
        let symbols = sch.next(CdmaSystemTime::default());
        assert_eq!(
            symbols.len(),
            OUTPUT_SYMBOLS_PER_FRAME,
            "F-SCH should produce {} QPSK symbols per frame",
            OUTPUT_SYMBOLS_PER_FRAME
        );
    }

    #[test]
    fn sch_blank_frame_produces_correct_symbol_count() {
        let sch = ForwardSupplementalChannelRc3::new_default(0);
        // No frame queued — should produce blank/fill
        let symbols = sch.next(CdmaSystemTime::default());
        assert_eq!(symbols.len(), OUTPUT_SYMBOLS_PER_FRAME);
    }

    #[test]
    fn sch_symbols_are_unit_magnitude() {
        let sch = ForwardSupplementalChannelRc3::new_default(0xAABBCCDD);
        sch.send_frame(vec![0u8; SCH_INFO_BITS]);
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
    fn build_sch_frame_bits_correct_length() {
        let data = vec![0u8; SCH_INFO_BITS];
        let frame = build_sch_frame_bits(&data);
        assert_eq!(frame.len(), SCH_FRAME_BITS);
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
