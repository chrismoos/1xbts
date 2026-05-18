use std::error::Error;
use std::fmt;
use std::path::Path;

use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceGatewayConfig {
    pub grpc: GrpcConfig,
    pub sip: SipConfig,
    pub rtp: RtpConfig,
    pub nat: NatConfig,
    pub calls: CallConfig,
    pub queues: QueueConfig,
    pub logging: LoggingConfig,
    pub jitter_buffer_ms: u64,
    pub dtmf_mode: String,
}

impl VoiceGatewayConfig {
    pub fn load_from_path(path: &Path) -> Result<Self, Box<dyn Error + Send + Sync>> {
        let merged = cdma_common::config_load::load_json_with_local_override(path)?;
        let config = serde_json::from_value(merged)?;
        Self::validate(&config)?;
        Ok(config)
    }

    pub fn validate(config: &Self) -> Result<(), String> {
        config
            .grpc
            .listen_addr
            .parse::<std::net::SocketAddr>()
            .map_err(|err| format!("invalid grpc.listen_addr: {err}"))?;

        let sip_listen_addr = config
            .sip
            .listen_addr
            .parse::<std::net::SocketAddr>()
            .map_err(|err| format!("invalid sip.listen_addr: {err}"))?;
        if sip_listen_addr.ip().is_unspecified() {
            return Err(
                "sip.listen_addr must use a concrete local IP address; libre rejects wildcard SIP binds like 0.0.0.0 and ::"
                    .to_string(),
            );
        }
        libre::Transport::try_from(config.sip.transport.as_str())?;
        if !config.sip.request_uri_template.contains("{called}") {
            return Err("sip.request_uri_template must contain {called}".to_string());
        }
        if config.sip.from_domain.trim().is_empty() {
            return Err("sip.from_domain must not be empty".to_string());
        }
        if config
            .sip
            .caller_id_override
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        {
            return Err("sip.caller_id_override must not be empty when set".to_string());
        }
        if config.sip.keepalive_interval_secs > 0 && !config.sip.registration.enabled {
            return Err(
                "sip.keepalive_interval_secs requires sip.registration.enabled=true".to_string(),
            );
        }

        let auth = config.sip.resolved_auth()?;
        if config.sip.registration.enabled
            && auth.is_none()
            && !config.sip.registration.allow_unauthenticated
        {
            return Err(
                "sip.registration requires sip.auth unless allow_unauthenticated=true".to_string(),
            );
        }
        config.sip.resolved_registration(auth.as_ref())?;

        let rtp_listen_ip = config
            .rtp
            .listen_addr
            .parse::<std::net::IpAddr>()
            .map_err(|err| format!("invalid rtp.listen_addr: {err}"))?;
        if rtp_listen_ip.is_unspecified() && config.rtp.advertise_addr.is_none() {
            return Err(
                "rtp.listen_addr is unspecified (0.0.0.0 / ::) and rtp.advertise_addr is not set; \
                 SDP would advertise 127.0.0.1 and media would not flow — \
                 set rtp.advertise_addr to the public IP of this host"
                    .to_string(),
            );
        }
        if let Some(advertise_addr) = config.rtp.advertise_addr.as_deref() {
            advertise_addr
                .parse::<std::net::IpAddr>()
                .map_err(|err| format!("invalid rtp.advertise_addr: {err}"))?;
        }
        let [min_port, max_port] = config.rtp.port_range;
        if min_port > max_port {
            return Err("rtp.port_range minimum must be <= maximum".to_string());
        }
        if rtp_port_capacity(config.rtp.port_range) == 0 {
            return Err("rtp.port_range must contain at least one RTP port".to_string());
        }
        if !config
            .rtp
            .preferred_codecs
            .iter()
            .any(|codec| is_supported_g711_codec(codec))
        {
            return Err("rtp.preferred_codecs must include PCMU or PCMA".to_string());
        }

        let nat_mode = config.nat.mode()?;
        if nat_mode == NatMode::StunLatch {
            let stun_server = config
                .nat
                .stun_server()?
                .expect("stun_server is required for stun_latch");
            validate_host_port("nat.stun_server", stun_server)?;
            if config.nat.stun_timeout_ms == 0 {
                return Err("nat.stun_timeout_ms must be greater than zero".to_string());
            }
        }

        if config.calls.max_concurrent_calls == 0 {
            return Err("calls.max_concurrent_calls must be greater than zero".to_string());
        }
        let port_capacity = rtp_port_capacity(config.rtp.port_range);
        if config.calls.max_concurrent_calls > port_capacity {
            return Err(format!(
                "calls.max_concurrent_calls ({}) exceeds RTP port capacity ({})",
                config.calls.max_concurrent_calls, port_capacity
            ));
        }
        if config.queues.gateway_voice_frames == 0 {
            return Err("queues.gateway_voice_frames must be greater than zero".to_string());
        }
        if config.queues.media_stream_frames == 0 {
            return Err("queues.media_stream_frames must be greater than zero".to_string());
        }
        if config.dtmf_mode.trim().to_ascii_lowercase() != "disabled" {
            return Err("dtmf_mode currently supports only \"disabled\"".to_string());
        }

        Ok(())
    }
}

