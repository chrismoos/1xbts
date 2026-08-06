use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use crossbeam_queue::ArrayQueue;
use parking_lot::Mutex;

use tokio::sync::{mpsc, oneshot, watch};

use cdma_common::{
    hrpd::air::{
        HrpdAccessIndication, HrpdForwardSignalingRequest, HrpdTrafficAssignmentRequest,
        HrpdTrafficEvent, HrpdTrafficReleaseRequest,
    },
    sch::Rc3FschProfile,
    time::CdmaSystemTime,
};

use crate::{
    channels::{
        Channel, PcgPcbFallbackMode, PcgPcbScheduler, WalshChannel, WalshChannelWrapper,
        f_sch_rc3::{ForwardSupplementalChannelRc3, SchConfigRc3, interleaver_params},
        ftch::{self, ForwardTrafficChannel},
        ftch_rc2::{ConfigRc2, ForwardTrafficChannelRc2},
        ftch_rc3::{self, ForwardTrafficChannelRc3},
    },
    phy::coding::{
        block_interleaver::{
            BitReversalInterleaver, ForwardBackwardsBitReversalInterleaver, SR1_PARAMS_384,
            SR1_PARAMS_768,
        },
        convolutional::{get_1_2_k9_encoder, get_1_4_k9_encoder},
        long_code::LongCodeGenerator,
    },
    phy::walsh::WalshGenerator,
    sdr::TxRadioHealth,
};

use super::{AccessChannelEvent, BtsPowerControlRegistry, BtsRuntimeSettings};
use crate::bts::hrpd::harq_bus::HarqBus;
use crate::bts::hrpd::scheduler::{
    ForwardTrafficPreparer, ForwardTrafficSender, PreparedForwardTrafficPacket,
};

/// Metrics snapshot from the TX loop, published every ~1 second.
#[derive(Debug, Clone, Default)]
pub struct TxMetrics {
    pub timestamp_ns: u64,
    pub chip_cursor: u64,
    pub blocks_transmitted: u64,
    pub rt_ratio: f64,
    pub gen_avg_us: u64,
    pub gen_max_us: u64,
    pub tx_avg_us: u64,
    pub tx_max_us: u64,
    pub pulse_avg_us: u64,
    pub pulse_max_us: u64,
    pub hw_margin_min_us: i64,
    pub hw_margin_avg_us: i64,
    pub hw_margin_max_us: i64,
    pub late_batches: u64,
    pub radio_health: TxRadioHealth,
    pub finalized_queue_airtime_us: u64,
    pub synth_pilot_us: u64,
    pub synth_sync_us: u64,
    pub synth_paging_us: u64,
    pub synth_spread_us: u64,
    pub sync_fragments_sent: u64,
    pub paging_fragments_sent: u64,
    pub realtime_degraded_events: u64,
}

/// Metrics snapshot from the RX pipeline, published every ~1 second.
#[derive(Debug, Clone, Default)]
pub struct RxMetrics {
    pub reads: u64,
    pub samples: u64,
    pub rt_ratio: f64,
    pub capture_us: u64,
    pub pipeline_us: u64,
    pub total_us: u64,
    pub total_max_us: u64,
    pub stages: Vec<StageMetrics>,
    pub deficit_ms: Option<f64>,
    pub queues: Vec<RxQueueMetrics>,
}

#[derive(Debug, Clone, Default)]
pub struct RxQueueMetrics {
    pub name: String,
    pub queued_samples: u64,
    pub queued_airtime_us: u64,
    pub max_queued_samples: u64,
    pub max_residency_us: u64,
}

/// Per-stage breakdown within the RX pipeline.
#[derive(Debug, Clone)]
pub struct StageMetrics {
    pub name: String,
    pub total_us: u64,
    pub calls: u64,
    pub max_us: u64,
    pub pct_pipeline: f64,
}

#[derive(Debug, Clone)]
pub struct IqCaptureStatus {
    pub active: bool,
    pub directory: PathBuf,
    pub wav_path: Option<PathBuf>,
    pub metadata_path: Option<PathBuf>,
    pub first_absolute_chip_start: Option<u64>,
    pub first_absolute_sample_start: Option<u64>,
    pub first_sample_system_time: Option<CdmaSystemTime>,
    pub first_hardware_time_ns: Option<u64>,
    pub captured_samples: u64,
    pub sample_rate_hz: usize,
    pub chip_rate_hz: usize,
}

#[derive(Debug, Clone)]
pub struct IqCaptureControlResult {
    pub status: IqCaptureStatus,
    pub message: String,
}

/// Commands the BSC can send to the BTS.
pub enum BtsCommand {
    GetCaptureStatus {
        directory: PathBuf,
        respond_to: oneshot::Sender<Result<IqCaptureControlResult, String>>,
    },
    StartCapture {
        directory: PathBuf,
        respond_to: oneshot::Sender<Result<IqCaptureControlResult, String>>,
    },
    StopCapture {
        respond_to: oneshot::Sender<Result<IqCaptureControlResult, String>>,
    },
    Shutdown,
}

/// Type alias for a Walsh-wrapped forward traffic channel (RC1).
pub type TrafficWalshChannel = WalshChannelWrapper<ForwardTrafficChannel<9, 2>>;

/// Type alias for a Walsh-wrapped RC2 forward traffic channel.
pub type TrafficWalshChannelRc2 = WalshChannelWrapper<ForwardTrafficChannelRc2>;

/// Type alias for a Walsh-wrapped RC3 forward traffic channel.
pub type TrafficWalshChannelRc3 = WalshChannelWrapper<ForwardTrafficChannelRc3>;

/// Type alias for a Walsh-wrapped RC3 forward supplemental channel.
pub type SchWalshChannelRc3 = WalshChannelWrapper<ForwardSupplementalChannelRc3>;

/// Wrapper enum for forward traffic channels of different radio configurations.
/// Implements the Channel trait to allow the TX loop to treat all traffic
/// channels uniformly regardless of RC.
#[derive(Clone)]
pub enum TrafficChannelWrapper {
    Rc1(TrafficWalshChannel),
    Rc2(TrafficWalshChannelRc2),
    Rc3(TrafficWalshChannelRc3),
    SchRc3(SchWalshChannelRc3),
}

