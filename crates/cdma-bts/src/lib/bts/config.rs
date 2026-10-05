//! BTS node configuration.
//!
//! `BtsNodeConfig` is the operator-facing configuration loaded from
//! `config/bts.json`: radio hardware setup, node-level parameters, and the
//! BTS-owned half of the Abis timers (A.S0003-A §8 Table 8-1). The in-memory
//! runtime and PHY channel settings derived from it live in `super::settings`.

use std::path::{Path, PathBuf};

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

fn default_network_samples_per_packet() -> usize {
    crate::sdr::network::wire::DEFAULT_SAMPLES_PER_PACKET
}

fn default_true() -> bool {
    true
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReverseRxTarget {
    #[default]
    OneX,
    Hrpd,
    Composite,
}

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
        #[serde(default)]
        tx_sample_delay: Option<i64>,
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
        /// UHD TX gain where the unit-RMS connector power estimate applies.
        #[serde(default)]
        tx_max_gain_db: Option<f64>,
        /// Approximate unit-RMS connector power at `tx_max_gain_db`.
        #[serde(default)]
        tx_max_gain_power_estimate_dbm: Option<f32>,
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
        #[serde(default)]
        rx_max_gain_db: Option<f64>,
        /// Approximate input power at 0 dBFS when RX gain is `rx_max_gain_db`.
        #[serde(default)]
        rx_max_gain_reference_dbm: Option<f64>,
        /// Radio-specific dBFS calibration offset applied to reverse-link raw
        /// power-control thresholds. Defaults to 0 dBFS.
        #[serde(default)]
        rx_power_adj: f32,
        #[serde(default)]
        rx_sample_delay: i64,
        #[serde(default)]
        tx_sample_delay: Option<i64>,
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
        #[serde(default)]
        tx_sample_delay: Option<i64>,
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
        #[serde(default)]
        tx_sample_delay: Option<i64>,
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
    Network {
        addr: String,
        #[serde(default)]
        data_host: Option<String>,
        #[serde(default = "default_network_samples_per_packet")]
        samples_per_packet: usize,
        #[serde(default)]
        tx_transport: crate::sdr::network::wire::TxTransport,
        #[serde(default = "default_true")]
        rx_enabled: bool,
        #[serde(default)]
        rx_antenna: Option<String>,
        #[serde(default)]
        rx_gain_db: Option<f64>,
        #[serde(default)]
        rx_power_adj: f32,
        #[serde(default)]
        rx_sample_delay: i64,
        #[serde(default)]
        tx_sample_delay: Option<i64>,
        #[serde(default = "default_rx_batch_pcgs")]
        rx_batch_pcgs: usize,
        #[serde(default)]
        traffic_rx_continuity: bool,
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
    pub fn has_rx(&self) -> bool {
        match self {
            Self::Soapy { .. }
            | Self::Uhd { .. }
            | Self::Lime { .. }
            | Self::BladeRf { .. }
            | Self::Noop => true,
            Self::Network { rx_enabled, .. } => *rx_enabled,
            Self::FileOutput { .. } => false,
        }
    }

    pub fn kind_name(&self) -> &'static str {
        match self {
            Self::FileOutput { .. } => "file_output",
            Self::Noop => "noop",
            Self::Soapy { .. } => "soapy",
            Self::Uhd { .. } => "uhd",
            Self::Lime { .. } => "lime",
            Self::BladeRf { .. } => "blade_rf",
            Self::Network { .. } => "network",
        }
    }

    pub fn rx_antenna_or_default(&self) -> String {
        match self {
            Self::Soapy { rx_antenna, .. } | Self::Lime { rx_antenna, .. } => {
                rx_antenna.clone().unwrap_or_else(|| "LNAW".to_string())
            }
            Self::Uhd { rx_antenna, .. } => rx_antenna.clone().unwrap_or_else(|| "RX2".to_string()),
            Self::BladeRf { rx_antenna, .. } | Self::Network { rx_antenna, .. } => {
                rx_antenna.clone().unwrap_or_default()
            }
            _ => String::new(),
        }
    }

    pub fn rx_gain_db_f64(&self) -> Option<f64> {
        match self {
            Self::Soapy { rx_gain_db, .. }
            | Self::Uhd { rx_gain_db, .. }
            | Self::Network { rx_gain_db, .. } => *rx_gain_db,
            Self::Lime { rx_gain_db, .. } => rx_gain_db.map(|g| g as f64),
            Self::BladeRf { rx_gain_db, .. } => rx_gain_db.map(|g| g as f64),
            _ => None,
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
            }
            | Self::Network {
                rx_sample_delay, ..
            } => *rx_sample_delay,
            _ => 0,
        }
    }

    /// Optional MS transmit timing correction at the 4× chip sample rate.
    pub fn tx_sample_delay(&self) -> Option<i64> {
        match self {
            Self::Soapy {
                tx_sample_delay, ..
            }
            | Self::Uhd {
                tx_sample_delay, ..
            }
            | Self::Lime {
                tx_sample_delay, ..
            }
            | Self::BladeRf {
                tx_sample_delay, ..
            }
            | Self::Network {
                tx_sample_delay, ..
            } => *tx_sample_delay,
            _ => None,
        }
    }

    pub fn tx_full_scale_power_estimate_dbm(&self) -> Option<f32> {
        match self {
            Self::Uhd {
                tx_gain_db,
                tx_max_gain_db: Some(max_gain_db),
                tx_max_gain_power_estimate_dbm: Some(max_power_dbm),
                ..
            } => {
                let estimate = *max_power_dbm as f64 + tx_gain_db - max_gain_db;
                estimate.is_finite().then_some(estimate as f32)
            }
            _ => None,
        }
    }

    pub fn rx_batch_pcgs(&self) -> usize {
        match self {
            Self::Soapy { rx_batch_pcgs, .. }
            | Self::Uhd { rx_batch_pcgs, .. }
            | Self::Lime { rx_batch_pcgs, .. }
            | Self::BladeRf { rx_batch_pcgs, .. }
            | Self::Network { rx_batch_pcgs, .. } => *rx_batch_pcgs,
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
            }
            | Self::Network {
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
            Self::Uhd {
                rx_gain_db: Some(gain_db),
                rx_max_gain_db: Some(max_gain_db),
                rx_max_gain_reference_dbm: Some(max_reference_dbm),
                ..
            } => {
                let reference_dbm = max_reference_dbm + max_gain_db - gain_db;
                reference_dbm.is_finite().then_some(reference_dbm)
            }
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
            | Self::BladeRf { rx_power_adj, .. }
            | Self::Network { rx_power_adj, .. } => *rx_power_adj,
            _ => 0.0,
        }
    }

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

