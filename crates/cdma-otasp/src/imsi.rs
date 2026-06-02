//! IMSI encoding for OTASP NAM blocks — re-exports of `cdma_common::paging`
//! helpers (C.S0005-E §2.3.1).

pub use cdma_common::paging::{
    imsi_11_12_from_digits, imsi_11_12_to_digits, imsi_s_from_imsi, imsi_s_to_digits_checked,
    mcc_from_digits, mcc_to_digits,
};

#[cfg(test)]
mod tests {
    use super::*;

    // C.S0005-E §2.3.1.3: MCC = (d1·100 + d2·10 + d3) − 1, 0→10. "310" → 209.
    #[test]
    fn mcc_310_encodes_via_common_helper() {
        assert_eq!(mcc_from_digits("310").unwrap(), 209);
        assert_eq!(mcc_to_digits(209).unwrap(), "310");
    }

    /// IMSI_11_12 "55" -> 10*5 + 5 - 11 = 44.
    #[test]
    fn imsi_11_12_55_encodes_to_44() {
        assert_eq!(imsi_11_12_from_digits("55").unwrap(), 44);
        assert_eq!(imsi_11_12_to_digits(44).unwrap(), "55");
    }

    /// IMSI_11_12 "00" -> 10*10 + 10 - 11 = 99.
    #[test]
    fn imsi_11_12_00_encodes_to_99() {
        assert_eq!(imsi_11_12_from_digits("00").unwrap(), 99);
        assert_eq!(imsi_11_12_to_digits(99).unwrap(), "00");
    }

    /// IMSI_M_S round-trip on a 15-digit IMSI. Last 10 digits = "0123456789".
    /// Encode then decode must return the same digit string.
    #[test]
    fn imsi_s_round_trip_with_zero_digit() {
        let (s1, s2) = imsi_s_from_imsi("310170123456789").unwrap();
        let back = imsi_s_to_digits_checked(s1, s2).unwrap();
        assert_eq!(back, "0123456789");
    }

    /// IMSI_M_S round-trip on a 10-digit MIN-style input. Zero-padded to 15.
    #[test]
    fn imsi_s_round_trip_short_imsi() {
        let (s1, s2) = imsi_s_from_imsi("5551234567").unwrap();
        let back = imsi_s_to_digits_checked(s1, s2).unwrap();
        assert_eq!(back, "5551234567");
    }
}
