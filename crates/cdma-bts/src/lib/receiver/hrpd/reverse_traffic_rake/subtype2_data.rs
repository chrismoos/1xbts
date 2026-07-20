//! HRPD Subtype 2 reverse Data Channel modulation mappers/demappers and
//! channel (de)interleaver (C.S0024-A v3.0 §13.2.1.3).
//!
//! Pure-DSP building blocks for the subtype-2 reverse Data Channel across all
//! twelve payload sizes (Table 13.2.1.3.4-2/-3):
//!
//! - encoder output → scrambler (§13.2.1.3.5) → channel interleaver
//!   (§13.2.1.3.7: symbol reordering + matrix interleaving) → sequence
//!   repetition / sub-packet symbol selection (§13.2.1.3.11) → B4 / Q4 / Q2 /
//!   Q4Q2 / E4E2 modulation (§13.2.1.3.9, Table 13.2.1.3.3.6-1).
//!
//! Each sub-packet occupies one 4-slot sub-frame (8192 chips). Sub-packet `i`
//! carries interleaver-output symbols `k = (j + i·M) mod N` for `j = 0..M`
//! (§13.2.1.3.11), where `N` is the encoder output block length and `M` the
//! per-sub-frame code symbol count of the modulation format.
//!
//! Conventions:
//! - Soft values are LLR-style: positive ⇒ code bit 0, matching the reverse
//!   demod path and [`crate::phy::hrpd::turbo_decoder::HrpdTurboDecoder`].
//! - QPSK/8-PSK symbols map `mI` to the real part and `mQ` to the imaginary
//!   part. B4 BPSK is placed on the imaginary (Q) component, matching the
//!   Access Channel Data Channel placement (§13.2.1.3.2.2) and the existing
//!   B4 receive path.

use num::complex::Complex32;

use crate::phy::hrpd::scrambler::HrpdForwardScrambler;
use crate::phy::hrpd::turbo::HrpdTurboBlock;

/// Chips per slot on the reverse link.
const CHIPS_PER_SLOT: usize = 2048;
/// A sub-packet spans one 4-slot sub-frame (§13.2.1.3.11 / Table
/// 13.2.1.3.1.1-2 "After 4 Slots" column granularity).
pub const SUBFRAME_SLOTS: usize = 4;
/// Chips carried by one sub-packet.
pub const SUBFRAME_CHIPS: usize = SUBFRAME_SLOTS * CHIPS_PER_SLOT;
/// All modulation formats are defined over 4-chip units (one `W_2^4` period).
const CHIPS_PER_UNIT: usize = W24_COVER.len();
const SUBFRAME_UNITS: usize = SUBFRAME_CHIPS / CHIPS_PER_UNIT;
/// `W_2^4`-covered modulation symbols per sub-frame.
pub const SUBFRAME_W24_SYMBOLS: usize = SUBFRAME_UNITS;
/// `W_1^2`-covered modulation symbols per sub-frame.
pub const SUBFRAME_W12_SYMBOLS: usize = SUBFRAME_CHIPS / W12_COVER.len();
/// A physical layer packet is transmitted in at most four sub-packets.
pub const MAX_SUBPACKETS: usize = 4;

/// Data Channel Walsh cover `W_2^4 = (+ + − −)` (§13.2.1.3.8).
pub const W24_COVER: [f32; 4] = [1.0, 1.0, -1.0, -1.0];
/// Data Channel Walsh cover `W_1^2 = (+ −)` (§13.2.1.3.8).
pub const W12_COVER: [f32; 2] = [1.0, -1.0];

/// `D = 1` in the B4 modulation table (Table 13.2.1.3.9.1-1).
const B4_AMPLITUDE: f32 = 1.0;
/// `D = 1/√2` in the Q4/Q2 modulation tables (Tables 13.2.1.3.9.2-1/.3-1).
const QPSK_AMPLITUDE: f32 = std::f32::consts::FRAC_1_SQRT_2;
/// `C = cos(π/8)` in the E4/E2 modulation tables (Tables 13.2.1.3.9.5-1/-2).
const PSK8_COS: f32 = 0.923_879_5;
/// `S = sin(π/8)` in the E4/E2 modulation tables.
const PSK8_SIN: f32 = 0.382_683_43;
/// Q4Q2/E4E2 scale the `W_2^4` branch by `√(1/3)` (§13.2.1.3.9.4/.5).
const W24_BRANCH_SCALE: f32 = 0.577_350_26;
/// Q4Q2/E4E2 scale the `W_1^2` branch by `√(2/3)` (§13.2.1.3.9.4/.5).
const W12_BRANCH_SCALE: f32 = 0.816_496_6;

/// 8-PSK constellation from Table 13.2.1.3.9.5-1, indexed `b2b1b0` where
/// `b0` is the first code symbol of the triplet (`x(3k)`), as `(mI, mQ)`.
const PSK8_POINTS: [(f32, f32); 8] = [
    (PSK8_COS, PSK8_SIN),
    (PSK8_SIN, PSK8_COS),
    (-PSK8_COS, PSK8_SIN),
    (-PSK8_SIN, PSK8_COS),
    (PSK8_COS, -PSK8_SIN),
    (PSK8_SIN, -PSK8_COS),
    (-PSK8_COS, -PSK8_SIN),
    (-PSK8_SIN, -PSK8_COS),
];
const PSK8_BITS: usize = 3;

