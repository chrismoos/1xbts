//! Exact typed models for A9 signaling messages.

use crate::{
    A8TrafficId, ElementId, Error, InformationElement, Message, MessageType, Result, decode, encode,
};

/// Exact typed A9 connection reference payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConRef(pub u8);

impl ConRef {
    /// Encodes the connection-reference payload.
    pub const fn encode(self) -> [u8; 1] {
        [self.0]
    }

    /// Decodes the connection-reference payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 1 {
            return Err(Error::InvalidLength {
                expected: 1,
                actual: input.len(),
            });
        }
        Ok(Self(input[0]))
    }
}

/// Exact typed correlation identifier used to associate A9 requests and responses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CorrelationId(pub [u8; 4]);

impl CorrelationId {
    /// Encodes the correlation identifier.
    pub const fn encode(self) -> [u8; 4] {
        self.0
    }

    /// Decodes the correlation identifier.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 4 {
            return Err(Error::InvalidLength {
                expected: 4,
                actual: input.len(),
            });
        }
        Ok(Self([input[0], input[1], input[2], input[3]]))
    }
}

/// Exact typed BSC identifier payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BscId(pub Vec<u8>);

impl BscId {
    /// Encodes the BSC identifier payload.
    pub fn encode(&self) -> Result<&[u8]> {
        if self.0.is_empty() || self.0.len() > 6 {
            return Err(Error::InvalidLength {
                expected: 6,
                actual: self.0.len(),
            });
        }
        Ok(&self.0)
    }

    /// Decodes the BSC identifier payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.is_empty() || input.len() > 6 {
            return Err(Error::InvalidLength {
                expected: 6,
                actual: input.len(),
            });
        }
        Ok(Self(input.to_vec()))
    }
}

/// Exact typed mobile identity payload for A9.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MobileIdentity {
    Imsi(String),
    Esn(u32),
    Meid(Meid),
}

impl MobileIdentity {
    /// Encodes the mobile identity payload.
    pub fn encode(&self) -> Result<Vec<u8>> {
        match self {
            Self::Imsi(imsi) => encode_imsi(imsi),
            Self::Esn(esn) => {
                let mut out = vec![0x05];
                out.extend_from_slice(&esn.to_be_bytes());
                Ok(out)
            }
            Self::Meid(meid) => meid.encode(),
        }
    }

    /// Decodes the mobile identity payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let Some((&first, rest)) = input.split_first() else {
            return Err(Error::InvalidLength {
                expected: 1,
                actual: 0,
            });
        };
        match first & 0x07 {
            0x06 => decode_imsi(first, rest),
            0x05 => {
                if input.len() != 5 {
                    return Err(Error::InvalidLength {
                        expected: 5,
                        actual: input.len(),
                    });
                }
                Ok(Self::Esn(u32::from_be_bytes([
                    input[1], input[2], input[3], input[4],
                ])))
            }
            0x01 => Ok(Self::Meid(Meid::decode(input)?)),
            other => Err(Error::UnknownInformationElement(other)),
        }
    }
}

/// Exact typed service-option payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceOptionValue(pub u16);

impl ServiceOptionValue {
    /// SO3: EVRC-A / IS-127 narrowband.
    pub const EVRC_A: Self = Self(3);

    /// SO6: Short Message Services.
    pub const SMS: Self = Self(6);

    /// SO7: Packet data, async/fax data service.
    pub const PACKET_DATA: Self = Self(7);

    /// SO33: High-rate packet data service.
    pub const HIGH_RATE_PACKET_DATA: Self = Self(33);

    /// SO68: EVRC-B narrowband.
    pub const EVRC_B: Self = Self(68);

    /// SO70: EVRC-WB.
    pub const EVRC_WB: Self = Self(70);

    /// Encodes the service-option payload.
    pub const fn encode(self) -> [u8; 2] {
        self.0.to_be_bytes()
    }

    /// Decodes the service-option payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 2 {
            return Err(Error::InvalidLength {
                expected: 2,
                actual: input.len(),
            });
        }
        Ok(Self(u16::from_be_bytes([input[0], input[1]])))
    }
}

/// Exact typed one-octet cause payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CauseValue(pub u8);

impl CauseValue {
    /// Encodes the cause payload.
    pub const fn encode(self) -> [u8; 1] {
        [self.0]
    }

    /// Decodes the cause payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 1 {
            return Err(Error::InvalidLength {
                expected: 1,
                actual: input.len(),
            });
        }
        Ok(Self(input[0]))
    }
}

/// Exact typed QoS payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QualityOfServiceParametersTyped {
    pub packet_priority: u8,
}

impl QualityOfServiceParametersTyped {
    /// Encodes the QoS payload.
    pub fn encode(self) -> Result<[u8; 1]> {
        if self.packet_priority > 0x0d {
            return Err(Error::InvalidValue {
                context: "QualityOfServiceParameters.packet_priority",
                value: self.packet_priority as u32,
            });
        }
        Ok([self.packet_priority & 0x0f])
    }

    /// Decodes the QoS payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 1 {
            return Err(Error::InvalidLength {
                expected: 1,
                actual: input.len(),
            });
        }
        if input[0] & 0xf0 != 0 {
            return Err(Error::InvalidValue {
                context: "QualityOfServiceParameters.reserved_bits",
                value: input[0] as u32,
            });
        }
        let packet_priority = input[0] & 0x0f;
        if packet_priority > 0x0d {
            return Err(Error::InvalidValue {
                context: "QualityOfServiceParameters.packet_priority",
                value: packet_priority as u32,
            });
        }
        Ok(Self { packet_priority })
    }
}

/// Exact typed A9 indicator payload carried on `A9-Setup-A8`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct A9Indicators {
    pub packet_boundary_supported: bool,
    pub gre_segmentation_supported: bool,
    pub sdb_supported: bool,
    pub ccpd_mode: bool,
    pub data_ready: bool,
    pub handoff: bool,
}

impl A9Indicators {
    /// Encodes the indicators payload.
    pub const fn encode(self) -> [u8; 1] {
        [((self.packet_boundary_supported as u8) << 6)
            | ((self.gre_segmentation_supported as u8) << 5)
            | ((self.sdb_supported as u8) << 4)
            | ((self.ccpd_mode as u8) << 3)
            | ((self.data_ready as u8) << 1)
            | (self.handoff as u8)]
    }

    /// Decodes the indicators payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 1 {
            return Err(Error::InvalidLength {
                expected: 1,
                actual: input.len(),
            });
        }
        if input[0] & 0x84 != 0 {
            return Err(Error::InvalidValue {
                context: "A9Indicators.reserved_bits",
                value: input[0] as u32,
            });
        }
        Ok(Self {
            packet_boundary_supported: input[0] & 0x40 != 0,
            gre_segmentation_supported: input[0] & 0x20 != 0,
            sdb_supported: input[0] & 0x10 != 0,
            ccpd_mode: input[0] & 0x08 != 0,
            data_ready: input[0] & 0x02 != 0,
            handoff: input[0] & 0x01 != 0,
        })
    }
}

/// Exact typed MEID payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Meid(pub [u8; 7]);

impl Meid {
    /// Encodes the MEID payload.
    pub fn encode(self) -> Result<Vec<u8>> {
        let mut digits = [0u8; 14];
        for (index, byte) in self.0.iter().copied().enumerate() {
            digits[index * 2] = byte >> 4;
            digits[index * 2 + 1] = byte & 0x0f;
        }
        let mut out = Vec::with_capacity(8);
        out.push((digits[0] << 4) | 0x01);
        let mut index = 1;
        while index < digits.len() {
            let low = digits[index];
            let high = if index + 1 < digits.len() {
                digits[index + 1]
            } else {
                0x0f
            };
            out.push((high << 4) | low);
            index += 2;
        }
        Ok(out)
    }

    /// Decodes the MEID payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 8 {
            return Err(Error::InvalidLength {
                expected: 8,
                actual: input.len(),
            });
        }
        if input[0] & 0x08 != 0 || input[0] & 0x07 != 0x01 {
            return Err(Error::InvalidValue {
                context: "MobileIdentity.meid.header",
                value: input[0] as u32,
            });
        }
        if input[7] >> 4 != 0x0f {
            return Err(Error::InvalidValue {
                context: "MobileIdentity.meid.fill",
                value: (input[7] >> 4) as u32,
            });
        }
        let mut digits = [0u8; 14];
        digits[0] = input[0] >> 4;
        let mut index = 1usize;
        for byte in &input[1..] {
            let low = byte & 0x0f;
            let high = byte >> 4;
            if index < digits.len() {
                digits[index] = low;
                index += 1;
            }
            if index < digits.len() {
                digits[index] = high;
                index += 1;
            } else if high != 0x0f {
                return Err(Error::InvalidValue {
                    context: "MobileIdentity.meid.fill",
                    value: high as u32,
                });
            }
        }
        let mut out = [0u8; 7];
        for (index, chunk) in digits.chunks_exact(2).enumerate() {
            out[index] = (chunk[0] << 4) | chunk[1];
        }
        Ok(Self(out))
    }
}

/// Exact typed User Zone identifier payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct UserZoneId(pub u16);

impl UserZoneId {
    /// Encodes the User Zone identifier payload.
    pub const fn encode(self) -> [u8; 2] {
        self.0.to_be_bytes()
    }

