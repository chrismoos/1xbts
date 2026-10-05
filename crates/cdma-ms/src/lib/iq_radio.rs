use std::path::Path;
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};

use cdma_common::error::Error;
use num_complex::Complex32;

use crate::engine::ReverseBurst;
use crate::radio::Radio;

const ON_CHANNEL_TOLERANCE_HZ: f64 = 200_000.0;
const CHUNK_SAMPLES: usize = 65_536;
/// Full-scale of the 16-bit WAV samples.
const WAV_FULL_SCALE: f32 = 32_768.0;

pub fn read_iq_wav(path: &Path) -> Result<(Vec<Complex32>, f64), Error> {
    let mut reader = hound::WavReader::open(path)
        .map_err(|e| Error::from(format!("open {}: {}", path.display(), e)))?;
    let spec = reader.spec();
    if spec.channels != 2 {
        return Err(format!(
            "{}: expected 2-channel IQ WAV, found {} channels",
            path.display(),
            spec.channels
        )
        .into());
    }
    let mut samples = Vec::with_capacity(reader.len() as usize / 2);
    let mut pending: Option<f32> = None;
    for s in reader.samples::<i16>() {
        let v = s.map_err(|e| Error::from(format!("{}: {}", path.display(), e)))? as f32
            / WAV_FULL_SCALE;
        match pending.take() {
            None => pending = Some(v),
            Some(i) => samples.push(Complex32::new(i, v)),
        }
    }
    Ok((samples, spec.sample_rate as f64))
}

pub struct IqSourceRadio {
    samples: Arc<Vec<Complex32>>,
    sample_rate_hz: f64,
    center_hz: f64,
    tuned_hz: f64,
    pos: usize,
    loop_playback: bool,
    paced: bool,
    carrier_offset_hz: f64,
    phase: f64,
    started: Option<Instant>,
    delivered: usize,
    exhausted: bool,
    reverse_bursts: usize,
}

impl IqSourceRadio {
    pub fn new(samples: Vec<Complex32>, sample_rate_hz: f64, center_hz: f64) -> Self {
        IqSourceRadio {
            samples: Arc::new(samples),
            sample_rate_hz,
            center_hz,
            tuned_hz: 0.0,
            pos: 0,
            loop_playback: true,
            paced: false,
            carrier_offset_hz: 0.0,
            phase: 0.0,
            started: None,
            delivered: 0,
            exhausted: false,
            reverse_bursts: 0,
        }
    }

    pub fn from_wav(path: &Path, center_hz: f64) -> Result<Self, Error> {
        let (samples, sample_rate_hz) = read_iq_wav(path)?;
        Ok(Self::new(samples, sample_rate_hz, center_hz))
    }

    pub fn with_loop(mut self, loop_playback: bool) -> Self {
        self.loop_playback = loop_playback;
        self
    }

    pub fn with_pacing(mut self, paced: bool) -> Self {
        self.paced = paced;
        self
    }

    pub fn with_carrier_offset_hz(mut self, offset_hz: f64) -> Self {
        self.carrier_offset_hz = offset_hz;
        self
    }

    pub fn reverse_bursts(&self) -> usize {
        self.reverse_bursts
    }

    fn on_channel(&self) -> bool {
        (self.tuned_hz - self.center_hz).abs() <= ON_CHANNEL_TOLERANCE_HZ
    }
}

impl Radio for IqSourceRadio {
    fn sample_rate_hz(&self) -> f64 {
        self.sample_rate_hz
    }

    fn tune_forward(&mut self, frequency_hz: f64) -> Result<(), Error> {
        self.tuned_hz = frequency_hz;
        self.pos = 0;
        self.phase = 0.0;
        Ok(())
    }

    fn read_forward(&mut self, out: &mut Vec<Complex32>) {
        if self.exhausted || self.samples.is_empty() {
            self.exhausted = true;
            thread::sleep(Duration::from_millis(1));
            return;
        }
        let mut take = CHUNK_SAMPLES.min(self.samples.len() - self.pos);
        if self.paced {
            let started = *self.started.get_or_insert_with(Instant::now);
            let due = (started.elapsed().as_secs_f64() * self.sample_rate_hz) as usize;
            let allowed = due.saturating_sub(self.delivered);
            take = take.min(allowed);
            if take == 0 {
                thread::sleep(Duration::from_millis(1));
                return;
            }
        }
        let end = self.pos + take;
        if self.on_channel() {
            if self.carrier_offset_hz == 0.0 {
                out.extend_from_slice(&self.samples[self.pos..end]);
            } else {
                let step =
                    2.0 * std::f64::consts::PI * self.carrier_offset_hz / self.sample_rate_hz;
                for s in &self.samples[self.pos..end] {
                    let rot = Complex32::new(self.phase.cos() as f32, self.phase.sin() as f32);
                    out.push(*s * rot);
                    self.phase += step;
                }
                self.phase %= 2.0 * std::f64::consts::PI;
            }
        } else {
            out.resize(out.len() + take, Complex32::new(0.0, 0.0));
        }
        self.pos = end;
        self.delivered += take;
        if self.pos >= self.samples.len() {
            if self.loop_playback {
                self.pos = 0;
            } else {
                self.exhausted = true;
            }
        }
    }

    fn forward_exhausted(&self) -> bool {
        self.exhausted
    }

    fn write_reverse(&mut self, burst: &ReverseBurst) {
        self.reverse_bursts += 1;
        log::info!(
            "iq_radio: dropped {} burst of {} samples at chip {} (receive-only source)",
            burst.label,
            burst.samples.len(),
            burst.absolute_chip_start
        );
    }
}
