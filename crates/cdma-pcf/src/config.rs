//! PCF node configuration (loaded from `config/pcf.json`).
//!
//! Empty in WS-0 PR1; fields land alongside the A9/A8 implementation.

use std::path::Path;

use serde::{Deserialize, Serialize};

fn default_packet_grpc_endpoint() -> String {
    "http://127.0.0.1:17021".to_string()
}

/// PCF node configuration (loaded from `config/pcf.json`).
///
/// Empty in WS-0 PR1 — populated alongside the A9/A8 implementation.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct PcfNodeConfig {
    /// Packet/PCF gRPC endpoint consumed by the BSC-facing PCF client.
    #[serde(default = "default_packet_grpc_endpoint")]
    pub packet_grpc_endpoint: String,
}

impl Default for PcfNodeConfig {
    fn default() -> Self {
        Self {
            packet_grpc_endpoint: default_packet_grpc_endpoint(),
        }
    }
}

impl PcfNodeConfig {
    /// Load a `PcfNodeConfig` from a JSON file. An empty object (`{}`)
    /// is the typical PR1 content.
    pub fn load_from_path(path: &Path) -> Result<Self, std::io::Error> {
        let merged = cdma_common::config_load::load_json_with_local_override(path)?;
        serde_json::from_value(merged)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_validates() {
        let _ = PcfNodeConfig::default();
    }
}
