use std::collections::HashMap;

use cdma_common::time;
use num_complex::Complex32;

use crate::phy::coding::long_code::LongCodeGenerator;
use log::{info, trace};

use super::{CDMA_CHIP_RATE, PipelineProcessor, SampleBlock};

/// Number of symbols (64 chips each) to accumulate before running
/// acquisition.  Larger values give the FFT diagnostic more data to
/// cross-correlate and improve LC detection SNR.
/// 384 symbols = 24,576 chips = one 20ms frame at SR1.
const ACQUISITION_SYMBOLS: usize = 384;
/// Minimum per-chip energy (linear) to consider preamble present.
/// This is a loose threshold — CRC validation downstream is the real gate.
const PREAMBLE_ENERGY_FLOOR: f32 = 2.0;

/// Reverse Access long-code descrambler operating on oversampled chip blocks.
///
/// Preamble detection uses non-coherent energy detection — squaring removes
/// the long-code modulation (`|LC[n] * h|^2 = |h|^2`), so we detect the
/// access probe without knowing or searching the long-code phase. Once
/// detected, the LC phase is deterministic from PN timing + access channel
/// parameters. CRC validation downstream is the real confirmation gate.
pub struct ReverseAccessLongCodeProcessor {
    template: LongCodeGenerator,
    generator: LongCodeGenerator,
    oversample: usize,
    next_chip: Option<usize>,
    logged_symbols: usize,
    acquisition_buffer: Vec<Complex32>,
    acquisition_tags: HashMap<&'static str, i64>,
    acquisition_chip_start: usize,
    acquisition_sample_rate_hz: f64,
}

impl ReverseAccessLongCodeProcessor {
    pub fn new(generator: LongCodeGenerator, oversample: usize) -> Self {
        Self {
            template: generator.clone(),
            generator,
            oversample: oversample.max(1),
            next_chip: None,
            logged_symbols: 0,
            acquisition_buffer: Vec::new(),
            acquisition_tags: HashMap::new(),
            acquisition_chip_start: 0,
            acquisition_sample_rate_hz: 0.0,
        }
    }

    fn chip_start_from_block(&self, block: &SampleBlock) -> usize {
        block
            .tags
            .get("absolute_chip_start")
            .copied()
            .map(|v| v.max(0) as usize)
            .unwrap_or(block.chip_start / self.oversample)
    }

    fn acquisition_samples_needed(&self) -> usize {
        self.oversample * 64 * ACQUISITION_SYMBOLS
    }

    fn should_acquire(&self, block: &SampleBlock) -> bool {
        self.next_chip.is_none() && block.tags.contains_key("absolute_chip_start")
    }

    /// Detect preamble using differential (lag-1) correlation.
    ///
    /// After PN despreading, preamble chips are y[n] = A * LC[n].
    /// Multiplying adjacent chips: y[n] * conj(y[n-1]) = A^2 * LC[n]*LC[n-1].
    /// The product LC[n]*LC[n-1] is a deterministic ±1 sequence that produces
    /// correlated structure, so |sum(y[n]*conj(y[n-1]))| is large during
    /// preamble and small during noise.  This cancels the unknown long code
    /// without any phase search.
    ///
    /// Also computes per-chip energy as a secondary metric.
    ///
    /// Returns `(detected, avg_chip_energy, diff_metric)`.
    fn detect_preamble_differential(&self, samples: &[Complex32]) -> (bool, f32, f32) {
        let chip_len = self.oversample;

        if samples.len() < chip_len * 128 {
            return (false, 0.0, 0.0);
        }

        // Decimate to chip rate by summing each oversample group.
        let mut chips: Vec<Complex32> = Vec::with_capacity(samples.len() / chip_len);
        for chunk in samples.chunks_exact(chip_len) {
            chips.push(
                chunk
                    .iter()
                    .copied()
                    .fold(Complex32::new(0.0, 0.0), |a, v| a + v),
            );
        }

        // Per-chip energy (secondary metric).
        let total_energy: f32 = chips.iter().map(|c| c.norm_sqr()).sum();
        let avg_chip_energy = total_energy / chips.len().max(1) as f32;

        // Differential correlation: sum |y[n] * conj(y[n-1])|
        // Integrate over 64-chip windows (one Walsh symbol period).
        let mut diff_metric = 0.0f32;
        let mut diff_count = 0usize;
        for window in chips.windows(2) {
            diff_metric += (window[1] * window[0].conj()).norm();
            diff_count += 1;
        }
        let avg_diff = diff_metric / diff_count.max(1) as f32;

        // Normalize: diff_metric / energy gives a ratio near 1.0 for
        // correlated signal (preamble) and near 0 for noise.
        let ratio = if avg_chip_energy > 1e-10 {
            avg_diff / avg_chip_energy
        } else {
            0.0
        };

        let detected = avg_chip_energy > PREAMBLE_ENERGY_FLOOR && ratio > 0.5;
        if avg_chip_energy > PREAMBLE_ENERGY_FLOOR * 0.5 {
            trace!(
                "preamble_differential: energy={:.2} diff={:.4} ratio={:.4} detected={}",
                avg_chip_energy, avg_diff, ratio, detected
            );
        }

        (detected, avg_chip_energy, avg_diff)
    }

