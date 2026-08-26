//! RLP Type 2 frame codec per TIA/EIA/IS-707-A.8 — Rate Set 2 (Mux Option 2)
//! traffic for the async data service (SO 12).
//!
//! Wire specifics that matter for the codec:
//!
//!   - Control CTL patterns (6 bits): SYNC `110110`, ACK `111010`,
//!     SYNC/ACK `111110`, NAK `110000`.
//!   - Non-NAK control frames carry no FIRST/LAST; the NAK frame carries a
//!     4-bit L_SEQ_HI plus 12-bit FIRST/LAST for the full 12-bit position.
//!   - Unsegmented data frames carry a REXMIT bit and a 6-bit LEN.
//!   - Sequence counters are 12-bit (mod 4096); only the low 8 bits ride in the
//!     on-wire SEQ field. The 12-bit tracking lives in the session layer.
//!
//! Encoded: control (SYNC/SYNC-ACK/ACK/NAK), idle, unsegmented data (Format A
//! wrap at full rate, raw at sub-rate), and Format B. Bitmap NAK
//! (NAK_TYPE=`01`) and segmented retransmission frames are not yet encoded; the
//! decoder rejects them rather than misinterpreting them.

use crate::rlp::{RlpMuxOption, RlpRate, crc16_rlp, get_bits, nordstrom_robinson_fcs, put_bits};

/// Control-frame CTL field values (6 bits), non-encrypted mode (IS-707-A.8 4.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rlp2ControlType {
    /// `110110` - Synchronization request.
    Sync,
    /// `111010` - Acknowledgment.
    Ack,
    /// `111110` - Both SYNC and ACK.
    SyncAck,
    /// `110000` - Negative acknowledgment (retransmission request).
    Nak,
}

/// CTL master-discriminator: the MSB of the post-SEQ field is `1` for both
/// control and segmented frames; the next bit separates them.
const CTL_SYNC: u8 = 0b110110;
const CTL_ACK: u8 = 0b111010;
const CTL_SYNC_ACK: u8 = 0b111110;
const CTL_NAK: u8 = 0b110000;

const SEQ_BITS: usize = 8;
const CTL_BITS: usize = 6;
const ENC_MODE_BITS: usize = 2;
const FCS_BITS: usize = 16;
/// NAK-only fields.
const NAK_TYPE_BITS: usize = 2;
const L_SEQ_HI_BITS: usize = 4;
const NAK_SEQ_BITS: usize = 12;
/// NAK_TYPE value for a contiguous FIRST..LAST range.
const NAK_TYPE_RANGE: u8 = 0b00;

impl Rlp2ControlType {
    pub fn ctl_bits(self) -> u8 {
        match self {
            Rlp2ControlType::Sync => CTL_SYNC,
            Rlp2ControlType::Ack => CTL_ACK,
            Rlp2ControlType::SyncAck => CTL_SYNC_ACK,
            Rlp2ControlType::Nak => CTL_NAK,
        }
    }

    fn from_ctl_bits(ctl: u8) -> Option<Rlp2ControlType> {
        match ctl & 0x3F {
            CTL_SYNC => Some(Rlp2ControlType::Sync),
            CTL_ACK => Some(Rlp2ControlType::Ack),
            CTL_SYNC_ACK => Some(Rlp2ControlType::SyncAck),
            CTL_NAK => Some(Rlp2ControlType::Nak),
            _ => None,
        }
    }
}

/// A decoded RLP Type 2 frame.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rlp2Frame {
    /// SYNC / SYNC-ACK / ACK control frame (non-encrypted).
    Control {
        seq: u8,
        control_type: Rlp2ControlType,
        /// ENCRYPTION_MODE field (2 bits). `00` = not supported (default).
        encryption_mode: u8,
    },
    /// NAK control frame, contiguous-range form (NAK_TYPE = `00`).
    Nak {
        seq: u8,
        /// Most-significant 4 bits of the sender's 12-bit L_V(S).
        l_seq_hi: u8,
        /// First missing 12-bit sequence number.
        first: u16,
        /// Last missing 12-bit sequence number (inclusive).
        last: u16,
    },
    /// Unsegmented data frame (Format A). LEN = 0 is an idle frame.
    Data {
        seq: u8,
        rexmit: bool,
        data: Vec<u8>,
    },
    /// Format B data frame (full rate only, max throughput).
    DataFormatB {
        seq: u8,
        rexmit: bool,
        data: Vec<u8>,
    },
    /// Rate 1/8 idle frame: SEQ + Nordstrom-Robinson FCS.
    Idle { seq: u8 },
}

