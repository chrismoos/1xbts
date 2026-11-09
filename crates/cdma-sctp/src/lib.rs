//! One-to-one SCTP associations (RFC 9260) over Linux kernel SCTP (RFC 6458)
//! or the vendored usrsctp. Both ends of an association must use the same
//! transport, native SCTP or SCTP over UDP (RFC 6951).

mod addr;
mod config;
#[cfg(kernel_backend)]
mod kernel;
#[cfg(usrsctp_backend)]
mod usrsctp;

use std::borrow::Cow;
use std::io;
use std::net::SocketAddr;

pub use config::{SCTP_UDP_ENCAPSULATION_PORT, SctpBackend, SctpConfig, UdpEncapsulation};

/// A longer partial delivery marks the association broken.
pub const MAX_MESSAGE_LEN: usize = 1 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CloseReason {
    Shutdown,
    /// The peer sent ABORT or stopped responding (COMMUNICATION LOST).
    Lost,
    CannotStart,
}

impl std::fmt::Display for CloseReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Shutdown => "shutdown",
            Self::Lost => "communication lost",
            Self::CannotStart => "cannot start association",
        })
    }
}

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("SCTP I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("no SCTP addresses given")]
    NoAddresses,
    #[error("SCTP backend {backend:?} is not available on this host: {reason}")]
    BackendUnavailable {
        backend: SctpBackend,
        reason: String,
    },
    #[error(
        "usrsctp is already running with UDP encapsulation port {active}, requested {requested}"
    )]
    UsrsctpPortConflict { active: u16, requested: u16 },
    #[error("SCTP association closed: {0}")]
    Closed(CloseReason),
    #[error("SCTP message of {len} bytes exceeds {MAX_MESSAGE_LEN}")]
    MessageTooLarge { len: usize },
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Delivery {
    #[default]
    Ordered,
    Unordered,
}

/// Per-message send parameters (RFC 6458 §5.3.4 `sctp_sndinfo`).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SendInfo {
    pub stream_id: u16,
    /// Payload protocol identifier, in host order.
    pub ppid: u32,
    pub delivery: Delivery,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SctpMessage {
    pub data: Vec<u8>,
    pub stream_id: u16,
    /// Payload protocol identifier, in host order.
    pub ppid: u32,
    pub delivery: Delivery,
}

/// Peer address reachability, as reported by SCTP_PEER_ADDR_CHANGE.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum PathState {
    Available,
    Unreachable,
    Other(u32),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Notification {
    CommUp {
        outbound_streams: u16,
    },
    Closed(CloseReason),
    PeerShutdown,
    PathChange {
        addr: Option<SocketAddr>,
        state: PathState,
    },
    Other,
}

pub struct SctpListener {
    inner: ListenerInner,
}

enum ListenerInner {
    #[cfg(kernel_backend)]
    Kernel(kernel::Listener),
    #[cfg(usrsctp_backend)]
    Usrsctp(usrsctp::Listener),
}

/// `recv` serializes its callers.
pub struct SctpAssociation {
    inner: AssociationInner,
}

