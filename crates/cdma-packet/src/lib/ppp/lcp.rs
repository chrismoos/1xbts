/// LCP (Link Control Protocol) per RFC 1661.
///
/// BS/MSC-side implementation that properly negotiates PPP link options
/// including ACCM, PFC, ACFC, MRU, and Magic-Number.
///
/// Protocol number: 0xC021
///
/// Supported codes:
/// - Configure-Request (1), Configure-Ack (2), Configure-Nak (3), Configure-Reject (4)
/// - Echo-Request (9) → respond with Echo-Reply (10)
/// - Terminate-Request (5) → respond with Terminate-Ack (6)
use super::framing::PppPacket;

pub const LCP_PROTOCOL: u16 = 0xC021;

// LCP code values per RFC 1661 Section 5.
const CONFIGURE_REQUEST: u8 = 1;
const CONFIGURE_ACK: u8 = 2;
const CONFIGURE_NAK: u8 = 3;
const CONFIGURE_REJECT: u8 = 4;
const TERMINATE_REQUEST: u8 = 5;
const TERMINATE_ACK: u8 = 6;
const ECHO_REQUEST: u8 = 9;
const ECHO_REPLY: u8 = 10;
const DISCARD_REQUEST: u8 = 11;

/// PPP restart timer in packet-session ticks. Packet sessions tick every 20 ms,
/// so this retransmits pending Configure-Requests once per second.
const CONFIGURE_RESTART_TICKS: u16 = 50;

// LCP option types.
const OPT_MRU: u8 = 1;
const OPT_ACCM: u8 = 2;
const OPT_MAGIC_NUMBER: u8 = 5;
const OPT_PFC: u8 = 7;
const OPT_ACFC: u8 = 8;

/// Negotiated PPP framing options per direction.
///
/// RFC 1661 semantics: when the peer sends option X in its Configure-Request
/// and we ACK it, we are agreeing to *receive* frames with that compression.
/// The peer is then allowed to *send* compressed frames to us.
///
/// Conversely, if we send option X in our Configure-Request and the peer ACKs,
/// the peer agrees to receive our compressed frames.
#[derive(Debug, Clone, Copy)]
pub struct NegotiatedOptions {
    /// Peer may send us compressed protocol fields (1 byte instead of 2).
    /// Set when we ACK their PFC option.
    pub peer_sends_pfc: bool,
    /// Peer may send us compressed address/control (omit FF 03).
    /// Set when we ACK their ACFC option.
    pub peer_sends_acfc: bool,
    /// We may send compressed protocol fields to the peer.
    /// Set when the peer ACKs our PFC option (currently we don't request PFC).
    pub we_send_pfc: bool,
    /// We may send compressed address/control to the peer.
    /// Set when the peer ACKs our ACFC option (currently we don't request ACFC).
    pub we_send_acfc: bool,
    /// ACCM we use when sending to the peer.
    /// This is the value the peer requested (and we ACKed) in their Configure-Request.
    /// Default: 0xFFFFFFFF (escape all 0x00-0x1F).
    pub tx_accm: u32,
    /// ACCM the peer uses when sending to us.
    /// This is the value we requested (and they ACKed) in our Configure-Request.
    /// Default: 0xFFFFFFFF.
    pub rx_accm: u32,
}

impl Default for NegotiatedOptions {
    fn default() -> Self {
        Self {
            peer_sends_pfc: false,
            peer_sends_acfc: false,
            we_send_pfc: false,
            we_send_acfc: false,
            tx_accm: 0xFFFFFFFF,
            rx_accm: 0xFFFFFFFF,
        }
    }
}

/// LCP negotiation state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LcpState {
    /// Not started — waiting to send/receive Configure-Request.
    Closed,
    /// We sent Configure-Request, waiting for Ack.
    RequestSent,
    /// We received their Ack but haven't acked theirs yet.
    AckReceived,
    /// We acked theirs but haven't received our Ack yet.
    AckSent,
    /// Both sides acked — link is open.
    Opened,
}

/// An LCP packet (parsed from PPP payload).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LcpPacket {
    pub code: u8,
    pub identifier: u8,
    pub data: Vec<u8>,
}

