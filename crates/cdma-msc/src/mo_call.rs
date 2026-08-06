//! Mobile-originated call handling for the MSC runtime.
//!
//! Owns per-call MO calling party number resolution and mobile-to-mobile
//! paging for on-net MO calls.

use std::collections::HashMap;

use cdma_common::consts::{SERVICE_OPTION_EVRC_A, SERVICE_OPTION_QCELP13};
use log::{info, warn};

use crate::runtime::select_pageable_imsi;

use crate::call_control::CallId;
use crate::circuit::CircuitService;

const IS2000_MIN_P_REV: u32 = 6;

pub(crate) fn select_mt_voice_service_option(
    mob_p_rev: Option<u32>,
    supported_service_options: &[u16],
    default_voice_service_option: u16,
) -> u16 {
    let preferred = match mob_p_rev {
        Some(p_rev) if p_rev >= IS2000_MIN_P_REV => Some(SERVICE_OPTION_EVRC_A),
        Some(_) => Some(SERVICE_OPTION_QCELP13),
        None => None,
    };
    preferred
        .filter(|service_option| supported_service_options.contains(service_option))
        .unwrap_or(default_voice_service_option)
}

/// Routing decision for an MO call's called-party number.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MoSubscriberRoute {
    /// Called number is not a local subscriber.
    NotSubscriber,
    /// Called subscriber was paged successfully.
    Paged,
    /// Call was rejected (subscriber inactive, unregistered, etc.).
    Rejected,
}

/// Routing recorded at MO origination, held until AssignmentComplete.
#[derive(Debug, Clone)]
pub(crate) struct PendingSipRoute {
    pub called_number: String,
    pub calling_number: Option<String>,
    pub service_option: u16,
}

pub(crate) struct MoCallService {
    pub(crate) mo_calling_numbers: HashMap<CallId, String>,
    pub(crate) pending_sip_routes: HashMap<CallId, PendingSipRoute>,
}

impl MoCallService {
    pub(crate) fn new() -> Self {
        Self {
            mo_calling_numbers: HashMap::new(),
            pending_sip_routes: HashMap::new(),
        }
    }

    pub(crate) async fn resolve_mo_originator(
        &self,
        request: Option<&cdma_ios::CmServiceRequestMessage>,
        hlr_repo: &dyn cdma_hlr::repository::HlrRepository,
    ) -> Option<(String, uuid::Uuid)> {
        let request = request?;
        let imsi = match &request.mobile_identity_imsi {
            cdma_ios::MobileIdentity::Imsi(imsi) => Some(imsi.as_str()),
            _ => None,
        };
        let esn = match request.mobile_identity_esn {
            Some(cdma_ios::MobileIdentity::Esn(esn)) => Some(esn),
            _ => None,
        };
        let identity_key = cdma_hlr::model::MobileIdentityKey::from_parts(imsi, esn, None).ok()?;

        match hlr_repo.resolve_by_identity(&identity_key).await {
            Ok(Some(resolved)) => Some((
                resolved.subscriber.phone_number,
                resolved.subscriber.subscriber_id,
            )),
            Ok(None) => None,
            Err(error) => {
                warn!("MSC: HLR originator lookup failed for MO call: {}", error);
                None
            }
        }
    }

