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
use std::net::Ipv4Addr;

pub const IPCP_PROTOCOL: u16 = 0x8021;

const CONFIGURE_REQUEST: u8 = 1;
const CONFIGURE_ACK: u8 = 2;
const CONFIGURE_NAK: u8 = 3;

/// PPP restart timer in packet-session ticks. Packet sessions tick every 20 ms,
/// so this retransmits pending Configure-Requests once per second.
const CONFIGURE_RESTART_TICKS: u16 = 50;

// IPCP option types.
const OPT_IP_ADDRESS: u8 = 3;
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
}

impl Default for IpcpConfig {
    fn default() -> Self {
        Self {
            our_ip: Ipv4Addr::new(10, 0, 0, 1),
            peer_ip: Ipv4Addr::new(10, 0, 0, 2),
            primary_dns: Ipv4Addr::new(8, 8, 8, 8),
            secondary_dns: Ipv4Addr::new(8, 8, 4, 4),
        }
    }
}

/// Number of times we'll NAK with an appended IP before giving up and
/// accepting a request without one (for phones that ignore appended options).
const IP_NAK_APPEND_MAX: u8 = 3;

/// BS/MSC-side IPCP state machine.
#[derive(Debug)]
pub struct IpcpSession {
    pub state: IpcpState,
    pub config: IpcpConfig,
    next_id: u8,
    peer_acked: bool,
    we_acked: bool,
    last_request_id: Option<u8>,
    last_request_data: Vec<u8>,
    restart_ticks_remaining: u16,
    configure_restarts: u32,
    /// How many times we've NAK'd with an appended IP-Address option
    /// when the peer didn't include one.
    ip_nak_append_count: u8,
}

impl IpcpSession {
    pub fn new(config: IpcpConfig) -> Self {
        Self {
            state: IpcpState::Closed,
            config,
            next_id: 1,
            peer_acked: false,
            we_acked: false,
            last_request_id: None,
            last_request_data: Vec::new(),
            restart_ticks_remaining: 0,
            configure_restarts: 0,
            ip_nak_append_count: 0,
        }
    }

    /// Generate our initial Configure-Request with our gateway IP.
    pub fn start(&mut self) -> PppPacket {
        let data = serialize_options(&[IpcpOption {
            opt_type: OPT_IP_ADDRESS,
            data: ip_to_bytes(self.config.our_ip),
        }]);
        self.send_configure_request(data)
    }

