use crate::phy::coding::block_interleaver::{self, BitReversalInterleaver};
use crate::phy::coding::convolutional::{
    ViterbiDecoder, get_1_2_k9_encoder, get_1_3_k9_soft_viterbi_decoder,
};
use crate::phy::coding::long_code::LongCodeGenerator;
use crate::phy::walsh::WalshDecoder;
use crate::receiver::hrpd::access::{ACCESS_PACKET_CHIPS, HRPD_DEFAULT_ACCESS_PREAMBLE_FRAMES};

use super::gardner_timing_recovery::GardnerTimingConfig;
use super::rake_access_searcher;
use super::rc1_reverse_traffic_decoder::Rc1ReverseTrafficDecoder;
use super::rc2_traffic_frame_aligner::Rc2TrafficFrameAligner;
use super::rc3_bpsk_despread;
use super::rc3_frame_aligner;
use super::rc3_pilot_detector;
use super::reverse_access_decoder;
use super::{
    AccessChannelProcessor, AcquisitionFftProcessor, DeinterleaverProcessor, LongCodeDescrambler,
    MatchedFilterDespreader, PipelineProcessorShared, PulseMatchedFilterProcessor,
    ReverseAccessOrthogonalDemodProcessor, ReverseAccessSettings, SoftViterbiDecoderR13Processor,
    SyncChannelProcessor, TrafficChannelProcessor, Unrepeater, ViterbiDecoderProcessor,
    WalshPilotCombiner,
};
use super::{HrpdAccessFrameFftConfig, HrpdAccessFrameRakeCorrelator};
use super::{generic_rake_receiver, pn_lc_correlator};

const REVERSE_ACCESS_ACTIVE_FINGER_DELAY_SUPPRESSION: bool = true;
const REVERSE_ACCESS_ACTIVE_FINGER_DELAY_SUPPRESS_SAMPLES: i32 = 0;
const RC1_TRAFFIC_PREAMBLE_COH_NORM_MIN: f32 = 0.15;
/// Reverse-access finger budget, fixed to the value the access capture
/// regression suite exercises.
const REVERSE_ACCESS_MAX_FINGERS: usize = 10;

/// Settings for building a reverse traffic channel receiver pipeline.
pub struct ReverseTrafficSettings {
    pub oversample: usize,
    /// Walsh code assigned to this traffic channel.
    pub walsh_code: u8,
    /// Mobile ESN (used to derive the traffic channel long code mask).
    pub esn: u32,
    /// Re-anchor origin on every block using hardware timestamps.
    pub reanchor_origin: bool,
    /// Override the default SNR threshold for PN/LC correlator search.
    pub snr_threshold: Option<f32>,
    /// Number of preamble PCGs required before declaring pilot acquisition.
    /// Maps to NUM_PREAMBLE from the Channel Assignment. None = use default (4).
    pub preamble_num_pcgs: Option<usize>,
    /// Enable pilot-coherent EPL tracking and active slew correction.
    /// Uses Walsh 0 (16-chip) as the timing reference for RC3+ traffic.
    pub epl_pilot: bool,
    /// Per C.S0002-E §2.1.3.12.7: when true and rate is 1500 bps (RC3),
    /// the mobile only transmits R-FCH on PCGs {2,3,6,7,10,11,14,15}.
    pub rev_fch_gating_mode: bool,
    /// Maximum RAKE finger workers. A value of one processes inline.
    pub finger_pool_size: usize,
}

/// Settings for an HRPD Reverse Access Channel receiver.
pub struct HrpdReverseAccessSettings {
    pub oversample: usize,
    pub access_cycle_number: u8,
    pub sector_id_lsb: u32,
    pub color_code: u8,
    pub reanchor_origin: bool,
    pub snr_threshold: Option<f32>,
    pub finger_pool_size: usize,
    /// AccessParameters `PreambleLength` (in frames) the reverse-access finger
    /// despreads the capsule at.
    pub preamble_frames: usize,
    /// Also hypothesize the Enhanced Access Channel MAC 19.2/38.4 kbps
    /// capsule packet sizes. Enable only when the sector broadcasts an
    /// enhanced AccessParameters with `SectorAccessMaxRate` above 9.6 kbps.
    pub enhanced_access_rates: bool,
}

