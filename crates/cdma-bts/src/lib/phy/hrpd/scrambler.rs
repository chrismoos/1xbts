//! HRPD forward-link symbol scrambler (C.S0024-200-C §1.4.1.3.2.3.3).
//!
//! The forward-traffic scrambler is a 17-bit Fibonacci LFSR with
//! characteristic polynomial h(D) = D^17 + D^14 + 1. At the start of each
//! physical-layer packet the register is loaded with
//! `[1 1 1 1 1 1 1  r5 r4 r3 r2 r1 r0  d3 d2 d1 d0]`,
//! where `r5..r0` are the 6-bit preamble MACIndex (§1.4.1.3.2.1.3) and
//! `d3..d0` encode the data rate / packet size (Table 1.4.1.3.2.3.3-1).
//! The loaded initial state itself yields the first scrambling bit; the
//! register is then clocked once per encoder output symbol.
//!
//! Convention used here: state bit 16 is the leftmost bit in the spec
//! figure and state bit 0 is the rightmost bit (`d0`). The scrambling
//! sequence is tapped from the leftmost bit. After emitting that bit,
//! feedback `bit16 XOR bit13` is shifted into bit 0 and the rest of the
//! state shifts toward bit 16.
//!
//! C.S0024-200-C also defines a structurally identical reverse-link
//! scrambler (§2.3.1.3.5) whose initial-state load differs: the MACIndex
//! field is replaced by eleven leading `1`s plus the 2-bit reverse-link
//! interlace-offset value `i1 i0`. That seeding is exposed via
//! [`HrpdForwardScrambler::with_initial_state`] but the per-packet helper
//! here covers the forward path only.

/// Forward-traffic scrambler state.
#[derive(Debug, Clone)]
pub struct HrpdForwardScrambler {
    /// 17-bit LFSR state held in the low 17 bits of a `u32`.
    state: u32,
}

impl HrpdForwardScrambler {
    /// LFSR width in bits, per h(D) = D^17 + D^14 + 1.
    pub const WIDTH: u32 = 17;
    /// Maximal-length sequence period for a primitive degree-17 polynomial.
    pub const PERIOD: u32 = (1 << 17) - 1;

    /// Construct a scrambler from an already-assembled 17-bit initial state.
    ///
    /// Bit 16 of `state` is the output tap (the leftmost bit in the spec
    /// figure); bit 0 is `d0`. Bits above 16 are ignored.
    pub fn with_initial_state(state: u32) -> Self {
        Self {
            state: state & ((1 << Self::WIDTH) - 1),
        }
    }

    /// Forward-traffic seed per C.S0024-200-C §1.4.1.3.2.3.3.
    ///
    /// `mac_index` is the 6-bit preamble MACIndex (only the low 6 bits are
    /// used). `rate_code` is the 4-bit `d3 d2 d1 d0` value from
    /// Table 1.4.1.3.2.3.3-1.
    pub fn new_forward(mac_index: u8, rate_code: u8) -> Self {
        let r = u32::from(mac_index) & 0x3F;
        let d = u32::from(rate_code) & 0x0F;
        // Layout (MSB..LSB, bit 16..bit 0):
        //   1 1 1 1 1 1 1  r5 r4 r3 r2 r1 r0  d3 d2 d1 d0
        let leading_ones = 0x7F << 10; // seven 1s in bits 16..10
        let state = leading_ones | (r << 4) | d;
        Self::with_initial_state(state)
    }

    /// Subtype 2 Physical Layer forward-traffic seed per C.S0024-A
    /// §13.3.1.3.2.3.3: `[1 1 1 b2 b1 b0 r̄6 r5 ... r0 d3 ... d0]`.
    ///
    /// r6 is COMPLEMENTED. The overbar over r6 exists only in the PDF
    /// (Figure 13.3.1.3.2.3.3-1 and the body text on pp. 13-91…13-93);
    /// pdftotext silently drops it, and that misreading has already caused
    /// one reverted on-air regression. Do not "fix" this back to plain r6.
    /// The complement makes the subtype-2 seed bit-identical to the Rev 0
    /// seed for MACIndex < 64 canonical (b=111) formats, which is how
    /// subtype-0 and subtype-2 ATs share chip-compatible legacy formats.
    pub fn new_forward_subtype2(mac_index: u8, b_code: u8, rate_code: u8) -> Self {
        let b = u32::from(b_code) & 0x07;
        let r = u32::from(mac_index) & 0x7F;
        let d = u32::from(rate_code) & 0x0F;
        let r6_complement = (!(r >> 6)) & 1;
        let state = (0b111u32 << 14) | (b << 11) | (r6_complement << 10) | ((r & 0x3F) << 4) | d;
        Self::with_initial_state(state)
    }

