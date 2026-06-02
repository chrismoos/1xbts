//! OTASP `RESULT_CODE` (C.S0016-D Table 3.5.1.2-1).
//!
//! Enumerated values cover everything the spec defines through `0x32`. Other
//! values pass through via [`ResultCode::Other`] so consumers don't lose
//! information when the spec evolves or vendor-specific codes appear in the
//! `0x80..=0xFE` range.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResultCode {
    Accepted,
    RejectedUnknown,
    RejectedDataSizeMismatch,
    RejectedProtocolVersionMismatch,
    RejectedInvalidParameter,
    RejectedSidNidLengthMismatch,
    RejectedMessageNotExpectedInMode,
    RejectedBlockIdNotSupported,
    RejectedPrlLengthMismatch,
    RejectedCrcError,
    RejectedMobileStationLocked,
    RejectedInvalidSpc,
    RejectedSpcChangeDeniedByUser,
    RejectedInvalidSpasm,
    RejectedBlockIdNotExpectedInMode,
    RejectedUserZoneAlreadyInPuzl,
    RejectedUserZoneNotInPuzl,
    RejectedNoEntriesInPuzl,
    RejectedOperationModeMismatch,
    RejectedSimpleIpMaxNumNaiMismatch,
    RejectedSimpleIpMaxNaiLengthMismatch,
    RejectedMobileIpMaxNumNaiMismatch,
    RejectedMobileIpMaxNaiLengthMismatch,
    RejectedSimpleIpPapMaxSsLengthMismatch,
    RejectedSimpleIpChapMaxSsLengthMismatch,
    RejectedMobileIpMaxMnAaaSsLengthMismatch,
    RejectedMobileIpMaxMnHaSsLengthMismatch,
    RejectedMobileIpMnAaaAuthAlgorithmMismatch,
    RejectedMobileIpMnHaAuthAlgorithmMismatch,
    RejectedSimpleIpActNaiEntryIndexMismatch,
    RejectedMobileIpActNaiEntryIndexMismatch,
    RejectedSimpleIpPapNaiEntryIndexMismatch,
    RejectedSimpleIpChapNaiEntryIndexMismatch,
    RejectedMobileIpNaiEntryIndexMismatch,
    RejectedUnexpectedPrlBlockIdChange,
    RejectedPrlFormatMismatch,
    RejectedHrpdAccessAuthMaxNaiLengthMismatch,
    RejectedHrpdAccessAuthChapMaxSsLengthMismatch,
    RejectedMmdMaxNumImpuMismatch,
    RejectedMmdMaxImpuLengthMismatch,
    RejectedMmdMaxNumPcscfMismatch,
    RejectedMmdMaxPcscfLengthMismatch,
    RejectedUnexpectedSystemTagBlockIdChange,
    RejectedSystemTagFormatMismatch,
    RejectedNumMmsUriMismatch,
    RejectedMmsUriLengthMismatch,
    RejectedInvalidMmsUri,
    RejectedMmssModeSettingsFormatMismatch,
    RejectedMlplFormatMismatch,
    RejectedMsplFormatMismatch,
    RejectedMmssWlanDownloadParamFormatMismatch,
    /// Any value not enumerated above — reserved-for-standardization,
    /// manufacturer-specific (`0x80..=0xFE`), or unrecognized.
    Other(u8),
}

