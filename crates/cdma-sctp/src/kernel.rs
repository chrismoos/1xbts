//! Linux kernel SCTP over one-to-one `SOCK_STREAM` sockets (RFC 6458).

use std::io;
use std::mem::{size_of, zeroed};
use std::net::SocketAddr;
use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
use std::sync::atomic::{AtomicU16, Ordering};

use tokio::io::Interest;
use tokio::io::unix::AsyncFd;
use tokio::sync::Mutex;

use crate::addr::{self, Family};
use crate::{
    Chunk, CloseReason, Delivery, Error, NotificationCodes, RecvState, Result, SctpBackend,
    SctpConfig, SctpMessage, SendInfo,
};

const SOL_SCTP: libc::c_int = 132;
const SCTP_SOCKOPT_BINDX_ADD: libc::c_int = 100;
const SCTP_GET_PEER_ADDRS: libc::c_int = 108;
const SCTP_GET_LOCAL_ADDRS: libc::c_int = 109;
const SCTP_SOCKOPT_CONNECTX: libc::c_int = 110;
const SCTP_EVENT: libc::c_int = 127;
const SCTP_REMOTE_UDP_ENCAPS_PORT: libc::c_int = 132;

const SCTP_UNORDERED: u16 = 1 << 0;
const SPP_HB_ENABLE: u32 = 1 << 0;

const SCTP_ASSOC_CHANGE: u16 = 0x8001;
const SCTP_PEER_ADDR_CHANGE: u16 = 0x8002;
const SCTP_SHUTDOWN_EVENT: u16 = 0x8005;

const CODES: NotificationCodes = NotificationCodes {
    assoc_change: SCTP_ASSOC_CHANGE,
    peer_addr_change: SCTP_PEER_ADDR_CHANGE,
    shutdown_event: SCTP_SHUTDOWN_EVENT,
    comm_up: 0,
    comm_lost: 1,
    restart: 2,
    shutdown_comp: 3,
    cant_str_assoc: 4,
    addr_available: 0,
    addr_unreachable: 1,
};

/// `struct sctp_paddrparams` is `packed, aligned(4)`: 155 bytes of fields
/// rounded to 156, and the kernel rejects any other option length.
const PADDRPARAMS_LEN: usize = 156;
const PADDRPARAMS_HBINTERVAL_OFFSET: usize = 132;
const PADDRPARAMS_PATHMAXRXT_OFFSET: usize = 136;
const PADDRPARAMS_FLAGS_OFFSET: usize = 146;

const RECV_BUF_LEN: usize = 64 * 1024;
const LISTEN_BACKLOG: libc::c_int = 16;

#[repr(C)]
struct SctpEvent {
    se_assoc_id: libc::sctp_assoc_t,
    se_type: u16,
    se_on: u8,
}

#[repr(C)]
struct SctpRtoinfo {
    srto_assoc_id: libc::sctp_assoc_t,
    srto_initial: u32,
    srto_max: u32,
    srto_min: u32,
}

#[repr(C)]
struct SctpUdpencaps {
    sue_assoc_id: libc::sctp_assoc_t,
    sue_address: libc::sockaddr_storage,
    sue_port: u16,
}

fn cvt(ret: libc::c_int) -> io::Result<libc::c_int> {
    if ret < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(ret)
    }
}

fn setsockopt_bytes(
    fd: RawFd,
    level: libc::c_int,
    name: libc::c_int,
    value: &[u8],
) -> io::Result<()> {
    // SAFETY: `value` is valid for `value.len()` bytes for the duration of the
    // call, and the kernel copies it before returning.
    cvt(unsafe {
        libc::setsockopt(
            fd,
            level,
            name,
            value.as_ptr().cast(),
            value.len() as libc::socklen_t,
        )
    })?;
    Ok(())
}

fn setsockopt<T>(fd: RawFd, level: libc::c_int, name: libc::c_int, value: &T) -> io::Result<()> {
    // SAFETY: `T` is a `repr(C)` option struct or integer, so viewing it as
    // bytes is valid for its whole size.
    let bytes =
        unsafe { std::slice::from_raw_parts((value as *const T).cast::<u8>(), size_of::<T>()) };
    setsockopt_bytes(fd, level, name, bytes)
}

