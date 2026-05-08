//! C.S0015-B SMS Point-to-Point encoder/decoder.
//!
//! Encoder: MT SMS Deliver (forward link).
//! Decoder: MO SMS Submit (reverse link) — parses Data Burst CHARi payload.

use crate::bits::Bitstream;

/// Encode an SMS Deliver message as C.S0015-B Transport Layer bytes.
///
/// MVP constraints:
///   - Originating addresses are encoded as 4-bit DTMF when possible, otherwise
///     as DIGIT_MODE=1 / NUMBER_MODE=0 / 8-bit ASCII per C.S0015-B 3.4.3.3.
///   - No Bearer Reply Option -- delivery is unconfirmed (see MVP Scope).
///   - 7-bit ASCII user data encoding (MSG_ENCODING=0x02).
pub fn encode_sms_deliver(originating_number: &str, text: &str, message_id: u16) -> Vec<u8> {
    let mut buf = Vec::new();

    // Transport Layer MSG_TYPE = 0x00 (point-to-point)
    buf.push(0x00);

    // Teleservice Identifier parameter (tag=0x00)
    buf.push(0x00); // tag
    buf.push(0x02); // len
    buf.push(0x10); // TELESERVICE_ID high byte (0x1002 = WMT)
    buf.push(0x02); // TELESERVICE_ID low byte

    // Originating Address parameter (tag=0x02)
    encode_originating_address(&mut buf, originating_number);

    // Bearer Data parameter (tag=0x08)
    encode_bearer_data(&mut buf, text, message_id);

    buf
}

fn char_to_dtmf(ch: char) -> Option<u8> {
    match ch {
        '1' => Some(1),
        '2' => Some(2),
        '3' => Some(3),
        '4' => Some(4),
        '5' => Some(5),
        '6' => Some(6),
        '7' => Some(7),
        '8' => Some(8),
        '9' => Some(9),
        '0' => Some(10),
        '*' => Some(11),
        '#' => Some(12),
        _ => None,
    }
}

fn encode_originating_address(buf: &mut Vec<u8>, number: &str) {
    let mut bs = Bitstream::new();
    if number.chars().all(|ch| char_to_dtmf(ch).is_some()) {
        // DIGIT_MODE=0, NUMBER_MODE=0, NUM_FIELDS, then 4-bit DTMF digits.
        let digits: Vec<u8> = number.chars().filter_map(char_to_dtmf).collect();
        bs.write_u8(0, 1); // DIGIT_MODE = 0 (4-bit DTMF)
        bs.write_u8(0, 1); // NUMBER_MODE = 0
        bs.write_u8(digits.len() as u8, 8); // NUM_FIELDS

        for dtmf in &digits {
            bs.write_u8(*dtmf, 4);
        }
    } else {
        // DIGIT_MODE=1, NUMBER_MODE=0, NUMBER_TYPE=unknown,
        // NUMBER_PLAN=unknown, NUM_FIELDS, then 8-bit ASCII CHARi.
        let chars: Vec<u8> = number
            .chars()
            .map(|ch| if ch.is_ascii() { ch as u8 } else { b'?' })
            .collect();
        bs.write_u8(1, 1); // DIGIT_MODE = 1 (8-bit ASCII)
        bs.write_u8(0, 1); // NUMBER_MODE = 0
        bs.write_u8(0, 3); // NUMBER_TYPE = unknown
        bs.write_u8(0, 4); // NUMBER_PLAN = unknown
        bs.write_u8(chars.len() as u8, 8); // NUM_FIELDS

        for ch in chars {
            bs.write_u8(ch & 0x7F, 8);
        }
    }

    // Pad to byte boundary
    let remainder = bs.len() % 8;
    if remainder != 0 {
        bs.write_u8(0, 8 - remainder);
    }

    let bytes = bitstream_to_packed_bytes(&bs);
    buf.push(0x02); // tag = Originating Address
    buf.push(bytes.len() as u8); // len
    buf.extend_from_slice(&bytes);
}

fn encode_bearer_data(buf: &mut Vec<u8>, text: &str, message_id: u16) {
    let mut bearer = Vec::new();

    // Message Identifier sub-parameter (tag=0x00)
    bearer.push(0x00); // sub-tag
    bearer.push(0x03); // len = 3 bytes
    // MESSAGE_TYPE(4) = 0x1 (deliver), MESSAGE_ID(16), HEADER_IND(1)=0, RESERVED(3)=0
    let mut bs = Bitstream::new();
    bs.write_u8(0x01, 4); // MESSAGE_TYPE = deliver
    bs.write_u32(message_id as u32, 16);
    bs.write_u8(0, 1); // HEADER_IND = 0
    bs.write_u8(0, 3); // reserved
    let id_bytes = bitstream_to_packed_bytes(&bs);
    bearer.extend_from_slice(&id_bytes);

    // User Data sub-parameter (tag=0x01)
    // MSG_ENCODING=0x02 (7-bit ASCII), NUM_FIELDS, then 7-bit chars
    let mut ud = Bitstream::new();
    ud.write_u8(0x02, 5); // MSG_ENCODING = 7-bit ASCII
    ud.write_u8(text.len() as u8, 8);
    for &b in text.as_bytes() {
        ud.write_u8(b & 0x7F, 7);
    }
    let remainder = ud.len() % 8;
    if remainder != 0 {
        ud.write_u8(0, 8 - remainder);
    }
    let ud_bytes = bitstream_to_packed_bytes(&ud);
    bearer.push(0x01); // sub-tag = User Data
    bearer.push(ud_bytes.len() as u8);
    bearer.extend_from_slice(&ud_bytes);

    // No Bearer Reply Option (MVP: unconfirmed delivery)

    buf.push(0x08); // tag = Bearer Data
    buf.push(bearer.len() as u8);
    buf.extend_from_slice(&bearer);
}

