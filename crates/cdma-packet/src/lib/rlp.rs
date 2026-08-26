//! RLP Type 1 frame codec per IS-707-A.2 (TIA/EIA/IS-707-A Chapter 2).
//!
//! Supports Multiplex Options 1 and 2 primary traffic frame sizes.
//!
//! Frame types:
//!   - Control (SYNC, SYNC/ACK, ACK, NAK) per 4.3.1
//!   - Unsegmented data (Format A) per 4.3.2.1 / 4.3.2.3.1
//!   - Format B data (max throughput) per 4.3.2.3.2
//!   - Segmented data (retransmissions) per 4.3.2.2
//!   - Idle per 4.3.3

/// Traffic channel rate for an RLP frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RlpRate {
    Full,
    Half,
    Quarter,
    Eighth,
}

/// Traffic-channel multiplex option: `One` = Rate Set 1, `Two` = Rate Set 2.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RlpMuxOption {
    One,
    Two,
}

impl RlpMuxOption {
    pub const fn rate_from_bps(self, rate_bps: u32) -> Option<RlpRate> {
        match (self, rate_bps) {
            (Self::One, 9_600) | (Self::Two, 14_400) => Some(RlpRate::Full),
            (Self::One, 4_800) | (Self::Two, 7_200) => Some(RlpRate::Half),
            (Self::Two, 3_600) => Some(RlpRate::Quarter),
            (Self::One, 1_200) | (Self::Two, 1_800) => Some(RlpRate::Eighth),
            _ => None,
        }
    }

    pub const fn rate_bps(self, rate: RlpRate) -> u32 {
        match (self, rate) {
            (Self::One, RlpRate::Full) => 9_600,
            (Self::One, RlpRate::Half) => 4_800,
            (Self::One, RlpRate::Quarter) => 2_400,
            (Self::One, RlpRate::Eighth) => 1_200,
            (Self::Two, RlpRate::Full) => 14_400,
            (Self::Two, RlpRate::Half) => 7_200,
            (Self::Two, RlpRate::Quarter) => 3_600,
            (Self::Two, RlpRate::Eighth) => 1_800,
        }
    }

    pub fn primary_bits(self, rate: RlpRate) -> usize {
        match (self, rate) {
            (Self::One, RlpRate::Full) => 171,
            (Self::One, RlpRate::Half) => 80,
            (Self::One, RlpRate::Quarter) => 40,
            (Self::One, RlpRate::Eighth) => 16,
            (Self::Two, RlpRate::Full) => 266,
            (Self::Two, RlpRate::Half) => 124,
            (Self::Two, RlpRate::Quarter) => 54,
            (Self::Two, RlpRate::Eighth) => 20,
        }
    }

    pub(crate) fn full_information_bits(self) -> usize {
        match self {
            Self::One => 168,
            Self::Two => 264,
        }
    }

    pub const fn format_b_octets(self) -> usize {
        match self {
            Self::One => 20,
            Self::Two => 32,
        }
    }

    pub const fn full_format_a_octets(self) -> usize {
        match self {
            Self::One => 19,
            Self::Two => 31,
        }
    }
}

/// Control frame type, identified by the 4 MSBs of the 6-bit CTL field (4.3.1).
///
/// CTL field format: 4-bit type + '00' (the two LSBs are always '00' for
/// non-encrypted mode where ENCRYPTION_MODE is also '00').
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ControlType {
    /// '1100 00' - Request retransmission of frames FIRST..LAST
    Nak,
    /// '1101 00' - Synchronization request
    Sync,
    /// '1110 00' - Acknowledgment
    Ack,
    /// '1111 00' - Both SYNC and ACK
    SyncAck,
}

impl ControlType {
    /// 6-bit CTL field value for non-encrypted mode.
    pub fn ctl_bits(self) -> u8 {
        match self {
            ControlType::Nak => 0b110000,
            ControlType::Sync => 0b110100,
            ControlType::Ack => 0b111000,
            ControlType::SyncAck => 0b111100,
        }
    }

    fn from_ctl_bits(ctl: u8) -> Option<ControlType> {
        match ctl & 0x3F {
            0b110000 => Some(ControlType::Nak),
            0b110100 => Some(ControlType::Sync),
            0b111000 => Some(ControlType::Ack),
            0b111100 => Some(ControlType::SyncAck),
            _ => None,
        }
    }
}

/// Segmented data frame type (4.3.2.2), used for retransmissions only.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SegmentType {
    /// '1000' - First segment
    First,
    /// '1001' - Second segment
    Second,
    /// '1010' - Last segment
    Last,
    /// '1011' - Intersegment fill (no data)
    IntersegmentFill,
}

/// A decoded RLP Type 1 frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RlpFrame {
    /// Control frame (SYNC, SYNC/ACK, ACK, NAK).
    Control {
        seq: u8,
        control_type: ControlType,
        /// ENCRYPTION_MODE field (2 bits). Always 0x00 for non-encrypted.
        encryption_mode: u8,
        /// First SEQ in NAK range (0x00 for non-NAK).
        first: u8,
        /// Last SEQ in NAK range (0x00 for non-NAK).
        last: u8,
    },
    /// Unsegmented data frame (Format A). CTL bit = '0'.
    Data {
        seq: u8,
        /// Data octets (0..=MAX_LEN). LEN=0 means idle.
        data: Vec<u8>,
    },
    /// Format B data frame (full rate only, max throughput).
    DataFormatB {
        seq: u8,
        /// Exactly 20 octets for Multiplex Option 1 or 32 for Option 2.
        data: Vec<u8>,
    },
    /// Segmented data frame (retransmissions only).
    Segmented {
        seq: u8,
        segment_type: SegmentType,
        /// Data octets (empty for IntersegmentFill).
        data: Vec<u8>,
    },
    /// Rate 1/8 idle frame with Nordstrom-Robinson FCS.
    Idle { seq: u8 },
}