/// Aggregated event bus the fused HRPD Access Network publishes to.
const DEFAULT_EVENTS_ENDPOINT: &str = "http://127.0.0.1:17023";
/// HLR the fused HRPD Access Network resolves subscriber identities against.
const DEFAULT_HLR_ENDPOINT: &str = "http://127.0.0.1:17019";

fn default_events_endpoint() -> Option<String> {
    Some(DEFAULT_EVENTS_ENDPOINT.to_string())
}

fn default_hlr_endpoint() -> Option<String> {
    Some(DEFAULT_HLR_ENDPOINT.to_string())
}

/// Wait for the MS acknowledgement of a paged SDU before the BTS reports an
/// Abis L2 failure, in milliseconds.
const DEFAULT_PAGING_ACK_TIMEOUT_MS: u64 = 15_000;
/// Slot-aligned over-the-air retransmissions of an unacknowledged page.
const DEFAULT_PAGING_MAX_RETRIES: u32 = 0;

fn default_paging_ack_timeout_ms() -> u64 {
    DEFAULT_PAGING_ACK_TIMEOUT_MS
}

fn default_paging_max_retries() -> u32 {
    DEFAULT_PAGING_MAX_RETRIES
}

/// BTS-side paging channel retransmission budget. The BTS supplier owns GPM
/// assembly, slot placement and page retry, so the timing lives with the
/// element that performs it.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct BtsPagingRetryConfig {
    /// How long to wait for an MS ACK before reporting Abis L2 failure.
    #[serde(default = "default_paging_ack_timeout_ms")]
    pub ack_timeout_ms: u64,
    /// Maximum number of slot-aligned over-the-air retransmissions.
    #[serde(default = "default_paging_max_retries")]
    pub max_retries: u32,
}

