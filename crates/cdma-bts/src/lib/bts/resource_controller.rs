//! BTS-owned traffic resource controller.
//!
//! `TrafficResourceService` is the sole intended *mutator* of the BTS
//! traffic-channel pool, the Walsh allocator, the reverse-traffic RX pool,
//! and the RX-removal queue. It exists so the BSC stops touching those
//! pools directly — instead the BSC calls into a `BtsControlClient` trait
//! whose in-process implementation forwards to this controller.
//!
//! WS-0 PR2 scope: the controller is a thin façade that wraps the existing
//! shared pool `Arc`s already exposed on `BtsHandle`. Both paths refer to
//! the same underlying state for now (BTS internal TX/RX threads continue
//! to read the pools via `BtsHandle`); a later PR will re-home pool
//! ownership entirely inside the controller and remove the duplicate
//! exposure on `BtsHandle`.

use std::sync::Arc;

use parking_lot::Mutex;

use super::handle::{
    SchWalshChannelRc3, TrafficChannelPool, TrafficRxPool, TrafficRxRemovals, TrafficRxRequest,
    TrafficWalshChannel, TrafficWalshChannelRc3, WalshAllocator, allocate_sch_rc3,
    allocate_traffic_channel, allocate_traffic_channel_rc3, commit_traffic_channel,
    commit_traffic_channel_rc3, deallocate_sch, deallocate_traffic_channel,
    set_traffic_channel_gain,
};
use crate::phy::coding::long_code::LongCodeGenerator;

/// BTS-owned façade over the traffic-channel resource pools.
///
/// Construct one per BTS at bootstrap (typically from the pool `Arc`s
/// exposed by `BtsHandle`) and inject it into the in-process
/// `BtsControlClient` adapter. The BSC must never touch the underlying
/// pools directly — all mutation goes through methods on this type.
#[derive(Clone)]
pub struct TrafficResourceService {
    walsh_allocator: Arc<Mutex<WalshAllocator>>,
    traffic_channels: TrafficChannelPool,
    traffic_rx_pool: TrafficRxPool,
    traffic_rx_removals: TrafficRxRemovals,
}

impl TrafficResourceService {
    /// Build a controller from existing pool `Arc`s (the typical
    /// bootstrap path: take the pools off `BtsHandle`).
    pub fn from_pools(
        walsh_allocator: Arc<Mutex<WalshAllocator>>,
        traffic_channels: TrafficChannelPool,
        traffic_rx_pool: TrafficRxPool,
        traffic_rx_removals: TrafficRxRemovals,
    ) -> Self {
        Self {
            walsh_allocator,
            traffic_channels,
            traffic_rx_pool,
            traffic_rx_removals,
        }
    }

