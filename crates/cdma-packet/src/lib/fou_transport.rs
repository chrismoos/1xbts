/// FOU (Foo-over-UDP) tunnel transport — no root required.
///
/// Sends raw IP packets as UDP payload to a remote Linux endpoint running
/// `ip fou add`. The remote decapsulates, routes via kernel, and sends
/// return traffic back through the same UDP tunnel.
use std::collections::HashMap;
use std::io;
use std::net::{Ipv4Addr, SocketAddr, SocketAddrV4, SocketAddrV6, UdpSocket};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use socket2::{Domain, Protocol, Socket, Type};
use tokio::sync::mpsc;

use crate::ip_transport::IpTransport;

/// Shared FOU tunnel — one per BSC.
///
/// Uses a plain `std::net::UdpSocket` for both send and receive.
/// A background thread runs the recv dispatcher that routes incoming
/// IP packets to the correct session based on destination IP.
pub struct FouTunnel {
    send_socket: Arc<UdpSocket>,
    pub remote_addr: SocketAddr,
    routes: Arc<Mutex<HashMap<Ipv4Addr, mpsc::Sender<Vec<u8>>>>>,
}

impl FouTunnel {
    /// Create a new FOU tunnel bound to `local_port`, sending to `remote_addr`.
    pub fn new(remote_addr: SocketAddr, local_port: u16) -> io::Result<Arc<Self>> {
        let recv_sockets = bind_recv_sockets(local_port)?;
        let send_socket = Arc::new(select_send_socket(&recv_sockets, remote_addr)?);
        let local_addrs: Vec<String> = recv_sockets
            .iter()
            .filter_map(|(_, socket)| socket.local_addr().ok())
            .map(|addr| addr.to_string())
            .collect();
        log::info!(
            "FOU tunnel: bound to [{}], remote={}",
            local_addrs.join(", "),
            remote_addr
        );

        let routes: Arc<Mutex<HashMap<Ipv4Addr, mpsc::Sender<Vec<u8>>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        let tunnel = Arc::new(Self {
            send_socket,
            remote_addr,
            routes: routes.clone(),
        });

        for (socket_label, recv_socket) in recv_sockets {
            let recv_routes = routes.clone();
            thread::Builder::new()
                .name(format!("fou-dispatcher-{}", socket_label))
                .spawn(move || {
                let mut buf = [0u8; 2048];
                let mut window_started = Instant::now();
                let mut rx_packets = 0u64;
                let mut rx_bytes = 0u64;
                let mut routed_packets = 0u64;
                let mut no_route_drops = 0u64;
                let mut channel_drops = 0u64;
                loop {
                    match recv_socket.recv_from(&mut buf) {
                        Ok((n, _from)) => {
                            if n < 20 {
                                continue;
                            }
                            rx_packets = rx_packets.saturating_add(1);
                            rx_bytes = rx_bytes.saturating_add(n as u64);
                            let dst_ip = Ipv4Addr::new(buf[16], buf[17], buf[18], buf[19]);
                            let packet = buf[..n].to_vec();

                            let routes = recv_routes.lock().unwrap();
                            if let Some(tx) = routes.get(&dst_ip) {
                                match tx.try_send(packet) {
                                    Ok(()) => {
                                        routed_packets = routed_packets.saturating_add(1);
                                    }
                                    Err(_) => {
                                        channel_drops = channel_drops.saturating_add(1);
                                    }
                                }
                            } else {
                                no_route_drops = no_route_drops.saturating_add(1);
                                log::debug!(
                                    "FOU dispatcher: no route for dst={}, dropping",
                                    dst_ip
                                );
                            }
                            if window_started.elapsed() >= Duration::from_secs(5) {
                                log::info!(
                                    "FOU dispatcher health [{}]: rx_pkts={} rx_bytes={} routed={} no_route_drops={} channel_drops={}",
                                    socket_label,
                                    rx_packets,
                                    rx_bytes,
                                    routed_packets,
                                    no_route_drops,
                                    channel_drops
                                );
                                rx_packets = 0;
                                rx_bytes = 0;
                                routed_packets = 0;
                                no_route_drops = 0;
                                channel_drops = 0;
                                window_started = Instant::now();
                            }
                        }
                        Err(e) => {
                            log::warn!("FOU dispatcher [{}]: recv error: {}", socket_label, e);
                            std::thread::sleep(std::time::Duration::from_millis(100));
                        }
                    }
                }
            })?;
        }

        Ok(tunnel)
    }

