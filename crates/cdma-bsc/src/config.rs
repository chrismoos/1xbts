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

// The config directory and the BTS filename are defined by the BTS crate so
// both elements name the same file.
pub use cdma_bts::bts::validate_page_chan_alignment;
pub use cdma_bts::startup::BTS_CONFIG_FILENAME;
pub use cdma_common::startup::DEFAULT_CONFIG_DIR;

// Default per-node config filenames within the configured config directory.

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

fn default_service_connect_timeout_ms() -> u64 {
    20000
}

fn default_voice_release_timeout_ms() -> u64 {
    5000
}

/// Supervision timers for a voice traffic channel. They apply only to voice
/// calls. Assignment delivery and MS Ack are supervised for every traffic
/// type by `traffic_retry` and `paging_retry`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceTimeoutConfig {
    /// How long a voice channel may sit assigned before service negotiation
    /// is treated as failed.
    #[serde(default = "default_service_connect_timeout_ms")]
    pub service_connect_timeout_ms: u64,
    /// How long to wait for the release to complete before forcing teardown.
    #[serde(default = "default_voice_release_timeout_ms")]
    pub release_timeout_ms: u64,
}

impl Default for VoiceTimeoutConfig {
    fn default() -> Self {
        Self {
            service_connect_timeout_ms: default_service_connect_timeout_ms(),
            release_timeout_ms: default_voice_release_timeout_ms(),
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

/// Default BTS OAM gRPC endpoint for a single-host run.
pub const DEFAULT_BTS_OAM_ENDPOINT: &str = "http://127.0.0.1:17024";
/// Default MSC A1 signaling address.
pub const DEFAULT_A1_BIND_ADDR: &str = "127.0.0.1:17013";
/// Default HLR gRPC endpoint.
pub const DEFAULT_HLR_ENDPOINT: &str = "http://127.0.0.1:17019";
/// Default SMSC gRPC endpoint.
pub const DEFAULT_SMSC_ENDPOINT: &str = "http://127.0.0.1:17020";
/// Default packet gRPC endpoint the packet-data path drives sessions through.
pub const DEFAULT_PACKET_ENDPOINT: &str = "http://127.0.0.1:17021";

fn default_packet_endpoint() -> String {
    DEFAULT_PACKET_ENDPOINT.to_string()
}

fn default_bts_oam_endpoint() -> String {
    DEFAULT_BTS_OAM_ENDPOINT.to_string()
}

fn default_a1_bind_addr() -> SocketAddr {
    DEFAULT_A1_BIND_ADDR
        .parse()
        .expect("valid default A1 address")
}

fn default_hlr_endpoint() -> String {
    DEFAULT_HLR_ENDPOINT.to_string()
}

fn default_smsc_endpoint() -> String {
    DEFAULT_SMSC_ENDPOINT.to_string()
}

fn default_bts_bearer_bind_addr() -> SocketAddr {
    SocketAddr::new(
        std::net::IpAddr::V4(Ipv4Addr::LOCALHOST),
        cdma_abis::transport::ABIS_BSC_BEARER_PORT,
    )
}

fn default_bts_bearer_remote_addr() -> SocketAddr {
    SocketAddr::new(
        std::net::IpAddr::V4(Ipv4Addr::LOCALHOST),
        cdma_abis::transport::ABIS_BTS_BEARER_PORT,
    )
}

fn default_bts_abis_addr() -> SocketAddr {
    SocketAddr::new(
        std::net::IpAddr::V4(Ipv4Addr::LOCALHOST),
        cdma_abis::transport::ABIS_SIGNALING_PORT,
    )
}

fn default_bts_peers() -> Vec<BtsPeerConfig> {
    vec![BtsPeerConfig::default()]
}

/// One BTS this BSC attaches to. Membership is BSC-owned. The radio
/// parameters for the cell come from the BTS itself at enrollment.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct BtsPeerConfig {
    /// Stable, opaque identifier the management API and UI address this peer
    /// by, independent of the cell it enrolls with. Defaults to
    /// `oam_endpoint` when unset. Must be unique across peers.
    #[serde(default)]
    pub id: Option<String>,
    /// BTS OAM gRPC endpoint. The BSC calls `Enroll` here before bringing
    /// up Abis, and again on every re-attach.
    #[serde(default = "default_bts_oam_endpoint")]
    pub oam_endpoint: String,
    /// BTS Abis signaling (TCP) address. Default `127.0.0.1:5604`.
    #[serde(default = "default_bts_abis_addr")]
    pub abis_addr: SocketAddr,
    /// Local UDP address this BSC binds for the peer's Abis bearer.
    /// Default `127.0.0.1:17022`.
    #[serde(default = "default_bts_bearer_bind_addr")]
    pub bearer_bind_addr: SocketAddr,
    /// Remote BTS Abis bearer address. Default `127.0.0.1:17014`.
    #[serde(default = "default_bts_bearer_remote_addr")]
    pub bearer_remote_addr: SocketAddr,
}

impl Default for BtsPeerConfig {
    fn default() -> Self {
        Self {
            id: None,
            oam_endpoint: default_bts_oam_endpoint(),
            abis_addr: default_bts_abis_addr(),
            bearer_bind_addr: default_bts_bearer_bind_addr(),
            bearer_remote_addr: default_bts_bearer_remote_addr(),
        }
    }
}

impl BtsPeerConfig {
    /// The stable id the management API addresses this peer by: the explicit
    /// `id` when set, otherwise the OAM endpoint.
    pub fn peer_id(&self) -> &str {
        self.id.as_deref().unwrap_or(&self.oam_endpoint)
    }
}

/// Standalone BSC node configuration (loaded from `config/bsc.json`).
///
/// Carries the BTS peers to attach, radio-resource assignment policy, traffic
/// retry and voice supervision timers, and the BSC-side Abis timers. Cell
/// broadcast and paging policy live with the BTS and arrive at enrollment.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct BscNodeConfig {
    /// Radio configuration / service-option policy applied during traffic
    /// channel assignment.
    pub traffic_assignment: TrafficAssignmentConfig,
    /// Forward-traffic ACK timing and retry budget.
    pub traffic_retry: TrafficRetryConfig,
    pub voice_timeouts: VoiceTimeoutConfig,
    /// Evict idle registered mobiles (no access activity and no active
    /// traffic channel) after this many seconds. Default: 3600 (1 hour).
    /// Set to 0 to disable.
    #[serde(default = "default_mobile_idle_timeout_s")]
    pub mobile_idle_timeout_s: u64,
    /// BSC-side Abis timers per A.S0003-A §8 Table 8-1.
    pub abis_timers: BscAbisTimers,
    /// The BTSs this BSC serves, one entry per cell.
    #[serde(default = "default_bts_peers")]
    pub bts_peers: Vec<BtsPeerConfig>,
    /// Address the A1 signaling listener accepts the MSC on. The MSC learns
    /// it from enrollment and dials in.
    #[serde(default = "default_a1_bind_addr")]
    pub a1_bind_addr: SocketAddr,
    /// HLR gRPC endpoint.
    #[serde(default = "default_hlr_endpoint")]
    pub hlr_endpoint: String,
    /// SMSC gRPC endpoint.
    #[serde(default = "default_smsc_endpoint")]
    pub smsc_endpoint: String,
    /// Packet gRPC endpoint the packet-data path drives sessions through.
    #[serde(default = "default_packet_endpoint")]
    pub packet_endpoint: String,
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
            traffic_assignment: TrafficAssignmentConfig::default(),
            traffic_retry: TrafficRetryConfig::default(),
            voice_timeouts: VoiceTimeoutConfig::default(),
            mobile_idle_timeout_s: default_mobile_idle_timeout_s(),
            abis_timers: BscAbisTimers::default(),
            bts_peers: default_bts_peers(),
            a1_bind_addr: default_a1_bind_addr(),
            hlr_endpoint: default_hlr_endpoint(),
            smsc_endpoint: default_smsc_endpoint(),
            packet_endpoint: default_packet_endpoint(),
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
        warn_on_removed_keys(&merged);
        let cfg: Self = serde_json::from_value(merged)?;
        cfg.validate()?;
        Ok(cfg)
    }