impl Default for HrpdReverseAccessSettings {
    fn default() -> Self {
        Self {
            oversample: 4,
            access_cycle_number: 0,
            sector_id_lsb: 0,
            color_code: 26,
            reanchor_origin: false,
            snr_threshold: None,
            finger_pool_size: 8,
            preamble_frames: HRPD_DEFAULT_ACCESS_PREAMBLE_FRAMES,
            enhanced_access_rates: false,
        }
    }
}

// ---------------------------------------------------------------------------
// Pre-built chains
// ---------------------------------------------------------------------------

/// Build a pipeline chain for the **Sync Channel** (Walsh 32).
pub fn sync_channel_chain(conv_invert_pair: bool) -> Vec<PipelineProcessorShared> {
    vec![
        Box::new(
            WalshPilotCombiner::new(WalshDecoder::new::<64>(32), WalshDecoder::new::<64>(0))
                .with_absolute_chip_modulus(64),
        ),
        Box::new(Unrepeater::new(4)),
        Box::new(DeinterleaverProcessor::new(
            BitReversalInterleaver::new(block_interleaver::SR1_PARAMS_128),
            2,
        )),
        Box::new(ViterbiDecoderProcessor::new(
            ViterbiDecoder::new(get_1_2_k9_encoder()),
            false,
            conv_invert_pair,
        )),
    ]
}

/// Build a full raw-IQ sync chain:
///   pulse-matched-filter -> acquisition -> despread -> sync decode chain.
pub fn sync_channel_chain_with_acquisition(
    sample_rate: u32,
    conv_invert_pair: bool,
) -> Vec<PipelineProcessorShared> {
    let mut chain: Vec<PipelineProcessorShared> = vec![
        Box::new(PulseMatchedFilterProcessor::new()),
        Box::new(AcquisitionFftProcessor::new(sample_rate)),
        Box::new(MatchedFilterDespreader::new(sample_rate)),
    ];
    chain.extend(sync_channel_chain(conv_invert_pair));
    chain
}

/// Build a full raw-IQ mobile-station sync chain:
///   pulse-matched-filter -> acquisition -> despread -> sync decode chain -> MS sync parser.
pub fn mobile_station_sync_chain_with_acquisition(
    sample_rate: u32,
    conv_invert_pair: bool,
) -> Vec<PipelineProcessorShared> {
    let mut chain = sync_channel_chain_with_acquisition(sample_rate, conv_invert_pair);
    chain.push(Box::new(SyncChannelProcessor::new()));
    chain
}

/// Build a pipeline chain for the **Paging Channel** (Walsh 1, 9600 bps).
pub fn paging_channel_chain(
    long_code_generator: LongCodeGenerator,
    rate_9600: bool,
    conv_invert_pair: bool,
) -> Vec<PipelineProcessorShared> {
    let (unrepeat, interleaver_params) = if rate_9600 {
        (1usize, block_interleaver::SR1_PARAMS_384)
    } else {
        (2usize, block_interleaver::SR1_PARAMS_192)
    };

    vec![
        Box::new(WalshPilotCombiner::new(
            WalshDecoder::new::<64>(1),
            WalshDecoder::new::<64>(0),
        )),
        Box::new(Unrepeater::new(unrepeat)),
        Box::new(LongCodeDescrambler::new(long_code_generator, 64)),
        Box::new(DeinterleaverProcessor::new(
            BitReversalInterleaver::new(interleaver_params),
            1,
        )),
        Box::new(ViterbiDecoderProcessor::new(
            ViterbiDecoder::new(get_1_2_k9_encoder()),
            false,
            conv_invert_pair,
        )),
    ]
}

/// Build a full raw-IQ paging chain:
///   pulse-matched-filter -> acquisition -> despread -> paging decode chain.
pub fn paging_channel_chain_with_acquisition(
    sample_rate: u32,
    long_code_generator: LongCodeGenerator,
    rate_9600: bool,
    conv_invert_pair: bool,
) -> Vec<PipelineProcessorShared> {
    let mut chain: Vec<PipelineProcessorShared> = vec![
        Box::new(PulseMatchedFilterProcessor::new()),
        Box::new(AcquisitionFftProcessor::new(sample_rate)),
        Box::new(MatchedFilterDespreader::new(sample_rate)),
    ];
    chain.extend(paging_channel_chain(
        long_code_generator,
        rate_9600,
        conv_invert_pair,
    ));
    chain
}

