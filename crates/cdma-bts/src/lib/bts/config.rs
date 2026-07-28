//! BTS node configuration.
//!
//! `BtsNodeConfig` is the operator-facing configuration loaded from
//! `config/bts.json`: radio hardware setup, node-level parameters, and the
//! BTS-owned half of the Abis timers (A.S0003-A §8 Table 8-1). The in-memory
//! runtime and PHY channel settings derived from it live in `super::settings`.

use std::path::Path;

use std::net::SocketAddr;

use cdma_common::error::Error;
use cdma_common::{band_class::ChannelPlan, consts::SR1_CHIP_RATE_HZ};
use serde::{Deserialize, Serialize};

use super::{evdo, settings::BtsRuntimeSettings};

fn default_rx_batch_pcgs() -> usize {
    2
}

fn default_tx_gain_db() -> f64 {
    60.0
}

fn default_uhd_master_clock_rate() -> u64 {
    39_321_600
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

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReverseRxTarget {
    #[default]
    OneX,
    Hrpd,
    Composite,
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
        #[serde(default)]
        rx_reference_dbm: Option<f64>,
        /// Radio-specific dBFS calibration offset applied to reverse-link raw
        /// power-control thresholds. Defaults to 0 dBFS.
        #[serde(default)]
        rx_power_adj: f32,
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
        #[serde(default)]
        rx_reference_dbm: Option<f64>,
        /// Radio-specific dBFS calibration offset applied to reverse-link raw
        /// power-control thresholds. Defaults to 0 dBFS.
        #[serde(default)]
        rx_power_adj: f32,
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
        #[serde(default)]
        rx_reference_dbm: Option<f64>,
        /// Radio-specific dBFS calibration offset applied to reverse-link raw
        /// power-control thresholds. Defaults to 0 dBFS.
        #[serde(default)]
        rx_power_adj: f32,
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
        #[serde(default)]
        rx_reference_dbm: Option<f64>,
        /// Radio-specific dBFS calibration offset applied to reverse-link raw
        /// power-control thresholds. Defaults to 0 dBFS.
        #[serde(default)]
        rx_power_adj: f32,
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
        // With no radio configured, bring up the full stack on the null radio:
        // TX is dropped and a dummy RX feeds silence, so the EV-DO forward link
        // and reverse pipeline run end to end without hardware.
        Self::Noop
    }
}

impl RadioConfig {
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

    /// Radio-specific dBFS calibration offset applied to reverse-link raw
    /// power-control thresholds. Positive values move the thresholds hotter.
    pub fn rx_power_adj(&self) -> f32 {
        match self {
            Self::Soapy { rx_power_adj, .. }
            | Self::Uhd { rx_power_adj, .. }
            | Self::Lime { rx_power_adj, .. }
            | Self::BladeRf { rx_power_adj, .. } => *rx_power_adj,
            _ => 0.0,
        }
    }

