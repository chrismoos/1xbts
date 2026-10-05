use super::modulation::CHIPS_PER_FRAME;
use super::*;
use cdma_bts::receiver::access::AccessFrameReader;
use cdma_bts::receiver::access_pdu::ReverseAccessPdu;
use cdma_common::phy::long_code::LongCodeGenerator;
use num_complex::Complex32;

#[test]
fn registration_pdu_round_trips_through_decoder() {
    let id = MsAccessIdentity {
        esn: 0x1234_5678,
        imsi_s: 1234567890,
        ..Default::default()
    };
    let pdu = encode_registration_pdu(&id, 1, &AccessArq::assured(3), &ServingSystem::at_p_rev(6));
    let decoded = ReverseAccessPdu::decode(&pdu).expect("PDU should decode");
    match decoded {
        ReverseAccessPdu::Pd01PRev6(p) => {
            assert_eq!(p.header.msg_type, MSG_TYPE_REGISTRATION);
            let arq = p.arq.expect("ARQ parsed");
            assert_eq!(arq.msg_seq, 3);
            let addr = p.addressing.expect("addressing parsed");
            assert_eq!(addr.msid_type, MSID_TYPE_IMSI_ESN);
            let mut msid = addr.msid_raw.clone();
            assert_eq!(
                msid.read_bits(32).map(|v| v as u32).unwrap(),
                0x1234_5678,
                "decoded ESN should match"
            );
            assert!(p.warnings.is_empty(), "decoder warnings: {:?}", p.warnings);
        }
        other => panic!("expected a PD=01 P_REV 6 PDU, got {other:?}"),
    }
}

fn decode_counts_with_settings(
    iq: Vec<Complex32>,
    frame_start_chip: u64,
    msg_type: u8,
    settings: cdma_bts::receiver::pipelined::ReverseAccessSettings,
) -> (usize, usize) {
    use cdma_bts::receiver::pipelined::{
        PipelineProcessorShared, PipelinedReceiver, reverse_access_chain,
    };

    let chain: Vec<PipelineProcessorShared> = reverse_access_chain(settings);
    let mut rx = PipelinedReceiver::new(iq.into_iter())
        .with_input_sample_rate_hz((1_228_800 * 4) as f64)
        .with_absolute_sample_start(frame_start_chip * 4);
    let out = rx.add_pipeline(chain);
    rx.run_pipeline().unwrap();

    let (mut total, mut valid) = (0usize, 0usize);
    for blocks in out {
        for blk in &blocks {
            if blk.tags.get("access_event") == Some(&1) {
                total += 1;
                if blk.tags.get("access_crc_valid") == Some(&1)
                    && blk.tags.get("access_msg_type") == Some(&(msg_type as i64))
                {
                    valid += 1;
                }
            }
        }
    }
    (total, valid)
}

fn decode_counts(iq: Vec<Complex32>, frame_start_chip: u64, msg_type: u8) -> (usize, usize) {
    decode_counts_with_settings(
        iq,
        frame_start_chip,
        msg_type,
        cdma_bts::receiver::pipelined::ReverseAccessSettings {
            rake_fast_path: false,
            ..Default::default()
        },
    )
}

#[test]
fn long_code_state_rewinds_before_sync_validity() {
    use cdma_common::consts::{SR1_CHIPS_PER_80MS, SR1_CHIPS_PER_FRAME, SR1_PCGS_PER_FRAME};
    const FRAME_OFFSET: u64 = 7;
    let pcg_chips = SR1_CHIPS_PER_FRAME / SR1_PCGS_PER_FRAME as u64;
    let earlier_chip = FRAME_OFFSET * pcg_chips - 1;
    let mut earlier = LongCodeGenerator::new(0);
    for _ in 0..earlier_chip {
        earlier.next_chip();
    }
    let mut anchor = earlier.clone();
    anchor.advance_chips((SR1_CHIPS_PER_80MS - earlier_chip) as usize);
    for chip in [
        earlier_chip,
        SR1_CHIPS_PER_80MS - 1,
        SR1_CHIPS_PER_80MS,
        SR1_CHIPS_PER_80MS + SR1_CHIPS_PER_FRAME,
    ] {
        let mut expected = earlier.clone();
        expected.advance_chips((chip - earlier_chip) as usize);
        assert_eq!(
            lc_state_at(anchor.state(), SR1_CHIPS_PER_80MS, chip),
            expected.state(),
            "chip {chip}"
        );
    }
}

