//! Forward traffic-channel signaling: ARQ, retries, teardown, and
//! L3 frame transmission.
//!
//! WS-0 PR3 sibling module per
//! `docs/architecture-update/09-pr3-method-map.md`.

use std::time::Instant;

use cdma_common::access::AccessMessage;
use cdma_common::error::Error;
use cdma_common::events::AccessChannelEvent;
use cdma_common::formatting::reverse_order_name;
use cdma_common::lac::{
    message_types::MessageId,
    paging_messages::{MsAddress, OrderMessage},
};
use cdma_voice::VoiceCodec;
use log::{debug, info, warn};

use crate::addressing::{format_ms_address, is_packet_data_so};
use crate::power_control::ForwardPowerControlState;

use super::{A1ClearState, Bsc, MsState, ServiceNegotiationMode, VoiceLegRole};

#[derive(Default)]
pub(crate) struct TrafficSignalingService {
    pub(crate) serv_con_seq: u8,
}

/// State the BSC tracks per walsh for the most recent OTASP ADDS
/// Deliver that's still awaiting either an L2 ack from the MS (success)
/// or an L3 Mobile Station Reject Order (failure). When either arrives,
/// the BSC sends an `AddsDeliverAck` to MSC per A.S0001 §6.1.7.5.
#[derive(Debug, Clone)]
pub(crate) struct PendingOtaspDbm {
    /// Tag the MSC put on the original ADDS Deliver. The BSC echoes
    /// this on the AddsDeliverAck so MSC can correlate.
    pub a1_tag: cdma_ios::Tag,
}

/// A1 cause codes the BSC reports on `AddsDeliverAck` failures.
/// Wire-defined per A.S0014; concrete values are operator-visible only
/// via the OTASP session's `BlockSkipped` reason text — the MSC
/// session driver treats any non-`None` cause as "current phase
/// failed, advance to next plan step".
/// Reverse-link Order codes the BSC reacts to on R-TCH. Six-bit ORDER
/// field per C.S0005-E Table 3.7.2.3.2.1-4.
pub(crate) mod reverse_order_code {
    /// Connect Order — MS confirms entering Conversation state.
    pub const CONNECT: u8 = 0b011000;
    /// Mobile Station Reject Order — C.S0005-E Table 2.7.3-1, with
    /// ORDQ + REJECTED_TYPE carrying the upper-layer reason.
    pub const MOBILE_STATION_REJECT: u8 = 0b011111;
    /// Service Option Response Order — MS reply to a BS-initiated
    /// Service Option Request Order (C.S0005-E §2.6.4 / §3.7.4.3).
    pub const SERVICE_OPTION_RESPONSE: u8 = 0b010100;
}

/// Forward-link Order codes the BSC emits on F-TCH. Six-bit ORDER field
/// per C.S0005-E Table 2.7.3-1.
pub(crate) mod forward_order_code {
    /// Service Option Request Order — BS proposes a service option to
    /// the MS (C.S0005-E §3.6.4).
    pub const SERVICE_OPTION_REQUEST: u8 = 0b010011;
    /// Service Option Response Order — BS reply accepting or rejecting
    /// the MS's requested service option (C.S0005-E §3.7.4.3).
    pub const SERVICE_OPTION_RESPONSE: u8 = 0b010100;
}

pub(crate) mod adds_deliver_ack_cause {
    /// MS rejected the application data delivery at L3 (Mobile Station
    /// Reject Order with `REJECTED_TYPE = 0x04 Data Burst Message`).
    /// Closest A.S0014 code: "Radio interface message failure" (0x00).
    pub const RADIO_INTERFACE_MESSAGE_FAILURE: u8 = 0x00;
    /// Forward traffic channel was torn down before the MS L2-acked
    /// the deliver (mobile dropped the call, BSC released the TCH).
    /// A.S0014 "Mobile call cleared" maps cleanly to this case.
    pub const CALL_CLEARED: u8 = 0x0F;
}

impl TrafficSignalingService {
    pub(crate) fn next_serv_con_seq(&mut self) -> u8 {
        let seq = self.serv_con_seq;
        self.serv_con_seq = (self.serv_con_seq + 1) & 0x07;
        seq
    }
}

pub(crate) fn mark_reverse_regular_msg_seq_received(
    msg_seq_rcvd: &mut [bool; 8],
    msg_seq: u8,
) -> bool {
    let seq = (msg_seq & 0b111) as usize;
    if msg_seq_rcvd[seq] {
        return true;
    }

    msg_seq_rcvd[seq] = true;
    msg_seq_rcvd[(seq + 4) % 8] = false;
    false
}

impl Bsc {
    /// Process a reverse-link ACK_SEQ acknowledging a forward traffic PDU.
    pub(crate) fn acknowledge_traffic_forward_pdu(
        &mut self,
        fwd_address: &cdma_common::lac::paging_messages::MsAddress,
        ack_seq: u8,
    ) {
        let key = super::SmsAckKey::TrafficMsgSeq {
            addr: fwd_address.clone(),
            msg_seq: ack_seq,
        };
        if let Some(pending) = self.sms.complete_delivery(&key) {
            if let Some(a1_tag) = pending.a1_tag {
                let client = self.a1.msc_client.clone();
                tokio::spawn(async move {
                    let ack_msg = cdma_ios::AddsDeliverAckMessage {
                        tag: Some(cdma_ios::Tag(a1_tag)),
                        cause: None,
                    };
                    match ack_msg.encode() {
                        Ok(payload) => {
                            let msg =
                                cdma_ios::EncodedA1Message::from_message(&cdma_ios::Message::new(
                                    cdma_ios::MessageType::AddsDeliverAck,
                                    payload,
                                ));
                            if let Err(e) = client.send_a1(msg).await {
                                log::warn!("BSC: failed to send ADDS Deliver Ack to MSC: {e}");
                            }
                        }
                        Err(e) => log::warn!("BSC: failed to encode ADDS Deliver Ack: {e}"),
                    }
                });
            }
            let addr = pending.addr.clone();
            self.dispatch_next_queued_sms_for(&addr);
        }
    }

