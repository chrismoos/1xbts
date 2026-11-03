//! Reverse 1x noise floor from per-PCG band power.
//!
//! A decode reports a burst only after its last frame, so each PCG waits
//! [`COMMIT_DELAY_PCGS`] before it is judged quiet. PCGs inside a decoded
//! access burst or while a traffic channel is active are dropped. Bursts the
//! receiver never decodes stay in, and the low percentile rejects them.

use std::collections::VecDeque;

use num_complex::Complex32;

const CHIPS_PER_PCG: u64 = 1536;
const PCGS_PER_SECOND: u64 = 800;
/// Longer than the longest access burst plus the access pipeline's buffering,
/// so a decode always marks its PCGs before they are committed.
const COMMIT_DELAY_PCGS: u64 = 2 * PCGS_PER_SECOND;
const WINDOW_PCGS: u64 = 30 * PCGS_PER_SECOND;
const MIN_QUIET_PCGS: usize = 2 * PCGS_PER_SECOND as usize;
const STALE_PCGS: u64 = 30 * PCGS_PER_SECOND;
const RECOMPUTE_PCGS: u64 = PCGS_PER_SECOND;
const FLOOR_PERCENTILE: f64 = 0.10;

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct NoiseFloorReading {
    /// Mean power per sample of the matched-filtered stream, not yet
    /// referred to the ADC input.
    pub mean_power: f64,
    pub stale: bool,
}

#[derive(Clone, Copy, Debug)]
struct PendingPcg {
    pcg: u64,
    mean_power: f64,
    busy: bool,
}

#[derive(Debug, Default)]
pub(crate) struct NoiseFloorEstimator {
    open_pcg: Option<u64>,
    open_power_sum: f64,
    open_samples: u64,
    open_busy: bool,
    pending: VecDeque<PendingPcg>,
    quiet: VecDeque<(u64, f64)>,
    last_quiet_pcg: Option<u64>,
    floor: Option<f64>,
    computed_at_pcg: Option<u64>,
}

impl NoiseFloorEstimator {
    pub fn ingest(
        &mut self,
        samples: &[Complex32],
        absolute_sample_start: u64,
        oversample: usize,
        busy: bool,
    ) {
        let samples_per_pcg = CHIPS_PER_PCG * oversample.max(1) as u64;
        let mut sample = absolute_sample_start;
        let mut rest = samples;
        while !rest.is_empty() {
            let pcg = sample / samples_per_pcg;
            let in_pcg = (samples_per_pcg - sample % samples_per_pcg).min(rest.len() as u64);
            let (head, tail) = rest.split_at(in_pcg as usize);
            if self.open_pcg != Some(pcg) {
                self.close_open_pcg();
                self.open_pcg = Some(pcg);
            }
            self.open_power_sum += head.iter().map(|s| s.norm_sqr() as f64).sum::<f64>();
            self.open_samples += in_pcg;
            self.open_busy |= busy;
            sample += in_pcg;
            rest = tail;
        }
        self.commit();
    }

    /// Chips already committed are not revisited.
    pub fn mark_busy(&mut self, start_chip: u64, end_chip: u64) {
        let first = start_chip / CHIPS_PER_PCG;
        let last = end_chip.div_ceil(CHIPS_PER_PCG);
        for entry in self.pending.iter_mut() {
            if (first..last).contains(&entry.pcg) {
                entry.busy = true;
            }
        }
        if self
            .open_pcg
            .is_some_and(|pcg| (first..last).contains(&pcg))
        {
            self.open_busy = true;
        }
    }

    pub fn reading(&self) -> Option<NoiseFloorReading> {
        let floor = self.floor?;
        let latest = self.open_pcg?;
        let stale = self
            .last_quiet_pcg
            .is_none_or(|quiet| latest.saturating_sub(quiet) > STALE_PCGS + COMMIT_DELAY_PCGS);
        Some(NoiseFloorReading {
            mean_power: floor,
            stale,
        })
    }

    pub fn quiet_pcgs(&self) -> usize {
        self.quiet.len()
    }

