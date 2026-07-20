//! GRE/IP bearer primitives for the A8 user plane.
//!
//! Implemented wire surface:
//! - keyed GRE session binding via the GRE `Key` field
//! - optional RFC 2890 GRE sequencing via the GRE sequence-number field
//! - endpoint and session ownership tracked by the bearer table
//!
//! The current in-repo source text for `A.S0016` / `X.S0011-*` still leaves the exact
//! non-RFC GRE attribute bit layout unresolved for short-data indication and GRE
//! segmentation signaling. Accordingly, this crate retains those negotiated capabilities in
//! [`BearerProfile`] as control-plane metadata, but it does not attempt to synthesize or
//! parse additional GRE header bits beyond keyed GRE plus optional RFC 2890 sequencing.

use std::collections::BTreeMap;
use std::fmt::{Display, Formatter};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};

use serde::{Deserialize, Serialize};
use socket2::{Domain, Protocol, SockAddr, Socket, Type};

/// Errors returned by the A8 GRE codec and bearer helpers.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Error {
    /// The input buffer ended before the full GRE header or payload could be read.
    Truncated { needed: usize, actual: usize },
    /// The GRE header carried flags or version bits outside the keyed-GRE profile used here.
    UnsupportedGreFlags(u16),
    /// The GRE packet did not carry the protocol type negotiated for the session.
    InvalidProtocolType(u16),
    /// The packet was missing the mandatory keyed-GRE session identifier.
    MissingSessionKey,
    /// The packet was missing the GRE sequence number required for the session.
    MissingSequenceNumber,
    /// The caller attempted to create a session for a session identifier that is already present.
    DuplicateSession(u32),
    /// The caller attempted to install an inbound GRE key that is already present.
    DuplicateInboundSessionKey(u32),
    /// The caller attempted to create a bearer session with mismatched endpoint address families.
    AddressFamilyMismatch { session_id: u32 },
    /// The caller referenced a control-plane session identifier that is not currently installed.
    UnknownSession(u32),
    /// A received GRE packet carried an inbound GRE key that is not currently installed.
    UnknownInboundSessionKey(u32),
    /// The caller attempted to start a new rebind while a prior transition is still active.
    TransitionInProgress { session_id: u32 },
    /// The received packet source/destination tuple did not match the installed session binding.
    EndpointMismatch { session_id: u32 },
    /// The configured outer transport cannot carry packets for this operation.
    InvalidTransportConfig(String),
    /// UDP transport failed while sending or receiving an exact GRE packet.
    UdpTransport(String),
    /// Raw IP protocol 47 (native GRE) transport failed while sending or receiving a packet.
    RawTransport(String),
}

/// Result type used by the `cdma-a8` crate.
pub type Result<T> = std::result::Result<T, Error>;

impl Display for Error {
    fn fmt(&self, f: &mut Formatter<'_>) -> std::fmt::Result {
        write!(f, "{self:?}")
    }
}

impl std::error::Error for Error {}

/// Outer delivery mode for exact keyed-GRE A8/A10 bearer packets.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BearerTransportMode {
    /// Send exact GRE packets over IP protocol 47.
    #[default]
    RawGre,
    /// Send the same exact GRE packet bytes as the payload of a UDP datagram.
    UdpEncapsulatedGre,
}

/// Configures how A8/A10 bearer packets are delivered outside the GRE codec.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct BearerTransportConfig {
    /// Outer delivery mode. The GRE packet format is identical in every mode.
    pub mode: BearerTransportMode,
    /// Local UDP socket for `udp_encapsulated_gre`.
    pub udp_bind_addr: Option<SocketAddr>,
    /// Peer UDP socket for `udp_encapsulated_gre`.
    pub udp_peer_addr: Option<SocketAddr>,
}

impl BearerTransportConfig {
    /// Production transport: exact GRE packets over IP protocol 47.
    pub const fn raw_gre() -> Self {
        Self {
            mode: BearerTransportMode::RawGre,
            udp_bind_addr: None,
            udp_peer_addr: None,
        }
    }

    /// UDP-encapsulated transport: exact GRE packets carried as UDP payloads.
    pub const fn udp_encapsulated_gre(bind: SocketAddr, peer: SocketAddr) -> Self {
        Self {
            mode: BearerTransportMode::UdpEncapsulatedGre,
            udp_bind_addr: Some(bind),
            udp_peer_addr: Some(peer),
        }
    }

    /// Validates the outer transport without inspecting any session keys.
    pub fn validate(&self, label: &str) -> std::result::Result<(), String> {
        match self.mode {
            BearerTransportMode::RawGre => {
                if self.udp_bind_addr.is_some() || self.udp_peer_addr.is_some() {
                    return Err(format!(
                        "{label}: raw_gre must not set udp_bind_addr or udp_peer_addr"
                    ));
                }
                Ok(())
            }
            BearerTransportMode::UdpEncapsulatedGre => {
                if self.udp_bind_addr.is_none() {
                    return Err(format!(
                        "{label}: udp_encapsulated_gre requires udp_bind_addr"
                    ));
                }
                if self.udp_peer_addr.is_none() {
                    return Err(format!(
                        "{label}: udp_encapsulated_gre requires udp_peer_addr"
                    ));
                }
                Ok(())
            }
        }
    }
}

/// UDP endpoint carrying exact GRE packet bytes.
pub struct UdpGreEndpoint {
    socket: UdpSocket,
    peer_addr: SocketAddr,
}

/// Tokio UDP endpoint carrying exact GRE packet bytes.
pub struct TokioUdpGreEndpoint {
    socket: tokio::net::UdpSocket,
    peer_addr: SocketAddr,
}

impl UdpGreEndpoint {
    /// Builds an endpoint from an already-bound UDP socket.
    pub fn from_socket(socket: UdpSocket, peer_addr: SocketAddr) -> Self {
        Self { socket, peer_addr }
    }

    /// Binds a UDP endpoint from a `udp_encapsulated_gre` transport config.
    pub fn bind(config: BearerTransportConfig, label: &str) -> Result<Self> {
        config
            .validate(label)
            .map_err(Error::InvalidTransportConfig)?;
        if config.mode != BearerTransportMode::UdpEncapsulatedGre {
            return Err(Error::InvalidTransportConfig(format!(
                "{label}: UdpGreEndpoint requires udp_encapsulated_gre"
            )));
        }
        let bind_addr = config
            .udp_bind_addr
            .ok_or_else(|| Error::InvalidTransportConfig(format!("{label}: missing bind addr")))?;
        let peer_addr = config
            .udp_peer_addr
            .ok_or_else(|| Error::InvalidTransportConfig(format!("{label}: missing peer addr")))?;
        let socket = UdpSocket::bind(bind_addr)
            .map_err(|e| Error::UdpTransport(format!("{label}: bind {bind_addr}: {e}")))?;
        Ok(Self { socket, peer_addr })
    }

    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    pub fn set_read_timeout(&self, timeout: Option<std::time::Duration>) -> std::io::Result<()> {
        self.socket.set_read_timeout(timeout)
    }

    pub fn set_nonblocking(&self, nonblocking: bool) -> std::io::Result<()> {
        self.socket.set_nonblocking(nonblocking)
    }

