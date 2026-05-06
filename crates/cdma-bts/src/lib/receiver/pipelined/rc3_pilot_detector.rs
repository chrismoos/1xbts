use std::collections::{HashMap, VecDeque};

use log::debug;
use num_complex::Complex32;

use super::{PipelineProcessor, SampleBlock};

/// Number of chip-rate samples per RC3 power control group (PCG).
/// 1 PCG = 1.25 ms = 1536 PN chips at 1.2288 Mcps.
const CHIPS_PER_PCG: usize = 1536;

/// Default minimum number of consecutive PCGs with energy above threshold to
/// declare pilot acquisition. RC3 traffic preamble is pilot-only transmission
/// on R-PICH = W(0,64), which after PN+LC despreading appears as sustained
/// broadband energy.
///
/// The spec allows the BS to configure the preamble length via NUM_PREAMBLE
/// in the Channel Assignment. Use `Rc3PilotDetector::with_min_pcgs()` to
/// set this based on the assigned preamble configuration.
const MIN_PCGS_FOR_LOCK: usize = 4;

/// Fraction of peak per-PCG energy that a PCG must exceed to count as "active".
const ENERGY_THRESHOLD_RATIO: f32 = 0.2;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DetectorState {
    Searching,
    Found,
}

/// RC3 reverse traffic pilot/preamble detector.
///
/// For RC3, the traffic channel preamble is the ungated Reverse Pilot Channel
/// (R-PICH, W(0,64)). After the PnLcCorrelator's PN+LC despreading, the pilot
/// shows up as sustained energy. This detector buffers chip-rate samples and
/// looks for consecutive power control groups with consistent energy, then
/// passes data through once pilot is acquired.
///
/// This is much simpler than the RC1 preamble detector (which looks for W0
/// patterns in 64-ary Walsh symbols) because we just need to confirm energy
/// presence — the correlator already locked onto the mobile's PN/LC timing.
pub struct Rc3PilotDetector {
    state: DetectorState,
    pending: VecDeque<Complex32>,
    pending_chip_start: usize,
    pending_sample_rate_hz: f64,
    pending_absolute_chip_start: Option<i64>,
    pending_absolute_sample_start: Option<i64>,
    /// Tags from the most recent input block — propagated to output blocks.
    pending_tags: HashMap<&'static str, i64>,
    buffer: VecDeque<SampleBlock>,
    pcg_energies: VecDeque<f32>,
    min_pcgs: usize,
    preamble_pcgs: usize,
    preamble_event_sent: bool,
    /// Tag name inserted on output blocks when preamble is detected.
    preamble_tag: &'static str,
}

impl Rc3PilotDetector {
    pub fn new() -> Self {
        Self::with_min_pcgs(MIN_PCGS_FOR_LOCK)
    }

    pub fn with_min_pcgs(min_pcgs: usize) -> Self {
        Self {
            state: DetectorState::Searching,
            pending: VecDeque::new(),
            pending_chip_start: 0,
            pending_sample_rate_hz: 0.0,
            pending_absolute_chip_start: None,
            pending_absolute_sample_start: None,
            pending_tags: HashMap::new(),
            buffer: VecDeque::new(),
            pcg_energies: VecDeque::new(),
            min_pcgs: min_pcgs.max(1),
            preamble_pcgs: 0,
            preamble_event_sent: false,
            preamble_tag: "access_preamble_detected",
        }
    }

    /// Set the tag name emitted on output blocks (default: `"access_preamble_detected"`).
    pub fn with_preamble_tag(mut self, tag: &'static str) -> Self {
        self.preamble_tag = tag;
        self
    }

