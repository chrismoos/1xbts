//! HRPD forward-traffic turbo encoder (C.S0024-0 v4.0 §9.3.1.3.2.3.2 and
//! §9.3.1.3.2.3.2.1, with the internal turbo interleaver in §9.3.1.3.2.3.2.2).
//!
//! The encoder is a parallel-concatenated rate-1/3 (mother) turbo code built
//! from two identical recursive systematic convolutional (RSC) constituent
//! encoders connected through a packet-size dependent interleaver. Each
//! constituent encoder has transfer function
//!
//! ```text
//!     G(D) = [ 1   n0(D)/d(D)   n1(D)/d(D) ]
//!     d(D)  = 1 + D^2 + D^3
//!     n0(D) = 1 + D + D^3
//!     n1(D) = 1 + D + D^2 + D^3
//! ```
//!
//! (Per §9.3.1.3.2.3.2.1 / Figure 9.3.1.3.2.3.2.1-1.) The forward link encoder
//! discards the 6-bit packet TAIL field before encoding, so the turbo encoder
//! sees `N_turbo = payload_bits - 6` input bits and produces
//! `(N_turbo + 6) / R` output symbols, i.e. `payload_bits / R` symbols where
//! `R` is the post-puncturing effective rate.
//!
//! ## Design choice: separate encode + rate_match
//!
//! [`HrpdTurboEncoder::encode_raw`] produces the *raw* turbo encoder symbol
//! stream — every output the encoder structure can emit, in the order
//! `X, Y0, Y1, X', Y'0, Y'1` per data bit period — *without* applying the
//! Table 9.3.1.3.2.3.2.1-1 / -2 puncturing. For tail bit periods only the
//! three outputs of the constituent that is actually being clocked are
//! present (CE1 outputs `X, Y0, Y1` for the first 3 tail periods; CE2 outputs
//! `X', Y'0, Y'1` for the last 3). The raw stream length is therefore
//! `6 * N_turbo + 18` symbols regardless of effective rate.
//!
//! [`HrpdTurboEncoder::rate_match`] applies the spec puncturing /
//! repetition pattern to a raw stream to produce the effective-rate
//! (1/2, 1/3, 1/4, or 1/5) symbol stream of length `payload_bits / R`.
//!
//! [`HrpdTurboEncoder::encode`] is a convenience wrapper that composes the
//! two.

/// Rev 0 HRPD turbo encoder block sizes (Table 9.3.1.3.2.3.2.2-1).
///
/// `payload_bits` includes the 6-bit packet TAIL field that the encoder
/// discards before turbo encoding. `n_turbo = payload_bits - 6` is the
/// number of bits actually clocked through the constituent encoders.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HrpdTurboBlock {
    pub payload_bits: u32,
    pub n_turbo: u32,
    /// Turbo interleaver parameter `n` (smallest integer with
    /// `n_turbo <= 2^(n+5)`).
    pub n: u8,
}

impl HrpdTurboBlock {
    /// HRPD packet sizes from the Rev 0 and Subtype 2 turbo interleaver
    /// parameter tables.
    pub const ALL: &'static [HrpdTurboBlock] = &[
        HrpdTurboBlock {
            payload_bits: 128,
            n_turbo: 122,
            n: 2,
        },
        HrpdTurboBlock {
            payload_bits: 256,
            n_turbo: 250,
            n: 3,
        },
        HrpdTurboBlock {
            payload_bits: 512,
            n_turbo: 506,
            n: 4,
        },
        HrpdTurboBlock {
            payload_bits: 768,
            n_turbo: 762,
            n: 5,
        },
        HrpdTurboBlock {
            payload_bits: 1024,
            n_turbo: 1018,
            n: 5,
        },
        HrpdTurboBlock {
            payload_bits: 1536,
            n_turbo: 1530,
            n: 6,
        },
        HrpdTurboBlock {
            payload_bits: 2048,
            n_turbo: 2042,
            n: 6,
        },
        HrpdTurboBlock {
            payload_bits: 3072,
            n_turbo: 3066,
            n: 7,
        },
        HrpdTurboBlock {
            payload_bits: 4096,
            n_turbo: 4090,
            n: 7,
        },
        HrpdTurboBlock {
            payload_bits: 5120,
            n_turbo: 5114,
            n: 8,
        },
        HrpdTurboBlock {
            payload_bits: 6144,
            n_turbo: 6138,
            n: 8,
        },
        HrpdTurboBlock {
            payload_bits: 8192,
            n_turbo: 8186,
            n: 8,
        },
        HrpdTurboBlock {
            payload_bits: 12288,
            n_turbo: 12282,
            n: 9,
        },
    ];

    /// Look up the block descriptor for a supported HRPD packet size.
    pub fn for_packet_size(payload_bits: u32) -> Option<HrpdTurboBlock> {
        Self::ALL
            .iter()
            .copied()
            .find(|b| b.payload_bits == payload_bits)
    }
}