/// Reasons an RLP Type 1 frame cannot be encoded at the requested rate.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RlpEncodeError {
    /// Unsegmented Format A payload exceeds the maximum for the requested rate.
    DataTooLong {
        rate: RlpRate,
        len: usize,
        max_len: usize,
    },
    /// Rate 1 Format B is defined as exactly 20 octets for Mux Option 1.
    FormatBRequiresTwentyOctets { len: usize },
    /// Rate 1 Format B is defined as exactly 32 octets for Mux Option 2.
    FormatBRequiresThirtyTwoOctets { len: usize },
    /// Format B is only valid for Rate 1 frames.
    FormatBRequiresFullRate { rate: RlpRate },
    /// This frame type is not valid at Rate 1/8 for Mux Option 1.
    EighthRateDataCarrier,
    /// Segmented frames cannot be sent with an empty LEN field.
    SegmentedDataRequiresNonZeroLen,
    /// Segmented payload exceeds the maximum for the requested rate.
    SegmentedDataTooLong {
        rate: RlpRate,
        len: usize,
        max_len: usize,
    },
}

impl std::fmt::Display for RlpEncodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RlpEncodeError::DataTooLong { rate, len, max_len } => write!(
                f,
                "RLP data frame has {len} octets, max for {rate:?} is {max_len}"
            ),
            RlpEncodeError::FormatBRequiresTwentyOctets { len } => {
                write!(f, "RLP Format B requires exactly 20 octets, got {len}")
            }
            RlpEncodeError::FormatBRequiresThirtyTwoOctets { len } => {
                write!(f, "RLP Format B requires exactly 32 octets, got {len}")
            }
            RlpEncodeError::FormatBRequiresFullRate { rate } => {
                write!(f, "RLP Format B is only valid at full rate, got {rate:?}")
            }
            RlpEncodeError::EighthRateDataCarrier => {
                write!(f, "Rate 1/8 RLP frames are idle-only")
            }
            RlpEncodeError::SegmentedDataRequiresNonZeroLen => {
                write!(f, "RLP segmented data frames require non-zero LEN")
            }
            RlpEncodeError::SegmentedDataTooLong { rate, len, max_len } => write!(
                f,
                "RLP segmented frame has {len} octets, max for {rate:?} is {max_len}"
            ),
        }
    }
}

impl std::error::Error for RlpEncodeError {}

impl RlpFrame {
    pub fn seq(&self) -> u8 {
        match self {
            RlpFrame::Control { seq, .. } => *seq,
            RlpFrame::Data { seq, .. } => *seq,
            RlpFrame::DataFormatB { seq, .. } => *seq,
            RlpFrame::Segmented { seq, .. } => *seq,
            RlpFrame::Idle { seq } => *seq,
        }
    }

    /// Returns true if this is an idle frame (Rate 1/8 idle or data with LEN=0).
    pub fn is_idle(&self) -> bool {
        match self {
            RlpFrame::Idle { .. } => true,
            RlpFrame::Data { data, .. } => data.is_empty(),
            _ => false,
        }
    }
}

// Encoding

/// Encode an RLP frame into a bit vector at the given rate.
///
/// This compatibility entry point uses Multiplex Option 1.
pub fn encode_frame(frame: &RlpFrame, rate: RlpRate) -> Result<Vec<u8>, RlpEncodeError> {
    encode_frame_for_mux(frame, rate, RlpMuxOption::One)
}

/// Encode an RLP frame for the selected multiplex option.
pub fn encode_frame_for_mux(
    frame: &RlpFrame,
    rate: RlpRate,
    mux_option: RlpMuxOption,
) -> Result<Vec<u8>, RlpEncodeError> {
    let total_bits = mux_option.primary_bits(rate);
    let mut bits = vec![0u8; total_bits];

    match (frame, rate) {
        (RlpFrame::Idle { seq }, RlpRate::Eighth) => {
            // SEQ(8) + FCS(8), followed by four padding bits for Mux Option 2.
            let fcs = nordstrom_robinson_fcs(*seq);
            put_bits(&mut bits, 0, *seq, 8);
            put_bits(&mut bits, 8, fcs, 8);
        }

        (RlpFrame::Idle { seq }, _) => {
            // Higher-rate idle: encode as unsegmented data with LEN=0.
            encode_data_format_a(&mut bits, rate, mux_option, *seq, &[])?;
        }

        (
            RlpFrame::Control {
                seq,
                control_type,
                encryption_mode,
                first,
                last,
            },
            rate,
        ) => {
            // Build the information field for Format A, then wrap it.
            // Control frame: SEQ(8) + CTL(6) + ENCRYPTION_MODE(2) + FIRST(8) + LAST(8) + FCS(16)
            // = 48 bits, rest padded with zeros.
            let information_bits = mux_option.full_information_bits();
            let mut info = vec![0u8; information_bits];
            put_bits(&mut info, 0, *seq, 8);
            put_bits(&mut info, 8, control_type.ctl_bits(), 6);
            put_bits(&mut info, 14, *encryption_mode, 2);
            put_bits(&mut info, 16, *first, 8);
            put_bits(&mut info, 24, *last, 8);
            // FCS-16 covers SEQ + CTL + ENCRYPTION_MODE + FIRST + LAST = bits 0..32,
            // transmitted low-octet first (RFC 1662).
            let fcs = fcs16_rfc1662(&info[0..32]);
            put_bits(&mut info, 32, (fcs & 0xFF) as u8, 8);
            put_bits(&mut info, 40, (fcs >> 8) as u8, 8);
            // bits 48..168 are already zero (padding)

            match rate {
                RlpRate::Full => {
                    bits[..information_bits].copy_from_slice(&info);
                    match mux_option {
                        RlpMuxOption::One => put_bits(&mut bits, information_bits, 0b001, 3),
                        RlpMuxOption::Two => put_bits(&mut bits, information_bits, 0b01, 2),
                    }
                }
                RlpRate::Half | RlpRate::Quarter => {
                    let frame_bits = bits.len();
                    bits.copy_from_slice(&info[..frame_bits]);
                }
                RlpRate::Eighth => {
                    return Err(RlpEncodeError::EighthRateDataCarrier);
                }
            }
        }

        (RlpFrame::Data { seq, data }, rate) => {
            encode_data_format_a(&mut bits, rate, mux_option, *seq, data)?;
        }

        (RlpFrame::DataFormatB { seq, data }, RlpRate::Full) => {
            let required_octets = mux_option.format_b_octets();
            if data.len() != required_octets {
                return Err(match mux_option {
                    RlpMuxOption::One => {
                        RlpEncodeError::FormatBRequiresTwentyOctets { len: data.len() }
                    }
                    RlpMuxOption::Two => {
                        RlpEncodeError::FormatBRequiresThirtyTwoOctets { len: data.len() }
                    }
                });
            }
            put_bits(&mut bits, 0, *seq, 8);
            for (i, byte) in data.iter().enumerate() {
                put_bits(&mut bits, 8 + i * 8, *byte, 8);
            }
            let information_bits = mux_option.full_information_bits();
            match mux_option {
                RlpMuxOption::One => put_bits(&mut bits, information_bits, 0b010, 3),
                RlpMuxOption::Two => put_bits(&mut bits, information_bits, 0b10, 2),
            }
        }

        (RlpFrame::DataFormatB { .. }, rate) => {
            return Err(RlpEncodeError::FormatBRequiresFullRate { rate });
        }

        (
            RlpFrame::Segmented {
                seq,
                segment_type,
                data,
            },
            rate,
        ) => {
            encode_segmented(&mut bits, rate, mux_option, *seq, *segment_type, data)?;
        }
    }

    Ok(bits)
}

