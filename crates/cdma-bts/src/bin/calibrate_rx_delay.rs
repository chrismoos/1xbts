//! Measure the inherent RX pipeline delay of an SDR.
//!
//! Schedules a known calibration chirp on TX at a precise hardware time, then
//! cross-correlates it against the RX stream to determine when the burst
//! actually arrives. The delta (RX_arrival - TX_scheduled) lumps together
//! DAC + TX RF + loopback path + RX RF + ADC + FPGA + host transport latency.
//!
//! Run on a full-duplex SDR with either an external TX→RX cable+attenuator
//! (most accurate) or in-air TX→RX bleed-through. Print the result and paste
//! it into your radio config as `rx_sample_delay`.
//!
//! Supports four backends: SoapySDR (default), native UHD, native LimeSDR, and native bladeRF.
//!
//! # Examples
//!
//! ## LimeSDR Mini 2.0 (native)
//! ```sh
//! cargo run --release -p cdma-bts --bin calibrate_rx_delay -- \
//!   --backend lime --tx-antenna BAND1 --rx-antenna LNAW
//! ```
//!
//! ## Ettus B210 (native UHD)
//! ```sh
//! cargo run --release -p cdma-bts --bin calibrate_rx_delay -- \
//!   --backend uhd --device "type=b200" --tx-antenna TX/RX --rx-antenna RX2
//! ```
//!
//! ## Ettus B210 via SoapySDR (SoapyUHD bridge)
//! ```sh
//! cargo run --release -p cdma-bts --bin calibrate_rx_delay -- \
//!   --device "driver=uhd,type=b200" --tx-antenna TX/RX --rx-antenna RX2
//! ```
//!
//! ## LimeSDR via SoapySDR (SoapyLMS bridge)
//! ```sh
//! cargo run --release -p cdma-bts --bin calibrate_rx_delay -- \
//!   --device "driver=lime" --tx-antenna BAND1 --rx-antenna LNAW
//! ```
//!
//! ## Using a radio config file
//! ```sh
//! cargo run --release -p cdma-bts --bin calibrate_rx_delay -- \
//!   --config config/radio_uhd_b210_native.json
//! ```
//!
//! ## bladeRF Micro 2.0 (native)
//! ```sh
//! cargo run --release -p cdma-bts --bin calibrate_rx_delay -- \
//!   --backend bladerf --tx-antenna TXA --rx-antenna B_BALANCED
//! ```
//!
//! ## bladeRF using a config file
//! ```sh
//! cargo run --release -p cdma-bts --bin calibrate_rx_delay -- \
//!   --config config/radio_bladerf_micro2.json
//! ```
//!
//! # Options
//!
//! - `--backend <soapy|uhd|lime|bladerf>` — Radio backend (default: soapy)
//! - `--device <str>` — Device args string (default: auto-detect)
//! - `--config <path>` — Load device/antenna from a radio config JSON
//! - `--tx-antenna <name>` — TX antenna (e.g. TX/RX, BAND1, TXA)
//! - `--rx-antenna <name>` — RX antenna (e.g. RX2, LNAW, B_BALANCED)
//! - `--tx-gain-db <dB>` — TX gain (default: 20, low for loopback safety)
//! - `--rx-gain-db <dB>` — RX gain (default: 20)
//! - `--repeats <n>` — Measurement rounds (default: 8, min/max dropped)
//! - `--master-clock-rate <Hz>` — UHD master clock rate (default: 49152000)
//! - `--oversample <n>` — LimeSDR oversampling factor (default: 0 = auto)
//! - `--tx-loop` — Diagnostic: transmit chirp continuously (no measurement)
//! - `--tx-tone-hz <Hz>` — Diagnostic: transmit CW tone (implies --tx-loop)
//! - `--full-search` — Search entire capture window (slow, for debugging)
//! - `--json` — Machine-readable JSON output

use std::{fs, path::PathBuf, thread, time::Duration};

use clap::Parser;
use num_complex::Complex32;
use serde::Deserialize;
use soapysdr::{Direction, ErrorCode as SoapyErrorCode};

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "Measure inherent RX pipeline delay of an SDR (samples / ns)."
)]
struct Cli {
    /// Radio backend: "soapy" (default), "uhd", "lime", or "bladerf".
    #[arg(long, default_value = "soapy")]
    backend: String,

    /// Optional radio config JSON (soapy/uhd/lime/blade_rf format).
    /// The backend is auto-detected from the config's `kind` field unless
    /// `--backend` is also specified. Used only for `device`, `channel`,
    /// `antenna`, and `rx_antenna` defaults. Gains are intentionally NOT
    /// loaded from the config: the bin uses low fixed defaults so it cannot
    /// overload the SDR front-end on a cabled loopback. Override with
    /// `--tx-gain-db` / `--rx-gain-db` if needed.
    #[arg(long, value_name = "PATH")]
    config: Option<PathBuf>,

    /// Device string. For SoapySDR: e.g. `driver=uhd`.
    /// For UHD: e.g. `type=b200`. For Lime: e.g. `""` for first device.
    /// Overrides --config.
    #[arg(long)]
    device: Option<String>,

    /// SoapySDR channel index.
    #[arg(long, default_value_t = 0)]
    channel: usize,

    /// TX antenna name.
    #[arg(long)]
    tx_antenna: Option<String>,

    /// RX antenna name.
    #[arg(long)]
    rx_antenna: Option<String>,

    /// TX/RX center frequency in Hz (loopback - same on both sides).
    #[arg(long, default_value_t = 870_000_000)]
    tx_freq_hz: usize,

    /// Sample rate in Hz (TX = RX).
    #[arg(long, default_value_t = 4_915_200)]
    sample_rate_hz: usize,

    /// Analog bandwidth in Hz.
    #[arg(long, default_value_t = 4_000_000)]
    bandwidth_hz: usize,

    /// TX gain in dB. Low default to avoid overloading the RX front-end on a
    /// cabled loopback.
    #[arg(long, default_value_t = 20.0)]
    tx_gain_db: f64,

    /// RX gain in dB. Low default to avoid clipping the cabled-loopback signal.
    #[arg(long, default_value_t = 20.0)]
    rx_gain_db: f64,

    /// Schedule each TX burst this many milliseconds in the future.
    #[arg(long, default_value_t = 50)]
    tx_lead_ms: u64,

    /// Listen for this many milliseconds after the scheduled TX time.
    #[arg(long, default_value_t = 50)]
    listen_ms: u64,

    /// Number of measurement rounds. Min/max are dropped before averaging.
    #[arg(long, default_value_t = 8)]
    repeats: usize,

    /// Length of the calibration chirp in samples. Longer chirps give more
    /// matched-filter processing gain (~sqrt(BT)) and are easier to see on a
    /// waterfall, at the cost of slightly coarser peak localization.
    #[arg(long, default_value_t = 8192)]
    chirp_len: usize,

    /// Trailing zero-pad samples appended to the chirp before transmission.
    /// USB-based SDRs (LimeSDR Mini, B200) won't flush sub-USB-frame bursts;
    /// padding to a few thousand samples and asserting `end_burst` forces the
    /// driver to push the burst out.
    #[arg(long, default_value_t = 8192)]
    tx_pad_samples: usize,

    /// Required peak/sidelobe SNR in dB to accept a round.
    #[arg(long, default_value_t = 6.0)]
    min_snr_db: f64,

    /// Emit machine-readable JSON instead of a human-readable summary.
    #[arg(long)]
    json: bool,

    /// Diagnostic: skip calibration and just transmit the chirp burst in a
    /// continuous untimed loop until Ctrl-C. Use this to verify on a separate
    /// SDR/waterfall that the device is actually radiating.
    #[arg(long)]
    tx_loop: bool,

    /// Diagnostic: instead of the chirp, transmit a continuous complex
    /// sinusoid at this baseband frequency (Hz). Implies --tx-loop. A single
    /// sharp tone is the easiest possible signal to see on a waterfall, so
    /// use this to verify that the TX path radiates at all.
    #[arg(long)]
    tx_tone_hz: Option<f64>,

    /// Search the entire capture window for the chirp instead of only a
    /// ±5 ms window around the predicted arrival time. Slower but useful
    /// when you suspect the timed TX isn't being honored at t_tx.
    #[arg(long)]
    full_search: bool,

    /// UHD master clock rate in Hz. Default: 49152000.
    #[arg(long, default_value_t = 49_152_000)]
    master_clock_rate: u64,

