//! Stream 0 (Default Signaling / Session Configuration) SLP packet parsing
//! and message-name helpers, split out of the air module.

use super::*;

#[cfg(test)]
pub(super) fn parse_stream0_default_signaling(
    session_packet: &[u8],
) -> Option<ParsedStream0Signaling> {
    match decode_stream0_slp_f_packet(session_packet)? {
        Stream0SlpFPacket::Complete(slp_d_bits) => parse_stream0_slp_d_bits(&slp_d_bits),
        Stream0SlpFPacket::Fragment { .. } => None,
    }
}

pub(super) fn decode_stream0_slp_f_packet(session_packet: &[u8]) -> Option<Stream0SlpFPacket> {
    let bits = bytes_to_bits_msb(session_packet);
    let mut cursor = 0usize;
    let stream = read_bits_msb(&bits, &mut cursor, 2)? as u8;
    if stream != 0 {
        return None;
    }
    let _reserved = read_bits_msb(&bits, &mut cursor, 4)?;
    let fragmented = read_bits_msb(&bits, &mut cursor, 1)?;
    if fragmented == 0 {
        return Some(Stream0SlpFPacket::Complete(bits[cursor..].to_vec()));
    }
    let begin = read_bits_msb(&bits, &mut cursor, 1)? != 0;
    let end = read_bits_msb(&bits, &mut cursor, 1)? != 0;
    let sequence = read_bits_msb(&bits, &mut cursor, 6)? as u8;
    if !begin {
        let _octet_alignment_pad = read_bits_msb(&bits, &mut cursor, 1)?;
    }
    Some(Stream0SlpFPacket::Fragment {
        begin,
        end,
        sequence,
        payload_bits: bits[cursor..].to_vec(),
    })
}

