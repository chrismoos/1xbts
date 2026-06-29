use std::{f32::consts::PI, sync::Arc};

use num_complex::Complex32;
use rustfft::{Fft, FftPlanner};

use crate::{
    phy::spread::PnSequence,
    sdr::{cdma2000_baseband_filter_taps_f64, fir::ComplexFir32},
};

use super::{PipelineProcessor, SampleBlock};

/// FFT-based PN acquisition/searcher.
///
/// Buffers one acquisition window, performs circular correlation in frequency
/// domain against precomputed conj(FFT(PN)), and tags output blocks with lock
/// metadata for downstream tracking/despread stages.
pub struct AcquisitionFftProcessor {
    sample_rate: f32,
    oversample: usize,
    fft_len: usize,
    fft_fwd: Arc<dyn Fft<f32>>,
    fft_inv: Arc<dyn Fft<f32>>,
    ref_spectrum_conj: Vec<Complex32>,
    frequency_hypotheses_hz: Vec<f32>,
    snr_threshold_db: f32,
    /// Buffered filtered samples waiting to fill a full FFT window.
    buffer: Vec<Complex32>,
    /// Chip index of the first sample in `buffer`.
    buffer_chip_start: usize,
    buffer_sample_rate_hz: f64,
    acq_epoch: i64,
    stage: AcquisitionStage,
    verify_blocks_required: usize,
    verify_locked_count: usize,
    tracking_search_interval_blocks: usize,
    blocks_since_tracking_search: usize,
    last_locked: bool,
    last_peak_idx: usize,
    last_snr_db: f32,
    last_best_freq_hz: f32,
    noncoherent: Option<NonCoherentConfig>,
}

struct NonCoherentConfig {
    segment_len: usize,
    fft_fwd: Arc<dyn Fft<f32>>,
    fft_inv: Arc<dyn Fft<f32>>,
    ref_spectrum_conj_by_segment: Vec<Vec<Complex32>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum AcquisitionStage {
    SearchingCoarse,
    RefiningFine,
    Verifying,
    Tracking,
}

impl AcquisitionFftProcessor {
    pub fn new(sample_rate: u32) -> Self {
        let oversample = (sample_rate / 1_228_800).max(1) as usize;
        Self::new_with_window_chips(sample_rate, 32768, oversample)
    }

    pub fn new_with_window_chips(sample_rate: u32, window_chips: usize, oversample: usize) -> Self {
        let fft_len = window_chips * oversample.max(1);
        let mut planner = FftPlanner::<f32>::new();
        let fft_fwd = planner.plan_fft_forward(fft_len);
        let fft_inv = planner.plan_fft_inverse(fft_len);
        let taps = cdma2000_baseband_filter_taps_f64();
        let mut reference_time = Self::build_reference_time(fft_len, oversample, &taps);
        fft_fwd.process(&mut reference_time);
        let ref_spectrum_conj = reference_time
            .into_iter()
            .map(|v| v.conj())
            .collect::<Vec<_>>();

        Self {
            sample_rate: sample_rate as f32,
            oversample: oversample.max(1),
            fft_len,
            fft_fwd,
            fft_inv,
            ref_spectrum_conj,
            frequency_hypotheses_hz: vec![0.0],
            snr_threshold_db: 7.0,
            buffer: Vec::new(),
            buffer_chip_start: 0,
            buffer_sample_rate_hz: 0.0,
            acq_epoch: 0,
            stage: AcquisitionStage::SearchingCoarse,
            verify_blocks_required: 2,
            verify_locked_count: 0,
            tracking_search_interval_blocks: 16,
            blocks_since_tracking_search: 0,
            last_locked: false,
            last_peak_idx: 0,
            last_snr_db: -120.0,
            last_best_freq_hz: 0.0,
            noncoherent: None,
        }
    }

