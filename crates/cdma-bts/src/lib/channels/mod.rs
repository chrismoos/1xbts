pub mod f_sch_rc3;
pub mod fpch;
pub mod fsch;
pub mod ftch;
pub mod ftch_rc2;
pub mod ftch_rc3;
pub mod pilot;
pub(crate) mod rc12_power_control;
pub mod rtch;

use std::{collections::BTreeMap, collections::VecDeque, sync::Arc};

use parking_lot::Mutex;

use cdma_common::{
    diagnostics::{power_control_verbose_enabled_for_walsh, power_control_verbose_summary_every},
    time::CdmaSystemTime,
};
use log::info;
use num::complex::Complex32;

use crate::{phy::spread::Spreader, phy::walsh::WalshGenerator};

/// Forward-channel sample source.
///
/// Implementations emit complex chip-rate samples at 1.2288 Mcps. Higher-level
/// wrappers handle Walsh spreading, PN spreading, buffering, and mixing.
pub trait Channel {
    /// Generate the next `num_samples` chip-rate samples for `system_time`.
    fn next_block(&self, num_samples: usize, system_time: CdmaSystemTime) -> Vec<Complex32>;

    /// Append the next `num_samples` chip-rate samples into `out`.
    fn next_block_into(
        &self,
        out: &mut Vec<Complex32>,
        num_samples: usize,
        system_time: CdmaSystemTime,
    ) {
        let block = self.next_block(num_samples, system_time);
        out.extend_from_slice(&block);
    }
}

// ---------------------------------------------------------------------------
// Shared per-PCG PCB scheduler
// ---------------------------------------------------------------------------

pub type PcgPcbSchedulerHandle = Arc<Mutex<PcgPcbScheduler>>;

#[derive(Debug)]
pub struct PcgPcbScheduler {
    scheduled: BTreeMap<u64, u8>,
    missed_fallbacks: BTreeMap<u64, PcgFallbackCause>,
    last_read_abs_pcg: Option<u64>,
    walsh_code: Option<u8>,
    label: Option<String>,
    fallback_mode: PcgPcbFallbackMode,
    verbose_counters: Option<PcgPcbVerboseCounters>,
}

#[derive(Debug, Clone, Copy)]
pub enum PcgPcbFallbackMode {
    Up,
    Down,
    AlternatingHold,
    UpBeforeFirstThenHold,
}

#[derive(Debug, Clone, Copy)]
enum PcgFallbackCause {
    Empty,
    BeforeFirstScheduled,
    GapWithFuture,
    RanDry,
}

#[derive(Debug)]
struct PcgPcbVerboseCounters {
    total_emits: u64,
    window_emits: u64,
    window_scheduled: u64,
    window_fallback: u64,
    window_up: u64,
    window_down: u64,
    window_late_schedule: u64,
    window_late_fill: u64,
    window_max_lag_pcgs: u64,
    window_fallback_empty: u64,
    window_fallback_before_first: u64,
    window_fallback_gap: u64,
    window_fallback_ran_dry: u64,
    window_fallback_gap_sum_pcgs: u64,
    window_fallback_gap_max_pcgs: u64,
    last_abs_pcg: u64,
}

impl PcgPcbVerboseCounters {
    fn new() -> Self {
        Self {
            total_emits: 0,
            window_emits: 0,
            window_scheduled: 0,
            window_fallback: 0,
            window_up: 0,
            window_down: 0,
            window_late_schedule: 0,
            window_late_fill: 0,
            window_max_lag_pcgs: 0,
            window_fallback_empty: 0,
            window_fallback_before_first: 0,
            window_fallback_gap: 0,
            window_fallback_ran_dry: 0,
            window_fallback_gap_sum_pcgs: 0,
            window_fallback_gap_max_pcgs: 0,
            last_abs_pcg: 0,
        }
    }

    fn record_schedule_late(&mut self, lag_pcgs: u64) {
        self.window_late_schedule = self.window_late_schedule.saturating_add(1);
        self.window_max_lag_pcgs = self.window_max_lag_pcgs.max(lag_pcgs);
    }

