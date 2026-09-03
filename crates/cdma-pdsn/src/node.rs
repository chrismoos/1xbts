//! Startup for the PDSN component.
//!
//! One function loads `pdsn.json`, brings up the packet gRPC service, the A11
//! signaling agent and the A10 bearer, then holds them until shutdown, so the
//! standalone `cdma-pdsn` binary and the launcher run the same
//! sequence.

use std::path::Path;
use std::sync::Arc;

use cdma_common::startup::wait_for_shutdown;
use log::info;

use crate::{
    PdsnNodeConfig, build_packet_service_with_sink, packet_grpc_endpoint,
    run_packet_grpc_server_on, spawn_hrpd_pdsn_a11_service,
};

/// PDSN node config file name within the config directory.
pub const CONFIG_FILENAME: &str = "pdsn.json";
/// Source label the PDSN publishes packet-session events under.
const EVENT_PUBLISHER_SOURCE: &str = "pdsn-0";

pub use cdma_common::startup::{CONFIG_DIR_ENV, DEFAULT_CONFIG_DIR, resolve_config_dir};

/// Run the PDSN node until it is signalled to stop.
///
/// The caller owns logging setup. This only reads config and runs services.
pub async fn run_node(config_dir: &Path) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    let config_path = config_dir.join(CONFIG_FILENAME);
    info!("Loading PDSN config from {}", config_path.display());
    let config = PdsnNodeConfig::load_from_path(&config_path)?;
    info!("Packet data transport: {:?}", config.packet.transport);

    let lifecycle_sink: Option<Arc<dyn cdma_packet::session_lifecycle::SessionLifecycleSink>> =
        match config.events_endpoint.as_deref() {
            Some(endpoint) => {
                let publisher =
                    cdma_events::EventPublisher::spawn(cdma_events::EventPublisherConfig::new(
                        endpoint.to_string(),
                        EVENT_PUBLISHER_SOURCE,
                    ))?;
                info!("PDSN packet-session events publishing to {endpoint}");
                Some(Arc::new(crate::events::PdsnLifecycleSink::new(publisher)))
            }
            None => None,
        };

    let packet_service = build_packet_service_with_sink(&config, lifecycle_sink)?;
    let a11_addr = spawn_hrpd_pdsn_a11_service(config.clone(), packet_service.clone()).await?;
    info!("PDSN A11 signaling on {a11_addr}");

    let packet_addr = config.packet_grpc_listen_addr;
    let packet_listener = tokio::net::TcpListener::bind(packet_addr)
        .await
        .map_err(|e| format!("packet gRPC bind on {packet_addr} failed: {e}"))?;
    let packet_service_for_server = packet_service.clone();
    let served = tokio::spawn(async move {
        run_packet_grpc_server_on(packet_listener, (*packet_service_for_server).clone()).await
    });
    info!(
        "Packet gRPC service listening at {}",
        packet_grpc_endpoint(packet_addr)
    );

    tokio::select! {
        _ = wait_for_shutdown() => {
            info!("PDSN node shutting down");
            Ok(())
        }
        result = served => match result {
            Ok(Err(error)) => Err(format!("packet gRPC server stopped: {error}").into()),
            _ => Err("packet gRPC server stopped".into()),
        },
    }
}
