//! PDSN node configuration (loaded from `config/pdsn.json`).
//!
//! Holds the packet-data transport fields (`tun` vs `fou`). FOU is the
//! no-root transport backend; TUN is the standard path.

use std::{
    net::{Ipv4Addr, SocketAddr},
    path::Path,
};

use base64::{Engine as _, engine::general_purpose::STANDARD as BASE64_STANDARD};
use cdma_a10::BearerTransportConfig;
use cdma_a11::{A11SecurityConfig, A11TransportConfig};
use cdma_packet::mobile_ip::{MobileIpAuthMode, MobileIpConfig, MobileIpSecurityAssociation};
use serde::{Deserialize, Serialize};

pub const DEFAULT_PPP_SESSION_TIMEOUT_SECS: u64 = 30 * 60;
const MIN_ENABLED_MOBILE_IP_ADVERTISEMENT_COUNT: u8 = 1;
const MIN_ENABLED_MOBILE_IP_LIFETIME_SECS: u16 = 1;
const MAX_RESERVED_MOBILE_IP_SPI: u32 = 255;

fn default_ppp_session_timeout_secs() -> u64 {
    DEFAULT_PPP_SESSION_TIMEOUT_SECS
}

fn socket_addr(s: &str) -> SocketAddr {
    s.parse().expect("static socket address should parse")
}

fn default_a10_bearer() -> BearerTransportConfig {
    BearerTransportConfig::udp_encapsulated_gre(
        socket_addr("127.0.0.1:17043"),
        socket_addr("127.0.0.1:17042"),
    )
}

