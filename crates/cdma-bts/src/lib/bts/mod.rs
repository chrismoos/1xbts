pub mod abis_agent;
pub mod bearer_agent;
pub mod bearer_transport_service;
pub mod config;
pub mod evdo;
pub mod handle;
pub mod hrpd;
pub mod launcher;
pub mod metrics_service;
pub mod paging_service;
pub mod paging_supplier;
pub mod power_control;
pub mod power_control_service;
pub(crate) mod realtime;
pub mod resource_controller;
pub mod reverse_power_predictor;
pub mod rx;
pub mod settings;
pub mod synthesis_service;
pub mod traffic_lac;
pub mod traffic_setup_service;
pub use bearer_transport_service::BearerTransportService;
pub use config::{
    BtsAbisTimers, BtsNodeConfig, RadioConfig, ReverseRxTarget, load_radio_from_path,
};
pub use evdo::{EvdoConfig, ResolvedEvdoConfig};
pub use handle::*;
pub use launcher::*;
pub use metrics_service::MetricsService;
pub use paging_service::PagingService;
pub use paging_supplier::PchTransmitEvent;
pub use power_control::{BtsPowerControlRegistry, BtsPowerControlSnapshot, BtsPowerControlTick};
pub use power_control_service::PowerControlService;
pub use resource_controller::{TrafficResourceController, TrafficResourceService};
pub use settings::*;
pub use synthesis_service::SynthesisService;
pub use traffic_setup_service::TrafficSetupService;

mod downlink;
mod synth;
mod timing;
#[cfg(test)]
mod tx_headroom_tests;

use std::{sync::Arc, sync::atomic::AtomicBool, thread, time::Instant};

use cdma_common::{
    consts::{SR1_CHIP_RATE_HZ, SR1_CHIPS_320MS},
    error::Error,
    time,
};
use log::{debug, info, trace, warn};
use num::complex::Complex32;
use tokio::sync::mpsc;

use crate::{
    channels::{
        WalshChannelWrapper, fpch::ForwardPagingChannel, fsch::ForwardSyncChannel,
        pilot::ForwardPilotChannel,
    },
    mac,
    receiver::sync::SyncChannelMessage,
    sdr::{Radio, RadioRx, RadioTx, TxPulseShaper, TxRadioHealth, pipe::RadioPipe},
};

pub use timing::TxRxAnchor;

/// Forward MAC RPC fallback for installed traffic channels before the reverse
/// receiver has published slot-specific measurements. Per C.S0024-200-C
/// §1.3.1.2.4.2/§1.3.1.4, RPC bit '0' commands the AT up and bit '1'
/// commands it down. With no reliable measurement yet, treat the reverse link
/// as below target and command up; measured per-slot RPC bits override this
/// fallback through the HARQ bus once the reverse pilot is decoded.
fn hrpd_rpc_mode() -> (bool, bool, &'static str) {
    // When the closed-loop reverse power controller has a scheduled bit for a
    // slot it always wins (see `rpc_bit_for_slot`). This is only the fallback
    // for slots with no scheduled bit — during acquisition or a loss-of-lock
    // gap. Alternate up/down (net-neutral hold) instead of commanding a steady
    // up, so a gap does not ramp the AT to full reverse power.
    (false, true, "alternating-hold")
}

/// HRPD-only carriers run at 0.75 of the 1x backoff: 16QAM traffic has
/// ~3 dB more PAPR than the loaded 1x composite (`tx_headroom_tests`).
pub(crate) const HRPD_PAPR_HEADROOM: f32 = 0.75;

fn hrpd_only_tx_scale(tx_digital_backoff: f32) -> f32 {
    tx_digital_backoff * HRPD_PAPR_HEADROOM
}

fn one_x_synth_scale(tx_digital_backoff: f32, adjacent_composite: bool) -> f32 {
    if adjacent_composite {
        1.0
    } else {
        tx_digital_backoff
    }
}

fn hrpd_forward_ingress_budget(tx_batch_chips: u64) -> usize {
    // A forward packet can start at most once per HRPD slot. Admitting no more
    // than one batch's maximum starts bounds control-plane work on the
    // real-time TX thread while excess packets remain on the ingress queue.
    let packet_starts = tx_batch_chips.div_ceil(crate::phy::hrpd::slot::SLOT_CHIPS);
    usize::try_from(packet_starts).unwrap_or(usize::MAX)
}

/// Static BTS wiring and protocol dependencies.
pub struct Config {
    /// Resolved TX center frequency in Hz (channel plan or override).
    pub tx_center_frequency_hz: usize,
    /// Pilot PN offset index in units of 64 chips.
    pub pilot_offset: usize,
    /// MAC service used for paging fragments and overhead availability.
    pub mac_layer: mac::Layer2MacRef,
    /// When set, the BTS starts from this exact CDMA system time instead of
    /// sampling wall clock time at runtime. Useful for deterministic tests.
    pub start_system_time: Option<time::CdmaSystemTime>,
    /// When set, the BTS generates sync channel messages directly instead of
    /// going through the MAC/LAC availability-indication path. The template
    /// is re-stamped with `lc_state` and `sys_time` at each superframe start.
    pub sync_channel_template: Option<SyncChannelMessage>,
    /// Source policy + overrides for the broadcast `LTM_OFF` / `DAYLT` /
    /// `LP_SEC` fields. The static fallback values live on `overhead`.
    pub timezone: cdma_common::timezone::TimezoneConfig,
    /// Static overhead values (used as the `Overhead` source fallback and
    /// for `lp_sec` defaults).
    pub overhead: settings::OverheadParameters,
    /// Optional reverse-link RX configuration.
    pub rx: Option<RxSettings>,
    /// Resolved adjacent EV-DO/HRPD carrier configuration.
    pub evdo: Option<evdo::ResolvedEvdoConfig>,
}

/// Forward-link BTS runtime and associated control-plane endpoints.
pub struct Bts {
    config: Config,
    radio: Option<Box<dyn Radio>>,
    runtime: BtsRuntimeSettings,
    evdo: Option<evdo::ResolvedEvdoConfig>,
    injected_rx: Option<rx::InjectedRxReceiver>,
    metrics: MetricsService,
    commands_rx: Option<mpsc::Receiver<BtsCommand>>,
    hrpd_access_event_tx:
        tokio::sync::mpsc::UnboundedSender<cdma_common::hrpd::air::HrpdAccessIndication>,
    hrpd_traffic_event_tx:
        tokio::sync::mpsc::UnboundedSender<cdma_common::hrpd::air::HrpdTrafficEvent>,
    hrpd_forward_signaling_rx:
        tokio::sync::mpsc::UnboundedReceiver<cdma_common::hrpd::air::HrpdForwardSignalingRequest>,
    hrpd_traffic_assignment_rx:
        tokio::sync::mpsc::UnboundedReceiver<cdma_common::hrpd::air::HrpdTrafficAssignmentRequest>,
    hrpd_traffic_release_rx:
        tokio::sync::mpsc::UnboundedReceiver<cdma_common::hrpd::air::HrpdTrafficReleaseRequest>,
    hrpd_forward_traffic_rx: tokio::sync::mpsc::UnboundedReceiver<
        crate::bts::hrpd::scheduler::PreparedForwardTrafficPacket,
    >,
    traffic_channels: TrafficChannelPool,
    traffic_rx_pool: TrafficRxPool,
    hrpd_traffic_rx_queue: HrpdTrafficRxQueue,
    traffic_rx_removals: TrafficRxRemovals,
    power_control: BtsPowerControlRegistry,
    rx_measurements: settings::RxMeasurementStore,
    /// Shared H-ARQ event bus between the HRPD forward scheduler (synth
    /// thread) and per-MAC reverse traffic RX workers.
    hrpd_harq_bus: std::sync::Arc<crate::bts::hrpd::HarqBus>,
    hrpd_power_control: crate::bts::hrpd::HrpdPowerControlRegistry,
}

type PilotWalshChannel = WalshChannelWrapper<ForwardPilotChannel>;
type SyncWalshChannel = WalshChannelWrapper<ForwardSyncChannel<9, 2>>;
type PagingWalshChannel = WalshChannelWrapper<ForwardPagingChannel<9, 2>>;

