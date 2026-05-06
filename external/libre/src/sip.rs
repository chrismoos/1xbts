use std::ffi::{CStr, CString};
use std::fmt;
use std::net::SocketAddr;
use std::os::raw::{c_char, c_int, c_void};
use std::ptr::NonNull;
use std::sync::Arc;

use crate::error::{Error, Result, native_status};
use crate::runtime::ThreadGuard;

const EINVAL: c_int = libc::EINVAL;
const ENOMEM: c_int = libc::ENOMEM;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Transport {
    Udp,
    Tcp,
    Tls,
}

impl Transport {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Udp => "udp",
            Self::Tcp => "tcp",
            Self::Tls => "tls",
        }
    }

    fn to_raw(self) -> libre_sys::sip_transp {
        match self {
            Self::Udp => libre_sys::sip_transp::SIP_TRANSP_UDP,
            Self::Tcp => libre_sys::sip_transp::SIP_TRANSP_TCP,
            Self::Tls => libre_sys::sip_transp::SIP_TRANSP_TLS,
        }
    }

    fn from_raw(raw: libre_sys::sip_transp) -> Option<Self> {
        match raw {
            libre_sys::sip_transp::SIP_TRANSP_UDP => Some(Self::Udp),
            libre_sys::sip_transp::SIP_TRANSP_TCP => Some(Self::Tcp),
            libre_sys::sip_transp::SIP_TRANSP_TLS => Some(Self::Tls),
            _ => None,
        }
    }
}

impl TryFrom<&str> for Transport {
    type Error = String;

    fn try_from(value: &str) -> std::result::Result<Self, Self::Error> {
        match value.trim().to_ascii_lowercase().as_str() {
            "udp" => Ok(Self::Udp),
            "tcp" => Ok(Self::Tcp),
            "tls" => Ok(Self::Tls),
            other => Err(format!("unsupported SIP transport {other:?}")),
        }
    }
}

pub struct SocketAddress {
    raw: libre_sys::sa,
}

impl SocketAddress {
    pub fn from_socket_addr(addr: SocketAddr) -> Result<Self> {
        if !libre_sys::LIBRE_AVAILABLE {
            return Err(Error::NativeUnavailable);
        }

        let ip = CString::new(addr.ip().to_string())?;
        // SAFETY: sa is a plain C value initialized by sa_set_str below.
        let mut raw = unsafe { std::mem::zeroed::<libre_sys::sa>() };

        // SAFETY: raw is valid for writes and ip is a NUL-terminated C string.
        let status = unsafe { libre_sys::sa_set_str(&mut raw, ip.as_ptr(), addr.port()) };
        native_status("sa_set_str", status)?;

        Ok(Self { raw })
    }

    pub(crate) fn as_ptr(&self) -> *const libre_sys::sa {
        &self.raw
    }
}

pub struct SipStack {
    ptr: NonNull<libre_sys::sip>,
    _trace_ctx: Option<Box<libre_sys::cdma_libre_sip_ctx>>,
    _trace_state: Option<Box<SipTraceState>>,
}

// SAFETY: SipStack serializes all libre API access through ThreadGuard,
// ensuring only one thread calls into the libre C library at a time. The
// raw pointer is reference-counted on the C side and only freed via Drop.
unsafe impl Send for SipStack {}
unsafe impl Sync for SipStack {}

impl fmt::Debug for SipStack {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SipStack")
            .field("ptr", &self.ptr)
            .finish_non_exhaustive()
    }
}

impl SipStack {
    pub fn new(user_agent: &str) -> Result<Self> {
        Self::new_with_trace(user_agent, None)
    }