fn default_a11() -> A11TransportConfig {
    A11TransportConfig::new(
        socket_addr("127.0.0.1:17045"),
        socket_addr("127.0.0.1:17044"),
    )
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PacketMobileIpAuthMode {
    Insecure,
    MnHa,
}

impl Default for PacketMobileIpAuthMode {
    fn default() -> Self {
        Self::Insecure
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PacketMobileIpConfig {
    /// Enables local Mobile IPv4 Foreign Agent registration after IPCP.
    pub enabled: bool,
    /// Foreign Agent address announced in MIP Agent Advertisements.
    pub fa_address: Ipv4Addr,
    /// Home Agent address returned in local Registration Replies.
    pub home_agent_address: Ipv4Addr,
    /// Number of unsolicited Agent Advertisements sent after IPCP opens.
    pub advertisement_count: u8,
    /// ICMP Router Advertisement lifetime.
    pub advertisement_lifetime_secs: u16,
    /// Maximum lifetime accepted for Mobile IPv4 registrations.
    pub registration_lifetime_secs: u16,
    /// Registration authentication policy.
    pub auth_mode: PacketMobileIpAuthMode,
    /// SPI selecting the MN-HA security association.
    pub mn_ha_spi: Option<u32>,
    /// Binary MN-HA shared secret encoded as base64.
    pub mn_ha_secret_base64: Option<String>,
    /// Accept MN-AAA credentials without an external AAA verification result.
    pub allow_unverified_mn_aaa: bool,
    /// Optional CIDR for the home-address pool. Defaults to the packet `/24`.
    pub home_address_pool: Option<String>,
}

impl std::fmt::Debug for PacketMobileIpConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("PacketMobileIpConfig")
            .field("enabled", &self.enabled)
            .field("fa_address", &self.fa_address)
            .field("home_agent_address", &self.home_agent_address)
            .field("advertisement_count", &self.advertisement_count)
            .field(
                "advertisement_lifetime_secs",
                &self.advertisement_lifetime_secs,
            )
            .field(
                "registration_lifetime_secs",
                &self.registration_lifetime_secs,
            )
            .field("auth_mode", &self.auth_mode)
            .field("mn_ha_spi", &self.mn_ha_spi)
            .field(
                "mn_ha_secret_base64",
                &self.mn_ha_secret_base64.as_ref().map(|_| "<redacted>"),
            )
            .field("allow_unverified_mn_aaa", &self.allow_unverified_mn_aaa)
            .field("home_address_pool", &self.home_address_pool)
            .finish()
    }
}

impl Default for PacketMobileIpConfig {
    fn default() -> Self {
        let defaults = MobileIpConfig::default();
        Self {
            enabled: defaults.enabled,
            fa_address: defaults.fa_address,
            home_agent_address: defaults.home_agent_address,
            advertisement_count: defaults.advertisement_count,
            advertisement_lifetime_secs: defaults.advertisement_lifetime_secs,
            registration_lifetime_secs: defaults.registration_lifetime_secs,
            auth_mode: PacketMobileIpAuthMode::default(),
            mn_ha_spi: None,
            mn_ha_secret_base64: None,
            allow_unverified_mn_aaa: false,
            home_address_pool: None,
        }
    }
}

impl PacketMobileIpConfig {
    fn validate(&self, gateway_ip: Ipv4Addr) -> Result<(), String> {
        if !self.enabled {
            return Ok(());
        }
        if self.advertisement_count < MIN_ENABLED_MOBILE_IP_ADVERTISEMENT_COUNT {
            return Err("pdsn.packet.mobile_ip.advertisement_count must be greater than zero when Mobile IP is enabled".to_string());
        }
        if self.advertisement_lifetime_secs < MIN_ENABLED_MOBILE_IP_LIFETIME_SECS {
            return Err("pdsn.packet.mobile_ip.advertisement_lifetime_secs must be greater than zero when Mobile IP is enabled".to_string());
        }
        if self.registration_lifetime_secs < MIN_ENABLED_MOBILE_IP_LIFETIME_SECS {
            return Err("pdsn.packet.mobile_ip.registration_lifetime_secs must be greater than zero when Mobile IP is enabled".to_string());
        }
        if self.fa_address.is_unspecified() {
            return Err("pdsn.packet.mobile_ip.fa_address must not be 0.0.0.0".to_string());
        }
        if self.home_agent_address.is_unspecified() {
            return Err("pdsn.packet.mobile_ip.home_agent_address must not be 0.0.0.0".to_string());
        }
        match self.auth_mode {
            PacketMobileIpAuthMode::Insecure => {
                if self.mn_ha_spi.is_some() || self.mn_ha_secret_base64.is_some() {
                    return Err(
                        "pdsn.packet.mobile_ip MN-HA credentials require auth_mode = \"mn_ha\""
                            .to_string(),
                    );
                }
                if self.allow_unverified_mn_aaa {
                    return Err("pdsn.packet.mobile_ip.allow_unverified_mn_aaa requires auth_mode = \"mn_ha\"".to_string());
                }
            }
            PacketMobileIpAuthMode::MnHa => {
                let spi = self.mn_ha_spi.ok_or(
                    "pdsn.packet.mobile_ip.mn_ha_spi is required when auth_mode = \"mn_ha\"",
                )?;
                if spi <= MAX_RESERVED_MOBILE_IP_SPI {
                    return Err(
                        "pdsn.packet.mobile_ip.mn_ha_spi must be greater than 255".to_string()
                    );
                }
                self.decode_mn_ha_secret()?;
            }
        }
        if let Some(pool) = self.home_address_pool.as_deref() {
            let expected = format!(
                "{}.{}.{}.0/24",
                gateway_ip.octets()[0],
                gateway_ip.octets()[1],
                gateway_ip.octets()[2]
            );
            if pool.trim() != expected {
                return Err(format!(
                    "pdsn.packet.mobile_ip.home_address_pool currently supports only the packet /24 ({expected})"
                ));
            }
        }
        Ok(())
    }

    pub fn to_packet_config(
        &self,
        primary_dns: Ipv4Addr,
        secondary_dns: Ipv4Addr,
    ) -> Result<MobileIpConfig, String> {
        let mn_ha_security = match self.auth_mode {
            PacketMobileIpAuthMode::Insecure => None,
            PacketMobileIpAuthMode::MnHa => Some(Box::new(MobileIpSecurityAssociation::new(
                self.mn_ha_spi.ok_or(
                    "pdsn.packet.mobile_ip.mn_ha_spi is required when auth_mode = \"mn_ha\"",
                )?,
                self.decode_mn_ha_secret()?,
            ))),
        };
        Ok(MobileIpConfig {
            enabled: self.enabled,
            fa_address: self.fa_address,
            home_agent_address: self.home_agent_address,
            advertisement_count: self.advertisement_count,
            advertisement_lifetime_secs: self.advertisement_lifetime_secs,
            registration_lifetime_secs: self.registration_lifetime_secs,
            primary_dns,
            secondary_dns,
            auth_mode: match self.auth_mode {
                PacketMobileIpAuthMode::Insecure => MobileIpAuthMode::Insecure,
                PacketMobileIpAuthMode::MnHa => MobileIpAuthMode::MnHa,
            },
            mn_ha_security,
            allow_unverified_mn_aaa: self.allow_unverified_mn_aaa,
        })
    }

    fn decode_mn_ha_secret(&self) -> Result<Vec<u8>, String> {
        let encoded = self.mn_ha_secret_base64.as_deref().ok_or(
            "pdsn.packet.mobile_ip.mn_ha_secret_base64 is required when auth_mode = \"mn_ha\"",
        )?;
        let secret = BASE64_STANDARD.decode(encoded).map_err(|error| {
            format!("pdsn.packet.mobile_ip.mn_ha_secret_base64 is invalid base64: {error}")
        })?;
        if secret.is_empty() {
            return Err(
                "pdsn.packet.mobile_ip.mn_ha_secret_base64 must not decode to an empty value"
                    .to_string(),
            );
        }
        Ok(secret)
    }
}

/// Packet-data transport configuration carried by `PdsnNodeConfig`.
///
/// FOU is the no-root path; TUN is the standard path.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct PacketTransportConfig {
    /// IP transport backend: `"tun"` (default, requires root), `"fou"`
    /// (no root, encapsulates over UDP), or `"fou_tcp"` (no root, uses a
    /// framed TCP relay to a local/nearby helper that owns the FOU socket).
    pub transport: String,
    /// FOU or FOU-TCP remote endpoint address (e.g., `"10.1.2.3:17010"` or
    /// `"127.0.0.1:17012"`). Only used when `transport = "fou"` or
    /// `transport = "fou_tcp"`.
    pub fou_remote: Option<String>,
    /// FOU local UDP port. Only used when `transport = "fou"`.
    pub fou_local_port: u16,
    /// Host egress interface for TUN NAT, e.g. `"eth0"`, `"enp3s0"`, or
    /// `"en0"`. Required when `transport = "tun"`.
    pub tun_nat_interface: Option<String>,
    /// Packet-data gateway IP advertised via IPCP and used as the mobile
    /// subnet gateway. Must be the `.1` address for the configured `/24`.
    pub gateway_ip: Ipv4Addr,
    /// Primary DNS server advertised to mobiles via IPCP.
    pub primary_dns: Ipv4Addr,
    /// Secondary DNS server advertised to mobiles via IPCP.
    pub secondary_dns: Ipv4Addr,
    /// Whether PDSN-originated IPCP Configure-Requests advertise VJ compression.
    pub enable_vj_compression_default: bool,
    /// Mobile IPv4 Foreign Agent behavior after IPCP opens without a peer IP.
    pub mobile_ip: PacketMobileIpConfig,
}

