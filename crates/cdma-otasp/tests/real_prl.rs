//! Real-carrier PRL fixtures decoded through our classic PRL parser.
//!
//! Each `.prl` file is the raw on-wire PRL the BS would reassemble
//! across SSPR Configuration Response segments. Filenames carry the
//! PR_LIST_ID in decimal; the test cross-checks that against the
//! decoded header so a corrupted fixture shows up loudly.
//!
//! Classic (SSPR_P_REV = 1) fixtures should decode cleanly with a
//! matching CRC. Extended (SSPR_P_REV >= 2) fixtures must NOT decode
//! as classic.

use cdma_otasp::param::prl;

fn decode_classic(path: &str, bytes: &[u8]) {
    let filename = path.rsplit('/').next().unwrap();
    let expected_id: u16 = filename
        .rsplit('_')
        .next()
        .and_then(|s| s.strip_suffix(".prl"))
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("fixture name lacks numeric ID: {}", filename));

    let p = prl::decode(bytes).unwrap_or_else(|e| panic!("{}: decode failed: {}", filename, e));
    assert_eq!(
        p.pr_list_id, expected_id,
        "{}: PR_LIST_ID mismatch (header says {}, filename says {})",
        filename, p.pr_list_id, expected_id
    );
    assert_eq!(
        p.pr_list_size as usize,
        bytes.len(),
        "{}: PR_LIST_SIZE ({}) != file length ({})",
        filename,
        p.pr_list_size,
        bytes.len()
    );
    assert!(
        p.crc_ok(),
        "{}: CRC mismatch (file=0x{:04x} computed=0x{:04x})",
        filename,
        p.pr_list_crc,
        p.computed_crc
    );
    assert!(
        !p.acquisition_records.is_empty(),
        "{}: no acquisition records",
        filename
    );
    assert!(
        !p.system_records.is_empty(),
        "{}: no system records",
        filename
    );
}

macro_rules! classic_fixture {
    ($name:ident, $path:literal) => {
        #[test]
        fn $name() {
            decode_classic($path, include_bytes!($path));
        }
    };
}

classic_fixture!(verizon_50408, "fixtures/verizon_50408.prl");
classic_fixture!(sprint_10021, "fixtures/sprint_10021.prl");
classic_fixture!(boost_10037, "fixtures/boost_10037.prl");
classic_fixture!(metropcs_00019, "fixtures/metropcs_00019.prl");
classic_fixture!(metropcs_00021, "fixtures/metropcs_00021.prl");
classic_fixture!(pocket_00008, "fixtures/pocket_00008.prl");
classic_fixture!(qwest_13022, "fixtures/qwest_13022.prl");
classic_fixture!(qwest_30012, "fixtures/qwest_30012.prl");
classic_fixture!(western_00099, "fixtures/western_00099.prl");
classic_fixture!(alltel_00206, "fixtures/alltel_00206.prl");
classic_fixture!(alltel_00505, "fixtures/alltel_00505.prl");
classic_fixture!(appalachian_00059, "fixtures/appalachian_00059.prl");
classic_fixture!(cricket_01001, "fixtures/cricket_01001.prl");
classic_fixture!(cricket_01004, "fixtures/cricket_01004.prl");
classic_fixture!(ntelos_00553, "fixtures/ntelos_00553.prl");
classic_fixture!(ntelos_02801, "fixtures/ntelos_02801.prl");

/// Sanity check on the Verizon 50408 fixture (the canary). Spot-checks a
/// few fields beyond shape so a regression in the SystemRecord decoder
/// (e.g. NID_INCL handling, ACQ_INDEX bit alignment) gets caught even
/// when the high-level counts and CRC still match.
#[test]
fn verizon_50408_spot_check() {
    let p = prl::decode(include_bytes!("fixtures/verizon_50408.prl")).unwrap();
    assert_eq!(p.pr_list_id, 50408);
    assert_eq!(p.acquisition_records.len(), 30);
    assert_eq!(p.system_records.len(), 794);

    // First two acquisition records on Verizon's PRL are the Cellular
    // CDMA Preferred records covering both 800 MHz A and B carriers.
    use prl::{AbSelection, AcquisitionBody};
    assert!(matches!(
        p.acquisition_records[0].body,
        AcquisitionBody::CellularCdmaPreferred {
            ab: AbSelection::SystemB
        }
    ));
    assert!(matches!(
        p.acquisition_records[1].body,
        AcquisitionBody::CellularCdmaPreferred {
            ab: AbSelection::SystemA
        }
    ));

    // First system record is preferred (subscribers always have at
    // least one preferred entry).
    assert_eq!(p.system_records[0].pref_neg, prl::PrefNeg::Preferred);
    assert!(p.system_records[0].roaming_indicator.is_some());
}

