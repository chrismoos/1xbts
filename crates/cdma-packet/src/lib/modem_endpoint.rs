//! TCP/380 "modem server" endpoint for the SO 12 async data service.
//!
//! When an SO 12 mobile reaches the Active (IP) phase it opens a TCP connection
//! to the IWF's well-known modem-server port 380 (IS-707-A.4 §2.3) and speaks
//! the TIA-617 modem control channel over it. This module terminates that
//! connection with a minimal single-connection userspace TCP state machine and
//! drives a [`cdma_modem::ModemServer`], so the AT/dial/CONNECT handshake
//! completes without an OS TCP stack.
//!
//! It consumes uplink IPv4 packets and produces downlink IPv4 packets plus
//! data-path events (dial, online user data, hang-up). The end-user data path
//! after CONNECT (NAS/PPP termination) is handled by the caller via those
//! events.

use std::net::Ipv4Addr;

use cdma_modem::{ModemServer, ServerEvent};

const IP_PROTO_TCP: u8 = 6;
const MODEM_SERVER_PORT: u16 = 380;

const TCP_FIN: u8 = 0x01;
const TCP_SYN: u8 = 0x02;
const TCP_RST: u8 = 0x04;
const TCP_PSH: u8 = 0x08;
const TCP_ACK: u8 = 0x10;

/// The reported CONNECT line rate for the emulated modem.
const CONNECT_RATE_BPS: u32 = 14_400;
/// Initial send sequence number for the IWF side (fixed for determinism;
/// randomness is unnecessary on a private point-to-point link).
const IWF_INITIAL_SEQ: u32 = 0x1000;
/// Max TCP payload per downlink segment for online data.
const TCP_MSS: usize = 512;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TcpState {
    SynReceived,
    Established,
    CloseWait,
    LastAck,
    Closed,
}

struct TcpConn {
    remote_port: u16,
    /// Next sequence number we will send.
    snd_nxt: u32,
    /// Next sequence number we expect to receive.
    rcv_nxt: u32,
    state: TcpState,
}

/// Result of feeding one uplink IP packet to the endpoint.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct EndpointOut {
    /// Downlink IPv4 packets to send to the mobile.
    pub downlink: Vec<Vec<u8>>,
    /// Online user data to forward to the connected data path (NAS/PPP).
    pub user_data: Vec<u8>,
    /// The mobile dialed this number (dialable digits).
    pub dialed: Option<String>,
    /// The mobile hung up.
    pub hangup: bool,
    /// True when the packet was addressed to the modem-server port and was
    /// consumed here (so the caller must not also forward it to the network).
    pub matched: bool,
}

/// Minimal TCP/380 modem-server endpoint (one connection at a time).
pub struct Tcp380Endpoint {
    iwf_ip: Ipv4Addr,
    ms_ip: Ipv4Addr,
    conn: Option<TcpConn>,
    server: ModemServer,
    /// True once the emulated modem has reported CONNECT (online).
    connected: bool,
}

impl Tcp380Endpoint {
    pub fn new(iwf_ip: Ipv4Addr, ms_ip: Ipv4Addr) -> Self {
        let mut server = ModemServer::new();
        server.set_addresses(Some(ms_ip.to_string()), Some(iwf_ip.to_string()));
        Self {
            iwf_ip,
            ms_ip,
            conn: None,
            server,
            connected: false,
        }
    }

