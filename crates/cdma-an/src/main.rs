//! HRPD (1xEV-DO Rev 0) Access Network process entry point.
//!
//! `cdma-an` runs as a separate process from the 1x BTS / BSC and the PCF.
//! It owns HRPD session state and
//! talks to its peers only over network reference points (A8/A9/A10/A11 for
//! the bearer + accounting plane, A21 for 1x-HRPD identity and cross-paging).
//! The binary stays free of `cdma-bts` / `cdma-bsc` / `cdma-pcf` imports.

use std::net::SocketAddr;
use std::sync::Arc;

use std::collections::HashMap;

use cdma_a21::{
    A21Connection, A21Handler, A21Hub, A21Message, A21Server, PagingSource, Result as A21Result,
};
use cdma_an::air::HrpdAirController;
use cdma_an::grpc::{AnServiceImpl, SessionStore, SharedUatiAllocator};
use cdma_an::identity_broker::IdentityBroker;
use cdma_an::session::Session;
use cdma_an::{SessionStateMachine, UatiAllocator, UatiSubnet};
use clap::Parser;
use log::{info, warn};
use tokio::signal;
use tokio::sync::Mutex;
use tracing_subscriber::{EnvFilter, prelude::*, util::SubscriberInitExt};

mod config {
    //! Inline `AnConfig` for cdma-an bring-up.
    //!
    //! cdma-an stands on its own without dragging in the 1x-side config loader
    //! (`cdma-common::config_load` lives in 1x land); we keep the config struct
    //! here with built-in defaults until the HRPD-side config story is fleshed out.

    use std::net::SocketAddr;

    use cdma_a8::BearerTransportConfig;
    use serde::{Deserialize, Serialize};

    fn socket_addr(s: &str) -> SocketAddr {
        s.parse().expect("static socket address should parse")
    }

    fn default_a8_bearer() -> BearerTransportConfig {
        BearerTransportConfig::udp_encapsulated_gre(
            socket_addr("127.0.0.1:17040"),
            socket_addr("127.0.0.1:17041"),
        )
    }

    #[derive(Debug, Clone, Serialize, Deserialize)]
    #[serde(default)]
    pub struct AnConfig {
        /// Management / control-plane gRPC listener.
        pub listen_grpc: SocketAddr,
        /// A21 (1x BSC ↔ HRPD AN) TCP listener.
        pub listen_a21: SocketAddr,
        /// A8 bearer delivery toward the PCF.
        pub a8_bearer: BearerTransportConfig,
        /// AN color code (C.S0024-400 §8.2).
        pub color_code: u8,
        /// Full 128-bit HRPD UATI subnet mask advertised for assignments.
        pub subnet_mask: u8,
        /// HRPD pilot PN offset (chips/64) for this sector.
        pub pilot_offset_pn: u16,
    }

    impl Default for AnConfig {
        fn default() -> Self {
            Self::standalone_default()
        }
    }

    impl AnConfig {
        /// Built-in defaults: loopback listeners, random UATI024 allocation,
        /// color code 0. Deployed configs should override every field.
        pub fn standalone_default() -> Self {
            Self {
                listen_grpc: "127.0.0.1:17030".parse().expect("static addr"),
                listen_a21: "127.0.0.1:17031".parse().expect("static addr"),
                a8_bearer: default_a8_bearer(),
                color_code: 0,
                subnet_mask: 24,
                pilot_offset_pn: 0,
            }
        }

        pub fn validate(&self) -> Result<(), String> {
            self.a8_bearer.validate("an.a8_bearer")
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn standalone_default_matches_documented_values() {
            let c = AnConfig::standalone_default();
            assert_eq!(c.color_code, 0);
            assert_eq!(c.subnet_mask, 24);
            assert_eq!(c.pilot_offset_pn, 0);
            assert_eq!(c.listen_grpc.ip().to_string(), "127.0.0.1");
            assert_eq!(c.listen_a21.ip().to_string(), "127.0.0.1");
            assert_eq!(
                c.a8_bearer.udp_bind_addr,
                Some("127.0.0.1:17040".parse().unwrap())
            );
            c.validate().unwrap();
            // The standalone AN allocates random non-zero UATI024 values.
            let subnet = super::super::subnet_from_config(&c);
            assert_eq!(subnet.capacity(), 0x00ff_ffff);
            assert_eq!(subnet.color_code, 0);
        }
    }
}

use config::AnConfig;

const DEFAULT_LOG_FILTER: &str = "info";
const DEFAULT_LOG_CLAMPS: &[&str] = &[
    "sqlx=warn",
    "h2=warn",
    "hyper=warn",
    "hyper_util=warn",
    "tower=warn",
    "tonic=warn",
];
const DEFAULT_GLOBAL_DEBUG_PROFILE: &[&str] = &[
    DEFAULT_LOG_FILTER,
    "cdma_packet=debug",
    "cdma_an=debug",
    "cdma_abis::transport=debug",
];

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "HRPD (1xEV-DO Rev 0) Access Network process."
)]
struct Cli {
    /// Override the management/gRPC listen address.
    #[arg(long, value_name = "ADDR")]
    grpc_addr: Option<SocketAddr>,

