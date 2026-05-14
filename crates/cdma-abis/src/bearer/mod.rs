//! Inherited A3 traffic-frame payloads used on Abis bearer channels.

use crate::control::MessageType;
use crate::{Error, Result};

/// Supported bearer channel families carried over the Abis UDP wrapper.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum ChannelFamily {
    Fch,
    Sch,
    Dcch,
}

/// Traffic direction for a bearer frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Direction {
    Forward,
    Reverse,
}

/// IS-2001 §6.2.2.75, Tables 6-76 through 6-81, IS-2000 Frame Content.
///
/// The wire field is an opaque one-octet index. The enum names include the
/// channel family/table and enough rate/RC information to avoid reusing fake
/// values for local meanings. In particular, `0x10` is a real FCH content
/// value (7200 bps for forward RC5/8/9 and reverse RC4/6), not a signaling
/// marker.
#[allow(non_camel_case_types)]
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[repr(u8)]
pub enum FrameContent {
    Idle = 0x00,
    FchRc1_9600 = 0x01,
    FchRc1_4800 = 0x02,
    FchRc1_2400 = 0x03,
    FchRc1_1200 = 0x04,
    FchRc2_14400 = 0x05,
    FchRc2_7200 = 0x06,
    FchRc2_3600 = 0x07,
    FchRc2_1800 = 0x08,
    FchRc3Forward5ms_9600 = 0x09,
    FchRc3_9600 = 0x0A,
    FchRc3_4800 = 0x0B,
    FchRc3_2700 = 0x0C,
    FchRc3_1500 = 0x0D,
    FchRc5Forward5ms_9600 = 0x0E,
    FchRc5_14400 = 0x0F,
    FchRc5_7200 = 0x10,
    FchRc5_3600 = 0x11,
    FchRc5_1800 = 0x12,
    DcchRc3_9600 = 0x20,
    DcchRc5_14400 = 0x21,
    DcchRc5Forward5ms_9600 = 0x22,
    Sch20msRc5_614400 = 0x30,
    Sch20msRc3_307200 = 0x31,
    Sch20msRc3_153600 = 0x32,
    Sch20msRc3_76800 = 0x33,
    Sch20msRc3_38400 = 0x34,
    Sch20msRc3_19200 = 0x35,
    Sch20msRc3_9600 = 0x36,
    Sch20msRc3_4800 = 0x37,
    Sch20msRc3_2700 = 0x38,
    Sch20msRc3_1500 = 0x39,
    Sch20msRc6_1036800 = 0x3A,
    Sch20msRc4_460800 = 0x3B,
    Sch20msRc4_230400 = 0x3C,
    Sch20msRc4_115200 = 0x3D,
    Sch20msRc4_57600 = 0x3E,
    Sch20msRc4_28800 = 0x3F,
    Sch20msRc4_14400 = 0x40,
    Sch20msRc4_7200 = 0x41,
    Sch20msRc4_3600 = 0x42,
    Sch20msRc4_1800 = 0x43,
    Sch40msRc5_307200 = 0x50,
    Sch40msRc3_153600 = 0x51,
    Sch40msRc3_76800 = 0x52,
    Sch40msRc3_38400 = 0x53,
    Sch40msRc3_19200 = 0x54,
    Sch40msRc3_9600 = 0x55,
    Sch40msRc3_4800 = 0x56,
    Sch40msRc3_2400 = 0x57,
    Sch40msRc3_1350 = 0x58,
    Sch40msRc6_518400 = 0x59,
    Sch40msRc4_230400 = 0x5A,
    Sch40msRc4_115200 = 0x5B,
    Sch40msRc4_57600 = 0x5C,
    Sch40msRc4_28800 = 0x5D,
    Sch40msRc4_14400 = 0x5E,
    Sch40msRc4_7200 = 0x5F,
    Sch40msRc4_3600 = 0x60,
    Sch40msRc4_1800 = 0x61,
    Sch80msRc5_153600 = 0x62,
    Sch80msRc3_76800 = 0x63,
    Sch80msRc3_38400 = 0x64,
    Sch80msRc3_19200 = 0x65,
    Sch80msRc3_9600 = 0x66,
    Sch80msRc3_4800 = 0x67,
    Sch80msRc3_2400 = 0x68,
    Sch80msRc3_1200 = 0x69,
    Sch80msRc6_259200 = 0x6A,
    Sch80msRc4_115200 = 0x6B,
    Sch80msRc4_57600 = 0x6C,
    Sch80msRc4_28800 = 0x6D,
    Sch80msRc4_14400 = 0x6E,
    Sch80msRc4_7200 = 0x6F,
    Sch80msRc4_3600 = 0x70,
    Sch80msRc4_1800 = 0x71,
    FullRateLikely = 0x7D,
    Erasure = 0x7E,
    Null = 0x7F,
}

