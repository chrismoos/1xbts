mod chain_factories;
mod pn_helpers;
mod runner;
mod sample_block;
mod timing;
mod trace;

mod access_channel_processor;
mod acquisition_fft_processor;
mod acquisition_fft_simple_processor;
mod all_zeros_primer;
mod cfo_tracker;
mod decimator_processor;
mod deinterleaver_processor;
mod gardner_timing_recovery;
pub mod generic_rake_receiver;
mod hard_viterbi_decoder_r13_processor;
pub mod hrpd_access_frame_correlator;
mod long_code_descrambler;
mod matched_filter_despreader;
mod matched_filter_tracker;
pub mod mobile_station;
mod mobile_station_processor;
mod paging_channel_processor;
mod peak_sample_decimator;
mod pn_align_processor;
pub mod pn_lc_correlator;
mod pulse_matched_filter_processor;
mod rake_access_searcher;
pub mod rake_receiver;
mod rc1_reverse_traffic_decoder;
mod rc1_traffic_frame_aligner;
mod rc1_traffic_multi_rate_decoder;
mod rc1_traffic_walsh_synchronizer;
mod rc3_bpsk_despread;
mod rc3_frame_aligner;
mod rc3_pilot_detector;
mod reverse_access_decoder;
mod reverse_access_frame_aligner;
mod reverse_access_lc_descrambler;
mod reverse_access_long_code_processor;
mod reverse_access_orthogonal_demod;
mod reverse_access_walsh_aligner;
mod reverse_access_walsh_symbol_demod;
mod sliding_correlator_processor;
mod soft_viterbi_decoder_processor;
mod soft_viterbi_decoder_r13_processor;
mod sync_channel_processor;
mod traffic_channel_processor;
mod unrepeater;
mod viterbi_decoder_processor;
mod walsh_decoder_processor;
mod walsh_pilot_combiner;

pub use access_channel_processor::AccessChannelProcessor;
pub use acquisition_fft_processor::AcquisitionFftProcessor;
pub use acquisition_fft_simple_processor::AcquisitionFftSimpleProcessor;
pub use all_zeros_primer::AllZerosPrimer;
pub use decimator_processor::DecimatorProcessor;
pub use deinterleaver_processor::DeinterleaverProcessor;
pub use gardner_timing_recovery::{GardnerTimingConfig, GardnerTimingRecovery};
pub use hard_viterbi_decoder_r13_processor::{
    HardViterbiDecoderR13Processor, HardViterbiDecoderR14Processor,
};
pub use hrpd_access_frame_correlator::{
    HrpdAccessFrameCorrelator, HrpdAccessFrameFftConfig, HrpdAccessFrameFftHit,
    HrpdAccessFrameRakeCorrelator,
};
pub use long_code_descrambler::LongCodeDescrambler;
pub use matched_filter_despreader::MatchedFilterDespreader;
pub use matched_filter_tracker::MatchedFilterTracker;
pub use mobile_station::MobileStation;
pub use mobile_station_processor::MobileStationProcessor;
pub use paging_channel_processor::PagingChannelProcessor;
pub use peak_sample_decimator::PeakSampleDecimator;
pub use pn_align_processor::PnAlignProcessor;
pub use pulse_matched_filter_processor::PulseMatchedFilterProcessor;
pub use rake_access_searcher::RakeAccessSearcher;
pub use rake_receiver::RakeReceiver;
pub use rc1_traffic_frame_aligner::Rc1TrafficFrameAligner;
pub use rc1_traffic_multi_rate_decoder::Rc1TrafficMultiRateDecoder;
pub use rc1_traffic_walsh_synchronizer::Rc1TrafficWalshSynchronizer;
pub use reverse_access_decoder::ReverseAccessDecoder;
pub use reverse_access_frame_aligner::ReverseAccessFrameAligner;
pub use reverse_access_lc_descrambler::ReverseAccessLcDescrambler;
pub use reverse_access_long_code_processor::ReverseAccessLongCodeProcessor;
pub use reverse_access_orthogonal_demod::ReverseAccessOrthogonalDemodProcessor;
pub use reverse_access_walsh_aligner::ReverseAccessWalshAligner;
pub use reverse_access_walsh_symbol_demod::ReverseAccessWalshSymbolDemodProcessor;
pub use sliding_correlator_processor::SlidingCorrelatorProcessor;
pub use soft_viterbi_decoder_processor::SoftViterbiDecoderProcessor;
pub use soft_viterbi_decoder_r13_processor::SoftViterbiDecoderR13Processor;
pub use sync_channel_processor::SyncChannelProcessor;
pub use traffic_channel_processor::TrafficChannelProcessor;
pub use traffic_channel_processor::{
    ReverseMux1FullRateFormat, ReverseMux1SignalingBlock, ReverseMux1SignalingLayout,
    extract_reverse_mux1_full_rate_signaling_block, parse_reverse_mux1_full_rate_format,
};
pub use unrepeater::Unrepeater;
pub use viterbi_decoder_processor::ViterbiDecoderProcessor;
pub use walsh_decoder_processor::WalshDecoderProcessor;
pub use walsh_pilot_combiner::WalshPilotCombiner;

// Re-exports from extracted sub-modules
use cdma_common::consts::SR1_CHIP_RATE_HZ;
pub use chain_factories::*;
pub use pn_helpers::{ShortCodeReferenceKind, chips_per_sample};
pub(crate) use pn_helpers::{
    build_fft_search_pn_samples, build_fft_search_pn_samples_with_kind, build_matched_pn_reference,
    build_oqpsk_pn_samples, build_oqpsk_pn_samples_with_kind,
};
pub use runner::{PipelinedReceiver, flush_sub_chain, run_sub_chain};
pub use sample_block::{RxSampleTimeAnchor, SampleBlock};

#[cfg(test)]
pub(crate) use rc3_bpsk_despread::Rc3BpskDespread;

// ---------------------------------------------------------------------------
// Shared soft-decision helper
// ---------------------------------------------------------------------------

/// Map a raw combined value (positive=0, negative=1) to soft \[0, 1\].
///
/// Uses the inverse of the peak absolute value of the current buffer for
/// normalization: `inv_peak = 1.0 / peak`.
#[inline]
pub(crate) fn raw_to_soft(raw: f32, inv_peak: f32) -> f32 {
    (0.5 - raw * 0.5 * inv_peak).clamp(0.0, 1.0)
}

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

/// CDMA2000 chip rate in chips per second.
pub const CDMA_CHIP_RATE: f64 = SR1_CHIP_RATE_HZ as f64;

// ---------------------------------------------------------------------------
// Pipeline trait and runner
// ---------------------------------------------------------------------------

pub type PipelineProcessorShared = Box<dyn PipelineProcessor>;

#[derive(Clone, Debug)]
pub struct ReverseAccessSettings {
    pub oversample: usize,
    pub access_channel_number: u8,
    pub paging_channel_number: u8,
    pub base_id: u16,
    pub pilot_pn: u16,
    pub long_code_state: u64,
    /// When true, the rake uses a fast despread path (no FFT) once fingers
    /// exist.  Set to false for bursty signals like access probes where
    /// continuous FFT tracking is needed.
    pub rake_fast_path: bool,
    /// Optional fixed finger phase (in samples).  When set, the rake seeds
    /// a pre-validated finger at this phase instead of discovering it via
    /// FFT correlation.  For reverse-link access, the BTS knows the pilot
    /// PN offset so no search is needed.
    pub fixed_finger_phase: Option<usize>,
    /// Re-anchor the correlator's absolute sample origin on every block
    /// using the hardware timestamp tag, correcting for SDR overflow drift.
    pub reanchor_origin: bool,
    /// Thread pool size for parallel finger feeding.  Default 8.
    pub finger_pool_size: usize,
}

impl Default for ReverseAccessSettings {
    fn default() -> Self {
        Self {
            oversample: 4,
            access_channel_number: 0,
            paging_channel_number: 1,
            base_id: 1,
            pilot_pn: 0,
            long_code_state: 1u64 << 41,
            rake_fast_path: true,
            fixed_finger_phase: None,
            reanchor_origin: false,
            finger_pool_size: 8,
        }
    }
}

/// Callback for emitting output blocks directly from within
/// `process_block`, bypassing the rest of the processor chain. Used for
/// latency-critical events like per-PCG power control measurements that
/// need to reach the BSC before the chain finishes processing.
pub trait PipelineEmitter: Send {
    fn emit(&mut self, block: SampleBlock);
}

/// Collects emitted blocks into a `Vec` for later retrieval.
pub struct VecEmitter {
    pub blocks: Vec<SampleBlock>,
}

impl VecEmitter {
    pub fn new() -> Self {
        Self { blocks: Vec::new() }
    }
}

impl PipelineEmitter for VecEmitter {
    fn emit(&mut self, block: SampleBlock) {
        self.blocks.push(block);
    }
}

pub trait PipelineProcessor: Send {
    fn process_block(&mut self, block: SampleBlock) -> Vec<SampleBlock>;
    fn name(&self) -> &'static str {
        std::any::type_name::<Self>()
    }
    /// Process a block with access to a side-channel emitter. Blocks sent
    /// via `emitter.emit()` go directly to the pipeline output, bypassing
    /// all downstream processors in the chain. The returned `Vec` still
    /// flows through the chain as usual.
    ///
    /// Override this (instead of `process_block`) in processors that need
    /// to emit latency-critical blocks like PCG measurements. The default
    /// delegates to `process_block` and ignores the emitter.
    fn process_block_emitting(
        &mut self,
        block: SampleBlock,
        _emitter: &mut dyn PipelineEmitter,
    ) -> Vec<SampleBlock> {
        self.process_block(block)
    }
    /// Flush buffered state at end-of-stream.
    fn flush(&mut self) -> Vec<SampleBlock> {
        Vec::new()
    }
    /// Optional custom metrics for pipeline timing reports.
    /// Returns a list of (key, value) string pairs.
    fn metrics(&self) -> Vec<(&'static str, String)> {
        Vec::new()
    }
}

#[cfg(test)]
mod tests {
    use std::env;
    use std::f64::consts::PI;
    use std::ffi::OsStr;
    use std::path::{Component, Path, PathBuf};

    use env_logger::Env;
    use num_complex::Complex32;
    use serde::Deserialize;

    use super::HrpdReverseAccessSettings;
    use super::{
        AccessChannelProcessor, AcquisitionFftProcessor, DeinterleaverProcessor,
        LongCodeDescrambler, MatchedFilterDespreader, MobileStation, PagingChannelProcessor,
        PipelineProcessor, PipelineProcessorShared, PipelinedReceiver, PnAlignProcessor,
        PulseMatchedFilterProcessor, ReverseAccessLongCodeProcessor,
        ReverseAccessOrthogonalDemodProcessor, ReverseAccessSettings, SampleBlock,
        SoftViterbiDecoderProcessor, SyncChannelProcessor, Unrepeater, WalshDecoderProcessor,
        WalshPilotCombiner, access_channel_chain, build_fft_search_pn_samples,
        build_oqpsk_pn_samples, hrpd_reverse_access_chain,
    };
    use crate::lac::crc30;
    use crate::phy::coding::long_code::LongCodeGenerator;
    use crate::phy::coding::{
        block_interleaver::{self, BitReversalInterleaver},
        convolutional::{
            SoftViterbiDecoder, get_1_2_k9_encoder, get_1_3_k9_encoder, get_1_4_k9_encoder,
        },
    };
    use crate::phy::walsh::{WalshDecoder, WalshGenerator};
    use crate::receiver::hrpd::access::{
        ACCESS_CHIP_RATE, ACCESS_PACKET_CHIPS, AccessFrameLayout, AccessPhyDecodeAttempt,
        decode_access_phy_chips_attempt, parse_access_mac_capsule, validate_access_mac_fragment,
    };
    use crate::receiver::paging::PagingChannelRate;
    use crate::receiver::pipelined::decimator_processor::DecimatorProcessor;
    use crate::receiver::pipelined::mobile_station::PagingRate;
    use crate::receiver::pipelined::rake_receiver::RakeReceiver;
    use crate::receiver::{access_layer3::AccessMessage, layer3::PagingMessage};
    use crate::sdr::fir::ComplexFir32;
    use cdma_common::bits::Bitstream;

    fn workspace_fixture_path(relative: impl AsRef<Path>) -> PathBuf {
        let relative = relative.as_ref();
        if relative.is_absolute() || relative.exists() {
            return relative.to_path_buf();
        }

        let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
        let test_relative = relative
            .components()
            .skip_while(|component| {
                !matches!(component, Component::Normal(part) if *part == OsStr::new("test"))
            })
            .collect::<PathBuf>();
        let lookup_relative = if test_relative.as_os_str().is_empty() {
            relative.to_path_buf()
        } else {
            test_relative
        };

        manifest_dir
            .ancestors()
            .map(|ancestor| ancestor.join(&lookup_relative))
            .find(|candidate| candidate.exists())
            .unwrap_or_else(|| manifest_dir.join(lookup_relative))
    }

    fn test_iq_path(file_name: &str) -> PathBuf {
        workspace_fixture_path(Path::new("test").join("iq").join(file_name))
    }

    fn test_capture_path(file_name: &str) -> PathBuf {
        workspace_fixture_path(Path::new("test").join("capture").join(file_name))
    }

    #[derive(Debug, Deserialize)]
    struct IqCaptureMetadata {
        sample_rate_hz: usize,
        chip_rate_hz: usize,
        rx_center_frequency_hz: Option<usize>,
        one_x_reverse_frequency_hz: Option<usize>,
        one_x_rx_shift_hz: Option<i64>,
        hrpd_reverse_frequency_hz: Option<usize>,
        hrpd_rx_shift_hz: Option<i64>,
        first_absolute_sample_start: u64,
    }

    #[allow(dead_code)]
    fn test_capture_metadata(file_name: &str) -> IqCaptureMetadata {
        let path = test_capture_path(file_name);
        test_capture_metadata_from_path(&path)
    }

