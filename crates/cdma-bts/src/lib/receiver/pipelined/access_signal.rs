//! Per-probe signal quality for the reverse access channel.
//!
//! The preamble is all-zero frames, so every Walsh symbol is row 0 and the
//! other 63 rows of the Walsh transform hold only noise. Each 256-chip Walsh
//! symbol spans one 4800 bps information bit, so row-0 energy over the mean
//! noise row is Eb/Nt directly.

use crate::phy::coding::block_interleaver::{BitReversalInterleaver, SR1_PARAMS_576};
use crate::phy::coding::convolutional::get_1_3_k9_encoder;
use cdma_common::consts::{
    RC1_PN_CHIPS_PER_WALSH_CHIP, RC1_SYMBOLS_PER_FRAME, RC1_WALSH_CHIPS_PER_SYMBOL,
};

use super::mobile_power_dbfs_from_despread_chip_power;

const PN_CHIPS_PER_SYMBOL: f64 = (RC1_PN_CHIPS_PER_WALSH_CHIP * RC1_WALSH_CHIPS_PER_SYMBOL) as f64;
const NOISE_ROWS: f32 = (RC1_WALSH_CHIPS_PER_SYMBOL - 1) as f32;
/// Fewer symbols than a quarter frame give a reading too noisy to show.
const MIN_PREAMBLE_SYMBOLS: usize = RC1_SYMBOLS_PER_FRAME / 4;
/// Each access channel code symbol is sent twice at 4800 bps.
const CODE_SYMBOL_REPEATS: usize = 2;

#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct WalshSymbolPower {
    pub w0: f32,
    pub noise: f32,
}

