use num_complex::Complex32;

use super::{PipelineEmitter, PipelineProcessor, SampleBlock};
use crate::phy::walsh::WalshGenerator;
use cdma_common::consts::SR1_CHIPS_PER_FRAME;

/// Walsh length for R-FCH in RC3: W(4,16) → 16-chip Walsh cover.
const WALSH_LENGTH: usize = 16;
const CHIPS_PER_PCG: usize = 1_536;
const PILOT_CHIPS_PER_PCG: usize = 1_152;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(dead_code)]
enum InitialAlignmentMode {
    /// Start on the next W(4,16) cover boundary.
    Walsh16,
    /// Start on the next 1.25 ms PCG boundary, the smallest legal frame-offset phase.
    Pcg125,
    /// Start on the next 20 ms traffic-frame boundary, which is also W16-aligned.
    T20,
}

const INITIAL_ALIGNMENT_MODE: InitialAlignmentMode = InitialAlignmentMode::Pcg125;

/// RC3 Reverse FCH BPSK Walsh despreader.
///
/// After PN+LC despreading and CFO correction by the finger, the chip-rate
/// complex samples contain all RC3 reverse channels multiplexed by their
/// Walsh covers:
///   - R-PICH: W(0,64) (pilot)
///   - R-FCH:  W(4,16) (fundamental channel — what we want)
///   - R-DCCH: W(8,16) (dedicated control)
///
/// This processor extracts the R-FCH by correlating every 16-chip window
/// with W(4,16) and producing one coherently-demodulated soft symbol per
/// Walsh period.
///
/// Pilot-aided coherent demod: each 16-chip block yields both a Walsh-0
/// (R-PICH pilot) sum and a Walsh-4 (R-FCH traffic) sum.  The traffic
/// symbol is cross-correlated with the pilot conjugate to remove the
/// carrier phase, then rotated by −j for the I/Q axis convention
/// (pilot=I, traffic=Q).  The result has signal on `.re` and orthogonal
/// noise on `.im`, so the downstream frame aligner can skip blind M2
/// axis estimation and use the pilot-referenced soft bits directly.
/// Number of 16-chip symbols per PCG (1536 / 16 = 96).
const SYMBOLS_PER_PCG: usize = CHIPS_PER_PCG / WALSH_LENGTH;
const PILOT_SYMBOLS_PER_PCG: usize = PILOT_CHIPS_PER_PCG / WALSH_LENGTH;

pub struct Rc3BpskDespread {
    /// W(4,16) Walsh cover
    walsh_cover: [i8; WALSH_LENGTH],
    /// Number of soft symbols to accumulate before emitting a block.
    output_symbols: usize,
    /// Buffer of accumulated complex despread symbols.
    symbol_buf: Vec<Complex32>,
    /// Buffer of chip-rate samples waiting to fill a Walsh period.
    chip_buf: Vec<Complex32>,
    /// Per-PCG pilot accumulator for pilot-aided coherent demod.
    pcg_pilot_accum: Complex32,
    /// Sum of |pilot_k|^2 for each 16-chip pilot symbol in the current PCG.
    /// Used with |pcg_pilot_accum|^2 to estimate per-symbol noise variance.
    pcg_pilot_sym_power_sum: f32,
    /// Per-PCG traffic symbol buffer (raw Walsh-4 decover, pre-rotation).
    pcg_traffic_buf: Vec<Complex32>,
    /// Per-PCG pilot metrics accumulated for the current output block.
    /// Each entry is (pilot_norm_sq, pilot_sym_power_sum, traffic_power_sum, chip_power_sum).
    pcg_pilot_metrics_buf: Vec<(f32, f32, f32, f32)>,
    /// Sum of |chip|² for the current PCG (total wideband chip power = Io).
    pcg_chip_power_sum: f32,
    /// Tags from the first block in this output batch.
    buffer_tags: std::collections::HashMap<&'static str, i64>,
    buffer_chip_start: usize,
    buffer_sample_rate_hz: f64,
    buffer_absolute_chip_start: Option<i64>,
    buffer_absolute_sample_start: Option<i64>,
    /// Number of leading chips to skip for Walsh boundary alignment.
    chips_to_skip: usize,
    /// Whether we've initialized the skip count from the first block.
    aligned: bool,
}

impl Rc3BpskDespread {
    /// Create a new despreader targeting R-FCH (W(4,16)).
    #[cfg(test)]
    pub fn new() -> Self {
        Self::with_output_symbols(1536)
    }

