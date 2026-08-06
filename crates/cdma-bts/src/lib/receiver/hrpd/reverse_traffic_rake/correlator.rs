//! HRPD reverse Traffic Channel correlator for `GenericRakeReceiver`.
//!
//! Acquisition driver: wraps the shared
//! [`HrpdReverseFftPilotSearcher`](crate::receiver::hrpd::reverse_fft_pilot_search)
//! primitive with a traffic-specific reference (the AT's per-call long-code
//! masks from `HrpdTrafficAssignmentRequest`) and on first lock spawns a
//! [`HrpdReverseTrafficFinger`] plus the four-stage decoder sub-chain. A
//! single AT is expected per worker, so a live finger suspends additional
//! spawn attempts. If tracking drops, the stale finger is retired and this
//! correlator resumes acquisition for up to the traffic pilot-loss deadline.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use log::{debug, info, warn};
use num_complex::Complex32;
use tokio::sync::mpsc as tokio_mpsc;

use cdma_common::hrpd::air::{
    HrpdTrafficAssignmentRequest, HrpdTrafficEvent, default_reverse_traffic_long_code_masks,
};

use crate::bts::hrpd::{HarqBus, HrpdPowerControlHandle};
use crate::receiver::hrpd::reverse_correlator_base::{
    HrpdReverseCorrelatorBase, HrpdReverseFingerSpawnStrategy, SpawnOutcome,
};
use crate::receiver::hrpd::reverse_fft_pilot_search::{
    HrpdReverseFftPilotHit, HrpdReverseFftPilotSearchConfig, HrpdReversePilotReference,
};
use crate::receiver::hrpd::reverse_spread::{
    HrpdReversePilotReferenceConfig, hrpd_reverse_pilot_reference_chips,
};
use crate::receiver::pipelined::generic_rake_receiver::Correlator;
use crate::receiver::pipelined::{PipelineProcessorShared, SampleBlock};

use super::despread::{
    HRPD_RRI_HEAD_CHIPS, HRPD_SLOT_CHIPS, HRPD_TRAFFIC_FRAME_CHIPS, HRPD_TRAFFIC_SLOTS_PER_FRAME,
    HrpdTrafficMaskCandidate, hrpd_long_code_signs_at_phase, hrpd_reverse_composite_reference,
    hrpd_reverse_terminal_pn_signs, pilot_phase_ramp_from_slots, sample_chip_at_delay,
};
use super::finger::{
    HrpdReverseTrafficFinger, HrpdReverseTrafficFingerConfig, HrpdReverseTrafficFingerLock,
};
use super::{
    HrpdReverseTrafficAckProcessor, HrpdReverseTrafficDataProcessor,
    HrpdReverseTrafficDrcProcessor, HrpdReverseTrafficRriProcessor,
};

/// Linear peak/mean SNR threshold for declaring a traffic-pilot lock from
/// the FFT primitive. About 15 dB rejects the FFT order-statistics noise floor
/// while passing field captures whose reverse traffic pilot sits in the high
/// teens.
const HRPD_TRAFFIC_PILOT_SNR_THRESHOLD_LINEAR: f32 = 31.622776;

/// Search window must be larger than the reference template for the FFT
/// primitive to actually slide — `valid_delay_samples = window - frame`.
const HRPD_TRAFFIC_SEARCH_WINDOW_FRAMES: usize = 2;
// A traffic pilot is continuous while the AT is transmitting. A two-frame
// window searches every possible one-frame delay, so overlapping adjacent
// windows repeat the same coverage and double acquisition CPU cost.
const HRPD_TRAFFIC_SEARCH_STEP_FRAMES: usize = HRPD_TRAFFIC_SEARCH_WINDOW_FRAMES;
const HRPD_TRAFFIC_REACQUISITION_TIMEOUT_CHIPS: u64 = 5 * 1_228_800;

/// Sub-chip refinement search half-width in samples around the FFT hit.
/// The FFT primitive's IFFT peak is NOT reliably chip-level accurate on
/// real captures (observed up to ±5 chips of residual misalignment), so
/// this sweep has to be wide. Drives both the search range and the
/// `refinement_buffer_ready` / trim head-room reservations.
const REFINE_SAMPLE_DELAY_RANGE: i64 = 64;
// Keep the historic delay coverage, but score it coarsely first and only run
// the expensive fractional search around the winning delay. HRPD-centered
// captures can land close to zero while live composite RX has been observed
// around +50 output samples of group delay.
const VERIFY_SAMPLE_DELAY_MIN: i32 = -32;
const VERIFY_SAMPLE_DELAY_MAX: i32 = 80;
const VERIFY_SAMPLE_DELAY_COARSE_STEP: usize = 8;
const VERIFY_SAMPLE_DELAY_FINE_RADIUS: i32 = 4;
const VERIFY_SAMPLE_DELAY_FINE_STEP: usize = 2;
const VERIFY_COHERENCE_THRESHOLD: f32 = 0.44;
const VERIFY_FINE_MIN_COHERENCE: f32 = 0.30;

/// Adapter: builds one frame of reverse-traffic pilot reference chips per
/// FFT window. The chips come from `hrpd_reverse_pilot_reference_chips` with
/// the per-AT mask pair and the production spreading-parity choice
/// (q_sign = -1, q_pair_phase = 0) — these are the values the existing live
/// worker locks against on field captures.
struct HrpdReverseTrafficPilotReference {
    i_mask: u64,
    q_mask: u64,
    q_sign: f32,
    q_pair_phase: u64,
}

