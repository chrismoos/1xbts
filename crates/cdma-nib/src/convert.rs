//! Conversions between decoded HRPD air-interface types and the AN gRPC
//! proto. Lives in the library so the nib binary and tests can share them.

use cdma_an::HrpdAnForwardTrafficPacket;
use cdma_an::proto::an::v1 as an_proto;
use cdma_common::hrpd::air as hrpd_air;

const HRPD_DEFAULT_STREAM0_APPLICATION_PROTOCOL_TYPE: u8 = 0x14;
/// Re-exported to the nib binary through the module glob.
pub const HRPD_DEFAULT_PACKET_STREAM2_PROTOCOL_TYPE: u8 = 0x16;
const HRPD_DEFAULT_PACKET_RLP_RESET: u8 = 0x00;
const HRPD_DEFAULT_PACKET_RLP_RESET_ACK: u8 = 0x01;
const HRPD_DEFAULT_PACKET_RLP_NAK: u8 = 0x02;
const HRPD_A8_RLP_SEQUENCE_MODULUS: u32 =
    1u32 << cdma_common::hrpd::traffic::DEFAULT_PACKET_RLP_SEQUENCE_BITS;
const HRPD_A8_RLP_SEQUENCE_MASK: u32 = HRPD_A8_RLP_SEQUENCE_MODULUS - 1;

pub fn ati_to_proto(ati: hrpd_air::AccessTerminalIdentifier) -> an_proto::AccessTerminalIdentifier {
    an_proto::AccessTerminalIdentifier {
        ati_type: match ati.ati_type {
            hrpd_air::AccessTerminalIdentifierType::Bati => {
                an_proto::AccessTerminalIdentifierType::Bati as i32
            }
            hrpd_air::AccessTerminalIdentifierType::Reserved => {
                an_proto::AccessTerminalIdentifierType::Reserved as i32
            }
            hrpd_air::AccessTerminalIdentifierType::Uati => {
                an_proto::AccessTerminalIdentifierType::Uati as i32
            }
            hrpd_air::AccessTerminalIdentifierType::Rati => {
                an_proto::AccessTerminalIdentifierType::Rati as i32
            }
        },
        value: ati.value,
    }
}

pub fn ati_from_proto(
    ati: Option<an_proto::AccessTerminalIdentifier>,
) -> Result<hrpd_air::AccessTerminalIdentifier, String> {
    let ati = ati.ok_or_else(|| "missing target ATI".to_string())?;
    let ati_type = an_proto::AccessTerminalIdentifierType::try_from(ati.ati_type)
        .map_err(|_| "invalid target ATI type".to_string())?;
    let ati_type = match ati_type {
        an_proto::AccessTerminalIdentifierType::Bati => {
            hrpd_air::AccessTerminalIdentifierType::Bati
        }
        an_proto::AccessTerminalIdentifierType::Reserved => {
            hrpd_air::AccessTerminalIdentifierType::Reserved
        }
        an_proto::AccessTerminalIdentifierType::Uati => {
            hrpd_air::AccessTerminalIdentifierType::Uati
        }
        an_proto::AccessTerminalIdentifierType::Rati => {
            hrpd_air::AccessTerminalIdentifierType::Rati
        }
        an_proto::AccessTerminalIdentifierType::Unspecified => {
            return Err("unspecified target ATI type".to_string());
        }
    };
    Ok(hrpd_air::AccessTerminalIdentifier {
        ati_type,
        value: ati.value,
    })
}