    /// Converts this endpoint to a readiness-driven Tokio UDP endpoint.
    pub fn into_tokio(self) -> std::io::Result<TokioUdpGreEndpoint> {
        self.socket.set_nonblocking(true)?;
        Ok(TokioUdpGreEndpoint {
            socket: tokio::net::UdpSocket::from_std(self.socket)?,
            peer_addr: self.peer_addr,
        })
    }

    /// Sends one already-encoded GRE packet as a UDP payload.
    pub fn send_wire_packet(&self, wire_bytes: &[u8]) -> Result<usize> {
        self.socket
            .send_to(wire_bytes, self.peer_addr)
            .map_err(|e| Error::UdpTransport(format!("send exact GRE UDP payload: {e}")))
    }

    /// Encodes and sends one GRE packet as a UDP payload.
    pub fn send_gre_packet(&self, packet: &GrePacket) -> Result<usize> {
        let wire_bytes = packet.encode()?;
        self.send_wire_packet(&wire_bytes)
    }

    /// Receives one UDP payload and parses it as an exact GRE packet.
    pub fn recv_gre_packet(&self, buf: &mut [u8]) -> Result<(GrePacket, SocketAddr)> {
        let (len, from) = self
            .socket
            .recv_from(buf)
            .map_err(|e| Error::UdpTransport(format!("receive exact GRE UDP payload: {e}")))?;
        let packet = GrePacket::decode(&buf[..len])?;
        Ok((packet, from))
    }
}

impl TokioUdpGreEndpoint {
    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        self.socket.local_addr()
    }

    /// Waits until the socket may have at least one inbound datagram.
    pub async fn readable(&self) -> std::io::Result<()> {
        self.socket.readable().await
    }

    /// Sends one already-encoded GRE packet as a UDP payload.
    pub async fn send_wire_packet(&self, wire_bytes: &[u8]) -> Result<usize> {
        self.socket
            .send_to(wire_bytes, self.peer_addr)
            .await
            .map_err(|e| Error::UdpTransport(format!("send exact GRE UDP payload: {e}")))
    }

    /// Encodes and sends one GRE packet as a UDP payload.
    pub async fn send_gre_packet(&self, packet: &GrePacket) -> Result<usize> {
        let wire_bytes = packet.encode()?;
        self.send_wire_packet(&wire_bytes).await
    }

    /// Tries to receive one UDP payload without blocking.
    pub fn try_recv_gre_packet(&self, buf: &mut [u8]) -> Result<(GrePacket, SocketAddr)> {
        let (len, from) = self
            .socket
            .try_recv_from(buf)
            .map_err(|e| Error::UdpTransport(format!("receive exact GRE UDP payload: {e}")))?;
        let packet = GrePacket::decode(&buf[..len])?;
        Ok((packet, from))
    }

    /// Receives one UDP payload and parses it as an exact GRE packet.
    pub async fn recv_gre_packet(&self, buf: &mut [u8]) -> Result<(GrePacket, SocketAddr)> {
        let (len, from) = self
            .socket
            .recv_from(buf)
            .await
            .map_err(|e| Error::UdpTransport(format!("receive exact GRE UDP payload: {e}")))?;
        let packet = GrePacket::decode(&buf[..len])?;
        Ok((packet, from))
    }
}

/// IANA IP protocol number for GRE.
const IP_PROTOCOL_GRE: i32 = 47;

/// Native GRE endpoint over a raw IP protocol 47 socket.
///
/// Unlike [`UdpGreEndpoint`], the GRE packet rides directly as the payload of an IP
/// packet with protocol 47 — no UDP wrapper. The kernel prepends the outer IP header
/// on send, so binding to the local address sets the source and lets the receive path
/// filter inbound frames by source address. This needs `CAP_NET_RAW` (Linux) or root.
#[derive(Debug)]
pub struct RawGreEndpoint {
    socket: Socket,
    remote_ip: IpAddr,
}

impl RawGreEndpoint {
    /// Binds a native GRE endpoint from a `raw_gre` transport config and bearer endpoint.
    ///
    /// The config carries no addresses for raw GRE, so the local and remote IPs come from
    /// `endpoint`. Returns a capability-naming error when the process lacks `CAP_NET_RAW`.
    pub fn bind(
        config: &BearerTransportConfig,
        endpoint: BearerEndpoint,
        label: &str,
    ) -> Result<Self> {
        config
            .validate(label)
            .map_err(Error::InvalidTransportConfig)?;
        if config.mode != BearerTransportMode::RawGre {
            return Err(Error::InvalidTransportConfig(format!(
                "{label}: RawGreEndpoint requires raw_gre"
            )));
        }
        let domain = match endpoint.local_ip {
            IpAddr::V4(_) => Domain::IPV4,
            IpAddr::V6(_) => Domain::IPV6,
        };
        let socket = Socket::new(domain, Type::RAW, Some(Protocol::from(IP_PROTOCOL_GRE)))
            .map_err(|e| Self::map_socket_error(label, "create raw GRE socket", &e))?;
        let bind_addr = SocketAddr::new(endpoint.local_ip, 0);
        socket
            .bind(&bind_addr.into())
            .map_err(|e| Self::map_socket_error(label, "bind raw GRE socket", &e))?;
        Ok(Self {
            socket,
            remote_ip: endpoint.remote_ip,
        })
    }

    /// Names the missing capability when a raw-socket operation fails on permissions.
    fn map_socket_error(label: &str, op: &str, err: &std::io::Error) -> Error {
        if matches!(
            err.raw_os_error(),
            Some(code) if code == libc_eperm() || code == libc_eacces()
        ) {
            return Error::RawTransport(format!(
                "{label}: {op}: permission denied (raw IP protocol 47 needs CAP_NET_RAW or root): {err}"
            ));
        }
        Error::RawTransport(format!("{label}: {op}: {err}"))
    }

    pub fn local_addr(&self) -> std::io::Result<SocketAddr> {
        let remote_ip = self.remote_ip;
        self.socket.local_addr().map(|addr| {
            addr.as_socket()
                .unwrap_or_else(|| SocketAddr::new(remote_ip, 0))
        })
    }

    pub fn set_read_timeout(&self, timeout: Option<std::time::Duration>) -> std::io::Result<()> {
        self.socket.set_read_timeout(timeout)
    }

    pub fn set_nonblocking(&self, nonblocking: bool) -> std::io::Result<()> {
        self.socket.set_nonblocking(nonblocking)
    }

    /// Sends one already-encoded GRE packet over the raw IP protocol 47 socket.
    ///
    /// The kernel prepends the outer IP header (proto 47, dst = remote_ip). The port in
    /// the destination address is ignored for a raw socket.
    pub fn send_wire_packet(&self, wire_bytes: &[u8]) -> Result<usize> {
        let dest = SockAddr::from(SocketAddr::new(self.remote_ip, 0));
        self.socket
            .send_to(wire_bytes, &dest)
            .map_err(|e| Error::RawTransport(format!("send native GRE packet: {e}")))
    }

    /// Encodes and sends one GRE packet over the raw IP protocol 47 socket.
    pub fn send_gre_packet(&self, packet: &GrePacket) -> Result<usize> {
        let wire_bytes = packet.encode()?;
        self.send_wire_packet(&wire_bytes)
    }