impl LcpPacket {
    /// Parse from raw PPP payload bytes.
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
        Some(LcpPacket {
            code,
            identifier,
            data,
        })
    }

    /// Serialize to bytes.
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

    /// Wrap into a PPP packet.
    pub fn to_ppp(&self) -> PppPacket {
        PppPacket {
            protocol: LCP_PROTOCOL,
            payload: self.to_bytes(),
        }
    }
}

/// An LCP option (type-length-value).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LcpOption {
    pub opt_type: u8,
    pub data: Vec<u8>, // value bytes (excludes type and length fields)
}

/// Parse options from the data field of a Configure-Request/Ack/Nak/Reject.
fn parse_options(data: &[u8]) -> Vec<LcpOption> {
    let mut opts = Vec::new();
    let mut i = 0;
    while i + 1 < data.len() {
        let opt_type = data[i];
        let opt_len = data[i + 1] as usize;
        if opt_len < 2 || i + opt_len > data.len() {
            break;
        }
        opts.push(LcpOption {
            opt_type,
            data: data[i + 2..i + opt_len].to_vec(),
        });
        i += opt_len;
    }
    opts
}

/// Serialize options back to bytes.
fn serialize_options(opts: &[LcpOption]) -> Vec<u8> {
    let mut out = Vec::new();
    for opt in opts {
        out.push(opt.opt_type);
        out.push((2 + opt.data.len()) as u8);
        out.extend_from_slice(&opt.data);
    }
    out
}

/// BS/MSC-side LCP state machine.
#[derive(Debug)]
pub struct LcpSession {
    pub state: LcpState,
    next_id: u8,
    our_mru: u16,
    peer_acked: bool,
    we_acked: bool,
    last_request_id: Option<u8>,
    last_request_data: Vec<u8>,
    restart_ticks_remaining: u16,
    configure_restarts: u32,
    /// Negotiated options — valid once LCP is Opened.
    pub negotiated: NegotiatedOptions,
    /// Echo keepalive: identifier of the last Echo-Request we sent.
    /// `None` if no outstanding echo.
    echo_pending_id: Option<u8>,
    /// Number of consecutive Echo-Requests sent without a reply.
    echo_failures: u8,
}

/// LCP Echo keepalive configuration.
pub const ECHO_MAX_FAILURES: u8 = 3;

impl LcpSession {
    pub fn new() -> Self {
        Self {
            state: LcpState::Closed,
            next_id: 1,
            our_mru: 1500,
            peer_acked: false,
            we_acked: false,
            last_request_id: None,
            last_request_data: Vec::new(),
            restart_ticks_remaining: 0,
            configure_restarts: 0,
            negotiated: NegotiatedOptions::default(),
            echo_pending_id: None,
            echo_failures: 0,
        }
    }

    /// Build our Configure-Request options.
    fn our_request_options(&self) -> Vec<LcpOption> {
        vec![LcpOption {
            opt_type: OPT_MRU,
            data: vec![(self.our_mru >> 8) as u8, (self.our_mru & 0xFF) as u8],
        }]
    }

    /// Generate our initial Configure-Request. Call once to start negotiation.
    pub fn start(&mut self) -> PppPacket {
        let data = serialize_options(&self.our_request_options());
        self.send_configure_request(data)
    }

    fn send_configure_request(&mut self, data: Vec<u8>) -> PppPacket {
        log::info!("LCP TX: Configure-Request (MRU={})", self.our_mru);
        let id = self.alloc_id();
        let pkt = LcpPacket {
            code: CONFIGURE_REQUEST,
            identifier: id,
            data: data.clone(),
        };
        self.last_request_id = Some(id);
        self.last_request_data = data;
        self.restart_ticks_remaining = CONFIGURE_RESTART_TICKS;
        self.state = LcpState::RequestSent;
        pkt.to_ppp()
    }

