//! Each Abis traffic connection has its own BTS UDP port, reported in
//! Abis-Connect (A.S0003-A §4.5.6.2).

use std::collections::HashMap;
use std::io;
use std::net::{IpAddr, SocketAddr, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::mpsc::{self as std_mpsc, Receiver, Sender};
use std::thread;
use std::time::Duration;

use log::{debug, info, warn};
use parking_lot::Mutex;

use crate::bearer::ChannelFamily;
use crate::transport;
use crate::udp_bearer::UdpBearerDatagram;

pub const DEFAULT_BSC_BEARER_PORT: u16 = transport::ABIS_BSC_BEARER_PORT;

const MAX_DATAGRAM_LEN: usize = 4096;
const BTS_SOCKET_POLL: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BearerRole {
    Bts,
    Bsc,
}

#[derive(Debug, Clone)]
pub struct BearerTransportConfig {
    pub role: BearerRole,
    /// The port is not used on the BTS.
    pub bind_addr: SocketAddr,
    /// The port is not used on the BSC.
    pub remote_addr: SocketAddr,
}

/// Per-transport counters.
#[derive(Debug, Clone, Copy, Default)]
pub struct BearerTransportStats {
    pub tx_datagrams: u64,
    pub tx_errors: u64,
    pub rx_accepted: u64,
    pub rx_decode_errors: u64,
    pub rx_route_errors: u64,
}

#[derive(Default)]
struct Counters {
    tx_datagrams: AtomicU64,
    tx_errors: AtomicU64,
    rx_accepted: AtomicU64,
    rx_decode_errors: AtomicU64,
    rx_route_errors: AtomicU64,
}

struct BtsBearerSocket {
    socket: UdpSocket,
    closed: Arc<AtomicBool>,
}

impl Drop for BtsBearerSocket {
    fn drop(&mut self) {
        self.closed.store(true, Ordering::Relaxed);
    }
}

/// FCH and SCH code channels are numbered independently.
pub type BearerKey = (ChannelFamily, u32);

#[derive(Default)]
struct BscPorts {
    by_bearer: HashMap<BearerKey, u16>,
    by_port: HashMap<u16, BearerKey>,
}

enum Side {
    Bts {
        bind_ip: IpAddr,
        sockets: Mutex<HashMap<BearerKey, BtsBearerSocket>>,
    },
    Bsc {
        socket: UdpSocket,
        ports: Arc<Mutex<BscPorts>>,
    },
}

pub struct BearerTransport {
    side: Side,
    remote_addr: SocketAddr,
    rx_sender: Sender<UdpBearerDatagram>,
    rx: Mutex<Receiver<UdpBearerDatagram>>,
    counters: Arc<Counters>,
}

impl BearerTransport {
    pub fn new(config: &BearerTransportConfig) -> io::Result<Self> {
        let (rx_sender, rx) = std_mpsc::channel();
        let counters = Arc::new(Counters::default());
        let side = match config.role {
            BearerRole::Bts => Side::Bts {
                bind_ip: config.bind_addr.ip(),
                sockets: Mutex::new(HashMap::new()),
            },
            BearerRole::Bsc => {
                let socket = UdpSocket::bind(config.bind_addr)?;
                info!(
                    "Bearer transport bound {} -> BTS {}",
                    socket.local_addr()?,
                    config.remote_addr.ip()
                );
                let ports = Arc::new(Mutex::new(BscPorts::default()));
                spawn_bsc_receiver(
                    socket.try_clone()?,
                    config.remote_addr.ip(),
                    ports.clone(),
                    rx_sender.clone(),
                    counters.clone(),
                );
                Side::Bsc { socket, ports }
            }
        };
        Ok(Self {
            side,
            remote_addr: config.remote_addr,
            rx_sender,
            rx: Mutex::new(rx),
            counters,
        })
    }

    /// The port is the connection's Traffic Circuit Identifier.
    pub fn open_bearer(&self, family: ChannelFamily, bearer_id: u32) -> io::Result<u16> {
        let key = (family, bearer_id);
        let Side::Bts { bind_ip, sockets } = &self.side else {
            return Err(io::Error::other("only the BTS opens bearer sockets"));
        };
        let mut sockets = sockets.lock();
        if let Some(open) = sockets.get(&key) {
            return Ok(open.socket.local_addr()?.port());
        }
        let socket = UdpSocket::bind(SocketAddr::new(*bind_ip, 0))?;
        socket.set_read_timeout(Some(BTS_SOCKET_POLL))?;
        let port = socket.local_addr()?.port();
        let closed = Arc::new(AtomicBool::new(false));
        spawn_bts_receiver(
            socket.try_clone()?,
            key,
            self.remote_addr.ip(),
            closed.clone(),
            self.rx_sender.clone(),
            self.counters.clone(),
        );
        info!("Bearer transport: {family:?} connection {bearer_id} on port {port}");
        sockets.insert(key, BtsBearerSocket { socket, closed });
        Ok(port)
    }

    pub fn close_bearer(&self, family: ChannelFamily, bearer_id: u32) {
        if let Side::Bts { sockets, .. } = &self.side
            && sockets.lock().remove(&(family, bearer_id)).is_some()
        {
            info!("Bearer transport: {family:?} connection {bearer_id} closed");
        }
    }

    pub fn set_bearer_port(&self, family: ChannelFamily, bearer_id: u32, port: u16) {
        let Side::Bsc { ports, .. } = &self.side else {
            return;
        };
        let key = (family, bearer_id);
        let mut ports = ports.lock();
        if let Some(old) = ports.by_bearer.insert(key, port)
            && old != port
        {
            ports.by_port.remove(&old);
        }
        if let Some(previous) = ports.by_port.insert(port, key)
            && previous != key
        {
            ports.by_bearer.remove(&previous);
        }
        info!("Bearer transport: {family:?} connection {bearer_id} at BTS port {port}");
    }

    pub fn send(&self, datagram: &UdpBearerDatagram) -> Result<(), String> {
        let result = match &self.side {
            Side::Bts { sockets, .. } => {
                let sockets = sockets.lock();
                let Some(open) = sockets.get(&(datagram.channel_family, datagram.bearer_id)) else {
                    self.counters.tx_errors.fetch_add(1, Ordering::Relaxed);
                    return Err(format!("no open connection {}", datagram.bearer_id));
                };
                open.socket.send_to(&datagram.payload, self.remote_addr)
            }
            Side::Bsc { socket, ports } => {
                let key = (datagram.channel_family, datagram.bearer_id);
                let Some(port) = ports.lock().by_bearer.get(&key).copied() else {
                    self.counters.tx_errors.fetch_add(1, Ordering::Relaxed);
                    return Err(format!("no BTS port for connection {}", datagram.bearer_id));
                };
                socket.send_to(
                    &datagram.payload,
                    SocketAddr::new(self.remote_addr.ip(), port),
                )
            }
        };
        result.map_err(|e| {
            self.counters.tx_errors.fetch_add(1, Ordering::Relaxed);
            format!("bearer send: {e}")
        })?;
        self.counters.tx_datagrams.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    pub fn drain(&self) -> Vec<UdpBearerDatagram> {
        let rx = self.rx.lock();
        let mut out = Vec::new();
        while let Ok(d) = rx.try_recv() {
            out.push(d);
        }
        out
    }

    pub fn try_recv(&self) -> Option<UdpBearerDatagram> {
        self.rx.lock().try_recv().ok()
    }

    pub fn local_addr(&self, key: BearerKey) -> Option<SocketAddr> {
        match &self.side {
            Side::Bsc { socket, .. } => socket.local_addr().ok(),
            Side::Bts { sockets, .. } => sockets
                .lock()
                .get(&key)
                .and_then(|open| open.socket.local_addr().ok()),
        }
    }

    pub fn remote_addr(&self) -> SocketAddr {
        self.remote_addr
    }

    pub fn stats(&self) -> BearerTransportStats {
        let c = &self.counters;
        BearerTransportStats {
            tx_datagrams: c.tx_datagrams.load(Ordering::Relaxed),
            tx_errors: c.tx_errors.load(Ordering::Relaxed),
            rx_accepted: c.rx_accepted.load(Ordering::Relaxed),
            rx_decode_errors: c.rx_decode_errors.load(Ordering::Relaxed),
            rx_route_errors: c.rx_route_errors.load(Ordering::Relaxed),
        }
    }
}

fn deliver(
    (family, bearer_id): BearerKey,
    bytes: &[u8],
    sender: &Sender<UdpBearerDatagram>,
    counters: &Counters,
) -> bool {
    match UdpBearerDatagram::from_payload(bearer_id, bytes.to_vec()) {
        Ok(datagram) if datagram.channel_family == family => {
            counters.rx_accepted.fetch_add(1, Ordering::Relaxed);
            sender.send(datagram).is_ok()
        }
        Ok(datagram) => {
            counters.rx_route_errors.fetch_add(1, Ordering::Relaxed);
            warn!(
                "Bearer transport: {:?} frame on {family:?} connection {bearer_id}",
                datagram.channel_family
            );
            true
        }
        Err(e) => {
            counters.rx_decode_errors.fetch_add(1, Ordering::Relaxed);
            warn!("Bearer transport decode failed on {family:?} connection {bearer_id}: {e}");
            true
        }
    }
}

fn spawn_bts_receiver(
    socket: UdpSocket,
    key: BearerKey,
    bsc_ip: IpAddr,
    closed: Arc<AtomicBool>,
    sender: Sender<UdpBearerDatagram>,
    counters: Arc<Counters>,
) {
    thread::spawn(move || {
        let mut buf = [0u8; MAX_DATAGRAM_LEN];
        while !closed.load(Ordering::Relaxed) {
            let (n, peer) = match socket.recv_from(&mut buf) {
                Ok(received) => received,
                Err(e)
                    if matches!(
                        e.kind(),
                        io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                    ) =>
                {
                    continue;
                }
                Err(e) => {
                    warn!("Bearer transport recv failed on connection {key:?}: {e}");
                    break;
                }
            };
            if peer.ip() != bsc_ip {
                counters.rx_route_errors.fetch_add(1, Ordering::Relaxed);
                debug!("Bearer transport: connection {key:?} ignored a frame from {peer}");
                continue;
            }
            if !deliver(key, &buf[..n], &sender, &counters) {
                break;
            }
        }
    });
}

fn spawn_bsc_receiver(
    socket: UdpSocket,
    bts_ip: IpAddr,
    ports: Arc<Mutex<BscPorts>>,
    sender: Sender<UdpBearerDatagram>,
    counters: Arc<Counters>,
) {
    thread::spawn(move || {
        let mut buf = [0u8; MAX_DATAGRAM_LEN];
        loop {
            let (n, peer) = match socket.recv_from(&mut buf) {
                Ok(received) => received,
                Err(e) => {
                    warn!("Bearer transport recv failed: {e}");
                    break;
                }
            };
            let key = if peer.ip() == bts_ip {
                ports.lock().by_port.get(&peer.port()).copied()
            } else {
                None
            };
            let Some(key) = key else {
                counters.rx_route_errors.fetch_add(1, Ordering::Relaxed);
                debug!("Bearer transport: frame from {peer} matches no connection");
                continue;
            };
            if !deliver(key, &buf[..n], &sender, &counters) {
                break;
            }
        }
    });
}
