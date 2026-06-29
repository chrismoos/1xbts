use cdma_common::crc::crc12;
use num::complex::Complex32;

use crate::phy::coding::block_interleaver::{
    Rc12ReverseTrafficInterleaver, Rc12ReverseTrafficRate,
};
use crate::phy::coding::convolutional::get_1_3_k9_encoder;
use crate::phy::coding::long_code::LongCodeGenerator;
use crate::phy::spread::PnSequence;
use crate::phy::walsh::WalshGenerator;
use crate::sdr::{cdma2000_baseband_filter_taps_f64, fir::ComplexFir32};

use cdma_common::consts::{RC1_PN_CHIPS_PER_WALSH_CHIP, SR1_CHIPS_PER_FRAME};
#[cfg(test)]
const WALSH_ORDER: usize = 64;
const SYMBOLS_PER_WALSH: usize = 6;
const FULL_RATE_INFO_BITS: usize = 172;
const FULL_RATE_CRC_BITS: usize = 12;
const FULL_RATE_TAIL_BITS: usize = 8;
const FULL_RATE_FRAME_BITS: usize = FULL_RATE_INFO_BITS + FULL_RATE_CRC_BITS + FULL_RATE_TAIL_BITS;

fn pulse_filter(samples: &[Complex32]) -> Vec<Complex32> {
    let taps = cdma2000_baseband_filter_taps_f64();
    ComplexFir32::new(&taps).process_block(samples)
}

/// Test-oriented reverse traffic channel encoder (RC1/RC2, full-rate 9600 bps).
///
/// Produces pulse-shaped IQ samples from 172 info bits using the full
/// reverse traffic encoding chain: CRC-12 → conv encode (R=1/3, K=9) →
/// Rc12 interleave → 64-ary Walsh → LC×PN spreading → pulse shaping.
pub struct ReverseTrafficChannelEncoder {
    esn: u32,
    oversample: usize,
}

impl ReverseTrafficChannelEncoder {
    pub fn new(esn: u32) -> Self {
        Self { esn, oversample: 4 }
    }

    /// Encode a full-rate (9600 bps) reverse traffic frame.
    ///
    /// `info_bits`: 172 bits (individual u8 values, each 0 or 1)
    /// `frame_chip_offset`: absolute chip start of this frame (for LC/PN alignment)
    ///
    /// Returns pulse-shaped IQ samples at `oversample` samples per chip.
    pub fn encode_full_rate_frame(
        &self,
        info_bits: &[u8],
        frame_chip_offset: u64,
    ) -> Vec<Complex32> {
        assert_eq!(info_bits.len(), FULL_RATE_INFO_BITS);

        // 1. CRC-12 over info bits
        let crc = crc12(info_bits);
        let mut frame_bits = Vec::with_capacity(FULL_RATE_FRAME_BITS);
        frame_bits.extend_from_slice(info_bits);
        // Append CRC bits MSB first
        for i in (0..FULL_RATE_CRC_BITS).rev() {
            frame_bits.push(((crc >> i) & 1) as u8);
        }
        // 8 encoder tail zeros
        frame_bits.extend(std::iter::repeat(0u8).take(FULL_RATE_TAIL_BITS));
        assert_eq!(frame_bits.len(), FULL_RATE_FRAME_BITS);

        // 2. R=1/3 K=9 convolutional encode → 576 code symbols
        let mut encoder = get_1_3_k9_encoder();
        let mut code_symbols = Vec::with_capacity(FULL_RATE_FRAME_BITS * 3);
        for &bit in &frame_bits {
            let out = encoder.encode(bit);
            code_symbols.extend_from_slice(&out);
        }
        assert_eq!(code_symbols.len(), 576);

        // 3. Rc12 reverse traffic interleaver (full rate = no symbol repetition)
        let interleaver = Rc12ReverseTrafficInterleaver::new(Rc12ReverseTrafficRate::Full);
        let interleaved = interleaver.encode(&code_symbols);
        assert_eq!(interleaved.len(), 576);

        // 4. 64-ary Walsh modulation: 6 symbols → Walsh index → 64 Walsh chips
        let walsh_matrix = WalshGenerator::generate_matrix::<64>();
        let mut walsh_chips: Vec<i8> = Vec::with_capacity(6144);
        for group in interleaved.chunks_exact(SYMBOLS_PER_WALSH) {
            let index = group[0] as usize
                + 2 * group[1] as usize
                + 4 * group[2] as usize
                + 8 * group[3] as usize
                + 16 * group[4] as usize
                + 32 * group[5] as usize;
            walsh_chips.extend_from_slice(&walsh_matrix[index]);
        }
        assert_eq!(walsh_chips.len(), 6144);

        // 5. LC×PN spreading
        self.spread_and_shape(
            &walsh_chips,
            frame_chip_offset,
            SR1_CHIPS_PER_FRAME as usize,
        )
    }

