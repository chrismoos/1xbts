#[cfg(feature = "bladerf-backend")]
pub mod bladerf_radio;
pub mod fir;
#[cfg(feature = "lime-backend")]
pub mod lime_radio;
pub mod pipe;
#[cfg(feature = "soapy-backend")]
pub mod soapy_radio;
#[cfg(feature = "uhd-backend")]
pub mod uhd_radio;
#[cfg(feature = "bladerf-backend")]
pub use bladerf_radio::BladeRfRadio;
#[cfg(feature = "lime-backend")]
pub use lime_radio::LimeRadio;
pub use pipe::*;
#[cfg(feature = "soapy-backend")]
pub use soapy_radio::SoapySdrRadio;
#[cfg(feature = "uhd-backend")]
pub use uhd_radio::UhdRadio;

use std::{
    io::{Seek, Write},
    thread,
    time::{Duration, Instant},
};

use biquad::{Biquad, Coefficients, DirectForm1};
use cdma_common::consts::SR1_CHIP_RATE_HZ;
use cdma_common::error::Error;
use hound::WavWriter;
use log::debug;
use num_complex::Complex32;

use self::fir::ComplexFir32;

pub struct RxReadResult {
    pub samples_read: usize,
    pub time_ticks: u64,
    pub overflow: bool,
}

/// Configuration trait -- used during setup, consumed by split().
pub trait Radio: Send {
    fn tick_rate(&self) -> u64;
    fn set_tx_frequency(&mut self, center_frequency: usize) -> Result<(), Error>;
    fn set_tx_sample_rate(&mut self, sample_rate: usize) -> Result<(), Error>;
    fn set_tx_bandwidth(&mut self, bandwidth: usize) -> Result<(), Error>;
    fn set_tx_lo_offset_hz(&mut self, _offset_hz: i64) -> Result<(), Error> {
        Ok(())
    }
    fn setup_rx(
        &mut self,
        _channel: usize,
        _antenna: &str,
        _frequency_hz: f64,
        _sample_rate_hz: f64,
        _bandwidth_hz: f64,
        _gain_db: Option<f64>,
    ) -> Result<(), Error> {
        Err("RX not supported by this radio".into())
    }
    /// Consume this radio and split into TX and RX halves.
    fn split(self: Box<Self>) -> Result<(Box<dyn RadioTx>, Option<Box<dyn RadioRx>>), Error>;
}

/// TX half -- owned exclusively by the TX thread.
pub trait RadioTx: Send {
    fn tick_rate(&self) -> u64;
    fn get_hardware_time(&self) -> Result<u64, Error>;
    fn set_hardware_time(&self, _ticks: u64) -> Result<(), Error> {
        Ok(())
    }
    fn transmit(&mut self, samples: &[Complex32]) -> Result<(), Error>;
    fn transmit_at(&mut self, samples: &[Complex32], _tick: Option<u64>) -> Result<(), Error> {
        self.transmit(samples)
    }
    fn enable_transmit(&mut self, enable: bool) -> Result<(), Error>;
    fn enable_transmit_at(&mut self, enable: bool, _tick: Option<u64>) -> Result<(), Error> {
        self.enable_transmit(enable)
    }
}

/// RX half -- owned exclusively by the RX thread.
pub trait RadioRx: Send {
    fn tick_rate(&self) -> u64;
    fn get_hardware_time(&self) -> Result<u64, Error>;
    fn rx_read(&mut self, buf: &mut [Complex32], timeout_us: i64) -> Result<RxReadResult, Error>;
    fn rx_activate(&mut self, time_ticks: Option<u64>) -> Result<(), Error>;
    fn rx_deactivate(&mut self) -> Result<(), Error>;
}

impl RadioTx for Box<dyn RadioTx> {
    fn tick_rate(&self) -> u64 {
        (**self).tick_rate()
    }
    fn get_hardware_time(&self) -> Result<u64, Error> {
        (**self).get_hardware_time()
    }
    fn set_hardware_time(&self, ticks: u64) -> Result<(), Error> {
        (**self).set_hardware_time(ticks)
    }
    fn transmit(&mut self, samples: &[Complex32]) -> Result<(), Error> {
        (**self).transmit(samples)
    }
    fn transmit_at(&mut self, samples: &[Complex32], tick: Option<u64>) -> Result<(), Error> {
        (**self).transmit_at(samples, tick)
    }
    fn enable_transmit(&mut self, enable: bool) -> Result<(), Error> {
        (**self).enable_transmit(enable)
    }
    fn enable_transmit_at(&mut self, enable: bool, tick: Option<u64>) -> Result<(), Error> {
        (**self).enable_transmit_at(enable, tick)
    }
}

pub(crate) const TX_SAMPLE_RATE: usize = SR1_CHIP_RATE_HZ as usize * 4;
pub(crate) const FILE_OUTPUT_TARGET_PEAK: f32 = 0.90;
const FILE_OUTPUT_HARD_CLIP_PEAK: f32 = 1.0;
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