impl FrameContent {
    pub fn value(self) -> u8 {
        self as u8
    }

    pub fn from_u8(value: u8) -> Option<Self> {
        Some(match value {
            0x00 => Self::Idle,
            0x01 => Self::FchRc1_9600,
            0x02 => Self::FchRc1_4800,
            0x03 => Self::FchRc1_2400,
            0x04 => Self::FchRc1_1200,
            0x05 => Self::FchRc2_14400,
            0x06 => Self::FchRc2_7200,
            0x07 => Self::FchRc2_3600,
            0x08 => Self::FchRc2_1800,
            0x09 => Self::FchRc3Forward5ms_9600,
            0x0A => Self::FchRc3_9600,
            0x0B => Self::FchRc3_4800,
            0x0C => Self::FchRc3_2700,
            0x0D => Self::FchRc3_1500,
            0x0E => Self::FchRc5Forward5ms_9600,
            0x0F => Self::FchRc5_14400,
            0x10 => Self::FchRc5_7200,
            0x11 => Self::FchRc5_3600,
            0x12 => Self::FchRc5_1800,
            0x20 => Self::DcchRc3_9600,
            0x21 => Self::DcchRc5_14400,
            0x22 => Self::DcchRc5Forward5ms_9600,
            0x30 => Self::Sch20msRc5_614400,
            0x31 => Self::Sch20msRc3_307200,
            0x32 => Self::Sch20msRc3_153600,
            0x33 => Self::Sch20msRc3_76800,
            0x34 => Self::Sch20msRc3_38400,
            0x35 => Self::Sch20msRc3_19200,
            0x36 => Self::Sch20msRc3_9600,
            0x37 => Self::Sch20msRc3_4800,
            0x38 => Self::Sch20msRc3_2700,
            0x39 => Self::Sch20msRc3_1500,
            0x3A => Self::Sch20msRc6_1036800,
            0x3B => Self::Sch20msRc4_460800,
            0x3C => Self::Sch20msRc4_230400,
            0x3D => Self::Sch20msRc4_115200,
            0x3E => Self::Sch20msRc4_57600,
            0x3F => Self::Sch20msRc4_28800,
            0x40 => Self::Sch20msRc4_14400,
            0x41 => Self::Sch20msRc4_7200,
            0x42 => Self::Sch20msRc4_3600,
            0x43 => Self::Sch20msRc4_1800,
            0x50 => Self::Sch40msRc5_307200,
            0x51 => Self::Sch40msRc3_153600,
            0x52 => Self::Sch40msRc3_76800,
            0x53 => Self::Sch40msRc3_38400,
            0x54 => Self::Sch40msRc3_19200,
            0x55 => Self::Sch40msRc3_9600,
            0x56 => Self::Sch40msRc3_4800,
            0x57 => Self::Sch40msRc3_2400,
            0x58 => Self::Sch40msRc3_1350,
            0x59 => Self::Sch40msRc6_518400,
            0x5A => Self::Sch40msRc4_230400,
            0x5B => Self::Sch40msRc4_115200,
            0x5C => Self::Sch40msRc4_57600,
            0x5D => Self::Sch40msRc4_28800,
            0x5E => Self::Sch40msRc4_14400,
            0x5F => Self::Sch40msRc4_7200,
            0x60 => Self::Sch40msRc4_3600,
            0x61 => Self::Sch40msRc4_1800,
            0x62 => Self::Sch80msRc5_153600,
            0x63 => Self::Sch80msRc3_76800,
            0x64 => Self::Sch80msRc3_38400,
            0x65 => Self::Sch80msRc3_19200,
            0x66 => Self::Sch80msRc3_9600,
            0x67 => Self::Sch80msRc3_4800,
            0x68 => Self::Sch80msRc3_2400,
            0x69 => Self::Sch80msRc3_1200,
            0x6A => Self::Sch80msRc6_259200,
            0x6B => Self::Sch80msRc4_115200,
            0x6C => Self::Sch80msRc4_57600,
            0x6D => Self::Sch80msRc4_28800,
            0x6E => Self::Sch80msRc4_14400,
            0x6F => Self::Sch80msRc4_7200,
            0x70 => Self::Sch80msRc4_3600,
            0x71 => Self::Sch80msRc4_1800,
            0x7D => Self::FullRateLikely,
            0x7E => Self::Erasure,
            0x7F => Self::Null,
            _ => return None,
        })
    }

