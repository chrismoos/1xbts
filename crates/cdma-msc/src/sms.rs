use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use log::{info, warn};
use uuid::Uuid;

use cdma_hlr::repository::HlrRepository;
use cdma_ios::{
    AddsDeliverAckMessage, AddsDeliverMessage, AddsPageAckMessage, AddsPageMessage,
    AddsTransferMessage, AddsUserPart, MobileIdentity, Tag,
};
use cdma_smsc::model::MoSmsFingerprint;
use cdma_smsc::repository::SmscRepository;

use crate::base_station::BaseStationId;
use crate::runtime::MscA1Endpoint;

fn encode_delivery_payload(
    command: &cdma_smsc::proto::DeliveryCommand,
    message_id: u16,
) -> Vec<u8> {
    use cdma_common::sms::{
        TELESERVICE_WMT, UserData, encode_sms_deliver, encode_sms_deliver_typed,
    };

    let teleservice = command
        .teleservice_id
        .and_then(|id| u16::try_from(id).ok())
        .unwrap_or(TELESERVICE_WMT);
    if teleservice == TELESERVICE_WMT && command.raw_user_data.is_none() {
        return encode_sms_deliver(&command.originating_number, &command.text, message_id);
    }
    let user_data = match command.raw_user_data.as_deref() {
        Some(bytes) => UserData::Octet(bytes.to_vec()),
        None => UserData::Ascii7(command.text.clone()),
    };
    encode_sms_deliver_typed(
        &command.originating_number,
        teleservice,
        &user_data,
        message_id,
    )
}

use cdma_smsc::proto::delivery_outcome::Result as OutcomeResult;

struct SmsCorrelation {
    sms_id: Uuid,
    attempt_id: Uuid,
    sent_at: Instant,
}

pub(crate) struct SmsTrafficDelivery {
    pub sms_id: Uuid,
    pub attempt_id: Uuid,
    pub mobile_identity: MobileIdentity,
    pub serving_node: BaseStationId,
    pub adds_page: Vec<u8>,
    pub tag: u32,
}

pub(crate) struct MscSmsCoordinator {
    smsc: Arc<dyn SmscRepository>,
    hlr: Arc<dyn HlrRepository>,
    access_mode: crate::admission::AccessMode,
    next_tag: u32,
    pending: HashMap<u32, SmsCorrelation>,
    outcomes: Option<tokio::sync::mpsc::Sender<cdma_smsc::proto::DeliveryOutcome>>,
    mo_submitter: Option<Arc<dyn cdma_smsc::client::MoSubmitter>>,
    traffic_deliveries: Vec<SmsTrafficDelivery>,
    serving_nodes: HashMap<String, BaseStationId>,
}

#[derive(Debug, Clone)]
pub enum SmsDestinationKey {
    PhoneNumber(String),
    Imsi(String),
}

impl std::fmt::Display for SmsDestinationKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            SmsDestinationKey::PhoneNumber(num) => write!(f, "phone={num}"),
            SmsDestinationKey::Imsi(imsi) => write!(f, "imsi={imsi}"),
        }
    }
}

/// Inputs to a management-plane SMS send request.
#[derive(Debug)]
pub struct SmsSendRequest {
    pub originating_number: String,
    pub text: String,
    pub destination: SmsDestinationKey,
    pub timeout_ms: u64,
    /// C.S0015-B teleservice ID. `None` selects WMT (0x1002).
    pub teleservice_id: Option<u16>,
    /// Opaque User Data bytes for non-text teleservices. When set, the BSC
    /// emits these verbatim as the bearer-data User Data sub-parameter
    /// (MSG_ENCODING=0x00 octet) instead of encoding `text`. Used to carry
    /// WAP Push PDUs end to end.
    pub raw_user_data: Option<Vec<u8>>,
    /// The base station to page through when the caller already knows it,
    /// as the registration path does. Otherwise the HLR binding decides.
    pub serving_node: Option<BaseStationId>,
}

impl MscSmsCoordinator {
    pub(crate) fn new(
        smsc: Arc<dyn SmscRepository>,
        hlr: Arc<dyn HlrRepository>,
        access_mode: crate::admission::AccessMode,
    ) -> Self {
        Self {
            smsc,
            hlr,
            access_mode,
            next_tag: 1,
            pending: HashMap::new(),
            outcomes: None,
            mo_submitter: None,
            traffic_deliveries: Vec::new(),
            serving_nodes: HashMap::new(),
        }
    }

