//! Exact typed models for Abis control messages and inherited A3 structures.

use super::ies::ElementId;
use super::messages::MessageType;
use crate::{Error, Result};

/// The fixed-width call connection reference used on Abis and inherited A3 messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct CallConnectionReference {
    pub market_id: u16,
    pub generating_entity_id: u16,
    pub call_connection_reference: u32,
}

impl CallConnectionReference {
    /// Encodes the call connection reference payload.
    pub fn encode(self) -> [u8; 8] {
        let mut out = [0u8; 8];
        out[..2].copy_from_slice(&self.market_id.to_be_bytes());
        out[2..4].copy_from_slice(&self.generating_entity_id.to_be_bytes());
        out[4..8].copy_from_slice(&self.call_connection_reference.to_be_bytes());
        out
    }

    /// Decodes the call connection reference payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 8 {
            return Err(Error::InvalidLength {
                context: "Call Connection Reference",
                expected: 8,
                actual: input.len(),
            });
        }
        Ok(Self {
            market_id: u16::from_be_bytes([input[0], input[1]]),
            generating_entity_id: u16::from_be_bytes([input[2], input[3]]),
            call_connection_reference: u32::from_be_bytes([input[4], input[5], input[6], input[7]]),
        })
    }
}

/// A 32-bit correlation identifier.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CorrelationId(pub u32);

impl CorrelationId {
    /// Encodes the correlation identifier payload.
    pub fn encode(self) -> [u8; 4] {
        self.0.to_be_bytes()
    }

    /// Decodes the correlation identifier payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 4 {
            return Err(Error::InvalidLength {
                context: "Correlation ID",
                expected: 4,
                actual: input.len(),
            });
        }
        Ok(Self(u32::from_be_bytes([
            input[0], input[1], input[2], input[3],
        ])))
    }
}

/// Variable-length SDU identifier used by inherited A3 procedures.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SduId(pub Vec<u8>);

impl SduId {
    /// Creates a validated SDU identifier.
    pub fn new(value: impl Into<Vec<u8>>) -> Result<Self> {
        let value = value.into();
        if !(1..=6).contains(&value.len()) {
            return Err(Error::InvalidLength {
                context: "SDU ID",
                expected: 6,
                actual: value.len(),
            });
        }
        Ok(Self(value))
    }

    /// Returns the encoded payload bytes.
    pub fn encode(&self) -> &[u8] {
        &self.0
    }

    /// Decodes a validated SDU identifier.
    pub fn decode(input: &[u8]) -> Result<Self> {
        Self::new(input.to_vec())
    }
}

/// Exact Abis mobile identity payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MobileIdentity {
    Imsi(String),
    Esn(u32),
}

impl MobileIdentity {
    /// Encodes the mobile identity payload.
    pub fn encode(&self) -> Result<Vec<u8>> {
        match self {
            MobileIdentity::Imsi(imsi) => encode_imsi(imsi),
            MobileIdentity::Esn(esn) => {
                let mut out = vec![0x05];
                out.extend_from_slice(&esn.to_be_bytes());
                Ok(out)
            }
        }
    }

    /// Decodes the mobile identity payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let Some((&first, rest)) = input.split_first() else {
            return Err(Error::Truncated {
                context: "Mobile Identity",
                needed: 1,
                actual: 0,
            });
        };
        match first & 0x07 {
            0x06 => decode_imsi(first, rest),
            0x05 => {
                if input.len() != 5 {
                    return Err(Error::InvalidLength {
                        context: "ESN Mobile Identity",
                        expected: 5,
                        actual: input.len(),
                    });
                }
                Ok(MobileIdentity::Esn(u32::from_be_bytes([
                    input[1], input[2], input[3], input[4],
                ])))
            }
            other => Err(Error::ReservedValue {
                context: "Mobile Identity type",
                value: other,
            }),
        }
    }
}

/// A cell identifier using discriminator `0x02` and a 12-bit cell plus 4-bit sector.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellId {
    pub cell: u16,
    pub sector: u8,
}

impl CellId {
    /// Encodes the cell identifier payload, including the discriminator octet.
    pub fn encode(self) -> Result<[u8; 3]> {
        if !(1..=0x0fff).contains(&self.cell) || self.sector > 0x0f {
            return Err(Error::InvalidValue {
                context: "Cell Identifier",
                reason: "cell must be 1..=0x0fff and sector must be 0..=0x0f",
            });
        }
        Ok([
            0x02,
            (self.cell >> 4) as u8,
            (((self.cell & 0x000f) as u8) << 4) | (self.sector & 0x0f),
        ])
    }

    /// Decodes a discriminator-`0x02` cell identifier payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 3 {
            return Err(Error::InvalidLength {
                context: "Cell Identifier",
                expected: 3,
                actual: input.len(),
            });
        }
        if input[0] != 0x02 {
            return Err(Error::ReservedValue {
                context: "Cell Identifier discriminator",
                value: input[0],
            });
        }
        let cell = ((input[1] as u16) << 4) | ((input[2] as u16) >> 4);
        let sector = input[2] & 0x0f;
        if cell == 0 {
            return Err(Error::InvalidValue {
                context: "Cell Identifier",
                reason: "cell identifier must be in the range 0x001..=0x0fff",
            });
        }
        Ok(Self { cell, sector })
    }
}

/// A cell identifier using discriminator `0x07` and a 24-bit MSCID.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellIdWithMscId {
    pub mscid: u32,
    pub cell: u16,
    pub sector: u8,
}

impl CellIdWithMscId {
    /// Encodes the cell identifier payload, including discriminator and MSCID.
    pub fn encode(self) -> Result<[u8; 6]> {
        if self.mscid > 0x00ff_ffff || !(1..=0x0fff).contains(&self.cell) || self.sector > 0x0f {
            return Err(Error::InvalidValue {
                context: "MSC Cell Identifier",
                reason: "mscid/cell/sector out of range",
            });
        }
        Ok([
            0x07,
            ((self.mscid >> 16) & 0xff) as u8,
            ((self.mscid >> 8) & 0xff) as u8,
            (self.mscid & 0xff) as u8,
            (self.cell >> 4) as u8,
            (((self.cell & 0x000f) as u8) << 4) | (self.sector & 0x0f),
        ])
    }

    /// Decodes a discriminator-`0x07` cell identifier payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 6 {
            return Err(Error::InvalidLength {
                context: "MSC Cell Identifier",
                expected: 6,
                actual: input.len(),
            });
        }
        if input[0] != 0x07 {
            return Err(Error::ReservedValue {
                context: "MSC Cell Identifier discriminator",
                value: input[0],
            });
        }
        Ok(Self {
            mscid: ((input[1] as u32) << 16) | ((input[2] as u32) << 8) | (input[3] as u32),
            cell: ((input[4] as u16) << 4) | ((input[5] as u16) >> 4),
            sector: input[5] & 0x0f,
        })
        .and_then(|value| {
            if value.cell == 0 {
                return Err(Error::InvalidValue {
                    context: "MSC Cell Identifier",
                    reason: "cell identifier must be in the range 0x001..=0x0fff",
                });
            }
            Ok(value)
        })
    }
}

/// The nested traffic-circuit identifier used inside A3 connect/remove procedures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrafficCircuitId {
    pub traffic_circuit_identifier: u16,
    pub traffic_connection_identifier: u8,
}

impl TrafficCircuitId {
    /// Encodes the nested traffic-circuit identifier structure.
    pub fn encode(self) -> [u8; 6] {
        [
            0x05,
            0x02,
            (self.traffic_circuit_identifier >> 8) as u8,
            self.traffic_circuit_identifier as u8,
            0x01,
            self.traffic_connection_identifier,
        ]
    }

    /// Decodes the nested traffic-circuit identifier structure.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 6 {
            return Err(Error::InvalidLength {
                context: "Traffic Circuit ID",
                expected: 6,
                actual: input.len(),
            });
        }
        if input[0] != 0x05 || input[1] != 0x02 || input[4] != 0x01 {
            return Err(Error::InvalidValue {
                context: "Traffic Circuit ID",
                reason: "unexpected nested length markers",
            });
        }
        Ok(Self {
            traffic_circuit_identifier: u16::from_be_bytes([input[2], input[3]]),
            traffic_connection_identifier: input[5],
        })
    }
}

/// Band classes allowed by the Abis setup and burst procedures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum BandClass {
    Pcs = 0x01,
    Tacs = 0x02,
    Jtacs = 0x03,
    KoreanPcs = 0x04,
    Nmt450 = 0x05,
    Imt2000 = 0x06,
}

impl BandClass {
    /// Encodes the band class payload.
    pub const fn encode(self) -> [u8; 1] {
        [self as u8]
    }

    /// Decodes the band class payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 1 {
            return Err(Error::InvalidLength {
                context: "Band Class",
                expected: 1,
                actual: input.len(),
            });
        }
        if input[0] & 0xe0 != 0 {
            return Err(Error::InvalidValue {
                context: "Band Class",
                reason: "reserved bits must be zero",
            });
        }
        let value = input[0] & 0x1f;
        match value {
            0x01 => Ok(Self::Pcs),
            0x02 => Ok(Self::Tacs),
            0x03 => Ok(Self::Jtacs),
            0x04 => Ok(Self::KoreanPcs),
            0x05 => Ok(Self::Nmt450),
            0x06 => Ok(Self::Imt2000),
            other => Err(Error::ReservedValue {
                context: "Band Class",
                value: other,
            }),
        }
    }
}

/// Physical channel identifiers used inside Abis channel-assignment structures.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PhysicalChannelType {
    Is95 = 0x00,
    Fch = 0x01,
    Sch = 0x02,
    Dcch = 0x03,
}

impl PhysicalChannelType {
    /// Parses the 2-bit/4-bit channel-type code carried on the wire.
    pub fn decode(value: u8) -> Result<Self> {
        match value {
            0x00 => Ok(Self::Is95),
            0x01 => Ok(Self::Fch),
            0x02 => Ok(Self::Sch),
            0x03 => Ok(Self::Dcch),
            other => Err(Error::ReservedValue {
                context: "Physical Channel Type",
                value: other,
            }),
        }
    }
}

/// Pilot gating rates carried in `Physical Channel Info`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PilotGatingRate {
    Full = 0b00,
    Half = 0b01,
    Quarter = 0b10,
}

impl PilotGatingRate {
    /// Parses the pilot gating rate code.
    pub fn decode(value: u8) -> Result<Self> {
        match value {
            0b00 => Ok(Self::Full),
            0b01 => Ok(Self::Half),
            0b10 => Ok(Self::Quarter),
            other => Err(Error::ReservedValue {
                context: "Pilot Gating Rate",
                value: other,
            }),
        }
    }
}

/// Exact typed `Physical Channel Info` payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PhysicalChannelInfo {
    pub frame_offset: u8,
    pub pilot_gating_rate: PilotGatingRate,
    pub arfcn: u16,
    pub otd: bool,
    pub physical_channels: Vec<PhysicalChannelType>,
}

impl PhysicalChannelInfo {
    /// Encodes the `Physical Channel Info` payload.
    pub fn encode(&self) -> Result<[u8; 5]> {
        if !(1..=2).contains(&self.physical_channels.len()) {
            return Err(Error::InvalidValue {
                context: "Physical Channel Info",
                reason: "must include one or two physical channels",
            });
        }
        if self.arfcn > 0x07ff {
            return Err(Error::InvalidValue {
                context: "Physical Channel Info",
                reason: "ARFCN must fit in 11 bits",
            });
        }
        let channel_1 = self.physical_channels[0] as u8;
        let channel_2 = self
            .physical_channels
            .get(1)
            .copied()
            .unwrap_or(PhysicalChannelType::Is95) as u8;
        Ok([
            self.frame_offset,
            0x20 | ((self.pilot_gating_rate as u8) << 3) | ((self.arfcn >> 8) as u8 & 0x07),
            self.arfcn as u8,
            self.physical_channels.len() as u8,
            ((self.otd as u8) << 3) | ((channel_2 & 0x03) << 2) | (channel_1 & 0x03),
        ])
    }

    /// Decodes the `Physical Channel Info` payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 5 {
            return Err(Error::InvalidLength {
                context: "Physical Channel Info",
                expected: 5,
                actual: input.len(),
            });
        }
        let protocol_stack = (input[1] >> 5) & 0x07;
        if protocol_stack != 0b001 {
            return Err(Error::ReservedValue {
                context: "A3 Traffic Channel Protocol Stack",
                value: protocol_stack,
            });
        }
        let pilot_gating_rate = PilotGatingRate::decode((input[1] >> 3) & 0x03)?;
        let arfcn = (((input[1] & 0x07) as u16) << 8) | input[2] as u16;
        if input[3] & 0xf0 != 0 || input[4] & 0xf0 != 0 {
            return Err(Error::InvalidValue {
                context: "Physical Channel Info",
                reason: "reserved bits must be zero",
            });
        }
        let count = (input[3] & 0x0f) as usize;
        if !(1..=2).contains(&count) {
            return Err(Error::InvalidValue {
                context: "Physical Channel Info",
                reason: "count of physical channels must be 1 or 2",
            });
        }
        let mut physical_channels = vec![PhysicalChannelType::decode(input[4] & 0x03)?];
        let channel_2 = (input[4] >> 2) & 0x03;
        if count == 2 {
            physical_channels.push(PhysicalChannelType::decode(channel_2)?);
        } else if channel_2 != 0 {
            return Err(Error::InvalidValue {
                context: "Physical Channel Info",
                reason: "second physical channel must be zero when count is one",
            });
        }
        Ok(Self {
            frame_offset: input[0],
            pilot_gating_rate,
            arfcn,
            otd: input[4] & 0x08 != 0,
            physical_channels,
        })
    }
}

/// Packet-data priority carried by `Quality of Service Parameters`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct QualityOfServiceParameters {
    pub packet_priority: u8,
}

impl QualityOfServiceParameters {
    /// Encodes the packet-priority payload.
    pub fn encode(self) -> Result<[u8; 1]> {
        if self.packet_priority > 0x0d {
            return Err(Error::ReservedValue {
                context: "Packet Priority",
                value: self.packet_priority,
            });
        }
        Ok([self.packet_priority & 0x0f])
    }

    /// Decodes the packet-priority payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 1 {
            return Err(Error::InvalidLength {
                context: "Quality of Service Parameters",
                expected: 1,
                actual: input.len(),
            });
        }
        if input[0] & 0xf0 != 0 {
            return Err(Error::InvalidValue {
                context: "Quality of Service Parameters",
                reason: "reserved bits must be zero",
            });
        }
        let packet_priority = input[0] & 0x0f;
        if packet_priority > 0x0d {
            return Err(Error::ReservedValue {
                context: "Packet Priority",
                value: packet_priority,
            });
        }
        Ok(Self { packet_priority })
    }
}

/// One privacy-mask entry carried by the `Privacy Info` IE.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivacyMaskInformation {
    pub privacy_mask_type: u8,
    pub status: bool,
    pub available: bool,
    pub privacy_mask: Vec<u8>,
}

impl PrivacyMaskInformation {
    /// Encodes the privacy-mask record.
    pub fn encode(&self, out: &mut Vec<u8>) -> Result<()> {
        if !(1..=2).contains(&self.privacy_mask_type) {
            return Err(Error::ReservedValue {
                context: "Privacy Mask Type",
                value: self.privacy_mask_type,
            });
        }
        if self.privacy_mask.is_empty() || self.privacy_mask.len() > u8::MAX as usize {
            return Err(Error::InvalidLength {
                context: "Privacy Mask",
                expected: u8::MAX as usize,
                actual: self.privacy_mask.len(),
            });
        }
        out.push(
            ((self.privacy_mask_type & 0x1f) << 2)
                | ((self.status as u8) << 1)
                | (self.available as u8),
        );
        out.push(self.privacy_mask.len() as u8);
        out.extend_from_slice(&self.privacy_mask);
        Ok(())
    }

