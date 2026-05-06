use log::debug;
use num_complex::Complex32;

use crate::phy::coding::block_interleaver::BitReversalInterleaver;
use crate::phy::coding::convolutional::{SoftViterbiDecoder, get_1_2_k9_encoder};

use super::{
    PipelineProcessor, SampleBlock, chips_per_sample, sync_channel_processor::SyncChannelProcessor,
};

/// Block deinterleaver. Accumulates a full interleaver block then
/// deinterleaves. Optionally un-repeats within each block (takes
/// every `deinterleave_repeats`-th symbol).
pub struct DeinterleaverProcessor {
    interleaver: BitReversalInterleaver,
    deinterleave_repeats: usize,
    buffer: Vec<Complex32>,
    buffer_tags: std::collections::HashMap<&'static str, i64>,
    buffer_chip_start: usize,
    buffer_sample_rate_hz: f64,
    offset_search: Option<OffsetSearchState>,
    reset_on_tag: Option<&'static str>,
}

/// Evaluates a candidate alignment: given decoded bits, a frame shift, and
/// whether to invert, returns `(messages_parsed, crc_valid_frames)`.
type AlignmentEvaluator = Box<dyn Fn(&[u8], usize, bool) -> (usize, usize) + Send>;

/// Soft evaluator: given deinterleaved soft symbols (f32, pre-Viterbi),
/// returns `(messages_parsed, crc_valid_frames)`.  Used when the offset
/// search must apply a non-R=1/2 Viterbi decoder (e.g. R=1/3 for access
/// channel).
type SoftAlignmentEvaluator = Box<dyn Fn(&[f32]) -> (usize, usize) + Send>;

struct OffsetSearchState {
    candidates: Vec<usize>,
    probe_blocks: usize,
    min_crc_valid: usize,
    warmup_input_blocks: usize,
    input_blocks_seen: usize,
    max_candidates_per_call: usize,
    next_candidate_idx: usize,
    current_pass_scores: Vec<Option<OffsetProbeScore>>,
    confirm_passes_required: usize,
    confirm_hits: usize,
    last_best: Option<OffsetProbeScore>,
    locked_offset: Option<usize>,
    lock_shift: Option<usize>,
    lock_invert: Option<bool>,
    /// Custom alignment evaluator. If None, uses SyncChannelProcessor default.
    evaluator: Option<AlignmentEvaluator>,
    /// Soft evaluator (pre-Viterbi). If set, overrides both `evaluator` and
    /// the default R=1/2 Viterbi path.
    soft_evaluator: Option<SoftAlignmentEvaluator>,
    /// Number of frame shifts to try (e.g. 32 for sync, 96 for 9600 paging).
    frame_size: usize,
    /// Hard bound: if probing hasn't locked by this many input blocks,
    /// force-lock to a fallback/best-known candidate.
    max_input_blocks: Option<usize>,
    fallback_offset: usize,
    fallback_shift: usize,
    fallback_invert: bool,
}

#[derive(Clone, Copy)]
struct OffsetProbeScore {
    offset: usize,
    crc_valid_frames: usize,
    sync_messages: usize,
    best_shift: usize,
    best_invert: bool,
}

impl DeinterleaverProcessor {
    pub fn new(interleaver: BitReversalInterleaver, deinterleave_repeats: usize) -> Self {
        Self {
            interleaver,
            deinterleave_repeats: deinterleave_repeats.max(1),
            buffer: Vec::new(),
            buffer_tags: std::collections::HashMap::new(),
            buffer_chip_start: 0,
            buffer_sample_rate_hz: 0.0,
            offset_search: None,
            reset_on_tag: None,
        }
    }

