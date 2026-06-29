use std::collections::HashMap;

use num_complex::Complex32;

use super::{PipelineProcessor, SampleBlock, build_matched_pn_reference};

/// CDMA sliding correlator — acquisition-only processor.
///
/// Replaces `AcquisitionFftProcessor` in the pipeline. Buffers one PN period
/// of oversampled samples, then searches all 32768 chip offsets using
/// noncoherent accumulation to find the PN timing. Once locked, passes
/// through all samples (raw, oversampled) with acquisition tags identical
/// to what `AcquisitionFftProcessor` produces.
///
/// Must be followed by `MatchedFilterDespreader` for despreading/decimation.
pub struct CdmaSlidingCorrelator {
    oversample: usize,
    pn_period_chips: usize,
    pn_period_samples: usize,

    /// Pulse-shaped PN template for correlation (IS-95 convention: PN_I - j·PN_Q)
    pn_template: Vec<Complex32>,

    // ---- Acquisition buffering ----
    acq_buffer: Vec<Complex32>,
    acq_buffer_chip_start: usize,
    acq_buffer_sample_rate: f64,
    acq_dwell_chips: usize,
    snr_threshold_db: f32,
    locked: bool,
    acq_epoch: i64,

    /// The oversampled-sample offset (within one PN period) where PN chip 0 aligns.
    /// This is what the MFD reads via the `acq_peak_sample` tag.
    peak_sample_offset: usize,
}

impl CdmaSlidingCorrelator {
    pub fn new(sample_rate: u32) -> Self {
        let oversample = (sample_rate / 1_228_800).max(1) as usize;
        let pn_period_chips = 32768usize;
        let pn_period_samples = pn_period_chips * oversample;

        // Samples reaching this correlator have already been RX matched-filtered,
        // so the reference models TX shaping plus the RX matched filter.
        let pn_template = build_matched_pn_reference(pn_period_samples, oversample, 2);

        Self {
            oversample,
            pn_period_chips,
            pn_period_samples,
            pn_template,

            acq_buffer: Vec::new(),
            acq_buffer_chip_start: 0,
            acq_buffer_sample_rate: 0.0,
            acq_dwell_chips: 1024,
            snr_threshold_db: 9.0,
            locked: false,
            acq_epoch: 0,
            peak_sample_offset: 0,
        }
    }

    pub fn with_snr_threshold_db(mut self, db: f32) -> Self {
        self.snr_threshold_db = db;
        self
    }

    pub fn with_acq_dwell_chips(mut self, chips: usize) -> Self {
        self.acq_dwell_chips = chips.max(64);
        self
    }

    /// Slide a full-period coherent correlation across all 32768 chip offsets.
    ///
    /// This is the time-domain equivalent of the FFT circular cross-correlation.
    /// SNR = peak |R[k]|² / mean(|R[k']|², k' ≠ k).
    ///
    /// Returns (best_chip_offset, best_snr_db).
    fn search_all_offsets(&self) -> (usize, f32) {
        let buf_len = self.acq_buffer.len();
        let n = self.pn_period_samples;
        assert!(buf_len >= n);

        // Compute |R[k]|² for every chip offset k.
        let mut corr_mag2 = Vec::with_capacity(self.pn_period_chips);
        for chip_offset in 0..self.pn_period_chips {
            let sample_offset = chip_offset * self.oversample;
            let mut sum_i = 0.0f64;
            let mut sum_q = 0.0f64;
            for k in 0..n {
                let bi = (sample_offset + k) % buf_len;
                let s = self.acq_buffer[bi];
                let p = self.pn_template[k];
                sum_i += (s.re * p.re + s.im * p.im) as f64;
                sum_q += (s.im * p.re - s.re * p.im) as f64;
            }
            corr_mag2.push((sum_i * sum_i + sum_q * sum_q) as f32);
        }

        // Find peak and compute SNR = peak / mean(rest).
        let mut best_offset = 0usize;
        let mut best_val = f32::MIN;
        let mut total = 0.0f64;
        for (i, &v) in corr_mag2.iter().enumerate() {
            total += v as f64;
            if v > best_val {
                best_val = v;
                best_offset = i;
            }
        }
        let mean_rest = ((total - best_val as f64)
            / (corr_mag2.len().saturating_sub(1).max(1) as f64))
            .max(1e-20);
        let snr_db = 10.0 * (best_val as f64 / mean_rest).max(1e-20).log10();

        // Print top 5 peaks for debugging.
        let mut sorted: Vec<(usize, f32)> = corr_mag2.iter().copied().enumerate().collect();
        sorted.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
        eprintln!(
            "cdma_sliding_correlator: search top5={:?} snr={:.1}dB (threshold={:.1}dB)",
            sorted.iter().take(5).map(|(i, v)| format!("{}:{:.0}", i, v)).collect::<Vec<_>>(),
            snr_db,
            self.snr_threshold_db
        );

        (best_offset, snr_db as f32)
    }

