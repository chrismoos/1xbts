//! HRPD forward-traffic rate / payload / slot / modulation table.
//!
//! Source: 3GPP2 C.S0024-0 v4.0 (cdma2000 High Rate Packet Data Air Interface
//! Specification, October 2002):
//!   - Table 9.3.1.3.1.1-1 / 9.3.1.3.1.1-2 ("Modulation Parameters for the
//!     Forward Traffic Channel and the Control Channel, Part 1/2 of 2") for
//!     slots, payload bits, code rate, modulation, and per-physical-layer-packet
//!     chip counts (preamble, pilot, MAC, data).
//!   - Table 8.4.6.1.4.1-1 ("DRC Value Specification") for the mapping from
//!     the 4-bit DRC value the access terminal signals to the (rate, slot
//!     count) tuple it requests.
//!
//! C.S0024-300-C Enhanced Forward Traffic MAC subtype 1 adds DRC 0xd and
//! 0xe as 5120-bit single-user canonical transmission formats. They are only
//! valid when the session negotiates enhanced FTC MAC.
//!
//! Code rate is the *effective* rate (post-puncturing / post-repetition) shown
//! in the "Code Rate" column of Table 9.3.1.3.1.1-1/-2. Per
//! Table 9.3.1.3.2.3.2-1 the underlying mother turbo code is rate 1/5 for the
//! 1,024-bit payload at 38.4/76.8/153.6 kbps and rate 1/3 for all other
//! payloads, and the values reproduced here match that column directly.

/// Forward-traffic modulation order used by an HRPD Rev 0 rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HrpdModulation {
    Qpsk,
    Psk8,
    Qam16,
}

/// One row of the HRPD forward-traffic rate table, keyed by the 4-bit
/// DRC value the access terminal sends to request the rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ForwardRate {
    /// DRC value (4 bits) from Table 8.4.6.1.4.1-1.
    pub drc_index: u8,
    /// Information data rate in kbps.
    pub kbps: u32,
    /// Information bits per physical-layer packet (pre-encoder payload,
    /// including MAC, CRC, and tail bits — i.e. the "Bits" column).
    pub payload_bits: u32,
    /// Number of HRPD slots (1.667 ms each) per physical-layer packet.
    pub slots: u8,
    /// Forward modulation order.
    pub modulation: HrpdModulation,
    /// Effective code-rate numerator (post-puncturing).
    pub code_rate_num: u8,
    /// Effective code-rate denominator (post-puncturing).
    pub code_rate_den: u8,
    /// Number of TDM preamble chips per physical-layer packet (first entry of
    /// the "TDM Chips (Preamble, Pilot, MAC, Data)" column).
    pub preamble_chips: u32,
}

