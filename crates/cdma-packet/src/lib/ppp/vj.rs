//! Van Jacobson TCP/IP header compression for PPP IP-Compression-Protocol.
//!
//! RFC 1144 defines two packet forms carried by PPP as protocols 0x002d
//! (compressed TCP/IP) and 0x002f (uncompressed TCP/IP with the IPv4 protocol
//! octet temporarily replaced by the VJ connection id).

use super::framing::PppPacket;

pub const PPP_IP_PROTOCOL: u16 = 0x0021;
pub const PPP_VJ_COMPRESSED_TCP: u16 = 0x002d;
pub const PPP_VJ_UNCOMPRESSED_TCP: u16 = 0x002f;
pub const VJ_COMPRESSION_PROTOCOL: u16 = PPP_VJ_COMPRESSED_TCP;

const IPPROTO_TCP: u8 = 6;
const NEW_C: u8 = 0x40;
const NEW_I: u8 = 0x20;
const TCP_PUSH_BIT: u8 = 0x10;
const NEW_S: u8 = 0x08;
const NEW_A: u8 = 0x04;
const NEW_W: u8 = 0x02;
const NEW_U: u8 = 0x01;
const SPECIAL_I: u8 = NEW_S | NEW_W | NEW_U;
const SPECIAL_D: u8 = NEW_S | NEW_A | NEW_W | NEW_U;
const SPECIALS_MASK: u8 = NEW_S | NEW_A | NEW_W | NEW_U;

const TCP_FIN: u8 = 0x01;
const TCP_SYN: u8 = 0x02;
const TCP_RST: u8 = 0x04;
const TCP_PUSH: u8 = 0x08;
const TCP_ACK: u8 = 0x10;
const TCP_URG: u8 = 0x20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VjCompressionOptions {
    pub max_slot_id: u8,
    pub comp_slot_id: bool,
}

impl Default for VjCompressionOptions {
    fn default() -> Self {
        Self {
            max_slot_id: 15,
            comp_slot_id: true,
        }
    }
}

impl VjCompressionOptions {
    pub fn slot_count(self) -> usize {
        usize::from(self.max_slot_id) + 1
    }

    pub fn to_ipcp_data(self) -> [u8; 4] {
        [
            (VJ_COMPRESSION_PROTOCOL >> 8) as u8,
            (VJ_COMPRESSION_PROTOCOL & 0xff) as u8,
            self.max_slot_id,
            u8::from(self.comp_slot_id),
        ]
    }

