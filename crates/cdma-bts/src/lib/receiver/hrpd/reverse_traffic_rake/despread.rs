//! Despread math for the HRPD reverse-traffic finger.
//!
//! These helpers mirror the per-chip pilot reference construction used by the
//! hand-rolled worker in `bts/rx.rs`. They are factored here so the
//! [`HrpdReverseTrafficFinger`](super::finger::HrpdReverseTrafficFinger) and
//! the per-frame pilot re-estimation can share a single code path.

use num_complex::Complex32;

use crate::phy::coding::long_code::LongCodeGenerator;
use crate::phy::spread::HrpdAccessTerminalPnSequence;
use crate::receiver::hrpd::ack_decoder::ACK_CHIPS_PER_BIT;
use crate::receiver::hrpd::long_code::HRPD_LONG_CODE_INITIAL_STATE;
use crate::receiver::hrpd::reverse_spread::hrpd_reverse_pilot_reference_from_signs;

/// HRPD Rev 0 chips per reverse slot (C.S0024-0 v4.0 §9.2.1.3.1).
pub const HRPD_SLOT_CHIPS: usize = 2048;
/// Slots per reverse Traffic Channel physical-layer packet.
pub const HRPD_TRAFFIC_SLOTS_PER_FRAME: usize = 16;
/// Chips per reverse Traffic Channel physical-layer packet (16 slots).
pub const HRPD_TRAFFIC_FRAME_CHIPS: usize = HRPD_SLOT_CHIPS * HRPD_TRAFFIC_SLOTS_PER_FRAME;
/// Pilot coherence threshold below which the finger considers itself dropped
/// (per-frame). Matches the production live worker's gate.
pub const HRPD_REVERSE_TRAFFIC_PILOT_MIN_COHERENCE: f32 = 0.44;
/// Number of chips at the head of each reverse slot reserved for the RRI burst
/// (C.S0024-0 v4.0 §9.2.1.3.3.1: pilot is TDM-displaced by RRI on slots that
/// carry one).
pub const HRPD_RRI_HEAD_CHIPS: usize = 256;
/// First chip in a reverse slot that is clean W0 pilot for pilot-quality
/// measurements. Chips 0..256 may carry RRI, and the ACK Channel may occupy
/// the first 1024 chips on the I arm, so RPC and phase-quality metrics use the
/// ACK-free second half-slot.
pub const HRPD_PILOT_CLEAN_START_CHIPS: usize = ACK_CHIPS_PER_BIT;
const HRPD_PILOT_WALSH_CHIPS: usize = 16;

/// One reverse-traffic mask candidate (long-code mask pair plus the Q-arm sign
/// and pair-phase conventions handed in from the assignment).
#[derive(Debug, Clone, Copy)]
pub struct HrpdTrafficMaskCandidate {
    pub i_mask: u64,
    pub q_mask: u64,
    pub q_sign: f32,
    pub q_pair_phase: u64,
    pub label: &'static str,
}

/// Per-frame pilot phase / despread parameters captured at finger spawn time.
///
/// Mirrors the live worker's `HrpdTrafficPilotMetric` minus the search-only
/// scoring fields. The finger only needs enough to despread a frame; the
/// correlator owns the search.
#[derive(Debug, Clone, Copy)]
pub struct HrpdReverseTrafficDespreadParams {
    pub frame_start_chip: u64,
    pub chip_offset: i32,
    pub sample_delay: i32,
    pub sample_delay_fraction: f32,
    pub pilot_phase: Complex32,
    pub mask: HrpdTrafficMaskCandidate,
}

/// Reverse short-PN sign sequence (I/Q) starting at `start_chip`.
pub fn hrpd_reverse_terminal_pn_signs(start_chip: u64, len: usize) -> Vec<(f32, f32)> {
    let mut pn = HrpdAccessTerminalPnSequence::new(0, 32768);
    pn.advance_chips(start_chip % 32768);
    (0..len)
        .map(|_| {
            let v = pn.generate_iq();
            (v.re, v.im)
        })
        .collect()
}

