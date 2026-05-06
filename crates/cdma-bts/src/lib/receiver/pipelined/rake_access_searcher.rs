use std::collections::VecDeque;
use std::sync::Arc;

use log::{debug, info, trace};
use num_complex::Complex32;
use rustfft::{Fft, FftPlanner};

use crate::phy::coding::long_code::LongCodeGenerator;
use crate::receiver::pipelined::{
    PipelineProcessor, PipelineProcessorShared, SampleBlock, build_fft_search_pn_samples,
    build_oqpsk_pn_samples,
};

const FILTER_TAPS: usize = 48;

#[derive(Debug, Clone)]
pub struct RakeAcquisitionConfig {
    pub oversample: usize,
    pub pn_coherent_chips: usize,
    pub pn_noncoherent_windows: usize,
    pub pn_keep_top_n: usize,
    pub pn_peak_suppress_samples: usize,
    pub pn_persistence_required: u32,
    pub lc_search_half_span_chips: i32,
    pub lc_integrate_chips: usize,
    pub lc_best_over_second_min: f32,
    pub preamble_coh_norm_min: f32,
    pub lc_noncoherent_segments: usize,
    pub preamble_hits_required: u32,
    pub fine_offset_search_samples: i32,
    pub max_active_candidates: usize,
    pub joint_search_interval_blocks: u64,
    pub joint_lc_half_span: i32,
    pub joint_snr_min: f32,
    pub stage2_interval_blocks: u64,
}