impl HrpdReversePilotReference for HrpdReverseTrafficPilotReference {
    fn template_chips(&self, window_start_chip: u64, len: usize) -> Vec<Complex32> {
        hrpd_reverse_pilot_reference_chips(HrpdReversePilotReferenceConfig {
            start_chip: window_start_chip,
            len,
            i_mask: self.i_mask,
            q_mask: self.q_mask,
            reference_chip_offset: 0,
            pn_phase_offset_chips: 0,
            lc_phase_offset_chips: 0,
            q_sign: self.q_sign,
            q_pair_phase: self.q_pair_phase,
        })
    }
}

/// Per-AT reverse Traffic Channel correlator. Delegates the scan loop to
/// [`HrpdReverseCorrelatorBase`].
pub struct HrpdReverseTrafficCorrelator {
    base: HrpdReverseCorrelatorBase<HrpdReverseTrafficSpawnStrategy>,
}

struct HrpdReverseTrafficSpawnStrategy {
    assignment: HrpdTrafficAssignmentRequest,
    oversample: usize,
    event_tx: Option<tokio_mpsc::UnboundedSender<HrpdTrafficEvent>>,
    harq_bus: Option<Arc<HarqBus>>,
    power_control: Option<HrpdPowerControlHandle>,
    reverse_pilot_acquired: Arc<AtomicBool>,
    reference: HrpdReverseTrafficPilotReference,
    mask_candidates: Vec<HrpdTrafficMaskCandidate>,
    /// Correlator construction time; the worker is created on traffic
    /// assignment, so this anchors acquisition latency against the
    /// TRTCMPANSetup 1 s budget.
    spawned_at: std::time::Instant,
    next_finger_id: u64,
    /// Fingers still active. While non-empty, the correlator does not spawn
    /// another finger (single-AT model).
    active_fingers: Vec<ActiveTrafficFinger>,
    acquisition_deadline: TrafficAcquisitionDeadline,
    /// Loss already accumulated by a retired tracking finger. The rake does
    /// not call the correlator while a validated finger is active, so this is
    /// anchored to the first fresh acquisition block after retirement.
    pending_reacquisition_lost_chips: u64,
    last_good_pilot_chip: u64,
    last_pilot_snr_db: f32,
    last_pilot_coherence: f32,
}

#[derive(Clone, Copy, Debug)]
struct ActiveTrafficFinger {
    id: u64,
    hard_validated: bool,
    signal_lost_chips: u64,
}

#[derive(Clone, Copy, Debug, Default)]
struct TrafficAcquisitionDeadline {
    started_chip: Option<u64>,
    timed_out: bool,
}

impl TrafficAcquisitionDeadline {
    fn ensure_started(&mut self, chip: u64) {
        if self.started_chip.is_none() && !self.timed_out {
            self.started_chip = Some(chip);
        }
    }

    fn begin_at(&mut self, chip: u64) {
        if !self.timed_out {
            self.started_chip = Some(self.started_chip.map_or(chip, |start| start.min(chip)));
        }
    }

    fn clear(&mut self) {
        self.started_chip = None;
        self.timed_out = false;
    }

    fn expire_at(&mut self, chip: u64) -> Option<(u64, u64)> {
        let started_chip = self.started_chip?;
        let elapsed = chip.saturating_sub(started_chip);
        if self.timed_out || elapsed < HRPD_TRAFFIC_REACQUISITION_TIMEOUT_CHIPS {
            return None;
        }
        self.timed_out = true;
        Some((started_chip, elapsed))
    }
}

#[derive(Clone, Copy, Debug)]
struct VerifiedTrafficPilot {
    frame_start_chip: u64,
    sample_delay: i32,
    sample_delay_fraction: f32,
    pilot_phase: Complex32,
    coherence: f32,
    mask: HrpdTrafficMaskCandidate,
}

/// PN/LC/composite reference generated once per FFT hit for the assigned
/// traffic-frame boundary. PN, the long codes, and reverse data framing are
/// anchored to the HRPD system-time frame grid, so verification may refine
/// sample delay but must not slide the chip-level frame start off the
/// assigned FrameOffset.
struct VerifyReferenceSpan {
    start_chip: u64,
    pn: Vec<(f32, f32)>,
    lc_i: Vec<f32>,
    lc_q: Vec<f32>,
    ref_conj: Vec<Complex32>,
}

impl VerifyReferenceSpan {
    fn build(nominal_frame_start_chip: u64, mask: HrpdTrafficMaskCandidate) -> Self {
        let start_chip = nominal_frame_start_chip;
        let len = HRPD_TRAFFIC_FRAME_CHIPS;
        let pn = hrpd_reverse_terminal_pn_signs(start_chip, len);
        let lc_i = hrpd_long_code_signs_at_phase(mask.i_mask, start_chip, len);
        let lc_q = hrpd_long_code_signs_at_phase(mask.q_mask, start_chip, len);
        let mut ref_conj: Vec<Complex32> = Vec::with_capacity(len);
        for chip in 0..len {
            let r = hrpd_reverse_composite_reference(
                start_chip + chip as u64,
                chip,
                &pn,
                &lc_i,
                &lc_q,
                mask,
            );
            ref_conj.push(r.conj());
        }
        Self {
            start_chip,
            pn,
            lc_i,
            lc_q,
            ref_conj,
        }
    }

