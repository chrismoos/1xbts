//! Unit tests for the HRPD reverse-traffic finger and sub-chain processors.
//!
//! All tests synthesize post-despread chip streams directly so they do not
//! depend on live IQ captures. The finger test additionally exercises the
//! per-chip despread math by spreading a known pilot via the shared
//! reference-chip helpers and verifying the finger recovers it.

use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use cdma_common::hrpd::air::{HrpdTrafficEvent, default_reverse_traffic_long_code_masks};
use num_complex::Complex32;
use tokio::sync::mpsc as tokio_mpsc;

use crate::bts::hrpd::scheduler::HarqResponse;
use crate::bts::hrpd::{HarqBus, HarqEmissionEvent};
use crate::phy::walsh::WalshGenerator;
use crate::receiver::hrpd::ack_decoder::{
    ACK_CHIPS_PER_BIT, ACK_WALSH_INDEX, ACK_WALSH_LEN, ACK_WALSH_SYMBOLS_PER_BIT,
};
use crate::receiver::hrpd::reverse_spread::{
    HrpdReversePilotReferenceConfig, hrpd_reverse_pilot_reference_chips,
};
use crate::receiver::pipelined::generic_rake_receiver::RakeFinger;
use crate::receiver::pipelined::{PipelineProcessor, SampleBlock};

use super::ack_processor::{HrpdReverseTrafficAckProcessor, TAG_ACK_PATTERN_PACKED};
use super::data_processor::HrpdReverseTrafficDataProcessor;
use super::despread::{
    HRPD_PILOT_CLEAN_START_CHIPS, HRPD_REVERSE_TRAFFIC_PILOT_MIN_COHERENCE, HRPD_RRI_HEAD_CHIPS,
    HRPD_SLOT_CHIPS, HRPD_TRAFFIC_FRAME_CHIPS, HRPD_TRAFFIC_SLOTS_PER_FRAME, PilotMoments,
    pilot_moments_by_slot_from_despread, pilot_moments_from_clean_slot_regions,
    pilot_moments_from_despread, pilot_moments_from_slot, pilot_moments_from_subtype2_slot_regions,
};
use super::drc_processor::{DRC_SLOT_GATED_VALUE, HrpdReverseTrafficDrcProcessor, TAG_DRC_PACKED};
use super::finger::{
    HRPD_RPC_TARGET_SNR_DB, HrpdReverseTrafficFinger, HrpdReverseTrafficFingerConfig,
    HrpdReverseTrafficFingerLock, HrpdRpcController, TAG_DRC_COVER, TAG_DRC_LENGTH,
    TAG_FRAME_OFFSET, TAG_FRAME_START_CHIP, TAG_MAC_INDEX, TAG_PILOT_COHERENCE_X1000,
    TAG_Q_SIGN_X1000, TAG_UATI, derotate_frame_by_pilot_ramp,
};
use super::rri_processor::{
    HrpdReverseTrafficRriProcessor, TAG_RRI_MARGIN_DB_TENTHS, TAG_RRI_RATE_BPS,
};

// -- Finger test ------------------------------------------------------------

fn unit_phase(phase_rad: f32) -> Complex32 {
    let (sin, cos) = phase_rad.sin_cos();
    Complex32::new(cos, sin)
}

fn phase_delta_abs(a: Complex32, b: Complex32) -> f32 {
    let delta = (a * b.conj()).im.atan2((a * b.conj()).re);
    delta.abs()
}

#[test]
fn subframe_pilot_moments_ignore_rri_and_ack_slot_heads() {
    let clean_phase = unit_phase(0.35);
    let contaminated_head = unit_phase(2.30) * 5.0;
    let mut chips = vec![contaminated_head; HRPD_SLOT_CHIPS * 4];
    for slot in 0..4 {
        let start = slot * HRPD_SLOT_CHIPS + HRPD_PILOT_CLEAN_START_CHIPS;
        let end = (slot + 1) * HRPD_SLOT_CHIPS;
        for chip in &mut chips[start..end] {
            *chip = clean_phase;
        }
    }

    let contaminated_sum = chips.iter().copied().sum::<Complex32>();
    let contaminated_phase = contaminated_sum / contaminated_sum.norm();
    assert!(
        phase_delta_abs(contaminated_phase, clean_phase) > 0.40,
        "test setup must bias a full-subframe phase estimate"
    );

    let moments = pilot_moments_from_clean_slot_regions(&chips, 4);
    assert!(moments.coherence > 0.99);
    assert!(
        phase_delta_abs(moments.pilot_phase, clean_phase) < 0.01,
        "clean-pilot phase should ignore the contaminated slot head"
    );
    assert!(
        moments.phase_step_rad_per_slot.abs() < 0.01,
        "constant clean pilot should not create a phase ramp"
    );
}

#[test]
fn subtype2_subframe_pilot_moments_use_full_slot() {
    let clean_phase = unit_phase(0.20);
    let mut chips = vec![Complex32::new(0.0, 0.0); HRPD_SLOT_CHIPS * 4];
    for slot in 0..4 {
        let start = slot * HRPD_SLOT_CHIPS;
        let end = (slot + 1) * HRPD_SLOT_CHIPS;
        for chip in &mut chips[start..end] {
            *chip = clean_phase;
        }
    }

    let moments = pilot_moments_from_subtype2_slot_regions(&chips, 4);
    assert!(moments.coherence > 0.99);
    assert!(
        phase_delta_abs(moments.pilot_phase, clean_phase) < 0.01,
        "subtype-2 pilot metric should include the whole continuous W0 slot"
    );
    assert!(moments.snr_db.is_finite());
}

#[test]
fn derotate_by_pilot_ramp_matches_per_chip_sin_cos() {
    // A steep ramp near the per-slot clamp maximizes rotor drift between
    // phasor resyncs.
    let moments = PilotMoments {
        pilot_phase: Complex32::new(1.0, 0.0),
        next_frame_phase: Complex32::new(1.0, 0.0),
        phase_at_frame_start_rad: 0.4,
        phase_step_rad_per_slot: 2.5,
        phase_ramp_valid: true,
        coherence: 1.0,
        snr_db: 20.0,
        rc3_sinr_db: 20.0,
        pilot_amplitude_step_db: 0.0,
        coherent_pilot_power: 1.0,
        noise_pilot_power: 0.01,
    };
    let chips = (0..HRPD_TRAFFIC_FRAME_CHIPS)
        .map(|n| {
            Complex32::new(
                (n as f32 * 0.113).sin() * 1.2,
                (n as f32 * 0.071).cos() * 0.9,
            )
        })
        .collect::<Vec<_>>();

    let mut got = chips.clone();
    derotate_frame_by_pilot_ramp(&mut got, moments);

    let mut max_err = 0.0f32;
    for (chip_idx, (got, original)) in got.iter().zip(&chips).enumerate() {
        let want = original * moments.phase_at_chip(chip_idx).conj();
        max_err = max_err.max((got - want).norm());
    }
    assert!(
        max_err < 2e-4,
        "phasor derotation drifted from per-chip sin/cos: {max_err}"
    );
}

#[test]
fn pilot_moments_phasor_matches_sin_cos_reference() {
    const WALSH_CHIPS: usize = 16;
    let slots = 4;
    // The noise keeps snr_db in a realistic regime; a near-noiseless ramp
    // would measure only numerical residue.
    let ramp_start = 0.3f32;
    let ramp_step_per_slot = 1.7f32;
    let chips = (0..slots * HRPD_SLOT_CHIPS)
        .map(|idx| {
            let phase = ramp_start + ramp_step_per_slot * idx as f32 / HRPD_SLOT_CHIPS as f32;
            unit_phase(phase)
                + Complex32::new(
                    (idx as f32 * 0.213).sin() * 0.15,
                    (idx as f32 * 0.377).sin() * 0.15,
                )
        })
        .collect::<Vec<_>>();

    let moments = pilot_moments_from_subtype2_slot_regions(&chips, slots);

    // Recompute the Walsh-symbol statistics with direct per-chip sin/cos
    // at the function's own fitted ramp parameters.
    let mut slot_projected = vec![0.0f32; slots];
    let mut abs_sum = 0.0f32;
    let mut power_sum = 0.0f32;
    let mut complex_sum = Complex32::new(0.0, 0.0);
    let mut complex_power_sum = 0.0f32;
    let mut symbol_count = 0usize;
    for slot in 0..slots {
        let slot_start = slot * HRPD_SLOT_CHIPS;
        let slot_end = (slot + 1) * HRPD_SLOT_CHIPS;
        let mut chip_idx = slot_start;
        while chip_idx + WALSH_CHIPS <= slot_end {
            let mut complex_symbol = Complex32::new(0.0, 0.0);
            for offset in 0..WALSH_CHIPS {
                let idx = chip_idx + offset;
                let phase = moments.phase_at_frame_start_rad
                    + moments.phase_step_rad_per_slot * idx as f32 / HRPD_SLOT_CHIPS as f32;
                complex_symbol += chips[idx] * unit_phase(-phase);
            }
            let complex_symbol = complex_symbol / WALSH_CHIPS as f32;
            let symbol = complex_symbol.re;
            slot_projected[slot] += symbol;
            abs_sum += symbol.abs();
            power_sum += symbol * symbol;
            complex_sum += complex_symbol;
            complex_power_sum += complex_symbol.norm_sqr();
            symbol_count += 1;
            chip_idx += WALSH_CHIPS;
        }
    }
    let noncoherent: f32 = slot_projected.iter().map(|s| s.abs()).sum();
    let coherence = (noncoherent / abs_sum.max(1.0e-12)).min(1.0);
    let mean_power = power_sum / symbol_count as f32;
    let coherent_power = (noncoherent * noncoherent) / (symbol_count * symbol_count) as f32;
    let noise_power = (mean_power - coherent_power).max(1.0e-12);
    let snr_db = 10.0 * (coherent_power / noise_power).max(1.0e-12).log10();
    let complex_mean_power = complex_power_sum / symbol_count as f32;
    let complex_coherent_power = complex_sum.norm_sqr() / (symbol_count * symbol_count) as f32;
    let complex_noise_power = (complex_mean_power - complex_coherent_power).max(1.0e-12);
    let rc3_sinr_db = 10.0
        * (complex_coherent_power / complex_noise_power)
            .max(1.0e-12)
            .log10();

    assert!(
        (moments.coherence - coherence).abs() < 1e-4,
        "coherence {} vs reference {coherence}",
        moments.coherence
    );
    assert!(
        (moments.snr_db - snr_db).abs() < 0.01,
        "snr_db {} vs reference {snr_db}",
        moments.snr_db
    );
    assert!(
        (moments.rc3_sinr_db - rc3_sinr_db).abs() < 0.01,
        "rc3_sinr_db {} vs reference {rc3_sinr_db}",
        moments.rc3_sinr_db
    );
}

