//! Traffic-channel lifecycle operations: release signaling and resource teardown.

use std::sync::Arc;

use cdma_common::error::Error;
use cdma_common::lac::{message_types::MessageId, paging_messages::OrderMessage};
use log::{info, warn};

use crate::addressing::format_ms_address;

use super::{A1ClearState, Bsc, ChannelState, MobileStation, MsState, TrafficChannelInfo};

#[derive(Default)]
pub(crate) struct TrafficLifecycleService;

impl TrafficLifecycleService {
    pub(crate) fn remove_channel_for_teardown(
        &self,
        mobile: &mut MobileStation,
        walsh_code: u8,
        bearer: Option<&Arc<cdma_ios::VoiceBearerManager>>,
    ) -> Option<TrafficChannelInfo> {
        let tc = mobile.remove_traffic_channel_by_walsh(walsh_code, bearer)?;
        if !mobile.has_traffic_channel() {
            mobile.set_state(MsState::Registered);
        }
        Some(tc)
    }
}

impl Bsc {
    /// Ask the MS to release a packet-data traffic channel before removing the
    /// BTS-side resources. Immediate deallocate can leave the MS transmitting
    /// reverse traffic without forward power control and pollute other Walshes.
    pub(crate) fn begin_packet_tch_release(&mut self, walsh_code: u8, reason: &str) {
        let Some(ms) = self.mobiles.get_by_walsh(walsh_code) else {
            warn!(
                "BSC: begin_packet_tch_release called but no traffic channel walsh={}",
                walsh_code
            );
            return;
        };
        let Some(tc) = ms.find_traffic_channel_by_walsh(walsh_code) else {
            warn!(
                "BSC: begin_packet_tch_release found no traffic channel walsh={}",
                walsh_code
            );
            return;
        };
        if tc.is_releasing() {
            info!(
                "BSC: packet TCH walsh={} already releasing ({})",
                walsh_code, reason
            );
            return;
        }
        let addr = ms.fwd_address.clone();

        info!(
            "BSC: initiating packet TCH release on walsh={} for {} ({})",
            walsh_code,
            format_ms_address(&addr),
            reason
        );
        if let Err(e) = self.send_traffic_release_order(walsh_code, super::DEFAULT_TRAFFIC_ACK_SEQ)
        {
            warn!(
                "BSC: failed to send packet Release Order on walsh={} during {}: {}",
                walsh_code, reason, e
            );
        }
        self.mobiles.update_tc(walsh_code, |_, tc| {
            tc.mark_releasing();
        });
    }

