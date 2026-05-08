//! Access-channel ingress: registration, origination, page response,
//! and order handling.
//!
//! WS-0 PR3 sibling module per
//! `docs/architecture-update/09-pr3-method-map.md`.

use std::sync::Arc;
use std::time::{Duration, Instant};

use cdma_common::access::{AccessMessage, OriginationMessage};
use cdma_common::error::Error;
use cdma_common::events::AccessChannelEvent;
use cdma_common::formatting::format_dtmf_digits;
use cdma_common::lac::{
    message_types::MessageId,
    paging_messages::{MsAddress, MsPageAddress, OrderMessage, PagingChannelMessage},
};
use cdma_voice::VoiceCodec;
use chrono::Duration as ChronoDuration;
use log::{debug, info, warn};
use uuid::Uuid;

use crate::addressing::{format_ms_address, is_packet_data_so, select_imsi_class0_forward_address};
use cdma_common::consts::SR1_CHIP_RATE_HZ;

use super::{
    AccessRegistrationUpdate, Bsc, MobileStation, PendingPage, VoiceLegRole,
    mark_reverse_regular_msg_seq_received, next_pch_correlation_id,
};

/// Result of an async HLR subscriber lookup, sent back to the BSC run loop.
pub(crate) struct HlrResolution {
    pub(crate) fwd_address: MsAddress,
    pub(crate) subscriber_id: Option<Uuid>,
    pub(crate) phone_number: Option<String>,
    pub(crate) display_name: Option<String>,
    pub(crate) canonical_imsi: Option<String>,
}

#[derive(Default)]
pub(crate) struct AccessService;

/// Implementation-defined r-csch inactivity interval after which MSG_SEQ_RCVD
/// is cleared before processing the next access PDU.
pub(crate) const ACCESS_INACTIVITY_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone)]
pub(crate) struct AccessTx {
    pub(crate) bts_client: Option<Arc<dyn crate::abis_edge::BtsControlClient>>,
}

pub(crate) enum AccessDuplicateDecision {
    NewSdu,
    Duplicate { addr: MsAddress, msg_seq: u8 },
}

impl AccessTx {
    pub(crate) fn new(bts_client: Option<Arc<dyn crate::abis_edge::BtsControlClient>>) -> Self {
        Self { bts_client }
    }

    pub(crate) fn send_directed_fpch(
        &self,
        addr: &MsAddress,
        message_id: MessageId,
        _paging_message: PagingChannelMessage,
        sdu: cdma_common::bits::Bitstream,
        ack_req: bool,
    ) -> Result<u32, Error> {
        let wire_msg_type = message_id
            .wire_type(cdma_common::lac::message_types::WireChannel::ForwardCommon)
            .unwrap_or(0);
        let correlation_id = self.send_pch_for_directed(addr, wire_msg_type, &sdu, ack_req);
        Ok(correlation_id)
    }

    fn send_pch_for_directed(
        &self,
        addr: &MsAddress,
        wire_msg_type: u8,
        sdu: &cdma_common::bits::Bitstream,
        ack_req: bool,
    ) -> u32 {
        use cdma_abis::control::typed::{
            AbisAckNotify, AirInterfaceMessagePayload, CorrelationId, Layer2AckRequestResults,
            PchMessageTransferMessage,
        };
        let mobile_id = super::paging::mobile_identity_for_ms_address(addr);
        let sdu_bytes = sdu.to_packed_bytes();
        let aim = AirInterfaceMessagePayload::new(wire_msg_type, sdu_bytes);
        let corr_id = next_pch_correlation_id();
        let pch = PchMessageTransferMessage {
            correlation_id: Some(CorrelationId(corr_id)),
            mobile_identities: vec![mobile_id],
            cell_identifier_list: None,
            air_interface_message: aim.ok(),
            layer2_ack_request_results: if ack_req {
                Some(Layer2AckRequestResults::request())
            } else {
                None
            },
            abis_ack_notify: if ack_req { Some(AbisAckNotify) } else { None },
        };
        if let Some(ref bts_client) = self.bts_client {
            if let Err(e) = bts_client.send_pch_message(pch) {
                warn!("BSC: send_pch_message failed: {}", e);
            }
        }
        corr_id
    }

    pub(crate) fn send_order(
        &self,
        addr: &MsAddress,
        ack_msg_seq: u8,
        ack_req: bool,
        order: u8,
        ordq: u8,
        order_name: &str,
        requested_tx_time: Option<cdma_common::time::CdmaSystemTime>,
        tx_deadline: Option<cdma_common::time::CdmaSystemTime>,
    ) -> Result<(), Error> {
        let order_msg = OrderMessage {
            order,
            ordq,
            order_specific_fields: Vec::new(),
        };
        let paging_message = PagingChannelMessage::Order(order_msg.clone());
        let sdu = order_msg.to_sdu();

        self.send_directed_fpch(addr, MessageId::Order, paging_message, sdu, ack_req)?;

        let req_tx_chip =
            requested_tx_time.map(|t| cdma_common::time::chips_since_epoch(t, 1_228_800));
        let deadline_chip = tx_deadline.map(|t| cdma_common::time::chips_since_epoch(t, 1_228_800));
        info!(
            "BSC: sending {} (ack_seq={}, ack_req={}) requested_tx_chip={:?} deadline_chip={:?}",
            order_name, ack_msg_seq, ack_req, req_tx_chip, deadline_chip,
        );
        Ok(())
    }

    pub(crate) fn send_registration_accepted(
        &self,
        addr: &MsAddress,
        ack_msg_seq: u8,
        requested_tx_time: Option<cdma_common::time::CdmaSystemTime>,
        tx_deadline: Option<cdma_common::time::CdmaSystemTime>,
    ) -> Result<(), Error> {
        self.send_order(
            addr,
            ack_msg_seq,
            true,
            0b011011,
            0,
            "Registration Accepted Order",
            requested_tx_time,
            tx_deadline,
        )
    }

    pub(crate) fn send_bs_ack_order(
        &self,
        addr: &MsAddress,
        ack_msg_seq: u8,
        requested_tx_time: Option<cdma_common::time::CdmaSystemTime>,
        tx_deadline: Option<cdma_common::time::CdmaSystemTime>,
    ) -> Result<(), Error> {
        self.send_order(
            addr,
            ack_msg_seq,
            false,
            0b010000,
            0,
            "Base Station Acknowledgment Order",
            requested_tx_time,
            tx_deadline,
        )
    }

