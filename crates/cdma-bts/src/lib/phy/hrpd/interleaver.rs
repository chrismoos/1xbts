//! HRPD (1xEV-DO Rev 0) channel interleaver primitives.
//!
//! Spec: 3GPP2 C.S0024 v4.0, §9.2.1.3.5 "Channel Interleaving" (reverse link)
//! and §9.3.1.3.2.3.4 "Channel Interleaving" (forward traffic channel).
//!
//! The reverse-link channel interleaver is a pure bit-reversal interleaver
//! on `2^L` symbol positions. Per §9.2.1.3.5, the i-th interleaved symbol
//! is read out from input address `A_i`:
//!
//! ```text
//!     A_i = Bit_Reversal(i, L)        i = 0, …, 2^L − 1
//! ```
//!
//! i.e. `output[i] = input[BRO(i, L)]`, where `Bit_Reversal(y, L)` reverses
//! the `L`-bit binary representation of `y` (C.S0024-0 v4.0 §9.2.1.3.5,
//! eq. on p. 9-52). The spec example gives BRO(6, 9-bit) = 192 for M = 512
//! (§9.3.1.3.2.3.4.2 step 3).
//!
//! The forward-traffic interleaver of §9.3.1.3.2.3.4 layers an additional
//! per-column end-around shift and a column-wise `BRO(j)` reorder on a
//! `K × M` rectangular array; that algorithm is built on top of this
//! `BRO(j, ⌈log2 M⌉)` primitive (step 3 of §9.3.1.3.2.3.4.2).
//!
//! The 1x `phy::coding::block_interleaver::BitReversalInterleaver` implements
//! the C.S0002 1x permutation `index = 2^m * (i mod J) + BRO(m, i / J)` for
//! arbitrary `J`. That is *not* the HRPD permutation — the 1x form pre-folds
//! a row index `J` and bit-reverses the row count, while the HRPD
//! reverse-link interleaver bit-reverses the full address. The two coincide
//! only for `J = 1`, so we do not reuse the 1x interleaver here.
//!
//! Non-power-of-two block sizes:
//!   Rev 0 forward-traffic block sizes (`M ∈ {512, 1024, 2048}` per
//!   §9.3.1.3.2.3.4.2-1) and reverse-link block sizes are always powers of
//!   two, so the spec's `Bit_Reversal(i, L)` is only defined for `2^L`. For
//!   robustness on non-power-of-two `block_size` we use
//!   `L = ⌈log2(block_size)⌉` and emit input symbols in BRO order while
//!   skipping addresses that fall outside `[0, block_size)`. This pruned
//!   bit-reversal still round-trips with `channel_deinterleave`.

/// Returns ⌈log2(n)⌉ for n ≥ 1.
fn ceil_log2(n: usize) -> u32 {
    assert!(n >= 1, "block size must be ≥ 1");
    if n == 1 { 0 } else { (n - 1).ilog2() + 1 }
}

/// Reverse the low `bits` bits of `value`. `bits` must be ≤ 32.
fn bit_reverse(value: u32, bits: u32) -> u32 {
    if bits == 0 {
        0
    } else {
        value.reverse_bits() >> (32 - bits)
    }
}

/// Channel-interleave `input` per HRPD §9.2.1.3.5: `output[i] = input[BRO(i, L)]`
/// with `L = ⌈log2(block_size)⌉`.
///
/// For power-of-two `block_size` (the only case the Rev 0 spec actually
/// defines), every address in `[0, 2^L)` is valid and this is the exact
/// spec permutation. For non-power-of-two `block_size`, addresses whose
/// `BRO(i, L)` exceeds `block_size − 1` are skipped (pruned), so the
/// output is the sequence of valid `input[BRO(i, L)]` values in increasing
/// `i` order.
///
/// Panics if `input.len() != block_size`.
pub fn channel_interleave(block_size: usize, input: &[u8]) -> Vec<u8> {
    assert_eq!(
        input.len(),
        block_size,
        "channel_interleave: input length {} != block_size {}",
        input.len(),
        block_size
    );
    let l = ceil_log2(block_size);
    let padded = 1usize << l;
    let mut output = Vec::with_capacity(block_size);
    for i in 0..padded {
        let a = bit_reverse(i as u32, l) as usize;
        if a < block_size {
            output.push(input[a]);
        }
    }
    debug_assert_eq!(output.len(), block_size);
    output
}