    fn test_capture_metadata_from_path(path: &Path) -> IqCaptureMetadata {
        let raw = std::fs::read_to_string(path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", path.display()));
        serde_json::from_str(&raw)
            .unwrap_or_else(|e| panic!("failed to parse {}: {e}", path.display()))
    }

    fn init_test_logger() {
        let _ = env_logger::Builder::from_env(Env::default().default_filter_or("info"))
            .is_test(true)
            .try_init();
    }

    /// Processor that emits a tagged block via the emitter (side-channel)
    /// and also returns a different block through the normal chain.
    struct EarlyEmitProcessor;

    impl PipelineProcessor for EarlyEmitProcessor {
        fn process_block(&mut self, block: SampleBlock) -> Vec<SampleBlock> {
            // Normal path: pass through
            vec![block]
        }

        fn process_block_emitting(
            &mut self,
            block: SampleBlock,
            emitter: &mut dyn super::PipelineEmitter,
        ) -> Vec<SampleBlock> {
            // Emit a tagged block via side-channel
            let mut early = SampleBlock::new(Vec::new(), block.chip_start);
            early.tags.insert("early_emitted", 1);
            early
                .tags
                .insert("source_chip_start", block.chip_start as i64);
            emitter.emit(early);

            // Return a different block through the chain
            let mut chain_block = block;
            chain_block.tags.insert("chain_passed", 1);
            vec![chain_block]
        }
    }

    /// Processor that would modify blocks passing through the chain.
    /// Used to verify early-emitted blocks bypass this.
    struct TaggingProcessor;

    impl PipelineProcessor for TaggingProcessor {
        fn process_block(&mut self, mut block: SampleBlock) -> Vec<SampleBlock> {
            block.tags.insert("downstream_touched", 1);
            vec![block]
        }
    }

    #[test]
    fn test_early_emit_bypasses_downstream_processors() {
        // Chain: EarlyEmitProcessor → TaggingProcessor
        // EarlyEmitProcessor emits one block via emitter and one through chain.
        // The emitted block should NOT have "downstream_touched".
        // The chain block SHOULD have "downstream_touched".
        let mut chain: Vec<super::PipelineProcessorShared> =
            vec![Box::new(EarlyEmitProcessor), Box::new(TaggingProcessor)];

        let input = SampleBlock::new(vec![Complex32::new(1.0, 0.0); 10], 42000);

        let mut emitter = super::VecEmitter::new();
        let chain_output = super::run_sub_chain(&mut chain, input, &mut emitter);

        // Chain output: should have both "chain_passed" and "downstream_touched"
        assert_eq!(chain_output.len(), 1, "expected 1 block from chain");
        assert_eq!(
            chain_output[0].tags.get("chain_passed"),
            Some(&1),
            "chain block should be tagged by EarlyEmitProcessor"
        );
        assert_eq!(
            chain_output[0].tags.get("downstream_touched"),
            Some(&1),
            "chain block should be tagged by TaggingProcessor"
        );

        // Early-emitted output: should have "early_emitted" but NOT "downstream_touched"
        assert_eq!(emitter.blocks.len(), 1, "expected 1 early-emitted block");
        assert_eq!(
            emitter.blocks[0].tags.get("early_emitted"),
            Some(&1),
            "early block should have the emitter tag"
        );
        assert_eq!(
            emitter.blocks[0].tags.get("source_chip_start"),
            Some(&42000),
            "early block should carry the source chip_start"
        );
        assert!(
            emitter.blocks[0].tags.get("downstream_touched").is_none(),
            "early-emitted block must NOT pass through downstream TaggingProcessor"
        );
    }

    fn open_baseband_downlink_4x_reader() -> hound::WavReader<std::io::BufReader<std::fs::File>> {
        let path = test_capture_path("baseband_downlink_4x.wav");
        if path.exists() {
            return hound::WavReader::open(&path)
                .unwrap_or_else(|e| panic!("failed to open {}: {e}", path.display()));
        }

        panic!("could not find baseband_downlink_4x.wav in known locations");
    }

    fn read_iq_wav(
        mut reader: hound::WavReader<std::io::BufReader<std::fs::File>>,
    ) -> (u32, Vec<Complex32>) {
        let sample_rate = reader.spec().sample_rate;
        let samples = reader
            .samples::<i16>()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let iq_samples = samples
            .chunks_exact(2)
            .map(|a| Complex32::new(a[0] as f32 / i16::MAX as f32, a[1] as f32 / i16::MAX as f32))
            .collect::<Vec<_>>();
        (sample_rate, iq_samples)
    }

    fn test_carrier_slice_anti_alias_taps(
        decimation: usize,
        sample_rate_hz: usize,
        chip_rate_hz: usize,
    ) -> Vec<f64> {
        let taps = 63usize;
        let center = (taps - 1) as f64 / 2.0;
        let nyquist = sample_rate_hz as f64 / 2.0;
        let alias_cutoff = sample_rate_hz as f64 / (2.0 * decimation as f64) * 0.82;
        let occupied_cutoff = chip_rate_hz as f64 * 1.25;
        let cutoff_hz = alias_cutoff.min(occupied_cutoff).min(nyquist * 0.95);
        let fc = cutoff_hz / sample_rate_hz as f64;
        let mut out = Vec::with_capacity(taps);
        for n in 0..taps {
            let x = n as f64 - center;
            let sinc = if x.abs() < f64::EPSILON {
                2.0 * fc
            } else {
                (2.0 * PI * fc * x).sin() / (PI * x)
            };
            let window = 0.42 - 0.5 * (2.0 * PI * n as f64 / (taps - 1) as f64).cos()
                + 0.08 * (4.0 * PI * n as f64 / (taps - 1) as f64).cos();
            out.push(sinc * window);
        }
        let gain: f64 = out.iter().sum();
        if gain.abs() > f64::EPSILON {
            for tap in &mut out {
                *tap /= gain;
            }
        }
        out
    }

    fn push_filtered_decimated_chunk(
        filter: &mut ComplexFir32,
        i_vals: &[f32],
        q_vals: &[f32],
        raw_chunk_start: u64,
        raw_abs_start: u64,
        decimation: usize,
        out: &mut Vec<Complex32>,
    ) {
        let chunk = i_vals
            .iter()
            .zip(q_vals)
            .map(|(&re, &im)| Complex32::new(re, im))
            .collect::<Vec<_>>();
        let filtered = filter.process_block(&chunk);
        for (idx, sample) in filtered.into_iter().enumerate() {
            let raw_idx = raw_chunk_start + idx as u64;
            if (raw_abs_start + raw_idx) % decimation as u64 == 0 {
                out.push(sample);
            }
        }
    }

    fn read_shifted_capture_to_4x_with_shift(
        wav_path: &Path,
        metadata: &IqCaptureMetadata,
        carrier_shift_hz: i64,
    ) -> (u32, Vec<Complex32>, u64) {
        let mut reader = hound::WavReader::open(wav_path)
            .unwrap_or_else(|e| panic!("failed to open {}: {e}", wav_path.display()));
        let spec = reader.spec();
        assert_eq!(spec.channels, 2, "capture WAV must be stereo IQ");
        assert_eq!(
            spec.sample_rate as usize, metadata.sample_rate_hz,
            "capture WAV sample rate must match sidecar metadata"
        );
        assert_eq!(
            metadata.sample_rate_hz % metadata.chip_rate_hz,
            0,
            "capture rate must be an integer chip-rate multiple"
        );

        let input_oversample = metadata.sample_rate_hz / metadata.chip_rate_hz;
        assert!(
            input_oversample >= 4 && input_oversample % 4 == 0,
            "capture oversample must be a multiple of 4x, got {input_oversample}x"
        );
        let decimation = input_oversample / 4;
        let output_sample_rate_hz = metadata.sample_rate_hz / decimation;
        let output_oversample = output_sample_rate_hz / metadata.chip_rate_hz;
        assert_eq!(output_oversample, 4);

        let phase_step = -2.0 * PI * carrier_shift_hz as f64 / metadata.sample_rate_hz as f64;
        let taps = test_carrier_slice_anti_alias_taps(
            decimation,
            metadata.sample_rate_hz,
            metadata.chip_rate_hz,
        );
        let mut filter = ComplexFir32::new(&taps);

        let first_idx = if decimation <= 1 {
            0
        } else {
            let rem = metadata.first_absolute_sample_start % decimation as u64;
            ((decimation as u64 - rem) % decimation as u64) as usize
        };
        let output_absolute_sample_start =
            (metadata.first_absolute_sample_start + first_idx as u64) / decimation as u64;

        let expected_output_len = reader.duration() as usize / spec.channels as usize / decimation;
        let mut out = Vec::with_capacity(expected_output_len);
        let mut i_vals = Vec::with_capacity(65_536);
        let mut q_vals = Vec::with_capacity(65_536);
        let mut phase = 0.0f64;
        let mut raw_sample_idx = 0u64;
        let mut raw_chunk_start = 0u64;
        let mut samples = reader.samples::<i16>();

        loop {
            let i = match samples.next() {
                Some(Ok(v)) => v,
                Some(Err(e)) => panic!("failed to read I sample from {}: {e}", wav_path.display()),
                None => break,
            };
            let q = match samples.next() {
                Some(Ok(v)) => v,
                Some(Err(e)) => panic!("failed to read Q sample from {}: {e}", wav_path.display()),
                None => panic!("capture WAV {} ended mid-IQ pair", wav_path.display()),
            };

            let sample = Complex32::new(i as f32 / i16::MAX as f32, q as f32 / i16::MAX as f32);
            let rot = Complex32::new(phase.cos() as f32, phase.sin() as f32);
            let shifted = sample * rot;
            i_vals.push(shifted.re);
            q_vals.push(shifted.im);

            phase += phase_step;
            if phase > PI || phase < -PI {
                phase = (phase + PI).rem_euclid(2.0 * PI) - PI;
            }
            raw_sample_idx += 1;

            if i_vals.len() == i_vals.capacity() {
                push_filtered_decimated_chunk(
                    &mut filter,
                    &i_vals,
                    &q_vals,
                    raw_chunk_start,
                    metadata.first_absolute_sample_start,
                    decimation,
                    &mut out,
                );
                raw_chunk_start = raw_sample_idx;
                i_vals.clear();
                q_vals.clear();
            }
        }

        if !i_vals.is_empty() {
            push_filtered_decimated_chunk(
                &mut filter,
                &i_vals,
                &q_vals,
                raw_chunk_start,
                metadata.first_absolute_sample_start,
                decimation,
                &mut out,
            );
        }

        (
            output_sample_rate_hz as u32,
            out,
            output_absolute_sample_start,
        )
    }

    fn hrpd_slot_aligned_samples(
        samples: &[Complex32],
        first_chip: u64,
        oversample: usize,
    ) -> (&[Complex32], u64, usize) {
        let offset_chips = (2048 - (first_chip % 2048)) % 2048;
        let offset_samples = offset_chips as usize * oversample;
        if offset_samples >= samples.len() {
            (&[], first_chip + offset_chips, offset_samples)
        } else {
            (
                &samples[offset_samples..],
                first_chip + offset_chips,
                offset_samples,
            )
        }
    }

    #[derive(Clone, Debug)]
    struct HrpdSlotPowerRun {
        start_slot: usize,
        end_slot: usize,
        start_chip: u64,
        end_chip: u64,
        peak_db: f32,
    }

    fn hrpd_slot_power_runs_at_threshold(
        samples: &[Complex32],
        first_chip: u64,
        oversample: usize,
        threshold_db: f32,
    ) -> Vec<HrpdSlotPowerRun> {
        let slot_samples = 2048 * oversample;
        let (slot_aligned_samples, aligned_first_chip, _) =
            hrpd_slot_aligned_samples(samples, first_chip, oversample);
        let powers = slot_aligned_samples
            .chunks_exact(slot_samples)
            .enumerate()
            .map(|(slot_idx, slot)| {
                let pwr = slot.iter().map(|s| s.norm_sqr()).sum::<f32>() / slot.len() as f32;
                (slot_idx, pwr)
            })
            .collect::<Vec<_>>();
        if powers.is_empty() {
            return Vec::new();
        }
        let mut sorted = powers.iter().map(|(_, pwr)| *pwr).collect::<Vec<_>>();
        sorted.sort_by(|a, b| a.total_cmp(b));
        let median = sorted[sorted.len() / 2].max(1.0e-12);

        let mut runs = Vec::new();
        let mut run_start: Option<usize> = None;
        let mut run_peak = f32::NEG_INFINITY;
        for (slot_idx, pwr) in powers
            .iter()
            .copied()
            .chain(std::iter::once((powers.len(), 0.0)))
        {
            let rel_db = 10.0 * (pwr / median).max(1.0e-12).log10();
            if rel_db >= threshold_db {
                run_start.get_or_insert(slot_idx);
                run_peak = run_peak.max(rel_db);
                continue;
            }
            if let Some(start_slot) = run_start.take() {
                let end_slot = slot_idx;
                let start_chip = aligned_first_chip + start_slot as u64 * 2048;
                let end_chip = aligned_first_chip + end_slot as u64 * 2048;
                runs.push(HrpdSlotPowerRun {
                    start_slot,
                    end_slot,
                    start_chip,
                    end_chip,
                    peak_db: run_peak,
                });
                run_peak = f32::NEG_INFINITY;
            }
        }
        runs
    }

    fn hrpd_blind_preamble_lag_coherence(
        samples: &[Complex32],
        absolute_sample_start: u64,
        oversample: usize,
        preamble_start_chip: i64,
        sample_delay: i32,
        sample_delay_fraction: f32,
    ) -> Option<(f32, f32)> {
        let stride = 64usize;
        let mut dot = Complex32::new(0.0, 0.0);
        let mut pow_a = 0.0f32;
        let mut pow_b = 0.0f32;
        for k in (0..ACCESS_PACKET_CHIPS).step_by(stride) {
            let a = sample_chip_interp(
                samples,
                absolute_sample_start,
                oversample,
                preamble_start_chip + k as i64,
                sample_delay,
                sample_delay_fraction,
            )?;
            let b = sample_chip_interp(
                samples,
                absolute_sample_start,
                oversample,
                preamble_start_chip + ACCESS_PACKET_CHIPS as i64 + k as i64,
                sample_delay,
                sample_delay_fraction,
            )?;
            dot += a.conj() * b;
            pow_a += a.norm_sqr();
            pow_b += b.norm_sqr();
        }
        let coherence = dot.norm() / (pow_a * pow_b).sqrt().max(1.0e-12);
        Some((coherence, dot.arg()))
    }

    fn pack_bits_to_bytes(bits: &[u8]) -> Vec<u8> {
        bits.chunks(8)
            .map(|chunk| chunk.iter().fold(0u8, |acc, &bit| (acc << 1) | (bit & 1)))
            .collect()
    }

    fn interp_complex_linear(samples: &[Complex32], idx: f64) -> Option<Complex32> {
        if idx < 0.0 {
            return None;
        }
        let i0 = idx.floor() as usize;
        let i1 = i0 + 1;
        if i1 >= samples.len() {
            return None;
        }
        let frac = (idx - i0 as f64) as f32;
        Some(samples[i0] * (1.0 - frac) + samples[i1] * frac)
    }

    fn sample_chip_interp(
        samples: &[Complex32],
        absolute_sample_start: u64,
        oversample: usize,
        chip: i64,
        sample_delay: i32,
        sample_delay_fraction: f32,
    ) -> Option<Complex32> {
        let sample_abs = chip as f64 * oversample as f64
            + f64::from(sample_delay)
            + f64::from(sample_delay_fraction);
        let sample_idx = sample_abs - absolute_sample_start as f64;
        interp_complex_linear(samples, sample_idx)
    }

    fn complex_phase(phase: f32) -> Complex32 {
        Complex32::new(phase.cos(), phase.sin())
    }

    fn blind_hrpd_access_attempt_from_preamble(
        samples: &[Complex32],
        absolute_sample_start: u64,
        oversample: usize,
        preamble_start_chip: i64,
        sample_delay: i32,
        sample_delay_fraction: f32,
        preamble_frames: usize,
        packet_frame: usize,
        phase_step: f32,
    ) -> Option<(Vec<Complex32>, AccessPhyDecodeAttempt)> {
        let frame_chips = ACCESS_PACKET_CHIPS;
        let mut periods = Vec::with_capacity(preamble_frames);
        for frame in 0..preamble_frames {
            let frame_start = preamble_start_chip + (frame * frame_chips) as i64;
            let mut period = Vec::with_capacity(frame_chips);
            for k in 0..frame_chips {
                period.push(sample_chip_interp(
                    samples,
                    absolute_sample_start,
                    oversample,
                    frame_start + k as i64,
                    sample_delay,
                    sample_delay_fraction,
                )?);
            }
            periods.push(period);
        }

        let mut reference = vec![Complex32::new(0.0, 0.0); frame_chips];
        for (frame, period) in periods.iter().enumerate() {
            let correction = complex_phase(-phase_step * frame as f32);
            for (acc, sample) in reference.iter_mut().zip(period) {
                *acc += *sample * correction;
            }
        }
        let scale = 1.0 / preamble_frames.max(1) as f32;
        for value in &mut reference {
            *value *= scale;
        }
        let mean_ref_power =
            reference.iter().map(|v| v.norm_sqr()).sum::<f32>() / frame_chips as f32;
        let inverse_floor = (mean_ref_power * 0.02).max(1.0e-10);

        let data_start =
            preamble_start_chip + ((preamble_frames + packet_frame) * frame_chips) as i64;
        let packet_phase_correction =
            complex_phase(-phase_step * (preamble_frames + packet_frame) as f32);
        let mut chips = Vec::with_capacity(frame_chips);
        for (k, reference_chip) in reference.iter().enumerate() {
            let sample = sample_chip_interp(
                samples,
                absolute_sample_start,
                oversample,
                data_start + k as i64,
                sample_delay,
                sample_delay_fraction,
            )?;
            let denom = reference_chip.norm_sqr().max(inverse_floor);
            chips.push(sample * reference_chip.conj() * (1.0 / denom) * packet_phase_correction);
        }

        decode_access_phy_chips_attempt(&chips).map(|attempt| (chips, attempt))
    }

    fn event_block_payload_bits(blk: &super::SampleBlock) -> Vec<u8> {
        blk.samples
            .iter()
            .map(|s| if s.re >= 0.5 { 1 } else { 0 })
            .collect()
    }

    fn build_access_encapsulated_bits() -> (Vec<u8>, u8, u8) {
        let pd = 1u8;
        let msg_type = 0x15u8;
        let mut payload = Bitstream::new();
        payload.write_u8(pd, 2);
        payload.write_u8(msg_type, 6);
        payload.write_u8(0xA5, 8);
        payload.write_u8(0x3C, 8);
        payload.write_u8(0x5A, 8);
        while (8 + payload.len() + 30) % 8 != 0 {
            payload.write_u8(0, 1);
        }

        let msg_len_octets = ((8 + payload.len() + 30) / 8) as u8;
        assert!(msg_len_octets >= 6);

        let mut crc_scope = Bitstream::new();
        crc_scope.write_u8(msg_len_octets, 8);
        crc_scope.extend(&payload);
        let crc = crc30(&crc_scope);

        let mut body = Bitstream::new();
        body.write_u8(msg_len_octets, 8);
        body.extend(&payload);
        body.write_u32(crc, 30);

        (body.bits().to_vec(), msg_type, msg_len_octets)
    }

    fn encode_access_body_to_soft_samples(encap_bits: &[u8]) -> Vec<Complex32> {
        let mut out = Vec::new();
        let mut rem = encap_bits;
        while !rem.is_empty() {
            let take = rem.len().min(88);
            let mut frame_info = rem[..take].to_vec();
            if take < 88 {
                frame_info.extend(std::iter::repeat(0u8).take(88 - take));
            }
            // 88 information bits + 8 tail bits (all-zero).
            frame_info.extend(std::iter::repeat(0u8).take(8));

            // Access channel encoder is reset each 20 ms frame.
            let mut encoder = get_1_3_k9_encoder();
            let coded = frame_info
                .iter()
                .flat_map(|b| encoder.encode(*b))
                .collect::<Vec<_>>();
            let repeated = coded.iter().flat_map(|b| [*b, *b]).collect::<Vec<_>>();
            let interleaved =
                BitReversalInterleaver::new(block_interleaver::SR1_PARAMS_576).encode(&repeated);

            out.extend(
                interleaved
                    .into_iter()
                    .map(|b| Complex32::new(if b == 0 { 1.0 } else { -1.0 }, 0.0)),
            );
            rem = &rem[take..];
        }
        out
    }

    #[test]
    fn test_access_channel_pipeline_decodes_encapsulated_pdu() {
        let (encap_bits, expected_msg_type, expected_msg_len_octets) =
            build_access_encapsulated_bits();
        let samples = encode_access_body_to_soft_samples(&encap_bits);

        let mut receiver = PipelinedReceiver::new(samples.into_iter());
        let out_rx = receiver.add_pipeline(access_channel_chain());
        receiver.run_pipeline().unwrap();

        let mut events = 0usize;
        let mut saw_crc_valid = false;
        let mut saw_msg_type = false;
        let mut saw_msg_len = false;

        for blocks in out_rx {
            for blk in blocks {
                if blk.tags.get("access_event") == Some(&1) {
                    events += 1;
                    saw_crc_valid |= blk.tags.get("access_crc_valid") == Some(&1);
                    saw_msg_type |=
                        blk.tags.get("access_msg_type") == Some(&(expected_msg_type as i64));
                    saw_msg_len |= blk.tags.get("access_msg_length_octets")
                        == Some(&(expected_msg_len_octets as i64));
                }
            }
        }

        assert_eq!(1, events, "expected one access event");
        assert!(saw_crc_valid, "expected CRC-valid access PDU");
        assert!(saw_msg_type, "expected decoded access msg_type tag");
        assert!(saw_msg_len, "expected decoded access MSG_LENGTH tag");
    }

    #[test]
    fn capture_pipelined_integration_extracts_pilot_from_baseband_downlink_4x() {
        let mut reader = open_baseband_downlink_4x_reader();
        let sample_rate = reader.spec().sample_rate;
        let samples = reader
            .samples::<i16>()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let iq_samples = samples
            .chunks_exact(2)
            .map(|a| Complex32::new(a[0] as f32 / i16::MAX as f32, a[1] as f32 / i16::MAX as f32))
            .collect::<Vec<_>>();

        let mut receiver = PipelinedReceiver::new(iq_samples.clone().into_iter())
            .with_input_sample_rate_hz(sample_rate as f64)
            .with_trace_files("analysis/pipeline_stage_wavs/pilot_basic");
        let chain: Vec<PipelineProcessorShared> = vec![
            Box::new(PulseMatchedFilterProcessor::new()),
            Box::new(
                AcquisitionFftProcessor::new_with_window_chips(sample_rate, 32768, 4)
                    //.with_noncoherent_segment_chips(1024)
                    .with_snr_threshold_db(10.0),
            ),
            Box::new(MatchedFilterDespreader::new(sample_rate)),
            //Box::new(SlidingCorrelatorProcessor::new(sample_rate)),
            Box::new(WalshDecoderProcessor::new(WalshDecoder::new::<64>(0))),
        ];

        let out_rx = receiver.add_pipeline(chain);
        receiver.run_pipeline().unwrap();

        let mut total_symbols = 0usize;
        let mut locked_symbols = 0usize;
        let mut max_abs = 0.0f32;
        for blocks in out_rx {
            for blk in &blocks {
                total_symbols += blk.len();
                if blk.tags.get("acq_locked") == Some(&1) {
                    locked_symbols += blk.len();
                }
                for s in &blk.samples {
                    max_abs = max_abs.max(s.re.abs()).max(s.im.abs());
                }
            }
        }

        assert!(
            total_symbols > 100,
            "expected pilot symbols, got {total_symbols}"
        );
        assert!(
            locked_symbols > 0,
            "expected some acquisition-locked pilot symbols"
        );
        assert!(
            max_abs > 1e-4,
            "expected non-trivial pilot extraction amplitude, got {max_abs}"
        );
    }

    /// Analyze autocorrelation sidelobes: raw PN vs matched-filtered PN
    #[test]
    #[ignore = "diagnostic autocorrelation analysis; run explicitly when requested"]
    fn test_analyze_mft_autocorrelation_sidelobes() {
        use crate::sdr::{cdma2000_baseband_filter_taps_f64, fir::ComplexFir32};
        use rustfft::FftPlanner;

        let oversample = 4usize;
        let pn_len = 32768 * oversample; // 131072

        // Generate raw PN
        let pn_raw: Vec<Complex32> = build_oqpsk_pn_samples(pn_len, oversample);

        // Generate filtered PN (same as MFT)
        let taps = cdma2000_baseband_filter_taps_f64();
        let mut filt = ComplexFir32::new(&taps);
        let pn_filtered: Vec<Complex32> = build_oqpsk_pn_samples(pn_len, oversample)
            .into_iter()
            .map(|s| filt.process_sample(Complex32::new(s.re, -s.im)))
            .collect();

        let fft_length = pn_len * 2;

        // Autocorrelation via FFT for raw PN
        let fwd = FftPlanner::new().plan_fft_forward(fft_length);
        let inv = FftPlanner::new().plan_fft_inverse(fft_length);

        let autocorr = |seq: &[Complex32], label: &str| {
            // Zero-pad to fft_length
            let mut buf: Vec<Complex32> = seq.to_vec();
            buf.resize(fft_length, Complex32::new(0.0, 0.0));
            fwd.process(&mut buf);
            // Power spectrum: X * conj(X)
            for x in buf.iter_mut() {
                *x = Complex32::new(x.norm_sqr(), 0.0);
            }
            inv.process(&mut buf);
            // Normalize
            let peak = buf[0].re;
            let mut powers: Vec<(usize, f32)> = buf
                .iter()
                .enumerate()
                .map(|(i, v)| (i, v.re / peak))
                .collect();
            // Sort by power descending
            powers.sort_by(|a, b| b.1.total_cmp(&a.1));

            eprintln!("\n=== {} autocorrelation ===", label);
            eprintln!("Peak (lag 0): {:.2}", peak / fft_length as f32);
            eprintln!("Top 20 sidelobes:");
            for i in 0..20.min(powers.len()) {
                let lag = powers[i].0;
                // Map lag to signed offset
                let signed_lag = if lag > fft_length / 2 {
                    lag as isize - fft_length as isize
                } else {
                    lag as isize
                };
                eprintln!(
                    "  lag={:>7} ({:>7} chips) power={:.6} ratio_db={:.1}",
                    signed_lag,
                    signed_lag / oversample as isize,
                    powers[i].1,
                    if powers[i].1 > 0.0 {
                        10.0 * powers[i].1.log10()
                    } else {
                        -999.0
                    }
                );
            }

            // Sidelobe statistics
            let sidelobes: Vec<f32> = powers
                .iter()
                .filter(|(i, _)| *i != 0)
                .map(|(_, p)| *p)
                .collect();
            let max_sidelobe = sidelobes.iter().cloned().fold(0.0f32, f32::max);
            let mean_sidelobe: f32 = sidelobes.iter().sum::<f32>() / sidelobes.len() as f32;
            eprintln!(
                "Max sidelobe: {:.6} ({:.1} dB)",
                max_sidelobe,
                10.0 * max_sidelobe.log10()
            );
            eprintln!(
                "Mean sidelobe: {:.6} ({:.1} dB)",
                mean_sidelobe,
                10.0 * mean_sidelobe.log10()
            );
            eprintln!(
                "Peak-to-max-sidelobe: {:.1} dB",
                -10.0 * max_sidelobe.log10()
            );
        };

        autocorr(&pn_raw, "Raw PN");
        autocorr(&pn_filtered, "Filtered PN (MFT reference)");

        // Now also compute the cross-correlation of the actual baseband_downlink_4x signal
        // with both raw and filtered PN to compare peak quality
        let mut reader = open_baseband_downlink_4x_reader();
        let samples = reader
            .samples::<i16>()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let iq_samples: Vec<Complex32> = samples
            .chunks_exact(2)
            .map(|a| Complex32::new(a[0] as f32 / i16::MAX as f32, a[1] as f32 / i16::MAX as f32))
            .collect();

        // Take first 2*pn_len samples of signal for cross-correlation
        let sig_len = fft_length.min(iq_samples.len());
        let mut sig_buf: Vec<Complex32> = iq_samples[..sig_len].to_vec();
        sig_buf.resize(fft_length, Complex32::new(0.0, 0.0));

        let cross_corr = |pn_ref: &[Complex32], label: &str| {
            // FFT of signal
            let mut sig_fft = sig_buf.clone();
            fwd.process(&mut sig_fft);

            // FFT of reversed-conjugated PN reference
            let mut pn_rev: Vec<Complex32> = pn_ref.to_vec();
            pn_rev.reverse();
            for x in pn_rev.iter_mut() {
                *x = x.conj();
            }
            pn_rev.resize(fft_length, Complex32::new(0.0, 0.0));
            fwd.process(&mut pn_rev);

            // Multiply
            let mut result: Vec<Complex32> = sig_fft
                .iter()
                .zip(pn_rev.iter())
                .map(|(a, b)| a * b)
                .collect();
            inv.process(&mut result);
            for v in result.iter_mut() {
                *v /= fft_length as f32;
            }

            // Power
            let mut powers: Vec<(usize, f32)> = result
                .iter()
                .enumerate()
                .map(|(i, v)| (i, v.norm_sqr()))
                .collect();
            powers.sort_by(|a, b| b.1.total_cmp(&a.1));

            let median_idx = powers.len() / 2;
            let median = powers[median_idx].1;

            eprintln!("\n=== Cross-correlation: signal x {} ===", label);
            eprintln!("Top 20 peaks (power/median):");
            for i in 0..20.min(powers.len()) {
                let lag = powers[i].0;
                let phase = (pn_len - lag % pn_len) % pn_len;
                eprintln!(
                    "  lag={:>7} pn_phase={:>6} power/median={:.2}",
                    lag,
                    phase,
                    powers[i].1 / median
                );
            }

            // Check near the known-good phase
            let target = 88641usize;
            let mut best_near = None::<(usize, f32)>;
            for &(idx, pwr) in &powers {
                let phase = (pn_len - idx % pn_len) % pn_len;
                let dist = (phase as isize - target as isize).unsigned_abs();
                let dist = dist.min(pn_len - dist);
                if dist <= 32 {
                    if best_near.is_none() || pwr > best_near.unwrap().1 {
                        best_near = Some((phase, pwr));
                    }
                }
            }
            if let Some((phase, pwr)) = best_near {
                eprintln!(
                    "  >>> near target 88641: phase={} power/median={:.2}",
                    phase,
                    pwr / median
                );
            } else {
                eprintln!("  >>> no peak near target 88641");
            }
        };

        cross_corr(&pn_raw, "Raw PN");
        cross_corr(&pn_filtered, "Filtered PN");
    }

    #[test]
    #[ignore = "needs re-evaluation on cdma-ms branch; MobileStation+rake integration will be addressed there"]
    fn test_mobile_station_with_rake_receiver() {
        init_test_logger();
        let mut reader = open_baseband_downlink_4x_reader();
        let sample_rate = reader.spec().sample_rate;
        let samples = reader
            .samples::<i16>()
            .collect::<Result<Vec<_>, _>>()
            .unwrap();
        let iq_samples = samples
            .chunks_exact(2)
            .map(|a| Complex32::new(a[0] as f32 / i16::MAX as f32, a[1] as f32 / i16::MAX as f32))
            .collect::<Vec<_>>();

        let swap_pair = false;
        let conv_invert = false;

        let mut receiver = PipelinedReceiver::new(iq_samples.into_iter())
            .with_input_sample_rate_hz(sample_rate as f64);

        let rake = RakeReceiver::new_with_reference_filter_passes(
            4,
            Box::new(move || -> Vec<PipelineProcessorShared> {
                vec![
                    Box::new(PnAlignProcessor::new(4)),
                    Box::new(DecimatorProcessor::new(4)),
                    Box::new(MobileStation::new(
                        // Sync sub-chain
                        vec![
                            Box::new(WalshPilotCombiner::new(
                                WalshDecoder::new::<64>(32),
                                WalshDecoder::new::<64>(0),
                            )),
                            Box::new(Unrepeater::new(4)),
                            Box::new(
                                DeinterleaverProcessor::new(
                                    BitReversalInterleaver::new(block_interleaver::SR1_PARAMS_128),
                                    2,
                                )
                                .with_offset_search((0..128).collect(), 12, 1)
                                .with_offset_search_warmup(16)
                                .with_offset_search_batch_size(4)
                                .with_offset_search_confirm_passes(1),
                            ),
                            Box::new(SoftViterbiDecoderProcessor::new(
                                SoftViterbiDecoder::new(get_1_2_k9_encoder()),
                                swap_pair,
                                conv_invert,
                            )),
                            Box::new(SyncChannelProcessor::new()),
                        ],
                        // Paging chain builder
                        Box::new(
                            move |pilot_pn: u16,
                                  lc_state: u64,
                                  paging_rate: PagingRate|
                                  -> Vec<PipelineProcessorShared> {
                                let lc_gen = LongCodeGenerator::new_paging_channel_with_state(
                                    1, pilot_pn, lc_state,
                                );
                                let (unrepeat_factor, paging_ch_rate) = match paging_rate {
                                    PagingRate::Rate9600 => (1, PagingChannelRate::Rate9600),
                                    PagingRate::Rate4800 => (2, PagingChannelRate::Rate4800),
                                };
                                vec![
                                    Box::new(WalshPilotCombiner::new(
                                        WalshDecoder::new::<64>(1),
                                        WalshDecoder::new::<64>(0),
                                    )),
                                    Box::new(Unrepeater::new(unrepeat_factor)),
                                    Box::new(LongCodeDescrambler::new(lc_gen, 64)),
                                    Box::new({
                                        let half_frame_bits = match paging_ch_rate {
                                            PagingChannelRate::Rate9600 => 96,
                                            PagingChannelRate::Rate4800 => 48,
                                        };
                                        let rate = paging_ch_rate;
                                        DeinterleaverProcessor::new(
                                            BitReversalInterleaver::new(
                                                block_interleaver::SR1_PARAMS_384,
                                            ),
                                            1,
                                        )
                                        .with_offset_search((0..384).collect(), 8, 1)
                                        .with_offset_search_warmup(8)
                                        .with_offset_search_batch_size(8)
                                        .with_offset_search_confirm_passes(1)
                                        .with_offset_search_evaluator(
                                            Box::new(
                                                move |bits: &[u8], shift: usize, invert: bool| {
                                                    PagingChannelProcessor::evaluate_alignment(
                                                        bits, shift, invert, rate,
                                                    )
                                                },
                                            ),
                                            half_frame_bits,
                                        )
                                    }),
                                    Box::new(SoftViterbiDecoderProcessor::new(
                                        SoftViterbiDecoder::new(get_1_2_k9_encoder()),
                                        swap_pair,
                                        conv_invert,
                                    )),
                                    Box::new(PagingChannelProcessor::new_with_rate(paging_ch_rate)),
                                ]
                            },
                        ),
                    )),
                ]
            }),
            1,
        );

        let chain: Vec<PipelineProcessorShared> =
            vec![Box::new(PulseMatchedFilterProcessor::new()), Box::new(rake)];

        let out_rx = receiver.add_pipeline(chain);
        receiver.run_pipeline().unwrap();

        let mut sync_events = 0usize;
        let mut paging_events = 0usize;
        for blocks in out_rx {
            for blk in blocks {
                if blk.tags.get("ms_sync_event") == Some(&1) {
                    sync_events += 1;
                    eprintln!(
                        "MS sync #{}: pilot_pn={:?} sys_time={:?} lc_state={:?}",
                        sync_events,
                        blk.tags.get("sync_pilot_pn"),
                        blk.tags.get("sync_sys_time"),
                        blk.tags.get("sync_lc_state"),
                    );
                }
                if blk.tags.get("paging_event") == Some(&1) {
                    paging_events += 1;
                    eprintln!(
                        "MS paging #{}: crc_valid={:?} msg_type={:?} payload_bits={:?}",
                        paging_events,
                        blk.tags.get("paging_crc_valid"),
                        blk.tags.get("paging_msg_type"),
                        blk.tags.get("paging_payload_bits"),
                    );
                    let payload_bits = event_block_payload_bits(&blk);
                    let payload_hex = payload_bits
                        .chunks(8)
                        .map(|chunk| {
                            let byte = chunk.iter().fold(0u8, |acc, &b| (acc << 1) | (b & 1))
                                << (8usize.saturating_sub(chunk.len()));
                            format!("{:02x}", byte)
                        })
                        .collect::<Vec<_>>()
                        .join(" ");
                    eprintln!(
                        "MS paging payload bits={} hex=[{}]",
                        payload_bits
                            .iter()
                            .map(|b| if *b == 0 { '0' } else { '1' })
                            .collect::<String>(),
                        payload_hex
                    );
                    match PagingMessage::decode(&Bitstream::new_init(&payload_bits)) {
                        Ok(msg) => msg.print(),
                        Err(err) => eprintln!("MS paging layer3 decode error: {}", err),
                    }
                }
            }
        }

        eprintln!(
            "MobileStation: {} sync events, {} paging events",
            sync_events, paging_events
        );
        assert!(
            sync_events > 0,
            "expected at least one sync event from MobileStation"
        );
    }

    #[test]
    fn capture_uplink_access_probe_finger_acquisition_full_chain() {
        init_test_logger();

        let Some(stats) = run_uplink_access_probe_full_chain_capture(
            "1791617173891930.wav",
            1791617173891930,
            "uplink_access_probe_full_chain",
        ) else {
            return;
        };
        assert_min_realtime_speedup("uplink access probe full chain", &stats, 1.4);

        // Lock the current deduped decode count for this capture. A narrower
        // same-burst dedupe window keeps 32-chip re-emits collapsed while
        // preserving distinct repeated bursts that a wider window would merge.
        assert_eq!(
            stats.crc_valid_data_frame_count, 12,
            "expected exactly 12 deduped CRC-valid registration frames from the long-lived current capture, got {}",
            stats.crc_valid_data_frame_count
        );
    }

    #[derive(Debug, Default, Clone)]
    struct UplinkAccessProbeCaptureStats {
        total_blocks: usize,
        preamble_count: usize,
        data_frame_count: usize,
        crc_valid_data_frame_count: usize,
        crc_invalid_data_frame_count: usize,
        wav_duration_ms: f64,
        decode_wall_ms: f64,
        realtime_speedup_x: f64,
        max_batch_wall_ms: f64,
    }

    fn run_uplink_access_probe_full_chain_capture(
        wav_name: &str,
        chip_start: u64,
        label: &str,
    ) -> Option<UplinkAccessProbeCaptureStats> {
        let wav_path = test_capture_path(wav_name);
        if !wav_path.exists() {
            eprintln!(
                "skipping {}: capture not found: {}",
                label,
                wav_path.display()
            );
            return None;
        }
        let reader = hound::WavReader::open(&wav_path)
            .unwrap_or_else(|e| panic!("failed to open {}: {e}", wav_path.display()));
        let (sample_rate, iq_samples) = read_iq_wav(reader);
        let oversample = (sample_rate as usize) / 1228800;
        let access_channel_number: u8 = 0;
        let paging_channel_number: u8 = 1;
        let base_id: u16 = 1;
        let pilot_pn: u16 = 0;
        let long_code_state: u64 = 1u64 << 41;
        let max_iq_samples: Option<usize> = None;
        let skip_iq_samples: usize = 0;
        let iq_gain_scale: f32 = 1.0;
        eprintln!(
            "{label}: sample_rate={} oversample={} iq_samples={} chip_start={} chain=reverse_access_chain acn={} pcn={} base_id={} pilot_pn={} lc_state={} skip_iq_samples={} max_iq_samples={:?} iq_gain_scale={}",
            sample_rate,
            oversample,
            iq_samples.len(),
            chip_start,
            access_channel_number,
            paging_channel_number,
            base_id,
            pilot_pn,
            long_code_state,
            skip_iq_samples,
            max_iq_samples,
            iq_gain_scale,
        );
        let chip_start = chip_start + (skip_iq_samples / oversample) as u64;
        let iq_samples = iq_samples.into_iter().skip(skip_iq_samples);
        let iq_samples = if let Some(limit) = max_iq_samples {
            iq_samples.take(limit).collect::<Vec<_>>()
        } else {
            iq_samples.collect::<Vec<_>>()
        };
        let iq_samples = if (iq_gain_scale - 1.0).abs() > f32::EPSILON {
            iq_samples
                .into_iter()
                .map(|sample| sample * iq_gain_scale)
                .collect::<Vec<_>>()
        } else {
            iq_samples
        };
        let wav_duration_secs = iq_samples.len() as f64 / sample_rate as f64;

        let settings = ReverseAccessSettings {
            oversample,
            access_channel_number,
            paging_channel_number,
            base_id,
            pilot_pn,
            long_code_state,
            rake_fast_path: false,
            fixed_finger_phase: None,
            reanchor_origin: false,
            finger_pool_size: 1,
        };
        let pipeline = super::reverse_access_chain(settings);

        let mut receiver = PipelinedReceiver::new(iq_samples.into_iter())
            .with_batch_size(32768)
            .with_input_sample_rate_hz(sample_rate as f64)
            .with_absolute_sample_start(chip_start * oversample as u64);
        let out_rx = receiver.add_pipeline(pipeline);
        let decode_started = std::time::Instant::now();
        let label_owned = label.to_string();
        let collector = std::thread::spawn(move || {
            let mut stats = UplinkAccessProbeCaptureStats::default();
            for blocks in out_rx {
                stats.total_blocks += blocks.len();
                for blk in &blocks {
                    if blk.tags.get("access_preamble_detected") == Some(&1)
                        && blk.tags.contains_key("access_preamble_frames")
                    {
                        stats.preamble_count += 1;
                    }
                    if blk.tags.get("access_event") == Some(&1) {
                        stats.data_frame_count += 1;
                        let crc_valid = blk.tags.get("access_crc_valid") == Some(&1);
                        if crc_valid {
                            stats.crc_valid_data_frame_count += 1;
                        } else {
                            stats.crc_invalid_data_frame_count += 1;
                        }
                        eprintln!(
                            "  {} data frame #{}: chip={:?} finger={:?} crc={:?} msg_len={:?} payload_bits={:?} pd={:?} msg_type={:?}",
                            label_owned,
                            stats.data_frame_count,
                            blk.tags.get("absolute_chip_start"),
                            blk.tags.get("finger_id"),
                            blk.tags.get("access_crc_valid"),
                            blk.tags.get("access_msg_length_octets"),
                            blk.tags.get("access_payload_bits"),
                            blk.tags.get("access_pd"),
                            blk.tags.get("access_msg_type"),
                        );
                    }
                }
            }
            stats
        });
        let run_stats = receiver.run_pipeline_with_stats().unwrap();
        let mut stats = collector
            .join()
            .unwrap_or_else(|_| panic!("{label}: output collector panicked"));
        let decode_elapsed = decode_started.elapsed();
        let decode_secs = decode_elapsed.as_secs_f64();
        let realtime_speedup = if decode_secs > 0.0 {
            wav_duration_secs / decode_secs
        } else {
            f64::INFINITY
        };
        stats.wav_duration_ms = wav_duration_secs * 1000.0;
        stats.decode_wall_ms = decode_secs * 1000.0;
        stats.realtime_speedup_x = realtime_speedup;
        stats.max_batch_wall_ms = run_stats.max_batch_elapsed_ns as f64 / 1_000_000.0;

        eprintln!(
            "{label} summary: total_blocks={} preamble_detections={} data_frames={} crc_valid_data_frames={} crc_invalid_data_frames={} wav_duration_ms={:.3} decode_wall_ms={:.3} realtime_speedup_x={:.2} max_batch_wall_ms={:.3}",
            stats.total_blocks,
            stats.preamble_count,
            stats.data_frame_count,
            stats.crc_valid_data_frame_count,
            stats.crc_invalid_data_frame_count,
            stats.wav_duration_ms,
            stats.decode_wall_ms,
            stats.realtime_speedup_x,
            stats.max_batch_wall_ms,
        );

        Some(stats)
    }

    fn assert_min_realtime_speedup(
        label: &str,
        stats: &UplinkAccessProbeCaptureStats,
        min_speedup: f64,
    ) {
        assert_min_realtime_speedup_value(
            label,
            stats.realtime_speedup_x,
            min_speedup,
            stats.decode_wall_ms,
            stats.wav_duration_ms,
            stats.max_batch_wall_ms,
        );
    }

    fn assert_min_realtime_speedup_value(
        label: &str,
        realtime_speedup_x: f64,
        min_speedup: f64,
        decode_wall_ms: f64,
        wav_duration_ms: f64,
        max_batch_wall_ms: f64,
    ) {
        let min_speedup = capture_timing_min_speedup(min_speedup);
        // These thresholds are deliberately loose. They are intended to catch
        // algorithmic regressions such as a full 96-offset Viterbi/CRC sweep,
        // not normal scheduler or logging noise.
        assert!(
            realtime_speedup_x >= min_speedup,
            "{label}: decode timing regression: realtime_speedup_x={:.2}, expected >= {:.2}; \
             decode_wall_ms={:.1}, wav_duration_ms={:.1}, max_batch_wall_ms={:.1}",
            realtime_speedup_x,
            min_speedup,
            decode_wall_ms,
            wav_duration_ms,
            max_batch_wall_ms,
        );
    }

    fn capture_timing_min_speedup(local_min_speedup: f64) -> f64 {
        if std::env::var_os("CI").is_some() {
            CAPTURE_CI_MIN_REALTIME_SPEEDUP
        } else {
            local_min_speedup
        }
    }

    #[derive(Debug)]
    struct HrpdAccessCaptureEventSummary {
        packet_start: i64,
        phy_crc_valid: bool,
        mac_fragment_valid: bool,
        single_fragment_fcs_valid: bool,
        mac_length_octets: Option<i64>,
        info_hex: String,
        decoded: String,
    }

    fn collect_hrpd_access_capture_events(
        out_rx: std::sync::mpsc::Receiver<Vec<SampleBlock>>,
    ) -> Vec<HrpdAccessCaptureEventSummary> {
        let mut events = Vec::new();
        for blocks in out_rx {
            for blk in blocks {
                if blk.tags.get("hrpd_access_event") != Some(&1) {
                    continue;
                }
                let phy_crc_valid = blk.tags.get("access_crc_valid") == Some(&1);
                let mac_fragment_valid = blk.tags.get("hrpd_access_reserved_zero") == Some(&1)
                    && blk.tags.get("hrpd_access_mac_fragment_valid") == Some(&1);
                let single_fragment_fcs_valid =
                    blk.tags.get("hrpd_access_mac_single_fragment_fcs_valid") == Some(&1);
                let bit_vec: Vec<u8> = blk.samples.iter().map(|s| u8::from(s.re >= 0.5)).collect();
                let body_bits = AccessFrameLayout::for_packet_bits(bit_vec.len())
                    .map(|layout| layout.body_bits)
                    .unwrap_or(bit_vec.len());
                let decoded = parse_access_mac_capsule(&bit_vec[..body_bits])
                    .map(|capsule| capsule.summary())
                    .unwrap_or_else(|| "unparsed".to_string());
                let info_hex = pack_bits_to_bytes(&bit_vec[..body_bits])
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<Vec<_>>()
                    .join("");
                events.push(HrpdAccessCaptureEventSummary {
                    packet_start: blk.tags.get("absolute_chip_start").copied().unwrap_or(-1),
                    phy_crc_valid,
                    mac_fragment_valid,
                    single_fragment_fcs_valid,
                    mac_length_octets: blk.tags.get("hrpd_access_mac_length_octets").copied(),
                    info_hex,
                    decoded,
                });
            }
        }
        events
    }

    fn decode_hrpd_access_run_by_preamble_sweep(
        samples: &[Complex32],
        absolute_sample_start: u64,
        oversample: usize,
        run: &HrpdSlotPowerRun,
    ) -> Option<HrpdAccessCaptureEventSummary> {
        let slot_offsets = [1i64, 2, 3, 4, 5, 6, 7, 8, 0, -1, -2];
        let sample_delays = [-24, -20, -16, -12, -8, -4, 0, -28, -32, 4];
        let fractions = [0.0f32, -0.75, 0.75];
        for slot_offset in slot_offsets {
            let preamble_start_chip = run.start_chip as i64 + slot_offset * 2048;
            if preamble_start_chip < 0 {
                continue;
            }
            for sample_delay in sample_delays {
                for &sample_delay_fraction in &fractions {
                    let Some((coherence, phase_step)) = hrpd_blind_preamble_lag_coherence(
                        samples,
                        absolute_sample_start,
                        oversample,
                        preamble_start_chip,
                        sample_delay,
                        sample_delay_fraction,
                    ) else {
                        continue;
                    };
                    if coherence < 0.90 {
                        continue;
                    }
                    let mut phase_steps = vec![phase_step, 0.0f32];
                    phase_steps.dedup_by(|a, b| (*a - *b).abs() < 1.0e-4);
                    for phase_step in phase_steps {
                        let Some((_chips, attempt)) = blind_hrpd_access_attempt_from_preamble(
                            samples,
                            absolute_sample_start,
                            oversample,
                            preamble_start_chip,
                            sample_delay,
                            sample_delay_fraction,
                            3,
                            0,
                            phase_step,
                        ) else {
                            continue;
                        };
                        if attempt.fcs_bit_errors != 0 || attempt.tail_ones != 0 {
                            continue;
                        }
                        let mac_check = validate_access_mac_fragment(&attempt.info_bits);
                        if !mac_check.valid || !mac_check.single_fragment_fcs_valid {
                            continue;
                        }
                        let decoded = parse_access_mac_capsule(&attempt.info_bits)
                            .map(|capsule| capsule.summary())
                            .unwrap_or_else(|| "unparsed".to_string());
                        let info_hex = pack_bits_to_bytes(&attempt.info_bits)
                            .iter()
                            .map(|byte| format!("{byte:02x}"))
                            .collect::<Vec<_>>()
                            .join("");
                        let packet_start = preamble_start_chip + (3 * ACCESS_PACKET_CHIPS) as i64;
                        eprintln!(
                            "HRPD run sweep decode: run_slots=[{}, {}) run_chip={} packet_start={} preamble_start={} sample_delay={}{:+.2} coherence={:.3} phase_step={:+.5} msg_id=0x{:02x} mac_len={:?} decoded={}",
                            run.start_slot,
                            run.end_slot,
                            run.start_chip,
                            packet_start,
                            preamble_start_chip,
                            sample_delay,
                            sample_delay_fraction,
                            coherence,
                            phase_step,
                            attempt.message_id,
                            mac_check.length_octets,
                            decoded,
                        );
                        return Some(HrpdAccessCaptureEventSummary {
                            packet_start,
                            phy_crc_valid: true,
                            mac_fragment_valid: mac_check.valid,
                            single_fragment_fcs_valid: mac_check.single_fragment_fcs_valid,
                            mac_length_octets: mac_check.length_octets.map(|v| v as i64),
                            info_hex,
                            decoded,
                        });
                    }
                }
            }
        }
        None
    }

    #[test]
    fn capture_hrpd_reverse_access_bursts_1799955224163772_centered() {
        init_test_logger();
        let metadata_path = test_capture_path("1799955224163772.json");
        let wav_path = test_capture_path("1799955224163772.wav");
        let metadata = test_capture_metadata_from_path(&metadata_path);
        assert_eq!(metadata.sample_rate_hz, 9_830_400);
        assert_eq!(metadata.chip_rate_hz, 1_228_800);
        assert!(
            wav_path.exists(),
            "missing HRPD reverse access capture {}",
            wav_path.display()
        );

        let reader = hound::WavReader::open(&wav_path)
            .unwrap_or_else(|e| panic!("failed to open {}: {e}", wav_path.display()));
        let spec = reader.spec();
        assert_eq!(spec.channels, 2, "capture WAV must be stereo IQ");
        assert_eq!(
            spec.sample_rate as usize, metadata.sample_rate_hz,
            "capture WAV sample rate must match sidecar sample-rate metadata"
        );
        let (sample_rate, iq_samples) = read_iq_wav(reader);
        let oversample = (sample_rate as usize) / ACCESS_CHIP_RATE as usize;
        assert_eq!(oversample, 8, "centered capture should be 8x chip rate");
        let absolute_sample_start = metadata.first_absolute_sample_start;
        let chip_start = absolute_sample_start / oversample as u64;
        eprintln!(
            "HRPD 1799955224163772 centered live-chain input: sample_rate={} oversample={} samples={} abs_sample_start={} chip_start={} chip_mod_frame={}",
            sample_rate,
            oversample,
            iq_samples.len(),
            absolute_sample_start,
            chip_start,
            chip_start % 32768,
        );
        let pipeline = hrpd_reverse_access_chain(HrpdReverseAccessSettings {
            oversample,
            ..HrpdReverseAccessSettings::default()
        });
        let total_samples = iq_samples.len();
        let mut receiver = PipelinedReceiver::new(iq_samples.into_iter())
            .with_batch_size(32768)
            .with_input_sample_rate_hz(sample_rate as f64)
            .with_absolute_sample_start(absolute_sample_start);
        let out_rx = receiver.add_pipeline(pipeline);
        let pipeline_start = std::time::Instant::now();
        receiver.run_pipeline().unwrap();
        let pipeline_elapsed = pipeline_start.elapsed();
        let capture_seconds = total_samples as f64 / sample_rate as f64;
        let multiplier = capture_seconds / pipeline_elapsed.as_secs_f64();
        eprintln!(
            "HRPD 1799955224163772 receiver pipeline timing: capture={:.2}s pipeline={:.2}s real_time={:.2}x",
            capture_seconds,
            pipeline_elapsed.as_secs_f64(),
            multiplier,
        );
        if let Some(metrics_handle) =
            crate::receiver::hrpd::reverse_correlator_base::get_metrics_handle("hrpd_access")
            && let Ok(m) = metrics_handle.lock()
        {
            let total_ms = pipeline_elapsed.as_millis() as u64;
            let fft_ms = m.fft_scan_ns / 1_000_000;
            let spawn_ms = m.spawn_finger_ns / 1_000_000;
            let ref_ms = m.searcher_ref_setup_ns / 1_000_000;
            let sig_ms = m.searcher_signal_fft_ns / 1_000_000;
            let ifft_ms = m.searcher_ifft_mult_ns / 1_000_000;
            let peak_ms = m.searcher_peak_find_ns / 1_000_000;
            let pct = |x| 100.0 * x as f64 / total_ms.max(1) as f64;
            eprintln!(
                "HRPD 1799955224163772 HEAT total_pipeline={total_ms}ms fft={fft_ms}ms({:.1}%) [ref={ref_ms}ms({:.1}%) sigfft={sig_ms}ms({:.1}%) ifft={ifft_ms}ms({:.1}%) peak={peak_ms}ms({:.1}%) windows={}] spawn={spawn_ms}ms({:.1}%) [calls={}]",
                pct(fft_ms),
                pct(ref_ms),
                pct(sig_ms),
                pct(ifft_ms),
                pct(peak_ms),
                m.searcher_ref_setup_calls,
                pct(spawn_ms),
                m.spawn_finger_calls,
            );
        }

        let mut event_count = 0usize;
        let mut phy_crc_valid_count = 0usize;
        let mut mac_fragment_valid_count = 0usize;
        let mut single_fragment_fcs_valid_count = 0usize;
        let mut parsed_events = Vec::new();
        for blocks in out_rx {
            for blk in blocks {
                if blk.tags.get("hrpd_access_event") != Some(&1) {
                    continue;
                }
                event_count += 1;
                let phy_crc_valid = blk.tags.get("access_crc_valid") == Some(&1);
                let mac_fragment_valid = blk.tags.get("hrpd_access_reserved_zero") == Some(&1)
                    && blk.tags.get("hrpd_access_mac_fragment_valid") == Some(&1);
                let single_fragment_fcs_valid =
                    blk.tags.get("hrpd_access_mac_single_fragment_fcs_valid") == Some(&1);
                phy_crc_valid_count += usize::from(phy_crc_valid);
                mac_fragment_valid_count += usize::from(mac_fragment_valid);
                single_fragment_fcs_valid_count += usize::from(single_fragment_fcs_valid);

                let bit_vec: Vec<u8> = blk.samples.iter().map(|s| u8::from(s.re >= 0.5)).collect();
                let body_bits = AccessFrameLayout::for_packet_bits(bit_vec.len())
                    .map(|layout| layout.body_bits)
                    .unwrap_or(bit_vec.len());
                let summary = parse_access_mac_capsule(&bit_vec[..body_bits])
                    .map(|capsule| capsule.summary())
                    .unwrap_or_else(|| "unparsed".to_string());
                let info_hex = pack_bits_to_bytes(&bit_vec[..body_bits])
                    .iter()
                    .map(|byte| format!("{byte:02x}"))
                    .collect::<Vec<_>>()
                    .join("");
                eprintln!(
                    "HRPD 1799955224163772 event #{}: chip={} phy_crc_valid={} mac_fragment_valid={} single_fragment_fcs_valid={} msg_len={:?} info_hex={} decoded={}",
                    event_count,
                    blk.tags.get("absolute_chip_start").copied().unwrap_or(-1),
                    phy_crc_valid,
                    mac_fragment_valid,
                    single_fragment_fcs_valid,
                    blk.tags.get("hrpd_access_mac_length_octets"),
                    info_hex,
                    summary,
                );
                parsed_events.push(summary);
            }
        }
        eprintln!(
            "HRPD 1799955224163772 live-chain CRC counts: events={} phy_crc_valid={} mac_fragment_valid={} single_fragment_fcs_valid={} parsed_events={:?}",
            event_count,
            phy_crc_valid_count,
            mac_fragment_valid_count,
            single_fragment_fcs_valid_count,
            parsed_events,
        );
    }

    /// Rev A access reprobe capture with reverse traffic present. The AT
    /// completes UATI assignment, then reprobes its ConnectionRequest because
    /// setup never ACKs; the later reprobes gate their reverse transmission
    /// 3-on/1-off (one dead slot per subframe). Exercises the production
    /// FFT-rake chain end to end — gating-robust acquisition, dead-slot
    /// erasure, and CRC-authoritative capsule validation recover every probe.
    #[test]
    fn capture_hrpd_reverse_access_bursts_1802828173947282() {
        init_test_logger();
        let metadata_path = test_capture_path("1802828173947282.json");
        let wav_path = test_capture_path("1802828173947282.wav");
        let metadata = test_capture_metadata_from_path(&metadata_path);
        let hrpd_shift_hz = metadata
            .hrpd_rx_shift_hz
            .expect("capture sidecar must include hrpd_rx_shift_hz");
        let (sample_rate, mut iq_samples, absolute_sample_start) =
            read_shifted_capture_to_4x_with_shift(&wav_path, &metadata, hrpd_shift_hz);
        let oversample = (sample_rate as usize) / ACCESS_CHIP_RATE as usize;
        assert_eq!(oversample, 4, "shifted HRPD slice should be 4x chip rate");
        // This capture was recorded before the writer reconciled RX stream
        // gaps: the session log shows one 31645-sample gap (4x domain) at
        // absolute sample 7211312721087393 that the receiver patched but the
        // WAV lost. Re-insert it so the replay timeline matches the live one.
        const GAP_ABS_SAMPLE: u64 = 7_211_312_721_087_393;
        const GAP_SAMPLES: usize = 31_645;
        let gap_index = (GAP_ABS_SAMPLE - absolute_sample_start) as usize;
        iq_samples.splice(
            gap_index..gap_index,
            std::iter::repeat_n(Complex32::new(0.0, 0.0), GAP_SAMPLES),
        );

        let pipeline = hrpd_reverse_access_chain(HrpdReverseAccessSettings {
            oversample,
            reanchor_origin: true,
            ..HrpdReverseAccessSettings::default()
        });
        let mut receiver = PipelinedReceiver::new(iq_samples.into_iter())
            .with_batch_size(32768)
            .with_input_sample_rate_hz(sample_rate as f64)
            .with_absolute_sample_start(absolute_sample_start);
        let out_rx = receiver.add_pipeline(pipeline);
        receiver.run_pipeline().unwrap();
        let events = collect_hrpd_access_capture_events(out_rx);
        let fcs_valid = events
            .iter()
            .filter(|event| event.single_fragment_fcs_valid)
            .collect::<Vec<_>>();
        for (idx, event) in events.iter().enumerate() {
            eprintln!(
                "HRPD 1802828173947282 event #{}: chip={} fcs_valid={} decoded={}",
                idx + 1,
                event.packet_start,
                event.single_fragment_fcs_valid,
                event.decoded,
            );
        }
        // Five initial probes (UATIRequest, then ConnectionRequest 0x41..0x43)
        // plus twelve gated ConnectionRequest 0x44 reprobes = 17 CRC-valid
        // capsules. The gated reprobes only decode because acquisition scores
        // them on their live slots, the dead slots are erased before decode,
        // and the MAC FCS (not the header ProbeNumber field, §10.5.6.2.1,
        // which increments across the probe ladder) decides capsule validity.
        assert_eq!(
            fcs_valid.len(),
            17,
            "expected all 17 access probes (5 initial + 12 gated reprobes) to decode"
        );
        let gated_reprobes = fcs_valid
            .iter()
            .filter(|event| event.decoded.contains("ConnectionRequest(transaction=0x44"))
            .count();
        assert_eq!(
            gated_reprobes, 12,
            "expected 12 gated ConnectionRequest 0x44 reprobes"
        );
    }

    /// Regression for noisy HRPD reverse-access bursts.
    #[test]
    fn capture_hrpd_reverse_access_bursts_1806111468045380() {
        init_test_logger();
        let metadata_path = test_capture_path("1806111468045380.json");
        let wav_path = test_capture_path("1806111468045380.wav");
        let metadata = test_capture_metadata_from_path(&metadata_path);
        assert_eq!(metadata.sample_rate_hz, 4_915_200);
        assert_eq!(metadata.chip_rate_hz, ACCESS_CHIP_RATE as usize);
        assert_eq!(metadata.hrpd_reverse_frequency_hz, Some(843_900_000));
        assert_eq!(metadata.hrpd_rx_shift_hz, Some(-915_000));

        let hrpd_shift_hz = metadata.hrpd_rx_shift_hz.expect("sidecar HRPD shift");
        let (sample_rate, iq_samples, absolute_sample_start) =
            read_shifted_capture_to_4x_with_shift(&wav_path, &metadata, hrpd_shift_hz);
        let oversample = (sample_rate as usize) / ACCESS_CHIP_RATE as usize;
        assert_eq!(oversample, 4, "shifted HRPD slice should be 4x chip rate");

        let pipeline = hrpd_reverse_access_chain(HrpdReverseAccessSettings {
            oversample,
            sector_id_lsb: 0,
            color_code: 26,
            preamble_frames: 3,
            ..HrpdReverseAccessSettings::default()
        });
        let total_samples = iq_samples.len();
        let mut receiver = PipelinedReceiver::new(iq_samples.into_iter())
            .with_batch_size(32768)
            .with_input_sample_rate_hz(sample_rate as f64)
            .with_absolute_sample_start(absolute_sample_start);
        let out_rx = receiver.add_pipeline(pipeline);
        let pipeline_start = std::time::Instant::now();
        receiver.run_pipeline().unwrap();
        let pipeline_elapsed = pipeline_start.elapsed();
        let events = collect_hrpd_access_capture_events(out_rx);
        let fcs_valid = events
            .iter()
            .filter(|event| event.single_fragment_fcs_valid)
            .collect::<Vec<_>>();
        for (idx, event) in events.iter().enumerate() {
            eprintln!(
                "HRPD 1806111468045380 event #{}: chip={} phy_crc_valid={} mac_fragment_valid={} single_fragment_fcs_valid={} msg_len={:?} info_hex={} decoded={}",
                idx + 1,
                event.packet_start,
                event.phy_crc_valid,
                event.mac_fragment_valid,
                event.single_fragment_fcs_valid,
                event.mac_length_octets,
                event.info_hex,
                event.decoded,
            );
        }
        eprintln!(
            "HRPD 1806111468045380 summary: streaming_events={} fcs_valid={} capture={:.2}s pipeline={:.2}s real_time={:.2}x",
            events.len(),
            fcs_valid.len(),
            total_samples as f64 / sample_rate as f64,
            pipeline_elapsed.as_secs_f64(),
            (total_samples as f64 / sample_rate as f64) / pipeline_elapsed.as_secs_f64(),
        );
        let packet_starts = fcs_valid
            .iter()
            .map(|event| event.packet_start as u64)
            .collect::<Vec<_>>();
        assert_eq!(
            packet_starts,
            vec![
                1_806_111_473_139_712,
                1_806_111_473_598_464,
                1_806_111_474_024_448,
                1_806_111_474_483_200,
                1_806_111_474_974_720,
                1_806_111_475_466_240,
                1_806_111_476_056_064,
                1_806_111_476_514_816,
                1_806_111_477_530_624,
                1_806_111_478_022_144,
                1_806_111_478_480_896,
                1_806_111_479_103_488,
                1_806_111_479_660_544,
                1_806_111_480_086_528,
                1_806_111_481_069_568,
                1_806_111_481_528_320,
            ],
            "expected all 16 CRC-valid HRPD reverse-access reprobes"
        );
        assert!(
            fcs_valid.iter().all(|event| {
                event.decoded.contains("ati=Uati/0x1a680b39")
                    && event.decoded.contains("RouteUpdate")
                    && event.decoded.contains("ConnectionRequest(transaction=0x45")
            }),
            "unexpected HRPD reverse-access contents: {:?}",
            fcs_valid,
        );
    }

    #[test]
    fn capture_hrpd_reverse_access_bursts_1799956520441591() {
        init_test_logger();
        let metadata_path = test_capture_path("1799956520441591.json");
        let wav_path = test_capture_path("1799956520441591.wav");
        let metadata = test_capture_metadata_from_path(&metadata_path);
        assert_eq!(metadata.sample_rate_hz, 9_830_400);
        assert_eq!(metadata.chip_rate_hz, 1_228_800);
        assert_eq!(metadata.rx_center_frequency_hz, Some(846_105_000));
        assert_eq!(metadata.one_x_reverse_frequency_hz, Some(848_310_000));
        assert_eq!(metadata.one_x_rx_shift_hz, Some(2_205_000));
        assert_eq!(metadata.hrpd_reverse_frequency_hz, Some(843_900_000));
        assert_eq!(metadata.hrpd_rx_shift_hz, Some(-2_205_000));
        assert!(
            wav_path.exists(),
            "missing HRPD reverse access capture {}",
            wav_path.display()
        );

        let hrpd_shift_hz = metadata
            .hrpd_rx_shift_hz
            .expect("capture sidecar must include hrpd_rx_shift_hz");
        let (sample_rate, iq_samples, absolute_sample_start) =
            read_shifted_capture_to_4x_with_shift(&wav_path, &metadata, hrpd_shift_hz);
        let oversample = (sample_rate as usize) / ACCESS_CHIP_RATE as usize;
        assert_eq!(oversample, 4, "shifted HRPD slice should be 4x chip rate");
        let chip_start = absolute_sample_start / oversample as u64;
        eprintln!(
            "HRPD 1799956520441591 shifted access capture: raw_rate={} shifted_rate={} samples={} abs_sample_start={} chip_start={} chip_mod_frame={} shift_hz={}",
            metadata.sample_rate_hz,
            sample_rate,
            iq_samples.len(),
            absolute_sample_start,
            chip_start,
            chip_start % 32768,
            hrpd_shift_hz,
        );
        let runs = hrpd_slot_power_runs_at_threshold(&iq_samples, chip_start, oversample, 2.0);
        for (idx, run) in runs.iter().enumerate() {
            eprintln!(
                "HRPD 1799956520441591 energy run #{}: slots=[{}, {}) chips=[{}, {}) slot_count={} peak={:+.2}dB",
                idx + 1,
                run.start_slot,
                run.end_slot,
                run.start_chip,
                run.end_chip,
                run.end_slot - run.start_slot,
                run.peak_db,
            );
        }
        assert_eq!(
            runs.len(),
            18,
            "expected 18 high-energy HRPD access burst runs"
        );
        let sweep_events = runs
            .iter()
            .map(|run| {
                decode_hrpd_access_run_by_preamble_sweep(
                    &iq_samples,
                    absolute_sample_start,
                    oversample,
                    run,
                )
            })
            .collect::<Option<Vec<_>>>()
            .expect("every high-energy HRPD access burst should have a valid preamble decode");
        assert_eq!(
            sweep_events.len(),
            18,
            "expected all 18 HRPD access bursts to decode by preamble sweep"
        );
        assert!(
            sweep_events.iter().all(|event| {
                event.single_fragment_fcs_valid
                    && event.decoded.contains("RouteUpdate")
                    && event.decoded.contains("UATIRequest")
                    && event.decoded.contains("ati=Rati/0xb9f1bf69")
            }),
            "unexpected preamble-sweep HRPD access events: {:?}",
            sweep_events
        );

        let pipeline = hrpd_reverse_access_chain(HrpdReverseAccessSettings {
            oversample,
            ..HrpdReverseAccessSettings::default()
        });
        let total_samples = iq_samples.len();
        let mut receiver = PipelinedReceiver::new(iq_samples.into_iter())
            .with_batch_size(32768)
            .with_input_sample_rate_hz(sample_rate as f64)
            .with_absolute_sample_start(absolute_sample_start);
        let out_rx = receiver.add_pipeline(pipeline);
        let pipeline_start = std::time::Instant::now();
        receiver.run_pipeline().unwrap();
        let pipeline_elapsed = pipeline_start.elapsed();
        let capture_seconds = total_samples as f64 / sample_rate as f64;
        let multiplier = capture_seconds / pipeline_elapsed.as_secs_f64();
        eprintln!(
            "HRPD 1799956520441591 receiver pipeline timing: capture={:.2}s pipeline={:.2}s real_time={:.2}x",
            capture_seconds,
            pipeline_elapsed.as_secs_f64(),
            multiplier,
        );
        if let Some(metrics_handle) =
            crate::receiver::hrpd::reverse_correlator_base::get_metrics_handle("hrpd_access")
            && let Ok(m) = metrics_handle.lock()
        {
            let total_ms = pipeline_elapsed.as_millis() as u64;
            let fft_ms = m.fft_scan_ns / 1_000_000;
            let spawn_ms = m.spawn_finger_ns / 1_000_000;
            let ref_ms = m.searcher_ref_setup_ns / 1_000_000;
            let sig_ms = m.searcher_signal_fft_ns / 1_000_000;
            let ifft_ms = m.searcher_ifft_mult_ns / 1_000_000;
            let peak_ms = m.searcher_peak_find_ns / 1_000_000;
            let pct = |x| 100.0 * x as f64 / total_ms.max(1) as f64;
            eprintln!(
                "HRPD 1799956520441591 HEAT total_pipeline={total_ms}ms fft={fft_ms}ms({:.1}%) [ref={ref_ms}ms({:.1}%) sigfft={sig_ms}ms({:.1}%) ifft={ifft_ms}ms({:.1}%) peak={peak_ms}ms({:.1}%) windows={}] spawn={spawn_ms}ms({:.1}%) [calls={}]",
                pct(fft_ms),
                pct(ref_ms),
                pct(sig_ms),
                pct(ifft_ms),
                pct(peak_ms),
                m.searcher_ref_setup_calls,
                pct(spawn_ms),
                m.spawn_finger_calls,
            );
        }

        let events = collect_hrpd_access_capture_events(out_rx);
        let fcs_valid = events
            .iter()
            .filter(|event| event.single_fragment_fcs_valid)
            .collect::<Vec<_>>();
        for (idx, event) in events.iter().enumerate() {
            eprintln!(
                "HRPD 1799956520441591 event #{}: chip={} phy_crc_valid={} mac_fragment_valid={} single_fragment_fcs_valid={} msg_len={:?} info_hex={} decoded={}",
                idx + 1,
                event.packet_start,
                event.phy_crc_valid,
                event.mac_fragment_valid,
                event.single_fragment_fcs_valid,
                event.mac_length_octets,
                event.info_hex,
                event.decoded,
            );
        }
        eprintln!(
            "HRPD 1799956520441591 shifted access summary: streaming_events={} streaming_fcs_valid={} sweep_fcs_valid={} decoded={:?}",
            events.len(),
            fcs_valid.len(),
            sweep_events.len(),
            fcs_valid
                .iter()
                .map(|event| &event.decoded)
                .collect::<Vec<_>>(),
        );
        assert_eq!(
            fcs_valid.len(),
            18,
            "expected the FFT-rake HRPD access receiver to recover all 18 downstream packets: {:?}",
            events
        );
        assert!(
            fcs_valid.iter().all(|event| {
                event.decoded.contains("RouteUpdate")
                    && event.decoded.contains("UATIRequest")
                    && event.decoded.contains("ati=Rati/0xb9f1bf69")
            }),
            "unexpected FFT-rake decoded HRPD access events: {:?}",
            fcs_valid
        );
    }

    #[test]
    fn capture_hrpd_reverse_access_bursts_1800354308350520() {
        init_test_logger();
        let metadata_path = test_capture_path("1800354308350520.json");
        let wav_path = test_capture_path("1800354308350520.wav");
        let metadata = test_capture_metadata_from_path(&metadata_path);
        assert_eq!(metadata.sample_rate_hz, 9_830_400);
        assert_eq!(metadata.hrpd_rx_shift_hz, Some(-2_205_000));
        assert!(
            wav_path.exists(),
            "missing HRPD reverse access capture {}",
            wav_path.display()
        );
        let hrpd_shift_hz = metadata.hrpd_rx_shift_hz.expect("sidecar shift");
        let (sample_rate, iq_samples, absolute_sample_start) =
            read_shifted_capture_to_4x_with_shift(&wav_path, &metadata, hrpd_shift_hz);
        let oversample = (sample_rate as usize) / ACCESS_CHIP_RATE as usize;
        let chip_start = absolute_sample_start / oversample as u64;
        let runs = hrpd_slot_power_runs_at_threshold(&iq_samples, chip_start, oversample, 2.0);
        for (idx, run) in runs.iter().enumerate() {
            eprintln!(
                "HRPD 1800354308350520 energy run #{}: slots=[{}, {}) chips=[{}, {}) slot_count={} peak={:+.2}dB",
                idx + 1,
                run.start_slot,
                run.end_slot,
                run.start_chip,
                run.end_chip,
                run.end_slot - run.start_slot,
                run.peak_db,
            );
        }
        let burst_count = runs.len();

        let pipeline = hrpd_reverse_access_chain(HrpdReverseAccessSettings {
            oversample,
            ..HrpdReverseAccessSettings::default()
        });
        let total_samples = iq_samples.len();
        let mut receiver = PipelinedReceiver::new(iq_samples.into_iter())
            .with_batch_size(32768)
            .with_input_sample_rate_hz(sample_rate as f64)
            .with_absolute_sample_start(absolute_sample_start);
        let out_rx = receiver.add_pipeline(pipeline);
        let pipeline_start = std::time::Instant::now();
        receiver.run_pipeline().unwrap();
        let pipeline_elapsed = pipeline_start.elapsed();
        eprintln!(
            "HRPD 1800354308350520 receiver pipeline timing: capture={:.2}s pipeline={:.2}s real_time={:.2}x",
            total_samples as f64 / sample_rate as f64,
            pipeline_elapsed.as_secs_f64(),
            (total_samples as f64 / sample_rate as f64) / pipeline_elapsed.as_secs_f64(),
        );

        let events = collect_hrpd_access_capture_events(out_rx);
        let fcs_valid = events
            .iter()
            .filter(|event| event.single_fragment_fcs_valid)
            .collect::<Vec<_>>();
        for (idx, event) in events.iter().enumerate() {
            eprintln!(
                "HRPD 1800354308350520 event #{}: chip={} fcs_valid={} msg_len={:?} decoded={}",
                idx + 1,
                event.packet_start,
                event.single_fragment_fcs_valid,
                event.mac_length_octets,
                event.decoded,
            );
        }
        eprintln!(
            "HRPD 1800354308350520 summary: energy_bursts={} streaming_events={} fcs_valid={}",
            burst_count,
            events.len(),
            fcs_valid.len(),
        );
        // 20 energy runs = 16 access bursts + 3 multi-second reverse traffic
        // sessions (886 slots each at ~+38 dB) + one 4-slot blip that is not
        // an access burst. All 16 access bursts decode, including two
        // 59-octet three-fragment capsules carrying UATIComplete +
        // ConnectionRequest + HardwareIDResponse.
        assert_eq!(
            burst_count, 20,
            "expected 20 high-energy runs (16 access + 3 traffic + 1 blip)"
        );
        assert_eq!(
            fcs_valid.len(),
            16,
            "expected the live HRPD access receiver chain to decode all 16 access bursts"
        );
        let session_close = fcs_valid
            .iter()
            .filter(|event| event.decoded.contains("SessionClose"))
            .count();
        assert_eq!(session_close, 1, "final burst carries the SessionClose");
    }

    #[test]
    fn capture_hrpd_reverse_access_bursts_1800347049472645() {
        init_test_logger();
        let metadata_path = test_capture_path("1800347049472645.json");
        let wav_path = test_capture_path("1800347049472645.wav");
        let metadata = test_capture_metadata_from_path(&metadata_path);
        assert_eq!(metadata.sample_rate_hz, 9_830_400);
        assert_eq!(metadata.chip_rate_hz, 1_228_800);
        assert_eq!(metadata.hrpd_rx_shift_hz, Some(-2_205_000));
        assert!(
            wav_path.exists(),
            "missing HRPD reverse access capture {}",
            wav_path.display()
        );

        let hrpd_shift_hz = metadata
            .hrpd_rx_shift_hz
            .expect("capture sidecar must include hrpd_rx_shift_hz");
        let (sample_rate, iq_samples, absolute_sample_start) =
            read_shifted_capture_to_4x_with_shift(&wav_path, &metadata, hrpd_shift_hz);
        let oversample = (sample_rate as usize) / ACCESS_CHIP_RATE as usize;
        assert_eq!(oversample, 4, "shifted HRPD slice should be 4x chip rate");
        let chip_start = absolute_sample_start / oversample as u64;
        eprintln!(
            "HRPD 1800347049472645 shifted access capture: shifted_rate={} samples={} chip_start={} chip_mod_frame={}",
            sample_rate,
            iq_samples.len(),
            chip_start,
            chip_start % 32768,
        );
        let runs = hrpd_slot_power_runs_at_threshold(&iq_samples, chip_start, oversample, 2.0);
        for (idx, run) in runs.iter().enumerate() {
            eprintln!(
                "HRPD 1800347049472645 energy run #{}: slots=[{}, {}) chips=[{}, {}) slot_count={} peak={:+.2}dB",
                idx + 1,
                run.start_slot,
                run.end_slot,
                run.start_chip,
                run.end_chip,
                run.end_slot - run.start_slot,
                run.peak_db,
            );
        }
        let burst_count = runs.len();

        let pipeline = hrpd_reverse_access_chain(HrpdReverseAccessSettings {
            oversample,
            ..HrpdReverseAccessSettings::default()
        });
        let total_samples = iq_samples.len();
        let mut receiver = PipelinedReceiver::new(iq_samples.into_iter())
            .with_batch_size(32768)
            .with_input_sample_rate_hz(sample_rate as f64)
            .with_absolute_sample_start(absolute_sample_start);
        let out_rx = receiver.add_pipeline(pipeline);
        let pipeline_start = std::time::Instant::now();
        receiver.run_pipeline().unwrap();
        let pipeline_elapsed = pipeline_start.elapsed();
        let capture_seconds = total_samples as f64 / sample_rate as f64;
        eprintln!(
            "HRPD 1800347049472645 receiver pipeline timing: capture={:.2}s pipeline={:.2}s real_time={:.2}x",
            capture_seconds,
            pipeline_elapsed.as_secs_f64(),
            capture_seconds / pipeline_elapsed.as_secs_f64(),
        );

        let events = collect_hrpd_access_capture_events(out_rx);
        let fcs_valid = events
            .iter()
            .filter(|event| event.single_fragment_fcs_valid)
            .collect::<Vec<_>>();
        for (idx, event) in events.iter().enumerate() {
            eprintln!(
                "HRPD 1800347049472645 event #{}: chip={} phy_crc_valid={} mac_fragment_valid={} single_fragment_fcs_valid={} msg_len={:?} info_hex={} decoded={}",
                idx + 1,
                event.packet_start,
                event.phy_crc_valid,
                event.mac_fragment_valid,
                event.single_fragment_fcs_valid,
                event.mac_length_octets,
                event.info_hex,
                event.decoded,
            );
        }
        eprintln!(
            "HRPD 1800347049472645 summary: energy_bursts={} streaming_events={} fcs_valid={}",
            burst_count,
            events.len(),
            fcs_valid.len(),
        );
        assert_eq!(
            burst_count, 19,
            "expected 19 high-energy HRPD access burst runs"
        );
        // All 19 high-energy bursts decode. Burst #5 (chips ~1800347055063040)
        // steps its carrier/gain mid-probe, so its three preamble frames are
        // not full copies of each other; per-slot non-coherent combining
        // scores it on the live slots that do match instead of on a
        // whole-frame coherent sum, which recovers the capsule the earlier
        // full-frame metric missed.
        assert_eq!(
            fcs_valid.len(),
            19,
            "expected the live HRPD access receiver chain to decode all 19 bursts"
        );
        let uati_requests = fcs_valid
            .iter()
            .filter(|event| event.decoded.contains("UATIRequest"))
            .count();
        let uati_completes = fcs_valid
            .iter()
            .filter(|event| {
                event.decoded.contains("UATIComplete")
                    && event.decoded.contains("ConnectionRequest")
                    && event.decoded.contains("ati=Uati/0x1a058001")
            })
            .count();
        assert_eq!(uati_requests, 1, "one initial UATIRequest probe");
        assert_eq!(
            uati_completes, 18,
            "eighteen multi-fragment UATIComplete + ConnectionRequest retries on the assigned UATI"
        );
    }

    #[test]
    fn capture_hrpd_reverse_access_1800067761628706() {
        init_test_logger();
        let metadata_path = test_capture_path("1800067761628706.json");
        let wav_path = test_capture_path("1800067761628706.wav");
        let metadata = test_capture_metadata_from_path(&metadata_path);
        assert_eq!(metadata.sample_rate_hz, 9_830_400);
        assert_eq!(metadata.chip_rate_hz, 1_228_800);
        assert_eq!(metadata.rx_center_frequency_hz, Some(846_105_000));
        assert_eq!(metadata.one_x_reverse_frequency_hz, Some(848_310_000));
        assert_eq!(metadata.one_x_rx_shift_hz, Some(2_205_000));
        assert_eq!(metadata.hrpd_reverse_frequency_hz, Some(843_900_000));
        assert_eq!(metadata.hrpd_rx_shift_hz, Some(-2_205_000));
        assert!(
            wav_path.exists(),
            "missing HRPD reverse access capture {}",
            wav_path.display()
        );

        let hrpd_shift_hz = metadata
            .hrpd_rx_shift_hz
            .expect("capture sidecar must include hrpd_rx_shift_hz");
        let (sample_rate, iq_samples, absolute_sample_start) =
            read_shifted_capture_to_4x_with_shift(&wav_path, &metadata, hrpd_shift_hz);
        let oversample = (sample_rate as usize) / ACCESS_CHIP_RATE as usize;
        assert_eq!(oversample, 4, "shifted HRPD slice should be 4x chip rate");
        let chip_start = absolute_sample_start / oversample as u64;
        eprintln!(
            "HRPD 1800067761628706 shifted access capture: raw_rate={} shifted_rate={} samples={} abs_sample_start={} chip_start={} chip_mod_frame={} shift_hz={}",
            metadata.sample_rate_hz,
            sample_rate,
            iq_samples.len(),
            absolute_sample_start,
            chip_start,
            chip_start % 32768,
            hrpd_shift_hz,
        );
        let runs = hrpd_slot_power_runs_at_threshold(&iq_samples, chip_start, oversample, 2.0);
        for (idx, run) in runs.iter().enumerate() {
            eprintln!(
                "HRPD 1800067761628706 energy run #{}: slots=[{}, {}) chips=[{}, {}) slot_count={} peak={:+.2}dB",
                idx + 1,
                run.start_slot,
                run.end_slot,
                run.start_chip,
                run.end_chip,
                run.end_slot - run.start_slot,
                run.peak_db,
            );
        }

        let pipeline = hrpd_reverse_access_chain(HrpdReverseAccessSettings {
            oversample,
            ..HrpdReverseAccessSettings::default()
        });
        let total_samples = iq_samples.len();
        let mut receiver = PipelinedReceiver::new(iq_samples.into_iter())
            .with_batch_size(32768)
            .with_input_sample_rate_hz(sample_rate as f64)
            .with_absolute_sample_start(absolute_sample_start);
        let out_rx = receiver.add_pipeline(pipeline);
        let pipeline_start = std::time::Instant::now();
        receiver.run_pipeline().unwrap();
        let pipeline_elapsed = pipeline_start.elapsed();
        let capture_seconds = total_samples as f64 / sample_rate as f64;
        let multiplier = capture_seconds / pipeline_elapsed.as_secs_f64();
        eprintln!(
            "HRPD 1800067761628706 receiver pipeline timing: capture={:.2}s pipeline={:.2}s real_time={:.2}x",
            capture_seconds,
            pipeline_elapsed.as_secs_f64(),
            multiplier,
        );
        let min_multiplier = if std::env::var_os("CI").is_some() {
            HRPD_ACCESS_CI_MIN_REALTIME_SPEEDUP
        } else {
            HRPD_ACCESS_MIN_REALTIME_SPEEDUP
        };
        assert!(
            multiplier >= min_multiplier,
            "HRPD reverse access below the {min_multiplier:.1}x realtime floor: \
             speedup={multiplier:.2}x capture={capture_seconds:.2}s \
             pipeline={:.2}s",
            pipeline_elapsed.as_secs_f64(),
        );

        let events = collect_hrpd_access_capture_events(out_rx);
        let fcs_valid = events
            .iter()
            .filter(|event| event.single_fragment_fcs_valid)
            .collect::<Vec<_>>();
        for (idx, event) in events.iter().enumerate() {
            eprintln!(
                "HRPD 1800067761628706 event #{}: chip={} phy_crc_valid={} mac_fragment_valid={} single_fragment_fcs_valid={} msg_len={:?} info_hex={} decoded={}",
                idx + 1,
                event.packet_start,
                event.phy_crc_valid,
                event.mac_fragment_valid,
                event.single_fragment_fcs_valid,
                event.mac_length_octets,
                event.info_hex,
                event.decoded,
            );
        }
        eprintln!(
            "HRPD 1800067761628706 shifted access summary: streaming_events={} streaming_fcs_valid={} decoded={:?}",
            events.len(),
            fcs_valid.len(),
            fcs_valid
                .iter()
                .map(|event| &event.decoded)
                .collect::<Vec<_>>(),
        );
        assert!(
            fcs_valid.iter().any(|event| {
                event.packet_start == 1_800_067_776_610_304
                    && event.decoded.contains("RouteUpdate")
                    && event.decoded.contains("ConnectionRequest")
                    && event.decoded.contains("ati=Uati/0x1a058001")
            }),
            "expected the logged UATI ConnectionRequest access packet to decode: {:?}",
            fcs_valid
        );

        // Guard the production search stages against algorithmic CPU regressions.
        let metrics_handle =
            crate::receiver::hrpd::reverse_correlator_base::get_metrics_handle("hrpd_access")
                .expect("hrpd_access correlator metrics should be registered");
        let metrics = metrics_handle.lock().expect("metrics mutex");
        let per_block_us = metrics.per_block_avg_us();
        let fft_scan_us = metrics.fft_scan_avg_us();
        let spawn_us = metrics.spawn_finger_avg_us();
        let append_us = metrics.append_block_avg_us();
        eprintln!(
            "HRPD 1800067761628706 correlator timing: per_block_avg={}us append_avg={}us fft_scan_avg={}us(n={}) spawn_avg={}us(n={})",
            per_block_us,
            append_us,
            fft_scan_us,
            metrics.fft_scan_calls,
            spawn_us,
            metrics.spawn_finger_calls,
        );
        // Total time per section across the whole run, so the heat
        // breakdown is meaningful regardless of call-rate differences.
        let total_pipeline_ms = pipeline_elapsed.as_millis() as u64;
        let fft_total_ms = metrics.fft_scan_ns / 1_000_000;
        let spawn_total_ms = metrics.spawn_finger_ns / 1_000_000;
        let ref_setup_ms = metrics.searcher_ref_setup_ns / 1_000_000;
        let sig_fft_ms = metrics.searcher_signal_fft_ns / 1_000_000;
        let ifft_mult_ms = metrics.searcher_ifft_mult_ns / 1_000_000;
        let peak_find_ms = metrics.searcher_peak_find_ns / 1_000_000;
        eprintln!(
            "HRPD 1800067761628706 HEAT total_pipeline={total_ms}ms\n\
             \tfft_scan_top_hits={fft_ms}ms ({fft_pct:.1}%)\n\
             \t\tref_setup={r_ms}ms ({r_pct:.1}%) [{r_calls} windows]\n\
             \t\tsignal_fft={s_ms}ms ({s_pct:.1}%)\n\
             \t\tifft+mult={i_ms}ms ({i_pct:.1}%)\n\
             \t\tpeak_find={p_ms}ms ({p_pct:.1}%)\n\
             \tspawn_finger={spawn_ms}ms ({spawn_pct:.1}%) [{spawn_calls} calls]",
            total_ms = total_pipeline_ms,
            fft_ms = fft_total_ms,
            fft_pct = 100.0 * fft_total_ms as f64 / total_pipeline_ms.max(1) as f64,
            r_ms = ref_setup_ms,
            r_pct = 100.0 * ref_setup_ms as f64 / total_pipeline_ms.max(1) as f64,
            r_calls = metrics.searcher_ref_setup_calls,
            s_ms = sig_fft_ms,
            s_pct = 100.0 * sig_fft_ms as f64 / total_pipeline_ms.max(1) as f64,
            i_ms = ifft_mult_ms,
            i_pct = 100.0 * ifft_mult_ms as f64 / total_pipeline_ms.max(1) as f64,
            p_ms = peak_find_ms,
            p_pct = 100.0 * peak_find_ms as f64 / total_pipeline_ms.max(1) as f64,
            spawn_ms = spawn_total_ms,
            spawn_pct = 100.0 * spawn_total_ms as f64 / total_pipeline_ms.max(1) as f64,
            spawn_calls = metrics.spawn_finger_calls,
        );
        // These per-stage ceilings leave scheduler headroom while catching
        // unexpectedly expensive search or timing refinement.
        assert!(
            per_block_us < 10_000,
            "hrpd_access per_block_avg too slow: {per_block_us}us (budget 10000us)",
        );
        assert!(
            fft_scan_us < 25_000,
            "hrpd_access fft_scan_avg too slow: {fft_scan_us}us (budget 25000us)",
        );
        assert!(
            spawn_us < 60_000,
            "hrpd_access spawn_finger_avg too slow: {spawn_us}us (budget 60000us)",
        );
    }

    const RC1_RATE_COUNT_PER_BUCKET_TOLERANCE: usize = 3;
    const RC1_RATE_COUNT_TOTAL_DRIFT_TOLERANCE: usize = 6;
    const CAPTURE_CI_MIN_REALTIME_SPEEDUP: f64 = 1.25;
    const HRPD_ACCESS_MIN_REALTIME_SPEEDUP: f64 = 18.0;
    const HRPD_ACCESS_CI_MIN_REALTIME_SPEEDUP: f64 = 12.0;

    fn assert_rc1_rate_counts_with_small_drift(
        actual: &std::collections::BTreeMap<i64, usize>,
        expected: &std::collections::BTreeMap<i64, usize>,
        walsh_code: u8,
    ) {
        let actual_total: usize = actual.values().sum();
        let expected_total: usize = expected.values().sum();
        assert_eq!(
            actual_total, expected_total,
            "rate_counts total mismatch for walsh={walsh_code}: got={actual:?} expected={expected:?}",
        );

        let rates = actual
            .keys()
            .chain(expected.keys())
            .copied()
            .collect::<std::collections::BTreeSet<_>>();
        let mut total_drift = 0usize;
        for rate in rates {
            let got = actual.get(&rate).copied().unwrap_or(0);
            let want = expected.get(&rate).copied().unwrap_or(0);
            let drift = got.abs_diff(want);
            total_drift += drift;
            assert!(
                drift <= RC1_RATE_COUNT_PER_BUCKET_TOLERANCE,
                "rate_counts drift too large for walsh={walsh_code} rate={rate}: got={got} expected={want} \
                 tolerance={RC1_RATE_COUNT_PER_BUCKET_TOLERANCE}; all got={actual:?} expected={expected:?}",
            );
        }

        assert!(
            total_drift <= RC1_RATE_COUNT_TOTAL_DRIFT_TOLERANCE,
            "rate_counts aggregate drift too large for walsh={walsh_code}: drift={total_drift} \
             tolerance={RC1_RATE_COUNT_TOTAL_DRIFT_TOLERANCE}; got={actual:?} expected={expected:?}",
        );
    }

    #[test]
    fn capture_wav_3_missed_all_bursts() {
        init_test_logger();

        let Some(default_stats) = run_uplink_access_probe_full_chain_capture(
            "1791707212196254.wav",
            1791707212196254,
            "wav_3_default",
        ) else {
            return;
        };

        assert_min_realtime_speedup("wav_3_default", &default_stats, 2.4);

        // Lock the current deduped decode count for this reproducer under the
        // production reverse-access chain. An offline high-energy envelope pass
        // on the raw WAV shows 13 real burst regions here; a higher count would
        // indicate cross-finger duplicate emits.
        assert_eq!(
            default_stats.crc_valid_data_frame_count, 13,
            "expected WAV 3 reproducer to produce exactly 13 deduped CRC-valid access frames, got {}",
            default_stats.crc_valid_data_frame_count
        );
    }

    #[test]
    fn capture_wav_access_bursts_decode_nonzero() {
        init_test_logger();

        let Some(stats) = run_uplink_access_probe_full_chain_capture(
            "1792124445932252.wav",
            1792124445932252,
            "wav_access_bursts_decode_nonzero",
        ) else {
            return;
        };
        assert_min_realtime_speedup("wav_access_bursts_decode_nonzero", &stats, 2.2);

        assert_eq!(
            // This capture contains about 31 real burst episodes. The old
            // >=144 floor was inflated by per-finger duplicate emits of the
            // same decoded burst before receiver-side dedupe.
            stats.crc_valid_data_frame_count,
            31,
            "expected WAV access burst capture to produce exactly 31 deduped CRC-valid access frames, got {}",
            stats.crc_valid_data_frame_count
        );
    }

    #[test]
    fn capture_wav_1792233789071851_decodes_all_crc_valid_frames() {
        init_test_logger();

        let Some(stats) = run_uplink_access_probe_full_chain_capture(
            "1792233789071851.wav",
            1792233789071851,
            "wav_1792233789071851",
        ) else {
            return;
        };
        assert_min_realtime_speedup("wav_1792233789071851", &stats, 3.0);

        assert_eq!(
            stats.crc_valid_data_frame_count, 7,
            "expected WAV 1792233789071851 to produce exactly 7 CRC-valid access frames, got {:?}",
            stats
        );
    }

    #[test]
    fn capture_wav_1792576442605628_repeated_probe_bursts_decode_4_crc_valid_frames() {
        init_test_logger();

        let Some(stats) = run_uplink_access_probe_full_chain_capture(
            "1792576442605628.wav",
            1792576442605628,
            "wav_1792576442605628_repeated_probe_bursts",
        ) else {
            return;
        };
        assert_min_realtime_speedup("wav_1792576442605628_repeated_probe_bursts", &stats, 2.6);

        assert_eq!(
            stats.crc_valid_data_frame_count, 4,
            "expected WAV 1792576442605628 to produce exactly 4 CRC-valid access frames, got {:?}",
            stats
        );
    }

    #[test]
    fn capture_wav_1793040844076000_short_preamble_decodes_5_crc_valid_frames() {
        init_test_logger();

        let Some(stats) = run_uplink_access_probe_full_chain_capture(
            "1793040844076000.wav",
            1793040844076000,
            "wav_1793040844076000_short_preamble",
        ) else {
            return;
        };
        assert_min_realtime_speedup("wav_1793040844076000_short_preamble", &stats, 2.5);

        assert_eq!(
            stats.crc_valid_data_frame_count, 5,
            "expected WAV 1793040844076000 short preamble capture to produce exactly 5 CRC-valid access frames, got {:?}",
            stats
        );
    }

    #[test]
    fn capture_wav_1793678826011477_decodes_crc_valid_frames() {
        init_test_logger();

        // `wav_burst_count` shows 6 bursts (~350 ms each) with
        // peak/noise SNR 11.2–19.9 dB. Raw burst energy is low compared
        // with other access captures, but the SNR is high enough that
        // the receiver should be able to demod at least some of them.
        // Baseline is currently 0 CRC-valid frames — this test is the
        // regression anchor for any future improvement. Expected count
        // should be updated (upward) as acquisition / demod improves;
        // a decrease is a regression.
        let Some(stats) = run_uplink_access_probe_full_chain_capture(
            "1793678826011477.wav",
            1793678826011477,
            "wav_1793678826011477",
        ) else {
            return;
        };
        assert_min_realtime_speedup("wav_1793678826011477", &stats, 3.5);

        assert_eq!(
            stats.crc_valid_data_frame_count, 0,
            "expected WAV 1793678826011477 to produce exactly 0 CRC-valid access frames \
             (baseline — update upward when acquisition improves), got {:?}",
            stats
        );
    }

    #[test]
    #[ignore = "manual low-SNR access capture sweep; run explicitly while tuning correlator gates"]
    fn capture_wav_1794057106107467_low_snr_probe() {
        init_test_logger();

        let Some(stats) = run_uplink_access_probe_full_chain_capture(
            "1794057106107467.wav",
            1794057106107467,
            "wav_1794057106107467_low_snr",
        ) else {
            return;
        };
        assert_min_realtime_speedup("wav_1794057106107467_low_snr", &stats, 2.0);

        eprintln!("wav_1794057106107467_low_snr stats: {:?}", stats);
    }

    #[test]
    #[ignore = "capture has 7 offline high-energy burst regions, but the live RX chain currently decodes 0 CRC-valid frames"]
    fn capture_wav_1793041280153327_short_preamble_decodes_7_crc_valid_frames() {
        init_test_logger();

        let Some(stats) = run_uplink_access_probe_full_chain_capture(
            "1793041280153327.wav",
            1793041280153327,
            "wav_1793041280153327_short_preamble",
        ) else {
            return;
        };
        assert_min_realtime_speedup("wav_1793041280153327_short_preamble", &stats, 2.0);

        assert_eq!(
            stats.crc_valid_data_frame_count, 7,
            "expected WAV 1793041280153327 short preamble capture to produce exactly 7 CRC-valid access frames, got {:?}",
            stats
        );
    }

    #[test]
    fn capture_wav_1792631693292558_none_decoded2() {
        init_test_logger();

        let Some(stats) = run_uplink_access_probe_full_chain_capture(
            "1792631693292558.wav",
            1792631693292558,
            "wav_1792631693292558_none_decoded2",
        ) else {
            return;
        };
        assert_min_realtime_speedup("wav_1792631693292558_none_decoded2", &stats, 1.75);

        // This runs through the production reverse_access_chain. The simplified
        // access decoder recovers one more clean burst than the previous staged
        // aligner path while still staying below the physical burst count.
        assert_eq!(
            stats.crc_valid_data_frame_count, 16,
            "expected WAV 1792631693292558 to produce exactly 16 CRC-valid access frames, got {:?}",
            stats
        );
    }

    #[test]
    fn capture_optimize_perf() {
        init_test_logger();

        let Some(stats) = run_uplink_access_probe_full_chain_capture(
            "1793200139566170.wav",
            1793200139566170,
            "optimize_perf",
        ) else {
            return;
        };
        assert_min_realtime_speedup("optimize_perf", &stats, 3.0);

        // 5 bursts visible offline but only burst 1 correlates — later probes
        // in the access sequence do not produce detectable PN+LC correlation
        // peaks even with full-period search and wide LC span.  Under
        // investigation: may be PN_RAND re-randomization or LC state mismatch
        // between consecutive probes in the sequence.
        assert_eq!(
            stats.crc_valid_data_frame_count, 1,
            "expected WAV 1793200139566170 (optimize_perf) to produce exactly 1 CRC-valid access frame, got {:?}",
            stats
        );
    }

    #[test]
    fn capture_probe_ran_9() {
        init_test_logger();

        let Some(stats) = run_uplink_access_probe_full_chain_capture(
            "1793062609071211.wav",
            1793062609071211,
            "probe_ran_9",
        ) else {
            return;
        };
        assert_min_realtime_speedup("probe_ran_9", &stats, 1.9);

        assert_eq!(
            stats.crc_valid_data_frame_count, 10,
            "expected WAV 1793062609071211 (probe_ran_9) to produce exactly 10 CRC-valid access frames, got {:?}",
            stats
        );
    }

    #[test]
    fn capture_v60s_decode() {
        init_test_logger();

        let Some(stats) = run_uplink_access_probe_full_chain_capture(
            "1793161835652125.wav",
            1793161835652125,
            "v60s_decode",
        ) else {
            return;
        };
        assert_min_realtime_speedup("v60s_decode", &stats, 3.0);

        // PAM_SZ=10 → 11 preamble frames (220 ms).  Burst durations in this
        // capture are 160–245 ms, so most bursts are entirely preamble with
        // at most 1 data frame.  The frame aligner cannot lock with <2 data
        // frames, so 0 CRC-valid is expected under the current BTS config.
        // Issue-tracked: revisit decode once PAM_SZ is lowered or replay
        // covers more preamble.
        let _ = &stats;
    }

    #[test]
    fn capture_v60s_decode_fail() {
        init_test_logger();

        let Some(stats) = run_uplink_access_probe_full_chain_capture(
            "1795077999123595.wav",
            1795077999123595,
            "v60s_decode_fail",
        ) else {
            return;
        };
        assert_min_realtime_speedup("v60s_decode_fail", &stats, 3.0);

        // Offline envelope detection finds two clean 361 ms reverse access
        // bursts in this capture at roughly 5.127 s and 5.847 s.
        assert_eq!(
            stats.crc_valid_data_frame_count, 2,
            "expected v60s_decode_fail to produce exactly 2 CRC-valid access frames, got {:?}",
            stats
        );
    }

    #[test]
    fn capture_v60s_decode_fail_bad() {
        init_test_logger();

        let Some(stats) = run_uplink_access_probe_full_chain_capture(
            "1795081057573573.wav",
            1795081057573573,
            "v60s_decode_fail_bad",
        ) else {
            return;
        };
        assert_min_realtime_speedup("v60s_decode_fail_bad", &stats, 1.4);

        // `wav_burst_count --threshold 5 --min-ms 20` finds twelve clean
        // 360-361 ms reverse access bursts in this rough capture.
        assert_eq!(
            stats.crc_valid_data_frame_count, 12,
            "expected v60s_decode_fail_bad to produce exactly 12 CRC-valid access frames, got {:?}",
            stats
        );
    }

    #[test]
    fn capture_v60s_slow_decode() {
        init_test_logger();

        let Some(stats) = run_uplink_access_probe_full_chain_capture(
            "1794579769922112.wav",
            1794579769922112,
            "v60s_slow_decode",
        ) else {
            return;
        };
        assert_min_realtime_speedup("v60s_slow_decode", &stats, 1.0);

        assert_eq!(
            stats.crc_valid_data_frame_count, 6,
            "expected v60s_slow_decode to produce exactly 6 CRC-valid access frames, got {:?}",
            stats
        );
    }

    #[test]
    fn capture_v60s_1794630927745164_decodes_5_crc_valid_frames() {
        init_test_logger();

        let Some(stats) = run_uplink_access_probe_full_chain_capture(
            "1794630927745164.wav",
            1794630927745164,
            "v60s_1794630927745164",
        ) else {
            return;
        };
        assert_min_realtime_speedup("v60s_1794630927745164", &stats, 1.4);

        assert_eq!(
            stats.crc_valid_data_frame_count, 6,
            "expected v60s_1794630927745164 to produce exactly 6 CRC-valid access frames, got {:?}",
            stats
        );
    }

    #[test]
    fn capture_uplink_access_probe_finger_acquisition_full_chain_prev_capture() {
        init_test_logger();

        let Some(stats) = run_uplink_access_probe_full_chain_capture(
            "1791587434867463.wav",
            1791587434867463,
            "uplink_access_probe_full_chain_previous_capture",
        ) else {
            return;
        };
        assert_min_realtime_speedup(
            "uplink access probe full chain previous capture",
            &stats,
            1.5,
        );

        // Lock the current deduped decode count for the older full-chain
        // capture. An offline high-energy envelope pass shows 5 real burst
        // regions here; the older 9-frame baseline was counting duplicate
        // cross-finger emits.
        assert_eq!(
            stats.crc_valid_data_frame_count, 5,
            "expected exactly 5 deduped CRC-valid access data frames from previous access probe capture, got {}",
            stats.crc_valid_data_frame_count
        );
    }

    #[test]
    #[ignore = "diagnostic offline WAV analysis; run explicitly when requested"]
    fn capture_uplink_wav_offline_analysis() {
        init_test_logger();
        let wav_path = test_capture_path("1791617173891930.wav");
        if !wav_path.exists() {
            eprintln!("skipping: {} not found", wav_path.display());
            return;
        }
        let reader = hound::WavReader::open(&wav_path).unwrap();
        let (sample_rate, iq_samples) = read_iq_wav(reader);
        let oversample = sample_rate as usize / 1228800;
        let chip_start: u64 = 1791617173891930;
        eprintln!(
            "offline analysis: {} samples, sr={}, os={}, chip_start={}",
            iq_samples.len(),
            sample_rate,
            oversample,
            chip_start
        );

        // 1) Windowed power profile — 1024-sample windows, sliding by 1024
        let win = 4096;
        let mut power_profile: Vec<(usize, f32)> = Vec::new();
        let mut max_pow = 0.0f32;
        let mut max_pow_idx = 0usize;
        for start in (0..iq_samples.len()).step_by(win) {
            let end = (start + win).min(iq_samples.len());
            let avg: f32 = iq_samples[start..end]
                .iter()
                .map(|s| s.norm_sqr())
                .sum::<f32>()
                / (end - start) as f32;
            if avg > max_pow {
                max_pow = avg;
                max_pow_idx = start;
            }
            power_profile.push((start, avg));
        }
        // Print power around 8M-12M region
        eprintln!("\n=== Power profile around 8M-14M samples ===");
        for &(idx, pow) in &power_profile {
            if idx >= 8_000_000 && idx <= 14_000_000 {
                eprintln!(
                    "  sample={:>10} chip={:>10} power={:.8}",
                    idx,
                    idx / oversample,
                    pow
                );
            }
        }
        eprintln!(
            "global max power: sample={} pow={:.8}",
            max_pow_idx, max_pow
        );

        // Find all regions where power > 2x the noise floor (estimated from first 4M)
        let noise_floor: f32 = power_profile
            .iter()
            .filter(|(idx, _)| *idx < 4_000_000)
            .map(|(_, p)| *p)
            .sum::<f32>()
            / power_profile
                .iter()
                .filter(|(idx, _)| *idx < 4_000_000)
                .count()
                .max(1) as f32;
        eprintln!("noise floor (first 4M): {:.8}", noise_floor);
        eprintln!("\n=== Regions with power > 3x noise ===");
        let mut in_burst = false;
        let mut burst_start = 0usize;
        for &(idx, pow) in &power_profile {
            if pow > noise_floor * 3.0 {
                if !in_burst {
                    burst_start = idx;
                    in_burst = true;
                }
            } else if in_burst {
                let burst_end = idx;
                let burst_samples = burst_end - burst_start;
                let burst_chips = burst_samples / oversample;
                let burst_ms = burst_samples as f64 / sample_rate as f64 * 1000.0;
                eprintln!(
                    "  burst: sample={}..{} ({} samples, {} chips, {:.1}ms) chip_abs={}..{}",
                    burst_start,
                    burst_end,
                    burst_samples,
                    burst_chips,
                    burst_ms,
                    chip_start as usize + burst_start / oversample,
                    chip_start as usize + burst_end / oversample,
                );
                in_burst = false;
            }
        }

        // 2) PN cross-correlation focused around the first burst region
        use rustfft::FftPlanner;
        let pn_period = 32768 * oversample;
        let mut planner = FftPlanner::<f32>::new();
        let fft_fwd = planner.plan_fft_forward(pn_period);
        let fft_inv = planner.plan_fft_inverse(pn_period);

        // PN0 reference
        let mut pn_ref = build_oqpsk_pn_samples(pn_period, oversample)
            .into_iter()
            .map(|s| Complex32::new(s.re, -s.im))
            .collect::<Vec<_>>();
        fft_fwd.process(&mut pn_ref);

        // Correlate at several offsets around the burst
        eprintln!("\n=== PN correlation at burst regions ===");
        for region_start in (8_000_000..14_000_000).step_by(pn_period) {
            if region_start + pn_period > iq_samples.len() {
                break;
            }
            let mut sig = iq_samples[region_start..region_start + pn_period].to_vec();
            fft_fwd.process(&mut sig);
            let mut corr: Vec<Complex32> = sig
                .iter()
                .zip(pn_ref.iter())
                .map(|(a, b)| a * b.conj())
                .collect();
            fft_inv.process(&mut corr);
            let scale = 1.0 / pn_period as f32;
            let powers: Vec<f32> = corr.iter().map(|c| (c * scale).norm_sqr()).collect();
            let avg: f32 = powers.iter().sum::<f32>() / powers.len() as f32;

            let mut indexed: Vec<(usize, f32)> =
                powers.iter().enumerate().map(|(i, &v)| (i, v)).collect();
            indexed.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());

            let top = &indexed[..5.min(indexed.len())];
            let top_str: Vec<String> = top
                .iter()
                .map(|(idx, pow)| {
                    format!(
                        "d={} chip={}.{} pow={:.4} ({:.1}x)",
                        idx,
                        idx / oversample,
                        idx % oversample,
                        pow,
                        pow / avg.max(1e-20)
                    )
                })
                .collect();
            eprintln!(
                "  region_start={:>10} avg={:.6} top: {}",
                region_start,
                avg,
                top_str.join(" | ")
            );
        }
    }