    /// Feed an uplink IPv4 packet. Returns downlink packets and data-path events.
    /// Packets not addressed to the modem-server port are ignored (empty out).
    pub fn on_uplink_ip(&mut self, ip: &[u8]) -> EndpointOut {
        let mut out = EndpointOut::default();
        let Some(seg) = parse_ipv4_tcp(ip) else {
            return out;
        };
        if seg.dst_ip != self.iwf_ip || seg.dst_port != MODEM_SERVER_PORT {
            return out;
        }
        out.matched = true;

        // Reset an out-of-band connection attempt.
        if seg.flags & TCP_RST != 0 {
            self.conn = None;
            self.connected = false;
            self.server.on_tcp_established(); // reset modem for next call
            return out;
        }

        if seg.flags & TCP_SYN != 0 && self.conn.is_none() {
            let conn = TcpConn {
                remote_port: seg.src_port,
                snd_nxt: IWF_INITIAL_SEQ,
                rcv_nxt: seg.seq.wrapping_add(1),
                state: TcpState::SynReceived,
            };
            // SYN-ACK.
            out.downlink
                .push(self.build_segment(&conn, TCP_SYN | TCP_ACK, conn.snd_nxt, &[]));
            let mut conn = conn;
            conn.snd_nxt = conn.snd_nxt.wrapping_add(1);
            self.conn = Some(conn);
            return out;
        }

        let Some(mut conn) = self.conn.take() else {
            return out;
        };
        if seg.src_port != conn.remote_port {
            self.conn = Some(conn);
            return out;
        }

        // Complete the handshake on the first ACK.
        if conn.state == TcpState::SynReceived && seg.flags & TCP_ACK != 0 {
            conn.state = TcpState::Established;
            self.server.on_tcp_established();
        }

        // Accept in-order payload.
        if !seg.payload.is_empty() && seg.seq == conn.rcv_nxt {
            conn.rcv_nxt = conn.rcv_nxt.wrapping_add(seg.payload.len() as u32);
            self.drive_modem(&seg.payload, &mut conn, &mut out);
            // Acknowledge the received data.
            out.downlink
                .push(self.build_segment(&conn, TCP_ACK, conn.snd_nxt, &[]));
        }

        // Handle FIN (mobile closing).
        if seg.flags & TCP_FIN != 0 && conn.state == TcpState::Established {
            conn.rcv_nxt = conn.rcv_nxt.wrapping_add(1);
            conn.state = TcpState::CloseWait;
            out.downlink
                .push(self.build_segment(&conn, TCP_ACK, conn.snd_nxt, &[]));
            // Send our FIN.
            out.downlink
                .push(self.build_segment(&conn, TCP_FIN | TCP_ACK, conn.snd_nxt, &[]));
            conn.snd_nxt = conn.snd_nxt.wrapping_add(1);
            conn.state = TcpState::LastAck;
            out.hangup = true;
            self.connected = false;
        } else if conn.state == TcpState::LastAck && seg.flags & TCP_ACK != 0 {
            conn.state = TcpState::Closed;
        }

        if conn.state != TcpState::Closed {
            self.conn = Some(conn);
        }
        out
    }

    /// Feed modem-server input and translate its events, transmitting any
    /// TIA-617 replies as TCP payload segments on the downlink.
    fn drive_modem(&mut self, payload: &[u8], conn: &mut TcpConn, out: &mut EndpointOut) {
        let events = self.server.feed(payload);
        self.apply_events(events, conn, out);
    }

    fn apply_events(
        &mut self,
        events: Vec<ServerEvent>,
        conn: &mut TcpConn,
        out: &mut EndpointOut,
    ) {
        for ev in events {
            match ev {
                ServerEvent::ToMobile(bytes) => {
                    out.downlink.push(self.build_segment(
                        conn,
                        TCP_PSH | TCP_ACK,
                        conn.snd_nxt,
                        &bytes,
                    ));
                    conn.snd_nxt = conn.snd_nxt.wrapping_add(bytes.len() as u32);
                }
                ServerEvent::Dial { digits, .. } => {
                    out.dialed = Some(digits);
                }
                ServerEvent::Answer => {}
                ServerEvent::UserData(d) => out.user_data.extend_from_slice(&d),
                ServerEvent::Hangup => out.hangup = true,
            }
        }
    }

    /// Signal that the end-user data path connected; emits CONNECT downstream.
    pub fn carrier_up(&mut self) -> EndpointOut {
        let mut out = EndpointOut::default();
        self.connected = true;
        let events = self.server.on_carrier_up(CONNECT_RATE_BPS);
        if let Some(mut conn) = self.conn.take() {
            self.apply_events(events, &mut conn, &mut out);
            self.conn = Some(conn);
        }
        out
    }

    pub fn is_connected(&self) -> bool {
        self.connected
    }

    /// Send online (post-CONNECT) data to the mobile as TCP/380 payload
    /// segments. Used to carry the NAS's downlink PPP octets. Returns the
    /// downlink IPv4 packets to transmit.
    pub fn send_online(&mut self, bytes: &[u8]) -> Vec<Vec<u8>> {
        let mut out = Vec::new();
        if bytes.is_empty() {
            return out;
        }
        if let Some(mut conn) = self.conn.take() {
            if conn.state == TcpState::Established {
                for chunk in bytes.chunks(TCP_MSS) {
                    out.push(self.build_segment(&conn, TCP_PSH | TCP_ACK, conn.snd_nxt, chunk));
                    conn.snd_nxt = conn.snd_nxt.wrapping_add(chunk.len() as u32);
                }
            }
            self.conn = Some(conn);
        }
        out
    }

    fn build_segment(&self, conn: &TcpConn, flags: u8, seq: u32, payload: &[u8]) -> Vec<u8> {
        build_ipv4_tcp(
            self.iwf_ip,
            self.ms_ip,
            MODEM_SERVER_PORT,
            conn.remote_port,
            seq,
            conn.rcv_nxt,
            flags,
            payload,
        )
    }
}

