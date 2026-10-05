use cdma_common::access::status_records::{
    BasicCapabilityInformation, FchMultiplexInformation, FchMultiplexOption,
    INFO_RECORD_BAND_CLASS, INFO_RECORD_CAPABILITY, INFO_RECORD_CHANNEL_CONFIG,
    INFO_RECORD_EXT_MULTIPLEX, INFO_RECORD_MEID, INFO_RECORD_POWER_CONTROL, INFO_RECORD_TERMINAL,
    TerminalInformation, fch_channel_configuration_record, meid_record, parse_meid,
};
use cdma_common::access::{
    AccessInfoRecord, AccessMessage, AccessMessageHeader, ExtendedStatusResponseMessage,
    StatusResponseMessage,
};
use cdma_common::bits::Bitstream;
use cdma_common::consts::{
    SERVICE_OPTION_BASIC_VOICE, SERVICE_OPTION_EVRC_A, SERVICE_OPTION_EVRC_B,
    SERVICE_OPTION_EVRC_WB, SERVICE_OPTION_QCELP13,
};
use cdma_common::lac::message_types::MessageId;

use super::{
    AccessArq, MsAccessIdentity, SERVICE_OPTION_SMS, ServingSystem, encode_r_csch_pdu,
    sar_encapsulate,
};

const MSG_TYPE_STATUS_RESPONSE: u8 = 0x07;
const MSG_TYPE_EXT_STATUS_RESPONSE: u8 = 0x0A;
/// Extended Status Response replaces Status Response above this revision.
const LAST_STATUS_RESPONSE_P_REV: u8 = 3;

const SUPPORTED_RATE_SET_ONE_MASK: u8 = 0b1111_0000;
const MULTIPLEX_OPTION_RATE_SET_ONE: u16 = 1;
const BAND_CLASS_ONE_SUPPORTED_MASK: u8 = 0x40;
const MOBILE_MANUFACTURER_CODE: u8 = 0;
const MOBILE_MODEL_CODE: u8 = 0;
const MOBILE_FIRMWARE_REVISION: u16 = 0;
const LAST_BASE_CAPABILITY_P_REV: u8 = 6;

fn terminal_info_record(id: &MsAccessIdentity) -> Vec<u8> {
    TerminalInformation {
        mob_p_rev: id.mob_p_rev,
        mob_mfg_code: MOBILE_MANUFACTURER_CODE,
        mob_model: MOBILE_MODEL_CODE,
        mob_firm_rev: MOBILE_FIRMWARE_REVISION,
        scm: id.scm,
        local_ctrl: false,
        slot_cycle_index: id.slot_cycle_index,
        service_options: vec![
            SERVICE_OPTION_BASIC_VOICE,
            SERVICE_OPTION_EVRC_A,
            SERVICE_OPTION_SMS,
            SERVICE_OPTION_EVRC_B,
            SERVICE_OPTION_EVRC_WB,
            SERVICE_OPTION_QCELP13,
        ],
    }
    .to_record()
    .data
}

fn extended_multiplex_info_record(p_rev_in_use: u8) -> Vec<u8> {
    let option = FchMultiplexOption {
        multiplex_option: MULTIPLEX_OPTION_RATE_SET_ONE,
        num_bits: SUPPORTED_RATE_SET_ONE_MASK,
    };
    FchMultiplexInformation {
        forward: vec![option.clone()],
        reverse: vec![option],
    }
    .to_record(p_rev_in_use)
    .expect("one multiplex option fits the count field")
    .data
}

fn capability_info_record(p_rev_in_use: u8) -> Vec<u8> {
    BasicCapabilityInformation {
        mahho: true,
        puf: true,
        analog_553a: p_rev_in_use <= LAST_BASE_CAPABILITY_P_REV,
        ..Default::default()
    }
    .to_record(p_rev_in_use)
    .data
}

fn channel_config_info_record() -> Vec<u8> {
    fch_channel_configuration_record(&super::fch_rc3_capability())
        .expect("RC1 through RC3 occupy the configured three-bit maps")
        .data
}

fn meid_info_record(meid: &str) -> Option<Vec<u8>> {
    let Some(octets) = parse_meid(meid) else {
        log::warn!("cdma-ms: MEID {meid:?} is not 14 hexadecimal digits, leaving it out");
        return None;
    };
    Some(meid_record(&octets).data)
}

fn status_info_record(id: &MsAccessIdentity, p_rev_in_use: u8, record_type: u8) -> Option<Vec<u8>> {
    match record_type {
        INFO_RECORD_TERMINAL => Some(terminal_info_record(id)),
        INFO_RECORD_BAND_CLASS => Some(vec![BAND_CLASS_ONE_SUPPORTED_MASK]),
        INFO_RECORD_POWER_CONTROL => Some(vec![0]),
        INFO_RECORD_CAPABILITY => Some(capability_info_record(p_rev_in_use)),
        INFO_RECORD_CHANNEL_CONFIG => Some(channel_config_info_record()),
        INFO_RECORD_EXT_MULTIPLEX => Some(extended_multiplex_info_record(p_rev_in_use)),
        INFO_RECORD_MEID => id.meid.as_deref().and_then(meid_info_record),
        _ => None,
    }
}