impl Default for VoiceGatewayConfig {
    fn default() -> Self {
        Self {
            grpc: GrpcConfig::default(),
            sip: SipConfig::default(),
            rtp: RtpConfig::default(),
            nat: NatConfig::default(),
            calls: CallConfig::default(),
            queues: QueueConfig::default(),
            logging: LoggingConfig::default(),
            jitter_buffer_ms: 60,
            dtmf_mode: "disabled".to_string(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct GrpcConfig {
    pub listen_addr: String,
}

impl Default for GrpcConfig {
    fn default() -> Self {
        Self {
            listen_addr: "127.0.0.1:17015".to_string(),
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SipConfig {
    pub listen_addr: String,
    pub transport: String,
    pub request_uri_template: String,
    pub from_domain: String,
    pub caller_id_override: Option<String>,
    pub user_agent: String,
    pub keepalive_interval_secs: u32,
    pub auth: SipAuthConfig,
    pub registration: SipRegistrationConfig,
    /// Auto-reject inbound INVITEs with 408 if MSC hasn't decided in this many ms; 0 disables.
    #[serde(default = "default_inbound_decision_timeout_ms")]
    pub inbound_decision_timeout_ms: u64,
}

fn default_inbound_decision_timeout_ms() -> u64 {
    30_000
}

impl Default for SipConfig {
    fn default() -> Self {
        Self {
            listen_addr: "127.0.0.1:5060".to_string(),
            transport: "udp".to_string(),
            request_uri_template: "sip:{called}@127.0.0.1:5060".to_string(),
            from_domain: "bts.local".to_string(),
            caller_id_override: None,
            user_agent: "1XBTS-VoiceGW/0.1".to_string(),
            keepalive_interval_secs: 0,
            auth: SipAuthConfig::default(),
            registration: SipRegistrationConfig::default(),
            inbound_decision_timeout_ms: default_inbound_decision_timeout_ms(),
        }
    }
}

impl SipConfig {
    pub fn resolved_auth(&self) -> Result<Option<ResolvedSipAuth>, String> {
        self.auth.resolved()
    }

    pub fn resolved_registration(
        &self,
        auth: Option<&ResolvedSipAuth>,
    ) -> Result<Option<ResolvedSipRegistration>, String> {
        self.registration.resolved(auth)
    }

    pub fn effective_caller_id(&self, caller_number: &str) -> String {
        let caller = self
            .caller_id_override
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| caller_number.trim());

        if caller.is_empty() {
            "anonymous".to_string()
        } else {
            caller.to_string()
        }
    }
}

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct SipAuthConfig {
    pub username: String,
    pub password: Option<String>,
    pub password_env: Option<String>,
}

impl fmt::Debug for SipAuthConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SipAuthConfig")
            .field("username", &self.username)
            .field("password", &self.password.as_ref().map(|_| "<redacted>"))
            .field("password_env", &self.password_env)
            .finish()
    }
}

impl SipAuthConfig {
    fn resolved(&self) -> Result<Option<ResolvedSipAuth>, String> {
        let username = self.username.trim();
        let has_password = self
            .password
            .as_ref()
            .is_some_and(|value| !value.is_empty());
        let has_password_env = self
            .password_env
            .as_ref()
            .is_some_and(|value| !value.trim().is_empty());

        if username.is_empty() {
            if has_password || has_password_env {
                return Err("sip.auth.username is required when a SIP auth password is set".into());
            }
            return Ok(None);
        }

        let password = match (&self.password, &self.password_env) {
            (Some(_), Some(env_key)) if !env_key.trim().is_empty() => {
                return Err("set only one of sip.auth.password or sip.auth.password_env".into());
            }
            (Some(password), _) if !password.is_empty() => password.clone(),
            (_, Some(env_key)) if !env_key.trim().is_empty() => std::env::var(env_key)
                .map_err(|_| format!("SIP auth password env var {env_key:?} is not set"))?,
            _ => {
                return Err("sip.auth.password or sip.auth.password_env is required".into());
            }
        };

        Ok(Some(ResolvedSipAuth {
            username: username.to_string(),
            password,
        }))
    }
}

#[derive(Clone, Eq, PartialEq)]
pub struct ResolvedSipAuth {
    pub username: String,
    pub password: String,
}

impl fmt::Debug for ResolvedSipAuth {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ResolvedSipAuth")
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct SipRegistrationConfig {
    pub enabled: bool,
    pub registrar_uri: String,
    pub to_uri: Option<String>,
    pub from_uri: Option<String>,
    pub from_name: Option<String>,
    pub contact_user: Option<String>,
    pub expires_secs: u32,
    pub allow_unauthenticated: bool,
}

impl Default for SipRegistrationConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            registrar_uri: String::new(),
            to_uri: None,
            from_uri: None,
            from_name: None,
            contact_user: None,
            expires_secs: 300,
            allow_unauthenticated: false,
        }
    }
}

impl SipRegistrationConfig {
    fn resolved(
        &self,
        auth: Option<&ResolvedSipAuth>,
    ) -> Result<Option<ResolvedSipRegistration>, String> {
        if !self.enabled {
            return Ok(None);
        }

        let registrar_uri = self.registrar_uri.trim();
        if registrar_uri.is_empty() {
            return Err(
                "sip.registration.registrar_uri is required when registration is enabled".into(),
            );
        }

        if self.expires_secs == 0 {
            return Err("sip.registration.expires_secs must be greater than zero".into());
        }

        let user = self
            .contact_user
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .or_else(|| auth.map(|auth| auth.username.as_str()))
            .ok_or_else(|| {
                "sip.registration.contact_user or sip.auth.username is required when registration is enabled"
                    .to_string()
            })?;
        let domain = sip_uri_domain(registrar_uri);

        Ok(Some(ResolvedSipRegistration {
            registrar_uri: registrar_uri.to_string(),
            to_uri: self
                .to_uri
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| format!("sip:{user}@{domain}")),
            from_uri: self
                .from_uri
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned)
                .unwrap_or_else(|| format!("sip:{user}@{domain}")),
            from_name: self
                .from_name
                .as_deref()
                .map(str::trim)
                .filter(|value| !value.is_empty())
                .map(ToOwned::to_owned),
            contact_user: user.to_string(),
            expires_secs: self.expires_secs,
        }))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ResolvedSipRegistration {
    pub registrar_uri: String,
    pub to_uri: String,
    pub from_uri: String,
    pub from_name: Option<String>,
    pub contact_user: String,
    pub expires_secs: u32,
}

fn sip_uri_domain(uri: &str) -> String {
    let without_scheme = uri
        .strip_prefix("sip:")
        .or_else(|| uri.strip_prefix("sips:"))
        .unwrap_or(uri);
    let without_params = without_scheme.split(';').next().unwrap_or(without_scheme);

    without_params
        .rsplit_once('@')
        .map_or(without_params, |(_, domain)| domain)
        .to_string()
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct RtpConfig {
    pub listen_addr: String,
    pub advertise_addr: Option<String>,
    pub port_range: [u16; 2],
    pub preferred_codecs: Vec<String>,
}

impl Default for RtpConfig {
    fn default() -> Self {
        Self {
            listen_addr: "127.0.0.1".to_string(),
            advertise_addr: None,
            port_range: [17100, 17200],
            preferred_codecs: vec!["PCMU".to_string(), "PCMA".to_string()],
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct NatConfig {
    pub mode: String,
    pub stun_server: Option<String>,
    pub stun_timeout_ms: u64,
    pub rtp_latch_packets: u8,
    pub rtp_latch_interval_ms: u64,
}

impl Default for NatConfig {
    fn default() -> Self {
        Self {
            mode: "disabled".to_string(),
            stun_server: None,
            stun_timeout_ms: 1_000,
            rtp_latch_packets: 5,
            rtp_latch_interval_ms: 20,
        }
    }
}

impl NatConfig {
    pub fn mode(&self) -> Result<NatMode, String> {
        match self.mode.trim().to_ascii_lowercase().as_str() {
            "disabled" | "off" | "none" => Ok(NatMode::Disabled),
            "stun_latch" | "auto" => Ok(NatMode::StunLatch),
            other => Err(format!("unsupported nat.mode {other:?}")),
        }
    }

    pub fn stun_server(&self) -> Result<Option<&str>, String> {
        let mode = self.mode()?;
        if mode == NatMode::Disabled {
            return Ok(None);
        }

        self.stun_server
            .as_deref()
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(Some)
            .ok_or_else(|| "nat.stun_server is required when nat.mode is enabled".to_string())
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NatMode {
    Disabled,
    StunLatch,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct CallConfig {
    pub max_concurrent_calls: usize,
    pub setup_timeout_ms: u64,
    pub ringing_timeout_ms: u64,
    pub media_idle_timeout_ms: u64,
}

impl Default for CallConfig {
    fn default() -> Self {
        Self {
            max_concurrent_calls: 32,
            setup_timeout_ms: 30_000,
            ringing_timeout_ms: 120_000,
            media_idle_timeout_ms: 30_000,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct QueueConfig {
    pub gateway_voice_frames: usize,
    pub media_stream_frames: usize,
}

impl Default for QueueConfig {
    fn default() -> Self {
        Self {
            gateway_voice_frames: 512,
            media_stream_frames: 256,
        }
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct LoggingConfig {
    pub control_events: bool,
    pub media_frames: bool,
    pub media_summary: bool,
    pub sip_events: bool,
    pub sip_trace: bool,
    pub sip_sdp: bool,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            control_events: true,
            media_frames: false,
            media_summary: true,
            sip_events: true,
            sip_trace: false,
            sip_sdp: false,
        }
    }
}

fn rtp_port_capacity(port_range: [u16; 2]) -> usize {
    let [min, max] = port_range;
    if min > max {
        return 0;
    }

    usize::from((max - min) / 2) + 1
}

fn is_supported_g711_codec(codec: &str) -> bool {
    matches!(
        codec.trim().to_ascii_uppercase().as_str(),
        "PCMU" | "G711U" | "ULAW" | "MU-LAW" | "PCMA" | "G711A" | "ALAW" | "A-LAW"
    )
}

fn validate_host_port(field: &str, value: &str) -> Result<(), String> {
    let value = value.trim();
    if value.is_empty() {
        return Err(format!("{field} must not be empty"));
    }

    if let Ok(addr) = value.parse::<std::net::SocketAddr>() {
        if addr.port() == 0 {
            return Err(format!("{field} port must be greater than zero"));
        }
        return Ok(());
    }

    let Some((host, port)) = value.rsplit_once(':') else {
        return Err(format!("{field} must include host:port"));
    };
    if host.trim().is_empty() {
        return Err(format!("{field} host must not be empty"));
    }
    let port = port
        .parse::<u16>()
        .map_err(|_| format!("{field} port must be a valid u16"))?;
    if port == 0 {
        return Err(format!("{field} port must be greater than zero"));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;
    use std::time::{SystemTime, UNIX_EPOCH};

    fn temp_config_path(name: &str) -> std::path::PathBuf {
        let unique = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("time went backwards")
            .as_nanos();
        std::env::temp_dir().join(format!("cdma-voice-gw-{name}-{unique}.json"))
    }

    #[test]
    fn defaults_bind_grpc_and_sip_locally_on_udp_5060() {
        let config = VoiceGatewayConfig::default();

        assert_eq!(config.grpc.listen_addr, "127.0.0.1:17015");
        assert_eq!(config.sip.listen_addr, "127.0.0.1:5060");
        assert_eq!(config.sip.transport, "udp");
        assert_eq!(config.sip.caller_id_override, None);
        assert!(!config.sip.registration.enabled);
        assert_eq!(config.nat.mode().unwrap(), NatMode::Disabled);
        VoiceGatewayConfig::validate(&config).unwrap();
    }

    #[test]
    fn rejects_unspecified_sip_listen_addr() {
        let mut config = VoiceGatewayConfig::default();
        config.sip.listen_addr = "0.0.0.0:5060".to_string();

        let err = VoiceGatewayConfig::validate(&config).expect_err("expected config error");
        assert!(err.contains("sip.listen_addr must use a concrete local IP address"));
    }

    #[test]
    fn defaults_prefer_pcmu_then_pcma() {
        let config = VoiceGatewayConfig::default();

        assert_eq!(config.rtp.port_range, [17100, 17200]);
        assert_eq!(config.rtp.advertise_addr.as_deref(), None);
        assert_eq!(config.rtp.preferred_codecs, ["PCMU", "PCMA"]);
        assert!(config.logging.control_events);
        assert!(!config.logging.media_frames);
        assert!(config.logging.media_summary);
        assert!(config.logging.sip_events);
        assert!(!config.logging.sip_trace);
        assert!(!config.logging.sip_sdp);
        assert_eq!(config.nat.mode, "disabled");
        assert_eq!(config.nat.stun_server, None);
        assert_eq!(config.nat.stun_timeout_ms, 1_000);
        assert_eq!(config.nat.rtp_latch_packets, 5);
        assert_eq!(config.calls.max_concurrent_calls, 32);
        assert_eq!(config.calls.setup_timeout_ms, 30_000);
        assert_eq!(config.calls.ringing_timeout_ms, 120_000);
        assert_eq!(config.calls.media_idle_timeout_ms, 30_000);
        assert_eq!(config.queues.gateway_voice_frames, 512);
        assert_eq!(config.queues.media_stream_frames, 256);
        assert_eq!(config.dtmf_mode, "disabled");
    }

    #[test]
    fn loads_partial_json_config_with_defaults() {
        let path = temp_config_path("partial");
        fs::write(
            &path,
            r#"{
  "grpc": {
    "listen_addr": "127.0.0.1:5099"
  },
  "sip": {
    "request_uri_template": "sip:{called}@fs.local",
    "from_domain": "bsc.local",
    "caller_id_override": "18005550100",
    "auth": {
      "username": "trunk-user",
      "password": "secret"
    },
    "registration": {
      "enabled": true,
      "registrar_uri": "sip:sip.example.com",
      "expires_secs": 120
    }
  },
  "rtp": {
    "advertise_addr": "203.0.113.10"
  },
  "nat": {
    "mode": "stun_latch",
    "stun_server": "stun.example.net:3478"
  },
  "calls": {
    "max_concurrent_calls": 4
  },
  "logging": {
    "media_frames": true,
    "sip_trace": true
  }
}
"#,
        )
        .expect("write voice gateway config");

        let config = VoiceGatewayConfig::load_from_path(&path).expect("load voice gateway config");

        assert_eq!(config.grpc.listen_addr, "127.0.0.1:5099");
        assert_eq!(config.sip.listen_addr, "127.0.0.1:5060");
        assert_eq!(config.sip.request_uri_template, "sip:{called}@fs.local");
        assert_eq!(config.sip.from_domain, "bsc.local");
        assert_eq!(
            config.sip.caller_id_override.as_deref(),
            Some("18005550100")
        );
        assert_eq!(config.sip.effective_caller_id("15551230000"), "18005550100");
        assert!(config.sip.registration.enabled);
        assert!(config.sip.resolved_auth().unwrap().is_some());
        assert_eq!(config.sip.registration.registrar_uri, "sip:sip.example.com");
        assert_eq!(config.sip.registration.expires_secs, 120);
        assert_eq!(config.rtp.advertise_addr.as_deref(), Some("203.0.113.10"));
        assert_eq!(config.rtp.preferred_codecs, ["PCMU", "PCMA"]);
        assert_eq!(config.nat.mode().unwrap(), NatMode::StunLatch);
        assert_eq!(
            config.nat.stun_server().unwrap(),
            Some("stun.example.net:3478")
        );
        assert_eq!(config.nat.rtp_latch_interval_ms, 20);
        assert_eq!(config.calls.max_concurrent_calls, 4);
        assert!(config.logging.control_events);
        assert!(config.logging.media_frames);
        assert!(config.logging.media_summary);
        assert!(config.logging.sip_trace);
        assert!(!config.logging.sip_sdp);

        fs::remove_file(path).ok();
    }

    #[test]
    fn resolves_direct_sip_auth_password() {
        let mut config = SipConfig::default();
        config.auth.username = "trunk-user".to_string();
        config.auth.password = Some("secret".to_string());

        let auth = config.resolved_auth().unwrap().unwrap();
        assert_eq!(auth.username, "trunk-user");
        assert_eq!(auth.password, "secret");
        assert!(format!("{auth:?}").contains("<redacted>"));
        assert!(!format!("{auth:?}").contains("secret"));
    }

    #[test]
    fn rejects_incomplete_sip_auth_config() {
        let mut config = SipConfig::default();
        config.auth.username = "trunk-user".to_string();

        assert!(config.resolved_auth().is_err());
    }

    #[test]
    fn resolves_effective_caller_id_override() {
        let mut config = SipConfig::default();

        assert_eq!(config.effective_caller_id("15551230000"), "15551230000");
        assert_eq!(config.effective_caller_id(" 15551230000 "), "15551230000");
        assert_eq!(config.effective_caller_id("   "), "anonymous");

        config.caller_id_override = Some(" 18005550100 ".to_string());
        assert_eq!(config.effective_caller_id("15551230000"), "18005550100");
    }

    #[test]
    fn resolves_sip_registration_from_auth_username() {
        let mut config = SipConfig::default();
        config.registration.enabled = true;
        config.registration.registrar_uri = "sip:sip.example.com".to_string();
        let auth = ResolvedSipAuth {
            username: "15555550100".to_string(),
            password: "secret".to_string(),
        };

        let registration = config.resolved_registration(Some(&auth)).unwrap().unwrap();

        assert_eq!(registration.registrar_uri, "sip:sip.example.com");
        assert_eq!(registration.to_uri, "sip:15555550100@sip.example.com");
        assert_eq!(registration.from_uri, "sip:15555550100@sip.example.com");
        assert_eq!(registration.contact_user, "15555550100");
        assert_eq!(registration.expires_secs, 300);
    }

    #[test]
    fn rejects_enabled_sip_registration_without_uri_or_user() {
        let mut config = SipConfig::default();
        config.registration.enabled = true;
        config.registration.registrar_uri = "sip:sip.example.com".to_string();

        assert!(config.resolved_registration(None).is_err());

        config.registration.contact_user = Some("15555550100".to_string());
        config.registration.registrar_uri.clear();

        assert!(config.resolved_registration(None).is_err());
    }

    #[test]
    fn validates_nat_mode_and_stun_server() {
        let mut config = NatConfig {
            mode: "auto".to_string(),
            ..NatConfig::default()
        };

        assert!(config.stun_server().is_err());

        config.stun_server = Some("stun.example.net:3478".to_string());
        assert_eq!(config.mode().unwrap(), NatMode::StunLatch);
        assert_eq!(config.stun_server().unwrap(), Some("stun.example.net:3478"));

        config.mode = "bogus".to_string();
        assert!(config.mode().is_err());
    }

    #[test]
    fn validates_gateway_config_edges() {
        let mut config = VoiceGatewayConfig::default();
        config.sip.request_uri_template = "sip:static@example.net".to_string();
        assert!(
            VoiceGatewayConfig::validate(&config)
                .unwrap_err()
                .contains("request_uri_template")
        );

        config = VoiceGatewayConfig::default();
        config.sip.caller_id_override = Some("  ".to_string());
        assert!(
            VoiceGatewayConfig::validate(&config)
                .unwrap_err()
                .contains("caller_id_override")
        );

        config = VoiceGatewayConfig::default();
        config.rtp.port_range = [17_200, 17_100];
        assert!(
            VoiceGatewayConfig::validate(&config)
                .unwrap_err()
                .contains("rtp.port_range")
        );

        config = VoiceGatewayConfig::default();
        config.calls.max_concurrent_calls = 100;
        assert!(
            VoiceGatewayConfig::validate(&config)
                .unwrap_err()
                .contains("max_concurrent_calls")
        );

        config = VoiceGatewayConfig::default();
        config.sip.registration.enabled = true;
        config.sip.registration.registrar_uri = "sip:sip.example.com".to_string();
        assert!(
            VoiceGatewayConfig::validate(&config)
                .unwrap_err()
                .contains("registration requires")
        );

        config.sip.registration.allow_unauthenticated = true;
        config.sip.registration.contact_user = Some("15555550100".to_string());
        VoiceGatewayConfig::validate(&config).unwrap();
    }

    #[test]
    fn example_config_file_loads() {
        let config: VoiceGatewayConfig =
            serde_json::from_str(include_str!("../../../../config/voice-gw.json"))
                .expect("example voice gateway config should parse");

        assert_eq!(config.grpc.listen_addr, "127.0.0.1:17015");
        assert_eq!(config.sip.transport, "udp");
        assert!(config.sip.resolved_auth().unwrap().is_none());
        assert!(config.sip.resolved_registration(None).unwrap().is_none());
        assert_eq!(config.rtp.preferred_codecs, ["PCMU", "PCMA"]);
        assert_eq!(config.nat.mode().unwrap(), NatMode::Disabled);
        assert!(config.logging.media_summary);
        assert_eq!(config.dtmf_mode, "disabled");
        VoiceGatewayConfig::validate(&config).unwrap();
    }
}
