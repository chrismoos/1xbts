//! HRPD reverse-traffic RAKE finger.
//!
//! One finger handles a single (UATI, MAC index, frame timing) hypothesis on
//! the reverse traffic channel. It accumulates raw IQ until a full 16-slot
//! physical-layer packet is buffered, despreads it using the locked pilot
//! parameters captured at spawn time, re-estimates the per-frame pilot phase
//! from the W0 pilot regions, and forwards the despread chips down the
//! sub-chain tagged with per-frame metadata that the RRI/ACK/DRC/Data
//! processors key on.

use std::collections::VecDeque;
use std::sync::{
    Arc,
    atomic::{AtomicBool, Ordering},
};

use log::{debug, info, trace, warn};
use num_complex::Complex32;
use tokio::sync::mpsc as tokio_mpsc;

use cdma_common::diagnostics::hrpd_rpc_control_verbose;
use cdma_common::hrpd::air::HrpdTrafficEvent;
use cdma_common::hrpd::traffic::implemented_forward_traffic_payload_bits_for_drc;

use crate::bts::hrpd::power_control::HRPD_INITIAL_TARGET_DB;
use crate::bts::hrpd::{
    HarqBus, HrpdPacketExclusion, HrpdPacketObservation, HrpdPacketOutcome, HrpdPowerControlHandle,
    mac_rpc_slot,
};
use crate::receiver::hrpd::data_decoder::traffic_events_from_mac_packet_for_reverse_mac_subtype;
use crate::receiver::hrpd::drc_decoder::{DRC_CHIPS_PER_SLOT, DrcDecoder, DrcSymbol};
use crate::receiver::pipelined::generic_rake_receiver::{BaseFinger, RakeFinger};
use crate::receiver::pipelined::{PipelineProcessorShared, RxSampleTimeAnchor, SampleBlock};

use super::despread::{
    HRPD_PILOT_CLEAN_START_CHIPS, HRPD_REVERSE_TRAFFIC_PILOT_MIN_COHERENCE, HRPD_SLOT_CHIPS,
    HRPD_TRAFFIC_FRAME_CHIPS, HRPD_TRAFFIC_SLOTS_PER_FRAME, HrpdReverseTrafficDespreadParams,
    HrpdTrafficMaskCandidate, PilotMoments, despread_chips_with_reference,
    despread_frame_with_reference, hrpd_reverse_traffic_reference_conj,
    pilot_moments_by_slot_from_despread, pilot_moments_from_despread, pilot_moments_from_slot,
    pilot_moments_from_subtype2_slot_regions, pilot_phase_ramp_from_slots, sample_chip_at_delay,
};
use super::drc_processor::{drc_completion_slot_for_repetition, drc_window_start_slot_at_or_after};
use super::rri_subtype2::{
    HRPD_SUBFRAME_CHIPS, RRI_SUBTYPE2_NULL_PAYLOAD_INDEX, RRI_SUBTYPE2_SUBFRAME_SYMBOLS,
    RriSubtype2Detection, decode_rri_subtype2_subframe, is_rri_subtype2_null,
    is_rri_subtype2_valid,
};
use super::subframe_harq::{
    SubframeHarq, SubframeOutcome, TerminalPacketDisposition, TerminalPacketOutcome,
};
use super::subtype2_data::{
    ModulationFormat, Subtype2DataFormat, decover_w12_symbols, decover_w24_symbols,
};
use crate::bts::reverse_power_predictor::{
    DeltaSigmaParams, delta_sigma_pcb_step, lsq_intercept_and_slope_at_newest,
    predict_ahead_clamped,
};

// SampleBlock tag schema produced by the finger. Downstream RRI/ACK/DRC/Data
// processors read these by name; do not rename without updating consumers.
pub const TAG_FRAME_START_CHIP: &str = "hrpd_reverse_frame_start_chip";
pub const TAG_FRAME_OFFSET: &str = "hrpd_reverse_frame_offset";
pub const TAG_PILOT_COHERENCE_X1000: &str = "hrpd_reverse_pilot_coherence_x1000";
pub const TAG_PILOT_SNR_DB_TENTHS: &str = "hrpd_reverse_pilot_snr_db_tenths";
pub const TAG_UATI: &str = "hrpd_reverse_uati";
pub const TAG_MAC_INDEX: &str = "hrpd_reverse_mac_index";
pub const TAG_PHYSICAL_LAYER_SUBTYPE: &str = "hrpd_reverse_physical_layer_subtype";
pub const TAG_REVERSE_TRAFFIC_MAC_SUBTYPE: &str = "hrpd_reverse_traffic_mac_subtype";
pub const TAG_DRC_COVER: &str = "hrpd_reverse_drc_cover";
pub const TAG_DRC_LENGTH: &str = "hrpd_reverse_drc_length";
pub const TAG_Q_SIGN_X1000: &str = "hrpd_reverse_q_sign_x1000";
pub(super) const TAG_POWER_CONTROL_MOBILE_POWER_LIMITED: &str =
    "hrpd_power_control_mobile_power_limited";
pub(super) const TAG_POWER_CONTROL_RECEIVER_REACQUIRING: &str =
    "hrpd_power_control_receiver_reacquiring";

/// Number of consecutive high-coherence frames required to flip the finger to
/// hard-validated. We hand-validate from coherence here because the sub-chain
/// may or may not produce CRC-valid tags depending on whether the AT is
/// transmitting data right now; a sustained pilot lock is sufficient health.
const CONSECUTIVE_HIGH_COHERENCE_FRAMES_FOR_VALIDATION: u32 = 2;
const HRPD_CHIP_RATE_HZ: u64 = 1_228_800;
/// Per-stage sub-frame diagnostics emitted per finger before going quiet.
const HRPD_SUBFRAME_DIAG_REPORTS_MAX: u32 = 96;
const HRPD_SUBFRAME_PHASE_DIAG_REPORTS_MAX: u32 = 48;
const HRPD_SUBFRAME_RRI_MIN_MARGIN: f32 = 0.25;
const HRPD_SUBFRAME_RRI_MIN_MARGIN_NORM: f32 = 0.06;
const HRPD_SUBFRAME_SUMMARY_WINDOW_SLOTS: u64 = 2400;
/// A stale finger must not pin the single-finger rake after sustained failure
/// to reacquire. Retire it so the assignment can return to full FFT acquisition.
const HRPD_REVERSE_TRAFFIC_REACQUIRE_TIMEOUT_CHIPS: u64 = HRPD_CHIP_RATE_HZ / 2;
const HRPD_REVERSE_TRAFFIC_PILOT_LOSS_TIMEOUT_CHIPS: u64 = 5 * HRPD_CHIP_RATE_HZ;
const HRPD_REVERSE_TRAFFIC_PILOT_REPORT_INTERVAL_CHIPS: u64 = HRPD_CHIP_RATE_HZ;
// Target for the full-complex, pilot-symbol SINR used by the reverse W0 loop.
// This is the same coherent-symbol / complex-residual form used by RC3. It is
// invariant when the whole despread signal and its proportional impairments
// scale together, but it must rise with AT power in the receiver-noise-limited
// region. A real-only projection reads about 3 dB high because it discards the
// quadrature residual, so use the full-complex form. Keep raw input out of the
// quality loop; it is only a receiver-input diagnostic and ADC safety guard.
// Live setup traffic at 5 dB sat only 2-3 dB above the receiver's raw floor:
// DRC publication collapsed and every reverse TrafficChannelComplete frame
// failed FCS. Keep enough margin for reverse control and data decoding.
pub(super) const HRPD_RPC_TARGET_SNR_DB: f32 = HRPD_INITIAL_TARGET_DB;
// A slot's pilot SINR drives the predictor only when it is strongly coherent
// and finite. The broader traffic-pilot lock gate is intentionally lower for
// detection continuity, but live Rev A traces showed 0.45..0.57 coherence
// "recoveries" with floor-level raw input polluting the RPC trend.
const HRPD_RPC_MIN_COHERENCE_FOR_DECISION: f32 = 0.90;
// RC3 pilot-symbol SINR assumes a stationary observation. Reject a slot when
// its despread W0 amplitude changes materially between halves; otherwise an AT
// on/off envelope edge appears as pilot noise and biases RPC upward.
const HRPD_RPC_MAX_PILOT_AMPLITUDE_STEP_DB: f32 = 3.0;
// Subtype-2 carries RPC on one slot phase out of four. A stationary W0 sample
// on a neighboring phase may therefore be the newest usable quality estimate
// when the control phase arrives. Reuse it only within that four-slot group;
// never bridge a sustained DTX interval.
const HRPD_RPC_MAX_REUSED_METRIC_AGE_SLOTS: u32 = 3;
// Slots the per-slot loop predicts ahead, and the absolute-slot offset it
// schedules the RPC bit into the future. This mirrors the RC3 inner loop's
// 12-PCG lookahead: fit a recent measured metric trend, extrapolate it to the
// TX air time, then make the HRPD RPC decision against that predicted metric.
const HRPD_RPC_TX_LEAD_SLOTS: u64 = 12;
const HRPD_RPC_PREDICTION_CLAMP_DB: f32 = 1.0;
const DRC_MID_SLOT_OFFSET_CHIPS: u64 = (DRC_CHIPS_PER_SLOT / 2) as u64;
const DRC_EVENT_MIN_CONFIDENCE: f32 = 4.0;
const FAST_DRC_MIN_CONFIRMED_REPETITIONS: u8 = 2;
// RPC controls one AT's pilot power, so its brake and hard ceiling use that
// AT's despread coherent pilot power. Carrier-wide input power cannot identify
// which AT should move and remains diagnostic-only.
const HRPD_RPC_MOBILE_BRAKE_BEGIN_DBFS: f32 = -45.0;
const HRPD_RPC_MOBILE_BRAKE_FULL_DBFS: f32 = -28.0;
const HRPD_RPC_MOBILE_BRAKE_MAX_OFFSET_DB: f32 = 10.0;
const HRPD_RPC_MOBILE_HARD_LIMIT_DBFS: f32 = -24.0;
const HRPD_RPC_MOBILE_HARD_RELEASE_DBFS: f32 = -30.0;
const HRPD_RPC_MOBILE_LIMIT_COOLDOWN_SLOTS: u8 = 12;
// General raw-power EMA is retained only for receiver diagnostics. The mobile
// brake uses a separate fast-attack / slow-release EMA.
const HRPD_RPC_RAW_FILTER_ALPHA: f32 = 0.05;
const HRPD_RPC_BRAKE_ATTACK_ALPHA: f32 = 1.0 / 40.0;
const HRPD_RPC_BRAKE_RELEASE_ALPHA: f32 = 1.0 / 16.0;
// Diagnostic-only: count slots whose instantaneous raw input approaches ADC
// full scale. Carrier-wide power never drives an RPC decision.
const HRPD_RPC_RAW_HOT_DIAG_DBFS: f32 = -6.0;
// Slot-rate SINR history depth for the least-squares level estimate. Keep this
// short: HRPD RPC lands about 12 slots after measurement, so a long history makes
// the controller chase stale fades and overshoots.
const HRPD_RPC_METRIC_HISTORY_LEN: usize = 12;
const HRPD_RPC_HOLD_BAND_DB: f32 = 0.5;
const HRPD_RPC_RESPONSE_GAIN_DB_PER_DB: f32 = 0.6;
// Limit the maximum net drive even for a large error. A full same-direction
// slot burst can walk the AT tens of dB in a few hundred ms; live HRPD traces
// show that is enough to drop the reverse pilot before the loop recovers.
const HRPD_RPC_DESIRED_STEP_CLAMP_DB: f32 = 0.25;
const HRPD_RPC_RESIDUAL_CLAMP_DB: f32 = 1.0;
const HRPD_RPC_DIRECTION_GUARD_DB: f32 = 2.0;
const HRPD_RPC_DELTA_SIGMA_PARAMS: DeltaSigmaParams = DeltaSigmaParams {
    hold_band_db: HRPD_RPC_HOLD_BAND_DB,
    response_gain_db_per_db: HRPD_RPC_RESPONSE_GAIN_DB_PER_DB,
    desired_step_clamp_db: HRPD_RPC_DESIRED_STEP_CLAMP_DB,
    residual_clamp_db: HRPD_RPC_RESIDUAL_CLAMP_DB,
};
// Cadence of the default aggregate RPC summary log line (slots, 600/s).
const HRPD_RPC_SUMMARY_INTERVAL_SLOTS: u32 = 600;

fn mobile_pilot_power_dbfs(moments: PilotMoments) -> f32 {
    10.0 * moments.coherent_pilot_power.max(1.0e-12).log10()
}

// Cadence of the fast-DRC summary log line (slots, 600/s).
const HRPD_FAST_DRC_SUMMARY_INTERVAL_SLOTS: u64 = 600;
// Minimum unreliable-RPC streak before reporting a lock/power-control warning.
const HRPD_RPC_UNRELIABLE_REPORT_MIN_SLOTS: u32 = 64;
// Normal Rev A gating produces roughly 67-slot pilot gaps. A continuous
// half-second loss is instead treated as a tune-away so it cannot bias PER.
const HRPD_POWER_CONTROL_TUNE_AWAY_MIN_SLOTS: u32 = 300;
// Phase feedback uses the W0-decovered coherence metric rather than the SNR
// estimate because reverse data bursts can leave the symbol-power SNR low even
// while the pilot ramp is coherent. 2.8 rad/slot stays below the ±π unwrap
// ambiguity but is wide enough for the live bladeRF/AT captures. Very low
// coherence frames still do not seed the next despread.
const HRPD_PHASE_TRACK_MIN_COHERENCE: f32 = 0.60;
const HRPD_PHASE_TRACK_MAX_STEP_RAD_PER_SLOT: f32 =
    super::despread::HRPD_PILOT_PHASE_RAMP_MAX_STEP_RAD_PER_SLOT;
// The traffic correlator refines sample timing at acquisition, but the live
// finger previously held that delay forever. Keep a small timing loop in the
// steady-state finger so sample-clock/group-delay drift does not take down the
// W0 pilot metric while raw reverse energy is still present.
const HRPD_TIMING_TRACK_INTERVAL_FRAMES: u32 = 8;
const HRPD_TIMING_TRACK_CHIP_STEP: usize = 16;
const HRPD_TIMING_TRACK_STEP_SAMPLES: f32 = 0.5;
const HRPD_TIMING_TRACK_NORMAL_RADIUS_SAMPLES: f32 = 3.0;
const HRPD_TIMING_TRACK_LOST_RADIUS_SAMPLES: f32 = 24.0;
const HRPD_TIMING_TRACK_MIN_IMPROVEMENT: f32 = 0.08;
const HRPD_TIMING_TRACK_RELOCK_MIN_COHERENCE: f32 = 0.60;
const HRPD_TIMING_TRACK_RELOCK_MIN_SNR_DB: f32 = 0.0;
const HRPD_TIMING_TRACK_REPORTS_MAX: u32 = 8;
// Lost-pilot relock searches are deliberately wider than steady-state tracking,
// so do not run them on every low-coherence frame/slot. Keeping the IQ timeline
// contiguous matters more than spending all CPU on repeated no-lock searches.
const HRPD_TIMING_TRACK_LOST_INTERVAL_FRAMES: u32 = 4;
const HRPD_RPC_TIMING_TRACK_REPORTS_MAX: u32 = 12;
const HRPD_RPC_CLEAN_DIAG_MIN_COHERENCE: f32 = 0.90;
const HRPD_TIMING_REACQUIRE_MIN_COHERENCE: f32 = 0.90;
const HRPD_TIMING_REACQUIRE_UNRELIABLE_SLOTS: u8 = 4;
const HRPD_TIMING_REACQUIRE_CONFIRM_SLOTS: u8 = 2;
const HRPD_TIMING_REACQUIRE_SEARCH_INITIAL_SLOTS: u32 = 16;
const HRPD_TIMING_REACQUIRE_SEARCH_MAX_SLOTS: u32 = 128;

mod fast_drc;

#[derive(Debug, Default)]
struct CorrelationStats {
    n: u32,
    sum_x: f64,
    sum_y: f64,
    sum_xx: f64,
    sum_yy: f64,
    sum_xy: f64,
}

impl CorrelationStats {
    fn observe(&mut self, x: f32, y: f32) {
        if !x.is_finite() || !y.is_finite() {
            return;
        }
        let x = f64::from(x);
        let y = f64::from(y);
        self.n = self.n.saturating_add(1);
        self.sum_x += x;
        self.sum_y += y;
        self.sum_xx += x * x;
        self.sum_yy += y * y;
        self.sum_xy += x * y;
    }

    fn mean_x(&self) -> f32 {
        if self.n == 0 {
            f32::NAN
        } else {
            (self.sum_x / f64::from(self.n)) as f32
        }
    }

    fn mean_y(&self) -> f32 {
        if self.n == 0 {
            f32::NAN
        } else {
            (self.sum_y / f64::from(self.n)) as f32
        }
    }

    fn correlation(&self) -> f32 {
        if self.n < 2 {
            return f32::NAN;
        }
        let n = f64::from(self.n);
        let denom = ((n * self.sum_xx - self.sum_x * self.sum_x)
            * (n * self.sum_yy - self.sum_y * self.sum_y))
            .max(0.0)
            .sqrt();
        if denom <= 1.0e-12 {
            f32::NAN
        } else {
            ((n * self.sum_xy - self.sum_x * self.sum_y) / denom) as f32
        }
    }
}

/// Per-slot reverse power-control loop. The reliable path keeps the RC3-style
/// lookahead predictor: a least-squares trend over recent slot metrics, clamped
/// extrapolation across the RX→TX command lead, then the shared delta-sigma
/// one-bit quantizer. A latest-slot direction guard only overrides stale
/// prediction sign conflicts; when the latest slot and prediction agree, the
/// quantizer still limits the net correction rate. An unmeasurable pilot holds
/// net-neutral: live subtype-3 DTX intervals do not respond to RPC, and blind
/// UP drive during each gap accumulates on the next active burst.
pub(super) struct HrpdRpcController {
    target_db: f32,
    /// Slot-rate pilot SINR history (oldest..=newest), least-squares smoothed
    /// into the current level estimate.
    metric_history: VecDeque<f32>,
    /// Last emitted bit (0=up, 1=down).
    last_bit: u8,
    /// Toggles the net-neutral hold bit while the pilot metric is unavailable.
    recovery_alt: u8,
    /// Diagnostics for the periodic summary line.
    last_level_db: f32,
    last_measured_level_db: f32,
    last_predicted_level_db: Option<f32>,
    last_prediction_delta_db: f32,
    last_slope_db_per_slot: f32,
    power_residual_db: f32,
    quiet_reliable_raw_power_dbfs: f32,
    /// Per-mobile pilot power paired with the newest reliable quality metric.
    last_reliable_mobile_power_dbfs: f32,
    /// General EMA of raw reverse power for diagnostics only.
    filtered_raw_power_dbfs: Option<f32>,
    /// Fast-attack / slow-release EMA of assigned-mobile pilot power.
    brake_filtered_mobile_power_dbfs: Option<f32>,
    /// Slots remaining in the per-mobile hard-limit cooldown.
    mobile_limit_cooldown_slots: u8,
    /// Per-mobile hard limiter latch with power hysteresis.
    mobile_hard_limiter_active: bool,
    /// True when the current slot exceeded the assigned-mobile hard ceiling.
    current_mobile_over_limit: bool,
    last_brake_offset_db: f32,
    slots_since_log: u32,
    up_bits: u32,
    down_bits: u32,
    metric_holds: u32,
    raw_hot_slots: u32,
    mobile_limit_downs: u32,
    unreliable: u32,
    envelope_rejects: u32,
    reused_metric_controls: u32,
    last_reliable_metric_age_slots: u32,
    metric_samples: u32,
    metric_snr_sum_db: f64,
    metric_snr_min_db: f32,
    metric_snr_max_db: f32,
    metric_coherence_sum: f64,
    metric_coherence_min: f32,
    metric_coherence_max: f32,
    clean_projected_raw: CorrelationStats,
    clean_rc3_raw: CorrelationStats,
    clean_rc3_pilot_power: CorrelationStats,
    clean_coherence_sum: f64,
}

#[derive(Debug, Clone, Copy)]
struct TimingRefinement {
    sample_delay: i32,
    sample_delay_fraction: f32,
    coherence: f32,
    snr_db: f32,
}

#[derive(Debug)]
struct RpcSlotTimingCandidate {
    sample_delay: i32,
    sample_delay_fraction: f32,
    moments: PilotMoments,
}