    /// Override the A21 (1x BSC ↔ HRPD AN) listen address.
    #[arg(long, value_name = "ADDR")]
    a21_addr: Option<SocketAddr>,
}

fn effective_log_filter() -> String {
    let filter = std::env::var("RUST_LOG")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_LOG_FILTER.to_string());
    apply_default_log_clamps(&filter)
}

fn apply_default_log_clamps(filter: &str) -> String {
    let requested_directives = filter
        .split(',')
        .map(str::trim)
        .filter(|directive| !directive.is_empty())
        .map(str::to_string)
        .collect::<Vec<_>>();
    let global_debug_requested = requested_directives
        .iter()
        .any(|directive| matches!(directive.as_str(), "debug" | "trace"));
    let mut directives = Vec::new();

    if global_debug_requested {
        directives.extend(
            DEFAULT_GLOBAL_DEBUG_PROFILE
                .iter()
                .map(|entry| entry.to_string()),
        );
    }

    directives.extend(
        requested_directives
            .into_iter()
            .filter(|directive| !matches!(directive.as_str(), "debug" | "trace")),
    );

    for clamp in DEFAULT_LOG_CLAMPS {
        let target = clamp
            .split_once('=')
            .map(|(target, _)| target)
            .unwrap_or(clamp);
        let target_already_configured = directives.iter().any(|directive| {
            directive
                .split_once('=')
                .map(|(configured_target, _)| configured_target.trim() == target)
                .unwrap_or(false)
        });
        if !target_already_configured {
            directives.push((*clamp).to_string());
        }
    }

    directives.join(",")
}

fn init_logging() {
    let filter = effective_log_filter();
    let fmt_filter = EnvFilter::builder().parse_lossy(&filter);
    let _ = tracing_subscriber::registry()
        .with(tracing_subscriber::fmt::layer().with_filter(fmt_filter))
        .try_init();
    let _ = env_logger::Builder::new()
        .parse_filters(&filter)
        .format_timestamp_millis()
        .try_init();
}

fn subnet_from_config(cfg: &AnConfig) -> UatiSubnet {
    UatiSubnet {
        color_code: cfg.color_code,
        uati104: [0; 13],
        subnet_mask: cfg.subnet_mask,
    }
}

/// Shared mutable state held by the cdma-an process.
///
/// Cross-paging and full identity coordination are not handled here yet.
struct AnState {
    #[allow(dead_code)]
    session: Mutex<SessionStateMachine>,
    #[allow(dead_code)]
    uati: SharedUatiAllocator,
    #[allow(dead_code)]
    sessions: SessionStore,
    /// The AN's authoritative IMSI ↔ UATI ↔ color_code map, used to resolve
    /// cross-pages locally. The 1x BSC learns only IMSI presence over A21 and
    /// suppresses its own paging accordingly.
    #[allow(dead_code)]
    identities: Mutex<IdentityBroker>,
}

/// Minimal A21 handler that logs every inbound message and relays presence
/// announcements to the other connected peers.
struct LoggingA21Handler {
    state: Arc<AnState>,
    hub: A21Hub,
}

impl A21Handler for LoggingA21Handler {
    async fn on_identity_binding(
        &self,
        peer: SocketAddr,
        _conn: A21Connection,
        imsi: u64,
    ) -> A21Result<()> {
        info!("A21 IdentityBinding from {peer}: imsi={imsi:#x}");
        // Re-broadcast to every other connected A21 peer so all 1x BSC
        // clients see the presence announcement without polling.
        let hub = self.hub.clone();
        let n = hub.broadcast(A21Message::IdentityBinding { imsi }).await;
        if n > 0 {
            info!("A21 IdentityBinding broadcast: imsi={imsi:#x} -> {n} peers");
        }
        Ok(())
    }

    async fn on_identity_release(
        &self,
        peer: SocketAddr,
        _conn: A21Connection,
        imsi: u64,
    ) -> A21Result<()> {
        info!("A21 IdentityRelease from {peer}: imsi={imsi:#x}");
        self.state.identities.lock().await.release_by_imsi(imsi);
        let hub = self.hub.clone();
        let _ = hub.broadcast(A21Message::IdentityRelease { imsi }).await;
        Ok(())
    }

    async fn on_cross_page_request(
        &self,
        peer: SocketAddr,
        _conn: A21Connection,
        imsi: u64,
        source: PagingSource,
        payload: Vec<u8>,
    ) -> A21Result<()> {
        info!(
            "A21 CrossPageRequest from {peer}: imsi={imsi:#x} source={source:?} payload_len={}",
            payload.len()
        );
        Ok(())
    }