    pub fn rate_bps(self) -> Option<u32> {
        Some(match self {
            Self::FchRc1_9600
            | Self::FchRc3Forward5ms_9600
            | Self::FchRc3_9600
            | Self::FchRc5Forward5ms_9600
            | Self::DcchRc3_9600
            | Self::DcchRc5Forward5ms_9600
            | Self::Sch20msRc3_9600
            | Self::Sch40msRc3_9600
            | Self::Sch80msRc3_9600 => 9_600,
            Self::FchRc1_4800
            | Self::FchRc3_4800
            | Self::Sch20msRc3_4800
            | Self::Sch40msRc3_4800
            | Self::Sch80msRc3_4800 => 4_800,
            Self::FchRc1_2400 | Self::Sch40msRc3_2400 | Self::Sch80msRc3_2400 => 2_400,
            Self::FchRc1_1200 | Self::Sch80msRc3_1200 => 1_200,
            Self::FchRc3_2700 | Self::Sch20msRc3_2700 => 2_700,
            Self::FchRc3_1500 | Self::Sch20msRc3_1500 => 1_500,
            Self::FchRc2_14400
            | Self::FchRc5_14400
            | Self::DcchRc5_14400
            | Self::Sch20msRc4_14400
            | Self::Sch40msRc4_14400
            | Self::Sch80msRc4_14400 => 14_400,
            Self::FchRc2_7200
            | Self::FchRc5_7200
            | Self::Sch20msRc4_7200
            | Self::Sch40msRc4_7200
            | Self::Sch80msRc4_7200 => 7_200,
            Self::FchRc2_3600
            | Self::FchRc5_3600
            | Self::Sch20msRc4_3600
            | Self::Sch40msRc4_3600
            | Self::Sch80msRc4_3600 => 3_600,
            Self::FchRc2_1800
            | Self::FchRc5_1800
            | Self::Sch20msRc4_1800
            | Self::Sch40msRc4_1800
            | Self::Sch80msRc4_1800 => 1_800,
            Self::Sch40msRc3_1350 => 1_350,
            Self::Sch20msRc3_19200 | Self::Sch40msRc3_19200 | Self::Sch80msRc3_19200 => 19_200,
            Self::Sch20msRc3_38400 | Self::Sch40msRc3_38400 | Self::Sch80msRc3_38400 => 38_400,
            Self::Sch20msRc3_76800 | Self::Sch40msRc3_76800 | Self::Sch80msRc3_76800 => 76_800,
            Self::Sch20msRc3_153600 | Self::Sch40msRc3_153600 | Self::Sch80msRc5_153600 => 153_600,
            Self::Sch20msRc3_307200 | Self::Sch40msRc5_307200 => 307_200,
            Self::Sch20msRc5_614400 => 614_400,
            Self::Sch20msRc6_1036800 => 1_036_800,
            Self::Sch20msRc4_460800 => 460_800,
            Self::Sch20msRc4_230400 | Self::Sch40msRc4_230400 => 230_400,
            Self::Sch20msRc4_115200 | Self::Sch40msRc4_115200 | Self::Sch80msRc4_115200 => 115_200,
            Self::Sch20msRc4_57600 | Self::Sch40msRc4_57600 | Self::Sch80msRc4_57600 => 57_600,
            Self::Sch20msRc4_28800 | Self::Sch40msRc4_28800 | Self::Sch80msRc4_28800 => 28_800,
            Self::Sch40msRc6_518400 => 518_400,
            Self::Sch80msRc6_259200 => 259_200,
            Self::Idle | Self::FullRateLikely | Self::Erasure | Self::Null => return None,
        })
    }