/// Forward-traffic rate rows for the negotiated Enhanced FTC MAC subtype 1
/// path, indexed by ascending DRC value.
///
/// DRC value 0x0 (null rate) and 0xf (reserved/invalid) are not represented.
pub const FORWARD_RATES: &[ForwardRate] = &[
    ForwardRate {
        drc_index: 0x1,
        kbps: 38,
        payload_bits: 1024,
        slots: 16,
        modulation: HrpdModulation::Qpsk,
        code_rate_num: 1,
        code_rate_den: 5,
        preamble_chips: 1024,
    },
    ForwardRate {
        drc_index: 0x2,
        kbps: 76,
        payload_bits: 1024,
        slots: 8,
        modulation: HrpdModulation::Qpsk,
        code_rate_num: 1,
        code_rate_den: 5,
        preamble_chips: 512,
    },
    ForwardRate {
        drc_index: 0x3,
        kbps: 153,
        payload_bits: 1024,
        slots: 4,
        modulation: HrpdModulation::Qpsk,
        code_rate_num: 1,
        code_rate_den: 5,
        preamble_chips: 256,
    },
    ForwardRate {
        drc_index: 0x4,
        kbps: 307,
        payload_bits: 1024,
        slots: 2,
        modulation: HrpdModulation::Qpsk,
        // C.S0024-0 v4.0 Table 9.3.1.3.2.3.2-1: the (1024, 2-slot) 307.2 kbps
        // format is turbo rate 1/5, not 1/3. Only the higher-payload formats
        // (2048/3072/4096-bit) and the 1-slot 614.4 kbps (DRC 0x6) use 1/3.
        code_rate_num: 1,
        code_rate_den: 5,
        preamble_chips: 128,
    },
    ForwardRate {
        drc_index: 0x5,
        kbps: 307,
        payload_bits: 2048,
        slots: 4,
        modulation: HrpdModulation::Qpsk,
        code_rate_num: 1,
        code_rate_den: 3,
        preamble_chips: 128,
    },
    ForwardRate {
        drc_index: 0x6,
        kbps: 614,
        payload_bits: 1024,
        slots: 1,
        modulation: HrpdModulation::Qpsk,
        code_rate_num: 1,
        code_rate_den: 3,
        preamble_chips: 64,
    },
    ForwardRate {
        drc_index: 0x7,
        kbps: 614,
        payload_bits: 2048,
        slots: 2,
        modulation: HrpdModulation::Qpsk,
        code_rate_num: 1,
        code_rate_den: 3,
        preamble_chips: 64,
    },
    ForwardRate {
        drc_index: 0x8,
        kbps: 921,
        payload_bits: 3072,
        slots: 2,
        modulation: HrpdModulation::Psk8,
        code_rate_num: 1,
        code_rate_den: 3,
        preamble_chips: 64,
    },
    ForwardRate {
        drc_index: 0x9,
        kbps: 1228,
        payload_bits: 2048,
        slots: 1,
        modulation: HrpdModulation::Qpsk,
        code_rate_num: 1,
        code_rate_den: 3,
        preamble_chips: 64,
    },
    ForwardRate {
        drc_index: 0xa,
        kbps: 1228,
        payload_bits: 4096,
        slots: 2,
        modulation: HrpdModulation::Qam16,
        code_rate_num: 1,
        code_rate_den: 3,
        preamble_chips: 64,
    },
    ForwardRate {
        drc_index: 0xb,
        kbps: 1843,
        payload_bits: 3072,
        slots: 1,
        modulation: HrpdModulation::Psk8,
        code_rate_num: 1,
        code_rate_den: 3,
        preamble_chips: 64,
    },
    ForwardRate {
        drc_index: 0xc,
        kbps: 2457,
        payload_bits: 4096,
        slots: 1,
        modulation: HrpdModulation::Qam16,
        code_rate_num: 1,
        code_rate_den: 3,
        preamble_chips: 64,
    },
    ForwardRate {
        drc_index: 0xd,
        kbps: 1536,
        payload_bits: 5120,
        slots: 2,
        modulation: HrpdModulation::Qam16,
        code_rate_num: 1,
        code_rate_den: 3,
        preamble_chips: 64,
    },
    ForwardRate {
        drc_index: 0xe,
        kbps: 3072,
        payload_bits: 5120,
        slots: 1,
        modulation: HrpdModulation::Qam16,
        code_rate_num: 1,
        code_rate_den: 3,
        preamble_chips: 64,
    },
];

/// Look up a forward-traffic rate row by its 4-bit DRC value.
pub fn by_drc(drc_index: u8) -> Option<&'static ForwardRate> {
    FORWARD_RATES.iter().find(|r| r.drc_index == drc_index)
}

