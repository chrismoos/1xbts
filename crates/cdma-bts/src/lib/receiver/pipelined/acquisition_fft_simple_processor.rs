use std::sync::Arc;

use num_complex::Complex32;
use rustfft::{Fft, FftPlanner};

use crate::{
    phy::spread::PnSequence,
    sdr::{cdma2000_baseband_filter_taps_f64, fir::ComplexFir32},
};

use super::{PipelineProcessor, SampleBlock};

/// Minimal FFT-based acquisition:
/// R = IFFT(FFT(r) * conj(FFT(c)))
///
/// This variant intentionally omits CFO hypotheses, noncoherent accumulation,
/// and coarse/fine/tracking state machines.
pub struct AcquisitionFftSimpleProcessor {
    oversample: usize,
    fft_len: usize,
    fft_fwd: Arc<dyn Fft<f32>>,
    fft_inv: Arc<dyn Fft<f32>>,
    ref_spectrum_conj: Vec<Complex32>,
    snr_threshold_db: f32,
    buffer: Vec<Complex32>,
    buffer_chip_start: usize,
    buffer_sample_rate_hz: f64,
    acq_epoch: i64,
    last_locked: bool,
    last_peak_idx: usize,
}

impl AcquisitionFftSimpleProcessor {
    /// Create an acquisition processor sized for the input sample rate.
    pub fn new(sample_rate: u32) -> Self {
        let oversample = (sample_rate / 1_228_800).max(1) as usize;
        Self::new_with_window_chips(sample_rate, 32768, oversample)
    }

    /// Create an acquisition processor with an explicit FFT window size.
    pub fn new_with_window_chips(
        _sample_rate: u32,
        window_chips: usize,
        oversample: usize,
    ) -> Self {
        let fft_len = window_chips * oversample.max(1);
        let mut planner = FftPlanner::<f32>::new();
        let fft_fwd = planner.plan_fft_forward(fft_len);
        let fft_inv = planner.plan_fft_inverse(fft_len);

        let taps = cdma2000_baseband_filter_taps_f64();
        let mut ref_time = Self::build_reference_time(fft_len, oversample.max(1), &taps);
        fft_fwd.process(&mut ref_time);
        let ref_spectrum_conj = ref_time.into_iter().map(|v| v.conj()).collect::<Vec<_>>();

        Self {
            oversample: oversample.max(1),
            fft_len,
            fft_fwd,
            fft_inv,
            ref_spectrum_conj,
            snr_threshold_db: 7.0,
            buffer: Vec::new(),
            buffer_chip_start: 0,
            buffer_sample_rate_hz: 0.0,
            acq_epoch: 0,
            last_locked: false,
            last_peak_idx: 0,
        }
    }

    /// Set the acquisition threshold in dB over the noise-floor estimate.
    pub fn with_snr_threshold_db(mut self, snr_threshold_db: f32) -> Self {
        self.snr_threshold_db = snr_threshold_db;
        self
    }

    fn build_reference_time(fft_len: usize, oversample: usize, taps: &[f64]) -> Vec<Complex32> {
        let mut ref_matched = ComplexFir32::new(taps);
        let mut pn = PnSequence::new_repeat(0, 32768, oversample.saturating_sub(1));

        // Forward-link PN convention: PN_I - jPN_Q.
        (0..fft_len)
            .map(|_| {
                let s = pn.generate_iq();
                ref_matched.process_sample(Complex32::new(s.re, -s.im))
            })
            .collect::<Vec<_>>()
    }

    fn correlate_block(&self, input: &[Complex32]) -> (usize, f32, f32) {
        let mut spectrum = input.to_vec();
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
        let noise_floor =
            ((total - peak_val) / (spectrum.len().saturating_sub(1).max(1) as f32)).max(1e-12);

        (peak_idx, peak_val, noise_floor)
    }

    fn process_window(&mut self, samples: Vec<Complex32>, chip_start: usize) -> SampleBlock {
        let (peak_idx, peak_val, noise_floor) = self.correlate_block(&samples);
        let snr_db = 10.0 * (peak_val / noise_floor.max(1e-12)).max(1e-12).log10();
        let locked = snr_db >= self.snr_threshold_db;

        let peak_changed = peak_idx.abs_diff(self.last_peak_idx) > self.oversample;
        if locked && (!self.last_locked || peak_changed) {
            self.acq_epoch += 1;
        }
        self.last_locked = locked;
        self.last_peak_idx = peak_idx;

        let mut block =
            SampleBlock::new(samples, chip_start).with_sample_rate_hz(self.buffer_sample_rate_hz);
        block.tags.insert("acq_locked", if locked { 1 } else { 0 });
        block.tags.insert("acq_peak_sample", peak_idx as i64);
        block
            .tags
            .insert("acq_peak_chip", (peak_idx / self.oversample) as i64);
        block
            .tags
            .insert("acq_timing_phase", (peak_idx % self.oversample) as i64);
        block
            .tags
            .insert("acq_snr_db_x100", (snr_db * 100.0) as i64);
        block.tags.insert("acq_cfo_hz", 0);
        block.tags.insert("acq_epoch", self.acq_epoch);
        block.tags.insert("acq_stage", 0);
        block.tags.insert("acq_searched", 1);
        block.tags.insert("acq_noncoherent", 0);
        block
    }
}

impl PipelineProcessor for AcquisitionFftSimpleProcessor {
    fn process_block(&mut self, block: SampleBlock) -> Vec<SampleBlock> {
        if self.buffer.is_empty() {
            self.buffer_chip_start = block.chip_start;
            self.buffer_sample_rate_hz = block.sample_rate_hz;
        }
        self.buffer.extend_from_slice(&block.samples);

        let mut output = Vec::new();
        while self.buffer.len() >= self.fft_len {
            let rest = self.buffer.split_off(self.fft_len);
            let window = std::mem::replace(&mut self.buffer, rest);
            let chip_start = self.buffer_chip_start;
            self.buffer_chip_start += self.fft_len;
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

    use super::AcquisitionFftSimpleProcessor;
    use crate::{
        phy::spread::PnSequence,
        receiver::pipelined::{PipelineProcessor, SampleBlock},
    };

    #[test]
    fn test_acquisition_fft_simple_processor_emits_lock_tags_for_matched_pn() {
        let mut p = AcquisitionFftSimpleProcessor::new_with_window_chips(1_228_800, 1024, 1)
            .with_snr_threshold_db(0.0);
        let mut pn = PnSequence::new_repeat(0, 32768, 0);
        let samples: Vec<Complex32> = (0..1024).map(|_| pn.generate_iq()).collect();
        let out = p.process_block(SampleBlock::new(samples, 0));

        assert_eq!(1, out.len());
        assert_eq!(1024, out[0].len());
        assert_eq!(Some(&1), out[0].tags.get("acq_locked"));
        assert_eq!(Some(&0), out[0].tags.get("acq_noncoherent"));
        assert_eq!(Some(&0), out[0].tags.get("acq_cfo_hz"));
    }
}