impl TrafficChannelWrapper {
    /// Align the channel's long code generator to the given absolute chip position.
    /// Called once by the TX loop on first use to synchronize with the system timeline.
    pub fn advance_lc_to_chip(&self, chip: u64) {
        match self {
            TrafficChannelWrapper::Rc1(ch) => ch.channel.advance_lc_to_chip(chip),
            TrafficChannelWrapper::Rc2(ch) => ch.channel.advance_lc_to_chip(chip),
            TrafficChannelWrapper::Rc3(ch) => ch.channel.advance_lc_to_chip(chip),
            TrafficChannelWrapper::SchRc3(ch) => ch.channel.advance_lc_to_chip(chip),
        }
    }

    /// Enqueue signaling bits (172-bit MuxPDU frame) on the priority queue.
    pub fn send_signaling_bits(&self, bits: Vec<u8>) {
        match self {
            TrafficChannelWrapper::Rc1(ch) => ch.channel.send_signaling_bits(bits),
            TrafficChannelWrapper::Rc2(ch) => ch.channel.send_signaling_bits(bits),
            TrafficChannelWrapper::Rc3(ch) => ch.channel.send_signaling_bits(bits),
            TrafficChannelWrapper::SchRc3(_) => {}
        }
    }

    /// Pre-fill the Walsh buffer with silence chips so the first real frame
    /// starts at the correct 20ms boundary.
    pub fn prefill_silence(&self, n: usize) {
        match self {
            TrafficChannelWrapper::Rc1(ch) => ch.prefill_silence(n),
            TrafficChannelWrapper::Rc2(ch) => ch.prefill_silence(n),
            TrafficChannelWrapper::Rc3(ch) => ch.prefill_silence(n),
            TrafficChannelWrapper::SchRc3(ch) => ch.prefill_silence(n),
        }
    }

    pub fn queue_len(&self) -> usize {
        match self {
            TrafficChannelWrapper::Rc1(ch) => ch.channel.queue_len(),
            TrafficChannelWrapper::Rc2(ch) => ch.channel.queue_len(),
            TrafficChannelWrapper::Rc3(ch) => ch.channel.queue_len(),
            TrafficChannelWrapper::SchRc3(ch) => ch.channel.queue_len(),
        }
    }

    pub fn last_enqueue_at(&self) -> Option<std::time::Instant> {
        match self {
            TrafficChannelWrapper::Rc1(ch) => ch.channel.last_enqueue_at(),
            TrafficChannelWrapper::Rc2(ch) => ch.channel.last_enqueue_at(),
            TrafficChannelWrapper::Rc3(ch) => ch.channel.last_enqueue_at(),
            TrafficChannelWrapper::SchRc3(_ch) => None,
        }
    }

    /// Generates a block and reports whether it contains a physical transmission.
    /// F-SCH reports false during pre-start silence and Null/DTX.
    pub fn next_block_into_with_activity(
        &self,
        out: &mut Vec<num::complex::Complex32>,
        num_samples: usize,
        system_time: CdmaSystemTime,
    ) -> bool {
        match self {
            TrafficChannelWrapper::Rc1(ch) => {
                ch.next_block_into(out, num_samples, system_time);
                true
            }
            TrafficChannelWrapper::Rc2(ch) => {
                ch.next_block_into(out, num_samples, system_time);
                true
            }
            TrafficChannelWrapper::Rc3(ch) => {
                ch.next_block_into(out, num_samples, system_time);
                true
            }
            TrafficChannelWrapper::SchRc3(ch) => {
                ch.next_block_into_with_activity(out, num_samples, system_time)
            }
        }
    }
}

impl Channel for TrafficChannelWrapper {
    fn next_block(
        &self,
        num_samples: usize,
        system_time: CdmaSystemTime,
    ) -> Vec<num::complex::Complex32> {
        match self {
            TrafficChannelWrapper::Rc1(ch) => ch.next_block(num_samples, system_time),
            TrafficChannelWrapper::Rc2(ch) => ch.next_block(num_samples, system_time),
            TrafficChannelWrapper::Rc3(ch) => ch.next_block(num_samples, system_time),
            TrafficChannelWrapper::SchRc3(ch) => ch.next_block(num_samples, system_time),
        }
    }

    fn next_block_into(
        &self,
        out: &mut Vec<num::complex::Complex32>,
        num_samples: usize,
        system_time: CdmaSystemTime,
    ) {
        match self {
            TrafficChannelWrapper::Rc1(ch) => ch.next_block_into(out, num_samples, system_time),
            TrafficChannelWrapper::Rc2(ch) => ch.next_block_into(out, num_samples, system_time),
            TrafficChannelWrapper::Rc3(ch) => ch.next_block_into(out, num_samples, system_time),
            TrafficChannelWrapper::SchRc3(ch) => ch.next_block_into(out, num_samples, system_time),
        }
    }
}

/// A single active traffic channel slot in the registry.
///
/// Shared by `Arc` between the BTS-side non-TX threads (bearer agent,
/// power-control, abis agent) and the TX thread's private working list.
/// Only `gain` is mutated post-publication, via atomics — all other fields
/// are immutable once the slot is constructed.
pub struct TrafficChannelSlot {
    pub walsh_code: u8,
    pub channel: TrafficChannelWrapper,
    /// `f32::to_bits` of the linear gain. Power control writes via
    /// `gain.store(f32::to_bits(g), Relaxed)`; TX reads via
    /// `f32::from_bits(gain.load(Relaxed))` once per block per slot.
    pub gain: AtomicU32,
}

impl TrafficChannelSlot {
    pub fn new(walsh_code: u8, channel: TrafficChannelWrapper, gain_linear: f32) -> Arc<Self> {
        Arc::new(Self {
            walsh_code,
            channel,
            gain: AtomicU32::new(gain_linear.to_bits()),
        })
    }

    /// Current gain as a linear amplitude ratio.
    pub fn gain(&self) -> f32 {
        f32::from_bits(self.gain.load(Ordering::Relaxed))
    }

    pub fn set_gain(&self, gain_linear: f32) {
        self.gain.store(gain_linear.to_bits(), Ordering::Relaxed);
    }
}

/// Command sent from the registry to the TX thread's private `TxPool`.
pub enum PoolCommand {
    Add(Arc<TrafficChannelSlot>),
    Remove(u8),
}

const POOL_CMD_QUEUE_CAPACITY: usize = 64;