#[test]
fn rc3_style_pilot_sinr_is_scale_invariant_and_counts_quadrature_error() {
    let chips = (0..128)
        .flat_map(|symbol| {
            let quadrature_error = if symbol & 1 == 0 { 0.4 } else { -0.4 };
            [Complex32::new(1.0, quadrature_error); 16]
        })
        .collect::<Vec<_>>();
    let scaled = chips.iter().map(|sample| *sample * 0.1).collect::<Vec<_>>();

    let moments = pilot_moments_from_slot(&chips);
    let scaled_moments = pilot_moments_from_slot(&scaled);

    assert!(moments.snr_db > moments.rc3_sinr_db + 20.0);
    assert!((moments.rc3_sinr_db - scaled_moments.rc3_sinr_db).abs() < 0.01);
    assert!(moments.pilot_amplitude_step_db < 0.01);
}

#[test]
fn rc3_style_pilot_sinr_identifies_nonstationary_on_edge() {
    let chips = (0..128)
        .flat_map(|symbol| {
            let amplitude = if symbol < 64 { 0.1 } else { 1.0 };
            [Complex32::new(amplitude, 0.0); 16]
        })
        .collect::<Vec<_>>();

    let moments = pilot_moments_from_slot(&chips);

    assert!(moments.coherence > 0.99);
    assert!(moments.pilot_amplitude_step_db > 19.0);
    assert!(
        moments.rc3_sinr_db < 2.0,
        "the undetected envelope edge would look like poor SINR: {}",
        moments.rc3_sinr_db
    );
}

fn synthesize_pilot_only_frame_samples(
    uati: u32,
    frame_start_chip: u64,
) -> (Vec<Complex32>, u64, u64) {
    // Build the per-chip composite spreading reference and treat it as the
    // transmitted (pre-channel) waveform. Then the finger's despread step
    // multiplies by `ref_chip.conj()` and the pilot region collapses to
    // `1+0j` per chip, with zero RRI/data modulation.
    let (i_mask, q_mask) = default_reverse_traffic_long_code_masks(uati);
    let chips = hrpd_reverse_pilot_reference_chips(HrpdReversePilotReferenceConfig {
        start_chip: frame_start_chip,
        len: HRPD_TRAFFIC_FRAME_CHIPS,
        i_mask,
        q_mask,
        reference_chip_offset: 0,
        pn_phase_offset_chips: 0,
        lc_phase_offset_chips: 0,
        q_sign: -1.0,
        q_pair_phase: 0,
    });
    // Pad trailing samples so the chip interpolator and the steady-state
    // timing tracker can safely score positive delay candidates.
    let mut samples = chips;
    samples.extend([Complex32::new(0.0, 0.0); 64]);
    (samples, i_mask, q_mask)
}

fn synthesize_noise_samples(len: usize) -> Vec<Complex32> {
    let mut state = 0x1357_2468u32;
    let mut next = || {
        state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        ((state >> 8) as f32 / 16_777_216.0) - 0.5
    };
    (0..len).map(|_| Complex32::new(next(), next())).collect()
}

#[test]
fn finger_emits_pilot_locked_block_for_pilot_only_frame() {
    let uati = 0x0123_4567;
    let frame_start_chip = 0u64;
    let (samples, i_mask, q_mask) = synthesize_pilot_only_frame_samples(uati, frame_start_chip);
    let config = HrpdReverseTrafficFingerConfig {
        uati,
        mac_index: 5,
        physical_layer_subtype: 0,
        reverse_traffic_mac_subtype: 0,
        frame_offset: 0,
        i_mask,
        q_mask,
        drc_cover: 3,
        drc_length: 1,
        oversample: 1,
        event_tx: None,
        harq_bus: None,
        reverse_pilot_acquired: None,
        worker_spawned_at: std::time::Instant::now(),
    };
    let lock = HrpdReverseTrafficFingerLock {
        frame_start_chip,
        chip_offset: 0,
        sample_delay: 0,
        sample_delay_fraction: 0.0,
        q_sign: -1.0,
        q_pair_phase: 0,
        initial_pilot_phase: Complex32::new(1.0, 0.0),
    };
    let mut finger = HrpdReverseTrafficFinger::new(1, config, lock);
    let block = SampleBlock {
        samples,
        chip_start: 0,
        sample_rate_hz: 1_228_800.0,
        rx_sample_time: None,
        tags: Default::default(),
        pcg_signal_snr_db: None,
        active_pcg_mask: None,
        pcg_pilot_metrics: None,
    };
    let mut chain = Vec::new();
    let out = finger.process(&block, &mut chain);
    assert_eq!(out.len(), 1, "expected one despread frame block");
    let frame = &out[0];
    let coherence_x1000 = frame.tags.get(TAG_PILOT_COHERENCE_X1000).copied().unwrap();
    let coherence = coherence_x1000 as f32 / 1000.0;
    assert!(
        coherence >= HRPD_REVERSE_TRAFFIC_PILOT_MIN_COHERENCE,
        "expected pilot-locked coherence, got {coherence:.3}"
    );
    // Despread chip count matches a full frame.
    assert_eq!(frame.samples.len(), HRPD_TRAFFIC_FRAME_CHIPS);
    // Tags carry the expected per-frame metadata.
    assert_eq!(frame.tags.get(TAG_UATI).copied().unwrap(), uati as i64);
    assert_eq!(frame.tags.get(TAG_MAC_INDEX).copied().unwrap(), 5);
    assert_eq!(
        frame.tags.get(TAG_FRAME_START_CHIP).copied().unwrap(),
        frame_start_chip as i64
    );
    assert_eq!(frame.tags.get(TAG_DRC_COVER).copied().unwrap(), 3);
    assert_eq!(frame.tags.get(TAG_DRC_LENGTH).copied().unwrap(), 1);
}

#[test]
fn finger_refines_stale_sample_delay_before_declaring_pilot_lost() {
    let uati = 0x0123_4567;
    let frame_start_chip = 0u64;
    let (samples, i_mask, q_mask) = synthesize_pilot_only_frame_samples(uati, frame_start_chip);
    let mut delayed_samples = vec![Complex32::new(0.0, 0.0); 2];
    delayed_samples.extend(samples);
    let config = HrpdReverseTrafficFingerConfig {
        uati,
        mac_index: 5,
        physical_layer_subtype: 0,
        reverse_traffic_mac_subtype: 0,
        frame_offset: 0,
        i_mask,
        q_mask,
        drc_cover: 3,
        drc_length: 1,
        oversample: 1,
        event_tx: None,
        harq_bus: None,
        reverse_pilot_acquired: None,
        worker_spawned_at: std::time::Instant::now(),
    };
    let lock = HrpdReverseTrafficFingerLock {
        frame_start_chip,
        chip_offset: 0,
        sample_delay: 0,
        sample_delay_fraction: 0.0,
        q_sign: -1.0,
        q_pair_phase: 0,
        initial_pilot_phase: Complex32::new(1.0, 0.0),
    };
    let mut finger = HrpdReverseTrafficFinger::new(1, config, lock);
    let block = SampleBlock {
        samples: delayed_samples,
        chip_start: 0,
        sample_rate_hz: 1_228_800.0,
        rx_sample_time: None,
        tags: Default::default(),
        pcg_signal_snr_db: None,
        active_pcg_mask: None,
        pcg_pilot_metrics: None,
    };
    let mut chain = Vec::new();
    let out = finger.process(&block, &mut chain);
    assert_eq!(out.len(), 1, "expected one recovered despread frame block");
    let frame = &out[0];
    let coherence_x1000 = frame.tags.get(TAG_PILOT_COHERENCE_X1000).copied().unwrap();
    let coherence = coherence_x1000 as f32 / 1000.0;
    assert!(
        coherence >= HRPD_REVERSE_TRAFFIC_PILOT_MIN_COHERENCE,
        "expected timing refinement to recover pilot lock, got {coherence:.3}"
    );
    assert!(
        finger.describe().contains("delay=2++0.00"),
        "expected timing loop to update the sample delay, got {}",
        finger.describe()
    );
}

#[test]
fn rpc_slot_timing_refines_before_power_decision() {
    let uati = 0x0123_4567;
    let frame_start_chip = 0u64;
    let (samples, i_mask, q_mask) = synthesize_pilot_only_frame_samples(uati, frame_start_chip);
    let mut delayed_samples = vec![Complex32::new(0.0, 0.0); 2];
    delayed_samples.extend(samples);
    let mac_index = 5u8;
    let bus = Arc::new(HarqBus::new());
    let config = HrpdReverseTrafficFingerConfig {
        uati,
        mac_index,
        physical_layer_subtype: 0,
        reverse_traffic_mac_subtype: 0,
        frame_offset: 0,
        i_mask,
        q_mask,
        drc_cover: 3,
        drc_length: 1,
        oversample: 1,
        event_tx: None,
        harq_bus: Some(bus.clone()),
        reverse_pilot_acquired: None,
        worker_spawned_at: std::time::Instant::now(),
    };
    let lock = HrpdReverseTrafficFingerLock {
        frame_start_chip,
        chip_offset: 0,
        sample_delay: 0,
        sample_delay_fraction: 0.0,
        q_sign: -1.0,
        q_pair_phase: 0,
        initial_pilot_phase: Complex32::new(1.0, 0.0),
    };
    let mut finger = HrpdReverseTrafficFinger::new(1, config, lock);
    let block = SampleBlock {
        samples: delayed_samples,
        chip_start: 0,
        sample_rate_hz: 1_228_800.0,
        rx_sample_time: None,
        tags: Default::default(),
        pcg_signal_snr_db: None,
        active_pcg_mask: None,
        pcg_pilot_metrics: None,
    };
    let mut chain = Vec::new();
    let _ = finger.process(&block, &mut chain);

    assert!(
        finger.describe().contains("delay=2++0.00"),
        "expected RPC timing loop to update the sample delay before frame tracking, got {}",
        finger.describe()
    );

    let mut up = 0;
    let mut down = 0;
    for slot in 0..128u64 {
        match bus.rpc_at_slot(mac_index, slot) {
            Some(0) => up += 1,
            Some(1) => down += 1,
            _ => {}
        }
    }
    assert!(down + up > 0, "RPC loop should have scheduled bits");
    assert!(
        down > up,
        "a corrected clean pilot should command net DOWN, got up={up} down={down}"
    );
}