    /// Validate self-contained BSC invariants.
    pub fn validate(&self) -> Result<(), Error> {
        validate_traffic_assignment(&self.traffic_assignment)?;
        validate_bts_peers(&self.bts_peers)?;
        Ok(())
    }
}

/// Warn about keys that moved out of `bsc.json`, which serde would skip
/// silently.
fn warn_on_removed_keys(merged: &serde_json::Value) {
    const REMOVED: [(&str, &str); 5] = [
        (
            "abis",
            "its address now lives in a `bts_peers` entry's `abis_addr`",
        ),
        (
            "bearer",
            "its addresses now live in a `bts_peers` entry's `bearer_bind_addr` and `bearer_remote_addr`",
        ),
        (
            "paging_retry",
            "the BTS owns it and supplies it at enrollment; move any tuned value to `bts.json`",
        ),
        ("an_a21_addr", "no element serves A21, remove the key"),
        (
            "msc_a1_addr",
            "the MSC now dials this BSC. Set `a1_bind_addr` to the address it should accept the MSC on",
        ),
    ];
    let Some(map) = merged.as_object() else {
        return;
    };
    for (key, hint) in REMOVED {
        if map.contains_key(key) {
            log::warn!("config: bsc.json key {key:?} is no longer read — {hint}");
        }
    }
}

