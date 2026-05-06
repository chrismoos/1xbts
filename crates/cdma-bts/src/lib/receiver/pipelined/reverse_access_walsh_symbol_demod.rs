use num_complex::Complex32;

use super::{PipelineProcessor, SampleBlock};
use crate::phy::walsh::WalshGenerator;
use cdma_common::consts::{
    RC1_PN_CHIPS_PER_WALSH_CHIP, RC1_SOFT_BITS_PER_SYMBOL, RC1_WALSH_CHIPS_PER_SYMBOL,
};
use std::array;

const PN_CHIPS_PER_SYMBOL: usize = RC1_PN_CHIPS_PER_WALSH_CHIP * RC1_WALSH_CHIPS_PER_SYMBOL;
const DEFAULT_OUTPUT_BITS: usize = 576;

pub struct ReverseAccessWalshSymbolDemodProcessor {
    output_bits: usize,
    soft_buf: Vec<f32>,
    buffer_tags: std::collections::HashMap<&'static str, i64>,
    buffer_chip_start: usize,
    buffer_sample_rate_hz: f64,
}

impl ReverseAccessWalshSymbolDemodProcessor {
    pub fn new() -> Self {
        Self::with_output_bits(DEFAULT_OUTPUT_BITS)
    }

    pub fn with_output_bits(output_bits: usize) -> Self {
        assert!(output_bits > 0, "output_bits must be > 0");
        assert_eq!(
            0,
            output_bits % RC1_SOFT_BITS_PER_SYMBOL,
            "output_bits must be a multiple of 6"
        );
        Self {
            output_bits,
            soft_buf: Vec::new(),
            buffer_tags: std::collections::HashMap::new(),
            buffer_chip_start: 0,
            buffer_sample_rate_hz: 0.0,
        }
    }

    fn block_oversample(block: &SampleBlock) -> usize {
        block
            .tags
            .get("access_oversample")
            .copied()
            .map(|v| v.max(1) as usize)
            .unwrap_or_else(|| (block.samples.len() / PN_CHIPS_PER_SYMBOL).max(1))
    }

    fn symbol_energies(
        &self,
        chips: &[Complex32],
        oversample: usize,
        phase: usize,
    ) -> [f32; RC1_WALSH_CHIPS_PER_SYMBOL] {
        debug_assert_eq!(chips.len(), PN_CHIPS_PER_SYMBOL * oversample);

        let mut walsh_chips = [Complex32::new(0.0, 0.0); RC1_WALSH_CHIPS_PER_SYMBOL];
        for walsh_chip_idx in 0..RC1_WALSH_CHIPS_PER_SYMBOL {
            let mut acc = Complex32::new(0.0, 0.0);
            let base = walsh_chip_idx * RC1_PN_CHIPS_PER_WALSH_CHIP * oversample;
            for pn in 0..RC1_PN_CHIPS_PER_WALSH_CHIP {
                acc += chips[base + pn * oversample + phase];
            }
            walsh_chips[walsh_chip_idx] = acc;
        }

        WalshGenerator::fwht_fixed(&mut walsh_chips);
        array::from_fn(|i| walsh_chips[i].norm_sqr())
    }

    fn soft_bits_from_energies(
        energies: &[f32; RC1_WALSH_CHIPS_PER_SYMBOL],
    ) -> [f32; RC1_SOFT_BITS_PER_SYMBOL] {
        let mut out = [0.0f32; RC1_SOFT_BITS_PER_SYMBOL];
        for bit in 0..RC1_SOFT_BITS_PER_SYMBOL {
            let mut max_zero = f32::NEG_INFINITY;
            let mut max_one = f32::NEG_INFINITY;
            for (row, &energy) in energies.iter().enumerate() {
                if ((row >> bit) & 1) == 0 {
                    max_zero = max_zero.max(energy);
                } else {
                    max_one = max_one.max(energy);
                }
            }
            out[bit] = max_zero - max_one;
        }
        out
    }

