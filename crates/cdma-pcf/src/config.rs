//! PCF node configuration (loaded from `config/pcf.json`).
//!
//! Owns packet-core interface endpoints for A8/A9 and A10/A11.

use std::{net::SocketAddr, path::Path};

use cdma_a8::BearerTransportConfig;
use cdma_a11::{A11SecurityConfig, A11TransportConfig};
use serde::{Deserialize, Serialize};

fn socket_addr(s: &str) -> SocketAddr {
    s.parse().expect("static socket address should parse")
}

// Between hosts, A11 uses UDP 699 (A.S0012-D §3.5) and GRE-in-UDP uses 4754.
fn default_a9_bind_addr() -> SocketAddr {
    socket_addr("127.0.0.1:5603")
}

fn default_management_bind_addr() -> SocketAddr {
    socket_addr("127.0.0.1:17047")
}

fn default_a8_bearer() -> BearerTransportConfig {
    BearerTransportConfig::udp_encapsulated_gre(
        socket_addr("127.0.0.1:17041"),
        socket_addr("127.0.0.1:17040"),
    )
}

fn default_a10_bearer() -> BearerTransportConfig {
    BearerTransportConfig::udp_encapsulated_gre(
        socket_addr("127.0.0.1:17042"),
        socket_addr("127.0.0.1:17043"),
    )
}

fn default_a11() -> A11TransportConfig {
    A11TransportConfig::new(
        socket_addr("127.0.0.1:17044"),
        socket_addr("127.0.0.1:17045"),
    )
}

/// PCF node configuration (loaded from `config/pcf.json`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PcfNodeConfig {
    /// A9 signaling bind address for AN/BS Setup-A8 requests.
    #[serde(default = "default_a9_bind_addr")]
    pub a9_bind_addr: SocketAddr,
    /// Management gRPC listener that lists the PCF's A10 connections.
    #[serde(default = "default_management_bind_addr")]
    pub management_bind_addr: SocketAddr,
    /// The AN must use the same one.
    #[serde(default)]
    pub a9_transport: cdma_a9::A9Transport,
    /// Raw GRE binds the `a9_bind_addr` IP.
    #[serde(default = "default_a8_bearer")]
    pub a8_bearer: BearerTransportConfig,
    /// Raw GRE uses the `a11` addresses.
    #[serde(default = "default_a10_bearer")]
    pub a10_bearer: BearerTransportConfig,
    /// A11 signaling endpoint toward the PDSN.
    #[serde(default = "default_a11")]
    pub a11: A11TransportConfig,
    /// A11 PCF/PDSN security association.
    pub a11_security: A11SecurityConfig,
}

impl PcfNodeConfig {
    pub fn validate(&self) -> Result<(), String> {
        if self.a9_bind_addr.ip().is_unspecified() {
            return Err("pcf.a9_bind_addr must not use an unspecified IP".to_string());
        }
        self.a8_bearer.validate("pcf.a8_bearer")?;
        self.a10_bearer.validate("pcf.a10_bearer")?;
        self.a11.validate("pcf.a11")?;
        self.a11_security.validate("pcf.a11_security")?;
        Ok(())
    }

    /// Load a `PcfNodeConfig` from a JSON file.
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

    fn test_a11_security_config() -> A11SecurityConfig {
        // Test fixture only. Live PCF configs must carry a11_security explicitly.
        A11SecurityConfig {
            spi: 256,
            shared_secret_hex: "31786274732d6131312d7368617265642d736563726574".to_string(),
        }
    }

    fn test_config() -> PcfNodeConfig {
        PcfNodeConfig {
            a9_bind_addr: default_a9_bind_addr(),
            management_bind_addr: default_management_bind_addr(),
            a9_transport: cdma_a9::A9Transport::Udp,
            a8_bearer: default_a8_bearer(),
            a10_bearer: default_a10_bearer(),
            a11: default_a11(),
            a11_security: test_a11_security_config(),
        }
    }

    #[test]
    fn test_config_validates() {
        let cfg = test_config();
        cfg.validate().unwrap();
        assert_eq!(cfg.a9_bind_addr, "127.0.0.1:5603".parse().unwrap());
        assert_eq!(
            cfg.a8_bearer.mode,
            cdma_a8::BearerTransportMode::UdpEncapsulatedGre
        );
        assert_eq!(
            cfg.a10_bearer.udp_peer_addr,
            Some("127.0.0.1:17043".parse().unwrap())
        );
        assert_eq!(cfg.a11.bind_addr, "127.0.0.1:17044".parse().unwrap());
    }

    #[test]
    fn raw_gre_bearers_deserialize() {
        let cfg: PcfNodeConfig = serde_json::from_str(
            r#"{
                "a8_bearer": { "mode": "raw_gre" },
                "a10_bearer": { "mode": "raw_gre" },
                "a9_bind_addr": "127.0.0.1:6990",
                "a11": {
                    "bind_addr": "127.0.0.1:6991",
                    "peer_addr": "127.0.0.1:6992"
                },
                "a11_security": {
                    "spi": 256,
                    "shared_secret_hex": "31786274732d6131312d7368617265642d736563726574"
                }
            }"#,
        )
        .unwrap();
        cfg.validate().unwrap();
        assert!(cfg.a8_bearer.udp_bind_addr.is_none());
        assert_eq!(cfg.a9_bind_addr, "127.0.0.1:6990".parse().unwrap());
        assert_eq!(cfg.a11.peer_addr, "127.0.0.1:6992".parse().unwrap());
    }

    #[test]
    fn invalid_bearer_config_is_rejected() {
        let mut cfg = test_config();
        cfg.a8_bearer = BearerTransportConfig::udp_encapsulated_gre(
            "127.0.0.2:4754".parse().unwrap(),
            "127.0.0.1:4754".parse().unwrap(),
        );
        cfg.a8_bearer.udp_peer_addr = None;
        assert!(cfg.validate().unwrap_err().contains("pcf.a8_bearer"));
    }

    #[test]
    fn a11_security_is_required() {
        let err = serde_json::from_str::<PcfNodeConfig>(
            r#"{
                "a9_bind_addr": "127.0.0.1:6990"
            }"#,
        )
        .expect_err("A11 security should be explicit in config");
        assert!(err.to_string().contains("a11_security"));
    }
}