pub fn access_message_to_proto(
    message: hrpd_air::HrpdAccessMessage,
) -> an_proto::HrpdAccessMessage {
    use an_proto::hrpd_access_message::Message;
    let message = match message {
        hrpd_air::HrpdAccessMessage::RouteUpdate(route) => {
            Message::RouteUpdate(an_proto::HrpdRouteUpdate {
                message_sequence: u32::from(route.message_sequence),
                reference_pilot_pn: u32::from(route.reference_pilot_pn),
                reference_pilot_strength: u32::from(route.reference_pilot_strength),
                reference_keep: route.reference_keep,
                num_pilots: u32::from(route.num_pilots),
                at_total_pilot_transmission: route.at_total_pilot_transmission.map(i32::from),
                reference_pilot_channel: route.reference_pilot_channel,
                reserved_zero: route.reserved_zero,
            })
        }
        hrpd_air::HrpdAccessMessage::UatiRequest(uati) => {
            Message::UatiRequest(an_proto::HrpdUatiRequest {
                transaction_id: u32::from(uati.transaction_id),
            })
        }
        hrpd_air::HrpdAccessMessage::UatiComplete(uati) => {
            Message::UatiComplete(an_proto::HrpdUatiComplete {
                message_sequence: u32::from(uati.message_sequence),
                upper_old_uati: uati.upper_old_uati,
                reserved_zero: uati.reserved_zero,
            })
        }
        hrpd_air::HrpdAccessMessage::ConnectionRequest(connection) => {
            Message::ConnectionRequest(an_proto::HrpdConnectionRequest {
                transaction_id: u32::from(connection.transaction_id),
                request_reason: u32::from(connection.request_reason),
                reserved_zero: connection.reserved_zero,
            })
        }
        hrpd_air::HrpdAccessMessage::TrafficChannelComplete(complete) => {
            Message::TrafficChannelComplete(an_proto::HrpdTrafficChannelComplete {
                message_sequence: u32::from(complete.message_sequence),
            })
        }
        hrpd_air::HrpdAccessMessage::SessionClose(close) => {
            Message::SessionClose(an_proto::HrpdSessionClose {
                close_reason: i32::from(close.close_reason),
                more_info: close.more_info,
            })
        }
        hrpd_air::HrpdAccessMessage::ConnectionClose(close) => {
            Message::ConnectionClose(an_proto::HrpdConnectionClose {
                close_reason: i32::from(close.close_reason),
                suspend_enable: close.suspend_enable,
                suspend_time: close.suspend_time,
                reserved_zero: close.reserved_zero,
            })
        }
        hrpd_air::HrpdAccessMessage::HardwareIdResponse(hardware) => {
            Message::HardwareIdResponse(an_proto::HrpdHardwareIdResponse {
                transaction_id: u32::from(hardware.transaction_id),
                hardware_id_type: hardware.hardware_id_type,
                hardware_id_value: hardware.hardware_id_value,
            })
        }
        hrpd_air::HrpdAccessMessage::KeepAlive => Message::KeepAlive(an_proto::HrpdKeepAlive {}),
        hrpd_air::HrpdAccessMessage::DefaultPacketXonRequest => {
            Message::DefaultPacketXonRequest(an_proto::HrpdDefaultPacketXonRequest {})
        }
        hrpd_air::HrpdAccessMessage::DefaultPacketXoffRequest => {
            Message::DefaultPacketXoffRequest(an_proto::HrpdDefaultPacketXoffRequest {})
        }
        hrpd_air::HrpdAccessMessage::DefaultPacketDataReadyAck(ack) => {
            Message::DefaultPacketDataReadyAck(an_proto::HrpdDefaultPacketDataReadyAck {
                transaction_id: u32::from(ack.transaction_id),
            })
        }
        hrpd_air::HrpdAccessMessage::DefaultPacketRlpReset(_) => {
            Message::Unknown(an_proto::HrpdUnknownAccessMessage {
                protocol_type: u32::from(HRPD_DEFAULT_PACKET_STREAM2_PROTOCOL_TYPE),
                message_id: Some(u32::from(HRPD_DEFAULT_PACKET_RLP_RESET)),
                payload: vec![HRPD_DEFAULT_PACKET_RLP_RESET],
            })
        }
        hrpd_air::HrpdAccessMessage::DefaultPacketRlpResetAck(_) => {
            Message::Unknown(an_proto::HrpdUnknownAccessMessage {
                protocol_type: u32::from(HRPD_DEFAULT_PACKET_STREAM2_PROTOCOL_TYPE),
                message_id: Some(u32::from(HRPD_DEFAULT_PACKET_RLP_RESET_ACK)),
                payload: vec![HRPD_DEFAULT_PACKET_RLP_RESET_ACK],
            })
        }
        hrpd_air::HrpdAccessMessage::DefaultPacketRlpNak(nak) => {
            let mut payload = vec![HRPD_DEFAULT_PACKET_RLP_NAK, nak.requests.len() as u8];
            for request in nak.requests {
                let mut value = u64::from(request.first_erased & HRPD_A8_RLP_SEQUENCE_MASK);
                value = (value << 16) | u64::from(request.window_len);
                payload.push(((value >> 32) & 0xff) as u8);
                payload.push(((value >> 24) & 0xff) as u8);
                payload.push(((value >> 16) & 0xff) as u8);
                payload.push(((value >> 8) & 0xff) as u8);
                payload.push((value & 0xff) as u8);
            }
            Message::Unknown(an_proto::HrpdUnknownAccessMessage {
                protocol_type: u32::from(HRPD_DEFAULT_PACKET_STREAM2_PROTOCOL_TYPE),
                message_id: Some(u32::from(HRPD_DEFAULT_PACKET_RLP_NAK)),
                payload,
            })
        }
        hrpd_air::HrpdAccessMessage::DefaultSignalingReset(reset) => {
            Message::Unknown(an_proto::HrpdUnknownAccessMessage {
                protocol_type: u32::from(HRPD_DEFAULT_STREAM0_APPLICATION_PROTOCOL_TYPE),
                message_id: Some(0),
                payload: vec![0, reset.message_sequence],
            })
        }
        hrpd_air::HrpdAccessMessage::DefaultSignalingResetAck(ack) => {
            Message::Unknown(an_proto::HrpdUnknownAccessMessage {
                protocol_type: u32::from(HRPD_DEFAULT_STREAM0_APPLICATION_PROTOCOL_TYPE),
                message_id: Some(1),
                payload: vec![1, ack.message_sequence],
            })
        }
        hrpd_air::HrpdAccessMessage::Unknown {
            protocol_type,
            message_id,
            payload,
        } => Message::Unknown(an_proto::HrpdUnknownAccessMessage {
            protocol_type: u32::from(protocol_type),
            message_id: message_id.map(u32::from),
            payload,
        }),
    };
    an_proto::HrpdAccessMessage {
        message: Some(message),
    }
}