    /// Generate preamble (Walsh 0 = all +1, just LC×PN).
    ///
    /// Returns pulse-shaped IQ samples for `num_chips` of preamble.
    pub fn encode_preamble(&self, num_chips: usize, chip_offset: u64) -> Vec<Complex32> {
        // Preamble is all Walsh 0 (all +1), so walsh_chips are all +1
        let walsh_chips: Vec<i8> = vec![1i8; num_chips / RC1_PN_CHIPS_PER_WALSH_CHIP];
        // For preamble, each Walsh chip still maps to 4 PN chips
        self.spread_and_shape(&walsh_chips, chip_offset, num_chips)
    }

    /// Core spreading + pulse shaping shared by both data frames and preamble.
    /// Encode a full-rate frame, returning raw (pre-pulse-shaped) IQ samples.
    /// Use this when encoding multiple consecutive frames to apply a single
    /// continuous pulse-shaping FIR across the entire stream.
    pub fn encode_full_rate_frame_raw(
        &self,
        info_bits: &[u8],
        frame_chip_offset: u64,
    ) -> Vec<Complex32> {
        assert_eq!(info_bits.len(), FULL_RATE_INFO_BITS);

        let crc = crc12(info_bits);
        let mut frame_bits = Vec::with_capacity(FULL_RATE_FRAME_BITS);
        frame_bits.extend_from_slice(info_bits);
        for i in (0..FULL_RATE_CRC_BITS).rev() {
            frame_bits.push(((crc >> i) & 1) as u8);
        }
        frame_bits.extend(std::iter::repeat(0u8).take(FULL_RATE_TAIL_BITS));
        assert_eq!(frame_bits.len(), FULL_RATE_FRAME_BITS);

        let mut encoder = get_1_3_k9_encoder();
        let mut code_symbols = Vec::with_capacity(FULL_RATE_FRAME_BITS * 3);
        for &bit in &frame_bits {
            code_symbols.extend_from_slice(&encoder.encode(bit));
        }
        assert_eq!(code_symbols.len(), 576);

        let interleaver = Rc12ReverseTrafficInterleaver::new(Rc12ReverseTrafficRate::Full);
        let interleaved = interleaver.encode(&code_symbols);
        assert_eq!(interleaved.len(), 576);

        let walsh_matrix = WalshGenerator::generate_matrix::<64>();
        let mut walsh_chips: Vec<i8> = Vec::with_capacity(6144);
        for group in interleaved.chunks_exact(SYMBOLS_PER_WALSH) {
            let index = group[0] as usize
                + 2 * group[1] as usize
                + 4 * group[2] as usize
                + 8 * group[3] as usize
                + 16 * group[4] as usize
                + 32 * group[5] as usize;
            walsh_chips.extend_from_slice(&walsh_matrix[index]);
        }
        assert_eq!(walsh_chips.len(), 6144);

        self.spread_no_shape(
            &walsh_chips,
            frame_chip_offset,
            SR1_CHIPS_PER_FRAME as usize,
        )
    }

    /// Generate raw (pre-pulse-shaped) preamble samples.
    pub fn encode_preamble_raw(&self, num_chips: usize, chip_offset: u64) -> Vec<Complex32> {
        let walsh_chips = vec![1i8; num_chips / RC1_PN_CHIPS_PER_WALSH_CHIP];
        self.spread_no_shape(&walsh_chips, chip_offset, num_chips)
    }

