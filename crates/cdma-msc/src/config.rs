//! MSC node configuration (loaded from `config/msc.json`).
//!
//! Track-B moves voice/circuit policy ownership here so the BSC no longer
//! sources that policy from `bsc.json`.

use std::{
    net::{Ipv4Addr, SocketAddr},
    path::Path,
};

use serde::{Deserialize, Serialize};

fn default_answer_delay_ms() -> u64 {
    10000
}

fn default_supported_voice_service_options() -> Vec<u16> {
    vec![3, 68, 70, 32768]
}

fn default_voice_gateway_endpoint() -> String {
    "http://127.0.0.1:17015".to_string()
}

fn default_hlr_endpoint() -> String {
    "http://127.0.0.1:17019".to_string()
}

fn default_smsc_endpoint() -> String {
    "http://127.0.0.1:17020".to_string()
}

fn default_packet_endpoint() -> String {
    "http://127.0.0.1:17021".to_string()
}

fn default_media_ringback_enabled() -> bool {
    false
}

fn default_sip_ringback_disable() -> bool {
    false
}

fn default_inbound_sip_msc_ringback() -> bool {
    true
}

fn default_generate_ringback() -> bool {
    true
}

fn default_send_tones_alert() -> bool {
    false
}

fn default_page_retry_cooldown_ms() -> u64 {
    1000
}

fn default_page_retry_max_duration_ms() -> u64 {
    60_000
}

fn default_failure_tone_duration_ms() -> u64 {
    3000
}

/// Ringback cadence selection for MSC-synthesized bearer media.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum MediaRingbackType {
    Nanp,
    Etsi,
}

fn default_media_ringback_type() -> MediaRingbackType {
    MediaRingbackType::Nanp
}

/// Configuration for the external media-gateway client controlled by the MSC.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct VoiceGatewayConfig {
    /// Enable voice-gateway integration.
    pub enabled: bool,
    /// gRPC endpoint for `cdma-voice-gw`.
    #[serde(default = "default_voice_gateway_endpoint")]
    pub endpoint: String,
    /// When true, the stack may fall back to WAV playback if the gateway is
    /// unavailable or inappropriate for the call.
    pub fallback_to_wav: bool,
}

impl Default for VoiceGatewayConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            endpoint: default_voice_gateway_endpoint(),
            fallback_to_wav: true,
        }
    }
}

/// MSC-owned voice/circuit call policy.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceConfig {
    /// Optional WAV file used for local call simulation.
    pub wav_file: Option<String>,
    /// Whether locally synthesized ringback is allowed.
    #[serde(default = "default_media_ringback_enabled")]
    pub media_ringback_enabled: bool,
    /// Ringback cadence to synthesize when enabled.
    #[serde(default = "default_media_ringback_type")]
    pub media_ringback_type: MediaRingbackType,
    /// Tell the caller MS to play ringback while the callee is being alerted.
    /// Disable to keep the caller MS silent during alerting.
    #[serde(default = "default_generate_ringback")]
    pub generate_ringback: bool,
    /// When sending caller-side ringback, also emit A1 `Progress` with
    /// `Signal{0x01 Ringback}` so the MS plays the network-instructed tone.
    #[serde(default = "default_send_tones_alert")]
    pub send_tones_alert: bool,
    /// Suppress MSC-side ringback for voice-gateway calls; rely on SIP early
    /// media / 200 OK instead.
    #[serde(default = "default_sip_ringback_disable")]
    pub sip_ringback_disable: bool,
    /// Generate MSC-side ringback toward the SIP caller for inbound INVITEs
    /// (subscriber custom ringtone if configured in HLR, synthetic NANP
    /// otherwise). Disable to let the SIP trunk provide ringback / early media.
    #[serde(default = "default_inbound_sip_msc_ringback")]
    pub inbound_sip_msc_ringback: bool,
    /// Delay between a BSC page-timeout and the next MSC paging burst, ms.
    #[serde(default = "default_page_retry_cooldown_ms")]
    pub page_retry_cooldown_ms: u64,
    /// Total time MSC will retry MT paging before declaring the call failed, ms.
    /// Must be greater than zero.
    #[serde(default = "default_page_retry_max_duration_ms")]
    pub page_retry_max_duration_ms: u64,
    /// Failure tone playback duration (ms) before ClearCommand; 0 disables.
    #[serde(default = "default_failure_tone_duration_ms")]
    pub failure_tone_duration_ms: u64,
    /// Delay before automatic answer in local simulation paths.
    #[serde(default = "default_answer_delay_ms")]
    pub answer_delay_ms: u64,
    /// Supported voice/circuit service options from the MSC policy point of view.
    #[serde(default = "default_supported_voice_service_options")]
    pub supported_service_options: Vec<u16>,
    /// Local IP address that voice bearer (RTP/circuit) UDP sockets bind to.
    /// Defaults to 127.0.0.1 for single-host deployments. Set to the host's
    /// network-facing IP when the MSC and voice gateway run on separate hosts.
    #[serde(default = "default_voice_bearer_bind_ip")]
    pub voice_bearer_bind_ip: Ipv4Addr,
    /// External media-gateway configuration.
    pub gateway: VoiceGatewayConfig,
}

