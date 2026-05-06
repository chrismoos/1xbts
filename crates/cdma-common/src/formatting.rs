use crate::bits::Bitstream;

/// Format a byte slice as uppercase hex string (e.g. "0A1BFF").
pub fn bytes_to_hex(bytes: &[u8]) -> String {
    use std::fmt::Write;
    let mut s = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        write!(s, "{byte:02X}").unwrap();
    }
    s
}

/// Format a `Bitstream` as uppercase hex string, padding the last byte with
/// trailing zeros when the bit count is not a multiple of 8.
pub fn bitstream_to_hex(bs: &Bitstream) -> String {
    let bytes = bs
        .bits()
        .chunks(8)
        .map(|chunk| chunk.iter().fold(0u8, |acc, bit| (acc << 1) | (bit & 1)) << (8 - chunk.len()))
        .collect::<Vec<_>>();
    bytes_to_hex(&bytes)
}

/// Forward-link order name per C.S0005-E Table 3.7.4-1.
///
/// Covers both f-csch (paging channel) and f-dsch (traffic channel) order codes.
pub fn forward_order_name(order: u8) -> &'static str {
    match order {
        // f-csch ORDER codes (C.S0005-E Table 3.7.4-1)
        0b000001 => "Abbreviated Alert",
        0b000010 => "Base Station Challenge Confirmation",
        0b000011 => "Message Encryption Mode",
        0b000100 => "Reorder",
        0b000101 => "Parameter Update",
        0b000110 => "Audit",
        0b001001 => "Intercept",
        0b001010 => "Maintenance",
        // f-dsch ORDER codes (C.S0005-E Table 3.7.4-1)
        0b010000 => "Base Station Acknowledgment",
        0b010001 => "Pilot Measurement Request",
        0b010010 => "Lock Until Power-Cycled",
        0b010011 => "Maintenance Required",
        0b010100 => "Unlock",
        0b010101 => "Release",
        0b010110 => "Outer Loop Report Request",
        0b010111 => "Long Code Transition",
        0b011001 => "Continuous DTMF Tone",
        0b011010 => "Status Request",
        0b011011 => "Registration Accepted",
        0b011100 => "Registration Rejected",
        0b011110 => "Local Control",
        0b100001 => "Connect",
        _ => "Unknown Order",
    }
}

/// Reverse-link (r-dsch / r-csch) order name per C.S0005-E Table 2.7.3-1.
pub fn reverse_order_name(order: u8) -> &'static str {
    match order {
        0b000010 => "Base Station Challenge",
        0b000011 => "SSD Update Confirmation/Rejection",
        0b000101 => "Parameter Update Confirmation",
        0b010000 => "Mobile Station Acknowledgment",
        0b010011 => "Service Option Request",
        0b010100 => "Service Option Response",
        0b010101 => "Release",
        0b010111 => "Long Code Transition",
        0b011000 => "Connect",
        0b011001 => "Continuous DTMF Tone",
        0b011101 => "Service Option Control",
        0b011110 => "Local Control Response",
        0b011111 => "Mobile Station Reject",
        0b100000 => "Call Rescue Cancel",
        0b100001 => "Security Mode Completion",
        0b100010 => "Fast Call Setup",
        0b100011 => "Shared Channel Configuration",
        _ => "Unknown Order",
    }
}

/// Mobile Station Reject Order ORDQ reason per C.S0005-E.
pub fn mobile_station_reject_reason(ordq: u8) -> &'static str {
    match ordq {
        0x01 => "unspecified reason",
        0x02 => "message not accepted in this state",
        0x03 => "message structure not acceptable",
        0x04 => "message field not in valid range",
        0x05 => "message type or order code not understood",
        0x06 => "capability not supported by mobile station",
        0x07 => "cannot be handled by current mobile station configuration",
        0x08 => "response message would exceed allowable length",
        0x09 => "info record not supported for specified band class/operating mode",
        0x0A => "search set not specified",
        0x0B => "invalid search request",
        0x0C => "invalid Frequency Assignment",
        0x0D => "search period too short",
        0x0E => "RC does not match DEFAULT_CONFIG value",
        0x10 => "call assignment not accepted",
        0x11 => "no call control instance with specified identifier",
        0x12 => "call control instance already present with specified identifier",
        0x13 => "TAG received does not match any stored TAG",
        0x14 => "UAK not supported",
        0x15 => "stored configuration already restored at channel assignment",
        0x16 => "MAC-I field is missing",
        0x18 => "MAC-I field is present but invalid",
        0x19 => "security sequence number is invalid",
        0x1A => "message cannot be decrypted",
        0x1B => "requested stored service configuration not available",
        0x1C => "PLCM_TYPE mismatch",
        0x1D => "General Extension Record contains unsupported record type",
        0x1E => "General Extension Record field value outside permissible range",
        0x1F => "General Extension Record field value not supported",
        0x20 => "General Extension Record not acceptable, unspecified reason",
        _ => "unknown ORDQ",
    }
}

/// Rejected PDU type name per C.S0005-E.
pub fn rejected_pdu_type_name(rejected_pdu_type: u8) -> &'static str {
    match rejected_pdu_type {
        0b00 => "20 ms regular message",
        0b01 => "5 ms mini message",
        _ => "reserved",
    }
}

/// Convert DTMF-encoded origination digits to a string.
///
/// Per C.S0005-E 2.7.4.1: when `digit_mode` is false, each 4-bit digit
/// uses DTMF encoding (1-9 -> '1'-'9', 0xA -> '0', 0xB -> '*', 0xC -> '#').
/// When `digit_mode` is true, digits are 8-bit ASCII characters.
pub fn format_dtmf_digits(digits: &[u8], digit_mode: bool) -> String {
    if digits.is_empty() {
        return String::new();
    }
    if digit_mode {
        return String::from_utf8_lossy(digits).to_string();
    }
    digits
        .iter()
        .map(|digit| match digit & 0x0f {
            0x1..=0x9 => char::from(b'0' + (digit & 0x0f)),
            0x0A => '0',
            0x0B => '*',
            0x0C => '#',
            _ => '?',
        })
        .collect()
}