    /// LimeSDR oversample factor (0 = auto). Default: 0.
    #[arg(long, default_value_t = 0)]
    oversample: usize,
}

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
enum RadioConfigFile {
    Soapy {
        device: String,
        #[serde(default)]
        channel: Option<usize>,
        antenna: String,
        #[serde(default)]
        rx_antenna: Option<String>,
    },
    Uhd {
        device: String,
        #[serde(default)]
        channel: Option<usize>,
        antenna: String,
        #[serde(default)]
        rx_antenna: Option<String>,
        #[serde(default)]
        master_clock_rate: Option<u64>,
    },
    Lime {
        #[serde(default)]
        device: Option<String>,
        #[serde(default)]
        channel: Option<usize>,
        #[serde(default)]
        tx_antenna: Option<String>,
        #[serde(default)]
        rx_antenna: Option<String>,
        #[serde(default)]
        oversample: Option<usize>,
    },
    BladeRf {
        #[serde(default)]
        device: Option<String>,
        #[serde(default)]
        channel: Option<u32>,
        #[serde(default)]
        tx_antenna: Option<String>,
        #[serde(default)]
        rx_antenna: Option<String>,
        #[serde(default)]
        fpga_path: Option<String>,
        #[serde(default)]
        num_buffers: Option<u32>,
        #[serde(default)]
        buffer_size: Option<u32>,
        #[serde(default)]
        num_transfers: Option<u32>,
        #[serde(default)]
        stream_timeout_ms: Option<u32>,
    },
    #[serde(other)]
    Other,
}

struct LoadedConfig {
    /// Backend name inferred from the config file's `kind` field.
    detected_backend: String,
    device: String,
    channel: Option<usize>,
    tx_antenna: String,
    rx_antenna: Option<String>,
    /// UHD master clock rate from config, if specified.
    master_clock_rate: Option<u64>,
    /// LimeSDR oversample from config, if specified.
    oversample: Option<usize>,
    /// bladeRF channel (separate from usize channel — bladeRF uses u32).
    bladerf_channel: Option<u32>,
    /// bladeRF FPGA bitstream path.
    bladerf_fpga_path: Option<String>,
    /// bladeRF stream buffer count.
    bladerf_num_buffers: Option<u32>,
    /// bladeRF buffer size in samples.
    bladerf_buffer_size: Option<u32>,
    /// bladeRF number of USB transfers.
    bladerf_num_transfers: Option<u32>,
    /// bladeRF stream timeout in ms.
    bladerf_stream_timeout_ms: Option<u32>,
}

fn load_config(path: &PathBuf) -> Result<LoadedConfig, Box<dyn std::error::Error>> {
    let raw = fs::read_to_string(path)?;
    let cfg: RadioConfigFile = serde_json::from_str(&raw)?;
    match cfg {
        RadioConfigFile::Soapy {
            device,
            channel,
            antenna,
            rx_antenna,
        } => Ok(LoadedConfig {
            detected_backend: "soapy".to_string(),
            device,
            channel,
            tx_antenna: antenna,
            rx_antenna,
            master_clock_rate: None,
            oversample: None,
            bladerf_channel: None,
            bladerf_fpga_path: None,
            bladerf_num_buffers: None,
            bladerf_buffer_size: None,
            bladerf_num_transfers: None,
            bladerf_stream_timeout_ms: None,
        }),
        RadioConfigFile::Uhd {
            device,
            channel,
            antenna,
            rx_antenna,
            master_clock_rate,
        } => Ok(LoadedConfig {
            detected_backend: "uhd".to_string(),
            device,
            channel,
            tx_antenna: antenna,
            rx_antenna,
            master_clock_rate,
            oversample: None,
            bladerf_channel: None,
            bladerf_fpga_path: None,
            bladerf_num_buffers: None,
            bladerf_buffer_size: None,
            bladerf_num_transfers: None,
            bladerf_stream_timeout_ms: None,
        }),
        RadioConfigFile::Lime {
            device,
            channel,
            tx_antenna,
            rx_antenna,
            oversample,
        } => Ok(LoadedConfig {
            detected_backend: "lime".to_string(),
            device: device.unwrap_or_default(),
            channel,
            tx_antenna: tx_antenna.unwrap_or_else(|| "BAND1".to_string()),
            rx_antenna,
            master_clock_rate: None,
            oversample,
            bladerf_channel: None,
            bladerf_fpga_path: None,
            bladerf_num_buffers: None,
            bladerf_buffer_size: None,
            bladerf_num_transfers: None,
            bladerf_stream_timeout_ms: None,
        }),
        RadioConfigFile::BladeRf {
            device,
            channel,
            tx_antenna,
            rx_antenna,
            fpga_path,
            num_buffers,
            buffer_size,
            num_transfers,
            stream_timeout_ms,
        } => Ok(LoadedConfig {
            detected_backend: "bladerf".to_string(),
            device: device.unwrap_or_default(),
            channel: None, // bladeRF channel is u32; stored separately
            tx_antenna: tx_antenna.unwrap_or_else(|| "TXA".to_string()),
            rx_antenna,
            master_clock_rate: None,
            oversample: None,
            bladerf_channel: channel,
            bladerf_fpga_path: fpga_path,
            bladerf_num_buffers: num_buffers,
            bladerf_buffer_size: buffer_size,
            bladerf_num_transfers: num_transfers,
            bladerf_stream_timeout_ms: stream_timeout_ms,
        }),
        RadioConfigFile::Other => Err("unsupported radio config kind in --config file".into()),
    }
}

// ---------------------------------------------------------------------------
// CalibrationBackend trait — thin abstraction over raw timed TX/RX I/O.
//
// Unlike the Radio/RadioTx/RadioRx traits in the SDR module, this trait
// passes raw samples without pulse shaping, which is what the calibrator
// needs (the chirp is already the signal to be transmitted).
//
// Timestamps are in *ticks* whose rate is backend-specific:
//   SoapySDR  → nanoseconds (tick_rate = 1 GHz)
//   UHD       → master clock ticks (tick_rate = master_clock_rate)
//   LimeSDR   → sample counts (tick_rate = sample_rate)
// ---------------------------------------------------------------------------

trait CalibrationBackend {
    /// Clock ticks per second.
    fn tick_rate(&self) -> u64;

    /// Read the current hardware time in ticks.
    fn get_time(&mut self) -> Result<u64, Box<dyn std::error::Error>>;

    /// Set (reset) the hardware time in ticks.
    fn set_time(&mut self, ticks: u64) -> Result<(), Box<dyn std::error::Error>>;

    /// Activate the TX stream.
    fn activate_tx(&mut self) -> Result<(), Box<dyn std::error::Error>>;

    /// Activate the RX stream.
    fn activate_rx(&mut self) -> Result<(), Box<dyn std::error::Error>>;

    /// Deactivate the TX stream.
    fn deactivate_tx(&mut self) -> Result<(), Box<dyn std::error::Error>>;

    /// Deactivate the RX stream.
    fn deactivate_rx(&mut self) -> Result<(), Box<dyn std::error::Error>>;

    /// Transmit raw samples at the given hardware time (ticks). `end_burst`
    /// hints the driver to flush a sub-frame burst.
    fn send_timed(
        &mut self,
        samples: &[Complex32],
        time_ticks: Option<u64>,
        end_burst: bool,
    ) -> Result<(), Box<dyn std::error::Error>>;

    /// Read raw samples. Returns `(samples_read, block_timestamp_ticks)`.
    /// On timeout returns `Ok((0, 0, false))`. The third element indicates
    /// overflow.
    fn recv(
        &mut self,
        buf: &mut [Complex32],
        timeout_us: i64,
    ) -> Result<(usize, u64, bool), Box<dyn std::error::Error>>;
}

// ---------------------------------------------------------------------------
// SoapySDR backend (always available — soapysdr is a non-optional dep)
// ---------------------------------------------------------------------------

struct SoapyBackend {
    device: soapysdr::Device,
    _channel: usize,
    tx_stream: soapysdr::TxStream<Complex32>,
    rx_stream: soapysdr::RxStream<Complex32>,
}

