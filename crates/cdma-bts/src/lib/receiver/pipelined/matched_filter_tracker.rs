use std::sync::Arc;

use log::{debug, trace};
use num_complex::Complex32;
use rustfft::{Fft, FftPlanner};

use cdma_common::consts::SR1_CHIP_RATE_HZ;

use crate::receiver::pipelined::{PipelineProcessor, SampleBlock, build_matched_pn_reference};
use crate::sdr::cdma2000_baseband_filter_taps_f64;

const PN_CHIPS: usize = 32768;

const ACQ_SEGMENT_CHIPS: usize = 256;

const ACQ_SEGMENTS: usize = 16;

/// Half-null spacing (64 FFT bins = 2400 Hz) avoids coherent-search blind spots.
const ACQ_CFO_BIN_STEP: isize = 64;
const ACQ_CFO_HALF_COUNT: isize = 2;

/// A 64-chip span puts the first coherent null at 19.2 kHz, beyond acquisition CFOs.
const TRACK_COHERENT_CHIPS: usize = 64;

const DEFAULT_ACQUIRE_THRESHOLD: f32 = 12.0;

const TRACK_STEP_CHIPS: usize = 1024;

const TRACK_SEARCH_HALF_SAMPLES: usize = 8;

/// Hysteresis prevents noise-driven timing shifts from jittering frame boundaries.
const TRACK_SLEW_MARGIN: f32 = 1.25;

/// Noise in `|corr|² / energy` is near the oversample factor.
const DEFAULT_TRACK_THRESHOLD: f32 = 25.0;

const CONFIRM_HITS: usize = 3;

const MISS_LIMIT: usize = 32;

const OUTPUT_BLOCK_CHIPS: usize = 64;

const REFERENCE_FILTER_PASSES: usize = 2;

/// Filter delay between the reference start and chip zero. PN epochs must use chip zero.
fn reference_group_delay() -> usize {
    (cdma2000_baseband_filter_taps_f64().len() - 1) * REFERENCE_FILTER_PASSES / 2
}

fn hypothesis_order() -> impl Iterator<Item = isize> {
    (0..=ACQ_CFO_HALF_COUNT).flat_map(|k| if k == 0 { vec![0isize] } else { vec![k, -k] })
}

pub struct MatchedFilterTracker {
    oversample: usize,
    period: usize,
    pn_seq_filtered: Vec<Complex32>,
    fft_pn_conj: Vec<Complex32>,
    fft_fwd: Arc<dyn Fft<f32>>,
    fft_inv: Arc<dyn Fft<f32>>,
    fft_scratch: Vec<Complex32>,
    search_buf: Vec<Complex32>,
    search_power: Vec<f32>,
    segment_spectra: Vec<Vec<Complex32>>,
    search_chips: Vec<Complex32>,

    acquire_threshold: f32,
    track_threshold: f32,
    track_step: usize,
    track_coherent: usize,

    state: State,
    lock_phase: usize,
    refine_pending: bool,
    reference_delay: usize,
    output_phase: usize,
    hits: usize,
    misses: usize,

    buffer: Vec<Complex32>,
    output_samples: Vec<Complex32>,
    output_chip_start: usize,
    speculative_blocks: Vec<SampleBlock>,
    pending_lock_lost_tag: bool,
    consumed: usize,
    /// Input sample index = output sample index - slew_samples.
    slew_samples: i64,
    produced: Vec<SampleBlock>,
}

#[derive(Debug, PartialEq, Eq)]
pub enum State {
    Searching,
    Confirming,
    Tracking,
}

