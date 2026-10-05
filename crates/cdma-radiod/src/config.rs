use std::net::SocketAddr;
use std::path::Path;

use cdma_bts::bts::config::BtsRfProfile;
use cdma_common::error::Error;
use serde::Deserialize;

pub const DEFAULT_CONFIG_PATH: &str = "config/radiod_network.json";
pub const DEFAULT_CONTROL_PORT: u16 = 50710;
pub const DEFAULT_DATA_PORT: u16 = 50711;

fn default_tx_sample_rate_hz() -> usize {
    BtsRfProfile::single_carrier().tx_sample_rate_hz
}

fn default_listen() -> SocketAddr {
    SocketAddr::new(
        std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
        DEFAULT_CONTROL_PORT,
    )
}

fn default_data_listen() -> SocketAddr {
    SocketAddr::new(
        std::net::IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED),
        DEFAULT_DATA_PORT,
    )
}

#[derive(Clone, Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RadiodConfig {
    #[serde(default = "default_listen")]
    pub listen: SocketAddr,
    #[serde(default = "default_data_listen")]
    pub data_listen: SocketAddr,
    #[serde(default = "default_tx_sample_rate_hz")]
    pub tx_sample_rate_hz: usize,
}

impl RadiodConfig {
    pub fn load_from_path(path: &Path) -> Result<Self, Error> {
        let merged = cdma_common::config_load::load_json_with_local_override(path)?;
        let config: RadiodConfig = serde_json::from_value(merged)?;
        Ok(config)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generic_config_matches_listener_defaults() {
        let config: RadiodConfig =
            serde_json::from_str(include_str!("../../../config/radiod_network.json")).unwrap();
        let defaults: RadiodConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(config.listen, defaults.listen);
        assert_eq!(config.data_listen, defaults.data_listen);
        assert_eq!(config.tx_sample_rate_hz, defaults.tx_sample_rate_hz);
    }

    #[test]
    fn rejects_embedded_radio_settings() {
        assert!(serde_json::from_str::<RadiodConfig>(r#"{"radio":{"kind":"noop"}}"#).is_err());
    }
}