    pub fn information_bits(self) -> usize {
        match self {
            Self::Idle | Self::FullRateLikely | Self::Erasure | Self::Null => 0,
            Self::FchRc1_9600
            | Self::FchRc3_9600
            | Self::DcchRc3_9600
            | Self::Sch20msRc3_9600
            | Self::Sch40msRc3_4800
            | Self::Sch80msRc3_2400 => 172,
            Self::FchRc1_4800
            | Self::FchRc3_4800
            | Self::Sch20msRc3_4800
            | Self::Sch40msRc3_2400
            | Self::Sch80msRc3_1200 => 80,
            Self::FchRc1_2400
            | Self::FchRc3_2700
            | Self::Sch20msRc3_2700
            | Self::Sch40msRc3_1350 => 40,
            Self::FchRc1_1200 | Self::FchRc3_1500 | Self::Sch20msRc3_1500 => 16,
            Self::FchRc2_14400
            | Self::FchRc5_14400
            | Self::DcchRc5_14400
            | Self::Sch20msRc4_14400
            | Self::Sch40msRc4_7200
            | Self::Sch80msRc4_3600 => 267,
            Self::FchRc2_7200
            | Self::FchRc5_7200
            | Self::Sch20msRc4_7200
            | Self::Sch40msRc4_3600
            | Self::Sch80msRc4_1800 => 125,
            Self::FchRc2_3600 | Self::FchRc5_3600 | Self::Sch20msRc4_3600 => 55,
            Self::FchRc2_1800
            | Self::FchRc5_1800
            | Self::Sch20msRc4_1800
            | Self::Sch40msRc4_1800 => 21,
            Self::FchRc3Forward5ms_9600
            | Self::FchRc5Forward5ms_9600
            | Self::DcchRc5Forward5ms_9600 => 24,
            Self::Sch20msRc5_614400 => 12264,
            Self::Sch20msRc3_307200 | Self::Sch40msRc3_153600 | Self::Sch80msRc3_76800 => 6120,
            Self::Sch20msRc3_153600 => 3048,
            Self::Sch20msRc3_76800 | Self::Sch40msRc3_38400 | Self::Sch80msRc3_19200 => 1512,
            Self::Sch20msRc3_38400 | Self::Sch40msRc3_19200 | Self::Sch80msRc3_9600 => 744,
            Self::Sch20msRc3_19200 | Self::Sch40msRc3_9600 | Self::Sch80msRc3_4800 => 360,
            Self::Sch20msRc6_1036800 => 20712,
            Self::Sch20msRc4_460800 | Self::Sch40msRc4_230400 | Self::Sch80msRc4_115200 => 9192,
            Self::Sch20msRc4_230400 | Self::Sch40msRc4_115200 | Self::Sch80msRc4_57600 => 4584,
            Self::Sch20msRc4_115200 | Self::Sch40msRc4_57600 | Self::Sch80msRc4_28800 => 2280,
            Self::Sch20msRc4_57600 | Self::Sch40msRc4_28800 | Self::Sch80msRc4_14400 => 1128,
            Self::Sch20msRc4_28800 | Self::Sch40msRc4_14400 | Self::Sch80msRc4_7200 => 552,
            Self::Sch40msRc5_307200 | Self::Sch80msRc5_153600 => 12264,
            Self::Sch40msRc3_76800 | Self::Sch80msRc3_38400 => 3048,
            Self::Sch40msRc6_518400 | Self::Sch80msRc6_259200 => 20712,
        }
    }
}

pub const REVERSE_FRAME_CONTENT_FULL_RATE: FrameContent = FrameContent::FchRc3_9600;
pub const REVERSE_FRAME_CONTENT_HALF_RATE: FrameContent = FrameContent::FchRc3_4800;
pub const REVERSE_FRAME_CONTENT_QUARTER_RATE: FrameContent = FrameContent::FchRc3_2700;
pub const REVERSE_FRAME_CONTENT_EIGHTH_RATE: FrameContent = FrameContent::FchRc3_1500;
pub const REVERSE_FRAME_CONTENT_NULL: FrameContent = FrameContent::Null;

/// Supported inherited A3 traffic message payloads used on Abis.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TrafficFrame {
    ForwardFchDcch(ForwardFchDcchFrame),
    ReverseFchDcch(ReverseFchDcchFrame),
    ForwardSch(ForwardSchFrame),
    ReverseSch(ReverseSchFrame),
}