    /// Hardware-time tick rate (ticks per second). UHD uses the configured
    /// master clock rate; Lime uses the sample rate (timestamps are sample
    /// counts); everything else uses 1 GHz (nanoseconds).
    pub fn tick_rate(&self, rx_sample_rate_hz: usize) -> u64 {
        match self {
            Self::Uhd {
                master_clock_rate, ..
            } => *master_clock_rate,
            Self::Lime { .. } | Self::BladeRf { .. } => rx_sample_rate_hz as u64,
            _ => 1_000_000_000,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BtsRfProfile {
    pub tx_sample_rate_hz: usize,
    pub tx_bandwidth_hz: usize,
    pub rx_sample_rate_hz: usize,
    pub rx_bandwidth_hz: usize,
}

impl Default for BtsRfProfile {
    fn default() -> Self {
        Self::single_carrier()
    }
}

impl BtsRfProfile {
    pub const SINGLE_CARRIER_BANDWIDTH_HZ: usize = 1_500_000;
    pub const MAX_COMPOSITE_OVERSAMPLE: usize = 16;

    pub fn single_carrier() -> Self {
        let sample_rate_hz = SR1_CHIP_RATE_HZ as usize * 4;
        Self {
            tx_sample_rate_hz: sample_rate_hz,
            tx_bandwidth_hz: Self::SINGLE_CARRIER_BANDWIDTH_HZ,
            rx_sample_rate_hz: sample_rate_hz,
            rx_bandwidth_hz: Self::SINGLE_CARRIER_BANDWIDTH_HZ,
        }
    }

    pub fn derive(channel: ChannelPlan, evdo: &evdo::EvdoConfig) -> Result<Self, Error> {
        if !evdo.enabled || evdo.mode == evdo::EvdoMode::HrpdOnly {
            if evdo.enabled && evdo.channel.is_none() {
                return Err("evdo.channel is required when EVDO is enabled".into());
            }
            return Ok(Self::single_carrier());
        }

        let hrpd_channel = evdo
            .channel
            .ok_or_else(|| Error::from("evdo.channel is required when EVDO is enabled"))?;
        let hrpd_plan = ChannelPlan::new(channel.band_class, channel.band_subclass, hrpd_channel);
        hrpd_plan.validate().map_err(|e| {
            Error::from(format!(
                "evdo: configured HRPD channel {} on {} is invalid: {e}",
                hrpd_plan.cdma_channel,
                hrpd_plan.band_class.as_str()
            ))
        })?;

        let required = required_composite_bandwidth_hz(
            channel.downlink_hz() as usize,
            hrpd_plan.downlink_hz() as usize,
        )
        .max(required_composite_bandwidth_hz(
            channel.uplink_hz() as usize,
            hrpd_plan.uplink_hz() as usize,
        ));
        let chip_rate = SR1_CHIP_RATE_HZ as usize;
        let sample_rate_hz = [4usize, 8, 16]
            .into_iter()
            .map(|multiple| chip_rate * multiple)
            .find(|rate| required < *rate)
            .ok_or_else(|| {
                Error::from(format!(
                    "evdo composite carriers require bandwidth {} Hz, which does not fit in the supported 16x sample-rate cap ({} Hz)",
                    required,
                    chip_rate * Self::MAX_COMPOSITE_OVERSAMPLE,
                ))
            })?;

        Ok(Self {
            tx_sample_rate_hz: sample_rate_hz,
            tx_bandwidth_hz: required,
            rx_sample_rate_hz: sample_rate_hz,
            rx_bandwidth_hz: required,
        })
    }
}

fn required_composite_bandwidth_hz(one_x_hz: usize, hrpd_hz: usize) -> usize {
    one_x_hz.abs_diff(hrpd_hz) + evdo::SR1_OCCUPIED_BANDWIDTH_HZ
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
    /// Drives TX/RX frequencies and broadcast `CDMA_FREQ` / `BAND_CLASS`
    /// via C.S0057-F. `runtime.tx_freq_hz_override` can override the TX
    /// center; RX is always derived from this channel plan.
    pub channel: ChannelPlan,
    /// SDR backend selection and per-backend hardware parameters.
    pub radio: RadioConfig,
    #[serde(skip)]
    pub rf: BtsRfProfile,
    /// Pilot PN offset (chips, in units of 64). Must be in `0..=511`.
    pub pilot_offset: usize,
    /// Optional adjacent EV-DO/HRPD carrier configuration.
    pub evdo: evdo::EvdoConfig,
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
            channel: ChannelPlan::default(),
            radio: RadioConfig::default(),
            rf: BtsRfProfile::default(),
            pilot_offset: 0,
            evdo: evdo::EvdoConfig::default(),
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
#[serde(default, deny_unknown_fields)]
struct BtsNodeConfigFile {
    pub channel: ChannelPlan,
    pub radio: Option<RadioConfig>,
    pub pilot_offset: usize,
    pub evdo: evdo::EvdoConfig,
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
            channel: ChannelPlan::default(),
            radio: None,
            pilot_offset: 0,
            evdo: evdo::EvdoConfig::default(),
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
    /// section may be inlined as `radio`; command-line callers can use
    /// `load_from_path_with_radio_override` to apply an external radio config
    /// before validation.
    pub fn load_from_path(path: &Path) -> Result<Self, Error> {
        Self::load_from_path_inner(path, None)
    }

    pub fn load_from_path_with_radio_override(
        path: &Path,
        radio_override: RadioConfig,
    ) -> Result<Self, Error> {
        Self::load_from_path_inner(path, Some(radio_override))
    }

    fn load_from_path_inner(
        path: &Path,
        radio_override: Option<RadioConfig>,
    ) -> Result<Self, Error> {
        let merged = cdma_common::config_load::load_json_with_local_override(path)?;
        let source: BtsNodeConfigFile = serde_json::from_value(merged)?;
        let radio = match (radio_override, source.radio) {
            (Some(radio), _) => radio,
            (None, Some(radio)) => radio,
            (None, None) => RadioConfig::default(),
        };
        let mut config = BtsNodeConfig {
            channel: source.channel,
            radio,
            rf: BtsRfProfile::default(),
            pilot_offset: source.pilot_offset,
            evdo: source.evdo,
            runtime: source.runtime,
            abis_timers: source.abis_timers,
            abis: source.abis,
            overhead: source.overhead,
            timezone: source.timezone,
            bearer: source.bearer,
        };
        config.apply_derived_rf_profile()?;
        config.validate()?;
        Ok(config)
    }

    fn apply_derived_rf_profile(&mut self) -> Result<(), Error> {
        self.rf = BtsRfProfile::derive(self.channel, &self.evdo)?;
        self.runtime.tx_sample_rate_hz = self.rf.tx_sample_rate_hz;
        self.runtime.tx_bandwidth_hz = self.rf.tx_bandwidth_hz;
        Ok(())
    }

    /// Load a config with EV-DO force-enabled and the RF profile re-derived for
    /// the composite carrier. EV-DO ships disabled by default, so tests that
    /// exercise the composite EV-DO paths use this to obtain an enabled config
    /// whose sample-rate/bandwidth are derived accordingly (a post-load flip of
    /// `evdo.enabled` alone would leave the narrower single-carrier rates).
    #[cfg(test)]
    pub(crate) fn load_evdo_enabled_for_test(path: &Path) -> Result<Self, Error> {
        let mut config = Self::load_from_path(path)?;
        config.evdo.enabled = true;
        config.apply_derived_rf_profile()?;
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
        self.channel
            .validate()
            .map_err(|e| Error::from(format!("bts.channel: {e}")))?;
        self.runtime.validate()?;
        if self.evdo.enabled {
            let _ = evdo::resolve_evdo_config(
                &self.evdo,
                self.pilot_offset,
                self.channel,
                self.runtime.tx_sample_rate_hz,
                self.runtime.tx_bandwidth_hz,
            )?;
        }
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
    fn loads_bts_config_with_radio_override() {
        let dir = temp_test_dir("radio-override");
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
  "rx_reference_dbm": null,
  "rx_power_adj": 3.5
}
"#,
        )
        .expect("write radio config");

        fs::write(&config_path, r#"{ "evdo": { "enabled": false } }"#).expect("write bts config");

        let radio = load_radio_from_path(&radio_path).expect("load radio override");
        let config = BtsNodeConfig::load_from_path_with_radio_override(&config_path, radio)
            .expect("load config with radio override");
        match config.radio {
            RadioConfig::Soapy {
                device,
                rx_power_adj,
                ..
            } => {
                assert_eq!(device, "driver=lime");
                assert_eq!(rx_power_adj, 3.5);
            }
            other => panic!("expected soapy radio, got {other:?}"),
        }
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn radio_rx_power_adj_defaults_to_zero() {
        let radio: RadioConfig = serde_json::from_str(
            r#"{
  "kind": "uhd",
  "device": "type=b200",
  "channel": 0,
  "antenna": "TX/RX"
}"#,
        )
        .expect("parse radio config");

        assert_eq!(radio.rx_power_adj(), 0.0);
    }

    #[test]
    fn loads_production_bts_json_with_channel_plan() {
        // Pins the shipped `config/bts.json` schema.
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("../../config/bts.json")
            .canonicalize()
            .expect("canonicalize");
        let shipped: BtsNodeConfigFile =
            serde_json::from_slice(&fs::read(&path).expect("read config/bts.json"))
                .expect("parse config/bts.json");
        assert!(
            !shipped.evdo.enabled,
            "shipped config should default EV-DO off"
        );
        let cfg = BtsNodeConfig::load_evdo_enabled_for_test(&path).expect("load config/bts.json");
        use cdma_common::band_class::BandClass;
        assert_eq!(cfg.channel.band_class, BandClass::Bc0);
        assert_eq!(cfg.channel.band_subclass, 0);
        assert_eq!(cfg.channel.cdma_channel, 777);
        assert_eq!(cfg.channel.downlink_hz(), 893_310_000);
        assert_eq!(cfg.channel.uplink_hz(), 848_310_000);
        assert!(cfg.runtime.tx_freq_hz_override.is_none());
        assert_eq!(cfg.evdo.channel, Some(630));
        assert_eq!(cfg.evdo.tx_mode(), evdo::EvdoTxMode::AdjacentComposite);
        assert_eq!(cfg.runtime.tx_sample_rate_hz, 9_830_400);
        assert_eq!(cfg.runtime.tx_bandwidth_hz, 5_890_000);
        assert_eq!(cfg.rf.rx_sample_rate_hz, 9_830_400);
        assert_eq!(cfg.rf.rx_bandwidth_hz, 5_890_000);
        let resolved = evdo::resolve_evdo_config(
            &cfg.evdo,
            cfg.pilot_offset,
            cfg.channel,
            cfg.runtime.tx_sample_rate_hz,
            cfg.runtime.tx_bandwidth_hz,
        )
        .expect("resolve evdo")
        .expect("evdo enabled");
        assert_eq!(resolved.one_x_channel, 777);
        assert_eq!(resolved.one_x_frequency_hz, 893_310_000);
        assert_eq!(resolved.evdo_channel, 630);
        assert_eq!(resolved.evdo_frequency_hz, 888_900_000);
        assert_eq!(resolved.evdo_reverse_frequency_hz, 843_900_000);
        assert_eq!(resolved.composite_center_frequency_hz, 891_105_000);
        assert_eq!(resolved.one_x_shift_hz, 2_205_000);
        assert_eq!(resolved.evdo_shift_hz, -2_205_000);
        assert_eq!(
            cfg.evdo
                .overhead
                .sector_id
                .expect("checked-in EVDO config should carry explicit SectorID")
                .to_hex(),
            "00800580000000000000000000000000"
        );
        assert_eq!(cfg.evdo.overhead.subnet_mask, Some(26));
        assert_eq!(cfg.evdo.overhead.color_code, Some(26));
    }

    #[test]
    fn loads_hrpd_only_from_bts_evdo_mode() {
        let dir = temp_test_dir("uhd-hrpd-only-single-tx");
        let config_path = dir.join("bts.json");

        fs::write(
            &config_path,
            r#"{
  "channel": {
    "band_class": "bc0",
    "band_subclass": 0,
    "cdma_channel": 384
  },
  "radio": {
    "kind": "uhd",
    "device": "type=b200",
    "channel": 0,
    "antenna": "TX/RX"
  },
  "evdo": {
    "enabled": true,
    "channel": 37,
    "mode": "hrpd_only",
    "overhead": {
      "sector_id": "00800580000000000000000000000000",
      "subnet_mask": 26,
      "color_code": 26
    }
  }
}
"#,
        )
        .expect("write bts config");

        let cfg = BtsNodeConfig::load_from_path(&config_path).expect("load config");
        assert_eq!(cfg.evdo.tx_mode(), evdo::EvdoTxMode::HrpdOnly);
        assert_eq!(cfg.runtime.tx_sample_rate_hz, 4_915_200);
        assert_eq!(cfg.runtime.tx_bandwidth_hz, 1_500_000);

        let resolved = evdo::resolve_evdo_config(
            &cfg.evdo,
            cfg.pilot_offset,
            cfg.channel,
            cfg.runtime.tx_sample_rate_hz,
            cfg.runtime.tx_bandwidth_hz,
        )
        .expect("resolve evdo")
        .expect("evdo enabled");
        assert_eq!(resolved.tx_mode, evdo::EvdoTxMode::HrpdOnly);
        assert_eq!(resolved.evdo_channel, 37);
        assert_eq!(resolved.evdo_frequency_hz, 871_110_000);
        assert_eq!(resolved.composite_center_frequency_hz, 871_110_000);
        assert_eq!(resolved.one_x_shift_hz, 0);
        assert_eq!(resolved.evdo_shift_hz, 0);
        assert!(!resolved.transmits_one_x());
        assert!(resolved.advertisement().is_none());
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn rejects_stripped_runtime_rate_fields() {
        let dir = temp_test_dir("stripped-runtime-rate-fields");
        let config_path = dir.join("bts.json");

        fs::write(
            &config_path,
            r#"{
  "channel": {
    "band_class": "bc0",
    "band_subclass": 0,
    "cdma_channel": 777
  },
  "radio": { "kind": "noop" },
  "evdo": {
    "enabled": true,
    "channel": 630,
    "overhead": {
      "sector_id": "00800580000000000000000000000000",
      "subnet_mask": 26,
      "color_code": 26
    }
  },
  "runtime": {
    "tx_sample_rate_hz": 4915200
  }
}
"#,
        )
        .expect("write bts config");

        let err = BtsNodeConfig::load_from_path(&config_path).expect_err("expected config error");
        assert!(
            err.to_string().contains("tx_sample_rate_hz"),
            "unexpected error: {err}"
        );
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn rejects_evdo_when_channel_omitted() {
        let dir = temp_test_dir("evdo-missing-channel");
        let config_path = dir.join("bts.json");

        fs::write(
            &config_path,
            r#"{
  "radio": { "kind": "noop" },
  "evdo": {
    "enabled": true,
    "overhead": {
      "sector_id": "00800580000000000000000000000000",
      "subnet_mask": 26,
      "color_code": 26
    }
  }
}
"#,
        )
        .expect("write bts config");

        let err = BtsNodeConfig::load_from_path(&config_path).expect_err("expected config error");
        assert!(
            err.to_string().contains("evdo.channel is required"),
            "unexpected error: {err}"
        );
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn rejects_composite_evdo_carriers_beyond_16x_cap() {
        let dir = temp_test_dir("evdo-composite-too-wide");
        let config_path = dir.join("bts.json");

        fs::write(
            &config_path,
            r#"{
  "channel": {
    "band_class": "bc0",
    "band_subclass": 0,
    "cdma_channel": 777
  },
  "radio": { "kind": "noop" },
  "evdo": {
    "enabled": true,
    "channel": 37,
    "overhead": {
      "sector_id": "00800580000000000000000000000000",
      "subnet_mask": 26,
      "color_code": 26
    }
  }
}
"#,
        )
        .expect("write bts config");

        let err = BtsNodeConfig::load_from_path(&config_path).expect_err("expected config error");
        assert!(
            err.to_string().contains("16x sample-rate cap"),
            "unexpected error: {err}"
        );
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn loads_non_overlapping_composite_evdo_carriers() {
        let dir = temp_test_dir("evdo-composite-non-overlapping-carriers");
        let config_path = dir.join("bts.json");

        fs::write(
            &config_path,
            r#"{
  "channel": {
    "band_class": "bc0",
    "band_subclass": 0,
    "cdma_channel": 110
  },
  "radio": { "kind": "noop" },
  "evdo": {
    "enabled": true,
    "channel": 160,
    "overhead": {
      "sector_id": "00800580000000000000000000000000",
      "subnet_mask": 26,
      "color_code": 26
    }
  }
}
"#,
        )
        .expect("write bts config");

        let cfg = BtsNodeConfig::load_from_path(&config_path).expect("load config");
        assert_eq!(cfg.runtime.tx_sample_rate_hz, 4_915_200);
        assert_eq!(cfg.runtime.tx_bandwidth_hz, 2_980_000);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn rejects_overlapping_composite_evdo_carriers() {
        let dir = temp_test_dir("evdo-composite-overlapping-carriers");
        let config_path = dir.join("bts.json");

        fs::write(
            &config_path,
            r#"{
  "channel": {
    "band_class": "bc0",
    "band_subclass": 0,
    "cdma_channel": 157
  },
  "radio": { "kind": "noop" },
  "evdo": {
    "enabled": true,
    "channel": 160,
    "overhead": {
      "sector_id": "00800580000000000000000000000000",
      "subnet_mask": 26,
      "color_code": 26
    }
  }
}
"#,
        )
        .expect("write bts config");

        let err = BtsNodeConfig::load_from_path(&config_path).expect_err("expected config error");
        assert!(
            err.to_string()
                .contains("evdo.channel must be at least 1480000 Hz from bts.channel"),
            "unexpected error: {err}"
        );
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
    fn rejects_radio_config_path_in_bts_config() {
        let dir = temp_test_dir("radio-config-path");
        let config_path = dir.join("bts.json");

        fs::write(
            &config_path,
            r#"{
  "radio_config_path": "radio_limesdr.json"
}
"#,
        )
        .expect("write bts config");

        let err = BtsNodeConfig::load_from_path(&config_path).expect_err("expected config error");
        assert!(
            err.to_string().contains("unknown field")
                && err.to_string().contains("radio_config_path"),
            "unexpected error: {err}"
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
