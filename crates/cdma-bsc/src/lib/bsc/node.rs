//! Startup for the BSC component.
//!
//! One function brings up everything a BSC process owns — the BTS registry and
//! its attach tasks, the call-control runtime, and the management gRPC server
//! — so the standalone `cdma-bsc` binary and the all-in-one launcher run the
//! same sequence.

use std::{path::Path, sync::Arc, time::Duration};

use cdma_common::error::Error;
use cdma_common::startup::wait_for_shutdown;
use cdma_hlr::repository::{GrpcHlrRepository, HlrRepository};
use cdma_smsc::repository::{GrpcSmscRepository, SmscRepository};
use log::{info, warn};

use crate::{
    a1_edge::{MscClient, network::NetworkMscClient},
    config::{BSC_CONFIG_FILENAME, BscNodeConfig, MANAGEMENT_CONFIG_FILENAME, ManagementConfig},
    grpc::{BscState, run_grpc_server},
    packet::GrpcPcfClient,
};

use super::{BscLaunchInputs, BtsRegistry, build_bsc_launch_parts, spawn_bts_attach};

/// First delay after a failed gRPC connect.
const CONNECT_RETRY_MIN: Duration = Duration::from_secs(1);
/// Ceiling the connect delay backs off to.
const CONNECT_RETRY_MAX: Duration = Duration::from_secs(30);

/// Per-process inputs the BSC cannot read out of `bsc.json`.
pub struct BscNodeOptions {
    pub hlr_repo: Arc<dyn HlrRepository>,
    /// Read by the management layer for SMS history. The MSC owns SMS
    /// coordination.
    pub smsc_repo: Arc<dyn SmscRepository>,
    pub msc_client: Arc<dyn MscClient>,
    /// Supervision timers for voice traffic channels.
    pub voice_timeouts: crate::config::VoiceTimeoutConfig,
    /// Packet gRPC endpoint the packet-data path drives sessions through.
    pub packet_endpoint: String,
    /// Address the A1 listener accepts the MSC on, reported at enrollment.
    pub a1_bind_addr: std::net::SocketAddr,
    pub management: ManagementConfig,
}

pub struct BscNode {
    /// Live BSC state, also served by the management gRPC server.
    pub state: Arc<BscState>,
}

/// Attach to every configured BTS, start call control, and serve the
/// management gRPC.
///
/// The attach tasks retry forever, so a cell that is not up yet joins when it
/// starts and nothing here blocks on it.
pub fn start_bsc_node(config: &BscNodeConfig, options: BscNodeOptions) -> BscNode {
    let bts = BtsRegistry::with_peers(&config.bts_peers);
    let (access_event_tx, access_event_rx) = tokio::sync::mpsc::unbounded_channel();
    let (cell_detach_tx, cell_detach_rx) = tokio::sync::mpsc::unbounded_channel();
    spawn_bts_attach(
        config.bts_peers.clone(),
        bts.clone(),
        access_event_tx,
        cell_detach_tx,
    );
    info!(
        "BSC attaching to {} configured BTS peer(s)",
        config.bts_peers.len()
    );

    let parts = build_bsc_launch_parts(BscLaunchInputs {
        bts,
        traffic_assignment: config.traffic_assignment.clone(),
        traffic_retry: config.traffic_retry.clone(),
        mobile_idle_timeout_s: config.mobile_idle_timeout_s,
        access_event_rx,
        cell_detach_rx,
        hlr_repo: options.hlr_repo,
        smsc_repo: options.smsc_repo,
        packet_endpoint: options.packet_endpoint.clone(),
        a1_bind_addr: options.a1_bind_addr,
        msc_client: options.msc_client,
        voice_timeouts: options.voice_timeouts,
        pcf_client: Arc::new(GrpcPcfClient::new(options.packet_endpoint)),
        voice_bearer_bind_ip: config.voice_bearer_bind_ip,
        node_id: config.node_id.clone(),
    });
    let state = parts.state.clone();

    let bsc = parts.bsc;
    tokio::spawn(async move {
        if let Err(e) = bsc.run().await {
            log::error!("BSC fatal error: {e}");
            std::process::exit(1);
        }
    });

    let grpc_addr = options.management.grpc_listen_addr;
    let grpc_mtls = options.management.mtls.clone();
    let grpc_state = state.clone();
    tokio::spawn(async move {
        // Exit if the listener dies rather than staying up with no management
        // plane, so a supervisor sees the failure instead of a healthy-looking
        // process.
        match run_grpc_server(grpc_state, grpc_addr, grpc_mtls).await {
            Ok(()) => log::error!("BSC gRPC server on {grpc_addr} stopped"),
            Err(e) => log::error!("BSC gRPC server error: {e}"),
        }
        std::process::exit(1);
    });
    info!("BSC management gRPC server on {grpc_addr}");

    BscNode { state }
}