    /// Decodes the User Zone identifier payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 2 {
            return Err(Error::InvalidLength {
                expected: 2,
                actual: input.len(),
            });
        }
        Ok(Self(u16::from_be_bytes([input[0], input[1]])))
    }
}

/// Exact typed data-count payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DataCount(pub u16);

impl DataCount {
    /// Encodes the data-count payload.
    pub const fn encode(self) -> [u8; 2] {
        self.0.to_be_bytes()
    }

    /// Decodes the data-count payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 2 {
            return Err(Error::InvalidLength {
                expected: 2,
                actual: input.len(),
            });
        }
        Ok(Self(u16::from_be_bytes([input[0], input[1]])))
    }
}

/// Exact typed PDSN IPv4 address payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PdsnIpAddress(pub [u8; 4]);

impl PdsnIpAddress {
    /// Encodes the IPv4 address payload.
    pub const fn encode(self) -> [u8; 4] {
        self.0
    }

    /// Decodes the IPv4 address payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 4 {
            return Err(Error::InvalidLength {
                expected: 4,
                actual: input.len(),
            });
        }
        Ok(Self([input[0], input[1], input[2], input[3]]))
    }
}

/// Exact typed call-connection-reference payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CallConnectionReference {
    /// Unique market identifier allocated by the operator.
    pub market_id: u16,
    /// Entity identifier of the generator of this reference.
    pub generating_entity_id: u16,
    /// Connection-reference value allocated by the generating entity.
    pub value: u32,
}

impl CallConnectionReference {
    /// Builds a call-connection-reference from its exact component fields.
    pub const fn new(market_id: u16, generating_entity_id: u16, value: u32) -> Self {
        Self {
            market_id,
            generating_entity_id,
            value,
        }
    }

    /// Encodes the call-connection-reference payload.
    pub const fn encode(self) -> [u8; 8] {
        let market_id = self.market_id.to_be_bytes();
        let generating_entity_id = self.generating_entity_id.to_be_bytes();
        let value = self.value.to_be_bytes();
        [
            market_id[0],
            market_id[1],
            generating_entity_id[0],
            generating_entity_id[1],
            value[0],
            value[1],
            value[2],
            value[3],
        ]
    }

    /// Decodes the call-connection-reference payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 8 {
            return Err(Error::InvalidLength {
                expected: 8,
                actual: input.len(),
            });
        }
        Ok(Self {
            market_id: u16::from_be_bytes([input[0], input[1]]),
            generating_entity_id: u16::from_be_bytes([input[2], input[3]]),
            value: u32::from_be_bytes([input[4], input[5], input[6], input[7]]),
        })
    }
}

/// Exact typed SR_ID payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SrId(pub u8);

impl SrId {
    /// Encodes the SR_ID payload.
    pub fn encode(self) -> Result<[u8; 1]> {
        if self.0 == 0 || self.0 > 0x7f {
            return Err(Error::InvalidValue {
                context: "SrId.value",
                value: self.0 as u32,
            });
        }
        Ok([self.0 & 0x7f])
    }

    /// Decodes the SR_ID payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 1 {
            return Err(Error::InvalidLength {
                expected: 1,
                actual: input.len(),
            });
        }
        if input[0] & 0x80 != 0 {
            return Err(Error::InvalidValue {
                context: "SrId.reserved_bits",
                value: input[0] as u32,
            });
        }
        if input[0] == 0 {
            return Err(Error::InvalidValue {
                context: "SrId.value",
                value: 0,
            });
        }
        Ok(Self(input[0] & 0x7f))
    }
}

/// Exact typed bit-exact IS-2000 service-configuration-record payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Is2000ServiceConfigurationRecord {
    pub fill_bits: u8,
    pub content: Vec<u8>,
}

impl Is2000ServiceConfigurationRecord {
    /// Encodes the record payload.
    pub fn encode(&self) -> Result<Vec<u8>> {
        if self.fill_bits > 7 {
            return Err(Error::InvalidValue {
                context: "Is2000ServiceConfigurationRecord.fill_bits",
                value: self.fill_bits as u32,
            });
        }
        if self.content.is_empty() || self.content.len() > u8::MAX as usize {
            return Err(Error::InvalidLength {
                expected: u8::MAX as usize,
                actual: self.content.len(),
            });
        }
        let mut out = Vec::with_capacity(self.content.len() + 2);
        out.push(self.content.len() as u8);
        out.push(self.fill_bits & 0x07);
        out.extend_from_slice(&self.content);
        Ok(out)
    }

    /// Decodes the record payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() < 2 {
            return Err(Error::InvalidLength {
                expected: 2,
                actual: input.len(),
            });
        }
        let octet_count = input[0] as usize;
        if input.len() != octet_count + 2 {
            return Err(Error::InvalidLength {
                expected: octet_count + 2,
                actual: input.len(),
            });
        }
        if input[1] & 0xf8 != 0 {
            return Err(Error::InvalidValue {
                context: "Is2000ServiceConfigurationRecord.reserved_bits",
                value: input[1] as u32,
            });
        }
        Ok(Self {
            fill_bits: input[1] & 0x07,
            content: input[2..].to_vec(),
        })
    }
}

/// Exact typed anchor PDSN IPv4 address payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnchorPdsnAddress(pub [u8; 4]);

impl AnchorPdsnAddress {
    /// Encodes the IPv4 address payload.
    pub const fn encode(self) -> [u8; 4] {
        self.0
    }

    /// Decodes the IPv4 address payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 4 {
            return Err(Error::InvalidLength {
                expected: 4,
                actual: input.len(),
            });
        }
        Ok(Self([input[0], input[1], input[2], input[3]]))
    }
}

/// Exact typed anchor P-P IPv4 address payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AnchorPpAddress(pub [u8; 4]);

impl AnchorPpAddress {
    /// Encodes the IPv4 address payload.
    pub const fn encode(self) -> [u8; 4] {
        self.0
    }

    /// Decodes the IPv4 address payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 4 {
            return Err(Error::InvalidLength {
                expected: 4,
                actual: input.len(),
            });
        }
        Ok(Self([input[0], input[1], input[2], input[3]]))
    }
}

/// Exact typed software-version payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SoftwareVersion {
    pub ios_major_revision_level: u8,
    pub ios_minor_revision_level: u8,
    pub ios_point_release_level: u8,
    pub manufacturer_carrier_software_information: String,
}

impl SoftwareVersion {
    /// Encodes the software-version payload.
    pub fn encode(&self) -> Result<Vec<u8>> {
        if !self
            .manufacturer_carrier_software_information
            .bytes()
            .all(|byte| (0x20..=0x7e).contains(&byte))
        {
            return Err(Error::InvalidValue {
                context: "SoftwareVersion.manufacturer_carrier_software_information",
                value: 0,
            });
        }
        let mut out = Vec::with_capacity(3 + self.manufacturer_carrier_software_information.len());
        out.push(self.ios_major_revision_level);
        out.push(self.ios_minor_revision_level);
        out.push(self.ios_point_release_level);
        out.extend_from_slice(self.manufacturer_carrier_software_information.as_bytes());
        Ok(out)
    }

    /// Decodes the software-version payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() < 3 {
            return Err(Error::InvalidLength {
                expected: 3,
                actual: input.len(),
            });
        }
        let trailer = &input[3..];
        if !trailer.iter().all(|byte| (0x20..=0x7e).contains(byte)) {
            return Err(Error::InvalidValue {
                context: "SoftwareVersion.manufacturer_carrier_software_information",
                value: 0,
            });
        }
        Ok(Self {
            ios_major_revision_level: input[0],
            ios_minor_revision_level: input[1],
            ios_point_release_level: input[2],
            manufacturer_carrier_software_information: String::from_utf8(trailer.to_vec())
                .map_err(|_| Error::InvalidValue {
                    context: "SoftwareVersion.manufacturer_carrier_software_information",
                    value: 0,
                })?,
        })
    }
}

/// Exact typed RN-PDIT payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RnPdit(pub u8);

impl RnPdit {
    /// Encodes the RN-PDIT payload.
    pub fn encode(self) -> Result<[u8; 1]> {
        if self.0 == 0 {
            return Err(Error::InvalidValue {
                context: "RnPdit.value",
                value: 0,
            });
        }
        Ok([self.0])
    }

    /// Decodes the RN-PDIT payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 1 {
            return Err(Error::InvalidLength {
                expected: 1,
                actual: input.len(),
            });
        }
        if input[0] == 0 {
            return Err(Error::InvalidValue {
                context: "RnPdit.value",
                value: 0,
            });
        }
        Ok(Self(input[0]))
    }
}

/// Exact typed ADDS User Part payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddsUserPart {
    pub data_burst_type: u8,
    pub application_data_message: Vec<u8>,
}

impl AddsUserPart {
    /// Builds a short-data-burst ADDS user part.
    pub fn short_data_burst(application_data_message: impl Into<Vec<u8>>) -> Self {
        Self {
            data_burst_type: 0x06,
            application_data_message: application_data_message.into(),
        }
    }