    /// Decodes a single privacy-mask record and returns bytes consumed.
    pub fn decode(input: &[u8]) -> Result<(Self, usize)> {
        if input.len() < 2 {
            return Err(Error::Truncated {
                context: "Privacy Mask Information",
                needed: 2,
                actual: input.len(),
            });
        }
        let mask_length = input[1] as usize;
        if input.len() < 2 + mask_length {
            return Err(Error::Truncated {
                context: "Privacy Mask Information",
                needed: 2 + mask_length,
                actual: input.len(),
            });
        }
        Ok((
            Self {
                privacy_mask_type: (input[0] >> 2) & 0x1f,
                status: input[0] & 0x02 != 0,
                available: input[0] & 0x01 != 0,
                privacy_mask: input[2..2 + mask_length].to_vec(),
            },
            2 + mask_length,
        ))
    }
}

/// Exact typed `Privacy Info` payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PrivacyInfo {
    pub privacy_masks: Vec<PrivacyMaskInformation>,
}

impl PrivacyInfo {
    /// Encodes the `Privacy Info` payload.
    pub fn encode(&self) -> Result<Vec<u8>> {
        if self.privacy_masks.is_empty() {
            return Err(Error::InvalidValue {
                context: "Privacy Info",
                reason: "must contain at least one privacy mask record",
            });
        }
        let mut out = Vec::new();
        for privacy_mask in &self.privacy_masks {
            privacy_mask.encode(&mut out)?;
        }
        Ok(out)
    }

    /// Decodes the `Privacy Info` payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let mut offset = 0;
        let mut privacy_masks = Vec::new();
        while offset < input.len() {
            let (record, consumed) = PrivacyMaskInformation::decode(&input[offset..])?;
            privacy_masks.push(record);
            offset += consumed;
        }
        if privacy_masks.is_empty() {
            return Err(Error::InvalidValue {
                context: "Privacy Info",
                reason: "must contain at least one privacy mask record",
            });
        }
        Ok(Self { privacy_masks })
    }
}

/// A single cell record inside `Downlink Radio Environment`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DownlinkRadioEnvironmentRecord {
    pub cell: CellId,
    pub downlink_signal_strength_raw: u8,
    pub cdma_target_one_way_delay: u16,
}

/// Exact typed `Downlink Radio Environment` payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownlinkRadioEnvironment {
    pub records: Vec<DownlinkRadioEnvironmentRecord>,
}

impl DownlinkRadioEnvironment {
    /// Encodes the `Downlink Radio Environment` payload.
    pub fn encode(&self) -> Result<Vec<u8>> {
        if self.records.is_empty() || self.records.len() > u8::MAX as usize {
            return Err(Error::InvalidValue {
                context: "Downlink Radio Environment",
                reason: "must contain at least one cell record",
            });
        }
        let mut out = Vec::with_capacity(2 + self.records.len() * 5);
        out.push(self.records.len() as u8);
        out.push(0x02);
        for record in &self.records {
            let cell = record.cell.encode()?;
            out.extend_from_slice(&cell[1..]);
            out.push(record.downlink_signal_strength_raw & 0x3f);
            out.extend_from_slice(&record.cdma_target_one_way_delay.to_be_bytes());
        }
        Ok(out)
    }

    /// Decodes the `Downlink Radio Environment` payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() < 2 {
            return Err(Error::Truncated {
                context: "Downlink Radio Environment",
                needed: 2,
                actual: input.len(),
            });
        }
        let count = input[0] as usize;
        if count == 0 {
            return Err(Error::InvalidValue {
                context: "Downlink Radio Environment",
                reason: "must contain at least one cell record",
            });
        }
        if input[1] != 0x02 {
            return Err(Error::ReservedValue {
                context: "Downlink Radio Environment discriminator",
                value: input[1],
            });
        }
        if input.len() != 2 + count * 5 {
            return Err(Error::InvalidLength {
                context: "Downlink Radio Environment",
                expected: 2 + count * 5,
                actual: input.len(),
            });
        }
        let mut records = Vec::with_capacity(count);
        let mut offset = 2;
        for _ in 0..count {
            let cell = CellId::decode(&[0x02, input[offset], input[offset + 1]])?;
            if input[offset + 2] & 0xc0 != 0 {
                return Err(Error::InvalidValue {
                    context: "Downlink Radio Environment",
                    reason: "reserved downlink signal strength bits must be zero",
                });
            }
            records.push(DownlinkRadioEnvironmentRecord {
                cell,
                downlink_signal_strength_raw: input[offset + 2] & 0x3f,
                cdma_target_one_way_delay: u16::from_be_bytes([
                    input[offset + 3],
                    input[offset + 4],
                ]),
            });
            offset += 5;
        }
        Ok(Self { records })
    }
}

/// Exact typed `CDMA Serving One Way Delay` payload used on Abis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CdmaServingOneWayDelay {
    pub cell: CellId,
    pub delay_100ns: u16,
}

impl CdmaServingOneWayDelay {
    /// Encodes the delay payload.
    pub fn encode(self) -> Result<[u8; 5]> {
        let cell = self.cell.encode()?;
        Ok([
            cell[0],
            cell[1],
            cell[2],
            (self.delay_100ns >> 8) as u8,
            self.delay_100ns as u8,
        ])
    }

    /// Decodes the delay payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 5 {
            return Err(Error::InvalidLength {
                context: "CDMA Serving One Way Delay",
                expected: 5,
                actual: input.len(),
            });
        }
        Ok(Self {
            cell: CellId::decode(&input[..3])?,
            delay_100ns: u16::from_be_bytes([input[3], input[4]]),
        })
    }
}

/// Exact typed `CDMA Target One Way Delay` payload used on Abis.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CdmaTargetOneWayDelay(pub u16);

impl CdmaTargetOneWayDelay {
    /// Encodes the target-delay payload.
    pub const fn encode(self) -> [u8; 2] {
        self.0.to_be_bytes()
    }

    /// Decodes the target-delay payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 2 {
            return Err(Error::InvalidLength {
                context: "CDMA Target One Way Delay",
                expected: 2,
                actual: input.len(),
            });
        }
        Ok(Self(u16::from_be_bytes([input[0], input[1]])))
    }
}

/// One cell-info record nested inside `A3 Connect Information`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CellInfoRecord {
    pub cell: CellId,
    pub qof_mask: u8,
    pub new_cell: bool,
    pub power_combine_indication: bool,
    pub pilot_pn: u16,
    pub code_channel: u8,
}

impl CellInfoRecord {
    /// Encodes the fixed-width cell-info record.
    pub fn encode(self) -> Result<[u8; 6]> {
        if self.qof_mask > 0x03 || self.pilot_pn > 0x01ff {
            return Err(Error::InvalidValue {
                context: "Cell Info Record",
                reason: "QOF mask or pilot PN out of range",
            });
        }
        let cell = self.cell.encode()?;
        Ok([
            cell[0],
            cell[1],
            cell[2],
            ((self.qof_mask & 0x03) << 4)
                | ((self.new_cell as u8) << 3)
                | ((self.power_combine_indication as u8) << 2)
                | (((self.pilot_pn >> 8) as u8) & 0x01),
            self.pilot_pn as u8,
            self.code_channel,
        ])
    }

    /// Decodes the fixed-width cell-info record.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 6 {
            return Err(Error::InvalidLength {
                context: "Cell Info Record",
                expected: 6,
                actual: input.len(),
            });
        }
        Ok(Self {
            cell: CellId::decode(&input[..3])?,
            qof_mask: (input[3] >> 4) & 0x03,
            new_cell: input[3] & 0x08 != 0,
            power_combine_indication: input[3] & 0x04 != 0,
            pilot_pn: (((input[3] & 0x01) as u16) << 8) | input[4] as u16,
            code_channel: input[5],
        })
    }
}

/// Exact typed extended handoff direction parameters.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ExtendedHandoffDirectionParameters {
    pub search_window_a_size: u8,
    pub search_window_n_size: u8,
    pub search_window_r_size: u8,
    pub t_add: u8,
    pub t_drop: u8,
    pub compare_threshold: u8,
    pub drop_timer_value: u8,
    pub neighbor_max_age: u8,
    pub soft_slope: u8,
    pub add_intercept: u8,
    pub drop_intercept: u8,
    pub target_bs_p_rev: u8,
}

impl ExtendedHandoffDirectionParameters {
    /// Encodes the fixed-width extended handoff direction payload.
    pub fn encode(self) -> Result<[u8; 9]> {
        if self.search_window_a_size > 0x0f
            || self.search_window_n_size > 0x0f
            || self.search_window_r_size > 0x0f
            || self.t_add > 0x3f
            || self.t_drop > 0x3f
            || self.compare_threshold > 0x0f
            || self.drop_timer_value > 0x0f
            || self.neighbor_max_age > 0x0f
            || self.soft_slope > 0x3f
            || self.add_intercept > 0x3f
            || self.drop_intercept > 0x3f
        {
            return Err(Error::InvalidValue {
                context: "Extended Handoff Direction Parameters",
                reason: "one or more fields exceed their bit width",
            });
        }
        Ok([
            (self.search_window_a_size << 4) | (self.search_window_n_size & 0x0f),
            (self.search_window_r_size << 4) | ((self.t_add >> 2) & 0x0f),
            ((self.t_add & 0x03) << 6) | (self.t_drop & 0x3f),
            (self.compare_threshold << 4) | (self.drop_timer_value & 0x0f),
            self.neighbor_max_age << 4,
            self.soft_slope,
            self.add_intercept,
            self.drop_intercept,
            self.target_bs_p_rev,
        ])
    }

    /// Decodes the fixed-width extended handoff direction payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 9 {
            return Err(Error::InvalidLength {
                context: "Extended Handoff Direction Parameters",
                expected: 9,
                actual: input.len(),
            });
        }
        Ok(Self {
            search_window_a_size: input[0] >> 4,
            search_window_n_size: input[0] & 0x0f,
            search_window_r_size: input[1] >> 4,
            t_add: ((input[1] & 0x0f) << 2) | (input[2] >> 6),
            t_drop: input[2] & 0x3f,
            compare_threshold: input[3] >> 4,
            drop_timer_value: input[3] & 0x0f,
            neighbor_max_age: input[4] >> 4,
            soft_slope: input[5] & 0x3f,
            add_intercept: input[6] & 0x3f,
            drop_intercept: input[7] & 0x3f,
            target_bs_p_rev: input[8],
        })
    }
}

/// Exact typed representation of `A3 Connect Information` / `Abis Connect Information`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct A3ConnectInformation {
    pub physical_channel_type: PhysicalChannelType,
    pub new_a3: bool,
    pub cell_info_records: Vec<CellInfoRecord>,
    pub traffic_circuit_id: TrafficCircuitId,
    pub extended_handoff_direction_parameters: Option<ExtendedHandoffDirectionParameters>,
    pub channel_element_id: Vec<u8>,
    pub a3_originating_id: u16,
    pub a7_destination_id: u16,
}

impl A3ConnectInformation {
    /// Encodes the `A3 Connect Information` payload.
    pub fn encode(&self) -> Result<Vec<u8>> {
        if self.cell_info_records.is_empty() || self.cell_info_records.len() > u8::MAX as usize / 6
        {
            return Err(Error::InvalidValue {
                context: "A3 Connect Information",
                reason: "must contain at least one cell info record",
            });
        }
        if self.channel_element_id.len() > u8::MAX as usize {
            return Err(Error::InvalidLength {
                context: "Channel Element ID",
                expected: u8::MAX as usize,
                actual: self.channel_element_id.len(),
            });
        }
        if self.physical_channel_type == PhysicalChannelType::Sch
            && self.extended_handoff_direction_parameters.is_some()
        {
            return Err(Error::InvalidValue {
                context: "A3 Connect Information",
                reason: "SCH channel type requires zero-length extended handoff direction parameters",
            });
        }
        let mut out = Vec::new();
        out.push(((self.physical_channel_type as u8) << 1) | (self.new_a3 as u8));
        out.push((self.cell_info_records.len() * 6) as u8);
        for cell_info in &self.cell_info_records {
            out.extend_from_slice(&cell_info.encode()?);
        }
        out.extend_from_slice(&self.traffic_circuit_id.encode());
        if let Some(parameters) = self.extended_handoff_direction_parameters {
            out.push(9);
            out.extend_from_slice(&parameters.encode()?);
        } else {
            out.push(0);
        }
        out.push(self.channel_element_id.len() as u8);
        out.extend_from_slice(&self.channel_element_id);
        out.push(0x02);
        out.extend_from_slice(&self.a3_originating_id.to_be_bytes());
        out.push(0x02);
        out.extend_from_slice(&self.a7_destination_id.to_be_bytes());
        Ok(out)
    }

    /// Decodes the `A3 Connect Information` payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() < 17 {
            return Err(Error::Truncated {
                context: "A3 Connect Information",
                needed: 17,
                actual: input.len(),
            });
        }
        let physical_channel_type = PhysicalChannelType::decode((input[0] >> 1) & 0x03)?;
        let cell_info_length = input[1] as usize;
        if cell_info_length == 0 || !cell_info_length.is_multiple_of(6) {
            return Err(Error::InvalidValue {
                context: "A3 Connect Information",
                reason: "cell info record length must be a non-zero multiple of 6",
            });
        }
        let mut offset = 2;
        let cell_info_end = offset + cell_info_length;
        if input.len() < cell_info_end + 13 {
            return Err(Error::Truncated {
                context: "A3 Connect Information",
                needed: cell_info_end + 13,
                actual: input.len(),
            });
        }
        let mut cell_info_records = Vec::with_capacity(cell_info_length / 6);
        while offset < cell_info_end {
            cell_info_records.push(CellInfoRecord::decode(&input[offset..offset + 6])?);
            offset += 6;
        }
        let traffic_circuit_id = TrafficCircuitId::decode(&input[offset..offset + 6])?;
        offset += 6;
        let ext_len = input[offset] as usize;
        offset += 1;
        let extended_handoff_direction_parameters = if ext_len == 0 {
            None
        } else {
            if ext_len != 9 || input.len() < offset + ext_len {
                return Err(Error::InvalidValue {
                    context: "A3 Connect Information",
                    reason: "extended handoff direction parameter length must be 0 or 9",
                });
            }
            let parameters =
                ExtendedHandoffDirectionParameters::decode(&input[offset..offset + ext_len])?;
            offset += ext_len;
            Some(parameters)
        };
        let channel_element_length = input[offset] as usize;
        offset += 1;
        if input.len() < offset + channel_element_length + 6 {
            return Err(Error::Truncated {
                context: "A3 Connect Information",
                needed: offset + channel_element_length + 6,
                actual: input.len(),
            });
        }
        let channel_element_id = input[offset..offset + channel_element_length].to_vec();
        offset += channel_element_length;
        if input[offset] != 0x02 || input[offset + 3] != 0x02 {
            return Err(Error::InvalidValue {
                context: "A3 Connect Information",
                reason: "unexpected ID length markers",
            });
        }
        Ok(Self {
            physical_channel_type,
            new_a3: input[0] & 0x01 != 0,
            cell_info_records,
            traffic_circuit_id,
            extended_handoff_direction_parameters,
            channel_element_id,
            a3_originating_id: u16::from_be_bytes([input[offset + 1], input[offset + 2]]),
            a7_destination_id: u16::from_be_bytes([input[offset + 4], input[offset + 5]]),
        })
    }
}

/// Abis uses the same payload structure as `A3 Connect Information`.
pub type AbisConnectInformation = A3ConnectInformation;

/// Exact typed Abis-originating identifier.
///
/// A.S0003 §7.21 defines this as a variable-length implementation-specific
/// identifier of up to eight octets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AbisOriginatingId(pub Vec<u8>);

impl AbisOriginatingId {
    /// Creates a validated Abis-originating identifier.
    pub fn new(value: impl Into<Vec<u8>>) -> Result<Self> {
        let value = value.into();
        if value.is_empty() || value.len() > 8 {
            return Err(Error::InvalidLength {
                context: "Abis Originating ID",
                expected: 8,
                actual: value.len(),
            });
        }
        Ok(Self(value))
    }

    /// Encodes the Abis-originating identifier payload.
    pub fn encode(&self) -> &[u8] {
        &self.0
    }

    /// Decodes the Abis-originating identifier payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        Self::new(input.to_vec())
    }
}

/// Exact typed Abis-destination identifier.
///
/// A.S0003 §7.22 defines this as a variable-length implementation-specific
/// identifier of up to eight octets.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AbisDestinationId(pub Vec<u8>);

