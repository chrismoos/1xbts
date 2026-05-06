use std::collections::HashMap;
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use num_complex::Complex32;

use crate::phy::walsh::WalshDecoder;

use super::{PipelineProcessor, SampleBlock};

/// Walsh decoder with pilot combining.
pub struct WalshPilotCombiner {
    channel_walsh: WalshDecoder,
    pilot_walsh: WalshDecoder,
    buffer: Vec<Complex32>,
    buffer_tags: HashMap<&'static str, i64>,
    buffer_chip_start: usize,
    buffer_sample_rate_hz: f64,
    absolute_chip_modulus: Option<usize>,
    initial_chip_discard: usize,
    dump_wav_path: Option<String>,
    dump_writer: Option<hound::WavWriter<BufWriter<File>>>,
}

impl WalshPilotCombiner {
    pub fn new(channel_walsh: WalshDecoder, pilot_walsh: WalshDecoder) -> Self {
        Self {
            channel_walsh,
            pilot_walsh,
            buffer: Vec::new(),
            buffer_tags: HashMap::new(),
            buffer_chip_start: 0,
            buffer_sample_rate_hz: 0.0,
            absolute_chip_modulus: None,
            initial_chip_discard: 0,
            dump_wav_path: std::env::var("CDMA_WPC_DUMP_WAV").ok(),
            dump_writer: None,
        }
    }

    pub fn with_absolute_chip_modulus(mut self, modulus: usize) -> Self {
        self.absolute_chip_modulus = Some(modulus.max(1));
        self
    }

    pub fn with_initial_chip_discard(mut self, chips: usize) -> Self {
        self.initial_chip_discard = chips;
        self
    }

    pub fn with_wav_dump(mut self, path: &str) -> Self {
        self.dump_wav_path = Some(path.to_string());
        self
    }

    fn ensure_dump_writer(&mut self, sample_rate_hz: f64) {
        if self.dump_writer.is_some() {
            return;
        }
        let Some(path) = &self.dump_wav_path else {
            return;
        };
        if sample_rate_hz <= 0.0 {
            return;
        }

        if let Some(parent) = Path::new(path).parent() {
            if let Err(e) = std::fs::create_dir_all(parent) {
                eprintln!("walsh_pilot_combiner: failed to create dump dir {parent:?}: {e}");
                self.dump_wav_path = None;
                return;
            }
        }

        let sample_rate = sample_rate_hz.round().max(1.0) as u32;
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        match hound::WavWriter::create(path, spec) {
            Ok(writer) => {
                self.dump_writer = Some(writer);
            }
            Err(e) => {
                eprintln!("walsh_pilot_combiner: failed to create dump wav {path}: {e}");
                self.dump_wav_path = None;
            }
        }
    }

    fn dump_samples(&mut self, samples: &[Complex32], sample_rate_hz: f64) {
        self.ensure_dump_writer(sample_rate_hz);
        let Some(writer) = self.dump_writer.as_mut() else {
            return;
        };
        for s in samples {
            if let Err(e) = writer.write_sample(s.re) {
                eprintln!("walsh_pilot_combiner: failed writing I sample: {e}");
                self.dump_writer = None;
                self.dump_wav_path = None;
                return;
            }
            if let Err(e) = writer.write_sample(s.im) {
                eprintln!("walsh_pilot_combiner: failed writing Q sample: {e}");
                self.dump_writer = None;
                self.dump_wav_path = None;
                return;
            }
        }
    }
}

