use parking_lot::Mutex;
use std::collections::VecDeque;

use cdma_common::crc::{crc8, crc12};
use cdma_common::time::CdmaSystemTime;
use log::trace;
use num::complex::Complex32;

use crate::{
    mac::types::DataRequest,
    phy::coding::{
        block_interleaver::BitReversalInterleaver, convolutional::Encoder,
        long_code::LongCodeGenerator, symbol_repeat::SymbolRepetition,
    },
};

use super::{Channel, PcgPcbSchedulerHandle};
pub use cdma_common::channel::TrafficRate;
use cdma_common::consts::SR1_PCGS_PER_FRAME;

/// A frame to be transmitted on the forward traffic channel.
pub struct TrafficFrame {
    /// The information bits to transmit (before CRC and tail).
    /// CRC and tail bits are computed automatically during encoding.
    pub data: Vec<u8>,
    /// The rate for this frame.
    pub rate: TrafficRate,
}

/// Build the complete encoder input (info + CRC + tail) for a traffic frame.
fn build_frame_bits(data: &[u8], rate: TrafficRate) -> Vec<u8> {
    let info_bits = rate.info_bits();
    let fqi_bits = rate.fqi_bits();
    let tail_bits = 8usize;
    let total = info_bits + fqi_bits + tail_bits;
    debug_assert_eq!(total, rate.frame_bits());

    let mut frame = Vec::with_capacity(total);

    // Info bits (zero-pad if data is shorter)
    for i in 0..info_bits {
        frame.push(if i < data.len() { data[i] } else { 0 });
    }

    // CRC (FQI) bits
    if fqi_bits == 12 {
        let crc = crc12(&frame[..info_bits]);
        for bit in (0..12).rev() {
            frame.push(((crc >> bit) & 1) as u8);
        }
    } else if fqi_bits == 8 {
        let crc = crc8(&frame[..info_bits]);
        for bit in (0..8).rev() {
            frame.push(((crc >> bit) & 1) as u8);
        }
    }

    // Tail bits (8 zeros)
    for _ in 0..tail_bits {
        frame.push(0);
    }

    frame
}

/// Forward traffic channel output symbols per 20ms frame (all rates produce this).
const SYMBOLS_PER_FRAME: usize = 384;

/// Symbols per PCG.
const SYMBOLS_PER_PCG: usize = SYMBOLS_PER_FRAME / SR1_PCGS_PER_FRAME; // = 24

/// Chips per RC1 power-control group.
const PCG_CHIPS: usize = SYMBOLS_PER_PCG * 64; // = 1536

struct PreparedFrame {
    interleaved: Vec<u8>,
    rate: TrafficRate,
    next_pcg: usize,
    frame_start_chip: u64,
    /// False for inline null frames.
    is_queued: bool,
}

struct TxState {
    symbol_buffer: VecDeque<Complex32>,
    prepared_frame: Option<PreparedFrame>,
}

pub struct Config<const EK: usize, const ER: usize> {
    pub encoder: Encoder<EK, ER>,
    pub interleaver: BitReversalInterleaver,
    pub long_code_generator: LongCodeGenerator,
    /// Absolute chip cursor (since CDMA epoch) corresponding to the current
    /// long-code generator state.
    pub lc_chip_cursor: u64,
    /// Shared scheduler for absolute-PCG power control bits.
    pub pcb_scheduler: PcgPcbSchedulerHandle,
}

/// Caller-side frame prep state.
struct PrepEngine<const EK: usize, const ER: usize> {
    encoder: Encoder<EK, ER>,
    interleaver: BitReversalInterleaver,
}

pub struct ForwardTrafficChannel<const EK: usize, const ER: usize> {
    config: Mutex<Config<EK, ER>>,
    tx_state: Mutex<TxState>,
    prep: Mutex<PrepEngine<EK, ER>>,
    frames: Mutex<VecDeque<PreparedFrame>>,
    signaling_frames: Mutex<VecDeque<PreparedFrame>>,
    /// Tracks consecutive null frames for rate-limited logging.
    null_frame_state: Mutex<NullFrameState>,
    /// Timestamp of the last frame enqueued via `send_frame`.
    /// Used by the BSC to determine whether the channel is actively in use.
    last_enqueue_at: Mutex<Option<std::time::Instant>>,
}

