//! Protocol Capability Request/Response — C.S0016-D §4.5.1.7 / §3.5.1.7.
//!
//! Request: BS asks the MS for its OTASP capabilities. The base form is a
//! single byte (just the message type); the extended form includes
//! `OTASP_P_REV`, `NUM_CAP_RECORDS`, and a list of `CAP_RECORD_TYPE` values
//! when the BS wants to probe specific capability records.
//!
//! Response: MS firmware revision, model, list of features it supports
//! (each with its own `FEATURE_P_REV`), and an `ADD_LENGTH`-prefixed extra
//! fields region that — per the spec — starts with `BAND_MODE_CAP`.

use crate::Error;
use crate::message::msg_type::PROTOCOL_CAPABILITY;
use crate::message::require_msg_type;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolCapabilityRequest {
    /// `OTASP_P_REV` to advertise. `None` means the BS does not request any
    /// capability records — the message degenerates to a single byte.
    pub otasp_p_rev: Option<u8>,
    /// `CAP_RECORD_TYPE` values. Ignored unless `otasp_p_rev.is_some()`.
    pub cap_record_types: Vec<u8>,
}

impl ProtocolCapabilityRequest {
    pub fn basic() -> Self {
        Self {
            otasp_p_rev: None,
            cap_record_types: vec![],
        }
    }

    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let mut out = vec![PROTOCOL_CAPABILITY];
        if let Some(rev) = self.otasp_p_rev {
            if self.cap_record_types.len() > u8::MAX as usize {
                return Err("too many CAP_RECORD_TYPE values".into());
            }
            out.push(rev);
            out.push(self.cap_record_types.len() as u8);
            out.extend_from_slice(&self.cap_record_types);
        }
        Ok(out)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.is_empty() {
            return Err("ProtocolCapabilityRequest empty".into());
        }
        require_msg_type(bytes[0], PROTOCOL_CAPABILITY)?;
        if bytes.len() == 1 {
            return Ok(Self::basic());
        }
        if bytes.len() < 3 {
            return Err("ProtocolCapabilityRequest truncated".into());
        }
        let otasp_p_rev = bytes[1];
        let n = bytes[2] as usize;
        if bytes.len() < 3 + n {
            return Err("ProtocolCapabilityRequest cap-record truncated".into());
        }
        Ok(Self {
            otasp_p_rev: Some(otasp_p_rev),
            cap_record_types: bytes[3..3 + n].to_vec(),
        })
    }
}

/// FEATURE_ID values from C.S0016-D Table 3.5.1.7-1.
pub mod feature_id {
    pub const NAM_DOWNLOAD: u8 = 0x00;
    pub const KEY_EXCHANGE: u8 = 0x01;
    pub const SSPR: u8 = 0x02;
    pub const SERVICE_PROGRAMMING_LOCK: u8 = 0x03;
    pub const OTASP: u8 = 0x04;
    pub const PUZL: u8 = 0x05;
    pub const PACKET_DATA_3GPD: u8 = 0x06;
    pub const SECURE_MODE: u8 = 0x07;
    pub const MMD: u8 = 0x08;
    pub const SYSTEM_TAG_DOWNLOAD: u8 = 0x09;
    pub const MMS: u8 = 0x0A;
    pub const MMSS: u8 = 0x0B;
    pub const RESERVED_FOR_FUTURE_STANDARDIZATION_START: u8 = 0x0C;
    pub const RESERVED_FOR_FUTURE_STANDARDIZATION_END: u8 = 0xBF;
    pub const MANUFACTURER_SPECIFIC_START: u8 = 0xC0;
    pub const MANUFACTURER_SPECIFIC_END: u8 = 0xFE;
    pub const RESERVED: u8 = 0xFF;
}

pub mod feature_p_rev {
    pub mod nam_download {
        pub const DATA_P_REV_2: u8 = 0x02;
        pub const DATA_P_REV_3_WITH_EHRPD_IMSI: u8 = 0x03;
    }