    /// Encodes the ADDS user part payload.
    pub fn encode(&self) -> Result<Vec<u8>> {
        if self.application_data_message.is_empty() {
            return Err(Error::InvalidLength {
                expected: 1,
                actual: 0,
            });
        }
        if self.data_burst_type != 0x06 {
            return Err(Error::InvalidValue {
                context: "AddsUserPart.data_burst_type",
                value: self.data_burst_type as u32,
            });
        }
        let mut out = Vec::with_capacity(self.application_data_message.len() + 1);
        out.push(self.data_burst_type);
        out.extend_from_slice(&self.application_data_message);
        Ok(out)
    }

    /// Decodes the ADDS user part payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.is_empty() {
            return Err(Error::InvalidLength {
                expected: 1,
                actual: 0,
            });
        }
        if input[0] & 0xc0 != 0 {
            return Err(Error::InvalidValue {
                context: "AddsUserPart.reserved_bits",
                value: input[0] as u32,
            });
        }
        let data_burst_type = input[0] & 0x3f;
        if data_burst_type != 0x06 {
            return Err(Error::InvalidValue {
                context: "AddsUserPart.data_burst_type",
                value: data_burst_type as u32,
            });
        }
        Ok(Self {
            data_burst_type,
            application_data_message: input[1..].to_vec(),
        })
    }
}

/// Exact typed `A9-Setup-A8` message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SetupA8Message {
    pub call_connection_reference: Option<CallConnectionReference>,
    pub correlation_id: Option<CorrelationId>,
    pub imsi: Option<String>,
    pub esn: Option<u32>,
    pub meid: Option<Meid>,
    pub con_ref: ConRef,
    pub quality_of_service_parameters: Option<QualityOfServiceParametersTyped>,
    pub bsc_id: BscId,
    pub a8_traffic_id: A8TrafficId,
    pub service_option: ServiceOptionValue,
    pub a9_indicators: A9Indicators,
    pub user_zone_id: Option<UserZoneId>,
}

impl SetupA8Message {
    /// Encodes the message.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut elements = Vec::new();
        push_optional(
            &mut elements,
            ElementId::CallConnectionReference,
            self.call_connection_reference.map(|v| v.encode().to_vec()),
        );
        push_optional(
            &mut elements,
            ElementId::CorrelationId,
            self.correlation_id.map(|v| v.encode().to_vec()),
        );
        push_mobile_identities(&mut elements, self.imsi.as_deref(), self.esn, self.meid)?;
        elements.push(InformationElement::new(
            ElementId::ConRef,
            self.con_ref.encode(),
        ));
        if let Some(value) = self.quality_of_service_parameters {
            elements.push(InformationElement::new(
                ElementId::QualityOfServiceParameters,
                value.encode()?.to_vec(),
            ));
        }
        elements.push(InformationElement::new(
            ElementId::BscId,
            self.bsc_id.encode()?.to_vec(),
        ));
        elements.push(InformationElement::new(
            ElementId::A8TrafficId,
            self.a8_traffic_id.encode(),
        ));
        elements.push(InformationElement::new(
            ElementId::ServiceOption,
            self.service_option.encode(),
        ));
        elements.push(InformationElement::new(
            ElementId::A9Indicators,
            self.a9_indicators.encode(),
        ));
        push_optional(
            &mut elements,
            ElementId::UserZoneId,
            self.user_zone_id.map(|v| v.encode().to_vec()),
        );
        encode(&Message::new(MessageType::SetupA8, elements)?)
    }

    /// Decodes the message.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let message = decode_typed_message(input, MessageType::SetupA8)?;
        let mut decoded = SetupA8Message {
            call_connection_reference: None,
            correlation_id: None,
            imsi: None,
            esn: None,
            meid: None,
            con_ref: ConRef(0),
            quality_of_service_parameters: None,
            bsc_id: BscId(vec![0]),
            a8_traffic_id: A8TrafficId::gre_ppp(0, [0, 0, 0, 0]),
            service_option: ServiceOptionValue(0),
            a9_indicators: A9Indicators::default(),
            user_zone_id: None,
        };
        let mut con_ref = None;
        let mut bsc_id = None;
        let mut a8_traffic_id = None;
        let mut service_option = None;
        let mut a9_indicators = None;
        for element in message.elements {
            match element.id {
                ElementId::CallConnectionReference => set_optional_field(
                    &mut decoded.call_connection_reference,
                    CallConnectionReference::decode(&element.value)?,
                    element.id,
                )?,
                ElementId::CorrelationId => set_optional_field(
                    &mut decoded.correlation_id,
                    CorrelationId::decode(&element.value)?,
                    element.id,
                )?,
                ElementId::MobileIdentity => apply_mobile_identity(
                    &mut decoded.imsi,
                    &mut decoded.esn,
                    &mut decoded.meid,
                    &element.value,
                )?,
                ElementId::ConRef => {
                    set_optional_field(&mut con_ref, ConRef::decode(&element.value)?, element.id)?
                }
                ElementId::QualityOfServiceParameters => set_optional_field(
                    &mut decoded.quality_of_service_parameters,
                    QualityOfServiceParametersTyped::decode(&element.value)?,
                    element.id,
                )?,
                ElementId::BscId => {
                    set_optional_field(&mut bsc_id, BscId::decode(&element.value)?, element.id)?
                }
                ElementId::A8TrafficId => set_optional_field(
                    &mut a8_traffic_id,
                    A8TrafficId::decode(&element.value)?,
                    element.id,
                )?,
                ElementId::ServiceOption => set_optional_field(
                    &mut service_option,
                    ServiceOptionValue::decode(&element.value)?,
                    element.id,
                )?,
                ElementId::A9Indicators => set_optional_field(
                    &mut a9_indicators,
                    A9Indicators::decode(&element.value)?,
                    element.id,
                )?,
                ElementId::UserZoneId => set_optional_field(
                    &mut decoded.user_zone_id,
                    UserZoneId::decode(&element.value)?,
                    element.id,
                )?,
                _ => unexpected(MessageType::SetupA8, element.id)?,
            }
        }
        decoded.con_ref = con_ref.ok_or(Error::MissingRequiredInformationElement(
            ElementId::ConRef as u8,
        ))?;
        decoded.bsc_id = bsc_id.ok_or(Error::MissingRequiredInformationElement(
            ElementId::BscId as u8,
        ))?;
        decoded.a8_traffic_id = a8_traffic_id.ok_or(Error::MissingRequiredInformationElement(
            ElementId::A8TrafficId as u8,
        ))?;
        decoded.service_option = service_option.ok_or(Error::MissingRequiredInformationElement(
            ElementId::ServiceOption as u8,
        ))?;
        decoded.a9_indicators = a9_indicators.ok_or(Error::MissingRequiredInformationElement(
            ElementId::A9Indicators as u8,
        ))?;
        Ok(decoded)
    }
}

/// Exact typed `A9-Connect-A8` message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectA8Message {
    pub call_connection_reference: Option<CallConnectionReference>,
    pub correlation_id: Option<CorrelationId>,
    pub imsi: Option<String>,
    pub esn: Option<u32>,
    pub meid: Option<Meid>,
    pub con_ref: ConRef,
    pub a8_traffic_id: A8TrafficId,
    pub cause: CauseValue,
    pub pdsn_ip_address: Option<PdsnIpAddress>,
}

impl ConnectA8Message {
    /// Encodes the message.
    pub fn encode(&self) -> Result<Vec<u8>> {
        ensure_connect_cause(self.cause)?;
        let mut elements = Vec::new();
        push_optional(
            &mut elements,
            ElementId::CallConnectionReference,
            self.call_connection_reference.map(|v| v.encode().to_vec()),
        );
        push_optional(
            &mut elements,
            ElementId::CorrelationId,
            self.correlation_id.map(|v| v.encode().to_vec()),
        );
        push_mobile_identities(&mut elements, self.imsi.as_deref(), self.esn, self.meid)?;
        elements.push(InformationElement::new(
            ElementId::ConRef,
            self.con_ref.encode(),
        ));
        elements.push(InformationElement::new(
            ElementId::A8TrafficId,
            self.a8_traffic_id.encode(),
        ));
        elements.push(InformationElement::new(
            ElementId::Cause,
            self.cause.encode(),
        ));
        push_optional(
            &mut elements,
            ElementId::PdsnIpAddress,
            self.pdsn_ip_address.map(|v| v.encode().to_vec()),
        );
        encode(&Message::new(MessageType::ConnectA8, elements)?)
    }