enum AssociationInner {
    #[cfg(kernel_backend)]
    Kernel(kernel::Association),
    #[cfg(usrsctp_backend)]
    Usrsctp(usrsctp::Association),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ResolvedBackend {
    #[cfg(kernel_backend)]
    Kernel,
    #[cfg(usrsctp_backend)]
    Usrsctp,
}

fn resolve_backend(config: &SctpConfig) -> Result<(ResolvedBackend, Cow<'_, SctpConfig>)> {
    #[cfg(kernel_backend)]
    let kernel_available = kernel_available();
    #[cfg(not(kernel_backend))]
    let kernel_available = false;
    resolve_backend_with(config, kernel_available)
}

fn resolve_backend_with(
    config: &SctpConfig,
    #[cfg_attr(not(kernel_backend), allow(unused_variables))] kernel_available: bool,
) -> Result<(ResolvedBackend, Cow<'_, SctpConfig>)> {
    let backend = config.backend;
    match backend {
        #[cfg(kernel_backend)]
        SctpBackend::Auto if kernel_available => {
            Ok((ResolvedBackend::Kernel, Cow::Borrowed(config)))
        }
        #[cfg(usrsctp_backend)]
        SctpBackend::Auto => {
            let mut config = config.clone();
            config.udp_encapsulation = config
                .udp_encapsulation
                .or(Some(config.fallback_udp_encapsulation));
            Ok((ResolvedBackend::Usrsctp, Cow::Owned(config)))
        }
        #[cfg(not(usrsctp_backend))]
        SctpBackend::Auto => Err(Error::BackendUnavailable {
            backend,
            reason: "kernel SCTP is unavailable and the build has no usrsctp backend".to_string(),
        }),
        #[cfg(kernel_backend)]
        SctpBackend::Kernel => Ok((ResolvedBackend::Kernel, Cow::Borrowed(config))),
        #[cfg(not(kernel_backend))]
        SctpBackend::Kernel => Err(Error::BackendUnavailable {
            backend,
            reason: "kernel SCTP is only supported on Linux".to_string(),
        }),
        #[cfg(usrsctp_backend)]
        SctpBackend::Usrsctp => Ok((ResolvedBackend::Usrsctp, Cow::Borrowed(config))),
        #[cfg(not(usrsctp_backend))]
        SctpBackend::Usrsctp => Err(Error::BackendUnavailable {
            backend,
            reason: "built without the `usrsctp` feature".to_string(),
        }),
    }
}

#[cfg(kernel_backend)]
fn kernel_available() -> bool {
    static AVAILABLE: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *AVAILABLE.get_or_init(|| match kernel::probe() {
        Ok(()) => true,
        Err(reason) => {
            log::warn!(
                "kernel SCTP is unavailable ({reason}). Using usrsctp over UDP. Load the sctp kernel module to use kernel SCTP"
            );
            false
        }
    })
}

impl SctpListener {
    /// All addresses in `local_addrs` must share one port.
    pub fn bind(local_addrs: &[SocketAddr], config: &SctpConfig) -> Result<Self> {
        let (backend, config) = resolve_backend(config)?;
        let inner = match backend {
            #[cfg(kernel_backend)]
            ResolvedBackend::Kernel => {
                ListenerInner::Kernel(kernel::Listener::bind(local_addrs, &config)?)
            }
            #[cfg(usrsctp_backend)]
            ResolvedBackend::Usrsctp => {
                ListenerInner::Usrsctp(usrsctp::Listener::bind(local_addrs, &config)?)
            }
        };
        Ok(Self { inner })
    }

    pub async fn accept(&self) -> Result<(SctpAssociation, SocketAddr)> {
        let (inner, peer) = match &self.inner {
            #[cfg(kernel_backend)]
            ListenerInner::Kernel(listener) => {
                let (assoc, peer) = listener.accept().await?;
                (AssociationInner::Kernel(assoc), peer)
            }
            #[cfg(usrsctp_backend)]
            ListenerInner::Usrsctp(listener) => {
                let (assoc, peer) = listener.accept().await?;
                (AssociationInner::Usrsctp(assoc), peer)
            }
        };
        Ok((SctpAssociation { inner }, peer))
    }

    pub fn local_addrs(&self) -> Result<Vec<SocketAddr>> {
        match &self.inner {
            #[cfg(kernel_backend)]
            ListenerInner::Kernel(listener) => listener.local_addrs(),
            #[cfg(usrsctp_backend)]
            ListenerInner::Usrsctp(listener) => listener.local_addrs(),
        }
    }
}

impl SctpAssociation {
    pub async fn connect(
        remote_addrs: &[SocketAddr],
        local_addrs: &[SocketAddr],
        config: &SctpConfig,
    ) -> Result<Self> {
        if remote_addrs.is_empty() {
            return Err(Error::NoAddresses);
        }
        let (backend, config) = resolve_backend(config)?;
        let inner = match backend {
            #[cfg(kernel_backend)]
            ResolvedBackend::Kernel => AssociationInner::Kernel(
                kernel::Association::connect(remote_addrs, local_addrs, &config).await?,
            ),
            #[cfg(usrsctp_backend)]
            ResolvedBackend::Usrsctp => AssociationInner::Usrsctp(
                usrsctp::Association::connect(remote_addrs, local_addrs, &config).await?,
            ),
        };
        Ok(Self { inner })
    }

