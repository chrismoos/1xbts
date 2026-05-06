use parking_lot::Mutex;
use std::collections::VecDeque;

pub(crate) use cdma_common::crc::crc16_sch as crc16;
use cdma_common::crc::{crc6, crc8, crc12};
use cdma_common::time::CdmaSystemTime;
use log::trace;
use num::complex::Complex32;

use crate::{
    mac::types::DataRequest,
    phy::coding::{
        block_interleaver::ForwardBackwardsBitReversalInterleaver, convolutional::Encoder,
        long_code::LongCodeGenerator, symbol_repeat::SymbolRepetition,
    },
};

use super::{Channel, PcgPcbSchedulerHandle};
use cdma_common::consts::SR1_PCGS_PER_FRAME;

/// Forward traffic channel frame for RC3.
/// Re-uses TrafficRate from ftch (same rate set, different encoding).
pub use super::ftch::TrafficRate;

/// Modulation symbols per 20ms frame (RC3, all rates after repeat + puncture).
/// Per Table 3.1.3.1.2.1-19: Modulation Symbol Rate = 38,400 sps at all rates.
const MOD_SYMBOLS_PER_FRAME: usize = 768;

/// QPSK output symbols per 20ms frame after I/Q demux.
/// 768 mod symbols → I/Q demux → 384 complex (I+jQ) symbols.
const OUTPUT_SYMBOLS_PER_FRAME: usize = MOD_SYMBOLS_PER_FRAME / 2;

/// Modulation symbols per PCG (pre-demux).
const SYMBOLS_PER_PCG: usize = MOD_SYMBOLS_PER_FRAME / SR1_PCGS_PER_FRAME; // = 48

/// Number of modulation symbols punctured per PCG for power control.
/// Per C.S0002-E Table 3.1.3.1.12-1 (RC3, non-TD): 4 symbols.
const PC_PUNCTURE_SYMBOLS: usize = 4;

/// Chips per RC3 modulation symbol at SR1 (1.2288 Mcps / 38,400 sps = 32).
/// Used for:
///   - Power-control puncture position extraction, where the decimator
///     (Figure 3.1.3.1.1.1-25) emits one bit per 32-chip mod symbol period
///     (`LC_DECIMATION` chip advances per decimator output).
///   - `scrambling_lc` group accounting: two mod symbols per group means
///     each group spans `2 * LC_DECIMATION = 64` chips of the long code.
///
/// Both `scrambling_lc` and `puncture_lc` advance at the full 1.2288 Mcps,
/// i.e. `24_576` chips per 20 ms frame.
const LC_DECIMATION: usize = 32;
const LONG_CODE_PERIOD: u64 = (1u64 << 42) - 1;
const PCG_CHIPS: usize = SYMBOLS_PER_PCG * LC_DECIMATION;

struct PreparedFrameRc3 {
    interleaved: Vec<u8>,
    rate: TrafficRate,
    next_pcg: usize,
    frame_start_chip: u64,
}

struct TxState {
    symbol_buffer: VecDeque<Complex32>,
    prepared_frame: Option<PreparedFrameRc3>,
}

/// A frame to be transmitted on the forward traffic channel.
pub struct TrafficFrameRc3 {
    /// The information bits to transmit (before CRC and tail).
    /// CRC and tail bits are computed automatically during encoding.
    pub data: Vec<u8>,
    /// The rate for this frame.
    pub rate: TrafficRate,
}

pub struct ConfigRc3 {
    pub encoder: Encoder<9, 4>,
    pub interleaver: ForwardBackwardsBitReversalInterleaver,
    /// Long-code generator driving data scrambling. Clocked at the full
    /// 1.2288 Mcps and interpreted over 2-symbol / 64-chip groups:
    ///   - even / I-lane symbol uses the chip valid at the group start,
    ///   - odd / Q-lane symbol uses the chip valid just prior to that start,
    ///   - `prev_frame_last_chip` carries `LC[frame_start - 1]` across frame
    ///     boundaries for the first odd / Q symbol of the frame.
    pub scrambling_lc: LongCodeGenerator,
    /// Long-code generator driving the forward-power-control puncture
    /// position decimator (Figure 3.1.3.1.1.1-25). Also clocked at the full
    /// 1.2288 Mcps: advanced by `LC_DECIMATION = 32` chips per modulation
    /// symbol, with the first chip of each 32-chip window emitted as the
    /// decimator output and the remaining 31 clocked through the LFSR
    /// without being used. 48 decimator outputs per power control group
    /// feed bits {44..47} of Table 3.1.3.1.12-1 (RC3 non-TD row) to select
    /// the puncture position.
    pub puncture_lc: LongCodeGenerator,
    pub lc_chip_cursor: u64,
    pub pcb_scheduler: PcgPcbSchedulerHandle,
    /// FPC subchannel gain as a linear amplitude ratio relative to data
    /// symbols. Per C.S0005-E, FPC_SUBCHAN_GAIN is in units of 0.25 dB
    /// relative to full-rate F-FCH. E.g. value 12 → 3.0 dB → 1.413×.
    pub fpc_subchan_gain_linear: f32,
    /// Carry-over long-code chip from the end of the previous 20 ms frame
    /// (= `LC[frame_start − 1]`). Supplies the Q-lane scrambling bit for
    /// the very first QPSK symbol of each frame.
    pub prev_frame_last_chip: u8,
    /// Diagnostic hook: keep LC timing/state progression intact but bypass
    /// the scrambling XOR at the modulation-symbol plane.
    pub disable_lc_scrambling: bool,
}

/// RC3 Forward Fundamental Channel (F-FCH).
///
/// Per IS-2000 C.S0002-E Figure 3.1.3.1.1.1-19, the forward RC3 F-FCH
/// processing chain is:
///   info+FQI+tail → R=1/4 K=9 encode → symbol repeat → puncture →
///   interleave (768 fwd-bwd bit-reversal) → LC scramble (64-chip pair extractor) →
///   PC puncture (4 symbols, LC-derived position) → signal-point map →
///   I/Q demux → W(n,64) Walsh spread → PN spread
///
/// Supports all four 20ms rates:
///   9600 bps: 172 info + 12 FQI + 8 tail = 192 → 768 sym (1× rep, no puncture)
///   4800 bps:  80 info +  8 FQI + 8 tail =  96 → 768 sym (2× rep, no puncture)
///   2700 bps:  40 info +  6 FQI + 8 tail =  54 → 768 sym (4× rep, 8/9 puncture)
///   1500 bps:  16 info +  6 FQI + 8 tail =  30 → 768 sym (8× rep, 4/5 puncture)
///
/// Produces 384 complex (QPSK) symbols per 20ms frame at all rates.
///
/// Limitations:
/// - Only 20ms framing (no 5ms F-FCH support)
/// - PLCM is ESN-only (PLCM_TYPE=0000)
pub struct ForwardTrafficChannelRc3 {
    config: Mutex<ConfigRc3>,
    tx_state: Mutex<TxState>,
    frames: Mutex<VecDeque<TrafficFrameRc3>>,
    signaling_frames: Mutex<VecDeque<TrafficFrameRc3>>,
    queue_diag: Mutex<QueueDiagState>,
    /// Timestamp of the last frame enqueued via `send_frame`.
    last_enqueue_at: Mutex<Option<std::time::Instant>>,
    /// Power-control bit scheduler. Stored outside `config` so
    /// `schedule_power_control_bit` never contends with `next_block`.
    pcb_scheduler: PcgPcbSchedulerHandle,
}