/// Reverse long-code sign sequence starting at `start_chip` for the given
/// mask. The long-code state resets every 32768 chips per spec.
pub fn hrpd_long_code_signs_at_phase(mask: u64, start_chip: u64, len: usize) -> Vec<f32> {
    let mut lc = LongCodeGenerator::new(mask);
    lc.set_state(HRPD_LONG_CODE_INITIAL_STATE);
    let mut phase = (start_chip % 32768) as usize;
    lc.advance_chips(phase);
    let mut out = Vec::with_capacity(len);
    for idx in 0..len {
        if idx > 0 && phase == 0 {
            lc.set_state(HRPD_LONG_CODE_INITIAL_STATE);
        }
        out.push(if lc.next_chip() == 1 { -1.0 } else { 1.0 });
        phase = (phase + 1) & 0x7fff;
    }
    out
}

/// Per-chip composite spreading reference (PNI + jPNQ) at `abs_chip`.
pub fn hrpd_reverse_composite_reference(
    abs_chip: u64,
    chip: usize,
    pn: &[(f32, f32)],
    lc_i: &[f32],
    lc_q: &[f32],
    mask: HrpdTrafficMaskCandidate,
) -> Complex32 {
    let pair_chip = if (abs_chip & 1) == (mask.q_pair_phase & 1) {
        chip
    } else {
        chip.saturating_sub(1)
    };
    hrpd_reverse_pilot_reference_from_signs(
        abs_chip & 0x7fff,
        pn[chip].0,
        pn[pair_chip].1,
        lc_i[chip],
        lc_q[pair_chip],
        mask.q_sign,
        mask.q_pair_phase,
    )
}

/// Linear interpolation across the input sample stream at the chip position
/// induced by `base_start + chip * oversample + sample_delay (+ frac)`.
pub fn sample_chip_at_delay(
    samples: &[Complex32],
    base_start: usize,
    oversample: usize,
    chip: usize,
    sample_delay: i32,
    sample_delay_fraction: f32,
) -> Option<Complex32> {
    let sample_pos = base_start as f32
        + chip as f32 * oversample.max(1) as f32
        + sample_delay as f32
        + sample_delay_fraction;
    if !sample_pos.is_finite() || sample_pos < 0.0 {
        return None;
    }
    let lo = sample_pos.floor() as usize;
    let frac = sample_pos - lo as f32;
    if lo + 1 >= samples.len() {
        return None;
    }
    Some(samples[lo] * (1.0 - frac) + samples[lo + 1] * frac)
}

/// Despread one 16-slot reverse-traffic frame from raw IQ samples using
/// `params`. Returns `HRPD_TRAFFIC_FRAME_CHIPS` post-despread chips, with the
/// pilot phase rotated out by `params.pilot_phase.conj()`.
///
/// Returns `None` if the sample window doesn't fully cover the frame (e.g. the
/// finger was just spawned and only partial buffer is available).
pub fn despread_frame(
    samples: &[Complex32],
    absolute_sample_start: u64,
    oversample: usize,
    params: &HrpdReverseTrafficDespreadParams,
) -> Option<Vec<Complex32>> {
    let ref_conj = hrpd_reverse_traffic_reference_conj(
        params.frame_start_chip,
        HRPD_TRAFFIC_FRAME_CHIPS,
        params.mask,
    );
    despread_frame_with_reference(
        samples,
        absolute_sample_start,
        oversample,
        params,
        &ref_conj,
    )
}

/// Build a per-chip conjugated reverse-traffic spreading reference for one
/// frame. Because reverse traffic frames are exactly one PN/LC period
/// (32768 chips), a finger can reuse this across sequential frames after
/// acquisition.
pub fn hrpd_reverse_traffic_reference_conj(
    frame_start_chip: u64,
    len: usize,
    mask: HrpdTrafficMaskCandidate,
) -> Vec<Complex32> {
    let pn = hrpd_reverse_terminal_pn_signs(frame_start_chip, len);
    let lc_i = hrpd_long_code_signs_at_phase(mask.i_mask, frame_start_chip, len);
    let lc_q = hrpd_long_code_signs_at_phase(mask.q_mask, frame_start_chip, len);
    let mut out = Vec::with_capacity(len);
    for chip in 0..len {
        let ref_chip = hrpd_reverse_composite_reference(
            frame_start_chip + chip as u64,
            chip,
            &pn,
            &lc_i,
            &lc_q,
            mask,
        );
        out.push(ref_chip.conj());
    }
    out
}