    pub async fn send(&self, data: &[u8], info: SendInfo) -> Result<()> {
        match &self.inner {
            #[cfg(kernel_backend)]
            AssociationInner::Kernel(assoc) => assoc.send(data, info).await,
            #[cfg(usrsctp_backend)]
            AssociationInner::Usrsctp(assoc) => assoc.send(data, info).await,
        }
    }

    pub async fn recv(&self) -> Result<SctpMessage> {
        match &self.inner {
            #[cfg(kernel_backend)]
            AssociationInner::Kernel(assoc) => assoc.recv().await,
            #[cfg(usrsctp_backend)]
            AssociationInner::Usrsctp(assoc) => assoc.recv().await,
        }
    }

    pub fn shutdown(&self) -> Result<()> {
        match &self.inner {
            #[cfg(kernel_backend)]
            AssociationInner::Kernel(assoc) => assoc.shutdown(),
            #[cfg(usrsctp_backend)]
            AssociationInner::Usrsctp(assoc) => assoc.shutdown(),
        }
    }

    pub fn peer_addrs(&self) -> Result<Vec<SocketAddr>> {
        match &self.inner {
            #[cfg(kernel_backend)]
            AssociationInner::Kernel(assoc) => assoc.peer_addrs(),
            #[cfg(usrsctp_backend)]
            AssociationInner::Usrsctp(assoc) => assoc.peer_addrs(),
        }
    }

    pub fn local_addrs(&self) -> Result<Vec<SocketAddr>> {
        match &self.inner {
            #[cfg(kernel_backend)]
            AssociationInner::Kernel(assoc) => assoc.local_addrs(),
            #[cfg(usrsctp_backend)]
            AssociationInner::Usrsctp(assoc) => assoc.local_addrs(),
        }
    }

