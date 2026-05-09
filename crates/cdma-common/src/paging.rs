//! CDMA2000 slotted mode paging helpers per C.S0005-E sections 2.6.2.1.1.3,
//! 2.6.7.1, and 3.6.2.1.3.
//!
//! - **Slot** = 80 ms = 4 × 20 ms frames = 98,304 chips (at 1.2288 Mcps)
//! - **SLOT_NUM** = `floor(t / 4) mod 2048` where `t` = system time in 20 ms frames
//! - **PGSLOT** = hash(IMSI) in 0..2047
//! - MS monitors when `(SLOT_NUM - PGSLOT) mod (16 * T) == 0`, `T = 2^slot_cycle_index`

/// Compute PGSLOT (0..2047) from IMSI per C.S0005-E 2.6.7.1.
///
/// `imsi_m_s1` is the 24-bit IMSI_M_S1 field.
/// `imsi_m_s2` is the 10-bit IMSI_M_S2 field.
///
/// HASH_KEY = (IMSI_O_S1 + 2^24 * IMSI_O_S2) & 0xFFFFFFFF
/// where IMSI_O_S1 = IMSI_M_S1, IMSI_O_S2 = IMSI_M_S2 for MIN-based IMSI.
/// L = bits[0:15], H = bits[16:31]
/// DECORR = 6 * (HASH_KEY & 0xFFF)
/// PGSLOT = floor(2048 * ((40503 * (L XOR H XOR DECORR)) mod 2^16) / 2^16)
pub fn compute_pgslot(imsi_m_s1: u32, imsi_m_s2: u16) -> u16 {
    let hash_key: u32 = (imsi_m_s1 as u64 + ((imsi_m_s2 as u64) << 24)) as u32;
    let l = (hash_key & 0xFFFF) as u16;
    let h = ((hash_key >> 16) & 0xFFFF) as u16;
    let decorr = (6u32 * (hash_key & 0xFFF)) as u16;
    let xor_val = l ^ h ^ decorr;
    let product = 40503u32.wrapping_mul(xor_val as u32) & 0xFFFF;
    ((2048u32 * product) >> 16) as u16
}

/// Compute SLOT_NUM (0..2047) from an absolute chip position.
///
/// SLOT_NUM = floor(t / 4) mod 2048, where t = chip_cursor / chips_per_20ms.
/// chips_per_20ms = chip_rate_hz / 50.
pub fn slot_num_from_chips(chip_cursor: u64, chip_rate_hz: u64) -> u16 {
    let chips_per_20ms = chip_rate_hz / 50;
    let t = chip_cursor / chips_per_20ms; // system time in 20ms frames
    ((t / 4) % 2048) as u16
}

/// Check if `chip_cursor` falls within an 80ms slot assigned to `pgslot`.
///
/// The MS monitors when `(SLOT_NUM - PGSLOT) mod (16 * T) == 0`,
/// where `T = 2^slot_cycle_index`.
pub fn is_assigned_slot(
    chip_cursor: u64,
    pgslot: u16,
    slot_cycle_index: u8,
    chip_rate_hz: u64,
) -> bool {
    let slot_num = slot_num_from_chips(chip_cursor, chip_rate_hz);
    let t: u16 = 1u16 << (slot_cycle_index as u16);
    let cycle = 16u16.saturating_mul(t);
    let diff = (slot_num as i32 - pgslot as i32).rem_euclid(2048) as u16;
    diff.is_multiple_of(cycle)
}

/// Return the chip position of the next assigned slot start (>= `current_chip`).
///
/// Scans forward from the current SLOT_NUM looking for the next slot where
/// `(SLOT_NUM - PGSLOT) mod (16 * T) == 0`.
pub fn next_assigned_slot_chip(
    current_chip: u64,
    pgslot: u16,
    slot_cycle_index: u8,
    chip_rate_hz: u64,
) -> u64 {
    let chips_per_20ms = chip_rate_hz / 50;
    let slot_chips = chips_per_20ms * 4;
    let current_slot_start = (current_chip / slot_chips) * slot_chips;

    let t: u16 = 1u16 << (slot_cycle_index as u16);
    let cycle = 16u16.saturating_mul(t);

    // Current SLOT_NUM at current_slot_start
    let t_frames = current_slot_start / chips_per_20ms;
    let current_slot_num = ((t_frames / 4) % 2048) as u16;

    // Search forward through SLOT_NUMs to find the next assigned one
    for offset in 0..2048u16 {
        let candidate_slot_num = (current_slot_num + offset) % 2048;
        let diff = (candidate_slot_num as i32 - pgslot as i32).rem_euclid(2048) as u16;
        if diff.is_multiple_of(cycle) {
            let candidate_chip = current_slot_start + (offset as u64) * slot_chips;
            // Make sure candidate is >= current_chip
            if candidate_chip >= current_chip {
                return candidate_chip;
            }
        }
    }

    // Fallback: wrap around full cycle (should not happen with cycle <= 2048)
    current_slot_start + 2048 * slot_chips
}