/// Build a decoded-symbol Access Channel chain (post reverse orthogonal demod).
///
/// Input is expected to be soft symbols at the Access Channel interleaver output
/// rate (576 symbols per 20 ms frame).
pub fn access_channel_chain() -> Vec<PipelineProcessorShared> {
    vec![
        Box::new(DeinterleaverProcessor::new(
            BitReversalInterleaver::new(block_interleaver::SR1_PARAMS_576),
            2,
        )),
        Box::new(
            SoftViterbiDecoderR13Processor::new(get_1_3_k9_soft_viterbi_decoder())
                .with_reset_per_block(true)
                .with_assume_zero_end_state(true),
        ),
        Box::new(AccessChannelProcessor::new()),
    ]
}

/// Build the default reverse-link Access chain used by the live RX path.
///
/// Uses the `PnLcCorrelator + GenericRakeReceiver` frontend and the chip-rate
/// preamble/Walsh/frame alignment chain (4x oversampled).
pub fn reverse_access_chain(settings: ReverseAccessSettings) -> Vec<PipelineProcessorShared> {
    let correlator = pn_lc_correlator::PnLcCorrelator::new(
        pn_lc_correlator::PnLcConfig::default_4x()
            .with_snr_threshold(20.0)
            .with_lc_half_span(4)
            .with_search_interval_windows(32)
            .with_split_pn_reference(true)
            .with_reanchor_origin(settings.reanchor_origin)
            .with_fractional_timing_recovery(false)
            .with_finger_timing_adaptive_search(0.5, 0.5)
            .with_active_finger_delay_suppression(
                REVERSE_ACCESS_ACTIVE_FINGER_DELAY_SUPPRESSION,
                REVERSE_ACCESS_ACTIVE_FINGER_DELAY_SUPPRESS_SAMPLES,
            )
            .with_gardner_timing(GardnerTimingConfig::reverse_access_4x())
            .with_output_oversampled_chips(false)
            .with_access_cfo(true),
        LongCodeGenerator::new_access_channel_with_state(
            settings.access_channel_number,
            settings.paging_channel_number,
            settings.base_id,
            settings.pilot_pn,
            settings.long_code_state,
        ),
        Box::new(|| {
            vec![
                Box::new(
                    reverse_access_decoder::ReverseAccessDecoder::new().with_soft_viterbi(true),
                ),
                Box::new(AccessChannelProcessor::new()),
            ]
        }),
    );

    vec![
        Box::new(PulseMatchedFilterProcessor::new()),
        Box::new(
            generic_rake_receiver::GenericRakeReceiver::new(correlator)
                .with_max_fingers(REVERSE_ACCESS_MAX_FINGERS)
                .with_finger_pool_size(settings.finger_pool_size),
        ),
    ]
}

/// Build the production HRPD Reverse Access Channel receiver.
///
/// HRPD reverse access has an explicit pilot-only preamble before the access
/// data packet.  Use that spec-defined preamble to train the access receiver
/// directly instead of spawning PN/LC rake fingers on low-SNR candidates.
pub fn hrpd_reverse_access_chain(
    settings: HrpdReverseAccessSettings,
) -> Vec<PipelineProcessorShared> {
    let correlator = HrpdAccessFrameRakeCorrelator::new(HrpdAccessFrameFftConfig {
        oversample: settings.oversample,
        access_cycle_number: settings.access_cycle_number,
        sector_id_lsb: settings.sector_id_lsb,
        color_code: settings.color_code,
        preamble_frames: settings.preamble_frames,
        snr_threshold: settings.snr_threshold.unwrap_or(11.0),
        decode: crate::receiver::hrpd::access::HrpdAccessDecodeConfig {
            enhanced_rates: settings.enhanced_access_rates,
        },
        ..HrpdAccessFrameFftConfig::default()
    });
    vec![Box::new(
        generic_rake_receiver::GenericRakeReceiver::new(correlator)
            .with_max_fingers(32)
            .with_finger_pool_size(settings.finger_pool_size)
            .with_prune_policy(Box::new(generic_rake_receiver::DefaultPrunePolicy {
                // A freshly spawned finger still waits on the rest of the
                // capsule streaming in. Give it the full 4-frame window.
                max_idle_chips: 4 * ACCESS_PACKET_CHIPS as u64,
                max_validated_idle_chips: ACCESS_PACKET_CHIPS as u64,
                ..generic_rake_receiver::DefaultPrunePolicy::default()
            })),
    )]
}

