//! Traffic-channel authentication relays between A1 and the air interface.
//!
//! The MSC drives unique challenge, SSD update with base station challenge and
//! COUNT update with DTAP messages (A.S0014-D §2.3.1-§2.3.6, §2.3.9). The BSC
//! forwards each one to the mobile on f-dsch and relays the reply.

use cdma_common::access::{AccessMessage, ReverseOrderDetail};
use cdma_common::events::{AccessCellId, AccessChannelEvent};
use cdma_common::lac::message_types::{MessageId, WireChannel};
use cdma_common::lac::paging_messages::{
    AuthenticationChallengeMessage, ForwardOrderDetail, OrderMessage, SsdUpdateMessage,
};
use log::{info, warn};

use super::Bsc;

/// C.S0005-E Table 2.7.3-1 and Table 3.7.4-1 order codes.
const BASE_STATION_CHALLENGE_ORDER: u8 = 0b000010;
const SSD_UPDATE_RESULT_ORDER: u8 = 0b000011;
const SSD_UPDATE_REJECTION_ORDQ: u8 = 0b0000_0001;
const PARAMETER_UPDATE_ORDER: u8 = 0b000101;
/// C.S0005-E Table 3.7.4-1 assigns the request number to ORDQ bits 3..0.
const PARAMETER_UPDATE_REQUEST_NUMBER_MASK: u8 = 0b0000_1111;
const LONG_CODE_TRANSITION_REQUEST_ORDER: u8 = 0b010111;
const LONG_CODE_TRANSITION_PUBLIC_ORDQ: u8 = 0b0000_0000;
const LONG_CODE_TRANSITION_PRIVATE_ORDQ: u8 = 0b0000_0001;

/// A.S0014-D §3.3.4 Cause Layer 3 for an SSD Update Rejection Order.
const CAUSE_LAYER3_SSD_UPDATE_REJECTED: u8 = 0x3B;
const CAUSE_LAYER3_CODING_STANDARD_ITU: u8 = 0b00;
const CAUSE_LAYER3_LOCATION_PUBLIC_REMOTE: u8 = 0b0100;

impl Bsc {
    pub(crate) fn traffic_channel_for_call(
        &self,
        call_id: Option<u64>,
    ) -> Option<(AccessCellId, u8, u64)> {
        let call_id = call_id?;
        self.mobiles
            .all_walshes_for_a1_call(call_id)
            .first()
            .map(|(_, cell, walsh)| (*cell, *walsh, call_id))
    }

    fn send_traffic_authentication_message(
        &mut self,
        cell: AccessCellId,
        walsh_code: u8,
        sdu: cdma_common::bits::Bitstream,
        msg_id: MessageId,
        order: Option<OrderMessage>,
        label: &str,
    ) {
        info!("BSC: sending {label} on F-TCH walsh={walsh_code}");
        if let Err(error) = self.send_traffic_signaling(
            cell,
            walsh_code,
            sdu,
            msg_id,
            super::DEFAULT_TRAFFIC_ACK_SEQ,
            true,
            order,
            None,
            None,
            None,
            None,
        ) {
            warn!("BSC: failed to send {label} on walsh={walsh_code}: {error}");
        }
    }

    fn send_traffic_order(
        &mut self,
        cell: AccessCellId,
        walsh_code: u8,
        detail: ForwardOrderDetail,
        label: &str,
    ) {
        let order = match detail.to_order_message() {
            Ok(order) => order,
            Err(error) => {
                warn!("BSC: cannot build {label}: {error}");
                return;
            }
        };
        let sdu = order.to_ftch_sdu();
        self.send_traffic_authentication_message(
            cell,
            walsh_code,
            sdu,
            MessageId::Order,
            Some(order),
            label,
        );
    }

    pub(crate) fn handle_a1_authentication_request(
        &mut self,
        call_id: Option<u64>,
        request: cdma_ios::AuthenticationRequestMessage,
    ) {
        let cdma_ios::AuthenticationRequestMessage::Dtap(request) = request else {
            warn!("BSC: idle-state A1 Authentication Request is not supported, ignoring");
            return;
        };
        let Some(randu) = request.authentication_challenge_parameter_randu.randu() else {
            warn!("BSC: A1 Authentication Request without RANDU, ignoring");
            return;
        };
        let Some((cell, walsh_code, _)) = self.traffic_channel_for_call(call_id) else {
            warn!("BSC: A1 Authentication Request for unknown call_id={call_id:?}");
            return;
        };
        let message = AuthenticationChallengeMessage {
            randu,
            gen_cmea_key: false,
        };
        self.send_traffic_authentication_message(
            cell,
            walsh_code,
            message.to_sdu(),
            MessageId::AuthChallenge,
            None,
            "Authentication Challenge Message",
        );
    }