impl MatchedFilterTracker {
    pub fn new(oversample: usize) -> MatchedFilterTracker {
        let oversample = oversample.max(1);
        let period = PN_CHIPS * oversample;
        let pn_seq_filtered = build_matched_pn_reference(period, oversample, 2);

        let mut planner = FftPlanner::new();
        let fft_fwd = planner.plan_fft_forward(PN_CHIPS);
        let fft_inv = planner.plan_fft_inverse(PN_CHIPS);

        let mut fft_pn_conj: Vec<Complex32> = pn_seq_filtered
            .chunks_exact(oversample)
            .map(|c| c.iter().sum())
            .collect();
        fft_fwd.process(&mut fft_pn_conj);
        for v in &mut fft_pn_conj {
            *v = v.conj();
        }

        let scratch_len = fft_fwd
            .get_inplace_scratch_len()
            .max(fft_inv.get_inplace_scratch_len());

        MatchedFilterTracker {
            oversample,
            period,
            pn_seq_filtered,
            fft_pn_conj,
            fft_fwd,
            fft_inv,
            fft_scratch: vec![Complex32::new(0.0, 0.0); scratch_len],
            search_buf: vec![Complex32::new(0.0, 0.0); PN_CHIPS],
            search_power: vec![0.0; PN_CHIPS],
            segment_spectra: vec![vec![Complex32::new(0.0, 0.0); PN_CHIPS]; ACQ_SEGMENTS],
            search_chips: vec![Complex32::new(0.0, 0.0); PN_CHIPS],
            acquire_threshold: DEFAULT_ACQUIRE_THRESHOLD,
            track_threshold: DEFAULT_TRACK_THRESHOLD,
            track_step: TRACK_STEP_CHIPS * oversample,
            track_coherent: TRACK_COHERENT_CHIPS * oversample,
            state: State::Searching,
            lock_phase: 0,
            refine_pending: false,
            reference_delay: reference_group_delay(),
            output_phase: 0,
            hits: 0,
            misses: 0,
            buffer: Vec::new(),
            output_samples: Vec::new(),
            output_chip_start: 0,
            speculative_blocks: Vec::new(),
            pending_lock_lost_tag: false,
            consumed: 0,
            slew_samples: 0,
            produced: Vec::new(),
        }
    }

    pub fn with_coarse_track_threshold(mut self, threshold: f32) -> Self {
        self.track_threshold = threshold;
        self
    }

    pub fn with_acquire_threshold(mut self, threshold: f32) -> Self {
        self.acquire_threshold = threshold;
        self
    }

    fn search(&mut self) -> (usize, f32, f32) {
        for (chip, samples) in self
            .search_chips
            .iter_mut()
            .zip(self.buffer[..self.period].chunks_exact(self.oversample))
        {
            *chip = samples.iter().sum();
        }

        for segment in 0..ACQ_SEGMENTS {
            let start = segment * ACQ_SEGMENT_CHIPS;
            let end = start + ACQ_SEGMENT_CHIPS;
            // Keep the segment in place inside a zero window so every
            // segment's peak lands on the same lag.
            let spectrum = &mut self.segment_spectra[segment];
            spectrum.fill(Complex32::new(0.0, 0.0));
            spectrum[start..end].copy_from_slice(&self.search_chips[start..end]);
            self.fft_fwd
                .process_with_scratch(spectrum, &mut self.fft_scratch);
        }

        let mut best = (0usize, 0.0f32, 0.0f32);
        for h in hypothesis_order() {
            let shift = h * ACQ_CFO_BIN_STEP;
            self.search_power.fill(0.0);
            for spectrum in &self.segment_spectra {
                for (i, (buf, r)) in self
                    .search_buf
                    .iter_mut()
                    .zip(self.fft_pn_conj.iter())
                    .enumerate()
                {
                    let src = (i as isize - shift).rem_euclid(PN_CHIPS as isize) as usize;
                    *buf = spectrum[src] * *r;
                }
                self.fft_inv
                    .process_with_scratch(&mut self.search_buf, &mut self.fft_scratch);
                for (p, v) in self.search_power.iter_mut().zip(self.search_buf.iter()) {
                    *p += v.norm_sqr();
                }
            }

            let mut peak_idx = 0usize;
            let mut peak = 0.0f32;
            for (i, p) in self.search_power.iter().enumerate() {
                if *p > peak {
                    peak = *p;
                    peak_idx = i;
                }
            }
            let mid = PN_CHIPS / 2;
            let mut sorted = self.search_power.clone();
            sorted.select_nth_unstable_by(mid, |a, b| a.total_cmp(b));
            let normalized = peak / sorted[mid].max(1e-20);

            // The window start sits `period - peak_lag` chips into the
            // sequence, and the winning profile shifted the signal up by
            // `shift` bins, so the signal itself sits that far low.
            if normalized > best.1 {
                let phase = ((PN_CHIPS - peak_idx) % PN_CHIPS) * self.oversample;
                let cfo = -2.0 * std::f32::consts::PI * shift as f32 / PN_CHIPS as f32;
                best = (phase, normalized, cfo);
            }
            if normalized > self.acquire_threshold {
                break;
            }
        }
        best
    }