    pub(crate) fn record_mobile_contact(&mut self, imsi: &str, node: &BaseStationId) {
        self.serving_nodes.insert(imsi.to_string(), node.clone());
    }

    pub(crate) fn remove_serving_node(&mut self, node: &BaseStationId) {
        self.serving_nodes.retain(|_, serving| serving != node);
    }

    fn delivery_node(
        &self,
        identity: &MobileIdentity,
        supplied: Option<&str>,
    ) -> Option<BaseStationId> {
        if let Some(node) = supplied {
            return Some(BaseStationId(node.to_string()));
        }
        let MobileIdentity::Imsi(imsi) = identity else {
            return None;
        };
        self.serving_nodes.get(imsi).cloned()
    }

    pub(crate) fn set_mo_submitter(&mut self, submitter: Arc<dyn cdma_smsc::client::MoSubmitter>) {
        self.mo_submitter = Some(submitter);
    }

    pub(crate) fn set_outcome_reporter(
        &mut self,
        outcomes: tokio::sync::mpsc::Sender<cdma_smsc::proto::DeliveryOutcome>,
    ) {
        self.outcomes = Some(outcomes);
    }

    async fn report(
        &self,
        sms_id: Uuid,
        attempt_id: Uuid,
        result: OutcomeResult,
        detail: Option<String>,
    ) {
        let Some(outcomes) = self.outcomes.as_ref() else {
            return;
        };
        let outcome = cdma_smsc::proto::DeliveryOutcome {
            sms_id: sms_id.to_string(),
            attempt_id: attempt_id.to_string(),
            result: result as i32,
            detail,
        };
        if outcomes.send(outcome).await.is_err() {
            warn!("MSC SMS: could not report the outcome for {sms_id}");
        }
    }

    pub(crate) async fn destination_admitted(&self, destination: &SmsDestinationKey) -> bool {
        if self.access_mode == crate::admission::AccessMode::Open {
            return true;
        }
        let resolved = match destination {
            SmsDestinationKey::PhoneNumber(number) => {
                self.hlr.get_subscriber_by_phone_number(number).await
            }
            SmsDestinationKey::Imsi(imsi) => {
                let Some(node) = self.serving_nodes.get(imsi) else {
                    return false;
                };
                self.hlr
                    .resolve_registered_imsi(imsi, &node.0)
                    .await
                    .map_err(|error| error.to_string())
            }
        };
        Self::mt_subscriber_admitted(resolved)
    }

    fn mt_subscriber_admitted(
        resolved: Result<Option<cdma_hlr::model::ResolvedSubscriber>, String>,
    ) -> bool {
        match resolved {
            Ok(Some(resolved)) => matches!(
                resolved.subscriber.status,
                cdma_hlr::model::SubscriberStatus::Active
                    | cdma_hlr::model::SubscriberStatus::Suspended
            ),
            Ok(None) => false,
            Err(error) => {
                warn!("MSC SMS: recipient admission lookup failed: {error}");
                false
            }
        }
    }

    async fn delivery_admitted(&self, identity: &MobileIdentity, node: &BaseStationId) -> bool {
        if self.access_mode == crate::admission::AccessMode::Open {
            return true;
        }
        let resolved = match identity {
            MobileIdentity::Imsi(imsi) => self
                .hlr
                .resolve_registered_imsi(imsi, &node.0)
                .await
                .map_err(|error| error.to_string()),
            MobileIdentity::Esn(esn) => self
                .hlr
                .resolve_by_hardware_identity(Some(*esn), None)
                .await
                .map_err(|error| error.to_string())
                .map(|resolved| {
                    resolved.filter(|resolved| {
                        resolved.binding.as_ref().is_some_and(|binding| {
                            binding.serving_bs_id == node.0
                                && binding.esn == Some(*esn)
                                && binding.imsi.as_ref().is_some_and(|imsi| {
                                    resolved
                                        .identities
                                        .iter()
                                        .any(|identity| identity.imsi.as_ref() == Some(imsi))
                                })
                        })
                    })
                }),
            MobileIdentity::Meid(_) => return false,
        };
        Self::mt_subscriber_admitted(resolved)
    }

