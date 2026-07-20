//! Forward-link short-code generation and complex spreading helpers.

use num::complex::Complex32;

use crate::phy::lfsr::{
    GaloisLfsr, HRPD_FORWARD_PN_I_TAPS, HRPD_FORWARD_PN_Q_TAPS, PN_I_TAPS, PN_Q_TAPS,
};

/// Forward-link short-code generator for the I/Q pilot PN sequences.
///
/// This generator emits the standard 15-bit short code with the inserted
/// zero chip at the epoch boundary.
pub struct PnSequence {
    inner: ShortCodePnSequence,
}

/// HRPD forward-link pilot PN sequence generator (C.S0024-200-C §1.4.1.3.4).
///
/// Uses the reciprocal of the 1x short code (see `HRPD_FORWARD_PN_I_TAPS`), with
/// zero offset at the start of the inserted zero run. Not interchangeable with
/// [`PnSequence`] — the two emit time-reversed chip streams.
pub struct HrpdForwardPnSequence {
    inner: ShortCodePnSequence,
}

/// HRPD access-terminal common short-code PN sequence generator.
///
/// Reverse HRPD access uses the access-terminal common short-code PI/PQ
/// polynomials and aligns the zero-offset epoch to the `1` following the
/// inserted 15-zero run.
pub struct HrpdAccessTerminalPnSequence {
    inner: ShortCodePnSequence,
}

struct ShortCodePnSequence {
    lfsr_i: PnLfsr,
    lfsr_q: PnLfsr,

    last_i: u8,
    last_q: u8,

    repeat_index: usize,
    num_repeats: usize,
    period_chips: u64,
}

impl PnSequence {
    /// Create a zero-offset PN generator that emits one chip per call.
    pub fn new(offset: usize, length: usize) -> PnSequence {
        Self::new_repeat(offset, length, 0)
    }

    /// Create a zero-offset PN generator that repeats each chip `repeat + 1` times.
    ///
    pub fn new_repeat(offset: usize, length: usize, repeat: usize) -> PnSequence {
        PnSequence {
            inner: ShortCodePnSequence::new_repeat(
                offset,
                length,
                repeat,
                PN_I_TAPS,
                PN_Q_TAPS,
                PnEpoch::AfterInsertedZeroRun,
            ),
        }
    }

    /// Return the next raw `(I, Q)` PN chip pair as bits.
    pub fn generate(&mut self) -> (u8, u8) {
        self.inner.generate()
    }

    /// Return the next PN chip pair mapped onto the complex alphabet `{+1, -1}`.
    pub fn generate_iq(&mut self) -> Complex32 {
        self.inner.generate_iq()
    }

    /// Advance the short-code generator by `chips` chip intervals.
    ///
    /// This is intended for startup alignment to an absolute CDMA chip clock.
    /// Uses period modulo so very large chip counts are cheap.
    pub fn advance_chips(&mut self, chips: u64) {
        self.inner.advance_chips(chips);
    }
}

impl HrpdForwardPnSequence {
    /// Create a zero-offset HRPD forward PN generator that emits one chip per call.
    pub fn new(offset: usize, length: usize) -> HrpdForwardPnSequence {
        Self::new_repeat(offset, length, 0)
    }

    /// Create a zero-offset HRPD forward PN generator that repeats each chip `repeat + 1` times.
    pub fn new_repeat(offset: usize, length: usize, repeat: usize) -> HrpdForwardPnSequence {
        HrpdForwardPnSequence {
            inner: ShortCodePnSequence::new_repeat(
                offset,
                length,
                repeat,
                HRPD_FORWARD_PN_I_TAPS,
                HRPD_FORWARD_PN_Q_TAPS,
                PnEpoch::StartOfInsertedZeroRun,
            ),
        }
    }

    /// Return the next raw `(I, Q)` PN chip pair as bits.
    pub fn generate(&mut self) -> (u8, u8) {
        self.inner.generate()
    }

    /// Return the next PN chip pair mapped onto the complex alphabet `{+1, -1}`.
    pub fn generate_iq(&mut self) -> Complex32 {
        self.inner.generate_iq()
    }

    /// Advance the short-code generator by `chips` chip intervals.
    pub fn advance_chips(&mut self, chips: u64) {
        self.inner.advance_chips(chips);
    }
}

impl HrpdAccessTerminalPnSequence {
    /// Create a zero-offset HRPD access-terminal PN generator that emits one
    /// chip per call.
    pub fn new(offset: usize, length: usize) -> HrpdAccessTerminalPnSequence {
        Self::new_repeat(offset, length, 0)
    }