    /// Returns `true` if a voice add-on attempt was in progress on this
    /// TCH and we kicked off the release-and-signal-failure path. The A1
    /// AssignmentFailure is emitted at the end of `teardown_traffic_channel`.
    pub(crate) fn release_tch_and_signal_assignment_failure(
        &mut self,
        walsh_code: u8,
        fwd_address: &MsAddress,
        reason: &str,
    ) -> bool {
        let Some(tc) = self.mobiles.get_traffic_channel(walsh_code) else {
            return false;
        };
        if tc.voice_service_option.is_none() {
            return false;
        }
        let Some(call_id) = tc.a1_call_id else {
            return false;
        };
        self.pending_a1_failure_after_release
            .retain(|(addr, _)| addr != fwd_address);
        self.pending_a1_failure_after_release.push((
            fwd_address.clone(),
            super::PendingAssignmentFailure {
                call_id,
                queued_at: Instant::now(),
            },
        ));
        // Strip voice add-on so teardown doesn't fire on_voice_leg_released.
        self.mobiles.update_tc(walsh_code, |_, tc| {
            tc.clear_voice_service_connection();
        });
        info!(
            "BSC: stashed A1 AssignmentFailure for {} call_id={} ({}); releasing walsh={}",
            format_ms_address(fwd_address),
            call_id,
            reason,
            walsh_code,
        );
        self.release_tch_for_assignment_failure(walsh_code, reason);
        true
    }

    pub(crate) async fn advance_waiting_ms_ack(
        &mut self,
        walsh_code: u8,
        ack_seq: u8,
        trigger_msg_type: &str,
    ) {
        let Some(ms) = self.mobiles.get_by_walsh(walsh_code) else {
            return;
        };
        let waiting_ms_ack = ms
            .find_traffic_channel_by_walsh(walsh_code)
            .is_some_and(|tc| tc.is_waiting_ms_ack());
        if !waiting_ms_ack {
            return;
        }

        let addr = ms.fwd_address.clone();
        info!(
            "BSC: BS Ack acknowledged on R-TCH walsh={} by {} for {}",
            walsh_code,
            trigger_msg_type,
            format_ms_address(&addr)
        );

        let Some((service_negotiation_mode, service_option, origination_service_option)) =
            self.mobiles.get_traffic_channel(walsh_code).map(|tc| {
                (
                    tc.service_negotiation_mode,
                    tc.service_option,
                    tc.origination_service_option,
                )
            })
        else {
            return;
        };

        let needs_negotiation =
            origination_service_option.is_some_and(|orig_so| orig_so != service_option);

        if service_negotiation_mode == ServiceNegotiationMode::ServiceOptionNegotiation {
            if needs_negotiation {
                info!(
                    "BSC: SERV_NEG disabled on walsh={}: origination SO={:?} differs from assigned SO={}; sending Service Option Request Order proposing SO{}",
                    walsh_code, origination_service_option, service_option, service_option
                );
                if let Err(e) =
                    self.send_service_option_request_order(walsh_code, ack_seq, service_option)
                {
                    warn!(
                        "BSC: failed to send Service Option Request Order on walsh={}: {}",
                        walsh_code, e
                    );
                    self.teardown_traffic_channel(walsh_code).await;
                    return;
                }
                self.mobiles.update_tc(walsh_code, |_, tc| {
                    tc.mark_waiting_service_response();
                });
            } else {
                info!(
                    "BSC: SERV_NEG disabled on walsh={}; accepting SO{} with Service Option Response Order",
                    walsh_code, service_option
                );
                if let Err(e) =
                    self.send_service_option_response_order(walsh_code, ack_seq, service_option)
                {
                    warn!(
                        "BSC: failed to send Service Option Response Order on walsh={}: {}",
                        walsh_code, e
                    );
                    return;
                }
                self.complete_service_negotiation(
                    walsh_code,
                    &addr,
                    "Service Option Response Order with SERV_NEG disabled",
                )
                .await;
            }
        } else if needs_negotiation {
            if let Err(e) = self.send_service_request(walsh_code, ack_seq) {
                warn!(
                    "BSC: failed to send Service Request on walsh={}: {}",
                    walsh_code, e
                );
            } else {
                self.mobiles.update_tc(walsh_code, |_, tc| {
                    tc.mark_waiting_service_response();
                });
            }
        } else if let Err(e) = self.send_service_connect(walsh_code, ack_seq) {
            warn!(
                "BSC: failed to send Service Connect on walsh={}: {}",
                walsh_code, e
            );
        } else {
            self.mobiles.update_tc(walsh_code, |_, tc| {
                tc.mark_service_connecting();
            });
        }
    }

    fn send_service_option_response_order(
        &mut self,
        walsh_code: u8,
        ack_seq: u8,
        service_option: u16,
    ) -> Result<(), Error> {
        let order_msg = OrderMessage {
            order: forward_order_code::SERVICE_OPTION_RESPONSE,
            ordq: 0,
            order_specific_fields: service_option.to_be_bytes().to_vec(),
        };
        let sdu = order_msg.to_ftch_sdu();

        self.send_traffic_signaling(
            walsh_code,
            sdu,
            MessageId::Order,
            ack_seq,
            true,
            Some(order_msg),
            None,
            None,
            None,
            None,
        )
    }

