use std::time::Instant;

use cdma_common::{
    bits::Bitstream,
    paging::{imsi_11_12_to_digits, imsi_s_to_digits_checked, mcc_to_digits},
    time,
};
use log::{info, warn};

use crate::lac::message_types::MessageId;
use crate::receiver::{
    access_layer3::{AccessMessage, AccessMessageHeader, RdschPdu, access_message_type_name},
    access_pdu::ReverseAccessPdu,
    pipelined::SampleBlock,
};

use super::super::AccessChannelEvent;

pub(super) fn build_access_event(
    blk: &SampleBlock,
    chip_rate_hz: usize,
    batch_hw_time_ns: u64,
    auth_mode: u8,
    p_rev_in_use: u8,
    overhead_mcc: u16,
    overhead_imsi_11_12: u8,
) -> Option<AccessChannelEvent> {
    const ACCESS_FRAME_CHIPS: u64 = 96 * 256;

    if blk.tags.get("access_crc_valid").copied().unwrap_or(0) != 1 {
        return None;
    }

    let payload_bits: Vec<u8> = blk
        .samples
        .iter()
        .map(|s| if s.re >= 0.5 { 1u8 } else { 0u8 })
        .collect();

    let preamble_frames = blk.tags.get("access_preamble_frames").copied().unwrap_or(0);
    let pd = blk.tags.get("access_pd").copied().unwrap_or(0) as u8;
    let raw_msg_type = blk.tags.get("access_msg_type").copied().unwrap_or(0) as u8;
    let Some(msg_type_id) = MessageId::from_wire(
        crate::lac::message_types::WireChannel::ReverseCommon,
        raw_msg_type,
    ) else {
        warn!("BTS RX: dropping access event with unsupported MSG_TAG 0x{raw_msg_type:02x}");
        return None;
    };
    let absolute_chip_start = blk
        .tags
        .get("absolute_chip_start")
        .copied()
        .and_then(|chip| u64::try_from(chip).ok());
    let receive_time = absolute_chip_start.map(|chip| {
        time::system_time_from_chips(chip.saturating_add(ACCESS_FRAME_CHIPS), chip_rate_hz as u64)
    });

    let decode_ctx = crate::receiver::access_layer3::AccessDecodeContext::new(
        Some(auth_mode),
        Some(p_rev_in_use),
    );
    let bs = Bitstream::new_init(&payload_bits);
    let pdu = match ReverseAccessPdu::decode(&bs) {
        Ok(pdu) => pdu,
        Err(err) => {
            warn!("BTS RX: dropping access event after PDU decode failure: {err}");
            return None;
        }
    };
    let decoded_l3 = match decode_access_message_from_pdu(&pdu, decode_ctx) {
        Ok(decoded_l3) => decoded_l3,
        Err(err) => {
            warn!("BTS RX: dropping access event after Layer 3 decode failure: {err}");
            return None;
        }
    };
    let address = extract_address(&pdu);
    let l3_summary = Some(decoded_l3.summary());
    let pdu_summary = pdu.summary();

    // Extract structured fields from the decoded PDU for BSC state machine use.
    let (
        arq_msg_seq,
        arq_ack_seq,
        arq_ack_req,
        arq_valid_ack,
        msid_type,
        esn,
        meid,
        imsi_m_s1,
        imsi_m_s2,
        imsi_class,
        imsi_addr_num,
        imsi_mcc,
        imsi_11_12_field,
    ) = match &pdu {
        ReverseAccessPdu::Pd01PRev6(p) => {
            let msg_seq = p.arq.as_ref().map(|a| a.msg_seq);
            let ack_seq = p.arq.as_ref().map(|a| a.ack_seq);
            let ack_req = p.arq.as_ref().is_some_and(|a| a.ack_req);
            let valid_ack = p.arq.as_ref().is_some_and(|a| a.valid_ack);
            let ea = extract_addressing_fields(p.addressing.as_ref());
            (
                msg_seq,
                ack_seq,
                ack_req,
                valid_ack,
                ea.msid_type,
                ea.esn,
                ea.meid,
                ea.imsi_m_s1,
                ea.imsi_m_s2,
                ea.imsi_class,
                ea.imsi_addr_num,
                ea.mcc,
                ea.imsi_11_12,
            )
        }
        _ => (
            None, None, false, false, None, None, None, None, None, None, None, None, None,
        ),
    };

    let mob_p_rev_field = decoded_l3.mob_p_rev();
    let slot_cycle_index_field = decoded_l3.slot_cycle_index();
    let scm_field = decoded_l3.scm();
    let service_option_field = decoded_l3.service_option();
    let data_burst_info = decoded_l3
        .data_burst_fields()
        .map(|(bt, mn, nm, f)| (bt, mn, nm, f.to_vec()));

    let (for_rc_pref_field, rev_rc_pref_field) = match &decoded_l3 {
        AccessMessage::Origination(m) => Some((m.for_rc_pref, m.rev_rc_pref)),
        AccessMessage::PageResponse(m) => Some((m.for_rc_pref, m.rev_rc_pref)),
        _ => None,
    }
    .unwrap_or((None, None));

    let rev_fch_gating_req_field = match &decoded_l3 {
        AccessMessage::Origination(m) => m.rev_fch_gating_req,
        AccessMessage::PageResponse(m) => m.rev_fch_gating_req,
        _ => None,
    };

    let order_code_field = decoded_l3.order_code();

    let (for_supported_rcs, rev_supported_rcs) = (
        decoded_l3.for_supported_rcs(),
        decoded_l3.rev_supported_rcs(),
    );
    let imsi = derive_full_imsi_from_access_identity(
        imsi_m_s1,
        imsi_m_s2,
        imsi_class,
        imsi_mcc,
        imsi_11_12_field,
        overhead_mcc,
        overhead_imsi_11_12,
    );

    Some(AccessChannelEvent {
        event_id: super::next_access_event_id(),
        chip_start: blk.chip_start,
        absolute_chip_start,
        receive_time,
        preamble_frames,
        pd,
        message_id: msg_type_id,
        msg_type_name: access_message_type_name(raw_msg_type).to_string(),
        address,
        resolved_address: None,
        subscriber_id: None,
        l3_summary,
        decoded_l3: Some(decoded_l3),
        pdu_summary,
        msg_seq: arq_msg_seq,
        ack_seq: arq_ack_seq,
        ack_req: arq_ack_req,
        valid_ack: arq_valid_ack,
        msid_type,
        esn,
        meid,
        imsi,
        imsi_m_s1,
        imsi_m_s2,
        imsi_class,
        imsi_addr_num,
        imsi_mcc,
        imsi_11_12: imsi_11_12_field,
        mob_p_rev: mob_p_rev_field,
        slot_cycle_index: slot_cycle_index_field,
        scm: scm_field,
        burst_type: data_burst_info.as_ref().map(|(bt, _, _, _)| *bt),
        data_burst_fields: data_burst_info.as_ref().map(|(_, _, _, f)| f.clone()),
        data_burst_num_msgs: data_burst_info.as_ref().map(|(_, _, nm, _)| *nm),
        data_burst_msg_number: data_burst_info.as_ref().map(|(_, mn, _, _)| *mn),
        wall_clock_us: chrono::Utc::now().timestamp_micros() as u64,
        rx_wall_time: Some(Instant::now()),
        rx_hw_time_ns: Some(batch_hw_time_ns),
        snr_db: blk.tags.get("finger_snr_mdb").map(|v| *v as f32 / 1000.0),
        signal_power_db: blk
            .tags
            .get("finger_signal_power_mdb")
            .map(|v| *v as f32 / 1000.0),
        reverse_pilot_ec_io_db: reverse_pilot_ec_io_db_from_tags(blk),
        raw_power_db: blk
            .tags
            .get("finger_raw_power_mdb")
            .map(|v| *v as f32 / 1000.0),
        demod_quality_pct: blk.tags.get("access_frame_weak_soft_bits").map(|&weak| {
            // 96 Walsh symbols * 6 code symbols per Walsh = 576 soft bits per frame
            let total = 576.0_f32;
            (100.0 - (weak as f32 / total * 100.0)).clamp(0.0, 100.0)
        }),
        pcg_signal_snr_db: None,
        active_pcg_mask: None,
        traffic_phy_valid: None,
        traffic_fqi_valid: None,
        traffic_tail_valid: None,
        traffic_fqi_bits: None,
        traffic_ml_tail_match: None,
        order_code: order_code_field,
        service_option: service_option_field,
        for_rc_pref: for_rc_pref_field,
        rev_rc_pref: rev_rc_pref_field,
        rev_fch_gating_req: rev_fch_gating_req_field,
        traffic_walsh_code: None,
        is_preamble_only: false,
        is_traffic_pcg_measurement: false,
        is_traffic_phy_status: false,
        traffic_measurement_age_chips: None,
        for_supported_rcs,
        rev_supported_rcs,
        decoded_rdsch: None,
        traffic_primary_bits: None,
        traffic_primary_rate_bps: None,
        traffic_primary_bearer_routed: false,
        traffic_voice_bits: None,
        traffic_voice_rate_bps: None,
        raw_pdu_bits: Some(payload_bits),
    })
}

