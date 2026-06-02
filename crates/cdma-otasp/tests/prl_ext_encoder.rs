//! Synthetic encoder/decoder round-trip for Extended PRL paths that no
//! real-carrier fixture exercises (per the coverage audit in
//! `examples/prl_coverage.rs`):
//!
//! - ACQ types `0x07`, `0x08` (JTACS — Japanese-only carriers)
//! - ACQ type `0x09` (Band Class 6 / 2 GHz — Korea/China)
//! - ACQ type `0x0A` (Generic 1x/IS-95)
//! - ACQ types `0x0F`, `0x10` (UMB — never commercially deployed)
//! - SYS record type `0x03` (MCC-MNC — international PRLs)
//! - Common Subnet Table records (no HRPD subnet sharing in our 5
//!   real Extended fixtures)
//!
//! These tests use the encoder itself as the source of truth — they're
//! round-trip sanity checks, not ground-truth verification. The
//! ground-truth tests live in `tests/encoder_round_trip.rs` and
//! consume real bytes from real carriers.

use cdma_otasp::param::prl::{
    AbSelection, PrefNeg, Priority, RoamingIndicator, StandardChannelSelection,
};
use cdma_otasp::param::prl_ext::{
    BandClassChannel, CommonSubnetRecord, ExtAcquisitionBody, ExtAcquisitionRecord,
    ExtSystemAssociation, ExtSystemId, ExtSystemRecord, ExtSystemRecordType, ExtendedPrl,
    MccMncSubnet, MccMncSubtype, SidNidPair, UmbAcqProfile, UmbBlock,
};

fn empty_prl(
    acquisition_records: Vec<ExtAcquisitionRecord>,
    common_subnet_records: Vec<CommonSubnetRecord>,
    system_records: Vec<ExtSystemRecord>,
) -> ExtendedPrl {
    ExtendedPrl {
        pr_list_size: 0, // patched by encoder
        pr_list_id: 0xC0DE,
        cur_sspr_p_rev: 0x03,
        pref_only: false,
        def_roam_ind: RoamingIndicator::OnHome,
        acquisition_records,
        common_subnet_records,
        system_records,
        pr_list_crc: 0,
        computed_crc: 0,
    }
}

fn round_trip(prl: ExtendedPrl) {
    let bytes = prl.encode().expect("encode");
    let decoded = cdma_otasp::param::prl_ext::decode(&bytes).expect("decode");
    assert!(decoded.crc_ok(), "CRC mismatch after round-trip");
    let reencoded = decoded.encode().expect("re-encode");
    assert_eq!(bytes, reencoded, "second encode must match first");
    // Field-level equality: pr_list_size and CRC are computed by the
    // encoder so we expect those to differ from the constructor input
    // on the first pass but match on the second.
    let decoded2 = cdma_otasp::param::prl_ext::decode(&reencoded).expect("decode2");
    assert_eq!(decoded, decoded2);
}

fn acq(acq_type_raw: u8, length: u8, body: ExtAcquisitionBody) -> ExtAcquisitionRecord {
    ExtAcquisitionRecord {
        acq_type_raw,
        length,
        body,
    }
}

fn sys_cdma2000_minimal(acq_index: u16) -> ExtSystemRecord {
    ExtSystemRecord {
        sys_record_length: 6,
        sys_record_type: ExtSystemRecordType::Cdma2000,
        pref_neg: PrefNeg::Preferred,
        same_geo_as_prev: false,
        priority: Priority::MoreDesirable,
        acq_index,
        system_id: ExtSystemId::Cdma2000 {
            nid_incl: cdma_otasp::param::prl::NidInclusion::AnyNid,
            sid: 22,
            nid: None,
        },
        roaming_indicator: Some(RoamingIndicator::OnHome),
        association: None,
    }
}

// ----------------------------------------------------------------------------
// Acquisition record coverage
// ----------------------------------------------------------------------------