struct QueueDiagState {
    consecutive_nulls: u64,
    window_queued: u64,
    window_nulls: u64,
    total_queued: u64,
    total_nulls: u64,
    null_run_start: Option<std::time::Instant>,
    last_log_at: std::time::Instant,
}

/// Build the complete encoder input (info + FQI + tail) for an RC3 frame.
/// Per C.S0002-E Section 3.1.3.15.2, Table 3.1.3.15.2-1.
fn build_frame_bits_rc3(data: &[u8], rate: TrafficRate) -> Vec<u8> {
    let info_bits = rate.info_bits();
    let fqi_bits = rate.rc3_fqi_bits();
    let tail_bits = 8usize;
    let total = info_bits + fqi_bits + tail_bits;
    debug_assert_eq!(total, rate.rc3_frame_bits());

    let mut frame = Vec::with_capacity(total);

    // Info bits (zero-pad if data is shorter)
    for i in 0..info_bits {
        frame.push(if i < data.len() { data[i] } else { 0 });
    }

    // FQI (CRC) bits — MSB first
    match fqi_bits {
        12 => {
            let crc = crc12(&frame[..info_bits]);
            for bit in (0..12).rev() {
                frame.push(((crc >> bit) & 1) as u8);
            }
        }
        8 => {
            let crc = crc8(&frame[..info_bits]);
            for bit in (0..8).rev() {
                frame.push(((crc >> bit) & 1) as u8);
            }
        }
        6 => {
            let crc = crc6(&frame[..info_bits]);
            for bit in (0..6).rev() {
                frame.push(((crc >> bit) & 1) as u8);
            }
        }
        _ => {}
    }

    // Encoder tail bits (8 zeros)
    for _ in 0..tail_bits {
        frame.push(0);
    }

    frame
}

/// Puncture repeated symbols to 768 modulation symbols.
///
/// Per C.S0002-E 3.1.3.1.7.3 (flexible rate puncturing):
///   output[k] = input[floor(k * L / N)]  for k = 0..N-1
/// where L = input length, N = 768 (desired output).
///
/// Puncturing ratios per Table 3.1.3.1.2.1-19:
///   9600 bps: 1    (768 → 768, no puncture)
///   4800 bps: 1    (768 → 768, no puncture)
///   2700 bps: 8/9  (864 → 768)
///   1500 bps: 4/5  (960 → 768)
fn puncture_rc3(symbols: &[u8], rate: TrafficRate) -> Vec<u8> {
    let input_len = symbols.len();
    match rate {
        TrafficRate::Full | TrafficRate::Half => {
            debug_assert_eq!(input_len, MOD_SYMBOLS_PER_FRAME);
            symbols.to_vec()
        }
        TrafficRate::Quarter | TrafficRate::Eighth => {
            let n = MOD_SYMBOLS_PER_FRAME;
            let mut output = Vec::with_capacity(n);
            for k in 0..n {
                let input_idx = (k * input_len) / n;
                output.push(symbols[input_idx]);
            }
            output
        }
    }
}

impl ForwardTrafficChannelRc3 {
    pub fn new(config: ConfigRc3) -> Self {
        let pcb_scheduler = config.pcb_scheduler.clone();
        ForwardTrafficChannelRc3 {
            config: Mutex::new(config),
            tx_state: Mutex::new(TxState {
                symbol_buffer: VecDeque::new(),
                prepared_frame: None,
            }),
            frames: Mutex::new(VecDeque::new()),
            signaling_frames: Mutex::new(VecDeque::new()),
            queue_diag: Mutex::new(QueueDiagState {
                consecutive_nulls: 0,
                window_queued: 0,
                window_nulls: 0,
                total_queued: 0,
                total_nulls: 0,
                null_run_start: None,
                last_log_at: std::time::Instant::now(),
            }),
            last_enqueue_at: Mutex::new(None),
            pcb_scheduler,
        }
    }

    /// Queue a frame for transmission.
    pub fn send_frame(&self, frame: TrafficFrameRc3) {
        self.frames.lock().push_back(frame);
        *self.last_enqueue_at.lock() = Some(std::time::Instant::now());
    }

    /// Queue a signaling frame for transmission before any queued voice frames.
    pub fn send_signaling_bits(&self, bits: Vec<u8>) {
        self.signaling_frames.lock().push_back(TrafficFrameRc3 {
            data: bits,
            rate: TrafficRate::Full,
        });
        *self.last_enqueue_at.lock() = Some(std::time::Instant::now());
    }

    /// Returns the timestamp of the last frame enqueued, if any.
    pub fn last_enqueue_at(&self) -> Option<std::time::Instant> {
        *self.last_enqueue_at.lock()
    }

    /// Number of frames currently queued for transmission.
    pub fn queue_len(&self) -> usize {
        // Lock each mutex independently to avoid holding both simultaneously.
        // pop_next_frame() locks signaling_frames → frames; acquiring them in
        // the opposite order here would deadlock under contention.
        let sig_len = self.signaling_frames.lock().len();
        let data_len = self.frames.lock().len();
        data_len + sig_len
    }

    /// Queue a data fragment (DataRequest) for transmission at full rate.
    pub fn send_fragment(&self, fragment: DataRequest) {
        self.send_signaling_bits(fragment.data.bits().to_vec());
    }

    /// Schedule a single power-control bit for an absolute PCG index.
    /// Uses the struct-level `pcb_scheduler` — never touches `config`.
    pub fn schedule_power_control_bit(&self, abs_pcg: u64, bit: u8) {
        self.pcb_scheduler.lock().schedule(abs_pcg, bit);
    }

    /// Seed both long-code generators for this channel at the given CDMA
    /// system chip position and align the carry-in Q-lane scrambling chip.
    ///
    /// Both the scrambling LC and the PC-puncture LC are advanced to the
    /// same starting state. The odd / Q-lane carry-in `prev_frame_last_chip`
    /// is seeded from `LC[chip − 1]`.
    /// Uses delta from current position, safe to call multiple times.
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