    fn correlate_at(&self, phase: usize) -> f32 {
        let mut acc = 0.0f32;
        let mut energy = 0.0f32;
        for (b, block) in self.buffer[..self.track_step]
            .chunks_exact(self.track_coherent)
            .enumerate()
        {
            let base = phase + b * self.track_coherent;
            let mut corr = Complex32::new(0.0, 0.0);
            for (i, s) in block.iter().enumerate() {
                let pn = self.pn_seq_filtered[(base + i) % self.period];
                corr += *s * pn.conj();
                energy += s.norm_sqr();
            }
            acc += corr.norm_sqr();
        }
        if energy <= 1e-20 {
            return 0.0;
        }
        acc / energy
    }

    fn refine(&self, predicted: usize) -> (usize, f32) {
        let mut best_phase = predicted;
        let mut best = 0.0f32;
        for offset in -(TRACK_SEARCH_HALF_SAMPLES as isize)..=(TRACK_SEARCH_HALF_SAMPLES as isize) {
            let phase = (predicted as isize + offset).rem_euclid(self.period as isize) as usize;
            let metric = self.correlate_at(phase);
            if metric > best {
                best = metric;
                best_phase = phase;
            }
        }
        (best_phase, best)
    }

    fn track(&self, predicted: usize) -> (usize, f32) {
        let prompt = self.correlate_at(predicted);
        let early = self.correlate_at((predicted + self.period - 1) % self.period);
        let late = self.correlate_at((predicted + 1) % self.period);
        if early > prompt * TRACK_SLEW_MARGIN && early >= late {
            ((predicted + self.period - 1) % self.period, early)
        } else if late > prompt * TRACK_SLEW_MARGIN {
            ((predicted + 1) % self.period, late)
        } else {
            (predicted, prompt)
        }
    }

    fn despread_step(&mut self, phase: usize, count: usize, slew: i64, sample_rate_hz: f64) {
        let samples: Vec<Complex32> = self
            .buffer
            .drain(..count)
            .enumerate()
            .map(|(i, v)| self.pn_seq_filtered[(phase + i) % self.period].conj() * v)
            .collect();
        self.consumed += count;
        // Positive phase steps repeat a sample, negative steps skip one.
        if slew > 0 {
            self.output_samples.push(samples[0]);
        }
        self.output_samples
            .extend(samples.into_iter().skip(usize::from(slew < 0)));

        let block_len = OUTPUT_BLOCK_CHIPS * self.oversample;
        while self.output_samples.len() >= block_len {
            let mut out = SampleBlock::new(
                self.output_samples.drain(..block_len).collect::<Vec<_>>(),
                self.output_chip_start,
            )
            .with_sample_rate_hz(sample_rate_hz);
            out.tags.insert("pilot_phase", self.output_phase as i64);
            out.tags.insert("pilot_slew_samples", self.slew_samples);
            self.output_phase = (self.output_phase + block_len) % self.period;
            if self.pending_lock_lost_tag {
                out.tags.insert("upstream_lock_lost", 1);
                self.pending_lock_lost_tag = false;
            }
            if self.state == State::Tracking {
                self.produced.push(out);
            } else {
                self.speculative_blocks.push(out);
            }
            self.output_chip_start += block_len;
        }
    }

    fn drop_lock(&mut self, lost_after_confirm: bool) {
        self.state = State::Searching;
        self.hits = 0;
        self.misses = 0;
        self.output_samples.clear();
        self.speculative_blocks.clear();
        self.output_chip_start = 0;
        self.slew_samples = 0;
        if lost_after_confirm {
            self.pending_lock_lost_tag = true;
        }
    }
}