fn default_voice_bearer_bind_ip() -> Ipv4Addr {
    Ipv4Addr::LOCALHOST
}

impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            wav_file: None,
            media_ringback_enabled: default_media_ringback_enabled(),
            media_ringback_type: default_media_ringback_type(),
            generate_ringback: default_generate_ringback(),
            send_tones_alert: default_send_tones_alert(),
            sip_ringback_disable: default_sip_ringback_disable(),
            inbound_sip_msc_ringback: default_inbound_sip_msc_ringback(),
            page_retry_cooldown_ms: default_page_retry_cooldown_ms(),
            page_retry_max_duration_ms: default_page_retry_max_duration_ms(),
            failure_tone_duration_ms: default_failure_tone_duration_ms(),
            answer_delay_ms: default_answer_delay_ms(),
            supported_service_options: default_supported_voice_service_options(),
            voice_bearer_bind_ip: default_voice_bearer_bind_ip(),
            gateway: VoiceGatewayConfig::default(),
        }
    }
}

impl VoiceConfig {
    /// Returns an immutable snapshot for consumers outside the MSC crate.
    pub fn snapshot(&self) -> VoicePolicySnapshot {
        self.clone().into()
    }

    /// Returns the preferred MT voice service option for MSC-originated paging.
    pub fn default_mobile_terminated_service_option(&self) -> u16 {
        self.snapshot().default_mobile_terminated_service_option()
    }
}

/// Immutable MSC-owned voice-policy view exposed to dependent nodes.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoicePolicySnapshot {
    /// Optional WAV file used for local call simulation.
    pub wav_file: Option<String>,
    /// Whether locally synthesized ringback is allowed.
    pub media_ringback_enabled: bool,
    /// Ringback cadence to synthesize when enabled.
    pub media_ringback_type: MediaRingbackType,
    pub generate_ringback: bool,
    pub send_tones_alert: bool,
    pub sip_ringback_disable: bool,
    pub inbound_sip_msc_ringback: bool,
    pub page_retry_cooldown_ms: u64,
    pub page_retry_max_duration_ms: u64,
    pub failure_tone_duration_ms: u64,
    /// Delay before automatic answer in local simulation paths.
    pub answer_delay_ms: u64,
    /// Supported voice/circuit service options from the MSC policy point of view.
    pub supported_service_options: Vec<u16>,
    /// External media-gateway configuration.
    pub gateway: VoiceGatewayConfig,
}

/// BSC-provided context for an MO voice origination that the MSC uses to make
/// a routing decision.
#[derive(Debug, Clone)]
pub struct MoOriginationContext {
    /// Service option requested by the mobile.
    pub service_option: u16,
    /// Dialed digits extracted from the Origination Message (empty = no digits).
    pub dialed_digits: String,
    /// Whether a registered mobile with a matching phone number exists on the
    /// BSC (enables mobile-to-mobile routing).
    pub has_local_mobile_target: bool,
    /// Whether the external voice gateway client is connected and ready.
    pub gateway_available: bool,
}