pub(crate) fn reverse_traffic_prune_policy() -> generic_rake_receiver::DefaultPrunePolicy {
    generic_rake_receiver::DefaultPrunePolicy {
        // RC1 traffic search can sit in the preamble/null-frame hunt for
        // multiple seconds before the first real frame lock.
        max_idle_chips: 6_291_456,
        // RC1 reverse traffic can sit quiet for multiple seconds between the
        // initial preamble/search episode and the first real signaling frame.
        // Keep validated traffic fingers on the older long idle budget so
        // they are not retired by the tighter access-channel default.
        max_validated_idle_chips: 6_291_456,
        // Reverse traffic signaling can stay quiet for long stretches after
        // the preamble while the RC1 frame/rate search waits for the first
        // real signaling block. Keep the traffic path alive much longer than
        // bursty access probes.
        max_post_walsh_no_event_chips: 192 * 24_576,
        // Use signal-time chip budgets, not wall-clock host time, to retire
        // traffic fingers. Offline RC1 search is computationally heavy and
        // can legitimately take seconds of host time before producing the
        // first decoded signaling frame.
        max_post_walsh_no_event_ms: u64::MAX,
        max_validated_post_walsh_no_event_ms: u64::MAX,
        max_post_walsh_miss_count: 32,
        ..generic_rake_receiver::DefaultPrunePolicy::default()
    }
}

/// Build an RC1 reverse traffic channel sub-chain.
///
/// Uses `Rc1ReverseTrafficDecoder` which anchors directly on the 256-chip
/// Walsh symbol grid and 24576-chip frame grid from the absolute chip
/// counter. No chip_phase or frame_phase search needed.
pub fn traffic_channel_chain(
    esn: u32,
    walsh_code: u8,
    _expected_preamble_frames: usize,
) -> Vec<PipelineProcessorShared> {
    vec![
        Box::new(Rc1ReverseTrafficDecoder::new(esn)),
        Box::new(TrafficChannelProcessor::new(walsh_code)),
    ]
}

/// Build a complete reverse traffic channel receiver pipeline.
///
/// Uses `PnLcCorrelator` + `GenericRakeReceiver` with the traffic channel
/// long code mask. The per-finger sub-chain follows the reverse traffic
/// receiver doc rather than the older access-derived RC1 path.
pub fn reverse_traffic_chain(settings: ReverseTrafficSettings) -> Vec<PipelineProcessorShared> {
    let walsh_code = settings.walsh_code;
    let expected_preamble_frames = settings
        .preamble_num_pcgs
        .map(|pcgs| ((pcgs.max(1)) + 15) / 16)
        .unwrap_or(1);

    let snr_threshold = settings.snr_threshold.unwrap_or(20.0);
    let correlator = pn_lc_correlator::PnLcCorrelator::new(
        pn_lc_correlator::PnLcConfig::default_4x()
            .with_snr_threshold(snr_threshold)
            .with_lc_half_span(4)
            // RC1's preamble measures 0.15-0.22 at valid traffic timing.
            .with_preamble_coh_norm_min(RC1_TRAFFIC_PREAMBLE_COH_NORM_MIN)
            // Rejects early locks that report a preamble but decode nothing.
            .with_preamble_hits_required(3)
            .with_search_interval_windows(32)
            .with_split_pn_reference(true)
            .with_reanchor_origin(settings.reanchor_origin)
            .with_suppress_search_when_locked(true)
            // Gated transmission and FER bursts must not start a competing
            // finger, so suppression outlasts a low-energy gap.
            .with_retained_search_suppression(true)
            // Gated E/P/L energy is not a stable steering reference.
            .with_epl_tracking(false)
            .with_epl_slew(false)
            // Path delay steps by whole chips during a call, which a
            // quarter-chip early/late gate cannot see.
            .with_delay_tracking(true)
            // RC1 has no pilot, so this keeps the prompt centered as the
            // sample clocks drift apart.
            .with_nonpilot_cfo_tracking(true),
        LongCodeGenerator::new_traffic_channel(settings.esn),
        Box::new(move || traffic_channel_chain(settings.esn, walsh_code, expected_preamble_frames)),
    );

    vec![
        Box::new(PulseMatchedFilterProcessor::new()),
        Box::new(
            generic_rake_receiver::GenericRakeReceiver::new(correlator)
                .with_finger_pool_size(settings.finger_pool_size)
                .with_prune_policy(Box::new(reverse_traffic_prune_policy())),
        ),
    ]
}