/// Encode an SMS Cause Code message (SMS_MSG_TYPE=0x02) for the f-dsch.
///
/// Per C.S0015-B §4.5.21, after receiving an MO SMS, the BS sends a Cause Code
/// with the matching REPLY_SEQ to acknowledge or reject the message.
/// ERROR_CLASS=0 means no error (delivery accepted).
///
/// Returns Transport Layer bytes suitable for wrapping in a ForwardDataBurstMessage.
pub fn encode_sms_cause_code(reply_seq: u8, error_class: u8) -> Vec<u8> {
    encode_sms_cause_code_with_cause(reply_seq, error_class, None)
}

/// Encode an SMS Cause Code with an optional SMS_CauseCode value.
///
/// C.S0015-B requires CAUSE_CODE to be omitted when ERROR_CLASS is `00` and
/// present when ERROR_CLASS indicates a temporary or permanent condition.
pub fn encode_sms_cause_code_with_cause(
    reply_seq: u8,
    error_class: u8,
    cause_code: Option<u8>,
) -> Vec<u8> {
    let mut buf = Vec::new();

    // Transport Layer SMS_MSG_TYPE = 0x02 (Cause Code)
    buf.push(0x02);

    // Cause Codes parameter (tag=0x07)
    buf.push(0x07); // PARAMETER_ID
    let include_cause = (error_class & 0x03) != 0 && cause_code.is_some();
    buf.push(if include_cause { 0x02 } else { 0x01 }); // PARAMETER_LEN
    // REPLY_SEQ(6) | ERROR_CLASS(2)
    buf.push((reply_seq & 0x3F) << 2 | (error_class & 0x03));
    if include_cause {
        buf.push(cause_code.unwrap());
    }

    buf
}

/// Convert a Bitstream (individual bits) to packed byte array.
fn bitstream_to_packed_bytes(bs: &Bitstream) -> Vec<u8> {
    bs.bits()
        .chunks(8)
        .map(|chunk| {
            let mut byte = 0u8;
            for (i, &bit) in chunk.iter().enumerate() {
                byte |= (bit & 1) << (7 - i);
            }
            byte
        })
        .collect()
}

// ---------------------------------------------------------------------------
// MO SMS Decoder (reverse link)
// ---------------------------------------------------------------------------

/// Decoded MO SMS Submit message from a reverse Data Burst.
#[derive(Debug, Clone)]
pub struct DecodedMoSms {
    /// Teleservice ID (0x1002 = WMT, 0x1005 = WAP, etc.)
    pub teleservice_id: u16,
    /// Destination address (phone number digits).
    pub destination_number: String,
    /// Bearer data message type (0x02 = Submit, 0x04 = Deliver Report/Ack).
    pub message_type: u8,
    /// Message identifier from the bearer data.
    pub message_id: u16,
    /// Decoded user data text (CDMA ASCII, GSM 7-bit, Unicode, or 8-bit best effort).
    pub text: String,
    /// Raw bearer data bytes for passthrough if needed.
    pub raw_bearer_data: Vec<u8>,
    /// Bearer Reply Option REPLY_SEQ, if present (tag 0x06).
    /// The BS echoes this in the SMS Cause Code response.
    pub reply_seq: Option<u8>,
}

/// Decoded SMS from a forward-link Data Burst (MT SMS Deliver or Cause Code).
#[derive(Debug, Clone)]
pub struct DecodedMtSms {
    /// Transport Layer MSG_TYPE (0x00 = point-to-point, 0x02 = cause code).
    pub tl_msg_type: u8,
    /// Teleservice ID (0x1002 = WMT). Zero for cause codes.
    pub teleservice_id: u16,
    /// Originating address (phone number). Empty for cause codes.
    pub originating_number: String,
    /// Bearer data message type (0x01 = Deliver). Zero for cause codes.
    pub message_type: u8,
    /// Message identifier from the bearer data. Zero for cause codes.
    pub message_id: u16,
    /// Decoded user data text. Empty for cause codes.
    pub text: String,
    /// For cause codes: REPLY_SEQ.
    pub reply_seq: Option<u8>,
    /// For cause codes: ERROR_CLASS.
    pub error_class: Option<u8>,
}

