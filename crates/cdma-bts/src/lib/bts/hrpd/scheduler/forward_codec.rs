//! HRPD forward-traffic packet codec: Format-B session-packet packing and
//! parsing plus the Default Signaling packet parse used by the scheduler.
//!
//! Split out of the scheduler module; shared bit helpers stay in the parent
//! and are reached through `super`.

use super::*;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct DefaultSignalingPacket {
    pub(super) protocol_type: u8,
    pub(super) payload: Vec<u8>,
    pub(super) reliable_sequence_number: Option<u8>,
    pub(super) ack_sequence_number: Option<u8>,
    pub(super) in_configuration: bool,
}

pub(super) fn default_signaling_packet(payload: &[u8]) -> Option<DefaultSignalingPacket> {
    let session_packets = forward_format_b_session_packets(payload)?;
    session_packets
        .iter()
        .find_map(|packet| parse_default_signaling_packet(packet))
}

pub(super) fn forward_format_b_session_packets(payload: &[u8]) -> Option<Vec<Vec<u8>>> {
    for format in forward_payload_formats(payload.len()) {
        if let Some(session_packets) = forward_format_b_session_packets_with_format(payload, format)
        {
            return Some(session_packets);
        }
    }
    None
}

fn forward_format_b_session_packets_with_format(
    payload: &[u8],
    format: ForwardPayloadFormat,
) -> Option<Vec<Vec<u8>>> {
    if payload.len() <= format.fcs_bits + PHYSICAL_TAIL_BITS + FORWARD_TRAFFIC_MAC_TRAILER_BITS {
        return None;
    }
    if payload[payload.len() - PHYSICAL_TAIL_BITS..]
        .iter()
        .any(|bit| bit & 1 != 0)
    {
        return None;
    }

    let mac_bits = payload.len() - format.fcs_bits - PHYSICAL_TAIL_BITS;
    let fcs_start = mac_bits;
    let fcs_end = fcs_start + format.fcs_bits;
    match format.fcs_bits {
        PHYSICAL_FCS_BITS => {
            let expected_fcs = bits_to_u16(&payload[fcs_start..fcs_end]);
            if expected_fcs != physical_crc16(&payload[..mac_bits]) {
                return None;
            }
        }
        ENHANCED_PHYSICAL_FCS_BITS => {
            let expected_fcs = bits_to_u24(&payload[fcs_start..fcs_end]);
            if expected_fcs != physical_crc24(&payload[..mac_bits]) {
                return None;
            }
        }
        _ => return None,
    }

    if mac_bits < format.mac_packet_bits {
        return None;
    }
    if format.fcs_bits == PHYSICAL_FCS_BITS
        && format.mac_packet_bits == FORWARD_TRAFFIC_MAC_PACKET_BITS
    {
        return legacy_forward_format_b_session_packets(payload, mac_bits);
    }
    forward_mac_format_b_session_packets(payload, 0, format.mac_packet_bits)
}

fn legacy_forward_format_b_session_packets(
    payload: &[u8],
    mac_bits: usize,
) -> Option<Vec<Vec<u8>>> {
    let mac_packets = (mac_bits + FORWARD_TRAFFIC_MAC_PAD_BITS)
        .checked_div(FORWARD_TRAFFIC_MAC_PACKET_BITS + FORWARD_TRAFFIC_MAC_PAD_BITS)?;
    if mac_packets == 0
        || mac_bits
            != mac_packets * FORWARD_TRAFFIC_MAC_PACKET_BITS
                + mac_packets.saturating_sub(1) * FORWARD_TRAFFIC_MAC_PAD_BITS
    {
        return None;
    }
    let mut packets = Vec::new();
    for mac_index in 0..mac_packets {
        let mac_start =
            mac_index * (FORWARD_TRAFFIC_MAC_PACKET_BITS + FORWARD_TRAFFIC_MAC_PAD_BITS);
        if mac_index > 0
            && payload[mac_start - FORWARD_TRAFFIC_MAC_PAD_BITS..mac_start]
                .iter()
                .any(|bit| bit & 1 != 0)
        {
            return None;
        }
        let mac_packets = forward_mac_format_b_session_packets(
            payload,
            mac_start,
            FORWARD_TRAFFIC_MAC_PACKET_BITS,
        )?;
        packets.extend(mac_packets);
    }
    (!packets.is_empty()).then_some(packets)
}

fn forward_mac_format_b_session_packets(
    payload: &[u8],
    mac_start: usize,
    mac_packet_bits: usize,
) -> Option<Vec<Vec<u8>>> {
    let security_bits = mac_packet_bits - FORWARD_TRAFFIC_MAC_TRAILER_BITS;
    let trailer = mac_start + security_bits;
    let mac_end = mac_start + mac_packet_bits;
    if mac_end > payload.len() {
        return None;
    }
    let connection_layer_format_b = payload[trailer] & 1 != 0;
    let mac_layer_format_valid = payload[trailer + 1] & 1 != 0;
    if !mac_layer_format_valid {
        let all_zero = payload[mac_start..mac_end].iter().all(|bit| bit & 1 == 0);
        return all_zero.then(Vec::new);
    }
    if !connection_layer_format_b {
        return None;
    }
    parse_connection_format_b_packets(&payload[mac_start..trailer]).ok()
}