impl ResultCode {
    pub fn from_u8(v: u8) -> Self {
        match v {
            0x00 => Self::Accepted,
            0x01 => Self::RejectedUnknown,
            0x02 => Self::RejectedDataSizeMismatch,
            0x03 => Self::RejectedProtocolVersionMismatch,
            0x04 => Self::RejectedInvalidParameter,
            0x05 => Self::RejectedSidNidLengthMismatch,
            0x06 => Self::RejectedMessageNotExpectedInMode,
            0x07 => Self::RejectedBlockIdNotSupported,
            0x08 => Self::RejectedPrlLengthMismatch,
            0x09 => Self::RejectedCrcError,
            0x0A => Self::RejectedMobileStationLocked,
            0x0B => Self::RejectedInvalidSpc,
            0x0C => Self::RejectedSpcChangeDeniedByUser,
            0x0D => Self::RejectedInvalidSpasm,
            0x0E => Self::RejectedBlockIdNotExpectedInMode,
            0x0F => Self::RejectedUserZoneAlreadyInPuzl,
            0x10 => Self::RejectedUserZoneNotInPuzl,
            0x11 => Self::RejectedNoEntriesInPuzl,
            0x12 => Self::RejectedOperationModeMismatch,
            0x13 => Self::RejectedSimpleIpMaxNumNaiMismatch,
            0x14 => Self::RejectedSimpleIpMaxNaiLengthMismatch,
            0x15 => Self::RejectedMobileIpMaxNumNaiMismatch,
            0x16 => Self::RejectedMobileIpMaxNaiLengthMismatch,
            0x17 => Self::RejectedSimpleIpPapMaxSsLengthMismatch,
            0x18 => Self::RejectedSimpleIpChapMaxSsLengthMismatch,
            0x19 => Self::RejectedMobileIpMaxMnAaaSsLengthMismatch,
            0x1A => Self::RejectedMobileIpMaxMnHaSsLengthMismatch,
            0x1B => Self::RejectedMobileIpMnAaaAuthAlgorithmMismatch,
            0x1C => Self::RejectedMobileIpMnHaAuthAlgorithmMismatch,
            0x1D => Self::RejectedSimpleIpActNaiEntryIndexMismatch,
            0x1E => Self::RejectedMobileIpActNaiEntryIndexMismatch,
            0x1F => Self::RejectedSimpleIpPapNaiEntryIndexMismatch,
            0x20 => Self::RejectedSimpleIpChapNaiEntryIndexMismatch,
            0x21 => Self::RejectedMobileIpNaiEntryIndexMismatch,
            0x22 => Self::RejectedUnexpectedPrlBlockIdChange,
            0x23 => Self::RejectedPrlFormatMismatch,
            0x24 => Self::RejectedHrpdAccessAuthMaxNaiLengthMismatch,
            0x25 => Self::RejectedHrpdAccessAuthChapMaxSsLengthMismatch,
            0x26 => Self::RejectedMmdMaxNumImpuMismatch,
            0x27 => Self::RejectedMmdMaxImpuLengthMismatch,
            0x28 => Self::RejectedMmdMaxNumPcscfMismatch,
            0x29 => Self::RejectedMmdMaxPcscfLengthMismatch,
            0x2A => Self::RejectedUnexpectedSystemTagBlockIdChange,
            0x2B => Self::RejectedSystemTagFormatMismatch,
            0x2C => Self::RejectedNumMmsUriMismatch,
            0x2D => Self::RejectedMmsUriLengthMismatch,
            0x2E => Self::RejectedInvalidMmsUri,
            0x2F => Self::RejectedMmssModeSettingsFormatMismatch,
            0x30 => Self::RejectedMlplFormatMismatch,
            0x31 => Self::RejectedMsplFormatMismatch,
            0x32 => Self::RejectedMmssWlanDownloadParamFormatMismatch,
            other => Self::Other(other),
        }
    }