    /// BS-side Service Option Request Order on F-TCH (C.S0005-E §3.6.4).
    /// The MS replies with a Service Option Response Order accepting or
    /// rejecting.
    fn send_service_option_request_order(
        &mut self,
        walsh_code: u8,
        ack_seq: u8,
        service_option: u16,
    ) -> Result<(), Error> {
        let order_msg = OrderMessage {
            order: forward_order_code::SERVICE_OPTION_REQUEST,
            ordq: 0,
            order_specific_fields: service_option.to_be_bytes().to_vec(),
        };
        let sdu = order_msg.to_ftch_sdu();

        self.send_traffic_signaling(
            walsh_code,
            sdu,
            MessageId::Order,
            ack_seq,
            true,
            Some(order_msg),
            None,
            None,
            None,
            None,
        )
    }

    async fn complete_service_negotiation(
        &mut self,
        walsh_code: u8,
        addr: &MsAddress,
        trigger: &str,
    ) {
        let is_voice = self
            .mobiles
            .get_traffic_channel(walsh_code)
            .and_then(super::traffic_forward::voice_service_option_for_channel)
            .is_some();
        if is_voice {
            let replaced_packet_session = self.replace_packet_service_with_voice(walsh_code);
            if let Some(packet_session_id) = replaced_packet_session {
                self.close_packet_session_background(walsh_code, packet_session_id);
            }
            let (a1_call_id, should_send_assignment_complete, voice_service_option) = self
                .mobiles
                .get_traffic_channel(walsh_code)
                .map(|tc| {
                    (
                        tc.a1_call_id,
                        tc.is_service_connecting() || tc.is_waiting_ms_ack(),
                        super::traffic_forward::voice_service_option_for_channel(tc)
                            .unwrap_or(tc.service_option),
                    )
                })
                .unwrap_or((None, false, 3));
            if should_send_assignment_complete && let Some(call_id) = a1_call_id {
                self.a1.send_assignment_complete(
                    &self.mobiles,
                    call_id,
                    walsh_code,
                    voice_service_option,
                );
            }
            info!(
                "BSC: voice service negotiation complete on walsh={} after {}",
                walsh_code, trigger
            );
            if let Some(tc) = self.mobiles.get_traffic_channel_mut(walsh_code) {
                tc.mark_active();
            }
        } else {
            let is_packet_data = self
                .mobiles
                .get_traffic_channel(walsh_code)
                .map_or(false, |tc| is_packet_data_so(tc.service_option));
            if is_packet_data {
                info!(
                    "BSC: packet-data service accepted on walsh={} for {} after {}",
                    walsh_code,
                    format_ms_address(addr),
                    trigger
                );
            }
            if let Some(tc) = self.mobiles.get_traffic_channel_mut(walsh_code) {
                tc.mark_active();
            }
            if is_packet_data {
                self.start_packet_session_after_service_connect(walsh_code)
                    .await;
            }
            // SO6 escalation: an SMS was parked on this channel after the BTS
            // rejected the original F-PCH attempt with cause 0x71. Re-deliver
            // it now on F-DSCH.
            self.dispatch_pending_sms_escalation(walsh_code, addr);
        }
    }

    /// Take any SMS parked on `walsh_code` by the 0x71-escalation path and
    /// re-send it on F-DSCH. The F-TCH ack tracker is installed by
    /// `send_sms_data_burst_auto` so the existing TrafficMsgSeq ack path
    /// relays success/failure to the MSC.
    fn dispatch_pending_sms_escalation(&mut self, walsh_code: u8, addr: &MsAddress) {
        let Some(parked) = self.pending_sms_escalations.remove(&walsh_code) else {
            return;
        };
        let Some(payload) = parked.escalation.clone() else {
            warn!(
                "BSC: parked escalation on walsh={} has no payload, dropping",
                walsh_code
            );
            return;
        };
        let sms_req = super::sms::SmsRequest {
            originating_number: payload.originating_number,
            text: payload.text,
            target_address: None,
            target_subscriber_id: payload.target_subscriber_id,
            timeout_ms: payload.timeout_ms,
            destination_number: payload.destination_number,
            sms_id: parked.sms_id,
            delivery_attempt_id: parked.delivery_attempt_id,
            a1_tag: parked.a1_tag,
            raw_payload: payload.raw_payload,
        };
        info!(
            "BSC: 0x71 escalation: re-delivering SMS {:?} on F-DSCH walsh={} for {}",
            sms_req.sms_id,
            walsh_code,
            format_ms_address(addr)
        );
        match self.send_sms_data_burst_auto(addr, &sms_req) {
            Ok(super::ForwardSignalingRoute::SentOnTraffic { .. }) => {}
            Ok(super::ForwardSignalingRoute::NeedsPaging) => {
                warn!(
                    "BSC: 0x71 escalation: walsh={} not active when dispatching parked SMS",
                    walsh_code
                );
            }
            Err(e) => warn!(
                "BSC: 0x71 escalation: F-DSCH re-delivery failed on walsh={}: {}",
                walsh_code, e
            ),
        }
    }