    fn record_late_fill(&mut self) {
        self.window_late_fill = self.window_late_fill.saturating_add(1);
    }

    fn record_fallback_cause(&mut self, cause: PcgFallbackCause, gap_pcgs: Option<u64>) {
        match cause {
            PcgFallbackCause::Empty => {
                self.window_fallback_empty = self.window_fallback_empty.saturating_add(1);
            }
            PcgFallbackCause::BeforeFirstScheduled => {
                self.window_fallback_before_first =
                    self.window_fallback_before_first.saturating_add(1);
            }
            PcgFallbackCause::GapWithFuture => {
                self.window_fallback_gap = self.window_fallback_gap.saturating_add(1);
            }
            PcgFallbackCause::RanDry => {
                self.window_fallback_ran_dry = self.window_fallback_ran_dry.saturating_add(1);
            }
        }
        if let Some(gap_pcgs) = gap_pcgs {
            self.window_fallback_gap_sum_pcgs =
                self.window_fallback_gap_sum_pcgs.saturating_add(gap_pcgs);
            self.window_fallback_gap_max_pcgs = self.window_fallback_gap_max_pcgs.max(gap_pcgs);
        }
    }

    fn record_emit(&mut self, abs_pcg: u64, scheduled: bool, bit: u8) -> bool {
        self.total_emits = self.total_emits.saturating_add(1);
        self.window_emits = self.window_emits.saturating_add(1);
        if scheduled {
            self.window_scheduled = self.window_scheduled.saturating_add(1);
        } else {
            self.window_fallback = self.window_fallback.saturating_add(1);
        }
        if bit == 0 {
            self.window_up = self.window_up.saturating_add(1);
        } else {
            self.window_down = self.window_down.saturating_add(1);
        }
        self.last_abs_pcg = abs_pcg;
        self.window_emits >= power_control_verbose_summary_every()
    }

    fn log_and_reset(&mut self, label: &str, walsh_code: u8, pending: usize) {
        if self.window_emits == 0 && self.window_late_schedule == 0 {
            return;
        }
        let fallback_gap_count = self.window_fallback_before_first + self.window_fallback_gap;
        let fallback_gap_avg_pcgs = if fallback_gap_count > 0 {
            self.window_fallback_gap_sum_pcgs as f64 / fallback_gap_count as f64
        } else {
            0.0
        };
        info!(
            "bts_tx: [power counters {} w{}] total_emits={} window_emits={} scheduled={} fallback={} up={} down={} late_schedule={} late_fill={} max_lag_pcgs={} fallback_empty={} fallback_before_first={} fallback_gap={} fallback_ran_dry={} fallback_gap_avg_pcgs={:.2} fallback_gap_max_pcgs={} pending={} last_abs_pcg={}",
            label,
            walsh_code,
            self.total_emits,
            self.window_emits,
            self.window_scheduled,
            self.window_fallback,
            self.window_up,
            self.window_down,
            self.window_late_schedule,
            self.window_late_fill,
            self.window_max_lag_pcgs,
            self.window_fallback_empty,
            self.window_fallback_before_first,
            self.window_fallback_gap,
            self.window_fallback_ran_dry,
            fallback_gap_avg_pcgs,
            self.window_fallback_gap_max_pcgs,
            pending,
            self.last_abs_pcg,
        );
        self.window_emits = 0;
        self.window_scheduled = 0;
        self.window_fallback = 0;
        self.window_up = 0;
        self.window_down = 0;
        self.window_late_schedule = 0;
        self.window_late_fill = 0;
        self.window_max_lag_pcgs = 0;
        self.window_fallback_empty = 0;
        self.window_fallback_before_first = 0;
        self.window_fallback_gap = 0;
        self.window_fallback_ran_dry = 0;
        self.window_fallback_gap_sum_pcgs = 0;
        self.window_fallback_gap_max_pcgs = 0;
    }
}

impl PcgPcbScheduler {
    pub fn new(_fallback_seed: u8) -> PcgPcbSchedulerHandle {
        Arc::new(Mutex::new(Self {
            scheduled: BTreeMap::new(),
            missed_fallbacks: BTreeMap::new(),
            last_read_abs_pcg: None,
            walsh_code: None,
            label: None,
            fallback_mode: PcgPcbFallbackMode::Up,
            verbose_counters: None,
        }))
    }