    pub mod key_exchange {
        pub const A_KEY_PROVISIONING: u8 = 0x02;
        pub const A_KEY_AND_3G_ROOT_KEY_PROVISIONING: u8 = 0x03;
        pub const ROOT_KEY_PROVISIONING: u8 = 0x04;
        pub const ENHANCED_3G_ROOT_KEY_PROVISIONING: u8 = 0x05;
        pub const SERVICE_KEY_GENERATION: u8 = 0x06;
        pub const EHRPD_ROOT_KEY_P_REV_7: u8 = 0x07;
        pub const EHRPD_ROOT_KEY_P_REV_8: u8 = 0x08;
    }

    pub mod sspr {
        pub const PREFERRED_ROAMING_LIST: u8 = 0x01;
        pub const RESERVED: u8 = 0x02;
        pub const EXTENDED_PREFERRED_ROAMING_LIST: u8 = 0x03;
    }

    pub mod secure_mode {
        pub const ROOT_KEY_UNAVAILABLE: u8 = 0x01;
        pub const ROOT_KEY_AVAILABLE: u8 = 0x02;
    }

    pub const REV_1: u8 = 0x01;
    pub const PUZL_REV_2: u8 = 0x02;
    pub const PACKET_DATA_3GPD_REV_3: u8 = 0x03;
}

/// Parsed `FEATURE_ID` classification from C.S0016-D Table 3.5.1.7-1.
///
/// Unknown range values are retained with their raw octet. The table reserves
/// `0x0c..=0xbf` for future standardization, `0xc0..=0xfe` for
/// manufacturer-specific features, and `0xff` as reserved.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeatureId {
    NamDownload,
    KeyExchange,
    SystemSelectionForPreferredRoaming,
    ServiceProgrammingLock,
    OverTheAirServiceProvisioning,
    PreferredUserZoneList,
    PacketData3gpd,
    SecureMode,
    MultimediaDomain,
    SystemTagDownload,
    MultimediaMessagingService,
    MultimodeSystemSelection,
    ReservedForFutureStandardization(u8),
    ManufacturerSpecific(u8),
    Reserved(u8),
}

impl FeatureId {
    pub fn from_u8(value: u8) -> Self {
        match value {
            feature_id::NAM_DOWNLOAD => Self::NamDownload,
            feature_id::KEY_EXCHANGE => Self::KeyExchange,
            feature_id::SSPR => Self::SystemSelectionForPreferredRoaming,
            feature_id::SERVICE_PROGRAMMING_LOCK => Self::ServiceProgrammingLock,
            feature_id::OTASP => Self::OverTheAirServiceProvisioning,
            feature_id::PUZL => Self::PreferredUserZoneList,
            feature_id::PACKET_DATA_3GPD => Self::PacketData3gpd,
            feature_id::SECURE_MODE => Self::SecureMode,
            feature_id::MMD => Self::MultimediaDomain,
            feature_id::SYSTEM_TAG_DOWNLOAD => Self::SystemTagDownload,
            feature_id::MMS => Self::MultimediaMessagingService,
            feature_id::MMSS => Self::MultimodeSystemSelection,
            feature_id::RESERVED_FOR_FUTURE_STANDARDIZATION_START
                ..=feature_id::RESERVED_FOR_FUTURE_STANDARDIZATION_END => {
                Self::ReservedForFutureStandardization(value)
            }
            feature_id::MANUFACTURER_SPECIFIC_START..=feature_id::MANUFACTURER_SPECIFIC_END => {
                Self::ManufacturerSpecific(value)
            }
            _ => Self::Reserved(value),
        }
    }