    pub fn to_u8(self) -> u8 {
        match self {
            Self::Accepted => 0x00,
            Self::RejectedUnknown => 0x01,
            Self::RejectedDataSizeMismatch => 0x02,
            Self::RejectedProtocolVersionMismatch => 0x03,
            Self::RejectedInvalidParameter => 0x04,
            Self::RejectedSidNidLengthMismatch => 0x05,
            Self::RejectedMessageNotExpectedInMode => 0x06,
            Self::RejectedBlockIdNotSupported => 0x07,
            Self::RejectedPrlLengthMismatch => 0x08,
            Self::RejectedCrcError => 0x09,
            Self::RejectedMobileStationLocked => 0x0A,
            Self::RejectedInvalidSpc => 0x0B,
            Self::RejectedSpcChangeDeniedByUser => 0x0C,
            Self::RejectedInvalidSpasm => 0x0D,
            Self::RejectedBlockIdNotExpectedInMode => 0x0E,
            Self::RejectedUserZoneAlreadyInPuzl => 0x0F,
            Self::RejectedUserZoneNotInPuzl => 0x10,
            Self::RejectedNoEntriesInPuzl => 0x11,
            Self::RejectedOperationModeMismatch => 0x12,
            Self::RejectedSimpleIpMaxNumNaiMismatch => 0x13,
            Self::RejectedSimpleIpMaxNaiLengthMismatch => 0x14,
            Self::RejectedMobileIpMaxNumNaiMismatch => 0x15,
            Self::RejectedMobileIpMaxNaiLengthMismatch => 0x16,
            Self::RejectedSimpleIpPapMaxSsLengthMismatch => 0x17,
            Self::RejectedSimpleIpChapMaxSsLengthMismatch => 0x18,
            Self::RejectedMobileIpMaxMnAaaSsLengthMismatch => 0x19,
            Self::RejectedMobileIpMaxMnHaSsLengthMismatch => 0x1A,
            Self::RejectedMobileIpMnAaaAuthAlgorithmMismatch => 0x1B,
            Self::RejectedMobileIpMnHaAuthAlgorithmMismatch => 0x1C,
            Self::RejectedSimpleIpActNaiEntryIndexMismatch => 0x1D,
            Self::RejectedMobileIpActNaiEntryIndexMismatch => 0x1E,
            Self::RejectedSimpleIpPapNaiEntryIndexMismatch => 0x1F,
            Self::RejectedSimpleIpChapNaiEntryIndexMismatch => 0x20,
            Self::RejectedMobileIpNaiEntryIndexMismatch => 0x21,
            Self::RejectedUnexpectedPrlBlockIdChange => 0x22,
            Self::RejectedPrlFormatMismatch => 0x23,
            Self::RejectedHrpdAccessAuthMaxNaiLengthMismatch => 0x24,
            Self::RejectedHrpdAccessAuthChapMaxSsLengthMismatch => 0x25,
            Self::RejectedMmdMaxNumImpuMismatch => 0x26,
            Self::RejectedMmdMaxImpuLengthMismatch => 0x27,
            Self::RejectedMmdMaxNumPcscfMismatch => 0x28,
            Self::RejectedMmdMaxPcscfLengthMismatch => 0x29,
            Self::RejectedUnexpectedSystemTagBlockIdChange => 0x2A,
            Self::RejectedSystemTagFormatMismatch => 0x2B,
            Self::RejectedNumMmsUriMismatch => 0x2C,
            Self::RejectedMmsUriLengthMismatch => 0x2D,
            Self::RejectedInvalidMmsUri => 0x2E,
            Self::RejectedMmssModeSettingsFormatMismatch => 0x2F,
            Self::RejectedMlplFormatMismatch => 0x30,
            Self::RejectedMsplFormatMismatch => 0x31,
            Self::RejectedMmssWlanDownloadParamFormatMismatch => 0x32,
            Self::Other(v) => v,
        }
    }

    pub fn is_accepted(self) -> bool {
        matches!(self, Self::Accepted)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_values_round_trip() {
        for raw in 0x00..=0x32u8 {
            let r = ResultCode::from_u8(raw);
            assert_eq!(r.to_u8(), raw);
            if raw != 0x00 {
                assert!(!r.is_accepted());
            }
        }
    }

    #[test]
    fn unknown_passes_through_other() {
        let r = ResultCode::from_u8(0x80);
        assert_eq!(r, ResultCode::Other(0x80));
        assert_eq!(r.to_u8(), 0x80);
    }

    #[test]
    fn accepted_check() {
        assert!(ResultCode::Accepted.is_accepted());
        assert!(!ResultCode::RejectedInvalidSpc.is_accepted());
    }
}
