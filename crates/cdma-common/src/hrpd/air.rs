//! Shared HRPD air-interface boundary messages between the BTS and AN.
//!
//! These are not IOS A-interface messages. They are the serialized internal
//! sector-control contract: the BTS owns RF/PHY/MAC decoding and modulation,
//! while the AN owns UATI, session, connection, and packet-service policy.

use crate::bits::Bitstream;
use crate::hrpd::messages::DEFAULT_ACCESS_CHANNEL_MAC_PROTOCOL_TYPE;
use crate::phy::long_code::LongCodeGenerator;
use serde::{Deserialize, Serialize};
use std::time::Instant;

pub const DEFAULT_IDLE_STATE_PROTOCOL_TYPE: u8 = 0x0c;
pub const DEFAULT_CONNECTED_STATE_PROTOCOL_TYPE: u8 = 0x0d;
pub const DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE: u8 = 0x0e;
pub const DEFAULT_SESSION_MANAGEMENT_PROTOCOL_TYPE: u8 = 0x10;
pub const DEFAULT_ADDRESS_MANAGEMENT_PROTOCOL_TYPE: u8 = 0x11;
pub const DEFAULT_SESSION_CONFIGURATION_PROTOCOL_TYPE: u8 = 0x12;
pub const DEFAULT_STREAM0_APPLICATION_PROTOCOL_TYPE: u8 = 0x14;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessTerminalIdentifierType {
    Bati,
    Reserved,
    Uati,
    Rati,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct AccessTerminalIdentifier {
    pub ati_type: AccessTerminalIdentifierType,
    pub value: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HrpdUatiRequest {
    pub transaction_id: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HrpdUatiComplete {
    pub message_sequence: u8,
    pub upper_old_uati: Vec<u8>,
    pub reserved_zero: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HrpdConnectionRequest {
    pub transaction_id: u8,
    pub request_reason: u8,
    pub reserved_zero: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HrpdTrafficChannelComplete {
    pub message_sequence: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HrpdSessionClose {
    pub close_reason: u8,
    pub more_info: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HrpdProtocolReference {
    pub protocol_type: u16,
    pub protocol_subtype: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HrpdConnectionClose {
    pub close_reason: u8,
    pub suspend_enable: bool,
    pub suspend_time: Option<u64>,
    pub reserved_zero: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HrpdHardwareIdResponse {
    pub transaction_id: u8,
    pub hardware_id_type: u32,
    pub hardware_id_value: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HrpdDefaultPacketDataReadyAck {
    pub transaction_id: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HrpdDefaultPacketRlpReset;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HrpdDefaultPacketRlpResetAck;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HrpdDefaultPacketRlpNakRequest {
    pub first_erased: u32,
    pub window_len: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HrpdDefaultPacketRlpNak {
    pub requests: Vec<HrpdDefaultPacketRlpNakRequest>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HrpdDefaultSignalingReset {
    pub message_sequence: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HrpdDefaultSignalingResetAck {
    pub message_sequence: u8,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HrpdAccessMessage {
    RouteUpdate(HrpdRouteUpdate),
    UatiRequest(HrpdUatiRequest),
    UatiComplete(HrpdUatiComplete),
    ConnectionRequest(HrpdConnectionRequest),
    TrafficChannelComplete(HrpdTrafficChannelComplete),
    SessionClose(HrpdSessionClose),
    ConnectionClose(HrpdConnectionClose),
    HardwareIdResponse(HrpdHardwareIdResponse),
    KeepAlive,
    DefaultPacketXonRequest,
    DefaultPacketXoffRequest,
    DefaultPacketDataReadyAck(HrpdDefaultPacketDataReadyAck),
    DefaultPacketRlpReset(HrpdDefaultPacketRlpReset),
    DefaultPacketRlpResetAck(HrpdDefaultPacketRlpResetAck),
    DefaultPacketRlpNak(HrpdDefaultPacketRlpNak),
    DefaultSignalingReset(HrpdDefaultSignalingReset),
    DefaultSignalingResetAck(HrpdDefaultSignalingResetAck),
    Unknown {
        protocol_type: u8,
        message_id: Option<u8>,
        payload: Vec<u8>,
    },
}

pub fn hrpd_session_close_reason_name(close_reason: u8) -> &'static str {
    match close_reason {
        0x00 => "Normal Close",
        0x01 => "Close Reply",
        0x02 => "Protocol Error",
        0x03 => "Protocol Configuration Failure",
        0x04 => "Protocol Negotiation Error",
        0x05 => "Session Configuration Failure",
        0x06 => "Session Lost",
        0x07 => "Session Unreachable",
        0x08 => "All session resources busy",
        _ => "Reserved",
    }
}

pub fn hrpd_connection_close_reason_name(close_reason: u8) -> &'static str {
    match close_reason {
        0x00 => "Normal Close",
        0x01 => "Close Reply",
        0x02 => "Connection Error",
        0x03 => "1x transition",
        0x04 => "Normal Close with non-zero suspend request",
        0x05 => "Normal Close with zero suspend request",
        _ => "Reserved",
    }
}

pub fn hrpd_protocol_reference_from_more_info(more_info: &[u8]) -> Option<HrpdProtocolReference> {
    let type_is_15_bits = more_info.first()? & 0x80 != 0;
    if type_is_15_bits {
        if more_info.len() < 4 {
            return None;
        }
        Some(HrpdProtocolReference {
            protocol_type: (u16::from(more_info[0] & 0x7f) << 8) | u16::from(more_info[1]),
            protocol_subtype: (u16::from(more_info[2]) << 8) | u16::from(more_info[3]),
        })
    } else {
        if more_info.len() < 3 {
            return None;
        }
        Some(HrpdProtocolReference {
            protocol_type: u16::from(more_info[0] & 0x7f),
            protocol_subtype: (u16::from(more_info[1]) << 8) | u16::from(more_info[2]),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HrpdAccessIndication {
    pub absolute_chip: u64,
    pub color_code: u8,
    pub sector_pilot_pn: u16,
    pub session_configuration_token: u16,
    pub ati: AccessTerminalIdentifier,
    pub security_layer_format: bool,
    pub connection_layer_format: bool,
    pub security_payload: Vec<u8>,
    pub messages: Vec<HrpdAccessMessage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HrpdForwardChannel {
    SynchronousControl,
    AsynchronousControl,
    ForwardTraffic,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HrpdForwardSignalingRequest {
    pub uati: Option<u32>,
    pub target_ati: AccessTerminalIdentifier,
    pub protocol_type: u8,
    pub payload: Vec<u8>,
    pub channel: HrpdForwardChannel,
    pub reliable_sequence: Option<u8>,
    #[serde(default)]
    pub synchronous_control_cycle: Option<HrpdSynchronousControlCycle>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HrpdSynchronousControlCycle {
    pub modulus: u16,
    pub residue: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HrpdUatiAssignment {
    pub message_sequence: u8,
    pub uati_color_code: u8,
    pub uati024: u32,
    pub upper_old_uati_length: u8,
    pub subnet: Option<HrpdUatiSubnetAssignment>,
}

impl HrpdUatiAssignment {
    pub const MESSAGE_ID: u8 = 0x01;

    pub fn from_uati032(message_sequence: u8, uati_color_code: u8, uati032: u32) -> Self {
        Self {
            message_sequence,
            uati_color_code,
            uati024: uati032 & 0x00ff_ffff,
            upper_old_uati_length: 0,
            subnet: None,
        }
    }

    pub fn with_subnet(mut self, subnet: HrpdUatiSubnetAssignment) -> Self {
        self.subnet = Some(subnet);
        self
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut bs = Bitstream::new();
        bs.write_u8(Self::MESSAGE_ID, 8);
        bs.write_u8(self.message_sequence, 8);
        bs.write_u8(0, 7);
        bs.write_u8(self.subnet.is_some() as u8, 1);
        if let Some(subnet) = &self.subnet {
            bs.write_u8(subnet.uati_subnet_mask, 8);
            for byte in &subnet.uati104 {
                bs.write_u8(*byte, 8);
            }
        }
        bs.write_u8(self.uati_color_code, 8);
        bs.write_u32(self.uati024 & 0x00ff_ffff, 24);
        bs.write_u8(self.upper_old_uati_length & 0x0f, 4);
        bs.write_u8(0, 4);
        bs.to_packed_bytes()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HrpdUatiSubnetAssignment {
    pub uati_subnet_mask: u8,
    pub uati104: [u8; 13],
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HrpdTrafficChannelAssignment {
    pub message_sequence: u8,
    pub channel: Option<HrpdChannelRecord>,
    pub frame_offset: u8,
    pub drc_length: u8,
    pub drc_channel_gain_half_db: i8,
    pub ack_channel_gain_half_db: i8,
    pub pilots: Vec<HrpdTrafficPilotAssignment>,
}

impl HrpdTrafficChannelAssignment {
    pub const MESSAGE_ID: u8 = 0x01;

    pub fn single_pilot(
        message_sequence: u8,
        channel: Option<HrpdChannelRecord>,
        pilot_pn: u16,
        mac_index: u8,
    ) -> Self {
        Self {
            message_sequence,
            channel,
            frame_offset: 0,
            // C.S0024-0 §6.6.6.2: DRCLength code 3 assigns an 8-slot DRC
            // window, giving the AN the widest spec-defined packet-start
            // interval after each completed DRC.
            drc_length: 3,
            // C.S0024-0 §6.6.6.2: reverse MAC channel gains are assigned by
            // the AN. Use a strong DRC within the mandated supported range
            // so the BTS can make rate decisions from a stable DRC stream.
            drc_channel_gain_half_db: 12, // +6 dB, valid DRC range is -9..+6 dB.
            ack_channel_gain_half_db: 0,  // 0 dB, valid ACK range is -3..+6 dB.
            pilots: vec![HrpdTrafficPilotAssignment {
                pilot_pn,
                softer_handoff: false,
                mac_index,
                // DRC cover index 0 is the NULL cover (C.S0024-0
                // §8.4.6.1.4): an AT covering its DRC with it is inhibiting
                // forward transmission and will not decode Forward Traffic
                // Channel packets — assigning it as the sector cover made
                // the AT deaf to RTCAck. Use sector cover 1.
                drc_cover: 1,
                rab_length: 0,
                rab_offset: 0,
            }],
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        self.encode_subtype0_route_update()
    }

    pub fn encode_subtype0_route_update(&self) -> Vec<u8> {
        let mut bs = Bitstream::new();
        self.write_subtype0_route_update_base(&mut bs);
        Self::pad_to_octet(&mut bs);
        bs.to_packed_bytes()
    }

    pub fn encode_subtype0_route_update_with_rev_a_tail(&self) -> Vec<u8> {
        let mut bs = Bitstream::new();
        self.write_subtype0_route_update_base(&mut bs);
        // The C.S0024-A §8.7.6.2.2 Route Update TCA keeps the legacy default
        // message format and appends this optional chain for
        // subtype-2/RTC-MAC-3 public data. The chain ends at DSC in Rev A;
        // DeltaT2PsIncluded is a Rev B addition and must not be written here.
        bs.write_u8(1, 1); // RAChannelGainIncluded.
        for _pilot in self.pilots.iter().take(15) {
            bs.write_u8(0, 2); // RAChannelGain = -6 dB.
        }
        bs.write_u8(1, 1); // MACIndexMSBsIncluded.
        for pilot in self.pilots.iter().take(15) {
            bs.write_u8((pilot.mac_index >> 6) & 0x01, 1);
        }
        bs.write_u8(0, 5); // DSCChannelGainBase = 0 dB relative to pilot.
        for _pilot in self
            .pilots
            .iter()
            .take(15)
            .filter(|pilot| !pilot.softer_handoff)
        {
            bs.write_u8(0, 3); // DSC for each non-soft sector.
        }
        Self::pad_to_octet(&mut bs);
        bs.to_packed_bytes()
    }

    fn write_subtype0_route_update_base(&self, bs: &mut Bitstream) {
        bs.write_u8(Self::MESSAGE_ID, 8);
        bs.write_u8(self.message_sequence, 8);
        bs.write_u8(self.channel.is_some() as u8, 1);
        if let Some(channel) = self.channel {
            channel.write(bs);
        }
        bs.write_u8(self.frame_offset & 0x0f, 4);
        bs.write_u8(self.drc_length & 0x03, 2);
        bs.write_u8(twos_complement_6(self.drc_channel_gain_half_db), 6);
        bs.write_u8(twos_complement_6(self.ack_channel_gain_half_db), 6);
        bs.write_u8((self.pilots.len().min(15) as u8) & 0x0f, 4);
        for pilot in self.pilots.iter().take(15) {
            bs.write_u32((pilot.pilot_pn & 0x01ff) as u32, 9);
            bs.write_u8(pilot.softer_handoff as u8, 1);
            bs.write_u8(pilot.mac_index & 0x3f, 6);
            bs.write_u8(pilot.drc_cover & 0x07, 3);
            bs.write_u8(pilot.rab_length & 0x03, 2);
            bs.write_u8(pilot.rab_offset & 0x07, 3);
        }
    }

    fn pad_to_octet(bs: &mut Bitstream) {
        while bs.len() % 8 != 0 {
            bs.write_u8(0, 1);
        }
    }

    pub fn encode_subtype1_route_update(&self) -> Vec<u8> {
        let mut bs = Bitstream::new();
        let pilot = self
            .pilots
            .first()
            .copied()
            .unwrap_or(HrpdTrafficPilotAssignment {
                pilot_pn: 0,
                softer_handoff: false,
                mac_index: 5,
                drc_cover: 0,
                rab_length: 0,
                rab_offset: 0,
            });
        let assigned_channel_included = self.channel.is_some();
        let mac_index = u16::from(pilot.mac_index);

        bs.write_u8(Self::MESSAGE_ID, 8);
        bs.write_u8(self.message_sequence, 8);
        bs.write_u8(0, 5); // DSCChannelGainBase = 0 dB relative to pilot.
        bs.write_u8(self.frame_offset & 0x0f, 4);
        bs.write_u8(1, 5); // NumSectors.
        bs.write_u8(1, 4); // NumSubActiveSets.
        bs.write_u8(assigned_channel_included as u8, 1);
        bs.write_u8(0, 1); // SchedulerTagIncluded.
        bs.write_u8(0, 1); // FeedbackMultiplexingEnabled.

        bs.write_u8(0, 2); // RAChannelGain = -6 dB.
        bs.write_u32(u32::from(pilot.pilot_pn & 0x01ff), 9);
        bs.write_u8(pilot.drc_cover & 0x07, 3);
        bs.write_u8(pilot.softer_handoff as u8, 1);

        bs.write_u8(0, 3); // DSC for the single non-softer-handoff cell.

        if let Some(channel) = self.channel {
            bs.write_u8(1, 4); // NumFwdChannelsThisSubActiveSet.
            channel.write(&mut bs);
        }
        bs.write_u8(1, 1); // FeedbackEnabled.
        bs.write_u8(0, 4); // FeedbackReverseChannelIndex.
        bs.write_u8(1, 1); // SubActiveSetCarriesControlChannel.
        bs.write_u8(0, 1); // ThisSubActiveSetNotReportable.
        bs.write_u8(0, 1); // DSCForThisSubActiveSetEnabled.
        bs.write_u8(0, 1); // Next3FieldsSameAsBefore.
        bs.write_u8(self.drc_length & 0x03, 2);
        bs.write_u8(twos_complement_6(self.drc_channel_gain_half_db), 6);
        bs.write_u8(twos_complement_6(self.ack_channel_gain_half_db), 6);
        bs.write_u8(1, 1); // NumReverseChannelsIncluded.
        bs.write_u8(1, 4); // NumReverseChannels.
        bs.write_u8(0b01, 2); // Paired reverse link channel enabled.
        bs.write_u8(0, 3); // ReverseChannelDroppingRank.

        bs.write_u8(1, 1); // PilotInThisSectorIncluded.
        bs.write_u8(0, 4); // ForwardChannelIndexThisPilot.
        bs.write_u8(0, 3); // PilotGroupID.
        bs.write_u8(1, 3); // NumUniqueForwardTrafficMACIndices.
        bs.write_u8(0, 1); // AuxDRCCoverIncluded.
        bs.write_u8(0, 1); // ForwardTrafficMACIndexPerInterlaceEnabled.
        bs.write_u32(u32::from(mac_index & 0x03ff), 10);
        bs.write_u32(u32::from(mac_index & 0x01ff), 9); // ReverseLinkMACIndex.
        bs.write_u8(pilot.mac_index & 0x7f, 7); // RABMACIndex.
        bs.write_u8(0, 6); // DeltaT2P = 0 dB.

        Self::pad_to_octet(&mut bs);
        bs.to_packed_bytes()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HrpdChannelRecord {
    pub system_type: u8,
    pub band_class: u8,
    pub channel_number: u16,
}

impl HrpdChannelRecord {
    fn write(self, bs: &mut Bitstream) {
        bs.write_u8(self.system_type, 8);
        bs.write_u8(self.band_class & 0x1f, 5);
        bs.write_u32((self.channel_number & 0x07ff) as u32, 11);
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HrpdTrafficPilotAssignment {
    pub pilot_pn: u16,
    pub softer_handoff: bool,
    pub mac_index: u8,
    pub drc_cover: u8,
    pub rab_length: u8,
    pub rab_offset: u8,
}

pub fn encode_default_signaling_packet(protocol_type: u8, payload: &[u8]) -> Vec<u8> {
    encode_default_signaling_packet_for_instance(protocol_type, payload, false)
}

pub fn encode_default_signaling_packet_for_instance(
    protocol_type: u8,
    payload: &[u8],
    in_configuration: bool,
) -> Vec<u8> {
    let mut bs = Bitstream::new();
    bs.write_u8(0, 2); // Stream 0: Default Signaling Application.
    bs.write_u8(0, 4); // SLP-F Reserved.
    bs.write_u8(0, 1); // SLP-F unfragmented.
    bs.write_u8(0, 1); // SLP-D best-effort without full header.
    bs.write_u8(in_configuration as u8, 1); // SNP protocol instance.
    bs.write_u8(protocol_type & 0x7f, 7);
    for byte in payload {
        bs.write_u8(*byte, 8);
    }
    bs.to_packed_bytes()
}

pub fn encode_reliable_default_signaling_packet(
    protocol_type: u8,
    payload: &[u8],
    sequence_number: u8,
) -> Vec<u8> {
    encode_reliable_default_signaling_packet_for_instance(
        protocol_type,
        payload,
        sequence_number,
        false,
    )
}

pub fn encode_reliable_default_signaling_packet_for_instance(
    protocol_type: u8,
    payload: &[u8],
    sequence_number: u8,
    in_configuration: bool,
) -> Vec<u8> {
    encode_reliable_default_signaling_packet_for_instance_with_ack(
        protocol_type,
        payload,
        sequence_number,
        in_configuration,
        None,
    )
}

pub fn encode_reliable_default_signaling_packet_for_instance_with_ack(
    protocol_type: u8,
    payload: &[u8],
    sequence_number: u8,
    in_configuration: bool,
    ack_sequence_number: Option<u8>,
) -> Vec<u8> {
    let mut bs = Bitstream::new();
    bs.write_u8(0, 2); // Stream 0: Default Signaling Application.
    bs.write_u8(0, 4); // SLP-F Reserved.
    bs.write_u8(0, 1); // SLP-F unfragmented.
    bs.write_u8(1, 1); // SLP-D full header follows.
    bs.write_u8(ack_sequence_number.is_some() as u8, 1);
    bs.write_u8(ack_sequence_number.unwrap_or_default() & 0x07, 3);
    bs.write_u8(1, 1); // SequenceValid=true for reliable delivery.
    bs.write_u8(sequence_number & 0x07, 3);
    bs.write_u8(in_configuration as u8, 1); // SNP protocol instance.
    bs.write_u8(protocol_type & 0x7f, 7);
    for byte in payload {
        bs.write_u8(*byte, 8);
    }
    bs.to_packed_bytes()
}

pub fn encode_default_signaling_slp_d_ack_packet(ack_sequence_number: u8) -> Vec<u8> {
    let mut bs = Bitstream::new();
    bs.write_u8(0, 2); // Stream 0: Default Signaling Application.
    bs.write_u8(0, 4); // SLP-F Reserved.
    bs.write_u8(0, 1); // SLP-F unfragmented.
    bs.write_u8(1, 1); // SLP-D full header follows.
    bs.write_u8(1, 1); // AckSequenceValid=true.
    bs.write_u8(ack_sequence_number & 0x07, 3);
    bs.write_u8(0, 1); // SequenceValid=false for header-only best effort.
    bs.write_u8(0, 3); // SequenceNumber ignored.
    bs.to_packed_bytes()
}

pub fn encode_default_signaling_slp_reset_packet(message_sequence: u8) -> Vec<u8> {
    let mut bs = Bitstream::new();
    bs.write_u8(0, 2); // Stream 0: Default Signaling Application.
    bs.write_u8(0, 4); // SLP-F Reserved.
    bs.write_u8(0, 1); // SLP-F unfragmented.
    bs.write_u8(0, 1); // SLP-D best-effort without full header.
    bs.write_u8(0x00, 8); // SLP Reset.
    bs.write_u8(message_sequence, 8);
    bs.to_packed_bytes()
}

impl HrpdForwardSignalingRequest {
    /// Default Idle State Page.
    ///
    /// C.S0024-400-C §1.4.6.2.1 / §1.5.6.2.1 defines Page as MessageID
    /// `0x00`, sent unicast on the synchronous Control Channel using best-effort SLP.
    pub fn idle_state_page(uati: u32, target_ati: AccessTerminalIdentifier) -> Self {
        Self::idle_state_page_for_control_cycle(uati, target_ati, None)
    }

    pub fn idle_state_page_for_control_cycle(
        uati: u32,
        target_ati: AccessTerminalIdentifier,
        synchronous_control_cycle: Option<HrpdSynchronousControlCycle>,
    ) -> Self {
        Self {
            uati: Some(uati),
            target_ati,
            protocol_type: DEFAULT_IDLE_STATE_PROTOCOL_TYPE,
            payload: vec![0x00],
            channel: HrpdForwardChannel::SynchronousControl,
            reliable_sequence: None,
            synchronous_control_cycle,
        }
    }

    /// Default Access Channel MAC ACAck.
    ///
    /// C.S0024-300 §1.4.6.2.5 defines ACAck as an 8-bit MessageID `0x00`,
    /// sent unicast on the Control Channel using best-effort SLP.
    pub fn access_channel_ack(target_ati: AccessTerminalIdentifier) -> Self {
        Self {
            uati: None,
            target_ati,
            protocol_type: DEFAULT_ACCESS_CHANNEL_MAC_PROTOCOL_TYPE,
            payload: vec![0x00],
            channel: HrpdForwardChannel::AsynchronousControl,
            reliable_sequence: None,
            synchronous_control_cycle: None,
        }
    }

    pub fn uati_assignment(
        uati: u32,
        message_sequence: u8,
        color_code: u8,
        target_ati: AccessTerminalIdentifier,
        subnet: Option<HrpdUatiSubnetAssignment>,
    ) -> Self {
        let mut assignment = HrpdUatiAssignment::from_uati032(message_sequence, color_code, uati);
        if let Some(subnet) = subnet {
            assignment = assignment.with_subnet(subnet);
        }
        Self {
            uati: Some(uati),
            target_ati,
            protocol_type: DEFAULT_ADDRESS_MANAGEMENT_PROTOCOL_TYPE,
            payload: assignment.encode(),
            channel: HrpdForwardChannel::AsynchronousControl,
            reliable_sequence: None,
            synchronous_control_cycle: None,
        }
    }

    pub fn traffic_channel_assignment(
        uati: u32,
        target_ati: AccessTerminalIdentifier,
        assignment: HrpdTrafficChannelAssignment,
    ) -> Self {
        Self::traffic_channel_assignment_for_route_update_subtype(uati, target_ati, assignment, 0)
    }

    pub fn traffic_channel_assignment_for_route_update_subtype(
        uati: u32,
        target_ati: AccessTerminalIdentifier,
        assignment: HrpdTrafficChannelAssignment,
        route_update_subtype: u16,
    ) -> Self {
        Self::traffic_channel_assignment_for_route_update_subtype_with_rev_a_tail(
            uati,
            target_ati,
            assignment,
            route_update_subtype,
            false,
        )
    }

    pub fn traffic_channel_assignment_for_route_update_subtype_with_rev_a_tail(
        uati: u32,
        target_ati: AccessTerminalIdentifier,
        assignment: HrpdTrafficChannelAssignment,
        route_update_subtype: u16,
        default_route_update_rev_a_tail: bool,
    ) -> Self {
        let payload = match route_update_subtype {
            1 => assignment.encode_subtype1_route_update(),
            _ if default_route_update_rev_a_tail => {
                assignment.encode_subtype0_route_update_with_rev_a_tail()
            }
            _ => assignment.encode_subtype0_route_update(),
        };
        Self {
            uati: Some(uati),
            target_ati,
            protocol_type: DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE,
            payload,
            channel: HrpdForwardChannel::AsynchronousControl,
            reliable_sequence: None,
            synchronous_control_cycle: None,
        }
    }

    /// Default Connected State ConnectionClose.
    ///
    /// C.S0024-400-C §1.7.6.2.1 defines MessageID `0x00`; the AN sets
    /// SuspendEnable to zero, so CloseReason plus padding fits in one octet.
    pub fn connection_close(
        uati: u32,
        target_ati: AccessTerminalIdentifier,
        close_reason: u8,
    ) -> Self {
        Self {
            uati: Some(uati),
            target_ati,
            protocol_type: DEFAULT_CONNECTED_STATE_PROTOCOL_TYPE,
            payload: vec![0x00, (close_reason & 0x07) << 5],
            channel: HrpdForwardChannel::AsynchronousControl,
            reliable_sequence: None,
            synchronous_control_cycle: None,
        }
    }

    pub fn hardware_id_request(
        uati: u32,
        target_ati: AccessTerminalIdentifier,
        transaction_id: u8,
    ) -> Self {
        Self {
            uati: Some(uati),
            target_ati,
            protocol_type: DEFAULT_ADDRESS_MANAGEMENT_PROTOCOL_TYPE,
            payload: vec![0x03, transaction_id],
            channel: HrpdForwardChannel::AsynchronousControl,
            reliable_sequence: None,
            synchronous_control_cycle: None,
        }
    }

    pub fn session_close(
        target_ati: AccessTerminalIdentifier,
        close_reason: u8,
        more_info: &[u8],
    ) -> Self {
        let mut payload = Vec::with_capacity(3 + more_info.len());
        payload.push(0x01);
        payload.push(close_reason);
        payload.push(more_info.len().min(usize::from(u8::MAX)) as u8);
        payload.extend_from_slice(&more_info[..more_info.len().min(usize::from(u8::MAX))]);
        Self {
            uati: None,
            target_ati,
            protocol_type: DEFAULT_SESSION_MANAGEMENT_PROTOCOL_TYPE,
            payload,
            channel: HrpdForwardChannel::AsynchronousControl,
            reliable_sequence: None,
            synchronous_control_cycle: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HrpdTrafficAssignmentRequest {
    /// Assigned session UATI. Session configuration and A9 identity state are
    /// keyed on this value, not on the receive-form traffic UATI.
    pub session_uati: u32,
    /// Receive-form traffic UATI used for reverse traffic long codes and A8.
    pub uati: u32,
    pub mac_index: u8,
    pub reverse_rate_limit_bps: u32,
    pub reverse_long_code_mask_i: u64,
    pub reverse_long_code_mask_q: u64,
    pub drc_lock: bool,
    /// Negotiated Physical Layer protocol subtype in use for this connection.
    /// This selects reverse physical-channel details such as subtype-2 ACK
    /// Walsh cover and data-channel FCS length after SessionConfiguration
    /// commits.
    #[serde(default)]
    pub physical_layer_subtype: u16,
    /// Negotiated Reverse Traffic Channel MAC protocol subtype in use for this
    /// connection. This selects the reverse MAC trailer semantics independently
    /// of the Physical Layer packet/FCS format.
    #[serde(default)]
    pub reverse_traffic_mac_subtype: u16,
    /// TCA FrameOffset in slots. Reverse traffic physical-layer frames start
    /// where `(T - FrameOffset) mod 16 = 0`.
    pub frame_offset: u8,
    /// AT's DRC inner Walsh cover index (per the first pilot's
    /// `drc_cover` in the TCA, C.S0024-0 v4.0 §6.7.4.3). Selects the
    /// W_{drc_cover}^8 inner cover for reverse DRC demod.
    pub drc_cover: u8,
    /// AT's DRC integration length in 1.667 ms slots (TCA `drc_length`,
    /// §6.7.4.3). 1, 2, 4, or 8 are the supported values.
    pub drc_length: u8,
}

/// Stops the BTS reverse traffic receiver and removes the forward MAC
/// channel for a traffic assignment whose session closed or was replaced.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct HrpdTrafficReleaseRequest {
    pub uati: u32,
    pub mac_index: u8,
}

/// Build the Rev 0 Default Reverse Traffic Channel I/Q long-code masks from
/// the ATILCM value. C.S0024-300 §1.10.6.1.2 defines the I-mask as ten leading
/// ones followed by the cdma2000 ESN permutation applied to ATILCM.
pub fn default_reverse_traffic_long_code_masks(atilcm: u32) -> (u64, u64) {
    let i_mask = (0x03ff_u64 << 32) | u64::from(LongCodeGenerator::permute_esn(atilcm));
    let q_mask = derive_hrpd_q_mask(i_mask);
    (i_mask, q_mask)
}

fn derive_hrpd_q_mask(i_mask: u64) -> u64 {
    const XOR_TAPS: [u32; 20] = [
        0, 1, 2, 4, 5, 6, 9, 15, 16, 17, 18, 20, 21, 24, 25, 26, 30, 32, 34, 41,
    ];
    let mut q0: u64 = 0;
    for tap in XOR_TAPS {
        q0 ^= (i_mask >> tap) & 1;
    }
    let shifted = (i_mask & ((1u64 << 41) - 1)) << 1;
    (shifted & !1u64) | (q0 & 1)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HrpdForwardTrafficPacket {
    pub uati: u32,
    pub mac_index: u8,
    #[serde(default)]
    pub physical_layer_subtype: u16,
    #[serde(default)]
    pub forward_traffic_mac_subtype: u16,
    /// One unpacked physical-layer bit per byte, including MAC payload, FCS,
    /// and tail. The scheduler binds this packet to the AT's governing DRC at
    /// transmit time and rebuilds recognized payloads if the DRC packet size
    /// differs from this queued representation.
    pub payload_bits: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum HrpdTrafficEvent {
    ReversePilot {
        uati: u32,
        mac_index: u8,
        absolute_chip: u64,
        snr_db_tenths: i16,
    },
    ReversePilotLost {
        uati: u32,
        mac_index: u8,
        last_good_chip: u64,
        lost_at_chip: u64,
        lost_chips: u64,
        last_snr_db_tenths: i16,
        last_coherence_x1000: u16,
    },
    Drc {
        uati: u32,
        mac_index: u8,
        slot: u64,
        drc_index: u8,
    },
    Ack {
        uati: u32,
        mac_index: u8,
        slot: u64,
        ack: bool,
    },
    Stream0Signaling {
        uati: u32,
        payload: Vec<u8>,
    },
    Stream1Packet {
        uati: u32,
        /// Default Packet RLP sequence number of the first payload octet.
        sequence: u32,
        payload: Vec<u8>,
        /// Local diagnostic timestamp; internal serialization intentionally omits it.
        #[serde(skip)]
        decoded_at: Option<Instant>,
        /// Estimated host arrival of the final air sample used by this decode.
        #[serde(skip)]
        air_frame_end_received_at: Option<Instant>,
    },
}

fn twos_complement_6(value: i8) -> u8 {
    (i16::from(value) & 0x3f) as u8
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_indication_json_round_trips() {
        let indication = HrpdAccessIndication {
            absolute_chip: 1234,
            color_code: 26,
            sector_pilot_pn: 0,
            session_configuration_token: 0x0200,
            ati: AccessTerminalIdentifier {
                ati_type: AccessTerminalIdentifierType::Rati,
                value: 0x5232_af53,
            },
            security_layer_format: false,
            connection_layer_format: true,
            security_payload: vec![1, 2, 3],
            messages: vec![
                HrpdAccessMessage::RouteUpdate(HrpdRouteUpdate {
                    message_sequence: 0,
                    reference_pilot_pn: 0,
                    reference_pilot_strength: 0,
                    reference_keep: true,
                    num_pilots: 0,
                    at_total_pilot_transmission: None,
                    reference_pilot_channel: None,
                    reserved_zero: true,
                }),
                HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
                    transaction_id: 0x9c,
                }),
            ],
        };

        let json = serde_json::to_string(&indication).unwrap();
        let decoded: HrpdAccessIndication = serde_json::from_str(&json).unwrap();
        assert_eq!(decoded, indication);
    }

    #[test]
    fn uati_assignment_encodes_minimal_no_subnet_form() {
        let msg = HrpdUatiAssignment::from_uati032(7, 26, 0x8005_8001);
        assert_eq!(
            msg.encode(),
            vec![0x01, 0x07, 0x00, 0x1a, 0x05, 0x80, 0x01, 0x00]
        );
    }

    #[test]
    fn traffic_channel_assignment_encodes_single_pilot() {
        let msg = HrpdTrafficChannelAssignment::single_pilot(
            3,
            Some(HrpdChannelRecord {
                system_type: 0,
                band_class: 0,
                channel_number: 630,
            }),
            0,
            5,
        );
        let bytes = msg.encode();
        assert_eq!(bytes[0], HrpdTrafficChannelAssignment::MESSAGE_ID);
        assert_eq!(bytes[1], 3);
        assert_eq!(bytes.len(), 11);
        // Encodes DRCLength=8 slots, DRCChannelGain=+6 dB,
        // AckChannelGain=0 dB, and DRCCover=1 (sector cover; 0 is null).
        assert_eq!(
            bytes,
            vec![
                0x01, 0x03, 0x80, 0x01, 0x3b, 0x06, 0x60, 0x02, 0x00, 0x0a, 0x40
            ]
        );
        assert_eq!(
            msg.encode_subtype0_route_update_with_rev_a_tail(),
            vec![
                0x01, 0x03, 0x80, 0x01, 0x3b, 0x06, 0x60, 0x02, 0x00, 0x0a, 0x41, 0x20, 0x00
            ]
        );
        assert_eq!(msg.encode_subtype1_route_update().len(), 21);
    }

    #[test]
    fn traffic_channel_assignment_request_uses_negotiated_route_update_subtype() {
        let msg = HrpdTrafficChannelAssignment::single_pilot(
            0,
            Some(HrpdChannelRecord {
                system_type: 0,
                band_class: 0,
                channel_number: 630,
            }),
            0,
            6,
        );
        let target_ati = AccessTerminalIdentifier {
            ati_type: AccessTerminalIdentifierType::Uati,
            value: 0x1a05_8001,
        };

        let default_request = HrpdForwardSignalingRequest::traffic_channel_assignment(
            0x8005_8001,
            target_ati,
            msg.clone(),
        );
        assert_eq!(default_request.payload, msg.encode_subtype0_route_update());
        assert_eq!(default_request.payload.len(), 11);

        let subtype1_request =
            HrpdForwardSignalingRequest::traffic_channel_assignment_for_route_update_subtype(
                0x8005_8001,
                target_ati,
                msg.clone(),
                1,
            );
        assert_eq!(subtype1_request.payload, msg.encode_subtype1_route_update());
        assert_eq!(subtype1_request.payload.len(), 21);
        assert_ne!(subtype1_request.payload, default_request.payload);

        let default_tail_request =
            HrpdForwardSignalingRequest::traffic_channel_assignment_for_route_update_subtype_with_rev_a_tail(
                0x8005_8001,
                target_ati,
                msg.clone(),
                0,
                true,
            );
        assert_eq!(
            default_tail_request.payload,
            msg.encode_subtype0_route_update_with_rev_a_tail()
        );
        assert_eq!(default_tail_request.payload.len(), 13);
        assert_ne!(default_tail_request.payload, default_request.payload);
        assert_ne!(default_tail_request.payload, subtype1_request.payload);
    }

    #[test]
    fn default_reverse_traffic_long_code_mask_uses_traffic_prefix_and_ati_permutation() {
        let ati = 0x1a05_8001;
        let (i_mask, q_mask) = default_reverse_traffic_long_code_masks(ati);
        assert_eq!(i_mask >> 32, 0x03ff);
        assert_eq!(
            (i_mask & 0xffff_ffff) as u32,
            LongCodeGenerator::permute_esn(ati)
        );
        assert_eq!(q_mask >> 42, 0);
        assert_ne!(q_mask, i_mask);
    }

    #[test]
    fn default_signaling_packet_prefixes_stream_slp_and_snp_headers() {
        let payload = HrpdUatiAssignment::from_uati032(1, 26, 0x8005_8001).encode();
        let packet =
            encode_default_signaling_packet(DEFAULT_ADDRESS_MANAGEMENT_PROTOCOL_TYPE, &payload);
        assert_eq!(packet[0], 0x00);
        assert_eq!(packet[1], DEFAULT_ADDRESS_MANAGEMENT_PROTOCOL_TYPE);
        assert_eq!(&packet[2..], payload.as_slice());
    }

    #[test]
    fn hardware_id_request_is_address_management_message() {
        let request = HrpdForwardSignalingRequest::hardware_id_request(
            0x8005_8001,
            AccessTerminalIdentifier {
                ati_type: AccessTerminalIdentifierType::Uati,
                value: 0x1a05_8001,
            },
            0x42,
        );
        assert_eq!(request.uati, Some(0x8005_8001));
        assert_eq!(
            request.protocol_type,
            DEFAULT_ADDRESS_MANAGEMENT_PROTOCOL_TYPE
        );
        assert_eq!(request.payload, vec![0x03, 0x42]);
        assert_eq!(request.channel, HrpdForwardChannel::AsynchronousControl);
    }

    #[test]
    fn idle_state_page_is_minimal_best_effort_message() {
        let target_ati = AccessTerminalIdentifier {
            ati_type: AccessTerminalIdentifierType::Uati,
            value: 0x1a05_8001,
        };
        let request = HrpdForwardSignalingRequest::idle_state_page(0x1a05_8001, target_ati);
        assert_eq!(request.uati, Some(0x1a05_8001));
        assert_eq!(request.target_ati, target_ati);
        assert_eq!(request.protocol_type, DEFAULT_IDLE_STATE_PROTOCOL_TYPE);
        assert_eq!(request.payload, vec![0x00]);
        assert_eq!(request.channel, HrpdForwardChannel::SynchronousControl);
        assert_eq!(request.reliable_sequence, None);
    }
}