impl SoapyBackend {
    fn new(
        device_str: &str,
        channel: usize,
        tx_antenna: &str,
        rx_antenna: &str,
        freq_hz: f64,
        sample_rate_hz: f64,
        bandwidth_hz: f64,
        tx_gain_db: f64,
        rx_gain_db: f64,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let device = soapysdr::Device::new(device_str)?;

        device.set_antenna(Direction::Tx, channel, tx_antenna)?;
        device.set_frequency(Direction::Tx, channel, freq_hz, "")?;
        device.set_sample_rate(Direction::Tx, channel, sample_rate_hz)?;
        device.set_bandwidth(Direction::Tx, channel, bandwidth_hz)?;
        device.set_gain(Direction::Tx, channel, tx_gain_db)?;

        device.set_antenna(Direction::Rx, channel, rx_antenna)?;
        device.set_frequency(Direction::Rx, channel, freq_hz, "")?;
        device.set_sample_rate(Direction::Rx, channel, sample_rate_hz)?;
        device.set_bandwidth(Direction::Rx, channel, bandwidth_hz)?;
        device.set_gain(Direction::Rx, channel, rx_gain_db)?;

        if !device.has_hardware_time(None)? {
            return Err("device does not support hardware time".into());
        }

        let tx_stream = device.tx_stream::<Complex32>(&[channel])?;
        let rx_stream = device.rx_stream::<Complex32>(&[channel])?;

        Ok(SoapyBackend {
            device,
            _channel: channel,
            tx_stream,
            rx_stream,
        })
    }
}

impl CalibrationBackend for SoapyBackend {
    fn tick_rate(&self) -> u64 {
        1_000_000_000 // SoapySDR uses nanoseconds
    }

    fn get_time(&mut self) -> Result<u64, Box<dyn std::error::Error>> {
        Ok(self.device.get_hardware_time(None)? as u64)
    }

    fn set_time(&mut self, ticks: u64) -> Result<(), Box<dyn std::error::Error>> {
        self.device.set_hardware_time(None, ticks as i64)?;
        Ok(())
    }

    fn activate_tx(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.tx_stream.activate(None)?;
        Ok(())
    }

    fn activate_rx(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.rx_stream.activate(None)?;
        Ok(())
    }

    fn deactivate_tx(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.tx_stream.deactivate(None).ok();
        Ok(())
    }

    fn deactivate_rx(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.rx_stream.deactivate(None).ok();
        Ok(())
    }

    fn send_timed(
        &mut self,
        samples: &[Complex32],
        time_ticks: Option<u64>,
        end_burst: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        self.tx_stream.write_all(
            &[samples],
            time_ticks.map(|t| t as i64),
            end_burst,
            1_000_000,
        )?;
        Ok(())
    }

    fn recv(
        &mut self,
        buf: &mut [Complex32],
        timeout_us: i64,
    ) -> Result<(usize, u64, bool), Box<dyn std::error::Error>> {
        match self.rx_stream.read(&mut [buf], timeout_us) {
            Ok(n) => {
                let t = self.rx_stream.time_ns() as u64;
                Ok((n, t, false))
            }
            Err(err) if err.code == SoapyErrorCode::Timeout => Ok((0, 0, false)),
            Err(err) if err.code == SoapyErrorCode::Overflow => Ok((0, 0, true)),
            Err(err) => Err(format!("rx read failed: {err}").into()),
        }
    }
}

// ---------------------------------------------------------------------------
// UHD backend (feature-gated)
// ---------------------------------------------------------------------------

#[cfg(feature = "uhd-backend")]
struct UhdBackend {
    usrp: uhd::Usrp,
    tx_streamer: uhd::TransmitStreamer<Complex32>,
    rx_streamer: uhd::ReceiveStreamer<Complex32>,
    master_clock_rate: u64,
}

#[cfg(feature = "uhd-backend")]
impl UhdBackend {
    fn new(
        device_str: &str,
        channel: usize,
        tx_antenna: &str,
        rx_antenna: &str,
        freq_hz: f64,
        sample_rate_hz: f64,
        bandwidth_hz: f64,
        tx_gain_db: f64,
        rx_gain_db: f64,
        master_clock_rate: u64,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let mut usrp = uhd::Usrp::open(device_str)
            .map_err(|e| format!("UHD: failed to open device: {}", e))?;

        usrp.set_master_clock_rate(master_clock_rate as f64, 0)
            .map_err(|e| format!("UHD: set master clock rate: {}", e))?;

        // TX setup
        usrp.set_tx_antenna(tx_antenna, channel)
            .map_err(|e| format!("UHD: set TX antenna: {}", e))?;
        usrp.set_tx_frequency(&uhd::TuneRequest::with_frequency(freq_hz), channel)
            .map_err(|e| format!("UHD: set TX freq: {}", e))?;
        usrp.set_tx_sample_rate(sample_rate_hz, channel)
            .map_err(|e| format!("UHD: set TX sample rate: {}", e))?;
        usrp.set_tx_bandwidth(bandwidth_hz, channel)
            .map_err(|e| format!("UHD: set TX bandwidth: {}", e))?;
        usrp.set_tx_gain(tx_gain_db, channel, "")
            .map_err(|e| format!("UHD: set TX gain: {}", e))?;

        // RX setup
        usrp.set_rx_antenna(rx_antenna, channel)
            .map_err(|e| format!("UHD: set RX antenna: {}", e))?;
        usrp.set_rx_frequency(&uhd::TuneRequest::with_frequency(freq_hz), channel)
            .map_err(|e| format!("UHD: set RX freq: {}", e))?;
        usrp.set_rx_sample_rate(sample_rate_hz, channel)
            .map_err(|e| format!("UHD: set RX sample rate: {}", e))?;
        usrp.set_rx_bandwidth(bandwidth_hz, channel)
            .map_err(|e| format!("UHD: set RX bandwidth: {}", e))?;
        usrp.set_rx_gain(rx_gain_db, channel, "")
            .map_err(|e| format!("UHD: set RX gain: {}", e))?;

        let tx_streamer = usrp
            .get_tx_stream(&uhd::StreamArgs::<Complex32>::new("sc16"))
            .map_err(|e| format!("UHD: create TX stream: {}", e))?;
        let rx_streamer = usrp
            .get_rx_stream(&uhd::StreamArgs::<Complex32>::new("sc16"))
            .map_err(|e| format!("UHD: create RX stream: {}", e))?;

        Ok(UhdBackend {
            usrp,
            tx_streamer,
            rx_streamer,
            master_clock_rate,
        })
    }

    fn ticks_to_timespec(ticks: u64, tick_rate: u64) -> uhd::TimeSpec {
        let full_secs = (ticks / tick_rate) as i64;
        let frac_ticks = ticks % tick_rate;
        let frac_secs = frac_ticks as f64 / tick_rate as f64;
        uhd::TimeSpec {
            seconds: full_secs,
            fraction: frac_secs,
        }
    }

    fn timespec_to_ticks(ts: &uhd::TimeSpec, tick_rate: u64) -> u64 {
        let rate_i = tick_rate as i64;
        let ticks_full = ts.seconds * rate_i;
        let ticks_frac = (ts.fraction * tick_rate as f64).round() as i64;
        (ticks_full + ticks_frac) as u64
    }
}

#[cfg(feature = "uhd-backend")]
impl CalibrationBackend for UhdBackend {
    fn tick_rate(&self) -> u64 {
        self.master_clock_rate
    }

    fn get_time(&mut self) -> Result<u64, Box<dyn std::error::Error>> {
        let ts = self
            .usrp
            .get_current_time(0)
            .map_err(|e| format!("UHD: get_current_time: {}", e))?;
        Ok(Self::timespec_to_ticks(&ts, self.master_clock_rate))
    }

    fn set_time(&mut self, ticks: u64) -> Result<(), Box<dyn std::error::Error>> {
        let ts = Self::ticks_to_timespec(ticks, self.master_clock_rate);
        self.usrp
            .set_time_unknown_pps(ts.seconds, ts.fraction)
            .map_err(|e| format!("UHD: set_time_unknown_pps: {}", e))?;
        Ok(())
    }

    fn activate_tx(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        // UHD TX stream doesn't need explicit activation — first send starts it.
        Ok(())
    }

    fn activate_rx(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.rx_streamer
            .send_command(&uhd::StreamCommand {
                time: uhd::StreamTime::Now,
                command_type: uhd::StreamCommandType::StartContinuous,
            })
            .map_err(|e| format!("UHD: RX activate: {}", e))?;
        Ok(())
    }

    fn deactivate_tx(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        // Send end-of-burst to flush.
        if let Ok(eob) = uhd::TransmitMetadata::with_time(0, 0.0, false, true) {
            let empty: &[Complex32] = &[];
            let _ = self.tx_streamer.send_with_metadata(&mut [empty], &eob, 0.1);
        }
        Ok(())
    }