/// Tracks null frame runs to avoid spamming logs every 20ms.
struct NullFrameState {
    /// Number of consecutive null frames sent.
    consecutive_nulls: u64,
    /// When the current null run started.
    null_run_start: Option<std::time::Instant>,
    /// Last time we logged a null-frame warning.
    last_log_at: Option<std::time::Instant>,
}

impl<const EK: usize, const ER: usize> ForwardTrafficChannel<EK, ER> {
    pub fn new(config: Config<EK, ER>) -> Self {
        let prep = PrepEngine {
            encoder: config.encoder,
            interleaver: config.interleaver.clone(),
        };
        ForwardTrafficChannel {
            config: Mutex::new(config),
            tx_state: Mutex::new(TxState {
                symbol_buffer: VecDeque::new(),
                prepared_frame: None,
            }),
            prep: Mutex::new(prep),
            frames: Mutex::new(VecDeque::new()),
            signaling_frames: Mutex::new(VecDeque::new()),
            null_frame_state: Mutex::new(NullFrameState {
                consecutive_nulls: 0,
                null_run_start: None,
                last_log_at: None,
            }),
            last_enqueue_at: Mutex::new(None),
        }
    }

    /// Queue a traffic frame for transmission.
    pub fn send_frame(&self, frame: TrafficFrame) {
        let prepared = {
            let mut prep = self.prep.lock();
            Self::prepare_frame_static(&mut prep, &frame.data, frame.rate, true)
        };
        self.frames.lock().push_back(prepared);
        *self.last_enqueue_at.lock() = Some(std::time::Instant::now());
    }

    /// Queue a signaling frame for transmission before any queued voice frames.
    pub fn send_signaling_bits(&self, bits: Vec<u8>) {
        let prepared = {
            let mut prep = self.prep.lock();
            Self::prepare_frame_static(&mut prep, &bits, TrafficRate::Full, true)
        };
        self.signaling_frames.lock().push_back(prepared);
        *self.last_enqueue_at.lock() = Some(std::time::Instant::now());
    }

    fn prepare_frame_static(
        prep: &mut PrepEngine<EK, ER>,
        data: &[u8],
        rate: TrafficRate,
        is_queued: bool,
    ) -> PreparedFrame {
        let frame_data = build_frame_bits(data, rate);
        let repeat_factor = rate.repeat_factor();

        prep.encoder.reset();
        let mut encoded = Vec::with_capacity(frame_data.len() * ER);
        for &bit in &frame_data {
            for &sym in prep.encoder.encode(bit).iter() {
                encoded.push(sym);
            }
        }

        let repeated = if repeat_factor > 1 {
            let mut rep = SymbolRepetition::new(repeat_factor);
            for &sym in &encoded {
                rep.feed(sym);
            }
            rep.take_all()
        } else {
            encoded
        };

        assert_eq!(repeated.len(), SYMBOLS_PER_FRAME);
        let interleaved = prep.interleaver.encode(&repeated);

        PreparedFrame {
            interleaved,
            rate,
            next_pcg: 0,
            frame_start_chip: 0,
            is_queued,
        }
    }

    /// Returns the timestamp of the last frame enqueued, if any.
    pub fn last_enqueue_at(&self) -> Option<std::time::Instant> {
        *self.last_enqueue_at.lock()
    }

    /// Number of frames currently queued for transmission.
    pub fn queue_len(&self) -> usize {
        let sig_len = self.signaling_frames.lock().len();
        let data_len = self.frames.lock().len();
        data_len + sig_len
    }

    /// Queue a data fragment (DataRequest) for transmission at full rate.
    /// This provides a paging-channel-compatible interface for signaling.
    pub fn send_fragment(&self, fragment: DataRequest) {
        self.send_signaling_bits(fragment.data.bits().to_vec());
    }