    fn build_reference_time(fft_len: usize, oversample: usize, taps: &[f64]) -> Vec<Complex32> {
        // try without matched i/q
        //let mut pn = PnSequence::new_repeat(0, 32768, oversample.saturating_sub(1));
        //(0..fft_len).map(|_| pn.generate_iq()).collect::<Vec<_>>()

        let mut ref_matched = ComplexFir32::new(taps);
        let mut pn = PnSequence::new_repeat(0, 32768, oversample.saturating_sub(1));

        // IS-95 forward link spreading convention is (PN_I - j*PN_Q).
        // generate_iq() returns (PN_I + j*PN_Q), so we negate Q to match.
        (0..fft_len)
            .map(|_| {
                let s = pn.generate_iq();
                ref_matched.process_sample(Complex32::new(s.re, -s.im))
            })
            .collect::<Vec<_>>()
    }

    pub fn with_noncoherent_segment_chips(mut self, segment_chips: usize) -> Self {
        let segment_len = segment_chips.max(1) * self.oversample.max(1);
        assert!(
            self.fft_len % segment_len == 0,
            "noncoherent segment length must divide acquisition window"
        );

        let mut planner = FftPlanner::<f32>::new();
        let fft_fwd = planner.plan_fft_forward(segment_len);
        let fft_inv = planner.plan_fft_inverse(segment_len);
        let taps = cdma2000_baseband_filter_taps_f64();
        let reference_time = Self::build_reference_time(self.fft_len, self.oversample, &taps);

        let mut ref_spectrum_conj_by_segment = Vec::new();
        for segment in reference_time.chunks_exact(segment_len) {
            let mut seg = segment.to_vec();
            fft_fwd.process(&mut seg);
            ref_spectrum_conj_by_segment.push(seg.into_iter().map(|v| v.conj()).collect());
        }

        self.noncoherent = Some(NonCoherentConfig {
            segment_len,
            fft_fwd,
            fft_inv,
            ref_spectrum_conj_by_segment,
        });
        self
    }

    pub fn with_frequency_hypotheses_hz(mut self, hypotheses_hz: Vec<f32>) -> Self {
        if !hypotheses_hz.is_empty() {
            self.frequency_hypotheses_hz = hypotheses_hz;
        }
        self
    }

    pub fn with_snr_threshold_db(mut self, snr_threshold_db: f32) -> Self {
        self.snr_threshold_db = snr_threshold_db;
        self
    }

    pub fn with_verify_blocks(mut self, verify_blocks: usize) -> Self {
        self.verify_blocks_required = verify_blocks.max(1);
        self
    }

    pub fn with_tracking_search_interval_blocks(mut self, interval_blocks: usize) -> Self {
        self.tracking_search_interval_blocks = interval_blocks.max(1);
        self
    }

    fn correlate_block_coherent(&self, input: &[Complex32]) -> (usize, f32, f32, f32) {
        let mut best_peak_idx = 0usize;
        let mut best_peak_val = f32::MIN;
        let mut best_noise_floor = 1e-12f32;
        let mut best_freq_hz = 0.0f32;

        for freq_hz in &self.frequency_hypotheses_hz {
            let mut spectrum = input
                .iter()
                .enumerate()
                .map(|(n, s)| {
                    if *freq_hz == 0.0 {
                        *s
                    } else {
                        let phase = -2.0 * PI * *freq_hz * (n as f32) / self.sample_rate;
                        let rot = Complex32::new(phase.cos(), phase.sin());
                        *s * rot
                    }
                })
                .collect::<Vec<_>>();

            self.fft_fwd.process(&mut spectrum);
            for (i, v) in spectrum.iter_mut().enumerate() {
                *v *= self.ref_spectrum_conj[i];
            }
            self.fft_inv.process(&mut spectrum);

            let mut peak_idx = 0usize;
            let mut peak_val = f32::MIN;
            let mut total = 0.0f32;
            for (i, v) in spectrum.iter().enumerate() {
                let mag2 = v.norm_sqr();
                total += mag2;
                if mag2 > peak_val {
                    peak_val = mag2;
                    peak_idx = i;
                }
            }

            let mean = (total / spectrum.len() as f32).max(1e-12);
            let noise_floor = ((total - peak_val)
                / (spectrum.len().saturating_sub(1).max(1) as f32))
                .max(1e-12)
                .min(mean * 2.0);

            if peak_val > best_peak_val {
                best_peak_val = peak_val;
                best_peak_idx = peak_idx;
                best_noise_floor = noise_floor;
                best_freq_hz = *freq_hz;
            }
        }

        (best_peak_idx, best_peak_val, best_noise_floor, best_freq_hz)
    }

