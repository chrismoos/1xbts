//! HRPD reverse access full-frame FFT preamble correlator.
//!
//! This is the production acquisition path for HRPD reverse access.  It
//! correlates one spec-derived access preamble PHY frame over its circular
//! delay range and uses the peak to spawn a burst-local finger. The finger uses
//! the acquired preamble only for burst timing, then despreads the packet with
//! the spec-derived access PN/LC sequence before feeding chip-rate packet chips
//! to the normal downstream packet processor.

use std::collections::{HashMap, VecDeque};

use log::{debug, info};
use num_complex::Complex32;

use crate::receiver::hrpd::access::{
    ACCESS_CHIP_RATE, ACCESS_PACKET_CHIPS, HRPD_DEFAULT_ACCESS_PREAMBLE_FRAMES,
    HrpdAccessDecodeConfig, HrpdAccessPacketProcessor, access_event_block,
    decode_access_phy_chips_with_attempt, decode_access_phy_chips_with_config,
    reassemble_access_mac_capsule_packet, validate_access_mac_fragment,
};
use crate::receiver::hrpd::long_code::{HrpdAccessLongCodeMask, derive_q_mask};
use crate::receiver::hrpd::reverse_correlator_base::{
    HrpdReverseCorrelatorBase, HrpdReverseFingerSpawnStrategy, SpawnOutcome,
};
use crate::receiver::hrpd::reverse_fft_pilot_search::{
    HrpdReverseFftPilotHit, HrpdReverseFftPilotSearchConfig, HrpdReverseFftPilotSearcher,
    HrpdReversePilotReference,
};
use crate::receiver::hrpd::reverse_spread::{
    HrpdReversePilotReferenceConfig, hrpd_reverse_pilot_reference_chips,
};
use crate::receiver::pipelined::generic_rake_receiver::{BaseFinger, Correlator, RakeFinger};
use crate::receiver::pipelined::{PipelineProcessorShared, SampleBlock};

const ACCESS_PREAMBLE_MIN_LAG_COHERENCE: f32 = 0.40;
const ACCESS_PREAMBLE_MIN_SPEC_COHERENCE: f32 = 0.35;

#[derive(Clone, Debug)]
pub struct HrpdAccessFrameFftConfig {
    pub oversample: usize,
    pub search_window_frames: usize,
    pub search_step_frames: usize,
    pub access_cycle_number: u8,
    pub derive_access_cycle_number_from_window: bool,
    pub sector_id_lsb: u32,
    pub color_code: u8,
    /// AccessParameters `PreambleLength` (in frames). The finger despreads the
    /// capsule starting this many frames past the preamble start.
    pub preamble_frames: usize,
    pub reference_chip_offset: i32,
    pub q_pair_phase: u64,
    pub q_sign: f32,
    pub snr_threshold: f32,
    pub max_fft_hits_per_window: usize,
    pub fft_hit_suppression_chips: usize,
    pub pn_phase_offset_chips: i32,
    pub lc_phase_offset_chips: i32,
    /// Capsule rate hypotheses for packet decode. Enable the enhanced sizes
    /// only when the sector broadcasts an enhanced AccessParameters with
    /// `SectorAccessMaxRate` above 9.6 kbps.
    pub decode: HrpdAccessDecodeConfig,
}

impl Default for HrpdAccessFrameFftConfig {
    fn default() -> Self {
        Self {
            oversample: 4,
            search_window_frames: 1,
            search_step_frames: 1,
            access_cycle_number: 0,
            derive_access_cycle_number_from_window: true,
            sector_id_lsb: 0,
            color_code: 26,
            preamble_frames: HRPD_DEFAULT_ACCESS_PREAMBLE_FRAMES,
            reference_chip_offset: 0,
            q_pair_phase: 0,
            q_sign: -1.0,
            snr_threshold: 20.0,
            max_fft_hits_per_window: 8,
            fft_hit_suppression_chips: ACCESS_PACKET_CHIPS / 2,
            pn_phase_offset_chips: 0,
            lc_phase_offset_chips: 0,
            decode: HrpdAccessDecodeConfig::REV0,
        }
    }
}

#[derive(Clone, Debug)]
pub struct HrpdAccessFrameFftHit {
    pub snr: f32,
    pub snr_db: f32,
    pub peak_power: f32,
    pub mean_power: f32,
    pub window_index: usize,
    pub window_sample_offset: usize,
    pub delay_samples: usize,
    pub preamble_start_sample: u64,
    pub preamble_start_chip: u64,
    pub sample_phase: usize,
    pub frame_phase_chips: u64,
    pub access_cycle_number: u8,
}

#[derive(Clone, Debug)]
pub struct HrpdAccessPreambleCoherence {
    pub chips: usize,
    pub preamble_start_sample: u64,
    pub preamble_start_chip: u64,
    pub sample_delay_samples: i32,
    pub access_cycle_number: u8,
    pub coherent_sum: Complex32,
    pub coherent_mean: Complex32,
    pub coherent_ratio: f32,
    pub rotated_i_mean: f32,
    pub rotated_q_mean: f32,
    pub rms: f32,
    pub sign_agreement: f32,
}

pub struct HrpdAccessFrameCorrelator {
    cfg: HrpdAccessFrameFftConfig,
    searcher: HrpdReverseFftPilotSearcher,
    reference: HrpdAccessFftPilotReference,
}

/// Access-channel adapter exposing the [`HrpdReversePilotReference`] trait.
/// Picks the access cycle number from the window-start chip when the config
/// requests it, otherwise pins to the configured ACN.
#[derive(Clone, Debug)]
pub struct HrpdAccessFftPilotReference {
    sector_id_lsb: u32,
    color_code: u8,
    reference_chip_offset: i32,
    q_pair_phase: u64,
    q_sign: f32,
    derive_acn_from_window: bool,
    pinned_acn: u8,
}

impl HrpdAccessFftPilotReference {
    pub(crate) fn from_config(cfg: &HrpdAccessFrameFftConfig) -> Self {
        Self {
            sector_id_lsb: cfg.sector_id_lsb,
            color_code: cfg.color_code,
            reference_chip_offset: cfg.reference_chip_offset,
            q_pair_phase: cfg.q_pair_phase,
            q_sign: cfg.q_sign,
            derive_acn_from_window: cfg.derive_access_cycle_number_from_window,
            pinned_acn: cfg.access_cycle_number,
        }
    }

    fn access_cycle_number_for_chip(&self, chip: u64) -> u8 {
        if self.derive_acn_from_window {
            derived_access_cycle_number_for_chip(chip)
        } else {
            self.pinned_acn
        }
    }
}

impl HrpdReversePilotReference for HrpdAccessFftPilotReference {
    fn template_chips(&self, window_start_chip: u64, len: usize) -> Vec<Complex32> {
        let acn = self.access_cycle_number_for_chip(window_start_chip);
        // The existing access correlator builds the FFT template at chip-0
        // phase against the current ACN's mask. Preserve that behavior.
        let _ = len;
        hrpd_access_preamble_chips(
            0,
            acn,
            self.sector_id_lsb,
            self.color_code,
            self.reference_chip_offset,
            self.q_pair_phase,
            self.q_sign,
            ACCESS_PACKET_CHIPS,
        )
    }

    /// The access template depends on the access cycle number as well as the
    /// short-PN phase, so both key the cache.
    fn reference_cache_key(&self, window_start_chip: u64) -> u64 {
        let acn = self.access_cycle_number_for_chip(window_start_chip) as u64;
        (acn << 32) | (window_start_chip % ACCESS_PACKET_CHIPS as u64)
    }
}

/// Reverse-access rake correlator: stitches incoming IQ blocks, runs the
/// shared FFT preamble searcher each frame, and constructs the access
/// preamble finger plus packet sub-chain for each fresh hit. Delegates the
/// scan-loop scaffolding to [`HrpdReverseCorrelatorBase`].
pub struct HrpdAccessFrameRakeCorrelator {
    base: HrpdReverseCorrelatorBase<HrpdAccessFrameSpawnStrategy>,
}