impl Rlp2Frame {
    pub fn seq(&self) -> u8 {
        match self {
            Rlp2Frame::Control { seq, .. }
            | Rlp2Frame::Nak { seq, .. }
            | Rlp2Frame::Data { seq, .. }
            | Rlp2Frame::DataFormatB { seq, .. }
            | Rlp2Frame::Idle { seq } => *seq,
        }
    }

    pub fn is_idle(&self) -> bool {
        match self {
            Rlp2Frame::Idle { .. } => true,
            Rlp2Frame::Data { data, .. } => data.is_empty(),
            _ => false,
        }
    }

    pub fn is_control(&self) -> bool {
        matches!(self, Rlp2Frame::Control { .. } | Rlp2Frame::Nak { .. })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rlp2EncodeError {
    /// Unsegmented Format A payload exceeds the maximum for the requested rate.
    DataTooLong { len: usize, max: usize },
    /// Format B requires exactly `format_b_octets()` octets.
    FormatBLength { len: usize, expected: usize },
    /// Format B is only valid at full rate.
    FormatBRequiresFullRate { rate: RlpRate },
    /// Data/control carriers are not defined at Rate 1/8.
    EighthRateDataCarrier,
}

impl std::fmt::Display for Rlp2EncodeError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Rlp2EncodeError::DataTooLong { len, max } => {
                write!(f, "RLP2 unsegmented data {len} octets exceeds max {max}")
            }
            Rlp2EncodeError::FormatBLength { len, expected } => {
                write!(f, "RLP2 Format B requires {expected} octets, got {len}")
            }
            Rlp2EncodeError::FormatBRequiresFullRate { rate } => {
                write!(f, "RLP2 Format B is only valid at full rate, got {rate:?}")
            }
            Rlp2EncodeError::EighthRateDataCarrier => {
                write!(f, "RLP2 data/control carrier undefined at Rate 1/8")
            }
        }
    }
}

impl std::error::Error for Rlp2EncodeError {}

/// Full-rate Format-A / Format-B trailing TYPE codes.
fn full_rate_type_codes(mux_option: RlpMuxOption) -> (usize, u8, u8, u8) {
    // (type_bits, format_a, format_b_new, format_b_rexmit)
    match mux_option {
        RlpMuxOption::One => (3, 0b001, 0b010, 0b011),
        RlpMuxOption::Two => (2, 0b01, 0b10, 0b11),
    }
}

/// Maximum unsegmented data length (LEN, in octets) per rate for RLP Type 2.
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

// Encoding

/// Encode an RLP Type 2 frame for the selected multiplex option and rate.
pub fn encode_frame_for_mux(
    frame: &Rlp2Frame,
    rate: RlpRate,
    mux_option: RlpMuxOption,
) -> Result<Vec<u8>, Rlp2EncodeError> {
    let total_bits = mux_option.primary_bits(rate);
    let mut bits = vec![0u8; total_bits];

    match (frame, rate) {
        (Rlp2Frame::Idle { seq }, RlpRate::Eighth) => {
            put_bits(&mut bits, 0, *seq, SEQ_BITS);
            put_bits(&mut bits, SEQ_BITS, nordstrom_robinson_fcs(*seq), 8);
        }

        (Rlp2Frame::Idle { seq }, _) => {
            // Higher-rate idle: unsegmented data with LEN=0.
            encode_unsegmented_data(&mut bits, rate, mux_option, *seq, false, &[])?;
        }

        (
            Rlp2Frame::Control {
                seq,
                control_type,
                encryption_mode,
            },
            rate,
        ) => {
            if rate == RlpRate::Eighth {
                return Err(Rlp2EncodeError::EighthRateDataCarrier);
            }
            let info = encode_control_info(*seq, control_type.ctl_bits(), *encryption_mode);
            place_information(&mut bits, &info, rate, mux_option);
        }

        (
            Rlp2Frame::Nak {
                seq,
                l_seq_hi,
                first,
                last,
            },
            rate,
        ) => {
            if rate == RlpRate::Eighth {
                return Err(Rlp2EncodeError::EighthRateDataCarrier);
            }
            let info = encode_nak_info(*seq, *l_seq_hi, *first, *last);
            place_information(&mut bits, &info, rate, mux_option);
        }

        (Rlp2Frame::Data { seq, rexmit, data }, rate) => {
            encode_unsegmented_data(&mut bits, rate, mux_option, *seq, *rexmit, data)?;
        }

        (Rlp2Frame::DataFormatB { seq, rexmit, data }, RlpRate::Full) => {
            let expected = mux_option.format_b_octets();
            if data.len() != expected {
                return Err(Rlp2EncodeError::FormatBLength {
                    len: data.len(),
                    expected,
                });
            }
            put_bits(&mut bits, 0, *seq, SEQ_BITS);
            for (i, byte) in data.iter().enumerate() {
                put_bits(&mut bits, SEQ_BITS + i * 8, *byte, 8);
            }
            let (type_bits, _fa, fb_new, fb_rexmit) = full_rate_type_codes(mux_option);
            let info_bits = mux_option.full_information_bits();
            let code = if *rexmit { fb_rexmit } else { fb_new };
            put_bits(&mut bits, info_bits, code, type_bits);
        }

        (Rlp2Frame::DataFormatB { .. }, rate) => {
            return Err(Rlp2EncodeError::FormatBRequiresFullRate { rate });
        }
    }

    Ok(bits)
}

