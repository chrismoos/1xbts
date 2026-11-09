//! usrsctp is one process-wide stack: the first configuration that asks for
//! UDP encapsulation sets its port for the whole process. Upcalls run on
//! usrsctp threads and may arrive after a socket closes.

use std::collections::HashMap;
use std::io;
use std::mem::{size_of, zeroed};
use std::net::SocketAddr;
use std::sync::atomic::{AtomicU16, Ordering};
use std::sync::{Arc, LazyLock, Mutex as StdMutex, OnceLock, Weak};
use std::time::Duration;

use tokio::sync::{Mutex, Notify};
use usrsctp_sys as sys;

use crate::addr::{self, Family};
use crate::{
    Chunk, CloseReason, Delivery, Error, NotificationCodes, RecvState, Result, SctpConfig,
    SctpMessage, SendInfo,
};

const CODES: NotificationCodes = NotificationCodes {
    assoc_change: sys::SCTP_ASSOC_CHANGE,
    peer_addr_change: sys::SCTP_PEER_ADDR_CHANGE,
    shutdown_event: sys::SCTP_SHUTDOWN_EVENT,
    comm_up: sys::SCTP_COMM_UP,
    comm_lost: sys::SCTP_COMM_LOST,
    restart: sys::SCTP_RESTART,
    shutdown_comp: sys::SCTP_SHUTDOWN_COMP,
    cant_str_assoc: sys::SCTP_CANT_STR_ASSOC,
    addr_available: sys::SCTP_ADDR_AVAILABLE,
    addr_unreachable: sys::SCTP_ADDR_UNREACHABLE,
};

const RECV_BUF_LEN: usize = 64 * 1024;
const LISTEN_BACKLOG: libc::c_int = 16;

/// UDP encapsulation port the stack was started with. 0 means raw IP only.
static STACK_UDP_PORT: OnceLock<u16> = OnceLock::new();

fn ensure_stack(config: &SctpConfig) -> Result<()> {
    let requested = config.udp_encapsulation.map_or(0, |encap| encap.local_port);
    let active = *STACK_UDP_PORT.get_or_init(|| {
        // SAFETY: usrsctp_init runs exactly once per process, guarded by the
        // OnceLock. No conn_output or debug callback is installed.
        unsafe { sys::usrsctp_init(requested, None, None) };
        log::info!("usrsctp stack started udp_encapsulation_port={requested}");
        requested
    });
    if requested != 0 && requested != active {
        return Err(Error::UsrsctpPortConflict { active, requested });
    }
    Ok(())
}

#[derive(Default)]
struct Readiness {
    read: Notify,
    write: Notify,
}

static REGISTRY: LazyLock<StdMutex<HashMap<usize, Weak<Readiness>>>> =
    LazyLock::new(|| StdMutex::new(HashMap::new()));

unsafe extern "C" fn upcall(so: *mut sys::socket, _arg: *mut libc::c_void, _flags: libc::c_int) {
    // SAFETY: usrsctp passes the socket the upcall was registered on, which is
    // open for the duration of the callback.
    let events = unsafe { sys::usrsctp_get_events(so) };
    let registry = REGISTRY
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let Some(readiness) = registry.get(&(so as usize)).and_then(Weak::upgrade) else {
        return;
    };
    if events < 0 {
        readiness.read.notify_one();
        readiness.write.notify_one();
        return;
    }
    if events & (sys::SCTP_EVENT_READ | sys::SCTP_EVENT_ERROR) != 0 {
        readiness.read.notify_one();
    }
    if events & (sys::SCTP_EVENT_WRITE | sys::SCTP_EVENT_ERROR) != 0 {
        readiness.write.notify_one();
    }
}

struct Socket {
    raw: *mut sys::socket,
    readiness: Arc<Readiness>,
    close_after_association: bool,
}

const ASSOCIATION_END_WAIT: Duration = Duration::from_secs(5);
const ASSOCIATION_END_POLL: Duration = Duration::from_millis(10);

struct ClosingSocket(*mut sys::socket);

// SAFETY: the closing thread is the only owner left, and usrsctp sockets may
// be used from any thread.
unsafe impl Send for ClosingSocket {}