struct HrpdAccessFrameSpawnStrategy {
    cfg: HrpdAccessFrameFftConfig,
    reference: HrpdAccessFftPilotReference,
    sample_rate_hz: f64,
    next_finger_id: u64,
    emitted_packet_starts: VecDeque<i64>,
    active_fingers: Vec<HrpdAccessFrameFingerState>,
    /// The FFT search fires on any frame-periodic energy, including an
    /// ongoing reverse traffic session's pilot, which looks periodic like an
    /// access preamble but can never pass the preamble timing sweep. Once
    /// the sweep rejects such a signal, hits are skipped until the scan
    /// passes this chip, so a long traffic session doesn't re-run the sweep
    /// on every window.
    foreign_signal_until_chip: i64,
}

#[derive(Clone, Debug)]
struct HrpdAccessFrameFingerState {
    id: u64,
    packet_start_chip: i64,
    hard_validated: bool,
    idle_chips: u64,
}

pub struct HrpdAccessFrameFinger {
    base: BaseFinger,
    oversample: usize,
    preamble_start_chip: i64,
    timing_candidates: Vec<HrpdAccessPreambleTiming>,
    preamble_frames: usize,
    detection_snr: f32,
    lag_coherence: f32,
    access_cycle_number: u8,
    sector_id_lsb: u32,
    color_code: u8,
    reference_chip_offset: i32,
    q_pair_phase: u64,
    q_sign: f32,
    pn_phase_offset_chips: i32,
    lc_phase_offset_chips: i32,
    decode: HrpdAccessDecodeConfig,
    sample_rate_hz: f64,
    buffer: Vec<Complex32>,
    buffer_abs_sample: Option<i64>,
    ignore_until_abs_sample: i64,
    emitted_packet: bool,
}

#[derive(Clone, Debug)]
struct HrpdAccessPreambleTiming {
    preamble_start_chip: i64,
    sample_delay: i32,
    sample_delay_fraction: f32,
    lag_coherence: f32,
    spec_coherence: f32,
    phase_step: f32,
}

#[derive(Clone, Copy, Debug)]
enum HrpdAccessPacketDespreadMode {
    Composite,
}

impl HrpdAccessPacketDespreadMode {
    const ALL: [Self; 1] = [Self::Composite];

    fn label(self) -> &'static str {
        match self {
            Self::Composite => "composite",
        }
    }
}

impl HrpdAccessFrameRakeCorrelator {
    pub fn new(cfg: HrpdAccessFrameFftConfig) -> Self {
        let strategy = HrpdAccessFrameSpawnStrategy {
            reference: HrpdAccessFftPilotReference::from_config(&cfg),
            cfg,
            sample_rate_hz: ACCESS_CHIP_RATE as f64 * 4.0,
            next_finger_id: 1,
            emitted_packet_starts: VecDeque::new(),
            active_fingers: Vec::new(),
            foreign_signal_until_chip: 0,
        };
        Self {
            base: HrpdReverseCorrelatorBase::new(strategy, "hrpd_access"),
        }
    }
}

impl HrpdAccessFrameSpawnStrategy {
    fn is_duplicate_packet(&self, packet_start_chip: i64) -> bool {
        const DUP_CHIPS: i64 = ACCESS_PACKET_CHIPS as i64;
        self.emitted_packet_starts
            .iter()
            .any(|&prior| (prior - packet_start_chip).abs() < DUP_CHIPS)
            || self
                .active_fingers
                .iter()
                .any(|state| (state.packet_start_chip - packet_start_chip).abs() < DUP_CHIPS)
    }

    fn remember_packet(&mut self, packet_start_chip: i64) {
        self.emitted_packet_starts.push_back(packet_start_chip);
        while self.emitted_packet_starts.len() > 256 {
            self.emitted_packet_starts.pop_front();
        }
    }

    /// Cheap same-burst suppression run before the timing-candidate search.
    /// The forward window stops short of the closest possible back-to-back
    /// probe. The post-search duplicate check catches anything missed here.
    fn is_recent_burst_region(&self, estimated_packet_start_chip: i64) -> bool {
        const BACK_CHIPS: i64 = ACCESS_PACKET_CHIPS as i64;
        const FORWARD_CHIPS: i64 = 5 * ACCESS_PACKET_CHIPS as i64;
        let in_region = |prior: i64| {
            let diff = estimated_packet_start_chip - prior;
            diff > -BACK_CHIPS && diff < FORWARD_CHIPS
        };
        self.emitted_packet_starts
            .iter()
            .any(|&prior| in_region(prior))
            || self
                .active_fingers
                .iter()
                .any(|state| in_region(state.packet_start_chip))
    }
}

impl HrpdReverseFingerSpawnStrategy for HrpdAccessFrameSpawnStrategy {
    type Finger = HrpdAccessFrameFinger;
    type Reference = HrpdAccessFftPilotReference;

    fn fft_config(&self) -> HrpdReverseFftPilotSearchConfig {
        HrpdReverseFftPilotSearchConfig {
            oversample: self.cfg.oversample,
            frame_chips: ACCESS_PACKET_CHIPS,
            search_window_frames: self.cfg.search_window_frames,
            search_step_frames: self.cfg.search_step_frames,
            snr_threshold: self.cfg.snr_threshold,
            max_hits_per_window: self.cfg.max_fft_hits_per_window,
            hit_suppression_chips: self.cfg.fft_hit_suppression_chips,
        }
    }

    fn reference(&self) -> &Self::Reference {
        &self.reference
    }

    fn max_hits_per_window(&self) -> usize {
        self.cfg.max_fft_hits_per_window
    }

    /// Frame-grid-aligned scan windows guarantee every probe lands at the
    /// start of a window whose template ACN matches the probe's own.
    fn align_scan_to_frame_grid(&self) -> bool {
        true
    }

    fn absolute_sample_start_for_block(&mut self, block: &SampleBlock) -> u64 {
        if block.sample_rate_hz > 0.0 {
            self.sample_rate_hz = block.sample_rate_hz;
        }
        block
            .tags
            .get("absolute_sample_start")
            .copied()
            .map(|v| v.max(0) as u64)
            .unwrap_or(block.chip_start as u64)
    }