    pub(crate) async fn deliver_from_smsc(
        &mut self,
        command: cdma_smsc::proto::DeliveryCommand,
        a1: &dyn MscA1Endpoint,
    ) {
        let Ok(sms_id) = Uuid::parse_str(&command.sms_id) else {
            warn!("MSC SMS: the SMSC asked to deliver {} ", command.sms_id);
            return;
        };
        let Ok(attempt_id) = Uuid::parse_str(&command.attempt_id) else {
            warn!("MSC SMS: delivery {sms_id} has no valid attempt ID");
            return;
        };
        let Some(target) = command.target.as_ref() else {
            self.report(
                sms_id,
                attempt_id,
                OutcomeResult::Failed,
                Some("no target".into()),
            )
            .await;
            return;
        };
        let mobile_identity = match target.identity.as_ref() {
            Some(cdma_smsc::proto::delivery_target::Identity::Imsi(imsi)) => {
                MobileIdentity::Imsi(imsi.clone())
            }
            Some(cdma_smsc::proto::delivery_target::Identity::Esn(esn)) => {
                match u32::try_from(*esn) {
                    Ok(esn) => MobileIdentity::Esn(esn),
                    Err(_) => {
                        self.report(
                            sms_id,
                            attempt_id,
                            OutcomeResult::Failed,
                            Some("ESN too large".into()),
                        )
                        .await;
                        return;
                    }
                }
            }
            None => {
                self.report(
                    sms_id,
                    attempt_id,
                    OutcomeResult::Failed,
                    Some("no identity".into()),
                )
                .await;
                return;
            }
        };
        let Some(node) = self
            .delivery_node(&mobile_identity, target.serving_node.as_deref())
            .filter(|node| a1.is_attached(node))
        else {
            self.report(
                sms_id,
                attempt_id,
                OutcomeResult::NotReachable,
                Some("no attached serving node".into()),
            )
            .await;
            return;
        };

        if !self.delivery_admitted(&mobile_identity, &node).await {
            warn!("MSC SMS: delivery {sms_id} refused by closed access policy");
            self.report(
                sms_id,
                attempt_id,
                OutcomeResult::Failed,
                Some("recipient access denied".into()),
            )
            .await;
            return;
        }

        let message_id = self.alloc_tag() as u16;
        let tag_value = self.alloc_tag();
        let payload = encode_delivery_payload(&command, message_id);
        let adds_page = AddsPageMessage {
            mobile_identity: mobile_identity.clone(),
            adds_user_part: AddsUserPart {
                burst_type: 0x03, // SMS
                data: payload,
            },
            tag: Some(Tag(tag_value)),
            slot_cycle_index: None,
        };
        let page_payload = match adds_page.encode() {
            Ok(payload) => payload,
            Err(e) => {
                self.report(
                    sms_id,
                    attempt_id,
                    OutcomeResult::Failed,
                    Some(format!("payload too large for A1 ADDS Page: {e}")),
                )
                .await;
                return;
            }
        };
        if command.raw_user_data.is_none() {
            let encoded = cdma_ios::EncodedA1Message::from_message(&cdma_ios::Message::new(
                cdma_ios::MessageType::AddsPage,
                page_payload,
            ));
            if let Err(e) = a1.send_to_node(&node, encoded).await {
                self.report(
                    sms_id,
                    attempt_id,
                    OutcomeResult::NotReachable,
                    Some(format!("send to {node} failed: {e}")),
                )
                .await;
                return;
            }
            self.pending.insert(
                tag_value,
                SmsCorrelation {
                    sms_id,
                    attempt_id,
                    sent_at: Instant::now(),
                },
            );
            info!("MSC SMS: sent ADDS Page for sms_id={sms_id} tag={tag_value} on request");
        } else {
            self.traffic_deliveries.push(SmsTrafficDelivery {
                sms_id,
                attempt_id,
                mobile_identity,
                serving_node: node,
                adds_page: page_payload,
                tag: tag_value,
            });
            info!(
                "MSC SMS: staged traffic-channel SMS delivery for sms_id={sms_id} tag={tag_value}"
            );
        }
    }

    pub(crate) fn take_traffic_deliveries(&mut self) -> Vec<SmsTrafficDelivery> {
        std::mem::take(&mut self.traffic_deliveries)
    }

    pub(crate) fn begin_pending(&mut self, tag: u32, sms_id: Uuid, attempt_id: Uuid) {
        self.pending.insert(
            tag,
            SmsCorrelation {
                sms_id,
                attempt_id,
                sent_at: Instant::now(),
            },
        );
    }

