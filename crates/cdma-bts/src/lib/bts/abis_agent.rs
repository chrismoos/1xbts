//! BTS-side Abis agent: consumes decoded Abis control messages from the
//! TCP transport, dispatches to `TrafficResourceService`, and produces
//! response messages.
//!
//! One `AbisAgent` per BTS peer. It owns per-CCR session state and drives
//! `TrafficSetupProcedure` / `TrafficReleaseProcedure` instances for each
//! active call connection.

use std::collections::HashMap;
use std::sync::Arc;

use log::{info, warn};

use cdma_abis::control::typed::{
    A3ConnectInformation, BurstCommitMessage, BurstRequestMessage, BurstResponseMessage, CellId,
    CellIdWithMscId, CellInfoRecord, ChannelElementStatus, CorrelationId, ForwardBurstRadioInfo,
    PhysicalChannelInfo, PhysicalChannelType, PilotGatingRate, TrafficChannelStatusMessage,
    TrafficCircuitId,
};
use cdma_abis::control::{
    AbisMessage, AbisTimerKind, BtsReleaseAckMessage, BtsReleaseMessage, BtsSetupAckMessage,
    BtsSetupMessage, CallConnectionReference, ConnectAckMessage, ConnectMessage, MessageType,
    PchMessageTransferAckMessage, PchMessageTransferMessage, RemoveAckMessage, RemoveMessage,
    TrafficReleaseProcedure, TrafficSetupProcedure, TrafficSetupState, TrafficSetupTimeoutAction,
};
use cdma_abis::control::{decode, encode};

use parking_lot::Mutex;

use super::paging_supplier::{PagingRetryEvent, PagingSupplierState, PendingPageRecord};
use super::resource_controller::TrafficResourceService;
use super::traffic_lac::{TrafficArqConfig, TrafficChannelArqState, TrafficLacEvent};
use crate::bts::AccessChannelEvent;
use crate::lac::message_types::{MessageId, WireChannel};
use crate::lac::paging_messages::{
    CHANNEL_ASSIGN_MODE_EXTENDED_TRAFFIC, CHANNEL_ASSIGN_MODE_TRAFFIC,
    CHANNEL_DEFAULT_CONFIG_RC1_RC1, CHANNEL_DEFAULT_CONFIG_RC2_RC2, ChannelAssignmentGrantedMode,
    ChannelAssignmentMessage, ExtendedChannelAssignmentMessage, GeneralPageMessage, MsAddress,
    MsPageAddress,
};
use crate::phy::coding::long_code::LongCodeGenerator;
use crate::receiver::access_layer3::AccessMessage;
use cdma_common::bits::Bitstream;
use cdma_common::consts::reverse_fch_gating_supported;
use cdma_common::sch::Rc3FschProfile;

pub(crate) fn abis_message_from_typed(bytes: &[u8]) -> Option<AbisMessage> {
    match decode(bytes) {
        Ok(msg) => Some(msg),
        Err(e) => {
            warn!("abis_agent: failed to build AbisMessage from typed: {e}");
            None
        }
    }
}

fn profile_from_sch_rate_index(index: u8) -> Option<Rc3FschProfile> {
    match index {
        0x1 => Rc3FschProfile::from_rate_bps(19_200),
        0x2 => Rc3FschProfile::from_rate_bps(38_400),
        0x3 => Rc3FschProfile::from_rate_bps(76_800),
        0x4 => Rc3FschProfile::from_rate_bps(153_600),
        _ => None,
    }
}

pub(super) fn cam_radio_config(cam: &ChannelAssignmentMessage) -> Result<(u8, u8), String> {
    match (cam.assign_mode, cam.granted_mode) {
        (CHANNEL_ASSIGN_MODE_TRAFFIC, _) => Ok((1, 1)),
        (CHANNEL_ASSIGN_MODE_EXTENDED_TRAFFIC, Some(mode))
            if mode == ChannelAssignmentGrantedMode::DefaultConfiguration as u8 =>
        {
            match cam.default_config {
                Some(CHANNEL_DEFAULT_CONFIG_RC1_RC1) => Ok((1, 1)),
                Some(CHANNEL_DEFAULT_CONFIG_RC2_RC2) => Ok((2, 2)),
                other => Err(format!("unsupported CAM DEFAULT_CONFIG={other:?}")),
            }
        }
        (CHANNEL_ASSIGN_MODE_EXTENDED_TRAFFIC, Some(mode))
            if mode == ChannelAssignmentGrantedMode::RequestedService as u8
                && cam.default_config == Some(CHANNEL_DEFAULT_CONFIG_RC1_RC1) =>
        {
            Ok((2, 2))
        }
        _ => Err(format!(
            "unsupported CAM ASSIGN_MODE=0b{:03b} GRANTED_MODE={:?}",
            cam.assign_mode, cam.granted_mode
        )),
    }
}

/// Per-CCR session tracking on the BTS side.
struct Session {
    walsh_code: u8,
    esn: u32,
    /// Long code generator, stored at reservation time and consumed when
    /// the traffic channel is committed (after ECAM decode).
    lc_gen: Option<LongCodeGenerator>,
    /// `true` once the forward TX channel + reverse RX have been created
    /// (i.e. after ECAM/CAM decode). Before that, only the walsh code is
    /// reserved.
    committed: bool,
    setup: TrafficSetupProcedure,
    release: TrafficReleaseProcedure,
    traffic_lac: Option<TrafficChannelArqState>,
    /// F-SCH code allocated through the Abis Burst path for this FCH session,
    /// or `None` before supplemental allocation. Freed when the session is
    /// removed.
    sch_walsh_code: Option<u8>,
}

/// Events emitted by the agent to BTS-internal consumers.
#[derive(Debug)]
pub enum AbisAgentEvent {
    /// A traffic channel was successfully set up.
    TrafficConnected {
        ccr: CallConnectionReference,
        walsh_code: u8,
    },
    /// A traffic channel was released and deallocated.
    TrafficReleased {
        ccr: CallConnectionReference,
        walsh_code: u8,
    },
    /// The BTS initiated a release toward the BSC for the given session.
    BtsReleaseInitiated {
        ccr: CallConnectionReference,
        walsh_code: u8,
    },
    /// Forward traffic signaling frames ready for air-interface transmission.
    ForwardTrafficFrames {
        walsh_code: u8,
        frames: Vec<cdma_common::bits::Bitstream>,
    },
    /// A paging channel directed SDU failed delivery after exhausting retries.
    /// The caller should send the encoded Abis response messages.
    PagingRetryFailed { responses: Vec<AbisMessage> },
}

/// Configuration for the BTS Abis agent.
#[derive(Debug, Clone)]
pub struct AbisAgentConfig {
    /// BTS pilot PN offset (0..511), used in Connect information.
    pub pilot_pn: u16,
    /// Cell identifier (cell_id, sector) for this BTS.
    pub cell_id: CellId,
    /// MSC identifier used in TrafficChannelStatus reports.
    pub mscid: u32,
}

/// BTS-side Abis message handler.
///
/// Processes incoming Abis control messages from the BSC, allocates/deallocates
/// BTS resources via `TrafficResourceService`, and returns response messages
/// to send back over the transport.
pub struct AbisAgent {
    config: AbisAgentConfig,
    controller: Arc<TrafficResourceService>,
    sessions: HashMap<CallConnectionReference, Session>,
    paging_state: Option<Arc<Mutex<PagingSupplierState>>>,
}

impl AbisAgent {
    /// Creates a new Abis agent backed by the given resource controller.
    pub fn new(config: AbisAgentConfig, controller: Arc<TrafficResourceService>) -> Self {
        Self {
            config,
            controller,
            sessions: HashMap::new(),
            paging_state: None,
        }
    }

    /// Attach the paging supplier state for L2 ack notification tracking.
    pub fn set_paging_state(&mut self, state: Arc<Mutex<PagingSupplierState>>) {
        self.paging_state = Some(state);
    }

    /// Processes an incoming Abis message and returns response messages to send
    /// back to the BSC, plus any internal events for BTS consumers.
    pub fn handle_message(
        &mut self,
        message: &AbisMessage,
    ) -> (Vec<AbisMessage>, Vec<AbisAgentEvent>) {
        info!("BTS Abis: rx {:?}", message.message_type);
        let mut responses = Vec::new();
        let mut events = Vec::new();

        match message.message_type {
            MessageType::BtsSetup => {
                self.handle_bts_setup(message, &mut responses, &mut events);
            }
            MessageType::ConnectAck => {
                self.handle_connect_ack(message, &mut responses, &mut events);
            }
            MessageType::BtsRelease => {
                self.handle_bts_release(message, &mut responses, &mut events);
            }
            MessageType::Remove => {
                self.handle_remove(message, &mut responses, &mut events);
            }
            MessageType::PchMessageTransfer => {
                self.handle_pch_msg_transfer(message, &mut responses, &mut events);
            }
            MessageType::BurstRequest => {
                self.handle_burst_request(message, &mut responses);
            }
            MessageType::BurstCommit => {
                self.handle_burst_commit(message);
            }
            other => {
                warn!("abis_agent: unhandled message type {:?}", other);
            }
        }

        (responses, events)
    }