fn encode_data_format_a(
    bits: &mut [u8],
    rate: RlpRate,
    mux_option: RlpMuxOption,
    seq: u8,
    data: &[u8],
) -> Result<(), RlpEncodeError> {
    match rate {
        RlpRate::Full => {
            let max_len = max_len_for_rate(rate, mux_option);
            if data.len() > max_len {
                return Err(RlpEncodeError::DataTooLong {
                    rate,
                    len: data.len(),
                    max_len,
                });
            }
            let len = data.len() as u8;
            put_bits(bits, 0, seq, 8);
            put_bits(bits, 8, 0, 1); // CTL = '0' for unsegmented
            put_bits(bits, 9, len, 7);
            for (i, byte) in data.iter().enumerate() {
                put_bits(bits, 16 + i * 8, *byte, 8);
            }
            // padding already zero
            let information_bits = mux_option.full_information_bits();
            match mux_option {
                RlpMuxOption::One => put_bits(bits, information_bits, 0b001, 3),
                RlpMuxOption::Two => put_bits(bits, information_bits, 0b01, 2),
            }
        }
        RlpRate::Half | RlpRate::Quarter => {
            let max_len = max_len_for_rate(rate, mux_option);
            if data.len() > max_len {
                return Err(RlpEncodeError::DataTooLong {
                    rate,
                    len: data.len(),
                    max_len,
                });
            }
            let len = data.len() as u8;
            put_bits(bits, 0, seq, 8);
            put_bits(bits, 8, 0, 1); // CTL = '0'
            put_bits(bits, 9, len, 7);
            for (i, byte) in data.iter().enumerate() {
                put_bits(bits, 16 + i * 8, *byte, 8);
            }
        }
        RlpRate::Eighth => {
            return Err(RlpEncodeError::EighthRateDataCarrier);
        }
    }
    Ok(())
}

fn encode_segmented(
    bits: &mut [u8],
    rate: RlpRate,
    mux_option: RlpMuxOption,
    seq: u8,
    seg_type: SegmentType,
    data: &[u8],
) -> Result<(), RlpEncodeError> {
    // Segmented frame: SEQ(8) + CTL(4) + LEN(0 or 4) + Data(0 or 8*LEN) + Padding
    let ctl = match seg_type {
        SegmentType::First => 0b1000,
        SegmentType::Second => 0b1001,
        SegmentType::Last => 0b1010,
        SegmentType::IntersegmentFill => 0b1011,
    };

    let offset = match rate {
        RlpRate::Full => {
            put_bits(bits, 0, seq, 8);
            put_bits(bits, 8, ctl, 4);
            if seg_type != SegmentType::IntersegmentFill {
                if data.is_empty() {
                    return Err(RlpEncodeError::SegmentedDataRequiresNonZeroLen);
                }
                if data.len() > 15 {
                    return Err(RlpEncodeError::SegmentedDataTooLong {
                        rate,
                        len: data.len(),
                        max_len: 15,
                    });
                }
                let len = data.len() as u8;
                put_bits(bits, 12, len, 4);
                for (i, byte) in data.iter().enumerate() {
                    put_bits(bits, 16 + i * 8, *byte, 8);
                }
            } else if !data.is_empty() {
                return Err(RlpEncodeError::SegmentedDataTooLong {
                    rate,
                    len: data.len(),
                    max_len: 0,
                });
            }
            let information_bits = mux_option.full_information_bits();
            match mux_option {
                RlpMuxOption::One => put_bits(bits, information_bits, 0b001, 3),
                RlpMuxOption::Two => put_bits(bits, information_bits, 0b01, 2),
            }
            return Ok(());
        }
        RlpRate::Half | RlpRate::Quarter => {
            put_bits(bits, 0, seq, 8);
            put_bits(bits, 8, ctl, 4);
            12
        }
        RlpRate::Eighth => return Err(RlpEncodeError::EighthRateDataCarrier),
    };

    if seg_type != SegmentType::IntersegmentFill {
        let max_len = max_len_for_rate_segmented(rate, mux_option);
        let max_len = max_len.min(15);
        if data.is_empty() {
            return Err(RlpEncodeError::SegmentedDataRequiresNonZeroLen);
        }
        if data.len() > max_len {
            return Err(RlpEncodeError::SegmentedDataTooLong {
                rate,
                len: data.len(),
                max_len,
            });
        }
        let len = data.len() as u8;
        put_bits(bits, offset, len, 4);
        for (i, byte) in data.iter().enumerate() {
            put_bits(bits, offset + 4 + i * 8, *byte, 8);
        }
    } else if !data.is_empty() {
        return Err(RlpEncodeError::SegmentedDataTooLong {
            rate,
            len: data.len(),
            max_len: 0,
        });
    }
    Ok(())
}

// Decoding