fn open_socket(family: Family) -> Result<OwnedFd> {
    // SAFETY: plain socket(2) call. The returned descriptor is owned below.
    let ret = unsafe {
        libc::socket(
            family.domain(),
            libc::SOCK_STREAM | libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
            libc::IPPROTO_SCTP,
        )
    };
    if ret < 0 {
        let err = io::Error::last_os_error();
        if matches!(
            err.raw_os_error(),
            Some(libc::EPROTONOSUPPORT) | Some(libc::ESOCKTNOSUPPORT)
        ) {
            return Err(Error::BackendUnavailable {
                backend: SctpBackend::Kernel,
                reason: format!("{err} (is the sctp kernel module loaded?)"),
            });
        }
        return Err(err.into());
    }
    // SAFETY: `ret` is a fresh descriptor returned by socket(2) and owned by
    // nothing else.
    Ok(unsafe { OwnedFd::from_raw_fd(ret) })
}

pub(crate) fn probe() -> std::result::Result<(), String> {
    open_socket(Family::V4)
        .map(drop)
        .map_err(|err| err.to_string())
}

/// Options that must be in place before INIT is sent or accepted.
fn apply_endpoint_options(fd: RawFd, config: &SctpConfig) -> io::Result<()> {
    let initmsg = libc::sctp_initmsg {
        sinit_num_ostreams: config.outbound_streams,
        sinit_max_instreams: config.max_inbound_streams,
        sinit_max_attempts: 0,
        sinit_max_init_timeo: 0,
    };
    setsockopt(fd, SOL_SCTP, libc::SCTP_INITMSG, &initmsg)?;

    if config.has_rto_settings() {
        let rto = SctpRtoinfo {
            srto_assoc_id: 0,
            srto_initial: config.rto_initial_ms.unwrap_or(0),
            srto_max: config.rto_max_ms.unwrap_or(0),
            srto_min: config.rto_min_ms.unwrap_or(0),
        };
        setsockopt(fd, SOL_SCTP, libc::SCTP_RTOINFO, &rto)?;
    }

    if config.has_path_settings() {
        let mut params = [0u8; PADDRPARAMS_LEN];
        if let Some(interval_ms) = config.heartbeat_interval_ms {
            params[PADDRPARAMS_HBINTERVAL_OFFSET..PADDRPARAMS_HBINTERVAL_OFFSET + 4]
                .copy_from_slice(&interval_ms.to_ne_bytes());
            params[PADDRPARAMS_FLAGS_OFFSET..PADDRPARAMS_FLAGS_OFFSET + 4]
                .copy_from_slice(&SPP_HB_ENABLE.to_ne_bytes());
        }
        if let Some(max_rxt) = config.path_max_retransmissions {
            params[PADDRPARAMS_PATHMAXRXT_OFFSET..PADDRPARAMS_PATHMAXRXT_OFFSET + 2]
                .copy_from_slice(&max_rxt.to_ne_bytes());
        }
        setsockopt_bytes(fd, SOL_SCTP, libc::SCTP_PEER_ADDR_PARAMS, &params)?;
    }

    // The remote encapsulation port is an endpoint default. Accepted
    // associations take the port from the peer's INIT.
    if let Some(encap) = config.udp_encapsulation {
        let encaps = SctpUdpencaps {
            sue_assoc_id: 0,
            // SAFETY: an all-zero `sockaddr_storage` (AF_UNSPEC) applies the
            // port to every peer address.
            sue_address: unsafe { zeroed() },
            sue_port: encap.remote_port.to_be(),
        };
        setsockopt(fd, SOL_SCTP, SCTP_REMOTE_UDP_ENCAPS_PORT, &encaps)?;
    }
    apply_association_options(fd)
}

/// Options set on every association socket, including accepted ones.
fn apply_association_options(fd: RawFd) -> io::Result<()> {
    let on: libc::c_int = 1;
    setsockopt(fd, SOL_SCTP, libc::SCTP_NODELAY, &on)?;
    setsockopt(fd, SOL_SCTP, libc::SCTP_RECVRCVINFO, &on)?;
    for se_type in [
        SCTP_ASSOC_CHANGE,
        SCTP_PEER_ADDR_CHANGE,
        SCTP_SHUTDOWN_EVENT,
    ] {
        let event = SctpEvent {
            se_assoc_id: 0,
            se_type,
            se_on: 1,
        };
        setsockopt(fd, SOL_SCTP, SCTP_EVENT, &event)?;
    }
    Ok(())
}

fn bindx(fd: RawFd, addrs: &[SocketAddr]) -> io::Result<()> {
    setsockopt_bytes(fd, SOL_SCTP, SCTP_SOCKOPT_BINDX_ADD, &addr::pack(addrs))
}