// Minimal IPv4 / TCP parse + build

struct TcpSegment {
    dst_ip: Ipv4Addr,
    src_port: u16,
    dst_port: u16,
    seq: u32,
    flags: u8,
    payload: Vec<u8>,
}

fn parse_ipv4_tcp(ip: &[u8]) -> Option<TcpSegment> {
    if ip.len() < 20 || (ip[0] >> 4) != 4 {
        return None;
    }
    let ihl = ((ip[0] & 0x0F) as usize) * 4;
    if ip.len() < ihl + 20 || ip[9] != IP_PROTO_TCP {
        return None;
    }
    let total_len = u16::from_be_bytes([ip[2], ip[3]]) as usize;
    let total_len = total_len.min(ip.len());
    let dst_ip = Ipv4Addr::new(ip[16], ip[17], ip[18], ip[19]);
    let tcp = &ip[ihl..total_len];
    if tcp.len() < 20 {
        return None;
    }
    let src_port = u16::from_be_bytes([tcp[0], tcp[1]]);
    let dst_port = u16::from_be_bytes([tcp[2], tcp[3]]);
    let seq = u32::from_be_bytes([tcp[4], tcp[5], tcp[6], tcp[7]]);
    let data_off = ((tcp[12] >> 4) as usize) * 4;
    if tcp.len() < data_off {
        return None;
    }
    let flags = tcp[13];
    let payload = tcp[data_off..].to_vec();
    Some(TcpSegment {
        dst_ip,
        src_port,
        dst_port,
        seq,
        flags,
        payload,
    })
}

#[allow(clippy::too_many_arguments)]
fn build_ipv4_tcp(
    src_ip: Ipv4Addr,
    dst_ip: Ipv4Addr,
    src_port: u16,
    dst_port: u16,
    seq: u32,
    ack: u32,
    flags: u8,
    payload: &[u8],
) -> Vec<u8> {
    let tcp_len = 20 + payload.len();
    let total_len = 20 + tcp_len;
    let mut pkt = vec![0u8; total_len];

    // IPv4 header.
    pkt[0] = 0x45;
    pkt[2..4].copy_from_slice(&(total_len as u16).to_be_bytes());
    pkt[8] = 64; // TTL
    pkt[9] = IP_PROTO_TCP;
    pkt[12..16].copy_from_slice(&src_ip.octets());
    pkt[16..20].copy_from_slice(&dst_ip.octets());
    let ip_csum = checksum(&pkt[0..20]);
    pkt[10..12].copy_from_slice(&ip_csum.to_be_bytes());

    // TCP header.
    let t = &mut pkt[20..];
    t[0..2].copy_from_slice(&src_port.to_be_bytes());
    t[2..4].copy_from_slice(&dst_port.to_be_bytes());
    t[4..8].copy_from_slice(&seq.to_be_bytes());
    t[8..12].copy_from_slice(&ack.to_be_bytes());
    t[12] = 5 << 4; // data offset = 5 words
    t[13] = flags;
    t[14..16].copy_from_slice(&8192u16.to_be_bytes()); // window
    t[20..].copy_from_slice(payload);
    let tcp_csum = tcp_checksum(&src_ip, &dst_ip, &pkt[20..]);
    pkt[36..38].copy_from_slice(&tcp_csum.to_be_bytes());

    pkt
}

fn checksum(data: &[u8]) -> u16 {
    let mut sum: u32 = 0;
    let mut i = 0;
    while i + 1 < data.len() {
        sum += u16::from_be_bytes([data[i], data[i + 1]]) as u32;
        i += 2;
    }
    if i < data.len() {
        sum += (data[i] as u32) << 8;
    }
    while sum >> 16 != 0 {
        sum = (sum & 0xFFFF) + (sum >> 16);
    }
    !(sum as u16)
}

fn tcp_checksum(src: &Ipv4Addr, dst: &Ipv4Addr, tcp: &[u8]) -> u16 {
    let mut pseudo = Vec::with_capacity(12 + tcp.len());
    pseudo.extend_from_slice(&src.octets());
    pseudo.extend_from_slice(&dst.octets());
    pseudo.push(0);
    pseudo.push(IP_PROTO_TCP);
    pseudo.extend_from_slice(&(tcp.len() as u16).to_be_bytes());
    pseudo.extend_from_slice(tcp);
    checksum(&pseudo)
}

#[cfg(test)]
mod tests {
    use super::*;

    const IWF: Ipv4Addr = Ipv4Addr::new(10, 55, 0, 1);
    const MS: Ipv4Addr = Ipv4Addr::new(10, 55, 0, 2);

