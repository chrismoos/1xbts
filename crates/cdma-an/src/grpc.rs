//! gRPC server implementing `an.v1.AnService`.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use tokio::sync::Mutex;
use tonic::{Request, Response, Status};

use cdma_common::bits::Bitstream;
use cdma_common::hrpd::air as hrpd_air;
use cdma_common::hrpd::uati::HrpdUati;
use cdma_events::proto as bus;

use crate::air::{HrpdAccessOutcome, HrpdAirController, HrpdTrafficOutcome};
use crate::events::{AnEventSink, RecentKind};
use crate::hrpd_identity::{HrpdHardwareIdentity, hardware_identity_from_response};
use crate::proto::an::v1 as proto;
use crate::proto::an::v1::an_service_server::{AnService, AnServiceServer};
use crate::proto::an::v1::{
    AnEventRecord, GetSessionEventsRequest, GetSessionEventsResponse, GetSessionRequest,
    GetSessionResponse, GetSessionsRequest, GetSessionsResponse, GetUatiAllocationRequest,
    GetUatiAllocationResponse, NegotiatedProtocols as ProtoNegotiated, Session as ProtoSession,
    SessionState as ProtoSessionState, an_event_record,
};
use crate::proto::events::v1 as an_events;
use crate::session::{Session, SessionState};
use crate::subnet::UatiAllocator;

const SESSION_CONFIGURATION_COMPLETE: u8 = 0x00;
const SESSION_CONFIGURATION_START: u8 = 0x01;
const SESSION_SOFT_CONFIGURATION_COMPLETE: u8 = 0x02;
const SESSION_CONFIGURATION_REQUEST: u8 = 0x50;
const SESSION_CONFIGURATION_RESPONSE: u8 = 0x51;
const SESSION_PROTOCOL_STREAM: u8 = 0x13;
const SESSION_PROTOCOL_MULTIMODE_CAPABILITY_DISCOVERY: u8 = 0x1b;
const SESSION_PROTOCOL_DEFAULT_PACKET_START: u8 = 0x15;
const SESSION_PROTOCOL_DEFAULT_PACKET_END: u8 = 0x17;
const DEFAULT_ACCESS_CHANNEL_MAC_PROTOCOL_TYPE: u8 = 0x02;
const ACCESS_CHANNEL_ACK_MESSAGE_ID: u8 = 0x00;
const TRAFFIC_CHANNEL_ASSIGNMENT_MESSAGE_ID: u8 = 0x01;

/// `GetUatiAllocation` color_code value meaning "the configured allocator",
/// so a caller that doesn't yet know the sector's color can still query it.
const WILDCARD_COLOR_CODE: u32 = 0;

pub type SessionStore = Arc<Mutex<HashMap<u32, Session>>>;
pub type SharedUatiAllocator = Arc<Mutex<UatiAllocator>>;
pub type SharedHrpdAirController = Arc<Mutex<HrpdAirController>>;

#[derive(Clone)]
pub struct AnServiceImpl {
    sessions: SessionStore,
    uati: SharedUatiAllocator,
    air: SharedHrpdAirController,
    events: Option<Arc<AnEventSink>>,
}

impl AnServiceImpl {
    pub fn new(sessions: SessionStore, uati: SharedUatiAllocator) -> Self {
        Self {
            sessions,
            uati,
            air: Arc::new(Mutex::new(HrpdAirController::new(0))),
            events: None,
        }
    }

    pub fn new_with_air(
        sessions: SessionStore,
        uati: SharedUatiAllocator,
        air: SharedHrpdAirController,
    ) -> Self {
        Self {
            sessions,
            uati,
            air,
            events: None,
        }
    }

    /// Attach an event-bus sink. HRPD session/access/traffic activity is then
    /// published to the aggregated bus in addition to flowing over this gRPC.
    pub fn with_events(mut self, sink: Arc<AnEventSink>) -> Self {
        self.events = Some(sink);
        self
    }

    pub fn into_server(self) -> AnServiceServer<Self> {
        AnServiceServer::new(self)
    }
}

/// Builds a Rev 0 HRPD session event for the bus. Subtypes are all "Default"
/// (0) in Rev 0, matching `subtype_id`.
fn bus_session_event(
    uati: u32,
    reason: bus::HrpdSessionReason,
    color_code: u32,
) -> bus::HrpdSessionEvent {
    bus::HrpdSessionEvent {
        timestamp_ns: wall_clock_ns(),
        uati,
        reason: reason as i32,
        color_code,
        air_link_management_subtype: 0,
        session_management_subtype: 0,
        address_management_subtype: 0,
        connection_layer_subtype: 0,
        security_subtype: 0,
        mac_subtype: 0,
        physical_layer_subtype: 0,
        full_uati: None,
    }
}

fn bus_uati(full: HrpdUati, compact_uati32: u32) -> bus::HrpdUati {
    bus::HrpdUati {
        value: full.value().to_vec(),
        color_code: u32::from(full.color_code()),
        subnet_mask: u32::from(full.subnet_mask()),
        compact_uati32,
    }
}

fn an_uati(full: HrpdUati, compact_uati32: u32) -> an_events::HrpdUati {
    an_events::HrpdUati {
        value: full.value().to_vec(),
        color_code: u32::from(full.color_code()),
        subnet_mask: u32::from(full.subnet_mask()),
        compact_uati32,
    }
}

fn bus_uati_from_session(session: &Session) -> bus::HrpdUati {
    bus_uati(session.uati.full(), session.uati.as_u32())
}

fn an_uati_from_session(session: &Session) -> an_events::HrpdUati {
    an_uati(session.uati.full(), session.uati.as_u32())
}

fn an_uati_from_bus(full: bus::HrpdUati) -> an_events::HrpdUati {
    an_events::HrpdUati {
        value: full.value,
        color_code: full.color_code,
        subnet_mask: full.subnet_mask,
        compact_uati32: full.compact_uati32,
    }
}

fn bus_uati_for_event_uati(sessions: &[Session], uati: u32) -> Option<bus::HrpdUati> {
    sessions
        .iter()
        .find(|session| session.uati.as_u32() == uati || session.uati.receive_ati_u32() == uati)
        .map(bus_uati_from_session)
}

fn receive_ati_for_event_uati(sessions: &[Session], uati: u32) -> u32 {
    sessions
        .iter()
        .find(|session| session.uati.as_u32() == uati || session.uati.receive_ati_u32() == uati)
        .map(|session| session.uati.receive_ati_u32())
        .unwrap_or(uati)
}

fn wall_clock_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0)
}

fn decoded_message(
    type_name: impl Into<String>,
    summary: impl Into<String>,
    protocol_type: u32,
    message_id: u32,
    payload: Vec<u8>,
) -> bus::HrpdDecodedMessage {
    bus::HrpdDecodedMessage {
        type_name: type_name.into(),
        summary: summary.into(),
        protocol_type,
        message_id,
        payload,
    }
}

fn hex_lower(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

fn stream0_protocol_name(protocol_type: u8) -> &'static str {
    match protocol_type {
        0x00 => "PhysicalLayer",
        0x01 => "ControlChannelMAC",
        0x02 => "AccessChannelMAC",
        0x03 => "ForwardTrafficChannelMAC",
        0x04 => "ReverseTrafficChannelMAC",
        0x05 => "KeyExchange",
        0x06 => "Authentication",
        0x07 => "Encryption",
        0x08 => "Security",
        0x09 => "PacketConsolidation",
        0x0a => "AirLinkManagement",
        0x0b => "InitializationState",
        hrpd_air::DEFAULT_IDLE_STATE_PROTOCOL_TYPE => "IdleState",
        hrpd_air::DEFAULT_CONNECTED_STATE_PROTOCOL_TYPE => "ConnectedState",
        hrpd_air::DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE => "RouteUpdate",
        0x0f => "OverheadMessages",
        hrpd_air::DEFAULT_SESSION_MANAGEMENT_PROTOCOL_TYPE => "SessionManagement",
        0x11 => "AddressManagement",
        hrpd_air::DEFAULT_SESSION_CONFIGURATION_PROTOCOL_TYPE => "SessionConfiguration",
        SESSION_PROTOCOL_STREAM => "Stream",
        hrpd_air::DEFAULT_STREAM0_APPLICATION_PROTOCOL_TYPE => "DefaultSignaling",
        SESSION_PROTOCOL_MULTIMODE_CAPABILITY_DISCOVERY => "MultimodeCapabilityDiscovery",
        SESSION_PROTOCOL_DEFAULT_PACKET_START..=SESSION_PROTOCOL_DEFAULT_PACKET_END => {
            "DefaultPacket"
        }
        _ => "Protocol",
    }
}

fn stream0_session_configuration_message_name(message_id: Option<u8>) -> &'static str {
    match message_id {
        Some(SESSION_CONFIGURATION_COMPLETE) => "ConfigurationComplete",
        Some(SESSION_CONFIGURATION_START) => "ConfigurationStart",
        Some(SESSION_SOFT_CONFIGURATION_COMPLETE) => "SoftConfigurationComplete",
        Some(SESSION_CONFIGURATION_REQUEST) => "ConfigurationRequest",
        Some(SESSION_CONFIGURATION_RESPONSE) => "ConfigurationResponse",
        _ => "Unhandled",
    }
}

fn stream0_message_name(protocol_type: u8, message_id: Option<u8>) -> &'static str {
    match (protocol_type, message_id) {
        (_, Some(SESSION_CONFIGURATION_REQUEST)) => "ConfigurationRequest",
        (_, Some(SESSION_CONFIGURATION_RESPONSE)) => "ConfigurationResponse",
        (hrpd_air::DEFAULT_SESSION_CONFIGURATION_PROTOCOL_TYPE, _) => {
            stream0_session_configuration_message_name(message_id)
        }
        (hrpd_air::DEFAULT_STREAM0_APPLICATION_PROTOCOL_TYPE, Some(0x00)) => "Reset",
        (hrpd_air::DEFAULT_STREAM0_APPLICATION_PROTOCOL_TYPE, Some(0x01)) => "ResetAck",
        _ => "Unhandled",
    }
}