    /// Probe candidate interleaver offsets during startup and lock to the best one.
    ///
    /// Candidate quality is scored by CRC-valid sync frames after
    /// deinterleave+soft-viterbi decode. Once locked, normal processing continues
    /// with the chosen offset.
    pub fn with_offset_search(
        mut self,
        mut candidates: Vec<usize>,
        probe_blocks: usize,
        min_crc_valid: usize,
    ) -> Self {
        candidates.retain(|o| *o < self.interleaver.block_len());
        candidates.sort_unstable();
        candidates.dedup();
        if candidates.is_empty() {
            return self;
        }
        let fallback_offset = candidates
            .iter()
            .copied()
            .find(|o| *o == 0)
            .unwrap_or(candidates[0]);
        self.offset_search = Some(OffsetSearchState {
            candidates,
            probe_blocks: probe_blocks.max(1),
            min_crc_valid,
            warmup_input_blocks: 0,
            input_blocks_seen: 0,
            max_candidates_per_call: 4,
            next_candidate_idx: 0,
            current_pass_scores: Vec::new(),
            confirm_passes_required: 2,
            confirm_hits: 0,
            last_best: None,
            locked_offset: None,
            lock_shift: None,
            lock_invert: None,
            evaluator: None,
            soft_evaluator: None,
            frame_size: 32,
            max_input_blocks: Some(2048),
            fallback_offset,
            fallback_shift: 0,
            fallback_invert: false,
        });
        if let Some(search) = self.offset_search.as_mut() {
            search.current_pass_scores = vec![None; search.candidates.len()];
        }
        self
    }

    /// Delay probing until at least `warmup_input_blocks` have arrived.
    pub fn with_offset_search_warmup(mut self, warmup_input_blocks: usize) -> Self {
        if let Some(search) = self.offset_search.as_mut() {
            search.warmup_input_blocks = warmup_input_blocks;
        }
        self
    }

    /// Incremental probe work budget per input block.
    pub fn with_offset_search_batch_size(mut self, max_candidates_per_call: usize) -> Self {
        if let Some(search) = self.offset_search.as_mut() {
            search.max_candidates_per_call = max_candidates_per_call.max(1);
        }
        self
    }

    /// Hard bound for probing duration. If lock is not achieved by this many
    /// input blocks, force-lock to best-known/fallback candidate.
    pub fn with_offset_search_max_input_blocks(mut self, max_input_blocks: usize) -> Self {
        if let Some(search) = self.offset_search.as_mut() {
            search.max_input_blocks = Some(max_input_blocks.max(1));
        }
        self
    }

    /// Set a custom alignment evaluator and frame size for offset search.
    ///
    /// The evaluator receives `(decoded_bits, shift, invert)` and returns
    /// `(messages_parsed, crc_valid_frames)`.  `frame_size` controls how many
    /// shifts are tried (0..frame_size).
    pub fn with_offset_search_evaluator(
        mut self,
        evaluator: Box<dyn Fn(&[u8], usize, bool) -> (usize, usize) + Send>,
        frame_size: usize,
    ) -> Self {
        if let Some(search) = self.offset_search.as_mut() {
            search.evaluator = Some(evaluator);
            search.frame_size = frame_size;
        }
        self
    }

    /// Set a soft evaluator for offset search.  When set, the offset search
    /// passes deinterleaved soft symbols (f32) directly to this evaluator
    /// instead of running the built-in R=1/2 Viterbi decoder.  Use this for
    /// non-R=1/2 channels (e.g. R=1/3 access channel).
    pub fn with_offset_search_soft_evaluator(
        mut self,
        evaluator: Box<dyn Fn(&[f32]) -> (usize, usize) + Send>,
    ) -> Self {
        if let Some(search) = self.offset_search.as_mut() {
            search.soft_evaluator = Some(evaluator);
        }
        self
    }

    /// Require this many consecutive probe passes to agree on the best lock.
    pub fn with_offset_search_confirm_passes(mut self, confirm_passes_required: usize) -> Self {
        if let Some(search) = self.offset_search.as_mut() {
            search.confirm_passes_required = confirm_passes_required.max(1);
        }
        self
    }

    /// Reset offset search/lock when `tag` appears with value 1 on input blocks.
    pub fn with_reset_on_tag(mut self, tag: &'static str) -> Self {
        self.reset_on_tag = Some(tag);
        self
    }