    pub(crate) fn send_service_option_rejected_release(
        &self,
        addr: &MsAddress,
        ack_msg_seq: u8,
        requested_tx_time: Option<cdma_common::time::CdmaSystemTime>,
        tx_deadline: Option<cdma_common::time::CdmaSystemTime>,
    ) -> Result<(), Error> {
        self.send_order(
            addr,
            ack_msg_seq,
            true,
            0b010101,
            0b00000010,
            "Release Order (requested service option rejected)",
            requested_tx_time,
            tx_deadline,
        )
    }
}

impl AccessService {
    pub(crate) fn new() -> Self {
        Self
    }

    pub(crate) fn handle_known_mobile_msg_seq(
        &mut self,
        mobile: &mut MobileStation,
        event: &AccessChannelEvent,
        activity_now: Instant,
    ) -> AccessDuplicateDecision {
        let Some(msg_seq) = event.msg_seq else {
            return AccessDuplicateDecision::NewSdu;
        };

        // Per C.S0004-E 3.1.1.2.2.2: if the MS has been inactive on
        // the r-csch, clear MSG_SEQ_RCVD before processing to avoid
        // false duplicate detection.
        if let Some(last) = mobile.last_access_activity {
            if activity_now.duration_since(last) > ACCESS_INACTIVITY_TIMEOUT {
                mobile.access_msg_seq_rcvd = [false; 8];
                debug!(
                    "BSC: cleared MSG_SEQ_RCVD for {} (inactive for {:?})",
                    format_ms_address(&mobile.fwd_address),
                    activity_now.duration_since(last),
                );
            }
        }

        if mark_reverse_regular_msg_seq_received(&mut mobile.access_msg_seq_rcvd, msg_seq) {
            AccessDuplicateDecision::Duplicate {
                addr: mobile.fwd_address.clone(),
                msg_seq,
            }
        } else {
            AccessDuplicateDecision::NewSdu
        }
    }

    pub(crate) fn apply_registration(
        &mut self,
        bsc: &mut Bsc,
        event: &AccessChannelEvent,
        update: AccessRegistrationUpdate,
    ) -> cdma_common::lac::paging_messages::MsAddress {
        let registration_imsi = update.registration_imsi.clone();
        let outcome = bsc.mobiles.apply_access_registration(event, update);
        // Notify the MSC on *every* event that flows through here
        // (Registration, Origination, PageResponse, Reconnect, …) so the
        // MSC's `mobiles_seen` table refreshes `last_seen_at` on real
        // contact, not just on explicit Registration Messages. The MSC
        // gates welcome SMS on `is_new || elapsed > threshold`, so this
        // does not multiply welcome traffic.
        bsc.notify_msc_registration(&outcome.fwd_address, registration_imsi);
        outcome.fwd_address
    }

    pub(crate) async fn handle_access_event(&mut self, bsc: &mut Bsc, event: AccessChannelEvent) {
        // Traffic channel events are routed separately
        if let Some(walsh_code) = event.traffic_walsh_code {
            bsc.handle_traffic_event(walsh_code, &event).await;
            return;
        }

        let addr = event.address.as_deref().unwrap_or("none");
        let l3 = event.l3_summary.as_deref().unwrap_or("unknown");
        let rx_age_us = event
            .rx_wall_time
            .map(|t| t.elapsed().as_micros() as u64)
            .unwrap_or(0);
        let now_utc = chrono::Utc::now();
        let air_age_us = event
            .receive_time
            .and_then(|t| (now_utc - t).num_microseconds());
        let t56_margin_us = event
            .receive_time
            .and_then(|t| (t + ChronoDuration::milliseconds(200) - now_utc).num_microseconds());
        let receive_chip = event
            .receive_time
            .map(|t| cdma_common::time::chips_since_epoch(t, SR1_CHIP_RATE_HZ));
        info!(
            "BSC: [access] msg chip={} abs_chip={:?} receive_chip={:?} type=\"{}\" pd={} preamble={} addr=[{}] l3=[{}] rx_age_us={} air_age_us={:?} t56_margin_us={:?} rx_hw_time_ns={}",
            event.chip_start,
            event.absolute_chip_start,
            receive_chip,
            event.msg_type_name,
            event.pd,
            event.preamble_frames,
            addr,
            l3,
            rx_age_us,
            air_age_us,
            t56_margin_us,
            event.rx_hw_time_ns.unwrap_or(0),
        );
        if let Some(margin_us) = t56_margin_us
            && margin_us < 0
            && event.ack_req
        {
            warn!(
                "BSC: access response already past T56m before enqueue: msg chip={} type=\"{}\" air_age_us={:?} late_by_us={}",
                event.chip_start, event.msg_type_name, air_age_us, -margin_us,
            );
        }

        let fwd_address = bsc.extract_fwd_address(&event);
        let known_mobile = fwd_address
            .as_ref()
            .filter(|addr| bsc.mobiles.get(addr).is_some())
            .cloned();
        let activity_now = Instant::now();

        // Per C.S0004-E 3.1.1.2.2.2: duplicate detection on the r-csch.
        // If the mobile is already known and the MSG_SEQ has been seen,
        // send an L2 acknowledgment only (BS Ack Order) and discard the
        // duplicate SDU — do NOT re-deliver to Layer 3 or re-send the
        // original L3 response. The original response has its own MSG_SEQ
        // and retransmission lifecycle on the f-csch.
        if let Some(addr) = known_mobile {
            let duplicate_decision = bsc
                .mobiles
                .update(&addr, |ms| {
                    self.handle_known_mobile_msg_seq(ms, &event, activity_now)
                })
                .unwrap_or(AccessDuplicateDecision::NewSdu);
            bsc.record_mobile_activity(&addr, &event, activity_now);

            match duplicate_decision {
                AccessDuplicateDecision::Duplicate { addr, msg_seq } => {
                    info!(
                        "BSC: duplicate access PDU (msg_seq={}) from [{}] — L2 ack only, SDU discarded",
                        msg_seq,
                        format_ms_address(&addr),
                    );
                    if event.ack_req {
                        if let Err(e) = bsc.access_tx.send_bs_ack_order(
                            &addr,
                            msg_seq,
                            access_response_tx_time(&event),
                            bsc.access_ack_deadline(&event),
                        ) {
                            warn!("BSC: failed to send BS Ack for duplicate access PDU: {}", e);
                        }
                    }
                    return;
                }
                AccessDuplicateDecision::NewSdu => {}
            }
            // For new mobiles (mobile_idx=None), the MSG_SEQ will be marked
            // after registration creates the MobileStation entry — see below.
        }

        match event.message_id {
            MessageId::Registration => {
                self.handle_registration(bsc, &event);
                bsc.publish_mobiles();
            }
            MessageId::Order => {
                self.handle_order(bsc, &event);
            }
            MessageId::DataBurst => {
                self.handle_data_burst(bsc, &event);
            }
            MessageId::Origination => {
                // Per C.S0005-E 2.6.5.1.8 / 3.6.5.1: the base station should
                // treat an Origination Message as an implicit registration.
                self.ensure_registered(bsc, &event);
                self.handle_origination(bsc, &event).await;
                bsc.publish_mobiles();
            }
            MessageId::PageResponse => {
                // Per C.S0005-E 2.6.5.1.8 / 3.6.5.1: the base station should
                // treat a Page Response Message as an implicit registration.
                self.ensure_registered(bsc, &event);
                self.handle_page_response(bsc, &event).await;
                bsc.publish_mobiles();
            }
            MessageId::Reconnect => {
                // Per C.S0005-E 2.6.5.1.8 / 3.6.5.1: the base station should
                // treat a Reconnect Message as an implicit registration. ORIG_IND
                // selects whether the message replaces Origination or Page Response.
                self.ensure_registered(bsc, &event);
                let orig_ind = matches!(
                    event.decoded_l3.as_ref(),
                    Some(AccessMessage::Reconnect(msg)) if msg.orig_ind
                );
                if orig_ind {
                    self.handle_origination(bsc, &event).await;
                } else {
                    self.handle_page_response(bsc, &event).await;
                }
                bsc.publish_mobiles();
            }
            MessageId::CallRecoveryRequest => {
                // C.S0005-E 3.6.3.5 handles Call Recovery Request in the same
                // access-response family as Origination.
                self.ensure_registered(bsc, &event);
                self.handle_origination(bsc, &event).await;
                bsc.publish_mobiles();
            }
            _ => {}
        }
    }