    fn pop_next_frame(&self) -> (Vec<u8>, TrafficRate, bool) {
        if let Some(f) = self.signaling_frames.lock().pop_front() {
            log::debug!(
                "tx_ftch_rc3: signaling frame queued, rate={:?}, data_len={}, first_bytes={:?}",
                f.rate,
                f.data.len(),
                &f.data[..f.data.len().min(16)]
            );
            return (f.data, f.rate, true);
        }

        match self.frames.lock().pop_front() {
            Some(f) => {
                log::debug!(
                    "tx_ftch_rc3: traffic frame queued, rate={:?}, data_len={}, first_bytes={:?}",
                    f.rate,
                    f.data.len(),
                    &f.data[..f.data.len().min(16)]
                );
                (f.data, f.rate, true)
            }
            None => {
                // Null traffic MuxPDU: lowest negotiated rate with all bits = 1
                // per C.S0003-E Section 2.2.1.1.1.3.1.1.
                (
                    vec![1u8; TrafficRate::Eighth.info_bits()],
                    TrafficRate::Eighth,
                    false,
                )
            }
        }
    }

    fn note_frame_source(&self, is_queued: bool, rate: TrafficRate, frame_start_chip: u64) {
        let mut diag = self.queue_diag.lock();
        let now = std::time::Instant::now();

        if is_queued {
            diag.window_queued += 1;
            diag.total_queued += 1;
            if diag.consecutive_nulls > 0 {
                let dur = diag
                    .null_run_start
                    .map(|s| s.elapsed().as_millis())
                    .unwrap_or(0);
                log::info!(
                    "tx_ftch_rc3_queue: resumed after {} null frames ({}ms)",
                    diag.consecutive_nulls,
                    dur
                );
                diag.consecutive_nulls = 0;
                diag.null_run_start = None;
            }
        } else {
            diag.window_nulls += 1;
            diag.total_nulls += 1;
            diag.consecutive_nulls += 1;
            if diag.null_run_start.is_none() {
                diag.null_run_start = Some(now);
            }
        }

        let should_log = (!is_queued && diag.consecutive_nulls == 1)
            || diag.last_log_at.elapsed().as_secs() >= 1;
        if should_log {
            let queue_len = self.queue_len();
            let null_run_ms = diag
                .null_run_start
                .map(|s| s.elapsed().as_millis())
                .unwrap_or(0);
            let level = if is_queued {
                log::Level::Debug
            } else {
                log::Level::Warn
            };
            log::log!(
                level,
                "tx_ftch_rc3_queue: chip={} rate={:?} queued_window={} null_window={} total_queued={} total_null={} queue_len={} consecutive_nulls={} null_run_ms={}",
                frame_start_chip,
                rate,
                diag.window_queued,
                diag.window_nulls,
                diag.total_queued,
                diag.total_nulls,
                queue_len,
                diag.consecutive_nulls,
                null_run_ms
            );
            diag.window_queued = 0;
            diag.window_nulls = 0;
            diag.last_log_at = now;
        }
    }

    fn prepare_frame(
        &self,
        config: &mut ConfigRc3,
        data: Vec<u8>,
        rate: TrafficRate,
        is_queued: bool,
    ) -> PreparedFrameRc3 {
        // Step 1: Build complete frame (info + FQI + tail)
        let frame_data = build_frame_bits_rc3(&data, rate);
        let frame_start_chip = config.lc_chip_cursor;
        self.note_frame_source(is_queued, rate, frame_start_chip);
        let frame_bits = frame_data.len();

        // Step 2: Convolutional encode (R=1/4, K=9) — each bit → 4 symbols
        config.encoder.reset();
        let mut encoded = Vec::with_capacity(frame_bits * 4);
        for &bit in &frame_data {
            for &sym in config.encoder.encode(bit).iter() {
                encoded.push(sym);
            }
        }

        // Step 3: Symbol repetition (rate-dependent)
        let repeat_factor = rate.rc3_repeat_factor();
        let repeated = if repeat_factor > 1 {
            let mut rep = SymbolRepetition::new(repeat_factor);
            for &sym in &encoded {
                rep.feed(sym);
            }
            rep.take_all()
        } else {
            encoded
        };

        // Step 4: Puncture to 768 modulation symbols
        let punctured = puncture_rc3(&repeated, rate);
        assert_eq!(
            punctured.len(),
            MOD_SYMBOLS_PER_FRAME,
            "Expected {} symbols after puncture, got {} (rate={:?})",
            MOD_SYMBOLS_PER_FRAME,
            punctured.len(),
            rate
        );

        // Step 5: Forward-backwards bit-reversal interleave (768 symbols)
        let interleaved = config.interleaver.encode(&punctured);
        PreparedFrameRc3 {
            interleaved,
            rate,
            next_pcg: 0,
            frame_start_chip,
        }
    }

    fn emit_next_pcg(&self, config: &mut ConfigRc3, tx_state: &mut TxState) {
        if tx_state.prepared_frame.is_none() {
            let (data, rate, is_queued) = self.pop_next_frame();
            tx_state.prepared_frame = Some(self.prepare_frame(config, data, rate, is_queued));
        }

        let prepared = tx_state
            .prepared_frame
            .as_mut()
            .expect("prepared RC3 frame must exist");
        let pcg_index = prepared.next_pcg;
        let start = pcg_index * SYMBOLS_PER_PCG;
        let end = start + SYMBOLS_PER_PCG;

        let abs_pcg = config.lc_chip_cursor / PCG_CHIPS as u64;
        let pcb = self.pcb_scheduler.lock().read(abs_pcg);

        let mut pcg_bits = [0u8; SYMBOLS_PER_PCG];
        for bit in pcg_bits.iter_mut() {
            *bit = config.puncture_lc.next_chip();
            for _ in 0..(LC_DECIMATION - 1) {
                config.puncture_lc.next_chip();
            }
        }
        let b3 = pcg_bits[47] as usize;
        let b2 = pcg_bits[46] as usize;
        let b1 = pcg_bits[45] as usize;
        let b0 = pcg_bits[44] as usize;
        let pc_start = ((b3 << 3) | (b2 << 2) | (b1 << 1) | b0) * 2;

        let mut previous_chip = config.prev_frame_last_chip;
        let mut mapped = [0.0f32; SYMBOLS_PER_PCG];
        for (pair_idx, pair) in prepared.interleaved[start..end].chunks_exact(2).enumerate() {
            let q_chip = previous_chip;
            let i_chip = config.scrambling_lc.next_chip();
            previous_chip = i_chip;
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
                    sign * config.fpc_subchan_gain_linear
                } else if scrambled == 0 {
                    1.0f32
                } else {
                    -1.0f32
                };
            }
        }

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
                "tx_ftch_rc3_frame: start_chip={} end_chip={} rate={:?} mod_symbols={} qpsk_symbols={}",
                prepared.frame_start_chip,
                config.lc_chip_cursor,
                prepared.rate,
                MOD_SYMBOLS_PER_FRAME,
                OUTPUT_SYMBOLS_PER_FRAME
            );
            tx_state.prepared_frame = None;
        }
    }

    /// Produce one 20ms frame of 384 complex (QPSK) symbols.
    pub fn next(&self, current_system_time: CdmaSystemTime) -> Vec<Complex32> {
        self.next_block(OUTPUT_SYMBOLS_PER_FRAME, current_system_time)
    }
}

