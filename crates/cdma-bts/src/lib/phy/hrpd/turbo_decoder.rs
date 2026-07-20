//! HRPD Rev 0 soft-input turbo decoder (Max-Log-MAP).
//!
//! C.S0024-0 v4.0 §9.3.1.3.2.4 / §9.2.1.3.4.2 turbo code:
//! - 8-state recursive constituent encoder, 3-bit shift register.
//! - Feedback polynomial   d(D)  = 1 + D² + D³
//! - Parity 0              n0(D) = 1 + D + D³
//! - Parity 1              n1(D) = 1 + D + D² + D³
//! - Two parallel CEs, an internal interleaver between them.
//! - Mother rate 1/5: per info bit the encoder emits [X, Y0, Y1, Y'0, Y'1]
//!   (the second-CE systematic X' is punctured by spec). After the data
//!   period CE1 clocks 3 tail steps, then CE2 clocks 3 tail steps, producing
//!   18 total tail output symbols.
//!
//! This decoder runs iterative Max-Log-MAP across the two constituent BCJR
//! sweeps. Max-Log-MAP avoids transcendental correction terms in the
//! reverse-link ARQ deadline path; the physical CRC remains the final packet
//! acceptance criterion. The interleaver permutation comes from
//! `HrpdTurboEncoder::interleaver()`.
//!
//! Input: 5 × `payload_bits` soft LLRs in encoder symbol order
//!         X[0], Y0[0], Y1[0], Y'0[0], Y'1[0], X[1], …
//! (Convention: positive LLR ⇒ bit 0; negative ⇒ bit 1, matching the
//! reverse-link demod path which produces +1 for chip 0 and −1 for chip 1.)
//!
//! Output: `payload_bits` hard-decided physical packet bits (one bit per byte,
//! 0/1). The first `payload_bits - 6` bits are decoded turbo inputs; the final
//! 6 physical TAIL bits are not turbo-encoded and are returned as zero.

use super::turbo::{HrpdTurboBlock, HrpdTurboEncoder};

const NUM_STATES: usize = 8;
const DEFAULT_ITERATIONS: usize = 8;
const NEG_INF: f32 = -1.0e9;

/// Soft-input Log-MAP turbo decoder, parameterised by payload size.
pub struct HrpdTurboDecoder {
    encoder: HrpdTurboEncoder,
    interleaver: Vec<u32>,
    iterations: usize,
}

impl HrpdTurboDecoder {
    pub fn new(payload_bits: u32) -> Option<Self> {
        let encoder = HrpdTurboEncoder::new(payload_bits)?;
        let interleaver = encoder.interleaver().to_vec();
        Some(Self {
            encoder,
            interleaver,
            iterations: DEFAULT_ITERATIONS,
        })
    }

    pub fn with_iterations(mut self, iters: usize) -> Self {
        self.iterations = iters.max(1);
        self
    }

    pub fn block(&self) -> HrpdTurboBlock {
        self.encoder.block()
    }

    pub fn decode(&self, llrs: &[f32]) -> Vec<u8> {
        self.decode_soft(llrs)
            .into_iter()
            .map(|llr| if llr >= 0.0 { 0 } else { 1 })
            .collect()
    }

    /// Decode iteratively and stop as soon as `accept` validates the hard
    /// decision. Reverse-link H-ARQ uses this with the physical CRC-24 so a
    /// clean first sub-packet does not pay for all configured turbo iterations
    /// before its fixed, near-term ARQ response window.
    pub fn decode_until<F>(&self, llrs: &[f32], mut accept: F) -> Option<(Vec<u8>, usize)>
    where
        F: FnMut(&[u8]) -> bool,
    {
        let (_, accepted) = self.decode_soft_impl(llrs, |soft, iteration| {
            let hard = soft
                .iter()
                .map(|&llr| if llr >= 0.0 { 0 } else { 1 })
                .collect::<Vec<_>>();
            accept(&hard).then_some((hard, iteration))
        });
        accepted
    }

    /// Decode a rate-1/5 mother-stream of LLRs into final posterior LLRs for
    /// `payload_bits` physical bits. Tail symbols are appended after the data
    /// section; if the input is shorter than expected, missing positions are
    /// treated as zero LLR (erasures).
    pub fn decode_soft(&self, llrs: &[f32]) -> Vec<f32> {
        self.decode_soft_impl(llrs, |_, _| None::<()>).0
    }