    /// LC×PN spread without pulse shaping.
    fn spread_no_shape(
        &self,
        walsh_chips: &[i8],
        chip_offset: u64,
        total_pn_chips: usize,
    ) -> Vec<Complex32> {
        let oversample = self.oversample;
        let pn_len = total_pn_chips * oversample;
        let pn_samples = build_oqpsk_pn_samples(pn_len, oversample);
        let pn_rotate =
            (chip_offset as usize * oversample) % 32768_usize.saturating_mul(oversample);
        let mut pn_rotated = pn_samples;
        if pn_rotate > 0 && pn_rotate < pn_rotated.len() {
            pn_rotated.rotate_left(pn_rotate);
        }
        let mut pn_iter = pn_rotated.into_iter();

        let mut lc_gen = LongCodeGenerator::new_traffic_channel(self.esn);
        lc_gen.advance_chips(chip_offset as usize);

        let mut tx_raw: Vec<Complex32> = Vec::with_capacity(total_pn_chips * oversample);
        for &wchip in walsh_chips {
            let w: f32 = wchip as f32;
            for _ in 0..RC1_PN_CHIPS_PER_WALSH_CHIP {
                let lc_chip = lc_gen.next_chip();
                let lc_sign: f32 = if lc_chip == 1 { -1.0 } else { 1.0 };
                for _ in 0..oversample {
                    let pn_iq = pn_iter.next().unwrap();
                    tx_raw.push(Complex32::new(
                        w * lc_sign * pn_iq.re,
                        w * lc_sign * pn_iq.im,
                    ));
                }
            }
        }
        tx_raw
    }

    fn spread_and_shape(
        &self,
        walsh_chips: &[i8],
        chip_offset: u64,
        total_pn_chips: usize,
    ) -> Vec<Complex32> {
        let oversample = self.oversample;

        // Build OQPSK PN samples
        let pn_len = total_pn_chips * oversample;
        let pn_samples = build_oqpsk_pn_samples(pn_len, oversample);
        let pn_rotate =
            (chip_offset as usize * oversample) % 32768_usize.saturating_mul(oversample);
        let mut pn_rotated = pn_samples;
        if pn_rotate > 0 && pn_rotate < pn_rotated.len() {
            pn_rotated.rotate_left(pn_rotate);
        }
        let mut pn_iter = pn_rotated.into_iter();

        // LC generator for traffic channel
        let mut lc_gen = LongCodeGenerator::new_traffic_channel(self.esn);
        lc_gen.advance_chips(chip_offset as usize);

        // Walsh × LC × PN spreading
        let mut tx_raw: Vec<Complex32> = Vec::with_capacity(total_pn_chips * oversample);
        for &wchip in walsh_chips {
            let w: f32 = wchip as f32;
            for _ in 0..RC1_PN_CHIPS_PER_WALSH_CHIP {
                let lc_chip = lc_gen.next_chip();
                let lc_sign: f32 = if lc_chip == 1 { -1.0 } else { 1.0 };
                for _ in 0..oversample {
                    let pn_iq = pn_iter.next().unwrap();
                    tx_raw.push(Complex32::new(
                        w * lc_sign * pn_iq.re,
                        w * lc_sign * pn_iq.im,
                    ));
                }
            }
        }

        pulse_filter(&tx_raw)
    }
}

/// Apply CDMA2000 baseband pulse-shaping FIR to a raw IQ stream.
/// Use this after concatenating multiple frames' raw samples to get
/// a continuous pulse-shaped signal without frame-boundary transients.
pub fn pulse_shape(raw: &[Complex32]) -> Vec<Complex32> {
    pulse_filter(raw)
}