    async fn on_cross_page_ack(
        &self,
        peer: SocketAddr,
        _conn: A21Connection,
        imsi: u64,
        accepted: bool,
        reason: Option<String>,
    ) -> A21Result<()> {
        info!("A21 CrossPageAck from {peer}: imsi={imsi:#x} accepted={accepted} reason={reason:?}");
        Ok(())
    }

    async fn on_suppression_start(
        &self,
        peer: SocketAddr,
        _conn: A21Connection,
        imsi: u64,
        source: PagingSource,
    ) -> A21Result<()> {
        info!("A21 SuppressionStart from {peer}: imsi={imsi:#x} source={source:?}");
        Ok(())
    }

    async fn on_suppression_end(
        &self,
        peer: SocketAddr,
        _conn: A21Connection,
        imsi: u64,
    ) -> A21Result<()> {
        info!("A21 SuppressionEnd from {peer}: imsi={imsi:#x}");
        Ok(())
    }
}

async fn wait_for_shutdown() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{SignalKind, signal};
        let mut term = match signal(SignalKind::terminate()) {
            Ok(s) => s,
            Err(e) => {
                warn!("failed to install SIGTERM handler: {e}; falling back to ctrl-c only");
                let _ = signal::ctrl_c().await;
                return;
            }
        };
        tokio::select! {
            _ = signal::ctrl_c() => info!("received ctrl-c"),
            _ = term.recv() => info!("received SIGTERM"),
        }
    }
    #[cfg(not(unix))]
    {
        let _ = signal::ctrl_c().await;
        info!("received ctrl-c");
    }
}

#[tokio::main(flavor = "multi_thread")]
async fn main() -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    init_logging();

    let cli = Cli::parse();
    let mut cfg = AnConfig::standalone_default();
    if let Some(a) = cli.grpc_addr {
        cfg.listen_grpc = a;
    }
    if let Some(a) = cli.a21_addr {
        cfg.listen_a21 = a;
    }
    cfg.validate()
        .map_err(|e| format!("invalid cdma-an config: {e}"))?;

    let subnet = subnet_from_config(&cfg);
    let uati: SharedUatiAllocator = Arc::new(Mutex::new(UatiAllocator::new(subnet)));
    let sessions: SessionStore = Arc::new(Mutex::new(HashMap::<u32, Session>::new()));
    let state = Arc::new(AnState {
        session: Mutex::new(SessionStateMachine::new(cfg.color_code)),
        uati: Arc::clone(&uati),
        sessions: Arc::clone(&sessions),
        identities: Mutex::new(IdentityBroker::new()),
    });

    info!("================================================================");
    info!("cdma-an HRPD Access Network starting");
    info!("  management gRPC listen: {}", cfg.listen_grpc);
    info!("  A21 listen:             {}", cfg.listen_a21);
    info!("  A8 bearer transport:    {:?}", cfg.a8_bearer);
    info!(
        "  UATI allocator:         random UATI024 color_code={} subnet_mask=/{} capacity={}",
        cfg.color_code,
        cfg.subnet_mask,
        subnet.capacity(),
    );
    info!("  pilot offset (PN):      {}", cfg.pilot_offset_pn);
    info!("================================================================");

    // Bind A21 listener.
    let a21_addr = cfg.listen_a21;
    let a21_server = A21Server::bind(a21_addr)
        .await
        .map_err(|e| format!("failed to bind A21 listener on {a21_addr}: {e}"))?;
    let bound_a21 = a21_server.local_addr()?;
    info!("A21 server listening on {bound_a21}");

    let hub = A21Hub::new();
    let handler = LoggingA21Handler {
        state: Arc::clone(&state),
        hub: hub.clone(),
    };
    let a21_task = tokio::spawn({
        let hub = hub.clone();
        async move {
            if let Err(e) = a21_server.serve_with_hub(handler, hub).await {
                log::error!("A21 server exited with error: {e}");
            }
        }
    });

    let grpc_addr = cfg.listen_grpc;
    let air = Arc::new(Mutex::new(HrpdAirController::with_sector(
        cfg.color_code,
        cfg.pilot_offset_pn,
        None,
    )));
    let grpc_service = AnServiceImpl::new_with_air(Arc::clone(&sessions), Arc::clone(&uati), air);
    let grpc_task = tokio::spawn(async move {
        info!("management gRPC server listening on {grpc_addr}");
        if let Err(e) = tonic::transport::Server::builder()
            .add_service(grpc_service.into_server())
            .serve(grpc_addr)
            .await
        {
            log::error!("gRPC server exited with error: {e}");
        }
    });

    wait_for_shutdown().await;
    info!("cdma-an shutting down");
    a21_task.abort();
    grpc_task.abort();
    let _ = (state,);
    Ok(())
}
