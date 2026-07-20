//! HRPD Rev 0 reverse-link pilot tracker. C.S0024-200 §9.2.1.3.1.
//!
//! Unmodulated pilot on Walsh 0; after W_0^16 despread the pilot symbol rate
//! is 1.2288 Mcps / 16 = 76.8 kHz. A boxcar of length `window` averages the
//! pilot symbols into a complex channel estimate `h_hat`; `apply` derotates
//! a data symbol by the conjugate of the normalized estimate.

use std::collections::VecDeque;

use num::complex::Complex32;

pub const HRPD_PILOT_SYMBOL_RATE_HZ: f32 = 1_228_800.0 / 16.0;
pub const DEFAULT_PILOT_WINDOW: usize = 8;
const REFRESH_INTERVAL: usize = 4096;

#[derive(Debug, Clone)]
pub struct HrpdReversePilotTracker {
    window: usize,
    fs: f32,
    buf: VecDeque<Complex32>,
    sum: Complex32,
    pushes_since_refresh: usize,
    current: Option<Complex32>,
    previous: Option<Complex32>,
    last_freq_hz: f32,
}

impl Default for HrpdReversePilotTracker {
    fn default() -> Self {
        Self::new()
    }
}

impl HrpdReversePilotTracker {
    pub fn new() -> Self {
        Self::with_window_and_rate(DEFAULT_PILOT_WINDOW, HRPD_PILOT_SYMBOL_RATE_HZ)
    }

    pub fn with_window(window: usize) -> Self {
        Self::with_window_and_rate(window, HRPD_PILOT_SYMBOL_RATE_HZ)
    }

    pub fn with_window_and_rate(window: usize, fs: f32) -> Self {
        assert!(window > 0, "window must be > 0");
        Self {
            window,
            fs,
            buf: VecDeque::with_capacity(window),
            sum: Complex32::new(0.0, 0.0),
            pushes_since_refresh: 0,
            current: None,
            previous: None,
            last_freq_hz: 0.0,
        }
    }

    /// Push one pilot symbol; returns the new boxcar average if the window is
    /// full.
    pub fn push(&mut self, sym: Complex32) -> Option<Complex32> {
        if self.buf.len() == self.window {
            let dropped = self.buf.pop_front().unwrap();
            self.sum -= dropped;
        }
        self.buf.push_back(sym);
        self.sum += sym;
        self.pushes_since_refresh += 1;
        if self.pushes_since_refresh >= REFRESH_INTERVAL {
            self.sum = self.buf.iter().copied().sum();
            self.pushes_since_refresh = 0;
        }
        if self.buf.len() < self.window {
            return None;
        }
        let avg = self.sum / (self.window as f32);
        self.previous = self.current;
        self.current = Some(avg);
        if let (Some(prev), Some(cur)) = (self.previous, self.current) {
            let cross = cur * prev.conj();
            self.last_freq_hz = cross.im.atan2(cross.re) * self.fs / (2.0 * std::f32::consts::PI);
        }
        Some(avg)
    }

    /// Derotate a data symbol by the conjugate of the normalized pilot
    /// channel estimate. Passthrough until lock.
    pub fn apply(&self, sym: Complex32) -> Complex32 {
        match self.current {
            Some(h) => {
                let mag = (h.re * h.re + h.im * h.im).sqrt();
                if mag <= 0.0 {
                    sym
                } else {
                    sym * h.conj() / mag
                }
            }
            None => sym,
        }
    }

    pub fn channel_estimate(&self) -> Option<Complex32> {
        self.current
    }

    pub fn power(&self) -> f32 {
        self.current.map_or(0.0, |h| h.re * h.re + h.im * h.im)
    }

    pub fn phase_rad(&self) -> f32 {
        self.current.map_or(0.0, |h| h.im.atan2(h.re))
    }

    pub fn frequency_hz(&self) -> f32 {
        self.last_freq_hz
    }

    pub fn is_locked(&self) -> bool {
        self.current.is_some() && self.previous.is_some()
    }