    pub fn new_named(
        _fallback_seed: u8,
        walsh_code: u8,
        label: impl Into<String>,
    ) -> PcgPcbSchedulerHandle {
        Self::new_named_with_fallback(_fallback_seed, walsh_code, label, PcgPcbFallbackMode::Up)
    }

    pub fn new_named_with_fallback(
        _fallback_seed: u8,
        walsh_code: u8,
        label: impl Into<String>,
        fallback_mode: PcgPcbFallbackMode,
    ) -> PcgPcbSchedulerHandle {
        Arc::new(Mutex::new(Self {
            scheduled: BTreeMap::new(),
            missed_fallbacks: BTreeMap::new(),
            last_read_abs_pcg: None,
            walsh_code: Some(walsh_code),
            label: Some(label.into()),
            fallback_mode,
            verbose_counters: power_control_verbose_enabled_for_walsh(walsh_code)
                .then(PcgPcbVerboseCounters::new),
        }))
    }

    pub fn schedule(&mut self, abs_pcg: u64, bit: u8) -> bool {
        let last_read_abs_pcg = self.last_read_abs_pcg;
        self.scheduled.insert(abs_pcg, bit & 1);
        self.trim_before(abs_pcg.saturating_sub(64));
        self.trim_missed_before(abs_pcg.saturating_sub(64));
        if self.missed_fallbacks.remove(&abs_pcg).is_some()
            && let Some(counters) = self.verbose_counters.as_mut()
        {
            counters.record_late_fill();
        }
        if let Some(last_read_abs_pcg) = last_read_abs_pcg
            && abs_pcg <= last_read_abs_pcg
            && let Some(counters) = self.verbose_counters.as_mut()
        {
            counters.record_schedule_late(last_read_abs_pcg.saturating_sub(abs_pcg));
        }
        last_read_abs_pcg.is_none_or(|last_read| abs_pcg > last_read)
    }

    pub fn schedule_burst(&mut self, start_abs_pcg: u64, pcgs: u64, bit: u8) {
        let last_read_abs_pcg = self.last_read_abs_pcg;
        let bit = bit & 1;
        for offset in 0..pcgs {
            let abs_pcg = start_abs_pcg.saturating_add(offset);
            self.scheduled.insert(abs_pcg, bit);
            if self.missed_fallbacks.remove(&abs_pcg).is_some()
                && let Some(counters) = self.verbose_counters.as_mut()
            {
                counters.record_late_fill();
            }
            if let Some(last_read_abs_pcg) = last_read_abs_pcg
                && abs_pcg <= last_read_abs_pcg
                && let Some(counters) = self.verbose_counters.as_mut()
            {
                counters.record_schedule_late(last_read_abs_pcg.saturating_sub(abs_pcg));
            }
        }
        self.trim_before(start_abs_pcg.saturating_sub(64));
        self.trim_missed_before(start_abs_pcg.saturating_sub(64));
    }

    pub fn read(&mut self, abs_pcg: u64) -> u8 {
        self.last_read_abs_pcg = Some(abs_pcg);
        self.trim_before(abs_pcg.saturating_sub(64));
        self.trim_missed_before(abs_pcg.saturating_sub(64));
        let scheduled = self.scheduled.get(&abs_pcg).copied();
        let fallback_cause = if scheduled.is_none() {
            let (cause, gap_pcgs) = self.classify_fallback(abs_pcg);
            self.missed_fallbacks.insert(abs_pcg, cause);
            if let Some(counters) = self.verbose_counters.as_mut() {
                counters.record_fallback_cause(cause, gap_pcgs);
            }
            Some(cause)
        } else {
            None
        };
        let bit = scheduled.unwrap_or_else(|| {
            self.fallback_for(
                abs_pcg,
                fallback_cause.expect("fallback cause exists when schedule is absent"),
            )
        });
        if let (Some(counters), Some(walsh_code), Some(label)) = (
            self.verbose_counters.as_mut(),
            self.walsh_code,
            self.label.as_deref(),
        ) && counters.record_emit(abs_pcg, scheduled.is_some(), bit)
        {
            counters.log_and_reset(label, walsh_code, self.scheduled.len());
        }
        bit
    }