/// Eleven leading `1`s of the reverse scrambler initial state
/// (Figure 13.2.1.3.5-1).
const SCRAMBLER_LEADING_ONES: u32 = 0x7ff;

/// Data Channel modulation formats (Table 13.2.1.3.3.6-1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModulationFormat {
    /// BPSK on `W_2^4`.
    B4,
    /// QPSK on `W_2^4`.
    Q4,
    /// QPSK on `W_1^2`.
    Q2,
    /// QPSK on `W_2^4` plus QPSK on `W_1^2`.
    Q4Q2,
    /// 8-PSK on `W_2^4` plus 8-PSK on `W_1^2`.
    E4E2,
}

impl ModulationFormat {
    /// Code symbols consumed per 4-chip unit (§13.2.1.3.9.1–.5).
    pub fn code_symbols_per_unit(self) -> usize {
        match self {
            Self::B4 => 1,
            Self::Q4 => 2,
            Self::Q2 => 4,
            Self::Q4Q2 => 6,
            Self::E4E2 => 9,
        }
    }

    pub fn uses_w24(self) -> bool {
        !matches!(self, Self::Q2)
    }

    pub fn uses_w12(self) -> bool {
        matches!(self, Self::Q2 | Self::Q4Q2 | Self::E4E2)
    }
}

/// Channel interleaver parameters for one payload size
/// (Table 13.2.1.3.7.2-1: `N = R × K × 2^m`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InterleaverParams {
    /// `K` (levels).
    pub levels: usize,
    /// `R` (rows).
    pub rows: usize,
    /// `m`; the U matrix has `2^m` columns, the V matrices `2^(m+1)`.
    pub column_bits: u32,
    /// `D` (end-around-shift divisor).
    pub shift_divisor: usize,
}

/// Per-payload-size reverse Data Channel format descriptor
/// (Tables 13.2.1.3.4-2/-3, 13.2.1.3.7.2-1 and §13.2.1.3.9).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Subtype2DataFormat {
    pub payload_bits: usize,
    pub modulation: ModulationFormat,
    /// Turbo code rate denominator (rate `1/5` or `1/3`).
    pub turbo_code_rate_den: u8,
    pub interleaver: InterleaverParams,
}

macro_rules! fmt_row {
    ($bits:expr, $modulation:ident, $den:expr, $k:expr, $r:expr, $m:expr, $d:expr) => {
        Subtype2DataFormat {
            payload_bits: $bits,
            modulation: ModulationFormat::$modulation,
            turbo_code_rate_den: $den,
            interleaver: InterleaverParams {
                levels: $k,
                rows: $r,
                column_bits: $m,
                shift_divisor: $d,
            },
        }
    };
}

/// All twelve Reverse Traffic Channel payload sizes.
pub const SUBTYPE2_DATA_FORMATS: [Subtype2DataFormat; 12] = [
    fmt_row!(128, B4, 5, 1, 1, 7, 1),
    fmt_row!(256, B4, 5, 1, 1, 8, 1),
    fmt_row!(512, B4, 5, 1, 1, 9, 1),
    fmt_row!(768, B4, 5, 3, 1, 8, 1),
    fmt_row!(1024, B4, 5, 1, 1, 10, 1),
    fmt_row!(1536, Q4, 5, 3, 2, 8, 1),
    fmt_row!(2048, Q4, 5, 1, 2, 10, 1),
    fmt_row!(3072, Q2, 5, 3, 2, 9, 1),
    fmt_row!(4096, Q2, 5, 1, 2, 11, 1),
    fmt_row!(6144, Q4Q2, 5, 3, 2, 10, 1),
    fmt_row!(8192, Q4Q2, 5, 1, 2, 12, 1),
    fmt_row!(12288, E4E2, 3, 1, 3, 12, 1),
];

impl Subtype2DataFormat {
    pub fn for_payload_bits(payload_bits: usize) -> Option<&'static Self> {
        SUBTYPE2_DATA_FORMATS
            .iter()
            .find(|f| f.payload_bits == payload_bits)
    }

    /// Encoder output block length `N` in code symbols
    /// (Tables 13.2.1.3.4-2/-3).
    pub fn encoder_output_symbols(&self) -> usize {
        self.payload_bits * usize::from(self.turbo_code_rate_den)
    }

    /// Code symbols `M` carried by one sub-packet (4-slot sub-frame).
    pub fn subframe_code_symbols(&self) -> usize {
        SUBFRAME_UNITS * self.modulation.code_symbols_per_unit()
    }

    /// `W_2^4`-covered modulation symbols per sub-frame (0 if unused).
    pub fn subframe_w24_symbols(&self) -> usize {
        if self.modulation.uses_w24() {
            SUBFRAME_W24_SYMBOLS
        } else {
            0
        }
    }

    /// `W_1^2`-covered modulation symbols per sub-frame (0 if unused).
    pub fn subframe_w12_symbols(&self) -> usize {
        if self.modulation.uses_w12() {
            SUBFRAME_W12_SYMBOLS
        } else {
            0
        }
    }
}

/// `d3d2d1d0` scrambler seed nibble for a payload size
/// (Table 13.2.1.3.5-1).
pub fn payload_size_code(payload_bits: usize) -> Option<u8> {
    SUBTYPE2_DATA_FORMATS
        .iter()
        .position(|f| f.payload_bits == payload_bits)
        .map(|idx| idx as u8)
}