    fn syn(seq: u32, sport: u16) -> Vec<u8> {
        build_ipv4_tcp(MS, IWF, sport, MODEM_SERVER_PORT, seq, 0, TCP_SYN, &[])
    }
    fn data(seq: u32, ack: u32, sport: u16, payload: &[u8]) -> Vec<u8> {
        build_ipv4_tcp(
            MS,
            IWF,
            sport,
            MODEM_SERVER_PORT,
            seq,
            ack,
            TCP_PSH | TCP_ACK,
            payload,
        )
    }

    fn first_downlink_payload(out: &EndpointOut) -> Vec<u8> {
        for p in &out.downlink {
            let seg = parse_ipv4_tcp(p).unwrap();
            if !seg.payload.is_empty() {
                return seg.payload;
            }
        }
        Vec::new()
    }

    #[test]
    fn syn_gets_syn_ack() {
        let mut ep = Tcp380Endpoint::new(IWF, MS);
        let out = ep.on_uplink_ip(&syn(1000, 40000));
        assert_eq!(out.downlink.len(), 1);
        let seg = parse_ipv4_tcp(&out.downlink[0]).unwrap();
        assert_eq!(seg.flags & (TCP_SYN | TCP_ACK), TCP_SYN | TCP_ACK);
        assert_eq!(seg.src_port, MODEM_SERVER_PORT);
    }

    #[test]
    fn checksums_are_valid() {
        let pkt = syn(1, 40000);
        assert_eq!(checksum(&pkt[0..20]), 0); // IP header checksum verifies to 0
        assert_eq!(tcp_checksum(&MS, &IWF, &pkt[20..]), 0);
    }

    #[test]
    fn full_handshake_dial_and_connect() {
        let mut ep = Tcp380Endpoint::new(IWF, MS);
        let sport = 41000u16;
        // SYN -> SYN-ACK.
        ep.on_uplink_ip(&syn(2000, sport));
        // ACK completing handshake (seq = 2001, our ISN+1 acked).
        let ack = build_ipv4_tcp(
            MS,
            IWF,
            sport,
            MODEM_SERVER_PORT,
            2001,
            IWF_INITIAL_SEQ + 1,
            TCP_ACK,
            &[],
        );
        ep.on_uplink_ip(&ack);

        // Config + dial over TCP.
        let mut seq = 2001u32;
        let out = ep.on_uplink_ip(&data(seq, IWF_INITIAL_SEQ + 1, sport, b"AT+CFG\r"));
        seq += "AT+CFG\r".len() as u32;
        // OK comes back framed in 617.
        let mut d = cdma_modem::tia617::Decoder::new();
        let items = d.feed(&first_downlink_payload(&out));
        assert!(items.iter().any(|it| matches!(
            it,
            cdma_modem::tia617::Item::Construct { string, .. } if string == b"OK"
        )));

        let out = ep.on_uplink_ip(&data(seq, IWF_INITIAL_SEQ + 1, sport, b"ATDT5551212\r"));
        assert_eq!(out.dialed.as_deref(), Some("5551212"));

        // Data path connects -> CONNECT downstream.
        let out = ep.carrier_up();
        let mut d = cdma_modem::tia617::Decoder::new();
        let items = d.feed(&first_downlink_payload(&out));
        assert!(items.iter().any(|it| matches!(
            it,
            cdma_modem::tia617::Item::Construct { string, .. } if string == b"CONNECT 14400"
        )));
        assert!(ep.is_connected());
    }

    #[test]
    fn online_user_data_surfaces() {
        let mut ep = Tcp380Endpoint::new(IWF, MS);
        let sport = 42000u16;
        ep.on_uplink_ip(&syn(0, sport));
        ep.on_uplink_ip(&build_ipv4_tcp(
            MS,
            IWF,
            sport,
            MODEM_SERVER_PORT,
            1,
            IWF_INITIAL_SEQ + 1,
            TCP_ACK,
            &[],
        ));
        ep.on_uplink_ip(&data(1, IWF_INITIAL_SEQ + 1, sport, b"ATD1\r"));
        ep.carrier_up();
        let out = ep.on_uplink_ip(&data(6, IWF_INITIAL_SEQ + 1, sport, b"\x7e\xff\x03ppp"));
        assert_eq!(out.user_data, b"\x7e\xff\x03ppp");
    }

    #[test]
    fn packet_to_other_port_ignored() {
        let mut ep = Tcp380Endpoint::new(IWF, MS);
        let other = build_ipv4_tcp(MS, IWF, 40000, 80, 1, 0, TCP_SYN, &[]);
        assert_eq!(ep.on_uplink_ip(&other), EndpointOut::default());
    }
}