    /// One frame's conjugated composite reference starting at
    /// `frame_start_chip`, bit-identical to building it from scratch.
    fn frame_ref_conj(
        &self,
        frame_start_chip: u64,
        mask: HrpdTrafficMaskCandidate,
    ) -> Option<Vec<Complex32>> {
        let offset = frame_start_chip.checked_sub(self.start_chip)? as usize;
        if offset + HRPD_TRAFFIC_FRAME_CHIPS > self.ref_conj.len() {
            return None;
        }
        let mut out = self.ref_conj[offset..offset + HRPD_TRAFFIC_FRAME_CHIPS].to_vec();
        // Chip 0 of a frame pairs against itself (saturating) rather than the
        // span's true previous chip; recompute it with frame-local indexing.
        out[0] = hrpd_reverse_composite_reference(
            frame_start_chip,
            0,
            &self.pn[offset..],
            &self.lc_i[offset..],
            &self.lc_q[offset..],
            mask,
        )
        .conj();
        Some(out)
    }
}

impl HrpdReverseTrafficCorrelator {
    pub fn new(
        assignment: HrpdTrafficAssignmentRequest,
        oversample: usize,
        event_tx: Option<tokio_mpsc::UnboundedSender<HrpdTrafficEvent>>,
        harq_bus: Option<Arc<HarqBus>>,
        power_control: Option<HrpdPowerControlHandle>,
        reverse_pilot_acquired: Arc<AtomicBool>,
    ) -> Self {
        let mut mask_candidates = Vec::with_capacity(2);
        push_mask_candidates(
            &mut mask_candidates,
            assignment.reverse_long_code_mask_i,
            assignment.reverse_long_code_mask_q,
            "assigned",
        );
        let (uati_i_mask, uati_q_mask) = default_reverse_traffic_long_code_masks(assignment.uati);
        if uati_i_mask != assignment.reverse_long_code_mask_i
            || uati_q_mask != assignment.reverse_long_code_mask_q
        {
            push_mask_candidates(&mut mask_candidates, uati_i_mask, uati_q_mask, "uati");
        }
        let reference = HrpdReverseTrafficPilotReference {
            i_mask: assignment.reverse_long_code_mask_i,
            q_mask: assignment.reverse_long_code_mask_q,
            q_sign: -1.0,
            q_pair_phase: 0,
        };
        let strategy = HrpdReverseTrafficSpawnStrategy {
            assignment,
            oversample,
            event_tx,
            harq_bus,
            power_control,
            reverse_pilot_acquired,
            reference,
            mask_candidates,
            spawned_at: std::time::Instant::now(),
            next_finger_id: 0,
            active_fingers: Vec::new(),
            acquisition_deadline: TrafficAcquisitionDeadline::default(),
            pending_reacquisition_lost_chips: 0,
            last_good_pilot_chip: 0,
            last_pilot_snr_db: 0.0,
            last_pilot_coherence: 0.0,
        };
        Self {
            base: HrpdReverseCorrelatorBase::new(strategy, "hrpd_traffic"),
        }
    }
}

fn push_mask_candidates(
    out: &mut Vec<HrpdTrafficMaskCandidate>,
    i_mask: u64,
    q_mask: u64,
    base_label: &'static str,
) {
    // The reverse pilot cannot distinguish Q-arm sign/pairing, while DRC,
    // ACK, and data polarity depend on it. Racing all four pilot-equivalent
    // candidates occasionally selected q+/p1 on live reacquisition: pilot
    // coherence then looked healthy, but every setup RTCAck was NAKed. Use
    // the production/spec spreading parity already used by the FFT reference.
    let label = match base_label {
        "assigned" => "assigned/q-/p0",
        "uati" => "uati/q-/p0",
        _ => base_label,
    };
    out.push(HrpdTrafficMaskCandidate {
        i_mask,
        q_mask,
        q_sign: -1.0,
        q_pair_phase: 0,
        label,
    });
}

fn assigned_frame_start_before_or_at(chip: u64, frame_offset: u8) -> u64 {
    let target_slot_phase = u64::from(frame_offset & 0x0f);
    let slot = chip / HRPD_SLOT_CHIPS as u64;
    let slot_phase = slot & 0x0f;
    let sub_slots = (slot_phase + 16 - target_slot_phase) & 0x0f;
    slot.saturating_sub(sub_slots) * HRPD_SLOT_CHIPS as u64
}

fn assigned_frame_start_candidates_near(chip: u64, frame_offset: u8) -> Vec<u64> {
    let before = assigned_frame_start_before_or_at(chip, frame_offset);
    let after = before.saturating_add(HRPD_TRAFFIC_FRAME_CHIPS as u64);
    if after == before {
        vec![before]
    } else {
        vec![before, after]
    }
}

impl HrpdReverseTrafficSpawnStrategy {
    fn snr_db_tenths(value: f32) -> i16 {
        if !value.is_finite() {
            return 0;
        }
        (value * 10.0)
            .round()
            .clamp(i16::MIN as f32, i16::MAX as f32) as i16
    }