    /// Schedule a single power-control bit for an absolute PCG index.
    pub fn schedule_power_control_bit(&self, abs_pcg: u64, bit: u8) {
        let scheduler = {
            let config = self.config.lock();
            config.pcb_scheduler.clone()
        };
        scheduler.lock().schedule(abs_pcg, bit);
    }

    /// Advance the internal long code generator to the given absolute chip
    /// position. Uses delta from current position, safe to call multiple times.
    pub fn advance_lc_to_chip(&self, chip: u64) {
        let mut config = self.config.lock();
        let delta = chip.saturating_sub(config.lc_chip_cursor);
        config.long_code_generator.advance_chips(delta as usize);
        config.lc_chip_cursor = chip;
    }

    fn pop_next_frame(&self) -> Option<PreparedFrame> {
        // Traffic signaling has priority over queued voice payloads.
        self.signaling_frames
            .lock()
            .pop_front()
            .or_else(|| self.frames.lock().pop_front())
    }

    fn note_frame_source(&self, is_queued: bool, rate: TrafficRate, frame_start_chip: u64) {
        let frame_end_chip = frame_start_chip.saturating_add(SYMBOLS_PER_FRAME as u64 * 64);
        if is_queued {
            log::debug!(
                "tx_ftch_frame: QUEUED start_chip={} end_chip={} rate={:?} symbols={}",
                frame_start_chip,
                frame_end_chip,
                rate,
                SYMBOLS_PER_FRAME
            );
            let mut ns = self.null_frame_state.lock();
            if ns.consecutive_nulls > 0 {
                let dur = ns
                    .null_run_start
                    .map(|s| s.elapsed().as_millis())
                    .unwrap_or(0);
                log::info!(
                    "tx_ftch_frame: queue resumed after {} null frames ({}ms)",
                    ns.consecutive_nulls,
                    dur
                );
                ns.consecutive_nulls = 0;
                ns.null_run_start = None;
                ns.last_log_at = None;
            }
            return;
        }

        let mut ns = self.null_frame_state.lock();
        ns.consecutive_nulls += 1;
        let now = std::time::Instant::now();
        if ns.null_run_start.is_none() {
            ns.null_run_start = Some(now);
        }
        let should_log = ns.consecutive_nulls == 1
            || ns.last_log_at.map_or(true, |t| t.elapsed().as_secs() >= 1);
        if should_log {
            let dur = ns
                .null_run_start
                .map(|s| s.elapsed().as_millis())
                .unwrap_or(0);
            log::warn!(
                "tx_ftch_frame: queue empty — {} consecutive null frames ({}ms)",
                ns.consecutive_nulls,
                dur
            );
            ns.last_log_at = Some(now);
        }
    }

    /// Build an inline null traffic MuxPDU.
    fn build_null_frame(config: &mut Config<EK, ER>) -> PreparedFrame {
        let data = vec![1u8; TrafficRate::Eighth.info_bits()];
        let rate = TrafficRate::Eighth;
        let frame_data = build_frame_bits(&data, rate);

        config.encoder.reset();
        let mut encoded = Vec::with_capacity(frame_data.len() * ER);
        for &bit in &frame_data {
            for &sym in config.encoder.encode(bit).iter() {
                encoded.push(sym);
            }
        }

        let mut rep = SymbolRepetition::new(rate.repeat_factor());
        for &sym in &encoded {
            rep.feed(sym);
        }
        let repeated = rep.take_all();
        assert_eq!(repeated.len(), SYMBOLS_PER_FRAME);
        let interleaved = config.interleaver.encode(&repeated);

        PreparedFrame {
            interleaved,
            rate,
            next_pcg: 0,
            frame_start_chip: 0,
            is_queued: false,
        }
    }

