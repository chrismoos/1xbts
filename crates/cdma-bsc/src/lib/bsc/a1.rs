//! Track B A1 ingress helpers.

use std::collections::HashMap;
use std::sync::Arc;

use cdma_common::lac::paging_messages::{CallingPartyNumberRecord, MsAddress};
use log::{debug, info, warn};
use tokio::spawn;

use crate::a1_edge::EncodedA1Message;
use crate::addressing::format_ms_address;

use super::{Bsc, MobileRegistryService, VoiceAlertMode, VoiceLegRole, VoiceSessionKind};

/// Pending state for an MT call between sending the L3 trigger to the MSC
/// and receiving the A1 Assignment Request back. Keyed by the *stable*
/// `fwd_address` of the mobile rather than its registry index, because the
/// registry can shift indexes between these two events (eviction, retain).
pub(crate) struct PendingA1Assignment {
    pub(crate) fwd_address: MsAddress,
    pub(crate) ack_msg_seq: u8,
    pub(crate) requested_tx_time: Option<cdma_common::time::CdmaSystemTime>,
    pub(crate) tx_deadline: Option<cdma_common::time::CdmaSystemTime>,
    pub(crate) session_id: uuid::Uuid,
    pub(crate) leg_role: VoiceLegRole,
    pub(crate) bind_existing_traffic: bool,
}

pub(crate) struct A1Service {
    pub(crate) msc_client: Arc<dyn crate::a1_edge::MscClient>,
    pending_assignments: HashMap<u64, Vec<PendingA1Assignment>>,
}

impl A1Service {
    pub(crate) fn new(msc_client: Arc<dyn crate::a1_edge::MscClient>) -> Self {
        Self {
            msc_client,
            pending_assignments: HashMap::new(),
        }
    }
}

impl A1Service {
    pub(crate) fn push_pending_assignment(&mut self, call_id: u64, pending: PendingA1Assignment) {
        self.pending_assignments
            .entry(call_id)
            .or_default()
            .push(pending);
    }

    pub(crate) fn pop_pending_assignment(&mut self, call_id: u64) -> Option<PendingA1Assignment> {
        let pending = {
            let entries = self.pending_assignments.get_mut(&call_id)?;
            if entries.is_empty() {
                return None;
            }
            entries.remove(0)
        };
        if self
            .pending_assignments
            .get(&call_id)
            .is_some_and(|entries| entries.is_empty())
        {
            self.pending_assignments.remove(&call_id);
        }
        Some(pending)
    }

    pub(crate) fn clear_pending_assignments(&mut self, call_id: u64) {
        self.pending_assignments.remove(&call_id);
    }
}

impl Bsc {
    async fn handle_incoming_a1_message_inner(&mut self, message: EncodedA1Message) {
        let call_id = message.call_id();
        let decoded = match message.decode() {
            Ok(message) => message,
            Err(error) => {
                warn!("BSC: dropping malformed A1 message: {}", error);
                return;
            }
        };
        info!(
            "BSC: A1 rx {:?} call_id={:?}",
            decoded.message_type, call_id,
        );

        match decoded.message_type {
            cdma_ios::MessageType::PagingRequest => {
                let request = match cdma_ios::PagingRequestMessage::decode(&decoded.payload) {
                    Ok(request) => request,
                    Err(error) => {
                        warn!("BSC: failed to decode A1 Paging Request: {}", error);
                        return;
                    }
                };
                self.handle_a1_paging_request(call_id, request);
            }
            cdma_ios::MessageType::AssignmentRequest => {
                let request = match cdma_ios::AssignmentRequestMessage::decode(&decoded.payload) {
                    Ok(request) => request,
                    Err(error) => {
                        warn!("BSC: failed to decode A1 Assignment Request: {}", error);
                        return;
                    }
                };
                self.handle_a1_assignment_request(call_id, request).await;
            }
            cdma_ios::MessageType::ClearCommand => {
                let command = match cdma_ios::ClearCommandMessage::decode(&decoded.payload) {
                    Ok(command) => command,
                    Err(error) => {
                        warn!("BSC: failed to decode A1 Clear Command: {}", error);
                        return;
                    }
                };
                self.handle_a1_clear_command(call_id, command);
            }
            cdma_ios::MessageType::AddsPage => {
                let msg = match cdma_ios::AddsPageMessage::decode(&decoded.payload) {
                    Ok(m) => m,
                    Err(e) => {
                        warn!("BSC: failed to decode ADDS Page: {e}");
                        return;
                    }
                };
                self.handle_a1_adds_page(msg);
            }
            cdma_ios::MessageType::AlertWithInformation => {
                let awi = match cdma_ios::AlertWithInformationMessage::decode(&decoded.payload) {
                    Ok(awi) => awi,
                    Err(error) => {
                        warn!("BSC: failed to decode A1 AlertWithInformation: {}", error);
                        return;
                    }
                };
                self.handle_a1_alert_with_information(call_id, awi);
            }
            cdma_ios::MessageType::Progress => {
                let progress = match cdma_ios::ProgressMessage::decode(&decoded.payload) {
                    Ok(p) => p,
                    Err(error) => {
                        warn!("BSC: failed to decode A1 Progress: {}", error);
                        return;
                    }
                };
                self.handle_a1_progress(call_id, progress);
            }
            other => {
                warn!("BSC: A1 message {:?} not yet handled on live path", other);
            }
        }
    }
}

impl Bsc {
    fn handle_a1_alert_with_information(
        &mut self,
        call_id: Option<u64>,
        msg: cdma_ios::AlertWithInformationMessage,
    ) {
        let Some(call_id) = call_id else {
            warn!("BSC: refusing A1 AlertWithInformation without call correlation");
            return;
        };
        // MS-MS calls share one call_id across two TCs (caller + callee); pick
        // the Callee leg first since AWI with caller-ID belongs to it. Fall
        // back to whichever TC matches the call_id.
        let candidates = self.mobiles.all_walshes_for_a1_call(call_id);
        let walsh_code = candidates
            .iter()
            .find(|(_, w)| {
                self.mobiles
                    .get_traffic_channel(*w)
                    .and_then(|tc| tc.voice_leg_role)
                    == Some(VoiceLegRole::Callee)
            })
            .or_else(|| candidates.first())
            .map(|(_, w)| *w);
        let Some(walsh_code) = walsh_code else {
            warn!(
                "BSC: A1 AlertWithInformation for unknown call_id={}, ignoring",
                call_id
            );
            return;
        };

        let (leg_role, session_id) = self
            .mobiles
            .get_traffic_channel(walsh_code)
            .map(|tc| (tc.voice_leg_role, tc.voice_session_id))
            .unwrap_or((None, None));
        let session_kind = session_id.and_then(|id| self.voice.session(id).map(|s| s.kind));
        let stashed_calling_party = session_id.and_then(|id| {
            self.voice
                .session(id)
                .and_then(|s| s.calling_party_record.clone())
        });

        // ack_seq 0b111 is the documented default for air-interface AWIM.
        let (send_result, mode, log_label) = match leg_role {
            Some(VoiceLegRole::Callee) => {
                // Fall back to the AssignmentRequest-stashed record for senders
                // that don't populate ms_information_records on AWIM.
                let calling_party = msg
                    .ms_information_records
                    .as_ref()
                    .and_then(extract_calling_party_number_record)
                    .or(stashed_calling_party);
                (
                    self.send_standard_alert(walsh_code, 0b111, calling_party),
                    VoiceAlertMode::WaitForConnectOrder,
                    "standard alert",
                )
            }
            _ => (
                self.send_alert_with_info(walsh_code, 0b111, None),
                VoiceAlertMode::WaitForPeerAnswer,
                "ringback",
            ),
        };

        if let Err(error) = send_result {
            warn!(
                "BSC: failed to forward MSC AWIM to MS on walsh={}: {}",
                walsh_code, error
            );
            return;
        }
        info!(
            "BSC: forwarded MSC AWIM as {} on F-TCH walsh={} call_id={}",
            log_label, walsh_code, call_id
        );

        if let Some(tc) = self.mobiles.get_traffic_channel_mut(walsh_code) {
            tc.mark_voice_alerting(mode);
        }
        if let Some(session_id) = session_id {
            if let Some(session) = self.voice.session_mut(session_id) {
                let party = match leg_role {
                    Some(VoiceLegRole::Caller) => session.caller.as_mut(),
                    Some(VoiceLegRole::Callee) => session.callee.as_mut(),
                    None => None,
                };
                if let Some(party) = party {
                    party.service_connected = true;
                }
            }
            if matches!(
                session_kind,
                Some(VoiceSessionKind::MobileOriginatedExternal)
            ) && leg_role == Some(VoiceLegRole::Caller)
            {
                self.note_msc_external_call_after_service_connect(session_id, walsh_code);
            }
        }
    }