/// Turbo interleaver lookup table, indexed `[counter_low5][n - 2]`.
///
/// Columns are `n=2..9` per C.S0024-A Table 13.2.1.3.4.2.3-2 (identical to
/// the Rev 0 table for the shared `n=2..8` columns). The `n=9` column exists
/// only for the 12,288-bit Subtype 2 reverse payload.
const TURBO_INTERLEAVER_LUT: [[u16; 8]; 32] = [
    [3, 1, 5, 27, 3, 15, 3, 13],
    [3, 1, 15, 3, 27, 127, 1, 335],
    [3, 3, 5, 1, 15, 89, 5, 87],
    [1, 5, 15, 15, 13, 1, 83, 15],
    [3, 1, 1, 13, 29, 31, 19, 15],
    [1, 5, 9, 17, 5, 15, 179, 1],
    [3, 1, 9, 23, 1, 61, 19, 333],
    [1, 5, 15, 13, 31, 47, 99, 11],
    [1, 3, 13, 9, 3, 127, 23, 13],
    [1, 5, 15, 3, 9, 17, 1, 1],
    [3, 3, 7, 15, 15, 119, 3, 121],
    [1, 5, 11, 3, 31, 15, 13, 155],
    [1, 3, 15, 13, 17, 57, 13, 1],
    [1, 5, 3, 1, 5, 123, 3, 175],
    [1, 5, 15, 13, 39, 95, 17, 421],
    [3, 1, 5, 29, 1, 5, 1, 5],
    [3, 3, 13, 21, 19, 85, 63, 509],
    [1, 5, 15, 19, 27, 17, 131, 215],
    [3, 3, 9, 1, 15, 55, 17, 47],
    [3, 5, 3, 3, 13, 57, 131, 425],
    [3, 3, 1, 29, 45, 15, 211, 295],
    [1, 5, 3, 17, 5, 41, 173, 229],
    [3, 5, 15, 25, 33, 93, 231, 427],
    [1, 5, 1, 29, 15, 87, 171, 83],
    [3, 1, 13, 9, 13, 63, 23, 409],
    [1, 5, 1, 13, 9, 15, 147, 387],
    [3, 1, 9, 23, 15, 13, 243, 193],
    [1, 5, 15, 13, 31, 15, 213, 57],
    [3, 3, 11, 13, 17, 81, 189, 501],
    [1, 5, 3, 1, 5, 57, 51, 313],
    [1, 5, 15, 13, 15, 31, 15, 489],
    [3, 3, 5, 13, 33, 69, 67, 391],
];

/// HRPD forward-traffic turbo encoder for one Rev 0 packet size.
#[derive(Debug, Clone)]
pub struct HrpdTurboEncoder {
    block: HrpdTurboBlock,
    /// Interleaver permutation π of length `n_turbo`: bit position
    /// `π[i]` of the original input goes into position `i` of CE2's input.
    perm: Vec<u32>,
}

