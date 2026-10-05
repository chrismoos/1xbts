mod reverse;
mod scan;

use std::collections::VecDeque;
use std::sync::Arc;
use std::time::Duration;

use num_complex::Complex32;

use crate::access::{AccessAttempt, AccessOutcome};
use crate::event::EventSink;
use crate::forward_rx::directed::{
    DirectedBody, DirectedPdu, ORDER_INTERCEPT, ORDER_RELEASE, ORDER_REORDER,
};
use crate::forward_rx::{
    ForwardEvent, ForwardReceiver, ForwardRxConfig, PilotMeasurement, SyncParameters,
};
use crate::lac::MsgSeqCounters;
use crate::ms::{AccessReason, MsCore, MsEvent, MsProtocolState, OverheadCache, PagingStats, T57M};
use crate::prl_scan::{PrlScanPlan, PrlVerdict, ScanChannel};
use crate::traffic::{OutgoingSms, TrafficOutcome, TrafficSession};
use crate::tx::{AccessChannelConfig, MsAccessIdentity, SERVICE_OPTION_SMS};

/// Chips per 20 ms access frame (C.S0002-E SR1).
const CHIPS_PER_FRAME: u64 = 24_576;
const MIN_PREAMBLE_FRAMES: usize = 8;
const TRAILING_FRAMES: usize = 4;
const REVERSE_LEAD_CHIPS: u64 = 5 * CHIPS_PER_FRAME;
const REVERSE_START_CHIPS: u64 = 30 * CHIPS_PER_FRAME;
const ACCESS_LEAD_CHIPS: u64 = 2 * CHIPS_PER_FRAME;
const DEFAULT_TRAFFIC_POWER_STEP_DB: f32 = 1.0;
// Queued TX must reach the cell before another full correction is applied.
const TRAFFIC_POWER_UPDATE_CHIPS: u64 = 2 * REVERSE_LEAD_CHIPS;

const SR1_CHIP_RATE_HZ: f64 = 1_228_800.0;
const SMS_ORIGINATION_ATTEMPTS: u8 = 3;

pub const DEFAULT_SCAN_DWELL_MS: u32 = 300;
pub const DEFAULT_PILOT_CONFIRM_MS: u32 = 800;
/// T20m, Pilot Channel Acquisition Substate timeout (C.S0005 §2.6.1.2).
pub const PILOT_ACQUISITION_TIMEOUT_MS: u32 = 15_000;
/// T21m, Sync Channel Acquisition Substate timeout (C.S0005 §2.6.1.3).
pub const SYNC_ACQUISITION_TIMEOUT_MS: u32 = 1_000;
/// Exceeds T21m because pilot-to-sync decode can take more than one second.
pub const SYNC_ACQUISITION_RECOVERY_MS: u32 = 15_000;

#[derive(Debug, Clone)]
pub struct ReverseBurst {
    pub samples: Vec<Complex32>,
    /// Network system-time chip of the first sample.
    pub absolute_chip_start: u64,
    pub stream_sample_start: u64,
    pub power_dbm: f32,
    pub access_power_offset_db: Option<f32>,
    /// Relative traffic gain change applied once when this burst is queued.
    pub traffic_gain_delta_db: f32,
    pub forward_carrier_offset_hz: f64,
    pub end_of_burst: bool,
    pub label: &'static str,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ScanMode {
    #[default]
    Camp,
    Survey,
}

impl ScanMode {
    pub fn label(self) -> &'static str {
        match self {
            ScanMode::Camp => "camp",
            ScanMode::Survey => "survey",
        }
    }
}

#[derive(Debug, Clone)]
pub struct ScanConfig {
    pub plan: Arc<PrlScanPlan>,
    pub mode: ScanMode,
    pub dwell_ms: u32,
    pub pilot_confirm_ms: u32,
    pub pilot_timeout_ms: u32,
    pub sync_timeout_ms: u32,
}

impl ScanConfig {
    pub fn new(plan: Arc<PrlScanPlan>) -> Self {
        ScanConfig {
            plan,
            mode: ScanMode::Camp,
            dwell_ms: DEFAULT_SCAN_DWELL_MS,
            pilot_confirm_ms: DEFAULT_PILOT_CONFIRM_MS,
            pilot_timeout_ms: PILOT_ACQUISITION_TIMEOUT_MS,
            sync_timeout_ms: SYNC_ACQUISITION_TIMEOUT_MS,
        }
    }

    pub fn with_dwell_ms(mut self, dwell_ms: u32) -> Self {
        self.dwell_ms = dwell_ms;
        self
    }