    fn deactivate_rx(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.rx_streamer
            .send_command(&uhd::StreamCommand {
                time: uhd::StreamTime::Now,
                command_type: uhd::StreamCommandType::StopContinuous,
            })
            .map_err(|e| format!("UHD: RX deactivate: {}", e))?;
        Ok(())
    }

    fn send_timed(
        &mut self,
        samples: &[Complex32],
        time_ticks: Option<u64>,
        end_burst: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let metadata = match time_ticks {
            Some(t) => {
                let ts = Self::ticks_to_timespec(t, self.master_clock_rate);
                uhd::TransmitMetadata::with_time(ts.seconds, ts.fraction, true, end_burst)?
            }
            None => uhd::TransmitMetadata::new()?,
        };
        self.tx_streamer
            .send_with_metadata(&mut [samples], &metadata, 1.0)
            .map_err(|e| format!("UHD: TX send: {}", e))?;
        Ok(())
    }

    fn recv(
        &mut self,
        buf: &mut [Complex32],
        timeout_us: i64,
    ) -> Result<(usize, u64, bool), Box<dyn std::error::Error>> {
        let timeout_s = timeout_us as f64 / 1_000_000.0;
        let md = self
            .rx_streamer
            .receive(&mut [buf], timeout_s, false)
            .map_err(|e| format!("UHD: RX recv: {}", e))?;

        if let Some(err) = md.last_error()? {
            use uhd::ReceiveErrorKind;
            match err.kind() {
                ReceiveErrorKind::Timeout => return Ok((0, 0, false)),
                ReceiveErrorKind::Overflow => return Ok((0, 0, true)),
                _ => {
                    eprintln!("UHD: RX error: {:?}", err);
                }
            }
        }

        let time_ticks = md
            .time_spec()?
            .map(|ts| Self::timespec_to_ticks(&ts, self.master_clock_rate))
            .unwrap_or(0);

        Ok((md.samples(), time_ticks, false))
    }
}

// ---------------------------------------------------------------------------
// LimeSDR backend (feature-gated)
// ---------------------------------------------------------------------------

#[cfg(feature = "lime-backend")]
struct LimeBackend {
    _device: limesuite::Device,
    tx_stream: limesuite::TxStream,
    rx_stream: limesuite::RxStream,
    sample_rate: u64,
}

#[cfg(feature = "lime-backend")]
impl LimeBackend {
    fn new(
        device_str: &str,
        channel: usize,
        tx_antenna: &str,
        rx_antenna: &str,
        freq_hz: f64,
        sample_rate_hz: f64,
        bandwidth_hz: f64,
        tx_gain_db: f64,
        rx_gain_db: f64,
        oversample: usize,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        let info = if device_str.is_empty() {
            None
        } else {
            Some(device_str)
        };
        let mut device = limesuite::Device::open(info)
            .map_err(|e| format!("Lime: failed to open device: {}", e))?;

        device.init().map_err(|e| format!("Lime: init: {}", e))?;

        // Enable TX and RX channels.
        device
            .enable_channel(true, channel, true)
            .map_err(|e| format!("Lime: enable TX channel: {}", e))?;
        device
            .enable_channel(false, channel, true)
            .map_err(|e| format!("Lime: enable RX channel: {}", e))?;

        // Set shared sample rate.
        device
            .set_sample_rate(sample_rate_hz, oversample)
            .map_err(|e| format!("Lime: set sample rate: {}", e))?;

        // TX config
        // Resolve antenna index by name.
        let tx_ant_idx = if let Ok(list) = device.antenna_list(true, channel) {
            list.iter()
                .position(|a| a.eq_ignore_ascii_case(tx_antenna))
                .unwrap_or(0)
        } else {
            0
        };
        device
            .set_antenna(true, channel, tx_ant_idx)
            .map_err(|e| format!("Lime: set TX antenna: {}", e))?;
        device
            .set_lo_frequency(true, channel, freq_hz)
            .map_err(|e| format!("Lime: set TX freq: {}", e))?;
        device
            .set_lpf_bw(true, channel, bandwidth_hz)
            .map_err(|e| format!("Lime: set TX LPF BW: {}", e))?;
        device
            .set_gain_db(true, channel, tx_gain_db as u32)
            .map_err(|e| format!("Lime: set TX gain: {}", e))?;
        device
            .calibrate(true, channel, sample_rate_hz)
            .map_err(|e| format!("Lime: calibrate TX: {}", e))?;

        // RX config
        let rx_ant_idx = if let Ok(list) = device.antenna_list(false, channel) {
            list.iter()
                .position(|a| a.eq_ignore_ascii_case(rx_antenna))
                .unwrap_or(2) // LNAW default
        } else {
            2
        };
        device
            .set_antenna(false, channel, rx_ant_idx)
            .map_err(|e| format!("Lime: set RX antenna: {}", e))?;
        device
            .set_lo_frequency(false, channel, freq_hz)
            .map_err(|e| format!("Lime: set RX freq: {}", e))?;
        device
            .set_lpf_bw(false, channel, bandwidth_hz)
            .map_err(|e| format!("Lime: set RX LPF BW: {}", e))?;
        device
            .set_gain_db(false, channel, rx_gain_db as u32)
            .map_err(|e| format!("Lime: set RX gain: {}", e))?;
        device
            .calibrate(false, channel, bandwidth_hz)
            .map_err(|e| format!("Lime: calibrate RX: {}", e))?;

        // Create streams.
        let fifo_size = 1024 * 1024;
        let tx_stream = limesuite::TxStream::new(&mut device, channel as u32, fifo_size)
            .map_err(|e| format!("Lime: create TX stream: {}", e))?;
        let rx_stream = limesuite::RxStream::new(&mut device, channel as u32, fifo_size)
            .map_err(|e| format!("Lime: create RX stream: {}", e))?;

        Ok(LimeBackend {
            _device: device,
            tx_stream,
            rx_stream,
            sample_rate: sample_rate_hz as u64,
        })
    }
}

#[cfg(feature = "lime-backend")]
impl CalibrationBackend for LimeBackend {
    fn tick_rate(&self) -> u64 {
        self.sample_rate // LimeSDR timestamps are sample counts
    }

    fn get_time(&mut self) -> Result<u64, Box<dyn std::error::Error>> {
        let st = self
            .rx_stream
            .status()
            .map_err(|e| format!("Lime: RX stream status: {}", e))?;
        Ok(st.timestamp)
    }

    fn set_time(&mut self, _ticks: u64) -> Result<(), Box<dyn std::error::Error>> {
        // LimeSDR resets timestamps when streams start; no set_time API.
        Ok(())
    }

    fn activate_tx(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.tx_stream
            .start()
            .map_err(|e| format!("Lime: start TX stream: {}", e))?;
        Ok(())
    }

    fn activate_rx(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.rx_stream
            .start()
            .map_err(|e| format!("Lime: start RX stream: {}", e))?;
        Ok(())
    }

    fn deactivate_tx(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        // Flush then stop.
        let flush_meta = limesuite::StreamMeta {
            timestamp: 0,
            wait_for_timestamp: false,
            flush_partial_packet: true,
        };
        let empty: &[Complex32] = &[];
        let _ = self.tx_stream.send(empty, &flush_meta, 100);
        self.tx_stream
            .stop()
            .map_err(|e| format!("Lime: TX stream stop: {}", e))?;
        Ok(())
    }

    fn deactivate_rx(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        self.rx_stream
            .stop()
            .map_err(|e| format!("Lime: RX stream stop: {}", e))?;
        Ok(())
    }

    fn send_timed(
        &mut self,
        samples: &[Complex32],
        time_ticks: Option<u64>,
        _end_burst: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let meta = limesuite::StreamMeta {
            timestamp: time_ticks.unwrap_or(0),
            wait_for_timestamp: time_ticks.is_some(),
            flush_partial_packet: true,
        };
        self.tx_stream
            .send(samples, &meta, 1000)
            .map_err(|e| format!("Lime: TX send: {}", e))?;
        Ok(())
    }

    fn recv(
        &mut self,
        buf: &mut [Complex32],
        timeout_us: i64,
    ) -> Result<(usize, u64, bool), Box<dyn std::error::Error>> {
        let timeout_ms = (timeout_us / 1000).max(1) as u32;
        let mut meta = limesuite::StreamMeta::default();
        match self.rx_stream.recv(buf, &mut meta, timeout_ms) {
            Ok(n) => Ok((n, meta.timestamp, false)),
            Err(e) => {
                // Check for overflow.
                if let Ok(st) = self.rx_stream.status() {
                    if st.overrun > 0 {
                        return Ok((0, 0, true));
                    }
                }
                Err(format!("Lime: RX recv: {}", e).into())
            }
        }
    }
}

