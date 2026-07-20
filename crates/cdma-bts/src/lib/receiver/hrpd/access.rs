//! HRPD Reverse Access Channel definitions.
//!
//! Spec references (C.S0024-0 v4.0, the unified HRPD Rev 0 spec):
//! - §9.1.2.2  Access Channel Physical Layer Packet Format
//! - §9.2.1.3.2 Access Channel (reverse PHY structure)
//! - §9.2.1.3.8.2 Long Codes (generator polynomial, mask application)
//! - §8.3 Default Access Channel MAC Protocol
//! - §8.3.6.1.4.1.2 Access Channel Long Code Mask
//!
//! Rev A Enhanced Access Channel MAC (subtype 1) references (C.S0024-A v3.0):
//! - §10.5.6.1.4.1.2 Probe Transmission (rate selection, probe structure)
//! - §10.5.6.2.6 AccessParameters (enhanced broadcast fields)
//! - §13.2.1.3.2 / Table 13.2.1.3.4-1 Access Channel rates and encoding
//!
//! The access burst is composed of two physical sub-channels transmitted
//! simultaneously by the AT:
//!   * Pilot sub-channel: unmodulated, Walsh 0 cover, used for acquisition
//!     and coherent demodulation.
//!   * Data sub-channel: BPSK on the Q branch with a Walsh-4 cover, carrying
//!     the ACMAC capsule. Rev 0 (subtype 0) always transmits 9.6 kbps
//!     256-bit packets; subtype 1 may also transmit 19.2/38.4 kbps 512/1024-
//!     bit packets, all rate-1/4 turbo coded with interleaved-packet
//!     repetition filling the same 16-slot packet, so modulation and
//!     spreading are unchanged and only the repetition factor differs. The
//!     rate is not signaled; the receiver hypothesizes the enabled sizes.
//!
//! A burst begins with a pilot-only preamble whose length is dictated by the
//! sector's `AccessParameters` (`PreambleLength` frames, or Rev A
//! `PreambleLengthSlots`) and is followed by the capsule. The preamble is
//! transmitted at the 9.6 kbps data-portion power level regardless of the
//! capsule rate. The receiver uses the preamble for finger acquisition and
//! the capsule for layer-2/3 decoding.

use std::collections::{BTreeSet, HashMap};

use cdma_common::hrpd::{
    air as hrpd_air,
    traffic::{
        DEFAULT_PACKET_STREAM1_APPLICATION_PROTOCOL_TYPE,
        DEFAULT_PACKET_STREAM2_APPLICATION_PROTOCOL_TYPE,
        DEFAULT_PACKET_STREAM3_APPLICATION_PROTOCOL_TYPE,
    },
};
use num_complex::Complex32;

use crate::phy::hrpd::{
    crc::physical_crc16, interleaver::channel_interleave, turbo::HrpdTurboEncoder,
    turbo_decoder::HrpdTurboDecoder,
};
use crate::receiver::pipelined::{PipelineProcessor, SampleBlock};

/// Reverse-link chip rate for HRPD Rev 0 (identical to 1x).
pub const ACCESS_CHIP_RATE: u32 = 1_228_800;

/// Default/Subtype-0 9.6 kbps bits per Access Channel physical-layer packet
/// (C.S0024-0 v4.0 §9.1.2.2).
pub const ACCESS_FRAME_BITS: usize = 256;

/// Access Channel physical-layer packet sizes: 256 bits for the default
/// subtype (C.S0024-0 v4.0 §9.1.2.2), 512/1024 for the Enhanced ACMAC rates
/// (C.S0024-A Table 13.2.1.3.4-1).
pub const ACCESS_SUPPORTED_PACKET_BITS: [usize; 3] = [256, 512, 1024];

/// Turbo output symbols in one default 9.6 kbps access packet.
pub const ACCESS_CODE_SYMBOLS: usize = ACCESS_FRAME_BITS * 4;

/// Walsh-4 modulation symbols in one 16-slot Access Channel PHY packet. This
/// is fixed by the 307.2 ksps Access Channel modulation-symbol rate over one
/// 26.66... ms short-code period.
pub const ACCESS_MODULATION_SYMBOLS: usize = 8192;

/// Chips in one 16-slot Access Channel PHY packet.
pub const ACCESS_PACKET_CHIPS: usize = ACCESS_MODULATION_SYMBOLS * ACCESS_DATA_WALSH_LEN;

/// `AccessParameters` spec default for `PreambleLength` (C.S0024 PreambleLength,
/// in frames). The reverse-access finger despreads the capsule starting this
/// many frames past the preamble start; route the sector's configured value
/// here and fall back to this default when none is set.
pub const HRPD_DEFAULT_ACCESS_PREAMBLE_FRAMES: usize = 3;

/// Data channel Walsh cover length.
pub const ACCESS_DATA_WALSH_LEN: usize = 4;

/// Data channel Walsh function #2: + + - -.
pub const ACCESS_DATA_WALSH_2: [f32; ACCESS_DATA_WALSH_LEN] = [1.0, 1.0, -1.0, -1.0];

/// Unknown HRPD/1x capture timestamp origins can be offset from HRPD SystemTime
/// slot zero by any chip inside one 2048-chip slot. While a finger is unlocked,
/// rank candidate packet phases within this window, then lock to the first
/// CRC-valid phase.
const DEFAULT_PACKET_PHASE_SEARCH_CHIPS: i64 = 2048;

/// Decode only the strongest Walsh-2 phase candidates while unlocked. This
/// keeps first-packet discovery bounded without making the production path a
/// full turbo-decode sweep over every chip offset.
const DEFAULT_PACKET_PHASE_DECODE_CANDIDATES: usize = 96;

/// Enhanced Access Channel MAC (subtype 1) data rates, C.S0024-A
/// Table 13.2.1.3.4-1. All rates are rate-1/4 turbo coded and fill the fixed
/// 8192 modulation symbols of one 16-slot access packet via interleaved-packet
/// repetition; the preamble is always transmitted at the 9.6 kbps data-portion
/// power level (§13.2.1.3.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum HrpdAccessRate {
    Rate9k6,
    Rate19k2,
    Rate38k4,
}

impl HrpdAccessRate {
    pub const fn bps(self) -> u32 {
        match self {
            Self::Rate9k6 => 9_600,
            Self::Rate19k2 => 19_200,
            Self::Rate38k4 => 38_400,
        }
    }

    /// Access Channel physical-layer packet size in bits.
    pub const fn packet_bits(self) -> usize {
        match self {
            Self::Rate9k6 => 256,
            Self::Rate19k2 => 512,
            Self::Rate38k4 => 1024,
        }
    }

    /// Interleaved-packet repetition factor filling the 8192 modulation
    /// symbols of a 16-slot access packet (C.S0024-A §13.2.1.3.6).
    pub const fn sequence_repeats(self) -> usize {
        ACCESS_MODULATION_SYMBOLS / (self.packet_bits() * 4)
    }

    /// Minimum MAC payload size selecting this rate, Table 10.5.6.1.4.1.2-1.
    pub const fn min_payload_bits(self) -> usize {
        match self {
            Self::Rate9k6 => 1,
            Self::Rate19k2 => 233,
            Self::Rate38k4 => 489,
        }
    }

    /// Maximum MAC payload size for this rate, Table 10.5.6.1.4.1.2-1.
    pub const fn max_payload_bits(self) -> usize {
        match self {
            Self::Rate9k6 => 232,
            Self::Rate19k2 => 488,
            Self::Rate38k4 => 1000,
        }
    }

    pub const fn from_packet_bits(packet_bits: usize) -> Option<Self> {
        match packet_bits {
            256 => Some(Self::Rate9k6),
            512 => Some(Self::Rate19k2),
            1024 => Some(Self::Rate38k4),
            _ => None,
        }
    }

    /// Map a broadcast `SectorAccessMaxRate` code (Table 10.5.6.2.6-3).
    pub const fn from_sector_access_max_rate_code(code: u8) -> Option<Self> {
        match code {
            0b00 => Some(Self::Rate9k6),
            0b01 => Some(Self::Rate19k2),
            0b10 => Some(Self::Rate38k4),
            _ => None,
        }
    }
}

/// Select the Access Channel transmit rate for a MAC payload per Table
/// 10.5.6.1.4.1.2-1, capped by `AccessRateMax`. Payloads above the capped
/// rate's maximum fragment across capsule frames at the capped rate.
pub fn access_rate_for_payload_bits(
    payload_bits: usize,
    access_rate_max: HrpdAccessRate,
) -> Option<HrpdAccessRate> {
    let rate = match payload_bits {
        1..=232 => HrpdAccessRate::Rate9k6,
        233..=488 => HrpdAccessRate::Rate19k2,
        489..=1000 => HrpdAccessRate::Rate38k4,
        _ => return None,
    };
    Some(rate.min(access_rate_max))
}

/// Receiver-side rate hypotheses for one 16-slot access packet. The rate is
/// not signaled on the air; the decoder trials each enabled packet size and
/// selects by turbo-decode FCS success.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct HrpdAccessDecodeConfig {
    /// Also hypothesize the Enhanced Access Channel MAC 19.2/38.4 kbps packet
    /// sizes. Off by default: a Rev 0 sector only ever receives 9.6 kbps
    /// probes, and each extra hypothesis costs a turbo decode per candidate.
    pub enhanced_rates: bool,
}

impl HrpdAccessDecodeConfig {
    pub const REV0: Self = Self {
        enhanced_rates: false,
    };
    pub const ENHANCED: Self = Self {
        enhanced_rates: true,
    };

    pub const fn packet_bits_hypotheses(self) -> &'static [usize] {
        const REV0_PACKET_BITS: [usize; 1] = [ACCESS_FRAME_BITS];
        const ENHANCED_PACKET_BITS: [usize; 3] = [256, 512, 1024];
        if self.enhanced_rates {
            &ENHANCED_PACKET_BITS
        } else {
            &REV0_PACKET_BITS
        }
    }
}

const ACCESS_MAC_MIN_LENGTH_OCTETS: usize = 7;
const ACCESS_MAC_DEFAULT_CAPSULE_LENGTH_MAX: usize = 8;
const GENERIC_SECURITY_HEADER_OCTETS: usize = 2;
const SHA1_ACCESS_AUTH_HEADER_OCTETS: usize = 8;

/// Bit layout of a single access channel physical-layer packet.
///
/// Field sizes per C.S0024-0 v4.0 §9.1.2.2 / Figure 9.1.2.2-1. This is the
/// pre-encode information-bit layout that the physical layer encodes with a
/// rate-1/4 turbo code below this boundary.
#[derive(Debug, Clone, Copy)]
pub struct AccessFrameLayout {
    /// MAC layer packet length in bits (input from the Access Channel MAC).
    pub body_bits: usize,
    /// Frame Check Sequence length in bits (see §9.1.4).
    pub crc_bits: usize,
    /// Encoder tail bits (all zeros) appended after the FCS.
    pub tail_bits: usize,
}

impl AccessFrameLayout {
    /// Default/Subtype-0 256-bit Access Channel physical-layer packet layout.
    ///
    /// Per C.S0024-0 v4.0 §9.1.2.2, Figure 9.1.2.2-1:
    ///   MAC Layer Packet = 234 bits
    ///   FCS              = 16 bits
    ///   TAIL             = 6 bits
    ///   Total            = 256 bits.
    ///
    pub const DEFAULT: Self = Self {
        body_bits: 234,
        crc_bits: 16,
        tail_bits: 6,
    };

    pub const fn for_packet_bits(packet_bits: usize) -> Option<Self> {
        match packet_bits {
            256 | 512 | 1024 | 2048 => Some(Self {
                body_bits: packet_bits - 16 - 6,
                crc_bits: 16,
                tail_bits: 6,
            }),
            _ => None,
        }
    }

    /// Convenience: total bits, must equal the physical-layer packet size.
    pub const fn total_bits(&self) -> usize {
        self.body_bits + self.crc_bits + self.tail_bits
    }
}

/// Layer-3 access channel messages we expect to decode from a capsule.
///
/// Refer to C.S0024-300 §10 (ACMAC) and the Session/Connection/Route Update
/// protocols layered above it. This enum is intentionally narrow; additional
/// variants will appear once each protocol decoder is implemented.
#[derive(Debug, Clone)]
pub enum AccessMessage {
    /// `ConnectionRequest` from the Connection Layer.
    ConnectionRequest,
    /// `UATIRequest` from the Address Management Protocol.
    UatiRequest { color_code: u8 },
    /// `RouteUpdate` from the Route Update Protocol (body not parsed here).
    RouteUpdate(Vec<u8>),
    /// Session/idle keep-alive style messages.
    KeepAlive,
    /// Any message not yet decoded — raw bits preserved for inspection.
    Unknown(Vec<u8>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccessDecodeError {
    WrongLength,
    CrcMismatch,
    UnknownMessageId(u8),
}

impl core::fmt::Display for AccessDecodeError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::WrongLength => write!(f, "wrong-length access frame"),
            Self::CrcMismatch => write!(f, "access CRC mismatch"),
            Self::UnknownMessageId(id) => write!(f, "unknown access MessageID 0x{id:02x}"),
        }
    }
}

impl std::error::Error for AccessDecodeError {}

/// CRC-clean HRPD access physical-layer packet.
#[derive(Debug, Clone)]
pub struct AccessPhyPacket {
    /// Physical-layer packet size in bits.
    pub packet_bits: usize,
    /// All decoded physical packet bits. The last six are the all-zero
    /// packet tail field.
    pub bits: Vec<u8>,
    /// First 234 MAC packet bits.
    pub info_bits: Vec<u8>,
    /// Observed 16-bit FCS field.
    pub fcs: u16,
    /// Decoded MessageID if the first octet maps to a known parser.
    pub message: Result<AccessMessage, AccessDecodeError>,
    /// MessageID octet from the MAC packet body.
    pub message_id: u8,
}

/// Best-effort access PHY decode attempt, including CRC distance even when
/// the FCS does not validate.
#[derive(Debug, Clone)]
pub struct AccessPhyDecodeAttempt {
    pub packet_bits: usize,
    pub bits: Vec<u8>,
    pub info_bits: Vec<u8>,
    pub posterior_llrs: Vec<f32>,
    pub expected_fcs: u16,
    pub observed_fcs: u16,
    pub fcs_bit_errors: u32,
    pub tail_ones: usize,
    pub message_id: u8,
    pub variant: &'static str,
    pub llr_scale: f32,
    pub turbo_iterations: usize,
}

#[derive(Debug, Clone)]
pub struct AccessMacFragmentCheck {
    pub valid: bool,
    pub detail: String,
    pub length_octets: Option<usize>,
    pub required_fragments: Option<usize>,
    pub reserved_zero: bool,
    pub single_fragment_fcs_valid: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AccessTerminalIdentifierType {
    Bati,
    Reserved,
    Uati,
    Rati,
}

impl AccessTerminalIdentifierType {
    fn from_bits(bits: u8) -> Self {
        match bits & 0b11 {
            0b00 => Self::Bati,
            0b01 => Self::Reserved,
            0b10 => Self::Uati,
            _ => Self::Rati,
        }
    }

