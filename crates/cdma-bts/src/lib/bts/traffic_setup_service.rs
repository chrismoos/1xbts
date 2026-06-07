//! Traffic channel setup and release procedure service.
//!
//! `TrafficSetupService` encapsulates the per-CCR lifecycle of traffic channel
//! allocation: walsh code reservation, long-code generator storage, channel
//! commit (after ECAM decode), and release teardown.

use std::collections::HashMap;
use std::sync::Arc;

use log::{info, warn};

use cdma_abis::control::typed::{
    A3ConnectInformation, CellInfoRecord, ChannelElementStatus, PhysicalChannelInfo,
    PhysicalChannelType, PilotGatingRate, TrafficChannelStatusMessage, TrafficCircuitId,
};
use cdma_abis::control::{
    AbisMessage, BtsReleaseAckMessage, BtsReleaseMessage, BtsSetupAckMessage, BtsSetupMessage,
    CallConnectionReference, ConnectAckMessage, ConnectMessage, RemoveAckMessage, RemoveMessage,
    TrafficReleaseProcedure, TrafficSetupProcedure, TrafficSetupState,
};

use super::abis_agent::{AbisAgentConfig, AbisAgentEvent};
use super::handle::TrafficRxRequest;
use super::resource_controller::TrafficResourceController;
use super::traffic_lac::{TrafficArqConfig, TrafficChannelArqState, TrafficLacEvent};
use crate::lac::message_types::{MessageId, WireChannel};
use crate::lac::paging_messages::ExtendedChannelAssignmentMessage;
use crate::phy::coding::long_code::LongCodeGenerator;
use cdma_common::bits::Bitstream;

use super::abis_agent::abis_message_from_typed;

/// Per-CCR session tracking on the BTS side.
pub(crate) struct Session {
    pub(crate) walsh_code: u8,
    pub(crate) esn: u32,
    /// Long code generator, stored at reservation time and consumed when
    /// the traffic channel is committed (after ECAM decode).
    pub(crate) lc_gen: Option<LongCodeGenerator>,
    /// `true` once the forward TX channel + reverse RX have been created
    /// (i.e. after ECAM/CAM decode). Before that, only the walsh code is
    /// reserved.
    pub(crate) committed: bool,
    pub(crate) setup: TrafficSetupProcedure,
    pub(crate) release: TrafficReleaseProcedure,
    pub(crate) traffic_lac: Option<TrafficChannelArqState>,
    /// F-SCH code allocated through the Abis Burst path for this FCH session,
    /// or `None` before supplemental allocation. Freed in `handle_remove`.
    pub(crate) sch_walsh_code: Option<u8>,
}

/// Service responsible for traffic channel setup and release procedures.
///
/// Manages per-CCR session state including walsh code reservation, long-code
/// generator storage, channel commit (triggered by ECAM decode), and release
/// teardown. The service delegates resource allocation to the underlying
/// `TrafficResourceController`.
pub struct TrafficSetupService {
    config: AbisAgentConfig,
    controller: Arc<TrafficResourceController>,
    pub(crate) sessions: HashMap<CallConnectionReference, Session>,
}

impl TrafficSetupService {
    /// Creates a new traffic setup service.
    pub fn new(config: AbisAgentConfig, controller: Arc<TrafficResourceController>) -> Self {
        Self {
            config,
            controller,
            sessions: HashMap::new(),
        }
    }

    /// Returns the number of active sessions.
    pub fn active_session_count(&self) -> usize {
        self.sessions.len()
    }

    /// Returns the Walsh code allocated for a given CCR, if any.
    pub fn walsh_code_for(&self, ccr: &CallConnectionReference) -> Option<u8> {
        self.sessions.get(ccr).map(|s| s.walsh_code)
    }