    /// Receives one native GRE packet, strips the outer IP header, and parses the GRE bytes.
    ///
    /// Frames whose source address differs from the bound peer are dropped (reported as
    /// `EndpointMismatch`) so a single raw socket only delivers the configured bearer's traffic.
    pub fn recv_gre_packet(&self, buf: &mut [u8]) -> Result<(GrePacket, IpAddr)> {
        // socket2 receives into `MaybeUninit<u8>`. A `&mut [u8]` is already initialized
        // and shares `u8`'s layout, so reborrowing it as uninit memory is sound, and we
        // only read back the bytes the kernel reports as written.
        let uninit = unsafe {
            std::slice::from_raw_parts_mut(
                buf.as_mut_ptr() as *mut std::mem::MaybeUninit<u8>,
                buf.len(),
            )
        };
        let (len, from) = self
            .socket
            .recv_from(uninit)
            .map_err(|e| Error::RawTransport(format!("receive native GRE packet: {e}")))?;
        let source_ip = from
            .as_socket()
            .map(|addr| addr.ip())
            .unwrap_or(self.remote_ip);
        if source_ip != self.remote_ip {
            return Err(Error::EndpointMismatch { session_id: 0 });
        }
        let gre_bytes = strip_inbound_ip_header(self.remote_ip, &buf[..len])?;
        let packet = GrePacket::decode(gre_bytes)?;
        Ok((packet, source_ip))
    }
}

/// Strips the outer IP header from a raw-socket receive buffer, returning the GRE bytes.
///
/// On an IPv4 raw socket Linux delivers the full inbound IP packet, so the leading IPv4
/// header (length = IHL * 4) precedes the GRE bytes. On an IPv6 raw socket there is no
/// leading header to strip — the kernel delivers the payload directly.
fn strip_inbound_ip_header(family_ip: IpAddr, frame: &[u8]) -> Result<&[u8]> {
    match family_ip {
        IpAddr::V6(_) => Ok(frame),
        IpAddr::V4(_) => {
            const IPV4_IHL_MASK: u8 = 0x0f;
            const IPV4_IHL_WORD_BYTES: usize = 4;
            if frame.is_empty() {
                return Err(Error::Truncated {
                    needed: 1,
                    actual: 0,
                });
            }
            let header_len = ((frame[0] & IPV4_IHL_MASK) as usize) * IPV4_IHL_WORD_BYTES;
            if frame.len() < header_len {
                return Err(Error::Truncated {
                    needed: header_len,
                    actual: frame.len(),
                });
            }
            Ok(&frame[header_len..])
        }
    }
}

/// `EPERM` raw OS error code (no `CAP_NET_RAW`).
const fn libc_eperm() -> i32 {
    1
}

/// `EACCES` raw OS error code (no `CAP_NET_RAW`).
const fn libc_eacces() -> i32 {
    13
}

/// Outer transport endpoint for A8/A10 bearer GRE packets, selected by config mode.
pub enum GreBearerEndpoint {
    /// UDP-encapsulated GRE (FOU-style, no root required).
    Udp(UdpGreEndpoint),
    /// Native GRE over raw IP protocol 47 (requires `CAP_NET_RAW`).
    Raw(RawGreEndpoint),
}

impl GreBearerEndpoint {
    /// Binds the bearer transport selected by `config.mode`.
    ///
    /// `endpoint` supplies the local/remote IPs the raw socket needs; the UDP path uses the
    /// `udp_bind_addr`/`udp_peer_addr` from the config and ignores `endpoint`.
    pub fn bind(
        config: &BearerTransportConfig,
        endpoint: BearerEndpoint,
        label: &str,
    ) -> Result<Self> {
        match config.mode {
            BearerTransportMode::UdpEncapsulatedGre => {
                UdpGreEndpoint::bind(*config, label).map(GreBearerEndpoint::Udp)
            }
            BearerTransportMode::RawGre => {
                RawGreEndpoint::bind(config, endpoint, label).map(GreBearerEndpoint::Raw)
            }
        }
    }

    pub fn set_read_timeout(&self, timeout: Option<std::time::Duration>) -> std::io::Result<()> {
        match self {
            GreBearerEndpoint::Udp(udp) => udp.set_read_timeout(timeout),
            GreBearerEndpoint::Raw(raw) => raw.set_read_timeout(timeout),
        }
    }

    pub fn set_nonblocking(&self, nonblocking: bool) -> std::io::Result<()> {
        match self {
            GreBearerEndpoint::Udp(udp) => udp.set_nonblocking(nonblocking),
            GreBearerEndpoint::Raw(raw) => raw.set_nonblocking(nonblocking),
        }
    }

    /// Sends one already-encoded GRE packet over the selected transport.
    pub fn send_wire_packet(&self, wire_bytes: &[u8]) -> Result<usize> {
        match self {
            GreBearerEndpoint::Udp(udp) => udp.send_wire_packet(wire_bytes),
            GreBearerEndpoint::Raw(raw) => raw.send_wire_packet(wire_bytes),
        }
    }

    /// Receives one GRE packet over the selected transport, normalized to the source `IpAddr`.
    pub fn recv_gre_packet(&self, buf: &mut [u8]) -> Result<(GrePacket, IpAddr)> {
        match self {
            GreBearerEndpoint::Udp(udp) => udp
                .recv_gre_packet(buf)
                .map(|(packet, from)| (packet, from.ip())),
            GreBearerEndpoint::Raw(raw) => raw.recv_gre_packet(buf),
        }
    }
}

/// Supported GRE protocol types for A8/A10 bearer traffic.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GreProtocolType {
    /// Unstructured byte stream carried over keyed GRE, negotiated via A9/A11.
    UnstructuredByteStream,
    /// PPP protocol type. This remains available for interoperability helpers and tests.
    PointToPointProtocol,
}

impl GreProtocolType {
    /// Returns the protocol type value encoded in the GRE header.
    pub fn as_u16(self) -> u16 {
        match self {
            Self::UnstructuredByteStream => 0x8881,
            Self::PointToPointProtocol => 0x880b,
        }
    }
}

/// Sequence-number handling policy for a bearer session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SequencingMode {
    /// The session does not require GRE sequence numbers.
    Unsequenced,
    /// The session requires RFC 2890 sequence numbers on every GRE packet.
    Required,
}

/// Session-local bearer framing negotiated through the control plane.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BearerProfile {
    /// GRE protocol type expected on the bearer.
    pub protocol_type: GreProtocolType,
    /// Whether the bearer requires GRE sequence numbers.
    pub sequencing: SequencingMode,
    /// Whether the sender guarantees higher-layer packet boundaries on the bearer.
    ///
    /// This is retained as negotiated session metadata. The current repo spec set does not yet
    /// pin down the exact non-RFC GRE attribute bit layout needed to carry this on the wire.
    pub packet_boundary_supported: bool,
    /// Whether the receiver can accept GRE segmentation indications for fragmented packets.
    ///
    /// This is retained as negotiated session metadata until the exact GRE attribute bit layout
    /// is confirmed from primary-source spec text.
    pub gre_segmentation_supported: bool,
    /// Whether the bearer supports short-data indication negotiation.
    ///
    /// This is retained as negotiated session metadata until the exact GRE attribute bit layout
    /// is confirmed from primary-source spec text.
    pub short_data_indication_supported: bool,
    /// Whether the peer has advertised GRE flow-control capability for the bearer.
    ///
    /// A10-related flow-control/duration bits remain metadata-only until the exact non-RFC GRE
    /// attribute layout is confirmed from primary-source spec text.
    pub flow_control_supported: bool,
}