    /// Create a zero-offset HRPD access-terminal PN generator that repeats
    /// each chip `repeat + 1` times.
    pub fn new_repeat(offset: usize, length: usize, repeat: usize) -> HrpdAccessTerminalPnSequence {
        HrpdAccessTerminalPnSequence {
            inner: ShortCodePnSequence::new_repeat(
                offset,
                length,
                repeat,
                PN_I_TAPS,
                PN_Q_TAPS,
                PnEpoch::AfterInsertedZeroRun,
            ),
        }
    }

    /// Return the next raw `(I, Q)` PN chip pair as bits.
    pub fn generate(&mut self) -> (u8, u8) {
        self.inner.generate()
    }

    /// Return the next PN chip pair mapped onto the complex alphabet `{+1, -1}`.
    pub fn generate_iq(&mut self) -> Complex32 {
        self.inner.generate_iq()
    }

    /// Advance the short-code generator by `chips` chip intervals.
    pub fn advance_chips(&mut self, chips: u64) {
        self.inner.advance_chips(chips);
    }
}

#[derive(Clone, Copy)]
enum PnEpoch {
    AfterInsertedZeroRun,
    StartOfInsertedZeroRun,
}

impl ShortCodePnSequence {
    fn new_repeat(
        offset: usize,
        length: usize,
        repeat: usize,
        i_taps: u64,
        q_taps: u64,
        epoch: PnEpoch,
    ) -> ShortCodePnSequence {
        assert!(offset <= 511);
        assert!(length > 0);
        let lfsr_i = epoch_pn_lfsr(i_taps, epoch);
        let lfsr_q = epoch_pn_lfsr(q_taps, epoch);

        let mut sequence = ShortCodePnSequence {
            lfsr_i,
            lfsr_q,
            repeat_index: 0,
            num_repeats: repeat,
            last_i: 0,
            last_q: 0,
            period_chips: length as u64,
        };
        let repeat_factor = (repeat as u64) + 1;
        let period = length as u64 * repeat_factor;
        let lag = (offset as u64 * 64 * repeat_factor) % period;
        if lag != 0 {
            sequence.advance_chips(period - lag);
        }
        sequence
    }

    fn generate(&mut self) -> (u8, u8) {
        if self.repeat_index == 0 {
            self.last_i = self.lfsr_i.next();
            self.last_q = self.lfsr_q.next();
        }

        self.repeat_index += 1;
        if self.repeat_index > self.num_repeats {
            self.repeat_index = 0;
        }

        (self.last_i, self.last_q)
    }

    fn generate_iq(&mut self) -> Complex32 {
        let val = self.generate();
        Complex32::new(
            if val.0 == 0 { 1.0 } else { -1.0 },
            if val.1 == 0 { 1.0 } else { -1.0 },
        )
    }

    fn advance_chips(&mut self, chips: u64) {
        let repeat_factor = (self.num_repeats as u64) + 1;
        let period = self.period_chips * repeat_factor;
        let steps = chips % period;
        for _ in 0..steps {
            let _ = self.generate();
        }
    }
}

struct PnLfsr {
    lfsr: GaloisLfsr,
    zeros: usize,
}

impl PnLfsr {
    fn new(lfsr: GaloisLfsr) -> PnLfsr {
        PnLfsr { lfsr, zeros: 0 }
    }

    fn next(&mut self) -> u8 {
        if self.zeros == 14 {
            self.zeros = 0;
            return 0;
        }

        let val = self.lfsr.next();
        if val == 0 {
            self.zeros += 1
        } else {
            self.zeros = 0;
        }
        val
    }
}

fn epoch_pn_lfsr(taps: u64, epoch: PnEpoch) -> PnLfsr {
    let mut probe = PnLfsr::new(GaloisLfsr::new((1 << 15) - 1, taps));
    let mut zeros = 0usize;
    let mut zero_run_start = None;
    let mut zero_run_end = None;
    for idx in 0..32_768 {
        let bit = probe.next();
        if bit == 0 {
            zeros += 1;
            if zeros == 15 {
                zero_run_start = Some(idx + 1 - 15);
                zero_run_end = Some(idx + 1);
                break;
            }
        } else {
            zeros = 0;
        }
    }

    let mut lfsr = PnLfsr::new(GaloisLfsr::new((1 << 15) - 1, taps));
    let advance = match epoch {
        PnEpoch::AfterInsertedZeroRun => {
            zero_run_end.expect("PN sequence must contain inserted 15-zero run")
        }
        PnEpoch::StartOfInsertedZeroRun => {
            zero_run_start.expect("PN sequence must contain inserted 15-zero run")
        }
    };
    for _ in 0..advance {
        let _ = lfsr.next();
    }
    lfsr
}

