//! BSC node configuration and management plane configuration.
//!
//! Loaded from `config/bsc.json` and `config/management.json`.
//!
//! Voice/circuit policy now lives in `cdma-msc::config`; BSC runtime code
//! imports the MSC-owned policy types directly where radio execution needs
//! them.

use std::{
    net::{Ipv4Addr, SocketAddr},
    path::Path,
    path::PathBuf,
};

use cdma_common::error::Error;
use cdma_common::sch::{DEFAULT_RC3_F_SCH_RATE_BPS, Rc3FschProfile};
// VoiceConfig, VoiceGatewayConfig, and MediaRingbackType are MSC-owned types
// defined in cdma_msc::config. BSC code that needs them imports from cdma_msc
// directly rather than re-exporting through cdma_bsc::config.
use serde::{Deserialize, Serialize};

// Default per-node config filenames within the configured config directory.

/// Filename of the standalone BTS node config inside the config directory.
pub const BTS_CONFIG_FILENAME: &str = "bts.json";
/// Filename of the standalone BSC node config inside the config directory.
pub const BSC_CONFIG_FILENAME: &str = "bsc.json";
/// Filename of the standalone MSC node config inside the config directory.
pub const MSC_CONFIG_FILENAME: &str = "msc.json";
/// Filename of the standalone PCF node config inside the config directory.
pub const PCF_CONFIG_FILENAME: &str = "pcf.json";
/// Filename of the standalone PDSN node config inside the config directory.
pub const PDSN_CONFIG_FILENAME: &str = "pdsn.json";
/// Filename of the standalone HLR node config inside the config directory.
pub const HLR_CONFIG_FILENAME: &str = "hlr.json";
/// Filename of the standalone SMSC node config inside the config directory.
pub const SMSC_CONFIG_FILENAME: &str = "smsc.json";
/// Filename of the management plane config inside the config directory.
pub const MANAGEMENT_CONFIG_FILENAME: &str = "management.json";
/// Filename of the aggregated event bus config inside the config directory.
pub const EVENTS_CONFIG_FILENAME: &str = "events.json";

/// Default config directory used when neither `--config-dir` nor
/// `CDMA_CONFIG_DIR` is set.
pub const DEFAULT_CONFIG_DIR: &str = "config";

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct OverheadConfig {
    pub sid: u16,
    pub nid: u16,
    pub base_id: u16,
    pub reg_zone: u16,
    pub total_zones: u8,
    pub zone_timer: u8,
    pub max_slot_cycle_index: u8,
    pub page_chan: u8,
    pub config_seq: u8,
    pub acc_config_seq: u8,
    pub power_up_reg: bool,
    pub parameter_reg: bool,
    pub auth_mode: u8,
    pub p_rev: u8,
    pub min_p_rev: u8,
    pub prat: u8,
    /// `None` → derive from BTS `ChannelPlan`.
    pub cdma_freq: Option<u16>,
    pub ext_cdma_freq: Option<u16>,
    /// T1b timer period in milliseconds (default 1280). Each required overhead
    /// message must be sent at least once per T1b on the paging channel.
    #[serde(default = "default_t1b_ms")]
    pub t1b_ms: u64,
}

fn default_t1b_ms() -> u64 {
    1280
}