/// Derive IMSI_M_S1_p (24 bits) and IMSI_M_S2_p (10 bits) from a full IMSI
/// string per C.S0005-E 2.3.1 / 2.3.1.1.
///
/// The IMSI_S is the last 10 digits of the IMSI (zero-padded to 15 digits
/// if shorter). IMSI_S is split into IMSI_S2 (first 3 digits) and IMSI_S1
/// (last 7 digits), then encoded into the 34-bit binary representation.
pub fn imsi_s_from_imsi(imsi: &str) -> Option<(u32, u16)> {
    let digits: Vec<u8> = imsi
        .bytes()
        .filter_map(|b| {
            if b.is_ascii_digit() {
                Some(b - b'0')
            } else {
                None
            }
        })
        .collect();
    if digits.is_empty() || digits.len() > 15 {
        return None;
    }
    // Zero-pad to 15 digits to get IMSI_S from last 10
    let padded_len = 15;
    let pad_count = padded_len - digits.len();
    let mut padded = vec![0u8; pad_count];
    padded.extend_from_slice(&digits);
    // IMSI_S = last 10 digits of the 15-digit padded IMSI
    let imsi_s = &padded[5..15];

    // IMSI_S2 = first 3 digits of IMSI_S → 10 bits
    let imsi_s2 = encode_three_digits(imsi_s[0], imsi_s[1], imsi_s[2]);

    // IMSI_S1 = last 7 digits of IMSI_S → 24 bits
    // Upper 10 bits: second 3 digits (imsi_s[3..6])
    let s1_upper = encode_three_digits(imsi_s[3], imsi_s[4], imsi_s[5]);
    // Middle 4 bits: thousands digit (imsi_s[6]) as BCD
    let s1_mid = encode_bcd_digit(imsi_s[6]);
    // Lower 10 bits: last 3 digits (imsi_s[7..10])
    let s1_lower = encode_three_digits(imsi_s[7], imsi_s[8], imsi_s[9]);
    let imsi_m_s1: u32 = ((s1_upper as u32) << 14) | ((s1_mid as u32) << 10) | (s1_lower as u32);

    Some((imsi_m_s1, imsi_s2))
}

/// Encode a 3-digit MCC string (e.g. "310") into its 10-bit binary
/// representation per C.S0005-E 2.3.1.3.  Returns `None` for invalid input.
pub fn mcc_from_digits(s: &str) -> Option<u16> {
    let digits: Vec<u8> = s.bytes().map(|b| b.wrapping_sub(b'0')).collect();
    if digits.len() != 3 || digits.iter().any(|&d| d > 9) {
        return None;
    }
    Some(encode_three_digits(digits[0], digits[1], digits[2]))
}

/// Encode a 2-digit IMSI_11_12 string (e.g. "55") into its 7-bit binary
/// representation per C.S0005-E 2.3.1.2.  Returns `None` for invalid input.
pub fn imsi_11_12_from_digits(s: &str) -> Option<u8> {
    let digits: Vec<u8> = s.bytes().map(|b| b.wrapping_sub(b'0')).collect();
    if digits.len() != 2 || digits.iter().any(|&d| d > 9) {
        return None;
    }
    let v = |d: u8| -> u16 { if d == 0 { 10 } else { d as u16 } };
    let encoded = 10 * v(digits[0]) + v(digits[1]) - 11;
    if encoded > 99 {
        return None;
    }
    Some(encoded as u8)
}

/// Encode 3 decimal digits into 10 bits per C.S0005-E Table 2.3.1.1-1.
/// Digit 0 is given the value 10: `100*D1 + 10*D2 + D3 - 111`.
fn encode_three_digits(d1: u8, d2: u8, d3: u8) -> u16 {
    let v = |d: u8| -> u16 { if d == 0 { 10 } else { d as u16 } };
    100 * v(d1) + 10 * v(d2) + v(d3) - 111
}