pub trait PnChipSource {
    fn generate_iq(&mut self) -> Complex32;
    fn advance_chips(&mut self, chips: u64);
}

impl PnChipSource for PnSequence {
    fn generate_iq(&mut self) -> Complex32 {
        PnSequence::generate_iq(self)
    }

    fn advance_chips(&mut self, chips: u64) {
        PnSequence::advance_chips(self, chips);
    }
}

impl PnChipSource for HrpdForwardPnSequence {
    fn generate_iq(&mut self) -> Complex32 {
        HrpdForwardPnSequence::generate_iq(self)
    }

    fn advance_chips(&mut self, chips: u64) {
        HrpdForwardPnSequence::advance_chips(self, chips);
    }
}

impl PnChipSource for HrpdAccessTerminalPnSequence {
    fn generate_iq(&mut self) -> Complex32 {
        HrpdAccessTerminalPnSequence::generate_iq(self)
    }

    fn advance_chips(&mut self, chips: u64) {
        HrpdAccessTerminalPnSequence::advance_chips(self, chips);
    }
}

/// Applies forward-link complex spreading with the configured short-code PN sequence.
///
/// Each call advances the underlying PN generator by one chip interval.
pub struct Spreader<P = PnSequence> {
    pn_sequence: P,
}

impl<P: PnChipSource> Spreader<P> {
    /// Create a spreader over the supplied PN sequence.
    pub fn new(pn_sequence: P) -> Spreader<P> {
        Spreader { pn_sequence }
    }

    /// Spread one complex chip using the current PN state.
    pub fn spread(&mut self, value: &Complex32) -> Complex32 {
        let pn = self.pn_sequence.generate_iq();
        // Forward-link spreading per the spec arm equations:
        //   I' = I*PN_I - Q*PN_Q
        //   Q' = I*PN_Q + Q*PN_I
        // which is the complex product (I + jQ) * (PN_I + jPN_Q).
        let out = Complex32::new(
            (value.re * pn.re) - (value.im * pn.im),
            (value.re * pn.im) + (value.im * pn.re),
        );
        // The SDR sees analytic baseband samples, so emit the conjugated
        // complex envelope of the spec arm outputs.
        out.conj()
    }

    /// Spread a slice of chips, preserving PN phase continuity across the slice.
    pub fn spread_many(&mut self, values: &[Complex32]) -> Vec<Complex32> {
        values.iter().map(|v| self.spread(v)).collect::<Vec<_>>()
    }

    /// Align the internal short-code PN phase to the provided absolute
    /// chip position since CDMA epoch.
    ///
    /// Call only before generating output samples.
    pub fn align_to_chip(&mut self, chip: u64) {
        self.pn_sequence.advance_chips(chip);
    }

    /// Advance relative to the current short-code PN phase.
    pub fn advance_chips(&mut self, chips: u64) {
        self.pn_sequence.advance_chips(chips);
    }
}

#[cfg(test)]
mod tests {
    use super::{HrpdAccessTerminalPnSequence, HrpdForwardPnSequence, PnSequence};

    #[test]
    pub fn test_pn_repeat() {
        let mut pn_seq = PnSequence::new_repeat(0, 32768, 1);
        let sequence = (0..32768).map(|_| pn_seq.generate().0).collect::<Vec<_>>();
        assert_eq!(&[1, 1, 0, 0, 1, 1, 0, 0], &sequence[0..8]);

        pn_seq = PnSequence::new_repeat(0, 32768, 2);
        let sequence = (0..32768).map(|_| pn_seq.generate().0).collect::<Vec<_>>();
        assert_eq!(&[1, 1, 1, 0, 0, 0, 1, 1, 1, 0, 0, 0], &sequence[0..12]);
    }

    #[test]
    pub fn test_pn_pilot() {
        let mut pn_seq = PnSequence::new(0, 32768);
        let sequence = (0..32768).map(|_| pn_seq.generate()).collect::<Vec<_>>();
        let sequence1 = (0..32768).map(|_| pn_seq.generate()).collect::<Vec<_>>();

        let sequence_i = sequence
            .iter()
            .map(|x| format!("{}", x.0))
            .collect::<Vec<_>>()
            .join("");
        let sequence_q = sequence
            .iter()
            .map(|x| format!("{}", x.1))
            .collect::<Vec<_>>()
            .join("");

        assert_eq!(
            "10101001001110100011011110011001000001111",
            &sequence_i[0..41]
        );

        assert_eq!(
            1,
            sequence_i
                .match_indices("000000000000000")
                .collect::<Vec<_>>()
                .len(),
        );
        assert_eq!(
            1,
            sequence_q
                .match_indices("000000000000000")
                .collect::<Vec<_>>()
                .len(),
        );

        assert_eq!(sequence, sequence1);
    }

