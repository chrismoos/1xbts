//! HRPD access authentication over the AN stream.
//!
//! A.S0008 access authentication runs PPP between the AT and the AN on the
//! Default Packet Application bound to the access network: LCP negotiates
//! CHAP MD5 (RFC 1994), the AN challenges, and the AT answers with its HRPD
//! access NAI. The AN hands the response to the AN-AAA over A12 and returns
//! CHAP Success or Failure, then closes the link. No network-layer protocol
//! runs on this stream.
//!
//! [`AccessAuthLink`] holds that procedure for one session and does no I/O.
//! The caller moves HDLC octets over the AN stream, runs the A12 exchange and
//! drives the timers.

use std::time::{Duration, Instant};

use cdma_packet::ppp::framing::{HdlcDeframer, PppPacket, frame};

const LCP_PROTOCOL: u16 = 0xC021;
const CHAP_PROTOCOL: u16 = 0xC223;

// RFC 1661 §5 LCP codes.
const LCP_CONFIGURE_REQUEST: u8 = 1;
const LCP_CONFIGURE_ACK: u8 = 2;
const LCP_CONFIGURE_NAK: u8 = 3;
const LCP_CONFIGURE_REJECT: u8 = 4;
const LCP_TERMINATE_REQUEST: u8 = 5;
const LCP_TERMINATE_ACK: u8 = 6;
const LCP_PROTOCOL_REJECT: u8 = 8;
const LCP_ECHO_REQUEST: u8 = 9;
const LCP_ECHO_REPLY: u8 = 10;
const LCP_DISCARD_REQUEST: u8 = 11;

// RFC 1661 §6 LCP options.
const LCP_OPT_MRU: u8 = 1;
const LCP_OPT_ACCM: u8 = 2;
const LCP_OPT_AUTH_PROTOCOL: u8 = 3;
const LCP_OPT_MAGIC_NUMBER: u8 = 5;
const LCP_OPT_PFC: u8 = 7;
const LCP_OPT_ACFC: u8 = 8;

/// RFC 1994 §3 CHAP algorithm 5 (MD5).
const CHAP_ALGORITHM_MD5: u8 = 5;
const CHAP_CHALLENGE: u8 = 1;
const CHAP_RESPONSE: u8 = 2;
const CHAP_SUCCESS: u8 = 3;
const CHAP_FAILURE: u8 = 4;
const CHAP_MD5_VALUE_OCTETS: usize = 16;
const CHAP_CHALLENGE_OCTETS: usize = 16;
const CHAP_AUTHENTICATOR_NAME: &[u8] = b"1xbts";

const PPP_HEADER_OCTETS: usize = 4;

