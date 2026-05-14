use std::net::SocketAddr;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// Aggregated event-bus node configuration (loaded from `config/events.json`).
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct EventsNodeConfig {
    /// gRPC listen address for the `EventService`.
    pub grpc_listen_addr: SocketAddr,
    /// Per-subscriber mpsc capacity. Slow subscribers exceeding this are
    /// disconnected from `ListenEvents`.
    #[serde(default = "default_queue_capacity")]
    pub subscriber_queue_capacity: usize,
    /// HLR gRPC endpoint (e.g. `"http://127.0.0.1:17019"`). When set, the
    /// bus stands up a `CachingHlrEnricher` and uses it to fill in
    /// subscriber/identity fields on each event before fan-out. When
    /// `None`, events flow through with only the fields producers
    /// supplied.
    #[serde(default)]
    pub hlr_endpoint: Option<String>,
}

fn default_queue_capacity() -> usize {
    1024
}

impl EventsNodeConfig {
    pub fn load_from_path(path: &Path) -> Result<Self, std::io::Error> {
        let bytes = std::fs::read(path)?;
        serde_json::from_slice(&bytes)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
    }

    pub fn endpoint_url(&self) -> String {
        format!("http://{}", self.grpc_listen_addr)
    }
}
