use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};

use bladerf::device::{rx_channel, tx_channel};
use bladerf::stream::{Sc16Q11, StreamMeta};
use bladerf::{Device, RxSync, TxSync};
use cdma_common::error::Error;
use log::{debug, info, warn};
use num_complex::Complex32;

use super::{Radio, RadioRx, RadioTx, RxReadResult, TX_SAMPLE_RATE, TxPulseShaper};

/// BLADERF_FORMAT_SC16_Q11_META — enables hardware timestamps in stream metadata.
const FORMAT_SC16_Q11_META: u32 = 2;

/// BLADERF_META_FLAG_TX_BURST_START — marks the beginning of a TX burst.
const META_FLAG_TX_BURST_START: u32 = 1;

/// BLADERF_META_FLAG_TX_BURST_END — marks the end of a TX burst.
const META_FLAG_TX_BURST_END: u32 = 2;

/// BLADERF_META_FLAG_TX_NOW — transmit immediately, ignore metadata timestamp.
const META_FLAG_TX_NOW: u32 = 4;

/// BLADERF_META_FLAG_TX_UPDATE_TIMESTAMP — use the metadata timestamp field for scheduled TX.
const META_FLAG_TX_UPDATE_TIMESTAMP: u32 = 8;

/// BLADERF_META_FLAG_RX_NOW — return samples immediately, ignore timestamp.
const META_FLAG_RX_NOW: u32 = 0x8000_0000;

/// BLADERF_META_STATUS_OVERRUN — RX overrun occurred.
const META_STATUS_OVERRUN: u32 = 1;

/// BLADERF_META_STATUS_UNDERRUN — TX underrun occurred.
const META_STATUS_UNDERRUN: u32 = 2;

/// Default number of stream buffers.
const DEFAULT_NUM_BUFFERS: u32 = 16;

/// Default buffer size in samples.
const DEFAULT_BUFFER_SIZE: u32 = 8192;

/// Default number of USB transfers.
const DEFAULT_NUM_TRANSFERS: u32 = 8;

/// Default stream timeout in ms.
const DEFAULT_STREAM_TIMEOUT_MS: u32 = 3500;

pub struct BladeRfRadio {
    device: Device,
    channel: u32,
    sample_rate: u32,
    tx_shaper: TxPulseShaper,
    tx_lo_offset_hz: f64,
    tx_sample_rate_hz: f64,
    tx_nco_phase_rad: f64,
    rx_configured: bool,
    num_buffers: u32,
    buffer_size: u32,
    num_transfers: u32,
    stream_timeout_ms: u32,
}