/// Despread one 16-slot reverse-traffic frame using a precomputed
/// `conj(reference)` slice.
pub fn despread_frame_with_reference(
    samples: &[Complex32],
    absolute_sample_start: u64,
    oversample: usize,
    params: &HrpdReverseTrafficDespreadParams,
    ref_conj: &[Complex32],
) -> Option<Vec<Complex32>> {
    if ref_conj.len() < HRPD_TRAFFIC_FRAME_CHIPS {
        return None;
    }
    despread_chips_with_reference(
        samples,
        absolute_sample_start,
        oversample,
        params.frame_start_chip,
        params.sample_delay,
        params.sample_delay_fraction,
        params.pilot_phase.conj(),
        &ref_conj[..HRPD_TRAFFIC_FRAME_CHIPS],
    )
}

/// Despread exactly `ref_conj.len()` chips beginning at absolute `start_chip`,
/// applying `phase_correction` to every despread chip. Generalizes
/// [`despread_frame_with_reference`] so the per-slot reverse power-control loop
/// can despread a single slot's pilot region as soon as its samples arrive,
/// without waiting for the whole 16-slot frame.
#[allow(clippy::too_many_arguments)]
pub fn despread_chips_with_reference(
    samples: &[Complex32],
    absolute_sample_start: u64,
    oversample: usize,
    start_chip: u64,
    sample_delay: i32,
    sample_delay_fraction: f32,
    phase_correction: Complex32,
    ref_conj: &[Complex32],
) -> Option<Vec<Complex32>> {
    let start_sample = start_chip.checked_mul(oversample as u64)?;
    if start_sample < absolute_sample_start {
        return None;
    }
    let base_start = (start_sample - absolute_sample_start) as usize;
    let mut chips = Vec::with_capacity(ref_conj.len());

    if let Some(walk) = ChipWalk::resolve(
        samples.len(),
        base_start,
        oversample,
        ref_conj.len(),
        sample_delay,
        sample_delay_fraction,
    ) {
        let step = walk.step;
        let frac = walk.fraction;
        let mut lo = walk.first_index;
        if frac == 0.0 {
            for &reference in ref_conj {
                chips.push(samples[lo] * reference * phase_correction);
                lo += step;
            }
        } else {
            for &reference in ref_conj {
                let a = samples[lo];
                let b = samples[lo + 1];
                let sample = a * (1.0 - frac) + b * frac;
                chips.push(sample * reference * phase_correction);
                lo += step;
            }
        }
        return Some(chips);
    }

    for (chip, &reference) in ref_conj.iter().enumerate() {
        let sample = sample_chip_at_delay(
            samples,
            base_start,
            oversample,
            chip,
            sample_delay,
            sample_delay_fraction,
        )?;
        chips.push(sample * reference * phase_correction);
    }
    Some(chips)
}

/// A despread span resolved to a constant sample stride and interpolation
/// fraction, so the loop needs no per-chip position math or bounds test.
///
/// [`resolve`](Self::resolve) returns `None` when the walk would not reproduce
/// [`sample_chip_at_delay`] exactly, and the caller falls back to it.
struct ChipWalk {
    first_index: usize,
    step: usize,
    fraction: f32,
}

impl ChipWalk {
    fn resolve(
        samples_len: usize,
        base_start: usize,
        oversample: usize,
        chips: usize,
        sample_delay: i32,
        sample_delay_fraction: f32,
    ) -> Option<Self> {
        if chips == 0 {
            return None;
        }
        let step = oversample.max(1);
        let base = base_start as i64 + i64::from(sample_delay);
        let position =
            |chip: usize| -> f32 { (base + (chip * step) as i64) as f32 + sample_delay_fraction };

        let first = position(0);
        let last = position(chips - 1);
        if !first.is_finite() || !last.is_finite() || first < 0.0 {
            return None;
        }
        let first_index = first.floor() as usize;
        let last_index = last.floor() as usize;
        if last_index + 1 >= samples_len {
            return None;
        }

        let fraction = first - first_index as f32;
        // Endpoints that agree pin every chip between them; they disagree once
        // the span crosses an f32 exponent and the positions round differently.
        if last_index != first_index + (chips - 1) * step || last - last_index as f32 != fraction {
            return None;
        }
        Some(Self {
            first_index,
            step,
            fraction,
        })
    }
}