    /// Ensure a mobile is registered (implicit registration per C.S0005-E 2.6.5.1.8).
    /// Called for Origination, Page Response, Reconnect, and Call Recovery Request.
    pub(crate) fn ensure_registered(&mut self, bsc: &mut Bsc, event: &AccessChannelEvent) {
        let fwd_address = match bsc.extract_fwd_address(event) {
            Some(addr) => addr,
            None => {
                warn!(
                    "BSC: implicit registration from unknown address (no ESN or IMSI_S), ignoring"
                );
                return;
            }
        };

        let mob_p_rev = event.mob_p_rev.unwrap_or(6);
        let last_msg_seq = event.msg_seq.unwrap_or(0);
        let slot_cycle_index = event.slot_cycle_index.unwrap_or(0);
        let pgslot = compute_pgslot_from_event(event);

        let esp = &bsc
            .config
            .paging
            .message_defaults
            .extended_system_parameters;
        let (imsi_mcc, imsi_11_12) = resolve_imsi_overhead(event, esp.mcc, esp.imsi_11_12);
        let activity_now = Instant::now();
        let last_heard_ms = event_last_heard_ms(event);
        let registration_imsi = bsc.derive_registration_imsi(event);

        self.apply_registration(
            bsc,
            event,
            AccessRegistrationUpdate {
                fwd_address,
                registration_imsi,
                imsi_mcc,
                imsi_11_12,
                mob_p_rev,
                last_msg_seq,
                slot_cycle_index,
                pgslot,
                activity_now,
                last_heard_ms,
                explicit_registration: false,
            },
        );
    }

    pub(crate) fn handle_registration(&mut self, bsc: &mut Bsc, event: &AccessChannelEvent) {
        let fwd_address = match bsc.extract_fwd_address(event) {
            Some(addr) => addr,
            None => {
                warn!("BSC: registration from unknown address (no ESN or IMSI_S), ignoring");
                return;
            }
        };

        let esp = &bsc
            .config
            .paging
            .message_defaults
            .extended_system_parameters;
        let (imsi_mcc, imsi_11_12) = resolve_imsi_overhead(event, esp.mcc, esp.imsi_11_12);
        let mob_p_rev = event.mob_p_rev.unwrap_or(6);
        let last_msg_seq = event.msg_seq.unwrap_or(0);
        let slot_cycle_index = event.slot_cycle_index.unwrap_or(0);
        let pgslot = compute_pgslot_from_event(event);

        info!(
            "BSC: registering mobile addr={} mob_p_rev={} pgslot={:?} slot_cycle_index={}",
            format_ms_address(&fwd_address),
            mob_p_rev,
            pgslot,
            slot_cycle_index,
        );

        let activity_now = Instant::now();
        let last_heard_ms = event_last_heard_ms(event);
        let registration_imsi = bsc.derive_registration_imsi(event);

        self.apply_registration(
            bsc,
            event,
            AccessRegistrationUpdate {
                fwd_address: fwd_address.clone(),
                registration_imsi,
                imsi_mcc,
                imsi_11_12,
                mob_p_rev,
                last_msg_seq,
                slot_cycle_index,
                pgslot,
                activity_now,
                last_heard_ms,
                explicit_registration: true,
            },
        );

        // Resolve subscriber via HLR (async, fire-and-forget)
        bsc.resolve_subscriber_from_hlr(event, &fwd_address);

        // Send Registration Accepted Order with ARQ ack piggybacked
        if let Err(e) = bsc.access_tx.send_registration_accepted(
            &fwd_address,
            last_msg_seq,
            access_response_tx_time(event),
            bsc.access_ack_deadline(event),
        ) {
            warn!("BSC: failed to send registration accepted: {}", e);
            return;
        }

        // Per C.S0005-E §3.6.3.6: Registration → Registration Accepted/Rejected
        // Order or Service Redirection only. No channel assignment.
        // Voice page consumption is handled exclusively by handle_page_response()
        // per §3.6.3.3.
        bsc.try_deliver_pending_sms_from_access(event, &fwd_address, "Registration");
    }