    /// Score a candidate chip offset by despreading with the long code and
    /// measuring coherent symbol energy.
    ///
    /// Uses the center sample of each chip group (where ISI is zero after
    /// matched filtering) rather than summing the full oversample group.
    fn score_candidate(&self, samples: &[Complex32], candidate_chip: usize) -> f32 {
        let mut lc_gen = self.template.clone();
        lc_gen.advance_chips(candidate_chip);
        let os = self.oversample;
        let center = os / 2;
        let symbol_len = os * 64;
        let mut score = 0.0f32;

        for symbol in samples.chunks_exact(symbol_len).take(ACQUISITION_SYMBOLS) {
            let mut acc = Complex32::new(0.0, 0.0);
            for chip_samples in symbol.chunks_exact(os) {
                let sign = if lc_gen.next_chip() == 1 { -1.0 } else { 1.0 };
                let chip = chip_samples[center] * sign;
                acc += chip;
            }
            score += acc.norm_sqr();
        }

        score
    }

    fn descramble_block(&mut self, block: SampleBlock, chip_start: usize) -> SampleBlock {
        match self.next_chip {
            Some(next_chip) if chip_start >= next_chip => {
                self.generator.advance_chips(chip_start - next_chip);
                self.next_chip = Some(chip_start);
            }
            Some(_) | None => {
                self.generator = self.template.clone();
                self.generator.advance_chips(chip_start);
                self.next_chip = Some(chip_start);
            }
        }

        let mut out = Vec::with_capacity(block.samples.len());
        for samples in block.samples.chunks(self.oversample) {
            let lc_chip = self.generator.next_chip();
            let sign = if lc_chip == 1 { -1.0 } else { 1.0 };
            for sample in samples {
                out.push(Complex32::new(sample.re * sign, sample.im * sign));
            }
        }
        self.next_chip = self
            .next_chip
            .map(|v| v.saturating_add(block.samples.len() / self.oversample));

        for (symbol_idx, symbol) in out.chunks(self.oversample * 64).enumerate() {
            if self.logged_symbols >= 8 || symbol.len() < self.oversample * 64 {
                break;
            }
            let chip_values = symbol
                .chunks(self.oversample)
                .map(|chip| {
                    chip.iter()
                        .copied()
                        .fold(Complex32::new(0.0, 0.0), |acc, v| acc + v)
                })
                .collect::<Vec<_>>();
            let pos = chip_values.iter().filter(|c| c.re >= 0.0).count();
            let neg = chip_values.len().saturating_sub(pos);
            let chips = chip_values
                .iter()
                .map(|c| if c.re >= 0.0 { '+' } else { '-' })
                .collect::<String>();
            trace!(
                "reverse_access_lc_symbol[{}]: chip_start={} pilot_phase={} pos={} neg={} chips={}",
                self.logged_symbols,
                chip_start + (symbol_idx * 64),
                block.tags.get("pilot_phase").copied().unwrap_or(-1),
                pos,
                neg,
                chips
            );
            self.logged_symbols += 1;
        }

        let mut out_block =
            SampleBlock::new(out, block.chip_start).with_sample_rate_hz(block.sample_rate_hz);
        out_block.tags = block.tags;
        out_block
            .tags
            .insert("absolute_chip_start", chip_start as i64);
        out_block
    }