    fn decode_soft_impl<T, F>(&self, llrs: &[f32], mut after_iteration: F) -> (Vec<f32>, Option<T>)
    where
        F: FnMut(&[f32], usize) -> Option<T>,
    {
        let block = self.encoder.block();
        let n_data = block.n_turbo as usize;
        let payload_bits = block.payload_bits as usize;
        let trellis_len = n_data + 3;

        // Pull per-bit systematic + parity LLRs out of the mother stream.
        // Layout per data bit (5 symbols): X, Y0, Y1, Y'0, Y'1.
        let symbols_per_bit = 5;
        let data_len = symbols_per_bit * payload_bits;
        let mut x = vec![0.0_f32; trellis_len];
        let mut y0 = vec![0.0_f32; trellis_len];
        let mut y1 = vec![0.0_f32; trellis_len];
        let mut y0p = vec![0.0_f32; trellis_len];
        let mut y1p = vec![0.0_f32; trellis_len];
        let mut x_pi = vec![0.0_f32; trellis_len];
        for k in 0..n_data {
            let base = k * symbols_per_bit;
            if base + 4 < llrs.len() && base + 4 < data_len {
                x[k] = llrs[base];
                y0[k] = llrs[base + 1];
                y1[k] = llrs[base + 2];
                y0p[k] = llrs[base + 3];
                y1p[k] = llrs[base + 4];
            }
        }
        let tail_base = n_data * symbols_per_bit;
        if tail_base + 30 <= llrs.len() {
            for t in 0..3 {
                let src = tail_base + t * symbols_per_bit;
                x[n_data + t] = llrs[src] + llrs[src + 1];
                y0[n_data + t] = llrs[src + 2];
                y1[n_data + t] = llrs[src + 3] + llrs[src + 4];
            }
            for t in 0..3 {
                let src = tail_base + 15 + t * symbols_per_bit;
                x_pi[n_data + t] = llrs[src] + llrs[src + 1];
                y0p[n_data + t] = llrs[src + 2];
                y1p[n_data + t] = llrs[src + 3] + llrs[src + 4];
            }
        }

        // Build CE2's view of the systematic bits via the interleaver. Only
        // the Nturbo data bits go through the interleaver; CE2's three tail
        // bits are generated by CE2's own switch-down clocks.
        for i in 0..n_data {
            let src = self.interleaver[i] as usize;
            x_pi[i] = x[src];
        }

        // Extrinsic LLRs flowing between the two BCJRs. Initialized to zero
        // (no a-priori).
        let mut le_to_ce1 = vec![0.0_f32; trellis_len];
        let mut soft = vec![0.0_f32; payload_bits];
        // The physical tail is not turbo encoded and is always zero. Seed its
        // hard-decision polarity before the first early-termination check.
        soft[n_data..payload_bits].fill(f32::INFINITY);

        // Scratch reused across every sweep/iteration so the decoder allocates
        // these once per frame instead of per BCJR call. `bcjr_log_map` resets
        // alpha/beta and fully writes `bcjr_llr`; le1/le1_pi/le2_pi are only
        // written on `[0, n_data)`, and their tail stays zero across iterations.
        let mut alpha = vec![[NEG_INF; NUM_STATES]; trellis_len + 1];
        let mut beta = vec![[NEG_INF; NUM_STATES]; trellis_len + 1];
        let mut bcjr_llr = vec![0.0_f32; trellis_len];
        let mut le1 = vec![0.0_f32; trellis_len];
        let mut le1_pi = vec![0.0_f32; trellis_len];
        let mut le2_pi = vec![0.0_f32; trellis_len];

        for iteration in 1..=self.iterations {
            // CE1 BCJR.
            bcjr_log_map(
                &x,
                &y0,
                &y1,
                &le_to_ce1,
                n_data,
                &mut alpha,
                &mut beta,
                &mut bcjr_llr,
            );
            // Extrinsic = posterior − systematic − a-priori, on the data
            // bits only.
            for k in 0..n_data {
                le1[k] = bcjr_llr[k] - x[k] - le_to_ce1[k];
            }

            // Interleave to feed CE2.
            for i in 0..n_data {
                let src = self.interleaver[i] as usize;
                le1_pi[i] = le1[src];
            }

            // CE2 BCJR on the interleaved systematic + Y'0 / Y'1.
            bcjr_log_map(
                &x_pi,
                &y0p,
                &y1p,
                &le1_pi,
                n_data,
                &mut alpha,
                &mut beta,
                &mut bcjr_llr,
            );
            // Extrinsic from CE2 in the interleaved domain.
            for k in 0..n_data {
                le2_pi[k] = bcjr_llr[k] - x_pi[k] - le1_pi[k];
            }
            // Deinterleave to feed CE1 next iteration.
            for i in 0..n_data {
                let src = self.interleaver[i] as usize;
                le_to_ce1[src] = le2_pi[i];
            }

            // Final hard decision from the combined LLR on CE1's order.
            for k in 0..n_data {
                soft[k] = x[k] + le1[k] + le_to_ce1[k];
            }
            if let Some(value) = after_iteration(&soft, iteration) {
                return (soft, Some(value));
            }
        }

        // The physical packet TAIL field is discarded before turbo encoding;
        // fill those trailing positions so the returned representation matches
        // the full physical packet length the spec defines.
        (soft, None)
    }
}