#[test]
fn access_filter_input_has_half_chip_offset_impulses() {
    let cfg = AccessChannelConfig::default();
    let frame_chip = CHIPS_PER_FRAME as u64;
    let samples =
        modulate_access_probe(&cfg, &[], frame_chip, default_lc_state_at(frame_chip), 1, 0);
    for (index, sample) in samples.iter().enumerate() {
        assert_eq!(
            sample.re.abs(),
            f32::from(index % cfg.oversample == 0),
            "I sample {index}",
        );
        assert_eq!(
            sample.im.abs(),
            f32::from(index % cfg.oversample == cfg.oversample / 2),
            "Q sample {index}",
        );
    }
}

#[test]
fn modulated_probe_decodes_registration_offline() {
    let _ = env_logger::builder().is_test(true).try_init();
    let id = MsAccessIdentity {
        esn: 0x4CDC_1D09,
        ..Default::default()
    };
    let capsule =
        build_registration_capsule(&id, 1, &AccessArq::assured(0), &ServingSystem::at_p_rev(6));
    let cfg = AccessChannelConfig::default();
    let frame_start_chip = CHIPS_PER_FRAME as u64 * 100;
    let iq = pulse_shape(&modulate_access_probe(
        &cfg,
        capsule.bits(),
        frame_start_chip,
        default_lc_state_at(frame_start_chip),
        8,
        4,
    ));

    let (total, valid_reg) = decode_counts(iq, frame_start_chip, MSG_TYPE_REGISTRATION);
    eprintln!("offline decode: total_access_events={total} crc_valid_registration={valid_reg}");
    assert!(
        valid_reg >= 1,
        "modulated probe should decode as a CRC-valid Registration (total_events={total}, valid_reg={valid_reg})"
    );
}

#[test]
fn modulated_probe_decodes_registration_with_pilot_pn_510() {
    let id = MsAccessIdentity {
        esn: 0x1234_5678,
        ..Default::default()
    };
    let capsule =
        build_registration_capsule(&id, 1, &AccessArq::assured(0), &ServingSystem::at_p_rev(6));
    let cfg = AccessChannelConfig {
        base_id: 16040,
        pilot_pn: 510,
        ..Default::default()
    };
    let frame_start_chip = CHIPS_PER_FRAME as u64 * 100;
    let iq = pulse_shape(&modulate_access_probe(
        &cfg,
        capsule.bits(),
        frame_start_chip,
        default_lc_state_at(frame_start_chip),
        4,
        4,
    ));
    let (total, valid_reg) = decode_counts_with_settings(
        iq,
        frame_start_chip,
        MSG_TYPE_REGISTRATION,
        cdma_bts::receiver::pipelined::ReverseAccessSettings {
            base_id: cfg.base_id,
            pilot_pn: cfg.pilot_pn,
            rake_fast_path: false,
            ..Default::default()
        },
    );
    eprintln!(
        "PN510 offline decode: total_access_events={total} crc_valid_registration={valid_reg}"
    );
    assert!(valid_reg >= 1);
}