    /// A.S0014-D §2.1.6: relay Signal IE to air-interface AWIM.
    fn handle_a1_progress(&mut self, call_id: Option<u64>, progress: cdma_ios::ProgressMessage) {
        let Some(call_id) = call_id else {
            warn!("BSC: refusing A1 Progress without call correlation");
            return;
        };
        let Some(signal) = progress.signal else {
            debug!(
                "BSC: A1 Progress call_id={} has no Signal IE, nothing to forward",
                call_id
            );
            return;
        };
        // Signal IE always targets the Caller leg (the one we instructed to
        // play ringback). Callee AWI carries no Signal IE so it has no
        // network-instructed tone to silence on answer.
        let candidates = self.mobiles.all_walshes_for_a1_call(call_id);
        let walsh_code = candidates
            .iter()
            .find(|(_, w)| {
                self.mobiles
                    .get_traffic_channel(*w)
                    .and_then(|tc| tc.voice_leg_role)
                    == Some(VoiceLegRole::Caller)
            })
            .or_else(|| candidates.first())
            .map(|(_, w)| *w);
        let Some(walsh_code) = walsh_code else {
            warn!("BSC: A1 Progress for unknown call_id={}, ignoring", call_id);
            return;
        };
        let signal_info = cdma_common::lac::paging_messages::SignalInfoRecord {
            signal_type: 0x00, // C.S0005-E §3.7.5.5 SIGNAL_TYPE=Tone
            alert_pitch: signal.alert_pitch,
            signal: signal.signal_value,
        };
        if let Err(error) = self.send_alert_with_info_signal(walsh_code, 0b111, signal_info) {
            warn!(
                "BSC: failed to forward MSC Progress signal to MS on walsh={}: {}",
                walsh_code, error
            );
        }
    }

    fn handle_a1_adds_page(&mut self, msg: cdma_ios::AddsPageMessage) {
        let imsi = match &msg.mobile_identity {
            cdma_ios::MobileIdentity::Imsi(imsi) => imsi.clone(),
            other => {
                warn!(
                    "BSC: ADDS Page has non-IMSI mobile identity {:?} — dropped",
                    other
                );
                return;
            }
        };
        let a1_tag = msg.tag.map(|t| t.0);
        info!("BSC: ADDS Page from MSC for IMSI {} tag={:?}", imsi, a1_tag);

        let (s1, s2) = match cdma_common::paging::imsi_s_from_imsi(&imsi) {
            Some(pair) => pair,
            None => {
                warn!(
                    "BSC: ADDS Page IMSI {} not parseable for IMSI_S paging",
                    imsi
                );
                return;
            }
        };
        let target_address = format!("IMSI_S:s1={s1},s2={s2}");

        let sms_req = super::SmsRequest {
            originating_number: String::new(),
            text: String::new(),
            target_address: Some(target_address),
            target_subscriber_id: None,
            destination_number: None,
            timeout_ms: None,
            sms_id: None,
            delivery_attempt_id: None,
            a1_tag,
            raw_payload: Some(msg.adds_user_part.data),
        };
        self.handle_sms_request(sms_req);
    }
}

impl A1Service {
    pub(crate) fn send_assignment_complete(
        &self,
        mobiles: &MobileRegistryService,
        call_id: u64,
        walsh_code: u8,
        service_option: u16,
    ) {
        let a2p_bearer_session_params = mobiles
            .get_traffic_channel(walsh_code)
            .and_then(|tc| tc.msc_bearer_local_addr)
            .map(|addr| cdma_ios::A2pBearerSessionParams {
                ip_address: match addr.ip() {
                    std::net::IpAddr::V4(v4) => v4,
                    _ => std::net::Ipv4Addr::UNSPECIFIED,
                },
                udp_port: addr.port(),
            });
        let a2p_bearer_format_params =
            a2p_bearer_session_params
                .as_ref()
                .map(|_| cdma_ios::A2pBearerFormatParams {
                    formats: vec![cdma_ios::BearerFormatEntry {
                        bearer_format_tag_type: 1,
                        bearer_format_id: 0,
                        rtp_payload_type: cdma_ios::voice_bearer::EVRC_RTP_PAYLOAD_TYPE,
                        bearer_addr: None,
                    }],
                });
        let assignment_complete = cdma_ios::AssignmentCompleteMessage {
            channel_number: cdma_ios::ChannelNumber(walsh_code as u16),
            encryption_information: None,
            service_option: Some(cdma_ios::ServiceOption(service_option)),
            a2p_bearer_session_params,
            a2p_bearer_format_params,
        };
        let payload = match assignment_complete.encode() {
            Ok(payload) => payload,
            Err(error) => {
                warn!(
                    "BSC: failed to encode A1 Assignment Complete for call_id={}: {}",
                    call_id, error
                );
                return;
            }
        };
        self.send_message(call_id, cdma_ios::MessageType::AssignmentComplete, payload);
    }

    pub(crate) fn send_connect(&self, call_id: u64) {
        let payload = match cdma_ios::ConnectMessage.encode() {
            Ok(payload) => payload,
            Err(error) => {
                warn!(
                    "BSC: failed to encode A1 Connect for call_id={}: {}",
                    call_id, error
                );
                return;
            }
        };
        self.send_message(call_id, cdma_ios::MessageType::Connect, payload);
    }

    pub(crate) fn send_clear_request(&self, call_id: u64, cause: u8) {
        let clear_request = cdma_ios::ClearRequestMessage {
            cause: cdma_ios::Cause(cause),
            cause_layer3: None,
        };
        let payload = match clear_request.encode() {
            Ok(payload) => payload,
            Err(error) => {
                warn!(
                    "BSC: failed to encode A1 Clear Request for call_id={}: {}",
                    call_id, error
                );
                return;
            }
        };
        self.send_message(call_id, cdma_ios::MessageType::ClearRequest, payload);
    }

    pub(crate) fn send_assignment_failure(&self, call_id: u64, cause: u8) {
        let msg = cdma_ios::AssignmentFailureMessage {
            cause: cdma_ios::Cause(cause),
        };
        let payload = match msg.encode() {
            Ok(payload) => payload,
            Err(error) => {
                warn!(
                    "BSC: failed to encode A1 Assignment Failure for call_id={}: {}",
                    call_id, error
                );
                return;
            }
        };
        info!(
            "BSC: A1 tx AssignmentFailure call_id={} cause=0x{:02x}",
            call_id, cause
        );
        self.send_message(call_id, cdma_ios::MessageType::AssignmentFailure, payload);
    }

