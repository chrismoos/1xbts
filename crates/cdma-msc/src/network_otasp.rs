//! Network-initiated provisioning uses SO18 and OTAPA Start (C.S0016-D §3.2.2).

use uuid::Uuid;

use crate::base_station::{BaseStationId, MscA1Endpoint};
use crate::call_control::CallDirection;
use crate::management::{InitiateCallAccepted, ManagementError, MtCallPlan};
use crate::runtime::{MscRuntime, PagePurpose};

/// C.S0072 §2.1.1.1 reserves manufacturer code 0x80 for non-unique pseudo-ESNs.
const PSEUDO_ESN_PREFIX: u32 = 0x80;
const ESN_MANUFACTURER_SHIFT: u32 = 24;

#[derive(Clone, PartialEq, Eq, Hash)]
pub(crate) enum ProvisioningIdentity {
    Meid(String),
    Esn(u32),
    Imsi(String),
}

impl ProvisioningIdentity {
    pub(crate) fn from_parts(imsi: &str, esn: Option<u32>, meid: Option<&str>) -> Self {
        if let Some(meid) = meid {
            Self::Meid(meid.to_ascii_uppercase())
        } else if let Some(esn) =
            esn.filter(|esn| esn >> ESN_MANUFACTURER_SHIFT != PSEUDO_ESN_PREFIX)
        {
            Self::Esn(esn)
        } else {
            Self::Imsi(imsi.to_string())
        }
    }

    pub(crate) fn matches(&self, imsi: &str, esn: Option<u32>, meid: Option<&str>) -> bool {
        match self {
            Self::Meid(expected) => meid.is_some_and(|meid| expected.eq_ignore_ascii_case(meid)),
            Self::Esn(expected) => esn == Some(*expected),
            Self::Imsi(expected) => imsi == expected,
        }
    }
}

#[derive(Clone)]
pub(crate) struct ProvisioningRoute {
    pub(crate) node: BaseStationId,
    pub(crate) imsi: String,
    pub(crate) meid: Option<String>,
    pub(crate) slot_cycle_index: Option<u8>,
}

impl MscRuntime {
    pub(crate) async fn initiate_otasp(
        &mut self,
        a1: &dyn MscA1Endpoint,
        subscriber_id: Uuid,
        target_imsi: Option<&str>,
    ) -> Result<InitiateCallAccepted, ManagementError> {
        if self.otasp.is_none() {
            return Err(ManagementError::Rejected("OTASP is disabled".into()));
        }
        if self.media_gw.is_subscriber_busy(subscriber_id) {
            return Err(ManagementError::Rejected(
                "Subscriber already has an active call".into(),
            ));
        }
        let resolved = self
            .config
            .hlr_repo
            .get_subscriber_by_id(subscriber_id)
            .await
            .map_err(ManagementError::Rejected)?
            .ok_or(ManagementError::UnknownSubscriber(subscriber_id))?;
        if resolved.subscriber.status != cdma_hlr::model::SubscriberStatus::Active {
            return Err(ManagementError::Rejected("Subscriber is inactive".into()));
        }
        let identity = match target_imsi {
            Some(imsi) => resolved.identities.iter().find(|identity| {
                identity.imsi.as_deref() == Some(imsi)
                    || self.otapa_routes.iter().any(|(key, route)| {
                        route.imsi == imsi
                            && *key
                                == ProvisioningIdentity::from_parts(
                                    identity.imsi.as_deref().unwrap_or_default(),
                                    identity.esn,
                                    identity.meid.as_deref(),
                                )
                    })
            }),
            None => resolved.primary_identity.as_ref(),
        }
        .ok_or_else(|| {
            ManagementError::Rejected("No matching subscriber identity is provisioned".into())
        })?;
        let target = ProvisioningIdentity::from_parts(
            identity.imsi.as_deref().unwrap_or_default(),
            identity.esn,
            identity.meid.as_deref(),
        );
        let route = self
            .otapa_routes
            .get(&target)
            .or_else(|| {
                self.otapa_routes.values().find(|route| {
                    matches!(&target, ProvisioningIdentity::Imsi(imsi) if *imsi == route.imsi)
                })
            })
            .cloned()
            .or_else(|| {
                resolved
                    .binding
                    .as_ref()
                    .filter(|binding| {
                        target.matches(
                            binding.imsi.as_deref().unwrap_or_default(),
                            binding.esn,
                            binding.meid.as_deref(),
                        )
                    })
                    .and_then(|binding| {
                        Some(ProvisioningRoute {
                            node: BaseStationId(binding.serving_bs_id.clone()),
                            imsi: binding.imsi.clone()?,
                            meid: binding.meid.clone(),
                            slot_cycle_index: binding
                                .slot_cycle_index
                                .and_then(|value| u8::try_from(value).ok()),
                        })
                    })
            })
            .ok_or_else(|| {
                ManagementError::Rejected(
                    "No observed paging identity is known for this handset".into(),
                )
            })?;
        let imsi = route.imsi.clone();
        if !a1.is_attached(&route.node) {
            return Err(ManagementError::Rejected(
                "Serving base station is not attached".into(),
            ));
        }
        let call_id = self.controller.create_call(
            CallDirection::MobileTerminated,
            Some(cdma_ios::MobileIdentity::Imsi(imsi.clone())),
        );
        a1.bind_call(call_id, &route.node);
        self.media_gw
            .register_active_subscriber(subscriber_id, call_id);
        self.pending_otapa_calls.insert(call_id, identity.clone());
        let tag = cdma_ios::Tag(call_id.0 as u32);
        let service_option = cdma_common::consts::SERVICE_OPTION_OTASP;
        let page = cdma_ios::PagingRequestMessage {
            mobile_identity_meid: route
                .meid
                .as_deref()
                .and_then(cdma_common::access::status_records::parse_meid)
                .map(cdma_ios::MobileIdentity::Meid),
            mobile_identity_imsi: cdma_ios::MobileIdentity::Imsi(imsi.clone()),
            tag: Some(tag),
            cell_identifier_list: None,
            slot_cycle_index: route.slot_cycle_index.map(cdma_ios::SlotCycleIndex),
            service_option: Some(cdma_ios::ServiceOption(service_option)),
            is2000_mobile_capabilities: None,
        };
        if let Err(error) = self.controller.apply_from_msc(
            call_id,
            &cdma_ios::ProcedureMessage::PagingRequest(page.clone()),
        ) {
            self.abort_mt_setup(a1, call_id);
            return Err(ManagementError::Rejected(error.to_string()));
        }
        self.mt_call.mt_plans.insert(
            tag.0,
            MtCallPlan {
                subscriber_id,
                imsi,
                audio_file: None,
                caller_number: None,
                service_option,
            },
        );
        self.circuits.paging_requests.insert(call_id, page.clone());
        if !self
            .send_paging_request_to_bsc(a1, call_id, page, PagePurpose::Initial)
            .await
        {
            self.abort_mt_setup(a1, call_id);
            return Err(ManagementError::Unavailable(
                "Provisioning page could not be sent",
            ));
        }
        log::info!(
            "MSC: network-initiated OTASP paging subscriber_id={} call_id={}",
            subscriber_id,
            call_id.0
        );
        Ok(InitiateCallAccepted { call_id })
    }
}