    pub(crate) fn handle_order(&mut self, bsc: &mut Bsc, event: &AccessChannelEvent) {
        let fwd_address = match bsc.extract_fwd_address(event) {
            Some(addr) => addr,
            None => return,
        };

        info!(
            "BSC: received Order from {} ack_req={} valid_ack={} ack_seq={:?} msg_seq={:?}",
            format_ms_address(&fwd_address),
            event.ack_req,
            event.valid_ack,
            event.ack_seq,
            event.msg_seq,
        );

        const MS_ACK_ORDER: u8 = 0b010000;
        if event.order_code == Some(MS_ACK_ORDER) {
            let last_msg_seq = event.msg_seq.unwrap_or(0);
            if event.ack_req {
                if let Err(e) = bsc.access_tx.send_bs_ack_order(
                    &fwd_address,
                    last_msg_seq,
                    access_response_tx_time(event),
                    bsc.access_ack_deadline(event),
                ) {
                    warn!("BSC: failed to send BS Ack for MS Ack Order: {}", e);
                }
            }
            return;
        }

        // Other access-channel Orders have no Layer 3 response here, but
        // still need L2 acknowledgment when the mobile requested one.
        if event.ack_req {
            let last_msg_seq = event.msg_seq.unwrap_or(0);
            if let Err(e) = bsc.access_tx.send_bs_ack_order(
                &fwd_address,
                last_msg_seq,
                access_response_tx_time(event),
                bsc.access_ack_deadline(event),
            ) {
                warn!("BSC: failed to send BS Ack for order: {}", e);
            }
            return;
        }

        // Per C.S0005-E §3.6.3.4: "No requirements" for other non-assured
        // Orders on the access channel.
        // Voice page consumption is handled exclusively by handle_page_response()
        // per §3.6.3.3.
        bsc.try_deliver_pending_sms_from_access(event, &fwd_address, "Order");
    }

    /// Handle a reverse Data Burst Message (MO SMS) on the access channel.
    pub(crate) fn handle_data_burst(&mut self, bsc: &mut Bsc, event: &AccessChannelEvent) {
        let burst_type = match event.burst_type {
            Some(bt) => bt,
            None => {
                warn!("BSC: Data Burst without burst_type, ignoring");
                return;
            }
        };
        let fields = match event.data_burst_fields.as_ref() {
            Some(f) => f,
            None => {
                warn!("BSC: Data Burst without payload, ignoring");
                return;
            }
        };

        // ACK the access message
        if event.ack_req {
            if let Some(ref fwd_address) = bsc.extract_fwd_address(event) {
                let ack_seq = event.msg_seq.unwrap_or(0);
                if let Err(e) = bsc.access_tx.send_bs_ack_order(
                    fwd_address,
                    ack_seq,
                    access_response_tx_time(event),
                    bsc.access_ack_deadline(event),
                ) {
                    warn!("BSC: failed to ACK Data Burst: {}", e);
                }
            }
        }

        if burst_type != 3 {
            info!(
                "BSC: received Data Burst burst_type={} (not SMS), ignoring",
                burst_type
            );
            return;
        }

        info!(
            "BSC: received MO SMS Data Burst ({} bytes) from {} on access channel",
            fields.len(),
            event.address.as_deref().unwrap_or("unknown")
        );

        let Some(fwd_addr) = bsc.extract_fwd_address(event) else {
            warn!("BSC: MO SMS dropped — originating MS has no forward address");
            return;
        };
        let Some((originating_number, originating_subscriber_id)) =
            bsc.mobiles.resolve_originating_sms_sender(&fwd_addr)
        else {
            warn!("BSC: MO SMS dropped — originating MS has no phone number in HLR");
            return;
        };

        let Some(sms) = bsc.decode_reverse_sms(
            fields,
            &originating_number,
            originating_subscriber_id,
            "access",
        ) else {
            return;
        };

        bsc.route_decoded_mo_sms(&sms, &fwd_addr, fields);
    }