/// Shared registry of active traffic channel slots.
///
/// Non-TX threads (bearer agent, abis agent, power control) look up channels
/// here by walsh code under a brief `Mutex<HashMap>`. The TX thread never
/// touches this lock — it maintains its own private working list (`TxPool`)
/// updated via a lock-free MPMC ring (`ArrayQueue`) that the registry feeds
/// on every `add`/`remove`.
pub struct ChannelRegistry {
    slots: Mutex<HashMap<u8, Arc<TrafficChannelSlot>>>,
    tx_cmd: Arc<ArrayQueue<PoolCommand>>,
}

impl ChannelRegistry {
    pub fn new() -> Self {
        Self {
            slots: Mutex::new(HashMap::new()),
            tx_cmd: Arc::new(ArrayQueue::new(POOL_CMD_QUEUE_CAPACITY)),
        }
    }

    /// Return the shared TX command queue so the TX thread can drain it.
    pub fn tx_cmd_queue(&self) -> Arc<ArrayQueue<PoolCommand>> {
        self.tx_cmd.clone()
    }

    pub fn add(&self, slot: Arc<TrafficChannelSlot>) {
        let walsh = slot.walsh_code;
        self.slots.lock().insert(walsh, slot.clone());
        if self.tx_cmd.push(PoolCommand::Add(slot)).is_err() {
            log::error!(
                "ChannelRegistry::add: tx-cmd queue overflow for walsh={}",
                walsh
            );
        }
    }

    pub fn remove(&self, walsh_code: u8) -> Option<Arc<TrafficChannelSlot>> {
        let removed = self.slots.lock().remove(&walsh_code);
        if removed.is_some() && self.tx_cmd.push(PoolCommand::Remove(walsh_code)).is_err() {
            log::error!(
                "ChannelRegistry::remove: tx-cmd queue overflow for walsh={}",
                walsh_code
            );
        }
        removed
    }

    pub fn lookup(&self, walsh_code: u8) -> Option<Arc<TrafficChannelSlot>> {
        self.slots.lock().get(&walsh_code).cloned()
    }

    pub fn contains(&self, walsh_code: u8) -> bool {
        self.slots.lock().contains_key(&walsh_code)
    }

    pub fn len(&self) -> usize {
        self.slots.lock().len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.lock().is_empty()
    }

    /// Snapshot of currently-registered walsh codes. Useful for tests and
    /// administrative iteration; not called on any hot path.
    pub fn walsh_codes(&self) -> Vec<u8> {
        self.slots.lock().keys().copied().collect()
    }
}

impl Default for ChannelRegistry {
    fn default() -> Self {
        Self::new()
    }
}

/// Shared handle to the channel registry. Cloned by every BTS thread that
/// needs to look up channels by walsh code (bearer, power-control, abis,
/// launcher event loop). The TX thread does **not** use this handle on its
/// hot path — it consumes from the `ArrayQueue<PoolCommand>` obtained via
/// [`ChannelRegistry::tx_cmd_queue`] into its private `TxPool`.
pub type TrafficChannelPool = Arc<ChannelRegistry>;

/// TX-thread-private working list of active traffic channels.
///
/// Updated only by the TX thread, sourced via the lock-free command queue
/// from [`ChannelRegistry`]. The TX hot path iterates this list without
/// taking any blocking lock.
pub struct TxPool {
    slots: Vec<TxSlot>,
    cmd_rx: Arc<ArrayQueue<PoolCommand>>,
}

pub struct TxSlot {
    pub walsh_code: u8,
    pub slot: Arc<TrafficChannelSlot>,
    /// Absolute chip time at which the channel becomes active on the air.
    /// Before this boundary the TX loop mixes zeros for the slot.
    pub start_chip: Option<u64>,
    /// False until the TX loop has aligned the channel's LC generator to
    /// the live chip cursor. Set on first observation.
    pub lc_aligned: bool,
    /// Set after the first block verifies frame-boundary alignment.
    pub frame_align_verified: bool,
}

impl TxPool {
    pub fn new(cmd_rx: Arc<ArrayQueue<PoolCommand>>) -> Self {
        Self {
            slots: Vec::new(),
            cmd_rx,
        }
    }

    /// Drain pending Add/Remove commands. Called by the TX thread at the
    /// top of each `synthesize_block`.
    pub fn drain_commands(&mut self) {
        while let Some(cmd) = self.cmd_rx.pop() {
            match cmd {
                PoolCommand::Add(slot) => {
                    let walsh_code = slot.walsh_code;
                    // Replace any prior entry with the same walsh; the
                    // registry guarantees a Remove precedes an Add for
                    // the same code in steady state, but be defensive.
                    self.slots.retain(|s| s.walsh_code != walsh_code);
                    self.slots.push(TxSlot {
                        walsh_code,
                        slot,
                        start_chip: None,
                        lc_aligned: false,
                        frame_align_verified: false,
                    });
                }
                PoolCommand::Remove(walsh_code) => {
                    self.slots.retain(|s| s.walsh_code != walsh_code);
                }
            }
        }
    }

    pub fn slots_mut(&mut self) -> &mut [TxSlot] {
        &mut self.slots
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }
}

pub use cdma_common::traffic::{
    RC1_TRAFFIC_INITIAL_GAIN_LINEAR, RC2_TRAFFIC_INITIAL_GAIN_LINEAR,
    RC3_TRAFFIC_INITIAL_GAIN_LINEAR, TrafficRxRequest,
};

/// Lifecycle command for an HRPD reverse traffic receiver worker.
///
/// The forward synth thread produces these as sessions are assigned and
/// released; the BTS RX loop drains them in FIFO order so assign/release
/// commands for a reused MAC index cannot cross.
#[derive(Debug)]
pub enum HrpdTrafficRxCommand {
    /// Start a reverse traffic worker for the assigned session.
    Assign(HrpdTrafficAssignmentRequest),
    /// Stop the reverse traffic worker for the released session.
    Release(HrpdTrafficReleaseRequest),
}

/// Lock-free command stream from the forward synth thread to the BTS RX loop.
pub type HrpdTrafficRxQueue = Arc<ArrayQueue<HrpdTrafficRxCommand>>;

/// Capacity of the HRPD reverse traffic command queue. Lifecycle events are
/// rare (per call setup/teardown) and the RX loop drains every block, so this
/// is far above any realistic in-flight backlog.
pub const HRPD_TRAFFIC_RX_QUEUE_CAPACITY: usize = 256;