    /// Before COMMUNICATION UP, the number requested in the config.
    pub fn outbound_streams(&self) -> u16 {
        match &self.inner {
            #[cfg(kernel_backend)]
            AssociationInner::Kernel(assoc) => assoc.outbound_streams(),
            #[cfg(usrsctp_backend)]
            AssociationInner::Usrsctp(assoc) => assoc.outbound_streams(),
        }
    }
}

/// Notification type and state codes, which differ between the Linux kernel
/// (`linux/sctp.h`) and usrsctp (FreeBSD numbering).
pub(crate) struct NotificationCodes {
    pub(crate) assoc_change: u16,
    pub(crate) peer_addr_change: u16,
    pub(crate) shutdown_event: u16,
    pub(crate) comm_up: u16,
    pub(crate) comm_lost: u16,
    pub(crate) restart: u16,
    pub(crate) shutdown_comp: u16,
    pub(crate) cant_str_assoc: u16,
    pub(crate) addr_available: u32,
    pub(crate) addr_unreachable: u32,
}

/// Byte offsets shared by both stacks' notification structures (RFC 6458
/// §6.1): `sn_type` at 0, `sac_state` at 8, `sac_outbound_streams` at 12,
/// `spc_aaddr` at 8 and `spc_state` after the 128-byte `sockaddr_storage`.
const SN_TYPE_OFFSET: usize = 0;
const SAC_STATE_OFFSET: usize = 8;
const SAC_OUTBOUND_STREAMS_OFFSET: usize = 12;
const SPC_AADDR_OFFSET: usize = 8;
const SPC_STATE_OFFSET: usize = SPC_AADDR_OFFSET + std::mem::size_of::<libc::sockaddr_storage>();

fn read_u16(buf: &[u8], offset: usize) -> Option<u16> {
    let bytes = buf.get(offset..offset + 2)?;
    Some(u16::from_ne_bytes([bytes[0], bytes[1]]))
}

fn read_u32(buf: &[u8], offset: usize) -> Option<u32> {
    let bytes = buf.get(offset..offset + 4)?;
    Some(u32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
}

pub(crate) fn parse_notification(buf: &[u8], codes: &NotificationCodes) -> Notification {
    let Some(sn_type) = read_u16(buf, SN_TYPE_OFFSET) else {
        return Notification::Other;
    };
    if sn_type == codes.assoc_change {
        let (Some(state), Some(outbound_streams)) = (
            read_u16(buf, SAC_STATE_OFFSET),
            read_u16(buf, SAC_OUTBOUND_STREAMS_OFFSET),
        ) else {
            return Notification::Other;
        };
        return match state {
            s if s == codes.comm_up => Notification::CommUp { outbound_streams },
            s if s == codes.comm_lost || s == codes.restart => {
                Notification::Closed(CloseReason::Lost)
            }
            s if s == codes.shutdown_comp => Notification::Closed(CloseReason::Shutdown),
            s if s == codes.cant_str_assoc => Notification::Closed(CloseReason::CannotStart),
            _ => Notification::Other,
        };
    }
    if sn_type == codes.peer_addr_change {
        let Some(raw_state) = read_u32(buf, SPC_STATE_OFFSET) else {
            return Notification::Other;
        };
        let addr = buf
            .get(SPC_AADDR_OFFSET..SPC_STATE_OFFSET)
            .and_then(|bytes| addr::unpack(bytes, 1).into_iter().next());
        let state = match raw_state {
            s if s == codes.addr_available => PathState::Available,
            s if s == codes.addr_unreachable => PathState::Unreachable,
            s => PathState::Other(s),
        };
        return Notification::PathChange { addr, state };
    }
    if sn_type == codes.shutdown_event {
        return Notification::PeerShutdown;
    }
    Notification::Other
}

pub(crate) struct Chunk<'a> {
    pub(crate) data: &'a [u8],
    pub(crate) is_notification: bool,
    pub(crate) end_of_record: bool,
    pub(crate) stream_id: u16,
    pub(crate) ppid: u32,
    pub(crate) delivery: Delivery,
}

pub(crate) struct RecvState {
    reassembly: Reassembly,
    closed: Option<CloseReason>,
    established: bool,
    pending: std::collections::VecDeque<SctpMessage>,
}

impl RecvState {
    pub(crate) fn new() -> Self {
        Self {
            reassembly: Reassembly::default(),
            closed: None,
            established: false,
            pending: std::collections::VecDeque::new(),
        }
    }

    pub(crate) fn is_established(&self) -> bool {
        self.established
    }

    pub(crate) fn stash(&mut self, message: SctpMessage) {
        self.pending.push_back(message);
    }

    pub(crate) fn take_pending(&mut self) -> Option<SctpMessage> {
        self.pending.pop_front()
    }

    pub(crate) fn closed(&self) -> Option<CloseReason> {
        self.closed
    }

    pub(crate) fn mark_closed(&mut self, reason: CloseReason) -> Error {
        self.closed = Some(reason);
        Error::Closed(reason)
    }