    /// Register a session's peer IP for routing.
    pub fn register(&self, peer_ip: Ipv4Addr, tx: mpsc::Sender<Vec<u8>>) {
        let mut routes = self.routes.lock().unwrap();
        routes.insert(peer_ip, tx);
        log::info!("FOU tunnel: registered route for {}", peer_ip);
    }

    /// Unregister a session's peer IP.
    pub fn unregister(&self, peer_ip: Ipv4Addr) {
        let mut routes = self.routes.lock().unwrap();
        routes.remove(&peer_ip);
        log::info!("FOU tunnel: unregistered route for {}", peer_ip);
    }

    /// Send a raw IP packet to the remote FOU endpoint.
    pub fn send(&self, ip_packet: &[u8]) -> io::Result<()> {
        self.send_socket
            .send_to(ip_packet, self.remote_addr)
            .map(|_| ())
    }
}

fn bind_recv_sockets(local_port: u16) -> io::Result<Vec<(String, UdpSocket)>> {
    let mut sockets = Vec::new();
    sockets.push(("ipv4".to_string(), bind_udp_socket_v4(local_port)?));
    match bind_udp_socket_v6(local_port) {
        Ok(socket) => sockets.push(("ipv6".to_string(), socket)),
        Err(err) => {
            log::warn!(
                "FOU tunnel: failed to bind IPv6 listener on [::]:{}: {}",
                local_port,
                err
            );
        }
    }
    Ok(sockets)
}

fn select_send_socket(
    recv_sockets: &[(String, UdpSocket)],
    remote_addr: SocketAddr,
) -> io::Result<UdpSocket> {
    let family = match remote_addr {
        SocketAddr::V4(_) => "ipv4",
        SocketAddr::V6(_) => "ipv6",
    };
    recv_sockets
        .iter()
        .find(|(label, _)| label == family)
        .and_then(|(_, socket)| socket.try_clone().ok())
        .ok_or_else(|| {
            io::Error::new(
                io::ErrorKind::AddrNotAvailable,
                format!("missing {} FOU socket for remote {}", family, remote_addr),
            )
        })
}

fn bind_udp_socket_v4(local_port: u16) -> io::Result<UdpSocket> {
    let socket = Socket::new(Domain::IPV4, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_reuse_address(true)?;
    let addr = SocketAddr::V4(SocketAddrV4::new(Ipv4Addr::UNSPECIFIED, local_port));
    socket.bind(&addr.into())?;
    Ok(socket.into())
}

fn bind_udp_socket_v6(local_port: u16) -> io::Result<UdpSocket> {
    let socket = Socket::new(Domain::IPV6, Type::DGRAM, Some(Protocol::UDP))?;
    socket.set_only_v6(true)?;
    socket.set_reuse_address(true)?;
    let addr = SocketAddr::V6(SocketAddrV6::new(
        std::net::Ipv6Addr::UNSPECIFIED,
        local_port,
        0,
        0,
    ));
    socket.bind(&addr.into())?;
    Ok(socket.into())
}

/// Per-session FOU transport — implements IpTransport.
pub struct FouSessionTransport {
    tunnel: Arc<FouTunnel>,
    peer_ip: Option<Ipv4Addr>,
}

impl FouSessionTransport {
    pub fn new(tunnel: Arc<FouTunnel>) -> Self {
        Self {
            tunnel,
            peer_ip: None,
        }
    }
}

impl IpTransport for FouSessionTransport {
    fn setup(
        &mut self,
        _local_ip: Ipv4Addr,
        peer_ip: Ipv4Addr,
        to_mobile_tx: mpsc::Sender<Vec<u8>>,
    ) -> io::Result<String> {
        self.peer_ip = Some(peer_ip);
        self.tunnel.register(peer_ip, to_mobile_tx);
        Ok(format!("fou:{}", self.tunnel.remote_addr))
    }

    fn send_to_network(&self, ip_packet: &[u8]) -> io::Result<()> {
        self.tunnel.send(ip_packet)
    }

    fn teardown(&mut self) {
        if let Some(peer_ip) = self.peer_ip.take() {
            self.tunnel.unregister(peer_ip);
        }
    }
}
