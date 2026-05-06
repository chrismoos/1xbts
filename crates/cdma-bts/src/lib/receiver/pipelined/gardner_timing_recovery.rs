use num_complex::Complex32;

#[derive(Debug, Clone, Copy)]
pub struct GardnerTimingConfig {
    pub enabled: bool,
    pub samples_per_symbol: f32,
    pub proportional_gain: f32,
    pub integral_gain: f32,
    pub max_error: f32,
    pub max_step_adjust_samples: f32,
    pub max_offset_samples: f32,
    pub min_mid_energy: f32,
    pub update_interval_chips: u32,
    pub max_update_chips: u64,
}

impl GardnerTimingConfig {
    pub fn disabled() -> Self {
        Self {
            enabled: false,
            ..Self::reverse_access_4x()
        }
    }

    pub fn reverse_access_4x() -> Self {
        Self {
            enabled: true,
            samples_per_symbol: 4.0,
            // Closed-loop timing is now responsible for replacing the older
            // open-loop fractional prompt search, so it must be able to pull
            // in from an integer prompt during the access preamble.
            proportional_gain: 0.0002,
            integral_gain: 0.0,
            max_error: 1.0,
            max_step_adjust_samples: 0.001,
            max_offset_samples: 4.0,
            min_mid_energy: 1e-8,
            update_interval_chips: 2,
            // Reverse access has no stable pilot after the W0 preamble. Use
            // Gardner only as an acquisition pull-in aid, then freeze timing
            // so random data decisions cannot walk a good finger off the eye.
            max_update_chips: 8 * 256,
        }
    }

    pub fn with_samples_per_symbol(mut self, samples_per_symbol: f32) -> Self {
        self.samples_per_symbol = samples_per_symbol.max(1.0);
        self
    }

    pub fn with_update_interval_chips(mut self, update_interval_chips: u32) -> Self {
        self.update_interval_chips = update_interval_chips.max(1);
        self
    }
}

#[derive(Debug, Clone, Copy, Default)]
pub struct GardnerTimingAdjustment {
    /// Fractional adjustment applied to the next nominal symbol interval.
    pub step_adjust_samples: f32,
    /// Whole-sample reference slew needed to keep the bounded fractional offset
    /// and integer despreading reference aligned.
    pub integer_slew_samples: i32,
    pub error: f32,
}

#[derive(Debug, Clone)]
pub struct GardnerTimingRecovery {
    cfg: GardnerTimingConfig,
    prev_prompt: Option<Complex32>,
    offset_samples: f32,
    integrator: f32,
    last_error: f32,
    updates: u64,
    skipped: u64,
    chips_since_update: u32,
    chips_observed: u64,
}

impl GardnerTimingRecovery {
    pub fn new(cfg: GardnerTimingConfig, initial_offset_samples: f32) -> Option<Self> {
        if !cfg.enabled {
            return None;
        }
        let max_offset = cfg.max_offset_samples.abs().max(0.001);
        let offset_samples = initial_offset_samples.clamp(-max_offset, max_offset);
        Some(Self {
            cfg,
            prev_prompt: None,
            offset_samples,
            integrator: 0.0,
            last_error: 0.0,
            updates: 0,
            skipped: 0,
            chips_since_update: 0,
            chips_observed: 0,
        })
    }

    pub fn needs_midpoint(&self) -> bool {
        self.prev_prompt.is_some()
            && self.chips_since_update.saturating_add(1) >= self.update_interval_chips()
    }

    pub fn is_tracking_active(&self) -> bool {
        self.cfg.max_update_chips == 0 || self.chips_observed < self.cfg.max_update_chips
    }

    pub fn observe(
        &mut self,
        prompt: Complex32,
        mid_between_prev_and_prompt: Option<Complex32>,
    ) -> GardnerTimingAdjustment {
        if self.cfg.max_update_chips > 0 && self.chips_observed >= self.cfg.max_update_chips {
            self.prev_prompt = Some(prompt);
            return GardnerTimingAdjustment::default();
        }
        self.chips_observed = self.chips_observed.saturating_add(1);

        let Some(prev_prompt) = self.prev_prompt.replace(prompt) else {
            return GardnerTimingAdjustment::default();
        };
        self.chips_since_update = self.chips_since_update.saturating_add(1);
        if self.chips_since_update < self.update_interval_chips() {
            return GardnerTimingAdjustment::default();
        }
        self.chips_since_update = 0;

        let Some(mid) = mid_between_prev_and_prompt else {
            self.skipped += 1;
            return GardnerTimingAdjustment::default();
        };

        let energy = prev_prompt.norm_sqr() + prompt.norm_sqr() + 2.0 * mid.norm_sqr();
        if energy < self.cfg.min_mid_energy {
            self.skipped += 1;
            return GardnerTimingAdjustment::default();
        }

        // Gardner TED for complex samples. The normalization makes the loop gain
        // usable across captures with different absolute signal amplitudes.
        let raw_error = ((prompt - prev_prompt) * mid.conj()).re;
        let error = (raw_error / energy.max(1e-12))
            .clamp(-self.cfg.max_error.abs(), self.cfg.max_error.abs());

        self.integrator += self.cfg.integral_gain * error;
        self.integrator = self.integrator.clamp(
            -self.cfg.max_step_adjust_samples,
            self.cfg.max_step_adjust_samples,
        );

        let step_adjust = (-(self.cfg.proportional_gain * error) + self.integrator).clamp(
            -self.cfg.max_step_adjust_samples,
            self.cfg.max_step_adjust_samples,
        );

        let max_offset = self.cfg.max_offset_samples.abs().max(0.001);
        self.offset_samples += step_adjust;
        let mut integer_slew_samples = 0i32;
        if self.offset_samples > max_offset {
            self.offset_samples -= 1.0;
            integer_slew_samples = 1;
        } else if self.offset_samples < -max_offset {
            self.offset_samples += 1.0;
            integer_slew_samples = -1;
        }

        self.last_error = error;
        self.updates += 1;

        GardnerTimingAdjustment {
            step_adjust_samples: step_adjust,
            integer_slew_samples,
            error,
        }
    }