    fn maybe_reset_on_tag(&mut self, tags: &std::collections::HashMap<&'static str, i64>) {
        let should_reset = self
            .reset_on_tag
            .and_then(|tag| tags.get(tag))
            .copied()
            .unwrap_or(0)
            == 1;
        if !should_reset {
            return;
        }
        if let Some(search) = self.offset_search.as_mut() {
            search.locked_offset = None;
            search.lock_shift = None;
            search.lock_invert = None;
            search.input_blocks_seen = 0;
            search.next_candidate_idx = 0;
            search.current_pass_scores.fill(None);
            search.confirm_hits = 0;
            search.last_best = None;
        }
        self.buffer.clear();
    }

    fn evaluate_offset_candidate(
        &self,
        offset: usize,
        probe_blocks: usize,
    ) -> Option<OffsetProbeScore> {
        let block_len = self.interleaver.block_len();
        let max_offset = block_len.saturating_sub(1);
        let required = max_offset.saturating_add(probe_blocks.saturating_mul(block_len));
        if self.buffer.len() < required {
            return None;
        }
        // Probe against the newest window, not the oldest startup data.
        let mut window_base = self.buffer.len().saturating_sub(required);
        // Keep probe window anchored to block boundaries so candidate offsets
        // are comparable across passes.
        window_base = window_base.saturating_sub(window_base % block_len);
        let start = window_base + offset;
        let end = start + probe_blocks * block_len;
        if self.buffer.len() < end {
            return None;
        }
        let slice = &self.buffer[start..end];
        let mut soft = Vec::with_capacity(probe_blocks * block_len / self.deinterleave_repeats);

        for chunk in slice.chunks_exact(block_len) {
            let in_soft: Vec<f32> = chunk.iter().map(|s| s.re).collect();
            let deinterleaved = self.interleaver.decode_soft(&in_soft);
            soft.extend(
                deinterleaved
                    .chunks_exact(self.deinterleave_repeats)
                    .map(|c| c.iter().sum::<f32>() / c.len() as f32),
            );
        }

        // If a soft evaluator is available, use it directly on the
        // deinterleaved soft symbols (skipping the built-in R=1/2 Viterbi).
        if let Some(search) = self.offset_search.as_ref()
            && let Some(soft_eval) = search.soft_evaluator.as_ref()
        {
            let (msgs, valid) = soft_eval(&soft);
            return Some(OffsetProbeScore {
                offset,
                crc_valid_frames: valid,
                sync_messages: msgs,
                best_shift: 0,
                best_invert: false,
            });
        }

        let peak = soft.iter().map(|v| v.abs()).fold(0.0f32, f32::max);
        let inv_peak = if peak > 1e-12 { 1.0 / peak } else { 1.0 };
        // Fresh decoder per candidate: no cross-candidate state carryover.
        let mut decoder = SoftViterbiDecoder::new(get_1_2_k9_encoder());
        let mut decoded = Vec::new();
        for pair in soft.chunks_exact(2) {
            let input = [
                (0.5 - pair[0] * 0.5 * inv_peak).clamp(0.0, 1.0),
                (0.5 - pair[1] * 0.5 * inv_peak).clamp(0.0, 1.0),
            ];
            if let Some(bit) = decoder.process(&input) {
                decoded.push(bit);
            }
        }
        decoded.extend(decoder.finish());

        let mut candidate_score = OffsetProbeScore {
            offset,
            crc_valid_frames: 0,
            sync_messages: 0,
            best_shift: 0,
            best_invert: false,
        };
        let (evaluator_fn, frame_size): (&dyn Fn(&[u8], usize, bool) -> (usize, usize), usize) =
            match self
                .offset_search
                .as_ref()
                .and_then(|s| s.evaluator.as_ref())
            {
                Some(eval) => {
                    let fs = self.offset_search.as_ref().map_or(32, |s| s.frame_size);
                    (eval.as_ref(), fs)
                }
                None => (&SyncChannelProcessor::evaluate_alignment, 32),
            };
        for invert in [false, true] {
            for shift in 0..frame_size {
                let (msgs, valid) = evaluator_fn(&decoded, shift, invert);
                if valid > candidate_score.crc_valid_frames
                    || (valid == candidate_score.crc_valid_frames
                        && msgs > candidate_score.sync_messages)
                {
                    candidate_score.crc_valid_frames = valid;
                    candidate_score.sync_messages = msgs;
                    candidate_score.best_shift = shift;
                    candidate_score.best_invert = invert;
                }
            }
        }
        Some(candidate_score)
    }