    /// Handle an event from the reverse traffic channel.
    ///
    /// Traffic channel events arrive via the same `AccessChannelEvent` channel
    /// but with `traffic_walsh_code` set. The BSC matches the Walsh code to a
    /// mobile in `TrafficAssigning` or `TrafficActive` state and processes the
    /// decoded LAC PDU (Data Burst for SMS, Order for signaling, etc.).
    pub(crate) async fn handle_traffic_event(
        &mut self,
        walsh_code: u8,
        event: &AccessChannelEvent,
    ) {
        debug!(
            "BSC: [traffic walsh={}] msg type={} preamble_only={} pdu=[{}]",
            walsh_code, event.msg_type_name, event.is_preamble_only, event.pdu_summary
        );

        // Find the mobile with this traffic channel
        let Some(addr) = self.mobiles.address_by_walsh(walsh_code) else {
            warn!(
                "BSC: traffic event for unknown walsh={}, ignoring",
                walsh_code
            );
            return;
        };
        let activity_now = Instant::now();

        // Update inactivity timer only on CRC-validated RX activity.
        // Signaling events (Orders, Data Bursts, etc.) already pass CRC in
        // the L3 decode path.  For PHY voice/packet frames, only full-rate
        // (9600) and half-rate (4800) carry FQI (CRC); quarter-rate (2400)
        // and eighth-rate (1200) are validated by tail bits alone, which
        // easily false-positive on noise when the mobile has gone silent.
        // Treating those as activity would prevent the idle timeout from
        // ever firing.
        let phy_activity_valid = event
            .traffic_phy_valid
            .unwrap_or(!event.is_traffic_phy_status);
        let is_crc_activity = event.message_id != MessageId::GeneralExtension
            || event.is_preamble_only
            || (phy_activity_valid
                && event
                    .traffic_primary_rate_bps
                    .map(|r| r >= 4800)
                    .unwrap_or(false));
        if is_crc_activity {
            self.record_mobile_activity(&addr, event, activity_now);
            self.mobiles.update_tc(walsh_code, |_, tc| {
                tc.last_activity_at = activity_now;
            });
        }

        // Reverse-link power control is BTS-owned. The BSC does not compute
        // per-PCG PCBs or reverse FER targets; management gRPC reads the BTS
        // power-control registry directly.
        if !event.is_preamble_only {
            self.mobiles.update_tc(walsh_code, |_, tc| {
                if let Some(reverse_pilot_ec_io_db) = event.reverse_pilot_ec_io_db {
                    tc.power_control.reverse_pilot_ec_io_db = Some(reverse_pilot_ec_io_db);
                }
            });
        }

        if event.is_traffic_phy_status {
            return;
        }

        // Transition TrafficAssigning → TrafficActive.
        //
        // Per IS-2000 3.6.4.2, the BS sends BS Ack Order when it acquires the
        // reverse traffic channel (preamble detection). The RX pipeline emits
        // a preamble event (is_preamble_only=true) only after confirming the
        // mobile is on-channel (consecutive null frames exceed threshold).
        // We gate BS Ack exclusively on this event — decoded data frames
        // alone do NOT trigger the transition, preventing premature ack
        // before the mobile's preamble has been validated.
        let channel_is_assigned = self
            .mobiles
            .get_traffic_channel(walsh_code)
            .is_some_and(|tc| tc.is_assigned());
        if event.is_preamble_only && channel_is_assigned {
            info!(
                "BSC: traffic channel walsh={} now active for {}",
                walsh_code,
                format_ms_address(&addr)
            );

            // Send BS Ack Order on the forward traffic channel (IS-2000 3.6.4.2).
            // The MS has T4m (10s) to receive this; we send it promptly on preamble detect.
            // Per C.S0004-E 3.2.2.1.1.2, if no r-dsch PDU has yet been
            // acknowledged since f-dsch acquisition/reset, ACK_SEQ is all ones.
            let ack_seq = 0b111;
            let order_msg = OrderMessage {
                order: 0b010000,
                ordq: 0,
                order_specific_fields: Vec::new(),
            };
            let sdu = order_msg.to_ftch_sdu();
            let bs_ack_sent = if let Err(e) = self.send_traffic_signaling(
                walsh_code,
                sdu,
                MessageId::Order,
                ack_seq,
                true,
                Some(order_msg),
                None,
                None,
                None,
                None,
            ) {
                warn!(
                    "BSC: failed to send BS Ack Order on F-TCH walsh={}: {}",
                    walsh_code, e
                );
                false
            } else {
                info!("BSC: sent BS Ack Order on F-TCH walsh={}", walsh_code);
                true
            };

            self.mobiles.set_state(&addr, MsState::TrafficActive);

            if bs_ack_sent {
                if let Some(tc) = self.mobiles.get_traffic_channel_mut(walsh_code) {
                    tc.mark_waiting_ms_ack();
                }
            }

            self.publish_mobiles();
        }

        // Any reverse traffic PDU can carry ACK_SEQ for a prior forward
        // traffic transmission. On r-dsch, ACK_SEQ=111 is only the "no ACK
        // pending" sentinel when there is no outstanding forward PDU requiring
        // acknowledgment since channel acquisition/reset; otherwise it is a
        // real acknowledgment for forward MSG_SEQ=7. Clear retry state before
        // duplicate filtering so repeated reverse PDUs still stop forward
        // retransmissions once the ACK has been observed.
        if let Some(ack_seq) = event.ack_seq {
            self.acknowledge_traffic_forward_pdu(&addr, ack_seq);

            let bs_ack_acked = event.msg_seq.is_some()
                && self
                    .mobiles
                    .get_traffic_channel(walsh_code)
                    .is_some_and(|tc| tc.is_waiting_ms_ack());
            if bs_ack_acked {
                self.advance_waiting_ms_ack(
                    walsh_code,
                    event.msg_seq.unwrap_or(0),
                    event.msg_type_name.as_str(),
                )
                .await;
            }
        }

        // ACK any r-dsch message that requests it (ack_req=1).
        // This prevents the MS from retransmitting when we don't have a
        // message-specific ACK handler below.
        if event.ack_req && !event.is_preamble_only {
            let ack_seq = event.msg_seq.unwrap_or(0);
            if let Err(e) = self.send_traffic_bs_ack(walsh_code, ack_seq) {
                warn!(
                    "BSC: failed to send BS Ack for {} on walsh={}: {}",
                    event.msg_type_name, walsh_code, e
                );
            }
        }

        // Per C.S0004-E 3.2.1.1.2.2, duplicate detection on the reverse
        // traffic channel uses MSG_SEQ. The mobile maintains separate
        // counters for assured (ack_req=1) vs unassured (ack_req=0) PDUs
        // per 3.2.2.1.2.2, so we track them in separate arrays.
        if let Some(msg_seq) = event.msg_seq {
            if !event.is_preamble_only {
                let is_duplicate = self
                    .mobiles
                    .update_tc(walsh_code, |_, tc| {
                        let arr = if event.ack_req {
                            &mut tc.reverse_regular_msg_seq_rcvd_ack
                        } else {
                            &mut tc.reverse_regular_msg_seq_rcvd_noack
                        };
                        mark_reverse_regular_msg_seq_received(arr, msg_seq)
                    })
                    .unwrap_or(false);
                if is_duplicate
                    && event.ack_req
                    && event.message_id != MessageId::ServiceConnectCompletion
                {
                    info!(
                        "BSC: duplicate reverse traffic PDU on walsh={} discarded after ACK (msg_seq={} msg_type={})",
                        walsh_code, msg_seq, event.msg_type_name
                    );
                    return;
                }
            }
        }

        // Dispatch based on message identity.
        if event.message_id == MessageId::Order {
            // Handle MS Order on reverse traffic channel (r-dsch)
            // Order codes per C.S0005-E Table 2.7.3-1
            if let Some(order) = event.order_code {
                if order == 0b010000 {
                    // r-dsch: Mobile Station Acknowledgment Order (0b010000)
                    info!(
                        "BSC: received MS Ack Order on R-TCH walsh={} for {} (valid_ack={} ack_seq={:?})",
                        walsh_code,
                        format_ms_address(&addr),
                        event.valid_ack,
                        event.ack_seq,
                    );
                    let ack_seq = event.msg_seq.unwrap_or(0);

                    // Dispatch based on the unified channel state machine.
                    if let Some(tc) = self.mobiles.get_traffic_channel(walsh_code) {
                        if tc.is_waiting_ms_ack() {
                            self.advance_waiting_ms_ack(
                                walsh_code,
                                ack_seq,
                                "Mobile Station Acknowledgment Order",
                            )
                            .await;
                        } else if tc.is_sms_pending_release() {
                            // MS acked SMS Cause Code — teardown
                            // (currently commented out / skip)
                        } else {
                            // SC already sent or service already negotiated — no-op
                        }
                    }
                } else if order == 0b010101 {
                    // r-dsch: Release Order (0b010101, ORDQ=0x00 normal release)
                    info!(
                        "BSC: received MS Release Order on R-TCH walsh={} for {}, tearing down",
                        walsh_code,
                        format_ms_address(&addr),
                    );
                    let session_id = self
                        .mobiles
                        .get_traffic_channel(walsh_code)
                        .and_then(|tc| tc.voice_session_id);
                    let leg_role = self
                        .mobiles
                        .get_traffic_channel(walsh_code)
                        .and_then(|tc| tc.voice_leg_role);
                    let (a1_call_id, a1_clear_state) = self
                        .mobiles
                        .get_traffic_channel(walsh_code)
                        .map(|tc| (tc.a1_call_id, tc.a1_clear_state))
                        .unwrap_or((None, A1ClearState::Idle));
                    if let (Some(call_id), A1ClearState::Idle) = (a1_call_id, a1_clear_state) {
                        self.a1.send_clear_request(call_id, 0);
                    }
                    self.teardown_traffic_channel(walsh_code).await;
                    self.on_voice_leg_released(session_id, leg_role);
                } else if order == reverse_order_code::CONNECT {
                    // MS→BS only. User answered an MT call.
                    info!(
                        "BSC: received MS Connect Order on R-TCH walsh={} for {}",
                        walsh_code,
                        format_ms_address(&addr),
                    );
                    let ack_seq = event.msg_seq.unwrap_or(0);
                    let (session_id, leg_role, a1_call_id) = self
                        .mobiles
                        .get_traffic_channel(walsh_code)
                        .map(|tc| (tc.voice_session_id, tc.voice_leg_role, tc.a1_call_id))
                        .unwrap_or((None, None, None));
                    match (session_id, leg_role) {
                        (Some(session_id), Some(VoiceLegRole::Callee)) => {
                            // MSC drives the tones-off AWIM in response to A1
                            // Connect via `Progress{Signal=0x3F}`. BSC only
                            // forwards air-side signaling driven by MSC.
                            let _ = ack_seq;
                            if let Some(call_id) = a1_call_id {
                                self.a1.send_connect(call_id);
                            }
                            if let Some(tc) = self.mobiles.get_traffic_channel_mut(walsh_code) {
                                tc.mark_voice_connected(true);
                            }
                            if let Some(session) = self.voice.session_mut(session_id) {
                                if let Some(callee) = session.callee.as_mut() {
                                    callee.answered = true;
                                }
                            }
                        }
                        _ => {}
                    }
                } else if order == cdma_common::access::reverse_order::CONTINUOUS_DTMF_TONE
                    || order == cdma_common::access::reverse_order::CONTINUOUS_DTMF_TONE_STOP
                {
                    // r-dsch Continuous DTMF Tone Order: ORDQ carries the
                    // digit per C.S0005-E §3.7.4 (same encoding as BDTMFM).
                    let start = order == cdma_common::access::reverse_order::CONTINUOUS_DTMF_TONE;
                    let digit = event
                        .decoded_l3
                        .as_ref()
                        .and_then(|l3| match l3 {
                            AccessMessage::Order(o) => o.order_specific.first().copied(),
                            _ => None,
                        })
                        .unwrap_or(0);
                    info!(
                        "BSC: Continuous DTMF Tone {} walsh={} digit=0x{:02x}",
                        if start { "start" } else { "stop" },
                        walsh_code,
                        digit,
                    );
                    self.emit_continuous_dtmf_order(walsh_code, digit, start);
                } else if order == reverse_order_code::SERVICE_OPTION_RESPONSE {
                    // MS reply to our Service Option Request Order. Per
                    // C.S0005-E §2.6.4: SERVICE_OPTION matches our proposed
                    // SO on accept, or a different value (typically 0xFFFF)
                    // on reject.
                    let resp_so = event.decoded_l3.as_ref().and_then(|l3| match l3 {
                        AccessMessage::Order(o) => o
                            .order_specific
                            .get(1..3)
                            .map(|bs| u16::from_be_bytes([bs[0], bs[1]])),
                        _ => None,
                    });
                    let waiting_service_response = self
                        .mobiles
                        .get_traffic_channel(walsh_code)
                        .is_some_and(|tc| tc.is_waiting_service_response());
                    let assigned_so = self
                        .mobiles
                        .get_traffic_channel(walsh_code)
                        .map(|tc| tc.service_option);
                    info!(
                        "BSC: received Service Option Response Order on R-TCH walsh={} SO={:?} (assigned SO={:?}, waiting={})",
                        walsh_code, resp_so, assigned_so, waiting_service_response
                    );
                    if waiting_service_response {
                        match (resp_so, assigned_so) {
                            (Some(resp), Some(assigned)) if resp == assigned => {
                                info!(
                                    "BSC: MS accepted Service Option Request Order on walsh={} SO={} — completing service negotiation",
                                    walsh_code, assigned
                                );
                                self.complete_service_negotiation(
                                    walsh_code,
                                    &addr,
                                    "Service Option Response Order accept",
                                )
                                .await;
                            }
                            _ => {
                                warn!(
                                    "BSC: MS rejected Service Option Request Order on walsh={} (SO={:?}, assigned SO={:?}), tearing down",
                                    walsh_code, resp_so, assigned_so
                                );
                                if !self.release_tch_and_signal_assignment_failure(
                                    walsh_code,
                                    &addr,
                                    "Service Option Response Order reject",
                                ) {
                                    self.teardown_traffic_channel(walsh_code).await;
                                }
                            }
                        }
                    }
                } else if order == reverse_order_code::MOBILE_STATION_REJECT {
                    // Forwards to MSC as an A.S0001 §6.1.7.5
                    // AddsDeliverAck failure when the rejected type is
                    // a Data Burst and there's a pending OTASP DBM on
                    // this walsh — otherwise the MSC session would
                    // wait through the full inbound-silence timeout.
                    use cdma_common::lac::message_types::{MessageId, WireChannel};
                    let detail = event.decoded_l3.as_ref().and_then(|l3| match l3 {
                        AccessMessage::Order(o) => {
                            o.parse_mobile_station_reject_order(WireChannel::ForwardDedicated)
                        }
                        _ => None,
                    });
                    let dbm_wire_type =
                        MessageId::DataBurst.wire_type(WireChannel::ForwardDedicated);
                    let mut handled_otasp_reject = false;
                    if let Some(ref detail) = detail
                        && let Some(dbm_wire_type) = dbm_wire_type
                        && detail.rejected_type == dbm_wire_type
                        && let Some(pending) = self.pending_otasp_dbm.remove(&walsh_code)
                    {
                        info!(
                            "BSC: MS Reject Order on walsh={} for OTASP DBM (ORDQ=0x{:02x}) — sending AddsDeliverAck failure tag=0x{:08x}",
                            walsh_code, detail.ordq, pending.a1_tag.0
                        );
                        self.a1.send_adds_deliver_ack(
                            pending.a1_tag,
                            Some(adds_deliver_ack_cause::RADIO_INTERFACE_MESSAGE_FAILURE),
                        );
                        handled_otasp_reject = true;
                    }
                    // MS rejected our outstanding Service Option Request /
                    // Response Order while we were awaiting its SO Response
                    // (C.S0005-E §2.6.4: MS sends MS Reject Order with
                    // REJECTED_ORDER=SOReq/SOResp when SERV_NEG is enabled
                    // on the channel and it doesn't accept the legacy order).
                    // Tear down the call.
                    let waiting_so_response = self
                        .mobiles
                        .get_traffic_channel(walsh_code)
                        .is_some_and(|tc| tc.is_waiting_service_response());
                    let so_neg_rejected = detail.as_ref().is_some_and(|d| {
                        matches!(
                            d.rejected_order,
                            Some(forward_order_code::SERVICE_OPTION_REQUEST)
                                | Some(forward_order_code::SERVICE_OPTION_RESPONSE)
                        )
                    });
                    if !handled_otasp_reject && waiting_so_response && so_neg_rejected {
                        warn!(
                            "BSC: MS Reject Order on walsh={} rejected our SO negotiation order (REJECTED_ORDER={:?} ORDQ=0x{:02x}), tearing down",
                            walsh_code,
                            detail.as_ref().and_then(|d| d.rejected_order),
                            detail.as_ref().map(|d| d.ordq).unwrap_or(0)
                        );
                        if !self.release_tch_and_signal_assignment_failure(
                            walsh_code,
                            &addr,
                            "MS Reject Order rejected SO negotiation",
                        ) {
                            self.teardown_traffic_channel(walsh_code).await;
                        }
                    } else if !handled_otasp_reject {
                        info!(
                            "BSC: received Mobile Station Reject Order on R-TCH walsh={} (not OTASP, or no pending DBM), ignoring",
                            walsh_code
                        );
                    }
                } else {
                    info!(
                        "BSC: received {} (0x{:02x}) on R-TCH walsh={}, ignoring",
                        reverse_order_name(order),
                        order,
                        walsh_code
                    );
                }
            }
        } else if event.message_id == MessageId::SendBurstDtmf {
            if let Some(AccessMessage::SendBurstDtmf(ref msg)) = event.decoded_l3 {
                info!(
                    "BSC: BDTMFM on walsh={} digits={} on_len=0b{:03b} off_len=0b{:03b}",
                    walsh_code,
                    msg.digits.len(),
                    msg.dtmf_on_length,
                    msg.dtmf_off_length,
                );
                self.play_bdtmfm_sequence(walsh_code, msg);
            } else {
                warn!(
                    "BSC: BDTMFM on walsh={} missing decoded L3, ignoring",
                    walsh_code
                );
            }
        } else if event.message_id == MessageId::ServiceResponse {
            // Service Response — MS accepts/rejects/counter-proposes our Service Request
            let resp_purpose = event
                .decoded_l3
                .as_ref()
                .and_then(|l3| match l3 {
                    AccessMessage::ServiceResponse(m) => Some(m.resp_purpose),
                    _ => None,
                })
                .unwrap_or(0xFF);
            let serv_req_seq = event.decoded_l3.as_ref().and_then(|l3| match l3 {
                AccessMessage::ServiceResponse(m) => Some(m.serv_req_seq),
                _ => None,
            });
            info!(
                "BSC: received Service Response on R-TCH walsh={} resp_purpose=0b{:04b} serv_req_seq={:?}",
                walsh_code, resp_purpose, serv_req_seq
            );

            let waiting_service_response = self
                .mobiles
                .get_traffic_channel(walsh_code)
                .is_some_and(|tc| tc.is_waiting_service_response());

            if waiting_service_response {
                let ack_seq = event.msg_seq.unwrap_or(0);
                match resp_purpose {
                    0b0000 => {
                        // Accept — mobile agreed to our proposed SO. Send Service Connect.
                        info!(
                            "BSC: mobile accepted Service Request on walsh={}, sending Service Connect",
                            walsh_code
                        );
                        if let Err(e) = self.send_service_connect(walsh_code, ack_seq) {
                            warn!(
                                "BSC: failed to send Service Connect on walsh={}: {}",
                                walsh_code, e
                            );
                        } else if let Some(tc) = self.mobiles.get_traffic_channel_mut(walsh_code) {
                            tc.mark_service_connecting();
                        }
                    }
                    0b0001 => {
                        warn!(
                            "BSC: mobile rejected Service Request on walsh={}",
                            walsh_code
                        );
                        if !self.release_tch_and_signal_assignment_failure(
                            walsh_code,
                            &addr,
                            "Service Response reject",
                        ) {
                            self.teardown_traffic_channel(walsh_code).await;
                        }
                    }
                    0b0010 => {
                        // Counter-propose: accept packet data or an implemented voice codec.
                        let proposed_so = event.decoded_l3.as_ref().and_then(|l3| match l3 {
                            AccessMessage::ServiceResponse(m) => m
                                .service_config
                                .as_ref()
                                .and_then(|cfg| cfg.connection_records.first())
                                .map(|cr| cr.service_option),
                            _ => None,
                        });
                        match proposed_so {
                            Some(so)
                                if is_packet_data_so(so)
                                    || VoiceCodec::from_service_option(so).is_some() =>
                            {
                                info!(
                                    "BSC: accepting counter-propose on walsh={} SO={} — sending Service Connect",
                                    walsh_code, so
                                );
                                if let Some(tc) = self.mobiles.get_traffic_channel_mut(walsh_code) {
                                    if VoiceCodec::from_service_option(so).is_some()
                                        && tc.voice_service_option.is_some()
                                    {
                                        tc.voice_service_option = Some(so);
                                    } else {
                                        tc.service_option = so;
                                    }
                                }
                                if let Err(e) = self.send_service_connect(walsh_code, ack_seq) {
                                    warn!(
                                        "BSC: failed to send Service Connect after counter-propose on walsh={}: {}",
                                        walsh_code, e
                                    );
                                    self.teardown_traffic_channel(walsh_code).await;
                                } else if let Some(tc) =
                                    self.mobiles.get_traffic_channel_mut(walsh_code)
                                {
                                    tc.mark_service_connecting();
                                }
                            }
                            Some(so) => {
                                warn!(
                                    "BSC: rejecting counter-propose on walsh={} — proposed SO={} is unsupported",
                                    walsh_code, so
                                );
                                let reason = format!("counter-propose SO={} unsupported", so);
                                if !self.release_tch_and_signal_assignment_failure(
                                    walsh_code, &addr, &reason,
                                ) {
                                    self.teardown_traffic_channel(walsh_code).await;
                                }
                            }
                            None => {
                                warn!(
                                    "BSC: counter-propose on walsh={} had no service config record — tearing down",
                                    walsh_code
                                );
                                self.teardown_traffic_channel(walsh_code).await;
                            }
                        }
                    }
                    _ => {
                        warn!(
                            "BSC: unknown RESP_PURPOSE 0b{:04b} on walsh={}, ignoring",
                            resp_purpose, walsh_code
                        );
                    }
                }
            } else {
                let channel_state_label = self
                    .mobiles
                    .get_traffic_channel(walsh_code)
                    .map(|tc| tc.state_label());
                info!(
                    "BSC: received Service Response in unexpected state {:?} on walsh={}, ignoring",
                    channel_state_label, walsh_code
                );
            }
        } else if event.message_id == MessageId::ServiceConnectCompletion {
            // Service Connect Completion — MS confirms service negotiation
            let serv_con_seq = event.decoded_l3.as_ref().and_then(|l3| l3.serv_con_seq());
            info!(
                "BSC: received Service Connect Completion on R-TCH walsh={} serv_con_seq={:?}",
                walsh_code, serv_con_seq
            );

            self.complete_service_negotiation(walsh_code, &addr, "Service Connect Completion")
                .await;
            // Issue-tracked: F-SCH setup should send ESCAM to activate the
            // supplemental channel after service negotiation. Disabled until
            // the ESCAM encoder is validated end-to-end with a handset.
        } else if event.message_id == MessageId::PowerMeasurementReport {
            // Power Measurement Report — MS reports forward FER statistics and
            // pilot strengths. Drive the forward power-control outer loop with
            // the FCH portion (errors_detected / pwr_meas_frames).
            if let Some(AccessMessage::PowerMeasurementReport(ref m)) = event.decoded_l3 {
                info!(
                    "BSC: received PMRM on R-TCH walsh={} errors={} frames={} last_hdm_seq={} pilots={:?}",
                    walsh_code,
                    m.errors_detected,
                    m.pwr_meas_frames,
                    m.last_hdm_seq,
                    m.pilot_strengths,
                );
                let mut pmrm_state_updated = false;
                let tick = self
                    .mobiles
                    .update_tc(walsh_code, |_, tc| {
                        pmrm_state_updated = true;
                        tc.forward_power_control.outer_loop_tick(
                            m.errors_detected,
                            m.pwr_meas_frames,
                            &m.pilot_strengths,
                            Instant::now(),
                        )
                    })
                    .flatten();
                if pmrm_state_updated {
                    self.publish_mobiles();
                }
                if let Some(tick) = tick
                    && let Some(bts_client) = self.config.bts_client.clone()
                {
                    let updated = bts_client
                        .set_traffic_gain(walsh_code, tick.new_gain_linear)
                        .await;
                    let delta_str = tick
                        .delta_since_prev
                        .map(|d| format!("{:.2}s", d.as_secs_f32()))
                        .unwrap_or_else(|| "first".to_string());
                    let pilots_db: Vec<String> = m
                        .pilot_strengths
                        .iter()
                        .map(|&raw| {
                            format!(
                                "{:.1}dB",
                                ForwardPowerControlState::pilot_strength_raw_to_ec_io_db(raw)
                            )
                        })
                        .collect();
                    info!(
                        "BSC: [forward power walsh={}] PMRM fer={:.2}% \
                         (errors={} frames={}) Δ={} fast_start={} \
                         pilots_ec_io=[{}] → gain_offset={:+.2} dB applied={} (linear={:.5})",
                        walsh_code,
                        tick.fer_pct,
                        m.errors_detected,
                        m.pwr_meas_frames,
                        delta_str,
                        tick.fast_start,
                        pilots_db.join(", "),
                        tick.gain_offset_db,
                        updated,
                        tick.new_gain_linear,
                    );
                }
            } else {
                info!(
                    "BSC: received Power Measurement Report on R-TCH walsh={} (no decoded L3)",
                    walsh_code
                );
            }
        } else if event.message_id == MessageId::DataBurst {
            // Process traffic channel data burst (MO SMS)
            self.handle_traffic_data_burst(walsh_code, event);
        } else if let (Some(bits), Some(rate_bps)) = (
            event.traffic_voice_bits.as_ref(),
            event.traffic_voice_rate_bps,
        ) {
            let (session_id, is_packet_data, msc_circuit_id) = self
                .mobiles
                .get_traffic_channel(walsh_code)
                .map(|tc| {
                    (
                        tc.voice_session_id,
                        is_packet_data_so(tc.service_option),
                        tc.msc_circuit_id,
                    )
                })
                .unwrap_or((None, false, None));

            if is_packet_data {
                debug!(
                    "BSC: packet primary frame walsh={} ignored from local event path; bearer poller owns packet ingress",
                    walsh_code
                );
            } else if self.is_voice_session_msc_media_controlled(session_id)
                && self.relay_reverse_frame_to_msc(&addr, bits, rate_bps)
            {
                // Frame relayed to MSC via voice bearer — MSC handles routing.
            } else if self.is_voice_session_msc_media_controlled(session_id) {
                let circuit_id = msc_circuit_id.unwrap_or_default();
                debug!(
                    "BSC: dropping MSC-controlled reverse voice frame walsh={} circuit_id={} because MSC bearer relay is unavailable",
                    walsh_code, circuit_id
                );
            }
        }
    }
}