/// Shared pool of pending reverse traffic channel receiver requests.
///
/// The BSC adds entries when assigning traffic channels; the BTS RX loop
/// picks them up and creates the actual receiver pipelines.
pub type TrafficRxPool = Arc<Mutex<Vec<TrafficRxRequest>>>;

/// Shared list of Walsh codes whose traffic RX receivers should be removed.
/// The BSC pushes codes here on teardown; the RX thread drains and removes.
pub type TrafficRxRemovals = Arc<Mutex<Vec<u8>>>;

const FIRST_TRAFFIC_WALSH_CODE: usize = 10;

/// Simple Walsh code allocator. Tracks which codes (0–63) are in use.
pub struct WalshAllocator {
    in_use: [bool; 64],
    /// Next index to try when allocating (round-robin cursor).
    next_start: usize,
}

impl WalshAllocator {
    pub fn new() -> Self {
        Self {
            in_use: [false; 64],
            next_start: FIRST_TRAFFIC_WALSH_CODE,
        }
    }

    /// Mark system channels (pilot=0, paging=1, sync=32) as reserved.
    pub fn reserve_system_channels(&mut self, pilot: u8, paging: u8, sync: u8) {
        self.in_use[pilot as usize] = true;
        self.in_use[paging as usize] = true;
        self.in_use[sync as usize] = true;
    }

    /// Allocate the next available traffic Walsh code using round-robin.
    /// Wraps around the range `FIRST_TRAFFIC_WALSH_CODE..64` so that
    /// recently-released codes are not immediately reused.
    pub fn allocate(&mut self) -> Option<u8> {
        let range = 64 - FIRST_TRAFFIC_WALSH_CODE;
        for offset in 0..range {
            let i = FIRST_TRAFFIC_WALSH_CODE
                + (self.next_start - FIRST_TRAFFIC_WALSH_CODE + offset) % range;
            if !self.in_use[i] {
                self.in_use[i] = true;
                self.next_start =
                    FIRST_TRAFFIC_WALSH_CODE + (i - FIRST_TRAFFIC_WALSH_CODE + 1) % range;
                return Some(i as u8);
            }
        }
        None
    }

    /// Release a Walsh code back to the pool.
    pub fn release(&mut self, code: u8) {
        if (code as usize) < 64 {
            self.in_use[code as usize] = false;
        }
    }

    /// Allocate a W(4), W(8), W(16), or W(32) code for F-SCH.
    pub fn allocate_sch(&mut self, walsh_len: usize) -> Option<u8> {
        if !matches!(walsh_len, 4 | 8 | 16 | 32) {
            return None;
        }
        let aliases = 64 / walsh_len;
        let first_code = FIRST_TRAFFIC_WALSH_CODE.div_ceil(aliases);
        let code_count = walsh_len;
        for code in first_code..code_count {
            let base = code * aliases;
            if (base..base + aliases).all(|i| !self.in_use[i]) {
                for i in base..base + aliases {
                    self.in_use[i] = true;
                }
                return Some(code as u8);
            }
        }
        None
    }

    /// Release a W(4), W(8), W(16), or W(32) F-SCH code.
    pub fn release_sch(&mut self, walsh_len: usize, code: u8) {
        if !matches!(walsh_len, 4 | 8 | 16 | 32) {
            return;
        }
        let aliases = 64 / walsh_len;
        let base = (code as usize) * aliases;
        if base + aliases <= 64 {
            for i in base..base + aliases {
                self.in_use[i] = false;
            }
        }
    }
}

/// Handle returned by `Bts::new_with_settings()` for the BSC to observe BTS state.
///
/// The BTS owns the sender sides; the BSC holds this handle with the receivers.
/// `watch` channels provide latest-value semantics for metrics (no backpressure).
/// `mpsc` channels carry discrete events that must not be dropped.
pub struct BtsHandle {
    pub tx_metrics: watch::Receiver<TxMetrics>,
    pub rx_metrics: watch::Receiver<RxMetrics>,
    pub config: Arc<BtsRuntimeSettings>,
    pub access_events: mpsc::UnboundedReceiver<AccessChannelEvent>,
    /// Decoded HRPD reverse access events emitted by the BTS receiver.
    pub hrpd_access_events: mpsc::UnboundedReceiver<HrpdAccessIndication>,
    /// Decoded HRPD reverse traffic events emitted by the BTS receiver.
    pub hrpd_traffic_events: mpsc::UnboundedReceiver<HrpdTrafficEvent>,
    pub commands: mpsc::Sender<BtsCommand>,
    /// Queue AN-originated HRPD Default Signaling for the Forward Control Channel.
    pub hrpd_forward_signaling: mpsc::UnboundedSender<HrpdForwardSignalingRequest>,
    /// Queue AN-originated HRPD traffic-resource assignments for the forward MAC.
    pub hrpd_traffic_assignments: mpsc::UnboundedSender<HrpdTrafficAssignmentRequest>,
    /// Queue AN-originated HRPD traffic releases (session closed/replaced).
    pub hrpd_traffic_releases: mpsc::UnboundedSender<HrpdTrafficReleaseRequest>,
    /// Queue HRPD Forward Traffic Channel physical packets for the forward MAC.
    pub hrpd_forward_traffic: ForwardTrafficSender,
    /// Shared pool of active forward traffic channels.
    pub traffic_channels: TrafficChannelPool,
    /// BTS-local reverse power-control state keyed by traffic Walsh code.
    pub power_control: BtsPowerControlRegistry,
    /// Walsh code allocator for traffic channels.
    pub walsh_allocator: Arc<Mutex<WalshAllocator>>,
    /// Shared pool of active reverse traffic channel receivers.
    pub traffic_rx_pool: TrafficRxPool,
    /// Lock-free HRPD reverse traffic worker lifecycle command queue.
    pub hrpd_traffic_rx_queue: HrpdTrafficRxQueue,
    /// Shared list of Walsh codes to remove from RX processing.
    pub traffic_rx_removals: TrafficRxRemovals,
    /// Shared access-channel signal quality store, written by BTS RX.
    pub rx_measurements: super::settings::RxMeasurementStore,
}

