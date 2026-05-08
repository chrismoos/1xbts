//! SMS delivery state and flows for the BSC.
//!
//! Owns mobile-originated SMS routing, mobile-terminated SMS data
//! bursts, redelivery scheduling, and the SMSC repository touchpoints.
//! Migrates to MSC / SMSC service-path integration in a later PR. WS-0
//! PR3 sibling module per
//! `docs/architecture-update/09-pr3-method-map.md`.

use std::time::{Duration, Instant};

use cdma_common::error::Error;
use cdma_common::events::AccessChannelEvent;
use cdma_common::lac::message_types::MessageId;
use cdma_common::lac::paging_messages::{
    ForwardDataBurstMessage, MsAddress, MsPageAddress, PagingChannelMessage,
};
use cdma_common::sms as air_sms;
use log::{info, warn};
use uuid::Uuid;

use crate::addressing::{format_ms_address, parse_sms_target_address};

use super::{Bsc, DEFAULT_PAGE_TIMEOUT_MS, MobileStation, MsState, PendingPage};

/// SMS request to deliver to a registered mobile station.
pub struct SmsRequest {
    pub originating_number: String,
    pub text: String,
    /// Target MS by forward-link address string (e.g. "ESN:0x12345678").
    /// Debug escape hatch -- prefer destination_number for production use.
    pub target_address: Option<String>,
    /// Preferred subscriber identity resolved via HLR, if available.
    pub target_subscriber_id: Option<Uuid>,
    /// How long (ms) the BSC retries paging before giving up. Default: 30 000 ms.
    pub timeout_ms: Option<u64>,
    /// Subscriber phone number -- preferred routing method via HLR.
    pub destination_number: Option<String>,
    /// SMSC submission ID for state tracking (set by gRPC layer).
    pub sms_id: Option<Uuid>,
    /// SMSC delivery attempt ID for tracking this specific delivery attempt.
    pub delivery_attempt_id: Option<Uuid>,
    /// A1 ADDS Tag from MSC's ADDS Page — echoed in ADDS Page Ack. None for BSC-originated SMS.
    pub a1_tag: Option<u32>,
    /// Pre-encoded C.S0015-B payload from MSC's ADDS Page — used instead of re-encoding.
    pub raw_payload: Option<Vec<u8>>,
}

fn sms_target_address_matches_mobile(ms: &MobileStation, target: &MsAddress) -> bool {
    match target {
        MsAddress::Esn(esn) => {
            ms.esn == Some(*esn) || matches!(ms.fwd_address, MsAddress::Esn(v) if v == *esn)
        }
        MsAddress::ImsiS {
            imsi_m_s1,
            imsi_m_s2,
        } => {
            if let Some(ref imsi) = ms.imsi {
                cdma_common::paging::imsi_s_from_imsi(imsi)
                    .is_some_and(|(s1, s2)| s1 == *imsi_m_s1 && s2 == *imsi_m_s2)
            } else {
                ms.matches_imsi_s(*imsi_m_s1, *imsi_m_s2)
            }
        }
        MsAddress::ImsiClass0 {
            imsi_m_s1,
            imsi_m_s2,
            mcc,
            imsi_11_12,
        } => {
            let imsi_s_matches = if let Some(ref imsi) = ms.imsi {
                cdma_common::paging::imsi_s_from_imsi(imsi)
                    .is_some_and(|(s1, s2)| s1 == *imsi_m_s1 && s2 == *imsi_m_s2)
            } else {
                ms.matches_imsi_s(*imsi_m_s1, *imsi_m_s2)
            };
            if !imsi_s_matches {
                return false;
            }
            if let MsAddress::ImsiClass0 {
                mcc: ms_mcc,
                imsi_11_12: ms_imsi_11_12,
                ..
            } = &ms.fwd_address
            {
                if ms_mcc != mcc || ms_imsi_11_12 != imsi_11_12 {
                    return false;
                }
            }
            true
        }
    }
}

fn sms_target_matches_mobile(
    ms: &MobileStation,
    target_subscriber_id: Option<Uuid>,
    target_address: Option<&str>,
) -> bool {
    if let Some(subscriber_id) = target_subscriber_id {
        if ms.subscriber_id == Some(subscriber_id) {
            return true;
        }
    }

    let Some(target_address) = target_address else {
        return false;
    };

    if format_ms_address(&ms.fwd_address) == target_address {
        return true;
    }

    parse_sms_target_address(target_address)
        .as_ref()
        .is_some_and(|parsed| sms_target_address_matches_mobile(ms, parsed))
}