    pub(crate) async fn send_sms(&mut self, req: SmsSendRequest) -> Option<Uuid> {
        if !self.destination_admitted(&req.destination).await {
            warn!(
                "MSC SMS: submission to {} refused by closed access policy",
                req.destination
            );
            return None;
        }
        let submitter = self.mo_submitter.as_ref()?;
        let message = cdma_smsc::client::MtSubmission {
            originating_number: req.originating_number.clone(),
            destination: match &req.destination {
                SmsDestinationKey::PhoneNumber(number) => {
                    cdma_smsc::model::SmsDestination::PhoneNumber(number.clone())
                }
                SmsDestinationKey::Imsi(imsi) => {
                    cdma_smsc::model::SmsDestination::Imsi(imsi.clone())
                }
            },
            text: req.text.clone(),
            teleservice_id: req.teleservice_id,
            raw_user_data: req.raw_user_data.clone(),
            destination_subscriber_id: None,
            serving_node: req.serving_node.as_ref().map(|node| node.0.clone()),
        };
        match submitter.submit_mt(message).await {
            Ok(sms_id) => Some(sms_id),
            Err(e) => {
                warn!("MSC SMS: the SMSC would not take the message: {e}");
                None
            }
        }
    }

    pub(crate) async fn handle_adds_page_ack(&mut self, msg: &AddsPageAckMessage) {
        let tag = match msg.tag {
            Some(t) => t.0,
            None => {
                warn!("MSC SMS: ADDS Page Ack missing tag — cannot correlate");
                return;
            }
        };
        let correlation = match self.pending.remove(&tag) {
            Some(c) => c,
            None => {
                warn!("MSC SMS: ADDS Page Ack tag={tag} unknown — dropped");
                return;
            }
        };
        let success = msg.cause.is_none();
        let failure_reason = msg
            .cause
            .map(|c| format!("ADDS Page Ack cause=0x{:02X}", c.0));
        let result = if success {
            OutcomeResult::Delivered
        } else {
            OutcomeResult::NotReachable
        };
        self.report(
            correlation.sms_id,
            correlation.attempt_id,
            result,
            failure_reason,
        )
        .await;
        info!(
            "MSC SMS: ADDS Page Ack sms_id={} success={success} tag={tag}",
            correlation.sms_id
        );
    }

    /// Handles an incoming ADDS Deliver Ack from the BS (traffic-channel delivery result).
    pub(crate) async fn handle_adds_deliver_ack(&mut self, msg: &AddsDeliverAckMessage) {
        let tag = match msg.tag {
            Some(t) => t.0,
            None => {
                warn!("MSC SMS: ADDS Deliver Ack missing tag — cannot correlate");
                return;
            }
        };
        let correlation = match self.pending.remove(&tag) {
            Some(c) => c,
            None => {
                warn!("MSC SMS: ADDS Deliver Ack tag={tag} unknown — dropped");
                return;
            }
        };
        let success = msg.cause.is_none();
        let failure_reason = msg
            .cause
            .map(|c| format!("ADDS Deliver Ack cause=0x{:02X}", c.0));
        let result = if success {
            OutcomeResult::Delivered
        } else {
            OutcomeResult::NotReachable
        };
        self.report(
            correlation.sms_id,
            correlation.attempt_id,
            result,
            failure_reason,
        )
        .await;
        info!(
            "MSC SMS: ADDS Deliver Ack sms_id={} success={success} tag={tag}",
            correlation.sms_id
        );
    }

    /// Handles an ADDS Transfer from BS (MO SMS received on access channel).
    pub(crate) async fn handle_adds_transfer(
        &mut self,
        msg: &AddsTransferMessage,
        serving_node: &BaseStationId,
    ) {
        self.record_mo_sms(
            &msg.mobile_identity_imsi,
            msg.mobile_identity_esn.as_ref(),
            msg.mobile_identity_meid.as_ref(),
            Some(serving_node),
            &msg.adds_user_part,
        )
        .await;
    }

    /// Handles an ADDS Deliver from BS (MO SMS received on traffic channel).
    pub(crate) async fn handle_adds_deliver_mo(
        &mut self,
        msg: &AddsDeliverMessage,
        mobile_identity: &MobileIdentity,
        mobile_identity_esn: Option<&MobileIdentity>,
        mobile_identity_meid: Option<&MobileIdentity>,
    ) {
        self.record_mo_sms(
            mobile_identity,
            mobile_identity_esn,
            mobile_identity_meid,
            None,
            &msg.adds_user_part,
        )
        .await;
    }