    fn log_locked_descramble_preview(&self, block: &SampleBlock, chip_start: usize) {
        const PREVIEW_SYMBOLS: usize = 4;
        const CHIPS_PER_SYMBOL: usize = 64;

        let chip_values = block
            .samples
            .chunks(self.oversample)
            .take(PREVIEW_SYMBOLS * CHIPS_PER_SYMBOL)
            .map(|chip| {
                chip.iter()
                    .copied()
                    .fold(Complex32::new(0.0, 0.0), |acc, v| acc + v)
            })
            .collect::<Vec<_>>();

        if chip_values.is_empty() {
            return;
        }

        let preview = chip_values
            .chunks(CHIPS_PER_SYMBOL)
            .enumerate()
            .map(|(symbol_idx, chips)| {
                let chip_signs = chips
                    .iter()
                    .map(|chip| if chip.re >= 0.0 { '+' } else { '-' })
                    .collect::<String>();
                let re_sum: f32 = chips.iter().map(|chip| chip.re).sum();
                let im_sum: f32 = chips.iter().map(|chip| chip.im).sum();
                format!(
                    "sym{} chip={} re_sum={:.3} im_sum={:.3} chips={}",
                    symbol_idx,
                    chip_start + (symbol_idx * CHIPS_PER_SYMBOL),
                    re_sum,
                    im_sum,
                    chip_signs
                )
            })
            .collect::<Vec<_>>()
            .join(" | ");

        info!(
            "reverse_access_lc_despread_preview: locked_chip={} pilot_phase={} preview=[{}]",
            chip_start,
            block.tags.get("pilot_phase").copied().unwrap_or(-1),
            preview
        );
    }
}