    fn correlate_block_noncoherent(
        &self,
        input: &[Complex32],
        nc: &NonCoherentConfig,
    ) -> (usize, f32, f32, f32) {
        let mut best_peak_idx = 0usize;
        let mut best_peak_val = f32::MIN;
        let mut best_noise_floor = 1e-12f32;
        let mut best_freq_hz = 0.0f32;

        for freq_hz in &self.frequency_hypotheses_hz {
            let mut accum_power = vec![0.0f32; nc.segment_len];
            for (segment_idx, segment) in input.chunks_exact(nc.segment_len).enumerate() {
                let start = segment_idx * nc.segment_len;
                let mut spectrum = segment
                    .iter()
                    .enumerate()
                    .map(|(k, s)| {
                        if *freq_hz == 0.0 {
                            *s
                        } else {
                            let n = start + k;
                            let phase = -2.0 * PI * *freq_hz * (n as f32) / self.sample_rate;
                            let rot = Complex32::new(phase.cos(), phase.sin());
                            *s * rot
                        }
                    })
                    .collect::<Vec<_>>();

                nc.fft_fwd.process(&mut spectrum);
                for (i, v) in spectrum.iter_mut().enumerate() {
                    *v *= nc.ref_spectrum_conj_by_segment[segment_idx][i];
                }
                nc.fft_inv.process(&mut spectrum);

                for (i, v) in spectrum.iter().enumerate() {
                    accum_power[i] += v.norm_sqr();
                }
            }

            let mut peak_idx = 0usize;
            let mut peak_val = f32::MIN;
            let mut total = 0.0f32;
            for (i, p) in accum_power.iter().enumerate() {
                total += *p;
                if *p > peak_val {
                    peak_val = *p;
                    peak_idx = i;
                }
            }

            let mean = (total / accum_power.len() as f32).max(1e-12);
            let noise_floor = ((total - peak_val)
                / (accum_power.len().saturating_sub(1).max(1) as f32))
                .max(1e-12)
                .min(mean * 2.0);

            if peak_val > best_peak_val {
                best_peak_val = peak_val;
                best_peak_idx = peak_idx;
                best_noise_floor = noise_floor;
                best_freq_hz = *freq_hz;
            }
        }

        (best_peak_idx, best_peak_val, best_noise_floor, best_freq_hz)
    }

    fn correlate_block(&self, input: &[Complex32]) -> (usize, f32, f32, f32) {
        if let Some(nc) = &self.noncoherent {
            self.correlate_block_noncoherent(input, nc)
        } else {
            self.correlate_block_coherent(input)
        }
    }

    fn correlate_block_mode(
        &self,
        input: &[Complex32],
        mode: AcquisitionStage,
    ) -> (usize, f32, f32, f32) {
        match mode {
            AcquisitionStage::SearchingCoarse => self.correlate_block(input),
            AcquisitionStage::RefiningFine
            | AcquisitionStage::Verifying
            | AcquisitionStage::Tracking => self.correlate_block_coherent(input),
        }
    }

    fn stage_code(stage: AcquisitionStage) -> i64 {
        match stage {
            AcquisitionStage::SearchingCoarse => 0,
            AcquisitionStage::RefiningFine => 1,
            AcquisitionStage::Verifying => 2,
            AcquisitionStage::Tracking => 3,
        }
    }