impl Channel for ForwardTrafficChannelRc3 {
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
    use crate::phy::coding::{
        block_interleaver::{ForwardBackwardsBitReversalInterleaver, SR1_PARAMS_768},
        convolutional::{get_1_4_k9_encoder, get_1_4_k9_soft_viterbi_decoder},
        long_code::LongCodeGenerator,
    };

    fn make_channel() -> ForwardTrafficChannelRc3 {
        ForwardTrafficChannelRc3::new(ConfigRc3 {
            encoder: get_1_4_k9_encoder(),
            interleaver: ForwardBackwardsBitReversalInterleaver::new(SR1_PARAMS_768),
            scrambling_lc: LongCodeGenerator::new_traffic_channel(0xDEADBEEF),
            puncture_lc: LongCodeGenerator::new_traffic_channel(0xDEADBEEF),
            lc_chip_cursor: 0,
            pcb_scheduler: crate::channels::PcgPcbScheduler::new(0),
            fpc_subchan_gain_linear: 1.0,
            prev_frame_last_chip: 0,
            disable_lc_scrambling: false,
        })
    }

    #[test]
    fn test_null_frame_produces_384_qpsk_symbols() {
        let ch = make_channel();
        let frame = ch.next(CdmaSystemTime::default());
        assert_eq!(frame.len(), OUTPUT_SYMBOLS_PER_FRAME);
    }

    #[test]
    fn test_full_rate_frame_produces_384_qpsk_symbols() {
        let ch = make_channel();
        ch.send_frame(TrafficFrameRc3 {
            data: vec![0; TrafficRate::Full.info_bits()],
            rate: TrafficRate::Full,
        });
        let frame = ch.next(CdmaSystemTime::default());
        assert_eq!(frame.len(), OUTPUT_SYMBOLS_PER_FRAME);
    }

    #[test]
    fn test_half_rate_frame_produces_384_qpsk_symbols() {
        let ch = make_channel();
        ch.send_frame(TrafficFrameRc3 {
            data: vec![0; TrafficRate::Half.info_bits()],
            rate: TrafficRate::Half,
        });
        let frame = ch.next(CdmaSystemTime::default());
        assert_eq!(frame.len(), OUTPUT_SYMBOLS_PER_FRAME);
    }

    #[test]
    fn test_quarter_rate_frame_produces_384_qpsk_symbols() {
        let ch = make_channel();
        ch.send_frame(TrafficFrameRc3 {
            data: vec![0; TrafficRate::Quarter.info_bits()],
            rate: TrafficRate::Quarter,
        });
        let frame = ch.next(CdmaSystemTime::default());
        assert_eq!(frame.len(), OUTPUT_SYMBOLS_PER_FRAME);
    }

    #[test]
    fn test_eighth_rate_frame_produces_384_qpsk_symbols() {
        let ch = make_channel();
        ch.send_frame(TrafficFrameRc3 {
            data: vec![0; TrafficRate::Eighth.info_bits()],
            rate: TrafficRate::Eighth,
        });
        let frame = ch.next(CdmaSystemTime::default());
        assert_eq!(frame.len(), OUTPUT_SYMBOLS_PER_FRAME);
    }

    #[test]
    fn test_qpsk_symbols_have_unit_components() {
        let ch = make_channel();
        let frame = ch.next(CdmaSystemTime::default());
        for (i, s) in frame.iter().enumerate() {
            assert!(
                (s.re.abs() - 1.0).abs() < 1e-6,
                "Symbol {} I component should be ±1.0, got {}",
                i,
                s.re
            );
            assert!(
                (s.im.abs() - 1.0).abs() < 1e-6,
                "Symbol {} Q component should be ±1.0, got {}",
                i,
                s.im
            );
        }
    }

    #[test]
    fn test_lc_chip_cursor_advances() {
        let ch = make_channel();
        let _frame = ch.next(CdmaSystemTime::default());
        let config = ch.config.lock();
        // Both LCs advance at the full 1.2288 Mcps chip rate — one 20 ms
        // frame = 768 mod symbols × 32 chips/mod sym = 24,576 chips.
        assert_eq!(
            config.lc_chip_cursor,
            (MOD_SYMBOLS_PER_FRAME * LC_DECIMATION) as u64
        );
        assert_eq!(config.lc_chip_cursor, 24_576);
    }

    #[test]
    fn test_different_data_produces_different_output() {
        let ch1 = make_channel();
        ch1.send_frame(TrafficFrameRc3 {
            data: vec![0; TrafficRate::Full.info_bits()],
            rate: TrafficRate::Full,
        });
        let frame1 = ch1.next(CdmaSystemTime::default());

        let ch2 = make_channel();
        ch2.send_frame(TrafficFrameRc3 {
            data: vec![1; TrafficRate::Full.info_bits()],
            rate: TrafficRate::Full,
        });
        let frame2 = ch2.next(CdmaSystemTime::default());

        assert_ne!(frame1, frame2);
    }

    #[test]
    fn test_build_frame_bits_rc3_full_rate() {
        let data = vec![0u8; 172];
        let frame = build_frame_bits_rc3(&data, TrafficRate::Full);
        assert_eq!(frame.len(), 192); // 172 info + 12 CRC + 8 tail
        // Last 8 bits should be tail zeros
        for &bit in &frame[184..192] {
            assert_eq!(bit, 0);
        }
    }

    #[test]
    fn test_build_frame_bits_rc3_quarter_rate() {
        let data = vec![0u8; 40];
        let frame = build_frame_bits_rc3(&data, TrafficRate::Quarter);
        assert_eq!(frame.len(), 54); // 40 info + 6 CRC + 8 tail
        // Last 8 bits should be tail zeros
        for &bit in &frame[46..54] {
            assert_eq!(bit, 0);
        }
    }