/// Identifies how a pending SMS ACK will be completed.
#[derive(Debug, Clone, PartialEq)]
pub(crate) enum SmsAckKey {
    /// F-PCH delivery: BTS owns L2, BSC tracks by Abis correlation_id.
    PchCorrelation(u32),
    /// F-TCH delivery: BSC owns L2, tracks by (mobile address, msg_seq).
    TrafficMsgSeq { addr: MsAddress, msg_seq: u8 },
}

/// SMS Data Burst awaiting delivery confirmation.
pub(crate) struct PendingSmsAck {
    pub(crate) key: SmsAckKey,
    pub(crate) sms_id: Option<Uuid>,
    pub(crate) delivery_attempt_id: Option<Uuid>,
    pub(crate) addr: MsAddress,
    pub(crate) sent_at: Instant,
    pub(crate) a1_tag: Option<u32>,
}

pub(crate) struct ExpiredSmsAck {
    pub(crate) sms_id: Uuid,
    pub(crate) delivery_attempt_id: Option<Uuid>,
    pub(crate) addr: MsAddress,
}

pub(crate) struct SmsService {
    pub(crate) message_id: u16,
    pub(crate) pending_acks: Vec<PendingSmsAck>,
}

impl Default for SmsService {
    fn default() -> Self {
        Self {
            message_id: 0,
            pending_acks: Vec::new(),
        }
    }
}

impl SmsService {
    pub(crate) fn new() -> Self {
        Self::default()
    }
}

impl SmsService {
    pub(crate) fn next_message_id(&mut self) -> u16 {
        self.message_id = self.message_id.wrapping_add(1);
        self.message_id
    }

    pub(crate) fn complete_ack(&mut self, key: &SmsAckKey) -> Option<PendingSmsAck> {
        let pos = self.pending_acks.iter().position(|p| p.key == *key)?;
        Some(self.pending_acks.remove(pos))
    }

    pub(crate) fn fail_ack(&mut self, key: &SmsAckKey) -> Option<PendingSmsAck> {
        self.complete_ack(key)
    }

    pub(crate) fn has_pending_ack_for(&self, addr: &MsAddress) -> bool {
        self.pending_acks.iter().any(|p| p.addr == *addr)
    }

    pub(crate) fn track_pending_ack(
        &mut self,
        key: SmsAckKey,
        sms_id: Option<Uuid>,
        delivery_attempt_id: Option<Uuid>,
        addr: MsAddress,
        sent_at: Instant,
        a1_tag: Option<u32>,
    ) {
        self.pending_acks.retain(|p| p.key != key);
        self.pending_acks.push(PendingSmsAck {
            key,
            sms_id,
            delivery_attempt_id,
            addr,
            sent_at,
            a1_tag,
        });
    }

    /// Track a pending SMS ACK after a Data Burst has been sent on the
    /// forward traffic channel. Keyed by (addr, msg_seq) since BSC owns
    /// F-TCH ARQ.
    pub(crate) fn track_pending_traffic_ack(
        &mut self,
        msg_seq: u8,
        addr: &MsAddress,
        sms_req: &SmsRequest,
    ) {
        if let Some(sms_id) = sms_req.sms_id {
            let delivery_attempt_id = sms_req.delivery_attempt_id;
            let key = SmsAckKey::TrafficMsgSeq {
                addr: addr.clone(),
                msg_seq,
            };
            self.track_pending_ack(
                key,
                Some(sms_id),
                delivery_attempt_id,
                addr.clone(),
                Instant::now(),
                sms_req.a1_tag,
            );
        }
    }

