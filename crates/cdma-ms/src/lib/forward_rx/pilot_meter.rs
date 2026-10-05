use std::sync::Arc;

use cdma_bts::receiver::pipelined::{PipelineProcessor, SampleBlock};
use num_complex::Complex32;

use super::Measurements;

const CHIPS_PER_SYMBOL: usize = 64;
const SYMBOLS_PER_REPORT: usize = 384;
const REPORT_SMOOTHING: f64 = 0.25;
const CARRIER_SMOOTHING: f32 = 0.02;
const CHIP_RATE_HZ: f32 = 1_228_800.0;

pub(crate) struct PilotMeter {
    measurements: Arc<Measurements>,
    carry: Vec<Complex32>,
    coherent_sum: f64,
    total_sum: f64,
    symbols: usize,
    smoothed_ratio: Option<f64>,
    previous_symbol: Option<Complex32>,
    carrier_cross: Complex32,
    smoothed_carrier_hz: Option<f32>,
}

impl PilotMeter {
    pub(crate) fn new(measurements: Arc<Measurements>) -> Self {
        PilotMeter {
            measurements,
            carry: Vec::with_capacity(CHIPS_PER_SYMBOL),
            coherent_sum: 0.0,
            total_sum: 0.0,
            symbols: 0,
            smoothed_ratio: None,
            previous_symbol: None,
            carrier_cross: Complex32::default(),
            smoothed_carrier_hz: None,
        }
    }

    fn symbol(&mut self, chips: &[Complex32]) {
        let mut sum = Complex32::new(0.0, 0.0);
        let mut energy = 0.0f32;
        for c in chips {
            sum += *c;
            energy += c.norm_sqr();
        }
        self.coherent_sum += sum.norm_sqr() as f64 / CHIPS_PER_SYMBOL as f64;
        self.total_sum += energy as f64;
        if let Some(previous) = self.previous_symbol {
            self.carrier_cross += sum * previous.conj();
        }
        self.previous_symbol = Some(sum);
        self.symbols += 1;
        if self.symbols >= SYMBOLS_PER_REPORT {
            self.publish();
        }
    }

    fn publish(&mut self) {
        if self.carrier_cross.norm_sqr() > 0.0 {
            let measured_hz = self.carrier_cross.arg() * CHIP_RATE_HZ
                / (std::f32::consts::TAU * CHIPS_PER_SYMBOL as f32);
            let carrier_hz = self.smoothed_carrier_hz.map_or(measured_hz, |previous| {
                previous + CARRIER_SMOOTHING * (measured_hz - previous)
            });
            self.smoothed_carrier_hz = Some(carrier_hz);
            self.measurements.set_carrier_offset_hz(carrier_hz);
            log::debug!("ms pilot: carrier_offset_hz={carrier_hz:.2}");
        }
        self.carrier_cross = Complex32::default();
        if self.total_sum > 0.0 {
            let ratio = (self.coherent_sum / self.total_sum).clamp(1e-9, 1.0);
            let smoothed = match self.smoothed_ratio {
                Some(prev) => prev + REPORT_SMOOTHING * (ratio - prev),
                None => ratio,
            };
            self.smoothed_ratio = Some(smoothed);
            self.measurements
                .set_ec_io_db((10.0 * smoothed.log10()) as f32);
            self.measurements.add_pilot_symbols(self.symbols as u64);
        }
        self.coherent_sum = 0.0;
        self.total_sum = 0.0;
        self.symbols = 0;
    }
}

impl PipelineProcessor for PilotMeter {
    fn process_block(&mut self, block: SampleBlock) -> Vec<SampleBlock> {
        let mut input: &[Complex32] = &block.samples;
        if !self.carry.is_empty() {
            let need = CHIPS_PER_SYMBOL - self.carry.len();
            let take = need.min(input.len());
            self.carry.extend_from_slice(&input[..take]);
            input = &input[take..];
            if self.carry.len() == CHIPS_PER_SYMBOL {
                let full = std::mem::take(&mut self.carry);
                self.symbol(&full);
            }
        }
        let mut chunks = input.chunks_exact(CHIPS_PER_SYMBOL);
        for chunk in &mut chunks {
            self.symbol(chunk);
        }
        self.carry.extend_from_slice(chunks.remainder());
        vec![block]
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pilot_carrier_measurement_recovers_signed_frequency_offset() {
        for frequency_hz in [-375.0, 325.0] {
            let measurements = Arc::new(Measurements::default());
            let mut meter = PilotMeter::new(measurements.clone());
            for symbol in 0..SYMBOLS_PER_REPORT * 2 {
                let chips: Vec<_> = (0..CHIPS_PER_SYMBOL)
                    .map(|chip| {
                        let phase = std::f32::consts::TAU
                            * frequency_hz
                            * (symbol * CHIPS_PER_SYMBOL + chip) as f32
                            / CHIP_RATE_HZ;
                        Complex32::from_polar(1.0, phase)
                    })
                    .collect();
                meter.symbol(&chips);
            }
            assert!((measurements.carrier_offset_hz() - frequency_hz as f64).abs() < 0.1);
        }
    }

    fn walsh_1(i: usize) -> f32 {
        if i % 2 == 0 { 1.0 } else { -1.0 }
    }

    #[test]
    fn ec_io_is_the_pilot_share_of_the_carrier() {
        let m = Arc::new(Measurements::default());
        let mut meter = PilotMeter::new(m.clone());
        let chips: Vec<Complex32> = (0..CHIPS_PER_SYMBOL * SYMBOLS_PER_REPORT)
            .map(|n| Complex32::new(1.0 + 2.0 * walsh_1(n % CHIPS_PER_SYMBOL), 0.0))
            .collect();
        meter.process_block(SampleBlock::new(chips, 0));
        let ec_io = m.ec_io_db();
        assert!((ec_io + 6.99).abs() < 0.05, "Ec/Io {ec_io} dB");
        assert_eq!(m.pilot_symbols(), SYMBOLS_PER_REPORT as u64);
    }

    #[test]
    fn symbols_split_across_blocks_are_reassembled() {
        let m = Arc::new(Measurements::default());
        let mut meter = PilotMeter::new(m.clone());
        let chips: Vec<Complex32> = (0..CHIPS_PER_SYMBOL * SYMBOLS_PER_REPORT)
            .map(|_| Complex32::new(1.0, 0.0))
            .collect();
        for chunk in chips.chunks(100) {
            meter.process_block(SampleBlock::new(chunk.to_vec(), 0));
        }
        assert!(m.ec_io_db().abs() < 0.01, "pure pilot should read 0 dB");
    }
}
