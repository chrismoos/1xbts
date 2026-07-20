//! Internal linear-feedback shift register helpers for CDMA short-code generation.

/// Feedback taps for the 1x SR1 forward short-code I sequence
/// (C.S0002-E §2.1.3.1.17.1). Also the HRPD reverse access-terminal common PN
/// (C.S0024-200-C §1.3.1.3.8.1).
pub(crate) const PN_I_TAPS: u64 = 0b100001011100010;

/// Feedback taps for the 1x SR1 forward short-code Q sequence
/// (C.S0002-E §2.1.3.1.17.1).
pub(crate) const PN_Q_TAPS: u64 = 0b100111100011100;

/// Feedback taps for the HRPD forward pilot PN I sequence
/// (C.S0024-200-C §1.4.1.3.4): the reciprocal (time-reversed) form of [`PN_I_TAPS`].
pub(crate) const HRPD_FORWARD_PN_I_TAPS: u64 = 0b101000111010000;

/// Feedback taps for the HRPD forward pilot PN Q sequence
/// (C.S0024-200-C §1.4.1.3.4): the reciprocal of [`PN_Q_TAPS`].
pub(crate) const HRPD_FORWARD_PN_Q_TAPS: u64 = 0b100111000111100;

/// 15-stage Galois LFSR used to synthesize CDMA short-code chips.
#[derive(Clone, Copy, Debug)]
pub(crate) struct GaloisLfsr {
    state: u64,
    taps: u64,
}

impl GaloisLfsr {
    /// Create an LFSR with the provided `seed` and Galois feedback `taps`.
    pub(crate) fn new(seed: u64, taps: u64) -> Self {
        Self { state: seed, taps }
    }

    /// Advance one chip and return the output bit.
    pub(crate) fn next(&mut self) -> u8 {
        let out = self.state & 1;
        self.state >>= 1;
        if out == 1 {
            self.state ^= self.taps;
        }
        out as u8
    }

    #[cfg(test)]
    fn state(&self) -> u64 {
        self.state
    }
}

#[cfg(test)]
mod tests {
    use super::GaloisLfsr;

    #[test]
    fn galois_lfsr_produces_expected_state_and_bits() {
        let mut lfsr = GaloisLfsr::new(0b1010110011100001, 0b1011010000000000);
        assert_eq!(1, lfsr.next());
        assert_eq!(0xe270, lfsr.state());

        let mut lfsr = GaloisLfsr::new(0b11111, 0b11110);
        let bits = (0..64)
            .map(|_| char::from(b'0' + lfsr.next()))
            .collect::<String>();

        assert_eq!(
            "1101111100100110000101101010001110111110010011000010110101000111",
            bits
        );
    }
}