    fn spawn_finger(
        &mut self,
        buffer: &[Complex32],
        buffer_abs_sample: u64,
        hit: &HrpdReverseFftPilotHit,
    ) -> SpawnOutcome<Self::Finger> {
        let aligned_start_chip = align_hit_to_frame_start(hit);
        let buffer_abs_signed = buffer_abs_sample as i64;
        // Cheap pre-gates before the expensive timing-candidate search.
        //
        // 1. Same-burst suppression: skip hits landing in the region of an
        //    already-spawned burst.
        let estimated_packet_start = aligned_start_chip + (3 * ACCESS_PACKET_CHIPS) as i64;
        if self.is_recent_burst_region(estimated_packet_start) {
            return SpawnOutcome::Skip;
        }
        // Inside a known foreign frame-periodic signal, skip hits instead of
        // re-running the timing sweep on each one.
        if aligned_start_chip < self.foreign_signal_until_chip {
            return SpawnOutcome::Skip;
        }
        // The probes integrate the full 3-frame preamble plus slack. Defer
        // hits that outrun the buffered samples rather than mis-score them.
        const PROBE_SLACK_CHIPS: i64 = 64;
        let probe_end_sample =
            (aligned_start_chip + (3 * ACCESS_PACKET_CHIPS) as i64 + PROBE_SLACK_CHIPS)
                .saturating_mul(self.cfg.oversample as i64);
        if probe_end_sample > buffer_abs_signed + buffer.len() as i64 {
            return SpawnOutcome::Defer;
        }
        // 2. Noise gate.
        let mut pregate_coherence = 0.0f32;
        for frame_back in 0..=1i64 {
            let start_chip = aligned_start_chip - frame_back * ACCESS_PACKET_CHIPS as i64;
            if let Some((coherence, _)) = preamble_lag_coherence(
                buffer,
                ChipGeometry::new(buffer_abs_signed, self.cfg.oversample),
                start_chip,
            ) {
                pregate_coherence = pregate_coherence.max(coherence);
            }
        }
        if pregate_coherence < ACCESS_PREAMBLE_MIN_LAG_COHERENCE {
            log::trace!(
                "HRPD access frame FFT rake: noise-gated hit snr={:.1}x aligned_start_chip={} pregate_lag_coh={:.3}",
                hit.snr,
                aligned_start_chip,
                pregate_coherence,
            );
            return SpawnOutcome::Skip;
        }
        let timings = timing_candidates_near_fft_hit(
            buffer,
            buffer_abs_signed,
            self.cfg.oversample,
            aligned_start_chip,
            self.cfg.sector_id_lsb,
            self.cfg.color_code,
            self.cfg.reference_chip_offset,
            self.cfg.q_pair_phase,
            self.cfg.q_sign,
        );
        let Some(timing) = timings
            .iter()
            .find(|candidate| {
                candidate.lag_coherence >= ACCESS_PREAMBLE_MIN_LAG_COHERENCE
                    && candidate.spec_coherence >= ACCESS_PREAMBLE_MIN_SPEC_COHERENCE
            })
            .cloned()
        else {
            let best = timings.iter().max_by(|a, b| {
                (a.lag_coherence + a.spec_coherence)
                    .total_cmp(&(b.lag_coherence + b.spec_coherence))
            });
            log::trace!(
                "HRPD access frame FFT rake: skipping hit snr={:.1}x aligned_start_chip={} candidates={} best_lag_coh={:.3} best_spec_coh={:.3}",
                hit.snr,
                aligned_start_chip,
                timings.len(),
                best.map(|t| t.lag_coherence).unwrap_or(0.0),
                best.map(|t| t.spec_coherence).unwrap_or(0.0),
            );
            // Frame-periodic but not our preamble: a foreign signal.
            // Suppress further hits for a couple of frames.
            if pregate_coherence >= ACCESS_PREAMBLE_MIN_LAG_COHERENCE {
                self.foreign_signal_until_chip =
                    aligned_start_chip + (2 * ACCESS_PACKET_CHIPS) as i64;
            }
            return SpawnOutcome::Skip;
        };
        let packet_start_chip = timing.preamble_start_chip + (3 * ACCESS_PACKET_CHIPS) as i64;
        let access_cycle_number =
            derived_access_cycle_number_for_chip(timing.preamble_start_chip as u64);
        if self.is_duplicate_packet(packet_start_chip) {
            return SpawnOutcome::Skip;
        }

        let preamble_start_sample = timings
            .iter()
            .map(|candidate| {
                candidate.preamble_start_chip * self.cfg.oversample as i64
                    + i64::from(candidate.sample_delay)
                    + candidate.sample_delay_fraction.floor() as i64
            })
            .min()
            .unwrap_or_else(|| {
                timing.preamble_start_chip * self.cfg.oversample as i64
                    + i64::from(timing.sample_delay)
            });
        let replay_start_sample = preamble_start_sample.max(buffer_abs_signed);
        let replay_offset = (replay_start_sample - buffer_abs_signed) as usize;
        let replay = buffer[replay_offset..].to_vec();
        let replay_end_sample = buffer_abs_signed + buffer.len() as i64;
        let id = self.next_finger_id;
        self.next_finger_id += 1;
        info!(
            "HRPD access frame FFT rake: spawning preamble finger id={} snr={:.2}x/{:.2}dB preamble_start={} packet_start={} acn={} sample_delay={}{:+.2} lag_coh={:.3} spec_coh={:.3}",
            id,
            hit.snr,
            hit.snr_db,
            timing.preamble_start_chip,
            packet_start_chip,
            access_cycle_number,
            timing.sample_delay,
            timing.sample_delay_fraction,
            timing.lag_coherence,
            timing.spec_coherence,
        );
        self.remember_packet(packet_start_chip);
        self.active_fingers.push(HrpdAccessFrameFingerState {
            id,
            packet_start_chip,
            hard_validated: false,
            idle_chips: 0,
        });
        let finger = HrpdAccessFrameFinger::new(
            id,
            self.cfg.oversample,
            vec![timing],
            hit.snr,
            access_cycle_number,
            self.cfg.sector_id_lsb,
            self.cfg.color_code,
            self.cfg.preamble_frames,
            self.cfg.reference_chip_offset,
            self.cfg.q_pair_phase,
            self.cfg.q_sign,
            self.cfg.pn_phase_offset_chips,
            self.cfg.lc_phase_offset_chips,
            self.cfg.decode,
            self.sample_rate_hz,
            replay,
            replay_start_sample,
            replay_end_sample,
        );
        SpawnOutcome::Spawn(
            finger,
            vec![Box::new(HrpdAccessPacketProcessor::with_decode_config(
                self.cfg.decode,
            )) as PipelineProcessorShared],
        )
    }

    fn buffer_trim_count_after_scan(
        &self,
        buffer_len: usize,
        next_scan_offset: usize,
        window_samples: usize,
    ) -> usize {
        // Keep enough history for the previous-frame timing probes.
        let oversample = self.cfg.oversample.max(1);
        let head_room = (2 * ACCESS_PACKET_CHIPS + 2048) * oversample + 128;
        let keep_high_water = window_samples * 6;
        if buffer_len > keep_high_water && next_scan_offset > head_room {
            (next_scan_offset - head_room).min(buffer_len)
        } else {
            0
        }
    }

    fn notify_hard_validated(&mut self, finger_id: u64) {
        if let Some(state) = self
            .active_fingers
            .iter_mut()
            .find(|state| state.id == finger_id)
        {
            state.hard_validated = true;
            state.idle_chips = 0;
        }
    }

    fn notify_finger_state(
        &mut self,
        finger_id: u64,
        hard_validated: bool,
        idle_chips: u64,
        _signal_lost_chips: u64,
        _crc_miss_count: u64,
        _post_walsh_no_event_ms: u64,
    ) {
        if let Some(state) = self
            .active_fingers
            .iter_mut()
            .find(|state| state.id == finger_id)
        {
            state.hard_validated = hard_validated;
            state.idle_chips = idle_chips;
        }
    }

    fn notify_finger_removed(&mut self, finger_id: u64) {
        self.active_fingers.retain(|state| state.id != finger_id);
    }
}

impl Correlator for HrpdAccessFrameRakeCorrelator {
    type Finger = HrpdAccessFrameFinger;

    fn correlate(
        &mut self,
        block: &SampleBlock,
    ) -> Vec<(Self::Finger, Vec<PipelineProcessorShared>)> {
        self.base.correlate(block)
    }

    fn search_suppressed(&self) -> bool {
        self.base.search_suppressed()
    }

    fn notify_hard_validated(&mut self, finger_id: u64) {
        self.base.notify_hard_validated(finger_id);
    }

    fn notify_finger_state(
        &mut self,
        finger_id: u64,
        hard_validated: bool,
        idle_chips: u64,
        signal_lost_chips: u64,
        crc_miss_count: u64,
        post_walsh_no_event_ms: u64,
    ) {
        self.base.notify_finger_state(
            finger_id,
            hard_validated,
            idle_chips,
            signal_lost_chips,
            crc_miss_count,
            post_walsh_no_event_ms,
        );
    }

    fn notify_finger_removed(&mut self, finger_id: u64) {
        self.base.notify_finger_removed(finger_id);
    }
}

impl HrpdAccessFrameFinger {
    #[allow(clippy::too_many_arguments)]
    fn new(
        id: u64,
        oversample: usize,
        timing_candidates: Vec<HrpdAccessPreambleTiming>,
        detection_snr: f32,
        access_cycle_number: u8,
        sector_id_lsb: u32,
        color_code: u8,
        preamble_frames: usize,
        reference_chip_offset: i32,
        q_pair_phase: u64,
        q_sign: f32,
        pn_phase_offset_chips: i32,
        lc_phase_offset_chips: i32,
        decode: HrpdAccessDecodeConfig,
        sample_rate_hz: f64,
        replay: Vec<Complex32>,
        replay_abs_sample: i64,
        ignore_until_abs_sample: i64,
    ) -> Self {
        let preamble_start_chip = timing_candidates
            .first()
            .map(|candidate| candidate.preamble_start_chip)
            .unwrap_or(0);
        let lag_coherence = timing_candidates
            .first()
            .map(|candidate| candidate.lag_coherence)
            .unwrap_or(0.0);
        Self {
            base: BaseFinger::new(id),
            oversample,
            preamble_start_chip,
            timing_candidates,
            preamble_frames,
            detection_snr,
            lag_coherence,
            access_cycle_number,
            sector_id_lsb,
            color_code,
            reference_chip_offset,
            q_pair_phase,
            q_sign,
            pn_phase_offset_chips,
            lc_phase_offset_chips,
            decode,
            sample_rate_hz,
            buffer: replay,
            buffer_abs_sample: Some(replay_abs_sample),
            ignore_until_abs_sample,
            emitted_packet: false,
        }
    }