/// Single Max-Log-MAP sweep over one constituent encoder's trellis.
/// `x` is the systematic LLR per bit (Nturbo data bits + 3 constituent tail
/// bits).
/// `y0`/`y1` are parity LLRs with the same length.
/// `la` is the a-priori LLR from the other CE with the same length; it is zero
/// on tail positions.
/// `n_data` is the number of data bits (the first `n_data` of `payload_bits`).
/// Returns the posterior LLR per bit.
/// Caller-provided scratch reused across every BCJR sweep within one
/// `decode_soft` call so the decoder allocates these buffers once per frame
/// instead of 16 times. The buffers are fully reset at the top of each sweep,
/// so their incoming contents do not matter.
fn bcjr_log_map(
    x: &[f32],
    y0: &[f32],
    y1: &[f32],
    la: &[f32],
    n_data: usize,
    alpha: &mut [[f32; NUM_STATES]],
    beta: &mut [[f32; NUM_STATES]],
    llr: &mut [f32],
) {
    let n = x.len();
    let trans = &*TRELLIS;

    // Forward metrics α[k][s] over (n+1) time steps.
    alpha.fill([NEG_INF; NUM_STATES]);
    alpha[0][0] = 0.0;
    for k in 0..n {
        for s in 0..NUM_STATES {
            if alpha[k][s] <= NEG_INF / 2.0 {
                continue;
            }
            for u in 0..2 {
                if k >= n_data && u as u8 != tail_input_for_state(s) {
                    continue;
                }
                let t = &trans[s][u];
                let g = branch_metric(u as u8, t.y0, t.y1, x[k], y0[k], y1[k], la[k], k < n_data);
                let cand = alpha[k][s] + g;
                alpha[k + 1][t.next_state] = log_sum(alpha[k + 1][t.next_state], cand);
            }
        }
    }

    // Backward metrics β[k][s]. We terminate the trellis at state 0 after
    // the tail (the encoder forces zero state via tail bits).
    beta.fill([NEG_INF; NUM_STATES]);
    beta[n][0] = 0.0;
    for k in (0..n).rev() {
        for s in 0..NUM_STATES {
            let mut sum = NEG_INF;
            for u in 0..2 {
                if k >= n_data && u as u8 != tail_input_for_state(s) {
                    continue;
                }
                let t = &trans[s][u];
                if beta[k + 1][t.next_state] <= NEG_INF / 2.0 {
                    continue;
                }
                let g = branch_metric(u as u8, t.y0, t.y1, x[k], y0[k], y1[k], la[k], k < n_data);
                let cand = beta[k + 1][t.next_state] + g;
                sum = log_sum(sum, cand);
            }
            beta[k][s] = sum;
        }
    }

    // Posterior LLR: max over (s -> s', u=0) − max over (s -> s', u=1).
    // Positive ⇒ u=0 ⇒ bit 0.
    for k in 0..n {
        let mut sum0 = NEG_INF;
        let mut sum1 = NEG_INF;
        for s in 0..NUM_STATES {
            if alpha[k][s] <= NEG_INF / 2.0 {
                continue;
            }
            for u in 0..2 {
                if k >= n_data && u as u8 != tail_input_for_state(s) {
                    continue;
                }
                let t = &trans[s][u];
                if beta[k + 1][t.next_state] <= NEG_INF / 2.0 {
                    continue;
                }
                let g = branch_metric(u as u8, t.y0, t.y1, x[k], y0[k], y1[k], la[k], k < n_data);
                let cand = alpha[k][s] + g + beta[k + 1][t.next_state];
                if u == 0 {
                    sum0 = log_sum(sum0, cand);
                } else {
                    sum1 = log_sum(sum1, cand);
                }
            }
        }
        llr[k] = sum0 - sum1;
    }
}