    /// Subtype 3 and later Physical Layer forward-traffic seed per
    /// C.S0024-200-C §3.4.1.3.2.3.3 and later: `[1 r7 d4 b2 b1 b0
    /// r6 ... r0 d3 ... d0]`.
    pub fn new_forward_subtype3_plus(mac_index: u8, b_code: u8, rate_code: u8) -> Self {
        let r = u32::from(mac_index);
        let d = u32::from(rate_code);
        let state = (1u32 << 16)
            | (((r >> 7) & 1) << 15)
            | (((d >> 4) & 1) << 14)
            | ((u32::from(b_code) & 0x07) << 11)
            | ((r & 0x7F) << 4)
            | (d & 0x0F);
        Self::with_initial_state(state)
    }

    /// Generic `(seed)` constructor: the low 17 bits of `seed` are loaded
    /// directly. Provided for callers that have already assembled the
    /// initial state from the slot/MAC inputs.
    pub fn new(seed: u32) -> Self {
        Self::with_initial_state(seed)
    }

    /// Produce the next scrambling-sequence bit.
    ///
    /// The current state's bit 16 is emitted as the output (the loaded
    /// initial state yields the first bit, before any shift). Feedback is
    /// then computed as `bit16 XOR bit13` and clocked into bit 0.
    pub fn next_bit(&mut self) -> bool {
        let out = ((self.state >> (Self::WIDTH - 1)) & 1) != 0;
        let feedback = ((self.state >> (Self::WIDTH - 1)) ^ (self.state >> 13)) & 1;
        self.state = ((self.state << 1) & ((1 << Self::WIDTH) - 1)) | feedback;
        out
    }

    /// Produce the next eight scrambling-sequence bits packed MSB-first.
    ///
    /// Bit 7 of the returned byte is the first bit produced by
    /// [`next_bit`]; this matches a typical big-endian packed-bit buffer.
    pub fn next_byte(&mut self) -> u8 {
        let mut b = 0u8;
        for i in (0..8).rev() {
            if self.next_bit() {
                b |= 1 << i;
            }
        }
        b
    }

    /// XOR the scrambling sequence onto a packed-bit buffer, MSB-first
    /// within each byte. Applying twice with the same seed restores the
    /// original buffer.
    pub fn apply(&mut self, bits: &mut [u8]) {
        for byte in bits.iter_mut() {
            *byte ^= self.next_byte();
        }
    }