    fn append_live_block(&mut self, block: &SampleBlock) {
        if block.sample_rate_hz > 0.0 {
            self.sample_rate_hz = block.sample_rate_hz;
        }
        let mut block_abs = block
            .tags
            .get("absolute_sample_start")
            .copied()
            .unwrap_or(block.chip_start as i64);
        let mut samples = block.samples.as_slice();
        let block_end = block_abs + samples.len() as i64;
        if block_end <= self.ignore_until_abs_sample {
            return;
        }
        if block_abs < self.ignore_until_abs_sample {
            let skip = (self.ignore_until_abs_sample - block_abs) as usize;
            samples = &samples[skip..];
            block_abs = self.ignore_until_abs_sample;
        }
        if let Some(buffer_abs) = self.buffer_abs_sample {
            let expected = buffer_abs + self.buffer.len() as i64;
            if expected != block_abs {
                debug!(
                    "HRPD access frame finger {}: sample discontinuity expected={} got={}, resetting local buffer",
                    self.base.id, expected, block_abs
                );
                self.buffer.clear();
                self.buffer_abs_sample = Some(block_abs);
            }
        } else {
            self.buffer_abs_sample = Some(block_abs);
        }
        self.buffer.extend_from_slice(samples);
    }

    fn try_emit_packet(&mut self, _chain: &mut Vec<PipelineProcessorShared>) -> Vec<SampleBlock> {
        if self.emitted_packet {
            return Vec::new();
        }
        let Some(buffer_abs_sample) = self.buffer_abs_sample else {
            return Vec::new();
        };
        let mut waiting_for_candidate_samples = false;
        let mut decode_attempts = 0usize;
        let mut best_fcs_bit_errors = u32::MAX;
        let mut best_tail_ones = usize::MAX;
        'timings: for (timing_idx, timing) in self.timing_candidates.iter().enumerate() {
            let packet_start_chip =
                timing.preamble_start_chip + (self.preamble_frames * ACCESS_PACKET_CHIPS) as i64;
            if !access_packet_samples_available(
                &self.buffer,
                buffer_abs_sample,
                self.oversample,
                timing,
                self.preamble_frames,
            ) {
                waiting_for_candidate_samples = true;
                continue;
            }
            for mode in HrpdAccessPacketDespreadMode::ALL {
                let Some(mut chips) = extract_spec_despread_access_packet_chips(
                    &self.buffer,
                    ChipGeometry::new(buffer_abs_sample, self.oversample)
                        .at_delay(timing.sample_delay, timing.sample_delay_fraction),
                    timing.preamble_start_chip,
                    self.preamble_frames,
                    derived_access_cycle_number_for_chip(timing.preamble_start_chip as u64),
                    self.sector_id_lsb,
                    self.color_code,
                    self.reference_chip_offset,
                    self.q_pair_phase,
                    self.q_sign,
                    self.pn_phase_offset_chips,
                    self.lc_phase_offset_chips,
                    mode,
                    timing.phase_step,
                ) else {
                    continue;
                };
                // A gated (3-on/1-off) reprobe's dead slots are noise; zero
                // them so the PHY decoder treats them as erasures. No-op for a
                // continuous probe.
                erase_gated_slots(&mut chips);

                decode_attempts += 1;
                // One decode pass yields both the accept decision and the
                // closest-to-valid diagnostics (FCS bit errors) a failing
                // finger reports instead of a bare "no candidates".
                let (attempt, decoded) = decode_access_phy_chips_with_attempt(&chips, self.decode);
                if let Some(a) = attempt
                    && a.fcs_bit_errors < best_fcs_bit_errors
                {
                    best_fcs_bit_errors = a.fcs_bit_errors;
                    best_tail_ones = a.tail_ones;
                }
                let Some(decoded) = decoded else {
                    continue;
                };
                let mac_check = validate_access_mac_fragment(&decoded.info_bits);
                if !mac_check.valid {
                    continue;
                }
                // A capsule longer than one MAC fragment continues in the
                // next 16-slot frame(s) of the same burst. Decode the
                // remaining fragments and reassemble; the synthetic packet
                // flows through the single-fragment path downstream.
                let required_fragments = mac_check.required_fragments.unwrap_or(1).max(1);
                let (decoded, capsule_fragments) = if required_fragments <= 1 {
                    (decoded, 1usize)
                } else {
                    if !access_packet_samples_available(
                        &self.buffer,
                        buffer_abs_sample,
                        self.oversample,
                        timing,
                        self.preamble_frames + required_fragments - 1,
                    ) {
                        waiting_for_candidate_samples = true;
                        break 'timings;
                    }
                    let mut fragment_infos: Vec<Vec<u8>> = vec![decoded.info_bits.clone()];
                    for fragment_idx in 1..required_fragments {
                        let Some(mut fragment_chips) = extract_spec_despread_access_packet_chips(
                            &self.buffer,
                            ChipGeometry::new(buffer_abs_sample, self.oversample)
                                .at_delay(timing.sample_delay, timing.sample_delay_fraction),
                            timing.preamble_start_chip,
                            self.preamble_frames + fragment_idx,
                            derived_access_cycle_number_for_chip(timing.preamble_start_chip as u64),
                            self.sector_id_lsb,
                            self.color_code,
                            self.reference_chip_offset,
                            self.q_pair_phase,
                            self.q_sign,
                            self.pn_phase_offset_chips,
                            self.lc_phase_offset_chips,
                            mode,
                            timing.phase_step,
                        ) else {
                            break;
                        };
                        erase_gated_slots(&mut fragment_chips);
                        decode_attempts += 1;
                        let Some(fragment) =
                            decode_access_phy_chips_with_config(&fragment_chips, self.decode)
                        else {
                            break;
                        };
                        fragment_infos.push(fragment.info_bits);
                    }
                    if fragment_infos.len() != required_fragments {
                        continue;
                    }
                    let fragment_refs: Vec<&[u8]> =
                        fragment_infos.iter().map(|bits| bits.as_slice()).collect();
                    let Some(packet) = reassemble_access_mac_capsule_packet(&fragment_refs) else {
                        continue;
                    };
                    (packet, required_fragments)
                };

                let mut source_tags = HashMap::new();
                source_tags.insert("finger_id", self.base.id as i64);
                source_tags.insert("access_preamble_detected", 1);
                source_tags.insert("access_preamble_frames", self.preamble_frames as i64);
                source_tags.insert(
                    "hrpd_access_preamble_start_chip",
                    timing.preamble_start_chip,
                );
                source_tags.insert(
                    "hrpd_access_preamble_sample_delay",
                    i64::from(timing.sample_delay),
                );
                source_tags.insert(
                    "hrpd_access_preamble_sample_delay_frac_milli",
                    (timing.sample_delay_fraction * 1000.0).round() as i64,
                );
                source_tags.insert(
                    "hrpd_access_preamble_lag_coherence_milli",
                    (timing.lag_coherence * 1000.0).round() as i64,
                );
                source_tags.insert(
                    "finger_snr_mdb",
                    (10.0 * self.detection_snr.max(1.0e-12).log10() * 1000.0).round() as i64,
                );
                source_tags.insert(
                    "hrpd_access_cycle_number",
                    i64::from(self.access_cycle_number),
                );
                source_tags.insert(
                    "hrpd_access_mac_capsule_fragments",
                    capsule_fragments as i64,
                );
                let out = vec![access_event_block(
                    packet_start_chip,
                    &decoded,
                    ACCESS_CHIP_RATE as f64,
                    &source_tags,
                    [(
                        "hrpd_access_packet_phase_chips",
                        packet_start_chip.rem_euclid(ACCESS_PACKET_CHIPS as i64),
                    )],
                )];
                {
                    info!(
                        "HRPD access frame finger {}: decoded packet_start={} mode={} fragments={} timing_idx={}/{} decode_attempts={} sample_delay={}{:+.2} lag_coh={:.3} spec_coh={:.3}",
                        self.base.id,
                        packet_start_chip,
                        mode.label(),
                        capsule_fragments,
                        timing_idx + 1,
                        self.timing_candidates.len(),
                        decode_attempts,
                        timing.sample_delay,
                        timing.sample_delay_fraction,
                        timing.lag_coherence,
                        timing.spec_coherence,
                    );
                    self.emitted_packet = true;
                    self.base
                        .tick_and_validate(&out, ACCESS_PACKET_CHIPS as u64);
                    return out;
                }
            }
        }
        if waiting_for_candidate_samples {
            return Vec::new();
        }
        info!(
            "HRPD access frame finger {}: no downstream-valid packet preamble_start={} acn={} lag_coh={:.3} snr={:.1}x decode={} candidates={} decode_attempts={} best_fcs_bit_errors={} best_tail_ones={}",
            self.base.id,
            self.preamble_start_chip,
            self.access_cycle_number,
            self.lag_coherence,
            self.detection_snr,
            if self.decode.enhanced_rates {
                "enhanced"
            } else {
                "rev0"
            },
            self.timing_candidates.len(),
            decode_attempts,
            if best_fcs_bit_errors == u32::MAX {
                -1
            } else {
                best_fcs_bit_errors as i64
            },
            if best_tail_ones == usize::MAX {
                -1
            } else {
                best_tail_ones as i64
            },
        );
        self.emitted_packet = true;
        self.base.tick_and_validate(&[], ACCESS_PACKET_CHIPS as u64);
        Vec::new()
    }
}