impl Default for PacketTransportConfig {
    fn default() -> Self {
        Self {
            transport: "tun".to_string(),
            fou_remote: None,
            fou_local_port: 17011,
            tun_nat_interface: None,
            gateway_ip: Ipv4Addr::new(10, 55, 0, 1),
            primary_dns: Ipv4Addr::new(10, 55, 0, 1),
            secondary_dns: Ipv4Addr::new(10, 55, 0, 1),
            enable_vj_compression_default: false,
            mobile_ip: PacketMobileIpConfig::default(),
        }
    }
}

impl PacketTransportConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.gateway_ip.octets()[3] != 1 {
            return Err(
                "pdsn.packet.gateway_ip must be the .1 address of a /24 subnet".to_string(),
            );
        }
        self.mobile_ip.validate(self.gateway_ip)?;
        match self.transport.as_str() {
            "tun" => {
                let has_nat_interface = self
                    .tun_nat_interface
                    .as_deref()
                    .map(str::trim)
                    .is_some_and(|iface| !iface.is_empty());
                if !has_nat_interface {
                    return Err(
                        "pdsn.packet.tun_nat_interface is required when transport = \"tun\""
                            .to_string(),
                    );
                }
                Ok(())
            }
            "fou" | "fou_tcp" => Ok(()),
            other => Err(format!(
                "unknown pdsn.packet.transport '{}' (expected \"tun\", \"fou\", or \"fou_tcp\")",
                other
            )),
        }
    }
}