/// Per-frame pilot phase + coherence estimate produced by accumulating across
/// the 16 slots after the RRI burst.
#[derive(Debug, Clone, Copy)]
pub struct PilotMoments {
    /// Unit-magnitude average pilot phase (`coherent / |coherent|`) across the
    /// frame. Pre-rotation; multiply by `conj()` to derotate the frame.
    pub pilot_phase: Complex32,
    /// Residual phase predicted at the start of the next 16-slot frame.
    /// The traffic finger folds this into the next despread pass so residual
    /// CFO does not accumulate frame to frame.
    pub next_frame_phase: Complex32,
    /// Linear residual phase at chip 0 of this frame, in radians.
    pub phase_at_frame_start_rad: f32,
    /// Residual phase slope in radians per 2048-chip slot.
    pub phase_step_rad_per_slot: f32,
    /// True when the phase fields came from a multi-slot least-squares ramp;
    /// false when they fell back to a single frame/slot phase estimate.
    pub phase_ramp_valid: bool,
    /// Per-slot noncoherent sum / per-chip incoherent sum. Always in [0, 1].
    pub coherence: f32,
    /// dB SNR estimate from `coherent_power / (mean_chip_power - coherent_power)`.
    pub snr_db: f32,
    /// RC3-style pilot-symbol SINR using the full complex residual. Unlike
    /// `snr_db`, this includes quadrature error instead of projecting it away.
    pub rc3_sinr_db: f32,
    /// Absolute change in mean despread pilot-symbol amplitude between the
    /// first and second halves of the observation. RC3 SINR assumes a
    /// stationary PCG/slot; a large value identifies an on/off envelope edge
    /// whose deterministic ramp would otherwise appear as noise variance.
    pub pilot_amplitude_step_db: f32,
    /// Coherent pilot symbol power (the SINR numerator). Should track total
    /// received power linearly if the pilot despread isolates the pilot.
    pub coherent_pilot_power: f32,
    /// Incoherent (noise + self-interference) power (the SINR denominator).
    pub noise_pilot_power: f32,
}

impl PilotMoments {
    /// Unit-magnitude residual phase at a chip offset within the frame.
    pub fn phase_at_chip(&self, chip_idx: usize) -> Complex32 {
        unit_phase(
            self.phase_at_frame_start_rad
                + self.phase_step_rad_per_slot * chip_idx as f32 / HRPD_SLOT_CHIPS as f32,
        )
    }
}

fn half_amplitude_step_db(
    first_sum: f32,
    first_count: usize,
    second_sum: f32,
    second_count: usize,
) -> f32 {
    if first_count == 0 || second_count == 0 {
        return f32::INFINITY;
    }
    let first_mean = first_sum / first_count as f32;
    let second_mean = second_sum / second_count as f32;
    20.0 * (first_mean.max(1.0e-12) / second_mean.max(1.0e-12))
        .log10()
        .abs()
}

fn unit_phase(phase_rad: f32) -> Complex32 {
    let (sin, cos) = phase_rad.sin_cos();
    Complex32::new(cos, sin)
}

fn phase_of(v: Complex32) -> f32 {
    v.im.atan2(v.re)
}

fn wrapped_phase_delta(delta: f32) -> f32 {
    let two_pi = std::f32::consts::TAU;
    let mut out = (delta + std::f32::consts::PI).rem_euclid(two_pi) - std::f32::consts::PI;
    if out <= -std::f32::consts::PI {
        out += two_pi;
    }
    out
}

#[derive(Debug, Default, Clone)]
pub(crate) struct PilotPhaseRampFit {
    prev_unwrapped: Option<f32>,
    sw: f32,
    sx: f32,
    sy: f32,
    sxx: f32,
    sxy: f32,
    n: usize,
}

