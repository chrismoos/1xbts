//! BTS node configuration.
//!
//! `BtsNodeConfig` is loaded from `config/bts.json`. This is the
//! standalone BTS-side configuration — it carries radio hardware setup,
//! BTS runtime settings, and the BTS-owned half of the Abis timers
//! (A.S0003-A §8 Table 8-1).

use std::path::Path;

use std::net::SocketAddr;

use cdma_common::error::Error;
use serde::{Deserialize, Serialize};

use super::settings::BtsRuntimeSettings;

fn default_rx_freq_hz() -> usize {
    836_520_000
}

fn default_rx_sample_rate_hz() -> usize {
    1_228_800 * 4
}

fn default_rx_batch_pcgs() -> usize {
    2
}

fn default_tx_gain_db() -> f64 {
    60.0
}

fn default_uhd_master_clock_rate() -> u64 {
    49_152_000
}

fn default_lime_tx_antenna() -> String {
    "BAND1".to_string()
}

fn default_lime_tx_gain_db() -> u32 {
    60
}

fn default_bladerf_tx_gain_db() -> i32 {
    60
}

fn default_bladerf_tx_antenna() -> Option<String> {
    Some("TXA".to_string())
}

fn default_bladerf_rx_antenna() -> Option<String> {
    Some("B_BALANCED".to_string())
}

