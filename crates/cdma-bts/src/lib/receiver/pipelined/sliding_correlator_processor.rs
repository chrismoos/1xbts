use std::collections::{HashMap, VecDeque};

use num_complex::Complex32;

use crate::{
    phy::spread::PnSequence,
    sdr::{cdma2000_baseband_filter_taps_f64, fir::ComplexFir32},
};

use super::{PipelineProcessor, SampleBlock};

/// Single-stage matched-filter + sliding PN correlator + despreader processor.
///
/// This implementation is self-contained and does not use receiver::Correlator.
pub struct SlidingCorrelatorProcessor {
    oversample: usize,
    pn_template: Vec<Complex32>,
    pn_len: usize,
    acq_index: usize,
    track_index: usize,
    locked: bool,
    lock_threshold: f32,
    unlock_threshold: f32,
    unlock_misses_required: usize,
    unlock_misses: usize,
    lock_eval_stride: usize,
    processed: usize,
    history: VecDeque<Complex32>,
    history_len: usize,
    history_energy: f32,
    warmup_samples_after_lock: usize,
    matched: ComplexFir32,
    sample_counter: usize,
    decimation_phase_energy: Vec<f32>,
    decimation_phase_alpha: f32,
    decimation_selected_phase: usize,
    acq_epoch: i64,
    output_buffer: Vec<Complex32>,
    output_tags: HashMap<&'static str, i64>,
    output_chip_start: usize,
    output_sample_rate_hz: f64,
}

impl SlidingCorrelatorProcessor {
    pub fn new(sample_rate: u32) -> Self {
        let oversample = (sample_rate / 1_228_800).max(1) as usize;
        let pn_len = 32768 * oversample;
        let mut pn = PnSequence::new_repeat(0, 32768, oversample.saturating_sub(1));
        let pn_template = (0..pn_len).map(|_| pn.generate_iq()).collect::<Vec<_>>();

        let taps = cdma2000_baseband_filter_taps_f64();

        Self {
            oversample,
            pn_template,
            pn_len,
            acq_index: 0,
            track_index: 0,
            locked: false,
            lock_threshold: 0.06,
            unlock_threshold: 0.01,
            unlock_misses_required: 16,
            unlock_misses: 0,
            lock_eval_stride: 64,
            processed: 0,
            history: VecDeque::new(),
            history_len: 128 * oversample,
            history_energy: 0.0,
            warmup_samples_after_lock: 0,
            matched: ComplexFir32::new(&taps),
            sample_counter: 0,
            decimation_phase_energy: vec![0.0; oversample.max(1)],
            decimation_phase_alpha: 0.01,
            decimation_selected_phase: 0,
            acq_epoch: 0,
            output_buffer: Vec::new(),
            output_tags: HashMap::new(),
            output_chip_start: 0,
            output_sample_rate_hz: 0.0,
        }
    }

    pub fn with_lock_threshold(mut self, threshold: f32) -> Self {
        self.lock_threshold = threshold;
        self
    }

    fn corr_metric_for_newest_index(&self, newest_pn_idx: usize) -> f32 {
        if self.history.len() < self.history_len {
            return 0.0;
        }

        let mut sum = Complex32::new(0.0, 0.0);
        let start = if newest_pn_idx + 1 >= self.history_len {
            newest_pn_idx + 1 - self.history_len
        } else {
            self.pn_len + newest_pn_idx + 1 - self.history_len
        };

        for (k, s) in self.history.iter().enumerate() {
            let pn = self.pn_template[(start + k) % self.pn_len];
            sum.re += s.re * pn.re;
            sum.im += s.im * pn.im;
        }

        let denom = self.history_energy.max(1e-12).sqrt();
        (sum.norm() / denom).max(0.0)
    }

    fn push_history(&mut self, sample: Complex32) {
        self.history.push_back(sample);
        self.history_energy += sample.norm_sqr();
        if self.history.len() > self.history_len
            && let Some(old) = self.history.pop_front()
        {
            self.history_energy = (self.history_energy - old.norm_sqr()).max(0.0);
        }
    }

    fn emit_aligned_blocks(&mut self) -> Vec<SampleBlock> {
        while !self.output_buffer.is_empty() && (self.output_chip_start % 64 != 0) {
            self.output_buffer.remove(0);
            self.output_chip_start = self.output_chip_start.saturating_add(1);
        }

        let mut out = Vec::new();
        while self.output_buffer.len() >= 64 {
            let chunk = self.output_buffer.drain(..64).collect::<Vec<_>>();
            let chip_start = self.output_chip_start;
            self.output_chip_start = self.output_chip_start.saturating_add(64);

            let mut out_block =
                SampleBlock::new(chunk, chip_start).with_sample_rate_hz(self.output_sample_rate_hz);
            out_block.tags = self.output_tags.clone();
            out_block
                .tags
                .insert("global_chip_start", chip_start as i64);
            out_block
                .tags
                .insert("walsh_phase", (chip_start % 64) as i64);
            out.push(out_block);
        }

        out
    }
}