    #[test]
    pub fn test_hrpd_forward_pn_pilot_matches_spec_epoch_and_recursions() {
        let mut pn_seq = HrpdForwardPnSequence::new(0, 32768);
        let sequence = (0..32768).map(|_| pn_seq.generate()).collect::<Vec<_>>();
        let sequence_i_string = sequence
            .iter()
            .map(|x| format!("{}", x.0))
            .collect::<Vec<_>>()
            .join("");
        let sequence_q_string = sequence
            .iter()
            .map(|x| format!("{}", x.1))
            .collect::<Vec<_>>()
            .join("");
        assert_eq!("0000000000000001", &sequence_i_string[0..16]);
        assert_eq!("0000000000000001", &sequence_q_string[0..16]);
        assert_eq!("1", &sequence_i_string[32767..32768]);
        assert_eq!("1", &sequence_q_string[32767..32768]);

        let sequence_i = strip_inserted_zero(sequence.iter().map(|x| x.0).collect::<Vec<_>>());
        let sequence_q = strip_inserted_zero(sequence.iter().map(|x| x.1).collect::<Vec<_>>());

        assert_forward_recursion(&sequence_i, &[15, 13, 9, 8, 7, 5]);
        assert_forward_recursion(&sequence_q, &[15, 12, 11, 10, 6, 5, 4, 3]);
    }

    #[test]
    pub fn test_pn_offset_lags_zero_offset_sequence() {
        let mut zero = PnSequence::new(0, 32768);
        let zero_sequence = (0..32768).map(|_| zero.generate()).collect::<Vec<_>>();
        let mut offset_one = PnSequence::new(1, 32768);
        let offset_one_sequence = (0..128).map(|_| offset_one.generate()).collect::<Vec<_>>();

        assert_eq!(&zero_sequence[32704..32768], &offset_one_sequence[0..64]);
        assert_eq!(&zero_sequence[0..64], &offset_one_sequence[64..128]);
    }

    #[test]
    pub fn test_hrpd_forward_pn_offset_lags_zero_offset_sequence() {
        let mut zero = HrpdForwardPnSequence::new(0, 32768);
        let zero_sequence = (0..32768).map(|_| zero.generate()).collect::<Vec<_>>();
        let mut offset_one = HrpdForwardPnSequence::new(1, 32768);
        let offset_one_sequence = (0..128).map(|_| offset_one.generate()).collect::<Vec<_>>();

        assert_eq!(&zero_sequence[32704..32768], &offset_one_sequence[0..64]);
        assert_eq!(&zero_sequence[0..64], &offset_one_sequence[64..128]);
    }

    #[test]
    pub fn test_hrpd_access_terminal_pn_epoch_is_after_inserted_zero_run() {
        let mut pn_seq = HrpdAccessTerminalPnSequence::new(0, 32768);
        let sequence = (0..32768).map(|_| pn_seq.generate()).collect::<Vec<_>>();
        let sequence_i_string = sequence
            .iter()
            .map(|x| format!("{}", x.0))
            .collect::<Vec<_>>()
            .join("");
        let sequence_q_string = sequence
            .iter()
            .map(|x| format!("{}", x.1))
            .collect::<Vec<_>>()
            .join("");

        assert_eq!("1", &sequence_i_string[0..1]);
        assert_eq!("1", &sequence_q_string[0..1]);
        assert_eq!(
            1,
            sequence_i_string
                .match_indices("000000000000000")
                .collect::<Vec<_>>()
                .len(),
        );
        assert_eq!(
            1,
            sequence_q_string
                .match_indices("000000000000000")
                .collect::<Vec<_>>()
                .len(),
        );
    }

    fn strip_inserted_zero(mut sequence: Vec<u8>) -> Vec<u8> {
        let mut zeros = 0usize;
        for idx in 0..sequence.len() {
            if sequence[idx] == 0 {
                zeros += 1;
                if zeros == 15 {
                    sequence.remove(idx);
                    return sequence;
                }
            } else {
                zeros = 0;
            }
        }
        panic!("PN sequence did not contain the inserted zero");
    }

    fn assert_forward_recursion(sequence: &[u8], lags: &[usize]) {
        assert_eq!(32_767, sequence.len());
        for n in 15..sequence.len() {
            let expected = lags.iter().fold(0u8, |acc, lag| acc ^ sequence[n - lag]);
            assert_eq!(expected, sequence[n], "recursion mismatch at chip {n}");
        }
    }