/// Reverse Data Channel scrambler initial state
/// `[1×11, i1 i0, d3 d2 d1 d0]` (§13.2.1.3.5, Figure 13.2.1.3.5-1).
pub fn scrambler_initial_state(payload_bits: usize, interlace_offset: u8) -> u32 {
    let d_code = payload_size_code(payload_bits).expect("supported subtype-2 payload size");
    (SCRAMBLER_LEADING_ONES << 6) | (u32::from(interlace_offset & 0b11) << 4) | u32::from(d_code)
}

impl Subtype2DataFormat {
    /// Scramble the encoder output in place (§13.2.1.3.5). Applies before
    /// channel interleaving; applying twice restores the input.
    pub fn scramble_encoder_output(&self, bits: &mut [u8], interlace_offset: u8) {
        assert_eq!(bits.len(), self.encoder_output_symbols());
        HrpdForwardScrambler::with_initial_state(scrambler_initial_state(
            self.payload_bits,
            interlace_offset,
        ))
        .apply_bits(bits);
    }
}

/// Flip the sign of encoder-order LLRs wherever the §13.2.1.3.5 scrambling
/// sequence is `1`. Mirrors [`scramble_encoder_output`] in the soft domain.
pub fn descramble_encoder_output_llrs(
    format: &Subtype2DataFormat,
    llrs: &mut [f32],
    interlace_offset: u8,
) {
    assert_eq!(llrs.len(), format.encoder_output_symbols());
    let mut scrambler = HrpdForwardScrambler::with_initial_state(scrambler_initial_state(
        format.payload_bits,
        interlace_offset,
    ));
    for llr in llrs {
        if scrambler.next_bit() {
            *llr = -*llr;
        }
    }
}

fn bit_reverse(value: usize, bits: u32) -> usize {
    if bits == 0 {
        0
    } else {
        ((value as u32).reverse_bits() >> (32 - bits)) as usize
    }
}

/// §13.2.1.3.7.2 matrix interleaver for one cuboidal array: returns, for
/// each output position, the input position it reads from. Write order is
/// level-first (`i = (r·C + c)·K + k`), rows are end-around-shifted by
/// `⌊(c·K + k)/D⌋ mod R`, columns are bit-reverse permuted, and the read
/// order is row-first (`o = (k·C + c)·R + r`).
fn matrix_interleave_source_indices(
    rows: usize,
    levels: usize,
    column_bits: u32,
    shift_divisor: usize,
) -> Vec<usize> {
    let cols = 1usize << column_bits;
    let n = rows * cols * levels;
    let mut out = Vec::with_capacity(n);
    for o in 0..n {
        let r = o % rows;
        let c = (o / rows) % cols;
        let k = o / (rows * cols);
        let c_src = bit_reverse(c, column_bits);
        let shift = ((c_src * levels + k) / shift_divisor) % rows;
        let r_src = (r + rows - shift) % rows;
        out.push((r_src * cols + c_src) * levels + k);
    }
    out
}

impl Subtype2DataFormat {
    /// Channel interleaver as a source-index map: entry `j` is the
    /// encoder-output symbol index that appears at interleaver output `j`.
    ///
    /// §13.2.1.3.7.1 demultiplexes the scrambled encoder output into `U, V0, V1,
    /// V′0, V′1` (rate 1/5) or `U, V0, V′0` (rate 1/3) by round-robin, orders the
    /// partitions `U | V0 V′0 | V1 V′1` (or `U | V0 V′0`), and §13.2.1.3.7.2
    /// matrix-interleaves `U` with `2^m` columns and each `V` pair with `2^(m+1)`
    /// columns.
    pub fn channel_interleaver_source_indices(&self) -> Vec<usize> {
        let n = self.payload_bits;
        let stride = usize::from(self.turbo_code_rate_den);
        let p = self.interleaver;
        let mut out = Vec::with_capacity(self.encoder_output_symbols());

        let u_map =
            matrix_interleave_source_indices(p.rows, p.levels, p.column_bits, p.shift_divisor);
        debug_assert_eq!(u_map.len(), n);
        out.extend(u_map.iter().map(|&pos| pos * stride));

        // Round-robin offsets of each V pair within an encoder-output period.
        let v_pairs: &[(usize, usize)] = match stride {
            5 => &[(1, 3), (2, 4)],
            3 => &[(1, 2)],
            _ => unreachable!("subtype-2 reverse turbo rate is 1/5 or 1/3"),
        };
        let v_map =
            matrix_interleave_source_indices(p.rows, p.levels, p.column_bits + 1, p.shift_divisor);
        debug_assert_eq!(v_map.len(), 2 * n);
        for &(first, second) in v_pairs {
            out.extend(v_map.iter().map(|&pos| {
                if pos < n {
                    pos * stride + first
                } else {
                    (pos - n) * stride + second
                }
            }));
        }
        out
    }
}

impl Subtype2DataFormat {
    /// Apply the channel interleaver to scrambled encoder-output bits.
    pub fn interleave_encoder_output(&self, bits: &[u8]) -> Vec<u8> {
        assert_eq!(bits.len(), self.encoder_output_symbols());
        self.channel_interleaver_source_indices()
            .into_iter()
            .map(|src| bits[src])
            .collect()
    }
}