#[test]
fn modulated_page_response_decodes_offline() {
    let id = MsAccessIdentity {
        esn: 0x4CDC_1D09,
        ..Default::default()
    };
    let capsule =
        build_page_response_capsule(&id, 6, &AccessArq::assured(1), &ServingSystem::at_p_rev(6));
    let cfg = AccessChannelConfig::default();
    let frame_start_chip = CHIPS_PER_FRAME as u64 * 100;
    let iq = pulse_shape(&modulate_access_probe(
        &cfg,
        capsule.bits(),
        frame_start_chip,
        default_lc_state_at(frame_start_chip),
        8,
        4,
    ));

    let (total, valid) = decode_counts(iq, frame_start_chip, MSG_TYPE_PAGE_RESPONSE);
    eprintln!("offline page-response decode: total_access_events={total} crc_valid={valid}");
    assert!(
        valid >= 1,
        "modulated page response should decode as a CRC-valid Page Response (total_events={total}, valid={valid})"
    );
}

#[test]
fn page_response_pdu_round_trips_through_decoder() {
    use cdma_common::access::{AccessDecodeContext, AccessMessage, AccessMessageHeader};
    use cdma_common::lac::message_types::MessageId;

    let id = MsAccessIdentity {
        esn: 0x4CDC_1D09,
        ..Default::default()
    };
    let so = 6u16;
    let pdu =
        encode_page_response_pdu(&id, so, &AccessArq::assured(2), &ServingSystem::at_p_rev(6));
    let decoded = ReverseAccessPdu::decode(&pdu).expect("PDU should decode");
    let ReverseAccessPdu::Pd01PRev6(p) = decoded else {
        panic!("expected a PD=01 P_REV 6 PDU");
    };
    assert_eq!(p.header.msg_type, MSG_TYPE_PAGE_RESPONSE);
    assert!(p.warnings.is_empty(), "decoder warnings: {:?}", p.warnings);
    let mut msid = p.addressing.expect("addressing").msid_raw;
    assert_eq!(msid.read_bits(32).map(|v| v as u32).unwrap(), id.esn);

    let header = AccessMessageHeader {
        pd: 1,
        message_id: MessageId::PageResponse,
    };
    let msg = AccessMessage::decode_sdu_with_context(
        header,
        &p.sdu_plus_padding_raw,
        AccessDecodeContext::new(Some(0), Some(6)),
    )
    .expect("SDU should decode");
    match msg {
        AccessMessage::PageResponse(pr) => {
            assert_eq!(pr.service_option, so);
            assert_eq!(pr.mob_p_rev, 6);
            assert_eq!(pr.ch_ind, Some(CH_IND_FCH));
            assert_eq!(pr.for_rc_pref, Some(PREFERRED_RC));
            assert_eq!(pr.rev_rc_pref, Some(PREFERRED_RC));
            let fch = pr.fch_capability.expect("FCH capability");
            assert_eq!(fch.for_supported_rcs, vec![1, 2, 3]);
            assert_eq!(pr.dcch_supported, Some(false));
        }
        other => panic!("expected PageResponse, got {other:?}"),
    }
}

#[test]
fn imsi_class_0_type_leaves_out_what_the_base_station_already_broadcasts() {
    let id = MsAccessIdentity {
        esn: 0x4CDC_1D09,
        imsi_s: 1234567890,
        ..Default::default()
    };
    let mcc = mcc_from_digits(&id.mcc).unwrap();
    let imsi_11_12 = imsi_11_12_from_digits(&id.imsi_11_12).unwrap();
    // (broadcast MCC, broadcast IMSI_11_12, expected type, MSID octets)
    let cases = [
        (Some(mcc), Some(imsi_11_12), 0b00u64, 9u8),
        (Some(mcc), Some(imsi_11_12 ^ 1), 0b01, 10),
        (Some(mcc ^ 1), Some(imsi_11_12), 0b10, 10),
        (Some(mcc ^ 1), Some(imsi_11_12 ^ 1), 0b11, 11),
        (None, None, 0b11, 11),
    ];
    for (base_mcc, base_imsi_11_12, class0_type, octets) in cases {
        let serving = ServingSystem {
            base_mcc,
            base_imsi_11_12,
            ..ServingSystem::at_p_rev(6)
        };
        let pdu = encode_registration_pdu(&id, 1, &AccessArq::assured(0), &serving);
        let ReverseAccessPdu::Pd01PRev6(p) =
            ReverseAccessPdu::decode(&pdu).expect("PDU should decode")
        else {
            panic!("expected a PD=01 P_REV 6 PDU");
        };
        assert!(p.warnings.is_empty(), "decoder warnings: {:?}", p.warnings);
        let addr = p.addressing.expect("addressing parsed");
        assert_eq!(addr.msid_len_octets, octets);
        let mut msid = addr.msid_raw.clone();
        assert_eq!(msid.read_bits(32).unwrap() as u32, id.esn);
        assert_eq!(msid.read_bits(1).unwrap(), 0, "IMSI_CLASS 0");
        assert_eq!(msid.read_bits(2).unwrap(), class0_type);
    }
}