    /// XOR the scrambling sequence onto an unpacked bit buffer.
    ///
    /// `bits` must contain one bit per byte (`0` or `1`). This is the form
    /// used by the HRPD channel-interleaver and QPSK mapper in this crate.
    pub fn apply_bits(&mut self, bits: &mut [u8]) {
        for bit in bits {
            *bit = (*bit & 1) ^ (self.next_bit() as u8);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The 17-bit state immediately after construction must match the
    /// spec figure exactly. We rebuild the bit pattern from the layout
    /// and compare against the implementation's internal state.
    #[test]
    fn forward_seed_layout_matches_spec_figure() {
        // MACIndex = 0b101010 (r5..r0), rate code = 0b1011 (d3..d0).
        let mac = 0b10_1010u8;
        let rate = 0b1011u8;
        let s = HrpdForwardScrambler::new_forward(mac, rate);

        // Manual reconstruction: 1 1 1 1 1 1 1 1 0 1 0 1 0 1 0 1 1
        // (MSB bit 16 .. LSB bit 0)
        let expected: u32 = 0b1_1111_1110_1010_1011;
        assert_eq!(s.state, expected);
    }

    /// The "initial state shall generate the first scrambling bit"
    /// requirement: the first call to next_bit returns the leftmost loaded bit.
    #[test]
    fn first_bit_is_leftmost_bit_of_initial_state() {
        let mut s = HrpdForwardScrambler::new_forward(0, 0b0000);
        assert!(s.next_bit());

        let mut s = HrpdForwardScrambler::with_initial_state(0);
        assert!(!s.next_bit());
    }

    /// Determinism: same seed -> same sequence.
    #[test]
    fn determinism_same_seed_same_sequence() {
        let mut a = HrpdForwardScrambler::new_forward(0x2A, 0b0110);
        let mut b = HrpdForwardScrambler::new_forward(0x2A, 0b0110);
        for _ in 0..10_000 {
            assert_eq!(a.next_bit(), b.next_bit());
        }
    }

    /// Two different seeds must produce different sequences (cheap
    /// sanity check, not a statistical test).
    #[test]
    fn distinct_seeds_diverge() {
        let mut a = HrpdForwardScrambler::new_forward(0x00, 0b0001);
        let mut b = HrpdForwardScrambler::new_forward(0x01, 0b0001);
        let mut diff = 0;
        for _ in 0..256 {
            if a.next_bit() != b.next_bit() {
                diff += 1;
            }
        }
        assert!(diff > 0, "distinct seeds produced identical 256-bit output");
    }

    /// A non-zero initial state of a primitive degree-17 LFSR has
    /// period 2^17 - 1 = 131071. Verify the cycle length and that the
    /// state never visits zero.
    #[test]
    fn maximal_length_period() {
        let mut s = HrpdForwardScrambler::new_forward(0, 0b0001);
        let initial = s.state;
        let mut period = 0u32;
        for _ in 0..HrpdForwardScrambler::PERIOD {
            assert_ne!(s.state, 0, "LFSR collapsed to all-zero state");
            s.next_bit();
            period += 1;
            if s.state == initial {
                break;
            }
        }
        assert_eq!(period, HrpdForwardScrambler::PERIOD);
    }

    /// XOR-apply is involutive: scrambling twice with the same seed
    /// reproduces the input.
    #[test]
    fn apply_is_involutive() {
        let original: Vec<u8> = (0u8..=200).collect();
        let mut buf = original.clone();

        let mut s1 = HrpdForwardScrambler::new_forward(0x15, 0b1100);
        s1.apply(&mut buf);
        assert_ne!(buf, original, "scrambler produced identity output");

        let mut s2 = HrpdForwardScrambler::new_forward(0x15, 0b1100);
        s2.apply(&mut buf);
        assert_eq!(buf, original);
    }

    #[test]
    fn apply_bits_is_involutive_and_keeps_bits_unpacked() {
        let original: Vec<u8> = (0..257).map(|i| (i & 1) as u8).collect();
        let mut buf = original.clone();

        let mut s1 = HrpdForwardScrambler::new_forward(0x15, 0b1100);
        s1.apply_bits(&mut buf);
        assert_ne!(buf, original);
        assert!(buf.iter().all(|&b| b <= 1));

        let mut s2 = HrpdForwardScrambler::new_forward(0x15, 0b1100);
        s2.apply_bits(&mut buf);
        assert_eq!(buf, original);
    }

    /// next_byte must equal eight successive next_bit calls packed
    /// MSB-first.
    #[test]
    fn next_byte_matches_bitwise_pack() {
        let mut by_byte = HrpdForwardScrambler::new_forward(0x2A, 0b0101);
        let mut by_bit = HrpdForwardScrambler::new_forward(0x2A, 0b0101);

        for _ in 0..32 {
            let byte = by_byte.next_byte();
            let mut packed = 0u8;
            for i in (0..8).rev() {
                if by_bit.next_bit() {
                    packed |= 1 << i;
                }
            }
            assert_eq!(byte, packed);
        }
    }

    /// Subtype-2 seed vectors from C.S0024-A Table 13.3.1.3.2.3.3-1: b2b1b0
    /// is the payload-size code and d3d2d1d0 the nominal-rate code. For the
    /// canonical single-user formats the payload code is 0b111 and the rate
    /// code equals the DRC index, which is what the scheduler passes. The
    /// r-field's leading bit is r̄6 (complemented, per the PDF figure), so
    /// MACIndex < 64 seeds carry a 1 there and MACIndex >= 64 a 0.
    #[test]
    fn subtype2_seed_matches_canonical_format_table() {
        // ((mac_index, b, d), expected 17-bit state)
        let cases: [((u8, u8, u8), u32); 4] = [
            // (4096,1,64) canonical for DRC 0xc, MACIndex 6:
            // [111 111 1000110 1100] — r̄6 = 1 because r6 = 0.
            ((6, 0b111, 0b1100), 0b111_111_1000110_1100),
            // (5120,1,64) canonical for DRC 0xe, MACIndex 127: r̄6 = 0.
            ((127, 0b111, 0b1110), 0b111_111_0111111_1110),
            // (512,1,64) short packet at 307.2 kbps: payload code 010,
            // rate code 0110 — d is NOT the serving DRC for short packets.
            ((6, 0b010, 0b0110), 0b111_010_1000110_0110),
            // (1024,2,64): the table's irregular payload code 011.
            ((6, 0b011, 0b0111), 0b111_011_1000110_0111),
        ];
        for ((mac, b, d), expected) in cases {
            let s = HrpdForwardScrambler::new_forward_subtype2(mac, b, d);
            assert_eq!(s.state, expected, "mac={mac} b={b:03b} d={d:04b}");
        }
    }

    /// new() and with_initial_state() are aliases of the same load path.
    #[test]
    fn new_and_with_initial_state_agree() {
        let raw = 0b1_1111_1100_1100_1010u32;
        let mut a = HrpdForwardScrambler::new(raw);
        let mut b = HrpdForwardScrambler::with_initial_state(raw);
        for _ in 0..128 {
            assert_eq!(a.next_bit(), b.next_bit());
        }
    }
}