/// Build a traffic channel event from a decoded reverse traffic channel frame.
///
/// Decodes the r-dsch PDU format: MSG_TYPE(8) + ACK_SEQ(3) + MSG_SEQ(3) +
/// ACK_REQ(1) + ENCRYPTION(2) + message-specific fields. Maps r-dsch MSG_TYPE
/// to the access-channel MSG_TAG constants used by the BSC dispatcher.
pub(super) fn build_traffic_event(
    blk: &SampleBlock,
    chip_rate_hz: usize,
    batch_hw_time_ns: u64,
) -> Option<AccessChannelEvent> {
    const TRAFFIC_FRAME_CHIPS: u64 = 96 * 256;

    if blk.tags.get("traffic_crc_valid").copied().unwrap_or(0) != 1 {
        return None;
    }

    let walsh_code = blk.tags.get("traffic_walsh_code").copied().unwrap_or(0) as u8;
    let rate_bps = blk
        .tags
        .get("traffic_rate_bps")
        .and_then(|value| u32::try_from(*value).ok())
        .unwrap_or(9600);

    let payload_bits: Vec<u8> = blk
        .samples
        .iter()
        .map(|s| if s.re >= 0.5 { 1u8 } else { 0u8 })
        .collect();

    let absolute_chip_start = blk
        .tags
        .get("absolute_chip_start")
        .copied()
        .and_then(|chip| u64::try_from(chip).ok());
    let receive_time = absolute_chip_start.map(|chip| {
        time::system_time_from_chips(
            chip.saturating_add(TRAFFIC_FRAME_CHIPS),
            chip_rate_hz as u64,
        )
    });

    let bs = Bitstream::new_init(&payload_bits);
    let rdsch = match RdschPdu::decode(&bs) {
        Ok(pdu) => pdu,
        Err(err) => {
            warn!(
                "rx: failed to decode r-dsch PDU on walsh={}: {}",
                walsh_code, err
            );
            return None;
        }
    };

    let order_code = rdsch.l3.order_code();
    let data_burst_info = rdsch
        .l3
        .data_burst_fields()
        .map(|(bt, mn, nm, f)| (bt, mn, nm, f.to_vec()));
    let decoded_l3 = Some(rdsch.l3.clone());
    let l3_summary = Some(rdsch.l3.summary());
    let pdu_summary = rdsch.summary();
    let valid_ack = true;

    info!(
        "rx: traffic event on walsh={} payload_bits={} rdsch={}",
        walsh_code,
        payload_bits.len(),
        pdu_summary,
    );

    Some(AccessChannelEvent {
        event_id: super::next_access_event_id(),
        chip_start: blk.chip_start,
        absolute_chip_start,
        receive_time,
        preamble_frames: 0,
        pd: 0,
        message_id: rdsch.message_id,
        msg_type_name: rdsch.msg_type_name().to_string(),
        address: None,
        resolved_address: None,
        subscriber_id: None,
        l3_summary,
        decoded_l3,
        pdu_summary,
        msg_seq: Some(rdsch.arq.msg_seq),
        ack_seq: Some(rdsch.arq.ack_seq),
        ack_req: rdsch.arq.ack_req,
        valid_ack,
        msid_type: None,
        esn: None,
        imsi: None,
        meid: None,
        imsi_m_s1: None,
        imsi_m_s2: None,
        imsi_class: None,
        imsi_addr_num: None,
        imsi_mcc: None,
        imsi_11_12: None,
        mob_p_rev: None,
        slot_cycle_index: None,
        scm: None,
        burst_type: data_burst_info.as_ref().map(|(bt, _, _, _)| *bt),
        data_burst_fields: data_burst_info.as_ref().map(|(_, _, _, f)| f.clone()),
        data_burst_num_msgs: data_burst_info.as_ref().map(|(_, _, nm, _)| *nm),
        data_burst_msg_number: data_burst_info.as_ref().map(|(_, mn, _, _)| *mn),
        wall_clock_us: chrono::Utc::now().timestamp_micros() as u64,
        rx_wall_time: Some(Instant::now()),
        rx_hw_time_ns: Some(batch_hw_time_ns),
        snr_db: blk.tags.get("finger_snr_mdb").map(|v| *v as f32 / 1000.0),
        signal_power_db: blk
            .tags
            .get("finger_signal_power_mdb")
            .map(|v| *v as f32 / 1000.0),
        reverse_pilot_ec_io_db: reverse_pilot_ec_io_db_from_tags(blk),
        raw_power_db: blk
            .tags
            .get("finger_raw_power_mdb")
            .map(|v| *v as f32 / 1000.0),
        demod_quality_pct: None,
        pcg_signal_snr_db: blk.pcg_signal_snr_db.clone(),
        active_pcg_mask: blk.active_pcg_mask,
        traffic_phy_valid: traffic_tag_bool(blk, "traffic_phy_valid"),
        traffic_fqi_valid: traffic_tag_bool(blk, "traffic_fqi_valid"),
        traffic_tail_valid: traffic_tag_bool(blk, "traffic_tail_valid"),
        traffic_fqi_bits: traffic_tag_u8(blk, "traffic_fqi_bits"),
        traffic_ml_tail_match: blk.tags.get("traffic_ml_tail_match").map(|v| *v != 0),
        order_code,
        service_option: None,
        for_rc_pref: None,
        rev_rc_pref: None,
        rev_fch_gating_req: None,
        traffic_walsh_code: Some(walsh_code),
        is_preamble_only: false,
        is_traffic_pcg_measurement: false,
        is_traffic_phy_status: false,
        traffic_measurement_age_chips: None,
        for_supported_rcs: Vec::new(),
        rev_supported_rcs: Vec::new(),
        decoded_rdsch: Some(rdsch),
        traffic_primary_bits: Some(payload_bits),
        traffic_primary_rate_bps: Some(rate_bps),
        traffic_primary_bearer_routed: false,
        traffic_voice_bits: None,
        traffic_voice_rate_bps: None,
        raw_pdu_bits: None,
    })
}

