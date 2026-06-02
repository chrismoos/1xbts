//! OTASP session events emitted on the event bus.
//!
//! Mirrors `events.v1.OtaspEvent` in `proto/events/v1/msc.proto`. Kept as a
//! plain Rust enum here so the session driver can emit events without the
//! `tonic` plumbing; the bus producer converts to the proto representation.

/// Identifies the OTASP mobile by whichever hardware identity it presented.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HardwareIdentity {
    pub esn: Option<u32>,
    pub meid: Option<String>,
}

/// Per-session lifecycle events.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OtaspEvent {
    /// Origination accepted and session started.
    SessionStart {
        device: HardwareIdentity,
        feature_code: String,
        service_option: u16,
    },
    /// Protocol Capability Response decoded.
    ProtocolCapabilityReceived {
        mob_firm_rev: u16,
        mob_model: u8,
        band_mode_cap: BandModeCapView,
        /// OTASP protocol revision the MS supports, from the OTASP feature
        /// entry in the features list (Table 3.5.1.7-1, `FEATURE_ID = 0x04`).
        otasp_p_rev: Option<u8>,
        /// All `(FEATURE_ID, FEATURE_P_REV)` pairs the MS advertised.
        features: Vec<(u8, u8)>,
    },
    /// Decoded Station Class Mark per C.S0005-E §2.3.3 Table 2.3.3-1. Sourced
    /// from the SCM field in a NAM read-back (CDMA/Analog NAM only); the MS
    /// does not send SCM in the Protocol Capability Response.
    StationClassMark(StationClassMark),
    /// MS accepted Verify SPC — programming may proceed.
    SpcVerified,
    /// MS rejected Verify SPC — session is being released.
    SpcMismatch,
    /// No inbound message received for the configured threshold; the call is
    /// being released.
    Timeout { phase: String },
    /// HLR lookup returned no record for the device.
    HlrMiss { device: HardwareIdentity },
    /// MS reported `MAX_SID_NID = 0` so the requested NAM block cannot be
    /// stored.
    NoNamCapacity { block_id: u8, feature: BlockFeature },
    /// Per-block download skipped because the MS did not advertise the
    /// feature in its Protocol Capability Response.
    BlockSkipped {
        block_id: u8,
        reason: String,
        feature: BlockFeature,
    },
    /// Decoded read-back from a Configuration Response. Emitted even
    /// when the block is read-only (not being written).
    NamReadback {
        block_id: u8,
        label: String,
        fields: Vec<(String, String)>,
        feature: BlockFeature,
    },
    /// SSPR (Preferred Roaming List) read-back. One emit per session,
    /// after the segmented retrieval finishes. `Outcome` captures the
    /// happy-path decoded PRL and the spec-defined skip reasons (MS
    /// has no PRL, MS only stores Extended PRL, MS rejected the
    /// SSPR Configuration Request, decode/CRC failure).
    PrlReadback(PrlReadback),
    /// Per-block download accepted by the MS. `fields` carries the
    /// decoded values the BS programmed, in the same name/value form
    /// as a `NamReadback` so the UI can render them identically.
    /// Empty for blocks without a structured field view (e.g. the
    /// PRL push, where the bytes speak for themselves).
    BlockDownloaded {
        block_id: u8,
        result_code: u8,
        feature: BlockFeature,
        fields: Vec<(String, String)>,
    },
    /// Per-block download rejected by the MS.
    BlockRejected {
        block_id: u8,
        result_code: u8,
        feature: BlockFeature,
    },
    /// MS responded to the Commit Request.
    CommitResult { result_code: u8 },
    /// Session ended; reports how many blocks reached commit.
    SessionEnded {
        completed_blocks: u32,
        outcome: SessionOutcomeKind,
    },
}

/// Disambiguator for events that carry a BLOCK_ID. Multiple feature
/// spaces overload the low BLOCK_ID values (NAM 0x00 vs Home System
/// Tag 0x00 vs MMS URI 0x00 vs PRL 0x00), so the consumer needs this
/// to render the right label.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlockFeature {
    Nam,
    SystemTag,
    MmsUri,
    Prl,
}

/// Terminal categorization of a session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionOutcomeKind {
    /// Commit Accepted by the MS.
    Committed,
    /// No blocks were enabled or all were skipped — no Commit sent.
    NothingToCommit,
    /// MS reported SPC mismatch.
    SpcRejected,
    /// HLR has no record for this device.
    HlrUnknown,
    /// MS rejected a Download or Commit Request.
    Rejected,
    /// MS reported no capacity for the NAM.
    NoCapacity,
    /// Decode error or unexpected MS response.
    ProtocolError,
    /// Inbound silence past the timeout threshold.
    TimedOut,
}

