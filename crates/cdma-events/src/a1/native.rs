use cdma_ios::{EncodedA1Message, MessageType, MobileIdentity};

use crate::NetworkEvent;
use crate::proto::{self, network_event};

pub(super) fn enrich(event: &mut NetworkEvent, encoded: &EncodedA1Message) {
    let Ok(message) = encoded.decode() else {
        return;
    };
    let Some(network_event::Body::A1(a1)) = event.body.as_mut() else {
        return;
    };
    let mut identity = a1.mobile_identity.clone().unwrap_or_default();
    match message.message_type {
        MessageType::CompleteLayer3Information => {
            if let Ok(complete) =
                cdma_ios::CompleteLayer3InformationMessage::decode(&message.payload)
            {
                a1.cell = Some(proto::CellId {
                    cell: u32::from(complete.cell_identifier.cell),
                    sector: u32::from(complete.cell_identifier.sector),
                });
                let layer3 = complete.layer3_information;
                if let Ok(request) = layer3.decode_cm_service_request() {
                    merge(&mut identity, &request.mobile_identity_imsi);
                    merge_optional(&mut identity, request.mobile_identity_esn.as_ref());
                    merge_optional(&mut identity, request.mobile_identity_meid.as_ref());
                } else if let Ok(response) = layer3.decode_paging_response() {
                    merge(&mut identity, &response.mobile_identity_imsi);
                    merge_optional(&mut identity, response.mobile_identity_esn.as_ref());
                    merge_optional(&mut identity, response.mobile_identity_meid.as_ref());
                } else if let Ok(request) = layer3.decode_location_updating_request() {
                    merge(&mut identity, &request.mobile_identity_imsi);
                    merge_optional(&mut identity, request.mobile_identity_esn.as_ref());
                    merge_optional(&mut identity, request.mobile_identity_meid.as_ref());
                }
            }
        }
        MessageType::PagingRequest => {
            if let Ok(request) = cdma_ios::PagingRequestMessage::decode(&message.payload) {
                merge(&mut identity, &request.mobile_identity_imsi);
            }
        }
        MessageType::CmServiceRequest => {
            if let Ok(request) = cdma_ios::CmServiceRequestMessage::decode(&message.payload) {
                merge(&mut identity, &request.mobile_identity_imsi);
                merge_optional(&mut identity, request.mobile_identity_esn.as_ref());
                merge_optional(&mut identity, request.mobile_identity_meid.as_ref());
            }
        }
        MessageType::PagingResponse => {
            if let Ok(response) = cdma_ios::PagingResponseMessage::decode_a1(&message.payload) {
                merge(&mut identity, &response.mobile_identity_imsi);
                merge_optional(&mut identity, response.mobile_identity_esn.as_ref());
                merge_optional(&mut identity, response.mobile_identity_meid.as_ref());
            }
        }
        MessageType::AddsPage => {
            if let Ok(request) = cdma_ios::AddsPageMessage::decode(&message.payload) {
                merge(&mut identity, &request.mobile_identity);
                a1.adds = Some(super::adds_summary(
                    request.adds_user_part.burst_type,
                    &request.adds_user_part.data,
                ));
            }
        }
        MessageType::AddsTransfer => {
            if let Ok(request) = cdma_ios::AddsTransferMessage::decode(&message.payload) {
                merge(&mut identity, &request.mobile_identity_imsi);
                merge_optional(&mut identity, request.mobile_identity_esn.as_ref());
                merge_optional(&mut identity, request.mobile_identity_meid.as_ref());
                if let Some(cell) = request.cell_identifier {
                    a1.cell = Some(proto::CellId {
                        cell: cell.cell.into(),
                        sector: cell.sector.into(),
                    });
                }
                a1.adds = Some(super::adds_summary(
                    request.adds_user_part.burst_type,
                    &request.adds_user_part.data,
                ));
            }
        }
        MessageType::AddsTransferAck => {
            if let Ok(response) = cdma_ios::AddsTransferAckMessage::decode(&message.payload) {
                merge(&mut identity, &response.mobile_identity_imsi);
            }
        }
        MessageType::AddsPageAck => {
            if let Ok(response) = cdma_ios::AddsPageAckMessage::decode(&message.payload) {
                merge(&mut identity, &response.mobile_identity);
                merge_optional(&mut identity, response.mobile_identity_esn.as_ref());
            }
        }
        MessageType::AddsDeliver => {
            if let Ok(request) = cdma_ios::AddsDeliverMessage::decode(&message.payload) {
                a1.adds = Some(super::adds_summary(
                    request.adds_user_part.burst_type,
                    &request.adds_user_part.data,
                ));
            }
        }
        _ => {}
    }
    let has_identity = !identity.imsi.is_empty() || identity.esn != 0 || !identity.meid.is_empty();
    a1.summary = super::summarize(
        &a1.message_type_name,
        a1.information_elements.len(),
        has_identity.then_some(&identity),
        a1.adds.as_ref(),
    );
    if !has_identity {
        return;
    }
    event.identity = Some(proto::MobileIdentity {
        imsi: identity.imsi.clone(),
        esn: identity.esn,
        meid: identity.meid.clone(),
    });
    a1.mobile_identity = Some(identity);
}

fn merge_optional(target: &mut proto::A1MobileIdentity, identity: Option<&MobileIdentity>) {
    if let Some(identity) = identity {
        merge(target, identity);
    }
}

fn merge(target: &mut proto::A1MobileIdentity, identity: &MobileIdentity) {
    match identity {
        MobileIdentity::Imsi(imsi) => {
            if target.imsi.is_empty() || !target.imsi.ends_with(imsi) {
                target.imsi.clone_from(imsi);
            }
        }
        MobileIdentity::Esn(esn) => target.esn = *esn,
        MobileIdentity::Meid(meid) => {
            target.meid = meid.iter().map(|byte| format!("{byte:02x}")).collect();
        }
    }
}
