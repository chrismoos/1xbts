use std::collections::{HashMap, VecDeque};
use std::fs::File;
use std::io::BufWriter;
use std::path::Path;

use num_complex::Complex32;

use crate::phy::spread::PnSequence;

use super::{PipelineProcessor, SampleBlock};

/// Matched-filter and despread processor for raw I/Q input.
///
/// This processor:
/// 1. Despreads with the short PN sequence
/// 2. Decimates to one sample per chip (adaptive phase-pick by default, optional averaging mode)
/// 3. Ensures that block outputs are of size 64 and aligned on a 64-bit boundary (based of PN sequence)
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum DecimationMode {
    PhasePick,
    Average,
}

pub struct MatchedFilterDespreader {
    oversample: usize,
    pn_period_samples: usize,
    pn: PnSequence,
    pn_phase_samples: usize,
    selected_phase: usize,
    phase_energy: Vec<f32>,
    phase_alpha: f32,
    decimation_clock: usize,
    fixed_timing_phase: Option<usize>,
    decimation_mode: DecimationMode,
    swap_iq: bool,
    use_conj_pn: bool,
    avg_accum: Complex32,
    avg_count: usize,
    output_buffer: VecDeque<Complex32>,
    output_tags: HashMap<&'static str, i64>,
    output_chip_start: usize,
    output_sample_rate_hz: f64,
    last_acq_epoch: i64,
    /// One-time initial alignment boundary (in chips). After acquisition,
    /// the first output block will be delayed until `output_chip_start`
    /// falls on a PN period boundary (derived from acquisition peak).
    /// Default is 64 (Walsh symbol boundary only). For sync channel
    /// decoding, set to 32768 (one PN period) so that Walsh repetition
    /// groups and interleaver blocks align to the TX frame boundary.
    frame_chip_alignment: usize,
    frame_aligned: bool,
    /// Global chip position of the first PN period boundary after the
    /// acquisition peak. Used to compute frame-aligned output positions.
    pn_epoch_chip: Option<usize>,
    dump_wav_path: Option<String>,
    dump_writer: Option<hound::WavWriter<BufWriter<File>>>,
}

impl MatchedFilterDespreader {
    fn env_truthy(name: &str) -> bool {
        std::env::var(name)
            .ok()
            .map(|v| {
                let s = v.trim().to_ascii_lowercase();
                s == "1" || s == "true" || s == "yes" || s == "on"
            })
            .unwrap_or(false)
    }

    pub fn new(sample_rate: u32) -> Self {
        let oversample = (sample_rate / 1_228_800).max(1) as usize;
        let decimation_mode = if Self::env_truthy("CDMA_MFD_AVG_DECIMATE") {
            DecimationMode::Average
        } else {
            DecimationMode::PhasePick
        };
        Self {
            oversample,
            pn_period_samples: 32768 * oversample,
            pn: PnSequence::new_repeat(0, 32768, oversample.saturating_sub(1)),
            pn_phase_samples: 0,
            selected_phase: 0,
            phase_energy: vec![0.0; oversample.max(1)],
            phase_alpha: 0.01,
            decimation_clock: 0,
            fixed_timing_phase: None,
            decimation_mode,
            swap_iq: false,
            use_conj_pn: false,
            avg_accum: Complex32::new(0.0, 0.0),
            avg_count: 0,
            output_buffer: VecDeque::new(),
            output_tags: HashMap::new(),
            output_chip_start: 0,
            output_sample_rate_hz: 0.0,
            last_acq_epoch: -1,
            frame_chip_alignment: 64,
            frame_aligned: false,
            pn_epoch_chip: None,
            dump_wav_path: std::env::var("CDMA_MFD_DUMP_WAV").ok(),
            dump_writer: None,
        }
    }

    pub fn with_wav_dump(mut self, path: &str) -> Self {
        self.dump_wav_path = Some(path.to_string());
        self
    }

    pub fn with_fixed_timing_phase(mut self, phase: usize) -> Self {
        self.fixed_timing_phase = Some(phase);
        self
    }

