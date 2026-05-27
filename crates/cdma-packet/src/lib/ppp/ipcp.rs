/// IPCP (IP Control Protocol) per RFC 1332.
///
/// Minimal BS/MSC-side implementation — assigns the mobile an IP address
/// and provides DNS server addresses.
///
/// Protocol number: 0x8021
///
/// Behavior:
/// - Mobile requests IP 0.0.0.0 → we NAK with assigned IP
/// - Mobile requests the assigned IP → we ACK
/// - We send our own Configure-Request with our gateway IP
/// - We include primary/secondary DNS in our NAK responses
/// - Mobile acks our request → IPCP is open
use super::framing::PppPacket;
use super::vj::{VJ_COMPRESSION_PROTOCOL, VjCompressionOptions};
use crate::mobile_ip::MobileIpConfig;
use std::collections::BTreeMap;
use std::net::Ipv4Addr;

pub const IPCP_PROTOCOL: u16 = 0x8021;

const CONFIGURE_REQUEST: u8 = 1;
const CONFIGURE_ACK: u8 = 2;
const CONFIGURE_NAK: u8 = 3;
const CONFIGURE_REJECT: u8 = 4;
const TERMINATE_REQUEST: u8 = 5;
const TERMINATE_ACK: u8 = 6;
const CODE_REJECT: u8 = 7;

/// PPP restart timer in packet-session ticks. Packet sessions tick every 20 ms,
/// so this retransmits pending Configure-Requests once per second.
const CONFIGURE_RESTART_TICKS: u16 = 50;
const MAX_CONFIGURE_RESTARTS: u32 = 10;
/// RFC 1661 §4.6 Max-Failure: after this many NAKs for an option without an
/// intervening ACK, convert to Reject (or drop if it was an appended suggestion).
const MAX_FAILURE: u32 = 5;
/// RFC 1661 §5.6: Rejected-Packet clamped below the default MRU.
const CODE_REJECT_MAX_DATA: usize = 1400;

// IPCP option types.
pub const IPCP_OPT_IP_COMPRESSION: u8 = 2;
const OPT_IP_ADDRESS: u8 = 3;
const OPT_MOBILE_IPV4: u8 = 4;
const OPT_PRIMARY_DNS: u8 = 129; // 0x81
const OPT_SECONDARY_DNS: u8 = 131; // 0x83

/// An IPCP option (type-length-value).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpcpOption {
    pub opt_type: u8,
    pub data: Vec<u8>,
}

fn parse_options(data: &[u8]) -> Vec<IpcpOption> {
    let mut opts = Vec::new();
    let mut i = 0;
    while i + 1 < data.len() {
        let opt_type = data[i];
        let opt_len = data[i + 1] as usize;
        if opt_len < 2 || i + opt_len > data.len() {
            break;
        }
        opts.push(IpcpOption {
            opt_type,
            data: data[i + 2..i + opt_len].to_vec(),
        });
        i += opt_len;
    }
    opts
}

fn serialize_options(opts: &[IpcpOption]) -> Vec<u8> {
    let mut out = Vec::new();
    for opt in opts {
        out.push(opt.opt_type);
        out.push((2 + opt.data.len()) as u8);
        out.extend_from_slice(&opt.data);
    }
    out
}

fn ip_to_bytes(addr: Ipv4Addr) -> Vec<u8> {
    addr.octets().to_vec()
}

fn bytes_to_ip(data: &[u8]) -> Option<Ipv4Addr> {
    if data.len() != 4 {
        return None;
    }
    Some(Ipv4Addr::new(data[0], data[1], data[2], data[3]))
}

/// An IPCP packet.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpcpPacket {
    pub code: u8,
    pub identifier: u8,
    pub data: Vec<u8>,
}

impl IpcpPacket {
    pub fn parse(payload: &[u8]) -> Option<Self> {
        if payload.len() < 4 {
            return None;
        }
        let code = payload[0];
        let identifier = payload[1];
        let length = ((payload[2] as u16) << 8) | payload[3] as u16;
        if (length as usize) < 4 || (length as usize) > payload.len() {
            return None;
        }
        let data = payload[4..length as usize].to_vec();
        Some(IpcpPacket {
            code,
            identifier,
            data,
        })
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let length = 4 + self.data.len() as u16;
        let mut out = Vec::with_capacity(length as usize);
        out.push(self.code);
        out.push(self.identifier);
        out.push((length >> 8) as u8);
        out.push((length & 0xFF) as u8);
        out.extend_from_slice(&self.data);
        out
    }

    pub fn to_ppp(&self) -> PppPacket {
        PppPacket {
            protocol: IPCP_PROTOCOL,
            payload: self.to_bytes(),
        }
    }
}

pub fn configure_request_peer_ip(ppp: &PppPacket) -> Option<Ipv4Addr> {
    let ipcp = IpcpPacket::parse(&ppp.payload)?;
    if ipcp.code != CONFIGURE_REQUEST {
        return None;
    }
    parse_options(&ipcp.data)
        .into_iter()
        .find(|opt| opt.opt_type == OPT_IP_ADDRESS)
        .and_then(|opt| bytes_to_ip(&opt.data))
}

/// IPCP negotiation state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IpcpState {
    Closed,
    RequestSent,
    AckReceived,
    AckSent,
    Opened,
}

/// IPCP configuration for the BS/MSC side.
#[derive(Debug, Clone)]
pub struct IpcpConfig {
    /// Our gateway IP.
    pub our_ip: Ipv4Addr,
    /// IP to assign to the mobile.
    pub peer_ip: Ipv4Addr,
    /// Primary DNS server.
    pub primary_dns: Ipv4Addr,
    /// Secondary DNS server.
    pub secondary_dns: Ipv4Addr,
    /// Whether our IPCP Configure-Request should ask the peer to send us VJ.
    pub request_vj: bool,
    /// Mobile IPv4 service behavior after IPCP opens without a peer address.
    pub mobile_ip: MobileIpConfig,
}

#[derive(Debug, Clone)]
pub struct IpcpOpenState {
    pub config: IpcpConfig,
    pub request_local_ip: bool,
    pub request_vj: bool,
    pub requested_vj: VjCompressionOptions,
    pub peer_vj: Option<VjCompressionOptions>,
    pub local_vj: Option<VjCompressionOptions>,
    pub last_acked_peer_request_data: Vec<u8>,
}

impl Default for IpcpConfig {
    fn default() -> Self {
        Self {
            our_ip: Ipv4Addr::new(10, 0, 0, 1),
            peer_ip: Ipv4Addr::new(10, 0, 0, 2),
            primary_dns: Ipv4Addr::new(10, 55, 0, 1),
            secondary_dns: Ipv4Addr::new(10, 55, 0, 1),
            request_vj: false,
            mobile_ip: MobileIpConfig::default(),
        }
    }
}

/// BS/MSC-side IPCP state machine.
#[derive(Debug)]
pub struct IpcpSession {
    pub state: IpcpState,
    pub config: IpcpConfig,
    log_context: Option<String>,
    next_id: u8,
    peer_acked: bool,
    we_acked: bool,
    last_request_id: Option<u8>,
    last_request_data: Vec<u8>,
    last_acked_peer_request_data: Vec<u8>,
    restart_ticks_remaining: u16,
    configure_restarts: u32,
    configure_failed: bool,
    /// RFC 1661 §4.6 Max-Failure: NAKs sent per option since last ACK.
    consecutive_naks_sent: BTreeMap<u8, u32>,
    request_local_ip: bool,
    request_vj: bool,
    requested_vj: VjCompressionOptions,
    peer_vj: Option<VjCompressionOptions>,
    local_vj: Option<VjCompressionOptions>,
}