impl AbisDestinationId {
    /// Creates a validated Abis-destination identifier.
    pub fn new(value: impl Into<Vec<u8>>) -> Result<Self> {
        let value = value.into();
        if value.is_empty() || value.len() > 8 {
            return Err(Error::InvalidLength {
                context: "Abis Destination ID",
                expected: 8,
                actual: value.len(),
            });
        }
        Ok(Self(value))
    }

    /// Encodes the Abis-destination identifier payload.
    pub fn encode(&self) -> &[u8] {
        &self.0
    }

    /// Decodes the Abis-destination identifier payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        Self::new(input.to_vec())
    }
}

/// Exact typed service-option payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ServiceOption(pub u16);

impl ServiceOption {
    /// Encodes the service-option payload.
    pub const fn encode(self) -> [u8; 2] {
        self.0.to_be_bytes()
    }

    /// Decodes the service-option payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 2 {
            return Err(Error::InvalidLength {
                context: "Service Option",
                expected: 2,
                actual: input.len(),
            });
        }
        Ok(Self(u16::from_be_bytes([input[0], input[1]])))
    }
}

/// Exact typed PACA timestamp payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PacaTimestamp(pub u32);

impl PacaTimestamp {
    /// Encodes the PACA timestamp payload.
    pub const fn encode(self) -> [u8; 4] {
        self.0.to_be_bytes()
    }

    /// Decodes the PACA timestamp payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 4 {
            return Err(Error::InvalidLength {
                context: "PACA Timestamp",
                expected: 4,
                actual: input.len(),
            });
        }
        Ok(Self(u32::from_be_bytes([
            input[0], input[1], input[2], input[3],
        ])))
    }
}

/// Exact typed `Forward Burst Radio Info` payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForwardBurstRadioInfo {
    pub coding_indicator: u8,
    pub qof_mask: u8,
    pub forward_code_channel_index: u16,
    pub pilot_pn_code: u16,
    pub forward_supplemental_channel_rate: u8,
    pub forward_supplemental_channel_start_time: u8,
    pub start_time_unit: u8,
    pub forward_supplemental_channel_duration: u8,
}

impl ForwardBurstRadioInfo {
    /// Encodes the `Forward Burst Radio Info` payload.
    pub fn encode(self) -> Result<[u8; 6]> {
        if self.coding_indicator > 0x01
            || self.qof_mask > 0x03
            || self.forward_code_channel_index > 0x07ff
            || self.pilot_pn_code > 0x01ff
            || self.forward_supplemental_channel_rate > 0x0f
            || self.forward_supplemental_channel_start_time > 0x1f
            || self.start_time_unit > 0x07
            || self.forward_supplemental_channel_duration > 0x0f
        {
            return Err(Error::InvalidValue {
                context: "Forward Burst Radio Info",
                reason: "one or more fields exceed their bit width",
            });
        }
        Ok([
            (self.coding_indicator << 6)
                | ((self.qof_mask & 0x03) << 3)
                | (((self.forward_code_channel_index >> 8) as u8) & 0x07),
            self.forward_code_channel_index as u8,
            self.pilot_pn_code as u8,
            (((self.pilot_pn_code >> 8) as u8) << 7)
                | (self.forward_supplemental_channel_rate & 0x0f),
            self.forward_supplemental_channel_start_time & 0x1f,
            ((self.start_time_unit & 0x07) << 4)
                | (self.forward_supplemental_channel_duration & 0x0f),
        ])
    }

    /// Decodes the `Forward Burst Radio Info` payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 6 {
            return Err(Error::InvalidLength {
                context: "Forward Burst Radio Info",
                expected: 6,
                actual: input.len(),
            });
        }
        Ok(Self {
            coding_indicator: (input[0] >> 6) & 0x01,
            qof_mask: (input[0] >> 3) & 0x03,
            forward_code_channel_index: (((input[0] & 0x07) as u16) << 8) | input[1] as u16,
            pilot_pn_code: (((input[3] >> 7) as u16) << 8) | input[2] as u16,
            forward_supplemental_channel_rate: input[3] & 0x0f,
            forward_supplemental_channel_start_time: input[4] & 0x1f,
            start_time_unit: (input[5] >> 4) & 0x07,
            forward_supplemental_channel_duration: input[5] & 0x0f,
        })
    }
}

/// Exact typed `Reverse Burst Radio Info` payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReverseBurstRadioInfo {
    pub coding_indicator: u8,
    pub reverse_supplemental_channel_rate: u8,
    pub reverse_supplemental_channel_start_time: u8,
    pub start_time_unit: u8,
    pub reverse_supplemental_channel_duration: u8,
}

impl ReverseBurstRadioInfo {
    /// Encodes the `Reverse Burst Radio Info` payload.
    pub fn encode(self) -> Result<[u8; 4]> {
        if self.coding_indicator > 0x01
            || self.reverse_supplemental_channel_rate > 0x0f
            || self.start_time_unit > 0x07
            || self.reverse_supplemental_channel_duration > 0x0f
        {
            return Err(Error::InvalidValue {
                context: "Reverse Burst Radio Info",
                reason: "one or more fields exceed their bit width",
            });
        }
        Ok([
            self.coding_indicator << 6,
            self.reverse_supplemental_channel_rate & 0x0f,
            self.reverse_supplemental_channel_start_time,
            ((self.start_time_unit & 0x07) << 4)
                | (self.reverse_supplemental_channel_duration & 0x0f),
        ])
    }

    /// Decodes the `Reverse Burst Radio Info` payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 4 {
            return Err(Error::InvalidLength {
                context: "Reverse Burst Radio Info",
                expected: 4,
                actual: input.len(),
            });
        }
        Ok(Self {
            coding_indicator: (input[0] >> 6) & 0x01,
            reverse_supplemental_channel_rate: input[1] & 0x0f,
            reverse_supplemental_channel_start_time: input[2],
            start_time_unit: (input[3] >> 4) & 0x07,
            reverse_supplemental_channel_duration: input[3] & 0x0f,
        })
    }
}

/// Exact typed `IS-2000 Forward Power Control Mode` payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Is2000ForwardPowerControlMode {
    pub fpc_mode: u8,
}

impl Is2000ForwardPowerControlMode {
    /// Encodes the `IS-2000 Forward Power Control Mode` payload.
    pub fn encode(self) -> Result<[u8; 2]> {
        if self.fpc_mode > 0x03 {
            return Err(Error::ReservedValue {
                context: "IS-2000 Forward Power Control Mode",
                value: self.fpc_mode,
            });
        }
        Ok([self.fpc_mode & 0x07, 0x00])
    }

    /// Decodes the `IS-2000 Forward Power Control Mode` payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 2 {
            return Err(Error::InvalidLength {
                context: "IS-2000 Forward Power Control Mode",
                expected: 2,
                actual: input.len(),
            });
        }
        Ok(Self {
            fpc_mode: input[0] & 0x07,
        })
    }
}

/// One min/max gain-ratio pair inside `IS-2000 FPC Gain Ratio Info`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GainRatioPair {
    pub min_gain_ratio: u8,
    pub max_gain_ratio: u8,
}

/// Exact typed `IS-2000 FPC Gain Ratio Info` payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Is2000FpcGainRatioInfo {
    pub initial_gain_ratio: u8,
    pub gain_adjust_step_size: u8,
    pub gain_ratio_pairs: [GainRatioPair; 3],
}

impl Is2000FpcGainRatioInfo {
    /// Encodes the `IS-2000 FPC Gain Ratio Info` payload.
    pub fn encode(&self) -> Result<[u8; 8]> {
        if self.gain_adjust_step_size > 0x0f {
            return Err(Error::InvalidValue {
                context: "IS-2000 FPC Gain Ratio Info",
                reason: "gain adjust step size must fit in 4 bits",
            });
        }
        Ok([
            self.initial_gain_ratio,
            (self.gain_adjust_step_size << 3) | 0x03,
            self.gain_ratio_pairs[0].min_gain_ratio,
            self.gain_ratio_pairs[0].max_gain_ratio,
            self.gain_ratio_pairs[1].min_gain_ratio,
            self.gain_ratio_pairs[1].max_gain_ratio,
            self.gain_ratio_pairs[2].min_gain_ratio,
            self.gain_ratio_pairs[2].max_gain_ratio,
        ])
    }

    /// Decodes the `IS-2000 FPC Gain Ratio Info` payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 8 {
            return Err(Error::InvalidLength {
                context: "IS-2000 FPC Gain Ratio Info",
                expected: 8,
                actual: input.len(),
            });
        }
        if input[1] & 0x07 != 0x03 {
            return Err(Error::InvalidValue {
                context: "IS-2000 FPC Gain Ratio Info",
                reason: "count of gain ratio pairs must be 3",
            });
        }
        Ok(Self {
            initial_gain_ratio: input[0],
            gain_adjust_step_size: input[1] >> 3,
            gain_ratio_pairs: [
                GainRatioPair {
                    min_gain_ratio: input[2],
                    max_gain_ratio: input[3],
                },
                GainRatioPair {
                    min_gain_ratio: input[4],
                    max_gain_ratio: input[5],
                },
                GainRatioPair {
                    min_gain_ratio: input[6],
                    max_gain_ratio: input[7],
                },
            ],
        })
    }
}

/// A one-octet channel element status used by traffic channel status messages.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelElementStatus {
    pub transmit_on: bool,
}

impl ChannelElementStatus {
    /// Encodes the status payload.
    pub fn encode(self) -> [u8; 1] {
        [self.transmit_on as u8]
    }

    /// Decodes the status payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 1 {
            return Err(Error::InvalidLength {
                context: "Channel Element Status",
                expected: 1,
                actual: input.len(),
            });
        }
        if input[0] & 0xfe != 0 {
            return Err(Error::InvalidValue {
                context: "Channel Element Status",
                reason: "reserved bits must be zero",
            });
        }
        Ok(Self {
            transmit_on: input[0] & 0x01 != 0,
        })
    }
}

/// Manufacturer-specific records carried in `Abis-BTS Release Request`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManufacturerSpecificRecords {
    pub manufacturer_id: u8,
    pub information: Vec<u8>,
}

/// PACA action values carried by the PACA Order IE.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PacaActionRequired {
    UpdateQueuePosition = 0b001,
    RemoveMsFromQueue = 0b011,
}

impl PacaActionRequired {
    /// Encodes the PACA action field.
    pub const fn encode(self) -> u8 {
        self as u8
    }
}

impl ManufacturerSpecificRecords {
    /// Encodes the record payload.
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(1 + self.information.len());
        out.push(self.manufacturer_id);
        out.extend_from_slice(&self.information);
        out
    }

    /// Decodes the record payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let Some((&manufacturer_id, information)) = input.split_first() else {
            return Err(Error::Truncated {
                context: "Manufacturer Specific Records",
                needed: 1,
                actual: 0,
            });
        };
        Ok(Self {
            manufacturer_id,
            information: information.to_vec(),
        })
    }
}

/// Typed representation of the inherited `A3 Connect Ack Information` structure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct A3ConnectAckInformation {
    pub soft_handoff_leg: u8,
    pub pmc_cause: Option<u8>,
    pub transmit_tch_status: bool,
    pub traffic_circuit_id: TrafficCircuitId,
    pub channel_element_id: Vec<u8>,
    pub a3_originating_id: u16,
    pub a3_destination_id: u16,
}

impl A3ConnectAckInformation {
    /// Encodes the nested `A3 Connect Ack Information` payload.
    pub fn encode(&self) -> Result<Vec<u8>> {
        if self.soft_handoff_leg > 0x0f || self.channel_element_id.is_empty() {
            return Err(Error::InvalidValue {
                context: "A3 Connect Ack Information",
                reason: "soft handoff leg or channel element id is invalid",
            });
        }
        let mut out = Vec::new();
        let flags = ((self.soft_handoff_leg & 0x0f) << 2)
            | ((self.pmc_cause.is_some() as u8) << 1)
            | (self.transmit_tch_status as u8);
        out.push(flags);
        out.extend_from_slice(&self.traffic_circuit_id.encode());
        if self.channel_element_id.len() > u8::MAX as usize {
            return Err(Error::InvalidLength {
                context: "Channel Element ID",
                expected: u8::MAX as usize,
                actual: self.channel_element_id.len(),
            });
        }
        out.push(self.channel_element_id.len() as u8);
        out.extend_from_slice(&self.channel_element_id);
        if let Some(pmc_cause) = self.pmc_cause {
            out.push(pmc_cause);
        }
        out.push(0x02);
        out.extend_from_slice(&self.a3_originating_id.to_be_bytes());
        out.push(0x02);
        out.extend_from_slice(&self.a3_destination_id.to_be_bytes());
        Ok(out)
    }

    /// Decodes the nested `A3 Connect Ack Information` payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() < 13 {
            return Err(Error::Truncated {
                context: "A3 Connect Ack Information",
                needed: 13,
                actual: input.len(),
            });
        }
        let flags = input[0];
        let traffic_circuit_id = TrafficCircuitId::decode(&input[1..7])?;
        let channel_len = input[7] as usize;
        let mut offset = 8;
        if input.len() < offset + channel_len + 6 {
            return Err(Error::Truncated {
                context: "A3 Connect Ack Information",
                needed: offset + channel_len + 6,
                actual: input.len(),
            });
        }
        let channel_element_id = input[offset..offset + channel_len].to_vec();
        offset += channel_len;
        let pmc_cause_present = flags & 0x02 != 0;
        let pmc_cause = if pmc_cause_present {
            let value = input[offset];
            offset += 1;
            Some(value)
        } else {
            None
        };
        if input[offset] != 0x02 || input[offset + 3] != 0x02 {
            return Err(Error::InvalidValue {
                context: "A3 Connect Ack Information",
                reason: "unexpected A3 ID length markers",
            });
        }
        Ok(Self {
            soft_handoff_leg: (flags >> 2) & 0x0f,
            pmc_cause,
            transmit_tch_status: flags & 0x01 != 0,
            traffic_circuit_id,
            channel_element_id,
            a3_originating_id: u16::from_be_bytes([input[offset + 1], input[offset + 2]]),
            a3_destination_id: u16::from_be_bytes([input[offset + 4], input[offset + 5]]),
        })
    }
}

/// Typed representation of the inherited `A3 Remove Information` structure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct A3RemoveInformation {
    pub traffic_circuit_id: TrafficCircuitId,
    pub cells_to_be_removed: Vec<CellIdWithMscId>,
    pub a3_destination_id: u16,
    pub a7_destination_id: u16,
}

impl A3RemoveInformation {
    /// Encodes the nested `A3 Remove Information` payload.
    pub fn encode(&self) -> Result<Vec<u8>> {
        if self.cells_to_be_removed.is_empty() || self.cells_to_be_removed.len() > u8::MAX as usize
        {
            return Err(Error::InvalidValue {
                context: "A3 Remove Information",
                reason: "must contain at least one cell and fit in one octet count",
            });
        }
        let mut out = Vec::new();
        out.extend_from_slice(&self.traffic_circuit_id.encode());
        out.push(self.cells_to_be_removed.len() as u8);
        for cell in &self.cells_to_be_removed {
            out.extend_from_slice(&cell.encode()?);
        }
        out.push(0x02);
        out.extend_from_slice(&self.a3_destination_id.to_be_bytes());
        out.push(0x02);
        out.extend_from_slice(&self.a7_destination_id.to_be_bytes());
        Ok(out)
    }

    /// Decodes the nested `A3 Remove Information` payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() < 17 {
            return Err(Error::Truncated {
                context: "A3 Remove Information",
                needed: 17,
                actual: input.len(),
            });
        }
        let traffic_circuit_id = TrafficCircuitId::decode(&input[..6])?;
        let cell_count = input[6] as usize;
        let mut offset = 7;
        let mut cells_to_be_removed = Vec::with_capacity(cell_count);
        for _ in 0..cell_count {
            if input.len() < offset + 6 {
                return Err(Error::Truncated {
                    context: "A3 Remove Information cells",
                    needed: offset + 6,
                    actual: input.len(),
                });
            }
            cells_to_be_removed.push(CellIdWithMscId::decode(&input[offset..offset + 6])?);
            offset += 6;
        }
        if input.len() < offset + 6 || input[offset] != 0x02 || input[offset + 3] != 0x02 {
            return Err(Error::InvalidValue {
                context: "A3 Remove Information",
                reason: "unexpected destination id length markers",
            });
        }
        Ok(Self {
            traffic_circuit_id,
            cells_to_be_removed,
            a3_destination_id: u16::from_be_bytes([input[offset + 1], input[offset + 2]]),
            a7_destination_id: u16::from_be_bytes([input[offset + 4], input[offset + 5]]),
        })
    }
}

