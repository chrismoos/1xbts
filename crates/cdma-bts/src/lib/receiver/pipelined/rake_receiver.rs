use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use std::time::Instant;

use log::{debug, info, trace};

use crate::receiver::pipelined::{
    PipelineProcessor, PipelineProcessorShared, SampleBlock, build_matched_pn_reference,
    build_oqpsk_pn_samples, flush_sub_chain,
};
use crate::sdr::cdma2000_baseband_filter_taps_f64;
use num_complex::Complex32;
use rustfft::{Fft, FftPlanner};

// --- Finger lifecycle ---
//
// Single-user (current): fingers compete to find the one correct PN phase.
// Validation = downstream channel evidence (sync/CRC pass or detected access preamble).
// Once a winner is chosen, all other fingers are pruned.
//
// Multi-user / multi-path (future):
//   - Each finger's downstream chain may decode a different logical channel
//     (different Walsh code, different pilot PN offset, different sector).
//   - Multiple fingers can be validated simultaneously if they represent
//     distinct users/paths. Deduplication should group by a downstream
//     identity key (e.g. pilot_pn + walsh_code) rather than single-winner.
//   - For true RAKE combining (same user, multiple paths), validated fingers
//     with the same identity key would have their soft symbols combined
//     (MRC) before Viterbi decoding, rather than running independent chains.
//   - The chain_builder would need to be parameterized per-finger (e.g.
//     different Walsh codes per finger) or the combining would happen at
//     the chip level before a shared downstream chain.

const MAX_FINGERS: usize = 3;
const MIN_PEAK_SEPARATION_CHIPS: usize = 16;
const FINGER_DEAD_MISSES: usize = 8;
const ACQUIRE_THRESHOLD: f32 = 8.0;
const TRACK_THRESHOLD: f32 = 4.0; // 16 is good but can track too slowly
const TRACK_SEARCH_HALF_WINDOW: usize = 8;

struct Finger {
    id: usize,
    phase: usize,
    hits: usize,
    misses: usize,
    validated: bool,
    last_snr: f32,
    chain: Vec<PipelineProcessorShared>,
    output_samples: VecDeque<Complex32>,
    chip_start: usize,
    started: bool,
}

pub struct RakeReceiver {
    fft_pn: Vec<Complex32>,
    fft_planner: Arc<dyn Fft<f32>>,
    fft_planner_inverse: Arc<dyn Fft<f32>>,
    fft_scratch: Vec<Complex32>,
    pn_seq_filtered: Vec<Complex32>,
    pn_seq_despread: Vec<Complex32>,
    despread_phase_offset: usize,
    use_raw_pn_despreading: bool,

    buffer: Vec<Complex32>,
    sample: usize,

    fft_length: usize,
    buffer_samples: usize,
    oversample: usize,
    phase_period: usize,
    overlap: usize,
    input_origin_sample: Option<usize>,
    absolute_origin_sample: Option<usize>,

    fingers: Vec<Finger>,
    chain_builder: Box<dyn Fn() -> Vec<PipelineProcessorShared> + Send>,
    next_finger_id: usize,
    last_stats_log: Instant,
    timing_blocks: usize,
    timing_fft_us: u64,
    timing_peaks_us: u64,
    timing_track_us: u64,
    timing_spawn_us: u64,
    timing_finger_us: u64,
    timing_select_prune_us: u64,
    timing_total_us: u64,
    timing_total_max_us: u64,
    timing_stage_us: BTreeMap<&'static str, u64>,
    timing_stage_calls: BTreeMap<&'static str, u64>,
    timing_finger_wrapper_us: BTreeMap<&'static str, u64>,
    timing_finger_wrapper_calls: BTreeMap<&'static str, u64>,
}

impl RakeReceiver {
    pub fn new(
        oversample: usize,
        chain_builder: Box<dyn Fn() -> Vec<PipelineProcessorShared> + Send>,
    ) -> Self {
        Self::new_with_reference_filter_passes(oversample, chain_builder, 2)
    }