    /// Build a controller with fresh empty pools. Used by tests that
    /// don't bring up a full `Bts`.
    pub fn new() -> Self {
        Self {
            walsh_allocator: Arc::new(Mutex::new(WalshAllocator::new())),
            traffic_channels: Arc::new(Mutex::new(Vec::new())),
            traffic_rx_pool: Arc::new(Mutex::new(Vec::new())),
            traffic_rx_removals: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Reserve a walsh code without creating a traffic channel.
    ///
    /// The code is marked in-use in the allocator but no
    /// `TrafficChannelSlot` is pushed to the pool. Call
    /// `commit_rc1_traffic` or `commit_rc3_traffic` later to create
    /// the actual channel once the RC is known (e.g. from an ECAM).
    pub fn reserve_walsh(&self) -> Option<u8> {
        self.walsh_allocator.lock().allocate()
    }

    /// Commit a previously-reserved walsh code as an RC1 forward traffic
    /// channel. Returns a clone of the live channel object.
    pub fn commit_rc1_traffic(
        &self,
        walsh_code: u8,
        lc_generator: LongCodeGenerator,
    ) -> TrafficWalshChannel {
        commit_traffic_channel(&self.traffic_channels, walsh_code, lc_generator)
    }

    /// Commit a previously-reserved walsh code as an RC3 forward traffic
    /// channel. Returns a clone of the live channel object.
    pub fn commit_rc3_traffic(
        &self,
        walsh_code: u8,
        lc_generator: LongCodeGenerator,
        fpc_subchan_gain: u8,
    ) -> TrafficWalshChannelRc3 {
        commit_traffic_channel_rc3(
            &self.traffic_channels,
            walsh_code,
            lc_generator,
            fpc_subchan_gain,
        )
    }

    /// Allocate an RC1 forward traffic channel. Returns the assigned
    /// Walsh code and a clone of the live channel object. The latter
    /// goes away in WS-0 PR4 when traffic channel references become
    /// opaque IDs (`CallConnectionRef`).
    pub fn allocate_rc1_traffic(
        &self,
        lc_generator: LongCodeGenerator,
        initial_lc_chip: u64,
    ) -> Option<(u8, TrafficWalshChannel)> {
        allocate_traffic_channel(
            &self.walsh_allocator,
            &self.traffic_channels,
            lc_generator,
            initial_lc_chip,
        )
    }

    /// Allocate an RC3 forward traffic channel. Same migration note as
    /// `allocate_rc1_traffic`.
    pub fn allocate_rc3_traffic(
        &self,
        lc_generator: LongCodeGenerator,
        initial_lc_chip: u64,
        fpc_subchan_gain: u8,
    ) -> Option<(u8, TrafficWalshChannelRc3)> {
        allocate_traffic_channel_rc3(
            &self.walsh_allocator,
            &self.traffic_channels,
            lc_generator,
            initial_lc_chip,
            fpc_subchan_gain,
        )
    }

    /// Allocate an RC3 forward Supplemental Channel (F-SCH) at 19.2 kbps.
    pub fn allocate_rc3_sch(
        &self,
        lc_generator: LongCodeGenerator,
        sch_gain_linear: f32,
    ) -> Option<(u8, SchWalshChannelRc3)> {
        allocate_sch_rc3(
            &self.walsh_allocator,
            &self.traffic_channels,
            lc_generator,
            sch_gain_linear,
        )
    }

    /// Deallocate a forward traffic channel by Walsh code.
    pub fn deallocate_traffic(&self, walsh_code: u8) {
        deallocate_traffic_channel(&self.walsh_allocator, &self.traffic_channels, walsh_code);
    }

    /// Deallocate an F-SCH by W(32) code.
    pub fn deallocate_sch(&self, w32_code: u8) {
        deallocate_sch(&self.walsh_allocator, &self.traffic_channels, w32_code);
    }

    /// Update an allocated traffic channel's composite gain. Returns
    /// `true` if the channel was found.
    pub fn set_traffic_gain(&self, walsh_code: u8, gain_linear: f32) -> bool {
        set_traffic_channel_gain(&self.traffic_channels, walsh_code, gain_linear)
    }

    /// Install a reverse-traffic receiver request. The BTS RX thread
    /// drains the pool and creates the actual receiver pipelines.
    pub fn install_rx_request(&self, request: TrafficRxRequest) {
        self.traffic_rx_pool.lock().push(request);
    }

    /// Drop any pending reverse-traffic RX request matching `walsh_code`
    /// (cleanup path used when the assignment fails before the RX
    /// thread picks up the request).
    pub fn drop_pending_rx_request(&self, walsh_code: u8) {
        self.traffic_rx_pool
            .lock()
            .retain(|r| r.walsh_code != walsh_code);
    }

    /// Queue a Walsh code for reverse-traffic RX teardown.
    pub fn request_rx_removal(&self, walsh_code: u8) {
        self.traffic_rx_removals.lock().push(walsh_code);
    }

    // ---- BTS-internal accessors (not for BSC use) ----
    //
    // These let the BTS RX/TX threads continue to read the underlying
    // pools directly. They are *not* part of the BSC-facing control
    // surface; the in-process `BtsControlClient` adapter does not expose
    // them. Marking them `pub` is a transitional artifact; once pool
    // ownership lives entirely inside this type, these accessors will
    // become `pub(crate)` and the duplicate exposure on `BtsHandle`
    // will be removed.

    /// BTS-internal: shared traffic-channel pool reference.
    pub fn traffic_channels_pool(&self) -> &TrafficChannelPool {
        &self.traffic_channels
    }

    /// BTS-internal: shared reverse-traffic RX request pool reference.
    pub fn traffic_rx_pool(&self) -> &TrafficRxPool {
        &self.traffic_rx_pool
    }

    /// BTS-internal: shared reverse-traffic RX removal queue reference.
    pub fn traffic_rx_removals(&self) -> &TrafficRxRemovals {
        &self.traffic_rx_removals
    }

    /// BTS-internal: shared Walsh allocator reference.
    pub fn walsh_allocator(&self) -> &Arc<Mutex<WalshAllocator>> {
        &self.walsh_allocator
    }
}

impl Default for TrafficResourceService {
    fn default() -> Self {
        Self::new()
    }
}

/// Backward-compatible alias for external crates that still reference the old name.
pub type TrafficResourceController = TrafficResourceService;