    pub(crate) async fn send_mo_mobile_to_mobile_page(
        &mut self,
        call_id: CallId,
        called_number: &str,
        supported_service_options: &[u16],
        default_voice_service_option: u16,
        hlr_repo: &dyn cdma_hlr::repository::HlrRepository,
        circuits: &mut CircuitService,
    ) -> MoSubscriberRoute {
        let resolved = match hlr_repo.get_subscriber_by_phone_number(called_number).await {
            Ok(Some(resolved)) => resolved,
            Ok(None) => return MoSubscriberRoute::NotSubscriber,
            Err(error) => {
                warn!(
                    "MSC: HLR lookup failed for MO called_number='{}': {}",
                    called_number, error
                );
                return MoSubscriberRoute::NotSubscriber;
            }
        };
        let subscriber_id = resolved.subscriber.subscriber_id;

        if !matches!(
            resolved.subscriber.status,
            cdma_hlr::model::SubscriberStatus::Active
        ) {
            warn!(
                "MSC: refusing MO M2M call_id={} to inactive subscriber {}",
                call_id.0, subscriber_id
            );
            return MoSubscriberRoute::Rejected;
        }

        let Some(binding) = resolved.binding.as_ref() else {
            warn!(
                "MSC: refusing MO M2M call_id={} to unregistered subscriber {}",
                call_id.0, subscriber_id
            );
            return MoSubscriberRoute::Rejected;
        };
        if !matches!(
            binding.state,
            cdma_hlr::model::RegistrationState::Registered
                | cdma_hlr::model::RegistrationState::PageResponseReceived
        ) {
            warn!(
                "MSC: refusing MO M2M call_id={} to subscriber {} in state {}",
                call_id.0,
                subscriber_id,
                binding.state.as_str()
            );
            return MoSubscriberRoute::Rejected;
        }

        let Some(imsi): Option<&str> = select_pageable_imsi(&resolved.identities, binding) else {
            warn!(
                "MSC: refusing MO M2M call_id={} to subscriber {} with no IMSI",
                call_id.0, subscriber_id
            );
            return MoSubscriberRoute::Rejected;
        };
        let service_option = select_mt_voice_service_option(
            binding.mob_p_rev,
            supported_service_options,
            default_voice_service_option,
        );

        let paging_request = cdma_ios::PagingRequestMessage {
            mobile_identity_imsi: cdma_ios::MobileIdentity::Imsi(imsi.to_string()),
            tag: Some(cdma_ios::Tag(call_id.0 as u32)),
            cell_identifier_list: None,
            slot_cycle_index: binding
                .slot_cycle_index
                .map(|value| cdma_ios::SlotCycleIndex(value as u8)),
            service_option: Some(cdma_ios::ServiceOption(service_option)),
            is2000_mobile_capabilities: None,
        };
        circuits
            .paging_requests
            .insert(call_id, paging_request.clone());
        circuits
            .deferred_paging_requests
            .insert(call_id, paging_request);
        info!(
            "MSC: deferring MO M2M PagingRequest call_id={} subscriber={} called_number='{}' callee_p_rev={:?} SO{} until primary leg AssignmentComplete",
            call_id.0, subscriber_id, called_number, binding.mob_p_rev, service_option
        );
        MoSubscriberRoute::Paged
    }

    /// Clean up MO state associated with a call.
    pub(crate) fn cleanup_call(&mut self, call_id: CallId) {
        self.mo_calling_numbers.remove(&call_id);
        self.pending_sip_routes.remove(&call_id);
    }
}

#[cfg(test)]
mod tests {
    use super::select_mt_voice_service_option;
    use cdma_common::consts::{SERVICE_OPTION_EVRC_A, SERVICE_OPTION_QCELP13};

    #[test]
    fn mt_voice_service_option_follows_registered_protocol_revision() {
        let supported = [SERVICE_OPTION_QCELP13, SERVICE_OPTION_EVRC_A];
        assert_eq!(
            select_mt_voice_service_option(Some(3), &supported, SERVICE_OPTION_QCELP13),
            SERVICE_OPTION_QCELP13
        );
        assert_eq!(
            select_mt_voice_service_option(Some(5), &supported, SERVICE_OPTION_QCELP13),
            SERVICE_OPTION_QCELP13
        );
        assert_eq!(
            select_mt_voice_service_option(Some(6), &supported, SERVICE_OPTION_QCELP13),
            SERVICE_OPTION_EVRC_A
        );
        assert_eq!(
            select_mt_voice_service_option(Some(9), &supported, SERVICE_OPTION_QCELP13),
            SERVICE_OPTION_EVRC_A
        );
        assert_eq!(
            select_mt_voice_service_option(None, &supported, SERVICE_OPTION_QCELP13),
            SERVICE_OPTION_QCELP13
        );
    }

    #[test]
    fn mt_voice_service_option_never_bypasses_policy() {
        assert_eq!(
            select_mt_voice_service_option(
                Some(3),
                &[SERVICE_OPTION_EVRC_A],
                SERVICE_OPTION_EVRC_A,
            ),
            SERVICE_OPTION_EVRC_A
        );
        assert_eq!(
            select_mt_voice_service_option(
                Some(9),
                &[SERVICE_OPTION_QCELP13],
                SERVICE_OPTION_QCELP13,
            ),
            SERVICE_OPTION_QCELP13
        );
    }
}
