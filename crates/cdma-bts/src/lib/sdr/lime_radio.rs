use std::cell::UnsafeCell;
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use cdma_common::error::Error;
use limesuite::{Device, RxStream, StreamMeta, TxStream};
use log::{debug, info};
use num_complex::Complex32;

use super::{Radio, RadioRx, RadioTx, RxReadResult, TxRadioHealth};

/// Default stream FIFO size in samples.
const DEFAULT_FIFO_SIZE: u32 = 1024 * 1024;

/// Default throughput vs latency tradeoff (0.0 = min latency, 1.0 = max throughput).
const DEFAULT_THROUGHPUT_VS_LATENCY: f32 = 0.0;

/// Resolve an antenna name to its LimeSuite index by querying the device.
/// Falls back to well-known LimeSDR Mini mappings if the device API is
/// unavailable.
fn resolve_antenna_index(device: &Device, dir_tx: bool, chan: usize, name: &str) -> usize {
    if let Ok(list) = device.antenna_list(dir_tx, chan) {
        for (i, entry) in list.iter().enumerate() {
            debug!(
                "Lime: antenna[{}] dir_tx={} chan={}: {}",
                i, dir_tx, chan, entry
            );
            if entry.eq_ignore_ascii_case(name) {
                return i;
            }
        }
    }

    // Fallback: well-known LimeSDR Mini antenna indices.
    if dir_tx {
        match name.to_ascii_uppercase().as_str() {
            "BAND1" => 0,
            "BAND2" => 1,
            _ => 0,
        }
    } else {
        match name.to_ascii_uppercase().as_str() {
            "LNAH" => 0,
            "LNAL" => 1,
            "LNAW" => 2,
            _ => 2,
        }
    }
}

pub struct LimeRadio {
    device: Arc<Device>,
    channel: usize,
    sample_rate: u64,
    oversample: usize,
    tx_lo_offset_hz: f64,
    tx_sample_rate_hz: f64,
    tx_nco_phase_rad: f64,
    tx_stream: Option<TxStream>,
    rx_stream: Option<RxStream>,
    tx_fifo_size: u32,
    rx_fifo_size: u32,
    throughput_vs_latency: f32,
}

impl LimeRadio {
    /// Open and initialize a LimeSDR device.
    ///
    /// `device_str` is passed to `LMS_Open`; use `""` for the first available
    /// device.  `channel` is typically 0.  `tx_antenna` is a name like
    /// "BAND1".  `sample_rate_hz` sets both TX and RX (LimeSDR Mini shares
    /// the sample rate).  `oversample` is passed to `LMS_SetSampleRate`
    /// (0 = auto).
    pub fn new(
        device_str: &str,
        channel: usize,
        tx_antenna: &str,
        tx_gain_db: u32,
        sample_rate_hz: usize,
        oversample: usize,
    ) -> Result<LimeRadio, Error> {
        Self::with_stream_config(
            device_str,
            channel,
            tx_antenna,
            tx_gain_db,
            sample_rate_hz,
            oversample,
            None,
            None,
            None,
        )
    }