    fn emit_next_pcg(&self, config: &mut Config<EK, ER>, tx_state: &mut TxState) {
        if tx_state.prepared_frame.is_none() {
            let mut prepared = self
                .pop_next_frame()
                .unwrap_or_else(|| Self::build_null_frame(config));
            prepared.frame_start_chip = config.lc_chip_cursor;
            self.note_frame_source(prepared.is_queued, prepared.rate, prepared.frame_start_chip);
            tx_state.prepared_frame = Some(prepared);
        }

        let prepared = tx_state
            .prepared_frame
            .as_mut()
            .expect("prepared frame must exist");
        let pcg_index = prepared.next_pcg;
        let start = pcg_index * SYMBOLS_PER_PCG;
        let end = start + SYMBOLS_PER_PCG;

        let abs_pcg = config.lc_chip_cursor / PCG_CHIPS as u64;
        let pcb = config.pcb_scheduler.lock().read(abs_pcg);

        let mut lc_decimated = [0u8; SYMBOLS_PER_PCG];
        for bit in &mut lc_decimated {
            *bit = config.long_code_generator.next_chip();
            for _ in 1..64 {
                config.long_code_generator.next_chip();
            }
        }

        let b3 = lc_decimated[23] as usize;
        let b2 = lc_decimated[22] as usize;
        let b1 = lc_decimated[21] as usize;
        let b0 = lc_decimated[20] as usize;
        let pc_start = (b3 << 3) | (b2 << 2) | (b1 << 1) | b0;

        for (symbol_in_pcg, &sym) in prepared.interleaved[start..end].iter().enumerate() {
            let scrambled = sym ^ lc_decimated[symbol_in_pcg];
            let output_bit = if symbol_in_pcg == pc_start || symbol_in_pcg == pc_start + 1 {
                pcb
            } else {
                scrambled
            };

            tx_state.symbol_buffer.push_back(Complex32::new(
                if output_bit == 0 { 1.0 } else { -1.0 },
                0.0,
            ));
        }

        config.lc_chip_cursor = config.lc_chip_cursor.saturating_add(PCG_CHIPS as u64);
        prepared.next_pcg += 1;
        if prepared.next_pcg == SR1_PCGS_PER_FRAME {
            trace!(
                "tx_ftch_frame_done: start_chip={} end_chip={} rate={:?}",
                prepared.frame_start_chip, config.lc_chip_cursor, prepared.rate
            );
            tx_state.prepared_frame = None;
        }
    }

    /// Produce one 20ms frame of 384 BPSK symbols.
    pub fn next(&self, current_system_time: CdmaSystemTime) -> Vec<Complex32> {
        self.next_block(SYMBOLS_PER_FRAME, current_system_time)
    }
}