impl BtsHandle {
    /// Add a forward traffic channel with the given long code generator.
    /// Returns the Walsh code and a reference to the underlying channel for sending frames.
    pub fn add_traffic_channel(
        &self,
        lc_generator: LongCodeGenerator,
        initial_lc_chip: u64,
    ) -> Option<(u8, TrafficWalshChannel)> {
        allocate_traffic_channel(
            &self.walsh_allocator,
            &self.traffic_channels,
            lc_generator,
            initial_lc_chip,
            0,
        )
    }

    /// Remove a traffic channel by Walsh code and free it.
    pub fn remove_traffic_channel(&self, walsh_code: u8) {
        deallocate_traffic_channel(&self.walsh_allocator, &self.traffic_channels, walsh_code);
        self.power_control.remove(walsh_code);
    }
}

/// Allocate a forward traffic channel on the given pool/allocator (RC1).
///
/// This is the shared implementation used by both `BtsHandle::add_traffic_channel`
/// and external callers (e.g. the BSC) that hold their own `Arc` references.
pub fn allocate_traffic_channel(
    walsh_allocator: &Arc<Mutex<WalshAllocator>>,
    traffic_channels: &TrafficChannelPool,
    lc_generator: LongCodeGenerator,
    _initial_lc_chip: u64,
    fpc_subchan_gain: u8,
) -> Option<(u8, TrafficWalshChannel)> {
    let walsh_code = walsh_allocator.lock().allocate()?;

    let fpc_subchan_gain_linear = fpc_subchan_gain_to_linear(fpc_subchan_gain);
    let ftch = WalshChannel::new(
        WalshGenerator::new::<64>(walsh_code as usize, 1),
        ForwardTrafficChannel::new(ftch::Config {
            encoder: get_1_2_k9_encoder(),
            interleaver: BitReversalInterleaver::new(SR1_PARAMS_384),
            long_code_generator: lc_generator,
            lc_chip_cursor: 0,
            pcb_scheduler: PcgPcbScheduler::new_named_with_fallback(
                0,
                walsh_code,
                format!("rc1-w{}", walsh_code),
                PcgPcbFallbackMode::UpBeforeFirstThenHold,
            ),
            fpc_subchan_gain_linear,
            previous_pcg_pc_start: 0,
        }),
    );

    // LC alignment is deferred — the TX loop will call advance_lc_to_chip()
    // on first use, using its live chip_cursor as the source of truth.

    let channel_ref = ftch.clone();

    traffic_channels.add(TrafficChannelSlot::new(
        walsh_code,
        TrafficChannelWrapper::Rc1(ftch),
        RC1_TRAFFIC_INITIAL_GAIN_LINEAR,
    ));

    Some((walsh_code, channel_ref))
}

/// Update a previously-allocated traffic channel's composite gain by
/// Walsh code. Used by the BSC's forward power-control outer loop to
/// boost or attenuate a single mobile's F-FCH relative to the rest of
/// the forward channels. Returns `true` if the channel was found.
pub fn set_traffic_channel_gain(
    traffic_channels: &TrafficChannelPool,
    walsh_code: u8,
    new_gain_linear: f32,
) -> bool {
    if let Some(slot) = traffic_channels.lookup(walsh_code) {
        slot.set_gain(new_gain_linear);
        true
    } else {
        false
    }
}

fn fpc_subchan_gain_to_linear(fpc_subchan_gain: u8) -> f32 {
    let gain_db = fpc_subchan_gain as f32 * 0.25;
    10f32.powf(gain_db / 20.0)
}

/// Allocate a forward traffic channel on the given pool/allocator (RC3).
///
/// RC3 uses R=1/4 K=9 encoding, 768-symbol forward-backwards interleaver
/// with I/Q demux producing 384 QPSK symbols per 20ms frame. Walsh
/// spreading uses W(n,64): 384 complex symbols × 64 chips/symbol = 24,576
/// chips = 20ms at 1.2288 Mcps. Per Table 3.1.3.1.2.1-19.
pub fn allocate_traffic_channel_rc3(
    walsh_allocator: &Arc<Mutex<WalshAllocator>>,
    traffic_channels: &TrafficChannelPool,
    lc_generator: LongCodeGenerator,
    _initial_lc_chip: u64,
    fpc_subchan_gain: u8,
) -> Option<(u8, TrafficWalshChannelRc3)> {
    let walsh_code = walsh_allocator.lock().allocate()?;

    let fpc_subchan_gain_linear = fpc_subchan_gain_to_linear(fpc_subchan_gain);

    // The RC3 F-FCH TX uses two independent LC generators (one for
    // scrambling, one for PC puncture position extraction) that are
    // seeded together by `advance_lc_to_chip` later in the TX loop.
    let scrambling_lc = lc_generator.clone();
    let puncture_lc = lc_generator;

    let ftch = WalshChannel::new(
        WalshGenerator::new::<64>(walsh_code as usize, 1),
        ForwardTrafficChannelRc3::new(ftch_rc3::ConfigRc3 {
            encoder: get_1_4_k9_encoder(),
            interleaver: ForwardBackwardsBitReversalInterleaver::new(SR1_PARAMS_768),
            scrambling_lc,
            puncture_lc,
            lc_chip_cursor: 0,
            pcb_scheduler: PcgPcbScheduler::new_named_with_fallback(
                0,
                walsh_code,
                format!("rc3-w{}", walsh_code),
                PcgPcbFallbackMode::UpBeforeFirstThenHold,
            ),
            fpc_subchan_gain_linear,
            prev_frame_last_chip: 0,
            disable_lc_scrambling: false,
        }),
    );

    // LC alignment is deferred — the TX loop will align on first use.

    let channel_ref = ftch.clone();

    traffic_channels.add(TrafficChannelSlot::new(
        walsh_code,
        TrafficChannelWrapper::Rc3(ftch),
        RC3_TRAFFIC_INITIAL_GAIN_LINEAR,
    ));

    Some((walsh_code, channel_ref))
}

