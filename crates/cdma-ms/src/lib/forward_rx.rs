use cdma_bts::phy::coding::{
    block_interleaver::{self, BitReversalInterleaver},
    convolutional::{SoftViterbiDecoder, get_1_2_k9_encoder},
    long_code::LongCodeGenerator,
};
use cdma_bts::phy::walsh::WalshDecoder;
use cdma_bts::receiver::layer3::PagingMessage;
use cdma_bts::receiver::paging::PagingChannelRate;
use cdma_bts::receiver::pipelined::generic_rake_receiver::GenericRakeReceiver;
use cdma_bts::receiver::pipelined::mobile_station::PagingRate;
use cdma_bts::receiver::pipelined::pn_lc_correlator::{PnLcConfig, PnLcCorrelator};
use cdma_bts::receiver::pipelined::{
    DecimatorProcessor, DeinterleaverProcessor, LongCodeDescrambler, MatchedFilterTracker,
    MobileStation, PagingChannelProcessor, PipelineProcessor, PipelineProcessorShared,
    PnAlignProcessor, PulseMatchedFilterProcessor, SampleBlock, SoftViterbiDecoderProcessor,
    SyncChannelProcessor, Unrepeater, VecEmitter, WalshPilotCombiner, flush_sub_chain,
    run_sub_chain,
};
use cdma_common::bits::Bitstream;
use cdma_common::paging::imsi_s_from_imsi;
use num_complex::Complex32;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU64, Ordering};

use crate::forward_rx::directed::{DirectedBody, MsAddressing};
use crate::forward_traffic_rx::{
    ForwardTrafficConfig, ForwardTrafficRc3Processor, TRAFFIC_POWER_CONTROL_EVENT,
    TRAFFIC_SIGNALING_EVENT, UnsupportedForwardPdu,
};

/// Chips from the end of the sync superframe to where the paging (and traffic)
/// long-code state becomes valid: 320 ms at 1.2288 Mcps.
const SR1_CHIPS_320MS: usize = 393_216;

/// Chips per SYS_TIME unit (80 ms at 1.2288 Mcps).
const SYS_TIME_UNIT_CHIPS: u64 = 98_304;

pub(crate) mod tags {
    pub const SYNC_EVENT: &str = "ms_sync_event";
    pub const SYNC_MSG_TYPE: &str = "sync_msg_type";
    pub const SYNC_PILOT_PN: &str = "sync_pilot_pn";
    pub const SYNC_SYS_TIME: &str = "sync_sys_time";
    pub const SYNC_LC_STATE: &str = "sync_lc_state";

    pub const SYNC_SID: &str = "sync_sid";
    pub const SYNC_NID: &str = "sync_nid";
    pub const SYNC_P_REV: &str = "sync_p_rev";
    pub const SYNC_MIN_P_REV: &str = "sync_min_p_rev";
    pub const SYNC_LP_SEC: &str = "sync_lp_sec";
    pub const SYNC_LTM_OFF: &str = "sync_ltm_off";
    pub const SYNC_DAYLT: &str = "sync_daylt";
    pub const SYNC_PRAT: &str = "sync_prat";
    pub const SYNC_CDMA_FREQ: &str = "sync_cdma_freq";

    pub const TRACKER_PILOT_PHASE: &str = "pilot_phase";
    pub const TRACKER_LOCK_LOST: &str = "upstream_lock_lost";

    pub const PAGING_EVENT: &str = "paging_event";
    pub const PAGING_CRC_VALID: &str = "paging_crc_valid";
    pub const PAGING_MSG_TYPE: &str = "paging_msg_type";
}

pub mod directed;
pub(crate) mod pilot_meter;

const OVERSAMPLE: usize = 4;

const FINGER_COHERENT_CHIPS: usize = 16_384;

const FORWARD_BATCH_SAMPLES: usize = 4_096;

/// Sync Channel Message PRAT values and the Paging Channel rates they select
/// (C.S0005-E §3.7.1.3).
const PRAT_9600_BPS: u8 = 0b00;
const PRAT_4800_BPS: u8 = 0b01;
const PAGING_RATE_FULL_BPS: u32 = 9_600;
const PAGING_RATE_HALF_BPS: u32 = 4_800;

#[derive(Default)]
pub struct Measurements {
    locked: AtomicBool,
    lock_lost: AtomicBool,
    /// Pilot Ec/Io, in centi-dB.
    ec_io_cdb: AtomicI64,
    pilot_symbols: AtomicU64,
    /// Total received power at the chain input, in centi-dBFS.
    rx_power_cdbfs: AtomicI64,
    carrier_offset_centihz: AtomicI64,
}