/// Decode an RLP frame from a bit vector at the given rate.
///
/// `bits` must contain individual bit values (0 or 1).
/// Returns `None` if the frame is invalid.
pub fn decode_frame(bits: &[u8], rate: RlpRate) -> Option<RlpFrame> {
    decode_frame_for_mux(bits, rate, RlpMuxOption::One)
}

/// Decode an RLP frame for the selected multiplex option.
pub fn decode_frame_for_mux(
    bits: &[u8],
    rate: RlpRate,
    mux_option: RlpMuxOption,
) -> Option<RlpFrame> {
    if bits.len() < mux_option.primary_bits(rate) {
        return None;
    }

    match rate {
        RlpRate::Eighth => decode_idle(bits),
        RlpRate::Half | RlpRate::Quarter => decode_non_full_rate(bits, rate, mux_option),
        RlpRate::Full => decode_full_rate(bits, mux_option),
    }
}

fn decode_idle(bits: &[u8]) -> Option<RlpFrame> {
    // Rate 1/8: SEQ(8) + FCS(8) = 16 bits
    let seq = get_bits(bits, 0, 8) as u8;
    let fcs = get_bits(bits, 8, 8) as u8;
    let expected = nordstrom_robinson_fcs(seq);
    if fcs != expected {
        return None;
    }
    Some(RlpFrame::Idle { seq })
}

fn decode_non_full_rate(bits: &[u8], rate: RlpRate, mux_option: RlpMuxOption) -> Option<RlpFrame> {
    // Check MSB of bit 8 to distinguish control (CTL MSB='1') from data (CTL='0').
    let seq = get_bits(bits, 0, 8) as u8;
    let ctl_msb = bits[8];

    if ctl_msb == 0 {
        // Unsegmented data: CTL(1='0') + LEN(7) + Data
        let len = get_bits(bits, 9, 7) as usize;
        let max_len = max_len_for_rate(rate, mux_option);
        if len > max_len {
            return None;
        }
        if len == 0 {
            return Some(RlpFrame::Data { seq, data: vec![] });
        }
        let mut data = vec![0u8; len];
        for i in 0..len {
            data[i] = get_bits(bits, 16 + i * 8, 8) as u8;
        }
        Some(RlpFrame::Data { seq, data })
    } else {
        // Could be control (CTL 6-bit) or segmented (CTL 4-bit).
        // Segmented: CTL 4 bits, MSB='1'. Control: CTL 6 bits, MSB='1'.
        // Differentiate: control CTL has bits[8..14] with pattern 11xxxx.
        // Segmented CTL has bits[8..12] with pattern 1xxx.
        // The key: control frames have CTL[1]='1' (second bit), segmented have CTL[1]='0'.
        let bit1 = bits[9];
        if bit1 == 1 {
            // Control frame: CTL(6) starts with '11'
            let ctl = get_bits(bits, 8, 6) as u8;
            let control_type = ControlType::from_ctl_bits(ctl)?;
            let encryption_mode = get_bits(bits, 14, 2) as u8;
            let first = get_bits(bits, 16, 8) as u8;
            let last = get_bits(bits, 24, 8) as u8;
            // FCS is transmitted low-octet first (RFC 1662).
            let fcs_lo = get_bits(bits, 32, 8) as u8;
            let fcs_hi = get_bits(bits, 40, 8) as u8;
            let received_fcs = ((fcs_hi as u16) << 8) | fcs_lo as u16;
            let computed_fcs = fcs16_rfc1662(&bits[0..32]);
            if received_fcs != computed_fcs {
                return None;
            }
            Some(RlpFrame::Control {
                seq,
                control_type,
                encryption_mode,
                first,
                last,
            })
        } else {
            // Segmented: CTL(4) = '10xx'
            let ctl4 = get_bits(bits, 8, 4) as u8;
            let segment_type = match ctl4 {
                0b1000 => SegmentType::First,
                0b1001 => SegmentType::Second,
                0b1010 => SegmentType::Last,
                0b1011 => SegmentType::IntersegmentFill,
                _ => return None,
            };
            if segment_type == SegmentType::IntersegmentFill {
                return Some(RlpFrame::Segmented {
                    seq,
                    segment_type,
                    data: vec![],
                });
            }
            let len = get_bits(bits, 12, 4) as usize;
            if len == 0 {
                return None; // segmented frames shall not have LEN=0
            }
            let max_len = max_len_for_rate_segmented(rate, mux_option);
            if len > max_len {
                return None;
            }
            let mut data = vec![0u8; len];
            for i in 0..len {
                data[i] = get_bits(bits, 16 + i * 8, 8) as u8;
            }
            Some(RlpFrame::Segmented {
                seq,
                segment_type,
                data,
            })
        }
    }
}

fn decode_full_rate(bits: &[u8], mux_option: RlpMuxOption) -> Option<RlpFrame> {
    let information_bits = mux_option.full_information_bits();
    let (type_bits, format_a_type, format_b_type) = match mux_option {
        RlpMuxOption::One => (3, 0b001, 0b010),
        RlpMuxOption::Two => (2, 0b01, 0b10),
    };
    let frame_type = get_bits(bits, information_bits, type_bits) as u8;
    match frame_type {
        value if value == format_a_type => decode_non_full_rate(bits, RlpRate::Full, mux_option),
        value if value == format_b_type => {
            let seq = get_bits(bits, 0, 8) as u8;
            let mut data = vec![0u8; mux_option.format_b_octets()];
            for i in 0..data.len() {
                data[i] = get_bits(bits, 8 + i * 8, 8) as u8;
            }
            Some(RlpFrame::DataFormatB { seq, data })
        }
        _ => None,
    }
}

// Max data lengths per Table 4.3.2.1-1

/// Maximum data length (LEN) for unsegmented data frames.
fn max_len_for_rate(rate: RlpRate, mux_option: RlpMuxOption) -> usize {
    match (mux_option, rate) {
        (RlpMuxOption::One, RlpRate::Full) => 19,
        (RlpMuxOption::One, RlpRate::Half) => 8,
        (RlpMuxOption::One, RlpRate::Quarter | RlpRate::Eighth) => 0,
        (RlpMuxOption::Two, RlpRate::Full) => 31,
        (RlpMuxOption::Two, RlpRate::Half) => 13,
        (RlpMuxOption::Two, RlpRate::Quarter) => 4,
        (RlpMuxOption::Two, RlpRate::Eighth) => 0,
    }
}