    /// Check if an access channel event carries a valid ACK that matches a
    /// pending Abis ack-notify request. Returns a `PchMessageTransferAck`
    /// response message with `bts_l2_termination=true` if so.
    pub fn check_access_ack_notify(&self, event: &AccessChannelEvent) -> Vec<AbisMessage> {
        if !event.valid_ack {
            return Vec::new();
        }
        let Some(ack_seq) = event.ack_seq else {
            return Vec::new();
        };
        let Some(ref paging_state) = self.paging_state else {
            return Vec::new();
        };
        let addr = access_event_to_ms_address(event);
        let Some(addr) = addr else {
            return Vec::new();
        };
        let corr_id = paging_state.lock().check_ack_notify(&addr, ack_seq);
        let Some(corr_id) = corr_id else {
            return Vec::new();
        };
        info!(
            "abis_agent: L2 ack notify correlation_id={} ack_seq={}",
            corr_id, ack_seq
        );
        let l2_ack = PchMessageTransferAckMessage {
            correlation_id: Some(cdma_abis::control::CorrelationId(corr_id)),
            cause: None,
            bts_l2_termination: Some(true),
        };
        match l2_ack.encode() {
            Ok(bytes) => match abis_message_from_typed(&bytes) {
                Some(msg) => vec![msg],
                None => Vec::new(),
            },
            Err(_) => Vec::new(),
        }
    }

    /// Record the ARQ msg_seq from an access probe so the paging supplier
    /// can stamp valid_ack + ack_seq on the next directed SDU to this mobile.
    pub fn record_access_msg_seq(&self, event: &AccessChannelEvent) {
        let Some(msg_seq) = event.msg_seq else {
            return;
        };
        let Some(ref paging_state) = self.paging_state else {
            return;
        };
        let Some(addr) = access_event_to_ms_address(event) else {
            return;
        };
        paging_state.lock().record_received_msg_seq(&addr, msg_seq);
    }

    /// Cancel pending page records when a Page Response is received on the
    /// access channel. Called from the BTS access event handler.
    pub fn check_page_response_cancel(&self, event: &AccessChannelEvent) -> Vec<AbisMessage> {
        let Some(AccessMessage::PageResponse(_)) = event.decoded_l3 else {
            return Vec::new();
        };
        let Some(ref paging_state) = self.paging_state else {
            return Vec::new();
        };
        let page_addr = if let (Some(s1), Some(s2)) = (event.imsi_m_s1, event.imsi_m_s2) {
            Some(MsPageAddress::ImsiS {
                imsi_m_s1: s1,
                imsi_m_s2: s2,
                mcc: event.imsi_mcc,
                imsi_11_12: event.imsi_11_12,
            })
        } else if let Some(esn) = event.esn {
            Some(MsPageAddress::Esn(esn))
        } else {
            None
        };
        if let Some(addr) = page_addr {
            let correlations = paging_state.lock().complete_pages_for_address(&addr);
            if !correlations.is_empty() {
                info!(
                    "abis_agent: completed {} pending page record(s) on Page Response from {:?}",
                    correlations.len(),
                    addr,
                );
            }
            return correlations
                .into_iter()
                .filter_map(|correlation_id| {
                    let ack = PchMessageTransferAckMessage {
                        correlation_id: Some(CorrelationId(correlation_id)),
                        cause: None,
                        bts_l2_termination: Some(true),
                    };
                    ack.encode()
                        .ok()
                        .and_then(|bytes| abis_message_from_typed(&bytes))
                })
                .collect();
        }
        Vec::new()
    }

    /// Returns the number of active sessions.
    pub fn active_session_count(&self) -> usize {
        self.sessions.len()
    }

    /// Returns the Walsh code allocated for a given CCR, if any.
    pub fn walsh_code_for(&self, ccr: &CallConnectionReference) -> Option<u8> {
        self.sessions.get(ccr).map(|s| s.walsh_code)
    }

    /// Builds a BTS-initiated release message for the session on `walsh_code`.
    ///
    /// Returns the encoded `AbisMessage` to send to the BSC and a
    /// `BtsReleaseInitiated` event. The session is **not** removed here;
    /// the BSC will follow up with a `Remove` that `handle_remove` processes.
    pub fn initiate_release(&mut self, walsh_code: u8) -> (Vec<AbisMessage>, Vec<AbisAgentEvent>) {
        let mut responses = Vec::new();
        let mut events = Vec::new();

        let Some((&ccr, _session)) = self
            .sessions
            .iter()
            .find(|(_, s)| s.walsh_code == walsh_code)
        else {
            warn!(
                "abis_agent: initiate_release called for unknown walsh_code {}",
                walsh_code
            );
            return (responses, events);
        };

        let release = BtsReleaseMessage {
            call_connection_reference: ccr,
            cell_identifier_list: None,
            correlation_id: None,
        };
        match release.encode() {
            Ok(bytes) => {
                if let Some(msg) = abis_message_from_typed(&bytes) {
                    info!(
                        "abis_agent: BTS-initiated release for CCR {:?} walsh={}",
                        ccr, walsh_code
                    );
                    responses.push(msg);
                    events.push(AbisAgentEvent::BtsReleaseInitiated { ccr, walsh_code });
                }
            }
            Err(e) => {
                warn!("abis_agent: BtsRelease encode failed: {e}");
            }
        }

        (responses, events)
    }

    /// Processes a timer expiry for the given CCR and timer kind.
    ///
    /// Returns any messages that should be retransmitted. The caller is
    /// responsible for scheduling timers (e.g. via `tokio::time`) and
    /// calling this method when they fire.
    pub fn handle_timer_expiry(
        &mut self,
        ccr: &CallConnectionReference,
        timer: AbisTimerKind,
    ) -> Vec<AbisMessage> {
        let Some(session) = self.sessions.get_mut(ccr) else {
            warn!(
                "abis_agent: timer {:?} fired for unknown CCR {:?}",
                timer, ccr
            );
            return Vec::new();
        };

        match session.setup.on_timer_expiry(timer) {
            Ok(TrafficSetupTimeoutAction::ResendConnect) => {
                info!(
                    "abis_agent: Tconnb expired for CCR {:?}, resending Connect",
                    ccr
                );
                // Rebuild and return the Connect message stored in the session's
                // last outbound. For now, log and return empty — the agent
                // would need to cache the last Connect to resend it.
                warn!("abis_agent: Connect retransmit not yet cached");
                Vec::new()
            }
            Ok(action) => {
                info!(
                    "abis_agent: timer {:?} fired for CCR {:?}, action={:?}",
                    timer, ccr, action
                );
                Vec::new()
            }
            Err(e) => {
                warn!(
                    "abis_agent: timer {:?} expiry rejected for CCR {:?}: {}",
                    timer, ccr, e
                );
                Vec::new()
            }
        }
    }

    /// Returns all active CCRs and the BTS-side timers that the caller should
    /// schedule for each. Call after `handle_message` to know which timers to
    /// arm or rearm.
    pub fn pending_timers(&self) -> Vec<(CallConnectionReference, AbisTimerKind)> {
        let mut timers = Vec::new();
        for (&ccr, session) in &self.sessions {
            match session.setup.state() {
                TrafficSetupState::AwaitingConnectAck => {
                    timers.push((ccr, AbisTimerKind::Tconnb));
                }
                _ => {}
            }
        }
        timers
    }

    fn decode_typed<T, F>(&self, message: &AbisMessage, decode_fn: F) -> Option<T>
    where
        F: FnOnce(&[u8]) -> cdma_abis::Result<T>,
    {
        match encode(message) {
            Ok(bytes) => match decode_fn(&bytes) {
                Ok(typed) => Some(typed),
                Err(e) => {
                    warn!("abis_agent: typed decode failed: {e}");
                    None
                }
            },
            Err(e) => {
                warn!("abis_agent: encode for typed decode failed: {e}");
                None
            }
        }
    }

    fn handle_bts_setup(
        &mut self,
        message: &AbisMessage,
        responses: &mut Vec<AbisMessage>,
        _events: &mut Vec<AbisAgentEvent>,
    ) {
        let Some(setup) = self.decode_typed(message, BtsSetupMessage::decode) else {
            return;
        };
        let ccr = setup.call_connection_reference;
        info!("abis_agent: BtsSetup for CCR {:?}", ccr);

        let esn = setup
            .mobile_identities
            .iter()
            .find_map(|id| match id {
                cdma_abis::control::MobileIdentity::Esn(e) => Some(*e),
                _ => None,
            })
            .unwrap_or(0);

        let lc_gen = LongCodeGenerator::new_traffic_channel(esn);

        // Phase 1: reserve walsh code only — the actual TX/RX channel is
        // created later when the ECAM arrives via PchMessageTransfer.
        let Some(walsh_code) = self.controller.reserve_walsh() else {
            warn!("abis_agent: no Walsh codes available for CCR {:?}", ccr);
            return;
        };

        let sch_walsh_code: Option<u8> = None;
        info!(
            "abis_agent: reserved Walsh code {} for CCR {:?} (channel pending ECAM/SCH burst)",
            walsh_code, ccr
        );

        let mut setup_proc = TrafficSetupProcedure::new(ccr);
        if let Err(e) = setup_proc.start_setup(&setup) {
            warn!("abis_agent: setup procedure start failed: {e}");
            self.controller.deallocate_traffic(walsh_code);
            return;
        }

        let connect_information = vec![A3ConnectInformation {
            physical_channel_type: PhysicalChannelType::Fch,
            new_a3: true,
            cell_info_records: vec![CellInfoRecord {
                cell: self.config.cell_id,
                qof_mask: 0,
                new_cell: true,
                power_combine_indication: false,
                pilot_pn: self.config.pilot_pn,
                code_channel: walsh_code,
            }],
            traffic_circuit_id: TrafficCircuitId {
                traffic_circuit_identifier: walsh_code as u16,
                traffic_connection_identifier: 0,
            },
            extended_handoff_direction_parameters: None,
            channel_element_id: vec![walsh_code],
            a3_originating_id: 1,
            a7_destination_id: 0,
        }];
        let connect = ConnectMessage {
            call_connection_reference: ccr,
            correlation_id: None,
            sdu_id: None,
            connect_information,
            physical_channel_info: setup.physical_channel_info.unwrap_or(PhysicalChannelInfo {
                frame_offset: 0,
                pilot_gating_rate: PilotGatingRate::Full,
                arfcn: 0,
                otd: false,
                physical_channels: vec![PhysicalChannelType::Fch],
            }),
        };

        match connect.encode() {
            Ok(bytes) => {
                if let Some(msg) = abis_message_from_typed(&bytes) {
                    if let Err(e) = setup_proc.on_connect(&connect) {
                        warn!("abis_agent: setup procedure on_connect failed: {e}");
                        self.controller.deallocate_traffic(walsh_code);
                        return;
                    }

                    self.sessions.insert(
                        ccr,
                        Session {
                            walsh_code,
                            esn,
                            lc_gen: Some(lc_gen),
                            committed: false,
                            setup: setup_proc,
                            release: TrafficReleaseProcedure::new(ccr),
                            traffic_lac: None,
                            sch_walsh_code,
                        },
                    );
                    responses.push(msg);
                }
            }
            Err(e) => {
                warn!("abis_agent: Connect encode failed: {e}");
                self.controller.deallocate_traffic(walsh_code);
            }
        }
    }