/// Build the information field for a SYNC/SYNC-ACK/ACK control frame.
/// Layout: SEQ(8) | CTL(6) | ENCRYPTION_MODE(2) | FCS(16), FCS over bits 0..16.
fn encode_control_info(seq: u8, ctl: u8, encryption_mode: u8) -> Vec<u8> {
    let mut info = vec![0u8; SEQ_BITS + CTL_BITS + ENC_MODE_BITS + FCS_BITS];
    put_bits(&mut info, 0, seq, SEQ_BITS);
    put_bits(&mut info, SEQ_BITS, ctl, CTL_BITS);
    put_bits(
        &mut info,
        SEQ_BITS + CTL_BITS,
        encryption_mode,
        ENC_MODE_BITS,
    );
    let fcs_offset = SEQ_BITS + CTL_BITS + ENC_MODE_BITS;
    let fcs = crc16_rlp(&info[0..fcs_offset]);
    put_bits(&mut info, fcs_offset, (fcs >> 8) as u8, 8);
    put_bits(&mut info, fcs_offset + 8, (fcs & 0xFF) as u8, 8);
    info
}

/// Build the information field for a range NAK control frame.
/// Layout: SEQ(8) | CTL(6) | NAK_TYPE(2) | L_SEQ_HI(4) | FIRST(12) | LAST(12) | FCS(16).
fn encode_nak_info(seq: u8, l_seq_hi: u8, first: u16, last: u16) -> Vec<u8> {
    let body_bits = SEQ_BITS + CTL_BITS + NAK_TYPE_BITS + L_SEQ_HI_BITS + NAK_SEQ_BITS * 2;
    let mut info = vec![0u8; body_bits + FCS_BITS];
    let mut off = 0;
    put_bits(&mut info, off, seq, SEQ_BITS);
    off += SEQ_BITS;
    put_bits(&mut info, off, CTL_NAK, CTL_BITS);
    off += CTL_BITS;
    put_bits(&mut info, off, NAK_TYPE_RANGE, NAK_TYPE_BITS);
    off += NAK_TYPE_BITS;
    put_bits(&mut info, off, l_seq_hi & 0x0F, L_SEQ_HI_BITS);
    off += L_SEQ_HI_BITS;
    put_bits12(&mut info, off, first);
    off += NAK_SEQ_BITS;
    put_bits12(&mut info, off, last);
    off += NAK_SEQ_BITS;
    debug_assert_eq!(off, body_bits);
    let fcs = crc16_rlp(&info[0..body_bits]);
    put_bits(&mut info, body_bits, (fcs >> 8) as u8, 8);
    put_bits(&mut info, body_bits + 8, (fcs & 0xFF) as u8, 8);
    info
}