pub(super) fn reverse_pilot_ec_io_db_from_tags(blk: &SampleBlock) -> Option<f32> {
    blk.tags
        .get("traffic_pcg_pilot_ec_io_true_mdb")
        .or_else(|| blk.tags.get("finger_pilot_ec_io_mdb"))
        .map(|value| *value as f32 / 1000.0)
}

pub(super) fn traffic_tag_bool(blk: &SampleBlock, key: &'static str) -> Option<bool> {
    blk.tags.get(key).map(|value| *value != 0)
}

pub(super) fn traffic_tag_u8(blk: &SampleBlock, key: &'static str) -> Option<u8> {
    blk.tags
        .get(key)
        .and_then(|value| u8::try_from(*value).ok())
}

pub(super) fn build_traffic_phy_status_event(
    blk: &SampleBlock,
    chip_rate_hz: usize,
    batch_hw_time_ns: u64,
) -> Option<AccessChannelEvent> {
    if blk.tags.get("traffic_phy_status").copied().unwrap_or(0) != 1 {
        return None;
    }

    let walsh_code = blk.tags.get("traffic_walsh_code").copied().unwrap_or(0) as u8;
    let rate_bps = blk
        .tags
        .get("traffic_rate_bps")
        .and_then(|value| u32::try_from(*value).ok())
        .unwrap_or(0);
    let fqi_bits = traffic_tag_u8(blk, "traffic_fqi_bits").unwrap_or(0);
    let phy_valid = traffic_tag_bool(blk, "traffic_phy_valid").unwrap_or(false);
    let fqi_valid = traffic_tag_bool(blk, "traffic_fqi_valid");
    let tail_valid = traffic_tag_bool(blk, "traffic_tail_valid");

    let absolute_chip_start = blk
        .tags
        .get("absolute_chip_start")
        .copied()
        .and_then(|chip| u64::try_from(chip).ok());
    let receive_time = absolute_chip_start.map(|chip| {
        time::system_time_from_chips(chip.saturating_add(96 * 256), chip_rate_hz as u64)
    });

    Some(AccessChannelEvent {
        event_id: super::next_access_event_id(),
        chip_start: blk.chip_start,
        absolute_chip_start,
        receive_time,
        preamble_frames: 0,
        pd: 0,
        message_id: MessageId::GeneralExtension,
        msg_type_name: format!("TrafficPhyStatus(W{} {}bps)", walsh_code, rate_bps),
        address: None,
        resolved_address: None,
        subscriber_id: None,
        l3_summary: None,
        decoded_l3: None,
        pdu_summary: format!(
            "traffic_phy_status walsh={} rate_bps={} phy_valid={} fqi_bits={} fqi_valid={:?} tail_valid={:?}",
            walsh_code, rate_bps, phy_valid, fqi_bits, fqi_valid, tail_valid
        ),
        msg_seq: None,
        ack_seq: None,
        ack_req: false,
        valid_ack: false,
        msid_type: None,
        esn: None,
        imsi: None,
        meid: None,
        imsi_m_s1: None,
        imsi_m_s2: None,
        imsi_class: None,
        imsi_addr_num: None,
        imsi_mcc: None,
        imsi_11_12: None,
        mob_p_rev: None,
        slot_cycle_index: None,
        scm: None,
        burst_type: None,
        data_burst_fields: None,
        data_burst_num_msgs: None,
        data_burst_msg_number: None,
        wall_clock_us: chrono::Utc::now().timestamp_micros() as u64,
        rx_wall_time: Some(Instant::now()),
        rx_hw_time_ns: Some(batch_hw_time_ns),
        snr_db: blk.tags.get("finger_snr_mdb").map(|v| *v as f32 / 1000.0),
        signal_power_db: blk
            .tags
            .get("finger_signal_power_mdb")
            .map(|v| *v as f32 / 1000.0),
        reverse_pilot_ec_io_db: reverse_pilot_ec_io_db_from_tags(blk),
        raw_power_db: blk
            .tags
            .get("finger_raw_power_mdb")
            .map(|v| *v as f32 / 1000.0),
        demod_quality_pct: None,
        pcg_signal_snr_db: blk.pcg_signal_snr_db.clone(),
        active_pcg_mask: blk.active_pcg_mask,
        traffic_phy_valid: traffic_tag_bool(blk, "traffic_phy_valid"),
        traffic_fqi_valid: fqi_valid,
        traffic_tail_valid: tail_valid,
        traffic_fqi_bits: Some(fqi_bits),
        traffic_ml_tail_match: blk.tags.get("traffic_ml_tail_match").map(|v| *v != 0),
        order_code: None,
        service_option: None,
        for_rc_pref: None,
        rev_rc_pref: None,
        rev_fch_gating_req: None,
        traffic_walsh_code: Some(walsh_code),
        is_preamble_only: false,
        is_traffic_pcg_measurement: false,
        is_traffic_phy_status: true,
        traffic_measurement_age_chips: None,
        for_supported_rcs: Vec::new(),
        rev_supported_rcs: Vec::new(),
        decoded_rdsch: None,
        traffic_primary_bits: None,
        traffic_primary_rate_bps: (rate_bps != 0).then_some(rate_bps),
        traffic_primary_bearer_routed: false,
        traffic_voice_bits: None,
        traffic_voice_rate_bps: None,
        raw_pdu_bits: None,
    })
}