    pub fn new_with_trace(
        user_agent: &str,
        trace_handler: Option<Arc<dyn SipTraceHandler>>,
    ) -> Result<Self> {
        if !libre_sys::LIBRE_AVAILABLE {
            return Err(Error::NativeUnavailable);
        }

        let user_agent = CString::new(user_agent)?;
        let mut sip = std::ptr::null_mut();
        let mut trace_state = trace_handler.map(|handler| Box::new(SipTraceState { handler }));
        let mut trace_ctx = trace_state.as_mut().map(|state| {
            Box::new(libre_sys::cdma_libre_sip_ctx {
                trace: Some(sip_trace_handler),
                arg: state.as_mut() as *mut SipTraceState as *mut c_void,
            })
        });
        let trace_ctx_ptr = trace_ctx.as_mut().map_or(std::ptr::null_mut(), |ctx| {
            ctx.as_mut() as *mut libre_sys::cdma_libre_sip_ctx
        });

        // SAFETY: sip is an out pointer and user_agent is a valid C string.
        let status = unsafe {
            libre_sys::cdma_libre_sip_alloc(&mut sip, user_agent.as_ptr(), trace_ctx_ptr)
        };
        native_status("cdma_libre_sip_alloc", status)?;

        let ptr = NonNull::new(sip).ok_or(Error::Native {
            operation: "cdma_libre_sip_alloc",
            status: -1,
        })?;

        Ok(Self {
            ptr,
            _trace_ctx: trace_ctx,
            _trace_state: trace_state,
        })
    }

    pub fn add_transport(&self, transport: Transport, listen_addr: &SocketAddress) -> Result<()> {
        let _guard = ThreadGuard::enter();
        // SAFETY: self.ptr and listen_addr are valid libre objects.
        let status = unsafe {
            libre_sys::cdma_libre_sip_transp_add(
                self.ptr.as_ptr(),
                transport.to_raw(),
                listen_addr.as_ptr(),
            )
        };
        native_status("cdma_libre_sip_transp_add", status)
    }

    fn as_ptr(&self) -> *mut libre_sys::sip {
        self.ptr.as_ptr()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SipTraceEvent {
    pub tx: bool,
    pub transport: Transport,
    pub src: Option<String>,
    pub dst: Option<String>,
    pub packet: Vec<u8>,
}

pub trait SipTraceHandler: Send + Sync + 'static {
    fn on_trace(&self, event: SipTraceEvent);
}

struct SipTraceState {
    handler: Arc<dyn SipTraceHandler>,
}

unsafe extern "C" fn sip_trace_handler(
    arg: *mut c_void,
    tx: u8,
    tp: libre_sys::sip_transp,
    src: *const libre_sys::sa,
    dst: *const libre_sys::sa,
    pkt: *const u8,
    len: usize,
) {
    if arg.is_null() || pkt.is_null() {
        return;
    }

    let Some(transport) = Transport::from_raw(tp) else {
        return;
    };

    // SAFETY: arg is the SipTraceState retained by SipStack.
    let state = unsafe { &*(arg as *const SipTraceState) };
    // SAFETY: libre guarantees pkt points to len bytes for this callback.
    let packet = unsafe { std::slice::from_raw_parts(pkt, len).to_vec() };

    state.handler.on_trace(SipTraceEvent {
        tx: tx != 0,
        transport,
        src: format_sa(src),
        dst: format_sa(dst),
        packet,
    });
}

fn format_sa(addr: *const libre_sys::sa) -> Option<String> {
    if addr.is_null() {
        return None;
    }

    let mut host = [0 as c_char; 128];
    // SAFETY: addr is provided by libre and host is a writable buffer.
    let status = unsafe { libre_sys::sa_ntop(addr, host.as_mut_ptr(), host.len() as c_int) };
    if status != 0 {
        return None;
    }

    // SAFETY: sa_ntop writes a NUL-terminated string on success.
    let host = unsafe { CStr::from_ptr(host.as_ptr()) }
        .to_string_lossy()
        .into_owned();
    // SAFETY: addr is provided by libre for this callback.
    let port = unsafe { libre_sys::sa_port(addr) };

    if host.contains(':') {
        Some(format!("[{host}]:{port}"))
    } else {
        Some(format!("{host}:{port}"))
    }
}

impl Drop for SipStack {
    fn drop(&mut self) {
        let _guard = ThreadGuard::enter();
        // SAFETY: ptr is owned by this wrapper and can be force-closed on drop.
        unsafe {
            libre_sys::cdma_libre_sip_close(self.ptr.as_ptr(), 1);
            libre_sys::cdma_libre_sip_deref(self.ptr.as_ptr());
        }
    }
}

#[derive(Debug)]
pub struct SipSessionSocket {
    ptr: NonNull<libre_sys::sipsess_sock>,
}

// SAFETY: SipSessionSocket is only accessed through ThreadGuard and is dropped
// before the SIP stack/event loop are torn down by the gateway backend.
unsafe impl Send for SipSessionSocket {}
unsafe impl Sync for SipSessionSocket {}

impl SipSessionSocket {
    pub fn listen(stack: &SipStack) -> Result<Self> {
        if !libre_sys::LIBRE_AVAILABLE {
            return Err(Error::NativeUnavailable);
        }

        let _guard = ThreadGuard::enter();
        let mut sock = std::ptr::null_mut();

        // SAFETY: sock is an out pointer and stack is a valid SIP stack.
        let status = unsafe { libre_sys::cdma_libre_sipsess_listen(&mut sock, stack.as_ptr()) };
        native_status("cdma_libre_sipsess_listen", status)?;

        let ptr = NonNull::new(sock).ok_or(Error::Native {
            operation: "cdma_libre_sipsess_listen",
            status: -1,
        })?;

        Ok(Self { ptr })
    }