/// Encode an unsegmented Format A data frame in place.
/// Layout: SEQ(8) | CTL(1='0') | REXMIT(1) | LEN(6) | Data(8*LEN) | pad,
/// with a trailing TYPE code at full rate.
fn encode_unsegmented_data(
    bits: &mut [u8],
    rate: RlpRate,
    mux_option: RlpMuxOption,
    seq: u8,
    rexmit: bool,
    data: &[u8],
) -> Result<(), Rlp2EncodeError> {
    if rate == RlpRate::Eighth {
        return Err(Rlp2EncodeError::EighthRateDataCarrier);
    }
    let max = max_len_for_rate(rate, mux_option);
    if data.len() > max {
        return Err(Rlp2EncodeError::DataTooLong {
            len: data.len(),
            max,
        });
    }
    let mut off = 0;
    put_bits(bits, off, seq, SEQ_BITS);
    off += SEQ_BITS;
    bits[off] = 0; // CTL bit = '0' (unsegmented data)
    off += 1;
    bits[off] = u8::from(rexmit);
    off += 1;
    put_bits(bits, off, data.len() as u8, 6);
    off += 6;
    for &byte in data {
        put_bits(bits, off, byte, 8);
        off += 8;
    }
    // Remainder is already zero-padded; append TYPE at full rate.
    if rate == RlpRate::Full {
        let (type_bits, fa, _fbn, _fbr) = full_rate_type_codes(mux_option);
        let info_bits = mux_option.full_information_bits();
        put_bits(bits, info_bits, fa, type_bits);
    }
    Ok(())
}

/// Copy a control/NAK information field into a frame at the given rate.
/// At full rate the information body occupies `full_information_bits()` and a
/// Format-A TYPE code is appended; at sub-rate it fills the frame directly.
fn place_information(bits: &mut [u8], info: &[u8], rate: RlpRate, mux_option: RlpMuxOption) {
    match rate {
        RlpRate::Full => {
            let info_bits = mux_option.full_information_bits();
            let n = info.len().min(info_bits);
            bits[..n].copy_from_slice(&info[..n]);
            let (type_bits, fa, _fbn, _fbr) = full_rate_type_codes(mux_option);
            put_bits(bits, info_bits, fa, type_bits);
        }
        RlpRate::Half | RlpRate::Quarter => {
            let n = info.len().min(bits.len());
            bits[..n].copy_from_slice(&info[..n]);
        }
        RlpRate::Eighth => {}
    }
}

fn put_bits12(bits: &mut [u8], offset: usize, val: u16) {
    for i in 0..NAK_SEQ_BITS {
        bits[offset + i] = ((val >> (NAK_SEQ_BITS - 1 - i)) & 1) as u8;
    }
}

// Decoding

/// Decode an RLP Type 2 frame for the selected multiplex option.
pub fn decode_frame_for_mux(
    bits: &[u8],
    rate: RlpRate,
    mux_option: RlpMuxOption,
) -> Option<Rlp2Frame> {
    if bits.len() < mux_option.primary_bits(rate) {
        return None;
    }
    match rate {
        RlpRate::Eighth => decode_idle(bits),
        RlpRate::Half | RlpRate::Quarter => decode_information(bits, rate, mux_option),
        RlpRate::Full => decode_full_rate(bits, mux_option),
    }
}

fn decode_idle(bits: &[u8]) -> Option<Rlp2Frame> {
    let seq = get_bits(bits, 0, SEQ_BITS) as u8;
    let fcs = get_bits(bits, SEQ_BITS, 8) as u8;
    if fcs != nordstrom_robinson_fcs(seq) {
        return None;
    }
    Some(Rlp2Frame::Idle { seq })
}

fn decode_full_rate(bits: &[u8], mux_option: RlpMuxOption) -> Option<Rlp2Frame> {
    let info_bits = mux_option.full_information_bits();
    let (type_bits, fa, fb_new, fb_rexmit) = full_rate_type_codes(mux_option);
    let frame_type = get_bits(bits, info_bits, type_bits) as u8;
    if frame_type == fa {
        decode_information(bits, RlpRate::Full, mux_option)
    } else if frame_type == fb_new || frame_type == fb_rexmit {
        let seq = get_bits(bits, 0, SEQ_BITS) as u8;
        let mut data = vec![0u8; mux_option.format_b_octets()];
        for (i, byte) in data.iter_mut().enumerate() {
            *byte = get_bits(bits, SEQ_BITS + i * 8, 8) as u8;
        }
        Some(Rlp2Frame::DataFormatB {
            seq,
            rexmit: frame_type == fb_rexmit,
            data,
        })
    } else {
        None
    }
}

