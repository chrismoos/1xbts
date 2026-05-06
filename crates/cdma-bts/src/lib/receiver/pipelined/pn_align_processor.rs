use super::{PipelineProcessor, SampleBlock};

/// One-shot PN phase aligner.
///
/// Drops leading samples so output starts aligned to the pilot PN epoch
/// modulo the PN period.  The target phase is read from the `"pilot_phase"`
/// tag on the first incoming block (set by the rake receiver).  After
/// alignment the processor passes data through unchanged until reset.
pub struct PnAlignProcessor {
    oversample: usize,
    period: usize,
    aligned: bool,
    pending_drop: usize,
    additional_drop_samples: usize,
    reset_on_tag: Option<&'static str>,
    buffer: Vec<num_complex::Complex32>,
    buffer_tags: std::collections::HashMap<&'static str, i64>,
    buffer_chip_start: usize,
    buffer_sample_rate_hz: f64,
    /// Sample index (in the incoming oversampled timeline) of the first
    /// post-alignment PN epoch boundary. Outgoing chip_start values are
    /// rebased relative to this anchor so downstream sees PN-relative time.
    pn_epoch_sample: Option<usize>,
}

impl PnAlignProcessor {
    /// Create a one-shot PN aligner for the given oversample rate.
    pub fn new(oversample: usize) -> Self {
        let period = 32768usize.saturating_mul(oversample.max(1));
        Self {
            oversample: oversample.max(1),
            period,
            aligned: false,
            pending_drop: 0,
            additional_drop_samples: 0,
            reset_on_tag: None,
            buffer: Vec::new(),
            buffer_tags: std::collections::HashMap::new(),
            buffer_chip_start: 0,
            buffer_sample_rate_hz: 0.0,
            pn_epoch_sample: None,
        }
    }

    /// Reset alignment state when the named tag is asserted upstream.
    pub fn with_reset_on_tag(mut self, tag: &'static str) -> Self {
        self.reset_on_tag = Some(tag);
        self
    }

    /// Drop extra samples after the epoch boundary for test-time skewing.
    pub fn with_additional_drop_samples(mut self, samples: usize) -> Self {
        self.additional_drop_samples = samples;
        self
    }
}

impl PipelineProcessor for PnAlignProcessor {
    fn process_block(&mut self, mut block: SampleBlock) -> Vec<SampleBlock> {
        let should_reset = self
            .reset_on_tag
            .and_then(|t| block.tags.get(t))
            .copied()
            .unwrap_or(0)
            == 1;
        if should_reset {
            self.aligned = false;
            self.pending_drop = 0;
            self.buffer.clear();
            self.pn_epoch_sample = None;
        }

        // Pick up pilot_phase from the tag on the first block after reset.
        // The pilot_phase is the PN chip offset within the period where the
        // rake finger is despreading.  To align downstream output to PN
        // chip 0, drop (period - pilot_phase) samples so we advance past
        // the remaining chips of the current PN period.
        if !self.aligned && self.pending_drop == 0 {
            if let Some(&phase) = block.tags.get("pilot_phase") {
                let pilot = (phase as usize) % self.period;
                self.pending_drop = ((self.period - pilot) % self.period)
                    .saturating_add(self.additional_drop_samples);
                self.aligned = true;
            }
        }

        if self.pending_drop > 0 {
            let drop = self.pending_drop.min(block.samples.len());
            block.samples.drain(0..drop);
            block.chip_start = block.chip_start.saturating_add(drop);
            if let Some(v) = block.tags.get_mut("absolute_sample_start") {
                *v += drop as i64;
            }
            if let Some(v) = block.tags.get_mut("absolute_chip_start") {
                *v = (*v).saturating_add((drop / self.oversample.max(1)) as i64);
            }
            self.pending_drop -= drop;
        }

        // Latch PN epoch anchor once alignment has been established. This is
        // the first sample index with PN phase 0 in the current lock epoch.
        if self.aligned && self.pending_drop == 0 && self.pn_epoch_sample.is_none() {
            self.pn_epoch_sample = Some(block.chip_start);
        }

        if block.samples.is_empty() {
            return Vec::new();
        }

        if self.buffer.is_empty() {
            self.buffer_tags = block.tags.clone();
            if let Some(epoch) = self.pn_epoch_sample {
                self.buffer_chip_start = block.chip_start.saturating_sub(epoch);
                self.buffer_tags.insert("pn_epoch_sample", epoch as i64);
                if let Some(abs_sample_start) = block.tags.get("absolute_sample_start").copied() {
                    self.buffer_tags
                        .insert("absolute_pn_epoch_sample", abs_sample_start);
                }
            } else {
                self.buffer_chip_start = block.chip_start;
            }
            self.buffer_sample_rate_hz = block.sample_rate_hz;
        }
        self.buffer.extend_from_slice(&block.samples);

        let emit_granularity = self.oversample.saturating_mul(64);
        let emit_len = (self.buffer.len() / emit_granularity) * emit_granularity;
        if emit_len == 0 {
            return Vec::new();
        }

        let mut out = SampleBlock::new(
            self.buffer.drain(0..emit_len).collect::<Vec<_>>(),
            self.buffer_chip_start,
        )
        .with_sample_rate_hz(self.buffer_sample_rate_hz);
        out.tags = self.buffer_tags.clone();
        // Report instantaneous PN phase at the first emitted sample.
        // `chip_start` here is in oversampled sample units.
        let pn_phase_samples = out.chip_start % self.period;
        let _pn_phase_chips = pn_phase_samples / self.oversample;
        out.tags.insert("pn_phase_samples", pn_phase_samples as i64);
        //out.tags.insert("pn_phase", pn_phase_chips as i64);
        out.tags.insert("pn_phase", out.chip_start as i64);
        if let Some(abs_epoch) = out.tags.get("absolute_pn_epoch_sample").copied() {
            let absolute_sample_start = abs_epoch.saturating_add(out.chip_start as i64);
            out.tags
                .insert("absolute_sample_start", absolute_sample_start);
            out.tags.insert(
                "absolute_chip_start",
                absolute_sample_start / self.oversample.max(1) as i64,
            );
        }
        self.buffer_chip_start = self.buffer_chip_start.saturating_add(emit_len);
        if !self.buffer.is_empty() {
            self.buffer_tags = block.tags;
        }
        vec![out]
    }
}