/// Every peer needs its own OAM endpoint and bearer socket, so duplicates are
/// rejected at load rather than at attach time.
fn validate_bts_peers(peers: &[BtsPeerConfig]) -> Result<(), Error> {
    for (index, peer) in peers.iter().enumerate() {
        if let Some(other) = peers[..index]
            .iter()
            .position(|earlier| earlier.oam_endpoint == peer.oam_endpoint)
        {
            return Err(format!(
                "bsc.bts_peers[{index}].oam_endpoint {} duplicates bts_peers[{other}]",
                peer.oam_endpoint
            )
            .into());
        }
        if let Some(other) = peers[..index]
            .iter()
            .position(|earlier| earlier.bearer_bind_addr == peer.bearer_bind_addr)
        {
            return Err(format!(
                "bsc.bts_peers[{index}].bearer_bind_addr {} duplicates bts_peers[{other}]",
                peer.bearer_bind_addr
            )
            .into());
        }
        // The effective peer id shares one namespace whether it comes from an
        // explicit `id` or defaults to the OAM endpoint, so check them together.
        if let Some(other) = peers[..index]
            .iter()
            .position(|earlier| earlier.peer_id() == peer.peer_id())
        {
            return Err(format!(
                "bsc.bts_peers[{index}] peer id {:?} duplicates bts_peers[{other}]",
                peer.peer_id()
            )
            .into());
        }
    }
    Ok(())
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

#[cfg(test)]
mod tests {
    use super::*;

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
    fn a_single_cell_run_needs_no_peer_configuration() {
        let cfg = BscNodeConfig::default();
        assert_eq!(cfg.bts_peers.len(), 1);
        assert_eq!(
            cfg.bts_peers[0].abis_addr,
            "127.0.0.1:5604".parse().unwrap()
        );
        assert_eq!(cfg.bts_peers[0].oam_endpoint, DEFAULT_BTS_OAM_ENDPOINT);
        assert_eq!(cfg.a1_bind_addr, DEFAULT_A1_BIND_ADDR.parse().unwrap());
    }

    #[test]
    fn a_peer_entry_fills_the_addresses_it_omits() {
        let cfg: BscNodeConfig = serde_json::from_str(
            r#"{ "bts_peers": [{ "abis_addr": "10.0.0.2:5604" }, { "abis_addr": "10.0.0.3:5604" }] }"#,
        )
        .expect("deserialize bsc config");
        assert_eq!(cfg.bts_peers.len(), 2);
        assert_eq!(cfg.bts_peers[1].abis_addr, "10.0.0.3:5604".parse().unwrap());
        assert_eq!(cfg.bts_peers[1].oam_endpoint, DEFAULT_BTS_OAM_ENDPOINT);
    }

    #[test]
    fn two_peers_may_not_share_an_oam_endpoint_or_a_bearer_socket() {
        let mut cfg = BscNodeConfig::default();
        cfg.bts_peers = vec![BtsPeerConfig::default(), BtsPeerConfig::default()];
        let error = cfg
            .validate()
            .expect_err("both peers default to one address");
        assert!(error.to_string().contains("oam_endpoint"), "{error}");

        cfg.bts_peers[1].oam_endpoint = "http://127.0.0.1:17124".to_string();
        let error = cfg
            .validate()
            .expect_err("both peers still bind one bearer socket");
        assert!(error.to_string().contains("bearer_bind_addr"), "{error}");

        cfg.bts_peers[1].bearer_bind_addr = "127.0.0.1:17122".parse().unwrap();
        cfg.validate().expect("distinct peers validate");
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
            iq_capture_dir: "capture-iq-wav".into(),
        };
        assert!(cfg.mtls.is_none());
    }
}