impl IpcpSession {
    pub fn new(config: IpcpConfig) -> Self {
        let request_vj = config.request_vj;
        Self {
            state: IpcpState::Closed,
            config,
            log_context: None,
            next_id: 1,
            peer_acked: false,
            we_acked: false,
            last_request_id: None,
            last_request_data: Vec::new(),
            last_acked_peer_request_data: Vec::new(),
            restart_ticks_remaining: 0,
            configure_restarts: 0,
            configure_failed: false,
            consecutive_naks_sent: BTreeMap::new(),
            request_local_ip: true,
            request_vj,
            requested_vj: VjCompressionOptions::default(),
            peer_vj: None,
            local_vj: None,
        }
    }

    pub fn set_log_context(&mut self, context: String) {
        self.log_context = Some(context);
    }

    fn log_prefix(&self, label: &str) -> String {
        match self.log_context.as_deref() {
            Some(context) => format!("{}[{}]", label, context),
            None => label.to_string(),
        }
    }

    /// Generate our initial Configure-Request with our gateway IP.
    pub fn start(&mut self) -> PppPacket {
        let data = self.our_request_data();
        self.send_configure_request(data)
    }

    fn our_request_data(&self) -> Vec<u8> {
        let mut opts = Vec::new();
        if self.request_local_ip {
            opts.push(IpcpOption {
                opt_type: OPT_IP_ADDRESS,
                data: ip_to_bytes(self.config.our_ip),
            });
        }
        if self.request_vj {
            opts.push(IpcpOption {
                opt_type: IPCP_OPT_IP_COMPRESSION,
                data: self.requested_vj.to_ipcp_data().to_vec(),
            });
        }
        serialize_options(&opts)
    }

    fn send_configure_request(&mut self, data: Vec<u8>) -> PppPacket {
        let id = self.alloc_id();
        log::info!(
            "{}: Configure-Request id={} opts=[{}]",
            self.log_prefix("IPCP TX"),
            id,
            format_ipcp_options(&data)
        );
        let pkt = IpcpPacket {
            code: CONFIGURE_REQUEST,
            identifier: id,
            data: data.clone(),
        };
        self.last_request_id = Some(id);
        self.last_request_data = data;
        self.restart_ticks_remaining = CONFIGURE_RESTART_TICKS;
        self.state = IpcpState::RequestSent;
        self.configure_failed = false;
        pkt.to_ppp()
    }

    /// Advance the Configure-Request restart timer and retransmit if needed.
    pub fn maybe_retransmit_configure_request(&mut self) -> Option<PppPacket> {
        if self.state == IpcpState::Closed
            || self.state == IpcpState::Opened
            || self.state == IpcpState::AckReceived
        {
            return None;
        }
        let id = self.last_request_id?;
        if self.restart_ticks_remaining > 0 {
            self.restart_ticks_remaining -= 1;
            return None;
        }

        self.configure_restarts = self.configure_restarts.saturating_add(1);
        if self.configure_restarts > MAX_CONFIGURE_RESTARTS {
            log::warn!(
                "{}: Configure-Request failed after {} retransmits",
                self.log_prefix("IPCP"),
                self.configure_restarts - 1
            );
            self.state = IpcpState::Closed;
            self.peer_acked = false;
            self.we_acked = false;
            self.last_request_id = None;
            self.last_request_data.clear();
            self.restart_ticks_remaining = 0;
            self.configure_failed = true;
            return None;
        }
        self.restart_ticks_remaining = CONFIGURE_RESTART_TICKS;
        log::info!(
            "{}: Configure-Request retransmit id={} restart_count={} opts=[{}]",
            self.log_prefix("IPCP TX"),
            id,
            self.configure_restarts,
            format_ipcp_options(&self.last_request_data)
        );
        Some(
            IpcpPacket {
                code: CONFIGURE_REQUEST,
                identifier: id,
                data: self.last_request_data.clone(),
            }
            .to_ppp(),
        )
    }