    fn handle_burst_request(&mut self, message: &AbisMessage, responses: &mut Vec<AbisMessage>) {
        let Some(request) = self.decode_typed(message, BurstRequestMessage::decode) else {
            return;
        };
        let Some(ccr) = request.call_connection_reference else {
            warn!("abis_agent: BurstRequest missing CCR");
            return;
        };
        let Some(request_info) = request.forward_burst_radio_info else {
            warn!(
                "abis_agent: BurstRequest for CCR {:?} missing ForwardBurstRadioInfo",
                ccr
            );
            return;
        };
        let Some(profile) =
            profile_from_sch_rate_index(request_info.forward_supplemental_channel_rate)
        else {
            warn!(
                "abis_agent: BurstRequest for CCR {:?} unsupported F-SCH rate index {}",
                ccr, request_info.forward_supplemental_channel_rate
            );
            return;
        };
        let Some(session) = self.sessions.get_mut(&ccr) else {
            warn!("abis_agent: BurstRequest for unknown CCR {:?}", ccr);
            return;
        };
        if request_info.forward_supplemental_channel_duration == 0 {
            let release_code = if let Some(code) = session.sch_walsh_code.take() {
                self.controller.deallocate_sch(code);
                info!(
                    "abis_agent: released F-SCH W({})={} for CCR {:?}",
                    profile.walsh_len, code, ccr
                );
                code
            } else {
                let code = request_info.forward_code_channel_index as u8;
                warn!(
                    "abis_agent: F-SCH release for CCR {:?} with no active SCH; requested code={}",
                    ccr, code
                );
                code
            };
            let mut response_info: ForwardBurstRadioInfo = request_info;
            response_info.forward_code_channel_index = release_code as u16;
            response_info.pilot_pn_code = self.config.pilot_pn;
            response_info.forward_supplemental_channel_rate = profile.num_bits_idx;
            let response = BurstResponseMessage {
                call_connection_reference: Some(ccr),
                correlation_id: request.correlation_id,
                committed_cell_identifier_list: request
                    .cell_identifier_list
                    .clone()
                    .or_else(|| Some(vec![self.config.cell_id])),
                uncommitted_cell_identifier_list: None,
                forward_burst_radio_info: Some(response_info),
                reverse_burst_radio_info: None,
                abis_destination_id: request.abis_destination_id.clone(),
            };
            if let Ok(bytes) = response.encode()
                && let Some(msg) = abis_message_from_typed(&bytes)
            {
                responses.push(msg);
            }
            return;
        }
        let sch_code = if let Some(code) = session.sch_walsh_code {
            code
        } else {
            let lc_gen = LongCodeGenerator::new_traffic_channel(session.esn);
            let sch_gain_linear = profile.nominal_gain_linear();
            match self.controller.allocate_rc3_sch(
                lc_gen,
                sch_gain_linear,
                profile,
                request_info.start_time_unit,
                request_info.forward_supplemental_channel_start_time,
            ) {
                Some((code, _ch)) => {
                    session.sch_walsh_code = Some(code);
                    info!(
                        "abis_agent: allocated F-SCH W({})={} rate={} gain={:.3}",
                        profile.walsh_len, code, profile.rate_bps, sch_gain_linear
                    );
                    code
                }
                None => {
                    warn!(
                        "abis_agent: BurstRequest for CCR {:?} no W({}) code available",
                        ccr, profile.walsh_len
                    );
                    return;
                }
            }
        };

        let mut response_info: ForwardBurstRadioInfo = request_info;
        response_info.forward_code_channel_index = sch_code as u16;
        response_info.pilot_pn_code = self.config.pilot_pn;
        response_info.forward_supplemental_channel_rate = profile.num_bits_idx;
        let response = BurstResponseMessage {
            call_connection_reference: Some(ccr),
            correlation_id: request.correlation_id,
            committed_cell_identifier_list: request
                .cell_identifier_list
                .clone()
                .or_else(|| Some(vec![self.config.cell_id])),
            uncommitted_cell_identifier_list: None,
            forward_burst_radio_info: Some(response_info),
            reverse_burst_radio_info: None,
            abis_destination_id: request.abis_destination_id.clone(),
        };
        if let Ok(bytes) = response.encode()
            && let Some(msg) = abis_message_from_typed(&bytes)
        {
            info!(
                "abis_agent: BurstResponse CCR {:?} F-SCH W({})={} rate={}",
                ccr, profile.walsh_len, sch_code, profile.rate_bps
            );
            responses.push(msg);
        }
    }

    fn handle_burst_commit(&mut self, message: &AbisMessage) {
        let Some(commit) = self.decode_typed(message, BurstCommitMessage::decode) else {
            return;
        };
        if let Some(ccr) = commit.call_connection_reference {
            info!("abis_agent: BurstCommit for CCR {:?}", ccr);
        }
    }

    fn handle_connect_ack(
        &mut self,
        message: &AbisMessage,
        responses: &mut Vec<AbisMessage>,
        _events: &mut Vec<AbisAgentEvent>,
    ) {
        let Some(ack) = self.decode_typed(message, ConnectAckMessage::decode) else {
            return;
        };
        let ccr = ack.call_connection_reference;

        let Some(session) = self.sessions.get_mut(&ccr) else {
            warn!("abis_agent: ConnectAck for unknown CCR {:?}", ccr);
            return;
        };

        if let Err(e) = session.setup.on_connect_ack(&ack) {
            warn!("abis_agent: setup procedure on_connect_ack failed: {e}");
            return;
        }

        let setup_ack = BtsSetupAckMessage {
            call_connection_reference: ccr,
            connect_information: Vec::new(),
            abis_originating_id: None,
            abis_destination_id: None,
            cause: None,
        };
        if let Ok(bytes) = setup_ack.encode() {
            if let Some(msg) = abis_message_from_typed(&bytes) {
                if let Err(e) = session.setup.on_setup_ack(&setup_ack) {
                    warn!("abis_agent: setup procedure on_setup_ack failed: {e}");
                }
                responses.push(msg);
            }
        }

        let status = TrafficChannelStatusMessage {
            call_connection_reference: ccr,
            cell_identifier_list: vec![CellIdWithMscId {
                mscid: self.config.mscid,
                cell: self.config.cell_id.cell,
                sector: self.config.cell_id.sector,
            }],
            channel_element_status: ChannelElementStatus { transmit_on: true },
            sdu_id: None,
            a3_destination_id: None,
            a7_destination_id: None,
        };
        if let Ok(bytes) = status.encode() {
            if let Some(msg) = abis_message_from_typed(&bytes) {
                responses.push(msg);
            }
        }

        if session.setup.state() == TrafficSetupState::Connected {
            // Traffic LAC is activated once the channel is committed (after
            // ECAM decode). If already committed (shouldn't happen at
            // ConnectAck time in the normal flow), activate it now.
            if session.committed && session.traffic_lac.is_none() {
                session.traffic_lac =
                    Some(TrafficChannelArqState::new(TrafficArqConfig::default()));
            }
            info!(
                "abis_agent: Abis connected for CCR {:?} walsh={} (channel committed={})",
                ccr, session.walsh_code, session.committed
            );
        }
    }

    fn handle_bts_release(
        &mut self,
        message: &AbisMessage,
        responses: &mut Vec<AbisMessage>,
        _events: &mut Vec<AbisAgentEvent>,
    ) {
        let Some(release) = self.decode_typed(message, BtsReleaseMessage::decode) else {
            return;
        };
        let ccr = release.call_connection_reference;

        let Some(session) = self.sessions.get_mut(&ccr) else {
            warn!("abis_agent: BtsRelease for unknown CCR {:?}", ccr);
            return;
        };

        if let Err(e) = session.release.start_release(&release) {
            warn!("abis_agent: release procedure start failed: {e}");
            return;
        }

        let ack = BtsReleaseAckMessage {
            call_connection_reference: ccr,
            correlation_id: release.correlation_id,
        };
        if let Ok(bytes) = ack.encode() {
            if let Some(msg) = abis_message_from_typed(&bytes) {
                responses.push(msg);
            }
        }
    }