    #[test]
    fn test_build_frame_bits_rc3_eighth_rate() {
        let data = vec![0u8; 16];
        let frame = build_frame_bits_rc3(&data, TrafficRate::Eighth);
        assert_eq!(frame.len(), 30); // 16 info + 6 CRC + 8 tail
    }

    #[test]
    fn test_puncture_quarter_rate() {
        // 54 bits → 216 code symbols → 4× rep → 864 → 8/9 puncture → 768
        let symbols = vec![0u8; 864];
        let punctured = puncture_rc3(&symbols, TrafficRate::Quarter);
        assert_eq!(punctured.len(), 768);
    }

    #[test]
    fn test_puncture_eighth_rate() {
        // 30 bits → 120 code symbols → 8× rep → 960 → 4/5 puncture → 768
        let symbols = vec![0u8; 960];
        let punctured = puncture_rc3(&symbols, TrafficRate::Eighth);
        assert_eq!(punctured.len(), 768);
    }

    #[test]
    fn test_crc6_nonzero() {
        // Verify CRC-6 produces non-trivial output
        let data = vec![1u8; 40];
        let crc = crc6(&data);
        assert_ne!(crc, 0);
        assert!(crc < 64); // 6-bit value
    }

    #[test]
    fn test_full_rate_loopback_with_lc_advance() {
        let esn: u32 = 0xDEADBEEF;
        let lc_start_chip: u64 = 1_792_951_525_063_768;

        let frame_boundary = {
            let rem = lc_start_chip % 24_576;
            if rem == 0 {
                lc_start_chip
            } else {
                lc_start_chip + (24_576 - rem)
            }
        };

        let ch = ForwardTrafficChannelRc3::new(ConfigRc3 {
            encoder: get_1_4_k9_encoder(),
            interleaver: ForwardBackwardsBitReversalInterleaver::new(SR1_PARAMS_768),
            scrambling_lc: LongCodeGenerator::new_traffic_channel(esn),
            puncture_lc: LongCodeGenerator::new_traffic_channel(esn),
            lc_chip_cursor: 0,
            pcb_scheduler: crate::channels::PcgPcbScheduler::new(0),
            fpc_subchan_gain_linear: 1.0,
            prev_frame_last_chip: 0,
            disable_lc_scrambling: false,
        });

        ch.advance_lc_to_chip(frame_boundary);

        let null_frames = 3usize;
        for _ in 0..null_frames {
            let _ = ch.next(CdmaSystemTime::default());
        }

        let mut info_bits = vec![0u8; TrafficRate::Full.info_bits()];
        info_bits[0] = 1;
        info_bits[1] = 0;
        info_bits[2] = 1;
        info_bits[3] = 1;
        for i in 4..info_bits.len() {
            info_bits[i] = ((i * 11 + 5) % 2) as u8;
        }

        ch.send_frame(TrafficFrameRc3 {
            data: info_bits.clone(),
            rate: TrafficRate::Full,
        });
        let tx_symbols = ch.next(CdmaSystemTime::default());
        assert_eq!(tx_symbols.len(), OUTPUT_SYMBOLS_PER_FRAME);

        let mut soft_symbols = Vec::with_capacity(MOD_SYMBOLS_PER_FRAME);
        for symbol in &tx_symbols {
            soft_symbols.push((1.0 - symbol.re) * 0.5);
            soft_symbols.push((1.0 - symbol.im) * 0.5);
        }

        // Mirror the TX LC state at the start of the signaling frame.
        // Both LCs advance at the full 1.2288 Mcps chip rate, so after
        // `null_frames` frames they are at LC position
        //   frame_boundary + null_frames * 24_576.
        const CHIPS_PER_FRAME: u64 = (MOD_SYMBOLS_PER_FRAME * LC_DECIMATION) as u64;
        let lc_pos = frame_boundary + (null_frames as u64) * CHIPS_PER_FRAME;

        // Walk the scrambling LC exactly the same way the TX does under
        // the "odd/Q uses raw previous chip" interpretation.
        let mut scr_probe = LongCodeGenerator::new_traffic_channel(esn);
        let previous_chip_start = if lc_pos == 0 {
            LONG_CODE_PERIOD - 1
        } else {
            lc_pos - 1
        };
        scr_probe.advance_chips(previous_chip_start as usize);
        let mut held_prev = scr_probe.next_chip(); // LC[lc_pos - 1] — carry-in
        let mut i_chips = [0u8; OUTPUT_SYMBOLS_PER_FRAME];
        let mut q_chips = [0u8; OUTPUT_SYMBOLS_PER_FRAME];
        for k in 0..OUTPUT_SYMBOLS_PER_FRAME {
            let i = scr_probe.next_chip();
            i_chips[k] = i;
            q_chips[k] = held_prev;
            held_prev = i;
            for _ in 0..(2 * LC_DECIMATION - 1) {
                held_prev = scr_probe.next_chip();
            }
        }

        // Walk the puncture LC with the decimator pattern: one output per
        // mod symbol, skipping 31 chips between outputs.
        let mut pun_probe = LongCodeGenerator::new_traffic_channel(esn);
        pun_probe.advance_chips(lc_pos as usize);
        let mut pc_positions = [0usize; SR1_PCGS_PER_FRAME];
        for pcg in 0..SR1_PCGS_PER_FRAME {
            let mut pcg_bits = [0u8; SYMBOLS_PER_PCG];
            for bit in pcg_bits.iter_mut() {
                *bit = pun_probe.next_chip();
                for _ in 0..(LC_DECIMATION - 1) {
                    pun_probe.next_chip();
                }
            }
            let b3 = pcg_bits[47] as usize;
            let b2 = pcg_bits[46] as usize;
            let b1 = pcg_bits[45] as usize;
            let b0 = pcg_bits[44] as usize;
            pc_positions[pcg] = ((b3 << 3) | (b2 << 2) | (b1 << 1) | b0) * 2;
        }

        let descrambled = soft_symbols
            .into_iter()
            .enumerate()
            .map(|(idx, value)| {
                let pcg_index = idx / SYMBOLS_PER_PCG;
                let symbol_in_pcg = idx % SYMBOLS_PER_PCG;
                let pc_start = pc_positions[pcg_index];
                if symbol_in_pcg >= pc_start && symbol_in_pcg < pc_start + PC_PUNCTURE_SYMBOLS {
                    0.5
                } else {
                    let k = idx / 2;
                    let lc_scr = if idx % 2 == 0 { i_chips[k] } else { q_chips[k] };
                    if lc_scr == 0 { value } else { 1.0 - value }
                }
            })
            .collect::<Vec<_>>();

        let interleaver = ForwardBackwardsBitReversalInterleaver::new(SR1_PARAMS_768);
        let deinterleaved = interleaver.decode_soft(&descrambled);

        let peak = deinterleaved
            .iter()
            .map(|v| (0.5 - *v).abs())
            .fold(0.0f32, f32::max);
        let inv_peak = if peak > 1e-12 { 1.0 / peak } else { 1.0 };
        let mut viterbi = get_1_4_k9_soft_viterbi_decoder();
        let metrics: Vec<[f32; 4]> = deinterleaved
            .chunks_exact(4)
            .map(|chunk| {
                let to_metric = |value: f32| (value - 0.5) * inv_peak + 0.5;
                [
                    to_metric(chunk[0]),
                    to_metric(chunk[1]),
                    to_metric(chunk[2]),
                    to_metric(chunk[3]),
                ]
            })
            .collect();
        let decoded = viterbi.decode_block_from_state(&metrics, 0);

        assert_eq!(decoded.len(), 192, "expected 192 decoded bits");
        assert_eq!(
            &decoded[..172],
            &info_bits[..],
            "decoded info bits do not match original after RC3 loopback"
        );

        let computed_crc = crc12(&decoded[..172]);
        let mut received_crc: u16 = 0;
        for &bit in &decoded[172..184] {
            received_crc = (received_crc << 1) | bit as u16;
        }
        assert_eq!(
            computed_crc, received_crc,
            "RC3 CRC mismatch after loopback: computed=0x{:03X} received=0x{:03X}",
            computed_crc, received_crc
        );
        assert_eq!(&decoded[184..192], &[0u8; 8]);
    }