pub fn access_indication_to_proto(
    indication: hrpd_air::HrpdAccessIndication,
) -> an_proto::HrpdAccessIndication {
    an_proto::HrpdAccessIndication {
        absolute_chip: indication.absolute_chip,
        color_code: u32::from(indication.color_code),
        sector_pilot_pn: u32::from(indication.sector_pilot_pn),
        session_configuration_token: u32::from(indication.session_configuration_token),
        ati: Some(ati_to_proto(indication.ati)),
        security_layer_format: indication.security_layer_format,
        connection_layer_format: indication.connection_layer_format,
        security_payload: indication.security_payload,
        messages: indication
            .messages
            .into_iter()
            .map(access_message_to_proto)
            .collect(),
    }
}

pub fn traffic_event_to_proto(event: hrpd_air::HrpdTrafficEvent) -> an_proto::HrpdTrafficEvent {
    use an_proto::hrpd_traffic_event::Event;
    let event = match event {
        hrpd_air::HrpdTrafficEvent::ReversePilot {
            uati,
            mac_index,
            absolute_chip,
            snr_db_tenths,
        } => Event::ReversePilot(an_proto::HrpdReversePilotEvent {
            uati,
            full_uati: None,
            receive_ati: uati,
            mac_index: u32::from(mac_index),
            absolute_chip,
            snr_db_tenths: i32::from(snr_db_tenths),
        }),
        hrpd_air::HrpdTrafficEvent::ReversePilotLost {
            uati,
            mac_index,
            last_good_chip,
            lost_at_chip,
            lost_chips,
            last_snr_db_tenths,
            last_coherence_x1000,
        } => Event::ReversePilotLost(an_proto::HrpdReversePilotLostEvent {
            uati,
            full_uati: None,
            receive_ati: uati,
            mac_index: u32::from(mac_index),
            last_good_chip,
            lost_at_chip,
            lost_chips,
            last_snr_db_tenths: i32::from(last_snr_db_tenths),
            last_coherence_x1000: u32::from(last_coherence_x1000),
        }),
        hrpd_air::HrpdTrafficEvent::Drc {
            uati,
            mac_index,
            slot,
            drc_index,
        } => Event::Drc(an_proto::HrpdDrcEvent {
            uati,
            full_uati: None,
            receive_ati: uati,
            mac_index: u32::from(mac_index),
            slot,
            drc_index: u32::from(drc_index),
        }),
        hrpd_air::HrpdTrafficEvent::Ack {
            uati,
            mac_index,
            slot,
            ack,
        } => Event::Ack(an_proto::HrpdAckEvent {
            uati,
            full_uati: None,
            receive_ati: uati,
            mac_index: u32::from(mac_index),
            slot,
            ack,
        }),
        hrpd_air::HrpdTrafficEvent::Stream0Signaling { uati, payload } => {
            Event::Stream0Signaling(an_proto::HrpdStream0SignalingEvent {
                uati,
                full_uati: None,
                receive_ati: uati,
                payload,
            })
        }
        hrpd_air::HrpdTrafficEvent::Stream1Packet {
            uati,
            sequence,
            payload,
            ..
        } => Event::Stream1Packet(an_proto::HrpdStream1PacketEvent {
            uati,
            full_uati: None,
            receive_ati: uati,
            payload,
            sequence,
        }),
    };
    an_proto::HrpdTrafficEvent { event: Some(event) }
}