    /// Create with a custom output block size (in soft symbols).
    pub fn with_output_symbols(output_symbols: usize) -> Self {
        let walsh_cover = WalshGenerator::generate_matrix::<WALSH_LENGTH>()[4];
        Self {
            walsh_cover,
            output_symbols,
            symbol_buf: Vec::with_capacity(output_symbols),
            chip_buf: Vec::with_capacity(WALSH_LENGTH),
            pcg_pilot_accum: Complex32::new(0.0, 0.0),
            pcg_pilot_sym_power_sum: 0.0,
            pcg_traffic_buf: Vec::with_capacity(SYMBOLS_PER_PCG),
            pcg_pilot_metrics_buf: Vec::with_capacity(16),
            pcg_chip_power_sum: 0.0,
            buffer_tags: std::collections::HashMap::new(),
            buffer_chip_start: 0,
            buffer_sample_rate_hz: 0.0,
            buffer_absolute_chip_start: None,
            buffer_absolute_sample_start: None,
            chips_to_skip: 0,
            aligned: false,
        }
    }

    fn despread_one_symbol(&self, chips: &[Complex32]) -> Complex32 {
        debug_assert_eq!(chips.len(), WALSH_LENGTH);
        let mut acc = Complex32::new(0.0, 0.0);
        for (chip, &w) in chips.iter().zip(self.walsh_cover.iter()) {
            let s = w as f32;
            acc += *chip * s;
        }
        acc
    }

    fn apply_buffer_timing_tags(
        tags: &mut std::collections::HashMap<&'static str, i64>,
        absolute_chip_start: Option<i64>,
        absolute_sample_start: Option<i64>,
    ) {
        if let Some(absolute_chip_start) = absolute_chip_start {
            tags.insert("absolute_chip_start", absolute_chip_start);
        }
        if let Some(absolute_sample_start) = absolute_sample_start {
            tags.insert("absolute_sample_start", absolute_sample_start);
        }
    }

    fn advance_buffer_timing(&mut self, chips: usize) {
        let delta = chips as i64;
        if let Some(absolute_chip_start) = &mut self.buffer_absolute_chip_start {
            *absolute_chip_start = absolute_chip_start.saturating_add(delta);
        }
        if let Some(absolute_sample_start) = &mut self.buffer_absolute_sample_start {
            *absolute_sample_start = absolute_sample_start.saturating_add(delta);
        }
    }
}

