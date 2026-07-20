//! HRPD Default Packet Application / Forward Traffic framing helpers.
//!
//! This module stops at the physical-layer information bit stream consumed by
//! the BTS Forward Traffic scheduler. It does not choose a DRC, schedule H-ARQ,
//! or touch A8/A10 bearer state.

use crate::bits::Bitstream;
use crate::hrpd::air::{
    encode_default_signaling_packet_for_instance, encode_default_signaling_slp_d_ack_packet,
    encode_default_signaling_slp_reset_packet, encode_reliable_default_signaling_packet,
    encode_reliable_default_signaling_packet_for_instance_with_ack,
};
use crate::hrpd::messages::DEFAULT_REVERSE_TRAFFIC_CHANNEL_MAC_PROTOCOL_TYPE;

pub const DEFAULT_PACKET_STREAM_ID: u8 = 1;
pub const DEFAULT_PACKET_STREAM2_ID: u8 = 2;
pub const DEFAULT_PACKET_STREAM3_ID: u8 = 3;
pub const DEFAULT_PACKET_STREAM1_APPLICATION_PROTOCOL_TYPE: u8 = 0x15;
pub const DEFAULT_PACKET_STREAM2_APPLICATION_PROTOCOL_TYPE: u8 = 0x16;
pub const DEFAULT_PACKET_STREAM3_APPLICATION_PROTOCOL_TYPE: u8 = 0x17;
pub const DEFAULT_PACKET_RLP_SEQUENCE_BITS: usize = 22;
pub const DEFAULT_STREAM_HEADER_BITS: usize = 2;
pub const CONNECTION_FORMAT_B_LENGTH_BITS: usize = 8;
pub const FORWARD_TRAFFIC_MAC_PACKET_BITS: usize = 1002;
pub const FORWARD_TRAFFIC_MAC_PAD_BITS: usize = 22;
pub const ENHANCED_FORWARD_TRAFFIC_MAC_PACKET_128_BITS: usize = 98;
pub const ENHANCED_FORWARD_TRAFFIC_MAC_PACKET_256_BITS: usize = 226;
pub const ENHANCED_FORWARD_TRAFFIC_MAC_PACKET_512_BITS: usize = 482;
pub const ENHANCED_FORWARD_TRAFFIC_MAC_PACKET_1024_BITS: usize = 994;
pub const ENHANCED_FORWARD_TRAFFIC_MAC_PACKET_2048_BITS: usize = 2018;
pub const ENHANCED_FORWARD_TRAFFIC_MAC_PACKET_3072_BITS: usize = 3042;
pub const ENHANCED_FORWARD_TRAFFIC_MAC_PACKET_4096_BITS: usize = 4066;
pub const ENHANCED_FORWARD_TRAFFIC_MAC_PACKET_5120_BITS: usize = 5090;
pub const FORWARD_TRAFFIC_MAC_TRAILER_BITS: usize = 2;
pub const TRAFFIC_MAC_TRAILER_BITS: usize = 2;
pub const PHYSICAL_FCS_BITS: usize = 16;
pub const ENHANCED_PHYSICAL_FCS_BITS: usize = 24;
pub const PHYSICAL_TAIL_BITS: usize = 6;
pub const REVERSE_TRAFFIC_CHANNEL_MAC_RTC_ACK_MESSAGE_ID: u8 = 0x00;
pub const REVERSE_TRAFFIC_CHANNEL_MAC_GRANT_MESSAGE_ID: u8 = 0x03;
pub const REVERSE_TRAFFIC_MAC_GRANT_NUM_MAC_FLOWS_BITS: usize = 4;
pub const REVERSE_TRAFFIC_MAC_GRANT_MAC_FLOW_ID_BITS: usize = 4;
pub const REVERSE_TRAFFIC_MAC_GRANT_T2P_INFLOW_BITS: usize = 8;
pub const REVERSE_TRAFFIC_MAC_GRANT_BUCKET_LEVEL_BITS: usize = 8;
// C.S0024-300 §1.13.6.2.4: subtype-3 Grant encodes TT2PHold as a 6-bit
// field, followed by octet padding. (C.S0024-A's prose says 4-bit; its
// message table and the -C revision both say 6 — the table is normative.)
pub const REVERSE_TRAFFIC_MAC_GRANT_TT2P_HOLD_BITS: usize = 6;
pub const FORWARD_TRAFFIC_MAC_SUBTYPE_DEFAULT: u16 = 0x0000;
pub const FORWARD_TRAFFIC_MAC_SUBTYPE_ENHANCED: u16 = 0x0001;
pub const REVERSE_TRAFFIC_MAC_SUBTYPE_DEFAULT: u16 = 0x0000;
pub const REVERSE_TRAFFIC_MAC_SUBTYPE_REV0: u16 = 0x0001;
pub const REVERSE_TRAFFIC_MAC_SUBTYPE3: u16 = 0x0003;

/// HRPD Enhanced Forward Traffic MAC subtype 1 canonical physical packet size.
///
/// C.S0024-300-C Table 1.7.6.1-2 adds DRC 0xd/0xe for Enhanced FTC MAC
/// subtype 1. Default FTC MAC validity is narrower; use
/// `forward_traffic_payload_bits_for_drc_for_mac_subtype` when the negotiated
/// MAC subtype is known.
pub fn forward_traffic_payload_bits_for_drc(drc_index: u8) -> Option<usize> {
    match drc_index {
        0x1 | 0x2 | 0x3 | 0x4 | 0x6 => Some(1024),
        0x5 | 0x7 | 0x9 => Some(2048),
        0x8 | 0xb => Some(3072),
        0xa | 0xc => Some(4096),
        0xd | 0xe => Some(5120),
        _ => None,
    }
}

pub fn forward_traffic_payload_bits_for_drc_for_mac_subtype(
    drc_index: u8,
    forward_traffic_mac_subtype: u16,
) -> Option<usize> {
    match forward_traffic_mac_subtype {
        FORWARD_TRAFFIC_MAC_SUBTYPE_DEFAULT => match drc_index {
            0x1 | 0x2 | 0x3 | 0x4 | 0x6 => Some(1024),
            0x5 | 0x7 | 0x9 => Some(2048),
            0x8 | 0xb => Some(3072),
            0xa | 0xc => Some(4096),
            _ => None,
        },
        FORWARD_TRAFFIC_MAC_SUBTYPE_ENHANCED => forward_traffic_payload_bits_for_drc(drc_index),
        _ => None,
    }
}

/// Forward DRC values this transmitter can encode end-to-end today.
pub fn implemented_forward_traffic_payload_bits_for_drc(drc_index: u8) -> Option<usize> {
    forward_traffic_payload_bits_for_drc(drc_index)
}

pub fn implemented_forward_traffic_payload_bits_for_drc_for_mac_subtype(
    drc_index: u8,
    forward_traffic_mac_subtype: u16,
) -> Option<usize> {
    forward_traffic_payload_bits_for_drc_for_mac_subtype(drc_index, forward_traffic_mac_subtype)
}

pub fn forward_traffic_security_capacity_bits(physical_packet_bits: usize) -> Option<usize> {
    enhanced_forward_traffic_mac_packet_bits(physical_packet_bits)
        .map(|bits| bits - FORWARD_TRAFFIC_MAC_TRAILER_BITS)
}

pub fn legacy_forward_traffic_security_capacity_bits(physical_packet_bits: usize) -> Option<usize> {
    match physical_packet_bits {
        1024 | 2048 | 3072 | 4096 => {
            Some(FORWARD_TRAFFIC_MAC_PACKET_BITS - FORWARD_TRAFFIC_MAC_TRAILER_BITS)
        }
        _ => None,
    }
}

pub fn forward_traffic_security_capacity_bits_for_mac_subtype(
    physical_packet_bits: usize,
    forward_traffic_mac_subtype: u16,
) -> Option<usize> {
    match forward_traffic_mac_subtype {
        FORWARD_TRAFFIC_MAC_SUBTYPE_DEFAULT => {
            legacy_forward_traffic_security_capacity_bits(physical_packet_bits)
        }
        FORWARD_TRAFFIC_MAC_SUBTYPE_ENHANCED => {
            forward_traffic_security_capacity_bits(physical_packet_bits)
        }
        _ => None,
    }
}