impl RakeFinger for HrpdAccessFrameFinger {
    fn id(&self) -> u64 {
        self.base.id
    }

    fn spawn_chip_start(&self) -> Option<u64> {
        u64::try_from(self.preamble_start_chip).ok()
    }

    fn describe(&self) -> String {
        format!(
            "hrpd_access_fft snr={:.1}x acn={} preamble_start={} sample_delay={}{:+.2} lag_coh={:.3}",
            self.detection_snr,
            self.access_cycle_number,
            self.preamble_start_chip,
            self.timing_candidates
                .first()
                .map(|candidate| candidate.sample_delay)
                .unwrap_or(0),
            self.timing_candidates
                .first()
                .map(|candidate| candidate.sample_delay_fraction)
                .unwrap_or(0.0),
            self.lag_coherence,
        )
    }

    fn process(
        &mut self,
        block: &SampleBlock,
        chain: &mut Vec<PipelineProcessorShared>,
    ) -> Vec<SampleBlock> {
        self.append_live_block(block);
        let out = self.try_emit_packet(chain);
        if out.is_empty() {
            let chips = (block.samples.len() / self.oversample.max(1)) as u64;
            self.base.tick_and_validate(&[], chips);
        }
        out
    }

    fn flush(&mut self, chain: &mut Vec<PipelineProcessorShared>) -> Vec<SampleBlock> {
        let mut out = self.try_emit_packet(chain);
        out.extend(BaseFinger::flush_chain(chain));
        out
    }

    fn is_hard_validated(&self) -> bool {
        self.base.is_hard_validated()
    }

    fn idle_blocks(&self) -> u64 {
        self.base.idle_blocks()
    }

    fn idle_chips(&self) -> u64 {
        self.base.idle_chips()
    }

    fn crc_miss_count(&self) -> u64 {
        self.base.crc_miss_count()
    }

    fn post_walsh_no_event_chips(&self) -> u64 {
        self.base.post_walsh_no_event_chips()
    }

    fn post_walsh_miss_count(&self) -> u64 {
        self.base.post_walsh_miss_count()
    }

    fn post_walsh_no_event_ms(&self) -> u64 {
        self.base.post_walsh_no_event_ms()
    }
}

impl HrpdAccessFrameCorrelator {
    pub fn new(cfg: HrpdAccessFrameFftConfig) -> Self {
        let searcher = HrpdReverseFftPilotSearcher::new(HrpdReverseFftPilotSearchConfig {
            oversample: cfg.oversample,
            frame_chips: ACCESS_PACKET_CHIPS,
            search_window_frames: cfg.search_window_frames,
            search_step_frames: cfg.search_step_frames,
            snr_threshold: cfg.snr_threshold,
            max_hits_per_window: cfg.max_fft_hits_per_window,
            hit_suppression_chips: cfg.fft_hit_suppression_chips,
        });
        let reference = HrpdAccessFftPilotReference::from_config(&cfg);
        Self {
            cfg,
            searcher,
            reference,
        }
    }

    pub fn scan_top_hits(
        &mut self,
        samples: &[Complex32],
        absolute_sample_start: u64,
        top_n: usize,
    ) -> Vec<HrpdAccessFrameFftHit> {
        let raw_hits =
            self.searcher
                .scan_top_hits(samples, absolute_sample_start, top_n, &self.reference);
        let hits: Vec<HrpdAccessFrameFftHit> = raw_hits
            .into_iter()
            .map(|hit| self.attach_access_metadata(hit))
            .collect();
        if let Some(best) = hits.first() {
            log::trace!(
                "HRPD access frame FFT: best_snr={:.2}x/{:.2}dB threshold={:.2}x peak={:.3e} mean={:.3e} window={} delay_samples={} start_sample={} start_chip={} sample_phase={} frame_phase={} preamble_acn={}",
                best.snr,
                best.snr_db,
                self.cfg.snr_threshold,
                best.peak_power,
                best.mean_power,
                best.window_index,
                best.delay_samples,
                best.preamble_start_sample,
                best.preamble_start_chip,
                best.sample_phase,
                best.frame_phase_chips,
                best.access_cycle_number,
            );
        }
        hits
    }

    fn attach_access_metadata(&self, hit: HrpdReverseFftPilotHit) -> HrpdAccessFrameFftHit {
        let frame_delta_chips =
            (ACCESS_PACKET_CHIPS as u64 - hit.frame_phase_chips) % ACCESS_PACKET_CHIPS as u64;
        let aligned_preamble_start_chip = hit.preamble_start_chip + frame_delta_chips;
        let access_cycle_number = self
            .reference
            .access_cycle_number_for_chip(aligned_preamble_start_chip);
        HrpdAccessFrameFftHit {
            snr: hit.snr,
            snr_db: hit.snr_db,
            peak_power: hit.peak_power,
            mean_power: hit.mean_power,
            window_index: hit.window_index,
            window_sample_offset: hit.window_sample_offset,
            delay_samples: hit.delay_samples,
            preamble_start_sample: hit.preamble_start_sample,
            preamble_start_chip: hit.preamble_start_chip,
            sample_phase: hit.sample_phase,
            frame_phase_chips: hit.frame_phase_chips,
            access_cycle_number,
        }
    }

    pub fn despread_aligned_preamble_coherence_with_chip_offset(
        &self,
        samples: &[Complex32],
        absolute_sample_start: u64,
        hit: &HrpdAccessFrameFftHit,
        chip_offset: i64,
        access_cycle_number: u8,
        sample_delay_radius: i32,
    ) -> Option<HrpdAccessPreambleCoherence> {
        let frame_delta_chips =
            (ACCESS_PACKET_CHIPS as u64 - hit.frame_phase_chips) % ACCESS_PACKET_CHIPS as u64;
        let frame_start_chip = hit
            .preamble_start_chip
            .checked_add(frame_delta_chips)?
            .checked_add_signed(chip_offset)?;
        let frame_start_sample = frame_start_chip * self.cfg.oversample as u64;

        let mut best: Option<HrpdAccessPreambleCoherence> = None;
        let radius = sample_delay_radius.abs();
        for sample_delay in -radius..=radius {
            if let Some(candidate) = self.despread_preamble_coherence_at(
                samples,
                absolute_sample_start,
                frame_start_sample,
                frame_start_chip,
                sample_delay,
                access_cycle_number,
            ) {
                if best.as_ref().is_none_or(|prev| {
                    candidate.coherent_ratio > prev.coherent_ratio
                        || (candidate.coherent_ratio == prev.coherent_ratio
                            && candidate.sign_agreement > prev.sign_agreement)
                }) {
                    best = Some(candidate);
                }
            }
        }
        best
    }