impl ClosingSocket {
    fn has_association(&self) -> bool {
        let mut list: *mut libc::sockaddr = std::ptr::null_mut();
        // SAFETY: the socket is open, and a returned list is freed with the
        // matching usrsctp call.
        unsafe {
            let count = sys::usrsctp_getpaddrs(self.0, 0, &mut list);
            if count > 0 && !list.is_null() {
                sys::usrsctp_freepaddrs(list);
            }
            count > 0
        }
    }

    fn close_when_association_ends(self) {
        // SAFETY: the socket is open and owned by `self`.
        unsafe { sys::usrsctp_shutdown(self.0, libc::SHUT_RDWR) };
        let deadline = std::time::Instant::now() + ASSOCIATION_END_WAIT;
        while self.has_association() && std::time::Instant::now() < deadline {
            std::thread::sleep(ASSOCIATION_END_POLL);
        }
        // SAFETY: nothing else holds the socket.
        unsafe { sys::usrsctp_close(self.0) };
    }
}

// SAFETY: usrsctp sockets are protected by the stack's own per-socket locks
// and may be used from any thread.
unsafe impl Send for Socket {}
// SAFETY: as above. All access goes through usrsctp calls that lock.
unsafe impl Sync for Socket {}

impl Socket {
    fn open(family: Family) -> Result<Self> {
        // SAFETY: no callbacks or ULP info are passed, so the socket is used
        // through the polling API only.
        let raw = unsafe {
            sys::usrsctp_socket(
                family.domain(),
                libc::SOCK_STREAM,
                sys::IPPROTO_SCTP,
                std::ptr::null(),
                std::ptr::null(),
                0,
                std::ptr::null_mut(),
            )
        };
        if raw.is_null() {
            return Err(io::Error::last_os_error().into());
        }
        Self::adopt(raw)
    }

    fn adopt(raw: *mut sys::socket) -> Result<Self> {
        let readiness = Arc::new(Readiness::default());
        REGISTRY
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(raw as usize, Arc::downgrade(&readiness));
        let socket = Self {
            raw,
            readiness,
            close_after_association: false,
        };
        // SAFETY: `raw` is an open socket owned by `socket`.
        cvt(unsafe { sys::usrsctp_set_non_blocking(socket.raw, 1) })?;
        // SAFETY: the upcall only reads the registry, so no argument is needed.
        cvt(unsafe { sys::usrsctp_set_upcall(socket.raw, Some(upcall), std::ptr::null_mut()) })?;
        Ok(socket)
    }

    fn setsockopt<T>(&self, name: libc::c_int, value: &T) -> io::Result<()> {
        // SAFETY: `value` is a `repr(C)` option struct or integer valid for
        // `size_of::<T>()` bytes, copied by usrsctp before returning.
        cvt(unsafe {
            sys::usrsctp_setsockopt(
                self.raw,
                sys::IPPROTO_SCTP,
                name,
                (value as *const T).cast(),
                size_of::<T>() as libc::socklen_t,
            )
        })?;
        Ok(())
    }

