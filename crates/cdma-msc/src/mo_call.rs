//! Mobile-originated call handling for the MSC runtime.
//!
//! Owns per-call MO calling party number resolution and mobile-to-mobile
//! paging for on-net MO calls.

use std::collections::HashMap;

use log::{info, warn};

use cdma_ios::EncodedA1Message;

use crate::runtime::select_pageable_imsi;

use crate::call_control::CallId;
use crate::circuit::CircuitService;
use crate::runtime::MscA1Endpoint;

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

/// Manages MO call state (calling party number cache, M2M paging).
pub(crate) struct MoCallService {
    /// MO calling party number resolved from the originating mobile identity.
    pub(crate) mo_calling_numbers: HashMap<CallId, String>,
}

impl MoCallService {
    pub(crate) fn new() -> Self {
        Self {
            mo_calling_numbers: HashMap::new(),
        }
    }

    pub(crate) async fn resolve_mo_calling_number(
        &self,
        request: Option<&cdma_ios::CmServiceRequestMessage>,
        hlr_repo: &dyn cdma_hlr::repository::HlrRepository,
    ) -> Option<String> {
        let request = request?;
        let imsi = match &request.mobile_identity_imsi {
            cdma_ios::MobileIdentity::Imsi(imsi) => Some(imsi.as_str()),
            _ => None,
        };
        let esn = match request.mobile_identity_esn {
            Some(cdma_ios::MobileIdentity::Esn(esn)) => Some(esn),
            _ => None,
        };

        match hlr_repo.resolve_by_identity(esn, imsi).await {
            Ok(Some(subscriber)) => Some(subscriber.phone_number),
            Ok(None) => None,
            Err(error) => {
                warn!("MSC: HLR originator lookup failed for MO call: {}", error);
                None
            }
        }
    }

    pub(crate) async fn send_mo_mobile_to_mobile_page(
        &mut self,
        a1: &dyn MscA1Endpoint,
        call_id: CallId,
        called_number: &str,
        service_option: u16,
        hlr_repo: &dyn cdma_hlr::repository::HlrRepository,
        circuits: &mut CircuitService,
    ) -> MoSubscriberRoute {
        let subscriber = match hlr_repo.get_subscriber_by_phone_number(called_number).await {
            Ok(Some(subscriber)) => subscriber,
            Ok(None) => return MoSubscriberRoute::NotSubscriber,
            Err(error) => {
                warn!(
                    "MSC: HLR lookup failed for MO called_number='{}': {}",
                    called_number, error
                );
                return MoSubscriberRoute::NotSubscriber;
            }
        };

        if !matches!(subscriber.status, cdma_hlr::model::SubscriberStatus::Active) {
            warn!(
                "MSC: refusing MO M2M call_id={} to inactive subscriber {}",
                call_id.0, subscriber.subscriber_id
            );
            return MoSubscriberRoute::Rejected;
        }

        let binding = match hlr_repo
            .get_registration_binding(subscriber.subscriber_id)
            .await
        {
            Ok(Some(binding)) => binding,
            Ok(None) => {
                warn!(
                    "MSC: refusing MO M2M call_id={} to unregistered subscriber {}",
                    call_id.0, subscriber.subscriber_id
                );
                return MoSubscriberRoute::Rejected;
            }
            Err(error) => {
                warn!(
                    "MSC: HLR binding lookup failed for MO M2M call_id={} subscriber={}: {}",
                    call_id.0, subscriber.subscriber_id, error
                );
                return MoSubscriberRoute::Rejected;
            }
        };
        if !matches!(
            binding.state,
            cdma_hlr::model::RegistrationState::Registered
                | cdma_hlr::model::RegistrationState::PageResponseReceived
        ) {
            warn!(
                "MSC: refusing MO M2M call_id={} to subscriber {} in state {}",
                call_id.0,
                subscriber.subscriber_id,
                binding.state.as_str()
            );
            return MoSubscriberRoute::Rejected;
        }

        let identities = match hlr_repo
            .get_identities_for_subscriber(subscriber.subscriber_id)
            .await
        {
            Ok(identities) => identities,
            Err(error) => {
                warn!(
                    "MSC: HLR identity lookup failed for MO M2M call_id={} subscriber={}: {}",
                    call_id.0, subscriber.subscriber_id, error
                );
                return MoSubscriberRoute::Rejected;
            }
        };
        let Some(imsi): Option<&str> = select_pageable_imsi(&identities, &binding) else {
            warn!(
                "MSC: refusing MO M2M call_id={} to subscriber {} with no IMSI",
                call_id.0, subscriber.subscriber_id
            );
            return MoSubscriberRoute::Rejected;
        };

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
        info!(
            "MSC: A1 tx PagingRequest for MO M2M call_id={} subscriber={} called_number='{}'",
            call_id.0, subscriber.subscriber_id, called_number
        );
        let payload = match paging_request.encode() {
            Ok(payload) => payload,
            Err(error) => {
                warn!(
                    "MSC: failed to encode MO M2M Paging Request call_id={}: {}",
                    call_id.0, error
                );
                circuits.paging_requests.remove(&call_id);
                return MoSubscriberRoute::Rejected;
            }
        };
        if let Err(error) = a1
            .send_to_bsc(EncodedA1Message::from_message_for_call(
                &cdma_ios::Message::new(cdma_ios::MessageType::PagingRequest, payload),
                Some(call_id.0),
            ))
            .await
        {
            warn!(
                "MSC: failed to send MO M2M Paging Request call_id={}: {}",
                call_id.0, error
            );
            circuits.paging_requests.remove(&call_id);
            return MoSubscriberRoute::Rejected;
        }
        MoSubscriberRoute::Paged
    }

    /// Clean up MO state associated with a call.
    pub(crate) fn cleanup_call(&mut self, call_id: CallId) {
        self.mo_calling_numbers.remove(&call_id);
    }
}