impl PipelineProcessor for WalshPilotCombiner {
    fn process_block(&mut self, block: SampleBlock) -> Vec<SampleBlock> {
        if self.buffer.is_empty() {
            self.buffer_tags = block.tags.clone();
            self.buffer_chip_start = block.chip_start;
            self.buffer_sample_rate_hz = block.sample_rate_hz;
        }
        self.buffer.extend_from_slice(&block.samples);

        let mut chips_to_discard = 0usize;
        if let Some(modulus) = self.absolute_chip_modulus {
            let remainder = self.buffer_chip_start % modulus;
            if remainder != 0 {
                chips_to_discard += modulus - remainder;
            }
        }
        chips_to_discard += self.initial_chip_discard;

        let drop_now = chips_to_discard.min(self.buffer.len());
        if drop_now > 0 {
            self.buffer.drain(..drop_now);
            self.buffer_chip_start = self.buffer_chip_start.saturating_add(drop_now);
            self.initial_chip_discard = self.initial_chip_discard.saturating_sub(drop_now);
        }

        if self.buffer.len() < 64 {
            if !self.buffer.is_empty() {
                self.buffer_tags = block.tags;
            }
            return Vec::new();
        }

        let mut out_samples = Vec::new();
        let mut pilot_energy_sum = 0.0f64;
        let mut sym_count = 0usize;
        let out_chip_start = self.buffer_chip_start;
        while self.buffer.len() >= 64 {
            let chunk = self.buffer.drain(..64).collect::<Vec<_>>();
            let channel = self.channel_walsh.process_symbol(&chunk);
            let pilot = self.pilot_walsh.process_symbol(&chunk);
            pilot_energy_sum += (pilot.re * pilot.re + pilot.im * pilot.im) as f64;
            let combined = Complex32::new((channel.re * pilot.re) + (channel.im * pilot.im), 0.0);
            sym_count += 1;
            out_samples.push(combined);
            self.buffer_chip_start = self.buffer_chip_start.saturating_add(64);
        }

        if out_samples.is_empty() {
            return Vec::new();
        }

        let out_rate = if self.buffer_sample_rate_hz > 0.0 {
            self.buffer_sample_rate_hz / 64.0
        } else {
            0.0
        };
        self.dump_samples(&out_samples, out_rate);
        let mut out_block =
            SampleBlock::new(out_samples, out_chip_start).with_sample_rate_hz(out_rate);
        out_block.tags = self.buffer_tags.clone();
        out_block
            .tags
            .insert("global_chip_start", out_chip_start as i64);
        out_block
            .tags
            .insert("walsh_phase", (out_chip_start % 64) as i64);

        if sym_count > 0 {
            let avg_pilot_energy = pilot_energy_sum / sym_count as f64;
            out_block
                .tags
                .insert("pilot_energy_x1000", (avg_pilot_energy * 1000.0) as i64);
            let combined_energy: f64 = out_block
                .samples
                .iter()
                .map(|s| (s.re * s.re) as f64)
                .sum::<f64>()
                / sym_count as f64;
            out_block
                .tags
                .insert("combined_energy_x1000", (combined_energy * 1000.0) as i64);
        }

        if !self.buffer.is_empty() {
            self.buffer_tags = block.tags;
        }
        vec![out_block]
    }
}

impl Drop for WalshPilotCombiner {
    fn drop(&mut self) {
        if let Some(writer) = self.dump_writer.take() {
            if let Err(e) = writer.finalize() {
                eprintln!("walsh_pilot_combiner: failed to finalize wav dump: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use num_complex::Complex32;

    use super::WalshPilotCombiner;
    use crate::{
        phy::walsh::{WalshDecoder, WalshGenerator},
        receiver::pipelined::{PipelineProcessor, SampleBlock},
    };

    #[test]
    fn test_walsh_pilot_combiner_outputs_one_symbol_per_64_chips() {
        let mut p = WalshPilotCombiner::new(WalshDecoder::new::<64>(0), WalshDecoder::new::<64>(0));
        let walsh0 = WalshGenerator::generate_matrix::<64>()[0];
        let mut samples = Vec::new();
        for _ in 0..3usize {
            for chip in walsh0 {
                samples.push(Complex32::new(chip as f32, 0.0));
            }
        }
        let block = SampleBlock::new(samples, 0).with_sample_rate_hz(1_228_800.0);
        let out = p.process_block(block);
        assert_eq!(1, out.len());
        assert_eq!(3, out[0].len());
        assert!(out[0].samples.iter().all(|s| s.re > 0.9));
    }

    #[test]
    fn test_walsh_pilot_combiner_respects_absolute_modulus_alignment() {
        let mut p = WalshPilotCombiner::new(WalshDecoder::new::<64>(0), WalshDecoder::new::<64>(0))
            .with_absolute_chip_modulus(64);
        let walsh0 = WalshGenerator::generate_matrix::<64>()[0];
        let mut samples = Vec::new();
        // Misaligned start (+63 chips) then one full 64-chip aligned symbol.
        for _ in 0..63 {
            samples.push(Complex32::new(0.0, 0.0));
        }
        for chip in walsh0 {
            samples.push(Complex32::new(chip as f32, 0.0));
        }

        let block = SampleBlock::new(samples, 1).with_sample_rate_hz(1_228_800.0);
        let out = p.process_block(block);
        assert_eq!(1, out.len());
        assert_eq!(1, out[0].len());
        assert_eq!(0, out[0].chip_start % 64);
    }
}