    /// Enable or disable oversample averaging decimation.
    ///
    /// Default is disabled (adaptive phase-pick).
    /// Can also be toggled at runtime via `CDMA_MFD_AVG_DECIMATE=1`.
    pub fn with_average_decimation(mut self, enabled: bool) -> Self {
        self.decimation_mode = if enabled {
            DecimationMode::Average
        } else {
            DecimationMode::PhasePick
        };
        self
    }

    pub fn with_swap_iq(mut self, enabled: bool) -> Self {
        self.swap_iq = enabled;
        self
    }

    pub fn with_conjugate_pn(mut self, enabled: bool) -> Self {
        self.use_conj_pn = enabled;
        self
    }

    /// Set the initial frame alignment boundary in chips.
    ///
    /// The first output block is delayed until `output_chip_start` is a
    /// multiple of this value.  For sync channel decoding set this to 32768
    /// (one PN period) so that the downstream unrepeater and deinterleaver
    /// start on a TX frame boundary.
    pub fn with_frame_chip_alignment(mut self, chips: usize) -> Self {
        self.frame_chip_alignment = chips.max(64);
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
                eprintln!("matched_filter_despreader: failed to create dump dir {parent:?}: {e}");
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
                eprintln!("matched_filter_despreader: failed to create dump wav {path}: {e}");
                self.dump_wav_path = None;
            }
        }
    }

    fn dump_samples(&mut self, samples: &[Complex32]) {
        self.ensure_dump_writer(self.output_sample_rate_hz);
        let Some(writer) = self.dump_writer.as_mut() else {
            return;
        };

        for s in samples {
            if let Err(e) = writer.write_sample(s.re) {
                eprintln!("matched_filter_despreader: failed writing I sample: {e}");
                self.dump_writer = None;
                self.dump_wav_path = None;
                return;
            }
            if let Err(e) = writer.write_sample(s.im) {
                eprintln!("matched_filter_despreader: failed writing Q sample: {e}");
                self.dump_writer = None;
                self.dump_wav_path = None;
                return;
            }
        }
    }

    fn reseed_pn_from_acquisition(&mut self, block: &SampleBlock) {
        let Some(&locked) = block.tags.get("acq_locked") else {
            return;
        };
        if locked != 1 {
            return;
        }
        let Some(&epoch) = block.tags.get("acq_epoch") else {
            return;
        };
        if epoch == self.last_acq_epoch {
            return;
        } else {
            eprintln!("locked epoch changed!");
        }
        let Some(&peak_sample) = block.tags.get("acq_peak_sample") else {
            return;
        };

        let peak_mod = (peak_sample.max(0) as usize) % self.pn_period_samples.max(1);
        let phase_at_block_start = if peak_mod == 0 {
            0
        } else {
            self.pn_period_samples - peak_mod
        };
        let start_chip = phase_at_block_start / self.oversample.max(1);
        let start_phase = phase_at_block_start % self.oversample.max(1);

        self.pn = PnSequence::new_repeat(0, 32768, self.oversample.saturating_sub(1));
        for _ in 0..phase_at_block_start {
            let _ = self.pn.generate_iq();
        }
        self.pn_phase_samples = phase_at_block_start % self.pn_period_samples.max(1);
        self.last_acq_epoch = epoch;
        self.frame_aligned = false;
        // Compute the global chip position of the next PN period boundary.
        // phase_at_block_start is how far the PN has advanced PAST its epoch
        // at block start.  The next epoch is (pn_period_samples - phase_at_block_start)
        // oversampled samples into the block, i.e. at sample peak_mod.
        let pn_epoch_chip = block
            .chip_start
            .saturating_add(peak_mod / self.oversample.max(1));
        self.pn_epoch_chip = Some(pn_epoch_chip);
        // Clear any pre-acquisition output (it's garbage).
        self.output_buffer.clear();
        eprintln!(
            "mfd_reseed epoch={} acq_peak_sample={} acq_peak_chip={} oversample={} pn_start_sample={} pn_start_chip={} pn_start_phase={} pn_epoch_chip={}",
            epoch,
            peak_sample,
            peak_sample.max(0) as usize / self.oversample.max(1),
            self.oversample,
            phase_at_block_start,
            start_chip,
            start_phase,
            pn_epoch_chip,
        );
    }

    fn maybe_update_selected_phase(&mut self, block: &SampleBlock) {
        if self.decimation_mode == DecimationMode::Average {
            return;
        }
        if let Some(phase) = self.fixed_timing_phase {
            self.selected_phase = phase % self.oversample.max(1);
            return;
        }

        if block.tags.get("acq_locked") == Some(&1) {
            if let Some(&phase) = block.tags.get("acq_timing_phase") {
                self.selected_phase = (phase.max(0) as usize) % self.oversample.max(1);
                return;
            }
        }

        if self.decimation_clock >= self.oversample.max(1) * 64
            && self.decimation_clock % (self.oversample.max(1) * 32) == 0
        {
            let mut best_idx = 0usize;
            let mut best_val = f32::MIN;
            for (idx, value) in self.phase_energy.iter().enumerate() {
                if *value > best_val {
                    best_val = *value;
                    best_idx = idx;
                }
            }
            self.selected_phase = best_idx;
        }
    }

    fn emit_aligned_blocks(&mut self) -> Vec<SampleBlock> {
        // One-time frame alignment: discard initial chips until we reach
        // a PN period boundary (derived from acquisition).  This ensures
        // downstream Walsh repetition groups and interleaver blocks start
        // at TX frame boundaries.
        //
        // When epoch-based alignment is active the PN boundary IS the
        // Walsh symbol boundary, so the 64-chip stream alignment below
        // must also be PN-relative (not global-chip-relative).
        if !self.frame_aligned && self.frame_chip_alignment > 64 {
            if let Some(epoch) = self.pn_epoch_chip {
                let align = self.frame_chip_alignment;
                while !self.output_buffer.is_empty() {
                    if self.output_chip_start >= epoch
                        && (self.output_chip_start - epoch) % align == 0
                    {
                        break;
                    }
                    self.output_buffer.pop_front();
                    self.output_chip_start = self.output_chip_start.saturating_add(1);
                }
            }
            if !self.output_buffer.is_empty() {
                self.frame_aligned = true;
            }
        }

        // Keep downstream Walsh decode aligned on 64-chip boundaries.
        // When epoch alignment is active, use PN-relative alignment;
        // otherwise use global chip alignment.
        let walsh_align_ref = if self.frame_chip_alignment > 64 {
            self.pn_epoch_chip.unwrap_or(0)
        } else {
            0
        };
        while !self.output_buffer.is_empty() {
            let rel = self.output_chip_start.wrapping_sub(walsh_align_ref);
            if rel % 64 == 0 {
                break;
            }
            self.output_buffer.pop_front();
            self.output_chip_start = self.output_chip_start.saturating_add(1);
        }

        let mut out = Vec::new();
        while self.output_buffer.len() >= 64 {
            let chunk: Vec<Complex32> = self.output_buffer.drain(..64).collect();
            let chip_start = self.output_chip_start;
            self.output_chip_start = self.output_chip_start.saturating_add(64);
            self.dump_samples(&chunk);

            let mut out_block =
                SampleBlock::new(chunk, chip_start).with_sample_rate_hz(self.output_sample_rate_hz);
            out_block.tags = self.output_tags.clone();
            out_block
                .tags
                .insert("global_chip_start", chip_start as i64);
            out_block
                .tags
                .insert("walsh_phase", (chip_start % 64) as i64);
            out.push(out_block);
        }
        out
    }
}