impl PilotPhaseRampFit {
    pub(crate) fn push(&mut self, x: f32, value: Complex32) {
        let weight = value.norm();
        if weight <= 1.0e-12 {
            return;
        }
        let raw = phase_of(value);
        let unwrapped = if let Some(prev) = self.prev_unwrapped {
            prev + wrapped_phase_delta(raw - prev)
        } else {
            raw
        };
        self.prev_unwrapped = Some(unwrapped);
        self.sw += weight;
        self.sx += weight * x;
        self.sy += weight * unwrapped;
        self.sxx += weight * x * x;
        self.sxy += weight * x * unwrapped;
        self.n += 1;
    }

    pub(crate) fn finish(&self, fallback_phase: f32) -> (f32, f32, bool) {
        if self.n == 0 || self.sw <= 1.0e-12 {
            return (0.0, 0.0, false);
        }
        if self.n == 1 {
            return (self.prev_unwrapped.unwrap_or(fallback_phase), 0.0, false);
        }
        let denom = self.sw * self.sxx - self.sx * self.sx;
        if denom.abs() <= 1.0e-9 {
            return (fallback_phase, 0.0, false);
        }
        let slope = (self.sw * self.sxy - self.sx * self.sy) / denom;
        let intercept = (self.sy - slope * self.sx) / self.sw;
        (intercept, slope, true)
    }
}

const PILOT_REGION_SLOT_CENTER: f32 =
    (HRPD_PILOT_CLEAN_START_CHIPS as f32 + HRPD_SLOT_CHIPS as f32) / (2.0 * HRPD_SLOT_CHIPS as f32);
// Shared with the finger's phase tracker: stays below the ±π unwrap
// ambiguity while covering the live capture CFO range.
pub(crate) const HRPD_PILOT_PHASE_RAMP_MAX_STEP_RAD_PER_SLOT: f32 = 2.8;

/// Fit residual pilot phase as `phase_at_frame_start + slot * phase_step`.
/// Slot phase observations are taken at the center of each clean W0 pilot
/// region, after the RRI head. If the ramp is underdetermined, fall back to
/// the frame-average pilot phase with zero slope.
pub(crate) fn pilot_phase_ramp_from_slots(
    slot_coherent: &[Complex32; HRPD_TRAFFIC_SLOTS_PER_FRAME],
    coherent: Complex32,
) -> (f32, f32, bool) {
    let fallback_phase = phase_of(coherent);
    let mut fit = PilotPhaseRampFit::default();
    for (slot, value) in slot_coherent.iter().enumerate() {
        fit.push(slot as f32 + PILOT_REGION_SLOT_CENTER, *value);
    }
    clamp_pilot_phase_ramp(fit.finish(fallback_phase), fallback_phase)
}

/// Re-estimate the per-frame pilot phase from a despread chip stream by
/// noncoherently combining 16 per-slot sums of the unmodulated W0 pilot
/// region (chips 1024..2048 of each slot). The result is the new pilot phase
/// hypothesis to use for the *next* frame, or for re-derotating this frame.
///
/// The input `chips` is the post-PN/LC despread chip stream as produced by
/// [`despread_frame`] (pilot already rotated by the prior phase). The function
/// folds in the *residual* phase: a perfectly coherent frame produces a real
/// positive sum and a returned pilot_phase ≈ `1+0j`.
pub fn pilot_moments_from_despread(chips: &[Complex32]) -> PilotMoments {
    pilot_moments_from_clean_slot_regions(chips, HRPD_TRAFFIC_SLOTS_PER_FRAME)
}

/// Re-estimate the pilot phase from the ACK-free clean W0 pilot half of each
/// slot of a legacy 16-slot reverse traffic frame. The subtype-2 sub-frame
/// path uses [`pilot_moments_from_subtype2_slot_regions`] instead. The
/// returned phase ramp is relative to `chips[0]`.
pub(crate) fn pilot_moments_from_clean_slot_regions(
    chips: &[Complex32],
    slots: usize,
) -> PilotMoments {
    pilot_moments_from_slot_regions(chips, slots, HRPD_PILOT_CLEAN_START_CHIPS)
}

/// Re-estimate subtype-2/subtype-3 pilot phase from full slots.
///
/// Rev A reverse pilot is continuously present on W0; ACK/RRI/DSC are Walsh
/// orthogonal to W0 when integrated on aligned 16-chip boundaries. Using all
/// chips gives the 4-slot RRI/data path more pilot SNR than the legacy Rev 0
/// ACK-free half-slot metric.
pub(crate) fn pilot_moments_from_subtype2_slot_regions(
    chips: &[Complex32],
    slots: usize,
) -> PilotMoments {
    pilot_moments_from_slot_regions(chips, slots, 0)
}

