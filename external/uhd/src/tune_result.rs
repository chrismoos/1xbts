/// The result of a tuning operation
#[derive(Clone)]
pub struct TuneResult(uhd_sys::uhd_tune_result_t);

impl TuneResult {
    /// Returns the target RF frequency
    pub fn target_rf_freq(&self) -> f64 {
        self.0.target_rf_freq
    }

    /// Returns the target RF frequency constrained to the device's supported frequency range
    pub fn clipped_rf_freq(&self) -> f64 {
        self.0.clipped_rf_freq
    }

    /// Returns the actual RF frequency
    pub fn actual_rf_freq(&self) -> f64 {
        self.0.actual_rf_freq
    }

    /// Returns the target DSP frequency adjustment
    pub fn target_dsp_freq(&self) -> f64 {
        self.0.target_dsp_freq
    }

    /// Returns the actual DSP frequency adjustment
    pub fn actual_dsp_freq(&self) -> f64 {
        self.0.actual_dsp_freq
    }

    pub(crate) fn inner_mut(&mut self) -> &mut uhd_sys::uhd_tune_result_t {
        &mut self.0
    }
}

impl Default for TuneResult {
    fn default() -> Self {
        TuneResult(uhd_sys::uhd_tune_result_t {
            clipped_rf_freq: 0.0,
            target_rf_freq: 0.0,
            actual_rf_freq: 0.0,
            target_dsp_freq: 0.0,
            actual_dsp_freq: 0.0,
        })
    }
}

unsafe impl Send for TuneResult {}
unsafe impl Sync for TuneResult {}

mod fmt {
    use super::TuneResult;
    use std::fmt::{Debug, Formatter, Result};

    impl Debug for TuneResult {
        fn fmt(&self, f: &mut Formatter<'_>) -> Result {
            f.debug_struct("TuneResult")
                .field("target_rf_freq", &self.0.target_rf_freq)
                .field("clipped_rf_freq", &self.0.clipped_rf_freq)
                .field("actual_rf_freq", &self.0.actual_rf_freq)
                .field("target_dsp_freq", &self.0.target_dsp_freq)
                .field("actual_dsp_freq", &self.0.actual_dsp_freq)
                .finish()
        }
    }
}