/// Radio backend selection plus per-backend hardware parameters used by the
/// BTS to construct an SDR transmit/receive pipeline at startup.
///
/// Variants are tagged by `kind` in JSON (`"file_output"`, `"noop"`,
/// `"soapy"`, `"uhd"`, `"lime"`). Each backend variant carries the device
/// addressing fields, RF parameters, and stream tuning knobs needed by the
/// corresponding `cdma_bts::sdr` implementation.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
pub enum RadioConfig {
    /// Write the TX baseband stream to a WAV file. Useful for offline
    /// generation and tests; provides no RX.
    FileOutput {
        /// Output path for the captured TX baseband.
        path: String,
    },
    /// No-op radio: drops TX, provides no RX. Used when the binary needs
    /// to bring up the rest of the stack without touching real hardware.
    Noop,
    /// SoapySDR-backed radio (e.g., LimeSDR via Soapy, generic Soapy
    /// devices). Supports shared TX/RX on a single device.
    Soapy {
        device: String,
        channel: usize,
        antenna: String,
        #[serde(default = "default_tx_gain_db")]
        tx_gain_db: f64,
        #[serde(default)]
        rx_antenna: Option<String>,
        #[serde(default)]
        rx_gain_db: Option<f64>,
        #[serde(default = "default_rx_freq_hz")]
        rx_freq_hz: usize,
        #[serde(default = "default_rx_sample_rate_hz")]
        rx_sample_rate_hz: usize,
        #[serde(default)]
        rx_bandwidth_hz: Option<usize>,
        #[serde(default)]
        rx_reference_dbm: Option<f64>,
        #[serde(default)]
        rx_sample_delay: i64,
        #[serde(default = "default_rx_batch_pcgs")]
        rx_batch_pcgs: usize,
        #[serde(default)]
        traffic_rx_continuity: bool,
    },
    Uhd {
        device: String,
        channel: usize,
        antenna: String,
        #[serde(default = "default_tx_gain_db")]
        tx_gain_db: f64,
        #[serde(default = "default_uhd_master_clock_rate")]
        master_clock_rate: u64,
        #[serde(default)]
        clock_source: Option<String>,
        #[serde(default)]
        time_source: Option<String>,
        #[serde(default)]
        rx_antenna: Option<String>,
        #[serde(default)]
        rx_gain_db: Option<f64>,
        #[serde(default = "default_rx_freq_hz")]
        rx_freq_hz: usize,
        #[serde(default = "default_rx_sample_rate_hz")]
        rx_sample_rate_hz: usize,
        #[serde(default)]
        rx_bandwidth_hz: Option<usize>,
        #[serde(default)]
        rx_reference_dbm: Option<f64>,
        #[serde(default)]
        rx_sample_delay: i64,
        #[serde(default = "default_rx_batch_pcgs")]
        rx_batch_pcgs: usize,
        #[serde(default)]
        traffic_rx_continuity: bool,
    },
    Lime {
        device: String,
        channel: usize,
        #[serde(default = "default_lime_tx_antenna")]
        tx_antenna: String,
        #[serde(default = "default_lime_tx_gain_db")]
        tx_gain_db: u32,
        #[serde(default)]
        rx_antenna: Option<String>,
        #[serde(default)]
        rx_gain_db: Option<u32>,
        #[serde(default = "default_rx_freq_hz")]
        rx_freq_hz: usize,
        #[serde(default = "default_rx_sample_rate_hz")]
        rx_sample_rate_hz: usize,
        #[serde(default)]
        rx_bandwidth_hz: Option<usize>,
        #[serde(default)]
        rx_reference_dbm: Option<f64>,
        #[serde(default)]
        rx_sample_delay: i64,
        #[serde(default = "default_rx_batch_pcgs")]
        rx_batch_pcgs: usize,
        #[serde(default)]
        traffic_rx_continuity: bool,
        #[serde(default)]
        oversample: Option<usize>,
        #[serde(default)]
        tx_lo_offset_hz: Option<i64>,
        #[serde(default)]
        tx_fifo_size: Option<u32>,
        #[serde(default)]
        rx_fifo_size: Option<u32>,
        #[serde(default)]
        stream_throughput_vs_latency: Option<f32>,
    },
    /// Native libbladeRF backend for bladeRF Micro 2.0 (and bladeRF x40/x115).
    BladeRf {
        #[serde(default)]
        device: String,
        #[serde(default)]
        channel: u32,
        /// Path to FPGA bitstream (.rbf). When null, libbladeRF auto-loads
        /// from ~/.config/Nuand/bladeRF/ or SPI flash.
        #[serde(default)]
        fpga_path: Option<String>,
        /// TX RF port name (e.g. "TXA", "TXB"). Default "TXA".
        #[serde(default = "default_bladerf_tx_antenna")]
        tx_antenna: Option<String>,
        /// RX RF port name (e.g. "A_BALANCED", "B_BALANCED"). Default "B_BALANCED".
        #[serde(default = "default_bladerf_rx_antenna")]
        rx_antenna: Option<String>,
        #[serde(default = "default_bladerf_tx_gain_db")]
        tx_gain_db: i32,
        #[serde(default)]
        rx_gain_db: Option<i32>,
        #[serde(default = "default_rx_freq_hz")]
        rx_freq_hz: usize,
        #[serde(default = "default_rx_sample_rate_hz")]
        rx_sample_rate_hz: usize,
        #[serde(default)]
        rx_bandwidth_hz: Option<usize>,
        #[serde(default)]
        rx_reference_dbm: Option<f64>,
        #[serde(default)]
        rx_sample_delay: i64,
        #[serde(default = "default_rx_batch_pcgs")]
        rx_batch_pcgs: usize,
        #[serde(default)]
        traffic_rx_continuity: bool,
        #[serde(default)]
        tx_lo_offset_hz: Option<i64>,
        #[serde(default)]
        num_buffers: Option<u32>,
        #[serde(default)]
        buffer_size: Option<u32>,
        #[serde(default)]
        num_transfers: Option<u32>,
        #[serde(default)]
        stream_timeout_ms: Option<u32>,
    },
}

impl Default for RadioConfig {
    fn default() -> Self {
        Self::FileOutput {
            path: "iq.wav".to_string(),
        }
    }
}

impl RadioConfig {
    /// RX sample rate in Hz for this radio backend, falling back to the
    /// project default when the variant has no RX (e.g., `FileOutput`,
    /// `Noop`).
    pub fn rx_sample_rate_hz(&self) -> usize {
        match self {
            Self::Soapy {
                rx_sample_rate_hz, ..
            }
            | Self::Uhd {
                rx_sample_rate_hz, ..
            }
            | Self::Lime {
                rx_sample_rate_hz, ..
            }
            | Self::BladeRf {
                rx_sample_rate_hz, ..
            } => *rx_sample_rate_hz,
            _ => default_rx_sample_rate_hz(),
        }
    }