    /// Analyze the difference between our PN spreading convention and the
    /// spec convention (C.S0002-E Figure 3.1.3.1.1.1-27).
    ///
    /// Spec complex multiplier:
    ///   s = V × P = (v_i + j·v_q)(pn_i + j·pn_q)
    ///   s_I = v_i·pn_i − v_q·pn_q
    ///   s_Q = v_i·pn_q + v_q·pn_i
    ///
    /// Our code (Spreader::spread):
    ///   s = V × P̄ = (v_i + j·v_q)(pn_i − j·pn_q)
    ///   s_I = v_i·pn_i + v_q·pn_q
    ///   s_Q = v_q·pn_i − v_i·pn_q
    #[test]
    fn test_pn_spread_conventions_analysis() {
        use num_complex::Complex32;

        // Helper: spec convention V × P
        fn spec_spread(v: Complex32, pn: Complex32) -> Complex32 {
            Complex32::new(v.re * pn.re - v.im * pn.im, v.re * pn.im + v.im * pn.re)
        }

        // Helper: our convention V × P̄
        fn our_spread(v: Complex32, pn: Complex32) -> Complex32 {
            Complex32::new(v.re * pn.re + v.im * pn.im, v.im * pn.re - v.re * pn.im)
        }

        // Helper: spec despread (multiply by conj(P))
        fn spec_despread(r: Complex32, pn: Complex32) -> Complex32 {
            Complex32::new(r.re * pn.re + r.im * pn.im, r.im * pn.re - r.re * pn.im)
        }

        // Helper: our despread (multiply by P, since TX was × P̄)
        fn our_despread(r: Complex32, pn: Complex32) -> Complex32 {
            Complex32::new(r.re * pn.re - r.im * pn.im, r.re * pn.im + r.im * pn.re)
        }

        let all_pn_combos: [(f32, f32); 4] = [(1.0, 1.0), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0)];

        eprintln!("=== Test 1: RC1 (BPSK, Q=0) — pilot data = +1 ===");
        let v_rc1 = Complex32::new(1.0, 0.0);
        for &(pi, pq) in &all_pn_combos {
            let pn = Complex32::new(pi, pq);
            let spec_out = spec_spread(v_rc1, pn);
            let our_out = our_spread(v_rc1, pn);
            eprintln!(
                "  PN=({:+.0},{:+.0}): spec=({:+.0},{:+.0}) ours=({:+.0},{:+.0}) I_same={} Q_same={}",
                pi,
                pq,
                spec_out.re,
                spec_out.im,
                our_out.re,
                our_out.im,
                (spec_out.re - our_out.re).abs() < 1e-6,
                (spec_out.im - our_out.im).abs() < 1e-6,
            );
        }
        // For RC1 Q=0: spec_I = v_i*pn_i, our_I = v_i*pn_i → SAME
        //              spec_Q = v_i*pn_q, our_Q = -v_i*pn_q → NEGATED
        for &(pi, pq) in &all_pn_combos {
            let pn = Complex32::new(pi, pq);
            let spec_out = spec_spread(v_rc1, pn);
            let our_out = our_spread(v_rc1, pn);
            assert!(
                (spec_out.re - our_out.re).abs() < 1e-6,
                "RC1: I-lane should be identical"
            );
            assert!(
                (spec_out.im + our_out.im).abs() < 1e-6,
                "RC1: Q-lane should be negated (spec_Q = -our_Q)"
            );
        }
        eprintln!("  → RC1 CONFIRMED: I identical, Q negated between spec and ours\n");

        eprintln!("=== Test 2: RC3 (QPSK, Q≠0) — data = (+1, -1) ===");
        let v_rc3 = Complex32::new(1.0, -1.0);
        for &(pi, pq) in &all_pn_combos {
            let pn = Complex32::new(pi, pq);
            let spec_out = spec_spread(v_rc3, pn);
            let our_out = our_spread(v_rc3, pn);
            eprintln!(
                "  PN=({:+.0},{:+.0}): spec=({:+.0},{:+.0}) ours=({:+.0},{:+.0})",
                pi, pq, spec_out.re, spec_out.im, our_out.re, our_out.im,
            );
        }
        eprintln!();

        eprintln!("=== Test 3: Round-trip despread — does each convention recover V? ===");