    fn fallback_for(&self, abs_pcg: u64, cause: PcgFallbackCause) -> u8 {
        match self.fallback_mode {
            // Always command "up" (0) on unscheduled PCGs so the MS doesn't
            // drop power during scheduling gaps. A slight upward creep is
            // preferable to losing the reverse link.
            PcgPcbFallbackMode::Up => 0,
            // Conservative fallback until the inner loop schedules commands.
            PcgPcbFallbackMode::Down => 1,
            // Alternating UP/DOWN is the closest representable HOLD command.
            PcgPcbFallbackMode::AlternatingHold => (abs_pcg as u8) & 1,
            PcgPcbFallbackMode::UpBeforeFirstThenHold => match cause {
                PcgFallbackCause::Empty | PcgFallbackCause::BeforeFirstScheduled => 0,
                PcgFallbackCause::GapWithFuture | PcgFallbackCause::RanDry => (abs_pcg as u8) & 1,
            },
        }
    }

    fn trim_before(&mut self, min_abs_pcg: u64) {
        self.scheduled.retain(|abs_pcg, _| *abs_pcg >= min_abs_pcg);
    }

    fn trim_missed_before(&mut self, min_abs_pcg: u64) {
        self.missed_fallbacks
            .retain(|abs_pcg, _| *abs_pcg >= min_abs_pcg);
    }

    fn classify_fallback(&self, abs_pcg: u64) -> (PcgFallbackCause, Option<u64>) {
        let prev = self.scheduled.range(..abs_pcg).next_back().map(|(k, _)| *k);
        let next = self
            .scheduled
            .range(abs_pcg.saturating_add(1)..)
            .next()
            .map(|(k, _)| *k);
        match (prev, next) {
            (None, None) => (PcgFallbackCause::Empty, None),
            (None, Some(next_abs_pcg)) => (
                PcgFallbackCause::BeforeFirstScheduled,
                Some(next_abs_pcg.saturating_sub(abs_pcg)),
            ),
            (Some(_), Some(next_abs_pcg)) => (
                PcgFallbackCause::GapWithFuture,
                Some(next_abs_pcg.saturating_sub(abs_pcg)),
            ),
            (Some(_), None) => (PcgFallbackCause::RanDry, None),
        }
    }
}

#[cfg(test)]
mod pcb_scheduler_tests {
    use super::*;

    #[test]
    fn default_fallback_commands_up() {
        let scheduler = PcgPcbScheduler::new(0);
        let mut scheduler = scheduler.lock();

        assert_eq!(scheduler.read(10), 0);
        assert_eq!(scheduler.read(11), 0);
    }

    #[test]
    fn alternating_fallback_holds_on_unscheduled_pcgs() {
        let scheduler = PcgPcbScheduler::new_named_with_fallback(
            0,
            11,
            "rc3-test",
            PcgPcbFallbackMode::AlternatingHold,
        );
        let mut scheduler = scheduler.lock();

        assert_eq!(scheduler.read(10), 0);
        assert_eq!(scheduler.read(11), 1);
        assert_eq!(scheduler.read(12), 0);
    }

    #[test]
    fn up_before_first_then_hold_fallback_commands_up_until_first_schedule() {
        let scheduler = PcgPcbScheduler::new_named_with_fallback(
            0,
            11,
            "rc3-test",
            PcgPcbFallbackMode::UpBeforeFirstThenHold,
        );
        let mut scheduler = scheduler.lock();

        assert_eq!(scheduler.read(8), 0);
        assert_eq!(scheduler.read(9), 0);
        assert_eq!(scheduler.read(65), 0);
        assert_eq!(scheduler.read(67), 0);
        scheduler.schedule(70, 0);
        assert_eq!(scheduler.read(68), 0);
        assert_eq!(scheduler.read(69), 0);
        assert_eq!(scheduler.read(70), 0);
        assert_eq!(scheduler.read(71), 1);
        assert_eq!(scheduler.read(72), 0);
    }

