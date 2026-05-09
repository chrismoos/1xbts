//! PDSN node configuration (loaded from `config/pdsn.json`).
//!
//! Currently a thin home for the legacy packet-data transport fields
//! (`tun` vs `fou`) previously living under `cdma-bsc::config::packet`.
//! These will be replaced when the A10 GRE bearer lands; FOU is legacy
//! per `docs/architecture-update/02-code-migration-map.md`.

use std::{net::SocketAddr, path::Path};

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
}

impl Default for PacketTransportConfig {
    fn default() -> Self {
        Self {
            transport: "tun".to_string(),
            fou_remote: None,
            fou_local_port: 17011,
            tun_nat_interface: None,
        }
    }
}

impl PacketTransportConfig {
    pub fn validate(&self) -> Result<(), String> {
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
        }
    }

    #[test]
    fn default_uses_tun_transport() {
        let cfg = test_config();
        assert_eq!(cfg.packet.transport, "tun");
        assert_eq!(cfg.packet.fou_local_port, 17011);
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
}