pub(super) fn parse_stream0_slp_d_bits(bits: &[u8]) -> Option<ParsedStream0Signaling> {
    let mut cursor = 0usize;
    let full_slp_d_header = read_bits_msb(&bits, &mut cursor, 1)?;
    let mut ack_sequence_number = None;
    let mut sequence_number = None;
    if full_slp_d_header != 0 {
        let ack_sequence_valid = read_bits_msb(&bits, &mut cursor, 1)?;
        let ack_sequence = read_bits_msb(&bits, &mut cursor, 3)? as u8;
        if ack_sequence_valid != 0 {
            ack_sequence_number = Some(ack_sequence);
        }
        let sequence_valid = read_bits_msb(&bits, &mut cursor, 1)?;
        let sequence = read_bits_msb(&bits, &mut cursor, 3)? as u8;
        if sequence_valid != 0 {
            sequence_number = Some(sequence);
        }
    }
    if bits.len() == cursor {
        return Some(ParsedStream0Signaling {
            message: None,
            ack_sequence_number,
            sequence_number,
            in_configuration: None,
        });
    }
    if bits.len() - cursor == 16 {
        let slp_payload = pack_bits_msb(&bits[cursor..]);
        let message_id = slp_payload.first().copied();
        if slp_payload.len() >= 2 && message_id == Some(DEFAULT_SIGNALING_SLP_RESET) {
            return Some(ParsedStream0Signaling {
                message: Some(HrpdAccessMessage::DefaultSignalingReset(
                    HrpdDefaultSignalingReset {
                        message_sequence: slp_payload[1],
                    },
                )),
                ack_sequence_number,
                sequence_number,
                in_configuration: None,
            });
        }
        if slp_payload.len() >= 2 && message_id == Some(DEFAULT_SIGNALING_SLP_RESET_ACK) {
            return Some(ParsedStream0Signaling {
                message: Some(HrpdAccessMessage::DefaultSignalingResetAck(
                    HrpdDefaultSignalingResetAck {
                        message_sequence: slp_payload[1],
                    },
                )),
                ack_sequence_number,
                sequence_number,
                in_configuration: None,
            });
        }
        if matches!(
            message_id,
            Some(DEFAULT_SIGNALING_SLP_RESET | DEFAULT_SIGNALING_SLP_RESET_ACK)
        ) {
            return Some(ParsedStream0Signaling {
                message: Some(HrpdAccessMessage::Unknown {
                    protocol_type: DEFAULT_STREAM0_APPLICATION_PROTOCOL_TYPE,
                    message_id,
                    payload: slp_payload,
                }),
                ack_sequence_number,
                sequence_number,
                in_configuration: None,
            });
        }
    }
    let in_configuration = read_bits_msb(&bits, &mut cursor, 1)? != 0;
    let protocol_type = read_bits_msb(&bits, &mut cursor, 7)? as u8;
    if cursor > bits.len() || (bits.len() - cursor) % 8 != 0 {
        return None;
    }
    let payload = pack_bits_msb(&bits[cursor..]);
    let message_id = payload.first().copied();
    let message = match (protocol_type, message_id) {
        (DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE, Some(ROUTE_UPDATE_MESSAGE_ID)) => {
            parse_stream0_route_update(&payload)
                .map(HrpdAccessMessage::RouteUpdate)
                .unwrap_or_else(|| HrpdAccessMessage::Unknown {
                    protocol_type,
                    message_id,
                    payload: payload.clone(),
                })
        }
        (
            DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE | DEFAULT_CONNECTED_STATE_PROTOCOL_TYPE,
            Some(TRAFFIC_CHANNEL_COMPLETE_MESSAGE_ID),
        ) if payload.len() >= 2 => {
            HrpdAccessMessage::TrafficChannelComplete(HrpdTrafficChannelComplete {
                message_sequence: payload[1],
            })
        }
        (DEFAULT_SESSION_MANAGEMENT_PROTOCOL_TYPE, Some(SESSION_CLOSE_MESSAGE_ID)) => {
            parse_stream0_session_close(&payload)
                .map(HrpdAccessMessage::SessionClose)
                .unwrap_or_else(|| HrpdAccessMessage::Unknown {
                    protocol_type,
                    message_id,
                    payload: payload.clone(),
                })
        }
        (DEFAULT_CONNECTED_STATE_PROTOCOL_TYPE, Some(CONNECTED_STATE_CONNECTION_CLOSE)) => {
            parse_stream0_connection_close(&payload)
                .map(HrpdAccessMessage::ConnectionClose)
                .unwrap_or_else(|| HrpdAccessMessage::Unknown {
                    protocol_type,
                    message_id,
                    payload: payload.clone(),
                })
        }
        (
            DEFAULT_SESSION_MANAGEMENT_PROTOCOL_TYPE,
            Some(SESSION_KEEP_ALIVE_REQUEST_MESSAGE_ID | SESSION_KEEP_ALIVE_RESPONSE_MESSAGE_ID),
        ) => HrpdAccessMessage::KeepAlive,
        (DEFAULT_ADDRESS_MANAGEMENT_PROTOCOL_TYPE, Some(HARDWARE_ID_RESPONSE_MESSAGE_ID)) => {
            parse_stream0_hardware_id_response(&payload)
                .map(HrpdAccessMessage::HardwareIdResponse)
                .unwrap_or_else(|| HrpdAccessMessage::Unknown {
                    protocol_type,
                    message_id,
                    payload: payload.clone(),
                })
        }
        (protocol_type, Some(DEFAULT_PACKET_XON_REQUEST))
            if is_default_packet_application_protocol_type(protocol_type) =>
        {
            HrpdAccessMessage::DefaultPacketXonRequest
        }
        (protocol_type, Some(DEFAULT_PACKET_RLP_RESET))
            if is_default_packet_application_protocol_type(protocol_type) =>
        {
            HrpdAccessMessage::DefaultPacketRlpReset(HrpdDefaultPacketRlpReset)
        }
        (protocol_type, Some(DEFAULT_PACKET_RLP_RESET_ACK))
            if is_default_packet_application_protocol_type(protocol_type) =>
        {
            HrpdAccessMessage::DefaultPacketRlpResetAck(HrpdDefaultPacketRlpResetAck)
        }
        (protocol_type, Some(DEFAULT_PACKET_RLP_NAK))
            if is_default_packet_application_protocol_type(protocol_type) =>
        {
            parse_default_packet_rlp_nak(&payload)
                .map(HrpdAccessMessage::DefaultPacketRlpNak)
                .unwrap_or_else(|| HrpdAccessMessage::Unknown {
                    protocol_type,
                    message_id,
                    payload: payload.clone(),
                })
        }
        (protocol_type, Some(DEFAULT_PACKET_XOFF_REQUEST))
            if is_default_packet_application_protocol_type(protocol_type) =>
        {
            HrpdAccessMessage::DefaultPacketXoffRequest
        }
        (protocol_type, Some(DEFAULT_PACKET_DATA_READY_ACK))
            if is_default_packet_application_protocol_type(protocol_type) && payload.len() >= 2 =>
        {
            HrpdAccessMessage::DefaultPacketDataReadyAck(
                cdma_common::hrpd::air::HrpdDefaultPacketDataReadyAck {
                    transaction_id: payload[1],
                },
            )
        }
        _ => HrpdAccessMessage::Unknown {
            protocol_type,
            message_id,
            payload,
        },
    };
    Some(ParsedStream0Signaling {
        message: Some(message),
        ack_sequence_number,
        sequence_number,
        in_configuration: Some(in_configuration),
    })
}