    fn despread_preamble_coherence_at(
        &self,
        samples: &[Complex32],
        absolute_sample_start: u64,
        preamble_start_sample: u64,
        preamble_start_chip: u64,
        sample_delay_samples: i32,
        access_cycle_number: u8,
    ) -> Option<HrpdAccessPreambleCoherence> {
        let delayed_start_sample = preamble_start_sample as i64 + sample_delay_samples as i64;
        if delayed_start_sample < absolute_sample_start as i64 {
            return None;
        }
        let start = (delayed_start_sample as u64 - absolute_sample_start) as usize;
        if start + (ACCESS_PACKET_CHIPS - 1) * self.cfg.oversample >= samples.len() {
            return None;
        }

        let reference = hrpd_access_preamble_chips(
            preamble_start_chip,
            access_cycle_number,
            self.cfg.sector_id_lsb,
            self.cfg.color_code,
            self.cfg.reference_chip_offset,
            self.cfg.q_pair_phase,
            self.cfg.q_sign,
            ACCESS_PACKET_CHIPS,
        );
        let mut despread = Vec::with_capacity(ACCESS_PACKET_CHIPS);
        let mut coh = Complex32::new(0.0, 0.0);
        let mut abs_sum = 0.0f32;
        let mut power_sum = 0.0f32;

        for (chip_idx, reference_chip) in reference.iter().enumerate() {
            let sample = samples[start + chip_idx * self.cfg.oversample];
            let value = sample * reference_chip.conj();
            coh += value;
            abs_sum += value.norm();
            power_sum += value.norm_sqr();
            despread.push(value);
        }

        if !(abs_sum > 0.0 && power_sum > 0.0) {
            return None;
        }
        let dir = if coh.norm_sqr() > 0.0 {
            coh.conj() / coh.norm()
        } else {
            Complex32::new(1.0, 0.0)
        };
        let mut sign_ok = 0usize;
        let mut rot_sum = Complex32::new(0.0, 0.0);
        for value in &despread {
            let rotated = *value * dir;
            if rotated.re >= 0.0 {
                sign_ok += 1;
            }
            rot_sum += rotated;
        }

        let chips = despread.len();
        Some(HrpdAccessPreambleCoherence {
            chips,
            preamble_start_sample,
            preamble_start_chip,
            sample_delay_samples,
            access_cycle_number,
            coherent_sum: coh,
            coherent_mean: coh / chips as f32,
            coherent_ratio: coh.norm() / abs_sum,
            rotated_i_mean: rot_sum.re / chips as f32,
            rotated_q_mean: rot_sum.im / chips as f32,
            rms: (power_sum / chips as f32).sqrt(),
            sign_agreement: sign_ok as f32 / chips as f32,
        })
    }

    pub fn snr_threshold(&self) -> f32 {
        self.cfg.snr_threshold
    }

    pub fn frame_samples(&self) -> usize {
        self.searcher.frame_samples()
    }

    pub fn window_samples(&self) -> usize {
        self.searcher.window_samples()
    }

    pub fn fft_len(&self) -> usize {
        self.searcher.fft_len()
    }

    pub fn preamble_reference_chips_len(
        &self,
        preamble_start_chip: u64,
        access_cycle_number: u8,
        len: usize,
    ) -> Vec<Complex32> {
        hrpd_access_preamble_chips(
            preamble_start_chip,
            access_cycle_number,
            self.cfg.sector_id_lsb,
            self.cfg.color_code,
            self.cfg.reference_chip_offset,
            self.cfg.q_pair_phase,
            self.cfg.q_sign,
            len,
        )
    }
}

pub fn derived_access_cycle_number_for_chip(chip: u64) -> u8 {
    ((chip / 2048) & 0xff) as u8
}

fn hrpd_access_preamble_chips(
    start_chip: u64,
    access_cycle_number: u8,
    sector_id_lsb: u32,
    color_code: u8,
    reference_chip_offset: i32,
    q_pair_phase: u64,
    q_sign: f32,
    len: usize,
) -> Vec<Complex32> {
    let i_mask = HrpdAccessLongCodeMask {
        access_cycle_number,
        sector_id_lsb,
        color_code,
    }
    .to_mask();
    let q_mask = derive_q_mask(i_mask);
    hrpd_reverse_pilot_reference_chips(HrpdReversePilotReferenceConfig {
        start_chip,
        len,
        i_mask,
        q_mask,
        reference_chip_offset,
        pn_phase_offset_chips: 0,
        lc_phase_offset_chips: 0,
        q_sign,
        q_pair_phase,
    })
}

fn align_hit_to_frame_start(hit: &HrpdReverseFftPilotHit) -> i64 {
    let frame_delta_chips =
        (ACCESS_PACKET_CHIPS as u64 - hit.frame_phase_chips) % ACCESS_PACKET_CHIPS as u64;
    (hit.preamble_start_chip + frame_delta_chips) as i64
}

#[allow(clippy::too_many_arguments)]
fn timing_candidates_near_fft_hit(
    samples: &[Complex32],
    absolute_sample_start: i64,
    oversample: usize,
    aligned_start_chip: i64,
    sector_id_lsb: u32,
    color_code: u8,
    reference_chip_offset: i32,
    q_pair_phase: u64,
    q_sign: f32,
) -> Vec<HrpdAccessPreambleTiming> {
    let frame = ACCESS_PACKET_CHIPS as i64;
    let sample_delays = [
        -24, -28, -20, -32, -16, -36, -12, -40, -8, -4, 0, 4, 8, 12, 16, 20, 24, 32, 40, 48, 56,
        64, 72, 84, 96, 108, 120,
    ];
    let grid = ChipGeometry::new(absolute_sample_start, oversample);
    let mut coarse = Vec::new();
    // Narrowed search space (2×3×27 = 162 candidates): frame_back=1 catches
    // an IFFT peak that rounded into the next frame, and ±1 slot covers the
    // few-chip uncertainty in the peak position.
    for frame_back in 0..=1 {
        for slot_offset in -1..=1 {
            let start_chip = aligned_start_chip - frame_back * frame + slot_offset * 2048;
            if start_chip < 0 {
                continue;
            }
            for sample_delay in sample_delays {
                let Some((lag_coherence, phase_step)) =
                    preamble_lag_coherence(samples, grid.at_delay(sample_delay, 0.0), start_chip)
                else {
                    continue;
                };
                if lag_coherence >= ACCESS_PREAMBLE_MIN_LAG_COHERENCE {
                    coarse.push(HrpdAccessPreambleTiming {
                        preamble_start_chip: start_chip,
                        sample_delay,
                        sample_delay_fraction: 0.0,
                        lag_coherence,
                        spec_coherence: 0.0,
                        phase_step,
                    });
                }
            }
        }
    }
    coarse.sort_by(|a, b| {
        b.lag_coherence
            .total_cmp(&a.lag_coherence)
            .then_with(|| b.preamble_start_chip.cmp(&a.preamble_start_chip))
            .then_with(|| a.sample_delay.cmp(&b.sample_delay))
    });
    coarse.truncate(48);

    // Reuse the spec reference across delay variants for the same frame start.
    let mut references = HashMap::<i64, Vec<Complex32>>::new();
    let mut all = Vec::new();
    for coarse in coarse {
        let reference = references
            .entry(coarse.preamble_start_chip)
            .or_insert_with(|| {
                let acn = derived_access_cycle_number_for_chip(coarse.preamble_start_chip as u64);
                hrpd_access_preamble_chips(
                    coarse.preamble_start_chip as u64,
                    acn,
                    sector_id_lsb,
                    color_code,
                    reference_chip_offset,
                    q_pair_phase,
                    q_sign,
                    ACCESS_PACKET_CHIPS,
                )
            });
        for sample_delay in [
            coarse.sample_delay - 4,
            coarse.sample_delay,
            coarse.sample_delay + 4,
        ] {
            for sample_delay_fraction in [0.0f32, -0.75, 0.75] {
                let geometry = grid.at_delay(sample_delay, sample_delay_fraction);
                let Some((lag_coherence, phase_step)) =
                    preamble_lag_coherence(samples, geometry, coarse.preamble_start_chip)
                else {
                    continue;
                };
                if lag_coherence < ACCESS_PREAMBLE_MIN_LAG_COHERENCE {
                    continue;
                }
                let spec_coherence = preamble_spec_coherence_with_reference(
                    samples,
                    geometry,
                    coarse.preamble_start_chip,
                    reference,
                )
                .unwrap_or(0.0);
                if spec_coherence < ACCESS_PREAMBLE_MIN_SPEC_COHERENCE {
                    continue;
                }
                all.push(HrpdAccessPreambleTiming {
                    preamble_start_chip: coarse.preamble_start_chip,
                    sample_delay,
                    sample_delay_fraction,
                    lag_coherence,
                    spec_coherence,
                    phase_step,
                });
            }
        }
    }
    all.sort_by(|a, b| {
        a.preamble_start_chip
            .cmp(&b.preamble_start_chip)
            .then_with(|| a.sample_delay.cmp(&b.sample_delay))
            .then_with(|| a.sample_delay_fraction.total_cmp(&b.sample_delay_fraction))
    });
    all.dedup_by(|a, b| {
        a.preamble_start_chip == b.preamble_start_chip
            && a.sample_delay == b.sample_delay
            && (a.sample_delay_fraction - b.sample_delay_fraction).abs() < f32::EPSILON
    });
    if all.is_empty() {
        return Vec::new();
    }
    all.sort_by(|a, b| {
        b.spec_coherence
            .total_cmp(&a.spec_coherence)
            .then_with(|| b.lag_coherence.total_cmp(&a.lag_coherence))
            .then_with(|| b.preamble_start_chip.cmp(&a.preamble_start_chip))
            .then_with(|| {
                a.sample_delay_fraction
                    .abs()
                    .total_cmp(&b.sample_delay_fraction.abs())
            })
            .then_with(|| a.sample_delay.cmp(&b.sample_delay))
    });
    all.truncate(96);
    all
}