    fn handle_remove(
        &mut self,
        message: &AbisMessage,
        responses: &mut Vec<AbisMessage>,
        events: &mut Vec<AbisAgentEvent>,
    ) {
        let Some(remove) = self.decode_typed(message, RemoveMessage::decode) else {
            return;
        };
        let ccr = remove.call_connection_reference;

        let Some(session) = self.sessions.get_mut(&ccr) else {
            warn!("abis_agent: Remove for unknown CCR {:?}", ccr);
            return;
        };

        if let Err(e) = session.release.on_remove(&remove) {
            warn!("abis_agent: release procedure on_remove failed: {e}");
            return;
        }

        let walsh_code = session.walsh_code;
        let sch_walsh_code = session.sch_walsh_code;
        self.controller.deallocate_traffic(walsh_code);
        self.controller.request_rx_removal(walsh_code);
        if let Some(w32) = sch_walsh_code {
            self.controller.deallocate_sch(w32);
        }

        let ack = RemoveAckMessage {
            call_connection_reference: ccr,
            correlation_id: remove.correlation_id,
            a3_destination_id: None,
        };
        if let Ok(bytes) = ack.encode() {
            if let Some(msg) = abis_message_from_typed(&bytes) {
                responses.push(msg);
            }
        }

        info!(
            "abis_agent: traffic released for CCR {:?} walsh={} sch_w32={:?}",
            ccr, walsh_code, sch_walsh_code
        );
        events.push(AbisAgentEvent::TrafficReleased { ccr, walsh_code });
        self.sessions.remove(&ccr);
    }

    fn handle_pch_msg_transfer(
        &mut self,
        message: &AbisMessage,
        responses: &mut Vec<AbisMessage>,
        events: &mut Vec<AbisAgentEvent>,
    ) {
        let Some(pch) = self.decode_typed(message, PchMessageTransferMessage::decode) else {
            return;
        };

        // Ack cause is set to SMS_MESSAGE_TOO_LONG if the encapsulated PCH
        // capsule won't fit, telling the BSC to escalate to a dedicated
        // channel.
        let mut ack_cause: Option<u8> = None;

        // Check if this PchMessageTransfer carries a channel assignment
        // (ECAM or CAM) that should commit a pending traffic channel.
        let is_assignment = pch
            .air_interface_message
            .as_ref()
            .map(|aim| self.try_commit_from_assignment(aim, events))
            .unwrap_or(false);

        let ack_req = pch
            .layer2_ack_request_results
            .as_ref()
            .map(|r| r.layer2_ack)
            .unwrap_or(false);
        let correlation_id = pch.correlation_id.map(|c| c.0);
        let ack_notify = pch.abis_ack_notify.is_some();

        info!(
            "abis_agent: PCH transfer accepted assignment={} corr={:?} l2_ack_req={} ack_notify={} mobiles={} aim_type={:?} aim_bytes={}",
            is_assignment,
            correlation_id,
            ack_req,
            ack_notify,
            pch.mobile_identities.len(),
            pch.air_interface_message
                .as_ref()
                .map(|aim| aim.message_type),
            pch.air_interface_message
                .as_ref()
                .map(|aim| aim.message.len())
                .unwrap_or(0),
        );

        if let (Some(aim), Some(paging_state)) = (&pch.air_interface_message, &self.paging_state) {
            let gpm_wire_type = MessageId::GeneralPage
                .wire_type(WireChannel::ForwardCommon)
                .unwrap();
            if aim.message_type == gpm_wire_type {
                let mut bs = Bitstream::new_bytes(&aim.message);
                match GeneralPageMessage::from_sdu(&mut bs) {
                    Ok(gpm) => {
                        let mut guard = paging_state.lock();
                        for record in gpm.page_records {
                            if guard
                                .pending_page_records
                                .iter()
                                .any(|p| p.record == record)
                            {
                                info!("abis_agent: de-duplicated GPM page record");
                                continue;
                            }
                            let Some(page_address) = record.page_address() else {
                                warn!(
                                    "abis_agent: GPM page record has no mobile page address, dropping: {:?}",
                                    record
                                );
                                continue;
                            };
                            info!(
                                "abis_agent: GPM page record queued: {:?} addr={:?} corr={:?}",
                                record, page_address, correlation_id,
                            );
                            guard.pending_page_records.push(
                                PendingPageRecord::new_with_correlation(
                                    record,
                                    page_address,
                                    correlation_id,
                                ),
                            );
                        }
                    }
                    Err(e) => {
                        warn!("abis_agent: failed to decode GPM from Abis: {}", e);
                    }
                }
            } else {
                let identity = pch.mobile_identities.first();
                if let Some(identity) = identity {
                    let result = paging_state.lock().queue_directed_pch(
                        identity,
                        aim,
                        ack_req,
                        correlation_id,
                        ack_notify,
                    );
                    if let Err(super::paging_supplier::DirectedPchQueueError::Oversize(reason)) =
                        result
                    {
                        warn!(
                            "abis_agent: directed PCH oversized for F-PCH (corr={:?}): {} — acking SMS_MESSAGE_TOO_LONG",
                            correlation_id, reason
                        );
                        ack_cause = Some(
                            cdma_abis::control::typed::pch_message_transfer_ack_cause::SMS_MESSAGE_TOO_LONG,
                        );
                    }
                } else {
                    warn!("abis_agent: directed PCH with no mobile identity, dropping");
                }
            }
        } else if pch.air_interface_message.is_some() && self.paging_state.is_none() {
            warn!("abis_agent: PchMessageTransfer received but no paging state attached");
        }

        let ack = PchMessageTransferAckMessage {
            correlation_id: pch.correlation_id,
            cause: ack_cause,
            bts_l2_termination: None,
        };
        if let Ok(bytes) = ack.encode() {
            if let Some(msg) = abis_message_from_typed(&bytes) {
                responses.push(msg);
            }
        }
    }

    /// Attempt to decode a channel assignment from an air-interface message
    /// payload and commit the pending traffic channel. Returns `true` if the
    /// message was a channel assignment (ECAM/CAM).
    fn try_commit_from_assignment(
        &mut self,
        aim: &cdma_abis::control::typed::AirInterfaceMessagePayload,
        events: &mut Vec<AbisAgentEvent>,
    ) -> bool {
        let cam_wire = MessageId::ChannelAssignment
            .wire_type(WireChannel::ForwardCommon)
            .unwrap();
        let ecam_wire = MessageId::ExtChannelAssignment
            .wire_type(WireChannel::ForwardCommon)
            .unwrap();

        let (walsh_code, for_rc, rev_rc, fpc_subchan_gain, rev_fch_gating_mode, kind) =
            if aim.message_type == cam_wire {
                let mut bs = Bitstream::new_bytes(&aim.message);
                let cam = match ChannelAssignmentMessage::from_sdu(&mut bs) {
                    Ok(cam) => cam,
                    Err(e) => {
                        warn!("abis_agent: CAM decode failed: {e}");
                        return true;
                    }
                };
                let (for_rc, rev_rc) = match cam_radio_config(&cam) {
                    Ok(pair) => pair,
                    Err(e) => {
                        warn!("abis_agent: {e}");
                        return true;
                    }
                };
                (cam.code_chan, for_rc, rev_rc, 0, false, "CAM")
            } else if aim.message_type == ecam_wire {
                let mut bs = Bitstream::new_bytes(&aim.message);
                let ecam = match ExtendedChannelAssignmentMessage::from_sdu(&mut bs) {
                    Ok(e) => e,
                    Err(e) => {
                        warn!("abis_agent: ECAM decode failed: {e}");
                        return true;
                    }
                };
                let Some(walsh_code) = ecam.pilots.first().map(|p| p.code_chan_fch as u8) else {
                    warn!("abis_agent: ECAM has no pilot records");
                    return true;
                };
                (
                    walsh_code,
                    ecam.for_rc,
                    ecam.rev_rc,
                    ecam.fpc_subchan_gain,
                    ecam.rev_fch_gating_mode,
                    "ECAM",
                )
            } else {
                return false;
            };

        info!(
            "abis_agent: {} decoded walsh={} for_rc={} rev_rc={} fpc_subchan_gain={}",
            kind, walsh_code, for_rc, rev_rc, fpc_subchan_gain
        );

        let Some((_ccr, session)) = self
            .sessions
            .iter_mut()
            .find(|(_, s)| s.walsh_code == walsh_code && !s.committed)
        else {
            warn!(
                "abis_agent: {} for walsh {} but no uncommitted session",
                kind, walsh_code
            );
            return true;
        };

        let Some(lc_gen) = session.lc_gen.take() else {
            warn!(
                "abis_agent: session walsh {} already consumed lc_gen",
                walsh_code
            );
            return true;
        };

        match for_rc {
            r if r >= 3 => {
                let channel =
                    self.controller
                        .commit_rc3_traffic(walsh_code, lc_gen, fpc_subchan_gain);
                channel.channel.set_rev_fch_gating_mode(
                    rev_fch_gating_mode && reverse_fch_gating_supported(rev_rc),
                );
            }
            2 => {
                self.controller
                    .commit_rc2_traffic(walsh_code, lc_gen, fpc_subchan_gain);
            }
            _ => {
                self.controller
                    .commit_rc1_traffic(walsh_code, lc_gen, fpc_subchan_gain);
            }
        }

        let rx_request = super::handle::TrafficRxRequest {
            walsh_code,
            esn: session.esn,
            assigned_rev_rc: rev_rc,
            preamble_num_pcgs: None,
            rev_fch_gating_mode: rev_fch_gating_mode && reverse_fch_gating_supported(rev_rc),
        };
        self.controller.install_rx_request(rx_request);
        session.committed = true;

        if session.setup.state() == TrafficSetupState::Connected && session.traffic_lac.is_none() {
            session.traffic_lac = Some(TrafficChannelArqState::new(TrafficArqConfig::default()));
        }

        let ccr = *_ccr;
        info!(
            "abis_agent: committed traffic channel walsh={} RC{}/{} for CCR {:?}",
            walsh_code, for_rc, rev_rc, ccr
        );

        events.push(AbisAgentEvent::TrafficConnected { ccr, walsh_code });
        true
    }