pub(super) fn build_traffic_voice_event(
    blk: &SampleBlock,
    chip_rate_hz: usize,
    batch_hw_time_ns: u64,
) -> Option<AccessChannelEvent> {
    if blk.tags.get("traffic_phy_frame").copied().unwrap_or(0) != 1 {
        return None;
    }

    let walsh_code = blk.tags.get("traffic_walsh_code").copied().unwrap_or(0) as u8;
    let info_bits = blk.tags.get("traffic_info_bits").copied().unwrap_or(0) as usize;
    let rate_bps = blk.tags.get("traffic_rate_bps").copied().unwrap_or(0) as u32;
    let signaling_bits = blk
        .tags
        .get("traffic_mux_signaling_bits")
        .copied()
        .unwrap_or(0) as usize;

    if info_bits == 0 || rate_bps == 0 {
        return None;
    }

    let primary_bits: Vec<u8> = blk
        .samples
        .iter()
        .take(info_bits)
        .map(|s| if s.re >= 0.5 { 1u8 } else { 0u8 })
        .collect();
    if primary_bits.len() != info_bits {
        return None;
    }

    let voice_bits = primary_bits.clone();

    let absolute_chip_start = blk
        .tags
        .get("absolute_chip_start")
        .copied()
        .and_then(|chip| u64::try_from(chip).ok());
    let receive_time = absolute_chip_start.map(|chip| {
        time::system_time_from_chips(chip.saturating_add(96 * 256), chip_rate_hz as u64)
    });

    Some(AccessChannelEvent {
        event_id: super::next_access_event_id(),
        chip_start: blk.chip_start,
        absolute_chip_start,
        receive_time,
        preamble_frames: 0,
        pd: 0,
        message_id: MessageId::GeneralExtension,
        msg_type_name: format!("TrafficPrimaryFrame({}bps)", rate_bps),
        address: None,
        resolved_address: None,
        subscriber_id: None,
        l3_summary: None,
        decoded_l3: None,
        pdu_summary: format!(
            "primary_frame walsh={} rate_bps={} mux_signaling_bits={}",
            walsh_code, rate_bps, signaling_bits
        ),
        msg_seq: None,
        ack_seq: None,
        ack_req: false,
        valid_ack: false,
        msid_type: None,
        esn: None,
        imsi: None,
        meid: None,
        imsi_m_s1: None,
        imsi_m_s2: None,
        imsi_class: None,
        imsi_addr_num: None,
        imsi_mcc: None,
        imsi_11_12: None,
        mob_p_rev: None,
        slot_cycle_index: None,
        scm: None,
        burst_type: None,
        data_burst_fields: None,
        data_burst_num_msgs: None,
        data_burst_msg_number: None,
        wall_clock_us: chrono::Utc::now().timestamp_micros() as u64,
        rx_wall_time: Some(Instant::now()),
        rx_hw_time_ns: Some(batch_hw_time_ns),
        snr_db: blk.tags.get("finger_snr_mdb").map(|v| *v as f32 / 1000.0),
        signal_power_db: blk
            .tags
            .get("finger_signal_power_mdb")
            .map(|v| *v as f32 / 1000.0),
        reverse_pilot_ec_io_db: reverse_pilot_ec_io_db_from_tags(blk),
        raw_power_db: blk
            .tags
            .get("finger_raw_power_mdb")
            .map(|v| *v as f32 / 1000.0),
        demod_quality_pct: None,
        pcg_signal_snr_db: blk.pcg_signal_snr_db.clone(),
        active_pcg_mask: blk.active_pcg_mask,
        traffic_phy_valid: traffic_tag_bool(blk, "traffic_phy_valid"),
        traffic_fqi_valid: traffic_tag_bool(blk, "traffic_fqi_valid"),
        traffic_tail_valid: traffic_tag_bool(blk, "traffic_tail_valid"),
        traffic_fqi_bits: traffic_tag_u8(blk, "traffic_fqi_bits"),
        traffic_ml_tail_match: blk.tags.get("traffic_ml_tail_match").map(|v| *v != 0),
        order_code: None,
        service_option: None,
        for_rc_pref: None,
        rev_rc_pref: None,
        rev_fch_gating_req: None,
        traffic_walsh_code: Some(walsh_code),
        is_preamble_only: false,
        is_traffic_pcg_measurement: false,
        is_traffic_phy_status: false,
        traffic_measurement_age_chips: None,
        for_supported_rcs: Vec::new(),
        rev_supported_rcs: Vec::new(),
        decoded_rdsch: None,
        traffic_primary_bits: Some(primary_bits),
        traffic_primary_rate_bps: Some(rate_bps),
        traffic_primary_bearer_routed: false,
        traffic_voice_bits: Some(voice_bits),
        traffic_voice_rate_bps: Some(rate_bps),
        raw_pdu_bits: None,
    })
}