    fn close_open_pcg(&mut self) {
        if let Some(pcg) = self.open_pcg.take()
            && self.open_samples > 0
        {
            self.pending.push_back(PendingPcg {
                pcg,
                mean_power: self.open_power_sum / self.open_samples as f64,
                busy: self.open_busy,
            });
        }
        self.open_power_sum = 0.0;
        self.open_samples = 0;
        self.open_busy = false;
    }

    fn commit(&mut self) {
        let Some(latest) = self.open_pcg else {
            return;
        };
        while let Some(entry) = self.pending.front().copied() {
            if latest.saturating_sub(entry.pcg) < COMMIT_DELAY_PCGS {
                break;
            }
            self.pending.pop_front();
            if !entry.busy {
                self.quiet.push_back((entry.pcg, entry.mean_power));
                self.last_quiet_pcg = Some(entry.pcg);
            }
        }
        while self
            .quiet
            .front()
            .is_some_and(|&(pcg, _)| latest.saturating_sub(pcg) > WINDOW_PCGS)
        {
            self.quiet.pop_front();
        }
        if self.quiet.len() >= MIN_QUIET_PCGS
            && self
                .computed_at_pcg
                .is_none_or(|at| latest.saturating_sub(at) >= RECOMPUTE_PCGS)
        {
            let mut powers: Vec<f64> = self.quiet.iter().map(|&(_, p)| p).collect();
            let index = ((powers.len() as f64 * FLOOR_PERCENTILE) as usize).min(powers.len() - 1);
            let (_, floor, _) = powers.select_nth_unstable_by(index, f64::total_cmp);
            self.floor = Some(*floor);
            self.computed_at_pcg = Some(latest);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const OVERSAMPLE: usize = 4;
    const SAMPLES_PER_PCG: u64 = CHIPS_PER_PCG * OVERSAMPLE as u64;

    struct Feed {
        est: NoiseFloorEstimator,
        next_pcg: u64,
    }

    impl Feed {
        fn new() -> Self {
            Self {
                est: NoiseFloorEstimator::default(),
                next_pcg: 0,
            }
        }

        fn chip(&self) -> u64 {
            self.next_pcg * CHIPS_PER_PCG
        }

        fn pcgs(&mut self, power: f32, count: u64, busy: bool) {
            let samples = vec![Complex32::new(power.sqrt(), 0.0); SAMPLES_PER_PCG as usize];
            for _ in 0..count {
                self.est
                    .ingest(&samples, self.next_pcg * SAMPLES_PER_PCG, OVERSAMPLE, busy);
                self.next_pcg += 1;
            }
        }
    }

    #[test]
    fn floor_ignores_marked_bursts_and_traffic() {
        let mut feed = Feed::new();
        for _ in 0..12 {
            feed.pcgs(1.0, 200, false);
            let burst_start = feed.chip();
            feed.pcgs(1000.0, 400, false);
            feed.est.mark_busy(burst_start, feed.chip());
            feed.pcgs(500.0, 200, true);
        }
        let reading = feed.est.reading().expect("floor after enough quiet PCGs");
        assert!((reading.mean_power - 1.0).abs() < 1e-6);
        assert!(!reading.stale);
    }

    #[test]
    fn low_percentile_rejects_unmarked_bursts() {
        let mut feed = Feed::new();
        for _ in 0..40 {
            feed.pcgs(2.0, 160, false);
            feed.pcgs(800.0, 10, false);
        }
        let reading = feed.est.reading().expect("floor");
        assert!((reading.mean_power - 2.0).abs() < 1e-6);
    }

    #[test]
    fn floor_holds_and_goes_stale_while_traffic_is_active() {
        let mut feed = Feed::new();
        feed.pcgs(1.0, 4000, false);
        let before = feed.est.reading().expect("floor").mean_power;
        feed.pcgs(300.0, 40_000, true);
        let reading = feed.est.reading().expect("floor is kept");
        assert_eq!(reading.mean_power, before);
        assert!(reading.stale);
    }

    #[test]
    fn no_floor_until_enough_quiet_pcgs_are_committed() {
        let mut feed = Feed::new();
        feed.pcgs(1.0, 3000, false);
        assert!(feed.est.reading().is_none());
    }
}