pub(super) fn parse_stream0_route_update(payload: &[u8]) -> Option<AirRouteUpdate> {
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
    let at_total_included = read_bits_msb(&bits, &mut cursor, 1)? != 0;
    let at_total_pilot_transmission = if at_total_included {
        Some(read_bits_msb(&bits, &mut cursor, 8)? as u8 as i8)
    } else {
        None
    };
    let reference_channel_included = read_bits_msb(&bits, &mut cursor, 1)? != 0;
    let reference_pilot_channel = if reference_channel_included {
        Some(read_bits_msb(&bits, &mut cursor, 24)?)
    } else {
        None
    };
    let reserved_zero = bits[cursor..].iter().all(|&bit| bit == 0);
    Some(AirRouteUpdate {
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

pub(super) fn parse_stream0_session_close(payload: &[u8]) -> Option<HrpdSessionClose> {
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

pub(super) fn parse_stream0_connection_close(payload: &[u8]) -> Option<HrpdConnectionClose> {
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

pub(super) fn parse_stream0_hardware_id_response(payload: &[u8]) -> Option<HrpdHardwareIdResponse> {
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

pub(super) fn default_packet_rlp_nak_payload(
    requests: &[cdma_common::hrpd::air::HrpdDefaultPacketRlpNakRequest],
) -> Vec<u8> {
    let request_count = requests.len().min(u8::MAX as usize);
    let mut payload = Vec::with_capacity(2 + request_count * 5);
    payload.push(DEFAULT_PACKET_RLP_NAK);
    payload.push(request_count as u8);
    for request in requests.iter().take(request_count) {
        let first_erased = request.first_erased & rlp::SEQUENCE_MASK;
        // Reserved(2)=0 followed by the 22-bit FirstErased field is exactly
        // three network-order octets.
        payload.push(((first_erased >> 16) & 0x3f) as u8);
        payload.push((first_erased >> 8) as u8);
        payload.push(first_erased as u8);
        payload.extend_from_slice(&request.window_len.to_be_bytes());
    }
    payload
}

pub(super) fn parse_default_packet_rlp_nak(
    payload: &[u8],
) -> Option<cdma_common::hrpd::air::HrpdDefaultPacketRlpNak> {
    if payload.len() < 2 || payload[0] != DEFAULT_PACKET_RLP_NAK {
        return None;
    }
    let bits = bytes_to_bits_msb(payload);
    let mut cursor = 0usize;
    let message_id = read_bits_msb(&bits, &mut cursor, 8)? as u8;
    if message_id != DEFAULT_PACKET_RLP_NAK {
        return None;
    }
    let request_count = read_bits_msb(&bits, &mut cursor, 8)? as usize;
    let required_bits = 16usize.checked_add(request_count.checked_mul(40)?)?;
    if bits.len() < required_bits {
        return None;
    }
    let mut requests = Vec::with_capacity(request_count);
    for _ in 0..request_count {
        let _reserved = read_bits_msb(&bits, &mut cursor, 2)?;
        let first_erased = read_bits_msb(&bits, &mut cursor, 22)?;
        let window_len = read_bits_msb(&bits, &mut cursor, 16)? as u16;
        requests.push(cdma_common::hrpd::air::HrpdDefaultPacketRlpNakRequest {
            first_erased,
            window_len,
        });
    }
    Some(cdma_common::hrpd::air::HrpdDefaultPacketRlpNak { requests })
}

pub(super) fn stream0_message_name(protocol_type: u8, message_id: Option<u8>) -> &'static str {
    match (protocol_type, message_id) {
        (_, Some(0x50)) => "ConfigurationRequest",
        (_, Some(0x51)) => "ConfigurationResponse",
        (DEFAULT_CONNECTED_STATE_PROTOCOL_TYPE, Some(0x00)) => "ConnectionClose",
        (DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE, Some(0x00)) => "RouteUpdate",
        (
            DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE | DEFAULT_CONNECTED_STATE_PROTOCOL_TYPE,
            Some(0x02),
        ) => "TrafficChannelComplete",
        (DEFAULT_SESSION_MANAGEMENT_PROTOCOL_TYPE, Some(0x01)) => "SessionClose",
        (DEFAULT_SESSION_MANAGEMENT_PROTOCOL_TYPE, Some(0x02)) => "KeepAliveRequest",
        (DEFAULT_SESSION_MANAGEMENT_PROTOCOL_TYPE, Some(0x03)) => "KeepAliveResponse",
        (protocol_type, Some(DEFAULT_PACKET_XON_REQUEST))
            if is_default_packet_application_protocol_type(protocol_type) =>
        {
            "DefaultPacketXonRequest"
        }
        (protocol_type, Some(DEFAULT_PACKET_RLP_RESET))
            if is_default_packet_application_protocol_type(protocol_type) =>
        {
            "DefaultPacketRlpReset"
        }
        (protocol_type, Some(DEFAULT_PACKET_RLP_RESET_ACK))
            if is_default_packet_application_protocol_type(protocol_type) =>
        {
            "DefaultPacketRlpResetAck"
        }
        (protocol_type, Some(DEFAULT_PACKET_RLP_NAK))
            if is_default_packet_application_protocol_type(protocol_type) =>
        {
            "DefaultPacketRlpNak"
        }
        (protocol_type, Some(DEFAULT_PACKET_XON_RESPONSE))
            if is_default_packet_application_protocol_type(protocol_type) =>
        {
            "DefaultPacketXonResponse"
        }
        (protocol_type, Some(DEFAULT_PACKET_XOFF_REQUEST))
            if is_default_packet_application_protocol_type(protocol_type) =>
        {
            "DefaultPacketXoffRequest"
        }
        (protocol_type, Some(DEFAULT_PACKET_XOFF_RESPONSE))
            if is_default_packet_application_protocol_type(protocol_type) =>
        {
            "DefaultPacketXoffResponse"
        }
        (protocol_type, Some(DEFAULT_PACKET_DATA_READY))
            if is_default_packet_application_protocol_type(protocol_type) =>
        {
            "DefaultPacketDataReady"
        }
        (protocol_type, Some(DEFAULT_PACKET_DATA_READY_ACK))
            if is_default_packet_application_protocol_type(protocol_type) =>
        {
            "DefaultPacketDataReadyAck"
        }
        (DEFAULT_SESSION_CONFIGURATION_PROTOCOL_TYPE, _) => {
            stream0_session_configuration_message_name(message_id)
        }
        (DEFAULT_STREAM0_APPLICATION_PROTOCOL_TYPE, _) => {
            stream0_default_signaling_message_name(message_id)
        }
        (DEFAULT_ADDRESS_MANAGEMENT_PROTOCOL_TYPE, Some(HARDWARE_ID_RESPONSE_MESSAGE_ID)) => {
            "HardwareIdResponse"
        }
        _ => "Unhandled",
    }
}

pub(super) fn is_default_packet_application_protocol_type(protocol_type: u8) -> bool {
    matches!(
        protocol_type,
        DEFAULT_PACKET_STREAM1_APPLICATION_PROTOCOL_TYPE
            | DEFAULT_PACKET_STREAM2_APPLICATION_PROTOCOL_TYPE
            | DEFAULT_PACKET_STREAM3_APPLICATION_PROTOCOL_TYPE
    )
}

pub(super) fn stream0_protocol_name(protocol_type: u8) -> &'static str {
    match protocol_type {
        SESSION_PROTOCOL_PHYSICAL_LAYER => "PhysicalLayer",
        SESSION_PROTOCOL_CONTROL_CHANNEL_MAC => "ControlChannelMAC",
        SESSION_PROTOCOL_ACCESS_CHANNEL_MAC => "AccessChannelMAC",
        SESSION_PROTOCOL_FORWARD_TRAFFIC_CHANNEL_MAC => "ForwardTrafficChannelMAC",
        SESSION_PROTOCOL_REVERSE_TRAFFIC_CHANNEL_MAC => "ReverseTrafficChannelMAC",
        SESSION_PROTOCOL_KEY_EXCHANGE => "KeyExchange",
        SESSION_PROTOCOL_AUTHENTICATION => "Authentication",
        SESSION_PROTOCOL_ENCRYPTION => "Encryption",
        SESSION_PROTOCOL_SECURITY => "Security",
        0x09 => "PacketConsolidation",
        SESSION_PROTOCOL_AIR_LINK_MANAGEMENT => "AirLinkManagement",
        SESSION_PROTOCOL_INITIALIZATION_STATE => "InitializationState",
        DEFAULT_IDLE_STATE_PROTOCOL_TYPE => "IdleState",
        DEFAULT_CONNECTED_STATE_PROTOCOL_TYPE => "ConnectedState",
        DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE => "RouteUpdate",
        SESSION_PROTOCOL_OVERHEAD_MESSAGES => "OverheadMessages",
        DEFAULT_SESSION_MANAGEMENT_PROTOCOL_TYPE => "SessionManagement",
        DEFAULT_ADDRESS_MANAGEMENT_PROTOCOL_TYPE => "AddressManagement",
        DEFAULT_SESSION_CONFIGURATION_PROTOCOL_TYPE => "SessionConfiguration",
        SESSION_PROTOCOL_STREAM => "Stream",
        DEFAULT_STREAM0_APPLICATION_PROTOCOL_TYPE => "DefaultSignaling",
        SESSION_PROTOCOL_MULTIMODE_CAPABILITY_DISCOVERY => "MultimodeCapabilityDiscovery",
        SESSION_PROTOCOL_DEFAULT_PACKET_FIRST..=SESSION_PROTOCOL_DEFAULT_PACKET_LAST => {
            "DefaultPacket"
        }
        _ => "Protocol",
    }
}

pub(super) fn stream0_default_signaling_message_name(message_id: Option<u8>) -> &'static str {
    match message_id {
        Some(0x00) => "Reset",
        Some(0x01) => "ResetAck",
        Some(0x50) => "ConfigurationRequest",
        Some(0x51) => "ConfigurationResponse",
        _ => "Unhandled",
    }
}

pub(super) fn stream0_session_configuration_message_name(message_id: Option<u8>) -> &'static str {
    match message_id {
        Some(SESSION_CONFIGURATION_COMPLETE) => "ConfigurationComplete",
        Some(SESSION_CONFIGURATION_START) => "ConfigurationStart",
        Some(SESSION_SOFT_CONFIGURATION_COMPLETE) => "SoftConfigurationComplete",
        Some(SESSION_CONFIGURATION_REQUEST) => "ConfigurationRequest",
        Some(SESSION_CONFIGURATION_RESPONSE) => "ConfigurationResponse",
        _ => "Unhandled",
    }
}