impl Default for BtsPagingRetryConfig {
    fn default() -> Self {
        Self {
            ack_timeout_ms: default_paging_ack_timeout_ms(),
            max_retries: default_paging_max_retries(),
        }
    }
}

/// `overhead.page_chan` must name the same paging channel as
/// `runtime.downlink.paging.paging_channel_number`. Both live in `bts.json`
/// but in different sections, so startup double-checks them.
pub fn validate_page_chan_alignment(
    overhead_page_chan: u8,
    bts_paging_channel_number: u8,
) -> Result<(), Error> {
    if overhead_page_chan != bts_paging_channel_number {
        return Err(
            "bts.overhead.page_chan must match bts.runtime.downlink.paging.paging_channel_number"
                .into(),
        );
    }
    Ok(())
}

fn default_bts_management_bind_addr() -> SocketAddr {
    "127.0.0.1:17024"
        .parse()
        .expect("static BTS management address should parse")
}

/// Address the BTS listens on for the operations plane. A.S0003-A leaves OAM
/// signaling for further study, so enrollment is gRPC rather than an Abis
/// message type: the BSC fetches this cell's identity and radio parameters
/// here before bringing up Abis.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BtsManagementConfig {
    /// Management and enrollment gRPC listener.
    pub bind_addr: SocketAddr,
}

