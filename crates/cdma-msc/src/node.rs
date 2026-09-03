//! Startup for the MSC component.
//!
//! One function loads `msc.json`, starts attaching to every configured
//! base station, and runs the call-control runtime with its management gRPC
//! service, so the standalone `cdma-msc` binary and the launcher run the
//! same sequence.

use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

use cdma_common::startup::wait_for_shutdown;
use cdma_hlr::repository::GrpcHlrRepository;
use cdma_smsc::repository::GrpcSmscRepository;
use log::info;

use crate::base_station::BaseStations;
use crate::{MscNodeConfig, MscRuntime, MscRuntimeConfig};

/// MSC node config file name within the config directory.
pub const CONFIG_FILENAME: &str = "msc.json";

pub use cdma_common::startup::{CONFIG_DIR_ENV, DEFAULT_CONFIG_DIR, resolve_config_dir};

/// Listen addresses supplied on the command line, each overriding the value
/// `msc.json` carries.
#[derive(Debug, Default, Clone, Copy)]
pub struct MscCliOverrides {
    /// Management gRPC listener.
    pub mgmt_addr: Option<SocketAddr>,
}

/// Run the MSC node until it is signalled to stop. `mgmt_addr` overrides
/// `mgmt_grpc_addr` from `msc.json`.
///
/// The caller owns logging setup. This only reads config and runs services.
pub async fn run_node(
    config_dir: &Path,
    overrides: MscCliOverrides,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config_path = config_dir.join(CONFIG_FILENAME);
    info!("Loading MSC config from {}", config_path.display());
    let config = MscNodeConfig::load_from_path(&config_path)?;

    // Both repositories run over lazy channels, so the MSC starts whether or
    // not the HLR and SMSC are already listening and picks them up on the
    // first RPC that needs them.
    let hlr_repo: Arc<dyn cdma_hlr::repository::HlrRepository> = Arc::new(
        GrpcHlrRepository::connect_lazy(&config.hlr_endpoint)
            .map_err(|e| format!("invalid HLR endpoint {:?}: {e}", config.hlr_endpoint))?,
    );
    let smsc_repo: Arc<dyn cdma_smsc::repository::SmscRepository> = Arc::new(
        GrpcSmscRepository::connect_lazy(&config.smsc_endpoint)
            .map_err(|e| format!("invalid SMSC endpoint {:?}: {e}", config.smsc_endpoint))?,
    );
    info!(
        "MSC HLR endpoint {}, SMSC endpoint {}",
        config.hlr_endpoint, config.smsc_endpoint
    );

    let base_stations = BaseStations::new(&config.base_stations);
    base_stations.spawn_attach_tasks();
    info!(
        "MSC attaching to {} configured base station(s)",
        config.base_stations.len()
    );

    let mgmt_addr = overrides.mgmt_addr.unwrap_or(config.mgmt_grpc_addr);
    let net_mgmt =
        crate::mgmt_proxy::NetworkManagement::new(&config.base_stations, &config.packet_endpoint);
    let mut runtime_config = MscRuntimeConfig::from_node_config(&config, hlr_repo);
    runtime_config.smsc_repo = Some(smsc_repo);
    let mut runtime = MscRuntime::new(runtime_config);
    info!("MSC management gRPC server on {mgmt_addr}");
    tokio::select! {
        _ = runtime.run_with_grpc(mgmt_addr, base_stations.clone(), net_mgmt) => {
            return Err("MSC runtime stopped".into());
        }
        _ = wait_for_shutdown() => {}
    }

    info!("MSC node shutting down");
    Ok(())
}