    #[test]
    fn scheduled_burst_keeps_all_future_pcgs() {
        let scheduler = PcgPcbScheduler::new_named_with_fallback(
            0,
            11,
            "rc3-test",
            PcgPcbFallbackMode::AlternatingHold,
        );
        let mut scheduler = scheduler.lock();

        scheduler.schedule_burst(100, 160, 1);

        for abs_pcg in 100..260 {
            assert_eq!(scheduler.read(abs_pcg), 1, "abs_pcg={abs_pcg}");
        }
    }
}

// ---------------------------------------------------------------------------
// Walsh-only channel wrapper (no PN spreading)
// ---------------------------------------------------------------------------

pub type WalshChannelWrapper<T> = Arc<WalshChannel<T>>;

pub struct WalshChannel<T>
where
    T: Channel,
{
    pub channel: T,
    state: Mutex<WalshState>,
}

struct WalshState {
    walsh: WalshGenerator,
    buffer: VecDeque<Complex32>,
    activity_segments: VecDeque<WalshActivitySegment>,
    symbol_scratch: Vec<Complex32>,
}

struct WalshActivitySegment {
    remaining: usize,
    active: bool,
}

impl WalshState {
    fn push_activity(&mut self, count: usize, active: bool) {
        if count == 0 {
            return;
        }
        if let Some(last) = self.activity_segments.back_mut()
            && last.active == active
        {
            last.remaining += count;
            return;
        }
        self.activity_segments.push_back(WalshActivitySegment {
            remaining: count,
            active,
        });
    }

    fn consume_activity(&mut self, mut count: usize) -> bool {
        let mut active = false;
        while count != 0 {
            let segment = self
                .activity_segments
                .front_mut()
                .expect("Walsh activity must track buffered chips");
            let consumed = count.min(segment.remaining);
            active |= segment.active;
            segment.remaining -= consumed;
            count -= consumed;
            if segment.remaining == 0 {
                self.activity_segments.pop_front();
            }
        }
        active
    }
}

impl<T> WalshChannel<T>
where
    T: Channel,
{
    pub fn new(walsh: WalshGenerator, channel: T) -> WalshChannelWrapper<T> {
        Arc::new(WalshChannel {
            channel,
            state: Mutex::new(WalshState {
                walsh,
                buffer: VecDeque::new(),
                activity_segments: VecDeque::new(),
                symbol_scratch: Vec::new(),
            }),
        })
    }

    /// Pre-fill the internal buffer with `n` zero-valued chips.
    ///
    /// Used to align the first real frame output with a 20ms frame boundary.
    /// The silence chips are drained before any actual frame content, so the
    /// first `ForwardTrafficChannel::next()` call happens exactly when the
    /// buffer runs empty — at the frame boundary.
    pub fn prefill_silence(&self, n: usize) {
        let mut state = self.state.lock();
        for _ in 0..n {
            state.buffer.push_back(Complex32::new(0.0, 0.0));
        }
        state.push_activity(n, false);
    }
}

impl<T> Channel for WalshChannel<T>
where
    T: Channel,
{
    fn next_block(&self, num_samples: usize, system_time: CdmaSystemTime) -> Vec<Complex32> {
        let mut out = Vec::with_capacity(num_samples);
        self.next_block_into(&mut out, num_samples, system_time);
        out
    }

    fn next_block_into(
        &self,
        out: &mut Vec<Complex32>,
        num_samples: usize,
        system_time: CdmaSystemTime,
    ) {
        let mut state = self.state.lock();
        if state.buffer.len() < num_samples {
            let chips_per_symbol = state.walsh.chips_per_symbol();
            let deficit = num_samples - state.buffer.len();
            let symbols_needed = deficit.div_ceil(chips_per_symbol);
            let state = &mut *state;
            state.symbol_scratch.clear();
            self.channel
                .next_block_into(&mut state.symbol_scratch, symbols_needed, system_time);
            let generated_chips = state.symbol_scratch.len() * chips_per_symbol;
            for sample in &state.symbol_scratch {
                for _ in 0..state.walsh.repetition() {
                    for c in state.walsh.code() {
                        state.buffer.push_back(Complex32::new(
                            *c as f32 * sample.re,
                            *c as f32 * sample.im,
                        ));
                    }
                }
            }
            // Other traffic channels transmit continuously.
            state.push_activity(generated_chips, true);
            debug_assert!(state.buffer.len() >= num_samples);
        }
        out.reserve(num_samples);
        for _ in 0..num_samples {
            out.push(state.buffer.pop_front().unwrap());
        }
        let _ = state.consume_activity(num_samples);
    }
}