fn pilot_moments_from_slot_regions(
    chips: &[Complex32],
    slots: usize,
    pilot_start_chips: usize,
) -> PilotMoments {
    let slots = slots.max(1);
    let max_chips = chips.len().min(slots.saturating_mul(HRPD_SLOT_CHIPS));
    let pilot_start_chips = pilot_start_chips.min(HRPD_SLOT_CHIPS);
    let mut coherent = Complex32::new(0.0, 0.0);
    let mut slot_coherent = vec![Complex32::new(0.0, 0.0); slots];
    let mut count = 0usize;
    for slot in 0..slots {
        let slot_start = slot * HRPD_SLOT_CHIPS + pilot_start_chips;
        if slot_start >= max_chips {
            continue;
        }
        let slot_end = ((slot + 1) * HRPD_SLOT_CHIPS).min(max_chips);
        for sample in &chips[slot_start..slot_end] {
            coherent += *sample;
            slot_coherent[slot] += *sample;
            count += 1;
        }
    }
    if count == 0 {
        return PilotMoments {
            pilot_phase: Complex32::new(1.0, 0.0),
            next_frame_phase: Complex32::new(1.0, 0.0),
            phase_at_frame_start_rad: 0.0,
            phase_step_rad_per_slot: 0.0,
            phase_ramp_valid: false,
            coherence: 0.0,
            snr_db: f32::NEG_INFINITY,
            rc3_sinr_db: f32::NEG_INFINITY,
            pilot_amplitude_step_db: f32::INFINITY,
            coherent_pilot_power: 0.0,
            noise_pilot_power: 0.0,
        };
    }
    let pilot_phase = if coherent.norm_sqr() > 0.0 {
        coherent / coherent.norm()
    } else {
        Complex32::new(1.0, 0.0)
    };
    let fallback_phase = phase_of(coherent);
    let mut fit = PilotPhaseRampFit::default();
    for (slot, value) in slot_coherent.iter().enumerate() {
        fit.push(slot as f32 + PILOT_REGION_SLOT_CENTER, *value);
    }
    let (phase_at_frame_start_rad, phase_step_rad_per_slot, phase_ramp_valid) =
        clamp_pilot_phase_ramp(fit.finish(fallback_phase), fallback_phase);

    let mut slot_projected = vec![0.0f32; slots];
    let mut abs_sum = 0.0f32;
    let mut power_sum = 0.0f32;
    let mut complex_sum = Complex32::new(0.0, 0.0);
    let mut complex_power_sum = 0.0f32;
    let mut symbol_count = 0usize;
    let total_symbol_count = (0..slots)
        .map(|slot| {
            let start = slot * HRPD_SLOT_CHIPS + pilot_start_chips;
            let end = ((slot + 1) * HRPD_SLOT_CHIPS).min(max_chips);
            end.saturating_sub(start) / HRPD_PILOT_WALSH_CHIPS
        })
        .sum::<usize>();
    let mut first_half_amplitude_sum = 0.0f32;
    let mut second_half_amplitude_sum = 0.0f32;
    let mut first_half_symbols = 0usize;
    let mut second_half_symbols = 0usize;
    let ramp_step_per_chip = -(phase_step_rad_per_slot as f64) / HRPD_SLOT_CHIPS as f64;
    for slot in 0..slots {
        let slot_start = slot * HRPD_SLOT_CHIPS + pilot_start_chips;
        if slot_start >= max_chips {
            continue;
        }
        let slot_end = ((slot + 1) * HRPD_SLOT_CHIPS).min(max_chips);
        // The region's chips are contiguous, so the conjugate ramp can
        // advance by phasor recurrence instead of per-chip sine/cosine.
        let mut ramp = crate::sdr::PhasorNco::with_start_phase(
            -(phase_at_frame_start_rad as f64
                + phase_step_rad_per_slot as f64 * slot_start as f64 / HRPD_SLOT_CHIPS as f64),
            ramp_step_per_chip,
        );
        let mut chip_idx = slot_start;
        while chip_idx + HRPD_PILOT_WALSH_CHIPS <= slot_end {
            let mut complex_symbol = Complex32::new(0.0, 0.0);
            for offset in 0..HRPD_PILOT_WALSH_CHIPS {
                complex_symbol += ramp.mix(chips[chip_idx + offset]);
            }
            let complex_symbol = complex_symbol / HRPD_PILOT_WALSH_CHIPS as f32;
            let symbol = complex_symbol.re;
            slot_projected[slot] += symbol;
            abs_sum += symbol.abs();
            power_sum += symbol * symbol;
            complex_sum += complex_symbol;
            complex_power_sum += complex_symbol.norm_sqr();
            if symbol_count < total_symbol_count / 2 {
                first_half_amplitude_sum += complex_symbol.norm();
                first_half_symbols += 1;
            } else {
                second_half_amplitude_sum += complex_symbol.norm();
                second_half_symbols += 1;
            }
            symbol_count += 1;
            chip_idx += HRPD_PILOT_WALSH_CHIPS;
        }
    }
    let noncoherent: f32 = slot_projected.iter().map(|s| s.abs()).sum();
    let coherence = (noncoherent / abs_sum.max(1.0e-12)).min(1.0);
    let mean_power = power_sum / symbol_count.max(1) as f32;
    let coherent_power =
        (noncoherent * noncoherent) / (symbol_count.max(1) * symbol_count.max(1)) as f32;
    let noise_power = (mean_power - coherent_power).max(1.0e-12);
    let snr_db = 10.0 * (coherent_power / noise_power).max(1.0e-12).log10();
    let complex_mean_power = complex_power_sum / symbol_count.max(1) as f32;
    let complex_coherent_power =
        complex_sum.norm_sqr() / (symbol_count.max(1) * symbol_count.max(1)) as f32;
    let complex_noise_power = (complex_mean_power - complex_coherent_power).max(1.0e-12);
    let rc3_sinr_db = 10.0
        * (complex_coherent_power / complex_noise_power)
            .max(1.0e-12)
            .log10();
    let pilot_amplitude_step_db = half_amplitude_step_db(
        first_half_amplitude_sum,
        first_half_symbols,
        second_half_amplitude_sum,
        second_half_symbols,
    );
    let next_frame_phase =
        unit_phase(phase_at_frame_start_rad + phase_step_rad_per_slot * slots as f32);
    PilotMoments {
        pilot_phase,
        next_frame_phase,
        phase_at_frame_start_rad,
        phase_step_rad_per_slot,
        phase_ramp_valid,
        coherence,
        snr_db,
        rc3_sinr_db,
        pilot_amplitude_step_db,
        coherent_pilot_power: coherent_power,
        noise_pilot_power: noise_power,
    }
}