    /// Process an incoming IPCP packet. Returns zero or more PPP packets to send.
    pub fn receive(&mut self, ppp: &PppPacket) -> Vec<PppPacket> {
        let ipcp = match IpcpPacket::parse(&ppp.payload) {
            Some(p) => p,
            None => return vec![],
        };

        let mut responses = Vec::new();

        match ipcp.code {
            CONFIGURE_REQUEST => {
                log::info!(
                    "{}: Configure-Request id={} state={:?} opts=[{}]",
                    self.log_prefix("IPCP RX"),
                    ipcp.identifier,
                    self.state,
                    format_ipcp_options(&ipcp.data)
                );

                // RFC 1661 §4.3: RCR in Opened triggers renegotiation.
                if self.state == IpcpState::Opened && ipcp.data == self.last_acked_peer_request_data
                {
                    log::info!(
                        "{}: duplicate Configure-Request id={} in Opened, re-ACKing without resetting IP",
                        self.log_prefix("IPCP RX"),
                        ipcp.identifier
                    );
                    let ack = IpcpPacket {
                        code: CONFIGURE_ACK,
                        identifier: ipcp.identifier,
                        data: ipcp.data.clone(),
                    };
                    log::info!(
                        "{}: Configure-Ack id={} opts=[{}]",
                        self.log_prefix("IPCP TX"),
                        ipcp.identifier,
                        format_ipcp_options(&ipcp.data)
                    );
                    responses.push(ack.to_ppp());
                    return responses;
                }
                if self.state == IpcpState::Opened {
                    log::info!(
                        "{}: peer restarted negotiation, re-sending Configure-Request",
                        self.log_prefix("IPCP")
                    );
                    self.peer_acked = false;
                    self.we_acked = false;
                    self.last_request_id = None;
                    self.last_request_data.clear();
                    self.last_acked_peer_request_data.clear();
                    self.restart_ticks_remaining = 0;
                    self.consecutive_naks_sent.clear();
                    self.request_local_ip = true;
                    self.request_vj = self.config.request_vj;
                    self.requested_vj = VjCompressionOptions::default();
                    self.peer_vj = None;
                    self.local_vj = None;
                    self.state = IpcpState::RequestSent;
                    responses.push(self.start());
                }

                let opts = parse_options(&ipcp.data);
                // peer_naks: peer sent option but value isn't acceptable.
                // appended: option to suggest the peer add to its next request.
                // rejects: peer sent option we don't recognise.
                let mut peer_naks: Vec<IpcpOption> = Vec::new();
                let mut appended: Vec<IpcpOption> = Vec::new();
                let mut rejects: Vec<IpcpOption> = Vec::new();

                for opt in &opts {
                    match opt.opt_type {
                        OPT_IP_ADDRESS => {
                            if bytes_to_ip(&opt.data) != Some(self.config.peer_ip) {
                                peer_naks.push(IpcpOption {
                                    opt_type: OPT_IP_ADDRESS,
                                    data: ip_to_bytes(self.config.peer_ip),
                                });
                            }
                        }
                        OPT_PRIMARY_DNS => {
                            if bytes_to_ip(&opt.data) != Some(self.config.primary_dns) {
                                peer_naks.push(IpcpOption {
                                    opt_type: OPT_PRIMARY_DNS,
                                    data: ip_to_bytes(self.config.primary_dns),
                                });
                            }
                        }
                        OPT_SECONDARY_DNS => {
                            if bytes_to_ip(&opt.data) != Some(self.config.secondary_dns) {
                                peer_naks.push(IpcpOption {
                                    opt_type: OPT_SECONDARY_DNS,
                                    data: ip_to_bytes(self.config.secondary_dns),
                                });
                            }
                        }
                        IPCP_OPT_IP_COMPRESSION => {
                            if VjCompressionOptions::from_ipcp_data(&opt.data).is_none() {
                                rejects.push(opt.clone());
                            }
                        }
                        OPT_MOBILE_IPV4 => {
                            rejects.push(opt.clone());
                        }
                        _ => {
                            rejects.push(opt.clone());
                        }
                    }
                }

                // RFC 1661 §5.3: append our IP-Address suggestion when the peer
                // didn't list it, bounded by Max-Failure.
                let peer_sent_ip = opts.iter().any(|o| o.opt_type == OPT_IP_ADDRESS);
                if !peer_sent_ip
                    && !self.config.mobile_ip.enabled
                    && self.nak_count(OPT_IP_ADDRESS) < MAX_FAILURE
                {
                    appended.push(IpcpOption {
                        opt_type: OPT_IP_ADDRESS,
                        data: ip_to_bytes(self.config.peer_ip),
                    });
                }

                // RFC 1661 §4.6: at Max-Failure, peer-sent NAKs convert to Reject.
                peer_naks.retain(|opt| {
                    if self.nak_count(opt.opt_type) >= MAX_FAILURE {
                        log::info!(
                            "{}: Max-Failure hit for option type={}, converting to Configure-Reject",
                            self.log_prefix("IPCP"),
                            opt.opt_type
                        );
                        if let Some(peer_opt) =
                            opts.iter().find(|o| o.opt_type == opt.opt_type)
                        {
                            rejects.push(peer_opt.clone());
                        }
                        false
                    } else {
                        true
                    }
                });

                if !rejects.is_empty() {
                    log::info!(
                        "{}: Configure-Reject id={} opts=[{}]",
                        self.log_prefix("IPCP TX"),
                        ipcp.identifier,
                        format_ipcp_options(&serialize_options(&rejects))
                    );
                    responses.push(
                        IpcpPacket {
                            code: CONFIGURE_REJECT,
                            identifier: ipcp.identifier,
                            data: serialize_options(&rejects),
                        }
                        .to_ppp(),
                    );
                    self.we_acked = false;
                    self.peer_vj = None;
                    self.update_state();
                    return responses;
                }

                let nak_opts: Vec<IpcpOption> =
                    peer_naks.iter().chain(appended.iter()).cloned().collect();

                if nak_opts.is_empty() {
                    log::info!(
                        "{}: Configure-Ack id={} opts=[{}]",
                        self.log_prefix("IPCP TX"),
                        ipcp.identifier,
                        format_ipcp_options(&ipcp.data)
                    );
                    let ack = IpcpPacket {
                        code: CONFIGURE_ACK,
                        identifier: ipcp.identifier,
                        data: ipcp.data.clone(),
                    };
                    responses.push(ack.to_ppp());
                    self.we_acked = true;
                    self.last_acked_peer_request_data = ipcp.data.clone();
                    self.peer_vj = parse_vj_option(&ipcp.data);
                    // RFC 1661 §4.6: ACK resets Max-Failure.
                    self.consecutive_naks_sent.clear();
                } else {
                    log::info!(
                        "{}: Configure-Nak id={} opts=[{}]",
                        self.log_prefix("IPCP TX"),
                        ipcp.identifier,
                        format_ipcp_options(&serialize_options(&nak_opts))
                    );
                    let nak = IpcpPacket {
                        code: CONFIGURE_NAK,
                        identifier: ipcp.identifier,
                        data: serialize_options(&nak_opts),
                    };
                    responses.push(nak.to_ppp());
                    for opt in &nak_opts {
                        *self.consecutive_naks_sent.entry(opt.opt_type).or_insert(0) += 1;
                    }
                    self.peer_vj = None;
                }

                self.update_state();
            }
            CONFIGURE_ACK => {
                if self.last_request_id != Some(ipcp.identifier)
                    || ipcp.data != self.last_request_data
                {
                    log::info!(
                        "{}: invalid Configure-Ack id={} expected_id={:?} opts=[{}], discarding",
                        self.log_prefix("IPCP RX"),
                        ipcp.identifier,
                        self.last_request_id,
                        format_ipcp_options(&ipcp.data)
                    );
                    return responses;
                }
                log::info!(
                    "{}: Configure-Ack id={} opts=[{}]",
                    self.log_prefix("IPCP RX"),
                    ipcp.identifier,
                    format_ipcp_options(&ipcp.data)
                );
                self.peer_acked = true;
                self.local_vj = parse_vj_option(&ipcp.data);
                self.restart_ticks_remaining = 0;
                self.update_state();
            }
            CONFIGURE_NAK => {
                if self.last_request_id != Some(ipcp.identifier) {
                    log::info!(
                        "{}: Configure-Nak id={} does not match last request {:?}, discarding",
                        self.log_prefix("IPCP RX"),
                        ipcp.identifier,
                        self.last_request_id
                    );
                    return responses;
                }
                log::info!(
                    "{}: Configure-Nak id={} opts=[{}]",
                    self.log_prefix("IPCP RX"),
                    ipcp.identifier,
                    format_ipcp_options(&ipcp.data)
                );
                // RFC 1661 §5.3 adoption is MAY; keep our IP-Address value.
                for opt in parse_options(&ipcp.data) {
                    if opt.opt_type == OPT_IP_ADDRESS
                        && let Some(ip) = bytes_to_ip(&opt.data)
                        && ip != self.config.our_ip
                    {
                        log::info!(
                            "{}: peer NAKed our local IP {} with {}, keeping our value",
                            self.log_prefix("IPCP"),
                            self.config.our_ip,
                            ip
                        );
                    }
                    if opt.opt_type == IPCP_OPT_IP_COMPRESSION {
                        if let Some(vj) = VjCompressionOptions::from_ipcp_data(&opt.data) {
                            log::info!(
                                "{}: peer NAKed VJ compression with protocol=0x{:04x} max_slot_id={} comp_slot_id={}, adopting",
                                self.log_prefix("IPCP"),
                                VJ_COMPRESSION_PROTOCOL,
                                vj.max_slot_id,
                                u8::from(vj.comp_slot_id)
                            );
                            self.requested_vj = vj;
                        } else {
                            log::info!(
                                "{}: peer NAKed VJ compression with unsupported data, continuing without it",
                                self.log_prefix("IPCP")
                            );
                            self.request_vj = false;
                        }
                    }
                }
                let data = self.our_request_data();
                self.peer_acked = false;
                self.local_vj = None;
                responses.push(self.send_configure_request(data));
                self.update_state();
            }
            CONFIGURE_REJECT => {
                if self.last_request_id != Some(ipcp.identifier) {
                    log::info!(
                        "{}: Configure-Reject id={} does not match last request {:?}, discarding",
                        self.log_prefix("IPCP RX"),
                        ipcp.identifier,
                        self.last_request_id
                    );
                    return responses;
                }
                log::info!(
                    "{}: Configure-Reject id={} opts=[{}]",
                    self.log_prefix("IPCP RX"),
                    ipcp.identifier,
                    format_ipcp_options(&ipcp.data)
                );
                for opt in parse_options(&ipcp.data) {
                    if opt.opt_type == OPT_IP_ADDRESS {
                        log::info!(
                            "{}: peer rejected our local IP option, continuing without it",
                            self.log_prefix("IPCP")
                        );
                        self.request_local_ip = false;
                    }
                    if opt.opt_type == IPCP_OPT_IP_COMPRESSION {
                        log::info!(
                            "{}: peer rejected our VJ compression option, continuing without it",
                            self.log_prefix("IPCP")
                        );
                        self.request_vj = false;
                    }
                }
                self.peer_acked = false;
                self.local_vj = None;
                responses.push(self.send_configure_request(self.our_request_data()));
                self.update_state();
            }
            TERMINATE_REQUEST => {
                // RFC 1661 §5.5: ACK and close down.
                log::info!(
                    "{}: Terminate-Request id={} data=[{}]",
                    self.log_prefix("IPCP RX"),
                    ipcp.identifier,
                    ipcp.data
                        .iter()
                        .map(|b| format!("{:02x}", b))
                        .collect::<String>()
                );
                let ack = IpcpPacket {
                    code: TERMINATE_ACK,
                    identifier: ipcp.identifier,
                    data: vec![],
                };
                responses.push(ack.to_ppp());
                self.state = IpcpState::Closed;
                self.peer_acked = false;
                self.we_acked = false;
                self.last_request_id = None;
                self.last_request_data.clear();
                self.last_acked_peer_request_data.clear();
                self.restart_ticks_remaining = 0;
                self.configure_failed = false;
                self.consecutive_naks_sent.clear();
                self.request_local_ip = true;
                self.request_vj = self.config.request_vj;
                self.requested_vj = VjCompressionOptions::default();
                self.peer_vj = None;
                self.local_vj = None;
            }
            TERMINATE_ACK => {
                // RFC 1661 §5.5: peer reports Closed; informational since we never
                // send Terminate-Request today.
                log::info!(
                    "{}: Terminate-Ack id={} (peer reports IPCP closed)",
                    self.log_prefix("IPCP RX"),
                    ipcp.identifier
                );
                self.state = IpcpState::Closed;
                self.peer_acked = false;
                self.we_acked = false;
                self.last_request_id = None;
                self.last_request_data.clear();
                self.last_acked_peer_request_data.clear();
                self.restart_ticks_remaining = 0;
                self.peer_vj = None;
                self.local_vj = None;
            }
            CODE_REJECT => {
                // RFC 1661 §5.6: peer rejected a standard IPCP code; let LCP teardown handle it.
                log::warn!(
                    "{}: Code-Reject id={} data=[{}]",
                    self.log_prefix("IPCP RX"),
                    ipcp.identifier,
                    ipcp.data
                        .iter()
                        .map(|b| format!("{:02x}", b))
                        .collect::<String>()
                );
            }
            _ => {
                // RFC 1661 §5.6: unknown code → Code-Reject with the offending packet.
                log::info!(
                    "{}: Code-Reject for unknown code={} id={}",
                    self.log_prefix("IPCP TX"),
                    ipcp.code,
                    ipcp.identifier
                );
                let mut data = ppp.payload.clone();
                if data.len() > CODE_REJECT_MAX_DATA {
                    data.truncate(CODE_REJECT_MAX_DATA);
                }
                let reject = IpcpPacket {
                    code: CODE_REJECT,
                    identifier: self.alloc_id(),
                    data,
                };
                responses.push(reject.to_ppp());
            }
        }

        responses
    }