    /// Open with explicit stream configuration.
    pub fn with_stream_config(
        device_str: &str,
        channel: usize,
        tx_antenna: &str,
        tx_gain_db: u32,
        sample_rate_hz: usize,
        oversample: usize,
        tx_fifo_size: Option<u32>,
        rx_fifo_size: Option<u32>,
        throughput_vs_latency: Option<f32>,
    ) -> Result<LimeRadio, Error> {
        let info = if device_str.is_empty() {
            None
        } else {
            Some(device_str)
        };
        let device = Arc::new(
            Device::open(info)
                .map_err(|e| Error::from(format!("Lime: failed to open device: {}", e)))?,
        );
        info!("Lime: device opened");

        device
            .init()
            .map_err(|e| Error::from(format!("Lime: init: {}", e)))?;

        // Enable TX channel.
        device
            .enable_channel(true, channel, true)
            .map_err(|e| Error::from(format!("Lime: enable TX channel: {}", e)))?;

        // Set shared sample rate (applies to both TX and RX on LimeSDR Mini).
        device
            .set_sample_rate(sample_rate_hz as f64, oversample)
            .map_err(|e| Error::from(format!("Lime: set sample rate: {}", e)))?;
        let actual_rate = device
            .get_sample_rate(true, channel)
            .map_err(|e| Error::from(format!("Lime: get sample rate: {}", e)))?;
        info!(
            "Lime: sample_rate requested={} actual={} oversample={}",
            sample_rate_hz, actual_rate, oversample
        );

        // Set TX antenna.
        let tx_ant_idx = resolve_antenna_index(&device, true, channel, tx_antenna);
        device
            .set_antenna(true, channel, tx_ant_idx)
            .map_err(|e| Error::from(format!("Lime: set TX antenna: {}", e)))?;
        info!("Lime: TX antenna='{}' (index {})", tx_antenna, tx_ant_idx);

        // Set TX gain.
        device
            .set_gain_db(true, channel, tx_gain_db)
            .map_err(|e| Error::from(format!("Lime: set TX gain: {}", e)))?;
        info!("Lime: TX gain={}dB", tx_gain_db);

        // Calibrate TX.
        device
            .calibrate(true, channel, sample_rate_hz as f64)
            .map_err(|e| Error::from(format!("Lime: calibrate TX: {}", e)))?;

        // Create TX stream (started later in split() after all config is done,
        // since LMS_Calibrate/LMS_SetupStream on RX can reset active streams).
        let tx_fifo = tx_fifo_size.unwrap_or(DEFAULT_FIFO_SIZE);
        let tvl = throughput_vs_latency.unwrap_or(DEFAULT_THROUGHPUT_VS_LATENCY);
        let tx_stream = TxStream::with_throughput(device.clone(), channel as u32, tx_fifo, tvl)
            .map_err(|e| Error::from(format!("Lime: create TX stream: {}", e)))?;
        info!(
            "Lime: TX stream FIFO size={} samples throughput_vs_latency={:.2}",
            tx_fifo, tvl
        );

        Ok(LimeRadio {
            device,
            channel,
            sample_rate: sample_rate_hz as u64,
            oversample,
            tx_lo_offset_hz: 0.0,
            tx_sample_rate_hz: sample_rate_hz as f64,
            tx_nco_phase_rad: 0.0,
            tx_stream: Some(tx_stream),
            rx_stream: None,
            tx_fifo_size: tx_fifo,
            rx_fifo_size: rx_fifo_size.unwrap_or(DEFAULT_FIFO_SIZE),
            throughput_vs_latency: tvl,
        })
    }

    /// Set the software TX LO offset in Hz. A non-zero offset causes the TX
    /// frequency to be shifted by `offset_hz` at the RF, and a software NCO
    /// rotates the baseband in the opposite direction to compensate.
    pub fn set_tx_lo_offset(&mut self, offset_hz: i64) -> Result<(), Error> {
        self.tx_lo_offset_hz = offset_hz as f64;
        self.tx_nco_phase_rad = 0.0;
        Ok(())
    }

    fn apply_tx_lo_offset(
        tx_lo_offset_hz: f64,
        tx_sample_rate_hz: f64,
        tx_nco_phase_rad: &mut f64,
        samples: &mut [Complex32],
    ) {
        if tx_lo_offset_hz == 0.0 || tx_sample_rate_hz <= 0.0 {
            return;
        }

        let phase_step = -2.0 * std::f64::consts::PI * tx_lo_offset_hz / tx_sample_rate_hz;
        let mut phase = *tx_nco_phase_rad;

        for sample in samples.iter_mut() {
            let rot = Complex32::new(phase.cos() as f32, phase.sin() as f32);
            *sample *= rot;
            phase += phase_step;
        }

        *tx_nco_phase_rad = phase.rem_euclid(2.0 * std::f64::consts::PI);
    }
}