    pub(crate) fn handle_a1_ssd_update_request(
        &mut self,
        call_id: Option<u64>,
        request: cdma_ios::SsdUpdateRequestMessage,
    ) {
        let Some((cell, walsh_code, _)) = self.traffic_channel_for_call(call_id) else {
            warn!("BSC: A1 SSD Update Request for unknown call_id={call_id:?}");
            return;
        };
        let raw = request.authentication_challenge_parameter_randssd.0;
        let randssd = raw[1..]
            .iter()
            .fold(0u64, |acc, &b| (acc << 8) | u64::from(b));
        self.send_traffic_authentication_message(
            cell,
            walsh_code,
            SsdUpdateMessage { randssd }.to_sdu(),
            MessageId::SsdUpdate,
            None,
            "SSD Update Message",
        );
    }

    pub(crate) fn handle_a1_base_station_challenge_response(
        &mut self,
        call_id: Option<u64>,
        response: cdma_ios::BaseStationChallengeResponseMessage,
    ) {
        let Some((cell, walsh_code, _)) = self.traffic_channel_for_call(call_id) else {
            warn!("BSC: A1 Base Station Challenge Response for unknown call_id={call_id:?}");
            return;
        };
        let authbs = response
            .authentication_response_parameter_authbs
            .signature();
        self.send_traffic_order(
            cell,
            walsh_code,
            ForwardOrderDetail::BaseStationChallengeConfirmation { authbs },
            "Base Station Challenge Confirmation Order",
        );
    }

    pub(crate) fn handle_a1_parameter_update_request(&mut self, call_id: Option<u64>) {
        let Some((cell, walsh_code, _)) = self.traffic_channel_for_call(call_id) else {
            warn!("BSC: A1 Parameter Update Request for unknown call_id={call_id:?}");
            return;
        };
        let request_number = self
            .mobiles
            .update_tc(cell, walsh_code, |_, tc| tc.next_parameter_update_request())
            .unwrap_or(0);
        self.send_traffic_order(
            cell,
            walsh_code,
            ForwardOrderDetail::QualificationOnly {
                order: PARAMETER_UPDATE_ORDER,
                ordq: request_number,
            },
            "Parameter Update Order",
        );
    }

    pub(crate) fn request_long_code_transition(
        &mut self,
        cell: AccessCellId,
        walsh_code: u8,
        private: bool,
    ) {
        use cdma_common::phy::long_code_privacy::{
            LongCodeTransition, PRIVACY_ACTION_CHIPS, PRIVACY_ACTION_MODULUS,
        };
        const PRIVACY_SIGNALING_LEAD_ACTIONS: u64 = 20;
        let privacy = self
            .mobiles
            .get_traffic_channel(cell, walsh_code)
            .and_then(|tc| tc.long_code_privacy.clone());
        let now_chip = cdma_common::time::chips_since_epoch(
            cdma_common::time::system_time_now(),
            cdma_common::consts::SR1_CHIP_RATE_HZ,
        );
        let action_chip = (now_chip / PRIVACY_ACTION_CHIPS + PRIVACY_SIGNALING_LEAD_ACTIONS)
            * PRIVACY_ACTION_CHIPS;
        let private = private && privacy.is_some();
        if let Some(privacy) = &privacy {
            if !privacy.request(LongCodeTransition {
                action_chip,
                private,
            }) {
                return;
            }
        }
        self.mobiles.update_tc(cell, walsh_code, |_, tc| {
            tc.privacy_report_at = Some(action_chip);
        });
        let order = OrderMessage {
            order: LONG_CODE_TRANSITION_REQUEST_ORDER,
            ordq: u8::from(private),
            order_specific_fields: Vec::new(),
        };
        let action_time = ((action_chip / PRIVACY_ACTION_CHIPS) % PRIVACY_ACTION_MODULUS) as u8;
        let sdu = order.to_ftch_sdu_at(Some(action_time));
        self.send_traffic_authentication_message(
            cell,
            walsh_code,
            sdu,
            MessageId::Order,
            Some(order),
            if private {
                "Long Code Transition Request Order (private)"
            } else {
                "Long Code Transition Request Order (public)"
            },
        );
    }