    fn peak_metrics(energies: &[f32; RC1_WALSH_CHIPS_PER_SYMBOL]) -> (usize, f32, f32) {
        let total_energy: f32 = energies.iter().sum();
        if total_energy <= 1e-9 {
            return (0, 0.0, 0.0);
        }

        let mut best_row = 0usize;
        let mut best_energy = energies[0];
        let mut second_energy = 0.0f32;
        for (row, &energy) in energies.iter().enumerate().skip(1) {
            if energy > best_energy {
                second_energy = best_energy;
                best_energy = energy;
                best_row = row;
            } else if energy > second_energy {
                second_energy = energy;
            }
        }

        (
            best_row,
            best_energy / total_energy,
            best_energy / second_energy.max(1e-9),
        )
    }

    fn demod_symbol(
        &self,
        chips: &[Complex32],
        oversample: usize,
    ) -> ([f32; RC1_SOFT_BITS_PER_SYMBOL], usize, usize, f32, f32) {
        let mut best_phase = 0usize;
        let mut best_row = 0usize;
        let mut best_peak_ratio = f32::NEG_INFINITY;
        let mut best_margin = f32::NEG_INFINITY;
        let mut best_soft = [0.0f32; RC1_SOFT_BITS_PER_SYMBOL];

        for phase in 0..oversample {
            let energies = self.symbol_energies(chips, oversample, phase);
            let (row, peak_ratio, margin) = Self::peak_metrics(&energies);
            if peak_ratio > best_peak_ratio
                || (peak_ratio == best_peak_ratio && margin > best_margin)
            {
                best_phase = phase;
                best_row = row;
                best_peak_ratio = peak_ratio;
                best_margin = margin;
                best_soft = Self::soft_bits_from_energies(&energies);
            }
        }

        (
            best_soft,
            best_phase,
            best_row,
            best_peak_ratio,
            best_margin,
        )
    }

    fn demod_symbol_at_phase(
        &self,
        chips: &[Complex32],
        oversample: usize,
        phase: usize,
    ) -> Option<([f32; RC1_SOFT_BITS_PER_SYMBOL], usize, f32, f32)> {
        if phase >= oversample {
            return None;
        }
        let energies = self.symbol_energies(chips, oversample, phase);
        let (row, peak_ratio, margin) = Self::peak_metrics(&energies);
        Some((
            Self::soft_bits_from_energies(&energies),
            row,
            peak_ratio,
            margin,
        ))
    }

    fn emit_ready_blocks(&mut self) -> Vec<SampleBlock> {
        let mut out = Vec::new();
        while self.soft_buf.len() >= self.output_bits {
            let block_soft: Vec<Complex32> = self
                .soft_buf
                .drain(..self.output_bits)
                .map(|v| Complex32::new(v, 0.0))
                .collect();
            let mut block = SampleBlock::new(block_soft, self.buffer_chip_start)
                .with_sample_rate_hz(self.buffer_sample_rate_hz);
            block.tags = self.buffer_tags.clone();
            out.push(block);
            self.buffer_chip_start = self.buffer_chip_start.saturating_add(
                (self.output_bits / RC1_SOFT_BITS_PER_SYMBOL) * PN_CHIPS_PER_SYMBOL,
            );
        }
        out
    }
}

