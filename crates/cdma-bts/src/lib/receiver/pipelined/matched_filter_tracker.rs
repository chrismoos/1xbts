use std::sync::Arc;

use crate::receiver::pipelined::{PipelineProcessor, SampleBlock, build_matched_pn_reference};
use log::{debug, trace};
use num_complex::Complex32;
use rustfft::{Fft, FftPlanner};

//pub const FFT_LENGTH: usize = 65536;
//pub const BUFFER_PADDING: usize = 32767;
//pub const BUFFER_SAMPLES_NEW: usize = 32769;

pub struct MatchedFilterTracker {
    fft_pn: Vec<Complex32>,
    fft_planner: Arc<dyn Fft<f32>>,
    fft_planner_inverse: Arc<dyn Fft<f32>>,
    fft_scratch: Vec<Complex32>,
    sample: usize,
    buffer: Vec<Complex32>,
    state: State,

    pn_seq_filtered: Vec<Complex32>,

    lock_phase: usize,
    lock_misses: usize,
    lock_hits: usize,

    fft_length: usize,
    buffer_samples: usize,
    oversample: usize,
    output_samples: Vec<Complex32>,

    lock_first_output_block: bool,
    lock_chip_start: usize,
    pending_lock_lost_tag: bool,
    speculative_blocks: Vec<SampleBlock>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum State {
    Searching,
    CoarseTracking,
    FineTracking,
}

impl MatchedFilterTracker {
    pub fn new(oversample: usize) -> MatchedFilterTracker {
        let pn_seq_filtered = build_matched_pn_reference(32768 * oversample, oversample, 2);

        let mut pn_seq_reversed = pn_seq_filtered.clone();
        pn_seq_reversed.reverse();
        for x in &mut pn_seq_reversed {
            *x = x.conj();
        }

        let fft_length = 32768 * 2 * oversample;

        let mut fft_pn = vec![];
        for x in 0..fft_length {
            if x < fft_length / 2 {
                fft_pn.push(pn_seq_reversed[x]);
            } else {
                fft_pn.push(Complex32::new(0.0, 0.0));
            }
        }

        let planner = FftPlanner::new().plan_fft_forward(fft_length);
        planner.process(&mut fft_pn);

        MatchedFilterTracker {
            sample: 0,
            state: State::Searching,
            fft_planner: planner,
            fft_planner_inverse: FftPlanner::new().plan_fft_inverse(fft_length),
            fft_pn,
            fft_scratch: vec![Complex32::new(0.0, 0.0); fft_length],
            pn_seq_filtered,
            lock_phase: 0,
            lock_hits: 0,
            lock_misses: 0,
            fft_length,
            oversample,
            buffer_samples: (fft_length / 2) + 1,
            output_samples: vec![],
            lock_first_output_block: false,
            lock_chip_start: 0,
            pending_lock_lost_tag: false,
            speculative_blocks: Vec::new(),
            buffer: vec![Complex32::new(0.0, 0.0); (fft_length / 2) - 1],
        }
    }
}

impl PipelineProcessor for MatchedFilterTracker {
    fn process_block(&mut self, block: super::SampleBlock) -> Vec<super::SampleBlock> {
        self.buffer.extend(&block.samples);
        let mut produced_blocks = Vec::new();

        while self.buffer.len() >= self.fft_length {
            for s in &self.buffer {
                if !s.is_finite() {
                    panic!("sample not finite!");
                }
            }

            trace!(
                "processing block {}, sample={}",
                self.sample / self.buffer_samples,
                self.sample
            );

            let mut signal_fft = self.buffer[0..self.fft_length].to_vec();
            self.fft_planner
                .process_with_scratch(&mut signal_fft, &mut self.fft_scratch);

            let mut multiplied = (0..self.fft_length)
                .map(|x| signal_fft[x] * self.fft_pn[x])
                .collect::<Vec<_>>();

            self.fft_planner_inverse.process(&mut multiplied);
            for v in &mut multiplied {
                *v /= self.fft_length as f32;
            }

            let filter_len = self.fft_length / 2;
            let overlap = filter_len - 1;
            let phase_period = filter_len;

            let powers: Vec<f32> = multiplied[overlap..self.fft_length]
                .iter()
                .map(|x| x.norm_sqr())
                .collect();

            let mut top_powers = powers.iter().enumerate().collect::<Vec<_>>();
            top_powers.sort_by(|a, b| b.1.total_cmp(a.1));

            let mid = powers.len() / 2;
            let mut mid_powers = powers.clone();
            mid_powers.select_nth_unstable_by(mid, |a, b| a.partial_cmp(b).unwrap());
            let median = mid_powers[mid];

            // Global peak for Searching / CoarseTracking.
            let mut global_max = overlap;
            for x in overlap..multiplied.len() {
                if multiplied[x].norm_sqr() > multiplied[global_max].norm_sqr() {
                    global_max = x;
                }
            }

            let global_peak_power = multiplied[global_max].norm_sqr();
            let global_power = global_peak_power / median;
            // Track PN phase at the start of the current buffer block.
            // For overlap-save correlation index rel = (idx - overlap), the
            // block-start phase is -rel mod period.
            let global_rel = (global_max - overlap) % phase_period;
            let global_phase = (phase_period - global_rel) % phase_period;

            let acquire_threshold = 12.0;
            let coarse_track_threshold = 8.0;
            let track_threshold = 4.0;
            let phase_threshold = 8usize;
            let coarse_confirm_threshold = 5usize;
            let fine_miss_threshold = 5usize;
            let track_search_half_window = 8usize;

            let log_top3 = |phase_period: usize,
                            overlap: usize,
                            top_powers: &Vec<(usize, &f32)>,
                            median: f32| {
                for x in 0..3.min(top_powers.len()) {
                    trace!(
                        "top (pn_phase={}) {} -> {}, idx: {}",
                        (phase_period - top_powers[x].0 % phase_period) % phase_period,
                        top_powers[x].0 + overlap,
                        top_powers[x].1 / median,
                        top_powers[x].0
                    );
                }
            };

            match self.state {
                State::Searching => {
                    if global_power > acquire_threshold {
                        log_top3(phase_period, overlap, &top_powers, median);
                        debug!(
                            "max @ {} (or pn_phase == {}) -> {}, median={}, power={}",
                            global_max, global_phase, global_peak_power, median, global_power
                        );

                        debug!("found candidate, Searching -> Coarse");
                        self.state = State::CoarseTracking;
                        self.lock_phase = global_phase;
                        self.lock_hits = 0;
                        self.lock_misses = 0;

                        // Speculative despread of the acquisition block.
                        self.sample += self.buffer_samples;
                        self.lock_chip_start = self.sample.saturating_sub(self.buffer_samples);
                        let samples = self
                            .buffer
                            .drain(0..self.buffer_samples)
                            .enumerate()
                            .map(|(idx, val)| {
                                self.pn_seq_filtered
                                    [(global_phase + idx) % (32768 * self.oversample)]
                                    .conj()
                                    * val
                            })
                            .collect::<Vec<_>>();
                        self.output_samples.extend(samples);
                        while self.output_samples.len() >= 64 * self.oversample {
                            let mut out_block = SampleBlock::new(
                                self.output_samples
                                    .drain(0..64 * self.oversample)
                                    .collect::<Vec<_>>(),
                                self.lock_chip_start,
                            )
                            .with_sample_rate_hz(block.sample_rate_hz);
                            out_block.tags.insert("pilot_phase", global_phase as i64);
                            if self.pending_lock_lost_tag {
                                out_block.tags.insert("upstream_lock_lost", 1);
                                self.pending_lock_lost_tag = false;
                            }
                            self.speculative_blocks.push(out_block);
                            self.lock_chip_start += 64 * self.oversample;
                        }
                    } else {
                        self.sample += self.buffer_samples;
                        self.buffer.drain(0..self.buffer_samples);
                    }
                }

                State::CoarseTracking => {
                    let expected_phase = (self.lock_phase + self.buffer_samples) % phase_period;
                    let coarse_search_half_window = 16usize;

                    // Local search around expected_phase (not global max).
                    let mut local_best_idx = overlap;
                    let mut local_best_power = 0.0f32;
                    let mut found_local = false;

                    for rel in 0..=(2 * coarse_search_half_window) {
                        let offset = rel as isize - coarse_search_half_window as isize;
                        let cand_phase = (expected_phase as isize + offset)
                            .rem_euclid(phase_period as isize)
                            as usize;
                        // If phase is defined at block start, the expected peak
                        // relative index is simply -phase mod period.
                        let cand_rel = (phase_period - cand_phase) % phase_period;
                        let cand_idx = overlap + cand_rel;

                        if cand_idx < multiplied.len() {
                            let p = multiplied[cand_idx].norm_sqr();
                            if !found_local || p > local_best_power {
                                found_local = true;
                                local_best_power = p;
                                local_best_idx = cand_idx;
                            }
                        }
                    }

                    let (measured_phase, measured_power) = if found_local {
                        (
                            (phase_period - ((local_best_idx - overlap) % phase_period))
                                % phase_period,
                            local_best_power / median,
                        )
                    } else {
                        (expected_phase, 0.0)
                    };

                    let distance = phase_distance(expected_phase, measured_phase, phase_period);

                    if found_local
                        && distance <= phase_threshold
                        && measured_power > coarse_track_threshold
                    {
                        self.lock_hits += 1;
                        self.lock_phase = measured_phase;
                        if self.lock_misses > 0 {
                            self.lock_misses -= 1;
                        }
                        trace!(
                            "coarse: hit @ pn_phase={}, power={:.1}, expected={}, hits={}, misses={}",
                            measured_phase,
                            measured_power,
                            expected_phase,
                            self.lock_hits,
                            self.lock_misses
                        );
                    } else {
                        self.lock_misses += 1;
                        if self.lock_hits > 0 {
                            self.lock_hits -= 1;
                        }
                        // Coast: advance predicted phase
                        self.lock_phase = expected_phase;
                        trace!(
                            "coarse: miss, local_power={:.1}, distance={}, expected={}, hits={}, misses={}",
                            measured_power,
                            distance,
                            expected_phase,
                            self.lock_hits,
                            self.lock_misses
                        );
                    }

                    if self.lock_misses > coarse_confirm_threshold {
                        debug!("lost coarse lock");
                        self.state = State::Searching;
                        self.speculative_blocks.clear();
                        self.output_samples.clear();
                        self.lock_chip_start = 0;

                        self.sample += self.buffer_samples;
                        self.buffer.drain(0..self.buffer_samples);
                    } else {
                        // Speculative despread: use measured_phase if on-phase,
                        // expected_phase otherwise.
                        let coarse_despread_phase = if found_local && distance <= phase_threshold {
                            measured_phase
                        } else {
                            expected_phase
                        };
                        self.sample += self.buffer_samples;
                        let samples = self
                            .buffer
                            .drain(0..self.buffer_samples)
                            .enumerate()
                            .map(|(idx, val)| {
                                self.pn_seq_filtered
                                    [(coarse_despread_phase + idx) % (32768 * self.oversample)]
                                    .conj()
                                    * val
                            })
                            .collect::<Vec<_>>();
                        self.output_samples.extend(samples);
                        while self.output_samples.len() >= 64 * self.oversample {
                            let mut out_block = SampleBlock::new(
                                self.output_samples
                                    .drain(0..64 * self.oversample)
                                    .collect::<Vec<_>>(),
                                self.lock_chip_start,
                            )
                            .with_sample_rate_hz(block.sample_rate_hz);
                            out_block
                                .tags
                                .insert("pilot_phase", coarse_despread_phase as i64);
                            if self.pending_lock_lost_tag {
                                out_block.tags.insert("upstream_lock_lost", 1);
                                self.pending_lock_lost_tag = false;
                            }
                            self.speculative_blocks.push(out_block);
                            self.lock_chip_start += 64 * self.oversample;
                        }

                        if self.lock_hits > coarse_confirm_threshold {
                            self.lock_hits = 0;
                            self.lock_misses = 0;
                            self.state = State::FineTracking;
                            self.lock_first_output_block = false;
                            debug!("promoted phase {} -> fine tracking", self.lock_phase);
                            // Flush speculative blocks downstream.
                            produced_blocks.append(&mut self.speculative_blocks);
                        }
                    }
                }

                State::FineTracking => {
                    let phase_period = 32768 * self.oversample;
                    let expected_phase = (self.lock_phase + self.buffer_samples) % phase_period;
                    // Search locally around expected_phase using the correct phase->idx mapping:
                    // If phase is defined at block start:
                    // rel_idx = (-phase) mod phase_period
                    // idx = overlap + rel_idx
                    let mut local_best_idx = overlap;
                    let mut local_best_power = 0.0f32;
                    let mut found_local = false;

                    for rel in 0..=(2 * track_search_half_window) {
                        let offset = rel as isize - track_search_half_window as isize;
                        let cand_phase = (expected_phase as isize + offset)
                            .rem_euclid(phase_period as isize)
                            as usize;

                        let cand_rel = (phase_period - cand_phase) % phase_period;
                        let cand_idx = overlap + cand_rel;

                        if cand_idx < multiplied.len() {
                            let p = multiplied[cand_idx].norm_sqr();
                            if !found_local || p > local_best_power {
                                found_local = true;
                                local_best_power = p;
                                local_best_idx = cand_idx;
                            }
                        }
                    }

                    let (measured_phase, measured_power) = if found_local {
                        (
                            (phase_period - ((local_best_idx - overlap) % phase_period))
                                % phase_period,
                            local_best_power / median,
                        )
                    } else {
                        (expected_phase, 0.0)
                    };

                    let distance = phase_distance(expected_phase, measured_phase, phase_period);

                    // Three cases:
                    // 1) strong + on-phase  -> update lock_phase
                    // 2) weak  + on-phase   -> coast, but DO NOT count as miss
                    // 3) off-phase / absent -> real miss
                    let strong_on_phase = found_local
                        && distance <= phase_threshold
                        && measured_power > track_threshold;

                    let weak_but_on_phase = found_local
                        && distance <= phase_threshold
                        && measured_power <= track_threshold;

                    let real_miss = !found_local || distance > phase_threshold;

                    let despread_phase = if strong_on_phase {
                        measured_phase
                    } else {
                        expected_phase
                    };

                    if strong_on_phase {
                        trace!(
                            "tracked local peak @ {} (pn_phase == {}) -> {}, median={}, power={}",
                            local_best_idx,
                            measured_phase,
                            local_best_power,
                            median,
                            measured_power
                        );

                        self.lock_phase = measured_phase;
                        self.lock_hits += 1;
                        if self.lock_misses > 0 {
                            self.lock_misses -= 1;
                        }
                    } else if weak_but_on_phase {
                        trace!(
                            "tracking coast: expected_phase={}, measured_phase={}, measured_power={}",
                            expected_phase, measured_phase, measured_power
                        );

                        // Coast: advance predicted phase, no miss counted
                        self.lock_phase = expected_phase;
                        self.lock_hits += 1;
                        if self.lock_misses > 0 {
                            self.lock_misses -= 1;
                        }
                    } else if real_miss {
                        trace!(
                            "tracking miss: expected_phase={}, measured_phase={}, measured_power={}",
                            expected_phase, measured_phase, measured_power
                        );

                        // Coast: advance predicted phase
                        self.lock_phase = expected_phase;
                        self.lock_misses += 1;
                        if self.lock_hits > 0 {
                            self.lock_hits -= 1;
                        }
                    }

                    if self.lock_misses > fine_miss_threshold {
                        println!("lost fine lock");
                        self.state = State::Searching;
                        self.output_samples.clear();
                        self.speculative_blocks.clear();
                        self.lock_first_output_block = false;
                        self.lock_chip_start = 0;

                        self.sample += self.buffer_samples;
                        self.buffer.drain(0..self.buffer_samples);
                        self.pending_lock_lost_tag = true;
                    } else {
                        self.sample += self.buffer_samples;
                        let drained_block_chip_start =
                            self.sample.saturating_sub(self.buffer_samples);

                        // Defer alignment until we have a strong measurement.
                        // On weak/miss blocks before alignment, skip despreading
                        // to avoid corrupting downstream with a bad phase.
                        if self.lock_first_output_block && !strong_on_phase {
                            self.buffer.drain(0..self.buffer_samples);
                        } else {
                            if self.lock_first_output_block {
                                // First strong block after lock: start chip timeline
                                // at the actual drained block position.
                                self.lock_chip_start = drained_block_chip_start;
                                self.lock_first_output_block = false;
                            }

                            let samples = self
                                .buffer
                                .drain(0..self.buffer_samples)
                                .enumerate()
                                .map(|(idx, val)| {
                                    self.pn_seq_filtered
                                        [(despread_phase + idx) % (32768 * self.oversample)]
                                        .conj()
                                        * val
                                })
                                .collect::<Vec<_>>();
                            self.output_samples.extend(samples);
                        }

                        while self.output_samples.len() >= 64 * self.oversample {
                            let mut out_block = SampleBlock::new(
                                self.output_samples
                                    .drain(0..64 * self.oversample)
                                    .collect::<Vec<_>>(),
                                self.lock_chip_start,
                            )
                            .with_sample_rate_hz(block.sample_rate_hz);
                            out_block.tags.insert("pilot_phase", despread_phase as i64);
                            if self.pending_lock_lost_tag {
                                out_block.tags.insert("upstream_lock_lost", 1);
                                self.pending_lock_lost_tag = false;
                            }
                            produced_blocks.push(out_block);
                            self.lock_chip_start += 64 * self.oversample;
                        }
                    }
                }
            }
        }

        produced_blocks
    }
}

fn phase_distance(a: usize, b: usize, period: usize) -> usize {
    let d = (a as isize - b as isize).rem_euclid(period as isize) as usize;
    d.min(period - d)
}