    /// Forwards a reverse-traffic ACK_SEQ to the matching session's traffic LAC.
    ///
    /// Returns any events produced (e.g. `Delivered` / `Failed` notifications).
    pub fn handle_reverse_ack_seq(&mut self, walsh_code: u8, ack_seq: u8) -> Vec<AbisAgentEvent> {
        let mut events = Vec::new();
        let Some((_ccr, session)) = self
            .sessions
            .iter_mut()
            .find(|(_, s)| s.walsh_code == walsh_code)
        else {
            return events;
        };
        let Some(ref mut traffic_lac) = session.traffic_lac else {
            return events;
        };
        let lac_events = traffic_lac.on_reverse_ack_seq(ack_seq);
        for lac_event in lac_events {
            match lac_event {
                TrafficLacEvent::FramesReady { frames } => {
                    events.push(AbisAgentEvent::ForwardTrafficFrames { walsh_code, frames });
                }
                TrafficLacEvent::Delivered { correlation_id } => {
                    info!(
                        "abis_agent: L3 SDU delivered (ack) corr={} walsh={}",
                        correlation_id, walsh_code
                    );
                }
                TrafficLacEvent::Failed { correlation_id } => {
                    warn!(
                        "abis_agent: L3 SDU delivery failed (ack) corr={} walsh={}",
                        correlation_id, walsh_code
                    );
                }
            }
        }
        events
    }

    /// Returns the current reverse ACK_SEQ for the session on `walsh_code`.
    pub fn current_reverse_ack_seq(&self, walsh_code: u8) -> u8 {
        self.sessions
            .values()
            .find(|s| s.walsh_code == walsh_code)
            .and_then(|s| s.traffic_lac.as_ref())
            .map(|lac| lac.current_reverse_ack_seq())
            .unwrap_or(0)
    }

    /// Submits an L3 SDU received via bearer signaling frame to the traffic LAC.
    ///
    /// This is the BTS-side entry point for forward traffic signaling: the BSC
    /// sends a ForwardFchDcchFrame over bearer UDP with Signaling queue flag,
    /// and the BTS routes it here for ARQ/SAR assembly.
    pub fn submit_bearer_l3_sdu(
        &mut self,
        walsh_code: u8,
        wire_msg_type: u8,
        sdu_body: cdma_common::bits::Bitstream,
        ack_seq: u8,
    ) -> Vec<AbisAgentEvent> {
        let mut events = Vec::new();
        let Some((_ccr, session)) = self
            .sessions
            .iter_mut()
            .find(|(_, s)| s.walsh_code == walsh_code)
        else {
            warn!("abis_agent: bearer L3 SDU for unknown walsh={}", walsh_code);
            return events;
        };
        let Some(ref mut traffic_lac) = session.traffic_lac else {
            warn!(
                "abis_agent: bearer L3 SDU for walsh={} but traffic LAC not active",
                walsh_code
            );
            return events;
        };

        let correlation_id = 0;
        let ack_req = true;

        info!(
            "abis_agent: bearer L3 SDU walsh={} msg_type=0x{:02x} sdu_bits={}",
            walsh_code,
            wire_msg_type,
            sdu_body.len()
        );

        let lac_events =
            traffic_lac.submit_l3_sdu(wire_msg_type, sdu_body, ack_seq, ack_req, correlation_id);
        for lac_event in lac_events {
            match lac_event {
                TrafficLacEvent::FramesReady { frames } => {
                    events.push(AbisAgentEvent::ForwardTrafficFrames { walsh_code, frames });
                }
                TrafficLacEvent::Delivered { correlation_id } => {
                    info!(
                        "abis_agent: bearer L3 SDU delivered corr={} walsh={}",
                        correlation_id, walsh_code
                    );
                }
                TrafficLacEvent::Failed { correlation_id } => {
                    warn!(
                        "abis_agent: bearer L3 SDU failed corr={} walsh={}",
                        correlation_id, walsh_code
                    );
                }
            }
        }
        events
    }

    /// Drains buffered paging retry failure events produced by slot-aligned
    /// retransmission in the paging supplier closure.
    pub fn tick_paging_retries(&self) -> Vec<AbisAgentEvent> {
        let Some(ref paging_state) = self.paging_state else {
            return Vec::new();
        };
        let retry_events = paging_state.lock().drain_retry_events();
        let mut events = Vec::new();
        for retry_event in retry_events {
            match retry_event {
                PagingRetryEvent::Failed { correlation_id } => {
                    // A.S0003-A cause 0x07: send PchMessageTransferAck with
                    // cause to notify BSC of delivery failure.
                    let ack = PchMessageTransferAckMessage {
                        correlation_id: Some(cdma_abis::control::CorrelationId(correlation_id)),
                        cause: Some(0x07),
                        bts_l2_termination: None,
                    };
                    match ack.encode() {
                        Ok(bytes) => match abis_message_from_typed(&bytes) {
                            Some(msg) => {
                                events.push(AbisAgentEvent::PagingRetryFailed {
                                    responses: vec![msg],
                                });
                            }
                            None => {}
                        },
                        Err(e) => {
                            warn!("abis_agent: PchMessageTransferAck encode failed: {e}");
                        }
                    }
                }
                PagingRetryEvent::Acknowledged { .. } => {}
            }
        }
        events
    }