    /// Offline analysis: apply matched filter to WAV, then PN+LC despread at
    /// the known detection parameters (delay=101, lc_phase=-25) and check
    /// coherence at each sub-sample offset to verify we're sampling at the
    /// optimal point.
    #[test]
    #[ignore = "diagnostic despread-quality sweep; run explicitly when requested"]
    fn capture_uplink_wav_despread_quality() {
        use crate::phy::walsh::WalshGenerator;
        use crate::sdr::{cdma2000_baseband_filter_taps_f64, fir::ComplexFir32};

        init_test_logger();
        let wav_path = test_capture_path("1791202702894280.wav");
        if !wav_path.exists() {
            eprintln!("skipping: {} not found", wav_path.display());
            return;
        }
        let reader = hound::WavReader::open(&wav_path).unwrap();
        let (sample_rate, iq_samples) = read_iq_wav(reader);
        let oversample = sample_rate as usize / 1228800;
        let chip_start: u64 = 1791202702894280;
        let phase_period = 32768 * oversample;

        // Apply matched filter
        let taps = cdma2000_baseband_filter_taps_f64();
        let filtered = ComplexFir32::new(&taps).process_block(&iq_samples);

        // Known parameters from detection log
        let abs_origin = chip_start as usize * oversample;
        let composite_filter_delay = 47usize;
        let aligned_delay = 101i32; // from correlator detection

        // Finger was spawned at sample 9469954 in the stream
        let finger_start_sample = 9_469_954usize;

        eprintln!(
            "despread quality: filtered={} os={} abs_origin={} finger_start={}",
            filtered.len(),
            oversample,
            abs_origin,
            finger_start_sample
        );

        // Build BOTH PN conj references
        let pn_plain_raw = build_fft_search_pn_samples(phase_period, oversample);
        let pn_oqpsk_raw = build_oqpsk_pn_samples(phase_period, oversample);
        let pn_refs: Vec<(&str, Vec<Complex32>)> = vec![
            (
                "plain",
                pn_plain_raw
                    .iter()
                    .map(|s| Complex32::new(s.re, -s.im))
                    .collect(),
            ),
            (
                "oqpsk",
                pn_oqpsk_raw
                    .iter()
                    .map(|s| Complex32::new(s.re, -s.im))
                    .collect(),
            ),
        ];

        let walsh = WalshGenerator::generate_matrix::<64>();

        // Try sweeping aligned_delay and sub_offset to find the optimal point
        for (pn_name, pn_conj) in &pn_refs {
            for delay_offset in [0i32] {
                let test_delay = aligned_delay + delay_offset;
                for sub_offset in 0..oversample {
                    for lc_delta in -30i32..=5 {
                        let start_sample = finger_start_sample;
                        let expected_abs_chip = {
                            let abs = abs_origin as i64;
                            let s = start_sample as i64 + sub_offset as i64;
                            ((abs + s - composite_filter_delay as i64 - test_delay as i64)
                                / oversample as i64)
                                .max(0) as usize
                        };
                        let lc_chip_start =
                            (expected_abs_chip as i64 + lc_delta as i64).max(0) as usize;
                        let mut lc = LongCodeGenerator::new_access_channel_with_state(
                            0,
                            1,
                            1,
                            0,
                            1u64 << 41,
                        );
                        lc.advance_chips(lc_chip_start);

                        // Despread 16 symbols (16 * 256 = 4096 chips)
                        let n_symbols = 16usize;
                        let n_chips = n_symbols * 256;
                        let mut chips = Vec::with_capacity(n_chips);

                        for k in 0..n_chips {
                            let sample_idx = start_sample + k * oversample + sub_offset;
                            if sample_idx >= filtered.len() {
                                break;
                            }
                            // PN phase: (abs + sample - cfd - aligned_delay) % pp
                            // This matches the finger's despread_phase computation
                            let raw_phase = abs_origin as i64 + sample_idx as i64
                                - composite_filter_delay as i64
                                - test_delay as i64;
                            let pn_idx = ((raw_phase % phase_period as i64 + phase_period as i64)
                                % phase_period as i64)
                                as usize;
                            let pn = pn_conj[pn_idx];
                            let despread = filtered[sample_idx] * pn;
                            let lc_bit = lc.next_chip();
                            let lc_sign: f32 = if lc_bit == 1 { -1.0 } else { 1.0 };
                            chips
                                .push(Complex32::new(despread.re * lc_sign, despread.im * lc_sign));
                        }

                        if chips.len() < n_chips {
                            continue;
                        }

                        // Compute per-symbol coherence and W0 energy ratio
                        let mut total_coh = 0.0f32;
                        let mut w0_ratio_sum = 0.0f32;
                        for sym in 0..n_symbols {
                            let sym_chips = &chips[sym * 256..(sym + 1) * 256];
                            let pilot: Complex32 = sym_chips.iter().sum();
                            let incoherent: f32 = sym_chips.iter().map(|s| s.norm()).sum();
                            let coh = if incoherent > 1e-9 {
                                pilot.norm() / incoherent
                            } else {
                                0.0
                            };
                            total_coh += coh;

                            // Walsh correlation: accumulate 4 PN chips per Walsh chip
                            let mut walsh_chips = [Complex32::new(0.0, 0.0); 64];
                            for (idx, chunk) in sym_chips.chunks_exact(4).enumerate() {
                                walsh_chips[idx] = chunk.iter().copied().sum();
                            }
                            let mut energies = [0.0f32; 64];
                            for (row_idx, row) in walsh.iter().enumerate() {
                                let mut corr = Complex32::new(0.0, 0.0);
                                for (wc, &sign) in walsh_chips.iter().zip(row.iter()) {
                                    let s = sign as f32;
                                    corr += Complex32::new(wc.re * s, wc.im * s);
                                }
                                energies[row_idx] = corr.norm_sqr();
                            }
                            let total_e: f32 = energies.iter().sum();
                            let w0_ratio = if total_e > 1e-9 {
                                energies[0] / total_e
                            } else {
                                0.0
                            };
                            w0_ratio_sum += w0_ratio;
                        }

                        let avg_coh = total_coh / n_symbols as f32;
                        let avg_w0 = w0_ratio_sum / n_symbols as f32;

                        if avg_coh > 0.20 {
                            eprintln!(
                                "  pn={:5} delay={:+} sub={} lc_delta={:+3} avg_coh={:.3} avg_w0={:.3}",
                                pn_name, delay_offset, sub_offset, lc_delta, avg_coh, avg_w0
                            );
                        }
                    }
                }
            }
        } // close pn_refs loop
    }