    fn make_tags(&self) -> HashMap<&'static str, i64> {
        let mut tags = HashMap::new();
        tags.insert("acq_locked", if self.locked { 1 } else { 0 });
        tags.insert("acq_peak_sample", self.peak_sample_offset as i64);
        tags.insert(
            "acq_peak_chip",
            (self.peak_sample_offset / self.oversample.max(1)) as i64,
        );
        tags.insert(
            "acq_timing_phase",
            (self.peak_sample_offset % self.oversample.max(1)) as i64,
        );
        tags.insert("acq_snr_db_x100", 0);
        tags.insert("acq_cfo_hz", 0);
        tags.insert("acq_epoch", self.acq_epoch);
        tags.insert("acq_stage", 3); // Tracking
        tags.insert("acq_searched", 1);
        tags.insert("acq_noncoherent", 0);
        tags
    }

    fn tag_block(&self, mut block: SampleBlock) -> SampleBlock {
        block.tags = self.make_tags();
        block
    }

    /// Re-emit buffered acquisition samples as tagged blocks.
    fn emit_buffer(&mut self) -> Vec<SampleBlock> {
        if self.acq_buffer.is_empty() {
            return Vec::new();
        }

        let samples = std::mem::take(&mut self.acq_buffer);
        let block = SampleBlock::new(samples, self.acq_buffer_chip_start)
            .with_sample_rate_hz(self.acq_buffer_sample_rate);
        vec![self.tag_block(block)]
    }
}

impl PipelineProcessor for CdmaSlidingCorrelator {
    fn process_block(&mut self, block: SampleBlock) -> Vec<SampleBlock> {
        if self.locked {
            return vec![self.tag_block(block)];
        }

        // ---- ACQUISITION: buffer incoming samples ----
        if self.acq_buffer.is_empty() {
            self.acq_buffer_chip_start = block.chip_start;
            self.acq_buffer_sample_rate = block.sample_rate_hz;
        }
        self.acq_buffer.extend_from_slice(&block.samples);

        if self.acq_buffer.len() >= self.pn_period_samples {
            let (best_offset, snr) = self.search_all_offsets();

            if snr >= self.snr_threshold_db {
                self.locked = true;
                self.acq_epoch += 1;
                self.peak_sample_offset = best_offset * self.oversample;

                // DEBUG: per-segment noncoherent breakdown at both offsets
                for &check_offset in &[best_offset, 4104usize] {
                    let buf_len = self.acq_buffer.len();
                    let sample_offset = check_offset * self.oversample;
                    let segment_chips = 128usize;
                    let segment_samples = segment_chips * self.oversample;
                    let dwell_samples = self.acq_dwell_chips * self.oversample;
                    let num_segments = dwell_samples / segment_samples;
                    let mut seg_vals = Vec::new();
                    let mut tot_e = 0.0f32;
                    for seg in 0..num_segments.min(8) {
                        let seg_start = seg * segment_samples;
                        let mut si = 0.0f32;
                        let mut sq = 0.0f32;
                        let mut se = 0.0f32;
                        for k in 0..segment_samples {
                            let bi = (sample_offset + seg_start + k) % buf_len;
                            let pi = (seg_start + k) % self.pn_period_samples;
                            let s = self.acq_buffer[bi];
                            let p = self.pn_template[pi];
                            si += s.re * p.re + s.im * p.im;
                            sq += s.im * p.re - s.re * p.im;
                            se += s.re * s.re + s.im * s.im;
                        }
                        let mag2 = si * si + sq * sq;
                        tot_e += se;
                        seg_vals.push((mag2, se));
                    }
                    eprintln!(
                        "  DEBUG offset={}: first 8 segs |Z|²={:?} energies={:?}",
                        check_offset,
                        seg_vals.iter().map(|(m, _)| format!("{:.0}", m)).collect::<Vec<_>>(),
                        seg_vals.iter().map(|(_, e)| format!("{:.0}", e)).collect::<Vec<_>>()
                    );
                }

                eprintln!(
                    "cdma_sliding_correlator: LOCKED offset={} peak_sample={} snr={:.1}dB epoch={}",
                    best_offset,
                    self.peak_sample_offset,
                    snr,
                    self.acq_epoch
                );

                // Re-emit buffered samples with acquisition tags
                return self.emit_buffer();
            }

            // Not found — slide buffer forward by half a PN period
            let half = self.pn_period_samples / 2;
            self.acq_buffer.drain(..half);
            self.acq_buffer_chip_start += half;
        }

        Vec::new()
    }

