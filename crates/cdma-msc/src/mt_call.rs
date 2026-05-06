//! Mobile-terminated call handling for the MSC runtime.
//!
//! Owns MT call plans and the assignment-request flow triggered by paging
//! responses.

use std::collections::HashMap;
use std::sync::Arc;

use log::{info, warn};

use cdma_ios::{EncodedA1Message, VoiceBearerManager};

use crate::call_control::{CallId, MscCallController};
use crate::circuit::{CircuitService, CircuitSession, MscVoiceLeg};
use crate::management::MtCallPlan;
use crate::mo_call::MoCallService;
use crate::runtime::MscA1Endpoint;

/// Manages MT call plans and paging-response assignment flows.
pub(crate) struct MtCallService {
    /// Locally-staged MT call plans, keyed by A1 Tag.
    pub(crate) mt_plans: HashMap<u32, MtCallPlan>,
}

impl MtCallService {
    pub(crate) fn new() -> Self {
        Self {
            mt_plans: HashMap::new(),
        }
    }

    pub(crate) async fn send_assignment_request_for_paging_response(
        &mut self,
        a1: &dyn MscA1Endpoint,
        call_id: CallId,
        response: cdma_ios::PagingResponseMessage,
        secondary_leg: bool,
        controller: &mut MscCallController,
        circuits: &mut CircuitService,
        mo_call: &MoCallService,
        voice_bearer: Option<&Arc<VoiceBearerManager>>,
        default_voice_service_option: u16,
    ) {
        let cic = circuits.assignment_circuit_identity_code_for_next_leg(call_id);
        let tag_val = response.tag.map(|t| t.0).unwrap_or(call_id.0 as u32);
        let mt_plan = if secondary_leg {
            None
        } else {
            self.mt_plans.remove(&tag_val)
        };
        let caller_number = if secondary_leg {
            mo_call.mo_calling_numbers.get(&call_id).cloned()
        } else {
            mt_plan.as_ref().and_then(|p| p.caller_number.clone())
        };
        let audio_file = mt_plan.as_ref().and_then(|p| p.audio_file.clone());
        let service_option = mt_plan
            .as_ref()
            .map(|p| p.service_option)
            .or(response.service_option.map(|so| so.0))
            .unwrap_or(default_voice_service_option);

        let circuit_id = cic.to_packed();
        circuits.insert_circuit_session(
            circuit_id,
            CircuitSession {
                call_id,
                audio_file,
                service_option,
                leg_role: if secondary_leg {
                    MscVoiceLeg::Secondary
                } else {
                    MscVoiceLeg::Primary
                },
                peer_circuit_id: None,
                bearer_remote_ready: voice_bearer.is_none(),
                media_gateway_handle: controller
                    .snapshot(call_id)
                    .and_then(|snapshot| snapshot.media_gateway_handle),
            },
        );
        let leg_role = if secondary_leg {
            MscVoiceLeg::Secondary
        } else {
            MscVoiceLeg::Primary
        };
        circuits.queue_assignment_complete_circuit(call_id, leg_role, circuit_id);

        let a2p_bearer_session_params = if let Some(bearer) = voice_bearer {
            match bearer.open_circuit(circuit_id, None).await {
                Ok(local_addr) => Some(cdma_ios::A2pBearerSessionParams {
                    ip_address: match local_addr.ip() {
                        std::net::IpAddr::V4(v4) => v4,
                        _ => std::net::Ipv4Addr::UNSPECIFIED,
                    },
                    udp_port: local_addr.port(),
                }),
                Err(e) => {
                    warn!("MSC: failed to open bearer circuit {circuit_id}: {e}");
                    None
                }
            }
        } else {
            None
        };

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
        let assignment_request = cdma_ios::AssignmentRequestMessage {
            channel_type: cdma_ios::ChannelType {
                speech_or_data_indicator: 0x01,
                channel_rate_and_type: 0x08,
                coding: 0x05,
            },
            circuit_identity_code: cic,
            encryption_information: None,
            service_option: response.service_option,
            signals: Vec::new(),
            calling_party_ascii_number: caller_number
                .map(|n| cdma_ios::CallingPartyAsciiNumber(n.into_bytes())),
            ms_information_records: None,
            priority: None,
            paca_timestamp: None,
            quality_of_service_parameters: None,
            a2p_bearer_session_params,
            a2p_bearer_format_params,
        };
        if secondary_leg {
            if let Err(error) = circuits.apply_secondary_leg_from_bsc(
                call_id,
                &cdma_ios::ProcedureMessage::PagingResponse(response.clone()),
            ) {
                warn!(
                    "MSC: failed to apply secondary-leg Paging Response: {:?}",
                    error
                );
                circuits.cancel_assignment_complete_circuit(call_id, leg_role);
                return;
            }
            if let Err(error) = circuits.apply_secondary_leg_from_msc(
                call_id,
                &cdma_ios::ProcedureMessage::AssignmentRequest(assignment_request.clone()),
            ) {
                warn!(
                    "MSC: failed to apply secondary-leg Assignment Request: {:?}",
                    error
                );
                circuits.cancel_assignment_complete_circuit(call_id, leg_role);
                return;
            }
        } else if let Err(error) = controller.apply_from_msc(
            call_id,
            &cdma_ios::ProcedureMessage::AssignmentRequest(assignment_request.clone()),
        ) {
            warn!(
                "MSC: failed to apply A1 Assignment Request state: {}",
                error
            );
            circuits.cancel_assignment_complete_circuit(call_id, leg_role);
            return;
        }
        let payload = match assignment_request.encode() {
            Ok(payload) => payload,
            Err(error) => {
                warn!("MSC: failed to encode A1 Assignment Request: {}", error);
                return;
            }
        };
        info!(
            "MSC: A1 tx AssignmentRequest call_id={} circuit_id={} leg={:?}",
            call_id.0, circuit_id, circuits.circuits[&circuit_id].leg_role
        );
        if let Err(error) = a1
            .send_to_bsc(EncodedA1Message::from_message_for_call(
                &cdma_ios::Message::new(cdma_ios::MessageType::AssignmentRequest, payload),
                Some(call_id.0),
            ))
            .await
        {
            warn!(
                "MSC: failed to send A1 Assignment Request to BSC: {}",
                error
            );
        }
    }
}