fn decoded_stream0_control_message(
    protocol_type: u8,
    message_id: Option<u8>,
    payload: &[u8],
) -> bus::HrpdDecodedMessage {
    let protocol_name = stream0_protocol_name(protocol_type);
    let message_name = stream0_message_name(protocol_type, message_id);
    if message_name == "Unhandled" {
        return decoded_message(
            "Unknown",
            format!(
                "Unknown protocol=0x{:02X} message={}",
                protocol_type,
                message_id
                    .map(|id| format!("0x{id:02X}"))
                    .unwrap_or_else(|| "-".to_string())
            ),
            u32::from(protocol_type),
            message_id.map(u32::from).unwrap_or(0),
            payload.to_vec(),
        );
    }

    let transaction = payload.get(1).copied();
    let attribute_len = payload.len().saturating_sub(2);
    let type_name = if protocol_type == hrpd_air::DEFAULT_SESSION_CONFIGURATION_PROTOCOL_TYPE {
        format!("Session{message_name}")
    } else {
        format!("{protocol_name}{message_name}")
    };
    let summary = match transaction {
        Some(transaction) => {
            format!("{type_name} transaction=0x{transaction:02X} attrs={attribute_len}B")
        }
        None => format!("{type_name} attrs={}B", payload.len()),
    };
    decoded_message(
        type_name,
        summary,
        u32::from(protocol_type),
        message_id.map(u32::from).unwrap_or(0),
        payload.to_vec(),
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ForwardTrafficChannelAssignment {
    message_sequence: u8,
    channel: Option<(u8, u8, u16)>,
    frame_offset: u8,
    drc_length: u8,
    drc_channel_gain_half_db: i8,
    ack_channel_gain_half_db: i8,
    pilot_count: u8,
    first_pilot_pn: Option<u16>,
    first_mac_index: Option<u8>,
    first_drc_cover: Option<u8>,
}

fn signed_6(raw: u64) -> i8 {
    let raw = (raw & 0x3f) as i8;
    if raw & 0x20 != 0 { raw - 64 } else { raw }
}

fn gain_half_db_text(value: i8) -> String {
    let sign = if value >= 0 { "+" } else { "-" };
    let abs = value.abs();
    format!(
        "{sign}{}.{:01}dB",
        abs / 2,
        if abs % 2 == 0 { 0 } else { 5 }
    )
}

fn decode_forward_traffic_channel_assignment(
    payload: &[u8],
) -> Option<ForwardTrafficChannelAssignment> {
    if payload.len() > 16 {
        return None;
    }
    let mut bits = Bitstream::new_bytes(payload);
    let message_id = bits.read_bits(8).ok()? as u8;
    if message_id != TRAFFIC_CHANNEL_ASSIGNMENT_MESSAGE_ID {
        return None;
    }
    let message_sequence = bits.read_bits(8).ok()? as u8;
    let channel = if bits.read_bits(1).ok()? != 0 {
        let system_type = bits.read_bits(8).ok()? as u8;
        let band_class = bits.read_bits(5).ok()? as u8;
        let channel_number = bits.read_bits(11).ok()? as u16;
        Some((system_type, band_class, channel_number))
    } else {
        None
    };
    let frame_offset = bits.read_bits(4).ok()? as u8;
    let drc_length = bits.read_bits(2).ok()? as u8;
    let drc_channel_gain_half_db = signed_6(bits.read_bits(6).ok()?);
    let ack_channel_gain_half_db = signed_6(bits.read_bits(6).ok()?);
    let pilot_count = bits.read_bits(4).ok()? as u8;
    let (first_pilot_pn, first_mac_index, first_drc_cover) = if pilot_count > 0 {
        let pilot_pn = bits.read_bits(9).ok()? as u16;
        let _softer_handoff = bits.read_bits(1).ok()?;
        let mac_index = bits.read_bits(6).ok()? as u8;
        let drc_cover = bits.read_bits(3).ok()? as u8;
        Some((pilot_pn, mac_index, drc_cover))
    } else {
        None
    }
    .map(|(pilot_pn, mac_index, drc_cover)| (Some(pilot_pn), Some(mac_index), Some(drc_cover)))
    .unwrap_or((None, None, None));

    Some(ForwardTrafficChannelAssignment {
        message_sequence,
        channel,
        frame_offset,
        drc_length,
        drc_channel_gain_half_db,
        ack_channel_gain_half_db,
        pilot_count,
        first_pilot_pn,
        first_mac_index,
        first_drc_cover,
    })
}

fn decoded_forward_control_message(
    protocol_type: u8,
    payload: &[u8],
) -> Option<bus::HrpdDecodedMessage> {
    let message_id = payload.first().copied();
    match (protocol_type, message_id) {
        (DEFAULT_ACCESS_CHANNEL_MAC_PROTOCOL_TYPE, Some(ACCESS_CHANNEL_ACK_MESSAGE_ID)) => {
            Some(decoded_message(
                "AccessChannelAck",
                "AccessChannelMAC ACAck",
                u32::from(protocol_type),
                u32::from(ACCESS_CHANNEL_ACK_MESSAGE_ID),
                Vec::new(),
            ))
        }
        (
            hrpd_air::DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE,
            Some(TRAFFIC_CHANNEL_ASSIGNMENT_MESSAGE_ID),
        ) => {
            let Some(assignment) = decode_forward_traffic_channel_assignment(payload) else {
                return Some(decoded_message(
                    "TrafficChannelAssignment",
                    format!("TrafficChannelAssignment payload={}B", payload.len()),
                    u32::from(protocol_type),
                    u32::from(TRAFFIC_CHANNEL_ASSIGNMENT_MESSAGE_ID),
                    payload.to_vec(),
                ));
            };
            let channel = assignment
                .channel
                .map(|(system_type, band_class, channel_number)| {
                    format!(" channel={system_type}/{band_class}/{channel_number}")
                })
                .unwrap_or_default();
            let first_pilot = match (
                assignment.first_pilot_pn,
                assignment.first_mac_index,
                assignment.first_drc_cover,
            ) {
                (Some(pilot_pn), Some(mac_index), Some(drc_cover)) => {
                    format!(" pilot_pn={pilot_pn} mac={mac_index} drc_cover={drc_cover}")
                }
                _ => String::new(),
            };
            Some(decoded_message(
                "TrafficChannelAssignment",
                format!(
                    "TrafficChannelAssignment seq={} pilots={}{}{} frame_offset={} drc_len={} drc_gain={} ack_gain={}",
                    assignment.message_sequence,
                    assignment.pilot_count,
                    first_pilot,
                    channel,
                    assignment.frame_offset,
                    assignment.drc_length,
                    gain_half_db_text(assignment.drc_channel_gain_half_db),
                    gain_half_db_text(assignment.ack_channel_gain_half_db)
                ),
                u32::from(protocol_type),
                u32::from(TRAFFIC_CHANNEL_ASSIGNMENT_MESSAGE_ID),
                payload.to_vec(),
            ))
        }
        _ => None,
    }
}

fn bus_identity_from_hardware(
    response: &hrpd_air::HrpdHardwareIdResponse,
) -> Option<bus::MobileIdentity> {
    match hardware_identity_from_response(response)? {
        HrpdHardwareIdentity::Esn(esn) => Some(bus::MobileIdentity {
            imsi: String::new(),
            esn,
            meid: String::new(),
        }),
        HrpdHardwareIdentity::Meid(meid) => Some(bus::MobileIdentity {
            imsi: String::new(),
            esn: 0,
            meid: hex_lower(&meid.0),
        }),
    }
}

fn decoded_access_message(message: &hrpd_air::HrpdAccessMessage) -> bus::HrpdDecodedMessage {
    match message {
        hrpd_air::HrpdAccessMessage::RouteUpdate(route) => decoded_message(
            "RouteUpdate",
            format!(
                "RouteUpdate seq={} ref_pn={} strength={} keep={} pilots={}",
                route.message_sequence,
                route.reference_pilot_pn,
                route.reference_pilot_strength,
                route.reference_keep,
                route.num_pilots
            ),
            u32::from(hrpd_air::DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE),
            0,
            Vec::new(),
        ),
        hrpd_air::HrpdAccessMessage::UatiRequest(request) => decoded_message(
            "UATIRequest",
            format!("UATIRequest transaction=0x{:02X}", request.transaction_id),
            u32::from(hrpd_air::DEFAULT_SESSION_MANAGEMENT_PROTOCOL_TYPE),
            0,
            Vec::new(),
        ),
        hrpd_air::HrpdAccessMessage::UatiComplete(complete) => decoded_message(
            "UATIComplete",
            format!(
                "UATIComplete seq={} upper_old_uati={}B",
                complete.message_sequence,
                complete.upper_old_uati.len()
            ),
            u32::from(hrpd_air::DEFAULT_SESSION_MANAGEMENT_PROTOCOL_TYPE),
            2,
            complete.upper_old_uati.clone(),
        ),
        hrpd_air::HrpdAccessMessage::ConnectionRequest(request) => decoded_message(
            "ConnectionRequest",
            format!(
                "ConnectionRequest transaction=0x{:02X} reason=0x{:02X}",
                request.transaction_id, request.request_reason
            ),
            u32::from(hrpd_air::DEFAULT_CONNECTED_STATE_PROTOCOL_TYPE),
            0,
            Vec::new(),
        ),
        hrpd_air::HrpdAccessMessage::TrafficChannelComplete(complete) => decoded_message(
            "TrafficChannelComplete",
            format!("TrafficChannelComplete seq={}", complete.message_sequence),
            u32::from(hrpd_air::DEFAULT_CONNECTED_STATE_PROTOCOL_TYPE),
            0,
            Vec::new(),
        ),
        hrpd_air::HrpdAccessMessage::SessionClose(close) => decoded_message(
            "SessionClose",
            format!(
                "SessionClose reason=0x{:02X} ({}) more_info={}B",
                close.close_reason,
                hrpd_air::hrpd_session_close_reason_name(close.close_reason),
                close.more_info.len()
            ),
            u32::from(hrpd_air::DEFAULT_SESSION_MANAGEMENT_PROTOCOL_TYPE),
            3,
            close.more_info.clone(),
        ),
        hrpd_air::HrpdAccessMessage::ConnectionClose(close) => decoded_message(
            "ConnectionClose",
            format!(
                "ConnectionClose reason=0x{:02X} ({}) suspend={}",
                close.close_reason,
                hrpd_air::hrpd_connection_close_reason_name(close.close_reason),
                close.suspend_enable
            ),
            u32::from(hrpd_air::DEFAULT_CONNECTED_STATE_PROTOCOL_TYPE),
            1,
            Vec::new(),
        ),
        hrpd_air::HrpdAccessMessage::HardwareIdResponse(response) => decoded_message(
            "HardwareIdResponse",
            format!(
                "HardwareIdResponse transaction=0x{:02X} type=0x{:06X} value={}B",
                response.transaction_id,
                response.hardware_id_type,
                response.hardware_id_value.len()
            ),
            u32::from(hrpd_air::DEFAULT_SESSION_MANAGEMENT_PROTOCOL_TYPE),
            0,
            response.hardware_id_value.clone(),
        ),
        hrpd_air::HrpdAccessMessage::KeepAlive => decoded_message(
            "KeepAlive",
            "KeepAlive",
            u32::from(hrpd_air::DEFAULT_CONNECTED_STATE_PROTOCOL_TYPE),
            0,
            Vec::new(),
        ),
        hrpd_air::HrpdAccessMessage::DefaultPacketXonRequest => decoded_message(
            "DefaultPacketXonRequest",
            "DefaultPacket XonRequest",
            0,
            0x07,
            Vec::new(),
        ),
        hrpd_air::HrpdAccessMessage::DefaultPacketXoffRequest => decoded_message(
            "DefaultPacketXoffRequest",
            "DefaultPacket XoffRequest",
            0,
            0x09,
            Vec::new(),
        ),
        hrpd_air::HrpdAccessMessage::DefaultPacketDataReadyAck(ack) => decoded_message(
            "DefaultPacketDataReadyAck",
            format!(
                "DefaultPacket DataReadyAck transaction=0x{:02X}",
                ack.transaction_id
            ),
            0,
            0x0d,
            Vec::new(),
        ),
        hrpd_air::HrpdAccessMessage::DefaultPacketRlpReset(_) => decoded_message(
            "DefaultPacketRlpReset",
            "DefaultPacket RLP Reset",
            0,
            0x0f,
            Vec::new(),
        ),
        hrpd_air::HrpdAccessMessage::DefaultPacketRlpResetAck(_) => decoded_message(
            "DefaultPacketRlpResetAck",
            "DefaultPacket RLP ResetAck",
            0,
            0x10,
            Vec::new(),
        ),
        hrpd_air::HrpdAccessMessage::DefaultPacketRlpNak(nak) => decoded_message(
            "DefaultPacketRlpNak",
            format!("DefaultPacket RLP Nak requests={}", nak.requests.len()),
            0,
            0x11,
            Vec::new(),
        ),
        hrpd_air::HrpdAccessMessage::DefaultSignalingReset(reset) => decoded_message(
            "DefaultSignalingReset",
            format!("DefaultSignaling Reset seq={}", reset.message_sequence),
            u32::from(hrpd_air::DEFAULT_STREAM0_APPLICATION_PROTOCOL_TYPE),
            0,
            Vec::new(),
        ),
        hrpd_air::HrpdAccessMessage::DefaultSignalingResetAck(ack) => decoded_message(
            "DefaultSignalingResetAck",
            format!("DefaultSignaling ResetAck seq={}", ack.message_sequence),
            u32::from(hrpd_air::DEFAULT_STREAM0_APPLICATION_PROTOCOL_TYPE),
            1,
            Vec::new(),
        ),
        hrpd_air::HrpdAccessMessage::Unknown {
            protocol_type,
            message_id,
            payload,
        } => decoded_stream0_control_message(*protocol_type, *message_id, payload),
    }
}

fn forward_channel_name(channel: &hrpd_air::HrpdForwardChannel) -> &'static str {
    match channel {
        hrpd_air::HrpdForwardChannel::SynchronousControl => "SynchronousControl",
        hrpd_air::HrpdForwardChannel::AsynchronousControl => "AsynchronousControl",
        hrpd_air::HrpdForwardChannel::ForwardTraffic => "ForwardTraffic",
    }
}

fn forward_signaling_uati(request: &hrpd_air::HrpdForwardSignalingRequest) -> u32 {
    request.uati.unwrap_or_else(|| {
        if request.target_ati.ati_type == hrpd_air::AccessTerminalIdentifierType::Uati {
            request.target_ati.value
        } else {
            0
        }
    })
}

fn decoded_forward_signaling(
    request: &hrpd_air::HrpdForwardSignalingRequest,
) -> bus::HrpdDecodedMessage {
    if let Some(message) = decoded_forward_control_message(request.protocol_type, &request.payload)
    {
        return message;
    }
    let mut message = decoded_stream0_control_message(
        request.protocol_type,
        request.payload.first().copied(),
        &request.payload,
    );
    if message.type_name == "Unknown" {
        message.type_name = "ForwardSignaling".to_string();
        message.summary = format!(
            "ForwardSignaling {} protocol=0x{:02X} payload={}B reliable_seq={}",
            forward_channel_name(&request.channel),
            request.protocol_type,
            request.payload.len(),
            request
                .reliable_sequence
                .map(|seq| seq.to_string())
                .unwrap_or_else(|| "-".to_string())
        );
    } else {
        message.summary = format!(
            "{} {} reliable_seq={}",
            message.summary,
            forward_channel_name(&request.channel),
            request
                .reliable_sequence
                .map(|seq| seq.to_string())
                .unwrap_or_else(|| "-".to_string())
        );
    }
    message
}

fn decoded_traffic_outcome(outcome: &HrpdTrafficOutcome) -> Vec<bus::HrpdDecodedMessage> {
    let mut messages = Vec::new();
    for message in &outcome.decoded_stream0_messages {
        if matches!(message, hrpd_air::HrpdAccessMessage::DefaultPacketRlpNak(_)) {
            continue;
        }
        messages.push(decoded_access_message(message));
    }
    for uati in &outcome.session_configuration_complete_uatis {
        messages.push(decoded_message(
            "SessionConfigurationComplete",
            format!("SessionConfigurationComplete UATI=0x{uati:08X}"),
            u32::from(hrpd_air::DEFAULT_SESSION_CONFIGURATION_PROTOCOL_TYPE),
            0,
            Vec::new(),
        ));
    }
    messages
}

// The event-bus sink builds `cdma_events::proto` (`bus`) event values, but the
// AnService response generates its own `events.v1` copies; these convert
// between the two structurally-identical types for the history query.
fn to_an_session_event(e: bus::HrpdSessionEvent) -> an_events::HrpdSessionEvent {
    an_events::HrpdSessionEvent {
        timestamp_ns: e.timestamp_ns,
        uati: e.uati,
        reason: e.reason,
        color_code: e.color_code,
        air_link_management_subtype: e.air_link_management_subtype,
        session_management_subtype: e.session_management_subtype,
        address_management_subtype: e.address_management_subtype,
        connection_layer_subtype: e.connection_layer_subtype,
        security_subtype: e.security_subtype,
        mac_subtype: e.mac_subtype,
        physical_layer_subtype: e.physical_layer_subtype,
        full_uati: e.full_uati.map(an_uati_from_bus),
    }
}

fn to_an_decoded_message(e: bus::HrpdDecodedMessage) -> an_events::HrpdDecodedMessage {
    an_events::HrpdDecodedMessage {
        type_name: e.type_name,
        summary: e.summary,
        protocol_type: e.protocol_type,
        message_id: e.message_id,
        payload: e.payload,
    }
}

fn to_an_access_event(e: bus::HrpdAccessEvent) -> an_events::HrpdAccessEvent {
    an_events::HrpdAccessEvent {
        timestamp_ns: e.timestamp_ns,
        access_signature: e.access_signature,
        reason: e.reason,
        payload: e.payload,
        color_code: e.color_code,
        direction: e.direction,
        decoded_messages: e
            .decoded_messages
            .into_iter()
            .map(to_an_decoded_message)
            .collect(),
        payload_length_bytes: e.payload_length_bytes,
        uati: e.uati,
        full_uati: e.full_uati.map(an_uati_from_bus),
        receive_ati: e.receive_ati,
    }
}

fn to_an_traffic_event(e: bus::HrpdTrafficEvent) -> an_events::HrpdTrafficEvent {
    an_events::HrpdTrafficEvent {
        timestamp_ns: e.timestamp_ns,
        uati: e.uati,
        reason: e.reason,
        mac_index: e.mac_index,
        drc_value: e.drc_value,
        payload: e.payload,
        reverse_pilot_snr_db_tenths: e.reverse_pilot_snr_db_tenths,
        direction: e.direction,
        decoded_messages: e
            .decoded_messages
            .into_iter()
            .map(to_an_decoded_message)
            .collect(),
        payload_length_bytes: e.payload_length_bytes,
        full_uati: e.full_uati.map(an_uati_from_bus),
        receive_ati: e.receive_ati,
    }
}

fn checked_u8(value: u32, field: &'static str) -> Result<u8, Status> {
    u8::try_from(value).map_err(|_| Status::invalid_argument(format!("{field} exceeds u8")))
}

fn checked_u16(value: u32, field: &'static str) -> Result<u16, Status> {
    u16::try_from(value).map_err(|_| Status::invalid_argument(format!("{field} exceeds u16")))
}

fn checked_i16(value: i32, field: &'static str) -> Result<i16, Status> {
    i16::try_from(value).map_err(|_| Status::invalid_argument(format!("{field} exceeds i16")))
}

fn ati_type_from_proto(
    value: proto::AccessTerminalIdentifierType,
) -> hrpd_air::AccessTerminalIdentifierType {
    match value {
        proto::AccessTerminalIdentifierType::Bati => hrpd_air::AccessTerminalIdentifierType::Bati,
        proto::AccessTerminalIdentifierType::Reserved => {
            hrpd_air::AccessTerminalIdentifierType::Reserved
        }
        proto::AccessTerminalIdentifierType::Uati => hrpd_air::AccessTerminalIdentifierType::Uati,
        proto::AccessTerminalIdentifierType::Rati => hrpd_air::AccessTerminalIdentifierType::Rati,
        proto::AccessTerminalIdentifierType::Unspecified => {
            hrpd_air::AccessTerminalIdentifierType::Reserved
        }
    }
}

fn ati_to_proto(ati: hrpd_air::AccessTerminalIdentifier) -> proto::AccessTerminalIdentifier {
    proto::AccessTerminalIdentifier {
        ati_type: match ati.ati_type {
            hrpd_air::AccessTerminalIdentifierType::Bati => {
                proto::AccessTerminalIdentifierType::Bati as i32
            }
            hrpd_air::AccessTerminalIdentifierType::Reserved => {
                proto::AccessTerminalIdentifierType::Reserved as i32
            }
            hrpd_air::AccessTerminalIdentifierType::Uati => {
                proto::AccessTerminalIdentifierType::Uati as i32
            }
            hrpd_air::AccessTerminalIdentifierType::Rati => {
                proto::AccessTerminalIdentifierType::Rati as i32
            }
        },
        value: ati.value,
    }
}

fn ati_from_proto(
    ati: Option<proto::AccessTerminalIdentifier>,
) -> Result<hrpd_air::AccessTerminalIdentifier, Status> {
    let ati = ati.ok_or_else(|| Status::invalid_argument("missing ATI"))?;
    let ati_type = proto::AccessTerminalIdentifierType::try_from(ati.ati_type)
        .map_err(|_| Status::invalid_argument("invalid ATI type"))?;
    Ok(hrpd_air::AccessTerminalIdentifier {
        ati_type: ati_type_from_proto(ati_type),
        value: ati.value,
    })
}

fn access_message_from_proto(
    message: proto::HrpdAccessMessage,
) -> Result<hrpd_air::HrpdAccessMessage, Status> {
    use proto::hrpd_access_message::Message;
    let Some(message) = message.message else {
        return Err(Status::invalid_argument("missing HRPD access message body"));
    };
    Ok(match message {
        Message::RouteUpdate(route) => {
            hrpd_air::HrpdAccessMessage::RouteUpdate(hrpd_air::HrpdRouteUpdate {
                message_sequence: checked_u8(
                    route.message_sequence,
                    "route_update.message_sequence",
                )?,
                reference_pilot_pn: checked_u16(
                    route.reference_pilot_pn,
                    "route_update.reference_pilot_pn",
                )?,
                reference_pilot_strength: checked_u8(
                    route.reference_pilot_strength,
                    "route_update.reference_pilot_strength",
                )?,
                reference_keep: route.reference_keep,
                num_pilots: checked_u8(route.num_pilots, "route_update.num_pilots")?,
                at_total_pilot_transmission: route
                    .at_total_pilot_transmission
                    .map(|v| {
                        i8::try_from(v).map_err(|_| {
                            Status::invalid_argument(
                                "route_update.at_total_pilot_transmission exceeds i8",
                            )
                        })
                    })
                    .transpose()?,
                reference_pilot_channel: route.reference_pilot_channel,
                reserved_zero: route.reserved_zero,
            })
        }
        Message::UatiRequest(uati) => {
            hrpd_air::HrpdAccessMessage::UatiRequest(hrpd_air::HrpdUatiRequest {
                transaction_id: checked_u8(uati.transaction_id, "uati_request.transaction_id")?,
            })
        }
        Message::UatiComplete(uati) => {
            hrpd_air::HrpdAccessMessage::UatiComplete(hrpd_air::HrpdUatiComplete {
                message_sequence: checked_u8(
                    uati.message_sequence,
                    "uati_complete.message_sequence",
                )?,
                upper_old_uati: uati.upper_old_uati,
                reserved_zero: uati.reserved_zero,
            })
        }
        Message::ConnectionRequest(connection) => {
            hrpd_air::HrpdAccessMessage::ConnectionRequest(hrpd_air::HrpdConnectionRequest {
                transaction_id: checked_u8(
                    connection.transaction_id,
                    "connection_request.transaction_id",
                )?,
                request_reason: checked_u8(
                    connection.request_reason,
                    "connection_request.request_reason",
                )?,
                reserved_zero: connection.reserved_zero,
            })
        }
        Message::TrafficChannelComplete(complete) => {
            hrpd_air::HrpdAccessMessage::TrafficChannelComplete(
                hrpd_air::HrpdTrafficChannelComplete {
                    message_sequence: checked_u8(
                        complete.message_sequence,
                        "traffic_channel_complete.message_sequence",
                    )?,
                },
            )
        }
        Message::SessionClose(close) => {
            hrpd_air::HrpdAccessMessage::SessionClose(hrpd_air::HrpdSessionClose {
                close_reason: checked_u8(close.close_reason as u32, "session_close.close_reason")?,
                more_info: close.more_info,
            })
        }
        Message::ConnectionClose(close) => {
            hrpd_air::HrpdAccessMessage::ConnectionClose(hrpd_air::HrpdConnectionClose {
                close_reason: checked_u8(
                    close.close_reason as u32,
                    "connection_close.close_reason",
                )?,
                suspend_enable: close.suspend_enable,
                suspend_time: close.suspend_time,
                reserved_zero: close.reserved_zero,
            })
        }
        Message::HardwareIdResponse(hardware) => {
            if hardware.hardware_id_type > 0x00ff_ffff {
                return Err(Status::invalid_argument(
                    "hardware_id_response.hardware_id_type exceeds 24 bits",
                ));
            }
            hrpd_air::HrpdAccessMessage::HardwareIdResponse(hrpd_air::HrpdHardwareIdResponse {
                transaction_id: checked_u8(
                    hardware.transaction_id,
                    "hardware_id_response.transaction_id",
                )?,
                hardware_id_type: hardware.hardware_id_type,
                hardware_id_value: hardware.hardware_id_value,
            })
        }
        Message::KeepAlive(_) => hrpd_air::HrpdAccessMessage::KeepAlive,
        Message::DefaultPacketXonRequest(_) => hrpd_air::HrpdAccessMessage::DefaultPacketXonRequest,
        Message::DefaultPacketXoffRequest(_) => {
            hrpd_air::HrpdAccessMessage::DefaultPacketXoffRequest
        }
        Message::DefaultPacketDataReadyAck(ack) => {
            hrpd_air::HrpdAccessMessage::DefaultPacketDataReadyAck(
                hrpd_air::HrpdDefaultPacketDataReadyAck {
                    transaction_id: checked_u8(
                        ack.transaction_id,
                        "default_packet_data_ready_ack.transaction_id",
                    )?,
                },
            )
        }
        Message::Unknown(unknown) => hrpd_air::HrpdAccessMessage::Unknown {
            protocol_type: checked_u8(unknown.protocol_type, "unknown.protocol_type")?,
            message_id: unknown
                .message_id
                .map(|v| checked_u8(v, "unknown.message_id"))
                .transpose()?,
            payload: unknown.payload,
        },
    })
}

fn access_indication_from_proto(
    ind: proto::HrpdAccessIndication,
) -> Result<hrpd_air::HrpdAccessIndication, Status> {
    let messages = ind
        .messages
        .into_iter()
        .map(access_message_from_proto)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(hrpd_air::HrpdAccessIndication {
        absolute_chip: ind.absolute_chip,
        color_code: checked_u8(ind.color_code, "color_code")?,
        sector_pilot_pn: checked_u16(ind.sector_pilot_pn, "sector_pilot_pn")?,
        session_configuration_token: checked_u16(
            ind.session_configuration_token,
            "session_configuration_token",
        )?,
        ati: ati_from_proto(ind.ati)?,
        security_layer_format: ind.security_layer_format,
        connection_layer_format: ind.connection_layer_format,
        security_payload: ind.security_payload,
        messages,
    })
}

fn forward_channel_to_proto(channel: hrpd_air::HrpdForwardChannel) -> proto::HrpdForwardChannel {
    match channel {
        hrpd_air::HrpdForwardChannel::SynchronousControl => {
            proto::HrpdForwardChannel::SynchronousControl
        }
        hrpd_air::HrpdForwardChannel::AsynchronousControl => {
            proto::HrpdForwardChannel::AsynchronousControl
        }
        hrpd_air::HrpdForwardChannel::ForwardTraffic => proto::HrpdForwardChannel::ForwardTraffic,
    }
}

fn forward_signaling_to_proto(
    request: hrpd_air::HrpdForwardSignalingRequest,
) -> proto::HrpdForwardSignalingRequest {
    let synchronous_control_cycle = request.synchronous_control_cycle;
    proto::HrpdForwardSignalingRequest {
        uati: request.uati,
        target_ati: Some(ati_to_proto(request.target_ati)),
        protocol_type: u32::from(request.protocol_type),
        payload: request.payload,
        channel: forward_channel_to_proto(request.channel) as i32,
        reliable_sequence: request.reliable_sequence.map(u32::from),
        synchronous_control_cycle_modulus: synchronous_control_cycle
            .map(|schedule| u32::from(schedule.modulus)),
        synchronous_control_cycle_residue: synchronous_control_cycle
            .map(|schedule| u32::from(schedule.residue)),
    }
}

fn traffic_assignment_to_proto(
    request: hrpd_air::HrpdTrafficAssignmentRequest,
) -> proto::HrpdTrafficAssignmentRequest {
    proto::HrpdTrafficAssignmentRequest {
        session_uati: request.session_uati,
        uati: request.uati,
        full_uati: None,
        receive_ati: request.uati,
        mac_index: u32::from(request.mac_index),
        reverse_rate_limit_bps: request.reverse_rate_limit_bps,
        reverse_long_code_mask_i: request.reverse_long_code_mask_i,
        reverse_long_code_mask_q: request.reverse_long_code_mask_q,
        drc_lock: request.drc_lock,
        physical_layer_subtype: u32::from(request.physical_layer_subtype),
        reverse_traffic_mac_subtype: u32::from(request.reverse_traffic_mac_subtype),
        drc_cover: u32::from(request.drc_cover),
        drc_length: u32::from(request.drc_length),
        frame_offset: u32::from(request.frame_offset),
    }
}

fn forward_traffic_to_proto(
    packet: hrpd_air::HrpdForwardTrafficPacket,
) -> proto::HrpdForwardTrafficPacket {
    proto::HrpdForwardTrafficPacket {
        uati: packet.uati,
        full_uati: None,
        receive_ati: packet.uati,
        mac_index: u32::from(packet.mac_index),
        physical_layer_subtype: u32::from(packet.physical_layer_subtype),
        forward_traffic_mac_subtype: u32::from(packet.forward_traffic_mac_subtype),
        payload_bits: packet.payload_bits,
        high_priority: false,
    }
}

fn hardware_id_response_to_proto(
    response: hrpd_air::HrpdHardwareIdResponse,
) -> proto::HrpdHardwareIdResponse {
    proto::HrpdHardwareIdResponse {
        transaction_id: u32::from(response.transaction_id),
        hardware_id_type: response.hardware_id_type,
        hardware_id_value: response.hardware_id_value,
    }
}

fn outcome_to_proto(outcome: HrpdAccessOutcome) -> proto::HrpdAccessOutcome {
    proto::HrpdAccessOutcome {
        forward_signaling: outcome
            .forward_signaling
            .into_iter()
            .map(forward_signaling_to_proto)
            .collect(),
        forward_traffic: outcome
            .forward_traffic
            .into_iter()
            .map(forward_traffic_to_proto)
            .collect(),
        traffic_assignments: outcome
            .traffic_assignments
            .into_iter()
            .map(traffic_assignment_to_proto)
            .collect(),
        traffic_releases: outcome
            .traffic_releases
            .into_iter()
            .map(|release| proto::HrpdTrafficReleaseRequest {
                uati: release.uati,
                full_uati: None,
                receive_ati: release.uati,
                mac_index: u32::from(release.mac_index),
            })
            .collect(),
        session_closed_uatis: outcome.session_closed_uatis,
        route_update_count: outcome.route_updates.len() as u32,
        uati_complete_count: outcome.uati_completes.len() as u32,
        connection_request_count: outcome.connection_requests.len() as u32,
        connection_requested: outcome.connection_requested,
        keepalive_seen: outcome.keepalive_seen,
        unknown_message_count: outcome.unknown_messages as u32,
        hardware_id_response_count: outcome.hardware_id_responses.len() as u32,
    }
}

fn traffic_event_from_proto(
    event: proto::HrpdTrafficEvent,
) -> Result<hrpd_air::HrpdTrafficEvent, Status> {
    use proto::hrpd_traffic_event::Event;
    let Some(event) = event.event else {
        return Err(Status::invalid_argument("missing HRPD traffic event body"));
    };
    Ok(match event {
        Event::ReversePilot(pilot) => hrpd_air::HrpdTrafficEvent::ReversePilot {
            uati: pilot.uati,
            mac_index: checked_u8(pilot.mac_index, "reverse_pilot.mac_index")?,
            absolute_chip: pilot.absolute_chip,
            snr_db_tenths: checked_i16(pilot.snr_db_tenths, "reverse_pilot.snr_db_tenths")?,
        },
        Event::ReversePilotLost(pilot) => hrpd_air::HrpdTrafficEvent::ReversePilotLost {
            uati: pilot.uati,
            mac_index: checked_u8(pilot.mac_index, "reverse_pilot_lost.mac_index")?,
            last_good_chip: pilot.last_good_chip,
            lost_at_chip: pilot.lost_at_chip,
            lost_chips: pilot.lost_chips,
            last_snr_db_tenths: checked_i16(
                pilot.last_snr_db_tenths,
                "reverse_pilot_lost.last_snr_db_tenths",
            )?,
            last_coherence_x1000: checked_u16(
                pilot.last_coherence_x1000,
                "reverse_pilot_lost.last_coherence_x1000",
            )?,
        },
        Event::Drc(drc) => hrpd_air::HrpdTrafficEvent::Drc {
            uati: drc.uati,
            mac_index: checked_u8(drc.mac_index, "drc.mac_index")?,
            slot: drc.slot,
            drc_index: checked_u8(drc.drc_index, "drc.drc_index")?,
        },
        Event::Ack(ack) => hrpd_air::HrpdTrafficEvent::Ack {
            uati: ack.uati,
            mac_index: checked_u8(ack.mac_index, "ack.mac_index")?,
            slot: ack.slot,
            ack: ack.ack,
        },
        Event::Stream0Signaling(signaling) => hrpd_air::HrpdTrafficEvent::Stream0Signaling {
            uati: signaling.uati,
            payload: signaling.payload,
        },
        Event::Stream1Packet(packet) => hrpd_air::HrpdTrafficEvent::Stream1Packet {
            uati: packet.uati,
            sequence: packet.sequence,
            payload: packet.payload,
            decoded_at: None,
            air_frame_end_received_at: None,
        },
    })
}

pub fn traffic_outcome_to_proto(outcome: HrpdTrafficOutcome) -> proto::HrpdTrafficOutcome {
    proto::HrpdTrafficOutcome {
        accepted_event_count: outcome.accepted_event_count as u32,
        dropped_event_count: outcome.dropped_event_count as u32,
        unknown_session_count: outcome.unknown_session_count as u32,
        reverse_pilot_count: outcome.reverse_pilot_count as u32,
        drc_count: outcome.drc_count as u32,
        ack_count: outcome.ack_count as u32,
        stream0_signaling_count: outcome.stream0_signaling_count as u32,
        a8_uplink: outcome
            .a8_uplink
            .into_iter()
            .map(|packet| proto::HrpdA8UplinkPacket {
                uati: packet.uati,
                full_uati: None,
                receive_ati: packet.uati,
                payload: packet.payload,
            })
            .collect(),
        forward_signaling: outcome
            .forward_signaling
            .into_iter()
            .map(forward_signaling_to_proto)
            .collect(),
        forward_traffic: outcome
            .forward_traffic
            .into_iter()
            .map(forward_traffic_to_proto)
            .collect(),
        hardware_id_responses: outcome
            .hardware_id_responses
            .into_iter()
            .map(|hardware| proto::HrpdTrafficHardwareIdResponse {
                uati: hardware.uati,
                full_uati: None,
                receive_ati: hardware.uati,
                hardware_id_response: Some(hardware_id_response_to_proto(hardware.response)),
            })
            .collect(),
        session_configuration_pending_uatis: outcome.session_configuration_pending_uatis,
        session_configuration_complete_uatis: outcome.session_configuration_complete_uatis,
        session_configuration_complete_events: outcome
            .session_configuration_complete_events
            .into_iter()
            .map(|event| proto::HrpdSessionConfigurationCompleteEvent {
                uati: event.uati,
                full_uati: None,
                receive_ati: event.uati,
                physical_layer_subtype: u32::from(event.physical_layer_subtype),
                forward_traffic_mac_subtype: u32::from(event.forward_traffic_mac_subtype),
                idle_preferred_control_channel_cycle_enabled: event
                    .idle_preferred_control_channel_cycle
                    .is_some(),
                idle_preferred_control_channel_cycle: u32::from(
                    event
                        .idle_preferred_control_channel_cycle
                        .unwrap_or_default(),
                ),
                idle_page_period_cycles: u32::from(event.idle_page_period_cycles),
            })
            .collect(),
        default_packet_flow_open_uatis: outcome.default_packet_flow_open_uatis,
        default_packet_flow_closed_uatis: outcome.default_packet_flow_closed_uatis,
        default_packet_data_ready_acks: outcome
            .default_packet_data_ready_acks
            .into_iter()
            .map(|ack| proto::HrpdDefaultPacketDataReadyAckEvent {
                uati: ack.uati,
                full_uati: None,
                receive_ati: ack.uati,
                transaction_id: u32::from(ack.transaction_id),
            })
            .collect(),
        default_packet_stream_configurations: outcome
            .default_packet_stream_configurations
            .into_iter()
            .map(|config| proto::HrpdDefaultPacketStreamConfiguration {
                uati: config.uati,
                full_uati: None,
                receive_ati: config.uati,
                stream_id: u32::from(config.stream_id),
                protocol_type: u32::from(config.protocol_type),
                application_subtype: u32::from(config.application_subtype),
            })
            .collect(),
        default_packet_rlp_reset_uatis: outcome.default_packet_rlp_reset_uatis,
        default_packet_rlp_naks: outcome
            .default_packet_rlp_naks
            .into_iter()
            .map(|nak| proto::HrpdDefaultPacketRlpNakEvent {
                uati: nak.uati,
                full_uati: None,
                receive_ati: nak.uati,
                requests: nak
                    .requests
                    .into_iter()
                    .map(|request| proto::HrpdDefaultPacketRlpNakRequest {
                        first_erased: request.first_erased,
                        window_len: u32::from(request.window_len),
                    })
                    .collect(),
            })
            .collect(),
        traffic_channel_open_uatis: outcome.traffic_channel_open_uatis,
        traffic_channel_closed_uatis: outcome.traffic_channel_closed_uatis,
        traffic_releases: outcome
            .traffic_releases
            .into_iter()
            .map(|release| proto::HrpdTrafficReleaseRequest {
                uati: release.uati,
                full_uati: None,
                receive_ati: release.uati,
                mac_index: u32::from(release.mac_index),
            })
            .collect(),
        session_closed_uatis: outcome.session_closed_uatis,
    }
}

fn to_proto_state(s: SessionState) -> ProtoSessionState {
    match s {
        SessionState::Closed => ProtoSessionState::Closed,
        SessionState::AmpSetup => ProtoSessionState::AmpSetup,
        SessionState::Open => ProtoSessionState::Open,
        SessionState::Closing => ProtoSessionState::Closing,
    }
}

fn subtype_id(_s: crate::protocols::ProtocolSubtype) -> u32 {
    // Rev 0: every slot pinned to "Default" subtype = 0 (C.S0024-500).
    0
}

fn to_proto_session(
    s: &Session,
    hardware_id_response: Option<hrpd_air::HrpdHardwareIdResponse>,
) -> ProtoSession {
    let p = &s.protocols;
    ProtoSession {
        uati: s.uati.as_u32(),
        color_code: u32::from(s.color_code),
        state: to_proto_state(s.state) as i32,
        protocols: Some(ProtoNegotiated {
            air_link_management: subtype_id(p.air_link_management),
            session_management: subtype_id(p.session_management),
            // proto's address_management has no internal counterpart; alias to
            // session_management since Rev 0 defaults are all "Default".
            address_management: subtype_id(p.session_management),
            connection_layer: subtype_id(p.connection),
            security: subtype_id(p.security),
            mac: subtype_id(p.idle_state),
            physical_layer: subtype_id(p.stream),
        }),
        hardware_id_response: hardware_id_response.map(hardware_id_response_to_proto),
        full_uati: Some(an_uati_from_session(s)),
    }
}

#[tonic::async_trait]
impl AnService for AnServiceImpl {
    async fn get_sessions(
        &self,
        request: Request<GetSessionsRequest>,
    ) -> Result<Response<GetSessionsResponse>, Status> {
        let filter = ProtoSessionState::try_from(request.into_inner().state_filter)
            .unwrap_or(ProtoSessionState::Unspecified);
        let store = self.sessions.lock().await;
        let matching_sessions = store
            .values()
            .filter(|s| {
                filter == ProtoSessionState::Unspecified || to_proto_state(s.state) == filter
            })
            .cloned()
            .collect::<Vec<_>>();
        drop(store);

        let air = self.air.lock().await;
        let sessions = matching_sessions
            .iter()
            .map(|s| to_proto_session(s, air.hardware_id_for_uati(s.uati.as_u32()).cloned()))
            .collect();
        Ok(Response::new(GetSessionsResponse { sessions }))
    }

    async fn get_session(
        &self,
        request: Request<GetSessionRequest>,
    ) -> Result<Response<GetSessionResponse>, Status> {
        let uati = request.into_inner().uati;
        let store = self.sessions.lock().await;
        let session = store.get(&uati).cloned();
        drop(store);
        match session {
            Some(s) => {
                let air = self.air.lock().await;
                Ok(Response::new(GetSessionResponse {
                    session: Some(to_proto_session(
                        &s,
                        air.hardware_id_for_uati(s.uati.as_u32()).cloned(),
                    )),
                }))
            }
            None => Err(Status::not_found(format!("session {uati:#010x} not found"))),
        }
    }

    async fn get_uati_allocation(
        &self,
        request: Request<GetUatiAllocationRequest>,
    ) -> Result<Response<GetUatiAllocationResponse>, Status> {
        let req_color = request.into_inner().color_code;
        let allocator = self.uati.lock().await;
        let subnet = allocator.subnet();
        let allocator_color = u32::from(subnet.color_code);
        if req_color != WILDCARD_COLOR_CODE && allocator_color != req_color {
            return Err(Status::not_found(format!(
                "no allocator for color_code {req_color}"
            )));
        }
        let capacity = subnet.capacity() as u32;
        let in_use = allocator.issued_count() as u32;
        Ok(Response::new(GetUatiAllocationResponse {
            color_code: allocator_color,
            capacity,
            in_use,
            available: capacity.saturating_sub(in_use),
        }))
    }

    async fn get_session_events(
        &self,
        request: Request<GetSessionEventsRequest>,
    ) -> Result<Response<GetSessionEventsResponse>, Status> {
        let req = request.into_inner();
        let buffered = match &self.events {
            Some(sink) => sink.recent(req.uati, req.limit as usize),
            None => Vec::new(),
        };
        let records = buffered
            .into_iter()
            .map(|r| AnEventRecord {
                received_ms: r.received_ms,
                event: Some(match r.kind {
                    RecentKind::Session(e) => {
                        an_event_record::Event::Session(to_an_session_event(e))
                    }
                    RecentKind::Access(e) => an_event_record::Event::Access(to_an_access_event(e)),
                    RecentKind::Traffic(e) => {
                        an_event_record::Event::Traffic(to_an_traffic_event(e))
                    }
                }),
            })
            .collect();
        Ok(Response::new(GetSessionEventsResponse { records }))
    }

    async fn handle_access_indication(
        &self,
        request: Request<proto::HrpdAccessIndication>,
    ) -> Result<Response<proto::HrpdAccessOutcome>, Status> {
        let indication = access_indication_from_proto(request.into_inner())?;
        let mut air = self.air.lock().await;
        let mut allocator = self.uati.lock().await;
        let outcome = air
            .handle_access_indication(&indication, &mut allocator)
            .map_err(|e| Status::failed_precondition(e.to_string()))?;
        // Snapshot the session state of every AT this indication updated. A
        // brand-new AT's UATI is only known after the call, so read the
        // affected UATIs from the outcome.
        let affected_sessions: Vec<_> = outcome
            .affected_uatis
            .iter()
            .filter_map(|uati| {
                air.session_for_uati(*uati)
                    .and_then(|s| s.session().cloned())
            })
            .collect();
        drop(allocator);
        drop(air);

        if let Some(sink) = &self.events {
            let color = sink.color_code();
            let reason = if indication
                .messages
                .iter()
                .any(|m| matches!(m, hrpd_air::HrpdAccessMessage::ConnectionRequest(_)))
            {
                bus::HrpdAccessReason::ConnectionRequest
            } else if indication
                .messages
                .iter()
                .any(|m| matches!(m, hrpd_air::HrpdAccessMessage::UatiRequest(_)))
            {
                bus::HrpdAccessReason::UatiRequest
            } else if indication
                .messages
                .iter()
                .any(|m| matches!(m, hrpd_air::HrpdAccessMessage::RouteUpdate(_)))
            {
                bus::HrpdAccessReason::RouteUpdate
            } else if indication
                .messages
                .iter()
                .any(|m| matches!(m, hrpd_air::HrpdAccessMessage::KeepAlive))
            {
                bus::HrpdAccessReason::KeepAlive
            } else {
                bus::HrpdAccessReason::Unknown
            };
            let uati = outcome.affected_uatis.first().copied().unwrap_or_else(|| {
                if indication.ati.ati_type == hrpd_air::AccessTerminalIdentifierType::Uati {
                    indication.ati.value
                } else {
                    0
                }
            });
            for response in &outcome.hardware_id_responses {
                if let Some(identity) = bus_identity_from_hardware(response) {
                    sink.record_identity(uati, identity);
                }
            }
            sink.access(bus::HrpdAccessEvent {
                timestamp_ns: 0,
                access_signature: 0,
                reason: reason as i32,
                payload: indication.security_payload.clone(),
                color_code: color,
                direction: bus::HrpdDirection::Rx as i32,
                decoded_messages: indication
                    .messages
                    .iter()
                    .map(decoded_access_message)
                    .collect(),
                payload_length_bytes: indication.security_payload.len() as u32,
                uati,
                full_uati: bus_uati_for_event_uati(&affected_sessions, uati),
                receive_ati: receive_ati_for_event_uati(&affected_sessions, uati),
            });
            for request in &outcome.forward_signaling {
                let payload_length_bytes = request.payload.len() as u32;
                let uati = forward_signaling_uati(request);
                sink.traffic(bus::HrpdTrafficEvent {
                    timestamp_ns: 0,
                    uati,
                    full_uati: bus_uati_for_event_uati(&affected_sessions, uati),
                    receive_ati: receive_ati_for_event_uati(&affected_sessions, uati),
                    reason: bus::HrpdTrafficReason::FrameDecoded as i32,
                    mac_index: 0,
                    drc_value: 0,
                    payload: request.payload.clone(),
                    reverse_pilot_snr_db_tenths: 0,
                    direction: bus::HrpdDirection::Tx as i32,
                    decoded_messages: vec![decoded_forward_signaling(request)],
                    payload_length_bytes,
                });
            }
            for uati in &outcome.session_closed_uatis {
                sink.session(bus_session_event(
                    *uati,
                    bus::HrpdSessionReason::Closed,
                    color,
                ));
                sink.forget(*uati);
            }
        }

        for session in affected_sessions {
            let uati = session.uati.as_u32();
            let now_state = session.state;
            let full_uati = bus_uati_from_session(&session);
            let mut store = self.sessions.lock().await;
            let prior_state = store.get(&uati).map(|s| s.state);
            store.insert(uati, session);
            drop(store);
            if let Some(sink) = &self.events {
                // Emit a session event only on a real state transition so an
                // open session does not re-announce on every access capsule.
                let reason = match now_state {
                    SessionState::Open if prior_state != Some(SessionState::Open) => {
                        Some(bus::HrpdSessionReason::Opened)
                    }
                    SessionState::AmpSetup
                        if prior_state != Some(SessionState::AmpSetup)
                            && prior_state != Some(SessionState::Open) =>
                    {
                        Some(bus::HrpdSessionReason::UatiAssigned)
                    }
                    _ => None,
                };
                if let Some(reason) = reason {
                    let mut event = bus_session_event(uati, reason, sink.color_code());
                    event.full_uati = Some(full_uati);
                    sink.session(event);
                }
            }
        }
        if !outcome.session_closed_uatis.is_empty() {
            let mut store = self.sessions.lock().await;
            for uati in &outcome.session_closed_uatis {
                store.remove(uati);
                store.remove(&(*uati & 0x00ff_ffff));
            }
        }

        Ok(Response::new(outcome_to_proto(outcome)))
    }

    async fn handle_traffic_event(
        &self,
        request: Request<proto::HrpdTrafficEvent>,
    ) -> Result<Response<proto::HrpdTrafficOutcome>, Status> {
        let proto_event = request.into_inner();
        // Capture control-plane events before the proto event is consumed.
        // Stream 0 signaling is UI-visible; DRC feeds the rate-limited change
        // detector; reverse-pilot updates the tracked SNR. Bulk Stream 1 data
        // and forward traffic packets are bearer payload, not message-log
        // events.
        let mut stream0: Option<(u32, Vec<u8>)> = None;
        let mut drc: Option<(u32, u32, u32)> = None;
        let mut pilot: Option<(u32, u32, i32)> = None;
        match &proto_event.event {
            Some(proto::hrpd_traffic_event::Event::Stream0Signaling(ev)) => {
                stream0 = Some((ev.uati, ev.payload.clone()));
            }
            Some(proto::hrpd_traffic_event::Event::Stream1Packet(_)) => {}
            Some(proto::hrpd_traffic_event::Event::Drc(ev)) => {
                drc = Some((ev.uati, ev.mac_index, ev.drc_index));
            }
            Some(proto::hrpd_traffic_event::Event::Ack(_)) => {}
            Some(proto::hrpd_traffic_event::Event::ReversePilot(ev)) => {
                pilot = Some((ev.uati, ev.mac_index, ev.snr_db_tenths));
            }
            _ => {}
        }
        let event = traffic_event_from_proto(proto_event)?;
        let mut air = self.air.lock().await;
        let mut allocator = self.uati.lock().await;
        let outcome = air.handle_traffic_event_with_allocator(&event, &mut allocator);
        drop(allocator);
        drop(air);

        if let Some(sink) = &self.events {
            let color = sink.color_code();
            if let Some((uati, mac_index, snr)) = pilot {
                sink.maybe_emit_reverse_pilot_snr(uati, mac_index, snr);
            }
            for response in &outcome.hardware_id_responses {
                if let Some(identity) = bus_identity_from_hardware(&response.response) {
                    sink.record_identity(response.uati, identity);
                }
            }
            if let Some((uati, payload)) = stream0 {
                let mut decoded_messages = decoded_traffic_outcome(&outcome);
                if decoded_messages.is_empty() && outcome.stream0_invalid_count > 0 {
                    decoded_messages.push(decoded_message(
                        "UndecodedStream0Signaling",
                        format!("Undecoded Stream0 Signaling payload={}B", payload.len()),
                        u32::from(hrpd_air::DEFAULT_STREAM0_APPLICATION_PROTOCOL_TYPE),
                        payload.first().copied().map(u32::from).unwrap_or(0),
                        payload.clone(),
                    ));
                }
                if !decoded_messages.is_empty() {
                    sink.traffic(bus::HrpdTrafficEvent {
                        timestamp_ns: 0,
                        uati,
                        full_uati: None,
                        receive_ati: uati,
                        reason: bus::HrpdTrafficReason::FrameDecoded as i32,
                        mac_index: 0,
                        drc_value: 0,
                        payload: payload.clone(),
                        reverse_pilot_snr_db_tenths: 0,
                        direction: bus::HrpdDirection::Rx as i32,
                        decoded_messages,
                        payload_length_bytes: payload.len() as u32,
                    });
                }
            }
            if let Some((uati, mac_index, drc_value)) = drc {
                sink.maybe_emit_drc(uati, mac_index, drc_value);
            }
            for request in &outcome.forward_signaling {
                let uati = forward_signaling_uati(request);
                sink.traffic(bus::HrpdTrafficEvent {
                    timestamp_ns: 0,
                    uati,
                    full_uati: None,
                    receive_ati: uati,
                    reason: bus::HrpdTrafficReason::FrameDecoded as i32,
                    mac_index: 0,
                    drc_value: 0,
                    payload: request.payload.clone(),
                    reverse_pilot_snr_db_tenths: 0,
                    direction: bus::HrpdDirection::Tx as i32,
                    decoded_messages: vec![decoded_forward_signaling(request)],
                    payload_length_bytes: request.payload.len() as u32,
                });
            }
            for uati in &outcome.traffic_channel_closed_uatis {
                sink.traffic(bus::HrpdTrafficEvent {
                    timestamp_ns: 0,
                    uati: *uati,
                    full_uati: None,
                    receive_ati: *uati,
                    reason: bus::HrpdTrafficReason::ConnectionClose as i32,
                    mac_index: 0,
                    drc_value: 0,
                    payload: Vec::new(),
                    reverse_pilot_snr_db_tenths: 0,
                    direction: bus::HrpdDirection::Rx as i32,
                    decoded_messages: vec![decoded_message(
                        "ConnectionClose",
                        "ConnectionClose",
                        u32::from(hrpd_air::DEFAULT_CONNECTED_STATE_PROTOCOL_TYPE),
                        1,
                        Vec::new(),
                    )],
                    payload_length_bytes: 0,
                });
                sink.forget(*uati);
            }
            for uati in &outcome.session_closed_uatis {
                sink.session(bus_session_event(
                    *uati,
                    bus::HrpdSessionReason::Closed,
                    color,
                ));
                sink.forget(*uati);
            }
        }
        if !outcome.session_closed_uatis.is_empty() {
            let mut store = self.sessions.lock().await;
            for uati in &outcome.session_closed_uatis {
                store.remove(uati);
                store.remove(&(*uati & 0x00ff_ffff));
            }
        }

        Ok(Response::new(traffic_outcome_to_proto(outcome)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::air::HrpdAirController;
    use crate::protocols::REV0_DEFAULTS;
    use crate::subnet::UatiSubnet;
    use crate::uati::Uati;

    fn make_state() -> AnServiceImpl {
        let sessions: SessionStore = Arc::new(Mutex::new(HashMap::new()));
        let uati = Arc::new(Mutex::new(UatiAllocator::new(UatiSubnet {
            color_code: 7,
            uati104: [0; 13],
            subnet_mask: 24,
        })));
        let air = Arc::new(Mutex::new(HrpdAirController::new(7)));
        AnServiceImpl::new_with_air(sessions, uati, air)
    }

    async fn request_test_uati(svc: &AnServiceImpl) -> u32 {
        let resp = svc
            .handle_access_indication(Request::new(proto::HrpdAccessIndication {
                absolute_chip: 123_456,
                color_code: 7,
                sector_pilot_pn: 0,
                session_configuration_token: 0,
                ati: Some(proto::AccessTerminalIdentifier {
                    ati_type: proto::AccessTerminalIdentifierType::Rati as i32,
                    value: 0x5232_af53,
                }),
                security_layer_format: false,
                connection_layer_format: true,
                security_payload: Vec::new(),
                messages: vec![proto::HrpdAccessMessage {
                    message: Some(proto::hrpd_access_message::Message::UatiRequest(
                        proto::HrpdUatiRequest {
                            transaction_id: 0x9c,
                        },
                    )),
                }],
            }))
            .await
            .unwrap()
            .into_inner();

        resp.forward_signaling
            .iter()
            .find(|request| request.protocol_type == 0x11)
            .and_then(|request| request.uati)
            .expect("expected UATIAssignment")
    }

    fn receive_ati_for_test_uati(uati: u32) -> u32 {
        0x0700_0000 | (uati & 0x00ff_ffff)
    }

    #[test]
    fn stream0_session_configuration_complete_decodes_for_events() {
        let message = decoded_stream0_control_message(
            hrpd_air::DEFAULT_SESSION_CONFIGURATION_PROTOCOL_TYPE,
            Some(SESSION_CONFIGURATION_COMPLETE),
            &[SESSION_CONFIGURATION_COMPLETE, 0x44],
        );

        assert_eq!(message.type_name, "SessionConfigurationComplete");
        assert_eq!(
            message.summary,
            "SessionConfigurationComplete transaction=0x44 attrs=0B"
        );
        assert_eq!(
            message.protocol_type,
            u32::from(hrpd_air::DEFAULT_SESSION_CONFIGURATION_PROTOCOL_TYPE)
        );
        assert_eq!(
            message.message_id,
            u32::from(SESSION_CONFIGURATION_COMPLETE)
        );
    }

    #[test]
    fn stream0_protocol_configuration_request_decodes_for_events() {
        let message = decoded_stream0_control_message(
            SESSION_PROTOCOL_STREAM,
            Some(SESSION_CONFIGURATION_REQUEST),
            &[SESSION_CONFIGURATION_REQUEST, 0x21, 0x04, 0x00, 0x15],
        );

        assert_eq!(message.type_name, "StreamConfigurationRequest");
        assert_eq!(
            message.summary,
            "StreamConfigurationRequest transaction=0x21 attrs=3B"
        );
        assert_eq!(message.protocol_type, u32::from(SESSION_PROTOCOL_STREAM));
        assert_eq!(message.message_id, u32::from(SESSION_CONFIGURATION_REQUEST));
    }

    #[test]
    fn unhandled_stream0_protocol_stays_unknown_for_events() {
        let message = decoded_stream0_control_message(0x7f, Some(0xaa), &[0xaa]);

        assert_eq!(message.type_name, "Unknown");
        assert_eq!(message.summary, "Unknown protocol=0x7F message=0xAA");
        assert_eq!(message.protocol_type, 0x7f);
        assert_eq!(message.message_id, 0xaa);
        assert_eq!(message.payload, vec![0xaa]);
    }

    #[test]
    fn forward_access_channel_ack_decodes_for_events() {
        let message =
            decoded_forward_control_message(DEFAULT_ACCESS_CHANNEL_MAC_PROTOCOL_TYPE, &[0x00])
                .expect("ACAck should decode");

        assert_eq!(message.type_name, "AccessChannelAck");
        assert_eq!(message.summary, "AccessChannelMAC ACAck");
        assert_eq!(
            message.protocol_type,
            u32::from(DEFAULT_ACCESS_CHANNEL_MAC_PROTOCOL_TYPE)
        );
        assert_eq!(message.message_id, 0);
        assert!(message.payload.is_empty());
    }

    #[test]
    fn forward_traffic_channel_assignment_decodes_for_events() {
        let assignment = hrpd_air::HrpdTrafficChannelAssignment::single_pilot(
            3,
            Some(hrpd_air::HrpdChannelRecord {
                system_type: 0,
                band_class: 0,
                channel_number: 630,
            }),
            0,
            7,
        );
        let payload = assignment.encode_subtype0_route_update();
        let message =
            decoded_forward_control_message(hrpd_air::DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE, &payload)
                .expect("TrafficChannelAssignment should decode");

        assert_eq!(message.type_name, "TrafficChannelAssignment");
        assert_eq!(
            message.summary,
            "TrafficChannelAssignment seq=3 pilots=1 pilot_pn=0 mac=7 drc_cover=1 channel=0/0/630 frame_offset=0 drc_len=3 drc_gain=+6.0dB ack_gain=+0.0dB"
        );
        assert_eq!(
            message.protocol_type,
            u32::from(hrpd_air::DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE)
        );
        assert_eq!(
            message.message_id,
            u32::from(TRAFFIC_CHANNEL_ASSIGNMENT_MESSAGE_ID)
        );
        assert_eq!(message.payload, payload);
    }

    #[test]
    fn forward_traffic_channel_assignment_subtype1_is_named_for_events() {
        let assignment = hrpd_air::HrpdTrafficChannelAssignment::single_pilot(
            3,
            Some(hrpd_air::HrpdChannelRecord {
                system_type: 0,
                band_class: 0,
                channel_number: 630,
            }),
            0,
            7,
        );
        let payload = assignment.encode_subtype1_route_update();
        let message =
            decoded_forward_control_message(hrpd_air::DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE, &payload)
                .expect("TrafficChannelAssignment should be named");

        assert_eq!(message.type_name, "TrafficChannelAssignment");
        assert_eq!(message.summary, "TrafficChannelAssignment payload=21B");
        assert_eq!(message.payload, payload);
    }

    #[tokio::test]
    async fn get_sessions_returns_seeded_session() {
        let svc = make_state();
        {
            let mut s = svc.sessions.lock().await;
            let mut sess = Session::new(
                Uati::from_compact(0x0034_5678, [0; 13], 7, 24),
                7,
                REV0_DEFAULTS,
            );
            sess.state = SessionState::Open;
            s.insert(0x0034_5678, sess);
        }
        let resp = svc
            .get_sessions(Request::new(GetSessionsRequest {
                state_filter: ProtoSessionState::Unspecified as i32,
            }))
            .await
            .unwrap();
        let r = resp.into_inner();
        assert_eq!(r.sessions.len(), 1);
        assert_eq!(r.sessions[0].uati, 0x0034_5678);
        assert_eq!(r.sessions[0].state(), ProtoSessionState::Open);
    }

    #[tokio::test]
    async fn get_session_includes_cached_hardware_id_response() {
        let svc = make_state();
        let assigned_uati = request_test_uati(&svc).await;
        let receive_ati = receive_ati_for_test_uati(assigned_uati);
        svc.handle_access_indication(Request::new(proto::HrpdAccessIndication {
            absolute_chip: 124_456,
            color_code: 7,
            sector_pilot_pn: 0,
            session_configuration_token: 0,
            ati: Some(proto::AccessTerminalIdentifier {
                ati_type: proto::AccessTerminalIdentifierType::Uati as i32,
                value: receive_ati,
            }),
            security_layer_format: false,
            connection_layer_format: true,
            security_payload: Vec::new(),
            messages: vec![proto::HrpdAccessMessage {
                message: Some(proto::hrpd_access_message::Message::UatiComplete(
                    proto::HrpdUatiComplete {
                        message_sequence: 0,
                        upper_old_uati: Vec::new(),
                        reserved_zero: true,
                    },
                )),
            }],
        }))
        .await
        .unwrap();
        svc.handle_access_indication(Request::new(proto::HrpdAccessIndication {
            absolute_chip: 125_456,
            color_code: 7,
            sector_pilot_pn: 0,
            session_configuration_token: 0,
            ati: Some(proto::AccessTerminalIdentifier {
                ati_type: proto::AccessTerminalIdentifierType::Uati as i32,
                value: receive_ati,
            }),
            security_layer_format: false,
            connection_layer_format: true,
            security_payload: Vec::new(),
            messages: vec![proto::HrpdAccessMessage {
                message: Some(proto::hrpd_access_message::Message::HardwareIdResponse(
                    proto::HrpdHardwareIdResponse {
                        transaction_id: 1,
                        hardware_id_type: 0x00ff_ff,
                        hardware_id_value: vec![0xa1, 0xb2, 0xc3, 0xd4, 0xe5, 0xf6, 0x70],
                    },
                )),
            }],
        }))
        .await
        .unwrap();

        let session = svc
            .get_session(Request::new(GetSessionRequest {
                uati: assigned_uati,
            }))
            .await
            .unwrap()
            .into_inner()
            .session
            .expect("session");
        let hardware = session.hardware_id_response.expect("hardware id");
        assert_eq!(hardware.hardware_id_type, 0x00ff_ff);
        assert_eq!(
            hardware.hardware_id_value,
            vec![0xa1, 0xb2, 0xc3, 0xd4, 0xe5, 0xf6, 0x70]
        );
    }

    #[tokio::test]
    async fn get_session_not_found() {
        let svc = make_state();
        let err = svc
            .get_session(Request::new(GetSessionRequest { uati: 0xDEAD }))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::NotFound);
    }

    #[tokio::test]
    async fn get_uati_allocation_returns_capacity() {
        let svc = make_state();
        let resp = svc
            .get_uati_allocation(Request::new(GetUatiAllocationRequest { color_code: 7 }))
            .await
            .unwrap();
        let r = resp.into_inner();
        assert_eq!(r.color_code, 7);
        assert_eq!(r.capacity, 0x00ff_ffff);
        assert_eq!(r.in_use, 0);
        assert_eq!(r.available, 0x00ff_ffff);
    }

    #[tokio::test]
    async fn get_uati_allocation_wrong_color_is_not_found() {
        let svc = make_state();
        let err = svc
            .get_uati_allocation(Request::new(GetUatiAllocationRequest { color_code: 99 }))
            .await
            .unwrap_err();
        assert_eq!(err.code(), tonic::Code::NotFound);
    }

    #[tokio::test]
    async fn handle_access_indication_returns_uati_assignment() {
        let svc = make_state();
        let resp = svc
            .handle_access_indication(Request::new(proto::HrpdAccessIndication {
                absolute_chip: 123_456,
                color_code: 7,
                sector_pilot_pn: 0,
                session_configuration_token: 0,
                ati: Some(proto::AccessTerminalIdentifier {
                    ati_type: proto::AccessTerminalIdentifierType::Rati as i32,
                    value: 0x5232_af53,
                }),
                security_layer_format: false,
                connection_layer_format: true,
                security_payload: Vec::new(),
                messages: vec![proto::HrpdAccessMessage {
                    message: Some(proto::hrpd_access_message::Message::UatiRequest(
                        proto::HrpdUatiRequest {
                            transaction_id: 0x9c,
                        },
                    )),
                }],
            }))
            .await
            .unwrap()
            .into_inner();

        assert_eq!(resp.forward_signaling.len(), 2);
        assert!(
            resp.forward_signaling
                .iter()
                .any(|request| request.protocol_type == 0x02 && request.payload == vec![0x00])
        );
        let request = resp
            .forward_signaling
            .iter()
            .find(|request| request.protocol_type == 0x11)
            .expect("expected UATIAssignment");
        let assigned_uati = request.uati.expect("assigned UATI");
        let receive_ati = receive_ati_for_test_uati(assigned_uati);
        assert_eq!(assigned_uati, 0x0005_8001);
        assert_eq!(request.protocol_type, 0x11);
        assert_eq!(
            request.payload,
            vec![0x01, 0x00, 0x00, 0x07, 0x05, 0x80, 0x01, 0x00]
        );
        let sessions = svc.sessions.lock().await;
        assert!(sessions.contains_key(&assigned_uati));
        assert_eq!(receive_ati, 0x0705_8001);
    }

    #[tokio::test]
    async fn handle_traffic_event_returns_a8_uplink_for_known_uati() {
        let svc = make_state();
        let assigned_uati = request_test_uati(&svc).await;
        let receive_ati = receive_ati_for_test_uati(assigned_uati);

        let resp = svc
            .handle_traffic_event(Request::new(proto::HrpdTrafficEvent {
                event: Some(proto::hrpd_traffic_event::Event::Stream1Packet(
                    proto::HrpdStream1PacketEvent {
                        uati: receive_ati,
                        full_uati: None,
                        receive_ati,
                        payload: vec![0x7e, 0xff, 0x03],
                        sequence: 0,
                    },
                )),
            }))
            .await
            .unwrap()
            .into_inner();

        assert_eq!(resp.accepted_event_count, 1);
        assert_eq!(resp.dropped_event_count, 0);
        assert_eq!(resp.a8_uplink.len(), 1);
        assert_eq!(resp.a8_uplink[0].uati, receive_ati);
        assert_eq!(resp.a8_uplink[0].payload, [0x7e, 0xff, 0x03]);
    }
}