    pub fn from_ipcp_data(data: &[u8]) -> Option<Self> {
        if data.len() != 4 {
            return None;
        }
        let protocol = u16::from_be_bytes([data[0], data[1]]);
        if protocol != VJ_COMPRESSION_PROTOCOL || data[3] > 1 {
            return None;
        }
        Some(Self {
            max_slot_id: data[2],
            comp_slot_id: data[3] != 0,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum VjError {
    NotNegotiated,
    Malformed,
    MissingState,
    Tossing,
}

#[derive(Debug, Clone)]
pub struct VjState {
    tx_options: Option<VjCompressionOptions>,
    rx_options: Option<VjCompressionOptions>,
    tx_states: Vec<Option<TcpHeaderState>>,
    rx_states: Vec<Option<TcpHeaderState>>,
    tx_lru: Vec<u8>,
    last_xmit: Option<u8>,
    last_recv: Option<u8>,
    toss: bool,
}

impl Default for VjState {
    fn default() -> Self {
        Self {
            tx_options: None,
            rx_options: None,
            tx_states: Vec::new(),
            rx_states: Vec::new(),
            tx_lru: Vec::new(),
            last_xmit: None,
            last_recv: None,
            toss: true,
        }
    }
}

impl VjState {
    pub fn configure(
        &mut self,
        tx_options: Option<VjCompressionOptions>,
        rx_options: Option<VjCompressionOptions>,
    ) {
        self.tx_options = tx_options;
        self.rx_options = rx_options;
        self.tx_states = vec![None; tx_options.map_or(0, VjCompressionOptions::slot_count)];
        self.rx_states = vec![None; rx_options.map_or(0, VjCompressionOptions::slot_count)];
        self.tx_lru = tx_options
            .map(|opts| (0..=opts.max_slot_id).collect())
            .unwrap_or_default();
        self.last_xmit = None;
        self.last_recv = None;
        self.toss = true;
    }

    pub fn compress_ip_packet(&mut self, ip_packet: &[u8]) -> PppPacket {
        let Some(options) = self.tx_options else {
            return ip_ppp(ip_packet);
        };
        let Some(parsed) = ParsedTcpPacket::parse(ip_packet) else {
            return ip_ppp(ip_packet);
        };
        if !is_compressible_tcp(&parsed) {
            return ip_ppp(ip_packet);
        }

        let slot = match self.find_tx_slot(&parsed) {
            Some(slot) => {
                self.mark_tx_mru(slot);
                slot
            }
            None => {
                let Some(slot) = self.allocate_tx_slot() else {
                    return ip_ppp(ip_packet);
                };
                self.tx_states[usize::from(slot)] = Some(TcpHeaderState::from_packet(&parsed));
                return self.uncompressed_tcp_packet(slot, &parsed);
            }
        };

        let state = self.tx_states[usize::from(slot)]
            .as_ref()
            .expect("existing tx slot must have state");
        let Some(mut compressed) = build_compressed_packet(&parsed, state, options, self.last_xmit)
        else {
            self.tx_states[usize::from(slot)] = Some(TcpHeaderState::from_packet(&parsed));
            return self.uncompressed_tcp_packet(slot, &parsed);
        };

        if !options.comp_slot_id || self.last_xmit != Some(slot) {
            compressed.payload.insert(1, slot);
            compressed.payload[0] |= NEW_C;
            self.last_xmit = Some(slot);
        }
        self.tx_states[usize::from(slot)] = Some(TcpHeaderState::from_packet(&parsed));
        compressed
    }

    pub fn decompress_packet(&mut self, protocol: u16, payload: &[u8]) -> Result<Vec<u8>, VjError> {
        match protocol {
            PPP_IP_PROTOCOL => Ok(payload.to_vec()),
            PPP_VJ_UNCOMPRESSED_TCP => self.decompress_uncompressed_tcp(payload),
            PPP_VJ_COMPRESSED_TCP => self.decompress_compressed_tcp(payload),
            _ => Err(VjError::Malformed),
        }
    }

    fn uncompressed_tcp_packet(&mut self, slot: u8, parsed: &ParsedTcpPacket<'_>) -> PppPacket {
        self.last_xmit = Some(slot);
        let mut payload = parsed.packet[..parsed.total_len].to_vec();
        payload[9] = slot;
        PppPacket {
            protocol: PPP_VJ_UNCOMPRESSED_TCP,
            payload,
        }
    }

    fn decompress_uncompressed_tcp(&mut self, payload: &[u8]) -> Result<Vec<u8>, VjError> {
        let Some(options) = self.rx_options else {
            return Err(VjError::NotNegotiated);
        };
        let mut packet = payload.to_vec();
        let Some(slot) = packet.get(9).copied() else {
            self.toss = true;
            return Err(VjError::Malformed);
        };
        if slot > options.max_slot_id {
            self.toss = true;
            return Err(VjError::Malformed);
        }
        packet[9] = IPPROTO_TCP;
        let Some(parsed) = ParsedTcpPacket::parse(&packet) else {
            self.toss = true;
            return Err(VjError::Malformed);
        };
        self.rx_states[usize::from(slot)] = Some(TcpHeaderState::from_packet(&parsed));
        self.last_recv = Some(slot);
        self.toss = false;
        Ok(packet)
    }

    fn decompress_compressed_tcp(&mut self, payload: &[u8]) -> Result<Vec<u8>, VjError> {
        let Some(options) = self.rx_options else {
            return Err(VjError::NotNegotiated);
        };
        let mut cursor = 0usize;
        let Some(mut changes) = read_u8(payload, &mut cursor) else {
            self.toss = true;
            return Err(VjError::Malformed);
        };

        if changes & NEW_C != 0 {
            let Some(slot) = read_u8(payload, &mut cursor) else {
                self.toss = true;
                return Err(VjError::Malformed);
            };
            if slot > options.max_slot_id {
                self.toss = true;
                return Err(VjError::Malformed);
            }
            self.last_recv = Some(slot);
            self.toss = false;
            changes &= !NEW_C;
        } else if self.toss {
            return Err(VjError::Tossing);
        }

        let Some(slot) = self.last_recv else {
            self.toss = true;
            return Err(VjError::MissingState);
        };
        let Some(mut state) = self.rx_states[usize::from(slot)].clone() else {
            self.toss = true;
            return Err(VjError::MissingState);
        };
        if payload.len().saturating_sub(cursor) < 2 {
            self.toss = true;
            return Err(VjError::Malformed);
        }
        let checksum = u16::from_be_bytes([payload[cursor], payload[cursor + 1]]);
        cursor += 2;

        let header_len = state.header.len();
        let ip_hlen = usize::from(state.header[0] & 0x0f) * 4;
        let tcp = ip_hlen;
        write_u16(&mut state.header, tcp + 16, checksum);
        if changes & TCP_PUSH_BIT != 0 {
            state.header[tcp + 13] |= TCP_PUSH;
        } else {
            state.header[tcp + 13] &= !TCP_PUSH;
        }

        match changes & SPECIALS_MASK {
            SPECIAL_I => {
                let data_len = usize::from(read_u16(&state.header, 2)).saturating_sub(header_len);
                add_u32(&mut state.header, tcp + 8, data_len as u32);
                add_u32(&mut state.header, tcp + 4, data_len as u32);
            }
            SPECIAL_D => {
                let data_len = usize::from(read_u16(&state.header, 2)).saturating_sub(header_len);
                add_u32(&mut state.header, tcp + 4, data_len as u32);
            }
            _ => {
                if changes & NEW_U != 0 {
                    state.header[tcp + 13] |= TCP_URG;
                    let Some(value) = decode_value(payload, &mut cursor) else {
                        self.toss = true;
                        return Err(VjError::Malformed);
                    };
                    write_u16(&mut state.header, tcp + 18, value);
                } else {
                    state.header[tcp + 13] &= !TCP_URG;
                }
                if changes & NEW_W != 0 {
                    let Some(delta) = decode_value(payload, &mut cursor) else {
                        self.toss = true;
                        return Err(VjError::Malformed);
                    };
                    add_u16(&mut state.header, tcp + 14, delta);
                }
                if changes & NEW_A != 0 {
                    let Some(delta) = decode_value(payload, &mut cursor) else {
                        self.toss = true;
                        return Err(VjError::Malformed);
                    };
                    add_u32(&mut state.header, tcp + 8, u32::from(delta));
                }
                if changes & NEW_S != 0 {
                    let Some(delta) = decode_value(payload, &mut cursor) else {
                        self.toss = true;
                        return Err(VjError::Malformed);
                    };
                    add_u32(&mut state.header, tcp + 4, u32::from(delta));
                }
            }
        }

        if changes & NEW_I != 0 {
            let Some(delta) = decode_value(payload, &mut cursor) else {
                self.toss = true;
                return Err(VjError::Malformed);
            };
            add_u16(&mut state.header, 4, delta);
        } else {
            add_u16(&mut state.header, 4, 1);
        }

        let data = &payload[cursor..];
        let total_len = header_len + data.len();
        if total_len > u16::MAX as usize {
            self.toss = true;
            return Err(VjError::Malformed);
        }
        write_u16(&mut state.header, 2, total_len as u16);
        state.header[10] = 0;
        state.header[11] = 0;
        let ip_sum = checksum16(&[&state.header[..ip_hlen]]);
        write_u16(&mut state.header, 10, ip_sum);

        let mut packet = Vec::with_capacity(total_len);
        packet.extend_from_slice(&state.header);
        packet.extend_from_slice(data);
        self.rx_states[usize::from(slot)] = Some(state);
        Ok(packet)
    }

    fn find_tx_slot(&self, parsed: &ParsedTcpPacket<'_>) -> Option<u8> {
        self.tx_lru.iter().copied().find(|slot| {
            self.tx_states[usize::from(*slot)]
                .as_ref()
                .is_some_and(|s| s.same_flow(parsed))
        })
    }

    fn allocate_tx_slot(&mut self) -> Option<u8> {
        let slot = *self.tx_lru.last()?;
        self.mark_tx_mru(slot);
        Some(slot)
    }

    fn mark_tx_mru(&mut self, slot: u8) {
        if let Some(pos) = self.tx_lru.iter().position(|s| *s == slot) {
            self.tx_lru.remove(pos);
        }
        self.tx_lru.insert(0, slot);
    }
}

fn ip_ppp(ip_packet: &[u8]) -> PppPacket {
    PppPacket {
        protocol: PPP_IP_PROTOCOL,
        payload: ip_packet.to_vec(),
    }
}

#[derive(Debug, Clone)]
struct TcpHeaderState {
    header: Vec<u8>,
}

impl TcpHeaderState {
    fn from_packet(parsed: &ParsedTcpPacket<'_>) -> Self {
        Self {
            header: parsed.packet[..parsed.header_len].to_vec(),
        }
    }

    fn same_flow(&self, parsed: &ParsedTcpPacket<'_>) -> bool {
        let old_tcp = usize::from(self.header[0] & 0x0f) * 4;
        self.header.get(12..20) == parsed.packet.get(12..20)
            && self.header.get(old_tcp..old_tcp + 4)
                == parsed.packet.get(parsed.tcp_offset..parsed.tcp_offset + 4)
    }

    fn stable_header_matches(&self, parsed: &ParsedTcpPacket<'_>) -> bool {
        let old_ip_hlen = usize::from(self.header[0] & 0x0f) * 4;
        let old_tcp = old_ip_hlen;
        self.header.len() == parsed.header_len
            && self.header[0..2] == parsed.packet[0..2]
            && self.header[6..10] == parsed.packet[6..10]
            && self.header[old_tcp + 12] == parsed.packet[parsed.tcp_offset + 12]
            && self.header[20..old_ip_hlen] == parsed.packet[20..parsed.ip_header_len]
            && self.header[old_tcp + 20..self.header.len()]
                == parsed.packet[parsed.tcp_offset + 20..parsed.header_len]
    }
}

#[derive(Debug, Clone, Copy)]
struct ParsedTcpPacket<'a> {
    packet: &'a [u8],
    total_len: usize,
    ip_header_len: usize,
    tcp_offset: usize,
    header_len: usize,
}

impl<'a> ParsedTcpPacket<'a> {
    fn parse(packet: &'a [u8]) -> Option<Self> {
        if packet.len() < 40 || packet[0] >> 4 != 4 {
            return None;
        }
        let ip_header_len = usize::from(packet[0] & 0x0f) * 4;
        if ip_header_len < 20 || packet.len() < ip_header_len {
            return None;
        }
        let total_len = usize::from(read_u16(packet, 2));
        if total_len < ip_header_len + 20 || total_len > packet.len() {
            return None;
        }
        if packet[9] != IPPROTO_TCP {
            return None;
        }
        let tcp_offset = ip_header_len;
        let tcp_header_len = usize::from(packet[tcp_offset + 12] >> 4) * 4;
        if tcp_header_len < 20 || total_len < tcp_offset + tcp_header_len {
            return None;
        }
        Some(Self {
            packet,
            total_len,
            ip_header_len,
            tcp_offset,
            header_len: tcp_offset + tcp_header_len,
        })
    }
}

fn is_compressible_tcp(parsed: &ParsedTcpPacket<'_>) -> bool {
    let fragment = read_u16(parsed.packet, 6);
    if fragment & 0x3fff != 0 {
        return false;
    }
    let flags = parsed.packet[parsed.tcp_offset + 13];
    flags & (TCP_SYN | TCP_FIN | TCP_RST | TCP_ACK) == TCP_ACK
}

fn build_compressed_packet(
    parsed: &ParsedTcpPacket<'_>,
    state: &TcpHeaderState,
    _options: VjCompressionOptions,
    _last_xmit: Option<u8>,
) -> Option<PppPacket> {
    if !state.stable_header_matches(parsed) {
        return None;
    }
    let tcp = parsed.tcp_offset;
    let old_tcp = usize::from(state.header[0] & 0x0f) * 4;
    let mut changes = 0u8;
    let mut encoded = Vec::new();

    let flags = parsed.packet[tcp + 13];
    let old_flags = state.header[old_tcp + 13];
    if flags & TCP_URG != 0 {
        encode_zero_allowed(read_u16(parsed.packet, tcp + 18), &mut encoded);
        changes |= NEW_U;
    } else if read_u16(parsed.packet, tcp + 18) != read_u16(&state.header, old_tcp + 18) {
        return None;
    }
    if (flags ^ old_flags) & !(TCP_PUSH | TCP_URG) != 0 {
        return None;
    }

    let window_delta =
        read_u16(parsed.packet, tcp + 14).wrapping_sub(read_u16(&state.header, old_tcp + 14));
    if window_delta != 0 {
        encode_nonzero(window_delta, &mut encoded);
        changes |= NEW_W;
    }

    let ack_delta =
        read_u32(parsed.packet, tcp + 8).wrapping_sub(read_u32(&state.header, old_tcp + 8));
    if ack_delta != 0 {
        if ack_delta > u16::MAX as u32 {
            return None;
        }
        encode_nonzero(ack_delta as u16, &mut encoded);
        changes |= NEW_A;
    }

    let seq_delta =
        read_u32(parsed.packet, tcp + 4).wrapping_sub(read_u32(&state.header, old_tcp + 4));
    if seq_delta != 0 {
        if seq_delta > u16::MAX as u32 {
            return None;
        }
        encode_nonzero(seq_delta as u16, &mut encoded);
        changes |= NEW_S;
    }

    let old_payload_len =
        usize::from(read_u16(&state.header, 2)).saturating_sub(state.header.len());
    match changes {
        0 => {
            if read_u16(parsed.packet, 2) == read_u16(&state.header, 2)
                || usize::from(read_u16(&state.header, 2)) != parsed.header_len
            {
                return None;
            }
        }
        SPECIAL_I | SPECIAL_D => return None,
        _ if changes == (NEW_S | NEW_A)
            && seq_delta == ack_delta
            && seq_delta == old_payload_len as u32 =>
        {
            changes = SPECIAL_I;
            encoded.clear();
        }
        _ if changes == NEW_S && seq_delta == old_payload_len as u32 => {
            changes = SPECIAL_D;
            encoded.clear();
        }
        _ => {}
    }

    let ip_id_delta = read_u16(parsed.packet, 4).wrapping_sub(read_u16(&state.header, 4));
    if ip_id_delta != 1 {
        encode_zero_allowed(ip_id_delta, &mut encoded);
        changes |= NEW_I;
    }
    if flags & TCP_PUSH != 0 {
        changes |= TCP_PUSH_BIT;
    }

    let mut payload = Vec::with_capacity(3 + encoded.len() + parsed.total_len - parsed.header_len);
    payload.push(changes);
    payload.extend_from_slice(&parsed.packet[tcp + 16..tcp + 18]);
    payload.extend_from_slice(&encoded);
    payload.extend_from_slice(&parsed.packet[parsed.header_len..parsed.total_len]);
    Some(PppPacket {
        protocol: PPP_VJ_COMPRESSED_TCP,
        payload,
    })
}

fn read_u8(data: &[u8], cursor: &mut usize) -> Option<u8> {
    let byte = *data.get(*cursor)?;
    *cursor += 1;
    Some(byte)
}

fn read_u16(data: &[u8], offset: usize) -> u16 {
    u16::from_be_bytes([data[offset], data[offset + 1]])
}

fn read_u32(data: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes([
        data[offset],
        data[offset + 1],
        data[offset + 2],
        data[offset + 3],
    ])
}

fn write_u16(data: &mut [u8], offset: usize, value: u16) {
    data[offset..offset + 2].copy_from_slice(&value.to_be_bytes());
}

fn write_u32(data: &mut [u8], offset: usize, value: u32) {
    data[offset..offset + 4].copy_from_slice(&value.to_be_bytes());
}

fn add_u16(data: &mut [u8], offset: usize, delta: u16) {
    let value = read_u16(data, offset).wrapping_add(delta);
    write_u16(data, offset, value);
}

fn add_u32(data: &mut [u8], offset: usize, delta: u32) {
    let value = read_u32(data, offset).wrapping_add(delta);
    write_u32(data, offset, value);
}

fn encode_nonzero(value: u16, out: &mut Vec<u8>) {
    debug_assert_ne!(value, 0);
    if value >= 256 {
        out.push(0);
        out.extend_from_slice(&value.to_be_bytes());
    } else {
        out.push(value as u8);
    }
}

fn encode_zero_allowed(value: u16, out: &mut Vec<u8>) {
    if value == 0 || value >= 256 {
        out.push(0);
        out.extend_from_slice(&value.to_be_bytes());
    } else {
        out.push(value as u8);
    }
}

fn decode_value(data: &[u8], cursor: &mut usize) -> Option<u16> {
    let first = read_u8(data, cursor)?;
    if first == 0 {
        if data.len().saturating_sub(*cursor) < 2 {
            return None;
        }
        let value = u16::from_be_bytes([data[*cursor], data[*cursor + 1]]);
        *cursor += 2;
        Some(value)
    } else {
        Some(u16::from(first))
    }
}

fn checksum16(parts: &[&[u8]]) -> u16 {
    let mut sum = 0u32;
    for part in parts {
        let mut i = 0usize;
        while i + 1 < part.len() {
            sum = sum.wrapping_add(u16::from_be_bytes([part[i], part[i + 1]]) as u32);
            i += 2;
        }
        if i < part.len() {
            sum = sum.wrapping_add(u16::from_be_bytes([part[i], 0]) as u32);
        }
    }
    while (sum >> 16) != 0 {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tcp_packet(seq: u32, ack: u32, ip_id: u16, flags: u8, payload: &[u8]) -> Vec<u8> {
        let total_len = 40 + payload.len();
        let mut packet = vec![0u8; total_len];
        packet[0] = 0x45;
        packet[2..4].copy_from_slice(&(total_len as u16).to_be_bytes());
        packet[4..6].copy_from_slice(&ip_id.to_be_bytes());
        packet[8] = 64;
        packet[9] = IPPROTO_TCP;
        packet[12..16].copy_from_slice(&[10, 0, 0, 1]);
        packet[16..20].copy_from_slice(&[10, 0, 0, 2]);
        packet[20..22].copy_from_slice(&1234u16.to_be_bytes());
        packet[22..24].copy_from_slice(&80u16.to_be_bytes());
        packet[24..28].copy_from_slice(&seq.to_be_bytes());
        packet[28..32].copy_from_slice(&ack.to_be_bytes());
        packet[32] = 5 << 4;
        packet[33] = flags;
        packet[34..36].copy_from_slice(&4096u16.to_be_bytes());
        packet[36..38].copy_from_slice(&0x1234u16.to_be_bytes());
        packet[40..].copy_from_slice(payload);
        let sum = checksum16(&[&packet[..20]]);
        packet[10..12].copy_from_slice(&sum.to_be_bytes());
        packet
    }

    #[test]
    fn uncompressed_tcp_seeds_receive_state_and_restores_protocol() {
        let packet = tcp_packet(10, 20, 1, TCP_ACK, b"");
        let mut tx = VjState::default();
        tx.configure(Some(VjCompressionOptions::default()), None);
        let ppp = tx.compress_ip_packet(&packet);
        assert_eq!(ppp.protocol, PPP_VJ_UNCOMPRESSED_TCP);
        assert_ne!(ppp.payload[9], IPPROTO_TCP);

        let mut rx = VjState::default();
        rx.configure(None, Some(VjCompressionOptions::default()));
        let restored = rx
            .decompress_packet(ppp.protocol, &ppp.payload)
            .expect("uncompressed VJ packet should restore");
        assert_eq!(restored, packet);
    }

    #[test]
    fn compressed_tcp_round_trips_after_uncompressed_seed() {
        let first = tcp_packet(100, 200, 1, TCP_ACK, b"x");
        let second = tcp_packet(101, 200, 2, TCP_ACK | TCP_PUSH, b"y");
        let mut tx = VjState::default();
        let mut rx = VjState::default();
        tx.configure(Some(VjCompressionOptions::default()), None);
        rx.configure(None, Some(VjCompressionOptions::default()));

        let seed = tx.compress_ip_packet(&first);
        assert_eq!(
            rx.decompress_packet(seed.protocol, &seed.payload).unwrap(),
            first
        );

        let compressed = tx.compress_ip_packet(&second);
        assert_eq!(compressed.protocol, PPP_VJ_COMPRESSED_TCP);
        let restored = rx
            .decompress_packet(compressed.protocol, &compressed.payload)
            .expect("compressed VJ packet should restore");
        assert_eq!(restored, second);
    }

    #[test]
    fn compressed_tcp_uses_interactive_special_i_when_seq_and_ack_advance_by_payload_len() {
        let first = tcp_packet(100, 200, 1, TCP_ACK, b"abcd");
        let second = tcp_packet(104, 204, 2, TCP_ACK, b"wxyz");
        let mut tx = VjState::default();
        tx.configure(Some(VjCompressionOptions::default()), None);

        let _seed = tx.compress_ip_packet(&first);
        let compressed = tx.compress_ip_packet(&second);

        assert_eq!(compressed.protocol, PPP_VJ_COMPRESSED_TCP);
        assert_eq!(compressed.payload[0] & SPECIALS_MASK, SPECIAL_I);
    }

    #[test]
    fn compressed_tcp_without_receive_state_is_tossed_until_uncompressed_seed() {
        let first = tcp_packet(100, 200, 1, TCP_ACK, b"x");
        let second = tcp_packet(101, 200, 2, TCP_ACK, b"y");
        let opts = VjCompressionOptions {
            comp_slot_id: false,
            ..VjCompressionOptions::default()
        };
        let mut tx = VjState::default();
        let mut rx = VjState::default();
        tx.configure(Some(opts), None);
        rx.configure(None, Some(opts));

        let seed = tx.compress_ip_packet(&first);
        let compressed = tx.compress_ip_packet(&second);
        assert_eq!(compressed.protocol, PPP_VJ_COMPRESSED_TCP);
        assert!(compressed.payload[0] & NEW_C != 0);
        assert_eq!(
            rx.decompress_packet(compressed.protocol, &compressed.payload),
            Err(VjError::MissingState)
        );

        assert_eq!(
            rx.decompress_packet(seed.protocol, &seed.payload).unwrap(),
            first
        );
        let restored = rx
            .decompress_packet(compressed.protocol, &compressed.payload)
            .expect("compressed packet with NEW_C should restore after uncompressed seed");
        assert_eq!(restored, second);
    }

    #[test]
    fn malformed_vj_packets_are_rejected() {
        let packet = tcp_packet(10, 20, 1, TCP_ACK, b"");
        let mut rx = VjState::default();
        rx.configure(None, Some(VjCompressionOptions::default()));

        assert_eq!(
            rx.decompress_packet(PPP_VJ_UNCOMPRESSED_TCP, &[0; 9]),
            Err(VjError::Malformed)
        );

        let mut bad_slot = packet.clone();
        bad_slot[9] = 16;
        assert_eq!(
            rx.decompress_packet(PPP_VJ_UNCOMPRESSED_TCP, &bad_slot),
            Err(VjError::Malformed)
        );

        let mut seed = packet;
        seed[9] = 0;
        assert!(rx.decompress_packet(PPP_VJ_UNCOMPRESSED_TCP, &seed).is_ok());
        assert_eq!(
            rx.decompress_packet(PPP_VJ_COMPRESSED_TCP, &[0x00, 0x12]),
            Err(VjError::Malformed)
        );
        assert_eq!(
            rx.decompress_packet(PPP_VJ_COMPRESSED_TCP, &[NEW_C, 16, 0x12, 0x34]),
            Err(VjError::Malformed)
        );
    }

    #[test]
    fn comp_slot_id_false_sends_connection_id_on_every_compressed_packet() {
        let opts = VjCompressionOptions {
            comp_slot_id: false,
            ..VjCompressionOptions::default()
        };
        let first = tcp_packet(100, 200, 1, TCP_ACK, b"x");
        let second = tcp_packet(101, 200, 2, TCP_ACK, b"y");
        let third = tcp_packet(102, 200, 3, TCP_ACK, b"z");
        let mut tx = VjState::default();
        tx.configure(Some(opts), None);
        let _ = tx.compress_ip_packet(&first);
        let second_ppp = tx.compress_ip_packet(&second);
        let third_ppp = tx.compress_ip_packet(&third);
        assert_ne!(second_ppp.protocol, PPP_IP_PROTOCOL);
        assert_ne!(third_ppp.protocol, PPP_IP_PROTOCOL);
        assert!(second_ppp.payload[0] & NEW_C != 0);
        assert!(third_ppp.payload[0] & NEW_C != 0);
    }
}