impl WalshSymbolPower {
    pub fn from_energies(energies: &[f32; RC1_WALSH_CHIPS_PER_SYMBOL]) -> Self {
        let noise_total: f32 = energies[1..].iter().sum();
        Self {
            w0: energies[0],
            noise: noise_total / NOISE_ROWS,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub(crate) struct PreambleMeasurement {
    pub eb_nt_db: f32,
    pub signal_power_dbfs: f32,
}

#[derive(Clone, Copy, Debug, Default)]
pub(crate) struct PreamblePowerAccumulator {
    w0_sum: f64,
    noise_sum: f64,
    symbols: usize,
}

impl PreamblePowerAccumulator {
    pub fn add(&mut self, power: WalshSymbolPower) {
        self.w0_sum += power.w0 as f64;
        self.noise_sum += power.noise as f64;
        self.symbols += 1;
    }

    pub fn is_empty(&self) -> bool {
        self.symbols == 0
    }

    pub fn symbols(&self) -> usize {
        self.symbols
    }

    pub fn clear(&mut self) {
        *self = Self::default();
    }

    /// Returns `None` below `MIN_PREAMBLE_SYMBOLS` or when the noise rows
    /// hold at least as much energy as row 0.
    pub fn measure(&self) -> Option<PreambleMeasurement> {
        if self.symbols < MIN_PREAMBLE_SYMBOLS || self.noise_sum <= 0.0 {
            return None;
        }
        let n = self.symbols as f64;
        let noise = self.noise_sum / n;
        let signal = self.w0_sum / n - noise;
        if signal <= 0.0 {
            return None;
        }
        let chip_power = signal / (PN_CHIPS_PER_SYMBOL * PN_CHIPS_PER_SYMBOL);
        Some(PreambleMeasurement {
            eb_nt_db: (10.0 * (signal / noise).log10()) as f32,
            signal_power_dbfs: mobile_power_dbfs_from_despread_chip_power(chip_power),
        })
    }
}

/// Counts code symbols in one 576-symbol interleaved frame whose hard decision
/// differs from the re-encoded `decoded_bits`. A positive soft value is a 0.
pub(crate) fn code_symbol_errors(interleaved_soft: &[f32], decoded_bits: &[u8]) -> usize {
    let deinterleaved = BitReversalInterleaver::new(SR1_PARAMS_576).decode_soft(interleaved_soft);
    let mut encoder = get_1_3_k9_encoder();
    decoded_bits
        .iter()
        .flat_map(|&bit| encoder.encode(bit))
        .flat_map(|code_symbol| std::iter::repeat_n(code_symbol, CODE_SYMBOL_REPEATS))
        .zip(deinterleaved)
        .filter(|&(sent, soft)| (soft < 0.0) != (sent == 1))
        .count()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::phy::walsh::WalshGenerator;
    use num_complex::Complex32;
    use rand::rngs::StdRng;
    use rand::{Rng, SeedableRng};

    fn gaussian(rng: &mut StdRng) -> f32 {
        let u1: f32 = rng.gen_range(f32::EPSILON..1.0);
        let u2: f32 = rng.r#gen();
        (-2.0 * u1.ln()).sqrt() * (2.0 * std::f32::consts::PI * u2).cos()
    }

    fn preamble_symbol_power(
        rng: &mut StdRng,
        amplitude: f32,
        noise_sigma: f32,
    ) -> WalshSymbolPower {
        let mut walsh_chips = [Complex32::new(0.0, 0.0); RC1_WALSH_CHIPS_PER_SYMBOL];
        for chip in walsh_chips.iter_mut() {
            for _ in 0..RC1_PN_CHIPS_PER_WALSH_CHIP {
                *chip += Complex32::new(
                    amplitude + noise_sigma * gaussian(rng),
                    noise_sigma * gaussian(rng),
                );
            }
        }
        WalshGenerator::fwht_fixed(&mut walsh_chips);
        WalshSymbolPower::from_energies(&std::array::from_fn(|i| walsh_chips[i].norm_sqr()))
    }

    #[test]
    fn preamble_eb_nt_matches_the_injected_level() {
        let mut rng = StdRng::seed_from_u64(7);
        for target_db in [3.0f32, 8.0, 15.0] {
            // Per-chip Ec = a², Nt = 2σ², Eb = 256·Ec.
            let noise_sigma = 1.0f32;
            let eb_nt = 10f32.powf(target_db / 10.0);
            let amplitude = (eb_nt * 2.0 * noise_sigma * noise_sigma / 256.0).sqrt();
            let mut acc = PreamblePowerAccumulator::default();
            for _ in 0..RC1_SYMBOLS_PER_FRAME * 5 {
                acc.add(preamble_symbol_power(&mut rng, amplitude, noise_sigma));
            }
            let measured = acc.measure().expect("measurement").eb_nt_db;
            assert!(
                (measured - target_db).abs() < 0.5,
                "target {target_db} dB measured {measured} dB"
            );
        }
    }

    #[test]
    fn short_or_noise_only_preamble_has_no_measurement() {
        let mut rng = StdRng::seed_from_u64(9);
        let mut acc = PreamblePowerAccumulator::default();
        for _ in 0..MIN_PREAMBLE_SYMBOLS - 1 {
            acc.add(preamble_symbol_power(&mut rng, 1.0, 1.0));
        }
        assert!(acc.measure().is_none());

        acc.clear();
        acc.add(WalshSymbolPower {
            w0: 1.0,
            noise: 2.0,
        });
        for _ in 0..MIN_PREAMBLE_SYMBOLS {
            acc.add(WalshSymbolPower {
                w0: 1.0,
                noise: 1.0,
            });
        }
        assert!(acc.measure().is_none());
    }

    #[test]
    fn code_symbol_errors_counts_flipped_decisions() {
        let mut rng = StdRng::seed_from_u64(11);
        let mut bits: Vec<u8> = (0..88).map(|_| rng.gen_range(0..2)).collect();
        bits.extend([0u8; 8]);

        let mut encoder = get_1_3_k9_encoder();
        let repeated: Vec<u8> = bits
            .iter()
            .flat_map(|&bit| encoder.encode(bit))
            .flat_map(|s| [s, s])
            .collect();
        let interleaved = BitReversalInterleaver::new(SR1_PARAMS_576).encode(&repeated);
        let mut soft: Vec<f32> = interleaved
            .iter()
            .map(|&s| if s == 0 { 1.0 } else { -1.0 })
            .collect();
        assert_eq!(code_symbol_errors(&soft, &bits), 0);

        for idx in [0usize, 17, 300, 575] {
            soft[idx] = -soft[idx];
        }
        assert_eq!(code_symbol_errors(&soft, &bits), 4);
    }
}