#[test]
fn rpc_slot_timing_does_not_chase_unlocked_noise() {
    let uati = 0x0123_4567;
    let frame_start_chip = 0u64;
    let (samples, i_mask, q_mask) = synthesize_pilot_only_frame_samples(uati, frame_start_chip);
    let mut delayed_samples = vec![Complex32::new(0.0, 0.0); 2];
    delayed_samples.extend(samples);
    let mac_index = 5u8;
    let bus = Arc::new(HarqBus::new());
    let config = HrpdReverseTrafficFingerConfig {
        uati,
        mac_index,
        physical_layer_subtype: 0,
        reverse_traffic_mac_subtype: 0,
        frame_offset: 0,
        i_mask,
        q_mask,
        drc_cover: 3,
        drc_length: 1,
        oversample: 1,
        event_tx: None,
        harq_bus: Some(bus),
        reverse_pilot_acquired: None,
        worker_spawned_at: std::time::Instant::now(),
    };
    let lock = HrpdReverseTrafficFingerLock {
        frame_start_chip,
        chip_offset: 0,
        sample_delay: 0,
        sample_delay_fraction: 0.0,
        q_sign: -1.0,
        q_pair_phase: 0,
        initial_pilot_phase: Complex32::new(1.0, 0.0),
    };
    let mut finger = HrpdReverseTrafficFinger::new(1, config, lock);
    let mut chain = Vec::new();
    let clean = SampleBlock {
        samples: delayed_samples,
        chip_start: 0,
        sample_rate_hz: 1_228_800.0,
        rx_sample_time: None,
        tags: Default::default(),
        pcg_signal_snr_db: None,
        active_pcg_mask: None,
        pcg_pilot_metrics: None,
    };
    let _ = finger.process(&clean, &mut chain);
    assert!(
        finger.describe().contains("delay=2++0.00"),
        "test setup should first recover the delayed pilot, got {}",
        finger.describe()
    );

    let noise = SampleBlock {
        samples: synthesize_noise_samples(HRPD_TRAFFIC_FRAME_CHIPS + 64),
        chip_start: HRPD_TRAFFIC_FRAME_CHIPS,
        sample_rate_hz: 1_228_800.0,
        rx_sample_time: None,
        tags: Default::default(),
        pcg_signal_snr_db: None,
        active_pcg_mask: None,
        pcg_pilot_metrics: None,
    };
    let _ = finger.process(&noise, &mut chain);
    assert!(
        finger.describe().contains("delay=2++0.00"),
        "timing search must not move delay on unlocked noise, got {}",
        finger.describe()
    );
}

#[test]
fn finger_reports_reverse_pilot_lost_after_timeout() {
    let uati = 0x0123_4567;
    let mac_index = 5;
    let frame_start_chip = 0u64;
    let (samples, i_mask, q_mask) = synthesize_pilot_only_frame_samples(uati, frame_start_chip);
    let (tx, mut rx) = tokio_mpsc::unbounded_channel();
    let acquired = Arc::new(AtomicBool::new(false));
    let config = HrpdReverseTrafficFingerConfig {
        uati,
        mac_index,
        physical_layer_subtype: 0,
        reverse_traffic_mac_subtype: 0,
        frame_offset: 0,
        i_mask,
        q_mask,
        drc_cover: 3,
        drc_length: 1,
        oversample: 1,
        event_tx: Some(tx),
        harq_bus: None,
        reverse_pilot_acquired: Some(acquired.clone()),
        worker_spawned_at: std::time::Instant::now(),
    };
    let lock = HrpdReverseTrafficFingerLock {
        frame_start_chip,
        chip_offset: 0,
        sample_delay: 0,
        sample_delay_fraction: 0.0,
        q_sign: -1.0,
        q_pair_phase: 0,
        initial_pilot_phase: Complex32::new(1.0, 0.0),
    };
    let mut finger = HrpdReverseTrafficFinger::new(1, config, lock);
    let mut chain = Vec::new();
    let first = SampleBlock {
        samples,
        chip_start: frame_start_chip as usize,
        sample_rate_hz: 1_228_800.0,
        rx_sample_time: None,
        tags: Default::default(),
        pcg_signal_snr_db: None,
        active_pcg_mask: None,
        pcg_pilot_metrics: None,
    };
    let _ = finger.process(&first, &mut chain);
    assert!(
        matches!(rx.try_recv(), Ok(HrpdTrafficEvent::ReversePilot { .. })),
        "clean first frame should report reverse-pilot acquisition"
    );
    assert!(acquired.load(Ordering::Acquire));

    let reacquire_timeout_chips = 1_228_800u64 / 2;
    let pilot_loss_timeout_chips = 5 * 1_228_800u64;
    let low_frames = pilot_loss_timeout_chips.div_ceil(HRPD_TRAFFIC_FRAME_CHIPS as u64);
    let mut lost = None;
    let mut requested_reacquisition = false;
    for frame in 1..=low_frames {
        let chip_start = frame_start_chip + frame * HRPD_TRAFFIC_FRAME_CHIPS as u64;
        let block = SampleBlock {
            samples: vec![Complex32::new(0.0, 0.0); HRPD_TRAFFIC_FRAME_CHIPS + 64],
            chip_start: chip_start as usize,
            sample_rate_hz: 1_228_800.0,
            rx_sample_time: None,
            tags: Default::default(),
            pcg_signal_snr_db: None,
            active_pcg_mask: None,
            pcg_pilot_metrics: None,
        };
        let _ = finger.process(&block, &mut chain);
        match rx.try_recv() {
            Ok(HrpdTrafficEvent::ReversePilotLost {
                uati: event_uati,
                mac_index: event_mac,
                last_good_chip,
                lost_at_chip,
                lost_chips,
                ..
            }) => {
                lost = Some((
                    event_uati,
                    event_mac,
                    last_good_chip,
                    lost_at_chip,
                    lost_chips,
                ));
                break;
            }
            Ok(other) => panic!("unexpected traffic event after acquisition: {other:?}"),
            Err(tokio_mpsc::error::TryRecvError::Empty) => {}
            Err(err) => panic!("traffic event channel closed: {err:?}"),
        }
        if finger.signal_lost_chips() >= reacquire_timeout_chips {
            requested_reacquisition = true;
            assert!(
                finger.should_retire(),
                "a stale tracking finger should retire and rearm acquisition before channel-loss supervision"
            );
        }
    }

    assert!(requested_reacquisition);
    let (event_uati, event_mac, last_good_chip, _lost_at_chip, lost_chips) =
        lost.expect("expected ReversePilotLost after five seconds below lock threshold");
    assert_eq!(event_uati, uati);
    assert_eq!(event_mac, mac_index);
    assert_eq!(last_good_chip, frame_start_chip);
    assert!(lost_chips >= pilot_loss_timeout_chips);
    assert!(!acquired.load(Ordering::Acquire));
    assert!(finger.should_retire());
}

#[test]
fn finger_reports_reverse_pilot_telemetry_while_locked() {
    let uati = 0x0123_4567;
    let mac_index = 5;
    let frame_start_chip = 0u64;
    let (samples, i_mask, q_mask) = synthesize_pilot_only_frame_samples(uati, frame_start_chip);
    let (tx, mut rx) = tokio_mpsc::unbounded_channel();
    let config = HrpdReverseTrafficFingerConfig {
        uati,
        mac_index,
        physical_layer_subtype: 0,
        reverse_traffic_mac_subtype: 0,
        frame_offset: 0,
        i_mask,
        q_mask,
        drc_cover: 3,
        drc_length: 1,
        oversample: 1,
        event_tx: Some(tx),
        harq_bus: None,
        reverse_pilot_acquired: None,
        worker_spawned_at: std::time::Instant::now(),
    };
    let lock = HrpdReverseTrafficFingerLock {
        frame_start_chip,
        chip_offset: 0,
        sample_delay: 0,
        sample_delay_fraction: 0.0,
        q_sign: -1.0,
        q_pair_phase: 0,
        initial_pilot_phase: Complex32::new(1.0, 0.0),
    };
    let mut finger = HrpdReverseTrafficFinger::new(1, config, lock);
    let mut chain = Vec::new();
    let first = SampleBlock {
        samples,
        chip_start: frame_start_chip as usize,
        sample_rate_hz: 1_228_800.0,
        rx_sample_time: None,
        tags: Default::default(),
        pcg_signal_snr_db: None,
        active_pcg_mask: None,
        pcg_pilot_metrics: None,
    };
    let _ = finger.process(&first, &mut chain);
    assert!(
        matches!(rx.try_recv(), Ok(HrpdTrafficEvent::ReversePilot { .. })),
        "clean first frame should report reverse-pilot acquisition"
    );

    let report_interval_frames = 1_228_800u64.div_ceil(HRPD_TRAFFIC_FRAME_CHIPS as u64);
    let mut telemetry = None;
    for frame in 1..=report_interval_frames {
        let chip_start = frame_start_chip + frame * HRPD_TRAFFIC_FRAME_CHIPS as u64;
        let (samples, _, _) = synthesize_pilot_only_frame_samples(uati, chip_start);
        let block = SampleBlock {
            samples,
            chip_start: chip_start as usize,
            sample_rate_hz: 1_228_800.0,
            rx_sample_time: None,
            tags: Default::default(),
            pcg_signal_snr_db: None,
            active_pcg_mask: None,
            pcg_pilot_metrics: None,
        };
        let _ = finger.process(&block, &mut chain);
        match rx.try_recv() {
            Ok(HrpdTrafficEvent::ReversePilot {
                uati: event_uati,
                mac_index: event_mac,
                absolute_chip,
                snr_db_tenths,
            }) => {
                telemetry = Some((event_uati, event_mac, absolute_chip, snr_db_tenths));
                break;
            }
            Ok(other) => panic!("unexpected traffic event while locked: {other:?}"),
            Err(tokio_mpsc::error::TryRecvError::Empty) => {}
            Err(err) => panic!("traffic event channel closed: {err:?}"),
        }
    }

    let (event_uati, event_mac, absolute_chip, snr_db_tenths) =
        telemetry.expect("locked finger should refresh reverse-pilot telemetry");
    assert_eq!(event_uati, uati);
    assert_eq!(event_mac, mac_index);
    assert!(absolute_chip >= 1_228_800);
    assert!(snr_db_tenths > 0);
}

#[test]
fn pilot_metric_rejects_walsh_orthogonal_data_energy() {
    const W2_4: [f32; 4] = [1.0, 1.0, -1.0, -1.0];
    let mut chips = vec![Complex32::new(0.0, 0.0); HRPD_TRAFFIC_FRAME_CHIPS];
    for slot in 0..HRPD_TRAFFIC_SLOTS_PER_FRAME {
        let base = slot * HRPD_SLOT_CHIPS;
        for chip in HRPD_RRI_HEAD_CHIPS..HRPD_SLOT_CHIPS {
            let idx = base + chip;
            let data_cover = W2_4[chip & 0x03];
            chips[idx] = Complex32::new(1.0, 8.0 * data_cover);
        }
    }

    let frame_moments = pilot_moments_from_despread(&chips);
    assert!(
        frame_moments.coherence > 0.999,
        "W0 pilot coherence should reject W2 leakage, got {:.3}",
        frame_moments.coherence
    );
    assert!(
        frame_moments.snr_db > 90.0,
        "W0 pilot SNR should not collapse on Walsh-orthogonal data, got {:.1}dB",
        frame_moments.snr_db
    );

    let slot_moments = pilot_moments_by_slot_from_despread(&chips);
    assert!(
        slot_moments
            .iter()
            .all(|moment| moment.coherence > 0.999 && moment.snr_db > 90.0),
        "slot RPC moments should also reject Walsh-orthogonal data: {slot_moments:?}"
    );
}

