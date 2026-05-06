use num_complex::Complex32;

use crate::phy::walsh::WalshGenerator;
use log::info;

use super::{PipelineProcessor, SampleBlock};
use cdma_common::consts::{RC1_PN_CHIPS_PER_WALSH_CHIP, RC1_WALSH_CHIPS_PER_SYMBOL};

/// Reverse-link Access Channel orthogonal demodulator.
///
/// Input: PN-rate despread+LC-removed samples at 1.2288 Mcps.
///
/// Per C.S0002-E Table 2.1.3.1.2.1-1, each 64-ary Walsh modulation symbol
/// occupies 256 PN chips (64 Walsh chips x 4 PN chips/Walsh chip). This
/// processor accumulates 256 PN-rate samples per symbol, sums groups of 4
/// to recover the 64 Walsh-rate chips, then correlates against the 64-point
/// Walsh matrix to produce 6 soft bits per symbol.
///
/// Detection is noncoherent: uses |corr|^2 (energy) rather than signed
/// real part, because the reverse access channel has no dedicated pilot and
/// the carrier phase is arbitrary.
///
/// Bit ordering follows C.S0002-E 2.1.3.1.13.1:
///   index = c0 + 2*c1 + 4*c2 + 8*c3 + 16*c4 + 32*c5
/// Output order is [c0, c1, c2, c3, c4, c5] to match the
/// deinterleaver's expected symbol ordering.
pub struct ReverseAccessOrthogonalDemodProcessor {
    buffer: Vec<Complex32>,
    buffer_tags: std::collections::HashMap<&'static str, i64>,
    buffer_chip_start: usize,
    debug_lc_symbol_logs: usize,
}

const PN_CHIPS_PER_SYMBOL: usize = RC1_PN_CHIPS_PER_WALSH_CHIP * RC1_WALSH_CHIPS_PER_SYMBOL;

impl ReverseAccessOrthogonalDemodProcessor {
    pub fn new() -> Self {
        Self {
            buffer: Vec::new(),
            buffer_tags: std::collections::HashMap::new(),
            buffer_chip_start: 0,
            debug_lc_symbol_logs: 0,
        }
    }

    fn despread_to_walsh_chips(
        pn_samples: &[Complex32],
    ) -> [Complex32; RC1_WALSH_CHIPS_PER_SYMBOL] {
        debug_assert_eq!(pn_samples.len(), PN_CHIPS_PER_SYMBOL);
        let mut walsh_chips = [Complex32::new(0.0, 0.0); RC1_WALSH_CHIPS_PER_SYMBOL];
        for (w, chunk) in walsh_chips
            .iter_mut()
            .zip(pn_samples.chunks_exact(RC1_PN_CHIPS_PER_WALSH_CHIP))
        {
            *w = chunk.iter().copied().sum();
        }
        walsh_chips
    }

    fn symbol_energies(walsh_chips: &[Complex32; RC1_WALSH_CHIPS_PER_SYMBOL]) -> [f32; 64] {
        let mut chips = *walsh_chips;
        WalshGenerator::fwht_fixed(&mut chips);
        std::array::from_fn(|i| chips[i].norm_sqr())
    }

    fn soft_bits_from_energies(energies: &[f32; 64]) -> [Complex32; 6] {
        let mut out = [Complex32::new(0.0, 0.0); 6];
        for bit_idx in 0..6usize {
            let bit_shift = bit_idx;
            let mut max_zero = f32::NEG_INFINITY;
            let mut max_one = f32::NEG_INFINITY;
            for (symbol, &energy) in energies.iter().enumerate() {
                if ((symbol >> bit_shift) & 1) == 0 {
                    max_zero = max_zero.max(energy);
                } else {
                    max_one = max_one.max(energy);
                }
            }
            out[bit_idx] = Complex32::new(max_zero - max_one, 0.0);
        }
        out
    }
}