    fn socket_error(&self) -> io::Result<()> {
        let mut err: libc::c_int = 0;
        let mut len = size_of::<libc::c_int>() as libc::socklen_t;
        // SAFETY: `err` is a writable c_int and `len` holds its size.
        cvt(unsafe {
            sys::usrsctp_getsockopt(
                self.raw,
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

    fn addrs(&self, peer: bool) -> Result<Vec<SocketAddr>> {
        let mut list: *mut libc::sockaddr = std::ptr::null_mut();
        // SAFETY: usrsctp allocates the list and reports its entry count.
        let count = unsafe {
            if peer {
                sys::usrsctp_getpaddrs(self.raw, 0, &mut list)
            } else {
                sys::usrsctp_getladdrs(self.raw, 0, &mut list)
            }
        };
        if count < 0 {
            return Err(io::Error::last_os_error().into());
        }
        if count == 0 || list.is_null() {
            return Ok(Vec::new());
        }
        let max_len = count as usize * size_of::<libc::sockaddr_in6>();
        // SAFETY: the list holds `count` packed entries, none longer than a
        // `sockaddr_in6`. `unpack` stops at the first entry it cannot decode,
        // and the list is freed with the matching usrsctp call.
        let addrs = unsafe {
            let bytes = std::slice::from_raw_parts(list.cast::<u8>(), max_len);
            let addrs = addr::unpack(bytes, count as usize);
            if peer {
                sys::usrsctp_freepaddrs(list);
            } else {
                sys::usrsctp_freeladdrs(list);
            }
            addrs
        };
        Ok(addrs)
    }
}

impl Drop for Socket {
    fn drop(&mut self) {
        REGISTRY
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(&(self.raw as usize));
        // SAFETY: `raw` is open and owned by this value. Clearing the upcall
        // first stops new callbacks.
        unsafe { sys::usrsctp_set_upcall(self.raw, None, std::ptr::null_mut()) };
        let closing = ClosingSocket(self.raw);
        if self.close_after_association {
            // usrsctp input reads a closed socket while its association lives
            // (sctp_input.c SOCKET_GONE check).
            std::thread::spawn(move || closing.close_when_association_ends());
        } else {
            // SAFETY: a listening socket has no association of its own.
            unsafe { sys::usrsctp_close(closing.0) };
        }
    }
}

fn cvt(ret: libc::c_int) -> io::Result<libc::c_int> {
    if ret < 0 {
        Err(io::Error::last_os_error())
    } else {
        Ok(ret)
    }
}

fn would_block(err: &io::Error) -> bool {
    matches!(
        err.raw_os_error(),
        Some(code) if code == libc::EWOULDBLOCK || code == libc::EAGAIN
    )
}

fn apply_endpoint_options(socket: &Socket, config: &SctpConfig) -> io::Result<()> {
    let initmsg = sys::sctp_initmsg {
        sinit_num_ostreams: config.outbound_streams,
        sinit_max_instreams: config.max_inbound_streams,
        sinit_max_attempts: 0,
        sinit_max_init_timeo: 0,
    };
    socket.setsockopt(sys::SCTP_INITMSG, &initmsg)?;

    if config.has_rto_settings() {
        let rto = sys::sctp_rtoinfo {
            srto_assoc_id: sys::SCTP_FUTURE_ASSOC,
            srto_initial: config.rto_initial_ms.unwrap_or(0),
            srto_max: config.rto_max_ms.unwrap_or(0),
            srto_min: config.rto_min_ms.unwrap_or(0),
        };
        socket.setsockopt(sys::SCTP_RTOINFO, &rto)?;
    }

    if config.has_path_settings() {
        // SAFETY: all-zero is a valid `sctp_paddrparams` (AF_UNSPEC address,
        // no flags), which applies to every peer address.
        let mut params: sys::sctp_paddrparams = unsafe { zeroed() };
        params.spp_assoc_id = sys::SCTP_FUTURE_ASSOC;
        if let Some(interval_ms) = config.heartbeat_interval_ms {
            params.spp_hbinterval = interval_ms;
            params.spp_flags |= sys::SPP_HB_ENABLE;
        }
        if let Some(max_rxt) = config.path_max_retransmissions {
            params.spp_pathmaxrxt = max_rxt;
        }
        socket.setsockopt(sys::SCTP_PEER_ADDR_PARAMS, &params)?;
    }

    // The remote encapsulation port is an endpoint default. Accepted
    // associations take the port from the peer's INIT.
    if let Some(encap) = config.udp_encapsulation {
        // SAFETY: all-zero is a valid `sctp_udpencaps` (AF_UNSPEC address),
        // which sets the port for every peer address.
        let mut encaps: sys::sctp_udpencaps = unsafe { zeroed() };
        encaps.sue_port = encap.remote_port.to_be();
        socket.setsockopt(sys::SCTP_REMOTE_UDP_ENCAPS_PORT, &encaps)?;
    }
    apply_association_options(socket)
}

fn apply_association_options(socket: &Socket) -> io::Result<()> {
    let on: libc::c_int = 1;
    socket.setsockopt(sys::SCTP_NODELAY, &on)?;
    socket.setsockopt(sys::SCTP_RECVRCVINFO, &on)?;
    for se_type in [
        sys::SCTP_ASSOC_CHANGE,
        sys::SCTP_PEER_ADDR_CHANGE,
        sys::SCTP_SHUTDOWN_EVENT,
    ] {
        let event = sys::sctp_event {
            se_assoc_id: sys::SCTP_ALL_ASSOC,
            se_type,
            se_on: 1,
        };
        socket.setsockopt(sys::SCTP_EVENT, &event)?;
    }
    Ok(())
}

fn bindx(socket: &Socket, addrs: &[SocketAddr]) -> io::Result<()> {
    let mut packed = addr::pack(addrs);
    // SAFETY: `packed` holds `addrs.len()` native sockaddr entries.
    cvt(unsafe {
        sys::usrsctp_bindx(
            socket.raw,
            packed.as_mut_ptr().cast(),
            addrs.len() as libc::c_int,
            sys::SCTP_BINDX_ADD_ADDR,
        )
    })?;
    Ok(())
}

pub(crate) struct Listener {
    socket: Socket,
    config: SctpConfig,
}

impl Listener {
    pub(crate) fn bind(local_addrs: &[SocketAddr], config: &SctpConfig) -> Result<Self> {
        ensure_stack(config)?;
        let socket = Socket::open(Family::for_addrs(local_addrs)?)?;
        apply_endpoint_options(&socket, config)?;
        bindx(&socket, local_addrs)?;
        // SAFETY: listen on a socket this listener owns.
        cvt(unsafe { sys::usrsctp_listen(socket.raw, LISTEN_BACKLOG) })?;
        Ok(Self {
            socket,
            config: config.clone(),
        })
    }

    pub(crate) async fn accept(&self) -> Result<(Association, SocketAddr)> {
        loop {
            let readable = self.socket.readiness.read.notified();
            // SAFETY: accept on the listening socket. The peer address is read
            // afterwards with usrsctp_getpaddrs, so none is requested here.
            let raw = unsafe {
                sys::usrsctp_accept(self.socket.raw, std::ptr::null_mut(), std::ptr::null_mut())
            };
            if raw.is_null() {
                let err = io::Error::last_os_error();
                if !would_block(&err) {
                    return Err(err.into());
                }
                readable.await;
                continue;
            }
            let socket = Socket::adopt(raw)?;
            apply_association_options(&socket)?;
            let peer = socket.addrs(true)?.into_iter().next().ok_or_else(|| {
                io::Error::new(
                    io::ErrorKind::NotConnected,
                    "accepted without a peer address",
                )
            })?;
            return Ok((Association::new(socket, &self.config), peer));
        }
    }

    pub(crate) fn local_addrs(&self) -> Result<Vec<SocketAddr>> {
        self.socket.addrs(false)
    }
}

pub(crate) struct Association {
    socket: Socket,
    recv: Mutex<RecvState>,
    outbound_streams: AtomicU16,
}

impl Association {
    fn new(mut socket: Socket, config: &SctpConfig) -> Self {
        socket.close_after_association = true;
        Self {
            socket,
            recv: Mutex::new(RecvState::new()),
            outbound_streams: AtomicU16::new(config.outbound_streams),
        }
    }

    pub(crate) async fn connect(
        remote_addrs: &[SocketAddr],
        local_addrs: &[SocketAddr],
        config: &SctpConfig,
    ) -> Result<Self> {
        ensure_stack(config)?;
        let mut all = remote_addrs.to_vec();
        all.extend_from_slice(local_addrs);
        let socket = Socket::open(Family::for_addrs(&all)?)?;
        apply_endpoint_options(&socket, config)?;
        if !local_addrs.is_empty() {
            bindx(&socket, local_addrs)?;
        }
        let packed = addr::pack(remote_addrs);
        // SAFETY: `packed` holds `remote_addrs.len()` native sockaddr entries.
        let ret = unsafe {
            sys::usrsctp_connectx(
                socket.raw,
                packed.as_ptr().cast(),
                remote_addrs.len() as libc::c_int,
                std::ptr::null_mut(),
            )
        };
        if ret < 0 {
            let err = io::Error::last_os_error();
            if err.raw_os_error() != Some(libc::EINPROGRESS) {
                return Err(err.into());
            }
        }
        let assoc = Self::new(socket, config);
        assoc.wait_established().await?;
        Ok(assoc)
    }

    async fn wait_established(&self) -> Result<()> {
        let mut state = self.recv.lock().await;
        let mut buf = vec![0u8; RECV_BUF_LEN];
        while !state.is_established() {
            if let Some(message) = self.recv_chunk(&mut state, &mut buf).await? {
                state.stash(message);
            }
        }
        self.socket.socket_error()?;
        Ok(())
    }

    pub(crate) async fn send(&self, data: &[u8], info: SendInfo) -> Result<()> {
        let mut sndinfo = sys::sctp_sndinfo {
            snd_sid: info.stream_id,
            snd_flags: match info.delivery {
                Delivery::Ordered => 0,
                Delivery::Unordered => sys::SCTP_UNORDERED,
            },
            snd_ppid: info.ppid.to_be(),
            snd_context: 0,
            snd_assoc_id: 0,
        };
        loop {
            let writable = self.socket.readiness.write.notified();
            // SAFETY: `data` and `sndinfo` are valid for the call, and the
            // info length matches SCTP_SENDV_SNDINFO.
            let sent = unsafe {
                sys::usrsctp_sendv(
                    self.socket.raw,
                    data.as_ptr().cast(),
                    data.len(),
                    std::ptr::null_mut(),
                    0,
                    std::ptr::addr_of_mut!(sndinfo).cast(),
                    size_of::<sys::sctp_sndinfo>() as libc::socklen_t,
                    sys::SCTP_SENDV_SNDINFO,
                    0,
                )
            };
            if sent >= 0 {
                return Ok(());
            }
            let err = io::Error::last_os_error();
            if would_block(&err) {
                writable.await;
                continue;
            }
            if matches!(
                err.raw_os_error(),
                Some(libc::EPIPE) | Some(libc::ENOTCONN) | Some(libc::ECONNRESET)
            ) {
                return Err(Error::Closed(CloseReason::Lost));
            }
            return Err(err.into());
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
            let readable = self.socket.readiness.read.notified();
            let mut rcvinfo = sys::sctp_rcvinfo::default();
            let mut infolen = size_of::<sys::sctp_rcvinfo>() as libc::socklen_t;
            let mut infotype: libc::c_uint = sys::SCTP_RECVV_NOINFO;
            let mut flags: libc::c_int = 0;
            // SAFETY: `buf`, `rcvinfo`, `infolen`, `infotype` and `flags` are
            // writable for the sizes passed. No source address is requested.
            let len = unsafe {
                sys::usrsctp_recvv(
                    self.socket.raw,
                    buf.as_mut_ptr().cast(),
                    buf.len(),
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    std::ptr::addr_of_mut!(rcvinfo).cast(),
                    &mut infolen,
                    &mut infotype,
                    &mut flags,
                )
            };
            if len < 0 {
                let err = io::Error::last_os_error();
                if would_block(&err) {
                    readable.await;
                    continue;
                }
                if matches!(
                    err.raw_os_error(),
                    Some(libc::ENOTCONN) | Some(libc::ECONNRESET)
                ) {
                    return Err(state.mark_closed(CloseReason::Lost));
                }
                return Err(err.into());
            }
            if len == 0 {
                return Err(state.mark_closed(CloseReason::Shutdown));
            }
            let has_rcvinfo = infotype == sys::SCTP_RECVV_RCVINFO;
            let chunk = Chunk {
                data: &buf[..len as usize],
                is_notification: flags & sys::MSG_NOTIFICATION != 0,
                end_of_record: flags & sys::MSG_EOR != 0,
                stream_id: if has_rcvinfo { rcvinfo.rcv_sid } else { 0 },
                ppid: if has_rcvinfo {
                    u32::from_be(rcvinfo.rcv_ppid)
                } else {
                    0
                },
                delivery: if has_rcvinfo && rcvinfo.rcv_flags & sys::SCTP_UNORDERED != 0 {
                    Delivery::Unordered
                } else {
                    Delivery::Ordered
                },
            };
            return state.absorb(chunk, &CODES, &self.outbound_streams);
        }
    }

    pub(crate) fn shutdown(&self) -> Result<()> {
        // SAFETY: shutdown on a socket this association owns.
        cvt(unsafe { sys::usrsctp_shutdown(self.socket.raw, libc::SHUT_WR) })?;
        Ok(())
    }

    pub(crate) fn peer_addrs(&self) -> Result<Vec<SocketAddr>> {
        self.socket.addrs(true)
    }

    pub(crate) fn local_addrs(&self) -> Result<Vec<SocketAddr>> {
        self.socket.addrs(false)
    }

    pub(crate) fn outbound_streams(&self) -> u16 {
        // Relaxed: see `RecvState::absorb`.
        self.outbound_streams.load(Ordering::Relaxed)
    }
}