    /// Advance the Configure-Request restart timer and retransmit if needed.
    pub fn maybe_retransmit_configure_request(&mut self) -> Option<PppPacket> {
        if self.state == LcpState::Closed
            || self.state == LcpState::Opened
            || self.state == LcpState::AckReceived
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
            "LCP TX: Configure-Request retransmit id={} restart_count={}",
            id,
            self.configure_restarts
        );
        Some(
            LcpPacket {
                code: CONFIGURE_REQUEST,
                identifier: id,
                data: self.last_request_data.clone(),
            }
            .to_ppp(),
        )
    }

    /// Process an incoming LCP packet. Returns zero or more PPP packets to send.
    pub fn receive(&mut self, ppp: &PppPacket) -> Vec<PppPacket> {
        let lcp = match LcpPacket::parse(&ppp.payload) {
            Some(p) => p,
            None => return vec![],
        };

        let mut responses = Vec::new();

        match lcp.code {
            CONFIGURE_REQUEST => {
                log::info!(
                    "LCP RX: Configure-Request id={} opts=[{}]",
                    lcp.identifier,
                    format_lcp_options(&lcp.data)
                );

                // RFC 1661 §3.6: If we're in Opened state and receive a
                // Configure-Request, the peer is restarting LCP.  We must:
                //   1. Signal This-Layer-Down (reset upper layers)
                //   2. Send our own Configure-Request (scr)
                //   3. Send Configure-Ack for theirs (sca)
                //   4. Transition to AckSent
                let restarting = self.state == LcpState::Opened;
                if restarting {
                    log::info!("LCP: peer restarted negotiation, re-sending Configure-Request");
                    self.peer_acked = false;
                    self.we_acked = false;
                    self.last_request_id = None;
                    self.last_request_data.clear();
                    self.restart_ticks_remaining = 0;
                    self.negotiated = NegotiatedOptions::default();
                    self.state = LcpState::RequestSent;

                    // Send our Configure-Request first.
                    responses.push(self.start());
                }

                // Evaluate each peer option: Ack, Nak, or Reject.
                let opts = parse_options(&lcp.data);
                let mut ack_opts = Vec::new();
                let mut reject_opts = Vec::new();

                for opt in &opts {
                    match opt.opt_type {
                        OPT_MRU => {
                            // Accept any MRU the peer wants.
                            ack_opts.push(opt.clone());
                        }
                        OPT_ACCM => {
                            // Accept peer's ACCM — this tells us what ACCM to
                            // use when we send TO them.  We store it and apply
                            // it on the TX side after LCP opens.
                            ack_opts.push(opt.clone());
                        }
                        OPT_MAGIC_NUMBER => {
                            // Accept magic number for loop detection.
                            ack_opts.push(opt.clone());
                        }
                        OPT_PFC => {
                            // Peer wants to send us compressed protocol fields.
                            // We can receive both compressed and uncompressed,
                            // so accept.
                            ack_opts.push(opt.clone());
                        }
                        OPT_ACFC => {
                            // Peer wants to send us compressed address/control.
                            // We can receive both, so accept.
                            ack_opts.push(opt.clone());
                        }
                        _ => {
                            // Unknown/unsupported option — reject per RFC 1661.
                            log::info!(
                                "LCP: rejecting unknown option type={} len={}",
                                opt.opt_type,
                                opt.data.len()
                            );
                            reject_opts.push(opt.clone());
                        }
                    }
                }

                if !reject_opts.is_empty() {
                    // Send Configure-Reject with unsupported options.
                    let reject = LcpPacket {
                        code: CONFIGURE_REJECT,
                        identifier: lcp.identifier,
                        data: serialize_options(&reject_opts),
                    };
                    log::info!(
                        "LCP TX: Configure-Reject id={} opts=[{}]",
                        lcp.identifier,
                        format_lcp_options(&reject.data)
                    );
                    responses.push(reject.to_ppp());
                    // Don't set we_acked — peer must retry without rejected options.
                } else {
                    // All options acceptable — send Configure-Ack.
                    let ack = LcpPacket {
                        code: CONFIGURE_ACK,
                        identifier: lcp.identifier,
                        data: lcp.data.clone(),
                    };
                    log::info!("LCP TX: Configure-Ack id={}", lcp.identifier);
                    responses.push(ack.to_ppp());

                    // Record negotiated options from the peer's request.
                    // These take effect when LCP reaches Opened.
                    for opt in &opts {
                        match opt.opt_type {
                            OPT_ACCM if opt.data.len() == 4 => {
                                self.negotiated.tx_accm = u32::from_be_bytes([
                                    opt.data[0],
                                    opt.data[1],
                                    opt.data[2],
                                    opt.data[3],
                                ]);
                            }
                            OPT_PFC => {
                                self.negotiated.peer_sends_pfc = true;
                            }
                            OPT_ACFC => {
                                self.negotiated.peer_sends_acfc = true;
                            }
                            _ => {}
                        }
                    }

                    self.we_acked = true;
                }
                self.update_state();
            }
            CONFIGURE_ACK => {
                if self.last_request_id != Some(lcp.identifier)
                    || lcp.data != self.last_request_data
                {
                    log::debug!(
                        "LCP RX: invalid Configure-Ack id={} expected_id={:?} opts=[{}], discarding",
                        lcp.identifier,
                        self.last_request_id,
                        format_lcp_options(&lcp.data)
                    );
                    return responses;
                }
                log::info!("LCP RX: Configure-Ack id={}", lcp.identifier);
                self.peer_acked = true;
                self.restart_ticks_remaining = 0;
                self.update_state();
            }
            CONFIGURE_NAK => {
                if self.last_request_id != Some(lcp.identifier) {
                    log::debug!(
                        "LCP RX: Configure-Nak id={} does not match last request {:?}, discarding",
                        lcp.identifier,
                        self.last_request_id
                    );
                    return responses;
                }
                log::info!(
                    "LCP RX: Configure-Nak id={} opts=[{}]",
                    lcp.identifier,
                    format_lcp_options(&lcp.data)
                );
                // Peer is suggesting different values. Parse their suggestions and
                // resend Configure-Request with their preferred values.
                let opts = parse_options(&lcp.data);
                let new_data = serialize_options(&opts);
                self.peer_acked = false;
                responses.push(self.send_configure_request(new_data));
                self.update_state();
            }
            CONFIGURE_REJECT => {
                if self.last_request_id != Some(lcp.identifier) {
                    log::debug!(
                        "LCP RX: Configure-Reject id={} does not match last request {:?}, discarding",
                        lcp.identifier,
                        self.last_request_id
                    );
                    return responses;
                }
                log::info!(
                    "LCP RX: Configure-Reject id={} opts=[{}]",
                    lcp.identifier,
                    format_lcp_options(&lcp.data)
                );
                // Peer rejects some of our options. Resend without the rejected options.
                let rejected = parse_options(&lcp.data);
                let rejected_types: Vec<u8> = rejected.iter().map(|o| o.opt_type).collect();

                // Rebuild our request without rejected options.
                let kept: Vec<LcpOption> = self
                    .our_request_options()
                    .into_iter()
                    .filter(|o| !rejected_types.contains(&o.opt_type))
                    .collect();

                self.peer_acked = false;
                responses.push(self.send_configure_request(serialize_options(&kept)));
                self.update_state();
            }
            ECHO_REQUEST => {
                log::info!("LCP RX: Echo-Request id={}", lcp.identifier);
                // Reply with Echo-Reply, same identifier, our magic number (0) + their data.
                let mut reply_data = vec![0x00, 0x00, 0x00, 0x00]; // magic number = 0
                if lcp.data.len() > 4 {
                    reply_data.extend_from_slice(&lcp.data[4..]);
                }
                let reply = LcpPacket {
                    code: ECHO_REPLY,
                    identifier: lcp.identifier,
                    data: reply_data,
                };
                responses.push(reply.to_ppp());
            }
            ECHO_REPLY => {
                // Response to our Echo-Request keepalive.
                if self.echo_pending_id == Some(lcp.identifier) {
                    log::debug!("LCP RX: Echo-Reply id={} (keepalive ok)", lcp.identifier);
                    self.echo_pending_id = None;
                    self.echo_failures = 0;
                } else {
                    log::debug!(
                        "LCP RX: Echo-Reply id={} (unexpected, pending={:?})",
                        lcp.identifier,
                        self.echo_pending_id
                    );
                }
            }
            TERMINATE_REQUEST => {
                log::info!("LCP RX: Terminate-Request id={}", lcp.identifier);
                let ack = LcpPacket {
                    code: TERMINATE_ACK,
                    identifier: lcp.identifier,
                    data: vec![],
                };
                responses.push(ack.to_ppp());
                self.state = LcpState::Closed;
                self.peer_acked = false;
                self.we_acked = false;
                self.last_request_id = None;
                self.last_request_data.clear();
                self.restart_ticks_remaining = 0;
                self.negotiated = NegotiatedOptions::default();
            }
            DISCARD_REQUEST => {
                // Per RFC 1661 §5.9: silently discard. No response required.
                log::debug!("LCP RX: Discard-Request id={}", lcp.identifier);
            }
            _ => {
                let hex: String = lcp.data.iter().map(|b| format!("{:02x}", b)).collect();
                log::info!(
                    "LCP RX: unknown code={} id={} len={} data=[{}]",
                    lcp.code,
                    lcp.identifier,
                    lcp.data.len() + 4,
                    hex
                );
            }
        }

        responses
    }

    /// Returns true when LCP negotiation is complete and the link is open.
    pub fn is_open(&self) -> bool {
        self.state == LcpState::Opened
    }

    /// Generate an Echo-Request keepalive if the link is open.
    /// Call this on a periodic timer (e.g., every 30 seconds).
    ///
    /// Returns `Some(PppPacket)` to send, or `None` if the link isn't
    /// open or an echo is already outstanding.
    ///
    /// If `ECHO_MAX_FAILURES` consecutive echos go unanswered, returns
    /// `None` and the caller should check `echo_dead()` to detect a
    /// dead link.
    pub fn maybe_send_echo(&mut self) -> Option<PppPacket> {
        if self.state != LcpState::Opened {
            return None;
        }

        // If a previous echo is still pending, count it as a failure.
        if self.echo_pending_id.is_some() {
            self.echo_failures += 1;
            log::info!(
                "LCP: Echo-Request timeout (failures={})",
                self.echo_failures,
            );
            if self.echo_failures >= ECHO_MAX_FAILURES {
                log::warn!(
                    "LCP: echo keepalive dead after {} failures",
                    self.echo_failures,
                );
                return None;
            }
        }

        let id = self.alloc_id();
        self.echo_pending_id = Some(id);
        let pkt = LcpPacket {
            code: ECHO_REQUEST,
            identifier: id,
            // Magic number = 0 (4 bytes, per RFC 1661 §5.8).
            data: vec![0x00, 0x00, 0x00, 0x00],
        };
        log::debug!("LCP TX: Echo-Request id={}", id);
        Some(pkt.to_ppp())
    }

    /// Returns true if the echo keepalive has detected a dead link
    /// (ECHO_MAX_FAILURES consecutive unanswered Echo-Requests).
    pub fn echo_dead(&self) -> bool {
        self.echo_failures >= ECHO_MAX_FAILURES
    }

    /// Force LCP to Opened state.  Used when upper-layer traffic (IPCP/IP)
    /// proves the peer considers LCP open but we missed their Configure-Ack.
    pub fn force_open(&mut self) {
        self.peer_acked = true;
        self.we_acked = true;
        self.restart_ticks_remaining = 0;
        self.state = LcpState::Opened;
    }

    fn update_state(&mut self) {
        self.state = match (self.we_acked, self.peer_acked) {
            (true, true) => LcpState::Opened,
            (true, false) => LcpState::AckSent,
            (false, true) => LcpState::AckReceived,
            (false, false) => self.state,
        };
    }

    fn alloc_id(&mut self) -> u8 {
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        id
    }
}