#[test]
fn jtacs_standard_round_trip() {
    // 2 + 2 = 4 bits body, padded to one octet → LENGTH = 1.
    round_trip(empty_prl(
        vec![acq(
            0x07,
            1,
            ExtAcquisitionBody::JtacsCdmaStandard {
                ab: AbSelection::SystemA,
                pri_sec: StandardChannelSelection::Primary,
            },
        )],
        vec![],
        vec![sys_cdma2000_minimal(0)],
    ));
}

#[test]
fn jtacs_custom_round_trip() {
    // NUM_CHANS(5) + 3 × CHAN(11) = 38 bits → 5 octets.
    round_trip(empty_prl(
        vec![acq(
            0x08,
            5,
            ExtAcquisitionBody::JtacsCdmaCustom {
                channels: vec![100, 200, 1500],
            },
        )],
        vec![],
        vec![sys_cdma2000_minimal(0)],
    ));
}

#[test]
fn band_class6_round_trip() {
    // NUM_CHANS(5) + 2 × CHAN(11) = 27 bits → 4 octets.
    round_trip(empty_prl(
        vec![acq(
            0x09,
            4,
            ExtAcquisitionBody::BandClass6UsingChannels {
                channels: vec![900, 1800],
            },
        )],
        vec![],
        vec![sys_cdma2000_minimal(0)],
    ));
}

#[test]
fn generic_1x_is95_round_trip() {
    // 3 × {BAND_CLASS(5) + CHANNEL_NUMBER(11)} = 48 bits → 6 octets.
    round_trip(empty_prl(
        vec![acq(
            0x0A,
            6,
            ExtAcquisitionBody::Generic1xIs95 {
                entries: vec![
                    BandClassChannel {
                        band_class: 0,
                        channel_number: 283,
                    },
                    BandClassChannel {
                        band_class: 1,
                        channel_number: 425,
                    },
                    BandClassChannel {
                        band_class: 6,
                        channel_number: 250,
                    },
                ],
            },
        )],
        vec![],
        vec![sys_cdma2000_minimal(0)],
    ));
}

#[test]
fn umb_common_table_round_trip() {
    // 2 × {UMB_ACQ_PROFILE(6) + FFT_SIZE(4) + CPL(3) + NGS(7)} = 40
    // bits → 5 octets. No trailing RESERVED needed.
    round_trip(empty_prl(
        vec![acq(
            0x0F,
            5,
            ExtAcquisitionBody::UmbCommonTable {
                entries: vec![
                    UmbAcqProfile {
                        umb_acq_profile: 1,
                        fft_size: 0b0100,
                        cyclic_prefix_length: 0b010,
                        num_guard_subcarriers: 0b0010000,
                    },
                    UmbAcqProfile {
                        umb_acq_profile: 2,
                        fft_size: 0b1111,                 // "any"
                        cyclic_prefix_length: 0b111,      // "any"
                        num_guard_subcarriers: 0b1111111, // "any"
                    },
                ],
            },
        )],
        vec![],
        vec![sys_cdma2000_minimal(0)],
    ));
}

#[test]
fn generic_umb_round_trip() {
    // NUM_UMB_BLOCKS(6) + 2 × {BAND_CLASS(8) + CHANNEL(16) + PROFILE(6)} = 66 bits → 9 octets.
    round_trip(empty_prl(
        vec![acq(
            0x10,
            9,
            ExtAcquisitionBody::GenericUmb {
                blocks: vec![
                    UmbBlock {
                        band_class: 14,
                        channel_number: 5000,
                        umb_acq_table_profile: 1,
                    },
                    UmbBlock {
                        band_class: 15,
                        channel_number: 0xFFFF,      // wildcard channel
                        umb_acq_table_profile: 0x3F, // ignore common table
                    },
                ],
            },
        )],
        vec![],
        vec![sys_cdma2000_minimal(0)],
    ));
}