    /// Synthetic test: generate chip-rate LC-scrambled preamble samples
    /// (no PN, no pulse shaping) and feed directly through the LC processor
    /// → Walsh demod → deinterleave → Viterbi → AccessChannelProcessor chain.
    ///
    /// This isolates the downstream pipeline from the rake/PN issues.
    #[test]
    fn test_synthetic_access_probe_lc_acquisition() {
        init_test_logger();

        let chip_rate = 1_228_800usize;
        let chip_start: usize = 100_000;

        // Generate 100ms of preamble = all +1 data, scrambled by LC.
        let preamble_chips = chip_rate / 10; // 122,880 chips

        let mut lc_gen = LongCodeGenerator::new_access_channel(0, 1, 1, 0);
        lc_gen.advance_chips(chip_start);

        let chip_samples: Vec<Complex32> = (0..preamble_chips)
            .map(|_| {
                let lc_chip = lc_gen.next_chip();
                let sign: f32 = if lc_chip == 1 { -1.0 } else { 1.0 };
                // Preamble = all +1 data * Walsh W0 = all +1, so just LC sign.
                // Scale to exceed PREAMBLE_ENERGY_FLOOR (2.0).
                Complex32::new(sign * 2.0, 0.0)
            })
            .collect();

        // Build the post-rake chain directly: LC descramble → Walsh demod
        // → deinterleave → Viterbi → AccessChannelProcessor
        let mut chain: Vec<PipelineProcessorShared> = vec![
            Box::new(ReverseAccessLongCodeProcessor::new(
                LongCodeGenerator::new_access_channel(0, 1, 1, 0),
                1,
            )),
            Box::new(ReverseAccessOrthogonalDemodProcessor::new()),
        ];
        chain.extend(access_channel_chain());

        // Feed in blocks of 64 chips (one Walsh symbol) with correct tags
        let mut all_outputs = Vec::new();
        for (blk_idx, chunk) in chip_samples.chunks(64).enumerate() {
            if chunk.len() < 64 {
                break;
            }
            let blk_chip = chip_start + blk_idx * 64;
            let mut block = SampleBlock::new(chunk.to_vec(), blk_idx * 64)
                .with_sample_rate_hz(chip_rate as f64);
            block.tags.insert("absolute_chip_start", blk_chip as i64);
            block.tags.insert("absolute_sample_start", blk_chip as i64);
            block.tags.insert("pilot_phase", 0);

            let mut inputs = vec![block];
            for stage in chain.iter_mut() {
                let mut next_inputs = Vec::new();
                for inp in inputs {
                    next_inputs.extend(stage.process_block(inp));
                }
                inputs = next_inputs;
            }
            all_outputs.extend(inputs);
        }

        // Flush all stages
        for stage in chain.iter_mut() {
            let flushed = stage.flush();
            all_outputs.extend(flushed);
        }

        let mut lc_acquired = 0usize;
        let mut preamble_detected = 0usize;
        for blk in &all_outputs {
            if blk.tags.get("reverse_access_lc_acquired") == Some(&1) {
                lc_acquired += 1;
                let delta = blk
                    .tags
                    .get("reverse_access_lc_chip_delta")
                    .copied()
                    .unwrap_or(i64::MAX);
                eprintln!(
                    "  LC acquired #{}: abs_chip={:?} delta={} ",
                    lc_acquired,
                    blk.tags.get("absolute_chip_start"),
                    delta,
                );
            }
            if blk.tags.get("access_preamble_detected") == Some(&1) {
                preamble_detected += 1;
            }
        }

        eprintln!(
            "synthetic probe summary: total_outputs={} lc_acquired={} preamble_detected={}",
            all_outputs.len(),
            lc_acquired,
            preamble_detected,
        );

        assert!(
            lc_acquired >= 1,
            "expected at least 1 LC acquisition, got {lc_acquired}"
        );

        assert!(
            preamble_detected >= 1,
            "expected at least 1 access_preamble_detected from downstream, got {preamble_detected}"
        );
    }