/// Code symbols carried by sub-packet `subpacket` (§13.2.1.3.11:
/// `k = (j + i·M) mod N`).
pub fn subpacket_code_symbols(
    format: &Subtype2DataFormat,
    interleaved: &[u8],
    subpacket: usize,
) -> Vec<u8> {
    assert!(subpacket < MAX_SUBPACKETS);
    let n = format.encoder_output_symbols();
    assert_eq!(interleaved.len(), n);
    let m = format.subframe_code_symbols();
    (0..m)
        .map(|j| interleaved[(j + subpacket * m) % n])
        .collect()
}

fn bpsk_value(bit: u8) -> f32 {
    if bit & 1 == 0 { 1.0 } else { -1.0 }
}

/// QPSK symbol per Tables 13.2.1.3.9.2-1/.3-1: first code symbol drives
/// `mI`, second drives `mQ`, `0 → +D`, `1 → −D`.
fn qpsk_symbol(i_bit: u8, q_bit: u8) -> Complex32 {
    Complex32::new(
        bpsk_value(i_bit) * QPSK_AMPLITUDE,
        bpsk_value(q_bit) * QPSK_AMPLITUDE,
    )
}

fn psk8_index(triplet: &[u8]) -> usize {
    usize::from(triplet[0] & 1)
        | (usize::from(triplet[1] & 1) << 1)
        | (usize::from(triplet[2] & 1) << 2)
}

/// 8-PSK symbol per Table 13.2.1.3.9.5-1; `triplet[0]` is `x(3k)`.
fn psk8_symbol(triplet: &[u8]) -> Complex32 {
    let (i, q) = PSK8_POINTS[psk8_index(triplet)];
    Complex32::new(i, q)
}

fn add_w24_symbol(chips: &mut [Complex32], symbol: Complex32) {
    for (chip, &cover) in chips.iter_mut().zip(W24_COVER.iter()) {
        *chip += symbol * cover;
    }
}

fn add_w12_symbol(chips: &mut [Complex32], symbol: Complex32) {
    for (chip, &cover) in chips.iter_mut().zip(W12_COVER.iter()) {
        *chip += symbol * cover;
    }
}

impl Subtype2DataFormat {
    /// Map one sub-packet's code symbols to its 8192-chip sequence
    /// (§13.2.1.3.9), combining the `W_2^4` and/or `W_1^2` branches with the
    /// §13.2.1.3.9.4/.5 branch scaling.
    pub fn modulate_subpacket(&self, code_symbols: &[u8]) -> Vec<Complex32> {
        assert_eq!(code_symbols.len(), self.subframe_code_symbols());
        let per_unit = self.modulation.code_symbols_per_unit();
        let mut chips = vec![Complex32::new(0.0, 0.0); SUBFRAME_CHIPS];
        for (unit, group) in code_symbols.chunks_exact(per_unit).enumerate() {
            let base = unit * CHIPS_PER_UNIT;
            let unit_chips = &mut chips[base..base + CHIPS_PER_UNIT];
            match self.modulation {
                ModulationFormat::B4 => {
                    let symbol = Complex32::new(0.0, bpsk_value(group[0]) * B4_AMPLITUDE);
                    add_w24_symbol(unit_chips, symbol);
                }
                ModulationFormat::Q4 => {
                    add_w24_symbol(unit_chips, qpsk_symbol(group[0], group[1]));
                }
                ModulationFormat::Q2 => {
                    add_w12_symbol(&mut unit_chips[..2], qpsk_symbol(group[0], group[1]));
                    add_w12_symbol(&mut unit_chips[2..], qpsk_symbol(group[2], group[3]));
                }
                ModulationFormat::Q4Q2 => {
                    add_w24_symbol(
                        unit_chips,
                        qpsk_symbol(group[0], group[1]) * W24_BRANCH_SCALE,
                    );
                    add_w12_symbol(
                        &mut unit_chips[..2],
                        qpsk_symbol(group[2], group[3]) * W12_BRANCH_SCALE,
                    );
                    add_w12_symbol(
                        &mut unit_chips[2..],
                        qpsk_symbol(group[4], group[5]) * W12_BRANCH_SCALE,
                    );
                }
                ModulationFormat::E4E2 => {
                    add_w24_symbol(unit_chips, psk8_symbol(&group[..3]) * W24_BRANCH_SCALE);
                    add_w12_symbol(
                        &mut unit_chips[..2],
                        psk8_symbol(&group[3..6]) * W12_BRANCH_SCALE,
                    );
                    add_w12_symbol(
                        &mut unit_chips[2..],
                        psk8_symbol(&group[6..9]) * W12_BRANCH_SCALE,
                    );
                }
            }
        }
        chips
    }
}

/// Convenience TX chain: interleaved block → sub-packet chips.
pub fn tx_subpacket_chips(
    format: &Subtype2DataFormat,
    interleaved: &[u8],
    subpacket: usize,
) -> Vec<Complex32> {
    format.modulate_subpacket(&subpacket_code_symbols(format, interleaved, subpacket))
}

/// Decover one sub-packet of chips against `W_2^4` (sum over each 4-chip
/// period; the two branches are orthogonal over every period).
pub fn decover_w24_symbols(chips: &[Complex32]) -> Vec<Complex32> {
    chips
        .chunks_exact(CHIPS_PER_UNIT)
        .map(|c| c[0] + c[1] - c[2] - c[3])
        .collect()
}