        eprintln!("\n--- 3a: Spec TX, spec despread (V × P, then × P̄) ---");
        for v in &[
            Complex32::new(1.0, 0.0),  // RC1 pilot
            Complex32::new(-1.0, 0.0), // RC1 data
            Complex32::new(1.0, -1.0), // RC3 QPSK
            Complex32::new(-1.0, 1.0), // RC3 QPSK
        ] {
            for &(pi, pq) in &all_pn_combos {
                let pn = Complex32::new(pi, pq);
                let on_air = spec_spread(*v, pn);
                let recovered = spec_despread(on_air, pn);
                // Should be 2*V (since P·P̄ = |P|² = pn_i²+pn_q² = 2)
                assert!(
                    (recovered.re - 2.0 * v.re).abs() < 1e-6
                        && (recovered.im - 2.0 * v.im).abs() < 1e-6,
                    "Spec round-trip failed: V=({},{}), PN=({},{}), got ({},{}), expected ({},{})",
                    v.re,
                    v.im,
                    pi,
                    pq,
                    recovered.re,
                    recovered.im,
                    2.0 * v.re,
                    2.0 * v.im,
                );
            }
        }
        eprintln!("  → Spec TX + spec despread: all recover 2V ✓");

        eprintln!("\n--- 3b: Our TX, our despread (V × P̄, then × P) ---");
        for v in &[
            Complex32::new(1.0, 0.0),
            Complex32::new(-1.0, 0.0),
            Complex32::new(1.0, -1.0),
            Complex32::new(-1.0, 1.0),
        ] {
            for &(pi, pq) in &all_pn_combos {
                let pn = Complex32::new(pi, pq);
                let on_air = our_spread(*v, pn);
                let recovered = our_despread(on_air, pn);
                assert!(
                    (recovered.re - 2.0 * v.re).abs() < 1e-6
                        && (recovered.im - 2.0 * v.im).abs() < 1e-6,
                    "Our round-trip failed: V=({},{}), PN=({},{}), got ({},{})",
                    v.re,
                    v.im,
                    pi,
                    pq,
                    recovered.re,
                    recovered.im,
                );
            }
        }
        eprintln!("  → Our TX + our despread: all recover 2V ✓");

        eprintln!("\n--- 3c: Our TX, spec despread (V × P̄, then × P̄) — CROSS-CONVENTION ---");
        let mut cross_ok = true;
        for v in &[Complex32::new(1.0, 0.0), Complex32::new(1.0, -1.0)] {
            for &(pi, pq) in &all_pn_combos {
                let pn = Complex32::new(pi, pq);
                let on_air = our_spread(*v, pn);
                let recovered = spec_despread(on_air, pn);
                let expected_2v = Complex32::new(2.0 * v.re, 2.0 * v.im);
                let matches = (recovered.re - expected_2v.re).abs() < 1e-6
                    && (recovered.im - expected_2v.im).abs() < 1e-6;
                if !matches {
                    cross_ok = false;
                }
                eprintln!(
                    "  V=({:+.0},{:+.0}) PN=({:+.0},{:+.0}): recovered=({:+.0},{:+.0}) expected=({:+.0},{:+.0}) {}",
                    v.re,
                    v.im,
                    pi,
                    pq,
                    recovered.re,
                    recovered.im,
                    expected_2v.re,
                    expected_2v.im,
                    if matches { "✓" } else { "✗ MISMATCH" },
                );
            }
        }
        if cross_ok {
            eprintln!("  → Cross-convention: all match (shouldn't happen!)");
        } else {
            eprintln!("  → Cross-convention: MISMATCHES found (expected — conventions don't pair)");
        }

        eprintln!("\n--- 3d: Our TX, phone pilot-based despread ---");
        eprintln!("  Phone learns channel H from pilot, despreads data with conj(H)/|H|²");
        // Our pilot: V_pilot = (1, 0). On air: our_spread((1,0), pn) for each chip.
        // Phone estimates H = average of (received_pilot × conj(known_pilot_PN)).
        // With our convention: H = our_spread((1,0), pn) × conj(pn)... but H is
        // estimated per-chip and averaged. For a single chip:
        //   H_chip = our_spread((1,0), pn) = (pn_i, -pn_q)
        //   phone_reference = spec pilot = (pn_i, pn_q)   [what phone expects]
        //   H_est = H_chip × conj(phone_reference) = (pn_i, -pn_q)(pn_i, -pn_q)
        //         = (pn_i² + pn_q², -(pn_i·pn_q + pn_q·pn_i))
        //         = (2, -2·pn_i·pn_q)
        //
        // Over many chips, pn_i·pn_q averages to 0, so H_est ≈ (2, 0).
        // Phone despreads data: received × conj(H_est)/|H_est|²
        //                     = received × (2, 0) / 4 = received / 2
        //
        // With our data TX: on_air = our_spread(V, pn) = V × P̄
        // Phone "matched filter": on_air × conj(H_est)/|H_est|²
        //   = (V × P̄) × (2, 0) / 4    [H_est ≈ (2,0)]
        //   This doesn't actually despread — we need per-chip processing.
        //
        // Actually, phone does: for each chip, multiply received by conj(pn)/scaling
        // using its OWN PN (spec convention). Let's compute properly:

