//! # GenericRakeReceiver
//!
//! A generic RAKE receiver that separates correlation/acquisition from
//! finger lifecycle management.  Concrete signal types plug in via the
//! [`Correlator`] and [`RakeFinger`] traits; the receiver handles:
//!
//! * **Spawning** fingers from correlator detections
//! * **Feeding** every active finger each input block
//! * **Collecting** and forwarding all finger output downstream
//! * **Validating** fingers by inspecting their output block tags
//! * **Pruning** dead fingers according to a configurable policy
//!
//! ## Lifecycle
//!
//! ```text
//!  Input block
//!       │
//!       ├──► Correlator::correlate()  ──► new (Finger, Chain) pairs
//!       │                                        │
//!       │                               spawn_fingers()
//!       │                                        │
//!       ├──► feed_fingers()  ◄──── active fingers ◄──── spawned fingers
//!       │         │
//!       │    per finger:
//!       │      RakeFinger::process(block, chain)
//!       │         │
//!       │    output blocks ──► validate_from_output() ──► set hard_validated
//!       │         │
//!       └──► prune_fingers()  ──► drop idle / expired fingers
//!
//!  Output: all blocks from all fingers, passed downstream
//! ```
//!
//! ## Validation tags
//!
//! Fingers self-validate by scanning the `SampleBlock` tags produced by their
//! internal chains:
//!
//! | Tag                    | Meaning                               |
//! |------------------------|---------------------------------------|
//! | `access_crc_valid`     | Hard validation (CRC-clean frame)     |
//! | `access_preamble_detected` | Soft validation (preamble seen)   |
//!
//! Only `access_crc_valid` upgrades a finger to *hard-validated*, which
//! exempts it from pruning indefinitely.

use std::{
    collections::VecDeque,
    sync::{Arc, mpsc},
    thread::{self, JoinHandle},
    time::Instant,
};

use log::{debug, info};

use crate::receiver::pipelined::{
    PipelineProcessor, PipelineProcessorShared, SampleBlock, VecEmitter, flush_sub_chain,
};

// ---------------------------------------------------------------------------
// RakeFinger trait
// ---------------------------------------------------------------------------

/// An active receiver path in a RAKE receiver.
///
/// Implementors are responsible for:
/// - All despreading (PN, LC, timing alignment)
/// - Driving their internal signal-processing chain
/// - Tracking their own validation state via [`BaseFinger`]
pub trait RakeFinger: Send {
    /// Unique identifier for logging and duplicate-suppression.
    fn id(&self) -> u64;

    /// Absolute chip where this finger was spawned, when known.
    fn spawn_chip_start(&self) -> Option<u64> {
        None
    }

    /// Process one input block.
    ///
    /// The implementation should despread the raw IQ in `block`, push the
    /// resulting chips/symbols into `chain`, and return any blocks that
    /// emerge from the end of `chain`.
    fn process(
        &mut self,
        block: &SampleBlock,
        chain: &mut Vec<PipelineProcessorShared>,
    ) -> Vec<SampleBlock>;

    /// Flush any buffered data at end-of-stream.
    ///
    /// Implementations should call `flush_sub_chain(chain)` and return the
    /// result, plus any locally buffered output.
    fn flush(&mut self, chain: &mut Vec<PipelineProcessorShared>) -> Vec<SampleBlock>;

    /// `true` once hard validation (e.g. CRC-clean frame) has been observed.
    fn is_hard_validated(&self) -> bool;

    /// A short human-readable description of this finger's signal parameters.
    ///
    /// Used in log messages when the finger is spawned.  Implementors should
    /// include timing and modulation details (e.g. delay, LC phase, despread
    /// phase).  The default returns an empty string.
    fn describe(&self) -> String {
        String::new()
    }

    /// Number of input blocks processed since last hard validation (or since
    /// last observed burst activity).
    fn idle_blocks(&self) -> u64;

    /// Number of chips processed since last hard validation (or since last
    /// observed burst activity).
    fn idle_chips(&self) -> u64 {
        self.idle_blocks()
    }

    /// Number of CRC-false access events seen since the last CRC-clean frame.
    fn crc_miss_count(&self) -> u64 {
        0
    }

    /// Number of chips processed since the most recent Walsh lock without a
    /// CRC-clean access frame.
    fn post_walsh_no_event_chips(&self) -> u64 {
        0
    }

    /// Number of CRC-false access events seen since the most recent Walsh
    /// lock but before the next CRC-clean frame.
    fn post_walsh_miss_count(&self) -> u64 {
        0
    }

    /// Wall-clock milliseconds since the most recent Walsh lock without a
    /// CRC-clean access frame.
    fn post_walsh_no_event_ms(&self) -> u64 {
        0
    }

    /// Consecutive chips where despread incoherent energy has been below the
    /// signal-loss threshold.  Returns 0 by default (no tracking).
    fn signal_lost_chips(&self) -> u64 {
        0
    }

    /// Print internal timing breakdown (optional).
    fn print_timing(&self) {}

    /// Return internal timing breakdown lines for structured logging
    /// (optional). Used by periodic GenericRakeReceiver timing reports.
    fn timing_report_lines(&self) -> Vec<String> {
        Vec::new()
    }
}

// ---------------------------------------------------------------------------
// BaseFinger — shared boilerplate for concrete finger implementations
// ---------------------------------------------------------------------------

/// Common finger state.  Embed this in a concrete finger struct and delegate
/// the [`RakeFinger`] bookkeeping methods to it.
///
/// ```rust,ignore
/// struct MyFinger {
///     base: BaseFinger,
///     // ... signal-specific fields ...
/// }
/// impl RakeFinger for MyFinger {
///     fn id(&self) -> u64 { self.base.id }
///     fn is_hard_validated(&self) -> bool { self.base.is_hard_validated() }
///     fn idle_blocks(&self) -> u64 { self.base.idle_blocks() }
///     fn idle_chips(&self) -> u64 { self.base.idle_chips() }
///     fn post_walsh_no_event_chips(&self) -> u64 { self.base.post_walsh_no_event_chips() }
///     fn post_walsh_miss_count(&self) -> u64 { self.base.post_walsh_miss_count() }
///     fn flush(&mut self, chain: &mut Vec<PipelineProcessorShared>) -> Vec<SampleBlock> {
///         self.base.flush_chain(chain)
///     }
///     fn process(&mut self, block: &SampleBlock, chain: &mut Vec<PipelineProcessorShared>)
///         -> Vec<SampleBlock>
///     {
///         // ... despread block, push chips to chain ...
///         let out = /* run chain */;
///         self.base.tick_and_validate(&out, (block.samples.len() / 4) as u64);
///         out
///     }
/// }
/// ```
#[derive(Default, Clone, Copy, Debug)]
pub struct FingerProgress {
    pub saw_activity: bool,
    pub saw_completed_event: bool,
    pub saw_crc_valid: bool,
    pub crc_misses: u64,
    pub saw_walsh_lock: bool,
    pub saw_frame_lock: bool,
    pub saw_access_completed_event: bool,
    pub saw_traffic_completed_event: bool,
    pub saw_traffic_phy_activity: bool,
    pub saw_access_preamble_activity: bool,
}