impl HrpdRpcController {
    pub(super) fn new() -> Self {
        Self {
            target_db: HRPD_RPC_TARGET_SNR_DB,
            metric_history: VecDeque::with_capacity(HRPD_RPC_METRIC_HISTORY_LEN),
            last_bit: 0,
            recovery_alt: 0,
            last_level_db: f32::NAN,
            last_predicted_level_db: None,
            last_prediction_delta_db: 0.0,
            last_slope_db_per_slot: 0.0,
            quiet_reliable_raw_power_dbfs: f32::NAN,
            last_reliable_mobile_power_dbfs: f32::NAN,
            filtered_raw_power_dbfs: None,
            brake_filtered_mobile_power_dbfs: None,
            mobile_limit_cooldown_slots: 0,
            mobile_hard_limiter_active: false,
            current_mobile_over_limit: false,
            last_brake_offset_db: 0.0,
            slots_since_log: 0,
            up_bits: 0,
            down_bits: 0,
            metric_holds: 0,
            raw_hot_slots: 0,
            mobile_limit_downs: 0,
            unreliable: 0,
            envelope_rejects: 0,
            reused_metric_controls: 0,
            last_reliable_metric_age_slots: u32::MAX,
            last_measured_level_db: f32::NAN,
            power_residual_db: 0.0,
            metric_samples: 0,
            metric_snr_sum_db: 0.0,
            metric_snr_min_db: f32::INFINITY,
            metric_snr_max_db: f32::NEG_INFINITY,
            metric_coherence_sum: 0.0,
            metric_coherence_min: f32::INFINITY,
            metric_coherence_max: f32::NEG_INFINITY,
            clean_projected_raw: CorrelationStats::default(),
            clean_rc3_raw: CorrelationStats::default(),
            clean_rc3_pilot_power: CorrelationStats::default(),
            clean_coherence_sum: 0.0,
        }
    }

    fn set_target_db(&mut self, target_db: f32) {
        if target_db.is_finite() {
            self.target_db = target_db;
        }
    }

    fn mobile_power_limited(&self) -> bool {
        self.current_mobile_over_limit
            || self.mobile_limit_cooldown_slots > 0
            || self.mobile_hard_limiter_active
    }

    fn record_log_metric(&mut self, moments: PilotMoments, raw_power_dbfs: f32) {
        if moments.snr_db.is_finite() && moments.coherence.is_finite() {
            self.metric_samples = self.metric_samples.saturating_add(1);
            self.metric_snr_sum_db += f64::from(moments.snr_db);
            self.metric_snr_min_db = self.metric_snr_min_db.min(moments.snr_db);
            self.metric_snr_max_db = self.metric_snr_max_db.max(moments.snr_db);
            self.metric_coherence_sum += f64::from(moments.coherence);
            self.metric_coherence_min = self.metric_coherence_min.min(moments.coherence);
            self.metric_coherence_max = self.metric_coherence_max.max(moments.coherence);
        }
        if moments.coherence >= HRPD_RPC_CLEAN_DIAG_MIN_COHERENCE
            && moments.pilot_amplitude_step_db <= HRPD_RPC_MAX_PILOT_AMPLITUDE_STEP_DB
            && moments.snr_db.is_finite()
            && moments.rc3_sinr_db.is_finite()
            && raw_power_dbfs.is_finite()
        {
            self.clean_projected_raw
                .observe(moments.snr_db, raw_power_dbfs);
            self.clean_rc3_raw
                .observe(moments.rc3_sinr_db, raw_power_dbfs);
            self.clean_rc3_pilot_power
                .observe(moments.rc3_sinr_db, mobile_pilot_power_dbfs(moments));
            self.clean_coherence_sum += f64::from(moments.coherence);
        }
    }

    fn avg_snr_db(&self) -> f32 {
        if self.metric_samples == 0 {
            f32::NAN
        } else {
            (self.metric_snr_sum_db / f64::from(self.metric_samples)) as f32
        }
    }

    fn avg_coherence(&self) -> f32 {
        if self.metric_samples == 0 {
            f32::NAN
        } else {
            (self.metric_coherence_sum / f64::from(self.metric_samples)) as f32
        }
    }

    fn reset_log_window(&mut self) {
        self.slots_since_log = 0;
        self.up_bits = 0;
        self.down_bits = 0;
        self.metric_holds = 0;
        self.raw_hot_slots = 0;
        self.mobile_limit_downs = 0;
        self.unreliable = 0;
        self.envelope_rejects = 0;
        self.reused_metric_controls = 0;
        self.metric_samples = 0;
        self.metric_snr_sum_db = 0.0;
        self.metric_snr_min_db = f32::INFINITY;
        self.metric_snr_max_db = f32::NEG_INFINITY;
        self.metric_coherence_sum = 0.0;
        self.metric_coherence_min = f32::INFINITY;
        self.metric_coherence_max = f32::NEG_INFINITY;
        self.clean_projected_raw = CorrelationStats::default();
        self.clean_rc3_raw = CorrelationStats::default();
        self.clean_rc3_pilot_power = CorrelationStats::default();
        self.clean_coherence_sum = 0.0;
    }

    fn record_emit(&mut self, bit: u8) -> u8 {
        self.last_bit = bit;
        if bit != 0 {
            self.down_bits = self.down_bits.saturating_add(1);
        } else {
            self.up_bits = self.up_bits.saturating_add(1);
        }
        bit
    }

    /// Ingest one measured slot's pilot SINR; returns the RC3-style predicted
    /// control metric when the slot is reliable, else `None` (pilot lost or the
    /// pilot was unavailable or changed amplitude within the observation.
    #[cfg(test)]
    pub(super) fn ingest(&mut self, snr_db: f32, coherence: f32) -> Option<f32> {
        self.ingest_with_amplitude_step(snr_db, coherence, 0.0)
    }

    pub(super) fn ingest_with_amplitude_step(
        &mut self,
        snr_db: f32,
        coherence: f32,
        pilot_amplitude_step_db: f32,
    ) -> Option<f32> {
        if pilot_amplitude_step_db > HRPD_RPC_MAX_PILOT_AMPLITUDE_STEP_DB {
            self.envelope_rejects = self.envelope_rejects.saturating_add(1);
        }
        if snr_db.is_finite()
            && coherence >= HRPD_RPC_MIN_COHERENCE_FOR_DECISION
            && pilot_amplitude_step_db <= HRPD_RPC_MAX_PILOT_AMPLITUDE_STEP_DB
        {
            if self.last_reliable_metric_age_slots > HRPD_RPC_MAX_REUSED_METRIC_AGE_SLOTS {
                self.metric_history.clear();
            }
            self.last_measured_level_db = snr_db;
            if self.metric_history.len() == HRPD_RPC_METRIC_HISTORY_LEN {
                self.metric_history.pop_front();
            }
            self.metric_history.push_back(snr_db);
            let (intercept_at_now, slope) = lsq_intercept_and_slope_at_newest(&self.metric_history);
            let predicted = predict_ahead_clamped(
                intercept_at_now,
                slope,
                HRPD_RPC_TX_LEAD_SLOTS as f32,
                HRPD_RPC_PREDICTION_CLAMP_DB,
            );
            self.last_level_db = intercept_at_now;
            self.last_predicted_level_db = Some(predicted);
            self.last_prediction_delta_db = predicted - intercept_at_now;
            self.last_slope_db_per_slot = slope;
            self.last_reliable_metric_age_slots = 0;
            Some(predicted)
        } else {
            self.unreliable = self.unreliable.saturating_add(1);
            self.last_reliable_metric_age_slots =
                self.last_reliable_metric_age_slots.saturating_add(1);
            None
        }
    }

    fn control_level(&mut self, current: Option<f32>) -> Option<f32> {
        if current.is_some() {
            return current;
        }
        if self.last_reliable_metric_age_slots == 0
            || self.last_reliable_metric_age_slots > HRPD_RPC_MAX_REUSED_METRIC_AGE_SLOTS
        {
            return None;
        }
        let predicted = self.last_predicted_level_db?;
        self.reused_metric_controls = self.reused_metric_controls.saturating_add(1);
        Some(predict_ahead_clamped(
            predicted,
            self.last_slope_db_per_slot,
            self.last_reliable_metric_age_slots as f32,
            HRPD_RPC_PREDICTION_CLAMP_DB,
        ))
    }

    fn control_mobile_power(&self, current: Option<f32>, current_mobile_power_dbfs: f32) -> f32 {
        if current.is_none()
            && self.last_reliable_metric_age_slots > 0
            && self.last_reliable_metric_age_slots <= HRPD_RPC_MAX_REUSED_METRIC_AGE_SLOTS
            && self.last_reliable_mobile_power_dbfs.is_finite()
        {
            self.last_reliable_mobile_power_dbfs
        } else {
            current_mobile_power_dbfs
        }
    }

    fn hold_neutral_bit(&mut self) -> u8 {
        let bit = self.recovery_alt;
        self.recovery_alt ^= 1;
        bit
    }

    fn compute_metric_bit_with_offset(
        &mut self,
        level_db: Option<f32>,
        level_offset_db: f32,
    ) -> u8 {
        match level_db {
            Some(level) => {
                let error_db = self.target_db - (level + level_offset_db);
                if self.last_measured_level_db.is_finite() {
                    let latest_error_db =
                        self.target_db - (self.last_measured_level_db + level_offset_db);
                    if latest_error_db >= HRPD_RPC_DIRECTION_GUARD_DB
                        && error_db < -HRPD_RPC_HOLD_BAND_DB
                    {
                        self.power_residual_db = 0.0;
                        return 0;
                    }
                    if latest_error_db <= -HRPD_RPC_DIRECTION_GUARD_DB
                        && error_db > HRPD_RPC_HOLD_BAND_DB
                    {
                        self.power_residual_db = 0.0;
                        return 1;
                    }
                }
                delta_sigma_pcb_step(
                    &mut self.power_residual_db,
                    error_db,
                    &HRPD_RPC_DELTA_SIGMA_PARAMS,
                )
            }
            None => self.hold_neutral_bit(),
        }
    }

    /// Emit one RPC bit (0=up, 1=down). A reliable level commands toward target
    /// using the RC3-style predicted control metric. A lost pilot holds
    /// net-neutral because raw power cannot distinguish a fade from DTX.
    #[cfg(test)]
    pub(super) fn emit(&mut self, level_db: Option<f32>) -> u8 {
        let bit = self.compute_metric_bit_with_offset(level_db, 0.0);
        self.record_emit(bit)
    }

    /// Record carrier-wide reverse input power for diagnostics only.
    pub(super) fn observe_raw_power(&mut self, raw_power_dbfs: f32) {
        if !raw_power_dbfs.is_finite() {
            return;
        }
        self.filtered_raw_power_dbfs = Some(match self.filtered_raw_power_dbfs {
            Some(prev) => prev + HRPD_RPC_RAW_FILTER_ALPHA * (raw_power_dbfs - prev),
            None => raw_power_dbfs,
        });

        if raw_power_dbfs >= HRPD_RPC_RAW_HOT_DIAG_DBFS {
            self.raw_hot_slots = self.raw_hot_slots.saturating_add(1);
        }
    }

    /// Record the assigned AT's coherent pilot power for brake and ceiling
    /// decisions. The AT's PN and long-code mask isolate this measurement.
    pub(super) fn observe_mobile_power(&mut self, mobile_power_dbfs: f32) {
        if !mobile_power_dbfs.is_finite() {
            self.current_mobile_over_limit = false;
            if self.mobile_limit_cooldown_slots > 0 {
                self.mobile_limit_cooldown_slots -= 1;
            }
            return;
        }
        self.brake_filtered_mobile_power_dbfs = Some(match self.brake_filtered_mobile_power_dbfs {
            Some(prev) => {
                let alpha = if mobile_power_dbfs > prev {
                    HRPD_RPC_BRAKE_ATTACK_ALPHA
                } else {
                    HRPD_RPC_BRAKE_RELEASE_ALPHA
                };
                prev + alpha * (mobile_power_dbfs - prev)
            }
            None => mobile_power_dbfs,
        });
        self.current_mobile_over_limit = mobile_power_dbfs >= HRPD_RPC_MOBILE_HARD_LIMIT_DBFS;
        if self.current_mobile_over_limit {
            self.mobile_limit_cooldown_slots = HRPD_RPC_MOBILE_LIMIT_COOLDOWN_SLOTS;
        } else if self.mobile_limit_cooldown_slots > 0 {
            self.mobile_limit_cooldown_slots -= 1;
        }
        if mobile_power_dbfs >= HRPD_RPC_MOBILE_HARD_LIMIT_DBFS {
            self.mobile_hard_limiter_active = true;
        } else if mobile_power_dbfs <= HRPD_RPC_MOBILE_HARD_RELEASE_DBFS {
            self.mobile_hard_limiter_active = false;
        }
    }

    /// Update the diagnostic raw floor only from a reliable measurement made
    /// in this slot. A recently reused quality estimate must not associate a
    /// silent slot's raw power with the prior active pilot.
    fn observe_reliable_raw_power(&mut self, raw_power_dbfs: f32) {
        if raw_power_dbfs.is_finite() {
            if !self.quiet_reliable_raw_power_dbfs.is_finite()
                || raw_power_dbfs < self.quiet_reliable_raw_power_dbfs
            {
                self.quiet_reliable_raw_power_dbfs = raw_power_dbfs;
            }
        }
    }

    fn observe_reliable_mobile_power(&mut self, mobile_power_dbfs: f32) {
        if mobile_power_dbfs.is_finite() {
            self.last_reliable_mobile_power_dbfs = mobile_power_dbfs;
        }
    }

    /// Per-mobile brake offset subtracted from the RPC error, ramping 0→max as
    /// pilot power crosses `BRAKE_BEGIN`→`BRAKE_FULL`. Keyed on
    /// `max(brake_filtered, instant)` so one hot burst engages it immediately.
    fn brake_offset_db(&self, instant_mobile_dbfs: f32) -> f32 {
        let braked = match self.brake_filtered_mobile_power_dbfs {
            Some(f) if instant_mobile_dbfs.is_finite() => f.max(instant_mobile_dbfs),
            Some(f) => f,
            None if instant_mobile_dbfs.is_finite() => instant_mobile_dbfs,
            None => return 0.0,
        };
        if braked <= HRPD_RPC_MOBILE_BRAKE_BEGIN_DBFS {
            return 0.0;
        }
        let span = HRPD_RPC_MOBILE_BRAKE_FULL_DBFS - HRPD_RPC_MOBILE_BRAKE_BEGIN_DBFS;
        let frac = ((braked - HRPD_RPC_MOBILE_BRAKE_BEGIN_DBFS) / span).clamp(0.0, 1.0);
        HRPD_RPC_MOBILE_BRAKE_MAX_OFFSET_DB * frac
    }

    /// Emit one RPC bit (0=up, 1=down). Pilot SINR controls toward target while
    /// assigned-mobile pilot power applies the soft brake and hard ceiling.
    pub(super) fn emit_with_mobile_power(
        &mut self,
        level_db: Option<f32>,
        mobile_power_dbfs: f32,
    ) -> u8 {
        let brake = self.brake_offset_db(mobile_power_dbfs);
        self.last_brake_offset_db = brake;

        if self.current_mobile_over_limit
            || mobile_power_dbfs >= HRPD_RPC_MOBILE_HARD_LIMIT_DBFS
            || self.mobile_limit_cooldown_slots > 0
            || self.mobile_hard_limiter_active
        {
            self.mobile_limit_downs = self.mobile_limit_downs.saturating_add(1);
            self.power_residual_db = 0.0;
            return self.record_emit(1);
        }

        if let Some(level) = level_db {
            // Brake lowers the effective setpoint as mobile pilot power rises.
            let bit = self.compute_metric_bit_with_offset(Some(level), brake);
            return self.record_emit(bit);
        }

        // Metric loss is ambiguous because subtype-3 DTX does not respond to
        // RPC during silent intervals. Hold until a coherent metric returns.
        self.metric_holds = self.metric_holds.saturating_add(1);
        let bit = self.hold_neutral_bit();
        self.record_emit(bit)
    }
}

/// Spawn-time pilot lock state captured from the correlator search.
#[derive(Debug, Clone, Copy)]
pub struct HrpdReverseTrafficFingerLock {
    pub frame_start_chip: u64,
    pub chip_offset: i32,
    pub sample_delay: i32,
    pub sample_delay_fraction: f32,
    pub q_sign: f32,
    pub q_pair_phase: u64,
    pub initial_pilot_phase: Complex32,
}

/// Static config handed in at spawn time.
#[derive(Debug, Clone)]
pub struct HrpdReverseTrafficFingerConfig {
    pub uati: u32,
    pub mac_index: u8,
    pub physical_layer_subtype: u16,
    pub reverse_traffic_mac_subtype: u16,
    pub frame_offset: u8,
    pub i_mask: u64,
    pub q_mask: u64,
    /// DRCCover for the assignment (per C.S0024-0 v4.0 §9.2.1.3.3.3).
    pub drc_cover: u8,
    /// DRCLength in slots (1, 2, 4, or 8).
    pub drc_length: u8,
    /// Sample oversample ratio that the buffered IQ arrives at.
    pub oversample: usize,
    /// Event sink used to notify the AN only after this finger proves a
    /// coherent reverse pilot lock.
    pub event_tx: Option<tokio_mpsc::UnboundedSender<HrpdTrafficEvent>>,
    /// Shared HRPD MAC bus used to publish the live RPC bit consumed by the
    /// Forward MAC encoder. RPC bit 0 commands power up; bit 1 commands power
    /// down.
    pub harq_bus: Option<Arc<HarqBus>>,
    pub power_control: Option<HrpdPowerControlHandle>,
    /// Set when this traffic receiver validates the reverse pilot. The RX
    /// loop uses this to stop feeding the HRPD Access Channel correlator while
    /// the connection is open on dedicated reverse traffic resources.
    pub reverse_pilot_acquired: Option<Arc<AtomicBool>>,
    /// When the owning traffic RX worker was created. The first validated
    /// pilot logs elapsed-since-spawn against the TRTCMPANSetup 3.0 s budget
    /// (C.S0024-0 v4.0): the AN must acquire the reverse channel and put
    /// RTCAck on air within 1 s of the TrafficChannelAssignment.
    pub worker_spawned_at: std::time::Instant,
}