    #[test]
    fn test_rc3_full_rate_encode_chain_layered() {
        use crate::phy::coding::block_interleaver::ForwardBackwardsBitReversalInterleaver;
        use crate::phy::coding::convolutional::{
            get_1_4_k9_encoder, get_1_4_k9_soft_viterbi_decoder,
        };
        use crate::phy::coding::symbol_repeat::SymbolRepetition;

        // Use the actual BS Ack content from the bench log.
        let mut info_bits = vec![0u8; 172];
        // first_bytes=[1, 0, 1, 1, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0]
        let header: [u8; 16] = [1, 0, 1, 1, 1, 0, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0];
        info_bits[..16].copy_from_slice(&header);

        // ========== Layer 1: Frame building ==========
        let frame = build_frame_bits_rc3(&info_bits, TrafficRate::Full);
        assert_eq!(frame.len(), 192, "Layer 1: frame should be 192 bits");
        // Verify info bits preserved
        assert_eq!(&frame[..172], &info_bits[..], "Layer 1: info bits mismatch");
        // Verify tail bits are zeros
        assert_eq!(
            &frame[184..192],
            &[0u8; 8],
            "Layer 1: tail bits should be zero"
        );
        // Verify CRC-12
        let computed_crc = crc12(&frame[..172]);
        let mut frame_crc: u16 = 0;
        for &bit in &frame[172..184] {
            frame_crc = (frame_crc << 1) | bit as u16;
        }
        assert_eq!(
            computed_crc, frame_crc,
            "Layer 1: CRC-12 mismatch: computed=0x{:03X} frame=0x{:03X}",
            computed_crc, frame_crc
        );
        eprintln!(
            "Layer 1 PASS: frame=192 bits, CRC-12=0x{:03X}",
            computed_crc
        );

        // ========== Layer 2: R=1/4 K=9 encoder ==========
        let mut encoder = get_1_4_k9_encoder();
        encoder.reset();
        let mut encoded = Vec::with_capacity(768);
        for &bit in &frame {
            for &sym in encoder.encode(bit).iter() {
                encoded.push(sym);
            }
        }
        assert_eq!(encoded.len(), 768, "Layer 2: encoded should be 768 symbols");
        // Spot check: encoder initialized to all-zero, first bit=1 should produce
        // non-trivial output (not all zeros)
        let first_4 = &encoded[0..4];
        assert!(
            first_4.iter().any(|&s| s != 0),
            "Layer 2: first 4 code symbols should be non-trivial for input bit 1"
        );
        eprintln!(
            "Layer 2 PASS: 768 code symbols, first 8: {:?}",
            &encoded[..8]
        );

        // ========== Layer 3: Interleaver round-trip ==========
        let mut interleaver = ForwardBackwardsBitReversalInterleaver::new(SR1_PARAMS_768);
        let interleaved = interleaver.encode(&encoded);
        assert_eq!(interleaved.len(), 768, "Layer 3: interleaved length");
        // Verify it's a permutation (all symbols present)
        let mut sorted_enc = encoded.clone();
        sorted_enc.sort();
        let mut sorted_int = interleaved.clone();
        sorted_int.sort();
        assert_eq!(
            sorted_enc, sorted_int,
            "Layer 3: interleaver should be a permutation"
        );
        // Round-trip
        let deinterleaved = interleaver.decode(&interleaved);
        assert_eq!(
            deinterleaved, encoded,
            "Layer 3: decode(encode(x)) should equal x"
        );
        eprintln!("Layer 3 PASS: interleave/deinterleave round-trip OK");

        // ========== Layer 4: Full encode → decode WITHOUT scrambling ==========
        // Take the interleaved 768 symbols, skip scrambling, go directly to
        // soft Viterbi decode to verify the encode chain is invertible.
        let soft_interleaved: Vec<f32> = interleaved
            .iter()
            .map(|&b| if b == 0 { 0.0 } else { 1.0 })
            .collect();
        let deinterleaved_soft = interleaver.decode_soft(&soft_interleaved);

        let mut viterbi = get_1_4_k9_soft_viterbi_decoder();
        let metrics: Vec<[f32; 4]> = deinterleaved_soft
            .chunks_exact(4)
            .map(|chunk| [chunk[0], chunk[1], chunk[2], chunk[3]])
            .collect();
        let decoded = viterbi.decode_block_from_state(&metrics, 0);

        assert_eq!(decoded.len(), 192, "Layer 4: decoded should be 192 bits");
        assert_eq!(
            &decoded[..172],
            &info_bits[..],
            "Layer 4: decoded info bits mismatch (no scrambling)"
        );
        let decoded_crc = {
            let mut c: u16 = 0;
            for &bit in &decoded[172..184] {
                c = (c << 1) | bit as u16;
            }
            c
        };
        let recomputed_crc = crc12(&decoded[..172]);
        assert_eq!(
            recomputed_crc, decoded_crc,
            "Layer 4: CRC-12 after decode: computed=0x{:03X} decoded=0x{:03X}",
            recomputed_crc, decoded_crc
        );
        eprintln!("Layer 4 PASS: full encode→decode round-trip OK (no scrambling), CRC-12 valid");

        // ========== Layer 5: Verify eighth-rate for comparison ==========
        let eighth_info = vec![1u8; 16]; // null frame
        let eighth_frame = build_frame_bits_rc3(&eighth_info, TrafficRate::Eighth);
        assert_eq!(eighth_frame.len(), 30, "Layer 5: eighth frame = 30 bits");

        let mut enc8 = get_1_4_k9_encoder();
        enc8.reset();
        let mut encoded8 = Vec::new();
        for &bit in &eighth_frame {
            for &sym in enc8.encode(bit).iter() {
                encoded8.push(sym);
            }
        }
        assert_eq!(encoded8.len(), 120, "Layer 5: eighth encoded = 120 symbols");

        // 8× repetition
        let mut rep = SymbolRepetition::new(8);
        for &sym in &encoded8 {
            rep.feed(sym);
        }
        let repeated8 = rep.take_all();
        assert_eq!(repeated8.len(), 960, "Layer 5: after 8× rep = 960");

        // Puncture 4/5
        let punctured8 = puncture_rc3(&repeated8, TrafficRate::Eighth);
        assert_eq!(punctured8.len(), 768, "Layer 5: after puncture = 768");

        // Interleave
        let mut interleaver8 = ForwardBackwardsBitReversalInterleaver::new(SR1_PARAMS_768);
        let interleaved8 = interleaver8.encode(&punctured8);

        // Decode without scrambling
        let soft8: Vec<f32> = interleaved8
            .iter()
            .map(|&b| if b == 0 { 0.0 } else { 1.0 })
            .collect();
        let deint8 = interleaver8.decode_soft(&soft8);
        // Undo puncture: expand 768 → 960 by inserting erasures (0.5)
        let mut unpunctured8 = vec![0.5f32; 960];
        for k in 0..768usize {
            let input_idx = (k * 960) / 768;
            unpunctured8[input_idx] = deint8[k];
        }
        // Undo repetition: average each group of 8
        let mut derep8 = Vec::with_capacity(120);
        for chunk in unpunctured8.chunks_exact(8) {
            let avg: f32 = chunk.iter().sum::<f32>() / 8.0;
            derep8.push(avg);
        }

        let mut viterbi8 = get_1_4_k9_soft_viterbi_decoder();
        let metrics8: Vec<[f32; 4]> = derep8
            .chunks_exact(4)
            .map(|chunk| [chunk[0], chunk[1], chunk[2], chunk[3]])
            .collect();
        let decoded8 = viterbi8.decode_block_from_state(&metrics8, 0);
        assert_eq!(decoded8.len(), 30, "Layer 5: decoded eighth = 30 bits");
        assert_eq!(
            &decoded8[..16],
            &eighth_info[..],
            "Layer 5: eighth info bits mismatch"
        );
        eprintln!("Layer 5 PASS: eighth-rate encode→decode round-trip OK");

        eprintln!(
            "\n=== ALL LAYERS PASS — encode chain is correct for both full and eighth rate ==="
        );
    }

