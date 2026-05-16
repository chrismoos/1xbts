use super::handle::TrafficChannelPool;
use super::power_control::{BtsPowerControlRegistry, BtsPowerControlSnapshot, BtsPowerControlTick};

/// Service wrapper around [`BtsPowerControlRegistry`] that provides a
/// higher-level API for managing reverse-link power control state.
#[derive(Clone)]
pub struct PowerControlService {
    registry: BtsPowerControlRegistry,
}

impl PowerControlService {
    /// Create a new service backed by a fresh, empty registry.
    pub fn new() -> Self {
        Self {
            registry: BtsPowerControlRegistry::default(),
        }
    }

    /// Wrap an existing registry in the service interface.
    pub fn from_registry(registry: BtsPowerControlRegistry) -> Self {
        Self { registry }
    }

    /// Get the underlying registry for compatibility with code that still
    /// operates directly on [`BtsPowerControlRegistry`].
    pub fn registry(&self) -> &BtsPowerControlRegistry {
        &self.registry
    }

    /// Configure radio-specific dBFS threshold adjustment for raw-power
    /// brake/clamp decisions. Positive values move thresholds hotter.
    pub fn set_rx_power_adj_dbfs(&self, rx_power_adj_dbfs: f32) {
        self.registry.set_rx_power_adj_dbfs(rx_power_adj_dbfs);
    }

    /// Override the Eb/Nt target for a given walsh code.
    ///
    /// When `held` is true the target is treated as a manual override and
    /// clamped to the wider manual min/max range; when false it is treated
    /// as an auto-loop target clamped to the tighter auto range.
    pub fn set_target(&self, walsh_code: u8, target_db: f32, held: bool) {
        self.registry.set_target(walsh_code, target_db, held);
    }

    /// Run the outer-loop power control tick for a completed traffic frame.
    ///
    /// This adjusts the Eb/Nt target based on CRC pass/fail history and
    /// returns an updated snapshot of the channel state.
    pub fn outer_loop_tick(
        &self,
        traffic_channels: Option<&TrafficChannelPool>,
        walsh_code: u8,
        frame_valid: bool,
    ) -> BtsPowerControlSnapshot {
        self.registry
            .outer_loop_tick(traffic_channels, walsh_code, frame_valid)
    }

    /// Return a point-in-time snapshot for one walsh code, or `None` if the
    /// code has no power-control state.
    pub fn snapshot(&self, walsh_code: u8) -> Option<BtsPowerControlSnapshot> {
        self.registry.snapshot(walsh_code)
    }

    /// Return point-in-time snapshots for every walsh code that has
    /// power-control state.
    pub fn snapshots(&self) -> Vec<BtsPowerControlSnapshot> {
        self.registry.snapshots()
    }

    /// Run the inner-loop (per-PCG) power control tick, compute a PCB
    /// decision, and schedule it on the appropriate forward traffic channel.
    ///
    /// Returns `None` if the walsh code has no matching traffic channel slot.
    pub fn tick_and_schedule(
        &self,
        traffic_channels: &TrafficChannelPool,
        walsh_code: u8,
        measured_abs_pcg: u64,
        tx_abs_pcg: u64,
        metric_db: f32,
        raw_power_db: Option<f32>,
    ) -> Option<BtsPowerControlTick> {
        self.registry.tick_and_schedule(
            traffic_channels,
            walsh_code,
            measured_abs_pcg,
            tx_abs_pcg,
            metric_db,
            raw_power_db,
        )
    }
}

impl Default for PowerControlService {
    fn default() -> Self {
        Self::new()
    }
}