    /// Inherent RX pipeline delay in samples (0 when unconfigured or for
    /// non-RX variants). Subtracted from the hardware-time → absolute-sample
    /// mapping so the chip number assigned to each received sample matches
    /// when it was actually transmitted on air.
    pub fn rx_sample_delay(&self) -> i64 {
        match self {
            Self::Soapy {
                rx_sample_delay, ..
            }
            | Self::Uhd {
                rx_sample_delay, ..
            }
            | Self::Lime {
                rx_sample_delay, ..
            }
            | Self::BladeRf {
                rx_sample_delay, ..
            } => *rx_sample_delay,
            _ => 0,
        }
    }

    /// Number of PCGs (1536 chips each) per RX read batch. Lower values
    /// reduce power-control latency jitter at the cost of more USB reads
    /// per second. Default 2 for non-RX variants.
    pub fn rx_batch_pcgs(&self) -> usize {
        match self {
            Self::Soapy { rx_batch_pcgs, .. }
            | Self::Uhd { rx_batch_pcgs, .. }
            | Self::Lime { rx_batch_pcgs, .. }
            | Self::BladeRf { rx_batch_pcgs, .. } => *rx_batch_pcgs,
            _ => default_rx_batch_pcgs(),
        }
    }

    /// Whether the radio backend should keep the traffic-RX pipeline
    /// continuous across PCG boundaries (vs gating between PCGs). False
    /// for non-RX variants.
    pub fn traffic_rx_continuity(&self) -> bool {
        match self {
            Self::Soapy {
                traffic_rx_continuity,
                ..
            }
            | Self::Uhd {
                traffic_rx_continuity,
                ..
            }
            | Self::Lime {
                traffic_rx_continuity,
                ..
            }
            | Self::BladeRf {
                traffic_rx_continuity,
                ..
            } => *traffic_rx_continuity,
            _ => false,
        }
    }

    /// Reference dBm offset for converting relative dB to absolute dBm
    /// (the absolute power that corresponds to 0 dB full-scale at the
    /// ADC). `None` when unconfigured (no calibration data).
    pub fn rx_reference_dbm(&self) -> Option<f64> {
        match self {
            Self::Soapy {
                rx_reference_dbm, ..
            }
            | Self::Uhd {
                rx_reference_dbm, ..
            }
            | Self::Lime {
                rx_reference_dbm, ..
            }
            | Self::BladeRf {
                rx_reference_dbm, ..
            } => *rx_reference_dbm,
            _ => None,
        }
    }

    /// Hardware-time tick rate (ticks per second). UHD uses the configured
    /// master clock rate; Lime uses the sample rate (timestamps are sample
    /// counts); everything else uses 1 GHz (nanoseconds).
    pub fn tick_rate(&self) -> u64 {
        match self {
            Self::Uhd {
                master_clock_rate, ..
            } => *master_clock_rate,
            Self::Lime {
                rx_sample_rate_hz, ..
            }
            | Self::BladeRf {
                rx_sample_rate_hz, ..
            } => *rx_sample_rate_hz as u64,
            _ => 1_000_000_000,
        }
    }
}

/// BTS-owned Abis timers per A.S0003-A §8 Table 8-1.
///
/// All values in milliseconds. Granularity is 100 ms; ranges are 0–1000 ms
/// except where noted. Configurable per BTS within the spec range.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct BtsAbisTimers {
    /// §8.1 — BTS-side timer for `Abis-Connect`. Default 100 ms.
    pub tconnb_ms: u64,
    /// §8.4 — BTS-side timer for `Abis-Remove Ack`. Default 100 ms.
    pub tdisconb_ms: u64,
    /// §8.7 — BTS-side timer for `Abis-Burst Commit`. Default 500 ms.
    pub tbstcomb_ms: u64,
    /// §8.8 — BTS-side timer for `Abis-BTS Release`. Default 100 ms.
    pub trelreqb_ms: u64,
}

impl Default for BtsAbisTimers {
    fn default() -> Self {
        Self {
            tconnb_ms: 100,
            tdisconb_ms: 100,
            tbstcomb_ms: 500,
            trelreqb_ms: 100,
        }
    }
}