impl HrpdTurboEncoder {
    /// Construct an encoder for a supported HRPD physical packet size.
    pub fn new(payload_bits: u32) -> Option<Self> {
        let block = HrpdTurboBlock::for_packet_size(payload_bits)?;
        let perm = build_turbo_interleaver(block.n_turbo, block.n);
        Some(Self { block, perm })
    }

    /// Packet-size descriptor used to construct this encoder.
    pub fn block(&self) -> HrpdTurboBlock {
        self.block
    }

    /// Turbo interleaver permutation (read addresses), length `n_turbo`.
    pub fn interleaver(&self) -> &[u32] {
        &self.perm
    }

    /// Encode and rate-match in one call.
    ///
    /// `payload` must be exactly `payload_bits` long with each byte holding
    /// one bit (0 or 1). The last 6 bits (the packet TAIL) are discarded by
    /// the encoder per §9.3.1.3.2.3.2.
    ///
    /// `effective_num` / `effective_den` must be `(1, 2)`, `(1, 3)`,
    /// `(1, 4)`, or `(1, 5)`.
    pub fn encode(&self, payload: &[u8], effective_num: u8, effective_den: u8) -> Vec<u8> {
        let raw = self.encode_raw(payload);
        self.rate_match(&raw, effective_num, effective_den)
    }

    /// Run the turbo encoder structure and return the raw (un-punctured)
    /// symbol stream of length `6 * n_turbo + 18`.
    ///
    /// Layout, in order:
    /// - `n_turbo` data bit periods, each emitting `[X, Y0, Y1, X', Y'0, Y'1]`
    ///   (6 symbols).
    /// - 3 CE1 tail bit periods, each emitting `[X, Y0, Y1]` (3 symbols).
    /// - 3 CE2 tail bit periods, each emitting `[X', Y'0, Y'1]` (3 symbols).
    ///
    /// Per §9.3.1.3.2.3.2.1, during the CE1 tail periods CE2 is not clocked,
    /// and vice versa, so those positions are simply absent from the raw
    /// stream (the rate-match step handles the spec puncturing /
    /// repetition).
    pub fn encode_raw(&self, payload: &[u8]) -> Vec<u8> {
        let n_turbo = self.block.n_turbo as usize;
        assert_eq!(
            payload.len(),
            self.block.payload_bits as usize,
            "payload length must equal packet size in bits",
        );
        for &b in payload {
            debug_assert!(b <= 1, "payload bits must be 0 or 1");
        }
        // The encoder discards the 6-bit TAIL field (§9.3.1.3.2.3.2). The
        // spec does not say *which* 6 bits, but Table 9.3.1.3.2.3.2-1 makes
        // it the last 6 of the physical-layer packet — the standard layout.
        let info = &payload[..n_turbo];

        // CE2 input is the bit-permuted info sequence.
        let mut info_pi = Vec::with_capacity(n_turbo);
        for i in 0..n_turbo {
            info_pi.push(info[self.perm[i] as usize]);
        }

        let mut out = Vec::with_capacity(6 * n_turbo + 18);

        // Data phase: clock both constituents in parallel.
        let mut ce1 = ConstituentEncoder::new();
        let mut ce2 = ConstituentEncoder::new();
        for i in 0..n_turbo {
            let (x, y0, y1) = ce1.step(info[i]);
            let (xp, y0p, y1p) = ce2.step(info_pi[i]);
            out.extend_from_slice(&[x, y0, y1, xp, y0p, y1p]);
        }

        // CE1 tail: clock CE1 three times with feedback fed back as input so
        // the register drains to zero. CE2 is not clocked. The "X" output for
        // each tail period is the synthesized tail bit (= feedback signal).
        for _ in 0..3 {
            let (x, y0, y1) = ce1.step_tail();
            out.extend_from_slice(&[x, y0, y1]);
        }

        // CE2 tail: same, on CE2. CE1 is not clocked.
        for _ in 0..3 {
            let (xp, y0p, y1p) = ce2.step_tail();
            out.extend_from_slice(&[xp, y0p, y1p]);
        }

        out
    }

