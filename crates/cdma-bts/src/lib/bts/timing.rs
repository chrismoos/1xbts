use std::{
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    thread,
    time::{Duration, Instant},
};

use cdma_common::{error::Error, time};
use log::{info, trace};
use num::complex::Complex32;

use crate::sdr::{RadioRx, RadioTx};

use super::TxLoopState;

/// Shared TX→RX timing anchor. TX writes once after first batch. RX reads to synchronize.
pub struct TxRxAnchor {
    /// Hardware tick at which TX started transmitting.
    pub tick: AtomicU64,
    /// Absolute chip number corresponding to that tick.
    pub chip: AtomicU64,
    /// Set to `true` when the anchor is valid.
    pub valid: AtomicBool,
}

impl TxRxAnchor {
    /// Create an empty timing anchor.
    pub fn new() -> Self {
        Self {
            tick: AtomicU64::new(0),
            chip: AtomicU64::new(0),
            valid: AtomicBool::new(false),
        }
    }

    /// Publish a hardware-tick to absolute-chip mapping.
    pub fn publish(&self, tick: u64, chip: u64) {
        self.tick.store(tick, Ordering::Release);
        self.chip.store(chip, Ordering::Release);
        self.valid.store(true, Ordering::Release);
    }

    /// Load the current anchor if TX has published one.
    pub fn try_load(&self) -> Option<(u64, u64)> {
        if self.valid.load(Ordering::Acquire) {
            Some((
                self.tick.load(Ordering::Acquire),
                self.chip.load(Ordering::Acquire),
            ))
        } else {
            None
        }
    }
}

pub(super) const HARDWARE_START_LEAD_NS: u64 = 100_000_000;
const LOOKAHEAD_FINAL_SPIN_NS: u64 = 20_000;
/// Adaptive sleep margin bounds. The cap stays well under the post-release
/// underrun cushion (`max_tx_lookahead_ms` minus gen+flush time).
const PACER_MIN_MARGIN_NS: u64 = 50_000;
const PACER_MAX_MARGIN_NS: u64 = 2_000_000;
/// Pad above the worst observed oversleep when raising the margin.
const PACER_OVERSLEEP_PAD_NS: u64 = 20_000;
/// Warn when a wake lands this far past the release point.
const PACER_LATE_WAKE_WARN_NS: u64 = 500_000;
/// Per-wait margin decay toward the floor: margin -= margin >> SHIFT.
const PACER_MARGIN_DECAY_SHIFT: u32 = 8;

pub(super) struct TxAnchor {
    pub hardware_start_tick: u64,
    pub hardware_start_chip: u64,
    pub chip_cursor: u64,
    pub skip_chips: u64,
}

pub(super) struct FrameBoundaries {
    pub sync_frame_boundary: bool,
    pub paging_frame_boundary: bool,
    pub paging_enabled: bool,
}

pub(super) fn align_to_residue(value: u64, modulus: u64, residue: u64) -> u64 {
    if modulus == 0 {
        return value;
    }
    let r = residue % modulus;
    let v = value % modulus;
    if v == r {
        value
    } else if v < r {
        value + (r - v)
    } else {
        value + (modulus - (v - r))
    }
}

pub(super) fn chips_to_ticks(chips: u64, chip_rate_hz: u64, tick_rate: u64) -> u64 {
    if chip_rate_hz == 0 {
        return 0;
    }
    let ticks = (chips as u128).saturating_mul(tick_rate as u128) / chip_rate_hz as u128;
    ticks.min(u64::MAX as u128) as u64
}

fn ticks_to_nanos(ticks: u64, tick_rate: u64) -> u64 {
    if tick_rate == 0 {
        return 0;
    }
    let ns = (ticks as u128).saturating_mul(1_000_000_000u128) / tick_rate as u128;
    ns.min(u64::MAX as u128) as u64
}

pub(super) fn pilot_offset_chips(pilot_offset: usize) -> u64 {
    (pilot_offset as u64) * 64
}

/// Lookahead throttle for the TX synth loop. Sleeps to the release point in
/// one shot, holding back an adaptive margin that tracks observed scheduler
/// oversleep, then yields/spins only across that margin. The release
/// condition is re-checked after every wake, so it can never release early;
/// a late wake only shrinks the post-release underrun cushion.
pub(super) struct LookaheadPacer {
    margin_ns: u64,
}

impl LookaheadPacer {
    pub(super) fn new() -> Self {
        Self {
            margin_ns: PACER_MIN_MARGIN_NS,
        }
    }

    /// Current sleep margin in microseconds (for heartbeat diagnostics).
    pub(super) fn margin_us(&self) -> u64 {
        self.margin_ns / 1_000
    }