impl TrafficFrame {
    /// Returns the bearer channel family implied by the frame variant.
    pub fn channel_family(&self) -> ChannelFamily {
        match self {
            TrafficFrame::ForwardFchDcch(frame) => frame.channel_family,
            TrafficFrame::ReverseFchDcch(frame) => frame.channel_family,
            TrafficFrame::ForwardSch(_) | TrafficFrame::ReverseSch(_) => ChannelFamily::Sch,
        }
    }

    /// Returns the direction implied by the frame variant.
    pub fn direction(&self) -> Direction {
        match self {
            TrafficFrame::ForwardFchDcch(_) | TrafficFrame::ForwardSch(_) => Direction::Forward,
            TrafficFrame::ReverseFchDcch(_) | TrafficFrame::ReverseSch(_) => Direction::Reverse,
        }
    }

    /// Encodes the frame payload exactly as the inherited A3/Abis traffic message.
    #[must_use = "encoding produces a new buffer; the result should not be discarded"]
    pub fn encode(&self) -> Result<Vec<u8>> {
        match self {
            TrafficFrame::ForwardFchDcch(frame) => frame.encode(),
            TrafficFrame::ReverseFchDcch(frame) => frame.encode(),
            TrafficFrame::ForwardSch(frame) => frame.encode(),
            TrafficFrame::ReverseSch(frame) => frame.encode(),
        }
    }

    /// Decodes a bearer frame for the given family and direction.
    pub fn decode(family: ChannelFamily, direction: Direction, input: &[u8]) -> Result<Self> {
        match (family, direction) {
            (ChannelFamily::Fch | ChannelFamily::Dcch, Direction::Forward) => Ok(
                TrafficFrame::ForwardFchDcch(ForwardFchDcchFrame::decode(family, input)?),
            ),
            (ChannelFamily::Fch | ChannelFamily::Dcch, Direction::Reverse) => Ok(
                TrafficFrame::ReverseFchDcch(ReverseFchDcchFrame::decode(family, input)?),
            ),
            (ChannelFamily::Sch, Direction::Forward) => {
                Ok(TrafficFrame::ForwardSch(ForwardSchFrame::decode(input)?))
            }
            (ChannelFamily::Sch, Direction::Reverse) => {
                Ok(TrafficFrame::ReverseSch(ReverseSchFrame::decode(input)?))
            }
        }
    }
}

/// Forward-link FCH/DCCH bearer frame payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForwardFchDcchFrame {
    pub channel_family: ChannelFamily,
    pub fpc_slc: u8,
    pub fsn: u8,
    pub fpc_gr: u8,
    pub rpc_olt: u8,
    pub frame_content: FrameContent,
    pub forward_link_information: Vec<u8>,
    pub message_crc: u16,
}

impl ForwardFchDcchFrame {
    /// Encodes a forward FCH/DCCH frame payload.
    #[must_use = "encoding produces a new buffer; the result should not be discarded"]
    pub fn encode(&self) -> Result<Vec<u8>> {
        if !matches!(
            self.channel_family,
            ChannelFamily::Fch | ChannelFamily::Dcch
        ) {
            return Err(Error::InvalidValue {
                context: "forward FCH/DCCH frame",
                reason: "channel family must be FCH or DCCH",
            });
        }
        if !(1..=6).contains(&self.fpc_slc) || self.fsn > 0x0f {
            return Err(Error::ReservedValue {
                context: "forward FCH/DCCH frame first octet",
                value: (self.fpc_slc << 4) | self.fsn,
            });
        }
        let mut out = vec![
            forward_message_type(self.channel_family).value(),
            (self.fpc_slc << 4) | self.fsn,
            self.fpc_gr,
            self.rpc_olt,
            self.frame_content.value(),
        ];
        out.extend_from_slice(&self.forward_link_information);
        out.extend_from_slice(&self.message_crc.to_be_bytes());
        Ok(out)
    }

