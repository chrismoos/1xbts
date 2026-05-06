/// A request to tune a frontend
#[derive(Debug, Clone)]
pub struct TuneRequest {
    pub(crate) target_frequency: f64,
    pub(crate) rf: TuneRequestPolicy,
    pub(crate) dsp: TuneRequestPolicy,
    /// Extra arguments
    pub(crate) args: String,
}

impl TuneRequest {
    /// Creates a tune request that automatically configures the hardware to tune to the desired
    /// frequency
    pub fn with_frequency(frequency: f64) -> Self {
        TuneRequest {
            target_frequency: frequency,
            rf: TuneRequestPolicy::Auto,
            dsp: TuneRequestPolicy::Auto,
            args: String::new(),
        }
    }
    /// Creates a tune request that automatically configures the hardware to tune to the desired
    /// frequency, with an offset between the RF center frequency and the
    pub fn with_frequency_lo(frequency: f64, local_offset: f64) -> Self {
        TuneRequest {
            target_frequency: frequency,
            rf: TuneRequestPolicy::Manual(frequency + local_offset),
            dsp: TuneRequestPolicy::Auto,
            args: String::new(),
        }
    }

    /// Sets the policy for tuning the RF frontend
    pub fn set_rf_policy(&mut self, policy: TuneRequestPolicy) {
        self.rf = policy
    }
    /// Sets the policy for tuning the DSP
    pub fn set_dsp_policy(&mut self, policy: TuneRequestPolicy) {
        self.dsp = policy
    }
    /// Sets additional device-specific arguments
    pub fn set_args(&mut self, args: String) {
        self.args = args
    }
}

/// Policies for how tuning should be accomplished
#[derive(Debug, Clone)]
pub enum TuneRequestPolicy {
    /// Keep the current value
    None,
    /// Automatically determine a new value
    Auto,
    /// Manually set a specific value
    ///
    /// The enclosed value is the desired frequency in hertz.
    Manual(f64),
}

impl TuneRequestPolicy {
    pub(crate) fn c_policy(&self) -> uhd_sys::uhd_tune_request_policy_t::Type {
        use uhd_sys::uhd_tune_request_policy_t::*;
        match self {
            TuneRequestPolicy::None => UHD_TUNE_REQUEST_POLICY_NONE,
            TuneRequestPolicy::Auto => UHD_TUNE_REQUEST_POLICY_AUTO,
            TuneRequestPolicy::Manual(_) => UHD_TUNE_REQUEST_POLICY_MANUAL,
        }
    }
    pub(crate) fn frequency(&self) -> f64 {
        match self {
            TuneRequestPolicy::None | TuneRequestPolicy::Auto => 0.0,
            TuneRequestPolicy::Manual(frequency) => *frequency,
        }
    }
}