#[inline]
fn log_sum(a: f32, b: f32) -> f32 {
    a.max(b)
}

#[inline]
fn tail_input_for_state(state: usize) -> u8 {
    let m2 = ((state >> 1) & 0b001) as u8;
    let m3 = ((state >> 2) & 0b001) as u8;
    m2 ^ m3
}

#[inline]
fn branch_metric(
    u: u8,
    y0_bit: u8,
    y1_bit: u8,
    lx: f32,
    ly0: f32,
    ly1: f32,
    la: f32,
    use_la: bool,
) -> f32 {
    // Convention: LLR positive => bit 0. Branch metric uses
    // 0.5 * (1 - 2*bit) * LLR which is +LLR/2 for bit 0 and −LLR/2 for bit 1.
    // We drop the constant 0.5 factor since it is uniform.
    let sgn = |b: u8| -> f32 { if b == 0 { 1.0 } else { -1.0 } };
    let mut m = sgn(u) * lx + sgn(y0_bit) * ly0 + sgn(y1_bit) * ly1;
    if use_la {
        m += sgn(u) * la;
    }
    m
}

#[derive(Clone, Copy)]
struct Transition {
    next_state: usize,
    y0: u8,
    y1: u8,
}

/// The rate-1/5 constituent-encoder trellis is fixed, so build it once and share
/// it across every BCJR sweep instead of rebuilding it per call (16 calls/frame).
static TRELLIS: std::sync::LazyLock<[[Transition; 2]; NUM_STATES]> =
    std::sync::LazyLock::new(trellis_transitions);

fn trellis_transitions() -> [[Transition; 2]; NUM_STATES] {
    // Build the table by inspecting the encoder's recursive logic.
    let mut tbl = [[Transition {
        next_state: 0,
        y0: 0,
        y1: 0,
    }; 2]; NUM_STATES];
    for s in 0..NUM_STATES {
        let m1 = (s & 0b001) as u8;
        let m2 = ((s >> 1) & 0b001) as u8;
        let m3 = ((s >> 2) & 0b001) as u8;
        for u in 0..2u8 {
            let v = u ^ m2 ^ m3;
            let y0 = v ^ m1 ^ m3;
            let y1 = v ^ m1 ^ m2 ^ m3;
            // Shift register update: m1' = v, m2' = m1, m3' = m2.
            let ns = (v as usize) | ((m1 as usize) << 1) | ((m2 as usize) << 2);
            tbl[s][u as usize] = Transition {
                next_state: ns,
                y0,
                y1,
            };
        }
    }
    tbl
}