#[test]
fn pilot_metric_rejects_i_arm_walsh_orthogonal_leakage() {
    const W2_4: [f32; 4] = [1.0, 1.0, -1.0, -1.0];
    let mut chips = vec![Complex32::new(0.0, 0.0); HRPD_TRAFFIC_FRAME_CHIPS];
    for slot in 0..HRPD_TRAFFIC_SLOTS_PER_FRAME {
        let base = slot * HRPD_SLOT_CHIPS;
        for chip in HRPD_RRI_HEAD_CHIPS..HRPD_SLOT_CHIPS {
            let idx = base + chip;
            let data_cover = W2_4[chip & 0x03];
            chips[idx] = Complex32::new(1.0 + 0.9 * data_cover, 0.0);
        }
    }

    let frame_moments = pilot_moments_from_despread(&chips);
    assert!(
        frame_moments.coherence > 0.999,
        "W0 pilot coherence should reject I-arm W2 leakage, got {:.3}",
        frame_moments.coherence
    );
    assert!(
        frame_moments.snr_db > 90.0,
        "W0 pilot SNR should Walsh-average I-arm W2 leakage, got {:.1}dB",
        frame_moments.snr_db
    );

    let slot_moments = pilot_moments_by_slot_from_despread(&chips);
    assert!(
        slot_moments
            .iter()
            .all(|moment| moment.coherence > 0.999 && moment.snr_db > 90.0),
        "slot RPC moments should reject I-arm W2 leakage: {slot_moments:?}"
    );
}

#[test]
fn pilot_metric_uses_ack_free_half_slot() {
    let mut chips = vec![Complex32::new(0.0, 0.0); HRPD_TRAFFIC_FRAME_CHIPS];
    let ack_chips = synthesize_ack_slot_chips(8.0);
    for slot in 0..HRPD_TRAFFIC_SLOTS_PER_FRAME {
        let base = slot * HRPD_SLOT_CHIPS;
        for chip in HRPD_RRI_HEAD_CHIPS..HRPD_SLOT_CHIPS {
            chips[base + chip] = Complex32::new(1.0, 0.0);
        }
        for (chip, ack) in ack_chips.iter().enumerate().take(ACK_CHIPS_PER_BIT) {
            chips[base + chip] += *ack;
        }
    }

    let frame_moments = pilot_moments_from_despread(&chips);
    assert!(
        frame_moments.coherence > 0.999,
        "ACK half-slot energy must not depress pilot coherence, got {:.3}",
        frame_moments.coherence
    );
    assert!(
        frame_moments.snr_db > 90.0,
        "ACK half-slot energy must not be counted as pilot noise, got {:.1}dB",
        frame_moments.snr_db
    );

    let slot_moments = pilot_moments_by_slot_from_despread(&chips);
    assert!(
        slot_moments
            .iter()
            .all(|moment| moment.coherence > 0.999 && moment.snr_db > 90.0),
        "slot RPC moments should use the ACK-free tail: {slot_moments:?}"
    );

    let first_tail_chip = HRPD_PILOT_CLEAN_START_CHIPS;
    assert_eq!(
        chips[first_tail_chip],
        Complex32::new(1.0, 0.0),
        "test fixture must leave the clean pilot tail unmodified"
    );
}

// -- RRI processor test -----------------------------------------------------

/// Build a synthetic despread frame whose RRI burst chips encode `symbol`.
/// All pilot chips (after the RRI head) are filled with the chip mean +1.
fn synthesize_rri_only_frame(symbol_codeword: [u8; 7]) -> Vec<Complex32> {
    let mut chips = vec![Complex32::new(0.0, 0.0); HRPD_TRAFFIC_FRAME_CHIPS];
    for slot in 0..HRPD_TRAFFIC_SLOTS_PER_FRAME {
        let slot_base = slot * HRPD_SLOT_CHIPS;
        // 16 length-16 W0 symbols within the RRI head, 256 symbols total per
        // frame, repeated 37 times and punctured per spec. The processor sums
        // each 16-chip block as one soft symbol; encode bit -> +/-1 chips.
        for symbol_idx in 0..16 {
            let absolute_symbol_idx = slot * 16 + symbol_idx;
            let bit = symbol_codeword[absolute_symbol_idx % 7];
            let chip_val = if bit == 0 { 1.0 } else { -1.0 };
            let base = slot_base + symbol_idx * 16;
            for chip in 0..16 {
                chips[base + chip] = Complex32::new(chip_val, 0.0);
            }
        }
        // Fill the pilot region with +1 so the RRI burst dominates the
        // soft-symbol denominator.
        for chip in HRPD_RRI_HEAD_CHIPS..HRPD_SLOT_CHIPS {
            chips[slot_base + chip] = Complex32::new(1.0, 0.0);
        }
    }
    chips
}

#[test]
fn rri_processor_tags_detected_rate_for_kbps9_6() {
    // Kbps9_6 codeword from C.S0024-0 Table 9.2.1.3.3.2-1.
    let frame = synthesize_rri_only_frame([1, 0, 1, 0, 1, 0, 1]);
    let mut block = SampleBlock {
        samples: frame,
        chip_start: 0,
        sample_rate_hz: 1_228_800.0,
        rx_sample_time: None,
        tags: Default::default(),
        pcg_signal_snr_db: None,
        active_pcg_mask: None,
        pcg_pilot_metrics: None,
    };
    block.tags.insert(TAG_UATI, 1);
    block.tags.insert(TAG_MAC_INDEX, 1);
    let mut proc = HrpdReverseTrafficRriProcessor::new();
    let out = proc.process_block(block);
    assert_eq!(out.len(), 1);
    let tagged = &out[0];
    assert_eq!(
        tagged.tags.get(TAG_RRI_RATE_BPS).copied().unwrap(),
        9_600,
        "expected 9.6 kbps RRI detection"
    );
    let margin = tagged.tags.get(TAG_RRI_MARGIN_DB_TENTHS).copied().unwrap();
    assert!(margin > 0, "expected positive RRI decode margin");
}

// -- ACK processor test -----------------------------------------------------

fn synthesize_ack_slot_chips(bit: f32) -> Vec<Complex32> {
    let spreader =
        WalshGenerator::new::<ACK_WALSH_LEN>(ACK_WALSH_INDEX as usize, ACK_WALSH_SYMBOLS_PER_BIT);
    spreader.feed(Complex32::new(bit, 0.0))
}

#[test]
fn ack_processor_publishes_feedback_for_expected_slot() {
    // Build a frame whose slot 7 carries an ACK bit; all other chips zero.
    let mut frame = vec![Complex32::new(0.0, 0.0); HRPD_TRAFFIC_FRAME_CHIPS];
    let ack_slot_idx = 7usize;
    let ack_chips = synthesize_ack_slot_chips(1.0); // ACK
    let slot_base = ack_slot_idx * HRPD_SLOT_CHIPS;
    for (k, sample) in ack_chips.iter().enumerate().take(ACK_CHIPS_PER_BIT) {
        frame[slot_base + k] = *sample;
    }
    let mut block = SampleBlock {
        samples: frame,
        chip_start: 0,
        sample_rate_hz: 1_228_800.0,
        rx_sample_time: None,
        tags: Default::default(),
        pcg_signal_snr_db: None,
        active_pcg_mask: None,
        pcg_pilot_metrics: None,
    };
    let frame_start_chip = 0i64;
    let frame_start_slot = 0u64;
    let mac_index = 11u8;
    block.tags.insert(TAG_FRAME_START_CHIP, frame_start_chip);
    block.tags.insert(TAG_MAC_INDEX, mac_index as i64);

    let bus = Arc::new(HarqBus::new());
    bus.publish_emission(
        mac_index,
        HarqEmissionEvent {
            packet_id: 2,
            subpacket: 2,
            packet_start_slot: frame_start_slot + ack_slot_idx as u64 - 3,
            forward_slot: frame_start_slot + ack_slot_idx as u64 - 3,
            expected_ack_reverse_slot: frame_start_slot + ack_slot_idx as u64,
            terminal: true,
        },
    );

    let mut proc = HrpdReverseTrafficAckProcessor::new(Some(bus.clone()), None);
    let out = proc.process_block(block);
    assert_eq!(out.len(), 1);
    let packed = out[0].tags.get(TAG_ACK_PATTERN_PACKED).copied().unwrap() as u32;
    let slot_bits = (packed >> (ack_slot_idx * 2)) & 0b11;
    assert_eq!(slot_bits, 0b10, "expected ACK state for slot 7");

    let feedback = bus.drain_feedback(mac_index);
    assert_eq!(feedback.len(), 1);
    assert_eq!(feedback[0].packet_id, 2);
    assert_eq!(feedback[0].subpacket, 2);
    assert_eq!(feedback[0].response, HarqResponse::Ack);
}

#[test]
fn ack_processor_suppresses_intermediate_nak_feedback() {
    let mac_index = 12u8;
    let ack_slot_idx = 4usize;
    let frame_start_slot = 0u64;
    let mut frame = vec![Complex32::new(0.0, 0.0); HRPD_TRAFFIC_FRAME_CHIPS];
    let ack_chips = synthesize_ack_slot_chips(-1.0); // NAK
    let slot_base = ack_slot_idx * HRPD_SLOT_CHIPS;
    for (k, sample) in ack_chips.iter().enumerate().take(ACK_CHIPS_PER_BIT) {
        frame[slot_base + k] = *sample;
    }
    let mut block = SampleBlock {
        samples: frame,
        chip_start: 0,
        sample_rate_hz: 1_228_800.0,
        rx_sample_time: None,
        tags: Default::default(),
        pcg_signal_snr_db: None,
        active_pcg_mask: None,
        pcg_pilot_metrics: None,
    };
    block.tags.insert(TAG_FRAME_START_CHIP, 0);
    block.tags.insert(TAG_MAC_INDEX, mac_index as i64);

    let bus = Arc::new(HarqBus::new());
    bus.publish_emission(
        mac_index,
        HarqEmissionEvent {
            packet_id: 3,
            subpacket: 0,
            packet_start_slot: frame_start_slot + ack_slot_idx as u64 - 3,
            forward_slot: frame_start_slot + ack_slot_idx as u64 - 3,
            expected_ack_reverse_slot: frame_start_slot + ack_slot_idx as u64,
            terminal: false,
        },
    );

    let mut proc = HrpdReverseTrafficAckProcessor::new(Some(bus.clone()), None);
    let out = proc.process_block(block);
    let packed = out[0].tags.get(TAG_ACK_PATTERN_PACKED).copied().unwrap() as u32;
    let slot_bits = (packed >> (ack_slot_idx * 2)) & 0b11;
    assert_eq!(slot_bits, 0b11, "expected NAK state for slot 4");
    assert!(
        bus.drain_feedback(mac_index).is_empty(),
        "intermediate NAK must not close a multi-slot packet"
    );
}

