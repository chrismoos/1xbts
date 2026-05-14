//! RLP Type 3 frame codec per C.S0017-010-A v1.0 Section 4.
//!
//! Supports MuxPDU Type 1 with multiplex options 0x1 (odd) and 0x2 (even),
//! FCH-only (Fundicated RLP frames). No SCH or F-PDCH support.
//!
//! Frame formats used (Table 5): Less than Rate 1 (control/idle/fill),
//! Format A, and Format B. Formats C and D are NOT used.
//!
//! Bit representation: all encode/decode functions work with `&[u8]` slices
//! where each element is a single bit value (0 or 1), MSB-first ordering.

// ---------------------------------------------------------------------------
// Error type
// ---------------------------------------------------------------------------

/// Errors returned by RLP Type 3 frame encode/decode operations.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RlpError {
    /// Input bit slice is shorter than required for the mux option.
    InsufficientBits { expected: usize, got: usize },
    /// FCS-16 check failed.
    FcsInvalid { expected: u16, got: u16 },
    /// Unrecognized or invalid CTL field value.
    InvalidCtl(u8),
    /// Data length exceeds maximum allowed for the mux option.
    DataTooLong { max: usize, got: usize },
    /// Unrecognized TYPE field value.
    InvalidType(u8),
    /// Unrecognized NAK_TYPE value.
    InvalidNakType(u8),
    /// Frame format is inconsistent (e.g. reserved bits set).
    InvalidFrame(String),
    /// RLP_BLOB decode error.
    InvalidBlob(String),
}

impl std::fmt::Display for RlpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RlpError::InsufficientBits { expected, got } => {
                write!(f, "insufficient bits: expected {}, got {}", expected, got)
            }
            RlpError::FcsInvalid { expected, got } => {
                write!(
                    f,
                    "FCS invalid: expected 0x{:04X}, got 0x{:04X}",
                    expected, got
                )
            }
            RlpError::InvalidCtl(v) => write!(f, "invalid CTL field: 0b{:06b}", v),
            RlpError::DataTooLong { max, got } => {
                write!(f, "data too long: max {} octets, got {}", max, got)
            }
            RlpError::InvalidType(v) => write!(f, "invalid TYPE field: 0b{:03b}", v),
            RlpError::InvalidNakType(v) => write!(f, "invalid NAK_TYPE: 0b{:02b}", v),
            RlpError::InvalidFrame(msg) => write!(f, "invalid frame: {}", msg),
            RlpError::InvalidBlob(msg) => write!(f, "invalid RLP_BLOB: {}", msg),
        }
    }
}

impl std::error::Error for RlpError {}

// ---------------------------------------------------------------------------
// Mux option configuration
// ---------------------------------------------------------------------------

/// Multiplex option determining frame sizes and TYPE field widths.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MuxOption {
    /// Mux option 0x1 (odd): 171-bit primary traffic frame.
    /// Format A: Information(168) + TYPE(3). TYPE='001' for RLP.
    /// Format B: SEQ(8) + Data(160) + TYPE(3).
    Odd,
    /// Mux option 0x2 (even): 266-bit primary traffic frame.
    /// Format A: Information(264) + TYPE(2). TYPE='01' for RLP.
    /// Format B: SEQ(8) + Data(256) + TYPE(2).
    Even,
}

impl MuxOption {
    /// Total primary traffic frame size in bits.
    pub fn frame_bits(self) -> usize {
        match self {
            MuxOption::Odd => 171,
            MuxOption::Even => 266,
        }
    }

    /// Width of the TYPE field in bits.
    pub fn type_bits(self) -> usize {
        match self {
            MuxOption::Odd => 3,
            MuxOption::Even => 2,
        }
    }

    /// Number of information bits (frame_bits - type_bits).
    pub fn info_bits(self) -> usize {
        self.frame_bits() - self.type_bits()
    }

    /// TYPE field value for Format A (RLP).
    pub fn type_format_a(self) -> u8 {
        match self {
            MuxOption::Odd => 0b001,
            MuxOption::Even => 0b01,
        }
    }

    /// TYPE field value for Format B new transmission.
    pub fn type_format_b_new(self) -> u8 {
        match self {
            MuxOption::Odd => 0b010,
            MuxOption::Even => 0b10,
        }
    }

    /// TYPE field value for Format B retransmission.
    pub fn type_format_b_rexmit(self) -> u8 {
        match self {
            MuxOption::Odd => 0b011,
            MuxOption::Even => 0b11,
        }
    }

    /// Maximum data octets for unsegmented data frames (Format A) at Rate 1.
    pub fn max_data_len(self) -> usize {
        match self {
            MuxOption::Odd => 19,
            MuxOption::Even => 31,
        }
    }

    /// Data octets carried by a Format B frame.
    pub fn format_b_data_len(self) -> usize {
        match self {
            MuxOption::Odd => 20,
            MuxOption::Even => 32,
        }
    }
}

/// Data octets carried by a 170-bit odd-mux supplemental Rate 1 Format C frame.
pub const SUPPLEMENTAL_FORMAT_C_DATA_LEN: usize = 20;
/// Data octets carried by a 346-bit odd-mux supplemental Rate 1 Format C frame.
pub const SUPPLEMENTAL_FORMAT_C_DOUBLE_DATA_LEN: usize = 42;

/// Encode a 170-bit supplemental Rate 1 Format C RLP frame for SCH/F-PDCH.
///
/// C.S0017-010-A §4.3.4 defines Format C as TYPE(2), SEQ(8), DATA(var). For
/// mux option 0x809 the MAC data block is 170 bits, leaving 160 data bits.
pub fn encode_supplemental_format_c(
    seq: u8,
    rexmit: bool,
    data: &[u8],
) -> Result<Vec<u8>, RlpError> {
    encode_supplemental_format_c_block(seq, rexmit, data, 170)
}

pub fn supplemental_format_c_data_len(block_bits: usize) -> Option<usize> {
    match block_bits {
        170 => Some(SUPPLEMENTAL_FORMAT_C_DATA_LEN),
        346 => Some(SUPPLEMENTAL_FORMAT_C_DOUBLE_DATA_LEN),
        _ => None,
    }
}

pub fn supplemental_format_d_data_len(block_bits: usize) -> Option<usize> {
    match block_bits {
        170 => Some(18),
        346 => Some(40),
        _ => None,
    }
}

pub fn supplemental_format_d_segment_data_len(block_bits: usize, seq_hi: bool) -> Option<usize> {
    let mut header_bits = 2 + 8 + 1 + 1 + 1 + 1 + 8 + 12;
    if seq_hi {
        header_bits += 4;
    }
    while (header_bits - 2) % 8 != 0 {
        header_bits += 1;
    }
    block_bits
        .checked_sub(header_bits)
        .map(|payload_bits| payload_bits / 8)
        .filter(|payload_octets| *payload_octets > 0)
}

/// Encode an odd-mux supplemental Rate 1 Format C RLP frame.
pub fn encode_supplemental_format_c_block(
    seq: u8,
    rexmit: bool,
    data: &[u8],
    block_bits: usize,
) -> Result<Vec<u8>, RlpError> {
    let Some(data_len) = supplemental_format_c_data_len(block_bits) else {
        return Err(RlpError::InvalidFrame(format!(
            "unsupported supplemental Format C block size {} bits",
            block_bits
        )));
    };
    if data.len() != data_len {
        return Err(RlpError::InvalidFrame(format!(
            "supplemental Format C requires {} octets, got {}",
            data_len,
            data.len()
        )));
    }

    let mut frame = vec![0u8; block_bits];
    put_bits(&mut frame, 0, if rexmit { 0b11 } else { 0b10 }, 2);
    put_bits(&mut frame, 2, seq as u32, 8);
    let mut pos = 10;
    for byte in data {
        put_bits(&mut frame, pos, *byte as u32, 8);
        pos += 8;
    }
    Ok(frame)
}

/// Encode an odd-mux supplemental Rate 1 Format D RLP frame.
///
/// C.S0017-010-A §4.3.5 defines Format D as the SCH/F-PDCH frame format that
/// can carry SEQ_HI. For 0x809/0x811/0x821/0x921 style mux options the LEN
/// field is present, so this encoder emits a single unsegmented frame with
/// LAST_SEG=1 and S_SEQ omitted.
pub fn encode_supplemental_format_d_block(
    seq: u8,
    seq_hi: Option<u8>,
    rexmit: bool,
    data: &[u8],
    block_bits: usize,
) -> Result<Vec<u8>, RlpError> {
    let Some(max_data_len) = supplemental_format_d_data_len(block_bits) else {
        return Err(RlpError::InvalidFrame(format!(
            "unsupported supplemental Format D block size {} bits",
            block_bits
        )));
    };
    if data.is_empty() || data.len() > max_data_len {
        return Err(RlpError::InvalidFrame(format!(
            "supplemental Format D requires 1..={} octets, got {}",
            max_data_len,
            data.len()
        )));
    }
    if seq_hi.is_some_and(|v| v > 0x0f) {
        return Err(RlpError::InvalidFrame(format!(
            "supplemental Format D SEQ_HI out of range: {}",
            seq_hi.unwrap()
        )));
    }

    let mut frame = vec![0u8; block_bits];
    let mut pos = 0usize;
    put_bits(&mut frame, pos, 0b00, 2);
    pos += 2;
    put_bits(&mut frame, pos, seq as u32, 8);
    pos += 8;
    put_bits(&mut frame, pos, 0, 1); // SSP: S_SEQ omitted.
    pos += 1;
    put_bits(&mut frame, pos, u32::from(seq_hi.is_some()), 1);
    pos += 1;
    put_bits(&mut frame, pos, 1, 1); // LAST_SEG.
    pos += 1;
    put_bits(&mut frame, pos, u32::from(rexmit), 1);
    pos += 1;
    put_bits(&mut frame, pos, data.len() as u32, 8);
    pos += 8;
    if let Some(seq_hi) = seq_hi {
        put_bits(&mut frame, pos, seq_hi as u32, 4);
        pos += 4;
    }

    while (pos - 2) % 8 != 0 {
        pos += 1;
    }

    for byte in data {
        put_bits(&mut frame, pos, *byte as u32, 8);
        pos += 8;
    }

    Ok(frame)
}

pub fn encode_supplemental_format_d_segment_block(
    seq: u8,
    seq_hi: Option<u8>,
    s_seq: u16,
    last_seg: bool,
    rexmit: bool,
    data: &[u8],
    block_bits: usize,
) -> Result<Vec<u8>, RlpError> {
    let Some(max_data_len) = supplemental_format_d_segment_data_len(block_bits, seq_hi.is_some())
    else {
        return Err(RlpError::InvalidFrame(format!(
            "unsupported supplemental segmented Format D block size {} bits",
            block_bits
        )));
    };
    if data.is_empty() || data.len() > max_data_len {
        return Err(RlpError::InvalidFrame(format!(
            "supplemental segmented Format D requires 1..={} octets, got {}",
            max_data_len,
            data.len()
        )));
    }
    if seq_hi.is_some_and(|v| v > 0x0f) {
        return Err(RlpError::InvalidFrame(format!(
            "supplemental segmented Format D SEQ_HI out of range: {}",
            seq_hi.unwrap()
        )));
    }
    if s_seq > 0x0fff {
        return Err(RlpError::InvalidFrame(format!(
            "supplemental segmented Format D S_SEQ out of range: {}",
            s_seq
        )));
    }

    let mut frame = vec![0u8; block_bits];
    let mut pos = 0usize;
    put_bits(&mut frame, pos, 0b00, 2);
    pos += 2;
    put_bits(&mut frame, pos, seq as u32, 8);
    pos += 8;
    put_bits(&mut frame, pos, 1, 1); // SSP: S_SEQ present.
    pos += 1;
    put_bits(&mut frame, pos, u32::from(seq_hi.is_some()), 1);
    pos += 1;
    put_bits(&mut frame, pos, u32::from(last_seg), 1);
    pos += 1;
    put_bits(&mut frame, pos, u32::from(rexmit), 1);
    pos += 1;
    put_bits(&mut frame, pos, data.len() as u32, 8);
    pos += 8;
    if let Some(seq_hi) = seq_hi {
        put_bits(&mut frame, pos, seq_hi as u32, 4);
        pos += 4;
    }
    put_bits(&mut frame, pos, s_seq as u32, 12);
    pos += 12;

    while (pos - 2) % 8 != 0 {
        pos += 1;
    }

    for byte in data {
        put_bits(&mut frame, pos, *byte as u32, 8);
        pos += 8;
    }

    Ok(frame)
}