fn sctp_addrs(fd: RawFd, option: libc::c_int) -> Result<Vec<SocketAddr>> {
    const GETADDRS_BUF_LEN: usize = 4096;
    const ADDRS_OFFSET: usize = 8;
    let mut buf = vec![0u8; GETADDRS_BUF_LEN];
    let mut len = buf.len() as libc::socklen_t;
    // SAFETY: `buf` is writable for `len` bytes. `struct sctp_getaddrs` starts
    // with the input `assoc_id` (0 for a one-to-one socket) and the output
    // `addr_num`, followed by the packed addresses.
    cvt(unsafe {
        libc::getsockopt(
            fd,
            SOL_SCTP,
            option,
            buf.as_mut_ptr().cast(),
            std::ptr::addr_of_mut!(len),
        )
    })?;
    let count = u32::from_ne_bytes([buf[4], buf[5], buf[6], buf[7]]) as usize;
    // The kernel sets `len` to the address bytes only, excluding the header.
    let end = (ADDRS_OFFSET + len as usize).min(buf.len());
    Ok(addr::unpack(&buf[ADDRS_OFFSET..end], count))
}

fn socket_error(fd: RawFd) -> io::Result<()> {
    let mut err: libc::c_int = 0;
    let mut len = size_of::<libc::c_int>() as libc::socklen_t;
    // SAFETY: `err` is a writable c_int and `len` holds its size.
    cvt(unsafe {
        libc::getsockopt(
            fd,
            libc::SOL_SOCKET,
            libc::SO_ERROR,
            std::ptr::addr_of_mut!(err).cast(),
            std::ptr::addr_of_mut!(len),
        )
    })?;
    if err != 0 {
        return Err(io::Error::from_raw_os_error(err));
    }
    Ok(())
}

pub(crate) struct Listener {
    fd: AsyncFd<OwnedFd>,
    config: SctpConfig,
}

impl Listener {
    pub(crate) fn bind(local_addrs: &[SocketAddr], config: &SctpConfig) -> Result<Self> {
        let fd = open_socket(Family::for_addrs(local_addrs)?)?;
        apply_endpoint_options(fd.as_raw_fd(), config)?;
        bindx(fd.as_raw_fd(), local_addrs)?;
        // SAFETY: listen(2) on a descriptor this function owns.
        cvt(unsafe { libc::listen(fd.as_raw_fd(), LISTEN_BACKLOG) })?;
        Ok(Self {
            fd: AsyncFd::with_interest(fd, Interest::READABLE)?,
            config: config.clone(),
        })
    }

    pub(crate) async fn accept(&self) -> Result<(Association, SocketAddr)> {
        loop {
            let mut guard = self.fd.readable().await?;
            // SAFETY: an all-zero `sockaddr_storage` is valid, and accept4 is
            // told its exact size.
            let mut storage: libc::sockaddr_storage = unsafe { zeroed() };
            let mut len = size_of::<libc::sockaddr_storage>() as libc::socklen_t;
            let result = guard.try_io(|fd| {
                // SAFETY: `storage` is writable for `len` bytes.
                cvt(unsafe {
                    libc::accept4(
                        fd.as_raw_fd(),
                        std::ptr::addr_of_mut!(storage).cast(),
                        std::ptr::addr_of_mut!(len),
                        libc::SOCK_NONBLOCK | libc::SOCK_CLOEXEC,
                    )
                })
            });
            let Ok(result) = result else {
                continue;
            };
            let raw = result?;
            // SAFETY: `raw` is a fresh descriptor returned by accept4.
            let fd = unsafe { OwnedFd::from_raw_fd(raw) };
            apply_association_options(fd.as_raw_fd())?;
            // SAFETY: accept4 wrote a valid address of `len` bytes.
            let bytes = unsafe {
                std::slice::from_raw_parts(std::ptr::addr_of!(storage).cast::<u8>(), len as usize)
            };
            let peer = addr::sockaddr_to_socket_addr(bytes).ok_or_else(|| {
                io::Error::new(io::ErrorKind::InvalidData, "unknown peer address family")
            })?;
            return Ok((Association::new(fd, &self.config)?, peer));
        }
    }

    pub(crate) fn local_addrs(&self) -> Result<Vec<SocketAddr>> {
        sctp_addrs(self.fd.as_raw_fd(), SCTP_GET_LOCAL_ADDRS)
    }
}

pub(crate) struct Association {
    fd: AsyncFd<OwnedFd>,
    recv: Mutex<RecvState>,
    outbound_streams: AtomicU16,
}