/// Build a lightweight preamble-acquired event for a traffic channel.
///
/// Sent when the RC3 pilot detector (or RC1 preamble detector) fires,
/// before any decoded frames. This lets the BSC send BS Ack Order
/// at the spec-correct time (IS-2000 3.6.4.2: "reverse traffic acquired").
pub(super) fn build_traffic_preamble_event(
    walsh_code: u8,
    chip_start: usize,
    batch_hw_time_ns: u64,
    preamble_pcgs: i64,
) -> AccessChannelEvent {
    AccessChannelEvent {
        event_id: super::next_access_event_id(),
        chip_start,
        absolute_chip_start: None,
        receive_time: None,
        preamble_frames: preamble_pcgs,
        pd: 0,
        message_id: MessageId::GeneralExtension,
        msg_type_name: format!("TrafficPreamble(W{})", walsh_code),
        address: None,
        resolved_address: None,
        subscriber_id: None,
        l3_summary: None,
        decoded_l3: None,
        pdu_summary: format!(
            "preamble_acquired walsh={} pcgs={}",
            walsh_code, preamble_pcgs
        ),
        msg_seq: None,
        ack_seq: None,
        ack_req: false,
        valid_ack: false,
        msid_type: None,
        esn: None,
        imsi: None,
        meid: None,
        imsi_m_s1: None,
        imsi_m_s2: None,
        imsi_class: None,
        imsi_addr_num: None,
        imsi_mcc: None,
        imsi_11_12: None,
        mob_p_rev: None,
        slot_cycle_index: None,
        scm: None,
        burst_type: None,
        data_burst_fields: None,
        data_burst_num_msgs: None,
        data_burst_msg_number: None,
        wall_clock_us: chrono::Utc::now().timestamp_micros() as u64,
        rx_wall_time: Some(Instant::now()),
        rx_hw_time_ns: Some(batch_hw_time_ns),
        snr_db: None,
        signal_power_db: None,
        reverse_pilot_ec_io_db: None,
        raw_power_db: None,
        demod_quality_pct: None,
        pcg_signal_snr_db: None,
        active_pcg_mask: None,
        traffic_phy_valid: None,
        traffic_fqi_valid: None,
        traffic_tail_valid: None,
        traffic_fqi_bits: None,
        traffic_ml_tail_match: None,
        order_code: None,
        service_option: None,
        for_rc_pref: None,
        rev_rc_pref: None,
        rev_fch_gating_req: None,
        traffic_walsh_code: Some(walsh_code),
        is_preamble_only: true,
        is_traffic_pcg_measurement: false,
        is_traffic_phy_status: false,
        traffic_measurement_age_chips: None,
        for_supported_rcs: Vec::new(),
        rev_supported_rcs: Vec::new(),
        decoded_rdsch: None,
        traffic_primary_bits: None,
        traffic_primary_rate_bps: None,
        traffic_primary_bearer_routed: false,
        traffic_voice_bits: None,
        traffic_voice_rate_bps: None,
        raw_pdu_bits: None,
    }
}

pub(super) fn build_traffic_pcg_measurement_event(
    blk: &SampleBlock,
    walsh_code: u8,
    chip_rate_hz: usize,
    batch_hw_time_ns: u64,
    processing_absolute_chip_end: u64,
) -> Option<AccessChannelEvent> {
    const TRAFFIC_PCG_CHIPS: u64 = 6 * 256;

    if blk
        .tags
        .get("traffic_pcg_measurement")
        .copied()
        .unwrap_or(0)
        != 1
    {
        return None;
    }

    let eb_nt_db = *blk.pcg_signal_snr_db.as_ref()?.first()?;
    let absolute_chip_start = blk
        .tags
        .get("absolute_chip_start")
        .copied()
        .and_then(|chip| u64::try_from(chip).ok());
    let receive_time = absolute_chip_start.map(|chip| {
        time::system_time_from_chips(chip.saturating_add(TRAFFIC_PCG_CHIPS), chip_rate_hz as u64)
    });
    let measurement_age_chips =
        absolute_chip_start.map(|chip| processing_absolute_chip_end.saturating_sub(chip));

    Some(AccessChannelEvent {
        event_id: super::next_access_event_id(),
        chip_start: blk.chip_start,
        absolute_chip_start,
        receive_time,
        preamble_frames: 0,
        pd: 0,
        message_id: MessageId::GeneralExtension,
        msg_type_name: format!("TrafficPcgMeasurement(W{})", walsh_code),
        address: None,
        resolved_address: None,
        subscriber_id: None,
        l3_summary: None,
        decoded_l3: None,
        pdu_summary: format!(
            "pcg_measurement walsh={} pilot_ec_nt_db={:.2}",
            walsh_code, eb_nt_db
        ),
        msg_seq: None,
        ack_seq: None,
        ack_req: false,
        valid_ack: false,
        msid_type: None,
        esn: None,
        imsi: None,
        meid: None,
        imsi_m_s1: None,
        imsi_m_s2: None,
        imsi_class: None,
        imsi_addr_num: None,
        imsi_mcc: None,
        imsi_11_12: None,
        mob_p_rev: None,
        slot_cycle_index: None,
        scm: None,
        burst_type: None,
        data_burst_fields: None,
        data_burst_num_msgs: None,
        data_burst_msg_number: None,
        wall_clock_us: chrono::Utc::now().timestamp_micros() as u64,
        rx_wall_time: Some(Instant::now()),
        rx_hw_time_ns: Some(batch_hw_time_ns),
        snr_db: blk.tags.get("finger_snr_mdb").map(|v| *v as f32 / 1000.0),
        signal_power_db: blk
            .tags
            .get("traffic_pcg_mobile_power_mdbfs")
            .map(|v| *v as f32 / 1000.0),
        reverse_pilot_ec_io_db: reverse_pilot_ec_io_db_from_tags(blk),
        raw_power_db: blk
            .tags
            .get("traffic_pcg_raw_power_mdb")
            .or_else(|| blk.tags.get("finger_raw_power_mdb"))
            .map(|v| *v as f32 / 1000.0),
        demod_quality_pct: None,
        pcg_signal_snr_db: Some(vec![eb_nt_db]),
        active_pcg_mask: None,
        traffic_phy_valid: None,
        traffic_fqi_valid: None,
        traffic_tail_valid: None,
        traffic_fqi_bits: None,
        traffic_ml_tail_match: None,
        order_code: None,
        service_option: None,
        for_rc_pref: None,
        rev_rc_pref: None,
        rev_fch_gating_req: None,
        traffic_walsh_code: Some(walsh_code),
        is_preamble_only: false,
        is_traffic_pcg_measurement: true,
        is_traffic_phy_status: false,
        traffic_measurement_age_chips: measurement_age_chips,
        for_supported_rcs: Vec::new(),
        rev_supported_rcs: Vec::new(),
        decoded_rdsch: None,
        traffic_primary_bits: None,
        traffic_primary_rate_bps: None,
        traffic_primary_bearer_routed: false,
        traffic_voice_bits: None,
        traffic_voice_rate_bps: None,
        raw_pdu_bits: None,
    })
}