impl PipelineProcessor for ReverseAccessOrthogonalDemodProcessor {
    fn process_block(&mut self, block: SampleBlock) -> Vec<SampleBlock> {
        if self.buffer.is_empty() {
            self.buffer_tags = block.tags.clone();
            self.buffer_chip_start = block.chip_start;
        }
        self.buffer.extend_from_slice(&block.samples);

        let full_symbols = self.buffer.len() / PN_CHIPS_PER_SYMBOL;
        let mut soft_bits = Vec::with_capacity(full_symbols * 6);
        for s in 0..full_symbols {
            let start = s * PN_CHIPS_PER_SYMBOL;
            let walsh_chips =
                Self::despread_to_walsh_chips(&self.buffer[start..start + PN_CHIPS_PER_SYMBOL]);
            let energies = Self::symbol_energies(&walsh_chips);
            if self.buffer_tags.get("reverse_access_lc_acquired") == Some(&1)
                && self.debug_lc_symbol_logs < 16
            {
                let symbol_chip = self.buffer_chip_start + start;
                let (best_row, best_energy) = energies
                    .iter()
                    .copied()
                    .enumerate()
                    .max_by(|a, b| a.1.total_cmp(&b.1))
                    .unwrap_or((0, 0.0));
                let mut second_best = f32::NEG_INFINITY;
                for (idx, &e) in energies.iter().enumerate() {
                    if idx != best_row {
                        second_best = second_best.max(e);
                    }
                }
                info!(
                    "access_orth_demod: chip={} best_row={} best_energy={:.1} margin={:.1}",
                    symbol_chip,
                    best_row,
                    best_energy,
                    best_energy - second_best,
                );
                self.debug_lc_symbol_logs = self.debug_lc_symbol_logs.saturating_add(1);
            }
            soft_bits.extend(Self::soft_bits_from_energies(&energies));
        }
        if full_symbols > 0 {
            self.buffer.drain(..full_symbols * PN_CHIPS_PER_SYMBOL);
            self.buffer_chip_start = self
                .buffer_chip_start
                .saturating_add(full_symbols * PN_CHIPS_PER_SYMBOL);
        }

        if soft_bits.is_empty() {
            return Vec::new();
        }

        let mut out = SampleBlock::new(
            soft_bits,
            self.buffer_chip_start.saturating_sub(
                (self.buffer_chip_start % PN_CHIPS_PER_SYMBOL).min(PN_CHIPS_PER_SYMBOL),
            ),
        )
        .with_sample_rate_hz(0.0);
        out.tags = self.buffer_tags.clone();
        vec![out]
    }

    fn name(&self) -> &'static str {
        "ReverseAccessOrthogonalDemodProcessor"
    }
}

#[cfg(test)]
mod tests {
    use num_complex::Complex32;

    use super::{RC1_PN_CHIPS_PER_WALSH_CHIP, ReverseAccessOrthogonalDemodProcessor};
    use crate::phy::walsh::WalshGenerator;
    use crate::receiver::pipelined::{PipelineProcessor, SampleBlock};

    #[test]
    fn test_reverse_access_orthogonal_demod_outputs_expected_soft_bits() {
        let mut p = ReverseAccessOrthogonalDemodProcessor::new();
        let idx = 42usize;
        let walsh = WalshGenerator::generate_matrix::<64>()[idx];
        let samples: Vec<Complex32> = walsh
            .iter()
            .flat_map(|chip| {
                std::iter::repeat(Complex32::new(*chip as f32, 0.0))
                    .take(RC1_PN_CHIPS_PER_WALSH_CHIP)
            })
            .collect();
        assert_eq!(256, samples.len());

        let out = p.process_block(SampleBlock::new(samples, 0));
        assert_eq!(1, out.len());
        assert_eq!(6, out[0].samples.len());

        let bits = out[0]
            .samples
            .iter()
            .map(|s| if s.re >= 0.0 { 0u8 } else { 1u8 })
            .collect::<Vec<_>>();
        assert_eq!(vec![0, 1, 0, 1, 0, 1], bits);
    }

    #[test]
    fn test_noncoherent_demod_works_with_phase_rotation() {
        let mut p = ReverseAccessOrthogonalDemodProcessor::new();
        let idx = 42usize;
        let walsh = WalshGenerator::generate_matrix::<64>()[idx];
        let phase = std::f32::consts::FRAC_PI_4;
        let (sin_p, cos_p) = phase.sin_cos();
        let samples: Vec<Complex32> = walsh
            .iter()
            .flat_map(|chip| {
                let c = *chip as f32;
                std::iter::repeat(Complex32::new(c * cos_p, c * sin_p))
                    .take(RC1_PN_CHIPS_PER_WALSH_CHIP)
            })
            .collect();

        let out = p.process_block(SampleBlock::new(samples, 0));
        assert_eq!(1, out.len());
        assert_eq!(6, out[0].samples.len());

        let bits = out[0]
            .samples
            .iter()
            .map(|s| if s.re >= 0.0 { 0u8 } else { 1u8 })
            .collect::<Vec<_>>();
        assert_eq!(vec![0, 1, 0, 1, 0, 1], bits);
    }
}