    /// Returns true when IPCP is open (both sides acked).
    pub fn is_open(&self) -> bool {
        self.state == IpcpState::Opened
    }

    pub fn open_state(&self) -> Option<IpcpOpenState> {
        self.is_open().then(|| IpcpOpenState {
            config: self.config.clone(),
            request_local_ip: self.request_local_ip,
            request_vj: self.request_vj,
            requested_vj: self.requested_vj,
            peer_vj: self.peer_vj,
            local_vj: self.local_vj,
            last_acked_peer_request_data: self.last_acked_peer_request_data.clone(),
        })
    }

    pub fn restore_open_state(&mut self, state: IpcpOpenState) {
        self.config = state.config;
        self.request_local_ip = state.request_local_ip;
        self.request_vj = state.request_vj;
        self.requested_vj = state.requested_vj;
        self.peer_vj = state.peer_vj;
        self.local_vj = state.local_vj;
        self.last_acked_peer_request_data = state.last_acked_peer_request_data;
        self.state = IpcpState::Opened;
        self.peer_acked = true;
        self.we_acked = true;
        self.last_request_id = None;
        self.last_request_data.clear();
        self.restart_ticks_remaining = 0;
        self.configure_failed = false;
        self.consecutive_naks_sent.clear();
    }

    pub fn restart_for_simple_ip(&mut self) -> PppPacket {
        self.config.mobile_ip.enabled = false;
        self.peer_acked = false;
        self.we_acked = false;
        self.last_request_id = None;
        self.last_request_data.clear();
        self.last_acked_peer_request_data.clear();
        self.restart_ticks_remaining = 0;
        self.configure_failed = false;
        self.consecutive_naks_sent.clear();
        self.request_local_ip = true;
        self.request_vj = self.config.request_vj;
        self.requested_vj = VjCompressionOptions::default();
        self.peer_vj = None;
        self.local_vj = None;
        log::info!(
            "{}: restarting negotiation in Simple IP mode",
            self.log_prefix("IPCP")
        );
        self.start()
    }

    /// Reassign the configured peer IP without touching state flags or timers;
    /// the next peer Configure-Request decides ACK vs NAK normally.
    pub fn reassign_peer_ip(&mut self, peer_ip: Ipv4Addr) {
        self.config.peer_ip = peer_ip;
    }

    /// Returns the IP assigned to the mobile peer.
    pub fn peer_ip(&self) -> Ipv4Addr {
        self.config.peer_ip
    }

    /// Returns our gateway IP.
    pub fn our_ip(&self) -> Ipv4Addr {
        self.config.our_ip
    }

    pub fn configure_failed(&self) -> bool {
        self.configure_failed
    }

    pub fn configure_restarts(&self) -> u32 {
        self.configure_restarts
    }

    pub fn peer_vj_options(&self) -> Option<VjCompressionOptions> {
        self.peer_vj
    }

    pub fn local_vj_options(&self) -> Option<VjCompressionOptions> {
        self.local_vj
    }

    pub fn peer_ip_address_negotiated(&self) -> bool {
        parse_options(&self.last_acked_peer_request_data)
            .iter()
            .any(|opt| opt.opt_type == OPT_IP_ADDRESS)
    }

    /// Telemetry: NAKs containing IP-Address since last ACK.
    pub fn omitted_peer_ip_naks(&self) -> u32 {
        self.nak_count(OPT_IP_ADDRESS)
    }

    fn nak_count(&self, opt_type: u8) -> u32 {
        self.consecutive_naks_sent
            .get(&opt_type)
            .copied()
            .unwrap_or(0)
    }

    fn update_state(&mut self) {
        self.state = match (self.we_acked, self.peer_acked) {
            (true, true) => IpcpState::Opened,
            (true, false) => IpcpState::AckSent,
            (false, true) => IpcpState::AckReceived,
            (false, false) => self.state,
        };
    }

    fn alloc_id(&mut self) -> u8 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        id
    }
}

fn parse_vj_option(data: &[u8]) -> Option<VjCompressionOptions> {
    parse_options(data)
        .into_iter()
        .find(|opt| opt.opt_type == IPCP_OPT_IP_COMPRESSION)
        .and_then(|opt| VjCompressionOptions::from_ipcp_data(&opt.data))
}