impl BladeRfRadio {
    /// Open with explicit stream buffer configuration.
    pub fn with_stream_config(
        device_id: &str,
        channel: u32,
        tx_gain_db: i32,
        sample_rate_hz: u32,
        fpga_path: Option<&str>,
        tx_antenna: Option<&str>,
        num_buffers: Option<u32>,
        buffer_size: Option<u32>,
        num_transfers: Option<u32>,
        stream_timeout_ms: Option<u32>,
    ) -> Result<Self, Error> {
        let id = if device_id.is_empty() {
            None
        } else {
            Some(device_id)
        };
        let device = Device::open(id).map_err(|e| Error::from(format!("bladeRF: open: {}", e)))?;

        let board = device.board_name();
        info!("bladeRF: opened board={}", board);

        if let Ok(serial) = device.serial() {
            info!("bladeRF: serial={}", serial);
        }

        // Load FPGA if not already configured.
        let fpga_loaded = device
            .is_fpga_configured()
            .map_err(|e| Error::from(format!("bladeRF: check FPGA: {}", e)))?;
        if !fpga_loaded {
            match fpga_path {
                Some(path) => {
                    info!("bladeRF: FPGA not loaded, loading from {}", path);
                    device
                        .load_fpga(path)
                        .map_err(|e| Error::from(format!("bladeRF: load FPGA: {}", e)))?;
                    info!("bladeRF: FPGA loaded successfully");
                }
                None => {
                    return Err(Error::from(
                        "bladeRF: FPGA not configured and no fpga_path specified. \
                         Place the correct .rbf bitstream (e.g. hostedxA4.rbf for Micro 2.0) \
                         in ~/.config/Nuand/bladeRF/ for automatic loading, flash it to SPI \
                         with `bladeRF-cli -L <file>.rbf`, or set fpga_path in the radio config.",
                    ));
                }
            }
        } else {
            info!("bladeRF: FPGA already configured");
        }

        let tx_ch = tx_channel(channel);

        // Log available TX RF ports.
        if let Ok(ports) = device.get_rf_ports(tx_ch) {
            info!("bladeRF: available TX RF ports: {:?}", ports);
        }

        // Set TX RF port/antenna if specified.
        if let Some(antenna) = tx_antenna {
            device
                .set_rf_port(tx_ch, antenna)
                .map_err(|e| Error::from(format!("bladeRF: set TX RF port: {}", e)))?;
            info!("bladeRF: TX RF port set to '{}'", antenna);
        }
        if let Ok(port) = device.get_rf_port(tx_ch) {
            info!("bladeRF: TX RF port active: '{}'", port);
        }

        let actual_rate = device
            .set_sample_rate(tx_ch, sample_rate_hz)
            .map_err(|e| Error::from(format!("bladeRF: set TX sample rate: {}", e)))?;
        info!(
            "bladeRF: TX sample_rate requested={} actual={}",
            sample_rate_hz, actual_rate
        );

        device
            .set_gain(tx_ch, tx_gain_db)
            .map_err(|e| Error::from(format!("bladeRF: set TX gain: {}", e)))?;
        info!("bladeRF: TX gain={}dB", tx_gain_db);

        let nb = num_buffers.unwrap_or(DEFAULT_NUM_BUFFERS);
        let bs = buffer_size.unwrap_or(DEFAULT_BUFFER_SIZE);
        let nt = num_transfers.unwrap_or(DEFAULT_NUM_TRANSFERS);
        let st = stream_timeout_ms.unwrap_or(DEFAULT_STREAM_TIMEOUT_MS);

        Ok(BladeRfRadio {
            device,
            channel,
            sample_rate: actual_rate,
            tx_shaper: TxPulseShaper::new(),
            tx_lo_offset_hz: 0.0,
            tx_sample_rate_hz: TX_SAMPLE_RATE as f64,
            tx_nco_phase_rad: 0.0,
            rx_configured: false,
            num_buffers: nb,
            buffer_size: bs,
            num_transfers: nt,
            stream_timeout_ms: st,
        })
    }

    /// Set the software TX LO offset.
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

impl Radio for BladeRfRadio {
    fn tick_rate(&self) -> u64 {
        self.sample_rate as u64
    }

    fn set_tx_frequency(&mut self, center_frequency: usize) -> Result<(), Error> {
        let rf_freq = center_frequency as u64 + self.tx_lo_offset_hz as u64;
        self.device
            .set_frequency(tx_channel(self.channel), rf_freq)
            .map_err(|e| Error::from(format!("bladeRF: set TX freq: {}", e)))?;
        debug!(
            "bladeRF: TX freq set to {} (RF={}, LO offset={})",
            center_frequency, rf_freq, self.tx_lo_offset_hz
        );
        Ok(())
    }

    fn set_tx_sample_rate(&mut self, sample_rate: usize) -> Result<(), Error> {
        let actual = self
            .device
            .set_sample_rate(tx_channel(self.channel), sample_rate as u32)
            .map_err(|e| Error::from(format!("bladeRF: set TX sample rate: {}", e)))?;
        self.sample_rate = actual;
        self.tx_sample_rate_hz = actual as f64;
        self.tx_nco_phase_rad = 0.0;
        Ok(())
    }

    fn set_tx_bandwidth(&mut self, bandwidth: usize) -> Result<(), Error> {
        self.device
            .set_bandwidth(tx_channel(self.channel), bandwidth as u32)
            .map_err(|e| Error::from(format!("bladeRF: set TX bandwidth: {}", e)))?;
        Ok(())
    }

    fn set_tx_lo_offset_hz(&mut self, offset_hz: i64) -> Result<(), Error> {
        self.set_tx_lo_offset(offset_hz)
    }

