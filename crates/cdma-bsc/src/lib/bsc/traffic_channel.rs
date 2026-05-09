use std::{
    collections::VecDeque,
    net::SocketAddr,
    time::{Duration, Instant},
};

use log::{debug, warn};

use cdma_abis::control::typed::CallConnectionReference;
use cdma_common::traffic::{RC1_TRAFFIC_INITIAL_GAIN_LINEAR, RC3_TRAFFIC_INITIAL_GAIN_LINEAR};
use tokio::{
    sync::{mpsc, oneshot},
    task::JoinHandle,
};
use uuid::Uuid;

use crate::abis_edge::BtsTrafficChannelHandle;
use crate::packet::PacketBearerFrame;
use crate::power_control::{
    ForwardPowerControlState, PowerControlState, TrafficChannelPowerSnapshot,
};

use super::{A1ClearState, VoiceAlertMode, VoiceLegRole};

pub(crate) const VOICE_TRAFFIC_CON_REF: u8 = 1;
pub(crate) const VOICE_REPLACEMENT_CON_REF: u8 = 0;
pub(crate) const VOICE_TRAFFIC_SR_ID: u8 = 2;

/// Request to pin or clear the reverse inner-loop target on an active
/// traffic channel.
pub struct TrafficPowerOverrideRequest {
    pub walsh_code: u8,
    pub action: TrafficPowerOverrideAction,
    pub response_tx: oneshot::Sender<Result<TrafficChannelPowerSnapshot, String>>,
}

pub enum TrafficPowerOverrideAction {
    SetTargetEbNtDb(f32),
    Clear,
}

/// Unified traffic channel state machine.
///
/// Every traffic channel progresses through a common acquisition prefix.
/// Full service negotiation uses ServiceConnecting; legacy CAM
/// ASSIGN_MODE=000 can advance directly after the MS Ack.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum ChannelState {
    /// ECAM sent, waiting for MS preamble.
    Assigned { assigned_at: Instant },
    /// BS Ack sent after preamble. Service negotiation begins once that BS Ack
    /// is acknowledged on the reverse traffic channel, regardless of which
    /// reverse message carries the matching ACK_SEQ.
    WaitingMsAck { bs_ack_sent_at: Instant },
    /// Service Request sent for SO negotiation. Waiting for Service Response.
    WaitingServiceResponse { sr_sent_at: Instant },
    /// Service Connect sent. Waiting for SCC.
    ServiceConnecting { sc_sent_at: Instant },
    /// Service negotiated. Terminal for SMS/SO6 and packet data.
    Active,
    /// AWIM sent, alerting/ringing.
    VoiceAlerting {
        awim_sent_at: Instant,
        mode: VoiceAlertMode,
    },
    /// Voice conversation active.
    VoiceConnected { bridged: bool },
    /// SMS Cause Code sent, waiting for MS Ack before release.
    SmsPendingRelease,
    /// Release Order sent.
    Releasing { release_sent_at: Instant },
}

impl ChannelState {
    pub(crate) fn label(&self) -> &'static str {
        match self {
            ChannelState::Assigned { .. } => "Assigned",
            ChannelState::WaitingMsAck { .. } => "WaitingMsAck",
            ChannelState::WaitingServiceResponse { .. } => "WaitingServiceResponse",
            ChannelState::ServiceConnecting { .. } => "ServiceConnecting",
            ChannelState::Active => "Active",
            ChannelState::VoiceAlerting { .. } => "VoiceAlerting",
            ChannelState::VoiceConnected { .. } => "VoiceConnected",
            ChannelState::SmsPendingRelease => "SmsPendingRelease",
            ChannelState::Releasing { .. } => "Releasing",
        }
    }

    pub(crate) fn is_service_negotiated(&self) -> bool {
        matches!(
            self,
            ChannelState::Active
                | ChannelState::VoiceAlerting { .. }
                | ChannelState::VoiceConnected { .. }
                | ChannelState::SmsPendingRelease
                | ChannelState::Releasing { .. }
        )
    }
}

