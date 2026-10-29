//! BTS traffic resource ownership and per-channel long-code transition state.

use std::collections::HashMap;
use std::sync::Arc;

use parking_lot::Mutex;

use super::handle::{
    SchWalshChannelRc3, TrafficChannelPool, TrafficRxPool, TrafficRxRemovals, TrafficRxRequest,
    TrafficWalshChannel, TrafficWalshChannelRc2, TrafficWalshChannelRc3, WalshAllocator,
    allocate_sch_rc3, allocate_traffic_channel, allocate_traffic_channel_rc2,
    allocate_traffic_channel_rc3, commit_traffic_channel, commit_traffic_channel_rc2,
    commit_traffic_channel_rc3, deallocate_sch, deallocate_traffic_channel,
    set_traffic_channel_gain,
};
use super::power_control::BtsPowerControlRegistry;
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
    power_control: BtsPowerControlRegistry,
    privacy: Arc<Mutex<HashMap<u8, super::voice_privacy::ForwardPrivacy>>>,
}

impl TrafficResourceService {
    /// Build a controller from existing pool `Arc`s (the typical
    /// bootstrap path: take the pools off `BtsHandle`).
    pub fn from_pools(
        walsh_allocator: Arc<Mutex<WalshAllocator>>,
        traffic_channels: TrafficChannelPool,
        traffic_rx_pool: TrafficRxPool,
        traffic_rx_removals: TrafficRxRemovals,
        power_control: BtsPowerControlRegistry,
    ) -> Self {
        Self {
            walsh_allocator,
            traffic_channels,
            traffic_rx_pool,
            traffic_rx_removals,
            power_control,
            privacy: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    /// Build a controller with fresh empty pools. Used by tests that
    /// don't bring up a full `Bts`.
    pub fn new() -> Self {
        Self {
            walsh_allocator: Arc::new(Mutex::new(WalshAllocator::new())),
            traffic_channels: Arc::new(super::handle::ChannelRegistry::new()),
            traffic_rx_pool: Arc::new(Mutex::new(Vec::new())),
            traffic_rx_removals: Arc::new(Mutex::new(Vec::new())),
            power_control: BtsPowerControlRegistry::default(),
            privacy: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    pub fn long_code_privacy(
        &self,
        walsh_code: u8,
    ) -> Option<Arc<cdma_common::phy::long_code_privacy::LongCodePrivacy>> {
        self.privacy
            .lock()
            .get(&walsh_code)
            .map(|observer| observer.privacy.clone())
    }

    pub(super) fn observe_forward_privacy(&self, walsh_code: u8, info_bits: &[u8]) {
        if let Some(observer) = self.privacy.lock().get_mut(&walsh_code) {
            observer.observe(info_bits);
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
        fpc_subchan_gain: u8,
    ) -> TrafficWalshChannel {
        if let Some(privacy) = lc_generator.privacy() {
            self.privacy.lock().insert(
                walsh_code,
                super::voice_privacy::ForwardPrivacy::new(privacy),
            );
        }
        commit_traffic_channel(
            &self.traffic_channels,
            walsh_code,
            lc_generator,
            fpc_subchan_gain,
        )
    }

    /// Commit a previously-reserved walsh code as an RC2 forward traffic
    /// channel. Returns a clone of the live channel object.
    pub fn commit_rc2_traffic(
        &self,
        walsh_code: u8,
        lc_generator: LongCodeGenerator,
        fpc_subchan_gain: u8,
    ) -> TrafficWalshChannelRc2 {
        if let Some(privacy) = lc_generator.privacy() {
            self.privacy.lock().insert(
                walsh_code,
                super::voice_privacy::ForwardPrivacy::new(privacy),
            );
        }
        commit_traffic_channel_rc2(
            &self.traffic_channels,
            walsh_code,
            lc_generator,
            fpc_subchan_gain,
        )
    }

    /// Commit a previously-reserved walsh code as an RC3 forward traffic
    /// channel. Returns a clone of the live channel object.
    pub fn commit_rc3_traffic(
        &self,
        walsh_code: u8,
        lc_generator: LongCodeGenerator,
        fpc_subchan_gain: u8,
    ) -> TrafficWalshChannelRc3 {
        if let Some(privacy) = lc_generator.privacy() {
            self.privacy.lock().insert(
                walsh_code,
                super::voice_privacy::ForwardPrivacy::new(privacy),
            );
        }
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
        fpc_subchan_gain: u8,
    ) -> Option<(u8, TrafficWalshChannel)> {
        allocate_traffic_channel(
            &self.walsh_allocator,
            &self.traffic_channels,
            lc_generator,
            initial_lc_chip,
            fpc_subchan_gain,
        )
    }

    /// Allocate an RC2 forward traffic channel (IS-95B Rate Set 2 —
    /// QCELP-13K). Same migration note as `allocate_rc1_traffic`.
    pub fn allocate_rc2_traffic(
        &self,
        lc_generator: LongCodeGenerator,
        initial_lc_chip: u64,
        fpc_subchan_gain: u8,
    ) -> Option<(u8, TrafficWalshChannelRc2)> {
        allocate_traffic_channel_rc2(
            &self.walsh_allocator,
            &self.traffic_channels,
            lc_generator,
            initial_lc_chip,
            fpc_subchan_gain,
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

    /// Allocate an RC3 forward Supplemental Channel (F-SCH).
    pub fn allocate_rc3_sch(
        &self,
        lc_generator: LongCodeGenerator,
        sch_gain_linear: f32,
        profile: cdma_common::sch::Rc3FschProfile,
        start_time_unit: u8,
        start_time_mod32: u8,
    ) -> Option<(u8, SchWalshChannelRc3)> {
        allocate_sch_rc3(
            &self.walsh_allocator,
            &self.traffic_channels,
            lc_generator,
            sch_gain_linear,
            profile,
            start_time_unit,
            start_time_mod32,
        )
    }

    /// Deallocate a forward traffic channel by Walsh code.
    pub fn deallocate_traffic(&self, walsh_code: u8) {
        self.privacy.lock().remove(&walsh_code);
        deallocate_traffic_channel(&self.walsh_allocator, &self.traffic_channels, walsh_code);
        self.power_control.remove(walsh_code);
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

#[cfg(test)]
mod tests {
    use super::TrafficResourceService;

    #[test]
    fn traffic_deallocation_removes_power_control_state() {
        let service = TrafficResourceService::new();
        service.power_control.set_target(12, 0.0, false);
        assert!(service.power_control.snapshot(12).is_some());

        service.deallocate_traffic(12);

        assert!(service.power_control.snapshot(12).is_none());
    }
}