        // Phone despread per chip = received × conj(spec_pn) where spec_pn = (pn_i, pn_q)
        // received = our_spread(V, pn) = V × (pn_i - j·pn_q)
        // conj(spec_pn) = (pn_i, -pn_q)
        // recovered = V × (pn_i - j·pn_q) × (pn_i - j·pn_q) = V × (pn_i - j·pn_q)²

        eprintln!("  Phone uses spec PN for despread: received × conj(pn_spec)");
        eprintln!("  = (V × P̄) × P̄  = V × P̄²");
        eprintln!("  P̄² = (pn_i - j·pn_q)² = (pn_i² - pn_q²) - 2j·pn_i·pn_q");
        eprintln!("  For pn_i,pn_q ∈ {{±1}}: pn_i²=pn_q²=1 → P̄² = -2j·pn_i·pn_q");
        eprintln!("  So recovered = V × (-2j·pn_i·pn_q) = ROTATED, not clean 2V");
        eprintln!();
        eprintln!("  BUT phone uses PILOT to estimate and compensate the channel.");
        eprintln!("  Pilot: our_spread((1,0), pn) = (pn_i, -pn_q)");
        eprintln!("  Phone correlates pilot against its known (pn_i, pn_q):");
        eprintln!("    pilot_corr = (pn_i, -pn_q) × conj(pn_i, pn_q)");
        eprintln!("               = (pn_i, -pn_q) × (pn_i, -pn_q)");
        eprintln!("               = (pn_i²+pn_q², -(pn_i·pn_q+pn_q·pn_i))");
        eprintln!("               = (2, -2·pn_i·pn_q)");
        eprintln!();

        // Over N chips: Σ pilot_corr = Σ(2, -2·pn_i·pn_q)
        // The real part sums to 2N. The imag part: Σ pn_i·pn_q is a cross-correlation
        // of I and Q PN sequences, which for distinct maximal-length sequences is ~√N.
        // So |imag| << |real| for large N, and H_est ≈ (2N, ~±√N) ≈ (2N, 0).
        //
        // Phone normalizes: H_est / |H_est| ≈ (1, 0).
        // Phone despreads data chip: recovered = received_chip × conj(H_est/|H_est|) × conj(pn)
        // Actually, standard CDMA receiver does RAKE despreading:
        //   despread_chip = received_chip × conj(pn) / H_est_normalized
        //
        // Simpler: phone computes per-finger:
        //   y = Σ_k received_k × conj(pn_k)    [integrate over Walsh period]
        //   Then divides by channel estimate to get data.
        //
        // Let's just compute the Walsh-integrated result:

        eprintln!("=== Test 4: Walsh-integrated despread over 4 chips (simplified) ===");
        eprintln!("Using 4 chips with PN pairs to show the summation behavior.\n");

        let pn_chips: [(f32, f32); 4] = [(1.0, 1.0), (1.0, -1.0), (-1.0, 1.0), (-1.0, -1.0)];
        let walsh_0 = [1.0f32, 1.0, 1.0, 1.0]; // Walsh 0 = all +1

        for v in &[
            ("RC1 pilot (1,0)", Complex32::new(1.0, 0.0)),
            ("RC1 data (-1,0)", Complex32::new(-1.0, 0.0)),
            ("RC3 QPSK (1,-1)", Complex32::new(1.0, -1.0)),
            ("RC3 QPSK (-1,1)", Complex32::new(-1.0, 1.0)),
        ] {
            let (label, data) = v;

            // Our TX: for each chip, spread data × walsh × PN
            let mut spec_sum = Complex32::new(0.0, 0.0);
            let mut our_sum = Complex32::new(0.0, 0.0);

            for k in 0..4 {
                let w = walsh_0[k];
                let pn = Complex32::new(pn_chips[k].0, pn_chips[k].1);
                let v_w = Complex32::new(data.re * w, data.im * w);

                let spec_chip = spec_spread(v_w, pn);
                let our_chip = our_spread(v_w, pn);

                // Phone despreads with conj(pn) × walsh (= conj(pn) since walsh=1)
                let conj_pn = Complex32::new(pn.re, -pn.im);

                // spec TX → spec despread
                let spec_ds = Complex32::new(
                    spec_chip.re * conj_pn.re - spec_chip.im * conj_pn.im,
                    spec_chip.re * conj_pn.im + spec_chip.im * conj_pn.re,
                );
                spec_sum.re += spec_ds.re * w;
                spec_sum.im += spec_ds.im * w;

                // our TX → phone despread with conj(pn)
                let our_ds = Complex32::new(
                    our_chip.re * conj_pn.re - our_chip.im * conj_pn.im,
                    our_chip.re * conj_pn.im + our_chip.im * conj_pn.re,
                );
                our_sum.re += our_ds.re * w;
                our_sum.im += our_ds.im * w;
            }

            eprintln!(
                "  {}: spec_despread=({:+.0},{:+.0}) our_TX+conj_despread=({:+.0},{:+.0}) expected_8V=({:+.0},{:+.0})",
                label,
                spec_sum.re,
                spec_sum.im,
                our_sum.re,
                our_sum.im,
                8.0 * data.re,
                8.0 * data.im,
            );
        }