/// Decode a forward-link SMS from Data Burst CHARi payload bytes.
///
/// Handles both SMS Deliver (MSG_TYPE=0x00) and SMS Cause Code (MSG_TYPE=0x02).
pub fn decode_mt_sms(data: &[u8]) -> Option<DecodedMtSms> {
    if data.is_empty() {
        return None;
    }

    let tl_msg_type = data[0];

    if tl_msg_type == 0x02 {
        // SMS Cause Code: PARAMETER_ID(8) + PARAMETER_LEN(8) + REPLY_SEQ(6) + ERROR_CLASS(2)
        if data.len() >= 4 && data[1] == 0x07 {
            let reply_seq = (data[3] >> 2) & 0x3F;
            let error_class = data[3] & 0x03;
            return Some(DecodedMtSms {
                tl_msg_type,
                teleservice_id: 0,
                originating_number: String::new(),
                message_type: 0,
                message_id: 0,
                text: String::new(),
                reply_seq: Some(reply_seq),
                error_class: Some(error_class),
            });
        }
        return None;
    }

    if tl_msg_type != 0x00 {
        return None;
    }

    let mut pos = 1;
    let mut teleservice_id: u16 = 0;
    let mut originating_number = String::new();
    let mut raw_bearer_data = Vec::new();

    while pos + 2 <= data.len() {
        let tag = data[pos];
        let len = data[pos + 1] as usize;
        pos += 2;
        if pos + len > data.len() {
            break;
        }
        let value = &data[pos..pos + len];
        pos += len;

        match tag {
            0x00 => {
                if value.len() >= 2 {
                    teleservice_id = ((value[0] as u16) << 8) | (value[1] as u16);
                }
            }
            0x02 => {
                // Originating Address
                originating_number = decode_address(value);
            }
            0x08 => {
                raw_bearer_data = value.to_vec();
            }
            _ => {}
        }
    }

    let mut message_type_bd: u8 = 0;
    let mut message_id: u16 = 0;
    let mut text = String::new();

    let mut bpos = 0;
    while bpos + 2 <= raw_bearer_data.len() {
        let sub_tag = raw_bearer_data[bpos];
        let sub_len = raw_bearer_data[bpos + 1] as usize;
        bpos += 2;
        if bpos + sub_len > raw_bearer_data.len() {
            break;
        }
        let sub_value = &raw_bearer_data[bpos..bpos + sub_len];
        bpos += sub_len;

        match sub_tag {
            0x00 => {
                if sub_value.len() >= 3 {
                    let bs = Bitstream::new_bytes(sub_value);
                    let bits = bs.bits();
                    if bits.len() >= 21 {
                        message_type_bd = bits_to_u8(&bits[0..4]);
                        message_id = bits_to_u16(&bits[4..20]);
                    }
                }
            }
            0x01 => {
                if sub_value.len() >= 2 {
                    text = decode_user_data_subparameter(sub_value);
                }
            }
            _ => {}
        }
    }

    Some(DecodedMtSms {
        tl_msg_type,
        teleservice_id,
        originating_number,
        message_type: message_type_bd,
        message_id,
        text,
        reply_seq: None,
        error_class: None,
    })
}

/// Decode an MO SMS from Data Burst CHARi payload bytes.
///
/// The payload is a C.S0015-B Transport Layer message containing TLV parameters.
/// Returns `None` if the payload is not a valid SMS or cannot be decoded.
pub fn decode_mo_sms(data: &[u8]) -> Option<DecodedMoSms> {
    if data.is_empty() {
        return None;
    }

    let msg_type_tl = data[0]; // Transport Layer MSG_TYPE
    if msg_type_tl != 0x00 {
        // Not point-to-point SMS
        return None;
    }

    let mut pos = 1;
    let mut teleservice_id: u16 = 0;
    let mut destination_number = String::new();
    let mut raw_bearer_data = Vec::new();
    let mut reply_seq: Option<u8> = None;

    // Parse TLV parameters
    while pos + 2 <= data.len() {
        let tag = data[pos];
        let len = data[pos + 1] as usize;
        pos += 2;
        if pos + len > data.len() {
            break;
        }
        let value = &data[pos..pos + len];
        pos += len;

        match tag {
            0x00 => {
                // Teleservice Identifier
                if value.len() >= 2 {
                    teleservice_id = ((value[0] as u16) << 8) | (value[1] as u16);
                }
            }
            0x02 => {
                // Originating Address (in MO direction, this is the sender = the mobile)
                // We don't need this for routing, skip
            }
            0x04 => {
                // Destination Address
                destination_number = decode_address(value);
            }
            0x06 => {
                // Bearer Reply Option: REPLY_SEQ(6) | RESERVED(2)
                if !value.is_empty() {
                    reply_seq = Some((value[0] >> 2) & 0x3F);
                }
            }
            0x08 => {
                // Bearer Data
                raw_bearer_data = value.to_vec();
            }
            _ => {
                // Skip unknown parameters
            }
        }
    }

    // Parse bearer data sub-parameters
    let mut message_type_bd: u8 = 0;
    let mut message_id: u16 = 0;
    let mut text = String::new();

    let mut bpos = 0;
    while bpos + 2 <= raw_bearer_data.len() {
        let sub_tag = raw_bearer_data[bpos];
        let sub_len = raw_bearer_data[bpos + 1] as usize;
        bpos += 2;
        if bpos + sub_len > raw_bearer_data.len() {
            break;
        }
        let sub_value = &raw_bearer_data[bpos..bpos + sub_len];
        bpos += sub_len;

        match sub_tag {
            0x00 => {
                // Message Identifier: MESSAGE_TYPE(4) + MESSAGE_ID(16) + HEADER_IND(1) + RESERVED(3)
                if sub_value.len() >= 3 {
                    let bs = Bitstream::new_bytes(sub_value);
                    let bits = bs.bits();
                    if bits.len() >= 21 {
                        message_type_bd = bits_to_u8(&bits[0..4]);
                        message_id = bits_to_u16(&bits[4..20]);
                    }
                }
            }
            0x01 => {
                // User Data: MSG_ENCODING(5) + NUM_FIELDS(8) + CHARi(variable)
                if sub_value.len() >= 2 {
                    text = decode_user_data_subparameter(sub_value);
                }
            }
            _ => {}
        }
    }

    Some(DecodedMoSms {
        teleservice_id,
        destination_number,
        message_type: message_type_bd,
        message_id,
        text,
        raw_bearer_data,
        reply_seq,
    })
}