    pub(crate) fn send_access_data_burst(
        &mut self,
        access_tx: &super::AccessTx,
        addr: &MsAddress,
        ack_msg_seq: u8,
        sms_req: &SmsRequest,
        _requested_tx_time: Option<cdma_common::time::CdmaSystemTime>,
        _tx_deadline: Option<cdma_common::time::CdmaSystemTime>,
    ) -> Result<(), Error> {
        let tl_message_id: u16 = if let Some(uuid) = sms_req.sms_id {
            let bytes = uuid.as_bytes();
            u16::from_le_bytes([bytes[0], bytes[1]])
        } else {
            self.next_message_id()
        };
        let sms_bytes = if let Some(ref raw) = sms_req.raw_payload {
            raw.clone()
        } else {
            air_sms::encode_sms_deliver(&sms_req.originating_number, &sms_req.text, tl_message_id)
        };

        let data_burst = ForwardDataBurstMessage {
            msg_number: 1,
            burst_type: 3,
            num_msgs: 1,
            fields: sms_bytes,
        };

        let sdu = data_burst.to_sdu();
        let correlation_id = access_tx.send_directed_fpch(
            addr,
            MessageId::DataBurst,
            PagingChannelMessage::DataBurst(data_burst.clone()),
            sdu,
            true,
        )?;

        info!(
            "BSC: sending SMS Data Burst (ack_seq={}, correlation_id={}, payload_bytes={})",
            ack_msg_seq,
            correlation_id,
            data_burst.fields.len()
        );

        let key = SmsAckKey::PchCorrelation(correlation_id);
        self.track_pending_ack(
            key,
            sms_req.sms_id,
            sms_req.delivery_attempt_id,
            addr.clone(),
            Instant::now(),
            sms_req.a1_tag,
        );

        Ok(())
    }

    /// Complete a pending MT SMS delivery. Matches by `SmsAckKey` — either
    /// Abis correlation_id (PCH) or (addr, msg_seq) (traffic channel).
    pub(crate) fn complete_delivery(&mut self, key: &SmsAckKey) -> Option<PendingSmsAck> {
        let pending = self.complete_ack(key)?;
        info!(
            "BSC: SMS {:?} delivery confirmed ({:?})",
            pending.sms_id, key
        );
        Some(pending)
    }

    pub(crate) fn fail_delivery(&mut self, key: &SmsAckKey, cause: u8) -> Option<PendingSmsAck> {
        let pending = self.fail_ack(key)?;
        info!(
            "BSC: SMS {:?} delivery failed ({:?}, cause=0x{:02X})",
            pending.sms_id, key, cause
        );
        Some(pending)
    }
}

/// Result of decoding an MO SMS Data Burst payload.
pub(crate) struct DecodedMoSms {
    pub originating_number: String,
    pub originating_subscriber_id: Option<Uuid>,
    pub destination_number: String,
    pub text: String,
    pub teleservice_id: u16,
    pub message_type: u8,
    pub message_id: u16,
    pub reply_seq: Option<u8>,
}

impl Bsc {
    /// Decode an MO SMS Data Burst payload and resolve the originating MS.
    /// Shared between access-channel and traffic-channel reverse signaling.
    pub(crate) fn decode_reverse_sms(
        &self,
        fields: &[u8],
        originating_number: &str,
        originating_subscriber_id: Option<Uuid>,
        channel_label: &str,
    ) -> Option<DecodedMoSms> {
        let decoded = match air_sms::decode_mo_sms(fields) {
            Some(d) => d,
            None => {
                warn!("BSC: {} MO SMS decode failed", channel_label);
                return None;
            }
        };

        info!(
            "BSC: {} MO SMS decoded: teleservice=0x{:04X} dest=\"{}\" text=\"{}\" msg_type={} msg_id={}",
            channel_label,
            decoded.teleservice_id,
            decoded.destination_number,
            decoded.text,
            decoded.message_type,
            decoded.message_id,
        );

        if decoded.destination_number.is_empty() {
            warn!(
                "BSC: {} MO SMS has no destination number, cannot route",
                channel_label
            );
            return None;
        }

        Some(DecodedMoSms {
            originating_number: originating_number.to_string(),
            originating_subscriber_id,
            destination_number: decoded.destination_number,
            text: decoded.text,
            teleservice_id: decoded.teleservice_id,
            message_type: decoded.message_type,
            message_id: decoded.message_id,
            reply_seq: decoded.reply_seq,
        })
    }