#[derive(Debug, Clone)]
pub(crate) struct PrimaryTrafficFrame {
    pub(crate) bits: Vec<u8>,
    pub(crate) rate_bps: u32,
    pub(crate) received_at: Instant,
}

pub(crate) enum VoicePollAction {
    None,
    ReleaseUnbridged,
    Teardown {
        reason: &'static str,
        timeout_ms: u64,
    },
}

pub(crate) enum TrafficChannelAction {
    None,
    Teardown {
        reason: &'static str,
        timeout_ms: u64,
    },
}

pub(crate) fn traffic_channel_power_snapshot(
    tc: &TrafficChannelInfo,
) -> TrafficChannelPowerSnapshot {
    TrafficChannelPowerSnapshot {
        target_eb_nt_db: tc.power_control.target_eb_nt_db,
        effective_target_eb_nt_db: tc.power_control.effective_target_eb_nt_db(),
        manual_target_override_db: tc.power_control.manual_target_override_db,
        last_pcg_snr_db: tc.power_control.last_pcg_snr_db,
        last_active_pcg_mask: tc.power_control.last_active_pcg_mask,
        last_pcbs: tc.power_control.last_committed_pcbs,
        reverse_pilot_ec_io_db: tc.power_control.reverse_pilot_ec_io_db,
        fer_pct: tc.power_control.last_fer_pct,
        frames_total: tc.power_control.total_frames_received,
        frames_crc_error: tc.power_control.total_frames_crc_error,
        forward_gain_offset_db: tc.forward_power_control.gain_offset_db,
        forward_last_fer_pct: tc.forward_power_control.last_reported_fer_pct,
        forward_last_pmrm_errors: tc.forward_power_control.last_pmrm_errors as u32,
        forward_last_pmrm_frames: tc.forward_power_control.last_pmrm_frames as u32,
        forward_pmrm_count: tc.forward_power_control.total_pmrm_count,
        forward_pilot_ec_io_db: tc
            .forward_power_control
            .last_pmrm_pilot_strengths
            .iter()
            .map(|&raw| ForwardPowerControlState::pilot_strength_raw_to_ec_io_db(raw))
            .collect(),
        last_pcg_pilot_ec_nt_db: tc.power_control.last_pcg_pilot_ec_nt_db,
        reverse_radio_config: tc.rev_rc as u32,
        power_history: tc.power_control.power_history.iter().cloned().collect(),
    }
}

/// Service negotiation context established by the Channel Assignment form.
///
/// Legacy CAM ASSIGN_MODE=000 disables full service negotiation; ECAM and
/// CAM ASSIGN_MODE=100 enable it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum ServiceNegotiationMode {
    ServiceOptionNegotiation,
    ServiceNegotiation,
}

impl ServiceNegotiationMode {
    pub(crate) fn label(self) -> &'static str {
        match self {
            ServiceNegotiationMode::ServiceOptionNegotiation => "serv_neg_disabled",
            ServiceNegotiationMode::ServiceNegotiation => "serv_neg_enabled",
        }
    }
}