    fn coherence_x1000(value: f32) -> u16 {
        if !value.is_finite() {
            return 0;
        }
        (value * 1000.0).round().clamp(0.0, u16::MAX as f32) as u16
    }

    fn report_acquisition_timeout(&mut self, at_chip: u64) {
        let Some((search_started_chip, lost_chips)) = self.acquisition_deadline.expire_at(at_chip)
        else {
            return;
        };
        self.reverse_pilot_acquired.store(false, Ordering::Release);
        let event = HrpdTrafficEvent::ReversePilotLost {
            uati: self.assignment.uati,
            mac_index: self.assignment.mac_index,
            last_good_chip: self.last_good_pilot_chip,
            lost_at_chip: at_chip,
            lost_chips,
            last_snr_db_tenths: Self::snr_db_tenths(self.last_pilot_snr_db),
            last_coherence_x1000: Self::coherence_x1000(self.last_pilot_coherence),
        };
        match &self.event_tx {
            Some(tx) => match tx.send(event) {
                Ok(()) => warn!(
                    "rx_hrpd_traffic[m{}]: reverse pilot acquisition timed out uati=0x{:08x} search_started_chip={} lost_at_chip={} lost_ms={} last_good_chip={} last_snr={:.2}dB last_coh={:.3}; sent AN event",
                    self.assignment.mac_index,
                    self.assignment.uati,
                    search_started_chip,
                    at_chip,
                    lost_chips.saturating_mul(1000) / 1_228_800,
                    self.last_good_pilot_chip,
                    self.last_pilot_snr_db,
                    self.last_pilot_coherence,
                ),
                Err(err) => warn!(
                    "rx_hrpd_traffic[m{}]: reverse pilot acquisition timed out uati=0x{:08x}, but AN event send failed: {}",
                    self.assignment.mac_index, self.assignment.uati, err
                ),
            },
            None => warn!(
                "rx_hrpd_traffic[m{}]: reverse pilot acquisition timed out uati=0x{:08x}, but no AN event channel is configured",
                self.assignment.mac_index, self.assignment.uati
            ),
        }
    }

    /// Returns true if `buffer` holds enough samples FORWARD of
    /// `frame_start_chip` to despread one frame plus the
    /// `REFINE_SAMPLE_DELAY_RANGE` positive sample-delay overhang. The
    /// negative-delay overhang is handled by `refine_sample_delay`
    /// clamping its search range to whatever back-history the buffer
    /// actually has — there's no way to grow buffer history backward.
    fn refinement_buffer_ready(
        &self,
        buffer: &[Complex32],
        buffer_abs_sample: u64,
        frame_start_chip: u64,
    ) -> bool {
        let oversample = self.oversample.max(1);
        let frame_samples = HRPD_TRAFFIC_FRAME_CHIPS * oversample;
        let frame_start_sample = frame_start_chip.saturating_mul(oversample as u64);
        let signed = frame_start_sample as i64 - buffer_abs_sample as i64;
        if signed < 0 {
            return false;
        }
        let offset = signed as usize;
        offset + frame_samples + REFINE_SAMPLE_DELAY_RANGE as usize + 2 <= buffer.len()
    }

    fn build_finger(
        &mut self,
        verified: VerifiedTrafficPilot,
    ) -> (HrpdReverseTrafficFinger, Vec<PipelineProcessorShared>) {
        let id = self.next_finger_id;
        self.next_finger_id = self.next_finger_id.wrapping_add(1);
        self.active_fingers.push(ActiveTrafficFinger {
            id,
            hard_validated: false,
            signal_lost_chips: 0,
        });
        let config = HrpdReverseTrafficFingerConfig {
            uati: self.assignment.uati,
            mac_index: self.assignment.mac_index,
            physical_layer_subtype: self.assignment.physical_layer_subtype,
            reverse_traffic_mac_subtype: self.assignment.reverse_traffic_mac_subtype,
            frame_offset: self.assignment.frame_offset & 0x0f,
            i_mask: verified.mask.i_mask,
            q_mask: verified.mask.q_mask,
            drc_cover: self.assignment.drc_cover,
            drc_length: self.assignment.drc_length.max(1),
            oversample: self.oversample,
            event_tx: self.event_tx.clone(),
            harq_bus: self.harq_bus.clone(),
            power_control: self.power_control.clone(),
            reverse_pilot_acquired: Some(self.reverse_pilot_acquired.clone()),
            worker_spawned_at: self.spawned_at,
        };
        let lock = HrpdReverseTrafficFingerLock {
            frame_start_chip: verified.frame_start_chip,
            chip_offset: 0,
            sample_delay: verified.sample_delay,
            sample_delay_fraction: verified.sample_delay_fraction,
            q_sign: verified.mask.q_sign,
            q_pair_phase: verified.mask.q_pair_phase,
            initial_pilot_phase: verified.pilot_phase,
        };
        let finger = HrpdReverseTrafficFinger::new(id, config, lock);
        let chain: Vec<PipelineProcessorShared> = vec![
            Box::new(HrpdReverseTrafficRriProcessor::new()),
            Box::new(HrpdReverseTrafficAckProcessor::new(
                self.harq_bus.clone(),
                self.event_tx.clone(),
            )),
            Box::new(HrpdReverseTrafficDrcProcessor::new(
                self.event_tx.clone(),
                self.harq_bus.clone(),
            )),
            Box::new(HrpdReverseTrafficDataProcessor::new_with_power_control(
                self.event_tx.clone(),
                self.power_control.clone(),
            )),
        ];
        (finger, chain)
    }