    pub fn to_u8(self) -> u8 {
        match self {
            Self::NamDownload => feature_id::NAM_DOWNLOAD,
            Self::KeyExchange => feature_id::KEY_EXCHANGE,
            Self::SystemSelectionForPreferredRoaming => feature_id::SSPR,
            Self::ServiceProgrammingLock => feature_id::SERVICE_PROGRAMMING_LOCK,
            Self::OverTheAirServiceProvisioning => feature_id::OTASP,
            Self::PreferredUserZoneList => feature_id::PUZL,
            Self::PacketData3gpd => feature_id::PACKET_DATA_3GPD,
            Self::SecureMode => feature_id::SECURE_MODE,
            Self::MultimediaDomain => feature_id::MMD,
            Self::SystemTagDownload => feature_id::SYSTEM_TAG_DOWNLOAD,
            Self::MultimediaMessagingService => feature_id::MMS,
            Self::MultimodeSystemSelection => feature_id::MMSS,
            Self::ReservedForFutureStandardization(value)
            | Self::ManufacturerSpecific(value)
            | Self::Reserved(value) => value,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::NamDownload => "NAM Download",
            Self::KeyExchange => "Key Exchange",
            Self::SystemSelectionForPreferredRoaming => "System Selection for Preferred Roaming",
            Self::ServiceProgrammingLock => "Service Programming Lock",
            Self::OverTheAirServiceProvisioning => "Over-The-Air Service Provisioning",
            Self::PreferredUserZoneList => "Preferred User Zone List",
            Self::PacketData3gpd => "3G Packet Data",
            Self::SecureMode => "Secure Mode",
            Self::MultimediaDomain => "Multimedia Domain",
            Self::SystemTagDownload => "System Tag Download",
            Self::MultimediaMessagingService => "Multimedia Messaging Service",
            Self::MultimodeSystemSelection => "Multimode System Selection",
            Self::ReservedForFutureStandardization(_) => "Reserved for future standardization",
            Self::ManufacturerSpecific(_) => "Manufacturer-specific feature",
            Self::Reserved(_) => "Reserved",
        }
    }
}