    pub(crate) fn send_clear_complete(&self, call_id: u64, power_down_indicator: bool) {
        let clear_complete = cdma_ios::ClearCompleteMessage {
            power_down_indicator,
        };
        let payload = match clear_complete.encode() {
            Ok(payload) => payload,
            Err(error) => {
                warn!(
                    "BSC: failed to encode A1 Clear Complete for call_id={}: {}",
                    call_id, error
                );
                return;
            }
        };
        self.send_message(call_id, cdma_ios::MessageType::ClearComplete, payload);
    }
}

impl Bsc {
    pub(crate) async fn send_complete_layer3_for_origination(
        &mut self,
        fwd_address: &MsAddress,
        event: &cdma_common::events::AccessChannelEvent,
        service_option: u16,
        called_number: Option<&str>,
        session_id: uuid::Uuid,
        leg_role: super::VoiceLegRole,
    ) -> Option<u64> {
        let call_id = self.voice.allocate_mo_call_id();

        let Some(l3_bytes) = complete_layer3_information_from_origination(
            self,
            fwd_address,
            event,
            service_option,
            called_number,
        ) else {
            warn!(
                "BSC: cannot send A1 Complete Layer 3 Information for MO call_id={}: cannot build CM Service Request",
                call_id
            );
            return None;
        };
        let cli3 = cdma_ios::CompleteLayer3InformationMessage {
            cell_identifier: cdma_ios::CellId {
                cell: self.config.overhead.base_id,
                sector: 0,
            },
            layer3_information: cdma_ios::Layer3Information(l3_bytes),
        };
        let payload = match cli3.encode() {
            Ok(payload) => payload,
            Err(error) => {
                warn!(
                    "BSC: failed to encode A1 Complete Layer 3 Information for MO call_id={}: {}",
                    call_id, error
                );
                return None;
            }
        };

        let requested_tx_time = super::access::access_response_tx_time(event);
        let tx_deadline = self.access_ack_deadline(event);
        let ack_msg_seq = event.msg_seq.unwrap_or(0);

        self.a1.push_pending_assignment(
            call_id,
            PendingA1Assignment {
                fwd_address: fwd_address.clone(),
                ack_msg_seq,
                requested_tx_time,
                tx_deadline,
                session_id,
                leg_role,
                bind_existing_traffic: false,
            },
        );

        let client = self.config.msc_client.clone();
        info!(
            "BSC: A1 tx {:?} call_id={}",
            cdma_ios::MessageType::CompleteLayer3Information,
            call_id
        );
        let encoded = EncodedA1Message::from_message_for_call(
            &cdma_ios::Message::new(cdma_ios::MessageType::CompleteLayer3Information, payload),
            Some(call_id),
        );
        if let Err(error) = client.send_a1(encoded).await {
            warn!(
                "BSC: failed to send CompleteLayer3Information to MSC: {}",
                error
            );
            self.a1.clear_pending_assignments(call_id);
            return None;
        }
        Some(call_id)
    }

    pub(crate) fn handle_mt_page_response(
        &mut self,
        event: &cdma_common::events::AccessChannelEvent,
        pending: &super::PendingVoicePage,
        fwd_address: &MsAddress,
    ) -> bool {
        let Some(call_id) = pending.a1_call_id else {
            return false;
        };
        let Some(imsi) = pending.imsi.clone() else {
            warn!(
                "BSC: cannot build A1 Paging Response for call_id={} without IMSI",
                call_id
            );
            return false;
        };

        let paging_response = cdma_ios::PagingResponseMessage {
            classmark_information_type_2: build_a1_classmark_information_type_2_for_event(event, 0),
            mobile_identity_imsi: cdma_ios::MobileIdentity::Imsi(imsi),
            tag: pending.a1_tag,
            mobile_identity_esn: event.esn.map(cdma_ios::MobileIdentity::Esn),
            slot_cycle_index: event.slot_cycle_index.map(cdma_ios::SlotCycleIndex),
            authentication_response_parameter: None,
            authentication_confirmation_parameter: None,
            authentication_parameter_count: None,
            authentication_challenge_parameter: None,
            service_option: Some(cdma_ios::ServiceOption(
                event.service_option.unwrap_or(pending.service_option),
            )),
            voice_privacy_request: a1_voice_privacy_requested(event),
            circuit_identity_code: None,
            authentication_event: None,
            radio_environment_and_resources: None,
            user_zone_id: None,
            is2000_mobile_capabilities: None,
            cdma_serving_one_way_delay: None,
        };
        let encoded = match paging_response.encode() {
            Ok(payload) => EncodedA1Message::from_message_for_call(
                &cdma_ios::Message::new(cdma_ios::MessageType::PagingResponse, payload),
                Some(call_id),
            ),
            Err(error) => {
                warn!(
                    "BSC: failed to encode A1 Paging Response for call_id={}: {}",
                    call_id, error
                );
                return false;
            }
        };

        let requested_tx_time = super::access::access_response_tx_time(event);
        let tx_deadline = self.access_ack_deadline(event);
        let ack_msg_seq = event.msg_seq.unwrap_or(0);

        self.a1.push_pending_assignment(
            call_id,
            PendingA1Assignment {
                fwd_address: fwd_address.clone(),
                ack_msg_seq,
                requested_tx_time,
                tx_deadline,
                session_id: pending.session_id,
                leg_role: pending.leg_role,
                bind_existing_traffic: false,
            },
        );
        info!(
            "BSC: deferring Page Response ACK to A1 assignment for call_id={} ack_seq={}",
            call_id, ack_msg_seq
        );

        if let Some(pending) = self.paging.take_voice_page_for_a1_call(call_id) {
            self.clear_pending_page_records_for(&pending.page_address);
        }

        let client = self.config.msc_client.clone();
        spawn(async move {
            if let Err(error) = client.send_a1(encoded).await {
                warn!("BSC: failed to send A1 Paging Response to MSC: {}", error);
            }
        });
        true
    }

    pub(crate) fn send_existing_traffic_paging_response(
        &mut self,
        call_id: u64,
        fwd_address: &MsAddress,
        service_option: u16,
        session_id: uuid::Uuid,
        leg_role: VoiceLegRole,
    ) -> bool {
        let Some(ms) = self.mobiles.get(fwd_address) else {
            warn!(
                "BSC: existing-traffic A1 Paging Response: mobile {} no longer registered (call_id={})",
                format_ms_address(fwd_address),
                call_id
            );
            return false;
        };
        let mobile_identity = match &ms.fwd_address {
            cdma_common::lac::paging_messages::MsAddress::Esn(esn) => {
                warn!(
                    "BSC: cannot send A1 existing-traffic Paging Response for ESN-only address 0x{:08X}",
                    esn
                );
                return false;
            }
            cdma_common::lac::paging_messages::MsAddress::ImsiS {
                imsi_m_s1,
                imsi_m_s2,
            } => cdma_ios::MobileIdentity::Imsi(cdma_common::paging::imsi_s_to_digits(
                *imsi_m_s1, *imsi_m_s2,
            )),
            cdma_common::lac::paging_messages::MsAddress::ImsiClass0 {
                imsi_m_s1,
                imsi_m_s2,
                ..
            } => cdma_ios::MobileIdentity::Imsi(cdma_common::paging::imsi_s_to_digits(
                *imsi_m_s1, *imsi_m_s2,
            )),
        };

        let response = cdma_ios::PagingResponseMessage {
            classmark_information_type_2: build_a1_classmark_information_type_2_from_projection(
                A1ClassmarkProjection {
                    mob_p_rev: ms.mob_p_rev,
                    scm: 0,
                    slotted: ms.slot_cycle_index != 0,
                    dtx: false,
                    mob_term: true,
                    nar_an_cap: false,
                    paca_supported: None,
                },
                0,
            ),
            mobile_identity_imsi: mobile_identity,
            tag: Some(cdma_ios::Tag(call_id as u32)),
            mobile_identity_esn: ms.esn.map(cdma_ios::MobileIdentity::Esn),
            slot_cycle_index: Some(cdma_ios::SlotCycleIndex(ms.slot_cycle_index)),
            authentication_response_parameter: None,
            authentication_confirmation_parameter: None,
            authentication_parameter_count: None,
            authentication_challenge_parameter: None,
            service_option: Some(cdma_ios::ServiceOption(service_option)),
            voice_privacy_request: false,
            circuit_identity_code: None,
            authentication_event: None,
            radio_environment_and_resources: None,
            user_zone_id: None,
            is2000_mobile_capabilities: None,
            cdma_serving_one_way_delay: None,
        };

        let encoded = match response.encode() {
            Ok(payload) => EncodedA1Message::from_message_for_call(
                &cdma_ios::Message::new(cdma_ios::MessageType::PagingResponse, payload),
                Some(call_id),
            ),
            Err(error) => {
                warn!(
                    "BSC: failed to encode A1 existing-traffic Paging Response for call_id={}: {}",
                    call_id, error
                );
                return false;
            }
        };

        self.a1.push_pending_assignment(
            call_id,
            PendingA1Assignment {
                fwd_address: fwd_address.clone(),
                ack_msg_seq: 0,
                requested_tx_time: None,
                tx_deadline: None,
                session_id,
                leg_role,
                bind_existing_traffic: true,
            },
        );

        if let Some(pending) = self
            .paging
            .take_voice_page_for_a1_call_or_session(call_id, session_id)
        {
            self.clear_pending_page_records_for(&pending.page_address);
        }

        let client = self.config.msc_client.clone();
        info!(
            "BSC: sending A1 existing-traffic Paging Response call_id={} addr={} session={} leg={:?}",
            call_id,
            format_ms_address(fwd_address),
            session_id,
            leg_role
        );
        spawn(async move {
            if let Err(error) = client.send_a1(encoded).await {
                warn!(
                    "BSC: failed to send existing-traffic A1 Paging Response to MSC: {}",
                    error
                );
            }
        });
        true
    }