/// Inverse of [`channel_interleave`]. Restores the original input order
/// by walking the same BRO addresses and placing `interleaved[o]` back
/// at the corresponding input position.
///
/// Panics if `interleaved.len() != block_size`.
pub fn channel_deinterleave(block_size: usize, interleaved: &[u8]) -> Vec<u8> {
    assert_eq!(
        interleaved.len(),
        block_size,
        "channel_deinterleave: input length {} != block_size {}",
        interleaved.len(),
        block_size
    );
    let l = ceil_log2(block_size);
    let padded = 1usize << l;
    let mut output = vec![0u8; block_size];
    let mut next_out: usize = 0;
    for i in 0..padded {
        let a = bit_reverse(i as u32, l) as usize;
        if a < block_size {
            output[a] = interleaved[next_out];
            next_out += 1;
        }
    }
    debug_assert_eq!(next_out, block_size);
    output
}

/// Forward Traffic / Control Channel interleaver from C.S0024-0
/// §9.3.1.3.2.3.4.
///
/// `payload_bits` is the physical-layer packet size, including the 6-bit
/// encoder tail field that the turbo encoder discards before encoding.
/// `effective_den` is the effective turbo-code denominator after rate
/// matching. Rev 0 forward traffic uses rate 1/3 or 1/5 here.
pub fn forward_channel_interleave(payload_bits: usize, effective_den: u8, input: &[u8]) -> Vec<u8> {
    match effective_den {
        3 => forward_rate_1_3_interleave(payload_bits, input),
        5 => forward_rate_1_5_interleave(payload_bits, input),
        _ => panic!("unsupported HRPD forward interleaver rate 1/{effective_den}"),
    }
}

fn forward_rate_1_3_interleave(payload_bits: usize, input: &[u8]) -> Vec<u8> {
    assert_eq!(input.len(), payload_bits * 3);
    let params = forward_interleaver_params(payload_bits);
    let mut u = vec![0u8; payload_bits];
    let mut v0_vp0 = vec![0u8; payload_bits * 2];
    for k in 0..payload_bits {
        u[k] = input[k * 3];
        v0_vp0[k] = input[k * 3 + 1];
        v0_vp0[payload_bits + k] = input[k * 3 + 2];
    }

    let u = forward_symbol_permute(&u, params, ForwardInterleaverBlock::U);
    let v0_vp0 = forward_symbol_permute(&v0_vp0, params, ForwardInterleaverBlock::V);
    [u, v0_vp0].concat()
}

fn forward_rate_1_5_interleave(payload_bits: usize, input: &[u8]) -> Vec<u8> {
    assert_eq!(input.len(), payload_bits * 5);
    let params = forward_interleaver_params(payload_bits);
    let mut u = vec![0u8; payload_bits];
    let mut v0_vp0 = vec![0u8; payload_bits * 2];
    let mut v1_vp1 = vec![0u8; payload_bits * 2];
    for k in 0..payload_bits {
        u[k] = input[k * 5];
        v0_vp0[k] = input[k * 5 + 1];
        // C.S0024 §9.3.1.3.2.3.4.1 maps the rate-1/5 turbo output
        // X,Y0,Y1,Y'0,Y'1 onto U,V0,V'0,V1,V1'. Keep the same order as the
        // live-proven Control Channel path.
        v0_vp0[payload_bits + k] = input[k * 5 + 3];
        v1_vp1[k] = input[k * 5 + 2];
        v1_vp1[payload_bits + k] = input[k * 5 + 4];
    }

    let u = forward_symbol_permute(&u, params, ForwardInterleaverBlock::U);
    let v0_vp0 = forward_symbol_permute(&v0_vp0, params, ForwardInterleaverBlock::V);
    let v1_vp1 = forward_symbol_permute(&v1_vp1, params, ForwardInterleaverBlock::V);
    [u, v0_vp0, v1_vp1].concat()
}

#[derive(Debug, Clone, Copy)]
struct ForwardInterleaverParams {
    levels: usize,
    rows: usize,
    column_bits: u32,
    v_shift_divisor: usize,
}

fn forward_interleaver_params(payload_bits: usize) -> ForwardInterleaverParams {
    match payload_bits {
        1024 => ForwardInterleaverParams {
            levels: 1,
            rows: 2,
            column_bits: 9,
            v_shift_divisor: 4,
        },
        2048 => ForwardInterleaverParams {
            levels: 1,
            rows: 2,
            column_bits: 10,
            v_shift_divisor: 4,
        },
        3072 => ForwardInterleaverParams {
            levels: 1,
            rows: 3,
            column_bits: 10,
            v_shift_divisor: 4,
        },
        4096 => ForwardInterleaverParams {
            levels: 1,
            rows: 4,
            column_bits: 10,
            v_shift_divisor: 4,
        },
        // C.S0024-200-C §2.4.1.3.2.3.4.3, Table 2.4.1.3.2.3.4.3-1:
        // Enhanced FTC subtype 1 DRC 0xd/0xe canonical 5120-bit packets use
        // K=5, R=4, m=8, D=10.
        5120 => ForwardInterleaverParams {
            levels: 5,
            rows: 4,
            column_bits: 8,
            v_shift_divisor: 10,
        },
        _ => panic!("unsupported HRPD forward packet size {payload_bits}"),
    }
}