/// Value of a measurement that has not been made yet.
pub const NOT_MEASURED_DB: f32 = f32::NAN;

impl Measurements {
    pub(crate) fn set_carrier_offset_hz(&self, hz: f32) {
        self.carrier_offset_centihz
            .store((hz * 100.0) as i64, Ordering::Relaxed);
    }

    pub(crate) fn carrier_offset_hz(&self) -> f64 {
        self.carrier_offset_centihz.load(Ordering::Relaxed) as f64 / 100.0
    }

    pub(crate) fn set_locked(&self, locked: bool) {
        self.locked.store(locked, Ordering::Relaxed);
    }

    pub fn locked(&self) -> bool {
        self.locked.load(Ordering::Relaxed)
    }

    pub(crate) fn set_ec_io_db(&self, ec_io_db: f32) {
        self.ec_io_cdb
            .store((ec_io_db * 100.0) as i64, Ordering::Relaxed);
    }

    /// Pilot Ec/Io in dB, or NaN before the meter has a value.
    pub fn ec_io_db(&self) -> f32 {
        if self.pilot_symbols() == 0 {
            return NOT_MEASURED_DB;
        }
        self.ec_io_cdb.load(Ordering::Relaxed) as f32 / 100.0
    }

    pub(crate) fn add_pilot_symbols(&self, symbols: u64) {
        self.pilot_symbols.fetch_add(symbols, Ordering::Relaxed);
    }

    pub fn pilot_symbols(&self) -> u64 {
        self.pilot_symbols.load(Ordering::Relaxed)
    }

    pub(crate) fn set_rx_power_dbfs(&self, dbfs: f32) {
        self.rx_power_cdbfs
            .store((dbfs * 100.0) as i64, Ordering::Relaxed);
    }

    /// Mean input power relative to full scale, in dB.
    pub fn rx_power_dbfs(&self) -> f32 {
        self.rx_power_cdbfs.load(Ordering::Relaxed) as f32 / 100.0
    }

