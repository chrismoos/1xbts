use std::env;
use std::fs;
use std::path::{Path, PathBuf};

fn main() {
    println!("cargo:rustc-check-cfg=cfg(libre_available)");
    println!("cargo:rerun-if-env-changed=LIBRE_INCLUDE_DIR");
    println!("cargo:rerun-if-env-changed=LIBRE_LIB_DIR");
    println!("cargo:rerun-if-env-changed=LIBRE_LIB_NAME");
    println!("cargo:rerun-if-env-changed=LIBRE_SYS_NO_PKG_CONFIG");
    println!("cargo:rerun-if-changed=wrapper.h");
    println!("cargo:rerun-if-changed=cdma_libre_shim.c");
    println!("cargo:rerun-if-changed=cdma_libre_shim.h");

    let out_dir = PathBuf::from(env::var_os("OUT_DIR").expect("OUT_DIR must be set"));
    let bindings_path = out_dir.join("bindgen.rs");

    match discover_libre() {
        Ok(discovery) => {
            println!("cargo:rustc-cfg=libre_available");
            let header_capabilities = detect_header_capabilities(&discovery.include_paths);
            if env::var_os("CARGO_FEATURE_REQUIRED").is_some() {
                require_header_capabilities(&header_capabilities);
            }
            compile_shim(&discovery.include_paths, &header_capabilities);
            generate_bindings(&bindings_path, &discovery.include_paths);
        }
        Err(err) if env::var_os("CARGO_FEATURE_REQUIRED").is_some() => {
            panic!("native libre/re is required but was not found: {err}");
        }
        Err(err) => {
            println!(
                "cargo:warning=building libre-sys stubs because native libre/re was not found: {}",
                err.trim()
            );
            write_stub_bindings(&bindings_path);
        }
    }
}

fn require_header_capabilities(capabilities: &HeaderCapabilities) {
    let mut missing = Vec::new();

    if !capabilities.sipreg {
        missing.push("re_sipreg.h with sipreg_alloc()");
    }

    if !capabilities.sipsess_connect_desc_handler {
        missing.push("re_sipsess.h with sipsess_desc_h connect support");
    }

    if !capabilities.sipsess_abort {
        missing.push("re_sipsess.h with sipsess_abort()");
    }

    if missing.is_empty() {
        return;
    }

    panic!(
        "native libre/re was found, but it lacks APIs required by cdma-voice-gw: {}. \
         Install a newer libre-dev/libre build or set LIBRE_INCLUDE_DIR and LIBRE_LIB_DIR \
         to a libre installation that provides these headers/APIs.",
        missing.join(", ")
    );
}

struct LibreDiscovery {
    include_paths: Vec<PathBuf>,
}

#[derive(Debug, Default)]
struct HeaderCapabilities {
    dnsc_cache_ttl_max: bool,
    dnsc_getaddrinfo: bool,
    sipreg: bool,
    sipsess_connect_desc_handler: bool,
    sipsess_connect_call_id: bool,
    sipsess_estab_handler: bool,
    sipsess_abort: bool,
}

fn discover_libre() -> Result<LibreDiscovery, String> {
    if let Some(include_dir) = env::var_os("LIBRE_INCLUDE_DIR") {
        let include_path = PathBuf::from(include_dir);
        if !include_path.exists() {
            return Err(format!(
                "LIBRE_INCLUDE_DIR does not exist: {}",
                include_path.display()
            ));
        }

        if let Some(lib_dir) = env::var_os("LIBRE_LIB_DIR") {
            println!(
                "cargo:rustc-link-search=native={}",
                PathBuf::from(lib_dir).display()
            );
        }

        let lib_name = env::var("LIBRE_LIB_NAME").unwrap_or_else(|_| "re".to_string());
        println!("cargo:rustc-link-lib={lib_name}");

        return Ok(LibreDiscovery {
            include_paths: vec![include_path],
        });
    }

    if env::var_os("LIBRE_SYS_NO_PKG_CONFIG").is_some() {
        return Err("pkg-config disabled by LIBRE_SYS_NO_PKG_CONFIG".to_string());
    }

    let library = pkg_config::Config::new()
        .probe("libre")
        .map_err(|err| err.to_string())?;

    Ok(LibreDiscovery {
        include_paths: library.include_paths,
    })
}