/// MSC-owned routing decision for a mobile-originated voice call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MoRoutingDecision {
    /// Route the call to another mobile registered on the same BSC.
    MobileToMobile,
    /// Route the call through the external voice gateway (SIP).
    VoiceGateway,
    /// Play a local WAV file (simulation/test path).
    LocalWavPlayback,
    /// Reject the origination (unsupported SO, gateway unavailable, etc.).
    Rejected {
        /// Human-readable reason for the rejection.
        reason: String,
    },
}

impl VoicePolicySnapshot {
    /// Returns whether the MSC policy allows the given service option.
    pub fn supports_service_option(&self, service_option: u16) -> bool {
        self.supported_service_options.contains(&service_option)
    }

    /// Returns the preferred MT voice service option for MSC-originated paging.
    pub fn default_mobile_terminated_service_option(&self) -> u16 {
        self.supported_service_options
            .iter()
            .copied()
            .find(|so| cdma_voice::VoiceCodec::from_service_option(*so).is_some())
            .unwrap_or(3)
    }

    /// Evaluate an MO voice origination and return the MSC-owned routing
    /// decision. The BSC should execute the returned decision without
    /// applying additional routing policy.
    pub fn evaluate_mo_origination(&self, ctx: &MoOriginationContext) -> MoRoutingDecision {
        if !self.supports_service_option(ctx.service_option)
            || cdma_voice::VoiceCodec::from_service_option(ctx.service_option).is_none()
        {
            return MoRoutingDecision::Rejected {
                reason: format!(
                    "service option {} not supported by MSC voice policy",
                    ctx.service_option
                ),
            };
        }

        if ctx.dialed_digits.is_empty() {
            return MoRoutingDecision::LocalWavPlayback;
        }

        if ctx.has_local_mobile_target {
            return MoRoutingDecision::MobileToMobile;
        }

        if self.gateway.enabled {
            if ctx.gateway_available {
                return MoRoutingDecision::VoiceGateway;
            }
            if self.gateway.fallback_to_wav {
                return MoRoutingDecision::LocalWavPlayback;
            }
            return MoRoutingDecision::Rejected {
                reason: "voice gateway unavailable and WAV fallback disabled".to_string(),
            };
        }

        MoRoutingDecision::LocalWavPlayback
    }
}

impl From<VoiceConfig> for VoicePolicySnapshot {
    fn from(value: VoiceConfig) -> Self {
        Self {
            wav_file: value.wav_file,
            media_ringback_enabled: value.media_ringback_enabled,
            media_ringback_type: value.media_ringback_type,
            generate_ringback: value.generate_ringback,
            send_tones_alert: value.send_tones_alert,
            sip_ringback_disable: value.sip_ringback_disable,
            inbound_sip_msc_ringback: value.inbound_sip_msc_ringback,
            page_retry_cooldown_ms: value.page_retry_cooldown_ms,
            page_retry_max_duration_ms: value.page_retry_max_duration_ms,
            failure_tone_duration_ms: value.failure_tone_duration_ms,
            answer_delay_ms: value.answer_delay_ms,
            supported_service_options: value.supported_service_options,
            gateway: value.gateway,
        }
    }
}

/// MSC-owned voice-policy provider consumed by dependent nodes such as the BSC.
pub trait VoicePolicy: Send + Sync {
    /// Returns the current MSC-owned voice-policy snapshot.
    fn snapshot(&self) -> VoicePolicySnapshot;
}

/// Static in-process voice-policy provider backed by one `VoiceConfig`.
#[derive(Clone, Debug)]
pub struct StaticVoicePolicy {
    snapshot: VoicePolicySnapshot,
}

impl StaticVoicePolicy {
    /// Creates a static policy provider from the configured MSC voice policy.
    pub fn new(config: VoiceConfig) -> Self {
        Self {
            snapshot: config.into(),
        }
    }
}

impl VoicePolicy for StaticVoicePolicy {
    fn snapshot(&self) -> VoicePolicySnapshot {
        self.snapshot.clone()
    }
}

/// One base station the MSC serves. The MSC pulls the node's identity and A1
/// address from this endpoint, so nothing else about the node is configured.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct BaseStationConfig {
    /// gRPC endpoint of the node's management service, e.g.
    /// `http://127.0.0.1:17016` for a BSC.
    pub management_endpoint: String,
    /// Stable name the management API and UI address this base station by,
    /// unique across `base_stations`. Defaults to `management_endpoint`.
    #[serde(default)]
    pub id: Option<String>,
}

