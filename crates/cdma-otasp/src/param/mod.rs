//! Parameter blocks carried inside OTASP messages.
//!
//! Each block has a `BLOCK_ID` (used in Configuration / Download messages)
//! and a fixed wire layout. Encoders/decoders here produce / consume the
//! bytes that go into the `PARAM_DATA` field — they do not encode the
//! `BLOCK_ID` + `BLOCK_LEN` envelope; that lives in the message layer.

pub mod change_spc;
pub mod home_system_tag;
pub mod mdn;
pub mod nam_cdma;
pub mod nam_cdma_analog;
pub mod prl;
pub mod prl_dimensions;
pub mod prl_ext;
pub mod prl_segment;
pub mod verify_spc;

/// NAM parameter block IDs per C.S0016-D Table 3.5.2-1.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamBlockId {
    CdmaAnalog = 0x00,
    Mdn = 0x01,
    Cdma = 0x02,
    ImsiT = 0x03,
    EhrpdImsi = 0x04,
}

/// Validation parameter block IDs per C.S0016-D Table 4.5.4-1.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ValidationBlockId {
    VerifySpc = 0x00,
    ChangeSpc = 0x01,
    ValidateSpasm = 0x02,
}

/// System-tag parameter block IDs per C.S0016-D Table 3.5.10-1.
#[repr(u8)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemTagBlockId {
    HomeSystemTag = 0x00,
    GroupTagList = 0x01,
    SpecificTagList = 0x02,
    CallPromptList = 0x03,
    GroupTagListDimensions = 0x04,
    SpecificTagListDimensions = 0x05,
    CallPromptListDimensions = 0x06,
}
