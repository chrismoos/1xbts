//! Traffic-channel lifecycle operations: release signaling and resource teardown.

use std::sync::Arc;

use cdma_common::error::Error;
use cdma_common::lac::{message_types::MessageId, paging_messages::OrderMessage};
use log::{info, warn};

use crate::addressing::format_ms_address;

use super::{A1ClearState, Bsc, MobileStation, MsState, TrafficChannelInfo};

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
    /// Tear down a traffic channel keyed by Walsh code (the unique stable
    /// key for an active TC). The owning mobile is resolved through the
    /// registry, so callers never need to track an `idx` across `.await`.
    pub(crate) async fn teardown_traffic_channel(&mut self, walsh_code: u8) {
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

        if let Some(session_id) = packet_session_id {
            self.close_packet_session(walsh_code, &session_id).await;
        }

        let bts_client = self.config.bts_client.clone();
        if let Some(sch_w32) = tc.sch_walsh_code.take() {
            if let Some(ref bts_client) = bts_client {
                bts_client.deallocate_sch(sch_w32).await;
                info!(
                    "BSC: released F-SCH W(32) code {} for walsh={}",
                    sch_w32, walsh_code
                );
            }
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
            }
        }

        self.publish_mobiles();
    }

    /// Send a Release Order on the forward traffic channel.
    ///
    /// ORDER=0b010101 (21 = Release) per C.S0004-E Table 3.7.2.3.2.1-3.
    pub(crate) fn send_traffic_release_order(
        &mut self,
        walsh_code: u8,
        ack_seq: u8,
    ) -> Result<(), Error> {
        let order_msg = OrderMessage {
            order: 0b010101, // Release Order
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