/// Exact typed `Abis-Connect` message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectMessage {
    pub call_connection_reference: CallConnectionReference,
    pub correlation_id: Option<CorrelationId>,
    pub sdu_id: Option<SduId>,
    pub connect_information: Vec<A3ConnectInformation>,
    pub physical_channel_info: PhysicalChannelInfo,
}

impl ConnectMessage {
    /// Encodes the `Abis-Connect` message.
    pub fn encode(&self) -> Result<Vec<u8>> {
        if self.connect_information.is_empty() {
            return Err(Error::InvalidValue {
                context: "Abis-Connect",
                reason: "must contain at least one A3 Connect Information element",
            });
        }
        let mut out = vec![MessageType::Connect.value()];
        push_ie(
            &mut out,
            ElementId::CallConnectionReference.value(),
            &self.call_connection_reference.encode(),
        )?;
        if let Some(correlation_id) = self.correlation_id {
            push_ie(
                &mut out,
                ElementId::CorrelationId.value(),
                &correlation_id.encode(),
            )?;
        }
        if let Some(sdu_id) = &self.sdu_id {
            push_ie(&mut out, ElementId::SduId.value(), sdu_id.encode())?;
        }
        for connect_information in &self.connect_information {
            let payload = connect_information.encode()?;
            push_ie(&mut out, ElementId::A3ConnectInformation.value(), &payload)?;
        }
        push_ie(&mut out, 0x07, &self.physical_channel_info.encode()?)?;
        Ok(out)
    }

    /// Decodes the `Abis-Connect` message.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let mut cursor = MessageCursor::new(MessageType::Connect, input)?;
        let mut call_connection_reference = None;
        let mut correlation_id = None;
        let mut sdu_id = None;
        let mut connect_information = Vec::new();
        let mut physical_channel_info = None;
        while let Some((id, payload)) = cursor.next_ie()? {
            match id {
                x if x == ElementId::CallConnectionReference.value() => {
                    call_connection_reference = Some(CallConnectionReference::decode(payload)?);
                }
                x if x == ElementId::CorrelationId.value() => {
                    correlation_id = Some(CorrelationId::decode(payload)?);
                }
                x if x == ElementId::SduId.value() => {
                    sdu_id = Some(SduId::decode(payload)?);
                }
                x if x == ElementId::A3ConnectInformation.value() => {
                    connect_information.push(A3ConnectInformation::decode(payload)?);
                }
                0x07 => {
                    physical_channel_info = Some(PhysicalChannelInfo::decode(payload)?);
                }
                other => return Err(Error::UnknownInformationElement(other)),
            }
        }
        let call_connection_reference =
            call_connection_reference.ok_or(Error::MissingRequiredElement {
                message_type: MessageType::Connect.value(),
                id: ElementId::CallConnectionReference.value(),
            })?;
        if connect_information.is_empty() {
            return Err(Error::MissingRequiredElement {
                message_type: MessageType::Connect.value(),
                id: ElementId::A3ConnectInformation.value(),
            });
        }
        Ok(Self {
            call_connection_reference,
            correlation_id,
            sdu_id,
            connect_information,
            physical_channel_info: physical_channel_info.ok_or(Error::MissingRequiredElement {
                message_type: MessageType::Connect.value(),
                id: 0x07,
            })?,
        })
    }
}

/// Exact typed `Abis-BTS Setup` message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BtsSetupMessage {
    pub call_connection_reference: CallConnectionReference,
    pub band_class: Option<BandClass>,
    pub privacy_info: Option<PrivacyInfo>,
    pub sdu_id: Option<SduId>,
    pub mobile_identities: Vec<MobileIdentity>,
    pub physical_channel_info: Option<PhysicalChannelInfo>,
    pub service_option: Option<ServiceOption>,
    pub paca_timestamp: Option<PacaTimestamp>,
    pub quality_of_service_parameters: Option<QualityOfServiceParameters>,
    pub connect_information: Vec<AbisConnectInformation>,
    pub abis_originating_id: Option<AbisOriginatingId>,
    pub cdma_serving_one_way_delay: CdmaServingOneWayDelay,
    pub cdma_target_one_way_delay: Option<CdmaTargetOneWayDelay>,
    pub walsh_code_assignment_request: bool,
}

impl BtsSetupMessage {
    /// Encodes the `Abis-BTS Setup` message.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut out = vec![MessageType::BtsSetup.value()];
        push_ie(
            &mut out,
            ElementId::CallConnectionReference.value(),
            &self.call_connection_reference.encode(),
        )?;
        if let Some(band_class) = self.band_class {
            push_ie(&mut out, ElementId::BandClass.value(), &band_class.encode())?;
        }
        if let Some(privacy_info) = &self.privacy_info {
            let payload = privacy_info.encode()?;
            push_ie(&mut out, ElementId::PrivacyInfo.value(), &payload)?;
        }
        if let Some(sdu_id) = &self.sdu_id {
            push_ie(&mut out, ElementId::SduId.value(), sdu_id.encode())?;
        }
        for mobile_identity in &self.mobile_identities {
            push_ie(
                &mut out,
                ElementId::MobileIdentity.value(),
                &mobile_identity.encode()?,
            )?;
        }
        if let Some(physical_channel_info) = &self.physical_channel_info {
            push_ie(&mut out, 0x07, &physical_channel_info.encode()?)?;
        }
        if let Some(service_option) = self.service_option {
            push_ie(
                &mut out,
                ElementId::ServiceOption.value(),
                &service_option.encode(),
            )?;
        }
        if let Some(paca_timestamp) = self.paca_timestamp {
            push_ie(
                &mut out,
                ElementId::PacaTimestamp.value(),
                &paca_timestamp.encode(),
            )?;
        }
        if let Some(quality_of_service_parameters) = self.quality_of_service_parameters {
            push_ie(
                &mut out,
                ElementId::QualityOfServiceParameters.value(),
                &quality_of_service_parameters.encode()?,
            )?;
        }
        for connect_information in &self.connect_information {
            let payload = connect_information.encode()?;
            push_ie(&mut out, ElementId::A3ConnectInformation.value(), &payload)?;
        }
        if let Some(abis_originating_id) = &self.abis_originating_id {
            push_ie(
                &mut out,
                ElementId::AbisOriginatingId.value(),
                abis_originating_id.encode(),
            )?;
        }
        push_ie(
            &mut out,
            ElementId::CdmaServingOneWayDelay.value(),
            &self.cdma_serving_one_way_delay.encode()?,
        )?;
        if let Some(cdma_target_one_way_delay) = self.cdma_target_one_way_delay {
            push_ie(
                &mut out,
                ElementId::CdmaTargetOneWayDelay.value(),
                &cdma_target_one_way_delay.encode(),
            )?;
        }
        if self.walsh_code_assignment_request {
            push_ie(&mut out, ElementId::WalshCodeAssignmentRequest.value(), &[])?;
        }
        Ok(out)
    }

    /// Decodes the `Abis-BTS Setup` message.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let mut cursor = MessageCursor::new(MessageType::BtsSetup, input)?;
        let mut call_connection_reference = None;
        let mut band_class = None;
        let mut privacy_info = None;
        let mut sdu_id = None;
        let mut mobile_identities = Vec::new();
        let mut physical_channel_info = None;
        let mut service_option = None;
        let mut paca_timestamp = None;
        let mut quality_of_service_parameters = None;
        let mut connect_information = Vec::new();
        let mut abis_originating_id = None;
        let mut cdma_serving_one_way_delay = None;
        let mut cdma_target_one_way_delay = None;
        let mut walsh_code_assignment_request = false;
        while let Some((id, payload)) = cursor.next_ie()? {
            match id {
                x if x == ElementId::CallConnectionReference.value() => {
                    call_connection_reference = Some(CallConnectionReference::decode(payload)?)
                }
                x if x == ElementId::BandClass.value() => {
                    band_class = Some(BandClass::decode(payload)?)
                }
                x if x == ElementId::PrivacyInfo.value() => {
                    privacy_info = Some(PrivacyInfo::decode(payload)?)
                }
                x if x == ElementId::SduId.value() => sdu_id = Some(SduId::decode(payload)?),
                x if x == ElementId::MobileIdentity.value() => {
                    mobile_identities.push(MobileIdentity::decode(payload)?)
                }
                0x07 => {
                    if physical_channel_info.is_none() {
                        physical_channel_info = Some(PhysicalChannelInfo::decode(payload)?);
                    } else {
                        quality_of_service_parameters =
                            Some(QualityOfServiceParameters::decode(payload)?);
                    }
                }
                x if x == ElementId::ServiceOption.value() => {
                    service_option = Some(ServiceOption::decode(payload)?)
                }
                x if x == ElementId::PacaTimestamp.value() => {
                    paca_timestamp = Some(PacaTimestamp::decode(payload)?)
                }
                x if x == ElementId::A3ConnectInformation.value() => {
                    connect_information.push(A3ConnectInformation::decode(payload)?)
                }
                x if x == ElementId::AbisOriginatingId.value() => {
                    abis_originating_id = Some(AbisOriginatingId::decode(payload)?)
                }
                x if x == ElementId::CdmaServingOneWayDelay.value() => {
                    cdma_serving_one_way_delay = Some(CdmaServingOneWayDelay::decode(payload)?)
                }
                x if x == ElementId::CdmaTargetOneWayDelay.value() => {
                    cdma_target_one_way_delay = Some(CdmaTargetOneWayDelay::decode(payload)?)
                }
                x if x == ElementId::WalshCodeAssignmentRequest.value() => {
                    if !payload.is_empty() {
                        return Err(Error::InvalidLength {
                            context: "Walsh Code Assignment Request",
                            expected: 0,
                            actual: payload.len(),
                        });
                    }
                    walsh_code_assignment_request = true;
                }
                other => return Err(Error::UnknownInformationElement(other)),
            }
        }
        Ok(Self {
            call_connection_reference: call_connection_reference.ok_or(
                Error::MissingRequiredElement {
                    message_type: MessageType::BtsSetup.value(),
                    id: ElementId::CallConnectionReference.value(),
                },
            )?,
            band_class,
            privacy_info,
            sdu_id,
            mobile_identities,
            physical_channel_info,
            service_option,
            paca_timestamp,
            quality_of_service_parameters,
            connect_information,
            abis_originating_id,
            cdma_serving_one_way_delay: cdma_serving_one_way_delay.ok_or(
                Error::MissingRequiredElement {
                    message_type: MessageType::BtsSetup.value(),
                    id: ElementId::CdmaServingOneWayDelay.value(),
                },
            )?,
            cdma_target_one_way_delay,
            walsh_code_assignment_request,
        })
    }
}

/// Exact typed `Abis-BTS Setup Ack` message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BtsSetupAckMessage {
    pub call_connection_reference: CallConnectionReference,
    pub connect_information: Vec<AbisConnectInformation>,
    pub abis_originating_id: Option<AbisOriginatingId>,
    pub abis_destination_id: Option<AbisDestinationId>,
    pub cause: Option<u8>,
}

impl BtsSetupAckMessage {
    /// Encodes the `Abis-BTS Setup Ack` message.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut out = vec![MessageType::BtsSetupAck.value()];
        push_ie(
            &mut out,
            ElementId::CallConnectionReference.value(),
            &self.call_connection_reference.encode(),
        )?;
        for connect_information in &self.connect_information {
            let payload = connect_information.encode()?;
            push_ie(&mut out, ElementId::A3ConnectInformation.value(), &payload)?;
        }
        if let Some(abis_originating_id) = &self.abis_originating_id {
            push_ie(
                &mut out,
                ElementId::AbisOriginatingId.value(),
                abis_originating_id.encode(),
            )?;
        }
        if let Some(abis_destination_id) = &self.abis_destination_id {
            push_ie(
                &mut out,
                ElementId::AbisDestinationId.value(),
                abis_destination_id.encode(),
            )?;
        }
        if let Some(cause) = self.cause {
            push_ie(
                &mut out,
                ElementId::Cause.value(),
                &[validate_bts_setup_ack_cause(cause)?],
            )?;
        }
        Ok(out)
    }

    /// Decodes the `Abis-BTS Setup Ack` message.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let mut cursor = MessageCursor::new(MessageType::BtsSetupAck, input)?;
        let mut call_connection_reference = None;
        let mut connect_information = Vec::new();
        let mut abis_originating_id = None;
        let mut abis_destination_id = None;
        let mut cause = None;
        while let Some((id, payload)) = cursor.next_ie()? {
            match id {
                x if x == ElementId::CallConnectionReference.value() => {
                    call_connection_reference = Some(CallConnectionReference::decode(payload)?)
                }
                x if x == ElementId::A3ConnectInformation.value() => {
                    connect_information.push(A3ConnectInformation::decode(payload)?)
                }
                x if x == ElementId::AbisOriginatingId.value() => {
                    abis_originating_id = Some(AbisOriginatingId::decode(payload)?)
                }
                x if x == ElementId::AbisDestinationId.value() => {
                    abis_destination_id = Some(AbisDestinationId::decode(payload)?)
                }
                x if x == ElementId::Cause.value() => {
                    if payload.len() != 1 {
                        return Err(Error::InvalidLength {
                            context: "Cause",
                            expected: 1,
                            actual: payload.len(),
                        });
                    }
                    cause = Some(validate_bts_setup_ack_cause(payload[0])?);
                }
                other => return Err(Error::UnknownInformationElement(other)),
            }
        }
        Ok(Self {
            call_connection_reference: call_connection_reference.ok_or(
                Error::MissingRequiredElement {
                    message_type: MessageType::BtsSetupAck.value(),
                    id: ElementId::CallConnectionReference.value(),
                },
            )?,
            connect_information,
            abis_originating_id,
            abis_destination_id,
            cause,
        })
    }
}

/// Exact typed `Abis-Burst Request` message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BurstRequestMessage {
    pub call_connection_reference: Option<CallConnectionReference>,
    pub band_class: Option<BandClass>,
    pub downlink_radio_environment: Option<DownlinkRadioEnvironment>,
    pub cdma_serving_one_way_delay: Option<CdmaServingOneWayDelay>,
    pub privacy_info: Option<PrivacyInfo>,
    pub correlation_id: Option<CorrelationId>,
    pub sdu_id: Option<SduId>,
    pub mobile_identities: Vec<MobileIdentity>,
    pub cell_identifier_list: Option<Vec<CellId>>,
    pub forward_burst_radio_info: Option<ForwardBurstRadioInfo>,
    pub reverse_burst_radio_info: Option<ReverseBurstRadioInfo>,
    pub abis_destination_id: Option<AbisDestinationId>,
}