/// Decode the information field common to sub-rate frames and full-rate Format A.
fn decode_information(bits: &[u8], rate: RlpRate, mux_option: RlpMuxOption) -> Option<Rlp2Frame> {
    let seq = get_bits(bits, 0, SEQ_BITS) as u8;
    let disc = bits[SEQ_BITS];
    if disc == 0 {
        // Unsegmented data: CTL(1='0') | REXMIT(1) | LEN(6) | Data
        let rexmit = bits[SEQ_BITS + 1] == 1;
        let len = get_bits(bits, SEQ_BITS + 2, 6) as usize;
        if len > max_len_for_rate(rate, mux_option) {
            return None;
        }
        let data_off = SEQ_BITS + 8;
        let mut data = vec![0u8; len];
        for (i, byte) in data.iter_mut().enumerate() {
            *byte = get_bits(bits, data_off + i * 8, 8) as u8;
        }
        return Some(Rlp2Frame::Data { seq, rexmit, data });
    }
    // disc == 1: control (CTL '11xxxx') or segmented (CTL '10xx').
    if bits[SEQ_BITS + 1] == 0 {
        // Segmented retransmission frames are not yet handled.
        return None;
    }
    let ctl = get_bits(bits, SEQ_BITS, CTL_BITS) as u8;
    let control_type = Rlp2ControlType::from_ctl_bits(ctl)?;
    match control_type {
        Rlp2ControlType::Nak => decode_nak(bits, seq),
        _ => {
            let enc_off = SEQ_BITS + CTL_BITS;
            let encryption_mode = get_bits(bits, enc_off, ENC_MODE_BITS) as u8;
            let fcs_off = enc_off + ENC_MODE_BITS;
            let received = get_bits(bits, fcs_off, FCS_BITS) as u16;
            if received != crc16_rlp(&bits[0..fcs_off]) {
                return None;
            }
            Some(Rlp2Frame::Control {
                seq,
                control_type,
                encryption_mode,
            })
        }
    }
}

fn decode_nak(bits: &[u8], seq: u8) -> Option<Rlp2Frame> {
    let mut off = SEQ_BITS + CTL_BITS;
    let nak_type = get_bits(bits, off, NAK_TYPE_BITS) as u8;
    off += NAK_TYPE_BITS;
    if nak_type != NAK_TYPE_RANGE {
        // Bitmap NAK (NAK_TYPE=01) not yet handled.
        return None;
    }
    let l_seq_hi = get_bits(bits, off, L_SEQ_HI_BITS) as u8;
    off += L_SEQ_HI_BITS;
    let first = get_bits(bits, off, NAK_SEQ_BITS) as u16;
    off += NAK_SEQ_BITS;
    let last = get_bits(bits, off, NAK_SEQ_BITS) as u16;
    off += NAK_SEQ_BITS;
    let received = get_bits(bits, off, FCS_BITS) as u16;
    if received != crc16_rlp(&bits[0..off]) {
        return None;
    }
    Some(Rlp2Frame::Nak {
        seq,
        l_seq_hi,
        first,
        last,
    })
}

// Constructors

pub fn sync_frame(seq: u8) -> Rlp2Frame {
    Rlp2Frame::Control {
        seq,
        control_type: Rlp2ControlType::Sync,
        encryption_mode: 0,
    }
}

pub fn sync_ack_frame(seq: u8) -> Rlp2Frame {
    Rlp2Frame::Control {
        seq,
        control_type: Rlp2ControlType::SyncAck,
        encryption_mode: 0,
    }
}

pub fn ack_frame(seq: u8) -> Rlp2Frame {
    Rlp2Frame::Control {
        seq,
        control_type: Rlp2ControlType::Ack,
        encryption_mode: 0,
    }
}

pub fn idle_frame(seq: u8) -> Rlp2Frame {
    Rlp2Frame::Idle { seq }
}

