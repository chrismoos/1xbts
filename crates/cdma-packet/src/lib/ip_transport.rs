/// IP packet transport abstraction.
///
/// Pluggable backend for forwarding IP packets between the mobile (via RLP/PPP)
/// and the internet. Implementations include kernel TUN devices and FOU tunnels.
use std::io;
use std::net::Ipv4Addr;

use tokio::sync::mpsc;

/// Configuration for selecting an IP transport backend.
#[derive(Debug, Clone)]
pub enum IpTransportConfig {
    /// Kernel TUN device (requires root).
    Tun { nat_interface: String },
    /// FOU (Foo-over-UDP) tunnel to a remote Linux endpoint.
    Fou {
        remote_addr: std::net::SocketAddr,
        local_port: u16,
    },
    /// FOU carried over a framed TCP relay to a local helper/container.
    FouTcp { remote_addr: std::net::SocketAddr },
}

/// Trait for IP packet transport backends.
///
/// Uses channels for the receive path to avoid async trait dyn-compatibility issues.
/// The transport spawns internal tasks as needed and delivers received packets
/// through the provided sender.
pub trait IpTransport: Send {
    /// Initialize the transport after IPCP assigns IP addresses.
    /// `to_mobile_tx` is used to deliver IP packets from the network to the mobile.
    /// Returns a display name for status reporting (e.g., "utun4" or "fou:10.1.2.3:17010").
    fn setup(
        &mut self,
        local_ip: Ipv4Addr,
        peer_ip: Ipv4Addr,
        to_mobile_tx: mpsc::Sender<Vec<u8>>,
    ) -> io::Result<String>;

    /// Forward an IP packet FROM the mobile TO the internet/network.
    fn send_to_network(&self, ip_packet: &[u8]) -> io::Result<()>;

    /// Cleanup on session end.
    fn teardown(&mut self);
}
