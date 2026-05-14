use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use crate::ip_transport::IpTransport;

const RECONNECT_INTERVAL: Duration = Duration::from_secs(3);
const IPV4_MIN_HEADER_LEN: usize = 20;

pub struct FouTcpTunnel {
    writer: Arc<Mutex<Option<TcpStream>>>,
    pub remote_addr: SocketAddr,
    routes: Arc<Mutex<HashMap<Ipv4Addr, mpsc::Sender<Vec<u8>>>>>,
    tx_stats: Arc<Mutex<FouTcpStats>>,
}

impl FouTcpTunnel {
    pub fn new(remote_addr: SocketAddr) -> Arc<Self> {
        let routes = Arc::new(Mutex::new(HashMap::new()));
        let writer: Arc<Mutex<Option<TcpStream>>> = Arc::new(Mutex::new(None));
        let tx_stats = Arc::new(Mutex::new(FouTcpStats::new()));

        let tunnel = Arc::new(Self {
            writer: writer.clone(),
            remote_addr,
            routes: routes.clone(),
            tx_stats,
        });

        thread::Builder::new()
            .name("fou-tcp-connector".into())
            .spawn(move || run_connect_loop(remote_addr, writer, routes))
            .expect("failed to spawn FOU TCP connector thread");

        log::info!("FOU TCP tunnel: will connect to {} (deferred)", remote_addr);
        tunnel
    }

    pub fn register(&self, peer_ip: Ipv4Addr, tx: mpsc::Sender<Vec<u8>>) {
        let mut routes = self.routes.lock().unwrap();
        routes.insert(peer_ip, tx);
        log::info!("FOU TCP tunnel: registered route for {}", peer_ip);
    }

    pub fn unregister(&self, peer_ip: Ipv4Addr) {
        let mut routes = self.routes.lock().unwrap();
        routes.remove(&peer_ip);
        log::info!("FOU TCP tunnel: unregistered route for {}", peer_ip);
    }

    pub fn send(&self, ip_packet: &[u8]) -> io::Result<()> {
        if ip_packet.len() > u16::MAX as usize {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("FOU TCP packet too large: {} bytes", ip_packet.len()),
            ));
        }

        let mut frame = Vec::with_capacity(2 + ip_packet.len());
        frame.extend_from_slice(&(ip_packet.len() as u16).to_be_bytes());
        frame.extend_from_slice(ip_packet);

        let mut guard = self.writer.lock().unwrap();
        let stream = guard.as_mut().ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotConnected, "FOU TCP tunnel not connected")
        })?;
        if let Err(e) = stream.write_all(&frame).and_then(|_| stream.flush()) {
            *guard = None;
            self.tx_stats.lock().unwrap().record_error();
            return Err(e);
        }
        self.tx_stats.lock().unwrap().record_packet(ip_packet.len());
        Ok(())
    }
}

struct FouTcpStats {
    window_started: Instant,
    packets: u64,
    bytes: u64,
    max_packet_len: usize,
    errors: u64,
}

impl FouTcpStats {
    fn new() -> Self {
        Self {
            window_started: Instant::now(),
            packets: 0,
            bytes: 0,
            max_packet_len: 0,
            errors: 0,
        }
    }

    fn record_packet(&mut self, packet_len: usize) {
        self.packets = self.packets.saturating_add(1);
        self.bytes = self.bytes.saturating_add(packet_len as u64);
        self.max_packet_len = self.max_packet_len.max(packet_len);
        self.maybe_log("tx");
    }

    fn record_error(&mut self) {
        self.errors = self.errors.saturating_add(1);
        self.maybe_log("tx");
    }

    fn maybe_log(&mut self, direction: &str) {
        if self.window_started.elapsed() < Duration::from_secs(5) {
            return;
        }
        let avg_len = if self.packets == 0 {
            0
        } else {
            self.bytes / self.packets
        };
        log::debug!(
            "FOU TCP {} health: pkts={} bytes={} avg_len={} max_len={} errors={}",
            direction,
            self.packets,
            self.bytes,
            avg_len,
            self.max_packet_len,
            self.errors
        );
        self.window_started = Instant::now();
        self.packets = 0;
        self.bytes = 0;
        self.max_packet_len = 0;
        self.errors = 0;
    }
}

fn try_connect(
    remote_addr: SocketAddr,
    writer: &Arc<Mutex<Option<TcpStream>>>,
) -> io::Result<TcpStream> {
    let stream = TcpStream::connect_timeout(&remote_addr, Duration::from_secs(5))?;
    stream.set_nodelay(true)?;
    let reader = stream.try_clone()?;
    *writer.lock().unwrap() = Some(stream);
    log::info!("FOU TCP tunnel: connected to {}", remote_addr);
    Ok(reader)
}