impl PipelineProcessor for ReverseAccessLongCodeProcessor {
    fn process_block(&mut self, block: SampleBlock) -> Vec<SampleBlock> {
        if self.should_acquire(&block) {
            if self.acquisition_buffer.is_empty() {
                self.acquisition_tags = block.tags.clone();
                self.acquisition_chip_start = block.chip_start;
                self.acquisition_sample_rate_hz = block.sample_rate_hz;
            }
            self.acquisition_buffer.extend_from_slice(&block.samples);

            if self.acquisition_buffer.len() < self.acquisition_samples_needed() {
                return Vec::new();
            }

            // Trim leading zeros from the acquisition buffer.  The rake's
            // FFT overlap-save initialisation fills the first ~32k chips with
            // zeros on the finger's first block.  Keeping them would corrupt
            // preamble detection, LC scoring, and downstream frame alignment.
            let trim_threshold = 0.01f32;
            let first_nonzero = self
                .acquisition_buffer
                .iter()
                .position(|s| s.re.abs() > trim_threshold || s.im.abs() > trim_threshold)
                .unwrap_or(0);
            let trim_samples = (first_nonzero / self.oversample) * self.oversample;
            let trim_chips = trim_samples / self.oversample;
            if trim_chips > 0 {
                self.acquisition_buffer.drain(..trim_samples);
                self.acquisition_chip_start =
                    self.acquisition_chip_start.saturating_add(trim_samples);
                // Advance tags to match the trimmed buffer.
                if let Some(v) = self.acquisition_tags.get_mut("absolute_chip_start") {
                    *v = v.saturating_add(trim_chips as i64);
                }
                if let Some(v) = self.acquisition_tags.get_mut("absolute_sample_start") {
                    *v = v.saturating_add(trim_samples as i64);
                }
                info!(
                    "reverse_access_lc: trimmed {} leading zero chips ({} samples) from acquisition buffer",
                    trim_chips, trim_samples,
                );
            }

            // Re-check: do we still have enough samples after trimming?
            // Don't clear the buffer — let more blocks accumulate.
            if self.acquisition_buffer.len() < self.acquisition_samples_needed() {
                trace!(
                    "reverse_access_lc: insufficient samples after trim ({} < {}), waiting for more",
                    self.acquisition_buffer.len(),
                    self.acquisition_samples_needed(),
                );
                return Vec::new();
            }

            // Differential preamble detection on the trimmed (clean) buffer.
            let (detected, avg_energy, diff_metric) =
                self.detect_preamble_differential(&self.acquisition_buffer);

            let expected_chip = self
                .acquisition_tags
                .get("absolute_chip_start")
                .copied()
                .map(|v| v.max(0) as usize)
                .unwrap_or(self.acquisition_chip_start / self.oversample);

            if !detected {
                trace!(
                    "reverse_access_lc: preamble not detected, avg_chip_energy={:.6} diff_metric={:.6} expected_chip={}",
                    avg_energy, diff_metric, expected_chip,
                );
                self.acquisition_buffer.clear();
                self.acquisition_tags.clear();
                return Vec::new();
            }

            // Use the expected chip directly — the rake's absolute_chip_start
            // already gives us the correct LC alignment.
            let locked_chip = expected_chip;
            let chip_delta = locked_chip as i64 - expected_chip as i64;
            let best_score = self.score_candidate(&self.acquisition_buffer, locked_chip);
            let expected_score = self.score_candidate(&self.acquisition_buffer, expected_chip);
            let mut dbg_gen = self.template.clone();
            dbg_gen.advance_chips(locked_chip);
            let lc_state_at_locked = dbg_gen.state();
            let lc_mask = self.template.mask();
            let locked_sys_time =
                time::system_time_from_chips(locked_chip as u64, CDMA_CHIP_RATE as u64);
            let locked_t20 = time::system_time_20ms_frames(locked_sys_time);
            info!(
                "reverse_access_lc_energy_detected: locked_chip={} locked_sys_time={} locked_t20={} expected_chip={} delta={} avg_chip_energy={:.4} diff_metric={:.4} best_score={:.1} expected_score={:.1} gain={:.1}dB lc_state=0x{:011x} lc_mask=0x{:011x}",
                locked_chip,
                locked_sys_time.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
                locked_t20,
                expected_chip,
                chip_delta,
                avg_energy,
                diff_metric,
                best_score,
                expected_score,
                10.0 * (best_score / expected_score.max(1e-10)).log10(),
                lc_state_at_locked,
                lc_mask,
            );

            let mut tags = self.acquisition_tags.clone();
            tags.insert("absolute_chip_start", locked_chip as i64);
            tags.insert("reverse_access_lc_acquired", 1);
            tags.insert("reverse_access_lc_chip_delta", chip_delta);
            if let Some(abs_sample_start) = tags.get_mut("absolute_sample_start") {
                *abs_sample_start = (*abs_sample_start)
                    .saturating_add(chip_delta.saturating_mul(self.oversample as i64));
            }

            let acq_block = SampleBlock::new(
                std::mem::take(&mut self.acquisition_buffer),
                self.acquisition_chip_start,
            )
            .with_sample_rate_hz(self.acquisition_sample_rate_hz)
            .with_tags(tags);
            self.acquisition_tags.clear();

            let out_block = self.descramble_block(acq_block, locked_chip);
            self.log_locked_descramble_preview(&out_block, locked_chip);
            return vec![out_block];
        }

        let chip_start = self.chip_start_from_block(&block);
        vec![self.descramble_block(block, chip_start)]
    }

    fn name(&self) -> &'static str {
        "ReverseAccessLongCodeProcessor"
    }
}

#[cfg(test)]
mod tests {
    use num_complex::Complex32;

    use super::ReverseAccessLongCodeProcessor;
    use crate::{
        phy::coding::long_code::LongCodeGenerator,
        receiver::pipelined::{PipelineProcessor, SampleBlock},
    };

