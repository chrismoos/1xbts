//! HLR node configuration (loaded from `config/hlr.json`).

use std::{net::SocketAddr, path::Path};

use serde::{Deserialize, Serialize};

/// HLR node configuration (loaded from `config/hlr.json`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HlrNodeConfig {
    /// HLR gRPC listen address.
    pub grpc_listen_addr: SocketAddr,
    /// PostgreSQL connection string.
    #[serde(default)]
    pub postgres_dsn: Option<String>,
}

impl HlrNodeConfig {
    /// Load an `HlrNodeConfig` from a JSON file.
    pub fn load_from_path(path: &Path) -> Result<Self, std::io::Error> {
        let merged = cdma_common::config_load::load_json_with_local_override(path)?;
        serde_json::from_value(merged)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> HlrNodeConfig {
        HlrNodeConfig {
            grpc_listen_addr: "127.0.0.1:17019".parse().unwrap(),
            postgres_dsn: None,
        }
    }

    #[test]
    fn default_has_no_dsn() {
        let cfg = test_config();
        assert!(cfg.postgres_dsn.is_none());
    }
}