#[test]
fn ack_processor_publishes_intermediate_ack_an_event() {
    let mac_index = 14u8;
    let uati = 0x1a12_3456;
    let ack_slot_idx = 6usize;
    let frame_start_slot = 0u64;
    let mut frame = vec![Complex32::new(0.0, 0.0); HRPD_TRAFFIC_FRAME_CHIPS];
    let ack_chips = synthesize_ack_slot_chips(1.0); // ACK
    let slot_base = ack_slot_idx * HRPD_SLOT_CHIPS;
    for (k, sample) in ack_chips.iter().enumerate().take(ACK_CHIPS_PER_BIT) {
        frame[slot_base + k] = *sample;
    }
    let mut block = SampleBlock {
        samples: frame,
        chip_start: 0,
        sample_rate_hz: 1_228_800.0,
        rx_sample_time: None,
        tags: Default::default(),
        pcg_signal_snr_db: None,
        active_pcg_mask: None,
        pcg_pilot_metrics: None,
    };
    block.tags.insert(TAG_FRAME_START_CHIP, 0);
    block.tags.insert(TAG_MAC_INDEX, mac_index as i64);
    block.tags.insert(TAG_UATI, uati as i64);

    let bus = Arc::new(HarqBus::new());
    bus.publish_emission(
        mac_index,
        HarqEmissionEvent {
            packet_id: 5,
            subpacket: 0,
            packet_start_slot: frame_start_slot + ack_slot_idx as u64 - 3,
            forward_slot: frame_start_slot + ack_slot_idx as u64 - 3,
            expected_ack_reverse_slot: frame_start_slot + ack_slot_idx as u64,
            terminal: false,
        },
    );
    let (tx, mut rx) = tokio_mpsc::unbounded_channel();

    let mut proc = HrpdReverseTrafficAckProcessor::new(Some(bus.clone()), Some(tx));
    let out = proc.process_block(block);
    let packed = out[0].tags.get(TAG_ACK_PATTERN_PACKED).copied().unwrap() as u32;
    let slot_bits = (packed >> (ack_slot_idx * 2)) & 0b11;
    assert_eq!(slot_bits, 0b10, "expected ACK state for slot 6");
    // An early ACK means the AT already decoded the packet: the scheduler
    // must retire it instead of transmitting the remaining slots and waiting
    // for a terminal ACK the AT may gate.
    let feedback = bus.drain_feedback(mac_index);
    assert_eq!(feedback.len(), 1, "early ACK closes the packet");
    assert_eq!(feedback[0].packet_id, 5);
    assert_eq!(feedback[0].response, HarqResponse::Ack);
    match rx
        .try_recv()
        .expect("intermediate ACK should publish AN event")
    {
        HrpdTrafficEvent::Ack {
            uati: event_uati,
            mac_index: event_mac,
            slot,
            ack,
        } => {
            assert_eq!(event_uati, uati);
            assert_eq!(event_mac, mac_index);
            assert_eq!(slot, frame_start_slot + ack_slot_idx as u64);
            assert!(ack);
        }
        other => panic!("expected ACK event, got {other:?}"),
    }
}

#[test]
fn ack_processor_publishes_terminal_ack_an_event() {
    let mac_index = 15u8;
    let uati = 0x1ade_ad00;
    let ack_slot_idx = 8usize;
    let frame_start_slot = 0u64;
    let mut frame = vec![Complex32::new(0.0, 0.0); HRPD_TRAFFIC_FRAME_CHIPS];
    let ack_chips = synthesize_ack_slot_chips(1.0); // ACK
    let slot_base = ack_slot_idx * HRPD_SLOT_CHIPS;
    for (k, sample) in ack_chips.iter().enumerate().take(ACK_CHIPS_PER_BIT) {
        frame[slot_base + k] = *sample;
    }
    let mut block = SampleBlock {
        samples: frame,
        chip_start: 0,
        sample_rate_hz: 1_228_800.0,
        rx_sample_time: None,
        tags: Default::default(),
        pcg_signal_snr_db: None,
        active_pcg_mask: None,
        pcg_pilot_metrics: None,
    };
    block.tags.insert(TAG_FRAME_START_CHIP, 0);
    block.tags.insert(TAG_MAC_INDEX, mac_index as i64);
    block.tags.insert(TAG_UATI, uati as i64);

    let bus = Arc::new(HarqBus::new());
    bus.publish_emission(
        mac_index,
        HarqEmissionEvent {
            packet_id: 6,
            subpacket: 0,
            packet_start_slot: frame_start_slot + ack_slot_idx as u64 - 3,
            forward_slot: frame_start_slot + ack_slot_idx as u64 - 3,
            expected_ack_reverse_slot: frame_start_slot + ack_slot_idx as u64,
            terminal: true,
        },
    );
    let (tx, mut rx) = tokio_mpsc::unbounded_channel();

    let mut proc = HrpdReverseTrafficAckProcessor::new(Some(bus), Some(tx));
    proc.process_block(block);

    match rx.try_recv().expect("terminal ACK should publish AN event") {
        HrpdTrafficEvent::Ack {
            uati: event_uati,
            mac_index: event_mac,
            slot,
            ack,
        } => {
            assert_eq!(event_uati, uati);
            assert_eq!(event_mac, mac_index);
            assert_eq!(slot, frame_start_slot + ack_slot_idx as u64);
            assert!(ack);
        }
        other => panic!("expected ACK event, got {other:?}"),
    }
}

#[test]
fn ack_processor_publishes_terminal_nak_feedback() {
    let mac_index = 13u8;
    let ack_slot_idx = 5usize;
    let frame_start_slot = 0u64;
    let mut frame = vec![Complex32::new(0.0, 0.0); HRPD_TRAFFIC_FRAME_CHIPS];
    let ack_chips = synthesize_ack_slot_chips(-1.0); // NAK
    let slot_base = ack_slot_idx * HRPD_SLOT_CHIPS;
    for (k, sample) in ack_chips.iter().enumerate().take(ACK_CHIPS_PER_BIT) {
        frame[slot_base + k] = *sample;
    }
    let mut block = SampleBlock {
        samples: frame,
        chip_start: 0,
        sample_rate_hz: 1_228_800.0,
        rx_sample_time: None,
        tags: Default::default(),
        pcg_signal_snr_db: None,
        active_pcg_mask: None,
        pcg_pilot_metrics: None,
    };
    block.tags.insert(TAG_FRAME_START_CHIP, 0);
    block.tags.insert(TAG_MAC_INDEX, mac_index as i64);

    let bus = Arc::new(HarqBus::new());
    bus.publish_emission(
        mac_index,
        HarqEmissionEvent {
            packet_id: 4,
            subpacket: 0,
            packet_start_slot: frame_start_slot + ack_slot_idx as u64 - 3,
            forward_slot: frame_start_slot + ack_slot_idx as u64 - 3,
            expected_ack_reverse_slot: frame_start_slot + ack_slot_idx as u64,
            terminal: true,
        },
    );

    let mut proc = HrpdReverseTrafficAckProcessor::new(Some(bus.clone()), None);
    let out = proc.process_block(block);
    let packed = out[0].tags.get(TAG_ACK_PATTERN_PACKED).copied().unwrap() as u32;
    let slot_bits = (packed >> (ack_slot_idx * 2)) & 0b11;
    assert_eq!(slot_bits, 0b11, "expected NAK state for slot 5");

    let feedback = bus.drain_feedback(mac_index);
    assert_eq!(feedback.len(), 1);
    assert_eq!(feedback[0].packet_id, 4);
    assert_eq!(feedback[0].subpacket, 0);
    assert_eq!(feedback[0].response, HarqResponse::Nak);
}

// -- DRC processor test -----------------------------------------------------

#[test]
fn drc_processor_decodes_drc_length_one_slots() {
    use crate::receiver::hrpd::drc_decoder::{DRC_CHIPS_PER_SLOT, encode_drc};
    let drc_cover = 4u8;
    let drc_length = 1u8;
    let mut frame = vec![Complex32::new(0.0, 0.0); HRPD_TRAFFIC_FRAME_CHIPS];
    let mut expected_values = [DRC_SLOT_GATED_VALUE; HRPD_TRAFFIC_SLOTS_PER_FRAME];
    for (slot, expected) in expected_values
        .iter_mut()
        .enumerate()
        .take(HRPD_TRAFFIC_SLOTS_PER_FRAME - 1)
    {
        let value = (slot as u8) % 16;
        *expected = value;
        let chips = encode_drc(value, drc_cover, drc_length);
        assert_eq!(chips.len(), DRC_CHIPS_PER_SLOT);
        let start = DRC_CHIPS_PER_SLOT / 2 + slot * DRC_CHIPS_PER_SLOT;
        frame[start..start + DRC_CHIPS_PER_SLOT].copy_from_slice(&chips);
    }
    assert_eq!(frame.len(), HRPD_TRAFFIC_FRAME_CHIPS);

    let mut block = SampleBlock {
        samples: frame,
        chip_start: 0,
        sample_rate_hz: 1_228_800.0,
        rx_sample_time: None,
        tags: Default::default(),
        pcg_signal_snr_db: None,
        active_pcg_mask: None,
        pcg_pilot_metrics: None,
    };
    block.tags.insert(TAG_DRC_COVER, drc_cover as i64);
    block.tags.insert(TAG_DRC_LENGTH, drc_length as i64);

    let mut proc = HrpdReverseTrafficDrcProcessor::new(None, None);
    let out = proc.process_block(block);
    assert_eq!(out.len(), 1);
    let packed = out[0].tags.get(TAG_DRC_PACKED).copied().unwrap() as u64;
    for (slot, expected) in expected_values.iter().enumerate() {
        let nibble = ((packed >> (slot * 4)) & 0xF) as u8;
        assert_eq!(
            nibble, *expected,
            "DRC mismatch at slot {slot}: got 0x{nibble:x} expected 0x{expected:x}"
        );
    }
}