impl BurstRequestMessage {
    /// Encodes the `Abis-Burst Request` message.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut out = vec![MessageType::BurstRequest.value()];
        if let Some(call_connection_reference) = self.call_connection_reference {
            push_ie(
                &mut out,
                ElementId::CallConnectionReference.value(),
                &call_connection_reference.encode(),
            )?;
        }
        if let Some(band_class) = self.band_class {
            push_ie(&mut out, ElementId::BandClass.value(), &band_class.encode())?;
        }
        if let Some(downlink_radio_environment) = &self.downlink_radio_environment {
            let payload = downlink_radio_environment.encode()?;
            push_ie(
                &mut out,
                ElementId::DownlinkRadioEnvironment.value(),
                &payload,
            )?;
        }
        if let Some(cdma_serving_one_way_delay) = self.cdma_serving_one_way_delay {
            push_ie(
                &mut out,
                ElementId::CdmaServingOneWayDelay.value(),
                &cdma_serving_one_way_delay.encode()?,
            )?;
        }
        if let Some(privacy_info) = &self.privacy_info {
            let payload = privacy_info.encode()?;
            push_ie(&mut out, ElementId::PrivacyInfo.value(), &payload)?;
        }
        if let Some(correlation_id) = self.correlation_id {
            push_ie(
                &mut out,
                ElementId::CorrelationId.value(),
                &correlation_id.encode(),
            )?;
        }
        if let Some(sdu_id) = &self.sdu_id {
            push_ie(&mut out, ElementId::SduId.value(), sdu_id.encode())?;
        }
        for mobile_identity in &self.mobile_identities {
            push_ie(
                &mut out,
                ElementId::MobileIdentity.value(),
                &mobile_identity.encode()?,
            )?;
        }
        if let Some(cell_identifier_list) = &self.cell_identifier_list {
            push_ie(
                &mut out,
                ElementId::CellIdentifierList.value(),
                &encode_cell_identifier_list(cell_identifier_list)?,
            )?;
        }
        if let Some(forward_burst_radio_info) = self.forward_burst_radio_info {
            push_ie(
                &mut out,
                ElementId::ForwardBurstRadioInfo.value(),
                &forward_burst_radio_info.encode()?,
            )?;
        }
        if let Some(reverse_burst_radio_info) = self.reverse_burst_radio_info {
            push_ie(
                &mut out,
                ElementId::ReverseBurstRadioInfo.value(),
                &reverse_burst_radio_info.encode()?,
            )?;
        }
        if let Some(abis_destination_id) = &self.abis_destination_id {
            push_ie(
                &mut out,
                ElementId::AbisDestinationId.value(),
                abis_destination_id.encode(),
            )?;
        }
        Ok(out)
    }

    /// Decodes the `Abis-Burst Request` message.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let mut cursor = MessageCursor::new(MessageType::BurstRequest, input)?;
        let mut call_connection_reference = None;
        let mut band_class = None;
        let mut downlink_radio_environment = None;
        let mut cdma_serving_one_way_delay = None;
        let mut privacy_info = None;
        let mut correlation_id = None;
        let mut sdu_id = None;
        let mut mobile_identities = Vec::new();
        let mut cell_identifier_list = None;
        let mut forward_burst_radio_info = None;
        let mut reverse_burst_radio_info = None;
        let mut abis_destination_id = None;
        while let Some((id, payload)) = cursor.next_ie()? {
            match id {
                x if x == ElementId::CallConnectionReference.value() => {
                    call_connection_reference = Some(CallConnectionReference::decode(payload)?)
                }
                x if x == ElementId::BandClass.value() => {
                    band_class = Some(BandClass::decode(payload)?)
                }
                x if x == ElementId::DownlinkRadioEnvironment.value() => {
                    downlink_radio_environment = Some(DownlinkRadioEnvironment::decode(payload)?)
                }
                x if x == ElementId::CdmaServingOneWayDelay.value() => {
                    cdma_serving_one_way_delay = Some(CdmaServingOneWayDelay::decode(payload)?)
                }
                x if x == ElementId::PrivacyInfo.value() => {
                    privacy_info = Some(PrivacyInfo::decode(payload)?)
                }
                x if x == ElementId::CorrelationId.value() => {
                    correlation_id = Some(CorrelationId::decode(payload)?)
                }
                x if x == ElementId::SduId.value() => sdu_id = Some(SduId::decode(payload)?),
                x if x == ElementId::MobileIdentity.value() => {
                    mobile_identities.push(MobileIdentity::decode(payload)?)
                }
                x if x == ElementId::CellIdentifierList.value() => {
                    cell_identifier_list = Some(decode_cell_identifier_list(payload)?)
                }
                x if x == ElementId::ForwardBurstRadioInfo.value() => {
                    forward_burst_radio_info = Some(ForwardBurstRadioInfo::decode(payload)?)
                }
                x if x == ElementId::ReverseBurstRadioInfo.value() => {
                    reverse_burst_radio_info = Some(ReverseBurstRadioInfo::decode(payload)?)
                }
                x if x == ElementId::AbisDestinationId.value() => {
                    abis_destination_id = Some(AbisDestinationId::decode(payload)?)
                }
                other => return Err(Error::UnknownInformationElement(other)),
            }
        }
        Ok(Self {
            call_connection_reference,
            band_class,
            downlink_radio_environment,
            cdma_serving_one_way_delay,
            privacy_info,
            correlation_id,
            sdu_id,
            mobile_identities,
            cell_identifier_list,
            forward_burst_radio_info,
            reverse_burst_radio_info,
            abis_destination_id,
        })
    }
}

/// Exact typed `Abis-Burst Response` message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BurstResponseMessage {
    pub call_connection_reference: Option<CallConnectionReference>,
    pub correlation_id: Option<CorrelationId>,
    pub committed_cell_identifier_list: Option<Vec<CellId>>,
    pub uncommitted_cell_identifier_list: Option<Vec<CellId>>,
    pub forward_burst_radio_info: Option<ForwardBurstRadioInfo>,
    pub reverse_burst_radio_info: Option<ReverseBurstRadioInfo>,
    pub abis_destination_id: Option<AbisDestinationId>,
}

impl BurstResponseMessage {
    /// Encodes the `Abis-Burst Response` message.
    pub fn encode(&self) -> Result<Vec<u8>> {
        if self
            .committed_cell_identifier_list
            .as_ref()
            .is_some_and(|cells| cells.len() > 1)
            || self
                .uncommitted_cell_identifier_list
                .as_ref()
                .is_some_and(|cells| cells.len() > 1)
        {
            return Err(Error::InvalidValue {
                context: "Abis-Burst Response",
                reason: "each response may include at most one committed and one uncommitted cell",
            });
        }
        let mut out = vec![MessageType::BurstResponse.value()];
        if let Some(call_connection_reference) = self.call_connection_reference {
            push_ie(
                &mut out,
                ElementId::CallConnectionReference.value(),
                &call_connection_reference.encode(),
            )?;
        }
        if let Some(correlation_id) = self.correlation_id {
            push_ie(
                &mut out,
                ElementId::CorrelationId.value(),
                &correlation_id.encode(),
            )?;
        }
        if let Some(committed_cell_identifier_list) = &self.committed_cell_identifier_list {
            push_ie(
                &mut out,
                ElementId::CellIdentifierList.value(),
                &encode_cell_identifier_list(committed_cell_identifier_list)?,
            )?;
        }
        if let Some(uncommitted_cell_identifier_list) = &self.uncommitted_cell_identifier_list {
            push_ie(
                &mut out,
                ElementId::CellIdentifierList.value(),
                &encode_cell_identifier_list(uncommitted_cell_identifier_list)?,
            )?;
        }
        if let Some(forward_burst_radio_info) = self.forward_burst_radio_info {
            push_ie(
                &mut out,
                ElementId::ForwardBurstRadioInfo.value(),
                &forward_burst_radio_info.encode()?,
            )?;
        }
        if let Some(reverse_burst_radio_info) = self.reverse_burst_radio_info {
            push_ie(
                &mut out,
                ElementId::ReverseBurstRadioInfo.value(),
                &reverse_burst_radio_info.encode()?,
            )?;
        }
        if let Some(abis_destination_id) = &self.abis_destination_id {
            push_ie(
                &mut out,
                ElementId::AbisDestinationId.value(),
                abis_destination_id.encode(),
            )?;
        }
        Ok(out)
    }

    /// Decodes the `Abis-Burst Response` message.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let mut cursor = MessageCursor::new(MessageType::BurstResponse, input)?;
        let mut call_connection_reference = None;
        let mut correlation_id = None;
        let mut committed_cell_identifier_list = None;
        let mut uncommitted_cell_identifier_list = None;
        let mut forward_burst_radio_info = None;
        let mut reverse_burst_radio_info = None;
        let mut abis_destination_id = None;
        while let Some((id, payload)) = cursor.next_ie()? {
            match id {
                x if x == ElementId::CallConnectionReference.value() => {
                    call_connection_reference = Some(CallConnectionReference::decode(payload)?)
                }
                x if x == ElementId::CorrelationId.value() => {
                    correlation_id = Some(CorrelationId::decode(payload)?)
                }
                x if x == ElementId::CellIdentifierList.value() => {
                    if committed_cell_identifier_list.is_none() {
                        committed_cell_identifier_list =
                            Some(decode_cell_identifier_list(payload)?);
                    } else if uncommitted_cell_identifier_list.is_none() {
                        uncommitted_cell_identifier_list =
                            Some(decode_cell_identifier_list(payload)?);
                    } else {
                        return Err(Error::DuplicateElement {
                            message_type: MessageType::BurstResponse.value(),
                            id,
                        });
                    }
                }
                x if x == ElementId::ForwardBurstRadioInfo.value() => {
                    forward_burst_radio_info = Some(ForwardBurstRadioInfo::decode(payload)?)
                }
                x if x == ElementId::ReverseBurstRadioInfo.value() => {
                    reverse_burst_radio_info = Some(ReverseBurstRadioInfo::decode(payload)?)
                }
                x if x == ElementId::AbisDestinationId.value() => {
                    abis_destination_id = Some(AbisDestinationId::decode(payload)?)
                }
                other => return Err(Error::UnknownInformationElement(other)),
            }
        }
        Self {
            call_connection_reference,
            correlation_id,
            committed_cell_identifier_list,
            uncommitted_cell_identifier_list,
            forward_burst_radio_info,
            reverse_burst_radio_info,
            abis_destination_id,
        }
        .validate()
    }

    fn validate(self) -> Result<Self> {
        if self
            .committed_cell_identifier_list
            .as_ref()
            .is_some_and(|cells| cells.len() > 1)
            || self
                .uncommitted_cell_identifier_list
                .as_ref()
                .is_some_and(|cells| cells.len() > 1)
        {
            return Err(Error::InvalidValue {
                context: "Abis-Burst Response",
                reason: "each response may include at most one committed and one uncommitted cell",
            });
        }
        Ok(self)
    }
}

/// Exact typed `Abis-Burst Commit` message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BurstCommitMessage {
    pub call_connection_reference: Option<CallConnectionReference>,
    pub correlation_id: Option<CorrelationId>,
    pub forward_cell_identifier_list: Option<Vec<CellId>>,
    pub reverse_cell_identifier_list: Option<Vec<CellId>>,
    pub forward_burst_radio_info: Option<ForwardBurstRadioInfo>,
    pub reverse_burst_radio_info: Option<ReverseBurstRadioInfo>,
    pub is2000_forward_power_control_mode: Option<Is2000ForwardPowerControlMode>,
    pub is2000_fpc_gain_ratio_info: Option<Is2000FpcGainRatioInfo>,
    pub abis_destination_id: Option<AbisDestinationId>,
}

impl BurstCommitMessage {
    /// Encodes the `Abis-Burst Commit` message.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut out = vec![MessageType::BurstCommit.value()];
        if let Some(call_connection_reference) = self.call_connection_reference {
            push_ie(
                &mut out,
                ElementId::CallConnectionReference.value(),
                &call_connection_reference.encode(),
            )?;
        }
        if let Some(correlation_id) = self.correlation_id {
            push_ie(
                &mut out,
                ElementId::CorrelationId.value(),
                &correlation_id.encode(),
            )?;
        }
        if let Some(forward_cell_identifier_list) = &self.forward_cell_identifier_list {
            push_ie(
                &mut out,
                ElementId::CellIdentifierList.value(),
                &encode_cell_identifier_list_allow_empty(forward_cell_identifier_list)?,
            )?;
        }
        if let Some(reverse_cell_identifier_list) = &self.reverse_cell_identifier_list {
            push_ie(
                &mut out,
                ElementId::CellIdentifierList.value(),
                &encode_cell_identifier_list_allow_empty(reverse_cell_identifier_list)?,
            )?;
        }
        if let Some(forward_burst_radio_info) = self.forward_burst_radio_info {
            push_ie(
                &mut out,
                ElementId::ForwardBurstRadioInfo.value(),
                &forward_burst_radio_info.encode()?,
            )?;
        }
        if let Some(reverse_burst_radio_info) = self.reverse_burst_radio_info {
            push_ie(
                &mut out,
                ElementId::ReverseBurstRadioInfo.value(),
                &reverse_burst_radio_info.encode()?,
            )?;
        }
        if let Some(is2000_forward_power_control_mode) = self.is2000_forward_power_control_mode {
            push_ie(
                &mut out,
                ElementId::Is2000PowerControlMode.value(),
                &is2000_forward_power_control_mode.encode()?,
            )?;
        }
        if let Some(is2000_fpc_gain_ratio_info) = &self.is2000_fpc_gain_ratio_info {
            push_ie(
                &mut out,
                ElementId::Is2000FpcGainRatioInfo.value(),
                &is2000_fpc_gain_ratio_info.encode()?,
            )?;
        }
        if let Some(abis_destination_id) = &self.abis_destination_id {
            push_ie(
                &mut out,
                ElementId::AbisDestinationId.value(),
                abis_destination_id.encode(),
            )?;
        }
        Ok(out)
    }

    /// Decodes the `Abis-Burst Commit` message.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let mut cursor = MessageCursor::new(MessageType::BurstCommit, input)?;
        let mut call_connection_reference = None;
        let mut correlation_id = None;
        let mut forward_cell_identifier_list = None;
        let mut reverse_cell_identifier_list = None;
        let mut forward_burst_radio_info = None;
        let mut reverse_burst_radio_info = None;
        let mut is2000_forward_power_control_mode = None;
        let mut is2000_fpc_gain_ratio_info = None;
        let mut abis_destination_id = None;
        while let Some((id, payload)) = cursor.next_ie()? {
            match id {
                x if x == ElementId::CallConnectionReference.value() => {
                    call_connection_reference = Some(CallConnectionReference::decode(payload)?)
                }
                x if x == ElementId::CorrelationId.value() => {
                    correlation_id = Some(CorrelationId::decode(payload)?)
                }
                x if x == ElementId::CellIdentifierList.value() => {
                    if forward_cell_identifier_list.is_none() {
                        forward_cell_identifier_list =
                            Some(decode_cell_identifier_list_allow_empty(payload)?);
                    } else if reverse_cell_identifier_list.is_none() {
                        reverse_cell_identifier_list =
                            Some(decode_cell_identifier_list_allow_empty(payload)?);
                    } else {
                        return Err(Error::DuplicateElement {
                            message_type: MessageType::BurstCommit.value(),
                            id,
                        });
                    }
                }
                x if x == ElementId::ForwardBurstRadioInfo.value() => {
                    forward_burst_radio_info = Some(ForwardBurstRadioInfo::decode(payload)?)
                }
                x if x == ElementId::ReverseBurstRadioInfo.value() => {
                    reverse_burst_radio_info = Some(ReverseBurstRadioInfo::decode(payload)?)
                }
                x if x == ElementId::Is2000PowerControlMode.value() => {
                    is2000_forward_power_control_mode =
                        Some(Is2000ForwardPowerControlMode::decode(payload)?)
                }
                x if x == ElementId::Is2000FpcGainRatioInfo.value() => {
                    is2000_fpc_gain_ratio_info = Some(Is2000FpcGainRatioInfo::decode(payload)?)
                }
                x if x == ElementId::AbisDestinationId.value() => {
                    abis_destination_id = Some(AbisDestinationId::decode(payload)?)
                }
                other => return Err(Error::UnknownInformationElement(other)),
            }
        }
        Ok(Self {
            call_connection_reference,
            correlation_id,
            forward_cell_identifier_list,
            reverse_cell_identifier_list,
            forward_burst_radio_info,
            reverse_burst_radio_info,
            is2000_forward_power_control_mode,
            is2000_fpc_gain_ratio_info,
            abis_destination_id,
        })
    }
}

/// Exact typed `Abis-Connect Ack` message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConnectAckMessage {
    pub call_connection_reference: CallConnectionReference,
    pub correlation_id: Option<CorrelationId>,
    pub connect_ack_information: Vec<A3ConnectAckInformation>,
}

impl ConnectAckMessage {
    /// Encodes the `Abis-Connect Ack` message.
    pub fn encode(&self) -> Result<Vec<u8>> {
        if self.connect_ack_information.is_empty() {
            return Err(Error::InvalidValue {
                context: "Abis-Connect Ack",
                reason: "must contain at least one A3 Connect Ack Information element",
            });
        }
        let mut out = vec![MessageType::ConnectAck.value()];
        push_ie(
            &mut out,
            ElementId::CallConnectionReference.value(),
            &self.call_connection_reference.encode(),
        )?;
        if let Some(correlation_id) = self.correlation_id {
            push_ie(
                &mut out,
                ElementId::CorrelationId.value(),
                &correlation_id.encode(),
            )?;
        }
        for information in &self.connect_ack_information {
            let payload = information.encode()?;
            push_ie(
                &mut out,
                ElementId::A3ConnectAckInformation.value(),
                &payload,
            )?;
        }
        Ok(out)
    }

