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

fn default_voice_release_timeout_ms() -> u64 {
    5000
}

fn default_service_connect_timeout_ms() -> u64 {
    20000
}

fn default_supported_voice_service_options() -> Vec<u16> {
    vec![3, 68, 70]
}

fn default_voice_gateway_endpoint() -> String {
    "http://127.0.0.1:17015".to_string()
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
    /// Timeout before the call is force-cleared after release starts.
    #[serde(default = "default_voice_release_timeout_ms")]
    pub release_timeout_ms: u64,
    /// Timeout before assignment/service-connect setup is treated as failed.
    #[serde(default = "default_service_connect_timeout_ms")]
    pub service_connect_timeout_ms: u64,
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
            release_timeout_ms: default_voice_release_timeout_ms(),
            service_connect_timeout_ms: default_service_connect_timeout_ms(),
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
    /// Timeout before the call is force-cleared after release starts.
    pub release_timeout_ms: u64,
    /// Timeout before assignment/service-connect setup is treated as failed.
    pub service_connect_timeout_ms: u64,
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
            release_timeout_ms: value.release_timeout_ms,
            service_connect_timeout_ms: value.service_connect_timeout_ms,
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

/// Static A1 peer reference used by the MSC bootstrap.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(default)]
pub struct A1PeerConfig {
    /// Logical peer identifier.
    pub peer_id: String,
    /// Peer socket address for the A1 transport.
    pub addr: Option<SocketAddr>,
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

/// MSC node configuration (loaded from `config/msc.json`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct MscNodeConfig {
    /// Socket address where the MSC will listen for A1 traffic.
    pub a1_listen_addr: SocketAddr,
    /// MSC management gRPC listen address.
    pub mgmt_grpc_addr: SocketAddr,
    /// Statically configured BSC A1 peers.
    #[serde(default)]
    pub a1_peers: Vec<A1PeerConfig>,
    /// MSC-owned voice/circuit policy.
    #[serde(default)]
    pub voice: VoiceConfig,
    /// Welcome SMS configuration.
    #[serde(default)]
    pub welcome_sms: WelcomeSmsConfig,
    /// MT SMS retry sweep configuration.
    #[serde(default)]
    pub sms_retry: SmsRetryConfig,
}

impl MscNodeConfig {
    /// Load and validate an `MscNodeConfig` from a JSON file.
    pub fn load_from_path(path: &Path) -> Result<Self, std::io::Error> {
        let merged = cdma_common::config_load::load_json_with_local_override(path)?;
        let cfg: Self = serde_json::from_value(merged)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        cfg.validate()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        Ok(cfg)
    }

    /// Validate self-contained MSC invariants.
    pub fn validate(&self) -> Result<(), String> {
        if self.voice.gateway.enabled && self.voice.gateway.endpoint.trim().is_empty() {
            return Err(
                "msc.voice.gateway.endpoint must be set when gateway is enabled".to_string(),
            );
        }
        if self.voice.page_retry_max_duration_ms == 0 {
            return Err("msc.voice.page_retry_max_duration_ms must be > 0".to_string());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> MscNodeConfig {
        MscNodeConfig {
            a1_listen_addr: "127.0.0.1:17013".parse().unwrap(),
            mgmt_grpc_addr: "127.0.0.1:17017".parse().unwrap(),
            a1_peers: Vec::new(),
            voice: VoiceConfig::default(),
            welcome_sms: WelcomeSmsConfig::default(),
            sms_retry: SmsRetryConfig::default(),
        }
    }

    #[test]
    fn default_validates() {
        let cfg = test_config();
        assert_eq!(cfg.a1_listen_addr, "127.0.0.1:17013".parse().unwrap());
        assert!(!cfg.voice.gateway.enabled);
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
    fn mo_routing_wav_when_gateway_disabled_and_external_digits() {
        let policy = test_voice_policy();
        let decision = policy.evaluate_mo_origination(&mo_ctx(3, "5551234567", false, false));
        assert_eq!(decision, MoRoutingDecision::LocalWavPlayback);
    }
}