pub(crate) struct TxLoopState {
    chip_rate: u64,
    block_size: u64,
    tx_batch_chips: u64,
    sync_frame_chips: u64,
    sync_superframe_chips: u64,
    paging_frame_chips: u64,
    paging_fragments_per_frame: usize,
    pilot_offset_chips: u64,
    paging_start_enable_chip: u64,
    sync_requested_fragments: usize,
    sync_sent_fragments: usize,
    paging_requested_fragments: usize,
    paging_sent_fragments: usize,
    current_sync_pdu: Option<crate::lac::EncapsulatedPdu>,
    timezone_cache: Option<(Instant, cdma_common::timezone::ResolvedTimezone)>,
    hardware_start_tick: u64,
    hardware_start_chip: u64,
    gen_time_sum_us: u64,
    gen_time_max_us: u64,
    sync_time_sum_us: u64,
    paging_time_sum_us: u64,
    synth_time_sum_us: u64,
    synth_pilot_us: u64,
    synth_fsch_us: u64,
    synth_fpch_us: u64,
    synth_ftch_us: u64,
    synth_spread_us: u64,
    tx_time_sum_us: u64,
    tx_time_max_us: u64,
    pulse_time_sum_us: u64,
    pulse_time_max_us: u64,
    hw_margin_min_us: i64,
    hw_margin_max_us: i64,
    hw_margin_sum_us: i64,
    hw_margin_samples: u64,
    late_batches: u64,
    last_radio_health: TxRadioHealth,
    synth_blocks: usize,
    tx_batches: usize,
    interval_start: Instant,
    pub(super) scratch_pilot: Vec<num::complex::Complex32>,
    pub(super) scratch_sync: Vec<num::complex::Complex32>,
    pub(super) scratch_paging: Vec<num::complex::Complex32>,
    pub(super) scratch_tc_snapshot: Vec<(f32, handle::TrafficChannelWrapper)>,
    pub(super) scratch_tc_blocks: Vec<(f32, bool, Vec<num::complex::Complex32>)>,
    pub(super) scratch_tc_amps: Vec<f32>,
    /// Last per-block FTCH timing breakdown.
    pub(super) last_snap_us: u64,
    pub(super) last_tc_n: usize,
    pub(super) last_tc_max_us: u64,
    pub(super) last_tc_sum_us: u64,
    /// TX-private traffic channel working list. Updated via the lock-free
    /// command queue from `ChannelRegistry`; the TX synth loop iterates
    /// this list with no blocking lock on the hot path.
    pub(super) tx_pool: handle::TxPool,
}

impl Bts {
    /// Create a BTS using default runtime settings.
    pub fn new(radio: Box<dyn Radio>, config: Config) -> (Bts, BtsHandle) {
        Bts::new_with_settings(radio, config, BtsRuntimeSettings::default())
    }

    /// Create a BTS with an injected RX path for tests and diagnostics.
    pub fn new_with_injected_rx(
        radio: Box<dyn Radio>,
        config: Config,
        runtime: BtsRuntimeSettings,
    ) -> (Bts, BtsHandle, rx::InjectedRxSender) {
        let (injected_tx, injected_rx) = rx::injected_rx_channel(32);
        let (bts, handle) = Self::build(radio, config, runtime, Some(injected_rx));
        (bts, handle, injected_tx)
    }

    /// Create a BTS with explicit runtime settings.
    pub fn new_with_settings(
        radio: Box<dyn Radio>,
        config: Config,
        runtime: BtsRuntimeSettings,
    ) -> (Bts, BtsHandle) {
        assert!(config.pilot_offset <= 511);
        Self::build(radio, config, runtime, None)
    }

    fn build(
        radio: Box<dyn Radio>,
        config: Config,
        runtime: BtsRuntimeSettings,
        injected_rx: Option<rx::InjectedRxReceiver>,
    ) -> (Bts, BtsHandle) {
        let (senders, handle) = handle::create_handle(Arc::new(runtime.clone()));
        let metrics = MetricsService::new(
            senders.tx_metrics,
            senders.rx_metrics,
            senders.access_event_tx,
        );
        let evdo = config.evdo.clone();
        let bts = Bts {
            config,
            radio: Some(radio),
            runtime,
            evdo,
            injected_rx,
            metrics,
            commands_rx: Some(senders.commands_rx),
            hrpd_access_event_tx: senders.hrpd_access_event_tx,
            hrpd_traffic_event_tx: senders.hrpd_traffic_event_tx,
            hrpd_forward_signaling_rx: senders.hrpd_forward_signaling_rx,
            hrpd_traffic_assignment_rx: senders.hrpd_traffic_assignment_rx,
            hrpd_traffic_release_rx: senders.hrpd_traffic_release_rx,
            hrpd_forward_traffic_rx: senders.hrpd_forward_traffic_rx,
            traffic_channels: senders.traffic_channels,
            traffic_rx_pool: senders.traffic_rx_pool,
            hrpd_traffic_rx_queue: senders.hrpd_traffic_rx_queue,
            traffic_rx_removals: senders.traffic_rx_removals,
            power_control: senders.power_control,
            rx_measurements: senders.rx_measurements,
            hrpd_harq_bus: senders.hrpd_harq_bus,
            hrpd_power_control: crate::bts::hrpd::HrpdPowerControlRegistry::default(),
        };
        (bts, handle)
    }

    fn configure_radio(&mut self) -> Result<(), Error> {
        let radio = self
            .radio
            .as_mut()
            .expect("radio consumed before configure");
        radio.set_tx_bandwidth(self.runtime.tx_bandwidth_hz)?;
        radio.set_tx_sample_rate(self.runtime.tx_sample_rate_hz)?;
        radio.set_tx_lo_offset_hz(self.runtime.tx_lo_offset_hz)?;
        let tx_center_frequency_hz = self
            .evdo
            .as_ref()
            .map(|evdo| {
                if evdo.uses_hrpd_only() {
                    evdo.evdo_frequency_hz
                } else {
                    evdo.composite_center_frequency_hz
                }
            })
            .unwrap_or(self.config.tx_center_frequency_hz);
        radio.set_tx_frequency(tx_center_frequency_hz)?;

        info!(
            "Set TX center frequency to {:.04}Mhz (LO offset {:+.03}kHz), sample rate {:.04}Mhz, spreading rate {:?}",
            tx_center_frequency_hz as f64 / 1_000_000.0,
            self.runtime.tx_lo_offset_hz as f64 / 1_000.0,
            self.runtime.tx_sample_rate_hz as f64 / 1_000_000.0,
            self.runtime.spreading_rate,
        );
        let dl = &self.runtime.downlink;
        let db = |fraction: f32| 10.0 * fraction.max(f32::MIN_POSITIVE).log10();
        info!(
            "forward power split: pilot {:.1}% ({:+.1} dB) sync {:.1}% ({:+.1} dB) paging {:.1}% ({:+.1} dB) traffic {:.1}% ({:+.1} dB, max {:.1}% per channel); composite {:+.1} dBFS before pulse shaping at tx_digital_backoff {:.2}",
            dl.pilot.power_fraction * 100.0,
            db(dl.pilot.power_fraction),
            dl.sync.power_fraction * 100.0,
            db(dl.sync.power_fraction),
            dl.paging.power_fraction * 100.0,
            db(dl.paging.power_fraction),
            dl.traffic.power_fraction * 100.0,
            db(dl.traffic.power_fraction),
            dl.traffic.max_channel_power_fraction * 100.0,
            db(dl.total_power_fraction()
                * self.runtime.tx_digital_backoff
                * self.runtime.tx_digital_backoff),
            self.runtime.tx_digital_backoff,
        );
        if let Some(evdo) = &self.evdo {
            if evdo.uses_hrpd_only() {
                info!(
                    "EV-DO HRPD-only: bc{} ch{} TX {:.04}MHz reverse-access {:.04}MHz pilot_pn={}; 1x TX/RX disabled; tx_bw {:.03}MHz HRPD gain {:.3} digital_backoff {:.3} effective_scale {:.3}",
                    evdo.evdo_band_class,
                    evdo.evdo_channel,
                    evdo.evdo_frequency_hz as f64 / 1_000_000.0,
                    evdo.evdo_reverse_frequency_hz as f64 / 1_000_000.0,
                    evdo.pilot_pn,
                    self.runtime.tx_bandwidth_hz as f64 / 1_000_000.0,
                    evdo.gain,
                    self.runtime.tx_digital_backoff,
                    hrpd_only_tx_scale(self.runtime.tx_digital_backoff),
                );
            } else {
                info!(
                    "EV-DO single-RF composite: center {:.04}MHz; 1x bc{} ch{} {:.04}MHz shift {:+.03}kHz; HRPD bc{} ch{} {:.04}MHz shift {:+.03}kHz reverse-access {:.04}MHz pilot_pn={}; tx_bw {:.03}MHz gain {:.3} (HRPD:1x ratio); ATIM advertise={}",
                    evdo.composite_center_frequency_hz as f64 / 1_000_000.0,
                    evdo.one_x_band_class,
                    evdo.one_x_channel,
                    evdo.one_x_frequency_hz as f64 / 1_000_000.0,
                    evdo.one_x_shift_hz as f64 / 1_000.0,
                    evdo.evdo_band_class,
                    evdo.evdo_channel,
                    evdo.evdo_frequency_hz as f64 / 1_000_000.0,
                    evdo.evdo_shift_hz as f64 / 1_000.0,
                    evdo.evdo_reverse_frequency_hz as f64 / 1_000_000.0,
                    evdo.pilot_pn,
                    self.runtime.tx_bandwidth_hz as f64 / 1_000_000.0,
                    evdo.gain,
                    evdo.advertise_on_1x,
                );
            }
        }
        Ok(())
    }