pub fn enhanced_forward_traffic_mac_packet_bits(physical_packet_bits: usize) -> Option<usize> {
    match physical_packet_bits {
        128 => Some(ENHANCED_FORWARD_TRAFFIC_MAC_PACKET_128_BITS),
        256 => Some(ENHANCED_FORWARD_TRAFFIC_MAC_PACKET_256_BITS),
        512 => Some(ENHANCED_FORWARD_TRAFFIC_MAC_PACKET_512_BITS),
        1024 => Some(ENHANCED_FORWARD_TRAFFIC_MAC_PACKET_1024_BITS),
        2048 => Some(ENHANCED_FORWARD_TRAFFIC_MAC_PACKET_2048_BITS),
        3072 => Some(ENHANCED_FORWARD_TRAFFIC_MAC_PACKET_3072_BITS),
        4096 => Some(ENHANCED_FORWARD_TRAFFIC_MAC_PACKET_4096_BITS),
        5120 => Some(ENHANCED_FORWARD_TRAFFIC_MAC_PACKET_5120_BITS),
        _ => None,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrafficFrameError {
    InvalidStreamId(u8),
    EmptySessionPacket,
    SessionPacketTooLong(usize),
    NotOctetAligned(&'static str),
    ApplicationPacketMisaligned(usize),
    TooManyBits {
        layer: &'static str,
        bits: usize,
        capacity: usize,
    },
    PhysicalPacketTooSmall(usize),
    MacPacketTooSmall(usize),
    InvalidMacLayerFormat,
    UnsupportedConnectionLayerFormat,
    ConnectionFormatBLengthTruncated,
    ConnectionFormatBPacketTruncated {
        needed: usize,
        actual: usize,
    },
    ConnectionFormatBPaddingNonZero,
    StreamPacketTooSmall(usize),
    DefaultPacketRlpTooSmall(usize),
    DefaultPacketRlpMisaligned(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReverseTrafficMacPacket {
    pub connection_layer_format_b: bool,
    pub security_payload_bits: Vec<u8>,
    pub transmission_mode_low_latency: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HrpdStreamLayerPacket {
    pub stream_id: u8,
    pub application_packet_bits: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HrpdDefaultPacketRlpPacket {
    pub sequence: u32,
    pub payload: Vec<u8>,
}

/// Build a Default Packet Application RLP packet.
///
/// C.S0024-500 defines the Default Packet RLP header as a 22-bit SEQ field.
/// With octet IP payload this produces a payload length congruent to 6 mod 8,
/// which is exactly what the Default Stream Protocol requires before its
/// 2-bit Stream header is prepended.
pub fn default_packet_rlp_packet_bits(seq: u32, octets: &[u8]) -> Vec<u8> {
    let mut bits = Bitstream::new();
    bits.write_u32(
        seq & ((1 << DEFAULT_PACKET_RLP_SEQUENCE_BITS) - 1),
        DEFAULT_PACKET_RLP_SEQUENCE_BITS,
    );
    bits.extend(&Bitstream::new_bytes(octets));
    bits.bits().to_vec()
}

/// Prefix a Session Layer packet with the 2-bit Default Stream Protocol header.
///
/// The application packet must be 6 mod 8 bits long so the resulting Stream
/// Layer packet is octet aligned.
pub fn stream_layer_packet_bytes(
    stream_id: u8,
    application_packet_bits: &[u8],
) -> Result<Vec<u8>, TrafficFrameError> {
    if stream_id > 3 {
        return Err(TrafficFrameError::InvalidStreamId(stream_id));
    }
    if application_packet_bits.len() % 8 != 6 {
        return Err(TrafficFrameError::ApplicationPacketMisaligned(
            application_packet_bits.len(),
        ));
    }

    let mut bits = Bitstream::new();
    bits.write_u8(stream_id, DEFAULT_STREAM_HEADER_BITS);
    bits.extend(&Bitstream::new_init(application_packet_bits));
    debug_assert_eq!(bits.len() % 8, 0);
    Ok(bits.to_packed_bytes())
}

pub fn default_packet_stream_protocol_type(stream_id: u8) -> Option<u8> {
    match stream_id {
        DEFAULT_PACKET_STREAM_ID => Some(DEFAULT_PACKET_STREAM1_APPLICATION_PROTOCOL_TYPE),
        DEFAULT_PACKET_STREAM2_ID => Some(DEFAULT_PACKET_STREAM2_APPLICATION_PROTOCOL_TYPE),
        DEFAULT_PACKET_STREAM3_ID => Some(DEFAULT_PACKET_STREAM3_APPLICATION_PROTOCOL_TYPE),
        _ => None,
    }
}

/// Build a Connection Layer Format B packet.
///
/// Each Session Layer packet is prefixed with an 8-bit length in octets. The
/// packet is padded with zero bits to the maximum size selected by the lower
/// layer. The caller supplies that size as `security_payload_bits`.
pub fn connection_format_b_bits(
    session_packets: &[Vec<u8>],
    security_payload_bits: usize,
) -> Result<Vec<u8>, TrafficFrameError> {
    if security_payload_bits % 8 != 0 {
        return Err(TrafficFrameError::NotOctetAligned(
            "connection format B capacity",
        ));
    }

    let mut bits = Bitstream::new();
    for packet in session_packets {
        if packet.is_empty() {
            return Err(TrafficFrameError::EmptySessionPacket);
        }
        if packet.len() > u8::MAX as usize {
            return Err(TrafficFrameError::SessionPacketTooLong(packet.len()));
        }
        bits.write_u8(packet.len() as u8, CONNECTION_FORMAT_B_LENGTH_BITS);
        bits.extend(&Bitstream::new_bytes(packet));
    }

    if bits.len() > security_payload_bits {
        return Err(TrafficFrameError::TooManyBits {
            layer: "connection format B",
            bits: bits.len(),
            capacity: security_payload_bits,
        });
    }
    let mut out = bits.bits().to_vec();
    out.resize(security_payload_bits, 0);
    Ok(out)
}

/// Build one legacy Forward Traffic Channel physical-layer packet bitstream.
///
/// `connection_layer_packet_bits` is the Security Layer packet because the
/// negotiated security/encryption path leaves non-Access Channel packets
/// unwrapped here. The MAC trailer order is ConnectionLayerFormat, then
/// MACLayerFormat.
pub fn default_ftc_physical_payload_bits(
    connection_layer_packet_bits: &[u8],
    connection_layer_format_b: bool,
    physical_packet_bits: usize,
) -> Result<Vec<u8>, TrafficFrameError> {
    default_ftc_physical_payload_bits_from_mac_packets(
        &[(connection_layer_packet_bits, connection_layer_format_b)],
        physical_packet_bits,
    )
}

fn default_ftc_physical_payload_bits_from_mac_packets(
    mac_packets_bits: &[(&[u8], bool)],
    physical_packet_bits: usize,
) -> Result<Vec<u8>, TrafficFrameError> {
    if !matches!(physical_packet_bits, 1024 | 2048 | 3072 | 4096) {
        return Err(TrafficFrameError::PhysicalPacketTooSmall(
            physical_packet_bits,
        ));
    }
    let mac_packets = physical_packet_bits / 1024;
    let security_capacity = FORWARD_TRAFFIC_MAC_PACKET_BITS - FORWARD_TRAFFIC_MAC_TRAILER_BITS;
    if mac_packets_bits.len() > mac_packets {
        return Err(TrafficFrameError::TooManyBits {
            layer: "forward traffic MAC packets",
            bits: mac_packets_bits.len(),
            capacity: mac_packets,
        });
    }

    let mut physical = Vec::with_capacity(physical_packet_bits);
    for mac_index in 0..mac_packets {
        if mac_index > 0 {
            physical.extend(std::iter::repeat_n(0u8, FORWARD_TRAFFIC_MAC_PAD_BITS));
        }
        if let Some((connection_layer_packet_bits, connection_layer_format_b)) =
            mac_packets_bits.get(mac_index)
        {
            if connection_layer_packet_bits.len() > security_capacity {
                return Err(TrafficFrameError::TooManyBits {
                    layer: "forward traffic security packet",
                    bits: connection_layer_packet_bits.len(),
                    capacity: security_capacity,
                });
            }
            physical.extend(connection_layer_packet_bits.iter().map(|b| b & 1));
            physical.resize(
                physical.len() + security_capacity - connection_layer_packet_bits.len(),
                0,
            );
            physical.push(u8::from(*connection_layer_format_b));
            physical.push(1); // MACLayerFormat = valid payload.
        } else {
            // Null/invalid Forward Traffic MAC packet. The AT discards this MAC
            // packet via MACLayerFormat=0 after the physical FCS succeeds.
            physical.extend(std::iter::repeat_n(0u8, FORWARD_TRAFFIC_MAC_PACKET_BITS));
        }
    }

    debug_assert_eq!(
        physical.len(),
        physical_packet_bits - PHYSICAL_FCS_BITS - PHYSICAL_TAIL_BITS
    );
    // C.S0024 §9.1.4 uses the same physical-layer FCS computation for
    // Control and Forward Traffic packets. Keep this exactly aligned with the
    // working Control Channel encoder.
    let fcs = physical_crc16(&physical);
    push_u16_msb(&mut physical, fcs);
    physical.extend(std::iter::repeat_n(0u8, PHYSICAL_TAIL_BITS));
    debug_assert_eq!(physical.len(), physical_packet_bits);
    Ok(physical)
}

/// Build one Enhanced FTC subtype-1 physical-layer packet bitstream.
///
/// C.S0024-200-C Subtype 2 Physical Layer §2.2.2.3 defines a single Forward
/// Traffic MAC packet per physical packet, followed by a 24-bit physical FCS
/// and six tail bits.
pub fn enhanced_ftc_physical_payload_bits(
    connection_layer_packet_bits: &[u8],
    connection_layer_format_b: bool,
    physical_packet_bits: usize,
) -> Result<Vec<u8>, TrafficFrameError> {
    let mac_packet_bits = enhanced_forward_traffic_mac_packet_bits(physical_packet_bits).ok_or(
        TrafficFrameError::PhysicalPacketTooSmall(physical_packet_bits),
    )?;
    let security_capacity = mac_packet_bits - FORWARD_TRAFFIC_MAC_TRAILER_BITS;
    if connection_layer_packet_bits.len() > security_capacity {
        return Err(TrafficFrameError::TooManyBits {
            layer: "enhanced forward traffic security packet",
            bits: connection_layer_packet_bits.len(),
            capacity: security_capacity,
        });
    }

    let mut physical = Vec::with_capacity(physical_packet_bits);
    physical.extend(connection_layer_packet_bits.iter().map(|b| b & 1));
    physical.resize(security_capacity, 0);
    physical.push(u8::from(connection_layer_format_b));
    physical.push(1);
    debug_assert_eq!(physical.len(), mac_packet_bits);

    let fcs = physical_crc24(&physical);
    push_u24_msb(&mut physical, fcs);
    physical.extend(std::iter::repeat_n(0u8, PHYSICAL_TAIL_BITS));
    debug_assert_eq!(physical.len(), physical_packet_bits);
    Ok(physical)
}

pub fn ftc_physical_payload_bits_for_mac_subtype(
    connection_layer_packet_bits: &[u8],
    connection_layer_format_b: bool,
    physical_packet_bits: usize,
    forward_traffic_mac_subtype: u16,
) -> Result<Vec<u8>, TrafficFrameError> {
    match forward_traffic_mac_subtype {
        FORWARD_TRAFFIC_MAC_SUBTYPE_DEFAULT => default_ftc_physical_payload_bits(
            connection_layer_packet_bits,
            connection_layer_format_b,
            physical_packet_bits,
        ),
        FORWARD_TRAFFIC_MAC_SUBTYPE_ENHANCED => enhanced_ftc_physical_payload_bits(
            connection_layer_packet_bits,
            connection_layer_format_b,
            physical_packet_bits,
        ),
        _ => Err(TrafficFrameError::PhysicalPacketTooSmall(
            physical_packet_bits,
        )),
    }
}

/// Build a one-RLP-packet Default Packet Application physical packet.
///
/// This uses Connection Layer Format B so it is valid for a shorter Session
/// Layer packet padded to the selected forward traffic physical packet size.
pub fn default_packet_ftc_payload_bits(
    stream_id: u8,
    rlp_seq: u32,
    ip_packet: &[u8],
    physical_packet_bits: usize,
) -> Result<Vec<u8>, TrafficFrameError> {
    default_packet_ftc_payload_bits_for_mac_subtype(
        stream_id,
        rlp_seq,
        ip_packet,
        physical_packet_bits,
        FORWARD_TRAFFIC_MAC_SUBTYPE_DEFAULT,
    )
}

pub fn default_packet_ftc_payload_bits_for_mac_subtype(
    stream_id: u8,
    rlp_seq: u32,
    ip_packet: &[u8],
    physical_packet_bits: usize,
    forward_traffic_mac_subtype: u16,
) -> Result<Vec<u8>, TrafficFrameError> {
    default_packet_ftc_payload_bits_many_for_mac_subtype(
        stream_id,
        &[(rlp_seq, ip_packet)],
        physical_packet_bits,
        forward_traffic_mac_subtype,
    )
}

pub fn default_packet_ftc_payload_bits_many_for_mac_subtype(
    stream_id: u8,
    rlp_packets: &[(u32, &[u8])],
    physical_packet_bits: usize,
    forward_traffic_mac_subtype: u16,
) -> Result<Vec<u8>, TrafficFrameError> {
    let mut stream_packets = Vec::with_capacity(rlp_packets.len());
    for (rlp_seq, ip_packet) in rlp_packets {
        let rlp = default_packet_rlp_packet_bits(*rlp_seq, ip_packet);
        stream_packets.push(stream_layer_packet_bytes(stream_id, &rlp)?);
    }
    format_b_ftc_payload_bits_for_mac_subtype(
        &stream_packets,
        physical_packet_bits,
        forward_traffic_mac_subtype,
    )
}

pub fn format_b_ftc_payload_bits_for_mac_subtype(
    session_packets: &[Vec<u8>],
    physical_packet_bits: usize,
    forward_traffic_mac_subtype: u16,
) -> Result<Vec<u8>, TrafficFrameError> {
    let security_capacity = forward_traffic_security_capacity_bits_for_mac_subtype(
        physical_packet_bits,
        forward_traffic_mac_subtype,
    )
    .ok_or(TrafficFrameError::PhysicalPacketTooSmall(
        physical_packet_bits,
    ))?;
    match forward_traffic_mac_subtype {
        FORWARD_TRAFFIC_MAC_SUBTYPE_DEFAULT => {
            let mac_packets = physical_packet_bits / 1024;
            if session_packets.len() > mac_packets {
                return Err(TrafficFrameError::TooManyBits {
                    layer: "default forward traffic MAC packets",
                    bits: session_packets.len(),
                    capacity: mac_packets,
                });
            }
            let mut connections = Vec::with_capacity(session_packets.len());
            for packet in session_packets {
                connections.push(connection_format_b_bits(
                    std::slice::from_ref(packet),
                    security_capacity,
                )?);
            }
            let refs = connections
                .iter()
                .map(|connection| (connection.as_slice(), true))
                .collect::<Vec<_>>();
            default_ftc_physical_payload_bits_from_mac_packets(&refs, physical_packet_bits)
        }
        FORWARD_TRAFFIC_MAC_SUBTYPE_ENHANCED => {
            let connection = connection_format_b_bits(session_packets, security_capacity)?;
            enhanced_ftc_physical_payload_bits(&connection, true, physical_packet_bits)
        }
        _ => Err(TrafficFrameError::PhysicalPacketTooSmall(
            physical_packet_bits,
        )),
    }
}

pub fn default_packet_stream1_ftc_payload_bits(
    rlp_seq: u32,
    ip_packet: &[u8],
    physical_packet_bits: usize,
) -> Result<Vec<u8>, TrafficFrameError> {
    default_packet_ftc_payload_bits(
        DEFAULT_PACKET_STREAM_ID,
        rlp_seq,
        ip_packet,
        physical_packet_bits,
    )
}

/// Build a Forward Traffic Channel RTCAck physical-layer packet.
///
/// C.S0024-300 §1.10.6.3.2 carries RTCAck as an 8-bit message of the Default
/// Reverse Traffic Channel MAC Protocol. The message is sent on unicast FTC,
/// so it is wrapped as Stream 0 Default Signaling, then as a Format B
/// connection packet, then as a Forward Traffic MAC packet for the negotiated
/// MAC subtype.
pub fn default_reverse_traffic_mac_rtc_ack_ftc_payload_bits(
    physical_packet_bits: usize,
    sequence_number: u8,
) -> Result<Vec<u8>, TrafficFrameError> {
    default_reverse_traffic_mac_rtc_ack_ftc_payload_bits_for_mac_subtype(
        physical_packet_bits,
        sequence_number,
        FORWARD_TRAFFIC_MAC_SUBTYPE_DEFAULT,
    )
}

pub fn default_reverse_traffic_mac_rtc_ack_ftc_payload_bits_for_mac_subtype(
    physical_packet_bits: usize,
    sequence_number: u8,
    forward_traffic_mac_subtype: u16,
) -> Result<Vec<u8>, TrafficFrameError> {
    let signaling = encode_reliable_default_signaling_packet(
        DEFAULT_REVERSE_TRAFFIC_CHANNEL_MAC_PROTOCOL_TYPE,
        &[REVERSE_TRAFFIC_CHANNEL_MAC_RTC_ACK_MESSAGE_ID],
        sequence_number,
    );
    stream0_signaling_ftc_physical_payload_bits_for_mac_subtype(
        physical_packet_bits,
        &[signaling],
        forward_traffic_mac_subtype,
    )
}

/// Per-MAC-flow reverse-link T2P grant carried in a subtype-3 RTCMAC Grant
/// message (C.S0024-300 §1.13.6.2.4). `t2p_inflow` and `bucket_level` are in
/// units of 0.25 dB (0..=254), or 0xff for -infinity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MacFlowGrant {
    pub mac_flow_id: u8,
    pub t2p_inflow: u8,
    pub bucket_level: u8,
    pub tt2p_hold: u8,
}

/// Encode a subtype-3 RTCMAC Grant message body (C.S0024-300 §1.13.6.2.4). The
/// message is octet-padded with a zero Reserved field. `grants` must be
/// non-empty; NumMACFlows is transmitted as one less than its length.
pub fn encode_reverse_traffic_mac_grant_message(grants: &[MacFlowGrant]) -> Vec<u8> {
    debug_assert!((1..=16).contains(&grants.len()));
    let mut bits = Bitstream::new();
    bits.write_u8(REVERSE_TRAFFIC_CHANNEL_MAC_GRANT_MESSAGE_ID, 8);
    let num_mac_flows = grants.len().saturating_sub(1) as u8;
    bits.write_u8(num_mac_flows, REVERSE_TRAFFIC_MAC_GRANT_NUM_MAC_FLOWS_BITS);
    for grant in grants {
        bits.write_u8(
            grant.mac_flow_id,
            REVERSE_TRAFFIC_MAC_GRANT_MAC_FLOW_ID_BITS,
        );
        bits.write_u8(grant.t2p_inflow, REVERSE_TRAFFIC_MAC_GRANT_T2P_INFLOW_BITS);
        bits.write_u8(
            grant.bucket_level,
            REVERSE_TRAFFIC_MAC_GRANT_BUCKET_LEVEL_BITS,
        );
        debug_assert!(grant.tt2p_hold <= 0x3f);
        bits.write_u8(
            grant.tt2p_hold.min(0x3f),
            REVERSE_TRAFFIC_MAC_GRANT_TT2P_HOLD_BITS,
        );
    }
    bits.to_packed_bytes()
}

/// Build a subtype-3 RTCMAC Grant on the Forward Traffic Channel.
///
/// The Grant (C.S0024-300 §1.13.6.2.4) explicitly tops up per-MAC-flow reverse
/// T2P state, but a default-configured Rev A terminal can still ramp from the
/// subtype-3 RTC MAC defaults without it. The message is best-effort (not
/// SLP-D reliable) and addressed to the in-use Reverse Traffic Channel MAC
/// Protocol instance.
pub fn default_reverse_traffic_mac_grant_ftc_payload_bits_for_mac_subtype(
    physical_packet_bits: usize,
    grants: &[MacFlowGrant],
    forward_traffic_mac_subtype: u16,
) -> Result<Vec<u8>, TrafficFrameError> {
    let grant = encode_reverse_traffic_mac_grant_message(grants);
    let signaling = encode_default_signaling_packet_for_instance(
        DEFAULT_REVERSE_TRAFFIC_CHANNEL_MAC_PROTOCOL_TYPE,
        &grant,
        false,
    );
    stream0_signaling_ftc_physical_payload_bits_for_mac_subtype(
        physical_packet_bits,
        &[signaling],
        forward_traffic_mac_subtype,
    )
}

pub fn default_signaling_slp_d_ack_ftc_payload_bits_for_mac_subtype(
    physical_packet_bits: usize,
    ack_sequence_number: u8,
    forward_traffic_mac_subtype: u16,
) -> Result<Vec<u8>, TrafficFrameError> {
    let signaling = encode_default_signaling_slp_d_ack_packet(ack_sequence_number);
    stream0_signaling_ftc_physical_payload_bits_for_mac_subtype(
        physical_packet_bits,
        &[signaling],
        forward_traffic_mac_subtype,
    )
}

/// Build a Stream 0 SLP Reset message on the Forward Traffic Channel.
///
/// Reset is an SLP message, not an SNP/default-signaling protocol message, so
/// it is carried directly after the SLP-D best-effort header and has no SNP
/// protocol instance/type byte.
pub fn default_signaling_slp_reset_ftc_payload_bits(
    physical_packet_bits: usize,
    message_sequence: u8,
) -> Result<Vec<u8>, TrafficFrameError> {
    default_signaling_slp_reset_ftc_payload_bits_for_mac_subtype(
        physical_packet_bits,
        message_sequence,
        FORWARD_TRAFFIC_MAC_SUBTYPE_DEFAULT,
    )
}

pub fn default_signaling_slp_reset_ftc_payload_bits_for_mac_subtype(
    physical_packet_bits: usize,
    message_sequence: u8,
    forward_traffic_mac_subtype: u16,
) -> Result<Vec<u8>, TrafficFrameError> {
    let signaling = encode_default_signaling_slp_reset_packet(message_sequence);
    stream0_signaling_ftc_physical_payload_bits_for_mac_subtype(
        physical_packet_bits,
        &[signaling],
        forward_traffic_mac_subtype,
    )
}

pub fn default_signaling_ftc_payload_bits_with_ack(
    physical_packet_bits: usize,
    protocol_type: u8,
    payload: &[u8],
    reliable_sequence_number: Option<u8>,
    in_configuration: bool,
    ack_sequence_number: Option<u8>,
) -> Result<Vec<u8>, TrafficFrameError> {
    default_signaling_ftc_payload_bits_with_ack_for_mac_subtype(
        physical_packet_bits,
        protocol_type,
        payload,
        reliable_sequence_number,
        in_configuration,
        ack_sequence_number,
        FORWARD_TRAFFIC_MAC_SUBTYPE_DEFAULT,
    )
}

pub fn default_signaling_ftc_payload_bits_with_ack_for_mac_subtype(
    physical_packet_bits: usize,
    protocol_type: u8,
    payload: &[u8],
    reliable_sequence_number: Option<u8>,
    in_configuration: bool,
    ack_sequence_number: Option<u8>,
    forward_traffic_mac_subtype: u16,
) -> Result<Vec<u8>, TrafficFrameError> {
    let signaling = if let Some(sequence) = reliable_sequence_number {
        encode_reliable_default_signaling_packet_for_instance_with_ack(
            protocol_type,
            payload,
            sequence,
            in_configuration,
            ack_sequence_number,
        )
    } else {
        encode_default_signaling_packet_for_instance(protocol_type, payload, in_configuration)
    };
    stream0_signaling_ftc_physical_payload_bits_for_mac_subtype(
        physical_packet_bits,
        &[signaling],
        forward_traffic_mac_subtype,
    )
}

fn stream0_signaling_ftc_physical_payload_bits_for_mac_subtype(
    physical_packet_bits: usize,
    session_packets: &[Vec<u8>],
    forward_traffic_mac_subtype: u16,
) -> Result<Vec<u8>, TrafficFrameError> {
    let security_capacity = forward_traffic_security_capacity_bits_for_mac_subtype(
        physical_packet_bits,
        forward_traffic_mac_subtype,
    )
    .ok_or(TrafficFrameError::PhysicalPacketTooSmall(
        physical_packet_bits,
    ))?;
    let connection = connection_format_b_bits(session_packets, security_capacity)?;
    ftc_physical_payload_bits_for_mac_subtype(
        &connection,
        true,
        physical_packet_bits,
        forward_traffic_mac_subtype,
    )
}

pub fn enhanced_signaling_ftc_payload_bits(
    physical_packet_bits: usize,
    protocol_type: u8,
    payload: &[u8],
    reliable_sequence_number: Option<u8>,
    in_configuration: bool,
) -> Result<Vec<u8>, TrafficFrameError> {
    enhanced_signaling_ftc_payload_bits_with_ack(
        physical_packet_bits,
        protocol_type,
        payload,
        reliable_sequence_number,
        in_configuration,
        None,
    )
}

pub fn enhanced_signaling_ftc_payload_bits_with_ack(
    physical_packet_bits: usize,
    protocol_type: u8,
    payload: &[u8],
    reliable_sequence_number: Option<u8>,
    in_configuration: bool,
    ack_sequence_number: Option<u8>,
) -> Result<Vec<u8>, TrafficFrameError> {
    let signaling = if let Some(sequence) = reliable_sequence_number {
        encode_reliable_default_signaling_packet_for_instance_with_ack(
            protocol_type,
            payload,
            sequence,
            in_configuration,
            ack_sequence_number,
        )
    } else {
        encode_default_signaling_packet_for_instance(protocol_type, payload, in_configuration)
    };
    let security_capacity = forward_traffic_security_capacity_bits(physical_packet_bits).ok_or(
        TrafficFrameError::PhysicalPacketTooSmall(physical_packet_bits),
    )?;
    let connection = connection_format_b_bits(&[signaling], security_capacity)?;
    enhanced_ftc_physical_payload_bits(&connection, true, physical_packet_bits)
}

/// Strip the Reverse Traffic Channel MAC trailer and return the Security Layer
/// packet bits. Default/subtype-1 order the trailer as ConnectionLayerFormat
/// then MACLayerFormat; subtype 3 orders it as ConnectionLayerFormat then
/// TransmissionMode.
pub fn parse_reverse_traffic_mac_packet(
    mac_packet_bits: &[u8],
) -> Result<ReverseTrafficMacPacket, TrafficFrameError> {
    parse_reverse_traffic_mac_packet_for_subtype(
        mac_packet_bits,
        REVERSE_TRAFFIC_MAC_SUBTYPE_DEFAULT,
    )
}

pub fn parse_reverse_traffic_mac_packet_for_subtype(
    mac_packet_bits: &[u8],
    reverse_traffic_mac_subtype: u16,
) -> Result<ReverseTrafficMacPacket, TrafficFrameError> {
    if mac_packet_bits.len() < TRAFFIC_MAC_TRAILER_BITS {
        return Err(TrafficFrameError::MacPacketTooSmall(mac_packet_bits.len()));
    }
    let trailer = mac_packet_bits.len() - TRAFFIC_MAC_TRAILER_BITS;
    let (connection_layer_format_b, transmission_mode_low_latency) =
        match reverse_traffic_mac_subtype {
            REVERSE_TRAFFIC_MAC_SUBTYPE3 => (
                mac_packet_bits[trailer] & 1 != 0,
                Some(mac_packet_bits[trailer + 1] & 1 != 0),
            ),
            _ => {
                let mac_layer_format_valid = mac_packet_bits[trailer + 1] & 1 != 0;
                if !mac_layer_format_valid {
                    return Err(TrafficFrameError::InvalidMacLayerFormat);
                }
                (mac_packet_bits[trailer] & 1 != 0, None)
            }
        };
    Ok(ReverseTrafficMacPacket {
        connection_layer_format_b,
        security_payload_bits: mac_packet_bits[..trailer]
            .iter()
            .map(|bit| bit & 1)
            .collect(),
        transmission_mode_low_latency,
    })
}

/// Parse Connection Layer Format B into octet-aligned Session Layer packets.
///
/// Format B carries each packet as an 8-bit octet length followed by that many
/// octets. The remaining capacity in the Security Layer packet is zero padding.
pub fn parse_connection_format_b_packets(bits: &[u8]) -> Result<Vec<Vec<u8>>, TrafficFrameError> {
    let mut packets = Vec::new();
    let mut offset = 0usize;
    while offset < bits.len() {
        let remaining = bits.len() - offset;
        if remaining < CONNECTION_FORMAT_B_LENGTH_BITS {
            if bits[offset..].iter().any(|bit| bit & 1 != 0) {
                return Err(TrafficFrameError::ConnectionFormatBLengthTruncated);
            }
            break;
        }
        let len = unpack_u8(&bits[offset..offset + CONNECTION_FORMAT_B_LENGTH_BITS]) as usize;
        if len == 0 {
            if bits[offset..].iter().any(|bit| bit & 1 != 0) {
                return Err(TrafficFrameError::ConnectionFormatBPaddingNonZero);
            }
            break;
        }
        offset += CONNECTION_FORMAT_B_LENGTH_BITS;
        let packet_bits = len * 8;
        let end = offset + packet_bits;
        if end > bits.len() {
            return Err(TrafficFrameError::ConnectionFormatBPacketTruncated {
                needed: end,
                actual: bits.len(),
            });
        }
        packets.push(pack_bits_to_bytes(&bits[offset..end]));
        offset = end;
    }
    Ok(packets)
}

/// Parse Connection Layer packets according to the MAC trailer's
/// ConnectionLayerFormat bit.
///
/// Format A carries exactly one Session Layer packet with no Connection Layer
/// header or padding. Format B carries one or more length-prefixed Session
/// Layer packets plus zero padding.
pub fn parse_connection_layer_packets(
    connection_layer_format_b: bool,
    bits: &[u8],
) -> Result<Vec<Vec<u8>>, TrafficFrameError> {
    if connection_layer_format_b {
        parse_connection_format_b_packets(bits)
    } else {
        Ok(vec![pack_bits_to_bytes(bits)])
    }
}

/// Parse a Default Stream Protocol packet.
pub fn parse_stream_layer_packet_bytes(
    packet: &[u8],
) -> Result<HrpdStreamLayerPacket, TrafficFrameError> {
    let bits = Bitstream::new_bytes(packet).bits().to_vec();
    if bits.len() < DEFAULT_STREAM_HEADER_BITS {
        return Err(TrafficFrameError::StreamPacketTooSmall(bits.len()));
    }
    let stream_id = unpack_u8(&bits[..DEFAULT_STREAM_HEADER_BITS]);
    Ok(HrpdStreamLayerPacket {
        stream_id,
        application_packet_bits: bits[DEFAULT_STREAM_HEADER_BITS..].to_vec(),
    })
}

/// Parse one Default Packet Application RLP packet into A8/A10 byte-stream
/// octets. The 22-bit SEQ is retained for observability and ordering checks.
pub fn parse_default_packet_rlp_packet_bits(
    bits: &[u8],
) -> Result<HrpdDefaultPacketRlpPacket, TrafficFrameError> {
    if bits.len() < DEFAULT_PACKET_RLP_SEQUENCE_BITS {
        return Err(TrafficFrameError::DefaultPacketRlpTooSmall(bits.len()));
    }
    if (bits.len() - DEFAULT_PACKET_RLP_SEQUENCE_BITS) % 8 != 0 {
        return Err(TrafficFrameError::DefaultPacketRlpMisaligned(bits.len()));
    }
    let sequence = unpack_u32(&bits[..DEFAULT_PACKET_RLP_SEQUENCE_BITS]);
    let payload = pack_bits_to_bytes(&bits[DEFAULT_PACKET_RLP_SEQUENCE_BITS..]);
    Ok(HrpdDefaultPacketRlpPacket { sequence, payload })
}

/// Decode a validated Reverse Traffic Channel MAC Layer packet into Default
/// Packet Application byte-stream packets.
pub fn parse_reverse_stream1_packets(
    mac_packet_bits: &[u8],
) -> Result<Vec<HrpdDefaultPacketRlpPacket>, TrafficFrameError> {
    parse_reverse_stream1_packets_for_subtype(mac_packet_bits, REVERSE_TRAFFIC_MAC_SUBTYPE_DEFAULT)
}

pub fn parse_reverse_stream1_packets_for_subtype(
    mac_packet_bits: &[u8],
    reverse_traffic_mac_subtype: u16,
) -> Result<Vec<HrpdDefaultPacketRlpPacket>, TrafficFrameError> {
    let mac =
        parse_reverse_traffic_mac_packet_for_subtype(mac_packet_bits, reverse_traffic_mac_subtype)?;
    let mut out = Vec::new();
    for session_packet in
        parse_connection_layer_packets(mac.connection_layer_format_b, &mac.security_payload_bits)?
    {
        let stream = parse_stream_layer_packet_bytes(&session_packet)?;
        if default_packet_stream_protocol_type(stream.stream_id).is_some() {
            out.push(parse_default_packet_rlp_packet_bits(
                &stream.application_packet_bits,
            )?);
        }
    }
    Ok(out)
}

pub fn physical_crc16(bits: &[u8]) -> u16 {
    let mut reg = 0u16;
    for &bit in bits {
        reg = clock_crc16(reg, bit & 1);
    }
    reg
}

pub fn physical_crc24(bits: &[u8]) -> u32 {
    let mut reg = 0u32;
    for &bit in bits {
        reg = clock_crc24(reg, bit & 1);
    }
    reg
}

fn clock_crc24(reg: u32, bit: u8) -> u32 {
    let feedback = ((reg >> 23) & 1) ^ u32::from(bit & 1);
    let mut next = (reg << 1) & 0x00ff_ffff;
    if feedback != 0 {
        next ^= 0x0080_0063;
    }
    next
}

fn clock_crc16(reg: u16, bit: u8) -> u16 {
    let feedback = ((reg >> 15) & 1) ^ u16::from(bit & 1);
    let mut next = reg << 1;
    if feedback != 0 {
        next ^= 0x1021;
    }
    next
}

fn push_u24_msb(bits: &mut Vec<u8>, value: u32) {
    for shift in (0..24).rev() {
        bits.push(((value >> shift) & 1) as u8);
    }
}

fn push_u16_msb(bits: &mut Vec<u8>, value: u16) {
    for shift in (0..16).rev() {
        bits.push(((value >> shift) & 1) as u8);
    }
}

fn unpack_u8(bits: &[u8]) -> u8 {
    bits.iter().fold(0u8, |acc, &bit| (acc << 1) | (bit & 1))
}

fn unpack_u32(bits: &[u8]) -> u32 {
    bits.iter()
        .fold(0u32, |acc, &bit| (acc << 1) | u32::from(bit & 1))
}

fn pack_bits_to_bytes(bits: &[u8]) -> Vec<u8> {
    Bitstream::new_init(bits).to_packed_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn unpack_u16(bits: &[u8]) -> u16 {
        bits.iter()
            .fold(0u16, |acc, &bit| (acc << 1) | u16::from(bit & 1))
    }

    fn unpack_u32(bits: &[u8]) -> u32 {
        bits.iter()
            .fold(0u32, |acc, &bit| (acc << 1) | u32::from(bit & 1))
    }

    #[test]
    fn rlp_packet_has_required_stream_alignment() {
        let rlp = default_packet_rlp_packet_bits(0x2a_55aa, &[0xde, 0xad, 0xbe, 0xef]);
        assert_eq!(rlp.len(), DEFAULT_PACKET_RLP_SEQUENCE_BITS + 32);
        assert_eq!(rlp.len() % 8, 6);
        assert_eq!(unpack_u32(&rlp[..22]), 0x2a_55aa);
    }

    #[test]
    fn stream_header_packs_into_first_octet() {
        let rlp = default_packet_rlp_packet_bits(0x155555, &[0x80]);
        let stream = stream_layer_packet_bytes(DEFAULT_PACKET_STREAM2_ID, &rlp).unwrap();
        assert_eq!(stream[0] >> 6, DEFAULT_PACKET_STREAM2_ID);
        assert_eq!((stream[0] >> 5) & 1, rlp[0]);
        assert_eq!(stream.len(), 4);
    }

    #[test]
    fn connection_format_b_adds_length_and_padding() {
        let pkt = vec![0x40, 0x12, 0x34];
        let bits = connection_format_b_bits(&[pkt], 40).unwrap();
        assert_eq!(bits.len(), 40);
        assert_eq!(
            Bitstream::new_init(&bits).to_packed_bytes(),
            vec![3, 0x40, 0x12, 0x34, 0]
        );
    }

    #[test]
    fn ftc_payload_has_mac_trailer_fcs_and_tail() {
        let connection = connection_format_b_bits(&[vec![0x40, 0x12, 0x34]], 1000).unwrap();
        let physical = default_ftc_physical_payload_bits(&connection, true, 1024).unwrap();
        assert_eq!(physical.len(), 1024);
        assert_eq!(physical[1000], 1, "ConnectionLayerFormat=Format B");
        assert_eq!(physical[1001], 1, "MACLayerFormat=valid payload");
        assert_eq!(
            unpack_u16(&physical[1002..1018]),
            physical_crc16(&physical[..1002])
        );
        assert!(physical[1018..].iter().all(|b| *b == 0));
    }

    #[test]
    fn default_packet_fits_drc1_packet() {
        let physical = default_packet_stream1_ftc_payload_bits(7, &[0x45, 0, 0, 20], 1024).unwrap();
        assert_eq!(physical.len(), 1024);
        let bytes = Bitstream::new_init(&physical[..1000]).to_packed_bytes();
        assert_eq!(bytes[0], 7, "Format B length octet");
        assert_eq!(bytes[1] >> 6, DEFAULT_PACKET_STREAM_ID);
        assert_eq!(physical[1000], 1, "ConnectionLayerFormat=Format B");
        assert_eq!(physical[1001], 1, "MACLayerFormat=valid payload");
        assert_eq!(
            unpack_u16(&physical[1002..1018]),
            physical_crc16(&physical[..1002])
        );
        assert!(physical[1018..].iter().all(|b| *b == 0));
    }

    /// A 120-octet RLP segment frames to exactly 992 bits (22-bit sequence +
    /// 2-bit stream header + 8-bit Format B length), so it exactly fills an
    /// enhanced 1024-bit packet and five of them fit a 5120-bit packet
    /// (5 x 992 = 4960 <= 5088). One octet more overflows the 1024-bit size.
    #[test]
    fn enhanced_packet_segment_capacities() {
        let seg = vec![0xa5u8; 120];
        let single: [(u32, &[u8]); 1] = [(0, seg.as_slice())];
        let physical = default_packet_ftc_payload_bits_many_for_mac_subtype(
            DEFAULT_PACKET_STREAM2_ID,
            &single,
            1024,
            FORWARD_TRAFFIC_MAC_SUBTYPE_ENHANCED,
        )
        .unwrap();
        assert_eq!(physical.len(), 1024);

        let five: Vec<(u32, &[u8])> = (0..5u32).map(|i| (i * 120, seg.as_slice())).collect();
        let physical = default_packet_ftc_payload_bits_many_for_mac_subtype(
            DEFAULT_PACKET_STREAM2_ID,
            &five,
            5120,
            FORWARD_TRAFFIC_MAC_SUBTYPE_ENHANCED,
        )
        .unwrap();
        assert_eq!(physical.len(), 5120);

        let oversize = vec![0xa5u8; 121];
        let single_oversize: [(u32, &[u8]); 1] = [(0, oversize.as_slice())];
        assert!(matches!(
            default_packet_ftc_payload_bits_many_for_mac_subtype(
                DEFAULT_PACKET_STREAM2_ID,
                &single_oversize,
                1024,
                FORWARD_TRAFFIC_MAC_SUBTYPE_ENHANCED,
            ),
            Err(TrafficFrameError::TooManyBits { .. })
        ));
    }

    #[test]
    fn default_rev0_4096_packet_carries_four_mac_packets() {
        let p0 = [0x10];
        let p1 = [0x11];
        let p2 = [0x12];
        let p3 = [0x13];
        let packets: [(u32, &[u8]); 4] = [
            (0, p0.as_slice()),
            (1, p1.as_slice()),
            (2, p2.as_slice()),
            (3, p3.as_slice()),
        ];
        let physical = default_packet_ftc_payload_bits_many_for_mac_subtype(
            DEFAULT_PACKET_STREAM2_ID,
            &packets,
            4096,
            FORWARD_TRAFFIC_MAC_SUBTYPE_DEFAULT,
        )
        .unwrap();

        assert_eq!(physical.len(), 4096);
        for (idx, expected_payload) in [p0, p1, p2, p3].iter().enumerate() {
            let mac_start = idx * 1024;
            if idx > 0 {
                assert!(
                    physical[mac_start - FORWARD_TRAFFIC_MAC_PAD_BITS..mac_start]
                        .iter()
                        .all(|bit| *bit == 0),
                    "inter-MAC pad before MAC {idx} is zero"
                );
            }
            assert_eq!(
                physical[mac_start + 1000],
                1,
                "MAC {idx} ConnectionLayerFormat=Format B"
            );
            assert_eq!(
                physical[mac_start + 1001],
                1,
                "MAC {idx} MACLayerFormat=valid payload"
            );
            let session_packets =
                parse_connection_format_b_packets(&physical[mac_start..mac_start + 1000]).unwrap();
            assert_eq!(session_packets.len(), 1);
            let stream = parse_stream_layer_packet_bytes(&session_packets[0]).unwrap();
            assert_eq!(stream.stream_id, DEFAULT_PACKET_STREAM2_ID);
            let rlp =
                parse_default_packet_rlp_packet_bits(&stream.application_packet_bits).unwrap();
            assert_eq!(rlp.sequence, idx as u32);
            assert_eq!(rlp.payload.as_slice(), expected_payload);
        }
        assert_eq!(
            unpack_u16(&physical[4074..4090]),
            physical_crc16(&physical[..4074])
        );
        assert!(physical[4090..].iter().all(|b| *b == 0));
    }

    #[test]
    fn enhanced_forward_traffic_payload_bits_follow_subtype1_drc_table() {
        for drc in [0x1, 0x2, 0x3, 0x4, 0x6] {
            assert_eq!(forward_traffic_payload_bits_for_drc(drc), Some(1024));
        }
        for drc in [0x5, 0x7, 0x9] {
            assert_eq!(forward_traffic_payload_bits_for_drc(drc), Some(2048));
        }
        assert_eq!(forward_traffic_payload_bits_for_drc(0x8), Some(3072));
        assert_eq!(forward_traffic_payload_bits_for_drc(0xb), Some(3072));
        assert_eq!(forward_traffic_payload_bits_for_drc(0xa), Some(4096));
        assert_eq!(forward_traffic_payload_bits_for_drc(0xc), Some(4096));
        assert_eq!(forward_traffic_payload_bits_for_drc(0xd), Some(5120));
        assert_eq!(forward_traffic_payload_bits_for_drc(0xe), Some(5120));
        assert_eq!(forward_traffic_payload_bits_for_drc(0x0), None);
        assert_eq!(forward_traffic_payload_bits_for_drc(0xf), None);
    }

    #[test]
    fn implemented_forward_traffic_payload_bits_include_subtype1_5120_rates() {
        assert_eq!(
            implemented_forward_traffic_payload_bits_for_drc(0xc),
            Some(4096)
        );
        assert_eq!(
            implemented_forward_traffic_payload_bits_for_drc(0xd),
            Some(5120)
        );
        assert_eq!(
            implemented_forward_traffic_payload_bits_for_drc(0xe),
            Some(5120)
        );
        assert_eq!(implemented_forward_traffic_payload_bits_for_drc(0xf), None);
    }

    #[test]
    fn forward_traffic_payload_bits_are_scoped_by_mac_subtype() {
        assert_eq!(
            forward_traffic_payload_bits_for_drc_for_mac_subtype(
                0xc,
                FORWARD_TRAFFIC_MAC_SUBTYPE_DEFAULT
            ),
            Some(4096)
        );
        assert_eq!(
            forward_traffic_payload_bits_for_drc_for_mac_subtype(
                0xd,
                FORWARD_TRAFFIC_MAC_SUBTYPE_DEFAULT
            ),
            None
        );
        assert_eq!(
            forward_traffic_payload_bits_for_drc_for_mac_subtype(
                0xe,
                FORWARD_TRAFFIC_MAC_SUBTYPE_DEFAULT
            ),
            None
        );
        assert_eq!(
            forward_traffic_payload_bits_for_drc_for_mac_subtype(
                0xe,
                FORWARD_TRAFFIC_MAC_SUBTYPE_ENHANCED
            ),
            Some(5120)
        );
    }

    #[test]
    fn enhanced_5120_packet_uses_single_mac_packet_and_crc24() {
        let physical = enhanced_signaling_ftc_payload_bits(
            5120,
            DEFAULT_REVERSE_TRAFFIC_CHANNEL_MAC_PROTOCOL_TYPE,
            &[REVERSE_TRAFFIC_CHANNEL_MAC_RTC_ACK_MESSAGE_ID],
            None,
            false,
        )
        .unwrap();

        assert_eq!(physical.len(), 5120);
        assert_eq!(
            physical[5088], 1,
            "ConnectionLayerFormat=Format B at subtype-1 trailer"
        );
        assert_eq!(physical[5089], 1, "MACLayerFormat=valid payload");
        assert_eq!(
            unpack_u32(&physical[5090..5114]),
            physical_crc24(&physical[..5090])
        );
        assert!(physical[5114..].iter().all(|b| *b == 0));
    }

    #[test]
    fn stream0_signaling_5120_packet_uses_enhanced_format() {
        let physical = default_signaling_ftc_payload_bits_with_ack_for_mac_subtype(
            5120,
            DEFAULT_REVERSE_TRAFFIC_CHANNEL_MAC_PROTOCOL_TYPE,
            &[REVERSE_TRAFFIC_CHANNEL_MAC_RTC_ACK_MESSAGE_ID],
            Some(3),
            true,
            Some(5),
            FORWARD_TRAFFIC_MAC_SUBTYPE_ENHANCED,
        )
        .unwrap();

        assert_eq!(physical.len(), 5120);
        assert_eq!(
            physical[5088], 1,
            "ConnectionLayerFormat=Format B at subtype-1 trailer"
        );
        assert_eq!(physical[5089], 1, "MACLayerFormat=valid payload");
        assert_eq!(
            unpack_u32(&physical[5090..5114]),
            physical_crc24(&physical[..5090])
        );
        assert!(physical[5114..].iter().all(|b| *b == 0));
    }

    #[test]
    fn enhanced_default_packet_connection_layer_round_trips_to_byte_stream_octets() {
        let payload = [0x7e, 0xff, 0x03, 0xc0, 0x21, 0x01, 0x2a, 0x00, 0x04, 0x7e];
        let physical = default_packet_ftc_payload_bits_for_mac_subtype(
            DEFAULT_PACKET_STREAM2_ID,
            0x2a_55aa,
            &payload,
            5120,
            FORWARD_TRAFFIC_MAC_SUBTYPE_ENHANCED,
        )
        .unwrap();

        assert_eq!(physical.len(), 5120);
        assert_eq!(
            physical[5088], 1,
            "ConnectionLayerFormat=Format B at subtype-1 trailer"
        );
        assert_eq!(physical[5089], 1, "MACLayerFormat=valid payload");
        assert_eq!(
            unpack_u32(&physical[5090..5114]),
            physical_crc24(&physical[..5090])
        );
        assert!(physical[5114..].iter().all(|b| *b == 0));

        let mac_packet =
            &physical[..physical.len() - ENHANCED_PHYSICAL_FCS_BITS - PHYSICAL_TAIL_BITS];
        let parsed = parse_reverse_traffic_mac_packet(mac_packet).unwrap();
        assert!(parsed.connection_layer_format_b);
        let packets = parse_connection_format_b_packets(&parsed.security_payload_bits).unwrap();
        assert_eq!(packets.len(), 1);
        let stream = parse_stream_layer_packet_bytes(&packets[0]).unwrap();
        assert_eq!(stream.stream_id, DEFAULT_PACKET_STREAM2_ID);
        let rlp = parse_default_packet_rlp_packet_bits(&stream.application_packet_bits).unwrap();
        assert_eq!(rlp.sequence, 0x2a_55aa);
        assert_eq!(rlp.payload, payload);
    }

    #[test]
    fn enhanced_4096_packet_uses_single_mac_packet_and_crc24() {
        let physical = enhanced_signaling_ftc_payload_bits(
            4096,
            DEFAULT_REVERSE_TRAFFIC_CHANNEL_MAC_PROTOCOL_TYPE,
            &[REVERSE_TRAFFIC_CHANNEL_MAC_RTC_ACK_MESSAGE_ID],
            None,
            false,
        )
        .unwrap();

        assert_eq!(physical.len(), 4096);
        assert_eq!(
            physical[4064], 1,
            "ConnectionLayerFormat=Format B at subtype-1 trailer"
        );
        assert_eq!(physical[4065], 1, "MACLayerFormat=valid payload");
        assert_eq!(
            unpack_u32(&physical[4066..4090]),
            physical_crc24(&physical[..4066])
        );
        assert!(physical[4090..].iter().all(|b| *b == 0));
    }

    #[test]
    fn enhanced_rtc_ack_connection_layer_round_trips_through_reverse_validated_parser() {
        // Decode our enhanced (subtype-1) forward RTCAck the way a Rev A handset
        // must, using the same connection/MAC parsers validated against real
        // handset reverse traffic. A pass means the forward RTCAck framing is
        // spec-correct end to end.
        let physical = default_reverse_traffic_mac_rtc_ack_ftc_payload_bits_for_mac_subtype(
            5120,
            0,
            FORWARD_TRAFFIC_MAC_SUBTYPE_ENHANCED,
        )
        .unwrap();
        // Strip the physical CRC-24 + tail to recover the MAC packet. The
        // enhanced Single-User trailer is [ConnectionLayerFormat, MACLayerFormat],
        // identical to the reverse default trailer, so the reverse parser applies.
        let mac_packet =
            &physical[..physical.len() - ENHANCED_PHYSICAL_FCS_BITS - PHYSICAL_TAIL_BITS];
        let parsed = parse_reverse_traffic_mac_packet(mac_packet).unwrap();
        assert!(parsed.connection_layer_format_b);
        let packets = parse_connection_format_b_packets(&parsed.security_payload_bits).unwrap();
        assert_eq!(packets.len(), 1, "one Stream 0 signaling packet");
        let signaling = &packets[0];
        assert_eq!(signaling[0] >> 6, 0, "Stream 0: Default Signaling");
        assert_eq!(signaling[0] & 0x01, 1, "SLP-D full header included");
        assert_eq!(signaling[1], 0b0000_1000, "reliable SLP-D seq=0");
        assert_eq!(
            signaling[2],
            DEFAULT_REVERSE_TRAFFIC_CHANNEL_MAC_PROTOCOL_TYPE
        );
        assert_eq!(signaling[3], REVERSE_TRAFFIC_CHANNEL_MAC_RTC_ACK_MESSAGE_ID);
    }

    #[test]
    fn rtc_ack_forward_traffic_packet_wraps_reverse_mac_default_signaling() {
        let physical = default_reverse_traffic_mac_rtc_ack_ftc_payload_bits(1024, 0).unwrap();

        assert_eq!(physical.len(), 1024);
        assert_eq!(physical[1000], 1, "ConnectionLayerFormat=Format B");
        assert_eq!(physical[1001], 1, "MACLayerFormat=valid payload");
        assert_eq!(
            unpack_u16(&physical[1002..1018]),
            physical_crc16(&physical[..1002])
        );
        let bytes = Bitstream::new_init(&physical[..1000]).to_packed_bytes();
        assert_eq!(bytes[0], 4, "4-octet reliable Stream 0 signaling packet");
        assert_eq!(bytes[1] >> 6, 0, "Stream 0: Default Signaling");
        assert_eq!(bytes[1] & 0x01, 1, "SLP-D full header included");
        assert_eq!(bytes[2], 0b0000_1000, "reliable SLP-D seq=0");
        assert_eq!(bytes[3], DEFAULT_REVERSE_TRAFFIC_CHANNEL_MAC_PROTOCOL_TYPE);
        assert_eq!(bytes[4], REVERSE_TRAFFIC_CHANNEL_MAC_RTC_ACK_MESSAGE_ID);
    }

    #[test]
    fn slp_reset_forward_traffic_packet_is_raw_slp_payload() {
        let physical = default_signaling_slp_reset_ftc_payload_bits(1024, 7).unwrap();

        assert_eq!(physical.len(), 1024);
        assert_eq!(physical[1000], 1, "ConnectionLayerFormat=Format B");
        assert_eq!(physical[1001], 1, "MACLayerFormat=valid payload");
        assert_eq!(
            unpack_u16(&physical[1002..1018]),
            physical_crc16(&physical[..1002])
        );
        let bytes = Bitstream::new_init(&physical[..1000]).to_packed_bytes();
        assert_eq!(bytes[0], 3, "3-octet Stream 0 SLP Reset packet");
        assert_eq!(bytes[1], 0, "Stream0/SLP-F/SLP-D best-effort headers");
        assert_eq!(bytes[2], 0, "SLP Reset message id");
        assert_eq!(bytes[3], 7, "SLP Reset MessageSequence");
    }

    #[test]
    fn rtc_ack_high_rate_packet_keeps_forward_mac_packet_boundaries() {
        let physical = default_reverse_traffic_mac_rtc_ack_ftc_payload_bits(2048, 0).unwrap();

        assert_eq!(physical.len(), 2048);
        assert_eq!(physical[1000], 1, "ConnectionLayerFormat=Format B");
        assert_eq!(physical[1001], 1, "MACLayerFormat=valid payload");
        assert!(
            physical[1002..1024].iter().all(|b| *b == 0),
            "first legacy MAC pad bits are zero before the aggregate physical FCS"
        );
        assert!(
            physical[1024..2026].iter().all(|b| *b == 0),
            "second legacy MAC packet is null/invalid"
        );
        assert_eq!(
            unpack_u16(&physical[2026..2042]),
            physical_crc16(&physical[..2026])
        );
        assert!(physical[2042..].iter().all(|b| *b == 0));
    }

    #[test]
    fn reverse_default_packet_stream_round_trips_to_byte_stream_octets() {
        let rlp = default_packet_rlp_packet_bits(0x12_3456, &[0x7e, 0xff, 0x03, 0xc0, 0x21, 0x7e]);
        let stream = stream_layer_packet_bytes(DEFAULT_PACKET_STREAM2_ID, &rlp).unwrap();
        let connection = connection_format_b_bits(&[stream], 232).unwrap();
        let mut mac = connection;
        mac.push(1);
        mac.push(1);

        let parsed = parse_reverse_stream1_packets(&mac).unwrap();

        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].sequence, 0x12_3456);
        assert_eq!(parsed[0].payload, [0x7e, 0xff, 0x03, 0xc0, 0x21, 0x7e]);
    }

    #[test]
    fn reverse_mac_rejects_invalid_payload_marker() {
        let err = parse_reverse_traffic_mac_packet(&[1, 0]).unwrap_err();
        assert_eq!(err, TrafficFrameError::InvalidMacLayerFormat);
    }

    #[test]
    fn reverse_mac_subtype3_uses_connection_layer_format_then_transmission_mode() {
        let mut mac = vec![0, 0, 0, 0, 0, 0, 0, 0];
        mac.push(0); // ConnectionLayerFormat = Format A.
        mac.push(1); // TransmissionMode = Low Latency.

        let parsed =
            parse_reverse_traffic_mac_packet_for_subtype(&mac, REVERSE_TRAFFIC_MAC_SUBTYPE3)
                .unwrap();

        assert!(!parsed.connection_layer_format_b);
        assert_eq!(parsed.transmission_mode_low_latency, Some(true));
        assert_eq!(parsed.security_payload_bits, vec![0; 8]);
    }

    #[test]
    fn reverse_mac_subtype3_accepts_low_latency_format_a_without_mac_layer_valid_bit() {
        let session_packet = [0x40, 0x00];
        let mut mac = Bitstream::new_bytes(&session_packet).bits().to_vec();
        mac.push(0); // ConnectionLayerFormat = Format A.
        mac.push(1); // TransmissionMode = Low Latency.

        let reverse_mac =
            parse_reverse_traffic_mac_packet_for_subtype(&mac, REVERSE_TRAFFIC_MAC_SUBTYPE3)
                .unwrap();
        let parsed = parse_connection_layer_packets(
            reverse_mac.connection_layer_format_b,
            &reverse_mac.security_payload_bits,
        )
        .unwrap();

        assert_eq!(parsed, vec![session_packet.to_vec()]);
    }

    #[test]
    fn connection_format_b_rejects_nonzero_padding() {
        let mut bits = Vec::new();
        bits.extend(Bitstream::new_bytes(&[0]).bits());
        bits.push(1);

        let err = parse_connection_format_b_packets(&bits).unwrap_err();

        assert_eq!(err, TrafficFrameError::ConnectionFormatBPaddingNonZero);
    }

    #[test]
    fn oversize_packet_is_rejected() {
        let err = default_packet_stream1_ftc_payload_bits(0, &[0u8; 140], 1024).unwrap_err();
        assert!(matches!(err, TrafficFrameError::TooManyBits { .. }));
    }

    #[test]
    fn single_flow_grant_message_matches_spec_layout() {
        // MessageID 0x03, NumMACFlows 0 (one flow), MACFlowID 0, T2PInflow 0x50,
        // BucketLevel 0x50, TT2PHold 0x0f as a 6-bit field, padded to octets.
        let grant = MacFlowGrant {
            mac_flow_id: 0,
            t2p_inflow: 0x50,
            bucket_level: 0x50,
            tt2p_hold: 0x0f,
        };
        assert_eq!(
            encode_reverse_traffic_mac_grant_message(&[grant]),
            vec![0x03, 0x00, 0x50, 0x50, 0x3c]
        );
    }

    #[test]
    fn multi_flow_grant_encodes_num_mac_flows_minus_one() {
        let grants = [
            MacFlowGrant {
                mac_flow_id: 0,
                t2p_inflow: 0x50,
                bucket_level: 0x50,
                tt2p_hold: 0x0f,
            },
            MacFlowGrant {
                mac_flow_id: 1,
                t2p_inflow: 0x30,
                bucket_level: 0x40,
                tt2p_hold: 0x07,
            },
        ];
        let bytes = encode_reverse_traffic_mac_grant_message(&grants);
        assert_eq!(bytes[0], REVERSE_TRAFFIC_CHANNEL_MAC_GRANT_MESSAGE_ID);
        // NumMACFlows is the high nibble of byte 1 and is len - 1 = 1.
        assert_eq!(bytes[1] >> 4, 1);
        // 8 (id) + 4 (count) + 2 * 26 = 64 bits = 8 octets.
        assert_eq!(bytes.len(), 8);
    }

    #[test]
    fn grant_ftc_payload_fills_physical_packet() {
        let grant = MacFlowGrant {
            mac_flow_id: 0,
            t2p_inflow: 0x50,
            bucket_level: 0x50,
            tt2p_hold: 0x0f,
        };
        let bits = default_reverse_traffic_mac_grant_ftc_payload_bits_for_mac_subtype(
            1024,
            &[grant],
            FORWARD_TRAFFIC_MAC_SUBTYPE_ENHANCED,
        )
        .unwrap();
        assert_eq!(bits.len(), 1024);
    }

    #[test]
    fn enhanced_grant_connection_layer_round_trips_through_reverse_validated_parser() {
        let grants = [
            MacFlowGrant {
                mac_flow_id: 0,
                t2p_inflow: 0x2b,
                bucket_level: 0x50,
                tt2p_hold: 0x0f,
            },
            MacFlowGrant {
                mac_flow_id: 1,
                t2p_inflow: 0x50,
                bucket_level: 0x6c,
                tt2p_hold: 0x0f,
            },
        ];
        let physical = default_reverse_traffic_mac_grant_ftc_payload_bits_for_mac_subtype(
            5120,
            &grants,
            FORWARD_TRAFFIC_MAC_SUBTYPE_ENHANCED,
        )
        .unwrap();

        let mac_packet =
            &physical[..physical.len() - ENHANCED_PHYSICAL_FCS_BITS - PHYSICAL_TAIL_BITS];
        let parsed = parse_reverse_traffic_mac_packet(mac_packet).unwrap();
        assert!(parsed.connection_layer_format_b);
        let packets = parse_connection_format_b_packets(&parsed.security_payload_bits).unwrap();
        assert_eq!(packets.len(), 1, "one Stream 0 signaling packet");

        let signaling = &packets[0];
        assert_eq!(signaling[0], 0, "best-effort Stream 0 signaling header");
        assert_eq!(
            signaling[1],
            DEFAULT_REVERSE_TRAFFIC_CHANNEL_MAC_PROTOCOL_TYPE
        );
        assert_eq!(
            &signaling[2..],
            encode_reverse_traffic_mac_grant_message(&grants).as_slice()
        );
    }
}