    fn required_probe_samples(&self, search: &OffsetSearchState) -> usize {
        let block_len = self.interleaver.block_len();
        search
            .candidates
            .iter()
            .copied()
            .max()
            .unwrap_or(0)
            .saturating_add(search.probe_blocks.saturating_mul(block_len))
    }
}

impl PipelineProcessor for DeinterleaverProcessor {
    fn process_block(&mut self, block: SampleBlock) -> Vec<SampleBlock> {
        // Pass through empty event blocks (e.g. preamble detection) unchanged.
        if block.samples.is_empty() {
            return vec![block];
        }
        self.maybe_reset_on_tag(&block.tags);
        if let Some(search) = self.offset_search.as_mut() {
            search.input_blocks_seen = search.input_blocks_seen.saturating_add(1);
        }
        if self.buffer.is_empty() {
            self.buffer_tags = block.tags.clone();
            self.buffer_chip_start = block.chip_start;
            self.buffer_sample_rate_hz = block.sample_rate_hz;
        }
        self.buffer.extend_from_slice(&block.samples);
        let block_len = self.interleaver.block_len();
        let mut out_samples = Vec::new();

        let mut should_lock: Option<OffsetProbeScore> = None;
        let mut lock_forced = false;
        let probe_ready = self.offset_search.as_ref().and_then(|search| {
            let required_samples = self.required_probe_samples(search);
            (search.locked_offset.is_none()
                && search.input_blocks_seen >= search.warmup_input_blocks
                && self.buffer.len() >= required_samples)
                .then_some((search.probe_blocks, search.max_candidates_per_call))
        });
        if let Some((probe_blocks, _max_candidates_per_call)) = probe_ready {
            let eval_offsets: Vec<(usize, usize)> = {
                let search = self.offset_search.as_mut().unwrap();
                let mut v = Vec::new();
                for _ in 0..search.max_candidates_per_call {
                    if search.candidates.is_empty() {
                        break;
                    }
                    let idx = search.next_candidate_idx % search.candidates.len();
                    let offset = search.candidates[idx];
                    search.next_candidate_idx += 1;
                    v.push((idx, offset));
                }
                v
            };

            for (idx, offset) in eval_offsets {
                if let Some(score) = self.evaluate_offset_candidate(offset, probe_blocks)
                    && let Some(search) = self.offset_search.as_mut()
                {
                    search.current_pass_scores[idx] = Some(score);
                }
            }

            if let Some(search) = self.offset_search.as_mut()
                && search.current_pass_scores.iter().all(Option::is_some)
            {
                let mut scores: Vec<OffsetProbeScore> = search
                    .current_pass_scores
                    .iter()
                    .flatten()
                    .copied()
                    .collect();
                scores.sort_by(|a, b| {
                    b.crc_valid_frames
                        .cmp(&a.crc_valid_frames)
                        .then_with(|| b.sync_messages.cmp(&a.sync_messages))
                });
                debug!(
                    "deinterleaver offset probe pass: warmup_blocks_seen={} candidates={} top={}",
                    search.input_blocks_seen,
                    scores.len(),
                    scores
                        .iter()
                        .take(8)
                        .map(|s| format!(
                            "off{}:crc{}:sync{}:sh{}:inv{}",
                            s.offset,
                            s.crc_valid_frames,
                            s.sync_messages,
                            s.best_shift,
                            s.best_invert as u8
                        ))
                        .collect::<Vec<_>>()
                        .join(", ")
                );
                if let Some(best) = scores.first().copied()
                    && best.crc_valid_frames >= search.min_crc_valid
                {
                    let same_as_last = search.last_best.is_some_and(|prev| {
                        prev.offset == best.offset
                            && prev.best_shift == best.best_shift
                            && prev.best_invert == best.best_invert
                    });
                    if same_as_last {
                        search.confirm_hits += 1;
                    } else {
                        search.confirm_hits = 1;
                        search.last_best = Some(best);
                    }
                    if search.confirm_hits >= search.confirm_passes_required {
                        should_lock = Some(best);
                    }
                }
                search.current_pass_scores.fill(None);
            }
        }

        if should_lock.is_none()
            && let Some(search) = self.offset_search.as_ref()
            && search.locked_offset.is_none()
            && search
                .max_input_blocks
                .is_some_and(|max| search.input_blocks_seen >= max)
        {
            should_lock = Some(search.last_best.unwrap_or(OffsetProbeScore {
                offset: search.fallback_offset,
                crc_valid_frames: 0,
                sync_messages: 0,
                best_shift: search.fallback_shift,
                best_invert: search.fallback_invert,
            }));
            lock_forced = true;
        }

        if let Some(best) = should_lock
            && let Some(search) = self.offset_search.as_mut()
            && search.locked_offset.is_none()
        {
            // Apply lock at the same tail-anchored probe window used during scoring.
            let required = search
                .candidates
                .iter()
                .copied()
                .max()
                .unwrap_or(0)
                .saturating_add(search.probe_blocks.saturating_mul(block_len));
            let mut window_base = self.buffer.len().saturating_sub(required);
            window_base = window_base.saturating_sub(window_base % block_len);
            let drain_to = window_base.saturating_add(best.offset);

            search.locked_offset = Some(best.offset);
            search.lock_shift = Some(best.best_shift);
            search.lock_invert = Some(best.best_invert);
            if drain_to > 0 && self.buffer.len() >= drain_to {
                self.buffer.drain(..drain_to);
                // Advance chip_start in chip-rate units (drain_to is in
                // samples at buffer_sample_rate_hz).
                let cps = chips_per_sample(self.buffer_sample_rate_hz);
                self.buffer_chip_start = self.buffer_chip_start.saturating_add(drain_to * cps);
            }
            self.buffer_tags
                .insert("deinterleaver_locked_offset", best.offset as i64);
            self.buffer_tags
                .insert("deinterleaver_locked_shift", best.best_shift as i64);
            self.buffer_tags
                .insert("deinterleaver_locked_invert", best.best_invert as i64);
            self.buffer_tags
                .insert("deinterleaver_lock_crc_valid", best.crc_valid_frames as i64);
            self.buffer_tags
                .insert("deinterleaver_lock_sync_msgs", best.sync_messages as i64);
            self.buffer_tags
                .insert("deinterleaver_lock_forced", lock_forced as i64);
            if lock_forced {
                debug!(
                    "deinterleaver offset force-lock: offset={} crc_valid={} sync_msgs={} shift={} invert={} after_blocks={}",
                    best.offset,
                    best.crc_valid_frames,
                    best.sync_messages,
                    best.best_shift,
                    best.best_invert as u8,
                    search.input_blocks_seen
                );
            }
            debug!(
                "deinterleaver offset lock: offset={} base={} drain_to={} crc_valid={} sync_msgs={} shift={} invert={}",
                best.offset,
                window_base,
                drain_to,
                best.crc_valid_frames,
                best.sync_messages,
                best.best_shift,
                best.best_invert as u8
            );
        }

        if self
            .offset_search
            .as_ref()
            .is_some_and(|search| search.locked_offset.is_none())
        {
            return Vec::new();
        }

        let cps = chips_per_sample(self.buffer_sample_rate_hz);
        let out_chip_start = self.buffer_chip_start;
        while self.buffer.len() >= block_len {
            let chunk: Vec<f32> = self.buffer.drain(..block_len).map(|s| s.re).collect();
            let deinterleaved = self.interleaver.decode_soft(&chunk);

            out_samples.extend(
                deinterleaved
                    .chunks_exact(self.deinterleave_repeats)
                    .map(|c| {
                        let avg: f32 = c.iter().sum::<f32>() / c.len() as f32;
                        Complex32::new(avg, 0.0)
                    }),
            );
            // Advance chip_start past the consumed interleaver block.
            self.buffer_chip_start += block_len * cps;
        }

        if out_samples.is_empty() {
            return Vec::new();
        }

        let out_rate = if self.buffer_sample_rate_hz > 0.0 {
            self.buffer_sample_rate_hz / self.deinterleave_repeats as f64
        } else {
            0.0
        };
        let mut out_block =
            SampleBlock::new(out_samples, out_chip_start).with_sample_rate_hz(out_rate);
        out_block.tags = self.buffer_tags.clone();
        if let Some(search) = self.offset_search.as_ref()
            && let Some(offset) = search.locked_offset
        {
            out_block
                .tags
                .insert("deinterleaver_locked_offset", offset as i64);
            if let Some(shift) = search.lock_shift {
                out_block
                    .tags
                    .insert("deinterleaver_locked_shift", shift as i64);
            }
            if let Some(invert) = search.lock_invert {
                out_block
                    .tags
                    .insert("deinterleaver_locked_invert", invert as i64);
            }
        }
        if !self.buffer.is_empty() {
            self.buffer_tags = block.tags;
        }
        vec![out_block]
    }
}