    pub(crate) async fn handle_page_response(&mut self, bsc: &mut Bsc, event: &AccessChannelEvent) {
        let fwd_address = bsc.extract_fwd_address(event);
        info!(
            "BSC: received Page Response from {}",
            fwd_address
                .as_ref()
                .map(|a| format_ms_address(a))
                .unwrap_or_else(|| "unknown".to_string())
        );

        bsc.mobiles
            .mark_page_response_received(fwd_address.as_ref());

        // If there's a pending page, take it and continue the corresponding flow.
        // Otherwise, if the MS requested an ACK, send a standalone BS Ack Order
        // so the MS stops retransmitting.
        if let Some(pending) = bsc.paging.take_sms_page() {
            info!(
                "BSC: page response received after {} retries ({:.0}ms elapsed) — delivering SMS",
                pending.retry_count,
                pending.started_at.elapsed().as_millis(),
            );
            let addr = fwd_address.unwrap_or_else(|| pending.fwd_address.clone());
            let ack_msg_seq = event.msg_seq.unwrap_or(0);

            if let Err(e) = bsc.sms.send_access_data_burst(
                &bsc.access_tx,
                &addr,
                ack_msg_seq,
                &pending.sms,
                access_response_tx_time(event),
                bsc.access_ack_deadline(event),
            ) {
                warn!("BSC: failed to send SMS data burst: {}", e);
            }

            // Per C.S0005-E §2.6.3.3: after the BS sends its response
            // (Data Burst), the MS processes it and returns to Idle.
            // Reflect this on the BS side.
            bsc.mobiles.mark_registered(&addr);
        } else if let Some(pending) = bsc.paging.take_voice_page() {
            bsc.clear_pending_page_records_for(&pending.page_address);
            info!(
                "BSC: page response received after {} retries ({:.0}ms elapsed) — assigning voice traffic",
                pending.retry_count,
                pending.started_at.elapsed().as_millis(),
            );
            let addr = fwd_address.unwrap_or_else(|| pending.fwd_address.clone());
            if bsc.mobiles.get(&addr).is_none() {
                warn!(
                    "BSC: voice page response from {} but no registered mobile matched",
                    format_ms_address(&addr)
                );
                return;
            }
            if pending.a1_call_id.is_some() {
                if bsc.handle_mt_page_response(event, &pending, &addr) {
                    return;
                }
                warn!("BSC: A1 Paging Response path failed; call remains unassigned");
                return;
            }
            let ack_msg_seq = event.msg_seq.unwrap_or(0);
            if let Err(e) = bsc
                .allocate_voice_channel_for_mobile(
                    &addr,
                    pending.service_option,
                    ack_msg_seq,
                    access_response_tx_time(event),
                    bsc.access_ack_deadline(event),
                    Some(pending.session_id),
                    Some(pending.leg_role),
                    pending.a1_call_id,
                )
                .await
            {
                warn!(
                    "BSC: failed to assign voice traffic after page response for {}: {}",
                    format_ms_address(&addr),
                    e
                );
            }
        } else if event.ack_req {
            // No pending SMS or voice — MS sent a Page Response but the BS
            // has nothing to deliver. Send a BS Ack Order so the MS stops
            // retransmitting, then return it to Registered (Idle equivalent).
            if let Some(ref addr) = fwd_address {
                let ack_msg_seq = event.msg_seq.unwrap_or(0);
                info!(
                    "BSC: sending BS Ack Order for Page Response retransmission (ack_seq={})",
                    ack_msg_seq
                );
                if let Err(e) = bsc.access_tx.send_bs_ack_order(
                    addr,
                    ack_msg_seq,
                    access_response_tx_time(event),
                    bsc.access_ack_deadline(event),
                ) {
                    warn!("BSC: failed to send BS Ack for page response: {}", e);
                }
                bsc.mobiles.mark_registered(addr);
            }
        } else {
            // No pending page, no ack requested — still transition back to
            // Registered so the MS doesn't stay stuck in PageResponseReceived.
            if let Some(ref addr) = fwd_address {
                bsc.mobiles.mark_registered(addr);
            }
        }
    }
    pub(crate) async fn handle_origination(&mut self, bsc: &mut Bsc, event: &AccessChannelEvent) {
        let fwd_address = match bsc.extract_fwd_address(event) {
            Some(addr) => addr,
            None => return,
        };
        let last_msg_seq = event.msg_seq.unwrap_or(0);

        info!(
            "BSC: received Origination from {} (service_option={:?})",
            format_ms_address(&fwd_address),
            event.service_option
        );

        // Implicit registration: resolve HLR if phone_number not yet known
        let needs_hlr = bsc
            .mobiles
            .get(&fwd_address)
            .map(|ms| ms.phone_number.is_none())
            .unwrap_or(true);
        if needs_hlr {
            bsc.resolve_subscriber_from_hlr(event, &fwd_address);
        }

        // Update RC capabilities from the origination message (FCH capability record)
        bsc.mobiles
            .apply_origination_capabilities(&fwd_address, event);

        if bsc
            .try_assign_access_sms_traffic(&fwd_address, event, last_msg_seq)
            .await
        {
            return;
        }

        if bsc
            .try_assign_access_packet_data_traffic(&fwd_address, event, last_msg_seq)
            .await
        {
            return;
        }

        // Voice service options - assign traffic channel for voice call.
        'assign_voice_traffic: {
            let so = match event.service_option {
                Some(so) => so,
                None => break 'assign_voice_traffic,
            };

            if !is_supported_origination_service_option(so) {
                info!(
                    "BSC: rejecting unsupported Origination service option SO{} from {}",
                    so,
                    format_ms_address(&fwd_address)
                );
                if let Err(e) = bsc.access_tx.send_service_option_rejected_release(
                    &fwd_address,
                    last_msg_seq,
                    access_response_tx_time(event),
                    bsc.access_ack_deadline(event),
                ) {
                    warn!(
                        "BSC: failed to send service-option rejection Release Order for SO{}: {}",
                        so, e
                    );
                }
                return;
            }

            if !is_voice_origination_service_option(so) {
                break 'assign_voice_traffic;
            }

            if bsc.mobiles.get(&fwd_address).is_none() {
                break 'assign_voice_traffic;
            }

            let digits = bsc
                .decoded_origination(event)
                .map(|msg| bsc.format_origination_digits(msg))
                .unwrap_or_default();

            let session_id = bsc.start_msc_controlled_mo_session(&fwd_address, so, digits.clone());
            if let Err(e) = bsc.access_tx.send_bs_ack_order(
                &fwd_address,
                last_msg_seq,
                access_response_tx_time(event),
                bsc.access_ack_deadline(event),
            ) {
                warn!("BSC: failed to send BS Ack for MO A1 origination: {}", e);
            }
            let called_number = (!digits.is_empty()).then_some(digits.as_str());
            if bsc
                .send_complete_layer3_for_origination(
                    &fwd_address,
                    event,
                    so,
                    called_number,
                    session_id,
                    VoiceLegRole::Caller,
                )
                .await
                .is_some()
            {
                return;
            }
            warn!("BSC: failed to send CLI3 for MO call; clearing local MSC-controlled session");
            bsc.voice
                .retain_sessions(|session| session.id != session_id);
            return;
        }

        // Default: send BS Ack Order for other unsupported service options
        if let Err(e) = bsc.access_tx.send_bs_ack_order(
            &fwd_address,
            last_msg_seq,
            access_response_tx_time(event),
            bsc.access_ack_deadline(event),
        ) {
            warn!("BSC: failed to send BS Ack for origination: {}", e);
            return;
        }

        // Per C.S0005-E §3.6.3.5: Origination gets its own channel assignment
        // (MO call flow), but must NOT consume a pending voice *page*.
        // Voice page consumption is handled exclusively by handle_page_response()
        // per §3.6.3.3.
        bsc.try_deliver_pending_sms_from_access(event, &fwd_address, "Origination");
    }
}

fn is_supported_origination_service_option(so: u16) -> bool {
    matches!(so, 6) || is_packet_data_so(so) || is_voice_origination_service_option(so)
}