// ---------------------------------------------------------------------------
// Control frame types
// ---------------------------------------------------------------------------

/// Control frame type identified by the 6-bit CTL field (Section 4.2).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rlp3ControlType {
    /// CTL = '110110' - Synchronization request.
    Sync,
    /// CTL = '111010' - Acknowledgment.
    Ack,
    /// CTL = '111110' - Combined SYNC and ACK.
    SyncAck,
    /// CTL = '110000' - Negative acknowledgment.
    Nak,
}

impl Rlp3ControlType {
    /// 6-bit CTL field value.
    pub fn ctl_bits(self) -> u8 {
        match self {
            Rlp3ControlType::Sync => 0b110110,
            Rlp3ControlType::Ack => 0b111010,
            Rlp3ControlType::SyncAck => 0b111110,
            Rlp3ControlType::Nak => 0b110000,
        }
    }

    fn from_ctl_bits(ctl: u8) -> Option<Rlp3ControlType> {
        match ctl & 0x3F {
            0b110110 => Some(Rlp3ControlType::Sync),
            0b111010 => Some(Rlp3ControlType::Ack),
            0b111110 => Some(Rlp3ControlType::SyncAck),
            0b110000 => Some(Rlp3ControlType::Nak),
            _ => None,
        }
    }
}

// ---------------------------------------------------------------------------
// NAK types
// ---------------------------------------------------------------------------