/// Encode a single decimal digit as 4-bit BCD per C.S0005-E Table 2.3.1.1-2.
/// Digit 0 maps to 0b1010.
fn encode_bcd_digit(d: u8) -> u8 {
    if d == 0 { 0b1010 } else { d }
}

fn decode_three_digit_field(encoded: u16) -> Option<[u8; 3]> {
    if encoded > 999 {
        return None;
    }

    let s = encoded + 111;
    let d1 = (s - 11) / 100;
    let r = s - 100 * d1;
    let d2 = (r - 1) / 10;
    let d3 = r - 10 * d2;

    let to_digit = |v: u16| -> Option<u8> {
        match v {
            1..=9 => Some(v as u8),
            10 => Some(0),
            _ => None,
        }
    };

    Some([to_digit(d1)?, to_digit(d2)?, to_digit(d3)?])
}

/// Decode the 10-bit MCC_M/MCC_T field into a 3-digit MCC string per
/// C.S0005-E 2.3.1.3.
pub fn mcc_to_digits(encoded_mcc: u16) -> Option<String> {
    let digits = decode_three_digit_field(encoded_mcc)?;
    Some(format!("{}{}{}", digits[0], digits[1], digits[2]))
}

/// Decode the 7-bit IMSI_M_11_12/IMSI_T_11_12 field into its two decimal
/// digits per C.S0005-E 2.3.1.2.
pub fn imsi_11_12_to_digits(encoded_imsi_11_12: u8) -> Option<String> {
    if encoded_imsi_11_12 > 99 {
        return None;
    }

    // Spec: encoded = 10 * N(P11) + N(P12) - 11, where N(0) = 10 and
    // N(d) = d for d in 1..=9. Inverse: with s = encoded + 11,
    // N(P12) is the low component (1..=10) and N(P11) is the high component.
    let s = encoded_imsi_11_12 as u16 + 11;
    let n_p12 = ((s - 1) % 10) + 1;
    let n_p11 = (s - n_p12) / 10;

    let to_digit = |v: u16| -> Option<u8> {
        match v {
            1..=9 => Some(v as u8),
            10 => Some(0),
            _ => None,
        }
    };

    Some(format!("{}{}", to_digit(n_p11)?, to_digit(n_p12)?))
}

/// Reverse-derive a 10-digit IMSI_S decimal string from IMSI_M_S1_p (24 bits)
/// and IMSI_M_S2_p (10 bits). Inverse of `imsi_s_from_imsi`, returning `None`
/// when any encoded digit group is outside the C.S0005-E decimal mapping.
pub fn imsi_s_to_digits_checked(imsi_m_s1: u32, imsi_m_s2: u16) -> Option<String> {
    let bcd_rev = |v: u8| -> u8 { if v == 0b1010 { 0 } else { v } };

    // IMSI_S2 = upper 10 bits of the 34-bit value → first 3 digits
    let s2_digits = decode_three_digit_field(imsi_m_s2)?;
    // IMSI_S1 breakdown: [second_3(10 bits) | thousands_bcd(4 bits) | last_3(10 bits)]
    let s1_upper = ((imsi_m_s1 >> 14) & 0x3FF) as u16;
    let s1_mid = ((imsi_m_s1 >> 10) & 0xF) as u8;
    let s1_lower = (imsi_m_s1 & 0x3FF) as u16;
    let second_3 = decode_three_digit_field(s1_upper)?;
    if s1_mid > 9 && s1_mid != 0b1010 {
        return None;
    }
    let thousands = bcd_rev(s1_mid);
    let last_3 = decode_three_digit_field(s1_lower)?;

    Some(format!(
        "{}{}{}{}{}{}{}{}{}{}",
        s2_digits[0],
        s2_digits[1],
        s2_digits[2],
        second_3[0],
        second_3[1],
        second_3[2],
        thousands,
        last_3[0],
        last_3[1],
        last_3[2],
    ))
}