/// Format IPCP options for logging.
fn format_ipcp_options(data: &[u8]) -> String {
    let opts = parse_options(data);
    opts.iter()
        .map(|o| match o.opt_type {
            IPCP_OPT_IP_COMPRESSION => {
                if o.data.len() == 4 {
                    let protocol = u16::from_be_bytes([o.data[0], o.data[1]]);
                    format!(
                        "IPCompression(protocol=0x{:04x},max_slot_id={},comp_slot_id={})",
                        protocol, o.data[2], o.data[3]
                    )
                } else {
                    format!("IPCompression(type={} len={})", o.opt_type, o.data.len())
                }
            }
            OPT_MOBILE_IPV4 => format!("MobileIPv4(len={})", o.data.len()),
            OPT_IP_ADDRESS | OPT_PRIMARY_DNS | OPT_SECONDARY_DNS => {
                let name = match o.opt_type {
                    OPT_IP_ADDRESS => "IP",
                    OPT_PRIMARY_DNS => "PrimaryDNS",
                    OPT_SECONDARY_DNS => "SecondaryDNS",
                    _ => unreachable!(),
                };
                if o.data.len() == 4 {
                    let ip = Ipv4Addr::new(o.data[0], o.data[1], o.data[2], o.data[3]);
                    format!("{}={}", name, ip)
                } else {
                    format!("{}(type={} len={})", name, o.opt_type, o.data.len())
                }
            }
            _ => {
                if o.data.len() == 4 {
                    let ip = Ipv4Addr::new(o.data[0], o.data[1], o.data[2], o.data[3]);
                    format!("Unknown={}", ip)
                } else {
                    format!("Unknown(type={} len={})", o.opt_type, o.data.len())
                }
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn local_vj_config() -> IpcpConfig {
        IpcpConfig {
            request_vj: true,
            ..IpcpConfig::default()
        }
    }

    fn ack_last_request(session: &mut IpcpSession) {
        let ack = IpcpPacket {
            code: CONFIGURE_ACK,
            identifier: session
                .last_request_id
                .expect("session should have an outstanding request"),
            data: session.last_request_data.clone(),
        };
        session.receive(&ack.to_ppp());
    }

    #[test]
    fn ipcp_packet_round_trip() {
        let pkt = IpcpPacket {
            code: CONFIGURE_REQUEST,
            identifier: 3,
            data: vec![3, 6, 10, 0, 0, 2],
        };
        let bytes = pkt.to_bytes();
        let parsed = IpcpPacket::parse(&bytes).unwrap();
        assert_eq!(parsed, pkt);
    }

    #[test]
    fn full_ipcp_negotiation_with_zero_ip() {
        let mut session = IpcpSession::new(IpcpConfig::default());

        // Step 1: BS sends Configure-Request with our IP (10.0.0.1).
        let our_req = session.start();
        assert_eq!(our_req.protocol, IPCP_PROTOCOL);
        assert_eq!(session.state, IpcpState::RequestSent);

        // Step 2: Mobile requests IP=0.0.0.0 (asking for assignment).
        let mobile_req_data = serialize_options(&[IpcpOption {
            opt_type: OPT_IP_ADDRESS,
            data: vec![0, 0, 0, 0],
        }]);
        let mobile_req = IpcpPacket {
            code: CONFIGURE_REQUEST,
            identifier: 1,
            data: mobile_req_data,
        };
        let responses = session.receive(&mobile_req.to_ppp());

        // Should NAK with 10.0.0.2.
        assert_eq!(responses.len(), 1);
        let nak = IpcpPacket::parse(&responses[0].payload).unwrap();
        assert_eq!(nak.code, CONFIGURE_NAK);
        let nak_opts = parse_options(&nak.data);
        assert_eq!(nak_opts.len(), 1);
        assert_eq!(nak_opts[0].opt_type, OPT_IP_ADDRESS);
        assert_eq!(
            bytes_to_ip(&nak_opts[0].data),
            Some(Ipv4Addr::new(10, 0, 0, 2))
        );

        // Step 3: Mobile retries with correct IP.
        let mobile_req2_data = serialize_options(&[IpcpOption {
            opt_type: OPT_IP_ADDRESS,
            data: vec![10, 0, 0, 2],
        }]);
        let mobile_req2 = IpcpPacket {
            code: CONFIGURE_REQUEST,
            identifier: 2,
            data: mobile_req2_data,
        };
        let responses = session.receive(&mobile_req2.to_ppp());
        assert_eq!(responses.len(), 1);
        let ack = IpcpPacket::parse(&responses[0].payload).unwrap();
        assert_eq!(ack.code, CONFIGURE_ACK);
        assert_eq!(session.state, IpcpState::AckSent);

        // Step 4: Mobile acks our Configure-Request.
        let our_req_parsed = IpcpPacket::parse(&our_req.payload).unwrap();
        let mobile_ack = IpcpPacket {
            code: CONFIGURE_ACK,
            identifier: our_req_parsed.identifier,
            data: our_req_parsed.data.clone(),
        };
        let responses = session.receive(&mobile_ack.to_ppp());
        assert!(responses.is_empty());
        assert_eq!(session.state, IpcpState::Opened);
        assert!(session.is_open());
        assert_eq!(session.peer_ip(), Ipv4Addr::new(10, 0, 0, 2));
        assert_eq!(session.our_ip(), Ipv4Addr::new(10, 0, 0, 1));
    }

    #[test]
    fn mobile_ip_mode_acks_dns_only_request_without_peer_ip() {
        let mut session = IpcpSession::new(IpcpConfig {
            mobile_ip: MobileIpConfig {
                enabled: true,
                ..MobileIpConfig::default()
            },
            ..IpcpConfig::default()
        });
        session.start();
        ack_last_request(&mut session);

        let mobile_req_data = serialize_options(&[
            IpcpOption {
                opt_type: OPT_PRIMARY_DNS,
                data: ip_to_bytes(Ipv4Addr::new(10, 55, 0, 1)),
            },
            IpcpOption {
                opt_type: OPT_SECONDARY_DNS,
                data: ip_to_bytes(Ipv4Addr::new(10, 55, 0, 1)),
            },
        ]);
        let mobile_req = IpcpPacket {
            code: CONFIGURE_REQUEST,
            identifier: 7,
            data: mobile_req_data.clone(),
        };
        let responses = session.receive(&mobile_req.to_ppp());

        assert_eq!(responses.len(), 1);
        let ack = IpcpPacket::parse(&responses[0].payload).unwrap();
        assert_eq!(ack.code, CONFIGURE_ACK);
        assert_eq!(ack.data, mobile_req_data);
        assert_eq!(session.state, IpcpState::Opened);
        assert!(!session.peer_ip_address_negotiated());
    }

    #[test]
    fn mobile_requests_correct_ip_immediately() {
        let mut session = IpcpSession::new(IpcpConfig::default());
        session.start();

        // Mobile already knows its IP.
        let mobile_req_data = serialize_options(&[IpcpOption {
            opt_type: OPT_IP_ADDRESS,
            data: vec![10, 0, 0, 2],
        }]);
        let mobile_req = IpcpPacket {
            code: CONFIGURE_REQUEST,
            identifier: 1,
            data: mobile_req_data,
        };
        let responses = session.receive(&mobile_req.to_ppp());
        assert_eq!(responses.len(), 1);
        let ack = IpcpPacket::parse(&responses[0].payload).unwrap();
        assert_eq!(ack.code, CONFIGURE_ACK);
    }

    #[test]
    fn dns_options_nak_with_correct_values() {
        let mut session = IpcpSession::new(IpcpConfig::default());
        session.start();

        // Mobile requests IP + DNS with zeros.
        let mobile_req_data = serialize_options(&[
            IpcpOption {
                opt_type: OPT_IP_ADDRESS,
                data: vec![0, 0, 0, 0],
            },
            IpcpOption {
                opt_type: OPT_PRIMARY_DNS,
                data: vec![0, 0, 0, 0],
            },
            IpcpOption {
                opt_type: OPT_SECONDARY_DNS,
                data: vec![0, 0, 0, 0],
            },
        ]);
        let mobile_req = IpcpPacket {
            code: CONFIGURE_REQUEST,
            identifier: 1,
            data: mobile_req_data,
        };
        let responses = session.receive(&mobile_req.to_ppp());
        assert_eq!(responses.len(), 1);
        let nak = IpcpPacket::parse(&responses[0].payload).unwrap();
        assert_eq!(nak.code, CONFIGURE_NAK);

        let nak_opts = parse_options(&nak.data);
        assert_eq!(nak_opts.len(), 3);

        // IP
        assert_eq!(nak_opts[0].opt_type, OPT_IP_ADDRESS);
        assert_eq!(
            bytes_to_ip(&nak_opts[0].data),
            Some(Ipv4Addr::new(10, 0, 0, 2))
        );
        // Primary DNS
        assert_eq!(nak_opts[1].opt_type, OPT_PRIMARY_DNS);
        assert_eq!(
            bytes_to_ip(&nak_opts[1].data),
            Some(Ipv4Addr::new(10, 55, 0, 1))
        );
        // Secondary DNS
        assert_eq!(nak_opts[2].opt_type, OPT_SECONDARY_DNS);
        assert_eq!(
            bytes_to_ip(&nak_opts[2].data),
            Some(Ipv4Addr::new(10, 55, 0, 1))
        );
    }

    #[test]
    fn omitted_peer_ip_accepts_after_max_failure() {
        // RFC 1661 §4.6 Max-Failure: after MAX_FAILURE NAKs appending the
        // IP-Address option (peer kept omitting it), we stop appending
        // and ACK whatever the peer sent.
        let mut session = IpcpSession::new(IpcpConfig::default());
        session.start();

        let mobile_req_data = serialize_options(&[
            IpcpOption {
                opt_type: OPT_PRIMARY_DNS,
                data: vec![10, 55, 0, 1],
            },
            IpcpOption {
                opt_type: OPT_SECONDARY_DNS,
                data: vec![10, 55, 0, 1],
            },
        ]);

        for id in 1..=MAX_FAILURE {
            let mobile_req = IpcpPacket {
                code: CONFIGURE_REQUEST,
                identifier: id as u8,
                data: mobile_req_data.clone(),
            };
            let responses = session.receive(&mobile_req.to_ppp());
            assert_eq!(responses.len(), 1);
            let nak = IpcpPacket::parse(&responses[0].payload).unwrap();
            assert_eq!(nak.code, CONFIGURE_NAK);
            let nak_opts = parse_options(&nak.data);
            assert_eq!(nak_opts.len(), 1);
            assert_eq!(nak_opts[0].opt_type, OPT_IP_ADDRESS);
            assert_eq!(
                bytes_to_ip(&nak_opts[0].data),
                Some(Ipv4Addr::new(10, 0, 0, 2))
            );
            assert_eq!(session.state, IpcpState::RequestSent);
        }

        let mobile_req = IpcpPacket {
            code: CONFIGURE_REQUEST,
            identifier: (MAX_FAILURE + 1) as u8,
            data: mobile_req_data.clone(),
        };
        let responses = session.receive(&mobile_req.to_ppp());
        assert_eq!(responses.len(), 1);
        let ack = IpcpPacket::parse(&responses[0].payload).unwrap();
        assert_eq!(ack.code, CONFIGURE_ACK);
        assert_eq!(ack.data, mobile_req_data);

        // ACK clears the counter per RFC §4.6.
        assert_eq!(session.omitted_peer_ip_naks(), 0);
        assert_eq!(session.peer_ip(), Ipv4Addr::new(10, 0, 0, 2));
        assert_eq!(session.state, IpcpState::AckSent);
    }

    #[test]
    fn stale_peer_ip_is_naked_with_assigned_ip() {
        let mut session = IpcpSession::new(IpcpConfig::default());
        session.start();

        let mobile_req_data = serialize_options(&[IpcpOption {
            opt_type: OPT_IP_ADDRESS,
            data: vec![10, 0, 0, 3],
        }]);
        let mobile_req = IpcpPacket {
            code: CONFIGURE_REQUEST,
            identifier: 1,
            data: mobile_req_data,
        };
        let responses = session.receive(&mobile_req.to_ppp());
        assert_eq!(responses.len(), 1);
        let nak = IpcpPacket::parse(&responses[0].payload).unwrap();
        assert_eq!(nak.code, CONFIGURE_NAK);
        let nak_opts = parse_options(&nak.data);
        assert_eq!(nak_opts.len(), 1);
        assert_eq!(nak_opts[0].opt_type, OPT_IP_ADDRESS);
        assert_eq!(
            bytes_to_ip(&nak_opts[0].data),
            Some(Ipv4Addr::new(10, 0, 0, 2))
        );
        assert_eq!(session.state, IpcpState::RequestSent);
    }

    #[test]
    fn dns_correct_values_accepted() {
        let mut session = IpcpSession::new(IpcpConfig::default());
        session.start();

        // Mobile requests with all correct values.
        let mobile_req_data = serialize_options(&[
            IpcpOption {
                opt_type: OPT_IP_ADDRESS,
                data: vec![10, 0, 0, 2],
            },
            IpcpOption {
                opt_type: OPT_PRIMARY_DNS,
                data: vec![10, 55, 0, 1],
            },
            IpcpOption {
                opt_type: OPT_SECONDARY_DNS,
                data: vec![10, 55, 0, 1],
            },
        ]);
        let mobile_req = IpcpPacket {
            code: CONFIGURE_REQUEST,
            identifier: 1,
            data: mobile_req_data,
        };
        let responses = session.receive(&mobile_req.to_ppp());
        assert_eq!(responses.len(), 1);
        let ack = IpcpPacket::parse(&responses[0].payload).unwrap();
        assert_eq!(ack.code, CONFIGURE_ACK);
    }

    #[test]
    fn our_ip_nak_keeps_our_value() {
        // RFC 1661 §5.3: adoption of the peer's NAK suggestion is MAY.
        // We refuse — re-propose the same IP-Address option with our
        // gateway value.
        let mut session = IpcpSession::new(IpcpConfig::default());
        session.start();

        let nak = IpcpPacket {
            code: CONFIGURE_NAK,
            identifier: 1,
            data: serialize_options(&[IpcpOption {
                opt_type: OPT_IP_ADDRESS,
                data: vec![10, 0, 0, 100],
            }]),
        };
        let responses = session.receive(&nak.to_ppp());
        assert_eq!(responses.len(), 1);
        let retry = IpcpPacket::parse(&responses[0].payload).unwrap();
        assert_eq!(retry.code, CONFIGURE_REQUEST);
        let opts = parse_options(&retry.data);
        let ip = opts
            .iter()
            .find(|opt| opt.opt_type == OPT_IP_ADDRESS)
            .expect("retry should still include our IP option");
        assert_eq!(bytes_to_ip(&ip.data), Some(Ipv4Addr::new(10, 0, 0, 1)));
        assert_eq!(session.our_ip(), Ipv4Addr::new(10, 0, 0, 1));
    }

    #[test]
    fn unknown_peer_option_is_rejected() {
        let mut session = IpcpSession::new(IpcpConfig::default());
        session.start();

        let mobile_req_data = serialize_options(&[
            IpcpOption {
                opt_type: OPT_IP_ADDRESS,
                data: vec![10, 0, 0, 2],
            },
            IpcpOption {
                opt_type: 200,
                data: vec![1, 2],
            },
        ]);
        let mobile_req = IpcpPacket {
            code: CONFIGURE_REQUEST,
            identifier: 9,
            data: mobile_req_data,
        };
        let responses = session.receive(&mobile_req.to_ppp());
        assert_eq!(responses.len(), 1);
        let reject = IpcpPacket::parse(&responses[0].payload).unwrap();
        assert_eq!(reject.code, CONFIGURE_REJECT);
        let reject_opts = parse_options(&reject.data);
        assert_eq!(reject_opts.len(), 1);
        assert_eq!(reject_opts[0].opt_type, 200);
        assert_eq!(session.state, IpcpState::RequestSent);
    }

    #[test]
    fn peer_vj_ip_compression_option_is_acked() {
        let mut session = IpcpSession::new(IpcpConfig::default());
        session.start();

        let mobile_req_data = serialize_options(&[
            IpcpOption {
                opt_type: OPT_IP_ADDRESS,
                data: vec![10, 0, 0, 2],
            },
            IpcpOption {
                opt_type: IPCP_OPT_IP_COMPRESSION,
                data: vec![0x00, 0x2d, 0x0f, 0x01],
            },
        ]);
        let mobile_req = IpcpPacket {
            code: CONFIGURE_REQUEST,
            identifier: 9,
            data: mobile_req_data.clone(),
        };
        let responses = session.receive(&mobile_req.to_ppp());
        assert_eq!(responses.len(), 1);
        let ack = IpcpPacket::parse(&responses[0].payload).unwrap();
        assert_eq!(ack.code, CONFIGURE_ACK);
        assert_eq!(ack.data, mobile_req_data);
        assert_eq!(
            session.peer_vj_options(),
            Some(VjCompressionOptions {
                max_slot_id: 15,
                comp_slot_id: true,
            })
        );
    }

    #[test]
    fn malformed_or_unsupported_peer_vj_option_is_rejected() {
        let mut session = IpcpSession::new(IpcpConfig::default());
        session.start();

        for (identifier, data) in [
            (1, vec![0x00, 0x2d, 0x0f]),
            (2, vec![0x00, 0x21, 0x0f, 0x01]),
        ] {
            let mobile_req = IpcpPacket {
                code: CONFIGURE_REQUEST,
                identifier,
                data: serialize_options(&[
                    IpcpOption {
                        opt_type: OPT_IP_ADDRESS,
                        data: vec![10, 0, 0, 2],
                    },
                    IpcpOption {
                        opt_type: IPCP_OPT_IP_COMPRESSION,
                        data,
                    },
                ]),
            };
            let responses = session.receive(&mobile_req.to_ppp());
            assert_eq!(responses.len(), 1);
            let reject = IpcpPacket::parse(&responses[0].payload).unwrap();
            assert_eq!(reject.code, CONFIGURE_REJECT);
            let reject_opts = parse_options(&reject.data);
            assert_eq!(reject_opts.len(), 1);
            assert_eq!(reject_opts[0].opt_type, IPCP_OPT_IP_COMPRESSION);
            assert_eq!(session.peer_vj_options(), None);
        }
    }

    #[test]
    fn default_request_omits_vj() {
        let mut session = IpcpSession::new(IpcpConfig::default());
        let our_req = session.start();
        let our_ipcp = IpcpPacket::parse(&our_req.payload).unwrap();
        let opts = parse_options(&our_ipcp.data);
        assert!(
            !opts
                .iter()
                .any(|opt| opt.opt_type == IPCP_OPT_IP_COMPRESSION)
        );
    }

    #[test]
    fn local_vj_request_includes_vj_and_peer_reject_disables_it() {
        let mut session = IpcpSession::new(local_vj_config());
        let our_req = session.start();
        let our_ipcp = IpcpPacket::parse(&our_req.payload).unwrap();
        let opts = parse_options(&our_ipcp.data);
        assert!(
            opts.iter()
                .any(|opt| opt.opt_type == IPCP_OPT_IP_COMPRESSION
                    && opt.data == VjCompressionOptions::default().to_ipcp_data().to_vec())
        );

        let reject = IpcpPacket {
            code: CONFIGURE_REJECT,
            identifier: our_ipcp.identifier,
            data: serialize_options(&[IpcpOption {
                opt_type: IPCP_OPT_IP_COMPRESSION,
                data: VjCompressionOptions::default().to_ipcp_data().to_vec(),
            }]),
        };
        let responses = session.receive(&reject.to_ppp());
        assert_eq!(responses.len(), 1);
        let retry = IpcpPacket::parse(&responses[0].payload).unwrap();
        assert_eq!(retry.code, CONFIGURE_REQUEST);
        assert!(
            !parse_options(&retry.data)
                .iter()
                .any(|opt| opt.opt_type == IPCP_OPT_IP_COMPRESSION)
        );
    }

    #[test]
    fn peer_nak_of_local_vj_option_is_adopted_when_supported() {
        let mut session = IpcpSession::new(local_vj_config());
        let our_req = session.start();
        let our_ipcp = IpcpPacket::parse(&our_req.payload).unwrap();
        let suggested = VjCompressionOptions {
            max_slot_id: 3,
            comp_slot_id: false,
        };
        let nak = IpcpPacket {
            code: CONFIGURE_NAK,
            identifier: our_ipcp.identifier,
            data: serialize_options(&[IpcpOption {
                opt_type: IPCP_OPT_IP_COMPRESSION,
                data: suggested.to_ipcp_data().to_vec(),
            }]),
        };
        let responses = session.receive(&nak.to_ppp());
        assert_eq!(responses.len(), 1);
        let retry = IpcpPacket::parse(&responses[0].payload).unwrap();
        let vj = parse_options(&retry.data)
            .into_iter()
            .find(|opt| opt.opt_type == IPCP_OPT_IP_COMPRESSION)
            .expect("retry should include adopted VJ option");
        assert_eq!(vj.data, suggested.to_ipcp_data().to_vec());
    }

    #[test]
    fn vj_option_is_formatted_without_ipv4_mislabel() {
        let data = serialize_options(&[IpcpOption {
            opt_type: IPCP_OPT_IP_COMPRESSION,
            data: vec![0x00, 0x2d, 0x0f, 0x01],
        }]);
        assert_eq!(
            format_ipcp_options(&data),
            "IPCompression(protocol=0x002d,max_slot_id=15,comp_slot_id=1)"
        );
    }

    #[test]
    fn configure_reject_removes_our_ip_option() {
        let mut session = IpcpSession::new(local_vj_config());
        let our_req = session.start();
        let our_ipcp = IpcpPacket::parse(&our_req.payload).unwrap();

        let reject = IpcpPacket {
            code: CONFIGURE_REJECT,
            identifier: our_ipcp.identifier,
            data: serialize_options(&[IpcpOption {
                opt_type: OPT_IP_ADDRESS,
                data: vec![10, 0, 0, 1],
            }]),
        };
        let responses = session.receive(&reject.to_ppp());
        assert_eq!(responses.len(), 1);
        let retry = IpcpPacket::parse(&responses[0].payload).unwrap();
        assert_eq!(retry.code, CONFIGURE_REQUEST);
        let retry_opts = parse_options(&retry.data);
        assert!(!retry_opts.iter().any(|opt| opt.opt_type == OPT_IP_ADDRESS));
        assert!(
            retry_opts
                .iter()
                .any(|opt| opt.opt_type == IPCP_OPT_IP_COMPRESSION)
        );
    }

    #[test]
    fn configure_request_retransmits_until_acked() {
        let mut session = IpcpSession::new(IpcpConfig::default());
        let first = session.start();
        let first_ipcp = IpcpPacket::parse(&first.payload).unwrap();

        for _ in 0..CONFIGURE_RESTART_TICKS {
            assert!(session.maybe_retransmit_configure_request().is_none());
        }

        let retransmit = session
            .maybe_retransmit_configure_request()
            .expect("pending IPCP request should retransmit");
        let retransmit_ipcp = IpcpPacket::parse(&retransmit.payload).unwrap();
        assert_eq!(retransmit_ipcp.identifier, first_ipcp.identifier);
        assert_eq!(retransmit_ipcp.data, first_ipcp.data);
    }

    #[test]
    fn configure_request_fails_after_max_restarts() {
        let mut session = IpcpSession::new(IpcpConfig::default());
        session.start();

        for _ in 0..MAX_CONFIGURE_RESTARTS {
            for _ in 0..CONFIGURE_RESTART_TICKS {
                assert!(session.maybe_retransmit_configure_request().is_none());
            }
            assert!(session.maybe_retransmit_configure_request().is_some());
        }

        for _ in 0..CONFIGURE_RESTART_TICKS {
            assert!(session.maybe_retransmit_configure_request().is_none());
        }
        assert!(session.maybe_retransmit_configure_request().is_none());
        assert!(session.configure_failed());
        assert_eq!(session.state, IpcpState::Closed);
    }

    #[test]
    fn invalid_configure_ack_is_discarded() {
        let mut session = IpcpSession::new(IpcpConfig::default());
        let first = session.start();
        let first_ipcp = IpcpPacket::parse(&first.payload).unwrap();

        let wrong_id = IpcpPacket {
            code: CONFIGURE_ACK,
            identifier: first_ipcp.identifier.wrapping_add(1),
            data: first_ipcp.data.clone(),
        };
        assert!(session.receive(&wrong_id.to_ppp()).is_empty());
        assert_eq!(session.state, IpcpState::RequestSent);

        let wrong_options = IpcpPacket {
            code: CONFIGURE_ACK,
            identifier: first_ipcp.identifier,
            data: vec![],
        };
        assert!(session.receive(&wrong_options.to_ppp()).is_empty());
        assert_eq!(session.state, IpcpState::RequestSent);
    }

    #[test]
    fn malformed_packet_ignored() {
        let mut session = IpcpSession::new(IpcpConfig::default());
        session.start();
        let bad = PppPacket {
            protocol: IPCP_PROTOCOL,
            payload: vec![0x01, 0x01], // too short
        };
        let responses = session.receive(&bad);
        assert!(responses.is_empty());
    }

    #[test]
    fn max_failure_converts_peer_ip_nak_to_reject() {
        // RFC 1661 §4.6: after MAX_FAILURE Configure-Naks for a
        // peer-requested option without an intervening Configure-Ack, the
        // option must be Configure-Rejected on the next round.
        let mut session = IpcpSession::new(IpcpConfig::default());
        session.start();

        for id in 1..=MAX_FAILURE {
            let mobile_req = IpcpPacket {
                code: CONFIGURE_REQUEST,
                identifier: id as u8,
                data: serialize_options(&[IpcpOption {
                    opt_type: OPT_IP_ADDRESS,
                    data: vec![10, 0, 0, 99], // not our peer_ip
                }]),
            };
            let responses = session.receive(&mobile_req.to_ppp());
            assert_eq!(responses.len(), 1);
            let resp = IpcpPacket::parse(&responses[0].payload).unwrap();
            assert_eq!(resp.code, CONFIGURE_NAK);
        }

        // MAX_FAILURE+1: peer still proposes the same bad IP — we Reject.
        let mobile_req = IpcpPacket {
            code: CONFIGURE_REQUEST,
            identifier: (MAX_FAILURE + 1) as u8,
            data: serialize_options(&[IpcpOption {
                opt_type: OPT_IP_ADDRESS,
                data: vec![10, 0, 0, 99],
            }]),
        };
        let responses = session.receive(&mobile_req.to_ppp());
        assert_eq!(responses.len(), 1);
        let resp = IpcpPacket::parse(&responses[0].payload).unwrap();
        assert_eq!(resp.code, CONFIGURE_REJECT);
        let reject_opts = parse_options(&resp.data);
        assert_eq!(reject_opts.len(), 1);
        assert_eq!(reject_opts[0].opt_type, OPT_IP_ADDRESS);
        // The Rejected option echoes the peer's original value (RFC §5.4).
        assert_eq!(
            bytes_to_ip(&reject_opts[0].data),
            Some(Ipv4Addr::new(10, 0, 0, 99))
        );
    }

    #[test]
    fn configure_ack_clears_max_failure_counter() {
        let mut session = IpcpSession::new(IpcpConfig::default());
        session.start();

        // Two NAKs for IP-Address, then an ACKable request — counter resets.
        for id in 1..=2 {
            let req = IpcpPacket {
                code: CONFIGURE_REQUEST,
                identifier: id,
                data: serialize_options(&[IpcpOption {
                    opt_type: OPT_IP_ADDRESS,
                    data: vec![0, 0, 0, 0],
                }]),
            };
            session.receive(&req.to_ppp());
        }
        assert_eq!(session.omitted_peer_ip_naks(), 2);

        let good_req = IpcpPacket {
            code: CONFIGURE_REQUEST,
            identifier: 3,
            data: serialize_options(&[IpcpOption {
                opt_type: OPT_IP_ADDRESS,
                data: vec![10, 0, 0, 2],
            }]),
        };
        session.receive(&good_req.to_ppp());
        assert_eq!(session.omitted_peer_ip_naks(), 0);
    }

    #[test]
    fn terminate_request_acked_and_state_closed() {
        // RFC 1661 §5.5: on Terminate-Request, MUST send Terminate-Ack and
        // close down our side.
        let mut session = IpcpSession::new(IpcpConfig::default());
        session.start();
        // Get to Opened.
        let req = IpcpPacket {
            code: CONFIGURE_REQUEST,
            identifier: 1,
            data: serialize_options(&[IpcpOption {
                opt_type: OPT_IP_ADDRESS,
                data: vec![10, 0, 0, 2],
            }]),
        };
        session.receive(&req.to_ppp());
        ack_last_request(&mut session);
        assert!(session.is_open());

        let term = IpcpPacket {
            code: TERMINATE_REQUEST,
            identifier: 42,
            data: b"bye".to_vec(),
        };
        let responses = session.receive(&term.to_ppp());
        assert_eq!(responses.len(), 1);
        let resp = IpcpPacket::parse(&responses[0].payload).unwrap();
        assert_eq!(resp.code, TERMINATE_ACK);
        assert_eq!(resp.identifier, 42);
        assert_eq!(resp.data, Vec::<u8>::new());
        assert_eq!(session.state, IpcpState::Closed);
    }

    #[test]
    fn code_reject_emitted_for_unknown_code() {
        // RFC 1661 §5.6: unknown code MUST elicit a Code-Reject.
        let mut session = IpcpSession::new(IpcpConfig::default());
        session.start();
        let unknown = IpcpPacket {
            code: 99,
            identifier: 5,
            data: vec![0xaa, 0xbb, 0xcc],
        };
        let responses = session.receive(&unknown.to_ppp());
        assert_eq!(responses.len(), 1);
        let resp = IpcpPacket::parse(&responses[0].payload).unwrap();
        assert_eq!(resp.code, CODE_REJECT);
        // Rejected-Packet field is the offending packet bytes.
        assert_eq!(&resp.data, &unknown.to_bytes());
    }

    #[test]
    fn opened_state_renegotiates_on_new_configure_request() {
        // RFC 1661 §4.3: RCR in Opened triggers a fresh Configure-Request
        // from us before evaluating the peer's request.
        let mut session = IpcpSession::new(IpcpConfig::default());
        session.start();
        // Drive to Opened.
        let req = IpcpPacket {
            code: CONFIGURE_REQUEST,
            identifier: 1,
            data: serialize_options(&[IpcpOption {
                opt_type: OPT_IP_ADDRESS,
                data: vec![10, 0, 0, 2],
            }]),
        };
        session.receive(&req.to_ppp());
        ack_last_request(&mut session);
        assert!(session.is_open());

        // Peer sends a brand-new Configure-Request.
        let renegotiate = IpcpPacket {
            code: CONFIGURE_REQUEST,
            identifier: 10,
            data: serialize_options(&[
                IpcpOption {
                    opt_type: OPT_IP_ADDRESS,
                    data: vec![10, 0, 0, 2],
                },
                IpcpOption {
                    opt_type: OPT_PRIMARY_DNS,
                    data: vec![10, 55, 0, 1],
                },
            ]),
        };
        let responses = session.receive(&renegotiate.to_ppp());
        // Expect a fresh Configure-Request from us AND a Configure-Ack for theirs.
        assert_eq!(responses.len(), 2);
        let our_req = IpcpPacket::parse(&responses[0].payload).unwrap();
        assert_eq!(our_req.code, CONFIGURE_REQUEST);
        let their_ack = IpcpPacket::parse(&responses[1].payload).unwrap();
        assert_eq!(their_ack.code, CONFIGURE_ACK);
        assert_eq!(their_ack.identifier, 10);
        assert_eq!(session.state, IpcpState::AckSent);
    }

    #[test]
    fn duplicate_peer_configure_request_after_opened_is_acked_without_restart() {
        let mut session = IpcpSession::new(IpcpConfig::default());
        session.start();

        let req = IpcpPacket {
            code: CONFIGURE_REQUEST,
            identifier: 1,
            data: serialize_options(&[IpcpOption {
                opt_type: OPT_IP_ADDRESS,
                data: vec![10, 0, 0, 2],
            }]),
        };
        session.receive(&req.to_ppp());
        ack_last_request(&mut session);
        assert!(session.is_open());

        let duplicate = IpcpPacket {
            code: CONFIGURE_REQUEST,
            identifier: 2,
            data: req.data.clone(),
        };
        let responses = session.receive(&duplicate.to_ppp());
        assert_eq!(responses.len(), 1);
        let resp = IpcpPacket::parse(&responses[0].payload).unwrap();
        assert_eq!(resp.code, CONFIGURE_ACK);
        assert_eq!(resp.identifier, 2);
        assert_eq!(session.state, IpcpState::Opened);
    }
}