/// HRPD reverse-traffic RAKE finger.
pub struct HrpdReverseTrafficFinger {
    base: BaseFinger,
    config: HrpdReverseTrafficFingerConfig,
    /// Mask candidate derived from `config` + lock-time conventions; cached so
    /// every frame uses the same despread reference.
    mask: HrpdTrafficMaskCandidate,
    /// Per-finger persistent state used to despread the next frame. The pilot
    /// phase here is the *running* estimate, updated per frame from the
    /// previous frame's residual phase (simple per-frame re-estimate; see
    /// module docs for the design rationale).
    next_params: HrpdReverseTrafficDespreadParams,
    /// Cached `conj(reference)` for the locked PN/LC phase. Reverse traffic
    /// physical frames are one full PN/LC period, so this repeats every
    /// frame while the finger is active.
    ref_conj: Vec<Complex32>,
    /// Local IQ buffer for the next pending frame. `buffer_abs_sample` is the
    /// absolute sample index of `buffer[0]`.
    buffer: Vec<Complex32>,
    buffer_abs_sample: Option<u64>,
    sample_rate_hz: f64,
    latest_rx_sample_time: Option<RxSampleTimeAnchor>,
    /// Counters used for finger lifecycle (idle/validation).
    consecutive_low_coherence: u32,
    consecutive_high_coherence: u32,
    hard_validated_locally: bool,
    reverse_pilot_event_sent: bool,
    last_reverse_pilot_event_chip: Option<u64>,
    reverse_pilot_lost_event_sent: bool,
    low_coherence_start_chip: Option<u64>,
    last_good_pilot_chip: u64,
    last_good_pilot_snr_db: f32,
    last_good_pilot_coherence: f32,
    low_coherence_reports: u32,
    phase_hold_reports: u32,
    frames_since_timing_refine: u32,
    frames_since_lost_timing_refine: u32,
    timing_refine_reports: u32,
    rpc_timing_refine_reports: u32,
    timing_state: HrpdTrafficTimingState,
    timing_unreliable_slots: u8,
    timing_reacquire_confirm_slots: u8,
    timing_reacquire_search_wait_slots: u32,
    timing_reacquire_search_interval_slots: u32,
    timing_last_pilot_reliable: bool,
    /// Per-slot reverse power-control loop (predictor + HRPD RPC decision).
    rpc: HrpdRpcController,
    /// Low-latency DRC publisher. The frame-rate DRC processor still fills
    /// diagnostic tags, but scheduler-facing DRC cannot wait for a full
    /// 16-slot reverse traffic frame.
    fast_drc_decoder: DrcDecoder,
    fast_drc_candidate_slot: Option<u64>,
    fast_drc_candidate_value: Option<u8>,
    fast_drc_candidate_run: u8,
    fast_drc_last_published_slot: Option<u64>,
    fast_drc_last_published_value: Option<u8>,
    fast_drc_stats: FastDrcStats,
    /// Consecutive slot-rate RPC measurements that failed the pilot reliability
    /// gate. Logged once per run so live traces show the collapse before the
    /// 5-second reverse-pilot-lost timer fires.
    rpc_unreliable_streak_slots: u32,
    rpc_unreliable_start_slot: Option<u64>,
    rpc_unreliable_reported: bool,
    power_control_tune_away_active: bool,
    /// Absolute chip of the next slot the per-slot RPC loop will measure. Runs
    /// ahead of the frame data cursor so RPC tracks at slot rate.
    next_rpc_slot_chip: u64,
    /// Absolute chip of the next 4-slot sub-frame boundary the subtype-3
    /// reverse data path will despread. Aligned to
    /// (T − FrameOffset) mod 4 = 0.
    next_subframe_chip: u64,
    /// Per-interlace HARQ packet accumulation for subtype-3 sessions.
    subframe_harq: SubframeHarq,
    /// Parallel HARQ state for inverted-Q data while subtype-3 data polarity
    /// is still unknown. B4 modulation puts the code symbol on the Q branch
    /// with the I branch all zeros (C.S0024-A §13.2.1.3.9.1), so only the two
    /// Q polarities are probed; the acquired `q_sign` is pilot-blind, so which
    /// one is correct is resolved from the data and CRC-24, then locked.
    subframe_harq_inverted_q: SubframeHarq,
    /// Parallel HARQ state for non-B4 data transform probes. RRI stays on the
    /// derotated pilot reference; only the data-channel symbols are probed.
    subframe_harq_data_transforms: [SubframeHarq; SUBTYPE2_DATA_TRANSFORM_COUNT],
    /// Rolling decode counters for the subtype-3 subframe path.
    subframe_stats: SubframeDecodeStats,
    /// Non-B4 data transform selected by the first CRC-valid subtype-3
    /// reverse sub-frame.
    subframe_data_transform: Option<Subtype2DataTransform>,
    /// Data-channel Q polarity selected by the first CRC-valid subtype-3
    /// reverse sub-frame. RRI stays on the I arm; only data symbols are probed.
    subframe_invert_q: Option<bool>,
    /// B4 branch selected by the first CRC-valid B4 reverse sub-frame.
    subframe_b4_lane: Option<B4DataLane>,
    /// Remaining per-stage sub-frame diagnostics before this finger goes
    /// quiet (first-connection bring-up visibility).
    subframe_diag_reports: u32,
    subframe_phase_diag_reports: u32,
    /// Absolute mid-slot chip where the next repeated DRC slot begins.
    next_drc_repetition_chip: u64,
    /// Absolute mid-slot chip where the next DRC integration window begins.
    next_drc_window_chip: u64,
    spawn_chip: u64,
    last_block_processed_chips: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum HrpdTrafficTimingState {
    Tracking,
    Reacquiring,
}

#[derive(Debug, Default)]
struct FastDrcStats {
    window_start_slot: Option<u64>,
    repetition_attempts: u32,
    repetition_none: u32,
    repetition_invalid: u32,
    repetition_low_confidence: u32,
    repetition_low_confidence_same_as_last: u32,
    repetition_low_confidence_min: Option<f32>,
    repetition_low_confidence_max: Option<f32>,
    repetition_low_confidence_values: [u32; 16],
    repetition_candidates: u32,
    repetition_published: u32,
    repetition_duplicates: u32,
    window_attempts: u32,
    window_none: u32,
    window_invalid: u32,
    window_low_confidence: u32,
    window_low_confidence_same_as_last: u32,
    window_low_confidence_min: Option<f32>,
    window_low_confidence_max: Option<f32>,
    window_low_confidence_values: [u32; 16],
    window_published: u32,
    max_publish_gap_slots: u64,
}

#[derive(Debug, Default)]
struct SubframeDecodeStats {
    window_start_slot: Option<u64>,
    total: u32,
    low_coherence: u32,
    no_rri: u32,
    null_rri: u32,
    low_margin: u32,
    invalid: u32,
    non_null_rri: u32,
    decoded: u32,
    delivered: u32,
    mac_parse_failed: u32,
    turbo_attempts: u32,
    turbo_iterations: u32,
    turbo_iterations_max: u8,
}

impl HrpdReverseTrafficFinger {
    pub fn new(
        id: u64,
        config: HrpdReverseTrafficFingerConfig,
        lock: HrpdReverseTrafficFingerLock,
    ) -> Self {
        let mask = HrpdTrafficMaskCandidate {
            i_mask: config.i_mask,
            q_mask: config.q_mask,
            q_sign: lock.q_sign,
            q_pair_phase: lock.q_pair_phase,
            label: "finger",
        };
        let next_params = HrpdReverseTrafficDespreadParams {
            frame_start_chip: lock.frame_start_chip,
            chip_offset: lock.chip_offset,
            sample_delay: lock.sample_delay,
            sample_delay_fraction: lock.sample_delay_fraction,
            pilot_phase: lock.initial_pilot_phase,
            mask,
        };
        let ref_conj = hrpd_reverse_traffic_reference_conj(
            lock.frame_start_chip,
            HRPD_TRAFFIC_FRAME_CHIPS,
            mask,
        );
        let drc_cover = config.drc_cover;
        let frame_offset_slots = config.frame_offset;
        let drc_length_slots = config.drc_length.max(1);
        let first_drc_window_slot = drc_window_start_slot_at_or_after(
            lock.frame_start_chip / HRPD_SLOT_CHIPS as u64,
            frame_offset_slots,
            drc_length_slots,
        );
        let power_control_tune_away_active = config
            .power_control
            .as_ref()
            .and_then(HrpdPowerControlHandle::snapshot)
            .is_some_and(|snapshot| snapshot.tune_away_active);
        Self {
            base: BaseFinger::new(id),
            config,
            mask,
            next_params,
            ref_conj,
            buffer: Vec::new(),
            buffer_abs_sample: None,
            sample_rate_hz: 0.0,
            latest_rx_sample_time: None,
            consecutive_low_coherence: 0,
            consecutive_high_coherence: 0,
            hard_validated_locally: false,
            reverse_pilot_event_sent: false,
            last_reverse_pilot_event_chip: None,
            reverse_pilot_lost_event_sent: false,
            low_coherence_start_chip: None,
            last_good_pilot_chip: lock.frame_start_chip,
            last_good_pilot_snr_db: f32::NAN,
            last_good_pilot_coherence: 0.0,
            low_coherence_reports: 0,
            phase_hold_reports: 0,
            frames_since_timing_refine: 0,
            frames_since_lost_timing_refine: HRPD_TIMING_TRACK_LOST_INTERVAL_FRAMES,
            timing_refine_reports: 0,
            rpc_timing_refine_reports: 0,
            timing_state: HrpdTrafficTimingState::Tracking,
            timing_unreliable_slots: 0,
            timing_reacquire_confirm_slots: 0,
            timing_reacquire_search_wait_slots: 0,
            timing_reacquire_search_interval_slots: HRPD_TIMING_REACQUIRE_SEARCH_INITIAL_SLOTS,
            timing_last_pilot_reliable: true,
            rpc: HrpdRpcController::new(),
            fast_drc_decoder: DrcDecoder::new(drc_cover),
            fast_drc_candidate_slot: None,
            fast_drc_candidate_value: None,
            fast_drc_candidate_run: 0,
            fast_drc_last_published_slot: None,
            fast_drc_last_published_value: None,
            fast_drc_stats: FastDrcStats::default(),
            rpc_unreliable_streak_slots: 0,
            rpc_unreliable_start_slot: None,
            rpc_unreliable_reported: false,
            power_control_tune_away_active,
            next_rpc_slot_chip: lock.frame_start_chip,
            next_subframe_chip: first_aligned_subframe_chip(
                lock.frame_start_chip,
                frame_offset_slots,
            ),
            subframe_harq: SubframeHarq::new(),
            subframe_harq_inverted_q: SubframeHarq::new(),
            subframe_harq_data_transforms: std::array::from_fn(|_| SubframeHarq::new()),
            subframe_stats: SubframeDecodeStats::default(),
            subframe_data_transform: None,
            subframe_invert_q: None,
            subframe_b4_lane: None,
            subframe_diag_reports: HRPD_SUBFRAME_DIAG_REPORTS_MAX,
            subframe_phase_diag_reports: HRPD_SUBFRAME_PHASE_DIAG_REPORTS_MAX,
            next_drc_repetition_chip: lock
                .frame_start_chip
                .saturating_add(DRC_MID_SLOT_OFFSET_CHIPS),
            next_drc_window_chip: first_drc_window_slot
                .saturating_mul(HRPD_SLOT_CHIPS as u64)
                .saturating_add(DRC_MID_SLOT_OFFSET_CHIPS),
            spawn_chip: lock.frame_start_chip,
            last_block_processed_chips: 0,
        }
    }

    fn log_subframe_diag(
        &mut self,
        start_slot: u64,
        stage: &str,
        moments: Option<PilotMoments>,
        detection: Option<&RriSubtype2Detection>,
    ) {
        if self.subframe_diag_reports == 0 {
            return;
        }
        self.subframe_diag_reports -= 1;
        let (coh, snr) = moments
            .map(|m| (m.coherence, m.snr_db))
            .unwrap_or((f32::NAN, f32::NAN));
        match detection {
            Some(det) => debug!(
                "rx_hrpd_traffic[m{}]: subframe_diag uati=0x{:08x} slot={} stage={} coh={:.3} snr={:.1}dB rri_payload_idx=0x{:x} rri_subpacket={} best={:.3} second={:.3} margin={:.3}",
                self.config.mac_index,
                self.config.uati,
                start_slot,
                stage,
                coh,
                snr,
                det.payload_index,
                det.subpacket_id,
                det.best_score,
                det.second_score,
                det.margin,
            ),
            None => debug!(
                "rx_hrpd_traffic[m{}]: subframe_diag uati=0x{:08x} slot={} stage={} coh={:.3} snr={:.1}dB",
                self.config.mac_index, self.config.uati, start_slot, stage, coh, snr,
            ),
        }
    }

    fn reference_window(&self, start_chip: u64, chips: usize) -> Option<Vec<Complex32>> {
        let offset = start_chip.checked_sub(self.spawn_chip)? % HRPD_TRAFFIC_FRAME_CHIPS as u64;
        let base = offset as usize;
        Some(
            (0..chips)
                .map(|idx| self.ref_conj[(base + idx) % self.ref_conj.len()])
                .collect(),
        )
    }

    fn subtype2_rri_detection_at(
        &self,
        start_chip: u64,
        oversample: usize,
        sample_delay: i32,
        sample_delay_fraction: f32,
    ) -> Option<(PilotMoments, Option<RriSubtype2Detection>)> {
        let buf_abs = self.buffer_abs_sample?;
        let oversample_u64 = oversample.max(1) as u64;
        let start_sample = (start_chip * oversample_u64) as i64;
        let earliest = start_sample + sample_delay as i64;
        let latest = start_sample
            + (HRPD_SUBFRAME_CHIPS as i64 - 1) * oversample_u64 as i64
            + sample_delay as i64
            + 2;
        if earliest < buf_abs as i64 || latest >= (buf_abs + self.buffer.len() as u64) as i64 {
            return None;
        }
        let reference = self.reference_window(start_chip, HRPD_SUBFRAME_CHIPS)?;
        let mut chips = despread_chips_with_reference(
            &self.buffer,
            buf_abs,
            oversample,
            start_chip,
            sample_delay,
            sample_delay_fraction,
            Complex32::new(1.0, 0.0),
            &reference,
        )?;
        let moments = pilot_moments_from_subtype2_slot_regions(&chips, 4);
        if moments.coherence < HRPD_REVERSE_TRAFFIC_PILOT_MIN_COHERENCE {
            return Some((moments, None));
        }
        derotate_frame_by_pilot_ramp(&mut chips, moments);
        let detection = decode_rri_subtype2_subframe(&chips);
        Some((moments, detection))
    }

    fn log_subframe_phase_diag(
        &mut self,
        start_slot: u64,
        oversample: usize,
        sample_delay: i32,
        sample_delay_fraction: f32,
    ) {
        if self.subframe_phase_diag_reports == 0 {
            return;
        }
        self.subframe_phase_diag_reports -= 1;
        let mut parts = Vec::with_capacity(4);
        for phase in 0..4u64 {
            let offset_slots = (4 - phase) & 0x03;
            let Some(scan_slot) = start_slot.checked_sub(offset_slots) else {
                continue;
            };
            let scan_chip = scan_slot * HRPD_SLOT_CHIPS as u64;
            match self.subtype2_rri_detection_at(
                scan_chip,
                oversample,
                sample_delay,
                sample_delay_fraction,
            ) {
                Some((moments, Some(det))) => parts.push(format!(
                    "p{}@{} coh={:.3} snr={:.1} idx=0x{:x}/{} best={:.3} margin={:.3}",
                    phase,
                    scan_slot,
                    moments.coherence,
                    moments.snr_db,
                    det.payload_index,
                    det.subpacket_id,
                    det.best_score,
                    det.margin
                )),
                Some((moments, None)) => parts.push(format!(
                    "p{}@{} coh={:.3} snr={:.1} low",
                    phase, scan_slot, moments.coherence, moments.snr_db
                )),
                None => parts.push(format!("p{}@{} unavailable", phase, scan_slot)),
            }
        }
        if !parts.is_empty() {
            debug!(
                "rx_hrpd_traffic[m{}]: subframe_phase_diag uati=0x{:08x} aligned_slot={} frame_offset={} {}",
                self.config.mac_index,
                self.config.uati,
                start_slot,
                self.config.frame_offset,
                parts.join(" | ")
            );
        }
    }

    fn ingest_null_subframe(
        &mut self,
        start_slot: u64,
        detection: &RriSubtype2Detection,
    ) -> SubframeOutcome {
        for harq in &mut self.subframe_harq_data_transforms {
            let _ = harq.ingest_subframe(start_slot, self.config.frame_offset, detection, &[], &[]);
        }
        let raw = self.subframe_harq.ingest_subframe(
            start_slot,
            self.config.frame_offset,
            detection,
            &[],
            &[],
        );
        let inverted = self.subframe_harq_inverted_q.ingest_subframe(
            start_slot,
            self.config.frame_offset,
            detection,
            &[],
            &[],
        );
        match self.subframe_b4_lane {
            Some(B4DataLane::QInverted) => inverted,
            Some(B4DataLane::QRaw) => raw,
            None if self.subframe_invert_q == Some(true) => inverted,
            None => raw,
        }
    }

    fn ingest_unsupported_subframe(
        &mut self,
        start_slot: u64,
        detection: &RriSubtype2Detection,
    ) -> SubframeOutcome {
        let raw = self.subframe_harq.ingest_subframe(
            start_slot,
            self.config.frame_offset,
            detection,
            &[],
            &[],
        );
        let inverted = self.subframe_harq_inverted_q.ingest_subframe(
            start_slot,
            self.config.frame_offset,
            detection,
            &[],
            &[],
        );
        if self.subframe_invert_q == Some(true) {
            inverted
        } else {
            raw
        }
    }

    fn ingest_subtype2_data_subframe(
        &mut self,
        start_slot: u64,
        detection: &RriSubtype2Detection,
        chips: &[Complex32],
        format: &Subtype2DataFormat,
    ) -> SubframeOutcome {
        if matches!(format.modulation, ModulationFormat::B4) {
            return self.ingest_b4_data_subframe(start_slot, detection, chips, format);
        }
        if let Some(transform) = self.subframe_data_transform {
            let (w24, w12) = decover_subframe_data(chips, format, transform);
            return self.subframe_harq_data_transforms[transform.index()].ingest_subframe(
                start_slot,
                self.config.frame_offset,
                detection,
                &w24,
                &w12,
            );
        }

        let mut outcomes = Vec::with_capacity(SUBTYPE2_DATA_TRANSFORM_COUNT);
        for transform in SUBTYPE2_DATA_TRANSFORMS {
            let (w24, w12) = decover_subframe_data(chips, format, transform);
            let outcome = self.subframe_harq_data_transforms[transform.index()].ingest_subframe(
                start_slot,
                self.config.frame_offset,
                detection,
                &w24,
                &w12,
            );
            outcomes.push((transform, outcome));
        }

        if let Some((transform, outcome)) = outcomes
            .iter()
            .find(|(_, outcome)| outcome.decoded)
            .map(|(transform, outcome)| (*transform, outcome.clone()))
        {
            self.subframe_data_transform = Some(transform);
            match transform.q_inversion() {
                Some(invert_q) => self.subframe_invert_q = Some(invert_q),
                None => self.subframe_invert_q = None,
            }
            info!(
                "rx_hrpd_traffic[m{}]: subtype3 data transform locked uati=0x{:08x} transform={} payload_bits={} subpacket={} mean_abs_llr={:.3} mother_mean={:.3}",
                self.config.mac_index,
                self.config.uati,
                transform.name(),
                detection.payload_bits,
                detection.subpacket_id,
                outcome.llr_mean_abs,
                outcome.mother_mean_abs,
            );
            return outcome;
        }

        let probe = outcomes
            .iter()
            .map(|(transform, outcome)| {
                format!(
                    "{}:dec={} llr={:.3} mother={:.3}",
                    transform.name(),
                    outcome.decoded,
                    outcome.llr_mean_abs,
                    outcome.mother_mean_abs
                )
            })
            .collect::<Vec<_>>()
            .join(" ");
        debug!(
            "rx_hrpd_traffic[m{}]: subtype3 data transform probe uati=0x{:08x} start_slot={} format={:?} payload_bits={} subpacket={} {}",
            self.config.mac_index,
            self.config.uati,
            start_slot,
            format.modulation,
            detection.payload_bits,
            detection.subpacket_id,
            probe,
        );
        outcomes[Subtype2DataTransform::Raw.index()].1.clone()
    }

    fn ingest_b4_data_subframe(
        &mut self,
        start_slot: u64,
        detection: &RriSubtype2Detection,
        chips: &[Complex32],
        format: &Subtype2DataFormat,
    ) -> SubframeOutcome {
        match self.subframe_b4_lane {
            Some(B4DataLane::QRaw) => {
                let (w24, w12) = decover_subframe_data(chips, format, Subtype2DataTransform::Raw);
                self.subframe_harq.ingest_subframe(
                    start_slot,
                    self.config.frame_offset,
                    detection,
                    &w24,
                    &w12,
                )
            }
            Some(B4DataLane::QInverted) => {
                let (w24, w12) =
                    decover_subframe_data(chips, format, Subtype2DataTransform::Conjugate);
                self.subframe_harq_inverted_q.ingest_subframe(
                    start_slot,
                    self.config.frame_offset,
                    detection,
                    &w24,
                    &w12,
                )
            }
            None => {
                let (raw_w24, raw_w12) =
                    decover_subframe_data(chips, format, Subtype2DataTransform::Raw);
                let raw = self.subframe_harq.ingest_subframe(
                    start_slot,
                    self.config.frame_offset,
                    detection,
                    &raw_w24,
                    &raw_w12,
                );
                let raw_energy = b4_q_branch_mean_abs_llr(&raw_w24);
                let (inv_w24, inv_w12) =
                    decover_subframe_data(chips, format, Subtype2DataTransform::Conjugate);
                let inverted = self.subframe_harq_inverted_q.ingest_subframe(
                    start_slot,
                    self.config.frame_offset,
                    detection,
                    &inv_w24,
                    &inv_w12,
                );
                let inverted_energy = b4_q_branch_mean_abs_llr(&inv_w24);
                // CRC-24 is the branch discriminator. The decovered symbol
                // amplitude depends on receiver scaling and reverse T2P; a
                // second absolute-amplitude gate can discard an otherwise
                // valid setup packet and strand TrafficChannelComplete.
                let raw_valid = raw.decoded;
                let inverted_valid = inverted.decoded;
                if raw_valid && raw_energy >= inverted_energy {
                    self.subframe_b4_lane = Some(B4DataLane::QRaw);
                    self.subframe_invert_q = Some(false);
                    info!(
                        "rx_hrpd_traffic[m{}]: subtype3 B4 data branch locked uati=0x{:08x} branch=qraw mean_abs_llr={:.3}",
                        self.config.mac_index, self.config.uati, raw_energy,
                    );
                    raw
                } else if inverted_valid {
                    self.subframe_b4_lane = Some(B4DataLane::QInverted);
                    self.subframe_invert_q = Some(true);
                    info!(
                        "rx_hrpd_traffic[m{}]: subtype3 B4 data branch locked uati=0x{:08x} branch=qinv mean_abs_llr={:.3}",
                        self.config.mac_index, self.config.uati, inverted_energy,
                    );
                    inverted
                } else {
                    debug!(
                        "rx_hrpd_traffic[m{}]: subtype3 B4 data branch probe uati=0x{:08x} start_slot={} payload_bits={} subpacket={} raw_decoded={} raw_q_energy={:.3} raw_llr={:.3} raw_mother={:.3} inv_decoded={} inv_q_energy={:.3} inv_llr={:.3} inv_mother={:.3}",
                        self.config.mac_index,
                        self.config.uati,
                        start_slot,
                        detection.payload_bits,
                        detection.subpacket_id,
                        raw.decoded,
                        raw_energy,
                        raw.llr_mean_abs,
                        raw.mother_mean_abs,
                        inverted.decoded,
                        inverted_energy,
                        inverted.llr_mean_abs,
                        inverted.mother_mean_abs,
                    );
                    raw
                }
            }
        }
    }

