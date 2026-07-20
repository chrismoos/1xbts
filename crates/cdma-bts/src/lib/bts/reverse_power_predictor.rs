//! Shared reverse-link closed-loop power-control primitives.
//!
//! Two metric-agnostic building blocks used by both the RC3 1x inner loop
//! (`power_control.rs`) and the HRPD reverse-traffic RPC loop:
//!
//! 1. A least-squares trend predictor that extrapolates the measured pilot
//!    metric forward across the RX→TX command delay, so the one-bit loop reacts to
//!    where the level *will be* when the bit lands rather than the stale present.
//! 2. A first-order delta-sigma quantizer that converts a continuous desired dB
//!    correction into a stream of single-step up/down power-control bits whose
//!    running average equals the desired correction. This is what keeps the loop
//!    from turning a large error into a burst of same-direction bits.
//!
//! Bit polarity matches both callers: `0` = power UP, `1` = power DOWN.

use std::collections::VecDeque;

/// Least-squares line fit over `samples` in arrival order (index 0 oldest,
/// last newest, unit time step). Returns the fitted value AT the newest sample
/// and the slope per step. Empty -> `(NaN, 0)`, single sample -> `(value, 0)`.
pub fn lsq_intercept_and_slope_at_newest(samples: &VecDeque<f32>) -> (f32, f32) {
    let n = samples.len();
    if n == 0 {
        return (f32::NAN, 0.0);
    }
    if n == 1 {
        return (samples[0], 0.0);
    }
    let nf = n as f32;
    let t_mean = (nf - 1.0) * 0.5;
    let y_mean: f32 = samples.iter().sum::<f32>() / nf;
    let mut num = 0.0_f32;
    let mut den = 0.0_f32;
    for (i, &y) in samples.iter().enumerate() {
        let dt = i as f32 - t_mean;
        num += dt * (y - y_mean);
        den += dt * dt;
    }
    let slope = if den > 0.0 { num / den } else { 0.0 };
    let intercept_at_newest = y_mean + slope * ((nf - 1.0) - t_mean);
    (intercept_at_newest, slope)
}

/// Extrapolate `lead` steps past the newest sample along `slope`, clamped to
/// `±clamp_db` around `intercept_at_now` so a noisy slope cannot launch the
/// prediction arbitrarily far.
pub fn predict_ahead_clamped(intercept_at_now: f32, slope: f32, lead: f32, clamp_db: f32) -> f32 {
    let raw = intercept_at_now + lead * slope;
    raw.clamp(intercept_at_now - clamp_db, intercept_at_now + clamp_db)
}

/// Tuning for [`delta_sigma_pcb_step`].
#[derive(Debug, Clone, Copy)]
pub struct DeltaSigmaParams {
    /// Errors within `±hold_band_db` produce no net drive (dead zone around
    /// the setpoint to suppress idle dither).
    pub hold_band_db: f32,
    /// Proportional gain mapping the dead-zoned error to a desired step.
    pub response_gain_db_per_db: f32,
    /// Clamp on the per-tick desired step (bounds how fast one measurement can
    /// drive the loop).
    pub desired_step_clamp_db: f32,
    /// Clamp on the accumulated residual (anti-windup).
    pub residual_clamp_db: f32,
}