    fn preamble_count_tag(&self) -> &'static str {
        match self.preamble_tag {
            "traffic_preamble_detected" => "traffic_preamble_frames",
            _ => "access_preamble_frames",
        }
    }

    fn pcg_energy(samples: &[Complex32]) -> f32 {
        samples
            .iter()
            .map(|s| s.re * s.re + s.im * s.im)
            .sum::<f32>()
            / samples.len().max(1) as f32
    }

    fn apply_pending_timing_tags(&self, tags: &mut HashMap<&'static str, i64>) {
        if let Some(absolute_chip_start) = self.pending_absolute_chip_start {
            tags.insert("absolute_chip_start", absolute_chip_start);
        }
        if let Some(absolute_sample_start) = self.pending_absolute_sample_start {
            tags.insert("absolute_sample_start", absolute_sample_start);
        }
    }

    fn advance_pending_timing(&mut self, chips: usize) {
        let delta = chips as i64;
        if let Some(absolute_chip_start) = &mut self.pending_absolute_chip_start {
            *absolute_chip_start = absolute_chip_start.saturating_add(delta);
        }
        if let Some(absolute_sample_start) = &mut self.pending_absolute_sample_start {
            *absolute_sample_start = absolute_sample_start.saturating_add(delta);
        }
    }

    fn try_acquire(&mut self) -> bool {
        while self.pending.len() >= CHIPS_PER_PCG {
            let pcg: Vec<Complex32> = self.pending.drain(..CHIPS_PER_PCG).collect();
            let energy = Self::pcg_energy(&pcg);
            self.pcg_energies.push_back(energy);

            let mut blk = SampleBlock::new(pcg, self.pending_chip_start);
            blk.sample_rate_hz = self.pending_sample_rate_hz;
            blk.tags = self.pending_tags.clone();
            self.apply_pending_timing_tags(&mut blk.tags);
            self.pending_chip_start += CHIPS_PER_PCG;
            self.advance_pending_timing(CHIPS_PER_PCG);
            self.buffer.push_back(blk);
        }

        if self.pcg_energies.len() < self.min_pcgs {
            return false;
        }

        let recent: Vec<f32> = self
            .pcg_energies
            .iter()
            .rev()
            .take(self.min_pcgs)
            .copied()
            .collect();
        let peak = recent.iter().copied().fold(0.0f32, f32::max);
        if peak < 1e-12 {
            return false;
        }
        let threshold = peak * ENERGY_THRESHOLD_RATIO;
        recent.iter().all(|&e| e >= threshold)
    }

    fn emit_pcg_blocks(&mut self) -> Vec<SampleBlock> {
        let mut out = Vec::new();
        while self.pending.len() >= CHIPS_PER_PCG {
            let pcg: Vec<Complex32> = self.pending.drain(..CHIPS_PER_PCG).collect();
            let mut blk = SampleBlock::new(pcg, self.pending_chip_start);
            blk.sample_rate_hz = self.pending_sample_rate_hz;
            blk.tags = self.pending_tags.clone();
            self.apply_pending_timing_tags(&mut blk.tags);
            self.pending_chip_start += CHIPS_PER_PCG;
            self.advance_pending_timing(CHIPS_PER_PCG);
            out.push(blk);
        }
        out
    }
}