    fn spawn_rx_thread(
        rx_settings: RxSettings,
        realtime_settings: RealtimeSettings,
        commands_rx: mpsc::Receiver<BtsCommand>,
        mut radio_rx: Box<dyn RadioRx>,
        shutdown: Arc<AtomicBool>,
    ) -> thread::JoinHandle<Result<(), Error>> {
        thread::Builder::new()
            .name("bts-rx".into())
            .spawn(move || {
                realtime::apply_rx(&realtime_settings);
                let shutdown_flag = shutdown.clone();
                let result = rx::run_rx_loop(rx_settings, commands_rx, &mut *radio_rx, shutdown);
                match &result {
                    Ok(()) => info!("rx: stopped normally"),
                    Err(_) => shutdown_flag.store(true, std::sync::atomic::Ordering::Relaxed),
                }
                result
            })
            .expect("failed to spawn RX thread")
    }

    const TX_BLOCK_SLOW_GEN_THRESHOLD_US: u64 = 500;
    const TX_BATCH_GEN_WARN_NUMERATOR: u64 = 4;
    const TX_BATCH_GEN_WARN_DENOMINATOR: u64 = 5;
    /// Fraction of the batch playout duration a single timed write may consume
    /// before it is worth warning about. Fixed thresholds over-report once the
    /// internal SDR write batch is longer than the original 2.5 ms default.
    const TX_SLOW_THRESHOLD_NUM: u64 = 4;
    const TX_SLOW_THRESHOLD_DEN: u64 = 5;

    fn flush_tx_samples_batch(
        radio_tx: &mut dyn RadioTx,
        state: &mut TxLoopState,
        tx_samples: &[Complex32],
        batch_tx_tick: u64,
        source_chips: usize,
    ) -> Result<(), Error> {
        if tx_samples.is_empty() {
            return Ok(());
        }

        const MARGIN_SAMPLE_INTERVAL: usize = 16;
        let hw_before = if state.tx_batches % MARGIN_SAMPLE_INTERVAL == 0 {
            radio_tx.get_hardware_time().ok()
        } else {
            None
        };

        let tx_start = Instant::now();
        radio_tx.transmit_at(tx_samples, Some(batch_tx_tick))?;
        let tx_us = tx_start.elapsed().as_micros() as u64;
        state.tx_time_sum_us += tx_us;
        state.tx_time_max_us = state.tx_time_max_us.max(tx_us);
        state.tx_batches += 1;

        if let Some(hw) = hw_before {
            let tick_rate = radio_tx.tick_rate().max(1);
            let margin_ticks = i128::from(batch_tx_tick) - i128::from(hw);
            let margin_us = (margin_ticks * 1_000_000 / i128::from(tick_rate)) as i64;
            state.hw_margin_min_us = state.hw_margin_min_us.min(margin_us);
            state.hw_margin_max_us = state.hw_margin_max_us.max(margin_us);
            state.hw_margin_sum_us = state.hw_margin_sum_us.saturating_add(margin_us);
            state.hw_margin_samples += 1;
            state.late_batches += u64::from(margin_us < 0);
            trace!(
                "tx_batch_debug: batch #{} tick={} hw={} margin_us={} tx_us={} chips={} samples={}",
                state.tx_batches,
                batch_tx_tick,
                hw,
                margin_us,
                tx_us,
                source_chips,
                tx_samples.len(),
            );
        }

        let batch_air_us =
            (source_chips as u64).saturating_mul(1_000_000) / u64::from(SR1_CHIP_RATE_HZ);
        let slow_threshold_us =
            batch_air_us.saturating_mul(Self::TX_SLOW_THRESHOLD_NUM) / Self::TX_SLOW_THRESHOLD_DEN;
        if tx_us > slow_threshold_us {
            log::warn!(
                "tx_slow_batch: transmit_at took {}us (threshold={}us batch #{}, {} chips, {} samples)",
                tx_us,
                slow_threshold_us,
                state.tx_batches,
                source_chips,
                tx_samples.len(),
            );
        }
        Ok(())
    }