    pub(crate) async fn handle_a1_assignment_request(
        &mut self,
        call_id: Option<u64>,
        request: cdma_ios::AssignmentRequestMessage,
    ) {
        let Some(call_id) = call_id else {
            warn!("BSC: refusing A1 Assignment Request without transport call correlation");
            return;
        };
        let Some(pending) = self.a1.pop_pending_assignment(call_id) else {
            warn!(
                "BSC: no pending MT assignment state for A1 Assignment Request call_id={}",
                call_id
            );
            return;
        };

        // Re-resolve the target mobile by its stable address. The pending
        // state has lived across the round-trip to the MSC, so the registry
        // may have shifted (eviction etc.).
        let Some(mobile) = self.mobiles.get(&pending.fwd_address) else {
            warn!(
                "BSC: A1 Assignment Request call_id={} target {} no longer registered, dropping",
                call_id,
                format_ms_address(&pending.fwd_address)
            );
            return;
        };

        let service_option = request
            .service_option
            .map(|value| value.0)
            .unwrap_or_else(|| mobile.traffic_service_option_or(3));

        let circuit_id = request.circuit_identity_code.to_packed();
        let calling_party = request
            .ms_information_records
            .as_ref()
            .and_then(extract_calling_party_number_record);

        let existing_voice_walsh = mobile.existing_voice_walsh_for_assignment(
            pending.bind_existing_traffic,
            pending.session_id,
            pending.leg_role,
        );

        if let Some(walsh_code) = existing_voice_walsh {
            self.mobiles.update_tc(walsh_code, |_, tc| {
                tc.msc_circuit_id = Some(circuit_id);
                tc.voice_session_id = Some(pending.session_id);
                tc.voice_leg_role = Some(pending.leg_role);
                tc.a1_call_id = Some(call_id);
            });
            if let Some(record) = calling_party.clone() {
                if let Some(session) = self.voice.session_mut(pending.session_id) {
                    session.calling_party_record = Some(record);
                }
            }

            if let Some(bearer) = self.config.msc_voice_bearer.as_ref() {
                let msc_remote = request.a2p_bearer_session_params.map(|params| {
                    std::net::SocketAddr::new(
                        std::net::IpAddr::V4(params.ip_address),
                        params.udp_port,
                    )
                });
                match bearer.open_circuit(circuit_id, msc_remote).await {
                    Ok(local_addr) => {
                        // After awaiting the bearer open, re-resolve via walsh
                        // (the walsh code is the stable key for the TC).
                        self.mobiles.update_tc(walsh_code, |_, tc| {
                            tc.msc_bearer_local_addr = Some(local_addr);
                        });
                    }
                    Err(e) => {
                        warn!("BSC: failed to open bearer circuit {circuit_id}: {e}");
                    }
                }
            }

            match self.start_mt_voice_on_existing_traffic(
                &pending.fwd_address,
                service_option,
                pending.session_id,
                pending.leg_role,
                Some(call_id),
            ) {
                Ok(started_walsh) => {
                    info!(
                        "BSC: applied A1 Assignment Request to existing F-TCH walsh={} circuit_id={}",
                        started_walsh, circuit_id
                    );
                }
                Err(error) => {
                    warn!(
                        "BSC: failed to apply A1 Assignment Request to existing F-TCH walsh={}: {}",
                        walsh_code, error
                    );
                }
            }
            return;
        }

        if let Err(error) = self
            .allocate_voice_channel_for_mobile(
                &pending.fwd_address,
                service_option,
                pending.ack_msg_seq,
                pending.requested_tx_time,
                pending.tx_deadline,
                Some(pending.session_id),
                Some(pending.leg_role),
                Some(call_id),
            )
            .await
        {
            warn!("BSC: failed to apply A1 Assignment Request: {}", error);
            return;
        }

        let assigned_circuit = self
            .mobiles
            .update(&pending.fwd_address, |ms| {
                ms.set_msc_circuit_id(circuit_id).is_some()
            })
            .unwrap_or(false);

        if assigned_circuit {
            if let Some(bearer) = self.config.msc_voice_bearer.as_ref() {
                let msc_remote = request.a2p_bearer_session_params.map(|params| {
                    std::net::SocketAddr::new(
                        std::net::IpAddr::V4(params.ip_address),
                        params.udp_port,
                    )
                });
                match bearer.open_circuit(circuit_id, msc_remote).await {
                    Ok(local_addr) => {
                        self.mobiles.update(&pending.fwd_address, |ms| {
                            ms.set_msc_bearer_local_addr(local_addr);
                        });
                    }
                    Err(e) => {
                        warn!("BSC: failed to open bearer circuit {circuit_id}: {e}");
                    }
                }
            }
        }
        if let Some(record) = calling_party {
            if let Some(session) = self.voice.session_mut(pending.session_id) {
                session.calling_party_record = Some(record);
            }
        }
    }

    pub(crate) fn handle_a1_clear_command(
        &mut self,
        call_id: Option<u64>,
        _command: cdma_ios::ClearCommandMessage,
    ) {
        let Some(call_id) = call_id else {
            warn!("BSC: refusing A1 Clear Command without transport call correlation");
            return;
        };

        if let Some(pending) = self.a1.pop_pending_assignment(call_id) {
            self.voice
                .retain_sessions(|session| session.id != pending.session_id);
            self.a1.send_clear_complete(call_id, false);
            return;
        }

        if let Some(pending) = self.paging.take_voice_page_for_a1_call(call_id) {
            self.voice
                .retain_sessions(|session| session.id != pending.session_id);
            self.mobiles.release_paged_without_tc();
            self.publish_mobiles();
            self.a1.send_clear_complete(call_id, false);
            return;
        }

        let Some((fwd_address, walsh_code)) = self.mobiles.locate_a1_call(call_id) else {
            info!(
                "BSC: A1 Clear Command for call_id={} arrived after local teardown; sending Clear Complete",
                call_id
            );
            self.a1.send_clear_complete(call_id, false);
            return;
        };

        let already_releasing = self.mobiles.update_tc(walsh_code, |_, tc| {
            tc.a1_clear_state = A1ClearState::ClearCommandReceived;
            tc.is_releasing()
        });
        let Some(already_releasing) = already_releasing else {
            self.a1.send_clear_complete(call_id, false);
            return;
        };

        if !already_releasing {
            self.begin_voice_release(&fwd_address, 0b111, "A1 Clear Command");
        }
    }