/// Maximum data length for segmented frames.
fn max_len_for_rate_segmented(rate: RlpRate, mux_option: RlpMuxOption) -> usize {
    match (mux_option, rate) {
        (_, RlpRate::Full) => 15,
        (RlpMuxOption::One, RlpRate::Half) => 8,
        (RlpMuxOption::One, RlpRate::Quarter | RlpRate::Eighth) => 0,
        (RlpMuxOption::Two, RlpRate::Half) => 13,
        (RlpMuxOption::Two, RlpRate::Quarter) => 4,
        (RlpMuxOption::Two, RlpRate::Eighth) => 0,
    }
}

// CRC-16 per RFC 1662 (used for control frame FCS)

/// Compute the 16-bit FCS per RFC 1662 over a slice of individual bits (0/1 values).
///
/// Polynomial: x^16 + x^12 + x^5 + 1 (0x8408 reflected).
/// Initial value: 0xFFFF. Final XOR: 0xFFFF.
pub(crate) fn crc16_rlp(bits: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    for &bit in bits {
        let b = (bit & 1) as u16;
        let xor_flag = (crc ^ b) & 0x0001;
        crc >>= 1;
        if xor_flag != 0 {
            crc ^= 0x8408;
        }
    }
    crc ^ 0xFFFF
}

/// RLP control-frame FCS-16 per RFC 1662, as real mobiles compute it.
///
/// The covered field bits are grouped into octets (MSB-first, as laid out in
/// the frame), and each octet is processed least-significant-bit first with the
/// reflected 0x8408 polynomial, then the result is complemented. The 16-bit
/// value is transmitted low-octet first (RFC 1662 §3.1). This differs from a
/// straight MSB-first bit CRC, which does not match on-air control frames.
pub(crate) fn fcs16_rfc1662(bits: &[u8]) -> u16 {
    let mut fcs: u16 = 0xFFFF;
    for octet in bits.chunks(8) {
        let mut byte = 0u8;
        for (i, &b) in octet.iter().enumerate() {
            byte |= (b & 1) << (7 - i);
        }
        fcs ^= byte as u16;
        for _ in 0..8 {
            if fcs & 1 != 0 {
                fcs = (fcs >> 1) ^ 0x8408;
            } else {
                fcs >>= 1;
            }
        }
    }
    !fcs
}

// Nordstrom-Robinson FCS for idle frames (Table 4.3.3-1)

/// Modified Nordstrom-Robinson code lookup table (Table 4.3.3-1 in IS-707-A.2).
///
/// Index = SEQ value (0..255). Each entry is a 16-bit word where the MSB is SEQ
/// and the LSB is the 8-bit FCS. We store only the FCS byte.
static NORDSTROM_ROBINSON_TABLE: [u16; 256] = [
    0x0007, 0x20f3, 0x40ee, 0x6034, 0x8078, 0xa08c, 0xc091, 0xe04b, 0x01d4, 0x2119, 0x4161, 0x6182,
    0x81ab, 0xa166, 0xc11e, 0xe1fd, 0x02a0, 0x226d, 0x423b, 0x62d8, 0x82df, 0xa212, 0xc244, 0xe2a7,
    0x034a, 0x23be, 0x438d, 0x6357, 0x8335, 0xa3c1, 0xc3f2, 0xe328, 0x04c9, 0x242a, 0x4452, 0x649f,
    0x84b6, 0xa455, 0xc42d, 0xe4e0, 0x057f, 0x25a5, 0x45b8, 0x654c, 0x8500, 0xa5da, 0xc5c7, 0xe533,
    0x061c, 0x26c6, 0x46f5, 0x6601, 0x8663, 0xa6b9, 0xc68a, 0xe67e, 0x0793, 0x2770, 0x4726, 0x67eb,
    0x87ec, 0xa70f, 0xc759, 0xe794, 0x089a, 0x2840, 0x485d, 0x68a9, 0x88e5, 0xa83f, 0xc822, 0xe8d6,
    0x092c, 0x29cf, 0x49b7, 0x697a, 0x8953, 0xa9b0, 0xc9c8, 0xe905, 0x0a76, 0x2a95, 0x4ac3, 0x6a0e,
    0x8a09, 0xaaea, 0xcabc, 0xea71, 0x0bf9, 0x2b23, 0x4b10, 0x6be4, 0x8b86, 0xab5c, 0xcb6f, 0xeb9b,
    0x0c31, 0x2cfc, 0x4c84, 0x6c67, 0x8c4e, 0xac83, 0xccfb, 0xec18, 0x0de2, 0x2d16, 0x4d0b, 0x6dd1,
    0x8d9d, 0xad69, 0xcd74, 0xedae, 0x0eaf, 0x2e5b, 0x4e68, 0x6eb2, 0x8ed0, 0xae24, 0xce17, 0xeecd,
    0x0f45, 0x2f88, 0x4fde, 0x6f3d, 0x8f3a, 0xaff7, 0xcfa1, 0xef42, 0x10bd, 0x305e, 0x5008, 0x70c5,
    0x90c2, 0xb021, 0xd077, 0xf0ba, 0x1132, 0x31e8, 0x51db, 0x712f, 0x914d, 0xb197, 0xd1a4, 0xf150,
    0x1251, 0x3296, 0x5262, 0x7298, 0x9237, 0xb2f4, 0xd2e9, 0xf21d, 0x13e7, 0x3304, 0x537c, 0x73b1,
    0x9398, 0xb37b, 0xd303, 0xf3ce, 0x1464, 0x3490, 0x54a3, 0x7479, 0x941b, 0xb4ef, 0xd4dc, 0xf406,
    0x158e, 0x3543, 0x5515, 0x75f6, 0x95f1, 0xb53c, 0xd56a, 0xf589, 0x16fa, 0x3637, 0x564f, 0x76ac,
    0x9685, 0xb648, 0xd630, 0xf6d3, 0x1729, 0x37dd, 0x57c0, 0x771a, 0x9756, 0xb7a2, 0xd7bf, 0xf765,
    0x186b, 0x38a6, 0x58f0, 0x7813, 0x9814, 0xb8d9, 0xd88f, 0xf86c, 0x1981, 0x3975, 0x5946, 0x799c,
    0x99fe, 0xb90a, 0xd939, 0xf9e3, 0x1acc, 0x3a38, 0x5a25, 0x7aff, 0x9ab3, 0xba47, 0xda5a, 0xfa80,
    0x1b1f, 0x3bd2, 0x5baa, 0x7b49, 0x9b60, 0xbbad, 0xdbd5, 0xfb36, 0x1cd7, 0x3c0d, 0x5c3e, 0x7cca,
    0x9ca8, 0xbc72, 0xdc41, 0xfcb5, 0x1d58, 0x3dbb, 0x5ded, 0x7d20, 0x9d27, 0xbdc4, 0xdd92, 0xfd5f,
    0x1e02, 0x3ee1, 0x5e99, 0x7e54, 0x9e7d, 0xbe9e, 0xdee6, 0xfe2b, 0x1fb4, 0x3f6e, 0x5f73, 0x7f87,
    0x9fcb, 0xbf11, 0xdf0c, 0xfff8,
];