    /// Decodes a forward FCH/DCCH frame payload.
    pub fn decode(channel_family: ChannelFamily, input: &[u8]) -> Result<Self> {
        if input.len() < 7 {
            return Err(Error::Truncated {
                context: "forward FCH/DCCH frame",
                needed: 7,
                actual: input.len(),
            });
        }
        let message_type = forward_message_type(channel_family).value();
        if input[0] != message_type {
            return Err(Error::InvalidValue {
                context: "forward FCH/DCCH message type",
                reason: "message type does not match channel family",
            });
        }
        let fpc_slc = input[1] >> 4;
        let fsn = input[1] & 0x0f;
        if !(1..=6).contains(&fpc_slc) {
            return Err(Error::ReservedValue {
                context: "forward FCH/DCCH frame first octet",
                value: input[1],
            });
        }
        Ok(Self {
            channel_family,
            fpc_slc,
            fsn,
            fpc_gr: input[2],
            rpc_olt: input[3],
            frame_content: FrameContent::from_u8(input[4]).ok_or(Error::ReservedValue {
                context: "forward FCH/DCCH frame content",
                value: input[4],
            })?,
            forward_link_information: input[5..input.len() - 2].to_vec(),
            message_crc: u16::from_be_bytes([input[input.len() - 2], input[input.len() - 1]]),
        })
    }
}

/// Reverse-link FCH/DCCH bearer frame payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReverseFchDcchFrame {
    pub channel_family: ChannelFamily,
    pub soft_handoff_leg: u8,
    pub fsn: u8,
    pub fqi: bool,
    pub reverse_link_quality: u8,
    pub scaling: u8,
    pub packet_arrival_time_error: u8,
    pub frame_content: FrameContent,
    pub fpc_s: u8,
    pub eib: bool,
    pub reverse_link_information: Vec<u8>,
    pub message_crc: u16,
}

impl ReverseFchDcchFrame {
    /// Encodes a reverse FCH/DCCH frame payload.
    #[must_use = "encoding produces a new buffer; the result should not be discarded"]
    pub fn encode(&self) -> Result<Vec<u8>> {
        if !matches!(
            self.channel_family,
            ChannelFamily::Fch | ChannelFamily::Dcch
        ) {
            return Err(Error::InvalidValue {
                context: "reverse FCH/DCCH frame",
                reason: "channel family must be FCH or DCCH",
            });
        }
        if self.soft_handoff_leg > 0x0f
            || self.fsn > 0x0f
            || self.reverse_link_quality > 0x7f
            || self.scaling > 0x03
            || self.packet_arrival_time_error > 0x3f
            || self.fpc_s > 0x7f
        {
            return Err(Error::InvalidValue {
                context: "reverse FCH/DCCH frame",
                reason: "field out of range",
            });
        }
        let mut out = vec![
            reverse_message_type(self.channel_family).value(),
            (self.soft_handoff_leg << 4) | self.fsn,
            ((self.fqi as u8) << 7) | (self.reverse_link_quality & 0x7f),
            (self.scaling << 6) | self.packet_arrival_time_error,
            self.frame_content.value(),
            (self.fpc_s << 1) | self.eib as u8,
        ];
        out.extend_from_slice(&self.reverse_link_information);
        out.extend_from_slice(&self.message_crc.to_be_bytes());
        Ok(out)
    }

    /// Decodes a reverse FCH/DCCH frame payload.
    pub fn decode(channel_family: ChannelFamily, input: &[u8]) -> Result<Self> {
        if input.len() < 8 {
            return Err(Error::Truncated {
                context: "reverse FCH/DCCH frame",
                needed: 8,
                actual: input.len(),
            });
        }
        let message_type = reverse_message_type(channel_family).value();
        if input[0] != message_type {
            return Err(Error::InvalidValue {
                context: "reverse FCH/DCCH message type",
                reason: "message type does not match channel family",
            });
        }
        Ok(Self {
            channel_family,
            soft_handoff_leg: input[1] >> 4,
            fsn: input[1] & 0x0f,
            fqi: input[2] & 0x80 != 0,
            reverse_link_quality: input[2] & 0x7f,
            scaling: input[3] >> 6,
            packet_arrival_time_error: input[3] & 0x3f,
            frame_content: FrameContent::from_u8(input[4]).ok_or(Error::ReservedValue {
                context: "reverse FCH/DCCH frame content",
                value: input[4],
            })?,
            fpc_s: input[5] >> 1,
            eib: input[5] & 1 != 0,
            reverse_link_information: input[6..input.len() - 2].to_vec(),
            message_crc: u16::from_be_bytes([input[input.len() - 2], input[input.len() - 1]]),
        })
    }
}

