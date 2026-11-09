//! Layouts and constants follow usrsctp's FreeBSD numbering, not
//! `linux/sctp.h`.

#![allow(non_camel_case_types)]

use libc::{c_int, c_uint, c_void, size_t, sockaddr, sockaddr_storage, socklen_t, ssize_t};

pub type sctp_assoc_t = u32;

#[repr(C)]
pub struct socket {
    _private: [u8; 0],
}

pub const IPPROTO_SCTP: c_int = 132;
pub const MSG_NOTIFICATION: c_int = 0x2000;
pub const MSG_EOR: c_int = libc::MSG_EOR;

pub const SCTP_FUTURE_ASSOC: sctp_assoc_t = 0;
pub const SCTP_ALL_ASSOC: sctp_assoc_t = 2;

pub const SCTP_EVENT_READ: c_int = 0x0001;
pub const SCTP_EVENT_WRITE: c_int = 0x0002;
pub const SCTP_EVENT_ERROR: c_int = 0x0004;

pub const SCTP_RECVV_NOINFO: c_uint = 0;
pub const SCTP_RECVV_RCVINFO: c_uint = 1;
pub const SCTP_SENDV_SNDINFO: c_uint = 1;

pub const SCTP_RTOINFO: c_int = 0x0000_0001;
pub const SCTP_INITMSG: c_int = 0x0000_0003;
pub const SCTP_NODELAY: c_int = 0x0000_0004;
pub const SCTP_PRIMARY_ADDR: c_int = 0x0000_0007;
pub const SCTP_PEER_ADDR_PARAMS: c_int = 0x0000_000a;
pub const SCTP_EVENT: c_int = 0x0000_001e;
pub const SCTP_RECVRCVINFO: c_int = 0x0000_001f;
pub const SCTP_REMOTE_UDP_ENCAPS_PORT: c_int = 0x0000_0024;

pub const SCTP_ASSOC_CHANGE: u16 = 0x0001;
pub const SCTP_PEER_ADDR_CHANGE: u16 = 0x0002;
pub const SCTP_SHUTDOWN_EVENT: u16 = 0x0005;

pub const SCTP_COMM_UP: u16 = 0x0001;
pub const SCTP_COMM_LOST: u16 = 0x0002;
pub const SCTP_RESTART: u16 = 0x0003;
pub const SCTP_SHUTDOWN_COMP: u16 = 0x0004;
pub const SCTP_CANT_STR_ASSOC: u16 = 0x0005;

pub const SCTP_ADDR_AVAILABLE: u32 = 0x0001;
pub const SCTP_ADDR_UNREACHABLE: u32 = 0x0002;

pub const SCTP_UNORDERED: u16 = 0x0400;

pub const SPP_HB_ENABLE: u32 = 0x0000_0001;