    fn verify_fft_hit(
        &self,
        buffer: &[Complex32],
        buffer_abs_sample: u64,
        hit: &HrpdReverseFftPilotHit,
    ) -> Option<VerifiedTrafficPilot> {
        let oversample = self.oversample.max(1);
        let last_sample = buffer_abs_sample + buffer.len().saturating_sub(1) as u64;
        let last_chip = last_sample / oversample as u64;
        let mut best: Option<VerifiedTrafficPilot> = None;
        for nominal in assigned_frame_start_candidates_near(
            hit.preamble_start_chip,
            self.assignment.frame_offset,
        ) {
            if nominal + HRPD_TRAFFIC_FRAME_CHIPS as u64 > last_chip {
                continue;
            }
            // Generate PN/LC/composite reference once at the assigned frame
            // boundary. Sample-delay refinement accounts for receiver timing;
            // moving the chip-level frame start would break reverse RRI/data
            // framing and can keep TrafficChannelComplete from decoding.
            let spans = self
                .mask_candidates
                .iter()
                .copied()
                .map(|mask| (mask, VerifyReferenceSpan::build(nominal, mask)))
                .collect::<Vec<_>>();
            let mut coarse: Vec<VerifiedTrafficPilot> = Vec::new();
            for (candidate_mask, span) in &spans {
                if let Some(candidate) = self.verify_candidate_at_frame_start(
                    buffer,
                    buffer_abs_sample,
                    nominal,
                    *candidate_mask,
                    64,
                    span,
                ) {
                    coarse.push(candidate);
                }
            }
            coarse.sort_by(|a, b| b.coherence.total_cmp(&a.coherence));
            coarse.truncate(2);
            for coarse_candidate in coarse {
                if coarse_candidate.coherence < VERIFY_FINE_MIN_COHERENCE {
                    if best
                        .as_ref()
                        .is_none_or(|best| coarse_candidate.coherence > best.coherence)
                    {
                        best = Some(coarse_candidate);
                    }
                    continue;
                }
                for (candidate_mask, span) in &spans {
                    if candidate_mask.i_mask != coarse_candidate.mask.i_mask
                        || candidate_mask.q_mask != coarse_candidate.mask.q_mask
                        || candidate_mask.q_sign != coarse_candidate.mask.q_sign
                        || candidate_mask.q_pair_phase != coarse_candidate.mask.q_pair_phase
                    {
                        continue;
                    }
                    if let Some(candidate) = self.verify_candidate_at_frame_start(
                        buffer,
                        buffer_abs_sample,
                        nominal,
                        *candidate_mask,
                        16,
                        span,
                    ) {
                        if best
                            .as_ref()
                            .is_none_or(|best| candidate.coherence > best.coherence)
                        {
                            best = Some(candidate);
                        }
                    }
                }
            }
        }
        best
    }

    #[allow(clippy::too_many_arguments)]
    fn verify_candidate_at_frame_start(
        &self,
        buffer: &[Complex32],
        buffer_abs_sample: u64,
        frame_start_chip: u64,
        mask: HrpdTrafficMaskCandidate,
        pilot_chip_step: usize,
        span: &VerifyReferenceSpan,
    ) -> Option<VerifiedTrafficPilot> {
        let oversample = self.oversample.max(1);
        let frame_start_sample = frame_start_chip.checked_mul(oversample as u64)?;
        if frame_start_sample < buffer_abs_sample {
            return None;
        }
        let base_start = (frame_start_sample - buffer_abs_sample) as usize;
        let frame_samples = HRPD_TRAFFIC_FRAME_CHIPS * oversample;
        if base_start + frame_samples + REFINE_SAMPLE_DELAY_RANGE as usize + 2 > buffer.len() {
            return None;
        }
        let ref_conj = span.frame_ref_conj(frame_start_chip, mask)?;
        let mut coarse_best: Option<VerifiedTrafficPilot> = None;
        for sample_delay in (VERIFY_SAMPLE_DELAY_MIN..=VERIFY_SAMPLE_DELAY_MAX)
            .step_by(VERIFY_SAMPLE_DELAY_COARSE_STEP)
        {
            if let Some((pilot_phase, coherence)) = refine_inner_step(
                buffer,
                base_start,
                oversample,
                sample_delay,
                0.0,
                &ref_conj,
                pilot_chip_step,
            ) {
                let candidate = VerifiedTrafficPilot {
                    frame_start_chip,
                    sample_delay,
                    sample_delay_fraction: 0.0,
                    pilot_phase,
                    coherence,
                    mask,
                };
                if coarse_best
                    .as_ref()
                    .is_none_or(|best| candidate.coherence > best.coherence)
                {
                    coarse_best = Some(candidate);
                }
            }
        }
        let coarse_best = coarse_best?;
        let mut best = Some(coarse_best);
        let fine_min = (coarse_best.sample_delay - VERIFY_SAMPLE_DELAY_FINE_RADIUS)
            .max(VERIFY_SAMPLE_DELAY_MIN);
        let fine_max = (coarse_best.sample_delay + VERIFY_SAMPLE_DELAY_FINE_RADIUS)
            .min(VERIFY_SAMPLE_DELAY_MAX);
        for sample_delay in (fine_min..=fine_max).step_by(VERIFY_SAMPLE_DELAY_FINE_STEP) {
            for sample_delay_fraction in [0.0_f32, -0.75, 0.75] {
                if let Some((pilot_phase, coherence)) = refine_inner_step(
                    buffer,
                    base_start,
                    oversample,
                    sample_delay,
                    sample_delay_fraction,
                    &ref_conj,
                    pilot_chip_step,
                ) {
                    let candidate = VerifiedTrafficPilot {
                        frame_start_chip,
                        sample_delay,
                        sample_delay_fraction,
                        pilot_phase,
                        coherence,
                        mask,
                    };
                    if best
                        .as_ref()
                        .is_none_or(|best| candidate.coherence > best.coherence)
                    {
                        best = Some(candidate);
                    }
                }
            }
        }
        best
    }
}

