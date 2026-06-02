//! Integration test for the PRL editor's save path.
//!
//! The full flow when an operator hits Save in the structured editor is:
//!
//! 1. UI sends a `PrlDecoded` proto in the `built` source of UpdatePrl.
//! 2. gRPC service calls `prl_proto::encode_proto_to_bytes(decoded)`.
//! 3. That converts proto → cdma_otasp Rust types → `encode()` → bytes.
//! 4. Server persists those bytes and decodes them on the next GetPrl.
//!
//! For the round-trip to be loss-free, this chain must satisfy:
//!
//!     bytes == encode(proto_from(decode(bytes)))
//!
//! That's what this test asserts against every real-carrier PRL we ship
//! as a fixture. Any proto-conversion bug that drops a field, defaults
//! it differently, or reorders a list will surface here loudly.

use cdma_hlr::prl_proto;
use cdma_hlr::proto::PrlSummary;
use cdma_otasp::param::{prl, prl_ext};

fn round_trip_via_proto(name: &str, bytes: &[u8]) {
    let summary = PrlSummary {
        prl_id: "00000000-0000-0000-0000-000000000000".to_string(),
        name: name.to_string(),
        pr_list_id: 0,
        sspr_p_rev: 0,
        is_default: false,
        raw_bytes_size: bytes.len() as u32,
        notes: String::new(),
        created_at: None,
        updated_at: None,
    };

    // Step 1: bytes → proto::Prl (with decoded tree)
    let full = prl_proto::proto_from_raw_bytes(summary, bytes.to_vec())
        .unwrap_or_else(|e| panic!("{}: proto_from_raw_bytes failed: {:?}", name, e));
    let decoded = full.decoded.expect("decoded tree");

    // Step 2: PrlDecoded → bytes (the editor's save path)
    let reencoded = prl_proto::encode_proto_to_bytes(&decoded)
        .unwrap_or_else(|e| panic!("{}: encode_proto_to_bytes failed: {:?}", name, e));

    assert_eq!(
        reencoded.len(),
        bytes.len(),
        "{}: length differs (orig={}, re-encoded={})",
        name,
        bytes.len(),
        reencoded.len()
    );
    if reencoded != bytes {
        for (i, (a, b)) in bytes.iter().zip(reencoded.iter()).enumerate() {
            if a != b {
                panic!(
                    "{}: byte {} differs (orig=0x{:02x}, re-encoded=0x{:02x})",
                    name, i, a, b
                );
            }
        }
    }

    // Step 3 (sanity): the re-encoded bytes must decode again identically.
    use cdma_hlr::proto::prl_decoded::Body;
    match &decoded.body {
        Some(Body::Classic(_)) => {
            let again = prl::decode(&reencoded).unwrap();
            assert!(again.crc_ok(), "{}: CRC failed on re-encoded bytes", name);
        }
        Some(Body::Extended(_)) => {
            let again = prl_ext::decode(&reencoded).unwrap();
            assert!(again.crc_ok(), "{}: CRC failed on re-encoded bytes", name);
        }
        None => panic!("{}: PrlDecoded.body missing", name),
    }
}

macro_rules! fixture {
    ($name:ident, $path:literal) => {
        #[test]
        fn $name() {
            round_trip_via_proto(
                stringify!($name),
                include_bytes!(concat!("../../cdma-otasp/tests/fixtures/", $path)),
            );
        }
    };
}

// Classic
fixture!(rt_verizon_50408, "verizon_50408.prl");
fixture!(rt_sprint_10021, "sprint_10021.prl");
fixture!(rt_boost_10037, "boost_10037.prl");
fixture!(rt_metropcs_00019, "metropcs_00019.prl");
fixture!(rt_metropcs_00021, "metropcs_00021.prl");
fixture!(rt_pocket_00008, "pocket_00008.prl");
fixture!(rt_qwest_13022, "qwest_13022.prl");
fixture!(rt_qwest_30012, "qwest_30012.prl");
fixture!(rt_western_00099, "western_00099.prl");
fixture!(rt_alltel_00206, "alltel_00206.prl");
fixture!(rt_alltel_00505, "alltel_00505.prl");
fixture!(rt_appalachian_00059, "appalachian_00059.prl");
fixture!(rt_cricket_01001, "cricket_01001.prl");
fixture!(rt_cricket_01004, "cricket_01004.prl");
fixture!(rt_ntelos_00553, "ntelos_00553.prl");
fixture!(rt_ntelos_02801, "ntelos_02801.prl");

// Extended
fixture!(rt_verizon_51611, "verizon_51611.prl");
fixture!(rt_sprint_60608, "sprint_60608.prl");
fixture!(rt_usc_15056, "usc_15056.prl");
fixture!(rt_usc_15508, "usc_15508.prl");
fixture!(rt_bluegrass_07067, "bluegrass_07067.prl");