/// Decover one sub-packet of chips against `W_1^2`.
pub fn decover_w12_symbols(chips: &[Complex32]) -> Vec<Complex32> {
    chips
        .chunks_exact(W12_COVER.len())
        .map(|c| c[0] - c[1])
        .collect()
}

/// Max-log LLRs (positive ⇒ bit 0) for one 8-PSK symbol, in code-symbol
/// order `x(3k), x(3k+1), x(3k+2)`.
fn psk8_demap(observed: Complex32) -> [f32; PSK8_BITS] {
    let mut best_zero = [f32::NEG_INFINITY; PSK8_BITS];
    let mut best_one = [f32::NEG_INFINITY; PSK8_BITS];
    for (idx, &(i, q)) in PSK8_POINTS.iter().enumerate() {
        let metric = observed.re * i + observed.im * q;
        for (bit, (zero, one)) in best_zero.iter_mut().zip(best_one.iter_mut()).enumerate() {
            if (idx >> bit) & 1 == 0 {
                *zero = zero.max(metric);
            } else {
                *one = one.max(metric);
            }
        }
    }
    [
        best_zero[0] - best_one[0],
        best_zero[1] - best_one[1],
        best_zero[2] - best_one[2],
    ]
}

/// Demap one sub-packet's decovered branch symbols to `M` soft code symbols
/// in interleaver-output order (positive ⇒ bit 0). Pass the branch streams a
/// format does not use as empty slices.
pub fn demap_subpacket(
    format: &Subtype2DataFormat,
    w24: &[Complex32],
    w12: &[Complex32],
) -> Vec<f32> {
    assert_eq!(w24.len(), format.subframe_w24_symbols());
    assert_eq!(w12.len(), format.subframe_w12_symbols());
    let mut llrs = Vec::with_capacity(format.subframe_code_symbols());
    for unit in 0..SUBFRAME_UNITS {
        match format.modulation {
            ModulationFormat::B4 => llrs.push(w24[unit].im),
            ModulationFormat::Q4 => {
                llrs.push(w24[unit].re);
                llrs.push(w24[unit].im);
            }
            ModulationFormat::Q2 => {
                for symbol in &w12[2 * unit..2 * unit + 2] {
                    llrs.push(symbol.re);
                    llrs.push(symbol.im);
                }
            }
            ModulationFormat::Q4Q2 => {
                llrs.push(w24[unit].re);
                llrs.push(w24[unit].im);
                for symbol in &w12[2 * unit..2 * unit + 2] {
                    llrs.push(symbol.re);
                    llrs.push(symbol.im);
                }
            }
            ModulationFormat::E4E2 => {
                llrs.extend(psk8_demap(w24[unit]));
                llrs.extend(psk8_demap(w12[2 * unit]));
                llrs.extend(psk8_demap(w12[2 * unit + 1]));
            }
        }
    }
    llrs
}

impl Subtype2DataFormat {
    /// HARQ combining buffer indexed by interleaver-output position.
    pub fn new_harq_buffer(&self) -> Vec<f32> {
        vec![0.0; self.encoder_output_symbols()]
    }
}

/// Accumulate one sub-packet's demapped LLRs into the HARQ buffer, undoing
/// the §13.2.1.3.11 symbol selection (`k = (j + i·M) mod N`).
pub fn accumulate_subpacket_llrs(
    format: &Subtype2DataFormat,
    harq: &mut [f32],
    subpacket: usize,
    llrs: &[f32],
) {
    assert!(subpacket < MAX_SUBPACKETS);
    let n = format.encoder_output_symbols();
    assert_eq!(harq.len(), n);
    let m = format.subframe_code_symbols();
    assert_eq!(llrs.len(), m);
    for (j, &llr) in llrs.iter().enumerate() {
        harq[(j + subpacket * m) % n] += llr;
    }
}

/// Deinterleave, descramble, and depuncture accumulated sub-packet LLRs
/// into the rate-1/5 mother stream consumed by
/// [`crate::phy::hrpd::turbo_decoder::HrpdTurboDecoder`]. For the rate-1/3
/// payload (12288 bits) the missing `Y1`/`Y′1` positions become zero-LLR
/// erasures.
pub fn mother_llrs_from_harq_buffer(
    format: &Subtype2DataFormat,
    harq: &[f32],
    interlace_offset: u8,
) -> Vec<f32> {
    let n = format.encoder_output_symbols();
    assert_eq!(harq.len(), n);
    let mut encoder_order = vec![0.0f32; n];
    for (j, src) in format
        .channel_interleaver_source_indices()
        .into_iter()
        .enumerate()
    {
        encoder_order[src] = harq[j];
    }
    descramble_encoder_output_llrs(format, &mut encoder_order, interlace_offset);
    format.depuncture_to_mother_rate_1_5(&encoder_order)
}

/// Rate-1/5 mother symbols per encoder bit period.
const MOTHER_SYMBOLS_PER_PERIOD: usize = 5;
/// Tail bit periods emitted by the turbo encoder (3 per constituent).
const TAIL_PERIODS: usize = 6;