    /// Decodes the `Abis-Connect Ack` message.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let mut cursor = MessageCursor::new(MessageType::ConnectAck, input)?;
        let mut call_connection_reference = None;
        let mut correlation_id = None;
        let mut connect_ack_information = Vec::new();
        while let Some((id, payload)) = cursor.next_ie()? {
            match id {
                x if x == ElementId::CallConnectionReference.value() => {
                    call_connection_reference = Some(CallConnectionReference::decode(payload)?);
                }
                x if x == ElementId::CorrelationId.value() => {
                    correlation_id = Some(CorrelationId::decode(payload)?);
                }
                x if x == ElementId::A3ConnectAckInformation.value() => {
                    connect_ack_information.push(A3ConnectAckInformation::decode(payload)?);
                }
                other => return Err(Error::UnknownInformationElement(other)),
            }
        }
        let call_connection_reference =
            call_connection_reference.ok_or(Error::MissingRequiredElement {
                message_type: MessageType::ConnectAck.value(),
                id: ElementId::CallConnectionReference.value(),
            })?;
        if connect_ack_information.is_empty() {
            return Err(Error::MissingRequiredElement {
                message_type: MessageType::ConnectAck.value(),
                id: ElementId::A3ConnectAckInformation.value(),
            });
        }
        Ok(Self {
            call_connection_reference,
            correlation_id,
            connect_ack_information,
        })
    }
}

/// Exact typed `Abis-Remove` message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoveMessage {
    pub call_connection_reference: CallConnectionReference,
    pub correlation_id: Option<CorrelationId>,
    pub sdu_id: Option<SduId>,
    pub remove_information: Vec<A3RemoveInformation>,
}

impl RemoveMessage {
    /// Encodes the `Abis-Remove` message.
    pub fn encode(&self) -> Result<Vec<u8>> {
        if self.remove_information.is_empty() {
            return Err(Error::InvalidValue {
                context: "Abis-Remove",
                reason: "must contain at least one A3 Remove Information element",
            });
        }
        let mut out = vec![MessageType::Remove.value()];
        push_ie(
            &mut out,
            ElementId::CallConnectionReference.value(),
            &self.call_connection_reference.encode(),
        )?;
        if let Some(correlation_id) = self.correlation_id {
            push_ie(
                &mut out,
                ElementId::CorrelationId.value(),
                &correlation_id.encode(),
            )?;
        }
        if let Some(sdu_id) = &self.sdu_id {
            push_ie(&mut out, ElementId::SduId.value(), sdu_id.encode())?;
        }
        for information in &self.remove_information {
            let payload = information.encode()?;
            push_ie(&mut out, ElementId::A3RemoveInformation.value(), &payload)?;
        }
        Ok(out)
    }

    /// Decodes the `Abis-Remove` message.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let mut cursor = MessageCursor::new(MessageType::Remove, input)?;
        let mut call_connection_reference = None;
        let mut correlation_id = None;
        let mut sdu_id = None;
        let mut remove_information = Vec::new();
        while let Some((id, payload)) = cursor.next_ie()? {
            match id {
                x if x == ElementId::CallConnectionReference.value() => {
                    call_connection_reference = Some(CallConnectionReference::decode(payload)?);
                }
                x if x == ElementId::CorrelationId.value() => {
                    correlation_id = Some(CorrelationId::decode(payload)?);
                }
                x if x == ElementId::SduId.value() => {
                    sdu_id = Some(SduId::decode(payload)?);
                }
                x if x == ElementId::A3RemoveInformation.value() => {
                    remove_information.push(A3RemoveInformation::decode(payload)?);
                }
                other => return Err(Error::UnknownInformationElement(other)),
            }
        }
        let call_connection_reference =
            call_connection_reference.ok_or(Error::MissingRequiredElement {
                message_type: MessageType::Remove.value(),
                id: ElementId::CallConnectionReference.value(),
            })?;
        if remove_information.is_empty() {
            return Err(Error::MissingRequiredElement {
                message_type: MessageType::Remove.value(),
                id: ElementId::A3RemoveInformation.value(),
            });
        }
        Ok(Self {
            call_connection_reference,
            correlation_id,
            sdu_id,
            remove_information,
        })
    }
}

/// Exact typed `Abis-Remove Ack` message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoveAckMessage {
    pub call_connection_reference: CallConnectionReference,
    pub correlation_id: Option<CorrelationId>,
    pub a3_destination_id: Option<u16>,
}

impl RemoveAckMessage {
    /// Encodes the `Abis-Remove Ack` message.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut out = vec![MessageType::RemoveAck.value()];
        push_ie(
            &mut out,
            ElementId::CallConnectionReference.value(),
            &self.call_connection_reference.encode(),
        )?;
        if let Some(correlation_id) = self.correlation_id {
            push_ie(
                &mut out,
                ElementId::CorrelationId.value(),
                &correlation_id.encode(),
            )?;
        }
        if let Some(a3_destination_id) = self.a3_destination_id {
            push_ie(
                &mut out,
                ElementId::A3DestinationId.value(),
                &a3_destination_id.to_be_bytes(),
            )?;
        }
        Ok(out)
    }

    /// Decodes the `Abis-Remove Ack` message.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let mut cursor = MessageCursor::new(MessageType::RemoveAck, input)?;
        let mut call_connection_reference = None;
        let mut correlation_id = None;
        let mut a3_destination_id = None;
        while let Some((id, payload)) = cursor.next_ie()? {
            match id {
                x if x == ElementId::CallConnectionReference.value() => {
                    call_connection_reference = Some(CallConnectionReference::decode(payload)?);
                }
                x if x == ElementId::CorrelationId.value() => {
                    correlation_id = Some(CorrelationId::decode(payload)?);
                }
                x if x == ElementId::A3DestinationId.value() => {
                    if payload.len() != 2 {
                        return Err(Error::InvalidLength {
                            context: "A3 Destination ID",
                            expected: 2,
                            actual: payload.len(),
                        });
                    }
                    a3_destination_id = Some(u16::from_be_bytes([payload[0], payload[1]]));
                }
                other => return Err(Error::UnknownInformationElement(other)),
            }
        }
        Ok(Self {
            call_connection_reference: call_connection_reference.ok_or(
                Error::MissingRequiredElement {
                    message_type: MessageType::RemoveAck.value(),
                    id: ElementId::CallConnectionReference.value(),
                },
            )?,
            correlation_id,
            a3_destination_id,
        })
    }
}

/// Exact typed `Abis-Traffic Channel Status` message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrafficChannelStatusMessage {
    pub call_connection_reference: CallConnectionReference,
    pub cell_identifier_list: Vec<CellIdWithMscId>,
    pub channel_element_status: ChannelElementStatus,
    pub sdu_id: Option<SduId>,
    pub a3_destination_id: Option<u16>,
    pub a7_destination_id: Option<u16>,
}

impl TrafficChannelStatusMessage {
    /// Encodes the `Abis-Traffic Channel Status` message.
    pub fn encode(&self) -> Result<Vec<u8>> {
        if self.cell_identifier_list.is_empty() {
            return Err(Error::InvalidValue {
                context: "Abis-Traffic Channel Status",
                reason: "must contain at least one cell identifier",
            });
        }
        let mut out = vec![MessageType::TrafficChannelStatus.value()];
        push_ie(
            &mut out,
            ElementId::CallConnectionReference.value(),
            &self.call_connection_reference.encode(),
        )?;
        let mut cell_list = Vec::with_capacity(1 + self.cell_identifier_list.len() * 6);
        for (index, cell) in self.cell_identifier_list.iter().enumerate() {
            let encoded = cell.encode()?;
            if index == 0 {
                cell_list.push(encoded[0]);
            }
            cell_list.extend_from_slice(&encoded[1..]);
        }
        push_ie(&mut out, ElementId::CellIdentifierList.value(), &cell_list)?;
        push_ie(
            &mut out,
            ElementId::ChannelElementStatus.value(),
            &self.channel_element_status.encode(),
        )?;
        if let Some(sdu_id) = &self.sdu_id {
            push_ie(&mut out, ElementId::SduId.value(), sdu_id.encode())?;
        }
        if let Some(a3_destination_id) = self.a3_destination_id {
            push_ie(
                &mut out,
                ElementId::A3DestinationId.value(),
                &a3_destination_id.to_be_bytes(),
            )?;
        }
        if let Some(a7_destination_id) = self.a7_destination_id {
            push_ie(
                &mut out,
                ElementId::A7DestinationId.value(),
                &a7_destination_id.to_be_bytes(),
            )?;
        }
        Ok(out)
    }

    /// Decodes the `Abis-Traffic Channel Status` message.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let mut cursor = MessageCursor::new(MessageType::TrafficChannelStatus, input)?;
        let mut call_connection_reference = None;
        let mut cell_identifier_list = None;
        let mut channel_element_status = None;
        let mut sdu_id = None;
        let mut a3_destination_id = None;
        let mut a7_destination_id = None;
        while let Some((id, payload)) = cursor.next_ie()? {
            match id {
                x if x == ElementId::CallConnectionReference.value() => {
                    call_connection_reference = Some(CallConnectionReference::decode(payload)?);
                }
                x if x == ElementId::CellIdentifierList.value() => {
                    cell_identifier_list = Some(decode_msc_cell_identifier_list(payload)?);
                }
                x if x == ElementId::ChannelElementStatus.value() => {
                    channel_element_status = Some(ChannelElementStatus::decode(payload)?);
                }
                x if x == ElementId::SduId.value() => {
                    sdu_id = Some(SduId::decode(payload)?);
                }
                x if x == ElementId::A3DestinationId.value() => {
                    if payload.len() != 2 {
                        return Err(Error::InvalidLength {
                            context: "A3 Destination ID",
                            expected: 2,
                            actual: payload.len(),
                        });
                    }
                    a3_destination_id = Some(u16::from_be_bytes([payload[0], payload[1]]));
                }
                x if x == ElementId::A7DestinationId.value() => {
                    if payload.len() != 2 {
                        return Err(Error::InvalidLength {
                            context: "A7 Destination ID",
                            expected: 2,
                            actual: payload.len(),
                        });
                    }
                    a7_destination_id = Some(u16::from_be_bytes([payload[0], payload[1]]));
                }
                other => return Err(Error::UnknownInformationElement(other)),
            }
        }
        Ok(Self {
            call_connection_reference: call_connection_reference.ok_or(
                Error::MissingRequiredElement {
                    message_type: MessageType::TrafficChannelStatus.value(),
                    id: ElementId::CallConnectionReference.value(),
                },
            )?,
            cell_identifier_list: cell_identifier_list.ok_or(Error::MissingRequiredElement {
                message_type: MessageType::TrafficChannelStatus.value(),
                id: ElementId::CellIdentifierList.value(),
            })?,
            channel_element_status: channel_element_status.ok_or(
                Error::MissingRequiredElement {
                    message_type: MessageType::TrafficChannelStatus.value(),
                    id: ElementId::ChannelElementStatus.value(),
                },
            )?,
            sdu_id,
            a3_destination_id,
            a7_destination_id,
        })
    }
}

/// Exact typed `Abis-BTS Release Request` message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BtsReleaseRequestMessage {
    pub call_connection_reference: CallConnectionReference,
    pub cause: Option<u8>,
    pub manufacturer_specific_records: Option<ManufacturerSpecificRecords>,
}

/// Exact typed `Abis-BTS Release` message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BtsReleaseMessage {
    pub call_connection_reference: CallConnectionReference,
    pub cell_identifier_list: Option<Vec<CellId>>,
    pub correlation_id: Option<CorrelationId>,
}

impl BtsReleaseMessage {
    /// Encodes the `Abis-BTS Release` message.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut out = vec![MessageType::BtsRelease.value()];
        push_ie(
            &mut out,
            ElementId::CallConnectionReference.value(),
            &self.call_connection_reference.encode(),
        )?;
        if let Some(cell_identifier_list) = &self.cell_identifier_list {
            let mut payload = Vec::new();
            for (index, cell) in cell_identifier_list.iter().enumerate() {
                let encoded = cell.encode()?;
                if index == 0 {
                    payload.push(encoded[0]);
                }
                payload.extend_from_slice(&encoded[1..]);
            }
            push_ie(&mut out, ElementId::CellIdentifierList.value(), &payload)?;
        }
        if let Some(correlation_id) = self.correlation_id {
            push_ie(
                &mut out,
                ElementId::CorrelationId.value(),
                &correlation_id.encode(),
            )?;
        }
        Ok(out)
    }

    /// Decodes the `Abis-BTS Release` message.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let mut cursor = MessageCursor::new(MessageType::BtsRelease, input)?;
        let mut call_connection_reference = None;
        let mut cell_identifier_list = None;
        let mut correlation_id = None;
        while let Some((id, payload)) = cursor.next_ie()? {
            match id {
                x if x == ElementId::CallConnectionReference.value() => {
                    call_connection_reference = Some(CallConnectionReference::decode(payload)?)
                }
                x if x == ElementId::CellIdentifierList.value() => {
                    cell_identifier_list = Some(decode_cell_identifier_list(payload)?)
                }
                x if x == ElementId::CorrelationId.value() => {
                    correlation_id = Some(CorrelationId::decode(payload)?)
                }
                other => return Err(Error::UnknownInformationElement(other)),
            }
        }
        Ok(Self {
            call_connection_reference: call_connection_reference.ok_or(
                Error::MissingRequiredElement {
                    message_type: MessageType::BtsRelease.value(),
                    id: ElementId::CallConnectionReference.value(),
                },
            )?,
            cell_identifier_list,
            correlation_id,
        })
    }
}

/// Exact typed `Abis-BTS Release Ack` message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BtsReleaseAckMessage {
    pub call_connection_reference: CallConnectionReference,
    pub correlation_id: Option<CorrelationId>,
}

impl BtsReleaseAckMessage {
    /// Encodes the `Abis-BTS Release Ack` message.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut out = vec![MessageType::BtsReleaseAck.value()];
        push_ie(
            &mut out,
            ElementId::CallConnectionReference.value(),
            &self.call_connection_reference.encode(),
        )?;
        if let Some(correlation_id) = self.correlation_id {
            push_ie(
                &mut out,
                ElementId::CorrelationId.value(),
                &correlation_id.encode(),
            )?;
        }
        Ok(out)
    }

    /// Decodes the `Abis-BTS Release Ack` message.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let mut cursor = MessageCursor::new(MessageType::BtsReleaseAck, input)?;
        let mut call_connection_reference = None;
        let mut correlation_id = None;
        while let Some((id, payload)) = cursor.next_ie()? {
            match id {
                x if x == ElementId::CallConnectionReference.value() => {
                    call_connection_reference = Some(CallConnectionReference::decode(payload)?)
                }
                x if x == ElementId::CorrelationId.value() => {
                    correlation_id = Some(CorrelationId::decode(payload)?)
                }
                other => return Err(Error::UnknownInformationElement(other)),
            }
        }
        Ok(Self {
            call_connection_reference: call_connection_reference.ok_or(
                Error::MissingRequiredElement {
                    message_type: MessageType::BtsReleaseAck.value(),
                    id: ElementId::CallConnectionReference.value(),
                },
            )?,
            correlation_id,
        })
    }
}

/// Exact payload carried by the Abis `Air Interface Message` information element.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AirInterfaceMessagePayload {
    /// TIA/EIA-IS-2000 message type carried by the IE payload header.
    pub message_type: u8,
    /// Encoded air-interface message bytes.
    pub message: Vec<u8>,
}

impl AirInterfaceMessagePayload {
    /// Creates a validated air-interface payload.
    pub fn new(message_type: u8, message: impl Into<Vec<u8>>) -> Result<Self> {
        let message = message.into();
        if message.is_empty() {
            return Err(Error::InvalidValue {
                context: "Air Interface Message",
                reason: "payload must not be empty",
            });
        }
        Ok(Self {
            message_type,
            message,
        })
    }

