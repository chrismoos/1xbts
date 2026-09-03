//! Event-bus node bootstrap shared by the standalone binary and the
//! launcher, so both wire the node the same way.

use std::path::Path;

use cdma_common::startup::wait_for_shutdown;
use log::info;

use crate::{EventBusConfig, EventBusServer, EventsNodeConfig};

/// Event-bus node config file name within the config directory.
pub const CONFIG_FILENAME: &str = "events.json";

pub use cdma_common::startup::{CONFIG_DIR_ENV, DEFAULT_CONFIG_DIR, resolve_config_dir};

/// Loads this node's config from `config_dir`, starts it, and runs until a
/// shutdown signal arrives.
pub async fn run_node(config_dir: &Path) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config_path = config_dir.join(CONFIG_FILENAME);
    info!("Loading event bus config from {}", config_path.display());
    let config = EventsNodeConfig::load_from_path(&config_path)?;

    let addr = config.grpc_listen_addr;
    let mut bus = EventBusServer::new(EventBusConfig {
        subscriber_queue_capacity: config.subscriber_queue_capacity,
    });
    // The enricher's HLR channel is lazy, so the bus serves immediately and
    // the HLR link comes up whenever the HLR does.
    if let Some(enricher) = crate::build_default_enricher(&config).await? {
        bus = bus.with_enricher(enricher);
        info!(
            "Event bus HLR enrichment enabled via {:?}",
            config.hlr_endpoint
        );
    }
    // Bind before serving so an unusable address fails startup with the
    // cause, rather than as a bare transport error out of the server task.
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| format!("event bus gRPC bind on {addr} failed: {e}"))?;
    let served = tokio::spawn(async move {
        tonic::transport::Server::builder()
            .add_service(bus.into_service())
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
            .await
    });
    info!("Event bus gRPC service listening on {addr}");

    tokio::select! {
        _ = wait_for_shutdown() => {
            info!("Event bus node shutting down");
            Ok(())
        }
        result = served => match result {
            Ok(Err(error)) => Err(format!("event bus gRPC server stopped: {error}").into()),
            _ => Err("event bus gRPC server stopped".into()),
        },
    }
}
