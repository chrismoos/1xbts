//! SMSC node configuration (loaded from `config/smsc.json`).

use std::{fs, net::SocketAddr, path::Path};

use serde::{Deserialize, Serialize};

/// SMSC node configuration (loaded from `config/smsc.json`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct SmscNodeConfig {
    /// SMSC gRPC listen address.
    pub grpc_listen_addr: SocketAddr,
    /// PostgreSQL connection string.
    #[serde(default)]
    pub postgres_dsn: Option<String>,
}

impl SmscNodeConfig {
    /// Load an `SmscNodeConfig` from a JSON file.
    pub fn load_from_path(path: &Path) -> Result<Self, std::io::Error> {
        let raw = fs::read_to_string(path)?;
        serde_json::from_str(&raw)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_config() -> SmscNodeConfig {
        SmscNodeConfig {
            grpc_listen_addr: "127.0.0.1:17020".parse().unwrap(),
            postgres_dsn: None,
        }
    }

    #[test]
    fn default_has_no_dsn() {
        let cfg = test_config();
        assert!(cfg.postgres_dsn.is_none());
    }
}