/// Look up the first forward-traffic rate row matching the given kbps.
///
/// Note: 307.2, 614.4, and 1228.8 kbps each appear in two rows (different
/// slot counts / payload sizes); this helper returns the first match. Callers
/// that need a specific packet size should iterate `FORWARD_RATES`
/// directly or filter by `slots` / `payload_bits`.
pub fn by_kbps(kbps: u32) -> Option<&'static ForwardRate> {
    FORWARD_RATES.iter().find(|r| r.kbps == kbps)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    const SUBTYPE1_KBPS: &[u32] = &[38, 76, 153, 307, 614, 921, 1228, 1536, 1843, 2457, 3072];

    #[test]
    fn subtype1_unique_kbps_rates_present() {
        let present: HashSet<u32> = FORWARD_RATES.iter().map(|r| r.kbps).collect();
        assert_eq!(
            present.len(),
            SUBTYPE1_KBPS.len(),
            "expected subtype-1 unique information rates"
        );
        for k in SUBTYPE1_KBPS {
            assert!(present.contains(k), "missing subtype-1 rate {} kbps", k);
        }
    }

    #[test]
    fn drc_indices_are_unique_and_in_valid_range() {
        let mut seen: HashSet<u8> = HashSet::new();
        for r in FORWARD_RATES {
            assert!(
                seen.insert(r.drc_index),
                "duplicate DRC index 0x{:x}",
                r.drc_index
            );
            // C.S0024-300-C Table 1.7.6.1-2: Enhanced FTC MAC subtype 1 uses
            // non-null DRC values 0x1..=0xe.
            assert!(
                (0x1..=0xe).contains(&r.drc_index),
                "DRC 0x{:x} out of subtype-1 range",
                r.drc_index
            );
        }
        assert_eq!(FORWARD_RATES.len(), 14);
    }

    #[test]
    fn payload_bits_match_rate_times_slot_duration() {
        // One HRPD slot = 1.667 ms (600 slots / second). Therefore the payload
        // bits per physical-layer packet should equal
        //   kbps * 1000 * slots / 600
        // using the *exact* (non-truncated) rate. We re-derive the exact
        // rate from the stored truncated integer by rounding to the nearest
        // 0.1 kbps spec value.
        let exact_rate_hz = |kbps: u32| -> u32 {
            match kbps {
                38 => 38_400,
                76 => 76_800,
                153 => 153_600,
                307 => 307_200,
                614 => 614_400,
                921 => 921_600,
                1228 => 1_228_800,
                1536 => 1_536_000,
                1843 => 1_843_200,
                2457 => 2_457_600,
                3072 => 3_072_000,
                _ => panic!("unexpected kbps {}", kbps),
            }
        };
        for r in FORWARD_RATES {
            // bits = rate_bps * slot_count / slots_per_second
            let slot_seconds_num = u64::from(r.slots); // numerator
            let slots_per_second = 600u64;
            let bits = u64::from(exact_rate_hz(r.kbps)) * slot_seconds_num / slots_per_second;
            assert_eq!(
                bits,
                u64::from(r.payload_bits),
                "DRC 0x{:x}: derived {} bits != stored {} bits",
                r.drc_index,
                bits,
                r.payload_bits,
            );
        }
    }

    #[test]
    fn by_drc_lookup_round_trips() {
        for r in FORWARD_RATES {
            let found = by_drc(r.drc_index).expect("row must round-trip");
            assert_eq!(found, r);
        }
    }

    #[test]
    fn common_drc_payload_helper_matches_rate_table() {
        for r in FORWARD_RATES {
            assert_eq!(
                cdma_common::hrpd::traffic::forward_traffic_payload_bits_for_drc(r.drc_index),
                Some(r.payload_bits as usize)
            );
        }
    }

    #[test]
    fn by_drc_rejects_invalid_indices() {
        assert!(by_drc(0x0).is_none(), "null rate must not be in table");
        assert_eq!(by_drc(0xd).unwrap().payload_bits, 5120);
        assert_eq!(by_drc(0xe).unwrap().payload_bits, 5120);
        assert!(by_drc(0xf).is_none(), "reserved DRC 0xf");
        assert!(by_drc(0x10).is_none(), "out of 4-bit range");
        assert!(by_drc(0xff).is_none());
    }

    #[test]
    fn by_kbps_finds_each_unique_rate() {
        for k in SUBTYPE1_KBPS {
            let r = by_kbps(*k).unwrap_or_else(|| panic!("missing kbps={}", k));
            assert_eq!(r.kbps, *k);
        }
    }

    #[test]
    fn by_kbps_rejects_unknown_rates() {
        assert!(by_kbps(0).is_none());
        assert!(by_kbps(100).is_none());
        assert!(by_kbps(5000).is_none());
    }

    #[test]
    fn modulation_assignments_match_spec() {
        // Spot-check from Tables 9.3.1.3.1.1-1/-2.
        assert_eq!(by_drc(0x1).unwrap().modulation, HrpdModulation::Qpsk);
        assert_eq!(by_drc(0x8).unwrap().modulation, HrpdModulation::Psk8);
        assert_eq!(by_drc(0xb).unwrap().modulation, HrpdModulation::Psk8);
        assert_eq!(by_drc(0xa).unwrap().modulation, HrpdModulation::Qam16);
        assert_eq!(by_drc(0xc).unwrap().modulation, HrpdModulation::Qam16);
        assert_eq!(by_drc(0xe).unwrap().modulation, HrpdModulation::Qam16);
    }

    #[test]
    fn preamble_chip_counts_match_spec() {
        // First entry of TDM Chips column for representative rows.
        assert_eq!(by_drc(0x1).unwrap().preamble_chips, 1024);
        assert_eq!(by_drc(0x2).unwrap().preamble_chips, 512);
        assert_eq!(by_drc(0x3).unwrap().preamble_chips, 256);
        assert_eq!(by_drc(0x4).unwrap().preamble_chips, 128);
        assert_eq!(by_drc(0x6).unwrap().preamble_chips, 64);
        assert_eq!(by_drc(0xc).unwrap().preamble_chips, 64);
        assert_eq!(by_drc(0xe).unwrap().preamble_chips, 64);
    }
}