impl PipelineProcessor for Rc3BpskDespread {
    fn name(&self) -> &'static str {
        "Rc3BpskDespread"
    }

    fn process_block(&mut self, block: SampleBlock) -> Vec<SampleBlock> {
        let mut emitter = super::VecEmitter::new();
        let mut out = self.process_block_emitting(block, &mut emitter);
        out.extend(emitter.blocks);
        out
    }

    fn process_block_emitting(
        &mut self,
        block: SampleBlock,
        _emitter: &mut dyn PipelineEmitter,
    ) -> Vec<SampleBlock> {
        // Pass through empty event blocks (e.g. preamble detection) unchanged.
        if block.samples.is_empty() {
            return vec![block];
        }
        if !self.aligned {
            let alignment_period = match INITIAL_ALIGNMENT_MODE {
                InitialAlignmentMode::Walsh16 => WALSH_LENGTH,
                InitialAlignmentMode::Pcg125 => CHIPS_PER_PCG,
                InitialAlignmentMode::T20 => SR1_CHIPS_PER_FRAME as usize,
            };
            let remainder = block.chip_start % alignment_period;
            self.chips_to_skip = if remainder == 0 {
                0
            } else {
                alignment_period - remainder
            };
            self.aligned = true;
            log::debug!(
                "rc3_bpsk_despread: chip_start={} period={} remainder={} skipping {} chips for {} alignment",
                block.chip_start,
                alignment_period,
                remainder,
                self.chips_to_skip,
                match INITIAL_ALIGNMENT_MODE {
                    InitialAlignmentMode::Walsh16 => "Walsh16",
                    InitialAlignmentMode::Pcg125 => "Pcg125",
                    InitialAlignmentMode::T20 => "T20",
                },
            );
        }

        let mut out = Vec::new();

        // Log input block diagnostics: pilot phase, size, alignment.
        {
            use std::sync::atomic::{AtomicU64, Ordering};
            static DESPREAD_LOG_COUNT: AtomicU64 = AtomicU64::new(0);
            let n = DESPREAD_LOG_COUNT.fetch_add(1, Ordering::Relaxed);
            if n < 10 || n % 800 == 0 {
                let block_skip = self.chips_to_skip.min(block.samples.len());
                let usable = &block.samples[block_skip..];
                let pilot_sum: Complex32 = usable.iter().copied().sum();
                let pilot_phase = pilot_sum.im.atan2(pilot_sum.re).to_degrees();
                let pilot_norm = pilot_sum.norm();
                let chip_start_mod_pcg = block.chip_start % CHIPS_PER_PCG;
                let chip_start_mod_walsh = block.chip_start % WALSH_LENGTH;
                log::debug!(
                    "RC3_DESPREAD_IN: block={} chip_start={} samples={} skip={} usable={} | pilot: {:.1}° norm={:.1} | chip%pcg={} chip%walsh={}",
                    n,
                    block.chip_start,
                    block.samples.len(),
                    block_skip,
                    usable.len(),
                    pilot_phase,
                    pilot_norm,
                    chip_start_mod_pcg,
                    chip_start_mod_walsh,
                );
            }
        }

        let mut chip_index = block.chip_start;
        for &sample in &block.samples {
            if self.chips_to_skip > 0 {
                self.chips_to_skip -= 1;
                chip_index += 1;
                continue;
            }
            if self.symbol_buf.is_empty()
                && self.chip_buf.is_empty()
                && self.pcg_traffic_buf.is_empty()
            {
                let offset = chip_index.saturating_sub(block.chip_start) as i64;
                self.buffer_tags = block.tags.clone();
                self.buffer_chip_start = chip_index;
                self.buffer_sample_rate_hz = block.sample_rate_hz;
                self.buffer_absolute_chip_start = block
                    .tags
                    .get("absolute_chip_start")
                    .map(|start| start.saturating_add(offset));
                self.buffer_absolute_sample_start = block
                    .tags
                    .get("absolute_sample_start")
                    .map(|start| start.saturating_add(offset));
            }
            self.chip_buf.push(sample);
            self.pcg_chip_power_sum += sample.norm_sqr();
            chip_index += 1;

            if self.chip_buf.len() == WALSH_LENGTH {
                // Accumulate per-16-chip Walsh-0 (pilot) and Walsh-4 (traffic).
                let pilot_sym: Complex32 = self.chip_buf.iter().copied().sum();
                let traffic_sym = self.despread_one_symbol(&self.chip_buf);

                let symbol_index_in_pcg = self.pcg_traffic_buf.len();
                if symbol_index_in_pcg < PILOT_SYMBOLS_PER_PCG {
                    self.pcg_pilot_accum += pilot_sym;
                    self.pcg_pilot_sym_power_sum += pilot_sym.norm_sqr();
                }
                self.pcg_traffic_buf.push(traffic_sym);
                self.chip_buf.clear();

                // At PCG boundary (96 symbols), apply pilot-aided coherent
                // demod to all symbols in the PCG using the accumulated pilot.
                if self.pcg_traffic_buf.len() >= SYMBOLS_PER_PCG {
                    let pilot = self.pcg_pilot_accum;
                    let pilot_phase_deg = pilot.im.atan2(pilot.re).to_degrees();
                    let pilot_norm = pilot.norm();

                    // Also compute per-symbol pilot stats for first few PCGs
                    // to see if 16-chip pilot sums are coherent within the PCG.
                    static PCG_DIAG_COUNT: std::sync::atomic::AtomicU64 =
                        std::sync::atomic::AtomicU64::new(0);
                    let pcg_n = PCG_DIAG_COUNT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                    if pcg_n < 40 || pcg_n % 800 == 0 {
                        // Show first 4 individual traffic symbols and the PCG pilot
                        let t0 = self.pcg_traffic_buf[0];
                        let t1 = self.pcg_traffic_buf[1];
                        let t2 = self.pcg_traffic_buf[2];
                        let t3 = self.pcg_traffic_buf[3];
                        log::debug!(
                            "PCG_DEMOD_DIAG pcg={}: pilot=({:.1},{:.1}) phase={:.1}° norm={:.1} | traffic[0..3]=({:.2},{:.2}) ({:.2},{:.2}) ({:.2},{:.2}) ({:.2},{:.2})",
                            pcg_n,
                            pilot.re,
                            pilot.im,
                            pilot_phase_deg,
                            pilot_norm,
                            t0.re,
                            t0.im,
                            t1.re,
                            t1.im,
                            t2.re,
                            t2.im,
                            t3.re,
                            t3.im,
                        );
                    }

                    let pilot_conj = pilot.conj();
                    for &tsym in &self.pcg_traffic_buf {
                        let cross = tsym * pilot_conj;
                        // Traffic sits at −90° from pilot (negative .im).
                        // cross.im is negative for +1 data bits.
                        // Rotate by +j to put signal on .re with correct polarity.
                        let soft = Complex32::new(-cross.im, cross.re);
                        self.symbol_buf.push(soft);
                    }
                    // Record pilot and traffic metrics for this PCG.
                    let pilot_norm_sq = pilot.norm_sqr();
                    let traffic_power_sum: f32 =
                        self.pcg_traffic_buf.iter().map(|t| t.norm_sqr()).sum();
                    self.pcg_pilot_metrics_buf.push((
                        pilot_norm_sq,
                        self.pcg_pilot_sym_power_sum,
                        traffic_power_sum,
                        self.pcg_chip_power_sum,
                    ));

                    self.pcg_pilot_accum = Complex32::new(0.0, 0.0);
                    self.pcg_pilot_sym_power_sum = 0.0;
                    self.pcg_chip_power_sum = 0.0;
                    self.pcg_traffic_buf.clear();
                }

                if self.symbol_buf.len() >= self.output_symbols {
                    let chunk: Vec<Complex32> =
                        self.symbol_buf.drain(..self.output_symbols).collect();
                    let mut out_blk = SampleBlock::new(chunk, self.buffer_chip_start);
                    out_blk.sample_rate_hz = self.buffer_sample_rate_hz;
                    for (&k, &v) in &self.buffer_tags {
                        out_blk.tags.insert(k, v);
                    }
                    out_blk.tags.insert("pilot_coherent", 1);
                    // Attach per-PCG pilot metrics for noise estimation.
                    let pcgs_in_block = self.output_symbols / SYMBOLS_PER_PCG;
                    if self.pcg_pilot_metrics_buf.len() >= pcgs_in_block {
                        out_blk.pcg_pilot_metrics =
                            Some(self.pcg_pilot_metrics_buf.drain(..pcgs_in_block).collect());
                    }
                    Self::apply_buffer_timing_tags(
                        &mut out_blk.tags,
                        self.buffer_absolute_chip_start,
                        self.buffer_absolute_sample_start,
                    );
                    self.buffer_chip_start += self.output_symbols * WALSH_LENGTH;
                    self.advance_buffer_timing(self.output_symbols * WALSH_LENGTH);
                    out.push(out_blk);
                }
            }
        }

        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn emitted_symbols_advance_absolute_chip_start() {
        // Pilot-coherent demod batches by PCG (96 symbols), so we need at
        // least 2 × 96 × 16 = 3072 chips to get 2 output blocks with
        // output_symbols=96 (one PCG each).
        let mut despreader = Rc3BpskDespread::with_output_symbols(SYMBOLS_PER_PCG);
        let chips_for_two_pcgs = SYMBOLS_PER_PCG * WALSH_LENGTH * 2;
        let mut block = SampleBlock::new(vec![Complex32::new(1.0, 0.0); chips_for_two_pcgs], 0)
            .with_sample_rate_hz(1_228_800.0);
        block.tags.insert("absolute_chip_start", 10_000);

        let out = despreader.process_block(block);

        assert_eq!(out.len(), 2);
        assert_eq!(out[0].chip_start, 0);
        assert_eq!(out[0].tags.get("absolute_chip_start"), Some(&10_000));
        let one_pcg_chips = (SYMBOLS_PER_PCG * WALSH_LENGTH) as i64;
        assert_eq!(out[1].chip_start, SYMBOLS_PER_PCG * WALSH_LENGTH);
        assert_eq!(
            out[1].tags.get("absolute_chip_start"),
            Some(&(10_000 + one_pcg_chips)),
        );
        // Verify pilot metrics are attached (one per PCG per block).
        assert!(out[0].pcg_pilot_metrics.is_some());
        assert_eq!(out[0].pcg_pilot_metrics.as_ref().unwrap().len(), 1);
        assert!(out[1].pcg_pilot_metrics.is_some());
        assert_eq!(out[1].pcg_pilot_metrics.as_ref().unwrap().len(), 1);
    }
}