/// Slots per 16-slot access frame, used to bin the coherence probes so a
/// 3-on/1-off reverse gating cadence can be detected and excluded.
const ACCESS_FRAME_SLOTS: usize = 16;
/// Chips per HRPD slot.
const HRPD_SLOT_CHIPS: usize = ACCESS_PACKET_CHIPS / ACCESS_FRAME_SLOTS;
/// A slot whose frame-0 power is below this fraction of the median slot
/// power is a gated (dead) slot, not signal.
const GATED_SLOT_POWER_FRACTION: f32 = 0.25;

/// Given per-slot frame-0 power, return the set of active (non-gated) slot
/// indices when a 3-on/1-off cadence is present (1..=4 slots far below the
/// median), else `None` (no gating — use the full frame).
fn active_slots_from_power(pow0: &[f32; ACCESS_FRAME_SLOTS]) -> Option<Vec<usize>> {
    let mut sorted = *pow0;
    sorted.sort_by(f32::total_cmp);
    let median = sorted[ACCESS_FRAME_SLOTS / 2];
    if median <= 0.0 {
        return None;
    }
    let threshold = GATED_SLOT_POWER_FRACTION * median;
    let active: Vec<usize> = (0..ACCESS_FRAME_SLOTS)
        .filter(|&s| pow0[s] >= threshold)
        .collect();
    let dead = ACCESS_FRAME_SLOTS - active.len();
    // A real 3-on/1-off cadence gates one slot in four (four dead in a
    // 16-slot frame); accept 1..=4 dead so a partially-buffered frame still
    // registers, but not so many that we are just cutting noise.
    if (1..=4).contains(&dead) {
        Some(active)
    } else {
        None
    }
}

/// Zero the gated (dead) slots of a despread access frame in place when a
/// 3-on/1-off cadence is present, turning them into decoder erasures. The
/// access PHY decoder tolerates the resulting 25% erasure. No-op for a
/// continuous (non-gated) frame, so the common path is untouched.
fn erase_gated_slots(chips: &mut [Complex32]) {
    if chips.len() < ACCESS_PACKET_CHIPS {
        return;
    }
    // Gated slots sit far below active ones, so a strided power estimate
    // detects the cadence cheaply. Only the zeroing needs every chip.
    const DETECT_STRIDE: usize = 8;
    let mut pow = [0.0f32; ACCESS_FRAME_SLOTS];
    for (slot, p) in pow.iter_mut().enumerate() {
        let base = slot * HRPD_SLOT_CHIPS;
        *p = chips[base..base + HRPD_SLOT_CHIPS]
            .iter()
            .step_by(DETECT_STRIDE)
            .map(|c| c.norm_sqr())
            .sum();
    }
    let Some(active) = active_slots_from_power(&pow) else {
        return;
    };
    for slot in (0..ACCESS_FRAME_SLOTS).filter(|s| !active.contains(s)) {
        let base = slot * HRPD_SLOT_CHIPS;
        for c in &mut chips[base..base + HRPD_SLOT_CHIPS] {
            *c = Complex32::new(0.0, 0.0);
        }
    }
}

/// Frame-to-frame preamble self-similarity, robust to 3-on/1-off reverse
/// gating: returns the better of the full-frame coherence and the
/// coherence over only the active slots (so a gated preamble, whose dead
/// slots otherwise dilute the metric, is scored on its live slots).
fn preamble_lag_coherence(
    samples: &[Complex32],
    geometry: ChipGeometry,
    preamble_start_chip: i64,
) -> Option<(f32, f32)> {
    let stride = 64usize;
    let mut dot01 = [Complex32::new(0.0, 0.0); ACCESS_FRAME_SLOTS];
    let mut dot02 = [Complex32::new(0.0, 0.0); ACCESS_FRAME_SLOTS];
    let mut dot12 = [Complex32::new(0.0, 0.0); ACCESS_FRAME_SLOTS];
    let mut pow0 = [0.0f32; ACCESS_FRAME_SLOTS];
    let mut pow1 = [0.0f32; ACCESS_FRAME_SLOTS];
    let mut pow2 = [0.0f32; ACCESS_FRAME_SLOTS];
    let last_k = (ACCESS_PACKET_CHIPS - 1) / stride * stride;
    let frame_samples = (ACCESS_PACKET_CHIPS * geometry.oversample) as i64;
    let last_chip = preamble_start_chip + (2 * ACCESS_PACKET_CHIPS + last_k) as i64;
    let scan = geometry.scan(samples, preamble_start_chip, last_chip, stride)?;

    let mut index = scan.index;
    for k in (0..ACCESS_PACKET_CHIPS).step_by(stride) {
        let slot = (k / HRPD_SLOT_CHIPS).min(ACCESS_FRAME_SLOTS - 1);
        let a = scan.interp(samples, index);
        let b = scan.interp(samples, index + frame_samples);
        let c = scan.interp(samples, index + 2 * frame_samples);
        index += scan.step;
        dot01[slot] += a.conj() * b;
        dot02[slot] += a.conj() * c;
        dot12[slot] += b.conj() * c;
        pow0[slot] += a.norm_sqr();
        pow1[slot] += b.norm_sqr();
        pow2[slot] += c.norm_sqr();
    }
    // Combine slots non-coherently: each slot is individually coherent
    // (pilot is frame-periodic within a slot), but a gated probe's carrier
    // phase jumps between slots, so a coherent sum across slots cancels.
    // Summing per-slot magnitudes keeps active slots additive and lets dead
    // (low-power) slots fall out on their own — no gating threshold needed.
    let (mut num01, mut num02, mut num12) = (0.0f32, 0.0f32, 0.0f32);
    let (mut den01, mut den02, mut den12) = (0.0f32, 0.0f32, 0.0f32);
    let mut sum_d01 = Complex32::new(0.0, 0.0);
    for s in 0..ACCESS_FRAME_SLOTS {
        num01 += dot01[s].norm();
        num02 += dot02[s].norm();
        num12 += dot12[s].norm();
        den01 += (pow0[s] * pow1[s]).sqrt();
        den02 += (pow0[s] * pow2[s]).sqrt();
        den12 += (pow1[s] * pow2[s]).sqrt();
        sum_d01 += dot01[s];
    }
    let coh01 = num01 / den01.max(1.0e-12);
    let coh02 = num02 / den02.max(1.0e-12);
    let coh12 = num12 / den12.max(1.0e-12);
    let coherence = coh01.min(coh02).min(coh12);
    Some((coherence, sum_d01.arg()))
}