    /// Returns the encoded air-interface payload.
    pub fn encode(&self) -> Result<Vec<u8>> {
        if self.message.len() > u8::MAX as usize {
            return Err(Error::InvalidLength {
                context: "Air Interface Message",
                expected: u8::MAX as usize,
                actual: self.message.len(),
            });
        }
        let mut out = Vec::with_capacity(2 + self.message.len());
        out.push(self.message_type);
        out.push(self.message.len() as u8);
        out.extend_from_slice(&self.message);
        Ok(out)
    }

    /// Decodes a validated air-interface payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() < 2 {
            return Err(Error::Truncated {
                context: "Air Interface Message",
                needed: 2,
                actual: input.len(),
            });
        }
        let message_len = input[1] as usize;
        let expected = 2 + message_len;
        if input.len() != expected {
            return Err(Error::InvalidLength {
                context: "Air Interface Message",
                expected,
                actual: input.len(),
            });
        }
        Self::new(input[0], input[2..].to_vec())
    }
}

/// Exact `Layer 2 Ack Request/Results` payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Layer2AckRequestResults {
    /// Whether the layer-2 acknowledgement bit is set.
    pub layer2_ack: bool,
}

impl Layer2AckRequestResults {
    /// Creates the request form used in `Abis-PCH Msg Transfer`.
    pub const fn request() -> Self {
        Self { layer2_ack: true }
    }

    /// Encodes the one-octet payload.
    pub const fn encode(self) -> [u8; 1] {
        [self.layer2_ack as u8]
    }

    /// Decodes the one-octet payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 1 {
            return Err(Error::InvalidLength {
                context: "Layer 2 Ack Request Results",
                expected: 1,
                actual: input.len(),
            });
        }
        if input[0] & 0xfe != 0 {
            return Err(Error::InvalidValue {
                context: "Layer 2 Ack Request Results",
                reason: "reserved bits must be zero",
            });
        }
        Ok(Self {
            layer2_ack: input[0] & 0x01 != 0,
        })
    }
}

/// Marker IE for `Abis Ack Notify`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct AbisAckNotify;

impl AbisAckNotify {
    /// Encodes the zero-length marker payload.
    pub const fn encode(self) -> [u8; 0] {
        []
    }

    /// Decodes the zero-length marker payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if !input.is_empty() {
            return Err(Error::InvalidLength {
                context: "Abis Ack Notify",
                expected: 0,
                actual: input.len(),
            });
        }
        Ok(Self)
    }
}

fn encode_bts_l2_termination(value: bool) -> Result<[u8; 1]> {
    if !value {
        return Err(Error::InvalidValue {
            context: "BTS L2 Termination",
            reason: "shall be set to one in this release",
        });
    }
    Ok([0x01])
}

fn decode_bts_l2_termination(payload: &[u8]) -> Result<bool> {
    if payload.len() != 1 {
        return Err(Error::InvalidLength {
            context: "BTS L2 Termination",
            expected: 1,
            actual: payload.len(),
        });
    }
    if payload[0] != 0x01 {
        return Err(Error::InvalidValue {
            context: "BTS L2 Termination",
            reason: "shall be set to one in this release",
        });
    }
    Ok(true)
}

fn validate_bts_setup_ack_cause(cause: u8) -> Result<u8> {
    if cause != 0x21 {
        return Err(Error::ReservedValue {
            context: "Abis-BTS Setup Ack cause",
            value: cause,
        });
    }
    Ok(cause)
}

fn validate_pch_message_transfer_ack_cause(cause: u8) -> Result<u8> {
    match cause {
        0x07 | 0x20 | 0x71 => Ok(cause),
        other => Err(Error::ReservedValue {
            context: "Abis-PCH Msg Transfer Ack cause",
            value: other,
        }),
    }
}

fn validate_bts_release_request_cause(cause: u8) -> Result<u8> {
    match cause {
        0x07 | 0x10 | 0x20 => Ok(cause),
        other => Err(Error::ReservedValue {
            context: "Abis-BTS Release Request cause",
            value: other,
        }),
    }
}

/// Supported authentication random-number formats.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AuthenticationRandomNumberType {
    Rand = 0x01,
}

impl AuthenticationRandomNumberType {
    /// Decodes the random-number type field.
    pub fn decode(value: u8) -> Result<Self> {
        if value & 0xf0 != 0 {
            return Err(Error::InvalidValue {
                context: "Authentication Challenge Parameter random number type",
                reason: "reserved bits must be zero",
            });
        }
        match value & 0x0f {
            0x01 => Ok(Self::Rand),
            other => Err(Error::ReservedValue {
                context: "Authentication Challenge Parameter random number type",
                value: other,
            }),
        }
    }
}

/// Exact payload carried by `Authentication Challenge Parameter` on Abis access transfer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthenticationChallengeParameter {
    /// Random-number type selector from A.S0003 §7.33.
    pub random_number_type: AuthenticationRandomNumberType,
    /// RAND value carried for authentication/SSD update.
    pub rand_value: [u8; 4],
}

impl AuthenticationChallengeParameter {
    /// Creates a RAND authentication-challenge payload.
    pub const fn new(rand_value: [u8; 4]) -> Self {
        Self {
            random_number_type: AuthenticationRandomNumberType::Rand,
            rand_value,
        }
    }

    /// Returns the encoded authentication-challenge payload.
    pub const fn encode(&self) -> [u8; 5] {
        [
            self.random_number_type as u8,
            self.rand_value[0],
            self.rand_value[1],
            self.rand_value[2],
            self.rand_value[3],
        ]
    }

    /// Decodes a validated authentication-challenge payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() != 5 {
            return Err(Error::InvalidLength {
                context: "Authentication Challenge Parameter",
                expected: 5,
                actual: input.len(),
            });
        }
        Ok(Self {
            random_number_type: AuthenticationRandomNumberType::decode(input[0])?,
            rand_value: [input[1], input[2], input[3], input[4]],
        })
    }
}

/// Exact typed `Abis-PCH Msg Transfer` message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PchMessageTransferMessage {
    pub correlation_id: Option<CorrelationId>,
    pub mobile_identities: Vec<MobileIdentity>,
    pub cell_identifier_list: Option<Vec<CellId>>,
    pub air_interface_message: Option<AirInterfaceMessagePayload>,
    pub layer2_ack_request_results: Option<Layer2AckRequestResults>,
    pub abis_ack_notify: Option<AbisAckNotify>,
}

impl PchMessageTransferMessage {
    /// Encodes the `Abis-PCH Msg Transfer` message.
    pub fn encode(&self) -> Result<Vec<u8>> {
        if self.mobile_identities.len() > 1 {
            return Err(Error::InvalidValue {
                context: "Abis-PCH Msg Transfer",
                reason: "at most one mobile identity may be included",
            });
        }
        if (self.layer2_ack_request_results.is_some() || self.abis_ack_notify.is_some())
            && self.correlation_id.is_none()
        {
            return Err(Error::InvalidValue {
                context: "Abis-PCH Msg Transfer",
                reason: "ack-related IEs require a correlation identifier",
            });
        }
        if self.abis_ack_notify.is_some() && self.layer2_ack_request_results.is_none() {
            return Err(Error::InvalidValue {
                context: "Abis-PCH Msg Transfer",
                reason: "Abis Ack Notify requires Layer 2 Ack Request/Results",
            });
        }
        if let Some(layer2_ack_request_results) = self.layer2_ack_request_results
            && !layer2_ack_request_results.layer2_ack
        {
            return Err(Error::InvalidValue {
                context: "Abis-PCH Msg Transfer",
                reason: "Layer 2 Ack Request/Results must request acknowledgement when present",
            });
        }
        let mut out = vec![MessageType::PchMessageTransfer.value()];
        if let Some(correlation_id) = self.correlation_id {
            push_ie(
                &mut out,
                ElementId::CorrelationId.value(),
                &correlation_id.encode(),
            )?;
        }
        for mobile_identity in &self.mobile_identities {
            push_ie(
                &mut out,
                ElementId::MobileIdentity.value(),
                &mobile_identity.encode()?,
            )?;
        }
        if let Some(cell_identifier_list) = &self.cell_identifier_list {
            push_ie(
                &mut out,
                ElementId::CellIdentifierList.value(),
                &encode_cell_identifier_list(cell_identifier_list)?,
            )?;
        }
        if let Some(air_interface_message) = &self.air_interface_message {
            push_ie(
                &mut out,
                ElementId::AirInterfaceMessage.value(),
                &air_interface_message.encode()?,
            )?;
        }
        if let Some(layer2_ack_request_results) = self.layer2_ack_request_results {
            push_ie(
                &mut out,
                ElementId::Layer2AckRequestResults.value(),
                &layer2_ack_request_results.encode(),
            )?;
        }
        if let Some(abis_ack_notify) = self.abis_ack_notify {
            push_ie(
                &mut out,
                ElementId::AbisAckNotify.value(),
                &abis_ack_notify.encode(),
            )?;
        }
        Ok(out)
    }

    /// Decodes the `Abis-PCH Msg Transfer` message.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let mut cursor = MessageCursor::new(MessageType::PchMessageTransfer, input)?;
        let mut correlation_id = None;
        let mut mobile_identities = Vec::new();
        let mut cell_identifier_list = None;
        let mut air_interface_message = None;
        let mut layer2_ack_request_results = None;
        let mut abis_ack_notify = None;
        while let Some((id, payload)) = cursor.next_ie()? {
            match id {
                x if x == ElementId::CorrelationId.value() => {
                    correlation_id = Some(CorrelationId::decode(payload)?)
                }
                x if x == ElementId::MobileIdentity.value() => {
                    mobile_identities.push(MobileIdentity::decode(payload)?)
                }
                x if x == ElementId::CellIdentifierList.value() => {
                    cell_identifier_list = Some(decode_cell_identifier_list(payload)?)
                }
                x if x == ElementId::AirInterfaceMessage.value() => {
                    air_interface_message = Some(AirInterfaceMessagePayload::decode(payload)?)
                }
                x if x == ElementId::Layer2AckRequestResults.value() => {
                    layer2_ack_request_results = Some(Layer2AckRequestResults::decode(payload)?)
                }
                x if x == ElementId::AbisAckNotify.value() => {
                    abis_ack_notify = Some(AbisAckNotify::decode(payload)?)
                }
                other => return Err(Error::UnknownInformationElement(other)),
            }
        }
        if (layer2_ack_request_results.is_some() || abis_ack_notify.is_some())
            && correlation_id.is_none()
        {
            return Err(Error::InvalidValue {
                context: "Abis-PCH Msg Transfer",
                reason: "ack-related IEs require a correlation identifier",
            });
        }
        if mobile_identities.len() > 1 {
            return Err(Error::InvalidValue {
                context: "Abis-PCH Msg Transfer",
                reason: "at most one mobile identity may be included",
            });
        }
        if abis_ack_notify.is_some() && layer2_ack_request_results.is_none() {
            return Err(Error::InvalidValue {
                context: "Abis-PCH Msg Transfer",
                reason: "Abis Ack Notify requires Layer 2 Ack Request/Results",
            });
        }
        if let Some(layer2_ack_request_results) = layer2_ack_request_results
            && !layer2_ack_request_results.layer2_ack
        {
            return Err(Error::InvalidValue {
                context: "Abis-PCH Msg Transfer",
                reason: "Layer 2 Ack Request/Results must request acknowledgement when present",
            });
        }
        Ok(Self {
            correlation_id,
            mobile_identities,
            cell_identifier_list,
            air_interface_message,
            layer2_ack_request_results,
            abis_ack_notify,
        })
    }
}

/// Exact typed `Abis-ACH Msg Transfer` message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AchMessageTransferMessage {
    pub correlation_id: Option<CorrelationId>,
    pub mobile_identities: Vec<MobileIdentity>,
    pub cell_identifier: Option<CellId>,
    pub bts_l2_termination: Option<bool>,
    pub air_interface_message: Option<AirInterfaceMessagePayload>,
    pub cdma_serving_one_way_delay: CdmaServingOneWayDelay,
    pub authentication_challenge_parameter: Option<AuthenticationChallengeParameter>,
}

impl AchMessageTransferMessage {
    /// Encodes the `Abis-ACH Msg Transfer` message.
    pub fn encode(&self) -> Result<Vec<u8>> {
        if self.authentication_challenge_parameter.is_some() && self.mobile_identities.is_empty() {
            return Err(Error::InvalidValue {
                context: "Abis-ACH Msg Transfer",
                reason: "authentication challenge requires at least one mobile identity",
            });
        }
        let mut out = vec![MessageType::AchMessageTransfer.value()];
        if let Some(correlation_id) = self.correlation_id {
            push_ie(
                &mut out,
                ElementId::CorrelationId.value(),
                &correlation_id.encode(),
            )?;
        }
        for mobile_identity in &self.mobile_identities {
            push_ie(
                &mut out,
                ElementId::MobileIdentity.value(),
                &mobile_identity.encode()?,
            )?;
        }
        if let Some(cell_identifier) = self.cell_identifier {
            push_ie(
                &mut out,
                ElementId::CellIdentifier.value(),
                &cell_identifier.encode()?,
            )?;
        }
        if let Some(bts_l2_termination) = self.bts_l2_termination {
            push_ie(
                &mut out,
                ElementId::BtsL2Termination.value(),
                &encode_bts_l2_termination(bts_l2_termination)?,
            )?;
        }
        if let Some(air_interface_message) = &self.air_interface_message {
            push_ie(
                &mut out,
                ElementId::AirInterfaceMessage.value(),
                &air_interface_message.encode()?,
            )?;
        }
        push_ie(
            &mut out,
            ElementId::CdmaServingOneWayDelay.value(),
            &self.cdma_serving_one_way_delay.encode()?,
        )?;
        if let Some(authentication_challenge_parameter) = &self.authentication_challenge_parameter {
            push_ie(
                &mut out,
                ElementId::AuthenticationChallengeParameter.value(),
                &authentication_challenge_parameter.encode(),
            )?;
        }
        Ok(out)
    }

    /// Decodes the `Abis-ACH Msg Transfer` message.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let mut cursor = MessageCursor::new(MessageType::AchMessageTransfer, input)?;
        let mut correlation_id = None;
        let mut mobile_identities = Vec::new();
        let mut cell_identifier = None;
        let mut bts_l2_termination = None;
        let mut air_interface_message = None;
        let mut cdma_serving_one_way_delay = None;
        let mut authentication_challenge_parameter = None;
        while let Some((id, payload)) = cursor.next_ie()? {
            match id {
                x if x == ElementId::CorrelationId.value() => {
                    correlation_id = Some(CorrelationId::decode(payload)?)
                }
                x if x == ElementId::MobileIdentity.value() => {
                    mobile_identities.push(MobileIdentity::decode(payload)?)
                }
                x if x == ElementId::CellIdentifier.value() => {
                    cell_identifier = Some(CellId::decode(payload)?)
                }
                x if x == ElementId::BtsL2Termination.value() => {
                    bts_l2_termination = Some(decode_bts_l2_termination(payload)?);
                }
                x if x == ElementId::AirInterfaceMessage.value() => {
                    air_interface_message = Some(AirInterfaceMessagePayload::decode(payload)?)
                }
                x if x == ElementId::CdmaServingOneWayDelay.value() => {
                    cdma_serving_one_way_delay = Some(CdmaServingOneWayDelay::decode(payload)?)
                }
                x if x == ElementId::AuthenticationChallengeParameter.value() => {
                    authentication_challenge_parameter =
                        Some(AuthenticationChallengeParameter::decode(payload)?)
                }
                other => return Err(Error::UnknownInformationElement(other)),
            }
        }
        if authentication_challenge_parameter.is_some() && mobile_identities.is_empty() {
            return Err(Error::InvalidValue {
                context: "Abis-ACH Msg Transfer",
                reason: "authentication challenge requires at least one mobile identity",
            });
        }
        Ok(Self {
            correlation_id,
            mobile_identities,
            cell_identifier,
            bts_l2_termination,
            air_interface_message,
            cdma_serving_one_way_delay: cdma_serving_one_way_delay.ok_or(
                Error::MissingRequiredElement {
                    message_type: MessageType::AchMessageTransfer.value(),
                    id: ElementId::CdmaServingOneWayDelay.value(),
                },
            )?,
            authentication_challenge_parameter,
        })
    }
}

/// Exact typed `Abis-PCH Msg Transfer Ack` message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PchMessageTransferAckMessage {
    pub correlation_id: Option<CorrelationId>,
    pub cause: Option<u8>,
    pub bts_l2_termination: Option<bool>,
}