/// Decode an address TLV value (Originating or Destination Address).
/// Format: DIGIT_MODE(1) + NUMBER_MODE(1) + [optional type fields] + NUM_FIELDS(8) + digits
fn decode_address(data: &[u8]) -> String {
    let bs = Bitstream::new_bytes(data);
    let bits = bs.bits();
    if bits.len() < 10 {
        return String::new();
    }

    let digit_mode = bits[0];
    let _number_mode = bits[1];
    let mut offset = 2;

    if digit_mode == 0 {
        // 4-bit DTMF digits
        if offset + 8 > bits.len() {
            return String::new();
        }
        let num_fields = bits_to_u8(&bits[offset..offset + 8]);
        offset += 8;

        let mut number = String::new();
        for _ in 0..num_fields {
            if offset + 4 > bits.len() {
                break;
            }
            let dtmf = bits_to_u8(&bits[offset..offset + 4]);
            offset += 4;
            let ch = match dtmf {
                1 => '1',
                2 => '2',
                3 => '3',
                4 => '4',
                5 => '5',
                6 => '6',
                7 => '7',
                8 => '8',
                9 => '9',
                10 => '0',
                11 => '*',
                12 => '#',
                _ => '?',
            };
            number.push(ch);
        }
        number
    } else {
        // 8-bit ASCII digits — DIGIT_MODE=1
        // NUMBER_TYPE(3) + NUMBER_PLAN(4) + NUM_FIELDS(8) + 8-bit chars
        if offset + 3 + 4 + 8 > bits.len() {
            return String::new();
        }
        offset += 3 + 4; // skip NUMBER_TYPE + NUMBER_PLAN
        let num_fields = bits_to_u8(&bits[offset..offset + 8]);
        offset += 8;

        let mut number = String::new();
        for _ in 0..num_fields {
            if offset + 8 > bits.len() {
                break;
            }
            let ch = bits_to_u8(&bits[offset..offset + 8]);
            offset += 8;
            number.push(ch as char);
        }
        number
    }
}

/// Decode a Bearer Data User Data subparameter value.
///
/// C.S0015-B 4.5.2 defines MSG_ENCODING(5), optional MESSAGE_TYPE(8) for
/// IS-91 extended protocol and GSM DCS encodings, NUM_FIELDS(8), then CHARi.
fn decode_user_data_subparameter(data: &[u8]) -> String {
    let bs = Bitstream::new_bytes(data);
    let bits = bs.bits();
    if bits.len() < 13 {
        return String::new();
    }

    let msg_encoding = bits_to_u8(&bits[0..5]);
    let mut offset = 5;
    let encoding_message_type = if matches!(msg_encoding, 0x01 | 0x0A) {
        if offset + 8 > bits.len() {
            return String::new();
        }
        let message_type = bits_to_u8(&bits[offset..offset + 8]);
        offset += 8;
        Some(message_type)
    } else {
        None
    };

    if offset + 8 > bits.len() {
        return String::new();
    }
    let num_fields = bits_to_u8(&bits[offset..offset + 8]);
    offset += 8;

    decode_user_data(
        msg_encoding,
        encoding_message_type,
        num_fields,
        &bits[offset..],
    )
}