/// Build an RC2 reverse traffic channel sub-chain.
pub fn traffic_channel_chain_rc2(esn: u32, walsh_code: u8) -> Vec<PipelineProcessorShared> {
    vec![
        Box::new(Rc2TrafficFrameAligner::new(esn)),
        Box::new(TrafficChannelProcessor::new(walsh_code)),
    ]
}

/// Build the RC2 reverse traffic receiver chain.
pub fn reverse_traffic_chain_rc2(settings: ReverseTrafficSettings) -> Vec<PipelineProcessorShared> {
    let walsh_code = settings.walsh_code;

    let snr_threshold = settings.snr_threshold.unwrap_or(20.0);
    let correlator = pn_lc_correlator::PnLcCorrelator::new(
        pn_lc_correlator::PnLcConfig::default_4x()
            .with_snr_threshold(snr_threshold)
            .with_lc_half_span(4)
            .with_preamble_hits_required(3)
            .with_search_interval_windows(32)
            .with_split_pn_reference(true)
            .with_reanchor_origin(settings.reanchor_origin)
            .with_suppress_search_when_locked(true)
            .with_fractional_timing_recovery(true)
            .with_epl_tracking(true)
            .with_epl_slew(false)
            .with_nonpilot_cfo_tracking(false),
        LongCodeGenerator::new_traffic_channel(settings.esn),
        Box::new(move || traffic_channel_chain_rc2(settings.esn, walsh_code)),
    );

    vec![
        Box::new(PulseMatchedFilterProcessor::new()),
        Box::new(
            generic_rake_receiver::GenericRakeReceiver::new(correlator)
                .with_prune_policy(Box::new(reverse_traffic_prune_policy())),
        ),
    ]
}

/// Build an RC3 traffic channel sub-chain (TrafficChannelProcessor only).
///
/// The Rc3FrameAligner already performs deinterleaving, de-repetition, and
/// Viterbi decoding internally, emitting decoded bits with all required tags
/// (`traffic_decoded_frame`, `traffic_fqi_valid`, `traffic_tail_valid`, etc.).
pub fn traffic_channel_chain_rc3(walsh_code: u8) -> Vec<PipelineProcessorShared> {
    vec![Box::new(TrafficChannelProcessor::new(walsh_code))]
}