impl FingerProgress {
    pub fn observe_blocks(&mut self, output: &[SampleBlock]) {
        for blk in output {
            // Accept both access and traffic activity tags for finger health.
            let saw_access_completed_event =
                blk.tags.get("access_event").copied().unwrap_or(0) != 0;
            let saw_traffic_completed_event = blk.tags.get("traffic_event").copied().unwrap_or(0)
                != 0
                || blk.tags.get("finger_event").copied().unwrap_or(0) != 0;
            let saw_completed_event = saw_access_completed_event || saw_traffic_completed_event;
            let saw_traffic_phy_activity = blk.tags.get("traffic_phy_frame").copied().unwrap_or(0)
                != 0
                || blk.tags.get("traffic_phy_status").copied().unwrap_or(0) != 0;
            let saw_activity = saw_completed_event || saw_traffic_phy_activity;
            if saw_activity {
                self.saw_activity = true;
            }
            if saw_completed_event {
                self.saw_completed_event = true;
            }
            let saw_access_preamble_activity = blk.tags.contains_key("access_preamble_frames")
                || blk
                    .tags
                    .get("access_preamble_detected")
                    .copied()
                    .unwrap_or(0)
                    != 0;
            if saw_access_completed_event {
                self.saw_access_completed_event = true;
            }
            if saw_traffic_completed_event {
                self.saw_traffic_completed_event = true;
            }
            if saw_traffic_phy_activity {
                self.saw_traffic_phy_activity = true;
            }
            if saw_access_preamble_activity {
                self.saw_access_preamble_activity = true;
            }
            let crc_valid = blk.tags.get("access_crc_valid").copied().unwrap_or(0) != 0
                || blk.tags.get("traffic_crc_valid").copied().unwrap_or(0) != 0
                || blk.tags.get("finger_crc_valid").copied().unwrap_or(0) != 0;
            if crc_valid {
                self.saw_crc_valid = true;
            } else if saw_completed_event {
                self.crc_misses = self.crc_misses.saturating_add(1);
            }
            if blk.tags.get("access_walsh_locked").copied().unwrap_or(0) != 0
                || blk.tags.get("traffic_walsh_locked").copied().unwrap_or(0) != 0
            {
                self.saw_walsh_lock = true;
            }
            if blk.tags.get("access_frame_aligned").copied().unwrap_or(0) != 0
                || blk.tags.get("traffic_frame_aligned").copied().unwrap_or(0) != 0
            {
                self.saw_frame_lock = true;
            }
        }
    }
}

pub struct BaseFinger {
    pub id: u64,
    hard_validated: bool,
    idle_block_count: u64,
    idle_chip_count: u64,
    crc_miss_count: u64,
    walsh_locked: bool,
    post_walsh_no_event_chip_count: u64,
    post_walsh_miss_count: u64,
    walsh_lock_started_at: Option<Instant>,
}

impl BaseFinger {
    pub fn new(id: u64) -> Self {
        Self {
            id,
            hard_validated: false,
            idle_block_count: 0,
            idle_chip_count: 0,
            crc_miss_count: 0,
            walsh_locked: false,
            post_walsh_no_event_chip_count: 0,
            post_walsh_miss_count: 0,
            walsh_lock_started_at: None,
        }
    }

    /// Call once per processed input block.  Scans `output` for validation
    /// tags and updates burst-local health counters.
    pub fn tick_and_validate(&mut self, output: &[SampleBlock], processed_chips: u64) {
        let mut progress = FingerProgress::default();
        progress.observe_blocks(output);
        self.tick_with_progress(&progress, processed_chips);
    }

    pub fn tick_with_progress(&mut self, progress: &FingerProgress, processed_chips: u64) {
        let now = Instant::now();
        if progress.saw_walsh_lock {
            self.walsh_locked = true;
            self.post_walsh_no_event_chip_count = 0;
            self.post_walsh_miss_count = 0;
            self.walsh_lock_started_at = Some(now);
        }

        if progress.saw_crc_valid {
            if !self.hard_validated {
                debug!("BaseFinger {}: hard validation (CRC clean)", self.id);
            }
            self.hard_validated = true;
            self.idle_block_count = 0;
            self.idle_chip_count = 0;
            self.crc_miss_count = 0;
            self.post_walsh_no_event_chip_count = 0;
            self.post_walsh_miss_count = 0;
            self.walsh_lock_started_at = None;
            return;
        }

        let keep_alive_activity = progress.saw_crc_valid
            || progress.saw_traffic_completed_event
            || progress.saw_traffic_phy_activity;

        if keep_alive_activity {
            self.idle_block_count = 0;
            self.idle_chip_count = 0;
        } else {
            self.idle_block_count += 1;
            self.idle_chip_count = self.idle_chip_count.saturating_add(processed_chips);
        }

        let completed_event_for_post_walsh = if self.hard_validated {
            progress.saw_crc_valid || progress.saw_traffic_completed_event
        } else {
            progress.saw_completed_event
        };

        // Post-walsh "no completed event" timer: only reset on a completed
        // event (decoded frame), not mere preamble activity.
        if completed_event_for_post_walsh && self.walsh_locked {
            self.post_walsh_no_event_chip_count = 0;
            self.post_walsh_miss_count = 0;
        }

        if self.walsh_locked
            && self.walsh_lock_started_at.is_some()
            && !completed_event_for_post_walsh
        {
            self.post_walsh_no_event_chip_count = self
                .post_walsh_no_event_chip_count
                .saturating_add(processed_chips);
            self.post_walsh_miss_count = self
                .post_walsh_miss_count
                .saturating_add(progress.crc_misses);
        }

        if self.hard_validated {
            self.crc_miss_count = self.crc_miss_count.saturating_add(progress.crc_misses);
        }
    }

    pub fn is_hard_validated(&self) -> bool {
        self.hard_validated
    }

    pub fn idle_blocks(&self) -> u64 {
        self.idle_block_count
    }

    pub fn idle_chips(&self) -> u64 {
        self.idle_chip_count
    }

    pub fn crc_miss_count(&self) -> u64 {
        self.crc_miss_count
    }

    pub fn post_walsh_no_event_chips(&self) -> u64 {
        self.post_walsh_no_event_chip_count
    }

    pub fn post_walsh_miss_count(&self) -> u64 {
        self.post_walsh_miss_count
    }

    pub fn post_walsh_no_event_ms(&self) -> u64 {
        self.walsh_lock_started_at
            .map(|started| started.elapsed().as_millis().try_into().unwrap_or(u64::MAX))
            .unwrap_or(0)
    }

    /// Convenience: flush the sub-chain in the standard way.
    pub fn flush_chain(chain: &mut Vec<PipelineProcessorShared>) -> Vec<SampleBlock> {
        let mut emitter = VecEmitter::new();
        let mut out = flush_sub_chain(chain, &mut emitter);
        out.extend(emitter.blocks);
        out
    }
}

// ---------------------------------------------------------------------------
// Correlator trait
// ---------------------------------------------------------------------------

/// Searches a stream of [`SampleBlock`]s for signals and spawns new fingers.
///
/// The correlator is responsible for:
/// - Maintaining its own accumulation buffers (e.g. noncoherent PN maps)
/// - Returning zero or more (finger, chain) pairs per block
/// - Not producing duplicate fingers for the same signal path
pub trait Correlator: Send {
    /// The concrete finger type produced by this correlator.
    type Finger: RakeFinger + 'static;

    /// Examine `block` and return any newly acquired (finger, chain) pairs.
    ///
    /// The returned `chain` is the sub-pipeline that will process chips
    /// output by the finger (Walsh demod → Viterbi → etc.).
    fn correlate(
        &mut self,
        block: &SampleBlock,
    ) -> Vec<(Self::Finger, Vec<PipelineProcessorShared>)>;

    /// Notify the correlator that a finger has been hard-validated (e.g.
    /// CRC-clean frame received).  Implementations may use this to pause
    /// or reduce search activity.
    fn notify_hard_validated(&mut self, _finger_id: u64) {}

    /// Returns true if the correlator has suppressed its search (e.g.
    /// because a finger is hard-validated and `suppress_search_when_locked`
    /// is enabled).  Used by the rake receiver to prune redundant sibling
    /// fingers after validation on traffic channels.
    fn search_suppressed(&self) -> bool {
        false
    }

    /// Notify the correlator about the current state of an active finger.
    ///
    /// Correlators can use this to relax duplicate suppression when a
    /// previously validated finger has clearly gone quiet and a new burst
    /// appears on the same path.
    fn notify_finger_state(
        &mut self,
        _finger_id: u64,
        _hard_validated: bool,
        _idle_chips: u64,
        _signal_lost_chips: u64,
        _crc_miss_count: u64,
        _post_walsh_no_event_ms: u64,
    ) {
    }

    /// Notify the correlator that a finger was retired so any duplicate
    /// suppression tied to that finger can be released.
    fn notify_finger_removed(&mut self, _finger_id: u64) {}
}