impl BearerProfile {
    /// Returns the standards-faithful packet-data bearer profile used by A8/A10.
    pub fn standard_packet_data() -> Self {
        Self {
            protocol_type: GreProtocolType::UnstructuredByteStream,
            sequencing: SequencingMode::Required,
            packet_boundary_supported: false,
            gre_segmentation_supported: false,
            short_data_indication_supported: false,
            flow_control_supported: false,
        }
    }

    /// Returns a compatibility profile for PPP over GRE without sequence numbering.
    pub fn ppp_compatibility() -> Self {
        Self {
            protocol_type: GreProtocolType::PointToPointProtocol,
            sequencing: SequencingMode::Unsequenced,
            packet_boundary_supported: false,
            gre_segmentation_supported: false,
            short_data_indication_supported: false,
            flow_control_supported: false,
        }
    }

    /// Returns `true` when the negotiated profile includes capabilities whose exact GRE
    /// attribute bit layout is still unresolved in the current in-repo spec text.
    ///
    /// When this returns `true`, the current implementation keeps the capability flags in the
    /// session record but still emits and accepts only keyed GRE plus the optional RFC 2890
    /// sequence-number field.
    pub fn has_unresolved_wire_attributes(self) -> bool {
        self.packet_boundary_supported
            || self.gre_segmentation_supported
            || self.short_data_indication_supported
            || self.flow_control_supported
    }
}

impl Default for BearerProfile {
    fn default() -> Self {
        Self::standard_packet_data()
    }
}

/// Configuration for the AN-side A9 client that establishes an A8 bearer with
/// the PCF. Produced by the PCF service setup and consumed by the AN A9 client.
#[derive(Clone, Copy, Debug)]
pub struct HrpdA9ClientConfig {
    /// PCF A9 signaling address the client sends SetupA8/ReleaseA8 to.
    pub pcf_addr: SocketAddr,
    /// PCF-side A8 bearer IPv4, used to build the A8 traffic id.
    pub a8_peer_ipv4: [u8; 4],
    /// AN-side A8 bearer transport binding.
    pub an_a8_bearer: BearerTransportConfig,
    /// AN-side A8 bearer endpoint.
    pub an_a8_endpoint: BearerEndpoint,
}

/// GRE/IP endpoint binding for an A8 bearer session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BearerEndpoint {
    /// Local IP address for the bearer socket.
    pub local_ip: IpAddr,
    /// Remote IP address expected on the bearer socket.
    pub remote_ip: IpAddr,
}

impl BearerEndpoint {
    /// Creates an IPv4 bearer endpoint binding.
    pub fn new(local_ipv4: [u8; 4], remote_ipv4: [u8; 4]) -> Self {
        Self::from_ip(
            IpAddr::V4(Ipv4Addr::from(local_ipv4)),
            IpAddr::V4(Ipv4Addr::from(remote_ipv4)),
        )
    }

    /// Creates an IPv6 bearer endpoint binding.
    pub fn new_v6(local_ipv6: [u8; 16], remote_ipv6: [u8; 16]) -> Self {
        Self::from_ip(
            IpAddr::V6(Ipv6Addr::from(local_ipv6)),
            IpAddr::V6(Ipv6Addr::from(remote_ipv6)),
        )
    }

    /// Creates a bearer endpoint binding from explicit IP addresses.
    pub fn from_ip(local_ip: IpAddr, remote_ip: IpAddr) -> Self {
        Self {
            local_ip,
            remote_ip,
        }
    }

    fn has_matching_address_family(self) -> bool {
        matches!(
            (self.local_ip, self.remote_ip),
            (IpAddr::V4(_), IpAddr::V4(_)) | (IpAddr::V6(_), IpAddr::V6(_))
        )
    }
}

/// Session record bound to GRE identifiers and a transport endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BearerSession {
    /// Stable control-plane identifier used to manage the bearer session locally.
    pub session_id: u32,
    /// GRE key the peer uses when sending traffic toward this bearer.
    pub inbound_session_key: u32,
    /// GRE key used by this bearer when sending traffic toward the peer.
    pub outbound_session_key: u32,
    /// Transport endpoint bound to this session.
    pub endpoint: BearerEndpoint,
    /// Wire-profile semantics negotiated for the bearer.
    pub profile: BearerProfile,
}

impl BearerSession {
    /// Creates a bearer session description using a symmetric GRE key and the standard packet-data profile.
    pub fn new(session_key: u32, endpoint: BearerEndpoint) -> Self {
        Self {
            session_id: session_key,
            inbound_session_key: session_key,
            outbound_session_key: session_key,
            endpoint,
            profile: BearerProfile::standard_packet_data(),
        }
    }

    /// Creates a bearer session description with a symmetric GRE key and explicit wire profile.
    pub fn with_profile(
        session_key: u32,
        endpoint: BearerEndpoint,
        profile: BearerProfile,
    ) -> Self {
        Self {
            session_id: session_key,
            inbound_session_key: session_key,
            outbound_session_key: session_key,
            endpoint,
            profile,
        }
    }

    /// Creates a bearer session description with explicit control-plane identifier and directional GRE keys.
    pub fn with_directional_keys(
        session_id: u32,
        inbound_session_key: u32,
        outbound_session_key: u32,
        endpoint: BearerEndpoint,
        profile: BearerProfile,
    ) -> Self {
        Self {
            session_id,
            inbound_session_key,
            outbound_session_key,
            endpoint,
            profile,
        }
    }
}

/// Bearer packet delivered after session and endpoint validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InboundPacket {
    /// Control-plane session identifier resolved for the GRE packet.
    pub session_id: u32,
    /// GRE key extracted from the received GRE header.
    pub gre_key: u32,
    /// Endpoint the packet was accepted against.
    pub endpoint: BearerEndpoint,
    /// Monotonic receive ordinal assigned by the local bearer table.
    pub rx_ordinal: u64,
    /// GRE sequence number carried by the packet, if present.
    pub gre_sequence: Option<u32>,
    /// Decapsulated bearer payload.
    pub payload: Vec<u8>,
}

/// Outbound packet resolved through the session table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutboundPacket {
    /// Control-plane session identifier used to resolve the packet.
    pub session_id: u32,
    /// GRE key encoded into the outbound GRE header.
    pub gre_key: u32,
    /// Endpoint selected from the installed bearer session.
    pub endpoint: BearerEndpoint,
    /// Monotonic transmit ordinal assigned by the local bearer table.
    pub tx_ordinal: u64,
    /// GRE sequence number assigned to the packet, if enabled for the session.
    pub gre_sequence: Option<u32>,
    /// Number of bearer payload bytes carried in `wire_bytes`.
    pub payload_len: usize,
    /// Serialized GRE packet ready to send on the bearer socket.
    pub wire_bytes: Vec<u8>,
}

