//! Byte-identical round-trip for the classic + Extended PRL encoders
//! against real-carrier fixtures. A failure here means a real PRL no
//! longer survives `decode → encode` — either the decoder lost
//! field-level detail it should have kept, or the encoder doesn't
//! reproduce a bit the original had.

use cdma_otasp::param::{prl, prl_ext};

fn round_trip_classic(name: &str, bytes: &[u8]) {
    let decoded =
        prl::decode(bytes).unwrap_or_else(|e| panic!("{}: classic decode failed: {}", name, e));
    let reencoded = decoded
        .encode()
        .unwrap_or_else(|e| panic!("{}: classic encode failed: {}", name, e));
    assert_eq!(
        reencoded.len(),
        bytes.len(),
        "{}: re-encoded length differs (orig={}, new={})",
        name,
        bytes.len(),
        reencoded.len()
    );
    if reencoded != bytes {
        // Find the first mismatching byte for a useful error message.
        for (i, (a, b)) in bytes.iter().zip(reencoded.iter()).enumerate() {
            if a != b {
                panic!(
                    "{}: byte {} differs: original=0x{:02x}, reencoded=0x{:02x}",
                    name, i, a, b
                );
            }
        }
    }
}

fn round_trip_extended(name: &str, bytes: &[u8]) {
    let decoded = prl_ext::decode(bytes)
        .unwrap_or_else(|e| panic!("{}: extended decode failed: {}", name, e));
    let reencoded = decoded
        .encode()
        .unwrap_or_else(|e| panic!("{}: extended encode failed: {}", name, e));
    assert_eq!(
        reencoded.len(),
        bytes.len(),
        "{}: re-encoded length differs (orig={}, new={})",
        name,
        bytes.len(),
        reencoded.len()
    );
    if reencoded != bytes {
        for (i, (a, b)) in bytes.iter().zip(reencoded.iter()).enumerate() {
            if a != b {
                panic!(
                    "{}: byte {} differs: original=0x{:02x}, reencoded=0x{:02x}",
                    name, i, a, b
                );
            }
        }
    }
}

macro_rules! classic_rt {
    ($name:ident, $path:literal) => {
        #[test]
        fn $name() {
            round_trip_classic(stringify!($name), include_bytes!($path));
        }
    };
}

macro_rules! extended_rt {
    ($name:ident, $path:literal) => {
        #[test]
        fn $name() {
            round_trip_extended(stringify!($name), include_bytes!($path));
        }
    };
}

// All 16 classic real-carrier fixtures.
classic_rt!(rt_verizon_50408, "fixtures/verizon_50408.prl");
classic_rt!(rt_sprint_10021, "fixtures/sprint_10021.prl");
classic_rt!(rt_boost_10037, "fixtures/boost_10037.prl");
classic_rt!(rt_metropcs_00019, "fixtures/metropcs_00019.prl");
classic_rt!(rt_metropcs_00021, "fixtures/metropcs_00021.prl");
classic_rt!(rt_pocket_00008, "fixtures/pocket_00008.prl");
classic_rt!(rt_qwest_13022, "fixtures/qwest_13022.prl");
classic_rt!(rt_qwest_30012, "fixtures/qwest_30012.prl");
classic_rt!(rt_western_00099, "fixtures/western_00099.prl");
classic_rt!(rt_alltel_00206, "fixtures/alltel_00206.prl");
classic_rt!(rt_alltel_00505, "fixtures/alltel_00505.prl");
classic_rt!(rt_appalachian_00059, "fixtures/appalachian_00059.prl");
classic_rt!(rt_cricket_01001, "fixtures/cricket_01001.prl");
classic_rt!(rt_cricket_01004, "fixtures/cricket_01004.prl");
classic_rt!(rt_ntelos_00553, "fixtures/ntelos_00553.prl");
classic_rt!(rt_ntelos_02801, "fixtures/ntelos_02801.prl");

// All 5 Extended real-carrier fixtures.
extended_rt!(rt_verizon_51611, "fixtures/verizon_51611.prl");
extended_rt!(rt_sprint_60608, "fixtures/sprint_60608.prl");
extended_rt!(rt_usc_15056, "fixtures/usc_15056.prl");
extended_rt!(rt_usc_15508, "fixtures/usc_15508.prl");
extended_rt!(rt_bluegrass_07067, "fixtures/bluegrass_07067.prl");