pub const SCTP_BINDX_ADD_ADDR: c_int = 0x0000_8001;

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct sctp_rcvinfo {
    pub rcv_sid: u16,
    pub rcv_ssn: u16,
    pub rcv_flags: u16,
    pub rcv_ppid: u32,
    pub rcv_tsn: u32,
    pub rcv_cumtsn: u32,
    pub rcv_context: u32,
    pub rcv_assoc_id: sctp_assoc_t,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct sctp_sndinfo {
    pub snd_sid: u16,
    pub snd_flags: u16,
    pub snd_ppid: u32,
    pub snd_context: u32,
    pub snd_assoc_id: sctp_assoc_t,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct sctp_initmsg {
    pub sinit_num_ostreams: u16,
    pub sinit_max_instreams: u16,
    pub sinit_max_attempts: u16,
    pub sinit_max_init_timeo: u16,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct sctp_rtoinfo {
    pub srto_assoc_id: sctp_assoc_t,
    pub srto_initial: u32,
    pub srto_max: u32,
    pub srto_min: u32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy, Default)]
pub struct sctp_event {
    pub se_assoc_id: sctp_assoc_t,
    pub se_type: u16,
    pub se_on: u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct sctp_paddrparams {
    pub spp_address: sockaddr_storage,
    pub spp_assoc_id: sctp_assoc_t,
    pub spp_hbinterval: u32,
    pub spp_pathmtu: u32,
    pub spp_flags: u32,
    pub spp_ipv6_flowlabel: u32,
    pub spp_pathmaxrxt: u16,
    pub spp_dscp: u8,
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct sctp_setprim {
    pub ssp_addr: sockaddr_storage,
    pub ssp_assoc_id: sctp_assoc_t,
    pub ssp_padding: [u8; 4],
}

#[repr(C)]
#[derive(Clone, Copy)]
pub struct sctp_udpencaps {
    pub sue_address: sockaddr_storage,
    pub sue_assoc_id: u32,
    pub sue_port: u16,
}

pub type ConnOutput = Option<
    unsafe extern "C" fn(
        addr: *mut c_void,
        buffer: *mut c_void,
        length: size_t,
        tos: u8,
        set_df: u8,
    ) -> c_int,
>;

pub type DebugPrintf = Option<unsafe extern "C" fn(format: *const libc::c_char, ...)>;

pub type Upcall = Option<unsafe extern "C" fn(so: *mut socket, arg: *mut c_void, flags: c_int)>;

unsafe extern "C" {
    pub fn usrsctp_init(udp_port: u16, conn_output: ConnOutput, debug_printf: DebugPrintf);

    pub fn usrsctp_socket(
        domain: c_int,
        type_: c_int,
        protocol: c_int,
        receive_cb: *const c_void,
        send_cb: *const c_void,
        sb_threshold: u32,
        ulp_info: *mut c_void,
    ) -> *mut socket;

    pub fn usrsctp_setsockopt(
        so: *mut socket,
        level: c_int,
        option_name: c_int,
        option_value: *const c_void,
        option_len: socklen_t,
    ) -> c_int;

    pub fn usrsctp_getsockopt(
        so: *mut socket,
        level: c_int,
        option_name: c_int,
        option_value: *mut c_void,
        option_len: *mut socklen_t,
    ) -> c_int;

    pub fn usrsctp_getpaddrs(
        so: *mut socket,
        id: sctp_assoc_t,
        raddrs: *mut *mut sockaddr,
    ) -> c_int;
    pub fn usrsctp_freepaddrs(addrs: *mut sockaddr);
    pub fn usrsctp_getladdrs(
        so: *mut socket,
        id: sctp_assoc_t,
        raddrs: *mut *mut sockaddr,
    ) -> c_int;
    pub fn usrsctp_freeladdrs(addrs: *mut sockaddr);

    pub fn usrsctp_sendv(
        so: *mut socket,
        data: *const c_void,
        len: size_t,
        to: *mut sockaddr,
        addrcnt: c_int,
        info: *mut c_void,
        infolen: socklen_t,
        infotype: c_uint,
        flags: c_int,
    ) -> ssize_t;

    pub fn usrsctp_recvv(
        so: *mut socket,
        dbuf: *mut c_void,
        len: size_t,
        from: *mut sockaddr,
        fromlen: *mut socklen_t,
        info: *mut c_void,
        infolen: *mut socklen_t,
        infotype: *mut c_uint,
        msg_flags: *mut c_int,
    ) -> ssize_t;

    pub fn usrsctp_bindx(
        so: *mut socket,
        addrs: *mut sockaddr,
        addrcnt: c_int,
        flags: c_int,
    ) -> c_int;
    pub fn usrsctp_listen(so: *mut socket, backlog: c_int) -> c_int;
    pub fn usrsctp_accept(
        so: *mut socket,
        aname: *mut sockaddr,
        anamelen: *mut socklen_t,
    ) -> *mut socket;
    pub fn usrsctp_connectx(
        so: *mut socket,
        addrs: *const sockaddr,
        addrcnt: c_int,
        id: *mut sctp_assoc_t,
    ) -> c_int;
    pub fn usrsctp_close(so: *mut socket);
    pub fn usrsctp_shutdown(so: *mut socket, how: c_int) -> c_int;
    pub fn usrsctp_set_non_blocking(so: *mut socket, onoff: c_int) -> c_int;
    pub fn usrsctp_set_upcall(so: *mut socket, upcall: Upcall, arg: *mut c_void) -> c_int;
    pub fn usrsctp_get_events(so: *mut socket) -> c_int;
}