    pub(crate) async fn expire_pending(&mut self, timeout: Duration) {
        let now = Instant::now();
        let expired_tags: Vec<u32> = self
            .pending
            .iter()
            .filter(|(_, c)| now.duration_since(c.sent_at) > timeout)
            .map(|(k, _)| *k)
            .collect();
        for tag in expired_tags {
            if let Some(c) = self.pending.remove(&tag) {
                warn!(
                    "MSC SMS: delivery tag={} expired after {}s for sms_id={}",
                    tag,
                    timeout.as_secs(),
                    c.sms_id
                );
                self.report(
                    c.sms_id,
                    c.attempt_id,
                    OutcomeResult::NotReachable,
                    Some("delivery ack timeout".to_string()),
                )
                .await;
            }
        }
    }

    // Internal helpers

    fn alloc_tag(&mut self) -> u32 {
        let tag = self.next_tag;
        self.next_tag = self.next_tag.wrapping_add(1).max(1);
        tag
    }

    async fn record_mo_sms(
        &mut self,
        mobile_identity_imsi: &MobileIdentity,
        mobile_identity_esn: Option<&MobileIdentity>,
        mobile_identity_meid: Option<&MobileIdentity>,
        serving_node: Option<&BaseStationId>,
        adds_user_part: &AddsUserPart,
    ) {
        if adds_user_part.burst_type != 0x03 {
            warn!(
                "MSC SMS: ADDS MO message burst_type=0x{:02X} (not SMS) — dropped",
                adds_user_part.burst_type
            );
            return;
        }

        let Some(decoded) = cdma_common::sms::decode_mo_sms(&adds_user_part.data) else {
            warn!("MSC SMS: failed to decode C.S0015-B MO SMS payload");
            return;
        };

        let imsi = match mobile_identity_imsi {
            MobileIdentity::Imsi(imsi) => imsi.as_str(),
            MobileIdentity::Esn(_) => {
                warn!("MSC SMS: ADDS Transfer with ESN-only identity, no IMSI");
                return;
            }
            MobileIdentity::Meid(_) => {
                warn!("MSC SMS: ADDS Transfer with MEID-only identity, no IMSI");
                return;
            }
        };
        let resolved = match mo_sms_lookup(
            imsi,
            mobile_identity_esn,
            mobile_identity_meid,
            serving_node,
        ) {
            Some(MoSmsLookup::Complete(identity_key)) => {
                match self.hlr.resolve_by_identity(&identity_key).await {
                    Ok(resolved) => resolved,
                    Err(error) => {
                        warn!("MSC SMS: HLR identity lookup failed: {error}");
                        return;
                    }
                }
            }
            Some(MoSmsLookup::Registration(node)) => {
                match self.hlr.resolve_registered_imsi(imsi, &node.0).await {
                    Ok(resolved) => {
                        if let Some(subscriber) = &resolved {
                            info!(
                                "MSC SMS: resolved IMSI-only origin {imsi} on {} to subscriber {} ({}) using registration binding",
                                node.0,
                                subscriber.subscriber.subscriber_id,
                                subscriber.subscriber.phone_number
                            );
                        }
                        resolved
                    }
                    Err(error) => {
                        warn!(
                            "MSC SMS: registered IMSI lookup failed for {imsi} on {}: {error}",
                            node.0
                        );
                        return;
                    }
                }
            }
            None => None,
        };
        let Some(originating) = resolved.map(|resolved| resolved.subscriber) else {
            warn!("MSC SMS: no subscriber identity or matching registration for IMSI {imsi}");
            if self.access_mode == crate::admission::AccessMode::Open {
                self.record_mo_sms_unknown_originator(imsi, &decoded).await;
            }
            return;
        };

        if self.access_mode == crate::admission::AccessMode::Closed
            && originating.status != cdma_hlr::model::SubscriberStatus::Active
        {
            warn!(
                "MSC SMS: MO submission from subscriber {} refused by closed access policy",
                originating.subscriber_id
            );
            return;
        }

        let Some(submitter) = self.mo_submitter.as_ref() else {
            warn!("MSC SMS: no SMSC to hand the message to — dropped");
            return;
        };
        let message = cdma_smsc::client::MoSubmission {
            originating_number: originating.phone_number.clone(),
            destination_number: decoded.destination_number.clone(),
            text: decoded.text.clone(),
            originating_subscriber_id: Some(originating.subscriber_id),
            teleservice_id: Some(decoded.teleservice_id),
            raw_user_data: None,
            fingerprint: MoSmsFingerprint {
                teleservice_id: decoded.teleservice_id,
                message_type: decoded.message_type,
                message_id: decoded.message_id,
            },
        };
        let submitter = Arc::clone(submitter);
        tokio::spawn(async move {
            match submitter.submit_mo(message).await {
                Ok((sms_id, accepted)) => info!(
                    "MSC SMS: handed {} -> {} to the SMSC as {sms_id} (new={accepted})",
                    originating.phone_number, decoded.destination_number
                ),
                Err(e) => warn!("MSC SMS: the SMSC would not take the message: {e}"),
            }
        });
    }