/// Accumulated bearer counters for observability and failure accounting.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BearerStats {
    /// Successfully encoded transmit packets.
    pub tx_packets: u64,
    /// Successfully encoded transmit payload bytes.
    pub tx_bytes: u64,
    /// Successfully accepted receive packets.
    pub rx_packets: u64,
    /// Successfully accepted receive payload bytes.
    pub rx_bytes: u64,
    /// GRE packets rejected before a session could be resolved.
    pub malformed_packets: u64,
    /// Packets carrying an unknown inbound GRE key.
    pub unknown_session_packets: u64,
    /// Packets dropped after a session lookup succeeded.
    pub dropped_packets: u64,
    /// Packets dropped specifically because the peer endpoint did not match.
    pub endpoint_mismatch_packets: u64,
    /// Receive packets that arrived with a duplicate GRE sequence number.
    pub duplicate_sequence_packets: u64,
    /// Receive packets that arrived behind the highest seen GRE sequence number.
    pub reordered_sequence_packets: u64,
    /// Receive packets that advanced the GRE sequence number by more than one.
    pub sequence_gap_events: u64,
    /// Successful control-plane session installs.
    pub sessions_created: u64,
    /// Successful control-plane endpoint and/or key changes on existing sessions.
    pub sessions_rebound: u64,
    /// Successful rebinds that used dormant-style immediate cutover.
    pub sessions_dormant_rebound: u64,
    /// Successful rebinds that installed mobility overlap state.
    pub sessions_mobility_rebound: u64,
    /// Successful rebinds that installed hard-handoff overlap state.
    pub sessions_hard_handoff_rebound: u64,
    /// Successful control-plane session removals.
    pub sessions_removed: u64,
    /// Successful transition completions that retired the previous endpoint.
    pub transitions_completed: u64,
    /// Number of currently installed bearer sessions.
    pub active_sessions: u64,
    /// Receive packets accepted on a draining endpoint during transition overlap.
    pub transition_rx_packets: u64,
}

/// Per-session counters retained with the session binding.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SessionStats {
    /// Successfully encoded transmit packets for the session.
    pub tx_packets: u64,
    /// Successfully encoded transmit payload bytes for the session.
    pub tx_bytes: u64,
    /// Successfully accepted receive packets for the session.
    pub rx_packets: u64,
    /// Successfully accepted receive payload bytes for the session.
    pub rx_bytes: u64,
    /// Packets dropped for this session because the endpoint binding mismatched.
    pub endpoint_mismatch_packets: u64,
    /// Total dropped packets attributed to this session.
    pub dropped_packets: u64,
    /// Receive packets accepted on the draining endpoint during a transition.
    pub transition_rx_packets: u64,
    /// Last transmit ordinal assigned by the local bearer table.
    pub last_tx_ordinal: u64,
    /// Last receive ordinal assigned by the local bearer table.
    pub last_rx_ordinal: u64,
    /// Last GRE sequence number transmitted for the session.
    pub last_tx_sequence: Option<u32>,
    /// Last GRE sequence number accepted for the session.
    pub last_rx_sequence: Option<u32>,
    /// Receive packets that arrived with a duplicate GRE sequence number.
    pub duplicate_sequence_packets: u64,
    /// Receive packets that arrived behind the highest seen GRE sequence number.
    pub reordered_sequence_packets: u64,
    /// Receive packets that advanced the GRE sequence number by more than one.
    pub sequence_gap_events: u64,
}

/// Snapshot of a session and its accumulated per-session counters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionSnapshot {
    /// Installed bearer session binding.
    pub session: BearerSession,
    /// Counters accumulated while the session was installed.
    pub stats: SessionStats,
    /// Active transition state, if the session is being rebound across endpoints.
    pub transition: Option<SessionTransition>,
}

/// Outcome of applying a control-plane session binding to the bearer table.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ApplySessionOutcome {
    /// A new session was installed.
    Created,
    /// An existing session was already installed with the same endpoint, keys, and profile.
    Unchanged,
    /// An existing session was rebound to a new endpoint, keys, and/or profile.
    Rebound {
        /// Endpoint that was replaced by the control-plane update.
        previous_endpoint: BearerEndpoint,
        /// Inbound GRE key that was replaced by the control-plane update.
        previous_inbound_session_key: u32,
        /// Outbound GRE key that was replaced by the control-plane update.
        previous_outbound_session_key: u32,
        /// Session profile that was replaced by the control-plane update.
        previous_profile: BearerProfile,
    },
}

/// Rebind behavior applied when a control-plane update moves a bearer to a new endpoint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RebindMode {
    /// Replace the endpoint immediately with no overlap period.
    DormantResume,
    /// Move outbound traffic immediately but continue accepting inbound packets from the
    /// previous endpoint until the controller explicitly finalizes the transition.
    Mobility,
    /// Move outbound traffic immediately and accept the previous endpoint only until the first
    /// valid packet arrives on the new endpoint.
    HardHandoff,
}

/// Active transition state retained while a session is moving between endpoints.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SessionTransition {
    /// Transition mode governing overlap behavior.
    pub mode: RebindMode,
    /// Endpoint that is still temporarily accepted while the transition drains.
    pub previous_endpoint: BearerEndpoint,
}

/// Outcome of an explicit rebind request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RebindOutcome {
    /// The current session binding already matched the requested endpoint.
    Unchanged,
    /// The session was rebound to the new endpoint.
    Rebound {
        /// Endpoint that was replaced by the control-plane update.
        previous_endpoint: BearerEndpoint,
        /// Transition semantics applied to the rebind.
        mode: RebindMode,
    },
}

/// GRE packet carrying A8/A10 bearer traffic.
///
/// This codec intentionally supports only keyed GRE plus the optional RFC 2890 sequence-number
/// field. Non-RFC attribute bits for short-data indication, GRE segmentation indication, and
/// A10 flow-control/duration remain out of scope until the exact bit layout is confirmed in the
/// current repo's primary-source spec set.
const GRE_KEY_FLAG: u16 = 0x2000;
const GRE_SEQUENCE_FLAG: u16 = 0x1000;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrePacket {
    /// Whether the keyed-GRE flag is set.
    pub key_present: bool,
    /// Whether the GRE sequence-number flag is set.
    pub sequence_present: bool,
    /// Protocol type carried in the GRE header.
    pub protocol_type: u16,
    /// Optional GRE key/session identifier.
    pub key: Option<u32>,
    /// Optional GRE sequence number.
    pub sequence_number: Option<u32>,
    /// Encapsulated bearer payload.
    pub payload: Vec<u8>,
}

impl GrePacket {
    /// Builds an unstructured-byte-stream GRE packet.
    pub fn octet_stream(
        key: u32,
        sequence_number: Option<u32>,
        payload: impl Into<Vec<u8>>,
    ) -> Self {
        Self {
            key_present: true,
            sequence_present: sequence_number.is_some(),
            protocol_type: GreProtocolType::UnstructuredByteStream.as_u16(),
            key: Some(key),
            sequence_number,
            payload: payload.into(),
        }
    }

    /// Builds a PPP-over-GRE packet for compatibility helpers and tests.
    pub fn ppp(key: u32, payload: impl Into<Vec<u8>>) -> Self {
        Self {
            key_present: true,
            sequence_present: false,
            protocol_type: GreProtocolType::PointToPointProtocol.as_u16(),
            key: Some(key),
            sequence_number: None,
            payload: payload.into(),
        }
    }

    /// Encodes the GRE packet.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut flags = 0u16;
        if self.key_present {
            flags |= GRE_KEY_FLAG;
        }
        if self.sequence_present {
            flags |= GRE_SEQUENCE_FLAG;
        }