    /// Apply the Table 9.3.1.3.2.3.2.1-1 (data) and 9.3.1.3.2.3.2.1-2 (tail)
    /// puncturing / repetition patterns to a raw stream from
    /// [`Self::encode_raw`].
    ///
    /// Per §9.3.1.3.2.3.2.1 and §9.2.1.3.4.2.2 the puncturing tables give:
    ///
    /// - Data, rate 1/2: alternate in pairs of data bit periods:
    ///   `[X,Y0]`, then `[X,Y'0]`.
    /// - Data, rate 1/3: keep X, Y0, Y'0  (3 out of 6 per data bit period).
    /// - Data, rate 1/4: alternate the Table 9.2.1.3.4.2.2-1 pattern in
    ///   pairs of data bit periods: `[X,Y0,Y1,Y'1]`, then
    ///   `[X,Y0,Y'0,Y'1]`.
    /// - Data, rate 1/5: keep X, Y0, Y1, Y'0, Y'1  (5 of 6; X' is dropped).
    /// - Tail, rate 1/2: per CE1 tail period emit `X Y0`; per CE2 tail
    ///   period emit `X' Y'0`. 12 tail symbols total.
    /// - Tail, rate 1/3: per CE1 tail period emit `X X Y0` (3 symbols, X
    ///   repeated); per CE2 tail period emit `X' X' Y'0`. 18 tail symbols
    ///   total.
    /// - Tail, rate 1/4: per CE1 tail period emit `X X Y0 Y1`; per CE2 tail
    ///   period emit `X' X' Y'0 Y'1`. 24 tail symbols total.
    /// - Tail, rate 1/5: per CE1 tail period emit `X X Y0 Y1 Y1` (X and Y1
    ///   repeated); per CE2 tail period emit `X' X' Y'0 Y'1 Y'1`. 30 tail
    ///   symbols total.
    ///
    /// The resulting length is `payload_bits * effective_den / effective_num`
    /// (i.e. `(n_turbo + 6) / R`) which matches the "Turbo Encoder Output
    /// Symbols" column of Table 9.3.1.3.2.3.2-1.
    pub fn rate_match(&self, raw: &[u8], effective_num: u8, effective_den: u8) -> Vec<u8> {
        assert_eq!(effective_num, 1, "only rate 1/N supported");
        let n_turbo = self.block.n_turbo as usize;
        let expected_raw_len = 6 * n_turbo + 18;
        assert_eq!(
            raw.len(),
            expected_raw_len,
            "raw stream must have length 6*n_turbo+18",
        );
        // Split the raw stream back into its sections.
        let (data_section, tail_section) = raw.split_at(6 * n_turbo);
        let (ce1_tail, ce2_tail) = tail_section.split_at(9);

        match effective_den {
            2 => {
                assert_eq!(
                    n_turbo % 2,
                    0,
                    "rate-1/2 data puncturing is defined over pairs of bit periods",
                );
                let mut out = Vec::with_capacity((n_turbo + 6) * 2);
                // Reverse-link rate 1/2, C.S0024-200-C Table
                // 1.3.1.3.4.2.2-1: read top-to-bottom, then left-to-right.
                // First data bit period keeps X,Y0; second keeps X,Y'0.
                let mut chunks = data_section.chunks_exact(6);
                while let Some(first) = chunks.next() {
                    let second = chunks
                        .next()
                        .expect("n_turbo is even, data chunks must be paired");
                    out.extend_from_slice(&[first[0], first[1]]);
                    out.extend_from_slice(&[second[0], second[4]]);
                }
                // Tail, Table 1.3.1.3.4.2.2-2: CE1 emits X,Y0 for each of
                // the first three tail bit periods; CE2 emits X',Y'0 for
                // each of the last three.
                for chunk in ce1_tail.chunks_exact(3) {
                    out.extend_from_slice(&[chunk[0], chunk[1]]);
                }
                for chunk in ce2_tail.chunks_exact(3) {
                    out.extend_from_slice(&[chunk[0], chunk[1]]);
                }
                out
            }
            3 => {
                let mut out = Vec::with_capacity((n_turbo + 6) * 3);
                // Data: keep X (offset 0), Y0 (1), Y'0 (4) from each 6-tuple.
                for chunk in data_section.chunks_exact(6) {
                    out.extend_from_slice(&[chunk[0], chunk[1], chunk[4]]);
                }
                // CE1 tail: per period [X, Y0, Y1] → emit [X, X, Y0].
                for chunk in ce1_tail.chunks_exact(3) {
                    out.extend_from_slice(&[chunk[0], chunk[0], chunk[1]]);
                }
                // CE2 tail: per period [X', Y'0, Y'1] → emit [X', X', Y'0].
                for chunk in ce2_tail.chunks_exact(3) {
                    out.extend_from_slice(&[chunk[0], chunk[0], chunk[1]]);
                }
                out
            }
            4 => {
                assert_eq!(
                    n_turbo % 2,
                    0,
                    "rate-1/4 data puncturing is defined over pairs of bit periods",
                );
                let mut out = Vec::with_capacity((n_turbo + 6) * 4);
                // Data, Table 9.2.1.3.4.2.2-1, rate 1/4:
                // read columns top-to-bottom, left-to-right. For the first
                // data bit period in each pair keep X,Y0,Y1,Y'1; for the
                // second keep X,Y0,Y'0,Y'1.
                let mut chunks = data_section.chunks_exact(6);
                while let Some(first) = chunks.next() {
                    let second = chunks
                        .next()
                        .expect("n_turbo is even, data chunks must be paired");
                    out.extend_from_slice(&[first[0], first[1], first[2], first[5]]);
                    out.extend_from_slice(&[second[0], second[1], second[4], second[5]]);
                }
                // CE1 tail: per period [X, Y0, Y1] -> emit [X, X, Y0, Y1].
                for chunk in ce1_tail.chunks_exact(3) {
                    out.extend_from_slice(&[chunk[0], chunk[0], chunk[1], chunk[2]]);
                }
                // CE2 tail: per period [X', Y'0, Y'1] -> emit
                // [X', X', Y'0, Y'1].
                for chunk in ce2_tail.chunks_exact(3) {
                    out.extend_from_slice(&[chunk[0], chunk[0], chunk[1], chunk[2]]);
                }
                out
            }
            5 => {
                let mut out = Vec::with_capacity((n_turbo + 6) * 5);
                // Data: keep X, Y0, Y1, Y'0, Y'1 (drop X' at offset 3).
                for chunk in data_section.chunks_exact(6) {
                    out.extend_from_slice(&[chunk[0], chunk[1], chunk[2], chunk[4], chunk[5]]);
                }
                // CE1 tail: per period [X, Y0, Y1] → emit [X, X, Y0, Y1, Y1].
                for chunk in ce1_tail.chunks_exact(3) {
                    out.extend_from_slice(&[chunk[0], chunk[0], chunk[1], chunk[2], chunk[2]]);
                }
                // CE2 tail: per period [X', Y'0, Y'1] → emit
                // [X', X', Y'0, Y'1, Y'1].
                for chunk in ce2_tail.chunks_exact(3) {
                    out.extend_from_slice(&[chunk[0], chunk[0], chunk[1], chunk[2], chunk[2]]);
                }
                out
            }
            _ => panic!("HRPD Rev 0 turbo only supports effective rate 1/2, 1/3, 1/4, or 1/5"),
        }
    }
}