    fn flush(&mut self) -> Vec<SampleBlock> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use num_complex::Complex32;

    use super::CdmaSlidingCorrelator;
    use crate::{
        phy::spread::PnSequence,
        phy::walsh::WalshGenerator,
        receiver::pipelined::{PipelineProcessor, SampleBlock},
        sdr::{cdma2000_baseband_filter_taps_f64, fir::ComplexFir32},
    };

    /// Generate a TX signal matching the IS-95 forward link convention
    /// (PN_I - j·PN_Q), pulse-shaped through the RRC filter.
    fn generate_tx_signal(sample_rate: u32, num_symbols: usize) -> Vec<Complex32> {
        let oversample = (sample_rate / 1_228_800).max(1) as usize;
        let taps = cdma2000_baseband_filter_taps_f64();
        let mut fir = ComplexFir32::new(&taps);
        let walsh0 = WalshGenerator::generate_matrix::<64>()[0];
        let mut pn = PnSequence::new_repeat(0, 32768, oversample.saturating_sub(1));
        let mut tx = Vec::new();
        for _ in 0..num_symbols {
            for chip in 0..64usize {
                let d = walsh0[chip] as f32;
                for _ in 0..oversample.max(1) {
                    let s = pn.generate_iq();
                    let raw_i = d * s.re;
                    let raw_q = d * (-s.im); // IS-95: PN_I - j·PN_Q
                    tx.push(fir.process_sample(Complex32::new(raw_i, raw_q)));
                }
            }
        }
        tx
    }

    #[test]
    fn test_cdma_sliding_correlator_locks_and_passes_through() {
        let sample_rate = 1_228_800u32;
        let mut p = CdmaSlidingCorrelator::new(sample_rate)
            .with_snr_threshold_db(2.0)
            .with_acq_dwell_chips(1024);

        let tx = generate_tx_signal(sample_rate, 600);

        let out = p.process_block(
            SampleBlock::new(tx.clone(), 0).with_sample_rate_hz(sample_rate as f64),
        );
        assert!(!out.is_empty(), "should produce output blocks after locking");
        assert!(
            out.iter().all(|b| b.tags.get("acq_locked") == Some(&1)),
            "blocks should be tagged as locked"
        );
        // Output should contain raw oversampled samples (pass-through)
        let total_samples: usize = out.iter().map(|b| b.len()).sum();
        assert!(total_samples > 0, "should pass through samples");
    }

    #[test]
    fn test_cdma_sliding_correlator_4x_oversampled() {
        let sample_rate = 1_228_800u32 * 4;
        let mut p = CdmaSlidingCorrelator::new(sample_rate)
            .with_snr_threshold_db(2.0)
            .with_acq_dwell_chips(1024);

        let tx = generate_tx_signal(sample_rate, 600);

        let out = p.process_block(
            SampleBlock::new(tx, 0).with_sample_rate_hz(sample_rate as f64),
        );
        assert!(!out.is_empty(), "should produce output blocks after locking");
        assert!(
            out.iter().all(|b| b.tags.get("acq_locked") == Some(&1)),
            "blocks should be tagged as locked"
        );
    }
}