pub(super) fn traffic_frame_validity(event: &AccessChannelEvent) -> bool {
    let is_signaling = event.message_id != MessageId::GeneralExtension;
    let primary_rate = event.traffic_primary_rate_bps.unwrap_or(0);
    if let Some(fqi_bits) = event.traffic_fqi_bits {
        let tail_valid = event.traffic_tail_valid.unwrap_or(false);
        if fqi_bits > 0 {
            tail_valid && event.traffic_fqi_valid.unwrap_or(false)
        } else if is_signaling || primary_rate >= 4800 {
            tail_valid && event.traffic_phy_valid.unwrap_or(true)
        } else {
            tail_valid && event.traffic_ml_tail_match.unwrap_or(true)
        }
    } else if is_signaling || primary_rate >= 4800 {
        event.traffic_phy_valid.unwrap_or(true)
    } else {
        event.traffic_ml_tail_match.unwrap_or(true)
    }
}

/// Extract addressing summary (IMSI/ESN/MEID) from a decoded PDU.
pub fn extract_address(pdu: &ReverseAccessPdu) -> Option<String> {
    match pdu {
        ReverseAccessPdu::Pd01PRev6(p) => p.addressing.as_ref().map(|a| a.summary()),
        _ => None,
    }
}

/// Extract Layer-3 SDU summary from a decoded PDU.
pub fn decode_access_message_from_pdu(
    pdu: &ReverseAccessPdu,
    ctx: crate::receiver::access_layer3::AccessDecodeContext,
) -> Result<AccessMessage, String> {
    match pdu {
        ReverseAccessPdu::Pd01PRev6(p) => {
            let message_id = MessageId::from_wire(
                crate::lac::message_types::WireChannel::ReverseCommon,
                p.header.msg_type,
            )
            .ok_or_else(|| format!("unsupported r-csch MSG_TAG 0x{:02X}", p.header.msg_type))?;
            let header = AccessMessageHeader {
                pd: p.header.pd,
                message_id,
            };
            AccessMessage::decode_sdu_with_context(header, &p.sdu_plus_padding_raw, ctx)
                .map_err(|err| err.to_string())
        }
        ReverseAccessPdu::Pd00Legacy(p) => {
            let message_id = MessageId::from_wire(
                crate::lac::message_types::WireChannel::ReverseCommon,
                p.header.msg_type,
            )
            .ok_or_else(|| format!("unsupported r-csch MSG_TAG 0x{:02X}", p.header.msg_type))?;
            AccessMessage::decode_sdu_with_context(
                AccessMessageHeader {
                    pd: p.header.pd,
                    message_id,
                },
                &p.sdu_plus_padding_raw,
                ctx,
            )
            .map_err(|err| err.to_string())
        }
        ReverseAccessPdu::Pd10Modern { .. } => {
            Err("PD=10 reverse-common PDU Layer 3 body decode is unsupported".to_string())
        }
    }
}

pub(super) fn extract_access_rc_preferences(
    msg: &AccessMessage,
) -> (Option<u8>, Option<u8>, Option<bool>) {
    match msg {
        AccessMessage::Origination(m) => (m.for_rc_pref, m.rev_rc_pref, m.rev_fch_gating_req),
        AccessMessage::PageResponse(m) => (m.for_rc_pref, m.rev_rc_pref, m.rev_fch_gating_req),
        _ => (None, None, None),
    }
}

/// Extract IMSI_M_S1 (24 bits) and IMSI_M_S2 (10 bits) from a 34-bit IMSI_S value.
/// Per C.S0005-E 2.3.1: IMSI_S = IMSI_S2(10 upper) || IMSI_S1(24 lower).
pub(super) fn split_imsi_s(imsi_s: u64) -> (u32, u16) {
    let imsi_m_s1 = (imsi_s & 0xFFFFFF) as u32; // lower 24 bits
    let imsi_m_s2 = ((imsi_s >> 24) & 0x3FF) as u16; // upper 10 bits
    (imsi_m_s1, imsi_m_s2)
}

/// Derive the full IMSI string from access channel identity fields.
pub fn derive_full_imsi_from_access_identity(
    imsi_m_s1: Option<u32>,
    imsi_m_s2: Option<u16>,
    imsi_class: Option<u8>,
    imsi_mcc: Option<u16>,
    imsi_11_12: Option<u8>,
    overhead_mcc: u16,
    overhead_imsi_11_12: u8,
) -> Option<String> {
    let imsi_s = imsi_s_to_digits_checked(imsi_m_s1?, imsi_m_s2?)?;

    let fallback_mcc = if imsi_class == Some(0) && overhead_mcc <= 999 {
        Some(overhead_mcc)
    } else {
        None
    };
    let fallback_imsi_11_12 = if imsi_class == Some(0) && overhead_imsi_11_12 <= 99 {
        Some(overhead_imsi_11_12)
    } else {
        None
    };

    let mcc = mcc_to_digits(imsi_mcc.or(fallback_mcc)?)?;
    let imsi_11_12 = imsi_11_12_to_digits(imsi_11_12.or(fallback_imsi_11_12)?)?;
    Some(format!("{mcc}{imsi_11_12}{imsi_s}"))
}