impl Association {
    fn new(fd: OwnedFd, config: &SctpConfig) -> Result<Self> {
        Ok(Self {
            fd: AsyncFd::new(fd)?,
            recv: Mutex::new(RecvState::new()),
            outbound_streams: AtomicU16::new(config.outbound_streams),
        })
    }

    pub(crate) async fn connect(
        remote_addrs: &[SocketAddr],
        local_addrs: &[SocketAddr],
        config: &SctpConfig,
    ) -> Result<Self> {
        let mut all = remote_addrs.to_vec();
        all.extend_from_slice(local_addrs);
        let fd = open_socket(Family::for_addrs(&all)?)?;
        apply_endpoint_options(fd.as_raw_fd(), config)?;
        if !local_addrs.is_empty() {
            bindx(fd.as_raw_fd(), local_addrs)?;
        }
        match setsockopt_bytes(
            fd.as_raw_fd(),
            SOL_SCTP,
            SCTP_SOCKOPT_CONNECTX,
            &addr::pack(remote_addrs),
        ) {
            Ok(()) => {}
            Err(err) if err.raw_os_error() == Some(libc::EINPROGRESS) => {}
            Err(err) => return Err(err.into()),
        }
        let assoc = Self::new(fd, config)?;
        assoc.wait_established().await?;
        Ok(assoc)
    }

    /// Reads until COMMUNICATION UP. A socket in COOKIE-WAIT already polls
    /// writable, so writability does not show that the handshake finished.
    async fn wait_established(&self) -> Result<()> {
        let mut state = self.recv.lock().await;
        let mut buf = vec![0u8; RECV_BUF_LEN];
        while !state.is_established() {
            if let Some(message) = self.recv_chunk(&mut state, &mut buf).await? {
                state.stash(message);
            }
        }
        socket_error(self.fd.as_raw_fd())?;
        Ok(())
    }

    pub(crate) async fn send(&self, data: &[u8], info: SendInfo) -> Result<()> {
        let sndinfo = libc::sctp_sndinfo {
            snd_sid: info.stream_id,
            snd_flags: match info.delivery {
                Delivery::Ordered => 0,
                Delivery::Unordered => SCTP_UNORDERED,
            },
            snd_ppid: info.ppid.to_be(),
            snd_context: 0,
            snd_assoc_id: 0,
        };
        loop {
            let mut guard = self.fd.writable().await?;
            match guard.try_io(|fd| sendmsg_with_sndinfo(fd.as_raw_fd(), data, &sndinfo)) {
                Ok(Ok(())) => return Ok(()),
                Ok(Err(err)) if err.raw_os_error() == Some(libc::EPIPE) => {
                    return Err(Error::Closed(CloseReason::Shutdown));
                }
                Ok(Err(err)) => return Err(err.into()),
                Err(_would_block) => continue,
            }
        }
    }

    pub(crate) async fn recv(&self) -> Result<SctpMessage> {
        let mut state = self.recv.lock().await;
        if let Some(message) = state.take_pending() {
            return Ok(message);
        }
        if let Some(reason) = state.closed() {
            return Err(Error::Closed(reason));
        }
        let mut buf = vec![0u8; RECV_BUF_LEN];
        loop {
            if let Some(message) = self.recv_chunk(&mut state, &mut buf).await? {
                return Ok(message);
            }
        }
    }

    async fn recv_chunk(
        &self,
        state: &mut RecvState,
        buf: &mut [u8],
    ) -> Result<Option<SctpMessage>> {
        loop {
            let mut guard = self.fd.readable().await?;
            let Ok(result) = guard.try_io(|fd| recvmsg_with_rcvinfo(fd.as_raw_fd(), buf)) else {
                continue;
            };
            let received = result?;
            if received.len == 0 {
                return Err(state.mark_closed(CloseReason::Shutdown));
            }
            let chunk = Chunk {
                data: &buf[..received.len],
                is_notification: received.flags & libc::MSG_NOTIFICATION != 0,
                end_of_record: received.flags & libc::MSG_EOR != 0,
                stream_id: received.rcvinfo.rcv_sid,
                ppid: u32::from_be(received.rcvinfo.rcv_ppid),
                delivery: if received.rcvinfo.rcv_flags & SCTP_UNORDERED != 0 {
                    Delivery::Unordered
                } else {
                    Delivery::Ordered
                },
            };
            return state.absorb(chunk, &CODES, &self.outbound_streams);
        }
    }

