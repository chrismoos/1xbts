use std::collections::HashMap;
use std::io::{self, Read, Write};
use std::net::{Ipv4Addr, SocketAddr, TcpStream};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

use tokio::sync::mpsc;

use crate::ip_transport::IpTransport;

const RECONNECT_INTERVAL: Duration = Duration::from_secs(3);

pub struct FouTcpTunnel {
    writer: Arc<Mutex<Option<TcpStream>>>,
    pub remote_addr: SocketAddr,
    routes: Arc<Mutex<HashMap<Ipv4Addr, mpsc::Sender<Vec<u8>>>>>,
}

impl FouTcpTunnel {
    pub fn new(remote_addr: SocketAddr) -> Arc<Self> {
        let routes = Arc::new(Mutex::new(HashMap::new()));
        let writer: Arc<Mutex<Option<TcpStream>>> = Arc::new(Mutex::new(None));

        let tunnel = Arc::new(Self {
            writer: writer.clone(),
            remote_addr,
            routes: routes.clone(),
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
            return Err(e);
        }
        Ok(())
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
    let mut routed_packets = 0u64;
    let mut no_route_drops = 0u64;
    let mut channel_drops = 0u64;

    loop {
        let mut len_buf = [0u8; 2];
        reader.read_exact(&mut len_buf)?;
        let packet_len = u16::from_be_bytes(len_buf) as usize;
        if packet_len < 20 {
            let mut discard = vec![0u8; packet_len];
            reader.read_exact(&mut discard)?;
            continue;
        }

        let mut packet = vec![0u8; packet_len];
        reader.read_exact(&mut packet)?;

        rx_packets = rx_packets.saturating_add(1);
        rx_bytes = rx_bytes.saturating_add(packet_len as u64);

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
            log::debug!(
                "FOU TCP dispatcher health: rx_pkts={} rx_bytes={} routed={} no_route_drops={} channel_drops={}",
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