// ---------------------------------------------------------------------------
// bladeRF backend (feature-gated)
// ---------------------------------------------------------------------------

#[cfg(feature = "bladerf-backend")]
struct BladeRfBackend {
    device: std::sync::Arc<bladerf::Device>,
    tx_sync: bladerf::TxSync,
    rx_sync: bladerf::RxSync,
    channel: u32,
    sample_rate: u64,
    stream_timeout_ms: u32,
    tx_burst_active: bool,
}

#[cfg(feature = "bladerf-backend")]
impl BladeRfBackend {
    /// BLADERF_FORMAT_SC16_Q11_META
    const FORMAT: u32 = 2;
    /// BLADERF_META_FLAG_TX_BURST_START
    const TX_BURST_START: u32 = 1;
    /// BLADERF_META_FLAG_TX_BURST_END
    const TX_BURST_END: u32 = 2;
    /// BLADERF_META_FLAG_TX_NOW
    const TX_NOW: u32 = 4;
    /// BLADERF_META_FLAG_TX_UPDATE_TIMESTAMP
    const TX_UPDATE_TS: u32 = 8;
    /// BLADERF_META_FLAG_RX_NOW
    const RX_NOW: u32 = 0x8000_0000;
    /// BLADERF_META_STATUS_OVERRUN
    const STATUS_OVERRUN: u32 = 1;

    fn new(
        device_str: &str,
        channel: u32,
        tx_antenna: &str,
        rx_antenna: &str,
        freq_hz: f64,
        sample_rate_hz: f64,
        bandwidth_hz: f64,
        tx_gain_db: f64,
        rx_gain_db: f64,
        fpga_path: Option<&str>,
        num_buffers: u32,
        buffer_size: u32,
        num_transfers: u32,
        stream_timeout_ms: u32,
    ) -> Result<Self, Box<dyn std::error::Error>> {
        use bladerf::device::{rx_channel, tx_channel};

        let id = if device_str.is_empty() {
            None
        } else {
            Some(device_str)
        };
        let device = bladerf::Device::open(id).map_err(|e| format!("bladeRF: open: {}", e))?;

        eprintln!("bladeRF: opened board={}", device.board_name());
        if let Ok(serial) = device.serial() {
            eprintln!("bladeRF: serial={}", serial);
        }

        let fpga_loaded = device
            .is_fpga_configured()
            .map_err(|e| format!("bladeRF: check FPGA: {}", e))?;
        if !fpga_loaded {
            match fpga_path {
                Some(path) => {
                    eprintln!("bladeRF: loading FPGA from {}", path);
                    device
                        .load_fpga(path)
                        .map_err(|e| format!("bladeRF: load FPGA: {}", e))?;
                }
                None => {
                    return Err("bladeRF: FPGA not configured and no fpga_path specified".into());
                }
            }
        }

        let tx_ch = tx_channel(channel);
        let rx_ch = rx_channel(channel);

        // TX setup.
        if !tx_antenna.is_empty() {
            device
                .set_rf_port(tx_ch, tx_antenna)
                .map_err(|e| format!("bladeRF: set TX RF port: {}", e))?;
        }
        device
            .set_frequency(tx_ch, freq_hz as u64)
            .map_err(|e| format!("bladeRF: set TX freq: {}", e))?;
        let actual_rate = device
            .set_sample_rate(tx_ch, sample_rate_hz as u32)
            .map_err(|e| format!("bladeRF: set TX sample rate: {}", e))?;
        device
            .set_bandwidth(tx_ch, bandwidth_hz as u32)
            .map_err(|e| format!("bladeRF: set TX bandwidth: {}", e))?;
        device
            .set_gain(tx_ch, tx_gain_db as i32)
            .map_err(|e| format!("bladeRF: set TX gain: {}", e))?;

        // RX setup.
        if !rx_antenna.is_empty() {
            device
                .set_rf_port(rx_ch, rx_antenna)
                .map_err(|e| format!("bladeRF: set RX RF port: {}", e))?;
        }
        device
            .set_frequency(rx_ch, freq_hz as u64)
            .map_err(|e| format!("bladeRF: set RX freq: {}", e))?;
        device
            .set_sample_rate(rx_ch, sample_rate_hz as u32)
            .map_err(|e| format!("bladeRF: set RX sample rate: {}", e))?;
        device
            .set_bandwidth(rx_ch, bandwidth_hz as u32)
            .map_err(|e| format!("bladeRF: set RX bandwidth: {}", e))?;
        device
            .set_gain_mode(rx_ch, 1)
            .map_err(|e| format!("bladeRF: set RX gain mode: {}", e))?;
        device
            .set_gain(rx_ch, rx_gain_db as i32)
            .map_err(|e| format!("bladeRF: set RX gain: {}", e))?;

        // Configure sync streams: RX first, then TX.
        device
            .sync_config(
                0u32,
                Self::FORMAT,
                num_buffers,
                buffer_size,
                num_transfers,
                stream_timeout_ms,
            )
            .map_err(|e| format!("bladeRF: sync_config RX: {}", e))?;
        device
            .sync_config(
                1u32,
                Self::FORMAT,
                num_buffers,
                buffer_size,
                num_transfers,
                stream_timeout_ms,
            )
            .map_err(|e| format!("bladeRF: sync_config TX: {}", e))?;

        let device = std::sync::Arc::new(device);
        let tx_sync = bladerf::TxSync::new(&device);
        let rx_sync = bladerf::RxSync::new(&device);

        eprintln!(
            "bladeRF: configured TX+RX  freq={:.3} MHz  rate={} Hz  bw={} Hz",
            freq_hz / 1e6,
            actual_rate,
            bandwidth_hz,
        );

        Ok(BladeRfBackend {
            device,
            tx_sync,
            rx_sync,
            channel,
            sample_rate: actual_rate as u64,
            stream_timeout_ms,
            tx_burst_active: false,
        })
    }
}

#[cfg(feature = "bladerf-backend")]
impl CalibrationBackend for BladeRfBackend {
    fn tick_rate(&self) -> u64 {
        // bladeRF timestamps are sample counts.
        self.sample_rate
    }

    fn get_time(&mut self) -> Result<u64, Box<dyn std::error::Error>> {
        self.device
            .get_timestamp(0) // 0 = RX direction
            .map_err(|e| format!("bladeRF: get_timestamp: {}", e).into())
    }

    fn set_time(&mut self, _ticks: u64) -> Result<(), Box<dyn std::error::Error>> {
        // bladeRF timestamps are driven by the FPGA counter and reset when
        // modules are enabled; there is no host-side set_time API.
        Ok(())
    }

    fn activate_tx(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        use bladerf::device::tx_channel;
        self.device
            .enable_module(tx_channel(self.channel), true)
            .map_err(|e| format!("bladeRF: enable TX: {}", e))?;
        self.tx_burst_active = false;
        Ok(())
    }

    fn activate_rx(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        use bladerf::device::rx_channel;
        self.device
            .enable_module(rx_channel(self.channel), true)
            .map_err(|e| format!("bladeRF: enable RX: {}", e))?;
        Ok(())
    }

    fn deactivate_tx(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        use bladerf::device::tx_channel;
        use bladerf::stream::StreamMeta;
        // Close any open burst before disabling the module.
        if self.tx_burst_active {
            let zero = [bladerf::stream::Sc16Q11 { i: 0, q: 0 }; 1];
            let mut meta = StreamMeta {
                flags: Self::TX_BURST_END | Self::TX_NOW,
                ..Default::default()
            };
            let _ = self
                .tx_sync
                .send(&zero, Some(&mut meta), self.stream_timeout_ms);
            self.tx_burst_active = false;
        }
        self.device
            .enable_module(tx_channel(self.channel), false)
            .map_err(|e| format!("bladeRF: disable TX: {}", e))?;
        Ok(())
    }

    fn deactivate_rx(&mut self) -> Result<(), Box<dyn std::error::Error>> {
        use bladerf::device::rx_channel;
        self.device
            .enable_module(rx_channel(self.channel), false)
            .map_err(|e| format!("bladeRF: disable RX: {}", e))?;
        Ok(())
    }