/// Commit a pre-reserved walsh code as an RC1 forward traffic channel.
///
/// Unlike `allocate_traffic_channel`, the walsh code has already been
/// reserved via `WalshAllocator::allocate()`. This only builds the
/// channel object and pushes it to the pool.
pub fn commit_traffic_channel(
    traffic_channels: &TrafficChannelPool,
    walsh_code: u8,
    lc_generator: LongCodeGenerator,
    fpc_subchan_gain: u8,
) -> TrafficWalshChannel {
    let fpc_subchan_gain_linear = fpc_subchan_gain_to_linear(fpc_subchan_gain);
    let ftch = WalshChannel::new(
        WalshGenerator::new::<64>(walsh_code as usize, 1),
        ForwardTrafficChannel::new(ftch::Config {
            encoder: get_1_2_k9_encoder(),
            interleaver: BitReversalInterleaver::new(SR1_PARAMS_384),
            long_code_generator: lc_generator,
            lc_chip_cursor: 0,
            pcb_scheduler: PcgPcbScheduler::new_named_with_fallback(
                0,
                walsh_code,
                format!("rc1-w{}", walsh_code),
                PcgPcbFallbackMode::UpBeforeFirstThenHold,
            ),
            fpc_subchan_gain_linear,
            previous_pcg_pc_start: 0,
        }),
    );

    let channel_ref = ftch.clone();

    traffic_channels.add(TrafficChannelSlot::new(
        walsh_code,
        TrafficChannelWrapper::Rc1(ftch),
        RC1_TRAFFIC_INITIAL_GAIN_LINEAR,
    ));

    channel_ref
}

/// Commit a pre-reserved walsh code as an RC3 forward traffic channel.
///
/// Unlike `allocate_traffic_channel_rc3`, the walsh code has already been
/// reserved via `WalshAllocator::allocate()`. This only builds the
/// channel object and pushes it to the pool.
pub fn commit_traffic_channel_rc3(
    traffic_channels: &TrafficChannelPool,
    walsh_code: u8,
    lc_generator: LongCodeGenerator,
    fpc_subchan_gain: u8,
) -> TrafficWalshChannelRc3 {
    let fpc_subchan_gain_linear = fpc_subchan_gain_to_linear(fpc_subchan_gain);

    let scrambling_lc = lc_generator.clone();
    let puncture_lc = lc_generator;

    let ftch = WalshChannel::new(
        WalshGenerator::new::<64>(walsh_code as usize, 1),
        ForwardTrafficChannelRc3::new(ftch_rc3::ConfigRc3 {
            encoder: get_1_4_k9_encoder(),
            interleaver: ForwardBackwardsBitReversalInterleaver::new(SR1_PARAMS_768),
            scrambling_lc,
            puncture_lc,
            lc_chip_cursor: 0,
            pcb_scheduler: PcgPcbScheduler::new_named_with_fallback(
                0,
                walsh_code,
                format!("rc3-w{}", walsh_code),
                PcgPcbFallbackMode::UpBeforeFirstThenHold,
            ),
            fpc_subchan_gain_linear,
            prev_frame_last_chip: 0,
            disable_lc_scrambling: false,
        }),
    );

    let channel_ref = ftch.clone();

    traffic_channels.add(TrafficChannelSlot::new(
        walsh_code,
        TrafficChannelWrapper::Rc3(ftch),
        RC3_TRAFFIC_INITIAL_GAIN_LINEAR,
    ));

    channel_ref
}

/// Allocate an RC2 forward traffic channel.
pub fn allocate_traffic_channel_rc2(
    walsh_allocator: &Arc<Mutex<WalshAllocator>>,
    traffic_channels: &TrafficChannelPool,
    lc_generator: LongCodeGenerator,
    _initial_lc_chip: u64,
    fpc_subchan_gain: u8,
) -> Option<(u8, TrafficWalshChannelRc2)> {
    let walsh_code = walsh_allocator.lock().allocate()?;
    let fpc_subchan_gain_linear = fpc_subchan_gain_to_linear(fpc_subchan_gain);

    let ftch = WalshChannel::new(
        WalshGenerator::new::<64>(walsh_code as usize, 1),
        ForwardTrafficChannelRc2::new(ConfigRc2 {
            long_code_generator: lc_generator,
            lc_chip_cursor: 0,
            pcb_scheduler: PcgPcbScheduler::new_named_with_fallback(
                0,
                walsh_code,
                format!("rc2-w{}", walsh_code),
                PcgPcbFallbackMode::UpBeforeFirstThenHold,
            ),
            fpc_subchan_gain_linear,
            previous_pcg_pc_start: 0,
        }),
    );

    // LC alignment is deferred — the TX loop will align on first use.

    let channel_ref = ftch.clone();

    traffic_channels.add(TrafficChannelSlot::new(
        walsh_code,
        TrafficChannelWrapper::Rc2(ftch),
        RC2_TRAFFIC_INITIAL_GAIN_LINEAR,
    ));

    Some((walsh_code, channel_ref))
}

/// Commit a pre-reserved walsh code as an RC2 forward traffic channel.
///
/// Unlike `allocate_traffic_channel_rc2`, the walsh code has already been
/// reserved via `WalshAllocator::allocate()`. This only builds the
/// channel object and pushes it to the pool.
pub fn commit_traffic_channel_rc2(
    traffic_channels: &TrafficChannelPool,
    walsh_code: u8,
    lc_generator: LongCodeGenerator,
    fpc_subchan_gain: u8,
) -> TrafficWalshChannelRc2 {
    let fpc_subchan_gain_linear = fpc_subchan_gain_to_linear(fpc_subchan_gain);
    let ftch = WalshChannel::new(
        WalshGenerator::new::<64>(walsh_code as usize, 1),
        ForwardTrafficChannelRc2::new(ConfigRc2 {
            long_code_generator: lc_generator,
            lc_chip_cursor: 0,
            pcb_scheduler: PcgPcbScheduler::new_named_with_fallback(
                0,
                walsh_code,
                format!("rc2-w{}", walsh_code),
                PcgPcbFallbackMode::UpBeforeFirstThenHold,
            ),
            fpc_subchan_gain_linear,
            previous_pcg_pc_start: 0,
        }),
    );

    let channel_ref = ftch.clone();

    traffic_channels.add(TrafficChannelSlot::new(
        walsh_code,
        TrafficChannelWrapper::Rc2(ftch),
        RC2_TRAFFIC_INITIAL_GAIN_LINEAR,
    ));

    channel_ref
}