    /// Process one full FFT window worth of filtered samples.
    /// Returns a SampleBlock with the same samples plus acquisition tags.
    fn process_window(&mut self, samples: Vec<Complex32>, chip_start: usize) -> SampleBlock {
        let should_search = match self.stage {
            AcquisitionStage::SearchingCoarse
            | AcquisitionStage::RefiningFine
            | AcquisitionStage::Verifying => true,
            AcquisitionStage::Tracking => {
                self.blocks_since_tracking_search += 1;
                self.blocks_since_tracking_search >= self.tracking_search_interval_blocks
            }
        };

        let (peak_idx, snr_db, best_freq_hz, locked) = if should_search {
            let (peak_idx, peak_val, noise_floor, best_freq_hz) =
                self.correlate_block_mode(&samples, self.stage);
            let snr_db = 10.0 * (peak_val / noise_floor.max(1e-12)).max(1e-12).log10();
            if self.last_locked {
                //println!("new snr: {} -> {}", snr_db, self.snr_threshold_db);
            }
            let locked = snr_db >= self.snr_threshold_db;
            self.last_snr_db = snr_db;
            self.last_best_freq_hz = best_freq_hz;
            self.blocks_since_tracking_search = 0;
            (peak_idx, snr_db, best_freq_hz, locked)
        } else {
            (
                self.last_peak_idx,
                self.last_snr_db,
                self.last_best_freq_hz,
                self.last_locked,
            )
        };

        let peak_changed = peak_idx.abs_diff(self.last_peak_idx) > self.oversample;
        let new_epoch = match self.stage {
            AcquisitionStage::Tracking => locked && !self.last_locked, // only on re-lock after loss
            _ => locked && (!self.last_locked || peak_changed),
        };
        if new_epoch {
            self.acq_epoch += 1;
            eprintln!(
                "acquisition_fft_lock epoch={} peak_sample={} peak_chip={} timing_phase={} snr_db={:.2} cfo_hz={:.1} stage={:?}",
                self.acq_epoch,
                peak_idx,
                peak_idx / self.oversample.max(1),
                peak_idx % self.oversample.max(1),
                snr_db,
                best_freq_hz,
                self.stage
            );
        }
        self.last_locked = locked;
        if self.stage != AcquisitionStage::Tracking {
            self.last_peak_idx = peak_idx;
        }

        if should_search {
            self.stage = match self.stage {
                AcquisitionStage::SearchingCoarse => {
                    //println!("searching coarse: locked {:?}", locked);
                    if locked {
                        AcquisitionStage::RefiningFine
                    } else {
                        AcquisitionStage::SearchingCoarse
                    }
                }
                AcquisitionStage::RefiningFine => {
                    if locked {
                        self.verify_locked_count = 0;
                        AcquisitionStage::Verifying
                    } else {
                        self.verify_locked_count = 0;
                        AcquisitionStage::SearchingCoarse
                    }
                }
                AcquisitionStage::Verifying => {
                    if locked {
                        self.verify_locked_count += 1;
                        if self.verify_locked_count >= self.verify_blocks_required {
                            self.verify_locked_count = 0;
                            AcquisitionStage::Tracking
                        } else {
                            AcquisitionStage::Verifying
                        }
                    } else {
                        self.verify_locked_count = 0;
                        AcquisitionStage::SearchingCoarse
                    }
                }
                AcquisitionStage::Tracking => {
                    if locked {
                        println!("we are now tracking");
                        AcquisitionStage::Tracking
                    } else {
                        AcquisitionStage::SearchingCoarse
                    }
                }
            };
        }

        let mut block =
            SampleBlock::new(samples, chip_start).with_sample_rate_hz(self.buffer_sample_rate_hz);
        block.tags.insert("acq_locked", if locked { 1 } else { 0 });
        block.tags.insert("acq_peak_sample", peak_idx as i64);
        block
            .tags
            .insert("acq_peak_chip", (peak_idx / self.oversample.max(1)) as i64);
        block.tags.insert(
            "acq_timing_phase",
            (peak_idx % self.oversample.max(1)) as i64,
        );
        block
            .tags
            .insert("acq_snr_db_x100", (snr_db * 100.0) as i64);
        block.tags.insert("acq_cfo_hz", best_freq_hz as i64);
        block.tags.insert("acq_epoch", self.acq_epoch);
        block.tags.insert("acq_stage", Self::stage_code(self.stage));
        block
            .tags
            .insert("acq_searched", if should_search { 1 } else { 0 });
        block.tags.insert(
            "acq_noncoherent",
            if self.noncoherent.is_some() { 1 } else { 0 },
        );
        block
    }
}

impl PipelineProcessor for AcquisitionFftProcessor {
    fn process_block(&mut self, block: SampleBlock) -> Vec<SampleBlock> {
        if self.buffer.is_empty() {
            self.buffer_chip_start = block.chip_start;
            self.buffer_sample_rate_hz = block.sample_rate_hz;
        }
        self.buffer.extend_from_slice(&block.samples);

        let mut output = Vec::new();

        let window_chips = self.fft_len / self.oversample.max(1);
        // optional but recommended:
        debug_assert_eq!(self.fft_len % self.oversample.max(1), 0);

        while self.buffer.len() >= self.fft_len {
            let rest = self.buffer.split_off(self.fft_len);
            let window = std::mem::replace(&mut self.buffer, rest);
            let chip_start = self.buffer_chip_start;
            self.buffer_chip_start += window_chips; // ✅ advance in chips
            output.push(self.process_window(window, chip_start));
        }

        output
    }