    fn setup_rx(
        &mut self,
        _channel: usize,
        antenna: &str,
        frequency_hz: f64,
        sample_rate_hz: f64,
        bandwidth_hz: f64,
        gain_db: Option<f64>,
    ) -> Result<(), Error> {
        let rx_ch = rx_channel(self.channel);

        // Log available RX RF ports and set antenna if specified.
        if let Ok(ports) = self.device.get_rf_ports(rx_ch) {
            info!("bladeRF: available RX RF ports: {:?}", ports);
        }
        if !antenna.is_empty() {
            self.device
                .set_rf_port(rx_ch, antenna)
                .map_err(|e| Error::from(format!("bladeRF: set RX RF port: {}", e)))?;
            info!("bladeRF: RX RF port set to '{}'", antenna);
        }
        if let Ok(port) = self.device.get_rf_port(rx_ch) {
            info!("bladeRF: RX RF port active: '{}'", port);
        }

        self.device
            .set_frequency(rx_ch, frequency_hz as u64)
            .map_err(|e| Error::from(format!("bladeRF: set RX freq: {}", e)))?;

        let actual_rate = self
            .device
            .set_sample_rate(rx_ch, sample_rate_hz as u32)
            .map_err(|e| Error::from(format!("bladeRF: set RX sample rate: {}", e)))?;

        self.device
            .set_bandwidth(rx_ch, bandwidth_hz as u32)
            .map_err(|e| Error::from(format!("bladeRF: set RX bandwidth: {}", e)))?;

        if let Some(gain) = gain_db {
            // Manual gain mode = 1
            self.device
                .set_gain_mode(rx_ch, 1)
                .map_err(|e| Error::from(format!("bladeRF: set RX gain mode: {}", e)))?;
            self.device
                .set_gain(rx_ch, gain as i32)
                .map_err(|e| Error::from(format!("bladeRF: set RX gain: {}", e)))?;
        }

        // Configure RX sync: BLADERF_RX_X1 = 0
        let rx_layout = 0u32;
        self.device
            .sync_config(
                rx_layout,
                FORMAT_SC16_Q11_META,
                self.num_buffers,
                self.buffer_size,
                self.num_transfers,
                self.stream_timeout_ms,
            )
            .map_err(|e| Error::from(format!("bladeRF: sync_config RX: {}", e)))?;

        self.rx_configured = true;

        let actual_freq = self.device.get_frequency(rx_ch).unwrap_or(0);
        info!(
            "bladeRF: RX configured freq={} rate={} bw={} gain={:?}",
            actual_freq, actual_rate, bandwidth_hz, gain_db
        );
        Ok(())
    }

    fn split(self: Box<Self>) -> Result<(Box<dyn RadioTx>, Option<Box<dyn RadioRx>>), Error> {
        let device = Arc::new(self.device);
        let shared_clock = Arc::new(AtomicU64::new(0));

        // Configure TX sync after RX sync (setup_rx configures RX sync).
        // Doing TX sync_config in the constructor and RX in setup_rx caused
        // the libbladeRF sync worker to time out on shutdown/restart.
        let tx_layout = 1u32; // BLADERF_TX_X1
        device
            .sync_config(
                tx_layout,
                FORMAT_SC16_Q11_META,
                self.num_buffers,
                self.buffer_size,
                self.num_transfers,
                self.stream_timeout_ms,
            )
            .map_err(|e| Error::from(format!("bladeRF: sync_config TX: {}", e)))?;
        info!(
            "bladeRF: TX sync configured buffers={} buf_size={} transfers={} timeout={}ms",
            self.num_buffers, self.buffer_size, self.num_transfers, self.stream_timeout_ms
        );

        device
            .enable_module(tx_channel(self.channel), true)
            .map_err(|e| Error::from(format!("bladeRF: enable TX module: {}", e)))?;
        info!("bladeRF: TX module enabled");

        let tx_sync = TxSync::new(&device);
        let tx = BladeRfTxHalf {
            _device: device.clone(),
            tx_sync,
            channel: self.channel,
            sample_rate: self.sample_rate,
            tx_shaper: self.tx_shaper,
            tx_lo_offset_hz: self.tx_lo_offset_hz,
            tx_sample_rate_hz: self.tx_sample_rate_hz,
            tx_nco_phase_rad: self.tx_nco_phase_rad,
            shared_clock: shared_clock.clone(),
            stream_timeout_ms: self.stream_timeout_ms,
            burst_active: false,
            module_enabled: true,
        };

        let rx = if self.rx_configured {
            device
                .enable_module(rx_channel(self.channel), true)
                .map_err(|e| Error::from(format!("bladeRF: enable RX module: {}", e)))?;
            info!("bladeRF: RX module enabled");

            let rx_sync = RxSync::new(&device);
            Some(Box::new(BladeRfRxHalf {
                _device: device.clone(),
                rx_sync,
                channel: self.channel,
                sample_rate: self.sample_rate,
                shared_clock: shared_clock.clone(),
                module_enabled: true,
            }) as Box<dyn RadioRx>)
        } else {
            None
        };

        Ok((Box::new(tx), rx))
    }
}

// ---------------------------------------------------------------------------
// TX half
// ---------------------------------------------------------------------------

struct BladeRfTxHalf {
    _device: Arc<Device>,
    tx_sync: TxSync,
    channel: u32,
    sample_rate: u32,
    tx_shaper: TxPulseShaper,
    tx_lo_offset_hz: f64,
    tx_sample_rate_hz: f64,
    tx_nco_phase_rad: f64,
    shared_clock: Arc<AtomicU64>,
    stream_timeout_ms: u32,
    burst_active: bool,
    module_enabled: bool,
}

unsafe impl Send for BladeRfTxHalf {}

impl RadioTx for BladeRfTxHalf {
    fn tick_rate(&self) -> u64 {
        self.sample_rate as u64
    }