// ---------------------------------------------------------------------------
// PrunePolicy
// ---------------------------------------------------------------------------

/// Decides whether an idle finger should be removed from the receiver.
///
/// Hard-validated fingers should almost always be kept regardless of this
/// policy; the default implementation enforces that invariant.
pub trait PrunePolicy: Send {
    /// Return `true` if this finger should be dropped.
    fn should_prune(&self, finger: &dyn RakeFinger) -> bool;
}

/// Default policy: unvalidated fingers are pruned after `max_idle_chips`
/// consecutive idle chips; validated fingers are treated as burst-local and
/// retired after a shorter idle chip budget or too many consecutive CRC misses.
pub struct DefaultPrunePolicy {
    /// Number of idle chips before an unvalidated finger is pruned.
    pub max_idle_chips: u64,
    /// Number of idle chips before a hard-validated finger is retired.
    pub max_validated_idle_chips: u64,
    /// Maximum number of CRC-false access events tolerated after validation.
    pub max_crc_miss_count: u64,
    /// Number of chips allowed after Walsh lock without a CRC-clean access
    /// frame for an already validated finger.
    pub max_validated_post_walsh_no_event_chips: u64,
    /// Wall-clock budget after Walsh lock for an already validated finger.
    /// Disabled by default in favor of the signal-time chip budget above.
    pub max_validated_post_walsh_no_event_ms: u64,
    /// Number of chips allowed after Walsh lock without a CRC-clean access
    /// frame.
    pub max_post_walsh_no_event_chips: u64,
    /// Wall-clock budget after Walsh lock without a CRC-clean frame for an
    /// unvalidated finger.
    pub max_post_walsh_no_event_ms: u64,
    /// Maximum CRC-false access events tolerated after Walsh lock but before
    /// the next CRC-clean frame.
    pub max_post_walsh_miss_count: u64,
    /// Consecutive chips of low incoherent energy before the finger is pruned.
    /// Zero disables signal-loss pruning and lets validated fingers ride
    /// across burst gaps until the other prune budgets retire them.
    pub max_signal_lost_chips: u64,
}

impl Default for DefaultPrunePolicy {
    fn default() -> Self {
        let max_crc_miss_count = 256u64;
        let max_validated_post_walsh_no_event_chips = 1_228_800u64;
        let max_validated_post_walsh_no_event_ms = u64::MAX;
        let max_post_walsh_no_event_ms = u64::MAX;
        let max_signal_lost_chips = 0u64;
        Self {
            // Unvalidated fingers should not linger for multiple seconds once
            // a probe has faded. Give them about 500 ms of signal time to
            // finish acquisition before retiring them.
            max_idle_chips: 1_843_200,
            // After a validated reverse-access burst goes quiet, retire the
            // finger after about two seconds of signal time so stale fingers
            // do not linger for tens of seconds waiting on CRC-miss pruning.
            max_validated_idle_chips: 2_457_600,
            // Validated fingers already proved they can decode. Do not let
            // them chew through dozens of CRC-false frames after a burst has
            // gone bad; retire them sooner so fresh access probes keep RX
            // deadline headroom.
            max_crc_miss_count,
            // Once a validated finger has Walsh lock, give it about one
            // second of signal time to produce another CRC-clean frame.
            // This is stable for both live and offline replay because it is
            // based on chips, not host execution time.
            max_validated_post_walsh_no_event_chips,
            max_validated_post_walsh_no_event_ms,
            // After Walsh lock, downstream should usually produce a CRC-clean
            // access frame quickly, but long access bursts can require many
            // 20 ms fragments before frame reassembly finally closes. Keep
            // the budget long enough to search the full access reassembly
            // window before retiring an unvalidated finger.
            max_post_walsh_no_event_chips: u64::MAX,
            // This budget is wall-clock host time, not signal time. Keep it
            // generous so offline captures are not pruned just because the
            // host is busy; use the chip budget and cheaper frame search to
            // control live bad-lock cost.
            max_post_walsh_no_event_ms,
            max_post_walsh_miss_count: 4,
            // Leave signal-loss pruning disabled by default. It helps split
            // repeated bursts on some captures, but it also suppresses later
            // CRC-clean decodes on others. Keep one shared default config and
            // let the other budgets control validated-finger lifetime.
            max_signal_lost_chips,
        }
    }
}

impl PrunePolicy for DefaultPrunePolicy {
    fn should_prune(&self, finger: &dyn RakeFinger) -> bool {
        // Signal-loss pruning is only safe after hard validation. Before
        // that, some captures legitimately gap before the eventual valid
        // burst and need the normal idle / post-Walsh budgets to finish
        // acquisition.
        if finger.is_hard_validated()
            && self.max_signal_lost_chips > 0
            && finger.signal_lost_chips() > self.max_signal_lost_chips
        {
            return true;
        }

        if finger.is_hard_validated() {
            finger.idle_chips() > self.max_validated_idle_chips
                || finger.crc_miss_count() > self.max_crc_miss_count
                || finger.post_walsh_no_event_chips() > self.max_validated_post_walsh_no_event_chips
                || finger.post_walsh_no_event_ms() > self.max_validated_post_walsh_no_event_ms
                || finger.post_walsh_miss_count() > self.max_post_walsh_miss_count
        } else {
            finger.idle_chips() > self.max_idle_chips
                || finger.post_walsh_no_event_chips() > self.max_post_walsh_no_event_chips
                || finger.post_walsh_no_event_ms() > self.max_post_walsh_no_event_ms
                || finger.post_walsh_miss_count() > self.max_post_walsh_miss_count
        }
    }
}

// ---------------------------------------------------------------------------
// Internal container
// ---------------------------------------------------------------------------

struct ActiveFinger<F: RakeFinger> {
    finger: F,
    chain: Vec<PipelineProcessorShared>,
    process_ns: u64,
    process_calls: u64,
    last_report_process_ns: u64,
    last_report_process_calls: u64,
    notified_validated: bool,
}

// ---------------------------------------------------------------------------
// Parallel finger-feed pool
// ---------------------------------------------------------------------------

/// Work item sent from the main thread to a pool worker.
struct FingerWorkItem<F: RakeFinger> {
    idx: usize,
    finger: ActiveFinger<F>,
    block: Arc<SampleBlock>,
}

/// Result sent from a pool worker back to the main thread.
struct FingerWorkResult<F: RakeFinger> {
    idx: usize,
    finger: ActiveFinger<F>,
    outputs: Vec<SampleBlock>,
}

/// A persistent thread pool that processes finger feeds in parallel.
///
/// Work is distributed via a crossbeam MPMC channel (single publisher,
/// multiple subscribers).  Results flow back through a standard mpsc channel.
/// Finger ownership round-trips through the channels on every block:
/// main → worker → main.
struct FingerFeedPool<F: RakeFinger + 'static> {
    work_tx: crossbeam_channel::Sender<FingerWorkItem<F>>,
    result_rx: mpsc::Receiver<FingerWorkResult<F>>,
    workers: Vec<JoinHandle<()>>,
}

impl<F: RakeFinger + 'static> FingerFeedPool<F> {
    fn new(pool_size: usize) -> Self {
        let (work_tx, work_rx) = crossbeam_channel::unbounded::<FingerWorkItem<F>>();
        let (result_tx, result_rx) = mpsc::channel::<FingerWorkResult<F>>();

        let mut workers = Vec::with_capacity(pool_size);
        for i in 0..pool_size {
            let work_rx = work_rx.clone();
            let result_tx = result_tx.clone();
            let handle = thread::Builder::new()
                .name(format!("finger-pool-{}", i))
                .spawn(move || {
                    while let Ok(mut item) = work_rx.recv() {
                        let t = Instant::now();
                        let outputs = item
                            .finger
                            .finger
                            .process(&item.block, &mut item.finger.chain);
                        let elapsed = t.elapsed().as_nanos() as u64;
                        item.finger.process_ns += elapsed;
                        item.finger.process_calls += 1;
                        // If the main thread dropped result_rx we are shutting down.
                        let _ = result_tx.send(FingerWorkResult {
                            idx: item.idx,
                            finger: item.finger,
                            outputs,
                        });
                    }
                })
                .expect("failed to spawn finger-pool worker thread");
            workers.push(handle);
        }

        Self {
            work_tx,
            result_rx,
            workers,
        }
    }
}

