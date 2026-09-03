//! PCF node bootstrap shared by the standalone binary and the in-process
//! launcher, so both wire the node the same way.

use std::path::Path;

use cdma_common::startup::wait_for_shutdown;
use log::info;

use crate::{PcfNodeConfig, spawn_hrpd_pcf_a9_service};

/// PCF node config file name within the config directory.
pub const CONFIG_FILENAME: &str = "pcf.json";

pub use cdma_common::startup::{CONFIG_DIR_ENV, DEFAULT_CONFIG_DIR, resolve_config_dir};

/// Loads this node's config from `config_dir`, starts it, and runs until a
/// shutdown signal arrives.
pub async fn run_node(config_dir: &Path) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config_path = config_dir.join(CONFIG_FILENAME);
    info!("Loading PCF config from {}", config_path.display());
    let config = PcfNodeConfig::load_from_path(&config_path)?;

    let a9_bind_addr = config.a9_bind_addr;
    let a11_peer_addr = config.a11.peer_addr;
    spawn_hrpd_pcf_a9_service(config).await?;
    info!("PCF A9 signaling on {a9_bind_addr}, A11 peer {a11_peer_addr}");

    wait_for_shutdown().await;
    info!("PCF node shutting down");
    Ok(())
}