#[test]
fn access_pdus_use_the_legacy_wrapper_below_p_rev_6() {
    let id = MsAccessIdentity {
        esn: 0x4CDC_1D09,
        ..Default::default()
    };
    let arq = AccessArq::assured(1);
    for pdu in [
        encode_registration_pdu(&id, 1, &arq, &ServingSystem::at_p_rev(5)),
        encode_page_response_pdu(&id, 6, &arq, &ServingSystem::at_p_rev(5)),
        encode_ms_ack_order_pdu(&id, &arq, &ServingSystem::at_p_rev(5)),
    ] {
        let decoded = ReverseAccessPdu::decode(&pdu).expect("PDU should decode");
        assert!(
            matches!(decoded, ReverseAccessPdu::Pd00Legacy(_)),
            "expected PD=00 below P_REV 6, got {decoded:?}"
        );
    }
    let ack = encode_ms_ack_order_pdu(&id, &arq, &ServingSystem::at_p_rev(6));
    assert!(matches!(
        ReverseAccessPdu::decode(&ack).expect("PDU should decode"),
        ReverseAccessPdu::Pd01PRev6(_)
    ));
}

#[test]
fn origination_pdu_round_trips_through_decoder() {
    use cdma_common::access::{AccessDecodeContext, AccessMessage, AccessMessageHeader};
    use cdma_common::lac::message_types::MessageId;
    use cdma_common::paging::imsi_s_to_digits;

    let id = MsAccessIdentity {
        esn: 0x4CDC_1D09,
        imsi_s: 1234567890,
        ..Default::default()
    };
    let pdu = encode_origination_pdu(
        &id,
        SERVICE_OPTION_SMS,
        "2222",
        &AccessArq::assured(3),
        &ServingSystem::at_p_rev(6),
    );
    let decoded = ReverseAccessPdu::decode(&pdu).expect("PDU should decode");
    let ReverseAccessPdu::Pd01PRev6(p) = decoded else {
        panic!("expected PD=01 P_REV6 PDU");
    };
    assert_eq!(p.header.msg_type, MSG_TYPE_ORIGINATION);

    let addr = p.addressing.expect("addressing");
    assert_eq!(addr.msid_type, MSID_TYPE_IMSI_ESN);
    let mut msid = addr.msid_raw;
    let esn = msid.read_bits(32).unwrap() as u32;
    assert_eq!(esn, id.esn);
    assert_eq!(msid.read_bits(1).unwrap(), 0, "IMSI_CLASS 0");
    assert_eq!(msid.read_bits(2).unwrap(), 0b11, "CLASS0_TYPE 0b11");
    let _reserved = msid.read_bits(2).unwrap();
    let _mcc = msid.read_bits(10).unwrap();
    let _imsi_11_12 = msid.read_bits(7).unwrap();
    let imsi_m_s2 = msid.read_bits(10).unwrap() as u16;
    let imsi_m_s1 = msid.read_bits(24).unwrap() as u32;
    assert_eq!(imsi_s_to_digits(imsi_m_s1, imsi_m_s2), "1234567890");

    let header = AccessMessageHeader {
        pd: 0,
        message_id: MessageId::Origination,
    };
    let msg = AccessMessage::decode_sdu_with_context(
        header,
        &p.sdu_plus_padding_raw,
        AccessDecodeContext::new(Some(0), Some(6)),
    )
    .expect("SDU should decode");
    match msg {
        AccessMessage::Origination(o) => {
            assert_eq!(o.service_option, Some(SERVICE_OPTION_SMS));
            assert_eq!(o.digits, vec![2, 2, 2, 2]);
            assert!(!o.digit_mode);
            assert_eq!(o.mob_p_rev, 6);
            assert_eq!(o.for_rc_pref, Some(3));
            assert_eq!(o.rev_rc_pref, Some(3));
            let fch = o.fch_capability.expect("FCH capability present");
            assert!(fch.for_supported_rcs.contains(&3), "advertises forward RC3");
            assert!(fch.rev_supported_rcs.contains(&3), "advertises reverse RC3");
        }
        other => panic!("expected Origination, got {other:?}"),
    }
}