// ----------------------------------------------------------------------------
// MCC-MNC system records (Table 3.5.5.3.2.2)
// ----------------------------------------------------------------------------

fn sys_mccmnc(length_octets: u8, subtype: MccMncSubtype) -> ExtSystemRecord {
    ExtSystemRecord {
        sys_record_length: length_octets,
        sys_record_type: ExtSystemRecordType::MccMnc,
        pref_neg: PrefNeg::Preferred,
        same_geo_as_prev: false,
        priority: Priority::MoreDesirable,
        acq_index: 0,
        system_id: ExtSystemId::MccMnc(subtype),
        roaming_indicator: Some(RoamingIndicator::Roaming),
        association: None,
    }
}

#[test]
fn mccmnc_subtype_000_round_trip() {
    // Header: SYS_RECORD_LENGTH(5) + SYS_RECORD_TYPE(4) + PREF_NEG(1)
    // + GEO(1) + PRI(1) + ACQ_INDEX(9) = 21 bits.
    // Body: SYS_RECORD_SUBTYPE(3) + MCC(12) + MNC(12) = 27 bits.
    // Tail: ROAM_IND(8) + ASSOC_INC(1) = 9 bits.
    // Total = 57 bits → round up to 8 octets = 64 bits, so 7 RESERVED.
    round_trip(empty_prl(
        vec![acq(
            0x02,
            1,
            ExtAcquisitionBody::CellularCdmaStandard {
                ab: AbSelection::SystemB,
                pri_sec: StandardChannelSelection::Primary,
            },
        )],
        vec![],
        vec![sys_mccmnc(
            8,
            MccMncSubtype::Subtype000 {
                mcc_bcd: 0x310, // MCC 310 (US)
                mnc_bcd: 0x23F, // MNC 23 with F padding for 2-digit
            },
        )],
    ));
}

#[test]
fn mccmnc_subtype_001_round_trip() {
    // Body: 3 + 12 + 12 + 4 + 4 + N*16. With N=2: 67 bits.
    // Plus header(21) + tail(9) = 97 bits → 13 octets (104 bits).
    round_trip(empty_prl(
        vec![acq(
            0x02,
            1,
            ExtAcquisitionBody::CellularCdmaStandard {
                ab: AbSelection::SystemA,
                pri_sec: StandardChannelSelection::PrimaryOrSecondary,
            },
        )],
        vec![],
        vec![sys_mccmnc(
            13,
            MccMncSubtype::Subtype001 {
                mcc_bcd: 0x310,
                mnc_bcd: 0x410, // 3-digit MNC (e.g. T-Mobile 410)
                sids: vec![22, 4097],
            },
        )],
    ));
}

#[test]
fn mccmnc_subtype_010_round_trip() {
    // 3 + 12 + 12 + 4 + 4 + N*(16+16). With N=2: 99 bits body.
    // Header(21) + body(99) + tail(9) = 129 bits → 17 octets (136 bits).
    round_trip(empty_prl(
        vec![acq(
            0x02,
            1,
            ExtAcquisitionBody::CellularCdmaStandard {
                ab: AbSelection::SystemA,
                pri_sec: StandardChannelSelection::Primary,
            },
        )],
        vec![],
        vec![sys_mccmnc(
            17,
            MccMncSubtype::Subtype010 {
                mcc_bcd: 0x310,
                mnc_bcd: 0x23F,
                pairs: vec![
                    SidNidPair { sid: 1024, nid: 0 },
                    SidNidPair {
                        sid: 1025,
                        nid: 0xFFFF,
                    },
                ],
            },
        )],
    ));
}