fn preamble_spec_coherence_with_reference(
    samples: &[Complex32],
    geometry: ChipGeometry,
    preamble_start_chip: i64,
    reference: &[Complex32],
) -> Option<f32> {
    const SPEC_COHERENCE_STRIDE: usize = 16;
    let mut coherent = [Complex32::new(0.0, 0.0); ACCESS_FRAME_SLOTS];
    let mut abs_sum = [0.0f32; ACCESS_FRAME_SLOTS];

    let last_k = (ACCESS_PACKET_CHIPS - 1) / SPEC_COHERENCE_STRIDE * SPEC_COHERENCE_STRIDE;
    let last_chip = preamble_start_chip + last_k as i64;
    let scan = geometry.scan(
        samples,
        preamble_start_chip,
        last_chip,
        SPEC_COHERENCE_STRIDE,
    )?;

    let mut index = scan.index;
    for k in (0..ACCESS_PACKET_CHIPS).step_by(SPEC_COHERENCE_STRIDE) {
        let slot = (k / HRPD_SLOT_CHIPS).min(ACCESS_FRAME_SLOTS - 1);
        let sample = scan.interp(samples, index);
        index += scan.step;
        let v = sample * reference[k].conj();
        coherent[slot] += v;
        abs_sum[slot] += v.norm();
    }
    // Non-coherent slot combining (see preamble_lag_coherence): sum
    // per-slot magnitudes because a gated probe's carrier phase jumps
    // between slots.
    let num: f32 = coherent.iter().map(|c| c.norm()).sum();
    let den: f32 = abs_sum.iter().sum();
    (den > 0.0).then_some(num / den)
}

fn access_packet_samples_available(
    samples: &[Complex32],
    absolute_sample_start: i64,
    oversample: usize,
    timing: &HrpdAccessPreambleTiming,
    preamble_frames: usize,
) -> bool {
    if samples.is_empty() {
        return false;
    }
    let first_sample = timing.preamble_start_chip as f64 * oversample as f64
        + f64::from(timing.sample_delay)
        + f64::from(timing.sample_delay_fraction);
    if first_sample < absolute_sample_start as f64 {
        return false;
    }
    let packet_last_chip =
        timing.preamble_start_chip + ((preamble_frames + 1) * ACCESS_PACKET_CHIPS - 1) as i64;
    let last_sample = packet_last_chip as f64 * oversample as f64
        + f64::from(timing.sample_delay)
        + f64::from(timing.sample_delay_fraction);
    let last_idx = last_sample - absolute_sample_start as f64;
    last_idx.is_finite() && last_idx >= 0.0 && (last_idx.floor() as usize + 1) < samples.len()
}

#[allow(clippy::too_many_arguments)]
fn extract_spec_despread_access_packet_chips(
    samples: &[Complex32],
    geometry: ChipGeometry,
    preamble_start_chip: i64,
    preamble_frames: usize,
    access_cycle_number: u8,
    sector_id_lsb: u32,
    color_code: u8,
    reference_chip_offset: i32,
    q_pair_phase: u64,
    q_sign: f32,
    pn_phase_offset_chips: i32,
    lc_phase_offset_chips: i32,
    mode: HrpdAccessPacketDespreadMode,
    phase_step: f32,
) -> Option<Vec<Complex32>> {
    let packet_start = preamble_start_chip + (preamble_frames * ACCESS_PACKET_CHIPS) as i64;
    if packet_start < 0 {
        return None;
    }
    let i_mask = HrpdAccessLongCodeMask {
        access_cycle_number,
        sector_id_lsb,
        color_code,
    }
    .to_mask();
    let q_mask = derive_q_mask(i_mask);
    let reference = hrpd_reverse_pilot_reference_chips(HrpdReversePilotReferenceConfig {
        start_chip: packet_start as u64,
        len: ACCESS_PACKET_CHIPS,
        i_mask,
        q_mask,
        reference_chip_offset,
        pn_phase_offset_chips,
        lc_phase_offset_chips,
        q_sign,
        q_pair_phase,
    });

    let packet_phase_correction = complex_phase(-phase_step * preamble_frames as f32);
    let last_chip = packet_start + (ACCESS_PACKET_CHIPS - 1) as i64;
    let scan = geometry.scan(samples, packet_start, last_chip, 1)?;

    let mut index = scan.index;
    let mut chips = Vec::with_capacity(ACCESS_PACKET_CHIPS);
    for k in 0..ACCESS_PACKET_CHIPS {
        let sample = scan.interp(samples, index);
        index += scan.step;
        let despread = match mode {
            HrpdAccessPacketDespreadMode::Composite => sample * reference[k].conj(),
        };
        chips.push(despread * packet_phase_correction);
    }
    Some(chips)
}

fn complex_phase(phase: f32) -> Complex32 {
    Complex32::new(phase.cos(), phase.sin())
}

/// Chip-to-sample mapping shared by every position in one scan.
#[derive(Clone, Copy)]
struct ChipGeometry {
    absolute_sample_start: i64,
    oversample: usize,
    sample_delay: i32,
    sample_delay_fraction: f32,
}

impl ChipGeometry {
    fn new(absolute_sample_start: i64, oversample: usize) -> Self {
        Self {
            absolute_sample_start,
            oversample,
            sample_delay: 0,
            sample_delay_fraction: 0.0,
        }
    }

    fn at_delay(self, sample_delay: i32, sample_delay_fraction: f32) -> Self {
        Self {
            sample_delay,
            sample_delay_fraction,
            ..self
        }
    }

    fn position(&self, chip: i64) -> Option<(i64, f32)> {
        let sample_abs = chip as f64 * self.oversample as f64
            + f64::from(self.sample_delay)
            + f64::from(self.sample_delay_fraction);
        let idx = sample_abs - self.absolute_sample_start as f64;
        if !idx.is_finite() || idx < 0.0 {
            return None;
        }
        let floor = idx.floor();
        Some((floor as i64, (idx - floor) as f32))
    }

    /// Resolve the scan covering chips `first..=last`, visited `stride` chips
    /// at a time.
    fn scan(
        &self,
        samples: &[Complex32],
        first: i64,
        last: i64,
        stride: usize,
    ) -> Option<ChipScan> {
        let (index, fraction) = self.position(first)?;
        let (last_index, last_fraction) = self.position(last)?;
        if last_fraction != fraction
            || last_index != index + (last - first) * self.oversample as i64
        {
            return None;
        }
        // The scan is monotonic, so the last chip covers every earlier one.
        if (last_index as usize) + 1 >= samples.len() {
            return None;
        }
        Some(ChipScan {
            index,
            fraction,
            step: (stride * self.oversample) as i64,
        })
    }
}

/// One sample index, fraction, and stride covering a whole chip scan. Only
/// valid when both endpoints agree on the fraction — at live-capture chip
/// counts an f64 position cannot hold one, and it rounds to a whole sample.
#[derive(Clone, Copy)]
struct ChipScan {
    index: i64,
    fraction: f32,
    step: i64,
}

impl ChipScan {
    #[inline]
    fn interp(&self, samples: &[Complex32], index: i64) -> Complex32 {
        let lo = index as usize;
        let a = samples[lo];
        let b = samples[lo + 1];
        Complex32::new(
            a.re + (b.re - a.re) * self.fraction,
            a.im + (b.im - a.im) * self.fraction,
        )
    }
}