    pub(crate) fn handle_a1_paging_request(
        &mut self,
        call_id: Option<u64>,
        request: cdma_ios::PagingRequestMessage,
    ) {
        let cdma_ios::MobileIdentity::Imsi(imsi) = request.mobile_identity_imsi else {
            warn!("BSC: refusing non-IMSI A1 Paging Request");
            return;
        };

        let Some(tag) = request.tag else {
            warn!("BSC: refusing A1 Paging Request without Tag");
            return;
        };

        let service_option = request.service_option.map(|so| so.0).unwrap_or(3);

        let (fwd_address, subscriber_id) = match self.mobiles.get_by_imsi(&imsi) {
            Some(mobile) => (
                mobile.fwd_address.clone(),
                mobile.subscriber_id.unwrap_or_default(),
            ),
            None => {
                // Page anyway via a synthesized address; queue_voice_page_for_mobile
                // falls back to slot_cycle_index=0 when the registry is empty.
                let Some(addr) = synthesize_fwd_address_from_imsi(&imsi) else {
                    warn!(
                        "BSC: A1 Paging Request IMSI {} could not be parsed; replying ClearRequest(0x6E)",
                        imsi
                    );
                    if let Some(call_id) = call_id {
                        self.a1.send_clear_request(call_id, 0x6E);
                    }
                    return;
                };
                warn!(
                    "BSC: A1 Paging Request target IMSI {} not in registry; attempting page from synthesized address",
                    imsi
                );
                (addr, uuid::Uuid::nil())
            }
        };

        let staging = cdma_msc::MtCallPlan {
            subscriber_id,
            imsi,
            audio_file: None,
            caller_number: None,
            service_option,
        };

        self.start_bs_voice_call_from_a1(
            &fwd_address,
            tag,
            call_id.or(Some(tag.0 as u64)),
            staging,
        );
    }

    fn start_bs_voice_call_from_a1(
        &mut self,
        fwd_address: &MsAddress,
        tag: cdma_ios::Tag,
        call_id: Option<u64>,
        staging: cdma_msc::MtCallPlan,
    ) {
        info!(
            "BSC: starting MT voice page from A1 tag={} subscriber={}",
            tag.0, staging.subscriber_id
        );
        self.start_bs_voice_call_for_mobile(
            fwd_address,
            staging.service_option,
            Some(tag),
            call_id,
            Some(staging.imsi),
        );
    }
}

