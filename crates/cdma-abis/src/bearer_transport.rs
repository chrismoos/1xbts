//! Bi-directional UDP bearer transport for Abis traffic frames.
//!
//! Both BTS and BSC instantiate a [`BearerTransport`] with their own
//! `(bind_addr, remote_addr)`. Each side sends [`UdpBearerDatagram`]s to
//! the remote and receives datagrams on the bind address via a background
//! thread.

use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self as std_mpsc, Receiver};
use std::thread;

use log::{info, warn};
use parking_lot::Mutex;

use crate::bearer::{ChannelFamily, Direction};
use crate::transport;
use crate::udp_bearer::{
    BearerRouteKey, UdpBearerDatagram, UdpBearerRouteOutcome, UdpBearerRouter,
};

/// Well-known default bearer port for the BSC side.
pub const DEFAULT_BSC_BEARER_PORT: u16 = transport::ABIS_BSC_BEARER_PORT;
/// Well-known default bearer port for the BTS side.
pub const DEFAULT_BTS_BEARER_PORT: u16 = transport::ABIS_BTS_BEARER_PORT;

/// Configurable bind + remote addresses for one side of the bearer link.
#[derive(Debug, Clone)]
pub struct BearerTransportConfig {
    /// Local address to bind the UDP socket.
    pub bind_addr: SocketAddr,
    /// Remote peer address to send datagrams to.
    pub remote_addr: SocketAddr,
    /// BTS identifier stamped into every outbound datagram and used as the
    /// receive routing key. Set to `base_id` from the BTS overhead config.
    pub bts_id: u32,
    /// Cell (sector) identifier stamped into every outbound datagram. Use 1
    /// for a single-sector BTS.
    pub cell_id: u32,
}

/// Per-transport counters.
#[derive(Debug, Clone, Copy, Default)]
pub struct BearerTransportStats {
    pub tx_datagrams: u64,
    pub tx_errors: u64,
    pub rx_accepted: u64,
    pub rx_duplicate_drop: u64,
    pub rx_late_drop: u64,
    pub rx_decode_errors: u64,
    pub rx_route_errors: u64,
}

/// Bi-directional UDP bearer transport.
///
/// Provides `send()` for outbound datagrams and `recv()` / `drain()` for
/// inbound datagrams. A background thread handles socket I/O; received
/// datagrams are routed, deduplicated, and queued for the caller.
pub struct BearerTransport {
    socket: UdpSocket,
    remote_addr: SocketAddr,
    bts_id: u32,
    cell_id: u32,
    rx: Mutex<Receiver<UdpBearerDatagram>>,
    tx_datagrams: AtomicU64,
    tx_errors: AtomicU64,
    rx_decode_errors: Arc<AtomicU64>,
    rx_route_errors: Arc<AtomicU64>,
    router_state: Arc<Mutex<UdpBearerRouter>>,
}

impl BearerTransport {
    /// Create a new bearer transport bound to `config.bind_addr`, sending
    /// datagrams to `config.remote_addr`.
    pub fn new(config: &BearerTransportConfig) -> std::io::Result<Self> {
        let socket = UdpSocket::bind(config.bind_addr)?;
        let local_addr = socket.local_addr()?;
        info!(
            "Bearer transport bound {} → remote {}",
            local_addr, config.remote_addr
        );

        let recv_socket = socket.try_clone()?;
        let (tx_chan, rx_chan) = std_mpsc::channel();
        let router = Arc::new(Mutex::new(all_routes_router(config.bts_id, config.cell_id)));
        let router_for_thread = router.clone();
        let rx_decode_errors = Arc::new(AtomicU64::new(0));
        let rx_route_errors = Arc::new(AtomicU64::new(0));
        let decode_err = rx_decode_errors.clone();
        let route_err = rx_route_errors.clone();

        thread::spawn(move || {
            let mut buf = [0u8; 4096];
            loop {
                let (n, _peer) = match recv_socket.recv_from(&mut buf) {
                    Ok(r) => r,
                    Err(e) => {
                        warn!("Bearer transport recv failed: {e}");
                        break;
                    }
                };
                let datagram = match UdpBearerDatagram::decode(&buf[..n]) {
                    Ok(d) => d,
                    Err(e) => {
                        decode_err.fetch_add(1, Ordering::Relaxed);
                        warn!("Bearer transport decode failed: {e}");
                        continue;
                    }
                };
                let outcome = {
                    let mut r = router_for_thread.lock();
                    match r.route(datagram) {
                        Ok(routed) => {
                            if routed.outcome == UdpBearerRouteOutcome::Accepted {
                                Some(routed.datagram)
                            } else {
                                None
                            }
                        }
                        Err(e) => {
                            route_err.fetch_add(1, Ordering::Relaxed);
                            warn!("Bearer transport route failed: {e}");
                            None
                        }
                    }
                };
                if let Some(datagram) = outcome
                    && tx_chan.send(datagram).is_err()
                {
                    break;
                }
            }
        });

        Ok(Self {
            socket,
            remote_addr: config.remote_addr,
            bts_id: config.bts_id,
            cell_id: config.cell_id,
            rx: Mutex::new(rx_chan),
            tx_datagrams: AtomicU64::new(0),
            tx_errors: AtomicU64::new(0),
            rx_decode_errors,
            rx_route_errors,
            router_state: router,
        })
    }