        eprintln!("\n=== Test 5: What does the phone actually see? ===");
        eprintln!("Phone uses conj(pn_spec) for despread. Our TX uses V×P̄.");
        eprintln!("If pn_spec = (pn_i, pn_q), conj(pn_spec) = (pn_i, -pn_q) = P̄.");
        eprintln!("So phone computes: received × P̄ = (V × P̄) × P̄ = V × P̄²");
        eprintln!("P̄² ≠ |P|² unless P is real. So this does NOT cleanly recover V.");
        eprintln!("HOWEVER: the phone's RAKE receiver uses pilot-based channel");
        eprintln!("estimation to compensate. The pilot goes through the same");
        eprintln!("V×P̄ convention, so the channel estimate absorbs the sign flip.");
        eprintln!();

        // Prove it: pilot-based equalized despread
        eprintln!("=== Test 6: Pilot-based equalized despread (the real phone path) ===");
        for v in &[
            ("RC1 (1,0)", Complex32::new(1.0, 0.0)),
            ("RC3 (1,-1)", Complex32::new(1.0, -1.0)),
            ("RC3 (-1,1)", Complex32::new(-1.0, 1.0)),
        ] {
            let (label, data) = v;
            let mut data_accum = Complex32::new(0.0, 0.0);
            let mut pilot_accum = Complex32::new(0.0, 0.0);
            let pilot_data = Complex32::new(1.0, 0.0); // pilot is always (1, 0)

            for k in 0..4 {
                let pn = Complex32::new(pn_chips[k].0, pn_chips[k].1);

                // Our TX
                let pilot_chip = our_spread(pilot_data, pn);
                let data_chip = our_spread(*data, pn);

                // Phone despreads both with conj(pn_spec) where pn_spec = pn
                let conj_pn = Complex32::new(pn.re, -pn.im);

                let pilot_ds = Complex32::new(
                    pilot_chip.re * conj_pn.re - pilot_chip.im * conj_pn.im,
                    pilot_chip.re * conj_pn.im + pilot_chip.im * conj_pn.re,
                );
                let data_ds = Complex32::new(
                    data_chip.re * conj_pn.re - data_chip.im * conj_pn.im,
                    data_chip.re * conj_pn.im + data_chip.im * conj_pn.re,
                );

                pilot_accum.re += pilot_ds.re;
                pilot_accum.im += pilot_ds.im;
                data_accum.re += data_ds.re;
                data_accum.im += data_ds.im;
            }

            // Phone equalizes: data_accum × conj(pilot_accum) / |pilot_accum|²
            let conj_pilot = Complex32::new(pilot_accum.re, -pilot_accum.im);
            let pilot_power = pilot_accum.re * pilot_accum.re + pilot_accum.im * pilot_accum.im;
            let equalized = Complex32::new(
                (data_accum.re * conj_pilot.re - data_accum.im * conj_pilot.im) / pilot_power,
                (data_accum.re * conj_pilot.im + data_accum.im * conj_pilot.re) / pilot_power,
            );

            eprintln!(
                "  {}: pilot_accum=({:+.1},{:+.1}) data_accum=({:+.1},{:+.1}) equalized=({:+.3},{:+.3}) expected=({:+.0},{:+.0}) {}",
                label,
                pilot_accum.re,
                pilot_accum.im,
                data_accum.re,
                data_accum.im,
                equalized.re,
                equalized.im,
                data.re,
                data.im,
                if (equalized.re - data.re).abs() < 0.01 && (equalized.im - data.im).abs() < 0.01 {
                    "✓"
                } else {
                    "✗ WRONG"
                },
            );
        }
    }
}