    /// Decodes the message.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let message = decode_typed_message(input, MessageType::ConnectA8)?;
        let mut decoded = ConnectA8Message {
            call_connection_reference: None,
            correlation_id: None,
            imsi: None,
            esn: None,
            meid: None,
            con_ref: ConRef(0),
            a8_traffic_id: A8TrafficId::gre_ppp(0, [0, 0, 0, 0]),
            cause: CauseValue(0),
            pdsn_ip_address: None,
        };
        let mut con_ref = None;
        let mut a8_traffic_id = None;
        let mut cause = None;
        for element in message.elements {
            match element.id {
                ElementId::CallConnectionReference => set_optional_field(
                    &mut decoded.call_connection_reference,
                    CallConnectionReference::decode(&element.value)?,
                    element.id,
                )?,
                ElementId::CorrelationId => set_optional_field(
                    &mut decoded.correlation_id,
                    CorrelationId::decode(&element.value)?,
                    element.id,
                )?,
                ElementId::MobileIdentity => apply_mobile_identity(
                    &mut decoded.imsi,
                    &mut decoded.esn,
                    &mut decoded.meid,
                    &element.value,
                )?,
                ElementId::ConRef => {
                    set_optional_field(&mut con_ref, ConRef::decode(&element.value)?, element.id)?
                }
                ElementId::A8TrafficId => set_optional_field(
                    &mut a8_traffic_id,
                    A8TrafficId::decode(&element.value)?,
                    element.id,
                )?,
                ElementId::Cause => {
                    set_optional_field(&mut cause, CauseValue::decode(&element.value)?, element.id)?
                }
                ElementId::PdsnIpAddress => set_optional_field(
                    &mut decoded.pdsn_ip_address,
                    PdsnIpAddress::decode(&element.value)?,
                    element.id,
                )?,
                _ => unexpected(MessageType::ConnectA8, element.id)?,
            }
        }
        decoded.con_ref = con_ref.ok_or(Error::MissingRequiredInformationElement(
            ElementId::ConRef as u8,
        ))?;
        decoded.a8_traffic_id = a8_traffic_id.ok_or(Error::MissingRequiredInformationElement(
            ElementId::A8TrafficId as u8,
        ))?;
        decoded.cause = cause.ok_or(Error::MissingRequiredInformationElement(
            ElementId::Cause as u8,
        ))?;
        ensure_connect_cause(decoded.cause)?;
        Ok(decoded)
    }
}

/// Exact typed `A9-Disconnect-A8` message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DisconnectA8Message {
    pub call_connection_reference: Option<CallConnectionReference>,
    pub correlation_id: Option<CorrelationId>,
    pub imsi: Option<String>,
    pub esn: Option<u32>,
    pub meid: Option<Meid>,
    pub con_ref: ConRef,
    pub a8_traffic_id: A8TrafficId,
    pub cause: CauseValue,
}

impl DisconnectA8Message {
    /// Encodes the message.
    pub fn encode(&self) -> Result<Vec<u8>> {
        ensure_release_cause(self.cause, "DisconnectA8.cause")?;
        let mut elements = Vec::new();
        push_optional(
            &mut elements,
            ElementId::CallConnectionReference,
            self.call_connection_reference.map(|v| v.encode().to_vec()),
        );
        push_optional(
            &mut elements,
            ElementId::CorrelationId,
            self.correlation_id.map(|v| v.encode().to_vec()),
        );
        push_mobile_identities(&mut elements, self.imsi.as_deref(), self.esn, self.meid)?;
        elements.push(InformationElement::new(
            ElementId::ConRef,
            self.con_ref.encode(),
        ));
        elements.push(InformationElement::new(
            ElementId::A8TrafficId,
            self.a8_traffic_id.encode(),
        ));
        elements.push(InformationElement::new(
            ElementId::Cause,
            self.cause.encode(),
        ));
        encode(&Message::new(MessageType::DisconnectA8, elements)?)
    }

    /// Decodes the message.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let message = decode_typed_message(input, MessageType::DisconnectA8)?;
        let mut decoded = DisconnectA8Message {
            call_connection_reference: None,
            correlation_id: None,
            imsi: None,
            esn: None,
            meid: None,
            con_ref: ConRef(0),
            a8_traffic_id: A8TrafficId::gre_ppp(0, [0, 0, 0, 0]),
            cause: CauseValue(0),
        };
        let mut con_ref = None;
        let mut a8_traffic_id = None;
        let mut cause = None;
        for element in message.elements {
            match element.id {
                ElementId::CallConnectionReference => set_optional_field(
                    &mut decoded.call_connection_reference,
                    CallConnectionReference::decode(&element.value)?,
                    element.id,
                )?,
                ElementId::CorrelationId => set_optional_field(
                    &mut decoded.correlation_id,
                    CorrelationId::decode(&element.value)?,
                    element.id,
                )?,
                ElementId::MobileIdentity => apply_mobile_identity(
                    &mut decoded.imsi,
                    &mut decoded.esn,
                    &mut decoded.meid,
                    &element.value,
                )?,
                ElementId::ConRef => {
                    set_optional_field(&mut con_ref, ConRef::decode(&element.value)?, element.id)?
                }
                ElementId::A8TrafficId => set_optional_field(
                    &mut a8_traffic_id,
                    A8TrafficId::decode(&element.value)?,
                    element.id,
                )?,
                ElementId::Cause => {
                    set_optional_field(&mut cause, CauseValue::decode(&element.value)?, element.id)?
                }
                _ => unexpected(MessageType::DisconnectA8, element.id)?,
            }
        }
        decoded.con_ref = con_ref.ok_or(Error::MissingRequiredInformationElement(
            ElementId::ConRef as u8,
        ))?;
        decoded.a8_traffic_id = a8_traffic_id.ok_or(Error::MissingRequiredInformationElement(
            ElementId::A8TrafficId as u8,
        ))?;
        decoded.cause = cause.ok_or(Error::MissingRequiredInformationElement(
            ElementId::Cause as u8,
        ))?;
        ensure_release_cause(decoded.cause, "DisconnectA8.cause")?;
        Ok(decoded)
    }
}

/// Exact typed `A9-Release-A8` message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseA8Message {
    pub call_connection_reference: Option<CallConnectionReference>,
    pub correlation_id: Option<CorrelationId>,
    pub imsi: Option<String>,
    pub esn: Option<u32>,
    pub meid: Option<Meid>,
    pub con_ref: ConRef,
    pub a8_traffic_id: A8TrafficId,
    pub cause: CauseValue,
}

impl ReleaseA8Message {
    /// Encodes the message.
    pub fn encode(&self) -> Result<Vec<u8>> {
        ensure_release_cause(self.cause, "ReleaseA8.cause")?;
        let mut elements = Vec::new();
        push_optional(
            &mut elements,
            ElementId::CallConnectionReference,
            self.call_connection_reference.map(|v| v.encode().to_vec()),
        );
        push_optional(
            &mut elements,
            ElementId::CorrelationId,
            self.correlation_id.map(|v| v.encode().to_vec()),
        );
        push_mobile_identities(&mut elements, self.imsi.as_deref(), self.esn, self.meid)?;
        elements.push(InformationElement::new(
            ElementId::ConRef,
            self.con_ref.encode(),
        ));
        elements.push(InformationElement::new(
            ElementId::A8TrafficId,
            self.a8_traffic_id.encode(),
        ));
        elements.push(InformationElement::new(
            ElementId::Cause,
            self.cause.encode(),
        ));
        encode(&Message::new(MessageType::ReleaseA8, elements)?)
    }

    /// Decodes the message.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let message = decode_typed_message(input, MessageType::ReleaseA8)?;
        let mut decoded = ReleaseA8Message {
            call_connection_reference: None,
            correlation_id: None,
            imsi: None,
            esn: None,
            meid: None,
            con_ref: ConRef(0),
            a8_traffic_id: A8TrafficId::gre_ppp(0, [0, 0, 0, 0]),
            cause: CauseValue(0),
        };
        let mut con_ref = None;
        let mut a8_traffic_id = None;
        let mut cause = None;
        for element in message.elements {
            match element.id {
                ElementId::CallConnectionReference => set_optional_field(
                    &mut decoded.call_connection_reference,
                    CallConnectionReference::decode(&element.value)?,
                    element.id,
                )?,
                ElementId::CorrelationId => set_optional_field(
                    &mut decoded.correlation_id,
                    CorrelationId::decode(&element.value)?,
                    element.id,
                )?,
                ElementId::MobileIdentity => apply_mobile_identity(
                    &mut decoded.imsi,
                    &mut decoded.esn,
                    &mut decoded.meid,
                    &element.value,
                )?,
                ElementId::ConRef => {
                    set_optional_field(&mut con_ref, ConRef::decode(&element.value)?, element.id)?
                }
                ElementId::A8TrafficId => set_optional_field(
                    &mut a8_traffic_id,
                    A8TrafficId::decode(&element.value)?,
                    element.id,
                )?,
                ElementId::Cause => {
                    set_optional_field(&mut cause, CauseValue::decode(&element.value)?, element.id)?
                }
                _ => unexpected(MessageType::ReleaseA8, element.id)?,
            }
        }
        decoded.con_ref = con_ref.ok_or(Error::MissingRequiredInformationElement(
            ElementId::ConRef as u8,
        ))?;
        decoded.a8_traffic_id = a8_traffic_id.ok_or(Error::MissingRequiredInformationElement(
            ElementId::A8TrafficId as u8,
        ))?;
        decoded.cause = cause.ok_or(Error::MissingRequiredInformationElement(
            ElementId::Cause as u8,
        ))?;
        ensure_release_cause(decoded.cause, "ReleaseA8.cause")?;
        Ok(decoded)
    }
}

/// Exact typed `A9-Release-A8 Complete` message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReleaseA8CompleteMessage {
    pub call_connection_reference: Option<CallConnectionReference>,
    pub correlation_id: Option<CorrelationId>,
}