impl BaseStationConfig {
    /// The addressable id for this base station: the configured `id`, or the
    /// management endpoint when none is set.
    pub fn id(&self) -> &str {
        self.id.as_deref().unwrap_or(&self.management_endpoint)
    }
}

/// Welcome SMS sent to mobiles on first registration or after inactivity.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct WelcomeSmsConfig {
    /// Whether the welcome SMS feature is enabled.
    pub enabled: bool,
    /// The text to send.
    pub text: String,
    /// Originating number shown on the mobile's screen.
    pub originating_number: String,
    /// Days of inactivity before re-sending the welcome SMS.
    pub inactive_days_threshold: u32,
}

impl Default for WelcomeSmsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            text: String::new(),
            originating_number: "0000".to_string(),
            inactive_days_threshold: 30,
        }
    }
}

/// SMS retry sweep configuration.
///
/// Controls the periodic MSC sweep that re-attempts MT SMS delivery for
/// submissions whose latest delivery attempt has failed. There is no
/// max-attempts cap — submissions retry until delivered, expired by an
/// operator, or marked structurally `Failed`.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SmsRetryConfig {
    /// Whether the retry sweep runs.
    pub enabled: bool,
    /// Minimum age (seconds) of the latest failed attempt before a fresh
    /// delivery attempt is created.
    pub retry_after_secs: u64,
    /// How often the sweep wakes up (seconds).
    pub sweep_interval_secs: u64,
}

impl Default for SmsRetryConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            retry_after_secs: 10,
            sweep_interval_secs: 10,
        }
    }
}

/// OTASP (C.S0016-D) configuration.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct OtaspConfig {
    /// Master switch for OTASP `*228`-style originations.
    pub enabled: bool,
    /// Dialed-digit prefixes that trigger an OTASP session.
    pub feature_codes: Vec<String>,
    /// SPC policy. Only `"leave_default"` is supported today.
    pub spc_policy: String,
    /// Home System Tag (operator banner) settings.
    pub system_tag: SystemTagConfig,
    /// Home network identity written into every provisioned NAM.
    pub home_network: HomeNetworkConfig,
    /// NAM defaults applied to every download.
    pub nam_defaults: NamDefaultsConfig,
    /// MMS URI to push when `writes.mms_uri = true`.
    pub mms: MmsConfig,
    /// Per-block write toggles.
    pub writes: OtaspWritesConfig,
}

impl Default for OtaspConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            feature_codes: vec!["*228".to_string()],
            spc_policy: "leave_default".to_string(),
            system_tag: SystemTagConfig::default(),
            home_network: HomeNetworkConfig::default(),
            nam_defaults: NamDefaultsConfig::default(),
            mms: MmsConfig::default(),
            writes: OtaspWritesConfig::default(),
        }
    }
}

/// MMS URI download settings (C.S0016-D §3.5.12 MMS Parameter Block).
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct MmsConfig {
    /// ASCII URL pushed as MMS URI entry index 0. Empty string disables
    /// the write even when `writes.mms_uri = true`.
    pub uri: String,
}

impl Default for MmsConfig {
    fn default() -> Self {
        Self {
            uri: "http://mmsc.local.1xbts.org/".to_string(),
        }
    }
}

/// Home System Tag operator-configurable banner.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SystemTagConfig {
    pub name: String,
    pub tag_p_rev: u8,
}

impl Default for SystemTagConfig {
    fn default() -> Self {
        Self {
            name: "1xBTS".to_string(),
            tag_p_rev: 1,
        }
    }
}

/// NAM defaults applied verbatim to every Download Request.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct NamDefaultsConfig {
    pub mob_term_home: bool,
    pub mob_term_for_sid: bool,
    pub mob_term_for_nid: bool,
}

impl Default for NamDefaultsConfig {
    fn default() -> Self {
        Self {
            mob_term_home: true,
            mob_term_for_sid: true,
            mob_term_for_nid: true,
        }
    }
}