impl Default for BtsManagementConfig {
    fn default() -> Self {
        Self {
            bind_addr: default_bts_management_bind_addr(),
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
    /// Sector this BTS operates, paired with `overhead.base_id` to form the
    /// cell identity it enrolls under and stamps on the Abis Cell Identifier
    /// IE. Required.
    pub sector: u8,
    /// Directory IQ captures requested over the operations plane are written
    /// to. Absent uses the built-in default.
    pub iq_capture_dir: Option<PathBuf>,
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
    /// Operations-plane listener for enrollment and management.
    #[serde(default)]
    pub management: BtsManagementConfig,
    /// Paging retransmission budget applied by the BTS paging supplier.
    #[serde(default)]
    pub paging_retry: BtsPagingRetryConfig,
    /// Aggregated event bus the fused HRPD Access Network publishes session
    /// events to. `null` stops the AN publishing.
    #[serde(default = "default_events_endpoint")]
    pub events_endpoint: Option<String>,
    /// HLR the fused HRPD Access Network resolves subscriber identities
    /// against. `null` leaves HRPD identities underived.
    #[serde(default = "default_hlr_endpoint")]
    pub hlr_endpoint: Option<String>,
}

impl Default for BtsNodeConfig {
    fn default() -> Self {
        Self {
            management: BtsManagementConfig::default(),
            channel: ChannelPlan::default(),
            radio: RadioConfig::default(),
            rf: BtsRfProfile::default(),
            pilot_offset: 0,
            sector: 0,
            iq_capture_dir: None,
            evdo: evdo::EvdoConfig::default(),
            runtime: BtsRuntimeSettings::default(),
            abis_timers: BtsAbisTimers::default(),
            abis: BtsAbisConfig::default(),
            overhead: super::settings::OverheadParameters::default(),
            timezone: cdma_common::timezone::TimezoneConfig::default(),
            bearer: BtsBearerConfig::default(),
            paging_retry: BtsPagingRetryConfig::default(),
            events_endpoint: default_events_endpoint(),
            hlr_endpoint: default_hlr_endpoint(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
#[serde(default, deny_unknown_fields)]
struct BtsNodeConfigFile {
    pub channel: ChannelPlan,
    pub radio: Option<RadioConfig>,
    pub pilot_offset: usize,
    pub sector: u8,
    pub iq_capture_dir: Option<PathBuf>,
    pub evdo: evdo::EvdoConfig,
    pub runtime: BtsRuntimeSettings,
    pub abis_timers: BtsAbisTimers,
    pub abis: BtsAbisConfig,
    pub overhead: super::settings::OverheadParameters,
    pub timezone: cdma_common::timezone::TimezoneConfig,
    pub bearer: BtsBearerConfig,
    #[serde(default)]
    pub management: BtsManagementConfig,
    #[serde(default)]
    pub paging_retry: BtsPagingRetryConfig,
    #[serde(default = "default_events_endpoint")]
    pub events_endpoint: Option<String>,
    #[serde(default = "default_hlr_endpoint")]
    pub hlr_endpoint: Option<String>,
}

impl Default for BtsNodeConfigFile {
    fn default() -> Self {
        Self {
            management: BtsManagementConfig::default(),
            channel: ChannelPlan::default(),
            radio: None,
            pilot_offset: 0,
            sector: 0,
            iq_capture_dir: None,
            evdo: evdo::EvdoConfig::default(),
            runtime: BtsRuntimeSettings::default(),
            abis_timers: BtsAbisTimers::default(),
            abis: BtsAbisConfig::default(),
            overhead: super::settings::OverheadParameters::default(),
            timezone: cdma_common::timezone::TimezoneConfig::default(),
            bearer: BtsBearerConfig::default(),
            paging_retry: BtsPagingRetryConfig::default(),
            events_endpoint: default_events_endpoint(),
            hlr_endpoint: default_hlr_endpoint(),
        }
    }
}

impl BtsNodeConfig {
    /// Load and validate a `BtsNodeConfig` from a JSON file. The radio
    /// section may be inlined as `radio`; command-line callers can use
    /// `load_from_path_with_radio_override` to apply an external radio config
    /// before validation.
    pub fn load_from_path(path: &Path) -> Result<Self, Error> {
        Self::load_from_path_inner(path, None, None)
    }

    pub fn load_from_path_with_radio_override(
        path: &Path,
        radio_override: RadioConfig,
    ) -> Result<Self, Error> {
        Self::load_from_path_inner(path, None, Some(radio_override))
    }

    /// Load a base BTS config, then apply an optional named-profile config and
    /// optional CLI radio override. Each JSON path includes its sibling local
    /// override before the next layer is applied.
    pub fn load_from_path_with_overrides(
        path: &Path,
        profile_path: Option<&Path>,
        radio_override: Option<RadioConfig>,
    ) -> Result<Self, Error> {
        Self::load_from_path_inner(path, profile_path, radio_override)
    }

    fn load_from_path_inner(
        path: &Path,
        profile_path: Option<&Path>,
        radio_override: Option<RadioConfig>,
    ) -> Result<Self, Error> {
        let mut merged = cdma_common::config_load::load_json_with_local_override(path)?;
        if let Some(profile_path) = profile_path {
            let profile = cdma_common::config_load::load_json_with_local_override(profile_path)?;
            cdma_common::config_load::merge_json(&mut merged, profile);
        }
        let source: BtsNodeConfigFile = serde_json::from_value(merged)?;
        Self::from_file_config(source, radio_override)
    }

    fn from_file_config(
        source: BtsNodeConfigFile,
        radio_override: Option<RadioConfig>,
    ) -> Result<Self, Error> {
        let radio = match (radio_override, source.radio) {
            (Some(radio), _) => radio,
            (None, Some(radio)) => radio,
            (None, None) => RadioConfig::default(),
        };
        let mut config = BtsNodeConfig {
            management: source.management,
            channel: source.channel,
            radio,
            rf: BtsRfProfile::default(),
            pilot_offset: source.pilot_offset,
            sector: source.sector,
            iq_capture_dir: source.iq_capture_dir,
            evdo: source.evdo,
            runtime: source.runtime,
            abis_timers: source.abis_timers,
            abis: source.abis,
            overhead: source.overhead,
            timezone: source.timezone,
            bearer: source.bearer,
            paging_retry: source.paging_retry,
            events_endpoint: source.events_endpoint,
            hlr_endpoint: source.hlr_endpoint,
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
        // Checked-in fixture tests must not inherit local operator overrides.
        let source: BtsNodeConfigFile = serde_json::from_slice(&std::fs::read(path)?)?;
        let mut config = Self::from_file_config(source, None)?;
        config.evdo.enabled = true;
        config.apply_derived_rf_profile()?;
        config.validate()?;
        Ok(config)
    }

    /// Validate self-contained BTS invariants. Checks that span two
    /// sections of the file, such as `validate_page_chan_alignment`, run at
    /// node startup instead.
    pub fn validate(&self) -> Result<(), Error> {
        if self.pilot_offset > 511 {
            return Err("bts.pilot_offset must be in 0..=511".into());
        }
        if self.sector == 0 {
            return Err("bts.sector is required and must be in 1..=255".into());
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

        fs::write(
            &config_path,
            r#"{ "sector": 1, "evdo": { "enabled": false } }"#,
        )
        .expect("write bts config");

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
    fn bts_profile_overrides_generic_local_config_and_uses_profile_local_config() {
        let dir = temp_test_dir("profile-precedence");
        let config_path = dir.join("bts.json");
        let local_path = dir.join("bts.local.json");
        let profile_path = dir.join("bts.sprint.json");
        let profile_local_path = dir.join("bts.sprint.local.json");

        fs::write(
            &config_path,
            r#"{
  "sector": 1,
  "pilot_offset": 1,
  "evdo": { "enabled": false },
  "overhead": { "sid": 1, "nid": 1 }
}"#,
        )
        .expect("write base config");
        fs::write(
            &local_path,
            r#"{
  "sector": 1,
  "pilot_offset": 2,
  "overhead": { "nid": 2 }
}"#,
        )
        .expect("write generic local config");
        fs::write(
            &profile_path,
            r#"{
  "sector": 1,
  "pilot_offset": 3,
  "overhead": { "sid": 3 }
}"#,
        )
        .expect("write profile config");
        fs::write(&profile_local_path, r#"{ "pilot_offset": 4 }"#)
            .expect("write profile local config");

        let config =
            BtsNodeConfig::load_from_path_with_overrides(&config_path, Some(&profile_path), None)
                .expect("load config with profile");

        assert_eq!(config.pilot_offset, 4);
        assert_eq!(config.overhead.sid, 3);
        assert_eq!(config.overhead.nid, 2);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn loads_shipped_sprint_profile() {
        let dir = temp_test_dir("shipped-sprint-profile");
        let config_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../config");
        let config_path = dir.join("bts.json");
        let profile_path = dir.join("bts.sprint.json");
        fs::copy(config_dir.join("bts.json"), &config_path).expect("copy base config");
        fs::copy(config_dir.join("bts.sprint.json"), &profile_path).expect("copy Sprint profile");

        let config =
            BtsNodeConfig::load_from_path_with_overrides(&config_path, Some(&profile_path), None)
                .expect("load shipped Sprint profile");

        use cdma_common::band_class::BandClass;
        assert_eq!(config.channel.band_class, BandClass::Bc1);
        assert_eq!(config.channel.cdma_channel, 50);
        assert_eq!(config.overhead.sid, 4107);
        assert!(config.evdo.enabled);
        assert_eq!(config.evdo.channel, Some(75));
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
        assert_eq!(cfg.channel.cdma_channel, 691);
        assert_eq!(cfg.channel.downlink_hz(), 890_730_000);
        assert_eq!(cfg.channel.uplink_hz(), 845_730_000);
        assert!(cfg.runtime.tx_freq_hz_override.is_none());
        assert_eq!(cfg.evdo.channel, Some(630));
        assert_eq!(cfg.evdo.tx_mode(), evdo::EvdoTxMode::AdjacentComposite);
        assert_eq!(cfg.runtime.tx_sample_rate_hz, 4_915_200);
        assert_eq!(cfg.runtime.tx_bandwidth_hz, 3_310_000);
        assert_eq!(cfg.runtime.tx_digital_backoff, 0.3);
        assert_eq!(cfg.rf.rx_sample_rate_hz, 4_915_200);
        assert_eq!(cfg.rf.rx_bandwidth_hz, 3_310_000);
        let resolved = evdo::resolve_evdo_config(
            &cfg.evdo,
            cfg.pilot_offset,
            cfg.channel,
            cfg.runtime.tx_sample_rate_hz,
            cfg.runtime.tx_bandwidth_hz,
        )
        .expect("resolve evdo")
        .expect("evdo enabled");
        assert_eq!(resolved.one_x_channel, 691);
        assert_eq!(resolved.one_x_frequency_hz, 890_730_000);
        assert_eq!(resolved.evdo_channel, 630);
        assert_eq!(resolved.evdo_frequency_hz, 888_900_000);
        assert_eq!(resolved.evdo_reverse_frequency_hz, 843_900_000);
        assert_eq!(resolved.composite_center_frequency_hz, 889_815_000);
        assert_eq!(resolved.one_x_shift_hz, 915_000);
        assert_eq!(resolved.evdo_shift_hz, -915_000);
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
  "sector": 1,
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
  "sector": 1,
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
  "sector": 1,
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
  "sector": 1,
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
  "sector": 1,
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
    fn loads_adjacent_bc1_composite_evdo_carriers() {
        let dir = temp_test_dir("evdo-composite-adjacent-bc1-carriers");
        let config_path = dir.join("bts.json");

        fs::write(
            &config_path,
            r#"{
  "sector": 1,
  "channel": {
    "band_class": "bc1",
    "band_subclass": 0,
    "cdma_channel": 775
  },
  "radio": { "kind": "noop" },
  "evdo": {
    "enabled": true,
    "channel": 750,
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
        assert_eq!(cfg.runtime.tx_bandwidth_hz, 2_730_000);

        let resolved = evdo::resolve_evdo_config(
            &cfg.evdo,
            cfg.pilot_offset,
            cfg.channel,
            cfg.runtime.tx_sample_rate_hz,
            cfg.runtime.tx_bandwidth_hz,
        )
        .expect("resolve adjacent BC1 carriers")
        .expect("EV-DO enabled");
        assert_eq!(resolved.one_x_frequency_hz, 1_968_750_000);
        assert_eq!(resolved.evdo_frequency_hz, 1_967_500_000);
        assert_eq!(resolved.one_x_shift_hz, 625_000);
        assert_eq!(resolved.evdo_shift_hz, -625_000);
        fs::remove_dir_all(dir).ok();
    }

    #[test]
    fn rejects_overlapping_composite_evdo_carriers() {
        let dir = temp_test_dir("evdo-composite-overlapping-carriers");
        let config_path = dir.join("bts.json");

        fs::write(
            &config_path,
            r#"{
  "sector": 1,
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
                .contains("evdo.channel must be at least 1180000 Hz from bts.channel"),
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
  "sector": 1,
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
        fs::write(&path, r#"{ "sector": 1, "radio": { "kind": "noop" } }"#).unwrap();
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
  "sector": 1,
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
  "sector": 1,
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
  "sector": 1,
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
  "sector": 1,
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