    async fn record_mo_sms_unknown_originator(
        &self,
        originating_number: &str,
        decoded: &cdma_common::sms::DecodedMoSms,
    ) {
        let _ = self
            .smsc
            .create_or_get_recent_mo_submission(
                originating_number,
                &decoded.destination_number,
                &decoded.text,
                None,
                None,
                &MoSmsFingerprint {
                    teleservice_id: decoded.teleservice_id,
                    message_type: decoded.message_type,
                    message_id: decoded.message_id,
                },
            )
            .await;
    }
}

fn mo_sms_identity_key(
    imsi: &str,
    esn: Option<&MobileIdentity>,
    meid: Option<&MobileIdentity>,
) -> Option<cdma_hlr::model::MobileIdentityKey> {
    let esn = match esn {
        Some(MobileIdentity::Esn(value)) => Some(*value),
        _ => None,
    };
    let meid = meid.and_then(crate::runtime::meid_hex);
    cdma_hlr::model::MobileIdentityKey::from_parts(Some(imsi), esn, meid.as_deref()).ok()
}

enum MoSmsLookup {
    Complete(cdma_hlr::model::MobileIdentityKey),
    Registration(BaseStationId),
}

fn mo_sms_lookup(
    imsi: &str,
    esn: Option<&MobileIdentity>,
    meid: Option<&MobileIdentity>,
    serving_node: Option<&BaseStationId>,
) -> Option<MoSmsLookup> {
    if let Some(identity) = mo_sms_identity_key(imsi, esn, meid) {
        return Some(MoSmsLookup::Complete(identity));
    }
    if esn.is_none() && meid.is_none() {
        return serving_node.cloned().map(MoSmsLookup::Registration);
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use cdma_hlr::model::MobileIdentityKey;

    #[test]
    fn mo_sms_identity_accepts_meid_without_esn() {
        let meid = MobileIdentity::Meid([0xa0, 0x00, 0x00, 0x2c, 0xc8, 0x1a, 0x35]);
        assert_eq!(
            mo_sms_identity_key("310005551239876", None, Some(&meid)),
            Some(MobileIdentityKey::ImsiMeid {
                imsi: "310005551239876".into(),
                meid: "a000002cc81a35".into(),
            })
        );
    }

    #[test]
    fn mo_sms_identity_requires_hardware_identity() {
        assert!(mo_sms_identity_key("310005551239876", None, None).is_none());
    }
    #[test]
    fn imsi_only_sms_uses_serving_registration() {
        let node = BaseStationId("femto-fcs-1".into());
        assert!(
            matches!(mo_sms_lookup("310005551239876", None, None, Some(&node)), Some(MoSmsLookup::Registration(found)) if found == node)
        );
        assert!(mo_sms_lookup("310005551239876", None, None, None).is_none());
    }

    #[test]
    fn supplied_hardware_identity_never_uses_registration_fallback() {
        let node = BaseStationId("femto-fcs-1".into());
        let esn = MobileIdentity::Esn(0x80b6_6bd7);
        assert!(matches!(
            mo_sms_lookup("310005551239876", Some(&esn), None, Some(&node)),
            Some(MoSmsLookup::Complete(MobileIdentityKey::ImsiEsn {
                esn: 0x80b6_6bd7,
                ..
            }))
        ));
        assert!(mo_sms_lookup("310005551239876", None, Some(&esn), Some(&node)).is_none());
    }
}