    fn send_timed(
        &mut self,
        samples: &[Complex32],
        time_ticks: Option<u64>,
        end_burst: bool,
    ) -> Result<(), Box<dyn std::error::Error>> {
        use bladerf::stream::{Sc16Q11, StreamMeta};

        let sc16: Vec<Sc16Q11> = samples
            .iter()
            .map(|s| Sc16Q11::from_complex32(*s))
            .collect();

        let mut flags = 0u32;
        if !self.tx_burst_active {
            flags |= Self::TX_BURST_START;
            if let Some(ts) = time_ticks {
                flags |= Self::TX_UPDATE_TS;
                let mut meta = StreamMeta {
                    timestamp: ts,
                    flags,
                    ..Default::default()
                };
                if end_burst {
                    meta.flags |= Self::TX_BURST_END;
                } else {
                    self.tx_burst_active = true;
                }
                self.tx_sync
                    .send(&sc16, Some(&mut meta), self.stream_timeout_ms)
                    .map_err(|e| format!("bladeRF: TX send: {}", e))?;
                return Ok(());
            } else {
                flags |= Self::TX_NOW;
                self.tx_burst_active = true;
            }
        }

        let mut meta = StreamMeta {
            flags: if end_burst {
                flags | Self::TX_BURST_END
            } else {
                flags
            },
            ..Default::default()
        };
        if end_burst {
            self.tx_burst_active = false;
        }
        self.tx_sync
            .send(&sc16, Some(&mut meta), self.stream_timeout_ms)
            .map_err(|e| format!("bladeRF: TX send: {}", e))?;
        Ok(())
    }

    fn recv(
        &mut self,
        buf: &mut [Complex32],
        timeout_us: i64,
    ) -> Result<(usize, u64, bool), Box<dyn std::error::Error>> {
        use bladerf::stream::{Sc16Q11, StreamMeta};

        let timeout_ms = (timeout_us / 1000).max(1) as u32;
        let mut sc16_buf = vec![Sc16Q11::default(); buf.len()];
        let mut meta = StreamMeta {
            flags: Self::RX_NOW,
            ..Default::default()
        };

        match self.rx_sync.recv(&mut sc16_buf, &mut meta, timeout_ms) {
            Ok(n) => {
                for i in 0..n.min(buf.len()) {
                    buf[i] = sc16_buf[i].to_complex32();
                }
                let overflow = (meta.status & Self::STATUS_OVERRUN) != 0;
                Ok((n, meta.timestamp, overflow))
            }
            Err(e) => Err(format!("bladeRF: RX recv: {}", e).into()),
        }
    }
}

/// Linear FM chirp sweeping ±0.45*sample_rate/2 across `len` samples,
/// Hann-windowed to suppress sidelobes. Returned signal is zero-mean so the
/// matched filter is immune to DC/LO leakage in the RX.
fn generate_chirp(len: usize, sample_rate_hz: f64) -> Vec<Complex32> {
    let bw = sample_rate_hz * 0.45;
    let t_total = len as f64 / sample_rate_hz;
    let k = bw / t_total; // sweep rate Hz/s
    let f0 = -bw / 2.0;
    let mut out = Vec::with_capacity(len);
    for n in 0..len {
        let t = n as f64 / sample_rate_hz;
        let phase = 2.0 * std::f64::consts::PI * (f0 * t + 0.5 * k * t * t);
        let w = if len > 1 {
            0.5 - 0.5 * (2.0 * std::f64::consts::PI * n as f64 / (len as f64 - 1.0)).cos()
        } else {
            1.0
        };
        out.push(Complex32::new(
            (w * phase.cos()) as f32,
            (w * phase.sin()) as f32,
        ));
    }
    // Subtract mean to make the template DC-free.
    let (mean_re, mean_im) = out.iter().fold((0.0f64, 0.0f64), |(re, im), c| {
        (re + c.re as f64, im + c.im as f64)
    });
    let mean = Complex32::new((mean_re / len as f64) as f32, (mean_im / len as f64) as f32);
    for s in out.iter_mut() {
        s.re -= mean.re;
        s.im -= mean.im;
    }
    out
}

/// Subtract the complex mean from a buffer in-place. Removes the DC/LO
/// leakage that otherwise rides at the same level under every matched-filter
/// offset and crushes the peak/sidelobe ratio.
fn remove_dc(samples: &mut [Complex32]) {
    if samples.is_empty() {
        return;
    }
    let (re, im) = samples.iter().fold((0.0f64, 0.0f64), |(re, im), c| {
        (re + c.re as f64, im + c.im as f64)
    });
    let mean = Complex32::new(
        (re / samples.len() as f64) as f32,
        (im / samples.len() as f64) as f32,
    );
    for s in samples.iter_mut() {
        s.re -= mean.re;
        s.im -= mean.im;
    }
}

/// Magnitude of the matched-filter output at one offset.
#[inline]
fn mf_mag(rx: &[Complex32], chirp: &[Complex32], offset: usize) -> f32 {
    let mut re = 0.0f32;
    let mut im = 0.0f32;
    for k in 0..chirp.len() {
        let r = rx[offset + k];
        let c = chirp[k];
        // r * conj(c)
        re += r.re * c.re + r.im * c.im;
        im += r.im * c.re - r.re * c.im;
    }
    (re * re + im * im).sqrt()
}

/// Sweep the matched filter over `rx` and return
/// `(peak_idx, peak_mag, noise_mean, noise_std)`. Noise stats are computed
/// from all matched-filter output magnitudes excluding a guard zone around
/// the peak so a real chirp doesn't pollute the noise estimate.
fn matched_filter_peak(rx: &[Complex32], chirp: &[Complex32]) -> Option<(usize, f32, f32, f32)> {
    if rx.len() < chirp.len() {
        return None;
    }
    let n_outputs = rx.len() - chirp.len() + 1;
    let mut mags = Vec::with_capacity(n_outputs);
    let mut max_mag = 0.0f32;
    let mut max_idx = 0usize;
    for i in 0..n_outputs {
        let mag = mf_mag(rx, chirp, i);
        mags.push(mag);
        if mag > max_mag {
            max_mag = mag;
            max_idx = i;
        }
    }
    // Exclude ±chirp_len around the peak for noise statistics — the matched
    // filter response of a real chirp has a width of order chirp_len.
    let guard = chirp.len();
    let lo = max_idx.saturating_sub(guard);
    let hi = (max_idx + guard).min(mags.len());
    let mut noise: Vec<f32> = Vec::with_capacity(mags.len() - (hi - lo));
    noise.extend_from_slice(&mags[..lo]);
    noise.extend_from_slice(&mags[hi..]);
    if noise.is_empty() {
        return Some((max_idx, max_mag, 0.0, 1.0));
    }
    let mean = noise.iter().map(|m| *m as f64).sum::<f64>() / noise.len() as f64;
    let var = noise
        .iter()
        .map(|m| (*m as f64 - mean).powi(2))
        .sum::<f64>()
        / noise.len() as f64;
    let std = var.sqrt().max(1e-9);
    Some((max_idx, max_mag, mean as f32, std as f32))
}

/// Parabolic interpolation around the integer peak for sub-sample resolution.
fn parabolic_refine(rx: &[Complex32], chirp: &[Complex32], peak: usize) -> f64 {
    if peak == 0 || peak + chirp.len() >= rx.len() {
        return peak as f64;
    }
    let y_m1 = mf_mag(rx, chirp, peak - 1) as f64;
    let y_0 = mf_mag(rx, chirp, peak) as f64;
    let y_p1 = mf_mag(rx, chirp, peak + 1) as f64;
    let denom = y_m1 - 2.0 * y_0 + y_p1;
    if denom.abs() < 1e-9 {
        return peak as f64;
    }
    let delta = 0.5 * (y_m1 - y_p1) / denom;
    peak as f64 + delta
}

/// Convert milliseconds to ticks at the given tick rate.
fn ms_to_ticks(ms: u64, tick_rate: u64) -> u64 {
    ms * tick_rate / 1000
}

/// Convert a tick delta to nanoseconds.
fn ticks_to_ns(ticks: i64, tick_rate: u64) -> i64 {
    (ticks as i128 * 1_000_000_000i128 / tick_rate as i128) as i64
}

/// Convert a tick delta to sample-rate samples.
fn ticks_to_samples(ticks: i64, tick_rate: u64, sample_rate_hz: u64) -> f64 {
    ticks as f64 * sample_rate_hz as f64 / tick_rate as f64
}