impl Default for OverheadConfig {
    fn default() -> Self {
        Self {
            sid: 1,
            nid: 1,
            base_id: 1,
            reg_zone: 0,
            total_zones: 1,
            zone_timer: 0,
            max_slot_cycle_index: 0,
            page_chan: 1,
            config_seq: 23,
            acc_config_seq: 1,
            power_up_reg: true,
            parameter_reg: false,
            auth_mode: 0,
            p_rev: 6,
            min_p_rev: 6,
            prat: 0,
            cdma_freq: None,
            ext_cdma_freq: None,
            t1b_ms: 1280,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RcPairConfig {
    pub for_rc: u8,
    pub rev_rc: u8,
}

impl RcPairConfig {
    pub const fn new(for_rc: u8, rev_rc: u8) -> Self {
        Self { for_rc, rev_rc }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct TrafficAssignmentConfig {
    pub supported_for_rcs: Vec<u8>,
    pub supported_rev_rcs: Vec<u8>,
    pub preferred_pairs: Vec<RcPairConfig>,
    /// Tear down a traffic channel after this many seconds of inactivity
    /// (no RX messages received). Applies to all traffic channels including
    /// voice calls stuck in setup. Default: 30 seconds.
    #[serde(default = "default_traffic_idle_timeout_s")]
    pub idle_timeout_s: u64,
    /// Tear down a traffic channel if the MS does not acknowledge the BS Ack
    /// Order sent after reverse preamble detection. Default: 5 seconds.
    #[serde(default = "default_ms_ack_timeout_ms")]
    pub ms_ack_timeout_ms: u64,
    /// Tear down a packet-data traffic channel if the MS does not respond
    /// to a Service Connect Message with a Service Connect Completion
    /// Message within this window. Default: 5 seconds. Voice sessions
    /// use the MSC voice policy's `service_connect_timeout_ms` instead.
    #[serde(default = "default_packet_service_connect_timeout_ms")]
    pub packet_service_connect_timeout_ms: u64,
    /// Per C.S0002-E §2.1.3.12.7: when true and RC3 rate is 1500 bps,
    /// the mobile only transmits R-FCH on PCGs {2,3,6,7,10,11,14,15}.
    /// Sent in the ECAM as REV_FCH_GATING_MODE. Default: false (no gating).
    #[serde(default)]
    pub rev_fch_gating_mode: bool,
    /// Enable F-SCH for eligible SO33 RC3 packet calls. Disabled calls stay
    /// FCH-only regardless of mobile capability.
    #[serde(default)]
    pub enable_f_sch: bool,
    /// Target RC3 F-SCH rate. Supported values: 19200, 38400, 76800, 153600.
    #[serde(default = "default_f_sch_rate_bps")]
    pub f_sch_rate_bps: u32,
}

impl Default for TrafficAssignmentConfig {
    fn default() -> Self {
        Self {
            supported_for_rcs: vec![1, 3],
            supported_rev_rcs: vec![1, 3],
            preferred_pairs: vec![RcPairConfig::new(1, 1), RcPairConfig::new(3, 3)],
            idle_timeout_s: default_traffic_idle_timeout_s(),
            ms_ack_timeout_ms: default_ms_ack_timeout_ms(),
            packet_service_connect_timeout_ms: default_packet_service_connect_timeout_ms(),
            rev_fch_gating_mode: false,
            enable_f_sch: false,
            f_sch_rate_bps: default_f_sch_rate_bps(),
        }
    }
}

fn default_f_sch_rate_bps() -> u32 {
    DEFAULT_RC3_F_SCH_RATE_BPS
}

fn default_traffic_idle_timeout_s() -> u64 {
    30
}

fn default_ms_ack_timeout_ms() -> u64 {
    5000
}

fn default_packet_service_connect_timeout_ms() -> u64 {
    5000
}

fn default_traffic_ack_timeout_ms() -> u64 {
    400 // T1m per C.S0004-E Annex A
}

fn default_traffic_max_retries() -> u32 {
    3
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct TrafficRetryConfig {
    #[serde(default = "default_traffic_ack_timeout_ms")]
    pub ack_timeout_ms: u64,
    #[serde(default = "default_traffic_max_retries")]
    pub max_retries: u32,
}

impl Default for TrafficRetryConfig {
    fn default() -> Self {
        Self {
            ack_timeout_ms: default_traffic_ack_timeout_ms(),
            max_retries: default_traffic_max_retries(),
        }
    }
}

fn default_paging_ack_timeout_ms() -> u64 {
    1000
}

fn default_paging_max_retries() -> u32 {
    3
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct PagingRetryConfig {
    #[serde(default = "default_paging_ack_timeout_ms")]
    pub ack_timeout_ms: u64,
    #[serde(default = "default_paging_max_retries")]
    pub max_retries: u32,
}

impl Default for PagingRetryConfig {
    fn default() -> Self {
        Self {
            ack_timeout_ms: default_paging_ack_timeout_ms(),
            max_retries: default_paging_max_retries(),
        }
    }
}

/// BSC-owned Abis timers per A.S0003-A §8 Table 8-1.
///
/// All values in milliseconds. Granularity is 100 ms; ranges are 0–1000 ms
/// except `tsetupb_ms` (0–500). Configurable per BSC within the spec range.
#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct BscAbisTimers {
    /// §8.2 — BSC-side timer for `Abis-BTS Setup`. Default 100 ms (range 0–500).
    pub tsetupb_ms: u64,
    /// §8.3 — BSC-side timer for `Abis-Traffic Channel Status`. Default 500 ms.
    pub tchanstatb_ms: u64,
    /// §8.5 — BSC-side timer for `Abis-BTS Release Ack`. Default 500 ms.
    pub tdrptgtb_ms: u64,
    /// §8.6 — BSC-side timer for `Abis-Burst Response`. Default 500 ms.
    pub tbstreqb_ms: u64,
}

impl Default for BscAbisTimers {
    fn default() -> Self {
        Self {
            tsetupb_ms: 100,
            tchanstatb_ms: 500,
            tdrptgtb_ms: 500,
            tbstreqb_ms: 500,
        }
    }
}

fn default_bsc_bearer_bind_addr() -> SocketAddr {
    SocketAddr::new(
        std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        cdma_abis::transport::ABIS_BSC_BEARER_PORT,
    )
}

fn default_bsc_bearer_remote_addr() -> SocketAddr {
    SocketAddr::new(
        std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        cdma_abis::transport::ABIS_BTS_BEARER_PORT,
    )
}

fn default_bsc_abis_remote_addr() -> SocketAddr {
    SocketAddr::new(
        std::net::IpAddr::V4(std::net::Ipv4Addr::LOCALHOST),
        cdma_abis::transport::ABIS_SIGNALING_PORT,
    )
}

/// BSC-side Abis signaling (TCP) addressing.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct BscAbisConfig {
    /// Remote BTS Abis signaling address. Default `127.0.0.1:5604`.
    #[serde(default = "default_bsc_abis_remote_addr")]
    pub remote_addr: SocketAddr,
}

impl Default for BscAbisConfig {
    fn default() -> Self {
        Self {
            remote_addr: default_bsc_abis_remote_addr(),
        }
    }
}

/// BSC-side Abis bearer (UDP) addressing.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct BscBearerConfig {
    /// Local UDP address for the BSC bearer transport. Default `127.0.0.1:17022`.
    #[serde(default = "default_bsc_bearer_bind_addr")]
    pub bind_addr: SocketAddr,
    /// Remote BTS bearer address. Default `127.0.0.1:17014`.
    #[serde(default = "default_bsc_bearer_remote_addr")]
    pub remote_addr: SocketAddr,
}

impl Default for BscBearerConfig {
    fn default() -> Self {
        Self {
            bind_addr: default_bsc_bearer_bind_addr(),
            remote_addr: default_bsc_bearer_remote_addr(),
        }
    }
}

/// Standalone BSC node configuration (loaded from `config/bsc.json`).
///
/// Carries cell broadcast/overhead policy, radio-resource assignment policy,
/// traffic and paging retry policy, and the BSC-side Abis timers.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct BscNodeConfig {
    /// Overhead message contents that the BSC instructs the BTS to broadcast
    /// (SID/NID, registration, P_REV, page channel, CDMA freq).
    pub overhead: OverheadConfig,
    /// Radio configuration / service-option policy applied during traffic
    /// channel assignment.
    pub traffic_assignment: TrafficAssignmentConfig,
    /// Forward-traffic ACK timing and retry budget.
    pub traffic_retry: TrafficRetryConfig,
    /// Forward-paging ACK timing and retry budget.
    pub paging_retry: PagingRetryConfig,
    /// Evict idle registered mobiles (no access activity and no active
    /// traffic channel) after this many seconds. Default: 3600 (1 hour).
    /// Set to 0 to disable.
    #[serde(default = "default_mobile_idle_timeout_s")]
    pub mobile_idle_timeout_s: u64,
    /// BSC-side Abis timers per A.S0003-A §8 Table 8-1.
    pub abis_timers: BscAbisTimers,
    /// BSC-side Abis signaling (TCP) addressing.
    pub abis: BscAbisConfig,
    /// Abis bearer (UDP) addressing for traffic frames.
    pub bearer: BscBearerConfig,
    /// Local IP that voice bearer UDP sockets bind to.
    /// Defaults to 127.0.0.1. Set to the host's network-facing IP when the
    /// BSC and voice gateway are on separate hosts.
    #[serde(default = "default_voice_bearer_bind_ip")]
    pub voice_bearer_bind_ip: Ipv4Addr,
    /// Stable identifier for this BSC node written to the HLR on every
    /// registration and included in management events. Must be unique
    /// across all BSC instances. Defaults to "bsc".
    #[serde(default = "default_node_id")]
    pub node_id: String,
}

fn default_node_id() -> String {
    "bsc".to_string()
}

fn default_voice_bearer_bind_ip() -> Ipv4Addr {
    Ipv4Addr::LOCALHOST
}

impl Default for BscNodeConfig {
    fn default() -> Self {
        Self {
            overhead: OverheadConfig::default(),
            traffic_assignment: TrafficAssignmentConfig::default(),
            traffic_retry: TrafficRetryConfig::default(),
            paging_retry: PagingRetryConfig::default(),
            mobile_idle_timeout_s: default_mobile_idle_timeout_s(),
            abis_timers: BscAbisTimers::default(),
            abis: BscAbisConfig::default(),
            bearer: BscBearerConfig::default(),
            voice_bearer_bind_ip: default_voice_bearer_bind_ip(),
            node_id: default_node_id(),
        }
    }
}

fn default_mobile_idle_timeout_s() -> u64 {
    3600
}

impl BscNodeConfig {
    /// Load and validate a `BscNodeConfig` from a JSON file.
    pub fn load_from_path(path: &Path) -> Result<Self, Error> {
        let merged = cdma_common::config_load::load_json_with_local_override(path)?;
        let cfg: Self = serde_json::from_value(merged)?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Validate self-contained BSC invariants. Cross-node validation
    /// (e.g. matching `page_chan` against the BTS paging channel) is done
    /// in bootstrap via `validate_page_chan_alignment`.
    pub fn validate(&self) -> Result<(), Error> {
        validate_traffic_assignment(&self.traffic_assignment)?;
        if self.overhead.page_chan == 0 || self.overhead.page_chan > 7 {
            return Err("overhead.page_chan must be in 1..=7".into());
        }
        if self.overhead.auth_mode > 3 {
            return Err("overhead.auth_mode must be in 0..=3".into());
        }
        Ok(())
    }
}

fn default_iq_capture_dir() -> PathBuf {
    PathBuf::from("capture-iq-wav")
}

/// mTLS configuration for the management plane. When `None` on
/// `ManagementConfig`, the management server is plaintext and accepts any
/// client. See `docs/architecture-update/07-management-and-web-touchpoints.md`
/// "Auth Model".
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MtlsConfig {
    /// Path to the PEM-encoded server certificate.
    pub cert_path: PathBuf,
    /// Path to the PEM-encoded server private key.
    pub key_path: PathBuf,
    /// Path to the PEM-encoded CA bundle used to verify client
    /// certificates (mutual TLS).
    pub client_ca_path: PathBuf,
}

/// Management plane configuration (loaded from `config/management.json`).
///
/// The management plane is the only place gRPC is used in this
/// architecture; all standards interfaces use their spec-defined transports.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct ManagementConfig {
    /// Operator/UI gRPC listen address.
    pub grpc_listen_addr: SocketAddr,
    /// Optional mTLS configuration. `None` (default) = plaintext server,
    /// accept any client. Required from WS-6 multi-host onward.
    #[serde(default)]
    pub mtls: Option<MtlsConfig>,
    /// Enable the tokio-console gRPC endpoint for async task introspection.
    #[serde(default)]
    pub tokio_console: bool,
    /// Directory where IQ capture files are written. BTS management RPCs
    /// reference this path.
    #[serde(default = "default_iq_capture_dir")]
    pub iq_capture_dir: PathBuf,
}

impl ManagementConfig {
    /// Load a `ManagementConfig` from a JSON file. Missing fields fall
    /// back to defaults (no mTLS, plaintext server, console disabled).
    pub fn load_from_path(path: &Path) -> Result<Self, Error> {
        let merged = cdma_common::config_load::load_json_with_local_override(path)?;
        let cfg: Self = serde_json::from_value(merged)?;
        Ok(cfg)
    }
}

fn validate_traffic_assignment(cfg: &TrafficAssignmentConfig) -> Result<(), Error> {
    if cfg.enable_f_sch && Rc3FschProfile::from_rate_bps(cfg.f_sch_rate_bps).is_none() {
        return Err(format!(
            "bsc.traffic_assignment.f_sch_rate_bps={} is unsupported; F-SCH is supplemental-channel data, not the 9600 bps FCH data rate; supported RC3 F-SCH rates are 19200, 38400, 76800, and 153600",
            cfg.f_sch_rate_bps
        )
        .into());
    }

    for &rc in &cfg.supported_for_rcs {
        if !matches!(rc, 1 | 3) {
            return Err(format!(
                "bsc.traffic_assignment.supported_for_rcs contains unsupported RC {}; only 1 and 3 are currently implemented",
                rc
            )
            .into());
        }
    }

    for &rc in &cfg.supported_rev_rcs {
        if !matches!(rc, 1 | 3) {
            return Err(format!(
                "bsc.traffic_assignment.supported_rev_rcs contains unsupported RC {}; only 1 and 3 are currently implemented",
                rc
            )
            .into());
        }
    }

    for pair in &cfg.preferred_pairs {
        if !matches!((pair.for_rc, pair.rev_rc), (1, 1) | (3, 3)) {
            return Err(format!(
                "bsc.traffic_assignment.preferred_pairs contains unsupported pair ({}, {}); only (1,1) and (3,3) are currently implemented",
                pair.for_rc, pair.rev_rc
            )
            .into());
        }
    }

    let supports_rc1 = cfg.supported_for_rcs.contains(&1) && cfg.supported_rev_rcs.contains(&1);
    let supports_rc3 = cfg.supported_for_rcs.contains(&3) && cfg.supported_rev_rcs.contains(&3);
    if !supports_rc1 && !supports_rc3 {
        return Err(
            "bsc.traffic_assignment must allow at least one implemented RC pair: (1,1) or (3,3)"
                .into(),
        );
    }

    for pair in &cfg.preferred_pairs {
        let allowed = cfg.supported_for_rcs.contains(&pair.for_rc)
            && cfg.supported_rev_rcs.contains(&pair.rev_rc);
        if !allowed {
            return Err(format!(
                "bsc.traffic_assignment.preferred_pairs contains pair ({}, {}) that is excluded by supported_for_rcs/supported_rev_rcs",
                pair.for_rc, pair.rev_rc
            )
            .into());
        }
    }

    Ok(())
}

/// Resolve `CDMA_FREQ`: `overhead.cdma_freq` if set, else derive from
/// the BTS `ChannelPlan`.
pub fn resolved_cdma_freq(
    overhead: &OverheadConfig,
    channel: cdma_common::band_class::ChannelPlan,
) -> u16 {
    overhead
        .cdma_freq
        .unwrap_or_else(|| channel.cdma_freq_field())
}

/// Cross-node validation: BSC `overhead.page_chan` must match the BTS
/// paging channel number.
///
/// Called from bootstrap after both `BtsNodeConfig` and `BscNodeConfig` are
/// loaded. Returns an error describing the mismatch when the two values
/// disagree; replaces the equivalent check that previously lived inside
/// `AppConfig::validate`.
pub fn validate_page_chan_alignment(
    bsc_page_chan: u8,
    bts_paging_channel_number: u8,
) -> Result<(), Error> {
    if bsc_page_chan != bts_paging_channel_number {
        return Err(
            "overhead.page_chan must match bts.runtime.downlink.paging.paging_channel_number"
                .into(),
        );
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn page_chan_mismatch_is_an_error() {
        assert!(validate_page_chan_alignment(1, 1).is_ok());
        assert!(validate_page_chan_alignment(1, 2).is_err());
    }

    #[test]
    fn cdma_freq_resolves_from_channel_plan() {
        use cdma_common::band_class::{BandClass, ChannelPlan};
        let plan = ChannelPlan::new(BandClass::Bc0, 0, 384);
        let mut overhead = OverheadConfig::default();
        overhead.cdma_freq = None;
        assert_eq!(resolved_cdma_freq(&overhead, plan), 384);
        overhead.cdma_freq = Some(100);
        assert_eq!(resolved_cdma_freq(&overhead, plan), 100);
    }

    #[test]
    fn abis_timer_defaults_match_spec_table_8_1() {
        let timers = BscAbisTimers::default();
        assert_eq!(timers.tsetupb_ms, 100);
        assert_eq!(timers.tchanstatb_ms, 500);
        assert_eq!(timers.tdrptgtb_ms, 500);
        assert_eq!(timers.tbstreqb_ms, 500);
    }

    #[test]
    fn f_sch_rate_accepts_supported_supplemental_tiers() {
        for f_sch_rate_bps in [19_200, 38_400, 76_800, 153_600] {
            let mut cfg = TrafficAssignmentConfig::default();
            cfg.enable_f_sch = true;
            cfg.f_sch_rate_bps = f_sch_rate_bps;
            validate_traffic_assignment(&cfg).expect("supported F-SCH rate");
        }
    }

    #[test]
    fn f_sch_rate_rejects_fch_data_rate() {
        let mut cfg = TrafficAssignmentConfig::default();
        cfg.enable_f_sch = true;
        cfg.f_sch_rate_bps = 9_600;

        let err = validate_traffic_assignment(&cfg).expect_err("9600 is FCH data rate, not F-SCH");
        let msg = err.to_string();
        assert!(msg.contains("not the 9600 bps FCH data rate"));
        assert!(msg.contains("19200, 38400, 76800, and 153600"));
    }

    #[test]
    fn f_sch_rate_is_ignored_when_f_sch_disabled() {
        let mut cfg = TrafficAssignmentConfig::default();
        cfg.enable_f_sch = false;
        cfg.f_sch_rate_bps = 9_600;

        validate_traffic_assignment(&cfg).expect("disabled F-SCH ignores rate field");
    }

    #[test]
    fn abis_signaling_default_remote_is_localhost_spec_port() {
        let cfg = BscNodeConfig::default();
        assert_eq!(cfg.abis.remote_addr, "127.0.0.1:5604".parse().unwrap());
    }

    #[test]
    fn abis_signaling_explicit_remote_deserializes() {
        let cfg: BscNodeConfig =
            serde_json::from_str(r#"{ "abis": { "remote_addr": "127.0.0.1:5604" } }"#)
                .expect("deserialize bsc config");
        assert_eq!(cfg.abis.remote_addr, "127.0.0.1:5604".parse().unwrap());
    }

    #[test]
    fn management_default_is_no_mtls() {
        let cfg = ManagementConfig {
            grpc_listen_addr: "127.0.0.1:17016".parse().unwrap(),
            mtls: None,
            tokio_console: false,
            iq_capture_dir: "capture-iq-wav".into(),
        };
        assert!(cfg.mtls.is_none());
        assert!(!cfg.tokio_console);
    }
}