fn clamp_pilot_phase_ramp(fit: (f32, f32, bool), fallback_phase: f32) -> (f32, f32, bool) {
    let (phase_at_frame_start_rad, phase_step_rad_per_slot, phase_ramp_valid) = fit;
    if phase_ramp_valid
        && phase_step_rad_per_slot.abs() > HRPD_PILOT_PHASE_RAMP_MAX_STEP_RAD_PER_SLOT
    {
        (fallback_phase, 0.0, false)
    } else {
        (
            phase_at_frame_start_rad,
            phase_step_rad_per_slot,
            phase_ramp_valid,
        )
    }
}

/// Produce one pilot quality estimate per reverse traffic slot. RPC decisions
/// are slot-based, so the receiver must not hold a single frame-average power
/// command across all future slots.
pub fn pilot_moments_by_slot_from_despread(
    chips: &[Complex32],
) -> [PilotMoments; HRPD_TRAFFIC_SLOTS_PER_FRAME] {
    std::array::from_fn(|slot| {
        let start = slot * HRPD_SLOT_CHIPS + HRPD_PILOT_CLEAN_START_CHIPS;
        let end = ((slot + 1) * HRPD_SLOT_CHIPS).min(chips.len());
        pilot_moments_from_slot(&chips[start.min(chips.len())..end])
    })
}

