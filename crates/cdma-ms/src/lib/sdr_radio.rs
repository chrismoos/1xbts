use std::collections::VecDeque;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::sync::{Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::Duration;

use cdma_bts::bts::config::RadioConfig as SdrRadioConfig;
use cdma_bts::bts::launcher::{RadioBuildOptions, build_radio_from_config};
use cdma_common::error::Error;
use cdma_common::radio::{RadioRx, RadioTx};
use num_complex::Complex32;

use crate::engine::ReverseBurst;
use crate::radio::{Radio, TxCalibration, TxTrim};

const RX_READ_SAMPLES: usize = 65_536;
const RX_READ_TIMEOUT_US: i64 = 100_000;
const QUEUE_CAPACITY_SECONDS: f64 = 1.0;
const CHUNK_SAMPLES: usize = 65_536;
/// A timestamp jump shorter than this is filled with silence so the stream
/// stays continuous. Longer jumps reset the stream position instead.
const MAX_GAP_FILL_SECONDS: f64 = 0.1;
const GAP_TOLERANCE_SAMPLES: u64 = 8;
const DEFAULT_SETTLE_MS: u64 = 20;
/// A burst must reach the hardware at least this far ahead of its time.
const TX_MIN_LEAD_SECONDS: f64 = 0.005;
// Pilot-only chunks need headroom for full-rate R-FCH without changing pilot gain.
const TRAFFIC_PEAK_RESERVE: f32 = 2.0;
const TX_BANDWIDTH_HZ: usize = 3_000_000;

enum RxCommand {
    Tune {
        frequency_hz: f64,
        done: SyncSender<Result<(), String>>,
    },
    SetGain {
        gain_db: f64,
        done: SyncSender<Result<(), String>>,
    },
    Stop,
}

struct TimedBurst {
    samples: Vec<Complex32>,
    tick: u64,
    end_of_burst: bool,
    label: &'static str,
    power_dbm: Option<f32>,
    estimated_power_dbm: Option<f32>,
    digital_backoff_db: f32,
    /// Bursts from an older generation predate a Mute or Stop and are dropped.
    generation: u64,
}

enum TxCommand {
    Burst(TimedBurst),
    Mute,
    Tune {
        frequency_hz: f64,
        done: SyncSender<Result<(), String>>,
    },
    Stop,
}

struct Segment {
    tick: Option<u64>,
    samples: Vec<Complex32>,
    discontinuity: bool,
}

#[derive(Default)]
struct Queue {
    segments: VecDeque<Segment>,
    len: usize,
    front_offset: usize,
    delivered: u64,
    /// The stream position and hardware tick of the latest timestamped
    /// sample handed out.
    anchor: Option<(u64, u64)>,
}

impl Queue {
    fn push(&mut self, segment: Segment) {
        self.len += segment.samples.len();
        self.segments.push_back(segment);
    }

    fn drop_oldest(&mut self, capacity: usize) -> usize {
        let mut dropped = 0;
        while self.len > capacity {
            let Some(front) = self.segments.pop_front() else {
                break;
            };
            let kept = front.samples.len() - self.front_offset;
            self.len -= kept;
            dropped += kept;
            self.front_offset = 0;
        }
        if dropped > 0
            && let Some(front) = self.segments.front_mut()
        {
            front.discontinuity = true;
        }
        dropped
    }

    fn clear(&mut self) {
        self.segments.clear();
        self.len = 0;
        self.front_offset = 0;
    }

    fn take(&mut self, max: usize, out: &mut Vec<Complex32>) -> bool {
        let mut remaining = max;
        let mut discontinuity = false;
        while remaining > 0 {
            let Some(front) = self.segments.front() else {
                break;
            };
            if self.front_offset == 0 && front.discontinuity {
                if !out.is_empty() {
                    break;
                }
                discontinuity = true;
            }
            if let Some(tick) = front.tick {
                self.anchor = Some((self.delivered - self.front_offset as u64, tick));
            }
            let available = front.samples.len() - self.front_offset;
            let n = available.min(remaining);
            out.extend_from_slice(&front.samples[self.front_offset..self.front_offset + n]);
            self.front_offset += n;
            self.delivered += n as u64;
            self.len -= n;
            remaining -= n;
            if self.front_offset == front.samples.len() {
                self.segments.pop_front();
                self.front_offset = 0;
            }
        }
        discontinuity
    }
}

struct Shared {
    queue: Mutex<Queue>,
    overflows: AtomicU64,
    gaps: AtomicU64,
    running: AtomicBool,
    tx_bursts: AtomicU64,
    tx_late: AtomicU64,
    tx_generation: AtomicU64,
}

impl Shared {
    fn new() -> Self {
        Shared {
            queue: Mutex::new(Queue::default()),
            overflows: AtomicU64::new(0),
            gaps: AtomicU64::new(0),
            running: AtomicBool::new(false),
            tx_bursts: AtomicU64::new(0),
            tx_late: AtomicU64::new(0),
            tx_generation: AtomicU64::new(0),
        }
    }
}

/// The TX queue is unbounded, so a key-down must skip the bursts queued ahead
/// of it rather than wait behind them.
fn send_tx_priority(shared: &Shared, commands: &Sender<TxCommand>, command: TxCommand) {
    shared.tx_generation.fetch_add(1, Ordering::Relaxed);
    let _ = commands.send(command);
}

const MIN_TRAFFIC_RELATIVE_GAIN_DB: f32 = -32.0;

struct TrafficScale {
    scale: f32,
    minimum_scale: f32,
    requested_dbm: f32,
}

// Four CS12 complex samples occupy three complete 32-bit wire words.
const TRAFFIC_SAMPLE_ALIGNMENT: usize = 4;
const MAX_TRAFFIC_TIMING_GAP_SAMPLES: u64 = 64;

#[derive(Default)]
struct TrafficTimeline {
    next_sample: Option<u64>,
    pending: Vec<Complex32>,
}

struct TrafficSamples {
    start_sample: u64,
    samples: Vec<Complex32>,
}

impl TrafficTimeline {
    fn prepare(
        &mut self,
        start_sample: u64,
        samples: &[Complex32],
        end_of_burst: bool,
    ) -> TrafficSamples {
        let mut next_sample = self.next_sample.unwrap_or(start_sample);
        let end_sample = next_sample + self.pending.len() as u64;
        let gap = start_sample.saturating_sub(end_sample);
        if gap > MAX_TRAFFIC_TIMING_GAP_SAMPLES {
            log::warn!("sdr_radio: traffic timing discontinuity gap_samples={gap}");
            self.pending.clear();
            next_sample = start_sample;
        } else {
            self.pending
                .resize(self.pending.len() + gap as usize, Complex32::default());
        }
        let end_sample = next_sample + self.pending.len() as u64;
        let overlap = end_sample
            .saturating_sub(start_sample)
            .min(samples.len() as u64) as usize;
        if overlap > 0 {
            log::debug!("sdr_radio: traffic timing overlap trimmed_samples={overlap}");
        }
        self.pending.extend_from_slice(&samples[overlap..]);
        if end_of_burst {
            self.pending.resize(
                self.pending
                    .len()
                    .next_multiple_of(TRAFFIC_SAMPLE_ALIGNMENT),
                Complex32::default(),
            );
        }
        let count = self.pending.len() / TRAFFIC_SAMPLE_ALIGNMENT * TRAFFIC_SAMPLE_ALIGNMENT;
        let tail = self.pending.split_off(count);
        let samples = std::mem::replace(&mut self.pending, tail);
        self.next_sample = Some(next_sample + count as u64);
        if end_of_burst {
            *self = Self::default();
        }
        TrafficSamples {
            start_sample: next_sample,
            samples,
        }
    }
}

impl TrafficScale {
    fn apply(&mut self, requested_dbm: f32, delta_db: f32, limit: f32) -> f32 {
        let wanted =
            self.scale * 10f32.powf((requested_dbm - self.requested_dbm + delta_db) / 20.0);
        self.scale = wanted.clamp(self.minimum_scale.min(limit), limit);
        self.requested_dbm = requested_dbm;
        self.scale
    }
}

#[derive(Default)]
struct TransmitCarrier {
    phase_rad: f64,
    next_tick: Option<f64>,
    frequency_hz: f64,
}

impl TransmitCarrier {
    fn correct(
        &mut self,
        samples: &mut [Complex32],
        tick: u64,
        tick_rate: u64,
        sample_rate_hz: f64,
        frequency_hz: f64,
    ) {
        if let Some(next_tick) = self.next_tick {
            self.phase_rad += std::f64::consts::TAU * self.frequency_hz * (tick as f64 - next_tick)
                / tick_rate as f64;
        }
        let step_rad = std::f64::consts::TAU * frequency_hz / sample_rate_hz;
        let step = num_complex::Complex64::from_polar(1.0, step_rad);
        let mut rotation = num_complex::Complex64::from_polar(1.0, self.phase_rad);
        for sample in samples.iter_mut() {
            *sample *= Complex32::new(rotation.re as f32, rotation.im as f32);
            rotation *= step;
        }
        self.phase_rad =
            (self.phase_rad + step_rad * samples.len() as f64).rem_euclid(std::f64::consts::TAU);
        self.next_tick =
            Some(tick as f64 + samples.len() as f64 * tick_rate as f64 / sample_rate_hz);
        self.frequency_hz = frequency_hz;
    }
}

pub struct SdrRadio {
    sample_rate_hz: f64,
    forward_hz: f64,
    reverse_hz: f64,
    settle: Duration,
    rx: Option<Box<dyn RadioRx>>,
    tx: Option<Box<dyn RadioTx>>,
    tick_rate: u64,
    shared: Arc<Shared>,
    rx_commands: Option<Sender<RxCommand>>,
    tx_commands: Option<Sender<TxCommand>>,
    rx_thread: Option<JoinHandle<()>>,
    tx_thread: Option<JoinHandle<()>>,
    calibration: TxCalibration,
    rx_reference_dbm: Option<f32>,
    traffic_scale: Option<TrafficScale>,
    traffic_timeline: TrafficTimeline,
    transmit_carrier: TransmitCarrier,
    forward_discontinuity: bool,
}

impl SdrRadio {
    /// Open the configured SDR with its receiver at `forward_hz` and its
    /// transmitter at `reverse_hz`, both idle until `start`. `sample_rate_hz`
    /// must match the config's RX rate.
    pub fn open(
        config: &SdrRadioConfig,
        forward_hz: f64,
        reverse_hz: f64,
        sample_rate_hz: f64,
        mut calibration: TxCalibration,
    ) -> Result<Self, Error> {
        if !(0.0..=1.0).contains(&calibration.peak_limit) || calibration.peak_limit == 0.0 {
            return Err("tx_peak_limit must be above zero and no greater than one".into());
        }
        if calibration
            .estimated_full_scale_dbm
            .is_some_and(|estimate_dbm| !estimate_dbm.is_finite())
        {
            return Err("estimated full-scale TX power must be finite".into());
        }
        if !calibration.access_initial_backoff_db.is_finite()
            || calibration.access_initial_backoff_db < 0.0
        {
            return Err("access_initial_backoff_db must be finite and nonnegative".into());
        }
        let rate = sample_rate_hz as usize;
        let mut radio = build_radio_from_config(
            config,
            forward_hz as usize,
            RadioBuildOptions {
                null_radio: false,
                configure_rx: true,
                tx_sample_rate_hz: rate,
                rx_sample_rate_hz: rate,
                rx_bandwidth_hz: rate,
                realtime: Default::default(),
            },
        )?;
        if let Some(reference_dbm) = radio.tx_full_scale_power_estimate_dbm() {
            calibration.tx_reference_dbm = reference_dbm;
            calibration.estimated_full_scale_dbm = Some(reference_dbm);
        }
        calibration.tx_delay_samples = config
            .tx_sample_delay()
            .or_else(|| radio.tx_sample_delay())
            .unwrap_or(calibration.tx_delay_samples);
        let rx_reference_dbm = radio.rx_reference_dbm();
        radio.set_tx_bandwidth(TX_BANDWIDTH_HZ)?;
        radio.set_tx_sample_rate(sample_rate_hz as usize)?;
        radio.set_tx_frequency(reverse_hz as usize)?;
        let tick_rate = radio.tick_rate();
        let (tx, rx) = radio.split()?;
        let rx = rx.ok_or_else(|| Error::from("the radio provides no receiver"))?;
        let estimated_reference = calibration
            .estimated_full_scale_dbm
            .map_or_else(|| "unavailable".to_string(), |dbm| format!("{dbm:.1} dBm"));
        log::info!(
            "sdr_radio: forward {:.3} MHz, reverse {:.3} MHz, tx power control {} reference {:.1} dBm, estimated full-scale {}, tx delay {} samples, peak {:.2}, relative access power {} initial backoff {:.1} dB",
            forward_hz / 1e6,
            reverse_hz / 1e6,
            calibration.power_control,
            calibration.tx_reference_dbm,
            estimated_reference,
            calibration.tx_delay_samples,
            calibration.peak_limit,
            calibration.relative_access_power,
            calibration.access_initial_backoff_db
        );
        Ok(SdrRadio {
            sample_rate_hz,
            forward_hz,
            reverse_hz,
            settle: Duration::from_millis(DEFAULT_SETTLE_MS),
            rx: Some(rx),
            tx: Some(tx),
            tick_rate,
            shared: Arc::new(Shared::new()),
            rx_commands: None,
            tx_commands: None,
            rx_thread: None,
            tx_thread: None,
            calibration,
            rx_reference_dbm,
            traffic_scale: None,
            traffic_timeline: TrafficTimeline::default(),
            transmit_carrier: TransmitCarrier::default(),
            forward_discontinuity: false,
        })
    }

    pub fn from_parts(
        tx: Box<dyn RadioTx>,
        rx: Box<dyn RadioRx>,
        tick_rate: u64,
        sample_rate_hz: f64,
        calibration: TxCalibration,
    ) -> Self {
        SdrRadio {
            sample_rate_hz,
            forward_hz: 0.0,
            reverse_hz: 0.0,
            settle: Duration::from_millis(DEFAULT_SETTLE_MS),
            rx: Some(rx),
            tx: Some(tx),
            tick_rate,
            shared: Arc::new(Shared::new()),
            rx_commands: None,
            tx_commands: None,
            rx_thread: None,
            tx_thread: None,
            calibration,
            rx_reference_dbm: None,
            traffic_scale: None,
            traffic_timeline: TrafficTimeline::default(),
            transmit_carrier: TransmitCarrier::default(),
            forward_discontinuity: false,
        }
    }

    pub fn with_settle_ms(mut self, settle_ms: u64) -> Self {
        self.settle = Duration::from_millis(settle_ms);
        self
    }

    pub fn tx_counts(&self) -> (u64, u64) {
        (
            self.shared.tx_bursts.load(Ordering::Relaxed),
            self.shared.tx_late.load(Ordering::Relaxed),
        )
    }

    fn ticks_per_sample(&self) -> f64 {
        self.tick_rate as f64 / self.sample_rate_hz
    }

    fn scale_to_power(
        &self,
        samples: &[Complex32],
        power_dbm: f32,
        digital_backoff_db: f32,
    ) -> (Vec<Complex32>, f32) {
        let n = samples.len().max(1) as f32;
        let rms = (samples.iter().map(|s| s.norm_sqr()).sum::<f32>() / n)
            .sqrt()
            .max(1e-9);
        let peak = samples
            .iter()
            .map(|s| s.norm())
            .fold(0.0f32, f32::max)
            .max(1e-9);
        if !self.calibration.power_control {
            let scale = self.calibration.peak_limit * 10f32.powf(-digital_backoff_db / 20.0) / peak;
            let actual_dbm = self.calibration.tx_reference_dbm + 20.0 * (rms * scale).log10();
            return (samples.iter().map(|s| s * scale).collect(), actual_dbm);
        }
        let wanted = 10f32.powf((power_dbm - self.calibration.tx_reference_dbm) / 20.0) / rms;
        let limit = self.calibration.peak_limit / peak;
        let scale = wanted.min(limit);
        let actual_dbm = power_dbm + 20.0 * (scale / wanted).log10();
        (samples.iter().map(|s| s * scale).collect(), actual_dbm)
    }

    fn scale_traffic(
        &mut self,
        samples: &[Complex32],
        power_dbm: f32,
        delta_db: f32,
    ) -> (Vec<Complex32>, f32) {
        let n = samples.len().max(1) as f32;
        let rms = (samples.iter().map(|s| s.norm_sqr()).sum::<f32>() / n)
            .sqrt()
            .max(1e-9);
        let peak = samples
            .iter()
            .map(|s| s.norm())
            .fold(0.0f32, f32::max)
            .max(1e-9);
        let state = self.traffic_scale.get_or_insert_with(|| {
            let limit = self.calibration.peak_limit / (peak * TRAFFIC_PEAK_RESERVE);
            let scale = if self.calibration.power_control {
                let wanted =
                    10f32.powf((power_dbm - self.calibration.tx_reference_dbm) / 20.0) / rms;
                wanted.min(limit)
            } else {
                limit
            };
            TrafficScale {
                scale,
                minimum_scale: scale * 10f32.powf(MIN_TRAFFIC_RELATIVE_GAIN_DB / 20.0),
                requested_dbm: power_dbm,
            }
        });
        let previous_scale = state.scale;
        let limit = self.calibration.peak_limit / peak;
        let scale = if self.calibration.power_control {
            state.apply(power_dbm, delta_db, limit)
        } else {
            state.scale.min(limit)
        };
        let actual_dbm = self.calibration.tx_reference_dbm + 20.0 * (rms * scale).log10();
        log::debug!(
            "sdr_radio: traffic power rms_dbfs={:.2} peak_dbfs={:.2} gain_change_db={:+.2} peak_limited={} floor_limited={}",
            20.0 * (rms * scale).log10(),
            20.0 * (peak * scale).log10(),
            20.0 * (scale / previous_scale).log10(),
            scale >= limit,
            scale <= state.minimum_scale,
        );
        (samples.iter().map(|s| s * scale).collect(), actual_dbm)
    }
}

fn rx_thread(
    mut rx: Box<dyn RadioRx>,
    shared: Arc<Shared>,
    commands: Receiver<RxCommand>,
    sample_rate_hz: f64,
    settle: Duration,
) {
    let tick_rate = rx.tick_rate() as f64;
    let ticks_per_sample = tick_rate / sample_rate_hz;
    let capacity = (sample_rate_hz * QUEUE_CAPACITY_SECONDS) as usize;
    let max_gap = (sample_rate_hz * MAX_GAP_FILL_SECONDS) as u64;
    let mut buf = vec![Complex32::new(0.0, 0.0); RX_READ_SAMPLES];
    let mut expected_tick: Option<u64> = None;

    if let Err(e) = rx.rx_activate(None) {
        log::error!("sdr_radio: RX activate failed: {}", e);
        shared.running.store(false, Ordering::Relaxed);
        return;
    }
    log::info!(
        "sdr_radio: RX streaming at {} Msps (tick rate {} Hz)",
        sample_rate_hz / 1e6,
        tick_rate
    );

    loop {
        match commands.try_recv() {
            Ok(RxCommand::Tune { frequency_hz, done }) => {
                let result = rx.set_rx_frequency(frequency_hz).map_err(|e| e.to_string());
                if result.is_ok() {
                    thread::sleep(settle);
                    // Flush the retune transient from the hardware. A live
                    // stream never empties, so bound this by wall clock rather
                    // than looping until `rx_read` returns nothing.
                    let flush_deadline = std::time::Instant::now() + settle;
                    while std::time::Instant::now() < flush_deadline {
                        match rx.rx_read(&mut buf, 0) {
                            Ok(r) if r.samples_read > 0 => continue,
                            _ => break,
                        }
                    }
                    shared.queue.lock().unwrap().clear();
                    expected_tick = None;
                    log::debug!("sdr_radio: retuned to {:.3} MHz", frequency_hz / 1e6);
                }
                let _ = done.send(result);
            }
            Ok(RxCommand::SetGain { gain_db, done }) => {
                let result = rx.set_rx_gain(gain_db).map_err(|e| e.to_string());
                if result.is_ok() {
                    log::info!("sdr_radio: RX gain set to {:.1} dB", gain_db);
                }
                let _ = done.send(result);
            }
            Ok(RxCommand::Stop) | Err(mpsc::TryRecvError::Disconnected) => break,
            Err(mpsc::TryRecvError::Empty) => {}
        }

        let result = match rx.rx_read(&mut buf, RX_READ_TIMEOUT_US) {
            Ok(r) => r,
            Err(e) => {
                log::warn!("sdr_radio: RX read error: {}", e);
                thread::sleep(Duration::from_millis(10));
                continue;
            }
        };
        if result.overflow {
            shared.overflows.fetch_add(1, Ordering::Relaxed);
        }
        if result.samples_read == 0 {
            continue;
        }

        let mut queue = shared.queue.lock().unwrap();
        let tick = (result.time_ticks > 0).then_some(result.time_ticks);
        let mut discontinuity = result.overflow && tick.is_none();
        if let Some(t) = tick {
            if let Some(expected) = expected_tick {
                if t < expected
                    && ((expected - t) as f64 / ticks_per_sample) as u64 > GAP_TOLERANCE_SAMPLES
                {
                    discontinuity = true;
                }
                let jump_ticks = t.saturating_sub(expected);
                let gap_samples = (jump_ticks as f64 / ticks_per_sample) as u64;
                if gap_samples > GAP_TOLERANCE_SAMPLES {
                    if gap_samples <= max_gap {
                        shared.gaps.fetch_add(1, Ordering::Relaxed);
                        log::debug!(
                            "sdr_radio: filled a {} sample RX timestamp gap ({:.1} ms)",
                            gap_samples,
                            gap_samples as f64 / sample_rate_hz * 1e3
                        );
                        queue.push(Segment {
                            tick: Some(expected),
                            samples: vec![Complex32::new(0.0, 0.0); gap_samples as usize],
                            discontinuity: false,
                        });
                    } else {
                        discontinuity = true;
                        shared.overflows.fetch_add(1, Ordering::Relaxed);
                        log::warn!(
                            "sdr_radio: {} sample RX gap too long to fill, continuity lost",
                            gap_samples
                        );
                    }
                }
            }
            expected_tick = Some(t + (result.samples_read as f64 * ticks_per_sample) as u64);
        }
        queue.push(Segment {
            tick,
            samples: buf[..result.samples_read].to_vec(),
            discontinuity,
        });
        if queue.len > capacity {
            let dropped = queue.drop_oldest(capacity);
            shared.overflows.fetch_add(1, Ordering::Relaxed);
            log::warn!(
                "sdr_radio: engine fell behind, dropped {} buffered samples",
                dropped
            );
        }
    }

    if let Err(e) = rx.rx_deactivate() {
        log::warn!("sdr_radio: RX deactivate failed: {}", e);
    }
    shared.running.store(false, Ordering::Relaxed);
    log::info!("sdr_radio: RX thread stopped");
}

fn tx_thread(mut tx: Box<dyn RadioTx>, shared: Arc<Shared>, commands: Receiver<TxCommand>) {
    let tick_rate = tx.tick_rate() as f64;
    let min_lead_ticks = (TX_MIN_LEAD_SECONDS * tick_rate) as u64;
    let mut in_burst = false;
    let mut flushed: u64 = 0;
    log::info!("sdr_radio: TX ready");
    while let Ok(command) = commands.recv() {
        match command {
            TxCommand::Burst(burst) => {
                if burst.generation != shared.tx_generation.load(Ordering::Relaxed) {
                    flushed += 1;
                    continue;
                }
                let now = match tx.get_hardware_time() {
                    Ok(t) => t,
                    Err(e) => {
                        log::warn!(
                            "sdr_radio: hardware time unavailable, dropping burst: {}",
                            e
                        );
                        if burst.end_of_burst && in_burst {
                            end_burst(tx.as_mut(), &mut in_burst);
                        }
                        continue;
                    }
                };
                if burst.tick < now + min_lead_ticks {
                    shared.tx_late.fetch_add(1, Ordering::Relaxed);
                    log::warn!(
                        "sdr_radio: {} burst is {:.1} ms late, dropped",
                        burst.label,
                        (now + min_lead_ticks - burst.tick) as f64 / tick_rate * 1e3
                    );
                    if burst.end_of_burst && in_burst {
                        end_burst(tx.as_mut(), &mut in_burst);
                    }
                    continue;
                }
                if !in_burst {
                    if let Err(e) = tx.enable_transmit_at(true, Some(burst.tick)) {
                        log::warn!("sdr_radio: TX enable failed: {}", e);
                        continue;
                    }
                    in_burst = true;
                }
                if let Some(power_dbm) = burst.power_dbm {
                    log::info!(
                        "sdr_radio: tx {} {} chips at {:.1} dBm, {:.1} ms ahead",
                        burst.label,
                        burst.samples.len(),
                        power_dbm,
                        (burst.tick - now) as f64 / tick_rate * 1e3
                    );
                } else if let Some(estimated_dbm) = burst.estimated_power_dbm {
                    log::info!(
                        "sdr_radio: tx {} {} chips at about {:.1} dBm estimated, {:.1} dB digital backoff, {:.1} ms ahead",
                        burst.label,
                        burst.samples.len(),
                        estimated_dbm,
                        burst.digital_backoff_db,
                        (burst.tick - now) as f64 / tick_rate * 1e3
                    );
                } else {
                    log::info!(
                        "sdr_radio: tx {} {} chips at {:.1} dB digital backoff, {:.1} ms ahead",
                        burst.label,
                        burst.samples.len(),
                        burst.digital_backoff_db,
                        (burst.tick - now) as f64 / tick_rate * 1e3
                    );
                }
                if let Err(e) = tx.transmit_shaped_at(&burst.samples, Some(burst.tick)) {
                    log::warn!("sdr_radio: TX send failed: {}", e);
                }
                shared.tx_bursts.fetch_add(1, Ordering::Relaxed);
                if burst.end_of_burst {
                    end_burst(tx.as_mut(), &mut in_burst);
                }
            }
            TxCommand::Tune { frequency_hz, done } => {
                let result = tx
                    .set_tx_frequency_hz(frequency_hz)
                    .map_err(|e| e.to_string());
                if result.is_ok() {
                    log::debug!("sdr_radio: TX retuned to {:.3} MHz", frequency_hz / 1e6);
                }
                let _ = done.send(result);
            }
            TxCommand::Mute => {
                log_flushed(&mut flushed);
                end_burst(tx.as_mut(), &mut in_burst);
            }
            TxCommand::Stop => break,
        }
    }
    log_flushed(&mut flushed);
    end_burst(tx.as_mut(), &mut in_burst);
    log::info!("sdr_radio: TX thread stopped");
}

fn end_burst(tx: &mut dyn RadioTx, in_burst: &mut bool) {
    if let Err(e) = tx.enable_transmit(false) {
        log::warn!("sdr_radio: TX disable failed: {}", e);
    }
    *in_burst = false;
}

fn log_flushed(flushed: &mut u64) {
    if *flushed > 0 {
        log::info!(
            "sdr_radio: discarded {} bursts queued before a TX mute or stop",
            flushed
        );
        *flushed = 0;
    }
}

impl Radio for SdrRadio {
    fn sample_rate_hz(&self) -> f64 {
        self.sample_rate_hz
    }

    fn rx_reference_dbm(&self) -> Option<f32> {
        self.rx_reference_dbm
    }

    fn tune_forward(&mut self, frequency_hz: f64) -> Result<(), Error> {
        let commands = self
            .rx_commands
            .as_ref()
            .ok_or_else(|| Error::from("radio is not started"))?;
        let (done_tx, done_rx) = mpsc::sync_channel(1);
        commands
            .send(RxCommand::Tune {
                frequency_hz,
                done: done_tx,
            })
            .map_err(|_| Error::from("RX thread is gone"))?;
        done_rx
            .recv()
            .map_err(|_| Error::from("RX thread dropped the tune request"))?
            .map_err(Error::from)?;
        self.forward_hz = frequency_hz;
        Ok(())
    }

    fn tune_reverse(&mut self, frequency_hz: f64) -> Result<(), Error> {
        let commands = self
            .tx_commands
            .as_ref()
            .ok_or_else(|| Error::from("radio is not started"))?;
        let (done_tx, done_rx) = mpsc::sync_channel(1);
        commands
            .send(TxCommand::Tune {
                frequency_hz,
                done: done_tx,
            })
            .map_err(|_| Error::from("TX thread is gone"))?;
        done_rx
            .recv()
            .map_err(|_| Error::from("TX thread dropped the tune request"))?
            .map_err(Error::from)?;
        self.reverse_hz = frequency_hz;
        Ok(())
    }

    fn set_rx_gain(&mut self, gain_db: f64) -> Result<(), Error> {
        let commands = self
            .rx_commands
            .as_ref()
            .ok_or_else(|| Error::from("radio is not started"))?;
        let (done_tx, done_rx) = mpsc::sync_channel(1);
        commands
            .send(RxCommand::SetGain {
                gain_db,
                done: done_tx,
            })
            .map_err(|_| Error::from("RX thread is gone"))?;
        done_rx
            .recv()
            .map_err(|_| Error::from("RX thread dropped the gain request"))?
            .map_err(Error::from)
    }

    fn read_forward(&mut self, out: &mut Vec<Complex32>) {
        self.forward_discontinuity = false;
        let mut queue = self.shared.queue.lock().unwrap();
        if queue.len == 0 {
            drop(queue);
            thread::sleep(Duration::from_millis(1));
            return;
        }
        self.forward_discontinuity = queue.take(CHUNK_SAMPLES, out);
    }

    fn forward_discontinuity(&self) -> bool {
        self.forward_discontinuity
    }

    fn forward_exhausted(&self) -> bool {
        self.rx_thread.is_some() && !self.shared.running.load(Ordering::Relaxed)
    }

    fn rx_overflows(&self) -> u64 {
        self.shared.overflows.load(Ordering::Relaxed)
    }

    fn forward_backlog_samples(&self) -> u64 {
        self.shared.queue.lock().unwrap().len as u64
    }

    fn write_reverse(&mut self, burst: &ReverseBurst) {
        if burst.end_of_burst && burst.samples.is_empty() {
            if let Some(commands) = &self.tx_commands {
                send_tx_priority(&self.shared, commands, TxCommand::Mute);
            }
            self.traffic_scale = None;
            self.traffic_timeline = TrafficTimeline::default();
            return;
        }
        if self.tx_commands.is_none() {
            log::warn!("sdr_radio: not started, dropping a {} burst", burst.label);
            return;
        }
        let anchor = self.shared.queue.lock().unwrap().anchor;
        let Some((anchor_sample, anchor_tick)) = anchor else {
            log::warn!(
                "sdr_radio: no receive timing yet, dropping a {} burst",
                burst.label
            );
            return;
        };
        let position = burst.stream_sample_start as i128
            + self.calibration.tx_delay_samples as i128
            - anchor_sample as i128;
        let mut tick =
            anchor_tick as i128 + (position as f64 * self.ticks_per_sample()).round() as i128;
        log::debug!(
            "sdr_radio: {} burst {} samples past the timing anchor",
            burst.label,
            position
        );
        if tick < 0 {
            log::warn!(
                "sdr_radio: {} burst is before the stream began, dropped",
                burst.label
            );
            return;
        }
        let digital_backoff_db = if self.calibration.relative_access_power
            && !self.calibration.power_control
            && burst.label == "access"
        {
            burst.access_power_offset_db.map_or(0.0, |offset_db| {
                (self.calibration.access_initial_backoff_db - offset_db).max(0.0)
            })
        } else {
            0.0
        };
        let (mut samples, actual_dbm) = if burst.label == "traffic" {
            let scaled =
                self.scale_traffic(&burst.samples, burst.power_dbm, burst.traffic_gain_delta_db);
            if burst.end_of_burst {
                self.traffic_scale = None;
            }
            scaled
        } else {
            self.scale_to_power(&burst.samples, burst.power_dbm, digital_backoff_db)
        };
        if burst.label == "traffic" {
            let ticks_per_sample = self.ticks_per_sample();
            let start_sample = (tick as f64 / ticks_per_sample).round() as u64;
            let prepared =
                self.traffic_timeline
                    .prepare(start_sample, &samples, burst.end_of_burst);
            tick = (prepared.start_sample as f64 * ticks_per_sample).round() as i128;
            samples = prepared.samples;
            if samples.is_empty() {
                if burst.end_of_burst
                    && let Some(commands) = &self.tx_commands
                {
                    let _ = commands.send(TxCommand::Mute);
                }
                return;
            }
        } else {
            self.traffic_timeline = TrafficTimeline::default();
        }
        let estimated_power_dbm = self
            .calibration
            .estimated_full_scale_dbm
            .map(|reference_dbm| {
                let mean_power = samples.iter().map(|sample| sample.norm_sqr()).sum::<f32>()
                    / samples.len().max(1) as f32;
                reference_dbm + 10.0 * mean_power.max(f32::MIN_POSITIVE).log10()
            });
        if self.calibration.power_control
            && burst.label != "traffic"
            && actual_dbm + 0.5 < burst.power_dbm
        {
            log::warn!(
                "sdr_radio: {} burst wanted {:.1} dBm, transmitter tops out at {:.1} dBm",
                burst.label,
                burst.power_dbm,
                actual_dbm
            );
        }
        let frequency_ratio = if self.forward_hz > 0.0 && self.reverse_hz > 0.0 {
            self.reverse_hz / self.forward_hz
        } else {
            1.0
        };
        let correction_hz = burst.forward_carrier_offset_hz * frequency_ratio;
        self.transmit_carrier.correct(
            &mut samples,
            tick as u64,
            self.tick_rate,
            self.sample_rate_hz,
            correction_hz,
        );
        log::debug!(
            "sdr_radio: transmit carrier correction_hz={:.2}",
            correction_hz
        );
        let Some(commands) = self.tx_commands.as_ref() else {
            return;
        };
        if commands
            .send(TxCommand::Burst(TimedBurst {
                samples,
                tick: tick as u64,
                end_of_burst: burst.end_of_burst,
                label: burst.label,
                power_dbm: self.calibration.power_control.then_some(actual_dbm),
                estimated_power_dbm,
                digital_backoff_db,
                generation: self.shared.tx_generation.load(Ordering::Relaxed),
            }))
            .is_err()
        {
            log::warn!(
                "sdr_radio: TX thread is gone, dropping a {} burst",
                burst.label
            );
        }
    }

    fn set_tx_calibration(&mut self, trim: &TxTrim) -> Result<(), Error> {
        if let Some(v) = trim.tx_reference_dbm {
            self.calibration.tx_reference_dbm = v;
        }
        if let Some(v) = trim.tx_delay_samples {
            self.calibration.tx_delay_samples = v;
        }
        if let Some(v) = trim.power_control {
            self.calibration.power_control = v;
        }
        log::info!(
            "sdr_radio: tx reference {:.1} dBm, tx delay {} samples, power control {}",
            self.calibration.tx_reference_dbm,
            self.calibration.tx_delay_samples,
            self.calibration.power_control
        );
        Ok(())
    }

    fn tx_calibration(&self) -> Option<TxCalibration> {
        Some(self.calibration)
    }

    fn start(&mut self) -> Result<(), Error> {
        let rx = self
            .rx
            .take()
            .ok_or_else(|| Error::from("radio already started"))?;
        let tx = self
            .tx
            .take()
            .ok_or_else(|| Error::from("radio already started"))?;
        let (rx_cmd_tx, rx_cmd_rx) = mpsc::channel();
        let (tx_cmd_tx, tx_cmd_rx) = mpsc::channel();
        let shared = self.shared.clone();
        shared.running.store(true, Ordering::Relaxed);
        let (rate, settle) = (self.sample_rate_hz, self.settle);
        let rx_thread = thread::Builder::new()
            .name("cdma-ms-rx".into())
            .spawn(move || rx_thread(rx, shared, rx_cmd_rx, rate, settle))
            .map_err(|e| Error::from(format!("spawn RX thread: {}", e)))?;
        let shared = self.shared.clone();
        let tx_thread = thread::Builder::new()
            .name("cdma-ms-tx".into())
            .spawn(move || tx_thread(tx, shared, tx_cmd_rx))
            .map_err(|e| Error::from(format!("spawn TX thread: {}", e)))?;
        self.rx_commands = Some(rx_cmd_tx);
        self.tx_commands = Some(tx_cmd_tx);
        self.rx_thread = Some(rx_thread);
        self.tx_thread = Some(tx_thread);
        Ok(())
    }

    fn stop(&mut self) {
        self.traffic_scale = None;
        if let Some(commands) = self.tx_commands.take() {
            send_tx_priority(&self.shared, &commands, TxCommand::Stop);
        }
        if let Some(thread) = self.tx_thread.take() {
            let _ = thread.join();
        }
        if let Some(commands) = self.rx_commands.take() {
            let _ = commands.send(RxCommand::Stop);
        }
        if let Some(thread) = self.rx_thread.take() {
            let _ = thread.join();
        }
    }
}

impl Drop for SdrRadio {
    fn drop(&mut self) {
        Radio::stop(self);
        if let Some(tx) = self.tx.as_mut()
            && let Err(e) = tx.enable_transmit(false)
        {
            log::debug!("sdr_radio: TX disable on drop failed: {}", e);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const MOCK_TICK_RATE: u64 = 1_000_000;
    const MOCK_NOW_TICK: u64 = 1_000;
    const MOCK_BURST_TICK: u64 = 1_000_000;
    const TX_THREAD_WAIT: Duration = Duration::from_secs(5);

    #[derive(Debug, Clone, Copy, PartialEq)]
    enum TxEvent {
        Enable(bool),
        Transmit(u64),
    }

    #[derive(Default)]
    struct MockTxState {
        events: Mutex<Vec<TxEvent>>,
        time_unavailable: AtomicBool,
    }

    impl MockTxState {
        fn events(&self) -> Vec<TxEvent> {
            self.events.lock().unwrap().clone()
        }
    }

    struct MockTx {
        state: Arc<MockTxState>,
        transmit_started: Option<SyncSender<()>>,
        transmit_gate: Option<Receiver<()>>,
    }

    impl MockTx {
        fn new(state: Arc<MockTxState>) -> Self {
            MockTx {
                state,
                transmit_started: None,
                transmit_gate: None,
            }
        }
    }

    impl RadioTx for MockTx {
        fn tick_rate(&self) -> u64 {
            MOCK_TICK_RATE
        }

        fn get_hardware_time(&self) -> Result<u64, Error> {
            if self.state.time_unavailable.load(Ordering::Relaxed) {
                return Err("hardware time unavailable".into());
            }
            Ok(MOCK_NOW_TICK)
        }

        fn transmit(&mut self, _samples: &[Complex32]) -> Result<(), Error> {
            Ok(())
        }

        fn transmit_shaped_at(
            &mut self,
            _samples: &[Complex32],
            tick: Option<u64>,
        ) -> Result<(), Error> {
            self.state
                .events
                .lock()
                .unwrap()
                .push(TxEvent::Transmit(tick.unwrap_or_default()));
            if let Some(started) = self.transmit_started.take() {
                let _ = started.send(());
                if let Some(gate) = self.transmit_gate.take() {
                    let _ = gate.recv();
                }
            }
            Ok(())
        }

        fn enable_transmit(&mut self, enable: bool) -> Result<(), Error> {
            self.state
                .events
                .lock()
                .unwrap()
                .push(TxEvent::Enable(enable));
            Ok(())
        }
    }

    struct IdleRx;

    impl RadioRx for IdleRx {
        fn tick_rate(&self) -> u64 {
            MOCK_TICK_RATE
        }

        fn get_hardware_time(&self) -> Result<u64, Error> {
            Ok(MOCK_NOW_TICK)
        }

        fn rx_read(
            &mut self,
            _buf: &mut [Complex32],
            _timeout_us: i64,
        ) -> Result<cdma_common::radio::RxReadResult, Error> {
            thread::sleep(Duration::from_millis(1));
            Ok(cdma_common::radio::RxReadResult {
                samples_read: 0,
                time_ticks: 0,
                overflow: false,
            })
        }

        fn rx_activate(&mut self, _time_ticks: Option<u64>) -> Result<(), Error> {
            Ok(())
        }

        fn rx_deactivate(&mut self) -> Result<(), Error> {
            Ok(())
        }
    }

    fn timed_burst(tick: u64, end_of_burst: bool, generation: u64) -> TxCommand {
        TxCommand::Burst(TimedBurst {
            samples: vec![Complex32::new(0.5, 0.0); 4],
            tick,
            end_of_burst,
            label: "access",
            power_dbm: None,
            estimated_power_dbm: None,
            digital_backoff_db: 0.0,
            generation,
        })
    }

    fn mock_calibration() -> TxCalibration {
        TxCalibration {
            tx_reference_dbm: 0.0,
            estimated_full_scale_dbm: None,
            tx_delay_samples: 0,
            power_control: false,
            peak_limit: 0.8,
            relative_access_power: false,
            access_initial_backoff_db: 0.0,
        }
    }

    fn wait_for_events(state: &MockTxState, count: usize) {
        let deadline = std::time::Instant::now() + TX_THREAD_WAIT;
        while state.events().len() < count {
            assert!(
                std::time::Instant::now() < deadline,
                "TX thread recorded only {:?}",
                state.events()
            );
            thread::yield_now();
        }
    }

    fn spawn_tx(tx: MockTx) -> (Arc<Shared>, Sender<TxCommand>, JoinHandle<()>) {
        let shared = Arc::new(Shared::new());
        let (commands, receiver) = mpsc::channel();
        let thread_shared = shared.clone();
        let handle = thread::spawn(move || tx_thread(Box::new(tx), thread_shared, receiver));
        (shared, commands, handle)
    }

    #[test]
    fn hardware_time_error_on_final_burst_still_disables_tx() {
        let state = Arc::new(MockTxState::default());
        let (shared, commands, handle) = spawn_tx(MockTx::new(state.clone()));
        commands
            .send(timed_burst(MOCK_BURST_TICK, false, 0))
            .unwrap();
        wait_for_events(&state, 2);
        state.time_unavailable.store(true, Ordering::Relaxed);
        commands
            .send(timed_burst(MOCK_BURST_TICK + 4, true, 0))
            .unwrap();
        wait_for_events(&state, 3);
        assert_eq!(
            state.events(),
            [
                TxEvent::Enable(true),
                TxEvent::Transmit(MOCK_BURST_TICK),
                TxEvent::Enable(false)
            ]
        );
        send_tx_priority(&shared, &commands, TxCommand::Stop);
        handle.join().unwrap();
    }

    #[test]
    fn mute_discards_queued_bursts_and_disables_tx() {
        let state = Arc::new(MockTxState::default());
        let (started_tx, started_rx) = mpsc::sync_channel(1);
        let (gate_tx, gate_rx) = mpsc::sync_channel(1);
        let mut tx = MockTx::new(state.clone());
        tx.transmit_started = Some(started_tx);
        tx.transmit_gate = Some(gate_rx);
        let (shared, commands, handle) = spawn_tx(tx);
        for chunk in 0..4 {
            commands
                .send(timed_burst(MOCK_BURST_TICK + chunk * 4, chunk == 3, 0))
                .unwrap();
        }
        started_rx.recv_timeout(TX_THREAD_WAIT).unwrap();
        send_tx_priority(&shared, &commands, TxCommand::Mute);
        gate_tx.send(()).unwrap();
        send_tx_priority(&shared, &commands, TxCommand::Stop);
        handle.join().unwrap();
        let events = state.events();
        assert_eq!(
            events[..3],
            [
                TxEvent::Enable(true),
                TxEvent::Transmit(MOCK_BURST_TICK),
                TxEvent::Enable(false)
            ]
        );
        assert!(
            !events.contains(&TxEvent::Transmit(MOCK_BURST_TICK + 4)),
            "a burst queued before the mute was transmitted: {events:?}"
        );
    }

    #[test]
    fn stop_disables_tx_without_draining_queued_bursts() {
        let state = Arc::new(MockTxState::default());
        let (started_tx, started_rx) = mpsc::sync_channel(1);
        let (gate_tx, gate_rx) = mpsc::sync_channel(1);
        let mut tx = MockTx::new(state.clone());
        tx.transmit_started = Some(started_tx);
        tx.transmit_gate = Some(gate_rx);
        let (shared, commands, handle) = spawn_tx(tx);
        for chunk in 0..4 {
            commands
                .send(timed_burst(MOCK_BURST_TICK + chunk * 4, false, 0))
                .unwrap();
        }
        started_rx.recv_timeout(TX_THREAD_WAIT).unwrap();
        send_tx_priority(&shared, &commands, TxCommand::Stop);
        gate_tx.send(()).unwrap();
        handle.join().unwrap();
        assert_eq!(
            state.events(),
            [
                TxEvent::Enable(true),
                TxEvent::Transmit(MOCK_BURST_TICK),
                TxEvent::Enable(false)
            ]
        );
    }

    #[test]
    fn dropping_the_radio_disables_tx() {
        let state = Arc::new(MockTxState::default());
        let mut radio = SdrRadio::from_parts(
            Box::new(MockTx::new(state.clone())),
            Box::new(IdleRx),
            MOCK_TICK_RATE,
            MOCK_TICK_RATE as f64,
            mock_calibration(),
        );
        radio.start().unwrap();
        drop(radio);
        assert_eq!(state.events().last(), Some(&TxEvent::Enable(false)));

        let unstarted = Arc::new(MockTxState::default());
        drop(SdrRadio::from_parts(
            Box::new(MockTx::new(unstarted.clone())),
            Box::new(IdleRx),
            MOCK_TICK_RATE,
            MOCK_TICK_RATE as f64,
            mock_calibration(),
        ));
        assert_eq!(unstarted.events(), [TxEvent::Enable(false)]);
    }

    #[test]
    fn transmit_carrier_keeps_phase_across_gaps_and_frequency_updates() {
        let mut carrier = TransmitCarrier::default();
        let mut first = [Complex32::new(1.0, 0.0); 4];
        carrier.correct(&mut first, 0, 1_000_000, 1_000.0, 125.0);
        let mut next = [Complex32::new(1.0, 0.0); 2];
        carrier.correct(&mut next, 6_000, 1_000_000, 1_000.0, 250.0);
        assert!((first[2] - Complex32::new(0.0, 1.0)).norm() < 1e-6);
        assert!((next[0] - Complex32::new(0.0, -1.0)).norm() < 1e-6);
        assert!((next[1] - Complex32::new(1.0, 0.0)).norm() < 1e-6);
    }

    #[test]
    fn traffic_timing_slew_preserves_samples_on_aligned_wire_packets() {
        let mut timeline = TrafficTimeline::default();
        let samples: Vec<_> = (0..20).map(|v| Complex32::new(v as f32, 0.0)).collect();
        let first = timeline.prepare(1_000, &samples[..8], false);
        let second = timeline.prepare(1_007, &samples[7..15], false);
        let third = timeline.prepare(1_015, &samples[15..], true);
        assert_eq!(
            [first.start_sample, second.start_sample, third.start_sample],
            [1_000, 1_008, 1_012]
        );
        let mut recovered = Vec::new();
        for packet in [first, second, third] {
            assert_eq!(packet.samples.len() % TRAFFIC_SAMPLE_ALIGNMENT, 0);
            recovered.extend(packet.samples);
        }
        assert_eq!(recovered, samples);
        assert!(timeline.next_sample.is_none());
        assert!(timeline.pending.is_empty());
    }

    #[test]
    fn traffic_timing_gap_is_silence_without_shifting_later_samples() {
        let mut timeline = TrafficTimeline::default();
        let value = Complex32::new(1.0, 0.0);
        let first = timeline.prepare(100, &[value; 4], false);
        let second = timeline.prepare(105, &[value; 4], true);
        assert_eq!(first.start_sample, 100);
        assert_eq!(second.start_sample, 104);
        assert_eq!(
            second.samples,
            [
                Complex32::default(),
                value,
                value,
                value,
                value,
                Complex32::default(),
                Complex32::default(),
                Complex32::default()
            ]
        );
    }

    #[test]
    fn traffic_power_decreases_immediately_after_peak_saturation() {
        let mut state = TrafficScale {
            scale: 1.0,
            minimum_scale: 10f32.powf(MIN_TRAFFIC_RELATIVE_GAIN_DB / 20.0),
            requested_dbm: 0.0,
        };
        assert_eq!(state.apply(0.0, 30.0, 2.0), 2.0);
        let reduced = state.apply(0.0, -1.0, 2.0);
        assert!((20.0 * (reduced / 2.0).log10() + 1.0).abs() < 1e-5);
        assert_eq!(state.apply(0.0, 0.0, 2.0), reduced);
        for _ in 0..100 {
            state.apply(0.0, -1.0, 2.0);
        }
        assert_eq!(state.scale, state.minimum_scale);
        let floor = state.scale;
        let raised = state.apply(0.0, 1.0, 2.0);
        assert!((20.0 * (raised / floor).log10() - 1.0).abs() < 1e-5);
    }

    #[test]
    fn queue_reports_discontinuity_at_the_first_sample_after_a_gap() {
        let mut q = Queue::default();
        for discontinuity in [false, true, false] {
            q.push(Segment {
                tick: None,
                samples: vec![Complex32::new(1.0, 0.0); 10],
                discontinuity,
            });
        }
        let mut out = Vec::new();
        assert!(!q.take(30, &mut out));
        assert_eq!(out.len(), 10);
        out.clear();
        assert!(q.take(5, &mut out));
        assert_eq!(out.len(), 5);
        out.clear();
        assert!(!q.take(30, &mut out));
        assert_eq!(out.len(), 15);
        assert_eq!(q.delivered, 30);
    }

    #[test]
    fn queue_anchors_positions_to_ticks_across_gaps_and_drops() {
        let mut q = Queue::default();
        q.push(Segment {
            tick: Some(1_000),
            samples: vec![Complex32::new(1.0, 0.0); 10],
            discontinuity: false,
        });
        q.push(Segment {
            tick: None,
            samples: vec![Complex32::new(2.0, 0.0); 10],
            discontinuity: false,
        });
        q.push(Segment {
            tick: Some(5_000),
            samples: vec![Complex32::new(3.0, 0.0); 10],
            discontinuity: false,
        });
        let mut out = Vec::new();
        q.take(15, &mut out);
        assert_eq!(out.len(), 15);
        assert_eq!(q.anchor, Some((0, 1_000)));
        q.take(10, &mut out);
        assert_eq!(q.anchor, Some((20, 5_000)));
        assert_eq!(q.delivered, 25);
        assert_eq!(q.len, 5);
        q.push(Segment {
            tick: Some(9_000),
            samples: vec![Complex32::new(4.0, 0.0); 10],
            discontinuity: false,
        });
        let dropped = q.drop_oldest(10);
        assert_eq!(dropped, 5);
        assert_eq!(q.len, 10);
        out.clear();
        assert!(q.take(1, &mut out));
        assert_eq!(q.anchor, Some((25, 9_000)));
    }
}
