//! Mobile-originated call handling for the MSC runtime.
//!
//! Resolves calling and called subscribers and retains pending SIP routes.

use std::collections::HashMap;

use cdma_common::consts::{SERVICE_OPTION_EVRC_A, SERVICE_OPTION_QCELP13};
use log::warn;

use crate::call_control::CallId;

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

    pub(crate) async fn resolve_mo_destination(
        &self,
        called_number: &str,
        hlr_repo: &dyn cdma_hlr::repository::HlrRepository,
    ) -> Option<cdma_hlr::model::ResolvedSubscriber> {
        match hlr_repo.get_subscriber_by_phone_number(called_number).await {
            Ok(resolved) => resolved,
            Err(error) => {
                warn!(
                    "MSC: HLR lookup failed for MO called_number='{}': {}",
                    called_number, error
                );
                None
            }
        }
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