/// Decode user data text from CHARi bits based on MSG_ENCODING.
pub(crate) fn decode_user_data(
    msg_encoding: u8,
    encoding_message_type: Option<u8>,
    num_fields: u8,
    bits: &[u8],
) -> String {
    match msg_encoding {
        0x00 => {
            // Octet, unspecified: treat CHARi as bytes and render as UTF-8 best effort.
            decode_octet_user_data(num_fields, bits)
        }
        0x01 => {
            // IS-91 Extended Protocol Message. MESSAGE_TYPE was consumed by
            // decode_user_data_subparameter; expose printable octets best effort.
            decode_octet_user_data(num_fields, bits)
        }
        0x02 => {
            // C.R1001 7-bit ASCII.
            decode_msb_7bit_user_data(num_fields, bits)
        }
        0x03 => {
            // IA5 is a 7-bit coded character set.
            decode_msb_7bit_user_data(num_fields, bits)
        }
        0x04 => {
            // Unicode (16-bit)
            decode_unicode_user_data(num_fields, bits)
        }
        0x05 | 0x06 => {
            // Shift-JIS and Korean use NUM_FIELDS as byte length per C.S0015-B.
            // The standard library has no decoder for these; preserve printable
            // ASCII and use UTF-8 replacement for non-UTF-8 byte sequences.
            decode_octet_user_data(num_fields, bits)
        }
        0x07 | 0x08 => {
            // Latin/Hebrew and Latin are 8-bit character sets.
            decode_latin1_user_data(num_fields, bits)
        }
        0x09 => {
            // GSM 7-bit default alphabet. These septets are packed LSB-first
            // inside the CHARi octets per 3GPP TS 23.038.
            decode_gsm7_user_data(num_fields, bits)
        }
        0x0A => {
            // GSM Data-Coding-Scheme. MESSAGE_TYPE carries the SMS DCS.
            decode_gsm_dcs_user_data(encoding_message_type.unwrap_or(0), num_fields, bits)
        }
        _ => {
            // Octet / unknown — extract as 8-bit bytes, render as lossy UTF-8
            decode_octet_user_data(num_fields, bits)
        }
    }
}

fn decode_msb_7bit_user_data(num_fields: u8, bits: &[u8]) -> String {
    let mut text = String::new();
    let mut offset = 0;
    for _ in 0..num_fields {
        if offset + 7 > bits.len() {
            break;
        }
        let ch = bits_to_u8(&bits[offset..offset + 7]);
        offset += 7;
        text.push(ch as char);
    }
    text
}

fn decode_latin1_user_data(num_fields: u8, bits: &[u8]) -> String {
    read_octets(bits, num_fields as usize)
        .into_iter()
        .map(|b| b as char)
        .collect()
}

fn decode_octet_user_data(num_fields: u8, bits: &[u8]) -> String {
    String::from_utf8_lossy(&read_octets(bits, num_fields as usize)).to_string()
}

fn decode_unicode_user_data(num_fields: u8, bits: &[u8]) -> String {
    let mut text = String::new();
    let mut offset = 0;
    for _ in 0..num_fields {
        if offset + 16 > bits.len() {
            break;
        }
        let ch = bits_to_u16(&bits[offset..offset + 16]);
        offset += 16;
        if let Some(c) = char::from_u32(ch as u32) {
            text.push(c);
        }
    }
    text
}

fn decode_gsm_dcs_user_data(dcs: u8, num_fields: u8, bits: &[u8]) -> String {
    // General Data Coding indication: bits 3..2 select alphabet
    // 00 GSM 7-bit, 01 8-bit data, 10 UCS2. Other DCS groups are treated
    // conservatively as GSM 7-bit unless they explicitly select another alphabet.
    if dcs & 0xC0 == 0x00 {
        match (dcs >> 2) & 0x03 {
            0x00 => decode_gsm7_user_data(num_fields, bits),
            0x01 => decode_octet_user_data(num_fields, bits),
            0x02 => decode_unicode_user_data(num_fields, bits),
            _ => decode_gsm7_user_data(num_fields, bits),
        }
    } else {
        decode_gsm7_user_data(num_fields, bits)
    }
}

fn decode_gsm7_user_data(num_fields: u8, bits: &[u8]) -> String {
    let bytes = bits_to_packed_octets(bits);
    let mut text = String::new();
    let mut septet_index = 0usize;

    while text.chars().count() < num_fields as usize {
        let Some(septet) = read_gsm7_septet(&bytes, septet_index) else {
            break;
        };
        septet_index += 1;

        if septet == 0x1B {
            let Some(extension) = read_gsm7_septet(&bytes, septet_index) else {
                break;
            };
            septet_index += 1;
            text.push(gsm7_extension_char(extension).unwrap_or('?'));
        } else {
            text.push(gsm7_default_char(septet).unwrap_or('?'));
        }
    }
    text
}

fn read_gsm7_septet(bytes: &[u8], septet_index: usize) -> Option<u8> {
    let bit_offset = septet_index.checked_mul(7)?;
    if bit_offset + 7 > bytes.len().checked_mul(8)? {
        return None;
    }

    let mut value = 0u8;
    for bit in 0..7 {
        let absolute = bit_offset + bit;
        let byte = bytes[absolute / 8];
        if ((byte >> (absolute % 8)) & 1) != 0 {
            value |= 1 << bit;
        }
    }
    Some(value)
}

fn read_octets(bits: &[u8], max_octets: usize) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut offset = 0;
    for _ in 0..max_octets {
        if offset + 8 > bits.len() {
            break;
        }
        bytes.push(bits_to_u8(&bits[offset..offset + 8]));
        offset += 8;
    }
    bytes
}