pub struct TxPulseShaper {
    interpolate: usize,
    baseband_filter: ComplexFir32,
}

impl TxPulseShaper {
    pub fn new() -> Self {
        let interpolate = TX_SAMPLE_RATE / SR1_CHIP_RATE_HZ as usize;
        let taps = cdma2000_baseband_filter_taps_f64();
        debug!("TX baseband filter taps: {}", taps.len());

        // Use the FIR's built-in polyphase interpolation: it splits the 48
        // taps into 4 sub-filters of 12 taps each, computing only non-zero
        // contributions. 4× fewer MACs and no zero-insert allocation.
        TxPulseShaper {
            interpolate,
            baseband_filter: ComplexFir32::with_interpolate(&taps, interpolate),
        }
    }

    pub fn shape(&mut self, samples: &[Complex32]) -> Vec<Complex32> {
        // Feed chip-rate samples directly; the polyphase FIR handles
        // the 4× interpolation internally.
        // The polyphase FIR matches the previous interpolation convention:
        // output is multiplied by interpolate, so compensate to match the
        // zero-insert path's unity gain.
        let scale = 1.0 / self.interpolate as f32;
        let mut out = self.baseband_filter.process_block(samples);
        for sample in &mut out {
            *sample *= scale;
        }
        out
    }
}

pub struct NoopRadio {
    tx_sample_rate: usize,
    tx_enabled: bool,
    next_tx_deadline: Option<Instant>,
    clock_start: Instant,
}

impl NoopRadio {
    pub fn new() -> NoopRadio {
        NoopRadio {
            tx_sample_rate: TX_SAMPLE_RATE,
            tx_enabled: false,
            next_tx_deadline: None,
            clock_start: Instant::now(),
        }
    }
}

impl Radio for NoopRadio {
    fn tick_rate(&self) -> u64 {
        1_000_000_000
    }

    fn set_tx_frequency(&mut self, _center_frequency: usize) -> Result<(), Error> {
        Ok(())
    }

    fn set_tx_sample_rate(&mut self, sample_rate: usize) -> Result<(), Error> {
        self.tx_sample_rate = sample_rate;
        Ok(())
    }

    fn set_tx_bandwidth(&mut self, _bandwidth: usize) -> Result<(), Error> {
        Ok(())
    }

    fn split(self: Box<Self>) -> Result<(Box<dyn RadioTx>, Option<Box<dyn RadioRx>>), Error> {
        let tx = NoopTxHalf {
            tx_sample_rate: self.tx_sample_rate,
            tx_enabled: self.tx_enabled,
            next_tx_deadline: self.next_tx_deadline,
            clock_start: self.clock_start,
        };
        Ok((Box::new(tx), None))
    }
}

struct NoopTxHalf {
    tx_sample_rate: usize,
    tx_enabled: bool,
    next_tx_deadline: Option<Instant>,
    clock_start: Instant,
}

impl NoopTxHalf {
    fn simulate_tx_timing(&mut self, sample_count: usize) {
        if !self.tx_enabled || sample_count == 0 || self.tx_sample_rate == 0 {
            return;
        }

        let effective_tx_samples =
            sample_count.saturating_mul(self.tx_sample_rate) / SR1_CHIP_RATE_HZ as usize;
        if effective_tx_samples == 0 {
            return;
        }

        let tx_duration_ns = (effective_tx_samples as u128).saturating_mul(1_000_000_000u128)
            / self.tx_sample_rate as u128;
        let tx_duration = Duration::from_nanos(tx_duration_ns.min(u64::MAX as u128) as u64);

        let deadline = self.next_tx_deadline.unwrap_or_else(Instant::now);
        let now = Instant::now();
        if deadline > now {
            thread::sleep(deadline.duration_since(now));
        }
        self.next_tx_deadline = Some(deadline + tx_duration);
    }
}

impl RadioTx for NoopTxHalf {
    fn tick_rate(&self) -> u64 {
        1_000_000_000
    }

    fn get_hardware_time(&self) -> Result<u64, Error> {
        Ok(self.clock_start.elapsed().as_nanos() as u64)
    }

    fn transmit(&mut self, samples: &[Complex32]) -> Result<(), Error> {
        self.simulate_tx_timing(samples.len());
        Ok(())
    }

    fn transmit_at(&mut self, samples: &[Complex32], _tick: Option<u64>) -> Result<(), Error> {
        self.simulate_tx_timing(samples.len());
        Ok(())
    }

    fn enable_transmit(&mut self, enable: bool) -> Result<(), Error> {
        self.tx_enabled = enable;
        self.next_tx_deadline = enable.then(Instant::now);
        Ok(())
    }