#[derive(Debug, Clone, Copy)]
enum ForwardInterleaverBlock {
    U,
    V,
}

fn forward_symbol_permute(
    input: &[u8],
    params: ForwardInterleaverParams,
    block: ForwardInterleaverBlock,
) -> Vec<u8> {
    let columns =
        1usize << (params.column_bits + u32::from(matches!(block, ForwardInterleaverBlock::V)));
    assert_eq!(input.len(), params.rows * columns * params.levels);
    let mut out = vec![0u8; input.len()];
    let bits = columns.ilog2();
    for c in 0..columns {
        let final_col = bit_reverse(c as u32, bits) as usize;
        for k in 0..params.levels {
            let shift = match block {
                ForwardInterleaverBlock::U => (c * params.levels + k) % params.rows,
                ForwardInterleaverBlock::V => {
                    ((c * params.levels + k) / params.v_shift_divisor) % params.rows
                }
            };
            let final_level = swapped_forward_level(k, params.levels);
            for final_row in 0..params.rows {
                let input_row = (final_row + params.rows - shift) % params.rows;
                let input_idx = ((input_row * columns + c) * params.levels) + k;
                let output_idx = ((final_level * columns + final_col) * params.rows) + final_row;
                out[output_idx] = input[input_idx];
            }
        }
    }
    out
}