impl PchMessageTransferAckMessage {
    /// Encodes the `Abis-PCH Msg Transfer Ack` message.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut out = vec![MessageType::PchMessageTransferAck.value()];
        if let Some(correlation_id) = self.correlation_id {
            push_ie(
                &mut out,
                ElementId::CorrelationId.value(),
                &correlation_id.encode(),
            )?;
        }
        if let Some(cause) = self.cause {
            push_ie(
                &mut out,
                ElementId::Cause.value(),
                &[validate_pch_message_transfer_ack_cause(cause)?],
            )?;
        }
        if let Some(bts_l2_termination) = self.bts_l2_termination {
            push_ie(
                &mut out,
                ElementId::BtsL2Termination.value(),
                &encode_bts_l2_termination(bts_l2_termination)?,
            )?;
        }
        Ok(out)
    }

    /// Decodes the `Abis-PCH Msg Transfer Ack` message.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let mut cursor = MessageCursor::new(MessageType::PchMessageTransferAck, input)?;
        let mut correlation_id = None;
        let mut cause = None;
        let mut bts_l2_termination = None;
        while let Some((id, payload)) = cursor.next_ie()? {
            match id {
                x if x == ElementId::CorrelationId.value() => {
                    correlation_id = Some(CorrelationId::decode(payload)?)
                }
                x if x == ElementId::Cause.value() => {
                    if payload.len() != 1 {
                        return Err(Error::InvalidLength {
                            context: "Cause",
                            expected: 1,
                            actual: payload.len(),
                        });
                    }
                    cause = Some(validate_pch_message_transfer_ack_cause(payload[0])?);
                }
                x if x == ElementId::BtsL2Termination.value() => {
                    bts_l2_termination = Some(decode_bts_l2_termination(payload)?);
                }
                other => return Err(Error::UnknownInformationElement(other)),
            }
        }
        Ok(Self {
            correlation_id,
            cause,
            bts_l2_termination,
        })
    }
}

/// Exact typed `Abis-PACA Update` message.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PacaUpdateMessage {
    pub call_connection_reference: CallConnectionReference,
    pub mobile_identity_imsi: Option<MobileIdentity>,
    pub action_required: Option<PacaActionRequired>,
}

impl PacaUpdateMessage {
    /// Encodes the `Abis-PACA Update` message.
    pub fn encode(&self) -> Result<Vec<u8>> {
        if matches!(self.mobile_identity_imsi, Some(MobileIdentity::Esn(_))) {
            return Err(Error::InvalidValue {
                context: "Abis-PACA Update",
                reason: "only IMSI mobile identity is allowed",
            });
        }
        let mut out = vec![MessageType::PacaUpdate.value()];
        push_ie(
            &mut out,
            ElementId::CallConnectionReference.value(),
            &self.call_connection_reference.encode(),
        )?;
        if let Some(mobile_identity_imsi) = &self.mobile_identity_imsi {
            push_ie(
                &mut out,
                ElementId::MobileIdentity.value(),
                &mobile_identity_imsi.encode()?,
            )?;
        }
        if let Some(action_required) = self.action_required {
            push_ie(
                &mut out,
                ElementId::PacaOrder.value(),
                &[0x00, action_required.encode()],
            )?;
        }
        Ok(out)
    }

    /// Decodes the `Abis-PACA Update` message.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let mut cursor = MessageCursor::new(MessageType::PacaUpdate, input)?;
        let mut call_connection_reference = None;
        let mut mobile_identity_imsi = None;
        let mut action_required = None;
        while let Some((id, payload)) = cursor.next_ie()? {
            match id {
                x if x == ElementId::CallConnectionReference.value() => {
                    call_connection_reference = Some(CallConnectionReference::decode(payload)?)
                }
                x if x == ElementId::MobileIdentity.value() => {
                    let identity = MobileIdentity::decode(payload)?;
                    if matches!(identity, MobileIdentity::Esn(_)) {
                        return Err(Error::InvalidValue {
                            context: "Abis-PACA Update",
                            reason: "only IMSI mobile identity is allowed",
                        });
                    }
                    mobile_identity_imsi = Some(identity)
                }
                x if x == ElementId::PacaOrder.value() => {
                    if payload.len() != 2 {
                        return Err(Error::InvalidLength {
                            context: "PACA Order",
                            expected: 2,
                            actual: payload.len(),
                        });
                    }
                    if payload[0] != 0x00 || payload[1] & 0xf8 != 0 {
                        return Err(Error::InvalidValue {
                            context: "PACA Order",
                            reason: "reserved bits must be zero",
                        });
                    }
                    action_required = match payload[1] & 0x07 {
                        0b001 => Some(PacaActionRequired::UpdateQueuePosition),
                        0b011 => Some(PacaActionRequired::RemoveMsFromQueue),
                        other => {
                            return Err(Error::ReservedValue {
                                context: "PACA action required",
                                value: other,
                            });
                        }
                    };
                }
                other => return Err(Error::UnknownInformationElement(other)),
            }
        }
        Ok(Self {
            call_connection_reference: call_connection_reference.ok_or(
                Error::MissingRequiredElement {
                    message_type: MessageType::PacaUpdate.value(),
                    id: ElementId::CallConnectionReference.value(),
                },
            )?,
            mobile_identity_imsi,
            action_required,
        })
    }
}

impl BtsReleaseRequestMessage {
    /// Encodes the `Abis-BTS Release Request` message.
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut out = vec![MessageType::BtsReleaseRequest.value()];
        push_ie(
            &mut out,
            ElementId::CallConnectionReference.value(),
            &self.call_connection_reference.encode(),
        )?;
        if let Some(cause) = self.cause {
            push_ie(
                &mut out,
                ElementId::Cause.value(),
                &[validate_bts_release_request_cause(cause)?],
            )?;
        }
        if let Some(records) = &self.manufacturer_specific_records {
            let payload = records.encode();
            push_ie(
                &mut out,
                ElementId::ManufacturerSpecificRecords.value(),
                &payload,
            )?;
        }
        Ok(out)
    }

    /// Decodes the `Abis-BTS Release Request` message.
    pub fn decode(input: &[u8]) -> Result<Self> {
        let mut cursor = MessageCursor::new(MessageType::BtsReleaseRequest, input)?;
        let mut call_connection_reference = None;
        let mut cause = None;
        let mut manufacturer_specific_records = None;
        while let Some((id, payload)) = cursor.next_ie()? {
            match id {
                x if x == ElementId::CallConnectionReference.value() => {
                    call_connection_reference = Some(CallConnectionReference::decode(payload)?);
                }
                x if x == ElementId::Cause.value() => {
                    if payload.len() != 1 {
                        return Err(Error::InvalidLength {
                            context: "Cause",
                            expected: 1,
                            actual: payload.len(),
                        });
                    }
                    cause = Some(validate_bts_release_request_cause(payload[0])?);
                }
                x if x == ElementId::ManufacturerSpecificRecords.value() => {
                    manufacturer_specific_records =
                        Some(ManufacturerSpecificRecords::decode(payload)?);
                }
                other => return Err(Error::UnknownInformationElement(other)),
            }
        }
        Ok(Self {
            call_connection_reference: call_connection_reference.ok_or(
                Error::MissingRequiredElement {
                    message_type: MessageType::BtsReleaseRequest.value(),
                    id: ElementId::CallConnectionReference.value(),
                },
            )?,
            cause,
            manufacturer_specific_records,
        })
    }
}

pub(crate) fn validate_message_semantics(
    message_type: MessageType,
    elements: &[super::ies::InformationElement],
) -> Result<()> {
    let mut encoded = Vec::with_capacity(1 + elements.len() * 3);
    encoded.push(message_type.value());
    for element in elements {
        element.encode(&mut encoded)?;
    }
    match message_type {
        MessageType::Connect => {
            ConnectMessage::decode(&encoded)?;
        }
        MessageType::ConnectAck => {
            ConnectAckMessage::decode(&encoded)?;
        }
        MessageType::Remove => {
            RemoveMessage::decode(&encoded)?;
        }
        MessageType::RemoveAck => {
            RemoveAckMessage::decode(&encoded)?;
        }
        MessageType::TrafficChannelStatus => {
            TrafficChannelStatusMessage::decode(&encoded)?;
        }
        MessageType::PacaUpdate => {
            PacaUpdateMessage::decode(&encoded)?;
        }
        MessageType::BtsSetup => {
            BtsSetupMessage::decode(&encoded)?;
        }
        MessageType::BtsSetupAck => {
            BtsSetupAckMessage::decode(&encoded)?;
        }
        MessageType::BtsRelease => {
            BtsReleaseMessage::decode(&encoded)?;
        }
        MessageType::BtsReleaseAck => {
            BtsReleaseAckMessage::decode(&encoded)?;
        }
        MessageType::BtsReleaseRequest => {
            BtsReleaseRequestMessage::decode(&encoded)?;
        }
        MessageType::PchMessageTransfer => {
            PchMessageTransferMessage::decode(&encoded)?;
        }
        MessageType::PchMessageTransferAck => {
            PchMessageTransferAckMessage::decode(&encoded)?;
        }
        MessageType::AchMessageTransfer => {
            AchMessageTransferMessage::decode(&encoded)?;
        }
        MessageType::BurstRequest => {
            BurstRequestMessage::decode(&encoded)?;
        }
        MessageType::BurstResponse => {
            BurstResponseMessage::decode(&encoded)?;
        }
        MessageType::BurstCommit => {
            BurstCommitMessage::decode(&encoded)?;
        }
        MessageType::FchForward
        | MessageType::FchReverse
        | MessageType::DcchForward
        | MessageType::DcchReverse
        | MessageType::SchForward
        | MessageType::SchReverse => {
            // Bearer channel types — encoded/decoded via cdma_abis::bearer, not
            // as Abis control messages.  No typed-message validation needed.
        }
    }
    Ok(())
}

fn push_ie(out: &mut Vec<u8>, id: u8, payload: &[u8]) -> Result<()> {
    if id == ElementId::ServiceOption.value() {
        if payload.len() != 2 {
            return Err(Error::InvalidLength {
                context: "Abis control IE payload",
                expected: 2,
                actual: payload.len(),
            });
        }
        out.push(id);
        out.extend_from_slice(payload);
        return Ok(());
    }
    if payload.len() > u8::MAX as usize {
        return Err(Error::InvalidLength {
            context: "Abis control IE payload",
            expected: u8::MAX as usize,
            actual: payload.len(),
        });
    }
    out.push(id);
    out.push(payload.len() as u8);
    out.extend_from_slice(payload);
    Ok(())
}

fn encode_cell_identifier_list(cells: &[CellId]) -> Result<Vec<u8>> {
    if cells.is_empty() {
        return Err(Error::InvalidValue {
            context: "Cell Identifier List",
            reason: "must contain at least one cell identifier",
        });
    }
    encode_cell_identifier_list_allow_empty(cells)
}

fn encode_cell_identifier_list_allow_empty(cells: &[CellId]) -> Result<Vec<u8>> {
    if cells.len() > (u8::MAX as usize).saturating_sub(1) / 2 {
        return Err(Error::InvalidLength {
            context: "Cell Identifier List",
            expected: (u8::MAX as usize).saturating_sub(1) / 2,
            actual: cells.len(),
        });
    }
    let mut out = Vec::with_capacity(1 + cells.len() * 2);
    if cells.is_empty() {
        return Ok(out);
    }
    for (index, cell) in cells.iter().enumerate() {
        let encoded = cell.encode()?;
        if index == 0 {
            out.push(encoded[0]);
        }
        out.extend_from_slice(&encoded[1..]);
    }
    Ok(out)
}

fn decode_msc_cell_identifier_list(input: &[u8]) -> Result<Vec<CellIdWithMscId>> {
    if input.is_empty() {
        return Err(Error::InvalidLength {
            context: "MSC Cell Identifier List",
            expected: 1,
            actual: 0,
        });
    }
    if input[0] != 0x07 || !(input.len() - 1).is_multiple_of(5) {
        return Err(Error::InvalidValue {
            context: "MSC Cell Identifier List",
            reason: "unexpected discriminator or element length",
        });
    }
    let mut cells = Vec::new();
    let mut offset = 1;
    while offset < input.len() {
        let mut payload = [0u8; 6];
        payload[0] = 0x07;
        payload[1..].copy_from_slice(&input[offset..offset + 5]);
        cells.push(CellIdWithMscId::decode(&payload)?);
        offset += 5;
    }
    Ok(cells)
}

fn decode_cell_identifier_list(input: &[u8]) -> Result<Vec<CellId>> {
    if input.is_empty() {
        return Err(Error::InvalidLength {
            context: "Cell Identifier List",
            expected: 1,
            actual: 0,
        });
    }
    if input[0] != 0x02 || !(input.len() - 1).is_multiple_of(2) {
        return Err(Error::InvalidValue {
            context: "Cell Identifier List",
            reason: "unexpected discriminator or element length",
        });
    }
    let mut cells = Vec::new();
    let mut offset = 1;
    while offset < input.len() {
        let payload = [0x02, input[offset], input[offset + 1]];
        cells.push(CellId::decode(&payload)?);
        offset += 2;
    }
    Ok(cells)
}

fn decode_cell_identifier_list_allow_empty(input: &[u8]) -> Result<Vec<CellId>> {
    if input.is_empty() {
        return Ok(Vec::new());
    }
    decode_cell_identifier_list(input)
}

fn encode_imsi(imsi: &str) -> Result<Vec<u8>> {
    let digits: Vec<u8> = imsi
        .chars()
        .map(|c| c.to_digit(10).map(|d| d as u8))
        .collect::<Option<Vec<_>>>()
        .ok_or(Error::InvalidValue {
            context: "IMSI Mobile Identity",
            reason: "IMSI must contain digits only",
        })?;
    if !(10..=15).contains(&digits.len()) {
        return Err(Error::InvalidValue {
            context: "IMSI Mobile Identity",
            reason: "IMSI must contain 10 to 15 digits",
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
    digits.push(char::from(b'0' + ((first >> 4) & 0x0f)));
    for (index, byte) in rest.iter().copied().enumerate() {
        let low = byte & 0x0f;
        let high = byte >> 4;
        digits.push(char::from(b'0' + low));
        let is_last_high_nibble = index == rest.len() - 1;
        if !(is_last_high_nibble && !odd && high == 0x0f) {
            digits.push(char::from(b'0' + high));
        }
    }
    Ok(MobileIdentity::Imsi(digits))
}

struct MessageCursor<'a> {
    message_type: MessageType,
    input: &'a [u8],
    offset: usize,
}

impl<'a> MessageCursor<'a> {
    fn new(message_type: MessageType, input: &'a [u8]) -> Result<Self> {
        let Some((&actual, rest)) = input.split_first() else {
            return Err(Error::EmptyMessage);
        };
        if actual != message_type.value() {
            return Err(Error::InvalidMessage {
                message_type: actual,
                reason: "unexpected message type for typed decoder",
            });
        }
        Ok(Self {
            message_type,
            input: rest,
            offset: 0,
        })
    }

    fn next_ie(&mut self) -> Result<Option<(u8, &'a [u8])>> {
        if self.offset == self.input.len() {
            return Ok(None);
        }
        let id = self.input[self.offset];
        if id == ElementId::ServiceOption.value() {
            let value_start = self.offset + 1;
            let value_end = value_start + 2;
            if self.input.len() < value_end {
                return Err(Error::Truncated {
                    context: "typed Abis IE value",
                    needed: value_end,
                    actual: self.input.len(),
                });
            }
            self.offset = value_end;
            let _ = self.message_type;
            return Ok(Some((id, &self.input[value_start..value_end])));
        }
        if self.input.len() - self.offset < 2 {
            return Err(Error::Truncated {
                context: "typed Abis IE header",
                needed: self.offset + 2,
                actual: self.input.len(),
            });
        }
        let len = self.input[self.offset + 1] as usize;
        let value_start = self.offset + 2;
        let value_end = value_start + len;
        if self.input.len() < value_end {
            return Err(Error::Truncated {
                context: "typed Abis IE value",
                needed: value_end,
                actual: self.input.len(),
            });
        }
        self.offset = value_end;
        let _ = self.message_type;
        Ok(Some((id, &self.input[value_start..value_end])))
    }
}