/// PDSN node configuration (loaded from `config/pdsn.json`).
///
/// Wraps the packet-data transport configuration.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PdsnNodeConfig {
    /// Packet gRPC listen address for packet service RPCs.
    pub packet_grpc_listen_addr: SocketAddr,
    /// A10 bearer delivery toward the PCF.
    #[serde(default = "default_a10_bearer")]
    pub a10_bearer: BearerTransportConfig,
    /// A11 signaling endpoint toward the PCF.
    #[serde(default = "default_a11")]
    pub a11: A11TransportConfig,
    /// A11 PCF/PDSN security association.
    pub a11_security: A11SecurityConfig,
    /// Legacy packet-data transport (TUN or FOU). Will be superseded by
    /// the A10 GRE bearer.
    #[serde(default)]
    pub packet: PacketTransportConfig,
    /// How long an open PPP/LCP/IPCP session remains resumable after the
    /// traffic channel closes, measured since last PPP control or IP activity.
    #[serde(default = "default_ppp_session_timeout_secs")]
    pub ppp_session_timeout_secs: u64,
    /// Optional events-bus endpoint (e.g. `"http://127.0.0.1:17023"`). When
    /// set, PDSN publishes packet-session bind/unbind events to the bus.
    #[serde(default)]
    pub events_endpoint: Option<String>,
}