    fn run_loop(
        mut self,
        max_blocks: Option<usize>,
        pace_to_hardware_time: bool,
    ) -> Result<(), Error> {
        self.runtime.validate()?;
        self.configure_radio()?;

        let radio = self.radio.take().expect("radio consumed before split");
        let (mut radio_tx, mut radio_rx) = {
            let _driver_priority =
                realtime::DriverPriorityGuard::enter("radio-stream-init", &self.runtime.realtime);
            radio.split()?
        };
        let max_tx_samples = self
            .runtime
            .tx_batch_chips
            .saturating_mul(self.runtime.tx_sample_rate_hz)
            / self.runtime.chip_rate_hz.max(1)
            + 256;
        radio_tx.prepare_transmit(max_tx_samples)?;

        let one_x_enabled = !self.evdo.as_ref().is_some_and(|cfg| cfg.uses_hrpd_only());
        let (pch, fsch, fpch) = downlink::build_channels(&self.config, &self.runtime)?;
        let pilot_offset_chips = timing::pilot_offset_chips(self.config.pilot_offset);

        let mut state = TxLoopState {
            chip_rate: self.runtime.chip_rate_hz as u64,
            block_size: self.runtime.block_size_chips as u64,
            tx_batch_chips: self.runtime.tx_batch_chips as u64,
            sync_frame_chips: self.runtime.overhead.fragment_availability_interval_chips as u64,
            sync_superframe_chips: self.runtime.overhead.sync_superframe_interval_chips as u64,
            paging_frame_chips: (self.runtime.chip_rate_hz / 50) as u64,
            paging_fragments_per_frame: 2,
            pilot_offset_chips,
            paging_start_enable_chip: pilot_offset_chips + SR1_CHIPS_320MS,
            sync_requested_fragments: 0,
            sync_sent_fragments: 0,
            paging_requested_fragments: 0,
            paging_sent_fragments: 0,
            current_sync_pdu: None,
            timezone_cache: None,
            hardware_start_tick: 0,
            hardware_start_chip: 0,
            gen_time_sum_us: 0,
            gen_time_max_us: 0,
            sync_time_sum_us: 0,
            paging_time_sum_us: 0,
            synth_time_sum_us: 0,
            synth_pilot_us: 0,
            synth_fsch_us: 0,
            synth_fpch_us: 0,
            synth_ftch_us: 0,
            synth_spread_us: 0,
            tx_time_sum_us: 0,
            tx_time_max_us: 0,
            pulse_time_sum_us: 0,
            pulse_time_max_us: 0,
            hw_margin_min_us: i64::MAX,
            hw_margin_max_us: i64::MIN,
            hw_margin_sum_us: 0,
            hw_margin_samples: 0,
            late_batches: 0,
            last_radio_health: TxRadioHealth::default(),
            synth_blocks: 0,
            tx_batches: 0,
            interval_start: Instant::now(),
            scratch_pilot: vec![Complex32::default(); self.runtime.block_size_chips],
            scratch_sync: vec![Complex32::default(); self.runtime.block_size_chips],
            scratch_paging: vec![Complex32::default(); self.runtime.block_size_chips],
            scratch_tc_snapshot: Vec::new(),
            scratch_tc_blocks: Vec::new(),
            scratch_tc_amps: Vec::new(),
            tx_pool: handle::TxPool::new(self.traffic_channels.tx_cmd_queue()),
            last_snap_us: 0,
            last_tc_n: 0,
            last_tc_max_us: 0,
            last_tc_sum_us: 0,
        };

        if let Some(rx) = radio_rx.as_mut() {
            timing::prime_hardware_clock(&mut **rx, &mut *radio_tx)?;
        }

        let tick_rate = radio_tx.tick_rate();
        let hardware_now = radio_tx.get_hardware_time()?;
        let start_system_time = self
            .config
            .start_system_time
            .unwrap_or_else(time::system_time_now);

        let initial_anchor = timing::compute_initial_tx_anchor(
            start_system_time,
            state.chip_rate,
            tick_rate,
            hardware_now,
            state.sync_superframe_chips,
            pilot_offset_chips,
        );
        timing::apply_anchor(&mut state, &initial_anchor);
        timing::log_anchor(
            "bts: hardware-time anchor",
            hardware_now,
            &initial_anchor,
            state.chip_rate,
        );

        let shutdown = Arc::new(AtomicBool::new(false));
        let realtime_settings = self.runtime.realtime.clone();
        {
            let flag = shutdown.clone();
            let _ = signal_hook::flag::register(signal_hook::consts::SIGINT, flag.clone());
            let _ = signal_hook::flag::register(signal_hook::consts::SIGTERM, flag);
        }

        let tx_rx_anchor = Arc::new(TxRxAnchor::new());

        let rx_thread = match (
            self.config.rx.clone(),
            self.commands_rx.take(),
            radio_rx.take(),
            self.injected_rx.take(),
        ) {
            (Some(mut rx_settings), Some(commands_rx), Some(rx), _) => {
                rx_settings.one_x_enabled = one_x_enabled;
                rx_settings.absolute_chip_start = 0;
                rx_settings.hardware_start_time_ns = 0;
                rx_settings.tick_rate = tick_rate;
                rx_settings.rx_metrics_tx = Some(self.metrics.rx_metrics_sender());
                rx_settings.hrpd_access_event_tx = Some(self.hrpd_access_event_tx.clone());
                rx_settings.hrpd_traffic_event_tx = Some(self.hrpd_traffic_event_tx.clone());
                rx_settings.hrpd_traffic_rx_queue = Some(self.hrpd_traffic_rx_queue.clone());
                rx_settings.hrpd_harq_bus = Some(self.hrpd_harq_bus.clone());
                rx_settings.hrpd_power_control = Some(self.hrpd_power_control.clone());
                if one_x_enabled {
                    rx_settings.access_event_tx = Some(self.metrics.access_event_sender());
                    rx_settings.traffic_rx_pool = Some(self.traffic_rx_pool.clone());
                    rx_settings.traffic_rx_removals = Some(self.traffic_rx_removals.clone());
                    rx_settings.traffic_channels = Some(self.traffic_channels.clone());
                    rx_settings.power_control = Some(self.power_control.clone());
                    rx_settings.rx_measurements = Some(self.rx_measurements.clone());
                }
                rx_settings.tx_rx_anchor = Some(tx_rx_anchor.clone());
                Some(Self::spawn_rx_thread(
                    rx_settings,
                    realtime_settings.clone(),
                    commands_rx,
                    rx,
                    shutdown.clone(),
                ))
            }
            (Some(mut rx_settings), Some(commands_rx), None, Some(injected_rx)) => {
                rx_settings.one_x_enabled = one_x_enabled;
                rx_settings.absolute_chip_start = state.hardware_start_chip;
                rx_settings.hardware_start_time_ns = state.hardware_start_tick;
                rx_settings.tick_rate = tick_rate;
                rx_settings.rx_metrics_tx = Some(self.metrics.rx_metrics_sender());
                rx_settings.hrpd_access_event_tx = Some(self.hrpd_access_event_tx.clone());
                rx_settings.hrpd_traffic_event_tx = Some(self.hrpd_traffic_event_tx.clone());
                rx_settings.hrpd_traffic_rx_queue = Some(self.hrpd_traffic_rx_queue.clone());
                rx_settings.hrpd_harq_bus = Some(self.hrpd_harq_bus.clone());
                rx_settings.hrpd_power_control = Some(self.hrpd_power_control.clone());
                if one_x_enabled {
                    rx_settings.access_event_tx = Some(self.metrics.access_event_sender());
                    rx_settings.traffic_rx_pool = Some(self.traffic_rx_pool.clone());
                    rx_settings.traffic_rx_removals = Some(self.traffic_rx_removals.clone());
                    rx_settings.traffic_channels = Some(self.traffic_channels.clone());
                    rx_settings.power_control = Some(self.power_control.clone());
                    rx_settings.rx_measurements = Some(self.rx_measurements.clone());
                }
                rx_settings.tx_rx_anchor = None;
                let injected_shutdown = shutdown.clone();
                let injected_realtime = realtime_settings.clone();
                Some(
                    thread::Builder::new()
                        .name("bts-rx-injected".into())
                        .spawn(move || {
                            realtime::apply_rx(&injected_realtime);
                            let shutdown_flag = injected_shutdown.clone();
                            let result = rx::run_injected_rx_loop(
                                rx_settings,
                                commands_rx,
                                injected_rx,
                                injected_shutdown,
                            );
                            if result.is_err() {
                                shutdown_flag.store(true, std::sync::atomic::Ordering::Relaxed);
                            }
                            result
                        })
                        .expect("failed to spawn injected RX thread"),
                )
            }
            (_, Some(commands_rx), _, _) => {
                drop(commands_rx);
                None
            }
            _ => None,
        };

        let mut chip_cursor;
        {
            let hw_now = radio_tx.get_hardware_time()?;
            let live_anchor = timing::reseed_tx_anchor_from_live_clock(
                state.chip_rate,
                tick_rate,
                hw_now,
                state.hardware_start_tick,
                state.hardware_start_chip,
                state.sync_superframe_chips,
                pilot_offset_chips,
                self.runtime.max_tx_lookahead_ms,
            );
            timing::apply_anchor(&mut state, &live_anchor);
            chip_cursor = live_anchor.chip_cursor;
            timing::log_anchor(
                "bts: TX anchor seeded from live clock:",
                hw_now,
                &live_anchor,
                state.chip_rate,
            );

            tx_rx_anchor.publish(state.hardware_start_tick, state.hardware_start_chip);
            if one_x_enabled {
                info!(
                    "bts: TX→RX anchor published: tick={} chip={}",
                    state.hardware_start_tick, state.hardware_start_chip
                );
            }
        }

        let mut spreader = synth::aligned_spreader(
            self.config.pilot_offset,
            self.runtime.short_code_length_chips,
            chip_cursor,
        );
        let initial_hrpd_timezone = self.evdo.as_ref().map(|_| {
            downlink::resolve_timezone_cached(
                &mut state,
                &self.config.timezone,
                &self.config.overhead,
            )
        });
        let mut evdo_idle = self.evdo.as_ref().map(|cfg| {
            let mut m = evdo::HrpdForwardSlotModulator::new(
                self.config.pilot_offset,
                self.runtime.short_code_length_chips,
            );
            // Install the explicit HRPD sector identity plus the 1x partner
            // neighbor advert. SyncMessage.SystemTime is overwritten live at
            // every cycle boundary inside maybe_advance_slot.
            let one_x_partner = cfg.transmits_one_x().then_some((
                cfg.one_x_band_class,
                cfg.one_x_channel,
                cfg.pilot_pn,
            ));
            m.install_sector_overheads(
                cfg.pilot_pn,
                one_x_partner,
                cfg.evdo_band_class,
                cfg.evdo_channel,
                cfg.overhead,
            );
            let timezone = initial_hrpd_timezone.expect("HRPD timezone resolved");
            m.install_sector_time(timezone.lp_sec, timezone.local_time_offset_minutes);
            m.set_harq_bus(self.hrpd_harq_bus.clone());
            log::info!(
                "HRPD forward link armed: ColorCode={} SectorID24=0x{:06X} SubnetMask=/{} HRPDch={} (bc{}) 1xPartner=bc{}/ch{} pilot_pn={}; overhead schedule: Sync+Access every 3 cycles (~1.28s), Quick every 3 cycles, Sector/ReverseRate every 4 cycles",
                cfg.overhead.color_code,
                cfg.overhead.sector_id24(),
                cfg.overhead.subnet_mask,
                cfg.evdo_channel,
                cfg.evdo_band_class,
                cfg.one_x_band_class,
                cfg.one_x_channel,
                cfg.pilot_pn,
            );
            log::info!(
                "HRPD sector time: LeapSeconds={} LocalTimeOffset={}min",
                timezone.lp_sec,
                timezone.local_time_offset_minutes,
            );
            m
        });
        let mut hrpd_active_macs: Vec<crate::bts::hrpd::ActiveMac> = Vec::new();
        let evdo_hrpd_only = self.evdo.as_ref().is_some_and(|cfg| cfg.uses_hrpd_only());
        let mut evdo_composer = if let Some(cfg) = self
            .evdo
            .as_ref()
            .filter(|cfg| cfg.uses_adjacent_composite())
        {
            Some(evdo::AdjacentCarrierComposer::new(
                cfg,
                self.runtime.tx_sample_rate_hz,
                self.runtime.tx_digital_backoff,
            )?)
        } else {
            None
        };
        let one_x_input_scale =
            one_x_synth_scale(self.runtime.tx_digital_backoff, evdo_composer.is_some());
        let mut one_x_shaper = TxPulseShaper::new(self.runtime.tx_sample_rate_hz)?;
        let mut hrpd_shaper = if evdo_hrpd_only {
            Some(TxPulseShaper::new(self.runtime.tx_sample_rate_hz)?)
        } else {
            None
        };
        fpch.channel.advance_lc_to_chip(chip_cursor);

        radio_tx.enable_transmit_at(true, Some(state.hardware_start_tick))?;
        info!(
            "tx_timing: enable_transmit_at start_tick={} hw_now={}",
            state.hardware_start_tick,
            radio_tx.get_hardware_time()?,
        );

        let mut synth_block = vec![Complex32::default(); state.block_size as usize];
        let mut tx_batch = vec![Complex32::default(); state.tx_batch_chips as usize];
        let mut evdo_tx_batch = self
            .evdo
            .as_ref()
            .map(|_| vec![Complex32::default(); state.tx_batch_chips as usize]);
        let shaped_batch_samples = state.tx_batch_chips as usize * self.runtime.tx_sample_rate_hz
            / self.runtime.chip_rate_hz
            + 128;
        let mut tx_shape_buf = vec![Complex32::default(); shaped_batch_samples];
        realtime::prefault_complex(&mut state.scratch_pilot);
        realtime::prefault_complex(&mut state.scratch_sync);
        realtime::prefault_complex(&mut state.scratch_paging);
        realtime::prefault_complex(&mut synth_block);
        realtime::prefault_complex(&mut tx_batch);
        if let Some(batch) = evdo_tx_batch.as_mut() {
            realtime::prefault_complex(batch);
        }
        realtime::prefault_complex(&mut tx_shape_buf);
        tx_shape_buf.clear();
        let blocks_per_batch = (state.tx_batch_chips / state.block_size) as usize;
        // Periodic TX cost breakdown (this thread only, no shared state).
        // gen covers the whole per-batch synthesis (1x + EVDO), evdo just the
        // EVDO modulator portion, shape the pulse-shaping/compositing stage.
        let mut tx_stat_batches = 0u64;
        let mut tx_stat_air_chips = 0u64;
        let mut tx_stat_gen_us = 0u64;
        let mut tx_stat_gen_max_us = 0u64;
        let mut tx_stat_evdo_us = 0u64;
        let mut tx_stat_evdo_max_us = 0u64;
        let mut tx_stat_hrpd_ingress = 0u64;
        let mut tx_stat_hrpd_backlog_max = 0usize;
        let mut tx_stat_shape_us = 0u64;
        let mut tx_stat_shape_max_us = 0u64;
        const TX_STAT_WINDOW_CHIPS: u64 = 5 * 1_228_800;
        let heartbeat_interval = (state.chip_rate / state.block_size) as usize;
        let mut sent_blocks = 0usize;

        let mut wall_anchor_instant = Instant::now();
        let mut wall_anchor_tick = radio_tx.get_hardware_time()?;
        let mut pacer = timing::LookaheadPacer::new();

        loop {
            if shutdown.load(std::sync::atomic::Ordering::Relaxed) {
                info!("shutdown signal received, stopping TX loop");
                break;
            }
            if let Some(limit) = max_blocks {
                if sent_blocks >= limit {
                    break;
                }
            }

            let batch_playout_tick = timing::batch_playout_tick(&state, chip_cursor, tick_rate);
            if pace_to_hardware_time && self.runtime.max_tx_lookahead_ms > 0 {
                pacer.wait_until_within_tx_lookahead(
                    batch_playout_tick,
                    wall_anchor_tick,
                    wall_anchor_instant,
                    tick_rate,
                    self.runtime.max_tx_lookahead_ms,
                    &shutdown,
                );
            }

            if max_blocks.is_none()
                && sent_blocks % heartbeat_interval < blocks_per_batch
                && sent_blocks > 0
            {
                if let Some(evdo_idle) = evdo_idle.as_mut() {
                    let timezone = downlink::resolve_timezone_cached(
                        &mut state,
                        &self.config.timezone,
                        &self.config.overhead,
                    );
                    if evdo_idle
                        .update_sector_time(timezone.lp_sec, timezone.local_time_offset_minutes)
                    {
                        info!(
                            "HRPD sector time updated: LeapSeconds={} LocalTimeOffset={}min",
                            timezone.lp_sec, timezone.local_time_offset_minutes,
                        );
                    }
                }
                let frame_system_time = time::system_time_from_chips(chip_cursor, state.chip_rate);
                let tx_rel_chips = chip_cursor.saturating_sub(state.hardware_start_chip);
                let tx_hardware_tick =
                    timing::hardware_tick_at_chip(&state, chip_cursor, tick_rate);
                let t20 = time::system_time_20ms_frames(frame_system_time);
                let wall_ms = state.interval_start.elapsed().as_millis();
                let synth_n = state.synth_blocks.max(1) as u64;
                let tx_n = state.tx_batches.max(1) as u64;
                let avg_gen_us = state.gen_time_sum_us / synth_n;
                let avg_tx_us = state.tx_time_sum_us / tx_n;
                let avg_pulse_us = state.pulse_time_sum_us / tx_n;
                let gen_total_ms = state.gen_time_sum_us / 1000;
                let tx_total_ms = state.tx_time_sum_us / 1000;
                let rt_ratio = if state.gen_time_sum_us > 0 {
                    1_000_000.0 / state.gen_time_sum_us as f64
                } else {
                    f64::INFINITY
                };
                let sync_total_ms = state.sync_time_sum_us / 1000;
                let paging_total_ms = state.paging_time_sum_us / 1000;
                let synth_total_ms = state.synth_time_sum_us / 1000;
                let hw_margin_avg_us = state
                    .hw_margin_sum_us
                    .checked_div(state.hw_margin_samples as i64)
                    .unwrap_or_default();
                let hw_margin_min_us = if state.hw_margin_samples == 0 {
                    0
                } else {
                    state.hw_margin_min_us
                };
                let hw_margin_max_us = if state.hw_margin_samples == 0 {
                    0
                } else {
                    state.hw_margin_max_us
                };
                if state.late_batches > 0 {
                    warn!(
                        "tx_deadline_missed: sampled_late={} worst_margin_us={} avg_margin_us={} best_margin_us={}",
                        state.late_batches, hw_margin_min_us, hw_margin_avg_us, hw_margin_max_us,
                    );
                }
                let radio_health = match radio_tx.tx_health() {
                    Ok(health) => {
                        let previous = state.last_radio_health;
                        let new_underflows = health.underflows.saturating_sub(previous.underflows);
                        let new_late_packets =
                            health.late_packets.saturating_sub(previous.late_packets);
                        let new_sequence_errors = health
                            .sequence_errors
                            .saturating_sub(previous.sequence_errors);
                        let new_dropped_packets = health
                            .dropped_packets
                            .saturating_sub(previous.dropped_packets);
                        let new_unknown_events = health
                            .unknown_events
                            .saturating_sub(previous.unknown_events);
                        if new_underflows > 0
                            || new_late_packets > 0
                            || new_sequence_errors > 0
                            || new_dropped_packets > 0
                            || new_unknown_events > 0
                        {
                            warn!(
                                "tx_radio_health: new_underflows={} new_late_packets={} new_sequence_errors={} new_dropped_packets={} new_unknown_events={} totals[underflows={} late_packets={} sequence_errors={} dropped_packets={} unknown_events={}]",
                                new_underflows,
                                new_late_packets,
                                new_sequence_errors,
                                new_dropped_packets,
                                new_unknown_events,
                                health.underflows,
                                health.late_packets,
                                health.sequence_errors,
                                health.dropped_packets,
                                health.unknown_events,
                            );
                        }
                        state.last_radio_health = health;
                        health
                    }
                    Err(err) => {
                        warn!("tx_health: radio status unavailable: {err}");
                        state.last_radio_health
                    }
                };
                trace!(
                    "transmit t20={} wall={}ms blocks={} tx_batches={} gen={}ms(avg={}us max={}us) sync={}ms paging={}ms synth={}ms[pilot={}ms fsch={}ms fpch={}ms spread={}ms] tx={}ms(avg={}us max={}us) rt={:.1}x pace_margin={}us",
                    t20,
                    wall_ms,
                    state.synth_blocks,
                    state.tx_batches,
                    gen_total_ms,
                    avg_gen_us,
                    state.gen_time_max_us,
                    sync_total_ms,
                    paging_total_ms,
                    synth_total_ms,
                    state.synth_pilot_us / 1000,
                    state.synth_fsch_us / 1000,
                    state.synth_fpch_us / 1000,
                    state.synth_spread_us / 1000,
                    tx_total_ms,
                    avg_tx_us,
                    state.tx_time_max_us,
                    rt_ratio,
                    pacer.margin_us()
                );
                trace!(
                    "tx_hardware_heartbeat: hw_tick={} chip={} rel_chip={} t20={}",
                    tx_hardware_tick, chip_cursor, tx_rel_chips, t20
                );
                trace!(
                    "tx_deadline_health: margin_us[min={} avg={} max={}] sampled_late={} pulse_us[avg={} max={}] radio[underflow={} late={} sequence={} dropped={} ack={} unknown={}] rt_degraded={}",
                    hw_margin_min_us,
                    hw_margin_avg_us,
                    hw_margin_max_us,
                    state.late_batches,
                    avg_pulse_us,
                    state.pulse_time_max_us,
                    radio_health.underflows,
                    radio_health.late_packets,
                    radio_health.sequence_errors,
                    radio_health.dropped_packets,
                    radio_health.burst_acks,
                    radio_health.unknown_events,
                    realtime::degraded_events(),
                );
                self.metrics.publish_tx_metrics(TxMetrics {
                    timestamp_ns: tx_hardware_tick,
                    chip_cursor,
                    blocks_transmitted: sent_blocks as u64,
                    rt_ratio,
                    gen_avg_us: avg_gen_us,
                    gen_max_us: state.gen_time_max_us,
                    tx_avg_us: avg_tx_us,
                    tx_max_us: state.tx_time_max_us,
                    pulse_avg_us: avg_pulse_us,
                    pulse_max_us: state.pulse_time_max_us,
                    hw_margin_min_us,
                    hw_margin_avg_us,
                    hw_margin_max_us,
                    late_batches: state.late_batches,
                    radio_health,
                    finalized_queue_airtime_us: 0,
                    synth_pilot_us: state.synth_pilot_us,
                    synth_sync_us: state.synth_fsch_us,
                    synth_paging_us: state.synth_fpch_us,
                    synth_spread_us: state.synth_spread_us,
                    sync_fragments_sent: state.sync_sent_fragments as u64,
                    paging_fragments_sent: state.paging_sent_fragments as u64,
                    realtime_degraded_events: realtime::degraded_events(),
                });
                state.gen_time_sum_us = 0;
                state.gen_time_max_us = 0;
                state.sync_time_sum_us = 0;
                state.paging_time_sum_us = 0;
                state.synth_time_sum_us = 0;
                state.synth_pilot_us = 0;
                state.synth_fsch_us = 0;
                state.synth_fpch_us = 0;
                state.synth_ftch_us = 0;
                state.synth_spread_us = 0;
                state.tx_time_sum_us = 0;
                state.tx_time_max_us = 0;
                state.pulse_time_sum_us = 0;
                state.pulse_time_max_us = 0;
                state.hw_margin_min_us = i64::MAX;
                state.hw_margin_max_us = i64::MIN;
                state.hw_margin_sum_us = 0;
                state.hw_margin_samples = 0;
                state.late_batches = 0;
                state.synth_blocks = 0;
                state.tx_batches = 0;
                state.interval_start = Instant::now();

                if let Ok(hw) = radio_tx.get_hardware_time() {
                    wall_anchor_tick = hw;
                    wall_anchor_instant = Instant::now();
                }
            }

            let mut hrpd_forward_ingress_remaining = evdo_idle
                .as_ref()
                .map(|_| hrpd_forward_ingress_budget(state.tx_batch_chips))
                .unwrap_or(0);
            let gen_start = Instant::now();
            let bs = state.block_size as usize;
            for block_idx in 0..blocks_per_batch {
                let block_chip = chip_cursor + (block_idx as u64) * state.block_size;
                let frame_system_time = time::system_time_from_chips(block_chip, state.chip_rate);
                let boundaries = timing::frame_boundaries(&state, block_chip);
                let offset = block_idx * bs;

                if one_x_enabled {
                    downlink::send_availability_indications(
                        &self.config,
                        &self.runtime,
                        boundaries.sync_frame_boundary,
                        frame_system_time,
                        block_chip,
                    )?;

                    if boundaries.sync_frame_boundary {
                        let t = Instant::now();
                        downlink::handle_sync_frame(
                            &self.config,
                            &self.runtime,
                            &mut state,
                            &fsch,
                            block_chip,
                        )?;
                        state.sync_time_sum_us += t.elapsed().as_micros() as u64;
                    }

                    if boundaries.paging_frame_boundary && boundaries.paging_enabled {
                        let t = Instant::now();
                        let hw_tick = timing::hardware_tick_at_chip(&state, block_chip, tick_rate);
                        downlink::handle_paging_frame(
                            &self.config,
                            &self.runtime,
                            &mut state,
                            &fpch,
                            block_chip,
                            hw_tick,
                        )?;
                        let next_frame_chip = block_chip.saturating_add(state.paging_frame_chips);
                        let next_frame_system_time =
                            time::system_time_from_chips(next_frame_chip, state.chip_rate);
                        downlink::send_paging_frame_availability(
                            &self.config,
                            &self.runtime,
                            &state,
                            next_frame_system_time,
                            next_frame_chip,
                        )?;
                        state.paging_time_sum_us += t.elapsed().as_micros() as u64;
                    }

                    let prev_synth_pilot_us = state.synth_pilot_us;
                    let prev_synth_fsch_us = state.synth_fsch_us;
                    let prev_synth_fpch_us = state.synth_fpch_us;
                    let prev_synth_ftch_us = state.synth_ftch_us;
                    let prev_synth_spread_us = state.synth_spread_us;
                    let block_gen_start = Instant::now();
                    synth::synthesize_block(
                        &self.runtime,
                        one_x_input_scale,
                        &mut state,
                        gen_start,
                        &pch,
                        &fsch,
                        &fpch,
                        &mut spreader,
                        &mut synth_block,
                        bs,
                        frame_system_time,
                        block_chip,
                    )?;
                    let block_gen_us = block_gen_start.elapsed().as_micros() as u64;
                    if block_gen_us > Self::TX_BLOCK_SLOW_GEN_THRESHOLD_US {
                        log::debug!(
                            "tx_slow_gen: {}us (block #{}, chip={}) pilot={}us sync={}us paging={}us ftch={}us [snap={}us tc_n={} tc_sum={}us tc_max={}us] spread={}us",
                            block_gen_us,
                            state.synth_blocks,
                            block_chip,
                            state.synth_pilot_us.saturating_sub(prev_synth_pilot_us),
                            state.synth_fsch_us.saturating_sub(prev_synth_fsch_us),
                            state.synth_fpch_us.saturating_sub(prev_synth_fpch_us),
                            state.synth_ftch_us.saturating_sub(prev_synth_ftch_us),
                            state.last_snap_us,
                            state.last_tc_n,
                            state.last_tc_sum_us,
                            state.last_tc_max_us,
                            state.synth_spread_us.saturating_sub(prev_synth_spread_us),
                        );
                    }
                    tx_batch[offset..offset + bs].copy_from_slice(&synth_block[..bs]);
                }
                let evdo_gen_start = Instant::now();
                if let (Some(evdo_idle), Some(evdo_tx_batch)) =
                    (evdo_idle.as_mut(), evdo_tx_batch.as_mut())
                {
                    while let Ok(release) = self.hrpd_traffic_release_rx.try_recv() {
                        info!(
                            "HRPD traffic release: uati=0x{:08x} mac_index={}",
                            release.uati, release.mac_index
                        );
                        let (queued, active, emissions, feedback) =
                            evdo_idle.purge_traffic_mac(release.mac_index);
                        info!(
                            "HRPD traffic release: purged mac_index={} queued={} active={} harq_emissions={} harq_feedback={}",
                            release.mac_index, queued, active, emissions, feedback
                        );
                        hrpd_active_macs.retain(|active| active.mac_index != release.mac_index);
                        evdo_idle.set_active_macs(hrpd_active_macs.clone());
                        // Queue the RX-side worker teardown. The RX loop drains
                        // commands in FIFO order, so a release that follows a
                        // not-yet-drained assignment for the same UATI tears the
                        // worker down right after it spawns — no shared-Vec
                        // retain, and no mutex on the synth thread.
                        if self
                            .hrpd_traffic_rx_queue
                            .push(HrpdTrafficRxCommand::Release(release))
                            .is_err()
                        {
                            log::error!("HRPD reverse traffic command queue full; dropped release");
                        }
                    }
                    while let Ok(request) = self.hrpd_traffic_assignment_rx.try_recv() {
                        if !(5..64).contains(&request.mac_index) {
                            log::warn!(
                                "HRPD traffic assignment ignored: invalid mac_index={} uati=0x{:08x}",
                                request.mac_index,
                                request.uati
                            );
                            continue;
                        }
                        let (rpc_bit, rpc_alternating, rpc_mode) = hrpd_rpc_mode();
                        info!(
                            "HRPD traffic assignment install: uati=0x{:08x} mac_index={} physical_subtype=0x{:04x} rtc_mac_subtype=0x{:04x} reverse_rate_limit={}bps rpc={} drc_lock={} reverse_lcm_i=0x{:016x} reverse_lcm_q=0x{:016x}",
                            request.uati,
                            request.mac_index,
                            request.physical_layer_subtype,
                            request.reverse_traffic_mac_subtype,
                            request.reverse_rate_limit_bps,
                            rpc_mode,
                            request.drc_lock,
                            request.reverse_long_code_mask_i,
                            request.reverse_long_code_mask_q
                        );
                        match hrpd_active_macs
                            .iter_mut()
                            .find(|active| active.mac_index == request.mac_index)
                        {
                            Some(active) => {
                                active.rpc = rpc_bit;
                                active.rpc_alternating = rpc_alternating;
                                active.drclock = request.drc_lock;
                                active.frame_offset = request.frame_offset & 0x0f;
                                active.physical_layer_subtype = request.physical_layer_subtype;
                            }
                            None => hrpd_active_macs.push(crate::bts::hrpd::ActiveMac {
                                mac_index: request.mac_index,
                                rpc: rpc_bit,
                                rpc_alternating,
                                drclock: request.drc_lock,
                                frame_offset: request.frame_offset & 0x0f,
                                physical_layer_subtype: request.physical_layer_subtype,
                            }),
                        }
                        evdo_idle.set_active_macs(hrpd_active_macs.clone());
                        let mac_index = request.mac_index;
                        if self
                            .hrpd_traffic_rx_queue
                            .push(HrpdTrafficRxCommand::Assign(request))
                            .is_err()
                        {
                            log::error!(
                                "HRPD reverse traffic command queue full; dropped assignment for mac_index={mac_index}"
                            );
                        }
                    }
                    while let Ok(request) = self.hrpd_forward_signaling_rx.try_recv() {
                        info!(
                            "HRPD forward signaling enqueue: channel={:?} protocol=0x{:02x} target={:?} payload_octets={}",
                            request.channel,
                            request.protocol_type,
                            request.target_ati,
                            request.payload.len(),
                        );
                        evdo_idle.enqueue_forward_signaling(request);
                    }
                    while hrpd_forward_ingress_remaining > 0 {
                        let Ok(packet) = self.hrpd_forward_traffic_rx.try_recv() else {
                            break;
                        };
                        hrpd_forward_ingress_remaining -= 1;
                        tx_stat_hrpd_ingress += 1;
                        if !hrpd_active_macs
                            .iter()
                            .any(|active| active.mac_index == packet.mac_index)
                        {
                            log::warn!(
                                "HRPD forward traffic drop: inactive mac_index={} payload_bits={}",
                                packet.mac_index,
                                packet.payload.len()
                            );
                            continue;
                        }
                        debug!(
                            "HRPD forward traffic enqueue: mac_index={} payload_bits={}",
                            packet.mac_index,
                            packet.payload.len()
                        );
                        evdo_idle.enqueue_prepared_traffic(packet);
                    }
                    evdo_idle.next_block_into(block_chip, &mut evdo_tx_batch[offset..offset + bs]);
                    let evdo_block_us = evdo_gen_start.elapsed().as_micros() as u64;
                    tx_stat_evdo_us += evdo_block_us;
                    tx_stat_evdo_max_us = tx_stat_evdo_max_us.max(evdo_block_us);
                }
            }

            let batch_gen_us = gen_start.elapsed().as_micros() as u64;
            let batch_airtime_us = state.tx_batch_chips.saturating_mul(1_000_000) / state.chip_rate;
            let batch_warn_us = batch_airtime_us.saturating_mul(Self::TX_BATCH_GEN_WARN_NUMERATOR)
                / Self::TX_BATCH_GEN_WARN_DENOMINATOR;
            if batch_gen_us > batch_warn_us {
                let rt_ratio = if batch_gen_us > 0 {
                    batch_airtime_us as f64 / batch_gen_us as f64
                } else {
                    f64::INFINITY
                };
                log::warn!(
                    "tx_slow_batch_gen: {}us > {}us warn (airtime={}us, rt={:.2}x, {} blocks, {} chips, chip={})",
                    batch_gen_us,
                    batch_warn_us,
                    batch_airtime_us,
                    rt_ratio,
                    blocks_per_batch,
                    state.tx_batch_chips,
                    chip_cursor
                );
            }

            let batch_shape_us;
            if let (Some(hrpd_shaper), Some(_), Some(evdo_tx_batch)) = (
                hrpd_shaper.as_mut(),
                self.evdo.as_ref().filter(|cfg| cfg.uses_hrpd_only()),
                evdo_tx_batch.as_ref(),
            ) {
                let shape_start = Instant::now();
                hrpd_shaper.shape_into(evdo_tx_batch, &mut tx_shape_buf);
                let hrpd_scale = hrpd_only_tx_scale(self.runtime.tx_digital_backoff);
                if (hrpd_scale - 1.0).abs() > f32::EPSILON {
                    for sample in &mut tx_shape_buf {
                        *sample *= hrpd_scale;
                    }
                }
                batch_shape_us = shape_start.elapsed().as_micros() as u64;
                Self::flush_tx_samples_batch(
                    &mut *radio_tx,
                    &mut state,
                    &tx_shape_buf,
                    batch_playout_tick,
                    evdo_tx_batch.len(),
                )?;
            } else if let (Some(composer), Some(evdo_tx_batch)) =
                (evdo_composer.as_mut(), evdo_tx_batch.as_ref())
            {
                let shape_start = Instant::now();
                composer.compose_into(&tx_batch, evdo_tx_batch, &mut tx_shape_buf);
                batch_shape_us = shape_start.elapsed().as_micros() as u64;
                Self::flush_tx_samples_batch(
                    &mut *radio_tx,
                    &mut state,
                    &tx_shape_buf,
                    batch_playout_tick,
                    tx_batch.len(),
                )?;
            } else {
                let shape_start = Instant::now();
                one_x_shaper.shape_into(&tx_batch, &mut tx_shape_buf);
                batch_shape_us = shape_start.elapsed().as_micros() as u64;
                Self::flush_tx_samples_batch(
                    &mut *radio_tx,
                    &mut state,
                    &tx_shape_buf,
                    batch_playout_tick,
                    tx_batch.len(),
                )?;
            }

            tx_stat_batches += 1;
            tx_stat_air_chips += state.tx_batch_chips;
            tx_stat_gen_us += batch_gen_us;
            tx_stat_gen_max_us = tx_stat_gen_max_us.max(batch_gen_us);
            tx_stat_shape_us += batch_shape_us;
            tx_stat_shape_max_us = tx_stat_shape_max_us.max(batch_shape_us);
            state.pulse_time_sum_us = state.pulse_time_sum_us.saturating_add(batch_shape_us);
            state.pulse_time_max_us = state.pulse_time_max_us.max(batch_shape_us);
            tx_stat_hrpd_backlog_max =
                tx_stat_hrpd_backlog_max.max(self.hrpd_forward_traffic_rx.len());
            if tx_stat_air_chips >= TX_STAT_WINDOW_CHIPS {
                let air_us = tx_stat_air_chips * 1000 / 1229;
                let busy_us = tx_stat_gen_us + tx_stat_shape_us;
                info!(
                    "tx_stats: batches={} gen_avg={}us gen_max={}us evdo_avg={}us evdo_max={}us hrpd_ingress={} hrpd_backlog_max={} shape_avg={}us shape_max={}us rt_load_pct={:.1}",
                    tx_stat_batches,
                    tx_stat_gen_us / tx_stat_batches.max(1),
                    tx_stat_gen_max_us,
                    tx_stat_evdo_us / tx_stat_batches.max(1),
                    tx_stat_evdo_max_us,
                    tx_stat_hrpd_ingress,
                    tx_stat_hrpd_backlog_max,
                    tx_stat_shape_us / tx_stat_batches.max(1),
                    tx_stat_shape_max_us,
                    100.0 * busy_us as f64 / air_us.max(1) as f64,
                );
                tx_stat_batches = 0;
                tx_stat_air_chips = 0;
                tx_stat_gen_us = 0;
                tx_stat_gen_max_us = 0;
                tx_stat_evdo_us = 0;
                tx_stat_evdo_max_us = 0;
                tx_stat_hrpd_ingress = 0;
                tx_stat_hrpd_backlog_max = 0;
                tx_stat_shape_us = 0;
                tx_stat_shape_max_us = 0;
            }

            chip_cursor += state.tx_batch_chips;
            sent_blocks += blocks_per_batch;
        }

        trace!(
            "bts_sync_fragments: requested={} sent={}",
            state.sync_requested_fragments, state.sync_sent_fragments
        );
        trace!(
            "bts_paging_fragments: requested={} sent={}",
            state.paging_requested_fragments, state.paging_sent_fragments
        );

        info!("disabling TX module");
        if let Err(e) = radio_tx.enable_transmit(false) {
            log::error!("failed to disable TX on shutdown: {}", e);
        }

        if let Some(handle) = rx_thread {
            info!("waiting for RX thread to shut down...");
            match handle.join() {
                Ok(Ok(())) => info!("RX thread stopped"),
                Ok(Err(err)) => {
                    log::error!("rx: fatal error: {err}");
                    std::process::exit(1);
                }
                Err(_) => {
                    log::error!("rx: thread panicked");
                    std::process::exit(1);
                }
            }
        }

        Ok(())
    }