impl HrpdReverseFingerSpawnStrategy for HrpdReverseTrafficSpawnStrategy {
    type Finger = HrpdReverseTrafficFinger;
    type Reference = HrpdReverseTrafficPilotReference;

    fn fft_config(&self) -> HrpdReverseFftPilotSearchConfig {
        HrpdReverseFftPilotSearchConfig {
            oversample: self.oversample,
            frame_chips: HRPD_TRAFFIC_FRAME_CHIPS,
            search_window_frames: HRPD_TRAFFIC_SEARCH_WINDOW_FRAMES,
            search_step_frames: HRPD_TRAFFIC_SEARCH_STEP_FRAMES,
            snr_threshold: HRPD_TRAFFIC_PILOT_SNR_THRESHOLD_LINEAR,
            max_hits_per_window: 1,
            hit_suppression_chips: HRPD_TRAFFIC_FRAME_CHIPS / 4,
        }
    }

    fn reference(&self) -> &Self::Reference {
        &self.reference
    }

    fn max_hits_per_window(&self) -> usize {
        1
    }

    fn primary_scan_step_samples(&self, one_frame_samples: usize) -> usize {
        one_frame_samples.saturating_mul(HRPD_TRAFFIC_SEARCH_STEP_FRAMES)
    }

    fn search_suppressed(&self) -> bool {
        !self.active_fingers.is_empty() || self.acquisition_deadline.timed_out
    }

    fn absolute_sample_start_for_block(&mut self, block: &SampleBlock) -> u64 {
        let absolute_sample_start = block
            .tags
            .get("absolute_sample_start")
            .and_then(|value| u64::try_from(*value).ok())
            .unwrap_or_else(|| (block.chip_start as u64) * self.oversample as u64);
        let oversample = self.oversample.max(1) as u64;
        let block_start_chip = absolute_sample_start / oversample;
        let block_end_chip =
            absolute_sample_start.saturating_add(block.samples.len() as u64) / oversample;
        if self.active_fingers.is_empty() {
            if self.acquisition_deadline.started_chip.is_none()
                && self.pending_reacquisition_lost_chips > 0
            {
                let first_lost_chip =
                    block_start_chip.saturating_sub(self.pending_reacquisition_lost_chips);
                self.last_good_pilot_chip = first_lost_chip;
                self.acquisition_deadline.begin_at(first_lost_chip);
                self.pending_reacquisition_lost_chips = 0;
            } else {
                self.acquisition_deadline.ensure_started(block_start_chip);
            }
        }
        self.report_acquisition_timeout(block_end_chip);
        absolute_sample_start
    }

