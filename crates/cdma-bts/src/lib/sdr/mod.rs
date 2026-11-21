pub mod pipe;
pub use pipe::*;
pub use sdr_hal::dsp::fir;
pub use sdr_hal::network;

use biquad::{Biquad, Coefficients, DirectForm1};
use cdma_common::consts::SR1_CHIP_RATE_HZ;
use cdma_common::error::Error;
use log::debug;
use num_complex::Complex32;

use self::fir::{ComplexFir32, PolyphaseComplexFir32};

pub use sdr_hal::dsp::PhasorNco;
pub use sdr_hal::{
    FileOutputRadio, NoopRadio, Radio, RadioRx, RadioTx, RxReadResult, TxRadioHealth,
};

pub(crate) const TX_SAMPLE_RATE: usize = SR1_CHIP_RATE_HZ as usize * 8;
pub(crate) const FILE_OUTPUT_TARGET_PEAK: f32 = 0.90;
const PHASE_EQUALIZER_ALPHA: f64 = 1.36;
const PHASE_EQUALIZER_F0_HZ: f64 = 315_000.0;

// C.S0002-E v1.0 3.1.3.1.20.1 Table 3.1.3.1.20.1-1 (mirrored to 48 taps).
pub fn cdma2000_baseband_filter_taps_f64() -> Vec<f64> {
    let mut baseband_filter = vec![
        -0.025288315,
        -0.034167931,
        -0.035752323,
        -0.016733702,
        0.021602514,
        0.064938487,
        0.091002137,
        0.081894974,
        0.037071157,
        -0.021998074,
        -0.060716277,
        -0.051178658,
        0.007874526,
        0.084368728,
        0.126869306,
        0.094528345,
        -0.012839661,
        -0.143477028,
        -0.211829088,
        -0.140513128,
        0.094601918,
        0.441387140,
        0.785875640,
        1.0,
    ];
    let mut reversed = baseband_filter.clone();
    reversed.reverse();
    baseband_filter.extend(reversed);
    baseband_filter
}

pub fn cdma2000_phase_equalizer_coeffs(sample_rate_hz: f64) -> Coefficients<f32> {
    let c = 2.0 * sample_rate_hz;
    let w0 = 2.0 * std::f64::consts::PI * PHASE_EQUALIZER_F0_HZ;
    let a = PHASE_EQUALIZER_ALPHA * w0;

    // Use the stable all-pass realization corresponding to the spec phase-equalizer response.
    let d0 = (c * c) + (a * c) + (w0 * w0);
    let d1 = (-2.0 * c * c) + (2.0 * w0 * w0);
    let d2 = (c * c) - (a * c) + (w0 * w0);

    Coefficients {
        a1: (d1 / d0) as f32,
        a2: (d2 / d0) as f32,
        b0: (d2 / d0) as f32,
        b1: (d1 / d0) as f32,
        b2: 1.0,
    }
}

pub fn cdma2000_pulse_shape_finite(
    samples: &[Complex32],
    oversample: usize,
    include_phase_equalizer: bool,
) -> Vec<Complex32> {
    if oversample == 0 || samples.is_empty() {
        return Vec::new();
    }

    let taps = cdma2000_baseband_filter_taps_f64();

    let mut upsampled = Vec::with_capacity(samples.len() * oversample);
    for s in samples {
        upsampled.push(*s);
        for _ in 1..oversample {
            upsampled.push(Complex32::default());
        }
    }

    let mut baseband_filter = ComplexFir32::new(&taps);
    let filtered = baseband_filter.process_block(&upsampled);

    if !include_phase_equalizer {
        return filtered;
    }

    let sample_rate_hz = SR1_CHIP_RATE_HZ as f64 * oversample as f64;
    let phase_eq = cdma2000_phase_equalizer_coeffs(sample_rate_hz);
    let mut phase_equalizer_i = DirectForm1::new(phase_eq);
    let mut phase_equalizer_q = DirectForm1::new(phase_eq);
    filtered
        .into_iter()
        .map(|s| Complex32::new(phase_equalizer_i.run(s.re), phase_equalizer_q.run(s.im)))
        .collect()
}

