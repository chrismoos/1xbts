//! AN-side consumer for decoded HRPD air-interface events.

mod crypto;
mod session_config;
mod stream0_codec;

use std::{
    collections::HashMap,
    time::{Duration, Instant},
};

use crate::rlp;

use crypto::{dh_compute_session_key, dh_key_signature, new_dh_key_exchange, random_u16};
use session_config::*;
use stream0_codec::*;

use cdma_common::hrpd::air::{
    AccessTerminalIdentifier, AccessTerminalIdentifierType,
    DEFAULT_ADDRESS_MANAGEMENT_PROTOCOL_TYPE, DEFAULT_CONNECTED_STATE_PROTOCOL_TYPE,
    DEFAULT_IDLE_STATE_PROTOCOL_TYPE, DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE,
    DEFAULT_SESSION_CONFIGURATION_PROTOCOL_TYPE, DEFAULT_SESSION_MANAGEMENT_PROTOCOL_TYPE,
    DEFAULT_STREAM0_APPLICATION_PROTOCOL_TYPE, HrpdAccessIndication, HrpdAccessMessage,
    HrpdChannelRecord, HrpdConnectionClose, HrpdConnectionRequest, HrpdDefaultPacketRlpReset,
    HrpdDefaultPacketRlpResetAck, HrpdDefaultSignalingReset, HrpdDefaultSignalingResetAck,
    HrpdForwardSignalingRequest, HrpdForwardTrafficPacket, HrpdHardwareIdResponse,
    HrpdRouteUpdate as AirRouteUpdate, HrpdSessionClose, HrpdTrafficAssignmentRequest,
    HrpdTrafficChannelAssignment, HrpdTrafficChannelComplete, HrpdTrafficEvent,
    HrpdTrafficReleaseRequest, HrpdUatiComplete, HrpdUatiRequest, HrpdUatiSubnetAssignment,
    default_reverse_traffic_long_code_masks, hrpd_connection_close_reason_name,
    hrpd_protocol_reference_from_more_info, hrpd_session_close_reason_name,
};
use cdma_common::hrpd::traffic::{
    DEFAULT_PACKET_STREAM_ID, DEFAULT_PACKET_STREAM1_APPLICATION_PROTOCOL_TYPE,
    DEFAULT_PACKET_STREAM2_APPLICATION_PROTOCOL_TYPE,
    DEFAULT_PACKET_STREAM3_APPLICATION_PROTOCOL_TYPE, MacFlowGrant,
    default_packet_stream_protocol_type,
    default_reverse_traffic_mac_grant_ftc_payload_bits_for_mac_subtype,
    default_reverse_traffic_mac_rtc_ack_ftc_payload_bits_for_mac_subtype,
    default_signaling_ftc_payload_bits_with_ack_for_mac_subtype,
    default_signaling_slp_d_ack_ftc_payload_bits_for_mac_subtype,
    default_signaling_slp_reset_ftc_payload_bits_for_mac_subtype,
    implemented_forward_traffic_payload_bits_for_drc,
};
use cdma_common::time;
use num_bigint::BigUint;
use sha1::{Digest, Sha1};

use crate::session::SessionState;
use crate::state_machine::{
    InboundSessionMessage, OutboundSessionMessage, SessionStateMachine, StateMachineError,
};
use crate::subnet::UatiAllocator;
use crate::uati::Uati;

// Setup RTCAck is prequeued at connection setup; the scheduler releases it only
// on the exact DRC governing the packet start slot.
const HRPD_RTC_ACK_DRC_INDEX: u8 = 0x1;
const HRPD_DEFAULT_FTC_PHYSICAL_BITS: usize = 1024;
const DEFAULT_PACKET_XON_REQUEST: u8 = 0x07;
const DEFAULT_PACKET_XON_RESPONSE: u8 = 0x08;
const DEFAULT_PACKET_XOFF_REQUEST: u8 = 0x09;
const DEFAULT_PACKET_XOFF_RESPONSE: u8 = 0x0a;
const DEFAULT_PACKET_DATA_READY: u8 = 0x0b;
const DEFAULT_PACKET_DATA_READY_ACK: u8 = 0x0c;
const DEFAULT_PACKET_RLP_RESET: u8 = 0x00;
const DEFAULT_PACKET_RLP_RESET_ACK: u8 = 0x01;
const DEFAULT_PACKET_RLP_NAK: u8 = 0x02;
// C.S0024-A v3.0 Table 3.6.1-1: TRLPAbort = 500 ms.
const DEFAULT_PACKET_RLP_ABORT: Duration = Duration::from_millis(500);
const DEFAULT_SIGNALING_SLP_RESET: u8 = 0x00;
const DEFAULT_SIGNALING_SLP_RESET_ACK: u8 = 0x01;
const CONNECTED_STATE_CONNECTION_CLOSE: u8 = 0x00;
// Stream-0 signaling message IDs, per their owning protocol (C.S0024-0 §8.3).
const ROUTE_UPDATE_MESSAGE_ID: u8 = 0x00;
const TRAFFIC_CHANNEL_COMPLETE_MESSAGE_ID: u8 = 0x02;
const SESSION_CLOSE_MESSAGE_ID: u8 = 0x01;
const SESSION_KEEP_ALIVE_REQUEST_MESSAGE_ID: u8 = 0x02;
const SESSION_KEEP_ALIVE_RESPONSE_MESSAGE_ID: u8 = 0x03;
const HARDWARE_ID_RESPONSE_MESSAGE_ID: u8 = 0x04;
const CONNECTION_CLOSE_REASON_NORMAL_UNSPECIFIED: u8 = 0x00;
const SESSION_CLOSE_REASON_SESSION_LOST: u8 = 0x06;
const DH_KEY_REQUEST: u8 = 0x00;
const DH_KEY_RESPONSE: u8 = 0x01;
const DH_AN_KEY_COMPLETE: u8 = 0x02;
const DH_AT_KEY_COMPLETE: u8 = 0x03;
// C.S0024-400-C §2.6.5.3.1: KeyRequest.Timeout is the maximum time
// the AN requires before sending ANKeyComplete. The common primitive table
// gives 3.5 s for the AN-side response timer, so advertise a whole-second
// budget that survives FTC retransmission and reverse SLP-F fragmentation.
const DH_KEY_EXCHANGE_TIMEOUT_SECONDS: u8 = 4;
const DH_KEY_LENGTH_OCTETS_768: usize = 96;
const SESSION_CONFIGURATION_COMPLETE: u8 = 0x00;
const SESSION_CONFIGURATION_START: u8 = 0x01;
const SESSION_SOFT_CONFIGURATION_COMPLETE: u8 = 0x02;
const SESSION_CONFIGURATION_REQUEST: u8 = 0x50;
const SESSION_CONFIGURATION_RESPONSE: u8 = 0x51;

const SESSION_PROTOCOL_PHYSICAL_LAYER: u8 = 0x00;
const SESSION_PROTOCOL_CONTROL_CHANNEL_MAC: u8 = 0x01;
const SESSION_PROTOCOL_ACCESS_CHANNEL_MAC: u8 = 0x02;
const SESSION_PROTOCOL_FORWARD_TRAFFIC_CHANNEL_MAC: u8 = 0x03;
const SESSION_PROTOCOL_REVERSE_TRAFFIC_CHANNEL_MAC: u8 = 0x04;
const SESSION_PROTOCOL_KEY_EXCHANGE: u8 = 0x05;
const SESSION_PROTOCOL_AUTHENTICATION: u8 = 0x06;
const SESSION_PROTOCOL_ENCRYPTION: u8 = 0x07;
const SESSION_PROTOCOL_SECURITY: u8 = 0x08;
const SESSION_PROTOCOL_AIR_LINK_MANAGEMENT: u8 = 0x0a;
const SESSION_PROTOCOL_INITIALIZATION_STATE: u8 = 0x0b;
const SESSION_PROTOCOL_OVERHEAD_MESSAGES: u8 = 0x0f;
const SESSION_PROTOCOL_STREAM: u8 = 0x13;
const SESSION_PROTOCOL_DEFAULT_PACKET_FIRST: u8 = 0x15;
const SESSION_PROTOCOL_DEFAULT_PACKET_LAST: u8 = 0x17;
const SESSION_PROTOCOL_MULTIMODE_CAPABILITY_DISCOVERY: u8 = 0x1b;

const SESSION_SUBTYPE_DEFAULT: u16 = 0x0000;
const SESSION_SUBTYPE_REV0: u16 = 0x0001;
/// Enhanced CC/AC/FTC MAC subtype (C.S0024-A §10.3/§10.5/§10.7).
const SESSION_SUBTYPE_ENHANCED: u16 = 0x0001;
/// Subtype 2 Physical Layer (C.S0024-A ch. 13).
const SESSION_SUBTYPE_PHYS_SUBTYPE2: u16 = 0x0002;
/// Subtype 3 Reverse Traffic Channel MAC (C.S0024-A §10.11).
const SESSION_SUBTYPE_RTC_MAC_SUBTYPE3: u16 = 0x0003;
/// Generic Attribute Update Protocol message IDs (C.S0024-A §14.10.3).
const ATTRIBUTE_UPDATE_REQUEST: u8 = 0x52;
const ATTRIBUTE_UPDATE_REJECT: u8 = 0x54;
const SESSION_ATTRIBUTE_PERSONALITY_COUNT: [u8; 2] = [0x01, 0x10];
const SESSION_PERSONALITY_COUNT_DEFAULT: u16 = 1;
const IDLE_STATE_ATTRIBUTE_PREFERRED_CONTROL_CHANNEL_CYCLE: u8 = 0x00;
// C.S0024-400-C Enhanced Idle defaults SlotCycle=0x09. Table 1.5.6.1.6 maps
// that to 2^(0x09-0x07) * 768 slots, i.e. 12 Control Channel cycles.
const ENHANCED_IDLE_DEFAULT_PAGE_PERIOD_CYCLES: u16 = 12;
const DEFAULT_PACKET_SERVICE_NETWORK_SUBTYPE: u16 = 0x0002;
const MCD_SIMULTANEOUS_COMMON_CHANNEL_TRANSMIT: u8 = 0xff;
const MCD_SIMULTANEOUS_DEDICATED_CHANNEL_TRANSMIT: u8 = 0xfe;
const MCD_SIMULTANEOUS_COMMON_CHANNEL_RECEIVE: u8 = 0xfd;
const MCD_SIMULTANEOUS_DEDICATED_CHANNEL_RECEIVE: u8 = 0xfc;
const MCD_HYBRID_MS_AT: u8 = 0xfb;
const MCD_RECEIVER_DIVERSITY: u8 = 0xfa;

const HRPD_REVERSE_TRAFFIC_SLOT_CHIPS: u64 = 2048;
// C.S0024-0 §8.5.8 sets TRTCMPANSetup to 1.0 s. Live SDR keeps the AN guard at
// 3.0 s so a near-boundary ConnectionRequest retry can refresh the assignment.
const HRPD_RTCMP_AN_SETUP_SLOTS: u64 = 1800; // TRTCMPANSetup = 3.0 s
const HRPD_RTCMP_AT_SETUP_SLOTS: u64 = 900; // TRTCMPATSetup = 1.5 s
const HRPD_RTCMP_AN_SETUP: Duration = Duration::from_millis(3000);
// C.S0024-0 §5.2.7 defines the default TSMPClose as 0x0CA8 minutes.
const HRPD_SESSION_IDLE_TIMEOUT: Duration = Duration::from_secs(0x0ca8 * 60);
// C.S0024-400-C §1.7.6.3.4 / Table 1.7.6.6-1: the AN Close State completes
// on a replying ConnectionClose or TCSPClose expiry. TCSPClose is 1.5 s.
const HRPD_CSP_CLOSE_TIMER: Duration = Duration::from_millis(1500);
const HRPD_FIRST_TRAFFIC_MAC_INDEX: u8 = 5;
const HRPD_LAST_TRAFFIC_MAC_INDEX: u8 = 63;

// RTCAck is SLP Reliable (C.S0024-0 v4.0 §8.5.6.3.2): the AN retransmits until
// the AT acknowledges. The reverse DRC stream is the cadence source, but the
// decoder emits one event per DRCLength window, not one per 26.67 ms frame.
// Use absolute reverse-slot spacing so the RTCAck physical packet can finish
// and its ACK-channel feedback can reach the scheduler before the next logical
// duplicate is queued.
const RTC_ACK_RETRANSMIT_MIN_SLOTS: u64 = 240;
const RTC_ACK_MAX_RETRANSMITS: u32 = 3;
// Rev A subtype-3 RTCMAC starts at its autonomous T2P minimum. Keep the
// default active flows scheduled above that minimum once traffic is open:
// flow 0 carries Stream 0 signaling and flow 1 carries packet data.
const RTC_MAC_GRANT_RETRANSMIT_MIN_SLOTS: u64 = 120;
const HRPD_AUTONOMOUS_SIGNALING_GRANT_MAC_FLOW_ID: u8 = 0x0;
// Flow 0 is granted at the packet-flow level because early DefaultPacket and
// control traffic can arrive before AssociatedFlows proves it has moved off
// the signaling flow.
const HRPD_AUTONOMOUS_SIGNALING_GRANT_T2P_INFLOW_QUARTER_DB: u8 = 0x50;
const HRPD_AUTONOMOUS_SIGNALING_GRANT_BUCKET_LEVEL_QUARTER_DB: u8 = 0x6c;
const HRPD_AUTONOMOUS_PACKET_GRANT_MAC_FLOW_ID: u8 = 0x1;
const HRPD_AUTONOMOUS_PACKET_GRANT_T2P_INFLOW_QUARTER_DB: u8 = 0x78;
const HRPD_AUTONOMOUS_PACKET_GRANT_BUCKET_LEVEL_QUARTER_DB: u8 = 0x6c;
// TT2PHold is in frames and the AT expands it to four subframes per frame.
// 0x0f therefore holds for 240 slots (400 ms), twice the refresh interval.
const HRPD_AUTONOMOUS_GRANT_TT2P_HOLD_FRAMES: u8 = 0x0f;
// C.S0024-0 §8.4.6.1.4.1.2 governs the physical forward packet rate; if we
// cannot decode an AT-requested DRC, there is no spec-compliant fallback rate
// for an FTC RTCAck.
// C.S0024-500-C §1.6.6: TSLPWaitAck=400 ms and NSLPAttempt=3 for
// reliable-delivery SLP-D payloads. Header-only SLP-D ACK packets are
// best-effort and intentionally do not enter this retransmission buffer.
const STREAM0_SLP_D_WAIT_ACK: Duration = Duration::from_millis(400);
const STREAM0_SLP_D_MAX_ATTEMPTS: u32 = 3;
const STREAM0_SLP_RESET_WAIT_ACK_SLOTS: u64 = 240;
const STREAM0_SLP_RESET_MAX_ATTEMPTS: u32 = 3;
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HrpdAccessOutcome {
    pub route_updates: Vec<AirRouteUpdate>,
    pub session_outbound: Vec<OutboundSessionMessage>,
    pub forward_signaling: Vec<HrpdForwardSignalingRequest>,
    pub forward_traffic: Vec<HrpdForwardTrafficPacket>,
    pub traffic_assignments: Vec<HrpdTrafficAssignmentRequest>,
    pub traffic_releases: Vec<HrpdTrafficReleaseRequest>,
    pub session_closed_uatis: Vec<u32>,
    pub uati_completes: Vec<HrpdUatiComplete>,
    pub connection_requests: Vec<HrpdConnectionRequest>,
    pub traffic_channel_completes: Vec<HrpdTrafficChannelComplete>,
    pub session_closes: Vec<HrpdSessionClose>,
    pub hardware_id_responses: Vec<HrpdHardwareIdResponse>,
    pub connection_requested: bool,
    pub keepalive_seen: bool,
    pub unknown_messages: usize,
    // Session UATIs of the ATs whose state this indication updated. A
    // brand-new AT's UATI is only known after processing, so callers that
    // snapshot per-AT session state read it from here rather than guessing
    // from the indication's ATI.
    pub affected_uatis: Vec<u32>,
}

impl HrpdAccessOutcome {
    fn empty() -> Self {
        Self {
            route_updates: Vec::new(),
            session_outbound: Vec::new(),
            forward_signaling: Vec::new(),
            forward_traffic: Vec::new(),
            traffic_assignments: Vec::new(),
            traffic_releases: Vec::new(),
            session_closed_uatis: Vec::new(),
            uati_completes: Vec::new(),
            connection_requests: Vec::new(),
            traffic_channel_completes: Vec::new(),
            session_closes: Vec::new(),
            hardware_id_responses: Vec::new(),
            connection_requested: false,
            keepalive_seen: false,
            unknown_messages: 0,
            affected_uatis: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HrpdA8UplinkPacket {
    pub uati: u32,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HrpdTrafficHardwareIdResponse {
    pub uati: u32,
    pub response: HrpdHardwareIdResponse,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HrpdDefaultPacketDataReadyAckEvent {
    pub uati: u32,
    pub transaction_id: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HrpdDefaultPacketRlpNakEvent {
    pub uati: u32,
    pub requests: Vec<cdma_common::hrpd::air::HrpdDefaultPacketRlpNakRequest>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HrpdDefaultPacketStreamConfiguration {
    pub uati: u32,
    pub stream_id: u8,
    pub protocol_type: u8,
    pub application_subtype: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HrpdSessionConfigurationCompleteEvent {
    pub uati: u32,
    pub physical_layer_subtype: u16,
    pub forward_traffic_mac_subtype: u16,
    pub idle_preferred_control_channel_cycle: Option<u16>,
    pub idle_page_period_cycles: u16,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct HrpdTrafficOutcome {
    pub accepted_event_count: usize,
    pub dropped_event_count: usize,
    pub unknown_session_count: usize,
    pub reverse_pilot_count: usize,
    pub drc_count: usize,
    pub ack_count: usize,
    pub stream0_signaling_count: usize,
    pub stream0_ack_only_count: usize,
    pub stream0_fragment_in_progress_count: usize,
    pub stream0_invalid_count: usize,
    pub decoded_stream0_messages: Vec<HrpdAccessMessage>,
    pub a8_uplink: Vec<HrpdA8UplinkPacket>,
    pub forward_signaling: Vec<HrpdForwardSignalingRequest>,
    pub forward_traffic: Vec<HrpdForwardTrafficPacket>,
    pub hardware_id_responses: Vec<HrpdTrafficHardwareIdResponse>,
    pub session_configuration_pending_uatis: Vec<u32>,
    pub session_configuration_complete_uatis: Vec<u32>,
    pub session_configuration_complete_events: Vec<HrpdSessionConfigurationCompleteEvent>,
    pub default_packet_flow_open_uatis: Vec<u32>,
    pub default_packet_flow_closed_uatis: Vec<u32>,
    pub default_packet_data_ready_acks: Vec<HrpdDefaultPacketDataReadyAckEvent>,
    pub default_packet_stream_configurations: Vec<HrpdDefaultPacketStreamConfiguration>,
    pub default_packet_rlp_reset_uatis: Vec<u32>,
    pub default_packet_rlp_naks: Vec<HrpdDefaultPacketRlpNakEvent>,
    pub traffic_channel_open_uatis: Vec<u32>,
    pub traffic_channel_closed_uatis: Vec<u32>,
    pub traffic_releases: Vec<HrpdTrafficReleaseRequest>,
    pub session_closed_uatis: Vec<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingTrafficAssignment {
    session_uati: u32,
    traffic_uati: u32,
    connection_request_transaction_id: u8,
    assignment: HrpdTrafficChannelAssignment,
    tca_reliable_sequence: Option<u8>,
    traffic_request: HrpdTrafficAssignmentRequest,
    active: bool,
    // V(S) captured when RTCAck was first composed for this assignment.
    // None until the first RTCAck send; retransmissions reuse the same value
    // so the AT's SLP-D V(R) treats them as duplicates of one logical message.
    rtc_ack_vs: Option<u8>,
    // Set whenever an RTCAck needs to be (re)emitted on the next reverse-pilot
    // event. Cleared after a send. Re-armed when the AN retransmits TCA in
    // response to a duplicate ConnectionRequest from the same AT.
    rtc_ack_needs_send: bool,
    // True after the BTS has reported a validated reverse traffic pilot for
    // this assignment. ConnectionRequest retries after this point can resend
    // RTCAck immediately; the traffic RX finger emits the acquisition event
    // once per assignment.
    rtc_acquired: bool,
    // Accepted reverse DRC decodes since the last RTCAck send; drives the
    // reliable-SLP retransmit cadence while no TrafficChannelComplete arrives.
    drc_events_since_rtc_ack: u32,
    // DRC-driven RTCAck retransmissions so far, capped at
    // RTC_ACK_MAX_RETRANSMITS. Retransmitted ConnectionRequests preserve the
    // same setup attempt, including this counter and the assigned MAC.
    rtc_ack_retransmits: u32,
    // Set once the reverse ACK channel reports that the AT physically decoded
    // a setup RTCAck forward traffic packet. For Rev A subtype-3, RTCAck is
    // the RTC MAC LinkAcquired transition; TrafficChannelComplete may still
    // arrive later as RouteUpdate signaling.
    rtc_ack_delivered: bool,
    // Rev A subtype-3 RTCMAC Grant retry state. Grants only start after RTCAck
    // is physically ACKed; delivery is tracked by the Stream 0 reliable queue.
    rtc_grant_last_send_slot: Option<u64>,
    rtc_grant_sends: u32,
    // Absolute reverse traffic slot at which the last RTCAck was requested.
    rtc_ack_last_send_slot: Option<u64>,
    // Access-channel slot that opened the current RTCMAC setup attempt. Used
    // only as a stale-event guard; the real AT timer starts on TCA reception.
    setup_start_slot: Option<u64>,
    setup_started_at: Instant,
    session_config_start_sent: bool,
    session_config_complete_sent: bool,
    an_session_config_complete_acked: bool,
    session_config_commit_connection_close_pending: bool,
    session_config_commit_connection_close_sent: bool,
    session_config_commit_connection_close_sent_at: Option<Instant>,
    at_session_config_complete_transaction_id: Option<u8>,
    stream0_slp_reset_sequence: u8,
    stream0_slp_reset_pending: bool,
    stream0_slp_reset_acked: bool,
    stream0_slp_reset_attempts: u32,
    stream0_slp_reset_last_send_slot: Option<u64>,
    reliable_stream0_tx: Vec<PendingReliableStream0Packet>,
    // Reverse-link Stream 0 SLP-D receive state. C.S0024 SLP-D duplicate
    // detection is below SNP/application handling: repeated reliable packets
    // must still be acknowledged, but their payload must not be delivered
    // again to Session Configuration, Hardware ID, TCC, or packet layers.
    reverse_stream0_slp_d_vn: u8,
    reverse_stream0_slp_d_rx: u8,
    session_config_trace: Option<SessionConfigTrace>,
    in_use_physical_layer_subtype: u16,
    session_personality_count: u16,
    protocol_config_traces: Vec<ProtocolConfigTrace>,
    dh_key_exchange: Option<DhKeyExchangeState>,
    dh_key_exchange_complete: bool,
    in_use_forward_traffic_mac_subtype: u16,
    in_use_reverse_traffic_mac_subtype: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DhKeyExchangeState {
    transaction_id: u8,
    an_private: BigUint,
    an_public: Vec<u8>,
    session_key: Option<Vec<u8>>,
    nonce: Option<u16>,
    timestamp_long: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CachedUatiAssignment {
    request_ati: AccessTerminalIdentifier,
    transaction_id: u8,
    uati: u32,
    sequence: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct DeferredConnectionRequest {
    ati: AccessTerminalIdentifier,
    request: HrpdConnectionRequest,
    access_slot: Option<u64>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct CompletedConnectionRequest {
    session_uati: u32,
    traffic_uati: u32,
    transaction_id: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct PendingReliableStream0Packet {
    sequence_number: u8,
    protocol_type: u8,
    payload: Vec<u8>,
    in_configuration: bool,
    ack_sequence_number: Option<u8>,
    label: &'static str,
    attempts: u32,
    last_send_at: Instant,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct SessionConfigTrace {
    transaction_id: u8,
    request_attrs: Vec<u8>,
    response_attrs: Vec<u8>,
    sequence_number: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ProtocolConfigTrace {
    protocol_type: u8,
    transaction_id: u8,
    request_attrs: Vec<u8>,
    response_attrs: Vec<u8>,
    sequence_number: Option<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
struct Stream0SlpFReassembly {
    sync: bool,
    last_sequence: Option<u8>,
    buffer: Vec<u8>,
}

#[derive(Debug, Default)]
struct DefaultPacketRlpReceiveOutcome {
    delivered: Vec<u8>,
    nak_requests: Vec<cdma_common::hrpd::air::HrpdDefaultPacketRlpNakRequest>,
    duplicate_octets: usize,
    reordered_octets: usize,
    aborted_octets: usize,
}

/// Default Packet RLP receive state, C.S0024-A v3.0 section 3.4.4.1.2.2.
///
/// Sequence numbers identify octets, not packets. Keeping the buffer here at
/// octet granularity lets a retransmitted RLP packet overlap an already
/// received packet without duplicating or reordering the PPP byte stream.
#[derive(Debug, Default)]
struct DefaultPacketRlpReceiver {
    v_r: u32,
    v_n: u32,
    resequencing: HashMap<u32, u8>,
    nak_abort_at: HashMap<u32, Instant>,
}

impl DefaultPacketRlpReceiver {
    fn reset(&mut self) {
        self.v_r = 0;
        self.v_n = 0;
        self.resequencing.clear();
        self.nak_abort_at.clear();
    }

    fn ingest(
        &mut self,
        sequence: u32,
        payload: &[u8],
        now: Instant,
    ) -> DefaultPacketRlpReceiveOutcome {
        let mut outcome = self.expire(now);
        for (offset, &octet) in payload.iter().enumerate() {
            let x = sequence.wrapping_add(offset as u32) & rlp::SEQUENCE_MASK;
            match rlp::cmp(x, self.v_n) {
                std::cmp::Ordering::Less => {
                    outcome.duplicate_octets += 1;
                }
                _ => match rlp::cmp(x, self.v_r) {
                    std::cmp::Ordering::Less => {
                        if self.resequencing.insert(x, octet).is_some() {
                            outcome.duplicate_octets += 1;
                        } else {
                            self.nak_abort_at.remove(&x);
                            outcome.reordered_octets += 1;
                            if x == self.v_n {
                                self.deliver_contiguous(&mut outcome.delivered);
                            }
                        }
                    }
                    std::cmp::Ordering::Equal => {
                        self.v_r = rlp::next(self.v_r);
                        if x == self.v_n {
                            self.v_n = rlp::next(self.v_n);
                            outcome.delivered.push(octet);
                        } else {
                            self.resequencing.insert(x, octet);
                            outcome.reordered_octets += 1;
                        }
                    }
                    std::cmp::Ordering::Greater => {
                        let missing = rlp::distance(self.v_r, x);
                        self.record_missing_range(
                            self.v_r,
                            missing,
                            now + DEFAULT_PACKET_RLP_ABORT,
                            &mut outcome.nak_requests,
                        );
                        self.resequencing.insert(x, octet);
                        outcome.reordered_octets += 1;
                        self.v_r = rlp::next(x);
                    }
                },
            }
        }
        outcome
    }

    fn expire(&mut self, now: Instant) -> DefaultPacketRlpReceiveOutcome {
        let mut outcome = DefaultPacketRlpReceiveOutcome::default();
        while self.v_n != self.v_r {
            if self.resequencing.contains_key(&self.v_n) {
                self.deliver_contiguous(&mut outcome.delivered);
                continue;
            }
            let Some(deadline) = self.nak_abort_at.get(&self.v_n).copied() else {
                break;
            };
            if deadline > now {
                break;
            }
            self.nak_abort_at.remove(&self.v_n);
            self.v_n = rlp::next(self.v_n);
            outcome.aborted_octets += 1;
        }
        outcome
    }

    fn deliver_contiguous(&mut self, delivered: &mut Vec<u8>) {
        while self.v_n != self.v_r {
            let Some(octet) = self.resequencing.remove(&self.v_n) else {
                break;
            };
            self.nak_abort_at.remove(&self.v_n);
            delivered.push(octet);
            self.v_n = rlp::next(self.v_n);
        }
    }

    fn record_missing_range(
        &mut self,
        first: u32,
        len: u32,
        abort_at: Instant,
        requests: &mut Vec<cdma_common::hrpd::air::HrpdDefaultPacketRlpNakRequest>,
    ) {
        let mut sequence = first;
        let mut remaining = len;
        while remaining != 0 {
            let window_len = remaining.min(u32::from(u16::MAX)) as u16;
            requests.push(cdma_common::hrpd::air::HrpdDefaultPacketRlpNakRequest {
                first_erased: sequence,
                window_len,
            });
            for _ in 0..window_len {
                self.nak_abort_at.entry(sequence).or_insert(abort_at);
                sequence = rlp::next(sequence);
            }
            remaining -= u32::from(window_len);
        }
    }
}

/// Per-AT state for one Access Terminal's HRPD session and connection.
///
/// One `AtSession` exists per AT, keyed in the controller by its session UATI.
/// The controller resolves the addressed AT at each entry point, owns its
/// `AtSession` for the duration of processing, and re-inserts it afterward, so
/// a handler can hold `&mut AtSession` while still calling `&mut self`
/// controller methods and touching the controller's UATI-keyed maps.
#[derive(Debug)]
struct AtSession {
    session: SessionStateMachine,
    uati_assignment_sequence: u8,
    expected_uati_complete_sequence: Option<u8>,
    last_uati_assignment: Option<CachedUatiAssignment>,
    deferred_connection_after_uati_complete: Option<DeferredConnectionRequest>,
    pending_hardware_id: Option<(u32, u8)>,
    pending_traffic_assignment: Option<PendingTrafficAssignment>,
    last_completed_connection_request: Option<CompletedConnectionRequest>,
    // Forward-link SLP-D V(S) for Stream 0 (Default Signaling), masked to 3
    // bits per C.S0024-0 §9.6.3. Incremented after each *new* reliable
    // signaling message is composed; retransmissions of the same message
    // reuse the captured V(S).
    slp_d_vs_stream0_fl: u8,
    session_configuration_complete: bool,
    committed_session_configuration_response: Option<Vec<u8>>,
    committed_protocol_configuration_responses: HashMap<u8, Vec<u8>>,
    reverse_default_packet_rlp: DefaultPacketRlpReceiver,
    last_activity_at: Instant,
}

impl AtSession {
    fn new(color_code: u8) -> Self {
        let now = Instant::now();
        Self {
            session: SessionStateMachine::new(color_code),
            uati_assignment_sequence: 0,
            expected_uati_complete_sequence: None,
            last_uati_assignment: None,
            deferred_connection_after_uati_complete: None,
            pending_hardware_id: None,
            pending_traffic_assignment: None,
            last_completed_connection_request: None,
            slp_d_vs_stream0_fl: 0,
            session_configuration_complete: false,
            committed_session_configuration_response: None,
            committed_protocol_configuration_responses: HashMap::new(),
            reverse_default_packet_rlp: DefaultPacketRlpReceiver::default(),
            last_activity_at: now,
        }
    }
}

/// HRPD Rev 0 AN air-event controller.
///
/// Tracks one `AtSession` per Access Terminal in `ats`, keyed by session UATI,
/// so multiple ATs can hold concurrent sessions and traffic assignments. The
/// remaining fields are either sector-wide or UATI-keyed maps shared across all
/// ATs.
#[derive(Debug)]
pub struct HrpdAirController {
    ats: HashMap<u32, AtSession>,
    color_code: u8,
    sector_pilot_pn: u16,
    channel: Option<HrpdChannelRecord>,
    uati_subnet_assignment: Option<HrpdUatiSubnetAssignment>,
    hardware_id_transaction: u8,
    hardware_ids_by_uati: HashMap<u32, HrpdHardwareIdResponse>,
    next_mac_index: u8,
    // Most recent reverse DRC value reported by each AT, keyed by traffic
    // UATI. Values are accepted only when the negotiated forward-traffic rate
    // table maps them to a physical packet size.
    last_drc_by_uati: HashMap<u32, u8>,
    default_packet_stream_id: u8,
    // Reverse-link SLP-F reassembly state for Stream 0, keyed by traffic UATI.
    // C.S0024-0 §2.6.4.3 requires an independent reassembly buffer per
    // connection endpoint before passing complete payloads up to SLP-D.
    stream0_slp_f_rx: HashMap<u32, Stream0SlpFReassembly>,
    // A Closed session returned by the test-only `session()` accessor when no
    // AT is tracked, so single-AT assertions read the pre-UATI Closed state.
    #[cfg(test)]
    test_closed_session: SessionStateMachine,
}

impl HrpdAirController {
    pub fn new(color_code: u8) -> Self {
        Self::with_sector(color_code, 0, None)
    }

    pub fn with_sector(
        color_code: u8,
        sector_pilot_pn: u16,
        channel: Option<HrpdChannelRecord>,
    ) -> Self {
        Self::with_sector_and_uati_subnet(color_code, sector_pilot_pn, channel, None)
    }

    pub fn with_sector_and_uati_subnet(
        color_code: u8,
        sector_pilot_pn: u16,
        channel: Option<HrpdChannelRecord>,
        uati_subnet_assignment: Option<HrpdUatiSubnetAssignment>,
    ) -> Self {
        Self {
            ats: HashMap::new(),
            color_code,
            sector_pilot_pn: sector_pilot_pn & 0x01ff,
            channel,
            uati_subnet_assignment,
            hardware_id_transaction: 0,
            hardware_ids_by_uati: HashMap::new(),
            next_mac_index: HRPD_FIRST_TRAFFIC_MAC_INDEX,
            last_drc_by_uati: HashMap::new(),
            default_packet_stream_id: DEFAULT_PACKET_STREAM_ID,
            stream0_slp_f_rx: HashMap::new(),
            #[cfg(test)]
            test_closed_session: SessionStateMachine::new(color_code),
        }
    }

    fn default_packet_protocol_type(&self) -> u8 {
        default_packet_stream_protocol_type(self.default_packet_stream_id)
            .unwrap_or(DEFAULT_PACKET_STREAM1_APPLICATION_PROTOCOL_TYPE)
    }

    pub fn session_for_uati(&self, uati: u32) -> Option<&SessionStateMachine> {
        self.ats.get(&uati).map(|at| &at.session)
    }

    /// The single tracked AT's session, or a Closed default when none is
    /// tracked. Single-AT test convenience only; production callers route by
    /// UATI through `session_for_uati`.
    #[cfg(test)]
    fn session(&self) -> &SessionStateMachine {
        self.ats
            .values()
            .next()
            .map(|at| &at.session)
            .unwrap_or(&self.test_closed_session)
    }

    /// The single tracked AT's pending traffic assignment. Single-AT test
    /// convenience only.
    #[cfg(test)]
    fn pending_traffic_assignment(&self) -> Option<&PendingTrafficAssignment> {
        self.ats
            .values()
            .next()
            .and_then(|at| at.pending_traffic_assignment.as_ref())
    }

    pub fn hardware_id_for_uati(&self, uati: u32) -> Option<&HrpdHardwareIdResponse> {
        self.hardware_ids_by_uati.get(&uati)
    }

    pub fn handle_access_indication(
        &mut self,
        indication: &HrpdAccessIndication,
        allocator: &mut UatiAllocator,
    ) -> Result<HrpdAccessOutcome, StateMachineError> {
        let mut outcome = HrpdAccessOutcome::empty();
        let access_contains_uati_complete = indication
            .messages
            .iter()
            .any(|message| matches!(message, HrpdAccessMessage::UatiComplete(_)));
        if !indication.messages.is_empty() {
            log::info!(
                "HRPD AN: queueing ACAck for access ati={:?} messages={}",
                indication.ati,
                indication.messages.len()
            );
            outcome
                .forward_signaling
                .push(HrpdForwardSignalingRequest::access_channel_ack(
                    indication.ati,
                ));
        }
        // All messages in one indication share the same ATI, so resolve the
        // addressed AT once. Own the AtSession for the whole message loop (a
        // brand-new AT starts from a fresh session whose UATI is only assigned
        // mid-loop) and re-key it on the now-known UATI afterward.
        let resolved_uati = self.resolve_access_uati(indication.ati, allocator);
        let mut at = resolved_uati
            .and_then(|uati| self.ats.remove(&uati))
            .unwrap_or_else(|| AtSession::new(self.color_code));
        for message in &indication.messages {
            let at = &mut at;
            match message {
                HrpdAccessMessage::RouteUpdate(route) => {
                    self.maybe_session_close_orphaned_cached_uati_access(
                        at,
                        indication.ati,
                        allocator,
                        &mut outcome,
                        "RouteUpdate",
                    );
                    outcome.route_updates.push(route.clone());
                }
                HrpdAccessMessage::UatiRequest(request) => {
                    if self.retransmit_cached_uati_assignment(
                        at,
                        indication.ati,
                        request,
                        &mut outcome,
                    ) {
                        continue;
                    }
                    if self.uati_request_matches_pending_traffic_setup(at, indication.ati) {
                        log::info!(
                            "HRPD AN: UATIRequest transaction=0x{:02x} from {:?} arrived while traffic setup is pending; releasing stale traffic assignment",
                            request.transaction_id,
                            indication.ati
                        );
                        self.release_pending_traffic_assignment(
                            at,
                            &mut outcome,
                            "UATIRequest during traffic setup",
                        );
                    }
                    at.pending_hardware_id = None;
                    at.deferred_connection_after_uati_complete = None;
                    let outbound = at
                        .session
                        .on_message(InboundSessionMessage::UatiRequest, allocator)?;
                    if let Some(session) = at.session.session() {
                        log::info!(
                            "HRPD AN: UATIRequest from {:?}; assigned UATI=0x{:08x}",
                            indication.ati,
                            session.uati.as_u32()
                        );
                    }
                    self.on_session_outbound(
                        at,
                        indication.ati,
                        Some(request.transaction_id),
                        &outbound,
                        &mut outcome,
                    );
                    outcome.session_outbound.extend(outbound);
                }
                HrpdAccessMessage::UatiComplete(uati) => {
                    if at.expected_uati_complete_sequence == Some(uati.message_sequence) {
                        at.expected_uati_complete_sequence = None;
                        at.last_uati_assignment = None;
                        outcome.uati_completes.push(uati.clone());
                        let outbound = at
                            .session
                            .on_message(InboundSessionMessage::UatiComplete, allocator)?;
                        outcome.session_outbound.extend(outbound);
                        self.queue_hardware_id_request(at, &mut outcome);
                        self.handle_deferred_connection_after_uati_complete(at, &mut outcome);
                    } else if at.session.state() == SessionState::Open
                        && self.current_session_matches_ati(at, indication.ati)
                    {
                        log::info!(
                            "HRPD AN: accepting duplicate UATIComplete seq={} for already-open UATI {:?}",
                            uati.message_sequence,
                            indication.ati
                        );
                        at.expected_uati_complete_sequence = None;
                        at.last_uati_assignment = None;
                        outcome.uati_completes.push(uati.clone());
                    } else {
                        log::info!(
                            "HRPD AN: ignoring unmatched UATIComplete seq={} expected={}",
                            uati.message_sequence,
                            at.expected_uati_complete_sequence
                                .map(|seq| seq.to_string())
                                .unwrap_or_else(|| "none".to_string())
                        );
                    }
                }
                HrpdAccessMessage::ConnectionRequest(connection) => {
                    let access_slot =
                        Some(indication.absolute_chip / HRPD_REVERSE_TRAFFIC_SLOT_CHIPS);
                    if self.connection_request_waiting_for_assigned_uati(at, indication.ati)
                        && !access_contains_uati_complete
                    {
                        log::info!(
                            "HRPD AN: deferring ConnectionRequest transaction=0x{:02x} from {:?} while UATIComplete is pending for assigned UATI",
                            connection.transaction_id,
                            indication.ati
                        );
                        at.deferred_connection_after_uati_complete =
                            Some(DeferredConnectionRequest {
                                ati: indication.ati,
                                request: connection.clone(),
                                access_slot,
                            });
                        self.retransmit_pending_uati_assignment_to_current_ati(
                            at,
                            indication.ati,
                            &mut outcome,
                        );
                        continue;
                    }
                    self.maybe_restore_cached_uati_from_connection_request(
                        at,
                        indication.ati,
                        allocator,
                        &mut outcome,
                    )?;
                    self.maybe_accept_pending_uati_from_connection_request(
                        at,
                        indication.ati,
                        allocator,
                        &mut outcome,
                    )?;
                    self.handle_connection_request(at, connection, access_slot, &mut outcome);
                }
                HrpdAccessMessage::TrafficChannelComplete(complete) => {
                    outcome.traffic_channel_completes.push(complete.clone());
                    let _ = self.handle_traffic_channel_complete(at, complete, None);
                }
                HrpdAccessMessage::SessionClose(close) => {
                    outcome.session_closes.push(close.clone());
                    self.cancel_access_traffic_outputs_for_session_close(&mut outcome);
                    self.close_session_from_access(
                        at,
                        indication.ati,
                        allocator,
                        &mut outcome,
                        "access SessionClose",
                    )?;
                }
                HrpdAccessMessage::ConnectionClose(close) => {
                    log::info!(
                        "HRPD AN: decoded access ConnectionClose reason=0x{:x} ({}) suspend_enable={} suspend_time={:?} reserved_zero={}",
                        close.close_reason,
                        hrpd_connection_close_reason_name(close.close_reason),
                        close.suspend_enable,
                        close.suspend_time,
                        close.reserved_zero
                    );
                    self.release_pending_traffic_assignment(at, &mut outcome, "ConnectionClose");
                }
                HrpdAccessMessage::HardwareIdResponse(hardware) => {
                    self.handle_hardware_id_response(at, hardware, &mut outcome);
                }
                HrpdAccessMessage::KeepAlive => {
                    outcome.keepalive_seen = true;
                }
                HrpdAccessMessage::DefaultPacketXonRequest => {
                    log::info!("HRPD AN: decoded access DefaultPacket XonRequest");
                    let uati = (indication.ati.ati_type == AccessTerminalIdentifierType::Uati)
                        .then_some(indication.ati.value);
                    outcome.forward_signaling.push(HrpdForwardSignalingRequest {
                        uati,
                        target_ati: indication.ati,
                        protocol_type: self.default_packet_protocol_type(),
                        payload: vec![DEFAULT_PACKET_XON_RESPONSE],
                        channel: cdma_common::hrpd::air::HrpdForwardChannel::AsynchronousControl,
                        reliable_sequence: None,
                        synchronous_control_cycle: None,
                    });
                }
                HrpdAccessMessage::DefaultPacketXoffRequest => {
                    log::info!("HRPD AN: decoded access DefaultPacket XoffRequest");
                    let uati = (indication.ati.ati_type == AccessTerminalIdentifierType::Uati)
                        .then_some(indication.ati.value);
                    outcome.forward_signaling.push(HrpdForwardSignalingRequest {
                        uati,
                        target_ati: indication.ati,
                        protocol_type: self.default_packet_protocol_type(),
                        payload: vec![DEFAULT_PACKET_XOFF_RESPONSE],
                        channel: cdma_common::hrpd::air::HrpdForwardChannel::AsynchronousControl,
                        reliable_sequence: None,
                        synchronous_control_cycle: None,
                    });
                }
                HrpdAccessMessage::DefaultPacketDataReadyAck(ack) => {
                    log::info!(
                        "HRPD AN: decoded access DefaultPacket DataReadyAck transaction=0x{:02x}",
                        ack.transaction_id
                    );
                }
                HrpdAccessMessage::DefaultPacketRlpReset(_) => {
                    log::info!("HRPD AN: decoded access DefaultPacket RLP Reset");
                    let uati = (indication.ati.ati_type == AccessTerminalIdentifierType::Uati)
                        .then_some(indication.ati.value);
                    outcome.forward_signaling.push(HrpdForwardSignalingRequest {
                        uati,
                        target_ati: indication.ati,
                        protocol_type: self.default_packet_protocol_type(),
                        payload: vec![DEFAULT_PACKET_RLP_RESET_ACK],
                        channel: cdma_common::hrpd::air::HrpdForwardChannel::AsynchronousControl,
                        reliable_sequence: None,
                        synchronous_control_cycle: None,
                    });
                }
                HrpdAccessMessage::DefaultPacketRlpResetAck(_) => {
                    log::info!("HRPD AN: decoded access DefaultPacket RLP ResetAck");
                }
                HrpdAccessMessage::DefaultPacketRlpNak(nak) => {
                    log::info!(
                        "HRPD AN: decoded access DefaultPacket RLP Nak requests={}",
                        nak.requests.len()
                    );
                }
                HrpdAccessMessage::DefaultSignalingReset(reset) => {
                    log::info!(
                        "HRPD AN: decoded access Stream0 DefaultSignaling Reset seq={}",
                        reset.message_sequence
                    );
                }
                HrpdAccessMessage::DefaultSignalingResetAck(ack) => {
                    log::info!(
                        "HRPD AN: decoded access Stream0 DefaultSignaling ResetAck seq={}",
                        ack.message_sequence
                    );
                }
                HrpdAccessMessage::Unknown { .. } => {
                    outcome.unknown_messages += 1;
                }
            }
        }
        // Re-key on the UATI assigned to (or restored for) this AT during
        // processing. Closed sessions are intentionally not reinserted: once
        // SessionClose runs, later stale-UATI access is handled through the
        // orphaned-cached-UATI Session Lost path.
        let reinsert_uati = at.session.session().map(|session| session.uati.as_u32());
        if let Some(uati) = reinsert_uati {
            at.last_activity_at = Instant::now();
            outcome.affected_uatis.push(uati);
            self.ats.insert(uati, at);
        }
        Ok(outcome)
    }

    /// Resolves the session UATI of an in-progress AT addressed by an access
    /// ATI, if any. Returns `None` for a brand-new AT (the caller starts a
    /// fresh `AtSession`).
    fn resolve_access_uati(
        &self,
        ati: AccessTerminalIdentifier,
        allocator: &UatiAllocator,
    ) -> Option<u32> {
        if ati.ati_type == AccessTerminalIdentifierType::Uati {
            if let Some(uati) = Self::cached_uati_from_access_ati(ati, allocator, "access")
                && self.ats.contains_key(&uati.as_u32())
            {
                return Some(uati.as_u32());
            }
        }
        // Otherwise the message still addresses the AT by the RATI in the
        // pending UATIAssignment's request, which only happens for a
        // pre-UATIComplete retransmit (before the AT switches TransmitATI to
        // the UATI per C.S0024-0 §5.3.7.1.5.1).
        self.ats.iter().find_map(|(uati, at)| {
            let matches = at
                .last_uati_assignment
                .is_some_and(|cached| cached.request_ati == ati);
            matches.then_some(*uati)
        })
    }

    fn current_session_matches_ati(&self, at: &AtSession, ati: AccessTerminalIdentifier) -> bool {
        if ati.ati_type != AccessTerminalIdentifierType::Uati {
            return false;
        }
        at.session
            .session()
            .is_some_and(|session| self.uati_receive_value(session.uati.as_u32()) == ati.value)
    }

    pub fn handle_traffic_event(&mut self, event: &HrpdTrafficEvent) -> HrpdTrafficOutcome {
        self.handle_traffic_event_inner(event, None)
    }

    pub fn handle_traffic_event_with_allocator(
        &mut self,
        event: &HrpdTrafficEvent,
        allocator: &mut UatiAllocator,
    ) -> HrpdTrafficOutcome {
        self.handle_traffic_event_inner(event, Some(allocator))
    }

    fn handle_traffic_event_inner(
        &mut self,
        event: &HrpdTrafficEvent,
        mut allocator: Option<&mut UatiAllocator>,
    ) -> HrpdTrafficOutcome {
        let mut outcome = HrpdTrafficOutcome::default();
        // Every traffic event carries the AT's traffic UATI, which maps to the
        // session UATI keying `ats`. Own that AT's session for the duration so
        // the handlers can hold `&mut at` while still touching the controller's
        // shared maps. A fresh throwaway covers events for an unknown UATI
        // (dropped exactly as before, never reinserted).
        let event_uati = traffic_event_uati(event);
        let session_uati = self.session_uati_for_traffic_uati(event_uati);
        let was_present = session_uati.is_some();
        let mut at_owned = session_uati
            .and_then(|uati| self.ats.remove(&uati))
            .unwrap_or_else(|| AtSession::new(self.color_code));
        let at = &mut at_owned;
        if was_present
            && let Some(traffic_uati) = at
                .pending_traffic_assignment
                .as_ref()
                .filter(|pending| pending.active)
                .map(|pending| pending.traffic_uati)
        {
            let expired = at.reverse_default_packet_rlp.expire(Instant::now());
            if expired.aborted_octets != 0 {
                log::warn!(
                    "HRPD AN: reverse DefaultPacket RLP abort UATI=0x{traffic_uati:08x} skipped_octets={} released_octets={} buffered_octets={} v_n={} v_r={}",
                    expired.aborted_octets,
                    expired.delivered.len(),
                    at.reverse_default_packet_rlp.resequencing.len(),
                    at.reverse_default_packet_rlp.v_n,
                    at.reverse_default_packet_rlp.v_r,
                );
            }
            if !expired.delivered.is_empty() {
                outcome.default_packet_flow_open_uatis.push(traffic_uati);
                outcome.a8_uplink.push(HrpdA8UplinkPacket {
                    uati: traffic_uati,
                    payload: expired.delivered,
                });
            }
        }
        match event {
            HrpdTrafficEvent::ReversePilot { uati, .. } => {
                outcome.reverse_pilot_count = 1;
                if self.accept_or_drop_event(at, *uati, &mut outcome) {
                    log::trace!("HRPD AN: accepted ReversePilot traffic event UATI=0x{uati:08x}");
                    self.queue_rtc_ack_for_reverse_pilot(at, event, *uati, &mut outcome);
                } else {
                    log::debug!("HRPD AN: dropped ReversePilot traffic event UATI=0x{uati:08x}");
                }
            }
            HrpdTrafficEvent::ReversePilotLost {
                uati,
                mac_index,
                last_good_chip,
                lost_at_chip,
                lost_chips,
                last_snr_db_tenths,
                last_coherence_x1000,
            } => {
                if self.accept_or_drop_event(at, *uati, &mut outcome) {
                    log::warn!(
                        "HRPD AN: reverse traffic pilot lost UATI=0x{uati:08x} MAC={} last_good_chip={} lost_at_chip={} lost_chips={} last_snr={:.1}dB last_coh={:.3}; releasing traffic channel",
                        mac_index,
                        last_good_chip,
                        lost_at_chip,
                        lost_chips,
                        f32::from(*last_snr_db_tenths) / 10.0,
                        f32::from(*last_coherence_x1000) / 1000.0,
                    );
                    let release_count_before = outcome.traffic_releases.len();
                    self.release_pending_traffic_assignment_for_traffic(
                        at,
                        &mut outcome,
                        "ReversePilotLost",
                    );
                    if outcome.traffic_releases.len() == release_count_before {
                        self.last_drc_by_uati.remove(uati);
                        self.stream0_slp_f_rx.remove(uati);
                        outcome.traffic_releases.push(HrpdTrafficReleaseRequest {
                            uati: *uati,
                            mac_index: *mac_index,
                        });
                    }
                    outcome.traffic_channel_closed_uatis.push(*uati);
                } else {
                    log::info!(
                        "HRPD AN: dropped ReversePilotLost traffic event UATI=0x{uati:08x} MAC={mac_index}"
                    );
                }
            }
            HrpdTrafficEvent::Drc {
                uati,
                slot,
                drc_index,
                ..
            } => {
                outcome.drc_count = 1;
                if self.accept_or_drop_event(at, *uati, &mut outcome) {
                    // Track only DRCs the live transmitter can encode today.
                    // Enhanced FTC subtype 1 includes spec-valid 0xd/0xe
                    // rates, but the forward interleaver/modulator path does
                    // not yet implement their 5120-bit physical packets.
                    let idx = *drc_index;
                    if implemented_forward_traffic_payload_bits_for_drc(idx).is_some() {
                        self.last_drc_by_uati.insert(*uati, idx);
                    } else {
                        self.last_drc_by_uati.remove(uati);
                    }
                    Self::maybe_retransmit_rtc_ack_on_drc(
                        at,
                        *uati,
                        *slot,
                        *drc_index,
                        &mut outcome,
                    );
                    Self::maybe_send_post_rtc_ack_grant_on_drc(
                        at,
                        *uati,
                        *slot,
                        *drc_index,
                        &mut outcome,
                    );
                    self.maybe_retransmit_stream0_slp_reset_on_drc(at, *uati, *slot, &mut outcome);
                    self.maybe_retransmit_reliable_stream0(at, Instant::now(), &mut outcome);
                }
            }
            HrpdTrafficEvent::Ack { uati, ack, .. } => {
                outcome.ack_count = 1;
                if self.accept_or_drop_event(at, *uati, &mut outcome) && *ack {
                    Self::mark_rtc_ack_delivered(at, *uati);
                }
            }
            HrpdTrafficEvent::Stream0Signaling { uati, payload } => {
                outcome.stream0_signaling_count = 1;
                if self.accept_or_drop_event(at, *uati, &mut outcome) {
                    log::debug!(
                        "HRPD AN: accepted Stream0 traffic signaling UATI=0x{uati:08x} payload_bits={}",
                        payload.len()
                    );
                    match self.parse_stream0_default_signaling_for_uati(*uati, payload) {
                        Stream0ParseOutcome::Complete(parsed) => {
                            if let Some(ack_sequence_number) = parsed.ack_sequence_number {
                                log::info!(
                                    "HRPD AN: received reverse Stream0 SLP-D ACK UATI=0x{uati:08x} ack_seq={ack_sequence_number}"
                                );
                                let acked_labels = Self::mark_reliable_stream0_acknowledged(
                                    at,
                                    *uati,
                                    ack_sequence_number,
                                );
                                for label in acked_labels {
                                    if is_session_configuration_complete_label(label) {
                                        self.mark_session_configuration_complete_acked(
                                            at,
                                            *uati,
                                            label,
                                            parsed.sequence_number,
                                            &mut outcome,
                                        );
                                    }
                                }
                            }
                            let mut ack_queued = false;
                            let duplicate_reliable = parsed
                                .sequence_number
                                .map(|sequence_number| {
                                    !Self::accept_reverse_stream0_slp_d_payload(
                                        at,
                                        *uati,
                                        sequence_number,
                                    )
                                })
                                .unwrap_or(false);
                            if duplicate_reliable {
                                if let Some(sequence_number) = parsed.sequence_number {
                                    log::info!(
                                        "HRPD AN: discarding duplicate reliable reverse Stream-0 SLP-D UATI=0x{uati:08x} seq={sequence_number}"
                                    );
                                }
                            } else if let Some(message) = &parsed.message {
                                outcome.decoded_stream0_messages.push(message.clone());
                                match message {
                                    HrpdAccessMessage::TrafficChannelComplete(complete) => {
                                        let (packets, accepted) = self
                                            .handle_traffic_channel_complete(
                                                at,
                                                complete,
                                                parsed.sequence_number,
                                            );
                                        if accepted {
                                            outcome.traffic_channel_open_uatis.push(*uati);
                                            self.maybe_emit_current_session_configuration_for_active_traffic(
                                                at,
                                                *uati,
                                                &mut outcome,
                                            );
                                        }
                                        if !packets.is_empty() {
                                            outcome.forward_traffic.extend(packets);
                                            ack_queued = true;
                                        }
                                    }
                                    HrpdAccessMessage::RouteUpdate(route) => {
                                        log::info!(
                                            "HRPD AN: decoded reverse Stream-0 RouteUpdate UATI=0x{uati:08x} seq={} ref_pn={} strength={} keep={} pilots={}",
                                            route.message_sequence,
                                            route.reference_pilot_pn,
                                            route.reference_pilot_strength,
                                            route.reference_keep,
                                            route.num_pilots
                                        );
                                        if let Some(sequence_number) = parsed.sequence_number
                                            && let Some(packet) = self
                                                .build_slp_d_ack_packet_for_uati(
                                                    at,
                                                    *uati,
                                                    sequence_number,
                                                )
                                        {
                                            log::info!(
                                                "HRPD AN: ACKing reliable reverse Stream-0 signaling UATI=0x{uati:08x} ack_seq={sequence_number}"
                                            );
                                            outcome.forward_traffic.push(packet);
                                            ack_queued = true;
                                        }
                                        outcome.forward_traffic.extend(
                                            self.initialize_stream0_slp_after_route_update(
                                                at, *uati,
                                            ),
                                        );
                                        self.maybe_emit_current_session_configuration_for_active_traffic(
                                            at,
                                            *uati,
                                            &mut outcome,
                                        );
                                    }
                                    HrpdAccessMessage::SessionClose(close) => {
                                        if let Some(reference) =
                                            hrpd_protocol_reference_from_more_info(&close.more_info)
                                        {
                                            log::info!(
                                                "HRPD AN: decoded reverse Stream-0 SessionClose UATI=0x{uati:08x} reason=0x{:02x} ({}) more_info_len={} failed_protocol_type=0x{:x} failed_protocol_subtype=0x{:04x}",
                                                close.close_reason,
                                                hrpd_session_close_reason_name(close.close_reason),
                                                close.more_info.len(),
                                                reference.protocol_type,
                                                reference.protocol_subtype
                                            );
                                        } else {
                                            log::info!(
                                                "HRPD AN: decoded reverse Stream-0 SessionClose UATI=0x{uati:08x} reason=0x{:02x} ({}) more_info_len={}",
                                                close.close_reason,
                                                hrpd_session_close_reason_name(close.close_reason),
                                                close.more_info.len()
                                            );
                                        }
                                        // `as_deref_mut` reborrows the `Option<&mut _>` so the
                                        // allocator survives the enclosing per-message loop;
                                        // clippy's suggested move would consume it on the first
                                        // iteration.
                                        #[allow(clippy::needless_option_as_deref)]
                                        if let Some(allocator) = allocator.as_deref_mut() {
                                            self.close_session_from_traffic(
                                                at,
                                                *uati,
                                                allocator,
                                                &mut outcome,
                                                "traffic SessionClose",
                                            );
                                        } else {
                                            Self::log_pending_config_trace(
                                                at,
                                                *uati,
                                                "SessionClose",
                                            );
                                            at.session_configuration_complete = false;
                                            Self::clear_committed_session_configuration(at);
                                            self.release_pending_traffic_assignment_for_traffic(
                                                at,
                                                &mut outcome,
                                                "SessionClose",
                                            );
                                            outcome.session_closed_uatis.push(*uati);
                                            outcome.traffic_channel_closed_uatis.push(*uati);
                                        }
                                    }
                                    HrpdAccessMessage::ConnectionClose(close) => {
                                        log::info!(
                                            "HRPD AN: decoded reverse Stream-0 ConnectionClose UATI=0x{uati:08x} reason=0x{:x} ({}) suspend_enable={} suspend_time={:?} reserved_zero={}",
                                            close.close_reason,
                                            hrpd_connection_close_reason_name(close.close_reason),
                                            close.suspend_enable,
                                            close.suspend_time,
                                            close.reserved_zero
                                        );
                                        Self::maybe_commit_session_configuration_after_connection_close(
                                            at,
                                            *uati,
                                            &mut outcome,
                                        );
                                        self.release_pending_traffic_assignment_for_traffic(
                                            at,
                                            &mut outcome,
                                            "ConnectionClose",
                                        );
                                        outcome.traffic_channel_closed_uatis.push(*uati);
                                    }
                                    HrpdAccessMessage::HardwareIdResponse(hardware) => {
                                        log::info!(
                                            "HRPD AN: decoded reverse Stream-0 HardwareIdResponse UATI=0x{uati:08x} transaction=0x{:02x} type=0x{:06x} value_len={}",
                                            hardware.transaction_id,
                                            hardware.hardware_id_type,
                                            hardware.hardware_id_value.len()
                                        );
                                        outcome.hardware_id_responses.push(
                                            HrpdTrafficHardwareIdResponse {
                                                uati: *uati,
                                                response: hardware.clone(),
                                            },
                                        );
                                        if let Some((session_uati, transaction_id)) =
                                            at.pending_hardware_id
                                            && transaction_id == hardware.transaction_id
                                        {
                                            self.hardware_ids_by_uati
                                                .insert(session_uati, hardware.clone());
                                            self.hardware_ids_by_uati
                                                .insert(*uati, hardware.clone());
                                            at.pending_hardware_id = None;
                                        }
                                    }
                                    HrpdAccessMessage::KeepAlive => {
                                        log::info!(
                                            "HRPD AN: decoded reverse Stream-0 KeepAlive UATI=0x{uati:08x}"
                                        );
                                    }
                                    HrpdAccessMessage::DefaultPacketXonRequest => {
                                        log::info!(
                                            "HRPD AN: decoded reverse Stream-0 DefaultPacket XonRequest UATI=0x{uati:08x}"
                                        );
                                        outcome.default_packet_flow_open_uatis.push(*uati);
                                        if let Some(packet) = self
                                            .build_default_packet_flow_control_packet_for_uati(
                                                at,
                                                *uati,
                                                &[DEFAULT_PACKET_XON_RESPONSE],
                                                "XonResponse",
                                            )
                                        {
                                            outcome.forward_traffic.push(packet);
                                        }
                                    }
                                    HrpdAccessMessage::DefaultPacketXoffRequest => {
                                        log::info!(
                                            "HRPD AN: decoded reverse Stream-0 DefaultPacket XoffRequest UATI=0x{uati:08x}"
                                        );
                                        outcome.default_packet_flow_closed_uatis.push(*uati);
                                        if let Some(packet) = self
                                            .build_default_packet_flow_control_packet_for_uati(
                                                at,
                                                *uati,
                                                &[DEFAULT_PACKET_XOFF_RESPONSE],
                                                "XoffResponse",
                                            )
                                        {
                                            outcome.forward_traffic.push(packet);
                                        }
                                    }
                                    HrpdAccessMessage::DefaultPacketDataReadyAck(ack) => {
                                        log::info!(
                                            "HRPD AN: decoded reverse Stream-0 DefaultPacket DataReadyAck UATI=0x{uati:08x} transaction=0x{:02x}",
                                            ack.transaction_id
                                        );
                                        outcome.default_packet_data_ready_acks.push(
                                            HrpdDefaultPacketDataReadyAckEvent {
                                                uati: *uati,
                                                transaction_id: ack.transaction_id,
                                            },
                                        );
                                    }
                                    HrpdAccessMessage::DefaultPacketRlpReset(_) => {
                                        log::info!(
                                            "HRPD AN: decoded reverse Stream-0 DefaultPacket RLP Reset UATI=0x{uati:08x}"
                                        );
                                        at.reverse_default_packet_rlp.reset();
                                        outcome.default_packet_rlp_reset_uatis.push(*uati);
                                        if let Some(packet) = self
                                            .build_default_packet_flow_control_packet_for_uati(
                                                at,
                                                *uati,
                                                &[DEFAULT_PACKET_RLP_RESET_ACK],
                                                "RlpResetAck",
                                            )
                                        {
                                            outcome.forward_traffic.push(packet);
                                        }
                                    }
                                    HrpdAccessMessage::DefaultPacketRlpResetAck(_) => {
                                        log::info!(
                                            "HRPD AN: decoded reverse Stream-0 DefaultPacket RLP ResetAck UATI=0x{uati:08x}"
                                        );
                                    }
                                    HrpdAccessMessage::DefaultPacketRlpNak(nak) => {
                                        log::debug!(
                                            "HRPD AN: decoded reverse Stream-0 DefaultPacket RLP Nak UATI=0x{uati:08x} requests={}",
                                            nak.requests.len()
                                        );
                                        outcome.default_packet_rlp_naks.push(
                                            HrpdDefaultPacketRlpNakEvent {
                                                uati: *uati,
                                                requests: nak.requests.clone(),
                                            },
                                        );
                                    }
                                    HrpdAccessMessage::DefaultSignalingReset(reset) => {
                                        log::info!(
                                            "HRPD AN: decoded reverse Stream-0 DefaultSignaling Reset UATI=0x{uati:08x} seq={}",
                                            reset.message_sequence
                                        );
                                    }
                                    HrpdAccessMessage::DefaultSignalingResetAck(ack) => {
                                        self.handle_stream0_slp_reset_ack(
                                            at,
                                            *uati,
                                            ack.message_sequence,
                                            &mut outcome,
                                        );
                                    }
                                    HrpdAccessMessage::Unknown {
                                        protocol_type,
                                        message_id,
                                        payload,
                                    } => {
                                        let protocol_type = *protocol_type;
                                        let message_id = *message_id;
                                        let payload = payload.as_slice();
                                        if protocol_type
                                            == DEFAULT_STREAM0_APPLICATION_PROTOCOL_TYPE
                                        {
                                            log::info!(
                                                "HRPD AN: decoded reverse Stream-0 DefaultSignaling {} UATI=0x{uati:08x} in_config={:?} msg_id={:?} payload_len={} payload_hex={}",
                                                stream0_default_signaling_message_name(message_id),
                                                parsed.in_configuration,
                                                message_id,
                                                payload.len(),
                                                bytes_to_hex(payload)
                                            );
                                        } else if protocol_type
                                            == DEFAULT_SESSION_CONFIGURATION_PROTOCOL_TYPE
                                        {
                                            log::info!(
                                                "HRPD AN: decoded reverse Stream-0 SessionConfiguration {} UATI=0x{uati:08x} in_config={:?} msg_id={:?} payload_len={} payload_hex={}",
                                                stream0_session_configuration_message_name(
                                                    message_id
                                                ),
                                                parsed.in_configuration,
                                                message_id,
                                                payload.len(),
                                                bytes_to_hex(payload)
                                            );
                                            if message_id == Some(SESSION_CONFIGURATION_REQUEST) {
                                                outcome
                                                    .session_configuration_pending_uatis
                                                    .push(*uati);
                                            }
                                            if let Some((
                                                packet,
                                                session_config_complete,
                                                queue_soft_commit_close,
                                            )) = self.handle_session_configuration_message(
                                                at,
                                                *uati,
                                                payload,
                                                parsed.sequence_number,
                                            ) {
                                                outcome.forward_traffic.push(packet);
                                                ack_queued = true;
                                                if queue_soft_commit_close {
                                                    self.queue_session_config_commit_connection_close(
                                                        at,
                                                        *uati,
                                                        &mut outcome,
                                                    );
                                                } else {
                                                    Self::mark_soft_configuration_no_commit_sent(
                                                        at, *uati,
                                                    );
                                                }
                                                if session_config_complete {
                                                    log::info!(
                                                        "HRPD AN: SessionConfigurationComplete is waiting for AN reliable ACK UATI=0x{uati:08x}"
                                                    );
                                                }
                                            }
                                        } else {
                                            if let Some(packet) = self
                                                .handle_stream0_default_packet_message(
                                                    at,
                                                    *uati,
                                                    protocol_type,
                                                    message_id,
                                                    payload,
                                                    &mut outcome,
                                                )
                                            {
                                                outcome.forward_traffic.push(packet);
                                            } else if let Some(packet) = self
                                                .handle_stream0_protocol_configuration_message(
                                                    at,
                                                    *uati,
                                                    protocol_type,
                                                    payload,
                                                    parsed.sequence_number,
                                                    &mut outcome,
                                                )
                                            {
                                                outcome.forward_traffic.push(packet);
                                                // C.S0024 SLP-D lets the transmitter satisfy the
                                                // ACK requirement by carrying AckSequenceNumber in
                                                // the outbound reliable packet. Avoid also sending
                                                // a header-only duplicate ACK for the same reverse
                                                // reliable packet.
                                                ack_queued = parsed.sequence_number.is_some();
                                            } else if let Some(packet) = self
                                                .handle_stream0_attribute_update_message(
                                                    at,
                                                    *uati,
                                                    protocol_type,
                                                    payload,
                                                    parsed.sequence_number,
                                                )
                                            {
                                                outcome.forward_traffic.push(packet);
                                                ack_queued = parsed.sequence_number.is_some();
                                            } else if protocol_type == SESSION_PROTOCOL_KEY_EXCHANGE
                                            {
                                                let (packets, queue_soft_commit_close) = self
                                                    .handle_stream0_key_exchange_message(
                                                        at,
                                                        *uati,
                                                        payload,
                                                        parsed.sequence_number,
                                                    );
                                                if !packets.is_empty() {
                                                    outcome.forward_traffic.extend(packets);
                                                    ack_queued = true;
                                                    if queue_soft_commit_close {
                                                        self.queue_session_config_commit_connection_close(
                                                            at,
                                                            *uati,
                                                            &mut outcome,
                                                        );
                                                    } else {
                                                        Self::mark_soft_configuration_no_commit_sent(
                                                            at,
                                                            *uati,
                                                        );
                                                    }
                                                }
                                            } else if log_rtc_mac_request(
                                                *uati,
                                                protocol_type,
                                                payload,
                                            ) {
                                                // Subtype-3 Request is informational: T2P
                                                // allocation is AT-autonomous without Grants.
                                            } else {
                                                log::info!(
                                                    "HRPD AN: decoded reverse Stream-0 {} UATI=0x{uati:08x} protocol=0x{:02x} msg_id={:?} payload_len={} payload_hex={}",
                                                    stream0_message_name(protocol_type, message_id),
                                                    protocol_type,
                                                    message_id,
                                                    payload.len(),
                                                    bytes_to_hex(payload)
                                                );
                                            }
                                        }
                                    }
                                    HrpdAccessMessage::UatiRequest(_)
                                    | HrpdAccessMessage::UatiComplete(_)
                                    | HrpdAccessMessage::ConnectionRequest(_) => {}
                                }
                            } else {
                                outcome.stream0_ack_only_count += 1;
                            }
                            if !ack_queued
                                && let Some(sequence_number) = parsed.sequence_number
                                && let Some(packet) =
                                    self.build_slp_d_ack_packet_for_uati(at, *uati, sequence_number)
                            {
                                log::info!(
                                    "HRPD AN: ACKing reliable reverse Stream-0 signaling UATI=0x{uati:08x} ack_seq={sequence_number}"
                                );
                                outcome.forward_traffic.push(packet);
                            }
                        }
                        Stream0ParseOutcome::InProgress => {
                            outcome.stream0_fragment_in_progress_count += 1;
                        }
                        Stream0ParseOutcome::Invalid => {
                            outcome.stream0_invalid_count += 1;
                            log::info!(
                                "HRPD AN: undecoded reverse Stream-0 signaling UATI=0x{uati:08x} payload_bits={} payload_hex={}",
                                payload.len(),
                                bytes_to_hex(payload)
                            );
                        }
                    }
                } else {
                    log::info!(
                        "HRPD AN: dropped Stream0 traffic signaling UATI=0x{uati:08x} payload_bits={}",
                        payload.len()
                    );
                }
            }
            HrpdTrafficEvent::Stream1Packet {
                uati,
                sequence,
                payload,
                ..
            } => {
                if self.is_known_uati(at, *uati) {
                    outcome.accepted_event_count = 1;
                    if !payload.is_empty() {
                        let rlp = at.reverse_default_packet_rlp.ingest(
                            *sequence,
                            payload,
                            Instant::now(),
                        );
                        if rlp.duplicate_octets != 0 || rlp.reordered_octets != 0 {
                            log::debug!(
                                "HRPD AN: reverse DefaultPacket RLP UATI=0x{uati:08x} seq={} packet_octets={} delivered_octets={} duplicate_octets={} reordered_octets={} buffered_octets={} v_n={} v_r={}",
                                sequence,
                                payload.len(),
                                rlp.delivered.len(),
                                rlp.duplicate_octets,
                                rlp.reordered_octets,
                                at.reverse_default_packet_rlp.resequencing.len(),
                                at.reverse_default_packet_rlp.v_n,
                                at.reverse_default_packet_rlp.v_r,
                            );
                        }
                        if !rlp.delivered.is_empty() {
                            outcome.default_packet_flow_open_uatis.push(*uati);
                            outcome.a8_uplink.push(HrpdA8UplinkPacket {
                                uati: *uati,
                                payload: rlp.delivered,
                            });
                        }
                        if !rlp.nak_requests.is_empty() {
                            let ranges = rlp
                                .nak_requests
                                .iter()
                                .map(|request| {
                                    format!("{}+{}", request.first_erased, request.window_len)
                                })
                                .collect::<Vec<_>>()
                                .join(",");
                            log::info!(
                                "HRPD AN: queueing reverse DefaultPacket RLP Nak UATI=0x{uati:08x} requests={} ranges=[{}]",
                                rlp.nak_requests.len(),
                                ranges,
                            );
                            let nak_payload = default_packet_rlp_nak_payload(&rlp.nak_requests);
                            if let Some(packet) = self
                                .build_default_packet_flow_control_packet_for_uati(
                                    at,
                                    *uati,
                                    &nak_payload,
                                    "RlpNak",
                                )
                            {
                                outcome.forward_traffic.push(packet);
                            }
                        }
                    }
                } else {
                    outcome.dropped_event_count = 1;
                    outcome.unknown_session_count = 1;
                }
            }
        }
        // Re-key the AT only if it was a tracked session. A throwaway used for
        // an unknown-UATI drop never enters the map. A SessionClose drops the
        // session inside the AtSession, so a now-empty session is not
        // reinserted.
        if was_present
            && let Some(reinsert_uati) = at_owned
                .session
                .session()
                .map(|session| session.uati.as_u32())
        {
            if outcome.accepted_event_count > 0 {
                at_owned.last_activity_at = Instant::now();
            }
            self.ats.insert(reinsert_uati, at_owned);
        }
        outcome
    }

    /// Maps a traffic-link UATI (carried by reverse traffic events and the A9
    /// boundary) to the session UATI keying `ats`. The traffic UATI is the
    /// session UATI with the configured color code in its top byte, so a single
    /// matching AT is found by comparing the reconstructed receive value.
    fn session_uati_for_traffic_uati(&self, traffic_uati: u32) -> Option<u32> {
        self.ats.iter().find_map(|(session_uati, at)| {
            let matches = at
                .pending_traffic_assignment
                .as_ref()
                .is_some_and(|pending| pending.traffic_uati == traffic_uati)
                || at.session.session().is_some_and(|session| {
                    self.uati_receive_value(session.uati.as_u32()) == traffic_uati
                });
            matches.then_some(*session_uati)
        })
    }

    /// Applies an A9 `DisconnectA8` received from the PCF.
    ///
    /// A9 owns the AN/PCF A8 control boundary. When the PCF disconnects A8
    /// after packet-side teardown, the AN releases only the active traffic
    /// assignment and A8 bearer; it does not close the HRPD session or discard
    /// the committed session configuration. The next access-channel
    /// `ConnectionRequest` can therefore reopen traffic using the normal A9
    /// `SetupA8` path.
    pub fn handle_a9_disconnect_a8(
        &mut self,
        uati: u32,
        mac_index: u8,
        cause: u8,
    ) -> HrpdTrafficOutcome {
        let mut outcome = HrpdTrafficOutcome::default();
        let Some(session_uati) = self.session_uati_for_traffic_uati(uati) else {
            log::info!(
                "HRPD AN: A9 DisconnectA8 ignored with no active traffic assignment UATI=0x{uati:08x} MAC={mac_index} cause=0x{cause:02x}"
            );
            outcome.dropped_event_count = 1;
            return outcome;
        };
        let mut at = self
            .ats
            .remove(&session_uati)
            .expect("session_uati_for_traffic_uati returns a present key");
        let dropped = match at.pending_traffic_assignment.as_ref() {
            None => {
                log::info!(
                    "HRPD AN: A9 DisconnectA8 ignored with no active traffic assignment UATI=0x{uati:08x} MAC={mac_index} cause=0x{cause:02x}"
                );
                true
            }
            Some(pending) => {
                let pending_mac = pending
                    .assignment
                    .pilots
                    .first()
                    .map(|pilot| pilot.mac_index)
                    .unwrap_or(pending.traffic_request.mac_index);
                if pending.traffic_uati != uati || pending_mac != mac_index {
                    log::warn!(
                        "HRPD AN: A9 DisconnectA8 ignored for non-current traffic UATI=0x{uati:08x} MAC={mac_index} cause=0x{cause:02x}; current traffic_uati=0x{:08x} MAC={}",
                        pending.traffic_uati,
                        pending_mac
                    );
                    true
                } else {
                    false
                }
            }
        };
        if dropped {
            outcome.dropped_event_count = 1;
            self.ats.insert(session_uati, at);
            return outcome;
        }

        log::info!(
            "HRPD AN: A9 DisconnectA8 releasing traffic UATI=0x{uati:08x} MAC={mac_index} cause=0x{cause:02x}"
        );
        outcome.accepted_event_count = 1;
        let release_count_before = outcome.traffic_releases.len();
        self.release_pending_traffic_assignment_for_traffic(
            &mut at,
            &mut outcome,
            "A9 DisconnectA8",
        );
        if outcome.traffic_releases.len() > release_count_before {
            outcome.traffic_channel_closed_uatis.push(uati);
        }
        // A9 DisconnectA8 releases only the traffic assignment, not the HRPD
        // session, so the AT stays tracked for the next ConnectionRequest.
        self.ats.insert(session_uati, at);
        outcome
    }

    pub fn handle_timer(&mut self, now: Instant) -> HrpdTrafficOutcome {
        self.handle_timer_inner(now, None)
    }

    pub fn handle_timer_with_allocator(
        &mut self,
        now: Instant,
        allocator: &mut UatiAllocator,
    ) -> HrpdTrafficOutcome {
        self.handle_timer_inner(now, Some(allocator))
    }

    fn handle_timer_inner(
        &mut self,
        now: Instant,
        mut allocator: Option<&mut UatiAllocator>,
    ) -> HrpdTrafficOutcome {
        let mut outcome = HrpdTrafficOutcome::default();
        let session_uatis: Vec<u32> = self.ats.keys().copied().collect();
        for session_uati in session_uatis {
            let Some(mut at) = self.ats.remove(&session_uati) else {
                continue;
            };
            if let Some(allocator) = allocator.as_deref_mut()
                && self.maybe_close_idle_session(&mut at, now, allocator, &mut outcome)
            {
                continue;
            }
            self.maybe_expire_session_config_commit_connection_close(&mut at, now, &mut outcome);
            self.maybe_retransmit_reliable_stream0(&mut at, now, &mut outcome);
            self.maybe_expire_pending_traffic_setup(&mut at, now, &mut outcome);
            if at.session.session().is_some() {
                self.ats.insert(session_uati, at);
            }
        }
        outcome
    }

    fn maybe_close_idle_session(
        &mut self,
        at: &mut AtSession,
        now: Instant,
        allocator: &mut UatiAllocator,
        outcome: &mut HrpdTrafficOutcome,
    ) -> bool {
        let Some(session_uati) = at.session.session().map(|session| session.uati.as_u32()) else {
            return false;
        };
        if now.saturating_duration_since(at.last_activity_at) < HRPD_SESSION_IDLE_TIMEOUT {
            return false;
        }
        let receive_uati = self.uati_receive_value(session_uati);
        log::info!(
            "HRPD AN: idle session timeout; queueing SessionClose and reclaiming session_uati=0x{session_uati:08x} receive_uati=0x{receive_uati:08x} idle_secs={}",
            now.saturating_duration_since(at.last_activity_at).as_secs()
        );
        outcome
            .forward_signaling
            .push(HrpdForwardSignalingRequest::session_close(
                self.uati_receive_ati_from_value(receive_uati),
                SESSION_CLOSE_REASON_SESSION_LOST,
                &[],
            ));
        self.close_session_from_traffic(
            at,
            receive_uati,
            allocator,
            outcome,
            "idle session timeout",
        );
        true
    }

    fn cancel_access_traffic_outputs_for_session_close(&self, outcome: &mut HrpdAccessOutcome) {
        let assignments = outcome.traffic_assignments.len();
        let forward_traffic = outcome.forward_traffic.len();
        let signaling_before = outcome.forward_signaling.len();
        outcome.traffic_assignments.clear();
        outcome.forward_traffic.clear();
        outcome.forward_signaling.retain(|request| {
            request.protocol_type != DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE
                || request.payload.first().copied()
                    != Some(HrpdTrafficChannelAssignment::MESSAGE_ID)
        });
        let removed_signaling = signaling_before.saturating_sub(outcome.forward_signaling.len());
        if assignments != 0 || forward_traffic != 0 || removed_signaling != 0 {
            log::info!(
                "HRPD AN: SessionClose cancelled same-access traffic outputs assignments={} tca_signaling={} forward_traffic={}",
                assignments,
                removed_signaling,
                forward_traffic
            );
        }
    }

    fn connection_request_waiting_for_assigned_uati(
        &self,
        at: &AtSession,
        ati: AccessTerminalIdentifier,
    ) -> bool {
        if at.session.state() != SessionState::AmpSetup
            || at.expected_uati_complete_sequence.is_none()
            || ati.ati_type != AccessTerminalIdentifierType::Uati
        {
            return false;
        }
        let Some(session) = at.session.session() else {
            return false;
        };
        ati == self.uati_receive_ati(session.uati.as_u32())
    }

    fn handle_deferred_connection_after_uati_complete(
        &mut self,
        at: &mut AtSession,
        outcome: &mut HrpdAccessOutcome,
    ) {
        let Some(deferred) = at.deferred_connection_after_uati_complete.take() else {
            return;
        };
        log::info!(
            "HRPD AN: replaying deferred ConnectionRequest transaction=0x{:02x} from {:?} after UATIComplete",
            deferred.request.transaction_id,
            deferred.ati
        );
        self.handle_connection_request(at, &deferred.request, deferred.access_slot, outcome);
    }

    fn on_session_outbound(
        &mut self,
        at: &mut AtSession,
        request_ati: AccessTerminalIdentifier,
        transaction_id: Option<u8>,
        outbound: &[OutboundSessionMessage],
        outcome: &mut HrpdAccessOutcome,
    ) {
        for message in outbound {
            match message {
                OutboundSessionMessage::UatiAssignment(uati) => {
                    let sequence = Self::next_uati_assignment_sequence(at);
                    at.expected_uati_complete_sequence = Some(sequence);
                    if let Some(transaction_id) = transaction_id {
                        at.last_uati_assignment = Some(CachedUatiAssignment {
                            request_ati,
                            transaction_id,
                            uati: uati.as_u32(),
                            sequence,
                        });
                    }
                    outcome
                        .forward_signaling
                        .push(HrpdForwardSignalingRequest::uati_assignment(
                            uati.as_u32(),
                            sequence,
                            self.color_code,
                            request_ati,
                            self.uati_assignment_subnet_for_request(request_ati),
                        ));
                }
                OutboundSessionMessage::SessionClose => {
                    outcome
                        .forward_signaling
                        .push(HrpdForwardSignalingRequest::session_close(
                            request_ati,
                            0,
                            &[],
                        ));
                }
                OutboundSessionMessage::SessionConfigurationResponse(_) => {}
            }
        }
    }

    fn retransmit_cached_uati_assignment(
        &self,
        at: &AtSession,
        request_ati: AccessTerminalIdentifier,
        request: &HrpdUatiRequest,
        outcome: &mut HrpdAccessOutcome,
    ) -> bool {
        let Some(cached) = at.last_uati_assignment else {
            return false;
        };
        if cached.request_ati != request_ati
            || cached.transaction_id != request.transaction_id
            || at.expected_uati_complete_sequence != Some(cached.sequence)
        {
            return false;
        }
        log::info!(
            "HRPD AN: retransmitting UATIAssignment seq={} UATI=0x{:08x} for repeated {:?}/transaction=0x{:02x}",
            cached.sequence,
            cached.uati,
            request_ati,
            request.transaction_id
        );
        outcome
            .forward_signaling
            .push(HrpdForwardSignalingRequest::uati_assignment(
                cached.uati,
                cached.sequence,
                self.color_code,
                request_ati,
                self.uati_assignment_subnet_for_request(request_ati),
            ));
        true
    }

    fn retransmit_pending_uati_assignment_to_current_ati(
        &self,
        at: &AtSession,
        current_ati: AccessTerminalIdentifier,
        outcome: &mut HrpdAccessOutcome,
    ) -> bool {
        let Some(cached) = at.last_uati_assignment else {
            return false;
        };
        if at.expected_uati_complete_sequence != Some(cached.sequence) {
            return false;
        }
        let Some(session) = at.session.session() else {
            return false;
        };
        if session.uati.as_u32() != cached.uati || current_ati != self.uati_receive_ati(cached.uati)
        {
            return false;
        }

        log::info!(
            "HRPD AN: retransmitting pending UATIAssignment seq={} UATI=0x{:08x} to current {:?} after UATI-scoped access before UATIComplete",
            cached.sequence,
            cached.uati,
            current_ati
        );
        outcome
            .forward_signaling
            .push(HrpdForwardSignalingRequest::uati_assignment(
                cached.uati,
                cached.sequence,
                self.color_code,
                current_ati,
                self.uati_assignment_subnet_for_request(current_ati),
            ));
        true
    }

    fn uati_request_matches_pending_traffic_setup(
        &self,
        at: &AtSession,
        ati: AccessTerminalIdentifier,
    ) -> bool {
        let Some(pending) = at
            .pending_traffic_assignment
            .as_ref()
            .filter(|pending| !pending.active)
        else {
            return false;
        };

        ati == self.uati_receive_ati_from_value(pending.traffic_uati)
            || ati == self.uati_receive_ati_from_value(pending.session_uati)
            || ati.ati_type == AccessTerminalIdentifierType::Uati
    }

    fn handle_connection_request(
        &mut self,
        at: &mut AtSession,
        request: &HrpdConnectionRequest,
        access_slot: Option<u64>,
        outcome: &mut HrpdAccessOutcome,
    ) {
        outcome.connection_requested = true;
        outcome.connection_requests.push(request.clone());
        let Some(session) = at.session.session() else {
            log::info!(
                "HRPD AN: ConnectionRequest transaction=0x{:02x} ignored without active session",
                request.transaction_id
            );
            return;
        };
        let uati = session.uati.as_u32();
        let traffic_uati = self.uati_receive_value(uati);
        let receive_ati = self.uati_receive_ati_from_value(traffic_uati);
        let pending_matches_session =
            |pending: &PendingTrafficAssignment| pending.session_uati == uati;
        let active_pending_same_session = at
            .pending_traffic_assignment
            .as_ref()
            .is_some_and(|pending| pending_matches_session(pending) && pending.active);
        if at
            .last_completed_connection_request
            .is_some_and(|completed| {
                completed.session_uati == uati
                    && completed.traffic_uati == traffic_uati
                    && completed.transaction_id == request.transaction_id
            })
            && !active_pending_same_session
        {
            log::info!(
                "HRPD AN: ignoring duplicate completed ConnectionRequest transaction=0x{:02x} session_uati=0x{:08x} traffic_uati=0x{:08x}",
                request.transaction_id,
                uati,
                traffic_uati
            );
            return;
        }
        // A ConnectionRequest is an Idle State message: an AT with a live
        // traffic connection never sends one. A *new* transaction arriving
        // while an assignment is still marked active means the AT already
        // abandoned that channel, so waiting out DRC-silence/pilot-loss
        // supervision only delays the retry — release the stale assignment
        // and hand the AT a fresh one now. A repeat of the transaction that
        // opened the active assignment is just a late access-probe
        // retransmission and keeps the current connection.
        match at
            .pending_traffic_assignment
            .as_ref()
            .filter(|pending| pending_matches_session(pending) && pending.active)
        {
            Some(pending)
                if pending.connection_request_transaction_id == request.transaction_id =>
            {
                log::info!(
                    "HRPD AN: ignoring duplicate ConnectionRequest transaction=0x{:02x} while traffic assignment is active session_uati=0x{:08x} traffic_uati=0x{:08x}",
                    request.transaction_id,
                    uati,
                    pending.traffic_uati
                );
                return;
            }
            Some(_) => {
                log::info!(
                    "HRPD AN: new ConnectionRequest transaction=0x{:02x} reason={} while traffic assignment is active session_uati=0x{:08x}; releasing stale assignment and reassigning",
                    request.transaction_id,
                    request.request_reason,
                    uati,
                );
                self.release_pending_traffic_assignment(
                    at,
                    outcome,
                    "AT sent new ConnectionRequest from idle",
                );
            }
            None => {}
        }
        if let Some(pending) = at
            .pending_traffic_assignment
            .as_ref()
            .filter(|pending| pending_matches_session(pending))
        {
            {
                let route_update_subtype = Self::current_route_update_subtype(at);
                let default_route_update_tca_rev_a_tail =
                    Self::current_default_route_update_tca_rev_a_tail(at);
                log::info!(
                    "HRPD AN: retransmitting pending TrafficChannelAssignment session_uati=0x{:08x} traffic_uati=0x{:08x} setup_transaction=0x{:02x} retry_transaction=0x{:02x} MAC={} seq={} route_update_subtype=0x{:04x} default_tca_rev_a_tail={} reliable_seq={:?} access_slot={:?} last_rtc_ack_slot={:?} rtc_acquired={}",
                    uati,
                    pending.traffic_uati,
                    pending.connection_request_transaction_id,
                    request.transaction_id,
                    pending
                        .assignment
                        .pilots
                        .first()
                        .map(|pilot| pilot.mac_index)
                        .unwrap_or_default(),
                    pending.assignment.message_sequence,
                    route_update_subtype,
                    default_route_update_tca_rev_a_tail,
                    pending.tca_reliable_sequence,
                    access_slot,
                    pending.rtc_ack_last_send_slot,
                    pending.rtc_acquired,
                );
                let assignment = pending.assignment.clone();
                let tca_reliable_sequence = pending.tca_reliable_sequence;
                if let Some(pending) = at
                    .pending_traffic_assignment
                    .as_mut()
                    .filter(|pending| pending_matches_session(pending))
                {
                    pending.setup_start_slot = access_slot.or(pending.setup_start_slot);
                    pending.setup_started_at = Instant::now();
                    if pending.rtc_acquired {
                        pending.rtc_ack_needs_send = true;
                        pending.rtc_ack_delivered = false;
                        pending.rtc_grant_last_send_slot = None;
                        pending.rtc_grant_sends = 0;
                        // A fresh access-channel retry means the AT still has
                        // not completed traffic setup. Re-arm the bounded
                        // DRC-paced RTCAck loop for this retry instead of
                        // inheriting an exhausted blind retransmit budget from
                        // the previous attempt.
                        pending.rtc_ack_retransmits = 0;
                    }
                }
                outcome
                    .forward_signaling
                    .push(Self::traffic_channel_assignment_signaling(
                        uati,
                        receive_ati,
                        assignment,
                        route_update_subtype,
                        default_route_update_tca_rev_a_tail,
                        tca_reliable_sequence,
                    ));
                return;
            }
        } else {
            // A pending assignment that survived to this point belongs to an
            // abandoned earlier session (the same-session retry path returned
            // above). Release its BTS traffic receiver and MAC channel before
            // assigning the new one, or every abandoned attempt leaks a worker
            // that scans until process shutdown.
            self.release_pending_traffic_assignment(
                at,
                outcome,
                "replaced by new ConnectionRequest",
            );
        }
        // C.S0024-400 §1.9.6.1.3.2: the AN sets the TCA MessageSequence it
        // sends in Idle State to 0. Connected-state active-set updates are the
        // incrementing/reliable-TCA case; this path is a fresh traffic open.
        let sequence = 0;
        let mac_index = self.allocate_mac_index();
        let tca_reliable_sequence = None;
        let route_update_subtype = Self::current_route_update_subtype(at);
        let default_route_update_tca_rev_a_tail =
            Self::current_default_route_update_tca_rev_a_tail(at);
        log::info!(
            "HRPD AN: ConnectionRequest transaction=0x{:02x} reason={} -> TrafficChannelAssignment session_uati=0x{:08x} traffic_uati=0x{:08x} MAC={} seq={} route_update_subtype=0x{:04x} default_tca_rev_a_tail={} reliable_seq={:?} channel={:?}",
            request.transaction_id,
            request.request_reason,
            uati,
            traffic_uati,
            mac_index,
            sequence,
            route_update_subtype,
            default_route_update_tca_rev_a_tail,
            tca_reliable_sequence,
            self.channel
        );
        let assignment = HrpdTrafficChannelAssignment::single_pilot(
            sequence,
            self.channel,
            self.sector_pilot_pn,
            mac_index,
        );
        outcome
            .forward_signaling
            .push(Self::traffic_channel_assignment_signaling(
                uati,
                self.uati_receive_ati_from_value(traffic_uati),
                assignment.clone(),
                route_update_subtype,
                default_route_update_tca_rev_a_tail,
                tca_reliable_sequence,
            ));
        let (reverse_long_code_mask_i, reverse_long_code_mask_q) =
            default_reverse_traffic_long_code_masks(traffic_uati);
        self.last_drc_by_uati.remove(&traffic_uati);
        // The TCA stores DRCLength as a 2-bit wire code (C.S0024-0
        // §6.7.4.3): 0->1 slot, 1->2 slots, 2->4 slots, 3->8 slots. The
        // receive path wants the actual slot count for DRC integration.
        let drc_length_slots = 1u8 << (assignment.drc_length & 0x03);
        // Per the first assigned pilot. Single-pilot operation today; if
        // a softer-handoff pilot is added later, the BTS will need to know
        // which pilot to demod against.
        let drc_cover = assignment
            .pilots
            .first()
            .map(|p| p.drc_cover & 0x07)
            .unwrap_or(0);
        let (
            in_use_physical_layer_subtype,
            in_use_forward_traffic_mac_subtype,
            in_use_reverse_traffic_mac_subtype,
        ) = Self::current_session_traffic_subtypes(at);
        let reverse_rate_limit_bps = if in_use_physical_layer_subtype
            == SESSION_SUBTYPE_PHYS_SUBTYPE2
            && in_use_reverse_traffic_mac_subtype == SESSION_SUBTYPE_RTC_MAC_SUBTYPE3
        {
            1_843_200
        } else {
            153_600
        };
        let traffic_request = HrpdTrafficAssignmentRequest {
            session_uati: uati,
            uati: traffic_uati,
            mac_index,
            reverse_rate_limit_bps,
            reverse_long_code_mask_i,
            reverse_long_code_mask_q,
            // Pinned locked: with a single sector the AT has no other sector to
            // switch to, so DRCLock supervision is a no-op. Driven dynamically
            // from the reverse-DRC decode lock loop once multi-sector is supported.
            drc_lock: true,
            physical_layer_subtype: in_use_physical_layer_subtype,
            reverse_traffic_mac_subtype: in_use_reverse_traffic_mac_subtype,
            frame_offset: assignment.frame_offset & 0x0f,
            drc_cover,
            drc_length: drc_length_slots,
        };
        outcome.traffic_assignments.push(traffic_request.clone());
        // SLP-D resets on each connection initiation (C.S0024-0 §2.6.4.2.3.2):
        // both sides return V(S)/V(N) to zero, so the first reliable forward
        // message of every connection must carry SequenceNumber 0. Without
        // this, the second and later connections send RTCAck at V(S) 1, 2, …
        // against an AT receiver expecting 0, and the AT's in-order SLP-D
        // delivery can hold the message in its resequencing buffer forever.
        at.slp_d_vs_stream0_fl = 0;
        at.reverse_default_packet_rlp.reset();
        self.stream0_slp_f_rx.remove(&traffic_uati);
        let mut pending = PendingTrafficAssignment {
            session_uati: uati,
            traffic_uati,
            connection_request_transaction_id: request.transaction_id,
            assignment,
            tca_reliable_sequence,
            traffic_request,
            active: false,
            rtc_ack_vs: None,
            rtc_ack_needs_send: true,
            rtc_acquired: false,
            drc_events_since_rtc_ack: 0,
            rtc_ack_retransmits: 0,
            rtc_ack_delivered: false,
            rtc_grant_last_send_slot: None,
            rtc_grant_sends: 0,
            rtc_ack_last_send_slot: None,
            setup_start_slot: access_slot,
            setup_started_at: Instant::now(),
            session_config_start_sent: at.session_configuration_complete,
            session_config_complete_sent: false,
            an_session_config_complete_acked: false,
            session_config_commit_connection_close_pending: false,
            session_config_commit_connection_close_sent: false,
            session_config_commit_connection_close_sent_at: None,
            at_session_config_complete_transaction_id: None,
            stream0_slp_reset_sequence: 0,
            stream0_slp_reset_pending: false,
            stream0_slp_reset_acked: false,
            stream0_slp_reset_attempts: 0,
            stream0_slp_reset_last_send_slot: None,
            reliable_stream0_tx: Vec::new(),
            reverse_stream0_slp_d_vn: 0,
            reverse_stream0_slp_d_rx: 0,
            session_config_trace: None,
            in_use_physical_layer_subtype,
            session_personality_count: SESSION_PERSONALITY_COUNT_DEFAULT,
            protocol_config_traces: Vec::new(),
            dh_key_exchange: None,
            dh_key_exchange_complete: false,
            in_use_forward_traffic_mac_subtype,
            in_use_reverse_traffic_mac_subtype,
        };
        if let Some(packet) = Self::build_rtc_ack_packet(
            &mut pending,
            &mut at.slp_d_vs_stream0_fl,
            HRPD_RTC_ACK_DRC_INDEX,
            None,
        ) {
            pending.rtc_ack_needs_send = true;
            log::info!(
                "HRPD AN: prequeueing setup RTCAck UATI=0x{traffic_uati:08x} MAC={mac_index}; scheduler will bind exact governing DRC"
            );
            outcome.forward_traffic.push(packet);
        }
        at.pending_traffic_assignment = Some(pending);
    }

    fn maybe_accept_pending_uati_from_connection_request(
        &mut self,
        at: &mut AtSession,
        ati: AccessTerminalIdentifier,
        allocator: &mut UatiAllocator,
        outcome: &mut HrpdAccessOutcome,
    ) -> Result<(), StateMachineError> {
        if at.session.state() != SessionState::AmpSetup {
            log::info!(
                "HRPD AN: UATI-scoped ConnectionRequest did not synthesize UATIComplete; state={:?}",
                at.session.state()
            );
            return Ok(());
        }
        let Some(session) = at.session.session() else {
            log::info!(
                "HRPD AN: UATI-scoped ConnectionRequest did not synthesize UATIComplete; no session"
            );
            return Ok(());
        };
        if ati.ati_type != AccessTerminalIdentifierType::Uati {
            log::info!(
                "HRPD AN: UATI-scoped ConnectionRequest did not synthesize UATIComplete; ati_type={:?}",
                ati.ati_type
            );
            return Ok(());
        }
        let expected_ati = self.uati_receive_ati(session.uati.as_u32());
        if ati != expected_ati {
            log::info!(
                "HRPD AN: UATI-scoped ConnectionRequest ATI mismatch got={:?} expected={:?} session_uati=0x{:08x}",
                ati,
                expected_ati,
                session.uati.as_u32()
            );
            return Ok(());
        }
        let _ = (allocator, outcome);
        log::info!(
            "HRPD AN: UATI-scoped ConnectionRequest did not synthesize UATIComplete; waiting for explicit UATIComplete ati={:?} session_uati=0x{:08x}",
            ati,
            session.uati.as_u32()
        );
        Ok(())
    }

    fn maybe_restore_cached_uati_from_connection_request(
        &mut self,
        at: &mut AtSession,
        ati: AccessTerminalIdentifier,
        allocator: &mut UatiAllocator,
        outcome: &mut HrpdAccessOutcome,
    ) -> Result<(), StateMachineError> {
        let Some(uati) = Self::cached_uati_from_access_ati(ati, allocator, "ConnectionRequest")
        else {
            return Ok(());
        };
        let current_uati = at.session.session().map(|session| session.uati.as_u32());
        if current_uati == Some(uati.as_u32()) {
            return Ok(());
        }
        if current_uati.is_none() {
            self.queue_session_lost_for_orphaned_cached_uati(
                ati,
                uati,
                outcome,
                "ConnectionRequest",
            );
            return Ok(());
        }
        if at.session.state() == SessionState::AmpSetup
            && current_uati.is_some_and(|current| self.uati_receive_value(current) == ati.value)
        {
            return Ok(());
        }
        allocator.reserve(uati)?;
        let previous_state = at.session.state();
        at.session.restore_open_session(uati);
        at.expected_uati_complete_sequence = None;
        at.last_uati_assignment = None;
        log::info!(
            "HRPD AN: restored UATI=0x{:08x} from UATI-scoped ConnectionRequest ati={:?}; previous_state={:?} previous_uati={}",
            uati.as_u32(),
            ati,
            previous_state,
            current_uati
                .map(|value| format!("0x{value:08x}"))
                .unwrap_or_else(|| "none".to_string())
        );
        Ok(())
    }

    fn maybe_session_close_orphaned_cached_uati_access(
        &self,
        at: &AtSession,
        ati: AccessTerminalIdentifier,
        allocator: &UatiAllocator,
        outcome: &mut HrpdAccessOutcome,
        message_name: &str,
    ) -> bool {
        let Some(uati) = Self::cached_uati_from_access_ati(ati, allocator, message_name) else {
            return false;
        };
        if at.session.session().is_some() {
            return false;
        }
        self.queue_session_lost_for_orphaned_cached_uati(ati, uati, outcome, message_name);
        true
    }

    fn queue_session_lost_for_orphaned_cached_uati(
        &self,
        ati: AccessTerminalIdentifier,
        uati: Uati,
        outcome: &mut HrpdAccessOutcome,
        message_name: &str,
    ) {
        let already_queued = outcome.forward_signaling.iter().any(|request| {
            request.target_ati == ati
                && request.protocol_type == DEFAULT_SESSION_MANAGEMENT_PROTOCOL_TYPE
                && request.payload == [0x01, SESSION_CLOSE_REASON_SESSION_LOST, 0x00]
        });
        if already_queued {
            return;
        }
        log::info!(
            "HRPD AN: cached UATI {message_name} has no in-memory session state; sending SessionClose(Session Lost) reconstructed UATI=0x{:08x} ati={:?}",
            uati.as_u32(),
            ati
        );
        outcome
            .forward_signaling
            .push(HrpdForwardSignalingRequest::session_close(
                ati,
                SESSION_CLOSE_REASON_SESSION_LOST,
                &[],
            ));
    }

    fn cached_uati_from_access_ati(
        ati: AccessTerminalIdentifier,
        allocator: &UatiAllocator,
        message_name: &str,
    ) -> Option<Uati> {
        if ati.ati_type != AccessTerminalIdentifierType::Uati {
            return None;
        }
        let subnet = allocator.subnet();
        if ((ati.value >> 24) as u8) != subnet.color_code {
            log::info!(
                "HRPD AN: cached UATI {message_name} ignored; ATI color_code={} configured={}",
                ati.value >> 24,
                subnet.color_code
            );
            return None;
        }
        let uati = Uati::from_compact(
            ati.value & 0x00ff_ffff,
            subnet.uati104,
            subnet.color_code,
            subnet.subnet_mask,
        );
        if !allocator.contains(uati) {
            log::info!(
                "HRPD AN: cached UATI {message_name} ignored; reconstructed UATI=0x{:08x} does not match allocator UATI104/color/subnet",
                uati.as_u32()
            );
            return None;
        }
        Some(uati)
    }

    fn close_session_from_access(
        &mut self,
        at: &mut AtSession,
        request_ati: AccessTerminalIdentifier,
        allocator: &mut UatiAllocator,
        outcome: &mut HrpdAccessOutcome,
        reason: &str,
    ) -> Result<(), StateMachineError> {
        let session_uati = at.session.session().map(|session| session.uati.as_u32());
        let receive_uati = session_uati
            .map(|uati| self.uati_receive_value(uati))
            .or_else(|| {
                (request_ati.ati_type == AccessTerminalIdentifierType::Uati)
                    .then_some(request_ati.value)
            });
        let traffic_uati = at
            .pending_traffic_assignment
            .as_ref()
            .map(|pending| pending.traffic_uati)
            .or(receive_uati);

        if let Some(uati) = receive_uati
            && !outcome.session_closed_uatis.contains(&uati)
        {
            outcome.session_closed_uatis.push(uati);
        }

        at.session_configuration_complete = false;
        Self::clear_committed_session_configuration(at);
        self.release_pending_traffic_assignment(at, outcome, reason);
        if let (Some(session_uati), Some(traffic_uati)) = (session_uati, traffic_uati) {
            self.clear_session_side_state(session_uati, traffic_uati);
        }
        let outbound = at
            .session
            .on_message(InboundSessionMessage::ConnectionClose, allocator)?;
        self.on_session_outbound(at, request_ati, None, &outbound, outcome);
        outcome.session_outbound.extend(outbound);
        log::info!(
            "HRPD AN: closed session reason={reason} session_uati={} receive_uati={} traffic_uati={}",
            session_uati
                .map(|uati| format!("0x{uati:08x}"))
                .unwrap_or_else(|| "none".to_string()),
            receive_uati
                .map(|uati| format!("0x{uati:08x}"))
                .unwrap_or_else(|| "none".to_string()),
            traffic_uati
                .map(|uati| format!("0x{uati:08x}"))
                .unwrap_or_else(|| "none".to_string()),
        );
        Ok(())
    }

    fn close_session_from_traffic(
        &mut self,
        at: &mut AtSession,
        traffic_uati: u32,
        allocator: &mut UatiAllocator,
        outcome: &mut HrpdTrafficOutcome,
        reason: &str,
    ) {
        let session_uati = at.session.session().map(|session| session.uati.as_u32());
        let receive_uati = session_uati.map(|uati| self.uati_receive_value(uati));
        let had_pending_traffic = at.pending_traffic_assignment.is_some();

        Self::log_pending_config_trace(at, traffic_uati, reason);
        at.session_configuration_complete = false;
        Self::clear_committed_session_configuration(at);
        self.release_pending_traffic_assignment_for_traffic(at, outcome, reason);
        if had_pending_traffic && !outcome.traffic_channel_closed_uatis.contains(&traffic_uati) {
            outcome.traffic_channel_closed_uatis.push(traffic_uati);
        }
        if !outcome.session_closed_uatis.contains(&traffic_uati) {
            outcome.session_closed_uatis.push(traffic_uati);
        }
        if let Some(session_uati) = session_uati {
            self.clear_session_side_state(session_uati, traffic_uati);
        }
        if let Err(err) = at
            .session
            .on_message(InboundSessionMessage::ConnectionClose, allocator)
        {
            log::warn!("HRPD AN: failed to release UATI on {reason}: {err}");
        }
        log::info!(
            "HRPD AN: closed session reason={reason} session_uati={} receive_uati={} traffic_uati=0x{traffic_uati:08x}",
            session_uati
                .map(|uati| format!("0x{uati:08x}"))
                .unwrap_or_else(|| "none".to_string()),
            receive_uati
                .map(|uati| format!("0x{uati:08x}"))
                .unwrap_or_else(|| "none".to_string()),
        );
    }

    fn clear_session_side_state(&mut self, session_uati: u32, traffic_uati: u32) {
        let receive_uati = self.uati_receive_value(session_uati);
        self.hardware_ids_by_uati.remove(&session_uati);
        self.hardware_ids_by_uati.remove(&receive_uati);
        self.hardware_ids_by_uati.remove(&traffic_uati);
        self.last_drc_by_uati.remove(&receive_uati);
        self.last_drc_by_uati.remove(&traffic_uati);
        self.stream0_slp_f_rx.remove(&receive_uati);
        self.stream0_slp_f_rx.remove(&traffic_uati);
    }

    /// Drop the pending traffic assignment and tell the BTS to stop its
    /// reverse traffic receiver and remove the forward MAC channel.
    fn release_pending_traffic_assignment(
        &mut self,
        at: &mut AtSession,
        outcome: &mut HrpdAccessOutcome,
        reason: &str,
    ) {
        if let Some(uati) = at.pending_traffic_assignment.as_ref().and_then(|pending| {
            (pending.session_config_trace.is_some() || !pending.protocol_config_traces.is_empty())
                .then_some(pending.traffic_uati)
        }) {
            Self::log_pending_config_trace(at, uati, reason);
        }
        let Some(pending) = at.pending_traffic_assignment.take() else {
            return;
        };
        let mac_index = pending
            .assignment
            .pilots
            .first()
            .map(|pilot| pilot.mac_index)
            .unwrap_or(pending.traffic_request.mac_index);
        log::info!(
            "HRPD AN: releasing traffic assignment session_uati=0x{:08x} traffic_uati=0x{:08x} MAC={} ({reason})",
            pending.session_uati,
            pending.traffic_uati,
            mac_index,
        );
        if at
            .last_completed_connection_request
            .is_some_and(|completed| {
                completed.session_uati == pending.session_uati
                    && completed.traffic_uati == pending.traffic_uati
            })
        {
            at.last_completed_connection_request = None;
        }
        self.last_drc_by_uati.remove(&pending.traffic_uati);
        self.stream0_slp_f_rx.remove(&pending.traffic_uati);
        outcome.traffic_releases.push(HrpdTrafficReleaseRequest {
            uati: pending.traffic_uati,
            mac_index,
        });
    }

    fn release_pending_traffic_assignment_for_traffic(
        &mut self,
        at: &mut AtSession,
        outcome: &mut HrpdTrafficOutcome,
        reason: &str,
    ) {
        if let Some(uati) = at.pending_traffic_assignment.as_ref().and_then(|pending| {
            (pending.session_config_trace.is_some() || !pending.protocol_config_traces.is_empty())
                .then_some(pending.traffic_uati)
        }) {
            Self::log_pending_config_trace(at, uati, reason);
        }
        let Some(pending) = at.pending_traffic_assignment.take() else {
            return;
        };
        let mac_index = pending
            .assignment
            .pilots
            .first()
            .map(|pilot| pilot.mac_index)
            .unwrap_or(pending.traffic_request.mac_index);
        log::info!(
            "HRPD AN: releasing traffic assignment session_uati=0x{:08x} traffic_uati=0x{:08x} MAC={} ({reason})",
            pending.session_uati,
            pending.traffic_uati,
            mac_index,
        );
        if at
            .last_completed_connection_request
            .is_some_and(|completed| {
                completed.session_uati == pending.session_uati
                    && completed.traffic_uati == pending.traffic_uati
            })
        {
            at.last_completed_connection_request = None;
        }
        self.last_drc_by_uati.remove(&pending.traffic_uati);
        self.stream0_slp_f_rx.remove(&pending.traffic_uati);
        outcome.traffic_releases.push(HrpdTrafficReleaseRequest {
            uati: pending.traffic_uati,
            mac_index,
        });
    }

    fn clear_committed_session_configuration(at: &mut AtSession) {
        at.committed_session_configuration_response = None;
        at.committed_protocol_configuration_responses.clear();
    }

    fn committed_session_traffic_subtypes(at: &AtSession) -> Option<(u16, u16, u16)> {
        let response = at.committed_session_configuration_response.as_deref()?;
        Some((
            session_config_selected_u16_attribute(
                response,
                [0x00, SESSION_PROTOCOL_PHYSICAL_LAYER],
            )
            .unwrap_or(SESSION_SUBTYPE_DEFAULT),
            session_config_selected_u16_attribute(
                response,
                [0x00, SESSION_PROTOCOL_FORWARD_TRAFFIC_CHANNEL_MAC],
            )
            .unwrap_or(SESSION_SUBTYPE_DEFAULT),
            session_config_selected_u16_attribute(
                response,
                [0x00, SESSION_PROTOCOL_REVERSE_TRAFFIC_CHANNEL_MAC],
            )
            .unwrap_or(SESSION_SUBTYPE_DEFAULT),
        ))
    }

    fn current_session_traffic_subtypes(at: &AtSession) -> (u16, u16, u16) {
        if at.session_configuration_complete {
            Self::committed_session_traffic_subtypes(at).unwrap_or((
                SESSION_SUBTYPE_DEFAULT,
                SESSION_SUBTYPE_DEFAULT,
                SESSION_SUBTYPE_DEFAULT,
            ))
        } else {
            (
                SESSION_SUBTYPE_DEFAULT,
                SESSION_SUBTYPE_DEFAULT,
                SESSION_SUBTYPE_DEFAULT,
            )
        }
    }

    fn current_route_update_subtype(at: &AtSession) -> u16 {
        if !at.session_configuration_complete {
            return SESSION_SUBTYPE_DEFAULT;
        }
        at.committed_session_configuration_response
            .as_deref()
            .and_then(|response| {
                session_config_selected_u16_attribute(
                    response,
                    [0x00, DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE],
                )
            })
            .unwrap_or(SESSION_SUBTYPE_DEFAULT)
    }

    fn current_default_route_update_tca_rev_a_tail(_at: &AtSession) -> bool {
        // The optional Rev A TCA tail (RAChannelGain/MACIndexMSBs/
        // DSCChannelGainBase/DSC, §8.7.6.2.2) is never sent: with the tail
        // included, the live Rev A handset accepts the TCA and keys the
        // reverse pilot but never transmits TrafficChannelComplete or any
        // reverse data, then falls back to idle. Omitting the tail is
        // format-legal and the handset proceeds through traffic setup and
        // carries data on the defaults. Revisit (starting from the encoder's
        // value coding) before any multi-sector deployment where the DSC and
        // RA demod gains stop being ignorable.
        false
    }

    #[cfg(test)]
    fn default_route_update_tca_rev_a_tail_eligible(at: &AtSession) -> bool {
        if Self::current_route_update_subtype(at) != SESSION_SUBTYPE_DEFAULT {
            return false;
        }
        let (physical_layer_subtype, forward_traffic_mac_subtype, reverse_traffic_mac_subtype) =
            Self::current_session_traffic_subtypes(at);
        Self::traffic_personality_uses_default_route_update_rev_a_tail(
            physical_layer_subtype,
            forward_traffic_mac_subtype,
            reverse_traffic_mac_subtype,
        )
    }

    fn traffic_personality_uses_default_route_update_rev_a_tail(
        physical_layer_subtype: u16,
        forward_traffic_mac_subtype: u16,
        reverse_traffic_mac_subtype: u16,
    ) -> bool {
        physical_layer_subtype == SESSION_SUBTYPE_PHYS_SUBTYPE2
            && forward_traffic_mac_subtype == SESSION_SUBTYPE_ENHANCED
            && reverse_traffic_mac_subtype == SESSION_SUBTYPE_RTC_MAC_SUBTYPE3
    }

    fn current_idle_page_timing(at: &AtSession) -> (Option<u16>, u16) {
        let preferred_cycle = at
            .session_configuration_complete
            .then_some(())
            .and_then(|()| {
                at.committed_protocol_configuration_responses
                    .get(&DEFAULT_IDLE_STATE_PROTOCOL_TYPE)
            })
            .and_then(|attrs| selected_idle_preferred_control_channel_cycle(attrs));
        (preferred_cycle, ENHANCED_IDLE_DEFAULT_PAGE_PERIOD_CYCLES)
    }

    fn session_configuration_complete_event(
        at: &AtSession,
        uati: u32,
        physical_layer_subtype: u16,
        forward_traffic_mac_subtype: u16,
    ) -> HrpdSessionConfigurationCompleteEvent {
        let (idle_preferred_control_channel_cycle, idle_page_period_cycles) =
            Self::current_idle_page_timing(at);
        HrpdSessionConfigurationCompleteEvent {
            uati,
            physical_layer_subtype,
            forward_traffic_mac_subtype,
            idle_preferred_control_channel_cycle,
            idle_page_period_cycles,
        }
    }

    fn pending_configuration_changes_in_use(
        at: &AtSession,
        pending: &PendingTrafficAssignment,
    ) -> bool {
        let session_changed = match (
            at.committed_session_configuration_response.as_ref(),
            pending.session_config_trace.as_ref(),
        ) {
            (Some(committed), Some(trace)) => committed != &trace.response_attrs,
            (None, Some(trace)) => !trace.response_attrs.is_empty(),
            (_, None) => false,
        };
        if session_changed {
            return true;
        }
        pending.protocol_config_traces.iter().any(|trace| {
            let pending_response = canonical_protocol_configuration_response(
                trace.protocol_type,
                &trace.request_attrs,
                &trace.response_attrs,
            );
            at.committed_protocol_configuration_responses
                .get(&trace.protocol_type)
                != Some(&pending_response)
        })
    }

    fn commit_pending_session_configuration(
        at: &mut AtSession,
        uati: u32,
    ) -> Option<(u16, u16, u16)> {
        let (
            physical_layer_subtype,
            forward_traffic_mac_subtype,
            reverse_traffic_mac_subtype,
            route_update_subtype,
            default_route_update_tca_rev_a_tail,
            session_response,
            protocol_responses,
        ) = {
            let Some(pending) = at
                .pending_traffic_assignment
                .as_mut()
                .filter(|pending| pending.traffic_uati == uati)
            else {
                return None;
            };
            let physical_layer_subtype = session_config_selected_protocol_subtype(
                pending.session_config_trace.as_ref(),
                SESSION_PROTOCOL_PHYSICAL_LAYER,
            )
            .unwrap_or(SESSION_SUBTYPE_DEFAULT);
            let forward_traffic_mac_subtype = session_config_selected_protocol_subtype(
                pending.session_config_trace.as_ref(),
                SESSION_PROTOCOL_FORWARD_TRAFFIC_CHANNEL_MAC,
            )
            .unwrap_or(SESSION_SUBTYPE_DEFAULT);
            let reverse_traffic_mac_subtype = session_config_selected_protocol_subtype(
                pending.session_config_trace.as_ref(),
                SESSION_PROTOCOL_REVERSE_TRAFFIC_CHANNEL_MAC,
            )
            .unwrap_or(SESSION_SUBTYPE_DEFAULT);
            let explicit_route_update_subtype = session_config_selected_protocol_subtype(
                pending.session_config_trace.as_ref(),
                DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE,
            );
            let route_update_subtype =
                explicit_route_update_subtype.unwrap_or(SESSION_SUBTYPE_DEFAULT);
            let default_route_update_tca_rev_a_tail = route_update_subtype
                == SESSION_SUBTYPE_DEFAULT
                && Self::traffic_personality_uses_default_route_update_rev_a_tail(
                    physical_layer_subtype,
                    forward_traffic_mac_subtype,
                    reverse_traffic_mac_subtype,
                );
            pending.in_use_physical_layer_subtype = physical_layer_subtype;
            pending.in_use_forward_traffic_mac_subtype = forward_traffic_mac_subtype;
            pending.in_use_reverse_traffic_mac_subtype = reverse_traffic_mac_subtype;
            let session_response = pending
                .session_config_trace
                .as_ref()
                .map(|trace| trace.response_attrs.clone());
            let protocol_responses = pending
                .protocol_config_traces
                .iter()
                .map(|trace| {
                    (
                        trace.protocol_type,
                        canonical_protocol_configuration_response(
                            trace.protocol_type,
                            &trace.request_attrs,
                            &trace.response_attrs,
                        ),
                    )
                })
                .collect::<Vec<_>>();
            (
                physical_layer_subtype,
                forward_traffic_mac_subtype,
                reverse_traffic_mac_subtype,
                route_update_subtype,
                default_route_update_tca_rev_a_tail,
                session_response,
                protocol_responses,
            )
        };
        if let Some(session_response) = session_response {
            at.committed_session_configuration_response = Some(session_response);
        }
        for (protocol_type, response_attrs) in protocol_responses {
            at.committed_protocol_configuration_responses
                .insert(protocol_type, response_attrs);
        }
        log::info!(
            "HRPD AN: committed session configuration UATI=0x{uati:08x} physical_subtype=0x{physical_layer_subtype:04x} ftc_mac_subtype=0x{forward_traffic_mac_subtype:04x} rtc_mac_subtype=0x{reverse_traffic_mac_subtype:04x} route_update_subtype=0x{route_update_subtype:04x} tca_rev_a_tail_eligible={default_route_update_tca_rev_a_tail}"
        );
        Some((
            physical_layer_subtype,
            forward_traffic_mac_subtype,
            reverse_traffic_mac_subtype,
        ))
    }

    fn log_pending_config_trace(at: &AtSession, uati: u32, reason: &str) {
        let Some(pending) = at
            .pending_traffic_assignment
            .as_ref()
            .filter(|pending| pending.traffic_uati == uati)
        else {
            return;
        };
        if let Some(trace) = &pending.session_config_trace {
            log::info!(
                "HRPD AN: config trace UATI=0x{uati:08x} reason={reason} session_tx=0x{:02x} session_req={} session_rsp={}",
                trace.transaction_id,
                bytes_to_hex(&trace.request_attrs),
                bytes_to_hex(&trace.response_attrs)
            );
        } else {
            log::info!(
                "HRPD AN: config trace UATI=0x{uati:08x} reason={reason} no SessionConfigurationRequest recorded"
            );
        }
        if pending.protocol_config_traces.is_empty() {
            log::info!(
                "HRPD AN: config trace UATI=0x{uati:08x} reason={reason} no per-protocol ConfigurationRequest recorded"
            );
            return;
        }
        for trace in &pending.protocol_config_traces {
            log::info!(
                "HRPD AN: config trace UATI=0x{uati:08x} reason={reason} protocol=0x{:02x}/{} tx=0x{:02x} req={} rsp={}",
                trace.protocol_type,
                stream0_protocol_name(trace.protocol_type),
                trace.transaction_id,
                bytes_to_hex(&trace.request_attrs),
                bytes_to_hex(&trace.response_attrs)
            );
        }
    }

    fn accept_reverse_stream0_slp_d_payload(
        at: &mut AtSession,
        uati: u32,
        sequence_number: u8,
    ) -> bool {
        let Some(pending) = at
            .pending_traffic_assignment
            .as_mut()
            .filter(|pending| pending.traffic_uati == uati)
        else {
            return true;
        };
        let k = sequence_number & 0x07;
        let bit = 1u8 << k;
        if k == pending.reverse_stream0_slp_d_vn && pending.reverse_stream0_slp_d_rx & bit != 0 {
            pending.reverse_stream0_slp_d_rx = 0;
        }
        if pending.reverse_stream0_slp_d_rx & bit != 0 {
            return false;
        }

        let old_vn = pending.reverse_stream0_slp_d_vn;
        pending.reverse_stream0_slp_d_rx |= bit;
        let ahead = k.wrapping_sub(old_vn) & 0x07;
        if k == old_vn || ahead <= 3 {
            pending.reverse_stream0_slp_d_vn = k.wrapping_add(1) & 0x07;
            let new_vn = pending.reverse_stream0_slp_d_vn;
            if new_vn > old_vn {
                for future in new_vn..8 {
                    pending.reverse_stream0_slp_d_rx &= !(1u8 << future);
                }
            } else if new_vn < old_vn {
                for future in new_vn..old_vn {
                    pending.reverse_stream0_slp_d_rx &= !(1u8 << future);
                }
            }
        }
        true
    }

    fn parse_stream0_default_signaling_for_uati(
        &mut self,
        uati: u32,
        session_packet: &[u8],
    ) -> Stream0ParseOutcome {
        match decode_stream0_slp_f_packet(session_packet) {
            Some(Stream0SlpFPacket::Complete(slp_d_bits)) => parse_stream0_slp_d_bits(&slp_d_bits)
                .map(Stream0ParseOutcome::Complete)
                .unwrap_or(Stream0ParseOutcome::Invalid),
            Some(Stream0SlpFPacket::Fragment {
                begin,
                end,
                sequence,
                payload_bits,
            }) => {
                log::info!(
                    "HRPD AN: received Stream0 SLP-F fragment UATI=0x{uati:08x} begin={} end={} seq={} payload_bits={}",
                    begin,
                    end,
                    sequence,
                    payload_bits.len()
                );
                let rx = self.stream0_slp_f_rx.entry(uati).or_default();
                if let Some(last_sequence) = rx.last_sequence
                    && sequence != (last_sequence.wrapping_add(1) & 0x3f)
                {
                    log::info!(
                        "HRPD AN: Stream0 SLP-F sequence gap UATI=0x{uati:08x} last={} current={}; discarding reassembly",
                        last_sequence,
                        sequence
                    );
                    rx.buffer.clear();
                    rx.sync = false;
                }
                if begin {
                    rx.buffer.clear();
                    rx.sync = true;
                }
                if rx.sync {
                    rx.buffer.extend_from_slice(&payload_bits);
                    rx.last_sequence = Some(sequence);
                }
                if !end {
                    return Stream0ParseOutcome::InProgress;
                }
                if !rx.sync {
                    rx.buffer.clear();
                    return Stream0ParseOutcome::InProgress;
                }
                let slp_d_bits = std::mem::take(&mut rx.buffer);
                rx.sync = false;
                parse_stream0_slp_d_bits(&slp_d_bits)
                    .map(Stream0ParseOutcome::Complete)
                    .unwrap_or(Stream0ParseOutcome::Invalid)
            }
            None => Stream0ParseOutcome::Invalid,
        }
    }

    fn handle_traffic_channel_complete(
        &mut self,
        at: &mut AtSession,
        complete: &HrpdTrafficChannelComplete,
        slp_d_sequence_number: Option<u8>,
    ) -> (Vec<HrpdForwardTrafficPacket>, bool) {
        let Some(pending_view) = at.pending_traffic_assignment.as_ref() else {
            return (Vec::new(), false);
        };
        if pending_view.assignment.message_sequence != complete.message_sequence {
            return (Vec::new(), false);
        }
        let traffic_uati = pending_view.traffic_uati;
        let drc_index = self
            .last_drc_by_uati
            .get(&traffic_uati)
            .copied()
            .filter(|idx| implemented_forward_traffic_payload_bits_for_drc(*idx).is_some())
            .unwrap_or(HRPD_RTC_ACK_DRC_INDEX);

        let pending = at
            .pending_traffic_assignment
            .as_mut()
            .expect("pending assignment checked above");
        log::info!(
            "HRPD AN: TrafficChannelComplete seq={} accepted for session_uati=0x{:08x} traffic_uati=0x{:08x}",
            complete.message_sequence,
            pending.session_uati,
            pending.traffic_uati
        );
        // TrafficChannelComplete is the protocol-level confirmation that the
        // AT received RTCAck. It can race ahead of the physical HARQ callback.
        pending.rtc_ack_delivered = true;
        pending.active = true;
        at.last_completed_connection_request = Some(CompletedConnectionRequest {
            session_uati: pending.session_uati,
            traffic_uati: pending.traffic_uati,
            transaction_id: pending.connection_request_transaction_id,
        });
        let mut packets = Vec::new();
        if let Some(sequence_number) = slp_d_sequence_number
            && let Some(packet) = Self::build_slp_d_ack_packet(pending, sequence_number, drc_index)
        {
            packets.push(packet);
        }
        if !pending.stream0_slp_reset_acked {
            return (packets, true);
        }
        if pending.session_config_trace.is_none()
            && pending.protocol_config_traces.is_empty()
            && !pending.session_config_start_sent
        {
            if let Some(packet) = Self::build_session_configuration_start_packet(pending, drc_index)
            {
                pending.session_config_start_sent = true;
                packets.push(packet);
            }
        }
        (packets, true)
    }

    fn maybe_emit_current_session_configuration_for_active_traffic(
        &mut self,
        at: &mut AtSession,
        uati: u32,
        outcome: &mut HrpdTrafficOutcome,
    ) {
        if !at.session_configuration_complete {
            return;
        }
        let (physical_layer_subtype, forward_traffic_mac_subtype, reverse_traffic_mac_subtype) =
            Self::current_session_traffic_subtypes(at);
        let Some(pending_view) = at.pending_traffic_assignment.as_ref().filter(|pending| {
            pending.traffic_uati == uati && pending.active && pending.stream0_slp_reset_acked
        }) else {
            return;
        };
        let session_uati = pending_view.session_uati;
        let hardware_request_transaction_id =
            self.traffic_hardware_id_transaction_if_needed(at, session_uati, uati);
        let drc_index = self
            .last_drc_by_uati
            .get(&uati)
            .copied()
            .filter(|idx| implemented_forward_traffic_payload_bits_for_drc(*idx).is_some())
            .unwrap_or(HRPD_RTC_ACK_DRC_INDEX);
        let session_configuration_complete_event = Self::session_configuration_complete_event(
            at,
            uati,
            physical_layer_subtype,
            forward_traffic_mac_subtype,
        );

        let Some(pending) = at.pending_traffic_assignment.as_mut().filter(|pending| {
            pending.traffic_uati == uati && pending.active && pending.stream0_slp_reset_acked
        }) else {
            return;
        };
        if pending.session_config_trace.is_some() || !pending.protocol_config_traces.is_empty() {
            return;
        }
        if pending.session_config_complete_sent && pending.an_session_config_complete_acked {
            return;
        }
        pending.session_config_start_sent = true;
        pending.session_config_complete_sent = true;
        pending.an_session_config_complete_acked = true;
        pending.in_use_physical_layer_subtype = physical_layer_subtype;
        pending.in_use_forward_traffic_mac_subtype = forward_traffic_mac_subtype;
        pending.in_use_reverse_traffic_mac_subtype = reverse_traffic_mac_subtype;
        outcome.session_configuration_complete_uatis.push(uati);
        outcome
            .session_configuration_complete_events
            .push(session_configuration_complete_event);
        log::info!(
            "HRPD AN: current SessionConfiguration already in use for reopened traffic UATI=0x{uati:08x}; traffic configuration complete physical_subtype=0x{physical_layer_subtype:04x} ftc_mac_subtype=0x{forward_traffic_mac_subtype:04x} rtc_mac_subtype=0x{reverse_traffic_mac_subtype:04x}"
        );
        if let Some(transaction_id) = hardware_request_transaction_id {
            if let Some(packet) = Self::build_stream0_ftc_signaling_packet(
                pending,
                drc_index,
                DEFAULT_ADDRESS_MANAGEMENT_PROTOCOL_TYPE,
                &[0x03, transaction_id],
                None,
                false,
                "HardwareIDRequest",
            ) {
                log::info!(
                    "HRPD AN: retrying HardwareIDRequest on FTC UATI=0x{uati:08x} transaction=0x{transaction_id:02x}; A9/A11 setup is waiting for an IMSI-format MSID"
                );
                outcome.forward_traffic.push(packet);
            }
        }
    }

    fn traffic_hardware_id_transaction_if_needed(
        &mut self,
        at: &mut AtSession,
        session_uati: u32,
        traffic_uati: u32,
    ) -> Option<u8> {
        if self.hardware_ids_by_uati.contains_key(&session_uati)
            || self.hardware_ids_by_uati.contains_key(&traffic_uati)
        {
            return None;
        }
        match at.pending_hardware_id {
            Some((pending_uati, transaction_id))
                if pending_uati == session_uati || pending_uati == traffic_uati =>
            {
                Some(transaction_id)
            }
            _ => {
                let transaction_id = self.next_hardware_id_transaction();
                at.pending_hardware_id = Some((session_uati, transaction_id));
                Some(transaction_id)
            }
        }
    }

    fn queue_session_configuration_start_after_stream0_slp_ready(
        at: &mut AtSession,
        uati: u32,
        drc_index: u8,
    ) -> Vec<HrpdForwardTrafficPacket> {
        let Some(pending) = at.pending_traffic_assignment.as_mut().filter(|pending| {
            pending.traffic_uati == uati && pending.active && pending.stream0_slp_reset_acked
        }) else {
            return Vec::new();
        };

        let mut packets = Vec::new();
        if pending.session_config_trace.is_none()
            && pending.protocol_config_traces.is_empty()
            && !pending.session_config_start_sent
        {
            if let Some(packet) = Self::build_session_configuration_start_packet(pending, drc_index)
            {
                pending.session_config_start_sent = true;
                packets.push(packet);
            }
        }
        packets
    }

    fn build_slp_d_ack_packet(
        pending: &PendingTrafficAssignment,
        sequence_number: u8,
        drc_index: u8,
    ) -> Option<HrpdForwardTrafficPacket> {
        let mac_index = pending
            .assignment
            .pilots
            .first()
            .map(|pilot| pilot.mac_index)?;
        let uati = pending.traffic_uati;
        let physical_packet_bits = implemented_forward_traffic_payload_bits_for_drc(drc_index)
            .unwrap_or(HRPD_DEFAULT_FTC_PHYSICAL_BITS);
        match default_signaling_slp_d_ack_ftc_payload_bits_for_mac_subtype(
            physical_packet_bits,
            sequence_number,
            pending.in_use_forward_traffic_mac_subtype,
        ) {
            Ok(payload_bits) => {
                log::info!(
                    "HRPD AN: queueing Stream0 SLP-D ACK UATI=0x{uati:08x} MAC={mac_index} ack_seq={sequence_number} drc_index=0x{drc_index:x} physical_subtype=0x{:04x} ftc_mac_subtype=0x{:04x} payload_bits={physical_packet_bits}",
                    pending.in_use_physical_layer_subtype,
                    pending.in_use_forward_traffic_mac_subtype,
                );
                Some(HrpdForwardTrafficPacket {
                    uati,
                    mac_index,
                    physical_layer_subtype: pending.in_use_physical_layer_subtype,
                    forward_traffic_mac_subtype: pending.in_use_forward_traffic_mac_subtype,
                    payload_bits,
                })
            }
            Err(err) => {
                log::warn!(
                    "HRPD AN: failed to build Stream0 SLP-D ACK UATI=0x{uati:08x} MAC={mac_index} ack_seq={sequence_number}: {err:?}"
                );
                None
            }
        }
    }

    fn build_stream0_slp_reset_packet(
        pending: &PendingTrafficAssignment,
        drc_index: u8,
    ) -> Option<HrpdForwardTrafficPacket> {
        let mac_index = pending
            .assignment
            .pilots
            .first()
            .map(|pilot| pilot.mac_index)?;
        let uati = pending.traffic_uati;
        let message_sequence = pending.stream0_slp_reset_sequence;
        let physical_packet_bits = implemented_forward_traffic_payload_bits_for_drc(drc_index)
            .unwrap_or(HRPD_DEFAULT_FTC_PHYSICAL_BITS);
        match default_signaling_slp_reset_ftc_payload_bits_for_mac_subtype(
            physical_packet_bits,
            message_sequence,
            pending.in_use_forward_traffic_mac_subtype,
        ) {
            Ok(payload_bits) => {
                log::info!(
                    "HRPD AN: queueing Stream0 SLP Reset UATI=0x{uati:08x} MAC={mac_index} seq={message_sequence} drc_index=0x{drc_index:x} physical_subtype=0x{:04x} ftc_mac_subtype=0x{:04x} payload_bits={physical_packet_bits}",
                    pending.in_use_physical_layer_subtype,
                    pending.in_use_forward_traffic_mac_subtype,
                );
                Some(HrpdForwardTrafficPacket {
                    uati,
                    mac_index,
                    physical_layer_subtype: pending.in_use_physical_layer_subtype,
                    forward_traffic_mac_subtype: pending.in_use_forward_traffic_mac_subtype,
                    payload_bits,
                })
            }
            Err(err) => {
                log::warn!(
                    "HRPD AN: failed to build Stream0 SLP Reset UATI=0x{uati:08x} MAC={mac_index} seq={message_sequence}: {err:?}"
                );
                None
            }
        }
    }

    fn initialize_stream0_slp_after_route_update(
        &mut self,
        at: &mut AtSession,
        uati: u32,
    ) -> Vec<HrpdForwardTrafficPacket> {
        let drc_index = self.current_forward_drc_index_for_uati(uati);
        let slp_d_vs_stream0_fl = at.slp_d_vs_stream0_fl;
        if let Some(pending) = at
            .pending_traffic_assignment
            .as_mut()
            .filter(|pending| pending.traffic_uati == uati)
            && !pending.stream0_slp_reset_acked
        {
            pending.stream0_slp_reset_pending = false;
            pending.stream0_slp_reset_acked = true;
            pending.stream0_slp_reset_attempts = 0;
            pending.stream0_slp_reset_last_send_slot = None;
            log::info!(
                "HRPD AN: Stream0 SLP ready from RouteUpdate.ConnectionInitiated UATI=0x{uati:08x}; SNP delivery enabled vs={} pending_reliable={}",
                slp_d_vs_stream0_fl & 0x07,
                pending.reliable_stream0_tx.len()
            );
        }
        Self::queue_session_configuration_start_after_stream0_slp_ready(at, uati, drc_index)
    }

    fn build_slp_d_ack_packet_for_uati(
        &self,
        at: &AtSession,
        uati: u32,
        sequence_number: u8,
    ) -> Option<HrpdForwardTrafficPacket> {
        let pending = at
            .pending_traffic_assignment
            .as_ref()
            .filter(|pending| pending.traffic_uati == uati)?;
        let drc_index = self
            .last_drc_by_uati
            .get(&uati)
            .copied()
            .filter(|idx| implemented_forward_traffic_payload_bits_for_drc(*idx).is_some())
            .unwrap_or(HRPD_RTC_ACK_DRC_INDEX);
        Self::build_slp_d_ack_packet(pending, sequence_number, drc_index)
    }

    fn build_session_configuration_start_packet(
        pending: &mut PendingTrafficAssignment,
        drc_index: u8,
    ) -> Option<HrpdForwardTrafficPacket> {
        let payload = [SESSION_CONFIGURATION_START];
        Self::build_stream0_ftc_signaling_packet(
            pending,
            drc_index,
            DEFAULT_SESSION_CONFIGURATION_PROTOCOL_TYPE,
            &payload,
            None,
            false,
            "SessionConfigurationStart",
        )
    }

    fn build_dh_key_request_packet(
        pending: &mut PendingTrafficAssignment,
        drc_index: u8,
        sequence_number: u8,
        ack_sequence_number: Option<u8>,
        key_exchange: &DhKeyExchangeState,
    ) -> Option<HrpdForwardTrafficPacket> {
        let mut payload = Vec::with_capacity(3 + key_exchange.an_public.len());
        payload.push(DH_KEY_REQUEST);
        payload.push(key_exchange.transaction_id);
        payload.push(DH_KEY_EXCHANGE_TIMEOUT_SECONDS);
        payload.extend_from_slice(&key_exchange.an_public);
        let packet = Self::build_stream0_ftc_signaling_packet_with_ack(
            pending,
            drc_index,
            SESSION_PROTOCOL_KEY_EXCHANGE,
            &payload,
            Some(sequence_number),
            true,
            ack_sequence_number,
            "KeyRequest",
        )?;
        Self::remember_reliable_stream0_packet(
            pending,
            sequence_number,
            SESSION_PROTOCOL_KEY_EXCHANGE,
            &payload,
            true,
            ack_sequence_number,
            "KeyRequest",
        );
        Some(packet)
    }

    fn build_dh_an_key_complete_packet(
        pending: &mut PendingTrafficAssignment,
        drc_index: u8,
        sequence_number: u8,
        ack_sequence_number: Option<u8>,
        transaction_id: u8,
        nonce: u16,
        timestamp_long: u64,
        key_signature: &[u8],
    ) -> Option<HrpdForwardTrafficPacket> {
        let mut payload = Vec::with_capacity(26);
        payload.push(DH_AN_KEY_COMPLETE);
        payload.push(transaction_id);
        payload.extend_from_slice(&nonce.to_be_bytes());
        payload.extend_from_slice(&(timestamp_long as u16).to_be_bytes());
        payload.extend_from_slice(key_signature);
        let packet = Self::build_stream0_ftc_signaling_packet_with_ack(
            pending,
            drc_index,
            SESSION_PROTOCOL_KEY_EXCHANGE,
            &payload,
            Some(sequence_number),
            true,
            ack_sequence_number,
            "ANKeyComplete",
        )?;
        Self::remember_reliable_stream0_packet(
            pending,
            sequence_number,
            SESSION_PROTOCOL_KEY_EXCHANGE,
            &payload,
            true,
            ack_sequence_number,
            "ANKeyComplete",
        );
        Some(packet)
    }

    fn build_session_configuration_complete_packet(
        pending: &mut PendingTrafficAssignment,
        drc_index: u8,
        transaction_id: u8,
        sequence_number: u8,
        ack_sequence_number: Option<u8>,
        commit_required: bool,
    ) -> Option<HrpdForwardTrafficPacket> {
        let (payload, label) = session_configuration_complete_payload(
            transaction_id,
            pending.session_personality_count,
            commit_required,
        );
        let packet = Self::build_stream0_ftc_signaling_packet_with_ack(
            pending,
            drc_index,
            DEFAULT_SESSION_CONFIGURATION_PROTOCOL_TYPE,
            &payload,
            Some(sequence_number),
            false,
            ack_sequence_number,
            label,
        )?;
        Self::remember_reliable_stream0_packet(
            pending,
            sequence_number,
            DEFAULT_SESSION_CONFIGURATION_PROTOCOL_TYPE,
            &payload,
            false,
            ack_sequence_number,
            label,
        );
        Some(packet)
    }

    fn build_session_configuration_response_packet(
        pending: &mut PendingTrafficAssignment,
        drc_index: u8,
        transaction_id: u8,
        attributes: &[u8],
        sequence_number: u8,
        ack_sequence_number: Option<u8>,
    ) -> Option<HrpdForwardTrafficPacket> {
        let packet = Self::build_session_configuration_response_packet_without_remembering(
            pending,
            drc_index,
            transaction_id,
            attributes,
            sequence_number,
            ack_sequence_number,
        )?;
        let mut payload = Vec::with_capacity(2 + attributes.len());
        payload.push(SESSION_CONFIGURATION_RESPONSE);
        payload.push(transaction_id);
        payload.extend_from_slice(attributes);
        Self::remember_reliable_stream0_packet(
            pending,
            sequence_number,
            DEFAULT_SESSION_CONFIGURATION_PROTOCOL_TYPE,
            &payload,
            false,
            ack_sequence_number,
            "SessionConfigurationResponse",
        );
        Some(packet)
    }

    fn build_session_configuration_response_packet_without_remembering(
        pending: &PendingTrafficAssignment,
        drc_index: u8,
        transaction_id: u8,
        attributes: &[u8],
        sequence_number: u8,
        ack_sequence_number: Option<u8>,
    ) -> Option<HrpdForwardTrafficPacket> {
        let mut payload = Vec::with_capacity(2 + attributes.len());
        payload.push(SESSION_CONFIGURATION_RESPONSE);
        payload.push(transaction_id);
        payload.extend_from_slice(attributes);
        Self::build_stream0_ftc_signaling_packet_with_ack(
            pending,
            drc_index,
            DEFAULT_SESSION_CONFIGURATION_PROTOCOL_TYPE,
            &payload,
            Some(sequence_number),
            false,
            ack_sequence_number,
            "SessionConfigurationResponse",
        )
    }

    fn build_protocol_configuration_response_packet(
        pending: &mut PendingTrafficAssignment,
        drc_index: u8,
        protocol_type: u8,
        transaction_id: u8,
        attributes: &[u8],
        sequence_number: u8,
        ack_sequence_number: Option<u8>,
    ) -> Option<HrpdForwardTrafficPacket> {
        let packet = Self::build_protocol_configuration_response_packet_without_remembering(
            pending,
            drc_index,
            protocol_type,
            transaction_id,
            attributes,
            sequence_number,
            ack_sequence_number,
        )?;
        let mut payload = Vec::with_capacity(2 + attributes.len());
        payload.push(SESSION_CONFIGURATION_RESPONSE);
        payload.push(transaction_id);
        payload.extend_from_slice(attributes);
        Self::remember_reliable_stream0_packet(
            pending,
            sequence_number,
            protocol_type,
            &payload,
            true,
            ack_sequence_number,
            stream0_protocol_name(protocol_type),
        );
        Some(packet)
    }

    fn build_protocol_configuration_response_packet_without_remembering(
        pending: &PendingTrafficAssignment,
        drc_index: u8,
        protocol_type: u8,
        transaction_id: u8,
        attributes: &[u8],
        sequence_number: u8,
        ack_sequence_number: Option<u8>,
    ) -> Option<HrpdForwardTrafficPacket> {
        let mut payload = Vec::with_capacity(2 + attributes.len());
        payload.push(SESSION_CONFIGURATION_RESPONSE);
        payload.push(transaction_id);
        payload.extend_from_slice(attributes);
        let label = stream0_protocol_name(protocol_type);
        Self::build_stream0_ftc_signaling_packet_with_ack(
            pending,
            drc_index,
            protocol_type,
            &payload,
            Some(sequence_number),
            true,
            ack_sequence_number,
            label,
        )
    }

    fn build_stream0_ftc_signaling_packet(
        pending: &PendingTrafficAssignment,
        drc_index: u8,
        protocol_type: u8,
        payload: &[u8],
        reliable_sequence_number: Option<u8>,
        in_configuration: bool,
        label: &str,
    ) -> Option<HrpdForwardTrafficPacket> {
        Self::build_stream0_ftc_signaling_packet_with_ack(
            pending,
            drc_index,
            protocol_type,
            payload,
            reliable_sequence_number,
            in_configuration,
            None,
            label,
        )
    }

    fn build_stream0_ftc_signaling_packet_with_ack(
        pending: &PendingTrafficAssignment,
        drc_index: u8,
        protocol_type: u8,
        payload: &[u8],
        reliable_sequence_number: Option<u8>,
        in_configuration: bool,
        ack_sequence_number: Option<u8>,
        label: &str,
    ) -> Option<HrpdForwardTrafficPacket> {
        let mac_index = pending
            .assignment
            .pilots
            .first()
            .map(|pilot| pilot.mac_index)?;
        let uati = pending.traffic_uati;
        Self::build_stream0_ftc_signaling_packet_for_mac(
            uati,
            mac_index,
            pending.in_use_physical_layer_subtype,
            pending.in_use_forward_traffic_mac_subtype,
            drc_index,
            protocol_type,
            payload,
            reliable_sequence_number,
            in_configuration,
            ack_sequence_number,
            label,
        )
    }

    fn build_stream0_ftc_signaling_packet_for_mac(
        uati: u32,
        mac_index: u8,
        physical_layer_subtype: u16,
        forward_traffic_mac_subtype: u16,
        drc_index: u8,
        protocol_type: u8,
        payload: &[u8],
        reliable_sequence_number: Option<u8>,
        in_configuration: bool,
        ack_sequence_number: Option<u8>,
        label: &str,
    ) -> Option<HrpdForwardTrafficPacket> {
        let physical_packet_bits = implemented_forward_traffic_payload_bits_for_drc(drc_index)
            .unwrap_or(HRPD_DEFAULT_FTC_PHYSICAL_BITS);
        match default_signaling_ftc_payload_bits_with_ack_for_mac_subtype(
            physical_packet_bits,
            protocol_type,
            payload,
            reliable_sequence_number,
            in_configuration,
            ack_sequence_number,
            forward_traffic_mac_subtype,
        ) {
            Ok(payload_bits) => {
                log::info!(
                    "HRPD AN: queueing Stream0 {label} UATI=0x{uati:08x} MAC={mac_index} protocol=0x{protocol_type:02x} in_config={in_configuration} reliable_seq={reliable_sequence_number:?} ack_seq={ack_sequence_number:?} drc_index=0x{drc_index:x} physical_subtype=0x{physical_layer_subtype:04x} ftc_mac_subtype=0x{forward_traffic_mac_subtype:04x} payload_bits={physical_packet_bits}"
                );
                Some(HrpdForwardTrafficPacket {
                    uati,
                    mac_index,
                    physical_layer_subtype,
                    forward_traffic_mac_subtype,
                    payload_bits,
                })
            }
            Err(err) => {
                log::warn!(
                    "HRPD AN: failed to build Stream0 {label} UATI=0x{uati:08x} MAC={mac_index}: {err:?}"
                );
                None
            }
        }
    }

    fn remember_reliable_stream0_packet(
        pending: &mut PendingTrafficAssignment,
        sequence_number: u8,
        protocol_type: u8,
        payload: &[u8],
        in_configuration: bool,
        ack_sequence_number: Option<u8>,
        label: &'static str,
    ) {
        if pending.reliable_stream0_tx.len() >= 7 {
            let dropped = pending.reliable_stream0_tx.remove(0);
            log::warn!(
                "HRPD AN: Stream0 reliable retransmit buffer full UATI=0x{:08x}; dropping oldest seq={} label={}",
                pending.traffic_uati,
                dropped.sequence_number,
                dropped.label
            );
        }
        pending
            .reliable_stream0_tx
            .push(PendingReliableStream0Packet {
                sequence_number: sequence_number & 0x07,
                protocol_type,
                payload: payload.to_vec(),
                in_configuration,
                ack_sequence_number: ack_sequence_number.map(|seq| seq & 0x07),
                label,
                attempts: 1,
                last_send_at: Instant::now(),
            });
    }

    fn handle_stream0_protocol_configuration_message(
        &mut self,
        at: &mut AtSession,
        uati: u32,
        protocol_type: u8,
        payload: &[u8],
        ack_sequence_number: Option<u8>,
        outcome: &mut HrpdTrafficOutcome,
    ) -> Option<HrpdForwardTrafficPacket> {
        let message_id = *payload.first()?;
        if message_id != SESSION_CONFIGURATION_REQUEST {
            return None;
        }
        let transaction_id = *payload.get(1)?;
        // In-configuration attribute record formats follow the subtype the
        // in-flight SessionConfiguration selected for the protocol.
        let pending_rtc_mac_subtype = at
            .pending_traffic_assignment
            .as_ref()
            .filter(|pending| pending.traffic_uati == uati)
            .and_then(|pending| {
                session_config_selected_protocol_subtype(
                    pending.session_config_trace.as_ref(),
                    SESSION_PROTOCOL_REVERSE_TRAFFIC_CHANNEL_MAC,
                )
            })
            .unwrap_or(SESSION_SUBTYPE_DEFAULT);
        let (attributes, default_packet_selection) = if protocol_type == SESSION_PROTOCOL_STREAM {
            let response = stream_configuration_response(&payload[2..]);
            (response.attributes, response.default_packet)
        } else {
            (
                configuration_response_attributes(
                    protocol_type,
                    &payload[2..],
                    pending_rtc_mac_subtype,
                ),
                None,
            )
        };
        log::info!(
            "HRPD AN: {} ConfigurationRequest received UATI=0x{uati:08x} protocol=0x{protocol_type:02x} transaction=0x{transaction_id:02x} request_attr_octets={} request_attrs={} response_attr_octets={} response_attrs={}",
            stream0_protocol_name(protocol_type),
            payload.len().saturating_sub(2),
            bytes_to_hex(&payload[2..]),
            attributes.len(),
            bytes_to_hex(&attributes)
        );
        if let Some(selection) = default_packet_selection {
            self.default_packet_stream_id = selection.stream_id;
            log::info!(
                "HRPD AN: negotiated DefaultPacket stream UATI=0x{uati:08x} stream={} protocol=0x{:02x} app_subtype=0x{:04x} value_id=0x{:02x}",
                selection.stream_id,
                selection.protocol_type,
                selection.application_subtype,
                selection.value_id
            );
            outcome.default_packet_stream_configurations.push(
                HrpdDefaultPacketStreamConfiguration {
                    uati,
                    stream_id: selection.stream_id,
                    protocol_type: selection.protocol_type,
                    application_subtype: selection.application_subtype,
                },
            );
        }
        let drc_index = self
            .last_drc_by_uati
            .get(&uati)
            .copied()
            .filter(|idx| implemented_forward_traffic_payload_bits_for_drc(*idx).is_some())
            .unwrap_or(HRPD_RTC_ACK_DRC_INDEX);
        let packet = {
            let pending = at
                .pending_traffic_assignment
                .as_mut()
                .filter(|pending| pending.traffic_uati == uati)?;
            if let Some(trace) = pending.protocol_config_traces.iter().find(|trace| {
                trace.protocol_type == protocol_type
                    && trace.transaction_id == transaction_id
                    && trace.request_attrs == payload[2..]
                    && trace.response_attrs == attributes
                    && trace.sequence_number.is_some()
            }) {
                let sequence_number = trace.sequence_number.unwrap_or_default();
                log::info!(
                    "HRPD AN: duplicate {} ConfigurationRequest UATI=0x{uati:08x} protocol=0x{protocol_type:02x} transaction=0x{transaction_id:02x}; resending reliable_seq={}",
                    stream0_protocol_name(protocol_type),
                    sequence_number
                );
                return Self::build_protocol_configuration_response_packet_without_remembering(
                    pending,
                    drc_index,
                    protocol_type,
                    transaction_id,
                    &attributes,
                    sequence_number,
                    ack_sequence_number,
                );
            }
            let sequence_number = at.slp_d_vs_stream0_fl & 0x07;
            let packet = Self::build_protocol_configuration_response_packet(
                pending,
                drc_index,
                protocol_type,
                transaction_id,
                &attributes,
                sequence_number,
                ack_sequence_number,
            )?;
            pending.protocol_config_traces.push(ProtocolConfigTrace {
                protocol_type,
                transaction_id,
                request_attrs: payload[2..].to_vec(),
                response_attrs: attributes.clone(),
                sequence_number: Some(sequence_number),
            });
            if pending.protocol_config_traces.len() > 16 {
                pending.protocol_config_traces.remove(0);
            }
            packet
        };
        at.slp_d_vs_stream0_fl = at.slp_d_vs_stream0_fl.wrapping_add(1) & 0x07;
        Some(packet)
    }

    /// Generic Attribute Update Protocol responder (C.S0024-A §14.10). The
    /// AN must answer every AttributeUpdateRequest within TTurnaround = 2 s;
    /// rejecting keeps the previously negotiated values in force, which is
    /// always safe. Per-attribute accept policies can hook in here once an
    /// updatable attribute is actually honored end-to-end.
    fn handle_stream0_attribute_update_message(
        &mut self,
        at: &mut AtSession,
        uati: u32,
        protocol_type: u8,
        payload: &[u8],
        ack_sequence_number: Option<u8>,
    ) -> Option<HrpdForwardTrafficPacket> {
        let message_id = *payload.first()?;
        if message_id != ATTRIBUTE_UPDATE_REQUEST {
            return None;
        }
        let transaction_id = *payload.get(1)?;
        log::info!(
            "HRPD AN: AttributeUpdateRequest received UATI=0x{uati:08x} protocol=0x{protocol_type:02x} transaction=0x{transaction_id:02x} attr_octets={} attrs={}; rejecting (no GAUP-updatable attributes)",
            payload.len().saturating_sub(2),
            bytes_to_hex(&payload[2..])
        );
        let drc_index = self.current_forward_drc_index_for_uati(uati);
        // The Reject needs nothing from session state beyond the AT's traffic
        // channel — the transaction comes from the request itself — so answer
        // on the AT's assignment even when the event UATI is the session
        // receive UATI rather than the assignment's traffic UATI.
        let Some(pending) = at.pending_traffic_assignment.as_mut() else {
            log::warn!(
                "HRPD AN: no traffic assignment to carry AttributeUpdateReject UATI=0x{uati:08x} transaction=0x{transaction_id:02x}"
            );
            return None;
        };
        // C.S0024-A specifies SLP Reliable delivery for AttributeUpdateReject.
        let sequence_number = at.slp_d_vs_stream0_fl & 0x07;
        let reject_payload = [ATTRIBUTE_UPDATE_REJECT, transaction_id];
        let packet = Self::build_stream0_ftc_signaling_packet_with_ack(
            pending,
            drc_index,
            protocol_type,
            &reject_payload,
            Some(sequence_number),
            false,
            ack_sequence_number,
            "AttributeUpdateReject",
        )?;
        Self::remember_reliable_stream0_packet(
            pending,
            sequence_number,
            protocol_type,
            &reject_payload,
            false,
            ack_sequence_number,
            "AttributeUpdateReject",
        );
        at.slp_d_vs_stream0_fl = at.slp_d_vs_stream0_fl.wrapping_add(1) & 0x07;
        Some(packet)
    }

    fn handle_stream0_key_exchange_message(
        &mut self,
        at: &mut AtSession,
        uati: u32,
        payload: &[u8],
        ack_sequence_number: Option<u8>,
    ) -> (Vec<HrpdForwardTrafficPacket>, bool) {
        let Some(&message_id) = payload.first() else {
            return (Vec::new(), false);
        };
        let Some(&transaction_id) = payload.get(1) else {
            return (Vec::new(), false);
        };
        let drc_index = self.current_forward_drc_index_for_uati(uati);
        match message_id {
            DH_KEY_RESPONSE => {
                let Some(&timeout) = payload.get(2) else {
                    return (Vec::new(), false);
                };
                let Some(at_public) = payload.get(3..3 + DH_KEY_LENGTH_OCTETS_768) else {
                    return (Vec::new(), false);
                };
                let sequence_number = at.slp_d_vs_stream0_fl & 0x07;
                let packet = {
                    let Some(pending) = at
                        .pending_traffic_assignment
                        .as_mut()
                        .filter(|pending| pending.traffic_uati == uati && pending.active)
                    else {
                        return (Vec::new(), false);
                    };
                    if pending.dh_key_exchange_complete {
                        log::info!(
                            "HRPD AN: ignoring duplicate DH KeyResponse UATI=0x{uati:08x} transaction=0x{transaction_id:02x}; key exchange already complete"
                        );
                        return (Vec::new(), false);
                    }
                    let Some(key_exchange) = pending.dh_key_exchange.as_ref() else {
                        log::warn!(
                            "HRPD AN: received DH KeyResponse UATI=0x{uati:08x} transaction=0x{transaction_id:02x} without pending KeyRequest"
                        );
                        return (Vec::new(), false);
                    };
                    if key_exchange.transaction_id != transaction_id {
                        log::warn!(
                            "HRPD AN: ignoring DH KeyResponse UATI=0x{uati:08x} transaction=0x{transaction_id:02x}; expected=0x{:02x}",
                            key_exchange.transaction_id
                        );
                        return (Vec::new(), false);
                    }
                    if key_exchange.session_key.is_some() {
                        let pending_an_key_complete = pending
                            .reliable_stream0_tx
                            .iter()
                            .find(|packet| {
                                packet.label == "ANKeyComplete"
                                    && packet.protocol_type == SESSION_PROTOCOL_KEY_EXCHANGE
                                    && packet.payload.get(1) == Some(&transaction_id)
                            })
                            .cloned();
                        if let Some(packet) = pending_an_key_complete {
                            let forward_packet = Self::build_stream0_ftc_signaling_packet_with_ack(
                                pending,
                                drc_index,
                                packet.protocol_type,
                                &packet.payload,
                                Some(packet.sequence_number),
                                packet.in_configuration,
                                ack_sequence_number,
                                packet.label,
                            );
                            if forward_packet.is_some() {
                                log::info!(
                                    "HRPD AN: duplicate DH KeyResponse UATI=0x{uati:08x} transaction=0x{transaction_id:02x}; retransmitting existing ANKeyComplete seq={}",
                                    packet.sequence_number
                                );
                            }
                            return (forward_packet.into_iter().collect(), false);
                        }
                        log::info!(
                            "HRPD AN: ignoring duplicate DH KeyResponse UATI=0x{uati:08x} transaction=0x{transaction_id:02x}; ANKeyComplete already sent"
                        );
                        return (Vec::new(), false);
                    }
                    let session_key = dh_compute_session_key(&key_exchange.an_private, at_public);
                    let nonce = random_u16();
                    let timestamp_long = cdma_system_time_80ms_now();
                    let key_signature =
                        dh_key_signature(&session_key, transaction_id, nonce, timestamp_long);
                    let packet = Self::build_dh_an_key_complete_packet(
                        pending,
                        drc_index,
                        sequence_number,
                        ack_sequence_number,
                        transaction_id,
                        nonce,
                        timestamp_long,
                        &key_signature,
                    );
                    let Some(packet) = packet else {
                        return (Vec::new(), false);
                    };
                    if let Some(key_exchange) = pending.dh_key_exchange.as_mut() {
                        key_exchange.session_key = Some(session_key);
                        key_exchange.nonce = Some(nonce);
                        key_exchange.timestamp_long = Some(timestamp_long);
                    }
                    log::info!(
                        "HRPD AN: decoded reverse Stream-0 DH KeyResponse UATI=0x{uati:08x} transaction=0x{transaction_id:02x} timeout={timeout}s; queueing ANKeyComplete nonce=0x{nonce:04x} timestamp_short=0x{:04x}",
                        timestamp_long as u16
                    );
                    packet
                };
                at.slp_d_vs_stream0_fl = at.slp_d_vs_stream0_fl.wrapping_add(1) & 0x07;
                (vec![packet], false)
            }
            DH_AT_KEY_COMPLETE => {
                let result = payload.get(2).is_some_and(|byte| byte & 0x80 != 0);
                let mut packets = Vec::new();
                let mut queue_soft_commit_close = false;
                let soft_commit_required = at
                    .pending_traffic_assignment
                    .as_ref()
                    .filter(|pending| pending.traffic_uati == uati)
                    .is_some_and(|pending| {
                        pending.session_personality_count > SESSION_PERSONALITY_COUNT_DEFAULT
                            && Self::pending_configuration_changes_in_use(at, pending)
                    });
                if let Some(pending) = at
                    .pending_traffic_assignment
                    .as_mut()
                    .filter(|pending| pending.traffic_uati == uati && pending.active)
                {
                    if pending
                        .dh_key_exchange
                        .as_ref()
                        .is_some_and(|state| state.transaction_id == transaction_id)
                        && result
                    {
                        pending.dh_key_exchange_complete = true;
                        if !pending.session_config_complete_sent {
                            if let Some(config_transaction_id) =
                                pending.at_session_config_complete_transaction_id
                            {
                                let sequence_number = at.slp_d_vs_stream0_fl & 0x07;
                                if let Some(packet) =
                                    Self::build_session_configuration_complete_packet(
                                        pending,
                                        drc_index,
                                        config_transaction_id,
                                        sequence_number,
                                        ack_sequence_number,
                                        soft_commit_required,
                                    )
                                {
                                    pending.session_config_complete_sent = true;
                                    at.slp_d_vs_stream0_fl =
                                        at.slp_d_vs_stream0_fl.wrapping_add(1) & 0x07;
                                    queue_soft_commit_close = soft_commit_required;
                                    log::info!(
                                        "HRPD AN: negotiated DH KeyExchange complete UATI=0x{uati:08x}; queueing AN SessionConfigurationComplete transaction=0x{config_transaction_id:02x} token=0x0000 soft_commit_required={soft_commit_required}"
                                    );
                                    packets.push(packet);
                                }
                            } else {
                                log::warn!(
                                    "HRPD AN: DH ATKeyComplete succeeded UATI=0x{uati:08x} before AT SessionConfigurationComplete; waiting for canonical GCP completion"
                                );
                            }
                        }
                    }
                }
                if result {
                    log::info!(
                        "HRPD AN: decoded reverse Stream-0 DH ATKeyComplete UATI=0x{uati:08x} transaction=0x{transaction_id:02x} result=success"
                    );
                } else {
                    log::warn!(
                        "HRPD AN: decoded reverse Stream-0 DH ATKeyComplete UATI=0x{uati:08x} transaction=0x{transaction_id:02x} result=failure"
                    );
                }
                (packets, queue_soft_commit_close)
            }
            DH_KEY_REQUEST | DH_AN_KEY_COMPLETE => {
                log::info!(
                    "HRPD AN: received unexpected reverse Stream-0 DH message UATI=0x{uati:08x} msg_id=0x{message_id:02x} transaction=0x{transaction_id:02x}"
                );
                (Vec::new(), false)
            }
            _ => (Vec::new(), false),
        }
    }

    fn handle_stream0_default_packet_message(
        &mut self,
        at: &mut AtSession,
        uati: u32,
        protocol_type: u8,
        message_id: Option<u8>,
        payload: &[u8],
        outcome: &mut HrpdTrafficOutcome,
    ) -> Option<HrpdForwardTrafficPacket> {
        if protocol_type != self.default_packet_protocol_type() {
            return None;
        }
        match message_id {
            Some(DEFAULT_PACKET_XON_REQUEST) => {
                log::info!(
                    "HRPD AN: decoded reverse Stream-0 DefaultPacket XonRequest UATI=0x{uati:08x}"
                );
                outcome.default_packet_flow_open_uatis.push(uati);
                self.build_default_packet_flow_control_packet_for_uati(
                    at,
                    uati,
                    &[DEFAULT_PACKET_XON_RESPONSE],
                    "XonResponse",
                )
            }
            Some(DEFAULT_PACKET_XOFF_REQUEST) => {
                log::info!(
                    "HRPD AN: decoded reverse Stream-0 DefaultPacket XoffRequest UATI=0x{uati:08x}"
                );
                outcome.default_packet_flow_closed_uatis.push(uati);
                self.build_default_packet_flow_control_packet_for_uati(
                    at,
                    uati,
                    &[DEFAULT_PACKET_XOFF_RESPONSE],
                    "XoffResponse",
                )
            }
            Some(DEFAULT_PACKET_DATA_READY_ACK) => {
                let transaction_id = payload.get(1).copied().unwrap_or_default();
                log::info!(
                    "HRPD AN: decoded reverse Stream-0 DefaultPacket DataReadyAck UATI=0x{uati:08x} transaction=0x{transaction_id:02x}"
                );
                outcome
                    .default_packet_data_ready_acks
                    .push(HrpdDefaultPacketDataReadyAckEvent {
                        uati,
                        transaction_id,
                    });
                None
            }
            Some(DEFAULT_PACKET_RLP_RESET) => {
                log::info!(
                    "HRPD AN: decoded reverse Stream-0 DefaultPacket RLP Reset UATI=0x{uati:08x}"
                );
                at.reverse_default_packet_rlp.reset();
                outcome.default_packet_rlp_reset_uatis.push(uati);
                self.build_default_packet_flow_control_packet_for_uati(
                    at,
                    uati,
                    &[DEFAULT_PACKET_RLP_RESET_ACK],
                    "RlpResetAck",
                )
            }
            Some(DEFAULT_PACKET_RLP_RESET_ACK) => {
                log::info!(
                    "HRPD AN: decoded reverse Stream-0 DefaultPacket RLP ResetAck UATI=0x{uati:08x}"
                );
                None
            }
            Some(DEFAULT_PACKET_RLP_NAK) => {
                if let Some(nak) = parse_default_packet_rlp_nak(payload) {
                    let requests = nak
                        .requests
                        .iter()
                        .map(|request| format!("{}+{}", request.first_erased, request.window_len))
                        .collect::<Vec<_>>()
                        .join(",");
                    log::debug!(
                        "HRPD AN: decoded reverse Stream-0 DefaultPacket RLP Nak UATI=0x{uati:08x} requests={} ranges=[{}]",
                        nak.requests.len(),
                        requests
                    );
                    outcome
                        .default_packet_rlp_naks
                        .push(HrpdDefaultPacketRlpNakEvent {
                            uati,
                            requests: nak.requests,
                        });
                } else {
                    log::warn!(
                        "HRPD AN: malformed reverse Stream-0 DefaultPacket RLP Nak UATI=0x{uati:08x} payload_len={} payload_hex={}",
                        payload.len(),
                        bytes_to_hex(payload)
                    );
                }
                None
            }
            _ => {
                log::info!(
                    "HRPD AN: decoded reverse Stream-0 {} UATI=0x{uati:08x} protocol=0x{protocol_type:02x} msg_id={message_id:?} payload_len={} payload_hex={}",
                    stream0_message_name(protocol_type, message_id),
                    payload.len(),
                    bytes_to_hex(payload)
                );
                None
            }
        }
    }

    fn build_default_packet_flow_control_packet_for_uati(
        &self,
        at: &AtSession,
        uati: u32,
        payload: &[u8],
        label: &str,
    ) -> Option<HrpdForwardTrafficPacket> {
        let pending = at
            .pending_traffic_assignment
            .as_ref()
            .filter(|pending| pending.traffic_uati == uati && pending.active)?;
        let drc_index = self
            .last_drc_by_uati
            .get(&uati)
            .copied()
            .filter(|idx| implemented_forward_traffic_payload_bits_for_drc(*idx).is_some())
            .unwrap_or(HRPD_RTC_ACK_DRC_INDEX);
        let mac_index = pending
            .assignment
            .pilots
            .first()
            .map(|pilot| pilot.mac_index)?;
        let physical_packet_bits = implemented_forward_traffic_payload_bits_for_drc(drc_index)
            .unwrap_or(HRPD_DEFAULT_FTC_PHYSICAL_BITS);
        match default_signaling_ftc_payload_bits_with_ack_for_mac_subtype(
            physical_packet_bits,
            self.default_packet_protocol_type(),
            payload,
            None,
            false,
            None,
            pending.in_use_forward_traffic_mac_subtype,
        ) {
            Ok(payload_bits) => {
                log::info!(
                    "HRPD AN: queueing FTC DefaultPacket {label} UATI=0x{uati:08x} MAC={mac_index} drc_index=0x{drc_index:x} physical_subtype=0x{:04x} ftc_mac_subtype=0x{:04x} payload_bits={physical_packet_bits}",
                    pending.in_use_physical_layer_subtype,
                    pending.in_use_forward_traffic_mac_subtype,
                );
                Some(HrpdForwardTrafficPacket {
                    uati,
                    mac_index,
                    physical_layer_subtype: pending.in_use_physical_layer_subtype,
                    forward_traffic_mac_subtype: pending.in_use_forward_traffic_mac_subtype,
                    payload_bits,
                })
            }
            Err(err) => {
                log::warn!(
                    "HRPD AN: failed to build FTC DefaultPacket {label} UATI=0x{uati:08x} MAC={mac_index}: {err:?}"
                );
                None
            }
        }
    }

    fn handle_session_configuration_message(
        &mut self,
        at: &mut AtSession,
        uati: u32,
        payload: &[u8],
        ack_sequence_number: Option<u8>,
    ) -> Option<(HrpdForwardTrafficPacket, bool, bool)> {
        let message_id = *payload.first()?;
        let transaction_id = *payload.get(1)?;
        let drc_index = self
            .last_drc_by_uati
            .get(&uati)
            .copied()
            .filter(|idx| implemented_forward_traffic_payload_bits_for_drc(*idx).is_some())
            .unwrap_or(HRPD_RTC_ACK_DRC_INDEX);
        match message_id {
            SESSION_CONFIGURATION_COMPLETE => {
                let sequence_number = at.slp_d_vs_stream0_fl & 0x07;
                let mut queue_soft_commit_close = false;
                let soft_commit_required = at
                    .pending_traffic_assignment
                    .as_ref()
                    .filter(|pending| pending.traffic_uati == uati)
                    .is_some_and(|pending| {
                        pending.session_personality_count > SESSION_PERSONALITY_COUNT_DEFAULT
                            && Self::pending_configuration_changes_in_use(at, pending)
                    });
                let packet = {
                    let pending = at
                        .pending_traffic_assignment
                        .as_mut()
                        .filter(|pending| pending.traffic_uati == uati && pending.active)?;
                    if pending.session_config_complete_sent {
                        return None;
                    }
                    pending.at_session_config_complete_transaction_id = Some(transaction_id);
                    let dh_selected = session_config_selected_subtype(
                        pending.session_config_trace.as_ref(),
                        SESSION_PROTOCOL_KEY_EXCHANGE,
                        SESSION_SUBTYPE_REV0,
                    );
                    if dh_selected && !pending.dh_key_exchange_complete {
                        if pending.dh_key_exchange.is_some() {
                            log::info!(
                                "HRPD AN: SessionConfigurationComplete received while negotiated DH KeyExchange is pending UATI=0x{uati:08x} transaction=0x{transaction_id:02x}; waiting for ATKeyComplete before AN complete"
                            );
                            return None;
                        }
                        let Some(key_exchange) = new_dh_key_exchange(sequence_number) else {
                            log::warn!(
                                "HRPD AN: failed to initialize DH KeyExchange UATI=0x{uati:08x}; cannot complete session configuration"
                            );
                            return None;
                        };
                        let packet = Self::build_dh_key_request_packet(
                            pending,
                            drc_index,
                            sequence_number,
                            ack_sequence_number,
                            &key_exchange,
                        )?;
                        pending.dh_key_exchange = Some(key_exchange);
                        log::info!(
                            "HRPD AN: SessionConfigurationComplete received UATI=0x{uati:08x} transaction=0x{transaction_id:02x}; queueing negotiated DH KeyRequest before AN complete"
                        );
                        packet
                    } else {
                        log::info!(
                            "HRPD AN: SessionConfigurationComplete received UATI=0x{uati:08x} transaction=0x{transaction_id:02x}; queueing AN complete token=0x0000 soft_commit_required={soft_commit_required}"
                        );
                        queue_soft_commit_close = soft_commit_required;
                        let packet = Self::build_session_configuration_complete_packet(
                            pending,
                            drc_index,
                            transaction_id,
                            sequence_number,
                            ack_sequence_number,
                            soft_commit_required,
                        )?;
                        pending.session_config_complete_sent = true;
                        packet
                    }
                };
                at.slp_d_vs_stream0_fl = at.slp_d_vs_stream0_fl.wrapping_add(1) & 0x07;
                Some((packet, false, queue_soft_commit_close))
            }
            SESSION_CONFIGURATION_REQUEST => {
                let request_attrs = payload[2..].to_vec();
                let attributes = configuration_response_attributes(
                    DEFAULT_SESSION_CONFIGURATION_PROTOCOL_TYPE,
                    &payload[2..],
                    SESSION_SUBTYPE_DEFAULT,
                );
                log::info!(
                    "HRPD AN: SessionConfigurationRequest received UATI=0x{uati:08x} transaction=0x{transaction_id:02x} request_attr_octets={} request_attrs={} response_attr_octets={} response_attrs={}",
                    payload.len().saturating_sub(2),
                    bytes_to_hex(&payload[2..]),
                    attributes.len(),
                    bytes_to_hex(&attributes)
                );
                if attributes.is_empty() && payload.len() > 2 {
                    log::info!(
                        "HRPD AN: SessionConfigurationRequest UATI=0x{uati:08x} has no selected attributes; AT will use fallbacks for skipped records"
                    );
                }
                let Some(pending) = at
                    .pending_traffic_assignment
                    .as_mut()
                    .filter(|pending| pending.traffic_uati == uati)
                else {
                    return None;
                };
                pending.session_personality_count = session_config_selected_u16_attribute(
                    &attributes,
                    SESSION_ATTRIBUTE_PERSONALITY_COUNT,
                )
                .unwrap_or(SESSION_PERSONALITY_COUNT_DEFAULT);
                log::info!(
                    "HRPD AN: SessionConfiguration selected PersonalityCount={} UATI=0x{uati:08x}",
                    pending.session_personality_count
                );
                let same_as_last_request =
                    pending.session_config_trace.as_ref().is_some_and(|trace| {
                        trace.transaction_id == transaction_id
                            && trace.request_attrs == request_attrs
                            && trace.response_attrs == attributes
                    });
                if same_as_last_request
                    && let Some(sequence_number) = pending
                        .session_config_trace
                        .as_ref()
                        .and_then(|trace| trace.sequence_number)
                {
                    log::info!(
                        "HRPD AN: duplicate SessionConfigurationRequest UATI=0x{uati:08x} transaction=0x{transaction_id:02x}; resending reliable_seq={sequence_number}"
                    );
                    return Self::build_session_configuration_response_packet_without_remembering(
                        pending,
                        drc_index,
                        transaction_id,
                        &attributes,
                        sequence_number,
                        ack_sequence_number,
                    )
                    .map(|packet| (packet, false, false));
                }
                let sequence_number = at.slp_d_vs_stream0_fl & 0x07;
                let packet = Self::build_session_configuration_response_packet(
                    pending,
                    drc_index,
                    transaction_id,
                    &attributes,
                    sequence_number,
                    ack_sequence_number,
                )?;
                pending.session_config_trace = Some(SessionConfigTrace {
                    transaction_id,
                    request_attrs,
                    response_attrs: attributes,
                    sequence_number: Some(sequence_number),
                });
                at.slp_d_vs_stream0_fl = at.slp_d_vs_stream0_fl.wrapping_add(1) & 0x07;
                Some((packet, false, false))
            }
            SESSION_CONFIGURATION_RESPONSE => {
                log::info!(
                    "HRPD AN: SessionConfigurationResponse received UATI=0x{uati:08x} transaction=0x{transaction_id:02x}; no AN action required"
                );
                None
            }
            _ => None,
        }
    }

    fn handle_stream0_slp_reset_ack(
        &mut self,
        at: &mut AtSession,
        uati: u32,
        message_sequence: u8,
        outcome: &mut HrpdTrafficOutcome,
    ) {
        let drc_index = self.current_forward_drc_index_for_uati(uati);
        let Some(pending) = at
            .pending_traffic_assignment
            .as_mut()
            .filter(|pending| pending.traffic_uati == uati)
        else {
            log::info!(
                "HRPD AN: decoded reverse Stream-0 DefaultSignaling ResetAck UATI=0x{uati:08x} seq={message_sequence} with no pending traffic assignment"
            );
            return;
        };
        if pending.stream0_slp_reset_sequence != message_sequence {
            log::warn!(
                "HRPD AN: ignoring Stream0 SLP ResetAck UATI=0x{uati:08x} seq={} expected_seq={}",
                message_sequence,
                pending.stream0_slp_reset_sequence
            );
            return;
        }
        if !pending.stream0_slp_reset_acked {
            pending.stream0_slp_reset_acked = true;
            pending.stream0_slp_reset_pending = false;
            log::info!(
                "HRPD AN: Stream0 SLP ResetAcked UATI=0x{uati:08x} seq={message_sequence}; Stream0 SNP delivery enabled"
            );
        } else {
            log::info!(
                "HRPD AN: duplicate Stream0 SLP ResetAck UATI=0x{uati:08x} seq={message_sequence}"
            );
        }
        outcome.forward_traffic.extend(
            Self::queue_session_configuration_start_after_stream0_slp_ready(at, uati, drc_index),
        );
        self.maybe_emit_session_configuration_complete_uati(at, uati, None, outcome);
    }

    fn mark_session_configuration_complete_acked(
        &mut self,
        at: &mut AtSession,
        uati: u32,
        label: &'static str,
        ack_sequence_number: Option<u8>,
        outcome: &mut HrpdTrafficOutcome,
    ) {
        if let Some(pending) = at
            .pending_traffic_assignment
            .as_mut()
            .filter(|pending| pending.traffic_uati == uati)
        {
            pending.an_session_config_complete_acked = true;
            if session_configuration_complete_label_requires_close(label) {
                pending.session_config_commit_connection_close_pending = true;
            }
            if !pending.stream0_slp_reset_acked {
                log::info!(
                    "HRPD AN: AN SessionConfigurationComplete ACKed UATI=0x{uati:08x}; waiting for Stream0 SLP readiness before A8 setup"
                );
                return;
            }
        }
        self.maybe_emit_session_configuration_complete_uati(at, uati, ack_sequence_number, outcome);
    }

    fn mark_soft_configuration_no_commit_sent(at: &AtSession, uati: u32) {
        let Some(pending) = at
            .pending_traffic_assignment
            .as_ref()
            .filter(|pending| pending.traffic_uati == uati)
        else {
            return;
        };
        if pending.session_personality_count <= SESSION_PERSONALITY_COUNT_DEFAULT
            || !pending.session_config_complete_sent
            || pending.an_session_config_complete_acked
            || pending.session_config_commit_connection_close_pending
            || !pending.stream0_slp_reset_acked
        {
            return;
        }
        log::info!(
            "HRPD AN: SoftConfigurationComplete Commit=0 sent UATI=0x{uati:08x}; waiting for AN reliable ACK before current traffic configuration is in use"
        );
    }

    fn maybe_emit_session_configuration_complete_uati(
        &mut self,
        at: &mut AtSession,
        uati: u32,
        _ack_sequence_number: Option<u8>,
        outcome: &mut HrpdTrafficOutcome,
    ) {
        let Some(pending) = at
            .pending_traffic_assignment
            .as_ref()
            .filter(|pending| pending.traffic_uati == uati)
        else {
            return;
        };
        if !pending.an_session_config_complete_acked || !pending.stream0_slp_reset_acked {
            return;
        }
        if pending.session_config_commit_connection_close_pending {
            self.queue_session_config_commit_connection_close(at, uati, outcome);
            return;
        }
        let _ = Self::emit_session_configuration_complete_event(
            at,
            uati,
            outcome,
            "AN SessionConfigurationComplete ACKed and Stream0 SLP ready",
        );
    }

    fn emit_session_configuration_complete_event(
        at: &mut AtSession,
        uati: u32,
        outcome: &mut HrpdTrafficOutcome,
        reason: &str,
    ) -> Option<(u16, u16, u16)> {
        let was_complete = at.session_configuration_complete;
        let (physical_layer_subtype, forward_traffic_mac_subtype, reverse_traffic_mac_subtype) =
            Self::commit_pending_session_configuration(at, uati)?;
        at.session_configuration_complete = true;
        outcome.session_configuration_complete_uatis.push(uati);
        outcome.session_configuration_complete_events.push(
            Self::session_configuration_complete_event(
                at,
                uati,
                physical_layer_subtype,
                forward_traffic_mac_subtype,
            ),
        );
        if was_complete {
            log::info!(
                "HRPD AN: {reason} UATI=0x{uati:08x}; current traffic configuration complete physical_subtype=0x{physical_layer_subtype:04x} ftc_mac_subtype=0x{forward_traffic_mac_subtype:04x} rtc_mac_subtype=0x{reverse_traffic_mac_subtype:04x}"
            );
        } else {
            log::info!(
                "HRPD AN: {reason} UATI=0x{uati:08x}; session configuration complete physical_subtype=0x{physical_layer_subtype:04x} ftc_mac_subtype=0x{forward_traffic_mac_subtype:04x} rtc_mac_subtype=0x{reverse_traffic_mac_subtype:04x}"
            );
        }
        Some((
            physical_layer_subtype,
            forward_traffic_mac_subtype,
            reverse_traffic_mac_subtype,
        ))
    }

    fn queue_session_config_commit_connection_close(
        &mut self,
        at: &mut AtSession,
        uati: u32,
        outcome: &mut HrpdTrafficOutcome,
    ) {
        let drc_index = self.current_forward_drc_index_for_uati(uati);
        let Some((packet, target_ati, session_uati, mac_index)) = ({
            let Some(pending) = at
                .pending_traffic_assignment
                .as_mut()
                .filter(|pending| pending.traffic_uati == uati && pending.active)
            else {
                return;
            };
            pending.session_config_commit_connection_close_pending = true;
            if pending.session_config_commit_connection_close_sent {
                log::info!(
                    "HRPD AN: SessionConfiguration commit ConnectionClose already queued UATI=0x{uati:08x}; waiting for Close Reply"
                );
                return;
            }
            let payload = [
                CONNECTED_STATE_CONNECTION_CLOSE,
                CONNECTION_CLOSE_REASON_NORMAL_UNSPECIFIED << 5,
            ];
            // C.S0024-400-C §1.7.6.2.1 / C.S0024-0 v4.0 §6.5.6.2.1
            // define ConnectionClose as SLP best effort on FTC/RTC. It is not
            // part of Stream 0 reliable delivery and must not consume V(S).
            let Some(packet) = Self::build_stream0_ftc_signaling_packet(
                pending,
                drc_index,
                DEFAULT_CONNECTED_STATE_PROTOCOL_TYPE,
                &payload,
                None,
                false,
                "ConnectionClose",
            ) else {
                return;
            };
            pending.session_config_commit_connection_close_sent = true;
            pending.session_config_commit_connection_close_sent_at = Some(Instant::now());
            let target_ati = AccessTerminalIdentifier {
                ati_type: AccessTerminalIdentifierType::Uati,
                value: pending.traffic_uati,
            };
            let mac_index = pending
                .assignment
                .pilots
                .first()
                .map(|pilot| pilot.mac_index)
                .unwrap_or(pending.traffic_request.mac_index);
            Some((packet, target_ati, pending.session_uati, mac_index))
        }) else {
            log::warn!(
                "HRPD AN: unable to queue SessionConfiguration commit ConnectionClose UATI=0x{uati:08x}"
            );
            return;
        };
        outcome.forward_traffic.push(packet);
        log::info!(
            "HRPD AN: SessionConfiguration commit requires CloseConnection; queued best-effort FTC ConnectedState ConnectionClose UATI=0x{uati:08x} session_uati=0x{session_uati:08x} MAC={mac_index} target={target_ati:?}"
        );
    }

    fn maybe_commit_session_configuration_after_connection_close(
        at: &mut AtSession,
        uati: u32,
        outcome: &mut HrpdTrafficOutcome,
    ) {
        let Some(pending) = at
            .pending_traffic_assignment
            .as_ref()
            .filter(|pending| pending.traffic_uati == uati)
        else {
            return;
        };
        if !pending.session_config_commit_connection_close_pending {
            return;
        }
        let _ = Self::emit_session_configuration_complete_event(
            at,
            uati,
            outcome,
            "ConnectionClose reply completes SessionConfiguration commit",
        );
    }

    fn maybe_expire_session_config_commit_connection_close(
        &mut self,
        at: &mut AtSession,
        now: Instant,
        outcome: &mut HrpdTrafficOutcome,
    ) {
        let Some((uati, sent_at)) = at
            .pending_traffic_assignment
            .as_ref()
            .filter(|pending| {
                pending.session_config_commit_connection_close_pending
                    && pending.session_config_commit_connection_close_sent
            })
            .and_then(|pending| {
                pending
                    .session_config_commit_connection_close_sent_at
                    .map(|sent_at| (pending.traffic_uati, sent_at))
            })
        else {
            return;
        };
        if now.duration_since(sent_at) < HRPD_CSP_CLOSE_TIMER {
            return;
        }
        if Self::emit_session_configuration_complete_event(
            at,
            uati,
            outcome,
            "TCSPClose expired; completing SessionConfiguration commit",
        )
        .is_none()
        {
            return;
        }
        self.release_pending_traffic_assignment_for_traffic(at, outcome, "TCSPClose");
        outcome.traffic_channel_closed_uatis.push(uati);
    }

    fn maybe_expire_pending_traffic_setup(
        &mut self,
        at: &mut AtSession,
        now: Instant,
        outcome: &mut HrpdTrafficOutcome,
    ) {
        let Some((traffic_uati, mac_index, setup_started_at, rtc_acquired, rtc_ack_delivered)) = at
            .pending_traffic_assignment
            .as_ref()
            .filter(|pending| !pending.active)
            .and_then(|pending| {
                pending.assignment.pilots.first().map(|pilot| {
                    (
                        pending.traffic_uati,
                        pilot.mac_index,
                        pending.setup_started_at,
                        pending.rtc_acquired,
                        pending.rtc_ack_delivered,
                    )
                })
            })
        else {
            return;
        };
        if now.saturating_duration_since(setup_started_at) < HRPD_RTCMP_AN_SETUP {
            return;
        }
        log::warn!(
            "HRPD AN: TRTCMPANSetup expired for traffic setup UATI=0x{traffic_uati:08x} MAC={mac_index} rtc_acquired={rtc_acquired} rtc_ack_delivered={rtc_ack_delivered}; releasing pending traffic assignment"
        );
        self.release_pending_traffic_assignment_for_traffic(at, outcome, "TRTCMPANSetup expired");
    }

    fn current_forward_drc_index_for_uati(&self, uati: u32) -> u8 {
        self.last_drc_by_uati
            .get(&uati)
            .copied()
            .filter(|idx| implemented_forward_traffic_payload_bits_for_drc(*idx).is_some())
            .unwrap_or(HRPD_RTC_ACK_DRC_INDEX)
    }

    fn mark_reliable_stream0_acknowledged(
        at: &mut AtSession,
        uati: u32,
        ack_sequence_number: u8,
    ) -> Vec<&'static str> {
        let Some(pending) = at
            .pending_traffic_assignment
            .as_mut()
            .filter(|pending| pending.traffic_uati == uati)
        else {
            return Vec::new();
        };
        let ack_sequence_number = ack_sequence_number & 0x07;
        let labels = pending
            .reliable_stream0_tx
            .iter()
            .filter(|packet| packet.sequence_number == ack_sequence_number)
            .map(|packet| packet.label)
            .collect::<Vec<_>>();
        let before = pending.reliable_stream0_tx.len();
        pending
            .reliable_stream0_tx
            .retain(|packet| packet.sequence_number != ack_sequence_number);
        let removed = before.saturating_sub(pending.reliable_stream0_tx.len());
        if removed > 0 {
            log::info!(
                "HRPD AN: Stream0 reliable forward ACKed UATI=0x{uati:08x} ack_seq={ack_sequence_number} labels={labels:?} removed={removed} pending={}",
                pending.reliable_stream0_tx.len()
            );
        }
        labels
    }

    fn maybe_retransmit_reliable_stream0(
        &mut self,
        at: &mut AtSession,
        now: Instant,
        outcome: &mut HrpdTrafficOutcome,
    ) {
        let Some((uati, mac_index, physical_layer_subtype, forward_traffic_mac_subtype)) = at
            .pending_traffic_assignment
            .as_ref()
            .filter(|pending| !pending.reliable_stream0_tx.is_empty())
            .and_then(|pending| {
                pending.assignment.pilots.first().map(|pilot| {
                    (
                        pending.traffic_uati,
                        pilot.mac_index,
                        pending.in_use_physical_layer_subtype,
                        pending.in_use_forward_traffic_mac_subtype,
                    )
                })
            })
        else {
            return;
        };
        let Some(drc_index) = self
            .last_drc_by_uati
            .get(&uati)
            .copied()
            .filter(|idx| implemented_forward_traffic_payload_bits_for_drc(*idx).is_some())
        else {
            log::debug!(
                "HRPD AN: Stream0 reliable forward retransmit pending UATI=0x{uati:08x}; waiting for valid DRC"
            );
            return;
        };
        let Some(pending) = at
            .pending_traffic_assignment
            .as_mut()
            .filter(|pending| pending.traffic_uati == uati)
        else {
            return;
        };

        let mut due_packets = Vec::new();
        let mut failed_sequences = Vec::new();
        let reliable_stream0_pending = pending.reliable_stream0_tx.len();
        for packet in &mut pending.reliable_stream0_tx {
            if now.saturating_duration_since(packet.last_send_at) < STREAM0_SLP_D_WAIT_ACK {
                continue;
            }
            if packet.attempts >= STREAM0_SLP_D_MAX_ATTEMPTS {
                failed_sequences.push(packet.sequence_number);
                log::warn!(
                    "HRPD AN: Stream0 reliable forward delivery failed UATI=0x{uati:08x} seq={} label={} attempts={} pending_after_timer={}",
                    packet.sequence_number,
                    packet.label,
                    packet.attempts,
                    reliable_stream0_pending.saturating_sub(failed_sequences.len())
                );
                continue;
            }
            packet.attempts += 1;
            packet.last_send_at = now;
            due_packets.push(packet.clone());
        }
        if !failed_sequences.is_empty() {
            pending
                .reliable_stream0_tx
                .retain(|packet| !failed_sequences.contains(&packet.sequence_number));
        }

        for packet in due_packets {
            if let Some(forward_packet) = Self::build_stream0_ftc_signaling_packet_for_mac(
                uati,
                mac_index,
                physical_layer_subtype,
                forward_traffic_mac_subtype,
                drc_index,
                packet.protocol_type,
                &packet.payload,
                Some(packet.sequence_number),
                packet.in_configuration,
                packet.ack_sequence_number,
                packet.label,
            ) {
                log::info!(
                    "HRPD AN: retransmitting Stream0 reliable forward UATI=0x{uati:08x} seq={} label={} attempt={} drc_index=0x{drc_index:x}",
                    packet.sequence_number,
                    packet.label,
                    packet.attempts
                );
                outcome.forward_traffic.push(forward_packet);
            }
        }
    }

    fn maybe_retransmit_stream0_slp_reset_on_drc(
        &mut self,
        at: &mut AtSession,
        uati: u32,
        slot: u64,
        outcome: &mut HrpdTrafficOutcome,
    ) {
        let drc_index = self.current_forward_drc_index_for_uati(uati);
        let Some(pending) = at.pending_traffic_assignment.as_mut().filter(|pending| {
            pending.traffic_uati == uati
                && pending.active
                && pending.stream0_slp_reset_pending
                && !pending.stream0_slp_reset_acked
        }) else {
            return;
        };
        let Some(last_send_slot) = pending.stream0_slp_reset_last_send_slot else {
            pending.stream0_slp_reset_last_send_slot = Some(slot);
            return;
        };
        if slot.saturating_sub(last_send_slot) < STREAM0_SLP_RESET_WAIT_ACK_SLOTS {
            return;
        }
        if pending.stream0_slp_reset_attempts >= STREAM0_SLP_RESET_MAX_ATTEMPTS {
            log::warn!(
                "HRPD AN: Stream0 SLP Reset delivery failed UATI=0x{uati:08x} seq={} attempts={}",
                pending.stream0_slp_reset_sequence,
                pending.stream0_slp_reset_attempts
            );
            pending.stream0_slp_reset_pending = false;
            return;
        }
        pending.stream0_slp_reset_attempts += 1;
        pending.stream0_slp_reset_last_send_slot = Some(slot);
        if let Some(packet) = Self::build_stream0_slp_reset_packet(pending, drc_index) {
            log::info!(
                "HRPD AN: retransmitting Stream0 SLP Reset UATI=0x{uati:08x} seq={} attempt={} drc_index=0x{drc_index:x}",
                pending.stream0_slp_reset_sequence,
                pending.stream0_slp_reset_attempts
            );
            outcome.forward_traffic.push(packet);
        }
    }

    /// DRC-paced reliable retransmission of RTCAck. While the AT keeps
    /// transmitting DRC without sending TrafficChannelComplete, it has not
    /// received (or not acted on) RTCAck. Retransmit the same logical message
    /// only after a slot-spaced guard interval; raw DRC event count is too
    /// dense when DRCLength is 2 slots.
    fn maybe_retransmit_rtc_ack_on_drc(
        at: &mut AtSession,
        uati: u32,
        slot: u64,
        drc_index: u8,
        outcome: &mut HrpdTrafficOutcome,
    ) {
        if implemented_forward_traffic_payload_bits_for_drc(drc_index).is_none() {
            return;
        }
        let Some(pending) = at.pending_traffic_assignment.as_mut().filter(|pending| {
            pending.traffic_uati == uati && pending.rtc_acquired && !pending.active
        }) else {
            return;
        };
        if pending.rtc_ack_retransmits >= RTC_ACK_MAX_RETRANSMITS {
            return;
        }
        if pending.rtc_ack_delivered
            && !pending.rtc_ack_needs_send
            && pending.in_use_reverse_traffic_mac_subtype == SESSION_SUBTYPE_RTC_MAC_SUBTYPE3
        {
            return;
        }
        if pending.rtc_ack_needs_send && pending.rtc_ack_vs.is_none() {
            let setup_age_slots = pending
                .setup_start_slot
                .map(|start| slot.saturating_sub(start))
                .unwrap_or_default();
            log::info!(
                "HRPD AN: initial RTCAck release on first DRC UATI=0x{uati:08x} drc=0x{drc_index:x} slot={} setup_age_slots={}",
                slot,
                setup_age_slots,
            );
            if let Some(packet) = Self::build_rtc_ack_packet(
                pending,
                &mut at.slp_d_vs_stream0_fl,
                drc_index,
                Some(slot),
            ) {
                outcome.forward_traffic.push(packet);
            }
            return;
        }
        if pending.rtc_ack_needs_send && pending.rtc_ack_last_send_slot.is_none() {
            if let Some(packet) = Self::build_rtc_ack_packet(
                pending,
                &mut at.slp_d_vs_stream0_fl,
                drc_index,
                Some(slot),
            ) {
                outcome.forward_traffic.push(packet);
            }
            return;
        }
        pending.drc_events_since_rtc_ack = pending.drc_events_since_rtc_ack.saturating_add(1);
        let Some(last_slot) = pending.rtc_ack_last_send_slot else {
            pending.rtc_ack_last_send_slot = Some(slot);
            return;
        };
        if slot.saturating_sub(last_slot) < RTC_ACK_RETRANSMIT_MIN_SLOTS {
            return;
        }
        pending.rtc_ack_needs_send = true;
        pending.rtc_ack_retransmits += 1;
        if let Some(packet) =
            Self::build_rtc_ack_packet(pending, &mut at.slp_d_vs_stream0_fl, drc_index, Some(slot))
        {
            outcome.forward_traffic.push(packet);
        }
    }

    fn maybe_send_post_rtc_ack_grant_on_drc(
        at: &mut AtSession,
        uati: u32,
        slot: u64,
        drc_index: u8,
        outcome: &mut HrpdTrafficOutcome,
    ) {
        if implemented_forward_traffic_payload_bits_for_drc(drc_index).is_none() {
            return;
        }
        let packet = {
            let Some(pending) = at.pending_traffic_assignment.as_mut().filter(|pending| {
                pending.traffic_uati == uati
                    && pending.active
                    && pending.rtc_acquired
                    && pending.rtc_ack_delivered
                    && pending.in_use_physical_layer_subtype == SESSION_SUBTYPE_PHYS_SUBTYPE2
                    && pending.in_use_reverse_traffic_mac_subtype
                        == SESSION_SUBTYPE_RTC_MAC_SUBTYPE3
            }) else {
                return;
            };
            if let Some(last_slot) = pending.rtc_grant_last_send_slot
                && slot.saturating_sub(last_slot) < RTC_MAC_GRANT_RETRANSMIT_MIN_SLOTS
            {
                return;
            }
            Self::build_rtc_mac_grant_packet(pending, drc_index, Some(slot))
        };
        if let Some(packet) = packet {
            outcome.forward_traffic.push(packet);
        }
    }

    fn autonomous_rtc_mac_grants() -> [MacFlowGrant; 2] {
        [
            MacFlowGrant {
                mac_flow_id: HRPD_AUTONOMOUS_SIGNALING_GRANT_MAC_FLOW_ID,
                t2p_inflow: HRPD_AUTONOMOUS_SIGNALING_GRANT_T2P_INFLOW_QUARTER_DB,
                bucket_level: HRPD_AUTONOMOUS_SIGNALING_GRANT_BUCKET_LEVEL_QUARTER_DB,
                tt2p_hold: HRPD_AUTONOMOUS_GRANT_TT2P_HOLD_FRAMES,
            },
            MacFlowGrant {
                mac_flow_id: HRPD_AUTONOMOUS_PACKET_GRANT_MAC_FLOW_ID,
                t2p_inflow: HRPD_AUTONOMOUS_PACKET_GRANT_T2P_INFLOW_QUARTER_DB,
                bucket_level: HRPD_AUTONOMOUS_PACKET_GRANT_BUCKET_LEVEL_QUARTER_DB,
                tt2p_hold: HRPD_AUTONOMOUS_GRANT_TT2P_HOLD_FRAMES,
            },
        ]
    }

    fn build_rtc_mac_grant_packet(
        pending: &mut PendingTrafficAssignment,
        drc_index: u8,
        send_slot: Option<u64>,
    ) -> Option<HrpdForwardTrafficPacket> {
        let uati = pending.traffic_uati;
        let mac_index = pending
            .assignment
            .pilots
            .first()
            .map(|pilot| pilot.mac_index)
            .unwrap_or_default();
        let Some(physical_packet_bits) =
            implemented_forward_traffic_payload_bits_for_drc(drc_index)
        else {
            log::info!(
                "HRPD AN: deferring subtype3 RTCMAC Grant UATI=0x{uati:08x} MAC={mac_index}; no implemented DRC for current index=0x{drc_index:x}"
            );
            return None;
        };
        let grants = Self::autonomous_rtc_mac_grants();
        match default_reverse_traffic_mac_grant_ftc_payload_bits_for_mac_subtype(
            physical_packet_bits,
            &grants,
            pending.in_use_forward_traffic_mac_subtype,
        ) {
            Ok(payload_bits) => {
                pending.rtc_grant_sends = pending.rtc_grant_sends.saturating_add(1);
                if let Some(slot) = send_slot {
                    pending.rtc_grant_last_send_slot = Some(slot);
                }
                log::debug!(
                    "HRPD AN: queueing best-effort subtype3 RTCMAC Grant UATI=0x{uati:08x} MAC={mac_index} send={} drc_index=0x{drc_index:x} payload_bits={physical_packet_bits} grants={grants:?}",
                    pending.rtc_grant_sends,
                );
                Some(HrpdForwardTrafficPacket {
                    uati,
                    mac_index,
                    physical_layer_subtype: pending.in_use_physical_layer_subtype,
                    forward_traffic_mac_subtype: pending.in_use_forward_traffic_mac_subtype,
                    payload_bits,
                })
            }
            Err(err) => {
                log::warn!(
                    "HRPD AN: failed to build subtype3 RTCMAC Grant UATI=0x{uati:08x} MAC={mac_index}: {err:?}"
                );
                None
            }
        }
    }

    fn queue_rtc_ack_for_reverse_pilot(
        &mut self,
        at: &mut AtSession,
        event: &HrpdTrafficEvent,
        uati: u32,
        outcome: &mut HrpdTrafficOutcome,
    ) {
        let current_drc = self.last_drc_by_uati.get(&uati).copied();
        let Some(pending) = at
            .pending_traffic_assignment
            .as_mut()
            .filter(|pending| pending.traffic_uati == uati)
        else {
            return;
        };
        if pending.active {
            if !pending.rtc_acquired {
                // The decoded TrafficChannelComplete can reach the AN just
                // before the rake's one-shot acquisition event. Preserve the
                // acquisition so post-RTCAck grant refreshes are not blocked
                // for the lifetime of the reopened connection.
                pending.rtc_acquired = true;
                log::info!(
                    "HRPD AN: ReversePilot arrived after TrafficChannelComplete UATI=0x{uati:08x} MAC={}; marking RTC acquired for open traffic",
                    pending.traffic_request.mac_index
                );
            } else {
                log::trace!(
                    "HRPD AN: ignoring post-setup ReversePilot UATI=0x{uati:08x} MAC={}; traffic channel is already complete",
                    pending.traffic_request.mac_index
                );
            }
            return;
        }
        if pending.rtc_acquired {
            log::debug!(
                "HRPD AN: ignoring duplicate ReversePilot acquisition UATI=0x{uati:08x} MAC={}; RTCAck setup gate is one-shot",
                pending.traffic_request.mac_index
            );
            return;
        }
        pending.rtc_acquired = true;
        let send_slot = match event {
            HrpdTrafficEvent::ReversePilot { absolute_chip, .. } => {
                Some(*absolute_chip / HRPD_REVERSE_TRAFFIC_SLOT_CHIPS)
            }
            _ => None,
        };
        if let Some(slot) = send_slot
            && let Some(start_slot) = pending.setup_start_slot
            && slot.saturating_sub(start_slot) > HRPD_RTCMP_AT_SETUP_SLOTS
        {
            log::info!(
                "HRPD AN: late ReversePilot UATI=0x{uati:08x} MAC={} slot={} setup_start_slot={} age_slots={} (TRTCMPATSetup={} slots, live_AN_guard={} slots); sending RTCAck anyway",
                pending.traffic_request.mac_index,
                slot,
                start_slot,
                slot.saturating_sub(start_slot),
                HRPD_RTCMP_AT_SETUP_SLOTS,
                HRPD_RTCMP_AN_SETUP_SLOTS,
            );
        }
        if pending.rtc_ack_needs_send && pending.rtc_ack_vs.is_none() {
            if let Some(slot) = send_slot
                && let Some(start_slot) = pending.setup_start_slot
            {
                let setup_age_slots = slot.saturating_sub(start_slot);
                let Some(drc_index) = current_drc else {
                    log::info!(
                        "HRPD AN: deferring initial RTCAck on ReversePilot UATI=0x{uati:08x} MAC={} slot={} setup_start_slot={} setup_age_slots={}; waiting for first valid DRC",
                        pending.traffic_request.mac_index,
                        slot,
                        start_slot,
                        setup_age_slots,
                    );
                    return;
                };
                log::info!(
                    "HRPD AN: initial RTCAck release on ReversePilot UATI=0x{uati:08x} MAC={} drc=0x{drc_index:x} slot={} setup_age_slots={}",
                    pending.traffic_request.mac_index,
                    slot,
                    setup_age_slots,
                );
            }
        }
        let Some(current_drc) = current_drc else {
            return;
        };
        if let Some(packet) =
            Self::build_rtc_ack_packet(pending, &mut at.slp_d_vs_stream0_fl, current_drc, send_slot)
        {
            outcome.forward_traffic.push(packet);
        }
    }

    fn build_rtc_ack_packet(
        pending: &mut PendingTrafficAssignment,
        slp_d_vs_stream0_fl: &mut u8,
        drc_index: u8,
        send_slot: Option<u64>,
    ) -> Option<HrpdForwardTrafficPacket> {
        if !pending.rtc_ack_needs_send {
            return None;
        }
        let Some(mac_index) = pending
            .assignment
            .pilots
            .first()
            .map(|pilot| pilot.mac_index)
        else {
            return None;
        };
        let uati = pending.traffic_uati;
        // First emission of this RTCAck consumes a fresh V(S); retransmissions
        // (triggered by ConnectionRequest retries after the AT failed to ack)
        // reuse the captured V(S) so the AT's SLP-D layer recognizes them as
        // the same logical message.
        let (sequence_number, vs_consumed) = match pending.rtc_ack_vs {
            Some(vs) => (vs, false),
            None => {
                let vs = *slp_d_vs_stream0_fl & 0x07;
                (vs, true)
            }
        };
        // Build for the decoded DRC that released setup. The scheduler
        // keeps authority over the actual start-slot DRC and may rebuild this
        // reliable RTCAck if the governing DRC has a different packet size.
        let Some(physical_packet_bits) =
            implemented_forward_traffic_payload_bits_for_drc(drc_index)
        else {
            log::info!(
                "HRPD AN: deferring RTCAck UATI=0x{uati:08x} MAC={mac_index}; no implemented DRC for current index=0x{drc_index:x}"
            );
            return None;
        };
        match default_reverse_traffic_mac_rtc_ack_ftc_payload_bits_for_mac_subtype(
            physical_packet_bits,
            sequence_number,
            pending.in_use_forward_traffic_mac_subtype,
        ) {
            Ok(payload_bits) => {
                log::info!(
                    "HRPD AN: queueing RTCAck UATI=0x{uati:08x} MAC={mac_index} slp_d_vs={sequence_number} drc_index=0x{drc_index:x} payload_bits={physical_packet_bits}"
                );
                pending.rtc_ack_vs = Some(sequence_number);
                pending.rtc_ack_needs_send = false;
                pending.drc_events_since_rtc_ack = 0;
                if let Some(slot) = send_slot {
                    pending.rtc_ack_last_send_slot = Some(slot);
                }
                let packet = HrpdForwardTrafficPacket {
                    uati,
                    mac_index,
                    physical_layer_subtype: pending.in_use_physical_layer_subtype,
                    forward_traffic_mac_subtype: pending.in_use_forward_traffic_mac_subtype,
                    payload_bits,
                };
                if vs_consumed {
                    *slp_d_vs_stream0_fl = slp_d_vs_stream0_fl.wrapping_add(1) & 0x07;
                }
                Some(packet)
            }
            Err(err) => {
                log::warn!(
                    "HRPD AN: failed to build RTCAck forward traffic packet UATI=0x{uati:08x} MAC={mac_index}: {err:?}"
                );
                None
            }
        }
    }

    fn mark_rtc_ack_delivered(at: &mut AtSession, uati: u32) {
        let Some(pending) = at.pending_traffic_assignment.as_mut().filter(|pending| {
            pending.traffic_uati == uati && pending.rtc_acquired && !pending.active
        }) else {
            return;
        };
        if !pending.rtc_ack_delivered {
            log::info!(
                "HRPD AN: RTCAck physically ACKed UATI=0x{uati:08x}; marking RTCAck delivered"
            );
        }
        // The H-ARQ ACK only proves the AT decoded the RTCAck packet. The
        // connection opens when the AT's TrafficChannelComplete arrives
        // (C.S0024 route update transaction), handled by
        // `handle_traffic_channel_complete`; TRTCMPANSetup supervision
        // releases the assignment if it never does.
        pending.rtc_ack_delivered = true;
    }

    fn queue_hardware_id_request(&mut self, at: &mut AtSession, outcome: &mut HrpdAccessOutcome) {
        let Some(session) = at.session.session() else {
            return;
        };
        let uati = session.uati.as_u32();
        if matches!(at.pending_hardware_id, Some((pending_uati, _)) if pending_uati == uati) {
            return;
        }
        let transaction_id = self.next_hardware_id_transaction();
        at.pending_hardware_id = Some((uati, transaction_id));
        outcome
            .forward_signaling
            .push(HrpdForwardSignalingRequest::hardware_id_request(
                uati,
                self.uati_receive_ati(uati),
                transaction_id,
            ));
    }

    fn handle_hardware_id_response(
        &mut self,
        at: &mut AtSession,
        response: &HrpdHardwareIdResponse,
        outcome: &mut HrpdAccessOutcome,
    ) {
        outcome.hardware_id_responses.push(response.clone());
        if let Some((pending_uati, transaction_id)) = at.pending_hardware_id {
            if transaction_id == response.transaction_id {
                self.hardware_ids_by_uati
                    .insert(pending_uati, response.clone());
                at.pending_hardware_id = None;
            }
        }
    }

    fn uati_receive_ati(&self, uati: u32) -> AccessTerminalIdentifier {
        self.uati_receive_ati_from_value(self.uati_receive_value(uati))
    }

    fn uati_receive_ati_from_value(&self, value: u32) -> AccessTerminalIdentifier {
        AccessTerminalIdentifier {
            ati_type: AccessTerminalIdentifierType::Uati,
            value,
        }
    }

    fn uati_receive_value(&self, uati: u32) -> u32 {
        (u32::from(self.color_code) << 24) | (uati & 0x00ff_ffff)
    }

    fn uati_assignment_subnet_for_request(
        &self,
        request_ati: AccessTerminalIdentifier,
    ) -> Option<HrpdUatiSubnetAssignment> {
        let _ = request_ati;
        // C.S0024-400 §5.3.7.1.5.2 allows the AN to include UATI104 and
        // UATISubnetMask in a UATIAssignment sent in response to UATIRequest.
        // Including it also makes the assignment "fresh" without depending on
        // the AT having accepted the latest overhead bundle.
        self.uati_subnet_assignment.clone()
    }

    fn accept_or_drop_event(
        &self,
        at: &AtSession,
        uati: u32,
        outcome: &mut HrpdTrafficOutcome,
    ) -> bool {
        if self.is_known_uati(at, uati) {
            outcome.accepted_event_count = 1;
            true
        } else {
            outcome.dropped_event_count = 1;
            outcome.unknown_session_count = 1;
            false
        }
    }

    fn is_known_uati(&self, at: &AtSession, uati: u32) -> bool {
        at.pending_traffic_assignment
            .as_ref()
            .is_some_and(|pending| pending.traffic_uati == uati)
            || at
                .session
                .session()
                .is_some_and(|session| self.uati_receive_value(session.uati.as_u32()) == uati)
    }

    fn next_uati_assignment_sequence(at: &mut AtSession) -> u8 {
        let sequence = at.uati_assignment_sequence;
        at.uati_assignment_sequence = at.uati_assignment_sequence.wrapping_add(1);
        sequence
    }

    fn traffic_channel_assignment_signaling(
        uati: u32,
        target_ati: AccessTerminalIdentifier,
        assignment: HrpdTrafficChannelAssignment,
        route_update_subtype: u16,
        default_route_update_tca_rev_a_tail: bool,
        reliable_sequence: Option<u8>,
    ) -> HrpdForwardSignalingRequest {
        let mut request =
            HrpdForwardSignalingRequest::traffic_channel_assignment_for_route_update_subtype_with_rev_a_tail(
                uati,
                target_ati,
                assignment,
                route_update_subtype,
                default_route_update_tca_rev_a_tail,
            );
        request.reliable_sequence = reliable_sequence.map(|sequence| sequence & 0x07);
        request
    }

    fn next_hardware_id_transaction(&mut self) -> u8 {
        self.hardware_id_transaction = self.hardware_id_transaction.wrapping_add(1);
        self.hardware_id_transaction
    }

    fn allocate_mac_index(&mut self) -> u8 {
        let mac_index = self.next_mac_index;
        self.next_mac_index = if self.next_mac_index >= HRPD_LAST_TRAFFIC_MAC_INDEX {
            HRPD_FIRST_TRAFFIC_MAC_INDEX
        } else {
            self.next_mac_index + 1
        };
        mac_index
    }
}

/// The traffic UATI carried by any reverse traffic event.
fn traffic_event_uati(event: &HrpdTrafficEvent) -> u32 {
    match event {
        HrpdTrafficEvent::ReversePilot { uati, .. }
        | HrpdTrafficEvent::ReversePilotLost { uati, .. }
        | HrpdTrafficEvent::Drc { uati, .. }
        | HrpdTrafficEvent::Ack { uati, .. }
        | HrpdTrafficEvent::Stream0Signaling { uati, .. }
        | HrpdTrafficEvent::Stream1Packet { uati, .. } => *uati,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct ParsedStream0Signaling {
    message: Option<HrpdAccessMessage>,
    ack_sequence_number: Option<u8>,
    sequence_number: Option<u8>,
    in_configuration: Option<bool>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Stream0ParseOutcome {
    Complete(ParsedStream0Signaling),
    InProgress,
    Invalid,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Stream0SlpFPacket {
    Complete(Vec<u8>),
    Fragment {
        begin: bool,
        end: bool,
        sequence: u8,
        payload_bits: Vec<u8>,
    },
}

fn bytes_to_hex(bytes: &[u8]) -> String {
    let mut out = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

fn bytes_to_bits_msb(bytes: &[u8]) -> Vec<u8> {
    let mut bits = Vec::with_capacity(bytes.len() * 8);
    for byte in bytes {
        for shift in (0..8).rev() {
            bits.push((byte >> shift) & 1);
        }
    }
    bits
}

fn read_bits_msb(bits: &[u8], cursor: &mut usize, n: usize) -> Option<u32> {
    if bits.len().saturating_sub(*cursor) < n {
        return None;
    }
    let mut value = 0u32;
    for _ in 0..n {
        value = (value << 1) | u32::from(bits[*cursor] & 1);
        *cursor += 1;
    }
    Some(value)
}

fn read_bits_msb_u64(bits: &[u8], cursor: &mut usize, n: usize) -> Option<u64> {
    if bits.len().saturating_sub(*cursor) < n {
        return None;
    }
    let mut value = 0u64;
    for _ in 0..n {
        value = (value << 1) | u64::from(bits[*cursor] & 1);
        *cursor += 1;
    }
    Some(value)
}

fn pack_bits_msb(bits: &[u8]) -> Vec<u8> {
    bits.chunks(8)
        .map(|chunk| chunk.iter().fold(0u8, |acc, bit| (acc << 1) | (bit & 1)))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use cdma_common::hrpd::air::{
        AccessTerminalIdentifier, AccessTerminalIdentifierType,
        DEFAULT_ADDRESS_MANAGEMENT_PROTOCOL_TYPE, DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE,
        DEFAULT_SESSION_MANAGEMENT_PROTOCOL_TYPE, HrpdConnectionRequest, HrpdForwardChannel,
        HrpdForwardSignalingRequest, HrpdUatiComplete, HrpdUatiRequest,
        encode_default_signaling_slp_d_ack_packet, encode_reliable_default_signaling_packet,
    };
    use cdma_common::hrpd::messages::DEFAULT_ACCESS_CHANNEL_MAC_PROTOCOL_TYPE;

    use crate::session::SessionState;
    use crate::subnet::UatiSubnet;

    fn allocator() -> UatiAllocator {
        UatiAllocator::new(UatiSubnet {
            color_code: 26,
            uati104: [0; 13],
            subnet_mask: 26,
        })
    }

    fn indication(messages: Vec<HrpdAccessMessage>) -> HrpdAccessIndication {
        HrpdAccessIndication {
            absolute_chip: 42,
            color_code: 26,
            sector_pilot_pn: 0,
            session_configuration_token: 0,
            ati: AccessTerminalIdentifier {
                ati_type: AccessTerminalIdentifierType::Rati,
                value: 0x5232_af53,
            },
            security_layer_format: false,
            connection_layer_format: true,
            security_payload: Vec::new(),
            messages,
        }
    }

    fn indication_with_ati(
        ati: AccessTerminalIdentifier,
        messages: Vec<HrpdAccessMessage>,
    ) -> HrpdAccessIndication {
        let mut ind = indication(messages);
        ind.ati = ati;
        ind
    }

    /// Access-form UATI a compliant AT uses to address every message after it
    /// accepts the default test UATIAssignment. Per C.S0024-0 §5.3.7.1.5.1 the
    /// AT sets `TransmitATI = UATI` before sending `UatiComplete`, so the RATI
    /// never appears on the access channel again.
    fn default_access_uati() -> AccessTerminalIdentifier {
        AccessTerminalIdentifier {
            ati_type: AccessTerminalIdentifierType::Uati,
            value: 0x1a05_8001,
        }
    }

    /// `indication` addressed by the access UATI, for post-UATIComplete messages.
    fn uati_indication(messages: Vec<HrpdAccessMessage>) -> HrpdAccessIndication {
        indication_with_ati(default_access_uati(), messages)
    }

    fn rtc_ack_release_chip_after_slot(setup_start_slot: u64) -> u64 {
        initial_rtc_ack_release_slot_after_slot(setup_start_slot) * HRPD_REVERSE_TRAFFIC_SLOT_CHIPS
    }

    fn initial_rtc_ack_release_chip() -> u64 {
        rtc_ack_release_chip_after_slot(0)
    }

    fn initial_rtc_ack_release_slot_after_slot(setup_start_slot: u64) -> u64 {
        setup_start_slot
    }

    fn initial_rtc_ack_release_slot() -> u64 {
        initial_rtc_ack_release_slot_after_slot(0)
    }

    fn reverse_pilot(
        controller: &mut HrpdAirController,
        uati: u32,
        mac_index: u8,
        absolute_chip: u64,
    ) -> HrpdTrafficOutcome {
        controller.handle_traffic_event(&HrpdTrafficEvent::ReversePilot {
            uati,
            mac_index,
            absolute_chip,
            snr_db_tenths: 80,
        })
    }

    fn drive_setup_drc(
        controller: &mut HrpdAirController,
        uati: u32,
        mac_index: u8,
        setup_start_slot: u64,
        drc_index: u8,
    ) -> HrpdTrafficOutcome {
        controller.handle_traffic_event(&HrpdTrafficEvent::Drc {
            uati,
            mac_index,
            slot: setup_start_slot,
            drc_index,
        })
    }

    fn acack(out: &HrpdAccessOutcome) -> &HrpdForwardSignalingRequest {
        out.forward_signaling
            .iter()
            .find(|msg| msg.protocol_type == DEFAULT_ACCESS_CHANNEL_MAC_PROTOCOL_TYPE)
            .expect("expected ACAck")
    }

    fn signaling_for(out: &HrpdAccessOutcome, protocol_type: u8) -> &HrpdForwardSignalingRequest {
        out.forward_signaling
            .iter()
            .find(|msg| msg.protocol_type == protocol_type)
            .expect("expected protocol signaling")
    }

    fn non_acack_signaling(out: &HrpdAccessOutcome) -> Vec<&HrpdForwardSignalingRequest> {
        out.forward_signaling
            .iter()
            .filter(|msg| msg.protocol_type != DEFAULT_ACCESS_CHANNEL_MAC_PROTOCOL_TYPE)
            .collect()
    }

    #[test]
    fn session_configuration_selects_enhanced_idle_when_default_is_unoffered() {
        let request = [0x04, 0x00, 0x0c, 0x00, 0x01];

        let response = default_session_configuration_response_attributes(&request);

        assert_eq!(response, vec![0x04, 0x00, 0x0c, 0x00, 0x01]);
    }

    #[test]
    fn session_configuration_selects_default_personality_count_when_offered() {
        let request = [0x08, 0x01, 0x10, 0x00, 0x04, 0x00, 0x02, 0x00, 0x01];

        let response = default_session_configuration_response_attributes(&request);

        assert_eq!(response, vec![0x04, 0x01, 0x10, 0x00, 0x01]);
    }

    #[test]
    fn session_configuration_skips_multi_personality_when_default_not_offered() {
        let request = [0x04, 0x01, 0x10, 0x00, 0x04];

        let response = default_session_configuration_response_attributes(&request);

        assert_eq!(response, Vec::<u8>::new());
    }

    #[test]
    fn session_configuration_prefers_rev_a_when_offered_alongside_defaults() {
        // Every traffic protocol offers both its Rev A subtype and the
        // default: the Rev A personality wins. Idle State is not part of the
        // Rev A tuple and keeps its default-first preference.
        let request = [
            0x06, 0x00, 0x0c, 0x00, 0x01, 0x00, 0x00, 0x06, 0x00, 0x00, 0x00, 0x02, 0x00, 0x00,
            0x06, 0x00, 0x02, 0x00, 0x01, 0x00, 0x00, 0x06, 0x00, 0x01, 0x00, 0x01, 0x00, 0x00,
            0x06, 0x00, 0x04, 0x00, 0x03, 0x00, 0x01, 0x06, 0x00, 0x03, 0x00, 0x01, 0x00, 0x00,
            0x04, 0x00, 0x1b, 0x00, 0x01,
        ];

        let response = default_session_configuration_response_attributes(&request);

        assert_eq!(
            response,
            vec![
                0x04, 0x00, 0x0c, 0x00, 0x00, 0x04, 0x00, 0x00, 0x00, 0x02, 0x04, 0x00, 0x02, 0x00,
                0x01, 0x04, 0x00, 0x01, 0x00, 0x01, 0x04, 0x00, 0x04, 0x00, 0x03, 0x04, 0x00, 0x03,
                0x00, 0x01, 0x04, 0x00, 0x1b, 0x00, 0x01,
            ]
        );
    }

    #[test]
    fn session_configuration_uses_soft_complete_for_multi_personality() {
        let (payload, label) = session_configuration_complete_payload(0x7a, 4, true);

        assert_eq!(label, "SoftConfigurationComplete");
        assert_eq!(payload, vec![0x02, 0x7a, 0x04, 0x00, 0x00]);
    }

    #[test]
    fn session_configuration_soft_complete_can_avoid_commit_close() {
        let (payload, label) = session_configuration_complete_payload(0x7a, 4, false);

        assert_eq!(label, "SoftConfigurationCompleteNoCommit");
        assert_eq!(payload, vec![0x02, 0x7a, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn session_configuration_requires_full_rev_a_offer_for_physical_subtype2() {
        // PHY subtype 2 + RTC MAC 3 offered without the Enhanced CC/AC/FTC
        // MACs: not a complete Rev A personality, so everything falls back to
        // defaults by omission.
        let request = [
            0x04, 0x00, 0x00, 0x00, 0x02, 0x06, 0x00, 0x04, 0x00, 0x03, 0x00, 0x01,
        ];

        let response = default_session_configuration_response_attributes(&request);

        assert_eq!(response, Vec::<u8>::new());
    }

    /// The exact SessionConfigurationRequest attribute block the live Rev A
    /// handset sends. The AN must select the full Rev A personality:
    /// physical subtype 2, Enhanced CC/AC/FTC MAC, RTC MAC subtype 3, plus
    /// the previously accepted Enhanced Idle State and Generic MMCD. Route
    /// Update is absent from the response, so the top-level Route Update
    /// protocol remains default; Rev A public data is carried in the default
    /// TCA's optional tail.
    #[test]
    fn session_configuration_selects_rev_a_personality_from_live_offer() {
        let request = hex_to_bytes(
            "04000c000104000000020400020001040001000106000400030001040003000104001b000104000500\
             0104000600010400080001121001000700010002000300050009000afffe0401100004",
        );

        let response = default_session_configuration_response_attributes(&request);

        let expected = hex_to_bytes(
            "04000c00010400000002040002000104000100010400040003040003000104001b000103100100",
        );
        assert_eq!(
            bytes_to_hex(&response),
            bytes_to_hex(&expected),
            "Rev A personality selection from the live handset offer"
        );
    }

    #[test]
    fn live_session_commit_uses_default_route_update_with_rev_a_tca_tail() {
        let request = hex_to_bytes(
            "04000c000104000000020400020001040001000106000400030001040003000104001b000104000500\
             0104000600010400080001121001000700010002000300050009000afffe0401100004",
        );

        let response = default_session_configuration_response_attributes(&request);
        let mut at = AtSession::new(26);
        at.session_configuration_complete = true;
        at.committed_session_configuration_response = Some(response.clone());

        assert_eq!(
            HrpdAirController::current_session_traffic_subtypes(&at),
            (
                SESSION_SUBTYPE_PHYS_SUBTYPE2,
                SESSION_SUBTYPE_ENHANCED,
                SESSION_SUBTYPE_RTC_MAC_SUBTYPE3,
            )
        );
        assert_eq!(
            session_config_selected_u16_attribute(
                &response,
                [0x00, DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE],
            ),
            None
        );
        assert_eq!(
            HrpdAirController::current_route_update_subtype(&at),
            SESSION_SUBTYPE_DEFAULT,
            "the live offer omits a top-level Route Update subtype selection"
        );
        assert!(
            HrpdAirController::default_route_update_tca_rev_a_tail_eligible(&at),
            "Rev A traffic personality is eligible for the default TCA optional public-data tail"
        );
        assert!(
            !HrpdAirController::current_default_route_update_tca_rev_a_tail(&at),
            "the TCA tail is never sent (the live Rev A handset stalls traffic setup with it)"
        );
    }

    #[test]
    fn live_session_configuration_reopens_rev_a_traffic_with_default_tca() {
        let mut controller = HrpdAirController::with_sector(26, 0, None);
        let mut allocator = allocator();
        let _ = controller
            .handle_access_indication(
                &indication(vec![HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
                    transaction_id: 0x9c,
                })]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::UatiComplete(HrpdUatiComplete {
                    message_sequence: 0,
                    upper_old_uati: Vec::new(),
                    reserved_zero: true,
                })]),
                &mut allocator,
            )
            .unwrap();

        let first = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::ConnectionRequest(
                    HrpdConnectionRequest {
                        transaction_id: 0x21,
                        request_reason: 0,
                        reserved_zero: true,
                    },
                )]),
                &mut allocator,
            )
            .unwrap();
        assert_eq!(first.traffic_assignments.len(), 1);
        let first_traffic = first.traffic_assignments[0].clone();
        assert_eq!(
            first_traffic.physical_layer_subtype,
            SESSION_SUBTYPE_DEFAULT
        );
        assert_eq!(
            first_traffic.reverse_traffic_mac_subtype,
            SESSION_SUBTYPE_DEFAULT
        );
        assert_eq!(first_traffic.reverse_rate_limit_bps, 153_600);

        let _ = drive_setup_drc(
            &mut controller,
            first_traffic.uati,
            first_traffic.mac_index,
            0,
            0xc,
        );
        let tcc = encode_reliable_default_signaling_packet(
            DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE,
            &[0x02, 0x00],
            0,
        );
        let tcc = controller.handle_traffic_event(&HrpdTrafficEvent::Stream0Signaling {
            uati: first_traffic.uati,
            payload: tcc,
        });
        assert_eq!(tcc.traffic_channel_open_uatis, vec![first_traffic.uati]);

        let reset_ack = controller.handle_traffic_event(&HrpdTrafficEvent::Stream0Signaling {
            uati: first_traffic.uati,
            payload: vec![0, 1, 0],
        });
        assert!(!reset_ack.forward_traffic.is_empty());

        let mut session_request = vec![SESSION_CONFIGURATION_REQUEST, 0x11];
        session_request.extend_from_slice(&hex_to_bytes(
            "04000c000104000000020400020001040001000106000400030001040003000104001b000104000500\
             0104000600010400080001121001000700010002000300050009000afffe0401100004",
        ));
        let session_request = encode_reliable_default_signaling_packet(
            DEFAULT_SESSION_CONFIGURATION_PROTOCOL_TYPE,
            &session_request,
            1,
        );
        let response = controller.handle_traffic_event(&HrpdTrafficEvent::Stream0Signaling {
            uati: first_traffic.uati,
            payload: session_request,
        });
        assert_eq!(
            response.session_configuration_pending_uatis,
            vec![first_traffic.uati]
        );
        assert_eq!(response.forward_traffic.len(), 1);
        let response_seq = {
            let pending = controller.pending_traffic_assignment().unwrap();
            let trace = pending.session_config_trace.as_ref().unwrap();
            assert_eq!(
                session_config_selected_u16_attribute(
                    &trace.response_attrs,
                    [0x00, DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE],
                ),
                None
            );
            assert_eq!(
                pending.in_use_physical_layer_subtype, SESSION_SUBTYPE_DEFAULT,
                "Rev A subtypes must not be marked in-use until config commit"
            );
            pending
                .reliable_stream0_tx
                .iter()
                .find(|packet| packet.label == "SessionConfigurationResponse")
                .expect("SessionConfigurationResponse should await ACK")
                .sequence_number
        };
        let _ = controller.handle_traffic_event(&HrpdTrafficEvent::Stream0Signaling {
            uati: first_traffic.uati,
            payload: encode_default_signaling_slp_d_ack_packet(response_seq),
        });

        let complete = encode_reliable_default_signaling_packet(
            DEFAULT_SESSION_CONFIGURATION_PROTOCOL_TYPE,
            &[SESSION_CONFIGURATION_COMPLETE, 0x12],
            2,
        );
        let complete = controller.handle_traffic_event(&HrpdTrafficEvent::Stream0Signaling {
            uati: first_traffic.uati,
            payload: complete,
        });
        assert_eq!(complete.forward_traffic.len(), 1);
        assert!(complete.session_configuration_complete_uatis.is_empty());
        let complete_seq = controller
            .pending_traffic_assignment()
            .unwrap()
            .reliable_stream0_tx
            .iter()
            .find(|packet| packet.label == "SessionConfigurationComplete")
            .expect("SessionConfigurationComplete should await ACK before commit close")
            .sequence_number;

        let commit_close = controller.handle_traffic_event(&HrpdTrafficEvent::Stream0Signaling {
            uati: first_traffic.uati,
            payload: encode_default_signaling_slp_d_ack_packet(complete_seq),
        });
        assert_eq!(commit_close.forward_traffic.len(), 1);
        assert!(commit_close.session_configuration_complete_uatis.is_empty());

        let close_reply = encode_reliable_default_signaling_packet(
            DEFAULT_CONNECTED_STATE_PROTOCOL_TYPE,
            &[0x00, 0x40],
            3,
        );
        let close_reply = controller.handle_traffic_event(&HrpdTrafficEvent::Stream0Signaling {
            uati: first_traffic.uati,
            payload: close_reply,
        });
        assert_eq!(
            close_reply.session_configuration_complete_uatis,
            vec![first_traffic.uati]
        );
        assert_eq!(
            close_reply.traffic_releases,
            vec![HrpdTrafficReleaseRequest {
                uati: first_traffic.uati,
                mac_index: first_traffic.mac_index,
            }]
        );
        let event = close_reply
            .session_configuration_complete_events
            .first()
            .expect("SessionConfigurationComplete event should carry committed subtypes");
        assert_eq!(event.physical_layer_subtype, SESSION_SUBTYPE_PHYS_SUBTYPE2);
        assert_eq!(event.forward_traffic_mac_subtype, SESSION_SUBTYPE_ENHANCED);
        assert!(controller.pending_traffic_assignment().is_none());

        let mut reopen = indication(vec![HrpdAccessMessage::ConnectionRequest(
            HrpdConnectionRequest {
                transaction_id: 0x22,
                request_reason: 0,
                reserved_zero: true,
            },
        )]);
        reopen.ati = AccessTerminalIdentifier {
            ati_type: AccessTerminalIdentifierType::Uati,
            value: first_traffic.uati,
        };
        let reopen = controller
            .handle_access_indication(&reopen, &mut allocator)
            .unwrap();

        assert_eq!(reopen.traffic_assignments.len(), 1);
        assert_eq!(
            reopen.traffic_assignments[0].physical_layer_subtype,
            SESSION_SUBTYPE_PHYS_SUBTYPE2
        );
        assert_eq!(
            reopen.traffic_assignments[0].reverse_traffic_mac_subtype,
            SESSION_SUBTYPE_RTC_MAC_SUBTYPE3
        );
        assert_eq!(
            reopen.traffic_assignments[0].reverse_rate_limit_bps,
            1_843_200
        );
        assert_eq!(reopen.forward_traffic.len(), 1);
        let expected_rtca = default_reverse_traffic_mac_rtc_ack_ftc_payload_bits_for_mac_subtype(
            implemented_forward_traffic_payload_bits_for_drc(HRPD_RTC_ACK_DRC_INDEX).unwrap(),
            0,
            SESSION_SUBTYPE_ENHANCED,
        )
        .unwrap();
        assert_eq!(reopen.forward_traffic[0].payload_bits, expected_rtca);
        let expected_tca = HrpdTrafficChannelAssignment::single_pilot(
            0,
            None,
            0,
            reopen.traffic_assignments[0].mac_index,
        )
        .encode_subtype0_route_update();
        assert_eq!(
            signaling_for(&reopen, DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE).payload,
            expected_tca,
            "Rev A reopen keeps the default TCA grammar with the optional RA/DSC tail omitted"
        );
        assert_eq!(
            signaling_for(&reopen, DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE).reliable_sequence,
            None,
            "OpenConnection-triggered TCA should use best-effort SLP"
        );
    }

    #[test]
    fn session_configuration_rejects_rtc_mac_subtype3_without_physical_subtype2() {
        // RTC MAC 3 offered alone: default physical layer only pairs with
        // the default RTC MAC, so the attribute is skipped.
        let request = [0x06, 0x00, 0x04, 0x00, 0x03, 0x00, 0x00];

        let response = default_session_configuration_response_attributes(&request);

        assert_eq!(response, vec![0x04, 0x00, 0x04, 0x00, 0x00]);
    }

    /// The live handset's subtype-3 RTC MAC in-configuration request:
    /// MaxMACFlows (two-octet attribute 0x0014) offering a non-default
    /// ValueID. No subtype-3 RTC MAC attribute is selected — MaxMACFlows
    /// included — so the GCP fallback keeps both sides at the spec defaults.
    #[test]
    fn rtc_mac_subtype3_configuration_skips_all_attributes() {
        let request = [
            0x05, 0x00, 0x14, 0x70, 0x08, 0x08, // MaxMACFlows.
            0x03, 0x00, 0x21, 0x02, // Unknown/unimplemented public data.
        ];

        let response = configuration_response_attributes(
            SESSION_PROTOCOL_REVERSE_TRAFFIC_CHANNEL_MAC,
            &request,
            SESSION_SUBTYPE_RTC_MAC_SUBTYPE3,
        );

        assert_eq!(response, Vec::<u8>::new());

        // Default-subtype sessions keep the Rev 0 one-octet form.
        let rev0 = configuration_response_attributes(
            SESSION_PROTOCOL_REVERSE_TRAFFIC_CHANNEL_MAC,
            &[0x02, 0x00, 0x01],
            SESSION_SUBTYPE_DEFAULT,
        );
        assert_eq!(rev0, vec![0x02, 0x00, 0x01]);
    }

    fn hex_to_bytes(hex: &str) -> Vec<u8> {
        let cleaned: String = hex.chars().filter(|c| !c.is_whitespace()).collect();
        cleaned
            .as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                u8::from_str_radix(std::str::from_utf8(pair).expect("hex utf8"), 16)
                    .expect("hex digit")
            })
            .collect()
    }

    #[test]
    fn session_configuration_skips_reverse_traffic_mac_subtype1_for_default_physical() {
        let request = [0x06, 0x00, 0x04, 0x00, 0x03, 0x00, 0x01];

        let response = default_session_configuration_response_attributes(&request);

        assert_eq!(response, Vec::<u8>::new());
    }

    #[test]
    fn dh_key_exchange_configuration_selects_implemented_key_length() {
        let request = [0x03, 0x00, 0x00, 0x01];

        let response = configuration_response_attributes(
            SESSION_PROTOCOL_KEY_EXCHANGE,
            &request,
            SESSION_SUBTYPE_DEFAULT,
        );

        assert_eq!(response, vec![0x02, 0x00, 0x00]);
    }

    #[test]
    fn session_configuration_prefers_default_over_earlier_enhanced_offer() {
        let request = [
            0x06, 0x00, 0x0c, 0x00, 0x01, 0x00, 0x00, 0x04, 0x00, 0x01, 0x00, 0x01,
        ];

        let response = default_session_configuration_response_attributes(&request);

        assert_eq!(response, vec![0x04, 0x00, 0x0c, 0x00, 0x00]);
    }

    #[test]
    fn session_configuration_selects_live_idle_state_offer() {
        let request = [
            0x04, 0x00, 0x0c, 0x00, 0x01, 0x04, 0x00, 0x00, 0x00, 0x02, 0x04, 0x00, 0x02, 0x00,
            0x01, 0x04, 0x00, 0x01, 0x00, 0x01, 0x06, 0x00, 0x04, 0x00, 0x03, 0x00, 0x01, 0x04,
            0x00, 0x03, 0x00, 0x01, 0x04, 0x00, 0x1b, 0x00, 0x01, 0x04, 0x00, 0x05, 0x00, 0x01,
            0x04, 0x00, 0x06, 0x00, 0x01, 0x04, 0x00, 0x08, 0x00, 0x01, 0x12, 0x10, 0x01, 0x00,
            0x07, 0x00, 0x01, 0x00, 0x02, 0x00, 0x03, 0x00, 0x05, 0x00, 0x09, 0x00, 0x0a, 0xff,
            0xfe, 0x04, 0x01, 0x10, 0x00, 0x04,
        ];

        let response = default_session_configuration_response_attributes(&request);

        assert_eq!(
            session_config_selected_u16_attribute(
                &response,
                [0x00, DEFAULT_IDLE_STATE_PROTOCOL_TYPE],
            ),
            Some(SESSION_SUBTYPE_REV0)
        );
    }

    #[test]
    fn idle_state_configuration_selects_preferred_control_channel_cycle() {
        let request = [0x04, 0x00, 0x72, 0x80, 0x0a];

        let response = configuration_response_attributes(
            DEFAULT_IDLE_STATE_PROTOCOL_TYPE,
            &request,
            SESSION_SUBTYPE_DEFAULT,
        );

        assert_eq!(response, vec![0x02, 0x00, 0x72]);
    }

    #[test]
    fn idle_state_canonical_selection_decodes_preferred_control_channel_cycle() {
        let request = [0x04, 0x00, 0x84, 0x80, 0x0a];
        let response = [0x02, 0x00, 0x84];
        let canonical = canonical_configuration_selection(&request, &response);

        assert_eq!(canonical, vec![0x03, 0x00, 0x80, 0x0a]);
        assert_eq!(
            selected_idle_preferred_control_channel_cycle(&canonical),
            Some(10)
        );
    }

    #[test]
    fn canonical_configuration_selection_ignores_complex_value_id() {
        let request_a = [0x04, 0x00, 0x29, 0x80, 0x0a];
        let response_a = [0x02, 0x00, 0x29];
        let request_b = [0x04, 0x00, 0x2f, 0x80, 0x0a];
        let response_b = [0x02, 0x00, 0x2f];

        assert_eq!(
            canonical_configuration_selection(&request_a, &response_a),
            vec![0x03, 0x00, 0x80, 0x0a]
        );
        assert_eq!(
            canonical_configuration_selection(&request_a, &response_a),
            canonical_configuration_selection(&request_b, &response_b)
        );
    }

    #[test]
    fn canonical_stream_configuration_selection_uses_selected_record() {
        let request_a = [
            0x1c, 0x00, 0x00, 0x00, 0x00, 0xff, 0xff, 0x00, 0x09, 0xff, 0xff, 0x01, 0x00, 0x00,
            0xff, 0xff, 0x00, 0x05, 0xff, 0xff, 0x02, 0x00, 0x00, 0xff, 0xff, 0x00, 0x02, 0xff,
            0xff,
        ];
        let request_b = [
            0x1c, 0x00, 0x20, 0x00, 0x00, 0xff, 0xff, 0x00, 0x09, 0xff, 0xff, 0x21, 0x00, 0x00,
            0xff, 0xff, 0x00, 0x05, 0xff, 0xff, 0x22, 0x00, 0x00, 0xff, 0xff, 0x00, 0x02, 0xff,
            0xff,
        ];
        let response_a = [0x02, 0x00, 0x02];
        let response_b = [0x02, 0x00, 0x22];

        assert_eq!(
            canonical_stream_configuration_selection(&request_a, &response_a),
            vec![0x09, 0x00, 0x00, 0x00, 0xff, 0xff, 0x00, 0x02, 0xff, 0xff]
        );
        assert_eq!(
            canonical_stream_configuration_selection(&request_a, &response_a),
            canonical_stream_configuration_selection(&request_b, &response_b)
        );
    }

    #[test]
    fn route_update_configuration_selects_supported_cdma_channels() {
        let request = [
            0x0b, 0x04, 0x00, 0x02, 0x00, 0x20, 0x00, 0x08, 0x10, 0x18, 0x40, 0x00,
        ];

        let response = configuration_response_attributes(
            DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE,
            &request,
            SESSION_SUBTYPE_DEFAULT,
        );

        assert_eq!(response, vec![0x02, 0x04, 0x00]);
    }

    #[test]
    fn multimode_capability_discovery_selects_offered_simple_attribute() {
        let request = [0x02, 0xfd, 0x01];

        let response = configuration_response_attributes(
            SESSION_PROTOCOL_MULTIMODE_CAPABILITY_DISCOVERY,
            &request,
            SESSION_SUBTYPE_DEFAULT,
        );

        assert_eq!(response, vec![0x02, 0xfd, 0x01]);
    }

    #[test]
    fn multimode_capability_discovery_skips_reserved_values() {
        let request = [0x03, 0xfb, 0x7f, 0x01];

        let response = configuration_response_attributes(
            SESSION_PROTOCOL_MULTIMODE_CAPABILITY_DISCOVERY,
            &request,
            SESSION_SUBTYPE_DEFAULT,
        );

        assert_eq!(response, vec![0x02, 0xfb, 0x01]);
    }

    #[test]
    fn uati_request_from_access_indication_allocates_session_uati() {
        let mut controller = HrpdAirController::new(26);
        let mut allocator = allocator();
        let ind = indication(vec![HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
            transaction_id: 0x9c,
        })]);

        let out = controller
            .handle_access_indication(&ind, &mut allocator)
            .unwrap();
        assert_eq!(out.session_outbound.len(), 1);
        let assigned = match out.session_outbound[0] {
            OutboundSessionMessage::UatiAssignment(u) => u,
            _ => panic!("expected UATI assignment"),
        };
        assert_eq!(assigned.as_u32(), 0x0005_8001);
        assert_eq!(out.forward_signaling.len(), 2);
        assert_eq!(acack(&out).payload, vec![0x00]);
        assert_eq!(acack(&out).target_ati, ind.ati);
        let assignment = signaling_for(&out, DEFAULT_ADDRESS_MANAGEMENT_PROTOCOL_TYPE);
        assert_eq!(
            assignment.protocol_type,
            DEFAULT_ADDRESS_MANAGEMENT_PROTOCOL_TYPE
        );
        assert_eq!(assignment.uati, Some(0x0005_8001));
        assert_eq!(assignment.channel, HrpdForwardChannel::AsynchronousControl);
        assert_eq!(
            assignment.target_ati,
            AccessTerminalIdentifier {
                ati_type: AccessTerminalIdentifierType::Rati,
                value: 0x5232_af53,
            }
        );
        assert_eq!(
            assignment.payload,
            vec![0x01, 0x00, 0x00, 0x1a, 0x05, 0x80, 0x01, 0x00]
        );
        assert_eq!(controller.session().state(), SessionState::AmpSetup);
    }

    #[test]
    fn repeated_uati_request_retransmits_assignment_sequence() {
        let mut controller = HrpdAirController::new(26);
        let mut allocator = allocator();
        let request = indication(vec![HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
            transaction_id: 0x9c,
        })]);

        let first = controller
            .handle_access_indication(&request, &mut allocator)
            .unwrap();
        let retry = controller
            .handle_access_indication(&request, &mut allocator)
            .unwrap();

        assert_eq!(
            signaling_for(&first, DEFAULT_ADDRESS_MANAGEMENT_PROTOCOL_TYPE).payload[1],
            0
        );
        assert_eq!(
            signaling_for(&retry, DEFAULT_ADDRESS_MANAGEMENT_PROTOCOL_TYPE).payload[1],
            0
        );
        let complete = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::UatiComplete(HrpdUatiComplete {
                    message_sequence: 0,
                    upper_old_uati: Vec::new(),
                    reserved_zero: true,
                })]),
                &mut allocator,
            )
            .unwrap();
        assert_eq!(complete.uati_completes.len(), 1);
        assert_eq!(controller.session().state(), SessionState::Open);
    }

    #[test]
    fn matching_uati_complete_opens_session() {
        let mut controller = HrpdAirController::new(26);
        let mut allocator = allocator();
        let _ = controller
            .handle_access_indication(
                &indication(vec![HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
                    transaction_id: 0x9c,
                })]),
                &mut allocator,
            )
            .unwrap();

        let out = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::UatiComplete(HrpdUatiComplete {
                    message_sequence: 0,
                    upper_old_uati: Vec::new(),
                    reserved_zero: true,
                })]),
                &mut allocator,
            )
            .unwrap();

        assert_eq!(out.forward_signaling.len(), 2);
        let hardware_id_request = signaling_for(&out, DEFAULT_ADDRESS_MANAGEMENT_PROTOCOL_TYPE);
        assert_eq!(
            hardware_id_request.protocol_type,
            DEFAULT_ADDRESS_MANAGEMENT_PROTOCOL_TYPE
        );
        assert_eq!(hardware_id_request.payload, vec![0x03, 0x01]);
        assert_eq!(
            hardware_id_request.target_ati,
            AccessTerminalIdentifier {
                ati_type: AccessTerminalIdentifierType::Uati,
                value: 0x1a05_8001,
            }
        );
        assert_eq!(out.uati_completes.len(), 1);
        assert_eq!(controller.session().state(), SessionState::Open);
    }

    #[test]
    fn uati_scoped_connection_request_waits_for_uati_complete_before_traffic_assignment() {
        let mut controller = HrpdAirController::new(26);
        let mut allocator = allocator();
        let _ = controller
            .handle_access_indication(
                &indication(vec![HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
                    transaction_id: 0x9c,
                })]),
                &mut allocator,
            )
            .unwrap();

        let mut request = indication(vec![HrpdAccessMessage::ConnectionRequest(
            HrpdConnectionRequest {
                transaction_id: 0x56,
                request_reason: 0,
                reserved_zero: true,
            },
        )]);
        request.ati = AccessTerminalIdentifier {
            ati_type: AccessTerminalIdentifierType::Uati,
            value: 0x1a05_8001,
        };

        let out = controller
            .handle_access_indication(&request, &mut allocator)
            .unwrap();

        assert_eq!(out.forward_signaling.len(), 2);
        assert_eq!(acack(&out).target_ati, request.ati);
        let retransmit = signaling_for(&out, DEFAULT_ADDRESS_MANAGEMENT_PROTOCOL_TYPE);
        assert_eq!(retransmit.target_ati, request.ati);
        assert_eq!(retransmit.channel, HrpdForwardChannel::AsynchronousControl);
        assert_eq!(retransmit.payload[0], 0x01);
        assert_eq!(retransmit.payload[1], 0);
        assert_eq!(out.uati_completes.len(), 0);
        assert_eq!(out.traffic_assignments.len(), 0);
        assert_eq!(controller.session().state(), SessionState::AmpSetup);

        let out = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::UatiComplete(HrpdUatiComplete {
                    message_sequence: 0,
                    upper_old_uati: Vec::new(),
                    reserved_zero: true,
                })]),
                &mut allocator,
            )
            .unwrap();

        assert_eq!(out.uati_completes.len(), 1);
        assert_eq!(controller.session().state(), SessionState::Open);
        assert_eq!(out.traffic_assignments.len(), 1);
        assert_eq!(out.traffic_assignments[0].uati, 0x1a05_8001);
        assert_eq!(out.traffic_assignments[0].mac_index, 5);
    }

    #[test]
    fn stale_cached_uati_connection_request_without_session_gets_session_lost_close() {
        let mut controller = HrpdAirController::new(26);
        let mut allocator = allocator();
        let mut request = indication(vec![HrpdAccessMessage::ConnectionRequest(
            HrpdConnectionRequest {
                transaction_id: 0x69,
                request_reason: 0,
                reserved_zero: true,
            },
        )]);
        request.ati = AccessTerminalIdentifier {
            ati_type: AccessTerminalIdentifierType::Uati,
            value: 0x1a05_8001,
        };

        let out = controller
            .handle_access_indication(&request, &mut allocator)
            .unwrap();

        assert_eq!(controller.session().state(), SessionState::Closed);
        assert!(controller.session().session().is_none());
        assert_eq!(allocator.issued_count(), 0);
        assert_eq!(out.connection_requests.len(), 1);
        assert_eq!(out.traffic_assignments.len(), 0);
        assert_eq!(out.forward_signaling.len(), 2);
        assert_eq!(acack(&out).target_ati, request.ati);
        let session_close = signaling_for(&out, DEFAULT_SESSION_MANAGEMENT_PROTOCOL_TYPE);
        assert_eq!(session_close.target_ati, request.ati);
        assert_eq!(session_close.uati, None);
        assert_eq!(
            session_close.channel,
            HrpdForwardChannel::AsynchronousControl
        );
        assert_eq!(
            session_close.payload,
            vec![0x01, SESSION_CLOSE_REASON_SESSION_LOST, 0x00]
        );
        assert_eq!(out.forward_traffic.len(), 0);
    }

    #[test]
    fn stale_cached_uati_route_update_without_session_gets_session_lost_close() {
        let mut controller = HrpdAirController::new(26);
        let mut allocator = allocator();
        let route = AirRouteUpdate {
            message_sequence: 8,
            reference_pilot_pn: 0,
            reference_pilot_strength: 0,
            reference_keep: true,
            num_pilots: 0,
            at_total_pilot_transmission: None,
            reference_pilot_channel: None,
            reserved_zero: true,
        };
        let mut request = indication(vec![HrpdAccessMessage::RouteUpdate(route.clone())]);
        request.ati = AccessTerminalIdentifier {
            ati_type: AccessTerminalIdentifierType::Uati,
            value: 0x1a05_8001,
        };

        let out = controller
            .handle_access_indication(&request, &mut allocator)
            .unwrap();

        assert_eq!(controller.session().state(), SessionState::Closed);
        assert!(controller.session().session().is_none());
        assert_eq!(allocator.issued_count(), 0);
        assert_eq!(out.route_updates, vec![route]);
        assert_eq!(out.traffic_assignments.len(), 0);
        assert_eq!(out.forward_signaling.len(), 2);
        assert_eq!(acack(&out).target_ati, request.ati);
        let session_close = signaling_for(&out, DEFAULT_SESSION_MANAGEMENT_PROTOCOL_TYPE);
        assert_eq!(session_close.target_ati, request.ati);
        assert_eq!(
            session_close.payload,
            vec![0x01, SESSION_CLOSE_REASON_SESSION_LOST, 0x00]
        );
        assert_eq!(out.forward_traffic.len(), 0);
    }

    #[test]
    fn uati_scoped_reconnect_reuses_existing_uati_after_normal_connection_close() {
        let mut controller = HrpdAirController::new(26);
        let mut allocator = allocator();
        let _ = controller
            .handle_access_indication(
                &indication(vec![HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
                    transaction_id: 0x9c,
                })]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::UatiComplete(HrpdUatiComplete {
                    message_sequence: 0,
                    upper_old_uati: Vec::new(),
                    reserved_zero: true,
                })]),
                &mut allocator,
            )
            .unwrap();

        let mut initial_request = indication(vec![HrpdAccessMessage::ConnectionRequest(
            HrpdConnectionRequest {
                transaction_id: 0x21,
                request_reason: 0,
                reserved_zero: true,
            },
        )]);
        initial_request.ati = AccessTerminalIdentifier {
            ati_type: AccessTerminalIdentifierType::Uati,
            value: 0x1a05_8001,
        };
        let initial = controller
            .handle_access_indication(&initial_request, &mut allocator)
            .unwrap();
        assert_eq!(initial.traffic_assignments[0].uati, 0x1a05_8001);

        let close_packet = encode_reliable_default_signaling_packet(
            DEFAULT_CONNECTED_STATE_PROTOCOL_TYPE,
            &[0x00, 0x00],
            0,
        );
        let close = controller.handle_traffic_event(&HrpdTrafficEvent::Stream0Signaling {
            uati: 0x1a05_8001,
            payload: close_packet,
        });
        assert_eq!(close.traffic_channel_closed_uatis, vec![0x1a05_8001]);
        assert_eq!(close.traffic_releases.len(), 1);
        assert_eq!(close.traffic_releases[0].uati, 0x1a05_8001);

        let mut stray_uati_request =
            indication(vec![HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
                transaction_id: 0x9d,
            })]);
        stray_uati_request.ati = AccessTerminalIdentifier {
            ati_type: AccessTerminalIdentifierType::Uati,
            value: 0x1a05_8001,
        };
        let stray = controller
            .handle_access_indication(&stray_uati_request, &mut allocator)
            .unwrap();
        assert_eq!(
            signaling_for(&stray, DEFAULT_ADDRESS_MANAGEMENT_PROTOCOL_TYPE).uati,
            Some(0x0005_8001)
        );
        assert_eq!(allocator.issued_count(), 1);

        let mut reconnect = indication(vec![HrpdAccessMessage::ConnectionRequest(
            HrpdConnectionRequest {
                transaction_id: 0x22,
                request_reason: 1,
                reserved_zero: true,
            },
        )]);
        reconnect.ati = AccessTerminalIdentifier {
            ati_type: AccessTerminalIdentifierType::Uati,
            value: 0x1a05_8001,
        };
        let out = controller
            .handle_access_indication(&reconnect, &mut allocator)
            .unwrap();

        assert_eq!(out.traffic_assignments.len(), 0);
        let out = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::UatiComplete(HrpdUatiComplete {
                    message_sequence: 1,
                    upper_old_uati: Vec::new(),
                    reserved_zero: true,
                })]),
                &mut allocator,
            )
            .unwrap();

        assert_eq!(
            controller.session().session().unwrap().uati.as_u32(),
            0x0005_8001
        );
        assert_eq!(out.traffic_assignments.len(), 1);
        assert_eq!(out.traffic_assignments[0].uati, 0x1a05_8001);
        assert_eq!(
            signaling_for(&out, DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE).target_ati,
            AccessTerminalIdentifier {
                ati_type: AccessTerminalIdentifierType::Uati,
                value: 0x1a05_8001,
            }
        );
    }

    #[test]
    fn hardware_id_response_is_preserved_after_request() {
        let mut controller = HrpdAirController::new(26);
        let mut allocator = allocator();
        let _ = controller
            .handle_access_indication(
                &indication(vec![HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
                    transaction_id: 0x9c,
                })]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::UatiComplete(HrpdUatiComplete {
                    message_sequence: 0,
                    upper_old_uati: Vec::new(),
                    reserved_zero: true,
                })]),
                &mut allocator,
            )
            .unwrap();

        let hardware = HrpdHardwareIdResponse {
            transaction_id: 1,
            hardware_id_type: 0x00ff_ff,
            hardware_id_value: vec![0xa1, 0xb2, 0xc3, 0xd4, 0xe5, 0xf6, 0x70],
        };
        let out = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::HardwareIdResponse(
                    hardware.clone(),
                )]),
                &mut allocator,
            )
            .unwrap();

        assert_eq!(out.hardware_id_responses, vec![hardware.clone()]);
        assert_eq!(
            controller.hardware_id_for_uati(0x0005_8001),
            Some(&hardware)
        );
    }

    #[test]
    fn access_indication_preserves_route_update_and_connection_request() {
        let mut controller = HrpdAirController::with_sector(
            26,
            0,
            Some(cdma_common::hrpd::air::HrpdChannelRecord {
                system_type: 0,
                band_class: 0,
                channel_number: 630,
            }),
        );
        let mut allocator = allocator();
        let _ = controller
            .handle_access_indication(
                &indication(vec![HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
                    transaction_id: 0x9c,
                })]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::UatiComplete(HrpdUatiComplete {
                    message_sequence: 0,
                    upper_old_uati: Vec::new(),
                    reserved_zero: true,
                })]),
                &mut allocator,
            )
            .unwrap();
        let route = AirRouteUpdate {
            message_sequence: 1,
            reference_pilot_pn: 0,
            reference_pilot_strength: 0,
            reference_keep: true,
            num_pilots: 0,
            at_total_pilot_transmission: None,
            reference_pilot_channel: None,
            reserved_zero: true,
        };
        let ind = uati_indication(vec![
            HrpdAccessMessage::RouteUpdate(route.clone()),
            HrpdAccessMessage::ConnectionRequest(HrpdConnectionRequest {
                transaction_id: 0x44,
                request_reason: 0,
                reserved_zero: true,
            }),
        ]);

        let out = controller
            .handle_access_indication(&ind, &mut allocator)
            .unwrap();
        assert_eq!(out.route_updates, vec![route]);
        assert!(out.connection_requested);
        assert_eq!(out.connection_requests.len(), 1);
        assert_eq!(out.forward_signaling.len(), 2);
        let assignment = signaling_for(&out, DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE);
        assert_eq!(assignment.protocol_type, DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE);
        assert_eq!(
            assignment.target_ati,
            AccessTerminalIdentifier {
                ati_type: AccessTerminalIdentifierType::Uati,
                value: 0x1a05_8001,
            }
        );
        assert_eq!(assignment.payload[0], 0x01);
        assert_eq!(assignment.payload[1], 0x00);
        assert_eq!(assignment.payload.len(), 11);
        assert_eq!(out.traffic_assignments.len(), 1);
        assert_eq!(out.traffic_assignments[0].uati, 0x1a05_8001);
        assert_eq!(out.traffic_assignments[0].mac_index, 5);
        assert_eq!(out.traffic_assignments[0].reverse_rate_limit_bps, 153_600);
        let (expected_i, expected_q) = default_reverse_traffic_long_code_masks(0x1a05_8001);
        assert_eq!(
            out.traffic_assignments[0].reverse_long_code_mask_i,
            expected_i
        );
        assert_eq!(
            out.traffic_assignments[0].reverse_long_code_mask_q,
            expected_q
        );
        assert!(out.traffic_assignments[0].drc_lock);
        assert_eq!(out.forward_traffic.len(), 1);
    }

    #[test]
    fn repeated_connection_request_retransmits_same_assignment_without_reinstall() {
        let mut controller = HrpdAirController::with_sector(26, 0, None);
        let mut allocator = allocator();
        let _ = controller
            .handle_access_indication(
                &indication(vec![HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
                    transaction_id: 0x9c,
                })]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::UatiComplete(HrpdUatiComplete {
                    message_sequence: 0,
                    upper_old_uati: Vec::new(),
                    reserved_zero: true,
                })]),
                &mut allocator,
            )
            .unwrap();
        let request = HrpdAccessMessage::ConnectionRequest(HrpdConnectionRequest {
            transaction_id: 0x21,
            request_reason: 0,
            reserved_zero: true,
        });

        let first = controller
            .handle_access_indication(&uati_indication(vec![request.clone()]), &mut allocator)
            .unwrap();
        let retry = controller
            .handle_access_indication(&uati_indication(vec![request]), &mut allocator)
            .unwrap();

        assert_eq!(first.traffic_assignments.len(), 1);
        assert_eq!(first.traffic_assignments[0].mac_index, 5);
        assert_ne!(first.traffic_assignments[0].reverse_long_code_mask_i, 0);
        assert_ne!(first.traffic_assignments[0].reverse_long_code_mask_q, 0);
        assert_eq!(retry.traffic_assignments.len(), 0);
        assert_eq!(retry.traffic_releases.len(), 0);
        assert_eq!(retry.forward_signaling.len(), 2);
        let first_tca = signaling_for(&first, DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE);
        let retry_tca = signaling_for(&retry, DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE);
        assert_eq!(retry_tca.payload, first_tca.payload);
        assert_eq!(retry_tca.uati, first_tca.uati);
        // C.S0024-400 §1.8.6.2.3 note 41: the TCA sent in response to
        // Open is best-effort SLP; later connected-state TCAs use reliable
        // delivery. Retrying the pending idle TCA keeps the same best-effort
        // wire image and preserves Stream 0 V(S) for RTCAck.
        assert_eq!(first_tca.reliable_sequence, None);
        assert_eq!(retry_tca.reliable_sequence, None);
        assert_eq!(first.forward_traffic.len(), 1);
        assert_eq!(retry.forward_traffic.len(), 0);
    }

    #[test]
    fn pending_setup_retry_with_new_transaction_retransmits_same_assignment() {
        let mut controller = HrpdAirController::with_sector(26, 0, None);
        let mut allocator = allocator();
        let _ = controller
            .handle_access_indication(
                &indication(vec![HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
                    transaction_id: 0x9c,
                })]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::UatiComplete(HrpdUatiComplete {
                    message_sequence: 0,
                    upper_old_uati: Vec::new(),
                    reserved_zero: true,
                })]),
                &mut allocator,
            )
            .unwrap();

        let first = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::ConnectionRequest(
                    HrpdConnectionRequest {
                        transaction_id: 0x21,
                        request_reason: 0,
                        reserved_zero: true,
                    },
                )]),
                &mut allocator,
            )
            .unwrap();
        let retry = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::ConnectionRequest(
                    HrpdConnectionRequest {
                        transaction_id: 0x22,
                        request_reason: 0,
                        reserved_zero: true,
                    },
                )]),
                &mut allocator,
            )
            .unwrap();

        assert_eq!(first.traffic_assignments.len(), 1);
        assert_eq!(first.traffic_assignments[0].mac_index, 5);
        assert_eq!(retry.traffic_assignments.len(), 0);
        assert_eq!(retry.traffic_releases.len(), 0);
        assert_eq!(retry.forward_signaling.len(), 2);
        let first_tca = signaling_for(&first, DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE);
        let retry_tca = signaling_for(&retry, DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE);
        assert_eq!(retry_tca.payload, first_tca.payload);
        assert_eq!(retry_tca.uati, first_tca.uati);
        assert_eq!(first_tca.reliable_sequence, None);
        assert_eq!(retry_tca.reliable_sequence, None);
        assert_eq!(first.forward_traffic.len(), 1);
        assert_eq!(retry.forward_traffic.len(), 0);
    }

    #[test]
    fn session_close_releases_traffic_assignment() {
        let mut controller = HrpdAirController::with_sector(26, 0, None);
        let mut allocator = allocator();
        let _ = controller
            .handle_access_indication(
                &indication(vec![HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
                    transaction_id: 0x9c,
                })]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::UatiComplete(HrpdUatiComplete {
                    message_sequence: 0,
                    upper_old_uati: Vec::new(),
                    reserved_zero: true,
                })]),
                &mut allocator,
            )
            .unwrap();
        let assigned = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::ConnectionRequest(
                    HrpdConnectionRequest {
                        transaction_id: 0x21,
                        request_reason: 0,
                        reserved_zero: true,
                    },
                )]),
                &mut allocator,
            )
            .unwrap();
        assert_eq!(assigned.traffic_assignments.len(), 1);
        let assignment = &assigned.traffic_assignments[0];

        let close = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::SessionClose(HrpdSessionClose {
                    close_reason: 2,
                    more_info: Vec::new(),
                })]),
                &mut allocator,
            )
            .unwrap();
        assert_eq!(
            close.traffic_releases,
            vec![HrpdTrafficReleaseRequest {
                uati: assignment.uati,
                mac_index: assignment.mac_index,
            }],
            "SessionClose must release the pending traffic assignment"
        );

        // A second close has nothing left to release.
        let again = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::SessionClose(HrpdSessionClose {
                    close_reason: 2,
                    more_info: Vec::new(),
                })]),
                &mut allocator,
            )
            .unwrap();
        assert!(again.traffic_releases.is_empty());
    }

    #[test]
    fn session_close_emits_forward_close_and_releases_session() {
        let mut controller = HrpdAirController::with_sector(26, 0, None);
        let mut allocator = allocator();
        let _ = controller
            .handle_access_indication(
                &indication(vec![HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
                    transaction_id: 0x9c,
                })]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::UatiComplete(HrpdUatiComplete {
                    message_sequence: 0,
                    upper_old_uati: Vec::new(),
                    reserved_zero: true,
                })]),
                &mut allocator,
            )
            .unwrap();

        let close = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::SessionClose(HrpdSessionClose {
                    close_reason: 2,
                    more_info: Vec::new(),
                })]),
                &mut allocator,
            )
            .unwrap();

        assert_eq!(controller.session().state(), SessionState::Closed);
        assert_eq!(allocator.issued_count(), 0);
        assert_eq!(close.session_closes.len(), 1);
        assert_eq!(close.session_outbound.len(), 1);
        assert_eq!(close.forward_signaling.len(), 2);
        let close_msg = signaling_for(&close, DEFAULT_SESSION_MANAGEMENT_PROTOCOL_TYPE);
        assert_eq!(
            close_msg.protocol_type,
            DEFAULT_SESSION_MANAGEMENT_PROTOCOL_TYPE
        );
        assert_eq!(close_msg.payload, vec![0x01, 0x00, 0x00]);

        // After SessionClose the AT discards its UATI and re-registers under a
        // RATI, starting a brand-new session (the closed one is dead).
        let reacquire = controller
            .handle_access_indication(
                &indication(vec![HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
                    transaction_id: 0x9d,
                })]),
                &mut allocator,
            )
            .unwrap();
        let new_uati = match reacquire.session_outbound[0] {
            OutboundSessionMessage::UatiAssignment(u) => u.as_u32(),
            _ => panic!("expected fresh UATI assignment after session close"),
        };
        assert_eq!(
            controller.session_for_uati(new_uati).unwrap().state(),
            SessionState::AmpSetup
        );
        assert_eq!(reacquire.forward_signaling.len(), 2);
    }

    #[test]
    fn reverse_stream0_session_close_releases_session_and_allocator() {
        let mut controller = HrpdAirController::with_sector(26, 0, None);
        let mut allocator = allocator();
        let _ = controller
            .handle_access_indication(
                &indication(vec![HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
                    transaction_id: 0x9c,
                })]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::UatiComplete(HrpdUatiComplete {
                    message_sequence: 0,
                    upper_old_uati: Vec::new(),
                    reserved_zero: true,
                })]),
                &mut allocator,
            )
            .unwrap();
        let assigned = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::ConnectionRequest(
                    HrpdConnectionRequest {
                        transaction_id: 0x21,
                        request_reason: 0,
                        reserved_zero: true,
                    },
                )]),
                &mut allocator,
            )
            .unwrap();
        let traffic_uati = assigned.traffic_assignments[0].uati;

        let close_packet = encode_reliable_default_signaling_packet(
            DEFAULT_SESSION_MANAGEMENT_PROTOCOL_TYPE,
            &[0x01, 0x02, 0x00],
            0,
        );
        let close = controller.handle_traffic_event_with_allocator(
            &HrpdTrafficEvent::Stream0Signaling {
                uati: traffic_uati,
                payload: close_packet,
            },
            &mut allocator,
        );

        assert_eq!(close.session_closed_uatis, vec![traffic_uati]);
        assert_eq!(close.traffic_channel_closed_uatis, vec![traffic_uati]);
        assert_eq!(close.traffic_releases.len(), 1);
        assert_eq!(close.traffic_releases[0].uati, traffic_uati);
        assert_eq!(allocator.issued_count(), 0);
        assert!(controller.session_for_uati(0x0005_8001).is_none());
    }

    #[test]
    fn idle_timer_closes_and_reclaims_inactive_session() {
        let mut controller = HrpdAirController::with_sector(26, 0, None);
        let mut allocator = allocator();
        let _ = controller
            .handle_access_indication(
                &indication(vec![HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
                    transaction_id: 0x9c,
                })]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::UatiComplete(HrpdUatiComplete {
                    message_sequence: 0,
                    upper_old_uati: Vec::new(),
                    reserved_zero: true,
                })]),
                &mut allocator,
            )
            .unwrap();
        let last_activity_at = controller
            .ats
            .get(&0x0005_8001)
            .expect("session should be tracked")
            .last_activity_at;

        let early = controller.handle_timer_with_allocator(
            last_activity_at + HRPD_SESSION_IDLE_TIMEOUT - Duration::from_millis(1),
            &mut allocator,
        );
        assert!(early.session_closed_uatis.is_empty());
        assert_eq!(allocator.issued_count(), 1);

        let expired = controller.handle_timer_with_allocator(
            last_activity_at + HRPD_SESSION_IDLE_TIMEOUT,
            &mut allocator,
        );
        assert_eq!(expired.session_closed_uatis, vec![0x1a05_8001]);
        assert_eq!(expired.forward_signaling.len(), 1);
        assert_eq!(
            expired.forward_signaling[0].protocol_type,
            DEFAULT_SESSION_MANAGEMENT_PROTOCOL_TYPE
        );
        assert_eq!(
            expired.forward_signaling[0].payload,
            vec![0x01, SESSION_CLOSE_REASON_SESSION_LOST, 0x00]
        );
        assert_eq!(allocator.issued_count(), 0);
        assert!(controller.session_for_uati(0x0005_8001).is_none());
    }

    #[test]
    fn session_close_cancels_same_access_connection_request_assignment() {
        let mut controller = HrpdAirController::with_sector(26, 0, None);
        let mut allocator = allocator();
        let _ = controller
            .handle_access_indication(
                &indication(vec![HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
                    transaction_id: 0x9c,
                })]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::UatiComplete(HrpdUatiComplete {
                    message_sequence: 0,
                    upper_old_uati: Vec::new(),
                    reserved_zero: true,
                })]),
                &mut allocator,
            )
            .unwrap();

        let mut close_with_request = indication(vec![
            HrpdAccessMessage::ConnectionRequest(HrpdConnectionRequest {
                transaction_id: 0x7c,
                request_reason: 1,
                reserved_zero: true,
            }),
            HrpdAccessMessage::SessionClose(HrpdSessionClose {
                close_reason: 2,
                more_info: Vec::new(),
            }),
        ]);
        close_with_request.ati = AccessTerminalIdentifier {
            ati_type: AccessTerminalIdentifierType::Uati,
            value: 0x1a05_8001,
        };

        let out = controller
            .handle_access_indication(&close_with_request, &mut allocator)
            .unwrap();

        assert_eq!(controller.session().state(), SessionState::Closed);
        assert_eq!(out.session_closed_uatis, vec![0x1a05_8001]);
        assert!(out.traffic_assignments.is_empty());
        assert!(out.forward_traffic.is_empty());
        assert_eq!(
            out.forward_signaling
                .iter()
                .filter(
                    |request| request.protocol_type == DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE
                        && request.payload.first().copied()
                            == Some(HrpdTrafficChannelAssignment::MESSAGE_ID)
                )
                .count(),
            0
        );
        assert_eq!(
            out.traffic_releases,
            vec![HrpdTrafficReleaseRequest {
                uati: 0x1a05_8001,
                mac_index: 5,
            }]
        );
    }

    #[test]
    fn repeated_rati_uati_request_retransmits_same_assignment_sequence() {
        let mut controller = HrpdAirController::with_sector(26, 0, None);
        let mut allocator = allocator();

        let first = controller
            .handle_access_indication(
                &indication(vec![HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
                    transaction_id: 0x9c,
                })]),
                &mut allocator,
            )
            .unwrap();
        let second = controller
            .handle_access_indication(
                &indication(vec![HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
                    transaction_id: 0x9c,
                })]),
                &mut allocator,
            )
            .unwrap();

        assert_eq!(first.forward_signaling.len(), 2);
        assert_eq!(second.forward_signaling.len(), 2);
        assert_eq!(
            signaling_for(&first, DEFAULT_ADDRESS_MANAGEMENT_PROTOCOL_TYPE).payload[1],
            0
        );
        assert_eq!(
            signaling_for(&second, DEFAULT_ADDRESS_MANAGEMENT_PROTOCOL_TYPE).payload[1],
            0
        );
    }

    #[test]
    fn new_rati_uati_transaction_advances_assignment_sequence() {
        let mut controller = HrpdAirController::with_sector(26, 0, None);
        let mut allocator = allocator();

        let first = controller
            .handle_access_indication(
                &indication(vec![HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
                    transaction_id: 0x9c,
                })]),
                &mut allocator,
            )
            .unwrap();
        let second = controller
            .handle_access_indication(
                &indication(vec![HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
                    transaction_id: 0x9d,
                })]),
                &mut allocator,
            )
            .unwrap();

        assert_eq!(first.forward_signaling.len(), 2);
        assert_eq!(second.forward_signaling.len(), 2);
        assert_eq!(
            signaling_for(&first, DEFAULT_ADDRESS_MANAGEMENT_PROTOCOL_TYPE).payload[1],
            0
        );
        assert_eq!(
            signaling_for(&second, DEFAULT_ADDRESS_MANAGEMENT_PROTOCOL_TYPE).payload[1],
            1
        );
    }

    #[test]
    fn two_access_terminals_hold_concurrent_sessions_and_traffic() {
        let mut controller = HrpdAirController::with_sector(26, 0, None);
        let mut allocator = allocator();

        let rati_a = AccessTerminalIdentifier {
            ati_type: AccessTerminalIdentifierType::Rati,
            value: 0x5232_af53,
        };
        let rati_b = AccessTerminalIdentifier {
            ati_type: AccessTerminalIdentifierType::Rati,
            value: 0x5232_af54,
        };

        // Both ATs request a UATI and get distinct assignments.
        let a = controller
            .handle_access_indication(
                &indication_with_ati(
                    rati_a,
                    vec![HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
                        transaction_id: 0x9c,
                    })],
                ),
                &mut allocator,
            )
            .unwrap();
        let b = controller
            .handle_access_indication(
                &indication_with_ati(
                    rati_b,
                    vec![HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
                        transaction_id: 0x9c,
                    })],
                ),
                &mut allocator,
            )
            .unwrap();
        let uati_a = match a.session_outbound[0] {
            OutboundSessionMessage::UatiAssignment(u) => u.as_u32(),
            _ => panic!("expected UATI assignment for AT A"),
        };
        let uati_b = match b.session_outbound[0] {
            OutboundSessionMessage::UatiAssignment(u) => u.as_u32(),
            _ => panic!("expected UATI assignment for AT B"),
        };
        assert_ne!(uati_a, uati_b);
        assert_eq!(uati_a, 0x0005_8001);
        assert_eq!(uati_b, 0x0005_8002);

        // Each AT accepts its UATIAssignment and from then on addresses every
        // access message by its UATI (C.S0024-0 §5.3.7.1.5.1).
        let access_uati_a = AccessTerminalIdentifier {
            ati_type: AccessTerminalIdentifierType::Uati,
            value: 0x1a05_8001,
        };
        let access_uati_b = AccessTerminalIdentifier {
            ati_type: AccessTerminalIdentifierType::Uati,
            value: 0x1a05_8002,
        };
        for (access_uati, seq) in [(access_uati_a, 0), (access_uati_b, 0)] {
            controller
                .handle_access_indication(
                    &indication_with_ati(
                        access_uati,
                        vec![HrpdAccessMessage::UatiComplete(HrpdUatiComplete {
                            message_sequence: seq,
                            upper_old_uati: Vec::new(),
                            reserved_zero: true,
                        })],
                    ),
                    &mut allocator,
                )
                .unwrap();
        }

        // AT A opens traffic.
        let conn_a = controller
            .handle_access_indication(
                &indication_with_ati(
                    access_uati_a,
                    vec![HrpdAccessMessage::ConnectionRequest(
                        HrpdConnectionRequest {
                            transaction_id: 0x21,
                            request_reason: 0,
                            reserved_zero: true,
                        },
                    )],
                ),
                &mut allocator,
            )
            .unwrap();
        assert_eq!(conn_a.traffic_assignments.len(), 1);
        let mac_a = conn_a.traffic_assignments[0].mac_index;
        let traffic_uati_a = conn_a.traffic_assignments[0].uati;

        // AT B's ConnectionRequest must open its own traffic and must NOT
        // release AT A's assignment.
        let conn_b = controller
            .handle_access_indication(
                &indication_with_ati(
                    access_uati_b,
                    vec![HrpdAccessMessage::ConnectionRequest(
                        HrpdConnectionRequest {
                            transaction_id: 0x21,
                            request_reason: 0,
                            reserved_zero: true,
                        },
                    )],
                ),
                &mut allocator,
            )
            .unwrap();
        assert_eq!(conn_b.traffic_assignments.len(), 1);
        assert!(
            conn_b.traffic_releases.is_empty(),
            "AT B ConnectionRequest must not release AT A traffic"
        );
        let mac_b = conn_b.traffic_assignments[0].mac_index;
        let traffic_uati_b = conn_b.traffic_assignments[0].uati;

        assert_ne!(traffic_uati_a, traffic_uati_b);
        assert_ne!(mac_a, mac_b);
        assert_eq!(traffic_uati_a, 0x1a05_8001);
        assert_eq!(traffic_uati_b, 0x1a05_8002);

        // Both sessions coexist and both hold a pending traffic assignment.
        assert!(controller.session_for_uati(uati_a).is_some());
        assert!(controller.session_for_uati(uati_b).is_some());
        assert_eq!(
            controller
                .ats
                .get(&uati_a)
                .unwrap()
                .session
                .session()
                .unwrap()
                .uati
                .as_u32(),
            uati_a
        );
        assert!(
            controller
                .ats
                .get(&uati_a)
                .unwrap()
                .pending_traffic_assignment
                .is_some()
        );
        assert!(
            controller
                .ats
                .get(&uati_b)
                .unwrap()
                .pending_traffic_assignment
                .is_some()
        );
    }

    #[test]
    fn reverse_pilot_acquisition_sends_rtc_ack_once() {
        let mut controller = HrpdAirController::with_sector(26, 0, None);
        let mut allocator = allocator();
        let _ = controller
            .handle_access_indication(
                &indication(vec![HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
                    transaction_id: 0x9c,
                })]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::UatiComplete(HrpdUatiComplete {
                    message_sequence: 0,
                    upper_old_uati: Vec::new(),
                    reserved_zero: true,
                })]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::ConnectionRequest(
                    HrpdConnectionRequest {
                        transaction_id: 0x21,
                        request_reason: 0,
                        reserved_zero: true,
                    },
                )]),
                &mut allocator,
            )
            .unwrap();

        let pilot = reverse_pilot(
            &mut controller,
            0x1a05_8001,
            5,
            initial_rtc_ack_release_chip(),
        );
        assert_eq!(pilot.accepted_event_count, 1);
        assert_eq!(pilot.reverse_pilot_count, 1);
        assert_eq!(pilot.forward_traffic.len(), 0);
        let first = drive_setup_drc(&mut controller, 0x1a05_8001, 5, 0, 0x2);
        assert_eq!(first.forward_traffic.len(), 1);
        assert_eq!(first.forward_traffic[0].uati, 0x1a05_8001);
        assert_eq!(first.forward_traffic[0].mac_index, 5);

        // Subsequent reverse-pilot events without a fresh ConnectionRequest do
        // not re-emit RTCAck; reliable retransmission is DRC-paced (RTCAck is
        // SLP Reliable, so the AN — not the AT — owns the resend loop).
        let second = controller.handle_traffic_event(&HrpdTrafficEvent::ReversePilot {
            uati: 0x1a05_8001,
            mac_index: 5,
            absolute_chip: initial_rtc_ack_release_chip() + HRPD_REVERSE_TRAFFIC_SLOT_CHIPS,
            snr_db_tenths: 70,
        });
        assert_eq!(second.accepted_event_count, 1);
        assert_eq!(second.forward_traffic.len(), 0);
    }

    #[test]
    fn initial_rtc_ack_waits_for_first_valid_drc() {
        let mut controller = HrpdAirController::with_sector(26, 0, None);
        let mut allocator = allocator();
        let _ = controller
            .handle_access_indication(
                &indication(vec![HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
                    transaction_id: 0x9c,
                })]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::UatiComplete(HrpdUatiComplete {
                    message_sequence: 0,
                    upper_old_uati: Vec::new(),
                    reserved_zero: true,
                })]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::ConnectionRequest(
                    HrpdConnectionRequest {
                        transaction_id: 0x21,
                        request_reason: 0,
                        reserved_zero: true,
                    },
                )]),
                &mut allocator,
            )
            .unwrap();

        let pilot = reverse_pilot(&mut controller, 0x1a05_8001, 5, 0);
        assert_eq!(pilot.forward_traffic.len(), 0);

        let unsupported = controller.handle_traffic_event(&HrpdTrafficEvent::Drc {
            uati: 0x1a05_8001,
            mac_index: 5,
            slot: 0,
            drc_index: 0xf,
        });
        assert_eq!(unsupported.forward_traffic.len(), 0);

        let first_valid = controller.handle_traffic_event(&HrpdTrafficEvent::Drc {
            uati: 0x1a05_8001,
            mac_index: 5,
            slot: 2,
            drc_index: 0x2,
        });
        assert_eq!(first_valid.forward_traffic.len(), 1);
    }

    #[test]
    fn pending_traffic_setup_expires_on_an_setup_timer() {
        let mut controller = HrpdAirController::with_sector(26, 0, None);
        let mut allocator = allocator();
        let _ = controller
            .handle_access_indication(
                &indication(vec![HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
                    transaction_id: 0x9c,
                })]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::UatiComplete(HrpdUatiComplete {
                    message_sequence: 0,
                    upper_old_uati: Vec::new(),
                    reserved_zero: true,
                })]),
                &mut allocator,
            )
            .unwrap();
        let connection = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::ConnectionRequest(
                    HrpdConnectionRequest {
                        transaction_id: 0x21,
                        request_reason: 0,
                        reserved_zero: true,
                    },
                )]),
                &mut allocator,
            )
            .unwrap();
        assert_eq!(connection.traffic_assignments.len(), 1);
        let setup_started_at = controller
            .pending_traffic_assignment()
            .unwrap()
            .setup_started_at;

        let early = controller.handle_timer(setup_started_at + HRPD_RTCMP_AN_SETUP / 2);
        assert!(early.traffic_releases.is_empty());
        assert!(controller.pending_traffic_assignment().is_some());

        let expired = controller.handle_timer(setup_started_at + HRPD_RTCMP_AN_SETUP);
        assert_eq!(
            expired.traffic_releases,
            vec![HrpdTrafficReleaseRequest {
                uati: 0x1a05_8001,
                mac_index: 5,
            }]
        );
        assert!(controller.pending_traffic_assignment().is_none());
    }

    #[test]
    fn late_reverse_pilot_acquisition_waits_for_first_drc() {
        let mut controller = HrpdAirController::with_sector(26, 0, None);
        let mut allocator = allocator();
        let _ = controller
            .handle_access_indication(
                &indication(vec![HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
                    transaction_id: 0x9c,
                })]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::UatiComplete(HrpdUatiComplete {
                    message_sequence: 0,
                    upper_old_uati: Vec::new(),
                    reserved_zero: true,
                })]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::ConnectionRequest(
                    HrpdConnectionRequest {
                        transaction_id: 0x21,
                        request_reason: 0,
                        reserved_zero: true,
                    },
                )]),
                &mut allocator,
            )
            .unwrap();

        let late_slot = HRPD_RTCMP_AN_SETUP_SLOTS + 1;
        let pilot = reverse_pilot(
            &mut controller,
            0x1a05_8001,
            5,
            late_slot * HRPD_REVERSE_TRAFFIC_SLOT_CHIPS,
        );
        assert_eq!(pilot.accepted_event_count, 1);
        assert_eq!(pilot.reverse_pilot_count, 1);
        assert_eq!(pilot.forward_traffic.len(), 0);

        let release = drive_setup_drc(&mut controller, 0x1a05_8001, 5, late_slot, 0x2);
        assert_eq!(release.forward_traffic.len(), 1);
        assert_eq!(release.forward_traffic[0].uati, 0x1a05_8001);
        assert_eq!(release.forward_traffic[0].mac_index, 5);
        assert_eq!(release.forward_traffic[0].payload_bits.len(), 1024);
    }

    #[test]
    fn rtc_ack_retransmits_on_drc_cadence_until_complete() {
        let mut controller = HrpdAirController::with_sector(26, 0, None);
        let mut allocator = allocator();
        let _ = controller
            .handle_access_indication(
                &indication(vec![HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
                    transaction_id: 0x9c,
                })]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::UatiComplete(HrpdUatiComplete {
                    message_sequence: 0,
                    upper_old_uati: Vec::new(),
                    reserved_zero: true,
                })]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::ConnectionRequest(
                    HrpdConnectionRequest {
                        transaction_id: 0x21,
                        request_reason: 0,
                        reserved_zero: true,
                    },
                )]),
                &mut allocator,
            )
            .unwrap();
        let pilot = reverse_pilot(
            &mut controller,
            0x1a05_8001,
            5,
            initial_rtc_ack_release_chip(),
        );
        assert_eq!(pilot.forward_traffic.len(), 0);
        let first = drive_setup_drc(&mut controller, 0x1a05_8001, 5, 0, 0x3);
        assert_eq!(first.forward_traffic.len(), 1);
        let first_payload = first.forward_traffic[0].payload_bits.clone();
        let first_send_slot = initial_rtc_ack_release_slot();

        let drc = |slot: u64| HrpdTrafficEvent::Drc {
            uati: 0x1a05_8001,
            mac_index: 5,
            slot,
            drc_index: 0x3,
        };

        // The DRC decodes before the cadence threshold do not resend.
        for n in 1..RTC_ACK_RETRANSMIT_MIN_SLOTS {
            let outcome = controller.handle_traffic_event(&drc(first_send_slot + n));
            assert_eq!(outcome.forward_traffic.len(), 0, "early resend at n={n}");
        }
        // The threshold-crossing DRC decode retransmits the same logical
        // RTCAck (identical payload — same SLP-D V(S)).
        let resend =
            controller.handle_traffic_event(&drc(first_send_slot + RTC_ACK_RETRANSMIT_MIN_SLOTS));
        assert_eq!(resend.forward_traffic.len(), 1);
        assert_eq!(resend.forward_traffic[0].payload_bits, first_payload);

        let high_rate_drc = HrpdTrafficEvent::Drc {
            uati: 0x1a05_8001,
            mac_index: 5,
            slot: first_send_slot + RTC_ACK_RETRANSMIT_MIN_SLOTS + 1,
            drc_index: 0xb,
        };
        let outcome = controller.handle_traffic_event(&high_rate_drc);
        assert_eq!(outcome.forward_traffic.len(), 0);

        // TrafficChannelComplete stops the RTCAck retransmit loop. Because TCC
        // arrives as reliable reverse SLP-D, the AN acknowledges its SLP-D
        // sequence using the latest valid DRC from the AT. Session
        // configuration waits until Stream 0 SLP ResetAck enables SNP
        // delivery.
        let complete = encode_reliable_default_signaling_packet(
            DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE,
            &[0x02, 0x00],
            4,
        );
        let tcc = controller.handle_traffic_event(&HrpdTrafficEvent::Stream0Signaling {
            uati: 0x1a05_8001,
            payload: complete,
        });
        assert_eq!(tcc.forward_traffic.len(), 1);
        assert_eq!(tcc.forward_traffic[0].uati, 0x1a05_8001);
        assert_eq!(tcc.forward_traffic[0].mac_index, 5);
        assert_eq!(tcc.forward_traffic[0].payload_bits.len(), 3072);
        assert_ne!(tcc.forward_traffic[0].payload_bits, first_payload);
        let reset_ack = controller.handle_traffic_event(&HrpdTrafficEvent::Stream0Signaling {
            uati: 0x1a05_8001,
            payload: vec![0, 1, 0],
        });
        assert_eq!(reset_ack.forward_traffic.len(), 1);
        assert_eq!(reset_ack.forward_traffic[0].uati, 0x1a05_8001);
        assert_eq!(reset_ack.forward_traffic[0].mac_index, 5);
        assert_eq!(reset_ack.forward_traffic[0].payload_bits.len(), 3072);
        for n in 1..(2 * RTC_ACK_RETRANSMIT_MIN_SLOTS) {
            let outcome = controller
                .handle_traffic_event(&drc(first_send_slot + RTC_ACK_RETRANSMIT_MIN_SLOTS + n));
            assert_eq!(
                outcome.forward_traffic.len(),
                0,
                "resend after TrafficChannelComplete at n={n}"
            );
        }
    }

    #[test]
    fn reliable_stream0_forward_uses_tslpwaitack_and_nslpattempt() {
        let mut controller = HrpdAirController::with_sector(26, 0, None);
        let mut allocator = allocator();
        let _ = controller
            .handle_access_indication(
                &indication(vec![HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
                    transaction_id: 0x9c,
                })]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::UatiComplete(HrpdUatiComplete {
                    message_sequence: 0,
                    upper_old_uati: Vec::new(),
                    reserved_zero: true,
                })]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::ConnectionRequest(
                    HrpdConnectionRequest {
                        transaction_id: 0x21,
                        request_reason: 0,
                        reserved_zero: true,
                    },
                )]),
                &mut allocator,
            )
            .unwrap();
        let _ = reverse_pilot(
            &mut controller,
            0x1a05_8001,
            5,
            initial_rtc_ack_release_chip(),
        );
        let _ = drive_setup_drc(&mut controller, 0x1a05_8001, 5, 0, 0xb);

        let complete = encode_reliable_default_signaling_packet(
            DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE,
            &[0x02, 0x00],
            4,
        );
        let _ = controller.handle_traffic_event(&HrpdTrafficEvent::Stream0Signaling {
            uati: 0x1a05_8001,
            payload: complete,
        });

        let request = encode_reliable_default_signaling_packet(
            DEFAULT_SESSION_CONFIGURATION_PROTOCOL_TYPE,
            &[SESSION_CONFIGURATION_REQUEST, 0x44],
            5,
        );
        let response = controller.handle_traffic_event(&HrpdTrafficEvent::Stream0Signaling {
            uati: 0x1a05_8001,
            payload: request,
        });
        assert_eq!(response.forward_traffic.len(), 1);
        let first_payload = response.forward_traffic[0].payload_bits.clone();
        let first_send_at = {
            let pending = controller.pending_traffic_assignment().unwrap();
            assert_eq!(pending.reliable_stream0_tx.len(), 1);
            assert_eq!(
                pending.reliable_stream0_tx[0].label,
                "SessionConfigurationResponse"
            );
            assert_eq!(pending.reliable_stream0_tx[0].attempts, 1);
            assert_eq!(pending.reliable_stream0_tx[0].ack_sequence_number, Some(5));
            pending.reliable_stream0_tx[0].last_send_at
        };

        let early = controller.handle_timer(first_send_at + STREAM0_SLP_D_WAIT_ACK / 2);
        assert_eq!(early.forward_traffic.len(), 0);

        let _ = drive_setup_drc(&mut controller, 0x1a05_8001, 5, 1, 0xb);
        let retry1 = controller.handle_timer(first_send_at + STREAM0_SLP_D_WAIT_ACK);
        assert_eq!(retry1.forward_traffic.len(), 1);
        assert_eq!(retry1.forward_traffic[0].payload_bits, first_payload);
        let second_send_at = {
            let pending = controller.pending_traffic_assignment().unwrap();
            assert_eq!(pending.reliable_stream0_tx[0].attempts, 2);
            pending.reliable_stream0_tx[0].last_send_at
        };

        let _ = drive_setup_drc(&mut controller, 0x1a05_8001, 5, 2, 0xb);
        let retry2 = controller.handle_timer(second_send_at + STREAM0_SLP_D_WAIT_ACK);
        assert_eq!(retry2.forward_traffic.len(), 1);
        assert_eq!(retry2.forward_traffic[0].payload_bits, first_payload);
        let third_send_at = {
            let pending = controller.pending_traffic_assignment().unwrap();
            assert_eq!(pending.reliable_stream0_tx[0].attempts, 3);
            pending.reliable_stream0_tx[0].last_send_at
        };

        let _ = drive_setup_drc(&mut controller, 0x1a05_8001, 5, 3, 0xb);
        let failed = controller.handle_timer(third_send_at + STREAM0_SLP_D_WAIT_ACK);
        assert_eq!(failed.forward_traffic.len(), 0);
        assert!(
            controller
                .pending_traffic_assignment()
                .unwrap()
                .reliable_stream0_tx
                .is_empty()
        );
    }

    #[test]
    fn attribute_update_request_is_rejected_with_reliable_slp() {
        let mut controller = HrpdAirController::with_sector(26, 0, None);
        let mut allocator = allocator();
        let _ = controller
            .handle_access_indication(
                &indication(vec![HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
                    transaction_id: 0x9c,
                })]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::UatiComplete(HrpdUatiComplete {
                    message_sequence: 0,
                    upper_old_uati: Vec::new(),
                    reserved_zero: true,
                })]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::ConnectionRequest(
                    HrpdConnectionRequest {
                        transaction_id: 0x21,
                        request_reason: 0,
                        reserved_zero: true,
                    },
                )]),
                &mut allocator,
            )
            .unwrap();
        let _ = reverse_pilot(
            &mut controller,
            0x1a05_8001,
            5,
            initial_rtc_ack_release_chip(),
        );
        let _ = drive_setup_drc(&mut controller, 0x1a05_8001, 5, 0, 0xb);
        let complete = encode_reliable_default_signaling_packet(
            DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE,
            &[0x02, 0x00],
            4,
        );
        let _ = controller.handle_traffic_event(&HrpdTrafficEvent::Stream0Signaling {
            uati: 0x1a05_8001,
            payload: complete,
        });

        // IdleState GAUP request offering one attribute record. The AN
        // rejects it, and the Reject must go out on SLP Reliable delivery.
        let request = encode_reliable_default_signaling_packet(
            DEFAULT_IDLE_STATE_PROTOCOL_TYPE,
            &[ATTRIBUTE_UPDATE_REQUEST, 0x07, 0x03, 0x00, 0x00, 0x01],
            5,
        );
        let response = controller.handle_traffic_event(&HrpdTrafficEvent::Stream0Signaling {
            uati: 0x1a05_8001,
            payload: request,
        });
        assert_eq!(response.forward_traffic.len(), 1);

        let pending = controller.pending_traffic_assignment().unwrap();
        let reject = pending
            .reliable_stream0_tx
            .iter()
            .find(|packet| packet.label == "AttributeUpdateReject")
            .expect("AttributeUpdateReject registered for reliable retransmission");
        assert_eq!(reject.protocol_type, DEFAULT_IDLE_STATE_PROTOCOL_TYPE);
        assert_eq!(reject.payload, vec![ATTRIBUTE_UPDATE_REJECT, 0x07]);
        assert_eq!(reject.attempts, 1);
        assert_eq!(reject.ack_sequence_number, Some(5));
    }

    #[test]
    fn duplicate_reliable_reverse_stream0_is_acked_without_redelivery() {
        let mut controller = HrpdAirController::with_sector(26, 0, None);
        let mut allocator = allocator();
        let _ = controller
            .handle_access_indication(
                &indication(vec![HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
                    transaction_id: 0x9c,
                })]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::UatiComplete(HrpdUatiComplete {
                    message_sequence: 0,
                    upper_old_uati: Vec::new(),
                    reserved_zero: true,
                })]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::ConnectionRequest(
                    HrpdConnectionRequest {
                        transaction_id: 0x21,
                        request_reason: 0,
                        reserved_zero: true,
                    },
                )]),
                &mut allocator,
            )
            .unwrap();
        let _ = reverse_pilot(
            &mut controller,
            0x1a05_8001,
            5,
            initial_rtc_ack_release_chip(),
        );
        let _ = drive_setup_drc(&mut controller, 0x1a05_8001, 5, 0, 0xc);

        let hardware = encode_reliable_default_signaling_packet(
            0x11,
            &[0x04, 0x01, 0x00, 0xff, 0xff, 0x07, 1, 2, 3, 4, 5, 6, 7],
            1,
        );
        let first = controller.handle_traffic_event(&HrpdTrafficEvent::Stream0Signaling {
            uati: 0x1a05_8001,
            payload: hardware.clone(),
        });
        assert_eq!(first.hardware_id_responses.len(), 1);
        assert_eq!(
            first.decoded_stream0_messages,
            vec![HrpdAccessMessage::HardwareIdResponse(
                HrpdHardwareIdResponse {
                    transaction_id: 0x01,
                    hardware_id_type: 0x00ffff,
                    hardware_id_value: vec![1, 2, 3, 4, 5, 6, 7],
                }
            )]
        );
        assert_eq!(first.forward_traffic.len(), 1);

        let duplicate = controller.handle_traffic_event(&HrpdTrafficEvent::Stream0Signaling {
            uati: 0x1a05_8001,
            payload: hardware,
        });
        assert_eq!(
            duplicate.hardware_id_responses.len(),
            0,
            "duplicate reliable SLP-D payload must not be delivered above SLP-D"
        );
        assert!(
            duplicate.decoded_stream0_messages.is_empty(),
            "duplicate reliable SLP-D payload must not be published as a decoded message"
        );
        assert_eq!(
            duplicate.forward_traffic.len(),
            1,
            "duplicate reliable SLP-D packet still needs an ACK"
        );
    }

    #[test]
    fn repeated_connection_request_after_rtc_acquired_resends_rtc_ack_on_drc_only() {
        let mut controller = HrpdAirController::with_sector(26, 0, None);
        let mut allocator = allocator();
        let _ = controller
            .handle_access_indication(
                &indication(vec![HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
                    transaction_id: 0x9c,
                })]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::UatiComplete(HrpdUatiComplete {
                    message_sequence: 0,
                    upper_old_uati: Vec::new(),
                    reserved_zero: true,
                })]),
                &mut allocator,
            )
            .unwrap();
        let connection = HrpdConnectionRequest {
            transaction_id: 0x21,
            request_reason: 0,
            reserved_zero: true,
        };
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::ConnectionRequest(
                    connection.clone(),
                )]),
                &mut allocator,
            )
            .unwrap();
        let pilot = reverse_pilot(
            &mut controller,
            0x1a05_8001,
            5,
            initial_rtc_ack_release_chip(),
        );
        assert_eq!(pilot.forward_traffic.len(), 0);
        let first = drive_setup_drc(&mut controller, 0x1a05_8001, 5, 0, 0x2);
        assert_eq!(first.forward_traffic.len(), 1);

        let mut stale = indication(vec![HrpdAccessMessage::ConnectionRequest(
            connection.clone(),
        )]);
        stale.ati = AccessTerminalIdentifier {
            ati_type: AccessTerminalIdentifierType::Uati,
            value: 0x1a05_8001,
        };
        let stale = controller
            .handle_access_indication(&stale, &mut allocator)
            .unwrap();
        assert_eq!(stale.forward_signaling.len(), 2);
        assert_eq!(stale.traffic_releases.len(), 0);
        assert_eq!(stale.traffic_assignments.len(), 0);
        assert_eq!(stale.forward_traffic.len(), 0);

        let mut retry = indication(vec![HrpdAccessMessage::ConnectionRequest(connection)]);
        retry.ati = AccessTerminalIdentifier {
            ati_type: AccessTerminalIdentifierType::Uati,
            value: 0x1a05_8001,
        };
        retry.absolute_chip = initial_rtc_ack_release_chip() + HRPD_REVERSE_TRAFFIC_SLOT_CHIPS;
        let retry_setup_slot = retry.absolute_chip / HRPD_REVERSE_TRAFFIC_SLOT_CHIPS;
        let retry = controller
            .handle_access_indication(&retry, &mut allocator)
            .unwrap();

        assert_eq!(retry.forward_signaling.len(), 2);
        assert_eq!(retry.traffic_releases.len(), 0);
        assert_eq!(retry.traffic_assignments.len(), 0);
        assert_eq!(retry.forward_traffic.len(), 0);

        let duplicate_pilot = controller.handle_traffic_event(&HrpdTrafficEvent::ReversePilot {
            uati: 0x1a05_8001,
            mac_index: 5,
            absolute_chip: rtc_ack_release_chip_after_slot(retry_setup_slot),
            snr_db_tenths: 80,
        });
        assert_eq!(duplicate_pilot.forward_traffic.len(), 0);

        let resend = controller.handle_traffic_event(&HrpdTrafficEvent::Drc {
            uati: 0x1a05_8001,
            mac_index: 5,
            slot: retry_setup_slot + RTC_ACK_RETRANSMIT_MIN_SLOTS,
            drc_index: 0x2,
        });
        assert_eq!(resend.forward_traffic.len(), 1);
        assert_eq!(resend.forward_traffic[0].uati, 0x1a05_8001);
        assert_eq!(resend.forward_traffic[0].mac_index, 5);
        assert_eq!(
            resend.forward_traffic[0].payload_bits,
            first.forward_traffic[0].payload_bits
        );
    }

    #[test]
    fn repeated_connection_request_rearms_exhausted_rtc_ack_retransmits() {
        let mut controller = HrpdAirController::with_sector(26, 0, None);
        let mut allocator = allocator();
        let _ = controller
            .handle_access_indication(
                &indication(vec![HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
                    transaction_id: 0x9c,
                })]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::UatiComplete(HrpdUatiComplete {
                    message_sequence: 0,
                    upper_old_uati: Vec::new(),
                    reserved_zero: true,
                })]),
                &mut allocator,
            )
            .unwrap();
        let connection = HrpdConnectionRequest {
            transaction_id: 0x21,
            request_reason: 0,
            reserved_zero: true,
        };
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::ConnectionRequest(
                    connection.clone(),
                )]),
                &mut allocator,
            )
            .unwrap();
        let pilot = reverse_pilot(
            &mut controller,
            0x1a05_8001,
            5,
            initial_rtc_ack_release_chip(),
        );
        assert_eq!(pilot.forward_traffic.len(), 0);
        let first = drive_setup_drc(&mut controller, 0x1a05_8001, 5, 0, 0x2);
        assert_eq!(first.forward_traffic.len(), 1);
        let first_payload = first.forward_traffic[0].payload_bits.clone();

        let mut last_send_slot = initial_rtc_ack_release_slot();
        for _ in 0..RTC_ACK_MAX_RETRANSMITS {
            let resend_slot = last_send_slot + RTC_ACK_RETRANSMIT_MIN_SLOTS;
            let resend = controller.handle_traffic_event(&HrpdTrafficEvent::Drc {
                uati: 0x1a05_8001,
                mac_index: 5,
                slot: resend_slot,
                drc_index: 0x2,
            });
            assert_eq!(resend.forward_traffic.len(), 1);
            assert_eq!(resend.forward_traffic[0].payload_bits, first_payload);
            last_send_slot = resend_slot;
        }

        let capped = controller.handle_traffic_event(&HrpdTrafficEvent::Drc {
            uati: 0x1a05_8001,
            mac_index: 5,
            slot: last_send_slot + RTC_ACK_RETRANSMIT_MIN_SLOTS,
            drc_index: 0x2,
        });
        assert_eq!(capped.forward_traffic.len(), 0);

        let mut retry = indication(vec![HrpdAccessMessage::ConnectionRequest(connection)]);
        retry.ati = AccessTerminalIdentifier {
            ati_type: AccessTerminalIdentifierType::Uati,
            value: 0x1a05_8001,
        };
        retry.absolute_chip =
            (last_send_slot + RTC_ACK_RETRANSMIT_MIN_SLOTS + 10) * HRPD_REVERSE_TRAFFIC_SLOT_CHIPS;
        let retry_slot = retry.absolute_chip / HRPD_REVERSE_TRAFFIC_SLOT_CHIPS;
        let retry = controller
            .handle_access_indication(&retry, &mut allocator)
            .unwrap();
        assert_eq!(retry.forward_traffic.len(), 0);
        assert_eq!(retry.traffic_assignments.len(), 0);

        let rearmed = controller.handle_traffic_event(&HrpdTrafficEvent::Drc {
            uati: 0x1a05_8001,
            mac_index: 5,
            slot: retry_slot,
            drc_index: 0x2,
        });
        assert_eq!(rearmed.forward_traffic.len(), 1);
        assert_eq!(rearmed.forward_traffic[0].payload_bits, first_payload);
    }

    #[test]
    fn rtc_ack_physical_ack_keeps_drc_retransmit_armed_until_tcc() {
        let mut controller = HrpdAirController::with_sector(26, 0, None);
        let mut allocator = allocator();
        let _ = controller
            .handle_access_indication(
                &indication(vec![HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
                    transaction_id: 0x9c,
                })]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::UatiComplete(HrpdUatiComplete {
                    message_sequence: 0,
                    upper_old_uati: Vec::new(),
                    reserved_zero: true,
                })]),
                &mut allocator,
            )
            .unwrap();
        let connection = HrpdConnectionRequest {
            transaction_id: 0x21,
            request_reason: 0,
            reserved_zero: true,
        };
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::ConnectionRequest(
                    connection.clone(),
                )]),
                &mut allocator,
            )
            .unwrap();
        let pilot = reverse_pilot(
            &mut controller,
            0x1a05_8001,
            5,
            initial_rtc_ack_release_chip(),
        );
        assert_eq!(pilot.forward_traffic.len(), 0);
        let first = drive_setup_drc(&mut controller, 0x1a05_8001, 5, 0, 0x2);
        assert_eq!(first.forward_traffic.len(), 1);
        let first_payload = first.forward_traffic[0].payload_bits.clone();
        let first_send_slot = initial_rtc_ack_release_slot();

        let ack = controller.handle_traffic_event(&HrpdTrafficEvent::Ack {
            uati: 0x1a05_8001,
            mac_index: 5,
            slot: first_send_slot + 4,
            ack: true,
        });
        assert_eq!(ack.ack_count, 1);
        assert_eq!(ack.forward_traffic.len(), 0);

        let drc = |slot: u64| HrpdTrafficEvent::Drc {
            uati: 0x1a05_8001,
            mac_index: 5,
            slot,
            drc_index: 0x2,
        };
        for n in 1..RTC_ACK_RETRANSMIT_MIN_SLOTS {
            let outcome = controller.handle_traffic_event(&drc(first_send_slot + n));
            assert_eq!(
                outcome.forward_traffic.len(),
                0,
                "early resend after physical ACK at n={n}"
            );
        }
        let retransmit =
            controller.handle_traffic_event(&drc(first_send_slot + RTC_ACK_RETRANSMIT_MIN_SLOTS));
        assert_eq!(retransmit.forward_traffic.len(), 1);
        assert_eq!(retransmit.forward_traffic[0].payload_bits, first_payload);

        let mut retry = indication(vec![HrpdAccessMessage::ConnectionRequest(connection)]);
        retry.ati = AccessTerminalIdentifier {
            ati_type: AccessTerminalIdentifierType::Uati,
            value: 0x1a05_8001,
        };
        retry.absolute_chip =
            (first_send_slot + 2 * RTC_ACK_RETRANSMIT_MIN_SLOTS) * HRPD_REVERSE_TRAFFIC_SLOT_CHIPS;
        let retry_setup_slot = retry.absolute_chip / HRPD_REVERSE_TRAFFIC_SLOT_CHIPS;
        let retry = controller
            .handle_access_indication(&retry, &mut allocator)
            .unwrap();
        assert_eq!(retry.forward_signaling.len(), 2);
        assert_eq!(retry.traffic_releases.len(), 0);
        assert_eq!(retry.traffic_assignments.len(), 0);
        assert_eq!(retry.forward_traffic.len(), 0);

        let resend =
            controller.handle_traffic_event(&drc(retry_setup_slot + RTC_ACK_RETRANSMIT_MIN_SLOTS));
        assert_eq!(resend.forward_traffic.len(), 1);
        assert_eq!(resend.forward_traffic[0].uati, 0x1a05_8001);
        assert_eq!(resend.forward_traffic[0].mac_index, 5);
        assert_eq!(resend.forward_traffic[0].payload_bits, first_payload);
    }

    #[test]
    fn rev_a_subtype3_refreshes_scheduled_grants_while_traffic_is_open() {
        let mut controller = HrpdAirController::with_sector(26, 0, None);
        let mut allocator = allocator();
        let _ = controller
            .handle_access_indication(
                &indication(vec![HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
                    transaction_id: 0x9c,
                })]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::UatiComplete(HrpdUatiComplete {
                    message_sequence: 0,
                    upper_old_uati: Vec::new(),
                    reserved_zero: true,
                })]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::ConnectionRequest(
                    HrpdConnectionRequest {
                        transaction_id: 0x21,
                        request_reason: 0,
                        reserved_zero: true,
                    },
                )]),
                &mut allocator,
            )
            .unwrap();
        {
            let at = controller.ats.get_mut(&0x0005_8001).unwrap();
            let pending = at.pending_traffic_assignment.as_mut().unwrap();
            pending.in_use_physical_layer_subtype = SESSION_SUBTYPE_PHYS_SUBTYPE2;
            pending.in_use_forward_traffic_mac_subtype = SESSION_SUBTYPE_ENHANCED;
            pending.in_use_reverse_traffic_mac_subtype = SESSION_SUBTYPE_RTC_MAC_SUBTYPE3;
        }

        let pilot = reverse_pilot(
            &mut controller,
            0x1a05_8001,
            5,
            initial_rtc_ack_release_chip(),
        );
        assert_eq!(pilot.forward_traffic.len(), 0);
        let first = drive_setup_drc(&mut controller, 0x1a05_8001, 5, 0, 0x2);
        let first_send_slot = initial_rtc_ack_release_slot();

        assert_eq!(first.forward_traffic.len(), 1);
        let expected_rtca = default_reverse_traffic_mac_rtc_ack_ftc_payload_bits_for_mac_subtype(
            implemented_forward_traffic_payload_bits_for_drc(0x2).unwrap(),
            0,
            SESSION_SUBTYPE_ENHANCED,
        )
        .unwrap();
        let expected_grant = default_reverse_traffic_mac_grant_ftc_payload_bits_for_mac_subtype(
            implemented_forward_traffic_payload_bits_for_drc(0x2).unwrap(),
            &HrpdAirController::autonomous_rtc_mac_grants(),
            SESSION_SUBTYPE_ENHANCED,
        )
        .unwrap();
        assert_ne!(expected_grant, expected_rtca);
        assert_eq!(first.forward_traffic[0].uati, 0x1a05_8001);
        assert_eq!(first.forward_traffic[0].mac_index, 5);
        assert_eq!(first.forward_traffic[0].payload_bits, expected_rtca);

        let before_ack = controller.handle_traffic_event(&HrpdTrafficEvent::Drc {
            uati: 0x1a05_8001,
            mac_index: 5,
            slot: first_send_slot + 1,
            drc_index: 0x2,
        });
        assert_eq!(before_ack.forward_traffic.len(), 0);

        // Do not alter T2P while the connection is still waiting for
        // TrafficChannelComplete.
        assert!(!controller.pending_traffic_assignment().unwrap().active);
        assert!(
            !controller
                .pending_traffic_assignment()
                .unwrap()
                .rtc_ack_delivered
        );

        let pre_tcc_drc = controller.handle_traffic_event(&HrpdTrafficEvent::Drc {
            uati: 0x1a05_8001,
            mac_index: 5,
            slot: first_send_slot + 5,
            drc_index: 0x2,
        });
        assert_eq!(pre_tcc_drc.forward_traffic.len(), 0);
        {
            let pending = controller.pending_traffic_assignment().unwrap();
            assert!(pending.reliable_stream0_tx.is_empty());
            assert_eq!(pending.rtc_grant_sends, 0);
            assert_eq!(pending.rtc_grant_last_send_slot, None);
        }

        let later_drc = controller.handle_traffic_event(&HrpdTrafficEvent::Drc {
            uati: 0x1a05_8001,
            mac_index: 5,
            slot: first_send_slot + 125,
            drc_index: 0x2,
        });
        assert_eq!(later_drc.forward_traffic.len(), 0);
        assert_eq!(
            controller
                .pending_traffic_assignment()
                .unwrap()
                .rtc_grant_sends,
            0
        );

        // TrafficChannelComplete opens the connection and stops the setup
        // supervision timer.
        let tcc = controller.handle_traffic_event(&HrpdTrafficEvent::Stream0Signaling {
            uati: 0x1a05_8001,
            payload: encode_reliable_default_signaling_packet(
                DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE,
                &[
                    0x02,
                    controller
                        .pending_traffic_assignment()
                        .unwrap()
                        .assignment
                        .message_sequence,
                ],
                1,
            ),
        });
        assert_eq!(tcc.traffic_channel_open_uatis, vec![0x1a05_8001]);
        {
            let pending = controller.pending_traffic_assignment().unwrap();
            assert!(pending.active);
            assert!(pending.rtc_ack_delivered);
        }

        let grant_send_slot = first_send_slot + 246;
        let first_grant = controller.handle_traffic_event(&HrpdTrafficEvent::Drc {
            uati: 0x1a05_8001,
            mac_index: 5,
            slot: grant_send_slot,
            drc_index: 0x2,
        });
        assert_eq!(first_grant.forward_traffic.len(), 1);
        assert_eq!(first_grant.forward_traffic[0].payload_bits, expected_grant);

        let early_repeat = controller.handle_traffic_event(&HrpdTrafficEvent::Drc {
            uati: 0x1a05_8001,
            mac_index: 5,
            slot: grant_send_slot + RTC_MAC_GRANT_RETRANSMIT_MIN_SLOTS - 1,
            drc_index: 0x2,
        });
        assert!(early_repeat.forward_traffic.is_empty());

        // The 400 ms grant hold is refreshed every 200 ms for the lifetime of
        // the open assignment, so each subsequent DRC still yields one grant.
        for send in 1..10u64 {
            let refresh = controller.handle_traffic_event(&HrpdTrafficEvent::Drc {
                uati: 0x1a05_8001,
                mac_index: 5,
                slot: grant_send_slot + send * RTC_MAC_GRANT_RETRANSMIT_MIN_SLOTS,
                drc_index: 0x2,
            });
            assert_eq!(refresh.forward_traffic.len(), 1, "grant refresh {send}");
            assert_eq!(refresh.forward_traffic[0].payload_bits, expected_grant);
        }
        assert_eq!(
            controller
                .pending_traffic_assignment()
                .unwrap()
                .rtc_grant_sends,
            10
        );

        let setup_started_at = controller
            .pending_traffic_assignment()
            .unwrap()
            .setup_started_at;
        let no_setup_expiry = controller
            .handle_timer_with_allocator(setup_started_at + HRPD_RTCMP_AN_SETUP, &mut allocator);
        assert!(
            no_setup_expiry.traffic_releases.is_empty(),
            "an active (TCC-completed) connection must not expire as pending setup"
        );
        assert!(controller.pending_traffic_assignment().is_some());
    }

    #[test]
    fn rev_a_subtype3_tcc_before_reverse_pilot_still_enables_grants() {
        let mut controller = HrpdAirController::with_sector(26, 0, None);
        let mut allocator = allocator();
        let _ = controller
            .handle_access_indication(
                &indication(vec![HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
                    transaction_id: 0x9c,
                })]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::UatiComplete(HrpdUatiComplete {
                    message_sequence: 0,
                    upper_old_uati: Vec::new(),
                    reserved_zero: true,
                })]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::ConnectionRequest(
                    HrpdConnectionRequest {
                        transaction_id: 0x21,
                        request_reason: 0,
                        reserved_zero: true,
                    },
                )]),
                &mut allocator,
            )
            .unwrap();
        {
            let at = controller.ats.get_mut(&0x0005_8001).unwrap();
            let pending = at.pending_traffic_assignment.as_mut().unwrap();
            pending.in_use_physical_layer_subtype = SESSION_SUBTYPE_PHYS_SUBTYPE2;
            pending.in_use_forward_traffic_mac_subtype = SESSION_SUBTYPE_ENHANCED;
            pending.in_use_reverse_traffic_mac_subtype = SESSION_SUBTYPE_RTC_MAC_SUBTYPE3;
        }

        let tcc = controller.handle_traffic_event(&HrpdTrafficEvent::Stream0Signaling {
            uati: 0x1a05_8001,
            payload: encode_reliable_default_signaling_packet(
                DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE,
                &[0x02, 0x00],
                1,
            ),
        });
        assert_eq!(tcc.traffic_channel_open_uatis, vec![0x1a05_8001]);
        {
            let pending = controller.pending_traffic_assignment().unwrap();
            assert!(pending.active);
            assert!(pending.rtc_ack_delivered);
            assert!(!pending.rtc_acquired);
        }

        let pilot = reverse_pilot(
            &mut controller,
            0x1a05_8001,
            5,
            initial_rtc_ack_release_chip(),
        );
        assert!(pilot.forward_traffic.is_empty());
        assert!(
            controller
                .pending_traffic_assignment()
                .unwrap()
                .rtc_acquired
        );

        let grant = controller.handle_traffic_event(&HrpdTrafficEvent::Drc {
            uati: 0x1a05_8001,
            mac_index: 5,
            slot: initial_rtc_ack_release_slot(),
            drc_index: 0x2,
        });
        let expected_grant = default_reverse_traffic_mac_grant_ftc_payload_bits_for_mac_subtype(
            implemented_forward_traffic_payload_bits_for_drc(0x2).unwrap(),
            &HrpdAirController::autonomous_rtc_mac_grants(),
            SESSION_SUBTYPE_ENHANCED,
        )
        .unwrap();
        assert_eq!(grant.forward_traffic.len(), 1);
        assert_eq!(grant.forward_traffic[0].payload_bits, expected_grant);
    }

    /// Without a TrafficChannelComplete, the physical RTCAck alone must not
    /// open the connection, and the setup supervision timer must release it.
    #[test]
    fn rev_a_subtype3_rtc_ack_without_tcc_expires_pending_setup() {
        let mut controller = HrpdAirController::with_sector(26, 0, None);
        let mut allocator = allocator();
        let _ = controller
            .handle_access_indication(
                &indication(vec![HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
                    transaction_id: 0x9c,
                })]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::UatiComplete(HrpdUatiComplete {
                    message_sequence: 0,
                    upper_old_uati: Vec::new(),
                    reserved_zero: true,
                })]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::ConnectionRequest(
                    HrpdConnectionRequest {
                        transaction_id: 0x21,
                        request_reason: 0,
                        reserved_zero: true,
                    },
                )]),
                &mut allocator,
            )
            .unwrap();
        {
            let at = controller.ats.get_mut(&0x0005_8001).unwrap();
            let pending = at.pending_traffic_assignment.as_mut().unwrap();
            pending.in_use_physical_layer_subtype = SESSION_SUBTYPE_PHYS_SUBTYPE2;
            pending.in_use_forward_traffic_mac_subtype = SESSION_SUBTYPE_ENHANCED;
            pending.in_use_reverse_traffic_mac_subtype = SESSION_SUBTYPE_RTC_MAC_SUBTYPE3;
        }
        let _ = reverse_pilot(
            &mut controller,
            0x1a05_8001,
            5,
            initial_rtc_ack_release_chip(),
        );
        let _ = drive_setup_drc(&mut controller, 0x1a05_8001, 5, 0, 0x2);
        let ack = controller.handle_traffic_event(&HrpdTrafficEvent::Ack {
            uati: 0x1a05_8001,
            mac_index: 5,
            slot: initial_rtc_ack_release_slot() + 4,
            ack: true,
        });
        assert!(ack.traffic_channel_open_uatis.is_empty());
        assert!(!controller.pending_traffic_assignment().unwrap().active);

        let setup_started_at = controller
            .pending_traffic_assignment()
            .unwrap()
            .setup_started_at;
        let expiry = controller
            .handle_timer_with_allocator(setup_started_at + HRPD_RTCMP_AN_SETUP, &mut allocator);
        assert_eq!(expiry.traffic_releases.len(), 1);
        assert!(controller.pending_traffic_assignment().is_none());
    }

    #[test]
    fn uati_request_during_pending_traffic_setup_restarts_address_management() {
        let mut controller = HrpdAirController::with_sector(26, 0, None);
        let mut allocator = allocator();
        let _ = controller
            .handle_access_indication(
                &indication(vec![HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
                    transaction_id: 0x9c,
                })]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::UatiComplete(HrpdUatiComplete {
                    message_sequence: 0,
                    upper_old_uati: Vec::new(),
                    reserved_zero: true,
                })]),
                &mut allocator,
            )
            .unwrap();
        let mut connection = indication(vec![HrpdAccessMessage::ConnectionRequest(
            HrpdConnectionRequest {
                transaction_id: 0x21,
                request_reason: 0,
                reserved_zero: true,
            },
        )]);
        connection.ati = AccessTerminalIdentifier {
            ati_type: AccessTerminalIdentifierType::Uati,
            value: 0x1a05_8001,
        };
        let connection = controller
            .handle_access_indication(&connection, &mut allocator)
            .unwrap();
        assert_eq!(connection.traffic_assignments.len(), 1);

        let mut uati_retry = indication(vec![HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
            transaction_id: 0x9d,
        })]);
        uati_retry.ati = AccessTerminalIdentifier {
            ati_type: AccessTerminalIdentifierType::Uati,
            value: 0x1a05_8001,
        };
        let uati_retry = controller
            .handle_access_indication(&uati_retry, &mut allocator)
            .unwrap();

        assert_eq!(uati_retry.traffic_releases.len(), 1);
        assert_eq!(uati_retry.traffic_releases[0].uati, 0x1a05_8001);
        assert_eq!(uati_retry.traffic_assignments.len(), 0);
        assert_eq!(
            signaling_for(&uati_retry, DEFAULT_ADDRESS_MANAGEMENT_PROTOCOL_TYPE).uati,
            Some(0x0005_8001)
        );
        assert_eq!(allocator.issued_count(), 1);

        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::UatiComplete(HrpdUatiComplete {
                    message_sequence: 1,
                    upper_old_uati: Vec::new(),
                    reserved_zero: true,
                })]),
                &mut allocator,
            )
            .unwrap();
        let mut reopen = indication(vec![HrpdAccessMessage::ConnectionRequest(
            HrpdConnectionRequest {
                transaction_id: 0x22,
                request_reason: 1,
                reserved_zero: true,
            },
        )]);
        reopen.ati = AccessTerminalIdentifier {
            ati_type: AccessTerminalIdentifierType::Uati,
            value: 0x1a05_8001,
        };
        let reopen = controller
            .handle_access_indication(&reopen, &mut allocator)
            .unwrap();

        assert_eq!(reopen.traffic_assignments.len(), 1);
        assert_eq!(reopen.traffic_assignments[0].uati, 0x1a05_8001);
        assert_eq!(reopen.traffic_releases.len(), 0);
    }

    #[test]
    fn connection_request_while_active_traffic_releases_and_reassigns() {
        // A ConnectionRequest is an Idle State message; receiving one while
        // an assignment is still marked active means the AT abandoned that
        // channel. The AN must release the stale worker and issue a fresh
        // TCA instead of ignoring the AT until DRC-silence supervision.
        let mut controller = HrpdAirController::with_sector(26, 0, None);
        let mut allocator = allocator();
        let _ = controller
            .handle_access_indication(
                &indication(vec![HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
                    transaction_id: 0x9c,
                })]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::UatiComplete(HrpdUatiComplete {
                    message_sequence: 0,
                    upper_old_uati: Vec::new(),
                    reserved_zero: true,
                })]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::ConnectionRequest(
                    HrpdConnectionRequest {
                        transaction_id: 0x21,
                        request_reason: 0,
                        reserved_zero: true,
                    },
                )]),
                &mut allocator,
            )
            .unwrap();
        let pilot = reverse_pilot(
            &mut controller,
            0x1a05_8001,
            5,
            initial_rtc_ack_release_chip(),
        );
        assert_eq!(pilot.forward_traffic.len(), 0);
        let _ = drive_setup_drc(&mut controller, 0x1a05_8001, 5, 0, 0x2);
        let complete = encode_reliable_default_signaling_packet(
            DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE,
            &[0x02, 0x00],
            4,
        );
        let _ = controller.handle_traffic_event(&HrpdTrafficEvent::Stream0Signaling {
            uati: 0x1a05_8001,
            payload: complete,
        });

        let mut reopen = indication(vec![HrpdAccessMessage::ConnectionRequest(
            HrpdConnectionRequest {
                transaction_id: 0x22,
                request_reason: 0,
                reserved_zero: true,
            },
        )]);
        reopen.ati = AccessTerminalIdentifier {
            ati_type: AccessTerminalIdentifierType::Uati,
            value: 0x1a05_8001,
        };
        reopen.absolute_chip = 456_789;
        let reopen = controller
            .handle_access_indication(&reopen, &mut allocator)
            .unwrap();

        assert_eq!(reopen.traffic_releases.len(), 1);
        assert_eq!(reopen.traffic_releases[0].uati, 0x1a05_8001);
        assert_eq!(reopen.traffic_assignments.len(), 1);
        assert_eq!(reopen.traffic_assignments[0].uati, 0x1a05_8001);
        assert_ne!(
            reopen.traffic_assignments[0].mac_index, 5,
            "the reassignment allocates a fresh MAC index"
        );
        assert!(
            reopen
                .forward_signaling
                .iter()
                .any(|signaling| signaling.protocol_type == DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE),
            "the reassignment sends a fresh TrafficChannelAssignment"
        );
    }

    #[test]
    fn slp_d_vs_resets_to_zero_for_each_new_connection() {
        // SLP-D resets on every connection initiation (C.S0024-0
        // §2.6.4.2.3.2), so the first reliable forward message of every
        // connection carries SequenceNumber 0. This mirrors the live
        // re-registration loop: a failed setup is followed by a fresh
        // UATIRequest, UATIComplete, and ConnectionRequest, and the new
        // connection's RTCAck must be byte-identical to the first one
        // (same V(S)=0), not V(S)=1.
        let mut controller = HrpdAirController::with_sector(26, 0, None);
        let mut allocator = allocator();
        let _ = controller
            .handle_access_indication(
                &indication(vec![HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
                    transaction_id: 0x9c,
                })]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::UatiComplete(HrpdUatiComplete {
                    message_sequence: 0,
                    upper_old_uati: Vec::new(),
                    reserved_zero: true,
                })]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::ConnectionRequest(
                    HrpdConnectionRequest {
                        transaction_id: 0x21,
                        request_reason: 0,
                        reserved_zero: true,
                    },
                )]),
                &mut allocator,
            )
            .unwrap();
        let pilot = reverse_pilot(
            &mut controller,
            0x1a05_8001,
            5,
            initial_rtc_ack_release_chip(),
        );
        assert_eq!(pilot.forward_traffic.len(), 0);
        let first = drive_setup_drc(&mut controller, 0x1a05_8001, 5, 0, 0x2);
        assert_eq!(first.forward_traffic.len(), 1);
        let first_payload = first.forward_traffic[0].payload_bits.clone();

        // The AT abandons setup and retries the connection under its assigned
        // UATI. The AN restarts the setup state, including Stream 0 SLP-D.
        let mut uati_retry = indication(vec![HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
            transaction_id: 0x9d,
        })]);
        uati_retry.ati = AccessTerminalIdentifier {
            ati_type: AccessTerminalIdentifierType::Uati,
            value: 0x1a05_8001,
        };
        let uati_retry = controller
            .handle_access_indication(&uati_retry, &mut allocator)
            .unwrap();
        assert_eq!(uati_retry.traffic_releases.len(), 1);

        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::UatiComplete(HrpdUatiComplete {
                    message_sequence: 1,
                    upper_old_uati: Vec::new(),
                    reserved_zero: true,
                })]),
                &mut allocator,
            )
            .unwrap();

        let mut second_setup = indication(vec![HrpdAccessMessage::ConnectionRequest(
            HrpdConnectionRequest {
                transaction_id: 0x22,
                request_reason: 0,
                reserved_zero: true,
            },
        )]);
        second_setup.ati = AccessTerminalIdentifier {
            ati_type: AccessTerminalIdentifierType::Uati,
            value: 0x1a05_8001,
        };
        second_setup.absolute_chip = 456_789;
        let second = controller
            .handle_access_indication(&second_setup, &mut allocator)
            .unwrap();
        assert_eq!(second.traffic_assignments.len(), 1);
        assert_eq!(second.traffic_releases.len(), 0);

        let pilot = reverse_pilot(
            &mut controller,
            0x1a05_8001,
            5,
            rtc_ack_release_chip_after_slot(
                second_setup.absolute_chip / HRPD_REVERSE_TRAFFIC_SLOT_CHIPS,
            ),
        );
        assert_eq!(pilot.forward_traffic.len(), 0);
        let release = drive_setup_drc(
            &mut controller,
            0x1a05_8001,
            5,
            second_setup.absolute_chip / HRPD_REVERSE_TRAFFIC_SLOT_CHIPS,
            0x2,
        );
        assert_eq!(release.forward_traffic.len(), 1);
        assert_eq!(
            release.forward_traffic[0].payload_bits, first_payload,
            "second connection's RTCAck must restart SLP-D at V(S)=0"
        );
    }

    #[test]
    fn uati_request_does_not_orphan_pending_traffic_uati() {
        let mut controller = HrpdAirController::with_sector(26, 0, None);
        let mut allocator = allocator();
        let _ = controller
            .handle_access_indication(
                &indication(vec![HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
                    transaction_id: 0x9c,
                })]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::UatiComplete(HrpdUatiComplete {
                    message_sequence: 0,
                    upper_old_uati: Vec::new(),
                    reserved_zero: true,
                })]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::ConnectionRequest(
                    HrpdConnectionRequest {
                        transaction_id: 0x21,
                        request_reason: 0,
                        reserved_zero: true,
                    },
                )]),
                &mut allocator,
            )
            .unwrap();

        let _ = controller
            .handle_access_indication(
                &indication(vec![HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
                    transaction_id: 0x9d,
                })]),
                &mut allocator,
            )
            .unwrap();

        let pilot = reverse_pilot(
            &mut controller,
            0x1a05_8001,
            5,
            initial_rtc_ack_release_chip(),
        );
        assert_eq!(pilot.accepted_event_count, 1);
        assert_eq!(pilot.unknown_session_count, 0);
        assert_eq!(pilot.forward_traffic.len(), 0);
        let release = drive_setup_drc(&mut controller, 0x1a05_8001, 5, 0, 0x2);
        assert_eq!(release.forward_traffic.len(), 1);
    }

    #[test]
    fn rtc_ack_queues_first_setup_drc_when_reverse_drc_was_seen() {
        // RTCAck is encoded for the first setup DRC observed from the AT. The
        // scheduler may still rebuild it for the packet start-slot DRC.
        let mut controller = HrpdAirController::with_sector(26, 0, None);
        let mut allocator = allocator();
        let _ = controller
            .handle_access_indication(
                &indication(vec![HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
                    transaction_id: 0x9c,
                })]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::UatiComplete(HrpdUatiComplete {
                    message_sequence: 0,
                    upper_old_uati: Vec::new(),
                    reserved_zero: true,
                })]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::ConnectionRequest(
                    HrpdConnectionRequest {
                        transaction_id: 0x21,
                        request_reason: 0,
                        reserved_zero: true,
                    },
                )]),
                &mut allocator,
            )
            .unwrap();

        // AT reports DRC index 0x5 (614.4 kbps) before pilot is acquired.
        let _ = controller.handle_traffic_event(&HrpdTrafficEvent::Drc {
            uati: 0x1a05_8001,
            mac_index: 5,
            slot: 100,
            drc_index: 0x5,
        });
        // Reserved/null indexes are not tracked.
        let _ = controller.handle_traffic_event(&HrpdTrafficEvent::Drc {
            uati: 0x1a05_8001,
            mac_index: 5,
            slot: 101,
            drc_index: 0x0,
        });
        let _ = controller.handle_traffic_event(&HrpdTrafficEvent::Drc {
            uati: 0x1a05_8001,
            mac_index: 5,
            slot: 102,
            drc_index: 0xf,
        });
        let pilot = reverse_pilot(
            &mut controller,
            0x1a05_8001,
            5,
            initial_rtc_ack_release_chip(),
        );
        assert_eq!(pilot.forward_traffic.len(), 0);
        let release = drive_setup_drc(&mut controller, 0x1a05_8001, 5, 0, 0x5);
        assert_eq!(
            release.forward_traffic[0].payload_bits.len(),
            2048,
            "RTCAck should be initially represented at the first setup DRC size"
        );
    }

    #[test]
    fn rtc_ack_waits_when_no_drc_reported() {
        // Without a decoded DRC, the AN has no spec-valid FTC rate for RTCAck.
        let mut controller = HrpdAirController::with_sector(26, 0, None);
        let mut allocator = allocator();
        let _ = controller
            .handle_access_indication(
                &indication(vec![HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
                    transaction_id: 0x9c,
                })]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::UatiComplete(HrpdUatiComplete {
                    message_sequence: 0,
                    upper_old_uati: Vec::new(),
                    reserved_zero: true,
                })]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::ConnectionRequest(
                    HrpdConnectionRequest {
                        transaction_id: 0x21,
                        request_reason: 0,
                        reserved_zero: true,
                    },
                )]),
                &mut allocator,
            )
            .unwrap();
        let pilot = reverse_pilot(&mut controller, 0x1a05_8001, 5, 123_456);
        assert_eq!(pilot.forward_traffic.len(), 0);
    }

    #[test]
    fn traffic_channel_complete_marks_assignment_active() {
        let mut controller = HrpdAirController::with_sector(26, 0, None);
        let mut allocator = allocator();
        let _ = controller
            .handle_access_indication(
                &indication(vec![HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
                    transaction_id: 0x9c,
                })]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::UatiComplete(HrpdUatiComplete {
                    message_sequence: 0,
                    upper_old_uati: Vec::new(),
                    reserved_zero: true,
                })]),
                &mut allocator,
            )
            .unwrap();
        let request = HrpdAccessMessage::ConnectionRequest(HrpdConnectionRequest {
            transaction_id: 0x21,
            request_reason: 0,
            reserved_zero: true,
        });
        let first = controller
            .handle_access_indication(&uati_indication(vec![request.clone()]), &mut allocator)
            .unwrap();
        let first_tca_sequence =
            signaling_for(&first, DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE).payload[1];
        assert_eq!(first_tca_sequence, 0);

        let complete = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::TrafficChannelComplete(
                    HrpdTrafficChannelComplete {
                        message_sequence: first_tca_sequence,
                    },
                )]),
                &mut allocator,
            )
            .unwrap();
        assert_eq!(complete.traffic_channel_completes.len(), 1);

        let after_active = controller
            .handle_access_indication(&uati_indication(vec![request]), &mut allocator)
            .unwrap();
        assert_eq!(non_acack_signaling(&after_active).len(), 0);
        assert_eq!(after_active.traffic_assignments.len(), 0);
    }

    #[test]
    fn reverse_pilot_lost_releases_active_traffic_without_closing_session() {
        let mut controller = HrpdAirController::with_sector(26, 0, None);
        let mut allocator = allocator();
        let _ = controller
            .handle_access_indication(
                &indication(vec![HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
                    transaction_id: 0x9c,
                })]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::UatiComplete(HrpdUatiComplete {
                    message_sequence: 0,
                    upper_old_uati: Vec::new(),
                    reserved_zero: true,
                })]),
                &mut allocator,
            )
            .unwrap();
        let first = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::ConnectionRequest(
                    HrpdConnectionRequest {
                        transaction_id: 0x21,
                        request_reason: 0,
                        reserved_zero: true,
                    },
                )]),
                &mut allocator,
            )
            .unwrap();
        let traffic = first.traffic_assignments[0].clone();
        let first_tca_sequence =
            signaling_for(&first, DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE).payload[1];

        let complete = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::TrafficChannelComplete(
                    HrpdTrafficChannelComplete {
                        message_sequence: first_tca_sequence,
                    },
                )]),
                &mut allocator,
            )
            .unwrap();
        assert_eq!(complete.traffic_channel_completes.len(), 1);

        let lost = controller.handle_traffic_event(&HrpdTrafficEvent::ReversePilotLost {
            uati: traffic.uati,
            mac_index: traffic.mac_index,
            last_good_chip: 123_456,
            lost_at_chip: 123_456 + 5 * HRPD_REVERSE_TRAFFIC_SLOT_CHIPS * 600,
            lost_chips: 5 * HRPD_REVERSE_TRAFFIC_SLOT_CHIPS * 600,
            last_snr_db_tenths: 90,
            last_coherence_x1000: 980,
        });

        assert_eq!(lost.accepted_event_count, 1);
        assert_eq!(
            lost.traffic_releases,
            vec![HrpdTrafficReleaseRequest {
                uati: traffic.uati,
                mac_index: traffic.mac_index,
            }]
        );
        assert_eq!(lost.traffic_channel_closed_uatis, vec![traffic.uati]);
        assert!(lost.session_closed_uatis.is_empty());
    }

    #[test]
    fn drc_silence_does_not_release_active_traffic() {
        let mut controller = HrpdAirController::with_sector(26, 0, None);
        let mut allocator = allocator();
        let _ = controller
            .handle_access_indication(
                &indication(vec![HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
                    transaction_id: 0x9c,
                })]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::UatiComplete(HrpdUatiComplete {
                    message_sequence: 0,
                    upper_old_uati: Vec::new(),
                    reserved_zero: true,
                })]),
                &mut allocator,
            )
            .unwrap();
        let first = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::ConnectionRequest(
                    HrpdConnectionRequest {
                        transaction_id: 0x21,
                        request_reason: 0,
                        reserved_zero: true,
                    },
                )]),
                &mut allocator,
            )
            .unwrap();
        let traffic = first.traffic_assignments[0].clone();
        let first_tca_sequence =
            signaling_for(&first, DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE).payload[1];

        let _ = drive_setup_drc(
            &mut controller,
            traffic.uati,
            traffic.mac_index,
            initial_rtc_ack_release_slot(),
            0x0c,
        );
        let complete = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::TrafficChannelComplete(
                    HrpdTrafficChannelComplete {
                        message_sequence: first_tca_sequence,
                    },
                )]),
                &mut allocator,
            )
            .unwrap();
        assert_eq!(complete.traffic_channel_completes.len(), 1);

        let setup_started_at = controller
            .pending_traffic_assignment()
            .unwrap()
            .setup_started_at;
        let silent = controller.handle_timer(setup_started_at + Duration::from_secs(10));

        assert!(silent.traffic_releases.is_empty());
        let pending = controller.pending_traffic_assignment().unwrap();
        assert!(pending.active);
        assert_eq!(pending.traffic_uati, traffic.uati);
    }

    #[test]
    fn a9_disconnect_a8_releases_traffic_and_allows_reconnect() {
        let mut controller = HrpdAirController::with_sector(26, 0, None);
        let mut allocator = allocator();
        let _ = controller
            .handle_access_indication(
                &indication(vec![HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
                    transaction_id: 0x9c,
                })]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::UatiComplete(HrpdUatiComplete {
                    message_sequence: 0,
                    upper_old_uati: Vec::new(),
                    reserved_zero: true,
                })]),
                &mut allocator,
            )
            .unwrap();
        let first = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::ConnectionRequest(
                    HrpdConnectionRequest {
                        transaction_id: 0x21,
                        request_reason: 0,
                        reserved_zero: true,
                    },
                )]),
                &mut allocator,
            )
            .unwrap();
        let traffic = first.traffic_assignments[0].clone();
        let first_tca_sequence =
            signaling_for(&first, DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE).payload[1];
        let complete = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::TrafficChannelComplete(
                    HrpdTrafficChannelComplete {
                        message_sequence: first_tca_sequence,
                    },
                )]),
                &mut allocator,
            )
            .unwrap();
        assert_eq!(complete.traffic_channel_completes.len(), 1);

        let disconnect = controller.handle_a9_disconnect_a8(traffic.uati, traffic.mac_index, 0x77);
        assert_eq!(disconnect.accepted_event_count, 1);
        assert_eq!(
            disconnect.traffic_releases,
            vec![HrpdTrafficReleaseRequest {
                uati: traffic.uati,
                mac_index: traffic.mac_index,
            }]
        );
        assert_eq!(disconnect.traffic_channel_closed_uatis, vec![traffic.uati]);
        assert!(disconnect.session_closed_uatis.is_empty());

        let mut reconnect = indication(vec![HrpdAccessMessage::ConnectionRequest(
            HrpdConnectionRequest {
                transaction_id: 0x22,
                request_reason: 0,
                reserved_zero: true,
            },
        )]);
        reconnect.ati = AccessTerminalIdentifier {
            ati_type: AccessTerminalIdentifierType::Uati,
            value: traffic.uati,
        };
        let reconnect = controller
            .handle_access_indication(&reconnect, &mut allocator)
            .unwrap();
        assert_eq!(reconnect.traffic_assignments.len(), 1);
        assert_eq!(reconnect.traffic_assignments[0].uati, traffic.uati);
        assert_eq!(reconnect.traffic_releases.len(), 0);
        assert_eq!(
            signaling_for(&reconnect, DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE).payload[1],
            0
        );
        assert_eq!(
            signaling_for(&reconnect, DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE).reliable_sequence,
            None
        );
    }

    #[test]
    fn parse_stream0_default_signaling_accepts_reliable_slp_d_header() {
        let packet = encode_reliable_default_signaling_packet(
            DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE,
            &[0x02, 0x5a],
            3,
        );

        let parsed = parse_stream0_default_signaling(&packet)
            .expect("reliable Stream 0 TrafficChannelComplete should parse");

        assert_eq!(
            parsed.message,
            Some(HrpdAccessMessage::TrafficChannelComplete(
                HrpdTrafficChannelComplete {
                    message_sequence: 0x5a
                }
            ))
        );
        assert_eq!(parsed.ack_sequence_number, None);
        assert_eq!(parsed.sequence_number, Some(3));
    }

    #[test]
    fn parse_stream0_default_signaling_accepts_slp_d_ack_only() {
        let packet = cdma_common::hrpd::air::encode_default_signaling_slp_d_ack_packet(6);

        let parsed = parse_stream0_default_signaling(&packet).expect("SLP-D ACK-only should parse");

        assert_eq!(parsed.message, None);
        assert_eq!(parsed.ack_sequence_number, Some(6));
        assert_eq!(parsed.sequence_number, None);
    }

    #[test]
    fn parse_stream0_default_signaling_decodes_slp_reset_ack() {
        let packet = vec![0, 1, 0x42];

        let parsed = parse_stream0_default_signaling(&packet).expect("SLP ResetAck should parse");

        assert_eq!(
            parsed.message,
            Some(HrpdAccessMessage::DefaultSignalingResetAck(
                HrpdDefaultSignalingResetAck {
                    message_sequence: 0x42,
                },
            ))
        );
        assert_eq!(parsed.ack_sequence_number, None);
        assert_eq!(parsed.sequence_number, None);
    }

    #[test]
    fn parse_stream0_default_signaling_accepts_padded_format_a_tcc() {
        let mut packet = cdma_common::hrpd::air::encode_default_signaling_packet(
            DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE,
            &[0x02, 0x5a],
        );
        packet.resize(29, 0);

        let parsed = parse_stream0_default_signaling(&packet)
            .expect("padded Format A Stream 0 TrafficChannelComplete should parse");

        assert_eq!(
            parsed.message,
            Some(HrpdAccessMessage::TrafficChannelComplete(
                HrpdTrafficChannelComplete {
                    message_sequence: 0x5a
                }
            ))
        );
        assert_eq!(parsed.ack_sequence_number, None);
        assert_eq!(parsed.sequence_number, None);
    }

    #[test]
    fn parse_stream0_default_signaling_decodes_basic_reverse_messages() {
        let route = parse_stream0_default_signaling(&encode_reliable_default_signaling_packet(
            DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE,
            &[0x00, 0x07, 0x00, 0x01, 0x00],
            2,
        ))
        .expect("route update should parse");
        assert!(matches!(
            route.message,
            Some(HrpdAccessMessage::RouteUpdate(AirRouteUpdate {
                message_sequence: 7,
                ..
            }))
        ));
        assert_eq!(route.ack_sequence_number, None);
        assert_eq!(route.sequence_number, Some(2));

        let session_close =
            parse_stream0_default_signaling(&encode_reliable_default_signaling_packet(
                DEFAULT_SESSION_MANAGEMENT_PROTOCOL_TYPE,
                &[0x01, 0x03, 0x03, 0x0d, 0x00, 0x00],
                4,
            ))
            .expect("session close should parse");
        assert_eq!(
            session_close.message,
            Some(HrpdAccessMessage::SessionClose(HrpdSessionClose {
                close_reason: 3,
                more_info: vec![0x0d, 0x00, 0x00]
            }))
        );
        let Some(HrpdAccessMessage::SessionClose(close)) = &session_close.message else {
            panic!("expected SessionClose");
        };
        let reference = hrpd_protocol_reference_from_more_info(&close.more_info)
            .expect("reason 0x03 MoreInfo should decode protocol reference");
        assert_eq!(
            reference.protocol_type,
            u16::from(DEFAULT_CONNECTED_STATE_PROTOCOL_TYPE)
        );
        assert_eq!(reference.protocol_subtype, 0);
        assert_eq!(session_close.ack_sequence_number, None);
        assert_eq!(session_close.sequence_number, Some(4));

        let connection_close =
            parse_stream0_default_signaling(&encode_reliable_default_signaling_packet(
                DEFAULT_CONNECTED_STATE_PROTOCOL_TYPE,
                &[0x00, 0x40],
                5,
            ))
            .expect("connection close should parse");
        assert_eq!(
            connection_close.message,
            Some(HrpdAccessMessage::ConnectionClose(HrpdConnectionClose {
                close_reason: 2,
                suspend_enable: false,
                suspend_time: None,
                reserved_zero: true,
            }))
        );
        assert_eq!(connection_close.ack_sequence_number, None);
        assert_eq!(connection_close.sequence_number, Some(5));
    }

    #[test]
    fn parse_stream0_default_signaling_decodes_default_packet_rlp_nak() {
        let packet = encode_reliable_default_signaling_packet(
            cdma_common::hrpd::traffic::DEFAULT_PACKET_STREAM2_APPLICATION_PROTOCOL_TYPE,
            &[0x02, 0x01, 0x01, 0x23, 0x45, 0x00, 0x3c],
            6,
        );

        let parsed = parse_stream0_default_signaling(&packet).expect("RLP Nak should parse");

        assert_eq!(parsed.ack_sequence_number, None);
        assert_eq!(parsed.sequence_number, Some(6));
        assert_eq!(
            parsed.message,
            Some(HrpdAccessMessage::DefaultPacketRlpNak(
                cdma_common::hrpd::air::HrpdDefaultPacketRlpNak {
                    requests: vec![cdma_common::hrpd::air::HrpdDefaultPacketRlpNakRequest {
                        first_erased: 0x1_2345,
                        window_len: 0x003c,
                    }],
                },
            ))
        );
    }

    #[test]
    fn reliable_unhandled_stream0_traffic_is_acknowledged() {
        let mut controller = HrpdAirController::with_sector(26, 0, None);
        let mut allocator = allocator();
        let _ = controller
            .handle_access_indication(
                &indication(vec![HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
                    transaction_id: 0x9c,
                })]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::UatiComplete(HrpdUatiComplete {
                    message_sequence: 0,
                    upper_old_uati: Vec::new(),
                    reserved_zero: true,
                })]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::ConnectionRequest(
                    HrpdConnectionRequest {
                        transaction_id: 0x21,
                        request_reason: 0,
                        reserved_zero: true,
                    },
                )]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller.handle_traffic_event(&HrpdTrafficEvent::Drc {
            uati: 0x1a05_8001,
            mac_index: 5,
            slot: 100,
            drc_index: 0xb,
        });

        let packet = encode_reliable_default_signaling_packet(0x7f, &[0xaa], 5);
        let outcome = controller.handle_traffic_event(&HrpdTrafficEvent::Stream0Signaling {
            uati: 0x1a05_8001,
            payload: packet,
        });

        assert_eq!(
            outcome.decoded_stream0_messages,
            vec![HrpdAccessMessage::Unknown {
                protocol_type: 0x7f,
                message_id: Some(0xaa),
                payload: vec![0xaa],
            }]
        );
        assert_eq!(outcome.forward_traffic.len(), 1);
        assert_eq!(outcome.forward_traffic[0].uati, 0x1a05_8001);
        assert_eq!(outcome.forward_traffic[0].mac_index, 5);
        assert_eq!(outcome.forward_traffic[0].payload_bits.len(), 3072);
    }

    #[test]
    fn stream0_ack_only_is_tracked_without_decoded_message_delivery() {
        let mut controller = HrpdAirController::with_sector(26, 0, None);
        let mut allocator = allocator();
        let _ = controller
            .handle_access_indication(
                &indication(vec![HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
                    transaction_id: 0x9c,
                })]),
                &mut allocator,
            )
            .unwrap();
        let _ = controller
            .handle_access_indication(
                &uati_indication(vec![HrpdAccessMessage::UatiComplete(HrpdUatiComplete {
                    message_sequence: 0,
                    upper_old_uati: Vec::new(),
                    reserved_zero: true,
                })]),
                &mut allocator,
            )
            .unwrap();

        let packet = cdma_common::hrpd::air::encode_default_signaling_slp_d_ack_packet(6);
        let outcome = controller.handle_traffic_event(&HrpdTrafficEvent::Stream0Signaling {
            uati: 0x1a05_8001,
            payload: packet,
        });

        assert_eq!(outcome.stream0_ack_only_count, 1);
        assert!(outcome.decoded_stream0_messages.is_empty());
    }

    #[test]
    fn stream1_traffic_event_is_authorized_only_for_known_uati() {
        let mut controller = HrpdAirController::new(26);
        let mut allocator = allocator();
        controller
            .handle_access_indication(
                &indication(vec![HrpdAccessMessage::UatiRequest(HrpdUatiRequest {
                    transaction_id: 0x9c,
                })]),
                &mut allocator,
            )
            .unwrap();

        let unknown = controller.handle_traffic_event(&HrpdTrafficEvent::Stream1Packet {
            uati: 0x0005_8002,
            sequence: 0,
            payload: vec![0x7e],
            decoded_at: None,
            air_frame_end_received_at: None,
        });
        assert_eq!(unknown.a8_uplink.len(), 0);
        assert_eq!(unknown.unknown_session_count, 1);

        let known = controller.handle_traffic_event(&HrpdTrafficEvent::Stream1Packet {
            uati: 0x1a05_8001,
            sequence: 0,
            payload: vec![0x7e, 0xff, 0x03],
            decoded_at: None,
            air_frame_end_received_at: None,
        });
        assert_eq!(known.accepted_event_count, 1);
        assert_eq!(known.a8_uplink.len(), 1);
        assert_eq!(known.a8_uplink[0].payload, [0x7e, 0xff, 0x03]);
    }

    #[test]
    fn default_packet_rlp_receiver_reorders_and_suppresses_duplicates() {
        let now = Instant::now();
        let mut receiver = DefaultPacketRlpReceiver::default();

        let ahead = receiver.ingest(3, b"def", now);
        assert!(ahead.delivered.is_empty());
        assert_eq!(
            ahead.nak_requests,
            [cdma_common::hrpd::air::HrpdDefaultPacketRlpNakRequest {
                first_erased: 0,
                window_len: 3,
            }]
        );

        let recovered = receiver.ingest(0, b"abc", now + Duration::from_millis(10));
        assert_eq!(recovered.delivered, b"abcdef");
        assert_eq!(receiver.v_n, 6);
        assert_eq!(receiver.v_r, 6);
        assert!(receiver.resequencing.is_empty());

        let duplicate = receiver.ingest(0, b"abc", now + Duration::from_millis(20));
        assert!(duplicate.delivered.is_empty());
        assert_eq!(duplicate.duplicate_octets, 3);
    }

    #[test]
    fn default_packet_rlp_receiver_aborts_missing_octets_after_500ms() {
        let now = Instant::now();
        let mut receiver = DefaultPacketRlpReceiver::default();
        let ahead = receiver.ingest(2, b"cd", now);
        assert!(ahead.delivered.is_empty());

        let early = receiver.expire(now + Duration::from_millis(499));
        assert!(early.delivered.is_empty());
        assert_eq!(early.aborted_octets, 0);

        let expired = receiver.expire(now + Duration::from_millis(500));
        assert_eq!(expired.aborted_octets, 2);
        assert_eq!(expired.delivered, b"cd");
        assert_eq!(receiver.v_n, 4);
        assert_eq!(receiver.v_r, 4);
    }

    #[test]
    fn default_packet_rlp_receiver_wraps_22_bit_sequence_space() {
        let mut receiver = DefaultPacketRlpReceiver {
            v_r: rlp::SEQUENCE_MASK - 1,
            v_n: rlp::SEQUENCE_MASK - 1,
            ..Default::default()
        };
        let outcome = receiver.ingest(rlp::SEQUENCE_MASK - 1, b"abc", Instant::now());
        assert_eq!(outcome.delivered, b"abc");
        assert_eq!(receiver.v_n, 1);
        assert_eq!(receiver.v_r, 1);
    }

    #[test]
    fn default_packet_rlp_nak_payload_matches_wire_fields() {
        let payload = default_packet_rlp_nak_payload(&[
            cdma_common::hrpd::air::HrpdDefaultPacketRlpNakRequest {
                first_erased: 0x32_4567,
                window_len: 0x1234,
            },
        ]);
        assert_eq!(payload, [0x02, 0x01, 0x32, 0x45, 0x67, 0x12, 0x34]);
        assert_eq!(
            parse_default_packet_rlp_nak(&payload).unwrap().requests,
            [cdma_common::hrpd::air::HrpdDefaultPacketRlpNakRequest {
                first_erased: 0x32_4567,
                window_len: 0x1234,
            }]
        );
    }
}