    /// Start the BTS TX loop until shutdown.
    pub async fn start(self) -> Result<(), Error> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        thread::Builder::new()
            .name("bts-tx".into())
            .spawn(move || {
                realtime::apply_tx(&self.runtime.realtime);
                let result = self.run_loop(None, true);
                let _ = tx.send(result);
            })
            .map_err(|e| Error::from(format!("failed to spawn TX thread: {}", e)))?;
        rx.await
            .map_err(|_| Error::from("BTS TX thread panicked"))?
    }

    /// Run the BTS TX loop for a bounded number of synthesis blocks.
    pub async fn run_for_blocks(self, blocks: usize) -> Result<(), Error> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        thread::Builder::new()
            .name("bts-tx".into())
            .spawn(move || {
                realtime::apply_tx(&self.runtime.realtime);
                let result = self.run_loop(Some(blocks), false);
                let _ = tx.send(result);
            })
            .map_err(|e| Error::from(format!("failed to spawn TX thread: {}", e)))?;
        rx.await
            .map_err(|_| Error::from("BTS TX thread panicked"))?
    }

    /// Run a bounded BTS TX loop with hardware-time pacing.
    pub async fn run_for_blocks_realtime(self, blocks: usize) -> Result<(), Error> {
        let (tx, rx) = tokio::sync::oneshot::channel();
        thread::Builder::new()
            .name("bts-tx".into())
            .spawn(move || {
                realtime::apply_tx(&self.runtime.realtime);
                let result = self.run_loop(Some(blocks), true);
                let _ = tx.send(result);
            })
            .map_err(|e| Error::from(format!("failed to spawn TX thread: {}", e)))?;
        rx.await
            .map_err(|_| Error::from("BTS TX thread panicked"))?
    }
}