    pub(crate) fn absorb(
        &mut self,
        chunk: Chunk<'_>,
        codes: &NotificationCodes,
        outbound_streams: &std::sync::atomic::AtomicU16,
    ) -> Result<Option<SctpMessage>> {
        let Some(data) = self.reassembly.push(chunk.data, chunk.end_of_record)? else {
            return Ok(None);
        };
        if !chunk.is_notification {
            return Ok(Some(SctpMessage {
                data,
                stream_id: chunk.stream_id,
                ppid: chunk.ppid,
                delivery: chunk.delivery,
            }));
        }
        match parse_notification(&data, codes) {
            Notification::CommUp {
                outbound_streams: granted,
            } => {
                // Relaxed: the count is advisory for stream selection and is
                // not used to order any other memory access.
                outbound_streams.store(granted, std::sync::atomic::Ordering::Relaxed);
                self.established = true;
                log::debug!("sctp association up outbound_streams={granted}");
            }
            Notification::Closed(reason) => return Err(self.mark_closed(reason)),
            Notification::PeerShutdown => log::info!("sctp peer started shutdown"),
            Notification::PathChange { addr, state } => log_path_change(addr, state),
            Notification::Other => {}
        }
        Ok(None)
    }
}

#[derive(Default)]
pub(crate) struct Reassembly {
    buf: Vec<u8>,
}

impl Reassembly {
    pub(crate) fn push(&mut self, chunk: &[u8], end_of_record: bool) -> Result<Option<Vec<u8>>> {
        let len = self.buf.len() + chunk.len();
        if len > MAX_MESSAGE_LEN {
            self.buf.clear();
            return Err(Error::MessageTooLarge { len });
        }
        self.buf.extend_from_slice(chunk);
        if !end_of_record {
            return Ok(None);
        }
        Ok(Some(std::mem::take(&mut self.buf)))
    }
}

pub(crate) fn log_path_change(addr: Option<SocketAddr>, state: PathState) {
    let addr = addr.map_or_else(|| "unknown".to_string(), |addr| addr.to_string());
    match state {
        PathState::Available => log::info!("sctp path available peer_addr={addr}"),
        PathState::Unreachable => log::warn!("sctp path unreachable peer_addr={addr}"),
        PathState::Other(state) => log::debug!("sctp path change peer_addr={addr} state={state}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reassembly_returns_message_only_at_end_of_record() {
        let mut reassembly = Reassembly::default();
        assert_eq!(reassembly.push(b"ab", false).unwrap(), None);
        assert_eq!(
            reassembly.push(b"cd", true).unwrap(),
            Some(b"abcd".to_vec())
        );
        assert_eq!(reassembly.push(b"e", true).unwrap(), Some(b"e".to_vec()));
    }

    #[test]
    fn reassembly_rejects_oversized_message() {
        let mut reassembly = Reassembly::default();
        let chunk = vec![0u8; MAX_MESSAGE_LEN];
        assert_eq!(reassembly.push(&chunk, false).unwrap(), None);
        assert!(matches!(
            reassembly.push(b"x", true),
            Err(Error::MessageTooLarge { .. })
        ));
    }

    #[cfg(usrsctp_backend)]
    #[test]
    fn auto_without_kernel_sctp_uses_usrsctp_over_udp() {
        let config = SctpConfig {
            fallback_udp_encapsulation: UdpEncapsulation {
                local_port: 9900,
                remote_port: 9899,
            },
            ..SctpConfig::default()
        };
        let (backend, resolved) = resolve_backend_with(&config, false).unwrap();
        assert_eq!(backend, ResolvedBackend::Usrsctp);
        assert_eq!(
            resolved.udp_encapsulation,
            Some(config.fallback_udp_encapsulation)
        );

        let explicit = SctpConfig {
            udp_encapsulation: Some(UdpEncapsulation::default()),
            ..config
        };
        let (_, resolved) = resolve_backend_with(&explicit, false).unwrap();
        assert_eq!(resolved.udp_encapsulation, explicit.udp_encapsulation);
    }

    #[cfg(kernel_backend)]
    #[test]
    fn auto_with_kernel_sctp_keeps_the_config() {
        let config = SctpConfig::default();
        let (backend, resolved) = resolve_backend_with(&config, true).unwrap();
        assert_eq!(backend, ResolvedBackend::Kernel);
        assert_eq!(resolved.udp_encapsulation, None);
    }

    #[cfg(usrsctp_backend)]
    #[test]
    fn explicit_usrsctp_keeps_raw_ip() {
        let config = SctpConfig {
            backend: SctpBackend::Usrsctp,
            ..SctpConfig::default()
        };
        let (_, resolved) = resolve_backend_with(&config, false).unwrap();
        assert_eq!(resolved.udp_encapsulation, None);
    }
}