/// Per-mobile traffic channel state.
pub(crate) struct TrafficChannelInfo {
    /// Spec-faithful Abis call connection reference (A.S0003-A section 6.2.2.29).
    /// BSC-generated when the channel is allocated; used as the primary
    /// identifier in Abis control messages.
    pub(crate) call_connection_ref: CallConnectionReference,
    pub(crate) walsh_code: u8,
    pub(crate) service_option: u16,
    /// The SO the mobile originally requested (before BSC override).
    /// When this differs from `service_option`, Service Request negotiation is needed.
    pub(crate) origination_service_option: Option<u16>,
    /// Service Reference Identifier used in Service Connect for this channel.
    pub(crate) service_ref_id: u8,
    /// Pending or active voice Service Option connection on this Fundamental
    /// Channel. cdma2000 permits multiple service option connections in one
    /// service configuration; SO33 packet data remains `service_option`.
    pub(crate) voice_service_option: Option<u16>,
    /// Connection reference for the voice Service Option connection, when
    /// `voice_service_option` is present.
    pub(crate) voice_connection_ref: Option<u8>,
    /// Service Reference Identifier for the voice Service Option connection,
    /// when `voice_service_option` is present.
    pub(crate) voice_service_ref_id: Option<u8>,
    pub(crate) bearer_id: u32,
    pub(crate) for_rc: u8,
    pub(crate) rev_rc: u8,
    pub(crate) rc_label: &'static str,
    pub(crate) service_negotiation_mode: ServiceNegotiationMode,
    pub(crate) power_control_delay_pcgs: u64,
    pub(crate) last_forward_enqueue_at: Option<Instant>,
    pub(crate) channel_state: ChannelState,
    /// Reverse regular-PDU duplicate status per C.S0004-E 3.2.1.1.2.2.
    ///
    /// When a new reverse regular PDU with MSG_SEQ=n is processed, the BS marks
    /// slot n as received and clears slot (n + 4) mod 8.
    ///
    /// Per C.S0004-E 3.2.2.1.2.2, the mobile maintains separate MSG_SEQ counters
    /// for assured (ack_req=1) vs unassured (ack_req=0) PDUs, so the BS must
    /// track them independently to avoid false duplicate detection.
    pub(crate) reverse_regular_msg_seq_rcvd_ack: [bool; 8],
    pub(crate) reverse_regular_msg_seq_rcvd_noack: [bool; 8],
    /// Forward-link MSG_SEQ counter for assured PDUs (ack_req=1).
    pub(crate) forward_msg_seq_ack: u8,
    /// Forward-link MSG_SEQ counter for unassured PDUs (ack_req=0).
    pub(crate) forward_msg_seq_noack: u8,
    /// Owning voice session, if this traffic channel is part of a voice call.
    pub(crate) voice_session_id: Option<Uuid>,
    /// Caller/callee role within the voice session.
    pub(crate) voice_leg_role: Option<VoiceLegRole>,
    /// MSC call correlation for channels controlled through the A1 seam.
    pub(crate) a1_call_id: Option<u64>,
    /// Circuit identity assigned by MSC in AssignmentRequest. Used for voice
    /// bearer relay between BSC and MSC.
    pub(crate) msc_circuit_id: Option<u16>,
    /// Local socket address of this circuit's bearer session, for inclusion
    /// in the A2p Bearer Session-Level Parameters IE in AssignmentComplete.
    pub(crate) msc_bearer_local_addr: Option<SocketAddr>,
    /// Current A1 clear/teardown progression for this radio leg.
    pub(crate) a1_clear_state: A1ClearState,
    /// Last time any activity occurred on this channel (RX message received,
    /// TX frame sent, signaling exchanged). Used for inactivity-based teardown.
    pub(crate) last_activity_at: Instant,
    /// Recent primary-traffic payloads received on this traffic channel.
    pub(crate) recent_primary_frames: VecDeque<PrimaryTrafficFrame>,
    /// Packet session ID for the PCF-owned packet-data session (SO 7/33).
    pub(crate) packet_session_id: Option<String>,
    /// Sender for uplink frames to the PCF bearer.
    pub(crate) packet_uplink_tx: Option<mpsc::Sender<PacketBearerFrame>>,
    /// Handle for the task that reads downlink frames from the PCF bearer.
    pub(crate) packet_downlink_task: Option<JoinHandle<()>>,
    /// Closed-loop reverse power control state. See `PowerControlState`
    /// and `docs/power-control.md`.
    pub(crate) power_control: PowerControlState,
    /// Closed-loop forward power control state, driven by PMRMs from the
    /// mobile. See `ForwardPowerControlState` and `docs/power-control.md`.
    pub(crate) forward_power_control: ForwardPowerControlState,
    /// F-SCH W(32) Walsh code when supplemental channel is allocated, or None.
    pub(crate) sch_walsh_code: Option<u8>,
    /// F-SCH bearer id when supplemental channel is allocated, or None.
    pub(crate) sch_bearer_id: Option<u32>,
}