fn swapped_forward_level(level: usize, levels: usize) -> usize {
    if levels <= 3 {
        return level;
    }
    let mid = levels / 2;
    if level == mid {
        1
    } else if level == 1 {
        mid
    } else {
        level
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ceil_log2_basic() {
        assert_eq!(ceil_log2(1), 0);
        assert_eq!(ceil_log2(2), 1);
        assert_eq!(ceil_log2(3), 2);
        assert_eq!(ceil_log2(4), 2);
        assert_eq!(ceil_log2(16), 4);
        assert_eq!(ceil_log2(1024), 10);
        assert_eq!(ceil_log2(2048), 11);
    }

    #[test]
    fn bit_reverse_basic() {
        // BRO(6, 4-bit) = 0b0110 → 0b0110 = 6
        assert_eq!(bit_reverse(0b0110, 4), 0b0110);
        // BRO(1, 4-bit) = 0b0001 → 0b1000 = 8
        assert_eq!(bit_reverse(1, 4), 8);
        // BRO(3, 4-bit) = 0b0011 → 0b1100 = 12
        assert_eq!(bit_reverse(3, 4), 12);
        // Spec example §9.3.1.3.2.3.4.2: for M = 512, BRO(6) = 192.
        // 6 = 0b000000110, reversed (9-bit) = 0b011000000 = 192.
        assert_eq!(bit_reverse(6, 9), 192);
    }

    #[test]
    fn subtype2_forward_interleaver_params_include_5120_row() {
        let params = forward_interleaver_params(5120);
        assert_eq!(params.levels, 5);
        assert_eq!(params.rows, 4);
        assert_eq!(params.column_bits, 8);
        assert_eq!(params.v_shift_divisor, 10);
    }

    #[test]
    fn forward_symbol_permute_k5_u_matches_spec_spot_checks() {
        let params = ForwardInterleaverParams {
            levels: 5,
            rows: 4,
            column_bits: 2,
            v_shift_divisor: 10,
        };
        let input: Vec<u8> = (0..80u8).collect();
        let out = forward_symbol_permute(&input, params, ForwardInterleaverBlock::U);

        assert_forward_symbol_mapping(&input, &out, params, ForwardInterleaverBlock::U, 0, 0, 0);
        assert_forward_symbol_mapping(&input, &out, params, ForwardInterleaverBlock::U, 1, 2, 3);
        assert_forward_symbol_mapping(&input, &out, params, ForwardInterleaverBlock::U, 3, 4, 1);
    }

    #[test]
    fn forward_symbol_permute_k5_v_uses_d10_shift_and_level_swap() {
        let params = ForwardInterleaverParams {
            levels: 5,
            rows: 4,
            column_bits: 2,
            v_shift_divisor: 10,
        };
        let input: Vec<u8> = (0..160u8).collect();
        let out = forward_symbol_permute(&input, params, ForwardInterleaverBlock::V);

        assert_forward_symbol_mapping(&input, &out, params, ForwardInterleaverBlock::V, 3, 4, 2);
        assert_forward_symbol_mapping(&input, &out, params, ForwardInterleaverBlock::V, 4, 2, 1);
        assert_forward_symbol_mapping(&input, &out, params, ForwardInterleaverBlock::V, 7, 1, 3);
    }

    fn assert_forward_symbol_mapping(
        input: &[u8],
        out: &[u8],
        params: ForwardInterleaverParams,
        block: ForwardInterleaverBlock,
        column: usize,
        level: usize,
        row: usize,
    ) {
        let column_bits =
            params.column_bits + u32::from(matches!(block, ForwardInterleaverBlock::V));
        let columns = 1usize << column_bits;
        let shift = match block {
            ForwardInterleaverBlock::U => (column * params.levels + level) % params.rows,
            ForwardInterleaverBlock::V => {
                ((column * params.levels + level) / params.v_shift_divisor) % params.rows
            }
        };
        let final_row = (row + shift) % params.rows;
        let final_col = bit_reverse(column as u32, column_bits) as usize;
        let final_level = swapped_forward_level(level, params.levels);
        let input_idx = ((row * columns + column) * params.levels) + level;
        let output_idx = ((final_level * columns + final_col) * params.rows) + final_row;
        assert_eq!(out[output_idx], input[input_idx]);
    }

    #[test]
    fn forward_channel_interleave_5120_rate_1_3_has_spec_symbol_count() {
        let input: Vec<u8> = (0..(5120 * 3)).map(|i| (i & 1) as u8).collect();
        let out = forward_channel_interleave(5120, 3, &input);

        assert_eq!(out.len(), 15_360);
        assert_eq!(out.iter().filter(|&&bit| bit == 1).count(), 7_680);
        assert_ne!(out, input, "5120 K=5 interleaver must not be identity");
    }

    /// Hand-computed expected output for block_size = 16, input = 0..16.
    /// Per §9.2.1.3.5: output[i] = input[BRO(i, 4)]
    /// BRO(i, 4) for i in 0..16:
    ///   0→0, 1→8, 2→4, 3→12, 4→2, 5→10, 6→6, 7→14,
    ///   8→1, 9→9, 10→5, 11→13, 12→3, 13→11, 14→7, 15→15
    #[test]
    fn channel_interleave_16_hand_computed() {
        let input: Vec<u8> = (0..16u8).collect();
        let out = channel_interleave(16, &input);
        assert_eq!(
            out,
            vec![0, 8, 4, 12, 2, 10, 6, 14, 1, 9, 5, 13, 3, 11, 7, 15]
        );
    }

    #[test]
    fn round_trip_power_of_two_sizes() {
        for &block_size in &[16usize, 64, 256, 512, 1024, 2048] {
            let input: Vec<u8> = (0..block_size)
                .map(|i| (i as u8).wrapping_mul(31))
                .collect();
            let interleaved = channel_interleave(block_size, &input);
            let recovered = channel_deinterleave(block_size, &interleaved);
            assert_eq!(recovered, input, "round-trip failed for block {block_size}");
        }
    }

    #[test]
    fn round_trip_rev0_packet_block_sizes() {
        // Rev 0 forward-traffic per-block M values from C.S0024
        // §9.3.1.3.2.3.4.2-1: {512, 1024, 2048}. All pow-2.
        for &m in &[512usize, 1024, 2048] {
            let input: Vec<u8> = (0..m).map(|i| (i as u8) ^ 0xA5).collect();
            let interleaved = channel_interleave(m, &input);
            let recovered = channel_deinterleave(m, &interleaved);
            assert_eq!(recovered, input);
        }
    }

    #[test]
    fn round_trip_non_power_of_two() {
        // The Rev 0 spec only defines pow-2 sizes; this covers the pruning
        // extension we use for robustness.
        for &block_size in &[3usize, 5, 7, 12, 17, 100] {
            let input: Vec<u8> = (0..block_size).map(|i| i as u8).collect();
            let interleaved = channel_interleave(block_size, &input);
            let recovered = channel_deinterleave(block_size, &interleaved);
            assert_eq!(recovered, input, "round-trip failed for block {block_size}");
        }
    }

    #[test]
    fn interleave_is_permutation_of_input() {
        for &block_size in &[16usize, 64, 1024] {
            let input: Vec<u8> = (0..block_size).map(|i| (i as u8).wrapping_add(1)).collect();
            let mut interleaved = channel_interleave(block_size, &input);
            let mut sorted_input = input.clone();
            interleaved.sort();
            sorted_input.sort();
            assert_eq!(interleaved, sorted_input);
        }
    }

    #[test]
    #[should_panic(expected = "input length")]
    fn length_mismatch_panics() {
        channel_interleave(16, &[0u8; 8]);
    }
}