impl<const EK: usize, const ER: usize> Channel for ForwardTrafficChannel<EK, ER> {
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
        while tx_state.symbol_buffer.len() < num_samples {
            self.emit_next_pcg(&mut config, &mut tx_state);
        }
        out.reserve(num_samples);
        for _ in 0..num_samples {
            out.push(tx_state.symbol_buffer.pop_front().unwrap());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phy::coding::{
        block_interleaver::{BitReversalInterleaver, SR1_PARAMS_384},
        convolutional::get_1_2_k9_encoder,
        long_code::LongCodeGenerator,
    };

    fn make_channel() -> ForwardTrafficChannel<9, 2> {
        ForwardTrafficChannel::new(Config {
            encoder: get_1_2_k9_encoder(),
            interleaver: BitReversalInterleaver::new(SR1_PARAMS_384),
            long_code_generator: LongCodeGenerator::new_traffic_channel(0xDEADBEEF),
            lc_chip_cursor: 0,
            pcb_scheduler: crate::channels::PcgPcbScheduler::new(0),
        })
    }

    #[test]
    fn test_null_frame_produces_384_symbols() {
        let ch = make_channel();
        let frame = ch.next(CdmaSystemTime::default());
        assert_eq!(frame.len(), SYMBOLS_PER_FRAME);
    }

    #[test]
    fn test_full_rate_frame_produces_384_symbols() {
        let ch = make_channel();
        ch.send_frame(TrafficFrame {
            data: vec![0; TrafficRate::Full.frame_bits()],
            rate: TrafficRate::Full,
        });
        let frame = ch.next(CdmaSystemTime::default());
        assert_eq!(frame.len(), SYMBOLS_PER_FRAME);
    }

    #[test]
    fn test_half_rate_frame_produces_384_symbols() {
        let ch = make_channel();
        ch.send_frame(TrafficFrame {
            data: vec![0; TrafficRate::Half.frame_bits()],
            rate: TrafficRate::Half,
        });
        let frame = ch.next(CdmaSystemTime::default());
        assert_eq!(frame.len(), SYMBOLS_PER_FRAME);
    }

    #[test]
    fn test_quarter_rate_frame_produces_384_symbols() {
        let ch = make_channel();
        ch.send_frame(TrafficFrame {
            data: vec![0; TrafficRate::Quarter.frame_bits()],
            rate: TrafficRate::Quarter,
        });
        let frame = ch.next(CdmaSystemTime::default());
        assert_eq!(frame.len(), SYMBOLS_PER_FRAME);
    }

    #[test]
    fn test_eighth_rate_frame_produces_384_symbols() {
        let ch = make_channel();
        ch.send_frame(TrafficFrame {
            data: vec![0; TrafficRate::Eighth.frame_bits()],
            rate: TrafficRate::Eighth,
        });
        let frame = ch.next(CdmaSystemTime::default());
        assert_eq!(frame.len(), SYMBOLS_PER_FRAME);
    }

    #[test]
    fn test_different_data_produces_different_output() {
        let ch1 = make_channel();
        ch1.send_frame(TrafficFrame {
            data: vec![0; TrafficRate::Full.frame_bits()],
            rate: TrafficRate::Full,
        });
        let frame1 = ch1.next(CdmaSystemTime::default());

        let ch2 = make_channel();
        ch2.send_frame(TrafficFrame {
            data: vec![1; TrafficRate::Full.frame_bits()],
            rate: TrafficRate::Full,
        });
        let frame2 = ch2.next(CdmaSystemTime::default());

        assert_ne!(frame1, frame2);
    }

    #[test]
    fn test_lc_chip_cursor_advances() {
        let ch = make_channel();
        let frame = ch.next(CdmaSystemTime::default());
        let config = ch.config.lock();
        // 384 symbols × 64 chips = 24,576 chips = one 20ms frame
        assert_eq!(config.lc_chip_cursor, (frame.len() as u64) * 64);
    }

    #[test]
    fn test_full_rate_loopback_decode() {
        // Forward link loopback: encode a known full-rate frame through the
        // ForwardTrafficChannel, then manually reverse all processing steps
        // and verify we recover the original info bits + valid CRC-12.
        use crate::phy::coding::convolutional::ViterbiDecoder;

        let esn: u32 = 0xDEADBEEF;
        let ch = ForwardTrafficChannel::new(Config {
            encoder: get_1_2_k9_encoder(),
            interleaver: BitReversalInterleaver::new(SR1_PARAMS_384),
            long_code_generator: LongCodeGenerator::new_traffic_channel(esn),
            lc_chip_cursor: 0,
            pcb_scheduler: crate::channels::PcgPcbScheduler::new(0),
        });

        // Build a recognizable 172-bit payload (MuxPDU header + signaling)
        let mut info_bits = vec![0u8; 172];
        // MuxPDU header: MM=1, TT=0, TM=11 (blank-and-burst)
        info_bits[0] = 1;
        info_bits[1] = 0;
        info_bits[2] = 1;
        info_bits[3] = 1;
        // Fill signaling with pattern
        for i in 4..172 {
            info_bits[i] = ((i * 7 + 3) % 2) as u8;
        }

        ch.send_frame(TrafficFrame {
            data: info_bits.clone(),
            rate: TrafficRate::Full,
        });
        let tx_symbols = ch.next(CdmaSystemTime::default());
        assert_eq!(tx_symbols.len(), 384);

        // === MS-side decode ===

        // 1. BPSK de-map: +1.0 → 0, -1.0 → 1
        let mut hard_bits: Vec<u8> = tx_symbols
            .iter()
            .map(|s| if s.re > 0.0 { 0u8 } else { 1u8 })
            .collect();

        // 2. Identify PC-punctured positions using same LC, then replace with
        //    de-scrambled estimates (we'll just descramble everything including
        //    punctured positions — since PC bits are 0 → +1.0 → hard 0, after
        //    descramble they become lc_bit XOR 0 = lc_bit, which is random.
        //    We'll mark them as erasures after descramble.)
        let mut lc = LongCodeGenerator::new_traffic_channel(esn);
        let mut lc_decimated = [0u8; 384];
        for i in 0..384 {
            lc_decimated[i] = lc.next_chip();
            for _ in 1..64 {
                lc.next_chip();
            }
        }

        // Compute PC positions (same as encoder)
        let mut pc_positions = [0usize; 16];
        for pcg in 0..16 {
            let base = pcg * 24;
            let b3 = lc_decimated[base + 23] as usize;
            let b2 = lc_decimated[base + 22] as usize;
            let b1 = lc_decimated[base + 21] as usize;
            let b0 = lc_decimated[base + 20] as usize;
            pc_positions[pcg] = (b3 << 3) | (b2 << 2) | (b1 << 1) | b0;
        }

        // 3. De-scramble: XOR with decimated LC
        for i in 0..384 {
            hard_bits[i] ^= lc_decimated[i];
        }

        // 4. Replace PC-punctured symbols with erasure (0 = "don't know")
        //    For hard Viterbi, erasures don't exist, but we can set to 0 (the
        //    "average" symbol). The R=1/2 K=9 code can handle 32/384 = 8.3% errors.
        // (skipping explicit erasure — the Viterbi should still decode)

        // 5. De-interleave
        let mut interleaver = BitReversalInterleaver::new(SR1_PARAMS_384);
        let deinterleaved = interleaver.decode(&hard_bits);

        // 6. Viterbi decode R=1/2 K=9
        let mut decoder = ViterbiDecoder::new(get_1_2_k9_encoder());
        let symbol_pairs: Vec<[u8; 2]> = deinterleaved
            .chunks_exact(2)
            .map(|c| [c[0], c[1]])
            .collect();
        let decoded = decoder.decode_block_from_state(&symbol_pairs, 0);
        assert_eq!(decoded.len(), 192, "expected 192 decoded bits");

        // 7. Verify info bits match
        assert_eq!(
            &decoded[..172],
            &info_bits[..],
            "decoded info bits don't match original"
        );

        // 8. Verify CRC-12
        let computed_crc = crc12(&decoded[..172]);
        let mut received_crc: u16 = 0;
        for &bit in &decoded[172..184] {
            received_crc = (received_crc << 1) | (bit as u16 & 1);
        }
        assert_eq!(
            computed_crc, received_crc,
            "CRC-12 mismatch: computed=0x{:03X} received=0x{:03X}",
            computed_crc, received_crc
        );

        // 9. Verify tail bits
        assert_eq!(&decoded[184..192], &[0u8; 8], "tail bits should be zero");

        eprintln!("forward link loopback decode PASSED");
    }

    /// Multi-frame loopback test: advance the LC to a realistic chip position,
    /// generate several null frames, then queue and decode a full-rate signaling
    /// frame. This verifies the LC state is correct after multi-frame operation.
    #[test]
    fn test_multi_frame_loopback_with_lc_advance() {
        use crate::phy::coding::convolutional::ViterbiDecoder;

        let esn: u32 = 0xDEADBEEF;
        let lc_start_chip: u64 = 1_792_135_971_780_949; // realistic chip position

        // Snap to frame boundary (multiple of 24576)
        let frame_boundary = {
            let rem = lc_start_chip % 24576;
            if rem == 0 {
                lc_start_chip
            } else {
                lc_start_chip + (24576 - rem)
            }
        };

        let ch = ForwardTrafficChannel::new(Config {
            encoder: get_1_2_k9_encoder(),
            interleaver: BitReversalInterleaver::new(SR1_PARAMS_384),
            long_code_generator: LongCodeGenerator::new_traffic_channel(esn),
            lc_chip_cursor: 0,
            pcb_scheduler: crate::channels::PcgPcbScheduler::new(0),
        });

        // Advance LC to frame boundary (like BTS TX loop does)
        ch.advance_lc_to_chip(frame_boundary);

        // Generate 10 null frames (like idle traffic channel)
        let null_frames_count = 10;
        for _ in 0..null_frames_count {
            let _ = ch.next(CdmaSystemTime::default());
        }

        // Now queue a full-rate signaling frame (BS Ack Order)
        let mut info_bits = vec![0u8; 172];
        info_bits[0] = 1; // MM=1
        info_bits[1] = 0; // TT=0
        info_bits[2] = 1; // TM=11
        info_bits[3] = 1;
        for i in 4..172 {
            info_bits[i] = ((i * 7 + 3) % 2) as u8;
        }

        ch.send_frame(TrafficFrame {
            data: info_bits.clone(),
            rate: TrafficRate::Full,
        });
        let tx_symbols = ch.next(CdmaSystemTime::default());
        assert_eq!(tx_symbols.len(), 384);

        // The LC chip position for THIS frame:
        let signaling_frame_lc_start = frame_boundary + (null_frames_count as u64) * 24576;

        // === RX decode ===
        let mut hard_bits: Vec<u8> = tx_symbols
            .iter()
            .map(|s| if s.re > 0.0 { 0u8 } else { 1u8 })
            .collect();

        // Generate LC at the correct position for the signaling frame
        let mut lc = LongCodeGenerator::new_traffic_channel(esn);
        lc.advance_chips(signaling_frame_lc_start as usize);
        let mut lc_decimated = [0u8; 384];
        for i in 0..384 {
            lc_decimated[i] = lc.next_chip();
            for _ in 1..64 {
                lc.next_chip();
            }
        }

        // Compute PC positions
        let mut pc_positions = [0usize; 16];
        for pcg in 0..16 {
            let base = pcg * 24;
            let b3 = lc_decimated[base + 23] as usize;
            let b2 = lc_decimated[base + 22] as usize;
            let b1 = lc_decimated[base + 21] as usize;
            let b0 = lc_decimated[base + 20] as usize;
            pc_positions[pcg] = (b3 << 3) | (b2 << 2) | (b1 << 1) | b0;
        }

        // De-scramble
        for i in 0..384 {
            hard_bits[i] ^= lc_decimated[i];
        }

        // De-interleave
        let mut interleaver = BitReversalInterleaver::new(SR1_PARAMS_384);
        let deinterleaved = interleaver.decode(&hard_bits);

        // Viterbi decode R=1/2 K=9
        let mut decoder = ViterbiDecoder::new(get_1_2_k9_encoder());
        let symbol_pairs: Vec<[u8; 2]> = deinterleaved
            .chunks_exact(2)
            .map(|c| [c[0], c[1]])
            .collect();
        let decoded = decoder.decode_block_from_state(&symbol_pairs, 0);
        assert_eq!(decoded.len(), 192);

        // Verify info bits
        assert_eq!(
            &decoded[..172],
            &info_bits[..],
            "decoded info bits don't match after LC advance + {} null frames",
            null_frames_count,
        );

        // Verify CRC-12
        let computed_crc = crc12(&decoded[..172]);
        let mut received_crc: u16 = 0;
        for &bit in &decoded[172..184] {
            received_crc = (received_crc << 1) | (bit as u16 & 1);
        }
        assert_eq!(
            computed_crc, received_crc,
            "CRC-12 mismatch after multi-frame: computed=0x{:03X} received=0x{:03X}",
            computed_crc, received_crc
        );

        // Verify tail bits
        assert_eq!(&decoded[184..192], &[0u8; 8]);

        eprintln!(
            "multi-frame loopback PASSED: lc_start={} null_frames={} signaling_lc_start={}",
            frame_boundary, null_frames_count, signaling_frame_lc_start
        );
    }

    #[test]
    fn test_symbols_are_unit_magnitude() {
        let ch = make_channel();
        let frame = ch.next(CdmaSystemTime::default());
        for (i, s) in frame.iter().enumerate() {
            assert!(
                (s.re.abs() - 1.0).abs() < 1e-6,
                "Symbol {} real part should be ±1.0, got {}",
                i,
                s.re
            );
            assert!(
                s.im.abs() < 1e-6,
                "Symbol {} imaginary part should be 0, got {}",
                i,
                s.im
            );
        }
    }
}