    pub fn offset_samples(&self) -> f32 {
        self.offset_samples
    }

    pub fn last_error(&self) -> f32 {
        self.last_error
    }

    pub fn updates(&self) -> u64 {
        self.updates
    }

    pub fn skipped(&self) -> u64 {
        self.skipped
    }

    pub fn update_interval_chips(&self) -> u32 {
        self.cfg.update_interval_chips.max(1)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn constant_samples_do_not_move_timing() {
        let cfg = GardnerTimingConfig::reverse_access_4x().with_update_interval_chips(1);
        let mut loop_state = GardnerTimingRecovery::new(cfg, 0.0).unwrap();
        for _ in 0..32 {
            let adj = loop_state.observe(Complex32::new(1.0, 0.0), Some(Complex32::new(1.0, 0.0)));
            assert_eq!(adj.integer_slew_samples, 0);
        }
        assert!(loop_state.updates() > 0);
        assert!(loop_state.offset_samples().abs() < 1e-6);
    }

    #[test]
    fn adjustment_is_clamped() {
        let cfg = GardnerTimingConfig {
            max_step_adjust_samples: 0.01,
            ..GardnerTimingConfig::reverse_access_4x()
        }
        .with_update_interval_chips(1);
        let mut loop_state = GardnerTimingRecovery::new(cfg, 0.0).unwrap();
        loop_state.observe(Complex32::new(10.0, 0.0), Some(Complex32::new(1.0, 0.0)));
        let adj = loop_state.observe(Complex32::new(-10.0, 0.0), Some(Complex32::new(1.0, 0.0)));
        assert_ne!(adj.error, 0.0);
        assert!(adj.step_adjust_samples.abs() <= 0.01);
    }

    #[test]
    fn update_interval_decimates_ted_work() {
        let cfg = GardnerTimingConfig::reverse_access_4x().with_update_interval_chips(4);
        let mut loop_state = GardnerTimingRecovery::new(cfg, 0.0).unwrap();
        assert!(!loop_state.needs_midpoint());

        for i in 0..5 {
            let needs_midpoint = loop_state.needs_midpoint();
            let adj = loop_state.observe(
                Complex32::new(if i % 2 == 0 { 1.0 } else { -1.0 }, 0.0),
                needs_midpoint.then_some(Complex32::new(0.25, 0.0)),
            );
            if i < 4 {
                assert_eq!(adj.error, 0.0);
            } else {
                assert_ne!(adj.error, 0.0);
            }
        }
        assert_eq!(loop_state.updates(), 1);
    }

    #[test]
    fn max_update_chips_freezes_loop() {
        let cfg = GardnerTimingConfig {
            proportional_gain: 1.0,
            max_step_adjust_samples: 0.25,
            max_update_chips: 2,
            ..GardnerTimingConfig::reverse_access_4x()
        }
        .with_update_interval_chips(1);
        let mut loop_state = GardnerTimingRecovery::new(cfg, 0.0).unwrap();

        loop_state.observe(Complex32::new(1.0, 0.0), Some(Complex32::new(0.5, 0.0)));
        let active = loop_state.observe(Complex32::new(-1.0, 0.0), Some(Complex32::new(0.5, 0.0)));
        assert_ne!(active.step_adjust_samples, 0.0);
        assert_eq!(loop_state.updates(), 1);

        for _ in 0..4 {
            let frozen =
                loop_state.observe(Complex32::new(-1.0, 0.0), Some(Complex32::new(0.5, 0.0)));
            assert_eq!(frozen.step_adjust_samples, 0.0);
            assert_eq!(frozen.integer_slew_samples, 0);
        }
        assert_eq!(loop_state.updates(), 1);
        assert!(!loop_state.is_tracking_active());
    }
}