/// Decoded Station Class Mark byte. Fields are spec-named per
/// C.S0005-E §2.3.3 Table 2.3.3-1.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StationClassMark {
    pub raw: u8,
    pub extended: ScmExtended,
    pub dual_mode: ScmDualMode,
    pub slotted_class: ScmSlottedClass,
    pub meid_support: ScmMeidSupport,
    pub bandwidth_25mhz: bool,
    pub transmission: ScmTransmission,
    /// Power Class for Band Class 0 Analog Operation (bits 1–0). Spec
    /// mandates 00 for CDMA-only stations; legacy radios may set non-zero.
    pub analog_power_class: u8,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScmExtended {
    /// Bit 7 = 0. Applies to all bands other than the PCS family.
    StandardBands,
    /// Bit 7 = 1. Applies to Band Classes 1, 4, 14.
    PcsFamily,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScmDualMode {
    /// Bit 6 = 0. Spec mandates this value (analog support deprecated).
    CdmaOnly,
    /// Bit 6 = 1. Legacy dual analog/CDMA stations.
    Dual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScmSlottedClass {
    NonSlotted,
    /// Battery-save paging. MS monitors only its assigned paging slot.
    Slotted,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScmMeidSupport {
    /// MS does not carry a MEID (ESN-only device).
    NotConfigured,
    /// MS has a MEID configured.
    Configured,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScmTransmission {
    Continuous,
    /// DTX — MS may stop transmitting between voice frames.
    Discontinuous,
}

/// Decoded BAND_MODE_CAP byte per C.S0016-D Table 3.5.1.7-2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BandModeCapView {
    pub raw: u8,
    pub band_class_0_analog: bool,
    pub band_class_0_cdma: bool,
    pub band_class_1_cdma: bool,
    pub band_class_3_cdma: bool,
    pub band_class_6_cdma: bool,
    /// RESERVED bits 2–0. Spec requires 0; kept so non-compliant MS
    /// behavior is visible.
    pub reserved: u8,
}

impl BandModeCapView {
    pub fn from_byte(b: u8) -> Self {
        Self {
            raw: b,
            band_class_0_analog: (b & 0x80) != 0,
            band_class_0_cdma: (b & 0x40) != 0,
            band_class_1_cdma: (b & 0x20) != 0,
            band_class_3_cdma: (b & 0x10) != 0,
            band_class_6_cdma: (b & 0x08) != 0,
            reserved: b & 0b0000_0111,
        }
    }
}

/// Decoded SSPR PRL read-back. Re-exports the [`cdma_otasp::param::prl`]
/// types so consumers don't need a direct dependency on `cdma-otasp`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrlReadback {
    /// `PR_LIST_ID` from the PRL Dimensions block (also embedded in the
    /// PRL itself when decode succeeds).
    pub pr_list_id: Option<u16>,
    /// Dimensions reported by the MS.
    pub max_pr_list_size: u16,
    pub cur_pr_list_size: u16,
    /// Number of segment fetches it took to assemble the PRL.
    pub segment_count: u32,
    pub outcome: PrlOutcome,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PrlOutcome {
    /// Classic PRL (`SSPR_P_REV = 1`) fully retrieved and decoded.
    Decoded(cdma_otasp::param::prl::ClassicPrl),
    /// Extended PRL (`SSPR_P_REV >= 3`) fully retrieved and decoded.
    /// The BS got here by issuing Extended PRL Dimensions
    /// (`BLOCK_ID = 0x02`) after the MS rejected the classic
    /// dimensions request with `0x23`, per C.S0016-D §3.5.1.8.
    /// `raw_bytes` is the assembled on-wire PRL so operators can
    /// download or import it into the structured editor.
    DecodedExtended {
        prl: cdma_otasp::param::prl_ext::ExtendedPrl,
        raw_bytes: Vec<u8>,
    },
    /// MS reports `CUR_PR_LIST_SIZE = 0` — no PRL programmed.
    Absent,
    /// MS doesn't advertise the SSPR feature so we never tried.
    FeatureNotAdvertised,
    /// MS rejected the SSPR Configuration Request with a result code
    /// other than `0x23`.
    Rejected { block_id: u8, result_code: u8 },
    /// SSPR retrieval finished but the assembled bytes don't decode or
    /// the CRC didn't match. `raw_bytes` is preserved for offline
    /// inspection.
    DecodeFailed { reason: String, raw_bytes: Vec<u8> },
}

impl StationClassMark {
    pub fn decode(byte: u8) -> Self {
        Self {
            raw: byte,
            extended: if (byte >> 7) & 1 == 1 {
                ScmExtended::PcsFamily
            } else {
                ScmExtended::StandardBands
            },
            dual_mode: if (byte >> 6) & 1 == 1 {
                ScmDualMode::Dual
            } else {
                ScmDualMode::CdmaOnly
            },
            slotted_class: if (byte >> 5) & 1 == 1 {
                ScmSlottedClass::Slotted
            } else {
                ScmSlottedClass::NonSlotted
            },
            meid_support: if (byte >> 4) & 1 == 1 {
                ScmMeidSupport::Configured
            } else {
                ScmMeidSupport::NotConfigured
            },
            bandwidth_25mhz: (byte >> 3) & 1 == 1,
            transmission: if (byte >> 2) & 1 == 1 {
                ScmTransmission::Discontinuous
            } else {
                ScmTransmission::Continuous
            },
            analog_power_class: byte & 0b11,
        }
    }
}