    fn spawn_finger(
        &mut self,
        buffer: &[Complex32],
        buffer_abs_sample: u64,
        hit: &HrpdReverseFftPilotHit,
    ) -> SpawnOutcome<Self::Finger> {
        // Snap the FFT-detected pilot hit forward to the AT's assigned
        // reverse traffic frame grid. Per-spec, reverse Traffic Data and RRI
        // frames are delayed by FrameOffset slots from system time.
        let frame_candidates = assigned_frame_start_candidates_near(
            hit.preamble_start_chip,
            self.assignment.frame_offset,
        );
        let Some(frame_start_chip) = frame_candidates
            .iter()
            .copied()
            .find(|candidate| self.refinement_buffer_ready(buffer, buffer_abs_sample, *candidate))
        else {
            let waiting_for = frame_candidates
                .iter()
                .copied()
                .find(|candidate| {
                    candidate.saturating_mul(self.oversample.max(1) as u64) >= buffer_abs_sample
                })
                .unwrap_or_else(|| frame_candidates[0]);
            info!(
                "rx_hrpd_traffic[m{}]: deferring rake hit uati=0x{:08x} waiting_frame_chip={} hit_chip={} frame_offset={} snr={:.2}dB delay_samples={} frame_phase_chips={} buffer_samples={} buffer_abs_sample={}",
                self.assignment.mac_index,
                self.assignment.uati,
                waiting_for,
                hit.preamble_start_chip,
                self.assignment.frame_offset & 0x0f,
                hit.snr_db,
                hit.delay_samples,
                hit.frame_phase_chips,
                buffer.len(),
                buffer_abs_sample,
            );
            return SpawnOutcome::Defer;
        };
        let Some(verified) = self.verify_fft_hit(buffer, buffer_abs_sample, hit) else {
            debug!(
                "rx_hrpd_traffic[m{}]: verify produced no candidate uati=0x{:08x} hit_chip={} ready_frame_chip={} snr={:.2}dB",
                self.assignment.mac_index,
                self.assignment.uati,
                hit.preamble_start_chip,
                frame_start_chip,
                hit.snr_db,
            );
            return SpawnOutcome::Skip;
        };
        if verified.coherence < VERIFY_COHERENCE_THRESHOLD {
            debug!(
                "rx_hrpd_traffic[m{}]: verify below threshold uati=0x{:08x} hit_chip={} frame_chip={} snr={:.2}dB coh={:.3} delay={}{:+.2} mask={}",
                self.assignment.mac_index,
                self.assignment.uati,
                hit.preamble_start_chip,
                verified.frame_start_chip,
                hit.snr_db,
                verified.coherence,
                verified.sample_delay,
                verified.sample_delay_fraction,
                verified.mask.label,
            );
            return SpawnOutcome::Skip;
        }
        info!(
            "rx_hrpd_traffic[m{}]: rake acquired uati=0x{:08x} frame_chip={} frame_offset={} mask={} fft_snr={:.2}dB pilot_coh={:.3} sample_delay={}+{:+.2} delay_samples={} frame_phase_chips={}",
            self.assignment.mac_index,
            self.assignment.uati,
            verified.frame_start_chip,
            self.assignment.frame_offset & 0x0f,
            verified.mask.label,
            hit.snr_db,
            verified.coherence,
            verified.sample_delay,
            verified.sample_delay_fraction,
            hit.delay_samples,
            hit.frame_phase_chips,
        );
        self.last_good_pilot_chip = verified
            .frame_start_chip
            .saturating_add(HRPD_TRAFFIC_FRAME_CHIPS as u64);
        self.last_pilot_snr_db = hit.snr_db;
        self.last_pilot_coherence = verified.coherence;
        let (finger, chain) = self.build_finger(verified);
        SpawnOutcome::Spawn(finger, chain)
    }

    fn buffer_trim_count_after_scan(
        &self,
        buffer_len: usize,
        next_scan_offset: usize,
        window_samples: usize,
    ) -> usize {
        // Keep the buffer bounded but leave head-room behind
        // `next_scan_offset` so sub-chip refinement (which reaches back up
        // to `REFINE_SAMPLE_DELAY_RANGE` samples behind the FFT-detected
        // frame start) has buffer to work with. Without this margin, every
        // FFT hit at a small delay-within-window deferred forever because
        // the trim aligned the buffer's left edge exactly at the next
        // window's start.
        let head_room = (REFINE_SAMPLE_DELAY_RANGE as usize) * 2 + 16;
        let keep_high_water = window_samples * 4;
        if buffer_len > keep_high_water && next_scan_offset > head_room {
            (next_scan_offset - head_room).min(buffer_len)
        } else {
            0
        }
    }

    fn notify_finger_removed(&mut self, finger_id: u64) {
        let removed = self
            .active_fingers
            .iter()
            .position(|finger| finger.id == finger_id)
            .map(|index| self.active_fingers.remove(index));
        if let Some(finger) = removed
            && finger.hard_validated
        {
            self.pending_reacquisition_lost_chips = self
                .pending_reacquisition_lost_chips
                .max(finger.signal_lost_chips);
        }
        if removed.is_some() && self.active_fingers.is_empty() {
            info!(
                "rx_hrpd_traffic[m{}]: traffic finger {} removed; rearming full reverse-pilot acquisition for this assignment",
                self.assignment.mac_index, finger_id
            );
        }
    }

    fn notify_hard_validated(&mut self, finger_id: u64) {
        if let Some(finger) = self
            .active_fingers
            .iter_mut()
            .find(|finger| finger.id == finger_id)
        {
            finger.hard_validated = true;
            finger.signal_lost_chips = 0;
        }
        self.acquisition_deadline.clear();
        info!(
            "rx_hrpd_traffic[m{}]: reverse pilot acquisition complete on finger {}; pausing traffic rake searches while this finger remains active",
            self.assignment.mac_index, finger_id
        );
    }

    fn notify_finger_state(
        &mut self,
        finger_id: u64,
        hard_validated: bool,
        _idle_chips: u64,
        signal_lost_chips: u64,
        _crc_miss_count: u64,
        _post_walsh_no_event_ms: u64,
    ) {
        let Some(finger) = self
            .active_fingers
            .iter_mut()
            .find(|finger| finger.id == finger_id)
        else {
            return;
        };
        finger.hard_validated |= hard_validated;
        finger.signal_lost_chips = signal_lost_chips;
        if !finger.hard_validated {
            return;
        }
        if signal_lost_chips == 0 {
            self.acquisition_deadline.clear();
        }
    }
}