    fn as_ptr(&self) -> *mut libre_sys::sipsess_sock {
        self.ptr.as_ptr()
    }
}

impl Drop for SipSessionSocket {
    fn drop(&mut self) {
        let _guard = ThreadGuard::enter();
        // SAFETY: ptr is owned by this wrapper.
        unsafe {
            libre_sys::cdma_libre_sipsess_close_all(self.ptr.as_ptr());
            libre_sys::cdma_libre_sipsess_sock_deref(self.ptr.as_ptr());
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct OutboundSipSessionConfig {
    pub session_id: String,
    pub to_uri: String,
    pub from_name: Option<String>,
    pub from_uri: String,
    pub contact_user: String,
    pub call_id: Option<String>,
    pub sdp_offer: String,
    pub auth: Option<SipCredentials>,
}

#[derive(Clone, Eq, PartialEq)]
pub struct SipCredentials {
    pub username: String,
    pub password: String,
}

impl fmt::Debug for SipCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("SipCredentials")
            .field("username", &self.username)
            .field("password", &"<redacted>")
            .finish()
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SipRegistrationConfig {
    pub registrar_uri: String,
    pub to_uri: String,
    pub from_name: Option<String>,
    pub from_uri: String,
    pub contact_user: String,
    pub expires_secs: u32,
    pub keepalive_interval_secs: u32,
    pub auth: Option<SipCredentials>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum SipRegistrationEvent {
    Response {
        error: i32,
        sip_status: u16,
        reason: Option<String>,
    },
}

pub trait SipRegistrationHandler: Send + Sync + 'static {
    fn on_event(&self, event: SipRegistrationEvent);

    fn on_auth_challenge(&self, _realm: Option<&str>) {}
}

struct SipRegistrationState {
    auth: Option<SipCredentials>,
    handler: Arc<dyn SipRegistrationHandler>,
}

#[derive(Debug)]
struct SipRegistrationStrings {
    registrar_uri: CString,
    to_uri: CString,
    from_name: Option<CString>,
    from_uri: CString,
    contact_user: CString,
}

impl SipRegistrationStrings {
    fn new(config: &SipRegistrationConfig) -> Result<Self> {
        Ok(Self {
            registrar_uri: CString::new(config.registrar_uri.clone())?,
            to_uri: CString::new(config.to_uri.clone())?,
            from_name: config
                .from_name
                .as_ref()
                .map(|value| CString::new(value.as_str()))
                .transpose()?,
            from_uri: CString::new(config.from_uri.clone())?,
            contact_user: CString::new(config.contact_user.clone())?,
        })
    }
}

pub struct SipRegistration {
    ptr: NonNull<libre_sys::sipreg>,
    _ctx: Box<libre_sys::cdma_libre_sipreg_ctx>,
    _state: Box<SipRegistrationState>,
    _strings: SipRegistrationStrings,
}

// SAFETY: SipRegistration is owned by the gateway backend. All C operations go
// through ThreadGuard. Callback state is retained for the lifetime of the
// native registration object.
unsafe impl Send for SipRegistration {}
unsafe impl Sync for SipRegistration {}

impl SipRegistration {
    pub fn register(
        stack: &SipStack,
        config: SipRegistrationConfig,
        handler: Arc<dyn SipRegistrationHandler>,
    ) -> Result<Self> {
        if !libre_sys::LIBRE_AVAILABLE {
            return Err(Error::NativeUnavailable);
        }

        let strings = SipRegistrationStrings::new(&config)?;
        let mut state = Box::new(SipRegistrationState {
            auth: config.auth,
            handler,
        });
        let mut ctx = Box::new(libre_sys::cdma_libre_sipreg_ctx {
            handlers: &SIP_REGISTRATION_HANDLERS,
            arg: state.as_mut() as *mut SipRegistrationState as *mut c_void,
            sip: std::ptr::null_mut(),
            keepalive: std::ptr::null_mut(),
            keepalive_interval_secs: config.keepalive_interval_secs,
        });
        let registration = libre_sys::cdma_libre_registration {
            registrar_uri: strings.registrar_uri.as_ptr(),
            to_uri: strings.to_uri.as_ptr(),
            from_name: strings
                .from_name
                .as_ref()
                .map_or(std::ptr::null(), |value| value.as_ptr()),
            from_uri: strings.from_uri.as_ptr(),
            contact_user: strings.contact_user.as_ptr(),
            expires: config.expires_secs,
            keepalive_interval_secs: config.keepalive_interval_secs,
            auth_enabled: u8::from(state.auth.is_some()),
            ctx: ctx.as_mut() as *mut libre_sys::cdma_libre_sipreg_ctx,
        };

        let _guard = ThreadGuard::enter();
        let mut reg = std::ptr::null_mut();

        // SAFETY: registration points to valid C strings and callback context
        // retained by the returned SipRegistration.
        let status =
            unsafe { libre_sys::cdma_libre_sipreg_alloc(&mut reg, stack.as_ptr(), &registration) };
        native_status("cdma_libre_sipreg_alloc", status)?;

        let ptr = NonNull::new(reg).ok_or(Error::Native {
            operation: "cdma_libre_sipreg_alloc",
            status: -1,
        })?;

        // SAFETY: ptr is a valid sipreg returned by libre.
        let status = unsafe { libre_sys::cdma_libre_sipreg_send(ptr.as_ptr()) };
        if status != 0 {
            // SAFETY: ptr is owned here because construction has not succeeded.
            unsafe { libre_sys::cdma_libre_sipreg_deref(ptr.as_ptr()) };
        }
        native_status("cdma_libre_sipreg_send", status)?;

        Ok(Self {
            ptr,
            _ctx: ctx,
            _state: state,
            _strings: strings,
        })
    }
}

impl Drop for SipRegistration {
    fn drop(&mut self) {
        let _guard = ThreadGuard::enter();
        // SAFETY: ptr is owned by this wrapper. libre's sipreg destructor sends
        // an unregister request when the registration is active.
        unsafe { libre_sys::cdma_libre_sipreg_keepalive_stop(self._ctx.as_mut()) };
        unsafe { libre_sys::cdma_libre_sipreg_deref(self.ptr.as_ptr()) };
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum OutboundSipSessionEvent {
    Progress {
        session_id: String,
        sip_status: u16,
        sdp: Vec<u8>,
    },
    Answer {
        session_id: String,
        sip_status: u16,
        sdp: Vec<u8>,
    },
    Established {
        session_id: String,
        sip_status: u16,
    },
    Closed {
        session_id: String,
        error: i32,
        sip_status: u16,
    },
}

pub trait OutboundSipSessionHandler: Send + Sync + 'static {
    fn on_event(&self, event: OutboundSipSessionEvent);

    fn on_auth_challenge(&self, _realm: Option<&str>) {}
}

struct OutboundSipSessionState {
    session_id: String,
    sdp_offer: CString,
    auth: Option<SipCredentials>,
    handler: Arc<dyn OutboundSipSessionHandler>,
}

#[derive(Debug)]
struct OutboundSipSessionStrings {
    to_uri: CString,
    from_name: Option<CString>,
    from_uri: CString,
    contact_user: CString,
    call_id: Option<CString>,
}

impl OutboundSipSessionStrings {
    fn new(config: &OutboundSipSessionConfig) -> Result<Self> {
        Ok(Self {
            to_uri: CString::new(config.to_uri.clone())?,
            from_name: config
                .from_name
                .as_ref()
                .map(|value| CString::new(value.as_str()))
                .transpose()?,
            from_uri: CString::new(config.from_uri.clone())?,
            contact_user: CString::new(config.contact_user.clone())?,
            call_id: config
                .call_id
                .as_ref()
                .map(|value| CString::new(value.as_str()))
                .transpose()?,
        })
    }
}

pub struct OutboundSipSession {
    ptr: NonNull<libre_sys::sipsess>,
    _ctx: Box<libre_sys::cdma_libre_sipsess_ctx>,
    _state: Box<OutboundSipSessionState>,
    _strings: OutboundSipSessionStrings,
}

// SAFETY: OutboundSipSession is stored behind a Mutex in the gateway backend.
// All C operations go through ThreadGuard and callbacks only use the retained
// state pointer, which lives as long as the session.
unsafe impl Send for OutboundSipSession {}
unsafe impl Sync for OutboundSipSession {}

impl OutboundSipSession {
    pub fn connect(
        socket: &SipSessionSocket,
        config: OutboundSipSessionConfig,
        handler: Arc<dyn OutboundSipSessionHandler>,
    ) -> Result<Self> {
        if !libre_sys::LIBRE_AVAILABLE {
            return Err(Error::NativeUnavailable);
        }

        let strings = OutboundSipSessionStrings::new(&config)?;
        let mut state = Box::new(OutboundSipSessionState {
            session_id: config.session_id,
            sdp_offer: CString::new(config.sdp_offer)?,
            auth: config.auth,
            handler,
        });
        let mut ctx = Box::new(libre_sys::cdma_libre_sipsess_ctx {
            handlers: &OUTBOUND_SESSION_HANDLERS,
            arg: state.as_mut() as *mut OutboundSipSessionState as *mut c_void,
        });

        let call = libre_sys::cdma_libre_outbound_call {
            to_uri: strings.to_uri.as_ptr(),
            from_name: strings
                .from_name
                .as_ref()
                .map_or(std::ptr::null(), |value| value.as_ptr()),
            from_uri: strings.from_uri.as_ptr(),
            contact_user: strings.contact_user.as_ptr(),
            call_id: strings
                .call_id
                .as_ref()
                .map_or(std::ptr::null(), |value| value.as_ptr()),
            auth_enabled: u8::from(state.auth.is_some()),
            ctx: ctx.as_mut() as *mut libre_sys::cdma_libre_sipsess_ctx,
        };

        let _guard = ThreadGuard::enter();
        let mut sess = std::ptr::null_mut();

        // SAFETY: call points to valid C strings and callback context that are
        // retained by the returned OutboundSipSession.
        let status =
            unsafe { libre_sys::cdma_libre_sipsess_connect(&mut sess, socket.as_ptr(), &call) };
        native_status("cdma_libre_sipsess_connect", status)?;

        let ptr = NonNull::new(sess).ok_or(Error::Native {
            operation: "cdma_libre_sipsess_connect",
            status: -1,
        })?;

        Ok(Self {
            ptr,
            _ctx: ctx,
            _state: state,
            _strings: strings,
        })
    }

    pub fn abort(&self) {
        let _guard = ThreadGuard::enter();
        // SAFETY: ptr is a valid sipsess owned by this wrapper.
        unsafe { libre_sys::cdma_libre_sipsess_abort(self.ptr.as_ptr()) };
    }
}

impl Drop for OutboundSipSession {
    fn drop(&mut self) {
        let _guard = ThreadGuard::enter();
        // SAFETY: ptr is owned by this wrapper.
        unsafe { libre_sys::cdma_libre_sipsess_deref(self.ptr.as_ptr()) };
    }
}

unsafe extern "C" fn outbound_desc_handler(
    arg: *mut c_void,
    descp: *mut *mut libre_sys::mbuf,
) -> c_int {
    if arg.is_null() || descp.is_null() {
        return EINVAL;
    }

    // SAFETY: arg is the OutboundSipSessionState retained by OutboundSipSession.
    let state = unsafe { &*(arg as *const OutboundSipSessionState) };

    // SAFETY: sdp_offer is a valid C string. The mbuf ownership transfers to
    // libre, which dereferences it after the INVITE body is built.
    let mbuf = unsafe { libre_sys::cdma_libre_mbuf_from_str(state.sdp_offer.as_ptr()) };
    if mbuf.is_null() {
        return ENOMEM;
    }

    // SAFETY: descp is a valid out pointer from libre.
    unsafe {
        *descp = mbuf;
    }

    0
}

unsafe extern "C" fn outbound_auth_handler(
    arg: *mut c_void,
    realm: *const c_char,
    usernamep: *mut *mut c_char,
    passwordp: *mut *mut c_char,
) -> c_int {
    if arg.is_null() || usernamep.is_null() || passwordp.is_null() {
        return EINVAL;
    }

    // SAFETY: arg is the OutboundSipSessionState retained by OutboundSipSession.
    let state = unsafe { &*(arg as *const OutboundSipSessionState) };
    let Some(auth) = state.auth.as_ref() else {
        return EINVAL;
    };

    let realm = if realm.is_null() {
        None
    } else {
        // SAFETY: realm is a NUL-terminated C string owned by libre for this callback.
        unsafe { CStr::from_ptr(realm) }.to_str().ok()
    };
    state.handler.on_auth_challenge(realm);

    let username = match CString::new(auth.username.as_str()) {
        Ok(value) => value,
        Err(_) => return EINVAL,
    };
    let password = match CString::new(auth.password.as_str()) {
        Ok(value) => value,
        Err(_) => return EINVAL,
    };

    // SAFETY: output pointers are provided by libre; cdma_libre_strdup allocates
    // with libre's allocator so libre can release the returned strings.
    let status = unsafe { libre_sys::cdma_libre_strdup(usernamep, username.as_ptr()) };
    if status != 0 {
        return status;
    }

    // SAFETY: same as above for the password output pointer.
    unsafe { libre_sys::cdma_libre_strdup(passwordp, password.as_ptr()) }
}

unsafe extern "C" fn registration_auth_handler(
    arg: *mut c_void,
    realm: *const c_char,
    usernamep: *mut *mut c_char,
    passwordp: *mut *mut c_char,
) -> c_int {
    if arg.is_null() || usernamep.is_null() || passwordp.is_null() {
        return EINVAL;
    }

    // SAFETY: arg is the SipRegistrationState retained by SipRegistration.
    let state = unsafe { &*(arg as *const SipRegistrationState) };
    let Some(auth) = state.auth.as_ref() else {
        return EINVAL;
    };

    let realm = if realm.is_null() {
        None
    } else {
        // SAFETY: realm is a NUL-terminated C string owned by libre for this callback.
        unsafe { CStr::from_ptr(realm) }.to_str().ok()
    };
    state.handler.on_auth_challenge(realm);

    let username = match CString::new(auth.username.as_str()) {
        Ok(value) => value,
        Err(_) => return EINVAL,
    };
    let password = match CString::new(auth.password.as_str()) {
        Ok(value) => value,
        Err(_) => return EINVAL,
    };

    // SAFETY: output pointers are provided by libre; cdma_libre_strdup allocates
    // with libre's allocator so libre can release the returned strings.
    let status = unsafe { libre_sys::cdma_libre_strdup(usernamep, username.as_ptr()) };
    if status != 0 {
        return status;
    }

    // SAFETY: same as above for the password output pointer.
    unsafe { libre_sys::cdma_libre_strdup(passwordp, password.as_ptr()) }
}

unsafe extern "C" fn registration_response_handler(
    arg: *mut c_void,
    err: c_int,
    scode: u16,
    reason: *const u8,
    reason_len: usize,
) {
    if arg.is_null() {
        return;
    }

    // SAFETY: arg is the SipRegistrationState retained by SipRegistration.
    let state = unsafe { &*(arg as *const SipRegistrationState) };
    let reason = if reason.is_null() || reason_len == 0 {
        None
    } else {
        // SAFETY: reason/reason_len are valid for the duration of this callback.
        Some(
            String::from_utf8_lossy(unsafe { std::slice::from_raw_parts(reason, reason_len) })
                .into_owned(),
        )
    };

    state.handler.on_event(SipRegistrationEvent::Response {
        error: err,
        sip_status: scode,
        reason,
    });
}

unsafe extern "C" fn outbound_answer_handler(
    arg: *mut c_void,
    scode: u16,
    body: *const u8,
    body_len: usize,
) {
    if arg.is_null() {
        return;
    }

    // SAFETY: arg is the OutboundSipSessionState retained by OutboundSipSession.
    let state = unsafe { &*(arg as *const OutboundSipSessionState) };
    let sdp = if body.is_null() || body_len == 0 {
        Vec::new()
    } else {
        // SAFETY: body/body_len are valid for the duration of this callback.
        unsafe { std::slice::from_raw_parts(body, body_len) }.to_vec()
    };

    state.handler.on_event(OutboundSipSessionEvent::Answer {
        session_id: state.session_id.clone(),
        sip_status: scode,
        sdp,
    });
}

unsafe extern "C" fn outbound_progress_handler(
    arg: *mut c_void,
    scode: u16,
    body: *const u8,
    body_len: usize,
) {
    if arg.is_null() {
        return;
    }

    // SAFETY: arg is the OutboundSipSessionState retained by OutboundSipSession.
    let state = unsafe { &*(arg as *const OutboundSipSessionState) };
    let sdp = if body.is_null() || body_len == 0 {
        Vec::new()
    } else {
        // SAFETY: body/body_len are valid for the duration of this callback.
        unsafe { std::slice::from_raw_parts(body, body_len) }.to_vec()
    };

    state.handler.on_event(OutboundSipSessionEvent::Progress {
        session_id: state.session_id.clone(),
        sip_status: scode,
        sdp,
    });
}

unsafe extern "C" fn outbound_established_handler(arg: *mut c_void, scode: u16) {
    if arg.is_null() {
        return;
    }

    // SAFETY: arg is the OutboundSipSessionState retained by OutboundSipSession.
    let state = unsafe { &*(arg as *const OutboundSipSessionState) };
    state
        .handler
        .on_event(OutboundSipSessionEvent::Established {
            session_id: state.session_id.clone(),
            sip_status: scode,
        });
}

unsafe extern "C" fn outbound_close_handler(arg: *mut c_void, err: c_int, scode: u16) {
    if arg.is_null() {
        return;
    }

    // SAFETY: arg is the OutboundSipSessionState retained by OutboundSipSession.
    let state = unsafe { &*(arg as *const OutboundSipSessionState) };
    state.handler.on_event(OutboundSipSessionEvent::Closed {
        session_id: state.session_id.clone(),
        error: err,
        sip_status: scode,
    });
}

static OUTBOUND_SESSION_HANDLERS: libre_sys::cdma_libre_sipsess_handlers =
    libre_sys::cdma_libre_sipsess_handlers {
        desc: Some(outbound_desc_handler),
        auth: Some(outbound_auth_handler),
        answer: Some(outbound_answer_handler),
        progress: Some(outbound_progress_handler),
        established: Some(outbound_established_handler),
        close: Some(outbound_close_handler),
    };

static SIP_REGISTRATION_HANDLERS: libre_sys::cdma_libre_sipreg_handlers =
    libre_sys::cdma_libre_sipreg_handlers {
        auth: Some(registration_auth_handler),
        response: Some(registration_response_handler),
    };

#[cfg(test)]
mod tests {
    use super::{SipCredentials, Transport};

    #[test]
    fn parses_supported_transports() {
        assert_eq!(Transport::try_from("udp"), Ok(Transport::Udp));
        assert_eq!(Transport::try_from("TCP"), Ok(Transport::Tcp));
        assert_eq!(Transport::try_from(" tls "), Ok(Transport::Tls));
    }

    #[test]
    fn rejects_unknown_transport() {
        assert!(Transport::try_from("sctp").is_err());
    }

    #[test]
    fn sip_credentials_debug_redacts_password() {
        let credentials = SipCredentials {
            username: "trunk-user".to_string(),
            password: "secret".to_string(),
        };
        let debug = format!("{credentials:?}");

        assert!(debug.contains("trunk-user"));
        assert!(debug.contains("<redacted>"));
        assert!(!debug.contains("secret"));
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct SipUserAgentConfig {
    pub listen_addr: SocketAddr,
    pub transport: Transport,
    pub user_agent: String,
}

impl SipUserAgentConfig {
    pub fn new(
        listen_addr: SocketAddr,
        transport: Transport,
        user_agent: impl Into<String>,
    ) -> Self {
        Self {
            listen_addr,
            transport,
            user_agent: user_agent.into(),
        }
    }
}