fn is_voice_origination_service_option(so: u16) -> bool {
    VoiceCodec::from_service_option(so).is_some()
}

impl Bsc {
    pub(crate) fn enrich_uplink_event(&self, mut event: AccessChannelEvent) -> AccessChannelEvent {
        let matched_mobile = if let Some(walsh_code) = event.traffic_walsh_code {
            self.mobiles.get_by_walsh(walsh_code)
        } else {
            self.extract_fwd_address(&event)
                .and_then(|addr| self.mobiles.get(&addr))
        };

        if let Some(ms) = matched_mobile {
            let resolved_address = format_ms_address(&ms.fwd_address);
            if event.address.is_none() {
                event.address = Some(resolved_address.clone());
            }
            event.resolved_address = Some(resolved_address);
            event.subscriber_id = event
                .subscriber_id
                .or_else(|| ms.subscriber_id.map(|id| id.to_string()));
            event.esn = event.esn.or(ms.esn);
            if let Some((s1, s2)) = ms.imsi_s_components() {
                event.imsi_m_s1 = event.imsi_m_s1.or(Some(s1));
                event.imsi_m_s2 = event.imsi_m_s2.or(Some(s2));
            }
            event.mob_p_rev = event.mob_p_rev.or(Some(ms.mob_p_rev));
            event.slot_cycle_index = event.slot_cycle_index.or(Some(ms.slot_cycle_index));
        } else if event.resolved_address.is_none() {
            event.resolved_address = self
                .extract_fwd_address(&event)
                .map(|addr| format_ms_address(&addr));
        }

        event
    }

    pub(crate) async fn handle_access_event(&mut self, event: AccessChannelEvent) {
        let mut access_service = std::mem::take(&mut self.access_service);
        access_service.handle_access_event(self, event).await;
        self.access_service = access_service;
    }
}

/// Convert an access/traffic event timestamp into the UI-facing last-heard time.
pub(crate) fn event_last_heard_ms(event: &AccessChannelEvent) -> u64 {
    event
        .receive_time
        .map(|t| t.timestamp_millis() as u64)
        .unwrap_or_else(|| chrono::Utc::now().timestamp_millis() as u64)
}

/// Compute PGSLOT from the IMSI fields in an access event, if available.
pub(crate) fn compute_pgslot_from_event(event: &AccessChannelEvent) -> Option<u16> {
    if let (Some(s1), Some(s2)) = (event.imsi_m_s1, event.imsi_m_s2) {
        Some(cdma_common::paging::compute_pgslot(s1, s2))
    } else {
        None
    }
}

impl Bsc {
    /// Build forward-link address from access event identity fields.
    ///
    /// Per C.S0005-E 2.6.2.2.5 and C.S0004-E 2.1.1.3.1.3: when a class-0
    /// mobile omits MCC or IMSI_11_12 from the access message, it means
    /// those values equal the current overhead parameters (MCC_S, IMSI_11_12_S).
    /// We resolve `None` → overhead so the forward address always contains
    /// the mobile's full identity.
    pub(crate) fn extract_fwd_address(&self, event: &AccessChannelEvent) -> Option<MsAddress> {
        if event.imsi_class == Some(0) {
            if let (Some(s1), Some(s2)) = (event.imsi_m_s1, event.imsi_m_s2) {
                let defaults = &self
                    .config
                    .paging
                    .message_defaults
                    .extended_system_parameters;
                return Some(select_imsi_class0_forward_address(
                    s1,
                    s2,
                    event.imsi_mcc,
                    event.imsi_11_12,
                    defaults.mcc,
                    defaults.imsi_11_12,
                ));
            }
            warn!("BSC: class-0 IMSI indicated but IMSI_S fields are missing for forward address");
        }

        if let Some(esn) = event.esn {
            Some(MsAddress::Esn(esn))
        } else if let (Some(s1), Some(s2)) = (event.imsi_m_s1, event.imsi_m_s2) {
            Some(MsAddress::ImsiS {
                imsi_m_s1: s1,
                imsi_m_s2: s2,
            })
        } else {
            None
        }
    }
}

/// Resolve MCC and IMSI_11_12 from access event fields, falling back
/// to overhead per C.S0005-E 2.6.2.2.5 when the mobile omits them.
/// When the event provides IMSI_S (any class), omitted MCC/IMSI_11_12
/// are assumed equal to overhead; this matches the behavior of
/// `extract_page_address`.
fn resolve_imsi_overhead(
    event: &AccessChannelEvent,
    overhead_mcc: u16,
    overhead_imsi_11_12: u8,
) -> (Option<u16>, Option<u8>) {
    if event.imsi_m_s1.is_some() && event.imsi_m_s2.is_some() {
        let mcc = Some(event.imsi_mcc.unwrap_or(overhead_mcc));
        let imsi_11_12 = Some(event.imsi_11_12.unwrap_or(overhead_imsi_11_12));
        (mcc, imsi_11_12)
    } else {
        (event.imsi_mcc, event.imsi_11_12)
    }
}