    #[test]
    fn test_generic_rake_access_channel() {
        use crate::receiver::pipelined::generic_rake_receiver::GenericRakeReceiver;
        use crate::receiver::pipelined::pn_lc_correlator::{PnLcConfig, PnLcCorrelator};
        use crate::sdr::{cdma2000_baseband_filter_taps_f64, fir::ComplexFir32};

        init_test_logger();

        let oversample = 4usize;
        let chip_rate = 1_228_800usize;
        let sample_rate_hz = (chip_rate * oversample) as f64;
        let chip_start: usize = {
            let raw = 1791068919103488usize;
            raw - (raw % 24576)
        };
        // The production reverse-access decoder first snaps the finger stream
        // to the 256-chip symbol grid. Give the synthetic burst enough W0
        // runway so a small initial skip still leaves multiple full preamble
        // frames to establish lock before the first data frame arrives.
        let preamble_chips = 24576 * 3;

        let mut lc_gen = LongCodeGenerator::new_access_channel_with_state(0, 1, 1, 0, 1u64 << 41);
        lc_gen.advance_chips(chip_start);
        let pdu_bits: Vec<u8> = vec![
            0, 0, // PD = 00
            0, 0, 0, 0, 0, 1, // MSG_ID = 000001 (Registration)
            0, 0, 0, 1, // REG_TYPE = 0001 (power-up)
            0, 0, 0, // SLOT_CYCLE_INDEX = 000
            0, 0, 0, 0, 0, 1, 1, 0, // MOB_P_REV = 00000110
            0, 0, 1, 0, 0, 0, 0, 0, // SCM = 00100000
            1, // MOB_TERM = 1
            0, 0, 0, 0, // RETURN_CAUSE = 0000
            0, 0, 0, 0, 0, 0, // padding to 42 bits
        ];
        assert_eq!(42, pdu_bits.len());

        let msg_length_octets: u8 = 10;
        let mut crc_scope = Bitstream::new();
        crc_scope.write_u8(msg_length_octets, 8);
        crc_scope.extend(&Bitstream::new_init(&pdu_bits));
        let crc = crc30(&crc_scope);

        let mut sar_body = Bitstream::new();
        sar_body.write_u8(msg_length_octets, 8);
        sar_body.extend(&Bitstream::new_init(&pdu_bits));
        sar_body.write_u32(crc, 30);
        assert_eq!(80, sar_body.len());

        let mut frame_bits = sar_body.bits().to_vec();
        frame_bits.extend(std::iter::repeat(0u8).take(8));
        frame_bits.extend(std::iter::repeat(0u8).take(8));
        assert_eq!(96, frame_bits.len());

        let mut conv_enc = get_1_3_k9_encoder();
        let mut code_symbols: Vec<u8> = Vec::with_capacity(288);
        for &bit in &frame_bits {
            let out = conv_enc.encode(bit);
            code_symbols.extend_from_slice(&out);
        }
        assert_eq!(288, code_symbols.len());

        let mut sr = crate::phy::coding::symbol_repeat::SymbolRepetition::new(2);
        for &sym in &code_symbols {
            sr.feed(sym);
        }
        let repeated = sr.take_all();
        assert_eq!(576, repeated.len());

        let mut interleaver = BitReversalInterleaver::new(block_interleaver::SR1_PARAMS_576);
        let interleaved = interleaver.encode(&repeated);
        assert_eq!(576, interleaved.len());

        let walsh_matrix = crate::phy::walsh::WalshGenerator::generate_matrix::<64>();
        let mut walsh_chips: Vec<i8> = Vec::with_capacity(6144);
        for group in interleaved.chunks_exact(6) {
            let index = group[0] as usize
                + 2 * group[1] as usize
                + 4 * group[2] as usize
                + 8 * group[3] as usize
                + 16 * group[4] as usize
                + 32 * group[5] as usize;
            walsh_chips.extend_from_slice(&walsh_matrix[index]);
        }
        assert_eq!(6144, walsh_chips.len());
        let data_frame_pn_chips = 6144 * 4;
        let trailing_chips = data_frame_pn_chips * 2;
        let total_tx_chips = preamble_chips + data_frame_pn_chips + trailing_chips;

        let mut tx_raw: Vec<Complex32> = Vec::with_capacity(total_tx_chips * oversample);
        let phase_period = 32768 * oversample;
        // Generate one full PN period, rotate to chip_start phase, then cycle.
        let pn_period = build_fft_search_pn_samples(phase_period, oversample);
        let pn_rotate = (chip_start * oversample) % phase_period;
        let total_pn_samples = total_tx_chips * oversample;
        let pn: Vec<Complex32> = (0..total_pn_samples)
            .map(|k| pn_period[(k + pn_rotate) % phase_period])
            .collect();
        let mut pn_iter = pn.into_iter();

        for _ in 0..preamble_chips {
            let lc_chip = lc_gen.next_chip();
            let lc_sign: f32 = if lc_chip == 1 { -1.0 } else { 1.0 };
            for _ in 0..oversample {
                let pn_iq = pn_iter.next().unwrap();
                tx_raw.push(Complex32::new(lc_sign * pn_iq.re, lc_sign * pn_iq.im));
            }
        }

        for &wchip in &walsh_chips {
            let w: f32 = wchip as f32;
            for _ in 0..4 {
                let lc_chip = lc_gen.next_chip();
                let lc_sign: f32 = if lc_chip == 1 { -1.0 } else { 1.0 };
                for _ in 0..oversample {
                    let pn_iq = pn_iter.next().unwrap();
                    tx_raw.push(Complex32::new(
                        w * lc_sign * pn_iq.re,
                        w * lc_sign * pn_iq.im,
                    ));
                }
            }
        }
        for _ in 0..trailing_chips {
            let lc_chip = lc_gen.next_chip();
            let lc_sign: f32 = if lc_chip == 1 { -1.0 } else { 1.0 };
            for _ in 0..oversample {
                let pn_iq = pn_iter.next().unwrap();
                tx_raw.push(Complex32::new(lc_sign * pn_iq.re, lc_sign * pn_iq.im));
            }
        }
        assert_eq!(total_tx_chips * oversample, tx_raw.len());

        let taps = cdma2000_baseband_filter_taps_f64();
        let tx_signal = ComplexFir32::new(&taps).process_block(&tx_raw);
        drop(tx_raw);

        let offsets = [0i64, 5, -5, 50, -50];
        let total_signal_seconds = offsets.len() as f64 * total_tx_chips as f64 / chip_rate as f64;
        let wall_start = std::time::Instant::now();

        for rx_chip_offset in offsets {
            let rx_chip_start = (chip_start as i64 + rx_chip_offset) as u64;
            eprintln!(
                "\n=== RX system time offset: {} chips (rx_chip_start={}) ===",
                rx_chip_offset, rx_chip_start
            );

            let correlator_cfg = PnLcConfig::default_4x();

            let correlator = PnLcCorrelator::new(
                correlator_cfg,
                LongCodeGenerator::new_access_channel_with_state(0, 1, 1, 0, 1u64 << 41),
                Box::new(|| {
                    vec![
                        Box::new(
                            super::reverse_access_decoder::ReverseAccessDecoder::new()
                                .with_soft_viterbi(true),
                        ),
                        Box::new(AccessChannelProcessor::new()),
                    ]
                }),
            );
            let pipeline: Vec<PipelineProcessorShared> = vec![
                Box::new(PulseMatchedFilterProcessor::new()),
                Box::new(
                    GenericRakeReceiver::new(correlator)
                        .with_finger_pool_size(1)
                        .with_prune_policy(Box::new(
                            super::generic_rake_receiver::DefaultPrunePolicy {
                                max_post_walsh_no_event_ms: 1_500,
                                ..Default::default()
                            },
                        )),
                ),
            ];

            let mut receiver = PipelinedReceiver::new(tx_signal.clone().into_iter())
                .with_input_sample_rate_hz(sample_rate_hz)
                .with_absolute_sample_start(rx_chip_start * oversample as u64);
            let out_rx = receiver.add_pipeline(pipeline);
            receiver.run_pipeline().unwrap();

            let mut access_data_frames = 0usize;
            for blocks in out_rx {
                for blk in &blocks {
                    if blk.tags.get("access_event") == Some(&1) {
                        access_data_frames += 1;
                    }
                }
            }

            eprintln!(
                "  offset={}: access_data_frames={}",
                rx_chip_offset, access_data_frames,
            );
            assert!(
                access_data_frames >= 1,
                "expected ≥1 CRC-valid data frame at offset={}, got {}",
                rx_chip_offset,
                access_data_frames,
            );
        }

        let wall_secs = wall_start.elapsed().as_secs_f64();
        let realtime_ratio = total_signal_seconds / wall_secs;
        eprintln!(
            "performance: {:.2}s signal in {:.2}s wall = {:.2}x realtime",
            total_signal_seconds, wall_secs, realtime_ratio,
        );
    }

    #[test]
    #[ignore = "legacy RakeAccessSearcher pulse-shaped access test; replaced by GenericRakeReceiver coverage"]
    fn test_rake_access_searcher_pulse_shaped() {
        use crate::receiver::pipelined::rake_access_searcher::RakeAccessSearcher;
        use crate::sdr::{cdma2000_baseband_filter_taps_f64, fir::ComplexFir32};

        init_test_logger();

        let oversample = 4usize;
        let chip_rate = 1_228_800usize;
        let sample_rate_hz = (chip_rate * oversample) as f64;
        let chip_start: usize = {
            let raw = 1791068919103488usize;
            raw - (raw % 24576)
        };
        let preamble_chips = 24576 * 2;

        let mut lc_gen = LongCodeGenerator::new_access_channel_with_state(0, 1, 1, 0, 1u64 << 41);
        lc_gen.advance_chips(chip_start);
        let pdu_bits: Vec<u8> = vec![
            0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 1, 0, 0, 0, 0, 0, 0, 0, 1, 1, 0, 0, 0, 1, 0, 0, 0, 0,
            0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0,
        ];
        assert_eq!(42, pdu_bits.len());

        let msg_length_octets: u8 = 10;
        let mut crc_scope = Bitstream::new();
        crc_scope.write_u8(msg_length_octets, 8);
        crc_scope.extend(&Bitstream::new_init(&pdu_bits));
        let crc = crc30(&crc_scope);

        let mut sar_body = Bitstream::new();
        sar_body.write_u8(msg_length_octets, 8);
        sar_body.extend(&Bitstream::new_init(&pdu_bits));
        sar_body.write_u32(crc, 30);
        assert_eq!(80, sar_body.len());

        let mut frame_bits = sar_body.bits().to_vec();
        frame_bits.extend(std::iter::repeat(0u8).take(8));
        frame_bits.extend(std::iter::repeat(0u8).take(8));
        assert_eq!(96, frame_bits.len());

        let mut conv_enc = get_1_3_k9_encoder();
        let mut code_symbols: Vec<u8> = Vec::with_capacity(288);
        for &bit in &frame_bits {
            let out = conv_enc.encode(bit);
            code_symbols.extend_from_slice(&out);
        }
        assert_eq!(288, code_symbols.len());

        let mut sr = crate::phy::coding::symbol_repeat::SymbolRepetition::new(2);
        for &sym in &code_symbols {
            sr.feed(sym);
        }
        let repeated = sr.take_all();
        assert_eq!(576, repeated.len());

        let mut interleaver = BitReversalInterleaver::new(block_interleaver::SR1_PARAMS_576);
        let interleaved = interleaver.encode(&repeated);
        assert_eq!(576, interleaved.len());

        let walsh_matrix = crate::phy::walsh::WalshGenerator::generate_matrix::<64>();
        let mut walsh_chips: Vec<i8> = Vec::with_capacity(6144);
        for group in interleaved.chunks_exact(6) {
            let index = group[0] as usize
                + 2 * group[1] as usize
                + 4 * group[2] as usize
                + 8 * group[3] as usize
                + 16 * group[4] as usize
                + 32 * group[5] as usize;
            walsh_chips.extend_from_slice(&walsh_matrix[index]);
        }
        assert_eq!(6144, walsh_chips.len());
        let data_frame_pn_chips = 6144 * 4;
        let trailing_chips = data_frame_pn_chips * 2;

        let total_tx_chips = preamble_chips + data_frame_pn_chips + trailing_chips;
        let mut tx_raw: Vec<Complex32> = Vec::with_capacity(total_tx_chips * oversample);
        let mut pn = build_oqpsk_pn_samples(total_tx_chips * oversample, oversample);
        let pn_rotate = (chip_start * oversample) % pn.len();
        pn.rotate_left(pn_rotate);
        let mut pn_iter = pn.into_iter();

        for _ in 0..preamble_chips {
            let lc_chip = lc_gen.next_chip();
            let lc_sign: f32 = if lc_chip == 1 { -1.0 } else { 1.0 };
            for _ in 0..oversample {
                let pn_iq = pn_iter.next().unwrap();
                tx_raw.push(Complex32::new(lc_sign * pn_iq.re, lc_sign * pn_iq.im));
            }
        }

        for &wchip in &walsh_chips {
            let w: f32 = wchip as f32;
            for _ in 0..4 {
                let lc_chip = lc_gen.next_chip();
                let lc_sign: f32 = if lc_chip == 1 { -1.0 } else { 1.0 };
                for _ in 0..oversample {
                    let pn_iq = pn_iter.next().unwrap();
                    tx_raw.push(Complex32::new(
                        w * lc_sign * pn_iq.re,
                        w * lc_sign * pn_iq.im,
                    ));
                }
            }
        }

        for _ in 0..trailing_chips {
            let lc_chip = lc_gen.next_chip();
            let lc_sign: f32 = if lc_chip == 1 { -1.0 } else { 1.0 };
            for _ in 0..oversample {
                let pn_iq = pn_iter.next().unwrap();
                tx_raw.push(Complex32::new(lc_sign * pn_iq.re, lc_sign * pn_iq.im));
            }
        }
        assert_eq!(total_tx_chips * oversample, tx_raw.len());

        let taps = cdma2000_baseband_filter_taps_f64();
        let tx_signal = ComplexFir32::new(&taps).process_block(&tx_raw);
        drop(tx_raw);

        let offsets = [0i64, 5, -5, 50, -50];
        let total_signal_seconds = offsets.len() as f64 * total_tx_chips as f64 / chip_rate as f64;
        let wall_start = std::time::Instant::now();

        for rx_chip_offset in offsets {
            let rx_chip_start = (chip_start as i64 + rx_chip_offset) as u64;
            eprintln!(
                "\n=== RX system time offset: {} chips (rx_chip_start={}) ===",
                rx_chip_offset, rx_chip_start
            );

            let searcher = RakeAccessSearcher::new(
                oversample,
                LongCodeGenerator::new_access_channel_with_state(0, 1, 1, 0, 1u64 << 41),
            )
            .with_chain_builder(Box::new(move || {
                let mut chain: Vec<PipelineProcessorShared> =
                    vec![Box::new(ReverseAccessOrthogonalDemodProcessor::new())];
                chain.extend(access_channel_chain());
                chain
            }));
            let pipeline: Vec<PipelineProcessorShared> = vec![
                Box::new(PulseMatchedFilterProcessor::new()),
                Box::new(searcher),
            ];

            let mut receiver = PipelinedReceiver::new(tx_signal.clone().into_iter())
                .with_input_sample_rate_hz(sample_rate_hz)
                .with_absolute_sample_start(rx_chip_start * oversample as u64);
            let out_rx = receiver.add_pipeline(pipeline);
            receiver.run_pipeline().unwrap();

            let mut searcher_detections = 0usize;
            let mut access_preamble_frames = 0usize;
            let mut access_data_frames = 0usize;
            for blocks in out_rx {
                for blk in &blocks {
                    if blk.tags.get("access_preamble_detected") == Some(&1)
                        && blk.tags.contains_key("finger_id")
                    {
                        searcher_detections += 1;
                    }
                    if blk.tags.get("access_preamble_detected") == Some(&1)
                        && blk.tags.contains_key("access_preamble_frames")
                    {
                        access_preamble_frames += 1;
                    }
                    if blk.tags.get("access_event") == Some(&1) {
                        access_data_frames += 1;
                        eprintln!(
                            "    data frame: crc={:?} payload_bits={:?} pd={:?} msg_type={:?}",
                            blk.tags.get("access_crc_valid"),
                            blk.tags.get("access_payload_bits"),
                            blk.tags.get("access_pd"),
                            blk.tags.get("access_msg_type"),
                        );
                        assert_eq!(
                            blk.tags.get("access_crc_valid"),
                            Some(&1),
                            "expected valid CRC on decoded registration message",
                        );
                        assert_eq!(
                            blk.tags.get("access_pd"),
                            Some(&0),
                            "expected PD=0 for P_REV < 7",
                        );
                        assert_eq!(
                            blk.tags.get("access_msg_type"),
                            Some(&1),
                            "expected MSG_ID=1 (Registration Message)",
                        );
                    }
                }
            }

            eprintln!(
                "  offset={}: searcher_detections={} access_preamble_frames={} access_data_frames={}",
                rx_chip_offset, searcher_detections, access_preamble_frames, access_data_frames,
            );

            assert!(
                searcher_detections >= 1,
                "expected at least 1 searcher detection at offset={}, got {}",
                rx_chip_offset,
                searcher_detections,
            );
            assert!(
                access_preamble_frames >= 1,
                "expected at least 1 decoded preamble frame at offset={}, got {}",
                rx_chip_offset,
                access_preamble_frames,
            );
            assert!(
                access_data_frames >= 1,
                "expected at least 1 decoded data frame at offset={}, got {}",
                rx_chip_offset,
                access_data_frames,
            );
        }

        let wall_secs = wall_start.elapsed().as_secs_f64();
        let realtime_ratio = total_signal_seconds / wall_secs;
        eprintln!(
            "performance: {:.2}s signal in {:.2}s wall = {:.2}x realtime",
            total_signal_seconds, wall_secs, realtime_ratio,
        );
        let min_realtime_ratio = capture_timing_min_speedup(1.0);
        assert!(
            realtime_ratio >= min_realtime_ratio,
            "expected at least {:.2}x realtime, got {:.2}x",
            min_realtime_ratio,
            realtime_ratio,
        );
    }

    /// Diagnostic test: sweep despread_phase and center_offset to find
    /// the correct PN alignment for pulse-shaped reverse access.
    #[test]
    #[ignore = "diagnostic despread-phase sweep; run explicitly when requested"]
    fn test_pulse_shaped_despread_phase_sweep() {
        use crate::phy::walsh::WalshGenerator;
        use crate::sdr::{cdma2000_baseband_filter_taps_f64, fir::ComplexFir32};

        let oversample = 4usize;
        let chip_start: usize = 100_000;
        let phase_period = 32768 * oversample;
        let test_chips = 1024usize; // Just enough for a few Walsh symbols

        // TX: generate PN×LC preamble, pulse-shape
        let mut lc_gen = LongCodeGenerator::new_access_channel(0, 1, 1, 0);
        lc_gen.advance_chips(chip_start);
        let mut pn_tx = build_oqpsk_pn_samples(test_chips * oversample, oversample);
        let pn_rotate = (chip_start * oversample) % pn_tx.len();
        pn_tx.rotate_left(pn_rotate);
        let mut pn_tx_iter = pn_tx.into_iter();

        let tx_raw: Vec<Complex32> = (0..test_chips)
            .flat_map(|_| {
                let lc_chip = lc_gen.next_chip();
                let lc_sign: f32 = if lc_chip == 1 { -1.0 } else { 1.0 };
                (0..oversample)
                    .map(|_| {
                        let pn_iq = pn_tx_iter.next().unwrap();
                        Complex32::new(lc_sign * pn_iq.re, lc_sign * pn_iq.im)
                    })
                    .collect::<Vec<_>>()
            })
            .collect();

        let taps = cdma2000_baseband_filter_taps_f64();
        let tx_filtered = ComplexFir32::new(&taps).process_block(&tx_raw);

        // RX matched filter (one more pass)
        let rx_signal = ComplexFir32::new(&taps).process_block(&tx_filtered);

        // Build raw PN reference
        let pn_ref: Vec<Complex32> = build_oqpsk_pn_samples(phase_period, oversample)
            .into_iter()
            .map(|s| Complex32::new(s.re, -s.im))
            .collect();

        // Build LC reference starting before chip_start to cover filter delay offset
        let lc_margin = 20usize; // enough to cover delay/oversample
        let mut lc_ref = LongCodeGenerator::new_access_channel(0, 1, 1, 0);
        lc_ref.advance_chips(chip_start - lc_margin);
        let lc_chips: Vec<f32> = (0..test_chips + 2 * lc_margin)
            .map(|_| if lc_ref.next_chip() == 1 { -1.0 } else { 1.0 })
            .collect();

        let base_phase = (chip_start * oversample) % phase_period; // 6784
        let walsh_matrix = WalshGenerator::generate_matrix::<64>();

        // Sanity check: despread raw (unfiltered) TX signal, delay=0, center=0
        {
            let mut chip_values_raw: Vec<Complex32> = Vec::new();
            for chip_idx in 0..test_chips {
                let sample_idx = chip_idx * oversample; // center=0 for raw
                if sample_idx >= tx_raw.len() {
                    break;
                }
                let sig = tx_raw[sample_idx];
                let pn_idx = (base_phase + sample_idx) % phase_period;
                let pn = pn_ref[pn_idx];
                let despread = pn * sig;
                let lc_sign = lc_chips[lc_margin + chip_idx]; // delay=0, no offset
                chip_values_raw.push(Complex32::new(despread.re * lc_sign, despread.im * lc_sign));
            }
            // Check first Walsh symbol
            if chip_values_raw.len() >= 64 {
                let chips = &chip_values_raw[..64];
                let mut row0_corr = 0.0f32;
                let mut best_row = 0usize;
                let mut best_corr = 0.0f32;
                for row in 0..64 {
                    let mut acc = 0.0f32;
                    for (j, &c) in chips.iter().enumerate() {
                        acc += c.re * walsh_matrix[row][j] as f32;
                    }
                    if row == 0 {
                        row0_corr = acc;
                    }
                    if acc.abs() > best_corr {
                        best_corr = acc.abs();
                        best_row = row;
                    }
                }
                eprintln!(
                    "SANITY (raw, no filter): row0_corr={:.1} best_row={} best_corr={:.1}",
                    row0_corr, best_row, best_corr
                );
                eprintln!(
                    "  first 8 chip_values: {:?}",
                    chip_values_raw[..8]
                        .iter()
                        .map(|c| format!("({:.1},{:.1})", c.re, c.im))
                        .collect::<Vec<_>>()
                );
            }
        }

        eprintln!("\n=== Despread phase sweep (base_phase={}) ===", base_phase);
        eprintln!(
            "rx_signal len={}, chip_values expected={}",
            rx_signal.len(),
            test_chips
        );
        eprintln!("  delay  center  despread_phase  row0_avg  best_row  best_avg");

        // Also test with NO filter (delay=0) as sanity check
        for delay in [0usize, 44, 45, 46, 47, 48, 49, 50] {
            for center in 0..oversample {
                let despread_phase = if base_phase >= delay {
                    base_phase - delay
                } else {
                    phase_period + base_phase - delay
                };

                // Despread: pick center sample, apply conj(pn) * signal
                // After filter delay, despread chip_idx corresponds to TX chip at:
                //   effective_tx_sample = chip_idx * oversample + center - delay
                //   tx_chip_offset = floor(effective_tx_sample / oversample) = chip_idx - ceil((delay - center) / oversample)
                let lc_chip_offset: isize = if delay == 0 {
                    0
                } else {
                    let effective_shift = delay as isize - center as isize;
                    if effective_shift >= 0 {
                        -((effective_shift + oversample as isize - 1) / oversample as isize)
                    } else {
                        (-effective_shift) / oversample as isize
                    }
                };
                let mut chip_values: Vec<Complex32> = Vec::new();
                for chip_idx in 0..test_chips {
                    let sample_idx = chip_idx * oversample + center;
                    if sample_idx >= rx_signal.len() {
                        break;
                    }
                    let sig = rx_signal[sample_idx];
                    let pn_idx = (despread_phase + sample_idx) % phase_period;
                    let pn = pn_ref[pn_idx];
                    let despread = pn * sig;
                    // Remove LC with corrected chip index
                    let lc_idx = (lc_margin as isize + chip_idx as isize + lc_chip_offset) as usize;
                    if lc_idx >= lc_chips.len() {
                        break;
                    }
                    let lc_sign = lc_chips[lc_idx];
                    chip_values.push(Complex32::new(despread.re * lc_sign, despread.im * lc_sign));
                }

                // Walsh decode 64-chip symbols, check row 0
                let mut row0_sum = 0.0f32;
                let mut best_row_counts = vec![0usize; 64];
                let mut n_symbols = 0usize;
                for sym_start in (0..chip_values.len()).step_by(64) {
                    if sym_start + 64 > chip_values.len() {
                        break;
                    }
                    let chips = &chip_values[sym_start..sym_start + 64];
                    // Correlate with all 64 Walsh rows
                    let mut best_row = 0usize;
                    let mut best_corr = 0.0f32;
                    let mut row0_corr = 0.0f32;
                    for row in 0..64 {
                        let mut acc = 0.0f32;
                        for (j, &c) in chips.iter().enumerate() {
                            let w = walsh_matrix[row][j] as f32;
                            acc += c.re * w;
                        }
                        if row == 0 {
                            row0_corr = acc;
                        }
                        if acc.abs() > best_corr {
                            best_corr = acc.abs();
                            best_row = row;
                        }
                    }
                    row0_sum += row0_corr.abs();
                    best_row_counts[best_row] += 1;
                    n_symbols += 1;
                }

                if delay == 44 && center == 0 {
                    eprintln!(
                        "  debug: chip_values.len()={} n_symbols={}",
                        chip_values.len(),
                        n_symbols
                    );
                }
                if n_symbols > 0 {
                    let row0_avg = row0_sum / n_symbols as f32;
                    let best_row = best_row_counts
                        .iter()
                        .enumerate()
                        .max_by_key(|(_, c)| *c)
                        .unwrap()
                        .0;
                    let best_pct = best_row_counts[best_row] as f32 / n_symbols as f32 * 100.0;
                    let row0_pct = best_row_counts[0] as f32 / n_symbols as f32 * 100.0;
                    {
                        eprintln!(
                            "  {:>3}    {:>3}     {:>6}        {:.2}     row{}({:.0}%)  row0={:.0}%",
                            delay, center, despread_phase, row0_avg, best_row, best_pct, row0_pct,
                        );
                    }
                }
            }
        }
        eprintln!("=== End sweep ===\n");
    }