    /// Test that the actual `next()` output from ForwardTrafficChannelRc3 matches
    /// an independently computed reference at every stage. This exercises the
    /// REAL code path including scrambling, PC puncture, signal-point mapping,
    /// and I/Q demux — not just the encode chain in isolation.
    #[test]
    fn test_rc3_next_output_matches_independent_reference() {
        use crate::phy::coding::block_interleaver::ForwardBackwardsBitReversalInterleaver;
        use crate::phy::coding::convolutional::get_1_4_k9_encoder;
        use crate::phy::coding::long_code::LongCodeGenerator;

        let esn: u32 = 0xAABBCCDD;
        let start_chip: u64 = 49152; // two frame boundaries in

        // ---- Build channel and produce one full-rate frame via next() ----
        let ch = ForwardTrafficChannelRc3::new(ConfigRc3 {
            encoder: get_1_4_k9_encoder(),
            interleaver: ForwardBackwardsBitReversalInterleaver::new(SR1_PARAMS_768),
            scrambling_lc: LongCodeGenerator::new_traffic_channel(esn),
            puncture_lc: LongCodeGenerator::new_traffic_channel(esn),
            lc_chip_cursor: 0,
            pcb_scheduler: crate::channels::PcgPcbScheduler::new(0),
            fpc_subchan_gain_linear: 1.0, // unity gain so PC syms are ±1
            prev_frame_last_chip: 0,
            disable_lc_scrambling: false,
        });
        ch.advance_lc_to_chip(start_chip);

        let mut info_bits = vec![0u8; 172];
        info_bits[0] = 1;
        info_bits[1] = 0;
        info_bits[2] = 1;
        info_bits[3] = 1;
        for i in 4..172 {
            info_bits[i] = ((i * 13 + 7) % 2) as u8;
        }

        ch.send_frame(TrafficFrameRc3 {
            data: info_bits.clone(),
            rate: TrafficRate::Full,
        });
        let actual_output = ch.next(CdmaSystemTime::default());
        assert_eq!(actual_output.len(), OUTPUT_SYMBOLS_PER_FRAME);

        // ---- Independently compute the expected output ----

        // Step 1: Frame bits
        let frame = build_frame_bits_rc3(&info_bits, TrafficRate::Full);
        assert_eq!(frame.len(), 192);

        // Step 2: Encode
        let mut encoder = get_1_4_k9_encoder();
        encoder.reset();
        let mut encoded = Vec::with_capacity(768);
        for &bit in &frame {
            for &sym in encoder.encode(bit).iter() {
                encoded.push(sym);
            }
        }
        assert_eq!(encoded.len(), 768);

        // Step 3: No repetition at full rate
        // Step 4: No puncture at full rate

        // Step 5: Interleave
        let mut interleaver = ForwardBackwardsBitReversalInterleaver::new(SR1_PARAMS_768);
        let interleaved = interleaver.encode(&encoded);
        assert_eq!(interleaved.len(), 768);

        // Step 6: Scrambling — generate the same pair-based LC chips the
        // channel used. Both scrambling_lc and puncture_lc were seeded to
        // start_chip.
        let mut scr_lc = LongCodeGenerator::new_traffic_channel(esn);
        scr_lc.advance_chips(start_chip as usize);
        let previous_chip_start = if start_chip == 0 {
            LONG_CODE_PERIOD - 1
        } else {
            start_chip - 1
        };
        let mut scr_prev_probe = LongCodeGenerator::new_traffic_channel(esn);
        scr_prev_probe.advance_chips(previous_chip_start as usize);
        let mut previous_chip = scr_prev_probe.next_chip();
        let mut pair_start_chips = [0u8; OUTPUT_SYMBOLS_PER_FRAME];
        let mut pair_previous_chips = [0u8; OUTPUT_SYMBOLS_PER_FRAME];
        for pair_idx in 0..OUTPUT_SYMBOLS_PER_FRAME {
            pair_previous_chips[pair_idx] = previous_chip;
            let start_chip = scr_lc.next_chip();
            pair_start_chips[pair_idx] = start_chip;
            previous_chip = start_chip;
            for _ in 0..((2 * LC_DECIMATION) - 1) {
                previous_chip = scr_lc.next_chip();
            }
        }

        // PC positions from puncture LC (same seed, same walk)
        let mut pun_lc = LongCodeGenerator::new_traffic_channel(esn);
        pun_lc.advance_chips(start_chip as usize);
        let mut pc_positions = [0usize; SR1_PCGS_PER_FRAME];
        for pcg in 0..SR1_PCGS_PER_FRAME {
            let mut pcg_bits = [0u8; SYMBOLS_PER_PCG];
            for bit in pcg_bits.iter_mut() {
                *bit = pun_lc.next_chip();
                for _ in 0..(LC_DECIMATION - 1) {
                    pun_lc.next_chip();
                }
            }
            let b3 = pcg_bits[47] as usize;
            let b2 = pcg_bits[46] as usize;
            let b1 = pcg_bits[45] as usize;
            let b0 = pcg_bits[44] as usize;
            pc_positions[pcg] = ((b3 << 3) | (b2 << 2) | (b1 << 1) | b0) * 2;
        }

        // The channel does not schedule explicit PCB values in this test, so
        // the shared scheduler falls back to constant UP (0) on every PCG.
        let pc_bits: [u8; 16] = [0; 16];

        // Step 7: Scramble + PC puncture + signal-point map
        let mut expected_mapped = Vec::with_capacity(MOD_SYMBOLS_PER_FRAME);
        for (idx, &sym) in interleaved.iter().enumerate() {
            let pair_idx = idx / 2;
            let lc_scr = if idx % 2 == 0 {
                pair_start_chips[pair_idx]
            } else {
                pair_previous_chips[pair_idx]
            };
            let scrambled = sym ^ lc_scr;

            let pcg_index = idx / SYMBOLS_PER_PCG;
            let symbol_in_pcg = idx % SYMBOLS_PER_PCG;
            let pc_start = pc_positions[pcg_index];
            let is_pc = symbol_in_pcg >= pc_start && symbol_in_pcg < pc_start + PC_PUNCTURE_SYMBOLS;

            if is_pc {
                let sign = if pc_bits[pcg_index] == 0 {
                    1.0f32
                } else {
                    -1.0f32
                };
                expected_mapped.push(sign * 1.0); // unity gain
            } else {
                expected_mapped.push(if scrambled == 0 { 1.0f32 } else { -1.0f32 });
            }
        }

        // Step 8: I/Q demux
        let mut expected_output = Vec::with_capacity(OUTPUT_SYMBOLS_PER_FRAME);
        for pair in expected_mapped.chunks_exact(2) {
            expected_output.push(Complex32::new(pair[0], pair[1]));
        }

        // ---- Compare ----
        let mut mismatches = 0;
        for (i, (actual, expected)) in actual_output.iter().zip(expected_output.iter()).enumerate()
        {
            if (actual.re - expected.re).abs() > 1e-6 || (actual.im - expected.im).abs() > 1e-6 {
                if mismatches < 5 {
                    eprintln!(
                        "MISMATCH at QPSK sym {}: actual=({:.3}, {:.3}) expected=({:.3}, {:.3})",
                        i, actual.re, actual.im, expected.re, expected.im
                    );
                }
                mismatches += 1;
            }
        }

        eprintln!(
            "Compared {} QPSK symbols: {} mismatches",
            OUTPUT_SYMBOLS_PER_FRAME, mismatches
        );
        assert_eq!(
            mismatches, 0,
            "next() output does not match independent reference ({} mismatches out of {})",
            mismatches, OUTPUT_SYMBOLS_PER_FRAME
        );
        eprintln!("PASS: next() output matches independent reference exactly");

        // ---- Verify the output is decodable by reversing the chain ----
        // Undo I/Q demux
        let mut received_mapped: Vec<f32> = Vec::with_capacity(MOD_SYMBOLS_PER_FRAME);
        for sym in &actual_output {
            received_mapped.push(sym.re);
            received_mapped.push(sym.im);
        }

        // Undo scrambling + mark PC as erasures
        let mut descrambled = Vec::with_capacity(MOD_SYMBOLS_PER_FRAME);
        for (idx, &val) in received_mapped.iter().enumerate() {
            let pcg_index = idx / SYMBOLS_PER_PCG;
            let symbol_in_pcg = idx % SYMBOLS_PER_PCG;
            let pc_start = pc_positions[pcg_index];
            let is_pc = symbol_in_pcg >= pc_start && symbol_in_pcg < pc_start + PC_PUNCTURE_SYMBOLS;

            if is_pc {
                descrambled.push(0.5); // erasure
            } else {
                // Map ±1 back to soft 0/1, then XOR (flip if lc=1)
                let soft = (1.0 - val) * 0.5; // +1 → 0.0, -1 → 1.0
                let pair_idx = idx / 2;
                let lc = if idx % 2 == 0 {
                    pair_start_chips[pair_idx]
                } else {
                    pair_previous_chips[pair_idx]
                };
                descrambled.push(if lc == 0 { soft } else { 1.0 - soft });
            }
        }

        // Deinterleave
        let deinterleaved = interleaver.decode_soft(&descrambled);

        // Viterbi decode
        let mut viterbi = get_1_4_k9_soft_viterbi_decoder();
        let metrics: Vec<[f32; 4]> = deinterleaved
            .chunks_exact(4)
            .map(|chunk| [chunk[0], chunk[1], chunk[2], chunk[3]])
            .collect();
        let decoded = viterbi.decode_block_from_state(&metrics, 0);

        assert_eq!(decoded.len(), 192);
        assert_eq!(
            &decoded[..172],
            &info_bits[..],
            "Round-trip decode failed: info bits mismatch"
        );
        let computed_crc = crc12(&decoded[..172]);
        let mut received_crc: u16 = 0;
        for &bit in &decoded[172..184] {
            received_crc = (received_crc << 1) | bit as u16;
        }
        assert_eq!(
            computed_crc, received_crc,
            "Round-trip CRC mismatch: computed=0x{:03X} received=0x{:03X}",
            computed_crc, received_crc
        );
        eprintln!("PASS: full round-trip decode with scrambling + PC puncture, CRC-12 valid");
    }
}