pub(crate) fn status_response_sdu(
    id: &MsAccessIdentity,
    p_rev_in_use: u8,
    qual_info_type: u8,
    qual_info: &[u8],
    record_types: &[u8],
    extended: bool,
) -> Bitstream {
    let records: Vec<AccessInfoRecord> = record_types
        .iter()
        .map(|record_type| AccessInfoRecord {
            record_type: *record_type,
            data: status_info_record(id, p_rev_in_use, *record_type).unwrap_or_default(),
        })
        .collect();
    let header = AccessMessageHeader {
        pd: 1,
        message_id: if extended {
            MessageId::ExtStatusResponse
        } else {
            MessageId::StatusResponse
        },
    };
    let message = if extended {
        AccessMessage::ExtStatusResponse(ExtendedStatusResponseMessage {
            header,
            qual_info_type,
            qual_info: qual_info.to_vec(),
            num_info_records: records.len() as u8,
            records,
            remaining_bits: 0,
        })
    } else {
        AccessMessage::StatusResponse(StatusResponseMessage {
            header,
            qual_info_type,
            qual_info: qual_info.to_vec(),
            records,
            remaining_bits: 0,
        })
    };
    message
        .to_sdu()
        .expect("decoded status request lengths and mobile records fit their wire fields")
}

pub fn build_status_response_capsule(
    id: &MsAccessIdentity,
    qual_info_type: u8,
    qual_info: &[u8],
    record_types: &[u8],
    arq: &AccessArq,
    serving: &ServingSystem,
) -> Bitstream {
    let extended = serving.p_rev_in_use > LAST_STATUS_RESPONSE_P_REV;
    sar_encapsulate(&encode_r_csch_pdu(
        serving,
        if extended {
            MSG_TYPE_EXT_STATUS_RESPONSE
        } else {
            MSG_TYPE_STATUS_RESPONSE
        },
        id,
        arq,
        &status_response_sdu(
            id,
            serving.p_rev_in_use,
            qual_info_type,
            qual_info,
            record_types,
            extended,
        ),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cdma_common::lac::message_types::MessageId;
    #[test]
    fn status_response_answers_every_record_the_request_named() {
        use cdma_common::access::{AccessDecodeContext, AccessMessage, AccessMessageHeader};

        let id = MsAccessIdentity {
            esn: 0x4CDC_1D09,
            meid: Some("A1000012345678".to_string()),
            ..Default::default()
        };
        let requested = [0x0E, 0x1A, 0x17, 0x1B, 0x27];
        let sdu = status_response_sdu(&id, 6, 0, &[], &requested, true);
        let decoded = AccessMessage::decode_sdu_with_context(
            AccessMessageHeader {
                pd: 1,
                message_id: MessageId::ExtStatusResponse,
            },
            &sdu,
            AccessDecodeContext::new(Some(0), Some(6)),
        )
        .expect("extended status response should decode");
        let AccessMessage::ExtStatusResponse(response) = decoded else {
            panic!("expected Extended Status Response");
        };
        assert_eq!(response.num_info_records, requested.len() as u8);
        assert_eq!(
            response
                .records
                .iter()
                .map(|record| record.record_type)
                .collect::<Vec<_>>(),
            requested
        );
        assert_eq!(response.records[0].data, vec![0x40]);
        assert_eq!(response.records[1].data, vec![0x0E, 0x00]);
        assert_eq!(response.records[2].data, vec![0]);
        assert_eq!(
            response.records[4].data,
            vec![0x7A, 0x10, 0, 1, 0x23, 0x45, 0x67, 0x80]
        );
    }

    #[test]
    fn capability_record_matches_the_advertised_protocol_revision() {
        assert_eq!(capability_info_record(6), vec![0x0E, 0x00]);
        assert_eq!(capability_info_record(7), vec![0x0C, 0x00, 0x00, 0x00]);
    }

    #[test]
    fn extended_multiplex_record_matches_the_advertised_protocol_revision() {
        assert_eq!(extended_multiplex_info_record(6).len(), 9);
        assert_eq!(extended_multiplex_info_record(8).len(), 9);
        assert_eq!(extended_multiplex_info_record(9).len(), 10);
    }

    #[test]
    fn status_records_follow_p_rev_in_use_not_mobile_maximum() {
        let id = MsAccessIdentity {
            mob_p_rev: 9,
            ..Default::default()
        };
        assert_eq!(
            status_info_record(&id, 6, INFO_RECORD_EXT_MULTIPLEX)
                .expect("extended multiplex record")
                .len(),
            9
        );
        assert_eq!(
            status_info_record(&id, 9, INFO_RECORD_EXT_MULTIPLEX)
                .expect("extended multiplex record")
                .len(),
            10
        );
    }

    #[test]
    fn a_malformed_meid_is_left_out_rather_than_padded() {
        let id = MsAccessIdentity {
            meid: Some("nope".to_string()),
            ..Default::default()
        };
        assert_eq!(None, status_info_record(&id, 6, INFO_RECORD_MEID));
    }
}