fn bits_to_packed_octets(bits: &[u8]) -> Vec<u8> {
    bits.chunks(8)
        .map(|chunk| {
            let mut byte = 0u8;
            for (i, &bit) in chunk.iter().enumerate() {
                byte |= (bit & 1) << (7 - i);
            }
            byte
        })
        .collect()
}

fn gsm7_default_char(value: u8) -> Option<char> {
    let ch = match value {
        0x00 => '@',
        0x01 => '\u{00A3}',
        0x02 => '$',
        0x03 => '\u{00A5}',
        0x04 => '\u{00E8}',
        0x05 => '\u{00E9}',
        0x06 => '\u{00F9}',
        0x07 => '\u{00EC}',
        0x08 => '\u{00F2}',
        0x09 => '\u{00C7}',
        0x0A => '\n',
        0x0B => '\u{00D8}',
        0x0C => '\u{00F8}',
        0x0D => '\r',
        0x0E => '\u{00C5}',
        0x0F => '\u{00E5}',
        0x10 => '\u{0394}',
        0x11 => '_',
        0x12 => '\u{03A6}',
        0x13 => '\u{0393}',
        0x14 => '\u{039B}',
        0x15 => '\u{03A9}',
        0x16 => '\u{03A0}',
        0x17 => '\u{03A8}',
        0x18 => '\u{03A3}',
        0x19 => '\u{0398}',
        0x1A => '\u{039E}',
        0x1B => return None,
        0x1C => '\u{00C6}',
        0x1D => '\u{00E6}',
        0x1E => '\u{00DF}',
        0x1F => '\u{00C9}',
        0x20..=0x3F => value as char,
        0x40 => '\u{00A1}',
        0x41..=0x5A => value as char,
        0x5B => '\u{00C4}',
        0x5C => '\u{00D6}',
        0x5D => '\u{00D1}',
        0x5E => '\u{00DC}',
        0x5F => '\u{00A7}',
        0x60 => '\u{00BF}',
        0x61..=0x7A => value as char,
        0x7B => '\u{00E4}',
        0x7C => '\u{00F6}',
        0x7D => '\u{00F1}',
        0x7E => '\u{00FC}',
        0x7F => '\u{00E0}',
        _ => return None,
    };
    Some(ch)
}

fn gsm7_extension_char(value: u8) -> Option<char> {
    let ch = match value {
        0x0A => '\u{000C}',
        0x14 => '^',
        0x28 => '{',
        0x29 => '}',
        0x2F => '\\',
        0x3C => '[',
        0x3D => '~',
        0x3E => ']',
        0x40 => '|',
        0x65 => '\u{20AC}',
        _ => return None,
    };
    Some(ch)
}

fn bits_to_u8(bits: &[u8]) -> u8 {
    let mut v = 0u8;
    for &b in bits {
        v = (v << 1) | (b & 1);
    }
    v
}

