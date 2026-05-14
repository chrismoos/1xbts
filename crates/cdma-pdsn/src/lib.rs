//! `cdma-pdsn` — PDSN node crate.
//!
//! Initial scope (WS-0 PR1): node configuration only (carries the legacy
//! `PacketConfig` fields previously living under `cdma-bsc::config::packet`).
//! A11 signaling, A10 GRE bearer, IP allocation, and TUN/host I/O land in
//! WS-3 / WS-4.

pub mod a11_agent;
pub mod config;
pub mod session;

use std::{net::SocketAddr, sync::Arc};

pub use config::{PacketTransportConfig, PdsnNodeConfig};
pub use session::{
    IpPool, PdsnError, PdsnEvent, PdsnSession, PdsnSessionManager, PdsnSessionPhase,
    PdsnTimerPolicy, Result,
};

pub fn build_packet_service(
    cfg: &PdsnNodeConfig,
) -> std::result::Result<Arc<cdma_packet::grpc::PacketServiceImpl>, String> {
    let transport_config = packet_transport_config(&cfg.packet)?;
    let allocator = Arc::new(cdma_packet::ip_allocator::SubnetIpAllocator::new(
        cfg.packet.gateway_ip,
        cfg.packet.primary_dns,
        cfg.packet.secondary_dns,
    ));
    let fou_tunnel = match &transport_config {
        cdma_packet::ip_transport::IpTransportConfig::Fou {
            remote_addr,
            local_port,
        } => {
            let tunnel = cdma_packet::fou_transport::FouTunnel::new(*remote_addr, *local_port)
                .map_err(|e| format!("failed to create FOU tunnel: {e}"))?;
            Some(tunnel)
        }
        _ => None,
    };
    let fou_tcp_tunnel = match &transport_config {
        cdma_packet::ip_transport::IpTransportConfig::FouTcp { remote_addr } => Some(
            cdma_packet::fou_tcp_transport::FouTcpTunnel::new(*remote_addr),
        ),
        _ => None,
    };
    Ok(Arc::new(
        cdma_packet::grpc::PacketServiceImpl::with_allocator(
            transport_config,
            fou_tunnel,
            fou_tcp_tunnel,
            allocator,
        ),
    ))
}

pub async fn run_packet_grpc_server(
    addr: SocketAddr,
    service: cdma_packet::grpc::PacketServiceImpl,
) -> std::result::Result<(), tonic::transport::Error> {
    tonic::transport::Server::builder()
        .add_service(cdma_packet::proto::packet_service_server::PacketServiceServer::new(service))
        .serve(addr)
        .await
}

pub fn packet_grpc_endpoint(addr: SocketAddr) -> String {
    format!("http://{addr}")
}

/// Spawns the packet gRPC server and returns the endpoint URI and a `JoinHandle`.
///
/// The caller should monitor the handle: if it completes the server has stopped,
/// which is a fatal condition for the packet-data path.
pub fn spawn_configured_packet_service(
    cfg: &PdsnNodeConfig,
) -> std::result::Result<(String, tokio::task::JoinHandle<()>), String> {
    let addr = cfg.packet_grpc_listen_addr;
    let service = build_packet_service(cfg)?;
    let handle = tokio::spawn(async move {
        if let Err(error) = run_packet_grpc_server(addr, (*service).clone()).await {
            log::error!("packet gRPC server error: {error}");
        }
    });
    Ok((packet_grpc_endpoint(addr), handle))
}

fn packet_transport_config(
    cfg: &PacketTransportConfig,
) -> std::result::Result<cdma_packet::ip_transport::IpTransportConfig, String> {
    cfg.validate()?;
    match cfg.transport.as_str() {
        "tun" => {
            let nat_interface = cfg
                .tun_nat_interface
                .as_deref()
                .map(str::trim)
                .filter(|iface| !iface.is_empty())
                .ok_or("pdsn.packet.tun_nat_interface is required when transport = \"tun\"")?;
            Ok(cdma_packet::ip_transport::IpTransportConfig::Tun {
                nat_interface: nat_interface.to_string(),
            })
        }
        "fou" => {
            let remote = cfg
                .fou_remote
                .as_ref()
                .ok_or("pdsn.packet.fou_remote is required when transport = \"fou\"")?;
            let addr = remote
                .parse()
                .map_err(|e| format!("invalid pdsn.packet.fou_remote '{}': {}", remote, e))?;
            Ok(cdma_packet::ip_transport::IpTransportConfig::Fou {
                remote_addr: addr,
                local_port: cfg.fou_local_port,
            })
        }
        "fou_tcp" => {
            let remote = cfg
                .fou_remote
                .as_ref()
                .ok_or("pdsn.packet.fou_remote is required when transport = \"fou_tcp\"")?;
            let addr = remote
                .parse()
                .map_err(|e| format!("invalid pdsn.packet.fou_remote '{}': {}", remote, e))?;
            Ok(cdma_packet::ip_transport::IpTransportConfig::FouTcp { remote_addr: addr })
        }
        other => Err(format!(
            "unknown pdsn.packet.transport '{}' (expected \"tun\", \"fou\", or \"fou_tcp\")",
            other
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packet_transport_config_maps_tun() {
        let cfg = PacketTransportConfig {
            transport: "tun".to_string(),
            tun_nat_interface: Some("eth0".to_string()),
            ..PacketTransportConfig::default()
        };
        let transport = packet_transport_config(&cfg).expect("transport config");
        assert!(matches!(
            transport,
            cdma_packet::ip_transport::IpTransportConfig::Tun { .. }
        ));
    }

    #[test]
    fn packet_transport_config_maps_fou() {
        let cfg = PacketTransportConfig {
            transport: "fou".to_string(),
            fou_remote: Some("127.0.0.1:17010".to_string()),
            ..PacketTransportConfig::default()
        };
        let transport = packet_transport_config(&cfg).expect("transport config");
        assert!(matches!(
            transport,
            cdma_packet::ip_transport::IpTransportConfig::Fou { .. }
        ));
    }

    #[test]
    fn packet_transport_config_maps_fou_tcp() {
        let cfg = PacketTransportConfig {
            transport: "fou_tcp".to_string(),
            fou_remote: Some("127.0.0.1:17012".to_string()),
            ..PacketTransportConfig::default()
        };
        let transport = packet_transport_config(&cfg).expect("transport config");
        assert!(matches!(
            transport,
            cdma_packet::ip_transport::IpTransportConfig::FouTcp { .. }
        ));
    }
}