    fn get_hardware_time(&self) -> Result<u64, Error> {
        match self._device.get_timestamp(1) {
            Ok(ts) if ts > 0 => Ok(ts),
            _ => Ok(self.shared_clock.load(Ordering::Relaxed)),
        }
    }

    fn set_hardware_time(&self, _ticks: u64) -> Result<(), Error> {
        Ok(())
    }

    fn transmit(&mut self, samples: &[Complex32]) -> Result<(), Error> {
        let mut shaped = self.tx_shaper.shape(samples);
        BladeRfRadio::apply_tx_lo_offset(
            self.tx_lo_offset_hz,
            self.tx_sample_rate_hz,
            &mut self.tx_nco_phase_rad,
            &mut shaped,
        );

        let sc16: Vec<Sc16Q11> = shaped.iter().map(|s| Sc16Q11::from_complex32(*s)).collect();
        let mut flags = META_FLAG_TX_NOW;
        if !self.burst_active {
            flags |= META_FLAG_TX_BURST_START;
            self.burst_active = true;
        }
        let mut meta = StreamMeta {
            flags,
            ..Default::default()
        };
        self.tx_sync
            .send(&sc16, Some(&mut meta), self.stream_timeout_ms)
            .map_err(|e| Error::from(format!("bladeRF: TX send: {}", e)))?;
        if meta.status & META_STATUS_UNDERRUN != 0 {
            warn!("bladeRF: TX underrun detected");
        }
        Ok(())
    }

    fn transmit_at(&mut self, samples: &[Complex32], tick: Option<u64>) -> Result<(), Error> {
        match tick {
            Some(ts) => {
                let mut shaped = self.tx_shaper.shape(samples);
                BladeRfRadio::apply_tx_lo_offset(
                    self.tx_lo_offset_hz,
                    self.tx_sample_rate_hz,
                    &mut self.tx_nco_phase_rad,
                    &mut shaped,
                );

                let sc16: Vec<Sc16Q11> =
                    shaped.iter().map(|s| Sc16Q11::from_complex32(*s)).collect();
                let mut meta = if !self.burst_active {
                    // First send: set the starting timestamp and begin burst.
                    // Subsequent sends stream continuously without per-buffer
                    // timestamp updates so the FPGA outputs them sequentially.
                    self.burst_active = true;
                    StreamMeta {
                        timestamp: ts,
                        flags: META_FLAG_TX_BURST_START | META_FLAG_TX_UPDATE_TIMESTAMP,
                        ..Default::default()
                    }
                } else {
                    StreamMeta::default()
                };
                match self
                    .tx_sync
                    .send(&sc16, Some(&mut meta), self.stream_timeout_ms)
                {
                    Ok(()) => {}
                    Err(e) if e.to_string().contains("in the past") => {
                        warn!("bladeRF: TX late @{}: {}", ts, e);
                    }
                    Err(e) => {
                        return Err(Error::from(format!("bladeRF: TX send @{}: {}", ts, e)));
                    }
                }
                if meta.status & META_STATUS_UNDERRUN != 0 {
                    warn!("bladeRF: TX underrun detected");
                }
                Ok(())
            }
            None => self.transmit(samples),
        }
    }