pub fn forward_signaling_from_proto(
    request: an_proto::HrpdForwardSignalingRequest,
) -> Result<hrpd_air::HrpdForwardSignalingRequest, String> {
    let channel = an_proto::HrpdForwardChannel::try_from(request.channel)
        .map_err(|_| "invalid HRPD forward channel".to_string())?;
    let channel = match channel {
        an_proto::HrpdForwardChannel::SynchronousControl => {
            hrpd_air::HrpdForwardChannel::SynchronousControl
        }
        an_proto::HrpdForwardChannel::AsynchronousControl => {
            hrpd_air::HrpdForwardChannel::AsynchronousControl
        }
        an_proto::HrpdForwardChannel::ForwardTraffic => {
            hrpd_air::HrpdForwardChannel::ForwardTraffic
        }
        an_proto::HrpdForwardChannel::Unspecified => {
            return Err("unspecified HRPD forward channel".to_string());
        }
    };
    let protocol_type = u8::try_from(request.protocol_type)
        .map_err(|_| "forward signaling protocol_type exceeds u8".to_string())?;
    let synchronous_control_cycle = match (
        request.synchronous_control_cycle_modulus,
        request.synchronous_control_cycle_residue,
    ) {
        (Some(modulus), Some(residue)) => {
            let modulus = u16::try_from(modulus)
                .map_err(|_| "forward signaling synchronous modulus exceeds u16".to_string())?;
            let residue = u16::try_from(residue)
                .map_err(|_| "forward signaling synchronous residue exceeds u16".to_string())?;
            Some(hrpd_air::HrpdSynchronousControlCycle { modulus, residue })
        }
        (None, None) => None,
        _ => {
            return Err(
                "forward signaling synchronous control cycle requires modulus and residue"
                    .to_string(),
            );
        }
    };
    Ok(hrpd_air::HrpdForwardSignalingRequest {
        uati: request.uati,
        target_ati: ati_from_proto(request.target_ati)?,
        protocol_type,
        payload: request.payload,
        channel,
        reliable_sequence: request
            .reliable_sequence
            .map(|sequence| {
                u8::try_from(sequence)
                    .map(|sequence| sequence & 0x07)
                    .map_err(|_| "forward signaling reliable_sequence exceeds u8".to_string())
            })
            .transpose()?,
        synchronous_control_cycle,
    })
}