/// Reverse-derive a 10-digit IMSI_S decimal string from IMSI_M_S1_p (24 bits)
/// and IMSI_M_S2_p (10 bits). Inverse of `imsi_s_from_imsi`.
pub fn imsi_s_to_digits(imsi_m_s1: u32, imsi_m_s2: u16) -> String {
    imsi_s_to_digits_checked(imsi_m_s1, imsi_m_s2).unwrap_or_else(|| "0000000000".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::consts::SR1_CHIP_RATE_HZ;

    #[test]
    fn test_compute_pgslot_range() {
        // PGSLOT must be in 0..2048
        // BB 8830 correct values per C.S0005-E 2.3.1: S1=lower 24 bits, S2=upper 10 bits
        let pgslot = compute_pgslot(9541790, 806);
        assert!(pgslot < 2048, "pgslot={} out of range", pgslot);
    }

    #[test]
    fn test_compute_pgslot_deterministic() {
        let a = compute_pgslot(9541790, 806);
        let b = compute_pgslot(9541790, 806);
        assert_eq!(a, b);
    }

    #[test]
    fn test_compute_pgslot_bb8830() {
        // BB 8830 IMSI_S = 0x32691989E (34 bits)
        // Per C.S0005-E 2.3.1: IMSI_S = IMSI_S2(10 upper) || IMSI_S1(24 lower)
        // imsi_m_s1 = 0x91989E = 9541790, imsi_m_s2 = 0x326 = 806
        let pgslot = compute_pgslot(9541790, 806);
        // Verify step-by-step against the spec formula:
        // HASH_KEY = (IMSI_O_S1 + 2^24 * IMSI_O_S2) & 0xFFFFFFFF
        //          = (0x0091989E + 0x26000000) = 0x2691989E
        let hash_key: u32 = (9541790u64 + (806u64 << 24)) as u32;
        assert_eq!(hash_key, 0x2691989E);
        let l = (hash_key & 0xFFFF) as u16;
        let h = ((hash_key >> 16) & 0xFFFF) as u16;
        let decorr = (6u32 * (hash_key & 0xFFF)) as u16;
        let xor_val = l ^ h ^ decorr;
        let product = 40503u32.wrapping_mul(xor_val as u32) & 0xFFFF;
        let expected = ((2048u32 * product) >> 16) as u16;
        assert_eq!(pgslot, expected);
        assert_eq!(pgslot, 1769);
        assert!(pgslot < 2048);
        eprintln!(
            "BB8830: hash_key=0x{:08X} L=0x{:04X} H=0x{:04X} DECORR={} xor=0x{:04X} pgslot={}",
            hash_key, l, h, decorr, xor_val, pgslot
        );
    }

    #[test]
    fn test_slot_num_from_chips() {
        // At chip 0, SLOT_NUM = 0
        assert_eq!(slot_num_from_chips(0, SR1_CHIP_RATE_HZ), 0);
        // One 80ms slot = 4 × 20ms frames = 98304 chips
        // After 1 slot: t=4, SLOT_NUM = floor(4/4) mod 2048 = 1
        assert_eq!(slot_num_from_chips(98_304, SR1_CHIP_RATE_HZ), 1);
        // After 2048 slots: wraps to 0
        assert_eq!(slot_num_from_chips(2048 * 98_304, SR1_CHIP_RATE_HZ), 0);
    }

    #[test]
    fn test_is_assigned_slot_cycle_0() {
        // With slot_cycle_index=0: T=1, cycle=16
        // MS monitors slots where (SLOT_NUM - PGSLOT) mod 16 == 0
        let pgslot = 5;
        // SLOT_NUM=5 => (5-5) mod 16 = 0 => assigned
        let chip_at_slot5 = 5u64 * 98_304;
        assert!(is_assigned_slot(chip_at_slot5, pgslot, 0, SR1_CHIP_RATE_HZ));
        // SLOT_NUM=21 => (21-5) mod 16 = 0 => assigned
        let chip_at_slot21 = 21u64 * 98_304;
        assert!(is_assigned_slot(
            chip_at_slot21,
            pgslot,
            0,
            SR1_CHIP_RATE_HZ
        ));
        // SLOT_NUM=6 => (6-5) mod 16 = 1 => not assigned
        let chip_at_slot6 = 6u64 * 98_304;
        assert!(!is_assigned_slot(
            chip_at_slot6,
            pgslot,
            0,
            SR1_CHIP_RATE_HZ
        ));
    }

    #[test]
    fn test_next_assigned_slot_chip() {
        let pgslot = 5;
        // Starting from chip 0 (SLOT_NUM=0), next assigned is SLOT_NUM=5
        let next = next_assigned_slot_chip(0, pgslot, 0, SR1_CHIP_RATE_HZ);
        assert_eq!(next, 5 * 98_304);

        // Starting from slot 5 exactly, should return slot 5
        let next = next_assigned_slot_chip(5 * 98_304, pgslot, 0, SR1_CHIP_RATE_HZ);
        assert_eq!(next, 5 * 98_304);

        // Starting from slot 6, next assigned is slot 21 (5 + 16)
        let next = next_assigned_slot_chip(6 * 98_304, pgslot, 0, SR1_CHIP_RATE_HZ);
        assert_eq!(next, 21 * 98_304);
    }

    #[test]
    fn test_next_assigned_slot_chip_mid_slot() {
        let pgslot = 5;
        // Starting mid-slot at slot 5 (chip offset 100 into the slot)
        let next = next_assigned_slot_chip(5 * 98_304 + 100, pgslot, 0, SR1_CHIP_RATE_HZ);
        // Next assigned slot is 21 since we're past the start of slot 5
        assert_eq!(next, 21 * 98_304);
    }

    #[test]
    fn test_cycle_index_1() {
        // slot_cycle_index=1: T=2, cycle=32
        let pgslot = 10;
        assert!(is_assigned_slot(10 * 98_304, pgslot, 1, SR1_CHIP_RATE_HZ));
        assert!(!is_assigned_slot(26 * 98_304, pgslot, 1, SR1_CHIP_RATE_HZ));
        assert!(is_assigned_slot(42 * 98_304, pgslot, 1, SR1_CHIP_RATE_HZ));
    }

    #[test]
    fn test_imsi_s_from_imsi_spec_example() {
        // C.S0005-E 2.3.1.1 example: IMSI_T = 123456789 (9 digits)
        // Padded to 15 digits: 000000123456789
        // IMSI_S = last 10 digits = 0123456789
        // IMSI_S2 = "012" → D1=10,D2=1,D3=2 → 100*10+10*1+2-111 = 901
        // IMSI_S1 second 3 = "345" → 100*3+10*4+5-111 = 234
        // IMSI_S1 thousands = "6" → BCD 0110
        // IMSI_S1 last 3 = "789" → 100*7+10*8+9-111 = 678
        // IMSI_S1 = (234 << 14) | (6 << 10) | 678
        let result = imsi_s_from_imsi("123456789");
        assert!(result.is_some());
        let (s1, s2) = result.unwrap();
        assert_eq!(s2, 901);
        let expected_s1 = (234u32 << 14) | (0b0110u32 << 10) | 678;
        assert_eq!(s1, expected_s1);
    }

    #[test]
    fn test_imsi_s_from_imsi_15_digit() {
        // 15-digit IMSI: "310260123456789"
        // IMSI_S = last 10 digits = "0123456789"
        // Same result as the 9-digit example padded
        let result = imsi_s_from_imsi("310260123456789");
        assert!(result.is_some());
        let (s1, s2) = result.unwrap();
        assert_eq!(s2, 901); // "012"
        let expected_s1 = (234u32 << 14) | (0b0110u32 << 10) | 678;
        assert_eq!(s1, expected_s1);
    }

    #[test]
    fn test_imsi_s_from_imsi_all_zeros() {
        // "0000000000" → IMSI_S = "0000000000"
        // IMSI_S2 = "000" → 100*10+10*10+10-111 = 999
        // IMSI_S1 second 3 = "000" → 999
        // IMSI_S1 thousands = "0" → BCD 1010
        // IMSI_S1 last 3 = "000" → 999
        let result = imsi_s_from_imsi("0000000000");
        assert!(result.is_some());
        let (s1, s2) = result.unwrap();
        assert_eq!(s2, 999);
        let expected_s1 = (999u32 << 14) | (0b1010u32 << 10) | 999;
        assert_eq!(s1, expected_s1);
    }

    #[test]
    fn test_imsi_s_round_trip_spec_example() {
        let (s1, s2) = imsi_s_from_imsi("123456789").unwrap();
        let digits = imsi_s_to_digits(s1, s2);
        let (rt_s1, rt_s2) = imsi_s_from_imsi(&digits).unwrap();
        assert_eq!(rt_s1, s1);
        assert_eq!(rt_s2, s2);
    }

    #[test]
    fn test_imsi_s_round_trip_all_zeros() {
        let (s1, s2) = imsi_s_from_imsi("0000000000").unwrap();
        let digits = imsi_s_to_digits(s1, s2);
        let (rt_s1, rt_s2) = imsi_s_from_imsi(&digits).unwrap();
        assert_eq!(rt_s1, s1);
        assert_eq!(rt_s2, s2);
    }

    #[test]
    fn test_imsi_s_round_trip_live_values() {
        let s1: u32 = 16369843;
        let s2: u16 = 999;
        let digits = imsi_s_to_digits_checked(s1, s2).unwrap();
        assert_eq!(digits, "0000002280");
        let (rt_s1, rt_s2) = imsi_s_from_imsi(&digits).unwrap();
        assert_eq!(rt_s1, s1, "s1 round-trip failed for digits={}", digits);
        assert_eq!(rt_s2, s2, "s2 round-trip failed for digits={}", digits);
    }

    #[test]
    fn test_imsi_s_to_digits_checked_rejects_invalid_digit_groups() {
        assert_eq!(imsi_s_to_digits_checked(0, 1000), None);
        assert_eq!(imsi_s_to_digits_checked(0x0000_3c00, 999), None);
    }

    #[test]
    fn test_mcc_to_digits_spec_encoding() {
        // MCC 310: 100*3 + 10*1 + 10 - 111 = 209.
        assert_eq!(mcc_to_digits(209).as_deref(), Some("310"));
        // MCC 000: 100*10 + 10*10 + 10 - 111 = 999.
        assert_eq!(mcc_to_digits(999).as_deref(), Some("000"));
        assert_eq!(mcc_to_digits(1000), None);
    }

    #[test]
    fn test_imsi_11_12_to_digits_spec_encoding() {
        // Digits "26" (P11=2, P12=6): 10*N(P11) + N(P12) - 11 = 10*2 + 6 - 11 = 15.
        assert_eq!(imsi_11_12_to_digits(15).as_deref(), Some("26"));
        // Digits "62" (P11=6, P12=2): 10*6 + 2 - 11 = 51.
        assert_eq!(imsi_11_12_to_digits(51).as_deref(), Some("62"));
        // Digits "00": 10*10 + 10 - 11 = 99.
        assert_eq!(imsi_11_12_to_digits(99).as_deref(), Some("00"));
        // Digits "10" (P11=1, P12=0 → N(P12)=10): 10*1 + 10 - 11 = 9.
        assert_eq!(imsi_11_12_to_digits(9).as_deref(), Some("10"));
        // Digits "01" (P11=0, P12=1 → N(P11)=10): 10*10 + 1 - 11 = 90.
        assert_eq!(imsi_11_12_to_digits(90).as_deref(), Some("01"));
        assert_eq!(imsi_11_12_to_digits(0x7f), None);
    }

    /// Asymmetric digit pairs (P11 != P12) must round-trip exactly. The
    /// previous decoder swapped the two output digits so this passed only
    /// for symmetric pairs like "00", "55", "99".
    #[test]
    fn imsi_11_12_round_trips_for_all_valid_pairs() {
        for d11 in 0u8..=9 {
            for d12 in 0u8..=9 {
                let s = format!("{d11}{d12}");
                let encoded = imsi_11_12_from_digits(&s).expect("valid 2-digit");
                let decoded = imsi_11_12_to_digits(encoded).expect("valid raw");
                assert_eq!(
                    decoded, s,
                    "round-trip failed for digits=\"{s}\" raw={encoded}"
                );
            }
        }
    }

    #[test]
    fn mcc_from_digits_roundtrip() {
        assert_eq!(mcc_from_digits("310"), Some(209));
        assert_eq!(mcc_from_digits("000"), Some(999));
        assert_eq!(
            mcc_to_digits(mcc_from_digits("310").unwrap()).as_deref(),
            Some("310")
        );
        assert_eq!(mcc_from_digits("31"), None);
        assert_eq!(mcc_from_digits("3100"), None);
    }

    #[test]
    fn imsi_11_12_from_digits_roundtrip() {
        assert_eq!(imsi_11_12_from_digits("55"), Some(44));
        assert_eq!(imsi_11_12_from_digits("00"), Some(99));
        assert_eq!(
            imsi_11_12_to_digits(imsi_11_12_from_digits("55").unwrap()).as_deref(),
            Some("55"),
        );
        assert_eq!(imsi_11_12_from_digits("5"), None);
        assert_eq!(imsi_11_12_from_digits("555"), None);
    }
}