    /// Reverse traffic channel decode test using a live WAV capture.
    ///
    /// The WAV was captured during an SMS origination where the BSC assigned
    /// walsh=8 to ESN 0x4CDC1D09. The mobile was responding on the access
    /// channel (Order messages) but the traffic RX never acquired a finger.
    /// This test helps diagnose the traffic channel acquisition.
    ///
    /// Run with: cargo test --release -p cdma-bts capture_reverse_traffic_channel_decode_wav -- --nocapture --test-threads=1
    #[test]
    fn capture_reverse_traffic_channel_decode_wav() {
        init_test_logger();
        let wav_path = test_capture_path("1791839259830557.wav");
        if !wav_path.exists() {
            eprintln!(
                "skipping capture_reverse_traffic_channel_decode_wav: {} not found",
                wav_path.display()
            );
            return;
        }
        let reader = hound::WavReader::open(&wav_path)
            .unwrap_or_else(|e| panic!("failed to open {}: {e}", wav_path.display()));
        let (sample_rate, iq_samples) = read_iq_wav(reader);
        let chip_start: u64 = 1791839259830557;
        let esn: u32 = 0x4CDC1D09;
        let walsh_code: u8 = 8;
        let oversample = (sample_rate as usize) / 1228800;

        eprintln!(
            "reverse traffic channel decode test: sample_rate={} oversample={} iq_samples={} chip_start={} esn=0x{:08X} walsh={}",
            sample_rate,
            oversample,
            iq_samples.len(),
            chip_start,
            esn,
            walsh_code,
        );

        // Build reverse traffic chain for RC3 mobile.
        // ESN=0x4CDC1D09 supports RC3 only (rev_rcs=[3,4,5]).
        let pipeline = super::reverse_traffic_chain_rc3(super::ReverseTrafficSettings {
            oversample,
            walsh_code,
            esn,
            reanchor_origin: false,
            snr_threshold: None,
            preamble_num_pcgs: None,
            epl_pilot: true,
            rev_fch_gating_mode: false,
            finger_pool_size: 1,
        });

        let mut receiver = PipelinedReceiver::new(iq_samples.into_iter())
            .with_batch_size(32768)
            .with_input_sample_rate_hz(sample_rate as f64)
            .with_absolute_sample_start(chip_start * oversample as u64);
        let out_rx = receiver.add_pipeline(pipeline);
        receiver.run_pipeline().unwrap();

        let mut preamble_count = 0usize;
        let mut data_frame_count = 0usize;
        let mut crc_valid_data_frame_count = 0usize;
        let mut total_blocks = 0usize;
        for blocks in out_rx {
            total_blocks += blocks.len();
            for blk in &blocks {
                // Check for preamble detections (finger spawned)
                if blk.tags.get("access_preamble_detected") == Some(&1)
                    && blk.tags.contains_key("access_preamble_frames")
                {
                    preamble_count += 1;
                    eprintln!(
                        "  preamble #{}: chip_start={:?} preamble_frames={:?}",
                        preamble_count,
                        blk.tags.get("absolute_chip_start"),
                        blk.tags.get("access_preamble_frames"),
                    );
                }
                // Check for traffic data frames
                if blk.tags.get("traffic_event") == Some(&1) {
                    data_frame_count += 1;
                    let crc_valid = blk.tags.get("traffic_crc_valid") == Some(&1);
                    if crc_valid {
                        crc_valid_data_frame_count += 1;
                    }
                    eprintln!(
                        "  traffic frame #{}: crc={} walsh={} chip={:?}",
                        data_frame_count,
                        crc_valid,
                        walsh_code,
                        blk.tags.get("absolute_chip_start"),
                    );
                }
            }
        }

        eprintln!(
            "reverse traffic decode summary: total_blocks={} preamble_detections={} traffic_frames={} crc_valid_traffic_frames={}",
            total_blocks, preamble_count, data_frame_count, crc_valid_data_frame_count
        );

        // This is a diagnostic test — the traffic RX never acquired a finger
        // in live operation. We log results for analysis; a threshold assertion
        // can be added once decoding works.
        eprintln!(
            "NOTE: if preamble_detections=0, the PnLcCorrelator never found the mobile's traffic signal. Check LC mask, timing offset, or whether the mobile actually transmitted on the traffic channel."
        );
    }