/// Connect with backoff so the BSC can start before its peer.
async fn connect_with_backoff<T, E, F, Fut>(label: &str, endpoint: &str, connect: F) -> T
where
    F: Fn() -> Fut,
    Fut: std::future::Future<Output = Result<T, E>>,
    E: std::fmt::Display,
{
    let mut retry = CONNECT_RETRY_MIN;
    loop {
        match connect().await {
            Ok(value) => {
                info!("BSC: {label} client connected to {endpoint}");
                return value;
            }
            Err(e) => {
                warn!("BSC: {label} connect to {endpoint} failed: {e}; retrying in {retry:?}");
                tokio::time::sleep(retry).await;
                retry = (retry * 2).min(CONNECT_RETRY_MAX);
            }
        }
    }
}

/// Connect to the HLR, retrying with backoff so the BSC can start before it.
pub async fn connect_hlr_with_backoff(endpoint: &str) -> Arc<dyn HlrRepository> {
    let repo = connect_with_backoff("HLR", endpoint, || {
        GrpcHlrRepository::connect(endpoint.to_string())
    })
    .await;
    Arc::new(repo)
}

/// Connect to the SMSC, retrying with backoff so the BSC can start before it.
pub async fn connect_smsc_with_backoff(endpoint: &str) -> Arc<dyn SmscRepository> {
    let repo = connect_with_backoff("SMSC", endpoint, || {
        GrpcSmscRepository::connect(endpoint.to_string())
    })
    .await;
    Arc::new(repo)
}

/// Load the configs, connect to peers, start the node, and hold it open
/// until the process is signalled. The caller owns logging setup.
pub async fn run_node(config_dir: &Path) -> Result<(), Error> {
    let bsc_config = BscNodeConfig::load_from_path(&config_dir.join(BSC_CONFIG_FILENAME))?;
    let mgmt_config =
        ManagementConfig::load_from_path(&config_dir.join(MANAGEMENT_CONFIG_FILENAME))?;
    info!("Loading per-node configs from {}", config_dir.display());

    let hlr_repo = connect_hlr_with_backoff(&bsc_config.hlr_endpoint).await;
    let smsc_repo = connect_smsc_with_backoff(&bsc_config.smsc_endpoint).await;

    let msc_client = NetworkMscClient::bind(bsc_config.a1_bind_addr)
        .await
        .map_err(|e| Error::from(format!("failed to bind the BSC A1 listener: {e}")))?;
    info!(
        "BSC A1 signaling listener bound on {}",
        msc_client.local_addr()?
    );

    let node = start_bsc_node(
        &bsc_config,
        BscNodeOptions {
            hlr_repo,
            smsc_repo,
            msc_client: Arc::new(msc_client),
            voice_timeouts: bsc_config.voice_timeouts.clone(),
            packet_endpoint: bsc_config.packet_endpoint.clone(),
            a1_bind_addr: bsc_config.a1_bind_addr,
            management: mgmt_config,
        },
    );
    info!(
        "BSC running: node_id={} packet={}",
        bsc_config.node_id, node.state.packet_endpoint
    );

    // The runtime and management server run as tasks. Hold the process open
    // until it is signalled.
    wait_for_shutdown().await;
    info!("BSC node shutting down");
    Ok(())
}