    pub fn with_mode(mut self, mode: ScanMode) -> Self {
        self.mode = mode;
        self
    }
}

#[derive(Debug, Clone)]
pub struct ChannelResult {
    pub channel: ScanChannel,
    pub pilot: Option<bool>,
    pub measurement: PilotMeasurement,
    pub sync: Option<SyncParameters>,
    pub verdict: Option<PrlVerdict>,
}

#[derive(Debug, Clone, Default)]
pub struct ScanReport {
    pub prl_id: u16,
    pub mode: ScanMode,
    pub running: bool,
    pub result: Option<String>,
    pub total_channels: usize,
    pub channels: Vec<ChannelResult>,
}

#[derive(Debug, Clone, Default)]
pub struct Diagnostics {
    pub state: String,
    pub pilot: PilotMeasurement,
    pub sync: Option<SyncParameters>,
    pub overhead: OverheadCache,
    pub paging: PagingStats,
    pub scan: Option<ScanReport>,
    pub samples_fed: u64,
    pub registered: bool,
}

const PILOT_REPORT_MIN_SYMBOLS: u64 = 384;
const PILOT_REPORT_MAX_WAIT_MS: u32 = 200;

#[derive(Debug, Clone)]
pub struct EngineConfig {
    pub identity: MsAccessIdentity,
    pub sample_rate_hz: f64,
    /// Receiver input power at 0 dBFS, added to measured dBFS (C.S0002-E §2.1.2.3.1.1).
    pub rx_reference_dbm: f32,
    pub forward_rx: ForwardRxConfig,
    pub scan: Option<ScanConfig>,
    pub power_up_delay: Duration,
}