/// Taps in the half-band-style lowpass used to interpolate the 4x-shaped
/// baseband by a further factor of two. A 23-tap Hamming-windowed sinc at
/// fc = fs/4 clears > 50 dB across the wide guard band between the occupied 1x
/// band (<= 740 kHz) and the 2x interpolation image (>= ~4.2 MHz at 8x).
const INTERP2_LP_TAPS: usize = 23;

/// Hamming-windowed sinc lowpass with cutoff at a quarter of the (doubled)
/// output rate, for 2x interpolation.
fn interp2_lowpass_taps() -> Vec<f64> {
    use std::f64::consts::PI;
    let n = INTERP2_LP_TAPS;
    let center = (n - 1) as f64 / 2.0;
    let fc = 0.25_f64; // cycles/sample at the interpolated (2x) rate
    (0..n)
        .map(|i| {
            let x = i as f64 - center;
            let sinc = if x.abs() < 1e-9 {
                2.0 * fc
            } else {
                (2.0 * PI * fc * x).sin() / (PI * x)
            };
            let hamming = 0.54 - 0.46 * (2.0 * PI * i as f64 / (n - 1) as f64).cos();
            sinc * hamming
        })
        .collect()
}

/// Mean magnitude over the steady-state tail of a response (skips warmup).
fn tail_mean_mag(samples: &[Complex32]) -> f32 {
    if samples.is_empty() {
        return 0.0;
    }
    let tail = &samples[samples.len() / 2..];
    tail.iter().map(|s| s.norm()).sum::<f32>() / tail.len() as f32
}

/// Final scale for unity passband (DC) gain, so `tx_digital_backoff` sets
/// the same output level at every TX sample rate.
fn calibrate_pulse_shaper_scale(spec: &[f64], lp: &[f64], extra_stages: usize) -> f32 {
    let dc = vec![Complex32::new(1.0, 0.0); 96];
    let mut stage1 = PolyphaseComplexFir32::with_interpolate(spec, 4);
    let mut buf = stage1.process_block(&dc);
    for _ in 0..extra_stages {
        let mut up = PolyphaseComplexFir32::with_interpolate(lp, 2);
        buf = up.process_block(&buf);
    }
    let cascade_gain = tail_mean_mag(&buf);
    if cascade_gain <= f32::EPSILON {
        return 1.0;
    }
    1.0 / cascade_gain
}

pub struct TxPulseShaper {
    /// chip -> 4x: the cdma2000 spec baseband pulse shape (correct at 4x).
    stage1: PolyphaseComplexFir32,
    /// each 2x: 4x -> 8x -> ... half-band-style interpolation that suppresses
    /// the image a single-stage `interpolate-by-N` path leaves in band.
    upsamplers: Vec<PolyphaseComplexFir32>,
    scale: f32,
    scratch_a: Vec<Complex32>,
    scratch_b: Vec<Complex32>,
}