impl TrafficChannelInfo {
    /// Construct a new traffic channel in the `Assigned` state with sensible
    /// defaults for all bookkeeping fields. Power control parameters are
    /// selected automatically based on the negotiated radio configuration.
    pub(crate) fn new(
        call_connection_ref: CallConnectionReference,
        handle: BtsTrafficChannelHandle,
        service_option: u16,
        origination_service_option: Option<u16>,
        service_ref_id: u8,
        service_negotiation_mode: ServiceNegotiationMode,
        voice_session_id: Option<Uuid>,
        voice_leg_role: Option<VoiceLegRole>,
        a1_call_id: Option<u64>,
    ) -> Self {
        let use_rc3 = handle.for_rc >= 3;
        Self {
            call_connection_ref,
            walsh_code: handle.walsh_code,
            service_option,
            origination_service_option,
            service_ref_id,
            voice_service_option: None,
            voice_connection_ref: None,
            voice_service_ref_id: None,
            bearer_id: handle.bearer_id,
            for_rc: handle.for_rc,
            rev_rc: handle.rev_rc,
            rc_label: handle.rc_label,
            service_negotiation_mode,
            power_control_delay_pcgs: handle.power_control_delay_pcgs,
            last_forward_enqueue_at: None,
            channel_state: ChannelState::Assigned {
                assigned_at: Instant::now(),
            },
            reverse_regular_msg_seq_rcvd_ack: [false; 8],
            reverse_regular_msg_seq_rcvd_noack: [false; 8],
            forward_msg_seq_ack: 0,
            forward_msg_seq_noack: 0,
            voice_session_id,
            voice_leg_role,
            a1_call_id,
            msc_circuit_id: None,
            msc_bearer_local_addr: None,
            a1_clear_state: A1ClearState::Idle,
            last_activity_at: Instant::now(),
            recent_primary_frames: VecDeque::new(),
            packet_session_id: None,
            packet_uplink_tx: None,
            packet_downlink_task: None,
            power_control: if use_rc3 {
                PowerControlState::new_rc3()
            } else {
                PowerControlState::new_rc1()
            },
            forward_power_control: ForwardPowerControlState::new(if use_rc3 {
                RC3_TRAFFIC_INITIAL_GAIN_LINEAR
            } else {
                RC1_TRAFFIC_INITIAL_GAIN_LINEAR
            }),
            sch_walsh_code: None,
            sch_bearer_id: None,
        }
    }

    /// Reset the reverse ARQ duplicate-detection state for this traffic channel
    /// per C.S0004-E 3.2.2.1.2.2. Forward ARQ is handled by the BTS traffic LAC.
    pub(crate) fn reset_arq(&mut self) {
        self.reverse_regular_msg_seq_rcvd_ack = [false; 8];
        self.reverse_regular_msg_seq_rcvd_noack = [false; 8];
    }

    pub(crate) fn push_primary_frame(&mut self, bits: &[u8], rate_bps: u32) {
        const MAX_RECENT_PRIMARY_FRAMES: usize = 32;
        if self.recent_primary_frames.len() >= MAX_RECENT_PRIMARY_FRAMES {
            self.recent_primary_frames.pop_front();
        }
        self.recent_primary_frames.push_back(PrimaryTrafficFrame {
            bits: bits.to_vec(),
            rate_bps,
            received_at: Instant::now(),
        });
    }

    pub(crate) fn next_forward_msg_seq(&mut self, ack_req: bool) -> u8 {
        if ack_req {
            let seq = self.forward_msg_seq_ack;
            self.forward_msg_seq_ack = (seq + 1) % 8;
            seq
        } else {
            let seq = self.forward_msg_seq_noack;
            self.forward_msg_seq_noack = (seq + 1) % 8;
            seq
        }
    }