/// Allocate an RC3 Forward Supplemental Channel (F-SCH).
///
/// Uses the Walsh length required by the selected SCH profile.
/// The SCH uses the same PLCM as the paired F-FCH (same ESN).
/// Returns the SCH Walsh code index and a cloned reference to the channel.
pub fn allocate_sch_rc3(
    walsh_allocator: &Arc<Mutex<WalshAllocator>>,
    traffic_channels: &TrafficChannelPool,
    lc_generator: LongCodeGenerator,
    sch_gain_linear: f32,
    profile: Rc3FschProfile,
    start_time_unit: u8,
    start_time_mod32: u8,
) -> Option<(u8, SchWalshChannelRc3)> {
    let sch_code = walsh_allocator.lock().allocate_sch(profile.walsh_len)?;

    let scrambling_lc = lc_generator.clone();
    let puncture_lc = lc_generator;
    let walsh = match profile.walsh_len {
        4 => WalshGenerator::new::<4>(sch_code as usize, 1),
        8 => WalshGenerator::new::<8>(sch_code as usize, 1),
        16 => WalshGenerator::new::<16>(sch_code as usize, 1),
        32 => WalshGenerator::new::<32>(sch_code as usize, 1),
        _ => return None,
    };

    let sch = WalshChannel::new(
        walsh,
        ForwardSupplementalChannelRc3::new(SchConfigRc3 {
            profile,
            encoder: get_1_4_k9_encoder(),
            interleaver: ForwardBackwardsBitReversalInterleaver::new(interleaver_params(profile)),
            scrambling_lc,
            puncture_lc,
            lc_chip_cursor: 0,
            sch_gain_linear,
            prev_frame_last_chip: 0,
            frame_pcg_index: 0,
            disable_lc_scrambling: false,
            assignment_start: Some((
                start_time_unit,
                start_time_mod32 % crate::channels::f_sch_rc3::SCH_START_TIME_MODULUS,
            )),
            assignment_started: false,
        }),
    );

    let channel_ref = sch.clone();

    traffic_channels.add(TrafficChannelSlot::new(
        sch_code,
        TrafficChannelWrapper::SchRc3(sch),
        sch_gain_linear,
    ));

    Some((sch_code, channel_ref))
}

/// Deallocate an F-SCH by code and free it from the pool/allocator.
pub fn deallocate_sch(
    walsh_allocator: &Arc<Mutex<WalshAllocator>>,
    traffic_channels: &TrafficChannelPool,
    sch_code: u8,
) {
    let walsh_len = match traffic_channels.lookup(sch_code) {
        Some(slot) => match &slot.channel {
            TrafficChannelWrapper::SchRc3(ch) => Some(ch.channel.profile().walsh_len),
            _ => None,
        },
        None => None,
    };
    if walsh_len.is_some() {
        traffic_channels.remove(sch_code);
    }
    if let Some(walsh_len) = walsh_len {
        walsh_allocator.lock().release_sch(walsh_len, sch_code);
        log::info!(
            "deallocate_sch: released W({}) code {}",
            walsh_len,
            sch_code
        );
    } else {
        log::warn!("deallocate_sch: no active SCH code {} found", sch_code);
    }
}

/// Deallocate a traffic channel by Walsh code and free it from the pool/allocator.
pub fn deallocate_traffic_channel(
    walsh_allocator: &Arc<Mutex<WalshAllocator>>,
    traffic_channels: &TrafficChannelPool,
    walsh_code: u8,
) {
    let before = traffic_channels.len();
    traffic_channels.remove(walsh_code);
    let after = traffic_channels.len();
    log::info!(
        "deallocate_traffic_channel: walsh={} pool_before={} pool_after={}",
        walsh_code,
        before,
        after
    );
    walsh_allocator.lock().release(walsh_code);
}

/// Internal sender-side state held by the BTS. Not exposed publicly.
pub(crate) struct BtsHandleSenders {
    pub tx_metrics: watch::Sender<TxMetrics>,
    pub rx_metrics: watch::Sender<RxMetrics>,
    pub access_event_tx: mpsc::UnboundedSender<AccessChannelEvent>,
    pub hrpd_access_event_tx: mpsc::UnboundedSender<HrpdAccessIndication>,
    pub hrpd_traffic_event_tx: mpsc::UnboundedSender<HrpdTrafficEvent>,
    pub hrpd_forward_signaling_rx: mpsc::UnboundedReceiver<HrpdForwardSignalingRequest>,
    pub hrpd_traffic_assignment_rx: mpsc::UnboundedReceiver<HrpdTrafficAssignmentRequest>,
    pub hrpd_traffic_release_rx: mpsc::UnboundedReceiver<HrpdTrafficReleaseRequest>,
    pub hrpd_forward_traffic_rx: mpsc::UnboundedReceiver<PreparedForwardTrafficPacket>,
    pub commands_rx: mpsc::Receiver<BtsCommand>,
    /// Shared reference to the traffic channel pool (same Arc as BtsHandle).
    pub traffic_channels: TrafficChannelPool,
    /// Shared reference to the traffic RX pool (same Arc as BtsHandle).
    pub traffic_rx_pool: TrafficRxPool,
    /// Shared reference to the HRPD reverse traffic command queue (same Arc as
    /// BtsHandle).
    pub hrpd_traffic_rx_queue: HrpdTrafficRxQueue,
    /// Shared reference to the traffic RX removals list (same Arc as BtsHandle).
    pub traffic_rx_removals: TrafficRxRemovals,
    /// BTS-local reverse power-control state keyed by traffic Walsh code.
    pub power_control: BtsPowerControlRegistry,
    /// Shared access-channel signal quality store, written by BTS RX.
    pub rx_measurements: super::settings::RxMeasurementStore,
    /// Shared H-ARQ bus (same Arc the handle's forward-traffic preparer holds).
    pub hrpd_harq_bus: Arc<HarqBus>,
}