    fn flush(&mut self) -> Vec<SampleBlock> {
        if self.buffer.len() == self.fft_len {
            let window = std::mem::take(&mut self.buffer);
            let chip_start = self.buffer_chip_start;
            return vec![self.process_window(window, chip_start)];
        }
        self.buffer.clear();
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use num_complex::Complex32;

    use super::AcquisitionFftProcessor;
    use crate::{
        phy::spread::PnSequence,
        receiver::pipelined::{PipelineProcessor, SampleBlock},
    };

    #[test]
    fn test_acquisition_fft_processor_emits_lock_tags_for_matched_pn() {
        let mut p = AcquisitionFftProcessor::new_with_window_chips(1_228_800, 1024, 1)
            .with_snr_threshold_db(0.0);
        let mut pn = PnSequence::new_repeat(0, 32768, 0);
        let samples: Vec<Complex32> = (0..1024).map(|_| pn.generate_iq()).collect();
        let block = SampleBlock::new(samples, 0);

        let out = p.process_block(block);
        assert_eq!(1, out.len());
        assert_eq!(1024, out[0].len());
        assert_eq!(
            Some(&1),
            out[0].tags.get("acq_locked"),
            "expected acquisition lock on matched PN"
        );
        assert!(out[0].tags.contains_key("acq_peak_sample"));
        assert!(out[0].tags.contains_key("acq_timing_phase"));
        assert!(out[0].tags.contains_key("acq_epoch"));
    }

    #[test]
    fn test_acquisition_fft_processor_buffers_until_full_window() {
        let mut p = AcquisitionFftProcessor::new_with_window_chips(1_228_800, 256, 1)
            .with_snr_threshold_db(0.0);
        let block1 = SampleBlock::new(vec![Complex32::new(0.0, 0.0); 128], 0);
        let block2 = SampleBlock::new(vec![Complex32::new(0.0, 0.0); 128], 128);

        assert!(p.process_block(block1).is_empty());
        let out = p.process_block(block2);
        assert_eq!(1, out.len());
        assert_eq!(256, out[0].len());
    }

    #[test]
    fn test_acquisition_fft_processor_supports_frequency_hypotheses() {
        let mut p = AcquisitionFftProcessor::new_with_window_chips(1_228_800, 256, 1)
            .with_frequency_hypotheses_hz(vec![-200.0, 0.0, 200.0])
            .with_snr_threshold_db(0.0);
        let block = SampleBlock::new(vec![Complex32::new(0.0, 0.0); 256], 0);

        let out = p.process_block(block);
        assert_eq!(1, out.len());
        assert!(out[0].tags.contains_key("acq_cfo_hz"));
    }

    #[test]
    fn test_acquisition_fft_processor_noncoherent_mode_tags_output() {
        let mut p = AcquisitionFftProcessor::new_with_window_chips(1_228_800, 1024, 1)
            .with_noncoherent_segment_chips(256)
            .with_snr_threshold_db(0.0);
        let mut pn = PnSequence::new_repeat(0, 32768, 0);
        let samples: Vec<Complex32> = (0..1024).map(|_| pn.generate_iq()).collect();
        let block = SampleBlock::new(samples, 0);

        let out = p.process_block(block);
        assert_eq!(1, out.len());
        assert_eq!(Some(&1), out[0].tags.get("acq_noncoherent"));
        assert_eq!(Some(&1), out[0].tags.get("acq_locked"));
    }
}
