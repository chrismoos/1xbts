//! OTASP Data Messages — per-feature wire codecs (C.S0016-D §3.5 / §4.5).

pub mod commit;
pub mod configuration;
pub mod download;
pub mod mms;
pub mod protocol_capability;
pub mod result_code;
pub mod sspr;
pub mod system_tag;
pub mod validation;

/// OTASP_MSG_TYPE octets per C.S0016-D §3.5 / §4.5 tables.
pub mod msg_type {
    pub const CONFIGURATION: u8 = 0x00;
    pub const DOWNLOAD: u8 = 0x01;
    pub const COMMIT_REQ_AND_RESP: u8 = 0x05;
    pub const PROTOCOL_CAPABILITY: u8 = 0x06;
    pub const SSPR_CONFIGURATION: u8 = 0x07;
    pub const SSPR_DOWNLOAD: u8 = 0x08;
    pub const VALIDATION: u8 = 0x09;
    pub const SYSTEM_TAG_CONFIGURATION: u8 = 0x13;
    pub const SYSTEM_TAG_DOWNLOAD: u8 = 0x14;
    pub const MMS_CONFIGURATION: u8 = 0x16;
    pub const MMS_DOWNLOAD: u8 = 0x17;
}

use crate::Error;

pub(crate) fn require_msg_type(actual: u8, expected: u8) -> Result<(), Error> {
    if actual != expected {
        return Err(format!(
            "OTASP_MSG_TYPE mismatch: got 0x{:02x}, expected 0x{:02x}",
            actual, expected
        )
        .into());
    }
    Ok(())
}