/// Parsed `FEATURE_P_REV`, interpreted in the context of its `FEATURE_ID`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FeatureCapabilityKind {
    NamDownload(NamDownloadPRev),
    KeyExchange(KeyExchangePRev),
    SystemSelectionForPreferredRoaming(SsprPRev),
    ServiceProgrammingLock(SingleRevisionFeature),
    OverTheAirServiceProvisioning(SingleRevisionFeature),
    PreferredUserZoneList(PuzlPRev),
    PacketData3gpd(PacketData3gpdPRev),
    SecureMode(SecureModePRev),
    MultimediaDomain(SingleRevisionFeature),
    SystemTagDownload(SingleRevisionFeature),
    MultimediaMessagingService(SingleRevisionFeature),
    MultimodeSystemSelection(SingleRevisionFeature),
    ReservedForFutureStandardization { feature_id: u8, feature_p_rev: u8 },
    ManufacturerSpecific { feature_id: u8, feature_p_rev: u8 },
    Reserved { feature_id: u8, feature_p_rev: u8 },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NamDownloadPRev {
    DataPRev2,
    DataPRev3WithEhrpdImsiProvisioning,
    Unknown(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyExchangePRev {
    AKeyProvisioning,
    AKeyAnd3gRootKeyProvisioning,
    RootKeyProvisioning,
    Enhanced3gRootKeyProvisioning,
    ServiceKeyGeneration,
    EhrpdRootKeyPRev7,
    EhrpdRootKeyPRev8,
    Unknown(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SsprPRev {
    PreferredRoamingList,
    Reserved,
    ExtendedPreferredRoamingList,
    Unknown(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SingleRevisionFeature {
    Rev1,
    Unknown(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PuzlPRev {
    Rev2,
    Unknown(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacketData3gpdPRev {
    Rev3,
    Unknown(u8),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SecureModePRev {
    RootKeyUnavailable,
    RootKeyAvailable,
    Unknown(u8),
}

/// One `(FEATURE_ID, FEATURE_P_REV)` pair from Table 3.5.1.7-1.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FeatureCapability {
    pub feature_id: u8,
    pub feature_p_rev: u8,
}

impl FeatureCapability {
    pub fn feature(self) -> FeatureId {
        FeatureId::from_u8(self.feature_id)
    }

    pub fn kind(self) -> FeatureCapabilityKind {
        match self.feature() {
            FeatureId::NamDownload => {
                FeatureCapabilityKind::NamDownload(match self.feature_p_rev {
                    feature_p_rev::nam_download::DATA_P_REV_2 => NamDownloadPRev::DataPRev2,
                    feature_p_rev::nam_download::DATA_P_REV_3_WITH_EHRPD_IMSI => {
                        NamDownloadPRev::DataPRev3WithEhrpdImsiProvisioning
                    }
                    value => NamDownloadPRev::Unknown(value),
                })
            }
            FeatureId::KeyExchange => {
                FeatureCapabilityKind::KeyExchange(match self.feature_p_rev {
                    feature_p_rev::key_exchange::A_KEY_PROVISIONING => {
                        KeyExchangePRev::AKeyProvisioning
                    }
                    feature_p_rev::key_exchange::A_KEY_AND_3G_ROOT_KEY_PROVISIONING => {
                        KeyExchangePRev::AKeyAnd3gRootKeyProvisioning
                    }
                    feature_p_rev::key_exchange::ROOT_KEY_PROVISIONING => {
                        KeyExchangePRev::RootKeyProvisioning
                    }
                    feature_p_rev::key_exchange::ENHANCED_3G_ROOT_KEY_PROVISIONING => {
                        KeyExchangePRev::Enhanced3gRootKeyProvisioning
                    }
                    feature_p_rev::key_exchange::SERVICE_KEY_GENERATION => {
                        KeyExchangePRev::ServiceKeyGeneration
                    }
                    feature_p_rev::key_exchange::EHRPD_ROOT_KEY_P_REV_7 => {
                        KeyExchangePRev::EhrpdRootKeyPRev7
                    }
                    feature_p_rev::key_exchange::EHRPD_ROOT_KEY_P_REV_8 => {
                        KeyExchangePRev::EhrpdRootKeyPRev8
                    }
                    value => KeyExchangePRev::Unknown(value),
                })
            }
            FeatureId::SystemSelectionForPreferredRoaming => {
                FeatureCapabilityKind::SystemSelectionForPreferredRoaming(
                    match self.feature_p_rev {
                        feature_p_rev::sspr::PREFERRED_ROAMING_LIST => {
                            SsprPRev::PreferredRoamingList
                        }
                        feature_p_rev::sspr::RESERVED => SsprPRev::Reserved,
                        feature_p_rev::sspr::EXTENDED_PREFERRED_ROAMING_LIST => {
                            SsprPRev::ExtendedPreferredRoamingList
                        }
                        value => SsprPRev::Unknown(value),
                    },
                )
            }
            FeatureId::ServiceProgrammingLock => {
                FeatureCapabilityKind::ServiceProgrammingLock(single_revision(self.feature_p_rev))
            }
            FeatureId::OverTheAirServiceProvisioning => {
                FeatureCapabilityKind::OverTheAirServiceProvisioning(single_revision(
                    self.feature_p_rev,
                ))
            }
            FeatureId::PreferredUserZoneList => {
                FeatureCapabilityKind::PreferredUserZoneList(match self.feature_p_rev {
                    feature_p_rev::PUZL_REV_2 => PuzlPRev::Rev2,
                    value => PuzlPRev::Unknown(value),
                })
            }
            FeatureId::PacketData3gpd => {
                FeatureCapabilityKind::PacketData3gpd(match self.feature_p_rev {
                    feature_p_rev::PACKET_DATA_3GPD_REV_3 => PacketData3gpdPRev::Rev3,
                    value => PacketData3gpdPRev::Unknown(value),
                })
            }
            FeatureId::SecureMode => FeatureCapabilityKind::SecureMode(match self.feature_p_rev {
                feature_p_rev::secure_mode::ROOT_KEY_UNAVAILABLE => {
                    SecureModePRev::RootKeyUnavailable
                }
                feature_p_rev::secure_mode::ROOT_KEY_AVAILABLE => SecureModePRev::RootKeyAvailable,
                value => SecureModePRev::Unknown(value),
            }),
            FeatureId::MultimediaDomain => {
                FeatureCapabilityKind::MultimediaDomain(single_revision(self.feature_p_rev))
            }
            FeatureId::SystemTagDownload => {
                FeatureCapabilityKind::SystemTagDownload(single_revision(self.feature_p_rev))
            }
            FeatureId::MultimediaMessagingService => {
                FeatureCapabilityKind::MultimediaMessagingService(single_revision(
                    self.feature_p_rev,
                ))
            }
            FeatureId::MultimodeSystemSelection => {
                FeatureCapabilityKind::MultimodeSystemSelection(single_revision(self.feature_p_rev))
            }
            FeatureId::ReservedForFutureStandardization(feature_id) => {
                FeatureCapabilityKind::ReservedForFutureStandardization {
                    feature_id,
                    feature_p_rev: self.feature_p_rev,
                }
            }
            FeatureId::ManufacturerSpecific(feature_id) => {
                FeatureCapabilityKind::ManufacturerSpecific {
                    feature_id,
                    feature_p_rev: self.feature_p_rev,
                }
            }
            FeatureId::Reserved(feature_id) => FeatureCapabilityKind::Reserved {
                feature_id,
                feature_p_rev: self.feature_p_rev,
            },
        }
    }
}

fn single_revision(value: u8) -> SingleRevisionFeature {
    match value {
        feature_p_rev::REV_1 => SingleRevisionFeature::Rev1,
        value => SingleRevisionFeature::Unknown(value),
    }
}

/// `BAND_MODE_CAP` per Table 3.5.1.7-2 (5 bits used, 3 reserved). Decomposed
/// into named flags; the raw bit pattern (the on-wire byte) is recoverable
/// from `to_byte()`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct BandModeCap {
    /// On-wire byte. Carried so RESERVED bits (2–0) are visible even though
    /// the spec mandates them to be 0 — vintage handsets sometimes don't
    /// comply.
    pub raw: u8,
    pub band_class_0_analog: bool,
    pub band_class_0_cdma: bool,
    pub band_class_1_cdma: bool,
    pub band_class_3_cdma: bool,
    pub band_class_6_cdma: bool,
}

impl BandModeCap {
    pub fn to_byte(self) -> u8 {
        let derived = ((self.band_class_0_analog as u8) << 7)
            | ((self.band_class_0_cdma as u8) << 6)
            | ((self.band_class_1_cdma as u8) << 5)
            | ((self.band_class_3_cdma as u8) << 4)
            | ((self.band_class_6_cdma as u8) << 3);
        derived | (self.raw & 0b0000_0111)
    }

    pub fn from_byte(b: u8) -> Self {
        Self {
            raw: b,
            band_class_0_analog: (b & 0x80) != 0,
            band_class_0_cdma: (b & 0x40) != 0,
            band_class_1_cdma: (b & 0x20) != 0,
            band_class_3_cdma: (b & 0x10) != 0,
            band_class_6_cdma: (b & 0x08) != 0,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProtocolCapabilityResponse {
    pub mob_firm_rev: u16,
    pub mob_model: u8,
    pub features: Vec<FeatureCapability>,
    pub band_mode_cap: BandModeCap,
    /// Octets in the additional fields region beyond the leading
    /// `BAND_MODE_CAP`. Carried verbatim because the spec leaves future
    /// extensions open.
    pub additional_trailing: Vec<u8>,
}

impl ProtocolCapabilityResponse {
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        if self.features.len() > u8::MAX as usize {
            return Err("too many features".into());
        }
        let add_len = 1 + self.additional_trailing.len();
        if add_len > u8::MAX as usize {
            return Err("ADD_LENGTH overflow".into());
        }
        let mut out = Vec::with_capacity(5 + 2 * self.features.len() + add_len);
        out.push(PROTOCOL_CAPABILITY);
        out.extend_from_slice(&self.mob_firm_rev.to_be_bytes());
        out.push(self.mob_model);
        out.push(self.features.len() as u8);
        for f in &self.features {
            out.push(f.feature_id);
            out.push(f.feature_p_rev);
        }
        out.push(add_len as u8);
        out.push(self.band_mode_cap.to_byte());
        out.extend_from_slice(&self.additional_trailing);
        Ok(out)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < 6 {
            return Err("ProtocolCapabilityResponse too short".into());
        }
        require_msg_type(bytes[0], PROTOCOL_CAPABILITY)?;
        let mob_firm_rev = u16::from_be_bytes([bytes[1], bytes[2]]);
        let mob_model = bytes[3];
        let nfeat = bytes[4] as usize;
        let feat_end = 5 + 2 * nfeat;
        if bytes.len() < feat_end + 1 {
            return Err("ProtocolCapabilityResponse features truncated".into());
        }
        let mut features = Vec::with_capacity(nfeat);
        for i in 0..nfeat {
            features.push(FeatureCapability {
                feature_id: bytes[5 + 2 * i],
                feature_p_rev: bytes[5 + 2 * i + 1],
            });
        }
        let add_len = bytes[feat_end] as usize;
        if add_len < 1 {
            return Err("ProtocolCapabilityResponse ADD_LENGTH < 1".into());
        }
        let add_start = feat_end + 1;
        if bytes.len() < add_start + add_len {
            return Err("ProtocolCapabilityResponse additional region truncated".into());
        }
        let band_mode_cap = BandModeCap::from_byte(bytes[add_start]);
        let additional_trailing = bytes[add_start + 1..add_start + add_len].to_vec();
        Ok(Self {
            mob_firm_rev,
            mob_model,
            features,
            band_mode_cap,
            additional_trailing,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pcap_request_basic_is_single_byte() {
        let r = ProtocolCapabilityRequest::basic();
        assert_eq!(r.encode().unwrap(), vec![0x06]);
        assert_eq!(ProtocolCapabilityRequest::decode(&[0x06]).unwrap(), r);
    }

    #[test]
    fn pcap_request_with_cap_records_round_trip() {
        let r = ProtocolCapabilityRequest {
            otasp_p_rev: Some(0x04),
            cap_record_types: vec![0x01, 0x02, 0x03],
        };
        let bytes = r.encode().unwrap();
        assert_eq!(bytes, vec![0x06, 0x04, 0x03, 0x01, 0x02, 0x03]);
        assert_eq!(ProtocolCapabilityRequest::decode(&bytes).unwrap(), r);
    }

    #[test]
    fn pcap_response_round_trip_minimal() {
        let r = ProtocolCapabilityResponse {
            mob_firm_rev: 0x0102,
            mob_model: 0x42,
            features: vec![
                FeatureCapability {
                    feature_id: 0x00,
                    feature_p_rev: 0x02,
                },
                FeatureCapability {
                    feature_id: 0x04,
                    feature_p_rev: 0x01,
                },
            ],
            band_mode_cap: BandModeCap::from_byte(0b0110_0000),
            additional_trailing: vec![],
        };
        let bytes = r.encode().unwrap();
        assert_eq!(
            bytes,
            vec![
                0x06,
                0x01,
                0x02,
                0x42,
                0x02, // header through NUM_FEATURES
                0x00,
                0x02,
                0x04,
                0x01,        // 2 features
                0x01,        // ADD_LENGTH
                0b0110_0000, // BAND_MODE_CAP
            ]
        );
        assert_eq!(ProtocolCapabilityResponse::decode(&bytes).unwrap(), r);
    }

    #[test]
    fn feature_id_table_matches_c_s0016_d() {
        let cases = [
            (0x00, FeatureId::NamDownload),
            (0x01, FeatureId::KeyExchange),
            (0x02, FeatureId::SystemSelectionForPreferredRoaming),
            (0x03, FeatureId::ServiceProgrammingLock),
            (0x04, FeatureId::OverTheAirServiceProvisioning),
            (0x05, FeatureId::PreferredUserZoneList),
            (0x06, FeatureId::PacketData3gpd),
            (0x07, FeatureId::SecureMode),
            (0x08, FeatureId::MultimediaDomain),
            (0x09, FeatureId::SystemTagDownload),
            (0x0A, FeatureId::MultimediaMessagingService),
            (0x0B, FeatureId::MultimodeSystemSelection),
            (0x0C, FeatureId::ReservedForFutureStandardization(0x0C)),
            (0xBF, FeatureId::ReservedForFutureStandardization(0xBF)),
            (0xC0, FeatureId::ManufacturerSpecific(0xC0)),
            (0xFE, FeatureId::ManufacturerSpecific(0xFE)),
            (0xFF, FeatureId::Reserved(0xFF)),
        ];

        for (raw, expected) in cases {
            assert_eq!(FeatureId::from_u8(raw), expected);
            assert_eq!(expected.to_u8(), raw);
        }
    }

    #[test]
    fn feature_revisions_are_interpreted_by_feature_id() {
        let cases = [
            (
                (0x00, 0x02),
                FeatureCapabilityKind::NamDownload(NamDownloadPRev::DataPRev2),
            ),
            (
                (0x00, 0x03),
                FeatureCapabilityKind::NamDownload(
                    NamDownloadPRev::DataPRev3WithEhrpdImsiProvisioning,
                ),
            ),
            (
                (0x01, 0x02),
                FeatureCapabilityKind::KeyExchange(KeyExchangePRev::AKeyProvisioning),
            ),
            (
                (0x01, 0x03),
                FeatureCapabilityKind::KeyExchange(KeyExchangePRev::AKeyAnd3gRootKeyProvisioning),
            ),
            (
                (0x01, 0x04),
                FeatureCapabilityKind::KeyExchange(KeyExchangePRev::RootKeyProvisioning),
            ),
            (
                (0x01, 0x05),
                FeatureCapabilityKind::KeyExchange(KeyExchangePRev::Enhanced3gRootKeyProvisioning),
            ),
            (
                (0x01, 0x06),
                FeatureCapabilityKind::KeyExchange(KeyExchangePRev::ServiceKeyGeneration),
            ),
            (
                (0x01, 0x07),
                FeatureCapabilityKind::KeyExchange(KeyExchangePRev::EhrpdRootKeyPRev7),
            ),
            (
                (0x01, 0x08),
                FeatureCapabilityKind::KeyExchange(KeyExchangePRev::EhrpdRootKeyPRev8),
            ),
            (
                (0x02, 0x01),
                FeatureCapabilityKind::SystemSelectionForPreferredRoaming(
                    SsprPRev::PreferredRoamingList,
                ),
            ),
            (
                (0x02, 0x02),
                FeatureCapabilityKind::SystemSelectionForPreferredRoaming(SsprPRev::Reserved),
            ),
            (
                (0x02, 0x03),
                FeatureCapabilityKind::SystemSelectionForPreferredRoaming(
                    SsprPRev::ExtendedPreferredRoamingList,
                ),
            ),
            (
                (0x03, 0x01),
                FeatureCapabilityKind::ServiceProgrammingLock(SingleRevisionFeature::Rev1),
            ),
            (
                (0x04, 0x01),
                FeatureCapabilityKind::OverTheAirServiceProvisioning(SingleRevisionFeature::Rev1),
            ),
            (
                (0x05, 0x02),
                FeatureCapabilityKind::PreferredUserZoneList(PuzlPRev::Rev2),
            ),
            (
                (0x06, 0x03),
                FeatureCapabilityKind::PacketData3gpd(PacketData3gpdPRev::Rev3),
            ),
            (
                (0x07, 0x01),
                FeatureCapabilityKind::SecureMode(SecureModePRev::RootKeyUnavailable),
            ),
            (
                (0x07, 0x02),
                FeatureCapabilityKind::SecureMode(SecureModePRev::RootKeyAvailable),
            ),
            (
                (0x08, 0x01),
                FeatureCapabilityKind::MultimediaDomain(SingleRevisionFeature::Rev1),
            ),
            (
                (0x09, 0x01),
                FeatureCapabilityKind::SystemTagDownload(SingleRevisionFeature::Rev1),
            ),
            (
                (0x0A, 0x01),
                FeatureCapabilityKind::MultimediaMessagingService(SingleRevisionFeature::Rev1),
            ),
            (
                (0x0B, 0x01),
                FeatureCapabilityKind::MultimodeSystemSelection(SingleRevisionFeature::Rev1),
            ),
        ];

        for ((feature_id, feature_p_rev), expected) in cases {
            assert_eq!(
                FeatureCapability {
                    feature_id,
                    feature_p_rev,
                }
                .kind(),
                expected
            );
        }

        assert_eq!(
            FeatureCapability {
                feature_id: 0xC0,
                feature_p_rev: 0x42,
            }
            .kind(),
            FeatureCapabilityKind::ManufacturerSpecific {
                feature_id: 0xC0,
                feature_p_rev: 0x42,
            }
        );
    }

    #[test]
    fn band_mode_cap_round_trip_all_flags() {
        let wire = 0b1111_1000;
        let b = BandModeCap::from_byte(wire);
        assert_eq!(b.to_byte(), wire);
        assert!(b.band_class_0_analog);
        assert!(b.band_class_0_cdma);
        assert!(b.band_class_1_cdma);
        assert!(b.band_class_3_cdma);
        assert!(b.band_class_6_cdma);
        assert_eq!(b.raw, wire);
    }
}