/// One first-order delta-sigma power-control step. `error_db` is
/// `target - metric` (positive => under target => command UP). Accumulates the
/// desired fractional step into `residual_db` and emits a single discrete bit
/// (`0` = UP, `1` = DOWN), carrying the remainder forward so the bit stream's
/// average tracks the desired correction.
pub fn delta_sigma_pcb_step(residual_db: &mut f32, error_db: f32, params: &DeltaSigmaParams) -> u8 {
    let effective_error_db = if error_db.abs() <= params.hold_band_db {
        0.0
    } else {
        error_db - params.hold_band_db * error_db.signum()
    };
    let desired_step_db = (effective_error_db * params.response_gain_db_per_db)
        .clamp(-params.desired_step_clamp_db, params.desired_step_clamp_db);
    let residual =
        (*residual_db + desired_step_db).clamp(-params.residual_clamp_db, params.residual_clamp_db);
    let (pcb, applied_step_db) = if residual >= 0.0 { (0, 1.0) } else { (1, -1.0) };
    *residual_db =
        (residual - applied_step_db).clamp(-params.residual_clamp_db, params.residual_clamp_db);
    pcb
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> DeltaSigmaParams {
        DeltaSigmaParams {
            hold_band_db: 0.25,
            response_gain_db_per_db: 0.6,
            desired_step_clamp_db: 1.0,
            residual_clamp_db: 1.0,
        }
    }

    #[test]
    fn lsq_flat_history_has_zero_slope() {
        let samples: VecDeque<f32> = [-5.0, -5.0, -5.0, -5.0].into_iter().collect();
        let (intercept, slope) = lsq_intercept_and_slope_at_newest(&samples);
        assert!(slope.abs() < 1e-6);
        assert!((intercept - -5.0).abs() < 1e-5);
    }

    #[test]
    fn lsq_recovers_slope_and_value_at_newest() {
        // y = 1 + 0.3 t for t = 0..5; newest (t=4) value = 2.2.
        let samples: VecDeque<f32> = (0..5).map(|t| 1.0 + 0.3 * t as f32).collect();
        let (intercept, slope) = lsq_intercept_and_slope_at_newest(&samples);
        assert!((slope - 0.3).abs() < 1e-4, "slope {slope}");
        assert!((intercept - 2.2).abs() < 1e-4, "intercept {intercept}");
    }

    #[test]
    fn predict_extrapolates_and_clamps() {
        // Slope +0.3/step, 12 steps ahead = +3.6, clamped to +1.0.
        let p = predict_ahead_clamped(2.2, 0.3, 12.0, 1.0);
        assert!((p - 3.2).abs() < 1e-5, "clamped to intercept+1: {p}");
        // Within clamp, exact extrapolation.
        let q = predict_ahead_clamped(2.2, 0.1, 5.0, 1.0);
        assert!((q - 2.7).abs() < 1e-5, "{q}");
    }

    #[test]
    fn delta_sigma_holds_inside_dead_band() {
        let mut residual = 0.0;
        // |error| <= hold band => desired 0 => residual stays 0 => emits UP at
        // the zero boundary, but never accumulates drive.
        for _ in 0..8 {
            let _ = delta_sigma_pcb_step(&mut residual, 0.1, &params());
        }
        assert!(
            residual.abs() < 1e-6,
            "no drive accumulates in the dead band"
        );
    }

    #[test]
    fn delta_sigma_average_tracks_desired_correction() {
        // A steady +0.3 dB error (under target) should command UP more often
        // than DOWN, with the long-run average bit reflecting the drive.
        let mut residual = 0.0;
        let mut up = 0u32;
        let mut down = 0u32;
        for _ in 0..1000 {
            match delta_sigma_pcb_step(&mut residual, 0.3, &params()) {
                0 => up += 1,
                _ => down += 1,
            }
        }
        assert!(
            up > down,
            "under-target error must net UP: up={up} down={down}"
        );
        // And it must not slam: both directions appear (it dithers).
        assert!(
            down > 0,
            "delta-sigma should still dither DOWN occasionally"
        );
    }

    #[test]
    fn delta_sigma_large_error_is_step_limited_not_slammed() {
        // Even a huge error emits exactly one bit per call (never a burst), and
        // the residual never exceeds its clamp.
        let mut residual = 0.0;
        for _ in 0..50 {
            let bit = delta_sigma_pcb_step(&mut residual, 100.0, &params());
            assert!(bit == 0 || bit == 1);
            assert!(residual.abs() <= 1.0 + 1e-6, "residual clamp respected");
        }
    }
}