    /// Route a decoded MO SMS to the MSC via ADDS Transfer (access channel).
    pub(crate) fn route_decoded_mo_sms(
        &self,
        sms: &DecodedMoSms,
        fwd_addr: &MsAddress,
        raw_fields: &[u8],
    ) {
        info!(
            "BSC: routing MO SMS from {} to {} via ADDS Transfer",
            sms.originating_number, sms.destination_number
        );
        let mobile = self.mobiles.iter().find(|ms| ms.fwd_address == *fwd_addr);
        let imsi = mobile.and_then(|ms| ms.imsi.as_ref()).cloned();
        let esn = mobile.and_then(|ms| ms.esn);
        let mobile_identity_imsi = imsi
            .map(cdma_ios::MobileIdentity::Imsi)
            .or_else(|| esn.map(cdma_ios::MobileIdentity::Esn))
            .unwrap_or_else(|| cdma_ios::MobileIdentity::Imsi("UNKNOWN".to_string()));
        let transfer = cdma_ios::AddsTransferMessage {
            mobile_identity_imsi,
            adds_user_part: cdma_ios::AddsUserPart {
                burst_type: 0x03,
                data: raw_fields.to_vec(),
            },
            mobile_identity_esn: esn.map(cdma_ios::MobileIdentity::Esn),
        };
        let client = self.a1.msc_client.clone();
        tokio::spawn(async move {
            match transfer.encode() {
                Ok(payload) => {
                    let msg = cdma_ios::EncodedA1Message::from_message(&cdma_ios::Message::new(
                        cdma_ios::MessageType::AddsTransfer,
                        payload,
                    ));
                    if let Err(e) = client.send_a1(msg).await {
                        log::warn!("BSC: failed to send ADDS Transfer to MSC: {e}");
                    }
                }
                Err(e) => log::warn!("BSC: failed to encode ADDS Transfer: {e}"),
            }
        });
    }

    /// Build an SMS Data Burst and deliver it via the best available channel.
    ///
    /// Uses `send_forward_signaling_paging_or_traffic` to route to F-DSCH
    /// (traffic) or returns `NeedsPaging` so the caller can page.
    pub(crate) fn send_sms_data_burst_auto(
        &mut self,
        fwd_address: &MsAddress,
        sms_req: &SmsRequest,
    ) -> Result<super::ForwardSignalingRoute, Error> {
        use super::ForwardSignalingRoute;

        let tl_message_id: u16 = if let Some(uuid) = sms_req.sms_id {
            let bytes = uuid.as_bytes();
            u16::from_le_bytes([bytes[0], bytes[1]])
        } else {
            self.sms.next_message_id()
        };
        let sms_bytes = if let Some(ref raw) = sms_req.raw_payload {
            raw.clone()
        } else {
            air_sms::encode_sms_deliver(&sms_req.originating_number, &sms_req.text, tl_message_id)
        };

        let data_burst = ForwardDataBurstMessage {
            msg_number: 1,
            burst_type: 3, // SMS
            num_msgs: 1,
            fields: sms_bytes,
        };

        let result = self.send_forward_signaling_paging_or_traffic(fwd_address, data_burst)?;

        if let ForwardSignalingRoute::SentOnTraffic { msg_seq } = &result {
            self.sms
                .track_pending_traffic_ack(*msg_seq, fwd_address, sms_req);
        }

        Ok(result)
    }
}