    /// Ticks all active traffic LAC sessions for retry timeouts.
    ///
    /// Returns any events produced (retransmission frames, delivery status).
    pub fn tick_all_sessions(&mut self) -> Vec<AbisAgentEvent> {
        let mut events = Vec::new();
        let now = std::time::Instant::now();
        for (_ccr, session) in &mut self.sessions {
            let Some(ref mut traffic_lac) = session.traffic_lac else {
                continue;
            };
            let walsh_code = session.walsh_code;
            let lac_events = traffic_lac.tick_retries(now);
            for lac_event in lac_events {
                match lac_event {
                    TrafficLacEvent::FramesReady { frames } => {
                        events.push(AbisAgentEvent::ForwardTrafficFrames { walsh_code, frames });
                    }
                    TrafficLacEvent::Delivered { correlation_id } => {
                        info!(
                            "abis_agent: L3 SDU delivered (tick) corr={} walsh={}",
                            correlation_id, walsh_code
                        );
                    }
                    TrafficLacEvent::Failed { correlation_id } => {
                        warn!(
                            "abis_agent: L3 SDU delivery failed (tick) corr={} walsh={}",
                            correlation_id, walsh_code
                        );
                    }
                }
            }
        }
        events
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lac::paging_messages::{
        ChannelAssignmentMessage, ExtendedChannelAssignmentMessage, GeneralPageRecord,
    };
    use crate::receiver::access_layer3::{AccessMessageHeader, PageResponseMessage};
    use cdma_abis::control::typed::{
        A3ConnectAckInformation, AirInterfaceMessagePayload, CdmaServingOneWayDelay,
        Layer2AckRequestResults, MobileIdentity, ServiceOption,
    };
    use cdma_common::consts::{SERVICE_OPTION_HIGH_RATE_PACKET_DATA, SERVICE_OPTION_SMS};

    fn test_config() -> AbisAgentConfig {
        AbisAgentConfig {
            pilot_pn: 0,
            cell_id: CellId {
                cell: 0x100,
                sector: 0x01,
            },
            mscid: 0x001234,
        }
    }

    #[test]
    fn cam_radio_config_distinguishes_legacy_rc1_and_requested_service_rc2() {
        let rc1 = ChannelAssignmentMessage::new_traffic_assignment(32, 0);
        assert_eq!(cam_radio_config(&rc1), Ok((1, 1)));

        let rc2 = ChannelAssignmentMessage::new_extended_traffic_assignment(
            33,
            0,
            CHANNEL_DEFAULT_CONFIG_RC1_RC1,
            crate::lac::paging_messages::ChannelAssignmentGrantedMode::RequestedService,
            false,
        );
        assert_eq!(cam_radio_config(&rc2), Ok((2, 2)));
    }

    fn minimal_access_event(imsi: &str, msg_seq: u8) -> AccessChannelEvent {
        let (imsi_m_s1, imsi_m_s2) = cdma_common::paging::imsi_s_from_imsi(imsi).unwrap();
        AccessChannelEvent {
            event_id: "test-access".to_string(),
            chip_start: 0,
            absolute_chip_start: None,
            receive_time: None,
            preamble_frames: 0,
            pd: 1,
            message_id: MessageId::Order,
            msg_type_name: "Order Message".to_string(),
            address: None,
            resolved_address: None,
            subscriber_id: None,
            l3_summary: None,
            decoded_l3: None,
            pdu_summary: String::new(),
            msg_seq: Some(msg_seq),
            ack_seq: Some(0),
            ack_req: true,
            valid_ack: false,
            msid_type: Some(0b011),
            esn: Some(0x808B_0B33),
            imsi: Some(imsi.to_string()),
            meid: None,
            imsi_m_s1: Some(imsi_m_s1),
            imsi_m_s2: Some(imsi_m_s2),
            imsi_class: Some(0),
            imsi_addr_num: None,
            imsi_mcc: None,
            imsi_11_12: Some(cdma_common::paging::imsi_11_12_from_digits(&imsi[3..5]).unwrap()),
            mob_p_rev: None,
            slot_cycle_index: None,
            scm: None,
            wall_clock_us: 0,
            rx_wall_time: None,
            rx_hw_time_ns: None,
            snr_db: None,
            signal_power_db: None,
            reverse_pilot_ec_io_db: None,
            raw_power_db: None,
            demod_quality_pct: None,
            pcg_signal_snr_db: None,
            active_pcg_mask: None,
            traffic_phy_valid: None,
            traffic_fqi_valid: None,
            traffic_tail_valid: None,
            traffic_fqi_bits: None,
            traffic_ml_tail_match: None,
            burst_type: None,
            data_burst_fields: None,
            data_burst_num_msgs: None,
            data_burst_msg_number: None,
            order_code: None,
            service_option: None,
            for_rc_pref: None,
            rev_rc_pref: None,
            rev_fch_gating_req: None,
            traffic_walsh_code: None,
            is_preamble_only: false,
            is_traffic_pcg_measurement: false,
            is_traffic_phy_status: false,
            traffic_measurement_age_chips: None,
            for_supported_rcs: Vec::new(),
            rev_supported_rcs: Vec::new(),
            decoded_rdsch: None,
            traffic_primary_bits: None,
            traffic_primary_rate_bps: None,
            traffic_primary_bearer_routed: false,
            traffic_voice_bits: None,
            traffic_voice_rate_bps: None,
            raw_pdu_bits: None,
        }
    }

    fn minimal_page_response_event(imsi: &str) -> AccessChannelEvent {
        let mut event = minimal_access_event(imsi, 0);
        event.message_id = MessageId::PageResponse;
        event.msg_type_name = "Page Response Message".to_string();
        event.decoded_l3 = Some(AccessMessage::PageResponse(PageResponseMessage {
            header: AccessMessageHeader {
                pd: 1,
                message_id: MessageId::PageResponse,
            },
            mob_term: true,
            slot_cycle_index: 0,
            mob_p_rev: 6,
            scm: 0,
            request_mode: 0,
            service_option: 6,
            pm: false,
            nar_an_cap: false,
            encryption_supported: None,
            num_alt_so: 0,
            alt_service_options: Vec::new(),
            uzid_incl: None,
            uzid: None,
            ch_ind: None,
            otd_supported: None,
            qpch_supported: None,
            enhanced_rc: None,
            for_rc_pref: None,
            rev_rc_pref: None,
            fch_supported: None,
            fch_capability: None,
            dcch_supported: None,
            dcch_capability: None,
            rev_fch_gating_req: None,
            sts_supported: None,
            cch_3x_supported: None,
            wll_incl: None,
            wll_device_type: None,
            hook_status: None,
            enc_info_incl: None,
            sig_encrypt_sup: None,
            d_sig_encrypt_req: None,
            c_sig_encrypt_req: None,
            new_sseq_h: None,
            new_sseq_h_sig: None,
            ui_encrypt_req: None,
            ui_encrypt_sup: None,
            sync_id_incl: None,
            sync_id_len: None,
            sync_id: None,
            so_bitmap_ind: None,
            so_group_num: None,
            so_bitmap: None,
            alt_band_class_sup: None,
            msg_int_info_incl: None,
            sig_integrity_sup_incl: None,
            sig_integrity_sup: None,
            sig_integrity_req: None,
            new_key_id: None,
            new_sseq_h_incl: None,
            for_pdch_supported: None,
            for_pdch_capability: None,
            ext_ch_ind: None,
            sign_slot_cycle_index: None,
            bcmc_incl: None,
            bcmc_pref_incl: None,
            bcmc: None,
            rev_pdch_supported: None,
            rev_pdch_capability: None,
            band_sub_rep_incl: None,
            num_band_subclass: None,
            band_subclass_sup: None,
            remaining_bits: 0,
        }));
        event
    }

    fn make_bts_setup(ccr: CallConnectionReference, esn: u32) -> AbisMessage {
        make_bts_setup_with_so(ccr, esn, None)
    }

    fn make_bts_setup_with_so(
        ccr: CallConnectionReference,
        esn: u32,
        service_option: Option<ServiceOption>,
    ) -> AbisMessage {
        let setup = BtsSetupMessage {
            call_connection_reference: ccr,
            band_class: None,
            privacy_info: None,
            sdu_id: None,
            mobile_identities: vec![MobileIdentity::Esn(esn)],
            physical_channel_info: Some(PhysicalChannelInfo {
                frame_offset: 0,
                pilot_gating_rate: PilotGatingRate::Full,
                arfcn: 283,
                otd: false,
                physical_channels: vec![PhysicalChannelType::Fch],
            }),
            service_option,
            paca_timestamp: None,
            quality_of_service_parameters: None,
            connect_information: Vec::new(),
            abis_originating_id: None,
            cdma_serving_one_way_delay: CdmaServingOneWayDelay {
                cell: CellId {
                    cell: 0x100,
                    sector: 0x01,
                },
                delay_100ns: 0,
            },
            cdma_target_one_way_delay: None,
            walsh_code_assignment_request: true,
        };
        let bytes = setup.encode().unwrap();
        decode(&bytes).unwrap()
    }

    fn make_connect_ack(ccr: CallConnectionReference, walsh_code: u8) -> AbisMessage {
        let ack = ConnectAckMessage {
            call_connection_reference: ccr,
            correlation_id: None,
            connect_ack_information: vec![A3ConnectAckInformation {
                soft_handoff_leg: 0,
                pmc_cause: None,
                transmit_tch_status: false,
                traffic_circuit_id: TrafficCircuitId {
                    traffic_circuit_identifier: walsh_code as u16,
                    traffic_connection_identifier: 0,
                },
                channel_element_id: vec![walsh_code],
                a3_originating_id: 1,
                a3_destination_id: 1,
            }],
        };
        let bytes = ack.encode().unwrap();
        decode(&bytes).unwrap()
    }

    fn make_bts_release(ccr: CallConnectionReference) -> AbisMessage {
        let release = BtsReleaseMessage {
            call_connection_reference: ccr,
            cell_identifier_list: None,
            correlation_id: None,
        };
        let bytes = release.encode().unwrap();
        decode(&bytes).unwrap()
    }

    fn make_remove(ccr: CallConnectionReference) -> AbisMessage {
        let remove = RemoveMessage {
            call_connection_reference: ccr,
            correlation_id: None,
            sdu_id: None,
            remove_information: vec![cdma_abis::control::typed::A3RemoveInformation {
                traffic_circuit_id: TrafficCircuitId {
                    traffic_circuit_identifier: 10,
                    traffic_connection_identifier: 0,
                },
                cells_to_be_removed: vec![CellIdWithMscId {
                    mscid: 0x001234,
                    cell: 0x100,
                    sector: 0x01,
                }],
                a3_destination_id: 1,
                a7_destination_id: 0,
            }],
        };
        let bytes = remove.encode().unwrap();
        decode(&bytes).unwrap()
    }

    fn make_ecam_pch(walsh_code: u8, for_rc: u8, rev_rc: u8) -> AbisMessage {
        make_ecam_pch_with_ack(walsh_code, for_rc, rev_rc, false)
    }

    fn make_cam_pch(walsh_code: u8) -> AbisMessage {
        let cam = ChannelAssignmentMessage::new_traffic_assignment(walsh_code, 0);
        let sdu = cam.to_sdu();
        let sdu_bytes: Vec<u8> = sdu
            .bits()
            .chunks(8)
            .map(|chunk| {
                chunk.iter().fold(0u8, |acc, bit| (acc << 1) | (bit & 1)) << (8 - chunk.len())
            })
            .collect();
        let cam_wire = MessageId::ChannelAssignment
            .wire_type(WireChannel::ForwardCommon)
            .unwrap();
        let pch = PchMessageTransferMessage {
            correlation_id: Some(cdma_abis::control::CorrelationId(78)),
            mobile_identities: vec![MobileIdentity::Esn(0x1234_5678)],
            cell_identifier_list: None,
            air_interface_message: Some(
                AirInterfaceMessagePayload::new(cam_wire, sdu_bytes).unwrap(),
            ),
            layer2_ack_request_results: None,
            abis_ack_notify: None,
        };
        let bytes = pch.encode().unwrap();
        decode(&bytes).unwrap()
    }

    fn make_ecam_pch_with_ack(
        walsh_code: u8,
        for_rc: u8,
        rev_rc: u8,
        ack_req: bool,
    ) -> AbisMessage {
        let ecam = ExtendedChannelAssignmentMessage::new_f_fch_r_fch_assignment(
            0, walsh_code, 0, for_rc, rev_rc, false,
        );
        let sdu = ecam.to_sdu();
        let sdu_bytes: Vec<u8> = sdu
            .bits()
            .chunks(8)
            .map(|chunk| {
                chunk.iter().fold(0u8, |acc, bit| (acc << 1) | (bit & 1)) << (8 - chunk.len())
            })
            .collect();
        let ecam_wire = MessageId::ExtChannelAssignment
            .wire_type(WireChannel::ForwardCommon)
            .unwrap();
        let pch = PchMessageTransferMessage {
            correlation_id: Some(cdma_abis::control::CorrelationId(77)),
            mobile_identities: vec![MobileIdentity::Esn(0x1234_5678)],
            cell_identifier_list: None,
            air_interface_message: Some(
                AirInterfaceMessagePayload::new(ecam_wire, sdu_bytes).unwrap(),
            ),
            layer2_ack_request_results: ack_req.then_some(Layer2AckRequestResults::request()),
            abis_ack_notify: ack_req.then_some(cdma_abis::control::typed::AbisAckNotify),
        };
        let bytes = pch.encode().unwrap();
        decode(&bytes).unwrap()
    }

    fn test_ccr() -> CallConnectionReference {
        CallConnectionReference {
            market_id: 100,
            generating_entity_id: 200,
            call_connection_reference: 1,
        }
    }

    #[test]
    fn setup_allocates_and_responds_with_connect() {
        let controller = Arc::new(TrafficResourceService::new());
        controller
            .walsh_allocator()
            .lock()
            .reserve_system_channels(0, 1, 32);
        let mut agent = AbisAgent::new(test_config(), controller.clone());

        let ccr = test_ccr();
        let setup_msg = make_bts_setup(ccr, 0xDEAD);

        let (responses, events) = agent.handle_message(&setup_msg);
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0].message_type, MessageType::Connect);
        assert_eq!(agent.active_session_count(), 1);
        assert!(agent.walsh_code_for(&ccr).is_some());
        assert!(events.is_empty());
    }

    #[test]
    fn connect_ack_produces_setup_ack_and_status() {
        let controller = Arc::new(TrafficResourceService::new());
        controller
            .walsh_allocator()
            .lock()
            .reserve_system_channels(0, 1, 32);
        let mut agent = AbisAgent::new(test_config(), controller);

        let ccr = test_ccr();
        let setup_msg = make_bts_setup(ccr, 0xBEEF);
        let (responses, _) = agent.handle_message(&setup_msg);
        assert_eq!(responses[0].message_type, MessageType::Connect);

        let walsh_code = agent.walsh_code_for(&ccr).unwrap();
        let ack_msg = make_connect_ack(ccr, walsh_code);
        let (responses, events) = agent.handle_message(&ack_msg);

        assert_eq!(responses.len(), 2);
        assert_eq!(responses[0].message_type, MessageType::BtsSetupAck);
        assert_eq!(responses[1].message_type, MessageType::TrafficChannelStatus);

        // TrafficConnected is now emitted when the ECAM arrives via
        // PchMessageTransfer, not at ConnectAck time.
        assert!(events.is_empty());
    }

    #[test]
    fn release_and_remove_deallocates() {
        let controller = Arc::new(TrafficResourceService::new());
        controller
            .walsh_allocator()
            .lock()
            .reserve_system_channels(0, 1, 32);
        let mut agent = AbisAgent::new(test_config(), controller.clone());

        let ccr = test_ccr();
        agent.handle_message(&make_bts_setup(ccr, 0x1234));
        let walsh_code = agent.walsh_code_for(&ccr).unwrap();
        let ack_msg = make_connect_ack(ccr, walsh_code);
        agent.handle_message(&ack_msg);

        let (responses, _) = agent.handle_message(&make_bts_release(ccr));
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0].message_type, MessageType::BtsReleaseAck);

        let (responses, events) = agent.handle_message(&make_remove(ccr));
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0].message_type, MessageType::RemoveAck);
        assert_eq!(agent.active_session_count(), 0);

        assert!(controller.traffic_channels_pool().is_empty());

        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            AbisAgentEvent::TrafficReleased { ccr: c, walsh_code: w }
            if *c == ccr && *w == walsh_code
        ));
    }

    #[test]
    fn pch_msg_transfer_acks() {
        let controller = Arc::new(TrafficResourceService::new());
        let mut agent = AbisAgent::new(test_config(), controller);

        let pch = PchMessageTransferMessage {
            correlation_id: Some(cdma_abis::control::CorrelationId(42)),
            mobile_identities: vec![MobileIdentity::Esn(0xAAAA)],
            cell_identifier_list: None,
            air_interface_message: None,
            layer2_ack_request_results: None,
            abis_ack_notify: None,
        };
        let bytes = pch.encode().unwrap();
        let msg = decode(&bytes).unwrap();

        let (responses, events) = agent.handle_message(&msg);
        assert_eq!(responses.len(), 1);
        assert_eq!(
            responses[0].message_type,
            MessageType::PchMessageTransferAck
        );
        assert!(events.is_empty());
    }

    #[test]
    fn gpm_pch_drops_records_without_page_address() {
        let controller = Arc::new(TrafficResourceService::new());
        let mut agent = AbisAgent::new(test_config(), controller);
        let paging_state = Arc::new(Mutex::new(PagingSupplierState::new(0, 0)));
        agent.set_paging_state(paging_state.clone());

        let gpm = GeneralPageMessage {
            config_msg_seq: 0,
            acc_msg_seq: 0,
            class_0_done: true,
            class_1_done: true,
            tmsi_done: true,
            ordered_tmsis: false,
            broadcast_done: false,
            reserved: 0,
            add_pfield: Vec::new(),
            page_records: vec![crate::lac::paging_messages::GeneralPageRecord::Broadcast {
                bc_addr: 0x1234,
            }],
        };
        let wire_type = MessageId::GeneralPage
            .wire_type(WireChannel::ForwardCommon)
            .unwrap();
        let pch = PchMessageTransferMessage {
            correlation_id: Some(cdma_abis::control::CorrelationId(43)),
            mobile_identities: vec![MobileIdentity::Esn(0xAAAA)],
            cell_identifier_list: None,
            air_interface_message: Some(
                AirInterfaceMessagePayload::new(wire_type, gpm.to_sdu().to_packed_bytes()).unwrap(),
            ),
            layer2_ack_request_results: None,
            abis_ack_notify: None,
        };
        let bytes = pch.encode().unwrap();
        let msg = decode(&bytes).unwrap();

        let (responses, events) = agent.handle_message(&msg);
        assert_eq!(responses.len(), 1);
        assert_eq!(
            responses[0].message_type,
            MessageType::PchMessageTransferAck
        );
        assert!(events.is_empty());
        assert!(paging_state.lock().pending_page_records.is_empty());
    }

    #[test]
    fn gpm_pch_queues_correlated_page_record() {
        let controller = Arc::new(TrafficResourceService::new());
        let mut agent = AbisAgent::new(test_config(), controller);
        let paging_state = Arc::new(Mutex::new(PagingSupplierState::new(0, 0)));
        agent.set_paging_state(paging_state.clone());

        let record = GeneralPageRecord::Class1 {
            esn: 0x1234_5678,
            msg_seq: 3,
            special_service: false,
            service_option: None,
        };
        let gpm = GeneralPageMessage {
            config_msg_seq: 0,
            acc_msg_seq: 0,
            class_0_done: true,
            class_1_done: true,
            tmsi_done: true,
            ordered_tmsis: false,
            broadcast_done: false,
            reserved: 0,
            add_pfield: Vec::new(),
            page_records: vec![record.clone()],
        };
        let wire_type = MessageId::GeneralPage
            .wire_type(WireChannel::ForwardCommon)
            .unwrap();
        let pch = PchMessageTransferMessage {
            correlation_id: Some(cdma_abis::control::CorrelationId(43)),
            mobile_identities: vec![MobileIdentity::Esn(0x1234_5678)],
            cell_identifier_list: None,
            air_interface_message: Some(
                AirInterfaceMessagePayload::new(wire_type, gpm.to_sdu().to_packed_bytes()).unwrap(),
            ),
            layer2_ack_request_results: None,
            abis_ack_notify: None,
        };
        let bytes = pch.encode().unwrap();
        let msg = decode(&bytes).unwrap();

        let (responses, events) = agent.handle_message(&msg);
        assert_eq!(responses.len(), 1);
        assert_eq!(
            responses[0].message_type,
            MessageType::PchMessageTransferAck
        );
        assert!(events.is_empty());

        let guard = paging_state.lock();
        assert_eq!(guard.pending_page_records.len(), 1);
        assert_eq!(guard.pending_page_records[0].record, record);
        assert_eq!(guard.pending_page_records[0].correlation_id, Some(43));
    }

    #[test]
    fn page_response_cancel_sends_positive_pch_ack() {
        let controller = Arc::new(TrafficResourceService::new());
        let mut agent = AbisAgent::new(test_config(), controller);
        let paging_state = Arc::new(Mutex::new(PagingSupplierState::new(0, 0)));
        agent.set_paging_state(paging_state.clone());
        let (imsi_m_s1, imsi_m_s2) =
            cdma_common::paging::imsi_s_from_imsi("209990123456789").unwrap();

        paging_state
            .lock()
            .pending_page_records
            .push(PendingPageRecord::new_with_correlation(
                GeneralPageRecord::Class0 {
                    page_subclass: 3,
                    msg_seq: 0,
                    imsi_s: None,
                    imsi_11_12: Some(cdma_common::paging::imsi_11_12_from_digits("99").unwrap()),
                    mcc: None,
                    imsi_addr_num: None,
                    imsi_m_s1: Some(imsi_m_s1),
                    imsi_m_s2: Some(imsi_m_s2),
                    special_service: false,
                    service_option: None,
                },
                MsPageAddress::ImsiS {
                    imsi_m_s1,
                    imsi_m_s2,
                    mcc: None,
                    imsi_11_12: Some(cdma_common::paging::imsi_11_12_from_digits("99").unwrap()),
                },
                Some(55),
            ));

        let responses =
            agent.check_page_response_cancel(&minimal_page_response_event("209990123456789"));

        assert_eq!(responses.len(), 1);
        let ack_bytes = encode(&responses[0]).unwrap();
        let ack = PchMessageTransferAckMessage::decode(&ack_bytes).unwrap();
        assert_eq!(ack.correlation_id.map(|c| c.0), Some(55));
        assert_eq!(ack.cause, None);
        assert_eq!(ack.bts_l2_termination, Some(true));
        assert!(paging_state.lock().pending_page_records.is_empty());
    }

    #[test]
    fn assignment_pch_queues_directed_sdu_with_ack() {
        let controller = Arc::new(TrafficResourceService::new());
        controller
            .walsh_allocator()
            .lock()
            .reserve_system_channels(0, 1, 32);
        let mut agent = AbisAgent::new(test_config(), controller.clone());
        let paging_state = Arc::new(Mutex::new(PagingSupplierState::new(0, 0)));
        agent.set_paging_state(paging_state.clone());

        let ccr = test_ccr();
        let setup_msg = make_bts_setup_with_so(
            ccr,
            0xDEAD,
            Some(ServiceOption(SERVICE_OPTION_HIGH_RATE_PACKET_DATA)),
        );
        let _ = agent.handle_message(&setup_msg);
        let walsh_code = agent.walsh_code_for(&ccr).unwrap();

        let ecam_msg = make_ecam_pch_with_ack(walsh_code, 3, 3, true);
        let (responses, events) = agent.handle_message(&ecam_msg);

        assert_eq!(
            responses[0].message_type,
            MessageType::PchMessageTransferAck
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AbisAgentEvent::TrafficConnected { .. }))
        );

        let guard = paging_state.lock();
        assert_eq!(guard.pending_directed_sdus.len(), 1);
        let dr = &guard.pending_directed_sdus[0];
        assert!(dr.mcsb.ack_req);
    }

    #[test]
    fn access_msg_seq_recording_uses_full_imsi_when_present() {
        let controller = Arc::new(TrafficResourceService::new());
        let mut agent = AbisAgent::new(test_config(), controller);
        let paging_state = Arc::new(Mutex::new(PagingSupplierState::new(
            cdma_common::paging::mcc_from_digits("310").unwrap(),
            cdma_common::paging::imsi_11_12_from_digits("26").unwrap(),
        )));
        agent.set_paging_state(paging_state.clone());

        let event = minimal_access_event("209990123456789", 7);
        agent.record_access_msg_seq(&event);

        let wire_type = MessageId::Order
            .wire_type(WireChannel::ForwardCommon)
            .unwrap();
        let payload = AirInterfaceMessagePayload::new(wire_type, [0x6C, 0x00]).unwrap();
        paging_state
            .lock()
            .queue_directed_pch(
                &MobileIdentity::Imsi("209990123456789".to_string()),
                &payload,
                true,
                Some(99),
                true,
            )
            .unwrap();

        let mut guard = paging_state.lock();
        let dr = guard.pending_directed_sdus.pop_front().unwrap();
        assert!(dr.mcsb.valid_ack);
        assert_eq!(dr.mcsb.ack_seq, 7);
    }

    #[test]
    fn setup_with_so33_reserves_walsh_then_ecam_commits_rc3() {
        let controller = Arc::new(TrafficResourceService::new());
        controller
            .walsh_allocator()
            .lock()
            .reserve_system_channels(0, 1, 32);
        let mut agent = AbisAgent::new(test_config(), controller.clone());

        let ccr = test_ccr();
        let setup_msg = make_bts_setup_with_so(
            ccr,
            0xDEAD,
            Some(ServiceOption(SERVICE_OPTION_HIGH_RATE_PACKET_DATA)),
        );

        let (responses, _events) = agent.handle_message(&setup_msg);
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0].message_type, MessageType::Connect);
        let walsh_code = agent.walsh_code_for(&ccr).unwrap();

        // No RX installed yet — channel not committed.
        assert!(controller.traffic_rx_pool().lock().is_empty());

        // Send ECAM via PchMessageTransfer to commit the channel.
        let ecam_msg = make_ecam_pch(walsh_code, 3, 3);
        let (responses, events) = agent.handle_message(&ecam_msg);

        // PchMessageTransferAck response + TrafficConnected event.
        assert_eq!(responses.len(), 1);
        assert_eq!(
            responses[0].message_type,
            MessageType::PchMessageTransferAck
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AbisAgentEvent::TrafficConnected { .. }))
        );

        // RX request now installed with rev_rc=3.
        let rx_pool = controller.traffic_rx_pool().lock();
        let rx = rx_pool.iter().find(|r| r.walsh_code == walsh_code);
        assert!(rx.is_some());
        assert_eq!(rx.unwrap().assigned_rev_rc, 3);
    }

    #[test]
    fn setup_reserves_walsh_then_cam_commits_rc1() {
        let controller = Arc::new(TrafficResourceService::new());
        controller
            .walsh_allocator()
            .lock()
            .reserve_system_channels(0, 1, 32);
        let mut agent = AbisAgent::new(test_config(), controller.clone());

        let ccr = test_ccr();
        let setup_msg =
            make_bts_setup_with_so(ccr, 0xDEAD, Some(ServiceOption(SERVICE_OPTION_SMS)));

        let (responses, _events) = agent.handle_message(&setup_msg);
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0].message_type, MessageType::Connect);
        let walsh_code = agent.walsh_code_for(&ccr).unwrap();

        assert!(controller.traffic_rx_pool().lock().is_empty());

        let cam_msg = make_cam_pch(walsh_code);
        let (responses, events) = agent.handle_message(&cam_msg);

        assert_eq!(responses.len(), 1);
        assert_eq!(
            responses[0].message_type,
            MessageType::PchMessageTransferAck
        );
        assert!(
            events
                .iter()
                .any(|e| matches!(e, AbisAgentEvent::TrafficConnected { .. }))
        );

        let rx_pool = controller.traffic_rx_pool().lock();
        let rx = rx_pool.iter().find(|r| r.walsh_code == walsh_code);
        assert!(rx.is_some());
        assert_eq!(rx.unwrap().assigned_rev_rc, 1);
        assert_eq!(controller.traffic_channels_pool().len(), 1);
    }

    #[test]
    fn bts_initiated_release_sends_release_message() {
        let controller = Arc::new(TrafficResourceService::new());
        controller
            .walsh_allocator()
            .lock()
            .reserve_system_channels(0, 1, 32);
        let mut agent = AbisAgent::new(test_config(), controller.clone());

        let ccr = test_ccr();
        agent.handle_message(&make_bts_setup(ccr, 0xBEEF));
        let walsh_code = agent.walsh_code_for(&ccr).unwrap();

        let (responses, events) = agent.initiate_release(walsh_code);
        assert_eq!(responses.len(), 1);
        assert_eq!(responses[0].message_type, MessageType::BtsRelease);
        assert_eq!(events.len(), 1);
        assert!(matches!(
            &events[0],
            AbisAgentEvent::BtsReleaseInitiated { ccr: c, walsh_code: w }
            if *c == ccr && *w == walsh_code
        ));

        assert_eq!(agent.active_session_count(), 1);
    }

    #[test]
    fn bts_initiated_release_unknown_walsh_is_noop() {
        let controller = Arc::new(TrafficResourceService::new());
        let mut agent = AbisAgent::new(test_config(), controller);

        let (responses, events) = agent.initiate_release(99);
        assert!(responses.is_empty());
        assert!(events.is_empty());
    }

    #[test]
    fn multiple_sessions_independent() {
        let controller = Arc::new(TrafficResourceService::new());
        controller
            .walsh_allocator()
            .lock()
            .reserve_system_channels(0, 1, 32);
        let mut agent = AbisAgent::new(test_config(), controller);

        let ccr1 = CallConnectionReference {
            market_id: 100,
            generating_entity_id: 200,
            call_connection_reference: 1,
        };
        let ccr2 = CallConnectionReference {
            market_id: 100,
            generating_entity_id: 200,
            call_connection_reference: 2,
        };

        agent.handle_message(&make_bts_setup(ccr1, 0x1111));
        agent.handle_message(&make_bts_setup(ccr2, 0x2222));
        assert_eq!(agent.active_session_count(), 2);

        let w1 = agent.walsh_code_for(&ccr1).unwrap();
        let w2 = agent.walsh_code_for(&ccr2).unwrap();
        assert_ne!(w1, w2);
    }
}

