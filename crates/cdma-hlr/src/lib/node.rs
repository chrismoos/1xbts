//! HLR node bootstrap shared by the standalone binary and the in-process
//! launcher, so both wire the node the same way.

use std::path::Path;

use cdma_common::startup::wait_for_shutdown;
use log::info;

/// HLR node config file name within the config directory.
pub const CONFIG_FILENAME: &str = "hlr.json";

pub use cdma_common::startup::{CONFIG_DIR_ENV, DEFAULT_CONFIG_DIR, resolve_config_dir};

/// Loads this node's config from `config_dir`, starts it, and runs until a
/// shutdown signal arrives.
pub async fn run_node(config_dir: &Path) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config_path = config_dir.join(CONFIG_FILENAME);
    info!("Loading HLR config from {}", config_path.display());
    let config = crate::HlrNodeConfig::load_from_path(&config_path)?;

    let (addr, served) = crate::service::spawn_configured_hlr_service(config).await?;
    info!("HLR gRPC service listening on {addr}");

    // Exit if the listener dies rather than staying up with no service, so a
    // supervisor sees the failure instead of a healthy-looking process.
    tokio::select! {
        _ = wait_for_shutdown() => {
            info!("HLR node shutting down");
            Ok(())
        }
        _ = served => Err("HLR gRPC service stopped".into()),
    }
}
