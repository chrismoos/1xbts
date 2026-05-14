//! PDSN node configuration (loaded from `config/pdsn.json`).
//!
//! Currently a thin home for the legacy packet-data transport fields
//! (`tun` vs `fou`) previously living under `cdma-bsc::config::packet`.
//! These will be replaced when the A10 GRE bearer lands; FOU is legacy
//! per `docs/architecture-update/02-code-migration-map.md`.

use std::{
    net::{Ipv4Addr, SocketAddr},
    path::Path,
};

use serde::{Deserialize, Serialize};

/// Legacy packet-data transport configuration carried by `PdsnNodeConfig`.
///
/// FOU is the legacy/no-root path; TUN is the standard path. Both are
/// scheduled for replacement by the A10 GRE bearer per
/// `docs/architecture-update/02-code-migration-map.md`.
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
/// Currently a thin wrapper around the legacy `PacketTransportConfig`. A11
/// signaling, A10 GRE bearer, IP allocation, and TUN/host I/O fields land
/// in WS-3 / WS-4.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct PdsnNodeConfig {
    /// Packet gRPC listen address for packet service RPCs.
    pub packet_grpc_listen_addr: SocketAddr,
    /// Legacy packet-data transport (TUN or FOU). Will be superseded by
    /// the A10 GRE bearer.
    #[serde(default)]
    pub packet: PacketTransportConfig,
    /// Optional events-bus endpoint (e.g. `"http://127.0.0.1:17023"`). When
    /// set, PDSN publishes packet-session bind/unbind events to the bus.
    #[serde(default)]
    pub events_endpoint: Option<String>,
}

impl PdsnNodeConfig {
    pub fn validate(&self) -> Result<(), String> {
        self.packet.validate()
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

    fn test_config() -> PdsnNodeConfig {
        PdsnNodeConfig {
            packet_grpc_listen_addr: "127.0.0.1:17021".parse().unwrap(),
            packet: PacketTransportConfig::default(),
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
        let cfg: PdsnNodeConfig = serde_json::from_str(
            r#"{
                "packet_grpc_listen_addr": "127.0.0.1:17021",
                "packet": {
                    "transport": "fou_tcp",
                    "fou_remote": "127.0.0.1:17012",
                    "primary_dns": "8.8.8.8",
                    "secondary_dns": "8.8.4.4"
                }
            }"#,
        )
        .expect("config should deserialize");
        assert_eq!(cfg.packet.primary_dns, Ipv4Addr::new(8, 8, 8, 8));
        assert_eq!(cfg.packet.secondary_dns, Ipv4Addr::new(8, 8, 4, 4));
        cfg.validate().expect("custom DNS config should validate");
    }

    #[test]
    fn invalid_dns_rejected_by_deserializer() {
        let err = serde_json::from_str::<PdsnNodeConfig>(
            r#"{
                "packet_grpc_listen_addr": "127.0.0.1:17021",
                "packet": { "primary_dns": "not-an-ip" }
            }"#,
        )
        .expect_err("invalid DNS address should fail JSON config parsing");
        assert!(err.to_string().contains("not-an-ip") || err.to_string().contains("IPv4"));
    }

    #[test]
    fn gateway_must_be_first_host() {
        let mut cfg = test_config();
        cfg.packet.gateway_ip = Ipv4Addr::new(10, 55, 0, 2);
        let err = cfg.validate().expect_err("gateway .2 should be rejected");
        assert!(err.contains("gateway_ip"));
    }
}