fn run_round(
    round: usize,
    cli: &Cli,
    backend: &mut dyn CalibrationBackend,
    burst: &[Complex32],
    chirp: &[Complex32],
) -> Result<Option<(i64, f64)>, Box<dyn std::error::Error>> {
    let tick_rate = backend.tick_rate();
    let t_now = backend.get_time()? as i64;
    let t_tx = t_now + ms_to_ticks(cli.tx_lead_ms, tick_rate) as i64;

    // Schedule the burst. end_burst=true tells the driver to flush the (small)
    // burst out the USB pipe instead of buffering for more samples.
    backend.send_timed(burst, Some(t_tx as u64), true)?;

    // Capture RX blocks until we are well past t_tx.
    let target_end = t_tx + ms_to_ticks(cli.listen_ms, tick_rate) as i64;
    let mut blocks: Vec<(i64, Vec<Complex32>)> = Vec::new();
    let mut rx_buf = vec![Complex32::new(0.0, 0.0); 8192];
    loop {
        let (n, block_t_u64, overflow) = backend.recv(&mut rx_buf, 500_000)?;
        if overflow {
            eprintln!("round {round}: RX overflow, retrying round");
            return Ok(None);
        }
        if n == 0 {
            continue;
        }
        let block_t = block_t_u64 as i64;
        blocks.push((block_t, rx_buf[..n].to_vec()));
        if block_t >= target_end {
            break;
        }
        if blocks.len() > 4096 {
            return Err("calibration capture overran block budget".into());
        }
    }

    // Find the first block whose end timestamp is at or after t_tx.
    let first = blocks.iter().position(|(t, b)| {
        let block_end = *t + (b.len() as i64 * tick_rate as i64 / cli.sample_rate_hz as i64);
        block_end >= t_tx
    });
    let first = match first {
        Some(i) => i,
        None => {
            eprintln!("round {round}: no RX block reached t_tx");
            return Ok(None);
        }
    };
    let (anchor_t, _) = blocks[first];
    let mut samples: Vec<Complex32> = Vec::new();
    for (_, b) in blocks.iter().skip(first) {
        samples.extend_from_slice(b);
    }
    // Strip RX DC/LO leakage so it doesn't dominate the matched-filter output.
    remove_dc(&mut samples);

    // Predict where t_tx falls inside `samples`, then search ±5 ms around it.
    // Convert tick delta to sample offset: (t_tx - anchor_t) * sample_rate / tick_rate
    let expected_offset =
        ((t_tx - anchor_t) as i128 * cli.sample_rate_hz as i128 / tick_rate as i128) as i64;
    if expected_offset < 0 {
        eprintln!("round {round}: anchor block is after t_tx");
        return Ok(None);
    }
    let (search_start, search_end_inclusive) = if cli.full_search {
        let max_start = samples.len().saturating_sub(chirp.len());
        (0usize, max_start)
    } else {
        let guard = (cli.sample_rate_hz as i64 / 1000) * 5; // ±5 ms
        let s = (expected_offset - guard).max(0) as usize;
        let e = ((expected_offset + guard) as usize).min(samples.len().saturating_sub(chirp.len()));
        (s, e)
    };
    if search_end_inclusive <= search_start {
        eprintln!("round {round}: search window empty");
        return Ok(None);
    }
    let window_end = search_end_inclusive + chirp.len();
    let window = &samples[search_start..window_end.min(samples.len())];

    let (peak_local, peak_mag, noise_mean, noise_std) = match matched_filter_peak(window, chirp) {
        Some(t) => t,
        None => {
            eprintln!("round {round}: window shorter than chirp");
            return Ok(None);
        }
    };
    // SNR as a z-score: how many noise standard deviations above the noise
    // floor the peak sits. 6 dB of "z" is roughly 2x noise std, very loose.
    let z = (peak_mag - noise_mean) / noise_std;
    let snr_db = 20.0 * (z as f64).max(1e-9).log10();
    let peak_offset_in_samples = search_start as f64 + peak_local as f64;
    // Convert sample offset back to a tick delta, then to ns.
    let peak_tick_delta =
        (peak_offset_in_samples * tick_rate as f64 / cli.sample_rate_hz as f64).round() as i64;
    let peak_hw_tick_unrefined = anchor_t + peak_tick_delta;
    let raw_delay_ticks = peak_hw_tick_unrefined - t_tx;
    let raw_delay_ns = ticks_to_ns(raw_delay_ticks, tick_rate);
    let raw_delay_samples = ticks_to_samples(raw_delay_ticks, tick_rate, cli.sample_rate_hz as u64);

    if snr_db < cli.min_snr_db {
        eprintln!(
            "round {round}: rejected (SNR {snr_db:.2} dB < {:.1}); peak={peak_mag:.4} noise_mean={noise_mean:.4} noise_std={noise_std:.4} z={z:.2} at offset {peak_local} ({raw_delay_ns} ns / {raw_delay_samples:.2} samples from t_tx); search_window=[{search_start}..{search_end_inclusive}] capture={} samples",
            cli.min_snr_db,
            samples.len()
        );
        return Ok(None);
    }
    let refined = parabolic_refine(window, chirp, peak_local);
    let peak_offset_in_samples = search_start as f64 + refined;
    let peak_tick_delta =
        (peak_offset_in_samples * tick_rate as f64 / cli.sample_rate_hz as f64).round() as i64;
    let peak_hw_tick = anchor_t + peak_tick_delta;
    let delay_ticks = peak_hw_tick - t_tx;
    let delay_ns = ticks_to_ns(delay_ticks, tick_rate);
    let delay_samples = ticks_to_samples(delay_ticks, tick_rate, cli.sample_rate_hz as u64);
    eprintln!(
        "round {round}: SNR {snr_db:.1} dB, delay = {delay_ns} ns ({delay_samples:.2} samples)"
    );
    Ok(Some((delay_ns, snr_db)))
}