fn detect_header_capabilities(include_paths: &[PathBuf]) -> HeaderCapabilities {
    let fallback = HeaderCapabilities {
        dnsc_cache_ttl_max: false,
        dnsc_getaddrinfo: false,
        sipreg: true,
        sipsess_connect_desc_handler: true,
        sipsess_connect_call_id: true,
        sipsess_estab_handler: true,
        sipsess_abort: true,
    };

    let Some(sipsess_contents) = read_header(include_paths, "re_sipsess.h") else {
        return fallback;
    };
    let dnsc_contents =
        read_header(include_paths, "re_dnsc.h").or_else(|| read_header(include_paths, "re_dns.h"));
    let sipreg_contents = read_header(include_paths, "re_sipreg.h");
    let sipsess_connect_desc_handler = sipsess_contents.contains("sipsess_desc_h *desch");

    HeaderCapabilities {
        dnsc_cache_ttl_max: dnsc_contents
            .as_deref()
            .is_some_and(|contents| contents.contains("cache_ttl_max")),
        dnsc_getaddrinfo: dnsc_contents
            .as_deref()
            .is_some_and(|contents| contents.contains("getaddrinfo")),
        sipreg: sipreg_contents.is_some_and(|contents| contents.contains("sipreg_alloc(")),
        sipsess_connect_desc_handler,
        sipsess_connect_call_id: sipsess_connect_desc_handler
            && sipsess_contents.contains("callid"),
        sipsess_estab_handler: sipsess_contents.contains("sipsess_estab_h *estabh"),
        sipsess_abort: sipsess_contents.contains("sipsess_abort("),
    }
}

fn read_header(include_paths: &[PathBuf], file_name: &str) -> Option<String> {
    let header = find_header(include_paths, file_name)?;
    fs::read_to_string(header).ok()
}

fn find_header(include_paths: &[PathBuf], file_name: &str) -> Option<PathBuf> {
    for include_path in include_paths {
        let direct = include_path.join(file_name);
        if direct.exists() {
            return Some(direct);
        }

        let nested = include_path.join("re").join(file_name);
        if nested.exists() {
            return Some(nested);
        }
    }

    None
}

fn generate_bindings(out_path: &Path, include_paths: &[PathBuf]) {
    let mut builder = bindgen::builder()
        .header("wrapper.h")
        .allowlist_function("^(cdma_libre_|libre_|re_|sip_|sipsess_|sdp_|sa_|mbuf_|mem_).+")
        .allowlist_type("^(cdma_libre_.+|fd_h|poll_method|re|re_.*|sip|sip_.*|sipreg|sipreg_.*|sipsess|sipsess_.*|sdp.*|sa|mbuf|pl|uri|rel100_mode|sdp_neg_state)$")
        .allowlist_var("^(FD_|METHOD_|SIP_|SDP_|REL100_).+")
        .default_enum_style(bindgen::EnumVariation::Rust {
            non_exhaustive: true,
        });

    for include_path in include_paths {
        builder = builder.clang_arg(format!("-I{}", include_path.display()));
    }

    let bindings = builder
        .generate()
        .expect("failed to generate libre/re bindings");

    bindings
        .write_to_file(out_path)
        .expect("failed to write libre/re bindings");
}

fn compile_shim(include_paths: &[PathBuf], header_capabilities: &HeaderCapabilities) {
    let mut build = cc::Build::new();
    build.file("cdma_libre_shim.c");

    for include_path in include_paths {
        build.include(include_path);
    }

    if header_capabilities.dnsc_cache_ttl_max {
        build.define("CDMA_LIBRE_DNSC_HAS_CACHE_TTL_MAX", None);
    }

    if header_capabilities.dnsc_getaddrinfo {
        build.define("CDMA_LIBRE_DNSC_HAS_GETADDRINFO", None);
    }

    if header_capabilities.sipreg {
        build.define("CDMA_LIBRE_HAS_SIPREG", None);
    }

    if header_capabilities.sipsess_connect_desc_handler {
        build.define("CDMA_LIBRE_SIPSESS_CONNECT_HAS_DESC_HANDLER", None);
    }

    if header_capabilities.sipsess_connect_call_id {
        build.define("CDMA_LIBRE_SIPSESS_CONNECT_HAS_CALL_ID", None);
    }

    if header_capabilities.sipsess_estab_handler {
        build.define("CDMA_LIBRE_HAS_SIPSESS_ESTAB_H", None);
    }

    if header_capabilities.sipsess_abort {
        build.define("CDMA_LIBRE_HAS_SIPSESS_ABORT", None);
    }

    build.compile("cdma_libre_shim");
}