    pub(crate) fn answer_long_code_transition_request(
        &mut self,
        cell: AccessCellId,
        walsh_code: u8,
        event: &AccessChannelEvent,
    ) -> bool {
        use cdma_common::phy::long_code_privacy::{
            LONG_CODE_RESPONSE_PRIVATE, LONG_CODE_RESPONSE_PUBLIC,
        };
        let Some(AccessMessage::Order(order)) = event.decoded_l3.as_ref() else {
            return false;
        };
        if order.order == cdma_common::phy::long_code_privacy::MOBILE_STATION_REJECT_ORDER
            && order
                .parse_mobile_station_reject_order_strict(WireChannel::ForwardDedicated)
                .is_ok_and(|reject| {
                    reject.rejected_type
                        == MessageId::Order
                            .wire_type(WireChannel::ForwardDedicated)
                            .expect("f-dsch Order has a wire type")
                        && reject.rejected_order == Some(LONG_CODE_TRANSITION_REQUEST_ORDER)
                })
        {
            self.mobiles.update_tc(cell, walsh_code, |_, tc| {
                if let Some(privacy) = &tc.long_code_privacy {
                    privacy.cancel_pending();
                }
                tc.privacy_report_at = None;
            });
            self.report_privacy_mode(cell, walsh_code);
            return true;
        }
        if order.order != LONG_CODE_TRANSITION_REQUEST_ORDER {
            return false;
        }
        match order.order_specific.first().copied() {
            Some(LONG_CODE_TRANSITION_PRIVATE_ORDQ) => {
                if let Some(call_id) = self
                    .mobiles
                    .get_traffic_channel(cell, walsh_code)
                    .and_then(|tc| tc.a1_call_id)
                {
                    self.a1_send_encoded(
                        call_id,
                        cdma_ios::MessageType::PrivacyModeComplete,
                        cdma_ios::PrivacyModeCompleteMessage {
                            encryption_information: None,
                            voice_privacy_request: true,
                        }
                        .encode(),
                    );
                }
            }
            Some(LONG_CODE_TRANSITION_PUBLIC_ORDQ) => {
                self.request_long_code_transition(cell, walsh_code, false)
            }
            Some(ordq @ (LONG_CODE_RESPONSE_PRIVATE | LONG_CODE_RESPONSE_PUBLIC)) => {
                let private = ordq == LONG_CODE_RESPONSE_PRIVATE;
                let now_chip = cdma_common::time::chips_since_epoch(
                    cdma_common::time::system_time_now(),
                    cdma_common::consts::SR1_CHIP_RATE_HZ,
                );
                let accepted = self
                    .mobiles
                    .get_traffic_channel(cell, walsh_code)
                    .and_then(|tc| tc.long_code_privacy.as_ref())
                    .is_some_and(|privacy| privacy.respond(private, now_chip));
                info!(
                    "BSC: long code transition response walsh={walsh_code} private={private} accepted={accepted}"
                );
                if !accepted {
                    self.mobiles.update_tc(cell, walsh_code, |_, tc| {
                        tc.privacy_report_at = None;
                    });
                    self.report_privacy_mode(cell, walsh_code);
                }
            }
            ordq => warn!("BSC: invalid long code transition ORDQ={ordq:?} walsh={walsh_code}"),
        }
        true
    }

    pub(crate) fn report_privacy_mode(&self, cell: AccessCellId, walsh_code: u8) {
        let Some(tc) = self.mobiles.get_traffic_channel(cell, walsh_code) else {
            return;
        };
        let Some(call_id) = tc.a1_call_id else {
            return;
        };
        let now_chip = cdma_common::time::chips_since_epoch(
            cdma_common::time::system_time_now(),
            cdma_common::consts::SR1_CHIP_RATE_HZ,
        );
        let active = tc
            .long_code_privacy
            .as_ref()
            .is_some_and(|privacy| privacy.is_private_at(now_chip));
        let mut info = cdma_ios::EncryptionInformation::private_long_code_status(active);
        info.parameters[0].available = tc.long_code_privacy.is_some();
        self.a1_send_encoded(
            call_id,
            cdma_ios::MessageType::PrivacyModeComplete,
            cdma_ios::PrivacyModeCompleteMessage {
                encryption_information: Some(info),
                voice_privacy_request: false,
            }
            .encode(),
        );
    }