/// One recursive systematic constituent encoder.
///
/// Memory layout: `m[0]` holds the value shifted in one clock ago (D^1
/// delay), `m[1]` holds D^2, `m[2]` holds D^3. Initial state is zero per
/// §9.3.1.3.2.3.2.1.
struct ConstituentEncoder {
    m: [u8; 3],
}

impl ConstituentEncoder {
    fn new() -> Self {
        Self { m: [0; 3] }
    }

    /// Clock one data bit. Returns `(X, Y0, Y1)`.
    ///
    /// `d(D) = 1 + D^2 + D^3` → feedback `v = u XOR m[1] XOR m[2]`.
    /// `n0(D) = 1 + D + D^3` → `Y0 = v XOR m[0] XOR m[2]`.
    /// `n1(D) = 1 + D + D^2 + D^3` → `Y1 = v XOR m[0] XOR m[1] XOR m[2]`.
    fn step(&mut self, u: u8) -> (u8, u8, u8) {
        let v = u ^ self.m[1] ^ self.m[2];
        let y0 = v ^ self.m[0] ^ self.m[2];
        let y1 = v ^ self.m[0] ^ self.m[1] ^ self.m[2];
        // Shift register: m[2] ← m[1], m[1] ← m[0], m[0] ← v.
        self.m[2] = self.m[1];
        self.m[1] = self.m[0];
        self.m[0] = v;
        (u, y0, y1)
    }