impl PipelineProcessor for ReverseAccessWalshSymbolDemodProcessor {
    fn process_block(&mut self, block: SampleBlock) -> Vec<SampleBlock> {
        assert_eq!(
            block.samples.len(),
            PN_CHIPS_PER_SYMBOL * Self::block_oversample(&block),
            "ReverseAccessWalshSymbolDemodProcessor expects aligned 256-chip blocks"
        );
        let oversample = Self::block_oversample(&block);

        if self.soft_buf.is_empty() {
            self.buffer_tags = block.tags.clone();
            self.buffer_chip_start = block.chip_start;
            self.buffer_sample_rate_hz = if block.sample_rate_hz > 0.0 {
                block.sample_rate_hz / block.samples.len() as f64 * RC1_SOFT_BITS_PER_SYMBOL as f64
            } else {
                0.0
            };
        }

        let fixed_phase = block
            .tags
            .get("access_fixed_phase")
            .copied()
            .and_then(|v| usize::try_from(v).ok())
            .filter(|phase| *phase < oversample);
        let (soft, phase, row, peak_ratio, margin) = match fixed_phase
            .and_then(|phase| self.demod_symbol_at_phase(&block.samples, oversample, phase))
        {
            Some((soft, row, peak_ratio, margin)) => {
                (soft, fixed_phase.unwrap(), row, peak_ratio, margin)
            }
            None => self.demod_symbol(&block.samples, oversample),
        };
        self.soft_buf.extend_from_slice(&soft);

        if !self.soft_buf.is_empty() {
            for (k, v) in block.tags {
                self.buffer_tags.insert(k, v);
            }
            self.buffer_tags
                .insert("access_selected_phase", phase as i64);
            self.buffer_tags.insert("access_selected_row", row as i64);
            self.buffer_tags.insert(
                "access_selected_peak_ratio_milli",
                (peak_ratio * 1000.0) as i64,
            );
            self.buffer_tags
                .insert("access_selected_margin_milli", (margin * 1000.0) as i64);
        }

        self.emit_ready_blocks()
    }

    fn flush(&mut self) -> Vec<SampleBlock> {
        if self.soft_buf.is_empty() {
            return Vec::new();
        }
        let mut block = SampleBlock::new(
            self.soft_buf
                .drain(..)
                .map(|v| Complex32::new(v, 0.0))
                .collect(),
            self.buffer_chip_start,
        )
        .with_sample_rate_hz(self.buffer_sample_rate_hz);
        block.tags = self.buffer_tags.clone();
        vec![block]
    }

    fn name(&self) -> &'static str {
        "ReverseAccessWalshSymbolDemodProcessor"
    }
}

#[cfg(test)]
mod tests {
    use num_complex::Complex32;

    use super::{
        PN_CHIPS_PER_SYMBOL, PipelineProcessor, ReverseAccessWalshSymbolDemodProcessor, SampleBlock,
    };
    use crate::phy::walsh::WalshGenerator;

    fn make_symbol(row: usize) -> Vec<Complex32> {
        let walsh = WalshGenerator::generate_matrix::<64>();
        let mut out = Vec::with_capacity(PN_CHIPS_PER_SYMBOL);
        for &chip in &walsh[row] {
            for _ in 0..4 {
                out.push(Complex32::new(chip as f32, 0.0));
            }
        }
        out
    }

    #[test]
    fn demod_emits_six_soft_bits_per_symbol() {
        let mut p = ReverseAccessWalshSymbolDemodProcessor::with_output_bits(6);
        let out = p.process_block(SampleBlock::new(make_symbol(37), 0));

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].samples.len(), 6);

        let hard_bits: Vec<u8> = out[0]
            .samples
            .iter()
            .map(|s| if s.re >= 0.0 { 0 } else { 1 })
            .collect();
        assert_eq!(hard_bits, vec![1, 0, 1, 0, 0, 1]);
    }

    #[test]
    fn demod_aggregates_symbols_into_configured_bit_blocks() {
        let mut p = ReverseAccessWalshSymbolDemodProcessor::with_output_bits(12);
        let mut out = Vec::new();
        out.extend(p.process_block(SampleBlock::new(make_symbol(3), 100)));
        assert!(out.is_empty());
        out.extend(p.process_block(SampleBlock::new(make_symbol(12), 356)));

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].samples.len(), 12);
        assert_eq!(out[0].chip_start, 100);
    }

    #[test]
    fn demod_flushes_partial_bit_block() {
        let mut p = ReverseAccessWalshSymbolDemodProcessor::with_output_bits(12);
        let mut out = p.process_block(SampleBlock::new(make_symbol(7), 0));
        assert!(out.is_empty());
        out.extend(p.flush());

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].samples.len(), 6);
    }
}