    pub(crate) fn poll_privacy_transitions(&mut self) {
        use cdma_common::phy::long_code_privacy::PRIVACY_ACTION_CHIPS;
        const RESPONSE_GUARD_ACTIONS: u64 = 2;
        let now = cdma_common::time::chips_since_epoch(
            cdma_common::time::system_time_now(),
            cdma_common::consts::SR1_CHIP_RATE_HZ,
        );
        let actions: Vec<_> = self
            .mobiles
            .iter()
            .filter_map(|ms| {
                let tc = ms.traffic_channel()?;
                let action = tc.privacy_report_at?;
                let pending = tc
                    .long_code_privacy
                    .as_ref()
                    .is_some_and(|privacy| privacy.pending().is_some());
                if pending && now + RESPONSE_GUARD_ACTIONS * PRIVACY_ACTION_CHIPS >= action {
                    Some((ms.serving_cell?, tc.walsh_code, true))
                } else if now >= action {
                    Some((ms.serving_cell?, tc.walsh_code, false))
                } else {
                    None
                }
            })
            .collect();
        for (cell, walsh_code, expired) in actions {
            self.mobiles.update_tc(cell, walsh_code, |_, tc| {
                tc.privacy_report_at = None;
            });
            if expired {
                let address = self
                    .mobiles
                    .get_by_walsh(cell, walsh_code)
                    .map(|ms| ms.fwd_address.clone());
                if let Some(address) = address {
                    self.begin_voice_release(
                        &address,
                        super::DEFAULT_TRAFFIC_ACK_SEQ,
                        "long code transition response timed out",
                    );
                }
            } else {
                self.report_privacy_mode(cell, walsh_code);
            }
        }
    }

    pub(crate) fn relay_traffic_authentication_reply(
        &mut self,
        cell: AccessCellId,
        walsh_code: u8,
        event: &AccessChannelEvent,
    ) -> bool {
        let Some(call_id) = self
            .mobiles
            .get_traffic_channel(cell, walsh_code)
            .and_then(|tc| tc.a1_call_id)
        else {
            return false;
        };
        match event.decoded_l3.as_ref() {
            Some(AccessMessage::AuthChallengeResponse(response)) => {
                let message = cdma_ios::AuthenticationResponseMessage::Dtap(
                    cdma_ios::AuthenticationResponseDtapMessage {
                        authentication_response_parameter_authu:
                            cdma_ios::AuthenticationResponseParameter::new(
                                cdma_ios::AuthSignatureType::Authu,
                                response.authu,
                            ),
                    },
                );
                self.a1_send_encoded(
                    call_id,
                    cdma_ios::MessageType::AuthenticationResponse,
                    message.encode(),
                );
                true
            }
            Some(AccessMessage::Order(order)) => match order.order {
                BASE_STATION_CHALLENGE_ORDER => {
                    let Ok(ReverseOrderDetail::BaseStationChallenge { randbs }) =
                        order.reverse_detail(WireChannel::ForwardDedicated)
                    else {
                        warn!("BSC: malformed Base Station Challenge Order on walsh={walsh_code}");
                        return true;
                    };
                    let message = cdma_ios::BaseStationChallengeMessage {
                        authentication_challenge_parameter_randbs:
                            cdma_ios::AuthenticationChallengeParameter::new(
                                cdma_ios::RandomNumberType::Randbs,
                                u64::from(randbs),
                            ),
                    };
                    self.a1_send_encoded(
                        call_id,
                        cdma_ios::MessageType::BaseStationChallenge,
                        message.encode(),
                    );
                    true
                }
                SSD_UPDATE_RESULT_ORDER => {
                    let rejected =
                        order.order_specific.first().copied() == Some(SSD_UPDATE_REJECTION_ORDQ);
                    let message = cdma_ios::SsdUpdateResponseMessage {
                        cause_layer_3: rejected.then_some(cdma_ios::CauseLayer3 {
                            coding_standard: CAUSE_LAYER3_CODING_STANDARD_ITU,
                            location: CAUSE_LAYER3_LOCATION_PUBLIC_REMOTE,
                            cause_value: CAUSE_LAYER3_SSD_UPDATE_REJECTED,
                        }),
                    };
                    info!(
                        "BSC: mobile {} the SSD update on walsh={walsh_code}",
                        if rejected { "rejected" } else { "confirmed" }
                    );
                    self.a1_send_encoded(
                        call_id,
                        cdma_ios::MessageType::SsdUpdateResponse,
                        message.encode(),
                    );
                    true
                }
                PARAMETER_UPDATE_ORDER => {
                    let request_number = order.order_specific.first().copied().unwrap_or(0)
                        & PARAMETER_UPDATE_REQUEST_NUMBER_MASK;
                    info!(
                        "BSC: Parameter Update Confirmation request={request_number} on walsh={walsh_code}"
                    );
                    self.a1_send_encoded(
                        call_id,
                        cdma_ios::MessageType::ParameterUpdateConfirm,
                        cdma_ios::ParameterUpdateConfirmMessage.encode(),
                    );
                    true
                }
                _ => false,
            },
            _ => false,
        }
    }

    fn a1_send_encoded(
        &self,
        call_id: u64,
        message_type: cdma_ios::MessageType,
        payload: cdma_ios::Result<Vec<u8>>,
    ) {
        match payload {
            Ok(payload) => self.a1.send_message(call_id, message_type, payload),
            Err(error) => warn!("BSC: failed to encode {message_type:?}: {error}"),
        }
    }
}