/// Convert an access channel event's identity fields to an MsAddress for
/// ack-notify matching.
fn access_event_to_ms_address(event: &AccessChannelEvent) -> Option<MsAddress> {
    if let Some(imsi) = event.imsi.as_deref() {
        if let Some(addr) = imsi_to_ms_address(imsi) {
            return Some(addr);
        }
    }

    if let Some(imsi_m_s1) = event.imsi_m_s1 {
        let imsi_m_s2 = event.imsi_m_s2.unwrap_or(0) as u16;
        if let (Some(mcc), Some(imsi_11_12)) = (event.imsi_mcc, event.imsi_11_12) {
            Some(MsAddress::ImsiClass0 {
                imsi_m_s1,
                imsi_m_s2,
                mcc,
                imsi_11_12,
            })
        } else {
            Some(MsAddress::ImsiS {
                imsi_m_s1,
                imsi_m_s2,
            })
        }
    } else if let Some(esn) = event.esn {
        Some(MsAddress::Esn(esn))
    } else {
        None
    }
}

fn imsi_to_ms_address(imsi: &str) -> Option<MsAddress> {
    let (imsi_m_s1, imsi_m_s2) = cdma_common::paging::imsi_s_from_imsi(imsi)?;
    let digits: String = imsi.chars().filter(|c| c.is_ascii_digit()).collect();
    if digits.len() == 15 {
        let mcc = cdma_common::paging::mcc_from_digits(&digits[0..3])?;
        let imsi_11_12 = cdma_common::paging::imsi_11_12_from_digits(&digits[3..5])?;
        Some(MsAddress::ImsiClass0 {
            imsi_m_s1,
            imsi_m_s2,
            mcc,
            imsi_11_12,
        })
    } else {
        Some(MsAddress::ImsiS {
            imsi_m_s1,
            imsi_m_s2,
        })
    }
}