/// Build page address from access event fields, resolving omitted
/// MCC/IMSI_11_12 from overhead per C.S0005-E 2.6.2.2.5.
///
/// Used by the HLR binding path to extract an ESN fallback from
/// the page identity.
pub(crate) fn extract_page_address(
    event: &AccessChannelEvent,
    overhead_mcc: u16,
    overhead_imsi_11_12: u8,
) -> Option<MsPageAddress> {
    if event.imsi_class == Some(0) {
        if let (Some(s1), Some(s2)) = (event.imsi_m_s1, event.imsi_m_s2) {
            return Some(MsPageAddress::ImsiS {
                imsi_m_s1: s1,
                imsi_m_s2: s2,
                mcc: Some(event.imsi_mcc.unwrap_or(overhead_mcc)),
                imsi_11_12: Some(event.imsi_11_12.unwrap_or(overhead_imsi_11_12)),
            });
        }
        warn!("BSC: class-0 IMSI indicated but IMSI_S fields are missing for paging");
    }

    if event.imsi_class == Some(1) {
        // Class-1 type 1 provides IMSI_S + MCC + IMSI_11_12 — page by IMSI.
        // Class-1 type 0 also provides IMSI_S + IMSI_11_12 (MCC implied).
        // Prefer IMSI_S paging when available; fall back to ESN.
        if let (Some(s1), Some(s2)) = (event.imsi_m_s1, event.imsi_m_s2) {
            return Some(MsPageAddress::ImsiS {
                imsi_m_s1: s1,
                imsi_m_s2: s2,
                mcc: Some(event.imsi_mcc.unwrap_or(overhead_mcc)),
                imsi_11_12: Some(event.imsi_11_12.unwrap_or(overhead_imsi_11_12)),
            });
        }
        return event.esn.map(MsPageAddress::Esn).or_else(|| {
            warn!("BSC: class-1 IMSI indicated but neither IMSI_S nor ESN available for paging");
            None
        });
    }

    if let (Some(s1), Some(s2)) = (event.imsi_m_s1, event.imsi_m_s2) {
        Some(MsPageAddress::ImsiS {
            imsi_m_s1: s1,
            imsi_m_s2: s2,
            mcc: Some(event.imsi_mcc.unwrap_or(overhead_mcc)),
            imsi_11_12: Some(event.imsi_11_12.unwrap_or(overhead_imsi_11_12)),
        })
    } else if let Some(esn) = event.esn {
        Some(MsPageAddress::Esn(esn))
    } else {
        warn!("BSC: no ESN or IMSI_S for GPM page address, MS is unpageable via GPM");
        None
    }
}

/// Requested TX time for access-coupled responses.  Send as soon as
/// possible — T56m (200 ms) is the *maximum* allowed BS response time,
/// not a minimum delay.
pub(crate) fn access_response_tx_time(
    event: &AccessChannelEvent,
) -> Option<cdma_common::time::CdmaSystemTime> {
    event.receive_time
}

impl Bsc {
    /// Notify the MSC of a registration event by sending a
    /// `CompleteLayer3Information` carrying a `LocationUpdatingRequest` (no
    /// call_id). The MSC uses this to update `mobiles_seen` and trigger the
    /// welcome SMS path when appropriate. Called from `apply_registration`
    /// whenever a new mobile entry is created or whenever the mobile sent an
    /// explicit Registration Message; not called on repeat implicit
    /// registrations of an already-known mobile.
    pub(crate) fn notify_msc_registration(
        &self,
        fwd_address: &MsAddress,
        registration_imsi: Option<String>,
    ) {
        let esn_val = match fwd_address {
            MsAddress::Esn(e) => Some(*e),
            _ => None,
        };
        let a1_client = self.a1.msc_client.clone();
        let cell_id = cdma_ios::CellId {
            cell: self.config.overhead.base_id,
            sector: 0,
        };
        tokio::spawn(async move {
            let mobile_identity = registration_imsi
                .map(cdma_ios::MobileIdentity::Imsi)
                .or_else(|| esn_val.map(cdma_ios::MobileIdentity::Esn))
                .unwrap_or_else(|| cdma_ios::MobileIdentity::Imsi("UNKNOWN".to_string()));
            let lur = cdma_ios::LocationUpdatingRequestMessage {
                mobile_identity_imsi: mobile_identity,
                location_area_identification: None,
                classmark_information_type_2: None,
                registration_type: None,
                mobile_identity_esn: esn_val.map(cdma_ios::MobileIdentity::Esn),
                slot_cycle_index: None,
                authentication_response_parameter: None,
                authentication_confirmation_parameter: None,
                authentication_parameter_count: None,
                authentication_challenge_parameter: None,
                authentication_event: None,
                user_zone_id: None,
                is2000_mobile_capabilities: None,
            };
            let l3 = match cdma_ios::Layer3Information::from_location_updating_request(&lur) {
                Ok(l) => l,
                Err(e) => {
                    log::warn!("BSC: failed to encode LocationUpdatingRequest: {e}");
                    return;
                }
            };
            let cli3 = cdma_ios::CompleteLayer3InformationMessage {
                cell_identifier: cell_id,
                layer3_information: l3,
            };
            let payload = match cli3.encode() {
                Ok(p) => p,
                Err(e) => {
                    log::warn!("BSC: failed to encode registration CLI3: {e}");
                    return;
                }
            };
            let msg = cdma_ios::EncodedA1Message::from_message(&cdma_ios::Message::new(
                cdma_ios::MessageType::CompleteLayer3Information,
                payload,
            ));
            if let Err(e) = a1_client.send_a1(msg).await {
                log::warn!("BSC: failed to send registration notification to MSC: {e}");
            }
        });
    }

    pub(crate) fn access_ack_deadline(
        &self,
        event: &AccessChannelEvent,
    ) -> Option<cdma_common::time::CdmaSystemTime> {
        // T56m (200 ms) is the max time the MS monitors PCH after its probe.
        // Use the full window — any artificial margin just causes the ECAM to
        // be dropped before the MS stops listening, forcing unnecessary retries.
        const T56M_MS: i64 = 200;
        let deadline_ms = T56M_MS;
        Some(event.receive_time? + ChronoDuration::milliseconds(deadline_ms))
    }