    pub(crate) fn shutdown(&self) -> Result<()> {
        // SAFETY: shutdown(2) on a descriptor this association owns.
        cvt(unsafe { libc::shutdown(self.fd.as_raw_fd(), libc::SHUT_WR) })?;
        Ok(())
    }

    pub(crate) fn peer_addrs(&self) -> Result<Vec<SocketAddr>> {
        sctp_addrs(self.fd.as_raw_fd(), SCTP_GET_PEER_ADDRS)
    }

    pub(crate) fn local_addrs(&self) -> Result<Vec<SocketAddr>> {
        sctp_addrs(self.fd.as_raw_fd(), SCTP_GET_LOCAL_ADDRS)
    }

    pub(crate) fn outbound_streams(&self) -> u16 {
        // Relaxed: see `RecvState::absorb`.
        self.outbound_streams.load(Ordering::Relaxed)
    }
}

fn cmsg_space<T>() -> usize {
    // SAFETY: CMSG_SPACE is a pure size computation.
    unsafe { libc::CMSG_SPACE(size_of::<T>() as u32) as usize }
}

fn sendmsg_with_sndinfo(fd: RawFd, data: &[u8], sndinfo: &libc::sctp_sndinfo) -> io::Result<()> {
    let mut control = vec![0u8; cmsg_space::<libc::sctp_sndinfo>()];
    let mut iov = libc::iovec {
        iov_base: data.as_ptr().cast_mut().cast(),
        iov_len: data.len(),
    };
    // SAFETY: an all-zero msghdr is valid. Every pointer set below outlives
    // the sendmsg call, and the control buffer is sized by CMSG_SPACE for
    // exactly one `sctp_sndinfo`, so CMSG_FIRSTHDR is non-null.
    unsafe {
        let mut msg: libc::msghdr = zeroed();
        msg.msg_iov = std::ptr::addr_of_mut!(iov);
        msg.msg_iovlen = 1;
        msg.msg_control = control.as_mut_ptr().cast();
        msg.msg_controllen = control.len();
        let cmsg = libc::CMSG_FIRSTHDR(&msg);
        (*cmsg).cmsg_level = libc::IPPROTO_SCTP;
        (*cmsg).cmsg_type = libc::SCTP_SNDINFO;
        (*cmsg).cmsg_len = libc::CMSG_LEN(size_of::<libc::sctp_sndinfo>() as u32) as usize;
        std::ptr::write_unaligned(libc::CMSG_DATA(cmsg).cast::<libc::sctp_sndinfo>(), *sndinfo);
        let sent = libc::sendmsg(fd, &msg, libc::MSG_NOSIGNAL);
        if sent < 0 {
            return Err(io::Error::last_os_error());
        }
    }
    Ok(())
}

struct Received {
    len: usize,
    flags: libc::c_int,
    rcvinfo: libc::sctp_rcvinfo,
}

fn recvmsg_with_rcvinfo(fd: RawFd, buf: &mut [u8]) -> io::Result<Received> {
    let mut control = vec![0u8; cmsg_space::<libc::sctp_rcvinfo>()];
    let mut iov = libc::iovec {
        iov_base: buf.as_mut_ptr().cast(),
        iov_len: buf.len(),
    };
    // SAFETY: an all-zero msghdr and sctp_rcvinfo are valid. The iovec and
    // control buffers outlive the call, and CMSG_NXTHDR only visits headers
    // inside `msg_controllen`.
    unsafe {
        let mut msg: libc::msghdr = zeroed();
        msg.msg_iov = std::ptr::addr_of_mut!(iov);
        msg.msg_iovlen = 1;
        msg.msg_control = control.as_mut_ptr().cast();
        msg.msg_controllen = control.len();
        let len = libc::recvmsg(fd, &mut msg, 0);
        if len < 0 {
            return Err(io::Error::last_os_error());
        }
        let mut rcvinfo: libc::sctp_rcvinfo = zeroed();
        let mut cmsg = libc::CMSG_FIRSTHDR(&msg);
        while !cmsg.is_null() {
            if (*cmsg).cmsg_level == libc::IPPROTO_SCTP && (*cmsg).cmsg_type == libc::SCTP_RCVINFO {
                rcvinfo =
                    std::ptr::read_unaligned(libc::CMSG_DATA(cmsg).cast::<libc::sctp_rcvinfo>());
            }
            cmsg = libc::CMSG_NXTHDR(&msg, cmsg);
        }
        Ok(Received {
            len: len as usize,
            flags: msg.msg_flags,
            rcvinfo,
        })
    }
}