impl RakeAcquisitionConfig {
    pub fn default_4x() -> Self {
        Self {
            oversample: 4,
            pn_coherent_chips: 256,
            pn_noncoherent_windows: 16,
            pn_keep_top_n: 32,
            pn_peak_suppress_samples: 4,
            pn_persistence_required: 1,
            lc_search_half_span_chips: 128,
            lc_integrate_chips: 256,
            lc_best_over_second_min: 1.2,
            preamble_coh_norm_min: 0.15,
            lc_noncoherent_segments: 4,
            preamble_hits_required: 2,
            fine_offset_search_samples: 2,
            max_active_candidates: 8,
            joint_search_interval_blocks: 8,
            joint_lc_half_span: 128,
            joint_snr_min: 8.0,
            stage2_interval_blocks: 1,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CandidateState {
    Stage1,
    Stage2SearchingLc,
    VerifyingPreamble,
    PreambleDetected,
    Rejected,
}

#[derive(Debug, Clone)]
struct PnPeak {
    delay_samples: i32,
    metric: f32,
}

#[derive(Debug, Clone)]
struct Candidate {
    id: u64,
    state: CandidateState,
    coarse_delay_samples: i32,
    fine_delay_samples: i32,
    stage1_metric: f32,
    persistence: u32,
    last_seen_block: u64,
    best_lc_phase_chips: i32,
    lc_ratio: f32,
    preamble_hits: u32,
    stage2_attempts: u32,
    first_preamble_tx_chip: Option<usize>,
}

#[derive(Debug, Clone)]
struct LcSearchResult {
    best_phase_chips: i32,
    best_score: f32,
    second_score: f32,
    best_over_second: f32,
    coh_norm: f32,
}

#[derive(Debug, Clone)]
struct DetectionEvent {
    candidate_id: u64,
    delay_samples: i32,
    finger_delay_samples: i32,
    lc_phase_chips: i32,
    tx_chip_at_despread_origin: usize,
    first_preamble_tx_chip: usize,
}

struct Finger {
    id: u64,
    sample_delay: i32,
    lc_phase_chips: i32,
    despread_phase: usize,
    chain: Vec<PipelineProcessorShared>,
    blocks_fed: u64,
    hard_validated: bool,
}

/// Reverse-link access frontend with explicit staged acquisition and finger tracking.
///
/// This path is tuned around the spec-faithful OQPSK waveform from the start,
/// runs stage-2 refinement every block, and uses preamble confirmation to hand
/// off descrambled 256-chip symbols into the existing downstream decoder chain.
pub struct RakeAccessSearcher {
    cfg: RakeAcquisitionConfig,
    search_pn_seq: Vec<Complex32>,
    despread_pn_seq: Vec<Complex32>,
    phase_period: usize,
    composite_filter_delay: usize,
    lc_template: LongCodeGenerator,
    fft_fwd: Arc<dyn Fft<f32>>,
    fft_inv: Arc<dyn Fft<f32>>,
    fft_scratch: Vec<Complex32>,
    chain_builder: Option<Box<dyn Fn() -> Vec<PipelineProcessorShared> + Send>>,
    fingers: Vec<Finger>,
    buffer: Vec<Complex32>,
    samples_consumed: usize,
    block_counter: u64,
    next_candidate_id: u64,
    absolute_origin_sample: Option<usize>,
    recent_windows: VecDeque<(usize, Vec<Complex32>)>,
    recent_pn_maps: VecDeque<Vec<f32>>,
    candidates: Vec<Candidate>,
    sample_rate_hz: f64,
    tags_snapshot: std::collections::HashMap<&'static str, i64>,
}

impl RakeAccessSearcher {
    pub fn new(oversample: usize, lc_template: LongCodeGenerator) -> Self {
        let cfg = RakeAcquisitionConfig::default_4x();
        let phase_period = 32768 * oversample;
        let search_pn_seq = build_fft_search_pn_samples(phase_period, oversample)
            .into_iter()
            .map(|s| Complex32::new(s.re, -s.im))
            .collect::<Vec<_>>();
        let despread_pn_seq = build_oqpsk_pn_samples(phase_period, oversample)
            .into_iter()
            .map(|s| Complex32::new(s.re, -s.im))
            .collect::<Vec<_>>();
        let window_len = cfg.pn_coherent_chips * oversample;
        let mut planner = FftPlanner::<f32>::new();
        let fft_fwd = planner.plan_fft_forward(window_len);
        let fft_inv = planner.plan_fft_inverse(window_len);
        let scratch_len = fft_fwd
            .get_inplace_scratch_len()
            .max(fft_inv.get_inplace_scratch_len());

        Self {
            cfg,
            search_pn_seq,
            despread_pn_seq,
            phase_period,
            composite_filter_delay: (FILTER_TAPS - 1) * 2,
            lc_template,
            fft_fwd,
            fft_inv,
            fft_scratch: vec![Complex32::new(0.0, 0.0); scratch_len],
            chain_builder: None,
            fingers: Vec::new(),
            buffer: Vec::new(),
            samples_consumed: 0,
            block_counter: 0,
            next_candidate_id: 1,
            absolute_origin_sample: None,
            recent_windows: VecDeque::new(),
            recent_pn_maps: VecDeque::new(),
            candidates: Vec::new(),
            sample_rate_hz: 0.0,
            tags_snapshot: std::collections::HashMap::new(),
        }
    }

    pub fn with_composite_filter_delay(mut self, delay: usize) -> Self {
        self.composite_filter_delay = delay;
        self
    }

    pub fn with_config(mut self, cfg: RakeAcquisitionConfig) -> Self {
        self.cfg = cfg;
        self
    }

    pub fn with_chain_builder(
        mut self,
        builder: Box<dyn Fn() -> Vec<PipelineProcessorShared> + Send>,
    ) -> Self {
        self.chain_builder = Some(builder);
        self
    }

    fn base_phase(&self, window_offset: usize) -> usize {
        let abs = self.absolute_origin_sample.unwrap_or(0);
        let raw = (abs + window_offset) % self.phase_period;
        (raw + self.phase_period - (self.composite_filter_delay % self.phase_period))
            % self.phase_period
    }

    fn abs_chip_at(&self, sample_offset: usize) -> usize {
        let abs = self.absolute_origin_sample.unwrap_or(0);
        (abs + sample_offset).saturating_sub(self.composite_filter_delay) / self.cfg.oversample
    }

    fn tx_chip_at_sample(&self, sample_offset: usize) -> usize {
        let abs = self.absolute_origin_sample.unwrap_or(0);
        (abs + sample_offset) / self.cfg.oversample
    }

    fn compute_stage1_pn_map(&mut self, block: &[Complex32], base_phase: usize) -> Vec<f32> {
        let os = self.cfg.oversample;
        let n_chips = self.cfg.pn_coherent_chips;
        let window_len = n_chips * os;
        let pp = self.phase_period;
        let mut pn_up = vec![Complex32::new(0.0, 0.0); window_len];
        for k in 0..n_chips {
            let pn_conj = self.search_pn_seq[(base_phase + k * os) % pp];
            pn_up[k * os] = Complex32::new(pn_conj.re, -pn_conj.im);
        }

        let mut block_fft: Vec<Complex32> = block[..window_len].to_vec();
        self.fft_fwd
            .process_with_scratch(&mut block_fft, &mut self.fft_scratch);
        self.fft_fwd
            .process_with_scratch(&mut pn_up, &mut self.fft_scratch);

        let mut result: Vec<Complex32> = block_fft
            .iter()
            .zip(pn_up.iter())
            .map(|(&b, &p)| b * Complex32::new(p.re, -p.im))
            .collect();

        self.fft_inv
            .process_with_scratch(&mut result, &mut self.fft_scratch);

        let norm = 1.0 / (window_len as f32 * window_len as f32);
        result.iter().map(|c| c.norm_sqr() * norm).collect()
    }

    fn push_stage1_map(&mut self, map: Vec<f32>) {
        self.recent_pn_maps.push_back(map);
        while self.recent_pn_maps.len() > self.cfg.pn_noncoherent_windows {
            self.recent_pn_maps.pop_front();
        }
    }

    fn retain_window(&mut self, window_offset: usize, window: &[Complex32]) {
        self.recent_windows
            .push_back((window_offset, window.to_vec()));
        while self.recent_windows.len() > 256 {
            self.recent_windows.pop_front();
        }
    }

    fn accumulated_stage1_map(&self) -> Option<Vec<f32>> {
        if self.recent_pn_maps.len() < self.cfg.pn_noncoherent_windows {
            return None;
        }
        let len = self.recent_pn_maps.front()?.len();
        let mut accum = vec![0.0f32; len];
        for map in &self.recent_pn_maps {
            for (dst, src) in accum.iter_mut().zip(map.iter()) {
                *dst += *src;
            }
        }
        Some(accum)
    }

    fn extract_local_peaks(&self, accum_map: &[f32]) -> Vec<PnPeak> {
        let len = accum_map.len();
        if len < 3 {
            return Vec::new();
        }
        let half = len / 2;
        let mut raw_peaks = Vec::new();
        for i in 1..(len - 1) {
            let left = accum_map[i - 1];
            let mid = accum_map[i];
            let right = accum_map[i + 1];
            if mid > left && mid >= right {
                let signed_delay = if i > half {
                    i as i32 - len as i32
                } else {
                    i as i32
                };
                raw_peaks.push(PnPeak {
                    delay_samples: signed_delay,
                    metric: mid,
                });
            }
        }
        raw_peaks.sort_by(|a, b| b.metric.partial_cmp(&a.metric).unwrap());

        let suppress = self.cfg.pn_peak_suppress_samples as i32;
        let mut kept: Vec<PnPeak> = Vec::new();
        'outer: for peak in raw_peaks {
            for kept_peak in &kept {
                if (peak.delay_samples - kept_peak.delay_samples).abs() <= suppress {
                    continue 'outer;
                }
            }
            kept.push(peak);
            if kept.len() >= self.cfg.pn_keep_top_n {
                break;
            }
        }
        kept
    }

    fn update_candidates_from_peaks(&mut self, peaks: &[PnPeak]) {
        let block_id = self.block_counter;
        let match_radius = self.cfg.oversample as i32;
        for peak in peaks {
            let mut matched = false;
            for cand in &mut self.candidates {
                if matches!(
                    cand.state,
                    CandidateState::Rejected | CandidateState::PreambleDetected
                ) {
                    continue;
                }
                let cur_delay = cand.coarse_delay_samples + cand.fine_delay_samples;
                if (peak.delay_samples - cur_delay).abs() <= match_radius {
                    cand.coarse_delay_samples = peak.delay_samples;
                    cand.stage1_metric = peak.metric.max(cand.stage1_metric);
                    cand.persistence += 1;
                    cand.last_seen_block = block_id;
                    matched = true;
                    break;
                }
            }
            if !matched && self.active_candidate_count() < self.cfg.max_active_candidates {
                self.candidates.push(Candidate {
                    id: self.next_candidate_id,
                    state: CandidateState::Stage1,
                    coarse_delay_samples: peak.delay_samples,
                    fine_delay_samples: 0,
                    stage1_metric: peak.metric,
                    persistence: 1,
                    last_seen_block: block_id,
                    best_lc_phase_chips: 0,
                    lc_ratio: 0.0,
                    preamble_hits: 0,
                    stage2_attempts: 0,
                    first_preamble_tx_chip: None,
                });
                self.next_candidate_id += 1;
            }
        }
    }

    fn prune_candidates(&mut self) {
        let stage2_max_attempts = 16u32;
        for cand in &mut self.candidates {
            if matches!(
                cand.state,
                CandidateState::PreambleDetected | CandidateState::Rejected
            ) {
                continue;
            }
            if cand.state == CandidateState::Stage1
                && cand.persistence >= self.cfg.pn_persistence_required
            {
                cand.state = CandidateState::Stage2SearchingLc;
            }
            if matches!(
                cand.state,
                CandidateState::Stage2SearchingLc | CandidateState::VerifyingPreamble
            ) && cand.stage2_attempts >= stage2_max_attempts
                && cand.preamble_hits < self.cfg.preamble_hits_required
            {
                cand.state = CandidateState::Rejected;
            }
        }
        self.candidates
            .retain(|c| c.state != CandidateState::Rejected);
    }

    fn active_candidate_count(&self) -> usize {
        self.candidates
            .iter()
            .filter(|c| {
                !matches!(
                    c.state,
                    CandidateState::Rejected | CandidateState::PreambleDetected
                )
            })
            .count()
    }

    fn refine_fine_offset(&self, block: &[Complex32], coarse_delay: i32, base_phase: usize) -> i32 {
        let mut best_offset = 0i32;
        let mut best_metric = f32::MIN;
        for offset in -self.cfg.fine_offset_search_samples..=self.cfg.fine_offset_search_samples {
            let delay = coarse_delay + offset;
            let preview = self.pn_despread(block, delay, base_phase, self.cfg.lc_integrate_chips);
            let metric: f32 = preview.iter().map(|c| c.norm_sqr()).sum();
            if metric > best_metric {
                best_metric = metric;
                best_offset = offset;
            }
        }
        best_offset
    }

    fn pn_despread(
        &self,
        block: &[Complex32],
        delay_samples: i32,
        base_phase: usize,
        n_chips: usize,
    ) -> Vec<Complex32> {
        let os = self.cfg.oversample;
        let window_len = block.len();
        let pp = self.phase_period;
        let mut out = Vec::with_capacity(n_chips);
        for k in 0..n_chips {
            let sample_idx =
                modulo(k as i32 * os as i32 + delay_samples, window_len as i32) as usize;
            let pn_idx = (base_phase + sample_idx) % pp;
            let pn = self.search_pn_seq[pn_idx];
            out.push(block[sample_idx] * pn);
        }
        out
    }

    fn search_lc_with_signs(
        &self,
        pn_despread: &[Complex32],
        lc_signs: &[f32],
        lc_offset: usize,
    ) -> LcSearchResult {
        let half = self.cfg.lc_search_half_span_chips;
        let n_seg = self.cfg.lc_noncoherent_segments.max(1);
        let seg_len = (pn_despread.len() / n_seg).max(1);

        let mut best_phase = 0i32;
        let mut best_score = f32::MIN;
        let mut second_score = f32::MIN;
        let mut best_coh_norm = 0.0f32;

        for phase in -half..=half {
            let lc_idx_base = lc_offset + (phase + half) as usize;
            let mut abs_sum = 0.0f32;
            let mut coh = Complex32::new(0.0, 0.0);
            let mut nc_power_sum = 0.0f32;
            let mut seg_coh = Complex32::new(0.0, 0.0);
            let mut seg_count = 0usize;

            for (i, &chip) in pn_despread.iter().enumerate() {
                let lc_idx = lc_idx_base + i;
                if lc_idx >= lc_signs.len() {
                    break;
                }
                let lc_sign = lc_signs[lc_idx];
                let d = Complex32::new(chip.re * lc_sign, chip.im * lc_sign);
                abs_sum += d.norm();
                coh += d;
                seg_coh += d;
                seg_count += 1;
                if seg_count >= seg_len {
                    nc_power_sum += seg_coh.norm_sqr();
                    seg_coh = Complex32::new(0.0, 0.0);
                    seg_count = 0;
                }
            }
            if seg_count > 0 {
                nc_power_sum += seg_coh.norm_sqr();
            }

            let abs_sum_safe = abs_sum.max(1e-6);
            let coh_norm = coh.norm() / abs_sum_safe;
            let nc_coh_norm = nc_power_sum.sqrt() / abs_sum_safe;
            let score = nc_coh_norm.max(coh_norm);

            if score > best_score {
                second_score = best_score;
                best_score = score;
                best_phase = phase;
                best_coh_norm = coh_norm;
            } else if score > second_score {
                second_score = score;
            }
        }

        let ratio = if second_score > 1e-6 {
            best_score / second_score
        } else {
            f32::INFINITY
        };
        LcSearchResult {
            best_phase_chips: best_phase,
            best_score,
            second_score,
            best_over_second: ratio,
            coh_norm: best_coh_norm,
        }
    }

    fn run_stage2_and_preamble(
        &mut self,
        block: &[Complex32],
        base_phase: usize,
        window_offset: usize,
    ) -> Vec<DetectionEvent> {
        let mut events = Vec::new();
        let half = self.cfg.lc_search_half_span_chips;
        let indices: Vec<usize> = self
            .candidates
            .iter()
            .enumerate()
            .filter(|(_, c)| {
                matches!(
                    c.state,
                    CandidateState::Stage2SearchingLc | CandidateState::VerifyingPreamble
                )
            })
            .map(|(i, _)| i)
            .collect();
        if indices.is_empty() {
            return events;
        }

        let mut positions = Vec::new();
        for &idx in &indices {
            let coarse = self.candidates[idx].coarse_delay_samples;
            let fine = self.refine_fine_offset(block, coarse, base_phase);
            let aligned_delay = coarse + fine;
            let shift_chips = -aligned_delay.div_euclid(self.cfg.oversample as i32);
            let verify_delay = aligned_delay + shift_chips * self.cfg.oversample as i32;
            let expected_chip = self.abs_chip_at(window_offset + verify_delay as usize);
            positions.push((verify_delay, expected_chip, aligned_delay, fine));
        }

        let (mut min_expected_chip, mut max_expected_chip) = (usize::MAX, 0usize);
        for (_, expected_chip, _, _) in &positions {
            min_expected_chip = min_expected_chip.min(*expected_chip);
            max_expected_chip = max_expected_chip.max(*expected_chip);
        }

        let lc_buf_start_chip = min_expected_chip.saturating_sub(half as usize);
        let lc_buf_end_chip = max_expected_chip
            .saturating_add(half as usize)
            .saturating_add(self.cfg.lc_integrate_chips)
            .saturating_add(1);
        let lc_buf_len = lc_buf_end_chip.saturating_sub(lc_buf_start_chip);
        let mut lc = self.lc_template.clone();
        lc.advance_chips(lc_buf_start_chip);
        let lc_signs: Vec<f32> = (0..lc_buf_len)
            .map(|_| if lc.next_chip() == 1 { -1.0 } else { 1.0 })
            .collect();

        for (pos_idx, idx) in indices.into_iter().enumerate() {
            let (verify_delay, expected_chip, aligned_delay, fine) = positions[pos_idx];
            let shift_samples = verify_delay - aligned_delay;
            let verify_base_phase = (base_phase
                + modulo(shift_samples, self.phase_period as i32) as usize)
                % self.phase_period;
            let despread = self.pn_despread(
                block,
                verify_delay,
                verify_base_phase,
                self.cfg.lc_integrate_chips,
            );
            let lc_offset =
                (expected_chip as i64 - half as i64 - lc_buf_start_chip as i64).max(0) as usize;
            let lc_result = self.search_lc_with_signs(&despread, &lc_signs, lc_offset);

            let cand = &mut self.candidates[idx];
            cand.stage2_attempts += 1;
            cand.fine_delay_samples = fine;
            cand.best_lc_phase_chips = lc_result.best_phase_chips;
            cand.lc_ratio = lc_result.best_over_second;

            trace!(
                "rake_stage2: id={} delay={} fine={} lc_phase={} score={:.3} ratio={:.2} coh={:.3}",
                cand.id,
                aligned_delay,
                fine,
                lc_result.best_phase_chips,
                lc_result.best_score,
                lc_result.best_over_second,
                lc_result.coh_norm,
            );

            let coherent_ok = lc_result.coh_norm >= self.cfg.preamble_coh_norm_min;
            let ratio_ok = lc_result.best_over_second >= self.cfg.lc_best_over_second_min;
            let score_ok = lc_result.best_score > 0.20 || lc_result.second_score <= 0.0;
            if coherent_ok && ratio_ok && score_ok {
                cand.preamble_hits += 1;
                cand.state = CandidateState::VerifyingPreamble;
                if cand.first_preamble_tx_chip.is_none() {
                    let first_chip =
                        (expected_chip as i64 + lc_result.best_phase_chips as i64).max(0) as usize;
                    cand.first_preamble_tx_chip = Some(first_chip);
                }
            }

            if cand.preamble_hits >= self.cfg.preamble_hits_required {
                cand.state = CandidateState::PreambleDetected;
                let tx_chip =
                    (expected_chip as i64 + lc_result.best_phase_chips as i64).max(0) as usize;
                info!(
                    "rake preamble detected: id={} delay={} lc_phase={} tx_chip={}",
                    cand.id, aligned_delay, lc_result.best_phase_chips, tx_chip
                );
                events.push(DetectionEvent {
                    candidate_id: cand.id,
                    delay_samples: aligned_delay,
                    finger_delay_samples: verify_delay,
                    lc_phase_chips: lc_result.best_phase_chips,
                    tx_chip_at_despread_origin: tx_chip,
                    first_preamble_tx_chip: cand.first_preamble_tx_chip.unwrap_or(tx_chip),
                });
            }
        }

        events
    }

    fn run_joint_pn_lc_fft_search(&mut self, block: &[Complex32], base_phase: usize) {
        let os = self.cfg.oversample;
        let n_chips = self.cfg.pn_coherent_chips;
        let window_len = n_chips * os;
        let pp = self.phase_period;
        let half = self.cfg.joint_lc_half_span;
        let expected_chip = self.abs_chip_at(self.samples_consumed - window_len);

        let mut signal_fft = block[..window_len].to_vec();
        self.fft_fwd
            .process_with_scratch(&mut signal_fft, &mut self.fft_scratch);

        let mut best_delay = 0i32;
        let mut best_power = 0.0f32;
        let mut total_power = 0.0f64;
        let mut total_count = 0usize;
        let norm = 1.0 / (window_len as f32 * window_len as f32);

        for lc_phase in -half..=half {
            let lc_start_chip = (expected_chip as i64 + lc_phase as i64).max(0) as usize;
            let mut lc = self.lc_template.clone();
            lc.advance_chips(lc_start_chip);
            let mut ref_buf = vec![Complex32::new(0.0, 0.0); window_len];
            for k in 0..n_chips {
                let pn_conj = self.search_pn_seq[(base_phase + k * os) % pp];
                let lc_sign: f32 = if lc.next_chip() == 1 { -1.0 } else { 1.0 };
                ref_buf[k * os] = Complex32::new(pn_conj.re * lc_sign, -pn_conj.im * lc_sign);
            }
            self.fft_fwd
                .process_with_scratch(&mut ref_buf, &mut self.fft_scratch);
            let mut result: Vec<Complex32> = signal_fft
                .iter()
                .zip(ref_buf.iter())
                .map(|(&s, &r)| s * Complex32::new(r.re, -r.im))
                .collect();
            self.fft_inv
                .process_with_scratch(&mut result, &mut self.fft_scratch);

            for (d, c) in result.iter().enumerate() {
                let power = c.norm_sqr() * norm;
                total_power += power as f64;
                total_count += 1;
                if power > best_power {
                    best_power = power;
                    best_delay = if d > window_len / 2 {
                        d as i32 - window_len as i32
                    } else {
                        d as i32
                    };
                }
            }
        }

        let avg_power = (total_power / total_count.max(1) as f64) as f32;
        let snr = best_power / avg_power.max(1e-20);
        if snr < self.cfg.joint_snr_min {
            return;
        }

        let match_radius = os as i32;
        let already_exists = self.candidates.iter().any(|c| {
            c.state != CandidateState::Rejected
                && (c.coarse_delay_samples - best_delay).abs() <= match_radius
        });
        if already_exists {
            return;
        }

        info!(
            "rake joint candidate: id={} delay={} snr={:.1}x",
            self.next_candidate_id, best_delay, snr
        );
        self.candidates.push(Candidate {
            id: self.next_candidate_id,
            state: CandidateState::Stage2SearchingLc,
            coarse_delay_samples: best_delay,
            fine_delay_samples: 0,
            stage1_metric: snr,
            persistence: self.cfg.pn_persistence_required,
            last_seen_block: self.block_counter,
            best_lc_phase_chips: 0,
            lc_ratio: 0.0,
            preamble_hits: 0,
            stage2_attempts: 0,
            first_preamble_tx_chip: None,
        });
        self.next_candidate_id += 1;
    }

    fn compute_despread_phase(&self, window_offset: usize) -> usize {
        let abs = self.absolute_origin_sample.unwrap_or(0);
        let pp = self.phase_period as i64;
        let raw = (abs + window_offset) as i64 - self.composite_filter_delay as i64;
        ((raw % pp + pp) % pp) as usize
    }

    fn compute_despread_phase_static(
        absolute_origin_sample: usize,
        composite_filter_delay: usize,
        phase_period: usize,
        window_offset: usize,
    ) -> usize {
        let pp = phase_period as i64;
        let raw = (absolute_origin_sample + window_offset) as i64 - composite_filter_delay as i64;
        ((raw % pp + pp) % pp) as usize
    }

    fn lc_start_chip_for_window_static(
        absolute_origin_sample: usize,
        composite_filter_delay: usize,
        oversample: usize,
        window_offset: usize,
        sample_delay: i32,
        lc_phase_chips: i32,
    ) -> usize {
        let expected_chip = (absolute_origin_sample + window_offset + sample_delay.max(0) as usize)
            .saturating_sub(composite_filter_delay)
            / oversample;
        (expected_chip as i64 + lc_phase_chips as i64).max(0) as usize
    }

    fn w0_coherence(
        absolute_origin_sample: usize,
        composite_filter_delay: usize,
        phase_period: usize,
        oversample: usize,
        pn_seq: &[Complex32],
        lc_template: &LongCodeGenerator,
        window_offset: usize,
        sample_delay: i32,
        lc_phase_chips: i32,
        input_samples: &[Complex32],
    ) -> f32 {
        let base_phase = Self::compute_despread_phase_static(
            absolute_origin_sample,
            composite_filter_delay,
            phase_period,
            window_offset,
        );
        let lc_start_chip = Self::lc_start_chip_for_window_static(
            absolute_origin_sample,
            composite_filter_delay,
            oversample,
            window_offset,
            sample_delay,
            lc_phase_chips,
        );
        let mut lc_gen = lc_template.clone();
        lc_gen.advance_chips(lc_start_chip);

        let mut coh = Complex32::new(0.0, 0.0);
        let mut incoh = 0.0f32;
        for k in 0..256usize {
            let sample_idx = sample_delay.max(0) as usize + k * oversample;
            if sample_idx >= input_samples.len() {
                break;
            }
            let pn_idx = (base_phase + sample_idx) % phase_period;
            let pn = pn_seq[pn_idx];
            let despread = input_samples[sample_idx] * pn;
            let lc_sign: f32 = if lc_gen.next_chip() == 1 { -1.0 } else { 1.0 };
            let chip = Complex32::new(despread.re * lc_sign, despread.im * lc_sign);
            coh += chip;
            incoh += chip.norm();
        }
        if incoh > 1e-9 {
            coh.norm() / incoh
        } else {
            0.0
        }
    }

    fn refine_spawn_lock(
        &self,
        replay_windows: &[(usize, Vec<Complex32>)],
        sample_delay_hint: i32,
        lc_phase_hint: i32,
    ) -> (i32, i32) {
        let absolute_origin_sample = self.absolute_origin_sample.unwrap_or(0);
        let mut best_delay = sample_delay_hint;
        let mut best_lc_phase = lc_phase_hint;
        let mut best_score = f32::NEG_INFINITY;
        let lc_span = 0;

        for sample_delay in 0..self.cfg.oversample as i32 {
            for lc_phase in (lc_phase_hint - lc_span)..=(lc_phase_hint + lc_span) {
                let mut score = 0.0f32;
                for (offset, samples) in replay_windows.iter().take(8) {
                    score += Self::w0_coherence(
                        absolute_origin_sample,
                        self.composite_filter_delay,
                        self.phase_period,
                        self.cfg.oversample,
                        &self.despread_pn_seq,
                        &self.lc_template,
                        *offset,
                        sample_delay,
                        lc_phase,
                        samples,
                    );
                }
                if score > best_score {
                    best_score = score;
                    best_delay = sample_delay;
                    best_lc_phase = lc_phase;
                }
            }
        }

        info!(
            "rake spawn refine: delay_hint={} lc_hint={} -> delay={} lc_phase={} score={:.3}",
            sample_delay_hint, lc_phase_hint, best_delay, best_lc_phase, best_score
        );
        (best_delay, best_lc_phase)
    }

    fn feed_one_finger(
        finger: &mut Finger,
        input_samples: &[Complex32],
        base_phase: usize,
        lc_start_chip: usize,
        lc_template: &LongCodeGenerator,
        pn_seq: &[Complex32],
        oversample: usize,
        phase_period: usize,
        sample_rate_hz: f64,
        tags_snapshot: &std::collections::HashMap<&'static str, i64>,
    ) -> Vec<SampleBlock> {
        let chip_rate_hz = sample_rate_hz / oversample.max(1) as f64;
        let mut produced = Vec::new();

        finger.blocks_fed += 1;
        let mut lc_gen = lc_template.clone();
        lc_gen.advance_chips(lc_start_chip);

        let mut samples = Vec::with_capacity(256);
        for k in 0..256usize {
            let sample_idx = finger.sample_delay.max(0) as usize + k * oversample;
            if sample_idx >= input_samples.len() {
                break;
            }
            let pn_idx = (base_phase + sample_idx) % phase_period;
            let pn = pn_seq[pn_idx];
            let despread = input_samples[sample_idx] * pn;
            let lc_sign: f32 = if lc_gen.next_chip() == 1 { -1.0 } else { 1.0 };
            samples.push(Complex32::new(despread.re * lc_sign, despread.im * lc_sign));
        }

        if samples.len() == 256 {
            let abs_chip = lc_start_chip;
            let mut blk = SampleBlock::new(samples, abs_chip).with_sample_rate_hz(chip_rate_hz);
            blk.tags = tags_snapshot.clone();
            blk.tags.insert("pilot_phase", base_phase as i64);
            blk.tags.insert("absolute_chip_start", abs_chip as i64);
            blk.tags.insert("finger_id", finger.id as i64);

            let mut chain_output = vec![blk];
            for proc in &mut finger.chain {
                let mut next = Vec::new();
                for b in chain_output {
                    next.extend(proc.process_block(b));
                }
                chain_output = next;
            }

            chain_output.retain(|b| {
                if b.tags.get("access_event") == Some(&1) {
                    return b.tags.get("access_crc_valid") == Some(&1);
                }
                true
            });

            if !finger.hard_validated {
                finger.hard_validated = chain_output
                    .iter()
                    .any(|b| b.tags.get("access_crc_valid") == Some(&1));
            }
            produced.extend(chain_output);
        }

        produced
    }

    fn spawn_finger(
        &mut self,
        detection: &DetectionEvent,
        window_offset: usize,
    ) -> Vec<SampleBlock> {
        if self.fingers.len() >= self.cfg.max_active_candidates {
            return Vec::new();
        }
        let builder = match &self.chain_builder {
            Some(b) => b,
            None => return Vec::new(),
        };
        let os = self.cfg.oversample;
        let replay_windows = self
            .recent_windows
            .iter()
            .filter(|(offset, _)| *offset <= window_offset)
            .map(|(offset, samples)| (*offset, samples.clone()))
            .collect::<Vec<_>>();
        let (delay, refined_lc_phase) = self.refine_spawn_lock(
            &replay_windows,
            modulo(detection.finger_delay_samples, os as i32),
            detection.lc_phase_chips,
        );
        let despread_delay = delay;
        let despread_phase = self.compute_despread_phase(window_offset);
        if self.fingers.iter().any(|f| {
            let diff = (f.despread_phase as i64 - despread_phase as i64).abs();
            (diff as usize) < os
        }) {
            return Vec::new();
        }

        let center_offset = modulo(delay, os as i32) as usize;
        let replay_window_offset = self
            .recent_windows
            .front()
            .map(|(offset, _)| *offset)
            .unwrap_or(window_offset);
        let replay_despread_phase = self.compute_despread_phase(replay_window_offset);
        let delay_pn_chip = (despread_phase as i64 + delay as i64).div_euclid(os as i64);
        let center_pn_chip = (despread_phase as i64 + center_offset as i64).div_euclid(os as i64);
        let chip_adjustment = center_pn_chip - delay_pn_chip;
        let replay_start_chip = self.tx_chip_at_sample(replay_window_offset + center_offset);
        let first_tx_chip =
            (detection.tx_chip_at_despread_origin as i64 + chip_adjustment).max(0) as usize;
        let first_preamble_chip =
            (detection.first_preamble_tx_chip as i64 + chip_adjustment).max(0) as usize;
        let chain_start_chip = replay_start_chip;

        info!(
            "spawn rake finger {} delay={} despread_phase={} first_tx_chip={} first_preamble_chip={} chain_start_chip={} lc_phase={}",
            detection.candidate_id,
            delay,
            despread_phase,
            first_tx_chip,
            first_preamble_chip,
            chain_start_chip,
            refined_lc_phase,
        );

        self.fingers.push(Finger {
            id: detection.candidate_id,
            despread_phase: replay_despread_phase,
            chain: builder(),
            blocks_fed: 0,
            hard_validated: false,
            sample_delay: despread_delay,
            lc_phase_chips: refined_lc_phase,
        });

        let mut replay_output = Vec::new();
        let retained = replay_windows
            .into_iter()
            .filter(|(offset, _)| *offset < window_offset)
            .collect::<Vec<_>>();
        let pn_seq = self.despread_pn_seq.clone();
        let tags_snapshot = self.tags_snapshot.clone();
        let sample_rate_hz = self.sample_rate_hz;
        let oversample = self.cfg.oversample;
        let phase_period = self.phase_period;
        let absolute_origin_sample = self.absolute_origin_sample.unwrap_or(0);
        let composite_filter_delay = self.composite_filter_delay;
        let lc_template = self.lc_template.clone();
        if let Some(finger) = self.fingers.last_mut() {
            for (replay_offset, samples) in retained {
                let base_phase = Self::compute_despread_phase_static(
                    absolute_origin_sample,
                    composite_filter_delay,
                    phase_period,
                    replay_offset,
                );
                let lc_start_chip = Self::lc_start_chip_for_window_static(
                    absolute_origin_sample,
                    composite_filter_delay,
                    oversample,
                    replay_offset,
                    finger.sample_delay,
                    finger.lc_phase_chips,
                );
                replay_output.extend(Self::feed_one_finger(
                    finger,
                    &samples,
                    base_phase,
                    lc_start_chip,
                    &lc_template,
                    &pn_seq,
                    oversample,
                    phase_period,
                    sample_rate_hz,
                    &tags_snapshot,
                ));
            }
        }
        replay_output
    }

    fn feed_fingers(
        &mut self,
        window_offset: usize,
        input_samples: &[Complex32],
    ) -> Vec<SampleBlock> {
        let mut produced = Vec::new();
        let pn_seq = self.despread_pn_seq.clone();
        let tags_snapshot = self.tags_snapshot.clone();
        let sample_rate_hz = self.sample_rate_hz;
        let oversample = self.cfg.oversample;
        let phase_period = self.phase_period;
        let absolute_origin_sample = self.absolute_origin_sample.unwrap_or(0);
        let composite_filter_delay = self.composite_filter_delay;
        let lc_template = self.lc_template.clone();

        for finger in &mut self.fingers {
            let base_phase = Self::compute_despread_phase_static(
                absolute_origin_sample,
                composite_filter_delay,
                phase_period,
                window_offset,
            );
            let lc_start_chip = Self::lc_start_chip_for_window_static(
                absolute_origin_sample,
                composite_filter_delay,
                oversample,
                window_offset,
                finger.sample_delay,
                finger.lc_phase_chips,
            );
            produced.extend(Self::feed_one_finger(
                finger,
                input_samples,
                base_phase,
                lc_start_chip,
                &lc_template,
                &pn_seq,
                oversample,
                phase_period,
                sample_rate_hz,
                &tags_snapshot,
            ));
        }

        self.fingers
            .retain(|f| f.hard_validated || f.blocks_fed <= 128);
        produced
    }
}

impl PipelineProcessor for RakeAccessSearcher {
    fn process_block(&mut self, block: SampleBlock) -> Vec<SampleBlock> {
        self.sample_rate_hz = block.sample_rate_hz;
        self.tags_snapshot = block.tags.clone();

        if self.absolute_origin_sample.is_none() {
            self.absolute_origin_sample = block
                .tags
                .get("absolute_sample_start")
                .copied()
                .map(|v| v.max(0) as usize);
        }

        self.buffer.extend_from_slice(&block.samples);

        let os = self.cfg.oversample;
        let window_samples = self.cfg.pn_coherent_chips * os;
        let mut produced = Vec::new();

        while self.buffer.len() >= window_samples {
            let window: Vec<Complex32> = self.buffer.drain(..window_samples).collect();
            let window_offset = self.samples_consumed;
            self.samples_consumed += window_samples;
            self.retain_window(window_offset, &window);
            let bp = self.base_phase(window_offset);

            let pn_map = self.compute_stage1_pn_map(&window, bp);
            self.push_stage1_map(pn_map);

            if let Some(accum_map) = self.accumulated_stage1_map() {
                let peaks = self.extract_local_peaks(&accum_map);
                if self.block_counter % self.cfg.pn_noncoherent_windows as u64 == 0 {
                    let total: f32 = accum_map.iter().sum();
                    let avg = total / accum_map.len().max(1) as f32;
                    let top5: Vec<String> = peaks
                        .iter()
                        .take(5)
                        .map(|p| {
                            format!(
                                "d={}: {:.1} ({:.1}x)",
                                p.delay_samples,
                                p.metric,
                                p.metric / avg.max(1e-10)
                            )
                        })
                        .collect();
                    debug!(
                        "rake stage1: block={} candidates={} avg={:.1} top5: {}",
                        self.block_counter,
                        self.candidates.len(),
                        avg,
                        top5.join(", ")
                    );
                }
                self.update_candidates_from_peaks(&peaks);
            }

            self.prune_candidates();
            if self.fingers.is_empty()
                && self.block_counter % self.cfg.joint_search_interval_blocks == 0
                && self.block_counter > 0
            {
                self.run_joint_pn_lc_fft_search(&window, bp);
            }

            let events = if self.block_counter % self.cfg.stage2_interval_blocks == 0 {
                self.run_stage2_and_preamble(&window, bp, window_offset)
            } else {
                Vec::new()
            };

            for ev in &events {
                produced.extend(self.spawn_finger(ev, window_offset));
            }

            for ev in events {
                let mut blk = SampleBlock::new(vec![Complex32::new(1.0, 0.0)], block.chip_start)
                    .with_sample_rate_hz(self.sample_rate_hz);
                blk.tags = self.tags_snapshot.clone();
                blk.tags.insert("access_preamble_detected", 1);
                blk.tags.insert("finger_id", ev.candidate_id as i64);
                blk.tags.insert("pn_delay", ev.delay_samples as i64);
                blk.tags.insert("lc_phase", ev.lc_phase_chips as i64);
                produced.push(blk);
            }

            produced.extend(self.feed_fingers(window_offset, &window));
            self.block_counter += 1;
        }

        produced
    }

    fn name(&self) -> &'static str {
        "RakeAccessSearcher"
    }
}

fn modulo(x: i32, m: i32) -> i32 {
    ((x % m) + m) % m
}