impl PdsnNodeConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.ppp_session_timeout_secs == 0 {
            return Err("pdsn.ppp_session_timeout_secs must be greater than zero".to_string());
        }
        self.a10_bearer.validate("pdsn.a10_bearer")?;
        self.a11.validate("pdsn.a11")?;
        self.a11_security.validate("pdsn.a11_security")?;
        self.packet.validate()?;
        if self.packet.mobile_ip.enabled
            && u64::from(self.packet.mobile_ip.registration_lifetime_secs)
                >= self.ppp_session_timeout_secs
        {
            return Err("pdsn.packet.mobile_ip.registration_lifetime_secs must be less than pdsn.ppp_session_timeout_secs".to_string());
        }
        Ok(())
    }

    /// Load and validate a `PdsnNodeConfig` from a JSON file.
    pub fn load_from_path(path: &Path) -> Result<Self, std::io::Error> {
        let merged = cdma_common::config_load::load_json_with_local_override(path)?;
        let cfg: Self = serde_json::from_value(merged)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        cfg.validate()
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        Ok(cfg)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_A11_SHARED_SECRET_HEX: &str = "31786274732d6131312d7368617265642d736563726574";

    fn test_a11_security_config() -> A11SecurityConfig {
        // Test fixture only. Live PDSN configs must carry a11_security explicitly.
        A11SecurityConfig {
            spi: 256,
            shared_secret_hex: TEST_A11_SHARED_SECRET_HEX.to_string(),
        }
    }

    fn test_config_from_json(json: &str) -> PdsnNodeConfig {
        let mut value: serde_json::Value = serde_json::from_str(json).unwrap();
        value
            .as_object_mut()
            .unwrap()
            .insert("a11_security".to_string(), test_a11_security_value());
        serde_json::from_value(value).expect("config should deserialize")
    }

    fn test_a11_security_value() -> serde_json::Value {
        serde_json::json!({
            "spi": 256,
            "shared_secret_hex": TEST_A11_SHARED_SECRET_HEX
        })
    }

    fn test_config() -> PdsnNodeConfig {
        PdsnNodeConfig {
            packet_grpc_listen_addr: "127.0.0.1:17021".parse().unwrap(),
            a10_bearer: default_a10_bearer(),
            a11: default_a11(),
            a11_security: test_a11_security_config(),
            packet: PacketTransportConfig::default(),
            ppp_session_timeout_secs: DEFAULT_PPP_SESSION_TIMEOUT_SECS,
            events_endpoint: None,
        }
    }

    #[test]
    fn default_uses_tun_transport() {
        let cfg = test_config();
        assert_eq!(cfg.packet.transport, "tun");
        assert_eq!(cfg.packet.fou_local_port, 17011);
        assert_eq!(cfg.packet.gateway_ip, Ipv4Addr::new(10, 55, 0, 1));
        assert_eq!(cfg.packet.primary_dns, Ipv4Addr::new(10, 55, 0, 1));
        assert_eq!(cfg.packet.secondary_dns, Ipv4Addr::new(10, 55, 0, 1));
        assert!(!cfg.packet.enable_vj_compression_default);
        assert!(!cfg.packet.mobile_ip.enabled);
        assert_eq!(cfg.packet.mobile_ip.registration_lifetime_secs, 1200);
        assert_eq!(
            cfg.ppp_session_timeout_secs,
            DEFAULT_PPP_SESSION_TIMEOUT_SECS
        );
        assert_eq!(
            cfg.a10_bearer.udp_bind_addr,
            Some("127.0.0.1:17043".parse().unwrap())
        );
        assert_eq!(cfg.a11.peer_addr, "127.0.0.1:17044".parse().unwrap());
    }

    #[test]
    fn tun_transport_requires_nat_interface() {
        let cfg = test_config();
        let err = cfg
            .validate()
            .expect_err("default tun config should need NAT interface");
        assert!(err.contains("tun_nat_interface"));
    }

    #[test]
    fn tun_transport_rejects_blank_nat_interface() {
        let mut cfg = test_config();
        cfg.packet.tun_nat_interface = Some("  ".to_string());
        let err = cfg
            .validate()
            .expect_err("blank NAT interface should be invalid");
        assert!(err.contains("tun_nat_interface"));
    }

    #[test]
    fn tun_transport_accepts_nat_interface() {
        let mut cfg = test_config();
        cfg.packet.tun_nat_interface = Some("eth0".to_string());
        cfg.validate()
            .expect("configured TUN NAT interface should be valid");
    }

    #[test]
    fn fou_transport_does_not_require_nat_interface() {
        let mut cfg = test_config();
        cfg.packet.transport = "fou_tcp".to_string();
        cfg.validate()
            .expect("FOU TCP should not require TUN NAT interface");
    }

    #[test]
    fn custom_dns_values_deserialize() {
        let cfg = test_config_from_json(
            r#"{
                "packet_grpc_listen_addr": "127.0.0.1:17021",
                "packet": {
                    "transport": "fou_tcp",
                    "fou_remote": "127.0.0.1:17012",
                    "primary_dns": "8.8.8.8",
                    "secondary_dns": "8.8.4.4"
                }
            }"#,
        );
        assert_eq!(cfg.packet.primary_dns, Ipv4Addr::new(8, 8, 8, 8));
        assert_eq!(cfg.packet.secondary_dns, Ipv4Addr::new(8, 8, 4, 4));
        cfg.validate().expect("custom DNS config should validate");
    }

    #[test]
    fn raw_gre_a10_and_custom_a11_deserialize() {
        let cfg = test_config_from_json(
            r#"{
                "packet_grpc_listen_addr": "127.0.0.1:17021",
                "a10_bearer": { "mode": "raw_gre" },
                "a11": {
                    "bind_addr": "127.0.0.1:6992",
                    "peer_addr": "127.0.0.1:6991"
                },
                "packet": { "transport": "fou_tcp", "fou_remote": "127.0.0.1:17012" }
            }"#,
        );
        cfg.validate()
            .expect("raw GRE A10 with unprivileged A11 should validate");
        assert!(cfg.a10_bearer.udp_bind_addr.is_none());
        assert_eq!(cfg.a11.bind_addr, "127.0.0.1:6992".parse().unwrap());
    }

    #[test]
    fn invalid_a10_bearer_config_is_rejected() {
        let mut cfg = test_config();
        cfg.packet.transport = "fou_tcp".to_string();
        cfg.a10_bearer.udp_peer_addr = None;
        assert!(cfg.validate().unwrap_err().contains("pdsn.a10_bearer"));
    }

    #[test]
    fn ppp_session_timeout_deserializes() {
        let cfg = test_config_from_json(
            r#"{
                "packet_grpc_listen_addr": "127.0.0.1:17021",
                "ppp_session_timeout_secs": 900,
                "packet": { "transport": "fou_tcp", "fou_remote": "127.0.0.1:17012" }
            }"#,
        );
        assert_eq!(cfg.ppp_session_timeout_secs, 900);
        cfg.validate().expect("timeout config should validate");
    }

    #[test]
    fn ppp_session_timeout_must_be_nonzero() {
        let mut cfg = test_config();
        cfg.ppp_session_timeout_secs = 0;
        let err = cfg
            .validate()
            .expect_err("zero PPP timeout should be invalid");
        assert!(err.contains("ppp_session_timeout_secs"));
    }

    #[test]
    fn mobile_ip_config_deserializes() {
        let cfg = test_config_from_json(
            r#"{
                "packet_grpc_listen_addr": "127.0.0.1:17021",
                "packet": {
                    "transport": "fou_tcp",
                    "fou_remote": "127.0.0.1:17012",
                    "mobile_ip": {
                        "enabled": true,
                        "fa_address": "10.55.0.1",
                        "home_agent_address": "10.55.0.1",
                        "advertisement_count": 2,
                        "registration_lifetime_secs": 600,
                        "auth_mode": "mn_ha",
                        "mn_ha_spi": 1234,
                        "mn_ha_secret_base64": "c2VjcmV0",
                        "allow_unverified_mn_aaa": true,
                        "home_address_pool": "10.55.0.0/24"
                    }
                }
            }"#,
        );
        cfg.validate().expect("mobile IP config should validate");
        assert!(cfg.packet.mobile_ip.enabled);
        assert_eq!(cfg.packet.mobile_ip.advertisement_count, 2);
        let packet_config = cfg
            .packet
            .mobile_ip
            .to_packet_config(cfg.packet.primary_dns, cfg.packet.secondary_dns)
            .expect("MN-HA config should convert");
        assert_eq!(packet_config.auth_mode, MobileIpAuthMode::MnHa);
        assert_eq!(packet_config.primary_dns, cfg.packet.primary_dns);
        assert_eq!(packet_config.secondary_dns, cfg.packet.secondary_dns);
        assert_eq!(packet_config.mn_ha_security.unwrap().spi, 1234);
        assert!(packet_config.allow_unverified_mn_aaa);
    }

    #[test]
    fn mobile_ip_mn_ha_secret_must_be_valid_base64() {
        let mut cfg = test_config();
        cfg.packet.transport = "fou_tcp".to_string();
        cfg.packet.mobile_ip.enabled = true;
        cfg.packet.mobile_ip.auth_mode = PacketMobileIpAuthMode::MnHa;
        cfg.packet.mobile_ip.mn_ha_spi = Some(1234);
        cfg.packet.mobile_ip.mn_ha_secret_base64 = Some("not base64!".to_string());
        let error = cfg
            .validate()
            .expect_err("invalid MN-HA secret should be rejected");
        assert!(error.contains("mn_ha_secret_base64"));
    }

    #[test]
    fn mobile_ip_lifetime_must_be_below_ppp_cache_timeout() {
        let mut cfg = test_config();
        cfg.packet.transport = "fou_tcp".to_string();
        cfg.packet.mobile_ip.enabled = true;
        cfg.packet.mobile_ip.registration_lifetime_secs = DEFAULT_PPP_SESSION_TIMEOUT_SECS as u16;
        let err = cfg
            .validate()
            .expect_err("MIP lifetime should be shorter than PPP cache timeout");
        assert!(err.contains("registration_lifetime_secs"));
    }

    #[test]
    fn invalid_dns_rejected_by_deserializer() {
        let err = serde_json::from_str::<PdsnNodeConfig>(
            r#"{
                "packet_grpc_listen_addr": "127.0.0.1:17021",
                "a11_security": {
                    "spi": 256,
                    "shared_secret_hex": "31786274732d6131312d7368617265642d736563726574"
                },
                "packet": { "primary_dns": "not-an-ip" }
            }"#,
        )
        .expect_err("invalid DNS address should fail JSON config parsing");
        assert!(err.to_string().contains("not-an-ip") || err.to_string().contains("IPv4"));
    }

    #[test]
    fn a11_security_is_required() {
        let err = serde_json::from_str::<PdsnNodeConfig>(
            r#"{
                "packet_grpc_listen_addr": "127.0.0.1:17021"
            }"#,
        )
        .expect_err("A11 security should be explicit in config");
        assert!(err.to_string().contains("a11_security"));
    }

    #[test]
    fn gateway_must_be_first_host() {
        let mut cfg = test_config();
        cfg.packet.gateway_ip = Ipv4Addr::new(10, 55, 0, 2);
        let err = cfg.validate().expect_err("gateway .2 should be rejected");
        assert!(err.contains("gateway_ip"));
    }
}