impl PipelineProcessor for SlidingCorrelatorProcessor {
    fn process_block(&mut self, block: SampleBlock) -> Vec<SampleBlock> {
        for (idx, sample) in block.samples.iter().enumerate() {
            let matched = self.matched.process_sample(*sample);

            self.push_history(matched);
            self.processed = self.processed.saturating_add(1);

            if !self.locked {
                if self.history.len() == self.history_len {
                    let metric = self.corr_metric_for_newest_index(self.acq_index);
                    if metric >= self.lock_threshold {
                        self.locked = true;
                        self.track_index = (self.acq_index + 1) % self.pn_len;
                        self.warmup_samples_after_lock = self.pn_len;
                        self.unlock_misses = 0;
                        self.acq_epoch += 1;
                    }
                }
                self.acq_index = (self.acq_index + 1) % self.pn_len;
                continue;
            }

            if self.processed % self.lock_eval_stride == 0 {
                let newest = if self.track_index == 0 {
                    self.pn_len - 1
                } else {
                    self.track_index - 1
                };
                let metric = self.corr_metric_for_newest_index(newest);
                if metric < self.unlock_threshold {
                    self.unlock_misses += 1;
                } else {
                    self.unlock_misses = 0;
                }
                if self.unlock_misses >= self.unlock_misses_required {
                    self.locked = false;
                    self.unlock_misses = 0;
                    self.history.clear();
                    self.history_energy = 0.0;
                    continue;
                }
            }

            let pn = self.pn_template[self.track_index];
            self.track_index = (self.track_index + 1) % self.pn_len;
            let despread = matched * pn;

            if self.warmup_samples_after_lock > 0 {
                self.warmup_samples_after_lock -= 1;
                continue;
            }

            let phase = self.sample_counter % self.oversample.max(1);
            self.sample_counter = self.sample_counter.saturating_add(1);

            let energy = despread.norm_sqr();
            let prev = self.decimation_phase_energy[phase];
            self.decimation_phase_energy[phase] =
                (1.0 - self.decimation_phase_alpha) * prev + self.decimation_phase_alpha * energy;

            if self.sample_counter >= self.oversample.max(1) * 64
                && self.sample_counter % (self.oversample.max(1) * 32) == 0
            {
                let mut best_idx = 0usize;
                let mut best_val = f32::MIN;
                for (i, v) in self.decimation_phase_energy.iter().enumerate() {
                    if *v > best_val {
                        best_val = *v;
                        best_idx = i;
                    }
                }
                self.decimation_selected_phase = best_idx;
            }

            if phase == self.decimation_selected_phase {
                if self.output_buffer.is_empty() {
                    self.output_sample_rate_hz = if block.sample_rate_hz > 0.0 {
                        block.sample_rate_hz / self.oversample.max(1) as f64
                    } else {
                        0.0
                    };
                    self.output_tags = block.tags.clone();
                    self.output_tags.insert("acq_locked", 1);
                    self.output_tags
                        .insert("acq_peak_sample", self.track_index as i64);
                    self.output_tags.insert(
                        "acq_peak_chip",
                        ((self.sample_counter - 1) / self.oversample) as i64,
                    );
                    self.output_tags
                        .insert("acq_timing_phase", self.decimation_selected_phase as i64);
                    self.output_tags.insert("acq_snr_db_x100", 0);
                    self.output_tags.insert("acq_cfo_hz", 0);
                    self.output_tags.insert("acq_epoch", self.acq_epoch);
                    self.output_tags.insert("acq_stage", 0);
                    self.output_tags.insert("acq_searched", 1);
                    self.output_tags.insert("acq_noncoherent", 0);

                    let global_sample = block.chip_start.saturating_add(idx);
                    self.output_chip_start = global_sample / self.oversample.max(1);
                }
                self.output_buffer.push(despread);
            }
        }

        self.emit_aligned_blocks()
    }

    fn flush(&mut self) -> Vec<SampleBlock> {
        self.output_buffer.clear();
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use num_complex::Complex32;

    use super::SlidingCorrelatorProcessor;
    use crate::{
        phy::spread::PnSequence,
        phy::walsh::WalshGenerator,
        receiver::pipelined::{PipelineProcessor, SampleBlock},
    };

    #[test]
    fn test_sliding_correlator_processor_emits_aligned_blocks() {
        let sample_rate = 1_228_800u32;
        let mut p = SlidingCorrelatorProcessor::new(sample_rate).with_lock_threshold(0.0);

        let walsh0 = WalshGenerator::generate_matrix::<64>()[0];
        let mut pn = PnSequence::new_repeat(0, 32768, 0);
        let mut tx = Vec::new();
        for _ in 0..1100usize {
            for chip in 0..64usize {
                let d = walsh0[chip] as f32;
                let pn_chip = pn.generate_iq();
                tx.push(Complex32::new(d, 0.0) * Complex32::new(pn_chip.re, -pn_chip.im));
            }
        }

        let out = p.process_block(SampleBlock::new(tx, 0).with_sample_rate_hz(sample_rate as f64));
        assert!(!out.is_empty());
        assert!(out.iter().all(|b| b.len() == 64));
    }
}