impl WalshChannel<crate::channels::f_sch_rc3::ForwardSupplementalChannelRc3> {
    /// Walsh-spreads F-SCH symbols while preserving frame activity metadata.
    pub fn next_block_into_with_activity(
        &self,
        out: &mut Vec<Complex32>,
        num_samples: usize,
        system_time: CdmaSystemTime,
    ) -> bool {
        let mut state = self.state.lock();
        if state.buffer.len() < num_samples {
            let chips_per_symbol = state.walsh.chips_per_symbol();
            let deficit = num_samples - state.buffer.len();
            let symbols_needed = deficit.div_ceil(chips_per_symbol);
            let state = &mut *state;
            state.symbol_scratch.clear();
            let symbols_active = self.channel.next_block_into_with_activity(
                &mut state.symbol_scratch,
                symbols_needed,
                system_time,
            );
            let generated_chips = state.symbol_scratch.len() * chips_per_symbol;
            for sample in &state.symbol_scratch {
                for _ in 0..state.walsh.repetition() {
                    for c in state.walsh.code() {
                        state.buffer.push_back(Complex32::new(
                            *c as f32 * sample.re,
                            *c as f32 * sample.im,
                        ));
                    }
                }
            }
            state.push_activity(generated_chips, symbols_active);
            debug_assert!(state.buffer.len() >= num_samples);
        }

        out.reserve(num_samples);
        for _ in 0..num_samples {
            out.push(state.buffer.pop_front().unwrap());
        }
        state.consume_activity(num_samples)
    }
}

// ---------------------------------------------------------------------------
// Legacy Walsh + per-channel PN spread wrapper (kept for existing tests)
// ---------------------------------------------------------------------------

pub type SpreadChannelWrapper<T> = Arc<WalshAndSpreadChannel<T>>;

pub struct WalshAndSpreadChannel<T>
where
    T: Channel,
{
    pub channel: T,
    state: Mutex<State>,
}

struct State {
    walsh: WalshGenerator,
    spreader: Spreader,
    buffer: VecDeque<Complex32>,
}

impl<T> WalshAndSpreadChannel<T>
where
    T: Channel,
{
    pub fn new(walsh: WalshGenerator, spreader: Spreader, channel: T) -> SpreadChannelWrapper<T> {
        Arc::new(WalshAndSpreadChannel {
            channel,
            state: Mutex::new(State {
                walsh,
                spreader,
                buffer: VecDeque::new(),
            }),
        })
    }

    /// Align this channel's short-code spreader phase to an absolute chip
    /// position since CDMA epoch.
    ///
    /// Must be called before any output is generated.
    pub fn align_short_code_to_chip(&self, chip: u64) {
        let mut state = self.state.lock();
        state.spreader.align_to_chip(chip);
        state.buffer.clear();
    }
}

impl<T> Channel for WalshAndSpreadChannel<T>
where
    T: Channel,
{
    fn next_block(&self, num_samples: usize, system_time: CdmaSystemTime) -> Vec<Complex32> {
        let mut state = self.state.lock();

        while state.buffer.len() < num_samples {
            let next_block = self.channel.next_block(1, system_time);
            let walsh_encoded = state.walsh.feed_many(&next_block);
            let spread = state.spreader.spread_many(&walsh_encoded);
            state.buffer.extend(spread);
        }

        state.buffer.drain(0..num_samples).collect::<Vec<_>>()
    }
}