fn write_stub_bindings(out_path: &Path) {
    fs::write(
        out_path,
        r#"
pub type re_signal_h = ::std::option::Option<unsafe extern "C" fn(sig: ::std::os::raw::c_int)>;
pub type cdma_libre_sipsess_desc_h = ::std::option::Option<
    unsafe extern "C" fn(
        arg: *mut ::std::os::raw::c_void,
        descp: *mut *mut mbuf,
    ) -> ::std::os::raw::c_int,
>;
pub type cdma_libre_sip_auth_h = ::std::option::Option<
    unsafe extern "C" fn(
        arg: *mut ::std::os::raw::c_void,
        realm: *const ::std::os::raw::c_char,
        username: *mut *mut ::std::os::raw::c_char,
        password: *mut *mut ::std::os::raw::c_char,
    ) -> ::std::os::raw::c_int,
>;
pub type cdma_libre_sip_trace_h = ::std::option::Option<
    unsafe extern "C" fn(
        arg: *mut ::std::os::raw::c_void,
        tx: u8,
        tp: sip_transp,
        src: *const sa,
        dst: *const sa,
        pkt: *const u8,
        len: usize,
    ),
>;
pub type cdma_libre_sipreg_resp_h = ::std::option::Option<
    unsafe extern "C" fn(
        arg: *mut ::std::os::raw::c_void,
        err: ::std::os::raw::c_int,
        scode: u16,
        reason: *const u8,
        reason_len: usize,
    ),
>;
pub type cdma_libre_sipsess_answer_h = ::std::option::Option<
    unsafe extern "C" fn(
        arg: *mut ::std::os::raw::c_void,
        scode: u16,
        body: *const u8,
        body_len: usize,
    ),
>;
pub type cdma_libre_sipsess_progress_h = ::std::option::Option<
    unsafe extern "C" fn(
        arg: *mut ::std::os::raw::c_void,
        scode: u16,
        body: *const u8,
        body_len: usize,
    ),
>;
pub type cdma_libre_sipsess_established_h = ::std::option::Option<
    unsafe extern "C" fn(arg: *mut ::std::os::raw::c_void, scode: u16),
>;
pub type cdma_libre_sipsess_close_h = ::std::option::Option<
    unsafe extern "C" fn(arg: *mut ::std::os::raw::c_void, err: ::std::os::raw::c_int, scode: u16),
>;

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct cdma_libre_sipsess_handlers {
    pub desc: cdma_libre_sipsess_desc_h,
    pub auth: cdma_libre_sip_auth_h,
    pub answer: cdma_libre_sipsess_answer_h,
    pub progress: cdma_libre_sipsess_progress_h,
    pub established: cdma_libre_sipsess_established_h,
    pub close: cdma_libre_sipsess_close_h,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct cdma_libre_sipsess_ctx {
    pub handlers: *const cdma_libre_sipsess_handlers,
    pub arg: *mut ::std::os::raw::c_void,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct cdma_libre_sipreg_handlers {
    pub auth: cdma_libre_sip_auth_h,
    pub response: cdma_libre_sipreg_resp_h,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct cdma_libre_sipreg_ctx {
    pub handlers: *const cdma_libre_sipreg_handlers,
    pub arg: *mut ::std::os::raw::c_void,
    pub sip: *mut sip,
    pub keepalive: *mut sip_keepalive,
    pub keepalive_interval_secs: u32,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct cdma_libre_sip_ctx {
    pub trace: cdma_libre_sip_trace_h,
    pub arg: *mut ::std::os::raw::c_void,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct cdma_libre_outbound_call {
    pub to_uri: *const ::std::os::raw::c_char,
    pub from_name: *const ::std::os::raw::c_char,
    pub from_uri: *const ::std::os::raw::c_char,
    pub contact_user: *const ::std::os::raw::c_char,
    pub call_id: *const ::std::os::raw::c_char,
    pub auth_enabled: u8,
    pub ctx: *mut cdma_libre_sipsess_ctx,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct cdma_libre_registration {
    pub registrar_uri: *const ::std::os::raw::c_char,
    pub to_uri: *const ::std::os::raw::c_char,
    pub from_name: *const ::std::os::raw::c_char,
    pub from_uri: *const ::std::os::raw::c_char,
    pub contact_user: *const ::std::os::raw::c_char,
    pub expires: u32,
    pub keepalive_interval_secs: u32,
    pub auth_enabled: u8,
    pub ctx: *mut cdma_libre_sipreg_ctx,
}

#[repr(C)]
#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub enum sip_transp {
    SIP_TRANSP_NONE = -1,
    SIP_TRANSP_UDP = 0,
    SIP_TRANSP_TCP = 1,
    SIP_TRANSP_TLS = 2,
    SIP_TRANSP_WS = 3,
    SIP_TRANSP_WSS = 4,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct sip {
    _unused: [u8; 0],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct sa {
    _unused: [u8; 128],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct mbuf {
    pub buf: *mut u8,
    pub size: usize,
    pub pos: usize,
    pub end: usize,
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct sipsess {
    _unused: [u8; 0],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct sipreg {
    _unused: [u8; 0],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct sip_keepalive {
    _unused: [u8; 0],
}

#[repr(C)]
#[derive(Debug, Copy, Clone)]
pub struct sipsess_sock {
    _unused: [u8; 0],
}

pub unsafe fn libre_init() -> ::std::os::raw::c_int {
    -1
}

pub unsafe fn libre_close() {}

pub unsafe fn re_main(_signalh: re_signal_h) -> ::std::os::raw::c_int {
    -1
}

pub unsafe fn re_cancel() {}

pub unsafe fn re_thread_enter() {}

pub unsafe fn re_thread_leave() {}

pub unsafe fn sa_set_str(
    _sa: *mut sa,
    _addr: *const ::std::os::raw::c_char,
    _port: u16,
) -> ::std::os::raw::c_int {
    -1
}

pub unsafe fn sa_ntop(
    _sa: *const sa,
    _buf: *mut ::std::os::raw::c_char,
    _size: ::std::os::raw::c_int,
) -> ::std::os::raw::c_int {
    -1
}

pub unsafe fn sa_port(_sa: *const sa) -> u16 {
    0
}

pub unsafe fn mem_deref(_data: *mut ::std::os::raw::c_void) -> *mut ::std::os::raw::c_void {
    ::std::ptr::null_mut()
}

pub unsafe fn cdma_libre_sip_alloc(
    _sipp: *mut *mut sip,
    _software: *const ::std::os::raw::c_char,
    _ctx: *mut cdma_libre_sip_ctx,
) -> ::std::os::raw::c_int {
    -1
}

pub unsafe fn cdma_libre_sip_transp_add(
    _sip: *mut sip,
    _tp: sip_transp,
    _laddr: *const sa,
) -> ::std::os::raw::c_int {
    -1
}

pub unsafe fn cdma_libre_sip_close(_sip: *mut sip, _force: u8) {}

pub unsafe fn cdma_libre_sip_deref(_sip: *mut sip) {}

pub unsafe fn cdma_libre_sipreg_alloc(
    _regp: *mut *mut sipreg,
    _sip: *mut sip,
    _registration: *const cdma_libre_registration,
) -> ::std::os::raw::c_int {
    -1
}

pub unsafe fn cdma_libre_sipreg_send(_reg: *mut sipreg) -> ::std::os::raw::c_int {
    -1
}

pub unsafe fn cdma_libre_sipreg_keepalive_stop(_ctx: *mut cdma_libre_sipreg_ctx) {}

pub unsafe fn cdma_libre_sipreg_deref(_reg: *mut sipreg) {}

pub unsafe fn cdma_libre_sipsess_listen(
    _sockp: *mut *mut sipsess_sock,
    _sip: *mut sip,
) -> ::std::os::raw::c_int {
    -1
}

pub unsafe fn cdma_libre_sipsess_close_all(_sock: *mut sipsess_sock) {}

pub unsafe fn cdma_libre_sipsess_sock_deref(_sock: *mut sipsess_sock) {}

pub unsafe fn cdma_libre_sipsess_connect(
    _sessp: *mut *mut sipsess,
    _sock: *mut sipsess_sock,
    _call: *const cdma_libre_outbound_call,
) -> ::std::os::raw::c_int {
    -1
}

pub unsafe fn cdma_libre_sipsess_abort(_sess: *mut sipsess) {}

pub unsafe fn cdma_libre_sipsess_deref(_sess: *mut sipsess) {}

pub unsafe fn cdma_libre_mbuf_from_str(
    _value: *const ::std::os::raw::c_char,
) -> *mut mbuf {
    ::std::ptr::null_mut()
}

pub unsafe fn cdma_libre_mbuf_rewind(_mb: *mut mbuf) {}

pub unsafe fn cdma_libre_strdup(
    _dst: *mut *mut ::std::os::raw::c_char,
    _src: *const ::std::os::raw::c_char,
) -> ::std::os::raw::c_int {
    -1
}
"#,
    )
    .expect("failed to write libre/re stub bindings");
}