fn decode_extended(path: &str, bytes: &[u8]) {
    use cdma_otasp::param::prl_ext;
    let filename = path.rsplit('/').next().unwrap();
    let expected_id: u16 = filename
        .rsplit('_')
        .next()
        .and_then(|s| s.strip_suffix(".prl"))
        .and_then(|s| s.parse().ok())
        .unwrap_or_else(|| panic!("fixture name lacks numeric ID: {}", filename));
    let p = prl_ext::decode(bytes)
        .unwrap_or_else(|e| panic!("{}: Extended decode failed: {}", filename, e));
    assert_eq!(
        p.pr_list_id, expected_id,
        "{}: PR_LIST_ID mismatch",
        filename
    );
    assert_eq!(
        p.pr_list_size as usize,
        bytes.len(),
        "{}: PR_LIST_SIZE mismatch",
        filename
    );
    assert_eq!(
        p.cur_sspr_p_rev, 0x03,
        "{}: only SSPR_P_REV=3 is in scope",
        filename
    );
    assert!(
        p.crc_ok(),
        "{}: Extended CRC mismatch (file=0x{:04x} computed=0x{:04x})",
        filename,
        p.pr_list_crc,
        p.computed_crc
    );
    assert!(
        !p.acquisition_records.is_empty(),
        "{}: no acq records",
        filename
    );
    assert!(!p.system_records.is_empty(), "{}: no sys records", filename);
}

macro_rules! extended_fixture {
    ($name:ident, $path:literal) => {
        #[test]
        fn $name() {
            decode_extended($path, include_bytes!($path));
        }
    };
}

extended_fixture!(verizon_51611_extended, "fixtures/verizon_51611.prl");
extended_fixture!(sprint_60608_extended, "fixtures/sprint_60608.prl");
extended_fixture!(usc_15056_extended, "fixtures/usc_15056.prl");
extended_fixture!(usc_15118_extended, "fixtures/usc_15118.prl");
extended_fixture!(usc_15508_extended, "fixtures/usc_15508.prl");
extended_fixture!(bluegrass_07067_extended, "fixtures/bluegrass_07067.prl");

/// Verify that the same Verizon network appears under both classic
/// (50408) and Extended (51611) PRLs — the cellular SID/roam-indicator
/// values should be identical even though the wire format differs.
#[test]
fn verizon_classic_vs_extended_share_network() {
    use cdma_otasp::param::prl_ext;
    let classic = prl::decode(include_bytes!("fixtures/verizon_50408.prl")).unwrap();
    let extended = prl_ext::decode(include_bytes!("fixtures/verizon_51611.prl")).unwrap();
    // Same operator → same PREF_ONLY policy and same default roam ind.
    assert!(classic.pref_only && extended.pref_only);
    assert_eq!(classic.def_roam_ind, extended.def_roam_ind);
    // Top Verizon SIDs from the classic system table must all appear
    // somewhere in the extended one too.
    // 50408 → 51611 spans ~1200 PRL revisions; Verizon retired some
    // SIDs in between, so a strict subset check is too brittle. Pick
    // a set of historically stable Verizon-owned SIDs from the
    // classic top-of-list and require they all survive into Extended.
    let stable_classic_sids: [u16; 5] = [5269, 5513, 5510, 5685, 5682];
    let extended_sids: std::collections::HashSet<u16> = extended
        .system_records
        .iter()
        .filter_map(|s| match &s.system_id {
            prl_ext::ExtSystemId::Cdma2000 { sid, .. } => Some(*sid),
            _ => None,
        })
        .collect();
    for sid in stable_classic_sids {
        assert!(
            extended_sids.contains(&sid),
            "stable Verizon SID {} missing from Extended PRL",
            sid
        );
    }
}

/// Sanity check: Extended decoder must reject buffers that aren't
/// SSPR_P_REV=3.
#[test]
fn extended_decoder_rejects_classic_prl() {
    use cdma_otasp::param::prl_ext;
    let bytes = include_bytes!("fixtures/verizon_50408.prl");
    assert!(prl_ext::decode(bytes).is_err());
}

/// Extended PRL fixtures (CUR_SSPR_P_REV = 3 in the header). The
/// classic decoder must NOT silently accept these: it either errors
/// or produces values that don't reconcile (CRC / size mismatch).
#[test]
fn extended_prl_not_decoded_as_classic() {
    for &(name, bytes) in &[
        (
            "bluegrass_07067",
            include_bytes!("fixtures/bluegrass_07067.prl").as_slice(),
        ),
        (
            "sprint_60608",
            include_bytes!("fixtures/sprint_60608.prl").as_slice(),
        ),
        (
            "usc_15056",
            include_bytes!("fixtures/usc_15056.prl").as_slice(),
        ),
        (
            "usc_15118",
            include_bytes!("fixtures/usc_15118.prl").as_slice(),
        ),
        (
            "usc_15508",
            include_bytes!("fixtures/usc_15508.prl").as_slice(),
        ),
        (
            "verizon_51611",
            include_bytes!("fixtures/verizon_51611.prl").as_slice(),
        ),
    ] {
        // Byte 4 is CUR_SSPR_P_REV in Extended PRLs. Should be 0x03.
        assert_eq!(
            bytes[4], 0x03,
            "{}: expected Extended PRL marker (byte 4 = SSPR_P_REV = 0x03)",
            name
        );
        match prl::decode(bytes) {
            Ok(p) => assert!(
                !p.crc_ok() || p.pr_list_size as usize != bytes.len(),
                "{}: classic decoder accepted an Extended PRL with matching CRC and size",
                name
            ),
            Err(_) => {} // expected
        }
    }
}