/// Compute the 8-bit Nordstrom-Robinson FCS for a given SEQ value.
///
/// The spec table (Table 4.3.3-1) lists 256 codewords in 32 rows x 8 columns.
/// Entry at (row, col) has SEQ = col*32 + row. To look up by SEQ:
///   table_index = (SEQ % 32) * 8 + (SEQ / 32)
pub fn nordstrom_robinson_fcs(seq: u8) -> u8 {
    let row = (seq % 32) as usize;
    let col = (seq / 32) as usize;
    let index = row * 8 + col;
    (NORDSTROM_ROBINSON_TABLE[index] & 0xFF) as u8
}

// Bit manipulation helpers

/// Extract `n` bits from a bit array starting at `offset`, MSB first.
pub(crate) fn get_bits(bits: &[u8], offset: usize, n: usize) -> u32 {
    let mut val: u32 = 0;
    for i in 0..n {
        val = (val << 1) | (bits[offset + i] as u32 & 1);
    }
    val
}

/// Put `n` bits of `val` into a bit array starting at `offset`, MSB first.
pub(crate) fn put_bits(bits: &mut [u8], offset: usize, val: u8, n: usize) {
    for i in 0..n {
        bits[offset + i] = (val >> (n - 1 - i)) & 1;
    }
}

// Constructor helpers for common frames

/// Create a SYNC control frame.
pub fn sync_frame(seq: u8) -> RlpFrame {
    RlpFrame::Control {
        seq,
        control_type: ControlType::Sync,
        encryption_mode: 0,
        first: 0,
        last: 0,
    }
}

/// Create a SYNC/ACK control frame.
pub fn sync_ack_frame(seq: u8) -> RlpFrame {
    RlpFrame::Control {
        seq,
        control_type: ControlType::SyncAck,
        encryption_mode: 0,
        first: 0,
        last: 0,
    }
}

/// Create an ACK control frame.
pub fn ack_frame(seq: u8) -> RlpFrame {
    RlpFrame::Control {
        seq,
        control_type: ControlType::Ack,
        encryption_mode: 0,
        first: 0,
        last: 0,
    }
}

/// Create a NAK control frame requesting retransmission of SEQ range [first, last].
pub fn nak_frame(seq: u8, first: u8, last: u8) -> RlpFrame {
    RlpFrame::Control {
        seq,
        control_type: ControlType::Nak,
        encryption_mode: 0,
        first,
        last,
    }
}

/// Create an unsegmented data frame (Format A).
pub fn data_frame(seq: u8, data: &[u8]) -> RlpFrame {
    RlpFrame::Data {
        seq,
        data: data.to_vec(),
    }
}

/// Create a Format B data frame (20 octets, full rate only).
pub fn data_format_b_frame(seq: u8, data: &[u8]) -> RlpFrame {
    RlpFrame::DataFormatB {
        seq,
        data: data.to_vec(),
    }
}

/// Create a Rate 1/8 idle frame.
pub fn idle_frame(seq: u8) -> RlpFrame {
    RlpFrame::Idle { seq }
}