/// Per-block write toggles. A disabled block is skipped entirely.
/// Per-block opt-in for OTASP `*228` writes. Every flag defaults to
/// `false` so a freshly-installed system reads NAM/PRL/MMS blocks
/// back from the handset, renders them on the session detail page,
/// but never overwrites anything. Operators flip on the specific
/// blocks they want programmed after they've eyeballed a few
/// read-backs.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct OtaspWritesConfig {
    pub cdma_analog_nam: bool,
    pub mdn: bool,
    pub cdma_nam: bool,
    pub home_system_tag: bool,
    /// Push MMS URI Parameters block to handsets that advertise
    /// FEATURE_ID = 0x0A. Requires `otasp.mms.uri` to be set.
    pub mms_uri: bool,
    /// Push the per-subscriber (or default) PRL via SSPR Download
    /// after the read-back. Gated on FEATURE_ID = 0x02 (SSPR).
    pub prl: bool,
}

/// Home network identity written into a subscriber's NAM during OTASP.
///
/// These are properties of the operator's network, not of whichever cell
/// happens to serve the `*228` call. Per C.S0005-C 2.6.5.2 a base station is a
/// member of a system (SID) and a network (NID), so many cells share one pair,
/// and the SID_NID_LIST written here is what decides whether the handset
/// considers itself roaming.
#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct HomeNetworkConfig {
    /// Home SID.
    pub sid: u16,
    /// Home NID. `0` covers every base station not in a specific network.
    pub nid: u16,
}

/// MSC node configuration (loaded from `config/msc.json`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MscNodeConfig {
    /// MSC management gRPC listen address.
    pub mgmt_grpc_addr: SocketAddr,
    /// HLR gRPC endpoint used for subscriber lookups.
    #[serde(default = "default_hlr_endpoint")]
    pub hlr_endpoint: String,
    /// SMSC gRPC endpoint used for SMS submission and delivery tracking.
    #[serde(default = "default_smsc_endpoint")]
    pub smsc_endpoint: String,
    /// Packet core (PDSN) gRPC endpoint the management front door reads packet
    /// sessions from.
    #[serde(default = "default_packet_endpoint")]
    pub packet_endpoint: String,
    /// The base stations this MSC serves, one entry per BSC-like element.
    /// Required, with no default.
    pub base_stations: Vec<BaseStationConfig>,
    /// MSC-owned voice/circuit policy.
    #[serde(default)]
    pub voice: VoiceConfig,
    /// Welcome SMS configuration.
    #[serde(default)]
    pub welcome_sms: WelcomeSmsConfig,
    /// MT SMS retry sweep configuration.
    #[serde(default)]
    pub sms_retry: SmsRetryConfig,
    /// OTASP `*228`-style provisioning.
    #[serde(default)]
    pub otasp: OtaspConfig,
}

impl MscNodeConfig {
    /// Load and validate an `MscNodeConfig` from a JSON file.
    pub fn load_from_path(path: &Path) -> Result<Self, std::io::Error> {
        let merged = cdma_common::config_load::load_json_with_local_override(path)?;
        warn_on_removed_keys(&merged);
        let cfg: Self = serde_json::from_value(merged)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        cfg.validate()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        Ok(cfg)
    }

    /// Validate self-contained MSC invariants.
    pub fn validate(&self) -> Result<(), String> {
        if self.base_stations.is_empty() {
            return Err("msc.base_stations must name at least one base station".to_string());
        }
        for (index, node) in self.base_stations.iter().enumerate() {
            if node.management_endpoint.trim().is_empty() {
                return Err(format!(
                    "msc.base_stations[{index}].management_endpoint must be set"
                ));
            }
        }
        if self.voice.gateway.enabled && self.voice.gateway.endpoint.trim().is_empty() {
            return Err(
                "msc.voice.gateway.endpoint must be set when gateway is enabled".to_string(),
            );
        }
        if self.voice.page_retry_max_duration_ms == 0 {
            return Err("msc.voice.page_retry_max_duration_ms must be > 0".to_string());
        }
        self.otasp.validate()?;
        Ok(())
    }
}

/// Warn about keys that moved out of `msc.json`, which serde would skip
/// silently.
fn warn_on_removed_keys(merged: &serde_json::Value) {
    const REMOVED: [(&str, &str); 2] = [
        (
            "a1_listen_addr",
            "the MSC now dials each base station. List them under `base_stations`",
        ),
        (
            "a1_peers",
            "replaced by `base_stations`, one `management_endpoint` per base station",
        ),
    ];
    let Some(map) = merged.as_object() else {
        return;
    };
    for (key, hint) in REMOVED {
        if map.contains_key(key) {
            log::warn!("config: msc.json key {key:?} is no longer read — {hint}");
        }
    }
}