fn default_bts_bearer_bind_addr() -> SocketAddr {
    SocketAddr::new(
        std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        cdma_abis::transport::ABIS_BTS_BEARER_PORT,
    )
}

fn default_bts_bearer_remote_addr() -> SocketAddr {
    SocketAddr::new(
        std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        cdma_abis::transport::ABIS_BSC_BEARER_PORT,
    )
}

fn default_bts_abis_bind_addr() -> SocketAddr {
    SocketAddr::new(
        std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        cdma_abis::transport::ABIS_SIGNALING_PORT,
    )
}

/// BTS-side Abis signaling (TCP) addressing.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct BtsAbisConfig {
    /// Local TCP address for the BTS Abis signaling listener. Default `127.0.0.1:5604`.
    #[serde(default = "default_bts_abis_bind_addr")]
    pub bind_addr: SocketAddr,
}

impl Default for BtsAbisConfig {
    fn default() -> Self {
        Self {
            bind_addr: default_bts_abis_bind_addr(),
        }
    }
}

/// BTS-side Abis bearer (UDP) addressing.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct BtsBearerConfig {
    /// Local UDP address for the BTS bearer transport. Default `127.0.0.1:17014`.
    #[serde(default = "default_bts_bearer_bind_addr")]
    pub bind_addr: SocketAddr,
    /// Remote BSC bearer address. Default `127.0.0.1:17013`.
    #[serde(default = "default_bts_bearer_remote_addr")]
    pub remote_addr: SocketAddr,
}

impl Default for BtsBearerConfig {
    fn default() -> Self {
        Self {
            bind_addr: default_bts_bearer_bind_addr(),
            remote_addr: default_bts_bearer_remote_addr(),
        }
    }
}

/// Standalone BTS node configuration (loaded from `config/bts.json`).
///
/// Carries everything needed to bring up the BTS in isolation: radio
/// hardware setup, BTS PHY/MAC/LAC runtime settings, the BTS pilot PN
/// offset, and the BTS-side Abis timers.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct BtsNodeConfig {
    /// SDR backend selection and per-backend hardware parameters.
    pub radio: RadioConfig,
    /// Pilot PN offset (chips, in units of 64). Must be in `0..=511`.
    pub pilot_offset: usize,
    /// BTS PHY/MAC/LAC runtime settings (sample rates, downlink/uplink
    /// channel parameters, overhead scheduling, etc.).
    pub runtime: BtsRuntimeSettings,
    /// BTS-side Abis timers per A.S0003-A §8 Table 8-1.
    pub abis_timers: BtsAbisTimers,
    /// BTS-side Abis signaling (TCP) addressing.
    pub abis: BtsAbisConfig,
    /// Cell-level overhead parameters for the sync and paging channels.
    /// The BTS generates the overhead train locally from these values.
    pub overhead: super::settings::OverheadParameters,
    /// Source for the broadcast `LTM_OFF` / `DAYLT` / `LP_SEC` fields.
    /// When absent, the static overhead values are used (legacy behavior).
    pub timezone: cdma_common::timezone::TimezoneConfig,
    /// Abis bearer (UDP) addressing for traffic frames.
    pub bearer: BtsBearerConfig,
}