    #[test]
    fn test_reverse_access_long_code_processor_repeats_sign_per_chip() {
        let mut p = ReverseAccessLongCodeProcessor::new(
            LongCodeGenerator::new_access_channel_with_state(0, 1, 1, 0, 1u64 << 41),
            4,
        );
        let mut ref_gen = LongCodeGenerator::new_access_channel_with_state(0, 1, 1, 0, 1u64 << 41);

        let input = vec![
            Complex32::new(1.0, 0.5),
            Complex32::new(2.0, 1.0),
            Complex32::new(3.0, 1.5),
            Complex32::new(4.0, 2.0),
            Complex32::new(5.0, 2.5),
            Complex32::new(6.0, 3.0),
            Complex32::new(7.0, 3.5),
            Complex32::new(8.0, 4.0),
        ];
        let out = p.process_block(SampleBlock::new(input.clone(), 0));
        assert_eq!(1, out.len());

        for chip_idx in 0..2 {
            let sign = if ref_gen.next_chip() == 1 { -1.0 } else { 1.0 };
            for samp_idx in 0..4 {
                let idx = chip_idx * 4 + samp_idx;
                assert_eq!(input[idx].re * sign, out[0].samples[idx].re);
                assert_eq!(input[idx].im * sign, out[0].samples[idx].im);
            }
        }
    }

    #[test]
    fn test_reverse_access_long_code_processor_detects_preamble_and_descrambles() {
        let oversample = 1;
        let chip_start = 50_037usize;
        let mut processor = ReverseAccessLongCodeProcessor::new(
            LongCodeGenerator::new_access_channel_with_state(0, 1, 1, 0, 1u64 << 41),
            oversample,
        );
        let mut tx_gen = LongCodeGenerator::new_access_channel_with_state(0, 1, 1, 0, 1u64 << 41);
        tx_gen.advance_chips(chip_start);

        let mut expected = Vec::new();
        let mut scrambled = Vec::new();

        for idx in 0..(super::ACQUISITION_SYMBOLS * 64) {
            let chip = Complex32::new(2.0 + (((idx * 17 + 3) % 23) as f32 / 23.0), 0.0);
            let sign = if tx_gen.next_chip() == 1 { -1.0 } else { 1.0 };
            expected.push(chip);
            scrambled.push(Complex32::new(chip.re * sign, chip.im * sign));
        }

        let mut block = SampleBlock::new(scrambled, 0).with_sample_rate_hz(1_228_800.0);
        block.tags.insert("absolute_chip_start", chip_start as i64);
        block
            .tags
            .insert("absolute_sample_start", chip_start as i64);

        let out = processor.process_block(block);
        assert_eq!(1, out.len());
        assert_eq!(expected.len(), out[0].samples.len());
        assert_eq!(Some(&1), out[0].tags.get("reverse_access_lc_acquired"));
        let chip_delta = out[0]
            .tags
            .get("reverse_access_lc_chip_delta")
            .copied()
            .unwrap_or(i64::MAX);
        assert!(
            chip_delta.abs() <= 16,
            "delta {} exceeds fine search window",
            chip_delta
        );
        let actual_chip = out[0].tags.get("absolute_chip_start").copied().unwrap_or(0) as usize;
        assert_eq!(chip_start as i64 + chip_delta, actual_chip as i64);

        for (got, want) in out[0].samples.iter().zip(expected.iter()) {
            assert!(
                (got.re - want.re).abs() < 1e-6,
                "got={} want={}",
                got.re,
                want.re
            );
            assert!(
                (got.im - want.im).abs() < 1e-6,
                "got={} want={}",
                got.im,
                want.im
            );
        }
    }

    #[test]
    fn test_reverse_access_long_code_processor_rejects_noise() {
        let oversample = 1;
        let mut processor = ReverseAccessLongCodeProcessor::new(
            LongCodeGenerator::new_access_channel_with_state(0, 1, 1, 0, 1u64 << 41),
            oversample,
        );

        // All-zero samples (no signal energy)
        let samples = vec![Complex32::new(0.0, 0.0); super::ACQUISITION_SYMBOLS * 64];
        let mut block = SampleBlock::new(samples, 0).with_sample_rate_hz(1_228_800.0);
        block.tags.insert("absolute_chip_start", 1000);

        let out = processor.process_block(block);
        assert!(out.is_empty(), "should reject when no preamble energy");
    }
}