    pub fn new_with_reference_filter_passes(
        oversample: usize,
        chain_builder: Box<dyn Fn() -> Vec<PipelineProcessorShared> + Send>,
        reference_filter_passes: usize,
    ) -> Self {
        // Same FFT setup as MatchedFilterTracker
        let filter_taps = cdma2000_baseband_filter_taps_f64();
        let pn_seq_filtered =
            build_matched_pn_reference(32768 * oversample, oversample, reference_filter_passes);
        let pn_seq_despread = build_oqpsk_pn_samples(32768 * oversample, oversample)
            .into_iter()
            .map(|s| Complex32::new(s.re, -s.im))
            .collect::<Vec<_>>();
        // The matched PN reference uses an even-length FIR, so there is no
        // single integer-valued center sample. Use the upper midpoint here;
        // test-path decimation can add any extra per-chip sample phase it wants.
        let per_pass_group_delay = filter_taps.len() / 2;
        let despread_phase_offset = per_pass_group_delay.saturating_mul(reference_filter_passes);

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

        let phase_period = 32768 * oversample;
        let overlap = fft_length / 2 - 1;

        Self {
            fft_planner: planner,
            fft_planner_inverse: FftPlanner::new().plan_fft_inverse(fft_length),
            fft_pn,
            fft_scratch: vec![Complex32::new(0.0, 0.0); fft_length],
            pn_seq_filtered,
            pn_seq_despread,
            despread_phase_offset,
            use_raw_pn_despreading: false,
            buffer: vec![Complex32::new(0.0, 0.0); overlap],
            sample: 0,
            fft_length,
            buffer_samples: (fft_length / 2) + 1,
            oversample,
            phase_period,
            overlap,
            input_origin_sample: None,
            absolute_origin_sample: None,
            fingers: Vec::new(),
            chain_builder,
            next_finger_id: 0,
            last_stats_log: Instant::now(),
            timing_blocks: 0,
            timing_fft_us: 0,
            timing_peaks_us: 0,
            timing_track_us: 0,
            timing_spawn_us: 0,
            timing_finger_us: 0,
            timing_select_prune_us: 0,
            timing_total_us: 0,
            timing_total_max_us: 0,
            timing_stage_us: BTreeMap::new(),
            timing_stage_calls: BTreeMap::new(),
            timing_finger_wrapper_us: BTreeMap::new(),
            timing_finger_wrapper_calls: BTreeMap::new(),
        }
    }

    pub fn with_despread_phase_offset_override(mut self, offset: usize) -> Self {
        self.despread_phase_offset = offset;
        self
    }

    pub fn with_raw_pn_despreading(mut self, enabled: bool) -> Self {
        self.use_raw_pn_despreading = enabled;
        self
    }

    /// Extract top correlation peaks above threshold with minimum separation.
    fn extract_peaks(&self, multiplied: &[Complex32], median: f32) -> Vec<(usize, f32)> {
        let candidate_cap = MAX_FINGERS * 16;
        let mut phase_powers: Vec<(usize, f32)> = Vec::with_capacity(candidate_cap);
        for idx in self.overlap..self.fft_length {
            let power = multiplied[idx].norm_sqr() / median;
            if power < ACQUIRE_THRESHOLD {
                continue;
            }
            let rel = (idx - self.overlap) % self.phase_period;
            let phase = (self.phase_period - rel) % self.phase_period;
            if phase_powers.len() < candidate_cap {
                phase_powers.push((phase, power));
                continue;
            }
            if let Some((min_idx, min_power)) = phase_powers
                .iter()
                .enumerate()
                .min_by(|a, b| a.1.1.total_cmp(&b.1.1))
                .map(|(i, (_, p))| (i, *p))
            {
                if power > min_power {
                    phase_powers[min_idx] = (phase, power);
                }
            }
        }

        phase_powers.sort_by(|a, b| b.1.total_cmp(&a.1));

        let min_sep = MIN_PEAK_SEPARATION_CHIPS * self.oversample;
        let mut peaks = Vec::new();
        for (phase, power) in phase_powers {
            let too_close = peaks.iter().any(|(p, _): &(usize, f32)| {
                phase_distance(phase, *p, self.phase_period) < min_sep
            });
            if !too_close {
                peaks.push((phase, power));
                if peaks.len() >= MAX_FINGERS * 2 {
                    break;
                }
            }
        }
        peaks
    }