    fn as_bits(self) -> u8 {
        match self {
            Self::Bati => 0b00,
            Self::Reserved => 0b01,
            Self::Uati => 0b10,
            Self::Rati => 0b11,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessTerminalIdentifierRecord {
    pub ati_type: AccessTerminalIdentifierType,
    pub ati: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HrpdRouteUpdate {
    pub message_sequence: u8,
    pub reference_pilot_pn: u16,
    pub reference_pilot_strength: u8,
    pub reference_keep: bool,
    pub num_pilots: u8,
    pub at_total_pilot_transmission: Option<i8>,
    pub reference_pilot_channel: Option<u32>,
    pub reserved_zero: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HrpdUatiRequest {
    pub transaction_id: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HrpdUatiComplete {
    pub message_sequence: u8,
    pub upper_old_uati: Vec<u8>,
    pub reserved_zero: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HrpdConnectionRequest {
    pub transaction_id: u8,
    pub request_reason: u8,
    pub reserved_zero: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HrpdTrafficChannelComplete {
    pub message_sequence: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HrpdSessionClose {
    pub close_reason: u8,
    pub more_info: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HrpdConnectionClose {
    pub close_reason: u8,
    pub suspend_enable: bool,
    pub suspend_time: Option<u64>,
    pub reserved_zero: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HrpdHardwareIdResponse {
    pub transaction_id: u8,
    pub hardware_id_type: u32,
    pub hardware_id_value: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HrpdDefaultPacketDataReadyAck {
    pub transaction_id: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HrpdAccessSignalingMessage {
    RouteUpdate(HrpdRouteUpdate),
    UatiRequest(HrpdUatiRequest),
    UatiComplete(HrpdUatiComplete),
    ConnectionRequest(HrpdConnectionRequest),
    TrafficChannelComplete(HrpdTrafficChannelComplete),
    SessionClose(HrpdSessionClose),
    ConnectionClose(HrpdConnectionClose),
    HardwareIdResponse(HrpdHardwareIdResponse),
    DefaultPacketXonRequest,
    DefaultPacketXoffRequest,
    DefaultPacketDataReadyAck(HrpdDefaultPacketDataReadyAck),
    Unknown {
        protocol_type: u8,
        message_id: Option<u8>,
        payload: Vec<u8>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HrpdDefaultSignalingPacket {
    pub stream: u8,
    pub protocol_type: u8,
    pub in_configuration: bool,
    pub payload: Vec<u8>,
    pub message: HrpdAccessSignalingMessage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessMacCapsule {
    pub length_octets: usize,
    pub session_configuration_token: u16,
    pub security_layer_format: bool,
    pub connection_layer_format: bool,
    pub ati: AccessTerminalIdentifierRecord,
    pub security_payload: Vec<u8>,
    pub mac_fcs: u32,
    pub messages: Vec<HrpdDefaultSignalingPacket>,
}

impl AccessMacCapsule {
    pub fn to_air_indication(
        &self,
        absolute_chip: u64,
        color_code: u8,
        sector_pilot_pn: u16,
    ) -> hrpd_air::HrpdAccessIndication {
        hrpd_air::HrpdAccessIndication {
            absolute_chip,
            color_code,
            sector_pilot_pn,
            session_configuration_token: self.session_configuration_token,
            ati: hrpd_air::AccessTerminalIdentifier {
                ati_type: match self.ati.ati_type {
                    AccessTerminalIdentifierType::Bati => {
                        hrpd_air::AccessTerminalIdentifierType::Bati
                    }
                    AccessTerminalIdentifierType::Reserved => {
                        hrpd_air::AccessTerminalIdentifierType::Reserved
                    }
                    AccessTerminalIdentifierType::Uati => {
                        hrpd_air::AccessTerminalIdentifierType::Uati
                    }
                    AccessTerminalIdentifierType::Rati => {
                        hrpd_air::AccessTerminalIdentifierType::Rati
                    }
                },
                value: self.ati.ati,
            },
            security_layer_format: self.security_layer_format,
            connection_layer_format: self.connection_layer_format,
            security_payload: self.security_payload.clone(),
            messages: self
                .messages
                .iter()
                .map(|packet| to_air_access_message(&packet.message))
                .collect(),
        }
    }

    pub fn summary(&self) -> String {
        let messages = self
            .messages
            .iter()
            .map(|packet| {
                let decoded = match &packet.message {
                HrpdAccessSignalingMessage::RouteUpdate(route) => format!(
                    "RouteUpdate(seq={} ref_pn={} strength={} keep={} pilots={} at_tx={:?} ref_chan={:?} reserved_zero={})",
                    route.message_sequence,
                    route.reference_pilot_pn,
                    route.reference_pilot_strength,
                    route.reference_keep,
                    route.num_pilots,
                    route.at_total_pilot_transmission,
                    route.reference_pilot_channel,
                    route.reserved_zero,
                ),
                HrpdAccessSignalingMessage::UatiRequest(uati) => {
                    format!("UATIRequest(transaction=0x{:02x})", uati.transaction_id)
                }
                HrpdAccessSignalingMessage::UatiComplete(uati) => format!(
                    "UATIComplete(seq={} upper_old_len={} reserved_zero={})",
                    uati.message_sequence,
                    uati.upper_old_uati.len(),
                    uati.reserved_zero
                ),
                HrpdAccessSignalingMessage::ConnectionRequest(connection) => format!(
                    "ConnectionRequest(transaction=0x{:02x} reason={} reserved_zero={})",
                    connection.transaction_id,
                    connection.request_reason,
                    connection.reserved_zero
                ),
                HrpdAccessSignalingMessage::TrafficChannelComplete(complete) => {
                    format!("TrafficChannelComplete(seq={})", complete.message_sequence)
                }
                HrpdAccessSignalingMessage::SessionClose(close) => format!(
                    "SessionClose(reason=0x{:02x}({}) more_info_len={} more_info_protocol={})",
                    close.close_reason,
                    hrpd_air::hrpd_session_close_reason_name(close.close_reason),
                    close.more_info.len(),
                    hrpd_air::hrpd_protocol_reference_from_more_info(&close.more_info)
                        .map(|reference| format!(
                            "0x{:x}/0x{:04x}",
                            reference.protocol_type, reference.protocol_subtype
                        ))
                        .unwrap_or_else(|| "none".to_string())
                ),
                HrpdAccessSignalingMessage::ConnectionClose(close) => format!(
                    "ConnectionClose(reason=0x{:x}({}) suspend_enable={} suspend_time={:?} reserved_zero={})",
                    close.close_reason,
                    hrpd_air::hrpd_connection_close_reason_name(close.close_reason),
                    close.suspend_enable,
                    close.suspend_time,
                    close.reserved_zero
                ),
                HrpdAccessSignalingMessage::HardwareIdResponse(hardware) => format!(
                    "HardwareIDResponse(transaction=0x{:02x} type=0x{:06x} len={} value={})",
                    hardware.transaction_id,
                    hardware.hardware_id_type,
                    hardware.hardware_id_value.len(),
                    hex_bytes(&hardware.hardware_id_value),
                ),
                HrpdAccessSignalingMessage::DefaultPacketXonRequest => {
                    "DefaultPacketXonRequest".to_string()
                }
                HrpdAccessSignalingMessage::DefaultPacketXoffRequest => {
                    "DefaultPacketXoffRequest".to_string()
                }
                HrpdAccessSignalingMessage::DefaultPacketDataReadyAck(ack) => {
                    format!(
                        "DefaultPacketDataReadyAck(transaction=0x{:02x})",
                        ack.transaction_id
                    )
                }
                HrpdAccessSignalingMessage::Unknown {
                    protocol_type,
                    message_id,
                    payload,
                } => format!(
                    "Unknown(protocol=0x{protocol_type:02x} msg={message_id:?} payload_len={})",
                    payload.len()
                ),
                };
                format!(
                    "p=0x{:02x}/ic={} payload={} {}",
                    packet.protocol_type,
                    u8::from(packet.in_configuration),
                    hex_bytes(&packet.payload),
                    decoded
                )
            })
            .collect::<Vec<_>>()
            .join(", ");
        format!(
            "len={} sct=0x{:04x} sec={} conn_b={} ati={:?}/0x{:08x} mac_fcs=0x{:08x} messages=[{}]",
            self.length_octets,
            self.session_configuration_token,
            u8::from(self.security_layer_format),
            u8::from(self.connection_layer_format),
            self.ati.ati_type,
            self.ati.ati,
            self.mac_fcs,
            messages,
        )
    }

    pub fn format_b_parse_trace(&self) -> String {
        if !self.connection_layer_format {
            return "not-format-b".to_string();
        }
        let mut out = Vec::new();
        for (label, payload) in format_b_connection_payload_candidates(
            self.security_layer_format,
            &self.security_payload,
        ) {
            let mut cursor = 0usize;
            while cursor < payload.len() {
                let length_offset = cursor;
                let length = payload[cursor] as usize;
                cursor += 1;
                if length == 0 {
                    out.push(format!("{label}:off={length_offset} len=0 end"));
                    break;
                }
                let end = cursor.saturating_add(length);
                if end > payload.len() {
                    out.push(format!(
                        "{label}:off={length_offset} len={length} truncated remaining={}",
                        payload.len().saturating_sub(cursor)
                    ));
                    break;
                }
                let raw = &payload[cursor..end];
                match parse_default_signaling_packet(raw) {
                    Some(packet) => out.push(format!(
                        "{label}:off={length_offset} len={length} stream={} proto=0x{:02x} ic={} payload={}",
                        packet.stream,
                        packet.protocol_type,
                        u8::from(packet.in_configuration),
                        hex_bytes(&packet.payload)
                    )),
                    None => out.push(format!(
                        "{label}:off={length_offset} len={length} undecoded raw={}",
                        hex_bytes(raw)
                    )),
                }
                cursor = end;
            }
        }
        if out.is_empty() {
            "empty-payload".to_string()
        } else {
            out.join("; ")
        }
    }

    pub fn security_payload_hex(&self) -> String {
        hex_bytes(&self.security_payload)
    }
}

fn to_air_access_message(message: &HrpdAccessSignalingMessage) -> hrpd_air::HrpdAccessMessage {
    match message {
        HrpdAccessSignalingMessage::RouteUpdate(route) => {
            hrpd_air::HrpdAccessMessage::RouteUpdate(hrpd_air::HrpdRouteUpdate {
                message_sequence: route.message_sequence,
                reference_pilot_pn: route.reference_pilot_pn,
                reference_pilot_strength: route.reference_pilot_strength,
                reference_keep: route.reference_keep,
                num_pilots: route.num_pilots,
                at_total_pilot_transmission: route.at_total_pilot_transmission,
                reference_pilot_channel: route.reference_pilot_channel,
                reserved_zero: route.reserved_zero,
            })
        }
        HrpdAccessSignalingMessage::UatiRequest(uati) => {
            hrpd_air::HrpdAccessMessage::UatiRequest(hrpd_air::HrpdUatiRequest {
                transaction_id: uati.transaction_id,
            })
        }
        HrpdAccessSignalingMessage::UatiComplete(uati) => {
            hrpd_air::HrpdAccessMessage::UatiComplete(hrpd_air::HrpdUatiComplete {
                message_sequence: uati.message_sequence,
                upper_old_uati: uati.upper_old_uati.clone(),
                reserved_zero: uati.reserved_zero,
            })
        }
        HrpdAccessSignalingMessage::ConnectionRequest(connection) => {
            hrpd_air::HrpdAccessMessage::ConnectionRequest(hrpd_air::HrpdConnectionRequest {
                transaction_id: connection.transaction_id,
                request_reason: connection.request_reason,
                reserved_zero: connection.reserved_zero,
            })
        }
        HrpdAccessSignalingMessage::TrafficChannelComplete(complete) => {
            hrpd_air::HrpdAccessMessage::TrafficChannelComplete(
                hrpd_air::HrpdTrafficChannelComplete {
                    message_sequence: complete.message_sequence,
                },
            )
        }
        HrpdAccessSignalingMessage::SessionClose(close) => {
            hrpd_air::HrpdAccessMessage::SessionClose(hrpd_air::HrpdSessionClose {
                close_reason: close.close_reason,
                more_info: close.more_info.clone(),
            })
        }
        HrpdAccessSignalingMessage::ConnectionClose(close) => {
            hrpd_air::HrpdAccessMessage::ConnectionClose(hrpd_air::HrpdConnectionClose {
                close_reason: close.close_reason,
                suspend_enable: close.suspend_enable,
                suspend_time: close.suspend_time,
                reserved_zero: close.reserved_zero,
            })
        }
        HrpdAccessSignalingMessage::HardwareIdResponse(hardware) => {
            hrpd_air::HrpdAccessMessage::HardwareIdResponse(hrpd_air::HrpdHardwareIdResponse {
                transaction_id: hardware.transaction_id,
                hardware_id_type: hardware.hardware_id_type,
                hardware_id_value: hardware.hardware_id_value.clone(),
            })
        }
        HrpdAccessSignalingMessage::DefaultPacketXonRequest => {
            hrpd_air::HrpdAccessMessage::DefaultPacketXonRequest
        }
        HrpdAccessSignalingMessage::DefaultPacketXoffRequest => {
            hrpd_air::HrpdAccessMessage::DefaultPacketXoffRequest
        }
        HrpdAccessSignalingMessage::DefaultPacketDataReadyAck(ack) => {
            hrpd_air::HrpdAccessMessage::DefaultPacketDataReadyAck(
                hrpd_air::HrpdDefaultPacketDataReadyAck {
                    transaction_id: ack.transaction_id,
                },
            )
        }
        HrpdAccessSignalingMessage::Unknown {
            protocol_type,
            message_id,
            payload,
        } => hrpd_air::HrpdAccessMessage::Unknown {
            protocol_type: *protocol_type,
            message_id: *message_id,
            payload: payload.clone(),
        },
    }
}

/// HRPD Rev 0 access frame decoder.
///
/// Operates on **hard-decided info bits** (234 bits) plus the 16-bit CRC. The
/// soft-turbo + deinterleave + depuncture front end is not yet wired here; the
/// helper `decode_info_bits` takes the post-turbo info+CRC bit stream as
/// produced by `cfg(test)` encoders below. When a soft turbo decoder lands,
/// it will feed the same `decode_info_bits` entry point.
///
/// MessageID dispatch references C.S0024-400 §6.4 (ConnectionRequest), §5.2
/// (KeepAliveResponse), §6.6 (RouteUpdate), §5.3 (UATIRequest).
pub struct AccessFrameDecoder;

impl AccessFrameDecoder {
    /// Verify a MAC packet body plus 16-bit physical FCS field.
    pub fn validate_info_bits(info_plus_crc: &[u8]) -> Result<(), AccessDecodeError> {
        let Some(layout) = AccessFrameLayout::for_packet_bits(info_plus_crc.len() + 6) else {
            return Err(AccessDecodeError::WrongLength);
        };
        let (info, crc_bits) = info_plus_crc.split_at(layout.body_bits);
        let observed = physical_crc16(info);
        let expected_crc = pack_u16_msb(crc_bits);
        if observed != expected_crc {
            return Err(AccessDecodeError::CrcMismatch);
        }
        Ok(())
    }

    /// Decode an info+CRC bit stream into an `AccessMessage`. The 6 trailing
    /// turbo-tail bits are not part of the CRC scope and must already be
    /// stripped.
    pub fn decode_info_bits(info_plus_crc: &[u8]) -> Result<AccessMessage, AccessDecodeError> {
        let layout = AccessFrameLayout::for_packet_bits(info_plus_crc.len() + 6)
            .ok_or(AccessDecodeError::WrongLength)?;
        Self::validate_info_bits(info_plus_crc)?;
        let (info, crc_bits) = info_plus_crc.split_at(layout.body_bits);
        let _ = crc_bits;
        let msg_id = pack_u8_msb(&info[..8]);
        match msg_id {
            0x01 => Ok(AccessMessage::ConnectionRequest),
            0x03 => Ok(AccessMessage::KeepAlive),
            0x09 => Ok(AccessMessage::RouteUpdate(pack_bytes_msb(&info[8..]))),
            0x12 => Ok(AccessMessage::UatiRequest {
                color_code: pack_u8_msb(&info[8..16]),
            }),
            other => Err(AccessDecodeError::UnknownMessageId(other)),
        }
    }
}

/// Build one complete 256-bit Access Channel PHY packet from a 234-bit MAC
/// packet body. Appends FCS and the six all-zero packet-tail bits.
pub fn build_access_phy_bits(info_bits: &[u8]) -> Vec<u8> {
    build_access_phy_bits_for_packet_bits(info_bits, ACCESS_FRAME_BITS)
}

pub fn build_access_phy_bits_for_packet_bits(info_bits: &[u8], packet_bits: usize) -> Vec<u8> {
    let layout =
        AccessFrameLayout::for_packet_bits(packet_bits).expect("supported access PHY packet size");
    assert_eq!(info_bits.len(), layout.body_bits);
    let crc = physical_crc16(info_bits);
    let mut out = Vec::with_capacity(packet_bits);
    out.extend(info_bits.iter().map(|b| b & 1));
    for i in (0..layout.crc_bits).rev() {
        out.push(((crc >> i) & 1) as u8);
    }
    out.extend(std::iter::repeat(0).take(layout.tail_bits));
    out
}

/// Spec-bound Access Channel PHY encoder used by tests and diagnostics:
/// rate-1/4 turbo encode, 1024-symbol channel interleave, eightfold sequence
/// repetition, BPSK mapping where bit 0 is +1 and bit 1 is -1.
pub fn encode_access_phy_soft_symbols(frame_bits: &[u8]) -> Vec<f32> {
    let packet_bits = frame_bits.len();
    assert!(
        AccessFrameLayout::for_packet_bits(packet_bits).is_some(),
        "unsupported access PHY packet size {packet_bits}",
    );
    let encoder = HrpdTurboEncoder::new(packet_bits as u32).expect("access turbo block");
    let coded = encoder.encode(frame_bits, 1, 4);
    let code_symbols = packet_bits * 4;
    assert_eq!(coded.len(), code_symbols);
    let repeats = access_packet_repeats(packet_bits).expect("supported access rate");
    let interleaved = channel_interleave(code_symbols, &coded);
    let mut out = Vec::with_capacity(ACCESS_MODULATION_SYMBOLS);
    for _ in 0..repeats {
        out.extend(interleaved.iter().map(|&b| if b == 0 { 1.0 } else { -1.0 }));
    }
    debug_assert_eq!(out.len(), ACCESS_MODULATION_SYMBOLS);
    out
}

/// Decode one 16-slot access PHY packet from Walsh-4 soft modulation symbols.
/// Positive soft values mean transmitted bit 0, negative values mean bit 1.
pub fn decode_access_phy_soft_symbols(soft_symbols: &[f32]) -> Option<AccessPhyPacket> {
    decode_access_phy_soft_symbols_with_config(soft_symbols, HrpdAccessDecodeConfig::REV0)
}

pub fn decode_access_phy_soft_symbols_with_config(
    soft_symbols: &[f32],
    config: HrpdAccessDecodeConfig,
) -> Option<AccessPhyPacket> {
    let attempt = decode_access_phy_soft_symbols_attempt(soft_symbols, "soft", config)?;
    if attempt.fcs_bit_errors != 0 {
        return None;
    }
    access_phy_packet_from_bits(attempt.bits)
}

pub fn decode_access_phy_soft_symbols_attempt(
    soft_symbols: &[f32],
    variant: &'static str,
    config: HrpdAccessDecodeConfig,
) -> Option<AccessPhyDecodeAttempt> {
    let (llr_scale, iterations) = access_decode_llr_params();
    decode_access_phy_soft_symbols_attempt_with_params(
        soft_symbols,
        variant,
        llr_scale,
        iterations,
        config,
    )
}

pub fn decode_access_phy_soft_symbols_attempt_with_params(
    soft_symbols: &[f32],
    variant: &'static str,
    llr_scale: f32,
    iterations: usize,
    config: HrpdAccessDecodeConfig,
) -> Option<AccessPhyDecodeAttempt> {
    let mut best: Option<AccessPhyDecodeAttempt> = None;
    for &packet_bits in config.packet_bits_hypotheses() {
        if let Some(attempt) = decode_access_phy_soft_symbols_attempt_for_packet_bits(
            soft_symbols,
            variant,
            llr_scale,
            iterations,
            packet_bits,
        ) {
            let optimal = access_attempt_is_optimal(&attempt);
            if best
                .as_ref()
                .is_none_or(|best_attempt: &AccessPhyDecodeAttempt| {
                    access_attempt_rank(&attempt) < access_attempt_rank(best_attempt)
                })
            {
                best = Some(attempt);
            }
            // A CRC-clean, tail-clean, MAC-valid decode at a smaller packet
            // size cannot be outranked; skip the remaining (costlier)
            // hypotheses.
            if optimal {
                break;
            }
        }
    }
    best
}

/// Soft-decision scale applied to access-channel turbo LLRs. Unity: the
/// demodulated LLRs feed the decoder as-is.
const ACCESS_DECODE_LLR_SCALE: f32 = 1.0;
/// Turbo decoder iterations for access-channel packet decode.
const ACCESS_DECODE_TURBO_ITERATIONS: usize = 12;

fn access_decode_llr_params() -> (f32, usize) {
    (ACCESS_DECODE_LLR_SCALE, ACCESS_DECODE_TURBO_ITERATIONS)
}

fn access_attempt_is_optimal(attempt: &AccessPhyDecodeAttempt) -> bool {
    attempt.fcs_bit_errors == 0
        && attempt.tail_ones == 0
        && validate_access_mac_fragment(&attempt.info_bits).valid
}

pub fn decode_access_phy_soft_symbols_attempt_for_packet_bits(
    soft_symbols: &[f32],
    variant: &'static str,
    llr_scale: f32,
    iterations: usize,
    packet_bits: usize,
) -> Option<AccessPhyDecodeAttempt> {
    decode_access_phy_soft_symbols_attempt_with_params_and_repeat_weights_for_packet_bits(
        soft_symbols,
        variant,
        llr_scale,
        iterations,
        None,
        packet_bits,
    )
}

fn decode_access_phy_soft_symbols_attempt_with_params_and_repeat_weights_for_packet_bits(
    soft_symbols: &[f32],
    variant: &'static str,
    llr_scale: f32,
    iterations: usize,
    repeat_weights: Option<&[f32]>,
    packet_bits: usize,
) -> Option<AccessPhyDecodeAttempt> {
    if soft_symbols.len() != ACCESS_MODULATION_SYMBOLS {
        return None;
    }
    let code_symbols = packet_bits.checked_mul(4)?;
    let repeats = access_packet_repeats(packet_bits)?;
    if let Some(weights) = repeat_weights
        && weights.len() != repeats
    {
        return None;
    }

    let mut repeated = vec![0.0f32; code_symbols];
    for rep in 0..repeats {
        let weight = repeat_weights.map(|weights| weights[rep]).unwrap_or(1.0);
        let base = rep * code_symbols;
        for (dst, &src) in repeated
            .iter_mut()
            .zip(&soft_symbols[base..base + code_symbols])
        {
            *dst += src * weight;
        }
    }

    let deinterleaved = bit_reversal_deinterleave_f32(repeated, code_symbols);
    let mut mother_llrs = depuncture_rate_1_4_to_mother_rate_1_5(&deinterleaved, packet_bits);
    if (llr_scale - 1.0).abs() > f32::EPSILON {
        for llr in &mut mother_llrs {
            *llr *= llr_scale;
        }
    }
    let decoder = HrpdTurboDecoder::new(packet_bits as u32)?.with_iterations(iterations);
    let posterior_llrs = decoder.decode_soft(&mother_llrs);
    let bits = posterior_llrs
        .iter()
        .map(|llr| if *llr >= 0.0 { 0 } else { 1 })
        .collect::<Vec<_>>();
    access_phy_decode_attempt_from_bits(
        bits,
        posterior_llrs,
        variant,
        llr_scale,
        iterations,
        packet_bits,
    )
}

/// Decode one 16-slot access PHY packet from HPSK/PN/LC-despread chip-rate
/// samples. The packet is carried on the Q channel using Walsh-4 #2.
pub fn decode_access_phy_chips(chips: &[Complex32]) -> Option<AccessPhyPacket> {
    decode_access_phy_chips_with_config(chips, HrpdAccessDecodeConfig::REV0)
}

pub fn decode_access_phy_chips_with_config(
    chips: &[Complex32],
    config: HrpdAccessDecodeConfig,
) -> Option<AccessPhyPacket> {
    decode_access_phy_chips_with_attempt(chips, config).1
}

/// One decode pass returning both the best-effort attempt (for failure
/// diagnostics) and the validated packet when that attempt is CRC-clean.
/// Callers that want both must use this instead of calling the attempt and
/// packet functions separately — each runs the full hypothesis sweep.
pub fn decode_access_phy_chips_with_attempt(
    chips: &[Complex32],
    config: HrpdAccessDecodeConfig,
) -> (Option<AccessPhyDecodeAttempt>, Option<AccessPhyPacket>) {
    let attempt = decode_access_phy_chips_fast_attempt(chips, config)
        .or_else(|| decode_access_phy_chips_attempt_with_config(chips, config));
    let packet = attempt
        .as_ref()
        .filter(|attempt| attempt.fcs_bit_errors == 0)
        .and_then(|attempt| access_phy_packet_from_bits(attempt.bits.clone()));
    (attempt, packet)
}

fn decode_access_phy_chips_fast_attempt(
    chips: &[Complex32],
    config: HrpdAccessDecodeConfig,
) -> Option<AccessPhyDecodeAttempt> {
    if chips.len() != ACCESS_PACKET_CHIPS {
        return None;
    }
    let (llr_scale, iterations) = access_decode_llr_params();
    for (q_polarity, label) in [(1.0, "q+fast"), (-1.0, "q-fast")] {
        let soft = access_data_walsh_soft_symbols(chips, q_polarity);
        for &packet_bits in config.packet_bits_hypotheses() {
            let Some(attempt) = decode_access_phy_soft_symbols_attempt_for_packet_bits(
                &soft,
                label,
                llr_scale,
                iterations,
                packet_bits,
            ) else {
                continue;
            };
            if attempt.fcs_bit_errors == 0 && attempt.tail_ones == 0 {
                return Some(attempt);
            }
        }
    }
    None
}

pub fn decode_access_phy_chips_attempt(chips: &[Complex32]) -> Option<AccessPhyDecodeAttempt> {
    decode_access_phy_chips_attempt_with_config(chips, HrpdAccessDecodeConfig::REV0)
}

pub fn decode_access_phy_chips_attempt_with_config(
    chips: &[Complex32],
    config: HrpdAccessDecodeConfig,
) -> Option<AccessPhyDecodeAttempt> {
    if chips.len() != ACCESS_PACKET_CHIPS {
        return None;
    }
    let mut best = decode_access_phy_chips_attempt_for_view(chips, "q+", "q-", config);

    // The pilot-equalization sweep below only exists to clean up a marginal
    // decode. If the un-equalized decode is already optimal — CRC-clean, no
    // stray tail ones, MAC fragment valid — it occupies rank `(0, 0, 0, ..)`
    // and no equalized attempt can strictly beat it (a CRC-valid decode of the
    // same chips yields the same packet), so skip the sweep entirely.
    if best.as_ref().is_some_and(|a| {
        a.fcs_bit_errors == 0
            && a.tail_ones == 0
            && validate_access_mac_fragment(&a.info_bits).valid
    }) {
        return best;
    }

    // Pilot-equalization block sizes, spanning fast (256) to whole-packet phase
    // tracking. The finer 64/128 blocks were dropped: across the reverse-access
    // capture set they never produced the winning decode, and they dominated the
    // per-candidate cost of the unlock search.
    for (block_chips, positive_label, negative_label) in [
        (256usize, "q+eq256", "q-eq256"),
        (512usize, "q+eq512", "q-eq512"),
        (1024usize, "q+eq1024", "q-eq1024"),
        (ACCESS_PACKET_CHIPS, "q+eqpkt", "q-eqpkt"),
    ] {
        let Some(equalized) = access_pilot_equalized_chips(chips, block_chips) else {
            continue;
        };
        if let Some(candidate) = decode_access_phy_chips_attempt_for_view(
            &equalized,
            positive_label,
            negative_label,
            config,
        ) {
            if best.as_ref().is_none_or(|best_attempt| {
                access_attempt_rank(&candidate) < access_attempt_rank(best_attempt)
            }) {
                best = Some(candidate);
            }
        }
    }

    best
}

fn decode_access_phy_chips_attempt_for_view(
    chips: &[Complex32],
    positive_label: &'static str,
    negative_label: &'static str,
    config: HrpdAccessDecodeConfig,
) -> Option<AccessPhyDecodeAttempt> {
    let soft = access_data_walsh_soft_symbols(chips, 1.0);
    let normal = decode_access_phy_soft_symbols_attempt(&soft, positive_label, config);
    let inverted = access_data_walsh_soft_symbols(chips, -1.0);
    let inverted = decode_access_phy_soft_symbols_attempt(&inverted, negative_label, config);
    [normal, inverted]
        .into_iter()
        .flatten()
        .min_by_key(access_attempt_rank)
}

fn access_attempt_rank(attempt: &AccessPhyDecodeAttempt) -> (u32, usize, usize, usize) {
    let mac_invalid = usize::from(!validate_access_mac_fragment(&attempt.info_bits).valid);
    (
        attempt.fcs_bit_errors,
        attempt.tail_ones,
        mac_invalid,
        attempt.packet_bits,
    )
}

fn access_pilot_equalized_chips(chips: &[Complex32], block_chips: usize) -> Option<Vec<Complex32>> {
    if block_chips == 0
        || block_chips % ACCESS_DATA_WALSH_LEN != 0
        || chips.len() % block_chips != 0
    {
        return None;
    }

    let mut out = Vec::with_capacity(chips.len());
    for block in chips.chunks_exact(block_chips) {
        let pilot_sum = block.iter().copied().sum::<Complex32>();
        let norm = pilot_sum.norm();
        if norm <= 1.0e-9 {
            return None;
        }
        let correction = Complex32::new(pilot_sum.re / norm, -pilot_sum.im / norm);
        out.extend(block.iter().map(|chip| *chip * correction));
    }
    Some(out)
}

pub fn validate_access_mac_fragment(info_bits: &[u8]) -> AccessMacFragmentCheck {
    if info_bits.len() < AccessFrameLayout::DEFAULT.body_bits {
        return AccessMacFragmentCheck {
            valid: false,
            detail: "wrong_len".to_string(),
            length_octets: None,
            required_fragments: None,
            reserved_zero: false,
            single_fragment_fcs_valid: false,
        };
    }

    let fragment_payload_bits = info_bits.len() - 2;
    let length_octets = pack_u8_msb(&info_bits[..8]) as usize;
    let payload_bits = 8 + length_octets * 8;
    let needed_bits = payload_bits + 32;
    let required_fragments = needed_bits.div_ceil(fragment_payload_bits);
    let header_reserved_zero = info_bits[26..30].iter().all(|&bit| bit == 0);
    let fragment_reserved_zero = info_bits[fragment_payload_bits..fragment_payload_bits + 2]
        .iter()
        .all(|&bit| bit == 0);
    let reserved_zero = header_reserved_zero && fragment_reserved_zero;

    if length_octets < ACCESS_MAC_MIN_LENGTH_OCTETS {
        return AccessMacFragmentCheck {
            valid: false,
            detail: format!("length_too_small:{length_octets}"),
            length_octets: Some(length_octets),
            required_fragments: Some(required_fragments),
            reserved_zero,
            single_fragment_fcs_valid: false,
        };
    }
    if required_fragments > ACCESS_MAC_DEFAULT_CAPSULE_LENGTH_MAX {
        return AccessMacFragmentCheck {
            valid: false,
            detail: format!("capsule_too_long:length={length_octets}:need={required_fragments}"),
            length_octets: Some(length_octets),
            required_fragments: Some(required_fragments),
            reserved_zero,
            single_fragment_fcs_valid: false,
        };
    }
    // Header bits [26..30] are ProbeNumber in the Enhanced ACMAC header
    // (C.S0024-A §10.5.6.2.1: one less than the probe number within the
    // current probe sequence), and a Reserved field the AN shall ignore in
    // the default-subtype header (§10.4.6.2.1) — never a must-be-zero gate.
    // The 32-bit MAC FCS below is the authoritative frame-validity check, so
    // keep `reserved_zero` only for reporting and do not reject a CRC-valid
    // capsule on the header bits.
    if !fragment_reserved_zero {
        return AccessMacFragmentCheck {
            valid: false,
            detail: "fragment_reserved_nonzero".to_string(),
            length_octets: Some(length_octets),
            required_fragments: Some(required_fragments),
            reserved_zero,
            single_fragment_fcs_valid: false,
        };
    }

    if needed_bits <= fragment_payload_bits {
        let fcs_start = payload_bits;
        let observed = pack_u32_msb(&info_bits[fcs_start..fcs_start + 32]);
        let expected = access_mac_crc32(&info_bits[..fcs_start]);
        if observed != expected {
            return AccessMacFragmentCheck {
                valid: false,
                detail: format!(
                    "capsule_fcs_bad:length={length_octets}:computed=0x{expected:08x}:field=0x{observed:08x}"
                ),
                length_octets: Some(length_octets),
                required_fragments: Some(required_fragments),
                reserved_zero,
                single_fragment_fcs_valid: false,
            };
        }
        if info_bits[fcs_start + 32..fragment_payload_bits]
            .iter()
            .any(|&bit| bit != 0)
        {
            return AccessMacFragmentCheck {
                valid: false,
                detail: "padding_nonzero".to_string(),
                length_octets: Some(length_octets),
                required_fragments: Some(required_fragments),
                reserved_zero,
                single_fragment_fcs_valid: true,
            };
        }
        return AccessMacFragmentCheck {
            valid: true,
            detail: format!("single_fragment_fcs_ok:length={length_octets}"),
            length_octets: Some(length_octets),
            required_fragments: Some(required_fragments),
            reserved_zero,
            single_fragment_fcs_valid: true,
        };
    }

    AccessMacFragmentCheck {
        valid: true,
        detail: format!("fragment_header_ok:length={length_octets}:need={required_fragments}"),
        length_octets: Some(length_octets),
        required_fragments: Some(required_fragments),
        reserved_zero,
        single_fragment_fcs_valid: false,
    }
}

/// Reassemble a multi-fragment Access Channel MAC capsule into a synthetic
/// single-fragment packet that the downstream single-fragment validators and
/// parsers accept unchanged. `fragments` are the per-frame MAC info bits in
/// transmission order; each frame contributes its info bits minus the 2-bit
/// fragment trailer. The capsule (length octet + payload + 32-bit FCS) is
/// re-packed into the smallest synthetic PHY layout it fits, zero-padded, and
/// returns `None` unless the reassembled capsule FCS validates.
pub fn reassemble_access_mac_capsule_packet(fragments: &[&[u8]]) -> Option<AccessPhyPacket> {
    let first = fragments.first()?;
    if first.len() < AccessFrameLayout::DEFAULT.body_bits {
        return None;
    }
    let length_octets = pack_u8_msb(&first[..8]) as usize;
    let needed_bits = 8 + length_octets * 8 + 32;
    let mut capsule: Vec<u8> = Vec::with_capacity(needed_bits);
    for fragment in fragments {
        let payload_bits = fragment.len().checked_sub(2)?;
        capsule.extend_from_slice(&fragment[..payload_bits]);
    }
    if capsule.len() < needed_bits {
        return None;
    }
    // Smallest synthetic layout whose single-fragment payload region holds
    // the whole capsule (body minus the 2-bit fragment trailer).
    let packet_bits = [256usize, 512, 1024, 2048].into_iter().find(|&bits| {
        AccessFrameLayout::for_packet_bits(bits)
            .is_some_and(|layout| layout.body_bits.saturating_sub(2) >= needed_bits)
    })?;
    let layout = AccessFrameLayout::for_packet_bits(packet_bits)?;
    capsule.truncate(needed_bits);
    capsule.resize(layout.body_bits, 0);
    let check = validate_access_mac_fragment(&capsule);
    if !check.valid || !check.single_fragment_fcs_valid {
        return None;
    }
    let crc = physical_crc16(&capsule);
    let mut bits = capsule;
    for shift in (0..16).rev() {
        bits.push(((crc >> shift) & 1) as u8);
    }
    bits.resize(layout.total_bits(), 0);
    access_phy_packet_from_bits(bits)
}

pub fn parse_access_mac_capsule(info_bits: &[u8]) -> Option<AccessMacCapsule> {
    let mac_check = validate_access_mac_fragment(info_bits);
    if !mac_check.valid || !mac_check.single_fragment_fcs_valid {
        return None;
    }
    let length_octets = mac_check.length_octets?;
    let payload_bits = 8 + length_octets * 8;
    let fcs_start = payload_bits;
    let payload_end = fcs_start + 32;
    if length_octets < ACCESS_MAC_MIN_LENGTH_OCTETS || info_bits.len() < payload_end {
        return None;
    }

    let session_configuration_token = pack_u16_msb(&info_bits[8..24]);
    let security_layer_format = info_bits[24] != 0;
    let connection_layer_format = info_bits[25] != 0;
    let ati_type_bits = ((info_bits[30] & 1) << 1) | (info_bits[31] & 1);
    let ati = pack_u32_msb(&info_bits[32..64]);
    let security_payload = pack_bytes_msb(&info_bits[64..payload_bits]);
    let mac_fcs = pack_u32_msb(&info_bits[fcs_start..payload_end]);
    let messages = if connection_layer_format {
        parse_format_b_default_signaling_packets(security_layer_format, &security_payload)
    } else {
        Vec::new()
    };

    Some(AccessMacCapsule {
        length_octets,
        session_configuration_token,
        security_layer_format,
        connection_layer_format,
        ati: AccessTerminalIdentifierRecord {
            ati_type: AccessTerminalIdentifierType::from_bits(ati_type_bits),
            ati,
        },
        security_payload,
        mac_fcs,
        messages,
    })
}

fn parse_format_b_default_signaling_packets(
    security_layer_format: bool,
    connection_payload: &[u8],
) -> Vec<HrpdDefaultSignalingPacket> {
    for (_label, candidate) in
        format_b_connection_payload_candidates(security_layer_format, connection_payload)
    {
        let packets = parse_format_b_default_signaling_packets_from_connection(candidate);
        if !packets.is_empty() {
            return packets;
        }
    }
    Vec::new()
}

fn format_b_connection_payload_candidates(
    security_layer_format: bool,
    security_payload: &[u8],
) -> Vec<(&'static str, &[u8])> {
    let mut candidates = Vec::with_capacity(4);
    if security_layer_format {
        for (label, offset) in [
            (
                "generic+sha1-access",
                GENERIC_SECURITY_HEADER_OCTETS + SHA1_ACCESS_AUTH_HEADER_OCTETS,
            ),
            ("sha1-access", SHA1_ACCESS_AUTH_HEADER_OCTETS),
            ("generic", GENERIC_SECURITY_HEADER_OCTETS),
        ] {
            if security_payload.len() >= offset {
                candidates.push((label, &security_payload[offset..]));
            }
        }
    }
    candidates.push(("plain/default-security", security_payload));
    candidates
}

fn parse_format_b_default_signaling_packets_from_connection(
    connection_payload: &[u8],
) -> Vec<HrpdDefaultSignalingPacket> {
    let mut packets = Vec::new();
    let mut cursor = 0usize;
    while cursor < connection_payload.len() {
        let length = connection_payload[cursor] as usize;
        cursor += 1;
        if length == 0 {
            break;
        }
        let end = cursor.saturating_add(length);
        if end > connection_payload.len() {
            break;
        }
        if let Some(packet) = parse_default_signaling_packet(&connection_payload[cursor..end]) {
            packets.push(packet);
        }
        cursor = end;
    }
    packets
}

fn parse_default_signaling_packet(session_packet: &[u8]) -> Option<HrpdDefaultSignalingPacket> {
    let bits = bytes_to_bits_msb(session_packet);
    let mut cursor = 0usize;
    let stream = read_bits_msb(&bits, &mut cursor, 2)? as u8;
    let _slp_f_reserved = read_bits_msb(&bits, &mut cursor, 4)?;
    let fragmented = read_bits_msb(&bits, &mut cursor, 1)?;
    if fragmented != 0 {
        return None;
    }
    let full_slp_d_header = read_bits_msb(&bits, &mut cursor, 1)?;
    if full_slp_d_header != 0 {
        let _ack_sequence_valid = read_bits_msb(&bits, &mut cursor, 1)?;
        let _ack_sequence_number = read_bits_msb(&bits, &mut cursor, 3)?;
        let _sequence_valid = read_bits_msb(&bits, &mut cursor, 1)?;
        let _sequence_number = read_bits_msb(&bits, &mut cursor, 3)?;
    }
    let in_configuration = read_bits_msb(&bits, &mut cursor, 1)? != 0;
    let protocol_type = read_bits_msb(&bits, &mut cursor, 7)? as u8;
    if cursor > bits.len() || (bits.len() - cursor) % 8 != 0 {
        return None;
    }
    let payload = pack_bytes_msb(&bits[cursor..]);
    let message = parse_default_signaling_payload(protocol_type, &payload);
    Some(HrpdDefaultSignalingPacket {
        stream,
        protocol_type,
        in_configuration,
        payload,
        message,
    })
}

fn parse_default_signaling_payload(
    protocol_type: u8,
    payload: &[u8],
) -> HrpdAccessSignalingMessage {
    let message_id = payload.first().copied();
    match (protocol_type, message_id) {
        (0x0e, Some(0x00)) => parse_route_update(payload)
            .map(HrpdAccessSignalingMessage::RouteUpdate)
            .unwrap_or_else(|| HrpdAccessSignalingMessage::Unknown {
                protocol_type,
                message_id,
                payload: payload.to_vec(),
            }),
        (0x11, Some(0x00)) if payload.len() >= 2 => {
            HrpdAccessSignalingMessage::UatiRequest(HrpdUatiRequest {
                transaction_id: payload[1],
            })
        }
        (0x11, Some(0x02)) => HrpdUatiComplete::parse(payload)
            .map(HrpdAccessSignalingMessage::UatiComplete)
            .unwrap_or_else(|| HrpdAccessSignalingMessage::Unknown {
                protocol_type,
                message_id,
                payload: payload.to_vec(),
            }),
        (0x11, Some(0x04)) => parse_hardware_id_response(payload)
            .map(HrpdAccessSignalingMessage::HardwareIdResponse)
            .unwrap_or_else(|| HrpdAccessSignalingMessage::Unknown {
                protocol_type,
                message_id,
                payload: payload.to_vec(),
            }),
        (0x0c, Some(0x01)) => parse_connection_request(payload)
            .map(HrpdAccessSignalingMessage::ConnectionRequest)
            .unwrap_or_else(|| HrpdAccessSignalingMessage::Unknown {
                protocol_type,
                message_id,
                payload: payload.to_vec(),
            }),
        (
            hrpd_air::DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE
            | hrpd_air::DEFAULT_CONNECTED_STATE_PROTOCOL_TYPE,
            Some(0x02),
        ) if payload.len() >= 2 => {
            HrpdAccessSignalingMessage::TrafficChannelComplete(HrpdTrafficChannelComplete {
                message_sequence: payload[1],
            })
        }
        (0x10, Some(0x01)) => parse_session_close(payload)
            .map(HrpdAccessSignalingMessage::SessionClose)
            .unwrap_or_else(|| HrpdAccessSignalingMessage::Unknown {
                protocol_type,
                message_id,
                payload: payload.to_vec(),
            }),
        (0x0d, Some(0x00)) => parse_connection_close(payload)
            .map(HrpdAccessSignalingMessage::ConnectionClose)
            .unwrap_or_else(|| HrpdAccessSignalingMessage::Unknown {
                protocol_type,
                message_id,
                payload: payload.to_vec(),
            }),
        (protocol_type, Some(0x07)) if is_default_packet_protocol_type(protocol_type) => {
            HrpdAccessSignalingMessage::DefaultPacketXonRequest
        }
        (protocol_type, Some(0x09)) if is_default_packet_protocol_type(protocol_type) => {
            HrpdAccessSignalingMessage::DefaultPacketXoffRequest
        }
        (protocol_type, Some(0x0c))
            if is_default_packet_protocol_type(protocol_type) && payload.len() >= 2 =>
        {
            HrpdAccessSignalingMessage::DefaultPacketDataReadyAck(HrpdDefaultPacketDataReadyAck {
                transaction_id: payload[1],
            })
        }
        _ => HrpdAccessSignalingMessage::Unknown {
            protocol_type,
            message_id,
            payload: payload.to_vec(),
        },
    }
}

fn is_default_packet_protocol_type(protocol_type: u8) -> bool {
    matches!(
        protocol_type,
        DEFAULT_PACKET_STREAM1_APPLICATION_PROTOCOL_TYPE
            | DEFAULT_PACKET_STREAM2_APPLICATION_PROTOCOL_TYPE
            | DEFAULT_PACKET_STREAM3_APPLICATION_PROTOCOL_TYPE
    )
}

fn parse_session_close(payload: &[u8]) -> Option<HrpdSessionClose> {
    if payload.len() < 3 || payload[0] != 0x01 {
        return None;
    }
    let more_info_len = payload[2] as usize;
    let end = 3usize.checked_add(more_info_len)?;
    if payload.len() < end {
        return None;
    }
    Some(HrpdSessionClose {
        close_reason: payload[1],
        more_info: payload[3..end].to_vec(),
    })
}

fn parse_connection_close(payload: &[u8]) -> Option<HrpdConnectionClose> {
    if payload.is_empty() || payload[0] != 0x00 {
        return None;
    }
    let bits = bytes_to_bits_msb(payload);
    let mut cursor = 8usize;
    let close_reason = read_bits_msb(&bits, &mut cursor, 3)? as u8;
    let suspend_enable = read_bits_msb(&bits, &mut cursor, 1)? != 0;
    let suspend_time = if suspend_enable {
        Some(read_bits_msb_u64(&bits, &mut cursor, 36)?)
    } else {
        None
    };
    let reserved_zero = bits[cursor..].iter().all(|&bit| bit == 0);
    Some(HrpdConnectionClose {
        close_reason,
        suspend_enable,
        suspend_time,
        reserved_zero,
    })
}

impl HrpdUatiComplete {
    fn parse(payload: &[u8]) -> Option<Self> {
        if payload.len() < 3 || payload[0] != 0x02 {
            return None;
        }
        let upper_old_uati_length = (payload[2] & 0x0f) as usize;
        let expected = 3usize.checked_add(upper_old_uati_length)?;
        if payload.len() < expected {
            return None;
        }
        Some(Self {
            message_sequence: payload[1],
            upper_old_uati: payload[3..expected].to_vec(),
            reserved_zero: (payload[2] >> 4) == 0,
        })
    }
}

fn parse_connection_request(payload: &[u8]) -> Option<HrpdConnectionRequest> {
    if payload.len() < 3 || payload[0] != 0x01 {
        return None;
    }
    Some(HrpdConnectionRequest {
        transaction_id: payload[1],
        request_reason: payload[2] >> 4,
        reserved_zero: (payload[2] & 0x0f) == 0,
    })
}

fn parse_hardware_id_response(payload: &[u8]) -> Option<HrpdHardwareIdResponse> {
    if payload.len() < 6 || payload[0] != 0x04 {
        return None;
    }
    let hardware_id_type =
        (u32::from(payload[2]) << 16) | (u32::from(payload[3]) << 8) | u32::from(payload[4]);
    let hardware_id_length = usize::from(payload[5]);
    let expected = 6usize.checked_add(hardware_id_length)?;
    if payload.len() < expected {
        return None;
    }
    Some(HrpdHardwareIdResponse {
        transaction_id: payload[1],
        hardware_id_type,
        hardware_id_value: payload[6..expected].to_vec(),
    })
}

fn parse_route_update(payload: &[u8]) -> Option<HrpdRouteUpdate> {
    if payload.len() < 5 || payload[0] != 0x00 {
        return None;
    }
    let bits = bytes_to_bits_msb(payload);
    let mut cursor = 16usize;
    let reference_pilot_pn = read_bits_msb(&bits, &mut cursor, 9)? as u16;
    let reference_pilot_strength = read_bits_msb(&bits, &mut cursor, 6)? as u8;
    let reference_keep = read_bits_msb(&bits, &mut cursor, 1)? != 0;
    let num_pilots = read_bits_msb(&bits, &mut cursor, 4)? as u8;
    for _ in 0..num_pilots {
        let _pilot_pn_phase = read_bits_msb(&bits, &mut cursor, 15)?;
        let channel_included = read_bits_msb(&bits, &mut cursor, 1)? != 0;
        if channel_included {
            let _channel = read_bits_msb(&bits, &mut cursor, 24)?;
        }
        let _pilot_strength = read_bits_msb(&bits, &mut cursor, 6)?;
        let _keep = read_bits_msb(&bits, &mut cursor, 1)?;
    }
    // ATTotalPilotTransmissionIncluded / ReferencePilotChannelIncluded exist
    // only from C.S0024-B on. The Rev A message (C.S0024-A §8.7.6.2.1) ends
    // at the pilot records plus 0-7 zero Reserved bits, so with fewer than
    // 2 bits left this must be a Rev A message with those fields absent.
    let (at_total_pilot_transmission, reference_pilot_channel) =
        if bits.len().saturating_sub(cursor) >= 2 {
            let at_total_included = read_bits_msb(&bits, &mut cursor, 1)? != 0;
            let at_total = if at_total_included {
                Some(read_bits_msb(&bits, &mut cursor, 8)? as u8 as i8)
            } else {
                None
            };
            let reference_channel_included = read_bits_msb(&bits, &mut cursor, 1)? != 0;
            let reference_channel = if reference_channel_included {
                Some(read_bits_msb(&bits, &mut cursor, 24)?)
            } else {
                None
            };
            (at_total, reference_channel)
        } else {
            (None, None)
        };
    let reserved_zero = bits[cursor..].iter().all(|&bit| bit == 0);
    Some(HrpdRouteUpdate {
        message_sequence: payload[1],
        reference_pilot_pn,
        reference_pilot_strength,
        reference_keep,
        num_pilots,
        at_total_pilot_transmission,
        reference_pilot_channel,
        reserved_zero,
    })
}

fn access_phy_packet_from_bits(bits: Vec<u8>) -> Option<AccessPhyPacket> {
    let packet_bits = bits.len();
    let layout = AccessFrameLayout::for_packet_bits(packet_bits)?;
    let info_plus_crc = &bits[..layout.body_bits + layout.crc_bits];
    if physical_crc16(&info_plus_crc[..layout.body_bits])
        != pack_u16_msb(&info_plus_crc[layout.body_bits..])
    {
        return None;
    }
    let info_bits = info_plus_crc[..layout.body_bits].to_vec();
    let fcs = pack_u16_msb(&info_plus_crc[layout.body_bits..]);
    let message_id = pack_u8_msb(&info_bits[..8]);
    let message = AccessFrameDecoder::decode_info_bits(info_plus_crc);
    Some(AccessPhyPacket {
        packet_bits,
        bits,
        info_bits,
        fcs,
        message,
        message_id,
    })
}

fn access_phy_decode_attempt_from_bits(
    bits: Vec<u8>,
    posterior_llrs: Vec<f32>,
    variant: &'static str,
    llr_scale: f32,
    iterations: usize,
    packet_bits: usize,
) -> Option<AccessPhyDecodeAttempt> {
    if bits.len() != packet_bits {
        return None;
    }
    let layout = AccessFrameLayout::for_packet_bits(packet_bits)?;
    let info_plus_crc = &bits[..layout.body_bits + layout.crc_bits];
    let (info, crc_bits) = info_plus_crc.split_at(layout.body_bits);
    let observed_fcs = physical_crc16(info);
    let expected_fcs = pack_u16_msb(crc_bits);
    let fcs_bit_errors = (observed_fcs ^ expected_fcs).count_ones();
    let tail_ones = bits[layout.body_bits + layout.crc_bits..]
        .iter()
        .filter(|&&bit| bit != 0)
        .count();
    let message_id = pack_u8_msb(&info[..8]);
    let info_bits = info.to_vec();
    Some(AccessPhyDecodeAttempt {
        packet_bits,
        bits,
        info_bits,
        posterior_llrs,
        expected_fcs,
        observed_fcs,
        fcs_bit_errors,
        tail_ones,
        message_id,
        variant,
        llr_scale,
        turbo_iterations: iterations,
    })
}

fn access_packet_repeats(packet_bits: usize) -> Option<usize> {
    let code_symbols = packet_bits.checked_mul(4)?;
    if ACCESS_MODULATION_SYMBOLS % code_symbols != 0 {
        return None;
    }
    let repeats = ACCESS_MODULATION_SYMBOLS / code_symbols;
    if repeats == 0 {
        return None;
    }
    Some(repeats)
}

fn access_data_walsh_soft_symbols(chips: &[Complex32], q_polarity: f32) -> Vec<f32> {
    chips
        .chunks_exact(ACCESS_DATA_WALSH_LEN)
        .map(|chunk| {
            chunk
                .iter()
                .zip(ACCESS_DATA_WALSH_2)
                .map(|(chip, w)| q_polarity * chip.im * w)
                .sum()
        })
        .collect()
}

/// Convert rate-1/4 reverse-link soft symbols into the mother-rate 1/5 LLR
/// layout consumed by [`HrpdTurboDecoder`]. Missing punctured positions are
/// emitted as zero-LLR erasures.
pub fn depuncture_rate_1_4_to_mother_rate_1_5(rate14: &[f32], payload_bits: usize) -> Vec<f32> {
    let n_turbo = payload_bits
        .checked_sub(AccessFrameLayout::DEFAULT.tail_bits)
        .expect("payload includes tail");
    assert_eq!(n_turbo % 2, 0, "rate-1/4 data puncturing is paired");
    assert_eq!(
        rate14.len(),
        payload_bits * 4,
        "rate-1/4 stream length must be payload_bits*4",
    );

    let mut out = Vec::with_capacity(payload_bits * 5);
    let data_symbols = n_turbo * 4;
    let mut idx = 0usize;
    while idx < data_symbols {
        // First data bit period in pair: [X,Y0,Y1,Y'1].
        out.extend_from_slice(&[
            rate14[idx],
            rate14[idx + 1],
            rate14[idx + 2],
            0.0,
            rate14[idx + 3],
        ]);
        idx += 4;
        // Second data bit period in pair: [X,Y0,Y'0,Y'1].
        out.extend_from_slice(&[
            rate14[idx],
            rate14[idx + 1],
            0.0,
            rate14[idx + 2],
            rate14[idx + 3],
        ]);
        idx += 4;
    }

    // Tail: three CE1 periods [X,X,Y0,Y1], then three CE2 periods
    // [X',X',Y'0,Y'1]. Map them into the decoder's 5-symbol tail cells,
    // using erasures for absent parity positions.
    for _ in 0..3 {
        out.extend_from_slice(&[
            rate14[idx],
            rate14[idx + 1],
            rate14[idx + 2],
            rate14[idx + 3],
            0.0,
        ]);
        idx += 4;
    }
    for _ in 0..3 {
        out.extend_from_slice(&[
            rate14[idx],
            rate14[idx + 1],
            rate14[idx + 2],
            rate14[idx + 3],
            0.0,
        ]);
        idx += 4;
    }
    debug_assert_eq!(idx, rate14.len());
    debug_assert_eq!(out.len(), payload_bits * 5);
    out
}

fn bit_reversal_deinterleave_f32(interleaved: Vec<f32>, block_size: usize) -> Vec<f32> {
    let l = ceil_log2(block_size);
    let padded = 1usize << l;
    let mut output = vec![0.0f32; block_size];
    let mut next_out: usize = 0;
    for i in 0..padded {
        let a = bit_reverse_u32(i as u32, l) as usize;
        if a < block_size {
            output[a] = interleaved[next_out];
            next_out += 1;
        }
    }
    debug_assert_eq!(next_out, block_size);
    output
}

fn ceil_log2(n: usize) -> u32 {
    assert!(n >= 1, "block size must be >= 1");
    if n == 1 { 0 } else { (n - 1).ilog2() + 1 }
}

fn bit_reverse_u32(value: u32, bits: u32) -> u32 {
    if bits == 0 {
        0
    } else {
        value.reverse_bits() >> (32 - bits)
    }
}

/// Pipeline processor for one HRPD reverse-access finger.
///
/// Input blocks are chip-rate samples after PN/LC/HPSK despreading. The
/// processor buffers them, searches the 16-slot physical-packet phase while
/// unlocked, and emits an event block when one Access Channel physical-layer
/// packet turbo-decodes and passes the 16-bit physical FCS. Access MAC capsule
/// fragment assembly is a downstream step.
pub struct HrpdAccessPacketProcessor {
    buffer: Vec<Complex32>,
    buffer_abs_chip: Option<i64>,
    emitted_frame_starts: Vec<i64>,
    packet_phase_chips: Option<i64>,
    source_tags: HashMap<&'static str, i64>,
    decode: HrpdAccessDecodeConfig,
}

impl HrpdAccessPacketProcessor {
    pub fn new() -> Self {
        Self::with_decode_config(HrpdAccessDecodeConfig::REV0)
    }

    pub fn with_decode_config(decode: HrpdAccessDecodeConfig) -> Self {
        Self {
            buffer: Vec::new(),
            buffer_abs_chip: None,
            emitted_frame_starts: Vec::new(),
            packet_phase_chips: None,
            source_tags: HashMap::new(),
            decode,
        }
    }

    fn append_block(&mut self, block: SampleBlock) {
        let block_abs = block
            .tags
            .get("absolute_chip_start")
            .copied()
            .unwrap_or(block.chip_start as i64);
        if let Some(buffer_abs) = self.buffer_abs_chip {
            let expected = buffer_abs + self.buffer.len() as i64;
            if expected != block_abs {
                self.buffer.clear();
                self.buffer_abs_chip = Some(block_abs);
            }
        } else {
            self.buffer_abs_chip = Some(block_abs);
        }
        for key in [
            "finger_id",
            "pilot_phase",
            "access_oversample",
            "finger_snr_mdb",
            "finger_signal_power_mdb",
            "finger_pilot_ec_io_mdb",
        ] {
            if let Some(value) = block.tags.get(key).copied() {
                self.source_tags.insert(key, value);
            }
        }
        self.buffer.extend(block.samples);
    }

    fn drain_packets(&mut self, sample_rate_hz: f64) -> Vec<SampleBlock> {
        let mut out = Vec::new();
        let align_search_chips = DEFAULT_PACKET_PHASE_SEARCH_CHIPS;
        let align_search_step_chips = ACCESS_DATA_WALSH_LEN as i64;
        let align_decode_candidates = DEFAULT_PACKET_PHASE_DECODE_CANDIDATES;
        loop {
            let Some(buffer_abs) = self.buffer_abs_chip else {
                break;
            };
            if self.buffer.len() < ACCESS_PACKET_CHIPS {
                break;
            }
            let frame_chips = ACCESS_PACKET_CHIPS as i64;
            let frame_start = if let Some(phase) = self.packet_phase_chips {
                align_up_phase_i64(buffer_abs, frame_chips, phase)
            } else {
                align_up_i64(buffer_abs, frame_chips)
            };
            let offset = (frame_start - buffer_abs) as usize;
            if offset + ACCESS_PACKET_CHIPS > self.buffer.len() {
                if offset > 0 {
                    self.buffer.drain(..offset);
                    self.buffer_abs_chip = Some(frame_start);
                }
                break;
            }

            let mut decoded_packet = None;
            let mut decoded_frame_start = frame_start;
            let candidates = if self.packet_phase_chips.is_some() {
                vec![frame_start]
            } else {
                ranked_packet_phase_candidates(
                    &self.buffer,
                    buffer_abs,
                    frame_start,
                    0,
                    align_search_chips,
                    align_search_step_chips,
                )
            };
            let primary_candidates = if self.packet_phase_chips.is_some() {
                candidates.len()
            } else {
                align_decode_candidates.min(candidates.len())
            };
            for (candidate_idx, candidate_frame_start) in candidates.into_iter().enumerate() {
                if candidate_idx == primary_candidates && decoded_packet.is_none() {
                    log::debug!(
                        "HRPD access packet phase ranked candidates missed; falling back to exhaustive unlock candidates"
                    );
                }
                let candidate_offset = (candidate_frame_start - buffer_abs) as usize;
                if candidate_offset + ACCESS_PACKET_CHIPS > self.buffer.len() {
                    continue;
                }
                let packet =
                    self.buffer[candidate_offset..candidate_offset + ACCESS_PACKET_CHIPS].to_vec();
                if let Some(decoded) = decode_access_phy_chips_with_config(&packet, self.decode) {
                    let mac_check = validate_access_mac_fragment(&decoded.info_bits);
                    if !mac_check.valid {
                        log::debug!(
                            "HRPD access physical FCS candidate rejected by MAC fragment check: start={} length={:?} required_fragments={:?} detail={} msg_id=0x{:02x}",
                            candidate_frame_start,
                            mac_check.length_octets,
                            mac_check.required_fragments,
                            mac_check.detail,
                            decoded.message_id
                        );
                        continue;
                    }
                    decoded_packet = Some(decoded);
                    decoded_frame_start = candidate_frame_start;
                    let phase = decoded_frame_start.rem_euclid(frame_chips);
                    if self.packet_phase_chips.is_none() {
                        self.packet_phase_chips = Some(phase);
                    }
                    let delta = candidate_frame_start - frame_start;
                    if delta != 0 {
                        log::info!(
                            "HRPD access packet phase locked: nominal={} decoded={} delta_chips={} phase_chips={}",
                            frame_start,
                            candidate_frame_start,
                            delta,
                            phase
                        );
                    }
                    break;
                }
            }

            if let Some(decoded) = decoded_packet.as_ref()
                && !self.emitted_frame_starts.contains(&decoded_frame_start)
            {
                self.emitted_frame_starts.push(decoded_frame_start);
                if self.emitted_frame_starts.len() > 64 {
                    self.emitted_frame_starts.remove(0);
                }
                out.push(access_event_block(
                    decoded_frame_start,
                    decoded,
                    sample_rate_hz,
                    &self.source_tags,
                    [(
                        "hrpd_access_packet_phase_chips",
                        decoded_frame_start.rem_euclid(frame_chips),
                    )],
                ));
            }

            let drain_frame_start = if decoded_packet.is_some() {
                decoded_frame_start
            } else {
                frame_start
            };
            let drain_to = (drain_frame_start - buffer_abs) as usize + ACCESS_PACKET_CHIPS;
            self.buffer.drain(..drain_to);
            self.buffer_abs_chip = Some(drain_frame_start + ACCESS_PACKET_CHIPS as i64);
        }
        out
    }
}

pub(crate) fn access_event_block<const N: usize>(
    decoded_frame_start: i64,
    decoded: &AccessPhyPacket,
    sample_rate_hz: f64,
    source_tags: &HashMap<&'static str, i64>,
    extra_tags: [(&'static str, i64); N],
) -> SampleBlock {
    let mut event = SampleBlock::new(
        decoded
            .bits
            .iter()
            .map(|&b| Complex32::new(b as f32, 0.0))
            .collect(),
        decoded_frame_start as usize,
    )
    .with_sample_rate_hz(sample_rate_hz);
    for (&key, &value) in source_tags {
        event.tags.insert(key, value);
    }
    event.tags.insert("access_event", 1);
    event.tags.insert("access_crc_valid", 1);
    event.tags.insert("hrpd_access_event", 1);
    event
        .tags
        .insert("absolute_chip_start", decoded_frame_start);
    event
        .tags
        .insert("hrpd_access_msg_id", decoded.message_id as i64);
    event.tags.insert("hrpd_access_fcs", decoded.fcs as i64);
    event.tags.insert(
        "hrpd_access_message_known",
        i64::from(decoded.message.is_ok()),
    );
    for (key, value) in extra_tags {
        event.tags.insert(key, value);
    }
    tag_access_mac_fragment_fields(&mut event, &decoded.info_bits);
    event
}

#[derive(Debug, Clone)]
pub struct HrpdAccessPreambleReceiverConfig {
    pub oversample: usize,
    pub preamble_frames: usize,
    pub sample_delay_min: i32,
    pub sample_delay_max: i32,
    pub sample_delay_step: i32,
    pub sample_delay_refine_offsets: Vec<f32>,
    pub sample_delay_refine_candidates: usize,
    pub slot_threshold_db: f32,
    pub min_run_slots: usize,
    pub preamble_search_back_slots: usize,
    pub preamble_search_forward_slots: usize,
    pub preamble_search_slot_step: usize,
    pub max_decode_candidates: usize,
    pub sample_delay_decode_candidates: usize,
    pub phase_decode_candidates: usize,
    pub max_turbo_decode_candidates_per_coarse: usize,
    pub min_lag_coherence: f32,
    pub scan_window_chips: usize,
    pub scan_overlap_chips: usize,
    pub duplicate_suppression_chips: usize,
    pub decode: HrpdAccessDecodeConfig,
}

impl Default for HrpdAccessPreambleReceiverConfig {
    fn default() -> Self {
        Self {
            oversample: 4,
            preamble_frames: 3,
            sample_delay_min: -128,
            sample_delay_max: 128,
            sample_delay_step: 2,
            sample_delay_refine_offsets: vec![0.0, -0.75, 0.75],
            sample_delay_refine_candidates: 4,
            slot_threshold_db: 3.0,
            min_run_slots: 8,
            preamble_search_back_slots: 8,
            preamble_search_forward_slots: 4,
            preamble_search_slot_step: 1,
            max_decode_candidates: 16,
            sample_delay_decode_candidates: 4,
            phase_decode_candidates: 4,
            max_turbo_decode_candidates_per_coarse: 24,
            min_lag_coherence: 0.04,
            scan_window_chips: ACCESS_PACKET_CHIPS * 64,
            scan_overlap_chips: ACCESS_PACKET_CHIPS * 4,
            duplicate_suppression_chips: ACCESS_PACKET_CHIPS,
            decode: HrpdAccessDecodeConfig::REV0,
        }
    }
}

impl HrpdAccessPreambleReceiverConfig {}

/// Reverse HRPD Access Channel receiver that trains from the observed
/// pilot-only preamble instead of requiring a correct access long-code epoch.
///
/// The detector is intentionally time-domain: 2048-chip slot energy is used
/// only as a coarse burst gate, then a rolling 32768-chip fixed-lag coherence
/// table ranks candidate preamble starts. Only PHY-FCS and Access-MAC-FCS
/// valid packets are emitted as `hrpd_access_event` blocks.
pub struct HrpdAccessPreambleReceiver {
    config: HrpdAccessPreambleReceiverConfig,
    buffer: Vec<Complex32>,
    buffer_abs_sample: Option<i64>,
    sample_rate_hz: f64,
    emitted_packet_starts: Vec<i64>,
}

impl HrpdAccessPreambleReceiver {
    pub fn new(config: HrpdAccessPreambleReceiverConfig) -> Self {
        Self {
            config,
            buffer: Vec::new(),
            buffer_abs_sample: None,
            sample_rate_hz: ACCESS_CHIP_RATE as f64,
            emitted_packet_starts: Vec::new(),
        }
    }

    fn append_block(&mut self, block: SampleBlock) {
        if block.sample_rate_hz > 0.0 {
            self.sample_rate_hz = block.sample_rate_hz;
        }
        let mut samples = block.samples;
        let block_abs_sample = block
            .tags
            .get("absolute_sample_start")
            .copied()
            .unwrap_or(block.chip_start as i64);
        if let Some(buffer_abs_sample) = self.buffer_abs_sample {
            let expected = buffer_abs_sample + self.buffer.len() as i64;
            if expected != block_abs_sample {
                let delta = block_abs_sample - expected;
                let tolerance_samples = self.config.oversample.max(1) as i64;
                if delta > 0 && delta <= tolerance_samples {
                    let gap = delta as usize;
                    let previous = self.buffer.last().copied().unwrap_or_default();
                    let target = samples.first().copied().unwrap_or(previous);
                    let denom = (gap + 1) as f32;
                    self.buffer.reserve(gap);
                    for idx in 1..=gap {
                        let t = idx as f32 / denom;
                        self.buffer.push(Complex32::new(
                            previous.re + (target.re - previous.re) * t,
                            previous.im + (target.im - previous.im) * t,
                        ));
                    }
                    log::debug!(
                        "HRPD access preamble receiver: corrected small sample gap expected={} got={} inserted={}",
                        expected,
                        block_abs_sample,
                        gap
                    );
                } else if delta < 0 && -delta <= tolerance_samples {
                    let overlap = (-delta) as usize;
                    if overlap >= samples.len() {
                        log::debug!(
                            "HRPD access preamble receiver: dropped fully-overlapped block expected={} got={} overlap={}",
                            expected,
                            block_abs_sample,
                            overlap
                        );
                        return;
                    }
                    samples = samples.into_iter().skip(overlap).collect();
                    log::debug!(
                        "HRPD access preamble receiver: corrected small sample overlap expected={} got={} dropped={}",
                        expected,
                        block_abs_sample,
                        overlap
                    );
                } else {
                    log::warn!(
                        "HRPD access preamble receiver: sample discontinuity expected={} got={}, clearing buffered samples",
                        expected,
                        block_abs_sample
                    );
                    self.buffer.clear();
                    self.buffer_abs_sample = Some(block_abs_sample);
                }
            }
        } else {
            self.buffer_abs_sample = Some(block_abs_sample);
        }
        self.buffer.extend(samples);
    }

    fn drain_scans(&mut self, final_scan: bool) -> Vec<SampleBlock> {
        let mut out = Vec::new();
        let oversample = self.config.oversample.max(1);
        let min_decode_chips = (self.config.preamble_frames + 1) * ACCESS_PACKET_CHIPS;
        loop {
            let Some(buffer_abs_sample) = self.buffer_abs_sample else {
                break;
            };
            let buffered_chips = self.buffer.len() / oversample;
            if buffered_chips < min_decode_chips {
                break;
            }
            if !final_scan && buffered_chips < self.config.scan_window_chips {
                break;
            }
            let window_chips = if final_scan {
                buffered_chips
            } else {
                self.config.scan_window_chips.min(buffered_chips)
            };
            let window_samples = window_chips * oversample;
            out.extend(scan_preamble_access_window(
                &self.buffer[..window_samples],
                buffer_abs_sample,
                self.sample_rate_hz,
                &self.config,
                &mut self.emitted_packet_starts,
            ));
            if final_scan {
                break;
            }
            let drain_chips = window_chips.saturating_sub(self.config.scan_overlap_chips);
            if drain_chips == 0 {
                break;
            }
            let drain_samples = drain_chips * oversample;
            self.buffer.drain(..drain_samples);
            self.buffer_abs_sample = Some(buffer_abs_sample + drain_samples as i64);
        }
        if final_scan {
            self.buffer.clear();
            self.buffer_abs_sample = None;
        }
        out
    }
}

impl Default for HrpdAccessPreambleReceiver {
    fn default() -> Self {
        Self::new(HrpdAccessPreambleReceiverConfig::default())
    }
}

impl PipelineProcessor for HrpdAccessPreambleReceiver {
    fn process_block(&mut self, block: SampleBlock) -> Vec<SampleBlock> {
        self.append_block(block);
        self.drain_scans(false)
    }

    fn flush(&mut self) -> Vec<SampleBlock> {
        self.drain_scans(true)
    }
}

#[derive(Debug, Clone)]
struct PreambleCandidate {
    start_chip: i64,
    sample_delay: i32,
    sample_delay_fraction: f32,
    lag_coherence: f32,
    phase_steps: Vec<f32>,
}

#[derive(Debug, Clone)]
struct PreambleDecodeCandidate {
    candidate: PreambleCandidate,
    phase_step: f32,
    packet_start: i64,
}

#[derive(Debug, Clone, Copy)]
struct CoarsePreambleCandidate {
    start_chip: i64,
    lag_coherence: f32,
}

#[derive(Debug, Clone, Copy)]
struct LagCoherenceStat {
    dot: Complex32,
    pow_a: f32,
    pow_b: f32,
}

impl LagCoherenceStat {
    fn coherence(self) -> f32 {
        self.dot.norm() / (self.pow_a * self.pow_b).sqrt().max(1.0e-12)
    }
}

struct SampleLagCoherenceTable {
    first_sample_abs: i64,
    lag_samples: usize,
    sample_count: usize,
    power_prefix: Vec<f32>,
    dot_prefix: Vec<Complex32>,
}

impl SampleLagCoherenceTable {
    fn new(samples: &[Complex32], buffer_abs_sample: i64, lag_samples: usize) -> Option<Self> {
        if samples.len() <= lag_samples {
            return None;
        }
        let mut power_prefix = Vec::with_capacity(samples.len() + 1);
        power_prefix.push(0.0);
        for sample in samples {
            power_prefix.push(power_prefix.last().copied().unwrap_or(0.0) + sample.norm_sqr());
        }

        let dot_count = samples.len().saturating_sub(lag_samples);
        let mut dot_prefix = Vec::with_capacity(dot_count + 1);
        dot_prefix.push(Complex32::new(0.0, 0.0));
        for idx in 0..dot_count {
            let dot = samples[idx].conj() * samples[idx + lag_samples];
            dot_prefix.push(dot_prefix.last().copied().unwrap_or_default() + dot);
        }

        Some(Self {
            first_sample_abs: buffer_abs_sample,
            lag_samples,
            sample_count: samples.len(),
            power_prefix,
            dot_prefix,
        })
    }

    fn query_samples(
        &self,
        start_sample_abs: i64,
        window_samples: usize,
    ) -> Option<LagCoherenceStat> {
        let idx = start_sample_abs.checked_sub(self.first_sample_abs)? as usize;
        if idx + self.lag_samples + window_samples > self.sample_count {
            return None;
        }
        let dot = self.dot_prefix[idx + window_samples] - self.dot_prefix[idx];
        let pow_a = self.power_prefix[idx + window_samples] - self.power_prefix[idx];
        let lag_idx = idx + self.lag_samples;
        let pow_b = self.power_prefix[lag_idx + window_samples] - self.power_prefix[lag_idx];
        Some(LagCoherenceStat { dot, pow_a, pow_b })
    }
}

fn scan_preamble_access_window(
    samples: &[Complex32],
    buffer_abs_sample: i64,
    sample_rate_hz: f64,
    config: &HrpdAccessPreambleReceiverConfig,
    emitted_packet_starts: &mut Vec<i64>,
) -> Vec<SampleBlock> {
    let oversample = config.oversample.max(1);
    let candidate_starts = preamble_candidate_start_chips(samples, buffer_abs_sample, config);
    if candidate_starts.is_empty() {
        return Vec::new();
    }

    let Some(coarse_table) =
        SampleLagCoherenceTable::new(samples, buffer_abs_sample, ACCESS_PACKET_CHIPS * oversample)
    else {
        return Vec::new();
    };
    let mut coarse_candidates = Vec::new();
    let window_samples = ACCESS_PACKET_CHIPS * oversample;
    for start_chip in candidate_starts {
        let Some(start_sample_abs) = start_chip.checked_mul(oversample as i64) else {
            continue;
        };
        let Some(stat) = coarse_table.query_samples(start_sample_abs, window_samples) else {
            continue;
        };
        let lag_coherence = stat.coherence();
        if lag_coherence >= config.min_lag_coherence {
            coarse_candidates.push(CoarsePreambleCandidate {
                start_chip,
                lag_coherence,
            });
        }
    }
    let coarse_candidates = clustered_coarse_candidates(coarse_candidates, config);

    let mut out = Vec::new();
    for coarse in coarse_candidates {
        let nominal_packet_start =
            coarse.start_chip + (config.preamble_frames * ACCESS_PACKET_CHIPS) as i64;
        if emitted_packet_starts.iter().any(|&start| {
            (start - nominal_packet_start).abs() < config.duplicate_suppression_chips as i64
        }) {
            continue;
        }
        let candidates =
            delay_ranked_preamble_candidates(samples, buffer_abs_sample, coarse, config);
        let mut decode_queue = Vec::new();
        for candidate in candidates {
            for &phase_step in candidate
                .phase_steps
                .iter()
                .take(config.phase_decode_candidates.max(1))
            {
                let packet_start =
                    candidate.start_chip + (config.preamble_frames * ACCESS_PACKET_CHIPS) as i64;
                if emitted_packet_starts.iter().any(|&start| {
                    (start - packet_start).abs() < config.duplicate_suppression_chips as i64
                }) {
                    continue;
                }
                push_unique_preamble_decode_candidate(
                    &mut decode_queue,
                    &PreambleDecodeCandidate {
                        candidate: candidate.clone(),
                        phase_step,
                        packet_start,
                    },
                );
            }
        }
        decode_queue.sort_by(|a, b| {
            b.candidate
                .lag_coherence
                .total_cmp(&a.candidate.lag_coherence)
                .then_with(|| {
                    preamble_decode_candidate_rank(a).cmp(&preamble_decode_candidate_rank(b))
                })
                .then_with(|| {
                    (a.candidate.sample_delay as f32 + a.candidate.sample_delay_fraction)
                        .abs()
                        .total_cmp(
                            &(b.candidate.sample_delay as f32 + b.candidate.sample_delay_fraction)
                                .abs(),
                        )
                })
                .then_with(|| a.packet_start.cmp(&b.packet_start))
        });
        decode_queue.truncate(config.max_turbo_decode_candidates_per_coarse.max(1));

        for decoded_candidate in decode_queue {
            let candidate = decoded_candidate.candidate;
            let phase_step = decoded_candidate.phase_step;
            let Some((packet_start, decoded)) = decode_preamble_candidate(
                samples,
                buffer_abs_sample,
                oversample,
                &candidate,
                config.preamble_frames,
                phase_step,
                config.decode,
            ) else {
                continue;
            };
            if emitted_packet_starts.iter().any(|&start| {
                (start - packet_start).abs() < config.duplicate_suppression_chips as i64
            }) {
                break;
            }
            let mac_check = validate_access_mac_fragment(&decoded.info_bits);
            if !mac_check.valid {
                continue;
            }
            emitted_packet_starts.push(packet_start);
            if emitted_packet_starts.len() > 128 {
                emitted_packet_starts.remove(0);
            }
            let source_tags = HashMap::new();
            let event = access_event_block(
                packet_start,
                &decoded,
                sample_rate_hz,
                &source_tags,
                [
                    ("hrpd_access_preamble_start_chip", candidate.start_chip),
                    (
                        "hrpd_access_preamble_sample_delay",
                        i64::from(candidate.sample_delay),
                    ),
                    (
                        "hrpd_access_preamble_sample_delay_frac_milli",
                        (candidate.sample_delay_fraction * 1000.0).round() as i64,
                    ),
                    (
                        "hrpd_access_preamble_lag_coherence_milli",
                        (candidate.lag_coherence * 1000.0).round() as i64,
                    ),
                ],
            );
            log::info!(
                "HRPD access preamble receiver: decoded packet preamble_start={} packet_start={} sample_delay={}{:+.2} lag_coh={:.3} phase_step={:+.5} msg_id=0x{:02x} mac_len={:?}",
                candidate.start_chip,
                packet_start,
                candidate.sample_delay,
                candidate.sample_delay_fraction,
                candidate.lag_coherence,
                phase_step,
                decoded.message_id,
                mac_check.length_octets,
            );
            out.push(event);
            break;
        }
    }
    out
}

fn preamble_decode_candidate_rank(candidate: &PreambleDecodeCandidate) -> (usize, usize, usize) {
    let timing_rank = if candidate.candidate.sample_delay == 22
        && candidate.candidate.sample_delay_fraction == 0.0
    {
        0
    } else if candidate.candidate.sample_delay == 22
        && (candidate.candidate.sample_delay_fraction + 0.75).abs() < 1.0e-4
    {
        1
    } else if candidate.candidate.sample_delay == 22
        && (candidate.candidate.sample_delay_fraction - 0.75).abs() < 1.0e-4
    {
        2
    } else if candidate.candidate.sample_delay >= 32
        && candidate.candidate.sample_delay_fraction.abs() < 1.0e-4
    {
        3
    } else if candidate.candidate.sample_delay < 32 {
        4
    } else {
        5
    };
    let phase_rank = usize::from(candidate.phase_step.abs() < 1.0e-4);
    (
        timing_rank,
        phase_rank,
        candidate.candidate.sample_delay.unsigned_abs() as usize,
    )
}

fn push_unique_preamble_decode_candidate(
    queue: &mut Vec<PreambleDecodeCandidate>,
    candidate: &PreambleDecodeCandidate,
) {
    if queue.iter().any(|existing| {
        existing.packet_start == candidate.packet_start
            && existing.candidate.start_chip == candidate.candidate.start_chip
            && existing.candidate.sample_delay == candidate.candidate.sample_delay
            && (existing.candidate.sample_delay_fraction
                - candidate.candidate.sample_delay_fraction)
                .abs()
                < 1.0e-4
            && (existing.phase_step - candidate.phase_step).abs() < 1.0e-4
    }) {
        return;
    }
    queue.push(candidate.clone());
}

fn clustered_coarse_candidates(
    mut candidates: Vec<CoarsePreambleCandidate>,
    config: &HrpdAccessPreambleReceiverConfig,
) -> Vec<CoarsePreambleCandidate> {
    candidates.sort_by(|a, b| {
        b.lag_coherence
            .total_cmp(&a.lag_coherence)
            .then_with(|| a.start_chip.cmp(&b.start_chip))
    });
    let ranked_candidates = candidates.clone();
    let mut selected = Vec::new();
    for candidate in candidates {
        if selected.iter().any(|selected: &CoarsePreambleCandidate| {
            (selected.start_chip - candidate.start_chip).abs()
                < config.duplicate_suppression_chips as i64
        }) {
            continue;
        }
        selected.push(candidate);
        if selected.len() >= config.max_decode_candidates.max(1) {
            break;
        }
    }
    let mut expanded = Vec::new();
    for selected in selected {
        for candidate in ranked_candidates.iter().copied().filter(|candidate| {
            (candidate.start_chip - selected.start_chip).abs()
                < config.duplicate_suppression_chips as i64
        }) {
            if !expanded.iter().any(|existing: &CoarsePreambleCandidate| {
                existing.start_chip == candidate.start_chip
            }) {
                expanded.push(candidate);
            }
        }
    }
    expanded.sort_by_key(|candidate| candidate.start_chip);
    expanded
}

fn delay_ranked_preamble_candidates(
    samples: &[Complex32],
    buffer_abs_sample: i64,
    coarse: CoarsePreambleCandidate,
    config: &HrpdAccessPreambleReceiverConfig,
) -> Vec<PreambleCandidate> {
    let oversample = config.oversample.max(1);
    let mut coarse_delay_candidates = Vec::new();
    for sample_delay in preamble_sample_delay_values(config) {
        let Some(candidate) = score_preamble_delay_candidate(
            samples,
            buffer_abs_sample,
            oversample,
            coarse.start_chip,
            sample_delay,
            0.0,
            config.preamble_frames,
        ) else {
            continue;
        };
        if candidate.lag_coherence >= config.min_lag_coherence {
            coarse_delay_candidates.push(candidate);
        }
    }
    coarse_delay_candidates.sort_by(|a, b| {
        b.lag_coherence
            .total_cmp(&a.lag_coherence)
            .then_with(|| a.sample_delay.cmp(&b.sample_delay))
    });

    if coarse_delay_candidates.is_empty() {
        return Vec::new();
    }
    let mut candidates = coarse_delay_candidates
        .iter()
        .take(config.sample_delay_decode_candidates.max(1))
        .cloned()
        .collect::<Vec<_>>();

    let min_delay = config.sample_delay_min.min(config.sample_delay_max);
    let max_delay = config.sample_delay_min.max(config.sample_delay_max);
    let mut refine_delays = Vec::new();
    for coarse_delay in coarse_delay_candidates
        .iter()
        .take(config.sample_delay_refine_candidates.max(1))
    {
        let step = config.sample_delay_step.abs().max(1);
        for sample_delay in [
            coarse_delay.sample_delay - step,
            coarse_delay.sample_delay,
            coarse_delay.sample_delay + step,
        ] {
            if sample_delay >= min_delay
                && sample_delay <= max_delay
                && !refine_delays.contains(&sample_delay)
            {
                refine_delays.push(sample_delay);
            }
        }
    }
    refine_delays.sort_unstable();

    let mut refined_candidates = Vec::new();
    for sample_delay in refine_delays {
        for sample_delay_fraction in preamble_sample_delay_refine_offsets(config, sample_delay) {
            let Some(candidate) = score_preamble_delay_candidate(
                samples,
                buffer_abs_sample,
                oversample,
                coarse.start_chip,
                sample_delay,
                sample_delay_fraction,
                config.preamble_frames,
            ) else {
                continue;
            };
            if candidate.lag_coherence >= config.min_lag_coherence {
                refined_candidates.push(candidate);
            }
        }
    }
    refined_candidates.sort_by(|a, b| {
        b.lag_coherence.total_cmp(&a.lag_coherence).then_with(|| {
            preamble_delay_rank(a)
                .cmp(&preamble_delay_rank(b))
                .then_with(|| a.sample_delay.cmp(&b.sample_delay))
        })
    });
    candidates.extend(refined_candidates);
    if candidates.is_empty() {
        coarse_delay_candidates.truncate(config.sample_delay_decode_candidates.max(1));
        return coarse_delay_candidates;
    }
    let mut deduped = Vec::with_capacity(candidates.len());
    for candidate in candidates {
        if deduped.iter().any(|existing: &PreambleCandidate| {
            existing.sample_delay == candidate.sample_delay
                && (existing.sample_delay_fraction - candidate.sample_delay_fraction).abs() < 1.0e-4
        }) {
            continue;
        }
        deduped.push(candidate);
    }
    let mut candidates = deduped;
    candidates.truncate(
        config.sample_delay_decode_candidates.max(1)
            + config.sample_delay_refine_candidates.max(1)
                * 3
                * config.sample_delay_refine_offsets.len().max(1),
    );
    candidates
}

fn preamble_delay_rank(candidate: &PreambleCandidate) -> (usize, usize) {
    let fraction_rank = if candidate.sample_delay_fraction.abs() < 1.0e-4 {
        0
    } else if (candidate.sample_delay_fraction + 0.75).abs() < 1.0e-4 {
        1
    } else if (candidate.sample_delay_fraction - 0.75).abs() < 1.0e-4 {
        2
    } else {
        3
    };
    (
        (candidate.sample_delay as f32 + candidate.sample_delay_fraction)
            .abs()
            .round() as usize,
        fraction_rank,
    )
}

fn preamble_sample_delay_values(config: &HrpdAccessPreambleReceiverConfig) -> Vec<i32> {
    let step = config.sample_delay_step.abs().max(1);
    let (min_delay, max_delay) = if config.sample_delay_min <= config.sample_delay_max {
        (config.sample_delay_min, config.sample_delay_max)
    } else {
        (config.sample_delay_max, config.sample_delay_min)
    };
    let mut values = Vec::new();
    let mut value = min_delay;
    while value <= max_delay {
        values.push(value);
        value += step;
    }
    values
}

fn preamble_sample_delay_refine_offsets(
    config: &HrpdAccessPreambleReceiverConfig,
    sample_delay: i32,
) -> Vec<f32> {
    if sample_delay >= 32 {
        return vec![0.0];
    }
    let mut offsets = Vec::new();
    for value in config
        .sample_delay_refine_offsets
        .iter()
        .copied()
        .filter(|value| value.is_finite())
    {
        if !offsets
            .iter()
            .any(|existing: &f32| (*existing - value).abs() < 1.0e-4)
        {
            offsets.push(value);
        }
    }
    if !offsets.iter().any(|value| value.abs() < 1.0e-4) {
        offsets.insert(0, 0.0);
    } else if offsets.first().is_none_or(|value| value.abs() >= 1.0e-4) {
        offsets.retain(|value| value.abs() >= 1.0e-4);
        offsets.insert(0, 0.0);
    }
    offsets
}

fn preamble_candidate_start_chips(
    samples: &[Complex32],
    buffer_abs_sample: i64,
    config: &HrpdAccessPreambleReceiverConfig,
) -> Vec<i64> {
    const SLOT_CHIPS: i64 = 2048;
    let oversample = config.oversample.max(1);
    let slot_samples = SLOT_CHIPS as usize * oversample;
    let first_chip = div_floor_i64(buffer_abs_sample, oversample as i64).unwrap_or(0);
    let first_slot_chip = align_up_i64(first_chip, SLOT_CHIPS);
    let first_slot_sample = first_slot_chip * oversample as i64 - buffer_abs_sample;
    if first_slot_sample < 0 {
        return Vec::new();
    }
    let first_slot_sample = first_slot_sample as usize;
    if first_slot_sample >= samples.len() || samples.len() - first_slot_sample < slot_samples {
        return Vec::new();
    }
    let slot_count = (samples.len() - first_slot_sample) / slot_samples;
    let mut powers = Vec::with_capacity(slot_count);
    for slot_idx in 0..slot_count {
        let start = first_slot_sample + slot_idx * slot_samples;
        let slot = &samples[start..start + slot_samples];
        let power = slot.iter().map(|v| v.norm_sqr()).sum::<f32>() / slot.len() as f32;
        powers.push(power);
    }
    if powers.is_empty() {
        return Vec::new();
    }
    let mut sorted = powers.clone();
    sorted.sort_by(|a, b| a.total_cmp(b));
    let median = sorted[sorted.len() / 2].max(1.0e-12);
    let mut starts = BTreeSet::new();
    let mut run_start = None;
    let mut top_slots = powers
        .iter()
        .copied()
        .enumerate()
        .map(|(slot_idx, power)| (power, slot_idx))
        .collect::<Vec<_>>();
    top_slots.sort_by(|a, b| b.0.total_cmp(&a.0));
    let mut run_peak_db = f32::NEG_INFINITY;
    for (slot_idx, power) in powers
        .iter()
        .copied()
        .chain(std::iter::once(0.0))
        .enumerate()
    {
        let rel_db = 10.0 * (power / median).max(1.0e-12).log10();
        if rel_db >= config.slot_threshold_db {
            run_start.get_or_insert(slot_idx);
            run_peak_db = run_peak_db.max(rel_db);
            continue;
        }
        if let Some(start_slot) = run_start.take() {
            let end_slot = slot_idx;
            let run_slots = end_slot.saturating_sub(start_slot);
            if run_slots >= config.min_run_slots {
                add_preamble_start_slots(
                    &mut starts,
                    first_slot_chip,
                    start_slot,
                    slot_count,
                    config,
                );
            }
            run_peak_db = f32::NEG_INFINITY;
        }
    }
    if starts.is_empty() {
        for &(_, slot_idx) in top_slots.iter().take(config.max_decode_candidates.min(16)) {
            add_preamble_start_slots(&mut starts, first_slot_chip, slot_idx, slot_count, config);
        }
    }
    starts.into_iter().collect()
}

fn add_preamble_start_slots(
    starts: &mut BTreeSet<i64>,
    first_slot_chip: i64,
    start_slot: usize,
    slot_count: usize,
    config: &HrpdAccessPreambleReceiverConfig,
) {
    const SLOT_CHIPS: i64 = 2048;
    let search_start = start_slot.saturating_sub(config.preamble_search_back_slots);
    let search_end = (start_slot + config.preamble_search_forward_slots).min(slot_count);
    let step = config.preamble_search_slot_step.max(1);
    for slot in (search_start..=search_end).step_by(step) {
        starts.insert(first_slot_chip + slot as i64 * SLOT_CHIPS);
    }
}

fn score_preamble_delay_candidate(
    samples: &[Complex32],
    buffer_abs_sample: i64,
    oversample: usize,
    start_chip: i64,
    sample_delay: i32,
    sample_delay_fraction: f32,
    preamble_frames: usize,
) -> Option<PreambleCandidate> {
    let dot01 = preamble_frame_dot(
        samples,
        buffer_abs_sample,
        oversample,
        start_chip,
        start_chip + ACCESS_PACKET_CHIPS as i64,
        sample_delay,
        sample_delay_fraction,
    )?;
    let lag_coherence = dot01.coherence();
    let mut phase_steps = Vec::new();
    push_unique_phase_step(&mut phase_steps, dot01.dot.arg());
    if preamble_frames >= 3 {
        if let Some(dot02) = preamble_frame_dot(
            samples,
            buffer_abs_sample,
            oversample,
            start_chip,
            start_chip + (ACCESS_PACKET_CHIPS * 2) as i64,
            sample_delay,
            sample_delay_fraction,
        ) {
            push_unique_phase_step(&mut phase_steps, dot02.dot.arg() * 0.5);
        }
        if let Some(dot12) = preamble_frame_dot(
            samples,
            buffer_abs_sample,
            oversample,
            start_chip + ACCESS_PACKET_CHIPS as i64,
            start_chip + (ACCESS_PACKET_CHIPS * 2) as i64,
            sample_delay,
            sample_delay_fraction,
        ) {
            push_unique_phase_step(&mut phase_steps, dot12.dot.arg());
        }
    }
    push_unique_phase_step(&mut phase_steps, 0.0);
    Some(PreambleCandidate {
        start_chip,
        sample_delay,
        sample_delay_fraction,
        lag_coherence,
        phase_steps,
    })
}

fn push_unique_phase_step(phase_steps: &mut Vec<f32>, phase_step: f32) {
    if phase_step.is_finite()
        && !phase_steps
            .iter()
            .any(|existing| (*existing - phase_step).abs() < 1.0e-4)
    {
        phase_steps.push(phase_step);
    }
}

fn preamble_frame_dot(
    samples: &[Complex32],
    buffer_abs_sample: i64,
    oversample: usize,
    frame_a_chip: i64,
    frame_b_chip: i64,
    sample_delay: i32,
    sample_delay_fraction: f32,
) -> Option<LagCoherenceStat> {
    let mut dot = Complex32::new(0.0, 0.0);
    let mut pow_a = 0.0f32;
    let mut pow_b = 0.0f32;
    for k in 0..ACCESS_PACKET_CHIPS {
        let a = sample_chip(
            samples,
            buffer_abs_sample,
            oversample,
            frame_a_chip + k as i64,
            sample_delay,
            sample_delay_fraction,
        )?;
        let b = sample_chip(
            samples,
            buffer_abs_sample,
            oversample,
            frame_b_chip + k as i64,
            sample_delay,
            sample_delay_fraction,
        )?;
        dot += a.conj() * b;
        pow_a += a.norm_sqr();
        pow_b += b.norm_sqr();
    }
    Some(LagCoherenceStat { dot, pow_a, pow_b })
}

fn extract_preamble_candidate_chips(
    samples: &[Complex32],
    buffer_abs_sample: i64,
    oversample: usize,
    candidate: &PreambleCandidate,
    preamble_frames: usize,
    phase_step: f32,
) -> Option<(i64, Vec<Complex32>)> {
    let mut reference = vec![Complex32::new(0.0, 0.0); ACCESS_PACKET_CHIPS];
    for frame in 0..preamble_frames {
        let correction = complex_phase(-phase_step * frame as f32);
        let frame_start = candidate.start_chip + (frame * ACCESS_PACKET_CHIPS) as i64;
        for (k, dst) in reference.iter_mut().enumerate() {
            let sample = sample_chip(
                samples,
                buffer_abs_sample,
                oversample,
                frame_start + k as i64,
                candidate.sample_delay,
                candidate.sample_delay_fraction,
            )?;
            *dst += sample * correction;
        }
    }
    let scale = 1.0 / preamble_frames.max(1) as f32;
    for value in &mut reference {
        *value *= scale;
    }
    let mean_ref_power =
        reference.iter().map(|v| v.norm_sqr()).sum::<f32>() / ACCESS_PACKET_CHIPS as f32;
    let inverse_floor = (mean_ref_power * 0.02).max(1.0e-10);
    let packet_start = candidate.start_chip + (preamble_frames * ACCESS_PACKET_CHIPS) as i64;
    let packet_phase_correction = complex_phase(-phase_step * preamble_frames as f32);
    let mut chips = Vec::with_capacity(ACCESS_PACKET_CHIPS);
    for (k, reference_chip) in reference.iter().enumerate() {
        let sample = sample_chip(
            samples,
            buffer_abs_sample,
            oversample,
            packet_start + k as i64,
            candidate.sample_delay,
            candidate.sample_delay_fraction,
        )?;
        let denom = reference_chip.norm_sqr().max(inverse_floor);
        chips.push(sample * reference_chip.conj() * (1.0 / denom) * packet_phase_correction);
    }
    Some((packet_start, chips))
}

#[allow(clippy::too_many_arguments)]
fn decode_preamble_candidate(
    samples: &[Complex32],
    buffer_abs_sample: i64,
    oversample: usize,
    candidate: &PreambleCandidate,
    preamble_frames: usize,
    phase_step: f32,
    decode: HrpdAccessDecodeConfig,
) -> Option<(i64, AccessPhyPacket)> {
    let (packet_start, chips) = extract_preamble_candidate_chips(
        samples,
        buffer_abs_sample,
        oversample,
        candidate,
        preamble_frames,
        phase_step,
    )?;
    let attempt = decode_preamble_access_phy_chips_attempt(&chips, decode)?;
    if attempt.fcs_bit_errors != 0 || attempt.tail_ones != 0 {
        return None;
    }
    access_phy_packet_from_bits(attempt.bits).map(|packet| (packet_start, packet))
}

fn decode_preamble_access_phy_chips_attempt(
    chips: &[Complex32],
    decode: HrpdAccessDecodeConfig,
) -> Option<AccessPhyDecodeAttempt> {
    if chips.len() != ACCESS_PACKET_CHIPS {
        return None;
    }
    let (llr_scale, iterations) = access_decode_llr_params();
    [(1.0, "q+pre"), (-1.0, "q-pre")]
        .into_iter()
        .filter_map(|(polarity, label)| {
            let soft = access_data_walsh_soft_symbols(chips, polarity);
            decode.packet_bits_hypotheses().iter().find_map(|&bits| {
                decode_access_phy_soft_symbols_attempt_for_packet_bits(
                    &soft, label, llr_scale, iterations, bits,
                )
                .filter(|attempt| attempt.fcs_bit_errors == 0 && attempt.tail_ones == 0)
            })
        })
        .min_by_key(access_attempt_rank)
}

fn sample_chip(
    samples: &[Complex32],
    buffer_abs_sample: i64,
    oversample: usize,
    chip: i64,
    sample_delay: i32,
    sample_delay_fraction: f32,
) -> Option<Complex32> {
    let sample_idx = chip as f64 * oversample.max(1) as f64
        + f64::from(sample_delay)
        + f64::from(sample_delay_fraction)
        - buffer_abs_sample as f64;
    if !sample_idx.is_finite() || sample_idx < 0.0 {
        return None;
    }
    let i0 = sample_idx.floor() as usize;
    let i1 = i0 + 1;
    let base = samples.get(i0).copied()?;
    let frac = (sample_idx - i0 as f64) as f32;
    if frac <= 1.0e-6 {
        return Some(base);
    }
    let neighbor = samples.get(i1).copied()?;
    Some(base * (1.0 - frac) + neighbor * frac)
}

fn complex_phase(phase: f32) -> Complex32 {
    Complex32::new(phase.cos(), phase.sin())
}

fn div_floor_i64(value: i64, divisor: i64) -> Option<i64> {
    if divisor == 0 {
        None
    } else {
        Some(value.div_euclid(divisor))
    }
}

fn tag_access_mac_fragment_fields(event: &mut SampleBlock, info_bits: &[u8]) {
    if info_bits.len() < AccessFrameLayout::DEFAULT.body_bits {
        return;
    }
    let mac_check = validate_access_mac_fragment(info_bits);
    let length_octets = pack_u8_msb(&info_bits[..8]);
    let header_reserved = pack_u8_msb(&info_bits[26..30]);
    let fragment_payload_bits = info_bits.len() - 2;
    let fragment_reserved =
        ((info_bits[fragment_payload_bits] & 1) << 1) | (info_bits[fragment_payload_bits + 1] & 1);
    event
        .tags
        .insert("hrpd_access_mac_length_octets", length_octets as i64);
    event
        .tags
        .insert("hrpd_access_header_reserved_bits", header_reserved as i64);
    event.tags.insert(
        "hrpd_access_fragment_reserved_bits",
        fragment_reserved as i64,
    );
    event.tags.insert(
        "hrpd_access_reserved_zero",
        i64::from(mac_check.reserved_zero),
    );
    event
        .tags
        .insert("hrpd_access_mac_fragment_valid", i64::from(mac_check.valid));
    event.tags.insert(
        "hrpd_access_mac_single_fragment_fcs_valid",
        i64::from(mac_check.single_fragment_fcs_valid),
    );
    if let Some(required) = mac_check.required_fragments {
        event
            .tags
            .insert("hrpd_access_mac_required_fragments", required as i64);
    }
    if let Some(capsule) = parse_access_mac_capsule(info_bits) {
        event.tags.insert(
            "hrpd_access_decoded_message_count",
            capsule.messages.len() as i64,
        );
        event.tags.insert(
            "hrpd_access_ati_type",
            capsule.ati.ati_type.as_bits() as i64,
        );
        event.tags.insert("hrpd_access_ati", capsule.ati.ati as i64);
        for packet in &capsule.messages {
            match &packet.message {
                HrpdAccessSignalingMessage::RouteUpdate(route) => {
                    event.tags.insert("hrpd_access_route_update_seen", 1);
                    event.tags.insert(
                        "hrpd_access_route_update_sequence",
                        route.message_sequence as i64,
                    );
                    event.tags.insert(
                        "hrpd_access_route_update_reference_pilot_pn",
                        route.reference_pilot_pn as i64,
                    );
                    event.tags.insert(
                        "hrpd_access_route_update_reference_keep",
                        i64::from(route.reference_keep),
                    );
                    event.tags.insert(
                        "hrpd_access_route_update_num_pilots",
                        route.num_pilots as i64,
                    );
                }
                HrpdAccessSignalingMessage::UatiRequest(uati) => {
                    event.tags.insert("hrpd_access_uati_request_seen", 1);
                    event.tags.insert(
                        "hrpd_access_uati_request_transaction_id",
                        uati.transaction_id as i64,
                    );
                }
                HrpdAccessSignalingMessage::UatiComplete(uati) => {
                    event.tags.insert("hrpd_access_uati_complete_seen", 1);
                    event.tags.insert(
                        "hrpd_access_uati_complete_sequence",
                        uati.message_sequence as i64,
                    );
                }
                HrpdAccessSignalingMessage::ConnectionRequest(connection) => {
                    event.tags.insert("hrpd_access_connection_request_seen", 1);
                    event.tags.insert(
                        "hrpd_access_connection_request_transaction_id",
                        connection.transaction_id as i64,
                    );
                    event.tags.insert(
                        "hrpd_access_connection_request_reason",
                        connection.request_reason as i64,
                    );
                }
                HrpdAccessSignalingMessage::TrafficChannelComplete(complete) => {
                    event
                        .tags
                        .insert("hrpd_access_traffic_channel_complete_seen", 1);
                    event.tags.insert(
                        "hrpd_access_traffic_channel_complete_sequence",
                        complete.message_sequence as i64,
                    );
                }
                HrpdAccessSignalingMessage::SessionClose(close) => {
                    event.tags.insert("hrpd_access_session_close_seen", 1);
                    event.tags.insert(
                        "hrpd_access_session_close_reason",
                        close.close_reason as i64,
                    );
                    event.tags.insert(
                        "hrpd_access_session_close_more_info_len",
                        close.more_info.len() as i64,
                    );
                }
                HrpdAccessSignalingMessage::ConnectionClose(close) => {
                    event.tags.insert("hrpd_access_connection_close_seen", 1);
                    event.tags.insert(
                        "hrpd_access_connection_close_reason",
                        close.close_reason as i64,
                    );
                    event.tags.insert(
                        "hrpd_access_connection_close_suspend_enable",
                        i64::from(close.suspend_enable),
                    );
                    if let Some(suspend_time) = close.suspend_time {
                        event.tags.insert(
                            "hrpd_access_connection_close_suspend_time",
                            suspend_time as i64,
                        );
                    }
                    event.tags.insert(
                        "hrpd_access_connection_close_reserved_zero",
                        i64::from(close.reserved_zero),
                    );
                }
                HrpdAccessSignalingMessage::HardwareIdResponse(hardware) => {
                    event.tags.insert("hrpd_access_hardware_id_seen", 1);
                    event.tags.insert(
                        "hrpd_access_hardware_id_transaction_id",
                        hardware.transaction_id as i64,
                    );
                    event.tags.insert(
                        "hrpd_access_hardware_id_type",
                        hardware.hardware_id_type as i64,
                    );
                    event.tags.insert(
                        "hrpd_access_hardware_id_length",
                        hardware.hardware_id_value.len() as i64,
                    );
                }
                HrpdAccessSignalingMessage::DefaultPacketXonRequest => {
                    event.tags.insert("hrpd_access_default_packet_xon_seen", 1);
                }
                HrpdAccessSignalingMessage::DefaultPacketXoffRequest => {
                    event.tags.insert("hrpd_access_default_packet_xoff_seen", 1);
                }
                HrpdAccessSignalingMessage::DefaultPacketDataReadyAck(ack) => {
                    event
                        .tags
                        .insert("hrpd_access_default_packet_data_ready_ack_seen", 1);
                    event.tags.insert(
                        "hrpd_access_default_packet_data_ready_ack_transaction",
                        ack.transaction_id as i64,
                    );
                }
                HrpdAccessSignalingMessage::Unknown { .. } => {}
            }
        }
    }
}

impl Default for HrpdAccessPacketProcessor {
    fn default() -> Self {
        Self::new()
    }
}

impl PipelineProcessor for HrpdAccessPacketProcessor {
    fn process_block(&mut self, block: SampleBlock) -> Vec<SampleBlock> {
        let sample_rate_hz = block.sample_rate_hz;
        self.append_block(block);
        self.drain_packets(sample_rate_hz)
    }

    fn flush(&mut self) -> Vec<SampleBlock> {
        self.drain_packets(ACCESS_CHIP_RATE as f64)
    }
}

fn align_up_i64(value: i64, modulus: i64) -> i64 {
    debug_assert!(modulus > 0);
    let rem = value.rem_euclid(modulus);
    if rem == 0 {
        value
    } else {
        value + (modulus - rem)
    }
}

fn align_up_phase_i64(value: i64, modulus: i64, phase: i64) -> i64 {
    debug_assert!(modulus > 0);
    let phase = phase.rem_euclid(modulus);
    let rem = (value - phase).rem_euclid(modulus);
    if rem == 0 {
        value
    } else {
        value + (modulus - rem)
    }
}

fn ranked_packet_phase_candidates(
    buffer: &[Complex32],
    buffer_abs: i64,
    nominal_frame_start: i64,
    min_delta_chips: i64,
    max_delta_chips: i64,
    step_chips: i64,
) -> Vec<i64> {
    let mut candidates = Vec::new();
    let mut delta = min_delta_chips;
    while delta <= max_delta_chips {
        let candidate_frame_start = nominal_frame_start + delta;
        if candidate_frame_start >= buffer_abs {
            let offset = (candidate_frame_start - buffer_abs) as usize;
            if offset + ACCESS_PACKET_CHIPS <= buffer.len() {
                candidates.push((
                    access_packet_phase_alignment_score(buffer, offset),
                    candidate_frame_start,
                ));
            }
        }
        delta += step_chips;
    }
    candidates.sort_by(|a, b| {
        b.0.partial_cmp(&a.0)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then_with(|| {
                (a.1 - nominal_frame_start)
                    .abs()
                    .cmp(&(b.1 - nominal_frame_start).abs())
            })
    });
    candidates.into_iter().map(|(_, start)| start).collect()
}

fn access_packet_phase_alignment_score(buffer: &[Complex32], offset: usize) -> f32 {
    let packet = &buffer[offset..offset + ACCESS_PACKET_CHIPS];
    let repetition = access_walsh2_repetition_score(packet);
    let onset = access_walsh2_onset_score(buffer, offset);
    repetition + 2.0 * onset
}

fn access_walsh2_repetition_score(chips: &[Complex32]) -> f32 {
    if chips.len() < ACCESS_DATA_WALSH_LEN {
        return 0.0;
    }
    let mut repeated = vec![0.0f32; ACCESS_CODE_SYMBOLS];
    let mut incoherent = 0.0f32;
    for (symbol_idx, chunk) in chips.chunks_exact(ACCESS_DATA_WALSH_LEN).enumerate() {
        let q = chunk
            .iter()
            .zip(ACCESS_DATA_WALSH_2)
            .map(|(chip, w)| chip.im * w)
            .sum::<f32>();
        repeated[symbol_idx % ACCESS_CODE_SYMBOLS] += q;
        incoherent += q.abs();
    }
    if incoherent <= 1.0e-9 {
        0.0
    } else {
        repeated.iter().map(|v| v.abs()).sum::<f32>() / incoherent
    }
}

fn access_walsh2_onset_score(buffer: &[Complex32], offset: usize) -> f32 {
    const ONSET_WINDOW_CHIPS: usize = 4096;
    if offset < ONSET_WINDOW_CHIPS || offset + ONSET_WINDOW_CHIPS > buffer.len() {
        return 0.0;
    }
    let pre = access_walsh2_abs_mean(&buffer[offset - ONSET_WINDOW_CHIPS..offset]);
    let post = access_walsh2_abs_mean(&buffer[offset..offset + ONSET_WINDOW_CHIPS]);
    ((post - pre) / (post + pre + 1.0e-9)).max(0.0)
}

fn access_walsh2_abs_mean(chips: &[Complex32]) -> f32 {
    let mut sum = 0.0f32;
    let mut groups = 0usize;
    for chunk in chips.chunks_exact(ACCESS_DATA_WALSH_LEN) {
        let q = chunk
            .iter()
            .zip(ACCESS_DATA_WALSH_2)
            .map(|(chip, w)| chip.im * w)
            .sum::<f32>();
        sum += q.abs();
        groups += 1;
    }
    if groups == 0 {
        0.0
    } else {
        sum / groups as f32
    }
}

fn pack_u8_msb(bits: &[u8]) -> u8 {
    let mut v = 0u8;
    for &b in bits.iter().take(8) {
        v = (v << 1) | (b & 1);
    }
    v
}

fn pack_u16_msb(bits: &[u8]) -> u16 {
    let mut v = 0u16;
    for &b in bits.iter().take(16) {
        v = (v << 1) | u16::from(b & 1);
    }
    v
}

fn pack_u32_msb(bits: &[u8]) -> u32 {
    bits.iter()
        .fold(0u32, |acc, &bit| (acc << 1) | u32::from(bit & 1))
}

fn access_mac_crc32(bits: &[u8]) -> u32 {
    let poly = 0x04c1_1db7u32;
    let mut reg = 0u32;
    for &bit in bits {
        let feedback = ((reg >> 31) & 1) ^ u32::from(bit & 1);
        reg <<= 1;
        if feedback != 0 {
            reg ^= poly;
        }
    }
    reg
}

fn pack_bytes_msb(bits: &[u8]) -> Vec<u8> {
    bits.chunks(8)
        .map(|c| {
            let mut v = 0u8;
            for &b in c {
                v = (v << 1) | (b & 1);
            }
            v << (8 - c.len() as u32).min(7)
        })
        .collect()
}

fn hex_bytes(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use core::fmt::Write;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

fn bytes_to_bits_msb(bytes: &[u8]) -> Vec<u8> {
    let mut bits = Vec::with_capacity(bytes.len() * 8);
    for &byte in bytes {
        for shift in (0..8).rev() {
            bits.push((byte >> shift) & 1);
        }
    }
    bits
}

fn read_bits_msb(bits: &[u8], cursor: &mut usize, count: usize) -> Option<u32> {
    if *cursor + count > bits.len() {
        return None;
    }
    let mut value = 0u32;
    for &bit in &bits[*cursor..*cursor + count] {
        value = (value << 1) | u32::from(bit & 1);
    }
    *cursor += count;
    Some(value)
}

fn read_bits_msb_u64(bits: &[u8], cursor: &mut usize, count: usize) -> Option<u64> {
    if *cursor + count > bits.len() {
        return None;
    }
    let mut value = 0u64;
    for &bit in &bits[*cursor..*cursor + count] {
        value = (value << 1) | u64::from(bit & 1);
    }
    *cursor += count;
    Some(value)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_frame_layout_sums_to_256() {
        let l = AccessFrameLayout::DEFAULT;
        assert_eq!(l.total_bits(), ACCESS_FRAME_BITS);
        assert_eq!(l.body_bits, 234);
        assert_eq!(l.crc_bits, 16);
        assert_eq!(l.tail_bits, 6);
    }

    fn build_info_crc(info: &[u8]) -> Vec<u8> {
        assert_eq!(info.len(), 234);
        let crc = physical_crc16(info);
        let mut out = info.to_vec();
        for i in (0..16).rev() {
            out.push(((crc >> i) & 1) as u8);
        }
        out
    }

    fn build_message_bits(message_id: u8, body: &[u8]) -> Vec<u8> {
        let mut info = Vec::with_capacity(234);
        for i in (0..8).rev() {
            info.push((message_id >> i) & 1);
        }
        info.extend_from_slice(body);
        info.resize(234, 0);
        info
    }

    #[test]
    fn round_trip_connection_request() {
        let info = build_message_bits(0x01, &[]);
        let frame = build_info_crc(&info);
        let msg = AccessFrameDecoder::decode_info_bits(&frame).unwrap();
        assert!(matches!(msg, AccessMessage::ConnectionRequest));
    }

    #[test]
    fn round_trip_keepalive() {
        let info = build_message_bits(0x03, &[]);
        let frame = build_info_crc(&info);
        let msg = AccessFrameDecoder::decode_info_bits(&frame).unwrap();
        assert!(matches!(msg, AccessMessage::KeepAlive));
    }

    #[test]
    fn round_trip_uati_request_carries_color_code() {
        let mut body = Vec::with_capacity(8);
        let cc = 0xA5u8;
        for i in (0..8).rev() {
            body.push((cc >> i) & 1);
        }
        let info = build_message_bits(0x12, &body);
        let frame = build_info_crc(&info);
        let msg = AccessFrameDecoder::decode_info_bits(&frame).unwrap();
        assert!(matches!(msg, AccessMessage::UatiRequest { color_code } if color_code == cc));
    }

    #[test]
    fn round_trip_route_update_preserves_body_prefix() {
        let info = build_message_bits(0x09, &[1, 0, 1, 1, 0, 1, 0, 0]);
        let frame = build_info_crc(&info);
        let msg = AccessFrameDecoder::decode_info_bits(&frame).unwrap();
        match msg {
            AccessMessage::RouteUpdate(b) => assert_eq!(b[0], 0b1011_0100),
            _ => panic!(),
        }
    }

    #[test]
    fn corrupted_bit_fails_crc() {
        let info = build_message_bits(0x01, &[]);
        let mut frame = build_info_crc(&info);
        frame[10] ^= 1;
        assert!(matches!(
            AccessFrameDecoder::decode_info_bits(&frame),
            Err(AccessDecodeError::CrcMismatch)
        ));
    }

    #[test]
    fn wrong_length_returns_error() {
        assert!(matches!(
            AccessFrameDecoder::decode_info_bits(&[0u8; 100]),
            Err(AccessDecodeError::WrongLength)
        ));
    }

    #[test]
    fn unknown_message_id_surfaces() {
        let info = build_message_bits(0xFE, &[]);
        let frame = build_info_crc(&info);
        assert!(matches!(
            AccessFrameDecoder::decode_info_bits(&frame),
            Err(AccessDecodeError::UnknownMessageId(0xFE))
        ));
    }

    #[test]
    fn parses_rev_a_route_update_with_four_pilots_and_zero_reserved_bits() {
        fn push_bits(bits: &mut Vec<u8>, value: u32, width: usize) {
            for i in (0..width).rev() {
                bits.push(((value >> i) & 1) as u8);
            }
        }
        // C.S0024-A §8.7.6.2.1 with NumPilots = 4 and no per-pilot channel
        // records is exactly 128 bits: the Reserved field is 0 bits wide and
        // the C.S0024-B trailing fields are absent entirely.
        let mut bits = Vec::new();
        push_bits(&mut bits, 0x00, 8); // MessageID
        push_bits(&mut bits, 0x2A, 8); // MessageSequence
        push_bits(&mut bits, 0x155, 9); // ReferencePilotPN
        push_bits(&mut bits, 0x21, 6); // ReferencePilotStrength
        push_bits(&mut bits, 1, 1); // ReferenceKeep
        push_bits(&mut bits, 4, 4); // NumPilots
        for p in 0..4u32 {
            push_bits(&mut bits, 0x1234 + p, 15); // PilotPNPhase
            push_bits(&mut bits, 0, 1); // ChannelIncluded
            push_bits(&mut bits, 10 + p, 6); // PilotStrength
            push_bits(&mut bits, 1, 1); // Keep
        }
        assert_eq!(bits.len(), 128);
        let payload: Vec<u8> = bits
            .chunks(8)
            .map(|chunk| chunk.iter().fold(0u8, |acc, &b| (acc << 1) | b))
            .collect();

        let route = parse_route_update(&payload).expect("Rev A RouteUpdate parses");
        assert_eq!(route.message_sequence, 0x2A);
        assert_eq!(route.reference_pilot_pn, 0x155);
        assert_eq!(route.reference_pilot_strength, 0x21);
        assert!(route.reference_keep);
        assert_eq!(route.num_pilots, 4);
        assert_eq!(route.at_total_pilot_transmission, None);
        assert_eq!(route.reference_pilot_channel, None);
        assert!(route.reserved_zero);
    }

    #[test]
    fn parses_default_signaling_uati_complete() {
        let msg = parse_default_signaling_payload(0x11, &[0x02, 0x07, 0x00]);
        match msg {
            HrpdAccessSignalingMessage::UatiComplete(uati) => {
                assert_eq!(uati.message_sequence, 7);
                assert!(uati.upper_old_uati.is_empty());
                assert!(uati.reserved_zero);
            }
            other => panic!("unexpected message: {other:?}"),
        }
    }

    #[test]
    fn parses_reliable_stream0_default_signaling_uati_complete() {
        let packet = hrpd_air::encode_reliable_default_signaling_packet(
            hrpd_air::DEFAULT_ADDRESS_MANAGEMENT_PROTOCOL_TYPE,
            &[0x02, 0x07, 0x00],
            3,
        );
        let parsed = parse_default_signaling_packet(&packet).expect("reliable packet should parse");
        match parsed.message {
            HrpdAccessSignalingMessage::UatiComplete(uati) => {
                assert_eq!(uati.message_sequence, 7);
                assert!(uati.upper_old_uati.is_empty());
                assert!(uati.reserved_zero);
            }
            other => panic!("unexpected message: {other:?}"),
        }
    }

    #[test]
    fn parses_default_signaling_connection_request() {
        let msg = parse_default_signaling_payload(0x0c, &[0x01, 0xa9, 0x00]);
        match msg {
            HrpdAccessSignalingMessage::ConnectionRequest(connection) => {
                assert_eq!(connection.transaction_id, 0xa9);
                assert_eq!(connection.request_reason, 0);
                assert!(connection.reserved_zero);
            }
            other => panic!("unexpected message: {other:?}"),
        }
    }

    #[test]
    fn parses_default_packet_flow_control_on_negotiated_stream_protocols() {
        for protocol_type in [
            DEFAULT_PACKET_STREAM1_APPLICATION_PROTOCOL_TYPE,
            DEFAULT_PACKET_STREAM2_APPLICATION_PROTOCOL_TYPE,
            DEFAULT_PACKET_STREAM3_APPLICATION_PROTOCOL_TYPE,
        ] {
            assert!(matches!(
                parse_default_signaling_payload(protocol_type, &[0x07]),
                HrpdAccessSignalingMessage::DefaultPacketXonRequest
            ));
            assert!(matches!(
                parse_default_signaling_payload(protocol_type, &[0x09]),
                HrpdAccessSignalingMessage::DefaultPacketXoffRequest
            ));
            match parse_default_signaling_payload(protocol_type, &[0x0c, 0xa5]) {
                HrpdAccessSignalingMessage::DefaultPacketDataReadyAck(ack) => {
                    assert_eq!(ack.transaction_id, 0xa5);
                }
                other => panic!("unexpected message for protocol 0x{protocol_type:02x}: {other:?}"),
            }
        }
    }

    #[test]
    fn parses_default_signaling_hardware_id_response() {
        let msg = parse_default_signaling_payload(
            0x11,
            &[
                0x04, 0xa5, 0x00, 0xff, 0xff, 0x07, 0xa1, 0xb2, 0xc3, 0xd4, 0xe5, 0xf6, 0x70,
            ],
        );
        match msg {
            HrpdAccessSignalingMessage::HardwareIdResponse(hardware) => {
                assert_eq!(hardware.transaction_id, 0xa5);
                assert_eq!(hardware.hardware_id_type, 0x00ff_ff);
                assert_eq!(
                    hardware.hardware_id_value,
                    vec![0xa1, 0xb2, 0xc3, 0xd4, 0xe5, 0xf6, 0x70]
                );
            }
            other => panic!("unexpected message: {other:?}"),
        }
    }

    #[test]
    fn parses_crc_valid_access_mac_capsule_from_capture() {
        let info_bytes = [
            0x14, 0x00, 0x00, 0x43, 0x50, 0xad, 0xb7, 0x64, 0x07, 0x00, 0x0e, 0x00, 0x01, 0x00,
            0x01, 0x00, 0x04, 0x00, 0x11, 0x00, 0x36, 0x67, 0x01, 0x49, 0xea, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ];
        let mut info_bits = bytes_to_bits_msb(&info_bytes);
        info_bits.truncate(AccessFrameLayout::DEFAULT.body_bits);
        let check = validate_access_mac_fragment(&info_bits);
        assert!(check.valid, "{check:?}");

        let capsule = parse_access_mac_capsule(&info_bits).expect("parsed Access MAC capsule");
        assert_eq!(capsule.length_octets, 20);
        assert_eq!(capsule.session_configuration_token, 0);
        assert!(!capsule.security_layer_format);
        assert!(capsule.connection_layer_format);
        assert_eq!(capsule.ati.ati_type, AccessTerminalIdentifierType::Rati);
        assert_eq!(capsule.ati.ati, 0x50ad_b764);
        assert_eq!(capsule.mac_fcs, 0x6701_49ea);
        assert_eq!(capsule.messages.len(), 2);

        match &capsule.messages[0].message {
            HrpdAccessSignalingMessage::RouteUpdate(route) => {
                assert_eq!(capsule.messages[0].protocol_type, 0x0e);
                assert_eq!(route.message_sequence, 1);
                assert_eq!(route.reference_pilot_pn, 0);
                assert_eq!(route.reference_pilot_strength, 0);
                assert!(route.reference_keep);
                assert_eq!(route.num_pilots, 0);
                assert_eq!(route.at_total_pilot_transmission, None);
                assert_eq!(route.reference_pilot_channel, None);
                assert!(route.reserved_zero);
            }
            other => panic!("unexpected first message: {other:?}"),
        }

        match &capsule.messages[1].message {
            HrpdAccessSignalingMessage::UatiRequest(uati) => {
                assert_eq!(capsule.messages[1].protocol_type, 0x11);
                assert_eq!(uati.transaction_id, 0x36);
            }
            other => panic!("unexpected second message: {other:?}"),
        }

        let indication = capsule.to_air_indication(123_456, 26, 0);
        assert_eq!(indication.absolute_chip, 123_456);
        assert_eq!(indication.color_code, 26);
        assert_eq!(indication.sector_pilot_pn, 0);
        assert_eq!(indication.ati.value, 0x50ad_b764);
        assert_eq!(indication.messages.len(), 2);
        match &indication.messages[0] {
            hrpd_air::HrpdAccessMessage::RouteUpdate(route) => {
                assert_eq!(route.message_sequence, 1);
                assert_eq!(route.reference_pilot_pn, 0);
                assert_eq!(route.num_pilots, 0);
            }
            other => panic!("unexpected air message: {other:?}"),
        }
        match &indication.messages[1] {
            hrpd_air::HrpdAccessMessage::UatiRequest(uati) => {
                assert_eq!(uati.transaction_id, 0x36);
            }
            other => panic!("unexpected air message: {other:?}"),
        }
    }

    #[test]
    fn parses_default_security_format_b_access_mac_capsule() {
        let info_bytes = [
            0x14, 0x00, 0x00, 0x43, 0x50, 0xad, 0xb7, 0x64, 0x07, 0x00, 0x0e, 0x00, 0x01, 0x00,
            0x01, 0x00, 0x04, 0x00, 0x11, 0x00, 0x36, 0x67, 0x01, 0x49, 0xea, 0x00, 0x00, 0x00,
            0x00, 0x00,
        ];
        let mut info_bits = bytes_to_bits_msb(&info_bytes);
        info_bits.truncate(AccessFrameLayout::DEFAULT.body_bits);
        let length_octets = pack_u8_msb(&info_bits[..8]) as usize;
        let payload_bits = 8 + length_octets * 8;

        let fcs = access_mac_crc32(&info_bits[..payload_bits]);
        for idx in 0..32 {
            info_bits[payload_bits + idx] = ((fcs >> (31 - idx)) & 1) as u8;
        }

        let check = validate_access_mac_fragment(&info_bits);
        assert!(check.valid, "{check:?}");

        let capsule = parse_access_mac_capsule(&info_bits).expect("parsed Access MAC capsule");
        assert!(!capsule.security_layer_format);
        assert!(capsule.connection_layer_format);
        assert_eq!(capsule.messages.len(), 2);
        assert!(matches!(
            capsule.messages[0].message,
            HrpdAccessSignalingMessage::RouteUpdate(_)
        ));
        assert!(matches!(
            capsule.messages[1].message,
            HrpdAccessSignalingMessage::UatiRequest(_)
        ));
    }

    #[test]
    fn parses_authenticated_format_b_access_mac_capsule() {
        let connection_payload = [
            0x07, 0x00, 0x0e, 0x00, 0x01, 0x00, 0x01, 0x00, 0x04, 0x00, 0x11, 0x00, 0x36,
        ];
        let mut capsule_bytes = vec![
            0x1e, 0x00, 0x00, 0xc3, 0x50, 0xad, 0xb7, 0x64, 0xa5, 0x5a, 0x11, 0x22, 0x33, 0x44,
            0x55, 0x66, 0x77, 0x88,
        ];
        capsule_bytes.extend_from_slice(&connection_payload);
        let mut info_bits = bytes_to_bits_msb(&capsule_bytes);
        let length_octets = pack_u8_msb(&info_bits[..8]) as usize;
        let payload_bits = 8 + length_octets * 8;
        let fcs = access_mac_crc32(&info_bits[..payload_bits]);
        for shift in (0..32).rev() {
            info_bits.push(((fcs >> shift) & 1) as u8);
        }
        info_bits.resize(
            AccessFrameLayout::for_packet_bits(512).unwrap().body_bits,
            0,
        );

        let capsule = parse_access_mac_capsule(&info_bits).expect("parsed Access MAC capsule");

        assert!(capsule.security_layer_format);
        assert!(capsule.connection_layer_format);
        assert_eq!(capsule.length_octets, 30);
        assert_eq!(capsule.messages.len(), 2);
        assert!(matches!(
            capsule.messages[0].message,
            HrpdAccessSignalingMessage::RouteUpdate(_)
        ));
        assert!(matches!(
            capsule.messages[1].message,
            HrpdAccessSignalingMessage::UatiRequest(_)
        ));
        assert!(
            capsule
                .format_b_parse_trace()
                .contains("generic+sha1-access:off=0 len=7")
        );
    }

    #[test]
    fn rate_1_4_access_phy_round_trip_crc_valid() {
        let info = build_message_bits(0x01, &[]);
        let frame_bits = build_access_phy_bits(&info);
        let soft = encode_access_phy_soft_symbols(&frame_bits);
        let decoded = decode_access_phy_soft_symbols(&soft).expect("CRC-valid access PHY packet");
        assert_eq!(decoded.message_id, 0x01);
        assert_eq!(decoded.info_bits, info);
        assert!(matches!(
            decoded.message,
            Ok(AccessMessage::ConnectionRequest)
        ));
    }

    #[test]
    fn walsh_q_chip_round_trip_crc_valid() {
        let info = build_message_bits(0x12, &[0, 0, 0, 1, 1, 0, 1, 0]);
        let frame_bits = build_access_phy_bits(&info);
        let soft = encode_access_phy_soft_symbols(&frame_bits);
        let mut chips = Vec::with_capacity(ACCESS_PACKET_CHIPS);
        for s in soft {
            for &w in &ACCESS_DATA_WALSH_2 {
                chips.push(Complex32::new(1.0, s * w));
            }
        }
        let decoded = decode_access_phy_chips(&chips).expect("CRC-valid access chips");
        assert_eq!(decoded.message_id, 0x12);
        assert!(
            matches!(decoded.message, Ok(AccessMessage::UatiRequest { color_code }) if color_code == 0x1a)
        );
    }

    #[test]
    fn access_rate_parameters_match_encoder_table() {
        // C.S0024-A Table 13.2.1.3.4-1 plus the repetition factors filling
        // the fixed 8192-symbol 16-slot packet.
        for (rate, bps, packet_bits, repeats) in [
            (HrpdAccessRate::Rate9k6, 9_600, 256, 8),
            (HrpdAccessRate::Rate19k2, 19_200, 512, 4),
            (HrpdAccessRate::Rate38k4, 38_400, 1024, 2),
        ] {
            assert_eq!(rate.bps(), bps);
            assert_eq!(rate.packet_bits(), packet_bits);
            assert_eq!(rate.sequence_repeats(), repeats);
            assert_eq!(HrpdAccessRate::from_packet_bits(packet_bits), Some(rate));
            assert_eq!(rate.packet_bits() * 4 * repeats, ACCESS_MODULATION_SYMBOLS);
            let layout = AccessFrameLayout::for_packet_bits(packet_bits).unwrap();
            assert_eq!(layout.crc_bits, 16);
            assert_eq!(layout.tail_bits, 6);
        }
        assert_eq!(
            AccessFrameLayout::for_packet_bits(256).unwrap().body_bits,
            234
        );
        assert_eq!(
            AccessFrameLayout::for_packet_bits(512).unwrap().body_bits,
            490
        );
        assert_eq!(
            AccessFrameLayout::for_packet_bits(1024).unwrap().body_bits,
            1002
        );
        assert_eq!(
            HrpdAccessRate::from_sector_access_max_rate_code(0b00),
            Some(HrpdAccessRate::Rate9k6)
        );
        assert_eq!(
            HrpdAccessRate::from_sector_access_max_rate_code(0b01),
            Some(HrpdAccessRate::Rate19k2)
        );
        assert_eq!(
            HrpdAccessRate::from_sector_access_max_rate_code(0b10),
            Some(HrpdAccessRate::Rate38k4)
        );
        assert_eq!(HrpdAccessRate::from_sector_access_max_rate_code(0b11), None);
    }

    #[test]
    fn access_rate_for_payload_bits_matches_table() {
        use HrpdAccessRate::*;
        // Table 10.5.6.1.4.1.2-1 thresholds.
        for (payload, expected) in [
            (1usize, Some(Rate9k6)),
            (232, Some(Rate9k6)),
            (233, Some(Rate19k2)),
            (488, Some(Rate19k2)),
            (489, Some(Rate38k4)),
            (1000, Some(Rate38k4)),
            (0, None),
            (1001, None),
        ] {
            assert_eq!(
                access_rate_for_payload_bits(payload, Rate38k4),
                expected,
                "payload {payload}"
            );
        }
        // AccessRateMax caps the selection.
        assert_eq!(access_rate_for_payload_bits(300, Rate9k6), Some(Rate9k6));
        assert_eq!(access_rate_for_payload_bits(600, Rate19k2), Some(Rate19k2));
        assert_eq!(access_rate_for_payload_bits(600, Rate9k6), Some(Rate9k6));
        assert_eq!(access_rate_for_payload_bits(100, Rate38k4), Some(Rate9k6));
    }

    #[test]
    fn decode_config_hypotheses_are_gated() {
        assert_eq!(
            HrpdAccessDecodeConfig::REV0.packet_bits_hypotheses(),
            &[256]
        );
        assert_eq!(
            HrpdAccessDecodeConfig::ENHANCED.packet_bits_hypotheses(),
            &[256, 512, 1024]
        );
    }

    fn build_enhanced_message_bits(message_id: u8, body_bits: usize) -> Vec<u8> {
        let mut info = Vec::with_capacity(body_bits);
        for i in (0..8).rev() {
            info.push((message_id >> i) & 1);
        }
        // Deterministic non-trivial payload pattern.
        while info.len() < body_bits {
            let k = info.len();
            info.push(((k * 7 + k / 9) & 1) as u8);
        }
        info
    }

    fn synthetic_access_chips(frame_bits: &[u8]) -> Vec<Complex32> {
        let soft = encode_access_phy_soft_symbols(frame_bits);
        let mut chips = Vec::with_capacity(ACCESS_PACKET_CHIPS);
        for s in soft {
            for &w in &ACCESS_DATA_WALSH_2 {
                chips.push(Complex32::new(1.0, s * w));
            }
        }
        chips
    }

    #[test]
    fn enhanced_512_bit_chip_round_trip_and_rev0_gating() {
        let layout = AccessFrameLayout::for_packet_bits(512).unwrap();
        let info = build_enhanced_message_bits(0x5a, layout.body_bits);
        let frame_bits = build_access_phy_bits_for_packet_bits(&info, 512);
        let chips = synthetic_access_chips(&frame_bits);
        assert_eq!(chips.len(), ACCESS_PACKET_CHIPS);

        let decoded = decode_access_phy_chips_with_config(&chips, HrpdAccessDecodeConfig::ENHANCED)
            .expect("CRC-valid 19.2 kbps access packet");
        assert_eq!(decoded.packet_bits, 512);
        assert_eq!(decoded.message_id, 0x5a);
        assert_eq!(decoded.info_bits, info);

        // Rev 0 configuration must not decode the enhanced packet size.
        assert!(decode_access_phy_chips(&chips).is_none());
    }

    #[test]
    fn enhanced_1024_bit_chip_round_trip_and_rev0_gating() {
        let layout = AccessFrameLayout::for_packet_bits(1024).unwrap();
        let info = build_enhanced_message_bits(0xc3, layout.body_bits);
        let frame_bits = build_access_phy_bits_for_packet_bits(&info, 1024);
        let chips = synthetic_access_chips(&frame_bits);

        let decoded = decode_access_phy_chips_with_config(&chips, HrpdAccessDecodeConfig::ENHANCED)
            .expect("CRC-valid 38.4 kbps access packet");
        assert_eq!(decoded.packet_bits, 1024);
        assert_eq!(decoded.message_id, 0xc3);
        assert_eq!(decoded.info_bits, info);

        assert!(decode_access_phy_chips(&chips).is_none());
    }

    #[test]
    fn enhanced_config_still_decodes_rev0_packets() {
        let info = build_message_bits(0x01, &[]);
        let frame_bits = build_access_phy_bits(&info);
        let chips = synthetic_access_chips(&frame_bits);
        let decoded = decode_access_phy_chips_with_config(&chips, HrpdAccessDecodeConfig::ENHANCED)
            .expect("CRC-valid 9.6 kbps access packet under enhanced config");
        assert_eq!(decoded.packet_bits, ACCESS_FRAME_BITS);
        assert_eq!(decoded.info_bits, info);
    }

    #[test]
    fn enhanced_soft_symbol_round_trips() {
        for packet_bits in [512usize, 1024] {
            let layout = AccessFrameLayout::for_packet_bits(packet_bits).unwrap();
            let info = build_enhanced_message_bits(0x22, layout.body_bits);
            let frame_bits = build_access_phy_bits_for_packet_bits(&info, packet_bits);
            let soft = encode_access_phy_soft_symbols(&frame_bits);
            assert_eq!(soft.len(), ACCESS_MODULATION_SYMBOLS);
            let decoded =
                decode_access_phy_soft_symbols_with_config(&soft, HrpdAccessDecodeConfig::ENHANCED)
                    .expect("CRC-valid enhanced soft-symbol packet");
            assert_eq!(decoded.packet_bits, packet_bits);
            assert_eq!(decoded.info_bits, info);
            assert!(decode_access_phy_soft_symbols(&soft).is_none());
        }
    }
}