impl A1Service {
    fn send_message(&self, call_id: u64, message_type: cdma_ios::MessageType, payload: Vec<u8>) {
        let encoded = EncodedA1Message::from_message_for_call(
            &cdma_ios::Message::new(message_type, payload),
            Some(call_id),
        );
        let client = self.msc_client.clone();
        info!("BSC: A1 tx {:?} call_id={}", message_type, call_id);
        spawn(async move {
            if let Err(error) = client.send_a1(encoded).await {
                warn!("BSC: failed to send {:?} to MSC: {}", message_type, error);
            }
        });
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum A1ClearState {
    Idle,
    ClearRequestSent,
    ClearCommandReceived,
}

impl Bsc {
    pub(crate) async fn handle_incoming_a1_message(&mut self, message: EncodedA1Message) {
        self.handle_incoming_a1_message_inner(message).await;
    }
}

/// Pull a Calling Party Number record (`record_type = 0x03`) out of the
/// IOS-A.S0014-D §4.2.55 MS Information Records IE. Per the spec the BS
/// transparently re-emits these bytes inside the AWIM SDU on the F-TCH,
/// so we just decode the C.S0005-E §3.7.5.3 content and stash it.
/// Build a paging address from a 10–15 digit IMSI for pages to unregistered
/// MSs. 15 digits → `ImsiClass0` (carries MCC + IMSI_11_12); else `ImsiS`.
fn synthesize_fwd_address_from_imsi(imsi: &str) -> Option<MsAddress> {
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

fn extract_calling_party_number_record(
    records: &cdma_ios::MsInformationRecords,
) -> Option<CallingPartyNumberRecord> {
    const CALLING_PARTY_NUMBER_RECORD_TYPE: u8 = 0x03;
    let record = records
        .records
        .iter()
        .find(|r| r.record_type == CALLING_PARTY_NUMBER_RECORD_TYPE)?;
    match CallingPartyNumberRecord::decode_content_bytes(&record.content) {
        Ok(decoded) => Some(decoded),
        Err(e) => {
            warn!("BSC: failed to decode AWIM Calling Party Number record: {e}");
            None
        }
    }
}

fn complete_layer3_information_from_origination(
    bsc: &Bsc,
    fwd_address: &MsAddress,
    event: &cdma_common::events::AccessChannelEvent,
    service_option: u16,
    called_number: Option<&str>,
) -> Option<Vec<u8>> {
    let cdma_common::access::AccessMessage::Origination(origination) = event.decoded_l3.as_ref()?
    else {
        return None;
    };
    let mobile = bsc.mobiles.get(fwd_address)?;
    let imsi = event
        .imsi
        .clone()
        .or_else(|| mobile.canonical_imsi.clone())
        .or_else(|| mobile.imsi.clone())?;
    let called_party_bcd_number = called_number.and_then(called_party_bcd_number_from_digits);
    let called_party_ascii_number = if called_number.is_some() && called_party_bcd_number.is_none()
    {
        called_number
            .filter(|digits| !digits.is_empty())
            .map(|digits| cdma_ios::CallingPartyAsciiNumber(digits.as_bytes().to_vec()))
    } else {
        None
    };
    let cm_service_request = cdma_ios::CmServiceRequestMessage {
        cm_service_type: cdma_ios::CmServiceType::MobileOriginatingCallEstablishment,
        classmark_information_type_2: build_a1_classmark_information_type_2_for_event(event, 0),
        mobile_identity_imsi: cdma_ios::MobileIdentity::Imsi(imsi),
        called_party_bcd_number,
        tag: None,
        mobile_identity_esn: event.esn.or(mobile.esn).map(cdma_ios::MobileIdentity::Esn),
        slot_cycle_index: Some(cdma_ios::SlotCycleIndex(origination.slot_cycle_index)),
        authentication_response_parameter: None,
        authentication_confirmation_parameter: None,
        authentication_parameter_count: None,
        authentication_challenge_parameter: None,
        service_option: Some(cdma_ios::ServiceOption(service_option)),
        voice_privacy_request: false,
        radio_environment_and_resources: None,
        called_party_ascii_number,
        circuit_identity_code: None,
        authentication_event: None,
        authentication_data: None,
        paca_reorigination_indicator: origination.paca_reorig,
        user_zone_id: origination.uzid.map(cdma_ios::UserZoneId),
        is2000_mobile_capabilities: None,
        cdma_serving_one_way_delay: None,
    };
    cdma_ios::Layer3Information::from_cm_service_request(&cm_service_request)
        .ok()
        .map(|layer3| layer3.0)
}

fn a1_voice_privacy_requested(event: &cdma_common::events::AccessChannelEvent) -> bool {
    event
        .decoded_l3
        .as_ref()
        .and_then(|message| match message {
            cdma_common::access::AccessMessage::PageResponse(msg) => Some(msg.pm),
            cdma_common::access::AccessMessage::Origination(msg) => Some(msg.pm),
            _ => None,
        })
        .unwrap_or(false)
}

fn called_party_bcd_number_from_digits(digits: &str) -> Option<cdma_ios::CalledPartyBcdNumber> {
    let (type_of_number, digits) = if let Some(rest) = digits.strip_prefix('+') {
        (0b001, rest)
    } else {
        (0b000, digits)
    };
    if digits.is_empty() {
        return None;
    }
    let nibbles = digits
        .bytes()
        .map(|digit| match digit {
            b'0'..=b'9' => Some(digit - b'0'),
            b'*' => Some(0x0a),
            b'#' => Some(0x0b),
            b'a' | b'A' => Some(0x0c),
            b'b' | b'B' => Some(0x0d),
            b'c' | b'C' => Some(0x0e),
            _ => None,
        })
        .collect::<Option<Vec<_>>>()?;
    if nibbles.len() > 32 {
        return None;
    }
    let mut payload = Vec::with_capacity(1 + nibbles.len().div_ceil(2));
    payload.push(0x80 | ((type_of_number & 0x07) << 4) | 0x01);
    for pair in nibbles.chunks(2) {
        let low = pair[0] & 0x0f;
        let high = pair.get(1).copied().unwrap_or(0x0f) & 0x0f;
        payload.push(low | (high << 4));
    }
    Some(cdma_ios::CalledPartyBcdNumber(payload))
}

#[derive(Debug, Clone, Copy)]
struct A1ClassmarkProjection {
    mob_p_rev: u8,
    scm: u8,
    slotted: bool,
    dtx: bool,
    mob_term: bool,
    nar_an_cap: bool,
    paca_supported: Option<bool>,
}

fn project_a1_classmark_from_event(
    event: &cdma_common::events::AccessChannelEvent,
) -> A1ClassmarkProjection {
    if let Some(message) = event.decoded_l3.as_ref() {
        match message {
            cdma_common::access::AccessMessage::PageResponse(msg) => {
                return A1ClassmarkProjection {
                    mob_p_rev: msg.mob_p_rev,
                    scm: msg.scm,
                    slotted: msg.slot_cycle_index != 0,
                    dtx: (msg.scm & 0b0000_0100) != 0,
                    mob_term: msg.mob_term,
                    nar_an_cap: msg.nar_an_cap,
                    paca_supported: None,
                };
            }
            cdma_common::access::AccessMessage::Origination(msg) => {
                return A1ClassmarkProjection {
                    mob_p_rev: msg.mob_p_rev,
                    scm: msg.scm,
                    slotted: msg.slot_cycle_index != 0,
                    dtx: (msg.scm & 0b0000_0100) != 0,
                    mob_term: msg.mob_term,
                    nar_an_cap: msg.nar_an_cap,
                    paca_supported: Some(msg.paca_supported),
                };
            }
            cdma_common::access::AccessMessage::Registration(msg) => {
                return A1ClassmarkProjection {
                    mob_p_rev: msg.mob_p_rev,
                    scm: msg.scm,
                    slotted: msg.slot_cycle_index != 0,
                    dtx: (msg.scm & 0b0000_0100) != 0,
                    mob_term: msg.mob_term,
                    nar_an_cap: false,
                    paca_supported: None,
                };
            }
            _ => {}
        }
    }

    let mob_p_rev = event.mob_p_rev.unwrap_or(6);
    let scm = event.scm.unwrap_or(0);
    A1ClassmarkProjection {
        mob_p_rev,
        scm,
        slotted: event.slot_cycle_index.unwrap_or(0) != 0,
        dtx: (scm & 0b0000_0100) != 0,
        mob_term: event
            .decoded_l3
            .as_ref()
            .and_then(|message| match message {
                cdma_common::access::AccessMessage::PageResponse(msg) => Some(msg.mob_term),
                cdma_common::access::AccessMessage::Origination(msg) => Some(msg.mob_term),
                cdma_common::access::AccessMessage::Registration(msg) => Some(msg.mob_term),
                _ => None,
            })
            .unwrap_or(true),
        nar_an_cap: false,
        paca_supported: None,
    }
}

fn build_a1_classmark_information_type_2_for_event(
    event: &cdma_common::events::AccessChannelEvent,
    serving_band_class: u8,
) -> cdma_ios::ClassmarkInformationType2 {
    let projection = project_a1_classmark_from_event(event);
    build_a1_classmark_information_type_2_from_projection(projection, serving_band_class)
}

fn build_a1_classmark_information_type_2_from_projection(
    projection: A1ClassmarkProjection,
    serving_band_class: u8,
) -> cdma_ios::ClassmarkInformationType2 {
    // SCM bit 1 = Band Class 0 Power Class (C.S0005-E §2.3.3):
    // 0 → Class I (rf_power_capability 0), 1 → Class II (rf_power_capability 1).
    let rf_power_capability = (projection.scm >> 1) & 0x01;
    // PACA_SUPPORTED is only carried in Origination (C.S0005-E §2.7.1.3.2.4),
    // not in Page Response or Registration — None is spec-correct for those paths.
    let paca_supported = projection.paca_supported.unwrap_or(false);

    let octet3 = ((projection.mob_p_rev & 0x07) << 5) | (1 << 3) | (rf_power_capability & 0x07);
    let octet5 = ((projection.nar_an_cap as u8) << 7)
        | (1 << 6)
        | ((projection.slotted as u8) << 5)
        | ((projection.dtx as u8) << 2)
        | ((projection.mob_term as u8) << 1);
    let octet7 = ((projection.mob_term as u8) << 1) | (paca_supported as u8);

    // The runtime currently operates as a Band Class 0 system only, so the
    // mandatory first band-class entry reflects Band Class 0 CDMA mode.
    let band_class_entry_air_interface_supported = 0x00;

    cdma_ios::ClassmarkInformationType2(vec![
        octet3,
        0x00,
        octet5,
        0x00,
        octet7,
        0x01,
        projection.scm,
        0x01,
        0x03,
        serving_band_class,
        band_class_entry_air_interface_supported,
        projection.mob_p_rev,
    ])
}

#[cfg(test)]
mod tests {
    use std::{sync::Arc, time::Duration};

    use super::{
        build_a1_classmark_information_type_2_for_event, called_party_bcd_number_from_digits,
    };
    use crate::a1_edge::{EncodedA1Message, InProcessMscClient};
    use crate::bsc::tests::test_bsc_with_active_traffic_channel;
    use crate::bsc::{PendingA1Assignment, VoiceLegRole};
    use cdma_common::access::{
        AccessMessage, AccessMessageHeader, OriginationMessage, PageResponseMessage,
    };
    use cdma_common::consts::{SERVICE_OPTION_EVRC_A, SERVICE_OPTION_HIGH_RATE_PACKET_DATA};
    use cdma_common::events::AccessChannelEvent;
    use cdma_common::lac::message_types::MessageId;
    use tokio::time::timeout;
    use uuid::Uuid;

    fn test_access_event() -> AccessChannelEvent {
        AccessChannelEvent {
            event_id: "a1-classmark-test".to_string(),
            chip_start: 0,
            absolute_chip_start: None,
            receive_time: None,
            preamble_frames: 0,
            pd: 1,
            message_id: MessageId::PageResponse,
            msg_type_name: "Page Response Message".to_string(),
            address: None,
            resolved_address: None,
            subscriber_id: None,
            l3_summary: None,
            decoded_l3: None,
            pdu_summary: String::new(),
            msg_seq: None,
            ack_seq: None,
            ack_req: false,
            valid_ack: false,
            msid_type: None,
            esn: None,
            imsi: None,
            imsi_m_s1: None,
            imsi_m_s2: None,
            imsi_class: None,
            imsi_addr_num: None,
            imsi_mcc: None,
            imsi_11_12: None,
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

    #[test]
    fn called_party_bcd_number_encodes_national_digits() {
        assert_eq!(
            called_party_bcd_number_from_digits("555998"),
            Some(cdma_ios::CalledPartyBcdNumber(vec![0x81, 0x55, 0x95, 0x89]))
        );
    }

    #[test]
    fn called_party_bcd_number_encodes_international_odd_digits() {
        assert_eq!(
            called_party_bcd_number_from_digits("+12345"),
            Some(cdma_ios::CalledPartyBcdNumber(vec![0x91, 0x21, 0x43, 0xf5]))
        );
    }

    #[test]
    fn called_party_bcd_number_rejects_non_bcd_digits() {
        assert_eq!(called_party_bcd_number_from_digits("555-998"), None);
    }

    #[test]
    fn classmark_projection_uses_decoded_page_response_fields() {
        let mut event = test_access_event();
        event.mob_p_rev = Some(3);
        event.slot_cycle_index = Some(0);
        event.scm = Some(0x00);
        event.decoded_l3 = Some(AccessMessage::PageResponse(PageResponseMessage {
            header: AccessMessageHeader {
                pd: 1,
                message_id: MessageId::PageResponse,
            },
            mob_term: false,
            slot_cycle_index: 3,
            mob_p_rev: 12,
            scm: 0xB4,
            request_mode: 1,
            service_option: 1,
            pm: false,
            nar_an_cap: true,
            encryption_supported: None,
            num_alt_so: 0,
            alt_service_options: Vec::new(),
            uzid_incl: Some(false),
            uzid: None,
            ch_ind: Some(0b01),
            otd_supported: Some(true),
            qpch_supported: Some(true),
            enhanced_rc: Some(true),
            for_rc_pref: Some(4),
            rev_rc_pref: Some(3),
            fch_supported: Some(true),
            fch_capability: None,
            dcch_supported: Some(false),
            dcch_capability: None,
            rev_fch_gating_req: Some(true),
            sts_supported: Some(false),
            cch_3x_supported: Some(false),
            wll_incl: Some(false),
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

        let classmark = build_a1_classmark_information_type_2_for_event(&event, 0);
        assert_eq!(
            classmark.0,
            vec![
                0x88, 0x00, 0xE4, 0x00, 0x00, 0x01, 0xB4, 0x01, 0x03, 0x00, 0x00, 0x0C
            ]
        );
    }

    #[test]
    fn classmark_projection_uses_direct_paca_capability_when_available() {
        let mut event = test_access_event();
        event.message_id = MessageId::Origination;
        event.msg_type_name = "Origination Message".to_string();
        event.decoded_l3 = Some(AccessMessage::Origination(OriginationMessage {
            header: AccessMessageHeader {
                pd: 1,
                message_id: MessageId::Origination,
            },
            mob_term: true,
            slot_cycle_index: 2,
            mob_p_rev: 6,
            scm: 0x24,
            request_mode: 1,
            special_service: false,
            service_option: Some(1),
            pm: false,
            digit_mode: false,
            number_type: None,
            number_plan: None,
            more_fields: false,
            num_fields: 0,
            digits: Vec::new(),
            nar_an_cap: false,
            paca_reorig: false,
            return_cause: 0,
            more_records: false,
            encryption_supported: None,
            paca_supported: true,
            num_alt_so: 0,
            alt_service_options: Vec::new(),
            drs: None,
            uzid_incl: Some(false),
            uzid: None,
            ch_ind: Some(0b01),
            sr_id: Some(0),
            otd_supported: Some(false),
            qpch_supported: Some(false),
            enhanced_rc: Some(false),
            for_rc_pref: Some(1),
            rev_rc_pref: Some(1),
            fch_supported: Some(true),
            fch_capability: None,
            dcch_supported: Some(false),
            dcch_capability: None,
            geo_loc_incl: None,
            geo_loc_type: None,
            rev_fch_gating_req: Some(false),
            orig_reason: None,
            orig_count: None,
            sts_supported: None,
            cch_3x_supported: None,
            wll_incl: None,
            wll_device_type: None,
            global_emergency_call: None,
            ms_init_pos_loc_ind: None,
            qos_parms_incl: None,
            qos_parms_len: None,
            qos_parms: Vec::new(),
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
            prev_sid_incl: None,
            prev_sid: None,
            prev_nid_incl: None,
            prev_nid: None,
            prev_pzid_incl: None,
            prev_pzid: None,
            so_bitmap_ind: None,
            so_group_num: None,
            so_bitmap: None,
            sdb_desired_only: None,
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
            add_serv_instance_incl: None,
            add_service_instances: Vec::new(),
            bcmc_incl: None,
            bcmc: None,
            rev_pdch_supported: None,
            rev_pdch_capability: None,
            band_sub_rep_incl: None,
            num_band_subclass: None,
            band_subclass_sup: Vec::new(),
            add_geo_loc_incl: None,
            add_geo_loc_type_len_ind: None,
            add_geo_loc_type: None,
            remaining_bits: 0,
        }));

        let classmark = build_a1_classmark_information_type_2_for_event(&event, 0);
        assert_eq!(classmark.0[4] & 0x03, 0x03);
    }

    #[test]
    fn classmark_rf_power_capability_sourced_from_scm_bit1() {
        let mut event = test_access_event();
        // SCM 0x02 → bit 1 set → Band Class 0 Class II → rf_power_capability = 1
        event.decoded_l3 = Some(AccessMessage::PageResponse(PageResponseMessage {
            header: AccessMessageHeader {
                pd: 1,
                message_id: MessageId::PageResponse,
            },
            mob_term: true,
            slot_cycle_index: 0,
            mob_p_rev: 6,
            scm: 0x02,
            request_mode: 0,
            service_option: 1,
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

        let classmark = build_a1_classmark_information_type_2_for_event(&event, 0);
        // octet3 low 3 bits = rf_power_capability = 1 (Class II)
        assert_eq!(classmark.0[0] & 0x07, 0x01);
    }

    fn clear_command_message(call_id: u64) -> EncodedA1Message {
        EncodedA1Message::from_message_for_call(
            &cdma_ios::Message::new(
                cdma_ios::MessageType::ClearCommand,
                cdma_ios::ClearCommandMessage {
                    cause: cdma_ios::Cause(0x09),
                    cause_layer3: None,
                }
                .encode()
                .unwrap(),
            ),
            Some(call_id),
        )
    }

    fn assignment_request_message(call_id: u64, circuit_id: u16) -> EncodedA1Message {
        EncodedA1Message::from_message_for_call(
            &cdma_ios::Message::new(
                cdma_ios::MessageType::AssignmentRequest,
                cdma_ios::AssignmentRequestMessage {
                    channel_type: cdma_ios::ChannelType {
                        speech_or_data_indicator: 0x01,
                        channel_rate_and_type: 0x08,
                        coding: 0x05,
                    },
                    circuit_identity_code: cdma_ios::CircuitIdentityCode {
                        pcm_multiplexer: (circuit_id >> 5) & 0x07ff,
                        timeslot: (circuit_id & 0x1f) as u8,
                    },
                    encryption_information: None,
                    service_option: Some(cdma_ios::ServiceOption::EVRC_A),
                    signals: Vec::new(),
                    ms_information_records: None,
                    priority: None,
                    paca_timestamp: None,
                    quality_of_service_parameters: None,
                    a2p_bearer_session_params: None,
                    a2p_bearer_format_params: None,
                }
                .encode()
                .unwrap(),
            ),
            Some(call_id),
        )
    }

    #[tokio::test]
    async fn assignment_request_for_existing_traffic_binds_msc_circuit_before_voice_setup() {
        let (mut bsc, mut traffic_rx, walsh_code) = test_bsc_with_active_traffic_channel(33).await;
        let call_id = 77;
        let circuit_id = 0x0123;
        let session_id = Uuid::new_v4();

        {
            let tc = bsc.mobiles[0]
                .find_traffic_channel_by_walsh_mut(walsh_code)
                .unwrap();
            tc.voice_session_id = Some(session_id);
            tc.voice_leg_role = Some(VoiceLegRole::Callee);
        }
        let test_addr = bsc.mobiles[0].fwd_address.clone();
        bsc.a1.push_pending_assignment(
            call_id,
            PendingA1Assignment {
                fwd_address: test_addr,
                ack_msg_seq: 0,
                requested_tx_time: None,
                tx_deadline: None,
                session_id,
                leg_role: VoiceLegRole::Callee,
                bind_existing_traffic: true,
            },
        );

        bsc.handle_incoming_a1_message(assignment_request_message(call_id, circuit_id))
            .await;

        let tc = bsc.mobiles[0]
            .find_traffic_channel_by_walsh(walsh_code)
            .unwrap();
        assert_eq!(tc.msc_circuit_id, Some(circuit_id));
        assert_eq!(tc.a1_call_id, Some(call_id));
        assert_eq!(tc.service_option, SERVICE_OPTION_HIGH_RATE_PACKET_DATA);
        assert_eq!(tc.voice_service_option, Some(SERVICE_OPTION_EVRC_A));
        assert!(tc.is_waiting_service_response());

        let event = traffic_rx
            .try_recv()
            .expect("Assignment Request should start Service Request on existing F-TCH");
        let cfg = event
            .service_request
            .and_then(|request| request.service_config)
            .expect("Service Request should carry voice service config");
        let service_options: Vec<u16> = cfg
            .connections
            .iter()
            .map(|connection| connection.service_option)
            .collect();
        assert_eq!(service_options, vec![SERVICE_OPTION_EVRC_A]);
    }

    #[tokio::test]
    async fn existing_traffic_paging_response_has_valid_classmark() {
        let (client, endpoint) = InProcessMscClient::pair(4);
        let (mut bsc, _traffic_rx, _walsh_code) = test_bsc_with_active_traffic_channel(33).await;
        let call_id = 88;
        let session_id = Uuid::new_v4();
        let msc: Arc<dyn crate::a1_edge::MscClient> = Arc::new(client);
        bsc.config.msc_client = msc.clone();
        bsc.a1.msc_client = msc;

        let test_addr = bsc.mobiles[0].fwd_address.clone();
        assert!(bsc.send_existing_traffic_paging_response(
            call_id,
            &test_addr,
            3,
            session_id,
            VoiceLegRole::Callee,
        ));

        let outbound = endpoint.recv_from_bsc().await.unwrap();
        assert_eq!(
            outbound.message_type(),
            cdma_ios::MessageType::PagingResponse
        );
        assert_eq!(outbound.call_id(), Some(call_id));
        let decoded = outbound.decode().unwrap();
        let response = cdma_ios::PagingResponseMessage::decode(&decoded.payload).unwrap();
        assert!(
            response.classmark_information_type_2.0.len() >= 4,
            "A1 Paging Response must carry a valid Classmark Information Type 2"
        );
        assert_eq!(
            response.service_option,
            Some(cdma_ios::ServiceOption::EVRC_A)
        );
    }

    #[tokio::test]
    async fn begin_voice_release_emits_a1_clear_request_once() {
        let (client, endpoint) = InProcessMscClient::pair(4);
        let (mut bsc, _traffic_rx, walsh_code) = test_bsc_with_active_traffic_channel(3).await;
        let msc: Arc<dyn crate::a1_edge::MscClient> = Arc::new(client);
        bsc.config.msc_client = msc.clone();
        bsc.a1.msc_client = msc;
        let call_id = 44;

        let tc = bsc.mobiles[0]
            .find_traffic_channel_by_walsh_mut(walsh_code)
            .unwrap();
        tc.a1_call_id = Some(call_id);
        tc.mark_voice_connected(false);

        let test_addr = bsc.mobiles[0].fwd_address.clone();
        bsc.begin_voice_release(&test_addr, 0b111, "test release");

        let outbound = endpoint.recv_from_bsc().await.unwrap();
        assert_eq!(outbound.message_type(), cdma_ios::MessageType::ClearRequest);
        assert_eq!(outbound.call_id(), Some(call_id));
        assert!(matches!(
            bsc.mobiles[0]
                .find_traffic_channel_by_walsh(walsh_code)
                .unwrap()
                .is_releasing(),
            true
        ));
        assert_eq!(
            bsc.mobiles[0]
                .find_traffic_channel_by_walsh(walsh_code)
                .unwrap()
                .a1_clear_state,
            super::super::A1ClearState::ClearRequestSent
        );
        assert!(
            timeout(Duration::from_millis(50), endpoint.recv_from_bsc())
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn clear_command_after_local_teardown_returns_clear_complete() {
        let (client, endpoint) = InProcessMscClient::pair(4);
        let (mut bsc, _traffic_rx, walsh_code) = test_bsc_with_active_traffic_channel(3).await;
        let msc: Arc<dyn crate::a1_edge::MscClient> = Arc::new(client);
        bsc.config.msc_client = msc.clone();
        bsc.a1.msc_client = msc;
        let call_id = 55;

        let tc = bsc.mobiles[0]
            .find_traffic_channel_by_walsh_mut(walsh_code)
            .unwrap();
        tc.a1_call_id = Some(call_id);
        tc.mark_voice_connected(false);

        let test_addr = bsc.mobiles[0].fwd_address.clone();
        bsc.begin_voice_release(&test_addr, 0b111, "test release");
        let first = endpoint.recv_from_bsc().await.unwrap();
        assert_eq!(first.message_type(), cdma_ios::MessageType::ClearRequest);

        bsc.teardown_traffic_channel(walsh_code).await;
        bsc.handle_incoming_a1_message(clear_command_message(call_id))
            .await;

        let complete = endpoint.recv_from_bsc().await.unwrap();
        assert_eq!(
            complete.message_type(),
            cdma_ios::MessageType::ClearComplete
        );
        assert_eq!(complete.call_id(), Some(call_id));
    }

    #[tokio::test]
    async fn clear_command_on_active_channel_completes_on_teardown() {
        let (client, endpoint) = InProcessMscClient::pair(4);
        let (mut bsc, _traffic_rx, walsh_code) = test_bsc_with_active_traffic_channel(3).await;
        let msc: Arc<dyn crate::a1_edge::MscClient> = Arc::new(client);
        bsc.config.msc_client = msc.clone();
        bsc.a1.msc_client = msc;
        let call_id = 66;

        let tc = bsc.mobiles[0]
            .find_traffic_channel_by_walsh_mut(walsh_code)
            .unwrap();
        tc.a1_call_id = Some(call_id);
        tc.mark_voice_connected(false);

        bsc.handle_incoming_a1_message(clear_command_message(call_id))
            .await;

        assert!(matches!(
            bsc.mobiles[0]
                .find_traffic_channel_by_walsh(walsh_code)
                .unwrap()
                .is_releasing(),
            true
        ));
        assert_eq!(
            bsc.mobiles[0]
                .find_traffic_channel_by_walsh(walsh_code)
                .unwrap()
                .a1_clear_state,
            super::super::A1ClearState::ClearCommandReceived
        );

        bsc.teardown_traffic_channel(walsh_code).await;
        let complete = endpoint.recv_from_bsc().await.unwrap();
        assert_eq!(
            complete.message_type(),
            cdma_ios::MessageType::ClearComplete
        );
        assert_eq!(complete.call_id(), Some(call_id));
    }
}