pub(super) fn rebuild_or_split_format_b_ftc_payloads(
    payload: &[u8],
    physical_packet_bits: usize,
    forward_traffic_mac_subtype: u16,
) -> Option<Vec<Vec<u8>>> {
    let session_packets = forward_format_b_session_packets(payload)?;
    forward_traffic_security_capacity_bits_for_mac_subtype(
        physical_packet_bits,
        forward_traffic_mac_subtype,
    )?;
    if let Ok(payload) = format_b_ftc_payload_bits_for_mac_subtype(
        &session_packets,
        physical_packet_bits,
        forward_traffic_mac_subtype,
    ) {
        return Some(vec![payload]);
    }

    let mut out = Vec::new();
    let mut remaining = session_packets.as_slice();
    while !remaining.is_empty() {
        let mut rebuilt = None;
        for count in (1..=remaining.len()).rev() {
            if let Ok(payload) = format_b_ftc_payload_bits_for_mac_subtype(
                &remaining[..count],
                physical_packet_bits,
                forward_traffic_mac_subtype,
            ) {
                rebuilt = Some((count, payload));
                break;
            }
        }
        let (count, payload) = rebuilt?;
        out.push(payload);
        remaining = &remaining[count..];
    }
    (!out.is_empty()).then_some(out)
}

#[derive(Debug, Clone, Copy)]
struct ForwardPayloadFormat {
    mac_packet_bits: usize,
    fcs_bits: usize,
}

const LEGACY_FORWARD_PAYLOAD_FORMAT: ForwardPayloadFormat = ForwardPayloadFormat {
    mac_packet_bits: FORWARD_TRAFFIC_MAC_PACKET_BITS,
    fcs_bits: PHYSICAL_FCS_BITS,
};

fn forward_payload_formats(physical_packet_bits: usize) -> Vec<ForwardPayloadFormat> {
    let mut formats = Vec::with_capacity(2);
    match physical_packet_bits {
        1024 | 2048 | 3072 | 4096 => formats.push(LEGACY_FORWARD_PAYLOAD_FORMAT),
        _ => {}
    }
    if let Some(mac_packet_bits) = enhanced_forward_traffic_mac_packet_bits(physical_packet_bits) {
        formats.push(ForwardPayloadFormat {
            mac_packet_bits,
            fcs_bits: ENHANCED_PHYSICAL_FCS_BITS,
        });
    }
    formats
}

fn parse_default_signaling_packet(packet: &[u8]) -> Option<DefaultSignalingPacket> {
    let bits = bytes_to_bits(packet);
    let mut offset = 0usize;
    let stream = read_bits(&bits, &mut offset, 2)? as u8;
    let _reserved = read_bits(&bits, &mut offset, 4)?;
    let fragmented = read_bits(&bits, &mut offset, 1)?;
    if stream != 0 || fragmented != 0 {
        return None;
    }

    let full_slp_d_header = read_bits(&bits, &mut offset, 1)?;
    let mut reliable_sequence_number = None;
    let mut ack_sequence_number = None;
    if full_slp_d_header != 0 {
        let ack_sequence_valid = read_bits(&bits, &mut offset, 1)?;
        let ack_sequence = read_bits(&bits, &mut offset, 3)? as u8;
        let sequence_valid = read_bits(&bits, &mut offset, 1)?;
        let sequence_number = read_bits(&bits, &mut offset, 3)? as u8;
        if ack_sequence_valid != 0 {
            ack_sequence_number = Some(ack_sequence & SLP_SEQUENCE_MASK);
        }
        if sequence_valid != 0 {
            reliable_sequence_number = Some(sequence_number & SLP_SEQUENCE_MASK);
        }
    }

    if offset == bits.len() {
        return None;
    }
    let in_configuration = read_bits(&bits, &mut offset, 1)? != 0;
    let protocol_type = read_bits(&bits, &mut offset, 7)? as u8;
    if (bits.len() - offset) % 8 != 0 {
        return None;
    }
    Some(DefaultSignalingPacket {
        protocol_type,
        payload: pack_bits_to_bytes(&bits[offset..]),
        reliable_sequence_number,
        ack_sequence_number,
        in_configuration,
    })
}

pub(super) fn reliable_rtc_ack_sequence_number(packet: &[u8]) -> Option<u8> {
    if packet.len() != 4 {
        return None;
    }

    let bits = bytes_to_bits(packet);
    let mut offset = 0usize;
    let stream = read_bits(&bits, &mut offset, 2)? as u8;
    let _reserved = read_bits(&bits, &mut offset, 4)?;
    let fragmented = read_bits(&bits, &mut offset, 1)?;
    let full_slp_d_header = read_bits(&bits, &mut offset, 1)?;
    if stream != 0 || fragmented != 0 || full_slp_d_header != 1 {
        return None;
    }
    let _ack_sequence_valid = read_bits(&bits, &mut offset, 1)?;
    let _ack_sequence_number = read_bits(&bits, &mut offset, 3)?;
    let sequence_valid = read_bits(&bits, &mut offset, 1)?;
    let sequence_number = read_bits(&bits, &mut offset, 3)? as u8;
    let _in_configuration = read_bits(&bits, &mut offset, 1)?;
    let protocol_type = read_bits(&bits, &mut offset, 7)? as u8;
    if sequence_valid != 1 || protocol_type != DEFAULT_REVERSE_TRAFFIC_CHANNEL_MAC_PROTOCOL_TYPE {
        return None;
    }
    if offset + 8 != bits.len() {
        return None;
    }
    let message_id = read_bits(&bits, &mut offset, 8)? as u8;
    (message_id == REVERSE_TRAFFIC_CHANNEL_MAC_RTC_ACK_MESSAGE_ID)
        .then_some(sequence_number & SLP_SEQUENCE_MASK)
}

fn bits_to_u16(bits: &[u8]) -> u16 {
    bits.iter()
        .fold(0u16, |acc, bit| (acc << 1) | u16::from(bit & 1))
}

fn bits_to_u24(bits: &[u8]) -> u32 {
    bits.iter()
        .fold(0u32, |acc, bit| (acc << 1) | u32::from(bit & 1))
}