pub(crate) fn pilot_moments_from_slot(chips: &[Complex32]) -> PilotMoments {
    let mut coherent = Complex32::new(0.0, 0.0);
    let mut count = 0usize;
    for sample in chips {
        coherent += *sample;
        count += 1;
    }
    if count == 0 {
        return PilotMoments {
            pilot_phase: Complex32::new(1.0, 0.0),
            next_frame_phase: Complex32::new(1.0, 0.0),
            phase_at_frame_start_rad: 0.0,
            phase_step_rad_per_slot: 0.0,
            phase_ramp_valid: false,
            coherence: 0.0,
            snr_db: f32::NEG_INFINITY,
            rc3_sinr_db: f32::NEG_INFINITY,
            pilot_amplitude_step_db: f32::INFINITY,
            coherent_pilot_power: 0.0,
            noise_pilot_power: 0.0,
        };
    }
    let pilot_phase = if coherent.norm_sqr() > 0.0 {
        coherent / coherent.norm()
    } else {
        Complex32::new(1.0, 0.0)
    };
    let derot = pilot_phase.conj();
    let mut projected_sum = 0.0f32;
    let mut abs_sum = 0.0f32;
    let mut power_sum = 0.0f32;
    let mut complex_sum = Complex32::new(0.0, 0.0);
    let mut complex_power_sum = 0.0f32;
    let mut symbol_count = 0usize;
    let total_symbol_count = chips.len() / HRPD_PILOT_WALSH_CHIPS;
    let mut first_half_amplitude_sum = 0.0f32;
    let mut second_half_amplitude_sum = 0.0f32;
    let mut first_half_symbols = 0usize;
    let mut second_half_symbols = 0usize;
    for (symbol_index, chunk) in chips.chunks_exact(HRPD_PILOT_WALSH_CHIPS).enumerate() {
        let symbol = chunk.iter().map(|sample| (*sample * derot).re).sum::<f32>()
            / HRPD_PILOT_WALSH_CHIPS as f32;
        projected_sum += symbol;
        abs_sum += symbol.abs();
        power_sum += symbol * symbol;
        let complex_symbol =
            chunk.iter().copied().sum::<Complex32>() / HRPD_PILOT_WALSH_CHIPS as f32;
        complex_sum += complex_symbol;
        complex_power_sum += complex_symbol.norm_sqr();
        if symbol_index < total_symbol_count / 2 {
            first_half_amplitude_sum += complex_symbol.norm();
            first_half_symbols += 1;
        } else {
            second_half_amplitude_sum += complex_symbol.norm();
            second_half_symbols += 1;
        }
        symbol_count += 1;
    }
    let noncoherent = projected_sum.abs();
    let coherence = (noncoherent / abs_sum.max(1.0e-12)).min(1.0);
    let mean_power = power_sum / symbol_count.max(1) as f32;
    let coherent_power =
        (noncoherent * noncoherent) / (symbol_count.max(1) * symbol_count.max(1)) as f32;
    let noise_power = (mean_power - coherent_power).max(1.0e-12);
    let snr_db = 10.0 * (coherent_power / noise_power).max(1.0e-12).log10();
    let complex_mean_power = complex_power_sum / symbol_count.max(1) as f32;
    let complex_coherent_power =
        complex_sum.norm_sqr() / (symbol_count.max(1) * symbol_count.max(1)) as f32;
    let complex_noise_power = (complex_mean_power - complex_coherent_power).max(1.0e-12);
    let rc3_sinr_db = 10.0
        * (complex_coherent_power / complex_noise_power)
            .max(1.0e-12)
            .log10();
    let pilot_amplitude_step_db = half_amplitude_step_db(
        first_half_amplitude_sum,
        first_half_symbols,
        second_half_amplitude_sum,
        second_half_symbols,
    );
    let phase = phase_of(pilot_phase);
    PilotMoments {
        pilot_phase,
        next_frame_phase: pilot_phase,
        phase_at_frame_start_rad: phase,
        phase_step_rad_per_slot: 0.0,
        phase_ramp_valid: false,
        coherence,
        snr_db,
        rc3_sinr_db,
        pilot_amplitude_step_db,
        coherent_pilot_power: coherent_power,
        noise_pilot_power: noise_power,
    }
}