/// Map a rate-1/5 hard symbol stream (one bit per byte, 0/1) into LLRs.
/// Bit 0 → +amplitude, bit 1 → −amplitude. Useful for tests against the
/// existing forward encoder which emits hard symbols.
pub fn hard_symbols_to_llrs(symbols: &[u8], amplitude: f32) -> Vec<f32> {
    symbols
        .iter()
        .map(|&b| if b == 0 { amplitude } else { -amplitude })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lcg_payload(payload_bits: u32) -> Vec<u8> {
        // Deterministic Park-Miller LCG bit stream.
        let mut s: u32 = 0xC0FFEEED;
        let mut out = Vec::with_capacity(payload_bits as usize);
        for i in 0..payload_bits {
            s = s.wrapping_mul(48271).wrapping_add(1);
            // Force the trailing 6 tail bits to 0 (the encoder discards them
            // as tail anyway, but pinning makes the test deterministic).
            let is_tail = i >= payload_bits - 6;
            let bit = if is_tail { 0 } else { ((s >> 16) & 1) as u8 };
            out.push(bit);
        }
        out
    }

    fn round_trip(payload_bits: u32) {
        let encoder = HrpdTurboEncoder::new(payload_bits).expect("encoder");
        let decoder = HrpdTurboDecoder::new(payload_bits)
            .expect("decoder")
            .with_iterations(8);
        let payload = lcg_payload(payload_bits);
        let coded = encoder.encode(&payload, 1, 5);
        let llrs = hard_symbols_to_llrs(&coded, 4.0);
        let decoded = decoder.decode(&llrs);
        // Compare the data bits (first n_turbo positions); the trailing 6
        // tail bits are encoder-driven, not part of the carried payload.
        let n_data = decoder.block().n_turbo as usize;
        for k in 0..n_data {
            assert_eq!(decoded[k], payload[k], "bit {k} mismatch");
        }
    }

    #[test]
    fn round_trip_128() {
        round_trip(128);
    }

    #[test]
    fn round_trip_256() {
        round_trip(256);
    }

    #[test]
    fn round_trip_512() {
        round_trip(512);
    }

    #[test]
    fn round_trip_1024() {
        round_trip(1024);
    }

    #[test]
    fn round_trip_2048() {
        round_trip(2048);
    }

    #[test]
    fn round_trip_3072() {
        round_trip(3072);
    }

    #[test]
    fn round_trip_4096() {
        round_trip(4096);
    }

    #[test]
    fn round_trip_768() {
        round_trip(768);
    }

    #[test]
    fn round_trip_1536() {
        round_trip(1536);
    }

    #[test]
    fn round_trip_6144() {
        round_trip(6144);
    }

    #[test]
    fn round_trip_8192() {
        round_trip(8192);
    }

    #[test]
    fn round_trip_12288() {
        round_trip(12288);
    }

    #[test]
    fn trellis_table_returns_state_zero_to_zero_on_zero_input() {
        let t = trellis_transitions();
        assert_eq!(t[0][0].next_state, 0);
        assert_eq!(t[0][0].y0, 0);
        assert_eq!(t[0][0].y1, 0);
    }

    /// Verify the soft decoder corrects errors that the hard channel would
    /// not survive: encode, add Gaussian noise sufficient to corrupt several
    /// bit positions in the systematic stream, decode, expect ≤1% error rate.
    #[test]
    fn decodes_through_awgn_with_low_error_rate() {
        // Use 1024 payload (n_turbo=1018 data bits) for a fast deterministic
        // test. Sigma = 0.7 corresponds to roughly Eb/N0 ~ 2 dB which is
        // well into the turbo-coding waterfall region.
        let payload_bits = 1024u32;
        let encoder = HrpdTurboEncoder::new(payload_bits).expect("encoder");
        let decoder = HrpdTurboDecoder::new(payload_bits)
            .expect("decoder")
            .with_iterations(8);
        let payload = lcg_payload(payload_bits);
        let coded = encoder.encode(&payload, 1, 5);
        let amplitude = 1.0_f32;
        let sigma = 0.7_f32;

        let mut s: u32 = 0xDEADBEEF;
        let mut rng = || -> f32 {
            s = s.wrapping_mul(1664525).wrapping_add(1013904223);
            (s >> 8) as f32 / ((1u32 << 24) as f32)
        };
        let llrs: Vec<f32> = coded
            .iter()
            .map(|&b| {
                let signal = if b == 0 { amplitude } else { -amplitude };
                let mut u1 = rng();
                if u1 < 1e-7 {
                    u1 = 1e-7;
                }
                let u2 = rng();
                let n = (-2.0_f32 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos();
                // The channel observation is `signal + sigma*n`. Convert to
                // LLR by scaling with 2 / sigma^2 (the optimal log-likelihood
                // mapping for AWGN BPSK).
                let obs = signal + sigma * n;
                obs * 2.0 / (sigma * sigma)
            })
            .collect();

        let decoded = decoder.decode(&llrs);
        let n_data = decoder.block().n_turbo as usize;
        let errors = (0..n_data).filter(|&k| decoded[k] != payload[k]).count();
        // At sigma=0.7 with 8 turbo iterations the decoder should correct
        // essentially everything; allow up to 1% as a guard against the
        // pseudo-random Box-Muller stream having an unusually bad sample.
        assert!(
            errors * 100 < n_data,
            "AWGN error count {errors}/{n_data} above 1% — decoder regression"
        );
    }

    #[test]
    fn hard_symbols_to_llrs_polarity() {
        let llrs = hard_symbols_to_llrs(&[0, 1, 0, 1], 2.5);
        assert_eq!(llrs, vec![2.5, -2.5, 2.5, -2.5]);
    }
}