/// NAK frame sub-type per Section 4.2.2.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum NakPayload {
    /// NAK_TYPE '00': Gap-based NAK.
    /// Each entry specifies a range [FIRST, LAST] of missing sequence numbers.
    Gap(Vec<NakGapEntry>),
    /// NAK_TYPE '01': Map-based NAK.
    /// Each entry specifies a starting sequence and an 8-bit bitmap of missing frames.
    Map(Vec<NakMapEntry>),
    /// NAK_TYPE '10': Segment-based NAK (first/last sub-sequence).
    SegmentRange(Vec<NakSegRangeEntry>),
    /// NAK_TYPE '11': Segment-based NAK (start + length sub-sequence).
    SegmentLength(Vec<NakSegLenEntry>),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NakGapEntry {
    pub first: u16,
    pub last: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NakMapEntry {
    pub nak_map_seq: u16,
    pub nak_map: u8,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NakSegRangeEntry {
    pub frame_seq: u16,
    pub first_s_seq: u16,
    pub last_s_seq: u16,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NakSegLenEntry {
    pub frame_seq: u16,
    pub first_s_seq: u16,
    pub length_s_seq: u8,
}

// ---------------------------------------------------------------------------
// RLP Type 3 frame enum
// ---------------------------------------------------------------------------

/// A decoded RLP Type 3 frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rlp3Frame {
    /// SYNC, ACK, or SYNC/ACK control frame (Section 4.2.1).
    Control {
        /// L_SEQ least significant 8 bits.
        seq: u8,
        control_type: Rlp3ControlType,
        /// INIT_VAR flag: true to force RLP initialization.
        init_var: bool,
        /// NAK_PARAM_INCL flag (currently always false).
        nak_param_incl: bool,
    },
    /// NAK control frame (Section 4.2.2).
    Nak {
        /// L_SEQ least significant 8 bits.
        seq: u8,
        /// SEQ_HI: 4 MSBs of the 12-bit L_SEQ.
        seq_hi: u8,
        payload: NakPayload,
    },
    /// Unsegmented data frame (Section 4.3.1.1, Format A).
    Data {
        /// L_SEQ least significant 8 bits.
        seq: u8,
        /// REXMIT flag: true if this is a retransmission.
        rexmit: bool,
        /// Data octets (0..=MAX_LEN).
        data: Vec<u8>,
    },
    /// Segmented data frame (Section 4.3.2).
    Segmented {
        /// L_SEQ least significant 8 bits.
        seq: u8,
        /// SQI: sequence qualifier indicator.
        sqi: bool,
        /// LAST_SEG: true if this is the last segment.
        last_seg: bool,
        /// REXMIT flag.
        rexmit: bool,
        /// SEQ_HI: present when SQI=1 (4 MSBs of 12-bit L_SEQ).
        seq_hi: Option<u8>,
        /// S_SEQ: segment sequence number (12 bits).
        s_seq: u16,
        /// Data octets.
        data: Vec<u8>,
    },
    /// Format B data frame (Section 4.3.3.1).
    DataFormatB {
        /// L_SEQ least significant 8 bits.
        seq: u8,
        /// True if retransmission.
        rexmit: bool,
        /// Data octets (20 for odd mux, 32 for even mux).
        data: Vec<u8>,
    },
    /// Fill frame (Section 4.4).
    Fill {
        /// L_SEQ least significant 8 bits.
        seq: u8,
        /// SEQ_HI: 4 MSBs of the 12-bit L_V(N).
        seq_hi: u8,
    },
    /// Idle frame Format 1 (Section 4.5.1).
    Idle1 {
        /// L_SEQ least significant 8 bits.
        seq: u8,
        /// SEQ_HI: 4 MSBs of the 12-bit L_V(N).
        seq_hi: u8,
    },
    /// Idle frame Format 2 (Section 4.5.2).
    Idle2 {
        /// L_SEQ least significant 8 bits.
        seq: u8,
    },
}

impl Rlp3Frame {
    /// Returns the 8-bit SEQ field common to all frame types.
    pub fn seq(&self) -> u8 {
        match self {
            Rlp3Frame::Control { seq, .. } => *seq,
            Rlp3Frame::Nak { seq, .. } => *seq,
            Rlp3Frame::Data { seq, .. } => *seq,
            Rlp3Frame::Segmented { seq, .. } => *seq,
            Rlp3Frame::DataFormatB { seq, .. } => *seq,
            Rlp3Frame::Fill { seq, .. } => *seq,
            Rlp3Frame::Idle1 { seq, .. } => *seq,
            Rlp3Frame::Idle2 { seq, .. } => *seq,
        }
    }
}

// ---------------------------------------------------------------------------
// CRC-16 (FCS-16) per CRC-CCITT
// ---------------------------------------------------------------------------

/// Compute CRC-CCITT (x^16 + x^12 + x^5 + 1) over a slice of individual
/// bits (0/1 values) in MSB-first frame order. Initial value: 0xFFFF.
/// Final XOR: 0xFFFF.
///
/// Per RFC 1662 / HDLC, the CRC processes data LSB-first within each octet.
/// Since our bit arrays are MSB-first (matching the air-interface bit order),
/// we reverse bits within each 8-bit group before feeding them to the
/// reflected CRC-16 engine.
///
/// Returns the 16-bit FCS value. Use `put_fcs16` / `get_fcs16` to write/read
/// it in the frame in the correct (low-byte-first) order.
fn fcs16(bits: &[u8]) -> u16 {
    let mut crc: u16 = 0xFFFF;
    // Process each octet LSB-first (HDLC convention).
    let mut i = 0;
    while i < bits.len() {
        let end = (i + 8).min(bits.len());
        // Reverse bits within this octet.
        for j in (i..end).rev() {
            let b = (bits[j] & 1) as u16;
            let xor_flag = (crc ^ b) & 0x0001;
            crc >>= 1;
            if xor_flag != 0 {
                crc ^= 0x8408;
            }
        }
        i += 8;
    }
    crc ^ 0xFFFF
}

/// Write a 16-bit FCS into a bit array at `offset` in HDLC order (low byte first,
/// each byte MSB-first in the frame).
fn put_fcs16(bits: &mut [u8], offset: usize, fcs: u16) {
    let lo = (fcs & 0xFF) as u32;
    let hi = ((fcs >> 8) & 0xFF) as u32;
    put_bits(bits, offset, lo, 8); // low byte first
    put_bits(bits, offset + 8, hi, 8); // high byte second
}

/// Read a 16-bit FCS from a bit array at `offset` in HDLC order (low byte first).
fn get_fcs16(bits: &[u8], offset: usize) -> u16 {
    let lo = get_bits(bits, offset, 8) as u16;
    let hi = get_bits(bits, offset + 8, 8) as u16;
    (hi << 8) | lo
}

// ---------------------------------------------------------------------------
// Bit manipulation helpers
// ---------------------------------------------------------------------------

/// Extract `n` bits from a bit array starting at `offset`, MSB first. Returns u32.
fn get_bits(bits: &[u8], offset: usize, n: usize) -> u32 {
    let mut val: u32 = 0;
    for i in 0..n {
        val = (val << 1) | (bits[offset + i] as u32 & 1);
    }
    val
}

/// Bounds-checked variant of get_bits for variable-length parsing.
fn try_get_bits(bits: &[u8], offset: usize, n: usize) -> Result<u32, RlpError> {
    if offset + n > bits.len() {
        return Err(RlpError::InsufficientBits {
            expected: offset + n,
            got: bits.len(),
        });
    }
    Ok(get_bits(bits, offset, n))
}

/// Put `n` bits of `val` into a bit array starting at `offset`, MSB first.
fn put_bits(bits: &mut [u8], offset: usize, val: u32, n: usize) {
    for i in 0..n {
        bits[offset + i] = ((val >> (n - 1 - i)) & 1) as u8;
    }
}

// ---------------------------------------------------------------------------
// Encoding
// ---------------------------------------------------------------------------

impl Rlp3Frame {
    /// Encode this frame into a bit vector for the given mux option.
    ///
    /// For Rate 1 frames (Format A and Format B), returns `mux.frame_bits()` bits.
    /// For less-than-Rate-1 frames (control/idle/fill), returns `mux.info_bits()` bits
    /// (the caller wraps them with the appropriate rate signaling).
    pub fn encode(&self, mux: MuxOption) -> Result<Vec<u8>, RlpError> {
        match self {
            Rlp3Frame::Control {
                seq,
                control_type,
                init_var,
                nak_param_incl,
            } => encode_control(*seq, *control_type, *init_var, *nak_param_incl, mux),
            Rlp3Frame::Nak {
                seq,
                seq_hi,
                payload,
            } => encode_nak(*seq, *seq_hi, payload, mux),
            Rlp3Frame::Data { seq, rexmit, data } => {
                encode_data_unsegmented(*seq, *rexmit, data, mux)
            }
            Rlp3Frame::Segmented {
                seq,
                sqi,
                last_seg,
                rexmit,
                seq_hi,
                s_seq,
                data,
            } => encode_segmented(*seq, *sqi, *last_seg, *rexmit, *seq_hi, *s_seq, data, mux),
            Rlp3Frame::DataFormatB { seq, rexmit, data } => {
                encode_format_b(*seq, *rexmit, data, mux)
            }
            Rlp3Frame::Fill { seq, seq_hi } => encode_fill(*seq, *seq_hi, mux),
            Rlp3Frame::Idle1 { seq, seq_hi } => encode_idle1(*seq, *seq_hi, mux),
            Rlp3Frame::Idle2 { seq } => encode_idle2(*seq, mux),
        }
    }
}

/// Encode a SYNC/ACK/SYNC_ACK control frame into a full rate frame.
fn encode_control(
    seq: u8,
    control_type: Rlp3ControlType,
    init_var: bool,
    nak_param_incl: bool,
    mux: MuxOption,
) -> Result<Vec<u8>, RlpError> {
    let info_bits = mux.info_bits();
    let total_bits = mux.frame_bits();

    // Build information field content (before padding).
    // SEQ(8) + CTL(6) + INIT_VAR(1) + NAK_PARAM_INCL(1) = 16 bits before FCS.
    let pre_fcs_bits = 16;
    // Padding_1 = 0 bits (no EXT_SEQ_M, no NAK params)
    // Total content = pre_fcs + FCS + Padding_2 to fill info_bits.

    let mut info = vec![0u8; info_bits];
    let mut pos = 0;

    put_bits(&mut info, pos, seq as u32, 8);
    pos += 8;
    put_bits(&mut info, pos, control_type.ctl_bits() as u32, 6);
    pos += 6;
    put_bits(&mut info, pos, if init_var { 1 } else { 0 }, 1);
    pos += 1;
    put_bits(&mut info, pos, if nak_param_incl { 1 } else { 0 }, 1);
    pos += 1;

    // FCS-16 covers all bits before FCS (bits 0..pre_fcs_bits).
    let fcs = fcs16(&info[0..pre_fcs_bits]);
    put_fcs16(&mut info, pos, fcs);
    // Remaining bits are Padding_2 (already zero).

    // Wrap in Format A.
    let mut frame = vec![0u8; total_bits];
    frame[..info_bits].copy_from_slice(&info);
    put_bits(
        &mut frame,
        info_bits,
        mux.type_format_a() as u32,
        mux.type_bits(),
    );
    Ok(frame)
}

/// Encode a NAK control frame.
fn encode_nak(
    seq: u8,
    seq_hi: u8,
    payload: &NakPayload,
    mux: MuxOption,
) -> Result<Vec<u8>, RlpError> {
    let info_bits = mux.info_bits();
    let total_bits = mux.frame_bits();
    let mut info = vec![0u8; info_bits];
    let mut pos = 0;

    // SEQ(8)
    put_bits(&mut info, pos, seq as u32, 8);
    pos += 8;
    // CTL(6) = '110000'
    put_bits(&mut info, pos, Rlp3ControlType::Nak.ctl_bits() as u32, 6);
    pos += 6;
    // NAK_TYPE(2)
    let nak_type: u8 = match payload {
        NakPayload::Gap(_) => 0b00,
        NakPayload::Map(_) => 0b01,
        NakPayload::SegmentRange(_) => 0b10,
        NakPayload::SegmentLength(_) => 0b11,
    };
    put_bits(&mut info, pos, nak_type as u32, 2);
    pos += 2;
    // SEQ_HI(4)
    put_bits(&mut info, pos, seq_hi as u32, 4);
    pos += 4;

    // Type-specific fields.
    match payload {
        NakPayload::Gap(entries) => {
            let count = (entries.len().saturating_sub(1) & 0x3) as u32;
            put_bits(&mut info, pos, count, 2);
            pos += 2;
            for entry in entries {
                put_bits(&mut info, pos, entry.first as u32, 12);
                pos += 12;
                put_bits(&mut info, pos, entry.last as u32, 12);
                pos += 12;
            }
        }
        NakPayload::Map(entries) => {
            let count = (entries.len().saturating_sub(1) & 0x3) as u32;
            put_bits(&mut info, pos, count, 2);
            pos += 2;
            for entry in entries {
                put_bits(&mut info, pos, entry.nak_map_seq as u32, 12);
                pos += 12;
                put_bits(&mut info, pos, entry.nak_map as u32, 8);
                pos += 8;
            }
        }
        NakPayload::SegmentRange(entries) => {
            let count = (entries.len().saturating_sub(1) & 0x3) as u32;
            put_bits(&mut info, pos, count, 2);
            pos += 2;
            for entry in entries {
                put_bits(&mut info, pos, entry.frame_seq as u32, 12);
                pos += 12;
                put_bits(&mut info, pos, entry.first_s_seq as u32, 12);
                pos += 12;
                put_bits(&mut info, pos, entry.last_s_seq as u32, 12);
                pos += 12;
            }
        }
        NakPayload::SegmentLength(entries) => {
            let count = (entries.len().saturating_sub(1) & 0x3) as u32;
            put_bits(&mut info, pos, count, 2);
            pos += 2;
            for entry in entries {
                put_bits(&mut info, pos, entry.frame_seq as u32, 12);
                pos += 12;
                put_bits(&mut info, pos, entry.first_s_seq as u32, 12);
                pos += 12;
                put_bits(&mut info, pos, entry.length_s_seq as u32, 8);
                pos += 8;
            }
        }
    }

    // Padding_1 octet-aligns the FCS field. FCS covers all fields before it,
    // including Padding_1.
    let fcs_pos = align_to_octet(pos);
    if fcs_pos + 16 > info_bits {
        return Err(RlpError::InsufficientBits {
            expected: fcs_pos + 16,
            got: info_bits,
        });
    }
    let fcs = fcs16(&info[0..fcs_pos]);
    put_fcs16(&mut info, fcs_pos, fcs);
    // Remaining bits are Padding_2 (zero).

    let mut frame = vec![0u8; total_bits];
    frame[..info_bits].copy_from_slice(&info);
    put_bits(
        &mut frame,
        info_bits,
        mux.type_format_a() as u32,
        mux.type_bits(),
    );
    Ok(frame)
}

/// Encode an unsegmented data frame (Format A).
fn encode_data_unsegmented(
    seq: u8,
    rexmit: bool,
    data: &[u8],
    mux: MuxOption,
) -> Result<Vec<u8>, RlpError> {
    let max_len = mux.max_data_len();
    if data.len() > max_len {
        return Err(RlpError::DataTooLong {
            max: max_len,
            got: data.len(),
        });
    }

    let info_bits = mux.info_bits();
    let total_bits = mux.frame_bits();
    let mut info = vec![0u8; info_bits];
    let mut pos = 0;

    // SEQ(8) + CTL(1='0') + REXMIT(1) + LEN(6) + Data(8*LEN) + Padding
    put_bits(&mut info, pos, seq as u32, 8);
    pos += 8;
    put_bits(&mut info, pos, 0, 1); // CTL = '0' for unsegmented
    pos += 1;
    put_bits(&mut info, pos, if rexmit { 1 } else { 0 }, 1);
    pos += 1;
    put_bits(&mut info, pos, data.len() as u32, 6);
    pos += 6;
    for byte in data {
        put_bits(&mut info, pos, *byte as u32, 8);
        pos += 8;
    }
    // Remaining bits are padding (zero).

    let mut frame = vec![0u8; total_bits];
    frame[..info_bits].copy_from_slice(&info);
    put_bits(
        &mut frame,
        info_bits,
        mux.type_format_a() as u32,
        mux.type_bits(),
    );
    Ok(frame)
}

/// Encode a segmented data frame (Format A).
fn encode_segmented(
    seq: u8,
    sqi: bool,
    last_seg: bool,
    rexmit: bool,
    seq_hi: Option<u8>,
    s_seq: u16,
    data: &[u8],
    mux: MuxOption,
) -> Result<Vec<u8>, RlpError> {
    let info_bits = mux.info_bits();
    let total_bits = mux.frame_bits();
    let mut info = vec![0u8; info_bits];
    let mut pos = 0;

    // SEQ(8) + CTL(4='1000') + SQI(1) + LAST_SEG(1) + REXMIT(1) + LEN(5)
    put_bits(&mut info, pos, seq as u32, 8);
    pos += 8;
    put_bits(&mut info, pos, 0b1000, 4); // CTL for segmented data
    pos += 4;
    put_bits(&mut info, pos, if sqi { 1 } else { 0 }, 1);
    pos += 1;
    put_bits(&mut info, pos, if last_seg { 1 } else { 0 }, 1);
    pos += 1;
    put_bits(&mut info, pos, if rexmit { 1 } else { 0 }, 1);
    pos += 1;
    put_bits(&mut info, pos, (data.len() & 0x1F) as u32, 5);
    pos += 5;

    if sqi {
        // SEQ_HI(4)
        let hi = seq_hi.unwrap_or(0);
        put_bits(&mut info, pos, hi as u32, 4);
        pos += 4;
    }
    // S_SEQ(12)
    put_bits(&mut info, pos, s_seq as u32, 12);
    pos += 12;
    if sqi {
        // Padding_1(4)
        pos += 4;
    }
    // Data(8*LEN)
    for byte in data {
        put_bits(&mut info, pos, *byte as u32, 8);
        pos += 8;
    }

    let mut frame = vec![0u8; total_bits];
    frame[..info_bits].copy_from_slice(&info);
    put_bits(
        &mut frame,
        info_bits,
        mux.type_format_a() as u32,
        mux.type_bits(),
    );
    Ok(frame)
}

/// Encode a Format B data frame.
fn encode_format_b(
    seq: u8,
    rexmit: bool,
    data: &[u8],
    mux: MuxOption,
) -> Result<Vec<u8>, RlpError> {
    let expected_len = mux.format_b_data_len();
    if data.len() != expected_len {
        return Err(RlpError::DataTooLong {
            max: expected_len,
            got: data.len(),
        });
    }

    let total_bits = mux.frame_bits();
    let mut frame = vec![0u8; total_bits];
    let mut pos = 0;

    // SEQ(8) + Data(data_bits) + TYPE
    put_bits(&mut frame, pos, seq as u32, 8);
    pos += 8;
    for byte in data {
        put_bits(&mut frame, pos, *byte as u32, 8);
        pos += 8;
    }
    let type_val = if rexmit {
        mux.type_format_b_rexmit()
    } else {
        mux.type_format_b_new()
    };
    put_bits(&mut frame, pos, type_val as u32, mux.type_bits());
    Ok(frame)
}

/// Encode a fill frame at Rate 1; wrapped in Format A.
fn encode_fill(seq: u8, seq_hi: u8, mux: MuxOption) -> Result<Vec<u8>, RlpError> {
    let info_bits = mux.info_bits();
    let total_bits = mux.frame_bits();
    let mut info = vec![0u8; info_bits];
    let mut pos = 0;

    // SEQ(8) + CTL(4='1001') + SEQ_HI(4) + Padding(variable)
    put_bits(&mut info, pos, seq as u32, 8);
    pos += 8;
    put_bits(&mut info, pos, 0b1001, 4);
    pos += 4;
    put_bits(&mut info, pos, seq_hi as u32, 4);

    let mut frame = vec![0u8; total_bits];
    frame[..info_bits].copy_from_slice(&info);
    put_bits(
        &mut frame,
        info_bits,
        mux.type_format_a() as u32,
        mux.type_bits(),
    );
    Ok(frame)
}

/// Encode idle frame Format 1 (Section 4.5.1).
fn encode_idle1(seq: u8, seq_hi: u8, mux: MuxOption) -> Result<Vec<u8>, RlpError> {
    let info_bits = mux.info_bits();
    let total_bits = mux.frame_bits();
    let mut info = vec![0u8; info_bits];
    let mut pos = 0;

    // SEQ(8) + CTL(4='1010') + SEQ_HI(4) + Padding(variable)
    put_bits(&mut info, pos, seq as u32, 8);
    pos += 8;
    put_bits(&mut info, pos, 0b1010, 4);
    pos += 4;
    put_bits(&mut info, pos, seq_hi as u32, 4);

    let mut frame = vec![0u8; total_bits];
    frame[..info_bits].copy_from_slice(&info);
    put_bits(
        &mut frame,
        info_bits,
        mux.type_format_a() as u32,
        mux.type_bits(),
    );
    Ok(frame)
}

/// Encode idle frame Format 2 (Section 4.5.2).
fn encode_idle2(seq: u8, mux: MuxOption) -> Result<Vec<u8>, RlpError> {
    let info_bits = mux.info_bits();
    let total_bits = mux.frame_bits();
    let mut info = vec![0u8; info_bits];
    let mut pos = 0;

    // SEQ(8) + CTL(4='1000') + SQI(1) + LAST_SEG(1='0') + REXMIT(1='0')
    // + LEN(5='00000') + SEQ_HI(4) + S_SEQ(12) + Padding(variable).
    put_bits(&mut info, pos, seq as u32, 8);
    pos += 8;
    put_bits(&mut info, pos, 0b1000, 4);
    pos += 4;
    put_bits(&mut info, pos, 1, 1); // SQI
    pos += 1;
    put_bits(&mut info, pos, 0, 1); // LAST_SEG
    pos += 1;
    put_bits(&mut info, pos, 0, 1); // REXMIT
    pos += 1;
    put_bits(&mut info, pos, 0, 5); // LEN
    pos += 5;
    put_bits(&mut info, pos, 0, 4); // SEQ_HI
    pos += 4;
    put_bits(&mut info, pos, 0, 12); // S_SEQ

    let mut frame = vec![0u8; total_bits];
    frame[..info_bits].copy_from_slice(&info);
    put_bits(
        &mut frame,
        info_bits,
        mux.type_format_a() as u32,
        mux.type_bits(),
    );
    Ok(frame)
}

// ---------------------------------------------------------------------------
// Decoding
// ---------------------------------------------------------------------------

/// Decode an RLP Type 3 frame from a bit vector for the given mux option.
///
/// `bits` must contain individual bit values (0 or 1).
pub fn decode_rlp3_frame(bits: &[u8], mux: MuxOption) -> Result<Rlp3Frame, RlpError> {
    let expected = mux.frame_bits();
    if bits.len() < expected {
        return Err(RlpError::InsufficientBits {
            expected,
            got: bits.len(),
        });
    }

    let info_bits = mux.info_bits();
    let type_offset = info_bits;
    let type_val = get_bits(bits, type_offset, mux.type_bits()) as u8;

    match mux {
        MuxOption::Odd => match type_val {
            0b001 => decode_format_a(&bits[..info_bits], mux),
            0b010 => decode_format_b(bits, mux, false),
            0b011 => decode_format_b(bits, mux, true),
            _ => Err(RlpError::InvalidType(type_val)),
        },
        MuxOption::Even => match type_val {
            0b01 => decode_format_a(&bits[..info_bits], mux),
            0b10 => decode_format_b(bits, mux, false),
            0b11 => decode_format_b(bits, mux, true),
            _ => Err(RlpError::InvalidType(type_val)),
        },
    }
}

/// Decode the information field of a Format A frame.
fn decode_format_a(info: &[u8], mux: MuxOption) -> Result<Rlp3Frame, RlpError> {
    if info.len() < 16 {
        return Err(RlpError::InsufficientBits {
            expected: 16,
            got: info.len(),
        });
    }

    let seq = get_bits(info, 0, 8) as u8;

    // Bit 8 distinguishes unsegmented data (CTL='0') from control/segmented (CTL MSB='1').
    let ctl_msb = info[8];

    if ctl_msb == 0 {
        // Unsegmented data frame: SEQ(8) + CTL(1='0') + REXMIT(1) + LEN(6) + Data
        let rexmit = info[9] == 1;
        let len = get_bits(info, 10, 6) as usize;
        let max_len = mux.max_data_len();
        if len > max_len {
            return Err(RlpError::DataTooLong {
                max: max_len,
                got: len,
            });
        }
        let data_start = 16;
        let mut data = vec![0u8; len];
        for i in 0..len {
            if data_start + (i + 1) * 8 > info.len() {
                return Err(RlpError::InsufficientBits {
                    expected: data_start + (i + 1) * 8,
                    got: info.len(),
                });
            }
            data[i] = get_bits(info, data_start + i * 8, 8) as u8;
        }
        return Ok(Rlp3Frame::Data { seq, rexmit, data });
    }

    // CTL MSB is 1. Check bit 9 to further distinguish.
    let bit9 = info[9];

    if bit9 == 0 && info[10] == 0 && info[11] == 0 {
        // CTL='1000' can be Idle Format 2, or a segmented data frame.
        // Idle2 is the zero-length segmented-format pattern defined in §4.5.2.
        if is_idle2_info(info) {
            return Ok(Rlp3Frame::Idle2 { seq });
        }
        return decode_segmented_frame(info, mux);
    }

    // CTL='1001' -> Fill, CTL='1010' -> Idle Format 1.
    if bit9 == 0 && info[10] == 0 && info[11] == 1 {
        let seq_hi = get_bits(info, 12, 4) as u8;
        return Ok(Rlp3Frame::Fill { seq, seq_hi });
    }
    if bit9 == 0 && info[10] == 1 && info[11] == 0 {
        let seq_hi = get_bits(info, 12, 4) as u8;
        return Ok(Rlp3Frame::Idle1 { seq, seq_hi });
    }

    // Read full 6-bit CTL field.
    let ctl = get_bits(info, 8, 6) as u8;

    if let Some(ct) = Rlp3ControlType::from_ctl_bits(ctl) {
        if ct == Rlp3ControlType::Nak {
            return decode_nak_frame(info, mux);
        }
        // SYNC/ACK/SYNC_ACK control frame.
        return decode_sync_ack_frame(info, seq, ct, mux);
    }

    Err(RlpError::InvalidCtl(ctl))
}

/// Decode a SYNC/ACK/SYNC_ACK control frame from the information field.
fn decode_sync_ack_frame(
    info: &[u8],
    seq: u8,
    control_type: Rlp3ControlType,
    _mux: MuxOption,
) -> Result<Rlp3Frame, RlpError> {
    // SEQ(8) + CTL(6) + INIT_VAR(1) + NAK_PARAM_INCL(1) = 16 bits + FCS(16)
    let init_var = info[14] == 1;
    let nak_param_incl = info[15] == 1;

    // FCS covers bits 0..16.
    let fcs_expected = fcs16(&info[0..16]);
    let fcs_got = get_fcs16(info, 16);
    if fcs_expected != fcs_got {
        return Err(RlpError::FcsInvalid {
            expected: fcs_expected,
            got: fcs_got,
        });
    }

    Ok(Rlp3Frame::Control {
        seq,
        control_type,
        init_var,
        nak_param_incl,
    })
}

/// Decode a NAK control frame from the information field.
fn decode_nak_frame(info: &[u8], _mux: MuxOption) -> Result<Rlp3Frame, RlpError> {
    let len = info.len();
    if len < 20 {
        return Err(RlpError::InsufficientBits {
            expected: 20,
            got: len,
        });
    }
    let seq = get_bits(info, 0, 8) as u8;
    // CTL(6) already verified as NAK.
    let nak_type = get_bits(info, 14, 2) as u8;
    let seq_hi = get_bits(info, 16, 4) as u8;
    let mut pos = 20;

    let payload = match nak_type {
        0b00 => {
            let count = try_get_bits(info, pos, 2)? as usize + 1;
            pos += 2;
            let mut entries = Vec::with_capacity(count);
            for _ in 0..count {
                let first = try_get_bits(info, pos, 12)? as u16;
                pos += 12;
                let last = try_get_bits(info, pos, 12)? as u16;
                pos += 12;
                entries.push(NakGapEntry { first, last });
            }
            NakPayload::Gap(entries)
        }
        0b01 => {
            let count = try_get_bits(info, pos, 2)? as usize + 1;
            pos += 2;
            let mut entries = Vec::with_capacity(count);
            for _ in 0..count {
                let nak_map_seq = try_get_bits(info, pos, 12)? as u16;
                pos += 12;
                let nak_map = try_get_bits(info, pos, 8)? as u8;
                pos += 8;
                entries.push(NakMapEntry {
                    nak_map_seq,
                    nak_map,
                });
            }
            NakPayload::Map(entries)
        }
        0b10 => {
            let count = try_get_bits(info, pos, 2)? as usize + 1;
            pos += 2;
            let mut entries = Vec::with_capacity(count);
            for _ in 0..count {
                let frame_seq = try_get_bits(info, pos, 12)? as u16;
                pos += 12;
                let first_s_seq = try_get_bits(info, pos, 12)? as u16;
                pos += 12;
                let last_s_seq = try_get_bits(info, pos, 12)? as u16;
                pos += 12;
                entries.push(NakSegRangeEntry {
                    frame_seq,
                    first_s_seq,
                    last_s_seq,
                });
            }
            NakPayload::SegmentRange(entries)
        }
        0b11 => {
            let count = try_get_bits(info, pos, 2)? as usize + 1;
            pos += 2;
            let mut entries = Vec::with_capacity(count);
            for _ in 0..count {
                let frame_seq = try_get_bits(info, pos, 12)? as u16;
                pos += 12;
                let first_s_seq = try_get_bits(info, pos, 12)? as u16;
                pos += 12;
                let length_s_seq = try_get_bits(info, pos, 8)? as u8;
                pos += 8;
                entries.push(NakSegLenEntry {
                    frame_seq,
                    first_s_seq,
                    length_s_seq,
                });
            }
            NakPayload::SegmentLength(entries)
        }
        _ => return Err(RlpError::InvalidNakType(nak_type)),
    };

    // Padding_1 octet-aligns the FCS field. FCS covers all fields before it,
    // including Padding_1.
    let fcs_pos = align_to_octet(pos);
    if fcs_pos + 16 > len {
        return Err(RlpError::InsufficientBits {
            expected: fcs_pos + 16,
            got: len,
        });
    }
    let fcs_expected = fcs16(&info[0..fcs_pos]);
    let fcs_got = get_fcs16(info, fcs_pos);
    if fcs_expected != fcs_got {
        return Err(RlpError::FcsInvalid {
            expected: fcs_expected,
            got: fcs_got,
        });
    }

    Ok(Rlp3Frame::Nak {
        seq,
        seq_hi,
        payload,
    })
}

fn align_to_octet(pos: usize) -> usize {
    (pos + 7) & !7
}

/// Decode a segmented data frame from the information field.
fn decode_segmented_frame(info: &[u8], _mux: MuxOption) -> Result<Rlp3Frame, RlpError> {
    let seq = get_bits(info, 0, 8) as u8;
    // CTL(4) = '1000' already verified.
    let sqi = info[12] == 1;
    let last_seg = info[13] == 1;
    let rexmit = info[14] == 1;
    let len = get_bits(info, 15, 5) as usize;
    let mut pos = 20;

    let seq_hi = if sqi {
        let hi = get_bits(info, pos, 4) as u8;
        pos += 4;
        Some(hi)
    } else {
        None
    };

    let s_seq = get_bits(info, pos, 12) as u16;
    pos += 12;

    if sqi {
        pos += 4; // Padding_1
    }

    let mut data = vec![0u8; len];
    for i in 0..len {
        if pos + 8 > info.len() {
            return Err(RlpError::InsufficientBits {
                expected: pos + 8,
                got: info.len(),
            });
        }
        data[i] = get_bits(info, pos, 8) as u8;
        pos += 8;
    }

    Ok(Rlp3Frame::Segmented {
        seq,
        sqi,
        last_seg,
        rexmit,
        seq_hi,
        s_seq,
        data,
    })
}

fn is_idle2_info(info: &[u8]) -> bool {
    info.len() >= 36
        && get_bits(info, 8, 4) == 0b1000
        && info[12] == 1
        && info[13] == 0
        && info[14] == 0
        && get_bits(info, 15, 5) == 0
}

/// Decode a Format B frame.
fn decode_format_b(bits: &[u8], mux: MuxOption, rexmit: bool) -> Result<Rlp3Frame, RlpError> {
    let seq = get_bits(bits, 0, 8) as u8;
    let data_len = mux.format_b_data_len();
    let mut data = vec![0u8; data_len];
    for i in 0..data_len {
        data[i] = get_bits(bits, 8 + i * 8, 8) as u8;
    }
    Ok(Rlp3Frame::DataFormatB { seq, rexmit, data })
}

/// Try to decode a fill or idle frame from Format A information bits.
///
/// Fill: SEQ(8) + CTL(4='1001') + SEQ_HI(4) + Padding.
/// Idle1: SEQ(8) + CTL(4='1010') + SEQ_HI(4) + Padding.
/// For Idle2, use `try_decode_idle2`.
pub fn try_decode_fill_or_idle1(bits: &[u8], mux: MuxOption) -> Result<Rlp3Frame, RlpError> {
    let expected = mux.frame_bits();
    if bits.len() < expected {
        return Err(RlpError::InsufficientBits {
            expected,
            got: bits.len(),
        });
    }

    let info_bits = mux.info_bits();
    let type_offset = info_bits;
    let type_val = get_bits(bits, type_offset, mux.type_bits()) as u8;
    let expected_type = mux.type_format_a();
    if type_val != expected_type {
        return Err(RlpError::InvalidType(type_val));
    }

    let info = &bits[..info_bits];
    let seq = get_bits(info, 0, 8) as u8;
    let ctl = get_bits(info, 8, 4) as u8;
    let seq_hi = get_bits(info, 12, 4) as u8;
    match ctl {
        0b1001 => Ok(Rlp3Frame::Fill { seq, seq_hi }),
        0b1010 => Ok(Rlp3Frame::Idle1 { seq, seq_hi }),
        _ => Err(RlpError::InvalidFrame(format!(
            "fill/idle1 CTL={:#06b}, expected 1001 or 1010",
            ctl
        ))),
    }
}

/// Try to decode an Idle Format 2 frame (no SEQ_HI).
pub fn try_decode_idle2(bits: &[u8], mux: MuxOption) -> Result<Rlp3Frame, RlpError> {
    let expected = mux.frame_bits();
    if bits.len() < expected {
        return Err(RlpError::InsufficientBits {
            expected,
            got: bits.len(),
        });
    }

    let info_bits = mux.info_bits();
    let type_offset = info_bits;
    let type_val = get_bits(bits, type_offset, mux.type_bits()) as u8;
    let expected_type = mux.type_format_a();
    if type_val != expected_type {
        return Err(RlpError::InvalidType(type_val));
    }

    let info = &bits[..info_bits];
    let seq = get_bits(info, 0, 8) as u8;
    if !is_idle2_info(info) {
        return Err(RlpError::InvalidFrame("not an Idle Format 2 frame".into()));
    }

    Ok(Rlp3Frame::Idle2 { seq })
}

// ---------------------------------------------------------------------------
// Sub-rate decoding (less than Rate 1)
// ---------------------------------------------------------------------------

/// Encode a sub-rate Fill/Idle1 frame without a Rate-1 TYPE field.
///
/// Sub-rate fill uses SEQ(8) + CTL(4='1001') + SEQ_HI(4), padded to the
/// physical-rate information bit count.
pub fn encode_sub_rate_fill(
    seq: u8,
    seq_hi: u8,
    num_info_bits: usize,
) -> Result<Vec<u8>, RlpError> {
    if num_info_bits < 16 {
        return Err(RlpError::InsufficientBits {
            expected: 16,
            got: num_info_bits,
        });
    }

    let mut info = vec![0u8; num_info_bits];
    put_bits(&mut info, 0, seq as u32, 8);
    put_bits(&mut info, 8, 0b1001, 4);
    put_bits(&mut info, 12, (seq_hi & 0x0f) as u32, 4);
    Ok(info)
}

/// Decode an RLP Type 3 frame received at less than Rate 1 (half, quarter rate).
///
/// Sub-rate frames do NOT have a TYPE field — the raw information content
/// (§4.2 control, §4.3 data, §4.4 fill, §4.5 idle) is placed directly in
/// the info bits.
///
/// `bits` contains individual bit values (0 or 1).
/// `num_info_bits` is the number of information bits at this rate:
///   half=80, quarter=40.
pub fn decode_sub_rate_frame(bits: &[u8], num_info_bits: usize) -> Result<Rlp3Frame, RlpError> {
    if bits.len() < num_info_bits {
        return Err(RlpError::InsufficientBits {
            expected: num_info_bits,
            got: bits.len(),
        });
    }
    let info = &bits[..num_info_bits];

    // Try fill / idle1: SEQ(8) + CTL(4) + SEQ_HI(4) + Padding.
    if num_info_bits >= 16 {
        let ctl4 = get_bits(info, 8, 4) as u8;
        if ctl4 == 0b1001 || ctl4 == 0b1010 {
            let seq = get_bits(info, 0, 8) as u8;
            let seq_hi = get_bits(info, 12, 4) as u8;
            return if ctl4 == 0b1001 {
                Ok(Rlp3Frame::Fill { seq, seq_hi })
            } else {
                Ok(Rlp3Frame::Idle1 { seq, seq_hi })
            };
        }
    }

    // Try idle2: zero-length segmented-format idle.
    if num_info_bits >= 36 && is_idle2_info(info) {
        let seq = get_bits(info, 0, 8) as u8;
        return Ok(Rlp3Frame::Idle2 { seq });
    }

    // Try unsegmented data: SEQ(8) + CTL(1='0') + REXMIT(1) + LEN(6) + Data(8*LEN)
    if num_info_bits >= 16 && info[8] == 0 {
        let rexmit = info[9] == 1;
        let len = get_bits(info, 10, 6) as usize;
        let data_start = 16;
        let data_end = data_start + len * 8;
        if len > 0 && data_end <= num_info_bits {
            let seq = get_bits(info, 0, 8) as u8;
            let mut data = vec![0u8; len];
            for i in 0..len {
                data[i] = get_bits(info, data_start + i * 8, 8) as u8;
            }
            return Ok(Rlp3Frame::Data { seq, rexmit, data });
        }
    }

    // Try segmented data: CTL starts with '1000', same as rate-1 Format A.
    if num_info_bits >= 20 && info[8] == 1 && info[9] == 0 && info[10] == 0 && info[11] == 0 {
        if let Ok(frame) = decode_segmented_frame(info, MuxOption::Odd) {
            return Ok(frame);
        }
    }

    let mut control_error = None;

    // Try control frame: SEQ(8) + CTL(6) + INIT_VAR(1) + NAK_PARAM_INCL(1) + FCS(16) = 32 bits
    if num_info_bits >= 32 {
        let ctl = get_bits(info, 8, 6) as u8;
        if let Some(ct) = Rlp3ControlType::from_ctl_bits(ctl) {
            if ct != Rlp3ControlType::Nak {
                // SYNC / SYNC_ACK / ACK
                let fcs_expected = fcs16(&info[0..16]);
                let fcs_got = get_fcs16(info, 16);
                if fcs_expected == fcs_got {
                    let seq = get_bits(info, 0, 8) as u8;
                    let init_var = info[14] == 1;
                    let nak_param_incl = info[15] == 1;
                    return Ok(Rlp3Frame::Control {
                        seq,
                        control_type: ct,
                        init_var,
                        nak_param_incl,
                    });
                }
                control_error = Some(RlpError::FcsInvalid {
                    expected: fcs_expected,
                    got: fcs_got,
                });
            }
            // NAK at sub-rate: needs at least ~44 bits for the smallest NAK.
            // Try at half rate only.
            if ct == Rlp3ControlType::Nak && num_info_bits >= 60 {
                match decode_nak_frame(info, MuxOption::Odd) {
                    Ok(f) => return Ok(f),
                    Err(e) => control_error = Some(e),
                }
            }
        }
    }

    if let Some(e) = control_error {
        return Err(e);
    }

    Err(RlpError::InvalidFrame(
        "no valid sub-rate frame found".into(),
    ))
}

/// Return a short structural diagnosis for a sub-rate frame that failed decode.
pub fn diagnose_sub_rate_frame(bits: &[u8], num_info_bits: usize) -> String {
    if bits.len() < num_info_bits {
        return format!(
            "short_sub_rate expected_info_bits={} got_bits={}",
            num_info_bits,
            bits.len()
        );
    }
    let info = &bits[..num_info_bits];
    if num_info_bits < 16 {
        return format!("unsupported_sub_rate_info_bits={}", num_info_bits);
    }

    let seq = get_bits(info, 0, 8) as u8;
    let ctl4 = get_bits(info, 8, 4) as u8;
    let ctl6 = if num_info_bits >= 14 {
        Some(get_bits(info, 8, 6) as u8)
    } else {
        None
    };

    if info[8] == 0 {
        let rexmit = info[9] == 1;
        let len = get_bits(info, 10, 6) as usize;
        let end = 16 + len * 8;
        return format!(
            "candidate=data seq={} rexmit={} len={} data_end={} info_bits={}",
            seq, rexmit, len, end, num_info_bits
        );
    }

    if ctl4 == 0b1001 || ctl4 == 0b1010 {
        let seq_hi = get_bits(info, 12, 4) as u8;
        return format!(
            "candidate={} seq={} seq_hi={}",
            if ctl4 == 0b1001 { "fill" } else { "idle1" },
            seq,
            seq_hi
        );
    }

    if ctl4 == 0b1000 {
        return format!("candidate=segmented_or_idle2 seq={} ctl4=0b1000", seq);
    }

    if let Some(ctl) = ctl6 {
        if let Some(ct) = Rlp3ControlType::from_ctl_bits(ctl) {
            if ct == Rlp3ControlType::Nak {
                let nak_type = if num_info_bits >= 16 {
                    Some(get_bits(info, 14, 2) as u8)
                } else {
                    None
                };
                let seq_hi = if num_info_bits >= 20 {
                    Some(get_bits(info, 16, 4) as u8)
                } else {
                    None
                };
                return format!(
                    "candidate=nak seq={} ctl=0b{:06b} nak_type={:?} seq_hi={:?} decode={:?}",
                    seq,
                    ctl,
                    nak_type,
                    seq_hi,
                    decode_nak_frame(info, MuxOption::Odd)
                );
            }

            let fcs_expected = fcs16(&info[0..16]);
            let fcs_got = get_fcs16(info, 16);
            return format!(
                "candidate=control seq={} ctl={:?}/0b{:06b} fcs_expected=0x{:04x} fcs_got=0x{:04x}",
                seq, ct, ctl, fcs_expected, fcs_got
            );
        }
    }

    format!(
        "unknown_sub_rate seq={} ctl4=0b{:04b} ctl6={}",
        seq,
        ctl4,
        ctl6.map(|v| format!("0b{:06b}", v))
            .unwrap_or_else(|| "n/a".to_string())
    )
}

/// Map a physical layer rate to the number of info bits for sub-rate frames.
pub fn sub_rate_info_bits(rate: crate::rlp3_session::FrameRate) -> Option<usize> {
    match rate {
        crate::rlp3_session::FrameRate::Half => Some(80),
        crate::rlp3_session::FrameRate::Quarter => Some(40),
        _ => None, // Full uses Format A; Eighth (16 bits) too small; Blank = nothing
    }
}

// ---------------------------------------------------------------------------
// RLP_BLOB (Section 4.6)
// ---------------------------------------------------------------------------

/// RLP_BLOB configuration exchanged during service negotiation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RlpBlob {
    /// Round-trip time in 20ms units. 0 means use SYNC exchange for RTT estimation.
    pub rtt: u8,
    /// Number of NAK rounds in the forward direction (0..7).
    pub nak_rounds_fwd: u8,
    /// Number of NAK rounds in the reverse direction (0..7).
    pub nak_rounds_rev: u8,
    /// Per-round NAK limits, forward direction. Length = nak_rounds_fwd.
    pub naks_per_round_fwd: Vec<u8>,
    /// Per-round NAK limits, reverse direction. Length = nak_rounds_rev.
    pub naks_per_round_rev: Vec<u8>,
}

impl RlpBlob {
    /// Encode this RLP_BLOB into a bit vector (0/1 u8 values).
    pub fn encode(&self) -> Vec<u8> {
        let mut bits = Vec::new();
        // RLP_BLOB_TYPE(3) = '001' (non-AES)
        push_bits(&mut bits, 0b001, 3);
        // RTT(8)
        push_bits(&mut bits, self.rtt as u32, 8);
        // NAK_ROUNDS_FWD(3)
        push_bits(&mut bits, self.nak_rounds_fwd as u32, 3);
        // NAK_ROUNDS_REV(3)
        push_bits(&mut bits, self.nak_rounds_rev as u32, 3);
        // Per-round NAK limits
        for i in 0..self.nak_rounds_fwd as usize {
            let val = self.naks_per_round_fwd.get(i).copied().unwrap_or(0);
            push_bits(&mut bits, val as u32, 3);
        }
        for i in 0..self.nak_rounds_rev as usize {
            let val = self.naks_per_round_rev.get(i).copied().unwrap_or(0);
            push_bits(&mut bits, val as u32, 3);
        }
        bits
    }

    /// Decode an RLP_BLOB from a bit slice.
    pub fn decode(bits: &[u8]) -> Result<Self, RlpError> {
        let mut pos = 0;

        if bits.len() < 17 {
            return Err(RlpError::InvalidBlob("too short".into()));
        }

        let blob_type = get_bits(bits, pos, 3) as u8;
        pos += 3;
        if blob_type != 0b001 {
            return Err(RlpError::InvalidBlob(format!(
                "unsupported RLP_BLOB_TYPE: {}",
                blob_type
            )));
        }

        let rtt = get_bits(bits, pos, 8) as u8;
        pos += 8;
        let nak_rounds_fwd = get_bits(bits, pos, 3) as u8;
        pos += 3;
        let nak_rounds_rev = get_bits(bits, pos, 3) as u8;
        pos += 3;

        let total_needed = pos + (nak_rounds_fwd as usize + nak_rounds_rev as usize) * 3;
        if bits.len() < total_needed {
            return Err(RlpError::InvalidBlob("too short for NAK rounds".into()));
        }

        let mut naks_per_round_fwd = Vec::with_capacity(nak_rounds_fwd as usize);
        for _ in 0..nak_rounds_fwd {
            naks_per_round_fwd.push(get_bits(bits, pos, 3) as u8);
            pos += 3;
        }
        let mut naks_per_round_rev = Vec::with_capacity(nak_rounds_rev as usize);
        for _ in 0..nak_rounds_rev {
            naks_per_round_rev.push(get_bits(bits, pos, 3) as u8);
            pos += 3;
        }

        Ok(RlpBlob {
            rtt,
            nak_rounds_fwd,
            nak_rounds_rev,
            naks_per_round_fwd,
            naks_per_round_rev,
        })
    }
}

/// Push `n` bits of `val` (MSB first) onto a bit vector.
fn push_bits(bits: &mut Vec<u8>, val: u32, n: usize) {
    for i in 0..n {
        bits.push(((val >> (n - 1 - i)) & 1) as u8);
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    const MUX: MuxOption = MuxOption::Odd;

    // -----------------------------------------------------------------------
    // 1. Round-trip: SYNC, SYNC/ACK, ACK control frames
    // -----------------------------------------------------------------------

    #[test]
    fn test_sync_round_trip() {
        let frame = Rlp3Frame::Control {
            seq: 0x00,
            control_type: Rlp3ControlType::Sync,
            init_var: true,
            nak_param_incl: false,
        };
        let bits = frame.encode(MUX).unwrap();
        assert_eq!(bits.len(), MUX.frame_bits());
        let decoded = decode_rlp3_frame(&bits, MUX).unwrap();
        assert_eq!(decoded, frame);
    }

    #[test]
    fn test_sync_ack_round_trip() {
        let frame = Rlp3Frame::Control {
            seq: 0x42,
            control_type: Rlp3ControlType::SyncAck,
            init_var: false,
            nak_param_incl: false,
        };
        let bits = frame.encode(MUX).unwrap();
        let decoded = decode_rlp3_frame(&bits, MUX).unwrap();
        assert_eq!(decoded, frame);
    }

    #[test]
    fn test_ack_round_trip() {
        let frame = Rlp3Frame::Control {
            seq: 0xFF,
            control_type: Rlp3ControlType::Ack,
            init_var: true,
            nak_param_incl: false,
        };
        let bits = frame.encode(MUX).unwrap();
        let decoded = decode_rlp3_frame(&bits, MUX).unwrap();
        assert_eq!(decoded, frame);
    }

    #[test]
    fn test_sync_init_var_false_round_trip() {
        let frame = Rlp3Frame::Control {
            seq: 0x10,
            control_type: Rlp3ControlType::Sync,
            init_var: false,
            nak_param_incl: false,
        };
        let bits = frame.encode(MUX).unwrap();
        let decoded = decode_rlp3_frame(&bits, MUX).unwrap();
        assert_eq!(decoded, frame);
    }

    // -----------------------------------------------------------------------
    // 2. Round-trip: NAK frames
    // -----------------------------------------------------------------------

    #[test]
    fn test_nak_gap_round_trip() {
        let frame = Rlp3Frame::Nak {
            seq: 0x05,
            seq_hi: 0x03,
            payload: NakPayload::Gap(vec![NakGapEntry {
                first: 0x010,
                last: 0x020,
            }]),
        };
        let bits = frame.encode(MUX).unwrap();
        assert_eq!(bits.len(), MUX.frame_bits());
        let decoded = decode_rlp3_frame(&bits, MUX).unwrap();
        assert_eq!(decoded, frame);
    }

    #[test]
    fn test_sub_rate_nak_from_live_ms_has_octet_aligned_fcs() {
        let bits: Vec<u8> =
            "00100011110000000000000000001000110000001010100011001111000100000000000000000000"
                .chars()
                .map(|c| match c {
                    '0' => 0,
                    '1' => 1,
                    _ => panic!("invalid bit"),
                })
                .collect();

        let decoded = decode_sub_rate_frame(&bits, 80).unwrap();

        assert_eq!(
            decoded,
            Rlp3Frame::Nak {
                seq: 35,
                seq_hi: 0,
                payload: NakPayload::Gap(vec![NakGapEntry {
                    first: 35,
                    last: 42,
                }]),
            }
        );
    }

    #[test]
    fn test_nak_gap_multiple_entries() {
        let frame = Rlp3Frame::Nak {
            seq: 0xAA,
            seq_hi: 0x0F,
            payload: NakPayload::Gap(vec![
                NakGapEntry {
                    first: 0x100,
                    last: 0x110,
                },
                NakGapEntry {
                    first: 0x200,
                    last: 0x220,
                },
            ]),
        };
        let bits = frame.encode(MUX).unwrap();
        let decoded = decode_rlp3_frame(&bits, MUX).unwrap();
        assert_eq!(decoded, frame);
    }

    #[test]
    fn test_nak_map_round_trip() {
        let frame = Rlp3Frame::Nak {
            seq: 0x07,
            seq_hi: 0x01,
            payload: NakPayload::Map(vec![NakMapEntry {
                nak_map_seq: 0x0FF,
                nak_map: 0b10101010,
            }]),
        };
        let bits = frame.encode(MUX).unwrap();
        let decoded = decode_rlp3_frame(&bits, MUX).unwrap();
        assert_eq!(decoded, frame);
    }

    #[test]
    fn test_nak_seg_range_round_trip() {
        let frame = Rlp3Frame::Nak {
            seq: 0x0C,
            seq_hi: 0x02,
            payload: NakPayload::SegmentRange(vec![NakSegRangeEntry {
                frame_seq: 0x123,
                first_s_seq: 0x001,
                last_s_seq: 0x00F,
            }]),
        };
        let bits = frame.encode(MUX).unwrap();
        let decoded = decode_rlp3_frame(&bits, MUX).unwrap();
        assert_eq!(decoded, frame);
    }

    #[test]
    fn test_nak_seg_length_round_trip() {
        let frame = Rlp3Frame::Nak {
            seq: 0x0D,
            seq_hi: 0x00,
            payload: NakPayload::SegmentLength(vec![NakSegLenEntry {
                frame_seq: 0xABC,
                first_s_seq: 0x010,
                length_s_seq: 5,
            }]),
        };
        let bits = frame.encode(MUX).unwrap();
        let decoded = decode_rlp3_frame(&bits, MUX).unwrap();
        assert_eq!(decoded, frame);
    }

    // -----------------------------------------------------------------------
    // 3. Round-trip: unsegmented data frames
    // -----------------------------------------------------------------------

    #[test]
    fn test_data_len_0() {
        let frame = Rlp3Frame::Data {
            seq: 0x01,
            rexmit: false,
            data: vec![],
        };
        let bits = frame.encode(MUX).unwrap();
        assert_eq!(bits.len(), MUX.frame_bits());
        let decoded = decode_rlp3_frame(&bits, MUX).unwrap();
        assert_eq!(decoded, frame);
    }

    #[test]
    fn test_data_len_1() {
        let frame = Rlp3Frame::Data {
            seq: 0x30,
            rexmit: false,
            data: vec![0xDE],
        };
        let bits = frame.encode(MUX).unwrap();
        let decoded = decode_rlp3_frame(&bits, MUX).unwrap();
        assert_eq!(decoded, frame);
    }

    #[test]
    fn test_data_len_max_odd() {
        let frame = Rlp3Frame::Data {
            seq: 0x7F,
            rexmit: true,
            data: vec![0xAB; 19],
        };
        let bits = frame.encode(MUX).unwrap();
        let decoded = decode_rlp3_frame(&bits, MUX).unwrap();
        assert_eq!(decoded, frame);
    }

    #[test]
    fn test_data_len_max_even() {
        let mux = MuxOption::Even;
        let frame = Rlp3Frame::Data {
            seq: 0x7F,
            rexmit: false,
            data: vec![0xCD; 31],
        };
        let bits = frame.encode(mux).unwrap();
        assert_eq!(bits.len(), mux.frame_bits());
        let decoded = decode_rlp3_frame(&bits, mux).unwrap();
        assert_eq!(decoded, frame);
    }

    #[test]
    fn test_data_rexmit_flag() {
        let frame = Rlp3Frame::Data {
            seq: 0x55,
            rexmit: true,
            data: vec![0x01, 0x02, 0x03],
        };
        let bits = frame.encode(MUX).unwrap();
        let decoded = decode_rlp3_frame(&bits, MUX).unwrap();
        assert_eq!(decoded, frame);
    }

    // -----------------------------------------------------------------------
    // 4. Round-trip: segmented data frames
    // -----------------------------------------------------------------------

    #[test]
    fn test_segmented_no_sqi() {
        let frame = Rlp3Frame::Segmented {
            seq: 0x10,
            sqi: false,
            last_seg: false,
            rexmit: false,
            seq_hi: None,
            s_seq: 0x001,
            data: vec![0xAA, 0xBB],
        };
        let bits = frame.encode(MUX).unwrap();
        assert_eq!(bits.len(), MUX.frame_bits());
        let decoded = decode_rlp3_frame(&bits, MUX).unwrap();
        assert_eq!(decoded, frame);
    }

    #[test]
    fn test_segmented_with_sqi() {
        let frame = Rlp3Frame::Segmented {
            seq: 0x20,
            sqi: true,
            last_seg: true,
            rexmit: true,
            seq_hi: Some(0x0F),
            s_seq: 0xFFF,
            data: vec![0x11, 0x22, 0x33],
        };
        let bits = frame.encode(MUX).unwrap();
        let decoded = decode_rlp3_frame(&bits, MUX).unwrap();
        assert_eq!(decoded, frame);
    }

    #[test]
    fn test_segmented_empty_data() {
        let frame = Rlp3Frame::Segmented {
            seq: 0x30,
            sqi: false,
            last_seg: true,
            rexmit: false,
            seq_hi: None,
            s_seq: 0x100,
            data: vec![],
        };
        let bits = frame.encode(MUX).unwrap();
        let decoded = decode_rlp3_frame(&bits, MUX).unwrap();
        assert_eq!(decoded, frame);
    }

    // -----------------------------------------------------------------------
    // 5. Round-trip: Format B frames
    // -----------------------------------------------------------------------

    #[test]
    fn test_format_b_new_odd() {
        let frame = Rlp3Frame::DataFormatB {
            seq: 0x42,
            rexmit: false,
            data: vec![0xDE; 20],
        };
        let bits = frame.encode(MUX).unwrap();
        assert_eq!(bits.len(), MUX.frame_bits());
        let decoded = decode_rlp3_frame(&bits, MUX).unwrap();
        assert_eq!(decoded, frame);
    }

    #[test]
    fn test_format_b_rexmit_odd() {
        let frame = Rlp3Frame::DataFormatB {
            seq: 0x99,
            rexmit: true,
            data: vec![0x01; 20],
        };
        let bits = frame.encode(MUX).unwrap();
        let decoded = decode_rlp3_frame(&bits, MUX).unwrap();
        assert_eq!(decoded, frame);
    }

    #[test]
    fn test_format_b_even() {
        let mux = MuxOption::Even;
        let frame = Rlp3Frame::DataFormatB {
            seq: 0x77,
            rexmit: false,
            data: vec![0xFE; 32],
        };
        let bits = frame.encode(mux).unwrap();
        assert_eq!(bits.len(), mux.frame_bits());
        let decoded = decode_rlp3_frame(&bits, mux).unwrap();
        assert_eq!(decoded, frame);
    }

    #[test]
    fn test_format_b_wrong_data_len() {
        let frame = Rlp3Frame::DataFormatB {
            seq: 0x00,
            rexmit: false,
            data: vec![0x00; 10], // wrong length
        };
        let result = frame.encode(MUX);
        assert!(result.is_err());
        if let Err(RlpError::DataTooLong { max: 20, got: 10 }) = result {
            // expected
        } else {
            panic!("unexpected error: {:?}", result);
        }
    }

    // -----------------------------------------------------------------------
    // 6. Round-trip: fill and idle frames
    // -----------------------------------------------------------------------

    #[test]
    fn test_fill_round_trip() {
        let frame = Rlp3Frame::Fill {
            seq: 0x0A,
            seq_hi: 0x03,
        };
        let bits = frame.encode(MUX).unwrap();
        assert_eq!(bits.len(), MUX.frame_bits());
        let decoded = try_decode_fill_or_idle1(&bits, MUX).unwrap();
        assert_eq!(
            decoded,
            Rlp3Frame::Fill {
                seq: 0x0A,
                seq_hi: 0x03
            }
        );
    }

    #[test]
    fn test_idle1_round_trip() {
        let frame = Rlp3Frame::Idle1 {
            seq: 0xBB,
            seq_hi: 0x07,
        };
        let bits = frame.encode(MUX).unwrap();
        let decoded = try_decode_fill_or_idle1(&bits, MUX).unwrap();
        assert_eq!(decoded, frame);
        assert_eq!(decode_rlp3_frame(&bits, MUX).unwrap(), frame);
    }

    #[test]
    fn test_idle2_round_trip() {
        let frame = Rlp3Frame::Idle2 { seq: 0x55 };
        let bits = frame.encode(MUX).unwrap();
        assert_eq!(bits.len(), MUX.frame_bits());
        let decoded = try_decode_idle2(&bits, MUX).unwrap();
        assert_eq!(decoded, frame);
    }

    #[test]
    fn test_idle2_even() {
        let mux = MuxOption::Even;
        let frame = Rlp3Frame::Idle2 { seq: 0xCC };
        let bits = frame.encode(mux).unwrap();
        assert_eq!(bits.len(), mux.frame_bits());
        let decoded = try_decode_idle2(&bits, mux).unwrap();
        assert_eq!(decoded, frame);
    }

    #[test]
    fn test_full_rate_idle1_ctl_1010_from_live_trace() {
        let mut bits = vec![0u8; MUX.frame_bits()];
        put_bits(&mut bits, 0, 0, 8);
        put_bits(&mut bits, 8, 0b1010, 4);
        put_bits(&mut bits, 12, 0, 4);
        put_bits(
            &mut bits,
            MUX.info_bits(),
            MUX.type_format_a() as u32,
            MUX.type_bits(),
        );

        let decoded = decode_rlp3_frame(&bits, MUX).unwrap();
        assert_eq!(decoded, Rlp3Frame::Idle1 { seq: 0, seq_hi: 0 });
    }

    // -----------------------------------------------------------------------
    // 7. FCS-16 validation
    // -----------------------------------------------------------------------

    #[test]
    fn test_fcs_correct() {
        let frame = Rlp3Frame::Control {
            seq: 0x00,
            control_type: Rlp3ControlType::Sync,
            init_var: true,
            nak_param_incl: false,
        };
        let bits = frame.encode(MUX).unwrap();
        // Should decode without error.
        assert!(decode_rlp3_frame(&bits, MUX).is_ok());
    }

    #[test]
    fn test_fcs_corrupted() {
        let frame = Rlp3Frame::Control {
            seq: 0x00,
            control_type: Rlp3ControlType::Sync,
            init_var: true,
            nak_param_incl: false,
        };
        let mut bits = frame.encode(MUX).unwrap();
        // Corrupt a bit in the FCS area (bit 16 is first bit of FCS).
        bits[16] ^= 1;
        let result = decode_rlp3_frame(&bits, MUX);
        assert!(matches!(result, Err(RlpError::FcsInvalid { .. })));
    }

    #[test]
    fn test_fill_rejects_non_fill_idle_ctl() {
        let frame = Rlp3Frame::Fill {
            seq: 0x0A,
            seq_hi: 0x03,
        };
        let mut bits = frame.encode(MUX).unwrap();
        put_bits(&mut bits, 8, 0b1011, 4);
        let result = try_decode_fill_or_idle1(&bits, MUX);
        assert!(matches!(result, Err(RlpError::InvalidFrame(_))));
    }

    #[test]
    fn test_idle2_rejects_bad_idle2_header() {
        let frame = Rlp3Frame::Idle2 { seq: 0x55 };
        let mut bits = frame.encode(MUX).unwrap();
        bits[12] = 0; // SQI must be 1 for Idle Format 2.
        let result = try_decode_idle2(&bits, MUX);
        assert!(matches!(result, Err(RlpError::InvalidFrame(_))));
    }

    #[test]
    fn test_fcs_corrupted_nak() {
        let frame = Rlp3Frame::Nak {
            seq: 0x05,
            seq_hi: 0x03,
            payload: NakPayload::Gap(vec![NakGapEntry {
                first: 0x010,
                last: 0x020,
            }]),
        };
        let mut bits = frame.encode(MUX).unwrap();
        // Corrupt a payload bit.
        bits[25] ^= 1;
        let result = decode_rlp3_frame(&bits, MUX);
        assert!(matches!(result, Err(RlpError::FcsInvalid { .. })));
    }

    // -----------------------------------------------------------------------
    // 8. MAX_LEN boundary enforcement
    // -----------------------------------------------------------------------

    #[test]
    fn test_data_too_long_odd() {
        let frame = Rlp3Frame::Data {
            seq: 0x01,
            rexmit: false,
            data: vec![0x00; 20], // max is 19 for odd mux
        };
        let result = frame.encode(MUX);
        assert!(matches!(
            result,
            Err(RlpError::DataTooLong { max: 19, got: 20 })
        ));
    }

    #[test]
    fn test_data_too_long_even() {
        let mux = MuxOption::Even;
        let frame = Rlp3Frame::Data {
            seq: 0x01,
            rexmit: false,
            data: vec![0x00; 32], // max is 31 for even mux
        };
        let result = frame.encode(mux);
        assert!(matches!(
            result,
            Err(RlpError::DataTooLong { max: 31, got: 32 })
        ));
    }

    #[test]
    fn test_data_at_max_boundary_odd() {
        let frame = Rlp3Frame::Data {
            seq: 0x01,
            rexmit: false,
            data: vec![0xFF; 19],
        };
        assert!(frame.encode(MUX).is_ok());
    }

    #[test]
    fn test_insufficient_bits() {
        let bits = vec![0u8; 10]; // way too short
        let result = decode_rlp3_frame(&bits, MUX);
        assert!(matches!(
            result,
            Err(RlpError::InsufficientBits {
                expected: 171,
                got: 10
            })
        ));
    }

    // -----------------------------------------------------------------------
    // 9. RLP_BLOB encode/decode round-trip
    // -----------------------------------------------------------------------

    #[test]
    fn test_rlp_blob_round_trip() {
        let blob = RlpBlob {
            rtt: 5,
            nak_rounds_fwd: 3,
            nak_rounds_rev: 2,
            naks_per_round_fwd: vec![1, 2, 3],
            naks_per_round_rev: vec![4, 5],
        };
        let bits = blob.encode();
        let decoded = RlpBlob::decode(&bits).unwrap();
        assert_eq!(decoded, blob);
    }

    #[test]
    fn test_rlp_blob_zero_rounds() {
        let blob = RlpBlob {
            rtt: 0,
            nak_rounds_fwd: 0,
            nak_rounds_rev: 0,
            naks_per_round_fwd: vec![],
            naks_per_round_rev: vec![],
        };
        let bits = blob.encode();
        let decoded = RlpBlob::decode(&bits).unwrap();
        assert_eq!(decoded, blob);
    }

    #[test]
    fn test_rlp_blob_max_rtt() {
        let blob = RlpBlob {
            rtt: 255,
            nak_rounds_fwd: 1,
            nak_rounds_rev: 1,
            naks_per_round_fwd: vec![7],
            naks_per_round_rev: vec![7],
        };
        let bits = blob.encode();
        let decoded = RlpBlob::decode(&bits).unwrap();
        assert_eq!(decoded, blob);
    }

    #[test]
    fn test_rlp_blob_too_short() {
        let bits = vec![0u8; 5];
        assert!(RlpBlob::decode(&bits).is_err());
    }

    #[test]
    fn test_rlp_blob_bad_type() {
        // Encode valid blob then change the type field.
        let blob = RlpBlob {
            rtt: 0,
            nak_rounds_fwd: 0,
            nak_rounds_rev: 0,
            naks_per_round_fwd: vec![],
            naks_per_round_rev: vec![],
        };
        let mut bits = blob.encode();
        // Set RLP_BLOB_TYPE to '010' instead of '001'.
        bits[0] = 0;
        bits[1] = 1;
        bits[2] = 0;
        assert!(matches!(
            RlpBlob::decode(&bits),
            Err(RlpError::InvalidBlob(_))
        ));
    }

    // -----------------------------------------------------------------------
    // Additional coverage
    // -----------------------------------------------------------------------

    #[test]
    fn test_data_various_payload_patterns() {
        // Verify data integrity with a recognizable byte pattern.
        let data: Vec<u8> = (0..19).collect();
        let frame = Rlp3Frame::Data {
            seq: 0xEE,
            rexmit: false,
            data,
        };
        let bits = frame.encode(MUX).unwrap();
        let decoded = decode_rlp3_frame(&bits, MUX).unwrap();
        assert_eq!(decoded, frame);
    }

    #[test]
    fn test_format_b_data_integrity() {
        let data: Vec<u8> = (0..20).collect();
        let frame = Rlp3Frame::DataFormatB {
            seq: 0xDD,
            rexmit: false,
            data,
        };
        let bits = frame.encode(MUX).unwrap();
        let decoded = decode_rlp3_frame(&bits, MUX).unwrap();
        assert_eq!(decoded, frame);
    }

    #[test]
    fn test_supplemental_format_c_uses_170_bits() {
        let data: Vec<u8> = (0..20).collect();
        let bits = encode_supplemental_format_c(0x34, false, &data).unwrap();

        assert_eq!(bits.len(), 170);
        assert_eq!(get_bits(&bits, 0, 2), 0b10);
        assert_eq!(get_bits(&bits, 2, 8), 0x34);
        assert_eq!(get_bits(&bits, 10, 8), 0);
        assert_eq!(get_bits(&bits, 162, 8), 19);
    }

    #[test]
    fn test_supplemental_format_c_uses_346_bits() {
        let data: Vec<u8> = (0..42).collect();
        let bits = encode_supplemental_format_c_block(0x34, false, &data, 346).unwrap();

        assert_eq!(bits.len(), 346);
        assert_eq!(get_bits(&bits, 0, 2), 0b10);
        assert_eq!(get_bits(&bits, 2, 8), 0x34);
        assert_eq!(get_bits(&bits, 10, 8), 0);
        assert_eq!(get_bits(&bits, 338, 8), 41);
    }

    #[test]
    fn test_supplemental_format_c_rejects_short_data() {
        let result = encode_supplemental_format_c(0, false, &[0u8; 19]);
        assert!(matches!(result, Err(RlpError::InvalidFrame(_))));
    }

    #[test]
    fn test_supplemental_format_d_carries_seq_hi() {
        let data: Vec<u8> = (0..40).collect();
        let bits = encode_supplemental_format_d_block(0x34, Some(0x7), false, &data, 346).unwrap();

        assert_eq!(bits.len(), 346);
        assert_eq!(get_bits(&bits, 0, 2), 0b00);
        assert_eq!(get_bits(&bits, 2, 8), 0x34);
        assert_eq!(get_bits(&bits, 10, 1), 0);
        assert_eq!(get_bits(&bits, 11, 1), 1);
        assert_eq!(get_bits(&bits, 12, 1), 1);
        assert_eq!(get_bits(&bits, 13, 1), 0);
        assert_eq!(get_bits(&bits, 14, 8), 40);
        assert_eq!(get_bits(&bits, 22, 4), 0x7);
        assert_eq!(get_bits(&bits, 26, 8), 0);
        assert_eq!(get_bits(&bits, 338, 8), 39);
    }

    #[test]
    fn test_supplemental_format_d_uses_170_bits() {
        let data: Vec<u8> = (0..18).collect();
        let bits = encode_supplemental_format_d_block(0x12, Some(0x1), true, &data, 170).unwrap();

        assert_eq!(bits.len(), 170);
        assert_eq!(get_bits(&bits, 0, 2), 0b00);
        assert_eq!(get_bits(&bits, 2, 8), 0x12);
        assert_eq!(get_bits(&bits, 13, 1), 1);
        assert_eq!(get_bits(&bits, 14, 8), 18);
        assert_eq!(get_bits(&bits, 22, 4), 0x1);
        assert_eq!(get_bits(&bits, 26, 8), 0);
        assert_eq!(get_bits(&bits, 162, 8), 17);
    }

    #[test]
    fn test_fcs16_known_value() {
        // Verify the FCS function produces a consistent result.
        let data = [0u8; 8]; // 8 zero bits
        let crc1 = fcs16(&data);
        let crc2 = fcs16(&data);
        assert_eq!(crc1, crc2);
        // Non-zero input should differ.
        let data2 = [1, 0, 1, 0, 1, 0, 1, 0];
        assert_ne!(fcs16(&data), fcs16(&data2));
    }

    #[test]
    fn test_invalid_type_field() {
        // Create a valid frame then corrupt the TYPE field.
        let frame = Rlp3Frame::Data {
            seq: 0x01,
            rexmit: false,
            data: vec![0x42],
        };
        let mut bits = frame.encode(MUX).unwrap();
        // Set TYPE to '000' (invalid for odd mux).
        bits[168] = 0;
        bits[169] = 0;
        bits[170] = 0;
        let result = decode_rlp3_frame(&bits, MUX);
        assert!(matches!(result, Err(RlpError::InvalidType(0b000))));
    }

    // -----------------------------------------------------------------------
    // Sub-rate decode tests
    // -----------------------------------------------------------------------

    #[test]
    fn test_sub_rate_sync_quarter_rate() {
        // Build a SYNC control frame's raw info field (no TYPE), padded to 40 bits.
        let frame = Rlp3Frame::Control {
            seq: 0,
            control_type: Rlp3ControlType::Sync,
            init_var: true,
            nak_param_incl: false,
        };
        // Encode at full rate, then extract just the info field (first 168 bits, no TYPE).
        let full = frame.encode(MUX).unwrap();
        // The info field content is the first 168 bits. For a SYNC frame,
        // only the first 32 bits matter (SEQ+CTL+INIT_VAR+NAK+FCS), rest is padding.
        // At quarter rate we have 40 info bits.
        let mut sub_bits = vec![0u8; 40];
        sub_bits[..32].copy_from_slice(&full[..32]);
        // Remaining 8 bits are padding zeros.

        let decoded = decode_sub_rate_frame(&sub_bits, 40).unwrap();
        assert!(matches!(
            decoded,
            Rlp3Frame::Control {
                seq: 0,
                control_type: Rlp3ControlType::Sync,
                init_var: true,
                nak_param_incl: false,
            }
        ));
    }

    #[test]
    fn test_sub_rate_sync_ack_half_rate() {
        let frame = Rlp3Frame::Control {
            seq: 5,
            control_type: Rlp3ControlType::SyncAck,
            init_var: true,
            nak_param_incl: false,
        };
        let full = frame.encode(MUX).unwrap();
        let mut sub_bits = vec![0u8; 80];
        sub_bits[..32].copy_from_slice(&full[..32]);

        let decoded = decode_sub_rate_frame(&sub_bits, 80).unwrap();
        assert!(matches!(
            decoded,
            Rlp3Frame::Control {
                seq: 5,
                control_type: Rlp3ControlType::SyncAck,
                init_var: true,
                nak_param_incl: false,
            }
        ));
    }

    #[test]
    fn test_sub_rate_ack_quarter_rate() {
        let frame = Rlp3Frame::Control {
            seq: 3,
            control_type: Rlp3ControlType::Ack,
            init_var: false,
            nak_param_incl: false,
        };
        let full = frame.encode(MUX).unwrap();
        let mut sub_bits = vec![0u8; 40];
        sub_bits[..32].copy_from_slice(&full[..32]);

        let decoded = decode_sub_rate_frame(&sub_bits, 40).unwrap();
        assert!(matches!(
            decoded,
            Rlp3Frame::Control {
                seq: 3,
                control_type: Rlp3ControlType::Ack,
                init_var: false,
                ..
            }
        ));
    }

    #[test]
    fn test_sub_rate_data_half_rate() {
        // Unsegmented data at half rate: 80 bits = SEQ(8)+CTL(1)+REXMIT(1)+LEN(6)+Data
        // Max LEN at 80 bits: (80 - 16) / 8 = 8 bytes
        let frame = Rlp3Frame::Data {
            seq: 1,
            rexmit: false,
            data: vec![0xAB, 0xCD],
        };
        let full = frame.encode(MUX).unwrap();
        // Extract info field only (no TYPE) — first 168 bits.
        // At half rate, take first 80 bits.
        let mut sub_bits = vec![0u8; 80];
        let copy_len = 80.min(full.len());
        sub_bits[..copy_len].copy_from_slice(&full[..copy_len]);

        let decoded = decode_sub_rate_frame(&sub_bits, 80).unwrap();
        match decoded {
            Rlp3Frame::Data { seq, rexmit, data } => {
                assert_eq!(seq, 1);
                assert!(!rexmit);
                assert_eq!(data, vec![0xAB, 0xCD]);
            }
            other => panic!("expected Data, got {:?}", other),
        }
    }

    #[test]
    fn test_sub_rate_segmented_half_rate() {
        let frame = Rlp3Frame::Segmented {
            seq: 0x34,
            sqi: true,
            last_seg: true,
            rexmit: true,
            seq_hi: Some(0x02),
            s_seq: 0x003,
            data: vec![0xAB, 0xCD],
        };
        let full = frame.encode(MUX).unwrap();
        let mut sub_bits = vec![0u8; 80];
        sub_bits.copy_from_slice(&full[..80]);

        let decoded = decode_sub_rate_frame(&sub_bits, 80).unwrap();
        assert_eq!(decoded, frame);
    }

    #[test]
    fn test_sub_rate_decode_live_mobile_sync_structure() {
        // Verify the structure observed from a real mobile at quarter rate:
        // SEQ=0, CTL=110110 (SYNC), INIT_VAR=1, NAK_PARAM_INCL=0
        // The mobile sends this as raw 40-bit frame without TYPE field.
        // Build the expected frame and verify it decodes correctly.
        let frame = Rlp3Frame::Control {
            seq: 0,
            control_type: Rlp3ControlType::Sync,
            init_var: true,
            nak_param_incl: false,
        };
        // Encode at full rate to get the info field, then extract first 40 bits.
        let full = frame.encode(MUX).unwrap();
        let mut quarter = vec![0u8; 40];
        quarter[..32].copy_from_slice(&full[..32]); // 32 bits of content + 8 padding

        // Verify bits match the pattern seen on air:
        // SEQ=00000000, CTL=110110, INIT_VAR=1, NAK=0
        assert_eq!(get_bits(&quarter, 0, 8), 0); // SEQ=0
        assert_eq!(get_bits(&quarter, 8, 6), 0b110110); // CTL=SYNC
        assert_eq!(quarter[14], 1); // INIT_VAR
        assert_eq!(quarter[15], 0); // NAK_PARAM_INCL

        let decoded = decode_sub_rate_frame(&quarter, 40).unwrap();
        assert!(matches!(
            decoded,
            Rlp3Frame::Control {
                seq: 0,
                control_type: Rlp3ControlType::Sync,
                init_var: true,
                nak_param_incl: false,
            }
        ));
    }
}