impl<F: RakeFinger + 'static> Drop for FingerFeedPool<F> {
    fn drop(&mut self) {
        // Replace the sender with a disconnected one so workers exit their
        // recv loop, then join all worker threads.
        let (dead, _) = crossbeam_channel::unbounded();
        self.work_tx = dead;
        for handle in self.workers.drain(..) {
            let _ = handle.join();
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
struct AccessBurstKey {
    chip_start: i64,
    access_pd: i64,
    access_msg_type: i64,
    payload_bits: Vec<u8>,
}

impl AccessBurstKey {
    // Duplicate same-burst re-emits in our access pipeline have shown up at
    // exact event-frame starts or very small frame-start chip deltas (for
    // example 32 chips). Use the emitted event frame start, not the finger's
    // absolute_chip_start tag, because multiple fingers can decode the same
    // burst while carrying different finger-relative absolute timing tags.
    // Keep this window narrow so repeated bursts with identical payloads are
    // not merged.
    const CHIP_TOLERANCE: i64 = 64;

    fn from_block(block: &SampleBlock) -> Option<Self> {
        if block.tags.get("access_event").copied().unwrap_or(0) == 0 {
            return None;
        }
        Some(Self {
            chip_start: block.chip_start as i64,
            access_pd: block.tags.get("access_pd").copied().unwrap_or(-1),
            access_msg_type: block.tags.get("access_msg_type").copied().unwrap_or(-1),
            payload_bits: block
                .samples
                .iter()
                .map(|s| if s.re >= 0.5 { 1u8 } else { 0u8 })
                .collect(),
        })
    }

    fn matches_burst(&self, other: &Self) -> bool {
        self.access_pd == other.access_pd
            && self.access_msg_type == other.access_msg_type
            && self.payload_bits == other.payload_bits
            && (self.chip_start - other.chip_start).abs() <= Self::CHIP_TOLERANCE
    }
}

// ---------------------------------------------------------------------------
// GenericRakeReceiver
// ---------------------------------------------------------------------------

/// Generic RAKE receiver.
///
/// Wraps a [`Correlator`] and manages the lifecycle of all active fingers.
/// Implement [`Correlator`] and [`RakeFinger`] for your signal type, then
/// wrap them in a `GenericRakeReceiver` and insert it into any
/// [`PipelineProcessor`] chain.
///
/// # Example (sketch)
///
/// ```rust,ignore
/// let receiver = GenericRakeReceiver::new(MyCorrelator::new())
///     .with_max_fingers(4)
///     .with_prune_policy(Box::new(DefaultPrunePolicy {
///         max_idle_chips: 262_144,
///         ..Default::default()
///     }));
///
/// let pipeline: Vec<PipelineProcessorShared> = vec![
///     Box::new(PulseMatchedFilterProcessor::new()),
///     Box::new(receiver),
/// ];
/// ```
pub struct GenericRakeReceiver<C: Correlator> {
    correlator: C,
    fingers: Vec<ActiveFinger<C::Finger>>,
    prune_policy: Box<dyn PrunePolicy>,
    /// Hard upper bound on simultaneously active fingers.
    max_fingers: usize,
    /// Total input blocks seen (for diagnostics).
    block_count: u64,
    /// Accumulated correlator time (nanoseconds).
    correlator_ns: u64,
    /// Accumulated finger processing time (nanoseconds).
    fingers_ns: u64,
    /// Rolling timing-report interval start.
    report_interval_start: Instant,
    /// Snapshots for interval timing reports.
    last_report_block_count: u64,
    last_report_correlator_ns: u64,
    last_report_fingers_ns: u64,
    /// Recently emitted access bursts so the same decoded burst is forwarded
    /// only once even if multiple fingers or re-emit paths produce it.
    recent_access_burst_order: VecDeque<AccessBurstKey>,
    /// Thread pool for parallel finger feeding.
    finger_pool: FingerFeedPool<C::Finger>,
    emit_timing_trace: bool,
}

impl<C: Correlator> GenericRakeReceiver<C> {
    /// Default finger pool size for parallel finger feeding.
    const DEFAULT_FINGER_POOL_SIZE: usize = 8;

    /// Create a receiver with the [`DefaultPrunePolicy`] and `max_fingers = 8`.
    pub fn new(correlator: C) -> Self {
        Self {
            correlator,
            fingers: Vec::new(),
            prune_policy: Box::new(DefaultPrunePolicy::default()),
            max_fingers: 8,
            block_count: 0,
            correlator_ns: 0,
            fingers_ns: 0,
            report_interval_start: Instant::now(),
            last_report_block_count: 0,
            last_report_correlator_ns: 0,
            last_report_fingers_ns: 0,
            recent_access_burst_order: VecDeque::new(),
            finger_pool: FingerFeedPool::new(Self::DEFAULT_FINGER_POOL_SIZE),
            emit_timing_trace: std::env::var_os("CDMA_ACCESS_TIMING_TRACE").is_some(),
        }
    }

    /// Override the pruning policy.
    pub fn with_prune_policy(mut self, policy: Box<dyn PrunePolicy>) -> Self {
        self.prune_policy = policy;
        self
    }

    /// Set the maximum number of simultaneously active fingers.
    ///
    /// When at capacity, newly detected fingers are discarded until a slot
    /// opens via pruning.
    pub fn with_max_fingers(mut self, n: usize) -> Self {
        self.max_fingers = n;
        self
    }

    /// Set the finger-feed thread pool size.  Default is 8.
    pub fn with_finger_pool_size(mut self, n: usize) -> Self {
        self.finger_pool = FingerFeedPool::new(n);
        self
    }

    /// Current number of active fingers.
    pub fn finger_count(&self) -> usize {
        self.fingers.len()
    }

    /// `true` if any currently active finger has been hard-validated.
    pub fn has_hard_validated_finger(&self) -> bool {
        self.fingers.iter().any(|af| af.finger.is_hard_validated())
    }

    // ------------------------------------------------------------------
    // Spawn
    // ------------------------------------------------------------------

    /// Integrate new detections from the correlator into the finger list.
    ///
    /// Fingers are deduplicated by `id()`.  When `max_fingers` is reached,
    /// excess detections in this block are discarded (the correlator will
    /// re-detect them in a later block once a slot opens).
    fn spawn_fingers(
        &mut self,
        detections: Vec<(C::Finger, Vec<PipelineProcessorShared>)>,
    ) -> Vec<SampleBlock> {
        let emit_timing_trace = self.emit_timing_trace;
        let mut timing_events = Vec::new();

        for (finger, chain) in detections {
            if self.fingers.len() >= self.max_fingers {
                debug!(
                    "GenericRakeReceiver: at capacity ({} fingers), discarding finger {}",
                    self.max_fingers,
                    finger.id()
                );
                self.correlator.notify_finger_removed(finger.id());
                continue;
            }
            // Deduplicate: do not add a second finger with the same id.
            if self.fingers.iter().any(|af| af.finger.id() == finger.id()) {
                debug!(
                    "GenericRakeReceiver: duplicate finger id {}, skipping",
                    finger.id()
                );
                continue;
            }
            let desc = finger.describe();
            if desc.is_empty() {
                info!(
                    "GenericRakeReceiver: spawning finger {} (total active: {})",
                    finger.id(),
                    self.fingers.len() + 1,
                );
            } else {
                info!(
                    "GenericRakeReceiver: spawning finger {} (total active: {}) [{}]",
                    finger.id(),
                    self.fingers.len() + 1,
                    desc,
                );
            }
            if emit_timing_trace {
                let spawn_chip = finger.spawn_chip_start().unwrap_or(0);
                let mut block = SampleBlock::new(Vec::new(), spawn_chip as usize);
                block.tags.insert("rake_finger_spawn_event", 1);
                block.tags.insert("finger_id", finger.id() as i64);
                if let Some(chip) = finger.spawn_chip_start() {
                    block.tags.insert("absolute_chip_start", chip as i64);
                }
                timing_events.push(block);
            }
            self.fingers.push(ActiveFinger {
                finger,
                chain,
                process_ns: 0,
                process_calls: 0,
                last_report_process_ns: 0,
                last_report_process_calls: 0,
                notified_validated: false,
            });
        }

        timing_events
    }

    // ------------------------------------------------------------------
    // Feed
    // ------------------------------------------------------------------

    /// Push `block` through every active finger in parallel and collect output.
    ///
    /// Finger ownership round-trips through the pool: main thread transfers
    /// all fingers to workers via a crossbeam MPMC channel, workers process
    /// and send results back via mpsc, then fingers are restored in their
    /// original order.
    fn feed_fingers(&mut self, block: &SampleBlock) -> Vec<SampleBlock> {
        let n = self.fingers.len();
        if n == 0 {
            return Vec::new();
        }

        let block = Arc::new(block.clone());
        let fingers = std::mem::take(&mut self.fingers);

        for (idx, finger) in fingers.into_iter().enumerate() {
            self.finger_pool
                .work_tx
                .send(FingerWorkItem {
                    idx,
                    finger,
                    block: Arc::clone(&block),
                })
                .expect("finger-pool worker threads died unexpectedly");
        }

        let mut slots: Vec<Option<ActiveFinger<C::Finger>>> = (0..n).map(|_| None).collect();
        let mut all_outputs = Vec::new();

        for _ in 0..n {
            let r = self
                .finger_pool
                .result_rx
                .recv()
                .expect("finger-pool worker threads died unexpectedly");
            all_outputs.extend(r.outputs);
            slots[r.idx] = Some(r.finger);
        }

        self.fingers = slots
            .into_iter()
            .map(|s| s.expect("missing finger result from pool"))
            .collect();

        all_outputs
    }

    fn prefer_access_block(a: &SampleBlock, b: &SampleBlock) -> std::cmp::Ordering {
        let a_abs_delta = a
            .tags
            .get("absolute_chip_start")
            .copied()
            .map(|chip| (chip - a.chip_start as i64).abs())
            .unwrap_or(i64::MAX);
        let b_abs_delta = b
            .tags
            .get("absolute_chip_start")
            .copied()
            .map(|chip| (chip - b.chip_start as i64).abs())
            .unwrap_or(i64::MAX);
        let a_finger = a.tags.get("finger_id").copied().unwrap_or(-1);
        let b_finger = b.tags.get("finger_id").copied().unwrap_or(-1);
        let a_snr = a.tags.get("finger_snr_mdb").copied().unwrap_or(i64::MIN);
        let b_snr = b.tags.get("finger_snr_mdb").copied().unwrap_or(i64::MIN);

        b_abs_delta
            .cmp(&a_abs_delta)
            .then_with(|| a_snr.cmp(&b_snr))
            .then_with(|| b.chip_start.cmp(&a.chip_start))
            .then_with(|| b_finger.cmp(&a_finger))
    }

    fn suppress_duplicate_access_events(&self, blocks: Vec<SampleBlock>) -> Vec<SampleBlock> {
        let mut keep = vec![true; blocks.len()];
        let keys: Vec<Option<AccessBurstKey>> =
            blocks.iter().map(AccessBurstKey::from_block).collect();

        for idx in 0..blocks.len() {
            if !keep[idx] {
                continue;
            }
            let Some(ref key) = keys[idx] else {
                continue;
            };

            let mut group = vec![idx];
            for other_idx in (idx + 1)..blocks.len() {
                let Some(ref other_key) = keys[other_idx] else {
                    continue;
                };
                if key.matches_burst(other_key) {
                    group.push(other_idx);
                }
            }

            if group.len() < 2 {
                continue;
            }

            let winner_idx = *group
                .iter()
                .max_by(|lhs, rhs| Self::prefer_access_block(&blocks[**lhs], &blocks[**rhs]))
                .expect("duplicate group must contain at least one block");
            let winner = &blocks[winner_idx];
            let winner_finger = winner.tags.get("finger_id").copied().unwrap_or(-1);

            let mut suppressed_fingers = Vec::new();
            for group_idx in group {
                if group_idx == winner_idx {
                    continue;
                }
                keep[group_idx] = false;
                if let Some(&finger_id) = blocks[group_idx].tags.get("finger_id") {
                    suppressed_fingers.push(finger_id);
                }
            }

            info!(
                "GenericRakeReceiver: suppressed duplicate access burst chip={} msg_type={} winner_finger={} suppressed_fingers={:?}",
                key.chip_start, key.access_msg_type, winner_finger, suppressed_fingers,
            );
        }

        blocks
            .into_iter()
            .enumerate()
            .filter_map(|(idx, block)| keep[idx].then_some(block))
            .collect()
    }

    fn remember_access_burst(&mut self, key: AccessBurstKey) {
        const MAX_RECENT_ACCESS_BURSTS: usize = 512;

        self.recent_access_burst_order.push_back(key);

        while self.recent_access_burst_order.len() > MAX_RECENT_ACCESS_BURSTS {
            self.recent_access_burst_order.pop_front();
        }
    }

    fn suppress_previously_emitted_access_events(
        &mut self,
        blocks: Vec<SampleBlock>,
    ) -> Vec<SampleBlock> {
        let mut out = Vec::with_capacity(blocks.len());

        for block in blocks {
            let Some(key) = AccessBurstKey::from_block(&block) else {
                out.push(block);
                continue;
            };

            if self
                .recent_access_burst_order
                .iter()
                .any(|prior| prior.matches_burst(&key))
            {
                info!(
                    "GenericRakeReceiver: suppressed previously-emitted access burst chip={} msg_type={} finger_id={}",
                    key.chip_start,
                    key.access_msg_type,
                    block.tags.get("finger_id").copied().unwrap_or(-1),
                );
                continue;
            }

            self.remember_access_burst(key);
            out.push(block);
        }

        out
    }

    // ------------------------------------------------------------------
    // Prune
    // ------------------------------------------------------------------

    /// Remove fingers the pruning policy says should be dropped.
    ///
    /// Fingers are retired according to the configured policy.  For bursty
    /// reverse-link traffic, even hard-validated fingers should eventually
    /// expire so later bursts can reacquire cleanly.
    fn prune_fingers(&mut self) {
        let before = self.fingers.len();
        let mut kept = Vec::with_capacity(self.fingers.len());
        let mut removed_ids = Vec::new();

        for af in self.fingers.drain(..) {
            let should_drop = self.prune_policy.should_prune(&af.finger);
            if should_drop {
                info!(
                    "GenericRakeReceiver: pruning finger {} (idle_blocks={}, idle_chips={}, validated={}, crc_misses={}, post_walsh_no_event_chips={}, post_walsh_no_event_ms={}, post_walsh_misses={}, signal_lost_chips={})",
                    af.finger.id(),
                    af.finger.idle_blocks(),
                    af.finger.idle_chips(),
                    af.finger.is_hard_validated(),
                    af.finger.crc_miss_count(),
                    af.finger.post_walsh_no_event_chips(),
                    af.finger.post_walsh_no_event_ms(),
                    af.finger.post_walsh_miss_count(),
                    af.finger.signal_lost_chips(),
                );
                af.finger.print_timing();
                for stage in &af.chain {
                    let m = stage.metrics();
                    if !m.is_empty() {
                        let pairs: Vec<String> =
                            m.iter().map(|(k, v)| format!("{k}={v}")).collect();
                        info!("  {} metrics: {}", stage.name(), pairs.join(" "));
                    }
                }
                removed_ids.push(af.finger.id());
            } else {
                kept.push(af);
            }
        }

        self.fingers = kept;
        for id in removed_ids {
            self.correlator.notify_finger_removed(id);
        }

        let after = self.fingers.len();
        if after < before {
            debug!(
                "GenericRakeReceiver: pruned {} finger(s), {} remaining",
                before - after,
                after
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use std::collections::VecDeque;

    use num_complex::Complex32;

    use super::{
        BaseFinger, Correlator, FingerProgress, GenericRakeReceiver, PipelineProcessor,
        PipelineProcessorShared, RakeFinger,
    };
    use crate::receiver::pipelined::SampleBlock;

    #[test]
    fn base_finger_refreshes_post_walsh_budget_on_relock() {
        let mut finger = BaseFinger::new(1);

        let mut first_lock = FingerProgress::default();
        first_lock.saw_walsh_lock = true;
        finger.tick_with_progress(&first_lock, 24_576);
        assert_eq!(finger.post_walsh_no_event_chips(), 24_576);

        let idle = FingerProgress::default();
        finger.tick_with_progress(&idle, 24_576);
        assert_eq!(finger.post_walsh_no_event_chips(), 49_152);

        let mut relock = FingerProgress::default();
        relock.saw_walsh_lock = true;
        finger.tick_with_progress(&relock, 24_576);
        assert_eq!(finger.post_walsh_no_event_chips(), 24_576);
        assert_eq!(finger.post_walsh_miss_count(), 0);
    }

    #[test]
    fn base_finger_keeps_post_walsh_budget_running_through_crc_misses() {
        let mut finger = BaseFinger::new(2);

        let mut valid = FingerProgress::default();
        valid.saw_crc_valid = true;
        finger.tick_with_progress(&valid, 24_576);
        assert!(finger.is_hard_validated());

        let mut walsh_lock = FingerProgress::default();
        walsh_lock.saw_walsh_lock = true;
        finger.tick_with_progress(&walsh_lock, 24_576);

        let mut crc_miss = FingerProgress::default();
        crc_miss.saw_completed_event = true;
        crc_miss.crc_misses = 1;
        finger.tick_with_progress(&crc_miss, 24_576);

        assert_eq!(finger.post_walsh_no_event_chips(), 49_152);
        assert_eq!(finger.post_walsh_miss_count(), 1);
        // This unit path executes back-to-back, so wall-clock milliseconds may
        // still be zero even though the post-Walsh budget progressed in chips.
        assert!(finger.post_walsh_no_event_ms() <= 1_000);
    }

    #[test]
    fn validated_access_crc_miss_does_not_refresh_idle_or_post_walsh() {
        let mut finger = BaseFinger::new(3);

        let mut valid = FingerProgress::default();
        valid.saw_crc_valid = true;
        finger.tick_with_progress(&valid, 24_576);
        assert!(finger.is_hard_validated());

        let mut walsh_lock = FingerProgress::default();
        walsh_lock.saw_walsh_lock = true;
        finger.tick_with_progress(&walsh_lock, 24_576);
        assert_eq!(finger.idle_chips(), 24_576);
        assert_eq!(finger.post_walsh_no_event_chips(), 24_576);

        let mut access_crc_miss = FingerProgress::default();
        access_crc_miss.saw_access_completed_event = true;
        access_crc_miss.saw_completed_event = true;
        access_crc_miss.saw_activity = true;
        access_crc_miss.crc_misses = 1;
        finger.tick_with_progress(&access_crc_miss, 24_576);

        assert_eq!(finger.idle_chips(), 49_152);
        assert_eq!(finger.post_walsh_no_event_chips(), 49_152);
        assert_eq!(finger.post_walsh_miss_count(), 1);
        assert_eq!(finger.crc_miss_count(), 1);
    }

    #[test]
    fn unvalidated_access_crc_miss_does_not_refresh_idle() {
        let mut finger = BaseFinger::new(4);

        let mut access_crc_miss = FingerProgress::default();
        access_crc_miss.saw_access_completed_event = true;
        access_crc_miss.saw_completed_event = true;
        access_crc_miss.saw_activity = true;
        access_crc_miss.crc_misses = 1;
        finger.tick_with_progress(&access_crc_miss, 24_576);

        assert_eq!(finger.idle_chips(), 24_576);
        assert_eq!(finger.idle_blocks(), 1);
        assert_eq!(finger.crc_miss_count(), 0);
    }

    #[test]
    fn validated_traffic_phy_activity_still_refreshes_idle() {
        let mut finger = BaseFinger::new(5);

        let mut valid = FingerProgress::default();
        valid.saw_crc_valid = true;
        finger.tick_with_progress(&valid, 24_576);
        assert!(finger.is_hard_validated());

        let idle = FingerProgress::default();
        finger.tick_with_progress(&idle, 24_576);
        assert_eq!(finger.idle_chips(), 24_576);

        let mut traffic_phy = FingerProgress::default();
        traffic_phy.saw_traffic_phy_activity = true;
        traffic_phy.saw_activity = true;
        finger.tick_with_progress(&traffic_phy, 24_576);
        assert_eq!(finger.idle_chips(), 0);
    }

    #[test]
    fn unvalidated_preamble_only_does_not_refresh_idle() {
        let mut finger = BaseFinger::new(6);

        let mut access_preamble = FingerProgress::default();
        access_preamble.saw_access_preamble_activity = true;
        finger.tick_with_progress(&access_preamble, 24_576);

        assert_eq!(finger.idle_chips(), 24_576);
        assert_eq!(finger.idle_blocks(), 1);
    }

    #[test]
    fn validated_traffic_preamble_only_does_not_refresh_idle() {
        let mut finger = BaseFinger::new(6);

        let mut valid = FingerProgress::default();
        valid.saw_crc_valid = true;
        finger.tick_with_progress(&valid, 24_576);
        assert!(finger.is_hard_validated());

        let idle = FingerProgress::default();
        finger.tick_with_progress(&idle, 24_576);
        assert_eq!(finger.idle_chips(), 24_576);

        let mut block = SampleBlock::new(Vec::new(), 0);
        block.tags.insert("traffic_preamble_detected", 1);
        finger.tick_and_validate(&[block], 24_576);
        assert_eq!(finger.idle_chips(), 49_152);
    }

    struct MockFinger {
        id: u64,
        outputs: VecDeque<Vec<SampleBlock>>,
    }

    impl MockFinger {
        fn new(id: u64, outputs: Vec<Vec<SampleBlock>>) -> Self {
            Self {
                id,
                outputs: outputs.into(),
            }
        }
    }

    impl RakeFinger for MockFinger {
        fn id(&self) -> u64 {
            self.id
        }

        fn process(
            &mut self,
            _block: &SampleBlock,
            _chain: &mut Vec<PipelineProcessorShared>,
        ) -> Vec<SampleBlock> {
            self.outputs.pop_front().unwrap_or_default()
        }

        fn flush(&mut self, _chain: &mut Vec<PipelineProcessorShared>) -> Vec<SampleBlock> {
            Vec::new()
        }

        fn is_hard_validated(&self) -> bool {
            true
        }

        fn idle_blocks(&self) -> u64 {
            0
        }
    }

    #[derive(Default)]
    struct MockCorrelator {
        detections: Vec<(MockFinger, Vec<PipelineProcessorShared>)>,
        removed_ids: Vec<u64>,
        search_suppressed: bool,
    }

    impl Correlator for MockCorrelator {
        type Finger = MockFinger;

        fn correlate(
            &mut self,
            _block: &SampleBlock,
        ) -> Vec<(Self::Finger, Vec<PipelineProcessorShared>)> {
            std::mem::take(&mut self.detections)
        }

        fn notify_finger_removed(&mut self, finger_id: u64) {
            self.removed_ids.push(finger_id);
        }

        fn search_suppressed(&self) -> bool {
            self.search_suppressed
        }
    }

    fn access_event_block(
        finger_id: i64,
        chip_start: usize,
        preamble_frames: i64,
        bits: &[u8],
    ) -> SampleBlock {
        access_event_block_with_absolute_chip(
            finger_id,
            chip_start,
            chip_start as i64,
            preamble_frames,
            bits,
        )
    }

    fn access_event_block_with_absolute_chip(
        finger_id: i64,
        chip_start: usize,
        absolute_chip_start: i64,
        preamble_frames: i64,
        bits: &[u8],
    ) -> SampleBlock {
        let samples = bits
            .iter()
            .map(|bit| Complex32::new(*bit as f32, 0.0))
            .collect();
        let mut block = SampleBlock::new(samples, chip_start);
        block.tags.insert("access_event", 1);
        block.tags.insert("access_crc_valid", 1);
        block.tags.insert("access_pd", 1);
        block.tags.insert("access_msg_type", 2);
        block.tags.insert("access_preamble_frames", preamble_frames);
        block.tags.insert("finger_id", finger_id);
        block
            .tags
            .insert("absolute_chip_start", absolute_chip_start);
        block
    }

    #[test]
    fn suppresses_duplicate_access_events_without_retiring_fingers() {
        let old = MockFinger::new(
            3,
            vec![vec![access_event_block(3, 42_000, 0, &[1, 0, 1, 1])]],
        );
        let new = MockFinger::new(
            7,
            vec![vec![access_event_block(7, 42_000, 4, &[1, 0, 1, 1])]],
        );
        let mut receiver = GenericRakeReceiver::new(MockCorrelator {
            detections: vec![(old, Vec::new()), (new, Vec::new())],
            removed_ids: Vec::new(),
            search_suppressed: false,
        });

        let out = receiver.process_block(SampleBlock::new(Vec::new(), 0));

        assert_eq!(1, out.len());
        assert_eq!(Some(&3), out[0].tags.get("finger_id"));
        assert_eq!(Some(&0), out[0].tags.get("access_preamble_frames"));
        assert_eq!(2, receiver.finger_count());
        assert!(receiver.correlator.removed_ids.is_empty());
    }

    #[test]
    fn suppresses_nearby_chip_duplicate_access_events() {
        let old = MockFinger::new(
            3,
            vec![vec![access_event_block(3, 42_000, 0, &[1, 0, 1, 1])]],
        );
        let new = MockFinger::new(
            7,
            vec![vec![access_event_block(7, 42_032, 4, &[1, 0, 1, 1])]],
        );
        let mut receiver = GenericRakeReceiver::new(MockCorrelator {
            detections: vec![(old, Vec::new()), (new, Vec::new())],
            removed_ids: Vec::new(),
            search_suppressed: false,
        });

        let out = receiver.process_block(SampleBlock::new(Vec::new(), 0));

        assert_eq!(1, out.len());
        assert_eq!(Some(&3), out[0].tags.get("finger_id"));
    }

    #[test]
    fn suppresses_duplicate_access_events_with_same_frame_start_and_different_absolute_tag() {
        let old = MockFinger::new(
            3,
            vec![vec![access_event_block_with_absolute_chip(
                3,
                42_000,
                50_191,
                0,
                &[1, 0, 1, 1],
            )]],
        );
        let new = MockFinger::new(
            7,
            vec![vec![access_event_block_with_absolute_chip(
                7,
                42_000,
                42_000,
                4,
                &[1, 0, 1, 1],
            )]],
        );
        let mut receiver = GenericRakeReceiver::new(MockCorrelator {
            detections: vec![(old, Vec::new()), (new, Vec::new())],
            removed_ids: Vec::new(),
            search_suppressed: false,
        });

        let out = receiver.process_block(SampleBlock::new(Vec::new(), 0));

        assert_eq!(1, out.len());
        assert_eq!(Some(&7), out[0].tags.get("finger_id"));
        assert_eq!(Some(&42_000), out[0].tags.get("absolute_chip_start"));
    }

    #[test]
    fn keeps_distinct_access_events_even_when_payload_matches() {
        let first = MockFinger::new(
            3,
            vec![vec![access_event_block(3, 42_000, 0, &[1, 0, 1, 1])]],
        );
        let second = MockFinger::new(
            7,
            vec![vec![access_event_block(7, 42_256, 4, &[1, 0, 1, 1])]],
        );
        let mut receiver = GenericRakeReceiver::new(MockCorrelator {
            detections: vec![(first, Vec::new()), (second, Vec::new())],
            removed_ids: Vec::new(),
            search_suppressed: false,
        });

        let out = receiver.process_block(SampleBlock::new(Vec::new(), 0));

        assert_eq!(2, out.len());
        assert_eq!(2, receiver.finger_count());
        assert!(receiver.correlator.removed_ids.is_empty());
    }

    #[test]
    fn suppresses_reemitted_access_event_across_blocks() {
        let finger = MockFinger::new(
            7,
            vec![
                vec![access_event_block(7, 42_000, 4, &[1, 0, 1, 1])],
                vec![access_event_block(7, 42_000, 0, &[1, 0, 1, 1])],
            ],
        );
        let mut receiver = GenericRakeReceiver::new(MockCorrelator {
            detections: vec![(finger, Vec::new())],
            removed_ids: Vec::new(),
            search_suppressed: false,
        });

        let first = receiver.process_block(SampleBlock::new(Vec::new(), 0));
        let second = receiver.process_block(SampleBlock::new(Vec::new(), 0));

        assert_eq!(1, first.len());
        assert!(second.is_empty());
        assert_eq!(1, receiver.finger_count());
        assert!(receiver.correlator.removed_ids.is_empty());
    }

    #[test]
    fn suppresses_reemitted_nearby_chip_access_event_across_blocks() {
        let finger = MockFinger::new(
            7,
            vec![
                vec![access_event_block(7, 42_000, 4, &[1, 0, 1, 1])],
                vec![access_event_block(7, 42_032, 0, &[1, 0, 1, 1])],
            ],
        );
        let mut receiver = GenericRakeReceiver::new(MockCorrelator {
            detections: vec![(finger, Vec::new())],
            removed_ids: Vec::new(),
            search_suppressed: false,
        });

        let first = receiver.process_block(SampleBlock::new(Vec::new(), 0));
        let second = receiver.process_block(SampleBlock::new(Vec::new(), 0));

        assert_eq!(1, first.len());
        assert!(second.is_empty());
    }

    #[test]
    fn traffic_mode_keeps_only_one_validated_finger_after_lock() {
        let first = MockFinger::new(3, vec![Vec::new()]);
        let second = MockFinger::new(7, vec![Vec::new()]);
        let mut receiver = GenericRakeReceiver::new(MockCorrelator {
            detections: vec![(first, Vec::new()), (second, Vec::new())],
            removed_ids: Vec::new(),
            search_suppressed: true,
        });

        let out = receiver.process_block(SampleBlock::new(Vec::new(), 0));

        assert!(out.is_empty());
        assert_eq!(1, receiver.finger_count());
        assert_eq!(vec![7], receiver.correlator.removed_ids);
    }
}

// ---------------------------------------------------------------------------
// PipelineProcessor impl
// ---------------------------------------------------------------------------

impl<C: Correlator> PipelineProcessor for GenericRakeReceiver<C> {
    fn process_block(&mut self, block: SampleBlock) -> Vec<SampleBlock> {
        let mut emitter = VecEmitter::new();
        let mut out = self.process_block_emitting(block, &mut emitter);
        out.extend(emitter.blocks);
        out
    }

    fn process_block_emitting(
        &mut self,
        block: SampleBlock,
        _emitter: &mut dyn super::PipelineEmitter,
    ) -> Vec<SampleBlock> {
        self.block_count += 1;

        // Retire stale fingers before running acquisition so a burst that just
        // aged out does not block a fresh detection for one extra input block.
        self.prune_fingers();

        // 1. Run the correlator — may return new (finger, chain) pairs.
        //    Skip entirely when we already have max_fingers validated fingers;
        //    the search + candidate verification + replay is wasted work.
        let t0 = std::time::Instant::now();
        let validated_count = self
            .fingers
            .iter()
            .filter(|af| af.notified_validated)
            .count();
        let mut out = Vec::new();
        if validated_count < self.max_fingers {
            let detections = self.correlator.correlate(&block);
            out.extend(self.spawn_fingers(detections));
        }
        let correlator_ns = t0.elapsed().as_nanos() as u64;

        // 2. Feed every active finger and collect output.
        let t1 = std::time::Instant::now();
        let finger_out = self.feed_fingers(&block);
        let finger_out = self.suppress_duplicate_access_events(finger_out);
        let finger_out = self.suppress_previously_emitted_access_events(finger_out);
        out.extend(finger_out);
        let fingers_ns = t1.elapsed().as_nanos() as u64;

        // 2b. Notify correlator when a finger becomes hard-validated.
        for af in &self.fingers {
            if af.finger.is_hard_validated() && !af.notified_validated {
                self.correlator.notify_hard_validated(af.finger.id());
            }
        }
        for af in &mut self.fingers {
            if af.finger.is_hard_validated() {
                af.notified_validated = true;
            }
        }

        // 2b½. When suppress_search_when_locked is active (traffic channels),
        // prune all siblings once any finger validates.
        // Traffic expects a single decoded stream — redundant fingers at the
        // same delay just waste CPU processing the same signal and can emit
        // duplicate decoded frames.
        if self.correlator.search_suppressed()
            && self.fingers.iter().any(|af| af.notified_validated)
        {
            let before = self.fingers.len();
            let winner_id = self
                .fingers
                .iter()
                .find(|af| af.notified_validated)
                .map(|af| af.finger.id())
                .expect("validated finger must exist when pruning traffic siblings");
            let mut removed = Vec::new();
            self.fingers.retain(|af| {
                if af.finger.id() == winner_id {
                    true
                } else {
                    removed.push(af.finger.id());
                    false
                }
            });
            for id in &removed {
                self.correlator.notify_finger_removed(*id);
            }
            if !removed.is_empty() {
                info!(
                    "GenericRakeReceiver: pruned {} traffic sibling(s) after validation, keeping finger {} ({} → {})",
                    removed.len(),
                    winner_id,
                    before,
                    self.fingers.len(),
                );
            }
        }

        // 2c. Keep the correlator informed about live finger state so it can
        // decide when same-delay reacquisition should be permitted.
        for af in &self.fingers {
            self.correlator.notify_finger_state(
                af.finger.id(),
                af.finger.is_hard_validated(),
                af.finger.idle_chips(),
                af.finger.signal_lost_chips(),
                af.finger.crc_miss_count(),
                af.finger.post_walsh_no_event_ms(),
            );
        }

        // 3. Prune fingers that have gone idle or exceeded their budget.
        self.prune_fingers();

        // Accumulate timing stats.
        self.correlator_ns += correlator_ns;
        self.fingers_ns += fingers_ns;
        if self.block_count % 500 == 0 || self.block_count == 1 {
            let total_ms = (self.correlator_ns + self.fingers_ns) as f64 / 1e6;
            let corr_ms = self.correlator_ns as f64 / 1e6;
            let fing_ms = self.fingers_ns as f64 / 1e6;
            let corr_pct = if total_ms > 0.0 {
                corr_ms / total_ms * 100.0
            } else {
                0.0
            };
            let fing_pct = if total_ms > 0.0 {
                fing_ms / total_ms * 100.0
            } else {
                0.0
            };
            debug!(
                "[RAKE blk={}] correlator: {:.1}ms ({:.1}%) | fingers({}):{:.1}ms ({:.1}%) | total: {:.1}ms",
                self.block_count,
                corr_ms,
                corr_pct,
                self.fingers.len(),
                fing_ms,
                fing_pct,
                total_ms,
            );
        }

        if self.report_interval_start.elapsed().as_secs_f64() >= 1.0 && !self.fingers.is_empty() {
            let interval_blocks = self
                .block_count
                .saturating_sub(self.last_report_block_count);
            let interval_corr_ns = self
                .correlator_ns
                .saturating_sub(self.last_report_correlator_ns);
            let interval_fingers_ns = self.fingers_ns.saturating_sub(self.last_report_fingers_ns);
            let interval_total_ns = interval_corr_ns.saturating_add(interval_fingers_ns);
            let corr_ms = interval_corr_ns as f64 / 1e6;
            let fing_ms = interval_fingers_ns as f64 / 1e6;
            let total_ms = interval_total_ns as f64 / 1e6;
            debug!(
                "GenericRakeReceiver periodic: blocks={} active_fingers={} correlator={:.1}ms ({:.1}%) fingers={:.1}ms ({:.1}%) total={:.1}ms",
                interval_blocks,
                self.fingers.len(),
                corr_ms,
                if total_ms > 0.0 {
                    corr_ms / total_ms * 100.0
                } else {
                    0.0
                },
                fing_ms,
                if total_ms > 0.0 {
                    fing_ms / total_ms * 100.0
                } else {
                    0.0
                },
                total_ms,
            );
            for af in &mut self.fingers {
                let delta_ns = af.process_ns.saturating_sub(af.last_report_process_ns);
                let delta_calls = af
                    .process_calls
                    .saturating_sub(af.last_report_process_calls);
                let delta_ms = delta_ns as f64 / 1e6;
                let avg_us = if delta_calls > 0 {
                    delta_ns as f64 / delta_calls as f64 / 1e3
                } else {
                    0.0
                };
                debug!(
                    "GenericRakeReceiver finger: id={} interval={:.1}ms calls={} avg={:.1}us validated={} idle_chips={} crc_misses={} post_walsh_ms={}",
                    af.finger.id(),
                    delta_ms,
                    delta_calls,
                    avg_us,
                    af.finger.is_hard_validated(),
                    af.finger.idle_chips(),
                    af.finger.crc_miss_count(),
                    af.finger.post_walsh_no_event_ms(),
                );
                for line in af.finger.timing_report_lines() {
                    debug!("{}", line);
                }
                af.last_report_process_ns = af.process_ns;
                af.last_report_process_calls = af.process_calls;
            }
            self.report_interval_start = Instant::now();
            self.last_report_block_count = self.block_count;
            self.last_report_correlator_ns = self.correlator_ns;
            self.last_report_fingers_ns = self.fingers_ns;
        }

        out
    }

    fn flush(&mut self) -> Vec<SampleBlock> {
        // Print per-finger timing summary before flush.
        let total_ms = (self.correlator_ns + self.fingers_ns) as f64 / 1e6;
        let corr_ms = self.correlator_ns as f64 / 1e6;
        let fing_ms = self.fingers_ns as f64 / 1e6;
        debug!(
            "\n=== GenericRakeReceiver Timing Report ({} blocks) ===",
            self.block_count
        );
        debug!(
            "  correlator (search):  {:.1}ms ({:.1}%)",
            corr_ms,
            if total_ms > 0.0 {
                corr_ms / total_ms * 100.0
            } else {
                0.0
            }
        );
        debug!(
            "  finger processing:    {:.1}ms ({:.1}%)",
            fing_ms,
            if total_ms > 0.0 {
                fing_ms / total_ms * 100.0
            } else {
                0.0
            }
        );
        debug!("  total:                {:.1}ms", total_ms);
        for af in &self.fingers {
            let avg_us = if af.process_calls > 0 {
                af.process_ns as f64 / af.process_calls as f64 / 1e3
            } else {
                0.0
            };
            debug!(
                "  finger {:>3}: {:.1}ms total, {} calls, {:.1}us/call, validated={}",
                af.finger.id(),
                af.process_ns as f64 / 1e6,
                af.process_calls,
                avg_us,
                af.finger.is_hard_validated(),
            );
            af.finger.print_timing();
            for stage in &af.chain {
                let m = stage.metrics();
                if !m.is_empty() {
                    let pairs: Vec<String> = m.iter().map(|(k, v)| format!("{k}={v}")).collect();
                    debug!("      {} metrics: {}", stage.name(), pairs.join(" "));
                }
            }
        }

        let mut out = Vec::new();
        for af in &mut self.fingers {
            out.extend(af.finger.flush(&mut af.chain));
        }
        out
    }

    fn name(&self) -> &'static str {
        "GenericRakeReceiver"
    }
}