#[test]
fn mccmnc_subtype_011_round_trip() {
    // 3 + 12 + 12 + 4 + 4 + sum(8 + SUBNET_LENGTH). One subnet of length 16:
    // 35 + 8 + 16 = 59 bits body.
    // Header(21) + body(59) + tail(9) = 89 bits → 12 octets (96 bits).
    round_trip(empty_prl(
        vec![acq(
            0x02,
            1,
            ExtAcquisitionBody::CellularCdmaStandard {
                ab: AbSelection::SystemA,
                pri_sec: StandardChannelSelection::Primary,
            },
        )],
        vec![],
        vec![sys_mccmnc(
            12,
            MccMncSubtype::Subtype011 {
                mcc_bcd: 0x310,
                mnc_bcd: 0x023, // T-Mobile-ish, 23 with F padding implied
                subnets: vec![MccMncSubnet {
                    subnet_length: 16,
                    subnet_id: vec![0xCA, 0xFE],
                }],
            },
        )],
    ));
}

// ----------------------------------------------------------------------------
// HRPD permutations + Common Subnet Table coverage
// ----------------------------------------------------------------------------

#[test]
fn hrpd_with_association_round_trip() {
    // HRPD sys record + ASSOCIATION_INC=1.
    round_trip(empty_prl(
        vec![acq(
            0x0B,
            2,
            ExtAcquisitionBody::GenericHrpd {
                entries: vec![BandClassChannel {
                    band_class: 1,
                    channel_number: 25,
                }],
            },
        )],
        vec![],
        vec![ExtSystemRecord {
            sys_record_length: 8,
            sys_record_type: ExtSystemRecordType::Hrpd,
            pref_neg: PrefNeg::Preferred,
            same_geo_as_prev: true,
            priority: Priority::EquallyDesirable,
            acq_index: 0,
            system_id: ExtSystemId::Hrpd {
                subnet_common_included: false,
                subnet_lsb_length: 8,
                subnet_lsb: vec![0xAB],
                subnet_common_offset: None,
            },
            roaming_indicator: Some(RoamingIndicator::OnHome),
            association: Some(ExtSystemAssociation {
                association_tag: 7,
                pn_association: true,
                data_association: false,
            }),
        }],
    ));
}

#[test]
fn hrpd_with_subnet_common_offset_and_table_round_trip() {
    // HRPD sys record with SUBNET_COMMON_INCLUDED=1 + a real Common
    // Subnet Table entry it references.
    round_trip(empty_prl(
        vec![acq(
            0x0B,
            2,
            ExtAcquisitionBody::GenericHrpd {
                entries: vec![BandClassChannel {
                    band_class: 1,
                    channel_number: 25,
                }],
            },
        )],
        vec![CommonSubnetRecord {
            subnet_common_length: 3,
            subnet_common: vec![0xDE, 0xAD, 0xBE],
        }],
        vec![ExtSystemRecord {
            sys_record_length: 8,
            sys_record_type: ExtSystemRecordType::Hrpd,
            pref_neg: PrefNeg::Preferred,
            same_geo_as_prev: false,
            priority: Priority::MoreDesirable,
            acq_index: 0,
            system_id: ExtSystemId::Hrpd {
                subnet_common_included: true,
                subnet_lsb_length: 0,
                subnet_lsb: vec![],
                subnet_common_offset: Some(0),
            },
            roaming_indicator: Some(RoamingIndicator::Other(64)),
            association: None,
        }],
    ));
}

#[test]
fn common_subnet_table_only_round_trip() {
    // Smallest possible Common Subnet Table — one record with empty
    // subnet_common payload. Exercises the previously-buggy 4+4 length
    // layout of §3.5.5.3.2.1.
    round_trip(empty_prl(
        vec![acq(
            0x02,
            1,
            ExtAcquisitionBody::CellularCdmaStandard {
                ab: AbSelection::SystemB,
                pri_sec: StandardChannelSelection::PrimaryOrSecondary,
            },
        )],
        vec![
            CommonSubnetRecord {
                subnet_common_length: 0,
                subnet_common: vec![],
            },
            CommonSubnetRecord {
                subnet_common_length: 4,
                subnet_common: vec![1, 2, 3, 4],
            },
        ],
        vec![sys_cdma2000_minimal(0)],
    ));
}