#[test]
fn drc_processor_normalizes_negative_q_sign() {
    use crate::receiver::hrpd::drc_decoder::{DRC_CHIPS_PER_SLOT, encode_drc};
    let drc_cover = 4u8;
    let drc_length = 1u8;
    let requested_value = 0x2;
    let mut frame = vec![Complex32::new(0.0, 0.0); HRPD_TRAFFIC_FRAME_CHIPS];

    let chips = encode_drc(requested_value, drc_cover, drc_length)
        .into_iter()
        .map(|s| Complex32::new(s.re, -s.im))
        .collect::<Vec<_>>();
    let start = DRC_CHIPS_PER_SLOT / 2;
    frame[start..start + DRC_CHIPS_PER_SLOT].copy_from_slice(&chips);

    let mut block = SampleBlock {
        samples: frame,
        chip_start: 0,
        sample_rate_hz: 1_228_800.0,
        rx_sample_time: None,
        tags: Default::default(),
        pcg_signal_snr_db: None,
        active_pcg_mask: None,
        pcg_pilot_metrics: None,
    };
    block.tags.insert(TAG_DRC_COVER, drc_cover as i64);
    block.tags.insert(TAG_DRC_LENGTH, drc_length as i64);
    block.tags.insert(TAG_Q_SIGN_X1000, -1000);

    let mut proc = HrpdReverseTrafficDrcProcessor::new(None, None);
    let out = proc.process_block(block);
    let packed = out[0].tags.get(TAG_DRC_PACKED).copied().unwrap() as u64;
    assert_eq!((packed & 0xF) as u8, requested_value);
}

#[test]
fn drc_processor_reports_last_slot_as_completion_slot() {
    use crate::receiver::hrpd::drc_decoder::{DRC_CHIPS_PER_SLOT, encode_drc};
    let drc_cover = 4u8;
    let drc_length = 2u8;
    let frame_offset = 1u8;
    let mac_index = 8u8;
    let uati = 0x1a05_8004u32;
    let frame_start_slot = 100u64;
    let mut frame = vec![Complex32::new(0.0, 0.0); HRPD_TRAFFIC_FRAME_CHIPS];

    let chips = encode_drc(0x3, drc_cover, drc_length);
    assert_eq!(chips.len(), (drc_length as usize) * DRC_CHIPS_PER_SLOT);
    let start = DRC_CHIPS_PER_SLOT / 2;
    frame[start..start + chips.len()].copy_from_slice(&chips);

    let (tx, mut rx) = tokio_mpsc::unbounded_channel();
    let mut block = SampleBlock {
        samples: frame,
        chip_start: 0,
        sample_rate_hz: 1_228_800.0,
        rx_sample_time: None,
        tags: Default::default(),
        pcg_signal_snr_db: None,
        active_pcg_mask: None,
        pcg_pilot_metrics: None,
    };
    block.tags.insert(TAG_DRC_COVER, drc_cover as i64);
    block.tags.insert(TAG_DRC_LENGTH, drc_length as i64);
    block.tags.insert(TAG_FRAME_OFFSET, frame_offset as i64);
    block.tags.insert(
        TAG_FRAME_START_CHIP,
        (frame_start_slot * HRPD_SLOT_CHIPS as u64) as i64,
    );
    block.tags.insert(TAG_MAC_INDEX, mac_index as i64);
    block.tags.insert(TAG_UATI, uati as i64);

    let mut proc = HrpdReverseTrafficDrcProcessor::new(Some(tx), None);
    let out = proc.process_block(block);
    assert_eq!(out.len(), 1);
    let event = rx.try_recv().expect("DRC event");
    assert_eq!(
        event,
        HrpdTrafficEvent::Drc {
            uati,
            mac_index,
            slot: frame_start_slot + u64::from(drc_length),
            drc_index: 0x3,
        }
    );
}

#[test]
fn drc_processor_publishes_confident_drc_when_pilot_metric_is_low() {
    use crate::receiver::hrpd::drc_decoder::{DRC_CHIPS_PER_SLOT, encode_drc};
    let drc_cover = 4u8;
    let drc_length = 2u8;
    let mac_index = 6u8;
    let uati = 0x1a05_8001u32;
    let frame_start_slot = 200u64;
    let requested_drc = 0x0c;
    let mut frame = vec![Complex32::new(0.0, 0.0); HRPD_TRAFFIC_FRAME_CHIPS];

    let chips = encode_drc(requested_drc, drc_cover, drc_length);
    let start = DRC_CHIPS_PER_SLOT + DRC_CHIPS_PER_SLOT / 2;
    frame[start..start + chips.len()].copy_from_slice(&chips);

    let bus = Arc::new(HarqBus::new());
    let (tx, mut rx) = tokio_mpsc::unbounded_channel();
    let mut block = SampleBlock {
        samples: frame,
        chip_start: 0,
        sample_rate_hz: 1_228_800.0,
        rx_sample_time: None,
        tags: Default::default(),
        pcg_signal_snr_db: None,
        active_pcg_mask: None,
        pcg_pilot_metrics: None,
    };
    block.tags.insert(TAG_DRC_COVER, drc_cover as i64);
    block.tags.insert(TAG_DRC_LENGTH, drc_length as i64);
    block.tags.insert(
        TAG_FRAME_START_CHIP,
        (frame_start_slot * HRPD_SLOT_CHIPS as u64) as i64,
    );
    block.tags.insert(TAG_MAC_INDEX, mac_index as i64);
    block.tags.insert(TAG_UATI, uati as i64);
    block.tags.insert(TAG_PILOT_COHERENCE_X1000, 100);

    let mut proc = HrpdReverseTrafficDrcProcessor::new(Some(tx), Some(bus.clone()));
    let out = proc.process_block(block);
    assert_eq!(out.len(), 1);
    let completion_slot = frame_start_slot + u64::from(drc_length) + 1;
    assert_eq!(
        rx.try_recv().expect("DRC event"),
        HrpdTrafficEvent::Drc {
            uati,
            mac_index,
            slot: completion_slot,
            drc_index: requested_drc,
        }
    );
    assert_eq!(
        bus.current_drc_record(mac_index),
        Some((completion_slot, requested_drc))
    );
}

// -- Data processor test ----------------------------------------------------

#[test]
fn data_processor_passes_block_through_and_does_not_panic_on_noise() {
    // The Turbo decoder + MAC parser tower is exercised heavily in the
    // `data_decoder` module's own tests; here we just confirm the
    // PipelineProcessor wrapper:
    //   - reads the RRI tag to choose a rate
    //   - falls back to the configured rate when RRI is absent
    //   - never panics on garbage input
    //   - always passes the block through
    let uati = 0xAABBCCDDu32;
    let mac_index = 9u8;
    let mut block = SampleBlock {
        samples: vec![Complex32::new(0.0, 0.0); HRPD_TRAFFIC_FRAME_CHIPS],
        chip_start: 0,
        sample_rate_hz: 1_228_800.0,
        rx_sample_time: None,
        tags: Default::default(),
        pcg_signal_snr_db: None,
        active_pcg_mask: None,
        pcg_pilot_metrics: None,
    };
    block.tags.insert(TAG_UATI, uati as i64);
    block.tags.insert(TAG_MAC_INDEX, mac_index as i64);
    block.tags.insert(TAG_RRI_RATE_BPS, 9_600);

    let (tx, mut rx) = tokio_mpsc::unbounded_channel::<HrpdTrafficEvent>();
    let mut proc = HrpdReverseTrafficDataProcessor::new(Some(tx));
    let out = proc.process_block(block);
    assert_eq!(out.len(), 1, "block must pass through");
    // Zero IQ never produces a CRC-clean frame; the channel should be empty.
    assert!(rx.try_recv().is_err());
}

#[test]
fn reverse_fer_counts_good_and_erased_but_excludes_pilot_only() {
    let mut proc = HrpdReverseTrafficDataProcessor::new(None);
    // Good frames: any decode CRC-clean.
    proc.test_record_reverse_fer(true, true);
    proc.test_record_reverse_fer(true, false);
    // Erased: data on the Q arm but nothing decoded.
    proc.test_record_reverse_fer(false, true);
    // Pilot-only: no data transmitted, excluded from FER entirely.
    proc.test_record_reverse_fer(false, false);
    proc.test_record_reverse_fer(false, false);

    let (ok, erased) = proc.fer_totals();
    assert_eq!(ok, 2, "both CRC-clean frames count as good");
    assert_eq!(erased, 1, "only the data-present miss is an erasure");
}

// -- Reverse power-control loop tests ----------------------------------------
//
// These exercise the per-slot HRPD RPC controller directly: it ingests one
// pilot SINR per slot, predicts across HRPD_RPC_TX_LEAD_SLOTS with the same
// clamped LSQ predictor form as RC3, and emits one RPC bit (0=up, 1=down) via
// the shared delta-sigma quantizer plus a large-error direction guard.
const RPC_TEST_TARGET_DB: f32 = HRPD_RPC_TARGET_SNR_DB;

/// Drive the controller for `slots` reliable measurements held at `snr_db` and
/// return `(up_bits, down_bits)`.
fn run_rpc_controller(snr_db: f32, slots: usize) -> (usize, usize) {
    let mut rpc = HrpdRpcController::new();
    let mut up: usize = 0;
    let mut down: usize = 0;
    for _ in 0..slots {
        let level = rpc.ingest(snr_db, 1.0);
        match rpc.emit(level) {
            0 => up += 1,
            _ => down += 1,
        }
    }
    (up, down)
}

#[test]
fn rpc_predictor_clamps_forward_trend_like_rc3() {
    let mut rpc = HrpdRpcController::new();
    for sample in [6.0_f32, 6.5, 7.0, 7.5, 8.0, 8.5] {
        let _ = rpc.ingest(sample, 1.0);
    }
    let predicted = rpc.ingest(9.0, 1.0).unwrap();
    assert!(
        predicted > 9.0,
        "upward trend should predict above newest measured level: predicted={predicted}"
    );
    assert!(
        predicted <= 10.0 + 1e-6,
        "prediction must be clamped to newest+1 dB: predicted={predicted}"
    );
}

#[test]
fn rpc_hot_pilot_commands_down() {
    let (up, down) = run_rpc_controller(RPC_TEST_TARGET_DB + 6.0, 24);
    assert!(down > up, "a hot pilot must net DOWN: up={up} down={down}");
    assert!(up > 0, "large errors still obey the fractional drive clamp");
}

#[test]
fn rpc_weak_pilot_commands_up() {
    let (up, down) = run_rpc_controller(RPC_TEST_TARGET_DB - 6.0, 24);
    assert!(up > down, "a weak pilot must net UP: up={up} down={down}");
    assert!(
        down > 0,
        "large errors still obey the fractional drive clamp"
    );
}