/// Create the backend based on the `--backend` flag and resolved configuration.
fn create_backend(
    backend_name: &str,
    device_str: &str,
    channel: usize,
    tx_antenna: &str,
    rx_antenna: &str,
    cli: &Cli,
    loaded: &Option<LoadedConfig>,
) -> Result<Box<dyn CalibrationBackend>, Box<dyn std::error::Error>> {
    match backend_name {
        "soapy" => {
            let b = SoapyBackend::new(
                device_str,
                channel,
                tx_antenna,
                rx_antenna,
                cli.tx_freq_hz as f64,
                cli.sample_rate_hz as f64,
                cli.bandwidth_hz as f64,
                cli.tx_gain_db,
                cli.rx_gain_db,
            )?;
            Ok(Box::new(b))
        }
        #[cfg(feature = "uhd-backend")]
        "uhd" => {
            let mcr = loaded
                .as_ref()
                .and_then(|c| c.master_clock_rate)
                .unwrap_or(cli.master_clock_rate);
            let b = UhdBackend::new(
                device_str,
                channel,
                tx_antenna,
                rx_antenna,
                cli.tx_freq_hz as f64,
                cli.sample_rate_hz as f64,
                cli.bandwidth_hz as f64,
                cli.tx_gain_db,
                cli.rx_gain_db,
                mcr,
            )?;
            Ok(Box::new(b))
        }
        #[cfg(not(feature = "uhd-backend"))]
        "uhd" => Err("UHD backend not compiled in (enable 'uhd-backend' feature)".into()),
        #[cfg(feature = "lime-backend")]
        "lime" => {
            let oversample = loaded
                .as_ref()
                .and_then(|c| c.oversample)
                .unwrap_or(cli.oversample);
            let b = LimeBackend::new(
                device_str,
                channel,
                tx_antenna,
                rx_antenna,
                cli.tx_freq_hz as f64,
                cli.sample_rate_hz as f64,
                cli.bandwidth_hz as f64,
                cli.tx_gain_db,
                cli.rx_gain_db,
                oversample,
            )?;
            Ok(Box::new(b))
        }
        #[cfg(not(feature = "lime-backend"))]
        "lime" => Err("LimeSDR backend not compiled in (enable 'lime-backend' feature)".into()),
        #[cfg(feature = "bladerf-backend")]
        "bladerf" => {
            let channel = loaded.as_ref().and_then(|c| c.bladerf_channel).unwrap_or(0);
            let fpga_path = loaded
                .as_ref()
                .and_then(|c| c.bladerf_fpga_path.as_deref().map(str::to_string));
            let num_buffers = loaded
                .as_ref()
                .and_then(|c| c.bladerf_num_buffers)
                .unwrap_or(16);
            let buffer_size = loaded
                .as_ref()
                .and_then(|c| c.bladerf_buffer_size)
                .unwrap_or(8192);
            let num_transfers = loaded
                .as_ref()
                .and_then(|c| c.bladerf_num_transfers)
                .unwrap_or(8);
            let stream_timeout_ms = loaded
                .as_ref()
                .and_then(|c| c.bladerf_stream_timeout_ms)
                .unwrap_or(3500);
            let b = BladeRfBackend::new(
                device_str,
                channel,
                tx_antenna,
                rx_antenna,
                cli.tx_freq_hz as f64,
                cli.sample_rate_hz as f64,
                cli.bandwidth_hz as f64,
                cli.tx_gain_db,
                cli.rx_gain_db,
                fpga_path.as_deref(),
                num_buffers,
                buffer_size,
                num_transfers,
                stream_timeout_ms,
            )?;
            Ok(Box::new(b))
        }
        #[cfg(not(feature = "bladerf-backend"))]
        "bladerf" => {
            Err("bladeRF backend not compiled in (enable 'bladerf-backend' feature)".into())
        }
        other => Err(format!(
            "unknown backend '{}'; expected 'soapy', 'uhd', 'lime', or 'bladerf'",
            other
        )
        .into()),
    }
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cli = Cli::parse();

    let loaded = if let Some(path) = &cli.config {
        Some(load_config(path)?)
    } else {
        None
    };

    // Prefer the backend explicitly supplied on the command line; fall back to
    // the kind inferred from the config file so users can just pass --config
    // without also having to specify --backend.
    let backend_name = if cli.backend != "soapy" {
        cli.backend.clone()
    } else if let Some(ref cfg) = loaded {
        cfg.detected_backend.clone()
    } else {
        cli.backend.clone()
    };

    let device_str = cli
        .device
        .clone()
        .or_else(|| loaded.as_ref().map(|c| c.device.clone()))
        .unwrap_or_default();
    let channel = cli.channel;
    let _ = loaded.as_ref().and_then(|c| c.channel); // documented: CLI wins
    let tx_antenna = cli
        .tx_antenna
        .clone()
        .or_else(|| loaded.as_ref().map(|c| c.tx_antenna.clone()))
        .ok_or("--tx-antenna or --config required")?;
    let rx_antenna = cli
        .rx_antenna
        .clone()
        .or_else(|| loaded.as_ref().and_then(|c| c.rx_antenna.clone()))
        .unwrap_or_else(|| "RX2".to_string());

    eprintln!("Backend: {}", backend_name);
    eprintln!("Opening device: {device_str}");
    eprintln!(
        "TX: ant={tx_antenna} freq={} Hz rate={} Hz bw={} Hz gain={} dB",
        cli.tx_freq_hz, cli.sample_rate_hz, cli.bandwidth_hz, cli.tx_gain_db
    );
    eprintln!(
        "RX: ant={rx_antenna} freq={} Hz rate={} Hz bw={} Hz gain={} dB",
        cli.tx_freq_hz, cli.sample_rate_hz, cli.bandwidth_hz, cli.rx_gain_db
    );

    let mut backend = create_backend(
        &backend_name,
        &device_str,
        channel,
        &tx_antenna,
        &rx_antenna,
        &cli,
        &loaded,
    )?;

    eprintln!("Tick rate: {} ticks/s", backend.tick_rate());

    // Reset hardware time to zero.
    backend.set_time(0)?;

    // --- TX-loop diagnostic modes (no RX needed) ---
    if cli.tx_loop || cli.tx_tone_hz.is_some() {
        let block: Vec<Complex32> = if let Some(tone_hz) = cli.tx_tone_hz {
            // Continuous complex sinusoid at +tone_hz baseband, amplitude 0.7.
            // Build one full cycle aligned to the buffer length so the
            // wrap-around is phase-continuous.
            let block_len = 16384usize;
            let amp = 0.7f32;
            let mut v = Vec::with_capacity(block_len);
            for n in 0..block_len {
                let phase =
                    2.0 * std::f64::consts::PI * tone_hz * (n as f64 / cli.sample_rate_hz as f64);
                v.push(Complex32::new(
                    (amp as f64 * phase.cos()) as f32,
                    (amp as f64 * phase.sin()) as f32,
                ));
            }
            eprintln!(
                "tx-loop mode: CW tone at RF {} Hz (baseband {:+} Hz, amp 0.7). Ctrl-C to stop.",
                cli.tx_freq_hz as f64 + tone_hz,
                tone_hz
            );
            v
        } else {
            eprintln!(
                "tx-loop mode: continuously transmitting chirp burst at {} Hz. Ctrl-C to stop.",
                cli.tx_freq_hz
            );
            let chirp = generate_chirp(cli.chirp_len, cli.sample_rate_hz as f64);
            let mut burst = Vec::with_capacity(chirp.len() + cli.tx_pad_samples);
            burst.extend_from_slice(&chirp);
            burst.resize(chirp.len() + cli.tx_pad_samples, Complex32::new(0.0, 0.0));
            burst
        };
        backend.activate_tx()?;
        let mut iters: u64 = 0;
        loop {
            backend.send_timed(&block, None, false)?;
            iters += 1;
            if iters % 100 == 0 {
                eprintln!("tx-loop: {} blocks written", iters);
            }
        }
    }

    // --- Normal calibration mode ---
    backend.activate_rx()?;
    // Prime: drain a few buffers so the hardware clock is advancing.
    let mut prime_buf = vec![Complex32::new(0.0, 0.0); 4096];
    for _ in 0..4 {
        let _ = backend.recv(&mut prime_buf, 200_000);
    }

    backend.activate_tx()?;

    let chirp = generate_chirp(cli.chirp_len, cli.sample_rate_hz as f64);
    // Burst = chirp followed by zero padding so the USB driver flushes it.
    // The chirp lives at offset 0, so t_tx still corresponds to chirp[0].
    let mut burst = Vec::with_capacity(chirp.len() + cli.tx_pad_samples);
    burst.extend_from_slice(&chirp);
    burst.resize(chirp.len() + cli.tx_pad_samples, Complex32::new(0.0, 0.0));

    let mut delays_ns: Vec<i64> = Vec::new();
    let mut drain_buf = vec![Complex32::new(0.0, 0.0); 8192];
    for round in 0..cli.repeats {
        match run_round(round, &cli, backend.as_mut(), &burst, &chirp)? {
            Some((delay_ns, _snr)) => delays_ns.push(delay_ns),
            None => {}
        }
        // Drain stale RX samples between rounds to prevent overflow.
        for _ in 0..50 {
            let _ = backend.recv(&mut drain_buf, 1_000);
        }
        thread::sleep(Duration::from_millis(20));
        // Drain again after the sleep.
        for _ in 0..50 {
            let _ = backend.recv(&mut drain_buf, 1_000);
        }
    }

    backend.deactivate_tx()?;
    backend.deactivate_rx()?;

    if delays_ns.is_empty() {
        return Err("no successful calibration rounds".into());
    }

    delays_ns.sort();
    let trim = if delays_ns.len() >= 4 { 1 } else { 0 };
    let core = &delays_ns[trim..delays_ns.len() - trim];
    let median_ns = core[core.len() / 2];
    let mean_ns: f64 = core.iter().map(|d| *d as f64).sum::<f64>() / core.len() as f64;
    let var: f64 = core
        .iter()
        .map(|d| (*d as f64 - mean_ns).powi(2))
        .sum::<f64>()
        / core.len() as f64;
    let stddev_ns = var.sqrt();
    let median_samples = (median_ns as f64 * cli.sample_rate_hz as f64 / 1e9).round() as i64;
    let stddev_samples = stddev_ns * cli.sample_rate_hz as f64 / 1e9;

    if cli.json {
        println!(
            "{{\"rx_delay_ns\":{median_ns},\"rx_sample_delay\":{median_samples},\"stddev_samples\":{stddev_samples:.3},\"rounds\":{}}}",
            core.len()
        );
    } else {
        println!();
        println!(
            "Median RX delay: {median_ns} ns ({median_samples} samples)  stddev: {stddev_samples:.2} samples  rounds: {}",
            core.len()
        );
        println!("Add to your radio config:");
        println!("    \"rx_sample_delay\": {median_samples}");
    }

    Ok(())
}