impl Correlator for HrpdReverseTrafficCorrelator {
    type Finger = HrpdReverseTrafficFinger;

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

/// Single sample-delay / fraction evaluation against a precomputed
/// per-chip `conj(reference)` array. Returns
/// `(pilot_phase_unit_vector, coherence)` if the buffer covers the whole
/// frame at this alignment, else `None`. Mirrors
/// `pilot_moments_from_despread`'s slot-noncoherent / chip-incoherent
/// coherence metric so the picked alignment matches what the original
/// per-iteration `despread_frame + pilot_moments` would have picked.
fn refine_inner_step(
    buffer: &[Complex32],
    base_start: usize,
    oversample: usize,
    sample_delay: i32,
    sample_delay_fraction: f32,
    ref_conj: &[Complex32],
    pilot_chip_step: usize,
) -> Option<(Complex32, f32)> {
    let mut coherent = Complex32::new(0.0, 0.0);
    let mut slot_coherent = [Complex32::new(0.0, 0.0); HRPD_TRAFFIC_SLOTS_PER_FRAME];
    let mut count = 0usize;
    for chip_idx in (0..HRPD_TRAFFIC_FRAME_CHIPS).step_by(pilot_chip_step.max(1)) {
        let s = sample_chip_at_delay(
            buffer,
            base_start,
            oversample,
            chip_idx,
            sample_delay,
            sample_delay_fraction,
        )?;
        let despread = s * ref_conj[chip_idx];
        if chip_idx % HRPD_SLOT_CHIPS < HRPD_RRI_HEAD_CHIPS {
            continue;
        }
        coherent += despread;
        slot_coherent[(chip_idx / HRPD_SLOT_CHIPS).min(HRPD_TRAFFIC_SLOTS_PER_FRAME - 1)] +=
            despread;
        count += 1;
    }
    if count == 0 {
        return None;
    }
    let pilot_phase = if coherent.norm_sqr() > 0.0 {
        coherent / coherent.norm()
    } else {
        Complex32::new(1.0, 0.0)
    };
    let (phase_at_frame_start_rad, phase_step_rad_per_slot, _) =
        pilot_phase_ramp_from_slots(&slot_coherent, coherent);
    let mut slot_projected = [0.0f32; HRPD_TRAFFIC_SLOTS_PER_FRAME];
    let mut abs_sum = 0.0f32;
    for chip_idx in (0..HRPD_TRAFFIC_FRAME_CHIPS).step_by(pilot_chip_step.max(1)) {
        let s = sample_chip_at_delay(
            buffer,
            base_start,
            oversample,
            chip_idx,
            sample_delay,
            sample_delay_fraction,
        )?;
        let despread = s * ref_conj[chip_idx];
        if chip_idx % HRPD_SLOT_CHIPS < HRPD_RRI_HEAD_CHIPS {
            continue;
        }
        let phase = phase_at_frame_start_rad
            + phase_step_rad_per_slot * chip_idx as f32 / HRPD_SLOT_CHIPS as f32;
        let (sin, cos) = (-phase).sin_cos();
        let projected = (despread * Complex32::new(cos, sin)).re;
        let slot = (chip_idx / HRPD_SLOT_CHIPS).min(HRPD_TRAFFIC_SLOTS_PER_FRAME - 1);
        slot_projected[slot] += projected;
        abs_sum += projected.abs();
    }
    if abs_sum <= 0.0 {
        return None;
    }
    let noncoherent: f32 = slot_projected.iter().map(|s| s.abs()).sum();
    let coherence = (noncoherent / abs_sum).min(1.0);
    Some((pilot_phase, coherence))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn traffic_mask_candidates_use_canonical_q_arm_parity() {
        let mut candidates = Vec::new();
        push_mask_candidates(&mut candidates, 0x123, 0x456, "assigned");

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].i_mask, 0x123);
        assert_eq!(candidates[0].q_mask, 0x456);
        assert_eq!(candidates[0].q_sign, -1.0);
        assert_eq!(candidates[0].q_pair_phase, 0);
        assert_eq!(candidates[0].label, "assigned/q-/p0");
    }

    #[test]
    fn acquisition_deadline_expires_once_at_five_seconds() {
        let start = 123_456_789;
        let mut deadline = TrafficAcquisitionDeadline::default();
        deadline.ensure_started(start);

        assert_eq!(
            deadline.expire_at(start + HRPD_TRAFFIC_REACQUISITION_TIMEOUT_CHIPS - 1),
            None
        );
        assert_eq!(
            deadline.expire_at(start + HRPD_TRAFFIC_REACQUISITION_TIMEOUT_CHIPS),
            Some((start, HRPD_TRAFFIC_REACQUISITION_TIMEOUT_CHIPS))
        );
        assert_eq!(
            deadline.expire_at(start + HRPD_TRAFFIC_REACQUISITION_TIMEOUT_CHIPS + 1),
            None
        );
    }

    #[test]
    fn reacquisition_deadline_preserves_tracking_loss_time() {
        let reentry_chip = 20_000_000;
        let tracking_loss_chips = 1_228_800 / 2;
        let first_lost_chip = reentry_chip - tracking_loss_chips;
        let mut deadline = TrafficAcquisitionDeadline::default();
        deadline.begin_at(first_lost_chip);

        assert_eq!(
            deadline.expire_at(first_lost_chip + HRPD_TRAFFIC_REACQUISITION_TIMEOUT_CHIPS),
            Some((first_lost_chip, HRPD_TRAFFIC_REACQUISITION_TIMEOUT_CHIPS))
        );
    }
}