#[test]
fn rpc_below_target_near_setpoint_nets_up_without_slam() {
    let (up, down) = run_rpc_controller(RPC_TEST_TARGET_DB - 1.0, 64);
    assert!(up > down, "below target should net UP: up={up} down={down}");
    assert!(
        down > 0,
        "near the setpoint the delta-sigma loop should still smooth with some DOWN bits: up={up} down={down}"
    );
}

#[test]
fn rpc_large_latest_error_overrides_stale_prediction_direction() {
    let mut rpc = HrpdRpcController::new();

    for _ in 0..12 {
        let hot = rpc.ingest(RPC_TEST_TARGET_DB + 6.0, 1.0);
        let _ = rpc.emit(hot);
    }
    let now_weak = rpc.ingest(RPC_TEST_TARGET_DB - 4.0, 1.0);
    assert_eq!(
        rpc.emit(now_weak),
        0,
        "latest weak pilot must command UP even if fitted history is stale"
    );

    for _ in 0..12 {
        let weak = rpc.ingest(RPC_TEST_TARGET_DB - 6.0, 1.0);
        let _ = rpc.emit(weak);
    }
    let now_hot = rpc.ingest(RPC_TEST_TARGET_DB + 4.0, 1.0);
    assert_eq!(
        rpc.emit(now_hot),
        1,
        "latest hot pilot must command DOWN even if fitted history is stale"
    );
}

#[test]
fn rpc_at_target_alternates_hold() {
    let (up, down) = run_rpc_controller(RPC_TEST_TARGET_DB, 100);
    let diff = (up as i64 - down as i64).unsigned_abs() as usize;
    assert!(
        diff <= 6,
        "at target the loop should alternate hold: up={up} down={down}"
    );
}

#[test]
fn rpc_lost_pilot_without_raw_evidence_holds_neutral() {
    // Without raw-power evidence, loss is ambiguous between a fade and DTX.
    // Hold net-neutral rather than accumulating blind UP drive.
    let mut rpc = HrpdRpcController::new();
    for _ in 0..8 {
        let l = rpc.ingest(RPC_TEST_TARGET_DB - 6.0, 1.0);
        let _ = rpc.emit(l);
    }
    // Pilot now lost for a long stretch.
    let mut up: usize = 0;
    let mut down: usize = 0;
    for _ in 0..64 {
        let l = rpc.ingest(f32::NAN, 0.0);
        match rpc.emit(l) {
            0 => up += 1,
            _ => down += 1,
        }
    }
    assert!(
        up.abs_diff(down) <= 1,
        "ambiguous loss should hold neutral: up={up} down={down}"
    );
}

#[test]
fn rpc_saturating_input_commands_down_for_reliable_and_lost_metrics() {
    // Raw input genuinely near ADC full scale (above the clip/hot-limit knee)
    // commands DOWN whether or not the SINR metric is reliable.
    let saturating = -1.0;
    let mut rpc = HrpdRpcController::new();
    for _ in 0..24 {
        rpc.observe_raw_power(saturating);
    }

    let weak_level = rpc.ingest(RPC_TEST_TARGET_DB - 10.0, 1.0);
    assert_eq!(
        rpc.emit_with_raw_power(weak_level, saturating),
        1,
        "saturating input commands DOWN over a reliable under-target metric"
    );

    let mut up = 0usize;
    let mut down = 0usize;
    for _ in 0..24 {
        rpc.observe_raw_power(saturating);
        let lost = rpc.ingest(f32::NAN, 0.0);
        match rpc.emit_with_raw_power(lost, saturating) {
            0 => up += 1,
            _ => down += 1,
        }
    }
    assert!(
        down == 24 && up == 0,
        "saturating input commands DOWN even while the SINR metric is lost: up={up} down={down}"
    );
}

#[test]
fn rpc_lost_pilot_holds_neutral_after_near_target_lock() {
    let mut rpc = HrpdRpcController::new();

    // Establish a reliable near-target history.
    for _ in 0..8 {
        let locked = rpc.ingest(RPC_TEST_TARGET_DB, 1.0);
        let _ = rpc.emit_with_raw_power(locked, -62.0);
    }

    // Raw energy cannot distinguish an AT fade from DTX or tracking loss. Hold
    // neutral until a reliable W0 quality measurement returns.
    let mut up = 0usize;
    let mut down = 0usize;
    for _ in 0..24 {
        let lost = rpc.ingest(f32::NAN, 0.0);
        match rpc.emit_with_raw_power(lost, -31.0) {
            0 => up += 1,
            _ => down += 1,
        }
    }
    assert!(
        up.abs_diff(down) <= 1,
        "loud lost-pilot input near target should hold neutral: up={up} down={down}"
    );
}

#[test]
fn rpc_reliable_metric_remains_quality_authority_below_adc_brake() {
    let mut rpc = HrpdRpcController::new();

    // Seed the raw diagnostic with a quiet reliable observation.
    for _ in 0..8 {
        let locked = rpc.ingest(RPC_TEST_TARGET_DB, 1.0);
        let _ = rpc.emit_with_raw_power(locked, -64.0);
    }

    // A raw level above that observation is not by itself an ADC safety
    // condition. Below the brake threshold, reliable pilot quality owns the
    // control decision.
    let mut up = 0usize;
    let mut down = 0usize;
    for _ in 0..16 {
        let locked = rpc.ingest(RPC_TEST_TARGET_DB - 2.0, 1.0);
        match rpc.emit_with_raw_power(locked, -56.0) {
            0 => up += 1,
            _ => down += 1,
        }
    }

    assert!(
        up > down,
        "reliable under-target metric should net UP despite relative raw rise: up={up} down={down}"
    );
}

#[test]
fn rpc_safe_raw_does_not_lower_reliable_effective_target() {
    let mut rpc = HrpdRpcController::new();

    let mut up = 0usize;
    let mut down = 0usize;
    for _ in 0..32 {
        let locked = rpc.ingest(RPC_TEST_TARGET_DB - 1.0, 1.0);
        match rpc.emit_with_raw_power(locked, -30.0) {
            0 => up += 1,
            _ => down += 1,
        }
    }

    assert!(
        up > down,
        "safe raw input must not lower the target while the pilot metric is below target: up={up} down={down}"
    );
}

#[test]
fn rpc_hot_but_not_clipping_raw_does_not_override_under_target_reliable_metric() {
    // A single loud slot below the brake threshold leaves the decision with
    // the reliable quality metric.
    let mut rpc = HrpdRpcController::new();

    let locked = rpc.ingest(RPC_TEST_TARGET_DB - 5.0, 1.0);
    assert_eq!(
        rpc.emit_with_raw_power(locked, -14.0),
        0,
        "hot-but-not-clipping raw is not an override while W0 is reliable and under target"
    );
}

#[test]
fn rpc_safe_cold_raw_does_not_override_under_target_metric() {
    // Raw input far below the ADC brake threshold is not a power-control
    // target. A reliable under-target pilot metric still nets UP.
    let cold_raw = -48.0;
    let mut rpc = HrpdRpcController::new();
    for _ in 0..24 {
        rpc.observe_raw_power(cold_raw);
    }

    let mut up = 0usize;
    let mut down = 0usize;
    for _ in 0..8 {
        let locked = rpc.ingest(RPC_TEST_TARGET_DB - 4.0, 1.0);
        match rpc.emit_with_raw_power(locked, cold_raw) {
            0 => up += 1,
            _ => down += 1,
        }
    }

    assert!(
        up > down,
        "safe cold raw leaves an under-target quality metric in control: up={up} down={down}"
    );
    assert!(
        down > 0,
        "large metric errors still obey the fractional drive clamp"
    );
}

#[test]
fn rpc_lost_pilot_holds_with_raw_energy_present() {
    let mut rpc = HrpdRpcController::new();

    // Once quality is unmeasurable, raw energy alone cannot justify an UP or
    // DOWN trend. Hold neutral until the pilot metric returns.
    for _ in 0..8 {
        let locked = rpc.ingest(RPC_TEST_TARGET_DB, 1.0);
        let _ = rpc.emit_with_raw_power(locked, -62.0);
    }

    let mut up = 0usize;
    let mut down = 0usize;
    for _ in 0..24 {
        let lost = rpc.ingest(f32::NAN, 0.0);
        match rpc.emit_with_raw_power(lost, -58.0) {
            0 => up += 1,
            _ => down += 1,
        }
    }

    let diff = up.abs_diff(down);
    assert!(
        diff <= 1,
        "lost metric with raw energy present should hold neutral: up={up} down={down}"
    );
}

#[test]
fn rpc_lost_under_target_pilot_holds_without_blind_recovery() {
    let mut rpc = HrpdRpcController::new();

    // Regression for the ramp-into-clipping failure: a stale under-target
    // prediction must not accumulate blind UP commands through a DTX gap.
    for _ in 0..8 {
        let locked = rpc.ingest(RPC_TEST_TARGET_DB - 6.0, 1.0);
        let _ = rpc.emit_with_raw_power(locked, -62.0);
    }

    let mut up = 0usize;
    let mut down = 0usize;
    for _ in 0..16 {
        let lost = rpc.ingest(f32::NAN, 0.0);
        match rpc.emit_with_raw_power(lost, -54.0) {
            0 => up += 1,
            _ => down += 1,
        }
    }

    assert!(
        up.abs_diff(down) <= 1,
        "lost under-target metric must hold neutral: up={up} down={down}"
    );
}

#[test]
fn rpc_lost_pilot_with_loud_raw_holds() {
    let mut rpc = HrpdRpcController::new();

    for _ in 0..8 {
        let locked = rpc.ingest(RPC_TEST_TARGET_DB - 6.0, 1.0);
        let _ = rpc.emit_with_raw_power(locked, -62.0);
    }

    // Even loud raw input does not provide pilot quality. It stays below the
    // ADC brake threshold here, so loss still holds net-neutral.
    let mut up = 0usize;
    let mut down = 0usize;
    for _ in 0..16 {
        let lost = rpc.ingest(f32::NAN, 0.0);
        match rpc.emit_with_raw_power(lost, -14.0) {
            0 => up += 1,
            _ => down += 1,
        }
    }

    assert!(
        up.abs_diff(down) <= 1,
        "lost metric with loud raw holds neutral (alternating, net-zero): up={up} down={down}"
    );
}