    fn send_configure_request(&mut self, data: Vec<u8>) -> PppPacket {
        log::info!("IPCP TX: Configure-Request (our_ip={})", self.config.our_ip);
        let id = self.alloc_id();
        let pkt = IpcpPacket {
            code: CONFIGURE_REQUEST,
            identifier: id,
            data: data.clone(),
        };
        self.last_request_id = Some(id);
        self.last_request_data = data;
        self.restart_ticks_remaining = CONFIGURE_RESTART_TICKS;
        self.state = IpcpState::RequestSent;
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
        self.restart_ticks_remaining = CONFIGURE_RESTART_TICKS;
        log::info!(
            "IPCP TX: Configure-Request retransmit id={} restart_count={}",
            id,
            self.configure_restarts
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
                    "IPCP RX: Configure-Request id={} opts=[{}]",
                    ipcp.identifier,
                    format_ipcp_options(&ipcp.data)
                );
                let opts = parse_options(&ipcp.data);
                let mut nak_opts = Vec::new();

                for opt in &opts {
                    match opt.opt_type {
                        OPT_IP_ADDRESS => {
                            let requested_ip = bytes_to_ip(&opt.data);
                            if requested_ip == Some(self.config.peer_ip) {
                                // They're requesting the correct IP — will ACK.
                            } else {
                                // NAK with the IP we want to assign.
                                nak_opts.push(IpcpOption {
                                    opt_type: OPT_IP_ADDRESS,
                                    data: ip_to_bytes(self.config.peer_ip),
                                });
                            }
                        }
                        OPT_PRIMARY_DNS => {
                            let requested_dns = bytes_to_ip(&opt.data);
                            if requested_dns != Some(self.config.primary_dns) {
                                nak_opts.push(IpcpOption {
                                    opt_type: OPT_PRIMARY_DNS,
                                    data: ip_to_bytes(self.config.primary_dns),
                                });
                            }
                        }
                        OPT_SECONDARY_DNS => {
                            let requested_dns = bytes_to_ip(&opt.data);
                            if requested_dns != Some(self.config.secondary_dns) {
                                nak_opts.push(IpcpOption {
                                    opt_type: OPT_SECONDARY_DNS,
                                    data: ip_to_bytes(self.config.secondary_dns),
                                });
                            }
                        }
                        _ => {
                            // Unknown options — accept for MVP.
                        }
                    }
                }

                // RFC 1661 §5.3: If the peer didn't include IP-Address and
                // we haven't exceeded our append limit, append it to the NAK
                // to inform the peer it should negotiate an IP.  Some phones
                // (e.g. Samsung) ignore appended options, so we give up after
                // IP_NAK_APPEND_MAX attempts and accept without IP.
                let has_ip_opt = opts.iter().any(|o| o.opt_type == OPT_IP_ADDRESS);
                if !has_ip_opt && self.ip_nak_append_count < IP_NAK_APPEND_MAX {
                    self.ip_nak_append_count += 1;
                    log::info!(
                        "IPCP: peer omitted IP-Address, appending to NAK (attempt {}/{})",
                        self.ip_nak_append_count,
                        IP_NAK_APPEND_MAX
                    );
                    nak_opts.push(IpcpOption {
                        opt_type: OPT_IP_ADDRESS,
                        data: ip_to_bytes(self.config.peer_ip),
                    });
                }

                if nak_opts.is_empty() {
                    // ACK — all options are acceptable.
                    log::info!("IPCP TX: Configure-Ack id={}", ipcp.identifier);
                    let ack = IpcpPacket {
                        code: CONFIGURE_ACK,
                        identifier: ipcp.identifier,
                        data: ipcp.data.clone(),
                    };
                    responses.push(ack.to_ppp());
                    self.we_acked = true;
                } else {
                    // NAK — suggest correct values.
                    log::info!(
                        "IPCP TX: Configure-Nak id={} opts=[{}]",
                        ipcp.identifier,
                        format_ipcp_options(&serialize_options(&nak_opts))
                    );
                    let nak = IpcpPacket {
                        code: CONFIGURE_NAK,
                        identifier: ipcp.identifier,
                        data: serialize_options(&nak_opts),
                    };
                    responses.push(nak.to_ppp());
                }

                self.update_state();
            }
            CONFIGURE_ACK => {
                if self.last_request_id != Some(ipcp.identifier)
                    || ipcp.data != self.last_request_data
                {
                    log::debug!(
                        "IPCP RX: invalid Configure-Ack id={} expected_id={:?} opts=[{}], discarding",
                        ipcp.identifier,
                        self.last_request_id,
                        format_ipcp_options(&ipcp.data)
                    );
                    return responses;
                }
                log::info!("IPCP RX: Configure-Ack id={}", ipcp.identifier);
                self.peer_acked = true;
                self.restart_ticks_remaining = 0;
                self.update_state();
            }
            CONFIGURE_NAK => {
                if self.last_request_id != Some(ipcp.identifier) {
                    log::debug!(
                        "IPCP RX: Configure-Nak id={} does not match last request {:?}, discarding",
                        ipcp.identifier,
                        self.last_request_id
                    );
                    return responses;
                }
                log::info!(
                    "IPCP RX: Configure-Nak id={} opts=[{}]",
                    ipcp.identifier,
                    format_ipcp_options(&ipcp.data)
                );
                // Peer suggests different values for our options. Adopt and retry.
                let opts = parse_options(&ipcp.data);
                for opt in &opts {
                    if opt.opt_type == OPT_IP_ADDRESS {
                        if let Some(ip) = bytes_to_ip(&opt.data) {
                            self.config.our_ip = ip;
                        }
                    }
                }
                // Resend Configure-Request with updated values.
                let data = serialize_options(&[IpcpOption {
                    opt_type: OPT_IP_ADDRESS,
                    data: ip_to_bytes(self.config.our_ip),
                }]);
                self.peer_acked = false;
                responses.push(self.send_configure_request(data));
                self.update_state();
            }
            _ => {
                log::debug!("IPCP: ignoring code {}", ipcp.code);
            }
        }

        responses
    }

    /// Returns true when IPCP is open (both sides acked).
    pub fn is_open(&self) -> bool {
        self.state == IpcpState::Opened
    }

    /// Returns the IP assigned to the mobile peer.
    pub fn peer_ip(&self) -> Ipv4Addr {
        self.config.peer_ip
    }

    /// Returns our gateway IP.
    pub fn our_ip(&self) -> Ipv4Addr {
        self.config.our_ip
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

/// Format IPCP options for logging.
fn format_ipcp_options(data: &[u8]) -> String {
    let opts = parse_options(data);
    opts.iter()
        .map(|o| {
            let name = match o.opt_type {
                OPT_IP_ADDRESS => "IP",
                OPT_PRIMARY_DNS => "PrimaryDNS",
                OPT_SECONDARY_DNS => "SecondaryDNS",
                _ => "Unknown",
            };
            if o.data.len() == 4 {
                let ip = Ipv4Addr::new(o.data[0], o.data[1], o.data[2], o.data[3]);
                format!("{}={}", name, ip)
            } else {
                format!("{}(type={} len={})", name, o.opt_type, o.data.len())
            }
        })
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod tests {
    use super::*;

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
            Some(Ipv4Addr::new(8, 8, 8, 8))
        );
        // Secondary DNS
        assert_eq!(nak_opts[2].opt_type, OPT_SECONDARY_DNS);
        assert_eq!(
            bytes_to_ip(&nak_opts[2].data),
            Some(Ipv4Addr::new(8, 8, 4, 4))
        );
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
                data: vec![8, 8, 8, 8],
            },
            IpcpOption {
                opt_type: OPT_SECONDARY_DNS,
                data: vec![8, 8, 4, 4],
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
    fn our_ip_nak_causes_retry() {
        let mut session = IpcpSession::new(IpcpConfig::default());
        session.start();

        // Mobile NAKs our IP, suggests 10.0.0.100.
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
        assert_eq!(
            bytes_to_ip(&opts[0].data),
            Some(Ipv4Addr::new(10, 0, 0, 100))
        );
        // Our IP should be updated.
        assert_eq!(session.our_ip(), Ipv4Addr::new(10, 0, 0, 100));
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
}