fn bits_to_u16(bits: &[u8]) -> u16 {
    let mut v = 0u16;
    for &b in bits {
        v = (v << 1) | ((b & 1) as u16);
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_encode_sms_deliver_basic() {
        let bytes = encode_sms_deliver("5551234", "Hello", 1);
        // Verify structure: MSG_TYPE, then TLV params
        assert_eq!(bytes[0], 0x00); // MSG_TYPE = point-to-point
        assert_eq!(bytes[1], 0x00); // Teleservice ID tag
        assert_eq!(bytes[2], 0x02); // Teleservice ID len
        assert_eq!(bytes[3], 0x10); // WMT high
        assert_eq!(bytes[4], 0x02); // WMT low
        assert_eq!(bytes[5], 0x02); // Originating Address tag
        // Verify Bearer Data tag is present somewhere after the address
        assert!(bytes.iter().any(|&b| b == 0x08));
    }

    #[test]
    fn test_encode_sms_deliver_preserves_dtmf_originating_address() {
        let bytes = encode_sms_deliver("5551234", "Hello", 1);
        let decoded = decode_mt_sms(&bytes).expect("encoded SMS should decode");
        assert_eq!(decoded.originating_number, "5551234");
    }

    #[test]
    fn test_encode_sms_deliver_preserves_ascii_originating_address() {
        let bytes = encode_sms_deliver("1xBTS", "Hello", 1);
        let decoded = decode_mt_sms(&bytes).expect("encoded SMS should decode");
        assert_eq!(decoded.originating_number, "1xBTS");
    }

    #[test]
    fn mt_sms_data_burst_sdu_roundtrips_non_byte_aligned_text() {
        for expected in ["test", "test1", "test2"] {
            let fields = encode_sms_deliver("5551234", expected, 1);
            let dbm = crate::lac::paging_messages::ForwardDataBurstMessage {
                msg_number: 1,
                burst_type: 3,
                num_msgs: 1,
                fields,
            };
            let packed = dbm.to_sdu().to_packed_bytes();
            let mut bs = Bitstream::new_bytes(&packed);
            let decoded_dbm =
                crate::lac::paging_messages::ForwardDataBurstMessage::from_sdu(&mut bs)
                    .expect("packed DBM SDU should decode");
            let decoded = decode_mt_sms(&decoded_dbm.fields).expect("SMS should decode");
            assert_eq!(decoded.text, expected);
        }
    }

    #[test]
    fn test_bitstream_to_packed_bytes() {
        let mut bs = Bitstream::new();
        bs.write_u8(0xA5, 8);
        let packed = bitstream_to_packed_bytes(&bs);
        assert_eq!(packed, vec![0xA5]);
    }

    fn gsm7_pack_ascii(text: &str) -> Vec<u8> {
        let mut out = vec![0u8; (text.len() * 7).div_ceil(8)];
        for (septet_index, b) in text.bytes().enumerate() {
            let septet = b & 0x7F;
            for bit in 0..7 {
                if ((septet >> bit) & 1) != 0 {
                    let absolute = septet_index * 7 + bit;
                    out[absolute / 8] |= 1 << (absolute % 8);
                }
            }
        }
        out
    }

    fn append_dtmf_address_param(payload: &mut Vec<u8>, tag: u8, digits: &str) {
        let mut addr_bs = Bitstream::new();
        addr_bs.write_u8(0, 1); // DIGIT_MODE=0
        addr_bs.write_u8(0, 1); // NUMBER_MODE=0
        addr_bs.write_u8(digits.len() as u8, 8);
        for ch in digits.chars() {
            let dtmf = char_to_dtmf(ch).expect("test address should be DTMF");
            addr_bs.write_u8(dtmf, 4);
        }
        let rem = addr_bs.len() % 8;
        if rem != 0 {
            addr_bs.write_u8(0, 8 - rem);
        }

        let addr_bytes = bitstream_to_packed_bytes(&addr_bs);
        payload.push(tag);
        payload.push(addr_bytes.len() as u8);
        payload.extend_from_slice(&addr_bytes);
    }

    fn append_submit_message_identifier(bearer: &mut Vec<u8>, message_id: u16) {
        bearer.push(0x00);
        bearer.push(0x03);
        let mut id_bs = Bitstream::new();
        id_bs.write_u8(0x02, 4); // MESSAGE_TYPE = Submit
        id_bs.write_u32(message_id as u32, 16);
        id_bs.write_u8(0, 1); // HEADER_IND
        id_bs.write_u8(0, 3); // RESERVED
        bearer.extend_from_slice(&bitstream_to_packed_bytes(&id_bs));
    }

    #[test]
    fn test_decode_gsm7_user_data_hi() {
        let gsm7 = gsm7_pack_ascii("Hi");
        assert_eq!(gsm7, vec![0xC8, 0x34]);

        let mut char_bits = Bitstream::new();
        for b in gsm7 {
            char_bits.write_u8(b, 8);
        }
        assert_eq!(decode_user_data(0x09, None, 2, char_bits.bits()), "Hi");
    }

    #[test]
    fn test_decode_gsm_dcs_user_data_with_optional_message_type() {
        let mut ud_bs = Bitstream::new();
        ud_bs.write_u8(0x0A, 5); // MSG_ENCODING = GSM Data-Coding-Scheme
        ud_bs.write_u8(0x00, 8); // DCS = GSM 7-bit default alphabet
        ud_bs.write_u8(2, 8); // NUM_FIELDS
        for b in gsm7_pack_ascii("Hi") {
            ud_bs.write_u8(b, 8);
        }
        let rem = ud_bs.len() % 8;
        if rem != 0 {
            ud_bs.write_u8(0, 8 - rem);
        }

        let sub_value = bitstream_to_packed_bytes(&ud_bs);
        assert_eq!(decode_user_data_subparameter(&sub_value), "Hi");
    }

    #[test]
    fn test_decode_mo_sms_gsm7_submit_from_handset_payload() {
        let mut payload = Vec::new();
        payload.push(0x00); // Transport Layer MSG_TYPE = point-to-point
        payload.extend_from_slice(&[0x00, 0x02, 0x10, 0x02]); // Teleservice 0x1002
        append_dtmf_address_param(&mut payload, 0x04, "5551234");
        payload.extend_from_slice(&[0x06, 0x01, 0x00]); // Bearer Reply Option

        let mut bearer = Vec::new();
        append_submit_message_identifier(&mut bearer, 6);

        let mut ud_bs = Bitstream::new();
        ud_bs.write_u8(0x09, 5); // MSG_ENCODING = GSM 7-bit default alphabet
        ud_bs.write_u8(2, 8); // NUM_FIELDS
        for b in gsm7_pack_ascii("Hi") {
            ud_bs.write_u8(b, 8);
        }
        let rem = ud_bs.len() % 8;
        if rem != 0 {
            ud_bs.write_u8(0, 8 - rem);
        }
        let ud_bytes = bitstream_to_packed_bytes(&ud_bs);
        bearer.push(0x01);
        bearer.push(ud_bytes.len() as u8);
        bearer.extend_from_slice(&ud_bytes);

        payload.push(0x08);
        payload.push(bearer.len() as u8);
        payload.extend_from_slice(&bearer);

        // The live log reported an SMS Data Burst CHARi payload_len of 28.
        assert_eq!(payload.len(), 28);

        let decoded = decode_mo_sms(&payload).expect("should decode");
        assert_eq!(decoded.teleservice_id, 0x1002);
        assert_eq!(decoded.destination_number, "5551234");
        assert_eq!(decoded.message_type, 2);
        assert_eq!(decoded.message_id, 6);
        assert_eq!(decoded.text, "Hi");
        assert_eq!(decoded.reply_seq, Some(0));
    }

    #[test]
    fn test_decode_mo_sms_synthetic() {
        // Build a synthetic MO SMS Submit payload:
        // MSG_TYPE=0x00, Teleservice ID=0x1002 (WMT),
        // Destination Address=5559876, Bearer Data with MESSAGE_TYPE=2 (Submit) + User Data "Hi"
        let mut payload = Vec::new();

        // Transport Layer MSG_TYPE = point-to-point
        payload.push(0x00);

        // Teleservice ID (tag=0x00, len=2, value=0x1002)
        payload.push(0x00);
        payload.push(0x02);
        payload.push(0x10);
        payload.push(0x02);

        // Destination Address (tag=0x04)
        // DIGIT_MODE=0, NUMBER_MODE=0, NUM_FIELDS=7, then 7 DTMF digits for "5559876"
        let mut addr_bs = Bitstream::new();
        addr_bs.write_u8(0, 1); // DIGIT_MODE=0
        addr_bs.write_u8(0, 1); // NUMBER_MODE=0
        addr_bs.write_u8(7, 8); // NUM_FIELDS=7
        for &d in &[5u8, 5, 5, 9, 8, 7, 6] {
            let dtmf = if d == 0 { 10 } else { d };
            addr_bs.write_u8(dtmf, 4);
        }
        // Pad to byte boundary
        let rem = addr_bs.len() % 8;
        if rem != 0 {
            addr_bs.write_u8(0, 8 - rem);
        }
        let addr_bytes = bitstream_to_packed_bytes(&addr_bs);
        payload.push(0x04); // tag
        payload.push(addr_bytes.len() as u8);
        payload.extend_from_slice(&addr_bytes);

        // Bearer Data (tag=0x08)
        let mut bearer = Vec::new();

        // Message Identifier sub-parameter (tag=0x00, len=3)
        bearer.push(0x00);
        bearer.push(0x03);
        let mut id_bs = Bitstream::new();
        id_bs.write_u8(0x02, 4); // MESSAGE_TYPE = Submit (2)
        id_bs.write_u32(42, 16); // MESSAGE_ID = 42
        id_bs.write_u8(0, 1); // HEADER_IND
        id_bs.write_u8(0, 3); // reserved
        bearer.extend_from_slice(&bitstream_to_packed_bytes(&id_bs));

        // User Data sub-parameter (tag=0x01)
        let text = "Hi";
        let mut ud_bs = Bitstream::new();
        ud_bs.write_u8(0x02, 5); // MSG_ENCODING = 7-bit ASCII
        ud_bs.write_u8(text.len() as u8, 8);
        for &b in text.as_bytes() {
            ud_bs.write_u8(b & 0x7F, 7);
        }
        let rem = ud_bs.len() % 8;
        if rem != 0 {
            ud_bs.write_u8(0, 8 - rem);
        }
        let ud_bytes = bitstream_to_packed_bytes(&ud_bs);
        bearer.push(0x01);
        bearer.push(ud_bytes.len() as u8);
        bearer.extend_from_slice(&ud_bytes);

        payload.push(0x08); // Bearer Data tag
        payload.push(bearer.len() as u8);
        payload.extend_from_slice(&bearer);

        // Decode
        let decoded = decode_mo_sms(&payload).expect("should decode");
        assert_eq!(decoded.teleservice_id, 0x1002);
        assert_eq!(decoded.destination_number, "5559876");
        assert_eq!(decoded.message_type, 2); // Submit
        assert_eq!(decoded.message_id, 42);
        assert_eq!(decoded.text, "Hi");
    }

    #[test]
    fn test_decode_mo_sms_roundtrip_from_deliver() {
        // The forward SMS encoder produces a Deliver message. While it uses
        // Originating Address (tag=0x02) not Destination (tag=0x04), we can
        // verify that the decoder correctly ignores unknown tags and extracts
        // bearer data.
        let bytes = encode_sms_deliver("5551234", "Test", 7);
        let decoded = decode_mo_sms(&bytes).expect("should decode");
        assert_eq!(decoded.teleservice_id, 0x1002);
        // Originating Address (tag=0x02) is not destination (tag=0x04),
        // so destination_number should be empty
        assert_eq!(decoded.destination_number, "");
        assert_eq!(decoded.message_type, 1); // Deliver
        assert_eq!(decoded.message_id, 7);
        assert_eq!(decoded.text, "Test");
    }
}