    pub fn reset(&mut self) {
        self.buf.clear();
        self.sum = Complex32::new(0.0, 0.0);
        self.pushes_since_refresh = 0;
        self.current = None;
        self.previous = None;
        self.last_freq_hz = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::PI;

    #[test]
    fn zero_noise_lock_constant_channel() {
        let mut t = HrpdReversePilotTracker::new();
        for _ in 0..DEFAULT_PILOT_WINDOW + 1 {
            t.push(Complex32::new(1.0, 0.0));
        }
        let h = t.channel_estimate().unwrap();
        assert!((h.re - 1.0).abs() < 1e-5);
        assert!(h.im.abs() < 1e-5);
        assert!(t.is_locked());
    }

    #[test]
    fn apply_removes_channel_rotation() {
        let mut t = HrpdReversePilotTracker::new();
        let phi = 0.7_f32;
        let h = Complex32::new(phi.cos(), phi.sin());
        for _ in 0..DEFAULT_PILOT_WINDOW + 1 {
            t.push(h);
        }
        let data = Complex32::new(0.6, 0.8) * h;
        let derot = t.apply(data);
        assert!((derot.re - 0.6).abs() < 1e-4);
        assert!((derot.im - 0.8).abs() < 1e-4);
    }

    #[test]
    fn frequency_estimate_plus_500hz() {
        let fs = HRPD_PILOT_SYMBOL_RATE_HZ;
        let f = 500.0_f32;
        let mut t = HrpdReversePilotTracker::new();
        for k in 0..(DEFAULT_PILOT_WINDOW * 4) {
            let phi = 2.0 * PI * f * (k as f32) / fs;
            t.push(Complex32::new(phi.cos(), phi.sin()));
        }
        assert!(
            (t.frequency_hz() - f).abs() < 5.0,
            "got {}",
            t.frequency_hz()
        );
    }

    #[test]
    fn frequency_estimate_minus_1200hz() {
        let fs = HRPD_PILOT_SYMBOL_RATE_HZ;
        let f = -1200.0_f32;
        let mut t = HrpdReversePilotTracker::new();
        for k in 0..(DEFAULT_PILOT_WINDOW * 4) {
            let phi = 2.0 * PI * f * (k as f32) / fs;
            t.push(Complex32::new(phi.cos(), phi.sin()));
        }
        assert!(
            (t.frequency_hz() - f).abs() < 10.0,
            "got {}",
            t.frequency_hz()
        );
    }

    #[test]
    fn awgn_phase_and_power_bounds() {
        let mut s = 0xBEEF_u32;
        let mut rng = || {
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            (s >> 8) as f32 / ((1u32 << 24) as f32)
        };
        let sigma = (0.01_f32 / 2.0).sqrt();
        let mut t = HrpdReversePilotTracker::with_window(64);
        for _ in 0..200 {
            let mut u1 = rng();
            if u1 < 1e-7 {
                u1 = 1e-7;
            }
            let u2 = rng();
            let r = (-2.0_f32 * u1.ln()).sqrt();
            let th = 2.0 * PI * u2;
            let n = Complex32::new(sigma * r * th.cos(), sigma * r * th.sin());
            t.push(Complex32::new(1.0, 0.0) + n);
        }
        assert!(t.phase_rad().abs() < 0.05, "phase {}", t.phase_rad());
        let p = t.power();
        assert!(p > 0.9 && p < 1.1, "power {}", p);
    }

    #[test]
    fn reset_clears_lock() {
        let mut t = HrpdReversePilotTracker::new();
        for _ in 0..DEFAULT_PILOT_WINDOW + 1 {
            t.push(Complex32::new(1.0, 0.0));
        }
        assert!(t.is_locked());
        t.reset();
        assert!(!t.is_locked());
        assert_eq!(t.frequency_hz(), 0.0);
    }

    #[test]
    fn pre_lock_apply_is_passthrough() {
        let t = HrpdReversePilotTracker::new();
        let s = Complex32::new(0.3, -0.7);
        assert_eq!(t.apply(s), s);
    }

    #[test]
    fn antipodal_jitter_boxcar_averages_to_zero() {
        let mut t = HrpdReversePilotTracker::with_window(4);
        t.push(Complex32::new(1.0, 0.0));
        t.push(Complex32::new(-1.0, 0.0));
        t.push(Complex32::new(1.0, 0.0));
        let h = t.push(Complex32::new(-1.0, 0.0)).unwrap();
        assert!(h.re.abs() < 1e-6);
    }
}