    /// Handles a BtsSetup message: reserves a walsh code, creates the session,
    /// and produces a Connect response.
    pub fn handle_bts_setup(&mut self, setup: &BtsSetupMessage, responses: &mut Vec<AbisMessage>) {
        let ccr = setup.call_connection_reference;
        info!("traffic_setup: BtsSetup for CCR {:?}", ccr);

        let esn = setup
            .mobile_identities
            .iter()
            .find_map(|id| match id {
                cdma_abis::control::MobileIdentity::Esn(e) => Some(*e),
                _ => None,
            })
            .unwrap_or(0);

        let lc_gen = LongCodeGenerator::new_traffic_channel(esn);

        let Some(walsh_code) = self.controller.reserve_walsh() else {
            warn!("traffic_setup: no Walsh codes available for CCR {:?}", ccr);
            return;
        };

        // F-SCH is now allocated through the rate-aware Abis Burst path after
        // FCH setup. Ignore legacy setup-time SCH requests so this path cannot
        // reserve a fixed 19.2k W(32) supplemental channel.
        let requested_sch = setup
            .physical_channel_info
            .as_ref()
            .map(|p| p.physical_channels.contains(&PhysicalChannelType::Sch))
            .unwrap_or(false);
        if requested_sch {
            info!(
                "traffic_setup: ignoring legacy setup-time SCH request for CCR {:?}; SCH uses Abis Burst allocation",
                ccr
            );
        }
        let sch_walsh_code: Option<u8> = None;
        info!(
            "traffic_setup: reserved Walsh code {} for CCR {:?} (channel pending ECAM)",
            walsh_code, ccr
        );

        let mut setup_proc = TrafficSetupProcedure::new(ccr);
        if let Err(e) = setup_proc.start_setup(setup) {
            warn!("traffic_setup: setup procedure start failed: {e}");
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
            physical_channel_info: setup.physical_channel_info.clone().unwrap_or(
                PhysicalChannelInfo {
                    frame_offset: 0,
                    pilot_gating_rate: PilotGatingRate::Full,
                    arfcn: 0,
                    otd: false,
                    physical_channels: vec![PhysicalChannelType::Fch],
                },
            ),
        };

        match connect.encode() {
            Ok(bytes) => {
                if let Some(msg) = abis_message_from_typed(&bytes) {
                    if let Err(e) = setup_proc.on_connect(&connect) {
                        warn!("traffic_setup: setup procedure on_connect failed: {e}");
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
                warn!("traffic_setup: Connect encode failed: {e}");
                self.controller.deallocate_traffic(walsh_code);
            }
        }
    }

    /// Handles a ConnectAck message: advances the setup state machine and
    /// produces BtsSetupAck + TrafficChannelStatus responses.
    pub fn handle_connect_ack(
        &mut self,
        ack: &ConnectAckMessage,
        responses: &mut Vec<AbisMessage>,
    ) {
        let ccr = ack.call_connection_reference;

        let Some(session) = self.sessions.get_mut(&ccr) else {
            warn!("traffic_setup: ConnectAck for unknown CCR {:?}", ccr);
            return;
        };

        if let Err(e) = session.setup.on_connect_ack(ack) {
            warn!("traffic_setup: setup procedure on_connect_ack failed: {e}");
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
                    warn!("traffic_setup: setup procedure on_setup_ack failed: {e}");
                }
                responses.push(msg);
            }
        }

        let status = TrafficChannelStatusMessage {
            call_connection_reference: ccr,
            cell_identifier_list: vec![cdma_abis::control::typed::CellIdWithMscId {
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
            if session.committed && session.traffic_lac.is_none() {
                session.traffic_lac =
                    Some(TrafficChannelArqState::new(TrafficArqConfig::default()));
            }
            info!(
                "traffic_setup: Abis connected for CCR {:?} walsh={} (channel committed={})",
                ccr, session.walsh_code, session.committed
            );
        }
    }

    /// Handles a BtsRelease message: starts the release procedure and produces
    /// a BtsReleaseAck response.
    pub fn handle_bts_release(
        &mut self,
        release: &BtsReleaseMessage,
        responses: &mut Vec<AbisMessage>,
    ) {
        let ccr = release.call_connection_reference;

        let Some(session) = self.sessions.get_mut(&ccr) else {
            warn!("traffic_setup: BtsRelease for unknown CCR {:?}", ccr);
            return;
        };

        if let Err(e) = session.release.start_release(release) {
            warn!("traffic_setup: release procedure start failed: {e}");
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

    /// Handles a Remove message: deallocates resources, removes the session,
    /// and produces a RemoveAck response + TrafficReleased event.
    pub fn handle_remove(
        &mut self,
        remove: &RemoveMessage,
        responses: &mut Vec<AbisMessage>,
        events: &mut Vec<AbisAgentEvent>,
    ) {
        let ccr = remove.call_connection_reference;

        let Some(session) = self.sessions.get_mut(&ccr) else {
            warn!("traffic_setup: Remove for unknown CCR {:?}", ccr);
            return;
        };

        if let Err(e) = session.release.on_remove(remove) {
            warn!("traffic_setup: release procedure on_remove failed: {e}");
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
            "traffic_setup: traffic released for CCR {:?} walsh={} sch_w32={:?}",
            ccr, walsh_code, sch_walsh_code
        );
        events.push(AbisAgentEvent::TrafficReleased { ccr, walsh_code });
        self.sessions.remove(&ccr);
    }

    /// Attempt to decode a channel assignment from an air-interface message
    /// payload and commit the pending traffic channel. Returns `true` if the
    /// message was a channel assignment (ECAM/CAM).
    pub fn try_commit_from_assignment(
        &mut self,
        aim: &cdma_abis::control::typed::AirInterfaceMessagePayload,
        events: &mut Vec<AbisAgentEvent>,
    ) -> bool {
        let ecam_wire = MessageId::ExtChannelAssignment
            .wire_type(WireChannel::ForwardCommon)
            .unwrap();
        if aim.message_type != ecam_wire {
            return false;
        }

        let mut bs = Bitstream::new_bytes(&aim.message);
        let ecam = match ExtendedChannelAssignmentMessage::from_sdu(&mut bs) {
            Ok(e) => e,
            Err(e) => {
                warn!("traffic_setup: ECAM decode failed: {e}");
                return true;
            }
        };

        let ecam_walsh = ecam.pilots.first().map(|p| p.code_chan_fch as u8);
        let Some(walsh_code) = ecam_walsh else {
            warn!("traffic_setup: ECAM has no pilot records");
            return true;
        };

        info!(
            "traffic_setup: ECAM decoded walsh={} for_rc={} rev_rc={} fpc_subchan_gain={}",
            walsh_code, ecam.for_rc, ecam.rev_rc, ecam.fpc_subchan_gain
        );

        let Some((_ccr, session)) = self
            .sessions
            .iter_mut()
            .find(|(_, s)| s.walsh_code == walsh_code && !s.committed)
        else {
            warn!(
                "traffic_setup: ECAM for walsh {} but no uncommitted session",
                walsh_code
            );
            return true;
        };

        let Some(lc_gen) = session.lc_gen.take() else {
            warn!(
                "traffic_setup: session walsh {} already consumed lc_gen",
                walsh_code
            );
            return true;
        };

        let is_rc3 = ecam.for_rc >= 3;
        if is_rc3 {
            self.controller
                .commit_rc3_traffic(walsh_code, lc_gen, ecam.fpc_subchan_gain);
        } else {
            self.controller
                .commit_rc1_traffic(walsh_code, lc_gen, ecam.fpc_subchan_gain);
        }

        let assigned_rev_rc = ecam.rev_rc;
        let rx_request = TrafficRxRequest {
            walsh_code,
            esn: session.esn,
            assigned_rev_rc,
            preamble_num_pcgs: None,
            rev_fch_gating_mode: ecam.rev_fch_gating_mode,
        };
        self.controller.install_rx_request(rx_request);
        session.committed = true;

        if session.setup.state() == TrafficSetupState::Connected && session.traffic_lac.is_none() {
            session.traffic_lac = Some(TrafficChannelArqState::new(TrafficArqConfig::default()));
        }

        let ccr = *_ccr;
        info!(
            "traffic_setup: committed traffic channel walsh={} RC{}/{} for CCR {:?}",
            walsh_code, ecam.for_rc, ecam.rev_rc, ccr
        );

        events.push(AbisAgentEvent::TrafficConnected { ccr, walsh_code });
        true
    }

    /// Builds a BTS-initiated release message for the session on `walsh_code`.
    ///
    /// Returns the encoded `AbisMessage` to send to the BSC and a
    /// `BtsReleaseInitiated` event. The session is **not** removed here;
    /// the BSC will follow up with a `Remove`.
    pub fn initiate_release(&mut self, walsh_code: u8) -> (Vec<AbisMessage>, Vec<AbisAgentEvent>) {
        let mut responses = Vec::new();
        let mut events = Vec::new();

        let Some((&ccr, _session)) = self
            .sessions
            .iter()
            .find(|(_, s)| s.walsh_code == walsh_code)
        else {
            warn!(
                "traffic_setup: initiate_release called for unknown walsh_code {}",
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
                        "traffic_setup: BTS-initiated release for CCR {:?} walsh={}",
                        ccr, walsh_code
                    );
                    responses.push(msg);
                    events.push(AbisAgentEvent::BtsReleaseInitiated { ccr, walsh_code });
                }
            }
            Err(e) => {
                warn!("traffic_setup: BtsRelease encode failed: {e}");
            }
        }

        (responses, events)
    }

    /// Forwards a reverse-traffic ACK_SEQ to the matching session's traffic LAC.
    ///
    /// Returns any events produced (e.g. retransmission frames, delivery status).
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
                        "traffic_setup: L3 SDU delivered (ack) corr={} walsh={}",
                        correlation_id, walsh_code
                    );
                }
                TrafficLacEvent::Failed { correlation_id } => {
                    warn!(
                        "traffic_setup: L3 SDU delivery failed (ack) corr={} walsh={}",
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

    /// Submits an L3 SDU received via bearer signaling to the traffic LAC.
    ///
    /// Returns any events produced (retransmission frames, delivery status).
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
            warn!(
                "traffic_setup: bearer L3 SDU for unknown walsh={}",
                walsh_code
            );
            return events;
        };
        let Some(ref mut traffic_lac) = session.traffic_lac else {
            warn!(
                "traffic_setup: bearer L3 SDU for walsh={} but traffic LAC not active",
                walsh_code
            );
            return events;
        };

        let correlation_id = 0;
        let ack_req = true;

        info!(
            "traffic_setup: bearer L3 SDU walsh={} msg_type=0x{:02x} sdu_bits={}",
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
                        "traffic_setup: bearer L3 SDU delivered corr={} walsh={}",
                        correlation_id, walsh_code
                    );
                }
                TrafficLacEvent::Failed { correlation_id } => {
                    warn!(
                        "traffic_setup: bearer L3 SDU failed corr={} walsh={}",
                        correlation_id, walsh_code
                    );
                }
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
                            "traffic_setup: L3 SDU delivered (tick) corr={} walsh={}",
                            correlation_id, walsh_code
                        );
                    }
                    TrafficLacEvent::Failed { correlation_id } => {
                        warn!(
                            "traffic_setup: L3 SDU delivery failed (tick) corr={} walsh={}",
                            correlation_id, walsh_code
                        );
                    }
                }
            }
        }
        events
    }
}