impl Radio for LimeRadio {
    fn tick_rate(&self) -> u64 {
        // LimeSDR timestamps are in sample counts, so tick rate = sample rate.
        self.sample_rate
    }

    fn set_tx_frequency(&mut self, center_frequency: usize) -> Result<(), Error> {
        // If we have a software LO offset, shift the RF frequency and let the
        // NCO compensate in baseband (same as SoapySdrRadio).
        let rf_freq = center_frequency as f64 + self.tx_lo_offset_hz;
        self.device
            .set_lo_frequency(true, self.channel, rf_freq)
            .map_err(|e| Error::from(format!("Lime: set TX freq: {}", e)))?;
        // Re-calibrate after frequency change.
        self.device
            .calibrate(true, self.channel, self.sample_rate as f64)
            .map_err(|e| Error::from(format!("Lime: calibrate TX after freq change: {}", e)))?;
        debug!(
            "Lime: TX freq set to {} (RF={}, LO offset={})",
            center_frequency, rf_freq, self.tx_lo_offset_hz
        );
        Ok(())
    }

    fn set_tx_sample_rate(&mut self, sample_rate: usize) -> Result<(), Error> {
        // LimeSDR Mini shares sample rate between TX and RX, so this updates
        // both.  The caller should be aware of this constraint.
        self.device
            .set_sample_rate(sample_rate as f64, self.oversample)
            .map_err(|e| Error::from(format!("Lime: set sample rate: {}", e)))?;
        self.sample_rate = sample_rate as u64;
        self.tx_sample_rate_hz = sample_rate as f64;
        self.tx_nco_phase_rad = 0.0;
        Ok(())
    }

    fn set_tx_bandwidth(&mut self, bandwidth: usize) -> Result<(), Error> {
        self.device
            .set_lpf_bw(true, self.channel, bandwidth as f64)
            .map_err(|e| Error::from(format!("Lime: set TX LPF BW: {}", e)))?;
        Ok(())
    }

    fn set_tx_lo_offset_hz(&mut self, offset_hz: i64) -> Result<(), Error> {
        self.set_tx_lo_offset(offset_hz)
    }

    fn setup_rx(
        &mut self,
        channel: usize,
        antenna: &str,
        frequency_hz: f64,
        sample_rate_hz: f64,
        bandwidth_hz: f64,
        gain_db: Option<f64>,
    ) -> Result<(), Error> {
        // Enable RX channel.
        self.device
            .enable_channel(false, channel, true)
            .map_err(|e| Error::from(format!("Lime: enable RX channel: {}", e)))?;

        // The sample rate is shared on LimeSDR Mini and was already set in
        // new(). If the caller requests a different rate, update it.
        if (sample_rate_hz - self.sample_rate as f64).abs() > 1.0 {
            self.device
                .set_sample_rate(sample_rate_hz, self.oversample)
                .map_err(|e| Error::from(format!("Lime: set RX sample rate: {}", e)))?;
            self.sample_rate = sample_rate_hz as u64;
        }

        // Set RX antenna.
        let rx_ant_idx = resolve_antenna_index(&self.device, false, channel, antenna);
        self.device
            .set_antenna(false, channel, rx_ant_idx)
            .map_err(|e| Error::from(format!("Lime: set RX antenna: {}", e)))?;

        // Set RX LO frequency.
        self.device
            .set_lo_frequency(false, channel, frequency_hz)
            .map_err(|e| Error::from(format!("Lime: set RX freq: {}", e)))?;

        // Set RX LPF bandwidth.
        self.device
            .set_lpf_bw(false, channel, bandwidth_hz)
            .map_err(|e| Error::from(format!("Lime: set RX LPF BW: {}", e)))?;

        // Set RX gain.
        if let Some(gain) = gain_db {
            self.device
                .set_gain_db(false, channel, gain as u32)
                .map_err(|e| Error::from(format!("Lime: set RX gain: {}", e)))?;
        }

        // Calibrate RX.
        self.device
            .calibrate(false, channel, bandwidth_hz)
            .map_err(|e| Error::from(format!("Lime: calibrate RX: {}", e)))?;

        // Create RX stream.
        let rx_fifo = self.rx_fifo_size;
        let rx_stream = RxStream::with_throughput(
            self.device.clone(),
            channel as u32,
            rx_fifo,
            self.throughput_vs_latency,
        )
        .map_err(|e| Error::from(format!("Lime: create RX stream: {}", e)))?;
        self.rx_stream = Some(rx_stream);
        info!(
            "Lime: RX stream FIFO size={} samples throughput_vs_latency={:.2}",
            rx_fifo, self.throughput_vs_latency
        );

        let actual_freq = self.device.get_lo_frequency(false, channel).unwrap_or(0.0);
        let actual_rate = self.device.get_sample_rate(false, channel).unwrap_or(0.0);
        info!(
            "Lime: RX configured antenna='{}' (index {}) freq={} rate={} bw={} gain={:?}",
            antenna, rx_ant_idx, actual_freq, actual_rate, bandwidth_hz, gain_db
        );
        Ok(())
    }