impl Default for BtsNodeConfig {
    fn default() -> Self {
        Self {
            radio: RadioConfig::default(),
            pilot_offset: 0,
            runtime: BtsRuntimeSettings::default(),
            abis_timers: BtsAbisTimers::default(),
            abis: BtsAbisConfig::default(),
            overhead: super::settings::OverheadParameters::default(),
            timezone: cdma_common::timezone::TimezoneConfig::default(),
            bearer: BtsBearerConfig::default(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default)]
struct BtsNodeConfigFile {
    pub radio: Option<RadioConfig>,
    pub radio_config_path: Option<String>,
    pub pilot_offset: usize,
    pub runtime: BtsRuntimeSettings,
    pub abis_timers: BtsAbisTimers,
    pub abis: BtsAbisConfig,
    pub overhead: super::settings::OverheadParameters,
    pub timezone: cdma_common::timezone::TimezoneConfig,
    pub bearer: BtsBearerConfig,
}

impl Default for BtsNodeConfigFile {
    fn default() -> Self {
        Self {
            radio: None,
            radio_config_path: None,
            pilot_offset: 0,
            runtime: BtsRuntimeSettings::default(),
            abis_timers: BtsAbisTimers::default(),
            abis: BtsAbisConfig::default(),
            overhead: super::settings::OverheadParameters::default(),
            timezone: cdma_common::timezone::TimezoneConfig::default(),
            bearer: BtsBearerConfig::default(),
        }
    }
}

impl BtsNodeConfig {
    /// Load and validate a `BtsNodeConfig` from a JSON file. The radio
    /// section may be inlined as `radio` or referenced via
    /// `radio_config_path` (relative paths resolve against the parent of
    /// `path`); specifying both is an error.
    pub fn load_from_path(path: &Path) -> Result<Self, Error> {
        let merged = cdma_common::config_load::load_json_with_local_override(path)?;
        let source: BtsNodeConfigFile = serde_json::from_value(merged)?;
        let radio = match (source.radio, source.radio_config_path.as_deref()) {
            (Some(_), Some(_)) => {
                return Err("config must specify only one of radio or radio_config_path".into());
            }
            (Some(radio), None) => radio,
            (None, Some(radio_config_path)) => load_radio_config(path, radio_config_path)?,
            (None, None) => RadioConfig::default(),
        };
        let config = BtsNodeConfig {
            radio,
            pilot_offset: source.pilot_offset,
            runtime: source.runtime,
            abis_timers: source.abis_timers,
            abis: source.abis,
            overhead: source.overhead,
            timezone: source.timezone,
            bearer: source.bearer,
        };
        config.validate()?;
        Ok(config)
    }

    /// Validate self-contained BTS invariants. Cross-node validation
    /// (e.g. matching `page_chan` against the BSC's overhead config) is
    /// done in bootstrap, not here.
    pub fn validate(&self) -> Result<(), Error> {
        if self.pilot_offset > 511 {
            return Err("bts.pilot_offset must be in 0..=511".into());
        }
        self.runtime.validate()?;
        cdma_common::timezone::validate(&self.timezone)
            .map_err(|e| Error::from(format!("bts.timezone: {e}")))?;
        Ok(())
    }
}

/// Load a standalone radio JSON file (without surrounding `BtsNodeConfig`
/// fields). Used by the CLI to override the radio section without rewriting
/// `bts.json`.
pub fn load_radio_from_path(path: &Path) -> Result<RadioConfig, Error> {
    let merged = cdma_common::config_load::load_json_with_local_override(path)?;
    let radio: RadioConfig = serde_json::from_value(merged)?;
    Ok(radio)
}

fn load_radio_config(config_path: &Path, radio_config_path: &str) -> Result<RadioConfig, Error> {
    let radio_path = Path::new(radio_config_path);
    let resolved_path = if radio_path.is_absolute() {
        radio_path.to_path_buf()
    } else {
        config_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join(radio_path)
    };
    load_radio_from_path(&resolved_path)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        path::PathBuf,
        time::{SystemTime, UNIX_EPOCH},
    };

    fn temp_test_dir(name: &str) -> PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time went backwards")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!("cdma-bts-config-{name}-{unique}"));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn loads_radio_from_relative_config_path() {
        let dir = temp_test_dir("radio-relative");
        let radio_path = dir.join("radio_limesdr.json");
        let config_path = dir.join("bts.json");

        fs::write(
            &radio_path,
            r#"{
  "kind": "soapy",
  "device": "driver=lime",
  "channel": 0,
  "antenna": "BAND1",
  "tx_gain_db": 80.0,
  "rx_antenna": "LNAW",
  "rx_gain_db": 45.0,
  "rx_freq_hz": 836520000,
  "rx_sample_rate_hz": 4915200,
  "rx_bandwidth_hz": 2500000,
  "rx_reference_dbm": null
}
"#,
        )
        .expect("write radio config");

        fs::write(
            &config_path,
            r#"{ "radio_config_path": "radio_limesdr.json" }"#,
        )
        .expect("write bts config");