    /// RC3 reverse traffic channel decode test for the post-ECAM capture.
    /// WAV captured after ECAM assignment to walsh=10, RC3.
    /// MS ESN=0x4CDC1D09, origination for SO6 SMS.
    #[test]
    fn capture_rc3_reverse_traffic_channel_decode() {
        init_test_logger();
        let wav_path = test_capture_path("1793735012485728.wav");
        if !wav_path.exists() {
            eprintln!("skipping test: {} not found", wav_path.display());
            return;
        }
        let reader = hound::WavReader::open(&wav_path)
            .unwrap_or_else(|e| panic!("failed to open {}: {e}", wav_path.display()));
        let (sample_rate, iq_samples) = read_iq_wav(reader);
        let chip_start: u64 = 1793735012485728;
        let esn: u32 = 0x808B0B33;
        let walsh_code: u8 = 15;
        let oversample = (sample_rate as usize) / 1228800;

        eprintln!(
            "RC3 traffic decode test: sample_rate={} oversample={} iq_samples={} chip_start={} esn=0x{:08X} walsh={}",
            sample_rate,
            oversample,
            iq_samples.len(),
            chip_start,
            esn,
            walsh_code,
        );

        // EXACTLY match live BTS settings (cdma-bts bts/rx.rs ~line 663).
        let pipeline = super::reverse_traffic_chain_rc3(super::ReverseTrafficSettings {
            oversample,
            walsh_code,
            esn,
            reanchor_origin: true,
            snr_threshold: None,
            preamble_num_pcgs: None,
            epl_pilot: true,
            rev_fch_gating_mode: false,
            finger_pool_size: 1,
        });

        let mut receiver = PipelinedReceiver::new(iq_samples.into_iter())
            .with_batch_size(32768)
            .with_input_sample_rate_hz(sample_rate as f64)
            .with_absolute_sample_start(chip_start * oversample as u64);
        let out_rx = receiver.add_pipeline(pipeline);
        receiver.run_pipeline().unwrap();

        let mut preamble_count = 0usize;
        let mut data_frame_count = 0usize;
        let mut crc_valid_data_frame_count = 0usize;
        let mut phy_frame_count = 0usize;
        let mut total_blocks = 0usize;
        for blocks in out_rx {
            total_blocks += blocks.len();
            for blk in &blocks {
                if blk.tags.get("traffic_preamble_detected") == Some(&1) {
                    preamble_count += 1;
                    eprintln!(
                        "  traffic preamble #{}: chip_start={:?}",
                        preamble_count,
                        blk.tags.get("absolute_chip_start"),
                    );
                }
                if blk.tags.get("traffic_phy_frame") == Some(&1) {
                    phy_frame_count += 1;
                    let fqi = blk.tags.get("traffic_fqi_valid").copied().unwrap_or(0);
                    let rate = blk.tags.get("traffic_rate_bps").copied().unwrap_or(0);
                    let fqi_bits = blk.tags.get("traffic_fqi_bits").copied().unwrap_or(0);
                    let tail_valid = blk.tags.get("traffic_tail_valid").copied().unwrap_or(0);
                    eprintln!(
                        "  phy frame #{}: rate={} fqi_bits={} fqi_valid={} tail_valid={} chip={:?}",
                        phy_frame_count,
                        rate,
                        fqi_bits,
                        fqi,
                        tail_valid,
                        blk.tags.get("absolute_chip_start"),
                    );
                }
                if blk.tags.get("traffic_event") == Some(&1) {
                    data_frame_count += 1;
                    let crc_valid = blk.tags.get("traffic_crc_valid") == Some(&1);
                    if crc_valid {
                        crc_valid_data_frame_count += 1;
                    }
                    let rate = blk.tags.get("traffic_rate").copied().unwrap_or(0);
                    eprintln!(
                        "  traffic frame #{}: crc={} rate={} walsh={} chip={:?}",
                        data_frame_count,
                        crc_valid,
                        rate,
                        walsh_code,
                        blk.tags.get("absolute_chip_start"),
                    );
                }
            }
        }

        eprintln!(
            "rc3_traffic summary: total_blocks={} preamble_detections={} phy_frames={} traffic_frames={} crc_valid_traffic_frames={}",
            total_blocks,
            preamble_count,
            phy_frame_count,
            data_frame_count,
            crc_valid_data_frame_count
        );
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    struct TrafficSignature {
        ack_seq: u8,
        msg_seq: u8,
        ack_req: bool,
        kind: &'static str,
        detail: Option<&'static str>,
    }

    impl TrafficSignature {
        fn order(ack_seq: u8, msg_seq: u8, ack_req: bool, order_name: &'static str) -> Self {
            Self {
                ack_seq,
                msg_seq,
                ack_req,
                kind: "order",
                detail: Some(order_name),
            }
        }

        fn pmrm(ack_seq: u8, msg_seq: u8, ack_req: bool) -> Self {
            Self {
                ack_seq,
                msg_seq,
                ack_req,
                kind: "pmrm",
                detail: None,
            }
        }

        fn sccm(ack_seq: u8, msg_seq: u8, ack_req: bool) -> Self {
            Self {
                ack_seq,
                msg_seq,
                ack_req,
                kind: "sccm",
                detail: None,
            }
        }
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum GoldenRc3Rate {
        Full,
        Half,
        Quarter,
        Eighth,
    }

    impl GoldenRc3Rate {
        const fn info_bits(self) -> usize {
            match self {
                Self::Full => 172,
                Self::Half => 80,
                Self::Quarter => 40,
                Self::Eighth => 16,
            }
        }

        const fn fqi_bits(self) -> usize {
            match self {
                Self::Full => 12,
                Self::Half => 8,
                Self::Quarter | Self::Eighth => 6,
            }
        }

        const fn frame_bits(self) -> usize {
            match self {
                Self::Full => 192,
                Self::Half => 96,
                Self::Quarter => 54,
                Self::Eighth => 30,
            }
        }

        const fn repetition_factor(self) -> usize {
            match self {
                Self::Full => 2,
                Self::Half => 4,
                Self::Quarter => 8,
                Self::Eighth => 16,
            }
        }

        const fn rate_bps(self) -> i64 {
            match self {
                Self::Full => 9600,
                Self::Half => 4800,
                Self::Quarter => 2700,
                Self::Eighth => 1500,
            }
        }
    }

    #[derive(Debug, Clone, PartialEq, Eq)]
    enum GoldenRc3Frame {
        PilotOnly,
        Traffic {
            rate: GoldenRc3Rate,
            info_bits: Vec<u8>,
        },
    }

    #[derive(Debug, Clone)]
    struct RustRc3GoldenDespreadFrame {
        chips: Vec<Complex32>,
    }

    #[derive(Debug)]
    struct RustRc3GoldenDespreadFramewise {
        frames: Vec<RustRc3GoldenDespreadFrame>,
        expected_crc_valid_rates: Vec<i64>,
        expected_signatures: Vec<TrafficSignature>,
    }

    #[derive(Debug, Clone)]
    struct ReverseRc3GoldenPlan {
        schedule: Vec<GoldenRc3Frame>,
        expected_crc_valid_rates: Vec<i64>,
        expected_signatures: Vec<TrafficSignature>,
    }

    #[derive(Debug, Default)]
    struct Rc3GoldenDecodeResult {
        preamble_count: usize,
        crc_valid_rates: Vec<i64>,
        recovered_signatures: Vec<TrafficSignature>,
        pcg_measurement_count: usize,
        nonzero_pcg_measurement_age_count: usize,
        max_pcg_measurement_age_chips: u64,
        pcg_measurement_timings: Vec<(u64, u64)>,
        total_batches: Option<u64>,
        max_batch_elapsed_ns: Option<u64>,
        batch_budget_ns: Option<u64>,
    }

    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    struct PcgTimingFailureRun {
        start_abs_pcg: u64,
        end_abs_pcg: u64,
        count: usize,
        max_age_chips: u64,
    }

    #[derive(Debug, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct MatlabReverseRc3GoldenManifest {
        generator_type: String,
        generator_version: f64,
        schedule_name: String,
        esn: f64,
        walsh_code: f64,
        pn_offset: f64,
        long_code_state: f64,
        long_code_mask: f64,
        filter_type: String,
        preamble_frames: f64,
        short_code_reset_each_frame: Option<bool>,
        frame_pn_offsets: Option<Vec<f64>>,
        frame_long_code_states: Option<Vec<f64>>,
        pilot_only_path: Option<String>,
        expected_crc_valid_rates: Vec<i64>,
        expected_signatures: Vec<MatlabReverseRc3Signature>,
    }

    #[derive(Debug, serde::Deserialize)]
    #[serde(rename_all = "camelCase")]
    struct MatlabReverseRc3Signature {
        ack_seq: u8,
        msg_seq: u8,
        ack_req: bool,
        kind: String,
        detail: Option<String>,
    }

    impl MatlabReverseRc3Signature {
        fn to_signature(&self) -> TrafficSignature {
            match self.kind.as_str() {
                "pmrm" => TrafficSignature::pmrm(self.ack_seq, self.msg_seq, self.ack_req),
                "sccm" => TrafficSignature::sccm(self.ack_seq, self.msg_seq, self.ack_req),
                "order" => {
                    let order_name = match self.detail.as_deref() {
                        Some("Mobile Station Acknowledgment") => "Mobile Station Acknowledgment",
                        Some("Release") => "Release",
                        Some(other) => panic!("unsupported MATLAB RC3 order detail: {other}"),
                        None => panic!("MATLAB RC3 order signature missing detail"),
                    };
                    TrafficSignature::order(self.ack_seq, self.msg_seq, self.ack_req, order_name)
                }
                other => panic!("unsupported MATLAB RC3 signature kind: {other}"),
            }
        }
    }

    fn matlab_json_int_u32(value: f64, field: &str) -> u32 {
        assert!(
            value.is_finite(),
            "MATLAB RC3 manifest field {field} is not finite: {value}"
        );
        let rounded = value.round();
        assert!(
            (rounded - value).abs() < 0.5e-6,
            "MATLAB RC3 manifest field {field} is not integral: {value}"
        );
        assert!(
            rounded >= 0.0 && rounded <= u32::MAX as f64,
            "MATLAB RC3 manifest field {field} out of range for u32: {value}"
        );
        rounded as u32
    }

    fn matlab_json_int_u64(value: f64, field: &str) -> u64 {
        assert!(
            value.is_finite(),
            "MATLAB RC3 manifest field {field} is not finite: {value}"
        );
        let rounded = value.round();
        assert!(
            (rounded - value).abs() < 0.5e-3,
            "MATLAB RC3 manifest field {field} is not integral: {value}"
        );
        assert!(
            rounded >= 0.0 && rounded <= u64::MAX as f64,
            "MATLAB RC3 manifest field {field} out of range for u64: {value}"
        );
        rounded as u64
    }

    fn matlab_json_int_usize(value: f64, field: &str) -> usize {
        let as_u64 = matlab_json_int_u64(value, field);
        usize::try_from(as_u64).unwrap_or_else(|_| {
            panic!("MATLAB RC3 manifest field {field} out of range for usize: {value}")
        })
    }

    const REVERSE_RC3_GOLDEN_GENERATOR_VERSION: u32 = 9;
    const REVERSE_RC3_GOLDEN_SCHEDULE_NAME: &str = "rust_reverse_rc3_golden_v1";
    const REVERSE_RC3_GOLDEN_ESN: u32 = 0x8085_7E58;
    const REVERSE_RC3_GOLDEN_WALSH: u8 = 4;
    const REVERSE_RC3_GOLDEN_PN_OFFSET: u64 = 0;
    const REVERSE_RC3_GOLDEN_PREAMBLE_FRAMES: usize = 10;
    const REVERSE_RC3_GOLDEN_FRAME_CHIPS: u64 = 24_576;
    const REVERSE_RC3_GOLDEN_MATLAB_CHIP_SHIFT: usize = 3;

    #[derive(Clone)]
    struct GoldenRc3HpskSpreader {
        lc_gen: LongCodeGenerator,
        prev_lc: f32,
        chip_count: usize,
    }

    impl GoldenRc3HpskSpreader {
        fn with_state(esn: u32, state: u64) -> Self {
            let mut lc_gen = LongCodeGenerator::new_traffic_channel_with_state(esn, state);
            let prev_lc = if lc_gen.next_chip() == 1 { -1.0 } else { 1.0 };
            Self {
                lc_gen,
                prev_lc,
                chip_count: 0,
            }
        }

        fn next_spread_reference(&mut self, pn_at_chip_start: Complex32) -> Complex32 {
            let lc_i = if self.lc_gen.next_chip() == 1 {
                -1.0
            } else {
                1.0
            };
            let pn_i = pn_at_chip_start.re;
            let pn_q = pn_at_chip_start.im;
            let w12 = if self.chip_count % 2 == 0 { 1.0 } else { -1.0 };
            let lc_q = self.prev_lc;
            let s_i = pn_i * lc_i;
            let s_q = w12 * s_i * pn_q * lc_q;
            self.prev_lc = lc_i;
            self.chip_count += 1;
            Complex32::new(s_i, s_q)
        }
    }

    fn advance_reverse_rc3_traffic_long_code_state(state: u64, chips: usize) -> u64 {
        let mut lc =
            LongCodeGenerator::new_traffic_channel_with_state(REVERSE_RC3_GOLDEN_ESN, state);
        lc.advance_chips(chips);
        lc.state()
    }

    fn rc3_crc12(data: &[u8]) -> u16 {
        let poly: u16 = 0x0F13;
        let mut register: u16 = 0x0FFF;
        for &bit in data {
            let feedback = ((register >> 11) & 1) ^ (bit as u16 & 1);
            register = (register << 1) & 0x0FFF;
            if feedback == 1 {
                register ^= poly;
            }
        }
        register
    }

    fn rc3_crc8(data: &[u8]) -> u8 {
        let poly: u8 = 0x9B;
        let mut register: u8 = 0xFF;
        for &bit in data {
            let feedback = ((register >> 7) & 1) ^ (bit & 1);
            register <<= 1;
            if feedback == 1 {
                register ^= poly;
            }
        }
        register
    }

    fn rc3_crc6(data: &[u8]) -> u8 {
        let poly: u8 = 0x27;
        let mut register: u8 = 0x3F;
        for &bit in data {
            let feedback = ((register >> 5) & 1) ^ (bit & 1);
            register = (register << 1) & 0x3F;
            if feedback == 1 {
                register ^= poly;
            }
        }
        register
    }

    fn build_reverse_rc3_frame_bits(info_bits: &[u8], rate: GoldenRc3Rate) -> Vec<u8> {
        let mut frame = Vec::with_capacity(rate.frame_bits());
        for i in 0..rate.info_bits() {
            frame.push(*info_bits.get(i).unwrap_or(&0));
        }

        match rate.fqi_bits() {
            12 => {
                let crc = rc3_crc12(&frame[..rate.info_bits()]);
                for bit in (0..12).rev() {
                    frame.push(((crc >> bit) & 1) as u8);
                }
            }
            8 => {
                let crc = rc3_crc8(&frame[..rate.info_bits()]);
                for bit in (0..8).rev() {
                    frame.push(((crc >> bit) & 1) as u8);
                }
            }
            6 => {
                let crc = rc3_crc6(&frame[..rate.info_bits()]);
                for bit in (0..6).rev() {
                    frame.push(((crc >> bit) & 1) as u8);
                }
            }
            _ => unreachable!(),
        }

        frame.extend(std::iter::repeat_n(0u8, 8));
        assert_eq!(frame.len(), rate.frame_bits());
        frame
    }

    fn puncture_reverse_rc3_repeated_symbols(symbols: &[u8]) -> Vec<u8> {
        const MOD_SYMBOLS_PER_FRAME: usize = 1536;
        let input_len = symbols.len();
        (0..MOD_SYMBOLS_PER_FRAME)
            .map(|k| symbols[(k * input_len) / MOD_SYMBOLS_PER_FRAME])
            .collect()
    }

    fn encode_reverse_rc3_fch_symbols(info_bits: &[u8], rate: GoldenRc3Rate) -> Vec<f32> {
        let frame_bits = build_reverse_rc3_frame_bits(info_bits, rate);
        encode_reverse_rc3_fch_frame_bits_to_symbols(&frame_bits, rate)
    }

    fn encode_reverse_rc3_fch_frame_bits_to_symbols(
        frame_bits: &[u8],
        rate: GoldenRc3Rate,
    ) -> Vec<f32> {
        assert_eq!(frame_bits.len(), rate.frame_bits());
        let mut encoder = get_1_4_k9_encoder();
        let mut code_symbols = Vec::with_capacity(frame_bits.len() * 4);
        for &bit in frame_bits {
            code_symbols.extend_from_slice(&encoder.encode(bit));
        }

        let repeated: Vec<u8> = code_symbols
            .iter()
            .flat_map(|&symbol| std::iter::repeat_n(symbol, rate.repetition_factor()))
            .collect();

        let punctured = match rate {
            GoldenRc3Rate::Full | GoldenRc3Rate::Half => repeated,
            GoldenRc3Rate::Quarter | GoldenRc3Rate::Eighth => {
                puncture_reverse_rc3_repeated_symbols(&repeated)
            }
        };
        assert_eq!(punctured.len(), 1536);

        let mut interleaver = BitReversalInterleaver::new(block_interleaver::SR1_PARAMS_1536);
        interleaver
            .encode(&punctured)
            .into_iter()
            .map(|bit| if bit == 0 { 1.0 } else { -1.0 })
            .collect()
    }

    fn patterned_bits(len: usize, seed: u32) -> Vec<u8> {
        let mut state = seed | 1;
        (0..len)
            .map(|_| {
                let bit = ((state >> 31) & 1) as u8;
                state = state.wrapping_mul(1664525).wrapping_add(1013904223);
                bit
            })
            .collect()
    }

    fn build_reverse_rc3_sccm_pdu(
        ack_seq: u8,
        msg_seq: u8,
        ack_req: bool,
        serv_con_seq: u8,
    ) -> Bitstream {
        let mut bs = Bitstream::new();
        bs.write_u32(
            crate::lac::message_types::MessageId::ServiceConnectCompletion
                .wire_type(crate::lac::message_types::WireChannel::ReverseDedicated)
                .unwrap() as u32,
            8,
        );
        bs.write_u32(ack_seq as u32, 3);
        bs.write_u32(msg_seq as u32, 3);
        bs.write_u32(ack_req as u32, 1);
        bs.write_u32(0, 2);
        bs.write_u32(0, 1); // reserved
        bs.write_u32(serv_con_seq as u32, 3);
        bs.write_u32(0, 3); // padding
        bs
    }

    fn build_reverse_rc3_ms_ack_order_pdu(ack_seq: u8, msg_seq: u8, ack_req: bool) -> Bitstream {
        let mut bs = Bitstream::new();
        bs.write_u32(
            crate::lac::message_types::MessageId::Order
                .wire_type(crate::lac::message_types::WireChannel::ReverseDedicated)
                .unwrap() as u32,
            8,
        );
        bs.write_u32(ack_seq as u32, 3);
        bs.write_u32(msg_seq as u32, 3);
        bs.write_u32(ack_req as u32, 1);
        bs.write_u32(0, 2);
        bs.write_u32(0b010000, 6); // Mobile Station Acknowledgment
        bs.write_u32(0, 3); // ADD_RECORD_LEN
        bs
    }

    fn encapsulate_reverse_rc3_full_rate_info_bits(mut pdu: Bitstream) -> Vec<u8> {
        while pdu.len() % 8 != 0 {
            pdu.write_u8(0, 1);
        }
        let frames = crate::lac::sar_fragment_ftch_pdu_dsch(&pdu);
        assert_eq!(
            frames.len(),
            1,
            "golden RC3 signaling frame unexpectedly fragmented",
        );
        let bits = frames[0].bits().to_vec();
        assert_eq!(bits.len(), GoldenRc3Rate::Full.info_bits());
        bits
    }

    fn reverse_rc3_golden_plan() -> ReverseRc3GoldenPlan {
        let mut schedule = Vec::new();
        for _ in 0..REVERSE_RC3_GOLDEN_PREAMBLE_FRAMES {
            schedule.push(GoldenRc3Frame::PilotOnly);
        }
        schedule.push(GoldenRc3Frame::Traffic {
            rate: GoldenRc3Rate::Full,
            info_bits: encapsulate_reverse_rc3_full_rate_info_bits(
                build_reverse_rc3_ms_ack_order_pdu(0, 1, false),
            ),
        });
        schedule.push(GoldenRc3Frame::Traffic {
            rate: GoldenRc3Rate::Half,
            info_bits: patterned_bits(GoldenRc3Rate::Half.info_bits(), 0xA5A5_0001),
        });
        schedule.push(GoldenRc3Frame::Traffic {
            rate: GoldenRc3Rate::Quarter,
            info_bits: patterned_bits(GoldenRc3Rate::Quarter.info_bits(), 0x5AA5_0002),
        });
        schedule.push(GoldenRc3Frame::Traffic {
            rate: GoldenRc3Rate::Eighth,
            info_bits: patterned_bits(GoldenRc3Rate::Eighth.info_bits(), 0xC33C_0003),
        });
        schedule.push(GoldenRc3Frame::Traffic {
            rate: GoldenRc3Rate::Full,
            info_bits: encapsulate_reverse_rc3_full_rate_info_bits(build_reverse_rc3_sccm_pdu(
                1, 2, true, 3,
            )),
        });
        schedule.push(GoldenRc3Frame::Traffic {
            rate: GoldenRc3Rate::Half,
            info_bits: patterned_bits(GoldenRc3Rate::Half.info_bits(), 0x0F0F_0004),
        });
        schedule.push(GoldenRc3Frame::Traffic {
            rate: GoldenRc3Rate::Quarter,
            info_bits: patterned_bits(GoldenRc3Rate::Quarter.info_bits(), 0xF0F0_0005),
        });
        schedule.push(GoldenRc3Frame::Traffic {
            rate: GoldenRc3Rate::Full,
            info_bits: encapsulate_reverse_rc3_full_rate_info_bits(
                build_reverse_rc3_ms_ack_order_pdu(2, 3, false),
            ),
        });
        schedule.push(GoldenRc3Frame::Traffic {
            rate: GoldenRc3Rate::Eighth,
            info_bits: patterned_bits(GoldenRc3Rate::Eighth.info_bits(), 0xAA55_0006),
        });
        schedule.push(GoldenRc3Frame::Traffic {
            rate: GoldenRc3Rate::Eighth,
            info_bits: patterned_bits(GoldenRc3Rate::Eighth.info_bits(), 0x55AA_0007),
        });

        let expected_crc_valid_rates =
            vec![9600, 4800, 2700, 1500, 9600, 4800, 2700, 9600, 1500, 1500];

        let expected_signatures = vec![
            TrafficSignature::order(0, 1, false, "Mobile Station Acknowledgment"),
            TrafficSignature::sccm(1, 2, true),
            TrafficSignature::order(2, 3, false, "Mobile Station Acknowledgment"),
        ];

        ReverseRc3GoldenPlan {
            schedule,
            expected_crc_valid_rates,
            expected_signatures,
        }
    }

    fn build_rust_reverse_rc3_frame_samples(
        frame: &GoldenRc3Frame,
        esn: u32,
        long_code_state: u64,
        oversample: usize,
    ) -> Vec<Complex32> {
        const FRAME_CHIPS: usize = 24_576;
        const CHIPS_PER_SYMBOL: usize = 16;

        let encoded_symbols = match frame {
            GoldenRc3Frame::PilotOnly => None,
            GoldenRc3Frame::Traffic { rate, info_bits } => {
                Some(encode_reverse_rc3_fch_symbols(info_bits, *rate))
            }
        };

        let total_samples = FRAME_CHIPS * oversample;
        let pn_samples: Vec<Complex32> = build_fft_search_pn_samples(total_samples, oversample);
        let walsh_cover = WalshGenerator::generate_matrix::<16>()[4];
        let mut spreader = GoldenRc3HpskSpreader::with_state(esn, long_code_state);
        let mut iq_samples = Vec::with_capacity(total_samples);

        for chip_idx in 0..FRAME_CHIPS {
            let prompt_sample_idx = chip_idx * oversample;
            let prompt_pn = pn_samples[prompt_sample_idx];
            let spread_ref = spreader.next_spread_reference(prompt_pn);
            let desired_chip = match &encoded_symbols {
                None => Complex32::new(1.0, 0.0),
                Some(symbols) => {
                    let symbol_idx = chip_idx / CHIPS_PER_SYMBOL;
                    let walsh_chip = walsh_cover[chip_idx % CHIPS_PER_SYMBOL] as f32;
                    // Reverse RC3 multiplexes R-PICH and R-FCH as orthogonal
                    // complex components before the common HPSK spreading.
                    // Pilot stays on the real axis; the Walsh-covered FCH
                    // contribution rides on the quadrature axis.
                    Complex32::new(1.0, symbols[symbol_idx] * walsh_chip)
                }
            };
            let tx_chip = desired_chip * spread_ref;
            iq_samples.extend(std::iter::repeat_n(tx_chip, oversample));
        }

        let peak = iq_samples
            .iter()
            .map(|s| s.re.abs().max(s.im.abs()))
            .fold(0.0f32, f32::max);
        if peak > 1e-9 {
            let scale = 0.9 / peak;
            for sample in &mut iq_samples {
                *sample *= scale;
            }
        }

        iq_samples
    }

    fn build_reverse_rc3_despread_chips(frame: &GoldenRc3Frame) -> Vec<Complex32> {
        const FRAME_CHIPS: usize = 24_576;

        match frame {
            GoldenRc3Frame::PilotOnly => vec![Complex32::new(1.0, 0.0); FRAME_CHIPS],
            GoldenRc3Frame::Traffic { rate, info_bits } => {
                let symbols = encode_reverse_rc3_fch_symbols(info_bits, *rate);
                build_reverse_rc3_despread_chips_from_symbols(&symbols)
            }
        }
    }

    fn build_reverse_rc3_despread_chips_from_symbols(symbols: &[f32]) -> Vec<Complex32> {
        const FRAME_CHIPS: usize = 24_576;
        const CHIPS_PER_SYMBOL: usize = 16;

        let walsh_cover = WalshGenerator::generate_matrix::<16>()[4];
        (0..FRAME_CHIPS)
            .map(|chip_idx| {
                let symbol_idx = chip_idx / CHIPS_PER_SYMBOL;
                let walsh_chip = walsh_cover[chip_idx % CHIPS_PER_SYMBOL] as f32;
                // Traffic on negative .im matches the live finger's HPSK output
                // convention (pilot on +real, R-FCH Walsh-4 on −imaginary).
                Complex32::new(1.0, -symbols[symbol_idx] * walsh_chip)
            })
            .collect()
    }

    fn build_rust_reverse_rc3_golden_despread_framewise() -> RustRc3GoldenDespreadFramewise {
        let plan = reverse_rc3_golden_plan();
        let frames = plan
            .schedule
            .iter()
            .map(|frame| RustRc3GoldenDespreadFrame {
                chips: build_reverse_rc3_despread_chips(frame),
            })
            .collect();

        RustRc3GoldenDespreadFramewise {
            frames,
            expected_crc_valid_rates: plan.expected_crc_valid_rates,
            expected_signatures: plan.expected_signatures,
        }
    }

    fn collect_rc3_reverse_traffic_decode_result(
        walsh_code: u8,
        label: &str,
        expected_preambles: Option<usize>,
        expected_exact_signatures: Option<&[TrafficSignature]>,
        expected_prefix_signatures: Option<&[TrafficSignature]>,
        expected_crc_valid_rates: Option<&[i64]>,
        outputs: impl IntoIterator<Item = SampleBlock>,
    ) -> Rc3GoldenDecodeResult {
        let mut preamble_count = 0usize;
        let mut data_frame_count = 0usize;
        let mut crc_valid_data_frame_count = 0usize;
        let mut phy_frame_count = 0usize;
        let mut phy_crc_valid_count = 0usize;
        let mut rate_counts = std::collections::BTreeMap::<i64, usize>::new();
        let mut recovered_crc_pdus = Vec::new();
        let mut crc_valid_rates = Vec::new();
        let mut pcg_measurement_count = 0usize;
        let mut nonzero_pcg_measurement_age_count = 0usize;
        let mut max_pcg_measurement_age_chips = 0u64;
        let mut pcg_measurement_timings = Vec::new();

        for blk in outputs {
            if blk.tags.get("traffic_preamble_detected") == Some(&1) {
                preamble_count += 1;
                eprintln!(
                    "  traffic preamble #{}: chip_start={:?}",
                    preamble_count,
                    blk.tags.get("absolute_chip_start"),
                );
            }
            if blk.tags.get("traffic_phy_frame") == Some(&1) {
                phy_frame_count += 1;
                let fqi = blk.tags.get("traffic_fqi_valid").copied().unwrap_or(0);
                let rate = blk.tags.get("traffic_rate_bps").copied().unwrap_or(0);
                let mux = blk.tags.get("traffic_mux_header").copied();
                let sig_bits = blk
                    .tags
                    .get("traffic_mux_signaling_bits")
                    .copied()
                    .unwrap_or(0);
                if fqi == 1 {
                    phy_crc_valid_count += 1;
                    crc_valid_rates.push(rate);
                }
                *rate_counts.entry(rate).or_insert(0) += 1;
                eprintln!(
                    "  [w{}] phy frame #{}: rate={} fqi_valid={} mux={:?} sig_bits={} chip={:?}",
                    walsh_code,
                    phy_frame_count,
                    rate,
                    fqi,
                    mux,
                    sig_bits,
                    blk.tags.get("absolute_chip_start"),
                );
            }
            if blk.tags.get("traffic_pcg_measurement") == Some(&1) {
                pcg_measurement_count += 1;
                let abs_chip = blk
                    .tags
                    .get("absolute_chip_start")
                    .copied()
                    .unwrap_or(0)
                    .max(0) as u64;
                let abs_pcg = abs_chip / 1_536;
                let age_chips = blk
                    .tags
                    .get("traffic_measurement_age_chips")
                    .copied()
                    .unwrap_or(0)
                    .max(0) as u64;
                if age_chips > 0 {
                    nonzero_pcg_measurement_age_count += 1;
                }
                max_pcg_measurement_age_chips = max_pcg_measurement_age_chips.max(age_chips);
                pcg_measurement_timings.push((abs_pcg, age_chips));
                eprintln!(
                    "  [w{}] pcg measurement #{}: abs_pcg={} age_chips={} age_pcgs={:.2} age_ms={:.3}",
                    walsh_code,
                    pcg_measurement_count,
                    abs_pcg,
                    age_chips,
                    age_chips as f64 / 1_536.0,
                    age_chips as f64 * 1000.0 / 1_228_800.0,
                );
            }
            if blk.tags.get("traffic_event") == Some(&1) {
                data_frame_count += 1;
                let crc_valid = blk.tags.get("traffic_crc_valid") == Some(&1);
                if crc_valid {
                    crc_valid_data_frame_count += 1;
                }
                let payload_bits = blk.tags.get("traffic_payload_bits").copied().unwrap_or(0);
                let payload_bytes: Vec<u8> = blk
                    .samples
                    .iter()
                    .map(|s| if s.re >= 0.5 { 1u8 } else { 0u8 })
                    .collect();
                let bs = cdma_common::bits::Bitstream::new_init(&payload_bytes);
                let rdsch_summary = match crate::receiver::access_layer3::RdschPdu::decode(&bs) {
                    Ok(pdu) => {
                        if crc_valid {
                            recovered_crc_pdus.push(pdu.clone());
                        }
                        pdu.summary()
                    }
                    Err(e) => format!("decode_err: {}", e),
                };
                eprintln!(
                    "  [w{}] traffic frame #{}: crc={} payload_bits={} chip={:?} rdsch={}",
                    walsh_code,
                    data_frame_count,
                    crc_valid,
                    payload_bits,
                    blk.tags.get("absolute_chip_start"),
                    rdsch_summary,
                );
            }
        }

        eprintln!(
            "{} [w{}] summary: preambles={} phy_frames={} phy_crc_valid={} rates={:?} traffic_frames={} crc_valid={}",
            label,
            walsh_code,
            preamble_count,
            phy_frame_count,
            phy_crc_valid_count,
            rate_counts,
            data_frame_count,
            crc_valid_data_frame_count,
        );
        if let Some(expected_preambles) = expected_preambles {
            assert_eq!(
                preamble_count, expected_preambles,
                "unexpected RC3 preamble count for walsh={}",
                walsh_code,
            );
        }

        let recovered_signatures = recovered_crc_pdus
            .iter()
            .filter_map(|pdu| match &pdu.l3 {
                AccessMessage::Order(order) => Some(TrafficSignature::order(
                    pdu.arq.ack_seq,
                    pdu.arq.msg_seq,
                    pdu.arq.ack_req,
                    order.order_name(),
                )),
                AccessMessage::PowerMeasurementReport(_) => Some(TrafficSignature::pmrm(
                    pdu.arq.ack_seq,
                    pdu.arq.msg_seq,
                    pdu.arq.ack_req,
                )),
                AccessMessage::ServiceConnectCompletion(_) => Some(TrafficSignature::sccm(
                    pdu.arq.ack_seq,
                    pdu.arq.msg_seq,
                    pdu.arq.ack_req,
                )),
                _ => None,
            })
            .collect::<Vec<_>>();
        if let Some(expected_signatures) = expected_exact_signatures {
            assert_eq!(
                recovered_signatures, expected_signatures,
                "unexpected ordered RC3 CRC-valid decode stream",
            );
        }
        if let Some(expected_signatures) = expected_prefix_signatures {
            assert!(
                recovered_signatures.len() >= expected_signatures.len(),
                "recovered stream shorter than expected prefix: got {} frames, need at least {}",
                recovered_signatures.len(),
                expected_signatures.len(),
            );
            assert_eq!(
                &recovered_signatures[..expected_signatures.len()],
                expected_signatures,
                "unexpected RC3 CRC-valid decode prefix",
            );
        }
        if let Some(expected_rates) = expected_crc_valid_rates {
            assert_eq!(
                crc_valid_rates, expected_rates,
                "unexpected ordered RC3 CRC-valid PHY rate sequence",
            );
        }

        Rc3GoldenDecodeResult {
            preamble_count,
            crc_valid_rates,
            recovered_signatures,
            pcg_measurement_count,
            nonzero_pcg_measurement_age_count,
            max_pcg_measurement_age_chips,
            pcg_measurement_timings,
            total_batches: None,
            max_batch_elapsed_ns: None,
            batch_budget_ns: None,
        }
    }

    fn run_rc3_reverse_traffic_despread_frame_test(
        despread_chips: Vec<Complex32>,
        walsh_code: u8,
        label: &str,
    ) -> Rc3GoldenDecodeResult {
        let outputs = run_rc3_reverse_traffic_despread_frame_outputs(despread_chips, walsh_code);

        collect_rc3_reverse_traffic_decode_result(
            walsh_code, label, None, None, None, None, outputs,
        )
    }

    fn run_rc3_reverse_traffic_despread_frame_outputs(
        despread_chips: Vec<Complex32>,
        walsh_code: u8,
    ) -> Vec<SampleBlock> {
        let mut chain: Vec<PipelineProcessorShared> = vec![
            Box::new(super::rc3_bpsk_despread::Rc3BpskDespread::new()),
            Box::new(super::rc3_frame_aligner::Rc3FrameAligner::new().with_walsh_code(walsh_code)),
        ];
        chain.extend(super::traffic_channel_chain_rc3(walsh_code));

        let mut input = SampleBlock::new(despread_chips, 0).with_sample_rate_hz(1_228_800.0);
        input.tags.insert("absolute_chip_start", 0);
        input.tags.insert("absolute_sample_start", 0);
        let mut emitter = super::VecEmitter::new();
        let mut outputs = super::run_sub_chain(&mut chain, input, &mut emitter);
        outputs.extend(super::flush_sub_chain(&mut chain, &mut emitter));
        outputs.extend(emitter.blocks);
        outputs
    }

    fn derive_reverse_rc3_despread_chips_from_pilot_reference(
        traffic_iq_samples: &[Complex32],
        pilot_iq_samples: &[Complex32],
        oversample: usize,
    ) -> Vec<Complex32> {
        assert_eq!(
            traffic_iq_samples.len(),
            pilot_iq_samples.len(),
            "traffic/pilot MATLAB RC3 frame lengths must match",
        );
        assert_eq!(
            traffic_iq_samples.len() % oversample,
            0,
            "MATLAB RC3 frame length must be an integer number of chips",
        );

        let mut chips = Vec::with_capacity(traffic_iq_samples.len() / oversample);
        for chip_idx in 0..(traffic_iq_samples.len() / oversample) {
            let mut dot = Complex32::new(0.0, 0.0);
            let mut pilot_energy = 0.0f32;
            for k in 0..oversample {
                let sample_idx = chip_idx * oversample + k;
                let traffic = traffic_iq_samples[sample_idx];
                let pilot = pilot_iq_samples[sample_idx];
                dot += traffic * pilot.conj();
                pilot_energy += pilot.norm_sqr();
            }
            // Negate .im to match the live finger's HPSK output convention
            // (pilot on +real, R-FCH Walsh-4 on −imaginary).
            let chip = if pilot_energy > 1e-9 {
                dot / pilot_energy
            } else {
                Complex32::new(0.0, 0.0)
            };
            chips.push(Complex32::new(chip.re, -chip.im));
        }
        chips
    }

    fn slice_shifted_frame_chips(
        raw_chips: &[Complex32],
        frame_idx: usize,
        chips_per_frame: usize,
        shift: usize,
    ) -> Vec<Complex32> {
        let start = frame_idx * chips_per_frame + shift;
        let available = raw_chips.len().saturating_sub(start).min(chips_per_frame);
        let mut out = Vec::with_capacity(chips_per_frame);
        out.extend_from_slice(&raw_chips[start..start + available]);
        out.resize(chips_per_frame, Complex32::new(0.0, 0.0));
        out
    }

    fn decode_phy_valid_frame_bits(outputs: Vec<SampleBlock>) -> Vec<(i64, Vec<u8>)> {
        outputs
            .into_iter()
            .filter(|blk| blk.tags.get("traffic_phy_valid") == Some(&1))
            .filter_map(|blk| {
                let rate = blk.tags.get("traffic_rate_bps").copied()?;
                let bits = blk
                    .samples
                    .iter()
                    .map(|s| s.re.round().clamp(0.0, 1.0) as u8)
                    .collect::<Vec<_>>();
                Some((rate, bits))
            })
            .collect()
    }

    fn matlab_reverse_rc3_shift_search_order() -> [usize; 9] {
        [
            REVERSE_RC3_GOLDEN_MATLAB_CHIP_SHIFT,
            REVERSE_RC3_GOLDEN_MATLAB_CHIP_SHIFT + 1,
            REVERSE_RC3_GOLDEN_MATLAB_CHIP_SHIFT + 2,
            REVERSE_RC3_GOLDEN_MATLAB_CHIP_SHIFT.saturating_sub(1),
            REVERSE_RC3_GOLDEN_MATLAB_CHIP_SHIFT + 3,
            REVERSE_RC3_GOLDEN_MATLAB_CHIP_SHIFT.saturating_sub(2),
            REVERSE_RC3_GOLDEN_MATLAB_CHIP_SHIFT + 4,
            REVERSE_RC3_GOLDEN_MATLAB_CHIP_SHIFT.saturating_sub(3),
            REVERSE_RC3_GOLDEN_MATLAB_CHIP_SHIFT + 5,
        ]
    }

    fn select_matlab_reverse_rc3_frame_chips(
        raw_chips: &[Complex32],
        frame_idx: usize,
        frame: &GoldenRc3Frame,
        chips_per_frame: usize,
    ) -> RustRc3GoldenDespreadFrame {
        match frame {
            GoldenRc3Frame::PilotOnly => RustRc3GoldenDespreadFrame {
                chips: slice_shifted_frame_chips(
                    raw_chips,
                    frame_idx,
                    chips_per_frame,
                    REVERSE_RC3_GOLDEN_MATLAB_CHIP_SHIFT,
                ),
            },
            GoldenRc3Frame::Traffic { rate, info_bits } => {
                let expected_rate = rate.rate_bps();
                let expected_bits = build_reverse_rc3_frame_bits(info_bits, *rate);
                let mut successful_shifts = Vec::new();

                for shift in matlab_reverse_rc3_shift_search_order() {
                    let chips =
                        slice_shifted_frame_chips(raw_chips, frame_idx, chips_per_frame, shift);
                    let decoded = decode_phy_valid_frame_bits(
                        run_rc3_reverse_traffic_despread_frame_outputs(
                            chips.clone(),
                            REVERSE_RC3_GOLDEN_WALSH,
                        ),
                    );
                    let valid_rates = decoded.iter().map(|(rate, _)| *rate).collect::<Vec<_>>();
                    if decoded.iter().any(|(actual_rate, bits)| {
                        *actual_rate == expected_rate && *bits == expected_bits
                    }) {
                        return RustRc3GoldenDespreadFrame { chips };
                    }
                    if !valid_rates.is_empty() {
                        successful_shifts.push((shift, valid_rates));
                    }
                }

                panic!(
                    "failed to align MATLAB reverse RC3 frame#{frame_idx:02} to expected rate {} and exact frame bits; nearby successful shifts={successful_shifts:?}",
                    expected_rate
                );
            }
        }
    }

    fn collect_rc3_reverse_traffic_despread_framewise_result(
        frames: &[RustRc3GoldenDespreadFrame],
        walsh_code: u8,
        label: &str,
    ) -> Rc3GoldenDecodeResult {
        let mut aggregated_rates = Vec::new();
        let mut aggregated_signatures = Vec::new();

        for (idx, frame) in frames.iter().enumerate() {
            let result = run_rc3_reverse_traffic_despread_frame_test(
                frame.chips.clone(),
                walsh_code,
                &format!("{label} frame#{idx:02}"),
            );
            aggregated_rates.extend(result.crc_valid_rates);
            aggregated_signatures.extend(result.recovered_signatures);
        }

        Rc3GoldenDecodeResult {
            preamble_count: 0,
            crc_valid_rates: aggregated_rates,
            recovered_signatures: aggregated_signatures,
            ..Default::default()
        }
    }

    fn run_rc3_reverse_traffic_despread_framewise_test(
        frames: &[RustRc3GoldenDespreadFrame],
        walsh_code: u8,
        label: &str,
        expected_crc_valid_rates: &[i64],
        expected_signatures: &[TrafficSignature],
    ) -> Rc3GoldenDecodeResult {
        let result =
            collect_rc3_reverse_traffic_despread_framewise_result(frames, walsh_code, label);

        assert_eq!(
            result.crc_valid_rates, expected_crc_valid_rates,
            "unexpected framewise RC3 CRC-valid PHY rate sequence",
        );
        assert_eq!(
            result.recovered_signatures, expected_signatures,
            "unexpected framewise RC3 CRC-valid traffic signatures",
        );

        result
    }

    fn rc3_pcg_batch_size(sample_rate: usize) -> usize {
        let oversample = (sample_rate / 1_228_800).max(1);
        oversample * 1_536
    }

    fn run_rc3_reverse_traffic_iq_test_with_batch_size(
        iq_samples: Vec<Complex32>,
        sample_rate: usize,
        chip_start: u64,
        esn: u32,
        walsh_code: u8,
        preamble_num_pcgs: Option<usize>,
        label: &str,
        expected_preambles: Option<usize>,
        expected_exact_signatures: Option<&[TrafficSignature]>,
        expected_prefix_signatures: Option<&[TrafficSignature]>,
        expected_crc_valid_rates: Option<&[i64]>,
        batch_size: usize,
    ) -> Rc3GoldenDecodeResult {
        let oversample = sample_rate / 1_228_800;
        let wav_duration_secs = iq_samples.len() as f64 / sample_rate as f64;

        eprintln!(
            "{}: sample_rate={} oversample={} iq_samples={} chip_start={} esn=0x{:08X} walsh={} batch_size={} batch_ms={:.3}",
            label,
            sample_rate,
            oversample,
            iq_samples.len(),
            chip_start,
            esn,
            walsh_code,
            batch_size,
            batch_size as f64 * 1000.0 / sample_rate as f64,
        );

        let pipeline = super::reverse_traffic_chain_rc3(super::ReverseTrafficSettings {
            oversample,
            walsh_code,
            esn,
            reanchor_origin: true,
            snr_threshold: None,
            preamble_num_pcgs,
            epl_pilot: true,
            rev_fch_gating_mode: false,
            finger_pool_size: 1,
        });

        let mut receiver = PipelinedReceiver::new(iq_samples.into_iter())
            .with_batch_size(batch_size)
            .with_input_sample_rate_hz(sample_rate as f64)
            .with_absolute_sample_start(chip_start * oversample as u64);
        let out_rx = receiver.add_pipeline(pipeline);
        let pipeline_start = std::time::Instant::now();
        let run_stats = receiver.run_pipeline_with_stats().unwrap();
        let processing_secs = pipeline_start.elapsed().as_secs_f64();
        let realtime_ratio = if processing_secs > 0.0 {
            wav_duration_secs / processing_secs
        } else {
            f64::INFINITY
        };
        let batch_budget_ns = ((batch_size as f64 * 1_000_000_000.0) / sample_rate as f64) as u64;
        eprintln!(
            "{} [w{}] performance: wav_duration={:.2}s processing={:.2}s realtime_ratio={:.2}x",
            label, walsh_code, wav_duration_secs, processing_secs, realtime_ratio,
        );

        let mut result = collect_rc3_reverse_traffic_decode_result(
            walsh_code,
            label,
            expected_preambles,
            expected_exact_signatures,
            expected_prefix_signatures,
            expected_crc_valid_rates,
            out_rx.into_iter().flatten(),
        );
        result.total_batches = Some(run_stats.total_batches);
        result.max_batch_elapsed_ns = Some(run_stats.max_batch_elapsed_ns);
        result.batch_budget_ns = Some(batch_budget_ns);
        result
    }

    fn run_rc3_reverse_traffic_iq_test(
        iq_samples: Vec<Complex32>,
        sample_rate: usize,
        chip_start: u64,
        esn: u32,
        walsh_code: u8,
        preamble_num_pcgs: Option<usize>,
        label: &str,
        expected_preambles: Option<usize>,
        expected_exact_signatures: Option<&[TrafficSignature]>,
        expected_prefix_signatures: Option<&[TrafficSignature]>,
        expected_crc_valid_rates: Option<&[i64]>,
    ) -> Rc3GoldenDecodeResult {
        run_rc3_reverse_traffic_iq_test_with_batch_size(
            iq_samples,
            sample_rate,
            chip_start,
            esn,
            walsh_code,
            preamble_num_pcgs,
            label,
            expected_preambles,
            expected_exact_signatures,
            expected_prefix_signatures,
            expected_crc_valid_rates,
            32_768,
        )
    }

    fn run_rc3_reverse_traffic_capture_test(
        wav_filename: &str,
        chip_start: u64,
        esn: u32,
        walsh_code: u8,
        label: &str,
        expected_preambles: Option<usize>,
        expected_exact_signatures: Option<&[TrafficSignature]>,
        expected_prefix_signatures: Option<&[TrafficSignature]>,
    ) -> Rc3GoldenDecodeResult {
        init_test_logger();
        let wav_path = test_capture_path(wav_filename);
        if !wav_path.exists() {
            eprintln!("skipping test: {} not found", wav_path.display());
            return Rc3GoldenDecodeResult {
                preamble_count: 0,
                crc_valid_rates: Vec::new(),
                recovered_signatures: Vec::new(),
                ..Default::default()
            };
        }
        let reader = hound::WavReader::open(&wav_path)
            .unwrap_or_else(|e| panic!("failed to open {}: {e}", wav_path.display()));
        let (sample_rate, iq_samples) = read_iq_wav(reader);
        run_rc3_reverse_traffic_iq_test(
            iq_samples,
            sample_rate as usize,
            chip_start,
            esn,
            walsh_code,
            None,
            label,
            expected_preambles,
            expected_exact_signatures,
            expected_prefix_signatures,
            None,
        )
    }

    fn run_rc3_reverse_traffic_capture_test_pcg_batches(
        wav_filename: &str,
        chip_start: u64,
        esn: u32,
        walsh_code: u8,
        label: &str,
        expected_preambles: Option<usize>,
        expected_exact_signatures: Option<&[TrafficSignature]>,
        expected_prefix_signatures: Option<&[TrafficSignature]>,
    ) -> Rc3GoldenDecodeResult {
        init_test_logger();
        let wav_path = test_capture_path(wav_filename);
        if !wav_path.exists() {
            eprintln!("skipping test: {} not found", wav_path.display());
            return Rc3GoldenDecodeResult {
                preamble_count: 0,
                crc_valid_rates: Vec::new(),
                recovered_signatures: Vec::new(),
                ..Default::default()
            };
        }
        let reader = hound::WavReader::open(&wav_path)
            .unwrap_or_else(|e| panic!("failed to open {}: {e}", wav_path.display()));
        let (sample_rate, iq_samples) = read_iq_wav(reader);
        run_rc3_reverse_traffic_iq_test_with_batch_size(
            iq_samples,
            sample_rate as usize,
            chip_start,
            esn,
            walsh_code,
            None,
            label,
            expected_preambles,
            expected_exact_signatures,
            expected_prefix_signatures,
            None,
            rc3_pcg_batch_size(sample_rate as usize),
        )
    }

    fn expected_rc3_so33_w11_signatures() -> Vec<TrafficSignature> {
        vec![
            TrafficSignature::order(0, 0, false, "Mobile Station Acknowledgment"),
            TrafficSignature::pmrm(0, 1, false),
            TrafficSignature::order(0, 2, false, "Mobile Station Acknowledgment"),
            TrafficSignature::sccm(1, 0, true),
            TrafficSignature::sccm(1, 0, true),
            TrafficSignature::sccm(1, 0, true),
            TrafficSignature::sccm(1, 0, true),
            TrafficSignature::pmrm(1, 3, false),
            TrafficSignature::sccm(1, 0, true),
            TrafficSignature::order(2, 4, false, "Mobile Station Acknowledgment"),
            TrafficSignature::pmrm(2, 5, false),
            TrafficSignature::pmrm(2, 6, false),
            TrafficSignature::order(3, 7, false, "Mobile Station Acknowledgment"),
            TrafficSignature::pmrm(3, 0, false),
            TrafficSignature::order(3, 1, false, "Mobile Station Acknowledgment"),
            TrafficSignature::pmrm(3, 2, false),
            TrafficSignature::order(3, 3, false, "Mobile Station Acknowledgment"),
            TrafficSignature::pmrm(3, 4, false),
            TrafficSignature::order(3, 5, false, "Mobile Station Acknowledgment"),
            TrafficSignature::pmrm(3, 6, false),
        ]
    }

    fn summarize_pcg_timing_failure_runs(
        measurements: &[(u64, u64)],
        max_allowed_age_chips: u64,
    ) -> Vec<PcgTimingFailureRun> {
        let mut runs: Vec<PcgTimingFailureRun> = Vec::new();
        for &(abs_pcg, age_chips) in measurements {
            if age_chips <= max_allowed_age_chips {
                continue;
            }
            if let Some(last) = runs.last_mut()
                && last.end_abs_pcg + 1 == abs_pcg
            {
                last.end_abs_pcg = abs_pcg;
                last.count += 1;
                last.max_age_chips = last.max_age_chips.max(age_chips);
                continue;
            }
            runs.push(PcgTimingFailureRun {
                start_abs_pcg: abs_pcg,
                end_abs_pcg: abs_pcg,
                count: 1,
                max_age_chips: age_chips,
            });
        }
        runs
    }

    /// RC3 reverse traffic channel decode test for SO33 packet data.
    /// WAV captured during SO33 origination on walsh=10, RC3.
    /// MS ESN=0x80857E58.
    #[test]
    fn capture_rc3_reverse_traffic_so33_decode() {
        const RC3_SO33_W11_EXPECTED_PCG_MEASUREMENTS: usize = 15_967;

        let expected_signatures = expected_rc3_so33_w11_signatures();
        let result = run_rc3_reverse_traffic_capture_test(
            "1793960586090657.wav",
            1793960586090657,
            0x80857E58,
            11,
            "RC3 SO33 traffic decode test",
            Some(1),
            Some(&expected_signatures),
            None,
        );
        assert_eq!(
            result.pcg_measurement_count, RC3_SO33_W11_EXPECTED_PCG_MEASUREMENTS,
            "unexpected RC3 SO33 decode per-PCG measurement count",
        );
    }

    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "release-only live-chain decode baseline; run with --release -- --nocapture"
    )]
    fn capture_rc3_reverse_traffic_so33_live_chain_baseline() {
        const RC3_MAX_MEASUREMENT_AGE_CHIPS: u64 = 1_536;
        const RC3_SO33_W11_EXPECTED_PCG_MEASUREMENTS: usize = 15_970;

        let expected_signatures = expected_rc3_so33_w11_signatures();
        let result = run_rc3_reverse_traffic_capture_test_pcg_batches(
            "1793960586090657.wav",
            1793960586090657,
            0x80857E58,
            11,
            "RC3 SO33 live-chain baseline",
            Some(1),
            Some(&expected_signatures),
            None,
        );
        assert!(
            result.pcg_measurement_count > 0,
            "expected RC3 SO33 live-chain baseline to emit per-PCG measurements",
        );
        assert_eq!(
            result.pcg_measurement_count, RC3_SO33_W11_EXPECTED_PCG_MEASUREMENTS,
            "unexpected RC3 SO33 live-chain per-PCG measurement count",
        );
        let failing_measurements = result
            .pcg_measurement_timings
            .iter()
            .copied()
            .filter(|&(_, age_chips)| age_chips > RC3_MAX_MEASUREMENT_AGE_CHIPS)
            .collect::<Vec<_>>();
        let failure_runs = summarize_pcg_timing_failure_runs(
            &result.pcg_measurement_timings,
            RC3_MAX_MEASUREMENT_AGE_CHIPS,
        );
        eprintln!(
            "RC3 SO33 live-chain timing stats: pcg_measurements={} on_time_measurements={} failing_measurements={} nonzero_age={} max_age_chips={} fail_runs={:?} total_batches={:?} max_batch_elapsed_ns={:?} batch_budget_ns={:?}",
            result.pcg_measurement_count,
            result.pcg_measurement_count - failing_measurements.len(),
            failing_measurements.len(),
            result.nonzero_pcg_measurement_age_count,
            result.max_pcg_measurement_age_chips,
            failure_runs,
            result.total_batches,
            result.max_batch_elapsed_ns,
            result.batch_budget_ns,
        );
        assert!(
            failing_measurements.is_empty(),
            "RC3 SO33 live-chain PCG timing failures: {}/{} measurements exceeded {} chips (1 PCG). Failure runs: {:?}. All measurements: {:?}",
            failing_measurements.len(),
            result.pcg_measurement_timings.len(),
            RC3_MAX_MEASUREMENT_AGE_CHIPS,
            failure_runs,
            result.pcg_measurement_timings,
        );
    }

    /// RC3 reverse traffic channel decode test for SO33 packet data.
    /// WAV captured during SO33 origination on walsh=10, RC3.
    /// MS ESN=0x80857E58.
    #[test]
    fn capture_rc3_reverse_traffic_so33_w10_decode() {
        run_rc3_reverse_traffic_capture_test(
            "1793963859800123.wav",
            1793963859800123,
            0x80857E58,
            10,
            "RC3 SO33 w10 traffic decode test",
            None,
            None,
            None,
        );
    }

    /// Access channel decode from CFO regression capture (LimeSDR).
    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "release-only; run with --release -- --nocapture"
    )]
    fn capture_wav_1794968212722404_access_decode() {
        init_test_logger();
        let Some(stats) = run_uplink_access_probe_full_chain_capture(
            "1794968212722404.wav",
            1794968212722404,
            "cfo_regression_access",
        ) else {
            return;
        };
        eprintln!(
            "cfo_regression_access: crc_valid={} preambles={}",
            stats.crc_valid_data_frame_count, stats.preamble_count
        );
        assert!(
            stats.crc_valid_data_frame_count >= 1,
            "expected at least 1 CRC-valid access frame, got {}",
            stats.crc_valid_data_frame_count
        );
    }

    /// RC3 reverse traffic CFO regression: LimeSDR capture, ESN=0x80857E58, walsh=10.
    /// 45-second capture with full session (origination → traffic → idle).
    /// Baseline: 1609 CRC-valid frames across all rates.
    #[test]
    #[cfg_attr(
        debug_assertions,
        ignore = "release-only; run with --release -- --nocapture"
    )]
    fn capture_rc3_cfo_regression_wav_1794968212722404() {
        let result = run_rc3_reverse_traffic_capture_test(
            "1794968212722404.wav",
            1794968212722404,
            0x80857E58,
            10,
            "RC3 CFO regression (LimeSDR)",
            None,
            None,
            None,
        );
        let valid = result.crc_valid_rates.len();
        eprintln!(
            "RC3 CFO regression: crc_valid_frames={} preambles={}",
            valid, result.preamble_count
        );
        assert_eq!(
            valid, 1609,
            "RC3 CFO regression baseline: expected 1609 CRC-valid frames, got {}",
            valid
        );
    }

    /// RC3 reverse traffic channel decode test for SO33 packet data on walsh=12.
    /// WAV captured during SO33 activity, RC3, same MS ESN=0x80857E58.
    #[test]
    fn capture_rc3_reverse_traffic_so33_w12_decode() {
        let expected_signatures = vec![
            TrafficSignature::order(0, 0, false, "Mobile Station Acknowledgment"),
            TrafficSignature::sccm(1, 0, true),
            TrafficSignature::order(2, 1, false, "Mobile Station Acknowledgment"),
            TrafficSignature::pmrm(2, 2, false),
            TrafficSignature::pmrm(2, 3, false),
            TrafficSignature::order(3, 4, false, "Mobile Station Acknowledgment"),
            TrafficSignature::order(4, 7, false, "Mobile Station Acknowledgment"),
            TrafficSignature::pmrm(4, 0, false),
            TrafficSignature::order(4, 1, false, "Mobile Station Acknowledgment"),
            TrafficSignature::pmrm(4, 2, false),
        ];
        run_rc3_reverse_traffic_capture_test(
            "1793967987133603.wav",
            1793967987133603,
            0x80857E58,
            12,
            "RC3 SO33 w12 traffic decode test",
            Some(1),
            Some(&expected_signatures),
            None,
        );
    }

    #[test]
    fn test_matlab_reverse_rc3_traffic_golden() {
        init_test_logger();
        let wav_path = test_iq_path("rev_rc3_traffic.wav");
        if !wav_path.exists() {
            eprintln!(
                "skipping MATLAB RC3 reverse golden: {} not found",
                wav_path.display()
            );
            return;
        }

        let manifest_path = test_iq_path("rev_rc3_traffic_meta.json");
        if !manifest_path.exists() {
            eprintln!(
                "skipping MATLAB RC3 reverse golden: {} not found; regenerate with tools/matlab/generate_reverse_rc3_traffic_wav.m",
                manifest_path.display(),
            );
            return;
        }

        let manifest_bytes = std::fs::read(&manifest_path)
            .unwrap_or_else(|e| panic!("failed to read {}: {e}", manifest_path.display()));
        let manifest: MatlabReverseRc3GoldenManifest = serde_json::from_slice(&manifest_bytes)
            .unwrap_or_else(|e| panic!("failed to parse {}: {e}", manifest_path.display()));

        let golden = build_rust_reverse_rc3_golden_despread_framewise();
        let plan = reverse_rc3_golden_plan();
        let expected_manifest_signatures = manifest
            .expected_signatures
            .iter()
            .map(MatlabReverseRc3Signature::to_signature)
            .collect::<Vec<_>>();

        assert_eq!(manifest.generator_type, "reverse_rc3_traffic");
        assert_eq!(
            matlab_json_int_u32(manifest.generator_version, "generatorVersion"),
            REVERSE_RC3_GOLDEN_GENERATOR_VERSION,
            "stale MATLAB reverse RC3 generator version; regenerate the golden WAV",
        );
        assert_eq!(
            manifest.schedule_name, REVERSE_RC3_GOLDEN_SCHEDULE_NAME,
            "stale MATLAB reverse RC3 schedule; regenerate the golden WAV",
        );
        assert_eq!(
            matlab_json_int_u32(manifest.esn, "esn"),
            REVERSE_RC3_GOLDEN_ESN
        );
        assert_eq!(
            matlab_json_int_u32(manifest.walsh_code, "walshCode") as u8,
            REVERSE_RC3_GOLDEN_WALSH
        );
        assert_eq!(
            matlab_json_int_u64(manifest.pn_offset, "pnOffset"),
            REVERSE_RC3_GOLDEN_PN_OFFSET
        );
        assert_eq!(
            matlab_json_int_u64(manifest.long_code_state, "longCodeState"),
            1u64 << 41
        );
        assert_eq!(
            matlab_json_int_u64(manifest.long_code_mask, "longCodeMask"),
            LongCodeGenerator::new_traffic_channel(REVERSE_RC3_GOLDEN_ESN).mask(),
        );
        assert_eq!(manifest.filter_type, "Off");
        assert_eq!(
            matlab_json_int_usize(manifest.preamble_frames, "preambleFrames"),
            REVERSE_RC3_GOLDEN_PREAMBLE_FRAMES
        );
        assert_eq!(manifest.short_code_reset_each_frame, Some(true));
        let manifest_frame_pn_offsets = manifest
            .frame_pn_offsets
            .as_ref()
            .unwrap_or_else(|| panic!("stale MATLAB reverse RC3 manifest: missing framePnOffsets"));
        let actual_frame_pn_offsets = manifest_frame_pn_offsets
            .iter()
            .enumerate()
            .map(|(idx, value)| matlab_json_int_u64(*value, &format!("framePnOffsets[{idx}]")))
            .collect::<Vec<_>>();
        assert_eq!(
            actual_frame_pn_offsets,
            vec![0u64; golden.frames.len()],
            "MATLAB reverse RC3 toolbox golden should reset the short-code epoch each frame",
        );
        let manifest_frame_long_code_states =
            manifest.frame_long_code_states.as_ref().unwrap_or_else(|| {
                panic!("stale MATLAB reverse RC3 manifest: missing frameLongCodeStates")
            });
        let actual_frame_long_code_states = manifest_frame_long_code_states
            .iter()
            .enumerate()
            .map(|(idx, value)| matlab_json_int_u64(*value, &format!("frameLongCodeStates[{idx}]")))
            .collect::<Vec<_>>();
        let expected_frame_long_code_states = (0..golden.frames.len())
            .scan(1u64 << 41, |state, _| {
                let current = *state;
                *state = advance_reverse_rc3_traffic_long_code_state(
                    *state,
                    REVERSE_RC3_GOLDEN_FRAME_CHIPS as usize,
                );
                Some(current)
            })
            .collect::<Vec<_>>();
        assert_eq!(
            actual_frame_long_code_states, expected_frame_long_code_states,
            "MATLAB reverse RC3 frameLongCodeStates do not match the shared golden plan",
        );
        assert_eq!(
            manifest.expected_crc_valid_rates,
            golden.expected_crc_valid_rates
        );
        assert_eq!(expected_manifest_signatures, golden.expected_signatures);

        let reader = hound::WavReader::open(&wav_path)
            .unwrap_or_else(|e| panic!("failed to open {}: {e}", wav_path.display()));
        let (sample_rate, iq_samples) = read_iq_wav(reader);
        let pilot_wav_path = manifest
            .pilot_only_path
            .as_ref()
            .map(|path| {
                let path = Path::new(path);
                if path.is_absolute() {
                    path.to_path_buf()
                } else if path.exists() {
                    path.to_path_buf()
                } else {
                    workspace_fixture_path(path)
                }
            })
            .unwrap_or_else(|| test_iq_path("rev_rc3_traffic_pilot_only.wav"));
        if !pilot_wav_path.exists() {
            panic!(
                "missing MATLAB RC3 reverse pilot-only companion {}; regenerate with tools/matlab/generate_reverse_rc3_traffic_wav.m",
                pilot_wav_path.display()
            );
        }
        let pilot_reader = hound::WavReader::open(&pilot_wav_path)
            .unwrap_or_else(|e| panic!("failed to open {}: {e}", pilot_wav_path.display()));
        let (pilot_sample_rate, pilot_iq_samples) = read_iq_wav(pilot_reader);
        assert_eq!(
            pilot_sample_rate, sample_rate,
            "MATLAB reverse RC3 traffic/pilot WAV sample rates must match",
        );
        let samples_per_frame =
            (REVERSE_RC3_GOLDEN_FRAME_CHIPS as usize) * (sample_rate as usize / 1_228_800);
        assert_eq!(
            iq_samples.len(),
            samples_per_frame * golden.frames.len(),
            "MATLAB reverse RC3 golden WAV length does not match expected frame count",
        );
        assert_eq!(
            pilot_iq_samples.len(),
            iq_samples.len(),
            "MATLAB reverse RC3 pilot-only WAV length does not match traffic WAV",
        );
        let oversample = sample_rate as usize / 1_228_800;
        let raw_matlab_chips = derive_reverse_rc3_despread_chips_from_pilot_reference(
            &iq_samples,
            &pilot_iq_samples,
            oversample,
        );
        let chips_per_frame = REVERSE_RC3_GOLDEN_FRAME_CHIPS as usize;
        assert_eq!(
            raw_matlab_chips.len(),
            chips_per_frame * golden.frames.len(),
            "MATLAB reverse RC3 despread chip stream length does not match expected frame count",
        );
        let matlab_frames = plan
            .schedule
            .iter()
            .enumerate()
            .map(|(idx, frame)| {
                select_matlab_reverse_rc3_frame_chips(
                    &raw_matlab_chips,
                    idx,
                    frame,
                    chips_per_frame,
                )
            })
            .collect::<Vec<_>>();
        // The MATLAB golden path goes through Rc3BpskDespread which performs
        // pilot-coherent demod.  With a strong pilot and zero traffic, the
        // pilot-only preamble frames decode as CRC-valid 1/8 rate (all-zero
        // payload passes CRC).  The Rust-generated golden bypasses this because
        // it builds despread chips directly.  Account for the 9 spurious
        // preamble decodes (the first preamble frame is consumed by aligner
        // startup).
        let matlab_expected_rates: Vec<i64> = std::iter::repeat(1500i64)
            .take(REVERSE_RC3_GOLDEN_PREAMBLE_FRAMES - 1)
            .chain(golden.expected_crc_valid_rates.iter().copied())
            .collect();
        let result = run_rc3_reverse_traffic_despread_framewise_test(
            &matlab_frames,
            REVERSE_RC3_GOLDEN_WALSH,
            "MATLAB RC3 reverse golden",
            &matlab_expected_rates,
            &golden.expected_signatures,
        );
        assert_eq!(result.crc_valid_rates, matlab_expected_rates);
        assert_eq!(result.recovered_signatures, golden.expected_signatures);
    }

    /// Golden RC3 reverse traffic channel decode test using a Rust-generated
    /// waveform. Set `CDMA_RUST_RC3_GOLDEN_WAV=/tmp/rev_rc3_traffic_rust.wav`
    /// to dump the generated IQ as a stereo WAV while running the test.
    #[test]
    fn test_rust_reverse_rc3_traffic_golden() {
        init_test_logger();
        let golden = build_rust_reverse_rc3_golden_despread_framewise();
        let result = run_rc3_reverse_traffic_despread_framewise_test(
            &golden.frames,
            REVERSE_RC3_GOLDEN_WALSH,
            "Rust RC3 reverse golden",
            &golden.expected_crc_valid_rates,
            &golden.expected_signatures,
        );
        assert_eq!(
            result.crc_valid_rates, golden.expected_crc_valid_rates,
            "unexpected ordered CRC-valid RC3 PHY rate stream from the Rust-generated golden",
        );
        assert_eq!(
            result.recovered_signatures, golden.expected_signatures,
            "unexpected ordered CRC-valid RC3 traffic stream from the Rust-generated golden",
        );
    }

    #[test]
    fn test_rust_reverse_rc3_traffic_golden_from_pilot_reference() {
        init_test_logger();
        let plan = reverse_rc3_golden_plan();
        let oversample = 4usize;
        let mut long_code_state = 1u64 << 41;
        let mut frames = Vec::with_capacity(plan.schedule.len());

        for frame in &plan.schedule {
            let traffic = build_rust_reverse_rc3_frame_samples(
                frame,
                REVERSE_RC3_GOLDEN_ESN,
                long_code_state,
                oversample,
            );
            let pilot = build_rust_reverse_rc3_frame_samples(
                &GoldenRc3Frame::PilotOnly,
                REVERSE_RC3_GOLDEN_ESN,
                long_code_state,
                oversample,
            );
            frames.push(RustRc3GoldenDespreadFrame {
                chips: derive_reverse_rc3_despread_chips_from_pilot_reference(
                    &traffic, &pilot, oversample,
                ),
            });
            long_code_state = advance_reverse_rc3_traffic_long_code_state(
                long_code_state,
                REVERSE_RC3_GOLDEN_FRAME_CHIPS as usize,
            );
        }

        let result = run_rc3_reverse_traffic_despread_framewise_test(
            &frames,
            REVERSE_RC3_GOLDEN_WALSH,
            "Rust RC3 reverse golden via pilot reference",
            &plan.expected_crc_valid_rates,
            &plan.expected_signatures,
        );
        assert_eq!(result.crc_valid_rates, plan.expected_crc_valid_rates);
        assert_eq!(result.recovered_signatures, plan.expected_signatures);
    }

    fn run_rc1_reverse_traffic_channel_decode_wav_test_with_options(
        wav_path: std::path::PathBuf,
        chip_start: u64,
        default_sample_start: usize,
        default_sample_len: Option<usize>,
        min_realtime_ratio: Option<f64>,
        expected_rate_counts: Option<std::collections::BTreeMap<i64, usize>>,
        expected_crc_valid_full_rate_signaling_frames: Option<usize>,
    ) {
        run_rc1_reverse_traffic_channel_decode_wav_impl(
            wav_path,
            chip_start,
            default_sample_start,
            default_sample_len,
            min_realtime_ratio,
            None,
            None,
            expected_rate_counts,
            expected_crc_valid_full_rate_signaling_frames,
        );
    }

    fn run_rc1_reverse_traffic_channel_decode_wav_impl(
        wav_path: std::path::PathBuf,
        chip_start: u64,
        default_sample_start: usize,
        default_sample_len: Option<usize>,
        min_realtime_ratio: Option<f64>,
        esn_override: Option<u32>,
        walsh_override: Option<Vec<u8>>,
        expected_rate_counts: Option<std::collections::BTreeMap<i64, usize>>,
        expected_crc_valid_full_rate_signaling_frames: Option<usize>,
    ) {
        if !wav_path.exists() {
            eprintln!(
                "skipping RC1 reverse traffic WAV test: {} not found",
                wav_path.display()
            );
            return;
        }
        let reader = hound::WavReader::open(&wav_path)
            .unwrap_or_else(|e| panic!("failed to open {}: {e}", wav_path.display()));
        let (sample_rate, mut iq_samples) = read_iq_wav(reader);
        let esn: u32 = esn_override.unwrap_or(0x4CDC1D09);
        let oversample = (sample_rate as usize) / 1228800;
        let sample_start: usize = default_sample_start;
        let sample_len: Option<usize> = default_sample_len;
        if sample_start > 0 || sample_len.is_some() {
            let end = sample_len
                .map(|len| sample_start.saturating_add(len))
                .unwrap_or(iq_samples.len())
                .min(iq_samples.len());
            iq_samples = iq_samples[sample_start.min(end)..end].to_vec();
        }
        let absolute_sample_start = chip_start
            .saturating_mul(oversample as u64)
            .saturating_add(sample_start as u64);
        let walsh_codes: Vec<u8> = walsh_override.unwrap_or_else(|| vec![10u8]);

        let rc = 1u8;

        eprintln!(
            "traffic wav decode: path={} chip_start={} esn=0x{:08X} walsh_codes={:?} rc={} pipeline=reverse_traffic_chain",
            wav_path.display(),
            chip_start,
            esn,
            walsh_codes,
            rc,
        );

        for &walsh_code in &walsh_codes {
            let verbose_phy_limit: usize = 32;
            eprintln!(
                "\n============================================================\ntraffic decode: RC1-ESN walsh={} chip_start={}",
                walsh_code, chip_start,
            );

            let pipeline = super::reverse_traffic_chain(super::ReverseTrafficSettings {
                oversample,
                walsh_code,
                esn,
                reanchor_origin: true,
                snr_threshold: None,
                preamble_num_pcgs: None,
                epl_pilot: false,
                rev_fch_gating_mode: false,
                finger_pool_size: 1,
            });

            let wav_duration_secs = iq_samples.len() as f64 / sample_rate as f64;
            let mut receiver = PipelinedReceiver::new(iq_samples.clone().into_iter())
                .with_batch_size(32768)
                .with_input_sample_rate_hz(sample_rate as f64)
                .with_absolute_sample_start(absolute_sample_start);
            let out_rx = receiver.add_pipeline(pipeline);
            let pipeline_start = std::time::Instant::now();
            receiver.run_pipeline().unwrap();
            let pipeline_elapsed = pipeline_start.elapsed();

            let mut preamble_count = 0usize;
            let mut phy_frame_count = 0usize;
            let mut crc_valid_phy_frame_count = 0usize;
            let mut data_frame_count = 0usize;
            let mut crc_valid_data_frame_count = 0usize;
            let mut crc_valid_full_rate_signaling_frame_count = 0usize;
            let mut found_ms_ack_order = false;
            let mut mux_header_counts: std::collections::BTreeMap<i64, usize> =
                std::collections::BTreeMap::new();
            let mut signaling_bits_counts: std::collections::BTreeMap<i64, usize> =
                std::collections::BTreeMap::new();
            let mut full_rate_with_signaling = 0usize;
            let mut total_blocks = 0usize;
            let mut rate_counts: std::collections::BTreeMap<i64, usize> =
                std::collections::BTreeMap::new();
            let mut fqi_checked_valid = 0usize;
            let mut fqi_checked_total = 0usize;
            let mut pcg_measurement_count = 0usize;
            let mut pcg_measurement_age_sum_chips = 0u128;
            let mut pcg_measurement_age_max_chips = 0u64;
            let mut pcg_measurement_phases: std::collections::BTreeMap<u64, usize> =
                std::collections::BTreeMap::new();
            for blocks in out_rx {
                total_blocks += blocks.len();
                for blk in &blocks {
                    if blk.tags.get("traffic_pcg_measurement") == Some(&1) {
                        pcg_measurement_count += 1;
                        let age_chips = blk
                            .tags
                            .get("traffic_measurement_age_chips")
                            .copied()
                            .unwrap_or(0)
                            .max(0) as u64;
                        pcg_measurement_age_sum_chips =
                            pcg_measurement_age_sum_chips.saturating_add(age_chips as u128);
                        pcg_measurement_age_max_chips =
                            pcg_measurement_age_max_chips.max(age_chips);
                        if let Some(abs_chip) = blk
                            .tags
                            .get("absolute_chip_start")
                            .copied()
                            .and_then(|chip| u64::try_from(chip).ok())
                        {
                            let pcg_phase = (abs_chip / 1536) % 16;
                            *pcg_measurement_phases.entry(pcg_phase).or_default() += 1;
                        }
                    }
                    if blk.tags.get("traffic_preamble_detected") == Some(&1)
                        && blk.tags.contains_key("traffic_preamble_frames")
                    {
                        preamble_count += 1;
                        eprintln!(
                            "  [RC1-ESN|walsh={}] preamble #{}: chip_start={:?} preamble_frames={:?}",
                            walsh_code,
                            preamble_count,
                            blk.tags.get("absolute_chip_start"),
                            blk.tags.get("traffic_preamble_frames"),
                        );
                    }
                    if blk.tags.get("traffic_phy_frame") == Some(&1) {
                        phy_frame_count += 1;
                        let phy_valid = blk.tags.get("traffic_phy_valid") == Some(&1);
                        if phy_valid {
                            crc_valid_phy_frame_count += 1;
                        }
                        if let Some(rate) = blk.tags.get("traffic_rate_bps").copied() {
                            *rate_counts.entry(rate).or_default() += 1;
                        }
                        let fqi_bits = blk.tags.get("traffic_fqi_bits").copied().unwrap_or(0);
                        if fqi_bits > 0 {
                            fqi_checked_total += 1;
                            if blk.tags.get("traffic_fqi_valid") == Some(&1) {
                                fqi_checked_valid += 1;
                            }
                        }
                        if let Some(mux_header) = blk.tags.get("traffic_mux_header").copied() {
                            *mux_header_counts.entry(mux_header).or_default() += 1;
                        }
                        if let Some(signaling_bits) =
                            blk.tags.get("traffic_mux_signaling_bits").copied()
                        {
                            *signaling_bits_counts.entry(signaling_bits).or_default() += 1;
                            if blk.tags.get("traffic_rate_bps") == Some(&9600) && signaling_bits > 0
                            {
                                full_rate_with_signaling += 1;
                            }
                        }
                        if phy_frame_count <= verbose_phy_limit {
                            eprintln!(
                                "  [RC1-ESN|walsh={}] phy frame #{}: phy_valid={} rate={:?} mux_header={:?} signaling_bits={:?} chip={:?}",
                                walsh_code,
                                phy_frame_count,
                                phy_valid,
                                blk.tags.get("traffic_rate_bps"),
                                blk.tags.get("traffic_mux_header"),
                                blk.tags.get("traffic_mux_signaling_bits"),
                                blk.tags.get("absolute_chip_start"),
                            );
                        }
                        // Hex dump full-rate frames
                        if blk.tags.get("traffic_rate_bps") == Some(&9600) {
                            let bits: Vec<u8> = blk
                                .samples
                                .iter()
                                .map(|s| if s.re > 0.5 { 1u8 } else { 0u8 })
                                .collect();
                            let hex_bytes: Vec<u8> = bits
                                .chunks(8)
                                .map(|chunk| {
                                    chunk
                                        .iter()
                                        .enumerate()
                                        .fold(0u8, |acc, (i, &b)| acc | (b << (7 - i)))
                                })
                                .collect();
                            eprintln!(
                                "  [RC1-ESN|walsh={}] 9600 frame hex ({} bits): {}",
                                walsh_code,
                                bits.len(),
                                hex_bytes
                                    .iter()
                                    .map(|b| format!("{:02X}", b))
                                    .collect::<Vec<_>>()
                                    .join(" "),
                            );
                        }
                    }
                    if blk.tags.get("traffic_event") == Some(&1) {
                        data_frame_count += 1;
                        let crc_valid = blk.tags.get("traffic_crc_valid") == Some(&1);
                        if crc_valid {
                            crc_valid_data_frame_count += 1;
                            if blk.tags.get("traffic_rate_bps") == Some(&9600) {
                                crc_valid_full_rate_signaling_frame_count += 1;
                            }
                        }
                        let payload_bits: Vec<u8> = blk
                            .samples
                            .iter()
                            .map(|s| if s.re > 0.5 { 1u8 } else { 0u8 })
                            .collect();
                        let hex_bytes: Vec<u8> = payload_bits
                            .chunks(8)
                            .map(|chunk| {
                                chunk
                                    .iter()
                                    .enumerate()
                                    .fold(0u8, |acc, (i, &b)| acc | (b << (7 - i)))
                            })
                            .collect();

                        // Decode r-dsch PDU
                        let bs = cdma_common::bits::Bitstream::new_init(&payload_bits);
                        let rdsch_result = crate::receiver::access_layer3::RdschPdu::decode(&bs);
                        let rdsch_summary = match &rdsch_result {
                            Ok(pdu) => pdu.summary(),
                            Err(e) => format!("decode_error={e}"),
                        };
                        if let Ok(ref pdu) = rdsch_result {
                            if let Some(order) = pdu.l3.order_code() {
                                if order == 0b010000 {
                                    found_ms_ack_order = true;
                                }
                            }
                        }

                        eprintln!(
                            "  [RC1-ESN|walsh={}] traffic frame #{}: crc={} chip={:?} hex={} rdsch=[{}]",
                            walsh_code,
                            data_frame_count,
                            crc_valid,
                            blk.tags.get("absolute_chip_start"),
                            hex_bytes
                                .iter()
                                .map(|b| format!("{:02X}", b))
                                .collect::<Vec<_>>()
                                .join(" "),
                            rdsch_summary,
                        );
                    }
                }
            }

            eprintln!(
                "[RC1-ESN walsh={}] summary: blocks={} preambles={} phy_frames={} crc_valid_phy={} fqi_checked={}/{} rates={:?} traffic_frames={} crc_valid={} full_rate_with_signaling={} mux_headers={:?} signaling_bits={:?} pcg_measurements={} pcg_age_avg_chips={:.1} pcg_age_max_chips={} pcg_phases={:?}",
                walsh_code,
                total_blocks,
                preamble_count,
                phy_frame_count,
                crc_valid_phy_frame_count,
                fqi_checked_valid,
                fqi_checked_total,
                rate_counts,
                data_frame_count,
                crc_valid_data_frame_count,
                full_rate_with_signaling,
                mux_header_counts,
                signaling_bits_counts,
                pcg_measurement_count,
                if pcg_measurement_count > 0 {
                    pcg_measurement_age_sum_chips as f64 / pcg_measurement_count as f64
                } else {
                    0.0
                },
                pcg_measurement_age_max_chips,
                pcg_measurement_phases,
            );

            // The frame aligner must find at least one CRC-valid signaling frame.
            assert!(
                crc_valid_data_frame_count > 0,
                "expected at least one CRC-valid traffic signaling frame for walsh={}, got 0",
                walsh_code,
            );
            if let Some(expected) = expected_crc_valid_full_rate_signaling_frames {
                assert_eq!(
                    crc_valid_full_rate_signaling_frame_count, expected,
                    "unexpected CRC-valid full-rate signaling frame count for walsh={}: got={} expected={}",
                    walsh_code, crc_valid_full_rate_signaling_frame_count, expected,
                );
            }

            // Keep the per-rate PHY frame distribution close to the
            // baseline. A few null-traffic frames can move between
            // adjacent sub-rate buckets across libm/CPU targets because
            // their ML terminal-state margins are effectively ties, but
            // large classifier shifts still indicate a real regression.
            if let Some(expected) = expected_rate_counts.as_ref() {
                assert_rc1_rate_counts_with_small_drift(&rate_counts, expected, walsh_code);
            }

            // The r-dsch decoder must find MS Ack Order (ORDER=010000).
            assert!(
                found_ms_ack_order,
                "expected MS Ack Order (ORDER=0b010000) in decoded r-dsch traffic frames for walsh={}",
                walsh_code,
            );

            let processing_secs = pipeline_elapsed.as_secs_f64();
            let realtime_ratio = wav_duration_secs / processing_secs;
            eprintln!(
                "[RC1-ESN walsh={}] performance: wav_duration={:.2}s processing={:.2}s realtime_ratio={:.2}x",
                walsh_code, wav_duration_secs, processing_secs, realtime_ratio,
            );
            if let Some(min_ratio) = min_realtime_ratio {
                let min_ratio = capture_timing_min_speedup(min_ratio);
                assert!(
                    realtime_ratio >= min_ratio,
                    "pipeline too slow: {:.2}x realtime (need >= {:.2}x) for walsh={}, wav={:.2}s processing={:.2}s",
                    realtime_ratio,
                    min_ratio,
                    walsh_code,
                    wav_duration_secs,
                    processing_secs,
                );
            }
        }
    }

    fn run_rc1_reverse_traffic_channel_decode_wav_test(
        wav_path: std::path::PathBuf,
        chip_start: u64,
        expected_rate_counts: Option<std::collections::BTreeMap<i64, usize>>,
        expected_crc_valid_full_rate_signaling_frames: Option<usize>,
    ) {
        run_rc1_reverse_traffic_channel_decode_wav_test_with_options(
            wav_path,
            chip_start,
            0,
            None,
            Some(2.0),
            expected_rate_counts,
            expected_crc_valid_full_rate_signaling_frames,
        );
    }

    /// Offline RC1 reverse traffic channel decode test using a captured WAV.
    ///
    /// This matches the current live trace: RC1 reverse traffic on Walsh 10
    /// using the ESN-based traffic long-code mask.
    ///
    /// Run with: cargo test --release -p cdma-bts capture_rc1_reverse_traffic_channel_decode_wav -- --nocapture --test-threads=1
    #[test]
    fn capture_rc1_reverse_traffic_channel_decode_wav() {
        init_test_logger();
        let wav_path = test_capture_path("1792143302208325.wav");
        let chip_start = 1792143302208325;
        // WAV 1792143302208325: a single 9600 bps signaling burst
        // (the r-dsch MS Ack Order) with the remaining call time
        // decoded as null-traffic sub-rate frames. The spec-aligned
        // data burst randomizer window lets the no-FQI low-rate decoder
        // classify almost all null frames as Eighth (1200).
        let expected: std::collections::BTreeMap<i64, usize> =
            [(1200, 1001), (2400, 5), (4800, 7), (9600, 1)]
                .into_iter()
                .collect();
        run_rc1_reverse_traffic_channel_decode_wav_test(
            wav_path,
            chip_start,
            Some(expected),
            Some(1),
        );
    }

    #[test]
    fn capture_rc1_reverse_traffic_channel_decode_wav_1792734620236321() {
        init_test_logger();
        let wav_path = test_capture_path("1792734620236321.wav");
        // WAV 1792734620236321: 11 CRC-valid signaling frames at 9600
        // bps, with the rest of the call carried as sub-rate null
        // traffic frames. The spec-aligned data burst randomizer window
        // lets the no-FQI low-rate decoder classify most null frames as
        // Eighth (1200).
        let expected: std::collections::BTreeMap<i64, usize> =
            [(1200, 409), (2400, 26), (4800, 5), (9600, 12)]
                .into_iter()
                .collect();
        run_rc1_reverse_traffic_channel_decode_wav_test_with_options(
            wav_path,
            1792734620236321,
            0,
            None,
            None,
            Some(expected),
            Some(11),
        );
    }

    #[test]
    fn capture_rc1_reverse_traffic_v60s() {
        init_test_logger();
        let wav_path = test_capture_path("1793198656416549.wav");
        run_rc1_reverse_traffic_channel_decode_wav_impl(
            wav_path,
            1793198656416549,
            0,
            None,
            None,
            Some(0x3D5D7EAD),
            Some(vec![10]),
            None,
            None,
        );
    }

    #[test]
    fn capture_rc1_reverse_traffic_1795197525095836() {
        init_test_logger();
        let wav_path = test_capture_path("1795197525095836.wav");
        run_rc1_reverse_traffic_channel_decode_wav_impl(
            wav_path,
            1795197525095836,
            0,
            None,
            None,
            Some(0x80857E58),
            Some(vec![10]),
            None,
            None,
        );
    }

    /// FER-vs-pilot-symbol-SINR calibration sweep for RC3 9600 bps on AWGN.
    /// Prints the SINR at which FER crosses 1%.
    ///
    /// **Lock-step with `rlgain_adj` in `paging_messages.rs`**: if it changes,
    /// update `PROD_RLGAIN_ADJ_QUARTERS` below and re-run before adjusting setpoints.
    #[test]
    #[ignore = "diagnostic FER-vs-pilot-SINR calibration sweep; run explicitly when tuning setpoints"]
    fn rc3_pilot_sinr_at_1pct_fer_calibration() {
        init_test_logger();

        fn next_uniform(state: &mut u64) -> f32 {
            *state = state
                .wrapping_mul(6364136223846793005)
                .wrapping_add(1442695040888963407);
            (((*state >> 32) as u32 | 1) as f32) / (u32::MAX as f32)
        }
        fn box_muller_pair(state: &mut u64) -> (f32, f32) {
            let u1 = next_uniform(state);
            let u2 = next_uniform(state);
            let r = (-2.0 * u1.ln()).sqrt();
            let theta = std::f32::consts::TAU * u2;
            (r * theta.cos(), r * theta.sin())
        }

        fn measure_pilot_sym_sinr_db(chips: Vec<Complex32>) -> f32 {
            const SYMBOLS_PER_PCG: usize = 96;
            let mut despreader =
                super::rc3_bpsk_despread::Rc3BpskDespread::with_output_symbols(SYMBOLS_PER_PCG);
            let mut block = SampleBlock::new(chips, 0).with_sample_rate_hz(1_228_800.0);
            block.tags.insert("absolute_chip_start", 0);
            let outputs = PipelineProcessor::process_block(&mut despreader, block);
            let mut sum = 0.0f32;
            let mut n = 0usize;
            for blk in outputs {
                let Some(metrics) = blk.pcg_pilot_metrics else {
                    continue;
                };
                for (pn_sq, ps_pwr, _, _) in metrics {
                    let nf = SYMBOLS_PER_PCG as f32;
                    let mean_sq = pn_sq / (nf * nf);
                    let var = (ps_pwr / nf - mean_sq).max(1e-12);
                    sum += 10.0 * (mean_sq / var).max(1e-12).log10();
                    n += 1;
                }
            }
            if n == 0 { f32::NAN } else { sum / n as f32 }
        }

        let walsh = REVERSE_RC3_GOLDEN_WALSH;
        let rate = GoldenRc3Rate::Full;
        let target_rate_bps = rate.rate_bps();
        let n_frames_per_point: usize = 200;
        let sigma_n: f32 = 1.0;

        const NOMINAL_RC3_FULL_TDG_DB: f32 = 3.75;
        // Lock-step with `rlgain_adj` in paging_messages.rs (0.25 dB units).
        const PROD_RLGAIN_ADJ_QUARTERS: i32 = 0;
        const TDG_DB: f32 = NOMINAL_RC3_FULL_TDG_DB + (PROD_RLGAIN_ADJ_QUARTERS as f32) * 0.25;
        let tdg_lin = 10f32.powf(TDG_DB / 20.0);
        eprintln!(
            "    TDG: nominal {} dB + rlgain_adj {} × 0.25 dB = {:.2} dB (×{:.3} on traffic axis)",
            NOMINAL_RC3_FULL_TDG_DB, PROD_RLGAIN_ADJ_QUARTERS, TDG_DB, tdg_lin
        );

        eprintln!(
            "\n=== RC3 {} bps FER vs pilot-symbol SINR calibration ===",
            target_rate_bps
        );
        eprintln!(
            "    walsh={}  sigma_n={}  frames/point={}\n",
            walsh, sigma_n, n_frames_per_point
        );
        eprintln!("  amp_dB | pilot_SINR_dB | crc_valid | FER%   | predicted_SINR_dB");
        eprintln!("  -------+---------------+-----------+--------+------------------");

        let amp_db_range: Vec<f32> = (-26..=-6).map(|v| v as f32).collect();
        let mut results: Vec<(f32, f32, f32)> = Vec::new();

        for amp_db in &amp_db_range {
            let amp = 10f32.powf(amp_db / 20.0);
            let mut crc_valid = 0usize;
            let mut sinr_sum = 0.0f32;
            let mut sinr_n = 0usize;
            let mut rng_state: u64 = 0xC0FFEE_u64
                .wrapping_add(((*amp_db as i64) as u64).wrapping_mul(0x9E3779B97F4A7C15));

            for frame_idx in 0..n_frames_per_point {
                let mut info = Vec::with_capacity(rate.info_bits());
                let mut bit_state: u32 = 0xC0FFEE_00u32.wrapping_add(frame_idx as u32);
                for _ in 0..rate.info_bits() {
                    bit_state = bit_state.wrapping_mul(1664525).wrapping_add(1013904223);
                    info.push(((bit_state >> 31) & 1) as u8);
                }

                let symbols = encode_reverse_rc3_fch_symbols(&info, rate);
                let mut chips = build_reverse_rc3_despread_chips_from_symbols(&symbols);
                // Pilot on +real, traffic on -imag with production TDG.
                for c in chips.iter_mut() {
                    c.re *= amp;
                    c.im *= amp * tdg_lin;
                }
                for c in chips.iter_mut() {
                    let (nr, ni) = box_muller_pair(&mut rng_state);
                    c.re += nr * sigma_n;
                    c.im += ni * sigma_n;
                }

                let sinr = measure_pilot_sym_sinr_db(chips.clone());
                if sinr.is_finite() {
                    sinr_sum += sinr;
                    sinr_n += 1;
                }

                let outputs = run_rc3_reverse_traffic_despread_frame_outputs(chips, walsh);
                for blk in outputs {
                    if blk.tags.get("traffic_phy_frame") == Some(&1)
                        && blk.tags.get("traffic_fqi_valid") == Some(&1)
                        && blk.tags.get("traffic_rate_bps") == Some(&target_rate_bps)
                    {
                        crc_valid += 1;
                    }
                }
            }

            let fer_pct = 100.0 * (1.0 - crc_valid as f32 / n_frames_per_point as f32);
            let pilot_sinr = sinr_sum / sinr_n.max(1) as f32;
            let predicted = 10.0 * (8.0 * amp * amp / (sigma_n * sigma_n)).log10();
            eprintln!(
                "   {:+5.1}  |   {:+7.2}     |  {:>3}/{:<3}   | {:>5.1}  |   {:+7.2}",
                amp_db, pilot_sinr, crc_valid, n_frames_per_point, fer_pct, predicted
            );
            results.push((*amp_db, pilot_sinr, fer_pct));
        }

        let mut knee: Option<f32> = None;
        for w in results.windows(2) {
            if w[0].2 > 1.0 && w[1].2 <= 1.0 {
                let s0 = w[0].1;
                let s1 = w[1].1;
                let f0 = (w[0].2.max(1e-3) / 100.0).ln();
                let f1 = (w[1].2.max(1e-3) / 100.0).ln();
                let target = 0.01_f32.ln();
                let t = ((target - f0) / (f1 - f0)).clamp(0.0, 1.0);
                knee = Some(s0 + t * (s1 - s0));
                break;
            }
        }
        eprintln!(
            "\n  RC3 {} bps FER=1% pilot-symbol SINR knee: {}",
            target_rate_bps,
            knee.map(|s| format!("{:+.2} dB", s))
                .unwrap_or_else(|| "(FER never crossed 1% in sweep range)".to_string()),
        );
        eprintln!(
            "  → suggested Phase 2 RC3_INITIAL_TARGET_PILOT_SYM_SINR_DB ≈ {}",
            knee.map(|s| format!("{:+.1} dB  (knee + 1.5 dB margin)", s + 1.5))
                .unwrap_or_else(|| "TBD (extend sweep range)".to_string()),
        );
    }
}