impl Bsc {
    pub(crate) fn handle_sms_request(&mut self, sms_req: SmsRequest) {
        use super::ForwardSignalingRoute;

        info!(
            "BSC: SMS request received: from=\"{}\" text=\"{}\" target={:?} subscriber={:?} sms_id={:?}",
            sms_req.originating_number,
            sms_req.text,
            sms_req.target_address,
            sms_req.target_subscriber_id,
            sms_req.sms_id,
        );

        // Find the target MS — by address string if specified, otherwise first registered
        let target_addr: Option<MsAddress> = if sms_req.target_address.is_some()
            || sms_req.target_subscriber_id.is_some()
        {
            self.mobiles
                .iter()
                .find(|ms| {
                    sms_target_matches_mobile(
                        ms,
                        sms_req.target_subscriber_id,
                        sms_req.target_address.as_deref(),
                    )
                })
                .map(|ms| ms.fwd_address.clone())
        } else {
            self.mobiles
                .iter()
                .find(|ms| ms.state == MsState::Registered || ms.state == MsState::TrafficActive)
                .map(|ms| ms.fwd_address.clone())
        };

        let Some(target_addr) = target_addr else {
            let mobile_summary: Vec<String> = self
                .mobiles
                .iter()
                .map(|ms| {
                    format!(
                        "addr={} sub_id={:?} state={:?}",
                        format_ms_address(&ms.fwd_address),
                        ms.subscriber_id,
                        ms.state,
                    )
                })
                .collect();
            warn!(
                "BSC: no matching mobile for SMS (target_addr={:?} target_sub={:?}), mobiles=[{}]",
                sms_req.target_address,
                sms_req.target_subscriber_id,
                mobile_summary.join("; "),
            );
            warn!("MSC SMS: cannot deliver — destination unknown");
            return;
        };

        // Try traffic channel first; fall through to paging if MS is idle.
        match self.send_sms_data_burst_auto(&target_addr, &sms_req) {
            Ok(ForwardSignalingRoute::SentOnTraffic { .. }) => return,
            Ok(ForwardSignalingRoute::NeedsPaging) => {}
            Err(e) => {
                warn!("BSC: failed to send SMS on traffic: {}", e);
                warn!("MSC SMS: cannot deliver — traffic channel send failed");
                return;
            }
        }

        if self.paging.has_pending_sms_page() {
            warn!("BSC: page already in progress — rejecting SMS request");
            warn!("MSC SMS: cannot deliver — page already in progress");
            return;
        }

        let Some((page_addr, pgslot, sci, fwd_address)) =
            self.mobiles.get(&target_addr).and_then(|ms| {
                ms.page_address()
                    .map(|p| (p, ms.pgslot, ms.slot_cycle_index, ms.fwd_address.clone()))
            })
        else {
            warn!("BSC: mobile has no pageable address (no IMSI_S or ESN)");
            warn!("MSC SMS: cannot deliver — mobile has no pageable address");
            return;
        };
        self.mobiles.set_state(&target_addr, MsState::Paged);

        let timeout_ms = sms_req.timeout_ms.unwrap_or(DEFAULT_PAGE_TIMEOUT_MS);

        self.paging.queue_sms_page(PendingPage {
            sms: sms_req,
            page_address: page_addr.clone(),
            fwd_address,
            pgslot,
            slot_cycle_index: sci,
            started_at: Instant::now(),
            timeout: Duration::from_millis(timeout_ms),
            retry_count: 0,
            next_retry_at: tokio::time::Instant::now(),
            last_target_chip: None,
            page_msg_seq: None,
            page_correlation_id: None,
        });

        self.publish_mobiles();
        match self.send_page_for_sms(&page_addr, pgslot, sci, None, None) {
            Ok((target_chip, page_seq, page_correlation_id)) => {
                let next_retry_at = self.compute_next_retry_at(pgslot, sci, target_chip);
                self.paging.record_sms_page_sent(
                    target_chip,
                    next_retry_at,
                    page_seq,
                    page_correlation_id,
                );
            }
            Err(e) => warn!("BSC: failed to send page: {}", e),
        }
    }

    /// Send a General Page Message for SMS delivery.
    pub(crate) fn send_page_for_sms(
        &self,
        page_addr: &MsPageAddress,
        pgslot: Option<u16>,
        slot_cycle_index: u8,
        after_chip: Option<u64>,
        override_msg_seq: Option<u8>,
    ) -> Result<(Option<u64>, u8, Option<u32>), Error> {
        self.send_general_page(
            page_addr,
            pgslot,
            slot_cycle_index,
            after_chip,
            None,
            "SMS delivery",
            override_msg_seq,
        )
    }