/// Extracted IMSI fields from the class-specific MSID encoding.
pub(super) struct ImsiFields {
    pub(super) imsi_class: u8,
    pub(super) imsi_m_s1: u32,
    pub(super) imsi_m_s2: u16,
    pub(super) imsi_addr_num: Option<u8>,
    pub(super) mcc: Option<u16>,
    pub(super) imsi_11_12: Option<u8>,
}

/// Try to extract IMSI_S (34 bits) and optional MCC/IMSI_11_12 from class-specific IMSI fields.
/// Handles both class 0 and class 1 IMSI encodings, retaining enough detail
/// to page later by IMSI or ESN as appropriate.
pub(super) fn extract_imsi_from_class_fields(
    bits: &mut cdma_common::bits::Bitstream,
) -> Option<ImsiFields> {
    let imsi_class = bits.read_bits(1).ok()? as u8;
    match imsi_class {
        0 => {
            let class0_type = bits.read_bits(2).ok()? as u8;
            match class0_type {
                0b00 => {
                    // reserved(3) + IMSI_S(34)
                    let _ = bits.read_bits(3).ok()?;
                    let imsi_s = bits.read_bits(34).ok()?;
                    let (s1, s2) = split_imsi_s(imsi_s);
                    Some(ImsiFields {
                        imsi_class,
                        imsi_m_s1: s1,
                        imsi_m_s2: s2,
                        imsi_addr_num: None,
                        mcc: None,
                        imsi_11_12: None,
                    })
                }
                0b01 => {
                    // reserved(4) + IMSI_11_12(7) + IMSI_S(34)
                    let _ = bits.read_bits(4).ok()?;
                    let imsi_11_12 = bits.read_bits(7).ok()? as u8;
                    let imsi_s = bits.read_bits(34).ok()?;
                    let (s1, s2) = split_imsi_s(imsi_s);
                    Some(ImsiFields {
                        imsi_class,
                        imsi_m_s1: s1,
                        imsi_m_s2: s2,
                        imsi_addr_num: None,
                        mcc: None,
                        imsi_11_12: Some(imsi_11_12),
                    })
                }
                0b10 => {
                    // reserved(1) + MCC(10) + IMSI_S(34)
                    let _ = bits.read_bits(1).ok()?;
                    let mcc = bits.read_bits(10).ok()? as u16;
                    let imsi_s = bits.read_bits(34).ok()?;
                    let (s1, s2) = split_imsi_s(imsi_s);
                    Some(ImsiFields {
                        imsi_class,
                        imsi_m_s1: s1,
                        imsi_m_s2: s2,
                        imsi_addr_num: None,
                        mcc: Some(mcc),
                        imsi_11_12: None,
                    })
                }
                0b11 => {
                    // reserved(2) + MCC(10) + IMSI_11_12(7) + IMSI_S(34)
                    let _ = bits.read_bits(2).ok()?;
                    let mcc = bits.read_bits(10).ok()? as u16;
                    let imsi_11_12 = bits.read_bits(7).ok()? as u8;
                    let imsi_s = bits.read_bits(34).ok()?;
                    let (s1, s2) = split_imsi_s(imsi_s);
                    Some(ImsiFields {
                        imsi_class,
                        imsi_m_s1: s1,
                        imsi_m_s2: s2,
                        imsi_addr_num: None,
                        mcc: Some(mcc),
                        imsi_11_12: Some(imsi_11_12),
                    })
                }
                _ => None,
            }
        }
        1 => {
            let class1_type = bits.read_bits(1).ok()? as u8;
            match class1_type {
                0 => {
                    // reserved(2) + IMSI_ADDR_NUM(3) + IMSI_11_12(7) + IMSI_S(34)
                    let _ = bits.read_bits(2).ok()?;
                    let imsi_addr_num = bits.read_bits(3).ok()? as u8;
                    let imsi_11_12 = bits.read_bits(7).ok()? as u8;
                    let imsi_s = bits.read_bits(34).ok()?;
                    let (s1, s2) = split_imsi_s(imsi_s);
                    Some(ImsiFields {
                        imsi_class,
                        imsi_m_s1: s1,
                        imsi_m_s2: s2,
                        imsi_addr_num: Some(imsi_addr_num),
                        mcc: None,
                        imsi_11_12: Some(imsi_11_12),
                    })
                }
                1 => {
                    // IMSI_ADDR_NUM(3) + MCC(10) + IMSI_11_12(7) + IMSI_S(34)
                    let imsi_addr_num = bits.read_bits(3).ok()? as u8;
                    let mcc = bits.read_bits(10).ok()? as u16;
                    let imsi_11_12 = bits.read_bits(7).ok()? as u8;
                    let imsi_s = bits.read_bits(34).ok()?;
                    let (s1, s2) = split_imsi_s(imsi_s);
                    Some(ImsiFields {
                        imsi_class,
                        imsi_m_s1: s1,
                        imsi_m_s2: s2,
                        imsi_addr_num: Some(imsi_addr_num),
                        mcc: Some(mcc),
                        imsi_11_12: Some(imsi_11_12),
                    })
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// Structured addressing fields extracted from a reverse-link PDU.
pub struct ExtractedAddr {
    pub msid_type: Option<u8>,
    pub esn: Option<u32>,
    pub meid: Option<String>,
    pub imsi_m_s1: Option<u32>,
    pub imsi_m_s2: Option<u16>,
    pub imsi_class: Option<u8>,
    pub imsi_addr_num: Option<u8>,
    pub mcc: Option<u16>,
    pub imsi_11_12: Option<u8>,
}

/// Extract structured addressing fields from a decoded PD=01 PDU for BSC use.
pub fn extract_addressing_fields(
    addr: Option<&crate::receiver::access_pdu::RcschAddressingFields>,
) -> ExtractedAddr {
    let Some(addr) = addr else {
        return ExtractedAddr {
            msid_type: None,
            esn: None,
            meid: None,
            imsi_m_s1: None,
            imsi_m_s2: None,
            imsi_class: None,
            imsi_addr_num: None,
            mcc: None,
            imsi_11_12: None,
        };
    };
    let msid_type = Some(addr.msid_type);
    let mut esn = None;
    let mut meid = None;
    let mut imsi_m_s1 = None;
    let mut imsi_m_s2 = None;
    let mut imsi_class = None;
    let mut imsi_addr_num = None;
    let mut mcc = None;
    let mut imsi_11_12 = None;

    let apply_imsi = |fields: &ImsiFields,
                      s1: &mut Option<u32>,
                      s2: &mut Option<u16>,
                      class: &mut Option<u8>,
                      addr_num: &mut Option<u8>,
                      m: &mut Option<u16>,
                      i: &mut Option<u8>| {
        *s1 = Some(fields.imsi_m_s1);
        *s2 = Some(fields.imsi_m_s2);
        *class = Some(fields.imsi_class);
        *addr_num = fields.imsi_addr_num;
        *m = fields.mcc;
        *i = fields.imsi_11_12;
    };

    match addr.msid_type {
        0b001 if addr.msid_raw.len() >= 32 => {
            // ESN only
            let mut bits = addr.msid_raw.clone();
            esn = bits.read_bits(32).ok().map(|v| v as u32);
        }
        0b000 if addr.msid_raw.len() >= 66 => {
            // IMSI_S + ESN: IMSI_M_S1(24) + IMSI_M_S2(10) + ESN(32)
            let mut bits = addr.msid_raw.clone();
            imsi_m_s1 = bits.read_bits(24).ok().map(|v| v as u32);
            imsi_m_s2 = bits.read_bits(10).ok().map(|v| v as u16);
            esn = bits.read_bits(32).ok().map(|v| v as u32);
        }
        0b010 if addr.msid_raw.len() >= 1 => {
            // IMSI only: IMSI_CLASS(1) + class-specific fields
            let mut bits = addr.msid_raw.clone();
            if let Some(fields) = extract_imsi_from_class_fields(&mut bits) {
                apply_imsi(
                    &fields,
                    &mut imsi_m_s1,
                    &mut imsi_m_s2,
                    &mut imsi_class,
                    &mut imsi_addr_num,
                    &mut mcc,
                    &mut imsi_11_12,
                );
            }
        }
        0b011 if addr.msid_raw.len() >= 33 => {
            // IMSI + ESN: ESN(32) + IMSI_CLASS(1) + class-specific fields
            let mut bits = addr.msid_raw.clone();
            esn = bits.read_bits(32).ok().map(|v| v as u32);
            if let Some(fields) = extract_imsi_from_class_fields(&mut bits) {
                apply_imsi(
                    &fields,
                    &mut imsi_m_s1,
                    &mut imsi_m_s2,
                    &mut imsi_class,
                    &mut imsi_addr_num,
                    &mut mcc,
                    &mut imsi_11_12,
                );
            }
        }
        0b100 => {
            // Extended MSID (MEID, IMSI+MEID, IMSI+ESN+MEID)
            match addr.ext_msid_type {
                Some(0b000) if addr.msid_raw.len() >= 56 => {
                    let mut bits = addr.msid_raw.clone();
                    meid = bits.read_bits(56).ok().map(|v| format!("{v:014x}"));
                }
                Some(0b010) if addr.msid_raw.len() >= 32 => {
                    // IMSI+ESN+MEID: ESN(32) + MEID(56) + IMSI
                    let mut bits = addr.msid_raw.clone();
                    esn = bits.read_bits(32).ok().map(|v| v as u32);
                    meid = bits.read_bits(56).ok().map(|v| format!("{v:014x}"));
                    if let Some(fields) = extract_imsi_from_class_fields(&mut bits) {
                        apply_imsi(
                            &fields,
                            &mut imsi_m_s1,
                            &mut imsi_m_s2,
                            &mut imsi_class,
                            &mut imsi_addr_num,
                            &mut mcc,
                            &mut imsi_11_12,
                        );
                    }
                }
                Some(0b001) if addr.msid_raw.len() >= 56 => {
                    // IMSI+MEID: MEID(56) + IMSI
                    let mut bits = addr.msid_raw.clone();
                    meid = bits.read_bits(56).ok().map(|v| format!("{v:014x}"));
                    if let Some(fields) = extract_imsi_from_class_fields(&mut bits) {
                        apply_imsi(
                            &fields,
                            &mut imsi_m_s1,
                            &mut imsi_m_s2,
                            &mut imsi_class,
                            &mut imsi_addr_num,
                            &mut mcc,
                            &mut imsi_11_12,
                        );
                    }
                }
                _ => {}
            }
        }
        _ => {}
    }

    ExtractedAddr {
        msid_type,
        esn,
        meid,
        imsi_m_s1,
        imsi_m_s2,
        imsi_class,
        imsi_addr_num,
        mcc,
        imsi_11_12,
    }
}

pub(super) fn log_access_preamble_event(blk: &SampleBlock, chip_rate_hz: usize) {
    let abs_chip = blk.tags.get("absolute_chip_start").copied().unwrap_or(-1);
    let (abs_sys_time, abs_t20) = if abs_chip >= 0 {
        let sys_time = time::system_time_from_chips(abs_chip as u64, chip_rate_hz as u64);
        (
            sys_time.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            time::system_time_20ms_frames(sys_time),
        )
    } else {
        ("<unknown>".to_string(), 0)
    };
    info!(
        "access_preamble_event: chip={} preamble_frames={} info_ones={} pilot_phase={} pn_phase={} abs_chip={} abs_sys_time={} abs_t20={} lc_acquired={} lc_delta={}",
        blk.chip_start,
        blk.tags.get("access_preamble_frames").copied().unwrap_or(0),
        blk.tags
            .get("access_preamble_info_ones")
            .copied()
            .unwrap_or(-1),
        blk.tags.get("pilot_phase").copied().unwrap_or(-1),
        blk.tags.get("pn_phase").copied().unwrap_or(-1),
        abs_chip,
        abs_sys_time,
        abs_t20,
        blk.tags
            .get("reverse_access_lc_acquired")
            .copied()
            .unwrap_or(0),
        blk.tags
            .get("reverse_access_lc_chip_delta")
            .copied()
            .unwrap_or(0),
    );
}