    /// Tear down a traffic channel keyed by Walsh code (the unique stable
    /// key for an active TC). The owning mobile is resolved through the
    /// registry, so callers never need to track an `idx` across `.await`.
    pub(crate) async fn teardown_traffic_channel(&mut self, walsh_code: u8) {
        // If there's a pending OTASP DBM that never got an L2 ack or
        // L3 reject, the call is going away before MSC will hear back.
        // Send an AddsDeliverAck(cause=call_cleared) per A.S0001
        // §6.1.7.5 so the OTASP session can advance / terminate
        // instead of waiting on the 5 s inbound-silence timeout.
        if let Some(pending) = self.pending_otasp_dbm.remove(&walsh_code) {
            log::info!(
                "BSC: walsh={} teardown with pending OTASP DBM tag=0x{:08x} — sending AddsDeliverAck call_cleared",
                walsh_code,
                pending.a1_tag.0
            );
            self.a1.send_adds_deliver_ack(
                pending.a1_tag,
                Some(super::traffic_signaling::adds_deliver_ack_cause::CALL_CLEARED),
            );
        }
        let Some(ms) = self.mobiles.get_by_walsh(walsh_code) else {
            warn!(
                "BSC: teardown_traffic_channel called but no traffic channel walsh={}",
                walsh_code
            );
            return;
        };
        let channel_state_label = ms
            .find_traffic_channel_by_walsh(walsh_code)
            .map(|tc| tc.state_label())
            .unwrap_or("?");
        // Pre-AC states (mirrors the AC-send guard in traffic_signaling).
        let pre_assignment_complete =
            ms.find_traffic_channel_by_walsh(walsh_code)
                .is_some_and(|tc| {
                    matches!(
                        tc.channel_state,
                        ChannelState::Assigned { .. }
                            | ChannelState::WaitingMsAck { .. }
                            | ChannelState::WaitingServiceResponse { .. }
                            | ChannelState::ServiceConnecting { .. }
                    )
                });
        let addr = ms.fwd_address.clone();

        info!(
            "BSC: tearing down traffic channel walsh={} for {} (channel_state={})",
            walsh_code,
            format_ms_address(&addr),
            channel_state_label,
        );

        let bearer = self.config.msc_voice_bearer.clone();
        let removed = self.mobiles.update(&addr, |ms| {
            let tc = ms.remove_traffic_channel_by_walsh(walsh_code, bearer.as_ref())?;
            if !ms.has_traffic_channel() {
                ms.set_state(MsState::Registered);
            }
            Some(tc)
        });
        let Some(Some(mut tc)) = removed else {
            warn!(
                "BSC: remove_channel_for_teardown found no channel walsh={} for {}",
                walsh_code,
                format_ms_address(&addr),
            );
            return;
        };
        let packet_session_id = self.packet.detach_session(&mut tc);
        self.traffic_bearer
            .reverse_voice_silence_encoders
            .remove(&walsh_code);

        if let Some(session_id) = packet_session_id {
            self.close_packet_session(walsh_code, &session_id).await;
        }

        let bts_client = self.config.bts_client.clone();
        // F-SCH lives on the same BTS-side session as the FCH and is released
        // by the BTS when it processes `Remove` for the FCH CCR. We just drop
        // the local reference; no separate deallocate exchange is needed here.
        let sch_code = tc.sch_walsh_code.take();
        if let Some(sch_code) = sch_code {
            info!(
                "BSC: F-SCH code {} will be released alongside walsh={} by BtsRelease",
                sch_code, walsh_code
            );
        }

        if let Some(ref bts_client) = bts_client {
            bts_client.deallocate_traffic(walsh_code).await;

            bts_client.drop_pending_rx_request(walsh_code).await;
            info!("BSC: dropped pending walsh={} from rx_pool", walsh_code);
            bts_client.request_rx_removal(walsh_code).await;
            info!("BSC: queued walsh={} for rx removal", walsh_code);
        } else {
            warn!(
                "BSC: teardown_traffic_channel walsh={} cannot deallocate - bts_client is None!",
                walsh_code
            );
        }

        if let Some(call_id) = tc.a1_call_id {
            if matches!(tc.a1_clear_state, A1ClearState::ClearCommandReceived) {
                self.a1.send_clear_complete(call_id, false);
            } else if pre_assignment_complete
                && !self
                    .pending_a1_failure_after_release
                    .iter()
                    .any(|(stash_addr, _)| stash_addr == &addr)
            {
                info!(
                    "BSC: A1 tx AssignmentFailure call_id={call_id} after teardown (no prior AC)"
                );
                self.a1.send_assignment_failure(call_id, 0x16);
            }
        }

        self.publish_mobiles();

        self.fire_pending_a1_failure_after_release(&addr);
        self.resume_voice_page_after_release(&addr);
    }

    /// Send a Release Order on the forward traffic channel.
    ///
    /// Uses the Release Order code from C.S0004-E Table 3.7.2.3.2.1-3.
    pub(crate) fn send_traffic_release_order(
        &mut self,
        walsh_code: u8,
        ack_seq: u8,
    ) -> Result<(), Error> {
        let order_msg = OrderMessage {
            order: super::RELEASE_ORDER_CODE,
            ordq: 0,
            order_specific_fields: Vec::new(),
        };
        let sdu = order_msg.to_ftch_sdu();

        info!(
            "BSC: sending Release Order on F-TCH walsh={} ack_seq={}",
            walsh_code, ack_seq
        );

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
}