fn forward_message_type(channel_family: ChannelFamily) -> MessageType {
    match channel_family {
        ChannelFamily::Fch => MessageType::FchForward,
        ChannelFamily::Dcch => MessageType::DcchForward,
        ChannelFamily::Sch => MessageType::SchForward,
    }
}

fn reverse_message_type(channel_family: ChannelFamily) -> MessageType {
    match channel_family {
        ChannelFamily::Fch => MessageType::FchReverse,
        ChannelFamily::Dcch => MessageType::DcchReverse,
        ChannelFamily::Sch => MessageType::SchReverse,
    }
}

/// Forward-link SCH bearer frame payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ForwardSchFrame {
    pub fpc_slc: u8,
    pub fsn: u8,
    pub fpc_gr: u8,
    pub frame_content: FrameContent,
    pub forward_link_information: Vec<u8>,
    pub message_crc: u16,
}

impl ForwardSchFrame {
    /// Encodes a forward SCH frame payload.
    #[must_use = "encoding produces a new buffer; the result should not be discarded"]
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut out = vec![
            (self.fpc_slc << 4) | (self.fsn & 0x0f),
            self.fpc_gr,
            self.frame_content.value(),
        ];
        out.extend_from_slice(&self.forward_link_information);
        out.extend_from_slice(&self.message_crc.to_be_bytes());
        Ok(out)
    }

    /// Decodes a forward SCH frame payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() < 5 {
            return Err(Error::Truncated {
                context: "forward SCH frame",
                needed: 5,
                actual: input.len(),
            });
        }
        Ok(Self {
            fpc_slc: input[0] >> 4,
            fsn: input[0] & 0x0f,
            fpc_gr: input[1],
            frame_content: FrameContent::from_u8(input[2]).ok_or(Error::ReservedValue {
                context: "forward SCH frame content",
                value: input[2],
            })?,
            forward_link_information: input[3..input.len() - 2].to_vec(),
            message_crc: u16::from_be_bytes([input[input.len() - 2], input[input.len() - 1]]),
        })
    }
}

/// Reverse-link SCH bearer frame payload.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReverseSchFrame {
    pub soft_handoff_leg: u8,
    pub fsn: u8,
    pub fqi: bool,
    pub reverse_link_quality: u8,
    pub scaling: u8,
    pub packet_arrival_time_error: u8,
    pub frame_content: FrameContent,
    pub reverse_link_information: Vec<u8>,
    pub message_crc: u16,
}

impl ReverseSchFrame {
    /// Encodes a reverse SCH frame payload.
    #[must_use = "encoding produces a new buffer; the result should not be discarded"]
    pub fn encode(&self) -> Result<Vec<u8>> {
        let mut out = vec![
            (self.soft_handoff_leg << 4) | (self.fsn & 0x0f),
            ((self.fqi as u8) << 7) | (self.reverse_link_quality & 0x7f),
            ((self.scaling & 0x03) << 6) | (self.packet_arrival_time_error & 0x3f),
            self.frame_content.value(),
        ];
        out.extend_from_slice(&self.reverse_link_information);
        out.extend_from_slice(&self.message_crc.to_be_bytes());
        Ok(out)
    }

    /// Decodes a reverse SCH frame payload.
    pub fn decode(input: &[u8]) -> Result<Self> {
        if input.len() < 6 {
            return Err(Error::Truncated {
                context: "reverse SCH frame",
                needed: 6,
                actual: input.len(),
            });
        }
        Ok(Self {
            soft_handoff_leg: input[0] >> 4,
            fsn: input[0] & 0x0f,
            fqi: input[1] & 0x80 != 0,
            reverse_link_quality: input[1] & 0x7f,
            scaling: input[2] >> 6,
            packet_arrival_time_error: input[2] & 0x3f,
            frame_content: FrameContent::from_u8(input[3]).ok_or(Error::ReservedValue {
                context: "reverse SCH frame content",
                value: input[3],
            })?,
            reverse_link_information: input[4..input.len() - 2].to_vec(),
            message_crc: u16::from_be_bytes([input[input.len() - 2], input[input.len() - 1]]),
        })
    }
}