/// Format LCP options for logging.
fn format_lcp_options(data: &[u8]) -> String {
    let opts = parse_options(data);
    opts.iter()
        .map(|o| {
            let name = match o.opt_type {
                1 => "MRU",
                2 => "ACCM",
                3 => "AuthProto",
                5 => "MagicNumber",
                7 => "ProtocolFieldCompression",
                8 => "AddressControlFieldCompression",
                _ => "Unknown",
            };
            if o.opt_type == 1 && o.data.len() == 2 {
                let mru = ((o.data[0] as u16) << 8) | o.data[1] as u16;
                format!("{}={}", name, mru)
            } else if o.opt_type == 2 && o.data.len() == 4 {
                let accm = u32::from_be_bytes([o.data[0], o.data[1], o.data[2], o.data[3]]);
                format!("{}=0x{:08x}", name, accm)
            } else if o.opt_type == 5 && o.data.len() == 4 {
                let magic = u32::from_be_bytes([o.data[0], o.data[1], o.data[2], o.data[3]]);
                format!("{}=0x{:08x}", name, magic)
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

    fn make_configure_request(id: u8, options: &[LcpOption]) -> PppPacket {
        let lcp = LcpPacket {
            code: CONFIGURE_REQUEST,
            identifier: id,
            data: serialize_options(options),
        };
        lcp.to_ppp()
    }

    fn make_configure_ack(id: u8, options: &[LcpOption]) -> PppPacket {
        let lcp = LcpPacket {
            code: CONFIGURE_ACK,
            identifier: id,
            data: serialize_options(options),
        };
        lcp.to_ppp()
    }

    fn ack_current_request(session: &LcpSession) -> PppPacket {
        LcpPacket {
            code: CONFIGURE_ACK,
            identifier: session
                .last_request_id
                .expect("session should have a pending request"),
            data: session.last_request_data.clone(),
        }
        .to_ppp()
    }

    #[test]
    fn lcp_packet_round_trip() {
        let pkt = LcpPacket {
            code: CONFIGURE_REQUEST,
            identifier: 42,
            data: vec![1, 4, 5, 220],
        };
        let bytes = pkt.to_bytes();
        let parsed = LcpPacket::parse(&bytes).unwrap();
        assert_eq!(parsed, pkt);
    }

    #[test]
    fn option_parse_and_serialize() {
        let opts = vec![LcpOption {
            opt_type: OPT_MRU,
            data: vec![0x05, 0xDC],
        }];
        let bytes = serialize_options(&opts);
        let parsed = parse_options(&bytes);
        assert_eq!(parsed, opts);
    }

    #[test]
    fn full_lcp_negotiation() {
        let mut session = LcpSession::new();
        assert_eq!(session.state, LcpState::Closed);

        // Step 1: BS sends Configure-Request (MRU=1500).
        let our_req = session.start();
        assert_eq!(session.state, LcpState::RequestSent);
        assert_eq!(our_req.protocol, LCP_PROTOCOL);
        let parsed = LcpPacket::parse(&our_req.payload).unwrap();
        assert_eq!(parsed.code, CONFIGURE_REQUEST);

        // Step 2: Mobile sends its Configure-Request.
        let mobile_req = make_configure_request(
            1,
            &[LcpOption {
                opt_type: OPT_MRU,
                data: vec![0x05, 0xDC],
            }],
        );
        let responses = session.receive(&mobile_req);
        // Should get a Configure-Ack for mobile's request.
        assert_eq!(responses.len(), 1);
        let ack = LcpPacket::parse(&responses[0].payload).unwrap();
        assert_eq!(ack.code, CONFIGURE_ACK);
        assert_eq!(ack.identifier, 1);
        assert_eq!(session.state, LcpState::AckSent);

        // Step 3: Mobile acks our Configure-Request.
        let mobile_ack = make_configure_ack(
            parsed.identifier,
            &[LcpOption {
                opt_type: OPT_MRU,
                data: vec![0x05, 0xDC],
            }],
        );
        let responses = session.receive(&mobile_ack);
        assert!(responses.is_empty());
        assert_eq!(session.state, LcpState::Opened);
        assert!(session.is_open());
    }

    #[test]
    fn lcp_ack_before_we_ack() {
        let mut session = LcpSession::new();
        let our_req = session.start();
        let parsed = LcpPacket::parse(&our_req.payload).unwrap();

        // Mobile acks us first.
        let mobile_ack = LcpPacket {
            code: CONFIGURE_ACK,
            identifier: parsed.identifier,
            data: parsed.data,
        };
        session.receive(&mobile_ack.to_ppp());
        assert_eq!(session.state, LcpState::AckReceived);

        // Then mobile sends its request.
        let mobile_req = make_configure_request(1, &[]);
        session.receive(&mobile_req);
        assert_eq!(session.state, LcpState::Opened);
    }

    #[test]
    fn configure_request_retransmits_until_acked() {
        let mut session = LcpSession::new();
        let first = session.start();
        let first_lcp = LcpPacket::parse(&first.payload).unwrap();

        for _ in 0..CONFIGURE_RESTART_TICKS {
            assert!(session.maybe_retransmit_configure_request().is_none());
        }

        let retransmit = session
            .maybe_retransmit_configure_request()
            .expect("pending LCP request should retransmit");
        let retransmit_lcp = LcpPacket::parse(&retransmit.payload).unwrap();
        assert_eq!(retransmit_lcp.identifier, first_lcp.identifier);
        assert_eq!(retransmit_lcp.data, first_lcp.data);
    }

    #[test]
    fn invalid_configure_ack_is_discarded() {
        let mut session = LcpSession::new();
        let first = session.start();
        let first_lcp = LcpPacket::parse(&first.payload).unwrap();

        let wrong_id = LcpPacket {
            code: CONFIGURE_ACK,
            identifier: first_lcp.identifier.wrapping_add(1),
            data: first_lcp.data.clone(),
        };
        assert!(session.receive(&wrong_id.to_ppp()).is_empty());
        assert_eq!(session.state, LcpState::RequestSent);

        let wrong_options = LcpPacket {
            code: CONFIGURE_ACK,
            identifier: first_lcp.identifier,
            data: vec![],
        };
        assert!(session.receive(&wrong_options.to_ppp()).is_empty());
        assert_eq!(session.state, LcpState::RequestSent);
    }

    #[test]
    fn negotiates_accm_pfc_acfc() {
        let mut session = LcpSession::new();
        session.start();

        // Mobile sends Configure-Request with ACCM=0, PFC, ACFC.
        let mobile_req = make_configure_request(
            1,
            &[
                LcpOption {
                    opt_type: OPT_ACCM,
                    data: vec![0x00, 0x00, 0x00, 0x00],
                },
                LcpOption {
                    opt_type: OPT_PFC,
                    data: vec![],
                },
                LcpOption {
                    opt_type: OPT_ACFC,
                    data: vec![],
                },
            ],
        );
        let responses = session.receive(&mobile_req);
        assert_eq!(responses.len(), 1);
        let ack = LcpPacket::parse(&responses[0].payload).unwrap();
        assert_eq!(ack.code, CONFIGURE_ACK);

        // Verify negotiated options.
        assert_eq!(session.negotiated.tx_accm, 0x00000000);
        assert!(session.negotiated.peer_sends_pfc);
        assert!(session.negotiated.peer_sends_acfc);
        // We didn't request PFC/ACFC, so we can't send them.
        assert!(!session.negotiated.we_send_pfc);
        assert!(!session.negotiated.we_send_acfc);
    }

    #[test]
    fn rejects_unknown_options() {
        let mut session = LcpSession::new();
        session.start();

        // Mobile sends Configure-Request with MRU + unknown option type 99.
        let mobile_req = make_configure_request(
            1,
            &[
                LcpOption {
                    opt_type: OPT_MRU,
                    data: vec![0x05, 0xDC],
                },
                LcpOption {
                    opt_type: 99,
                    data: vec![0x01, 0x02],
                },
            ],
        );
        let responses = session.receive(&mobile_req);
        assert_eq!(responses.len(), 1);
        let reject = LcpPacket::parse(&responses[0].payload).unwrap();
        assert_eq!(reject.code, CONFIGURE_REJECT);
        // Reject should contain only the unknown option.
        let rejected_opts = parse_options(&reject.data);
        assert_eq!(rejected_opts.len(), 1);
        assert_eq!(rejected_opts[0].opt_type, 99);
        // we_acked should NOT be set.
        assert!(!session.we_acked);
    }

    #[test]
    fn lcp_configure_nak_triggers_retry() {
        let mut session = LcpSession::new();
        session.start();

        // Mobile NAKs our MRU, suggesting 576.
        let nak = LcpPacket {
            code: CONFIGURE_NAK,
            identifier: 1,
            data: serialize_options(&[
                LcpOption {
                    opt_type: OPT_MRU,
                    data: vec![0x02, 0x40],
                }, // 576
            ]),
        };
        let responses = session.receive(&nak.to_ppp());
        assert_eq!(responses.len(), 1);
        let retry = LcpPacket::parse(&responses[0].payload).unwrap();
        assert_eq!(retry.code, CONFIGURE_REQUEST);
        // Should contain the NAK'd value.
        let opts = parse_options(&retry.data);
        assert_eq!(opts.len(), 1);
        assert_eq!(opts[0].opt_type, OPT_MRU);
        assert_eq!(opts[0].data, vec![0x02, 0x40]);
    }

    #[test]
    fn lcp_configure_reject_drops_option() {
        let mut session = LcpSession::new();
        session.start();

        // Mobile rejects our MRU option entirely.
        let reject = LcpPacket {
            code: CONFIGURE_REJECT,
            identifier: 1,
            data: serialize_options(&[LcpOption {
                opt_type: OPT_MRU,
                data: vec![0x05, 0xDC],
            }]),
        };
        let responses = session.receive(&reject.to_ppp());
        assert_eq!(responses.len(), 1);
        let retry = LcpPacket::parse(&responses[0].payload).unwrap();
        assert_eq!(retry.code, CONFIGURE_REQUEST);
        // MRU should be gone.
        let opts = parse_options(&retry.data);
        assert!(opts.is_empty());
    }

    #[test]
    fn echo_request_gets_reply() {
        let mut session = LcpSession::new();
        session.start();

        let echo = LcpPacket {
            code: ECHO_REQUEST,
            identifier: 7,
            data: vec![0x00, 0x00, 0x00, 0x00, 0xDE, 0xAD], // magic + data
        };
        let responses = session.receive(&echo.to_ppp());
        assert_eq!(responses.len(), 1);
        let reply = LcpPacket::parse(&responses[0].payload).unwrap();
        assert_eq!(reply.code, ECHO_REPLY);
        assert_eq!(reply.identifier, 7);
        // Magic number (4 bytes) + echoed data.
        assert_eq!(reply.data.len(), 6);
        assert_eq!(&reply.data[4..], &[0xDE, 0xAD]);
    }

    #[test]
    fn terminate_request_resets_state() {
        let mut session = LcpSession::new();
        session.start();

        // Get to Opened state.
        let mobile_req = make_configure_request(1, &[]);
        session.receive(&mobile_req);
        let mobile_ack = ack_current_request(&session);
        session.receive(&mobile_ack);
        assert!(session.is_open());

        // Terminate.
        let term = LcpPacket {
            code: TERMINATE_REQUEST,
            identifier: 5,
            data: vec![],
        };
        let responses = session.receive(&term.to_ppp());
        assert_eq!(responses.len(), 1);
        let ack = LcpPacket::parse(&responses[0].payload).unwrap();
        assert_eq!(ack.code, TERMINATE_ACK);
        assert_eq!(session.state, LcpState::Closed);
        assert!(!session.is_open());
    }

    #[test]
    fn malformed_packet_ignored() {
        let mut session = LcpSession::new();
        session.start();

        let bad = PppPacket {
            protocol: LCP_PROTOCOL,
            payload: vec![0x01], // too short
        };
        let responses = session.receive(&bad);
        assert!(responses.is_empty());
    }

    #[test]
    fn restart_resets_negotiated_options() {
        let mut session = LcpSession::new();
        session.start();

        // Negotiate with ACCM=0, PFC, ACFC.
        let mobile_req = make_configure_request(
            1,
            &[
                LcpOption {
                    opt_type: OPT_ACCM,
                    data: vec![0x00, 0x00, 0x00, 0x00],
                },
                LcpOption {
                    opt_type: OPT_PFC,
                    data: vec![],
                },
                LcpOption {
                    opt_type: OPT_ACFC,
                    data: vec![],
                },
            ],
        );
        session.receive(&mobile_req);
        let mobile_ack = ack_current_request(&session);
        session.receive(&mobile_ack);
        assert!(session.is_open());
        assert_eq!(session.negotiated.tx_accm, 0);
        assert!(session.negotiated.peer_sends_pfc);

        // Peer restarts — negotiated options should reset.
        let restart_req = make_configure_request(2, &[]);
        session.receive(&restart_req);
        assert_eq!(session.negotiated.tx_accm, 0xFFFFFFFF);
        assert!(!session.negotiated.peer_sends_pfc);
        assert!(!session.negotiated.peer_sends_acfc);
    }
}