    pub fn snapshot(&self) -> PilotMeasurement {
        PilotMeasurement {
            locked: self.locked(),
            ec_io_db: self.ec_io_db(),
            rx_power_dbfs: self.rx_power_dbfs(),
            pilot_symbols: self.pilot_symbols(),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct PilotMeasurement {
    pub locked: bool,
    /// NaN until the pilot meter has run.
    pub ec_io_db: f32,
    pub rx_power_dbfs: f32,
    pub pilot_symbols: u64,
}

struct LockTap {
    state: Arc<Measurements>,
}

impl PipelineProcessor for LockTap {
    fn process_block(&mut self, block: SampleBlock) -> Vec<SampleBlock> {
        if block.tags.contains_key(tags::TRACKER_LOCK_LOST) {
            self.state.set_locked(false);
            self.state.lock_lost.store(true, Ordering::Relaxed);
        } else if block.tags.contains_key(tags::TRACKER_PILOT_PHASE) {
            self.state.set_locked(true);
        }
        vec![block]
    }
}

#[derive(Clone, Debug)]
pub struct ForwardRxConfig {
    pub bypass_paging_long_code: bool,
    pub force_start_paging_on_sync_lock: bool,
    /// Diagnostic rake path. Its sync decoder does not recover CRC-valid frames and it has no traffic tap.
    pub use_production_finger: bool,
}

impl Default for ForwardRxConfig {
    fn default() -> Self {
        ForwardRxConfig {
            bypass_paging_long_code: false,
            // Let the paging chain start via natural paging acquisition rather
            // than forcing it at sync lock. forcing it can misalign the paging
            // capsule/slot boundary and corrupt overhead-message CRC.
            force_start_paging_on_sync_lock: false,
            use_production_finger: false,
        }
    }
}

pub(crate) fn build_forward_rx_chain(
    config: ForwardRxConfig,
    traffic_config: Arc<ForwardTrafficConfig>,
    lock: Arc<Measurements>,
    paging_decoded_through_chip: Arc<AtomicU64>,
) -> Vec<PipelineProcessorShared> {
    if config.use_production_finger {
        let mut chain =
            build_finger_forward_chain(config, traffic_config, paging_decoded_through_chip);
        chain.push(Box::new(LockTap { state: lock }));
        return chain;
    }
    let mut chain: Vec<PipelineProcessorShared> =
        vec![Box::new(PulseMatchedFilterProcessor::new())];
    chain.extend(build_tracking_chain(
        config,
        traffic_config,
        lock,
        paging_decoded_through_chip,
    ));
    chain
}

const PN_EPOCH_LATENCY_SAMPLES: i64 = -45;

const PN_PERIOD_CHIPS: usize = 32_768;

fn build_finger_forward_chain(
    config: ForwardRxConfig,
    _traffic_config: Arc<ForwardTrafficConfig>,
    paging_decoded_through_chip: Arc<AtomicU64>,
) -> Vec<PipelineProcessorShared> {
    let swap_pair = false;
    let conv_invert = false;
    let builder_config = config.clone();

    let mut cfg = PnLcConfig::default_4x();
    cfg.coherent_chips = FINGER_COHERENT_CHIPS;
    let correlator = PnLcCorrelator::new(
        cfg.with_noncoherent_segments(8)
            // Forward is plain QPSK: `(I+jQ)(PN_I - jPN_Q)`, so the plain PN
            // reference (already conjugated to `PN_I - jPN_Q`) despreads it.
            .with_split_pn_reference(false)
            .with_lc_decimation(1)
            // The finger's long code is a no-op (raw mask 0), so there is no
            // long-code phase to sweep.
            .with_lc_half_span(0)
            .with_integrate_and_dump(true)
            .with_fractional_timing_recovery(true)
            .with_suppress_search_when_locked(true)
            .with_snr_threshold(2.0)
            .with_preamble_coh_norm_min(0.02)
            // A constant (zero-mask) long code has no phase to discriminate, so
            // the best-over-second-best LC ratio is meaningless — drop that gate.
            .with_lc_best_over_second_min(0.0)
            .with_preamble_hits_required(1),
        LongCodeGenerator::new_traffic_channel_raw_mask(0),
        Box::new(move || {
            vec![Box::new(
                MobileStation::new(
                    sync_sub_chain(swap_pair, conv_invert),
                    paging_sub_chain_builder(
                        builder_config.clone(),
                        swap_pair,
                        conv_invert,
                        paging_decoded_through_chip.clone(),
                    ),
                )
                .with_force_start_paging_on_sync_lock(
                    builder_config.force_start_paging_on_sync_lock,
                ),
            ) as PipelineProcessorShared]
        }),
    );

    vec![
        Box::new(PulseMatchedFilterProcessor::new()),
        Box::new(GenericRakeReceiver::new(correlator).with_max_fingers(1)),
    ]
}

/// The Sync Channel Message (C.S0005 §3.7.2.3.2.26), as decoded.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct SyncParameters {
    pub msg_type: u8,
    pub p_rev: u8,
    pub min_p_rev: u8,
    pub sid: u16,
    pub nid: u16,
    pub pilot_pn: u16,
    pub lc_state: u64,
    /// SYS_TIME in 80 ms units since the CDMA epoch.
    pub sys_time: u64,
    pub lp_sec: u8,
    /// LTM_OFF in 30 minute units.
    pub ltm_off: i8,
    pub daylt: bool,
    pub prat: u8,
    pub cdma_freq: u16,
}

/// Chips per SYS_TIME unit (80 ms at 1.2288 Mcps).
impl SyncParameters {
    pub fn system_time(&self) -> cdma_common::time::CdmaSystemTime {
        cdma_common::time::system_time_from_chips(
            self.sys_time * SYS_TIME_UNIT_CHIPS,
            cdma_common::consts::SR1_CHIP_RATE_HZ as u64,
        )
    }

    pub fn paging_rate_bps(&self) -> u32 {
        match self.prat {
            PRAT_9600_BPS => PAGING_RATE_FULL_BPS,
            PRAT_4800_BPS => PAGING_RATE_HALF_BPS,
            _ => 0,
        }
    }
}

#[derive(Debug)]
pub enum ForwardEvent {
    PilotLock,
    PilotLost,
    Sync(SyncParameters),
    PilotMeasurement(PilotMeasurement),
    Paging(PagingMessage),
    PagingCrcFail { msg_type: u8 },
    PagingDecodeError { msg_type: u8, error: String },
    Directed(crate::forward_rx::directed::DirectedPdu),
    TrafficSignaling(cdma_bts::receiver::access_layer3::FdschPdu),
    TrafficUnsupported(UnsupportedForwardPdu),
    TrafficPowerControl { up: u8, down: u8 },
    TrafficVoice { rate_bps: u32, bits: Vec<u8> },
    TrafficErasure,
}

pub struct ForwardReceiver {
    chain: Vec<PipelineProcessorShared>,
    sample_rate_hz: f64,
    batch_size: usize,
    buffer: Vec<Complex32>,
    sample_pos: usize,
    /// Zero disables directed-assignment matching for forward-only decoding.
    ms_esn: u32,
    ms_imsi_s1: u32,
    ms_imsi_s2: u16,
    ms_mcc: u16,
    ms_imsi_11_12: u8,
    traffic_config: Arc<ForwardTrafficConfig>,
    config: ForwardRxConfig,
    lock: Arc<Measurements>,
    reported_locked: bool,
    chain_origin_sample: u64,
    samples_since_measurement: usize,
    sync_time_anchor: Option<SyncTimeAnchor>,
    paging_decode_time_anchor: Option<(u64, u64)>,
    paging_decoded_through_chip: Arc<AtomicU64>,
}

#[derive(Debug, Clone, Copy)]
pub struct SyncTimeAnchor {
    pub sync_chip: u64,
    /// Recovered-clock sample of the sync validity point. Subtract the current
    /// pilot slew to express this anchor on the input-sample timeline.
    pub input_sample: u64,
    pub lc_state: u64,
}

const MEASUREMENT_PERIOD_S: f64 = 1.0;

impl ForwardReceiver {
    pub fn deactivate_traffic(&mut self) {
        self.traffic_config.active.store(false, Ordering::Relaxed);
        self.traffic_config.signaling.drain();
        self.traffic_config.signaling.drain_unsupported();
        self.traffic_config.power_control.lock().clear();
    }

    pub fn new(config: ForwardRxConfig, sample_rate_hz: f64) -> Self {
        let traffic_config = Arc::new(ForwardTrafficConfig::default());
        let lock = Arc::new(Measurements::default());
        let paging_decoded_through_chip = Arc::new(AtomicU64::new(0));
        ForwardReceiver {
            chain: build_forward_rx_chain(
                config.clone(),
                traffic_config.clone(),
                lock.clone(),
                paging_decoded_through_chip.clone(),
            ),
            sample_rate_hz,
            batch_size: FORWARD_BATCH_SAMPLES,
            buffer: Vec::new(),
            sample_pos: 0,
            ms_esn: 0,
            ms_imsi_s1: 0,
            ms_imsi_s2: 0,
            ms_mcc: 0,
            ms_imsi_11_12: 0,
            traffic_config,
            config,
            lock,
            reported_locked: false,
            chain_origin_sample: 0,
            samples_since_measurement: 0,
            sync_time_anchor: None,
            paging_decode_time_anchor: None,
            paging_decoded_through_chip,
        }
    }

    pub fn reset(&mut self) {
        self.traffic_config = Arc::new(ForwardTrafficConfig::default());
        self.lock = Arc::new(Measurements::default());
        self.paging_decoded_through_chip = Arc::new(AtomicU64::new(0));
        self.chain = build_forward_rx_chain(
            self.config.clone(),
            self.traffic_config.clone(),
            self.lock.clone(),
            self.paging_decoded_through_chip.clone(),
        );
        self.buffer.clear();
        self.sample_pos = 0;
        self.reported_locked = false;
        self.chain_origin_sample = 0;
        self.samples_since_measurement = 0;
        self.sync_time_anchor = None;
        self.paging_decode_time_anchor = None;
    }

    pub fn pilot_locked(&self) -> bool {
        self.lock.locked()
    }

    pub fn measurements(&self) -> &Measurements {
        &self.lock
    }

    pub fn sync_time_anchor(&self) -> Option<SyncTimeAnchor> {
        self.sync_time_anchor
    }

    /// Sample slews the pilot tracker has applied since it locked. The network
    /// has run this far ahead of the receive sample counter.
    pub fn pilot_slew(&self) -> i64 {
        self.traffic_config.pilot_slew()
    }

    pub fn traffic_forward_confirmed(&self) -> bool {
        self.traffic_config.forward_confirmed()
    }

    pub fn take_traffic_power_measurements(&self) -> (u32, u32) {
        self.traffic_config.take_power_measurements()
    }

    pub fn traffic_forward_lost(&self) -> bool {
        self.traffic_config.forward_lost()
    }

    pub fn paging_decoded_through_network_chip(&self) -> Option<u64> {
        let decoded_chip = self.paging_decoded_through_chip.load(Ordering::Relaxed);
        let (paging_anchor_chip, network_anchor_chip) = self.paging_decode_time_anchor?;
        decoded_chip
            .checked_sub(paging_anchor_chip)
            .and_then(|offset| network_anchor_chip.checked_add(offset))
    }

    pub fn sync_lc_anchor(&self) -> Option<(u64, u64)> {
        use std::sync::atomic::Ordering::Relaxed;
        if self.traffic_config.anchor_set.load(Relaxed) {
            let chip = self.traffic_config.paging_start_chip.load(Relaxed);
            let state = self.traffic_config.lc_state.load(Relaxed) as u64;
            (chip >= 0).then_some((chip as u64, state))
        } else {
            None
        }
    }

    pub fn paging_anchor_chip(&self) -> Option<u64> {
        if self
            .traffic_config
            .anchor_set
            .load(std::sync::atomic::Ordering::Relaxed)
        {
            let chip = self
                .traffic_config
                .paging_start_chip
                .load(std::sync::atomic::Ordering::Relaxed);
            (chip >= 0).then_some(chip as u64)
        } else {
            None
        }
    }

    fn lock_transition(&mut self, events: &mut Vec<ForwardEvent>) {
        let locked = self.lock.locked();
        if locked != self.reported_locked {
            self.reported_locked = locked;
            events.push(if locked {
                ForwardEvent::PilotLock
            } else {
                ForwardEvent::PilotLost
            });
        }
    }

    pub fn with_ms_identity(mut self, identity: &crate::tx::MsAccessIdentity) -> Self {
        self.ms_esn = identity.esn;
        if let Some((s1, s2)) = imsi_s_from_imsi(&format!("{:010}", identity.imsi_s)) {
            self.ms_imsi_s1 = s1;
            self.ms_imsi_s2 = s2;
        }
        self.ms_mcc = cdma_common::paging::mcc_from_digits(&identity.mcc).unwrap_or(0);
        self.ms_imsi_11_12 =
            cdma_common::paging::imsi_11_12_from_digits(&identity.imsi_11_12).unwrap_or(0);
        self
    }

    /// Samples taken in since the last reset. Sample positions this receiver
    /// reports count from that reset, not from the start of the radio stream.
    pub fn samples_accepted(&self) -> u64 {
        (self.sample_pos + self.buffer.len()) as u64
    }

    pub fn push(&mut self, samples: &[Complex32]) -> Vec<ForwardEvent> {
        let mut events = Vec::new();
        let mut input = samples;

        if !self.buffer.is_empty() {
            let need = self.batch_size - self.buffer.len();
            let take = need.min(input.len());
            self.buffer.extend_from_slice(&input[..take]);
            input = &input[take..];
            if self.buffer.len() == self.batch_size {
                let batch = std::mem::take(&mut self.buffer);
                self.run_block(batch, &mut events);
            }
        }

        let mut chunks = input.chunks_exact(self.batch_size);
        for chunk in &mut chunks {
            self.run_block(chunk.to_vec(), &mut events);
        }
        self.buffer.extend_from_slice(chunks.remainder());
        events
    }

    pub fn flush(&mut self) -> Vec<ForwardEvent> {
        let mut events = Vec::new();
        if !self.buffer.is_empty() {
            let batch = std::mem::take(&mut self.buffer);
            self.run_block(batch, &mut events);
        }
        let mut emitter = VecEmitter::new();
        let tail = flush_sub_chain(&mut self.chain, &mut emitter);
        for blk in tail.iter().chain(emitter.blocks.iter()) {
            events.extend(self.block_events(blk));
        }
        self.lock_transition(&mut events);
        events
    }

    pub fn decode_all(&mut self, samples: &[Complex32]) -> Vec<ForwardEvent> {
        let mut events = self.push(samples);
        events.extend(self.flush());
        events
    }

    fn run_block(&mut self, batch: Vec<Complex32>, events: &mut Vec<ForwardEvent>) {
        let n = batch.len();
        if n > 0 {
            let mean_power = batch.iter().map(|s| s.norm_sqr()).sum::<f32>() / n as f32;
            self.lock
                .set_rx_power_dbfs(10.0 * mean_power.max(1e-12).log10());
        }
        self.samples_since_measurement += n;
        let report_every = (self.sample_rate_hz * MEASUREMENT_PERIOD_S) as usize;
        if self.samples_since_measurement >= report_every {
            self.samples_since_measurement = 0;
            if self.lock.locked() {
                events.push(ForwardEvent::PilotMeasurement(self.lock.snapshot()));
            }
        }
        let block =
            SampleBlock::new(batch, self.sample_pos).with_sample_rate_hz(self.sample_rate_hz);
        self.sample_pos += n;
        let mut emitter = VecEmitter::new();
        let out = run_sub_chain(&mut self.chain, block, &mut emitter);
        if self.lock.lock_lost.swap(false, Ordering::Relaxed) {
            // A new PN epoch invalidates every buffered frame and long-code anchor.
            let sample_pos = self.sample_pos;
            self.reset();
            self.sample_pos = sample_pos;
            self.chain_origin_sample = sample_pos as u64;
            events.push(ForwardEvent::PilotLost);
            return;
        }
        let mut lock_events = Vec::new();
        self.lock_transition(&mut lock_events);
        events.extend(lock_events);
        for blk in out.iter().chain(emitter.blocks.iter()) {
            self.latch_time_anchor(blk);
            events.extend(self.block_events(blk));
        }
    }

    fn latch_time_anchor(&mut self, blk: &SampleBlock) {
        if blk.tags.get(tags::SYNC_EVENT) == Some(&1) {
            let Some(pn_epoch) = self.traffic_config.pn_epoch() else {
                return;
            };
            let pilot_pn = blk.tags.get(tags::SYNC_PILOT_PN).copied().unwrap_or(0);
            let sys_time = blk.tags.get(tags::SYNC_SYS_TIME).copied().unwrap_or(0) as u64;
            let last_sf_end = blk
                .tags
                .get("sync_last_superframe_end_chip")
                .copied()
                .unwrap_or(0);
            let lc_state = blk.tags.get(tags::SYNC_LC_STATE).copied().unwrap_or(0) as u64;
            // `paging_start` counts chips from the PN-aligned stream's chip
            // zero, which sits at receive sample `pn_epoch`. A radio stream
            // starts anywhere in the PN period, so the epoch has to be part of
            // the anchor. The sync message describes the period before the one
            // the aligned stream counts from, which takes one period back off.
            let paging_start = last_sf_end + SR1_CHIPS_320MS as i64 - pilot_pn * 64;
            let period_samples = (PN_PERIOD_CHIPS * OVERSAMPLE) as i64;
            let epoch_offset = pn_epoch + PN_EPOCH_LATENCY_SAMPLES - period_samples;
            let input_sample = paging_start * OVERSAMPLE as i64 + epoch_offset;
            log::debug!(
                "ms_rx: time anchor pn_epoch={} slew={} epoch_offset={} paging_start_chip={} sys_time_chip={} input_sample={}",
                pn_epoch,
                self.pilot_slew(),
                epoch_offset,
                paging_start,
                sys_time * SYS_TIME_UNIT_CHIPS,
                input_sample
            );
            if input_sample >= 0 {
                if self.paging_decode_time_anchor.is_none() && paging_start >= 0 {
                    self.paging_decode_time_anchor =
                        Some((paging_start as u64, sys_time * SYS_TIME_UNIT_CHIPS));
                }
                self.sync_time_anchor = Some(SyncTimeAnchor {
                    sync_chip: sys_time * SYS_TIME_UNIT_CHIPS,
                    input_sample: self.chain_origin_sample + input_sample as u64,
                    lc_state,
                });
            }
        }
    }
}

fn block_payload_bits(blk: &SampleBlock) -> Vec<u8> {
    blk.samples
        .iter()
        .map(|s| if s.re >= 0.5 { 1 } else { 0 })
        .collect()
}

impl ForwardReceiver {
    fn ms_addressing(&self) -> MsAddressing {
        MsAddressing {
            esn: self.ms_esn,
            imsi_s1: self.ms_imsi_s1,
            imsi_s2: self.ms_imsi_s2,
            mcc: self.ms_mcc,
            imsi_11_12: self.ms_imsi_11_12,
        }
    }

    fn block_events(&self, blk: &SampleBlock) -> Vec<ForwardEvent> {
        let mut events = Vec::new();
        if blk.tags.get(tags::SYNC_EVENT) == Some(&1) {
            let pilot_pn = blk.tags.get(tags::SYNC_PILOT_PN).copied().unwrap_or(0) as u16;
            let lc_state = blk.tags.get(tags::SYNC_LC_STATE).copied().unwrap_or(0) as u64;
            // The forward traffic long code is valid at the same chip the paging
            // long code is: 320 ms past the last sync superframe, less the pilot
            // PN offset. Hand that anchor to the forward-traffic decoder.
            let last_sf_end = blk
                .tags
                .get("sync_last_superframe_end_chip")
                .copied()
                .unwrap_or(0);
            let paging_start = last_sf_end + SR1_CHIPS_320MS as i64 - (pilot_pn as i64) * 64;
            let sys_time = blk.tags.get(tags::SYNC_SYS_TIME).copied().unwrap_or(0);
            log::debug!(
                "ms_ftraffic: sync stream_chip={} sys_time={} last_sf_end={} paging_start={}",
                blk.chip_start,
                sys_time,
                last_sf_end,
                paging_start
            );
            self.traffic_config.set_anchor(paging_start, lc_state);

            let tag = |key: &str| blk.tags.get(key).copied().unwrap_or(0);
            events.push(ForwardEvent::Sync(SyncParameters {
                msg_type: tag(tags::SYNC_MSG_TYPE) as u8,
                p_rev: tag(tags::SYNC_P_REV) as u8,
                min_p_rev: tag(tags::SYNC_MIN_P_REV) as u8,
                sid: tag(tags::SYNC_SID) as u16,
                nid: tag(tags::SYNC_NID) as u16,
                pilot_pn,
                lc_state,
                sys_time: tag(tags::SYNC_SYS_TIME) as u64,
                lp_sec: tag(tags::SYNC_LP_SEC) as u8,
                ltm_off: tag(tags::SYNC_LTM_OFF) as i8,
                daylt: tag(tags::SYNC_DAYLT) != 0,
                prat: tag(tags::SYNC_PRAT) as u8,
                cdma_freq: tag(tags::SYNC_CDMA_FREQ) as u16,
            }));
        }
        if blk.tags.get(TRAFFIC_SIGNALING_EVENT) == Some(&1) {
            for pdu in self.traffic_config.signaling.drain() {
                events.push(ForwardEvent::TrafficSignaling(pdu));
            }
            for header in self.traffic_config.signaling.drain_unsupported() {
                events.push(ForwardEvent::TrafficUnsupported(header));
            }
        }
        if blk.tags.get(TRAFFIC_POWER_CONTROL_EVENT) == Some(&1) {
            for (up, down) in self.traffic_config.power_control.lock().drain(..) {
                events.push(ForwardEvent::TrafficPowerControl { up, down });
            }
        }
        if blk.tags.get(crate::forward_traffic_rx::TRAFFIC_VOICE_EVENT) == Some(&1) {
            for (rate_bps, bits) in self.traffic_config.voice.lock().drain(..) {
                // rate_bps 0 is the RX's erasure marker for a lost speech frame.
                if rate_bps == 0 {
                    events.push(ForwardEvent::TrafficErasure);
                } else {
                    events.push(ForwardEvent::TrafficVoice { rate_bps, bits });
                }
            }
        }
        if blk.tags.get(tags::PAGING_EVENT) == Some(&1) {
            let msg_type = blk.tags.get(tags::PAGING_MSG_TYPE).copied().unwrap_or(0) as u8;
            if blk.tags.get(tags::PAGING_CRC_VALID) == Some(&1) {
                let payload = block_payload_bits(blk);
                if directed::is_directed(msg_type) {
                    match directed::decode_directed(&payload, &self.ms_addressing()) {
                        Ok(Some(pdu)) => {
                            if let DirectedBody::ChannelAssignment(a) = &pdu.body {
                                self.traffic_config.activate(
                                    a.walsh_code,
                                    a.frame_offset,
                                    self.ms_esn,
                                );
                            }
                            events.push(ForwardEvent::Directed(pdu));
                        }
                        Ok(None) => {}
                        Err(error) => {
                            events.push(ForwardEvent::PagingDecodeError { msg_type, error })
                        }
                    }
                } else {
                    match PagingMessage::decode(&Bitstream::new_init(&payload)) {
                        Ok(msg) => events.push(ForwardEvent::Paging(msg)),
                        Err(error) => {
                            events.push(ForwardEvent::PagingDecodeError { msg_type, error })
                        }
                    }
                }
            } else {
                events.push(ForwardEvent::PagingCrcFail { msg_type });
            }
        }
        events
    }
}

fn build_tracking_chain(
    config: ForwardRxConfig,
    traffic_config: Arc<ForwardTrafficConfig>,
    lock: Arc<Measurements>,
    paging_decoded_through_chip: Arc<AtomicU64>,
) -> Vec<PipelineProcessorShared> {
    let swap_pair = false;
    let conv_invert = false;

    vec![
        Box::new(MatchedFilterTracker::new(OVERSAMPLE)),
        Box::new(LockTap {
            state: lock.clone(),
        }),
        Box::new(PnAlignProcessor::new(OVERSAMPLE).with_reset_on_tag("upstream_lock_lost")),
        Box::new(DecimatorProcessor::new(OVERSAMPLE)),
        Box::new(pilot_meter::PilotMeter::new(lock.clone())),
        Box::new(ForwardTrafficRc3Processor::new(traffic_config)),
        Box::new(
            MobileStation::new(
                sync_sub_chain(swap_pair, conv_invert),
                paging_sub_chain_builder(
                    config.clone(),
                    swap_pair,
                    conv_invert,
                    paging_decoded_through_chip,
                ),
            )
            .with_force_start_paging_on_sync_lock(config.force_start_paging_on_sync_lock),
        ),
    ]
}

fn sync_sub_chain(swap_pair: bool, conv_invert: bool) -> Vec<PipelineProcessorShared> {
    vec![
        Box::new(WalshPilotCombiner::new(
            WalshDecoder::new::<64>(32),
            WalshDecoder::new::<64>(0),
        )),
        Box::new(Unrepeater::new(4)),
        // Sync interleaver blocks start at the pilot PN roll (C.S0002-E §3.1.3.3.1).
        Box::new(
            DeinterleaverProcessor::new(
                BitReversalInterleaver::new(block_interleaver::SR1_PARAMS_128),
                2,
            )
            .with_reset_on_tag("upstream_lock_lost"),
        ),
        Box::new(
            SoftViterbiDecoderProcessor::new(
                SoftViterbiDecoder::new(get_1_2_k9_encoder()),
                swap_pair,
                conv_invert,
            )
            .with_reset_on_tag("upstream_lock_lost"),
        ),
        Box::new(SyncChannelProcessor::new().with_reset_on_tag("upstream_lock_lost")),
    ]
}

type PagingBuilder = Box<dyn Fn(u16, u64, PagingRate) -> Vec<PipelineProcessorShared> + Send>;

fn paging_sub_chain_builder(
    config: ForwardRxConfig,
    swap_pair: bool,
    conv_invert: bool,
    paging_decoded_through_chip: Arc<AtomicU64>,
) -> PagingBuilder {
    Box::new(
        move |pilot_pn: u16, lc_state: u64, paging_rate: PagingRate| {
            paging_decoded_through_chip.store(0, Ordering::Relaxed);
            let lc_gen = LongCodeGenerator::new_paging_channel_with_state(1, pilot_pn, lc_state);
            let (unrepeat_factor, paging_ch_rate) = match paging_rate {
                PagingRate::Rate9600 => (1, PagingChannelRate::Rate9600),
                PagingRate::Rate4800 => (2, PagingChannelRate::Rate4800),
            };
            vec![
                Box::new(WalshPilotCombiner::new(
                    WalshDecoder::new::<64>(1),
                    WalshDecoder::new::<64>(0),
                )),
                Box::new(Unrepeater::new(unrepeat_factor)),
                Box::new(
                    LongCodeDescrambler::new(lc_gen, 64)
                        .with_bypass(config.bypass_paging_long_code),
                ),
                // Paging frames start on zero-offset PN even-second boundaries (C.S0002-E §3.1.3.4.1).
                Box::new(
                    DeinterleaverProcessor::new(
                        BitReversalInterleaver::new(block_interleaver::SR1_PARAMS_384),
                        1,
                    )
                    .with_reset_on_tag("upstream_lock_lost"),
                ),
                Box::new(
                    SoftViterbiDecoderProcessor::new(
                        SoftViterbiDecoder::new(get_1_2_k9_encoder()),
                        swap_pair,
                        conv_invert,
                    )
                    .with_reset_on_tag("upstream_lock_lost"),
                ),
                Box::new(
                    PagingChannelProcessor::new_with_rate(paging_ch_rate)
                        .with_decode_progress(paging_decoded_through_chip.clone()),
                ),
            ]
        },
    )
}

#[cfg(test)]
mod decode_progress_tests {
    use super::*;

    #[test]
    fn paging_progress_keeps_its_original_network_anchor_across_sync_updates() {
        let mut receiver = ForwardReceiver::new(ForwardRxConfig::default(), 4_915_200.0);
        receiver.paging_decode_time_anchor = Some((10_000, 1_000_000));
        receiver.sync_time_anchor = Some(SyncTimeAnchor {
            sync_chip: 1_000_000,
            input_sample: 40_000,
            lc_state: 0,
        });
        receiver
            .paging_decoded_through_chip
            .store(12_000, Ordering::Relaxed);
        assert_eq!(
            receiver.paging_decoded_through_network_chip(),
            Some(1_002_000)
        );

        receiver.sync_time_anchor = Some(SyncTimeAnchor {
            sync_chip: 1_491_520,
            input_sample: 2_005_000,
            lc_state: 0,
        });
        assert_eq!(
            receiver.paging_decoded_through_network_chip(),
            Some(1_002_000)
        );

        receiver.reset();
        assert_eq!(receiver.paging_decoded_through_network_chip(), None);
    }
}