    pub(crate) fn decoded_origination<'a>(
        &self,
        event: &'a AccessChannelEvent,
    ) -> Option<&'a OriginationMessage> {
        match event.decoded_l3.as_ref()? {
            AccessMessage::Origination(msg) => Some(msg),
            _ => None,
        }
    }

    pub(crate) fn format_origination_digits(&self, msg: &OriginationMessage) -> String {
        format_dtmf_digits(&msg.digits, msg.digit_mode)
    }

    /// Resolve subscriber identity via HLR and update the mobile's phone_number
    /// and subscriber_id. Also upserts the registration binding in the HLR.
    /// Spawns an async task; the result is sent back via `hlr_result_rx` and
    /// applied in the `run()` select loop.
    pub(crate) fn resolve_subscriber_from_hlr(
        &self,
        event: &AccessChannelEvent,
        fwd_address: &MsAddress,
    ) {
        let Some(ref hlr_repo) = self.config.hlr_repo else {
            return;
        };

        let esn = event.esn;
        let registration_imsi = self.derive_registration_imsi(event);
        if registration_imsi.is_none() && esn.is_none() {
            log::error!(
                "BSC: dropping registration — no resolvable identity \
                 (no IMSI, no S1/S2, no ESN; esn={:?}, imsi_class={:?})",
                event.esn,
                event.imsi_class,
            );
            return;
        }
        let mob_p_rev_raw = event.mob_p_rev.unwrap_or(6);
        let mob_p_rev = mob_p_rev_raw as u32;
        let last_msg_seq = event.msg_seq.unwrap_or(0) as u32;
        let slot_cycle_index = event.slot_cycle_index.unwrap_or(0) as u32;
        let pgslot = compute_pgslot_from_event(event).map(|v| v as u32);
        let fwd_addr = fwd_address.clone();
        let esp = &self
            .config
            .paging
            .message_defaults
            .extended_system_parameters;
        let page_addr = extract_page_address(event, esp.mcc, esp.imsi_11_12);
        let repo = hlr_repo.clone();
        let result_tx = self.hlr_result_tx.clone();
        let node_id = self.config.node_id.clone();

        tokio::spawn(async move {
            let result = repo
                .resolve_by_identity(esn, registration_imsi.as_deref())
                .await;

            let resolution = match result {
                Ok(Some(subscriber)) => {
                    let sub_id = subscriber.subscriber_id;
                    let phone = subscriber.phone_number.clone();
                    let display_name = subscriber.display_name.clone();
                    info!(
                        "BSC: HLR resolved {} → subscriber {} ({})",
                        format_ms_address(&fwd_addr),
                        sub_id,
                        phone,
                    );

                    let binding_esn = esn.or_else(|| match &fwd_addr {
                        MsAddress::Esn(e) => Some(*e),
                        _ => match page_addr {
                            Some(MsPageAddress::Esn(e)) => Some(e),
                            _ => None,
                        },
                    });

                    let binding = cdma_hlr::model::RegistrationBinding {
                        subscriber_id: sub_id,
                        serving_node_id: node_id.clone(),
                        state: cdma_hlr::model::RegistrationState::Registered,
                        imsi: registration_imsi.clone(),
                        esn: binding_esn,
                        mob_p_rev: Some(mob_p_rev),
                        pgslot,
                        slot_cycle_index: Some(slot_cycle_index),
                        last_msg_seq: Some(last_msg_seq),
                        last_registered_at: chrono::Utc::now(),
                        last_seen_at: chrono::Utc::now(),
                        updated_at: chrono::Utc::now(),
                    };

                    if let Err(e) = repo.upsert_registration_binding(binding.clone()).await {
                        warn!("BSC: HLR upsert_registration_binding failed: {}", e);
                    }

                    let canonical_imsi = match repo.get_identities_for_subscriber(sub_id).await {
                        Ok(identities) => identities
                            .iter()
                            .find(|identity| identity.is_primary)
                            .and_then(|identity| identity.imsi.clone()),
                        Err(e) => {
                            warn!("BSC: HLR get_identities_for_subscriber failed: {}", e);
                            None
                        }
                    };

                    HlrResolution {
                        fwd_address: fwd_addr,
                        subscriber_id: Some(sub_id),
                        phone_number: Some(phone),
                        display_name: Some(display_name),
                        canonical_imsi,
                    }
                }
                Ok(None) => {
                    debug!(
                        "BSC: HLR has no subscriber for {}",
                        format_ms_address(&fwd_addr),
                    );
                    HlrResolution {
                        fwd_address: fwd_addr,
                        subscriber_id: None,
                        phone_number: None,
                        display_name: None,
                        canonical_imsi: None,
                    }
                }
                Err(e) => {
                    warn!("BSC: HLR resolve_by_identity failed: {}", e);
                    HlrResolution {
                        fwd_address: fwd_addr,
                        subscriber_id: None,
                        phone_number: None,
                        display_name: None,
                        canonical_imsi: None,
                    }
                }
            };

            let _ = result_tx.send(resolution).await;
        });
    }

    /// Apply an HLR resolution result to the registered mobile.
    pub(crate) fn apply_hlr_resolution(&mut self, resolution: HlrResolution) {
        if let (Some(sub_id), Some(phone)) = (resolution.subscriber_id, resolution.phone_number) {
            if self.mobiles.apply_subscriber_resolution(
                &resolution.fwd_address,
                sub_id,
                phone,
                resolution.display_name,
                resolution.canonical_imsi,
            ) {
                self.publish_mobiles();
            }
        }
    }

    pub(crate) fn restore_pending_page(&mut self, mut pending: PendingPage) {
        self.mobiles.mark_page_pending(&pending.fwd_address);
        pending.next_retry_at = self.compute_next_retry_at(
            pending.pgslot,
            pending.slot_cycle_index,
            pending.last_target_chip,
        );
        self.paging.restore_sms_page(pending);
    }

    /// If the MS we were paging is already active on the access channel for a
    /// different reason, stop paging retries and try to deliver the pending SMS
    /// directly after the normal response to that access message has been queued.
    pub(crate) fn try_deliver_pending_sms_from_access(
        &mut self,
        event: &AccessChannelEvent,
        fwd_address: &MsAddress,
        trigger_name: &str,
    ) -> bool {
        let Some(pending) = self.paging.take_matching_sms_page_for_access(fwd_address) else {
            return false;
        };
        let removed_pending = self.clear_pending_page_records_for(&pending.page_address);
        self.mobiles.mark_registered(fwd_address);

        info!(
            "BSC: {} from {} while page pending after {} retries ({:.0}ms elapsed) — cancelling page and delivering SMS",
            trigger_name,
            format_ms_address(fwd_address),
            pending.retry_count,
            pending.started_at.elapsed().as_millis(),
        );
        if removed_pending > 0 {
            info!(
                "BSC: removed {} pending page record(s) for {} before direct SMS delivery",
                removed_pending,
                format_ms_address(fwd_address),
            );
        }
        let ack_msg_seq = event.msg_seq.unwrap_or(0);
        if let Err(e) = self.sms.send_access_data_burst(
            &self.access_tx,
            fwd_address,
            ack_msg_seq,
            &pending.sms,
            access_response_tx_time(event),
            self.access_ack_deadline(event),
        ) {
            warn!(
                "BSC: failed to send SMS data burst after {} from {}: {}",
                trigger_name,
                format_ms_address(fwd_address),
                e,
            );
            self.restore_pending_page(pending);
        }
        true
    }
}