    /// Handle a Data Burst from the reverse traffic channel (MO SMS).
    ///
    /// Per the IS-2000 signaling trace for SO6 MO SMS:
    /// 1. BS sends BS Ack Order (ack the data burst)
    /// 2. BS sends SMS Cause Code Data Burst on f-dsch (ack_req=1)
    /// 3. MS sends MS Ack Order -> BS sends Release Order -> teardown
    pub(crate) fn handle_traffic_data_burst(&mut self, walsh_code: u8, event: &AccessChannelEvent) {
        let burst_type = match event.burst_type {
            Some(bt) => bt,
            None => {
                warn!(
                    "BSC: traffic Data Burst without burst_type on walsh={}",
                    walsh_code
                );
                return;
            }
        };
        let fields = match event.data_burst_fields.as_ref() {
            Some(f) => f,
            None => {
                warn!(
                    "BSC: traffic Data Burst without payload on walsh={}",
                    walsh_code
                );
                return;
            }
        };

        // BS Ack is sent by the generic ack_req handler in handle_traffic_event.
        let ack_seq = event.msg_seq.unwrap_or(0);

        if burst_type != 3 {
            info!(
                "BSC: traffic Data Burst burst_type={} on walsh={} (not SMS)",
                burst_type, walsh_code
            );
            return;
        }

        info!(
            "BSC: MO SMS via traffic channel walsh={} payload_len={}",
            walsh_code,
            fields.len()
        );

        let sender = self
            .mobiles
            .get_by_walsh(walsh_code)
            .and_then(|ms| ms.phone_number.clone().map(|n| (n, ms.subscriber_id)));
        let (originating_number, originating_subscriber_id) =
            sender.clone().unwrap_or_else(|| (String::new(), None));

        let Some(sms) = self.decode_reverse_sms(
            fields,
            &originating_number,
            originating_subscriber_id,
            &format!("traffic walsh={}", walsh_code),
        ) else {
            return;
        };

        let reply_seq = sms.reply_seq.unwrap_or(0);
        if sender.is_none() {
            const TEMPORARY_ERROR_CLASS: u8 = 0b10;
            const SMS_CAUSE_NETWORK_FAILURE: u8 = 0x03;
            warn!(
                "BSC: rejecting MO SMS on traffic walsh={} with temporary SMS Cause Code: no subscriber/phone number resolved for originating MS (dest=\"{}\" teleservice=0x{:04X} msg_id={} reply_seq={})",
                walsh_code, sms.destination_number, sms.teleservice_id, sms.message_id, reply_seq,
            );
            self.send_traffic_sms_cause_code(
                walsh_code,
                ack_seq,
                reply_seq,
                TEMPORARY_ERROR_CLASS,
                Some(SMS_CAUSE_NETWORK_FAILURE),
            );
            return;
        }

        self.send_traffic_mo_sms_to_msc(walsh_code, fields);

        self.send_traffic_sms_cause_code(walsh_code, ack_seq, reply_seq, 0, None);
    }

    fn send_traffic_sms_cause_code(
        &mut self,
        walsh_code: u8,
        ack_seq: u8,
        reply_seq: u8,
        error_class: u8,
        cause_code: Option<u8>,
    ) {
        let cause_code_bytes =
            air_sms::encode_sms_cause_code_with_cause(reply_seq, error_class, cause_code);
        let data_burst = ForwardDataBurstMessage {
            msg_number: 1,
            burst_type: 3,
            num_msgs: 1,
            fields: cause_code_bytes,
        };
        let sdu = data_burst.to_sdu();
        if let Err(e) = self.send_traffic_signaling(
            walsh_code,
            sdu,
            MessageId::DataBurst,
            ack_seq,
            true,
            None,
            None,
            None,
            Some(data_burst),
            None,
        ) {
            warn!(
                "BSC: failed to send SMS Cause Code on walsh={} reply_seq={} error_class={} cause_code={:?}: {}",
                walsh_code, reply_seq, error_class, cause_code, e
            );
        } else {
            info!(
                "BSC: sent SMS Cause Code on F-TCH walsh={} reply_seq={} error_class={} cause_code={:?}",
                walsh_code, reply_seq, error_class, cause_code
            );
        }

        self.mobiles.update_tc(walsh_code, |_, tc| {
            tc.mark_sms_pending_release();
        });
    }

    /// Send a traffic-channel MO SMS to the MSC via ADDS Deliver (DTAP 0x4c).
    fn send_traffic_mo_sms_to_msc(&self, walsh_code: u8, fields: &[u8]) {
        let deliver = cdma_ios::AddsDeliverMessage {
            adds_user_part: cdma_ios::AddsUserPart {
                burst_type: 0x03,
                data: fields.to_vec(),
            },
            tag: None,
        };
        let call_id = self
            .mobiles
            .get_traffic_channel(walsh_code)
            .and_then(|tc| tc.a1_call_id);
        if call_id.is_none() {
            warn!(
                "BSC: MO SMS on traffic walsh={} has no A1 call_id — MSC will not be able to resolve originator",
                walsh_code
            );
        }
        let client = self.a1.msc_client.clone();
        tokio::spawn(async move {
            match deliver.encode() {
                Ok(payload) => {
                    let msg = cdma_ios::EncodedA1Message::from_message_for_call(
                        &cdma_ios::Message::new(cdma_ios::MessageType::AddsDeliver, payload),
                        call_id,
                    );
                    if let Err(e) = client.send_a1(msg).await {
                        log::warn!("BSC: failed to send ADDS Deliver (MO) to MSC: {e}");
                    }
                }
                Err(e) => log::warn!("BSC: failed to encode ADDS Deliver (MO): {e}"),
            }
        });
    }
}