    fn adapt_after_sleep(&mut self, requested_ns: u64, actual_ns: u64) {
        let oversleep_ns = actual_ns.saturating_sub(requested_ns);
        // Oversleep beyond the held-back margin means we woke past release.
        let late_past_release_ns = oversleep_ns.saturating_sub(self.margin_ns);
        if late_past_release_ns > PACER_LATE_WAKE_WARN_NS {
            log::warn!(
                "tx_pace_late_wake: slept {}us for a {}us request, {}us past release (margin was {}us)",
                actual_ns / 1_000,
                requested_ns / 1_000,
                late_past_release_ns / 1_000,
                self.margin_ns / 1_000,
            );
        }
        let needed_ns = oversleep_ns.saturating_add(PACER_OVERSLEEP_PAD_NS);
        if needed_ns > self.margin_ns {
            self.margin_ns = needed_ns.min(PACER_MAX_MARGIN_NS);
        } else {
            self.margin_ns = (self.margin_ns - (self.margin_ns >> PACER_MARGIN_DECAY_SHIFT))
                .max(PACER_MIN_MARGIN_NS);
        }
    }

    pub(super) fn wait_until_within_tx_lookahead(
        &mut self,
        batch_playout_tick: u64,
        wall_anchor_tick: u64,
        wall_anchor_instant: Instant,
        tick_rate: u64,
        max_tx_lookahead_ms: u32,
        shutdown: &AtomicBool,
    ) {
        if max_tx_lookahead_ms == 0 || tick_rate == 0 {
            return;
        }
        let lookahead_ticks = max_tx_lookahead_ms as u64 * tick_rate / 1_000;

        loop {
            let elapsed_ns = wall_anchor_instant.elapsed().as_nanos() as u64;
            let estimated_hw = wall_anchor_tick
                .saturating_add((elapsed_ns as u128 * tick_rate as u128 / 1_000_000_000) as u64);
            let ahead_ticks = batch_playout_tick.saturating_sub(estimated_hw);
            if ahead_ticks <= lookahead_ticks || shutdown.load(Ordering::Relaxed) {
                break;
            }

            let wait_ns = ticks_to_nanos(ahead_ticks - lookahead_ticks, tick_rate);
            if wait_ns > self.margin_ns {
                let sleep_ns = wait_ns - self.margin_ns;
                let sleep_start = Instant::now();
                thread::sleep(Duration::from_nanos(sleep_ns));
                self.adapt_after_sleep(sleep_ns, sleep_start.elapsed().as_nanos() as u64);
            } else if wait_ns > LOOKAHEAD_FINAL_SPIN_NS {
                thread::yield_now();
            } else {
                std::hint::spin_loop();
            }
        }
    }
}

pub(super) fn prime_hardware_clock(
    rx: &mut dyn RadioRx,
    radio_tx: &mut dyn RadioTx,
) -> Result<(), Error> {
    info!("bts: priming hardware clock with initial RX reads");
    rx.rx_activate(None)?;
    let mut prime_buf = vec![Complex32::new(0.0, 0.0); 1024];
    let _ = rx.rx_read(&mut prime_buf, 1_000_000);
    let t0 = radio_tx.get_hardware_time()?;
    thread::sleep(std::time::Duration::from_millis(1));
    let _ = rx.rx_read(&mut prime_buf, 1_000_000);
    let t1 = radio_tx.get_hardware_time()?;
    assert!(
        t1 > t0,
        "bts: hardware clock not incrementing after prime reads: t0={} t1={}",
        t0,
        t1
    );
    info!(
        "bts: hardware clock primed and verified: t0={} t1={} delta={}",
        t0,
        t1,
        t1 - t0
    );
    Ok(())
}

pub(super) fn compute_initial_tx_anchor(
    start_system_time: time::CdmaSystemTime,
    chip_rate: u64,
    tick_rate: u64,
    hardware_now: u64,
    sync_superframe_chips: u64,
    pilot_offset_chips: u64,
) -> TxAnchor {
    let now_chips = time::chips_since_epoch(start_system_time, chip_rate);
    let lead_chips = HARDWARE_START_LEAD_NS.saturating_mul(chip_rate) / 1_000_000_000u64;
    let future_chips = now_chips.saturating_add(lead_chips);
    let chip_cursor = align_to_residue(future_chips, sync_superframe_chips, pilot_offset_chips);
    let skip_chips = chip_cursor.saturating_sub(now_chips);
    let hardware_start_tick =
        hardware_now.saturating_add(chips_to_ticks(skip_chips, chip_rate, tick_rate));

    TxAnchor {
        hardware_start_tick,
        hardware_start_chip: chip_cursor,
        chip_cursor,
        skip_chips,
    }
}