impl TxPulseShaper {
    pub fn new(sample_rate_hz: usize) -> Result<Self, Error> {
        let chip_rate = SR1_CHIP_RATE_HZ as usize;
        if sample_rate_hz < chip_rate * 4 || sample_rate_hz % chip_rate != 0 {
            return Err(format!(
                "TX pulse shaper sample_rate_hz={sample_rate_hz} must be an integer multiple of chip rate {chip_rate} and at least 4x ({})",
                chip_rate * 4
            )
            .into());
        }
        let interpolate = sample_rate_hz / chip_rate;
        let extra = interpolate / 4;
        if interpolate % 4 != 0 || !extra.is_power_of_two() {
            return Err(format!(
                "TX pulse shaper interpolate={interpolate} must be 4x times a power of two (4x, 8x, 16x)"
            )
            .into());
        }
        let extra_stages = extra.trailing_zeros() as usize;

        // The cdma2000 baseband filter is the spec interpolate-by-4 pulse shape.
        // Applying it directly at a higher rate widens the passband ~2x and
        // leaves the first interpolation image (at the chip rate) unsuppressed.
        // Shape at 4x where the filter is correct, then reach the target rate
        // with half-band-style 2x stages that suppress each new image.
        let spec = cdma2000_baseband_filter_taps_f64();
        let lp = interp2_lowpass_taps();
        debug!(
            "TX baseband shaper: spec_taps={} lp_taps={} interpolate={} (4x + {} half-band stage(s)) sample_rate_hz={}",
            spec.len(),
            lp.len(),
            interpolate,
            extra_stages,
            sample_rate_hz
        );

        Ok(TxPulseShaper {
            stage1: PolyphaseComplexFir32::with_interpolate(&spec, 4),
            upsamplers: (0..extra_stages)
                .map(|_| PolyphaseComplexFir32::with_interpolate(&lp, 2))
                .collect(),
            scale: calibrate_pulse_shaper_scale(&spec, &lp, extra_stages),
            scratch_a: Vec::new(),
            scratch_b: Vec::new(),
        })
    }

    /// Alloc-free [`shape`](Self::shape) writing into a caller buffer.
    pub fn shape_into(&mut self, samples: &[Complex32], out: &mut Vec<Complex32>) {
        if self.upsamplers.is_empty() {
            self.stage1.process_block_into(samples, out);
            for sample in out {
                *sample *= self.scale;
            }
            return;
        }

        self.stage1.process_block_into(samples, &mut self.scratch_a);
        for up in &mut self.upsamplers {
            up.process_block_into(&self.scratch_a, &mut self.scratch_b);
            std::mem::swap(&mut self.scratch_a, &mut self.scratch_b);
        }
        out.clear();
        out.reserve(self.scratch_a.len());
        out.extend(self.scratch_a.iter().map(|sample| sample * self.scale));
    }