impl PipelineProcessor for Rc3PilotDetector {
    fn name(&self) -> &'static str {
        "Rc3PilotDetector"
    }

    fn process_block(&mut self, block: SampleBlock) -> Vec<SampleBlock> {
        if self.pending.is_empty() {
            self.pending_chip_start = block.chip_start;
            self.pending_sample_rate_hz = block.sample_rate_hz;
            self.pending_absolute_chip_start = block.tags.get("absolute_chip_start").copied();
            self.pending_absolute_sample_start = block.tags.get("absolute_sample_start").copied();
        }
        // Always update tags from the latest input block so that
        // absolute_chip_start and other per-block tags propagate downstream.
        self.pending_tags = block.tags.clone();
        self.pending.extend(block.samples.iter());

        match self.state {
            DetectorState::Searching => {
                if self.try_acquire() {
                    self.preamble_pcgs = self.pcg_energies.len();
                    debug!(
                        "rc3_pilot_detector: pilot acquired after {} PCGs ({:.1} ms) min_pcgs={} tag={}",
                        self.preamble_pcgs,
                        self.preamble_pcgs as f64 * 1.25,
                        self.min_pcgs,
                        self.preamble_tag,
                    );
                    self.state = DetectorState::Found;
                    let mut out: Vec<SampleBlock> = Vec::new();

                    if !self.preamble_event_sent {
                        // Emit a dedicated preamble-only event block (no samples) so
                        // downstream processors can surface it immediately without
                        // waiting for frame alignment to succeed.
                        let mut preamble_event =
                            SampleBlock::new(Vec::new(), self.pending_chip_start);
                        preamble_event.sample_rate_hz = self.pending_sample_rate_hz;
                        preamble_event.tags = self.pending_tags.clone();
                        self.apply_pending_timing_tags(&mut preamble_event.tags);
                        preamble_event.tags.insert(self.preamble_tag, 1);
                        preamble_event
                            .tags
                            .insert(self.preamble_count_tag(), self.preamble_pcgs as i64);
                        debug!(
                            "rc3_pilot_detector: emitting preamble event tag={} count_tag={} pcgs={}",
                            self.preamble_tag,
                            self.preamble_count_tag(),
                            self.preamble_pcgs,
                        );
                        out.push(preamble_event);
                        self.preamble_event_sent = true;
                    }

                    for blk in self.buffer.drain(..) {
                        out.push(blk);
                    }
                    out.extend(self.emit_pcg_blocks());
                    out
                } else {
                    while self.pcg_energies.len() > self.min_pcgs * 4 {
                        self.pcg_energies.pop_front();
                        self.buffer.pop_front();
                    }
                    Vec::new()
                }
            }
            DetectorState::Found => self.emit_pcg_blocks(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emits_only_one_preamble_event_when_pilot_is_acquired() {
        let mut detector =
            Rc3PilotDetector::with_min_pcgs(2).with_preamble_tag("traffic_preamble_detected");
        let samples = vec![Complex32::new(1.0, 0.0); CHIPS_PER_PCG * 3];
        let block = SampleBlock::new(samples, 100).with_sample_rate_hz(1_228_800.0);

        let out = detector.process_block(block);
        assert_eq!(out.len(), 4);
        assert_eq!(
            out.iter()
                .filter(|blk| blk.tags.get("traffic_preamble_detected") == Some(&1))
                .count(),
            1
        );
        assert_eq!(out[0].samples.len(), 0);
        assert_eq!(out[0].tags.get("traffic_preamble_detected"), Some(&1));
        assert_eq!(out[0].tags.get("traffic_preamble_frames"), Some(&3));
        assert!(out[0].tags.get("access_preamble_frames").is_none());
        assert!(
            out[1..]
                .iter()
                .all(|blk| blk.samples.len() == CHIPS_PER_PCG)
        );
        assert!(
            out[1..]
                .iter()
                .all(|blk| blk.tags.get("traffic_preamble_detected").is_none())
        );
    }

    #[test]
    fn does_not_reemit_preamble_tag_after_lock() {
        let mut detector =
            Rc3PilotDetector::with_min_pcgs(2).with_preamble_tag("traffic_preamble_detected");
        let acquire_block = SampleBlock::new(vec![Complex32::new(1.0, 0.0); CHIPS_PER_PCG * 2], 0)
            .with_sample_rate_hz(1_228_800.0);
        let _ = detector.process_block(acquire_block);

        let follow_on =
            SampleBlock::new(vec![Complex32::new(1.0, 0.0); CHIPS_PER_PCG], CHIPS_PER_PCG)
                .with_sample_rate_hz(1_228_800.0);
        let out = detector.process_block(follow_on);

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].samples.len(), CHIPS_PER_PCG);
        assert!(out[0].tags.get("traffic_preamble_detected").is_none());
    }

    #[test]
    fn default_access_preamble_event_uses_access_count_tag() {
        let mut detector = Rc3PilotDetector::with_min_pcgs(2);
        let samples = vec![Complex32::new(1.0, 0.0); CHIPS_PER_PCG * 2];
        let block = SampleBlock::new(samples, 100).with_sample_rate_hz(1_228_800.0);

        let out = detector.process_block(block);

        assert_eq!(out[0].tags.get("access_preamble_detected"), Some(&1));
        assert_eq!(out[0].tags.get("access_preamble_frames"), Some(&2));
        assert!(out[0].tags.get("traffic_preamble_frames").is_none());
    }

    #[test]
    fn emitted_pcg_blocks_advance_absolute_chip_start() {
        let mut detector =
            Rc3PilotDetector::with_min_pcgs(2).with_preamble_tag("traffic_preamble_detected");
        let mut block = SampleBlock::new(vec![Complex32::new(1.0, 0.0); CHIPS_PER_PCG * 2], 200)
            .with_sample_rate_hz(1_228_800.0);
        block.tags.insert("absolute_chip_start", 5_000);

        let out = detector.process_block(block);
        let pcg_blocks = out
            .into_iter()
            .filter(|blk| !blk.samples.is_empty())
            .collect::<Vec<_>>();

        assert_eq!(pcg_blocks.len(), 2);
        assert_eq!(pcg_blocks[0].chip_start, 200);
        assert_eq!(pcg_blocks[0].tags.get("absolute_chip_start"), Some(&5_000));
        assert_eq!(pcg_blocks[1].chip_start, 200 + CHIPS_PER_PCG);
        assert_eq!(
            pcg_blocks[1].tags.get("absolute_chip_start"),
            Some(&(5_000 + CHIPS_PER_PCG as i64)),
        );
    }
}