impl ReleaseA8CompleteMessage {
    /// Encodes the message.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut elements = Vec::new();
        push_optional(
            &mut elements,
            ElementId::CallConnectionReference,
            self.call_connection_reference.map(|v| v.encode().to_vec()),
        );
        push_optional(
            &mut elements,
            ElementId::CorrelationId,
            self.correlation_id.map(|v| v.encode().to_vec()),
        );
        encode(&Message::new(MessageType::ReleaseA8Complete, elements)?)
    }

    /// Decodes the message.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let message = decode_typed_message(input, MessageType::ReleaseA8Complete)?;
        let mut decoded = Self {
            call_connection_reference: None,
            correlation_id: None,
        };
        for element in message.elements {
            match element.id {
                ElementId::CallConnectionReference => set_optional_field(
                    &mut decoded.call_connection_reference,
                    CallConnectionReference::decode(&element.value)?,
                    element.id,
                )?,
                ElementId::CorrelationId => set_optional_field(
                    &mut decoded.correlation_id,
                    CorrelationId::decode(&element.value)?,
                    element.id,
                )?,
                _ => unexpected(MessageType::ReleaseA8Complete, element.id)?,
            }
        }
        Ok(decoded)
    }
}

/// Exact typed `A9-BS Service Request` message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BsServiceRequestMessage {
    pub correlation_id: Option<CorrelationId>,
    pub imsi: Option<String>,
    pub esn: Option<u32>,
    pub meid: Option<Meid>,
    pub service_option: ServiceOptionValue,
    pub data_count: DataCount,
}

impl BsServiceRequestMessage {
    /// Encodes the message.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut elements = Vec::new();
        push_optional(
            &mut elements,
            ElementId::CorrelationId,
            self.correlation_id.map(|v| v.encode().to_vec()),
        );
        push_mobile_identities(&mut elements, self.imsi.as_deref(), self.esn, self.meid)?;
        elements.push(InformationElement::new(
            ElementId::ServiceOption,
            self.service_option.encode(),
        ));
        elements.push(InformationElement::new(
            ElementId::DataCount,
            self.data_count.encode(),
        ));
        encode(&Message::new(MessageType::BsServiceRequest, elements)?)
    }

    /// Decodes the message.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let message = decode_typed_message(input, MessageType::BsServiceRequest)?;
        let mut decoded = Self {
            correlation_id: None,
            imsi: None,
            esn: None,
            meid: None,
            service_option: ServiceOptionValue(0),
            data_count: DataCount(0),
        };
        let mut service_option = None;
        let mut data_count = None;
        for element in message.elements {
            match element.id {
                ElementId::CorrelationId => set_optional_field(
                    &mut decoded.correlation_id,
                    CorrelationId::decode(&element.value)?,
                    element.id,
                )?,
                ElementId::MobileIdentity => apply_mobile_identity(
                    &mut decoded.imsi,
                    &mut decoded.esn,
                    &mut decoded.meid,
                    &element.value,
                )?,
                ElementId::ServiceOption => set_optional_field(
                    &mut service_option,
                    ServiceOptionValue::decode(&element.value)?,
                    element.id,
                )?,
                ElementId::DataCount => set_optional_field(
                    &mut data_count,
                    DataCount::decode(&element.value)?,
                    element.id,
                )?,
                _ => unexpected(MessageType::BsServiceRequest, element.id)?,
            }
        }
        decoded.service_option = service_option.ok_or(Error::MissingRequiredInformationElement(
            ElementId::ServiceOption as u8,
        ))?;
        decoded.data_count = data_count.ok_or(Error::MissingRequiredInformationElement(
            ElementId::DataCount as u8,
        ))?;
        Ok(decoded)
    }
}

/// Exact typed `A9-BS Service Response` message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BsServiceResponseMessage {
    pub correlation_id: Option<CorrelationId>,
    pub cause: Option<CauseValue>,
}

impl BsServiceResponseMessage {
    /// Encodes the message.
    pub fn encode(&self) -> Result<Vec<u8>> {
        if let Some(cause) = self.cause {
            ensure_bs_service_response_cause(cause)?;
        }
        let mut elements = Vec::new();
        push_optional(
            &mut elements,
            ElementId::CorrelationId,
            self.correlation_id.map(|v| v.encode().to_vec()),
        );
        push_optional(
            &mut elements,
            ElementId::Cause,
            self.cause.map(|v| v.encode().to_vec()),
        );
        encode(&Message::new(MessageType::BsServiceResponse, elements)?)
    }

    /// Decodes the message.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let message = decode_typed_message(input, MessageType::BsServiceResponse)?;
        let mut decoded = Self {
            correlation_id: None,
            cause: None,
        };
        for element in message.elements {
            match element.id {
                ElementId::CorrelationId => set_optional_field(
                    &mut decoded.correlation_id,
                    CorrelationId::decode(&element.value)?,
                    element.id,
                )?,
                ElementId::Cause => set_optional_field(
                    &mut decoded.cause,
                    CauseValue::decode(&element.value)?,
                    element.id,
                )?,
                _ => unexpected(MessageType::BsServiceResponse, element.id)?,
            }
        }
        if let Some(cause) = decoded.cause {
            ensure_bs_service_response_cause(cause)?;
        }
        Ok(decoded)
    }
}

/// Exact typed `A9-AL Connected` message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlConnectedMessage {
    pub call_connection_reference: Option<CallConnectionReference>,
    pub correlation_id: Option<CorrelationId>,
    pub a8_traffic_id: A8TrafficId,
    pub pdsn_ip_address: Option<PdsnIpAddress>,
}

impl AlConnectedMessage {
    /// Encodes the message.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut elements = Vec::new();
        push_optional(
            &mut elements,
            ElementId::CallConnectionReference,
            self.call_connection_reference.map(|v| v.encode().to_vec()),
        );
        push_optional(
            &mut elements,
            ElementId::CorrelationId,
            self.correlation_id.map(|v| v.encode().to_vec()),
        );
        elements.push(InformationElement::new(
            ElementId::A8TrafficId,
            self.a8_traffic_id.encode(),
        ));
        push_optional(
            &mut elements,
            ElementId::PdsnIpAddress,
            self.pdsn_ip_address.map(|v| v.encode().to_vec()),
        );
        encode(&Message::new(MessageType::AlConnected, elements)?)
    }

    /// Decodes the message.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let message = decode_typed_message(input, MessageType::AlConnected)?;
        let mut decoded = Self {
            call_connection_reference: None,
            correlation_id: None,
            a8_traffic_id: A8TrafficId::gre_ppp(0, [0, 0, 0, 0]),
            pdsn_ip_address: None,
        };
        let mut a8_traffic_id = None;
        for element in message.elements {
            match element.id {
                ElementId::CallConnectionReference => set_optional_field(
                    &mut decoded.call_connection_reference,
                    CallConnectionReference::decode(&element.value)?,
                    element.id,
                )?,
                ElementId::CorrelationId => set_optional_field(
                    &mut decoded.correlation_id,
                    CorrelationId::decode(&element.value)?,
                    element.id,
                )?,
                ElementId::A8TrafficId => set_optional_field(
                    &mut a8_traffic_id,
                    A8TrafficId::decode(&element.value)?,
                    element.id,
                )?,
                ElementId::PdsnIpAddress => set_optional_field(
                    &mut decoded.pdsn_ip_address,
                    PdsnIpAddress::decode(&element.value)?,
                    element.id,
                )?,
                _ => unexpected(MessageType::AlConnected, element.id)?,
            }
        }
        decoded.a8_traffic_id = a8_traffic_id.ok_or(Error::MissingRequiredInformationElement(
            ElementId::A8TrafficId as u8,
        ))?;
        Ok(decoded)
    }
}

/// Exact typed `A9-AL Connected Ack` message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlConnectedAckMessage {
    pub call_connection_reference: Option<CallConnectionReference>,
    pub correlation_id: Option<CorrelationId>,
}

impl AlConnectedAckMessage {
    /// Encodes the message.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut elements = Vec::new();
        push_optional(
            &mut elements,
            ElementId::CallConnectionReference,
            self.call_connection_reference.map(|v| v.encode().to_vec()),
        );
        push_optional(
            &mut elements,
            ElementId::CorrelationId,
            self.correlation_id.map(|v| v.encode().to_vec()),
        );
        encode(&Message::new(MessageType::AlConnectedAck, elements)?)
    }

    /// Decodes the message.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let message = decode_typed_message(input, MessageType::AlConnectedAck)?;
        let mut decoded = Self {
            call_connection_reference: None,
            correlation_id: None,
        };
        for element in message.elements {
            match element.id {
                ElementId::CallConnectionReference => set_optional_field(
                    &mut decoded.call_connection_reference,
                    CallConnectionReference::decode(&element.value)?,
                    element.id,
                )?,
                ElementId::CorrelationId => set_optional_field(
                    &mut decoded.correlation_id,
                    CorrelationId::decode(&element.value)?,
                    element.id,
                )?,
                _ => unexpected(MessageType::AlConnectedAck, element.id)?,
            }
        }
        Ok(decoded)
    }
}

/// Exact typed `A9-AL Disconnected` message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlDisconnectedMessage {
    pub call_connection_reference: Option<CallConnectionReference>,
    pub correlation_id: Option<CorrelationId>,
    pub a8_traffic_id: A8TrafficId,
}