    /// Clock one tail bit with the switch "down" (§9.3.1.3.2.3.2.1).
    ///
    /// In that mode the encoder's input is replaced by the feedback signal
    /// `m[1] XOR m[2]`, which forces `v = 0` and drains the registers. The
    /// systematic output is the synthesized tail bit itself.
    fn step_tail(&mut self) -> (u8, u8, u8) {
        let u_tail = self.m[1] ^ self.m[2];
        let v = u_tail ^ self.m[1] ^ self.m[2]; // = 0 by construction
        let y0 = v ^ self.m[0] ^ self.m[2];
        let y1 = v ^ self.m[0] ^ self.m[1] ^ self.m[2];
        self.m[2] = self.m[1];
        self.m[1] = self.m[0];
        self.m[0] = v;
        (u_tail, y0, y1)
    }
}

/// Build the turbo interleaver permutation per the HRPD turbo interleaver
/// procedure.
///
/// Returns a `Vec` of length `n_turbo` where entry `i` is the original
/// input-bit address that should be placed at output position `i` (i.e. the
/// read addresses described by the spec).
fn build_turbo_interleaver(n_turbo: u32, n: u8) -> Vec<u32> {
    assert!((2..=9).contains(&n), "HRPD turbo uses n in {{2..9}}");
    let lut_col = (n - 2) as usize;
    let total = 1u32 << (n + 5); // 2^(n+5) counter values to scan
    let n_mask = (1u32 << n) - 1;
    let mut out = Vec::with_capacity(n_turbo as usize);

    for counter in 0..total {
        // n MSBs of the (n+5)-bit counter, plus one, n LSBs of result.
        let msbs = (counter >> 5) & n_mask;
        let step3 = msbs.wrapping_add(1) & n_mask;
        // 5 LSBs index the LUT.
        let lut_idx = (counter & 0x1f) as usize;
        let lut_val = TURBO_INTERLEAVER_LUT[lut_idx][lut_col] as u32;
        let product = step3.wrapping_mul(lut_val) & n_mask;
        // Bit-reverse the 5 LSBs of the counter.
        let rev5 = bit_reverse_5(counter & 0x1f);
        // Tentative output address: 5 MSBs = rev5, n LSBs = product.
        let addr = (rev5 << n) | product;
        if addr < n_turbo {
            out.push(addr);
        }
    }

    // Sanity: we should have collected exactly n_turbo addresses (the spec
    // procedure scans all 2^(n+5) counter values and accepts those whose
    // tentative address is < n_turbo, and every value 0..n_turbo appears
    // exactly once because the map is a permutation of 0..2^(n+5)).
    debug_assert_eq!(out.len(), n_turbo as usize);
    out
}