fn slew_of(from: usize, to: usize, period: usize) -> i64 {
    match (to + period - from) % period {
        1 => 1,
        d if d == period - 1 => -1,
        _ => 0,
    }
}

impl PipelineProcessor for MatchedFilterTracker {
    fn process_block(&mut self, block: super::SampleBlock) -> Vec<super::SampleBlock> {
        self.buffer.extend(&block.samples);
        self.produced.clear();

        loop {
            match self.state {
                State::Searching => {
                    if self.buffer.len() < self.period {
                        break;
                    }
                    let (phase, peak, cfo_rad_per_chip) = self.search();
                    if peak > self.acquire_threshold {
                        debug!(
                            "acquired pilot at pn_phase={} (peak/median={:.1}, carrier {:+.0} Hz), confirming",
                            phase,
                            peak,
                            cfo_rad_per_chip * SR1_CHIP_RATE_HZ as f32
                                / (2.0 * std::f32::consts::PI)
                        );
                        self.state = State::Confirming;
                        self.lock_phase = phase;
                        self.refine_pending = true;
                        self.hits = 0;
                        self.misses = 0;
                        self.output_chip_start = self.consumed;
                        self.slew_samples = 0;
                    } else {
                        trace!("search: no pilot (peak/median={peak:.1})");
                        let drop = self.period.min(self.buffer.len());
                        self.buffer.drain(..drop);
                        self.consumed += drop;
                    }
                }

                State::Confirming | State::Tracking => {
                    if self.buffer.len() < self.track_step {
                        break;
                    }
                    let refining = self.refine_pending;
                    let (phase, metric) = if refining {
                        self.refine_pending = false;
                        self.refine(self.lock_phase)
                    } else {
                        self.track(self.lock_phase)
                    };
                    // Refinement corrects chip quantization regardless of the hit threshold.
                    let hit = metric > self.track_threshold;
                    let mut slew = 0;
                    if hit || refining {
                        if !refining {
                            slew = slew_of(self.lock_phase, phase, self.period);
                            self.slew_samples += slew;
                        }
                        self.lock_phase = phase;
                    }
                    if refining {
                        // Start output at a PN roll so downstream chip and frame counts remain aligned.
                        let ahead = (self.reference_delay + self.period
                            - self.lock_phase % self.period)
                            % self.period;
                        if ahead <= self.period / 2 {
                            let skip = ahead.min(self.buffer.len());
                            self.buffer.drain(..skip);
                            self.consumed += skip;
                            self.lock_phase = (self.lock_phase + skip) % self.period;
                            self.output_chip_start = self.consumed + self.period;
                        } else {
                            let backfill = self.period - ahead;
                            self.output_chip_start = self.consumed + self.period - backfill;
                            self.output_samples
                                .extend(std::iter::repeat_n(Complex32::new(0.0, 0.0), backfill));
                        }
                        self.output_phase = 0;
                        if self.buffer.len() < self.track_step {
                            break;
                        }
                    }
                    if hit {
                        self.hits += 1;
                        self.misses = self.misses.saturating_sub(1);
                    } else {
                        self.misses += 1;
                        self.hits = self.hits.saturating_sub(1);
                    }
                    trace!(
                        "track: phase={} metric={:.1} hit={} hits={} misses={}",
                        self.lock_phase, metric, hit, self.hits, self.misses
                    );

                    if self.misses > MISS_LIMIT {
                        debug!("lost pilot lock");
                        let confirmed = self.state == State::Tracking;
                        self.drop_lock(confirmed);
                        continue;
                    }

                    let despread_phase = self.lock_phase;
                    self.despread_step(despread_phase, self.track_step, slew, block.sample_rate_hz);
                    self.lock_phase = (self.lock_phase + self.track_step) % self.period;

                    if self.state == State::Confirming && self.hits >= CONFIRM_HITS {
                        debug!("confirmed pilot at pn_phase={}", self.lock_phase);
                        self.state = State::Tracking;
                        let mut released = std::mem::take(&mut self.speculative_blocks);
                        self.produced.append(&mut released);
                    }
                }
            }
        }

        std::mem::take(&mut self.produced)
    }
}