impl Default for EngineConfig {
    fn default() -> Self {
        EngineConfig {
            identity: MsAccessIdentity::default(),
            sample_rate_hz: SR1_CHIP_RATE_HZ * 4.0,
            rx_reference_dbm: 0.0,
            forward_rx: ForwardRxConfig::default(),
            scan: None,
            power_up_delay: T57M,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TuneRequest {
    pub index: usize,
    pub total: usize,
    pub channel: ScanChannel,
}

impl TuneRequest {
    fn tag(&self) -> String {
        format!(
            "[{:>2}/{}] acq#{} {}",
            self.index + 1,
            self.total,
            self.channel.acq_index,
            self.channel.label()
        )
    }
}

struct ReverseTransmitter {
    identity: MsAccessIdentity,
    access_cfg: AccessChannelConfig,
    seq: MsgSeqCounters,
    current: Option<(AccessReason, u8)>,
}

#[derive(Debug, Clone, Copy, PartialEq)]
enum ScanPhase {
    Idle,
    Tune(usize),
    AwaitTune(usize),
    Dwell {
        index: usize,
        fed: usize,
        pilot_at: Option<usize>,
    },
    Finished,
}

struct ScanState {
    cfg: ScanConfig,
    phase: ScanPhase,
    camped_index: Option<usize>,
    fed_total: usize,
    pilots: u32,
    syncs: u32,
    rejected: u32,
    pilot_report_pending: bool,
    results: Vec<ChannelResult>,
    result: Option<String>,
}

impl ScanState {
    fn new(cfg: ScanConfig) -> Self {
        ScanState {
            cfg,
            phase: ScanPhase::Idle,
            camped_index: None,
            fed_total: 0,
            pilots: 0,
            syncs: 0,
            rejected: 0,
            pilot_report_pending: false,
            results: Vec::new(),
            result: None,
        }
    }

    fn report(&self) -> ScanReport {
        ScanReport {
            prl_id: self.cfg.plan.prl_id,
            mode: self.cfg.mode,
            running: !matches!(self.phase, ScanPhase::Idle | ScanPhase::Finished),
            result: self.result.clone(),
            total_channels: self.cfg.plan.channels.len(),
            channels: self.results.clone(),
        }
    }
}

#[derive(Debug, Clone, Copy)]
struct TimeBase {
    /// Network system time, in chips, at recovered-clock sample `at_sample`.
    sync_chip: u64,
    at_sample: u64,
    oversample: u64,
}

impl TimeBase {
    /// Network system-time chip at receive-stream sample `sample`.
    ///
    /// The anchor counts recovered-clock samples. The tracker's net inserted
    /// samples convert the radio's input count to that timeline.
    fn chip_at(&self, sample: u64, slew: i64) -> u64 {
        let samples = sample as i64 - self.at_sample as i64 + slew;
        (self.sync_chip as i64 + samples.div_euclid(self.oversample as i64)).max(0) as u64
    }

    /// Receive-stream sample at network system-time `chip`, for the tracker
    /// slew `slew`.
    fn sample_at(&self, chip: u64, slew: i64) -> u64 {
        let chips = chip as i64 - self.sync_chip as i64;
        (self.at_sample as i64 + chips * self.oversample as i64 - slew).max(0) as u64
    }
}

pub struct Engine {
    core: MsCore,
    forward: ForwardReceiver,
    reverse: ReverseTransmitter,
    oversample: u64,
    esn: u32,
    /// dBFS-to-dBm offset for the open-loop transmit power estimate.
    rx_reference_dbm: f32,
    /// Forward samples buffered in the radio but not yet consumed. The engine
    /// is this far behind the hardware clock, so reverse bursts are scheduled
    /// past `samples_fed + tx_backlog`.
    tx_backlog: u64,
    time_base: Option<TimeBase>,
    /// Decoded LC_STATE and its validity chip. Do not assume the network long code starts at the CDMA epoch.
    lc_anchor: Option<(u64, u64)>,
    access: Option<(AccessAttempt, AccessReason)>,
    access_capsule: Option<cdma_common::bits::Bitstream>,
    pending_sms: Option<OutgoingSms>,
    sms_attempts: u8,
    pending_origination: Option<(u16, String)>,
    decline_incoming: bool,
    pending_registration: bool,
    traffic: Option<TrafficSession>,
    voice_output: VecDeque<[i16; cdma_voice::SAMPLES_PER_FRAME]>,
    traffic_shaper: Option<cdma_bts::sdr::fir::ComplexFir32>,
    traffic_power_delta_db: f32,
    traffic_power_step_db: f32,
    traffic_power_frames: u64,
    traffic_tx_start_chip: Option<u64>,
    traffic_power_window: [u32; 2],
    traffic_power_report: [u32; 2],
    traffic_power_next_update_chip: u64,
    staged_probe: Option<(u64, u8, Vec<Complex32>)>,
    pending_tx: VecDeque<ReverseBurst>,
    pending_reverse_hz: Option<f64>,
    last_tuned_reverse_hz: Option<f64>,
    /// Network chip after the last queued reverse burst, so the next one does
    /// not overlap it on the air.
    reverse_end_chip: u64,
    sample_rate_hz: f64,
    scan: Option<ScanState>,
    samples_fed: u64,
    /// Radio-stream sample the forward receiver's sample zero falls on. The
    /// receiver restarts its count on every reset and the radio does not.
    forward_origin: u64,
    pilot_locked_at: Option<u64>,
    sync_at: Option<u64>,
    overhead_at: Option<u64>,
    sync_warned: bool,
    overhead_warned: bool,
}

const OVERHEAD_TIMEOUT_MS: u32 = 3_000;

impl Engine {
    pub fn new(config: EngineConfig) -> Self {
        let esn = config.identity.esn;
        let core = MsCore::new(config.identity.clone()).with_power_up_delay(config.power_up_delay);
        let identity = config.identity;
        let forward_identity = identity.clone();
        let oversample = (config.sample_rate_hz / SR1_CHIP_RATE_HZ).round().max(1.0) as u64;
        let rx_reference_dbm = config.rx_reference_dbm;
        Engine {
            core,
            forward: ForwardReceiver::new(config.forward_rx, config.sample_rate_hz)
                .with_ms_identity(&forward_identity),
            reverse: ReverseTransmitter {
                identity,
                access_cfg: AccessChannelConfig::default(),
                seq: MsgSeqCounters::default(),
                current: None,
            },
            oversample,
            esn,
            rx_reference_dbm,
            tx_backlog: 0,
            time_base: None,
            lc_anchor: None,
            access: None,
            access_capsule: None,
            pending_sms: None,
            sms_attempts: 0,
            pending_origination: None,
            decline_incoming: false,
            pending_registration: false,
            traffic_shaper: None,
            traffic: None,
            voice_output: VecDeque::new(),
            traffic_power_delta_db: 0.0,
            traffic_power_step_db: DEFAULT_TRAFFIC_POWER_STEP_DB,
            traffic_power_frames: 0,
            traffic_tx_start_chip: None,
            traffic_power_window: [0; 2],
            traffic_power_report: [0; 2],
            traffic_power_next_update_chip: 0,
            staged_probe: None,
            pending_tx: VecDeque::new(),
            pending_reverse_hz: None,
            last_tuned_reverse_hz: None,
            reverse_end_chip: 0,
            sample_rate_hz: config.sample_rate_hz,
            scan: config.scan.map(ScanState::new),
            samples_fed: 0,
            forward_origin: 0,
            pilot_locked_at: None,
            sync_at: None,
            overhead_at: None,
            sync_warned: false,
            overhead_warned: false,
        }
    }

    fn system_time_chips(&self) -> u64 {
        let slew = self.forward.pilot_slew();
        self.time_base
            .map(|tb| tb.chip_at(self.samples_fed, slew))
            .unwrap_or(0)
    }

    pub fn set_tx_backlog_samples(&mut self, samples: u64) {
        self.tx_backlog = samples;
    }

    fn reverse_now_chips(&self) -> u64 {
        let slew = self.forward.pilot_slew();
        self.time_base
            .map(|tb| tb.chip_at(self.samples_fed + self.tx_backlog, slew))
            .unwrap_or(0)
    }

    fn check_decode_progress(&mut self) {
        let now = self.samples_fed;
        if let (Some(at), None) = (self.pilot_locked_at, self.sync_at) {
            let elapsed = now.saturating_sub(at);
            if elapsed >= self.samples_for_ms(SYNC_ACQUISITION_TIMEOUT_MS) as u64
                && !self.sync_warned
            {
                self.sync_warned = true;
                log::warn!(
                    "ms_rx: pilot locked {} ms ago with no CRC-valid sync frame, the PN-aligned sync decode is not producing",
                    SYNC_ACQUISITION_TIMEOUT_MS
                );
            }
            if !self.scanning()
                && elapsed >= self.samples_for_ms(SYNC_ACQUISITION_RECOVERY_MS) as u64
            {
                log::warn!(
                    "cdma-ms: no Sync Channel Message within {} ms of pilot lock, returning to system determination",
                    SYNC_ACQUISITION_RECOVERY_MS
                );
                self.core.on_system_rejected();
                self.core.begin_pilot_search();
                self.forward.reset();
                self.pilot_locked_at = None;
                self.sync_at = None;
                self.sync_warned = false;
                return;
            }
        }
        if let (Some(at), None) = (self.sync_at, self.overhead_at)
            && !self.overhead_warned
            && *self.core.state() == MsProtocolState::Idle
            && now.saturating_sub(at) >= self.samples_for_ms(OVERHEAD_TIMEOUT_MS) as u64
        {
            self.overhead_warned = true;
            log::warn!(
                "ms_rx: no overhead message {} ms after sync, the paging decode is not producing",
                OVERHEAD_TIMEOUT_MS
            );
        }
    }

    pub fn emit_event(&self, event: MsEvent) {
        self.core.emit(event);
    }

    pub fn diagnostics(&self) -> Diagnostics {
        Diagnostics {
            state: self.core.state().label().to_string(),
            pilot: self.forward.measurements().snapshot(),
            sync: self.core.sync_info().cloned(),
            overhead: self.core.overhead().clone(),
            paging: self.core.paging_stats().clone(),
            scan: self.scan.as_ref().map(ScanState::report),
            samples_fed: self.samples_fed,
            registered: self.core.registered(),
        }
    }

    pub fn add_event_sink(&mut self, sink: Arc<dyn EventSink>) {
        self.core.add_event_sink(sink);
    }

    pub fn power_on(&mut self) {
        if *self.core.state() != MsProtocolState::PowerOff {
            return;
        }
        self.forward.reset();
        self.pilot_locked_at = None;
        self.sync_at = None;
        self.overhead_at = None;
        self.sync_warned = false;
        self.overhead_warned = false;
        match &mut self.scan {
            None => self.core.power_on(),
            Some(scan) => {
                if *self.core.state() != MsProtocolState::PowerOff {
                    return;
                }
                self.core.power_on_scanning();
                scan.fed_total = 0;
                scan.camped_index = None;
                scan.pilots = 0;
                scan.syncs = 0;
                scan.rejected = 0;
                scan.pilot_report_pending = false;
                scan.results.clear();
                scan.result = None;
                let plan = &scan.cfg.plan;
                log::info!("ms_scan: mode={} {}", scan.cfg.mode.label(), plan.summary());
                self.core.emit(MsEvent::ScanStarted {
                    prl_id: plan.prl_id,
                    channels: plan.channels.len() as u32,
                });
                if plan.channels.is_empty() {
                    log::warn!("ms_scan: the PRL yields no channels to scan");
                    scan.phase = ScanPhase::Finished;
                    self.finish_scan("no_permitted_system");
                } else {
                    scan.phase = ScanPhase::Tune(0);
                }
            }
        }
    }

    pub fn power_off(&mut self) {
        self.mute_reverse();
        self.forward.deactivate_traffic();
        if let Some(scan) = &mut self.scan {
            scan.phase = ScanPhase::Idle;
            scan.camped_index = None;
        }
        self.time_base = None;
        self.lc_anchor = None;
        self.access = None;
        self.access_capsule = None;
        self.staged_probe = None;
        self.traffic = None;
        self.traffic_shaper = None;
        self.traffic_power_delta_db = 0.0;
        self.traffic_power_step_db = DEFAULT_TRAFFIC_POWER_STEP_DB;
        self.traffic_power_frames = 0;
        self.traffic_tx_start_chip = None;
        self.traffic_power_window = [0; 2];
        self.traffic_power_report = [0; 2];
        self.traffic_power_next_update_chip = 0;
        self.decline_incoming = false;
        self.pending_sms = None;
        self.sms_attempts = 0;
        self.pending_origination = None;
        self.pending_registration = false;
        self.reverse_end_chip = 0;
        self.reverse.current = None;
        self.core.power_off();
    }

    pub fn originate(&mut self, service_option: u16, digits: String) {
        self.pending_origination = Some((service_option, digits));
        self.try_start_origination();
    }

    pub fn register(&mut self) {
        self.pending_registration = true;
        self.try_start_origination();
    }

    pub fn send_dtmf_burst(&mut self, burst: crate::traffic::DtmfBurst) -> Result<(), String> {
        self.traffic
            .as_mut()
            .ok_or_else(|| "DTMF requires an active voice call".to_string())?
            .send_dtmf_burst(burst)
    }

    pub fn hang_up(&mut self) {
        self.pending_origination = None;
        if let Some(session) = &mut self.traffic {
            session.release();
        } else if self.core.incoming_call() {
            self.decline_incoming = true;
        }
    }

    pub fn originate_sms(&mut self, destination: String, text: String) {
        self.pending_sms = Some(OutgoingSms {
            destination: destination.clone(),
            text,
        });
        self.sms_attempts = 1;
        self.pending_origination = Some((SERVICE_OPTION_SMS, destination));
        self.try_start_origination();
    }

    fn retry_pending_sms(&mut self) {
        if self.pending_origination.is_some()
            || self.traffic.is_some()
            || *self.core.state() != MsProtocolState::Idle
        {
            return;
        }
        let Some(sms) = self.pending_sms.as_ref() else {
            return;
        };
        let destination = sms.destination.clone();
        if self.sms_attempts >= SMS_ORIGINATION_ATTEMPTS {
            log::warn!(
                "cdma-ms: SMS to {} not sent after {} originations",
                destination,
                self.sms_attempts
            );
            self.pending_sms = None;
            self.sms_attempts = 0;
            return;
        }
        self.sms_attempts += 1;
        log::info!(
            "cdma-ms: origination {} for the SMS to {}",
            self.sms_attempts,
            destination
        );
        self.pending_origination = Some((SERVICE_OPTION_SMS, destination));
    }

    fn try_start_origination(&mut self) {
        if *self.core.state() != MsProtocolState::Idle {
            return;
        }
        if self.pending_registration {
            self.pending_registration = false;
            self.core.force_registration();
            return;
        }
        if let Some((service_option, digits)) = self.pending_origination.take() {
            self.core.originate(service_option, digits);
        }
        self.retry_pending_sms();
    }

    pub fn state(&self) -> &MsProtocolState {
        self.core.state()
    }

    pub fn core(&self) -> &MsCore {
        &self.core
    }

    pub fn on_forward_exhausted(&mut self) {
        self.flush_forward();
        if self.scanning() {
            log::warn!("ms_scan: forward source exhausted before the scan finished");
            self.finish_scan("source_exhausted");
        }
    }

    pub fn handle_forward(&mut self, samples: &[Complex32]) {
        self.samples_fed += samples.len() as u64;
        if *self.core.state() == MsProtocolState::PowerOff {
            return;
        }
        if let Some(scan) = &mut self.scan {
            scan.fed_total += samples.len();
            if let ScanPhase::Dwell { fed, .. } = &mut scan.phase {
                *fed += samples.len();
            } else if scan.phase != ScanPhase::Idle && scan.phase != ScanPhase::Finished {
                return;
            }
        }
        self.forward_origin = (self.samples_fed - samples.len() as u64)
            .saturating_sub(self.forward.samples_accepted());
        let events = self.forward.push(samples);
        for ev in events {
            self.apply_forward(ev);
        }
        self.flush_pilot_report(false);
        self.check_scan_timers();
        self.check_decode_progress();
        self.try_start_origination();
        self.drive_reverse();
        self.drive_traffic();
    }

    pub fn on_forward_discontinuity(&mut self) {
        if *self.core.state() != MsProtocolState::PowerOff {
            self.apply_forward(ForwardEvent::PilotLost);
        }
        self.forward.reset();
    }

    fn flush_pilot_report(&mut self, force: bool) {
        let Some(scan) = &self.scan else {
            return;
        };
        if !scan.pilot_report_pending {
            return;
        }
        let ScanPhase::Dwell { fed, pilot_at, .. } = scan.phase else {
            return;
        };
        let waited = fed.saturating_sub(pilot_at.unwrap_or(fed));
        let ready = self.forward.measurements().pilot_symbols() >= PILOT_REPORT_MIN_SYMBOLS
            || waited >= self.samples_for_ms(PILOT_REPORT_MAX_WAIT_MS);
        if !(force || ready) {
            return;
        }
        let m = self.forward.measurements().snapshot();
        let tag = self.channel_tag();
        log::info!(
            "ms_scan: {} pilot locked ec_io={:.1}dB rx={:.1}dBFS t={:.2}s",
            tag,
            m.ec_io_db,
            m.rx_power_dbfs,
            self.air_ms(pilot_at.unwrap_or(0)) as f64 / 1000.0
        );
        if let Some(scan) = &mut self.scan {
            scan.pilot_report_pending = false;
            if let Some(last) = scan.results.last_mut() {
                last.pilot = Some(true);
                last.measurement = m.clone();
            }
        }
        self.emit_pilot_search(true, &m);
    }

    pub fn flush_forward(&mut self) {
        let events = self.forward.flush();
        for ev in events {
            self.apply_forward(ev);
        }
        self.try_start_origination();
        self.drive_reverse();
        self.drive_traffic();
    }

    pub fn poll_transmit(&mut self) -> Option<ReverseBurst> {
        self.pending_tx.pop_front()
    }

    pub fn pending_reverse_tune(&mut self) -> Option<f64> {
        self.pending_reverse_hz.take()
    }

    fn samples_for_ms(&self, ms: u32) -> usize {
        (self.sample_rate_hz * ms as f64 / 1000.0) as usize
    }

    fn air_ms(&self, samples: usize) -> u64 {
        (samples as f64 * 1000.0 / self.sample_rate_hz) as u64
    }

    fn apply_forward(&mut self, ev: ForwardEvent) {
        match ev {
            ForwardEvent::PilotLock => {
                if self.pilot_locked_at.is_none() {
                    self.pilot_locked_at = Some(self.samples_fed);
                }
                let tag = self.channel_tag();
                if let Some(scan) = &mut self.scan {
                    if let ScanPhase::Dwell { fed, pilot_at, .. } = &mut scan.phase {
                        if pilot_at.is_none() {
                            *pilot_at = Some(*fed);
                            scan.pilots += 1;
                            scan.pilot_report_pending = true;
                            log::debug!("ms_scan: {} tracker locked, measuring", tag);
                        }
                    }
                }
                self.core.on_pilot_detected();
            }
            ForwardEvent::PilotLost => {
                log::warn!("cdma-ms: forward pilot lost, reacquiring sync and paging");
                self.reacquire_forward_link("forward pilot lost");
                if self.scanning() {
                    log::info!("ms_scan: {} tracker lost lock", self.channel_tag());
                }
            }
            ForwardEvent::Sync(params) => {
                if self.sync_at.is_none() {
                    self.sync_at = Some(self.samples_fed);
                }
                // SYS_TIME identifies the chip where the sync long-code register becomes valid.
                if let Some(anchor) = self.forward.sync_time_anchor() {
                    self.time_base = Some(TimeBase {
                        sync_chip: anchor.sync_chip,
                        at_sample: self.forward_origin + anchor.input_sample,
                        oversample: self.oversample,
                    });
                    self.lc_anchor = Some((anchor.sync_chip, anchor.lc_state));
                }
                if self.scanning() {
                    self.apply_scan_sync(params);
                } else {
                    let camped_before = *self.core.state() == MsProtocolState::Idle;
                    self.core.on_pilot_acquired(params.pilot_pn as u32);
                    self.core.on_sync_decoded(params);
                    if !camped_before && *self.core.state() == MsProtocolState::Idle {
                        self.core.begin_idle_timers(self.system_time_chips());
                    }
                    if !camped_before
                        && *self.core.state() == MsProtocolState::Idle
                        && self.pending_reverse_hz.is_none()
                    {
                        self.pending_reverse_hz = self.last_tuned_reverse_hz;
                    }
                }
            }
            ForwardEvent::PilotMeasurement(m) => {
                self.core.emit(MsEvent::PilotMeasurement {
                    ec_io_db: (!m.ec_io_db.is_nan()).then_some(m.ec_io_db),
                    rx_power_dbfs: m.rx_power_dbfs,
                });
            }
            ForwardEvent::Paging(msg) => {
                if self.overhead_at.is_none() {
                    self.overhead_at = Some(self.samples_fed);
                }
                let now_chips = self.system_time_chips();
                self.core.ingest_paging_message(&msg, now_chips)
            }
            ForwardEvent::PagingCrcFail { .. } => self.core.on_paging_crc_failed(),
            ForwardEvent::PagingDecodeError { msg_type, error } => {
                self.core.on_paging_decode_failed(msg_type, &error)
            }
            ForwardEvent::Directed(pdu) => self.on_directed(pdu),
            ForwardEvent::TrafficSignaling(pdu) => {
                if let Some(session) = &mut self.traffic {
                    for outcome in session.on_forward(&pdu) {
                        self.on_traffic_outcome(outcome);
                    }
                }
            }
            ForwardEvent::TrafficUnsupported(header) => {
                if let Some(session) = &mut self.traffic {
                    let outcome = session.on_unsupported_forward(&header);
                    self.on_traffic_outcome(outcome);
                }
            }
            ForwardEvent::TrafficPowerControl { up, down } => {
                // C.S0002-E §2.1.2.3.2 freezes the accumulator while TX is disabled.
                if self.traffic.is_some()
                    && self
                        .traffic_tx_start_chip
                        .is_some_and(|start| self.system_time_chips() >= start)
                {
                    self.traffic_power_window[0] += u32::from(up);
                    self.traffic_power_window[1] += u32::from(down);
                    self.traffic_power_report[0] += u32::from(up);
                    self.traffic_power_report[1] += u32::from(down);
                    let now = self.system_time_chips();
                    if now >= self.traffic_power_next_update_chip {
                        let [up_count, down_count] = self.traffic_power_window;
                        let total = up_count + down_count;
                        if total > 0 {
                            let balance = (up_count as f32 - down_count as f32) / total as f32;
                            self.traffic_power_delta_db = balance * self.traffic_power_step_db;
                        }
                        self.traffic_power_window = [0; 2];
                        self.traffic_power_next_update_chip = now + TRAFFIC_POWER_UPDATE_CHIPS;
                    }
                    log::debug!(
                        "cdma-ms: RC3 power feedback up={} down={} pending_delta_db={:+.2}",
                        up,
                        down,
                        self.traffic_power_delta_db,
                    );
                    self.traffic_power_frames += 1;
                    if self.traffic_power_frames % 50 == 0 {
                        log::info!(
                            "cdma-ms: RC3 power control frames={} up={} down={}",
                            self.traffic_power_frames,
                            self.traffic_power_report[0],
                            self.traffic_power_report[1],
                        );
                        self.traffic_power_report = [0; 2];
                    }
                }
            }
            ForwardEvent::TrafficVoice { rate_bps, bits } => {
                if let Some(session) = &mut self.traffic
                    && let Some(pcm) = session.decode_voice(rate_bps, &bits)
                {
                    self.voice_output.push_back(pcm);
                }
            }
            ForwardEvent::TrafficErasure => {
                if let Some(session) = &mut self.traffic
                    && let Some(pcm) = session.decode_erasure()
                {
                    self.voice_output.push_back(pcm);
                }
            }
        }
    }

    fn on_directed(&mut self, pdu: DirectedPdu) {
        let now = self.system_time_chips();
        if pdu.ack_req && self.core.note_directed_seq(pdu.msg_seq) {
            log::debug!(
                "cdma-ms: duplicate directed PDU msg_seq={}, acknowledging again",
                pdu.msg_seq
            );
            self.core.acknowledge_duplicate(pdu.msg_seq);
            return;
        }
        // VALID_ACK with matching ACK_SEQ acknowledges an access probe (C.S0004-E §2.1.1.2.2.1).
        let acked_our_access = self.access.is_some()
            && pdu.valid_ack
            && self
                .reverse
                .current
                .as_ref()
                .is_some_and(|(_, seq)| *seq == pdu.ack_seq);
        let registered_before = self.core.registered();
        let on_traffic_before = self.core.state().on_traffic();
        let explicit_origination_rejection = matches!(
            (&pdu.body, self.core.state()),
            (
                DirectedBody::Order { order, .. },
                MsProtocolState::SystemAccess {
                    reason: AccessReason::Origination { .. }
                }
            ) if matches!(*order, ORDER_RELEASE | ORDER_REORDER | ORDER_INTERCEPT)
        );
        self.core.on_directed(&pdu, now);
        if explicit_origination_rejection && self.pending_sms.take().is_some() {
            self.sms_attempts = 0;
            self.pending_origination = None;
            log::warn!("cdma-ms: SMS origination explicitly rejected, not retrying");
        }
        let assigned = self.core.state().on_traffic() && !on_traffic_before;
        if assigned {
            self.finish_access(AccessOutcome::Acknowledged);
            return;
        }
        let newly_registered = self.core.registered() && !registered_before;
        if acked_our_access || newly_registered {
            self.finish_access(AccessOutcome::Acknowledged);
            self.core.on_access_acknowledged(now);
        }
    }

    fn on_traffic_outcome(&mut self, outcome: TrafficOutcome) {
        match outcome {
            TrafficOutcome::Message {
                name,
                order,
                msg_seq,
                ack_seq,
                ack_req,
            } => self.core.emit(MsEvent::TrafficMessage {
                name,
                order,
                msg_seq,
                ack_seq,
                ack_req,
            }),
            TrafficOutcome::ServiceConnected { service_option } => {
                self.core.emit(MsEvent::ServiceConnected { service_option })
            }
            TrafficOutcome::Ringing { caller_number } => {
                let service_option = self.core.pending_service_option().unwrap_or_default();
                self.core.emit(MsEvent::CallRinging {
                    caller_number,
                    service_option,
                });
            }
            TrafficOutcome::DataBurst { burst_type, fields } => {
                if self.core.on_data_burst(burst_type, &fields).is_some() {
                    if let Some(session) = &mut self.traffic {
                        session.complete_sms();
                    }
                }
            }
            TrafficOutcome::StatusRequested {
                qualification_type,
                qualification,
                record_types,
            } => {
                let body = crate::tx::status_response_sdu(
                    self.core.identity(),
                    crate::tx::p_rev_in_use(
                        self.core
                            .base_station_p_rev()
                            .unwrap_or(self.core.identity().mob_p_rev),
                        self.core.identity().mob_p_rev,
                    ),
                    qualification_type,
                    &qualification,
                    &record_types,
                    false,
                );
                if let Some(session) = &mut self.traffic {
                    session.queue_status_response(body);
                }
                self.core.emit(MsEvent::StatusRequested { record_types });
            }
            TrafficOutcome::ReversePowerStep { db } => {
                self.traffic_power_step_db = db;
            }
            TrafficOutcome::Released { by } => {
                if self.traffic.is_some() {
                    log::info!("cdma-ms: traffic release initiated by {by}");
                }
            }
        }
    }

    pub fn push_voice_pcm(&mut self, pcm: [i16; cdma_voice::SAMPLES_PER_FRAME]) {
        if let Some(session) = &mut self.traffic {
            session.push_microphone(pcm);
        }
    }

    pub fn answer(&mut self) -> bool {
        let answered = self.traffic.as_mut().is_some_and(TrafficSession::answer);
        if answered {
            self.core.emit(MsEvent::CallAnswered);
        }
        answered
    }

    pub fn drain_voice_pcm(
        &mut self,
    ) -> impl Iterator<Item = [i16; cdma_voice::SAMPLES_PER_FRAME]> + '_ {
        self.voice_output.drain(..)
    }

    fn reacquire_forward_link(&mut self, reason: &'static str) {
        let now = self.system_time_chips();
        self.mute_reverse();
        self.traffic = None;
        self.time_base = None;
        self.lc_anchor = None;
        self.access = None;
        self.access_capsule = None;
        self.staged_probe = None;
        self.reverse.current = None;
        self.reverse_end_chip = 0;
        self.traffic_shaper = None;
        self.forward.deactivate_traffic();
        self.pilot_locked_at = None;
        self.sync_at = None;
        self.overhead_at = None;
        self.sync_warned = false;
        self.overhead_warned = false;
        self.core.on_forward_link_lost(reason, now);
    }
}

#[cfg(test)]
mod power_control_tests;