    fn enable_transmit(&mut self, enable: bool) -> Result<(), Error> {
        if enable {
            self.tx_nco_phase_rad = 0.0;
            self.burst_active = false;
            if !self.module_enabled {
                self._device
                    .enable_module(tx_channel(self.channel), enable)
                    .map_err(|e| Error::from(format!("bladeRF: enable TX: {}", e)))?;
                self.module_enabled = true;
            }
        } else {
            if self.burst_active {
                let zero = [Sc16Q11 { i: 0, q: 0 }; 1];
                let mut meta = StreamMeta {
                    flags: META_FLAG_TX_BURST_END | META_FLAG_TX_NOW,
                    ..Default::default()
                };
                let _ = self
                    .tx_sync
                    .send(&zero, Some(&mut meta), self.stream_timeout_ms);
                self.burst_active = false;
            }
            if self.module_enabled {
                self._device
                    .enable_module(tx_channel(self.channel), false)
                    .map_err(|e| Error::from(format!("bladeRF: disable TX: {}", e)))?;
                self.module_enabled = false;
            }
        }
        Ok(())
    }

    fn enable_transmit_at(&mut self, enable: bool, _tick: Option<u64>) -> Result<(), Error> {
        self.enable_transmit(enable)
    }
}

impl Drop for BladeRfTxHalf {
    fn drop(&mut self) {
        if self.burst_active {
            let zero = [Sc16Q11 { i: 0, q: 0 }; 1];
            let mut meta = StreamMeta {
                flags: META_FLAG_TX_BURST_END | META_FLAG_TX_NOW,
                ..Default::default()
            };
            let _ = self
                .tx_sync
                .send(&zero, Some(&mut meta), self.stream_timeout_ms);
        }
        let _ = self._device.enable_module(tx_channel(self.channel), false);
    }
}

// ---------------------------------------------------------------------------
// RX half
// ---------------------------------------------------------------------------

struct BladeRfRxHalf {
    _device: Arc<Device>,
    rx_sync: RxSync,
    channel: u32,
    sample_rate: u32,
    shared_clock: Arc<AtomicU64>,
    module_enabled: bool,
}

unsafe impl Send for BladeRfRxHalf {}

impl RadioRx for BladeRfRxHalf {
    fn tick_rate(&self) -> u64 {
        self.sample_rate as u64
    }

    fn get_hardware_time(&self) -> Result<u64, Error> {
        match self._device.get_timestamp(0) {
            Ok(ts) => {
                self.shared_clock.store(ts, Ordering::Relaxed);
                Ok(ts)
            }
            Err(_) => Ok(self.shared_clock.load(Ordering::Relaxed)),
        }
    }

    fn rx_read(&mut self, buf: &mut [Complex32], timeout_us: i64) -> Result<RxReadResult, Error> {
        let timeout_ms = (timeout_us / 1000).max(1) as u32;

        let mut sc16_buf = vec![Sc16Q11::default(); buf.len()];
        let mut meta = StreamMeta {
            flags: META_FLAG_RX_NOW,
            ..Default::default()
        };

        match self.rx_sync.recv(&mut sc16_buf, &mut meta, timeout_ms) {
            Ok(n) => {
                for i in 0..n.min(buf.len()) {
                    buf[i] = sc16_buf[i].to_complex32();
                }

                let end_ts = meta.timestamp + n as u64;
                self.shared_clock.store(end_ts, Ordering::Relaxed);

                Ok(RxReadResult {
                    samples_read: n,
                    time_ticks: meta.timestamp,
                    overflow: (meta.status & META_STATUS_OVERRUN) != 0,
                })
            }
            Err(e) => Err(Error::from(format!("bladeRF: RX recv: {}", e))),
        }
    }

    fn rx_activate(&mut self, _time_ticks: Option<u64>) -> Result<(), Error> {
        if !self.module_enabled {
            self._device
                .enable_module(rx_channel(self.channel), true)
                .map_err(|e| Error::from(format!("bladeRF: enable RX: {}", e)))?;
            self.module_enabled = true;
        }
        Ok(())
    }

    fn rx_deactivate(&mut self) -> Result<(), Error> {
        if self.module_enabled {
            self._device
                .enable_module(rx_channel(self.channel), false)
                .map_err(|e| Error::from(format!("bladeRF: disable RX: {}", e)))?;
            self.module_enabled = false;
        }
        Ok(())
    }
}

impl Drop for BladeRfRxHalf {
    fn drop(&mut self) {
        let _ = self._device.enable_module(rx_channel(self.channel), false);
    }
}