fn run_connect_loop(
    remote_addr: SocketAddr,
    writer: Arc<Mutex<Option<TcpStream>>>,
    routes: Arc<Mutex<HashMap<Ipv4Addr, mpsc::Sender<Vec<u8>>>>>,
) {
    loop {
        match try_connect(remote_addr, &writer) {
            Ok(reader) => {
                if let Err(err) = run_reader(reader, &routes) {
                    log::warn!("FOU TCP dispatcher stopped: {}", err);
                }
                *writer.lock().unwrap() = None;
                log::info!(
                    "FOU TCP tunnel: disconnected, reconnecting in {:?}",
                    RECONNECT_INTERVAL
                );
            }
            Err(err) => {
                log::debug!(
                    "FOU TCP tunnel: connect to {} failed: {}, retrying in {:?}",
                    remote_addr,
                    err,
                    RECONNECT_INTERVAL
                );
            }
        }
        thread::sleep(RECONNECT_INTERVAL);
    }
}

fn run_reader(
    mut reader: TcpStream,
    routes: &Arc<Mutex<HashMap<Ipv4Addr, mpsc::Sender<Vec<u8>>>>>,
) -> io::Result<()> {
    let mut window_started = Instant::now();
    let mut rx_packets = 0u64;
    let mut rx_bytes = 0u64;
    let mut max_packet_len = 0usize;
    let mut routed_packets = 0u64;
    let mut no_route_drops = 0u64;
    let mut channel_drops = 0u64;

    loop {
        let mut len_buf = [0u8; 2];
        reader.read_exact(&mut len_buf)?;
        let packet_len = u16::from_be_bytes(len_buf) as usize;
        if packet_len < IPV4_MIN_HEADER_LEN {
            let mut discard = vec![0u8; packet_len];
            reader.read_exact(&mut discard)?;
            log::warn!(
                "FOU TCP dispatcher: dropping short framed packet len={}",
                packet_len
            );
            continue;
        }

        let mut packet = vec![0u8; packet_len];
        reader.read_exact(&mut packet)?;

        rx_packets = rx_packets.saturating_add(1);
        rx_bytes = rx_bytes.saturating_add(packet_len as u64);
        max_packet_len = max_packet_len.max(packet_len);

        let dst_ip = Ipv4Addr::new(packet[16], packet[17], packet[18], packet[19]);
        let routes = routes.lock().unwrap();
        if let Some(tx) = routes.get(&dst_ip) {
            match tx.try_send(packet) {
                Ok(()) => routed_packets = routed_packets.saturating_add(1),
                Err(_) => channel_drops = channel_drops.saturating_add(1),
            }
        } else {
            no_route_drops = no_route_drops.saturating_add(1);
            log::debug!("FOU TCP dispatcher: no route for dst={}, dropping", dst_ip);
        }

        if window_started.elapsed() >= Duration::from_secs(5) {
            let avg_len = if rx_packets == 0 {
                0
            } else {
                rx_bytes / rx_packets
            };
            log::debug!(
                "FOU TCP rx health: pkts={} bytes={} avg_len={} max_len={} routed={} no_route_drops={} channel_drops={}",
                rx_packets,
                rx_bytes,
                avg_len,
                max_packet_len,
                routed_packets,
                no_route_drops,
                channel_drops
            );
            rx_packets = 0;
            rx_bytes = 0;
            max_packet_len = 0;
            routed_packets = 0;
            no_route_drops = 0;
            channel_drops = 0;
            window_started = Instant::now();
        }
    }
}

pub struct FouTcpSessionTransport {
    tunnel: Arc<FouTcpTunnel>,
    peer_ip: Option<Ipv4Addr>,
}

impl FouTcpSessionTransport {
    pub fn new(tunnel: Arc<FouTcpTunnel>) -> Self {
        Self {
            tunnel,
            peer_ip: None,
        }
    }
}

impl IpTransport for FouTcpSessionTransport {
    fn setup(
        &mut self,
        _local_ip: Ipv4Addr,
        peer_ip: Ipv4Addr,
        to_mobile_tx: mpsc::Sender<Vec<u8>>,
    ) -> io::Result<String> {
        self.peer_ip = Some(peer_ip);
        self.tunnel.register(peer_ip, to_mobile_tx);
        Ok(format!("fou-tcp:{}", self.tunnel.remote_addr))
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