#[cfg(test)]
mod tests {
    use num_complex::Complex32;

    use super::PnAlignProcessor;
    use crate::receiver::pipelined::{PipelineProcessor, SampleBlock};

    #[test]
    fn test_pn_align_processor_aligns_to_pilot_phase_tag() {
        // oversample=1 => period=32768, emit granularity is 64 samples.
        // pilot_phase = period - 10 => drop 10 samples to reach PN chip 0.
        let mut p = PnAlignProcessor::new(1);
        let phase = 32768 - 10;
        let mut block = SampleBlock::new(vec![Complex32::new(1.0, 0.0); 128], 0)
            .with_sample_rate_hz(1_228_800.0);
        block.tags.insert("pilot_phase", phase as i64);

        let out = p.process_block(block);
        assert_eq!(1, out.len());
        // 128 - 10 = 118, rounded down to 64-sample granularity = 64
        assert_eq!(64, out[0].len());
        // Outgoing timeline is rebased to PN epoch.
        assert_eq!(0, out[0].chip_start);
    }

    #[test]
    fn test_pn_align_processor_passes_through_without_tag() {
        // No pilot_phase tag => no alignment, pass through at emit granularity.
        let mut p = PnAlignProcessor::new(1);
        let block = SampleBlock::new(vec![Complex32::new(1.0, 0.0); 128], 63)
            .with_sample_rate_hz(1_228_800.0);

        let out = p.process_block(block);
        assert_eq!(1, out.len());
        assert_eq!(128, out[0].len());
        assert_eq!(63, out[0].chip_start);
    }

    #[test]
    fn test_pn_align_processor_reset_rearms_alignment() {
        let mut p = PnAlignProcessor::new(1).with_reset_on_tag("upstream_lock_lost");

        // pilot_phase=0 => drop 0 samples (already aligned to PN chip 0).
        let mut first = SampleBlock::new(vec![Complex32::new(1.0, 0.0); 128], 0);
        first.tags.insert("pilot_phase", 0);
        let out1 = p.process_block(first);
        assert_eq!(1, out1.len());
        assert_eq!(128, out1[0].len());
        assert_eq!(0, out1[0].chip_start);

        // Trigger reset and feed a block with pilot_phase = period - 5 => drop 5.
        let mut second = SampleBlock::new(vec![Complex32::new(1.0, 0.0); 256], 100);
        second.tags.insert("upstream_lock_lost", 1);
        second.tags.insert("pilot_phase", (32768 - 5) as i64);
        let out2 = p.process_block(second);
        assert_eq!(1, out2.len());
        // 256 - 5 = 251, rounded to 64-granularity = 192
        assert_eq!(192, out2[0].len());
        // After reset, timeline is rebased to the new PN epoch.
        assert_eq!(0, out2[0].chip_start);
    }
}