#[test]
fn rpc_lost_pilot_holds_after_healthy_lock_while_raw_remains_present() {
    let mut rpc = HrpdRpcController::new();

    // The last reliable slots were above target. If the metric then goes
    // unreliable, neither the stale prediction nor raw input may ramp the AT.
    for _ in 0..8 {
        let locked = rpc.ingest(RPC_TEST_TARGET_DB + 4.0, 1.0);
        let _ = rpc.emit_with_raw_power(locked, -56.0);
    }

    let mut up = 0usize;
    let mut down = 0usize;
    for _ in 0..16 {
        let lost = rpc.ingest(f32::NAN, 0.0);
        match rpc.emit_with_raw_power(lost, -56.0) {
            0 => up += 1,
            _ => down += 1,
        }
    }

    assert!(
        up.abs_diff(down) <= 1,
        "a healthy-then-lost pilot with raw energy present holds neutral: up={up} down={down}"
    );
}

#[test]
fn rpc_lost_pilot_holds_when_raw_faded() {
    let mut rpc = HrpdRpcController::new();

    // Last reliable lock was weak. Even if raw input then drops, the loss is
    // indistinguishable from subtype-3 DTX, which does not
    // respond to RPC during the silent interval.
    for _ in 0..8 {
        let locked = rpc.ingest(RPC_TEST_TARGET_DB - 6.0, 1.0);
        let _ = rpc.emit_with_raw_power(locked, -62.0);
    }

    let mut up = 0usize;
    let mut down = 0usize;
    for _ in 0..8 {
        let lost = rpc.ingest(f32::NAN, 0.0);
        match rpc.emit_with_raw_power(lost, -68.0) {
            0 => up += 1,
            _ => down += 1,
        }
    }

    assert!(
        up.abs_diff(down) <= 1,
        "lost metric plus faded raw should hold neutral: up={up} down={down}"
    );
}

#[test]
fn rpc_per_slot_despread_aligns_on_non_frame_aligned_lock() {
    // Regression: the per-slot RPC despread must index the spreading reference
    // by offset from the spawn anchor, not the absolute slot number. A lock that
    // does not land on a frame-period boundary (chip 10240 = slot 5) would
    // otherwise despread every slot against the wrong PN/LC slice, read noise,
    // mark every slot unreliable, and hold the AT UP. With correct alignment the
    // clean synthetic pilot reads far above the 10 dB target and commands DOWN.
    let uati = 0x0123_4567;
    let frame_start_chip = 5 * HRPD_SLOT_CHIPS as u64; // 10240, not a frame boundary
    let (samples, i_mask, q_mask) = synthesize_pilot_only_frame_samples(uati, frame_start_chip);
    let mac_index = 5u8;
    let bus = Arc::new(HarqBus::new());
    let config = HrpdReverseTrafficFingerConfig {
        uati,
        mac_index,
        physical_layer_subtype: 0,
        reverse_traffic_mac_subtype: 0,
        frame_offset: 0,
        i_mask,
        q_mask,
        drc_cover: 3,
        drc_length: 1,
        oversample: 1,
        event_tx: None,
        harq_bus: Some(bus.clone()),
        reverse_pilot_acquired: None,
        worker_spawned_at: std::time::Instant::now(),
    };
    let lock = HrpdReverseTrafficFingerLock {
        frame_start_chip,
        chip_offset: 0,
        sample_delay: 0,
        sample_delay_fraction: 0.0,
        q_sign: -1.0,
        q_pair_phase: 0,
        initial_pilot_phase: Complex32::new(1.0, 0.0),
    };
    let mut finger = HrpdReverseTrafficFinger::new(1, config, lock);
    let block = SampleBlock {
        samples,
        chip_start: frame_start_chip as usize,
        sample_rate_hz: 1_228_800.0,
        rx_sample_time: None,
        tags: Default::default(),
        pcg_signal_snr_db: None,
        active_pcg_mask: None,
        pcg_pilot_metrics: None,
    };
    let mut chain = Vec::new();
    let _ = finger.process(&block, &mut chain);

    let mut up = 0;
    let mut down = 0;
    for slot in 0..256u64 {
        match bus.rpc_at_slot(mac_index, slot) {
            Some(0) => up += 1,
            Some(1) => down += 1,
            _ => {}
        }
    }
    assert!(
        down + up > 0,
        "the per-slot loop should have scheduled RPC bits"
    );
    assert!(
        down > up,
        "a clean hot pilot on a non-frame-aligned lock must command DOWN \
         (reference correctly aligned), got up={up} down={down}"
    );
}

// -- Subtype-3 sub-frame waveform golden -------------------------------------

/// Spread a composite (pilot + RRI + data) reverse subtype-2 waveform through
/// the shared spreading reference and decode it through the production finger
/// path: despread → derotate → RRI detect → data demap → HARQ → CRC-24 →
/// forward ARQ scheduling.
fn run_subtype3_subframe_packet(q_invert_data: bool) {
    use super::rri_subtype2::{HRPD_SUBFRAME_CHIPS, encode_rri_subtype2_subframe};
    use super::subtype2_data::{Subtype2DataFormat, subpacket_code_symbols};
    use crate::bts::hrpd::harq_bus::ArqLevel;
    use crate::phy::hrpd::turbo::HrpdTurboEncoder;
    use cdma_common::hrpd::traffic::physical_crc24;

    const PAYLOAD_BITS: usize = 1024;
    const RRI_GAIN: f32 = 1.0;
    const DATA_GAIN: f32 = 1.0;

    let uati = 0x0042_ad2d;
    let mac_index = 6u8;
    let frame_start_chip = 0u64;
    let (i_mask, q_mask) = default_reverse_traffic_long_code_masks(uati);
    let reference = hrpd_reverse_pilot_reference_chips(HrpdReversePilotReferenceConfig {
        start_chip: frame_start_chip,
        len: HRPD_TRAFFIC_FRAME_CHIPS,
        i_mask,
        q_mask,
        reference_chip_offset: 0,
        pn_phase_offset_chips: 0,
        lc_phase_offset_chips: 0,
        q_sign: -1.0,
        q_pair_phase: 0,
    });

    // CRC-valid 1024-bit physical packet, sub-packet 0 in sub-frame 0
    // (slots 0..3, interlace 0 for FrameOffset 0).
    let format = Subtype2DataFormat::for_payload_bits(PAYLOAD_BITS).expect("format");
    let mac_bits_len = PAYLOAD_BITS - 24 - 6;
    let mut packet_bits: Vec<u8> = {
        let mut state = 0x0f0f_1234u32;
        (0..mac_bits_len)
            .map(|_| {
                state = state.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                ((state >> 16) & 1) as u8
            })
            .collect()
    };
    let fcs = physical_crc24(&packet_bits);
    for i in (0..24).rev() {
        packet_bits.push(((fcs >> i) & 1) as u8);
    }
    packet_bits.extend(std::iter::repeat_n(0u8, 6));

    let encoder = HrpdTurboEncoder::new(PAYLOAD_BITS as u32).expect("encoder");
    let mut coded = encoder.encode(&packet_bits, 1, format.turbo_code_rate_den);
    format.scramble_encoder_output(&mut coded, 0);
    let interleaved = format.interleave_encoder_output(&coded);
    let code_symbols = subpacket_code_symbols(format, &interleaved, 0);
    let mut data_chips = format.modulate_subpacket(&code_symbols);
    if q_invert_data {
        for chip in &mut data_chips {
            chip.im = -chip.im;
        }
    }
    let rri_data = encode_rri_subtype2_subframe(0x5, 0); // 1024 bits, sub-packet 0
    let rri_null = encode_rri_subtype2_subframe(0x0, 0);

    // Composite per chip: pilot (1 + 0j, already the reference's despread
    // identity) + RRI + data in sub-frame 0; pilot + null RRI afterwards.
    let mut samples = Vec::with_capacity(HRPD_TRAFFIC_FRAME_CHIPS + 64);
    for (n, ref_chip) in reference.iter().enumerate() {
        let mut composite = Complex32::new(1.0, 0.0);
        let subframe_pos = n % HRPD_SUBFRAME_CHIPS;
        if n < HRPD_SUBFRAME_CHIPS {
            composite += rri_data[subframe_pos] * RRI_GAIN + data_chips[subframe_pos] * DATA_GAIN;
        } else {
            composite += rri_null[subframe_pos] * RRI_GAIN;
        }
        samples.push(ref_chip * composite);
    }
    samples.extend([Complex32::new(0.0, 0.0); 64]);

    let bus = Arc::new(HarqBus::new());
    let (event_tx, mut event_rx) = tokio_mpsc::unbounded_channel();
    let config = HrpdReverseTrafficFingerConfig {
        uati,
        mac_index,
        physical_layer_subtype: 2,
        reverse_traffic_mac_subtype: cdma_common::hrpd::traffic::REVERSE_TRAFFIC_MAC_SUBTYPE3,
        frame_offset: 0,
        i_mask,
        q_mask,
        drc_cover: 3,
        drc_length: 1,
        oversample: 1,
        event_tx: Some(event_tx),
        harq_bus: Some(bus.clone()),
        reverse_pilot_acquired: None,
        worker_spawned_at: std::time::Instant::now(),
    };
    let lock = HrpdReverseTrafficFingerLock {
        frame_start_chip,
        chip_offset: 0,
        sample_delay: 0,
        sample_delay_fraction: 0.0,
        q_sign: -1.0,
        q_pair_phase: 0,
        initial_pilot_phase: Complex32::new(1.0, 0.0),
    };
    let mut finger = HrpdReverseTrafficFinger::new(1, config, lock);
    let block = SampleBlock {
        samples,
        chip_start: 0,
        sample_rate_hz: 1_228_800.0,
        rx_sample_time: None,
        tags: Default::default(),
        pcg_signal_snr_db: None,
        active_pcg_mask: None,
        pcg_pilot_metrics: None,
    };
    let mut chain = Vec::new();
    let _ = finger.process(&block, &mut chain);

    // Sub-packet 0 occupied slots 0..3; H-ARQ ACK is due in slots 8..10
    // (slot 11 is the RPC/DRCLock slot and stays clear).
    for slot in 8..=10u64 {
        let arq = bus
            .arq_at_slot(mac_index, slot)
            .unwrap_or_else(|| panic!("ARQ scheduled for slot {slot}"));
        assert_eq!(arq.h_or_l, ArqLevel::Plus, "H-ARQ ACK at slot {slot}");
        assert_eq!(arq.p, ArqLevel::Off, "P-ARQ off at slot {slot}");
    }
    assert!(bus.arq_at_slot(mac_index, 11).is_none());

    // The decoded MAC packet is random bits, so no traffic event needs to
    // parse out of it; the ACK above proves the CRC-valid decode. Drain the
    // channel so an event, if any, does not leak into other tests.
    while event_rx.try_recv().is_ok() {}
}

#[test]
fn finger_decodes_subtype3_subframe_packet_and_schedules_arq() {
    run_subtype3_subframe_packet(false);
}

#[test]
fn finger_decodes_subtype3_subframe_packet_with_inverted_q_data() {
    run_subtype3_subframe_packet(true);
}