        let mut out = Vec::with_capacity(12 + self.payload.len());
        out.extend_from_slice(&flags.to_be_bytes());
        out.extend_from_slice(&self.protocol_type.to_be_bytes());
        if self.key_present {
            let Some(key) = self.key else {
                return Err(Error::MissingSessionKey);
            };
            out.extend_from_slice(&key.to_be_bytes());
        }
        if self.sequence_present {
            let Some(sequence_number) = self.sequence_number else {
                return Err(Error::MissingSequenceNumber);
            };
            out.extend_from_slice(&sequence_number.to_be_bytes());
        }
        out.extend_from_slice(&self.payload);
        Ok(out)
    }

    /// Decodes the GRE packet.
    pub fn decode(input: &[u8]) -> Result<Self> {
        const GRE_SUPPORTED_FLAGS: u16 = GRE_KEY_FLAG | GRE_SEQUENCE_FLAG;

        if input.len() < 4 {
            return Err(Error::Truncated {
                needed: 4,
                actual: input.len(),
            });
        }

        let flags = u16::from_be_bytes([input[0], input[1]]);
        if flags & !GRE_SUPPORTED_FLAGS != 0 {
            return Err(Error::UnsupportedGreFlags(flags));
        }

        let key_present = flags & GRE_KEY_FLAG != 0;
        let sequence_present = flags & GRE_SEQUENCE_FLAG != 0;
        let protocol_type = u16::from_be_bytes([input[2], input[3]]);

        let mut offset = 4;
        let key = if key_present {
            if input.len() < offset + 4 {
                return Err(Error::Truncated {
                    needed: offset + 4,
                    actual: input.len(),
                });
            }
            let value = u32::from_be_bytes([
                input[offset],
                input[offset + 1],
                input[offset + 2],
                input[offset + 3],
            ]);
            offset += 4;
            Some(value)
        } else {
            None
        };

        let sequence_number = if sequence_present {
            if input.len() < offset + 4 {
                return Err(Error::Truncated {
                    needed: offset + 4,
                    actual: input.len(),
                });
            }
            let value = u32::from_be_bytes([
                input[offset],
                input[offset + 1],
                input[offset + 2],
                input[offset + 3],
            ]);
            offset += 4;
            Some(value)
        } else {
            None
        };

        Ok(Self {
            key_present,
            sequence_present,
            protocol_type,
            key,
            sequence_number,
            payload: input[offset..].to_vec(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SessionEntry {
    session: BearerSession,
    stats: SessionStats,
    transition: Option<SessionTransition>,
    next_tx_ordinal: u64,
    next_rx_ordinal: u64,
    next_tx_sequence: u32,
}

/// Session table keyed by a local control-plane session identifier.
#[derive(Debug, Default)]
pub struct BearerTable {
    sessions: BTreeMap<u32, SessionEntry>,
    inbound_key_index: BTreeMap<u32, u32>,
    stats: BearerStats,
}

impl BearerTable {
    /// Creates an empty bearer table.
    pub fn new() -> Self {
        Self::default()
    }

    /// Returns the number of currently installed sessions.
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    /// Returns `true` when the control-plane session identifier is installed.
    pub fn has_session(&self, session_id: u32) -> bool {
        self.sessions.contains_key(&session_id)
    }

    /// Registers a new bearer session and rejects duplicate session identifiers.
    pub fn create_session(&mut self, session: BearerSession) -> Result<()> {
        if self.has_session(session.session_id) {
            return Err(Error::DuplicateSession(session.session_id));
        }
        let outcome = self.apply_session(session)?;
        debug_assert_eq!(outcome, ApplySessionOutcome::Created);
        Ok(())
    }

    /// Applies a control-plane session binding to the bearer table.
    ///
    /// This is the preferred lifecycle API for control-plane driven bearer setup because it
    /// makes create, replayed-create, and immediate endpoint replacement explicit.
    ///
    /// For mobility-aware overlap semantics, use [`Self::rebind_session_with_mode`]. If a
    /// transition is already active for the session, this method rejects the update with
    /// [`Error::TransitionInProgress`] rather than silently overwriting in-flight state.
    pub fn apply_session(&mut self, session: BearerSession) -> Result<ApplySessionOutcome> {
        self.validate_session(session)?;
        self.reserve_inbound_key(session.inbound_session_key, Some(session.session_id))?;

        match self.sessions.get_mut(&session.session_id) {
            None => {
                self.sessions.insert(
                    session.session_id,
                    SessionEntry {
                        session,
                        stats: SessionStats::default(),
                        transition: None,
                        next_tx_ordinal: 1,
                        next_rx_ordinal: 1,
                        next_tx_sequence: 0,
                    },
                );
                self.inbound_key_index
                    .insert(session.inbound_session_key, session.session_id);
                self.stats.sessions_created += 1;
                self.stats.active_sessions = self.sessions.len() as u64;
                Ok(ApplySessionOutcome::Created)
            }
            Some(existing)
                if existing.session.endpoint == session.endpoint
                    && existing.session.profile == session.profile
                    && existing.session.inbound_session_key == session.inbound_session_key
                    && existing.session.outbound_session_key == session.outbound_session_key =>
            {
                Ok(ApplySessionOutcome::Unchanged)
            }
            Some(existing) if existing.transition.is_some() => Err(Error::TransitionInProgress {
                session_id: session.session_id,
            }),
            Some(existing) => {
                let previous_endpoint = existing.session.endpoint;
                let previous_inbound_session_key = existing.session.inbound_session_key;
                let previous_outbound_session_key = existing.session.outbound_session_key;
                let previous_profile = existing.session.profile;

                if previous_inbound_session_key != session.inbound_session_key {
                    self.inbound_key_index.remove(&previous_inbound_session_key);
                    self.inbound_key_index
                        .insert(session.inbound_session_key, session.session_id);
                }

                existing.session = session;
                existing.transition = None;
                self.stats.sessions_rebound += 1;
                self.stats.sessions_dormant_rebound += 1;
                Ok(ApplySessionOutcome::Rebound {
                    previous_endpoint,
                    previous_inbound_session_key,
                    previous_outbound_session_key,
                    previous_profile,
                })
            }
        }
    }

    /// Rebinds an existing session using dormant-resume semantics.
    ///
    /// This performs an immediate endpoint cutover with no overlap window for the previous
    /// endpoint. For mobility-aware overlap, use [`Self::rebind_session_with_mode`].
    pub fn rebind_session(&mut self, session_id: u32, endpoint: BearerEndpoint) -> Result<()> {
        self.rebind_session_with_mode(session_id, endpoint, RebindMode::DormantResume)
            .map(|_| ())
    }

    /// Rebinds an existing session with explicit transition semantics.
    pub fn rebind_session_with_mode(
        &mut self,
        session_id: u32,
        endpoint: BearerEndpoint,
        mode: RebindMode,
    ) -> Result<RebindOutcome> {
        let Some(existing) = self.sessions.get_mut(&session_id) else {
            return Err(Error::UnknownSession(session_id));
        };
        if !endpoint.has_matching_address_family() {
            return Err(Error::AddressFamilyMismatch { session_id });
        }
        if existing.session.endpoint == endpoint {
            return Ok(RebindOutcome::Unchanged);
        }
        if existing.transition.is_some() {
            return Err(Error::TransitionInProgress { session_id });
        }

        let previous_endpoint = existing.session.endpoint;
        existing.session.endpoint = endpoint;
        existing.transition = match mode {
            RebindMode::DormantResume => None,
            RebindMode::Mobility | RebindMode::HardHandoff => Some(SessionTransition {
                mode,
                previous_endpoint,
            }),
        };
        self.stats.sessions_rebound += 1;
        match mode {
            RebindMode::DormantResume => self.stats.sessions_dormant_rebound += 1,
            RebindMode::Mobility => self.stats.sessions_mobility_rebound += 1,
            RebindMode::HardHandoff => self.stats.sessions_hard_handoff_rebound += 1,
        }

        Ok(RebindOutcome::Rebound {
            previous_endpoint,
            mode,
        })
    }

    /// Finalizes an active transition and retires the draining endpoint.
    ///
    /// Returns `Ok(true)` when a transition was present and completed, `Ok(false)` when the
    /// session had no active transition, and `Err` when the session does not exist.
    pub fn finalize_rebind(&mut self, session_id: u32) -> Result<bool> {
        let Some(existing) = self.sessions.get_mut(&session_id) else {
            return Err(Error::UnknownSession(session_id));
        };
        let had_transition = existing.transition.take().is_some();
        if had_transition {
            self.stats.transitions_completed += 1;
        }
        Ok(had_transition)
    }

    /// Removes a session from the table.
    pub fn remove_session(&mut self, session_id: u32) -> Result<BearerSession> {
        self.remove_session_if_present(session_id)
            .ok_or(Error::UnknownSession(session_id))
    }

    /// Removes a session if it is present and returns the removed binding.
    pub fn remove_session_if_present(&mut self, session_id: u32) -> Option<BearerSession> {
        let removed = self.sessions.remove(&session_id).map(|entry| {
            self.inbound_key_index
                .retain(|_, indexed_session_id| *indexed_session_id != session_id);
            entry.session
        });
        if removed.is_some() {
            self.stats.sessions_removed += 1;
            self.stats.active_sessions = self.sessions.len() as u64;
        }
        removed
    }

    /// Accepts an additional inbound GRE key for an already-installed session.
    ///
    /// A dormant HRPD packet session can be rebound to a fresh air-interface
    /// UATI while the upstream peer continues to send A8/A10 downlink with the
    /// previous GRE key until its control-plane update catches up. Keep that
    /// old key as an inbound alias so the downlink is delivered to the current
    /// session instead of being dropped during the reopen window.
    pub fn add_inbound_key_alias(
        &mut self,
        session_id: u32,
        inbound_session_key: u32,
    ) -> Result<()> {
        if !self.sessions.contains_key(&session_id) {
            return Err(Error::UnknownSession(session_id));
        }
        if let Some(existing_session_id) = self.inbound_key_index.get(&inbound_session_key).copied()
        {
            if existing_session_id == session_id {
                return Ok(());
            }
            return Err(Error::DuplicateInboundSessionKey(inbound_session_key));
        }
        self.inbound_key_index
            .insert(inbound_session_key, session_id);
        Ok(())
    }

    /// Returns the registered session, if present.
    pub fn session(&self, session_id: u32) -> Option<&BearerSession> {
        self.sessions.get(&session_id).map(|entry| &entry.session)
    }

    /// Returns the session binding and per-session counters, if present.
    pub fn session_snapshot(&self, session_id: u32) -> Option<SessionSnapshot> {
        self.sessions
            .get(&session_id)
            .copied()
            .map(|entry| SessionSnapshot {
                session: entry.session,
                stats: entry.stats,
                transition: entry.transition,
            })
    }

    /// Returns a snapshot of accumulated bearer counters.
    pub fn stats(&self) -> BearerStats {
        self.stats
    }

    /// Resolves an outbound payload through the session table and updates transmit counters.
    ///
    /// The returned packet includes the endpoint binding so callers can send the encoded GRE
    /// bytes without a second session lookup.
    pub fn build_outbound_packet(
        &mut self,
        session_id: u32,
        payload: impl Into<Vec<u8>>,
    ) -> Result<OutboundPacket> {
        let payload = payload.into();
        let Some(entry) = self.sessions.get_mut(&session_id) else {
            return Err(Error::UnknownSession(session_id));
        };

        let gre_sequence = match entry.session.profile.sequencing {
            SequencingMode::Unsequenced => None,
            SequencingMode::Required => {
                let sequence = entry.next_tx_sequence;
                entry.next_tx_sequence = entry.next_tx_sequence.wrapping_add(1);
                Some(sequence)
            }
        };
        let gre_key = entry.session.outbound_session_key;
        let wire_bytes = GrePacket {
            key_present: true,
            sequence_present: gre_sequence.is_some(),
            protocol_type: entry.session.profile.protocol_type.as_u16(),
            key: Some(gre_key),
            sequence_number: gre_sequence,
            payload: payload.clone(),
        }
        .encode()?;
        let tx_ordinal = entry.next_tx_ordinal;
        entry.next_tx_ordinal += 1;
        self.stats.tx_packets += 1;
        self.stats.tx_bytes += payload.len() as u64;
        entry.stats.tx_packets += 1;
        entry.stats.tx_bytes += payload.len() as u64;
        entry.stats.last_tx_ordinal = tx_ordinal;
        entry.stats.last_tx_sequence = gre_sequence;

        Ok(OutboundPacket {
            session_id,
            gre_key,
            endpoint: entry.session.endpoint,
            tx_ordinal,
            gre_sequence,
            payload_len: payload.len(),
            wire_bytes,
        })
    }

    /// Encodes a payload for a known session and updates transmit counters.
    pub fn encode_for_session(
        &mut self,
        session_id: u32,
        payload: impl Into<Vec<u8>>,
    ) -> Result<Vec<u8>> {
        self.build_outbound_packet(session_id, payload)
            .map(|packet| packet.wire_bytes)
    }

    /// Decodes and validates a received GRE packet against the bearer table.
    pub fn decode_for_session(
        &mut self,
        endpoint: BearerEndpoint,
        input: &[u8],
    ) -> Result<InboundPacket> {
        let packet = match GrePacket::decode(input) {
            Ok(packet) => packet,
            Err(error) => {
                self.stats.malformed_packets += 1;
                return Err(error);
            }
        };
        let inbound_session_key = packet.key.ok_or_else(|| {
            self.stats.malformed_packets += 1;
            Error::MissingSessionKey
        })?;
        let Some(session_id) = self.inbound_key_index.get(&inbound_session_key).copied() else {
            self.stats.unknown_session_packets += 1;
            return Err(Error::UnknownInboundSessionKey(inbound_session_key));
        };
        let Some(entry) = self.sessions.get_mut(&session_id) else {
            self.stats.unknown_session_packets += 1;
            return Err(Error::UnknownInboundSessionKey(inbound_session_key));
        };
        if packet.protocol_type != entry.session.profile.protocol_type.as_u16() {
            self.stats.malformed_packets += 1;
            return Err(Error::InvalidProtocolType(packet.protocol_type));
        }
        let gre_sequence = match entry.session.profile.sequencing {
            SequencingMode::Unsequenced => packet.sequence_number,
            SequencingMode::Required => Some(packet.sequence_number.ok_or_else(|| {
                self.stats.malformed_packets += 1;
                Error::MissingSequenceNumber
            })?),
        };

        let accepts_endpoint = if entry.session.endpoint == endpoint {
            true
        } else {
            matches!(
                entry.transition,
                Some(SessionTransition {
                    previous_endpoint,
                    ..
                }) if previous_endpoint == endpoint
            )
        };
        if !accepts_endpoint {
            self.stats.dropped_packets += 1;
            self.stats.endpoint_mismatch_packets += 1;
            entry.stats.dropped_packets += 1;
            entry.stats.endpoint_mismatch_packets += 1;
            return Err(Error::EndpointMismatch { session_id });
        }

        if let Some(sequence) = gre_sequence {
            if let Some(last_sequence) = entry.stats.last_rx_sequence {
                let delta = sequence.wrapping_sub(last_sequence);
                if delta == 0 {
                    self.stats.duplicate_sequence_packets += 1;
                    entry.stats.duplicate_sequence_packets += 1;
                } else if delta & 0x8000_0000 != 0 {
                    self.stats.reordered_sequence_packets += 1;
                    entry.stats.reordered_sequence_packets += 1;
                } else if delta > 1 {
                    self.stats.sequence_gap_events += 1;
                    entry.stats.sequence_gap_events += 1;
                }
            }
            entry.stats.last_rx_sequence = Some(sequence);
        }

        self.stats.rx_packets += 1;
        self.stats.rx_bytes += packet.payload.len() as u64;
        entry.stats.rx_packets += 1;
        entry.stats.rx_bytes += packet.payload.len() as u64;
        let rx_ordinal = entry.next_rx_ordinal;
        entry.next_rx_ordinal += 1;
        entry.stats.last_rx_ordinal = rx_ordinal;
        if let Some(transition) = entry.transition {
            if endpoint == transition.previous_endpoint {
                self.stats.transition_rx_packets += 1;
                entry.stats.transition_rx_packets += 1;
            } else if matches!(transition.mode, RebindMode::HardHandoff) {
                entry.transition = None;
                self.stats.transitions_completed += 1;
            }
        }
        Ok(InboundPacket {
            session_id,
            gre_key: inbound_session_key,
            endpoint,
            rx_ordinal,
            gre_sequence,
            payload: packet.payload,
        })
    }

    fn validate_session(&self, session: BearerSession) -> Result<()> {
        if !session.endpoint.has_matching_address_family() {
            return Err(Error::AddressFamilyMismatch {
                session_id: session.session_id,
            });
        }
        Ok(())
    }

    fn reserve_inbound_key(&self, inbound_session_key: u32, session_id: Option<u32>) -> Result<()> {
        if let Some(existing_session_id) = self.inbound_key_index.get(&inbound_session_key).copied()
            && Some(existing_session_id) != session_id
        {
            return Err(Error::DuplicateInboundSessionKey(inbound_session_key));
        }
        Ok(())
    }
}

#[cfg(test)]
mod transport_tests {
    use super::*;

    fn loopback_endpoint() -> BearerEndpoint {
        BearerEndpoint::new([127, 0, 0, 1], [127, 0, 0, 1])
    }

    #[test]
    fn dispatch_selects_udp_for_udp_encapsulated_mode() {
        let bind: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let peer: SocketAddr = "127.0.0.1:55555".parse().unwrap();
        let config = BearerTransportConfig::udp_encapsulated_gre(bind, peer);
        let endpoint = GreBearerEndpoint::bind(&config, loopback_endpoint(), "test.udp")
            .expect("udp bind should succeed without elevated capability");
        assert!(matches!(endpoint, GreBearerEndpoint::Udp(_)));
    }

    #[test]
    fn dispatch_selects_raw_for_raw_gre_mode() {
        let config = BearerTransportConfig::raw_gre();
        // A raw IPPROTO_GRE socket needs CAP_NET_RAW. On a host without it the bind must
        // fail with a capability-naming error rather than silently selecting another mode.
        match GreBearerEndpoint::bind(&config, loopback_endpoint(), "test.raw") {
            Ok(GreBearerEndpoint::Raw(_)) => {}
            Ok(GreBearerEndpoint::Udp(_)) => {
                panic!("raw_gre must not select the UDP transport")
            }
            Err(Error::RawTransport(msg)) => {
                assert!(
                    msg.contains("CAP_NET_RAW"),
                    "raw bind failure must name the missing capability, got: {msg}"
                );
            }
            Err(other) => panic!("unexpected raw bind error: {other}"),
        }
    }

    #[test]
    fn raw_gre_rejects_udp_addresses() {
        let bind: SocketAddr = "127.0.0.1:0".parse().unwrap();
        let peer: SocketAddr = "127.0.0.1:55555".parse().unwrap();
        let mut config = BearerTransportConfig::udp_encapsulated_gre(bind, peer);
        config.mode = BearerTransportMode::RawGre;
        let err = RawGreEndpoint::bind(&config, loopback_endpoint(), "test.raw")
            .expect_err("raw_gre must reject udp_bind_addr/udp_peer_addr");
        assert!(matches!(err, Error::InvalidTransportConfig(_)));
    }

    #[test]
    fn ipv4_header_strip_recovers_gre_bytes() {
        let gre = GrePacket::octet_stream(0xDEAD_BEEF, Some(7), vec![1, 2, 3, 4]);
        let gre_bytes = gre.encode().unwrap();

        // Minimal IPv4 header: version 4, IHL 5 (20 bytes). Only the first byte's IHL
        // nibble drives the strip length, so the remaining header bytes are arbitrary.
        let mut frame = vec![0u8; 20];
        frame[0] = 0x45;
        frame.extend_from_slice(&gre_bytes);

        let recovered = strip_inbound_ip_header(IpAddr::V4(Ipv4Addr::LOCALHOST), &frame).unwrap();
        assert_eq!(recovered, gre_bytes.as_slice());
        let decoded = GrePacket::decode(recovered).unwrap();
        assert_eq!(decoded, gre);
    }

    #[test]
    fn ipv4_header_strip_with_options_uses_ihl() {
        let gre_bytes = GrePacket::octet_stream(1, None, vec![9, 9])
            .encode()
            .unwrap();
        // IHL 6 -> 24-byte header (one 4-byte options word).
        let mut frame = vec![0u8; 24];
        frame[0] = 0x46;
        frame.extend_from_slice(&gre_bytes);

        let recovered = strip_inbound_ip_header(IpAddr::V4(Ipv4Addr::LOCALHOST), &frame).unwrap();
        assert_eq!(recovered, gre_bytes.as_slice());
    }

    #[test]
    fn ipv4_header_strip_rejects_truncated_frame() {
        // Claims a 20-byte header but only 3 bytes are present.
        let frame = [0x45u8, 0x00, 0x00];
        let err = strip_inbound_ip_header(IpAddr::V4(Ipv4Addr::LOCALHOST), &frame)
            .expect_err("truncated");
        assert!(matches!(err, Error::Truncated { .. }));
    }

    #[test]
    fn ipv6_header_strip_is_identity() {
        let gre_bytes = GrePacket::octet_stream(2, None, vec![5]).encode().unwrap();
        let recovered =
            strip_inbound_ip_header(IpAddr::V6(Ipv6Addr::LOCALHOST), &gre_bytes).unwrap();
        assert_eq!(recovered, gre_bytes.as_slice());
    }
}