impl PipelineProcessor for MatchedFilterDespreader {
    fn process_block(&mut self, block: SampleBlock) -> Vec<SampleBlock> {
        self.reseed_pn_from_acquisition(&block);
        self.maybe_update_selected_phase(&block);

        for (idx, sample) in block.samples.iter().enumerate() {
            let pn = self.pn.generate_iq();
            let s = if self.swap_iq {
                Complex32::new(sample.im, sample.re)
            } else {
                *sample
            };
            // Forward link convention used in this project is PN_I - jPN_Q,
            // so despreading uses sample * (PN_I + jPN_Q).
            let despread = if self.use_conj_pn {
                s * pn.conj()
            } else {
                s * pn
            };

            let phase = self.decimation_clock % self.oversample.max(1);
            let energy = despread.norm_sqr();
            let prev = self.phase_energy[phase];
            self.phase_energy[phase] = (1.0 - self.phase_alpha) * prev + self.phase_alpha * energy;

            match self.decimation_mode {
                DecimationMode::PhasePick => {
                    if phase == self.selected_phase {
                        if self.output_buffer.is_empty() {
                            self.output_sample_rate_hz = if block.sample_rate_hz > 0.0 {
                                block.sample_rate_hz / self.oversample.max(1) as f64
                            } else {
                                0.0
                            };
                            self.output_tags = block.tags.clone();
                            let global_sample = block
                                .chip_start
                                .saturating_mul(self.oversample.max(1))
                                .saturating_add(idx);
                            self.output_chip_start = global_sample / self.oversample.max(1);
                        }
                        self.output_buffer.push_back(despread);
                    }
                }
                DecimationMode::Average => {
                    self.avg_accum += despread;
                    self.avg_count += 1;
                    if self.avg_count == self.oversample.max(1) {
                        if self.output_buffer.is_empty() {
                            self.output_sample_rate_hz = if block.sample_rate_hz > 0.0 {
                                block.sample_rate_hz / self.oversample.max(1) as f64
                            } else {
                                0.0
                            };
                            self.output_tags = block.tags.clone();
                            let global_sample = block
                                .chip_start
                                .saturating_mul(self.oversample.max(1))
                                .saturating_add(idx);
                            self.output_chip_start = global_sample / self.oversample.max(1);
                        }
                        self.output_buffer
                            .push_back(self.avg_accum / self.oversample.max(1) as f32);
                        self.avg_accum = Complex32::new(0.0, 0.0);
                        self.avg_count = 0;
                    }
                }
            }

            self.decimation_clock = self.decimation_clock.saturating_add(1);
            self.pn_phase_samples = (self.pn_phase_samples + 1) % self.pn_period_samples.max(1);
        }

        self.emit_aligned_blocks()
    }

