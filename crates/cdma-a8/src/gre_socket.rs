//! A raw GRE socket on an address receives every protocol 47 packet sent to
//! it, and reports peers with port 0.

use std::io;
use std::net::{IpAddr, SocketAddr};

use crate::{
    BearerTransportConfig, BearerTransportMode, Error, Result, TokioRawGreSocket,
    TokioUdpGreEndpoint, UdpGreEndpoint,
};

/// RFC 8086 GRE-in-UDP destination port.
pub const GRE_IN_UDP_PORT: u16 = 4754;

pub enum GreSocket {
    Udp(TokioUdpGreEndpoint),
    Raw(TokioRawGreSocket),
}

impl GreSocket {
    pub fn bind(config: BearerTransportConfig, local_ip: IpAddr, label: &str) -> Result<Self> {
        match config.mode {
            BearerTransportMode::UdpEncapsulatedGre => {
                let endpoint = UdpGreEndpoint::bind_multi_peer(config, label)?
                    .into_tokio()
                    .map_err(|err| Error::UdpTransport(format!("{label}: Tokio setup: {err}")))?;
                Ok(Self::Udp(endpoint))
            }
            BearerTransportMode::RawGre => Ok(Self::Raw(TokioRawGreSocket::bind(local_ip, label)?)),
        }
    }

    pub fn mode(&self) -> BearerTransportMode {
        match self {
            Self::Udp(_) => BearerTransportMode::UdpEncapsulatedGre,
            Self::Raw(_) => BearerTransportMode::RawGre,
        }
    }

    pub async fn readable(&self) -> io::Result<()> {
        match self {
            Self::Udp(endpoint) => endpoint.readable().await,
            Self::Raw(socket) => socket.readable().await,
        }
    }

    /// `None` for an empty queue or an invalid frame, which is logged.
    pub fn try_recv_wire(&self, buf: &mut [u8], label: &str) -> Option<(Vec<u8>, SocketAddr)> {
        let received = match self {
            Self::Udp(endpoint) => endpoint
                .socket
                .try_recv_from(buf)
                .map(|(len, from)| (buf[..len].to_vec(), from)),
            Self::Raw(socket) => socket
                .try_recv_wire_io(buf)
                .map(|(wire, from)| (wire, SocketAddr::new(from, 0))),
        };
        let (wire, from) = match received {
            Ok(received) => received,
            Err(err) if err.kind() == io::ErrorKind::WouldBlock => return None,
            Err(err) => {
                log::warn!("{label}: receive failed: {err}");
                return None;
            }
        };
        if let Err(err) = crate::GrePacket::decode(&wire) {
            log::warn!("{label}: dropping malformed GRE frame from {from}: {err}");
            return None;
        }
        Some((wire, from))
    }

    /// Raw GRE uses only `peer`'s address.
    pub async fn send_wire_to(&self, wire_bytes: &[u8], peer: SocketAddr) -> Result<usize> {
        match self {
            Self::Udp(endpoint) => endpoint.send_wire_packet_to(wire_bytes, peer).await,
            Self::Raw(socket) => socket.send_wire_packet_to(peer.ip(), wire_bytes).await,
        }
    }
}