    fn report_terminal_packet(&self, terminal: TerminalPacketOutcome) {
        let Some(power_control) = &self.config.power_control else {
            return;
        };
        let (mut outcome, transmission_mode, termination_target, late_success) = match terminal
            .disposition
        {
            TerminalPacketDisposition::Decoded { transmission_mode } => {
                let Some(target) = power_control
                    .termination_target_subpackets(terminal.payload_bits, transmission_mode)
                else {
                    let _ = power_control.report(HrpdPacketObservation {
                        outcome: HrpdPacketOutcome::Excluded(
                            HrpdPacketExclusion::UnknownTerminationTarget,
                        ),
                        payload_bits: Some(terminal.payload_bits),
                        transmission_mode: Some(transmission_mode),
                        decoded_subpacket: terminal.decoded_subpacket,
                        termination_target_subpackets: None,
                        late_success: false,
                    });
                    return;
                };
                let late = terminal
                    .decoded_subpacket
                    .is_some_and(|decoded| decoded > target);
                (
                    if late {
                        HrpdPacketOutcome::Erasure
                    } else {
                        HrpdPacketOutcome::Success
                    },
                    Some(transmission_mode),
                    Some(target),
                    late,
                )
            }
            TerminalPacketDisposition::Exhausted => (HrpdPacketOutcome::Erasure, None, None, false),
            TerminalPacketDisposition::Abandoned => (
                HrpdPacketOutcome::Excluded(HrpdPacketExclusion::AbandonedHarq),
                None,
                None,
                false,
            ),
        };
        if self.timing_state != HrpdTrafficTimingState::Tracking {
            outcome = HrpdPacketOutcome::Excluded(HrpdPacketExclusion::ReceiverReacquiring);
        } else if outcome == HrpdPacketOutcome::Erasure && self.rpc.mobile_power_limited() {
            outcome = HrpdPacketOutcome::Excluded(HrpdPacketExclusion::MobilePowerLimited);
        }
        let _ = power_control.report(HrpdPacketObservation {
            outcome,
            payload_bits: Some(terminal.payload_bits),
            transmission_mode,
            decoded_subpacket: terminal.decoded_subpacket,
            termination_target_subpackets: termination_target,
            late_success,
        });
    }

    /// Process every buffered subtype-3 4-slot sub-frame: despread, decode
    /// RRI, accumulate data LLRs per interlace, attempt early-termination
    /// turbo decode, publish forward ARQ levels, and deliver CRC-valid MAC
    /// packets exactly once.
    fn process_pending_subframes(&mut self) {
        if self.config.reverse_traffic_mac_subtype
            != cdma_common::hrpd::traffic::REVERSE_TRAFFIC_MAC_SUBTYPE3
        {
            return;
        }
        let oversample = self.config.oversample.max(1) as u64;
        let subframe_chips = HRPD_SUBFRAME_CHIPS;
        loop {
            let Some(buf_abs) = self.buffer_abs_sample else {
                return;
            };
            let start_chip = self.next_subframe_chip;
            let start_sample = (start_chip * oversample) as i64;
            let sample_delay = self.next_params.sample_delay;
            let sample_delay_fraction = self.next_params.sample_delay_fraction;
            let earliest = start_sample + sample_delay as i64;
            let latest = start_sample
                + (subframe_chips as i64 - 1) * oversample as i64
                + sample_delay as i64
                + 2;
            if earliest < buf_abs as i64 {
                self.next_subframe_chip = start_chip + subframe_chips as u64;
                continue;
            }
            if latest >= (buf_abs + self.buffer.len() as u64) as i64 {
                return;
            }
            let ref_base =
                ((start_chip - self.spawn_chip) % HRPD_TRAFFIC_FRAME_CHIPS as u64) as usize;
            // The reference repeats every 16-slot frame and a sub-frame never
            // crosses a frame boundary (both are 4-slot aligned).
            let ref_range = ref_base..ref_base + subframe_chips;
            let despread = despread_chips_with_reference(
                &self.buffer,
                buf_abs,
                oversample as usize,
                start_chip,
                sample_delay,
                sample_delay_fraction,
                Complex32::new(1.0, 0.0),
                &self.ref_conj[ref_range.clone()],
            );
            self.next_subframe_chip = start_chip + subframe_chips as u64;
            let start_slot = start_chip / HRPD_SLOT_CHIPS as u64;
            let Some(mut chips) = despread else {
                self.subframe_stats.total = self.subframe_stats.total.saturating_add(1);
                self.subframe_stats.no_rri = self.subframe_stats.no_rri.saturating_add(1);
                self.log_subframe_diag(start_slot, "despread_none", None, None);
                self.maybe_report_subframe_stats(start_slot);
                continue;
            };
            let moments = pilot_moments_from_subtype2_slot_regions(&chips, 4);
            if moments.coherence < HRPD_REVERSE_TRAFFIC_PILOT_MIN_COHERENCE {
                self.subframe_stats.total = self.subframe_stats.total.saturating_add(1);
                self.subframe_stats.low_coherence =
                    self.subframe_stats.low_coherence.saturating_add(1);
                self.log_subframe_diag(start_slot, "low_coherence", Some(moments), None);
                self.maybe_report_subframe_stats(start_slot);
                continue;
            }
            derotate_frame_by_pilot_ramp(&mut chips, moments);
            let rri_expected_best = rri_expected_best_from_pilot(&chips, 4);
            let detection = decode_rri_subtype2_subframe(&chips);
            self.log_subframe_phase_diag(
                start_slot,
                oversample as usize,
                sample_delay,
                sample_delay_fraction,
            );
            self.log_subframe_diag(start_slot, "rri", Some(moments), detection.as_ref());
            let Some(detection) = detection else {
                self.subframe_stats.total = self.subframe_stats.total.saturating_add(1);
                self.subframe_stats.no_rri = self.subframe_stats.no_rri.saturating_add(1);
                self.maybe_report_subframe_stats(start_slot);
                continue;
            };
            // The rate and sub-packet the RRI reports are trusted directly: a
            // low decision margin drops the sub-frame and CRC-24 confirms
            // delivery.
            let observed_rri_margin_norm =
                normalized_rri_score(detection.margin, rri_expected_best);
            let observed_rri_best_norm =
                normalized_rri_score(detection.best_score, rri_expected_best);
            if is_subframe_rri_margin_low(&detection, observed_rri_margin_norm) {
                self.subframe_stats.total = self.subframe_stats.total.saturating_add(1);
                self.subframe_stats.low_margin = self.subframe_stats.low_margin.saturating_add(1);
                if detection.payload_index != RRI_SUBTYPE2_NULL_PAYLOAD_INDEX {
                    debug!(
                        "rx_hrpd_traffic[m{}]: subframe RRI rejected low margin uati=0x{:08x} start_slot={} payload_idx=0x{:x} subpacket={} margin={:.3} margin_norm={:.3} best_norm={:.3}",
                        self.config.mac_index,
                        self.config.uati,
                        start_slot,
                        detection.payload_index,
                        detection.subpacket_id,
                        detection.margin,
                        observed_rri_margin_norm,
                        observed_rri_best_norm,
                    );
                }
                self.maybe_report_subframe_stats(start_slot);
                continue;
            }
            if !is_rri_subtype2_valid(&detection) {
                self.subframe_stats.total = self.subframe_stats.total.saturating_add(1);
                self.subframe_stats.invalid = self.subframe_stats.invalid.saturating_add(1);
                debug!(
                    "rx_hrpd_traffic[m{}]: subframe RRI rejected invalid null subpacket uati=0x{:08x} start_slot={} payload_idx=0x{:x} subpacket={}",
                    self.config.mac_index,
                    self.config.uati,
                    start_slot,
                    detection.payload_index,
                    detection.subpacket_id,
                );
                self.maybe_report_subframe_stats(start_slot);
                continue;
            }
            self.subframe_stats.total = self.subframe_stats.total.saturating_add(1);
            if is_rri_subtype2_null(&detection) {
                self.subframe_stats.null_rri = self.subframe_stats.null_rri.saturating_add(1);
            } else {
                self.subframe_stats.non_null_rri =
                    self.subframe_stats.non_null_rri.saturating_add(1);
            }
            let outcome = if is_rri_subtype2_null(&detection) {
                self.ingest_null_subframe(start_slot, &detection)
            } else if let Some(format) =
                Subtype2DataFormat::for_payload_bits(detection.payload_bits as usize)
            {
                self.ingest_subtype2_data_subframe(start_slot, &detection, &chips, format)
            } else {
                self.ingest_unsupported_subframe(start_slot, &detection)
            };
            for terminal in &outcome.terminal_packets {
                self.report_terminal_packet(*terminal);
            }
            if let Some(bus) = self.config.harq_bus.as_ref() {
                for decision in &outcome.arq {
                    bus.schedule_arq_at_slot(
                        self.config.mac_index,
                        decision.slot,
                        decision.h_or_l,
                        decision.p,
                    );
                }
            }
            if outcome.payload_bits != 0 {
                if outcome.turbo_iterations != 0 {
                    self.subframe_stats.turbo_attempts =
                        self.subframe_stats.turbo_attempts.saturating_add(1);
                    self.subframe_stats.turbo_iterations = self
                        .subframe_stats
                        .turbo_iterations
                        .saturating_add(u32::from(outcome.turbo_iterations));
                    self.subframe_stats.turbo_iterations_max = self
                        .subframe_stats
                        .turbo_iterations_max
                        .max(outcome.turbo_iterations);
                }
                log::log!(
                    if outcome.decoded {
                        log::Level::Trace
                    } else {
                        log::Level::Debug
                    },
                    "rx_hrpd_traffic[m{}]: subframe uati=0x{:08x} start_slot={} payload_bits={} subpacket={} decoded={} turbo_iterations={} rri_margin={:.2} rri_margin_norm={:.3} rri_best_norm={:.3} interlace={} accumulated={} llr_mean={:.3} mother_mean={:.3}",
                    self.config.mac_index,
                    self.config.uati,
                    start_slot,
                    outcome.payload_bits,
                    outcome.subpacket_id,
                    outcome.decoded,
                    outcome.turbo_iterations,
                    detection.margin,
                    observed_rri_margin_norm,
                    observed_rri_best_norm,
                    outcome.interlace,
                    outcome.subpackets_accumulated,
                    outcome.llr_mean_abs,
                    outcome.mother_mean_abs,
                );
            }
            if outcome.decoded {
                self.subframe_stats.decoded = self.subframe_stats.decoded.saturating_add(1);
            }
            if let Some(mac_packet) = outcome.delivered {
                match traffic_events_from_mac_packet_for_reverse_mac_subtype(
                    self.config.uati,
                    self.config.mac_index,
                    &mac_packet,
                    self.config.reverse_traffic_mac_subtype,
                ) {
                    Ok(mut events) => {
                        trace!(
                            "rx_hrpd_traffic[m{}]: subframe packet decoded uati=0x{:08x} start_slot={} payload_bits={} subpacket={} events={}",
                            self.config.mac_index,
                            self.config.uati,
                            start_slot,
                            outcome.payload_bits,
                            outcome.subpacket_id,
                            events.len(),
                        );
                        let air_frame_end_received_at = self
                            .air_received_at_chip(start_chip.saturating_add(subframe_chips as u64));
                        for event in &mut events {
                            if let HrpdTrafficEvent::Stream1Packet {
                                air_frame_end_received_at: event_air_time,
                                ..
                            } = event
                            {
                                *event_air_time = air_frame_end_received_at;
                            }
                        }
                        if let Some(tx) = self.config.event_tx.as_ref() {
                            for event in events {
                                let _ = tx.send(event);
                            }
                        }
                        self.subframe_stats.delivered =
                            self.subframe_stats.delivered.saturating_add(1);
                    }
                    Err(err) => {
                        self.subframe_stats.mac_parse_failed =
                            self.subframe_stats.mac_parse_failed.saturating_add(1);
                        debug!(
                            "rx_hrpd_traffic[m{}]: subframe packet MAC parse failed uati=0x{:08x} start_slot={}: {err:?}",
                            self.config.mac_index, self.config.uati, start_slot,
                        );
                    }
                }
            }
            self.maybe_report_subframe_stats(start_slot);
        }
    }

    fn maybe_report_subframe_stats(&mut self, start_slot: u64) {
        let window_start = *self
            .subframe_stats
            .window_start_slot
            .get_or_insert(start_slot);
        if start_slot.saturating_sub(window_start) < HRPD_SUBFRAME_SUMMARY_WINDOW_SLOTS {
            return;
        }
        let stats = std::mem::take(&mut self.subframe_stats);
        let (arq_on_time, arq_late, arq_tx_hits, arq_latest_tx_slot) =
            self.config.harq_bus.as_ref().map_or((0, 0, 0, 0), |bus| {
                bus.arq_schedule_stats(self.config.mac_index)
            });
        let turbo_iterations_avg = if stats.turbo_attempts == 0 {
            0.0
        } else {
            f64::from(stats.turbo_iterations) / f64::from(stats.turbo_attempts)
        };
        info!(
            "rx_hrpd_traffic[m{}]: subtype2_subframe_summary uati=0x{:08x} window_start_slot={} window_slots={} total={} low_coherence={} no_rri={} null_rri={} low_margin={} invalid={} non_null_rri={} decoded={} delivered={} mac_parse_failed={} turbo_attempts={} turbo_iterations_avg={:.2} turbo_iterations_max={} arq_on_time={} arq_late={} arq_tx_hits={} arq_latest_tx_slot={}",
            self.config.mac_index,
            self.config.uati,
            window_start,
            start_slot.saturating_sub(window_start),
            stats.total,
            stats.low_coherence,
            stats.no_rri,
            stats.null_rri,
            stats.low_margin,
            stats.invalid,
            stats.non_null_rri,
            stats.decoded,
            stats.delivered,
            stats.mac_parse_failed,
            stats.turbo_attempts,
            turbo_iterations_avg,
            stats.turbo_iterations_max,
            arq_on_time,
            arq_late,
            arq_tx_hits,
            arq_latest_tx_slot,
        );
        self.subframe_stats.window_start_slot = Some(start_slot);
    }

    /// Advance `frame_start_chip` to the next 16-slot frame boundary.
    fn advance_to_next_frame(&mut self) {
        self.next_params.frame_start_chip = self
            .next_params
            .frame_start_chip
            .saturating_add(HRPD_TRAFFIC_FRAME_CHIPS as u64);
    }

    fn snr_db_tenths(snr_db: f32) -> i16 {
        if snr_db.is_finite() {
            (snr_db * 10.0)
                .round()
                .clamp(i16::MIN as f32, i16::MAX as f32) as i16
        } else {
            i16::MIN
        }
    }

    fn coherence_x1000(coherence: f32) -> u16 {
        if coherence.is_finite() {
            (coherence * 1000.0).round().clamp(0.0, 1000.0) as u16
        } else {
            0
        }
    }

    fn remember_good_pilot(&mut self, moments: PilotMoments) {
        self.low_coherence_start_chip = None;
        self.last_good_pilot_chip = self.next_params.frame_start_chip;
        self.last_good_pilot_snr_db = moments.snr_db;
        self.last_good_pilot_coherence = moments.coherence;
    }

    fn maybe_emit_reverse_pilot(&mut self, moments: PilotMoments) {
        let frame_start_chip = self.next_params.frame_start_chip;
        let first_report = !self.reverse_pilot_event_sent;
        let periodic_report = self.last_reverse_pilot_event_chip.is_some_and(|last| {
            frame_start_chip.saturating_sub(last)
                >= HRPD_REVERSE_TRAFFIC_PILOT_REPORT_INTERVAL_CHIPS
        });
        if !first_report && !periodic_report {
            return;
        }

        if first_report && let Some(acquired) = &self.config.reverse_pilot_acquired {
            acquired.store(true, Ordering::Release);
        }

        let event = HrpdTrafficEvent::ReversePilot {
            uati: self.config.uati,
            mac_index: self.config.mac_index,
            absolute_chip: frame_start_chip,
            snr_db_tenths: Self::snr_db_tenths(moments.snr_db),
        };
        match &self.config.event_tx {
            Some(tx) => match tx.send(event) {
                Ok(()) if first_report => {
                    let acquisition_ms = self.config.worker_spawned_at.elapsed().as_millis();
                    info!(
                        "rx_hrpd_traffic[m{}]: validated reverse pilot uati=0x{:08x} frame_chip={} coh={:.3} snr={:.2}dB acquisition_ms={} (TRTCMPANSetup budget 3000ms); sent AN event",
                        self.config.mac_index,
                        self.config.uati,
                        frame_start_chip,
                        moments.coherence,
                        moments.snr_db,
                        acquisition_ms,
                    );
                }
                Ok(()) => {
                    trace!(
                        "rx_hrpd_traffic[m{}]: reverse pilot telemetry uati=0x{:08x} frame_chip={} coh={:.3} snr={:.2}dB; sent AN event",
                        self.config.mac_index,
                        self.config.uati,
                        frame_start_chip,
                        moments.coherence,
                        moments.snr_db,
                    );
                }
                Err(err) if first_report => warn!(
                    "rx_hrpd_traffic[m{}]: validated reverse pilot uati=0x{:08x} frame_chip={} but AN event send failed: {}",
                    self.config.mac_index, self.config.uati, frame_start_chip, err,
                ),
                Err(err) => warn!(
                    "rx_hrpd_traffic[m{}]: reverse pilot telemetry uati=0x{:08x} frame_chip={} but AN event send failed: {}",
                    self.config.mac_index, self.config.uati, frame_start_chip, err,
                ),
            },
            None if first_report => warn!(
                "rx_hrpd_traffic[m{}]: validated reverse pilot uati=0x{:08x} frame_chip={} but no AN event channel is installed",
                self.config.mac_index, self.config.uati, frame_start_chip,
            ),
            None => {}
        }
        self.reverse_pilot_event_sent = true;
        self.last_reverse_pilot_event_chip = Some(frame_start_chip);
    }

    fn normalize_sample_delay(total_samples: f32) -> (i32, f32) {
        let sample_delay = total_samples.floor() as i32;
        let mut sample_delay_fraction = total_samples - sample_delay as f32;
        if sample_delay_fraction.abs() < 1.0e-6 {
            sample_delay_fraction = 0.0;
        }
        (sample_delay, sample_delay_fraction)
    }