    fn split(mut self: Box<Self>) -> Result<(Box<dyn RadioTx>, Option<Box<dyn RadioRx>>), Error> {
        // Start TX stream now — after all configuration (set_tx_frequency,
        // setup_rx, calibrate) is done. Starting earlier risks LimeSuite
        // resetting the stream during RX setup/calibration.
        let mut tx_stream = self
            .tx_stream
            .take()
            .ok_or_else(|| Error::from("Lime: TX stream not initialized"))?;
        tx_stream
            .start()
            .map_err(|e| Error::from(format!("Lime: start TX stream: {}", e)))?;
        info!("Lime: TX stream started (FPGA timestamp counter active)");

        // Shared hardware clock: the RX half updates this on every rx_read
        // and get_hardware_time call. The TX half reads it. LimeSuite only
        // reports a running FPGA timestamp on RX stream status; the TX
        // stream status only reflects the last submitted TX timestamp.
        let shared_clock = Arc::new(AtomicU64::new(0));

        let device = self.device.clone();
        let tx = LimeTxHalf {
            _device: device.clone(),
            tx_stream: UnsafeCell::new(tx_stream),
            sample_rate: self.sample_rate,
            tx_lo_offset_hz: self.tx_lo_offset_hz,
            tx_sample_rate_hz: self.tx_sample_rate_hz,
            tx_nco_phase_rad: self.tx_nco_phase_rad,
            start_of_burst: true,
            shared_clock: shared_clock.clone(),
            last_underrun: 0,
            last_dropped_packets: 0,
            tx_scratch: Vec::with_capacity(self.tx_fifo_size as usize),
            health: TxRadioHealth::default(),
        };
        let rx = self.rx_stream.take().map(|s| -> Box<dyn RadioRx> {
            Box::new(LimeRxHalf {
                _device: device.clone(),
                rx_stream: UnsafeCell::new(s),
                sample_rate: self.sample_rate,
                shared_clock: shared_clock.clone(),
                rx_read_count: 0,
                last_overrun: 0,
            })
        });
        Ok((Box::new(tx), rx))
    }
}

// ---------------------------------------------------------------------------
// TX half
// ---------------------------------------------------------------------------

struct LimeTxHalf {
    _device: Arc<Device>,
    tx_stream: UnsafeCell<TxStream>,
    sample_rate: u64,
    tx_lo_offset_hz: f64,
    tx_sample_rate_hz: f64,
    tx_nco_phase_rad: f64,
    start_of_burst: bool,
    /// Shared clock updated by the RX half from FPGA timestamps.
    shared_clock: Arc<AtomicU64>,
    /// Last reported underrun count (to detect new underruns).
    last_underrun: u32,
    last_dropped_packets: u32,
    tx_scratch: Vec<Complex32>,
    health: TxRadioHealth,
}