// Tests

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nordstrom_robinson_table_spot_check() {
        // Table 4.3.3-1 spot checks from spec:
        // SEQ=0x00 => FCS=0x07 (entry 0x0007)
        assert_eq!(nordstrom_robinson_fcs(0x00), 0x07);
        // SEQ=0x01 => FCS=0xd4 (row 1, col 0 = entry 0x01d4)
        assert_eq!(nordstrom_robinson_fcs(0x01), 0xd4);
        // SEQ=0x20 => FCS=0xf3 (row 0, col 1 = entry 0x20f3)
        assert_eq!(nordstrom_robinson_fcs(0x20), 0xf3);
        // SEQ=0xFF => FCS=0xf8 (last entry 0xfff8)
        assert_eq!(nordstrom_robinson_fcs(0xFF), 0xf8);
    }

    #[test]
    fn idle_frame_round_trip() {
        let frame = idle_frame(42);
        let bits = encode_frame(&frame, RlpRate::Eighth).unwrap();
        assert_eq!(bits.len(), 16);
        let decoded = decode_frame(&bits, RlpRate::Eighth).unwrap();
        assert_eq!(decoded, frame);
    }

    #[test]
    fn data_frame_full_rate_round_trip() {
        let data = vec![0xDE, 0xAD, 0xBE, 0xEF, 0x01, 0x02, 0x03];
        let frame = data_frame(10, &data);
        let bits = encode_frame(&frame, RlpRate::Full).unwrap();
        assert_eq!(bits.len(), 171);
        let decoded = decode_frame(&bits, RlpRate::Full).unwrap();
        assert_eq!(decoded, frame);
    }

    #[test]
    fn data_frame_half_rate_round_trip() {
        let data = vec![0xCA, 0xFE, 0xBA, 0xBE];
        let frame = data_frame(5, &data);
        let bits = encode_frame(&frame, RlpRate::Half).unwrap();
        assert_eq!(bits.len(), 80);
        let decoded = decode_frame(&bits, RlpRate::Half).unwrap();
        assert_eq!(decoded, frame);
    }

    #[test]
    fn data_frame_max_len_full_rate() {
        // 19 octets max at full rate Format A
        let data: Vec<u8> = (0..19).collect();
        let frame = data_frame(100, &data);
        let bits = encode_frame(&frame, RlpRate::Full).unwrap();
        let decoded = decode_frame(&bits, RlpRate::Full).unwrap();
        assert_eq!(decoded, frame);
    }

    #[test]
    fn data_frame_max_len_half_rate() {
        // 8 octets max at half rate
        let data: Vec<u8> = (0..8).collect();
        let frame = data_frame(200, &data);
        let bits = encode_frame(&frame, RlpRate::Half).unwrap();
        let decoded = decode_frame(&bits, RlpRate::Half).unwrap();
        assert_eq!(decoded, frame);
    }

    #[test]
    fn oversized_data_frame_encode_is_rejected() {
        let frame = data_frame(1, &[0xAA; 20]);
        assert_eq!(
            encode_frame(&frame, RlpRate::Full),
            Err(RlpEncodeError::DataTooLong {
                rate: RlpRate::Full,
                len: 20,
                max_len: 19,
            })
        );

        let frame = data_frame(1, &[0xAA; 9]);
        assert_eq!(
            encode_frame(&frame, RlpRate::Half),
            Err(RlpEncodeError::DataTooLong {
                rate: RlpRate::Half,
                len: 9,
                max_len: 8,
            })
        );
    }

    #[test]
    fn eighth_rate_rejects_non_idle_encode() {
        let frame = data_frame(1, &[0xAA]);
        assert_eq!(
            encode_frame(&frame, RlpRate::Eighth),
            Err(RlpEncodeError::EighthRateDataCarrier)
        );

        let frame = nak_frame(1, 2, 2);
        assert_eq!(
            encode_frame(&frame, RlpRate::Eighth),
            Err(RlpEncodeError::EighthRateDataCarrier)
        );
    }

    #[test]
    fn format_b_requires_exact_full_rate_frame() {
        let frame = data_format_b_frame(77, &[0xAA; 19]);
        assert_eq!(
            encode_frame(&frame, RlpRate::Full),
            Err(RlpEncodeError::FormatBRequiresTwentyOctets { len: 19 })
        );

        let frame = data_format_b_frame(77, &[0xAA; 20]);
        assert_eq!(
            encode_frame(&frame, RlpRate::Half),
            Err(RlpEncodeError::FormatBRequiresFullRate {
                rate: RlpRate::Half
            })
        );
    }

    #[test]
    fn format_b_round_trip() {
        let data: Vec<u8> = (0..20).collect();
        let frame = data_format_b_frame(77, &data);
        let bits = encode_frame(&frame, RlpRate::Full).unwrap();
        assert_eq!(bits.len(), 171);
        let decoded = decode_frame(&bits, RlpRate::Full).unwrap();
        assert_eq!(decoded, frame);
    }

    #[test]
    fn mux_option_two_idle_round_trip() {
        let frame = idle_frame(42);
        let bits = encode_frame_for_mux(&frame, RlpRate::Eighth, RlpMuxOption::Two).unwrap();
        assert_eq!(bits.len(), 20);
        assert_eq!(&bits[16..], &[0, 0, 0, 0]);
        let decoded = decode_frame_for_mux(&bits, RlpRate::Eighth, RlpMuxOption::Two).unwrap();
        assert_eq!(decoded, frame);
    }

    #[test]
    fn mux_option_two_format_a_round_trips_all_data_rates() {
        for (rate, octets, bits_len) in [
            (RlpRate::Full, 31, 266),
            (RlpRate::Half, 13, 124),
            (RlpRate::Quarter, 4, 54),
        ] {
            let data: Vec<u8> = (0..octets).map(|value| value as u8).collect();
            let frame = data_frame(91, &data);
            let bits = encode_frame_for_mux(&frame, rate, RlpMuxOption::Two).unwrap();
            assert_eq!(bits.len(), bits_len);
            let decoded = decode_frame_for_mux(&bits, rate, RlpMuxOption::Two).unwrap();
            assert_eq!(decoded, frame);
        }
    }

    #[test]
    fn mux_option_two_format_b_round_trip() {
        let data: Vec<u8> = (0..32).collect();
        let frame = data_format_b_frame(77, &data);
        let bits = encode_frame_for_mux(&frame, RlpRate::Full, RlpMuxOption::Two).unwrap();
        assert_eq!(bits.len(), 266);
        assert_eq!(&bits[264..], &[1, 0]);
        let decoded = decode_frame_for_mux(&bits, RlpRate::Full, RlpMuxOption::Two).unwrap();
        assert_eq!(decoded, frame);
    }

    #[test]
    fn mux_option_two_control_frame_round_trip() {
        let frame = sync_frame(0);
        let bits = encode_frame_for_mux(&frame, RlpRate::Full, RlpMuxOption::Two).unwrap();
        assert_eq!(&bits[264..], &[0, 1]);
        let decoded = decode_frame_for_mux(&bits, RlpRate::Full, RlpMuxOption::Two).unwrap();
        assert_eq!(decoded, frame);
    }

    #[test]
    fn sync_frame_full_rate_round_trip() {
        let frame = sync_frame(0);
        let bits = encode_frame(&frame, RlpRate::Full).unwrap();
        assert_eq!(bits.len(), 171);
        let decoded = decode_frame(&bits, RlpRate::Full).unwrap();
        assert_eq!(decoded, frame);
    }

    #[test]
    fn sync_ack_frame_round_trip() {
        let frame = sync_ack_frame(0);
        let bits = encode_frame(&frame, RlpRate::Full).unwrap();
        let decoded = decode_frame(&bits, RlpRate::Full).unwrap();
        assert_eq!(decoded, frame);
    }

    #[test]
    fn ack_frame_round_trip() {
        let frame = ack_frame(0);
        let bits = encode_frame(&frame, RlpRate::Full).unwrap();
        let decoded = decode_frame(&bits, RlpRate::Full).unwrap();
        assert_eq!(decoded, frame);
    }

    #[test]
    fn nak_frame_round_trip() {
        let frame = nak_frame(50, 10, 15);
        let bits = encode_frame(&frame, RlpRate::Full).unwrap();
        let decoded = decode_frame(&bits, RlpRate::Full).unwrap();
        assert_eq!(decoded, frame);
    }

    #[test]
    fn control_frame_half_rate_round_trip() {
        let frame = sync_frame(0);
        let bits = encode_frame(&frame, RlpRate::Half).unwrap();
        assert_eq!(bits.len(), 80);
        let decoded = decode_frame(&bits, RlpRate::Half).unwrap();
        assert_eq!(decoded, frame);
    }

    #[test]
    fn idle_data_frame_full_rate_round_trip() {
        // LEN=0 data frame at full rate = idle
        let frame = data_frame(33, &[]);
        let bits = encode_frame(&frame, RlpRate::Full).unwrap();
        let decoded = decode_frame(&bits, RlpRate::Full).unwrap();
        assert_eq!(decoded, frame);
        assert!(decoded.is_idle());
    }

    #[test]
    fn invalid_idle_fcs_rejected() {
        let frame = idle_frame(42);
        let mut bits = encode_frame(&frame, RlpRate::Eighth).unwrap();
        // Corrupt FCS
        bits[15] ^= 1;
        assert!(decode_frame(&bits, RlpRate::Eighth).is_none());
    }

    #[test]
    fn invalid_control_fcs_rejected() {
        let frame = sync_frame(0);
        let mut bits = encode_frame(&frame, RlpRate::Half).unwrap();
        // Corrupt one of the FCS bits
        bits[35] ^= 1;
        assert!(decode_frame(&bits, RlpRate::Half).is_none());
    }

    #[test]
    fn nordstrom_robinson_table_consistency() {
        // Table is in (row, col) layout where SEQ = col*32 + row.
        // Verify that the MSB of each entry matches its expected SEQ.
        for index in 0..256 {
            let word = NORDSTROM_ROBINSON_TABLE[index];
            let seq_from_table = (word >> 8) as u8;
            let row = index / 8;
            let col = index % 8;
            let expected_seq = (col * 32 + row) as u8;
            assert_eq!(
                seq_from_table, expected_seq,
                "Table index {index} (row={row}, col={col}): expected SEQ={expected_seq:#04x}, got {seq_from_table:#04x}"
            );
        }
    }

    #[test]
    fn nordstrom_robinson_all_seq_round_trip() {
        // Verify FCS lookup works for all 256 SEQ values
        for seq in 0..=255u8 {
            let fcs = nordstrom_robinson_fcs(seq);
            // The FCS should match what we find in the table for this SEQ
            let row = (seq % 32) as usize;
            let col = (seq / 32) as usize;
            let index = row * 8 + col;
            let expected_fcs = (NORDSTROM_ROBINSON_TABLE[index] & 0xFF) as u8;
            assert_eq!(fcs, expected_fcs, "SEQ={seq}");
            // Also verify the MSB of that entry is our SEQ
            let table_seq = (NORDSTROM_ROBINSON_TABLE[index] >> 8) as u8;
            assert_eq!(table_seq, seq, "SEQ mismatch at index {index}");
        }
    }

    #[test]
    fn all_control_types_distinguishable() {
        for ct in [
            ControlType::Sync,
            ControlType::SyncAck,
            ControlType::Ack,
            ControlType::Nak,
        ] {
            let ctl = ct.ctl_bits();
            assert_eq!(ControlType::from_ctl_bits(ctl), Some(ct));
        }
    }

    #[test]
    fn type_field_format_a_is_001() {
        let frame = data_frame(1, &[0xFF]);
        let bits = encode_frame(&frame, RlpRate::Full).unwrap();
        let type_val = get_bits(&bits, 168, 3);
        assert_eq!(type_val, 0b001);
    }

    #[test]
    fn type_field_format_b_is_010() {
        let frame = data_format_b_frame(1, &[0xFF; 20]);
        let bits = encode_frame(&frame, RlpRate::Full).unwrap();
        let type_val = get_bits(&bits, 168, 3);
        assert_eq!(type_val, 0b010);
    }

    #[test]
    fn decodes_real_mobile_type1_sync_half_rate() {
        // Exact reverse SYNC captured from a live SO 12 handset (ESN F274799A)
        // as the 124-bit primary of a 7200-bps RC2 frame:
        //   SEQ=0 CTL=110100(SYNC) EM=0 FIRST=0 LAST=0 FCS=0xd173 (low-octet first)
        // The FCS is RFC 1662 FCS-16, which a straight MSB-first bit CRC misses.
        let prefix = "000000001101000000000000000000001101000101110011";
        let mut bits: Vec<u8> = prefix.bytes().map(|c| c - b'0').collect();
        bits.resize(RlpMuxOption::Two.primary_bits(RlpRate::Half), 0);
        let frame = decode_frame_for_mux(&bits, RlpRate::Half, RlpMuxOption::Two)
            .expect("real mobile SYNC must decode");
        assert!(matches!(
            frame,
            RlpFrame::Control {
                control_type: ControlType::Sync,
                seq: 0,
                ..
            }
        ));
    }

    #[test]
    fn control_fcs_roundtrips_after_rfc1662_fix() {
        for ct in [ControlType::Sync, ControlType::SyncAck, ControlType::Ack] {
            let f = RlpFrame::Control {
                seq: 0x42,
                control_type: ct,
                encryption_mode: 0,
                first: 0,
                last: 0,
            };
            for rate in [RlpRate::Full, RlpRate::Half] {
                let bits = encode_frame_for_mux(&f, rate, RlpMuxOption::Two).unwrap();
                assert_eq!(
                    decode_frame_for_mux(&bits, rate, RlpMuxOption::Two),
                    Some(f.clone())
                );
            }
        }
    }
}