    #[allow(clippy::too_many_arguments)]
    fn score_timing_candidate(
        &self,
        samples: &[Complex32],
        absolute_sample_start: u64,
        oversample: usize,
        sample_delay: i32,
        sample_delay_fraction: f32,
    ) -> Option<TimingRefinement> {
        let frame_start_sample = self
            .next_params
            .frame_start_chip
            .checked_mul(oversample.max(1) as u64)?;
        if frame_start_sample < absolute_sample_start {
            return None;
        }
        let base_start = (frame_start_sample - absolute_sample_start) as usize;
        let mut coherent = Complex32::new(0.0, 0.0);
        let mut slot_coherent = [Complex32::new(0.0, 0.0); HRPD_TRAFFIC_SLOTS_PER_FRAME];
        let mut count = 0usize;

        for slot in 0..HRPD_TRAFFIC_SLOTS_PER_FRAME {
            let mut chip_idx = slot * HRPD_SLOT_CHIPS + HRPD_PILOT_CLEAN_START_CHIPS;
            let slot_end = (slot + 1) * HRPD_SLOT_CHIPS;
            while chip_idx + HRPD_TIMING_TRACK_CHIP_STEP <= slot_end {
                let mut symbol = Complex32::new(0.0, 0.0);
                for offset in 0..HRPD_TIMING_TRACK_CHIP_STEP {
                    let idx = chip_idx + offset;
                    let sample = sample_chip_at_delay(
                        samples,
                        base_start,
                        oversample,
                        idx,
                        sample_delay,
                        sample_delay_fraction,
                    )?;
                    symbol += sample * self.ref_conj[idx];
                }
                symbol /= HRPD_TIMING_TRACK_CHIP_STEP as f32;
                coherent += symbol;
                slot_coherent[slot] += symbol;
                count += 1;
                chip_idx += HRPD_TIMING_TRACK_CHIP_STEP;
            }
        }

        if count == 0 {
            return None;
        }

        let (phase_at_frame_start_rad, phase_step_rad_per_slot, _) =
            pilot_phase_ramp_from_slots(&slot_coherent, coherent);
        let mut slot_projected = [0.0f32; HRPD_TRAFFIC_SLOTS_PER_FRAME];
        let mut abs_sum = 0.0f32;
        let mut power_sum = 0.0f32;
        count = 0;

        for slot in 0..HRPD_TRAFFIC_SLOTS_PER_FRAME {
            let mut chip_idx = slot * HRPD_SLOT_CHIPS + HRPD_PILOT_CLEAN_START_CHIPS;
            let slot_end = (slot + 1) * HRPD_SLOT_CHIPS;
            while chip_idx + HRPD_TIMING_TRACK_CHIP_STEP <= slot_end {
                let mut symbol = Complex32::new(0.0, 0.0);
                for offset in 0..HRPD_TIMING_TRACK_CHIP_STEP {
                    let idx = chip_idx + offset;
                    let sample = sample_chip_at_delay(
                        samples,
                        base_start,
                        oversample,
                        idx,
                        sample_delay,
                        sample_delay_fraction,
                    )?;
                    symbol += sample * self.ref_conj[idx];
                }
                symbol /= HRPD_TIMING_TRACK_CHIP_STEP as f32;
                let phase = phase_at_frame_start_rad
                    + phase_step_rad_per_slot * chip_idx as f32 / HRPD_SLOT_CHIPS as f32;
                let (sin, cos) = (-phase).sin_cos();
                let projected = (symbol * Complex32::new(cos, sin)).re;
                slot_projected[slot] += projected;
                abs_sum += projected.abs();
                power_sum += projected * projected;
                count += 1;
                chip_idx += HRPD_TIMING_TRACK_CHIP_STEP;
            }
        }

        let noncoherent: f32 = slot_projected.iter().map(|s| s.abs()).sum();
        let coherence = (noncoherent / abs_sum.max(1.0e-12)).min(1.0);
        let mean_power = power_sum / count.max(1) as f32;
        let coherent_power = (noncoherent * noncoherent) / (count.max(1) * count.max(1)) as f32;
        let noise_power = (mean_power - coherent_power).max(1.0e-12);
        let snr_db = 10.0 * (coherent_power / noise_power).max(1.0e-12).log10();

        Some(TimingRefinement {
            sample_delay,
            sample_delay_fraction,
            coherence,
            snr_db,
        })
    }

    fn best_timing_candidate(
        &self,
        samples: &[Complex32],
        absolute_sample_start: u64,
        oversample: usize,
        radius_samples: f32,
    ) -> Option<TimingRefinement> {
        let current_total =
            self.next_params.sample_delay as f32 + self.next_params.sample_delay_fraction;
        let steps = (radius_samples / HRPD_TIMING_TRACK_STEP_SAMPLES).ceil() as i32;
        let mut best = None;
        for step in -steps..=steps {
            let total = current_total + step as f32 * HRPD_TIMING_TRACK_STEP_SAMPLES;
            let (sample_delay, sample_delay_fraction) = Self::normalize_sample_delay(total);
            let Some(candidate) = self.score_timing_candidate(
                samples,
                absolute_sample_start,
                oversample,
                sample_delay,
                sample_delay_fraction,
            ) else {
                continue;
            };
            if best.as_ref().is_none_or(|old: &TimingRefinement| {
                candidate.coherence > old.coherence
                    || ((candidate.coherence - old.coherence).abs() < 1.0e-3
                        && candidate.snr_db > old.snr_db)
            }) {
                best = Some(candidate);
            }
        }
        best
    }

    #[allow(clippy::too_many_arguments)]
    fn score_rpc_slot_timing_candidate(
        &self,
        samples: &[Complex32],
        absolute_sample_start: u64,
        oversample: usize,
        region_start_chip: u64,
        ref_slice: &[Complex32],
        sample_delay: i32,
        sample_delay_fraction: f32,
    ) -> Option<RpcSlotTimingCandidate> {
        let despread = despread_chips_with_reference(
            samples,
            absolute_sample_start,
            oversample,
            region_start_chip,
            sample_delay,
            sample_delay_fraction,
            Complex32::new(1.0, 0.0),
            ref_slice,
        )?;
        let moments = pilot_moments_from_slot(&despread);
        Some(RpcSlotTimingCandidate {
            sample_delay,
            sample_delay_fraction,
            moments,
        })
    }

    fn best_rpc_slot_timing_candidate(
        &self,
        samples: &[Complex32],
        absolute_sample_start: u64,
        oversample: usize,
        region_start_chip: u64,
        ref_slice: &[Complex32],
        radius_samples: f32,
    ) -> Option<RpcSlotTimingCandidate> {
        let current_total =
            self.next_params.sample_delay as f32 + self.next_params.sample_delay_fraction;
        let steps = (radius_samples / HRPD_TIMING_TRACK_STEP_SAMPLES).ceil() as i32;
        let mut best = None;
        for step in -steps..=steps {
            let total = current_total + step as f32 * HRPD_TIMING_TRACK_STEP_SAMPLES;
            let (sample_delay, sample_delay_fraction) = Self::normalize_sample_delay(total);
            let Some(candidate) = self.score_rpc_slot_timing_candidate(
                samples,
                absolute_sample_start,
                oversample,
                region_start_chip,
                ref_slice,
                sample_delay,
                sample_delay_fraction,
            ) else {
                continue;
            };
            if best.as_ref().is_none_or(|old: &RpcSlotTimingCandidate| {
                candidate.moments.coherence > old.moments.coherence
                    || ((candidate.moments.coherence - old.moments.coherence).abs() < 1.0e-3
                        && candidate.moments.snr_db > old.moments.snr_db)
            }) {
                best = Some(candidate);
            }
        }
        best
    }

    fn timing_refinement_radius(&mut self, current_coherence: f32) -> Option<f32> {
        if current_coherence < HRPD_REVERSE_TRAFFIC_PILOT_MIN_COHERENCE {
            self.frames_since_timing_refine = 0;
            self.frames_since_lost_timing_refine =
                self.frames_since_lost_timing_refine.saturating_add(1);
            if self.frames_since_lost_timing_refine >= HRPD_TIMING_TRACK_LOST_INTERVAL_FRAMES {
                self.frames_since_lost_timing_refine = 0;
                return Some(HRPD_TIMING_TRACK_LOST_RADIUS_SAMPLES);
            }
            return None;
        }

        self.frames_since_lost_timing_refine = HRPD_TIMING_TRACK_LOST_INTERVAL_FRAMES;
        self.frames_since_timing_refine = self.frames_since_timing_refine.saturating_add(1);
        if self.frames_since_timing_refine >= HRPD_TIMING_TRACK_INTERVAL_FRAMES {
            self.frames_since_timing_refine = 0;
            Some(HRPD_TIMING_TRACK_NORMAL_RADIUS_SAMPLES)
        } else {
            None
        }
    }

    fn update_timing_state(
        &mut self,
        measured_slot: u64,
        moments: PilotMoments,
        raw_power_dbfs: f32,
    ) -> bool {
        let pilot_reliable =
            moments.snr_db.is_finite() && moments.coherence >= HRPD_TIMING_REACQUIRE_MIN_COHERENCE;
        self.timing_last_pilot_reliable = pilot_reliable;

        if self.timing_state == HrpdTrafficTimingState::Tracking {
            if pilot_reliable {
                self.timing_unreliable_slots = 0;
            } else {
                self.timing_unreliable_slots = self
                    .timing_unreliable_slots
                    .saturating_add(1)
                    .min(HRPD_TIMING_REACQUIRE_UNRELIABLE_SLOTS);
            }
            if self.timing_unreliable_slots >= HRPD_TIMING_REACQUIRE_UNRELIABLE_SLOTS {
                info!(
                    "rx_hrpd_traffic[m{}]: timing_reacquiring uati=0x{:08x} slot={} delay={}+{:+.2} coherence={:.3} raw={:.1}dBFS raw_ref={:.1}dBFS",
                    self.config.mac_index,
                    self.config.uati,
                    measured_slot,
                    self.next_params.sample_delay,
                    self.next_params.sample_delay_fraction,
                    moments.coherence,
                    raw_power_dbfs,
                    self.rpc.quiet_reliable_raw_power_dbfs,
                );
                self.timing_state = HrpdTrafficTimingState::Reacquiring;
                self.timing_unreliable_slots = 0;
                self.timing_reacquire_confirm_slots = 0;
                self.timing_reacquire_search_wait_slots =
                    u32::from(HRPD_TIMING_REACQUIRE_UNRELIABLE_SLOTS);
                self.timing_reacquire_search_interval_slots =
                    HRPD_TIMING_REACQUIRE_SEARCH_INITIAL_SLOTS;
            }
        }

        if self.timing_state == HrpdTrafficTimingState::Reacquiring {
            if pilot_reliable {
                self.timing_reacquire_confirm_slots = self
                    .timing_reacquire_confirm_slots
                    .saturating_add(1)
                    .min(HRPD_TIMING_REACQUIRE_CONFIRM_SLOTS);
            } else {
                self.timing_reacquire_confirm_slots = 0;
            }
            if self.timing_reacquire_confirm_slots >= HRPD_TIMING_REACQUIRE_CONFIRM_SLOTS {
                self.timing_state = HrpdTrafficTimingState::Tracking;
                self.timing_reacquire_confirm_slots = 0;
                info!(
                    "rx_hrpd_traffic[m{}]: timing_reacquired uati=0x{:08x} slot={} delay={}+{:+.2} coherence={:.3} raw={:.1}dBFS raw_ref={:.1}dBFS",
                    self.config.mac_index,
                    self.config.uati,
                    measured_slot,
                    self.next_params.sample_delay,
                    self.next_params.sample_delay_fraction,
                    moments.coherence,
                    raw_power_dbfs,
                    self.rpc.quiet_reliable_raw_power_dbfs,
                );
                return false;
            }
        }

        if self.timing_state != HrpdTrafficTimingState::Reacquiring || pilot_reliable {
            return false;
        }
        self.timing_reacquire_search_wait_slots =
            self.timing_reacquire_search_wait_slots.saturating_add(1);
        if self.timing_reacquire_search_wait_slots < self.timing_reacquire_search_interval_slots {
            return false;
        }
        self.timing_reacquire_search_wait_slots = 0;
        self.timing_reacquire_search_interval_slots = self
            .timing_reacquire_search_interval_slots
            .saturating_mul(2)
            .min(HRPD_TIMING_REACQUIRE_SEARCH_MAX_SLOTS);
        true
    }

    fn maybe_emit_reverse_pilot_lost(&mut self, moments: PilotMoments) {
        if !self.reverse_pilot_event_sent || self.reverse_pilot_lost_event_sent {
            return;
        }
        let first_lost_chip = *self
            .low_coherence_start_chip
            .get_or_insert(self.next_params.frame_start_chip);
        let lost_at_chip = self
            .next_params
            .frame_start_chip
            .saturating_add(HRPD_TRAFFIC_FRAME_CHIPS as u64);
        let lost_chips = lost_at_chip.saturating_sub(first_lost_chip);
        if lost_chips < HRPD_REVERSE_TRAFFIC_PILOT_LOSS_TIMEOUT_CHIPS {
            return;
        }

        if let Some(acquired) = &self.config.reverse_pilot_acquired {
            acquired.store(false, Ordering::Release);
        }
        let event = HrpdTrafficEvent::ReversePilotLost {
            uati: self.config.uati,
            mac_index: self.config.mac_index,
            last_good_chip: self.last_good_pilot_chip,
            lost_at_chip,
            lost_chips,
            last_snr_db_tenths: Self::snr_db_tenths(self.last_good_pilot_snr_db),
            last_coherence_x1000: Self::coherence_x1000(self.last_good_pilot_coherence),
        };
        match &self.config.event_tx {
            Some(tx) => match tx.send(event) {
                Ok(()) => warn!(
                    "rx_hrpd_traffic[m{}]: reverse pilot lost uati=0x{:08x} last_good_chip={} lost_at_chip={} lost_ms={} last_coh={:.3} last_snr={:.2}dB current_coh={:.3} current_snr={:.2}dB rpc_unreliable_streak_slots={} rpc_unreliable_start_slot={:?} rpc_raw_ref={:.1}dBFS rpc_pred_delta={:+.2}dB rpc_slope={:+.3}dB/slot; sent AN event",
                    self.config.mac_index,
                    self.config.uati,
                    self.last_good_pilot_chip,
                    lost_at_chip,
                    lost_chips.saturating_mul(1000) / HRPD_CHIP_RATE_HZ,
                    self.last_good_pilot_coherence,
                    self.last_good_pilot_snr_db,
                    moments.coherence,
                    moments.snr_db,
                    self.rpc_unreliable_streak_slots,
                    self.rpc_unreliable_start_slot,
                    self.rpc.quiet_reliable_raw_power_dbfs,
                    self.rpc.last_prediction_delta_db,
                    self.rpc.last_slope_db_per_slot,
                ),
                Err(err) => warn!(
                    "rx_hrpd_traffic[m{}]: reverse pilot lost uati=0x{:08x} last_good_chip={} lost_at_chip={} but AN event send failed: {}",
                    self.config.mac_index,
                    self.config.uati,
                    self.last_good_pilot_chip,
                    lost_at_chip,
                    err,
                ),
            },
            None => warn!(
                "rx_hrpd_traffic[m{}]: reverse pilot lost uati=0x{:08x} last_good_chip={} lost_at_chip={} but no AN event channel is installed",
                self.config.mac_index, self.config.uati, self.last_good_pilot_chip, lost_at_chip,
            ),
        }
        self.reverse_pilot_lost_event_sent = true;
    }

    /// Build the output `SampleBlock` for one despread frame.
    fn build_output_block(
        &self,
        chips: Vec<Complex32>,
        coherence: f32,
        snr_db: f32,
    ) -> SampleBlock {
        let mut block = SampleBlock::new(chips, 0);
        block.sample_rate_hz = self.sample_rate_hz;
        block.rx_sample_time = self.latest_rx_sample_time;
        let tags = &mut block.tags;
        tags.insert(
            TAG_FRAME_START_CHIP,
            self.next_params.frame_start_chip as i64,
        );
        tags.insert(TAG_FRAME_OFFSET, i64::from(self.config.frame_offset));
        tags.insert(
            TAG_PILOT_COHERENCE_X1000,
            (coherence * 1000.0).round() as i64,
        );
        tags.insert(
            TAG_PILOT_SNR_DB_TENTHS,
            if snr_db.is_finite() {
                (snr_db * 10.0).round() as i64
            } else {
                i64::MIN
            },
        );
        tags.insert(TAG_UATI, self.config.uati as i64);
        tags.insert(TAG_MAC_INDEX, self.config.mac_index as i64);
        tags.insert(
            TAG_PHYSICAL_LAYER_SUBTYPE,
            i64::from(self.config.physical_layer_subtype),
        );
        tags.insert(
            TAG_REVERSE_TRAFFIC_MAC_SUBTYPE,
            i64::from(self.config.reverse_traffic_mac_subtype),
        );
        tags.insert(TAG_DRC_COVER, self.config.drc_cover as i64);
        tags.insert(TAG_DRC_LENGTH, self.config.drc_length as i64);
        tags.insert(TAG_Q_SIGN_X1000, (self.mask.q_sign * 1000.0).round() as i64);
        tags.insert(
            TAG_POWER_CONTROL_MOBILE_POWER_LIMITED,
            i64::from(self.rpc.mobile_power_limited()),
        );
        tags.insert(
            TAG_POWER_CONTROL_RECEIVER_REACQUIRING,
            i64::from(self.timing_state != HrpdTrafficTimingState::Tracking),
        );
        // Mark this block as carrying traffic PHY activity so BaseFinger's
        // post-walsh activity counters do not retire a healthy finger.
        tags.insert("traffic_phy_frame", 1);
        block
    }

    fn air_received_at_chip(&self, chip: u64) -> Option<std::time::Instant> {
        self.latest_rx_sample_time?.received_at_sample(
            chip.saturating_mul(self.config.oversample.max(1) as u64),
            self.sample_rate_hz,
        )
    }

    fn update_rpc_reliability_trace(
        &mut self,
        reliable: bool,
        measured_slot: u64,
        slot_chip: u64,
        moments: PilotMoments,
        raw_power_dbfs: f32,
        level_db: Option<f32>,
    ) {
        if reliable {
            if self.power_control_tune_away_active {
                if let Some(power_control) = &self.config.power_control {
                    power_control.resume_after_tune_away();
                }
                self.power_control_tune_away_active = false;
            }
            if self.rpc_unreliable_streak_slots > 0 && self.rpc_unreliable_reported {
                info!(
                    "rx_hrpd_traffic[m{}]: rpc_recovered uati=0x{:08x} measured_slot={} previous_start_slot={:?} previous_slots={} coh={:.3} projected_snr={:.2}dB pilot_sinr={:.2}dB fitted={:.2}dB pred={:.2}dB pred_delta={:+.2}dB slope={:+.3}dB/slot residual={:+.2}dB raw={:.1}dBFS raw_ref={:.1}dBFS",
                    self.config.mac_index,
                    self.config.uati,
                    measured_slot,
                    self.rpc_unreliable_start_slot,
                    self.rpc_unreliable_streak_slots,
                    moments.coherence,
                    moments.snr_db,
                    moments.rc3_sinr_db,
                    self.rpc.last_level_db,
                    level_db.unwrap_or(f32::NAN),
                    self.rpc.last_prediction_delta_db,
                    self.rpc.last_slope_db_per_slot,
                    self.rpc.power_residual_db,
                    raw_power_dbfs,
                    self.rpc.quiet_reliable_raw_power_dbfs,
                );
            }
            self.rpc_unreliable_streak_slots = 0;
            self.rpc_unreliable_start_slot = None;
            self.rpc_unreliable_reported = false;
            return;
        }

        if self.rpc_unreliable_streak_slots == 0 {
            self.rpc_unreliable_start_slot = Some(measured_slot);
        }
        self.rpc_unreliable_streak_slots = self.rpc_unreliable_streak_slots.saturating_add(1);
        if !self.power_control_tune_away_active
            && self.rpc_unreliable_streak_slots >= HRPD_POWER_CONTROL_TUNE_AWAY_MIN_SLOTS
            && let Some(power_control) = &self.config.power_control
            && power_control.suspend_for_tune_away()
        {
            self.power_control_tune_away_active = true;
        }
        if self.rpc_unreliable_reported
            || self.rpc_unreliable_streak_slots < HRPD_RPC_UNRELIABLE_REPORT_MIN_SLOTS
        {
            return;
        }

        let last_good_age_ms = slot_chip
            .saturating_sub(self.last_good_pilot_chip)
            .saturating_mul(1000)
            / HRPD_CHIP_RATE_HZ;
        warn!(
            "rx_hrpd_traffic[m{}]: rpc_unreliable_streak uati=0x{:08x} start_slot={:?} measured_slot={} slots={} last_good_age_ms={} coh={:.3} projected_snr={:.2}dB pilot_sinr={:.2}dB amp_step={:.2}dB raw={:.1}dBFS raw_ref={:.1}dBFS latest={:.2}dB fitted={:.2}dB pred_delta={:+.2}dB slope={:+.3}dB/slot residual={:+.2}dB",
            self.config.mac_index,
            self.config.uati,
            self.rpc_unreliable_start_slot,
            measured_slot,
            self.rpc_unreliable_streak_slots,
            last_good_age_ms,
            moments.coherence,
            moments.snr_db,
            moments.rc3_sinr_db,
            moments.pilot_amplitude_step_db,
            raw_power_dbfs,
            self.rpc.quiet_reliable_raw_power_dbfs,
            self.rpc.last_measured_level_db,
            self.rpc.last_level_db,
            self.rpc.last_prediction_delta_db,
            self.rpc.last_slope_db_per_slot,
            self.rpc.power_residual_db,
        );
        self.rpc_unreliable_reported = true;
    }