pub(super) fn reseed_tx_anchor_from_live_clock(
    chip_rate: u64,
    tick_rate: u64,
    hardware_now: u64,
    previous_start_tick: u64,
    previous_start_chip: u64,
    sync_superframe_chips: u64,
    pilot_offset_chips: u64,
    max_tx_lookahead_ms: u32,
) -> TxAnchor {
    let lead_ticks = max_tx_lookahead_ms as u64 * tick_rate / 1_000;
    let elapsed_ticks = hardware_now.saturating_sub(previous_start_tick);
    let elapsed_chips = (elapsed_ticks as u128 * chip_rate as u128 / tick_rate as u128) as u64;
    let current_chips = previous_start_chip + elapsed_chips;
    let lead_chips = (lead_ticks as u128 * chip_rate as u128 / tick_rate as u128) as u64;
    let chip_cursor = align_to_residue(
        current_chips + lead_chips,
        sync_superframe_chips,
        pilot_offset_chips,
    );
    let skip_chips = chip_cursor.saturating_sub(current_chips);
    let hardware_start_tick = hardware_now + chips_to_ticks(skip_chips, chip_rate, tick_rate);

    TxAnchor {
        hardware_start_tick,
        hardware_start_chip: chip_cursor,
        chip_cursor,
        skip_chips,
    }
}

pub(super) fn apply_anchor(state: &mut TxLoopState, anchor: &TxAnchor) {
    state.hardware_start_tick = anchor.hardware_start_tick;
    state.hardware_start_chip = anchor.hardware_start_chip;
}

pub(super) fn batch_playout_tick(state: &TxLoopState, chip_cursor: u64, tick_rate: u64) -> u64 {
    state.hardware_start_tick.saturating_add(chips_to_ticks(
        chip_cursor.saturating_sub(state.hardware_start_chip),
        state.chip_rate,
        tick_rate,
    ))
}

pub(super) fn hardware_tick_at_chip(state: &TxLoopState, chip: u64, tick_rate: u64) -> u64 {
    state.hardware_start_tick.saturating_add(chips_to_ticks(
        chip.saturating_sub(state.hardware_start_chip),
        state.chip_rate,
        tick_rate,
    ))
}

pub(super) fn frame_boundaries(state: &TxLoopState, block_chip: u64) -> FrameBoundaries {
    FrameBoundaries {
        sync_frame_boundary: block_chip >= state.pilot_offset_chips
            && (block_chip - state.pilot_offset_chips) % state.sync_frame_chips == 0,
        paging_frame_boundary: block_chip >= state.pilot_offset_chips
            && (block_chip - state.pilot_offset_chips) % state.paging_frame_chips == 0,
        paging_enabled: block_chip >= state.paging_start_enable_chip,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pacer_margin_rises_to_observed_oversleep_plus_pad() {
        let mut pacer = LookaheadPacer::new();
        // Requested 1 ms, took 1.5 ms → 500 µs oversleep.
        pacer.adapt_after_sleep(1_000_000, 1_500_000);
        assert_eq!(pacer.margin_ns, 500_000 + PACER_OVERSLEEP_PAD_NS);
    }

    #[test]
    fn pacer_margin_is_capped() {
        let mut pacer = LookaheadPacer::new();
        pacer.adapt_after_sleep(1_000_000, 100_000_000);
        assert_eq!(pacer.margin_ns, PACER_MAX_MARGIN_NS);
    }

    #[test]
    fn pacer_margin_decays_toward_floor_on_clean_sleeps() {
        let mut pacer = LookaheadPacer::new();
        pacer.adapt_after_sleep(1_000_000, 2_000_000);
        let raised = pacer.margin_ns;
        for _ in 0..10_000 {
            pacer.adapt_after_sleep(1_000_000, 1_000_000);
        }
        assert!(pacer.margin_ns < raised);
        assert_eq!(pacer.margin_ns, PACER_MIN_MARGIN_NS);
    }

    #[test]
    fn pacer_returns_immediately_when_within_lookahead() {
        let mut pacer = LookaheadPacer::new();
        let shutdown = AtomicBool::new(false);
        let start = Instant::now();
        // Playout tick equals the anchor tick → 0 ticks ahead, no wait.
        pacer.wait_until_within_tx_lookahead(
            1_000,
            1_000,
            Instant::now(),
            1_000_000_000,
            5,
            &shutdown,
        );
        assert!(start.elapsed() < Duration::from_millis(2));
    }

    #[test]
    fn pacer_respects_shutdown() {
        let mut pacer = LookaheadPacer::new();
        let shutdown = AtomicBool::new(true);
        let start = Instant::now();
        // 10 s ahead of the lookahead window, but shutdown is set.
        pacer.wait_until_within_tx_lookahead(
            10_000_000_000,
            0,
            Instant::now(),
            1_000_000_000,
            5,
            &shutdown,
        );
        assert!(start.elapsed() < Duration::from_millis(2));
    }
}

pub(super) fn log_anchor(label: &str, hardware_now: u64, anchor: &TxAnchor, chip_rate: u64) {
    let lead_ms = chips_to_ticks(anchor.skip_chips, chip_rate, 1_000);
    trace!("chip aligned to {}", anchor.chip_cursor);
    info!(
        "{} hw_now={} start_tick={} chip_cursor={} skip_chips={} lead_ms={}",
        label,
        hardware_now,
        anchor.hardware_start_tick,
        anchor.chip_cursor,
        anchor.skip_chips,
        lead_ms
    );
}