pub fn data_frame(seq: u8, data: &[u8]) -> Rlp2Frame {
    Rlp2Frame::Data {
        seq,
        rexmit: false,
        data: data.to_vec(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(frame: &Rlp2Frame, rate: RlpRate) -> Rlp2Frame {
        let bits = encode_frame_for_mux(frame, rate, RlpMuxOption::Two).expect("encode");
        assert_eq!(bits.len(), RlpMuxOption::Two.primary_bits(rate));
        decode_frame_for_mux(&bits, rate, RlpMuxOption::Two).expect("decode")
    }

    #[test]
    fn control_ctl_patterns_match_type2_spec() {
        assert_eq!(Rlp2ControlType::Sync.ctl_bits(), 0b110110);
        assert_eq!(Rlp2ControlType::Ack.ctl_bits(), 0b111010);
        assert_eq!(Rlp2ControlType::SyncAck.ctl_bits(), 0b111110);
        assert_eq!(Rlp2ControlType::Nak.ctl_bits(), 0b110000);
    }

    #[test]
    fn sync_full_rate_carries_type2_ctl_on_the_wire() {
        let bits = encode_frame_for_mux(&sync_frame(0), RlpRate::Full, RlpMuxOption::Two).unwrap();
        // SEQ occupies bits 0..8, CTL bits 8..14.
        assert_eq!(get_bits(&bits, 8, 6) as u8, 0b110110);
        // Full-rate Format-A TYPE trailer = '01' for Mux Option 2.
        let info_bits = RlpMuxOption::Two.full_information_bits();
        assert_eq!(get_bits(&bits, info_bits, 2) as u8, 0b01);
    }

    #[test]
    fn control_roundtrips_all_rates() {
        for &ct in &[
            Rlp2ControlType::Sync,
            Rlp2ControlType::Ack,
            Rlp2ControlType::SyncAck,
        ] {
            let frame = Rlp2Frame::Control {
                seq: 0x5A,
                control_type: ct,
                encryption_mode: 0,
            };
            for &rate in &[RlpRate::Full, RlpRate::Half, RlpRate::Quarter] {
                assert_eq!(roundtrip(&frame, rate), frame, "ct={ct:?} rate={rate:?}");
            }
        }
    }

    #[test]
    fn nak_range_roundtrips_with_12bit_fields() {
        let frame = Rlp2Frame::Nak {
            seq: 0x34,
            l_seq_hi: 0x0A,
            first: 0x0FA,
            last: 0xABC,
        };
        assert_eq!(roundtrip(&frame, RlpRate::Full), frame);
        assert_eq!(roundtrip(&frame, RlpRate::Half), frame);
    }

    #[test]
    fn unsegmented_data_roundtrips_and_carries_rexmit() {
        let payload: Vec<u8> = (0..13).collect();
        let frame = Rlp2Frame::Data {
            seq: 0x11,
            rexmit: true,
            data: payload.clone(),
        };
        // 13 octets fits Half rate for RS2.
        assert_eq!(roundtrip(&frame, RlpRate::Half), frame);
        // And full rate (31-octet max).
        let big = Rlp2Frame::Data {
            seq: 0x22,
            rexmit: false,
            data: (0..31).collect(),
        };
        assert_eq!(roundtrip(&big, RlpRate::Full), big);
    }

    #[test]
    fn format_b_roundtrips_full_rate_only() {
        let frame = Rlp2Frame::DataFormatB {
            seq: 0x77,
            rexmit: false,
            data: (0..32).collect(),
        };
        assert_eq!(roundtrip(&frame, RlpRate::Full), frame);
        assert!(matches!(
            encode_frame_for_mux(&frame, RlpRate::Half, RlpMuxOption::Two),
            Err(Rlp2EncodeError::FormatBRequiresFullRate { .. })
        ));
    }

    #[test]
    fn idle_roundtrips_eighth_rate() {
        let frame = idle_frame(0x9C);
        assert_eq!(roundtrip(&frame, RlpRate::Eighth), frame);
    }

    #[test]
    fn type1_sync_is_not_decoded_as_type2_control() {
        // A Type 1 SYNC (CTL 110100) must not decode as a valid Type 2 control.
        let t1 = crate::rlp::encode_frame_for_mux(
            &crate::rlp::sync_frame(0),
            RlpRate::Full,
            RlpMuxOption::Two,
        )
        .unwrap();
        // Either it fails to decode, or it does not come back as a SYNC.
        match decode_frame_for_mux(&t1, RlpRate::Full, RlpMuxOption::Two) {
            None => {}
            Some(Rlp2Frame::Control { control_type, .. }) => {
                assert_ne!(control_type, Rlp2ControlType::Sync)
            }
            Some(_) => {}
        }
    }
}