    fn spawn_finger(&mut self, phase: usize) {
        let id = self.next_finger_id;
        self.next_finger_id += 1;
        let chain = (self.chain_builder)();
        debug!("rake: spawning finger {} at phase {}", id, phase);
        self.fingers.push(Finger {
            id,
            phase,
            hits: 1,
            misses: 0,
            validated: false,
            last_snr: 0.0,
            chain,
            output_samples: VecDeque::new(),
            chip_start: 0,
            started: false,
        });
    }

    /// Despread and feed samples through a finger's pipeline chain.
    fn feed_finger(
        pn_seq: &[Complex32],
        despread_phase_offset: usize,
        finger: &mut Finger,
        input_samples: &[Complex32],
        oversample: usize,
        phase_period: usize,
        block_chip_start: usize,
        input_sample_rate_hz: f64,
        block_tags: &std::collections::HashMap<&'static str, i64>,
        input_origin_sample: usize,
        absolute_origin_sample: Option<usize>,
        timing_stage_us: &mut BTreeMap<&'static str, u64>,
        timing_stage_calls: &mut BTreeMap<&'static str, u64>,
        timing_finger_wrapper_us: &mut BTreeMap<&'static str, u64>,
        timing_finger_wrapper_calls: &mut BTreeMap<&'static str, u64>,
    ) -> Vec<SampleBlock> {
        let t_despread = Instant::now();
        let despread_phase = if phase_period == 0 {
            finger.phase
        } else {
            (finger.phase + phase_period - (despread_phase_offset % phase_period)) % phase_period
        };
        // Use raw (unfiltered) PN for despreading. The input has already been
        // matched-filtered, giving a raised-cosine pulse shape with zero ISI
        // at chip instants. Convert the filtered-reference search phase back
        // to the raw-PN despread phase by subtracting the per-reference filter
        // group delay.
        finger.output_samples.extend(
            input_samples
                .iter()
                .enumerate()
                .map(|(idx, val)| pn_seq[(despread_phase + idx) % phase_period].conj() * val),
        );
        let despread_us = t_despread.elapsed().as_micros() as u64;
        *timing_finger_wrapper_us.entry("despread").or_insert(0) += despread_us;
        *timing_finger_wrapper_calls.entry("despread").or_insert(0) += 1;

        if !finger.started {
            finger.chip_start = block_chip_start;
            finger.started = true;
        }

        // Keep finger output blocks at one 64-chip symbol. Larger batches
        // reduced sync/paging decode reliability in the forward-link RAKE
        // tests because downstream offset-search/warmup logic is keyed to
        // process_block() cadence rather than absolute sample count.
        let block_size = 64 * oversample;
        let mut produced = Vec::new();

        while finger.output_samples.len() >= block_size {
            let t_block_extract = Instant::now();
            let samples: Vec<Complex32> = finger.output_samples.drain(..block_size).collect();
            let block_extract_us = t_block_extract.elapsed().as_micros() as u64;
            *timing_finger_wrapper_us.entry("block_extract").or_insert(0) += block_extract_us;
            *timing_finger_wrapper_calls
                .entry("block_extract")
                .or_insert(0) += 1;

            let t_tag_build = Instant::now();
            let mut finger_block = SampleBlock::new(samples, finger.chip_start)
                .with_sample_rate_hz(input_sample_rate_hz);
            finger_block.tags = block_tags.clone();
            finger_block
                .tags
                .insert("pilot_phase", despread_phase as i64);
            if let Some(abs_origin) = absolute_origin_sample {
                let rel_sample = finger.chip_start.saturating_sub(input_origin_sample);
                let absolute_sample_start = abs_origin.saturating_add(rel_sample);
                finger_block
                    .tags
                    .insert("absolute_sample_start", absolute_sample_start as i64);
                finger_block.tags.insert(
                    "absolute_chip_start",
                    (absolute_sample_start / oversample.max(1)) as i64,
                );
            }
            finger.chip_start += block_size;
            let tag_build_us = t_tag_build.elapsed().as_micros() as u64;
            *timing_finger_wrapper_us.entry("tag_build").or_insert(0) += tag_build_us;
            *timing_finger_wrapper_calls.entry("tag_build").or_insert(0) += 1;

            let t_subchain = Instant::now();
            let chain_output = Self::run_timed_finger_chain(
                &mut finger.chain,
                finger_block,
                timing_stage_us,
                timing_stage_calls,
            );
            let subchain_us = t_subchain.elapsed().as_micros() as u64;
            *timing_finger_wrapper_us.entry("subchain").or_insert(0) += subchain_us;
            *timing_finger_wrapper_calls.entry("subchain").or_insert(0) += 1;

            let t_validation = Instant::now();
            for blk in &chain_output {
                let ms_sync = blk.tags.get("ms_sync_event").copied();
                let crc_valid = blk.tags.get("deinterleaver_lock_crc_valid").copied();
                let access_crc = blk.tags.get("access_crc_valid").copied();
                let access_preamble = blk.tags.get("access_preamble_detected").copied();

                if ms_sync.is_some()
                    || crc_valid.is_some()
                    || access_crc.is_some()
                    || access_preamble.is_some()
                {
                    debug!(
                        "rake: finger {} validation attempt phase={} hits={} — ms_sync={:?} crc={:?} access_crc={:?} access_preamble={:?}",
                        finger.id,
                        finger.phase,
                        finger.hits,
                        ms_sync,
                        crc_valid,
                        access_crc,
                        access_preamble
                    );
                }

                let passed = ms_sync == Some(1)
                    || crc_valid.is_some_and(|v| v > 0)
                    || access_crc.is_some_and(|v| v > 0)
                    || access_preamble.is_some_and(|v| v > 0);
                if passed && !finger.validated {
                    info!(
                        "rake: finger {} VALIDATED at phase {} (hits={} misses={})",
                        finger.id, finger.phase, finger.hits, finger.misses
                    );
                    finger.validated = true;
                }
            }
            let validation_us = t_validation.elapsed().as_micros() as u64;
            *timing_finger_wrapper_us.entry("validation").or_insert(0) += validation_us;
            *timing_finger_wrapper_calls.entry("validation").or_insert(0) += 1;

            produced.extend(chain_output);
        }

        produced
    }