        let config = BtsNodeConfig::load_from_path(&config_path).expect("load config");
        match config.radio {
            RadioConfig::Soapy { device, .. } => assert_eq!(device, "driver=lime"),
            other => panic!("expected soapy radio, got {other:?}"),
        }
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn loads_inline_radio_config() {
        let dir = temp_test_dir("radio-inline");
        let config_path = dir.join("bts.json");

        fs::write(
            &config_path,
            r#"{
  "radio": { "kind": "noop" }
}
"#,
        )
        .expect("write bts config");

        let config = BtsNodeConfig::load_from_path(&config_path).expect("load config");
        assert!(matches!(config.radio, RadioConfig::Noop));
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn loads_timezone_overhead_default() {
        use cdma_common::timezone::TimezoneSource;
        let dir = temp_test_dir("tz-default");
        let path = dir.join("bts.json");
        fs::write(&path, r#"{ "radio": { "kind": "noop" } }"#).unwrap();
        let cfg = BtsNodeConfig::load_from_path(&path).expect("load");
        assert_eq!(cfg.timezone.source, TimezoneSource::Overhead);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn loads_timezone_user_block() {
        use cdma_common::timezone::TimezoneSource;
        let dir = temp_test_dir("tz-user");
        let path = dir.join("bts.json");
        fs::write(
            &path,
            r#"{
  "radio": { "kind": "noop" },
  "timezone": { "source": "user", "tz": "America/Los_Angeles" }
}"#,
        )
        .unwrap();
        let cfg = BtsNodeConfig::load_from_path(&path).expect("load");
        assert_eq!(
            cfg.timezone.source,
            TimezoneSource::User {
                tz: "America/Los_Angeles".into()
            }
        );
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn rejects_user_timezone_with_invalid_iana() {
        let dir = temp_test_dir("tz-bad-iana");
        let path = dir.join("bts.json");
        fs::write(
            &path,
            r#"{
  "radio": { "kind": "noop" },
  "timezone": { "source": "user", "tz": "Mars/Olympus_Mons" }
}"#,
        )
        .unwrap();
        let err = BtsNodeConfig::load_from_path(&path).expect_err("expected validation error");
        assert!(
            err.to_string().contains("invalid IANA timezone"),
            "unexpected error: {err}"
        );
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn rejects_user_timezone_missing_tz_field() {
        let dir = temp_test_dir("tz-missing");
        let path = dir.join("bts.json");
        fs::write(
            &path,
            r#"{
  "radio": { "kind": "noop" },
  "timezone": { "source": "user" }
}"#,
        )
        .unwrap();
        let err = BtsNodeConfig::load_from_path(&path).expect_err("expected parse error");
        assert!(
            err.to_string().contains("required when source"),
            "unexpected error: {err}"
        );
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn rejects_inline_and_external_radio_config_together() {
        let dir = temp_test_dir("radio-both");
        let config_path = dir.join("bts.json");

        fs::write(
            &config_path,
            r#"{
  "radio": { "kind": "noop" },
  "radio_config_path": "radio_limesdr.json"
}
"#,
        )
        .expect("write bts config");

        let err = BtsNodeConfig::load_from_path(&config_path).expect_err("expected config error");
        assert!(
            err.to_string()
                .contains("only one of radio or radio_config_path")
        );
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn abis_timer_defaults_match_spec_table_8_1() {
        let timers = BtsAbisTimers::default();
        assert_eq!(timers.tconnb_ms, 100);
        assert_eq!(timers.tdisconb_ms, 100);
        assert_eq!(timers.tbstcomb_ms, 500);
        assert_eq!(timers.trelreqb_ms, 100);
    }

    #[test]
    fn abis_signaling_default_bind_is_localhost_spec_port() {
        let cfg = BtsNodeConfig::default();
        assert_eq!(cfg.abis.bind_addr, "127.0.0.1:5604".parse().unwrap());
    }

    #[test]
    fn abis_signaling_explicit_bind_deserializes() {
        let cfg: BtsNodeConfig =
            serde_json::from_str(r#"{ "abis": { "bind_addr": "127.0.0.1:5604" } }"#)
                .expect("deserialize bts config");
        assert_eq!(cfg.abis.bind_addr, "127.0.0.1:5604".parse().unwrap());
    }
}