impl AlDisconnectedMessage {
    /// Encodes the message.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut elements = Vec::new();
        push_optional(
            &mut elements,
            ElementId::CallConnectionReference,
            self.call_connection_reference.map(|v| v.encode().to_vec()),
        );
        push_optional(
            &mut elements,
            ElementId::CorrelationId,
            self.correlation_id.map(|v| v.encode().to_vec()),
        );
        elements.push(InformationElement::new(
            ElementId::A8TrafficId,
            self.a8_traffic_id.encode(),
        ));
        encode(&Message::new(MessageType::AlDisconnected, elements)?)
    }

    /// Decodes the message.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let message = decode_typed_message(input, MessageType::AlDisconnected)?;
        let mut decoded = Self {
            call_connection_reference: None,
            correlation_id: None,
            a8_traffic_id: A8TrafficId::gre_ppp(0, [0, 0, 0, 0]),
        };
        let mut a8_traffic_id = None;
        for element in message.elements {
            match element.id {
                ElementId::CallConnectionReference => set_optional_field(
                    &mut decoded.call_connection_reference,
                    CallConnectionReference::decode(&element.value)?,
                    element.id,
                )?,
                ElementId::CorrelationId => set_optional_field(
                    &mut decoded.correlation_id,
                    CorrelationId::decode(&element.value)?,
                    element.id,
                )?,
                ElementId::A8TrafficId => set_optional_field(
                    &mut a8_traffic_id,
                    A8TrafficId::decode(&element.value)?,
                    element.id,
                )?,
                _ => unexpected(MessageType::AlDisconnected, element.id)?,
            }
        }
        decoded.a8_traffic_id = a8_traffic_id.ok_or(Error::MissingRequiredInformationElement(
            ElementId::A8TrafficId as u8,
        ))?;
        Ok(decoded)
    }
}

/// Exact typed `A9-AL Disconnected Ack` message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AlDisconnectedAckMessage {
    pub call_connection_reference: Option<CallConnectionReference>,
    pub correlation_id: Option<CorrelationId>,
}

impl AlDisconnectedAckMessage {
    /// Encodes the message.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut elements = Vec::new();
        push_optional(
            &mut elements,
            ElementId::CallConnectionReference,
            self.call_connection_reference.map(|v| v.encode().to_vec()),
        );
        push_optional(
            &mut elements,
            ElementId::CorrelationId,
            self.correlation_id.map(|v| v.encode().to_vec()),
        );
        encode(&Message::new(MessageType::AlDisconnectedAck, elements)?)
    }

    /// Decodes the message.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let message = decode_typed_message(input, MessageType::AlDisconnectedAck)?;
        let mut decoded = Self {
            call_connection_reference: None,
            correlation_id: None,
        };
        for element in message.elements {
            match element.id {
                ElementId::CallConnectionReference => set_optional_field(
                    &mut decoded.call_connection_reference,
                    CallConnectionReference::decode(&element.value)?,
                    element.id,
                )?,
                ElementId::CorrelationId => set_optional_field(
                    &mut decoded.correlation_id,
                    CorrelationId::decode(&element.value)?,
                    element.id,
                )?,
                _ => unexpected(MessageType::AlDisconnectedAck, element.id)?,
            }
        }
        Ok(decoded)
    }
}

/// Exact typed `A9-Version Info` message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionInfoMessage {
    pub correlation_id: Option<CorrelationId>,
    pub cause: Option<CauseValue>,
    pub software_version: Option<SoftwareVersion>,
}

impl VersionInfoMessage {
    /// Encodes the message.
    pub fn encode(&self) -> Result<Vec<u8>> {
        if let Some(cause) = self.cause {
            ensure_version_info_cause(cause)?;
        }
        let mut elements = Vec::new();
        push_optional(
            &mut elements,
            ElementId::CorrelationId,
            self.correlation_id.map(|v| v.encode().to_vec()),
        );
        push_optional(
            &mut elements,
            ElementId::Cause,
            self.cause.map(|v| v.encode().to_vec()),
        );
        push_optional(
            &mut elements,
            ElementId::SoftwareVersion,
            self.software_version
                .as_ref()
                .map(|v| v.encode())
                .transpose()?,
        );
        encode(&Message::new(MessageType::VersionInfo, elements)?)
    }

    /// Decodes the message.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let message = decode_typed_message(input, MessageType::VersionInfo)?;
        let mut decoded = Self {
            correlation_id: None,
            cause: None,
            software_version: None,
        };
        for element in message.elements {
            match element.id {
                ElementId::CorrelationId => set_optional_field(
                    &mut decoded.correlation_id,
                    CorrelationId::decode(&element.value)?,
                    element.id,
                )?,
                ElementId::Cause => set_optional_field(
                    &mut decoded.cause,
                    CauseValue::decode(&element.value)?,
                    element.id,
                )?,
                ElementId::SoftwareVersion => set_optional_field(
                    &mut decoded.software_version,
                    SoftwareVersion::decode(&element.value)?,
                    element.id,
                )?,
                _ => unexpected(MessageType::VersionInfo, element.id)?,
            }
        }
        if let Some(cause) = decoded.cause {
            ensure_version_info_cause(cause)?;
        }
        Ok(decoded)
    }
}

/// Exact typed `A9-Version Info Ack` message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VersionInfoAckMessage {
    pub correlation_id: Option<CorrelationId>,
    pub software_version: Option<SoftwareVersion>,
}

impl VersionInfoAckMessage {
    /// Encodes the message.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut elements = Vec::new();
        push_optional(
            &mut elements,
            ElementId::CorrelationId,
            self.correlation_id.map(|v| v.encode().to_vec()),
        );
        push_optional(
            &mut elements,
            ElementId::SoftwareVersion,
            self.software_version
                .as_ref()
                .map(|v| v.encode())
                .transpose()?,
        );
        encode(&Message::new(MessageType::VersionInfoAck, elements)?)
    }

    /// Decodes the message.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let message = decode_typed_message(input, MessageType::VersionInfoAck)?;
        let mut decoded = Self {
            correlation_id: None,
            software_version: None,
        };
        for element in message.elements {
            match element.id {
                ElementId::CorrelationId => set_optional_field(
                    &mut decoded.correlation_id,
                    CorrelationId::decode(&element.value)?,
                    element.id,
                )?,
                ElementId::SoftwareVersion => set_optional_field(
                    &mut decoded.software_version,
                    SoftwareVersion::decode(&element.value)?,
                    element.id,
                )?,
                _ => unexpected(MessageType::VersionInfoAck, element.id)?,
            }
        }
        Ok(decoded)
    }
}

/// Exact typed `A9-Update-A8` message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateA8Message {
    pub call_connection_reference: Option<CallConnectionReference>,
    pub correlation_id: Option<CorrelationId>,
    pub imsi: Option<String>,
    pub esn: Option<u32>,
    pub meid: Option<Meid>,
    pub service_configuration_record: Option<Is2000ServiceConfigurationRecord>,
    pub service_option: Option<ServiceOptionValue>,
    pub user_zone_id: Option<UserZoneId>,
    pub quality_of_service_parameters: Option<QualityOfServiceParametersTyped>,
    pub cause: Option<CauseValue>,
    pub rn_pdit: Option<RnPdit>,
    pub sr_id: Option<SrId>,
    pub a9_indicators: Option<A9Indicators>,
    pub pdsn_ip_address: Option<PdsnIpAddress>,
    pub anchor_pdsn_address: Option<AnchorPdsnAddress>,
    pub anchor_pp_address: Option<AnchorPpAddress>,
}

impl UpdateA8Message {
    /// Encodes the message.
    pub fn encode(&self) -> Result<Vec<u8>> {
        if let Some(cause) = self.cause {
            ensure_update_a8_cause(cause)?;
        }
        let mut elements = Vec::new();
        push_optional(
            &mut elements,
            ElementId::CallConnectionReference,
            self.call_connection_reference.map(|v| v.encode().to_vec()),
        );
        push_optional(
            &mut elements,
            ElementId::CorrelationId,
            self.correlation_id.map(|v| v.encode().to_vec()),
        );
        push_mobile_identities(&mut elements, self.imsi.as_deref(), self.esn, self.meid)?;
        push_optional(
            &mut elements,
            ElementId::Is2000ServiceConfigurationRecord,
            self.service_configuration_record
                .as_ref()
                .map(|v| v.encode())
                .transpose()?,
        );
        push_optional(
            &mut elements,
            ElementId::ServiceOption,
            self.service_option.map(|v| v.encode().to_vec()),
        );
        push_optional(
            &mut elements,
            ElementId::UserZoneId,
            self.user_zone_id.map(|v| v.encode().to_vec()),
        );
        if let Some(value) = self.quality_of_service_parameters {
            elements.push(InformationElement::new(
                ElementId::QualityOfServiceParameters,
                value.encode()?.to_vec(),
            ));
        }
        push_optional(
            &mut elements,
            ElementId::Cause,
            self.cause.map(|v| v.encode().to_vec()),
        );
        push_optional(
            &mut elements,
            ElementId::RnPdit,
            self.rn_pdit
                .map(|v| v.encode())
                .transpose()?
                .map(|v| v.to_vec()),
        );
        push_optional(
            &mut elements,
            ElementId::SrId,
            self.sr_id
                .map(|v| v.encode())
                .transpose()?
                .map(|v| v.to_vec()),
        );
        push_optional(
            &mut elements,
            ElementId::A9Indicators,
            self.a9_indicators.map(|v| v.encode().to_vec()),
        );
        push_optional(
            &mut elements,
            ElementId::PdsnIpAddress,
            self.pdsn_ip_address.map(|v| v.encode().to_vec()),
        );
        push_optional(
            &mut elements,
            ElementId::AnchorPdsnAddress,
            self.anchor_pdsn_address.map(|v| v.encode().to_vec()),
        );
        push_optional(
            &mut elements,
            ElementId::AnchorPpAddress,
            self.anchor_pp_address.map(|v| v.encode().to_vec()),
        );
        encode(&Message::new(MessageType::UpdateA8, elements)?)
    }