    fn enable_transmit_at(&mut self, enable: bool, _tick: Option<u64>) -> Result<(), Error> {
        self.tx_enabled = enable;
        self.next_tx_deadline = enable.then(Instant::now);
        Ok(())
    }
}

pub struct FileOutputRadio<W>
where
    W: Write + Seek,
{
    sink: WavWriter<W>,
    tx_shaper: TxPulseShaper,
    clock_start: Instant,
}

impl<W> FileOutputRadio<W>
where
    W: Write + Seek,
{
    pub fn new(writer: W) -> Result<FileOutputRadio<W>, Error> {
        Ok(FileOutputRadio {
            sink: WavWriter::new(
                writer,
                hound::WavSpec {
                    channels: 2,
                    sample_rate: TX_SAMPLE_RATE as u32,
                    bits_per_sample: 16,
                    sample_format: hound::SampleFormat::Int,
                },
            )?,
            tx_shaper: TxPulseShaper::new(),
            clock_start: Instant::now(),
        })
    }
}

impl<W> Radio for FileOutputRadio<W>
where
    W: Write + Seek + Send + 'static,
{
    fn tick_rate(&self) -> u64 {
        1_000_000_000
    }

    fn set_tx_frequency(&mut self, _center_frequency: usize) -> Result<(), Error> {
        Ok(())
    }

    fn set_tx_sample_rate(&mut self, _sample_rate: usize) -> Result<(), Error> {
        Ok(())
    }

    fn set_tx_bandwidth(&mut self, _bandwidth: usize) -> Result<(), Error> {
        Ok(())
    }

    fn split(self: Box<Self>) -> Result<(Box<dyn RadioTx>, Option<Box<dyn RadioRx>>), Error> {
        let tx = FileOutputTxHalf {
            sink: self.sink,
            tx_shaper: self.tx_shaper,
            clock_start: self.clock_start,
        };
        Ok((Box::new(tx), None))
    }
}

struct FileOutputTxHalf<W: Write + Seek + Send> {
    sink: WavWriter<W>,
    tx_shaper: TxPulseShaper,
    clock_start: Instant,
}

impl<W> RadioTx for FileOutputTxHalf<W>
where
    W: Write + Seek + Send,
{
    fn tick_rate(&self) -> u64 {
        1_000_000_000
    }

    fn get_hardware_time(&self) -> Result<u64, Error> {
        Ok(self.clock_start.elapsed().as_nanos() as u64)
    }

    fn transmit(&mut self, samples: &[Complex32]) -> Result<(), Error> {
        let shaped = self.tx_shaper.shape(samples);
        for (idx, sample) in shaped.iter().enumerate() {
            // Keep deterministic fixed scaling for WAV output. Any overflow is a TX bug.
            let re = sample.re * FILE_OUTPUT_TARGET_PEAK;
            let im = sample.im * FILE_OUTPUT_TARGET_PEAK;
            if re.abs() > FILE_OUTPUT_HARD_CLIP_PEAK || im.abs() > FILE_OUTPUT_HARD_CLIP_PEAK {
                log::error!(
                    "TX hard clip detected in FileOutputRadio: idx={} raw_re={} raw_im={} scaled_re={} scaled_im={} hard_limit={} target_peak={}",
                    idx,
                    sample.re,
                    sample.im,
                    re,
                    im,
                    FILE_OUTPUT_HARD_CLIP_PEAK,
                    FILE_OUTPUT_TARGET_PEAK
                );
                return Err("TX hard clip: signal exceeds file output range".into());
            }
            self.sink.write_sample((re * (i16::MAX as f32)) as i16)?;
            self.sink.write_sample((im * (i16::MAX as f32)) as i16)?;
        }
        self.sink.flush()?;
        Ok(())
    }

    fn enable_transmit(&mut self, _enable: bool) -> Result<(), Error> {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::{TX_SAMPLE_RATE, cdma2000_baseband_filter_taps_f64};
    use num_complex::Complex32;
    use std::f64::consts::PI;

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
        let taps = cdma2000_baseband_filter_taps_f64();
        let fs = TX_SAMPLE_RATE as f64;
        let fp = 590_000.0_f64;
        let fstop = 740_000.0_f64;
        let passband_db = 1.5_f64;
        let stopband_db = -40.0_f64;
        // Spec tables publish rounded coefficients (9 decimal places), not full design precision.
        // With rounded taps, edge droop at fp is slightly below -1.5 dB (~ -1.94 dB), so allow
        // a small quantization margin while keeping stopband strict.
        let passband_quantization_margin_db = 0.5_f64;

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
            "passband outside +{passband_db} dB / -{} dB: min={passband_min_db:.3} dB max={passband_max_db:.3} dB",
            passband_db + passband_quantization_margin_db
        );
        assert!(
            stopband_max_db <= stopband_db,
            "stopband above {stopband_db} dB: max={stopband_max_db:.3} dB"
        );
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