const LCP_RETRY_INTERVAL: Duration = Duration::from_secs(1);
const LCP_MAX_CONFIGURE: u8 = 5;
const CHAP_RETRY_INTERVAL: Duration = Duration::from_secs(2);
const CHAP_MAX_CHALLENGES: u8 = 3;
const AAA_DEADLINE: Duration = Duration::from_secs(8);
const TERMINATE_RETRY_INTERVAL: Duration = Duration::from_secs(1);
const TERMINATE_MAX_REQUESTS: u8 = 2;
/// Upper bound on the whole procedure, so data setup never waits on a stalled link.
const OVERALL_DEADLINE: Duration = Duration::from_secs(20);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChapCredentials {
    pub nai: String,
    pub chap_id: u8,
    pub challenge: Vec<u8>,
    pub response: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AaaVerdict {
    Accept { imsi: Option<String> },
    Reject,
    Error(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessAuthResult {
    Accepted {
        imsi: Option<String>,
    },
    Rejected,
    AaaError(String),
    AaaTimeout,
    ChapRefused,
    /// The AT stopped answering before it sent a CHAP Response.
    NoResponse,
    /// The AT closed LCP before it sent a CHAP Response.
    PeerTerminated,
}

impl AccessAuthResult {
    pub fn imsi(&self) -> Option<&str> {
        match self {
            Self::Accepted { imsi } => imsi.as_deref(),
            _ => None,
        }
    }

    pub fn label(&self) -> &'static str {
        match self {
            Self::Accepted { .. } => "accepted",
            Self::Rejected => "rejected",
            Self::AaaError(_) => "aaa_error",
            Self::AaaTimeout => "aaa_timeout",
            Self::ChapRefused => "chap_refused",
            Self::NoResponse => "no_response",
            Self::PeerTerminated => "peer_terminated",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessAuthOutput {
    /// An HDLC-framed PPP frame for the AN stream.
    Send(Vec<u8>),
    /// Credentials to send to the AN-AAA. Answer with [`AccessAuthLink::on_aaa_verdict`].
    AaaRequest(ChapCredentials),
    Settled(AccessAuthResult),
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum State {
    Idle,
    Lcp {
        ours_acked: bool,
        theirs_acked: bool,
        requests_sent: u8,
        last_sent: Instant,
    },
    ChallengeSent {
        challenges_sent: u8,
        last_sent: Instant,
    },
    AwaitingAaa {
        since: Instant,
    },
    Terminating {
        requests_sent: u8,
        last_sent: Instant,
        result: AccessAuthResult,
    },
    Settled(AccessAuthResult),
}

#[derive(Debug)]
pub struct AccessAuthLink {
    state: State,
    /// Sends CHAP Failure for a refused or failed A12 exchange when set.
    enforce: bool,
    started_at: Option<Instant>,
    deframer: HdlcDeframer,
    next_lcp_id: u8,
    configure_id: u8,
    terminate_id: u8,
    chap_id: u8,
    challenge: [u8; CHAP_CHALLENGE_OCTETS],
}

impl Default for AccessAuthLink {
    fn default() -> Self {
        Self::new()
    }
}

impl AccessAuthLink {
    pub fn new() -> Self {
        Self {
            state: State::Idle,
            enforce: false,
            started_at: None,
            deframer: HdlcDeframer::new(),
            next_lcp_id: 1,
            configure_id: 0,
            terminate_id: 0,
            chap_id: 0,
            challenge: [0; CHAP_CHALLENGE_OCTETS],
        }
    }

    #[cfg(test)]
    fn enforcing() -> Self {
        Self {
            enforce: true,
            ..Self::new()
        }
    }

    pub fn is_started(&self) -> bool {
        !matches!(self.state, State::Idle)
    }

    pub fn result(&self) -> Option<&AccessAuthResult> {
        match &self.state {
            State::Settled(result) => Some(result),
            _ => None,
        }
    }

    pub fn state_name(&self) -> &'static str {
        match self.state {
            State::Idle => "idle",
            State::Lcp { .. } => "lcp",
            State::ChallengeSent { .. } => "challenge_sent",
            State::AwaitingAaa { .. } => "awaiting_aaa",
            State::Terminating { .. } => "terminating",
            State::Settled(_) => "settled",
        }
    }

    pub fn start(&mut self, now: Instant) -> Vec<AccessAuthOutput> {
        if self.is_started() {
            return Vec::new();
        }
        self.started_at = Some(now);
        getrandom::getrandom(&mut self.challenge).unwrap_or_else(|_| {
            let nanos = now.elapsed().as_nanos() ^ std::process::id() as u128;
            self.challenge.copy_from_slice(&nanos.to_be_bytes());
        });
        self.state = State::Lcp {
            ours_acked: false,
            theirs_acked: false,
            requests_sent: 1,
            last_sent: now,
        };
        vec![self.configure_request()]
    }

    /// Feeds octets the RLP receiver delivered on the AN stream.
    pub fn on_stream_octets(&mut self, octets: &[u8], now: Instant) -> Vec<AccessAuthOutput> {
        let mut out = Vec::new();
        for packet in self.deframer.feed(octets) {
            match packet.protocol {
                LCP_PROTOCOL => self.on_lcp(&packet.payload, now, &mut out),
                CHAP_PROTOCOL => self.on_chap(&packet.payload, now, &mut out),
                protocol => {
                    log::info!(
                        "HRPD access auth: protocol-rejecting 0x{protocol:04x} on the AN stream state={}",
                        self.state_name()
                    );
                    if self.lcp_open() {
                        let mut data = protocol.to_be_bytes().to_vec();
                        data.extend_from_slice(&packet.payload);
                        let id = self.take_lcp_id();
                        out.push(lcp_frame(LCP_PROTOCOL_REJECT, id, &data));
                    }
                }
            }
        }
        out
    }

    /// Settles an unfinished procedure whose traffic channel closed.
    pub fn abandon(&mut self) -> Vec<AccessAuthOutput> {
        let result = match &self.state {
            State::Idle | State::Settled(_) => return Vec::new(),
            State::AwaitingAaa { .. } => AccessAuthResult::AaaTimeout,
            State::Terminating { result, .. } => result.clone(),
            _ => AccessAuthResult::NoResponse,
        };
        log::info!(
            "HRPD access auth: traffic channel closed in state={}",
            self.state_name()
        );
        self.settle(result)
    }

    pub fn on_aaa_verdict(&mut self, verdict: AaaVerdict, now: Instant) -> Vec<AccessAuthOutput> {
        if !matches!(self.state, State::AwaitingAaa { .. }) {
            log::info!(
                "HRPD access auth: ignoring A12 verdict {verdict:?} in state={}",
                self.state_name()
            );
            return Vec::new();
        }
        let (result, success) = match verdict {
            AaaVerdict::Accept { imsi } => (AccessAuthResult::Accepted { imsi }, true),
            AaaVerdict::Reject => (AccessAuthResult::Rejected, false),
            AaaVerdict::Error(error) => (AccessAuthResult::AaaError(error), false),
        };
        self.finish_chap(success, result, now)
    }

    fn finish_chap(
        &mut self,
        accepted: bool,
        result: AccessAuthResult,
        now: Instant,
    ) -> Vec<AccessAuthOutput> {
        // Access authentication does not gate service yet, so the AT always
        // gets CHAP Success and the A12 result is only reported.
        let success = accepted || !self.enforce;
        if success != accepted {
            log::info!(
                "HRPD access auth: sending CHAP Success without enforcement result={}",
                result.label()
            );
        }
        self.send_chap_result(success, result, now)
    }

    pub fn on_timer(&mut self, now: Instant) -> Vec<AccessAuthOutput> {
        let mut out = Vec::new();
        if let Some(started_at) = self.started_at
            && !matches!(self.state, State::Settled(_) | State::Idle)
            && now.saturating_duration_since(started_at) >= OVERALL_DEADLINE
        {
            let result = match &self.state {
                State::AwaitingAaa { .. } => AccessAuthResult::AaaTimeout,
                State::Terminating { result, .. } => result.clone(),
                _ => AccessAuthResult::NoResponse,
            };
            log::info!(
                "HRPD access auth: overall deadline reached state={} result={}",
                self.state_name(),
                result.label()
            );
            return self.settle(result);
        }
        match self.state.clone() {
            State::Lcp {
                ours_acked,
                theirs_acked,
                requests_sent,
                last_sent,
            } if !ours_acked && now.saturating_duration_since(last_sent) >= LCP_RETRY_INTERVAL => {
                if requests_sent >= LCP_MAX_CONFIGURE {
                    log::info!(
                        "HRPD access auth: no LCP Configure-Ack after {requests_sent} requests"
                    );
                    return self.settle(AccessAuthResult::NoResponse);
                }
                self.state = State::Lcp {
                    ours_acked,
                    theirs_acked,
                    requests_sent: requests_sent + 1,
                    last_sent: now,
                };
                out.push(self.configure_request());
            }
            State::ChallengeSent {
                challenges_sent,
                last_sent,
            } if now.saturating_duration_since(last_sent) >= CHAP_RETRY_INTERVAL => {
                if challenges_sent >= CHAP_MAX_CHALLENGES {
                    log::info!(
                        "HRPD access auth: no CHAP Response after {challenges_sent} challenges"
                    );
                    return self.begin_terminate(AccessAuthResult::NoResponse, now);
                }
                self.state = State::ChallengeSent {
                    challenges_sent: challenges_sent + 1,
                    last_sent: now,
                };
                out.push(self.chap_challenge());
            }
            State::AwaitingAaa { since }
                if now.saturating_duration_since(since) >= AAA_DEADLINE =>
            {
                log::info!("HRPD access auth: no A12 verdict within {AAA_DEADLINE:?}");
                return self.finish_chap(false, AccessAuthResult::AaaTimeout, now);
            }
            State::Terminating {
                requests_sent,
                last_sent,
                result,
            } if now.saturating_duration_since(last_sent) >= TERMINATE_RETRY_INTERVAL => {
                if requests_sent >= TERMINATE_MAX_REQUESTS {
                    return self.settle(result);
                }
                self.state = State::Terminating {
                    requests_sent: requests_sent + 1,
                    last_sent: now,
                    result,
                };
                out.push(lcp_frame(LCP_TERMINATE_REQUEST, self.terminate_id, &[]));
            }
            _ => {}
        }
        out
    }

    fn lcp_open(&self) -> bool {
        !matches!(
            self.state,
            State::Idle | State::Lcp { .. } | State::Settled(_)
        )
    }

    fn take_lcp_id(&mut self) -> u8 {
        let id = self.next_lcp_id;
        self.next_lcp_id = self.next_lcp_id.wrapping_add(1);
        id
    }

    fn configure_request(&mut self) -> AccessAuthOutput {
        self.configure_id = self.take_lcp_id();
        lcp_frame(
            LCP_CONFIGURE_REQUEST,
            self.configure_id,
            &[
                LCP_OPT_AUTH_PROTOCOL,
                5,
                (CHAP_PROTOCOL >> 8) as u8,
                CHAP_PROTOCOL as u8,
                CHAP_ALGORITHM_MD5,
            ],
        )
    }

    fn chap_challenge(&self) -> AccessAuthOutput {
        let mut data =
            Vec::with_capacity(1 + CHAP_CHALLENGE_OCTETS + CHAP_AUTHENTICATOR_NAME.len());
        data.push(CHAP_CHALLENGE_OCTETS as u8);
        data.extend_from_slice(&self.challenge);
        data.extend_from_slice(CHAP_AUTHENTICATOR_NAME);
        chap_frame(CHAP_CHALLENGE, self.chap_id, &data)
    }

    fn on_lcp(&mut self, payload: &[u8], now: Instant, out: &mut Vec<AccessAuthOutput>) {
        let Some((code, id, data)) = parse_control_packet(payload) else {
            log::info!(
                "HRPD access auth: malformed LCP packet len={}",
                payload.len()
            );
            return;
        };
        match code {
            LCP_CONFIGURE_REQUEST => self.on_peer_configure_request(id, data, now, out),
            LCP_CONFIGURE_ACK if id == self.configure_id => {
                if let State::Lcp {
                    ours_acked: false,
                    theirs_acked,
                    requests_sent,
                    last_sent,
                } = self.state
                {
                    self.state = State::Lcp {
                        ours_acked: true,
                        theirs_acked,
                        requests_sent,
                        last_sent,
                    };
                    self.maybe_open(now, out);
                }
            }
            LCP_CONFIGURE_NAK | LCP_CONFIGURE_REJECT if id == self.configure_id => {
                if matches!(self.state, State::Lcp { .. }) {
                    log::info!(
                        "HRPD access auth: AT refused CHAP in LCP code={} options={}",
                        code,
                        hex(data)
                    );
                    out.extend(self.begin_terminate(AccessAuthResult::ChapRefused, now));
                }
            }
            LCP_TERMINATE_REQUEST => {
                out.push(lcp_frame(LCP_TERMINATE_ACK, id, &[]));
                let result = match &self.state {
                    State::Terminating { result, .. } => result.clone(),
                    State::AwaitingAaa { .. } => return,
                    State::Settled(_) => return,
                    _ => AccessAuthResult::PeerTerminated,
                };
                out.extend(self.settle(result));
            }
            LCP_TERMINATE_ACK => {
                if let State::Terminating { result, .. } = &self.state
                    && id == self.terminate_id
                {
                    let result = result.clone();
                    out.extend(self.settle(result));
                }
            }
            LCP_ECHO_REQUEST if self.lcp_open() => {
                let mut reply = vec![0; 4];
                reply.extend_from_slice(data.get(4..).unwrap_or_default());
                out.push(lcp_frame(LCP_ECHO_REPLY, id, &reply));
            }
            LCP_ECHO_REPLY | LCP_DISCARD_REQUEST => {}
            _ => log::info!(
                "HRPD access auth: ignoring LCP code={code} id={id} state={}",
                self.state_name()
            ),
        }
    }

    fn on_peer_configure_request(
        &mut self,
        id: u8,
        data: &[u8],
        now: Instant,
        out: &mut Vec<AccessAuthOutput>,
    ) {
        let mut rejected = Vec::new();
        let mut cursor = 0;
        while cursor + 2 <= data.len() {
            let len = usize::from(data[cursor + 1]);
            if len < 2 || cursor + len > data.len() {
                log::info!("HRPD access auth: malformed LCP option list {}", hex(data));
                return;
            }
            let option = &data[cursor..cursor + len];
            if !matches!(
                option[0],
                LCP_OPT_MRU | LCP_OPT_ACCM | LCP_OPT_MAGIC_NUMBER | LCP_OPT_PFC | LCP_OPT_ACFC
            ) {
                rejected.extend_from_slice(option);
            }
            cursor += len;
        }
        if !rejected.is_empty() {
            out.push(lcp_frame(LCP_CONFIGURE_REJECT, id, &rejected));
            return;
        }
        out.push(lcp_frame(LCP_CONFIGURE_ACK, id, data));
        match self.state {
            State::Lcp {
                ours_acked,
                requests_sent,
                last_sent,
                ..
            } => {
                self.state = State::Lcp {
                    ours_acked,
                    theirs_acked: true,
                    requests_sent,
                    last_sent,
                };
                self.maybe_open(now, out);
            }
            State::Idle | State::Settled(_) | State::Terminating { .. } => {}
            _ => log::info!(
                "HRPD access auth: AT renegotiated LCP in state={}",
                self.state_name()
            ),
        }
    }

    fn maybe_open(&mut self, now: Instant, out: &mut Vec<AccessAuthOutput>) {
        if let State::Lcp {
            ours_acked: true,
            theirs_acked: true,
            ..
        } = self.state
        {
            self.chap_id = self.chap_id.wrapping_add(1);
            self.state = State::ChallengeSent {
                challenges_sent: 1,
                last_sent: now,
            };
            log::info!(
                "HRPD access auth: LCP open; sending CHAP Challenge id={}",
                self.chap_id
            );
            out.push(self.chap_challenge());
        }
    }

    fn on_chap(&mut self, payload: &[u8], now: Instant, out: &mut Vec<AccessAuthOutput>) {
        let Some((code, id, data)) = parse_control_packet(payload) else {
            log::info!(
                "HRPD access auth: malformed CHAP packet len={}",
                payload.len()
            );
            return;
        };
        if code != CHAP_RESPONSE {
            log::info!("HRPD access auth: ignoring CHAP code={code} id={id}");
            return;
        }
        if !matches!(self.state, State::ChallengeSent { .. }) || id != self.chap_id {
            log::info!(
                "HRPD access auth: ignoring CHAP Response id={id} expected={} state={}",
                self.chap_id,
                self.state_name()
            );
            return;
        }
        let Some(&value_size) = data.first() else {
            return;
        };
        let value_end = 1 + usize::from(value_size);
        if usize::from(value_size) != CHAP_MD5_VALUE_OCTETS || data.len() < value_end {
            log::info!(
                "HRPD access auth: CHAP Response with value_size={value_size} len={}",
                data.len()
            );
            return;
        }
        let credentials = ChapCredentials {
            nai: String::from_utf8_lossy(&data[value_end..]).into_owned(),
            chap_id: id,
            challenge: self.challenge.to_vec(),
            response: data[1..value_end].to_vec(),
        };
        log::info!(
            "HRPD access auth: CHAP Response nai={} chap_id={} challenge={} response={}",
            credentials.nai,
            credentials.chap_id,
            hex(&credentials.challenge),
            hex(&credentials.response)
        );
        self.state = State::AwaitingAaa { since: now };
        out.push(AccessAuthOutput::AaaRequest(credentials));
    }

    fn send_chap_result(
        &mut self,
        success: bool,
        result: AccessAuthResult,
        now: Instant,
    ) -> Vec<AccessAuthOutput> {
        let (code, message): (u8, &[u8]) = if success {
            (CHAP_SUCCESS, b"Welcome")
        } else {
            (CHAP_FAILURE, b"Access denied")
        };
        let mut out = vec![chap_frame(code, self.chap_id, message)];
        out.extend(self.begin_terminate(result, now));
        out
    }

    fn begin_terminate(&mut self, result: AccessAuthResult, now: Instant) -> Vec<AccessAuthOutput> {
        self.terminate_id = self.take_lcp_id();
        self.state = State::Terminating {
            requests_sent: 1,
            last_sent: now,
            result,
        };
        vec![lcp_frame(LCP_TERMINATE_REQUEST, self.terminate_id, &[])]
    }

    fn settle(&mut self, result: AccessAuthResult) -> Vec<AccessAuthOutput> {
        log::info!("HRPD access auth: settled result={result:?}");
        self.state = State::Settled(result.clone());
        vec![AccessAuthOutput::Settled(result)]
    }
}

fn parse_control_packet(payload: &[u8]) -> Option<(u8, u8, &[u8])> {
    if payload.len() < PPP_HEADER_OCTETS {
        return None;
    }
    let length = usize::from(u16::from_be_bytes([payload[2], payload[3]]));
    if length < PPP_HEADER_OCTETS || length > payload.len() {
        return None;
    }
    Some((payload[0], payload[1], &payload[PPP_HEADER_OCTETS..length]))
}

fn control_packet(code: u8, id: u8, data: &[u8]) -> Vec<u8> {
    let length = (PPP_HEADER_OCTETS + data.len()) as u16;
    let mut packet = Vec::with_capacity(usize::from(length));
    packet.push(code);
    packet.push(id);
    packet.extend_from_slice(&length.to_be_bytes());
    packet.extend_from_slice(data);
    packet
}

fn lcp_frame(code: u8, id: u8, data: &[u8]) -> AccessAuthOutput {
    AccessAuthOutput::Send(frame(&PppPacket {
        protocol: LCP_PROTOCOL,
        payload: control_packet(code, id, data),
    }))
}

fn chap_frame(code: u8, id: u8, data: &[u8]) -> AccessAuthOutput {
    AccessAuthOutput::Send(frame(&PppPacket {
        protocol: CHAP_PROTOCOL,
        payload: control_packet(code, id, data),
    }))
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sent_packets(out: &[AccessAuthOutput]) -> Vec<PppPacket> {
        let mut deframer = HdlcDeframer::new();
        out.iter()
            .filter_map(|output| match output {
                AccessAuthOutput::Send(octets) => Some(deframer.feed(octets)),
                _ => None,
            })
            .flatten()
            .collect()
    }

    fn at_frame(protocol: u16, code: u8, id: u8, data: &[u8]) -> Vec<u8> {
        frame(&PppPacket {
            protocol,
            payload: control_packet(code, id, data),
        })
    }

    fn open_lcp(link: &mut AccessAuthLink, now: Instant) -> Vec<AccessAuthOutput> {
        let start = link.start(now);
        let request = &sent_packets(&start)[0];
        assert_eq!(request.protocol, LCP_PROTOCOL);
        assert_eq!(&request.payload[4..], &[3, 5, 0xc2, 0x23, 5]);
        let id = request.payload[1];
        let mut out = link.on_stream_octets(
            &at_frame(LCP_PROTOCOL, LCP_CONFIGURE_REQUEST, 7, &[5, 6, 1, 2, 3, 4]),
            now,
        );
        out.extend(link.on_stream_octets(&at_frame(LCP_PROTOCOL, LCP_CONFIGURE_ACK, id, &[]), now));
        out
    }

    fn chap_response(link: &AccessAuthLink, nai: &[u8]) -> Vec<u8> {
        let mut data = vec![16];
        data.extend_from_slice(&[0xab; 16]);
        data.extend_from_slice(nai);
        at_frame(CHAP_PROTOCOL, CHAP_RESPONSE, link.chap_id, &data)
    }

    #[test]
    fn lcp_opens_then_challenge_response_requests_aaa() {
        let now = Instant::now();
        let mut link = AccessAuthLink::new();
        let out = open_lcp(&mut link, now);
        let packets = sent_packets(&out);
        assert_eq!(packets[0].payload[0], LCP_CONFIGURE_ACK);
        let challenge = packets
            .iter()
            .find(|p| p.protocol == CHAP_PROTOCOL)
            .unwrap();
        assert_eq!(challenge.payload[0], CHAP_CHALLENGE);
        assert_eq!(challenge.payload[4], 16);
        assert_eq!(&challenge.payload[21..], CHAP_AUTHENTICATOR_NAME);

        let out = link.on_stream_octets(&chap_response(&link, b"a0000025f07973@hrpd"), now);
        let AccessAuthOutput::AaaRequest(credentials) = &out[0] else {
            panic!("expected an A12 request, got {out:?}");
        };
        assert_eq!(credentials.nai, "a0000025f07973@hrpd");
        assert_eq!(credentials.response, [0xab; 16]);
        assert_eq!(credentials.challenge, challenge.payload[5..21]);
        assert_eq!(link.state_name(), "awaiting_aaa");
    }

    #[test]
    fn accept_sends_success_and_terminate_then_settles_on_terminate_ack() {
        let now = Instant::now();
        let mut link = AccessAuthLink::new();
        open_lcp(&mut link, now);
        link.on_stream_octets(&chap_response(&link, b"nai"), now);
        let out = link.on_aaa_verdict(
            AaaVerdict::Accept {
                imsi: Some("310550000000001".into()),
            },
            now,
        );
        let packets = sent_packets(&out);
        assert_eq!(packets[0].protocol, CHAP_PROTOCOL);
        assert_eq!(packets[0].payload[0], CHAP_SUCCESS);
        assert_eq!(packets[1].payload[0], LCP_TERMINATE_REQUEST);
        let out = link.on_stream_octets(
            &at_frame(LCP_PROTOCOL, LCP_TERMINATE_ACK, packets[1].payload[1], &[]),
            now,
        );
        assert_eq!(
            out,
            vec![AccessAuthOutput::Settled(AccessAuthResult::Accepted {
                imsi: Some("310550000000001".into())
            })]
        );
        assert_eq!(
            link.result().and_then(AccessAuthResult::imsi),
            Some("310550000000001")
        );
    }

    #[test]
    fn reject_sends_success_without_enforcement() {
        let now = Instant::now();
        let mut link = AccessAuthLink::new();
        open_lcp(&mut link, now);
        link.on_stream_octets(&chap_response(&link, b"nai"), now);
        let packets = sent_packets(&link.on_aaa_verdict(AaaVerdict::Reject, now));
        assert_eq!(packets[0].payload[0], CHAP_SUCCESS);
    }

    #[test]
    fn reject_sends_failure_when_enforcing() {
        let now = Instant::now();
        let mut link = AccessAuthLink::enforcing();
        open_lcp(&mut link, now);
        link.on_stream_octets(&chap_response(&link, b"nai"), now);
        let packets = sent_packets(&link.on_aaa_verdict(AaaVerdict::Reject, now));
        assert_eq!(packets[0].payload[0], CHAP_FAILURE);
    }

    #[test]
    fn chap_nak_settles_as_refused_after_terminate() {
        let now = Instant::now();
        let mut link = AccessAuthLink::new();
        let start = link.start(now);
        let id = sent_packets(&start)[0].payload[1];
        let out = link.on_stream_octets(
            &at_frame(LCP_PROTOCOL, LCP_CONFIGURE_NAK, id, &[3, 4, 0xc0, 0x23]),
            now,
        );
        let terminate = &sent_packets(&out)[0];
        assert_eq!(terminate.payload[0], LCP_TERMINATE_REQUEST);
        let out = link.on_timer(now + Duration::from_secs(1));
        assert_eq!(sent_packets(&out)[0].payload[0], LCP_TERMINATE_REQUEST);
        let out = link.on_timer(now + Duration::from_secs(2));
        assert_eq!(
            out,
            vec![AccessAuthOutput::Settled(AccessAuthResult::ChapRefused)]
        );
    }

    #[test]
    fn silent_at_settles_as_no_response_after_retries() {
        let now = Instant::now();
        let mut link = AccessAuthLink::new();
        link.start(now);
        let mut settled = None;
        for second in 1..10 {
            for output in link.on_timer(now + Duration::from_secs(second)) {
                if let AccessAuthOutput::Settled(result) = output {
                    settled = Some((second, result));
                }
            }
        }
        assert_eq!(settled, Some((5, AccessAuthResult::NoResponse)));
    }

    #[test]
    fn missing_aaa_verdict_times_out_with_failure() {
        let now = Instant::now();
        let mut link = AccessAuthLink::enforcing();
        open_lcp(&mut link, now);
        link.on_stream_octets(&chap_response(&link, b"nai"), now);
        let out = link.on_timer(now + AAA_DEADLINE);
        let packets = sent_packets(&out);
        assert_eq!(packets[0].payload[0], CHAP_FAILURE);
        let out = link.on_timer(now + OVERALL_DEADLINE);
        assert_eq!(
            out,
            vec![AccessAuthOutput::Settled(AccessAuthResult::AaaTimeout)]
        );
    }

    #[test]
    fn unsupported_peer_options_are_rejected() {
        let now = Instant::now();
        let mut link = AccessAuthLink::new();
        link.start(now);
        let out = link.on_stream_octets(
            &at_frame(
                LCP_PROTOCOL,
                LCP_CONFIGURE_REQUEST,
                9,
                &[1, 4, 5, 220, 3, 4, 0xc0, 0x23],
            ),
            now,
        );
        let reject = &sent_packets(&out)[0];
        assert_eq!(reject.payload[0], LCP_CONFIGURE_REJECT);
        assert_eq!(&reject.payload[4..], &[3, 4, 0xc0, 0x23]);
    }

    #[test]
    fn peer_terminate_before_response_settles() {
        let now = Instant::now();
        let mut link = AccessAuthLink::new();
        open_lcp(&mut link, now);
        let out =
            link.on_stream_octets(&at_frame(LCP_PROTOCOL, LCP_TERMINATE_REQUEST, 3, &[]), now);
        assert_eq!(sent_packets(&out)[0].payload[0], LCP_TERMINATE_ACK);
        assert_eq!(
            out.last(),
            Some(&AccessAuthOutput::Settled(AccessAuthResult::PeerTerminated))
        );
    }
}