// Safety: LimeTxHalf is only accessed from the TX thread.
unsafe impl Send for LimeTxHalf {}

impl LimeTxHalf {
    fn tx_stream_mut(&self) -> &mut TxStream {
        // Safety: LimeTxHalf is only ever accessed from a single thread
        // (the TX thread owns it exclusively after split()).
        unsafe { &mut *self.tx_stream.get() }
    }
}

impl RadioTx for LimeTxHalf {
    fn tick_rate(&self) -> u64 {
        self.sample_rate
    }

    fn get_hardware_time(&self) -> Result<u64, Error> {
        Ok(self.shared_clock.load(Ordering::Relaxed))
    }

    fn set_hardware_time(&self, _ticks: u64) -> Result<(), Error> {
        // LimeSDR does not support setting the hardware time externally.
        // The timestamp counter resets when streams are started.
        Ok(())
    }

    fn transmit(&mut self, samples: &[Complex32]) -> Result<(), Error> {
        self.transmit_at(samples, None)
    }

    fn prepare_transmit(&mut self, max_samples: usize) -> Result<(), Error> {
        self.tx_scratch.resize(max_samples, Complex32::default());
        self.tx_scratch.clear();
        Ok(())
    }

    fn transmit_at(&mut self, samples: &[Complex32], tick: Option<u64>) -> Result<(), Error> {
        self.tx_scratch.clear();
        self.tx_scratch.extend_from_slice(samples);
        LimeRadio::apply_tx_lo_offset(
            self.tx_lo_offset_hz,
            self.tx_sample_rate_hz,
            &mut self.tx_nco_phase_rad,
            &mut self.tx_scratch,
        );
        let meta = StreamMeta {
            timestamp: tick.unwrap_or(0),
            wait_for_timestamp: tick.is_some(),
            flush_partial_packet: false,
        };
        self.tx_stream_mut()
            .send(&self.tx_scratch, &meta, 1000)
            .map_err(|e| Error::from(format!("Lime: TX send: {}", e)))?;
        self.start_of_burst = false;
        Ok(())
    }

    fn enable_transmit(&mut self, enable: bool) -> Result<(), Error> {
        self.enable_transmit_at(enable, None)
    }

    fn enable_transmit_at(&mut self, enable: bool, _tick: Option<u64>) -> Result<(), Error> {
        if enable {
            self.tx_nco_phase_rad = 0.0;
            self.start_of_burst = true;
            // TX stream is already started from LimeRadio::new().
        } else {
            // Flush any remaining partial packet before stopping.
            let flush_meta = StreamMeta {
                timestamp: 0,
                wait_for_timestamp: false,
                flush_partial_packet: true,
            };
            let empty: &[Complex32] = &[];
            let _ = self.tx_stream_mut().send(empty, &flush_meta, 100);
            self.tx_stream_mut()
                .stop()
                .map_err(|e| Error::from(format!("Lime: TX stream stop: {}", e)))?;
            self.start_of_burst = false;
        }
        Ok(())
    }

    fn tx_health(&mut self) -> Result<TxRadioHealth, Error> {
        let status = self
            .tx_stream_mut()
            .status()
            .map_err(|e| Error::from(format!("Lime: TX stream status: {e}")))?;
        let new_underruns = status.underrun.saturating_sub(self.last_underrun);
        let new_dropped = status
            .dropped_packets
            .saturating_sub(self.last_dropped_packets);
        self.health.underflows += u64::from(new_underruns);
        self.health.dropped_packets += u64::from(new_dropped);
        if new_underruns > 0 || new_dropped > 0 {
            log::warn!(
                "Lime TX stream: underrun={} (+{}) dropped={} (+{}) fifo={}/{}",
                status.underrun,
                new_underruns,
                status.dropped_packets,
                new_dropped,
                status.fifo_filled,
                status.fifo_size,
            );
        }
        self.last_underrun = status.underrun;
        self.last_dropped_packets = status.dropped_packets;
        Ok(self.health)
    }
}