/// Create a matched pair of (senders for BTS internals, handle for BSC).
pub(crate) fn create_handle(config: Arc<BtsRuntimeSettings>) -> (BtsHandleSenders, BtsHandle) {
    let (tx_metrics_tx, tx_metrics_rx) = watch::channel(TxMetrics::default());
    let (rx_metrics_tx, rx_metrics_rx) = watch::channel(RxMetrics::default());
    let (access_tx, access_rx) = mpsc::unbounded_channel();
    let (hrpd_access_tx, hrpd_access_rx) = mpsc::unbounded_channel();
    let (hrpd_traffic_event_tx, hrpd_traffic_event_rx) = mpsc::unbounded_channel();
    let (hrpd_forward_tx, hrpd_forward_rx) = mpsc::unbounded_channel();
    let (hrpd_traffic_tx, hrpd_traffic_rx) = mpsc::unbounded_channel();
    let (hrpd_traffic_release_tx, hrpd_traffic_release_rx) = mpsc::unbounded_channel();
    let (hrpd_forward_traffic_tx, hrpd_forward_traffic_rx) = mpsc::unbounded_channel();
    let (commands_tx, commands_rx) = mpsc::channel(16);

    let traffic_channels: TrafficChannelPool = Arc::new(ChannelRegistry::new());
    let traffic_rx_pool: TrafficRxPool = Arc::new(Mutex::new(Vec::new()));
    let hrpd_traffic_rx_queue: HrpdTrafficRxQueue =
        Arc::new(ArrayQueue::new(HRPD_TRAFFIC_RX_QUEUE_CAPACITY));
    let traffic_rx_removals: TrafficRxRemovals = Arc::new(Mutex::new(Vec::new()));
    let power_control = BtsPowerControlRegistry::default();
    let rx_measurements: super::settings::RxMeasurementStore =
        Arc::new(std::sync::Mutex::new(std::collections::HashMap::new()));

    let hrpd_harq_bus: Arc<HarqBus> = Arc::new(HarqBus::new());

    let mut walsh_alloc = WalshAllocator::new();
    walsh_alloc.reserve_system_channels(
        config.downlink.pilot.walsh_code as u8,
        config.downlink.paging.walsh_code as u8,
        config.downlink.sync.walsh_code as u8,
    );

    let senders = BtsHandleSenders {
        tx_metrics: tx_metrics_tx,
        rx_metrics: rx_metrics_tx,
        access_event_tx: access_tx,
        hrpd_access_event_tx: hrpd_access_tx,
        hrpd_traffic_event_tx,
        hrpd_forward_signaling_rx: hrpd_forward_rx,
        hrpd_traffic_assignment_rx: hrpd_traffic_rx,
        hrpd_traffic_release_rx,
        hrpd_forward_traffic_rx,
        commands_rx,
        traffic_channels: traffic_channels.clone(),
        traffic_rx_pool: traffic_rx_pool.clone(),
        hrpd_traffic_rx_queue: hrpd_traffic_rx_queue.clone(),
        traffic_rx_removals: traffic_rx_removals.clone(),
        power_control: power_control.clone(),
        rx_measurements: rx_measurements.clone(),
        hrpd_harq_bus: hrpd_harq_bus.clone(),
    };

    let handle = BtsHandle {
        tx_metrics: tx_metrics_rx,
        rx_metrics: rx_metrics_rx,
        config,
        access_events: access_rx,
        hrpd_access_events: hrpd_access_rx,
        hrpd_traffic_events: hrpd_traffic_event_rx,
        commands: commands_tx,
        hrpd_forward_signaling: hrpd_forward_tx,
        hrpd_traffic_assignments: hrpd_traffic_tx,
        hrpd_traffic_releases: hrpd_traffic_release_tx,
        hrpd_forward_traffic: ForwardTrafficSender::new(
            hrpd_forward_traffic_tx,
            ForwardTrafficPreparer::new(hrpd_harq_bus),
        ),
        traffic_channels,
        power_control,
        walsh_allocator: Arc::new(Mutex::new(walsh_alloc)),
        traffic_rx_pool,
        hrpd_traffic_rx_queue,
        traffic_rx_removals,
        rx_measurements,
    };

    (senders, handle)
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use parking_lot::Mutex;

    use super::{ChannelRegistry, TrafficChannelPool, WalshAllocator, deallocate_sch};

    #[test]
    fn walsh_allocator_starts_traffic_pool_at_ten() {
        let mut alloc = WalshAllocator::new();
        alloc.reserve_system_channels(0, 1, 32);

        assert_eq!(alloc.allocate(), Some(10));
        assert_eq!(alloc.allocate(), Some(11));
    }

    #[test]
    fn sch_allocator_returns_aligned_walsh_codes() {
        let mut alloc = WalshAllocator::new();
        alloc.reserve_system_channels(0, 1, 32);

        assert_eq!(alloc.allocate_sch(16), Some(3));
        assert!(alloc.in_use[12]);
        assert!(alloc.in_use[13]);
        assert!(alloc.in_use[14]);
        assert!(alloc.in_use[15]);
        assert!(!alloc.in_use[10]);
        assert!(!alloc.in_use[11]);
        assert!(!alloc.in_use[16]);
        assert!(!alloc.in_use[17]);
    }

    #[test]
    fn sch_allocator_avoids_fch_alias_overlap() {
        let mut alloc = WalshAllocator::new();
        alloc.reserve_system_channels(0, 1, 32);

        assert_eq!(alloc.allocate(), Some(10));
        assert_eq!(alloc.allocate(), Some(11));
        assert_eq!(alloc.allocate(), Some(12));
        assert_eq!(alloc.allocate(), Some(13));

        assert_eq!(alloc.allocate_sch(16), Some(4));
        assert!(alloc.in_use[16]);
        assert!(alloc.in_use[17]);
        assert!(alloc.in_use[18]);
        assert!(alloc.in_use[19]);
    }

    #[test]
    fn sch_allocator_reserves_full_w4_subtree() {
        let mut alloc = WalshAllocator::new();
        alloc.reserve_system_channels(0, 1, 32);

        assert_eq!(alloc.allocate_sch(4), Some(1));
        for code in 16..32 {
            assert!(alloc.in_use[code], "W64 descendant {code} must be blocked");
        }
        assert!(!alloc.in_use[33]);

        assert_eq!(alloc.allocate_sch(4), Some(3));
        for code in 48..64 {
            assert!(alloc.in_use[code], "W64 descendant {code} must be blocked");
        }

        assert_eq!(alloc.allocate_sch(4), None);
    }

    #[test]
    fn deallocate_sch_missing_slot_does_not_free_aliases() {
        let allocator = Arc::new(Mutex::new(WalshAllocator::new()));
        allocator.lock().reserve_system_channels(0, 1, 32);
        assert_eq!(allocator.lock().allocate(), Some(10));
        assert!(allocator.lock().in_use[10]);

        let traffic_channels: TrafficChannelPool = Arc::new(ChannelRegistry::new());
        deallocate_sch(&allocator, &traffic_channels, 5);

        assert!(
            allocator.lock().in_use[10],
            "missing SCH deallocation must not release W64 aliases owned by other channels"
        );
    }
}