    pub(crate) fn is_releasing(&self) -> bool {
        matches!(self.channel_state, ChannelState::Releasing { .. })
    }

    pub(crate) fn state_label(&self) -> &'static str {
        self.channel_state.label()
    }

    pub(crate) fn is_assigned(&self) -> bool {
        matches!(self.channel_state, ChannelState::Assigned { .. })
    }

    pub(crate) fn is_waiting_ms_ack(&self) -> bool {
        matches!(self.channel_state, ChannelState::WaitingMsAck { .. })
    }

    pub(crate) fn is_waiting_service_response(&self) -> bool {
        matches!(
            self.channel_state,
            ChannelState::WaitingServiceResponse { .. }
        )
    }

    pub(crate) fn is_service_connecting(&self) -> bool {
        matches!(self.channel_state, ChannelState::ServiceConnecting { .. })
    }

    pub(crate) fn is_voice_connected(&self) -> bool {
        matches!(self.channel_state, ChannelState::VoiceConnected { .. })
    }

    pub(crate) fn is_sms_pending_release(&self) -> bool {
        matches!(self.channel_state, ChannelState::SmsPendingRelease)
    }

    pub(crate) fn is_negotiating(&self) -> bool {
        matches!(
            self.channel_state,
            ChannelState::Assigned { .. }
                | ChannelState::WaitingMsAck { .. }
                | ChannelState::ServiceConnecting { .. }
        )
    }

    pub(crate) fn latest_activity_for_idle_check(
        &self,
        last_bts_enqueue_at: Option<Instant>,
    ) -> Instant {
        if self.is_negotiating() {
            return self.last_activity_at;
        }

        self.last_forward_enqueue_at
            .or(last_bts_enqueue_at)
            .filter(|tx_at| *tx_at > self.last_activity_at)
            .unwrap_or(self.last_activity_at)
    }

    pub(crate) fn traffic_lifecycle_action(
        &self,
        ms_ack_timeout: Duration,
        now: Instant,
    ) -> TrafficChannelAction {
        match self.channel_state {
            ChannelState::WaitingMsAck { bs_ack_sent_at }
                if now.duration_since(bs_ack_sent_at) >= ms_ack_timeout =>
            {
                TrafficChannelAction::Teardown {
                    reason: "MS Ack timeout",
                    timeout_ms: ms_ack_timeout.as_millis() as u64,
                }
            }
            _ => TrafficChannelAction::None,
        }
    }

    pub(crate) fn next_traffic_lifecycle_deadline(
        &self,
        ms_ack_timeout: Duration,
    ) -> Option<Instant> {
        match self.channel_state {
            ChannelState::WaitingMsAck { bs_ack_sent_at } => Some(bs_ack_sent_at + ms_ack_timeout),
            _ => None,
        }
    }

    pub(crate) fn voice_poll_action(
        &self,
        service_connect_timeout: Duration,
        release_timeout: Duration,
    ) -> VoicePollAction {
        match self.channel_state {
            ChannelState::Assigned { assigned_at }
                if assigned_at.elapsed() >= service_connect_timeout =>
            {
                VoicePollAction::Teardown {
                    reason: "voice assignment timeout",
                    timeout_ms: service_connect_timeout.as_millis() as u64,
                }
            }
            ChannelState::ServiceConnecting { sc_sent_at }
                if sc_sent_at.elapsed() >= service_connect_timeout =>
            {
                VoicePollAction::Teardown {
                    reason: "service connect timeout",
                    timeout_ms: service_connect_timeout.as_millis() as u64,
                }
            }
            ChannelState::VoiceConnected { bridged: false } => VoicePollAction::ReleaseUnbridged,
            ChannelState::Releasing { release_sent_at }
                if release_sent_at.elapsed() >= release_timeout =>
            {
                VoicePollAction::Teardown {
                    reason: "voice release timeout",
                    timeout_ms: release_timeout.as_millis() as u64,
                }
            }
            _ => VoicePollAction::None,
        }
    }

    pub(crate) fn next_voice_poll_deadline(
        &self,
        service_connect_timeout: Duration,
        release_timeout: Duration,
        connected_poll_interval: Duration,
        now: Instant,
    ) -> Option<Instant> {
        match self.channel_state {
            ChannelState::Assigned { assigned_at } => Some(assigned_at + service_connect_timeout),
            ChannelState::ServiceConnecting { sc_sent_at } => {
                Some(sc_sent_at + service_connect_timeout)
            }
            ChannelState::VoiceConnected { bridged: false } => Some(now + connected_poll_interval),
            ChannelState::Releasing { release_sent_at } => Some(release_sent_at + release_timeout),
            _ => None,
        }
    }

    fn set_channel_state(&mut self, next: ChannelState) {
        if matches!(self.channel_state, ChannelState::Releasing { .. }) {
            warn!(
                "traffic_channel: transition out of Releasing is invalid \
                 walsh={} {:?} -> {:?}",
                self.walsh_code,
                self.channel_state.label(),
                next.label(),
            );
        } else {
            debug!(
                "traffic_channel: walsh={} {} -> {}",
                self.walsh_code,
                self.channel_state.label(),
                next.label(),
            );
        }
        self.channel_state = next;
    }

    pub(crate) fn mark_waiting_ms_ack(&mut self) {
        self.set_channel_state(ChannelState::WaitingMsAck {
            bs_ack_sent_at: Instant::now(),
        });
    }

    pub(crate) fn mark_assigned(&mut self) {
        self.set_channel_state(ChannelState::Assigned {
            assigned_at: Instant::now(),
        });
    }

    pub(crate) fn mark_waiting_service_response(&mut self) {
        self.set_channel_state(ChannelState::WaitingServiceResponse {
            sr_sent_at: Instant::now(),
        });
    }

    pub(crate) fn mark_service_connecting(&mut self) {
        self.set_channel_state(ChannelState::ServiceConnecting {
            sc_sent_at: Instant::now(),
        });
    }

    pub(crate) fn mark_active(&mut self) {
        self.set_channel_state(ChannelState::Active);
    }

    pub(crate) fn mark_voice_alerting(&mut self, mode: VoiceAlertMode) {
        self.set_channel_state(ChannelState::VoiceAlerting {
            awim_sent_at: Instant::now(),
            mode,
        });
    }

    pub(crate) fn mark_voice_connected(&mut self, bridged: bool) {
        self.set_channel_state(ChannelState::VoiceConnected { bridged });
    }

    pub(crate) fn mark_sms_pending_release(&mut self) {
        self.set_channel_state(ChannelState::SmsPendingRelease);
    }

    pub(crate) fn mark_releasing(&mut self) {
        self.set_channel_state(ChannelState::Releasing {
            release_sent_at: Instant::now(),
        });
    }

    pub(crate) fn clear_voice_service_connection(&mut self) {
        self.voice_service_option = None;
        self.voice_connection_ref = None;
        self.voice_service_ref_id = None;
        self.voice_session_id = None;
        self.voice_leg_role = None;
        self.a1_call_id = None;
        self.a1_clear_state = A1ClearState::Idle;
    }

    pub(crate) fn mark_a1_clear_request_sent(&mut self) {
        self.a1_clear_state = A1ClearState::ClearRequestSent;
    }
}

impl std::fmt::Debug for TrafficChannelInfo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("TrafficChannelInfo")
            .field("walsh_code", &self.walsh_code)
            .field("service_option", &self.service_option)
            .field("service_negotiation_mode", &self.service_negotiation_mode)
            .field("channel_state", &self.channel_state)
            .finish()
    }
}
