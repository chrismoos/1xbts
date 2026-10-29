//! MEID discovery through Status Request and SCM capability signaling (C.S0072-0 §2.1.2–2.1.3).

use std::time::{Duration, Instant};

use cdma_common::access::status_records::{INFO_RECORD_MEID, decode_meid_response};
use cdma_common::events::{AccessCellId, AccessChannelEvent};
use cdma_common::lac::message_types::MessageId;
use cdma_common::lac::paging_messages::{
    MsAddress, PagingChannelMessage, StatusQualificationInfo, StatusRequestMessage,
};
use cdma_hlr::model::MobileIdentityKey;
use log::{info, warn};

use super::Bsc;

/// C.S0072-0 retrofits MEID into Release 0 (P_REV 6) and later mobiles.
pub(crate) const FIRST_MEID_COMPATIBLE_P_REV: u8 = 6;
/// C.S0005-E §2.3.2 requires MEID for MOB_P_REV 11 and later.
const FIRST_MANDATORY_MEID_P_REV: u8 = 11;
/// C.S0072-0 §2.1.2 assigns SCM bit 4 to MEID support.
const SCM_MEID_SUPPORTED: u8 = 0b0001_0000;
const MEID_QUERY_RETRY_INTERVAL: Duration = Duration::from_secs(60);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MeidQueryChannel {
    Paging,
    Traffic { cell: AccessCellId, walsh_code: u8 },
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct PendingMeidQuery {
    pub sent_at: Instant,
    pub channel: MeidQueryChannel,
}

pub(crate) fn supports_meid(mob_p_rev: u8, scm: Option<u8>) -> bool {
    mob_p_rev >= FIRST_MANDATORY_MEID_P_REV
        || (mob_p_rev >= FIRST_MEID_COMPATIBLE_P_REV
            && scm.is_some_and(|scm| scm & SCM_MEID_SUPPORTED != 0))
}

pub(crate) fn can_discover_meid(mob_p_rev: u8, scm: Option<u8>) -> bool {
    supports_meid(mob_p_rev, scm) || (mob_p_rev >= FIRST_MEID_COMPATIBLE_P_REV && scm.is_none())
}

impl Bsc {
    pub(crate) fn request_meid(&mut self, addr: &MsAddress, channel: MeidQueryChannel) {
        let Some(ms) = self.mobiles.get(addr) else {
            return;
        };
        if ms.meid.is_some()
            || !(supports_meid(ms.mob_p_rev, ms.scm)
                || (self.access_identity.is_pending(addr, ms.serving_cell)
                    && can_discover_meid(ms.mob_p_rev, ms.scm)))
            || ms.meid_query.is_some_and(|query| {
                query.channel == channel && query.sent_at.elapsed() < MEID_QUERY_RETRY_INTERVAL
            })
        {
            return;
        }
        let request = StatusRequestMessage {
            qual_info: StatusQualificationInfo::None,
            record_types: vec![INFO_RECORD_MEID],
        };
        let result = match channel {
            MeidQueryChannel::Paging => self
                .access_tx
                .send_directed_fpch(
                    ms.serving_cell,
                    addr,
                    MessageId::StatusRequest,
                    PagingChannelMessage::StatusRequest(request.clone()),
                    request.to_sdu(),
                    true,
                )
                .map(|_| ()),
            MeidQueryChannel::Traffic { cell, walsh_code } => self
                .send_traffic_signaling(
                    cell,
                    walsh_code,
                    request.to_ftch_sdu(),
                    MessageId::StatusRequest,
                    super::DEFAULT_TRAFFIC_ACK_SEQ,
                    true,
                    None,
                    None,
                    None,
                    None,
                    None,
                )
                .map(|_| ()),
        };
        match result {
            Ok(()) => {
                self.mobiles.update(addr, |ms| {
                    ms.meid_query = Some(PendingMeidQuery {
                        sent_at: Instant::now(),
                        channel,
                    })
                });
                info!(
                    "BSC: requested MEID for {}",
                    crate::addressing::format_ms_address(addr)
                );
            }
            Err(error) => warn!("BSC: failed to request MEID: {error}"),
        }
    }

    pub(crate) fn handle_meid_response(&mut self, addr: &MsAddress, event: &AccessChannelEvent) {
        let Some(message) = event.decoded_l3.as_ref() else {
            return;
        };
        let meid = match decode_meid_response(message) {
            Ok(Some(meid)) => meid,
            Ok(None) => return,
            Err(error) => {
                warn!("BSC: invalid MEID status response: {error}");
                return;
            }
        };
        let Some(ms) = self.mobiles.get(addr) else {
            return;
        };
        if ms
            .meid_query
            .is_none_or(|query| query.sent_at.elapsed() >= MEID_QUERY_RETRY_INTERVAL)
            || event.esn.is_some_and(|esn| ms.esn != Some(esn))
            || ms.serving_cell != event.cell
            || !(supports_meid(ms.mob_p_rev, ms.scm)
                || (self.access_identity.is_pending(addr, ms.serving_cell)
                    && can_discover_meid(ms.mob_p_rev, ms.scm)))
        {
            return;
        }
        let imsi = ms.imsi.clone();
        let esn = ms.esn;
        let mob_p_rev = ms.mob_p_rev;
        self.mobiles.update(addr, |ms| {
            ms.meid = Some(meid.clone());
            ms.meid_query = None;
        });
        info!(
            "BSC: learned MEID={} for {}",
            meid,
            crate::addressing::format_ms_address(addr)
        );
        self.publish_mobiles();
        if self.access_identity.is_pending(addr, event.cell) {
            self.resume_access_identity_lookup(addr, event.cell);
        } else {
            self.resolve_subscriber_from_hlr(event, addr);
        }
        let Some(hlr) = self.config.hlr_repo.clone() else {
            return;
        };
        let Ok(identity) = MobileIdentityKey::from_parts(imsi.as_deref(), esn, Some(&meid)) else {
            return;
        };
        tokio::spawn(async move {
            if let Err(error) = hlr.upsert_mobile_seen(&identity, Some(mob_p_rev)).await {
                warn!("BSC: failed to save discovered MEID: {error}");
            }
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn meid_capability_requires_revision_and_scm_support() {
        assert!(can_discover_meid(FIRST_MEID_COMPATIBLE_P_REV, None));
        assert!(!can_discover_meid(FIRST_MEID_COMPATIBLE_P_REV, Some(0)));
        assert!(!can_discover_meid(FIRST_MEID_COMPATIBLE_P_REV - 1, None));
        for revision in 0..FIRST_MEID_COMPATIBLE_P_REV {
            assert!(!supports_meid(revision, Some(u8::MAX)));
        }
        for revision in FIRST_MEID_COMPATIBLE_P_REV..FIRST_MANDATORY_MEID_P_REV {
            assert!(!supports_meid(revision, None));
            assert!(!supports_meid(revision, Some(0)));
            assert!(supports_meid(revision, Some(SCM_MEID_SUPPORTED)));
        }
        for revision in FIRST_MANDATORY_MEID_P_REV..=u8::MAX {
            assert!(supports_meid(revision, None));
        }
    }
}