impl OtaspConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.spc_policy != "leave_default" {
            return Err(format!(
                "msc.otasp.spc_policy must be \"leave_default\" (got {:?})",
                self.spc_policy
            ));
        }
        if self.feature_codes.is_empty() {
            return Err("msc.otasp.feature_codes must be non-empty".to_string());
        }
        if !self.system_tag.name.is_ascii() {
            return Err("msc.otasp.system_tag.name must be ASCII".to_string());
        }
        if self.system_tag.name.len() > 31 {
            return Err(format!(
                "msc.otasp.system_tag.name too long: {} bytes (max 31)",
                self.system_tag.name.len()
            ));
        }
        if self.enabled && self.home_network.sid == 0 {
            log::warn!(
                "msc.otasp.home_network.sid is 0 — every provisioned handset will consider \
                 itself roaming; set it to the SID your cells broadcast"
            );
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> MscNodeConfig {
        MscNodeConfig {
            mgmt_grpc_addr: "127.0.0.1:17017".parse().unwrap(),
            hlr_endpoint: default_hlr_endpoint(),
            smsc_endpoint: default_smsc_endpoint(),
            packet_endpoint: default_packet_endpoint(),
            base_stations: vec![BaseStationConfig {
                management_endpoint: "http://127.0.0.1:17016".to_string(),
                id: None,
            }],
            voice: VoiceConfig::default(),
            welcome_sms: WelcomeSmsConfig::default(),
            sms_retry: SmsRetryConfig::default(),
            otasp: OtaspConfig::default(),
        }
    }

    #[test]
    fn default_validates() {
        let cfg = test_config();
        assert!(cfg.validate().is_ok());
        assert!(!cfg.voice.gateway.enabled);
    }

    #[test]
    fn an_msc_with_no_base_stations_is_rejected() {
        let mut cfg = test_config();
        cfg.base_stations.clear();
        let err = cfg.validate().expect_err("no base stations");
        assert!(err.contains("base_stations"));
    }

    #[test]
    fn shipped_msc_config_prefers_qcelp13() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/msc.json");
        let cfg: MscNodeConfig =
            serde_json::from_slice(&std::fs::read(path).expect("read config/msc.json"))
                .expect("parse config/msc.json");
        assert_eq!(cfg.voice.default_mobile_terminated_service_option(), 32768);
    }

    #[test]
    fn shipped_msc_config_carries_hlr_and_smsc_endpoints() {
        let path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/msc.json");
        let cfg: MscNodeConfig =
            serde_json::from_slice(&std::fs::read(path).expect("read config/msc.json"))
                .expect("parse config/msc.json");
        assert_eq!(cfg.hlr_endpoint, default_hlr_endpoint());
        assert_eq!(cfg.smsc_endpoint, default_smsc_endpoint());
    }

    #[test]
    fn shipped_home_network_matches_the_shipped_cell_overhead() {
        let msc_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/msc.json");
        let cfg: MscNodeConfig =
            serde_json::from_slice(&std::fs::read(msc_path).expect("read config/msc.json"))
                .expect("parse config/msc.json");
        let bts_path = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../config/bts.json");
        let bts: serde_json::Value =
            serde_json::from_slice(&std::fs::read(bts_path).expect("read config/bts.json"))
                .expect("parse config/bts.json");
        // A mismatch provisions handsets that roam on the very cell that
        // programmed them.
        assert_eq!(
            u64::from(cfg.otasp.home_network.sid),
            bts["overhead"]["sid"].as_u64().expect("bts overhead sid")
        );
        assert_eq!(
            u64::from(cfg.otasp.home_network.nid),
            bts["overhead"]["nid"].as_u64().expect("bts overhead nid")
        );
    }

    #[test]
    fn rejects_enabled_voice_gateway_with_empty_endpoint() {
        let mut cfg = test_config();
        cfg.voice.gateway.enabled = true;
        cfg.voice.gateway.endpoint = "   ".to_string();
        let err = cfg.validate().expect_err("expected config error");
        assert!(err.contains("msc.voice.gateway.endpoint must be set"));
    }

    fn test_voice_policy() -> VoicePolicySnapshot {
        VoiceConfig::default().into()
    }

    fn mo_ctx(so: u16, digits: &str, local_target: bool, gw_ready: bool) -> MoOriginationContext {
        MoOriginationContext {
            service_option: so,
            dialed_digits: digits.to_string(),
            has_local_mobile_target: local_target,
            gateway_available: gw_ready,
        }
    }

    #[test]
    fn mo_routing_rejects_unsupported_so() {
        let policy = test_voice_policy();
        let decision = policy.evaluate_mo_origination(&mo_ctx(999, "", false, false));
        assert!(matches!(decision, MoRoutingDecision::Rejected { .. }));
    }

    #[test]
    fn mo_routing_wav_when_no_digits() {
        let policy = test_voice_policy();
        let decision = policy.evaluate_mo_origination(&mo_ctx(3, "", false, false));
        assert_eq!(decision, MoRoutingDecision::LocalWavPlayback);
    }

    #[test]
    fn mo_routing_mobile_to_mobile_when_local_target_exists() {
        let policy = test_voice_policy();
        let decision = policy.evaluate_mo_origination(&mo_ctx(3, "5551234567", true, false));
        assert_eq!(decision, MoRoutingDecision::MobileToMobile);
    }

    #[test]
    fn mo_routing_voice_gateway_when_enabled_and_available() {
        let mut policy = test_voice_policy();
        policy.gateway.enabled = true;
        let decision = policy.evaluate_mo_origination(&mo_ctx(3, "5551234567", false, true));
        assert_eq!(decision, MoRoutingDecision::VoiceGateway);
    }

    #[test]
    fn mo_routing_wav_fallback_when_gateway_enabled_but_unavailable() {
        let mut policy = test_voice_policy();
        policy.gateway.enabled = true;
        policy.gateway.fallback_to_wav = true;
        let decision = policy.evaluate_mo_origination(&mo_ctx(3, "5551234567", false, false));
        assert_eq!(decision, MoRoutingDecision::LocalWavPlayback);
    }

    #[test]
    fn mo_routing_rejected_when_gateway_unavailable_no_fallback() {
        let mut policy = test_voice_policy();
        policy.gateway.enabled = true;
        policy.gateway.fallback_to_wav = false;
        let decision = policy.evaluate_mo_origination(&mo_ctx(3, "5551234567", false, false));
        assert!(matches!(decision, MoRoutingDecision::Rejected { .. }));
    }

    #[test]
    fn otasp_default_disabled_validates() {
        let cfg = test_config();
        assert!(!cfg.otasp.enabled);
        assert!(cfg.validate().is_ok());
    }

    #[test]
    fn otasp_rejects_non_default_spc_policy() {
        let mut cfg = test_config();
        cfg.otasp.spc_policy = "rotate".to_string();
        let err = cfg.validate().expect_err("expected spc_policy error");
        assert!(err.contains("spc_policy"));
    }

    #[test]
    fn otasp_rejects_empty_feature_codes() {
        let mut cfg = test_config();
        cfg.otasp.feature_codes.clear();
        let err = cfg.validate().expect_err("expected feature_codes error");
        assert!(err.contains("feature_codes"));
    }

    #[test]
    fn otasp_rejects_non_ascii_system_tag_name() {
        let mut cfg = test_config();
        cfg.otasp.system_tag.name = "naïve".to_string();
        let err = cfg.validate().expect_err("expected ASCII error");
        assert!(err.contains("ASCII"));
    }

    #[test]
    fn otasp_rejects_overlong_system_tag_name() {
        let mut cfg = test_config();
        cfg.otasp.system_tag.name = "X".repeat(32);
        let err = cfg.validate().expect_err("expected length error");
        assert!(err.contains("too long"));
    }

    #[test]
    fn mo_routing_wav_when_gateway_disabled_and_external_digits() {
        let policy = test_voice_policy();
        let decision = policy.evaluate_mo_origination(&mo_ctx(3, "5551234567", false, false));
        assert_eq!(decision, MoRoutingDecision::LocalWavPlayback);
    }
}