#[cfg(test)]
mod tests {
    use num_complex::Complex32;

    use super::DeinterleaverProcessor;
    use crate::{
        phy::coding::block_interleaver::{BitReversalInterleaver, SR1_PARAMS_48},
        receiver::pipelined::{PipelineProcessor, SampleBlock},
    };

    #[test]
    fn test_deinterleaver_processor_reverses_interleaving() {
        let mut i = BitReversalInterleaver::new(SR1_PARAMS_48);
        let original = (0..48u8).map(|v| v % 2).collect::<Vec<_>>();
        let interleaved = i.encode(&original);

        let mut p = DeinterleaverProcessor::new(BitReversalInterleaver::new(SR1_PARAMS_48), 1);
        let block = SampleBlock::new(
            interleaved
                .iter()
                .map(|b| Complex32::new(*b as f32, 0.0))
                .collect(),
            123,
        );

        let out = p.process_block(block);
        assert_eq!(1, out.len());
        let out_bits: Vec<u8> = out[0].samples.iter().map(|s| s.re as u8).collect();
        assert_eq!(original, out_bits);
        assert_eq!(123, out[0].chip_start);
    }

    #[test]
    fn test_deinterleaver_processor_with_repeat_stride() {
        let mut p = DeinterleaverProcessor::new(BitReversalInterleaver::new(SR1_PARAMS_48), 2);
        let block = SampleBlock::new(vec![Complex32::new(1.0, 0.0); 48], 0);
        let out = p.process_block(block);
        assert_eq!(1, out.len());
        assert_eq!(24, out[0].len());
    }

    #[test]
    fn test_deinterleaver_offset_search_force_locks_after_budget() {
        let mut p = DeinterleaverProcessor::new(BitReversalInterleaver::new(SR1_PARAMS_48), 1)
            .with_offset_search((0..48).collect(), 4, 999)
            .with_offset_search_max_input_blocks(1);

        let block = SampleBlock::new(vec![Complex32::new(1.0, 0.0); 48], 0);
        let out = p.process_block(block);

        assert_eq!(1, out.len());
        assert_eq!(48, out[0].len());
        assert_eq!(Some(&1), out[0].tags.get("deinterleaver_lock_forced"));
    }
}
