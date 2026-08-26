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
    /// voice calls stuck in setup. Default: 15 seconds.
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
    /// Enable reverse-FCH gating when the assigned RC supports it and the
    /// mobile requests it. Disabled assignments use the 800 bps FPC cadence.
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
            supported_for_rcs: vec![1, 2, 3],
            supported_rev_rcs: vec![1, 2, 3],
            preferred_pairs: vec![
                RcPairConfig::new(1, 1),
                RcPairConfig::new(2, 2),
                RcPairConfig::new(3, 3),
            ],
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
    15
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
    /// Optional HRPD AN A21 endpoint. When set the BSC opens an A21 client
    /// connection on startup, maintains a HybridIdentityCache from inbound
    /// IdentityBinding / IdentityRelease messages, and consults it from the
    /// paging path to divert HRPD-attached MTs into A21 CrossPageRequest.
    #[serde(default)]
    pub an_a21_addr: Option<SocketAddr>,
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
            traffic_assignment: TrafficAssignmentConfig::default(),
            traffic_retry: TrafficRetryConfig::default(),
            paging_retry: PagingRetryConfig::default(),
            mobile_idle_timeout_s: default_mobile_idle_timeout_s(),
            abis_timers: BscAbisTimers::default(),
            abis: BscAbisConfig::default(),
            bearer: BscBearerConfig::default(),
            voice_bearer_bind_ip: default_voice_bearer_bind_ip(),
            node_id: default_node_id(),
            an_a21_addr: None,
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
        Ok(())
    }
}

fn default_iq_capture_dir() -> PathBuf {
    PathBuf::from("capture-iq-wav")
}

/// mTLS configuration for the management plane. When `None` on
/// `ManagementConfig`, the management server is plaintext and accepts any
/// client.
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
    /// accept any client. Required for multi-host deployments.
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
        if !matches!(rc, 1 | 2 | 3) {
            return Err(format!(
                "bsc.traffic_assignment.supported_for_rcs contains unsupported RC {}; only 1, 2, and 3 are currently implemented",
                rc
            )
            .into());
        }
    }

    for &rc in &cfg.supported_rev_rcs {
        if !matches!(rc, 1 | 2 | 3) {
            return Err(format!(
                "bsc.traffic_assignment.supported_rev_rcs contains unsupported RC {}; only 1, 2, and 3 are currently implemented",
                rc
            )
            .into());
        }
    }

    for pair in &cfg.preferred_pairs {
        if !matches!((pair.for_rc, pair.rev_rc), (1, 1) | (2, 2) | (3, 3)) {
            return Err(format!(
                "bsc.traffic_assignment.preferred_pairs contains unsupported pair ({}, {}); only (1,1), (2,2), and (3,3) are currently implemented",
                pair.for_rc, pair.rev_rc
            )
            .into());
        }
    }

    let supports_rc1 = cfg.supported_for_rcs.contains(&1) && cfg.supported_rev_rcs.contains(&1);
    let supports_rc2 = cfg.supported_for_rcs.contains(&2) && cfg.supported_rev_rcs.contains(&2);
    let supports_rc3 = cfg.supported_for_rcs.contains(&3) && cfg.supported_rev_rcs.contains(&3);
    if !supports_rc1 && !supports_rc2 && !supports_rc3 {
        return Err(
            "bsc.traffic_assignment must allow at least one implemented RC pair: (1,1), (2,2), or (3,3)"
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
    overhead: &cdma_common::overhead::OverheadParameters,
    channel: cdma_common::band_class::ChannelPlan,
) -> u16 {
    overhead
        .cdma_freq
        .unwrap_or_else(|| channel.cdma_freq_field())
}

/// Cross-node validation: BTS `overhead.page_chan` must match
/// `bts.runtime.downlink.paging.paging_channel_number`. Both live in
/// `bts.json` but in different sections, so the bootstrap still
/// double-checks them.
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
        let mut overhead = cdma_common::overhead::OverheadParameters::default();
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
    fn reverse_fch_gating_is_supported_with_dynamic_fpc_cadence() {
        let mut cfg = TrafficAssignmentConfig::default();
        cfg.rev_fch_gating_mode = true;

        validate_traffic_assignment(&cfg).expect("RC3 gated FPC cadence is supported");
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
    fn shipped_bsc_config_admits_rc2() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/bsc.json");
        let cfg: BscNodeConfig =
            serde_json::from_slice(&std::fs::read(path).expect("read config/bsc.json"))
                .expect("parse config/bsc.json");
        assert!(cfg.traffic_assignment.supported_for_rcs.contains(&2));
        assert!(cfg.traffic_assignment.supported_rev_rcs.contains(&2));
        validate_traffic_assignment(&cfg.traffic_assignment).expect("valid traffic policy");
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
