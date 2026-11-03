//! Per-mobile signal quality from reverse access probes.

use std::collections::VecDeque;

use cdma_common::events::AccessSignal;

/// Eb/Nt the 4800 bps access channel needs for 1% frame errors without fading.
const REQUIRED_EB_NT_DB: f32 = 5.0;
/// A probe at mean Eb/Nt x on a single-path Rayleigh channel fails with
/// probability 1 - exp(-R/x). These margins over R set that to 50%, 10% and 1%.
const AVERAGE_MARGIN_DB: f32 = 1.6;
const GOOD_MARGIN_DB: f32 = 9.8;
const EXCELLENT_MARGIN_DB: f32 = 20.0;
const RECENT_READINGS: usize = 12;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SignalQuality {
    Poor,
    Average,
    Good,
    Excellent,
}

impl SignalQuality {
    pub fn from_eb_nt_db(eb_nt_db: f32) -> Self {
        let margin_db = eb_nt_db - REQUIRED_EB_NT_DB;
        if margin_db >= EXCELLENT_MARGIN_DB {
            Self::Excellent
        } else if margin_db >= GOOD_MARGIN_DB {
            Self::Good
        } else if margin_db >= AVERAGE_MARGIN_DB {
            Self::Average
        } else {
            Self::Poor
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AccessSignalReading {
    pub eb_nt_db: f32,
    pub quality: SignalQuality,
    pub signal_power_dbfs: Option<f32>,
    pub band_level_dbfs: Option<f32>,
    pub raw_ber_pct: Option<f32>,
    pub noise_floor_dbfs: Option<f32>,
    pub noise_floor_stale: bool,
    pub message: String,
    pub measured_at_ms: u64,
}

#[derive(Clone, Debug, Default)]
pub(crate) struct AccessSignalHistory {
    latest: Option<AccessSignalReading>,
    recent_eb_nt_db: VecDeque<f32>,
}

impl AccessSignalHistory {
    /// Records a probe measurement. Returns `false` and leaves the history
    /// unchanged when the probe has no preamble Eb/Nt or is not newer than the
    /// latest reading.
    pub fn record(
        &mut self,
        signal: &AccessSignal,
        band_level_dbfs: Option<f32>,
        message: &str,
        measured_at_ms: u64,
    ) -> bool {
        let Some(eb_nt_db) = signal.eb_nt_db else {
            return false;
        };
        if self
            .latest
            .as_ref()
            .is_some_and(|latest| latest.measured_at_ms >= measured_at_ms)
        {
            return false;
        }
        if self.recent_eb_nt_db.len() == RECENT_READINGS {
            self.recent_eb_nt_db.pop_front();
        }
        self.recent_eb_nt_db.push_back(eb_nt_db);
        self.latest = Some(AccessSignalReading {
            eb_nt_db,
            quality: SignalQuality::from_eb_nt_db(eb_nt_db),
            signal_power_dbfs: signal.signal_power_dbfs,
            band_level_dbfs,
            raw_ber_pct: signal.raw_ber_pct,
            noise_floor_dbfs: signal.noise_floor_dbfs,
            noise_floor_stale: signal.noise_floor_stale,
            message: message.to_string(),
            measured_at_ms,
        });
        true
    }

    pub fn latest(&self) -> Option<&AccessSignalReading> {
        self.latest.as_ref()
    }

    pub fn recent_eb_nt_db(&self) -> Vec<f32> {
        self.recent_eb_nt_db.iter().copied().collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tiers_follow_the_margin_over_the_required_eb_nt() {
        assert_eq!(SignalQuality::from_eb_nt_db(4.0), SignalQuality::Poor);
        assert_eq!(SignalQuality::from_eb_nt_db(6.5), SignalQuality::Poor);
        assert_eq!(SignalQuality::from_eb_nt_db(6.7), SignalQuality::Average);
        assert_eq!(SignalQuality::from_eb_nt_db(14.7), SignalQuality::Average);
        assert_eq!(SignalQuality::from_eb_nt_db(14.9), SignalQuality::Good);
        assert_eq!(SignalQuality::from_eb_nt_db(24.9), SignalQuality::Good);
        assert_eq!(SignalQuality::from_eb_nt_db(25.1), SignalQuality::Excellent);
    }

    #[test]
    fn history_keeps_only_newer_probes_with_a_preamble_reading() {
        let signal = AccessSignal {
            eb_nt_db: Some(12.0),
            ..AccessSignal::default()
        };
        let mut history = AccessSignalHistory::default();
        assert!(!history.record(&AccessSignal::default(), None, "Registration", 1));
        assert!(history.record(&signal, Some(-40.0), "Registration", 10));
        assert!(!history.record(&signal, Some(-40.0), "Registration", 10));
        assert!(history.record(&signal, Some(-40.0), "Origination", 20));
        assert_eq!(history.recent_eb_nt_db(), vec![12.0, 12.0]);
        assert_eq!(
            history.latest().map(|r| r.message.as_str()),
            Some("Origination")
        );
    }
}