/// Build OQPSK PN samples with half-chip Q delay.
fn build_oqpsk_pn_samples(output_len: usize, oversample: usize) -> Vec<Complex32> {
    assert_eq!(
        0,
        oversample % 2,
        "OQPSK half-chip delay requires even oversample"
    );
    let q_delay_samples = oversample / 2;
    let mut pn = PnSequence::new_repeat(0, 32768, oversample.saturating_sub(1));
    let mut pn_i = Vec::with_capacity(output_len);
    let mut pn_q = Vec::with_capacity(output_len);
    for _ in 0..output_len {
        let s = pn.generate_iq();
        pn_i.push(s.re);
        pn_q.push(s.im);
    }
    (0..output_len)
        .map(|k| {
            let q_idx = k.saturating_sub(q_delay_samples);
            Complex32::new(pn_i[k], pn_q[q_idx])
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_full_rate_frame_produces_correct_sample_count() {
        let encoder = ReverseTrafficChannelEncoder::new(0xDEADBEEF);
        let info_bits = vec![0u8; FULL_RATE_INFO_BITS];
        let samples = encoder.encode_full_rate_frame(&info_bits, 0);
        // 24576 chips × 4 oversample = 98304 samples
        assert_eq!(samples.len(), SR1_CHIPS_PER_FRAME as usize * 4);
    }

    #[test]
    fn test_encode_preamble_produces_correct_sample_count() {
        let encoder = ReverseTrafficChannelEncoder::new(0xDEADBEEF);
        let preamble_chips = 24576 * 2; // 2 frames
        let samples = encoder.encode_preamble(preamble_chips, 0);
        assert_eq!(samples.len(), preamble_chips * 4);
    }

    #[test]
    fn test_crc12_known_value() {
        // All zeros should produce a known CRC
        let data = vec![0u8; 172];
        let crc = crc12(&data);
        // CRC should be non-zero (initial register is 0x0FFF)
        assert_ne!(crc, 0);
    }

    fn quick_walsh_demod(chip_samples: &[Complex32]) -> Vec<f32> {
        let mut soft_bits: Vec<f32> = Vec::new();
        let sym_count = chip_samples.len() / (RC1_PN_CHIPS_PER_WALSH_CHIP * WALSH_ORDER);
        for sym_idx in 0..sym_count {
            let mut walsh_corr = [Complex32::new(0.0, 0.0); WALSH_ORDER];
            for wc in 0..WALSH_ORDER {
                let base = sym_idx * RC1_PN_CHIPS_PER_WALSH_CHIP * WALSH_ORDER
                    + wc * RC1_PN_CHIPS_PER_WALSH_CHIP;
                walsh_corr[wc] = chip_samples[base..base + RC1_PN_CHIPS_PER_WALSH_CHIP]
                    .iter()
                    .copied()
                    .sum();
            }
            let mut span = 1usize;
            while span < WALSH_ORDER {
                let step = span * 2;
                for base in (0..WALSH_ORDER).step_by(step) {
                    for idx in 0..span {
                        let a = walsh_corr[base + idx];
                        let b = walsh_corr[base + idx + span];
                        walsh_corr[base + idx] = a + b;
                        walsh_corr[base + idx + span] = a - b;
                    }
                }
                span <<= 1;
            }
            let energies: Vec<f32> = walsh_corr
                .iter()
                .map(|c| c.re * c.re + c.im * c.im)
                .collect();
            for bit in 0..SYMBOLS_PER_WALSH {
                let mut max_zero = f32::NEG_INFINITY;
                let mut max_one = f32::NEG_INFINITY;
                for (row, &energy) in energies.iter().enumerate() {
                    if ((row >> bit) & 1) == 0 {
                        max_zero = max_zero.max(energy);
                    } else {
                        max_one = max_one.max(energy);
                    }
                }
                soft_bits.push(max_zero - max_one);
            }
        }
        soft_bits
    }

    fn quick_viterbi(deinterleaved: &[f32]) -> Vec<u8> {
        use crate::phy::coding::convolutional::get_1_3_k9_soft_viterbi_decoder;
        let peak = deinterleaved.iter().map(|v| v.abs()).fold(0.0f32, f32::max);
        let inv_peak = if peak > 1e-12 { 1.0 / peak } else { 1.0 };
        let inputs: Vec<[f32; 3]> = deinterleaved
            .chunks_exact(3)
            .map(|chunk| {
                [
                    (0.5 - chunk[0] * 0.5 * inv_peak).clamp(0.0, 1.0),
                    (0.5 - chunk[1] * 0.5 * inv_peak).clamp(0.0, 1.0),
                    (0.5 - chunk[2] * 0.5 * inv_peak).clamp(0.0, 1.0),
                ]
            })
            .collect();
        let mut decoder = get_1_3_k9_soft_viterbi_decoder();
        decoder.decode_block_from_state(&inputs, 0)
    }

    /// Full encode→decode loopback: encode a frame, then reverse the chain
    /// (deinterleave, Viterbi decode) and verify CRC-12 matches.
    #[test]
    fn test_full_rate_encode_decode_loopback_crc_valid() {
        use crate::phy::coding::convolutional::get_1_3_k9_soft_viterbi_decoder;

        let esn = 0xDEADBEEF_u32;
        let chip_offset = 196_608u64; // typical BTS start

        // Build a test frame with known content
        let mut info_bits = vec![0u8; FULL_RATE_INFO_BITS];
        for i in 0..172 {
            info_bits[i] = ((i * 7 + 3) % 11 > 5) as u8;
        }

        // === ENCODE ===
        let crc = crc12(&info_bits);
        let mut frame_bits = Vec::with_capacity(192);
        frame_bits.extend_from_slice(&info_bits);
        for i in (0..12).rev() {
            frame_bits.push(((crc >> i) & 1) as u8);
        }
        frame_bits.extend(std::iter::repeat(0u8).take(8));
        assert_eq!(frame_bits.len(), 192);

        // Conv encode R=1/3 K=9
        let mut enc = get_1_3_k9_encoder();
        let mut code_symbols = Vec::with_capacity(576);
        for &bit in &frame_bits {
            code_symbols.extend_from_slice(&enc.encode(bit));
        }
        assert_eq!(code_symbols.len(), 576);

        // Interleave
        let interleaver = Rc12ReverseTrafficInterleaver::new(Rc12ReverseTrafficRate::Full);
        let interleaved = interleaver.encode(&code_symbols);
        assert_eq!(interleaved.len(), 576);

        // 64-ary Walsh modulation → LC×PN spreading → pulse shape → IQ
        let walsh_matrix = WalshGenerator::generate_matrix::<64>();
        let mut walsh_chips: Vec<i8> = Vec::with_capacity(6144);
        for group in interleaved.chunks_exact(SYMBOLS_PER_WALSH) {
            let index = group[0] as usize
                + 2 * group[1] as usize
                + 4 * group[2] as usize
                + 8 * group[3] as usize
                + 16 * group[4] as usize
                + 32 * group[5] as usize;
            walsh_chips.extend_from_slice(&walsh_matrix[index]);
        }
        assert_eq!(walsh_chips.len(), 6144);

        // LC×PN spread
        let mut lc_gen = LongCodeGenerator::new_traffic_channel(esn);
        lc_gen.advance_chips(chip_offset as usize);
        let oversample = 4usize;
        let pn_len = SR1_CHIPS_PER_FRAME as usize * oversample;
        let pn_samples = super::build_oqpsk_pn_samples(pn_len, oversample);
        let pn_rotate = (chip_offset as usize * oversample) % (32768 * oversample);
        let mut pn_rotated = pn_samples;
        if pn_rotate > 0 && pn_rotate < pn_rotated.len() {
            pn_rotated.rotate_left(pn_rotate);
        }
        let mut pn_iter = pn_rotated.iter();

        let mut tx_raw: Vec<Complex32> =
            Vec::with_capacity(SR1_CHIPS_PER_FRAME as usize * oversample);
        for &wchip in &walsh_chips {
            let w: f32 = wchip as f32;
            for _ in 0..RC1_PN_CHIPS_PER_WALSH_CHIP {
                let lc_chip = lc_gen.next_chip();
                let lc_sign: f32 = if lc_chip == 1 { -1.0 } else { 1.0 };
                for _ in 0..oversample {
                    let pn_iq = pn_iter.next().unwrap();
                    tx_raw.push(Complex32::new(
                        w * lc_sign * pn_iq.re,
                        w * lc_sign * pn_iq.im,
                    ));
                }
            }
        }

        // Pulse shape
        let tx_shaped = pulse_filter(&tx_raw);

        // === DECODE (without BTS infrastructure) ===
        // Matched filter
        let rx_filtered = pulse_filter(&tx_shaped);

        // PN×LC despread at chip rate
        let pn_period = 32768 * oversample;
        let pn_despread_ref: Vec<Complex32> = super::build_oqpsk_pn_samples(pn_period, oversample)
            .into_iter()
            .map(|s| Complex32::new(s.re, -s.im)) // conjugate
            .collect();

        // Zero-pad rx_filtered so that sample_start offsets near the composite
        // filter delay (~47 samples) can still address all SR1_CHIPS_PER_FRAME as usize chips.
        let pad_needed = 64 * oversample; // generous padding
        let mut rx_padded = rx_filtered;
        rx_padded.resize(rx_padded.len() + pad_needed, Complex32::new(0.0, 0.0));

        // The TX FIR + RX matched filter introduce ~47 samples of group delay.
        // Sweep sample_start to find the offset that compensates the delay.
        let mut best_errors = 172usize;
        let mut best_sample_start = 0usize;
        for sample_start in 40..56 {
            let mut lc_rx = LongCodeGenerator::new_traffic_channel(esn);
            lc_rx.advance_chips(chip_offset as usize);

            let mut test_chips: Vec<Complex32> = Vec::new();
            for chip in 0..SR1_CHIPS_PER_FRAME as usize {
                let sample_idx = sample_start + chip * oversample;
                if sample_idx >= rx_padded.len() {
                    break;
                }
                let pn_idx = ((chip_offset as usize + chip) * oversample) % pn_period;
                let pn = pn_despread_ref[pn_idx];
                let despread = rx_padded[sample_idx] * pn;

                let lc_bit = lc_rx.next_chip();
                let lc_conj = Complex32::new(if lc_bit == 1 { -1.0 } else { 1.0 }, 0.0);
                test_chips.push(despread * lc_conj);
            }

            let min_chips = SR1_CHIPS_PER_FRAME as usize - 256; // allow last Walsh symbol to be incomplete
            if test_chips.len() < min_chips {
                eprintln!(
                    "  sample_start={}: not enough chips ({})",
                    sample_start,
                    test_chips.len()
                );
                continue;
            }
            let test_soft = quick_walsh_demod(&test_chips);
            let test_deint = interleaver.decode_soft(&test_soft);
            let test_decoded = quick_viterbi(&test_deint);
            let errs = if test_decoded.len() >= 172 {
                test_decoded[..172]
                    .iter()
                    .zip(info_bits.iter())
                    .filter(|(a, b)| a != b)
                    .count()
            } else {
                172
            };
            eprintln!("  sample_start={}: bit_errors={}/172", sample_start, errs);
            if errs < best_errors {
                best_errors = errs;
                best_sample_start = sample_start;
            }
        }
        eprintln!(
            "Best sample_start: {} with {} errors",
            best_sample_start, best_errors
        );

        // Use the best offset
        let center_offset = best_sample_start;
        let mut lc_rx = LongCodeGenerator::new_traffic_channel(esn);
        lc_rx.advance_chips(chip_offset as usize);

        let mut chip_samples: Vec<Complex32> = Vec::new();
        for chip in 0..SR1_CHIPS_PER_FRAME as usize {
            let sample_idx = center_offset + chip * oversample;
            if sample_idx >= rx_padded.len() {
                break;
            }
            let pn_idx = ((chip_offset as usize + chip) * oversample) % pn_period;
            let pn = pn_despread_ref[pn_idx];
            let despread = rx_padded[sample_idx] * pn;

            let lc_bit = lc_rx.next_chip();
            let lc_conj = Complex32::new(if lc_bit == 1 { -1.0 } else { 1.0 }, 0.0);
            chip_samples.push(despread * lc_conj);
        }

        // Walsh demodulation (Hadamard transform per symbol)
        let mut soft_bits: Vec<f32> = Vec::new();
        for sym_idx in 0..(chip_samples.len() / (RC1_PN_CHIPS_PER_WALSH_CHIP * WALSH_ORDER)) {
            let mut walsh_corr = [Complex32::new(0.0, 0.0); WALSH_ORDER];
            for wc in 0..WALSH_ORDER {
                let base = sym_idx * RC1_PN_CHIPS_PER_WALSH_CHIP * WALSH_ORDER
                    + wc * RC1_PN_CHIPS_PER_WALSH_CHIP;
                walsh_corr[wc] = chip_samples[base..base + RC1_PN_CHIPS_PER_WALSH_CHIP]
                    .iter()
                    .copied()
                    .sum();
            }
            // Hadamard transform
            let mut span = 1usize;
            while span < WALSH_ORDER {
                let step = span * 2;
                for base in (0..WALSH_ORDER).step_by(step) {
                    for idx in 0..span {
                        let a = walsh_corr[base + idx];
                        let b = walsh_corr[base + idx + span];
                        walsh_corr[base + idx] = a + b;
                        walsh_corr[base + idx + span] = a - b;
                    }
                }
                span <<= 1;
            }

            let energies: Vec<f32> = walsh_corr
                .iter()
                .map(|c| c.re * c.re + c.im * c.im)
                .collect();

            for bit in 0..SYMBOLS_PER_WALSH {
                let mut max_zero = f32::NEG_INFINITY;
                let mut max_one = f32::NEG_INFINITY;
                for (row, &energy) in energies.iter().enumerate() {
                    if ((row >> bit) & 1) == 0 {
                        max_zero = max_zero.max(energy);
                    } else {
                        max_one = max_one.max(energy);
                    }
                }
                soft_bits.push(max_zero - max_one);
            }
        }

        // Deinterleave
        let deinterleaved = interleaver.decode_soft(&soft_bits);

        // Viterbi decode
        let peak = deinterleaved.iter().map(|v| v.abs()).fold(0.0f32, f32::max);
        let inv_peak = if peak > 1e-12 { 1.0 / peak } else { 1.0 };
        let inputs: Vec<[f32; 3]> = deinterleaved
            .chunks_exact(3)
            .map(|chunk| {
                [
                    (0.5 - chunk[0] * 0.5 * inv_peak).clamp(0.0, 1.0),
                    (0.5 - chunk[1] * 0.5 * inv_peak).clamp(0.0, 1.0),
                    (0.5 - chunk[2] * 0.5 * inv_peak).clamp(0.0, 1.0),
                ]
            })
            .collect();
        let mut decoder = get_1_3_k9_soft_viterbi_decoder();
        let decoded_bits = decoder.decode_block_from_state(&inputs, 0);

        // Check CRC-12
        assert!(
            decoded_bits.len() >= 192,
            "expected 192 decoded bits, got {}",
            decoded_bits.len()
        );
        let decoded_info = &decoded_bits[..172];
        let computed_crc = crc12(decoded_info);
        let mut received_crc: u16 = 0;
        for &bit in &decoded_bits[172..184] {
            received_crc = (received_crc << 1) | (bit as u16 & 1);
        }
        eprintln!(
            "CRC-12 loopback: computed=0x{:03X} received=0x{:03X} match={}",
            computed_crc,
            received_crc,
            computed_crc == received_crc
        );

        // Check tail bits
        let tail_valid = decoded_bits[184..192].iter().all(|&b| b == 0);
        eprintln!("Tail valid: {}", tail_valid);

        // Check info bit match
        let info_match = decoded_info
            .iter()
            .zip(info_bits.iter())
            .all(|(a, b)| a == b);
        eprintln!("Info bits match: {}", info_match);

        let bit_errors: usize = decoded_info
            .iter()
            .zip(info_bits.iter())
            .filter(|(a, b)| a != b)
            .count();
        eprintln!("Bit errors: {} / 172", bit_errors);

        assert_eq!(
            computed_crc, received_crc,
            "CRC-12 should match after full encode→decode loopback"
        );
        assert!(tail_valid, "Tail bits should be all zero");
        assert!(info_match, "Decoded info bits should match original");
    }
}