#[test]
fn modulated_origination_decodes_offline() {
    let id = MsAccessIdentity {
        esn: 0x4CDC_1D09,
        ..Default::default()
    };
    let capsule = build_origination_capsule(
        &id,
        SERVICE_OPTION_SMS,
        "2222",
        &AccessArq::assured(3),
        &ServingSystem::at_p_rev(6),
    );
    let cfg = AccessChannelConfig::default();
    let frame_start_chip = CHIPS_PER_FRAME as u64 * 100;
    let iq = pulse_shape(&modulate_access_probe(
        &cfg,
        capsule.bits(),
        frame_start_chip,
        default_lc_state_at(frame_start_chip),
        16,
        4,
    ));

    let (total, valid) = decode_counts(iq, frame_start_chip, MSG_TYPE_ORIGINATION);
    eprintln!("offline origination decode: total_access_events={total} crc_valid={valid}");
    assert!(
        valid >= 1,
        "modulated origination should decode as a CRC-valid Origination (total_events={total}, valid={valid})"
    );
}

#[test]
fn origination_after_registration_decodes_offline() {
    let id = MsAccessIdentity {
        esn: 0x4CDC_1D09,
        ..Default::default()
    };
    let serving = ServingSystem::at_p_rev(6);
    let cfg = AccessChannelConfig::default();
    let registration_chip = CHIPS_PER_FRAME as u64 * 100;
    let origination_chip = CHIPS_PER_FRAME as u64 * 126;
    let registration = build_registration_capsule(&id, 1, &AccessArq::assured(0), &serving);
    let origination = build_origination_capsule(
        &id,
        SERVICE_OPTION_SMS,
        "2222",
        &AccessArq::assured(0),
        &serving,
    );
    let mut iq = pulse_shape(&modulate_access_probe(
        &cfg,
        registration.bits(),
        registration_chip,
        default_lc_state_at(registration_chip),
        16,
        3,
    ));
    let origination_offset = ((origination_chip - registration_chip) * 4) as usize;
    assert!(iq.len() <= origination_offset);
    iq.resize(origination_offset, Complex32::new(0.0, 0.0));
    iq.extend(pulse_shape(&modulate_access_probe(
        &cfg,
        origination.bits(),
        origination_chip,
        default_lc_state_at(origination_chip),
        16,
        4,
    )));

    let (total, valid) = decode_counts(iq, registration_chip, MSG_TYPE_ORIGINATION);
    assert!(
        valid >= 1,
        "origination following registration should decode (total_events={total}, valid={valid})"
    );
}

#[test]
fn registration_capsule_crc_valid() {
    let id = MsAccessIdentity {
        esn: 0x4CDC_1D09,
        ..Default::default()
    };
    let capsule =
        build_registration_capsule(&id, 1, &AccessArq::assured(0), &ServingSystem::at_p_rev(6));
    assert_eq!(capsule.len() % 8, 0);

    let mut reader = AccessFrameReader::new();
    let mut frag = capsule;
    let frame = reader
        .process(&mut frag)
        .expect("frame reader ok")
        .expect("frame reassembled");
    assert!(
        frame.crc_valid,
        "encapsulated capsule must have valid CRC-30"
    );
}