    fn flush(&mut self) -> Vec<SampleBlock> {
        self.output_buffer.clear();
        self.avg_accum = Complex32::new(0.0, 0.0);
        self.avg_count = 0;
        Vec::new()
    }
}

impl Drop for MatchedFilterDespreader {
    fn drop(&mut self) {
        if let Some(writer) = self.dump_writer.take() {
            if let Err(e) = writer.finalize() {
                eprintln!("matched_filter_despreader: failed to finalize wav dump: {e}");
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::MatchedFilterDespreader;
    use num_complex::Complex32;

    use crate::{
        phy::spread::PnSequence,
        phy::walsh::WalshGenerator,
        receiver::pipelined::{PipelineProcessor, PulseMatchedFilterProcessor, SampleBlock},
        sdr::{cdma2000_baseband_filter_taps_f64, fir::ComplexFir32},
    };

    #[test]
    fn test_despreader_decimates_and_aligns_to_64_chip_blocks() {
        let sample_rate = 1_228_800u32 * 4;
        let oversample = 4usize;
        let mut p = MatchedFilterDespreader::new(sample_rate);

        let walsh0 = WalshGenerator::generate_matrix::<64>()[0];
        let mut pn = PnSequence::new_repeat(0, 32768, oversample - 1);
        let symbols = 3usize;
        let mut tx = Vec::new();
        for _ in 0..symbols {
            for chip in 0..64usize {
                let d = walsh0[chip] as f32;
                for _ in 0..oversample {
                    let pn_chip = pn.generate_iq();
                    tx.push(Complex32::new(d, 0.0) * Complex32::new(pn_chip.re, -pn_chip.im));
                }
            }
        }

        let mut block = SampleBlock::new(tx, 0).with_sample_rate_hz(sample_rate as f64);
        block.tags.insert("acq_locked", 1);
        block.tags.insert("acq_timing_phase", 0);
        block.tags.insert("acq_peak_sample", 0);
        block.tags.insert("acq_epoch", 1);
        let out = p.process_block(block);
        assert_eq!(3, out.len());
        assert!(out.iter().all(|b| b.samples.len() == 64));
        assert!(
            out.iter()
                .all(|b| b.tags.get("walsh_phase").copied() == Some(0))
        );
        assert!(out[0].sample_rate_hz > 1_200_000.0);
    }

    #[test]
    fn test_despreader_average_decimation_4x_outputs_1x() {
        let sample_rate = 1_228_800u32 * 4;
        let oversample = 4usize;
        let mut p = MatchedFilterDespreader::new(sample_rate).with_average_decimation(true);

        let walsh0 = WalshGenerator::generate_matrix::<64>()[0];
        let mut pn = PnSequence::new_repeat(0, 32768, oversample - 1);
        let symbols = 2usize;
        let mut tx = Vec::new();
        for _ in 0..symbols {
            for chip in 0..64usize {
                let d = walsh0[chip] as f32;
                for _ in 0..oversample {
                    let pn_chip = pn.generate_iq();
                    tx.push(Complex32::new(d, 0.0) * Complex32::new(pn_chip.re, -pn_chip.im));
                }
            }
        }

        let mut block = SampleBlock::new(tx, 0).with_sample_rate_hz(sample_rate as f64);
        block.tags.insert("acq_locked", 1);
        block.tags.insert("acq_timing_phase", 0);
        block.tags.insert("acq_peak_sample", 0);
        block.tags.insert("acq_epoch", 1);
        let out = p.process_block(block);

        assert_eq!(2, out.len());
        assert!(out.iter().all(|b| b.samples.len() == 64));
        assert!(out.iter().all(|b| b.sample_rate_hz > 1_200_000.0));
    }

    fn generate_pulse_shaped_walsh0(sample_rate: u32, symbols: usize) -> Vec<Complex32> {
        let oversample = (sample_rate / 1_228_800).max(1) as usize;
        let taps = cdma2000_baseband_filter_taps_f64();
        let mut fir = ComplexFir32::new(&taps);
        let walsh0 = WalshGenerator::generate_matrix::<64>()[0];
        let mut pn = PnSequence::new_repeat(0, 32768, oversample.saturating_sub(1));
        let mut upsampled = Vec::new();
        for _ in 0..symbols {
            for chip in 0..64usize {
                let d = walsh0[chip] as f32;
                let s = pn.generate_iq();
                upsampled.push(Complex32::new(d * s.re, d * (-s.im)));
                for _ in 1..oversample {
                    let _ = pn.generate_iq();
                    upsampled.push(Complex32::default());
                }
            }
        }
        fir.process_block(&upsampled)
    }

    fn generate_tx_rx_pulse_response(sample_rate: u32, chips: usize) -> Vec<Complex32> {
        let oversample = (sample_rate / 1_228_800).max(1) as usize;
        let taps = cdma2000_baseband_filter_taps_f64();
        let mut tx_filter = ComplexFir32::new(&taps);
        let mut rx = PulseMatchedFilterProcessor::new();

        let mut upsampled = Vec::with_capacity(chips * oversample);
        for chip in 0..chips {
            upsampled.push(Complex32::new(if chip == 0 { 1.0 } else { 0.0 }, 0.0));
            for _ in 1..oversample {
                upsampled.push(Complex32::default());
            }
        }

        let tx = tx_filter.process_block(&upsampled);

        rx.process_block(SampleBlock::new(tx, 0).with_sample_rate_hz(sample_rate as f64))
            .into_iter()
            .flat_map(|b| b.samples)
            .collect()
    }

    #[test]
    fn test_tx_rx_pulse_response_has_a_clear_chip_center() {
        let sample_rate = 1_228_800u32 * 4;
        let response = generate_tx_rx_pulse_response(sample_rate, 64);

        let mut best_phase = 0usize;
        let mut best_main = 0.0f32;
        let mut best_isi = f32::MAX;

        for phase in 0..4usize {
            let samples = response[phase..]
                .chunks_exact(4)
                .map(|chunk| chunk[0].norm())
                .take(16)
                .collect::<Vec<_>>();
            let (main_idx, main) = samples
                .iter()
                .copied()
                .enumerate()
                .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
                .unwrap();
            let isi = samples
                .iter()
                .enumerate()
                .filter(|(idx, _)| *idx != main_idx)
                .map(|(_, v)| *v)
                .sum::<f32>();
            println!(
                "tx_rx_pulse_response phase={} main_idx={} main={:.6} isi_sum={:.6} samples={:?}",
                phase, main_idx, main, isi, samples
            );
            if main > best_main || (main == best_main && isi < best_isi) {
                best_phase = phase;
                best_main = main;
                best_isi = isi;
            }
        }

        assert!(
            best_main > 0.5 && best_isi < best_main,
            "no clean chip-center phase found: best_phase={} main={:.6} isi_sum={:.6}",
            best_phase,
            best_main,
            best_isi
        );
    }

    #[test]
    #[ignore = "diagnostic: pulse-shaped despreader recovery is still under investigation"]
    fn test_despreader_recovers_pulse_shaped_signal_with_known_timing() {
        let sample_rate = 1_228_800u32 * 4;
        let tx = generate_pulse_shaped_walsh0(sample_rate, 8);
        let expected = WalshGenerator::generate_matrix::<64>()[0]
            .iter()
            .copied()
            .cycle()
            .take(8 * 64)
            .map(|v| v as f32)
            .collect::<Vec<_>>();

        let mut rx_mf = PulseMatchedFilterProcessor::new();
        let mf_tx = rx_mf
            .process_block(SampleBlock::new(tx.clone(), 0).with_sample_rate_hz(sample_rate as f64))
            .into_iter()
            .flat_map(|b| b.samples)
            .collect::<Vec<_>>();

        let mut results = Vec::new();
        for (label, input) in [("tx_only", tx), ("tx_plus_rx_mf", mf_tx)] {
            let mut best_corr = -1.0f32;
            let mut best_phase = 0usize;
            let mut best_shift = 0usize;

            for phase in 0..4usize {
                let mut p =
                    MatchedFilterDespreader::new(sample_rate).with_fixed_timing_phase(phase);
                let mut block =
                    SampleBlock::new(input.clone(), 0).with_sample_rate_hz(sample_rate as f64);
                block.tags.insert("acq_locked", 1);
                block.tags.insert("acq_timing_phase", phase as i64);
                block.tags.insert("acq_peak_sample", 0);
                block.tags.insert("acq_epoch", 1);
                let recovered = p
                    .process_block(block)
                    .into_iter()
                    .flat_map(|b| b.samples)
                    .map(|s| s.re)
                    .collect::<Vec<_>>();

                for shift in 0..128usize {
                    if recovered.len() < expected.len() + shift {
                        break;
                    }
                    let got = &recovered[shift..shift + expected.len()];
                    let dot = got
                        .iter()
                        .zip(expected.iter())
                        .map(|(a, b)| a * b)
                        .sum::<f32>();
                    let got_norm = got.iter().map(|v| v * v).sum::<f32>().sqrt();
                    let exp_norm = expected.iter().map(|v| v * v).sum::<f32>().sqrt();
                    let corr = dot / (got_norm * exp_norm).max(1e-6);
                    if corr > best_corr {
                        best_corr = corr;
                        best_phase = phase;
                        best_shift = shift;
                    }
                }
            }

            println!(
                "{} pulse-shaped despreader: best_phase={} best_shift={} corr={}",
                label, best_phase, best_shift, best_corr
            );
            results.push((label, best_corr));
        }

        let tx_plus_rx_mf_corr = results
            .into_iter()
            .find_map(|(label, corr)| (label == "tx_plus_rx_mf").then_some(corr))
            .unwrap();
        assert!(
            tx_plus_rx_mf_corr > 0.90,
            "tx_plus_rx_mf pulse-shaped despreader failed to recover chips: corr={}",
            tx_plus_rx_mf_corr
        );
    }
}