    /// Decodes the message.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let message = decode_typed_message(input, MessageType::UpdateA8)?;
        let mut decoded = Self {
            call_connection_reference: None,
            correlation_id: None,
            imsi: None,
            esn: None,
            meid: None,
            service_configuration_record: None,
            service_option: None,
            user_zone_id: None,
            quality_of_service_parameters: None,
            cause: None,
            rn_pdit: None,
            sr_id: None,
            a9_indicators: None,
            pdsn_ip_address: None,
            anchor_pdsn_address: None,
            anchor_pp_address: None,
        };
        for element in message.elements {
            match element.id {
                ElementId::CallConnectionReference => set_optional_field(
                    &mut decoded.call_connection_reference,
                    CallConnectionReference::decode(&element.value)?,
                    element.id,
                )?,
                ElementId::CorrelationId => set_optional_field(
                    &mut decoded.correlation_id,
                    CorrelationId::decode(&element.value)?,
                    element.id,
                )?,
                ElementId::MobileIdentity => apply_mobile_identity(
                    &mut decoded.imsi,
                    &mut decoded.esn,
                    &mut decoded.meid,
                    &element.value,
                )?,
                ElementId::Is2000ServiceConfigurationRecord => set_optional_field(
                    &mut decoded.service_configuration_record,
                    Is2000ServiceConfigurationRecord::decode(&element.value)?,
                    element.id,
                )?,
                ElementId::ServiceOption => set_optional_field(
                    &mut decoded.service_option,
                    ServiceOptionValue::decode(&element.value)?,
                    element.id,
                )?,
                ElementId::UserZoneId => set_optional_field(
                    &mut decoded.user_zone_id,
                    UserZoneId::decode(&element.value)?,
                    element.id,
                )?,
                ElementId::QualityOfServiceParameters => set_optional_field(
                    &mut decoded.quality_of_service_parameters,
                    QualityOfServiceParametersTyped::decode(&element.value)?,
                    element.id,
                )?,
                ElementId::Cause => set_optional_field(
                    &mut decoded.cause,
                    CauseValue::decode(&element.value)?,
                    element.id,
                )?,
                ElementId::RnPdit => set_optional_field(
                    &mut decoded.rn_pdit,
                    RnPdit::decode(&element.value)?,
                    element.id,
                )?,
                ElementId::SrId => set_optional_field(
                    &mut decoded.sr_id,
                    SrId::decode(&element.value)?,
                    element.id,
                )?,
                ElementId::A9Indicators => set_optional_field(
                    &mut decoded.a9_indicators,
                    A9Indicators::decode(&element.value)?,
                    element.id,
                )?,
                ElementId::PdsnIpAddress => set_optional_field(
                    &mut decoded.pdsn_ip_address,
                    PdsnIpAddress::decode(&element.value)?,
                    element.id,
                )?,
                ElementId::AnchorPdsnAddress => set_optional_field(
                    &mut decoded.anchor_pdsn_address,
                    AnchorPdsnAddress::decode(&element.value)?,
                    element.id,
                )?,
                ElementId::AnchorPpAddress => set_optional_field(
                    &mut decoded.anchor_pp_address,
                    AnchorPpAddress::decode(&element.value)?,
                    element.id,
                )?,
                _ => unexpected(MessageType::UpdateA8, element.id)?,
            }
        }
        if let Some(cause) = decoded.cause {
            ensure_update_a8_cause(cause)?;
        }
        Ok(decoded)
    }
}

/// Exact typed `A9-Update-A8 Ack` message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateA8AckMessage {
    pub call_connection_reference: Option<CallConnectionReference>,
    pub correlation_id: Option<CorrelationId>,
    pub cause: Option<CauseValue>,
}

impl UpdateA8AckMessage {
    /// Encodes the message.
    pub fn encode(&self) -> Result<Vec<u8>> {
        if let Some(cause) = self.cause {
            ensure_update_a8_ack_cause(cause)?;
        }
        let mut elements = Vec::new();
        push_optional(
            &mut elements,
            ElementId::CallConnectionReference,
            self.call_connection_reference.map(|v| v.encode().to_vec()),
        );
        push_optional(
            &mut elements,
            ElementId::CorrelationId,
            self.correlation_id.map(|v| v.encode().to_vec()),
        );
        push_optional(
            &mut elements,
            ElementId::Cause,
            self.cause.map(|v| v.encode().to_vec()),
        );
        encode(&Message::new(MessageType::UpdateA8Ack, elements)?)
    }

    /// Decodes the message.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let message = decode_typed_message(input, MessageType::UpdateA8Ack)?;
        let mut decoded = Self {
            call_connection_reference: None,
            correlation_id: None,
            cause: None,
        };
        for element in message.elements {
            match element.id {
                ElementId::CallConnectionReference => set_optional_field(
                    &mut decoded.call_connection_reference,
                    CallConnectionReference::decode(&element.value)?,
                    element.id,
                )?,
                ElementId::CorrelationId => set_optional_field(
                    &mut decoded.correlation_id,
                    CorrelationId::decode(&element.value)?,
                    element.id,
                )?,
                ElementId::Cause => set_optional_field(
                    &mut decoded.cause,
                    CauseValue::decode(&element.value)?,
                    element.id,
                )?,
                _ => unexpected(MessageType::UpdateA8Ack, element.id)?,
            }
        }
        if let Some(cause) = decoded.cause {
            ensure_update_a8_ack_cause(cause)?;
        }
        Ok(decoded)
    }
}

/// Exact typed `A9-Short Data Delivery` message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShortDataDeliveryMessage {
    pub correlation_id: Option<CorrelationId>,
    pub imsi: Option<String>,
    pub esn: Option<u32>,
    pub meid: Option<Meid>,
    pub sr_id: Option<SrId>,
    pub data_count: Option<DataCount>,
    pub adds_user_part: AddsUserPart,
    pub a9_indicators: Option<A9Indicators>,
}

impl ShortDataDeliveryMessage {
    /// Encodes the message.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut elements = Vec::new();
        push_optional(
            &mut elements,
            ElementId::CorrelationId,
            self.correlation_id.map(|v| v.encode().to_vec()),
        );
        push_mobile_identities(&mut elements, self.imsi.as_deref(), self.esn, self.meid)?;
        push_optional(
            &mut elements,
            ElementId::SrId,
            self.sr_id
                .map(|v| v.encode())
                .transpose()?
                .map(|v| v.to_vec()),
        );
        push_optional(
            &mut elements,
            ElementId::DataCount,
            self.data_count.map(|v| v.encode().to_vec()),
        );
        elements.push(InformationElement::new(
            ElementId::AddsUserPart,
            self.adds_user_part.encode()?,
        ));
        push_optional(
            &mut elements,
            ElementId::A9Indicators,
            self.a9_indicators.map(|v| v.encode().to_vec()),
        );
        encode(&Message::new(MessageType::ShortDataDelivery, elements)?)
    }

    /// Decodes the message.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let message = decode_typed_message(input, MessageType::ShortDataDelivery)?;
        let mut decoded = Self {
            correlation_id: None,
            imsi: None,
            esn: None,
            meid: None,
            sr_id: None,
            data_count: None,
            adds_user_part: AddsUserPart::short_data_burst([0u8]),
            a9_indicators: None,
        };
        let mut adds_user_part = None;
        for element in message.elements {
            match element.id {
                ElementId::CorrelationId => set_optional_field(
                    &mut decoded.correlation_id,
                    CorrelationId::decode(&element.value)?,
                    element.id,
                )?,
                ElementId::MobileIdentity => apply_mobile_identity(
                    &mut decoded.imsi,
                    &mut decoded.esn,
                    &mut decoded.meid,
                    &element.value,
                )?,
                ElementId::SrId => set_optional_field(
                    &mut decoded.sr_id,
                    SrId::decode(&element.value)?,
                    element.id,
                )?,
                ElementId::DataCount => set_optional_field(
                    &mut decoded.data_count,
                    DataCount::decode(&element.value)?,
                    element.id,
                )?,
                ElementId::AddsUserPart => set_optional_field(
                    &mut adds_user_part,
                    AddsUserPart::decode(&element.value)?,
                    element.id,
                )?,
                ElementId::A9Indicators => set_optional_field(
                    &mut decoded.a9_indicators,
                    A9Indicators::decode(&element.value)?,
                    element.id,
                )?,
                _ => unexpected(MessageType::ShortDataDelivery, element.id)?,
            }
        }
        decoded.adds_user_part = adds_user_part.ok_or(Error::MissingRequiredInformationElement(
            ElementId::AddsUserPart as u8,
        ))?;
        Ok(decoded)
    }
}

/// Exact typed `A9-Short Data Ack` message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShortDataAckMessage {
    pub correlation_id: Option<CorrelationId>,
    pub imsi: Option<String>,
    pub esn: Option<u32>,
    pub meid: Option<Meid>,
    pub cause: CauseValue,
}