impl Bts {
    /// Construct a BTS backed by a `RadioPipe` for testing.
    pub fn new_with_radio_pipe(
        mut radio: RadioPipe,
        config: Config,
        runtime: BtsRuntimeSettings,
    ) -> (Bts, BtsHandle) {
        let injected_rx = radio.take_injected_rx();
        Self::build(Box::new(radio), config, runtime, injected_rx)
    }
}
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hrpd_only_tx_scale_applies_configured_backoff() {
        assert!((hrpd_only_tx_scale(0.4) - 0.3).abs() < 1e-6);
        assert!((hrpd_only_tx_scale(0.2) - 0.15).abs() < 1e-6);
    }

    #[test]
    fn adjacent_composite_defers_one_x_backoff_to_composer() {
        assert_eq!(one_x_synth_scale(0.45, true), 1.0);
        assert_eq!(one_x_synth_scale(0.45, false), 0.45);
    }

    #[test]
    fn hrpd_forward_ingress_is_bounded_to_available_batch_slots() {
        assert_eq!(hrpd_forward_ingress_budget(6_144), 3);
        assert_eq!(hrpd_forward_ingress_budget(3_072), 2);
    }

    #[test]
    fn hrpd_only_idle_waveform_stays_below_full_scale_after_backoff() {
        let mut modulator = evdo::HrpdForwardSlotModulator::new(0, 32_768);
        let chips = modulator.next_block(0, 32_768);
        let mut shaper = TxPulseShaper::new(SR1_CHIP_RATE_HZ as usize * 4).unwrap();
        let samples = shaper.shape(&chips);
        // The DAC clips I and Q separately.
        let unscaled_peak = samples
            .iter()
            .map(|sample| sample.re.abs().max(sample.im.abs()))
            .fold(0.0, f32::max);
        let scaled_peak = unscaled_peak * hrpd_only_tx_scale(0.3);

        assert!(
            scaled_peak <= 1.0,
            "backed-off HRPD-only peak exceeds full scale: raw={unscaled_peak:.3} scaled={scaled_peak:.3}"
        );
    }
}