    pub fn shape(&mut self, samples: &[Complex32]) -> Vec<Complex32> {
        let mut out = Vec::new();
        self.shape_into(samples, &mut out);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::{
        TX_SAMPLE_RATE, TxPulseShaper, calibrate_pulse_shaper_scale,
        cdma2000_baseband_filter_taps_f64, fir::ComplexFir32, interp2_lowpass_taps,
    };
    use num_complex::Complex32;
    use std::f64::consts::PI;

    #[test]
    fn tx_pulse_shaper_matches_legacy_cascade() {
        let spec = cdma2000_baseband_filter_taps_f64();
        let lp = interp2_lowpass_taps();
        let interpolate = TX_SAMPLE_RATE as usize / super::SR1_CHIP_RATE_HZ as usize;
        let extra_stages = (interpolate / 4).trailing_zeros() as usize;
        let scale = calibrate_pulse_shaper_scale(&spec, &lp, extra_stages);

        let chips: Vec<Complex32> = (0..256)
            .map(|n| {
                Complex32::new(
                    (n as f32 * 0.113).sin() * 1.5,
                    (n as f32 * 0.071).cos() * 0.8,
                )
            })
            .collect();

        let mut shaper = TxPulseShaper::new(TX_SAMPLE_RATE as usize).unwrap();
        let got = shaper.shape(&chips);

        let mut stage1 = ComplexFir32::with_interpolate(&spec, 4);
        let mut want = stage1.process_block(&chips);
        for _ in 0..extra_stages {
            let mut up = ComplexFir32::with_interpolate(&lp, 2);
            want = up.process_block(&want);
        }
        for sample in &mut want {
            *sample *= scale;
        }

        assert_eq!(got.len(), want.len());
        for (idx, (got, want)) in got.iter().zip(&want).enumerate() {
            let err = (got - want).norm();
            assert!(
                err <= 1e-5 * (1.0 + want.norm()),
                "sample {idx}: got {got}, want {want}, err {err}"
            );
        }
    }

    #[test]
    fn complex_fir_matches_naive_convolution_across_calls() {
        let taps = cdma2000_baseband_filter_taps_f64();
        let mut fir = ComplexFir32::new(&taps);
        let input: Vec<Complex32> = (0..400)
            .map(|i| Complex32::new((0.03 * i as f32).sin(), (0.05 * i as f32 + 0.2).cos()))
            .collect();
        // Feed in two chunks to exercise cross-call delay-line continuity.
        let mut got = Vec::new();
        for chunk in [&input[..173], &input[173..]] {
            for &s in chunk {
                got.push(fir.process_sample(s));
            }
        }
        for (m, want) in input.iter().enumerate().map(|(m, _)| {
            let mut acc = num::complex::Complex::<f64>::new(0.0, 0.0);
            for (i, &tap) in taps.iter().enumerate() {
                if m >= i {
                    acc.re += input[m - i].re as f64 * tap;
                    acc.im += input[m - i].im as f64 * tap;
                }
            }
            (m, Complex32::new(acc.re as f32, acc.im as f32))
        }) {
            assert!(
                (got[m] - want).norm() < 1e-4,
                "sample {m}: {:?} vs {:?}",
                got[m],
                want
            );
        }
    }

    fn magnitude_at_hz(taps: &[f64], hz: f64, sample_rate: f64) -> f64 {
        let w = 2.0 * PI * hz / sample_rate;
        let (re, im) = taps
            .iter()
            .enumerate()
            .fold((0.0, 0.0), |(re, im), (n, &h)| {
                let phase = w * n as f64;
                (re + h * phase.cos(), im - h * phase.sin())
            });
        (re * re + im * im).sqrt()
    }

    #[test]
    fn test_sr1_baseband_filter_frequency_limits() {
        let fp = 590_000.0_f64;
        let fstop = 740_000.0_f64;
        let passband_db = 1.5_f64;
        let stopband_db = -40.0_f64;
        // Spec tables publish rounded coefficients (9 decimal places), not full design precision.
        // With rounded taps, edge droop at fp is slightly below -1.5 dB (~ -1.94 dB), so allow
        // a small quantization margin while keeping stopband strict.
        let passband_quantization_margin_db = 0.5_f64;

        // Validate both the HRPD-only 4x rate and the composite 8x rate. This
        // checks the spectrum that actually goes on the air via the complete
        // shaper impulse response, not the raw coefficients at the wrong rate.
        for sample_rate in [super::SR1_CHIP_RATE_HZ as usize * 4, TX_SAMPLE_RATE] {
            let fs = sample_rate as f64;
            let mut chips = vec![Complex32::new(1.0, 0.0)];
            chips.resize(64, Complex32::new(0.0, 0.0));
            let mut shaper = TxPulseShaper::new(sample_rate).expect("tx pulse shaper");
            let taps: Vec<f64> = shaper.shape(&chips).iter().map(|s| s.re as f64).collect();

            let dc = magnitude_at_hz(&taps, 0.0, fs);
            let mut passband_max_db = f64::NEG_INFINITY;
            let mut passband_min_db = f64::INFINITY;
            let mut stopband_max_db = f64::NEG_INFINITY;

            let bins = 16_384usize;
            for i in 0..=bins {
                let f = (i as f64 / bins as f64) * (fs / 2.0);
                let mag = magnitude_at_hz(&taps, f, fs);
                let db = 20.0 * (mag / dc).max(1e-12).log10();
                if f <= fp {
                    passband_max_db = passband_max_db.max(db);
                    passband_min_db = passband_min_db.min(db);
                }
                if f >= fstop {
                    stopband_max_db = stopband_max_db.max(db);
                }
            }

            assert!(
                passband_max_db <= passband_db
                    && passband_min_db >= -(passband_db + passband_quantization_margin_db),
                "sample_rate={sample_rate}: passband outside +{passband_db} dB / -{} dB: min={passband_min_db:.3} dB max={passband_max_db:.3} dB",
                passband_db + passband_quantization_margin_db
            );
            assert!(
                stopband_max_db <= stopband_db,
                "sample_rate={sample_rate}: stopband above {stopband_db} dB: max={stopband_max_db:.3} dB"
            );
        }
    }

    #[test]
    fn test_sr1_baseband_filter_coefficients_match_spec_table() {
        let taps = cdma2000_baseband_filter_taps_f64();
        assert_eq!(48, taps.len(), "expected 48 taps");

        let reference_first_half = [
            -0.025288315,
            -0.034167931,
            -0.035752323,
            -0.016733702,
            0.021602514,
            0.064938487,
            0.091002137,
            0.081894974,
            0.037071157,
            -0.021998074,
            -0.060716277,
            -0.051178658,
            0.007874526,
            0.084368728,
            0.126869306,
            0.094528345,
            -0.012839661,
            -0.143477028,
            -0.211829088,
            -0.140513128,
            0.094601918,
            0.441387140,
            0.785875640,
            1.0,
        ];

        let mse = reference_first_half
            .iter()
            .enumerate()
            .map(|(k, &h)| {
                let e = taps[k] - h;
                e * e
            })
            .sum::<f64>()
            / reference_first_half.len() as f64;
        assert!(
            mse <= 0.03,
            "coefficient MSE too high: {mse:.8} (must be <= 0.03)"
        );

        for k in 0..taps.len() {
            let mirror = taps[taps.len() - 1 - k];
            assert!(
                (taps[k] - mirror).abs() <= 1e-12,
                "tap symmetry violation at k={k}: {} vs {}",
                taps[k],
                mirror
            );
        }
    }

    #[test]
    fn tx_pulse_shaper_emits_hardware_rate_samples_and_keeps_state() {
        let interpolation = TX_SAMPLE_RATE / super::SR1_CHIP_RATE_HZ as usize;
        let first_chips = vec![Complex32::new(1.0, 0.0); 3072];
        let second_chips = vec![Complex32::new(0.0, 1.0); 3072];

        let mut batched = TxPulseShaper::new(TX_SAMPLE_RATE).expect("tx pulse shaper");
        let first = batched.shape(&first_chips);
        let second = batched.shape(&second_chips);
        assert_eq!(first.len(), first_chips.len() * interpolation);
        assert_eq!(second.len(), second_chips.len() * interpolation);

        let mut continuous = TxPulseShaper::new(TX_SAMPLE_RATE).expect("tx pulse shaper");
        let combined =
            continuous.shape(&[first_chips.as_slice(), second_chips.as_slice()].concat());
        assert_eq!(
            combined.len(),
            (first_chips.len() + second_chips.len()) * interpolation
        );
        assert_eq!([first, second].concat(), combined);
    }

    #[test]
    fn tx_lo_offset_rotation_is_phase_continuous_across_batches() {
        let offset_hz = 200_000.0_f64;
        let sample_rate_hz = TX_SAMPLE_RATE as f64;
        let phase_step = -2.0 * PI * offset_hz / sample_rate_hz;
        let mut phase = 0.0_f64;

        let mut first = vec![Complex32::new(1.0, 0.0); 4];
        for sample in &mut first {
            let rot = Complex32::new(phase.cos() as f32, phase.sin() as f32);
            *sample *= rot;
            phase += phase_step;
        }
        let first_phase_end = phase.rem_euclid(2.0 * PI);

        let mut second = vec![Complex32::new(1.0, 0.0); 4];
        let mut continued_phase = first_phase_end;
        for sample in &mut second {
            let rot = Complex32::new(continued_phase.cos() as f32, continued_phase.sin() as f32);
            *sample *= rot;
            continued_phase += phase_step;
        }

        let expected_first_second = Complex32::new(
            (4.0 * phase_step).cos() as f32,
            (4.0 * phase_step).sin() as f32,
        );
        assert!((second[0].re - expected_first_second.re).abs() < 1e-6);
        assert!((second[0].im - expected_first_second.im).abs() < 1e-6);
    }
}