fn bit_reverse_5(x: u32) -> u32 {
    let mut r = 0u32;
    for i in 0..5 {
        if (x >> i) & 1 == 1 {
            r |= 1 << (4 - i);
        }
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phy::hrpd::rates::FORWARD_RATES;

    fn zero_payload(bits: u32) -> Vec<u8> {
        vec![0u8; bits as usize]
    }

    fn deterministic_payload(bits: u32) -> Vec<u8> {
        // Simple LCG-ish pseudo-random bit pattern, fully reproducible.
        let mut s: u32 = 0x1234_5678;
        (0..bits)
            .map(|_| {
                s = s.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                ((s >> 16) & 1) as u8
            })
            .collect()
    }

    #[test]
    fn all_four_packet_sizes_construct() {
        for &b in HrpdTurboBlock::ALL {
            let enc = HrpdTurboEncoder::new(b.payload_bits).expect("constructible");
            assert_eq!(enc.block(), b);
            assert_eq!(enc.interleaver().len(), b.n_turbo as usize);
        }
        assert!(HrpdTurboEncoder::new(999).is_none());
    }

    #[test]
    fn interleaver_is_a_permutation() {
        for &b in HrpdTurboBlock::ALL {
            let enc = HrpdTurboEncoder::new(b.payload_bits).unwrap();
            let perm = enc.interleaver();
            let mut seen = vec![false; b.n_turbo as usize];
            for &a in perm {
                assert!(a < b.n_turbo, "address {} out of range", a);
                assert!(!seen[a as usize], "duplicate address {}", a);
                seen[a as usize] = true;
            }
            assert!(seen.iter().all(|s| *s), "permutation must cover 0..n_turbo");
        }
    }

    #[test]
    fn raw_output_length_matches_spec_structure() {
        for &b in HrpdTurboBlock::ALL {
            let enc = HrpdTurboEncoder::new(b.payload_bits).unwrap();
            let raw = enc.encode_raw(&zero_payload(b.payload_bits));
            assert_eq!(
                raw.len(),
                6 * b.n_turbo as usize + 18,
                "raw length wrong for payload {}",
                b.payload_bits,
            );
        }
    }

    #[test]
    fn zero_payload_produces_all_zero_output() {
        // Zero input + zero initial state → feedback always 0, all parity
        // outputs zero, tails are 0 too. Useful structural sanity check.
        for &b in HrpdTurboBlock::ALL {
            let enc = HrpdTurboEncoder::new(b.payload_bits).unwrap();
            let raw = enc.encode_raw(&zero_payload(b.payload_bits));
            assert!(raw.iter().all(|&x| x == 0));
            let r13 = enc.rate_match(&raw, 1, 3);
            assert!(r13.iter().all(|&x| x == 0));
            let r14 = enc.rate_match(&raw, 1, 4);
            assert!(r14.iter().all(|&x| x == 0));
            let r15 = enc.rate_match(&raw, 1, 5);
            assert!(r15.iter().all(|&x| x == 0));
        }
    }

    #[test]
    fn rate_matched_lengths_match_table_9_3_1_3_2_dash_1() {
        // Table 9.3.1.3.2.3.2-1 "Turbo Encoder Output Symbols" column.
        let expected = [
            (256u32, 1u8, 4u8, 1024usize),
            (1024u32, 1u8, 5u8, 5120usize),
            (1024, 1, 3, 3072),
            (2048, 1, 3, 6144),
            (3072, 1, 3, 9216),
            (4096, 1, 3, 12288),
        ];
        for (payload, num, den, exp_len) in expected {
            let enc = HrpdTurboEncoder::new(payload).unwrap();
            let raw = enc.encode_raw(&zero_payload(payload));
            let out = enc.rate_match(&raw, num, den);
            assert_eq!(
                out.len(),
                exp_len,
                "payload={} rate=1/{} expected {} got {}",
                payload,
                den,
                exp_len,
                out.len(),
            );
            // Cross-check against the formula `payload_bits / R`.
            assert_eq!(
                out.len(),
                (payload as usize) * (den as usize) / (num as usize),
            );
        }
    }

    #[test]
    fn all_rev0_forward_rates_have_matching_output_length() {
        // Every (packet_size, code rate) pair in the forward-rate table must
        // be producible by this encoder, and the post-rate-match length must
        // equal payload_bits/R.
        for r in FORWARD_RATES {
            let enc = HrpdTurboEncoder::new(r.payload_bits).unwrap();
            let raw = enc.encode_raw(&zero_payload(r.payload_bits));
            let out = enc.rate_match(&raw, r.code_rate_num, r.code_rate_den);
            let expected =
                (r.payload_bits as usize) * (r.code_rate_den as usize) / (r.code_rate_num as usize);
            assert_eq!(
                out.len(),
                expected,
                "DRC 0x{:x} payload={} rate=1/{}",
                r.drc_index,
                r.payload_bits,
                r.code_rate_den,
            );
        }
    }

    #[test]
    fn encoder_is_deterministic() {
        let enc = HrpdTurboEncoder::new(1024).unwrap();
        let payload = deterministic_payload(1024);
        let a = enc.encode(&payload, 1, 5);
        let b = enc.encode(&payload, 1, 5);
        assert_eq!(a, b, "encoder must be deterministic");
    }

    #[test]
    fn nontrivial_input_yields_nontrivial_output() {
        let enc = HrpdTurboEncoder::new(1024).unwrap();
        let payload = deterministic_payload(1024);
        let raw = enc.encode_raw(&payload);
        // Not all zeros and not all ones — a sanity check, not a byte-level
        // golden (the spec gives no published vector we could pin to).
        assert!(raw.iter().any(|&x| x == 0));
        assert!(raw.iter().any(|&x| x == 1));
        for (num, den) in [(1u8, 3u8), (1, 4), (1, 5)] {
            let out = enc.rate_match(&raw, num, den);
            assert!(out.iter().any(|&x| x == 0));
            assert!(out.iter().any(|&x| x == 1));
        }
    }

    #[test]
    fn convenience_encode_matches_encode_then_rate_match() {
        let enc = HrpdTurboEncoder::new(2048).unwrap();
        let payload = deterministic_payload(2048);
        for (num, den) in [(1u8, 3u8), (1, 4), (1, 5)] {
            let raw = enc.encode_raw(&payload);
            let split = enc.rate_match(&raw, num, den);
            let combined = enc.encode(&payload, num, den);
            assert_eq!(split, combined);
        }
    }

    #[test]
    fn constituent_encoder_clears_to_zero_after_tail() {
        // After 3 tail "switch-down" clocks the constituent register must be
        // back to (0,0,0) — this is the whole point of trellis termination.
        for &b in HrpdTurboBlock::ALL {
            let enc = HrpdTurboEncoder::new(b.payload_bits).unwrap();
            let payload = deterministic_payload(b.payload_bits);
            // Drive a real CE1 through the same sequence the encoder uses
            // and confirm it terminates to zero.
            let info = &payload[..b.n_turbo as usize];
            let mut ce1 = ConstituentEncoder::new();
            for &u in info {
                ce1.step(u);
            }
            for _ in 0..3 {
                ce1.step_tail();
            }
            assert_eq!(ce1.m, [0, 0, 0], "CE1 did not terminate to zero");
            let _ = enc; // silence unused
        }
    }

    #[test]
    fn n_parameters_match_table_9_3_1_3_2_2_dash_1() {
        // Sanity-check the (n_turbo, n) values against the spec table.
        let expected = [
            (1024u32, 1018u32, 5u8),
            (2048, 2042, 6),
            (3072, 3066, 7),
            (4096, 4090, 7),
        ];
        for (payload, n_turbo, n) in expected {
            let b = HrpdTurboBlock::for_packet_size(payload).unwrap();
            assert_eq!(b.n_turbo, n_turbo);
            assert_eq!(b.n, n);
            assert!(b.n_turbo <= 1u32 << (b.n + 5));
            if b.n >= 1 {
                // n is the *smallest* integer with the property.
                assert!(b.n_turbo > 1u32 << (b.n + 5 - 1) || b.n == 5);
            }
        }
    }
}