/// Build a complete RC3 reverse traffic channel receiver pipeline.
///
/// Uses the same `PnLcCorrelator` + `GenericRakeReceiver` architecture as RC1,
/// but the per-finger sub-chain uses RC3-specific processing:
///   - `Rc3PilotDetector` (pilot energy detection, replaces W0 preamble detector)
///   - `Rc3BpskDespread` (W(4,16) BPSK despreading, replaces 64-ary Walsh demod)
///   - `Rc3FrameAligner` (1536-symbol frame boundary search with R=1/4 Viterbi)
///   - `DeinterleaverProcessor` (1536-symbol bit-reversal deinterleaver)
///   - `HardViterbiDecoderR14Processor` (R=1/4 K=9 Viterbi)
///   - `TrafficChannelProcessor` (frame assembly, CRC, message output)
pub fn reverse_traffic_chain_rc3(settings: ReverseTrafficSettings) -> Vec<PipelineProcessorShared> {
    const RC3_SYMBOLS_PER_PCG: usize = 96;

    let walsh_code = settings.walsh_code;
    let preamble_pcgs = settings.preamble_num_pcgs;
    let rev_fch_gating_mode = settings.rev_fch_gating_mode;

    let snr_threshold = settings.snr_threshold.unwrap_or(20.0);
    let correlator = pn_lc_correlator::PnLcCorrelator::new(
        pn_lc_correlator::PnLcConfig::default_4x()
            .with_snr_threshold(snr_threshold)
            .with_lc_half_span(4)
            // Match the conservative traffic acquisition gate used for RC1.
            // The 1-hit default was introduced for access-channel PN_RAN work;
            // keep reverse traffic on 3 hits until it has its own tuned default.
            .with_preamble_hits_required(3)
            .with_search_interval_windows(32)
            .with_split_pn_reference(true)
            .with_reanchor_origin(settings.reanchor_origin)
            .with_lc_decimation(2) // HPSK: c_long = c_I + j*c_Q
            .with_suppress_search_when_locked(true)
            // FER bursts must not start a competing finger.
            .with_retained_search_suppression(true)
            .with_epl_pilot(settings.epl_pilot)
            .with_rc3_pilot_gating_mode(rev_fch_gating_mode)
            // Without slewing, clock drift moves the despreader off the pilot.
            .with_epl_slew(settings.epl_pilot),
        LongCodeGenerator::new_traffic_channel(settings.esn),
        Box::new(move || {
            let pilot_detector = match preamble_pcgs {
                Some(n) => rc3_pilot_detector::Rc3PilotDetector::with_min_pcgs(n),
                None => rc3_pilot_detector::Rc3PilotDetector::new(),
            }
            .with_preamble_tag("traffic_preamble_detected");
            let mut chain: Vec<PipelineProcessorShared> = vec![
                Box::new(pilot_detector),
                // Emit one PCG of despread RC3 symbols per block so the
                // aligner can produce a per-PCG measurement at 1.25 ms cadence.
                Box::new(rc3_bpsk_despread::Rc3BpskDespread::with_output_symbols(
                    RC3_SYMBOLS_PER_PCG,
                )),
                Box::new(
                    rc3_frame_aligner::Rc3FrameAligner::new()
                        .with_walsh_code(walsh_code)
                        .with_rev_fch_gating_mode(rev_fch_gating_mode),
                ),
            ];
            chain.extend(traffic_channel_chain_rc3(walsh_code));
            chain
        }),
    );

    vec![
        Box::new(PulseMatchedFilterProcessor::new()),
        Box::new(
            generic_rake_receiver::GenericRakeReceiver::new(correlator)
                // One decoder per bearer, so speculative fingers cannot enter
                // FER and signaling accounting.
                .with_max_fingers(1)
                .with_finger_pool_size(settings.finger_pool_size)
                .with_prune_policy(Box::new(reverse_traffic_prune_policy())),
        ),
    ]
}

pub fn reverse_access_chain_rake(settings: ReverseAccessSettings) -> Vec<PipelineProcessorShared> {
    let oversample = settings.oversample.max(1);
    let lc_template = LongCodeGenerator::new_access_channel_with_state(
        settings.access_channel_number,
        settings.paging_channel_number,
        settings.base_id,
        settings.pilot_pn,
        settings.long_code_state,
    );
    let searcher = rake_access_searcher::RakeAccessSearcher::new(oversample, lc_template)
        .with_chain_builder(Box::new(move || {
            let mut chain: Vec<PipelineProcessorShared> =
                vec![Box::new(ReverseAccessOrthogonalDemodProcessor::new())];
            chain.extend(access_channel_chain());
            chain
        }));
    vec![
        Box::new(PulseMatchedFilterProcessor::new()),
        Box::new(searcher),
    ]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traffic_channel_chain_rc2_ends_in_rc2_frame_aligner() {
        let chain = traffic_channel_chain_rc2(0, 10);
        assert_eq!(chain.len(), 2);
        assert_eq!(chain[0].name(), "Rc2TrafficFrameAligner");
        assert_eq!(chain[1].name(), "TrafficChannelProcessor");
    }
}