    fn run_timed_finger_chain(
        chain: &mut [PipelineProcessorShared],
        input: SampleBlock,
        timing_stage_us: &mut BTreeMap<&'static str, u64>,
        timing_stage_calls: &mut BTreeMap<&'static str, u64>,
    ) -> Vec<SampleBlock> {
        let mut blocks = vec![input];
        for processor in chain.iter_mut() {
            let stage_name = processor.name();
            let stage_start = Instant::now();
            let mut next = Vec::new();
            for blk in blocks {
                if blk.is_empty() {
                    continue;
                }
                next.extend(processor.process_block(blk));
            }
            let stage_us = stage_start.elapsed().as_micros() as u64;
            *timing_stage_us.entry(stage_name).or_insert(0) += stage_us;
            *timing_stage_calls.entry(stage_name).or_insert(0) += 1;
            blocks = next;
        }
        blocks.retain(|b| !b.is_empty());
        blocks
    }
}

impl PipelineProcessor for RakeReceiver {
    fn process_block(&mut self, block: SampleBlock) -> Vec<SampleBlock> {
        let input_sample_rate_hz = block.sample_rate_hz;
        if self.input_origin_sample.is_none() {
            self.input_origin_sample = Some(block.chip_start);
            self.absolute_origin_sample = block
                .tags
                .get("absolute_sample_start")
                .copied()
                .and_then(|v| usize::try_from(v).ok());
        }
        self.buffer.extend(&block.samples);
        let mut produced_blocks = Vec::new();

        if !self.fingers.is_empty() {
            while self.buffer.len() >= self.buffer_samples {
                let iter_start = Instant::now();
                let input_samples: Vec<Complex32> = self.buffer[0..self.buffer_samples].to_vec();
                self.sample += self.buffer_samples;
                self.buffer.drain(0..self.buffer_samples);

                // Advance finger phases by consumed samples within PN period
                for finger in self.fingers.iter_mut() {
                    finger.phase = (finger.phase + self.buffer_samples) % self.phase_period;
                }

                let block_chip_start = self
                    .input_origin_sample
                    .unwrap_or(0)
                    .saturating_add(self.sample.saturating_sub(self.buffer_samples));

                let mut per_finger_output: Vec<(usize, Vec<SampleBlock>)> = Vec::new();
                for finger in self.fingers.iter_mut() {
                    let (pn_seq, despread_phase_offset) = if self.use_raw_pn_despreading {
                        (&self.pn_seq_despread, self.despread_phase_offset)
                    } else {
                        (&self.pn_seq_filtered, 0)
                    };
                    let out = Self::feed_finger(
                        pn_seq,
                        despread_phase_offset,
                        finger,
                        &input_samples,
                        self.oversample,
                        self.phase_period,
                        block_chip_start,
                        input_sample_rate_hz,
                        &block.tags,
                        self.input_origin_sample.unwrap_or(0),
                        self.absolute_origin_sample,
                        &mut self.timing_stage_us,
                        &mut self.timing_stage_calls,
                        &mut self.timing_finger_wrapper_us,
                        &mut self.timing_finger_wrapper_calls,
                    );
                    per_finger_output.push((finger.id, out));
                }

                let winner_id = self
                    .fingers
                    .iter()
                    .filter(|f| f.validated)
                    .max_by_key(|f| f.hits)
                    .map(|f| f.id);
                for (fid, blocks) in per_finger_output {
                    if winner_id == Some(fid) || winner_id.is_none() {
                        produced_blocks.extend(blocks);
                    }
                }

                // Prune dead fingers
                self.fingers.retain(|f| {
                    if f.validated {
                        return true;
                    }
                    if f.misses > FINGER_DEAD_MISSES {
                        debug!(
                            "rake(fast): pruning dead finger {} (phase={} hits={} misses={})",
                            f.id, f.phase, f.hits, f.misses
                        );
                        return false;
                    }
                    true
                });

                let iter_total_us = iter_start.elapsed().as_micros() as u64;
                self.timing_blocks = self.timing_blocks.saturating_add(1);
                self.timing_total_us = self.timing_total_us.saturating_add(iter_total_us);
                self.timing_total_max_us = self.timing_total_max_us.max(iter_total_us);
            }

            // Periodic stats
            if self.last_stats_log.elapsed().as_secs() >= 1 {
                let tracked: Vec<String> = self
                    .fingers
                    .iter()
                    .map(|f| {
                        format!(
                            "f{}:ph{}:h{}:m{}:{}",
                            f.id,
                            f.phase,
                            f.hits,
                            f.misses,
                            if f.validated { "V" } else { "." }
                        )
                    })
                    .collect();
                info!(
                    "rake(fast): tracked=[{}] blocks={} total={}ms(avg={}us max={}us)",
                    tracked.join(", "),
                    self.timing_blocks,
                    self.timing_total_us / 1000,
                    self.timing_total_us / (self.timing_blocks as u64).max(1),
                    self.timing_total_max_us,
                );
                if !self.timing_stage_us.is_empty() {
                    let stage_parts = self
                        .timing_stage_us
                        .iter()
                        .map(|(name, total_us)| {
                            let calls = self.timing_stage_calls.get(name).copied().unwrap_or(0);
                            format!(
                                "{}={}ms(calls={} avg={}us)",
                                name,
                                total_us / 1000,
                                calls,
                                if calls > 0 { total_us / calls } else { 0 }
                            )
                        })
                        .collect::<Vec<_>>();
                    info!("rake_stage_timing(fast): {}", stage_parts.join(" "));
                }
                self.timing_blocks = 0;
                self.timing_total_us = 0;
                self.timing_total_max_us = 0;
                self.timing_fft_us = 0;
                self.timing_peaks_us = 0;
                self.timing_track_us = 0;
                self.timing_spawn_us = 0;
                self.timing_finger_us = 0;
                self.timing_select_prune_us = 0;
                self.timing_stage_us.clear();
                self.timing_stage_calls.clear();
                self.timing_finger_wrapper_us.clear();
                self.timing_finger_wrapper_calls.clear();
                self.last_stats_log = Instant::now();
            }

            return produced_blocks;
        }

        while self.buffer.len() >= self.fft_length {
            let iter_start = Instant::now();

            // 1. FFT cross-correlation
            let t_fft = Instant::now();
            let mut signal_fft = self.buffer[0..self.fft_length].to_vec();
            self.fft_planner
                .process_with_scratch(&mut signal_fft, &mut self.fft_scratch);

            let mut multiplied: Vec<Complex32> = signal_fft
                .iter()
                .zip(self.fft_pn.iter())
                .map(|(a, b)| a * b)
                .collect();
            self.fft_planner_inverse.process(&mut multiplied);
            for v in &mut multiplied {
                *v /= self.fft_length as f32;
            }
            self.timing_fft_us = self
                .timing_fft_us
                .saturating_add(t_fft.elapsed().as_micros() as u64);

            // Median for normalization
            let mut powers: Vec<f32> = multiplied[self.overlap..self.fft_length]
                .iter()
                .map(|x| x.norm_sqr())
                .collect();
            let mid = powers.len() / 2;
            powers.select_nth_unstable_by(mid, |a, b| a.partial_cmp(b).unwrap());
            let median = powers[mid];

            // 2. Extract peaks
            let t_peaks = Instant::now();
            let peaks = self.extract_peaks(&multiplied, median);
            self.timing_peaks_us = self
                .timing_peaks_us
                .saturating_add(t_peaks.elapsed().as_micros() as u64);
            if !peaks.is_empty() {
                let top_n: Vec<String> = peaks
                    .iter()
                    .take(10)
                    .map(|(ph, pw)| format!("ph{}:{:.1}", ph, pw))
                    .collect();
                debug!(
                    "rake: block {} peaks=[{}]",
                    self.sample / self.buffer_samples,
                    top_n.join(", ")
                );
            }

            // 3. Track existing fingers
            let t_track = Instant::now();
            for (n, finger) in self.fingers.iter_mut().enumerate() {
                let expected_phase = (finger.phase + self.buffer_samples) % self.phase_period;
                let mut local_best_power = 0.0f32;
                let mut local_best_phase = expected_phase;
                let mut found_local = false;

                for rel in 0..=(2 * TRACK_SEARCH_HALF_WINDOW) {
                    let offset = rel as isize - TRACK_SEARCH_HALF_WINDOW as isize;
                    let cand_phase = (expected_phase as isize + offset)
                        .rem_euclid(self.phase_period as isize)
                        as usize;
                    let cand_rel = (self.phase_period - cand_phase) % self.phase_period;
                    let cand_idx = self.overlap + cand_rel;

                    if cand_idx < multiplied.len() {
                        let p = multiplied[cand_idx].norm_sqr() / median;
                        if !found_local || p > local_best_power {
                            found_local = true;
                            local_best_power = p;
                            local_best_phase = cand_phase;
                        }
                    }
                }

                finger.last_snr = local_best_power;
                if found_local && local_best_power > TRACK_THRESHOLD {
                    trace!(
                        "finger {} changed phase: {} -> {}",
                        n, expected_phase, local_best_phase
                    );
                    finger.phase = local_best_phase;
                    finger.hits += 1;
                    if finger.misses > 0 {
                        finger.misses -= 1;
                    }
                } else {
                    finger.phase = expected_phase;
                    finger.misses += 1;
                    if finger.hits > 0 {
                        finger.hits -= 1;
                    }
                }
            }
            self.timing_track_us = self
                .timing_track_us
                .saturating_add(t_track.elapsed().as_micros() as u64);

            // 4. Spawn new fingers for unmatched peaks
            let t_spawn = Instant::now();
            let has_validated = self.fingers.iter().any(|f| f.validated);
            if !has_validated {
                let min_sep = MIN_PEAK_SEPARATION_CHIPS * self.oversample;
                for (peak_phase, _peak_power) in &peaks {
                    let too_close = self
                        .fingers
                        .iter()
                        .any(|f| phase_distance(f.phase, *peak_phase, self.phase_period) < min_sep);
                    if !too_close && self.fingers.len() < MAX_FINGERS {
                        self.spawn_finger(*peak_phase);
                    }
                }
            }
            self.timing_spawn_us = self
                .timing_spawn_us
                .saturating_add(t_spawn.elapsed().as_micros() as u64);

            // 5. Despread and feed through each finger's chain
            let t_finger = Instant::now();
            let input_samples: Vec<Complex32> = self.buffer[0..self.buffer_samples].to_vec();
            self.sample += self.buffer_samples;
            self.buffer.drain(0..self.buffer_samples);

            let block_chip_start = self
                .input_origin_sample
                .unwrap_or(0)
                .saturating_add(self.sample.saturating_sub(self.buffer_samples));

            // Collect output per-finger so we can filter after validation check.
            let mut per_finger_output: Vec<(usize, Vec<SampleBlock>)> = Vec::new();
            for finger in self.fingers.iter_mut() {
                let (pn_seq, despread_phase_offset) = if self.use_raw_pn_despreading {
                    (&self.pn_seq_despread, self.despread_phase_offset)
                } else {
                    (&self.pn_seq_filtered, 0)
                };
                let out = Self::feed_finger(
                    pn_seq,
                    despread_phase_offset,
                    finger,
                    &input_samples,
                    self.oversample,
                    self.phase_period,
                    block_chip_start,
                    input_sample_rate_hz,
                    &block.tags,
                    self.input_origin_sample.unwrap_or(0),
                    self.absolute_origin_sample,
                    &mut self.timing_stage_us,
                    &mut self.timing_stage_calls,
                    &mut self.timing_finger_wrapper_us,
                    &mut self.timing_finger_wrapper_calls,
                );
                per_finger_output.push((finger.id, out));
            }
            self.timing_finger_us = self
                .timing_finger_us
                .saturating_add(t_finger.elapsed().as_micros() as u64);

            // Single-user mode: only forward output from one validated finger.
            // If multiple fingers validate in the same cycle, pick the one
            // with the most hits (strongest tracking). For multi-user, this
            // would need to group by downstream identity key and keep one
            // winner per group (see top-of-file comments).
            let t_select_prune = Instant::now();
            let winner_id = self
                .fingers
                .iter()
                .filter(|f| f.validated)
                .max_by_key(|f| f.hits)
                .map(|f| f.id);
            if let Some(wid) = winner_id {
                if let Some(winner) = self.fingers.iter().find(|f| f.id == wid) {
                    debug!(
                        "rake: winner finger {} phase={} hits={} misses={}",
                        winner.id, winner.phase, winner.hits, winner.misses
                    );
                }
            }
            for (fid, blocks) in per_finger_output {
                if winner_id == Some(fid) || winner_id.is_none() {
                    produced_blocks.extend(blocks);
                }
            }
            // Demote non-winner validated fingers so only one survives pruning.
            if let Some(wid) = winner_id {
                for finger in self.fingers.iter_mut() {
                    if finger.validated && finger.id != wid {
                        debug!(
                            "rake: demoting validated finger {} phase={} hits={} misses={} (winner={})",
                            finger.id, finger.phase, finger.hits, finger.misses, wid
                        );
                        finger.validated = false;
                    }
                }
            }

            // 6. Prune dead fingers
            self.fingers.retain(|f| {
                if f.validated {
                    return true;
                }
                if f.misses > FINGER_DEAD_MISSES {
                    debug!(
                        "rake: pruning dead finger {} (phase={} hits={} misses={})",
                        f.id, f.phase, f.hits, f.misses
                    );
                    return false;
                }
                true
            });

            // Once validated, kill non-validated fingers
            if has_validated {
                self.fingers.retain(|f| {
                    if !f.validated {
                        debug!(
                            "rake: pruning non-validated finger {} (phase={} hits={} misses={} validated finger exists)",
                            f.id, f.phase, f.hits, f.misses
                        );
                    }
                    f.validated
                });
            }
            self.timing_select_prune_us = self
                .timing_select_prune_us
                .saturating_add(t_select_prune.elapsed().as_micros() as u64);

            let iter_total_us = iter_start.elapsed().as_micros() as u64;
            self.timing_blocks = self.timing_blocks.saturating_add(1);
            self.timing_total_us = self.timing_total_us.saturating_add(iter_total_us);
            self.timing_total_max_us = self.timing_total_max_us.max(iter_total_us);

            // Periodic stats (once per second at info level)
            if self.last_stats_log.elapsed().as_secs() >= 1 {
                let tracked: Vec<String> = self
                    .fingers
                    .iter()
                    .filter(|f| f.hits > 2)
                    .map(|f| {
                        format!(
                            "f{}:ph{}:snr{:.1}:h{}:m{}:{}",
                            f.id,
                            f.phase,
                            f.last_snr,
                            f.hits,
                            f.misses,
                            if f.validated { "V" } else { "." }
                        )
                    })
                    .collect();
                if tracked.is_empty() {
                    info!("rake: no tracked fingers (total={})", self.fingers.len());
                } else {
                    info!(
                        "rake: tracked=[{}] (total={})",
                        tracked.join(", "),
                        self.fingers.len()
                    );
                }
                let blocks = self.timing_blocks.max(1) as u64;
                info!(
                    "rake_timing: blocks={} fft={}ms peaks={}ms track={}ms spawn={}ms finger={}ms select_prune={}ms total={}ms(avg={}us max={}us)",
                    self.timing_blocks,
                    self.timing_fft_us / 1000,
                    self.timing_peaks_us / 1000,
                    self.timing_track_us / 1000,
                    self.timing_spawn_us / 1000,
                    self.timing_finger_us / 1000,
                    self.timing_select_prune_us / 1000,
                    self.timing_total_us / 1000,
                    self.timing_total_us / blocks,
                    self.timing_total_max_us,
                );
                if !self.timing_stage_us.is_empty() {
                    let stage_parts = self
                        .timing_stage_us
                        .iter()
                        .map(|(name, total_us)| {
                            let calls = self.timing_stage_calls.get(name).copied().unwrap_or(0);
                            format!(
                                "{}={}ms(calls={} avg={}us)",
                                name,
                                total_us / 1000,
                                calls,
                                if calls > 0 { total_us / calls } else { 0 }
                            )
                        })
                        .collect::<Vec<_>>();
                    info!("rake_stage_timing: {}", stage_parts.join(" "));
                }
                if !self.timing_finger_wrapper_us.is_empty() {
                    let wrapper_parts = self
                        .timing_finger_wrapper_us
                        .iter()
                        .map(|(name, total_us)| {
                            let calls = self
                                .timing_finger_wrapper_calls
                                .get(name)
                                .copied()
                                .unwrap_or(0);
                            format!(
                                "{}={}ms(calls={} avg={}us)",
                                name,
                                total_us / 1000,
                                calls,
                                if calls > 0 { total_us / calls } else { 0 }
                            )
                        })
                        .collect::<Vec<_>>();
                    info!("rake_finger_timing: {}", wrapper_parts.join(" "));
                }
                self.timing_blocks = 0;
                self.timing_fft_us = 0;
                self.timing_peaks_us = 0;
                self.timing_track_us = 0;
                self.timing_spawn_us = 0;
                self.timing_finger_us = 0;
                self.timing_select_prune_us = 0;
                self.timing_total_us = 0;
                self.timing_total_max_us = 0;
                self.timing_stage_us.clear();
                self.timing_stage_calls.clear();
                self.timing_finger_wrapper_us.clear();
                self.timing_finger_wrapper_calls.clear();
                self.last_stats_log = Instant::now();
            }
        }

        produced_blocks
    }

    fn name(&self) -> &'static str {
        "RakeReceiver"
    }

    fn flush(&mut self) -> Vec<SampleBlock> {
        let mut emitter = super::VecEmitter::new();
        let mut out = Vec::new();
        for finger in &mut self.fingers {
            out.extend(flush_sub_chain(&mut finger.chain, &mut emitter));
        }
        out.extend(emitter.blocks);
        out
    }
}

fn phase_distance(a: usize, b: usize, period: usize) -> usize {
    let d = (a as isize - b as isize).rem_euclid(period as isize) as usize;
    d.min(period - d)
}