impl ShortDataAckMessage {
    /// Encodes the message.
    pub fn encode(&self) -> Result<Vec<u8>> {
        ensure_short_data_ack_cause(self.cause)?;
        let mut elements = Vec::new();
        push_optional(
            &mut elements,
            ElementId::CorrelationId,
            self.correlation_id.map(|v| v.encode().to_vec()),
        );
        push_mobile_identities(&mut elements, self.imsi.as_deref(), self.esn, self.meid)?;
        elements.push(InformationElement::new(
            ElementId::Cause,
            self.cause.encode(),
        ));
        encode(&Message::new(MessageType::ShortDataAck, elements)?)
    }

    /// Decodes the message.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let message = decode_typed_message(input, MessageType::ShortDataAck)?;
        let mut decoded = Self {
            correlation_id: None,
            imsi: None,
            esn: None,
            meid: None,
            cause: CauseValue(0),
        };
        let mut cause = None;
        for element in message.elements {
            match element.id {
                ElementId::CorrelationId => set_optional_field(
                    &mut decoded.correlation_id,
                    CorrelationId::decode(&element.value)?,
                    element.id,
                )?,
                ElementId::MobileIdentity => apply_mobile_identity(
                    &mut decoded.imsi,
                    &mut decoded.esn,
                    &mut decoded.meid,
                    &element.value,
                )?,
                ElementId::Cause => {
                    set_optional_field(&mut cause, CauseValue::decode(&element.value)?, element.id)?
                }
                _ => unexpected(MessageType::ShortDataAck, element.id)?,
            }
        }
        decoded.cause = cause.ok_or(Error::MissingRequiredInformationElement(
            ElementId::Cause as u8,
        ))?;
        ensure_short_data_ack_cause(decoded.cause)?;
        Ok(decoded)
    }
}

fn decode_typed_message(input: &[u8], expected: MessageType) -> Result<Message> {
    let message = decode(input)?;
    if message.message_type != expected {
        return Err(Error::UnknownMessageType(message.message_type as u8));
    }
    Ok(message)
}

fn push_optional(elements: &mut Vec<InformationElement>, id: ElementId, value: Option<Vec<u8>>) {
    if let Some(value) = value {
        elements.push(InformationElement::new(id, value));
    }
}

fn push_mobile_identities(
    elements: &mut Vec<InformationElement>,
    imsi: Option<&str>,
    esn: Option<u32>,
    meid: Option<Meid>,
) -> Result<()> {
    ensure_mobile_identity_fields(imsi, esn, meid)?;
    if let Some(imsi) = imsi {
        elements.push(InformationElement::new(
            ElementId::MobileIdentity,
            MobileIdentity::Imsi(imsi.to_owned()).encode()?,
        ));
    }
    if let Some(esn) = esn {
        elements.push(InformationElement::new(
            ElementId::MobileIdentity,
            MobileIdentity::Esn(esn).encode()?,
        ));
    }
    if let Some(meid) = meid {
        elements.push(InformationElement::new(
            ElementId::MobileIdentity,
            MobileIdentity::Meid(meid).encode()?,
        ));
    }
    Ok(())
}

fn set_optional_field<T>(slot: &mut Option<T>, value: T, element_id: ElementId) -> Result<()> {
    if slot.is_some() {
        return Err(Error::DuplicateInformationElement(element_id as u8));
    }
    *slot = Some(value);
    Ok(())
}

fn apply_mobile_identity(
    imsi: &mut Option<String>,
    esn: &mut Option<u32>,
    meid: &mut Option<Meid>,
    value: &[u8],
) -> Result<()> {
    match MobileIdentity::decode(value)? {
        MobileIdentity::Imsi(decoded) => {
            if imsi.replace(decoded).is_some() {
                return Err(Error::DuplicateInformationElement(
                    ElementId::MobileIdentity as u8,
                ));
            }
        }
        MobileIdentity::Esn(decoded) => {
            if imsi.is_none() {
                return Err(Error::InvalidValue {
                    context: "MobileIdentity.sequence",
                    value: decoded,
                });
            }
            if esn.replace(decoded).is_some() {
                return Err(Error::DuplicateInformationElement(
                    ElementId::MobileIdentity as u8,
                ));
            }
        }
        MobileIdentity::Meid(decoded) => {
            if imsi.is_none() {
                return Err(Error::InvalidValue {
                    context: "MobileIdentity.sequence",
                    value: 0x01,
                });
            }
            if meid.replace(decoded).is_some() {
                return Err(Error::DuplicateInformationElement(
                    ElementId::MobileIdentity as u8,
                ));
            }
        }
    }
    Ok(())
}

fn ensure_mobile_identity_fields(
    imsi: Option<&str>,
    esn: Option<u32>,
    meid: Option<Meid>,
) -> Result<()> {
    if (esn.is_some() || meid.is_some()) && imsi.is_none() {
        return Err(Error::InvalidValue {
            context: "MobileIdentity.sequence",
            value: if esn.is_some() { 0x05 } else { 0x01 },
        });
    }
    Ok(())
}

fn unexpected<T>(message_type: MessageType, element_id: ElementId) -> Result<T> {
    Err(Error::UnexpectedInformationElement {
        message_type,
        element_id: element_id as u8,
    })
}

fn ensure_connect_cause(cause: CauseValue) -> Result<()> {
    match cause.0 {
        0x13 | 0x20 | 0x32 | 0x79 | 0x7a => Ok(()),
        _ => Err(Error::InvalidValue {
            context: "ConnectA8.cause",
            value: cause.0 as u32,
        }),
    }
}

fn ensure_release_cause(cause: CauseValue, context: &'static str) -> Result<()> {
    match cause.0 {
        0x10 | 0x14 | 0x20 => Ok(()),
        _ => Err(Error::InvalidValue {
            context,
            value: cause.0 as u32,
        }),
    }
}

fn ensure_bs_service_response_cause(cause: CauseValue) -> Result<()> {
    match cause.0 {
        0x08 | 0x11 => Ok(()),
        _ => Err(Error::InvalidValue {
            context: "BsServiceResponse.cause",
            value: cause.0 as u32,
        }),
    }
}

fn ensure_version_info_cause(cause: CauseValue) -> Result<()> {
    match cause.0 {
        0x07 | 0x20 => Ok(()),
        _ => Err(Error::InvalidValue {
            context: "VersionInfo.cause",
            value: cause.0 as u32,
        }),
    }
}

fn ensure_update_a8_cause(cause: CauseValue) -> Result<()> {
    match cause.0 {
        0x17 | 0x1b | 0x1c | 0x1e => Ok(()),
        _ => Err(Error::InvalidValue {
            context: "UpdateA8.cause",
            value: cause.0 as u32,
        }),
    }
}

fn ensure_update_a8_ack_cause(cause: CauseValue) -> Result<()> {
    match cause.0 {
        0x13 | 0x36 => Ok(()),
        _ => Err(Error::InvalidValue {
            context: "UpdateA8Ack.cause",
            value: cause.0 as u32,
        }),
    }
}

fn ensure_short_data_ack_cause(cause: CauseValue) -> Result<()> {
    match cause.0 {
        0x13 | 0x16 | 0x17 | 0x18 => Ok(()),
        _ => Err(Error::InvalidValue {
            context: "ShortDataAck.cause",
            value: cause.0 as u32,
        }),
    }
}

fn encode_imsi(imsi: &str) -> Result<Vec<u8>> {
    let digits: Vec<u8> = imsi
        .chars()
        .map(|c| c.to_digit(10).map(|d| d as u8))
        .collect::<Option<Vec<_>>>()
        .ok_or(Error::UnknownInformationElement(0xff))?;
    if !(10..=15).contains(&digits.len()) {
        return Err(Error::InvalidLength {
            expected: 10,
            actual: digits.len(),
        });
    }
    let odd = digits.len() % 2 == 1;
    let mut out = Vec::with_capacity(1 + digits.len().div_ceil(2));
    out.push((digits[0] << 4) | ((odd as u8) << 3) | 0b110);
    let mut index = 1;
    while index < digits.len() {
        let low = digits[index];
        let high = if index + 1 < digits.len() {
            digits[index + 1]
        } else {
            0x0f
        };
        out.push((high << 4) | low);
        index += 2;
    }
    Ok(out)
}

fn decode_imsi(first: u8, rest: &[u8]) -> Result<MobileIdentity> {
    let odd = first & 0x08 != 0;
    let mut digits = String::new();
    let first_digit = (first >> 4) & 0x0f;
    if first_digit > 9 {
        return Err(Error::InvalidValue {
            context: "MobileIdentity.imsi.first_digit",
            value: first_digit as u32,
        });
    }
    digits.push(char::from(b'0' + first_digit));
    for (index, byte) in rest.iter().copied().enumerate() {
        let low = byte & 0x0f;
        let high = byte >> 4;
        if low > 9 {
            return Err(Error::InvalidValue {
                context: "MobileIdentity.imsi.low_digit",
                value: low as u32,
            });
        }
        digits.push(char::from(b'0' + low));
        let is_last_high_nibble = index == rest.len() - 1;
        if high == 0x0f {
            if is_last_high_nibble && !odd {
                continue;
            }
            return Err(Error::InvalidValue {
                context: "MobileIdentity.imsi.filler",
                value: high as u32,
            });
        }
        if high > 9 {
            return Err(Error::InvalidValue {
                context: "MobileIdentity.imsi.high_digit",
                value: high as u32,
            });
        }
        digits.push(char::from(b'0' + high));
    }
    Ok(MobileIdentity::Imsi(digits))
}