    /// Run the per-slot reverse power-control loop over every buffered slot that
    /// is now fully available. Each slot's W0 pilot region is despread on its own
    /// — independent of the frame-rate data despread — so the RPC bit reflects a
    /// slot-fresh SINR. The controller predicts across the RX→TX slot delay and
    /// converts the result into one bit scheduled
    /// `HRPD_RPC_TX_LEAD_SLOTS` ahead. This runs ahead of the frame data cursor
    /// and only reads the shared IQ buffer; the frame loop owns draining it.
    fn process_pending_rpc_slots(&mut self) {
        let Some(bus) = self.config.harq_bus.clone() else {
            return;
        };
        let oversample = self.config.oversample.max(1) as u64;
        let pilot_region_start = if self.config.physical_layer_subtype >= 2 {
            0
        } else {
            HRPD_PILOT_CLEAN_START_CHIPS
        };
        let pilot_region_len = HRPD_SLOT_CHIPS - pilot_region_start;
        loop {
            let Some(buf_abs) = self.buffer_abs_sample else {
                return;
            };
            let slot_chip = self.next_rpc_slot_chip;
            let region_start_chip = slot_chip + pilot_region_start as u64;
            let region_start_sample = (region_start_chip * oversample) as i64;
            let sample_delay = self.next_params.sample_delay;
            let sample_delay_fraction = self.next_params.sample_delay_fraction;
            // Absolute sample window the chip interpolator touches for this
            // region (sample_delay may be negative; +2 covers the `lo+1` read).
            let earliest = region_start_sample + sample_delay as i64;
            let latest = region_start_sample
                + (pilot_region_len as i64 - 1) * oversample as i64
                + sample_delay as i64
                + 2;
            if earliest < buf_abs as i64 {
                // Samples for this slot were already drained by the frame loop;
                // skip ahead (converges to the first slot still buffered).
                self.next_rpc_slot_chip = slot_chip + HRPD_SLOT_CHIPS as u64;
                continue;
            }
            if latest >= (buf_abs + self.buffer.len() as u64) as i64 {
                // Not enough IQ buffered for this slot yet; wait for more.
                return;
            }
            // Despread this slot's pilot region with identity phase: the metric
            // estimator fits and removes the per-slot residual phase itself, so
            // the RPC measurement is decoupled from the data path's phase state.
            // `ref_conj` is one PN/LC period anchored at the spawn frame start
            // (`spawn_chip`), reused every frame. Index it by this region's
            // offset from that anchor, not by the absolute slot number, which
            // aligns to the reference only when the acquisition happened to land
            // on a frame-period boundary.
            let ref_base =
                ((region_start_chip - self.spawn_chip) % HRPD_TRAFFIC_FRAME_CHIPS as u64) as usize;
            let ref_range = ref_base..ref_base + pilot_region_len;
            let Some(despread) = despread_chips_with_reference(
                &self.buffer,
                buf_abs,
                oversample as usize,
                region_start_chip,
                sample_delay,
                sample_delay_fraction,
                Complex32::new(1.0, 0.0),
                &self.ref_conj[ref_range.clone()],
            ) else {
                return;
            };
            let mut moments = pilot_moments_from_slot(&despread);
            let region_base = (region_start_sample - buf_abs as i64).max(0) as usize;
            let region_samples = (pilot_region_len * oversample as usize)
                .min(self.buffer.len().saturating_sub(region_base));
            let raw_power_dbfs = if region_samples > 0 {
                let power = self.buffer[region_base..region_base + region_samples]
                    .iter()
                    .map(|s| s.norm_sqr() as f64)
                    .sum::<f64>()
                    / region_samples as f64;
                10.0 * (power.max(1.0e-12).log10() as f32)
            } else {
                f32::NAN
            };
            let measured_slot = slot_chip / HRPD_SLOT_CHIPS as u64;
            self.rpc.set_target_db(
                self.config
                    .power_control
                    .as_ref()
                    .map_or(HRPD_RPC_TARGET_SNR_DB, HrpdPowerControlHandle::target_db),
            );
            self.rpc.observe_raw_power(raw_power_dbfs);
            let reacquire_search_due =
                self.update_timing_state(measured_slot, moments, raw_power_dbfs);
            if reacquire_search_due {
                let ref_slice = &self.ref_conj[ref_range.clone()];
                let best = self.best_rpc_slot_timing_candidate(
                    &self.buffer,
                    buf_abs,
                    oversample as usize,
                    region_start_chip,
                    ref_slice,
                    HRPD_TIMING_TRACK_LOST_RADIUS_SAMPLES,
                );
                if let Some(best) = best {
                    let current_delay = self.next_params.sample_delay;
                    let current_fraction = self.next_params.sample_delay_fraction;
                    let delay_changed = current_delay != best.sample_delay
                        || (current_fraction - best.sample_delay_fraction).abs() > 1.0e-3;
                    let recovered_lock = best.moments.coherence
                        >= HRPD_TIMING_REACQUIRE_MIN_COHERENCE
                        && best.moments.snr_db >= HRPD_TIMING_TRACK_RELOCK_MIN_SNR_DB
                        && best.moments.coherence
                            >= moments.coherence + HRPD_TIMING_TRACK_MIN_IMPROVEMENT;

                    if delay_changed && recovered_lock {
                        self.next_params.sample_delay = best.sample_delay;
                        self.next_params.sample_delay_fraction = best.sample_delay_fraction;
                        self.frames_since_timing_refine = 0;
                        self.timing_reacquire_confirm_slots = 1;
                        self.timing_last_pilot_reliable = true;
                        if self.rpc_timing_refine_reports < HRPD_RPC_TIMING_TRACK_REPORTS_MAX {
                            self.rpc_timing_refine_reports += 1;
                            debug!(
                                "rx_hrpd_traffic[m{}]: rpc_timing_refined uati=0x{:08x} measured_slot={} delay={}+{:+.2}->{}+{:+.2} coh={:.3}->{:.3} snr={:.2}->{:.2}dB radius={:.1}",
                                self.config.mac_index,
                                self.config.uati,
                                slot_chip / HRPD_SLOT_CHIPS as u64,
                                current_delay,
                                current_fraction,
                                best.sample_delay,
                                best.sample_delay_fraction,
                                moments.coherence,
                                best.moments.coherence,
                                moments.snr_db,
                                best.moments.snr_db,
                                HRPD_TIMING_TRACK_LOST_RADIUS_SAMPLES,
                            );
                        }
                        moments = best.moments;
                    } else if self.rpc_timing_refine_reports < HRPD_RPC_TIMING_TRACK_REPORTS_MAX {
                        self.rpc_timing_refine_reports += 1;
                        debug!(
                            "rx_hrpd_traffic[m{}]: rpc_timing_search_no_relock uati=0x{:08x} measured_slot={} current_delay={}+{:+.2} current_coh={:.3} current_snr={:.2}dB best_delay={}+{:+.2} best_coh={:.3} best_snr={:.2}dB radius={:.1}",
                            self.config.mac_index,
                            self.config.uati,
                            slot_chip / HRPD_SLOT_CHIPS as u64,
                            current_delay,
                            current_fraction,
                            moments.coherence,
                            moments.snr_db,
                            best.sample_delay,
                            best.sample_delay_fraction,
                            best.moments.coherence,
                            best.moments.snr_db,
                            HRPD_TIMING_TRACK_LOST_RADIUS_SAMPLES,
                        );
                    }
                }
            }
            let mobile_power_dbfs = mobile_pilot_power_dbfs(moments);
            self.rpc.observe_mobile_power(mobile_power_dbfs);
            let level = self.rpc.ingest_with_amplitude_step(
                moments.rc3_sinr_db,
                moments.coherence,
                moments.pilot_amplitude_step_db,
            );
            if level.is_some() {
                self.rpc.observe_reliable_raw_power(raw_power_dbfs);
                self.rpc.observe_reliable_mobile_power(mobile_power_dbfs);
            }

            let scheduled_slot = measured_slot + HRPD_RPC_TX_LEAD_SLOTS;
            self.update_rpc_reliability_trace(
                level.is_some(),
                measured_slot,
                slot_chip,
                moments,
                raw_power_dbfs,
                level,
            );
            // Default/subtype-0 DRCLock punctures carry no RPC bit. Rev A
            // subtype 2/3 control slots carry RPC and DRCLock together; the
            // negotiated PHY-specific rule is centralized in `mac_rpc_slot`.
            let mut scheduled_rpc_bit = None;
            if mac_rpc_slot(
                scheduled_slot,
                self.config.frame_offset,
                self.config.physical_layer_subtype,
            ) {
                let control_mobile_power_dbfs =
                    self.rpc.control_mobile_power(level, mobile_power_dbfs);
                let control_level = self.rpc.control_level(level);
                let bit = self
                    .rpc
                    .emit_with_mobile_power(control_level, control_mobile_power_dbfs);
                bus.schedule_rpc_at_slot(self.config.mac_index, scheduled_slot, bit);
                scheduled_rpc_bit = Some((bit, control_mobile_power_dbfs));
            }

            self.rpc.record_log_metric(moments, raw_power_dbfs);
            self.rpc.slots_since_log = self.rpc.slots_since_log.saturating_add(1);
            if let Some((bit, control_mobile_power_dbfs)) =
                scheduled_rpc_bit.filter(|_| hrpd_rpc_control_verbose())
            {
                debug!(
                    "rx_hrpd_traffic[m{}]: rpc_control uati=0x{:08x} measured_slot={} scheduled_slot={} rpc_bit={} target={:.1}dB metric_coh={:.3} amp_step={:.2}dB projected_snr={:.2}dB metric_sinr={:.2}dB latest_sinr={:.2}dB fitted={:.2}dB pred={:.2}dB pred_delta={:+.2}dB slope={:+.3}dB/slot residual={:+.2}dB brake={:.2}dB limit_cd={} mobile_lim={} delay={}+{:+.2} raw={:.1}dBFS mobile={:.1}dBFS control_mobile={:.1}dBFS mobile_ref={:.1}dBFS up_bits={} down_bits={} metric_holds={} raw_hot={} mobile_limit_downs={} unreliable={}",
                    self.config.mac_index,
                    self.config.uati,
                    measured_slot,
                    scheduled_slot,
                    bit,
                    self.rpc.target_db,
                    moments.coherence,
                    moments.pilot_amplitude_step_db,
                    moments.snr_db,
                    moments.rc3_sinr_db,
                    self.rpc.last_measured_level_db,
                    self.rpc.last_level_db,
                    level
                        .or(self.rpc.last_predicted_level_db)
                        .unwrap_or(f32::NAN),
                    self.rpc.last_prediction_delta_db,
                    self.rpc.last_slope_db_per_slot,
                    self.rpc.power_residual_db,
                    self.rpc.last_brake_offset_db,
                    self.rpc.mobile_limit_cooldown_slots,
                    self.rpc.mobile_hard_limiter_active as u8,
                    self.next_params.sample_delay,
                    self.next_params.sample_delay_fraction,
                    raw_power_dbfs,
                    mobile_power_dbfs,
                    control_mobile_power_dbfs,
                    self.rpc.last_reliable_mobile_power_dbfs,
                    self.rpc.up_bits,
                    self.rpc.down_bits,
                    self.rpc.metric_holds,
                    self.rpc.raw_hot_slots,
                    self.rpc.mobile_limit_downs,
                    self.rpc.unreliable,
                );
            }
            if self.rpc.slots_since_log >= HRPD_RPC_SUMMARY_INTERVAL_SLOTS {
                let (tx_sched_hits, tx_sched_misses) = self
                    .config
                    .harq_bus
                    .as_ref()
                    .map_or((0, 0), |bus| bus.rpc_lookup_stats(self.config.mac_index));
                debug!(
                    "rx_hrpd_traffic[m{}]: rpc_summary uati=0x{:08x} slots={} target_pilot_sinr={:.1}dB avg_projected_snr={:.2}dB min_projected_snr={:.2}dB max_projected_snr={:.2}dB avg_coh={:.3} min_coh={:.3} max_coh={:.3} clean_n={} clean_projected_snr={:.2}dB clean_pilot_sinr={:.2}dB clean_coh={:.3} clean_raw={:.1}dBFS clean_pilot_power={:.1}dBFS clean_corr_snr_raw={:.3} clean_corr_pilot_sinr_raw={:.3} clean_corr_pilot_sinr_power={:.3} latest_sinr={:.2}dB fitted={:.2}dB pred={:.2}dB pred_delta={:+.2}dB slope={:+.3}dB/slot residual={:+.2}dB brake={:.2}dB filt_raw={:.1}dBFS filt_mobile={:.1}dBFS delay={}+{:+.2} raw={:.1}dBFS mobile={:.1}dBFS mobile_ref={:.1}dBFS raw_ref={:.1}dBFS up_bits={} down_bits={} metric_holds={} raw_hot={} mobile_limit_downs={} unreliable={} envelope_rejects={} reused_controls={} metric_age={} tx_sched_hits={} tx_sched_misses={}",
                    self.config.mac_index,
                    self.config.uati,
                    self.rpc.slots_since_log,
                    self.rpc.target_db,
                    self.rpc.avg_snr_db(),
                    self.rpc.metric_snr_min_db,
                    self.rpc.metric_snr_max_db,
                    self.rpc.avg_coherence(),
                    self.rpc.metric_coherence_min,
                    self.rpc.metric_coherence_max,
                    self.rpc.clean_projected_raw.n,
                    self.rpc.clean_projected_raw.mean_x(),
                    self.rpc.clean_rc3_raw.mean_x(),
                    (self.rpc.clean_coherence_sum
                        / f64::from(self.rpc.clean_projected_raw.n.max(1)))
                        as f32,
                    self.rpc.clean_projected_raw.mean_y(),
                    self.rpc.clean_rc3_pilot_power.mean_y(),
                    self.rpc.clean_projected_raw.correlation(),
                    self.rpc.clean_rc3_raw.correlation(),
                    self.rpc.clean_rc3_pilot_power.correlation(),
                    self.rpc.last_measured_level_db,
                    self.rpc.last_level_db,
                    level
                        .or(self.rpc.last_predicted_level_db)
                        .unwrap_or(f32::NAN),
                    self.rpc.last_prediction_delta_db,
                    self.rpc.last_slope_db_per_slot,
                    self.rpc.power_residual_db,
                    self.rpc.last_brake_offset_db,
                    self.rpc.filtered_raw_power_dbfs.unwrap_or(f32::NAN),
                    self.rpc
                        .brake_filtered_mobile_power_dbfs
                        .unwrap_or(f32::NAN),
                    self.next_params.sample_delay,
                    self.next_params.sample_delay_fraction,
                    raw_power_dbfs,
                    mobile_power_dbfs,
                    self.rpc.last_reliable_mobile_power_dbfs,
                    self.rpc.quiet_reliable_raw_power_dbfs,
                    self.rpc.up_bits,
                    self.rpc.down_bits,
                    self.rpc.metric_holds,
                    self.rpc.raw_hot_slots,
                    self.rpc.mobile_limit_downs,
                    self.rpc.unreliable,
                    self.rpc.envelope_rejects,
                    self.rpc.reused_metric_controls,
                    self.rpc.last_reliable_metric_age_slots,
                    tx_sched_hits,
                    tx_sched_misses,
                );
                self.rpc.reset_log_window();
            }

            self.next_rpc_slot_chip = slot_chip + HRPD_SLOT_CHIPS as u64;
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum FastDrcSource {
    Repetition,
    Window,
}

#[derive(Debug, Clone, Copy)]
enum FastDrcReject {
    Invalid,
    LowConfidence,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum B4DataLane {
    QRaw,
    QInverted,
}

// After pilot-ramp derotation pins the I axis, the only physically possible
// residual ambiguity on a spec waveform is receive-side conjugation (the
// acquired `q_sign` is pilot-blind), so the identity and conjugate lanes are
// the complete set. If data ever fails to lock on both, suspect the despread
// quadrature reference, not the constellation orientation.
const SUBTYPE2_DATA_TRANSFORM_COUNT: usize = 2;
const SUBTYPE2_DATA_TRANSFORMS: [Subtype2DataTransform; SUBTYPE2_DATA_TRANSFORM_COUNT] =
    [Subtype2DataTransform::Raw, Subtype2DataTransform::Conjugate];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(usize)]
enum Subtype2DataTransform {
    Raw = 0,
    Conjugate = 1,
}

impl Subtype2DataTransform {
    fn index(self) -> usize {
        self as usize
    }

    fn name(self) -> &'static str {
        match self {
            Self::Raw => "raw",
            Self::Conjugate => "conj",
        }
    }

    fn q_inversion(self) -> Option<bool> {
        match self {
            Self::Raw => Some(false),
            Self::Conjugate => Some(true),
        }
    }

    fn apply(self, sample: Complex32) -> Complex32 {
        match self {
            Self::Raw => sample,
            Self::Conjugate => Complex32::new(sample.re, -sample.im),
        }
    }
}

fn implemented_forward_traffic_payload_bits_for_drc_in_subtype(
    drc_index: u8,
    physical_layer_subtype: u16,
) -> Option<usize> {
    if physical_layer_subtype < 2 && drc_index >= 0x0d {
        return None;
    }
    implemented_forward_traffic_payload_bits_for_drc(drc_index)
}

#[inline]
fn normalize_drc_polarity(value: u8, q_sign: f32) -> u8 {
    if q_sign < 0.0 { value ^ 1 } else { value }
}

#[inline]
/// First chip at or after `start_chip` whose slot satisfies
/// (T − FrameOffset) mod 4 = 0 — the only slots a subtype-3 reverse
/// sub-packet may start in.
fn first_aligned_subframe_chip(start_chip: u64, frame_offset: u8) -> u64 {
    let slot = start_chip.div_ceil(HRPD_SLOT_CHIPS as u64);
    let phase = (slot + 4 - u64::from(frame_offset & 0x03)) % 4;
    let aligned_slot = if phase == 0 { slot } else { slot + (4 - phase) };
    aligned_slot * HRPD_SLOT_CHIPS as u64
}

pub(super) fn is_subframe_rri_margin_low(
    detection: &RriSubtype2Detection,
    margin_norm: f32,
) -> bool {
    if margin_norm.is_finite() {
        margin_norm < HRPD_SUBFRAME_RRI_MIN_MARGIN_NORM
    } else {
        detection.margin < HRPD_SUBFRAME_RRI_MIN_MARGIN
    }
}

/// Derotates the frame by the pilot phase ramp using a phasor recurrence, so no
/// per-chip trigonometry is needed.
pub(super) fn derotate_frame_by_pilot_ramp(chips: &mut [Complex32], moments: PilotMoments) {
    let step = -(moments.phase_step_rad_per_slot as f64) / HRPD_SLOT_CHIPS as f64;
    let mut nco =
        crate::sdr::PhasorNco::with_start_phase(-(moments.phase_at_frame_start_rad as f64), step);
    nco.rotate_in_place(chips);
}

fn rri_expected_best_from_pilot(chips: &[Complex32], slots: usize) -> f32 {
    let max_chips = chips.len().min(slots.saturating_mul(HRPD_SLOT_CHIPS));
    let mut total = 0.0f32;
    let mut symbols = 0usize;
    for slot in 0..slots {
        let mut chip_idx = slot * HRPD_SLOT_CHIPS;
        let slot_end = ((slot + 1) * HRPD_SLOT_CHIPS).min(max_chips);
        while chip_idx + 16 <= slot_end {
            let symbol = chips[chip_idx..chip_idx + 16]
                .iter()
                .map(|chip| chip.re)
                .sum::<f32>()
                / 16.0;
            total += symbol.abs();
            symbols += 1;
            chip_idx += 16;
        }
    }
    if symbols == 0 {
        f32::NAN
    } else {
        (total / symbols as f32) * RRI_SUBTYPE2_SUBFRAME_SYMBOLS as f32
    }
}

fn normalized_rri_score(score: f32, expected_best: f32) -> f32 {
    if expected_best.is_finite() && expected_best.abs() > 1.0e-9 {
        score / expected_best
    } else {
        f32::NAN
    }
}

fn decover_subframe_data(
    chips: &[Complex32],
    format: &Subtype2DataFormat,
    transform: Subtype2DataTransform,
) -> (Vec<Complex32>, Vec<Complex32>) {
    let transformed;
    let data_chips = if transform == Subtype2DataTransform::Raw {
        chips
    } else {
        transformed = chips
            .iter()
            .map(|sample| transform.apply(*sample))
            .collect::<Vec<_>>();
        transformed.as_slice()
    };
    (
        if format.subframe_w24_symbols() > 0 {
            decover_w24_symbols(data_chips)
        } else {
            Vec::new()
        },
        if format.subframe_w12_symbols() > 0 {
            decover_w12_symbols(data_chips)
        } else {
            Vec::new()
        },
    )
}

fn b4_q_branch_mean_abs_llr(w24: &[Complex32]) -> f32 {
    if w24.is_empty() {
        return 0.0;
    }
    w24.iter().map(|symbol| symbol.im.abs()).sum::<f32>() / w24.len() as f32
}

impl RakeFinger for HrpdReverseTrafficFinger {
    fn id(&self) -> u64 {
        self.base.id
    }

    fn spawn_chip_start(&self) -> Option<u64> {
        Some(self.spawn_chip)
    }

    fn describe(&self) -> String {
        format!(
            "uati=0x{:08x} mac={} frame_chip={} delay={}+{:+.2} q_sign={:+.0} q_pair_phase={}",
            self.config.uati,
            self.config.mac_index,
            self.next_params.frame_start_chip,
            self.next_params.sample_delay,
            self.next_params.sample_delay_fraction,
            self.mask.q_sign,
            self.mask.q_pair_phase,
        )
    }

    fn process(
        &mut self,
        block: &SampleBlock,
        chain: &mut Vec<PipelineProcessorShared>,
    ) -> Vec<SampleBlock> {
        self.sample_rate_hz = block.sample_rate_hz;
        if block.rx_sample_time.is_some() {
            self.latest_rx_sample_time = block.rx_sample_time;
        }
        let oversample = self.config.oversample.max(1);
        let block_abs_sample = block
            .tags
            .get("absolute_sample_start")
            .and_then(|value| u64::try_from(*value).ok())
            .unwrap_or_else(|| block.chip_start as u64 * oversample as u64);
        // Stitch onto an existing buffer if the block lines up; otherwise
        // restart from this block.
        if let Some(abs) = self.buffer_abs_sample {
            let expected = abs + self.buffer.len() as u64;
            if expected != block_abs_sample {
                debug!(
                    "hrpd_rev_traffic_finger {} sample discontinuity expected={} got={}, resetting",
                    self.base.id, expected, block_abs_sample
                );
                self.buffer.clear();
                self.buffer_abs_sample = None;
            }
        }
        self.buffer_abs_sample.get_or_insert(block_abs_sample);
        self.buffer.extend_from_slice(&block.samples);

        // DRC has the tightest forward-link deadline: a completed DRC only
        // governs packet starts in the next DRCLength slots.
        self.process_pending_drc_repetitions();
        self.process_pending_drc_windows();
        // RPC is also slot-rate, but it is scheduled with TX lead time.
        self.process_pending_rpc_slots();
        // Subtype-3 reverse data decodes per 4-slot sub-frame so H-ARQ
        // responses meet their m..m+2 forward deadline.
        self.process_pending_subframes();

        let mut out = Vec::new();
        let frame_samples = HRPD_TRAFFIC_FRAME_CHIPS * oversample;
        // The sample-delay interpolator reaches `sample_delay` (plus the
        // fractional part) outside the frame on both sides. A negative spawn
        // delay reads samples *before* the frame start, so the buffer must
        // retain that margin or every frame despreads to None.
        let delay_with_frac =
            self.next_params.sample_delay as f32 + self.next_params.sample_delay_fraction;
        let timing_margin = HRPD_TIMING_TRACK_LOST_RADIUS_SAMPLES.ceil() as u64 + 2;
        let back_margin = (-delay_with_frac).ceil().max(0.0) as u64;
        let retention_back_margin = back_margin + timing_margin;
        let forward_margin =
            (delay_with_frac.ceil().max(0.0) as usize) + timing_margin as usize + 2;
        loop {
            let abs_sample = match self.buffer_abs_sample {
                Some(v) => v,
                None => break,
            };
            let frame_start_sample = self
                .next_params
                .frame_start_chip
                .saturating_mul(oversample as u64);
            if frame_start_sample < abs_sample + back_margin {
                self.advance_to_next_frame();
                continue;
            }
            // Drop only samples that are already before the next frame's
            // interpolator back margin. The frame/RPC cursors own timeline
            // advancement; backlog is preserved rather than skipped.
            let frame_needed_start = frame_start_sample.saturating_sub(retention_back_margin);
            // DRC starts halfway through a slot, so a repetition or full
            // window can straddle this frame boundary. Preserve its first
            // half until the following input block completes the decode.
            let needed_start = self
                .pending_fast_drc_retention_sample(oversample as u64, retention_back_margin)
                .map_or(frame_needed_start, |drc_start| {
                    frame_needed_start.min(drc_start)
                });
            if needed_start > abs_sample {
                let to_drop = (needed_start - abs_sample) as usize;
                if to_drop >= self.buffer.len() {
                    let old_len = self.buffer.len() as u64;
                    self.buffer.clear();
                    self.buffer_abs_sample = Some(abs_sample + old_len);
                    break;
                }
                self.buffer.drain(..to_drop);
                self.buffer_abs_sample = Some(needed_start);
                continue;
            }
            let head_offset = (frame_start_sample - abs_sample) as usize;
            // Need a full frame of samples after the frame start plus the
            // interpolator's forward reach.
            if self.buffer.len() < head_offset + frame_samples + forward_margin {
                break;
            }
            let abs_sample = self.buffer_abs_sample.unwrap();
            let mut despread = match despread_frame_with_reference(
                &self.buffer,
                abs_sample,
                oversample,
                &self.next_params,
                &self.ref_conj,
            ) {
                Some(chips) => chips,
                None => {
                    // Should not happen given the size check above, but bail
                    // safely if it does.
                    self.advance_to_next_frame();
                    continue;
                }
            };

            // Per-frame pilot phase re-estimate. Simpler than an IIR and the
            // historical worker uses the same per-frame approach; if a future
            // diff finds value in a one-pole smoother, this is the place.
            let mut moments = pilot_moments_from_despread(&despread);
            if self.timing_state == HrpdTrafficTimingState::Tracking
                && self.timing_last_pilot_reliable
                && let Some(radius_samples) = self.timing_refinement_radius(moments.coherence)
            {
                let best = self.best_timing_candidate(
                    &self.buffer,
                    abs_sample,
                    oversample,
                    radius_samples,
                );
                if let Some(best) = best {
                    let current_delay = self.next_params.sample_delay;
                    let current_fraction = self.next_params.sample_delay_fraction;
                    let delay_changed = current_delay != best.sample_delay
                        || (current_fraction - best.sample_delay_fraction).abs() > 1.0e-3;
                    let current_locked =
                        moments.coherence >= HRPD_REVERSE_TRAFFIC_PILOT_MIN_COHERENCE;
                    let best_locked = best.coherence >= HRPD_REVERSE_TRAFFIC_PILOT_MIN_COHERENCE;
                    let strong_improvement =
                        best.coherence >= moments.coherence + HRPD_TIMING_TRACK_MIN_IMPROVEMENT;
                    let recovered_lock = !current_locked
                        && best.coherence >= HRPD_TIMING_TRACK_RELOCK_MIN_COHERENCE
                        && best.snr_db >= HRPD_TIMING_TRACK_RELOCK_MIN_SNR_DB;
                    let apply_candidate = if current_locked {
                        best_locked && strong_improvement
                    } else {
                        recovered_lock
                    };

                    if delay_changed && apply_candidate {
                        self.next_params.sample_delay = best.sample_delay;
                        self.next_params.sample_delay_fraction = best.sample_delay_fraction;
                        if let Some(redespread) = despread_frame_with_reference(
                            &self.buffer,
                            abs_sample,
                            oversample,
                            &self.next_params,
                            &self.ref_conj,
                        ) {
                            let refined_moments = pilot_moments_from_despread(&redespread);
                            let refined_locked = refined_moments.coherence
                                >= HRPD_REVERSE_TRAFFIC_PILOT_MIN_COHERENCE;
                            let refined_safe = if current_locked {
                                refined_locked
                                    && refined_moments.coherence + 0.02 >= moments.coherence
                            } else {
                                refined_locked
                            };
                            if refined_safe {
                                if self.timing_refine_reports < HRPD_TIMING_TRACK_REPORTS_MAX {
                                    self.timing_refine_reports += 1;
                                    info!(
                                        "rx_hrpd_traffic[m{}]: timing_refined uati=0x{:08x} frame_chip={} delay={}+{:+.2}->{}+{:+.2} coh={:.3}->{:.3} snr={:.2}->{:.2}dB radius={:.1}",
                                        self.config.mac_index,
                                        self.config.uati,
                                        self.next_params.frame_start_chip,
                                        current_delay,
                                        current_fraction,
                                        best.sample_delay,
                                        best.sample_delay_fraction,
                                        moments.coherence,
                                        refined_moments.coherence,
                                        moments.snr_db,
                                        refined_moments.snr_db,
                                        radius_samples,
                                    );
                                }
                                despread = redespread;
                                moments = refined_moments;
                            } else {
                                self.next_params.sample_delay = current_delay;
                                self.next_params.sample_delay_fraction = current_fraction;
                            }
                        } else {
                            self.next_params.sample_delay = current_delay;
                            self.next_params.sample_delay_fraction = current_fraction;
                        }
                    } else if !current_locked
                        && self.timing_refine_reports < HRPD_TIMING_TRACK_REPORTS_MAX
                    {
                        self.timing_refine_reports += 1;
                        info!(
                            "rx_hrpd_traffic[m{}]: timing_search_no_relock uati=0x{:08x} frame_chip={} current_delay={}+{:+.2} current_coh={:.3} current_snr={:.2}dB best_delay={}+{:+.2} best_coh={:.3} best_snr={:.2}dB radius={:.1}",
                            self.config.mac_index,
                            self.config.uati,
                            self.next_params.frame_start_chip,
                            current_delay,
                            current_fraction,
                            moments.coherence,
                            moments.snr_db,
                            best.sample_delay,
                            best.sample_delay_fraction,
                            best.coherence,
                            best.snr_db,
                            radius_samples,
                        );
                    }
                }
            }

            if log::log_enabled!(log::Level::Trace) {
                let per_slot = pilot_moments_by_slot_from_despread(&despread);
                let finite: Vec<f32> = per_slot
                    .iter()
                    .map(|m| m.snr_db)
                    .filter(|s| s.is_finite())
                    .collect();
                if !finite.is_empty() {
                    let min = finite.iter().copied().fold(f32::INFINITY, f32::min);
                    let max = finite.iter().copied().fold(f32::NEG_INFINITY, f32::max);
                    let mean = finite.iter().sum::<f32>() / finite.len() as f32;
                    trace!(
                        "rpc_metric_compare[m{}]: frame_snr={:.2}dB frame_coh={:.3} per_slot_snr min/mean/max={:.2}/{:.2}/{:.2}dB n={}",
                        self.config.mac_index,
                        moments.snr_db,
                        moments.coherence,
                        min,
                        mean,
                        max,
                        finite.len(),
                    );
                }
            }

            let pilot_locked = moments.coherence >= HRPD_REVERSE_TRAFFIC_PILOT_MIN_COHERENCE;
            let mut corrected_chips = despread;
            if pilot_locked || moments.phase_ramp_valid {
                derotate_frame_by_pilot_ramp(&mut corrected_chips, moments);
            }
            let phase_feedback_reliable = moments.phase_ramp_valid
                && moments.coherence >= HRPD_PHASE_TRACK_MIN_COHERENCE
                && moments.phase_step_rad_per_slot.abs() <= HRPD_PHASE_TRACK_MAX_STEP_RAD_PER_SLOT;

            if pilot_locked {
                // Roll the residual phase into the running estimate only
                // from strong pilot frames. Marginal frames can still carry
                // FCS-valid Q-arm data, but their phase ramp is not reliable
                // enough to seed the next frame's derotation.
                if phase_feedback_reliable {
                    self.next_params.pilot_phase =
                        self.next_params.pilot_phase * moments.next_frame_phase;
                    // Normalize to unit magnitude to keep the phase stable.
                    let norm = self.next_params.pilot_phase.norm();
                    if norm > 0.0 {
                        self.next_params.pilot_phase /= norm;
                    } else {
                        self.next_params.pilot_phase = Complex32::new(1.0, 0.0);
                    }
                } else if self.phase_hold_reports < 4 {
                    self.phase_hold_reports += 1;
                    info!(
                        "rx_hrpd_traffic[m{}]: holding pilot phase update uati=0x{:08x} frame_chip={} coh={:.3} snr={:.2}dB ramp_valid={} phase_step={:.3}",
                        self.config.mac_index,
                        self.config.uati,
                        self.next_params.frame_start_chip,
                        moments.coherence,
                        moments.snr_db,
                        moments.phase_ramp_valid,
                        moments.phase_step_rad_per_slot,
                    );
                }
                self.consecutive_low_coherence = 0;
                self.consecutive_high_coherence = self.consecutive_high_coherence.saturating_add(1);
                self.maybe_emit_reverse_pilot(moments);
                self.remember_good_pilot(moments);
                if self.consecutive_high_coherence
                    >= CONSECUTIVE_HIGH_COHERENCE_FRAMES_FOR_VALIDATION
                {
                    self.hard_validated_locally = true;
                }
            } else {
                self.consecutive_high_coherence = 0;
                self.consecutive_low_coherence = self.consecutive_low_coherence.saturating_add(1);
                trace!(
                    "hrpd_rev_traffic_finger {} low coherence frame_chip={} coh={:.3} snr={:.2}dB",
                    self.base.id,
                    self.next_params.frame_start_chip,
                    moments.coherence,
                    moments.snr_db
                );
                if self.low_coherence_reports < 1 {
                    self.low_coherence_reports += 1;
                    info!(
                        "rx_hrpd_traffic[m{}]: low pilot coherence finger={} uati=0x{:08x} frame_chip={} coh={:.3} snr={:.2}dB phase_update=skipped ramp_valid={} phase_step={:.3} delay={}+{:+.2}",
                        self.config.mac_index,
                        self.base.id,
                        self.config.uati,
                        self.next_params.frame_start_chip,
                        moments.coherence,
                        moments.snr_db,
                        moments.phase_ramp_valid,
                        moments.phase_step_rad_per_slot,
                        self.next_params.sample_delay,
                        self.next_params.sample_delay_fraction,
                    );
                }
                self.maybe_emit_reverse_pilot_lost(moments);
            }

            // Pilot coherence is a receiver lock metric, not a reverse
            // signaling validity check. Keep feeding the FCS-protected data
            // decoder so a hot Q-arm data frame is not lost just because the
            // pilot metric dipped; ACK/DRC processors gate their own
            // non-FCS decisions on the coherence tag.
            let frame_block =
                self.build_output_block(corrected_chips, moments.coherence, moments.snr_db);
            let sub_out = run_chain(chain, frame_block);
            out.extend(sub_out);

            self.advance_to_next_frame();
        }

        // Drop already-consumed samples; keep the interpolator back margin
        // ahead of the next frame start for the next call.
        if let Some(abs) = self.buffer_abs_sample {
            let frame_start_sample = self
                .next_params
                .frame_start_chip
                .saturating_mul(oversample as u64);
            let frame_needed_start = frame_start_sample.saturating_sub(retention_back_margin);
            let needed_start = self
                .pending_fast_drc_retention_sample(oversample as u64, retention_back_margin)
                .map_or(frame_needed_start, |drc_start| {
                    frame_needed_start.min(drc_start)
                });
            if needed_start > abs {
                let to_drop = ((needed_start - abs) as usize).min(self.buffer.len());
                self.buffer.drain(..to_drop);
                self.buffer_abs_sample = Some(abs + to_drop as u64);
            }
        }
        self.last_block_processed_chips = (block.samples.len() / oversample.max(1)) as u64;
        self.base
            .tick_and_validate(&out, self.last_block_processed_chips);
        out
    }

    fn flush(&mut self, chain: &mut Vec<PipelineProcessorShared>) -> Vec<SampleBlock> {
        BaseFinger::flush_chain(chain)
    }

    fn is_hard_validated(&self) -> bool {
        // Honor either the BaseFinger CRC-clean path or our local coherence
        // sustained-lock path; either way the finger is healthy.
        self.hard_validated_locally || self.base.is_hard_validated()
    }

    fn is_soft_validated(&self) -> bool {
        // The reverse pilot has been acquired and reported to the AN. The
        // connection is established even if no CRC-clean reverse frame has
        // arrived yet, so the prune policy should not retire it on the short
        // pre-validation grace.
        self.reverse_pilot_event_sent || self.is_hard_validated()
    }

    fn signal_lost_chips(&self) -> u64 {
        self.low_coherence_start_chip
            .map(|start| self.next_params.frame_start_chip.saturating_sub(start))
            .unwrap_or(0)
    }

    fn should_retire(&self) -> bool {
        self.reverse_pilot_lost_event_sent
            || (self.reverse_pilot_event_sent
                && self.signal_lost_chips() >= HRPD_REVERSE_TRAFFIC_REACQUIRE_TIMEOUT_CHIPS)
    }

    fn idle_blocks(&self) -> u64 {
        self.base.idle_blocks()
    }

    fn idle_chips(&self) -> u64 {
        self.base.idle_chips()
    }
}

fn run_chain(chain: &mut Vec<PipelineProcessorShared>, input: SampleBlock) -> Vec<SampleBlock> {
    use crate::receiver::pipelined::{VecEmitter, run_sub_chain};
    let mut emitter = VecEmitter::new();
    let mut out = run_sub_chain(chain, input, &mut emitter);
    out.extend(emitter.blocks);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use cdma_common::hrpd::traffic::physical_crc24;

    use crate::phy::hrpd::turbo::HrpdTurboEncoder;
    use crate::receiver::hrpd::reverse_traffic_rake::rri_subtype2::rri_subtype2_payload_index_for_bits;
    use crate::receiver::hrpd::reverse_traffic_rake::subtype2_data::subpacket_code_symbols;

    const PACKET_FCS_BITS: usize = 24;
    const PACKET_TAIL_BITS: usize = 6;

    fn test_finger_with_power_control(
        power_control: Option<HrpdPowerControlHandle>,
    ) -> HrpdReverseTrafficFinger {
        let config = HrpdReverseTrafficFingerConfig {
            uati: 0x0102_0304,
            mac_index: 5,
            physical_layer_subtype: 2,
            reverse_traffic_mac_subtype: cdma_common::hrpd::traffic::REVERSE_TRAFFIC_MAC_SUBTYPE3,
            frame_offset: 0,
            i_mask: 0,
            q_mask: 0,
            drc_cover: 0,
            drc_length: 8,
            oversample: 1,
            event_tx: None,
            harq_bus: None,
            power_control,
            reverse_pilot_acquired: None,
            worker_spawned_at: std::time::Instant::now(),
        };
        let lock = HrpdReverseTrafficFingerLock {
            frame_start_chip: 0,
            chip_offset: 0,
            sample_delay: 0,
            sample_delay_fraction: 0.0,
            q_sign: 1.0,
            q_pair_phase: 0,
            initial_pilot_phase: Complex32::new(1.0, 0.0),
        };
        HrpdReverseTrafficFinger::new(1, config, lock)
    }

    fn test_finger() -> HrpdReverseTrafficFinger {
        test_finger_with_power_control(None)
    }

    fn lcg_bits(count: usize, seed: u32) -> Vec<u8> {
        let mut s = seed;
        (0..count)
            .map(|_| {
                s = s.wrapping_mul(1_103_515_245).wrapping_add(12_345);
                ((s >> 16) & 1) as u8
            })
            .collect()
    }

    fn build_packet_bits(payload_bits: usize, seed: u32) -> Vec<u8> {
        let mac_bits = payload_bits - PACKET_FCS_BITS - PACKET_TAIL_BITS;
        let mut bits = lcg_bits(mac_bits, seed);
        let fcs = physical_crc24(&bits);
        for bit in (0..PACKET_FCS_BITS).rev() {
            bits.push(((fcs >> bit) & 1) as u8);
        }
        bits.extend(std::iter::repeat_n(0u8, PACKET_TAIL_BITS));
        bits
    }

    fn subtype2_detection(payload_bits: u32, subpacket_id: u8) -> RriSubtype2Detection {
        let payload_index = rri_subtype2_payload_index_for_bits(payload_bits).expect("RRI payload");
        RriSubtype2Detection {
            payload_index,
            subpacket_id,
            payload_bits,
            best_score: 1.0,
            second_score: 0.0,
            margin: 1.0,
        }
    }

    fn subtype2_packet_chips(
        payload_bits: usize,
        interlace: u8,
        seed: u32,
    ) -> (Vec<u8>, Vec<Complex32>) {
        let format = Subtype2DataFormat::for_payload_bits(payload_bits).expect("format");
        let packet = build_packet_bits(payload_bits, seed);
        let encoder = HrpdTurboEncoder::new(payload_bits as u32).expect("encoder");
        let mut coded = encoder.encode(&packet, 1, format.turbo_code_rate_den);
        format.scramble_encoder_output(&mut coded, interlace);
        let interleaved = format.interleave_encoder_output(&coded);
        let code_symbols = subpacket_code_symbols(format, &interleaved, 0);
        (packet, format.modulate_subpacket(&code_symbols))
    }

    #[test]
    fn lost_pilot_holds_despite_high_stale_prediction() {
        let mut rpc = HrpdRpcController::new();
        assert!(rpc.ingest(HRPD_RPC_TARGET_SNR_DB + 4.0, 1.0).is_some());

        let mut ups = 0usize;
        let mut downs = 0usize;
        for _ in 0..24 {
            match rpc.emit_with_mobile_power(None, -80.0) {
                0 => ups += 1,
                _ => downs += 1,
            }
        }

        assert!(
            ups.abs_diff(downs) <= 1,
            "pilot loss should hold neutral: up={ups} down={downs}"
        );
    }

    #[test]
    fn rpc_rejects_borderline_coherence_measurements() {
        let mut rpc = HrpdRpcController::new();

        assert!(
            rpc.ingest(HRPD_RPC_TARGET_SNR_DB + 2.0, 0.50).is_none(),
            "borderline-coherence pilot samples should not reset the RPC predictor"
        );
        assert!(
            rpc.ingest(
                HRPD_RPC_TARGET_SNR_DB + 2.0,
                HRPD_RPC_MIN_COHERENCE_FOR_DECISION
            )
            .is_some(),
            "the configured reliability gate remains inclusive"
        );
    }

    #[test]
    fn rpc_controller_uses_updated_outer_loop_target() {
        let mut rpc = HrpdRpcController::new();
        rpc.set_target_db(crate::bts::hrpd::power_control::HRPD_AUTO_MAX_TARGET_DB);

        let level = rpc.ingest(12.0, 1.0);
        assert_eq!(rpc.emit_with_mobile_power(level, -80.0), 0);
    }

    #[test]
    fn rev_a_decode_after_low_latency_target_counts_as_erasure() {
        let registry = crate::bts::hrpd::HrpdPowerControlRegistry::default();
        let handle = registry.install(0x0102_0304, 5);
        let mut finger = test_finger();
        finger.config.power_control = Some(handle.clone());

        finger.report_terminal_packet(TerminalPacketOutcome {
            disposition: TerminalPacketDisposition::Decoded {
                transmission_mode: crate::bts::hrpd::HrpdTransmissionMode::LowLatency,
            },
            payload_bits: 1024,
            first_subpacket_start_slot: 120,
            interlace: 0,
            subpackets_accumulated: 3,
            decoded_subpacket: Some(3),
        });

        let snapshot = handle.snapshot().expect("active assignment");
        assert_eq!(snapshot.target_db, HRPD_INITIAL_TARGET_DB);
        assert_eq!(snapshot.packets_erased, 1);
        assert_eq!(snapshot.packets_late_success, 1);
    }

    #[test]
    fn mobile_power_limited_excludes_rev_a_erasure_from_outer_loop() {
        let registry = crate::bts::hrpd::HrpdPowerControlRegistry::default();
        let handle = registry.install(0x0102_0304, 5);
        let mut finger = test_finger();
        finger.config.power_control = Some(handle.clone());
        finger.rpc.current_mobile_over_limit = true;

        finger.report_terminal_packet(TerminalPacketOutcome {
            disposition: TerminalPacketDisposition::Exhausted,
            payload_bits: 1024,
            first_subpacket_start_slot: 120,
            interlace: 0,
            subpackets_accumulated: 4,
            decoded_subpacket: None,
        });

        let snapshot = handle.snapshot().expect("active assignment");
        assert_eq!(snapshot.target_db, HRPD_INITIAL_TARGET_DB);
        assert_eq!(snapshot.packets_total, 0);
        assert_eq!(snapshot.packets_excluded, 1);
    }

    #[test]
    fn timing_reacquisition_excludes_rev_a_success_from_outer_loop() {
        let registry = crate::bts::hrpd::HrpdPowerControlRegistry::default();
        let handle = registry.install(0x0102_0304, 5);
        let mut finger = test_finger();
        finger.config.power_control = Some(handle.clone());
        finger.timing_state = HrpdTrafficTimingState::Reacquiring;

        finger.report_terminal_packet(TerminalPacketOutcome {
            disposition: TerminalPacketDisposition::Decoded {
                transmission_mode: crate::bts::hrpd::HrpdTransmissionMode::HighCapacity,
            },
            payload_bits: 1024,
            first_subpacket_start_slot: 120,
            interlace: 0,
            subpackets_accumulated: 1,
            decoded_subpacket: Some(1),
        });

        let snapshot = handle.snapshot().expect("active assignment");
        assert_eq!(snapshot.target_db, HRPD_INITIAL_TARGET_DB);
        assert_eq!(snapshot.packets_total, 0);
        assert_eq!(snapshot.packets_excluded, 1);
    }

    #[test]
    fn replacement_finger_resumes_outer_loop_after_tune_away() {
        let registry = crate::bts::hrpd::HrpdPowerControlRegistry::default();
        let handle = registry.install(0x0102_0304, 5);
        assert!(handle.suspend_for_tune_away());

        let mut replacement = test_finger_with_power_control(Some(handle.clone()));
        assert!(replacement.power_control_tune_away_active);
        replacement.update_rpc_reliability_trace(
            true,
            1,
            0,
            PilotMoments {
                pilot_phase: Complex32::new(1.0, 0.0),
                next_frame_phase: Complex32::new(1.0, 0.0),
                phase_at_frame_start_rad: 0.0,
                phase_step_rad_per_slot: 0.0,
                phase_ramp_valid: true,
                coherence: 1.0,
                snr_db: HRPD_RPC_TARGET_SNR_DB,
                rc3_sinr_db: HRPD_RPC_TARGET_SNR_DB,
                pilot_amplitude_step_db: 0.0,
                coherent_pilot_power: 1.0,
                noise_pilot_power: 1.0,
            },
            -60.0,
            Some(HRPD_RPC_TARGET_SNR_DB),
        );

        let snapshot = handle.snapshot().expect("active assignment");
        assert!(!snapshot.tune_away_active);
        assert!(snapshot.return_successes_remaining > 0);
    }

    #[test]
    fn rpc_reuses_recent_stable_metric_but_not_across_dtx() {
        let mut rpc = HrpdRpcController::new();
        let reliable = rpc
            .ingest(HRPD_RPC_TARGET_SNR_DB + 2.0, 1.0)
            .expect("reliable pilot metric");
        assert_eq!(rpc.control_level(Some(reliable)), Some(reliable));

        for age in 1..=HRPD_RPC_MAX_REUSED_METRIC_AGE_SLOTS {
            assert!(rpc.ingest(f32::NAN, 0.0).is_none());
            assert!(
                rpc.control_level(None).is_some(),
                "stable metric should be reusable at age {age}"
            );
        }
        assert!(rpc.ingest(f32::NAN, 0.0).is_none());
        assert!(
            rpc.control_level(None).is_none(),
            "a sustained DTX gap must expire the quality estimate"
        );
    }

    #[test]
    fn reused_metric_carries_the_reliable_mobile_power() {
        let mut rpc = HrpdRpcController::new();
        let active_mobile_dbfs = -55.0;
        let reliable = rpc
            .ingest(HRPD_RPC_TARGET_SNR_DB, 1.0)
            .expect("reliable pilot metric");
        rpc.observe_reliable_mobile_power(active_mobile_dbfs);
        assert_eq!(rpc.control_level(Some(reliable)), Some(reliable));

        assert!(rpc.ingest(f32::NAN, 0.0).is_none());
        assert_eq!(
            rpc.control_mobile_power(None, -80.0),
            active_mobile_dbfs,
            "reused quality must carry its active-slot mobile power"
        );
        let reused = rpc.control_level(None);
        assert!(reused.is_some());
        let _ = rpc.emit_with_mobile_power(reused, -80.0);

        assert_eq!(rpc.last_reliable_mobile_power_dbfs, active_mobile_dbfs);
    }

    #[test]
    fn paired_mobile_power_forces_down_on_a_silent_control_phase() {
        let mut rpc = HrpdRpcController::new();
        let level = rpc
            .ingest(HRPD_RPC_TARGET_SNR_DB - 4.0, 1.0)
            .expect("reliable under-target metric");

        assert_eq!(
            rpc.emit_with_mobile_power(Some(level), HRPD_RPC_MOBILE_HARD_LIMIT_DBFS + 1.0),
            1,
            "the mobile-power sample paired with a reused metric owns the ceiling"
        );
    }

    #[test]
    fn rpc_strong_under_target_nets_up_without_slam() {
        let mut rpc = HrpdRpcController::new();

        let mut up = 0;
        let mut down = 0;
        for _ in 0..24 {
            let level = rpc.ingest(HRPD_RPC_TARGET_SNR_DB - 8.0, 1.0);
            match rpc.emit_with_mobile_power(level, -40.0) {
                0 => up += 1,
                _ => down += 1,
            }
        }

        assert!(
            up > down,
            "under-target metric must net UP: up={up} down={down}"
        );
        assert!(
            down > 0,
            "large errors still obey the fractional drive clamp"
        );
    }

    #[test]
    fn rpc_strong_over_target_nets_down_without_slam() {
        let mut rpc = HrpdRpcController::new();

        let mut up = 0;
        let mut down = 0;
        for _ in 0..24 {
            let level = rpc.ingest(HRPD_RPC_TARGET_SNR_DB + 8.0, 1.0);
            match rpc.emit_with_mobile_power(level, -40.0) {
                0 => up += 1,
                _ => down += 1,
            }
        }

        assert!(
            down > up,
            "over-target metric must net DOWN: up={up} down={down}"
        );
        assert!(up > 0, "large errors still obey the fractional drive clamp");
    }

    #[test]
    fn rpc_cold_mobile_power_lets_metric_own_the_decision() {
        let mut rpc = HrpdRpcController::new();
        let cold = -49.0;
        for _ in 0..24 {
            rpc.observe_mobile_power(cold);
        }
        let level = rpc.ingest(HRPD_RPC_TARGET_SNR_DB + 8.0, 1.0);

        assert_eq!(
            rpc.brake_offset_db(cold),
            0.0,
            "cold mobile power applies no brake"
        );
        assert_eq!(
            rpc.emit_with_mobile_power(level, cold),
            1,
            "an over-target metric at cold mobile power commands DOWN"
        );
        assert_eq!(rpc.mobile_limit_downs, 0, "cold power is not limited");
    }

    #[test]
    fn rpc_mobile_power_ceiling_forces_down_without_rejecting_metric() {
        let mut rpc = HrpdRpcController::new();
        let clipping = HRPD_RPC_MOBILE_HARD_LIMIT_DBFS + 2.0;
        rpc.observe_mobile_power(clipping);
        let level = rpc.ingest(HRPD_RPC_TARGET_SNR_DB + 20.0, 1.0);
        assert!(level.is_some(), "a power-limited pilot remains measurable");

        assert_eq!(
            rpc.emit_with_mobile_power(level, clipping),
            1,
            "a mobile above its power ceiling forces DOWN"
        );
        assert_eq!(rpc.mobile_limit_downs, 1);
    }

    #[test]
    fn rpc_mobile_brake_converts_up_to_down_before_the_ceiling() {
        let hot_but_unclipped = HRPD_RPC_MOBILE_HARD_LIMIT_DBFS - 0.5;

        // Control: the same under-target metric with cold mobile power goes UP.
        let mut cold = HrpdRpcController::new();
        for _ in 0..64 {
            cold.observe_mobile_power(-50.0);
        }
        let cold_level = cold.ingest(HRPD_RPC_TARGET_SNR_DB - 1.5, 1.0);
        assert_eq!(
            cold.emit_with_mobile_power(cold_level, -50.0),
            0,
            "under-target with cold mobile power commands UP"
        );

        let mut rpc = HrpdRpcController::new();
        for _ in 0..64 {
            rpc.observe_mobile_power(hot_but_unclipped);
        }
        assert!(
            !rpc.current_mobile_over_limit,
            "the brake zone is below the hard limit"
        );
        assert!(
            rpc.brake_offset_db(hot_but_unclipped) > 1.5,
            "the per-mobile brake is engaged"
        );
        let level = rpc.ingest(HRPD_RPC_TARGET_SNR_DB - 1.5, 1.0);
        assert_eq!(
            rpc.emit_with_mobile_power(level, hot_but_unclipped),
            1,
            "the pre-clip brake converts an under-target UP to DOWN"
        );
    }

    #[test]
    fn rpc_mobile_brake_is_not_overridden_by_direction_guard() {
        let hot_but_unclipped = HRPD_RPC_MOBILE_BRAKE_FULL_DBFS;
        let mut rpc = HrpdRpcController::new();
        let level = rpc.ingest(HRPD_RPC_TARGET_SNR_DB - 5.0, 1.0);

        assert_eq!(
            rpc.emit_with_mobile_power(level, hot_but_unclipped),
            1,
            "the full mobile brake remains authoritative below a low SINR target"
        );
    }

    #[test]
    fn rpc_lost_metric_holds_instead_of_ramping() {
        // Live regression: during a reverse data transfer the W0 metric is
        // unreliable on nearly every slot. Blindly commanding UP there ramps
        // the AT into clipping, so a lost metric with cold raw and no fade
        // holds neutral.
        let mut rpc = HrpdRpcController::new();
        for _ in 0..24 {
            rpc.observe_raw_power(-45.0);
        }
        // Last reliable measurement was healthy (above target).
        let _ = rpc.ingest(HRPD_RPC_TARGET_SNR_DB + 4.0, 1.0);

        let mut up = 0u32;
        let mut down = 0u32;
        for _ in 0..24 {
            rpc.observe_raw_power(-45.0);
            let lost = rpc.ingest(f32::NAN, 0.0);
            match rpc.emit_with_mobile_power(lost, -45.0) {
                0 => up += 1,
                _ => down += 1,
            }
        }
        assert!(
            up.abs_diff(down) <= 1,
            "a lost metric with a healthy last prediction holds neutral: up={up} down={down}"
        );
        assert!(rpc.metric_holds >= 20);
    }

    #[test]
    fn q2_data_transform_probe_locks_conjugate_branch() {
        let payload_bits = 3072usize;
        let seed = 0x5132_0001;
        let mut finger = test_finger();
        let format = Subtype2DataFormat::for_payload_bits(payload_bits).expect("format");
        assert_eq!(format.modulation, ModulationFormat::Q2);
        let (packet, chips) = subtype2_packet_chips(payload_bits, 0, seed);
        // Receive-side conjugation is the only physically possible residual
        // ambiguity after pilot derotation; conjugating the observed chips
        // must lock the conj lane (which un-conjugates them).
        let observed = chips
            .into_iter()
            .map(|chip| Subtype2DataTransform::Conjugate.apply(chip))
            .collect::<Vec<_>>();
        let detection = subtype2_detection(payload_bits as u32, 0);

        let outcome = finger.ingest_subtype2_data_subframe(120, &detection, &observed, format);

        assert_eq!(
            finger.subframe_data_transform,
            Some(Subtype2DataTransform::Conjugate)
        );
        assert!(outcome.decoded);
        let mac_end = payload_bits - PACKET_FCS_BITS - PACKET_TAIL_BITS;
        assert_eq!(outcome.delivered.as_deref(), Some(&packet[..mac_end]));
    }

    #[test]
    fn b4_branch_probe_accepts_crc_valid_low_amplitude_packet() {
        let payload_bits = 128usize;
        let seed = 0x5132_0002;
        let mut finger = test_finger();
        let format = Subtype2DataFormat::for_payload_bits(payload_bits).expect("format");
        assert_eq!(format.modulation, ModulationFormat::B4);
        let (packet, chips) = subtype2_packet_chips(payload_bits, 0, seed);
        let observed = chips
            .into_iter()
            .map(|chip| Subtype2DataTransform::Conjugate.apply(chip) * 0.01)
            .collect::<Vec<_>>();
        let detection = subtype2_detection(payload_bits as u32, 0);

        let outcome = finger.ingest_subtype2_data_subframe(120, &detection, &observed, format);

        assert_eq!(finger.subframe_b4_lane, Some(B4DataLane::QInverted));
        assert!(outcome.decoded);
        let mac_end = payload_bits - PACKET_FCS_BITS - PACKET_TAIL_BITS;
        assert_eq!(outcome.delivered.as_deref(), Some(&packet[..mac_end]));
    }

    #[test]
    fn repeated_drc_slot_projects_to_window_completion_slot() {
        for slot in 0..7 {
            assert_eq!(drc_completion_slot_for_repetition(slot, 0, 8), 7);
        }
        for slot in 7..15 {
            assert_eq!(drc_completion_slot_for_repetition(slot, 0, 8), 15);
        }
        assert_eq!(drc_completion_slot_for_repetition(15, 0, 8), 23);
        assert_eq!(drc_completion_slot_for_repetition(12, 0, 4), 15);
        assert_eq!(drc_completion_slot_for_repetition(12, 0, 1), 13);
        assert_eq!(drc_completion_slot_for_repetition(0, 3, 8), 2);
        assert_eq!(drc_completion_slot_for_repetition(3, 3, 8), 10);
        assert_eq!(drc_window_start_slot_at_or_after(0, 3, 8), 2);
        assert_eq!(drc_window_start_slot_at_or_after(2, 3, 8), 2);
        assert_eq!(drc_window_start_slot_at_or_after(3, 3, 8), 10);
    }

    #[test]
    fn fast_drc_repetitions_require_same_slot_confirmation() {
        let bus = Arc::new(HarqBus::new());
        let config = HrpdReverseTrafficFingerConfig {
            uati: 0x0102_0304,
            mac_index: 5,
            physical_layer_subtype: 0,
            reverse_traffic_mac_subtype: 0,
            frame_offset: 0,
            i_mask: 0,
            q_mask: 0,
            drc_cover: 0,
            drc_length: 8,
            oversample: 1,
            event_tx: None,
            harq_bus: Some(bus.clone()),
            power_control: None,
            reverse_pilot_acquired: None,
            worker_spawned_at: std::time::Instant::now(),
        };
        let lock = HrpdReverseTrafficFingerLock {
            frame_start_chip: 0,
            chip_offset: 0,
            sample_delay: 0,
            sample_delay_fraction: 0.0,
            q_sign: 1.0,
            q_pair_phase: 0,
            initial_pilot_phase: Complex32::new(1.0, 0.0),
        };
        let mut finger = HrpdReverseTrafficFinger::new(1, config, lock);
        let slot = 1234;
        let symbol_a = DrcSymbol {
            value: 0x0a,
            confidence: 7.0,
        };
        let symbol_b = DrcSymbol {
            value: 0x0c,
            confidence: 7.0,
        };

        finger.publish_repeated_drc(slot, symbol_a);
        finger.publish_repeated_drc(slot, symbol_b);
        finger.publish_repeated_drc(slot, symbol_a);
        assert_eq!(bus.drc_at_slot(5, slot), None);

        finger.publish_repeated_drc(slot, symbol_a);
        assert_eq!(bus.drc_at_slot(5, slot), Some(0x0a));

        finger.publish_repeated_drc(slot, symbol_b);
        finger.publish_repeated_drc(slot, symbol_b);
        assert_eq!(bus.drc_at_slot(5, slot), Some(0x0a));

        finger.publish_confirmed_drc(slot, symbol_b, true);
        assert_eq!(bus.drc_at_slot(5, slot), Some(0x0c));
    }

    #[test]
    fn fast_drc_retains_mid_slot_windows_across_frame_blocks() {
        let mut finger = test_finger();
        let mut chain = Vec::new();
        let block_chips = HRPD_TRAFFIC_FRAME_CHIPS + 64;

        let first = SampleBlock::new(vec![Complex32::new(0.0, 0.0); block_chips], 0);
        let _ = finger.process(&first, &mut chain);
        assert_eq!(finger.fast_drc_stats.window_attempts, 1);
        assert_eq!(finger.fast_drc_stats.repetition_attempts, 15);

        let second = SampleBlock::new(vec![Complex32::new(0.0, 0.0); block_chips], block_chips);
        let _ = finger.process(&second, &mut chain);

        assert_eq!(
            finger.fast_drc_stats.window_attempts, 3,
            "the full window crossing the first frame boundary must be decoded"
        );
        assert_eq!(
            finger.fast_drc_stats.repetition_attempts, 31,
            "the one-slot repetition crossing the first frame boundary must be decoded"
        );
    }
}