impl Subtype2DataFormat {
    fn depuncture_to_mother_rate_1_5(&self, encoder_order: &[f32]) -> Vec<f32> {
        match self.turbo_code_rate_den {
            5 => encoder_order.to_vec(),
            3 => {
                // Rate-1/3 layout per period: data `[X, Y0, Y′0]`, tail
                // `[X, X, Y0]` / `[X′, X′, Y′0]` (Tables 13.2.1.3.4.2.2-1/-2).
                // Mother layout: data `[X, Y0, Y1, Y′0, Y′1]`, tail
                // `[X, X, Y0, Y1, Y1]` / `[X′, X′, Y′0, Y′1, Y′1]`.
                let n_turbo = HrpdTurboBlock::for_packet_size(self.payload_bits as u32)
                    .expect("supported subtype-2 payload size")
                    .n_turbo as usize;
                let mut mother = vec![0.0f32; self.payload_bits * MOTHER_SYMBOLS_PER_PERIOD];
                for period in 0..n_turbo {
                    let src = period * 3;
                    let dst = period * MOTHER_SYMBOLS_PER_PERIOD;
                    mother[dst] = encoder_order[src];
                    mother[dst + 1] = encoder_order[src + 1];
                    mother[dst + 3] = encoder_order[src + 2];
                }
                for period in n_turbo..n_turbo + TAIL_PERIODS {
                    let src = period * 3;
                    let dst = period * MOTHER_SYMBOLS_PER_PERIOD;
                    mother[dst] = encoder_order[src];
                    mother[dst + 1] = encoder_order[src + 1];
                    mother[dst + 2] = encoder_order[src + 2];
                }
                mother
            }
            _ => unreachable!("subtype-2 reverse turbo rate is 1/5 or 1/3"),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phy::hrpd::turbo::HrpdTurboEncoder;
    use crate::phy::hrpd::turbo_decoder::HrpdTurboDecoder;
    use cdma_common::hrpd::traffic::physical_crc24;

    const FCS_BITS: usize = 24;
    const TAIL_BITS: usize = 6;

    struct Lcg(u32);

    impl Lcg {
        fn next_u32(&mut self) -> u32 {
            self.0 = self.0.wrapping_mul(1_103_515_245).wrapping_add(12_345);
            self.0
        }

        fn next_bit(&mut self) -> u8 {
            ((self.next_u32() >> 16) & 1) as u8
        }

        fn next_unit(&mut self) -> f32 {
            (self.next_u32() >> 8) as f32 / (1u32 << 24) as f32
        }

        /// Approximately unit-variance zero-mean noise (Irwin–Hall with 4
        /// uniform terms).
        fn next_gaussian(&mut self) -> f32 {
            let sum: f32 = (0..4).map(|_| self.next_unit()).sum();
            (sum - 2.0) * 3.0f32.sqrt()
        }
    }

    fn build_physical_packet(payload_bits: usize, rng: &mut Lcg) -> Vec<u8> {
        let mac_bits = payload_bits - FCS_BITS - TAIL_BITS;
        let mut bits: Vec<u8> = (0..mac_bits).map(|_| rng.next_bit()).collect();
        let fcs = physical_crc24(&bits);
        for shift in (0..FCS_BITS).rev() {
            bits.push(((fcs >> shift) & 1) as u8);
        }
        bits.extend(std::iter::repeat_n(0u8, TAIL_BITS));
        assert_eq!(bits.len(), payload_bits);
        bits
    }

    fn packet_crc_ok(bits: &[u8]) -> bool {
        let mac_bits = bits.len() - FCS_BITS - TAIL_BITS;
        let observed = physical_crc24(&bits[..mac_bits]);
        let expected = bits[mac_bits..mac_bits + FCS_BITS]
            .iter()
            .fold(0u32, |acc, &b| (acc << 1) | u32::from(b));
        let tail_ok = bits[mac_bits + FCS_BITS..].iter().all(|&b| b == 0);
        observed == expected && tail_ok
    }

    /// Full TX → noisy channel → RX round trip. Returns the decoded packet.
    fn round_trip(
        payload_bits: usize,
        subpackets: &[usize],
        interlace_offset: u8,
        noise_sigma: f32,
        seed: u32,
    ) -> (Vec<u8>, Vec<u8>) {
        let format = Subtype2DataFormat::for_payload_bits(payload_bits).expect("format");
        let mut rng = Lcg(seed);
        let packet = build_physical_packet(payload_bits, &mut rng);

        let encoder = HrpdTurboEncoder::new(payload_bits as u32).expect("turbo block");
        let mut coded = encoder.encode(&packet, 1, format.turbo_code_rate_den);
        assert_eq!(coded.len(), format.encoder_output_symbols());
        format.scramble_encoder_output(&mut coded, interlace_offset);
        let interleaved = format.interleave_encoder_output(&coded);

        let mut harq = format.new_harq_buffer();
        for &subpacket in subpackets {
            let mut chips = tx_subpacket_chips(format, &interleaved, subpacket);
            if noise_sigma > 0.0 {
                for chip in &mut chips {
                    chip.re += noise_sigma * rng.next_gaussian();
                    chip.im += noise_sigma * rng.next_gaussian();
                }
            }
            let w24 = if format.modulation.uses_w24() {
                decover_w24_symbols(&chips)
            } else {
                Vec::new()
            };
            let w12 = if format.modulation.uses_w12() {
                decover_w12_symbols(&chips)
            } else {
                Vec::new()
            };
            let llrs = demap_subpacket(format, &w24, &w12);
            accumulate_subpacket_llrs(format, &mut harq, subpacket, &llrs);
        }

        let mother = mother_llrs_from_harq_buffer(format, &harq, interlace_offset);
        let decoded = HrpdTurboDecoder::new(payload_bits as u32)
            .expect("turbo block")
            .decode(&mother);
        (packet, decoded)
    }

    #[test]
    fn format_table_matches_spec() {
        // (payload, format, rate den, N, M) from Tables 13.2.1.3.4-2/-3,
        // 13.2.1.3.1.1-2, and §13.2.1.3.9.1–.5.
        let expected = [
            (128, ModulationFormat::B4, 5, 640, 2048),
            (256, ModulationFormat::B4, 5, 1280, 2048),
            (512, ModulationFormat::B4, 5, 2560, 2048),
            (768, ModulationFormat::B4, 5, 3840, 2048),
            (1024, ModulationFormat::B4, 5, 5120, 2048),
            (1536, ModulationFormat::Q4, 5, 7680, 4096),
            (2048, ModulationFormat::Q4, 5, 10240, 4096),
            (3072, ModulationFormat::Q2, 5, 15360, 8192),
            (4096, ModulationFormat::Q2, 5, 20480, 8192),
            (6144, ModulationFormat::Q4Q2, 5, 30720, 12288),
            (8192, ModulationFormat::Q4Q2, 5, 40960, 12288),
            (12288, ModulationFormat::E4E2, 3, 36864, 18432),
        ];
        for (payload, modulation, den, n, m) in expected {
            let format = Subtype2DataFormat::for_payload_bits(payload).expect("format");
            assert_eq!(format.modulation, modulation, "payload {payload}");
            assert_eq!(format.turbo_code_rate_den, den, "payload {payload}");
            assert_eq!(format.encoder_output_symbols(), n, "payload {payload}");
            assert_eq!(format.subframe_code_symbols(), m, "payload {payload}");
            // Table 13.2.1.3.7.2-1 requires N = R × K × 2^m for the U matrix.
            let p = format.interleaver;
            assert_eq!(p.rows * p.levels * (1usize << p.column_bits), payload);
        }
        assert!(Subtype2DataFormat::for_payload_bits(5120).is_none());
    }

    #[test]
    fn walsh_cover_assignment_matches_table_13_2_1_3_3_6() {
        for format in &SUBTYPE2_DATA_FORMATS {
            let (w24, w12) = match format.modulation {
                ModulationFormat::B4 | ModulationFormat::Q4 => (true, false),
                ModulationFormat::Q2 => (false, true),
                ModulationFormat::Q4Q2 | ModulationFormat::E4E2 => (true, true),
            };
            assert_eq!(format.modulation.uses_w24(), w24);
            assert_eq!(format.modulation.uses_w12(), w12);
            assert_eq!(format.subframe_w24_symbols(), if w24 { 2048 } else { 0 });
            assert_eq!(format.subframe_w12_symbols(), if w12 { 4096 } else { 0 });
        }
    }

    #[test]
    fn channel_interleaver_is_a_permutation_for_all_payload_sizes() {
        for format in &SUBTYPE2_DATA_FORMATS {
            let indices = format.channel_interleaver_source_indices();
            let n = format.encoder_output_symbols();
            assert_eq!(indices.len(), n);
            let mut seen = vec![false; n];
            for &idx in &indices {
                assert!(idx < n, "payload {}", format.payload_bits);
                assert!(
                    !seen[idx],
                    "payload {} duplicate {}",
                    format.payload_bits, idx
                );
                seen[idx] = true;
            }
        }
    }

    #[test]
    fn interleaver_reduces_to_pure_bit_reversal_when_k_and_r_are_1() {
        // For K = R = 1 the §13.2.1.3.7.2 matrix stages degenerate to a plain
        // bit-reversal read of each partition, which is the structure the
        // existing B4-only decoder assumes.
        for &payload in &[128usize, 256, 512, 1024] {
            let format = Subtype2DataFormat::for_payload_bits(payload).expect("format");
            let indices = format.channel_interleaver_source_indices();
            let m = format.interleaver.column_bits;
            for (o, &src) in indices.iter().take(payload).enumerate() {
                assert_eq!(src, bit_reverse(o, m) * 5);
            }
            for (o, &src) in indices[payload..3 * payload].iter().enumerate() {
                let pos = bit_reverse(o, m + 1);
                let expected = if pos < payload {
                    pos * 5 + 1
                } else {
                    (pos - payload) * 5 + 3
                };
                assert_eq!(src, expected);
            }
            for (o, &src) in indices[3 * payload..].iter().enumerate() {
                let pos = bit_reverse(o, m + 1);
                let expected = if pos < payload {
                    pos * 5 + 2
                } else {
                    (pos - payload) * 5 + 4
                };
                assert_eq!(src, expected);
            }
        }
    }

    #[test]
    fn scramble_is_an_involution_and_seed_matches_spec_layout() {
        let format = Subtype2DataFormat::for_payload_bits(768).expect("format");
        let mut rng = Lcg(7);
        let original: Vec<u8> = (0..format.encoder_output_symbols())
            .map(|_| rng.next_bit())
            .collect();
        let mut bits = original.clone();
        format.scramble_encoder_output(&mut bits, 2);
        assert_ne!(bits, original);
        format.scramble_encoder_output(&mut bits, 2);
        assert_eq!(bits, original);
        // [1×11, i1 i0, d3 d2 d1 d0] with i=2 → i1i0=10, 768 → d=0011.
        assert_eq!(
            scrambler_initial_state(768, 2),
            (0x7ff << 6) | (0b10 << 4) | 0b0011
        );
    }

    #[test]
    fn q4_and_e4_modulation_tables_spot_checks() {
        let q4 = Subtype2DataFormat::for_payload_bits(1536).expect("format");
        // Table 13.2.1.3.9.2-1 row x(2k)=0, x(2k+1)=1 → (mI, mQ) = (+D, −D).
        let mut symbols = vec![0u8; q4.subframe_code_symbols()];
        symbols[0] = 0;
        symbols[1] = 1;
        let chips = q4.modulate_subpacket(&symbols);
        let w24 = decover_w24_symbols(&chips);
        assert!((w24[0].re - 4.0 * QPSK_AMPLITUDE).abs() < 1e-5);
        assert!((w24[0].im + 4.0 * QPSK_AMPLITUDE).abs() < 1e-5);

        let e4e2 = Subtype2DataFormat::for_payload_bits(12288).expect("format");
        // Table 13.2.1.3.9.5-1 row (x(9k+2), x(9k+1), x(9k)) = (0,0,1)
        // → (mI, mQ) = (+S, +C); Table 13.2.1.3.9.5-2 row x(9k+3..8) = 0
        // → both W_1^2 symbols at (+C, +S).
        let mut symbols = vec![0u8; e4e2.subframe_code_symbols()];
        symbols[0] = 1;
        let chips = e4e2.modulate_subpacket(&symbols);
        let w24 = decover_w24_symbols(&chips);
        let w12 = decover_w12_symbols(&chips);
        assert!((w24[0].re - 4.0 * PSK8_SIN * W24_BRANCH_SCALE).abs() < 1e-5);
        assert!((w24[0].im - 4.0 * PSK8_COS * W24_BRANCH_SCALE).abs() < 1e-5);
        assert!((w12[0].re - 2.0 * PSK8_COS * W12_BRANCH_SCALE).abs() < 1e-5);
        assert!((w12[0].im - 2.0 * PSK8_SIN * W12_BRANCH_SCALE).abs() < 1e-5);
        // The two branches stay orthogonal: each decover recovers exactly its
        // own branch's contribution even though the chips carry the sum.
        assert!((w24[1].re - 4.0 * PSK8_COS * W24_BRANCH_SCALE).abs() < 1e-5);
        assert!((w24[1].im - 4.0 * PSK8_SIN * W24_BRANCH_SCALE).abs() < 1e-5);
        assert!((w12[1].re - 2.0 * PSK8_COS * W12_BRANCH_SCALE).abs() < 1e-5);
        assert!((w12[1].im - 2.0 * PSK8_SIN * W12_BRANCH_SCALE).abs() < 1e-5);
    }

    #[test]
    fn subpacket_selection_wraps_modulo_encoder_block() {
        let format = Subtype2DataFormat::for_payload_bits(128).expect("format");
        let n = format.encoder_output_symbols();
        let interleaved: Vec<u8> = (0..n).map(|i| (i % 2) as u8).collect();
        let m = format.subframe_code_symbols();
        for subpacket in 0..MAX_SUBPACKETS {
            let selected = subpacket_code_symbols(format, &interleaved, subpacket);
            for (j, &symbol) in selected.iter().enumerate() {
                assert_eq!(symbol, interleaved[(j + subpacket * m) % n]);
            }
        }
    }

    #[test]
    fn round_trip_all_payload_sizes_with_mild_noise() {
        for format in &SUBTYPE2_DATA_FORMATS {
            let payload = format.payload_bits;
            // High-payload formats need more than one sub-packet before the
            // effective rate drops to the mother rate.
            let subpackets: &[usize] = if payload >= 6144 { &[0, 1] } else { &[0] };
            let (packet, decoded) =
                round_trip(payload, subpackets, 1, 0.1, 0x1357_9bd0 ^ payload as u32);
            assert_eq!(decoded, packet, "payload {payload}");
            assert!(packet_crc_ok(&decoded), "payload {payload}");
        }
    }

    #[test]
    fn early_termination_decodes_from_subpacket_zero_alone() {
        let (packet, decoded) = round_trip(128, &[0], 0, 0.5, 0xc0ffee11);
        assert_eq!(decoded, packet);
        assert!(packet_crc_ok(&decoded));
    }

    #[test]
    fn incremental_redundancy_gain_second_subpacket_rescues_decode() {
        // 12288-bit E4E2: sub-packet 0 alone is effective rate 2/3 and fails
        // at this SNR; adding sub-packet 1 reaches the rate-1/3 mother code
        // and decodes.
        let sigma = 0.55;
        let seed = 0x0badf00d;
        let (_, decoded_single) = round_trip(12288, &[0], 0, sigma, seed);
        assert!(
            !packet_crc_ok(&decoded_single),
            "single sub-packet unexpectedly decoded; raise sigma"
        );
        let (packet, decoded_pair) = round_trip(12288, &[0, 1], 0, sigma, seed);
        assert_eq!(decoded_pair, packet);
        assert!(packet_crc_ok(&decoded_pair));
    }
}