// ---------------------------------------------------------------------------
// RX half
// ---------------------------------------------------------------------------

struct LimeRxHalf {
    _device: Arc<Device>,
    rx_stream: UnsafeCell<RxStream>,
    sample_rate: u64,
    /// Shared clock: updated from RX FPGA timestamps so the TX half can read it.
    shared_clock: Arc<AtomicU64>,
    /// Counter for periodic stream status logging.
    rx_read_count: u64,
    /// Last reported overrun count.
    last_overrun: u32,
}

// Safety: LimeRxHalf is only accessed from the RX thread.
unsafe impl Send for LimeRxHalf {}

impl LimeRxHalf {
    fn rx_stream_mut(&self) -> &mut RxStream {
        // Safety: LimeRxHalf is only ever accessed from a single thread
        // (the RX thread owns it exclusively after split()).
        unsafe { &mut *self.rx_stream.get() }
    }
}

impl RadioRx for LimeRxHalf {
    fn tick_rate(&self) -> u64 {
        self.sample_rate
    }

    fn get_hardware_time(&self) -> Result<u64, Error> {
        let st = self
            .rx_stream_mut()
            .status()
            .map_err(|e| Error::from(format!("Lime: RX stream status: {}", e)))?;
        self.shared_clock.store(st.timestamp, Ordering::Relaxed);
        Ok(st.timestamp)
    }

    fn rx_read(&mut self, buf: &mut [Complex32], timeout_us: i64) -> Result<RxReadResult, Error> {
        let timeout_ms = (timeout_us / 1000).max(1) as u32;
        let mut meta = StreamMeta::default();
        match self.rx_stream_mut().recv(buf, &mut meta, timeout_ms) {
            Ok(n) => {
                // meta.timestamp is the time of the first sample in the buffer.
                // Update shared clock to the end of the buffer (latest HW time).
                let end_timestamp = meta.timestamp + n as u64;
                self.shared_clock.store(end_timestamp, Ordering::Relaxed);
                self.rx_read_count += 1;

                // Periodic RX stream health check (~every 1 second).
                if self.rx_read_count % 400 == 0 {
                    if let Ok(st) = self.rx_stream_mut().status() {
                        let new_overruns = st.overrun.saturating_sub(self.last_overrun);
                        if new_overruns > 0 || st.dropped_packets > 0 {
                            log::warn!(
                                "Lime RX stream: overrun={} (+{}) dropped={} fifo={}/{}",
                                st.overrun,
                                new_overruns,
                                st.dropped_packets,
                                st.fifo_filled,
                                st.fifo_size,
                            );
                        }
                        self.last_overrun = st.overrun;
                    }
                }

                Ok(RxReadResult {
                    samples_read: n,
                    time_ticks: meta.timestamp,
                    overflow: false,
                })
            }
            Err(e) => {
                // Check stream status for overflow.
                if let Ok(st) = self.rx_stream_mut().status() {
                    if st.overrun > 0 {
                        return Ok(RxReadResult {
                            samples_read: 0,
                            time_ticks: 0,
                            overflow: true,
                        });
                    }
                }
                Err(Error::from(format!("Lime: RX recv: {}", e)))
            }
        }
    }

    fn rx_activate(&mut self, _time_ticks: Option<u64>) -> Result<(), Error> {
        self.rx_stream_mut()
            .start()
            .map_err(|e| Error::from(format!("Lime: RX stream start: {}", e)))?;
        Ok(())
    }

    fn rx_deactivate(&mut self) -> Result<(), Error> {
        self.rx_stream_mut()
            .stop()
            .map_err(|e| Error::from(format!("Lime: RX stream stop: {}", e)))?;
        Ok(())
    }
}
