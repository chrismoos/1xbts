//! C.S0005-E §3.6.4.1.5 private-mask transitions on both traffic directions.

use cdma_common::access::AccessMessage;
use cdma_common::bits::Bitstream;
use cdma_common::events::AccessChannelEvent;
use cdma_common::lac::message_types::{MessageId, WireChannel};
use cdma_common::phy::long_code_privacy::{
    LONG_CODE_RESPONSE_PRIVATE, LONG_CODE_RESPONSE_PUBLIC, LONG_CODE_TRANSITION_ORDER,
    LongCodePrivacy, LongCodeTransition, resolve_privacy_action_chip,
};

/// C.S0005-E Table 3.7.4-1 assigns ORDQ 00000001 to a private-code request.
const PRIVATE_REQUEST: u8 = 0b0000_0001;

pub(super) struct ForwardPrivacy {
    pub(super) privacy: std::sync::Arc<LongCodePrivacy>,
    reader: crate::receiver::access::DedicatedFrameReader,
}

impl ForwardPrivacy {
    pub(super) fn new(privacy: std::sync::Arc<LongCodePrivacy>) -> Self {
        Self {
            privacy,
            reader: crate::receiver::access::DedicatedFrameReader::new(),
        }
    }

    pub(super) fn observe(&mut self, info_bits: &[u8]) {
        const MUX1_SIGNALING_PREFIX: &[u8] = &[1, 0, 1, 1];
        const MUX2_SIGNALING_PREFIX: &[u8] = &[1, 0, 0, 1, 1];
        let Some(fragment) = info_bits
            .strip_prefix(MUX1_SIGNALING_PREFIX)
            .or_else(|| info_bits.strip_prefix(MUX2_SIGNALING_PREFIX))
        else {
            self.reader.reset();
            return;
        };
        let mut fragment = Bitstream::new_init(fragment);
        let frame = match self.reader.process(&mut fragment) {
            Ok(Some(frame)) if frame.crc_valid => frame,
            Ok(_) => return,
            Err(_) => {
                self.reader.reset();
                return;
            }
        };
        let Ok(pdu) = cdma_common::access::FdschPdu::decode(&frame.data) else {
            return;
        };
        let cdma_common::access::FdschMessage::Order(order) = pdu.body else {
            return;
        };
        if !order.use_time || order.order != LONG_CODE_TRANSITION_ORDER {
            return;
        }
        let ordq = order.order_specific.first().copied().unwrap_or(0);
        if ordq > PRIVATE_REQUEST {
            return;
        }
        let now_chip = cdma_common::time::chips_since_epoch(
            cdma_common::time::system_time_now(),
            cdma_common::consts::SR1_CHIP_RATE_HZ,
        );
        let transition = LongCodeTransition {
            action_chip: resolve_privacy_action_chip(order.action_time, now_chip),
            private: ordq == PRIVATE_REQUEST,
        };
        let accepted = self.privacy.request(transition);
        log::info!(
            "BTS: long code transition queued private={} action_chip={} accepted={}",
            transition.private,
            transition.action_chip,
            accepted,
        );
    }
}

pub(super) fn observe_reverse_order(privacy: &LongCodePrivacy, event: &AccessChannelEvent) {
    let Some(AccessMessage::Order(order)) = &event.decoded_l3 else {
        return;
    };
    if order.order == cdma_common::phy::long_code_privacy::MOBILE_STATION_REJECT_ORDER
        && order
            .parse_mobile_station_reject_order_strict(WireChannel::ForwardDedicated)
            .is_ok_and(|reject| {
                reject.rejected_type
                    == MessageId::Order
                        .wire_type(WireChannel::ForwardDedicated)
                        .expect("f-dsch Order has a wire type")
                    && reject.rejected_order == Some(LONG_CODE_TRANSITION_ORDER)
            })
    {
        privacy.cancel_pending();
        return;
    }
    if order.order != LONG_CODE_TRANSITION_ORDER {
        return;
    }
    let Some(ordq @ (LONG_CODE_RESPONSE_PUBLIC | LONG_CODE_RESPONSE_PRIVATE)) =
        order.order_specific.first().copied()
    else {
        return;
    };
    let now_chip = cdma_common::time::chips_since_epoch(
        cdma_common::time::system_time_now(),
        cdma_common::consts::SR1_CHIP_RATE_HZ,
    );
    let pending = privacy.pending();
    let private = ordq == LONG_CODE_RESPONSE_PRIVATE;
    let accepted = privacy.respond(private, now_chip);
    log::info!(
        "BTS: long code transition response walsh={:?} private={} accepted={} action_chip={:?} received_chip={}",
        event.traffic_walsh_code,
        private,
        accepted,
        pending.map(|transition| transition.action_chip),
        now_chip,
    );
}