pub fn traffic_assignment_from_proto(
    request: an_proto::HrpdTrafficAssignmentRequest,
) -> Result<hrpd_air::HrpdTrafficAssignmentRequest, String> {
    let mac_index = u8::try_from(request.mac_index)
        .map_err(|_| "traffic assignment mac_index exceeds u8".to_string())?;
    let drc_cover = u8::try_from(request.drc_cover)
        .map_err(|_| "traffic assignment drc_cover exceeds u8".to_string())?;
    let drc_length = u8::try_from(request.drc_length)
        .map_err(|_| "traffic assignment drc_length exceeds u8".to_string())?;
    let frame_offset = u8::try_from(request.frame_offset)
        .map_err(|_| "traffic assignment frame_offset exceeds u8".to_string())?;
    let physical_layer_subtype = u16::try_from(request.physical_layer_subtype)
        .map_err(|_| "traffic assignment physical_layer_subtype exceeds u16".to_string())?;
    let reverse_traffic_mac_subtype = u16::try_from(request.reverse_traffic_mac_subtype)
        .map_err(|_| "traffic assignment reverse_traffic_mac_subtype exceeds u16".to_string())?;
    Ok(hrpd_air::HrpdTrafficAssignmentRequest {
        session_uati: request.session_uati,
        uati: request.uati,
        mac_index,
        reverse_rate_limit_bps: request.reverse_rate_limit_bps,
        reverse_long_code_mask_i: request.reverse_long_code_mask_i,
        reverse_long_code_mask_q: request.reverse_long_code_mask_q,
        drc_lock: request.drc_lock,
        physical_layer_subtype,
        reverse_traffic_mac_subtype,
        frame_offset: frame_offset & 0x0f,
        drc_cover,
        drc_length,
    })
}

pub fn forward_traffic_from_proto(
    packet: an_proto::HrpdForwardTrafficPacket,
) -> Result<cdma_bts::bts::hrpd::scheduler::ForwardTrafficPacket, String> {
    let mac_index = u8::try_from(packet.mac_index)
        .map_err(|_| "forward traffic mac_index exceeds u8".to_string())?;
    let physical_layer_subtype = u16::try_from(packet.physical_layer_subtype)
        .map_err(|_| "forward traffic physical_layer_subtype exceeds u16".to_string())?;
    let forward_traffic_mac_subtype = u16::try_from(packet.forward_traffic_mac_subtype)
        .map_err(|_| "forward traffic forward_traffic_mac_subtype exceeds u16".to_string())?;
    Ok(cdma_bts::bts::hrpd::scheduler::ForwardTrafficPacket {
        mac_index,
        physical_layer_subtype,
        forward_traffic_mac_subtype,
        high_priority: packet.high_priority,
        payload: packet.payload_bits,
    })
}

pub fn forward_traffic_from_an_packet(
    packet: HrpdAnForwardTrafficPacket,
) -> cdma_bts::bts::hrpd::scheduler::ForwardTrafficPacket {
    cdma_bts::bts::hrpd::scheduler::ForwardTrafficPacket {
        mac_index: packet.mac_index,
        physical_layer_subtype: packet.physical_layer_subtype,
        forward_traffic_mac_subtype: packet.forward_traffic_mac_subtype,
        high_priority: packet.high_priority,
        payload: packet.payload,
    }
}

pub fn hardware_id_response_from_proto(
    response: an_proto::HrpdHardwareIdResponse,
) -> Result<hrpd_air::HrpdHardwareIdResponse, String> {
    let transaction_id = u8::try_from(response.transaction_id)
        .map_err(|_| "hardware_id_response transaction_id exceeds u8".to_string())?;
    if response.hardware_id_type > 0x00ff_ffff {
        return Err("hardware_id_response hardware_id_type exceeds 24 bits".to_string());
    }
    Ok(hrpd_air::HrpdHardwareIdResponse {
        transaction_id,
        hardware_id_type: response.hardware_id_type,
        hardware_id_value: response.hardware_id_value,
    })
}