    /// Send a datagram to the remote peer.
    pub fn send(&self, datagram: &UdpBearerDatagram) -> Result<(), String> {
        let bytes = datagram
            .encode()
            .map_err(|e| format!("bearer encode: {e}"))?;
        self.socket.send_to(&bytes, self.remote_addr).map_err(|e| {
            self.tx_errors.fetch_add(1, Ordering::Relaxed);
            format!("bearer send: {e}")
        })?;
        self.tx_datagrams.fetch_add(1, Ordering::Relaxed);
        Ok(())
    }

    /// Drain all received datagrams currently queued.
    pub fn drain(&self) -> Vec<UdpBearerDatagram> {
        let rx = self.rx.lock();
        let mut out = Vec::new();
        while let Ok(d) = rx.try_recv() {
            out.push(d);
        }
        out
    }

    /// Try to receive a single datagram without blocking.
    pub fn try_recv(&self) -> Option<UdpBearerDatagram> {
        self.rx.lock().try_recv().ok()
    }

    /// The local address this transport is bound to.
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    /// The configured remote peer address.
    pub fn remote_addr(&self) -> SocketAddr {
        self.remote_addr
    }

    /// The BTS identifier stamped into outbound datagrams and used as the receive routing key.
    pub fn bts_id(&self) -> u32 {
        self.bts_id
    }

    /// The cell (sector) identifier stamped into outbound datagrams.
    pub fn cell_id(&self) -> u32 {
        self.cell_id
    }

    /// Current transport statistics.
    pub fn stats(&self) -> BearerTransportStats {
        let bts_id = self.bts_id;
        let cell_id = self.cell_id;
        let r = self.router_state.lock();
        let mut rx_accepted = 0u64;
        let mut rx_dup = 0u64;
        let mut rx_late = 0u64;
        for family in [ChannelFamily::Fch, ChannelFamily::Sch, ChannelFamily::Dcch] {
            for direction in [Direction::Forward, Direction::Reverse] {
                for bearer_id in 0..=u8::MAX {
                    if let Some(c) = r.counters(BearerRouteKey {
                        channel_family: family,
                        direction,
                        bts_id,
                        cell_id,
                        bearer_id: bearer_id as u32,
                    }) {
                        rx_accepted += c.accepted;
                        rx_dup += c.duplicate_drop;
                        rx_late += c.late_drop;
                    }
                }
            }
        }

        BearerTransportStats {
            tx_datagrams: self.tx_datagrams.load(Ordering::Relaxed),
            tx_errors: self.tx_errors.load(Ordering::Relaxed),
            rx_accepted,
            rx_duplicate_drop: rx_dup,
            rx_late_drop: rx_late,
            rx_decode_errors: self.rx_decode_errors.load(Ordering::Relaxed),
            rx_route_errors: self.rx_route_errors.load(Ordering::Relaxed),
        }
    }
}

fn all_routes_router(bts_id: u32, cell_id: u32) -> UdpBearerRouter {
    let mut router = UdpBearerRouter::default();
    for family in [ChannelFamily::Fch, ChannelFamily::Sch, ChannelFamily::Dcch] {
        for direction in [Direction::Forward, Direction::Reverse] {
            for bearer_id in 0..=u8::MAX {
                router.register_route(BearerRouteKey {
                    channel_family: family,
                    direction,
                    bts_id,
                    cell_id,
                    bearer_id: bearer_id as u32,
                });
            }
        }
    }
    router
}
