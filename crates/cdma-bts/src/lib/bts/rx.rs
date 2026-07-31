use std::{
    f64::consts::PI,
    fs,
    io::BufWriter,
    path::PathBuf,
    sync::Arc,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    sync::mpsc,
    time::{Duration, Instant},
};

use cdma_abis::{
    bearer::{ChannelFamily, Direction, FrameContent, ReverseFchDcchFrame, TrafficFrame},
    udp_bearer::UdpBearerDatagram,
};
use cdma_common::{
    bits::Bitstream,
    diagnostics::{power_control_verbose_enabled_for_walsh, power_control_verbose_summary_every},
    error::Error,
    hrpd::air::{HrpdAccessIndication, HrpdTrafficAssignmentRequest, HrpdTrafficEvent},
    paging::{imsi_11_12_to_digits, imsi_s_to_digits_checked, mcc_to_digits},
    time,
};
use crossbeam_channel;
use hound::{SampleFormat, WavSpec, WavWriter};
use log::{debug, info, trace, warn};
use num::complex::Complex32;
use serde::Serialize;
use tokio::sync::{mpsc as tokio_mpsc, oneshot};

use crate::bts::handle::HrpdTrafficRxCommand;
use crate::lac::message_types::MessageId;
use crate::phy::{coding::long_code::LongCodeGenerator, spread::HrpdAccessTerminalPnSequence};
use crate::receiver::hrpd::long_code::HRPD_LONG_CODE_INITIAL_STATE;
use crate::receiver::hrpd::reverse_spread::hrpd_reverse_pilot_reference_from_signs;
use crate::receiver::{
    access_layer3::{AccessMessage, AccessMessageHeader, RdschPdu, access_message_type_name},
    access_pdu::ReverseAccessPdu,
    hrpd::access::{AccessFrameLayout, HrpdAccessSignalingMessage, parse_access_mac_capsule},
    pipelined::{
        HrpdReverseAccessSettings, PipelineEmitter, PipelineProcessorShared, ReverseAccessSettings,
        SampleBlock, VecEmitter, flush_sub_chain, hrpd_reverse_access_chain, reverse_access_chain,
        run_sub_chain,
    },
};
use crate::sdr::{PhasorNco, fir::SymmetricComplexFir32};

use super::{
    AccessChannelEvent, BtsCommand, BtsPowerControlRegistry, IqCaptureControlResult,
    IqCaptureStatus, RxSettings,
    handle::{RxMetrics, StageMetrics, TrafficChannelPool},
    power_control::PCG_PREDICTION_LEAD_PCGS,
    settings,
};

#[allow(dead_code)]
#[path = "rx_bearer.rs"]
mod rx_bearer;
#[allow(dead_code)]
#[path = "rx_capture.rs"]
mod rx_capture;
#[allow(dead_code)]
#[path = "rx_events.rs"]
mod rx_events;

static ACCESS_EVENT_SEQ: AtomicU64 = AtomicU64::new(1);
static REVERSE_BEARER_SEQ: AtomicU64 = AtomicU64::new(1);
fn next_access_event_id() -> String {
    format!(
        "access-{:016x}",
        ACCESS_EVENT_SEQ.fetch_add(1, Ordering::Relaxed)
    )
}

fn build_hrpd_access_indication(
    blk: &SampleBlock,
    color_code: u8,
    sector_pilot_pn: u16,
) -> Option<HrpdAccessIndication> {
    if blk.tags.get("hrpd_access_event") != Some(&1)
        || blk.tags.get("hrpd_access_mac_fragment_valid") != Some(&1)
        || blk.tags.get("hrpd_access_mac_single_fragment_fcs_valid") != Some(&1)
    {
        return None;
    }
    let bits: Vec<u8> = blk
        .samples
        .iter()
        .map(|sample| u8::from(sample.re >= 0.5))
        .collect();
    let layout = AccessFrameLayout::for_packet_bits(bits.len())?;
    let info_bits = bits.get(..layout.body_bits)?;
    let capsule = parse_access_mac_capsule(info_bits)?;
    let absolute_chip = blk
        .tags
        .get("absolute_chip_start")
        .and_then(|chip| u64::try_from(*chip).ok())
        .unwrap_or(blk.chip_start as u64);
    if capsule.messages.is_empty() {
        warn!(
            "rx_hrpd_access_empty_capsule: chip={} summary={} security_payload={} format_b_trace={}",
            absolute_chip,
            capsule.summary(),
            capsule.security_payload_hex(),
            capsule.format_b_parse_trace()
        );
    } else if capsule.security_layer_format
        || capsule
            .messages
            .iter()
            .any(|packet| matches!(packet.message, HrpdAccessSignalingMessage::SessionClose(_)))
    {
        info!(
            "rx_hrpd_access_capsule: chip={} summary={} security_payload={} format_b_trace={}",
            absolute_chip,
            capsule.summary(),
            capsule.security_payload_hex(),
            capsule.format_b_parse_trace()
        );
    }
    Some(capsule.to_air_indication(absolute_chip, color_code, sector_pilot_pn))
}

fn reverse_frame_content_from_rate_bps(rate_bps: u32) -> FrameContent {
    use cdma_abis::bearer::{
        REVERSE_FRAME_CONTENT_EIGHTH_RATE, REVERSE_FRAME_CONTENT_FULL_RATE,
        REVERSE_FRAME_CONTENT_HALF_RATE, REVERSE_FRAME_CONTENT_QUARTER_RATE,
    };
    match rate_bps {
        9600 => REVERSE_FRAME_CONTENT_FULL_RATE,
        4800 => REVERSE_FRAME_CONTENT_HALF_RATE,
        2700 | 2400 => REVERSE_FRAME_CONTENT_QUARTER_RATE,
        1500 | 1200 => REVERSE_FRAME_CONTENT_EIGHTH_RATE,
        _ => FrameContent::Idle,
    }
}

fn reverse_frame_content_from_event(event: &AccessChannelEvent) -> FrameContent {
    reverse_frame_content_from_rate_bps(event.traffic_primary_rate_bps.unwrap_or(0))
}

fn emit_reverse_primary_bearer(
    tx: &Option<mpsc::Sender<UdpBearerDatagram>>,
    event: &AccessChannelEvent,
    bts_id: u32,
    cell_id: u32,
) -> bool {
    let Some(tx) = tx else {
        warn!("emit_reverse_primary_bearer: no bearer tx configured");
        return false;
    };
    let (Some(walsh_code), Some(bits), Some(rate_bps)) = (
        event.traffic_walsh_code,
        event.traffic_primary_bits.as_ref(),
        event.traffic_primary_rate_bps,
    ) else {
        return false;
    };
    // `decoded_rdsch` events carry post-SAR LAC payloads for the local BTS
    // event path. The Abis bearer must carry the raw reverse traffic
    // information bits, emitted by `traffic_phy_frame`, so the BSC can parse
    // the MUX header and reassemble signaling itself.
    if event.decoded_rdsch.is_some() {
        return false;
    }

    // Forward every decoded primary traffic frame over bearer. The Frame
    // Content value tells the BSC how many raw information bits/MUX bits are
    // present and which rate-specific decode path applies.
    let frame_content = reverse_frame_content_from_event(event);
    if frame_content == FrameContent::Idle {
        return false;
    }
    let frame = TrafficFrame::ReverseFchDcch(ReverseFchDcchFrame {
        channel_family: ChannelFamily::Fch,
        soft_handoff_leg: 0,
        fsn: 0,
        fqi: event
            .traffic_fqi_valid
            .unwrap_or(event.traffic_phy_valid.unwrap_or(true)),
        reverse_link_quality: 0,
        scaling: 0,
        packet_arrival_time_error: 0,
        frame_content,
        fpc_s: 0,
        eib: false,
        reverse_link_information: bits.clone(),
        message_crc: 0,
    });
    let payload = match frame.encode() {
        Ok(payload) => payload,
        Err(e) => {
            warn!(
                "rx_traffic[w{}]: failed to encode reverse bearer frame: {}",
                walsh_code, e
            );
            return false;
        }
    };
    let sent = tx
        .send(UdpBearerDatagram {
            flags: 0,
            channel_family: ChannelFamily::Fch,
            direction: Direction::Reverse,
            bts_id,
            cell_id,
            bearer_id: walsh_code as u32,
            sequence_no: REVERSE_BEARER_SEQ.fetch_add(1, Ordering::Relaxed) as u32,
            tx_frame_number: event.absolute_chip_start.unwrap_or_default() as u32,
            payload,
        })
        .is_ok();
    log::trace!(
        "emit_reverse_primary_bearer: walsh={} rate={} bits={} sent={}",
        walsh_code,
        rate_bps,
        bits.len(),
        sent
    );
    sent
}

/// Send a preamble notification as an FCH Rvs null frame (frame_content=0x7F)
/// over the Abis UDP bearer.
fn emit_reverse_preamble_bearer(
    tx: &Option<mpsc::Sender<UdpBearerDatagram>>,
    walsh_code: u8,
    abs_chip: u64,
    bts_id: u32,
    cell_id: u32,
) {
    let Some(tx) = tx else {
        return;
    };
    let frame = TrafficFrame::ReverseFchDcch(ReverseFchDcchFrame {
        channel_family: ChannelFamily::Fch,
        soft_handoff_leg: 0,
        fsn: 0,
        fqi: false,
        reverse_link_quality: 0,
        scaling: 0,
        packet_arrival_time_error: 0,
        frame_content: cdma_abis::bearer::REVERSE_FRAME_CONTENT_NULL,
        fpc_s: 0,
        eib: false,
        reverse_link_information: Vec::new(),
        message_crc: 0,
    });
    let payload = match frame.encode() {
        Ok(p) => p,
        Err(e) => {
            warn!(
                "rx_traffic[w{}]: failed to encode preamble bearer frame: {}",
                walsh_code, e
            );
            return;
        }
    };
    match tx.send(UdpBearerDatagram {
        flags: 0,
        channel_family: ChannelFamily::Fch,
        direction: Direction::Reverse,
        bts_id,
        cell_id,
        bearer_id: walsh_code as u32,
        sequence_no: REVERSE_BEARER_SEQ.fetch_add(1, Ordering::Relaxed) as u32,
        tx_frame_number: abs_chip as u32,
        payload,
    }) {
        Ok(()) => debug!(
            "rx_traffic[w{}]: preamble FCH Rvs null frame sent via bearer",
            walsh_code
        ),
        Err(e) => warn!(
            "rx_traffic[w{}]: failed to send preamble bearer frame: {}",
            walsh_code, e
        ),
    }
}

/// Test/diagnostic IQ block injected into the BTS RX runtime from the outside.
#[derive(Clone, Debug)]
pub struct InjectedRxBlock {
    pub samples: Vec<Complex32>,
    pub time_ns: u64,
    pub absolute_chip_start: Option<u64>,
}

pub type InjectedRxSender = mpsc::SyncSender<InjectedRxBlock>;
pub(crate) type InjectedRxReceiver = mpsc::Receiver<InjectedRxBlock>;

pub fn injected_rx_channel(capacity: usize) -> (InjectedRxSender, InjectedRxReceiver) {
    mpsc::sync_channel(capacity.max(1))
}

/// IQ block metadata sent from the main RX thread to each traffic RX thread.
struct TrafficRxBlock {
    samples: Vec<Complex32>,
    relative_sample_start: usize,
    absolute_chip_start: u64,
    absolute_sample_start: u64,
    sample_rate_hz: usize,
    hw_time_ns: u64,
    enqueue_time: std::time::Instant,
}

/// HRPD carrier-sliced IQ block sent from the main RX loop to the access worker.
struct HrpdAccessRxBlock {
    block: CarrierSliceBlock,
    enqueue_time: Instant,
}

/// HRPD carrier-sliced IQ block sent from the main RX loop to traffic workers.
#[derive(Clone)]
struct HrpdTrafficRxBlock {
    samples: Vec<Complex32>,
    absolute_sample_start: u64,
    sample_rate_hz: usize,
    rx_read_completed_at: Instant,
    enqueue_time: Instant,
}

/// Handle to a running traffic RX thread.
struct TrafficRxThread {
    walsh_code: u8,
    tx: mpsc::Sender<TrafficRxBlock>,
    shutdown: Arc<AtomicBool>,
}

/// Handle to the HRPD reverse access worker.
struct HrpdAccessRxThread {
    tx: mpsc::Sender<HrpdAccessRxBlock>,
}

/// Handle to a running HRPD reverse traffic worker.
struct HrpdTrafficRxThread {
    uati: u32,
    mac_index: u8,
    tx: crossbeam_channel::Sender<HrpdTrafficRxBlock>,
}

const PCG_CHIPS: usize = 1536;
const HRPD_SLOT_CHIPS: usize = 2048;
const HRPD_TRAFFIC_MAX_INTERPOLATED_GAP_SLOTS: usize = 8;
#[allow(dead_code)]
const HRPD_TRAFFIC_FRAME_CHIPS: usize = HRPD_SLOT_CHIPS * 16;

/// `rx_sample_delay` is calibrated at the single-carrier 4× chip rate.
const RX_SAMPLE_DELAY_CALIBRATION_OVERSAMPLE: i64 = 4;

fn rx_target_batch_samples(sample_rate_hz: usize, chip_rate_hz: usize, batch_pcgs: usize) -> usize {
    let oversample = (sample_rate_hz / chip_rate_hz.max(1)).max(1);
    oversample.saturating_mul(PCG_CHIPS * batch_pcgs.max(1))
}

fn effective_rx_target_batch_samples(
    sample_rate_hz: usize,
    chip_rate_hz: usize,
    batch_pcgs: usize,
    hrpd_enabled: bool,
) -> usize {
    let configured = rx_target_batch_samples(sample_rate_hz, chip_rate_hz, batch_pcgs);
    if !hrpd_enabled {
        return configured;
    }

    let oversample = (sample_rate_hz / chip_rate_hz.max(1)).max(1);
    let hrpd_slot_samples = oversample.saturating_mul(HRPD_SLOT_CHIPS);
    configured.min(hrpd_slot_samples.max(1))
}

fn scaled_rx_sample_delay(rx_sample_delay: i64, rx_oversample: usize) -> i64 {
    rx_sample_delay * rx_oversample as i64 / RX_SAMPLE_DELAY_CALIBRATION_OVERSAMPLE
}

fn spawn_traffic_rx_thread(
    oversample: usize,
    walsh_code: u8,
    esn: u32,
    preamble_num_pcgs: Option<usize>,
    use_rc3: bool,
    rev_fch_gating_mode: bool,
    finger_pool_size: usize,
    traffic_rx_continuity: bool,
    event_tx: Option<tokio_mpsc::UnboundedSender<AccessChannelEvent>>,
    reverse_bearer_tx: Option<mpsc::Sender<UdpBearerDatagram>>,
    traffic_channels: Option<TrafficChannelPool>,
    power_control: Option<BtsPowerControlRegistry>,
    chip_rate_hz: usize,
    traffic_ack_seq_tx: Option<tokio_mpsc::Sender<(u8, u8)>>,
    bearer_bts_id: u32,
    bearer_cell_id: u32,
) -> TrafficRxThread {
    let settings = crate::receiver::pipelined::ReverseTrafficSettings {
        oversample,
        walsh_code,
        esn,
        reanchor_origin: true,
        snr_threshold: None,
        preamble_num_pcgs,
        epl_pilot: use_rc3,
        rev_fch_gating_mode,
        finger_pool_size,
    };
    let (iq_tx, iq_rx) = mpsc::channel::<TrafficRxBlock>();
    let thread_shutdown = Arc::new(AtomicBool::new(false));
    let thread_shutdown_clone = thread_shutdown.clone();
    let continuity_configured = traffic_rx_continuity;

    std::thread::Builder::new()
        .name(format!("traffic-rx-w{}", walsh_code))
        .spawn(move || {
            let processors = if use_rc3 {
                crate::receiver::pipelined::reverse_traffic_chain_rc3(settings)
            } else {
                crate::receiver::pipelined::reverse_traffic_chain(settings)
            };
            run_traffic_rx_thread(
                walsh_code,
                processors,
                iq_rx,
                event_tx,
                reverse_bearer_tx,
                traffic_channels,
                power_control,
                chip_rate_hz,
                continuity_configured,
                thread_shutdown_clone,
                traffic_ack_seq_tx,
                bearer_bts_id,
                bearer_cell_id,
            );
        })
        .expect("failed to spawn traffic RX thread");

    TrafficRxThread {
        walsh_code,
        tx: iq_tx,
        shutdown: thread_shutdown,
    }
}

fn spawn_hrpd_access_rx_thread(
    mut processors: Vec<PipelineProcessorShared>,
    mut stage_timings: Vec<StageTiming>,
    color_code: u8,
    sector_pilot_pn: u16,
    event_tx: Option<tokio_mpsc::UnboundedSender<HrpdAccessIndication>>,
    shutdown: Arc<AtomicBool>,
) -> HrpdAccessRxThread {
    // Access-burst timing must stay contiguous; dropping IQ in the middle of
    // a burst corrupts the preamble/capsule relationship.
    let (iq_tx, iq_rx) = mpsc::channel::<HrpdAccessRxBlock>();
    std::thread::Builder::new()
        .name("hrpd-access-rx".to_string())
        .spawn(move || {
            let mut blocks_processed = 0u64;
            let mut total_us = 0u64;
            let mut max_us = 0u64;
            while !shutdown.load(Ordering::Relaxed) {
                let blk = match iq_rx.recv_timeout(std::time::Duration::from_millis(500)) {
                    Ok(blk) => blk,
                    Err(mpsc::RecvTimeoutError::Timeout) => continue,
                    Err(mpsc::RecvTimeoutError::Disconnected) => break,
                };
                let queue_delay_us = blk.enqueue_time.elapsed().as_micros() as u64;
                let hrpd_block = blk.block;
                let mut block = SampleBlock::new(hrpd_block.samples, hrpd_block.relative_sample_start)
                    .with_sample_rate_hz(hrpd_block.sample_rate_hz as f64);
                block
                    .tags
                    .insert("absolute_chip_start", hrpd_block.absolute_chip_start as i64);
                block.tags.insert(
                    "absolute_sample_start",
                    hrpd_block.absolute_sample_start as i64,
                );

                let iter_start = Instant::now();
                let mut emitter = VecEmitter::new();
                let mut outputs =
                    run_sub_chain_timed(&mut processors, block, &mut stage_timings, &mut emitter);
                outputs.extend(emitter.blocks);
                let elapsed_us = iter_start.elapsed().as_micros() as u64;
                blocks_processed = blocks_processed.saturating_add(1);
                total_us = total_us.saturating_add(elapsed_us);
                max_us = max_us.max(elapsed_us);
                emit_hrpd_access_outputs(outputs, color_code, sector_pilot_pn, &event_tx);

                if elapsed_us > 100_000 {
                    warn!(
                        "rx_hrpd_access_worker_slow: block_us={} queue_us={} blocks={} avg_us={} max_us={}",
                        elapsed_us,
                        queue_delay_us,
                        blocks_processed,
                        total_us / blocks_processed.max(1),
                        max_us,
                    );
                }
            }

            let mut emitter = VecEmitter::new();
            let mut outputs = flush_sub_chain(&mut processors, &mut emitter);
            outputs.extend(emitter.blocks);
            emit_hrpd_access_outputs(outputs, color_code, sector_pilot_pn, &event_tx);
            info!("rx: HRPD reverse access worker stopped");
        })
        .expect("failed to spawn HRPD access RX thread");

    HrpdAccessRxThread { tx: iq_tx }
}

fn spawn_hrpd_traffic_rx_thread(
    oversample: usize,
    assignment: HrpdTrafficAssignmentRequest,
    event_tx: Option<tokio_mpsc::UnboundedSender<HrpdTrafficEvent>>,
    harq_bus: Option<Arc<crate::bts::hrpd::HarqBus>>,
    shutdown: Arc<AtomicBool>,
) -> HrpdTrafficRxThread {
    use crate::receiver::hrpd::reverse_traffic_rake::HrpdReverseTrafficCorrelator;
    use crate::receiver::pipelined::generic_rake_receiver::GenericRakeReceiver;
    use crate::receiver::pipelined::{RxSampleTimeAnchor, VecEmitter, run_sub_chain};

    // Reverse traffic is timeline-sensitive. Once the AT has a traffic
    // assignment, DRC/ACK/RRI/data are all decoded against continuous slot
    // timing, so dropping IQ to keep the mailbox fresh creates false chip
    // discontinuities. Keep the handoff lossless and report backlog through
    // queue_age_us diagnostics instead.
    let (iq_tx, iq_rx) = crossbeam_channel::unbounded::<HrpdTrafficRxBlock>();
    let uati = assignment.uati;
    let mac_index = assignment.mac_index;
    let reverse_pilot_acquired = Arc::new(AtomicBool::new(false));
    let correlator = HrpdReverseTrafficCorrelator::new(
        assignment.clone(),
        oversample,
        event_tx,
        harq_bus,
        reverse_pilot_acquired.clone(),
    );
    // Each HRPD traffic RX thread uses one finger until multipath combining is supported.
    let rake: PipelineProcessorShared = Box::new(
        GenericRakeReceiver::new(correlator)
            .with_prune_policy(Box::new(HrpdTrafficPrunePolicy))
            .with_max_fingers(1)
            .with_finger_pool_size(1),
    );
    let mut processors: Vec<PipelineProcessorShared> = vec![rake];

    std::thread::Builder::new()
        .name(format!("hrpd-traffic-rx-mac{}", mac_index))
        .spawn(move || {
            info!(
                "rx_hrpd_traffic[m{}]: rake worker started uati=0x{:08x} physical_subtype=0x{:04x} rtc_mac_subtype=0x{:04x} lcm_i=0x{:016x} lcm_q=0x{:016x} drc_cover={} drc_length={}",
                mac_index,
                uati,
                assignment.physical_layer_subtype,
                assignment.reverse_traffic_mac_subtype,
                assignment.reverse_long_code_mask_i,
                assignment.reverse_long_code_mask_q,
                assignment.drc_cover,
                assignment.drc_length,
            );
            let mut queue_age_samples = 0u64;
            let mut queue_age_total_us = 0u64;
            let mut queue_age_max_us = 0u64;
            let mut timing_samples = 0u64;
            let mut rake_total_us = 0u64;
            let mut rake_max_us = 0u64;
            let mut discontinuity_count = 0u64;
            let mut continuity_state = TrafficContinuityState::new(true);
            continuity_state.enabled = true;
            while !shutdown.load(Ordering::Relaxed) {
                // DRC only governs the next DRCLength slots, so avoid RX
                // batching that delays scheduler-facing DRC publication.
                let blk = match iq_rx.recv_timeout(Duration::from_millis(500)) {
                    Ok(blk) => blk,
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
                };
                let queue_us = blk.enqueue_time.elapsed().as_micros() as u64;
                queue_age_samples = queue_age_samples.saturating_add(1);
                queue_age_total_us = queue_age_total_us.saturating_add(queue_us);
                queue_age_max_us = queue_age_max_us.max(queue_us);
                let raw_absolute_sample_start = blk.absolute_sample_start;
                let raw_samples_len = blk.samples.len();
                let max_interpolated_gap_samples = oversample
                    .max(1)
                    .saturating_mul(HRPD_SLOT_CHIPS)
                    .saturating_mul(HRPD_TRAFFIC_MAX_INTERPOLATED_GAP_SLOTS);
                let expected_abs = continuity_state
                    .expected_absolute_sample_start
                    .unwrap_or(raw_absolute_sample_start);
                let delta_samples = raw_absolute_sample_start as i128 - expected_abs as i128;
                let gap_reset = continuity_state
                    .expected_absolute_sample_start
                    .is_some_and(|expected| {
                        raw_absolute_sample_start > expected
                            && (raw_absolute_sample_start - expected) as usize
                                > max_interpolated_gap_samples
                    });
                let continuity = reconcile_traffic_stream_continuity_with_max_insert(
                    blk.samples,
                    raw_absolute_sample_start,
                    continuity_state.expected_absolute_sample_start,
                    continuity_state.last_tail_sample,
                    Some(max_interpolated_gap_samples),
                );
                if gap_reset {
                    discontinuity_count = discontinuity_count.saturating_add(1);
                    let gap_samples = raw_absolute_sample_start.saturating_sub(expected_abs);
                    warn!(
                        "rx_hrpd_traffic[m{}]: local_rx_discontinuity reset raw_abs_start={} expected_abs_start={} delta_samples={} delta_chips={:.2} max_insert={} raw_samples={} output_samples={} queue_age_us={} queue_avg_us={} queue_max_us={} discontinuities={}",
                        mac_index,
                        raw_absolute_sample_start,
                        expected_abs,
                        delta_samples,
                        delta_samples as f64 / oversample.max(1) as f64,
                        max_interpolated_gap_samples,
                        raw_samples_len,
                        continuity.samples.len(),
                        queue_us,
                        queue_age_total_us / queue_age_samples.max(1),
                        queue_age_max_us,
                        discontinuity_count,
                    );
                    debug!(
                        "rx_hrpd_traffic[m{}]: skipped HRPD traffic gap samples={} chips={:.2}",
                        mac_index,
                        gap_samples,
                        gap_samples as f64 / oversample.max(1) as f64,
                    );
                } else if continuity.inserted_samples > 0 || continuity.dropped_samples > 0 {
                    discontinuity_count = discontinuity_count.saturating_add(1);
                    warn!(
                        "rx_hrpd_traffic[m{}]: local_rx_discontinuity corrected raw_abs_start={} expected_abs_start={} delta_samples={} delta_chips={:.2} inserted={} dropped={} raw_samples={} output_samples={} queue_age_us={} queue_avg_us={} queue_max_us={} discontinuities={}",
                        mac_index,
                        raw_absolute_sample_start,
                        expected_abs,
                        delta_samples,
                        delta_samples as f64 / oversample.max(1) as f64,
                        continuity.inserted_samples,
                        continuity.dropped_samples,
                        raw_samples_len,
                        continuity.samples.len(),
                        queue_us,
                        queue_age_total_us / queue_age_samples.max(1),
                        queue_age_max_us,
                        discontinuity_count,
                    );
                }
                let absolute_sample_start = continuity.absolute_sample_start;
                let absolute_chip_start = absolute_sample_start / oversample.max(1) as u64;
                let samples = continuity.samples;
                let block_samples = samples.len();
                let represented_us =
                    ((block_samples as f64 * 1_000_000.0) / blk.sample_rate_hz.max(1) as f64)
                        .round() as u64;
                if samples.is_empty() {
                    continue;
                }
                continuity_state.expected_absolute_sample_start =
                    Some(absolute_sample_start.saturating_add(block_samples as u64));
                continuity_state.last_tail_sample = samples.last().copied();
                // Build the SampleBlock with `chip_start` = absolute chip
                // index. The finger and correlator both compute their
                // absolute sample positions from the explicit tag below.
                let mut block = SampleBlock::new(samples, absolute_chip_start as usize)
                    .with_sample_rate_hz(blk.sample_rate_hz as f64);
                block
                    .tags
                    .insert("absolute_sample_start", absolute_sample_start as i64);
                block.rx_sample_time = Some(RxSampleTimeAnchor {
                    absolute_sample_end: absolute_sample_start.saturating_add(block_samples as u64),
                    received_at: blk.rx_read_completed_at,
                });
                let mut emitter = VecEmitter::new();
                // The rake's output blocks carry diagnostic tags but are
                // not consumed downstream — HrpdTrafficEvent goes out via
                // the data processor's tokio channel directly.
                let rake_start = Instant::now();
                let _outputs = run_sub_chain(&mut processors, block, &mut emitter);
                let rake_us = rake_start.elapsed().as_micros() as u64;
                timing_samples = timing_samples.saturating_add(1);
                rake_total_us = rake_total_us.saturating_add(rake_us);
                rake_max_us = rake_max_us.max(rake_us);
                let slow_rake = rake_us > represented_us.saturating_mul(2).max(10_000);
                if queue_us > 250_000 || slow_rake || timing_samples % 400 == 0 {
                    log::trace!(
                        "rx_hrpd_traffic[m{}]: worker_timing queue_age_us={} queue_avg={} queue_max={} coalesced={} samples={} iq_us={} rake_us={} rake_avg={} rake_max={} blocks={}",
                        mac_index,
                        queue_us,
                        queue_age_total_us / queue_age_samples.max(1),
                        queue_age_max_us,
                        1,
                        block_samples,
                        represented_us,
                        rake_us,
                        rake_total_us / timing_samples.max(1),
                        rake_max_us,
                        timing_samples,
                    );
                }
            }
            info!(
                "rx_hrpd_traffic[m{}]: rake worker stopped uati=0x{:08x}",
                mac_index, uati
            );
        })
        .expect("failed to spawn HRPD traffic RX thread");

    HrpdTrafficRxThread {
        uati,
        mac_index,
        tx: iq_tx,
    }
}

fn send_hrpd_traffic_rx_block(thread: &HrpdTrafficRxThread, block: HrpdTrafficRxBlock) -> bool {
    match thread.tx.send(block) {
        Ok(()) => true,
        Err(_) => {
            warn!(
                "rx: HRPD traffic RX thread exited uati=0x{:08x} mac={}",
                thread.uati, thread.mac_index
            );
            false
        }
    }
}

struct HrpdTrafficPrunePolicy;

/// Idle grace for a finger that has validated its reverse pilot (connection is
/// established). 3 s of silence before teardown, so a rough patch does not force
/// a re-acquisition and the closed-loop power-control gap that comes with it.
const HRPD_TRAFFIC_VALIDATED_IDLE_GRACE_CHIPS: u64 = 3 * HRPD_CHIP_RATE_HZ;
/// Idle grace before any validation: enough for the assignment/RTCAck window,
/// but short enough that a false FFT hit does not pin the single-finger worker.
const HRPD_TRAFFIC_UNVALIDATED_IDLE_GRACE_CHIPS: u64 = HRPD_CHIP_RATE_HZ;
const HRPD_CHIP_RATE_HZ: u64 = 1_228_800;

impl crate::receiver::pipelined::generic_rake_receiver::PrunePolicy for HrpdTrafficPrunePolicy {
    fn should_prune(
        &self,
        finger: &dyn crate::receiver::pipelined::generic_rake_receiver::RakeFinger,
    ) -> bool {
        // Reverse traffic is connection-scoped: once the pilot validates, the
        // finger stays up while coherent PHY frames keep arriving. The generic
        // access/1x policy is burst-scoped and can retire a healthy HRPD
        // traffic finger just because no data CRC or ACK event arrived. Use the
        // long grace as soon as the pilot soft-validates — not only after a
        // CRC-clean (hard) frame — because tearing down a tracking finger opens
        // a reverse-power-control gap.
        if finger.is_soft_validated() {
            return finger.idle_chips() > HRPD_TRAFFIC_VALIDATED_IDLE_GRACE_CHIPS;
        }
        finger.idle_chips() > HRPD_TRAFFIC_UNVALIDATED_IDLE_GRACE_CHIPS
    }
}

#[derive(Clone, Copy)]
#[allow(dead_code)]
struct HrpdTrafficPilotMetric {
    frame_start_chip: u64,
    chip_offset: i32,
    coherence: f32,
    snr_db: f32,
    sample_delay: i32,
    sample_delay_fraction: f32,
    pilot_phase: Complex32,
    i_mask: u64,
    q_mask: u64,
    q_sign: f32,
    q_pair_phase: u64,
    mask_label: &'static str,
}

#[derive(Debug, Clone, Copy)]
#[allow(dead_code)]
struct HrpdTrafficMaskCandidate {
    i_mask: u64,
    q_mask: u64,
    q_sign: f32,
    q_pair_phase: u64,
    label: &'static str,
}

#[allow(dead_code)]
fn hrpd_reverse_traffic_pilot_metric_at_offset(
    samples: &[Complex32],
    absolute_sample_start: u64,
    oversample: usize,
    nominal_frame_start_chip: u64,
    chip_offset: i32,
    mask: HrpdTrafficMaskCandidate,
    pilot_chip_step: usize,
) -> Option<HrpdTrafficPilotMetric> {
    let frame_start_chip = if chip_offset.is_negative() {
        nominal_frame_start_chip.checked_sub(chip_offset.unsigned_abs() as u64)?
    } else {
        nominal_frame_start_chip.checked_add(chip_offset as u64)?
    };
    let start_sample = frame_start_chip.checked_mul(oversample as u64)?;
    if start_sample < absolute_sample_start {
        return None;
    }
    let base_start = (start_sample - absolute_sample_start) as usize;
    let pn = hrpd_reverse_terminal_pn_signs(frame_start_chip, HRPD_TRAFFIC_FRAME_CHIPS);
    let lc_i =
        hrpd_long_code_signs_at_phase(mask.i_mask, frame_start_chip, HRPD_TRAFFIC_FRAME_CHIPS);
    let lc_q =
        hrpd_long_code_signs_at_phase(mask.q_mask, frame_start_chip, HRPD_TRAFFIC_FRAME_CHIPS);

    let mut best: Option<HrpdTrafficPilotMetric> = None;
    // Access acquisition on the same channelizer has landed near +52 samples
    // in live composite captures. Keep the traffic search bounded, but cover
    // that observed RX filter/group-delay range; chip-offset refinement still
    // accounts for the spec FrameOffset/slot boundary.
    for sample_delay in -32..=80 {
        for sample_delay_fraction in [0.0f32, -0.75, 0.75] {
            let mut coherent = Complex32::new(0.0, 0.0);
            let mut slot_coherent = [Complex32::new(0.0, 0.0); 16];
            let mut count = 0usize;
            for chip in (0..HRPD_TRAFFIC_FRAME_CHIPS).step_by(pilot_chip_step.max(1)) {
                // RRI replaces the pilot over the first 256 chips of each slot.
                // Skip that TDM region so the metric scores only unmodulated
                // Pilot Channel chips on W0^16.
                if chip % 2048 < 256 {
                    continue;
                }
                let sample = match sample_chip_at_delay(
                    samples,
                    base_start,
                    oversample,
                    chip,
                    sample_delay,
                    sample_delay_fraction,
                ) {
                    Some(sample) => sample,
                    None => continue,
                };
                let ref_chip = hrpd_reverse_traffic_pilot_reference(
                    frame_start_chip + chip as u64,
                    chip,
                    &pn,
                    &lc_i,
                    &lc_q,
                    mask,
                );
                let v = sample * ref_chip.conj();
                coherent += v;
                slot_coherent[(chip / HRPD_SLOT_CHIPS).min(15)] += v;
                count += 1;
            }
            if count == 0 {
                continue;
            }
            let slot_phase = slot_coherent.map(|sum| {
                if sum.norm_sqr() > 0.0 {
                    sum / sum.norm()
                } else {
                    Complex32::new(1.0, 0.0)
                }
            });
            let mut slot_projected = [0.0f32; 16];
            let mut abs_sum = 0.0f32;
            let mut power_sum = 0.0f32;
            let mut projected_count = 0usize;
            for chip in (0..HRPD_TRAFFIC_FRAME_CHIPS).step_by(pilot_chip_step.max(1)) {
                if chip % 2048 < 256 {
                    continue;
                }
                let sample = match sample_chip_at_delay(
                    samples,
                    base_start,
                    oversample,
                    chip,
                    sample_delay,
                    sample_delay_fraction,
                ) {
                    Some(sample) => sample,
                    None => continue,
                };
                let ref_chip = hrpd_reverse_traffic_pilot_reference(
                    frame_start_chip + chip as u64,
                    chip,
                    &pn,
                    &lc_i,
                    &lc_q,
                    mask,
                );
                let v = sample * ref_chip.conj();
                let slot = (chip / HRPD_SLOT_CHIPS).min(15);
                let projected = (v * slot_phase[slot].conj()).re;
                slot_projected[slot] += projected;
                abs_sum += projected.abs();
                power_sum += projected * projected;
                projected_count += 1;
            }
            if projected_count == 0
                || abs_sum.partial_cmp(&0.0) != Some(std::cmp::Ordering::Greater)
            {
                continue;
            }
            // Reverse traffic can carry residual AT/BTS CFO over the 26.7 ms
            // frame.  Score acquisition noncoherently per slot after PN/LC
            // despread; this preserves the spec pilot structure while avoiding
            // a false miss from phase rotation across the full frame.
            let noncoherent = slot_projected.iter().map(|sum| sum.abs()).sum::<f32>();
            let coherence = noncoherent / abs_sum;
            let mean_power = power_sum / projected_count as f32;
            let coherent_power =
                (noncoherent * noncoherent) / (projected_count * projected_count) as f32;
            let noise_power = (mean_power - coherent_power).max(1.0e-12);
            let snr_db = 10.0 * (coherent_power / noise_power).max(1.0e-12).log10();
            let metric = HrpdTrafficPilotMetric {
                frame_start_chip,
                chip_offset,
                coherence,
                snr_db,
                sample_delay,
                sample_delay_fraction,
                pilot_phase: if coherent.norm_sqr() > 0.0 {
                    coherent / coherent.norm()
                } else {
                    Complex32::new(1.0, 0.0)
                },
                i_mask: mask.i_mask,
                q_mask: mask.q_mask,
                q_sign: mask.q_sign,
                q_pair_phase: mask.q_pair_phase,
                mask_label: mask.label,
            };
            if best.as_ref().is_none_or(|best| {
                metric.coherence > best.coherence
                    || (metric.coherence == best.coherence && metric.snr_db > best.snr_db)
            }) {
                best = Some(metric);
            }
        }
    }
    best
}

#[allow(dead_code)]
fn sample_chip_at_delay(
    samples: &[Complex32],
    base_start: usize,
    oversample: usize,
    chip: usize,
    sample_delay: i32,
    sample_delay_fraction: f32,
) -> Option<Complex32> {
    let sample_pos = base_start as f32
        + chip as f32 * oversample.max(1) as f32
        + sample_delay as f32
        + sample_delay_fraction;
    if !sample_pos.is_finite() || sample_pos < 0.0 {
        return None;
    }
    let lo = sample_pos.floor() as usize;
    let frac = sample_pos - lo as f32;
    if lo + 1 >= samples.len() {
        return None;
    }
    Some(samples[lo] * (1.0 - frac) + samples[lo + 1] * frac)
}

#[allow(dead_code)]
fn hrpd_reverse_composite_reference(
    abs_chip: u64,
    chip: usize,
    pn: &[(f32, f32)],
    lc_i: &[f32],
    lc_q: &[f32],
    mask: HrpdTrafficMaskCandidate,
) -> Complex32 {
    let pair_chip = if (abs_chip & 1) == (mask.q_pair_phase & 1) {
        chip
    } else {
        chip.saturating_sub(1)
    };
    hrpd_reverse_pilot_reference_from_signs(
        abs_chip & 0x7fff,
        pn[chip].0,
        pn[pair_chip].1,
        lc_i[chip],
        lc_q[pair_chip],
        mask.q_sign,
        mask.q_pair_phase,
    )
}

#[allow(dead_code)]
fn hrpd_reverse_traffic_pilot_reference(
    abs_chip: u64,
    chip: usize,
    pn: &[(f32, f32)],
    lc_i: &[f32],
    lc_q: &[f32],
    mask: HrpdTrafficMaskCandidate,
) -> Complex32 {
    hrpd_reverse_composite_reference(abs_chip, chip, pn, lc_i, lc_q, mask)
}

#[allow(dead_code)]
fn hrpd_reverse_terminal_pn_signs(start_chip: u64, len: usize) -> Vec<(f32, f32)> {
    let mut pn = HrpdAccessTerminalPnSequence::new(0, 32768);
    pn.advance_chips(start_chip % 32768);
    (0..len)
        .map(|_| {
            let v = pn.generate_iq();
            (v.re, v.im)
        })
        .collect()
}

#[allow(dead_code)]
fn hrpd_long_code_signs_at_phase(mask: u64, start_chip: u64, len: usize) -> Vec<f32> {
    let mut lc = LongCodeGenerator::new(mask);
    lc.set_state(HRPD_LONG_CODE_INITIAL_STATE);
    let mut phase = (start_chip % 32768) as usize;
    lc.advance_chips(phase);
    let mut out = Vec::with_capacity(len);
    for idx in 0..len {
        if idx > 0 && phase == 0 {
            lc.set_state(HRPD_LONG_CODE_INITIAL_STATE);
        }
        out.push(if lc.next_chip() == 1 { -1.0 } else { 1.0 });
        phase = (phase + 1) & 0x7fff;
    }
    out
}

fn emit_hrpd_access_outputs(
    outputs: Vec<SampleBlock>,
    color_code: u8,
    sector_pilot_pn: u16,
    event_tx: &Option<tokio_mpsc::UnboundedSender<HrpdAccessIndication>>,
) {
    for blk in outputs {
        if let Some(indication) = build_hrpd_access_indication(&blk, color_code, sector_pilot_pn) {
            let message_summary = indication
                .messages
                .iter()
                .map(|message| format!("{message:?}"))
                .collect::<Vec<_>>()
                .join(", ");
            info!(
                "rx_hrpd_access_event: chip={} ati={:?} messages={} [{}]",
                indication.absolute_chip,
                indication.ati,
                indication.messages.len(),
                message_summary
            );
            if let Some(tx) = event_tx {
                let _ = tx.send(indication);
            }
        }
    }
}

#[derive(Clone, Debug)]
struct StageTiming {
    name: &'static str,
    total_us: u64,
    calls: u64,
    max_us: u64,
}

pub(super) struct RxRuntime {
    config: RxSettings,
    processors: Vec<PipelineProcessorShared>,
    one_x_rx_slice: Option<RxCarrierSlice>,
    hrpd_processors: Option<Vec<PipelineProcessorShared>>,
    hrpd_rx_slice: Option<RxCarrierSlice>,
    hrpd_access_thread: Option<HrpdAccessRxThread>,
    hrpd_traffic_threads: Vec<HrpdTrafficRxThread>,
    pipeline_oversample: usize,
    capture_writer: Option<WavWriter<BufWriter<std::fs::File>>>,
    capture_target_samples: Option<usize>,
    captured_samples: usize,
    last_capture_status: Option<IqCaptureStatus>,
    pending_capture_start: Option<PendingCaptureStart>,
    active_capture: Option<ActiveCapture>,
    next_sample_index: usize,
    absolute_sample_origin: u64,
    hardware_start_time_ns: u64,
    last_hardware_time_ns: u64,
    last_absolute_sample_start: u64,
    last_absolute_chip_start: u64,
    timing_interval_start: Instant,
    timing_reads: usize,
    timing_samples: usize,
    timing_read_us: u64,
    timing_copy_us: u64,
    timing_capture_us: u64,
    timing_pipeline_us: u64,
    timing_total_us: u64,
    timing_total_max_us: u64,
    stage_timings: Vec<StageTiming>,
    hrpd_stage_timings: Vec<StageTiming>,
    last_pipeline_lag_warn: Option<Instant>,
    last_pipeline_lag_warn_deficit_ms: u64,
    /// Deferred capture stop: the StopCapture command sets this so the
    /// capture continues until the next RX buffer is written, preventing
    /// truncation of samples already buffered in the reader channel.
    pending_capture_stop: Option<oneshot::Sender<Result<IqCaptureControlResult, String>>>,
    /// Continuity tracking for the capture stream. The receiver workers
    /// reconcile hardware stream gaps (USB overruns) by inserting or
    /// dropping samples against the hardware timestamps; the capture must
    /// apply the same correction or every sample after a gap sits at the
    /// wrong offset in the WAV and a replay decodes nothing there.
    capture_expected_abs_sample: Option<u64>,
    capture_last_tail: Option<Complex32>,
}

#[derive(Debug, Clone)]
struct CarrierSliceBlock {
    samples: Vec<Complex32>,
    relative_sample_start: usize,
    absolute_sample_start: u64,
    absolute_chip_start: u64,
    sample_rate_hz: usize,
}

/// Extracts one carrier from the composite reverse stream: mixes it to
/// baseband, anti-alias filters, and decimates to 4x chip rate. Mixing,
/// filtering, and decimation run in a single fused pass over the input — the
/// filter computes a tap dot-product only at the kept (decimated) output
/// positions, and the mixer is a phasor recurrence ([`PhasorNco`]) rather than
/// a per-sample sine/cosine.
struct RxCarrierSlice {
    nco: PhasorNco,
    decimation: usize,
    output_sample_rate_hz: usize,
    output_oversample: usize,
    /// Anti-alias filter applied before decimation. `None` when decimation <= 1
    /// (the slice is then a pure mixer pass-through).
    anti_alias: Option<SymmetricComplexFir32>,
}

impl RxCarrierSlice {
    fn new(
        carrier_shift_hz: i64,
        sample_rate_hz: usize,
        chip_rate_hz: usize,
    ) -> Result<Self, Error> {
        let input_oversample = sample_rate_hz / chip_rate_hz.max(1);
        if sample_rate_hz != input_oversample.saturating_mul(chip_rate_hz) {
            return Err(format!(
                "rx: sample_rate_hz={} must be an integer multiple of chip_rate_hz={}",
                sample_rate_hz, chip_rate_hz
            )
            .into());
        }
        if input_oversample < 4 || input_oversample % 4 != 0 {
            return Err(format!(
                "rx: sample_rate_hz={} gives oversample={}x; carrier slicing expects a multiple of 4x chip rate",
                sample_rate_hz, input_oversample
            )
            .into());
        }
        let decimation = (input_oversample / 4).max(1);
        let output_sample_rate_hz = sample_rate_hz / decimation;
        let output_oversample = output_sample_rate_hz / chip_rate_hz.max(1);
        // Mix down to baseband: negate the carrier shift.
        let phase_step_rad = if carrier_shift_hz == 0 || sample_rate_hz == 0 {
            0.0
        } else {
            -2.0 * PI * carrier_shift_hz as f64 / sample_rate_hz as f64
        };
        let anti_alias = (decimation > 1).then(|| {
            SymmetricComplexFir32::new(&carrier_slice_anti_alias_taps(
                decimation,
                sample_rate_hz,
                chip_rate_hz,
            ))
        });
        Ok(Self {
            nco: PhasorNco::new(phase_step_rad),
            decimation,
            output_sample_rate_hz,
            output_oversample,
            anti_alias,
        })
    }

    fn process(
        &mut self,
        mut samples: Vec<Complex32>,
        raw_relative_sample_start: usize,
        raw_absolute_sample_start: u64,
    ) -> CarrierSliceBlock {
        self.process_in_place(
            &mut samples,
            raw_relative_sample_start,
            raw_absolute_sample_start,
        )
    }

    fn process_in_place(
        &mut self,
        samples: &mut [Complex32],
        raw_relative_sample_start: usize,
        raw_absolute_sample_start: u64,
    ) -> CarrierSliceBlock {
        let oversample = self.output_oversample.max(1) as u64;
        let output_sample_rate_hz = self.output_sample_rate_hz;

        // Pass-through path: no decimation, so no anti-alias filter. Apply only
        // the NCO mix (a no-op for a zero shift).
        let Some(fir) = self.anti_alias.as_mut() else {
            self.nco.rotate_in_place(samples);
            return CarrierSliceBlock {
                samples: samples.to_vec(),
                relative_sample_start: raw_relative_sample_start,
                absolute_sample_start: raw_absolute_sample_start,
                absolute_chip_start: raw_absolute_sample_start / oversample,
                sample_rate_hz: output_sample_rate_hz,
            };
        };

        // Fused mix + anti-alias filter + decimate. A kept output is emitted at
        // absolute sample positions that are multiples of the decimation
        // factor; only those positions pay for the tap dot-product.
        let nco = &mut self.nco;
        let d = self.decimation as u64;
        let first_idx = ((d - raw_absolute_sample_start % d) % d) as usize;
        let mut out =
            Vec::with_capacity(samples.len().saturating_sub(first_idx).div_ceil(d as usize));
        for (m, sample) in samples.iter().copied().enumerate() {
            let mixed = nco.mix(sample);
            let emit = (raw_absolute_sample_start + m as u64) % d == 0;
            if let Some(filtered) = fir.process_sample_if(mixed, emit) {
                out.push(filtered);
            }
        }

        let absolute_sample_start = (raw_absolute_sample_start + first_idx as u64) / d;
        let relative_sample_start = (raw_relative_sample_start + first_idx) / self.decimation;
        CarrierSliceBlock {
            samples: out,
            relative_sample_start,
            absolute_sample_start,
            absolute_chip_start: absolute_sample_start / oversample,
            sample_rate_hz: output_sample_rate_hz,
        }
    }

    fn process_pair(
        one_x: &mut Self,
        hrpd: &mut Self,
        samples: Vec<Complex32>,
        raw_relative_sample_start: usize,
        raw_absolute_sample_start: u64,
    ) -> (CarrierSliceBlock, CarrierSliceBlock) {
        if one_x.decimation != hrpd.decimation
            || one_x.output_oversample != hrpd.output_oversample
            || one_x.output_sample_rate_hz != hrpd.output_sample_rate_hz
        {
            let hrpd_block = hrpd.process(
                samples.clone(),
                raw_relative_sample_start,
                raw_absolute_sample_start,
            );
            let one_x_block = one_x.process(
                samples,
                raw_relative_sample_start,
                raw_absolute_sample_start,
            );
            return (one_x_block, hrpd_block);
        }

        let Some(one_x_fir) = one_x.anti_alias.as_mut() else {
            let mut one_x_samples = samples.clone();
            one_x.nco.rotate_in_place(&mut one_x_samples);
            let hrpd_block = hrpd.process(
                samples,
                raw_relative_sample_start,
                raw_absolute_sample_start,
            );
            let oversample = one_x.output_oversample.max(1) as u64;
            let one_x_block = CarrierSliceBlock {
                samples: one_x_samples,
                relative_sample_start: raw_relative_sample_start,
                absolute_sample_start: raw_absolute_sample_start,
                absolute_chip_start: raw_absolute_sample_start / oversample,
                sample_rate_hz: one_x.output_sample_rate_hz,
            };
            return (one_x_block, hrpd_block);
        };
        let Some(hrpd_fir) = hrpd.anti_alias.as_mut() else {
            let mut hrpd_samples = samples.clone();
            hrpd.nco.rotate_in_place(&mut hrpd_samples);
            let one_x_block = one_x.process(
                samples,
                raw_relative_sample_start,
                raw_absolute_sample_start,
            );
            let oversample = hrpd.output_oversample.max(1) as u64;
            let hrpd_block = CarrierSliceBlock {
                samples: hrpd_samples,
                relative_sample_start: raw_relative_sample_start,
                absolute_sample_start: raw_absolute_sample_start,
                absolute_chip_start: raw_absolute_sample_start / oversample,
                sample_rate_hz: hrpd.output_sample_rate_hz,
            };
            return (one_x_block, hrpd_block);
        };

        let d = one_x.decimation as u64;
        let first_idx = ((d - raw_absolute_sample_start % d) % d) as usize;
        let out_capacity = samples.len().saturating_sub(first_idx).div_ceil(d as usize);
        let mut one_x_out = Vec::with_capacity(out_capacity);
        let mut hrpd_out = Vec::with_capacity(out_capacity);
        for (m, sample) in samples.into_iter().enumerate() {
            let emit = (raw_absolute_sample_start + m as u64) % d == 0;
            let one_x_mixed = one_x.nco.mix(sample);
            if let Some(filtered) = one_x_fir.process_sample_if(one_x_mixed, emit) {
                one_x_out.push(filtered);
            }
            let hrpd_mixed = hrpd.nco.mix(sample);
            if let Some(filtered) = hrpd_fir.process_sample_if(hrpd_mixed, emit) {
                hrpd_out.push(filtered);
            }
        }

        let absolute_sample_start = (raw_absolute_sample_start + first_idx as u64) / d;
        let relative_sample_start = (raw_relative_sample_start + first_idx) / one_x.decimation;
        let absolute_chip_start = absolute_sample_start / one_x.output_oversample.max(1) as u64;
        (
            CarrierSliceBlock {
                samples: one_x_out,
                relative_sample_start,
                absolute_sample_start,
                absolute_chip_start,
                sample_rate_hz: one_x.output_sample_rate_hz,
            },
            CarrierSliceBlock {
                samples: hrpd_out,
                relative_sample_start,
                absolute_sample_start,
                absolute_chip_start,
                sample_rate_hz: hrpd.output_sample_rate_hz,
            },
        )
    }
}

fn carrier_slice_anti_alias_taps(
    decimation: usize,
    sample_rate_hz: usize,
    chip_rate_hz: usize,
) -> Vec<f64> {
    let taps = 63usize;
    let center = (taps - 1) as f64 / 2.0;
    let nyquist = sample_rate_hz as f64 / 2.0;
    let alias_cutoff = sample_rate_hz as f64 / (2.0 * decimation as f64) * 0.82;
    let occupied_cutoff = chip_rate_hz as f64 * 1.25;
    let cutoff_hz = alias_cutoff.min(occupied_cutoff).min(nyquist * 0.95);
    let fc = cutoff_hz / sample_rate_hz as f64;
    let mut out = Vec::with_capacity(taps);
    for n in 0..taps {
        let x = n as f64 - center;
        let sinc = if x.abs() < f64::EPSILON {
            2.0 * fc
        } else {
            (2.0 * PI * fc * x).sin() / (PI * x)
        };
        let window = 0.42 - 0.5 * (2.0 * PI * n as f64 / (taps - 1) as f64).cos()
            + 0.08 * (4.0 * PI * n as f64 / (taps - 1) as f64).cos();
        out.push(sinc * window);
    }
    let gain: f64 = out.iter().sum();
    if gain.abs() > f64::EPSILON {
        for tap in &mut out {
            *tap /= gain;
        }
    }
    out
}

use rx_capture::{ActiveCapture, PendingCaptureStart};

#[derive(Debug)]
struct TrafficContinuityBlock {
    samples: Vec<Complex32>,
    absolute_sample_start: u64,
    inserted_samples: usize,
    dropped_samples: usize,
}

#[derive(Debug, Default)]
struct TrafficContinuityState {
    configured: bool,
    enabled: bool,
    expected_absolute_sample_start: Option<u64>,
    last_tail_sample: Option<Complex32>,
    next_relative_sample_start: Option<usize>,
}

impl TrafficContinuityState {
    fn new(configured: bool) -> Self {
        Self {
            configured,
            ..Self::default()
        }
    }
}

fn reconcile_traffic_stream_continuity(
    raw_samples: Vec<Complex32>,
    raw_absolute_sample_start: u64,
    expected_absolute_sample_start: Option<u64>,
    previous_tail_sample: Option<Complex32>,
) -> TrafficContinuityBlock {
    reconcile_traffic_stream_continuity_with_max_insert(
        raw_samples,
        raw_absolute_sample_start,
        expected_absolute_sample_start,
        previous_tail_sample,
        None,
    )
}

fn reconcile_traffic_stream_continuity_with_max_insert(
    raw_samples: Vec<Complex32>,
    raw_absolute_sample_start: u64,
    expected_absolute_sample_start: Option<u64>,
    previous_tail_sample: Option<Complex32>,
    max_insert_samples: Option<usize>,
) -> TrafficContinuityBlock {
    let Some(expected_start) = expected_absolute_sample_start else {
        return TrafficContinuityBlock {
            samples: raw_samples,
            absolute_sample_start: raw_absolute_sample_start,
            inserted_samples: 0,
            dropped_samples: 0,
        };
    };

    if raw_absolute_sample_start > expected_start {
        let gap = (raw_absolute_sample_start - expected_start) as usize;
        if let Some(max_insert) = max_insert_samples {
            if gap > max_insert {
                return TrafficContinuityBlock {
                    samples: raw_samples,
                    absolute_sample_start: raw_absolute_sample_start,
                    inserted_samples: 0,
                    dropped_samples: 0,
                };
            }
        }
        let Some(previous_tail) = previous_tail_sample else {
            return TrafficContinuityBlock {
                samples: raw_samples,
                absolute_sample_start: raw_absolute_sample_start,
                inserted_samples: 0,
                dropped_samples: 0,
            };
        };

        let mut samples = Vec::with_capacity(gap.saturating_add(raw_samples.len()));
        let target = raw_samples.first().copied().unwrap_or(previous_tail);
        let denom = (gap + 1) as f32;
        for idx in 1..=gap {
            let t = idx as f32 / denom;
            samples.push(Complex32::new(
                previous_tail.re + (target.re - previous_tail.re) * t,
                previous_tail.im + (target.im - previous_tail.im) * t,
            ));
        }
        samples.extend(raw_samples);
        return TrafficContinuityBlock {
            samples,
            absolute_sample_start: expected_start,
            inserted_samples: gap,
            dropped_samples: 0,
        };
    }

    if raw_absolute_sample_start < expected_start {
        let overlap = (expected_start - raw_absolute_sample_start) as usize;
        let dropped_samples = overlap.min(raw_samples.len());
        let samples = if overlap >= raw_samples.len() {
            Vec::new()
        } else {
            raw_samples.into_iter().skip(overlap).collect()
        };
        return TrafficContinuityBlock {
            samples,
            absolute_sample_start: expected_start,
            inserted_samples: 0,
            dropped_samples,
        };
    }

    TrafficContinuityBlock {
        samples: raw_samples,
        absolute_sample_start: raw_absolute_sample_start,
        inserted_samples: 0,
        dropped_samples: 0,
    }
}

#[derive(Serialize)]
struct CaptureMetadataFile {
    wav_path: String,
    sample_rate_hz: usize,
    chip_rate_hz: usize,
    rx_center_frequency_hz: Option<usize>,
    one_x_reverse_frequency_hz: Option<usize>,
    one_x_rx_shift_hz: i64,
    hrpd_reverse_frequency_hz: Option<usize>,
    hrpd_rx_shift_hz: Option<i64>,
    first_absolute_chip_start: u64,
    first_absolute_sample_start: u64,
    first_sample_system_time_rfc3339: String,
    first_hardware_time_ns: u64,
    captured_samples: u64,
    captured_seconds: f64,
}

pub(super) fn open_rx_runtime(rx: RxSettings) -> Result<RxRuntime, Error> {
    if rx.capture_iq_wav.is_some() || rx.capture_seconds.is_some() {
        info!("rx: startup IQ capture disabled; use gRPC start/stop capture controls");
    }
    info!(
        "rx: sample_rate_hz={} chip_rate_hz={} center_freq_hz={:?} 1x_reverse_hz={:?} 1x_shift_hz={:+} hrpd_reverse_hz={:?} hrpd_shift_hz={:?} capture=<ui-driven> hw_start_ns={:?} absolute_chip_start={:?} rx_sample_delay={}",
        rx.sample_rate_hz,
        rx.chip_rate_hz,
        rx.rx_center_frequency_hz,
        rx.one_x_reverse_frequency_hz,
        rx.one_x_rx_shift_hz,
        rx.hrpd_reverse_frequency_hz,
        rx.hrpd_rx_shift_hz,
        rx.hardware_start_time_ns,
        rx.absolute_chip_start,
        rx.rx_sample_delay,
    );

    let hardware_start_time_ns = rx.hardware_start_time_ns;
    info!(
        "rx: hardware_start_time_ns={} absolute_chip_start={}",
        hardware_start_time_ns, rx.absolute_chip_start
    );
    let oversample = rx.sample_rate_hz / rx.chip_rate_hz;
    let one_x_rx_slice = rx
        .one_x_enabled
        .then(|| RxCarrierSlice::new(rx.one_x_rx_shift_hz, rx.sample_rate_hz, rx.chip_rate_hz))
        .transpose()?;
    let absolute_chip_origin = rx.absolute_chip_start;
    let absolute_sample_origin = absolute_chip_origin.saturating_mul(oversample as u64);
    let capture_writer = None;
    let capture_target_samples = None;
    let (processors, stage_timings) = if let Some(slice) = one_x_rx_slice.as_ref() {
        let processors = reverse_access_chain(ReverseAccessSettings {
            oversample: slice.output_oversample,
            access_channel_number: rx.access_channel_number,
            paging_channel_number: rx.paging_channel_number,
            base_id: rx.base_id,
            pilot_pn: rx.pilot_pn,
            long_code_state: 1u64 << 41,
            rake_fast_path: false,
            fixed_finger_phase: None,
            reanchor_origin: rx.reanchor_origin,
            finger_pool_size: rx.reverse_access_finger_pool_size,
        });
        let timings = processors
            .iter()
            .map(|p| StageTiming {
                name: p.name(),
                total_us: 0,
                calls: 0,
                max_us: 0,
            })
            .collect();
        (processors, timings)
    } else {
        info!("rx: 1x carrier slice, access correlator, and traffic chains disabled");
        (Vec::new(), Vec::new())
    };
    let (hrpd_rx_slice, hrpd_processors, hrpd_stage_timings) = if let (
        Some(_hrpd_reverse_hz),
        Some(hrpd_shift_hz),
    ) =
        (rx.hrpd_reverse_frequency_hz, rx.hrpd_rx_shift_hz)
    {
        let slice = RxCarrierSlice::new(hrpd_shift_hz, rx.sample_rate_hz, rx.chip_rate_hz)?;
        let processors = hrpd_reverse_access_chain(HrpdReverseAccessSettings {
            oversample: slice.output_oversample,
            access_cycle_number: rx.hrpd_access_cycle_number,
            sector_id_lsb: rx.hrpd_access_sector_id_lsb,
            color_code: rx.hrpd_access_color_code,
            reanchor_origin: rx.reanchor_origin,
            snr_threshold: None,
            finger_pool_size: rx.reverse_access_finger_pool_size,
            preamble_frames: rx.hrpd_access_preamble_frames,
            enhanced_access_rates: rx.hrpd_access_enhanced_rates,
        });
        let timings = processors
            .iter()
            .map(|p| StageTiming {
                name: p.name(),
                total_us: 0,
                calls: 0,
                max_us: 0,
            })
            .collect();
        info!(
            "rx: HRPD reverse access FFT frame correlator pipeline enabled shift_hz={:+} oversample={} sector_id_lsb=0x{:06x} color_code={} access_cycle={}",
            hrpd_shift_hz,
            slice.output_oversample,
            rx.hrpd_access_sector_id_lsb & 0x00ff_ffff,
            rx.hrpd_access_color_code,
            rx.hrpd_access_cycle_number
        );
        (Some(slice), Some(processors), timings)
    } else {
        (None, None, Vec::new())
    };
    let active_rx_slice = one_x_rx_slice
        .as_ref()
        .or(hrpd_rx_slice.as_ref())
        .ok_or_else(|| Error::from("rx: no reverse carrier slice configured"))?;
    let pipeline_sample_rate_hz = active_rx_slice.output_sample_rate_hz;
    let pipeline_oversample = active_rx_slice.output_oversample;
    info!(
        "rx: stream already active from prime (raw_oversample={} pipeline_oversample={} slice_decimation={} pipeline_sample_rate_hz={} absolute_chip_origin={} absolute_sample_origin={})",
        oversample,
        pipeline_oversample,
        active_rx_slice.decimation,
        pipeline_sample_rate_hz,
        absolute_chip_origin,
        absolute_sample_origin
    );

    Ok(RxRuntime {
        config: rx,
        stage_timings,
        processors,
        one_x_rx_slice,
        hrpd_processors,
        hrpd_rx_slice,
        hrpd_access_thread: None,
        hrpd_traffic_threads: Vec::new(),
        pipeline_oversample,
        capture_writer,
        capture_target_samples,
        captured_samples: 0,
        last_capture_status: None,
        pending_capture_start: None,
        active_capture: None,
        next_sample_index: 0,
        absolute_sample_origin,
        hardware_start_time_ns,
        last_hardware_time_ns: hardware_start_time_ns,
        last_absolute_sample_start: absolute_sample_origin,
        last_absolute_chip_start: absolute_chip_origin,
        timing_interval_start: Instant::now(),
        timing_reads: 0,
        timing_samples: 0,
        timing_read_us: 0,
        timing_copy_us: 0,
        timing_capture_us: 0,
        timing_pipeline_us: 0,
        timing_total_us: 0,
        timing_total_max_us: 0,
        hrpd_stage_timings,
        last_pipeline_lag_warn: None,
        last_pipeline_lag_warn_deficit_ms: 0,
        pending_capture_stop: None,
        capture_expected_abs_sample: None,
        capture_last_tail: None,
    })
}

fn start_hrpd_access_worker(runtime: &mut RxRuntime, shutdown: Arc<AtomicBool>) {
    let Some(processors) = runtime.hrpd_processors.take() else {
        return;
    };
    let stage_timings = std::mem::take(&mut runtime.hrpd_stage_timings);
    runtime.hrpd_access_thread = Some(spawn_hrpd_access_rx_thread(
        processors,
        stage_timings,
        runtime.config.hrpd_access_color_code,
        runtime.config.pilot_pn,
        runtime.config.hrpd_access_event_tx.clone(),
        shutdown,
    ));
}

/// Message sent from the reader thread to the processing thread.
struct RxReaderMessage {
    samples: Vec<Complex32>,
    time_ns: u64,
    n: usize,
    enqueue_time: Instant,
    absolute_sample_start_override: Option<u64>,
}

fn capture_status_from_active(
    runtime: &RxRuntime,
    active: &ActiveCapture,
    active_flag: bool,
) -> IqCaptureStatus {
    IqCaptureStatus {
        active: active_flag,
        directory: active.directory.clone(),
        wav_path: Some(active.wav_path.clone()),
        metadata_path: Some(active.metadata_path.clone()),
        first_absolute_chip_start: Some(active.first_absolute_chip_start),
        first_absolute_sample_start: Some(active.first_absolute_sample_start),
        first_sample_system_time: Some(active.first_sample_system_time.clone()),
        first_hardware_time_ns: Some(active.first_hardware_time_ns),
        captured_samples: runtime.captured_samples as u64,
        sample_rate_hz: runtime.config.sample_rate_hz,
        chip_rate_hz: runtime.config.chip_rate_hz,
    }
}

fn write_capture_metadata(runtime: &RxRuntime, active: &ActiveCapture) -> Result<(), Error> {
    let metadata = CaptureMetadataFile {
        wav_path: active.wav_path.display().to_string(),
        sample_rate_hz: runtime.config.sample_rate_hz,
        chip_rate_hz: runtime.config.chip_rate_hz,
        rx_center_frequency_hz: runtime.config.rx_center_frequency_hz,
        one_x_reverse_frequency_hz: runtime.config.one_x_reverse_frequency_hz,
        one_x_rx_shift_hz: runtime.config.one_x_rx_shift_hz,
        hrpd_reverse_frequency_hz: runtime.config.hrpd_reverse_frequency_hz,
        hrpd_rx_shift_hz: runtime.config.hrpd_rx_shift_hz,
        first_absolute_chip_start: active.first_absolute_chip_start,
        first_absolute_sample_start: active.first_absolute_sample_start,
        first_sample_system_time_rfc3339: active.first_sample_system_time.to_rfc3339(),
        first_hardware_time_ns: active.first_hardware_time_ns,
        captured_samples: runtime.captured_samples as u64,
        captured_seconds: runtime.captured_samples as f64
            / runtime.config.sample_rate_hz.max(1) as f64,
    };
    fs::write(&active.metadata_path, serde_json::to_vec_pretty(&metadata)?)?;
    Ok(())
}

fn respond_pending_capture_start(
    runtime: &RxRuntime,
    active: &ActiveCapture,
    pending: PendingCaptureStart,
) {
    let _ = pending.respond_to.send(Ok(IqCaptureControlResult {
        status: capture_status_from_active(runtime, active, true),
        message: format!("IQ capture started: {}", active.wav_path.display()),
    }));
}

fn idle_capture_status(runtime: &RxRuntime, directory: PathBuf) -> IqCaptureStatus {
    runtime
        .last_capture_status
        .clone()
        .unwrap_or(IqCaptureStatus {
            active: false,
            directory,
            wav_path: None,
            metadata_path: None,
            first_absolute_chip_start: None,
            first_absolute_sample_start: None,
            first_sample_system_time: None,
            first_hardware_time_ns: None,
            captured_samples: 0,
            sample_rate_hz: runtime.config.sample_rate_hz,
            chip_rate_hz: runtime.config.chip_rate_hz,
        })
}

fn cancel_pending_capture_start(runtime: &mut RxRuntime, reason: &str) {
    if let Some(pending) = runtime.pending_capture_start.take() {
        let _ = pending.respond_to.send(Err(reason.to_string()));
    }
}

fn stop_active_capture(
    runtime: &mut RxRuntime,
    reason: &str,
) -> Result<Option<IqCaptureControlResult>, Error> {
    let Some(active) = runtime.active_capture.take() else {
        if runtime.capture_writer.take().is_some() {
            warn!("rx: capture writer existed without active metadata");
        }
        return Ok(None);
    };

    if let Some(mut wav) = runtime.capture_writer.take() {
        wav.flush()?;
        wav.finalize()?;
    }
    write_capture_metadata(runtime, &active)?;
    let status = capture_status_from_active(runtime, &active, false);
    runtime.last_capture_status = Some(status.clone());
    info!(
        "rx: capture stopped reason=\"{}\" path={} samples={} ({:.3}s)",
        reason,
        active.wav_path.display(),
        runtime.captured_samples,
        runtime.captured_samples as f64 / runtime.config.sample_rate_hz.max(1) as f64
    );
    Ok(Some(IqCaptureControlResult {
        status,
        message: reason.to_string(),
    }))
}

fn handle_bts_command(
    runtime: &mut RxRuntime,
    command: BtsCommand,
    shutdown: &AtomicBool,
) -> Result<(), Error> {
    match command {
        BtsCommand::GetCaptureStatus {
            directory,
            respond_to,
        } => {
            let status = if let Some(active) = runtime.active_capture.as_ref() {
                capture_status_from_active(runtime, active, true)
            } else {
                idle_capture_status(runtime, directory)
            };
            let message = if status.active {
                format!(
                    "IQ capture active: {}",
                    status
                        .wav_path
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_else(|| "<pending>".to_string())
                )
            } else if let Some(path) = status.wav_path.as_ref() {
                format!("IQ capture idle; last file: {}", path.display())
            } else {
                "IQ capture idle".to_string()
            };
            let _ = respond_to.send(Ok(IqCaptureControlResult { status, message }));
        }
        BtsCommand::StartCapture {
            directory,
            respond_to,
        } => {
            if runtime.pending_capture_start.is_some() || runtime.active_capture.is_some() {
                let _ = respond_to.send(Err("IQ capture is already active".to_string()));
            } else {
                info!(
                    "rx: arming IQ capture in {} (waiting for next RX buffer)",
                    directory.display()
                );
                runtime.captured_samples = 0;
                runtime.capture_writer = None;
                runtime.pending_capture_start = Some(PendingCaptureStart {
                    directory,
                    respond_to,
                });
            }
        }
        BtsCommand::StopCapture { respond_to } => {
            if runtime.active_capture.is_some() {
                // Defer the actual stop until after the next RX buffer is
                // written. This ensures all samples already buffered in
                // the reader channel make it into the WAV file.
                runtime.pending_capture_stop = Some(respond_to);
            } else if runtime.pending_capture_start.is_some() {
                cancel_pending_capture_start(runtime, "IQ capture canceled before first RX buffer");
                let _ =
                    respond_to.send(Err("IQ capture was pending but never started".to_string()));
            } else {
                let _ = respond_to.send(Err("no active IQ capture".to_string()));
            }
        }
        BtsCommand::Shutdown => {
            shutdown.store(true, Ordering::Relaxed);
        }
    }
    Ok(())
}

fn drain_bts_commands(
    runtime: &mut RxRuntime,
    commands_rx: &mut tokio_mpsc::Receiver<BtsCommand>,
    shutdown: &AtomicBool,
) -> Result<(), Error> {
    loop {
        match commands_rx.try_recv() {
            Ok(command) => handle_bts_command(runtime, command, shutdown)?,
            Err(tokio_mpsc::error::TryRecvError::Empty) => break,
            Err(tokio_mpsc::error::TryRecvError::Disconnected) => break,
        }
    }
    Ok(())
}

pub(super) fn run_rx_loop(
    rx: RxSettings,
    mut commands_rx: tokio_mpsc::Receiver<BtsCommand>,
    radio_rx: &mut dyn crate::sdr::RadioRx,
    shutdown: Arc<AtomicBool>,
) -> Result<(), Error> {
    let mut runtime = open_rx_runtime(rx)?;
    start_hrpd_access_worker(&mut runtime, shutdown.clone());
    let bearer_bts_id = runtime.config.base_id as u32;
    let bearer_cell_id: u32 = 1;
    let configured_batch_samples = rx_target_batch_samples(
        runtime.config.sample_rate_hz,
        runtime.config.chip_rate_hz,
        runtime.config.rx_batch_pcgs,
    );
    let hrpd_enabled = runtime.hrpd_rx_slice.is_some();
    let target_batch_samples = effective_rx_target_batch_samples(
        runtime.config.sample_rate_hz,
        runtime.config.chip_rate_hz,
        runtime.config.rx_batch_pcgs,
        hrpd_enabled,
    );
    let target_batch_ms =
        target_batch_samples as f64 * 1000.0 / runtime.config.sample_rate_hz.max(1) as f64;
    info!(
        "rx: live SDR target_batch_samples={} target_batch_ms={:.3} configured_batch_samples={} hrpd_enabled={}",
        target_batch_samples, target_batch_ms, configured_batch_samples, hrpd_enabled
    );

    // Drain samples while waiting for TX to publish its timing anchor.
    // This prevents RX buffer overflow during TX startup.
    if let Some(ref anchor) = runtime.config.tx_rx_anchor {
        info!("rx: waiting for TX timing anchor...");
        let mut drain_buf = vec![Complex32::new(0.0, 0.0); target_batch_samples];
        let mut drained = 0usize;
        loop {
            let _ = radio_rx.rx_read(&mut drain_buf, 250_000);
            drained += 1;
            if let Some((tick, chip)) = anchor.try_load() {
                let oversample =
                    (runtime.config.sample_rate_hz / runtime.config.chip_rate_hz).max(1);
                runtime.hardware_start_time_ns = tick;
                runtime.absolute_sample_origin = chip * oversample as u64;
                runtime.last_hardware_time_ns = tick;
                runtime.last_absolute_chip_start = chip;
                runtime.last_absolute_sample_start = chip * oversample as u64;
                info!(
                    "rx: TX anchor received after {} drain reads: tick={} chip={} abs_sample_origin={}",
                    drained, tick, chip, runtime.absolute_sample_origin
                );
                break;
            }
            if shutdown.load(Ordering::Relaxed) {
                return Ok(());
            }
        }
        // Drain until we reach samples at or after the anchor tick so the
        // first buffer processed by the pipeline has a positive delta_ns.
        loop {
            let result = radio_rx.rx_read(&mut drain_buf, 250_000)?;
            if result.samples_read > 0 && result.time_ticks >= runtime.hardware_start_time_ns {
                info!(
                    "rx: synced to TX anchor, first valid buffer time={}",
                    result.time_ticks
                );
                break;
            }
            if shutdown.load(Ordering::Relaxed) {
                return Ok(());
            }
        }
    } else {
        // No TX anchor (shouldn't happen for live SDR, but keep old path as fallback).
        let anchor = runtime.hardware_start_time_ns;
        let mut drain_buf = vec![Complex32::new(0.0, 0.0); target_batch_samples];
        let mut drained_reads = 0usize;
        let mut drained_samples = 0usize;
        let mut startup_overflow_warned = false;
        loop {
            let result = radio_rx.rx_read(&mut drain_buf, 250_000)?;
            if result.overflow {
                if !startup_overflow_warned {
                    warn!(
                        "rx: SDR overflow during startup drain; temporarily tolerating while catching up to anchor"
                    );
                    startup_overflow_warned = true;
                }
                continue;
            }
            let n = result.samples_read;
            if n == 0 {
                continue;
            }
            let t = result.time_ticks;
            drained_reads += 1;
            drained_samples += n;
            if t >= anchor {
                info!(
                    "rx: drained {} reads ({} samples) of stale data; first valid buffer time_ns={} anchor={}",
                    drained_reads, drained_samples, t, anchor
                );
                break;
            }
        }
    }

    // Unbounded channel: the reader thread must never block on send, otherwise
    // the hardware RX buffer overflows and samples are dropped. The processing
    // thread drains as fast as it can; if it falls behind, the queue grows in
    // memory (preferable to losing samples).
    let (tx, rx_chan) = mpsc::channel::<RxReaderMessage>();

    let shutdown_reader = shutdown.clone();
    let sample_rate_hz = runtime.config.sample_rate_hz;
    let tick_rate = runtime.config.tick_rate;

    std::thread::scope(|scope| {
        // Reader thread: reads from SDR hardware and sends samples + timestamp
        let reader = scope.spawn(move || -> Result<(), Error> {
            let read_batch_samples = target_batch_samples;
            info!(
                "rx: SDR reader read_batch_samples={} read_batch_ms={:.3}",
                read_batch_samples,
                read_batch_samples as f64 * 1000.0 / sample_rate_hz.max(1) as f64,
            );
            let mut buffer = vec![Complex32::new(0.0, 0.0); read_batch_samples];
            let mut last_read = Instant::now();
            let mut read_count: u64 = 0;
            let mut max_gap_us: u64 = 0;
            let mut max_receive_us: u64 = 0;
            let mut last_end_ticks: Option<u64> = None;
            let mut overflow_count: u64 = 0;
            while !shutdown_reader.load(Ordering::Relaxed) {
                let since_last = last_read.elapsed();
                let receive_start = Instant::now();
                let result = radio_rx.rx_read(&mut buffer, 250_000)?;
                let receive_us = receive_start.elapsed().as_micros() as u64;
                max_receive_us = max_receive_us.max(receive_us);
                last_read = Instant::now();
                if result.overflow {
                    overflow_count += 1;
                    let gap_us = since_last.as_micros() as u64;
                    log::warn!(
                        "rx: SDR overflow #{} metadata_samples={} metadata_time_ticks={} \
                         read_count={} gap_since_last_read={}us max_gap={}us \
                         receive={}us max_receive={}us",
                        overflow_count,
                        result.samples_read,
                        result.time_ticks,
                        read_count,
                        gap_us,
                        max_gap_us,
                        receive_us,
                        max_receive_us,
                    );
                }
                let n = result.samples_read;
                if n == 0 {
                    continue;
                }
                if let Some(prev_end) = last_end_ticks {
                    let gap_ticks = result.time_ticks.saturating_sub(prev_end);
                    let gap_samples = (gap_ticks as u128 * sample_rate_hz as u128
                        / tick_rate.max(1) as u128) as usize;
                    if gap_samples > 2 {
                        log::warn!(
                            "rx: SDR timestamp gap after_overflows={} zero_filling_samples={} \
                             gap_ms={:.3} previous_end_ticks={} current_start_ticks={} \
                             read_count={} gap_since_last_read={}us max_gap={}us \
                             receive={}us max_receive={}us",
                            overflow_count,
                            gap_samples,
                            gap_samples as f64 * 1000.0 / sample_rate_hz.max(1) as f64,
                            prev_end,
                            result.time_ticks,
                            read_count,
                            since_last.as_micros(),
                            max_gap_us,
                            receive_us,
                            max_receive_us,
                        );
                        let fill = vec![Complex32::new(0.0, 0.0); gap_samples];
                        if tx
                            .send(RxReaderMessage {
                                samples: fill,
                                time_ns: prev_end,
                                n: gap_samples,
                                enqueue_time: Instant::now(),
                                absolute_sample_start_override: None,
                            })
                            .is_err()
                        {
                            break;
                        }
                    }
                }
                read_count += 1;
                let gap_us = since_last.as_micros() as u64;
                max_gap_us = max_gap_us.max(gap_us);
                let duration_ticks = n as u128 * tick_rate as u128 / sample_rate_hz.max(1) as u128;
                last_end_ticks = Some(result.time_ticks + duration_ticks as u64);

                let time_ns = result.time_ticks;
                let samples = buffer[..n].to_vec();
                if tx
                    .send(RxReaderMessage {
                        samples,
                        time_ns,
                        n,
                        enqueue_time: Instant::now(),
                        absolute_sample_start_override: None,
                    })
                    .is_err()
                {
                    break; // processing thread gone
                }
            }
            Ok(())
        });

        // Processing thread (this thread): receives blocks and runs pipeline
        let process_result = process_rx_from_channel(
            &mut runtime,
            &mut commands_rx,
            &rx_chan,
            &shutdown,
            bearer_bts_id,
            bearer_cell_id,
        );

        // Wait for reader to finish
        drop(rx_chan); // ensure reader's send() fails if we exit first
        let reader_result = reader
            .join()
            .unwrap_or_else(|_| Err("reader thread panicked".into()));

        finalize_capture(&mut runtime);

        // Propagate errors — prefer reader errors (hardware) over pipeline
        reader_result?;
        process_result
    })
}

fn process_rx_message(
    runtime: &mut RxRuntime,
    traffic_threads: &mut Vec<TrafficRxThread>,
    msg: RxReaderMessage,
    bearer_bts_id: u32,
    bearer_cell_id: u32,
) -> Result<(), Error> {
    let oversample = (runtime.config.sample_rate_hz / runtime.config.chip_rate_hz).max(1);
    let iter_start = Instant::now();
    let reader_queue_us = msg.enqueue_time.elapsed().as_micros() as u64;
    let rx_read_completed_at = msg.enqueue_time;
    let n = msg.n;

    // Compute absolute sample position from this buffer's hardware timestamp.
    // Every buffer is independently positioned using the shared anchor:
    //   absolute_sample = origin + ((buffer_hw_time - anchor_hw_time) * sample_rate / 1e9)
    let delta_ns = msg.time_ns.saturating_sub(runtime.hardware_start_time_ns);
    let raw_elapsed_samples = if delta_ns == 0 {
        0u128
    } else {
        (delta_ns as u128) * runtime.config.sample_rate_hz as u128
            / runtime.config.tick_rate as u128
    };
    // Subtract the calibrated RX pipeline delay, scaled from its 4×-chip basis
    // to the derived RX rate, so a received sample is labeled with the absolute
    // sample number at which it was transmitted.
    let scaled_delay = scaled_rx_sample_delay(runtime.config.rx_sample_delay, oversample);
    let elapsed_samples = raw_elapsed_samples.saturating_sub(scaled_delay as u128) as u64;
    let absolute_sample_start = msg.absolute_sample_start_override.unwrap_or_else(|| {
        runtime
            .absolute_sample_origin
            .saturating_add(elapsed_samples)
    });
    if runtime.next_sample_index == 0 {
        info!(
            "rx: first buffer hw_time_ns={} anchor_ns={} delta_ns={} absolute_sample={} absolute_chip={}",
            msg.time_ns,
            runtime.hardware_start_time_ns,
            delta_ns,
            absolute_sample_start,
            absolute_sample_start / oversample as u64
        );
    }
    let absolute_chip_start = absolute_sample_start / oversample as u64;
    runtime.last_hardware_time_ns = msg.time_ns;
    runtime.last_absolute_sample_start = absolute_sample_start;
    runtime.last_absolute_chip_start = absolute_chip_start;

    // Write capture after chip position is known so deferred WAV creation
    // can use the real first-sample chip for the filename.
    let raw_samples = msg.samples;
    let capture_start = Instant::now();
    maybe_write_capture(runtime, &raw_samples)?;
    let capture_us = capture_start.elapsed().as_micros() as u64;

    let raw_relative_sample_start = runtime.next_sample_index;
    if let Some(ref queue) = runtime.config.hrpd_traffic_rx_queue {
        // Drain the synth thread's lock-free command stream in FIFO order so
        // assign/release ordering for a reused MAC index is preserved.
        while let Some(command) = queue.pop() {
            match command {
                HrpdTrafficRxCommand::Release(release) => {
                    runtime.hrpd_traffic_threads.retain(|thread| {
                        let matches =
                            thread.uati == release.uati && thread.mac_index == release.mac_index;
                        if matches {
                            info!(
                                "rx: stopping HRPD reverse traffic receiver uati=0x{:08x} mac={} (released)",
                                thread.uati, thread.mac_index
                            );
                        }
                        !matches
                    });
                }
                HrpdTrafficRxCommand::Assign(assignment) => {
                    if runtime
                        .hrpd_traffic_threads
                        .iter()
                        .any(|t| t.uati == assignment.uati && t.mac_index == assignment.mac_index)
                    {
                        continue;
                    }
                    let Some(hrpd_slice) = runtime.hrpd_rx_slice.as_ref() else {
                        warn!(
                            "rx: HRPD traffic assignment ignored without HRPD reverse RX slice uati=0x{:08x} mac={}",
                            assignment.uati, assignment.mac_index
                        );
                        continue;
                    };
                    info!(
                        "rx: starting HRPD reverse traffic receiver uati=0x{:08x} mac={} shift_hz={:+}",
                        assignment.uati,
                        assignment.mac_index,
                        runtime.config.hrpd_rx_shift_hz.unwrap_or(0)
                    );
                    let thread = spawn_hrpd_traffic_rx_thread(
                        hrpd_slice.output_oversample.max(1),
                        assignment,
                        runtime.config.hrpd_traffic_event_tx.clone(),
                        runtime.config.hrpd_harq_bus.clone(),
                        Arc::new(AtomicBool::new(false)),
                    );
                    // No history replay: the worker spawns before the TCA airs,
                    // so pre-spawn IQ cannot contain the AT's reverse pilot. The
                    // worker starts at the next live block and the stream stays
                    // contiguous from there.
                    runtime.hrpd_traffic_threads.push(thread);
                }
            }
        }
    }
    let (one_x_block, hrpd_block) = match (
        runtime.one_x_rx_slice.as_mut(),
        runtime.hrpd_rx_slice.as_mut(),
    ) {
        (Some(one_x_slice), Some(hrpd_slice)) => {
            let (one_x_block, hrpd_block) = RxCarrierSlice::process_pair(
                one_x_slice,
                hrpd_slice,
                raw_samples,
                raw_relative_sample_start,
                absolute_sample_start,
            );
            (Some(one_x_block), Some(hrpd_block))
        }
        (Some(one_x_slice), None) => (
            Some(one_x_slice.process(
                raw_samples,
                raw_relative_sample_start,
                absolute_sample_start,
            )),
            None,
        ),
        (None, Some(hrpd_slice)) => (
            None,
            Some(hrpd_slice.process(
                raw_samples,
                raw_relative_sample_start,
                absolute_sample_start,
            )),
        ),
        (None, None) => return Err("rx: no reverse carrier slice configured".into()),
    };

    // Keep the HRPD Access Channel correlator fed even while a reverse traffic
    // assignment has pilot lock; all HRPD consumers share this one sliced block.
    if let Some(hrpd_block) = hrpd_block.as_ref() {
        if let Some(thread) = runtime.hrpd_access_thread.as_ref() {
            let _ = thread.tx.send(HrpdAccessRxBlock {
                block: hrpd_block.clone(),
                enqueue_time: Instant::now(),
            });
        }
        runtime.hrpd_traffic_threads.retain(|thread| {
            send_hrpd_traffic_rx_block(
                thread,
                HrpdTrafficRxBlock {
                    samples: hrpd_block.samples.clone(),
                    absolute_sample_start: hrpd_block.absolute_sample_start,
                    sample_rate_hz: hrpd_block.sample_rate_hz,
                    rx_read_completed_at,
                    enqueue_time: Instant::now(),
                },
            )
        });
    } else {
        runtime.hrpd_traffic_threads.clear();
    }

    // Clone samples for traffic RX threads if any are active or pending.
    let has_traffic = one_x_block.is_some()
        && (!traffic_threads.is_empty()
            || runtime
                .config
                .traffic_rx_pool
                .as_ref()
                .is_some_and(|p| !p.lock().is_empty()));
    let traffic_block_msg =
        one_x_block
            .as_ref()
            .filter(|_| has_traffic)
            .map(|block| TrafficRxBlock {
                samples: block.samples.clone(),
                relative_sample_start: block.relative_sample_start,
                absolute_chip_start: block.absolute_chip_start,
                absolute_sample_start: block.absolute_sample_start,
                sample_rate_hz: block.sample_rate_hz,
                hw_time_ns: msg.time_ns,
                enqueue_time: std::time::Instant::now(),
            });

    runtime.next_sample_index = raw_relative_sample_start.saturating_add(n);

    let pipeline_start = Instant::now();
    let outputs = if let Some(one_x_block) = one_x_block {
        let mut block = SampleBlock::new(one_x_block.samples, one_x_block.relative_sample_start)
            .with_sample_rate_hz(one_x_block.sample_rate_hz as f64);
        block.tags.insert(
            "absolute_chip_start",
            one_x_block.absolute_chip_start as i64,
        );
        block.tags.insert(
            "absolute_sample_start",
            one_x_block.absolute_sample_start as i64,
        );
        let mut access_emitter = VecEmitter::new();
        let mut outputs = run_sub_chain_timed(
            &mut runtime.processors,
            block,
            &mut runtime.stage_timings,
            &mut access_emitter,
        );
        outputs.extend(access_emitter.blocks);
        outputs
    } else {
        Vec::new()
    };
    let pipeline_us = pipeline_start.elapsed().as_micros() as u64;
    for blk in outputs {
        if blk.tags.get("access_preamble_detected") == Some(&1) {
            log_access_preamble_event(&blk, runtime.config.chip_rate_hz);
        }
        if blk.tags.get("access_event") == Some(&1) {
            if let Some(event) = build_access_event(
                &blk,
                runtime.config.chip_rate_hz,
                runtime.last_hardware_time_ns,
                runtime.config.auth_mode,
                runtime.config.p_rev_in_use,
                runtime.config.overhead_mcc,
                runtime.config.overhead_imsi_11_12,
            ) {
                if log::log_enabled!(log::Level::Debug) {
                    let now = chrono::Utc::now();
                    let air_age_us = event.receive_time.and_then(|t| {
                        (now - t)
                            .num_microseconds()
                            .map(|us| us.clamp(i64::MIN, i64::MAX))
                    });
                    let t56_margin_us = event.receive_time.and_then(|t| {
                        (t + chrono::Duration::milliseconds(200) - now).num_microseconds()
                    });
                    let receive_chip = event.absolute_chip_start.map(|chip| {
                        // build_access_event stamps receive_time at the end of the 96-symbol
                        // access frame, which is the earliest useful response anchor.
                        chip.saturating_add(96 * 256)
                    });
                    let projected_total_us = runtime
                        .timing_total_us
                        .saturating_add(iter_start.elapsed().as_micros() as u64);
                    let projected_samples = runtime.timing_samples.saturating_add(n);
                    let projected_budget_us = ((projected_samples as u128) * 1_000_000u128
                        / runtime.config.sample_rate_hz.max(1) as u128)
                        as u64;
                    let projected_deficit_ms = projected_total_us
                        .saturating_sub(projected_budget_us)
                        .checked_div(1000)
                        .unwrap_or(0);
                    let top_stage = runtime
                        .stage_timings
                        .iter()
                        .max_by_key(|s| s.total_us)
                        .map(|s| format!("{}:{}ms", s.name, s.total_us / 1000))
                        .unwrap_or_else(|| "n/a".to_string());
                    debug!(
                        "rx_access_event_latency: chip={} abs_chip={:?} receive_chip={:?} type=\"{}\" preamble={} air_age_us={:?} t56_margin_us={:?} reader_queue_us={} pipeline_us={} block_elapsed_us={} projected_deficit_ms={} top_stage={}",
                        event.chip_start,
                        event.absolute_chip_start,
                        receive_chip,
                        event.msg_type_name,
                        event.preamble_frames,
                        air_age_us,
                        t56_margin_us,
                        reader_queue_us,
                        pipeline_us,
                        iter_start.elapsed().as_micros(),
                        projected_deficit_ms,
                        top_stage,
                    );
                }
                let event = event;
                if let Some(store) = &runtime.config.rx_measurements {
                    let key = if let Some(esn) = event.esn {
                        Some(settings::RxMeasurementKey::Esn(esn))
                    } else if let Some(ref imsi) = event.imsi {
                        Some(settings::RxMeasurementKey::Imsi(imsi.clone()))
                    } else {
                        None
                    };
                    if let Some(key) = key {
                        if let Ok(mut map) = store.lock() {
                            map.insert(
                                key,
                                settings::RxMeasurement {
                                    snr_db: event.snr_db,
                                    signal_power_db: event.signal_power_db,
                                    raw_power_db: event.raw_power_db,
                                    demod_quality_pct: event.demod_quality_pct,
                                    timestamp_us: event.wall_clock_us,
                                },
                            );
                        }
                    }
                }
                if let Some(tx) = &runtime.config.access_event_tx {
                    let _ = tx.send(event);
                }
            }
        }
    }

    // Remove traffic channel RX threads that the BSC has torn down.
    // Signal shutdown first so the thread exits even if its channel is full,
    // then drop the sender to unblock any recv().
    if runtime.one_x_rx_slice.is_some()
        && let Some(ref removals) = runtime.config.traffic_rx_removals
    {
        let mut codes = removals.lock();
        for walsh_code in codes.drain(..) {
            let before = traffic_threads.len();
            for t in traffic_threads.iter() {
                if t.walsh_code == walsh_code {
                    t.shutdown.store(true, Ordering::Relaxed);
                }
            }
            traffic_threads.retain(|t| t.walsh_code != walsh_code);
            if traffic_threads.len() < before {
                info!("rx: signaled traffic RX thread stop walsh={}", walsh_code);
            }
        }
    }

    // Check for new traffic channel RX requests from the BSC.
    if runtime.one_x_rx_slice.is_some()
        && let Some(ref pool) = runtime.config.traffic_rx_pool
    {
        let mut requests = pool.lock();
        for req in requests.drain(..) {
            let use_rc3 = req.assigned_rev_rc >= 3;
            info!(
                "rx: starting reverse traffic channel receiver walsh={} esn=0x{:08X} rc={}",
                req.walsh_code,
                req.esn,
                if use_rc3 { "RC3" } else { "RC1" }
            );
            traffic_threads.push(spawn_traffic_rx_thread(
                runtime.pipeline_oversample,
                req.walsh_code,
                req.esn,
                req.preamble_num_pcgs,
                use_rc3,
                req.rev_fch_gating_mode,
                runtime.config.global_finger_pool_size,
                runtime.config.traffic_rx_continuity,
                runtime.config.access_event_tx.clone(),
                runtime.config.reverse_bearer_tx.clone(),
                runtime.config.traffic_channels.clone(),
                runtime.config.power_control.clone(),
                runtime.config.chip_rate_hz,
                runtime.config.traffic_ack_seq_tx.clone(),
                bearer_bts_id,
                bearer_cell_id,
            ));
        }
    }

    // Broadcast IQ to all active traffic RX threads.
    if let Some(blk) = traffic_block_msg {
        traffic_threads.retain(|t| {
            match t.tx.send(TrafficRxBlock {
                samples: blk.samples.clone(),
                relative_sample_start: blk.relative_sample_start,
                absolute_chip_start: blk.absolute_chip_start,
                absolute_sample_start: blk.absolute_sample_start,
                sample_rate_hz: blk.sample_rate_hz,
                hw_time_ns: blk.hw_time_ns,
                enqueue_time: std::time::Instant::now(),
            }) {
                Ok(()) => true,
                Err(mpsc::SendError(_)) => {
                    warn!("rx: traffic RX thread walsh={} exited", t.walsh_code);
                    false
                }
            }
        });
    }

    let total_us = iter_start.elapsed().as_micros() as u64;

    runtime.timing_reads = runtime.timing_reads.saturating_add(1);
    runtime.timing_samples = runtime.timing_samples.saturating_add(n);
    runtime.timing_capture_us = runtime.timing_capture_us.saturating_add(capture_us);
    runtime.timing_pipeline_us = runtime.timing_pipeline_us.saturating_add(pipeline_us);
    runtime.timing_total_us = runtime.timing_total_us.saturating_add(total_us);
    runtime.timing_total_max_us = runtime.timing_total_max_us.max(total_us);

    // Only warn when cumulative pipeline time exceeds cumulative sample
    // budget — meaning the channel buffer is draining and we risk dropping
    // samples. Individual iterations over budget are fine as long as the
    // average keeps up.
    let cumulative_budget_us = ((runtime.timing_samples as u128) * 1_000_000u128
        / runtime.config.sample_rate_hz.max(1) as u128) as u64;
    if runtime.timing_total_us > cumulative_budget_us {
        let deficit_ms = (runtime.timing_total_us - cumulative_budget_us) / 1000;
        let should_warn = runtime
            .last_pipeline_lag_warn
            .is_none_or(|last| last.elapsed() >= Duration::from_millis(500))
            || deficit_ms >= runtime.last_pipeline_lag_warn_deficit_ms.saturating_add(25);
        if should_warn {
            let top_stage = runtime
                .stage_timings
                .iter()
                .chain(runtime.hrpd_stage_timings.iter())
                .max_by_key(|s| s.total_us)
                .map(|s| format!("{}:{}ms", s.name, s.total_us / 1000))
                .unwrap_or_else(|| "n/a".to_string());
            warn!(
                "rx_pipeline_falling_behind: deficit={}ms pipeline={}us this_block={}us avg_rt={:.2}x top_stage={}",
                deficit_ms,
                pipeline_us,
                total_us,
                cumulative_budget_us as f64 / runtime.timing_total_us.max(1) as f64,
                top_stage,
            );
            runtime.last_pipeline_lag_warn = Some(Instant::now());
            runtime.last_pipeline_lag_warn_deficit_ms = deficit_ms;
        }
    }

    let interval_elapsed = runtime.timing_interval_start.elapsed();
    if interval_elapsed.as_secs_f64() >= 1.0 {
        let wall_ms = interval_elapsed.as_millis();
        let avg_total_us = runtime.timing_total_us / runtime.timing_reads.max(1) as u64;
        let sample_budget_us = ((runtime.timing_samples as u128) * 1_000_000u128
            / runtime.config.sample_rate_hz.max(1) as u128) as u64;
        let interval_rt = if runtime.timing_total_us > 0 {
            sample_budget_us as f64 / runtime.timing_total_us as f64
        } else {
            f64::INFINITY
        };
        log::trace!(
            "rx_hardware_heartbeat: hw_time_ns={} absolute_chip_start={} t20={} abs_sample_start={}",
            runtime.last_hardware_time_ns,
            runtime.last_absolute_chip_start,
            time::system_time_20ms_frames(time::system_time_from_chips(
                runtime.last_absolute_chip_start,
                runtime.config.chip_rate_hz as u64
            )),
            runtime.last_absolute_sample_start
        );
        log::trace!(
            "rx_timing: wall={}ms reads={} samples={} capture={}ms pipeline={}ms total={}ms(avg={}us max={}us) rt={:.2}x",
            wall_ms,
            runtime.timing_reads,
            runtime.timing_samples,
            runtime.timing_capture_us / 1000,
            runtime.timing_pipeline_us / 1000,
            runtime.timing_total_us / 1000,
            avg_total_us,
            runtime.timing_total_max_us,
            interval_rt
        );
        if !runtime.stage_timings.is_empty() {
            log::trace!(
                "rx_pipeline_budget: sample_budget={}ms pipeline_actual={}ms",
                sample_budget_us / 1000,
                runtime.timing_pipeline_us / 1000,
            );
            for (idx, stage) in runtime.stage_timings.iter().enumerate() {
                let avg_us = stage.total_us / stage.calls.max(1);
                let pct_pipeline = if runtime.timing_pipeline_us > 0 {
                    100.0 * stage.total_us as f64 / runtime.timing_pipeline_us as f64
                } else {
                    0.0
                };
                let pct_budget = if sample_budget_us > 0 {
                    100.0 * stage.total_us as f64 / sample_budget_us as f64
                } else {
                    0.0
                };
                let stage_rt = if stage.total_us > 0 {
                    sample_budget_us as f64 / stage.total_us as f64
                } else {
                    f64::INFINITY
                };
                log::trace!(
                    "rx_pipeline_stage: stg={} name={} actual={}ms budget={}ms avg={}us max={}us pct_pipeline={:.1}% pct_budget={:.1}% rt={:.2}x",
                    idx,
                    stage.name,
                    stage.total_us / 1000,
                    sample_budget_us / 1000,
                    avg_us,
                    stage.max_us,
                    pct_pipeline,
                    pct_budget,
                    stage_rt,
                );
            }
        }
        if let Some(ref rx_metrics_tx) = runtime.config.rx_metrics_tx {
            let cumulative_budget_us = ((runtime.timing_samples as u128) * 1_000_000u128
                / runtime.config.sample_rate_hz.max(1) as u128)
                as u64;
            let deficit = if runtime.timing_total_us > cumulative_budget_us {
                Some((runtime.timing_total_us - cumulative_budget_us) as f64 / 1000.0)
            } else {
                None
            };
            let _ = rx_metrics_tx.send(RxMetrics {
                reads: runtime.timing_reads as u64,
                samples: runtime.timing_samples as u64,
                rt_ratio: interval_rt,
                capture_us: runtime.timing_capture_us,
                pipeline_us: runtime.timing_pipeline_us,
                total_us: runtime.timing_total_us,
                total_max_us: runtime.timing_total_max_us,
                stages: runtime
                    .stage_timings
                    .iter()
                    .map(|s| {
                        let pct = if runtime.timing_pipeline_us > 0 {
                            100.0 * s.total_us as f64 / runtime.timing_pipeline_us as f64
                        } else {
                            0.0
                        };
                        StageMetrics {
                            name: s.name.to_string(),
                            total_us: s.total_us,
                            calls: s.calls,
                            max_us: s.max_us,
                            pct_pipeline: pct,
                        }
                    })
                    .collect(),
                deficit_ms: deficit,
            });
        }
        runtime.timing_interval_start = Instant::now();
        runtime.timing_reads = 0;
        runtime.timing_samples = 0;
        runtime.timing_read_us = 0;
        runtime.timing_copy_us = 0;
        runtime.timing_capture_us = 0;
        runtime.timing_pipeline_us = 0;
        runtime.timing_total_us = 0;
        runtime.timing_total_max_us = 0;
        runtime.last_pipeline_lag_warn = None;
        runtime.last_pipeline_lag_warn_deficit_ms = 0;
        for stage in &mut runtime.stage_timings {
            stage.total_us = 0;
            stage.calls = 0;
            stage.max_us = 0;
        }
        for stage in &mut runtime.hrpd_stage_timings {
            stage.total_us = 0;
            stage.calls = 0;
            stage.max_us = 0;
        }
    }
    Ok(())
}

fn process_rx_from_channel(
    runtime: &mut RxRuntime,
    commands_rx: &mut tokio_mpsc::Receiver<BtsCommand>,
    rx_chan: &mpsc::Receiver<RxReaderMessage>,
    shutdown: &AtomicBool,
    bearer_bts_id: u32,
    bearer_cell_id: u32,
) -> Result<(), Error> {
    let mut traffic_threads: Vec<TrafficRxThread> = Vec::new();

    while !shutdown.load(Ordering::Relaxed) {
        // Drain commands after processing RX data so that a StopCapture
        // doesn't truncate samples already buffered in the reader channel.
        // The deferred stop in maybe_write_capture ensures the current
        // buffer is written before the capture is finalized.
        let msg = match rx_chan.recv_timeout(std::time::Duration::from_millis(250)) {
            Ok(msg) => msg,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                drain_bts_commands(runtime, commands_rx, shutdown)?;
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        process_rx_message(
            runtime,
            &mut traffic_threads,
            msg,
            bearer_bts_id,
            bearer_cell_id,
        )?;
        drain_bts_commands(runtime, commands_rx, shutdown)?;
    }
    Ok(())
}

pub(super) fn run_injected_rx_loop(
    rx: RxSettings,
    mut commands_rx: tokio_mpsc::Receiver<BtsCommand>,
    injected_rx: InjectedRxReceiver,
    shutdown: Arc<AtomicBool>,
) -> Result<(), Error> {
    let oversample = (rx.sample_rate_hz / rx.chip_rate_hz).max(1) as u64;
    let mut runtime = open_rx_runtime(rx)?;
    start_hrpd_access_worker(&mut runtime, shutdown.clone());
    let bearer_bts_id = runtime.config.base_id as u32;
    let bearer_cell_id: u32 = 1;
    let mut traffic_threads: Vec<TrafficRxThread> = Vec::new();

    while !shutdown.load(Ordering::Relaxed) {
        let msg = match injected_rx.recv_timeout(std::time::Duration::from_millis(250)) {
            Ok(msg) => msg,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                drain_bts_commands(&mut runtime, &mut commands_rx, &shutdown)?;
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let n = msg.samples.len();
        process_rx_message(
            &mut runtime,
            &mut traffic_threads,
            RxReaderMessage {
                samples: msg.samples,
                time_ns: msg.time_ns,
                n,
                enqueue_time: Instant::now(),
                absolute_sample_start_override: msg
                    .absolute_chip_start
                    .map(|chip| chip.saturating_mul(oversample)),
            },
            bearer_bts_id,
            bearer_cell_id,
        )?;
        drain_bts_commands(&mut runtime, &mut commands_rx, &shutdown)?;
    }

    drain_bts_commands(&mut runtime, &mut commands_rx, &shutdown)?;
    finalize_capture(&mut runtime);
    Ok(())
}

fn run_sub_chain_timed(
    chain: &mut [PipelineProcessorShared],
    input: SampleBlock,
    stage_timings: &mut [StageTiming],
    emitter: &mut dyn PipelineEmitter,
) -> Vec<SampleBlock> {
    let mut blocks = vec![input];
    for (idx, processor) in chain.iter_mut().enumerate() {
        let stage_start = Instant::now();
        let mut next = Vec::new();
        for blk in blocks {
            if blk.is_empty() {
                continue;
            }
            next.extend(processor.process_block_emitting(blk, emitter));
        }
        let stage_us = stage_start.elapsed().as_micros() as u64;
        if let Some(stage) = stage_timings.get_mut(idx) {
            stage.total_us = stage.total_us.saturating_add(stage_us);
            stage.calls = stage.calls.saturating_add(1);
            stage.max_us = stage.max_us.max(stage_us);
        }
        blocks = next;
    }
    blocks.retain(|b| !b.is_empty());
    blocks
}

#[derive(Debug, Default)]
struct PowerControlRxCounterWindow {
    log_periodic: bool,
    total_measurements: u64,
    window_measurements: u64,
    /// Sum of pilot symbol SINR (dB) — the loop's control metric.
    window_metric_sum_db: f64,
    /// Min/max of the per-PCG pilot SINR within the window.
    window_metric_min_db: f32,
    window_metric_max_db: f32,
    /// Sum/count of legacy Ec/Io for the diagnostic log line.
    window_legacy_ec_io_sum_db: f64,
    window_legacy_ec_io_count: u64,
    /// Raw (un-smoothed) per-PCG pilot SINR stats — diagnostic only.
    window_raw_sinr_sum_db: f64,
    window_raw_sinr_count: u64,
    window_raw_sinr_min_db: f32,
    window_raw_sinr_max_db: f32,
    /// Latest smoothing-window length reported by the finger (PCGs).
    last_smoothing_window: Option<u32>,
    window_raw_power_count: u64,
    window_raw_power_sum_db: f64,
    window_raw_power_min_db: f32,
    window_raw_power_max_db: f32,
    last_raw_power_db: Option<f32>,
    last_filtered_raw_power_db: Option<f32>,
    window_raw_power_clamp_down: u64,
    /// PCB direction counts: PCB=0 is UP (raise MS Tx), PCB=1 is DOWN.
    window_pcb_up: u64,
    window_pcb_down: u64,
    window_age_chips_sum: u64,
    window_max_age_chips: u64,
    window_over_1pcg: u64,
    last_abs_pcg: u64,
    /// Latest closed-loop snapshot fields (sampled once per record()).
    last_target_db: Option<f32>,
    last_filtered_metric_db: Option<f32>,
    last_fer_pct: Option<f32>,
    last_frames_total: u64,
    last_frames_crc_error: u64,
    last_brake_offset_db: Option<f32>,
}

impl PowerControlRxCounterWindow {
    #[allow(clippy::too_many_arguments)]
    fn record(
        &mut self,
        abs_pcg: u64,
        metric_db: f32,
        legacy_ec_io_db: Option<f32>,
        raw_sinr_db: Option<f32>,
        smoothing_window_len: Option<u32>,
        raw_power_db: Option<f32>,
        filtered_raw_power_db: Option<f32>,
        raw_power_clamp_active: bool,
        pcb: Option<u8>,
        target_db: Option<f32>,
        filtered_metric_db: Option<f32>,
        fer_pct: Option<f32>,
        frames_total: u64,
        frames_crc_error: u64,
        brake_offset_db: Option<f32>,
        age_chips: u64,
    ) -> bool {
        self.total_measurements = self.total_measurements.saturating_add(1);
        self.window_measurements = self.window_measurements.saturating_add(1);
        if metric_db.is_finite() {
            self.window_metric_sum_db += metric_db as f64;
            self.window_metric_min_db =
                if self.window_metric_min_db == 0.0 && self.window_measurements == 1 {
                    metric_db
                } else {
                    self.window_metric_min_db.min(metric_db)
                };
            self.window_metric_max_db =
                if self.window_metric_max_db == 0.0 && self.window_measurements == 1 {
                    metric_db
                } else {
                    self.window_metric_max_db.max(metric_db)
                };
        }
        if let Some(legacy_db) = legacy_ec_io_db.filter(|db| db.is_finite()) {
            self.window_legacy_ec_io_sum_db += legacy_db as f64;
            self.window_legacy_ec_io_count = self.window_legacy_ec_io_count.saturating_add(1);
        }
        if let Some(raw_db) = raw_sinr_db.filter(|db| db.is_finite()) {
            self.window_raw_sinr_sum_db += raw_db as f64;
            self.window_raw_sinr_count = self.window_raw_sinr_count.saturating_add(1);
            self.window_raw_sinr_min_db = if self.window_raw_sinr_count == 1 {
                raw_db
            } else {
                self.window_raw_sinr_min_db.min(raw_db)
            };
            self.window_raw_sinr_max_db = if self.window_raw_sinr_count == 1 {
                raw_db
            } else {
                self.window_raw_sinr_max_db.max(raw_db)
            };
        }
        if smoothing_window_len.is_some() {
            self.last_smoothing_window = smoothing_window_len;
        }
        if let Some(raw_power_db) = raw_power_db.filter(|db| db.is_finite()) {
            self.window_raw_power_count = self.window_raw_power_count.saturating_add(1);
            self.window_raw_power_sum_db += raw_power_db as f64;
            self.window_raw_power_min_db = if self.window_raw_power_count == 1 {
                raw_power_db
            } else {
                self.window_raw_power_min_db.min(raw_power_db)
            };
            self.window_raw_power_max_db = if self.window_raw_power_count == 1 {
                raw_power_db
            } else {
                self.window_raw_power_max_db.max(raw_power_db)
            };
            self.last_raw_power_db = Some(raw_power_db);
        }
        if let Some(filtered_raw_power_db) = filtered_raw_power_db.filter(|db| db.is_finite()) {
            self.last_filtered_raw_power_db = Some(filtered_raw_power_db);
        }
        if raw_power_clamp_active {
            self.window_raw_power_clamp_down = self.window_raw_power_clamp_down.saturating_add(1);
        }
        match pcb {
            Some(0) => self.window_pcb_up = self.window_pcb_up.saturating_add(1),
            Some(1) => self.window_pcb_down = self.window_pcb_down.saturating_add(1),
            _ => {}
        }
        if let Some(t) = target_db.filter(|db| db.is_finite()) {
            self.last_target_db = Some(t);
        }
        if let Some(f) = filtered_metric_db.filter(|db| db.is_finite()) {
            self.last_filtered_metric_db = Some(f);
        }
        if let Some(fer) = fer_pct.filter(|f| f.is_finite()) {
            self.last_fer_pct = Some(fer);
        }
        self.last_frames_total = frames_total;
        self.last_frames_crc_error = frames_crc_error;
        if let Some(brake) = brake_offset_db.filter(|b| b.is_finite()) {
            self.last_brake_offset_db = Some(brake);
        }
        self.window_age_chips_sum = self.window_age_chips_sum.saturating_add(age_chips);
        self.window_max_age_chips = self.window_max_age_chips.max(age_chips);
        if age_chips > 1536 {
            self.window_over_1pcg = self.window_over_1pcg.saturating_add(1);
        }
        self.last_abs_pcg = abs_pcg;
        if self.window_measurements < power_control_verbose_summary_every() {
            return false;
        }
        if self.log_periodic || self.window_raw_power_clamp_down > 0 {
            true
        } else {
            self.reset_window();
            false
        }
    }

    fn should_log_partial(&self) -> bool {
        self.window_measurements > 0 && (self.log_periodic || self.window_raw_power_clamp_down > 0)
    }

    fn log_and_reset(&mut self, walsh_code: u8) {
        if self.window_measurements == 0 {
            return;
        }
        let n = self.window_measurements as f64;
        let avg_metric_db = self.window_metric_sum_db / n;
        let metric_summary = format!(
            "pilot_sinr_avg={:.2} (min={:.2} max={:.2}) filt={}",
            avg_metric_db,
            self.window_metric_min_db,
            self.window_metric_max_db,
            self.last_filtered_metric_db
                .map(|db| format!("{db:.2}"))
                .unwrap_or_else(|| "none".to_string()),
        );
        let raw_sinr_summary = if self.window_raw_sinr_count > 0 {
            format!(
                " raw_sinr_avg={:.2} (min={:.2} max={:.2}){}",
                self.window_raw_sinr_sum_db / self.window_raw_sinr_count as f64,
                self.window_raw_sinr_min_db,
                self.window_raw_sinr_max_db,
                self.last_smoothing_window
                    .map(|w| format!(" smooth_w={w}"))
                    .unwrap_or_default(),
            )
        } else {
            String::new()
        };
        let legacy_summary = if self.window_legacy_ec_io_count > 0 {
            format!(
                " legacy_ec_io_avg={:.2}",
                self.window_legacy_ec_io_sum_db / self.window_legacy_ec_io_count as f64
            )
        } else {
            " legacy_ec_io=missing".to_string()
        };
        let target_summary = self
            .last_target_db
            .map(|t| {
                let brake = self
                    .last_brake_offset_db
                    .map(|b| format!(" brake={b:.2}"))
                    .unwrap_or_default();
                format!(" target={t:.2}{brake}")
            })
            .unwrap_or_default();
        let pcb_total = self.window_pcb_up + self.window_pcb_down;
        let pcb_summary = if pcb_total > 0 {
            format!(
                " pcb_up={}/{} ({:.0}% UP)",
                self.window_pcb_up,
                pcb_total,
                100.0 * self.window_pcb_up as f64 / pcb_total as f64,
            )
        } else {
            String::new()
        };
        let fer_summary = self
            .last_fer_pct
            .map(|fer| {
                format!(
                    " fer={:.2}% frames={}/{}err",
                    fer, self.last_frames_total, self.last_frames_crc_error
                )
            })
            .unwrap_or_default();
        let avg_age_pcgs = self.window_age_chips_sum as f64 / n / 1536.0;
        let max_age_pcgs = self.window_max_age_chips as f64 / 1536.0;
        let raw_power_summary = if self.window_raw_power_count > 0 {
            format!(
                " raw_power_avg_dbfs={:.2} (min={:.2} max={:.2} last={:.2}) filt={}",
                self.window_raw_power_sum_db / self.window_raw_power_count as f64,
                self.window_raw_power_min_db,
                self.window_raw_power_max_db,
                self.last_raw_power_db.unwrap_or(f32::NAN),
                self.last_filtered_raw_power_db
                    .map(|db| format!("{db:.2}"))
                    .unwrap_or_else(|| "none".to_string()),
            )
        } else {
            " raw_power=none".to_string()
        };
        info!(
            "rx_traffic[w{}]: [power counters] total_meas={} window_meas={} {}{}{}{}{}{}{} clamp_down={} age_avg_pcgs={:.2} age_max_pcgs={:.2} over_1pcg={} last_abs_pcg={}",
            walsh_code,
            self.total_measurements,
            self.window_measurements,
            metric_summary,
            raw_sinr_summary,
            legacy_summary,
            target_summary,
            pcb_summary,
            fer_summary,
            raw_power_summary,
            self.window_raw_power_clamp_down,
            avg_age_pcgs,
            max_age_pcgs,
            self.window_over_1pcg,
            self.last_abs_pcg,
        );
        self.reset_window();
    }

    fn reset_window(&mut self) {
        self.window_measurements = 0;
        self.window_metric_sum_db = 0.0;
        self.window_metric_min_db = 0.0;
        self.window_metric_max_db = 0.0;
        self.window_legacy_ec_io_sum_db = 0.0;
        self.window_legacy_ec_io_count = 0;
        self.window_raw_sinr_sum_db = 0.0;
        self.window_raw_sinr_count = 0;
        self.window_raw_sinr_min_db = 0.0;
        self.window_raw_sinr_max_db = 0.0;
        self.window_raw_power_count = 0;
        self.window_raw_power_sum_db = 0.0;
        self.window_raw_power_min_db = 0.0;
        self.window_raw_power_max_db = 0.0;
        self.last_raw_power_db = None;
        self.window_raw_power_clamp_down = 0;
        self.window_pcb_up = 0;
        self.window_pcb_down = 0;
        self.window_age_chips_sum = 0;
        self.window_max_age_chips = 0;
        self.window_over_1pcg = 0;
    }
}

/// Dedicated thread for a single reverse traffic channel receiver.
///
/// Receives IQ blocks from the main RX thread, runs the traffic pipeline,
/// and sends decoded events back to the BSC via `event_tx`. Tracks its own
/// pipeline timing independently so falling-behind warnings are per-channel.
fn run_traffic_rx_thread(
    walsh_code: u8,
    mut processors: Vec<PipelineProcessorShared>,
    iq_rx: mpsc::Receiver<TrafficRxBlock>,
    event_tx: Option<tokio::sync::mpsc::UnboundedSender<AccessChannelEvent>>,
    reverse_bearer_tx: Option<mpsc::Sender<UdpBearerDatagram>>,
    traffic_channels: Option<TrafficChannelPool>,
    power_control: Option<BtsPowerControlRegistry>,
    chip_rate_hz: usize,
    continuity_configured: bool,
    shutdown: Arc<AtomicBool>,
    traffic_ack_seq_tx: Option<tokio::sync::mpsc::Sender<(u8, u8)>>,
    bearer_bts_id: u32,
    bearer_cell_id: u32,
) {
    info!(
        "rx_traffic[w{}]: thread started traffic_rx_continuity={}",
        walsh_code, continuity_configured
    );

    let mut timing_interval_start = Instant::now();
    let mut timing_reads: usize = 0;
    let mut timing_samples: usize = 0;
    let mut timing_pipeline_us: u64 = 0;
    let mut timing_total_us: u64 = 0;
    let mut timing_total_max_us: u64 = 0;
    let mut last_processing_absolute_chip_end: u64 = 0;

    // PCB latency diagnostics: track where delay accumulates.
    let mut latency_queue_us_sum: u64 = 0;
    let mut latency_queue_us_max: u64 = 0;
    let mut latency_pipeline_internal_chips_sum: u64 = 0;
    let mut latency_pipeline_internal_chips_max: u64 = 0;
    let mut latency_recomputed_chips_sum: u64 = 0;
    let mut latency_recomputed_chips_max: u64 = 0;
    let mut latency_measurement_count: u64 = 0;
    let mut power_control_counters = PowerControlRxCounterWindow {
        log_periodic: power_control_verbose_enabled_for_walsh(walsh_code),
        ..PowerControlRxCounterWindow::default()
    };
    let mut continuity_state = TrafficContinuityState::new(continuity_configured);

    let mut emit_outputs = |outputs: Vec<SampleBlock>,
                            hw_time_ns: u64,
                            processing_absolute_chip_end: u64|
     -> bool {
        let mut should_exit = false;
        for out_blk in outputs {
            if out_blk.tags.get("traffic_preamble_detected") == Some(&1) {
                let preamble_pcgs = out_blk
                    .tags
                    .get("traffic_preamble_frames")
                    .copied()
                    .unwrap_or(0);
                let abs_chip = out_blk
                    .tags
                    .get("absolute_chip_start")
                    .copied()
                    .unwrap_or(0);
                let preamble_raw_power_db = out_blk
                    .tags
                    .get("finger_raw_power_mdb")
                    .map(|v| *v as f32 / 1000.0);
                info!(
                    "rx_traffic[w{}]: traffic preamble detected pcgs={} abs_chip={} raw_power_dbfs={}",
                    walsh_code,
                    preamble_pcgs,
                    abs_chip,
                    preamble_raw_power_db
                        .map(|db| format!("{db:.2}"))
                        .unwrap_or_else(|| "none".to_string())
                );
                // Send preamble as FCH Rvs null frame over Abis UDP bearer
                emit_reverse_preamble_bearer(
                    &reverse_bearer_tx,
                    walsh_code,
                    abs_chip as u64,
                    bearer_bts_id,
                    bearer_cell_id,
                );

                let event = build_traffic_preamble_event(
                    walsh_code,
                    out_blk.chip_start,
                    hw_time_ns,
                    preamble_pcgs,
                );
                if let Some(ref tx) = event_tx {
                    match tx.send(event) {
                        Ok(()) => debug!("rx_traffic[w{}]: preamble event sent to BSC", walsh_code),
                        Err(e) => warn!(
                            "rx_traffic[w{}]: failed to send preamble event to BSC: {}",
                            walsh_code, e
                        ),
                    }
                } else {
                    trace!(
                        "rx_traffic[w{}]: preamble detected, no event_tx (Abis bearer used)",
                        walsh_code
                    );
                }
            }
            if out_blk.tags.get("traffic_event") == Some(&1) {
                if let Some(mut event) = build_traffic_event(&out_blk, chip_rate_hz, hw_time_ns) {
                    let frame_valid = traffic_frame_validity(&event);
                    if let Some(power_control) = power_control.as_ref() {
                        let _ = power_control.outer_loop_tick(
                            traffic_channels.as_ref(),
                            walsh_code,
                            frame_valid,
                        );
                    }
                    if let (Some(ack_seq), Some(ack_tx)) = (event.ack_seq, &traffic_ack_seq_tx) {
                        let _ = ack_tx.blocking_send((walsh_code, ack_seq));
                    }
                    event.traffic_primary_bearer_routed = emit_reverse_primary_bearer(
                        &reverse_bearer_tx,
                        &event,
                        bearer_bts_id,
                        bearer_cell_id,
                    );
                    if let Some(ref tx) = event_tx {
                        let _ = tx.send(event);
                    }
                }
            }
            if out_blk.tags.get("traffic_pcg_measurement") == Some(&1) {
                if let Some(event) = build_traffic_pcg_measurement_event(
                    &out_blk,
                    walsh_code,
                    chip_rate_hz,
                    hw_time_ns,
                    processing_absolute_chip_end,
                ) {
                    let eb_nt_db = event
                        .pcg_signal_snr_db
                        .as_ref()
                        .and_then(|values| values.first())
                        .copied()
                        .unwrap_or(f32::NAN);
                    let legacy_ec_io_db = event.reverse_pilot_ec_io_db.or_else(|| {
                        out_blk
                            .tags
                            .get("traffic_pcg_pilot_ec_io_mdb")
                            .map(|v| *v as f32 / 1000.0)
                    });
                    let raw_sinr_db = out_blk
                        .tags
                        .get("traffic_pcg_pilot_sinr_raw_mdb")
                        .map(|v| *v as f32 / 1000.0);
                    let smoothing_window_len = out_blk
                        .tags
                        .get("traffic_pcg_smoothing_window")
                        .and_then(|v| u32::try_from(*v).ok());
                    let mut tick_raw_power = None;
                    let mut tick_filtered_raw_power = None;
                    let mut raw_power_clamp_active = false;
                    let mut tick_pcb: Option<u8> = None;
                    let mut tick_target_db: Option<f32> = None;
                    let mut tick_filtered_metric_db: Option<f32> = None;
                    let mut snapshot_fer_pct: Option<f32> = None;
                    let mut snapshot_frames_total: u64 = 0;
                    let mut snapshot_frames_crc_error: u64 = 0;
                    let mut snapshot_brake_offset_db: Option<f32> = None;
                    if let (Some(power_control), Some(traffic_channels), Some(abs_chip)) = (
                        power_control.as_ref(),
                        traffic_channels.as_ref(),
                        event.absolute_chip_start,
                    ) {
                        let measured_abs_pcg = abs_chip / 1536;
                        let tx_abs_pcg = measured_abs_pcg + PCG_PREDICTION_LEAD_PCGS as u64;
                        if let Some(tick) = power_control.tick_and_schedule(
                            traffic_channels,
                            walsh_code,
                            measured_abs_pcg,
                            tx_abs_pcg,
                            eb_nt_db,
                            event.raw_power_db,
                        ) {
                            tick_raw_power = tick.raw_power_db;
                            tick_filtered_raw_power = tick.filtered_raw_power_db;
                            raw_power_clamp_active = tick.raw_power_clamp_active;
                            tick_pcb = Some(tick.pcb);
                            tick_target_db = Some(tick.target_db);
                            if tick.control_metric_db.is_finite() {
                                tick_filtered_metric_db = Some(tick.control_metric_db);
                            }
                        }
                        if let Some(snap) = power_control.snapshot(walsh_code) {
                            snapshot_fer_pct = Some(snap.fer_pct);
                            snapshot_frames_total = snap.frames_total;
                            snapshot_frames_crc_error = snap.frames_crc_error;
                            snapshot_brake_offset_db = Some(snap.last_brake_offset_db);
                        }
                    }
                    {
                        let counters = &mut power_control_counters;
                        let abs_pcg = event.absolute_chip_start.unwrap_or(0) / 1536;
                        let age_chips = event.traffic_measurement_age_chips.unwrap_or(0);
                        if counters.record(
                            abs_pcg,
                            eb_nt_db,
                            legacy_ec_io_db,
                            raw_sinr_db,
                            smoothing_window_len,
                            tick_raw_power.or(event.raw_power_db),
                            tick_filtered_raw_power,
                            raw_power_clamp_active,
                            tick_pcb,
                            tick_target_db,
                            tick_filtered_metric_db,
                            snapshot_fer_pct,
                            snapshot_frames_total,
                            snapshot_frames_crc_error,
                            snapshot_brake_offset_db,
                            age_chips,
                        ) {
                            counters.log_and_reset(walsh_code);
                        }
                    }
                    if let Some(ref tx) = event_tx {
                        let _ = tx.send(event);
                    }
                }
            }
            if out_blk.tags.get("traffic_phy_status") == Some(&1) {
                if let Some(event) =
                    build_traffic_phy_status_event(&out_blk, chip_rate_hz, hw_time_ns)
                {
                    let frame_valid = traffic_frame_validity(&event);
                    if let Some(power_control) = power_control.as_ref() {
                        let _ = power_control.outer_loop_tick(
                            traffic_channels.as_ref(),
                            walsh_code,
                            frame_valid,
                        );
                    }
                    if let Some(ref tx) = event_tx {
                        let _ = tx.send(event);
                    }
                }
            }
            if out_blk.tags.get("traffic_phy_frame") == Some(&1) {
                if let Some(mut event) =
                    build_traffic_voice_event(&out_blk, chip_rate_hz, hw_time_ns)
                {
                    let frame_valid = traffic_frame_validity(&event);
                    if let Some(power_control) = power_control.as_ref() {
                        let _ = power_control.outer_loop_tick(
                            traffic_channels.as_ref(),
                            walsh_code,
                            frame_valid,
                        );
                    }
                    event.traffic_primary_bearer_routed = emit_reverse_primary_bearer(
                        &reverse_bearer_tx,
                        &event,
                        bearer_bts_id,
                        bearer_cell_id,
                    );
                    if let Some(ref tx) = event_tx {
                        let _ = tx.send(event);
                    }
                }
            }
            if out_blk.tags.get("traffic_search_gave_up") == Some(&1) {
                warn!(
                    "rx_traffic[w{}]: frame aligner gave up searching, exiting thread",
                    walsh_code
                );
                should_exit = true;
            }
        }
        should_exit
    };

    loop {
        if shutdown.load(Ordering::Relaxed) {
            break;
        }
        // Use recv_timeout so we periodically check the shutdown flag
        // even if no IQ blocks arrive (e.g. sender dropped or stalled).
        let blk = match iq_rx.recv_timeout(std::time::Duration::from_millis(500)) {
            Ok(blk) => blk,
            Err(mpsc::RecvTimeoutError::Timeout) => continue,
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        };
        let queue_delay_us = blk.enqueue_time.elapsed().as_micros() as u64;
        latency_queue_us_sum = latency_queue_us_sum.saturating_add(queue_delay_us);
        latency_queue_us_max = latency_queue_us_max.max(queue_delay_us);
        let iter_start = Instant::now();
        let oversample = (blk.sample_rate_hz / chip_rate_hz).max(1);
        let raw_absolute_sample_start = blk.absolute_sample_start;
        let raw_samples_len = blk.samples.len();
        let local_relative_sample_start = continuity_state
            .next_relative_sample_start
            .unwrap_or(blk.relative_sample_start);
        let continuity = if continuity_state.enabled {
            let continuity = reconcile_traffic_stream_continuity(
                blk.samples,
                raw_absolute_sample_start,
                continuity_state.expected_absolute_sample_start,
                continuity_state.last_tail_sample,
            );
            if continuity.inserted_samples > 0 || continuity.dropped_samples > 0 {
                let expected_abs_start = continuity_state
                    .expected_absolute_sample_start
                    .unwrap_or(raw_absolute_sample_start);
                let delta_samples = raw_absolute_sample_start as i128 - expected_abs_start as i128;
                warn!(
                    "rx_traffic[w{}]: corrected sample discontinuity raw_abs_start={} expected_abs_start={} delta_samples={} delta_chips={:.2} inserted={} dropped={} raw_samples={} output_samples={}",
                    walsh_code,
                    raw_absolute_sample_start,
                    expected_abs_start,
                    delta_samples,
                    delta_samples as f64 / oversample.max(1) as f64,
                    continuity.inserted_samples,
                    continuity.dropped_samples,
                    raw_samples_len,
                    continuity.samples.len(),
                );
            }
            continuity
        } else {
            TrafficContinuityBlock {
                samples: blk.samples,
                absolute_sample_start: raw_absolute_sample_start,
                inserted_samples: 0,
                dropped_samples: 0,
            }
        };
        let absolute_sample_start = continuity.absolute_sample_start;
        let absolute_chip_start = absolute_sample_start / oversample as u64;
        let samples = continuity.samples;
        let tail_sample = samples.last().copied();
        let n = samples.len();
        if n == 0 {
            continue;
        }
        continuity_state.next_relative_sample_start =
            Some(local_relative_sample_start.saturating_add(n));
        if continuity_state.enabled {
            continuity_state.expected_absolute_sample_start =
                Some(absolute_sample_start.saturating_add(n as u64));
            continuity_state.last_tail_sample = samples.last().copied();
        }
        let block_chip_span = (n / oversample) as u64;

        let mut block = SampleBlock::new(samples, local_relative_sample_start)
            .with_sample_rate_hz(blk.sample_rate_hz as f64);
        block
            .tags
            .insert("absolute_chip_start", absolute_chip_start as i64);
        block
            .tags
            .insert("absolute_sample_start", absolute_sample_start as i64);

        let pipeline_start = Instant::now();
        let mut sub_emitter = VecEmitter::new();
        let mut outputs = run_sub_chain(&mut processors, block, &mut sub_emitter);
        outputs.extend(sub_emitter.blocks);
        let pipeline_us = pipeline_start.elapsed().as_micros() as u64;
        let processing_absolute_chip_end = absolute_chip_start.saturating_add(block_chip_span);
        last_processing_absolute_chip_end = processing_absolute_chip_end;
        let saw_traffic_activity = outputs.iter().any(|out_blk| {
            out_blk.tags.get("traffic_preamble_detected") == Some(&1)
                || out_blk.tags.get("traffic_event") == Some(&1)
                || out_blk.tags.get("traffic_phy_status") == Some(&1)
                || out_blk.tags.get("traffic_phy_frame") == Some(&1)
        });

        // Collect per-measurement latency breakdown before emit_outputs consumes them.
        for out_blk in &outputs {
            if out_blk.tags.get("traffic_pcg_measurement") == Some(&1) {
                let internal_age = out_blk
                    .tags
                    .get("traffic_measurement_age_chips")
                    .copied()
                    .unwrap_or(0)
                    .max(0) as u64;
                let meas_chip = out_blk
                    .tags
                    .get("absolute_chip_start")
                    .copied()
                    .unwrap_or(0)
                    .max(0) as u64;
                let recomputed_age = processing_absolute_chip_end.saturating_sub(meas_chip);
                latency_pipeline_internal_chips_sum =
                    latency_pipeline_internal_chips_sum.saturating_add(internal_age);
                latency_pipeline_internal_chips_max =
                    latency_pipeline_internal_chips_max.max(internal_age);
                latency_recomputed_chips_sum =
                    latency_recomputed_chips_sum.saturating_add(recomputed_age);
                latency_recomputed_chips_max = latency_recomputed_chips_max.max(recomputed_age);
                latency_measurement_count = latency_measurement_count.saturating_add(1);
            }
        }

        if emit_outputs(outputs, blk.hw_time_ns, processing_absolute_chip_end) {
            break;
        }

        if continuity_state.configured && !continuity_state.enabled && saw_traffic_activity {
            continuity_state.enabled = true;
            continuity_state.expected_absolute_sample_start =
                Some(absolute_sample_start.saturating_add(n as u64));
            continuity_state.last_tail_sample = tail_sample;
            debug!(
                "rx_traffic[w{}]: enabled local continuity abs_start={} next_expected_abs_start={}",
                walsh_code,
                absolute_sample_start,
                continuity_state
                    .expected_absolute_sample_start
                    .unwrap_or(absolute_sample_start),
            );
        }

        let total_us = iter_start.elapsed().as_micros() as u64;
        let sample_rate_hz = blk.sample_rate_hz.max(1);
        let _sample_span_us = (n as u128 * 1_000_000u128 / sample_rate_hz as u128) as u64;

        timing_reads += 1;
        timing_samples += n;
        timing_pipeline_us += pipeline_us;
        timing_total_us += total_us;
        timing_total_max_us = timing_total_max_us.max(total_us);

        let cumulative_budget_us =
            (timing_samples as u128 * 1_000_000u128 / sample_rate_hz as u128) as u64;
        if timing_total_us > cumulative_budget_us {
            let deficit_ms = (timing_total_us - cumulative_budget_us) / 1000;
            let avg_rt = cumulative_budget_us as f64 / timing_total_us.max(1) as f64;
            warn!(
                "rx_traffic_pipeline_falling_behind[w{}]: deficit={}ms pipeline={}us this_block={}us avg_rt={:.2}x",
                walsh_code, deficit_ms, pipeline_us, total_us, avg_rt
            );
        }

        let interval_elapsed = timing_interval_start.elapsed();
        if interval_elapsed.as_secs_f64() >= 1.0 {
            let cumulative_budget_us =
                (timing_samples as u128 * 1_000_000u128 / sample_rate_hz as u128) as u64;
            let interval_rt = if timing_total_us > 0 {
                cumulative_budget_us as f64 / timing_total_us as f64
            } else {
                f64::INFINITY
            };
            debug!(
                "rx_traffic_timing[w{}]: wall={}ms reads={} samples={} pipeline={}ms total={}ms(max={}us) rt={:.2}x",
                walsh_code,
                interval_elapsed.as_millis(),
                timing_reads,
                timing_samples,
                timing_pipeline_us / 1000,
                timing_total_us / 1000,
                timing_total_max_us,
                interval_rt,
            );
            if latency_measurement_count > 0 {
                let m = latency_measurement_count as f64;
                debug!(
                    "rx_traffic_latency[w{}]: measurements={} queue_avg_us={:.0} queue_max_us={} internal_avg_pcgs={:.2} internal_max_pcgs={:.2} recomputed_avg_pcgs={:.2} recomputed_max_pcgs={:.2} delta_avg_pcgs={:.2}",
                    walsh_code,
                    latency_measurement_count,
                    latency_queue_us_sum as f64 / timing_reads.max(1) as f64,
                    latency_queue_us_max,
                    latency_pipeline_internal_chips_sum as f64 / m / 1536.0,
                    latency_pipeline_internal_chips_max as f64 / 1536.0,
                    latency_recomputed_chips_sum as f64 / m / 1536.0,
                    latency_recomputed_chips_max as f64 / 1536.0,
                    (latency_recomputed_chips_sum as f64
                        - latency_pipeline_internal_chips_sum as f64)
                        / m
                        / 1536.0,
                );
            }
            latency_queue_us_sum = 0;
            latency_queue_us_max = 0;
            latency_pipeline_internal_chips_sum = 0;
            latency_pipeline_internal_chips_max = 0;
            latency_recomputed_chips_sum = 0;
            latency_recomputed_chips_max = 0;
            latency_measurement_count = 0;
            timing_interval_start = Instant::now();
            timing_reads = 0;
            timing_samples = 0;
            timing_pipeline_us = 0;
            timing_total_us = 0;
            timing_total_max_us = 0;
        }
    }

    let mut flush_emitter = VecEmitter::new();
    let mut flushed = flush_sub_chain(&mut processors, &mut flush_emitter);
    flushed.extend(flush_emitter.blocks);
    emit_outputs(flushed, 0, last_processing_absolute_chip_end);
    if power_control_counters.should_log_partial() {
        power_control_counters.log_and_reset(walsh_code);
    }

    info!("rx_traffic[w{}]: thread exiting", walsh_code);
}

fn finalize_capture(runtime: &mut RxRuntime) {
    cancel_pending_capture_start(runtime, "RX loop stopped before IQ capture could start");
    if let Err(err) = stop_active_capture(runtime, "RX loop shutting down") {
        warn!("rx: capture finalize error: {}", err);
    }
}

fn maybe_write_capture(runtime: &mut RxRuntime, samples: &[Complex32]) -> Result<(), Error> {
    if runtime.capture_writer.is_none() {
        if let Some(pending) = runtime.pending_capture_start.take() {
            let directory = pending.directory.clone();
            let (wav_path, metadata_path, writer) = create_capture_writer(
                &directory,
                runtime.config.sample_rate_hz,
                runtime.last_absolute_chip_start,
            )?;
            let active = ActiveCapture {
                directory,
                wav_path,
                metadata_path,
                first_absolute_chip_start: runtime.last_absolute_chip_start,
                first_absolute_sample_start: runtime.last_absolute_sample_start,
                first_sample_system_time: time::system_time_from_chips(
                    runtime.last_absolute_chip_start,
                    runtime.config.chip_rate_hz as u64,
                ),
                first_hardware_time_ns: runtime.last_hardware_time_ns,
            };
            runtime.capture_writer = Some(writer);
            runtime.active_capture = Some(active.clone());
            runtime.captured_samples = 0;
            runtime.capture_expected_abs_sample = None;
            runtime.capture_last_tail = None;
            write_capture_metadata(runtime, &active)?;
            respond_pending_capture_start(runtime, &active, pending);
        }
    }
    if runtime.capture_writer.is_none() {
        return Ok(());
    }
    // Reconcile hardware stream gaps against the absolute sample timeline
    // before writing, exactly as the receiver workers do, so a WAV sample's
    // position always equals its absolute sample index minus the anchor. A
    // capture written from the raw buffers silently loses every gap and all
    // signal after it lands early in the file.
    let continuity = reconcile_traffic_stream_continuity(
        samples.to_vec(),
        runtime.last_absolute_sample_start,
        runtime.capture_expected_abs_sample,
        runtime.capture_last_tail,
    );
    if continuity.inserted_samples > 0 || continuity.dropped_samples > 0 {
        warn!(
            "rx: capture stream discontinuity corrected raw_abs_start={} expected_abs_start={:?} inserted={} dropped={}",
            runtime.last_absolute_sample_start,
            runtime.capture_expected_abs_sample,
            continuity.inserted_samples,
            continuity.dropped_samples,
        );
    }
    let samples = continuity.samples.as_slice();
    runtime.capture_expected_abs_sample =
        Some(continuity.absolute_sample_start + samples.len() as u64);
    if let Some(tail) = samples.last() {
        runtime.capture_last_tail = Some(*tail);
    }
    let remaining = runtime
        .capture_target_samples
        .map(|target| target.saturating_sub(runtime.captured_samples))
        .unwrap_or(samples.len());
    let to_write = remaining.min(samples.len());
    if to_write > 0 {
        // Log peak amplitude on first batch so user can spot gain issues early.
        if runtime.captured_samples == 0 {
            let peak = samples[..to_write]
                .iter()
                .map(|s| s.re.abs().max(s.im.abs()))
                .fold(0.0f32, f32::max);
            info!("rx: capture first batch peak_amplitude={:.6}", peak);
            if peak < 1e-4 {
                warn!(
                    "rx: capture samples are near-zero (peak={:.2e}). \
                     RX gain may not be set — try --capture-gain-db 40",
                    peak
                );
            }
        }
        {
            let wav = runtime
                .capture_writer
                .as_mut()
                .expect("capture writer must exist while active");
            write_capture_block(wav, &samples[..to_write])?;
        }
        runtime.captured_samples = runtime.captured_samples.saturating_add(to_write);
        if let Some(active) = runtime.active_capture.as_ref() {
            write_capture_metadata(runtime, active)?;
        }
    }
    if runtime
        .capture_target_samples
        .map(|target| runtime.captured_samples >= target)
        .unwrap_or(false)
    {
        let _ = stop_active_capture(runtime, "IQ capture target reached")?;
    }
    // Handle deferred capture stop: the StopCapture command was received
    // but we deferred it so the current RX buffer could be written first.
    if runtime.pending_capture_stop.is_some() && runtime.active_capture.is_some() {
        let respond_to = runtime.pending_capture_stop.take().unwrap();
        let result = stop_active_capture(runtime, "IQ capture stopped by command")?;
        let _ = respond_to.send(result.ok_or_else(|| "no active IQ capture".to_string()));
    }
    Ok(())
}

fn build_access_event(
    blk: &SampleBlock,
    chip_rate_hz: usize,
    batch_hw_time_ns: u64,
    auth_mode: u8,
    p_rev_in_use: u8,
    overhead_mcc: u16,
    overhead_imsi_11_12: u8,
) -> Option<AccessChannelEvent> {
    const ACCESS_FRAME_CHIPS: u64 = 96 * 256;

    if blk.tags.get("access_crc_valid").copied().unwrap_or(0) != 1 {
        return None;
    }

    let payload_bits: Vec<u8> = blk
        .samples
        .iter()
        .map(|s| if s.re >= 0.5 { 1u8 } else { 0u8 })
        .collect();

    let preamble_frames = blk.tags.get("access_preamble_frames").copied().unwrap_or(0);
    let pd = blk.tags.get("access_pd").copied().unwrap_or(0) as u8;
    let raw_msg_type = blk.tags.get("access_msg_type").copied().unwrap_or(0) as u8;
    let Some(msg_type_id) = MessageId::from_wire(
        crate::lac::message_types::WireChannel::ReverseCommon,
        raw_msg_type,
    ) else {
        warn!("BTS RX: dropping access event with unsupported MSG_TAG 0x{raw_msg_type:02x}");
        return None;
    };
    let absolute_chip_start = blk
        .tags
        .get("absolute_chip_start")
        .copied()
        .and_then(|chip| u64::try_from(chip).ok());
    let receive_time = absolute_chip_start.map(|chip| {
        time::system_time_from_chips(chip.saturating_add(ACCESS_FRAME_CHIPS), chip_rate_hz as u64)
    });

    let decode_ctx = crate::receiver::access_layer3::AccessDecodeContext::new(
        Some(auth_mode),
        Some(p_rev_in_use),
    );
    let bs = Bitstream::new_init(&payload_bits);
    let pdu = match ReverseAccessPdu::decode(&bs) {
        Ok(pdu) => pdu,
        Err(err) => {
            warn!("BTS RX: dropping access event after PDU decode failure: {err}");
            return None;
        }
    };
    let decoded_l3 = match decode_access_message_from_pdu(&pdu, decode_ctx) {
        Ok(decoded_l3) => decoded_l3,
        Err(err) => {
            warn!("BTS RX: dropping access event after Layer 3 decode failure: {err}");
            return None;
        }
    };
    let address = extract_address(&pdu);
    let l3_summary = Some(decoded_l3.summary());
    let pdu_summary = pdu.summary();

    // Extract structured fields from the decoded PDU for BSC state machine use.
    let (
        arq_msg_seq,
        arq_ack_seq,
        arq_ack_req,
        arq_valid_ack,
        msid_type,
        esn,
        meid,
        imsi_m_s1,
        imsi_m_s2,
        imsi_class,
        imsi_addr_num,
        imsi_mcc,
        imsi_11_12_field,
    ) = match &pdu {
        ReverseAccessPdu::Pd01PRev6(p) => {
            let msg_seq = p.arq.as_ref().map(|a| a.msg_seq);
            let ack_seq = p.arq.as_ref().map(|a| a.ack_seq);
            let ack_req = p.arq.as_ref().is_some_and(|a| a.ack_req);
            let valid_ack = p.arq.as_ref().is_some_and(|a| a.valid_ack);
            let ea = extract_addressing_fields(p.addressing.as_ref());
            (
                msg_seq,
                ack_seq,
                ack_req,
                valid_ack,
                ea.msid_type,
                ea.esn,
                ea.meid,
                ea.imsi_m_s1,
                ea.imsi_m_s2,
                ea.imsi_class,
                ea.imsi_addr_num,
                ea.mcc,
                ea.imsi_11_12,
            )
        }
        ReverseAccessPdu::Pd00Legacy(p) => {
            let msg_seq = p.arq.as_ref().map(|a| a.msg_seq);
            let ack_seq = p.arq.as_ref().map(|a| a.ack_seq);
            let ack_req = p.arq.as_ref().is_some_and(|a| a.ack_req);
            let valid_ack = p.arq.as_ref().is_some_and(|a| a.valid_ack);
            let ea = extract_addressing_fields(p.addressing.as_ref());
            (
                msg_seq,
                ack_seq,
                ack_req,
                valid_ack,
                ea.msid_type,
                ea.esn,
                ea.meid,
                ea.imsi_m_s1,
                ea.imsi_m_s2,
                ea.imsi_class,
                ea.imsi_addr_num,
                ea.mcc,
                ea.imsi_11_12,
            )
        }
        _ => (
            None, None, false, false, None, None, None, None, None, None, None, None, None,
        ),
    };

    let mob_p_rev_field = decoded_l3.mob_p_rev();
    let slot_cycle_index_field = decoded_l3.slot_cycle_index();
    let scm_field = decoded_l3.scm();
    let service_option_field = decoded_l3.service_option();
    let data_burst_info = decoded_l3
        .data_burst_fields()
        .map(|(bt, mn, nm, f)| (bt, mn, nm, f.to_vec()));

    let (for_rc_pref_field, rev_rc_pref_field) = match &decoded_l3 {
        AccessMessage::Origination(m) => Some((m.for_rc_pref, m.rev_rc_pref)),
        AccessMessage::PageResponse(m) => Some((m.for_rc_pref, m.rev_rc_pref)),
        _ => None,
    }
    .unwrap_or((None, None));

    let rev_fch_gating_req_field = match &decoded_l3 {
        AccessMessage::Origination(m) => m.rev_fch_gating_req,
        AccessMessage::PageResponse(m) => m.rev_fch_gating_req,
        _ => None,
    };

    let order_code_field = decoded_l3.order_code();

    let (for_supported_rcs, rev_supported_rcs) = (
        decoded_l3.for_supported_rcs(),
        decoded_l3.rev_supported_rcs(),
    );
    let imsi = derive_full_imsi_from_access_identity(
        imsi_m_s1,
        imsi_m_s2,
        imsi_class,
        imsi_mcc,
        imsi_11_12_field,
        overhead_mcc,
        overhead_imsi_11_12,
    );

    Some(AccessChannelEvent {
        event_id: next_access_event_id(),
        chip_start: blk.chip_start,
        absolute_chip_start,
        receive_time,
        preamble_frames,
        pd,
        message_id: msg_type_id,
        msg_type_name: access_message_type_name(raw_msg_type).to_string(),
        address,
        resolved_address: None,
        subscriber_id: None,
        l3_summary,
        decoded_l3: Some(decoded_l3),
        pdu_summary,
        msg_seq: arq_msg_seq,
        ack_seq: arq_ack_seq,
        ack_req: arq_ack_req,
        valid_ack: arq_valid_ack,
        msid_type,
        esn,
        meid,
        imsi,
        imsi_m_s1,
        imsi_m_s2,
        imsi_class,
        imsi_addr_num,
        imsi_mcc,
        imsi_11_12: imsi_11_12_field,
        mob_p_rev: mob_p_rev_field,
        slot_cycle_index: slot_cycle_index_field,
        scm: scm_field,
        burst_type: data_burst_info.as_ref().map(|(bt, _, _, _)| *bt),
        data_burst_fields: data_burst_info.as_ref().map(|(_, _, _, f)| f.clone()),
        data_burst_num_msgs: data_burst_info.as_ref().map(|(_, _, nm, _)| *nm),
        data_burst_msg_number: data_burst_info.as_ref().map(|(_, mn, _, _)| *mn),
        wall_clock_us: chrono::Utc::now().timestamp_micros() as u64,
        rx_wall_time: Some(Instant::now()),
        rx_hw_time_ns: Some(batch_hw_time_ns),
        snr_db: blk.tags.get("finger_snr_mdb").map(|v| *v as f32 / 1000.0),
        signal_power_db: blk
            .tags
            .get("finger_signal_power_mdb")
            .map(|v| *v as f32 / 1000.0),
        reverse_pilot_ec_io_db: reverse_pilot_ec_io_db_from_tags(blk),
        raw_power_db: blk
            .tags
            .get("finger_raw_power_mdb")
            .map(|v| *v as f32 / 1000.0),
        demod_quality_pct: blk.tags.get("access_frame_weak_soft_bits").map(|&weak| {
            // 96 Walsh symbols * 6 code symbols per Walsh = 576 soft bits per frame
            let total = 576.0_f32;
            (100.0 - (weak as f32 / total * 100.0)).clamp(0.0, 100.0)
        }),
        pcg_signal_snr_db: None,
        active_pcg_mask: None,
        traffic_phy_valid: None,
        traffic_fqi_valid: None,
        traffic_tail_valid: None,
        traffic_fqi_bits: None,
        traffic_ml_tail_match: None,
        order_code: order_code_field,
        service_option: service_option_field,
        for_rc_pref: for_rc_pref_field,
        rev_rc_pref: rev_rc_pref_field,
        rev_fch_gating_req: rev_fch_gating_req_field,
        traffic_walsh_code: None,
        is_preamble_only: false,
        is_traffic_pcg_measurement: false,
        is_traffic_phy_status: false,
        traffic_measurement_age_chips: None,
        for_supported_rcs,
        rev_supported_rcs,
        decoded_rdsch: None,
        traffic_primary_bits: None,
        traffic_primary_rate_bps: None,
        traffic_primary_bearer_routed: false,
        traffic_voice_bits: None,
        traffic_voice_rate_bps: None,
        raw_pdu_bits: Some(payload_bits),
    })
}

/// Build a traffic channel event from a decoded reverse traffic channel frame.
///
/// Decodes the r-dsch PDU format: MSG_TYPE(8) + ACK_SEQ(3) + MSG_SEQ(3) +
/// ACK_REQ(1) + ENCRYPTION(2) + message-specific fields. Maps r-dsch MSG_TYPE
/// to the access-channel MSG_TAG constants used by the BSC dispatcher.
fn build_traffic_event(
    blk: &SampleBlock,
    chip_rate_hz: usize,
    batch_hw_time_ns: u64,
) -> Option<AccessChannelEvent> {
    const TRAFFIC_FRAME_CHIPS: u64 = 96 * 256;

    if blk.tags.get("traffic_crc_valid").copied().unwrap_or(0) != 1 {
        return None;
    }

    let walsh_code = blk.tags.get("traffic_walsh_code").copied().unwrap_or(0) as u8;
    let rate_bps = blk
        .tags
        .get("traffic_rate_bps")
        .and_then(|value| u32::try_from(*value).ok())
        .unwrap_or(9600);

    let payload_bits: Vec<u8> = blk
        .samples
        .iter()
        .map(|s| if s.re >= 0.5 { 1u8 } else { 0u8 })
        .collect();

    let absolute_chip_start = blk
        .tags
        .get("absolute_chip_start")
        .copied()
        .and_then(|chip| u64::try_from(chip).ok());
    let receive_time = absolute_chip_start.map(|chip| {
        time::system_time_from_chips(
            chip.saturating_add(TRAFFIC_FRAME_CHIPS),
            chip_rate_hz as u64,
        )
    });

    let bs = Bitstream::new_init(&payload_bits);
    let rdsch = match RdschPdu::decode(&bs) {
        Ok(pdu) => pdu,
        Err(err) => {
            warn!(
                "rx: failed to decode r-dsch PDU on walsh={}: {}",
                walsh_code, err
            );
            return None;
        }
    };

    let order_code = rdsch.l3.order_code();
    let data_burst_info = rdsch
        .l3
        .data_burst_fields()
        .map(|(bt, mn, nm, f)| (bt, mn, nm, f.to_vec()));
    let decoded_l3 = Some(rdsch.l3.clone());
    let l3_summary = Some(rdsch.l3.summary());
    let pdu_summary = rdsch.summary();
    let valid_ack = true;

    info!(
        "rx: traffic event on walsh={} payload_bits={} rdsch={}",
        walsh_code,
        payload_bits.len(),
        pdu_summary,
    );

    Some(AccessChannelEvent {
        event_id: next_access_event_id(),
        chip_start: blk.chip_start,
        absolute_chip_start,
        receive_time,
        preamble_frames: 0,
        pd: 0,
        message_id: rdsch.message_id,
        msg_type_name: rdsch.msg_type_name().to_string(),
        address: None,
        resolved_address: None,
        subscriber_id: None,
        l3_summary,
        decoded_l3,
        pdu_summary,
        msg_seq: Some(rdsch.arq.msg_seq),
        ack_seq: Some(rdsch.arq.ack_seq),
        ack_req: rdsch.arq.ack_req,
        valid_ack,
        msid_type: None,
        esn: None,
        imsi: None,
        meid: None,
        imsi_m_s1: None,
        imsi_m_s2: None,
        imsi_class: None,
        imsi_addr_num: None,
        imsi_mcc: None,
        imsi_11_12: None,
        mob_p_rev: None,
        slot_cycle_index: None,
        scm: None,
        burst_type: data_burst_info.as_ref().map(|(bt, _, _, _)| *bt),
        data_burst_fields: data_burst_info.as_ref().map(|(_, _, _, f)| f.clone()),
        data_burst_num_msgs: data_burst_info.as_ref().map(|(_, _, nm, _)| *nm),
        data_burst_msg_number: data_burst_info.as_ref().map(|(_, mn, _, _)| *mn),
        wall_clock_us: chrono::Utc::now().timestamp_micros() as u64,
        rx_wall_time: Some(Instant::now()),
        rx_hw_time_ns: Some(batch_hw_time_ns),
        snr_db: blk.tags.get("finger_snr_mdb").map(|v| *v as f32 / 1000.0),
        signal_power_db: blk
            .tags
            .get("finger_signal_power_mdb")
            .map(|v| *v as f32 / 1000.0),
        reverse_pilot_ec_io_db: reverse_pilot_ec_io_db_from_tags(blk),
        raw_power_db: blk
            .tags
            .get("finger_raw_power_mdb")
            .map(|v| *v as f32 / 1000.0),
        demod_quality_pct: None,
        pcg_signal_snr_db: blk.pcg_signal_snr_db.clone(),
        active_pcg_mask: blk.active_pcg_mask,
        traffic_phy_valid: traffic_tag_bool(blk, "traffic_phy_valid"),
        traffic_fqi_valid: traffic_tag_bool(blk, "traffic_fqi_valid"),
        traffic_tail_valid: traffic_tag_bool(blk, "traffic_tail_valid"),
        traffic_fqi_bits: traffic_tag_u8(blk, "traffic_fqi_bits"),
        traffic_ml_tail_match: blk.tags.get("traffic_ml_tail_match").map(|v| *v != 0),
        order_code,
        service_option: None,
        for_rc_pref: None,
        rev_rc_pref: None,
        rev_fch_gating_req: None,
        traffic_walsh_code: Some(walsh_code),
        is_preamble_only: false,
        is_traffic_pcg_measurement: false,
        is_traffic_phy_status: false,
        traffic_measurement_age_chips: None,
        for_supported_rcs: Vec::new(),
        rev_supported_rcs: Vec::new(),
        decoded_rdsch: Some(rdsch),
        traffic_primary_bits: Some(payload_bits),
        traffic_primary_rate_bps: Some(rate_bps),
        traffic_primary_bearer_routed: false,
        traffic_voice_bits: None,
        traffic_voice_rate_bps: None,
        raw_pdu_bits: None,
    })
}

fn reverse_pilot_ec_io_db_from_tags(blk: &SampleBlock) -> Option<f32> {
    blk.tags
        .get("finger_pilot_ec_io_mdb")
        .map(|value| *value as f32 / 1000.0)
}

fn traffic_tag_bool(blk: &SampleBlock, key: &'static str) -> Option<bool> {
    blk.tags.get(key).map(|value| *value != 0)
}

fn traffic_tag_u8(blk: &SampleBlock, key: &'static str) -> Option<u8> {
    blk.tags
        .get(key)
        .and_then(|value| u8::try_from(*value).ok())
}

fn build_traffic_phy_status_event(
    blk: &SampleBlock,
    chip_rate_hz: usize,
    batch_hw_time_ns: u64,
) -> Option<AccessChannelEvent> {
    if blk.tags.get("traffic_phy_status").copied().unwrap_or(0) != 1 {
        return None;
    }

    let walsh_code = blk.tags.get("traffic_walsh_code").copied().unwrap_or(0) as u8;
    let rate_bps = blk
        .tags
        .get("traffic_rate_bps")
        .and_then(|value| u32::try_from(*value).ok())
        .unwrap_or(0);
    let fqi_bits = traffic_tag_u8(blk, "traffic_fqi_bits").unwrap_or(0);
    let phy_valid = traffic_tag_bool(blk, "traffic_phy_valid").unwrap_or(false);
    let fqi_valid = traffic_tag_bool(blk, "traffic_fqi_valid");
    let tail_valid = traffic_tag_bool(blk, "traffic_tail_valid");

    let absolute_chip_start = blk
        .tags
        .get("absolute_chip_start")
        .copied()
        .and_then(|chip| u64::try_from(chip).ok());
    let receive_time = absolute_chip_start.map(|chip| {
        time::system_time_from_chips(chip.saturating_add(96 * 256), chip_rate_hz as u64)
    });

    Some(AccessChannelEvent {
        event_id: next_access_event_id(),
        chip_start: blk.chip_start,
        absolute_chip_start,
        receive_time,
        preamble_frames: 0,
        pd: 0,
        message_id: MessageId::GeneralExtension,
        msg_type_name: format!("TrafficPhyStatus(W{} {}bps)", walsh_code, rate_bps),
        address: None,
        resolved_address: None,
        subscriber_id: None,
        l3_summary: None,
        decoded_l3: None,
        pdu_summary: format!(
            "traffic_phy_status walsh={} rate_bps={} phy_valid={} fqi_bits={} fqi_valid={:?} tail_valid={:?}",
            walsh_code, rate_bps, phy_valid, fqi_bits, fqi_valid, tail_valid
        ),
        msg_seq: None,
        ack_seq: None,
        ack_req: false,
        valid_ack: false,
        msid_type: None,
        esn: None,
        imsi: None,
        meid: None,
        imsi_m_s1: None,
        imsi_m_s2: None,
        imsi_class: None,
        imsi_addr_num: None,
        imsi_mcc: None,
        imsi_11_12: None,
        mob_p_rev: None,
        slot_cycle_index: None,
        scm: None,
        burst_type: None,
        data_burst_fields: None,
        data_burst_num_msgs: None,
        data_burst_msg_number: None,
        wall_clock_us: chrono::Utc::now().timestamp_micros() as u64,
        rx_wall_time: Some(Instant::now()),
        rx_hw_time_ns: Some(batch_hw_time_ns),
        snr_db: blk.tags.get("finger_snr_mdb").map(|v| *v as f32 / 1000.0),
        signal_power_db: blk
            .tags
            .get("finger_signal_power_mdb")
            .map(|v| *v as f32 / 1000.0),
        reverse_pilot_ec_io_db: reverse_pilot_ec_io_db_from_tags(blk),
        raw_power_db: blk
            .tags
            .get("finger_raw_power_mdb")
            .map(|v| *v as f32 / 1000.0),
        demod_quality_pct: None,
        pcg_signal_snr_db: blk.pcg_signal_snr_db.clone(),
        active_pcg_mask: blk.active_pcg_mask,
        traffic_phy_valid: traffic_tag_bool(blk, "traffic_phy_valid"),
        traffic_fqi_valid: fqi_valid,
        traffic_tail_valid: tail_valid,
        traffic_fqi_bits: Some(fqi_bits),
        traffic_ml_tail_match: blk.tags.get("traffic_ml_tail_match").map(|v| *v != 0),
        order_code: None,
        service_option: None,
        for_rc_pref: None,
        rev_rc_pref: None,
        rev_fch_gating_req: None,
        traffic_walsh_code: Some(walsh_code),
        is_preamble_only: false,
        is_traffic_pcg_measurement: false,
        is_traffic_phy_status: true,
        traffic_measurement_age_chips: None,
        for_supported_rcs: Vec::new(),
        rev_supported_rcs: Vec::new(),
        decoded_rdsch: None,
        traffic_primary_bits: None,
        traffic_primary_rate_bps: (rate_bps != 0).then_some(rate_bps),
        traffic_primary_bearer_routed: false,
        traffic_voice_bits: None,
        traffic_voice_rate_bps: None,
        raw_pdu_bits: None,
    })
}

fn build_traffic_voice_event(
    blk: &SampleBlock,
    chip_rate_hz: usize,
    batch_hw_time_ns: u64,
) -> Option<AccessChannelEvent> {
    if blk.tags.get("traffic_phy_frame").copied().unwrap_or(0) != 1 {
        return None;
    }

    let walsh_code = blk.tags.get("traffic_walsh_code").copied().unwrap_or(0) as u8;
    let info_bits = blk.tags.get("traffic_info_bits").copied().unwrap_or(0) as usize;
    let rate_bps = blk.tags.get("traffic_rate_bps").copied().unwrap_or(0) as u32;
    let signaling_bits = blk
        .tags
        .get("traffic_mux_signaling_bits")
        .copied()
        .unwrap_or(0) as usize;

    if info_bits == 0 || rate_bps == 0 {
        return None;
    }

    let primary_bits: Vec<u8> = blk
        .samples
        .iter()
        .take(info_bits)
        .map(|s| if s.re >= 0.5 { 1u8 } else { 0u8 })
        .collect();
    if primary_bits.len() != info_bits {
        return None;
    }

    let voice_bits = primary_bits.clone();

    let absolute_chip_start = blk
        .tags
        .get("absolute_chip_start")
        .copied()
        .and_then(|chip| u64::try_from(chip).ok());
    let receive_time = absolute_chip_start.map(|chip| {
        time::system_time_from_chips(chip.saturating_add(96 * 256), chip_rate_hz as u64)
    });

    Some(AccessChannelEvent {
        event_id: next_access_event_id(),
        chip_start: blk.chip_start,
        absolute_chip_start,
        receive_time,
        preamble_frames: 0,
        pd: 0,
        message_id: MessageId::GeneralExtension,
        msg_type_name: format!("TrafficPrimaryFrame({}bps)", rate_bps),
        address: None,
        resolved_address: None,
        subscriber_id: None,
        l3_summary: None,
        decoded_l3: None,
        pdu_summary: format!(
            "primary_frame walsh={} rate_bps={} mux_signaling_bits={}",
            walsh_code, rate_bps, signaling_bits
        ),
        msg_seq: None,
        ack_seq: None,
        ack_req: false,
        valid_ack: false,
        msid_type: None,
        esn: None,
        imsi: None,
        meid: None,
        imsi_m_s1: None,
        imsi_m_s2: None,
        imsi_class: None,
        imsi_addr_num: None,
        imsi_mcc: None,
        imsi_11_12: None,
        mob_p_rev: None,
        slot_cycle_index: None,
        scm: None,
        burst_type: None,
        data_burst_fields: None,
        data_burst_num_msgs: None,
        data_burst_msg_number: None,
        wall_clock_us: chrono::Utc::now().timestamp_micros() as u64,
        rx_wall_time: Some(Instant::now()),
        rx_hw_time_ns: Some(batch_hw_time_ns),
        snr_db: blk.tags.get("finger_snr_mdb").map(|v| *v as f32 / 1000.0),
        signal_power_db: blk
            .tags
            .get("finger_signal_power_mdb")
            .map(|v| *v as f32 / 1000.0),
        reverse_pilot_ec_io_db: reverse_pilot_ec_io_db_from_tags(blk),
        raw_power_db: blk
            .tags
            .get("finger_raw_power_mdb")
            .map(|v| *v as f32 / 1000.0),
        demod_quality_pct: None,
        pcg_signal_snr_db: blk.pcg_signal_snr_db.clone(),
        active_pcg_mask: blk.active_pcg_mask,
        traffic_phy_valid: traffic_tag_bool(blk, "traffic_phy_valid"),
        traffic_fqi_valid: traffic_tag_bool(blk, "traffic_fqi_valid"),
        traffic_tail_valid: traffic_tag_bool(blk, "traffic_tail_valid"),
        traffic_fqi_bits: traffic_tag_u8(blk, "traffic_fqi_bits"),
        traffic_ml_tail_match: blk.tags.get("traffic_ml_tail_match").map(|v| *v != 0),
        order_code: None,
        service_option: None,
        for_rc_pref: None,
        rev_rc_pref: None,
        rev_fch_gating_req: None,
        traffic_walsh_code: Some(walsh_code),
        is_preamble_only: false,
        is_traffic_pcg_measurement: false,
        is_traffic_phy_status: false,
        traffic_measurement_age_chips: None,
        for_supported_rcs: Vec::new(),
        rev_supported_rcs: Vec::new(),
        decoded_rdsch: None,
        traffic_primary_bits: Some(primary_bits),
        traffic_primary_rate_bps: Some(rate_bps),
        traffic_primary_bearer_routed: false,
        traffic_voice_bits: Some(voice_bits),
        traffic_voice_rate_bps: Some(rate_bps),
        raw_pdu_bits: None,
    })
}

/// Build a lightweight preamble-acquired event for a traffic channel.
///
/// Sent when the RC3 pilot detector (or RC1 preamble detector) fires,
/// before any decoded frames. This lets the BSC send BS Ack Order
/// at the spec-correct time (IS-2000 3.6.4.2: "reverse traffic acquired").
fn build_traffic_preamble_event(
    walsh_code: u8,
    chip_start: usize,
    batch_hw_time_ns: u64,
    preamble_pcgs: i64,
) -> AccessChannelEvent {
    AccessChannelEvent {
        event_id: next_access_event_id(),
        chip_start,
        absolute_chip_start: None,
        receive_time: None,
        preamble_frames: preamble_pcgs,
        pd: 0,
        message_id: MessageId::GeneralExtension,
        msg_type_name: format!("TrafficPreamble(W{})", walsh_code),
        address: None,
        resolved_address: None,
        subscriber_id: None,
        l3_summary: None,
        decoded_l3: None,
        pdu_summary: format!(
            "preamble_acquired walsh={} pcgs={}",
            walsh_code, preamble_pcgs
        ),
        msg_seq: None,
        ack_seq: None,
        ack_req: false,
        valid_ack: false,
        msid_type: None,
        esn: None,
        imsi: None,
        meid: None,
        imsi_m_s1: None,
        imsi_m_s2: None,
        imsi_class: None,
        imsi_addr_num: None,
        imsi_mcc: None,
        imsi_11_12: None,
        mob_p_rev: None,
        slot_cycle_index: None,
        scm: None,
        burst_type: None,
        data_burst_fields: None,
        data_burst_num_msgs: None,
        data_burst_msg_number: None,
        wall_clock_us: chrono::Utc::now().timestamp_micros() as u64,
        rx_wall_time: Some(Instant::now()),
        rx_hw_time_ns: Some(batch_hw_time_ns),
        snr_db: None,
        signal_power_db: None,
        reverse_pilot_ec_io_db: None,
        raw_power_db: None,
        demod_quality_pct: None,
        pcg_signal_snr_db: None,
        active_pcg_mask: None,
        traffic_phy_valid: None,
        traffic_fqi_valid: None,
        traffic_tail_valid: None,
        traffic_fqi_bits: None,
        traffic_ml_tail_match: None,
        order_code: None,
        service_option: None,
        for_rc_pref: None,
        rev_rc_pref: None,
        rev_fch_gating_req: None,
        traffic_walsh_code: Some(walsh_code),
        is_preamble_only: true,
        is_traffic_pcg_measurement: false,
        is_traffic_phy_status: false,
        traffic_measurement_age_chips: None,
        for_supported_rcs: Vec::new(),
        rev_supported_rcs: Vec::new(),
        decoded_rdsch: None,
        traffic_primary_bits: None,
        traffic_primary_rate_bps: None,
        traffic_primary_bearer_routed: false,
        traffic_voice_bits: None,
        traffic_voice_rate_bps: None,
        raw_pdu_bits: None,
    }
}

fn build_traffic_pcg_measurement_event(
    blk: &SampleBlock,
    walsh_code: u8,
    chip_rate_hz: usize,
    batch_hw_time_ns: u64,
    processing_absolute_chip_end: u64,
) -> Option<AccessChannelEvent> {
    const TRAFFIC_PCG_CHIPS: u64 = 6 * 256;

    if blk
        .tags
        .get("traffic_pcg_measurement")
        .copied()
        .unwrap_or(0)
        != 1
    {
        return None;
    }

    let eb_nt_db = *blk.pcg_signal_snr_db.as_ref()?.first()?;
    let absolute_chip_start = blk
        .tags
        .get("absolute_chip_start")
        .copied()
        .and_then(|chip| u64::try_from(chip).ok());
    let receive_time = absolute_chip_start.map(|chip| {
        time::system_time_from_chips(chip.saturating_add(TRAFFIC_PCG_CHIPS), chip_rate_hz as u64)
    });
    let measurement_age_chips =
        absolute_chip_start.map(|chip| processing_absolute_chip_end.saturating_sub(chip));

    Some(AccessChannelEvent {
        event_id: next_access_event_id(),
        chip_start: blk.chip_start,
        absolute_chip_start,
        receive_time,
        preamble_frames: 0,
        pd: 0,
        message_id: MessageId::GeneralExtension,
        msg_type_name: format!("TrafficPcgMeasurement(W{})", walsh_code),
        address: None,
        resolved_address: None,
        subscriber_id: None,
        l3_summary: None,
        decoded_l3: None,
        pdu_summary: format!(
            "pcg_measurement walsh={} pilot_ec_nt_db={:.2}",
            walsh_code, eb_nt_db
        ),
        msg_seq: None,
        ack_seq: None,
        ack_req: false,
        valid_ack: false,
        msid_type: None,
        esn: None,
        imsi: None,
        meid: None,
        imsi_m_s1: None,
        imsi_m_s2: None,
        imsi_class: None,
        imsi_addr_num: None,
        imsi_mcc: None,
        imsi_11_12: None,
        mob_p_rev: None,
        slot_cycle_index: None,
        scm: None,
        burst_type: None,
        data_burst_fields: None,
        data_burst_num_msgs: None,
        data_burst_msg_number: None,
        wall_clock_us: chrono::Utc::now().timestamp_micros() as u64,
        rx_wall_time: Some(Instant::now()),
        rx_hw_time_ns: Some(batch_hw_time_ns),
        snr_db: blk.tags.get("finger_snr_mdb").map(|v| *v as f32 / 1000.0),
        signal_power_db: blk
            .tags
            .get("finger_signal_power_mdb")
            .map(|v| *v as f32 / 1000.0),
        reverse_pilot_ec_io_db: reverse_pilot_ec_io_db_from_tags(blk),
        raw_power_db: blk
            .tags
            .get("traffic_pcg_raw_power_mdb")
            .or_else(|| blk.tags.get("finger_raw_power_mdb"))
            .map(|v| *v as f32 / 1000.0),
        demod_quality_pct: None,
        pcg_signal_snr_db: Some(vec![eb_nt_db]),
        active_pcg_mask: None,
        traffic_phy_valid: None,
        traffic_fqi_valid: None,
        traffic_tail_valid: None,
        traffic_fqi_bits: None,
        traffic_ml_tail_match: None,
        order_code: None,
        service_option: None,
        for_rc_pref: None,
        rev_rc_pref: None,
        rev_fch_gating_req: None,
        traffic_walsh_code: Some(walsh_code),
        is_preamble_only: false,
        is_traffic_pcg_measurement: true,
        is_traffic_phy_status: false,
        traffic_measurement_age_chips: measurement_age_chips,
        for_supported_rcs: Vec::new(),
        rev_supported_rcs: Vec::new(),
        decoded_rdsch: None,
        traffic_primary_bits: None,
        traffic_primary_rate_bps: None,
        traffic_primary_bearer_routed: false,
        traffic_voice_bits: None,
        traffic_voice_rate_bps: None,
        raw_pdu_bits: None,
    })
}

fn traffic_frame_validity(event: &AccessChannelEvent) -> bool {
    let is_signaling = event.message_id != MessageId::GeneralExtension;
    let primary_rate = event.traffic_primary_rate_bps.unwrap_or(0);
    if let Some(fqi_bits) = event.traffic_fqi_bits {
        let tail_valid = event.traffic_tail_valid.unwrap_or(false);
        if fqi_bits > 0 {
            tail_valid && event.traffic_fqi_valid.unwrap_or(false)
        } else {
            tail_valid && event.traffic_phy_valid.unwrap_or(true)
        }
    } else if is_signaling || primary_rate >= 4800 {
        event.traffic_phy_valid.unwrap_or(true)
    } else {
        event.traffic_phy_valid.unwrap_or(true)
    }
}

/// Extract addressing summary (IMSI/ESN/MEID) from a decoded PDU.
pub fn extract_address(pdu: &ReverseAccessPdu) -> Option<String> {
    match pdu {
        ReverseAccessPdu::Pd01PRev6(p) => p.addressing.as_ref().map(|a| a.summary()),
        ReverseAccessPdu::Pd00Legacy(p) => p.addressing.as_ref().map(|a| a.summary()),
        _ => None,
    }
}

/// Extract Layer-3 SDU summary from a decoded PDU.
pub fn decode_access_message_from_pdu(
    pdu: &ReverseAccessPdu,
    ctx: crate::receiver::access_layer3::AccessDecodeContext,
) -> Result<AccessMessage, String> {
    match pdu {
        ReverseAccessPdu::Pd01PRev6(p) => {
            let message_id = MessageId::from_wire(
                crate::lac::message_types::WireChannel::ReverseCommon,
                p.header.msg_type,
            )
            .ok_or_else(|| format!("unsupported r-csch MSG_TAG 0x{:02X}", p.header.msg_type))?;
            let header = AccessMessageHeader {
                pd: p.header.pd,
                message_id,
            };
            AccessMessage::decode_sdu_with_context(header, &p.sdu_plus_padding_raw, ctx)
                .map_err(|err| err.to_string())
        }
        ReverseAccessPdu::Pd00Legacy(p) => {
            let message_id = MessageId::from_wire(
                crate::lac::message_types::WireChannel::ReverseCommon,
                p.header.msg_type,
            )
            .ok_or_else(|| format!("unsupported r-csch MSG_TAG 0x{:02X}", p.header.msg_type))?;
            let header = AccessMessageHeader {
                pd: p.header.pd,
                message_id,
            };
            AccessMessage::decode_sdu_with_context(header, &p.sdu_plus_padding_raw, ctx)
                .map_err(|err| err.to_string())
        }
        ReverseAccessPdu::Pd10Modern { .. } => {
            Err("PD=10 reverse-common PDU Layer 3 body decode is unsupported".to_string())
        }
    }
}

/// Extract IMSI_M_S1 (24 bits) and IMSI_M_S2 (10 bits) from a 34-bit IMSI_S value.
/// Per C.S0005-E 2.3.1: IMSI_S = IMSI_S2(10 upper) || IMSI_S1(24 lower).
fn split_imsi_s(imsi_s: u64) -> (u32, u16) {
    let imsi_m_s1 = (imsi_s & 0xFFFFFF) as u32; // lower 24 bits
    let imsi_m_s2 = ((imsi_s >> 24) & 0x3FF) as u16; // upper 10 bits
    (imsi_m_s1, imsi_m_s2)
}

pub fn derive_full_imsi_from_access_identity(
    imsi_m_s1: Option<u32>,
    imsi_m_s2: Option<u16>,
    imsi_class: Option<u8>,
    imsi_mcc: Option<u16>,
    imsi_11_12: Option<u8>,
    overhead_mcc: u16,
    overhead_imsi_11_12: u8,
) -> Option<String> {
    let imsi_s = imsi_s_to_digits_checked(imsi_m_s1?, imsi_m_s2?)?;

    let fallback_mcc = if imsi_class == Some(0) && overhead_mcc <= 999 {
        Some(overhead_mcc)
    } else {
        None
    };
    let fallback_imsi_11_12 = if imsi_class == Some(0) && overhead_imsi_11_12 <= 99 {
        Some(overhead_imsi_11_12)
    } else {
        None
    };

    let mcc = mcc_to_digits(imsi_mcc.or(fallback_mcc)?)?;
    let imsi_11_12 = imsi_11_12_to_digits(imsi_11_12.or(fallback_imsi_11_12)?)?;
    Some(format!("{mcc}{imsi_11_12}{imsi_s}"))
}

/// Extracted IMSI fields from the class-specific MSID encoding.
struct ImsiFields {
    imsi_class: u8,
    imsi_m_s1: u32,
    imsi_m_s2: u16,
    imsi_addr_num: Option<u8>,
    mcc: Option<u16>,
    imsi_11_12: Option<u8>,
}

/// Try to extract IMSI_S (34 bits) and optional MCC/IMSI_11_12 from class-specific IMSI fields.
/// Handles both class 0 and class 1 IMSI encodings, retaining enough detail
/// to page later by IMSI or ESN as appropriate.
fn extract_imsi_from_class_fields(bits: &mut cdma_common::bits::Bitstream) -> Option<ImsiFields> {
    let imsi_class = bits.read_bits(1).ok()? as u8;
    match imsi_class {
        0 => {
            let class0_type = bits.read_bits(2).ok()? as u8;
            match class0_type {
                0b00 => {
                    // reserved(3) + IMSI_S(34)
                    let _ = bits.read_bits(3).ok()?;
                    let imsi_s = bits.read_bits(34).ok()?;
                    let (s1, s2) = split_imsi_s(imsi_s);
                    Some(ImsiFields {
                        imsi_class,
                        imsi_m_s1: s1,
                        imsi_m_s2: s2,
                        imsi_addr_num: None,
                        mcc: None,
                        imsi_11_12: None,
                    })
                }
                0b01 => {
                    // reserved(4) + IMSI_11_12(7) + IMSI_S(34)
                    let _ = bits.read_bits(4).ok()?;
                    let imsi_11_12 = bits.read_bits(7).ok()? as u8;
                    let imsi_s = bits.read_bits(34).ok()?;
                    let (s1, s2) = split_imsi_s(imsi_s);
                    Some(ImsiFields {
                        imsi_class,
                        imsi_m_s1: s1,
                        imsi_m_s2: s2,
                        imsi_addr_num: None,
                        mcc: None,
                        imsi_11_12: Some(imsi_11_12),
                    })
                }
                0b10 => {
                    // reserved(1) + MCC(10) + IMSI_S(34)
                    let _ = bits.read_bits(1).ok()?;
                    let mcc = bits.read_bits(10).ok()? as u16;
                    let imsi_s = bits.read_bits(34).ok()?;
                    let (s1, s2) = split_imsi_s(imsi_s);
                    Some(ImsiFields {
                        imsi_class,
                        imsi_m_s1: s1,
                        imsi_m_s2: s2,
                        imsi_addr_num: None,
                        mcc: Some(mcc),
                        imsi_11_12: None,
                    })
                }
                0b11 => {
                    // reserved(2) + MCC(10) + IMSI_11_12(7) + IMSI_S(34)
                    let _ = bits.read_bits(2).ok()?;
                    let mcc = bits.read_bits(10).ok()? as u16;
                    let imsi_11_12 = bits.read_bits(7).ok()? as u8;
                    let imsi_s = bits.read_bits(34).ok()?;
                    let (s1, s2) = split_imsi_s(imsi_s);
                    Some(ImsiFields {
                        imsi_class,
                        imsi_m_s1: s1,
                        imsi_m_s2: s2,
                        imsi_addr_num: None,
                        mcc: Some(mcc),
                        imsi_11_12: Some(imsi_11_12),
                    })
                }
                _ => None,
            }
        }
        1 => {
            let class1_type = bits.read_bits(1).ok()? as u8;
            match class1_type {
                0 => {
                    // reserved(2) + IMSI_ADDR_NUM(3) + IMSI_11_12(7) + IMSI_S(34)
                    let _ = bits.read_bits(2).ok()?;
                    let imsi_addr_num = bits.read_bits(3).ok()? as u8;
                    let imsi_11_12 = bits.read_bits(7).ok()? as u8;
                    let imsi_s = bits.read_bits(34).ok()?;
                    let (s1, s2) = split_imsi_s(imsi_s);
                    Some(ImsiFields {
                        imsi_class,
                        imsi_m_s1: s1,
                        imsi_m_s2: s2,
                        imsi_addr_num: Some(imsi_addr_num),
                        mcc: None,
                        imsi_11_12: Some(imsi_11_12),
                    })
                }
                1 => {
                    // IMSI_ADDR_NUM(3) + MCC(10) + IMSI_11_12(7) + IMSI_S(34)
                    let imsi_addr_num = bits.read_bits(3).ok()? as u8;
                    let mcc = bits.read_bits(10).ok()? as u16;
                    let imsi_11_12 = bits.read_bits(7).ok()? as u8;
                    let imsi_s = bits.read_bits(34).ok()?;
                    let (s1, s2) = split_imsi_s(imsi_s);
                    Some(ImsiFields {
                        imsi_class,
                        imsi_m_s1: s1,
                        imsi_m_s2: s2,
                        imsi_addr_num: Some(imsi_addr_num),
                        mcc: Some(mcc),
                        imsi_11_12: Some(imsi_11_12),
                    })
                }
                _ => None,
            }
        }
        _ => None,
    }
}

/// Structured addressing fields extracted from a reverse-link PDU.
pub struct ExtractedAddr {
    pub msid_type: Option<u8>,
    pub esn: Option<u32>,
    pub meid: Option<String>,
    pub imsi_m_s1: Option<u32>,
    pub imsi_m_s2: Option<u16>,
    pub imsi_class: Option<u8>,
    pub imsi_addr_num: Option<u8>,
    pub mcc: Option<u16>,
    pub imsi_11_12: Option<u8>,
}

/// Extract structured addressing fields from a decoded PD=01 PDU for BSC use.
pub fn extract_addressing_fields(
    addr: Option<&crate::receiver::access_pdu::RcschAddressingFields>,
) -> ExtractedAddr {
    let Some(addr) = addr else {
        return ExtractedAddr {
            msid_type: None,
            esn: None,
            meid: None,
            imsi_m_s1: None,
            imsi_m_s2: None,
            imsi_class: None,
            imsi_addr_num: None,
            mcc: None,
            imsi_11_12: None,
        };
    };
    let msid_type = Some(addr.msid_type);
    let mut esn = None;
    let mut meid = None;
    let mut imsi_m_s1 = None;
    let mut imsi_m_s2 = None;
    let mut imsi_class = None;
    let mut imsi_addr_num = None;
    let mut mcc = None;
    let mut imsi_11_12 = None;

    let apply_imsi = |fields: &ImsiFields,
                      s1: &mut Option<u32>,
                      s2: &mut Option<u16>,
                      class: &mut Option<u8>,
                      addr_num: &mut Option<u8>,
                      m: &mut Option<u16>,
                      i: &mut Option<u8>| {
        *s1 = Some(fields.imsi_m_s1);
        *s2 = Some(fields.imsi_m_s2);
        *class = Some(fields.imsi_class);
        *addr_num = fields.imsi_addr_num;
        *m = fields.mcc;
        *i = fields.imsi_11_12;
    };

    match addr.msid_type {
        0b001 if addr.msid_raw.len() >= 32 => {
            // ESN only
            let mut bits = addr.msid_raw.clone();
            esn = bits.read_bits(32).ok().map(|v| v as u32);
        }
        0b000 if addr.msid_raw.len() >= 66 => {
            // IMSI_S + ESN: IMSI_M_S1(24) + IMSI_M_S2(10) + ESN(32)
            let mut bits = addr.msid_raw.clone();
            imsi_m_s1 = bits.read_bits(24).ok().map(|v| v as u32);
            imsi_m_s2 = bits.read_bits(10).ok().map(|v| v as u16);
            esn = bits.read_bits(32).ok().map(|v| v as u32);
        }
        0b010 if addr.msid_raw.len() >= 1 => {
            // IMSI only: IMSI_CLASS(1) + class-specific fields
            let mut bits = addr.msid_raw.clone();
            if let Some(fields) = extract_imsi_from_class_fields(&mut bits) {
                apply_imsi(
                    &fields,
                    &mut imsi_m_s1,
                    &mut imsi_m_s2,
                    &mut imsi_class,
                    &mut imsi_addr_num,
                    &mut mcc,
                    &mut imsi_11_12,
                );
            }
        }
        0b011 if addr.msid_raw.len() >= 33 => {
            // IMSI + ESN: ESN(32) + IMSI_CLASS(1) + class-specific fields
            let mut bits = addr.msid_raw.clone();
            esn = bits.read_bits(32).ok().map(|v| v as u32);
            if let Some(fields) = extract_imsi_from_class_fields(&mut bits) {
                apply_imsi(
                    &fields,
                    &mut imsi_m_s1,
                    &mut imsi_m_s2,
                    &mut imsi_class,
                    &mut imsi_addr_num,
                    &mut mcc,
                    &mut imsi_11_12,
                );
            }
        }
        0b100 => {
            // Extended MSID (MEID, IMSI+MEID, IMSI+ESN+MEID)
            match addr.ext_msid_type {
                Some(0b000) if addr.msid_raw.len() >= 56 => {
                    let mut bits = addr.msid_raw.clone();
                    meid = bits.read_bits(56).ok().map(|v| format!("{v:014x}"));
                }
                Some(0b010) if addr.msid_raw.len() >= 32 => {
                    // IMSI+ESN+MEID: ESN(32) + MEID(56) + IMSI
                    let mut bits = addr.msid_raw.clone();
                    esn = bits.read_bits(32).ok().map(|v| v as u32);
                    meid = bits.read_bits(56).ok().map(|v| format!("{v:014x}"));
                    if let Some(fields) = extract_imsi_from_class_fields(&mut bits) {
                        apply_imsi(
                            &fields,
                            &mut imsi_m_s1,
                            &mut imsi_m_s2,
                            &mut imsi_class,
                            &mut imsi_addr_num,
                            &mut mcc,
                            &mut imsi_11_12,
                        );
                    }
                }
                Some(0b001) if addr.msid_raw.len() >= 56 => {
                    // IMSI+MEID: MEID(56) + IMSI
                    let mut bits = addr.msid_raw.clone();
                    meid = bits.read_bits(56).ok().map(|v| format!("{v:014x}"));
                    if let Some(fields) = extract_imsi_from_class_fields(&mut bits) {
                        apply_imsi(
                            &fields,
                            &mut imsi_m_s1,
                            &mut imsi_m_s2,
                            &mut imsi_class,
                            &mut imsi_addr_num,
                            &mut mcc,
                            &mut imsi_11_12,
                        );
                    }
                }
                _ => {}
            }
        }
        _ => {}
    }

    ExtractedAddr {
        msid_type,
        esn,
        meid,
        imsi_m_s1,
        imsi_m_s2,
        imsi_class,
        imsi_addr_num,
        mcc,
        imsi_11_12,
    }
}

fn log_access_preamble_event(blk: &SampleBlock, chip_rate_hz: usize) {
    let abs_chip = blk.tags.get("absolute_chip_start").copied().unwrap_or(-1);
    let (abs_sys_time, abs_t20) = if abs_chip >= 0 {
        let sys_time = time::system_time_from_chips(abs_chip as u64, chip_rate_hz as u64);
        (
            sys_time.to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
            time::system_time_20ms_frames(sys_time),
        )
    } else {
        ("<unknown>".to_string(), 0)
    };
    debug!(
        "access_preamble_event: chip={} preamble_frames={} info_ones={} pilot_phase={} pn_phase={} abs_chip={} abs_sys_time={} abs_t20={} lc_acquired={} lc_delta={}",
        blk.chip_start,
        blk.tags.get("access_preamble_frames").copied().unwrap_or(0),
        blk.tags
            .get("access_preamble_info_ones")
            .copied()
            .unwrap_or(-1),
        blk.tags.get("pilot_phase").copied().unwrap_or(-1),
        blk.tags.get("pn_phase").copied().unwrap_or(-1),
        abs_chip,
        abs_sys_time,
        abs_t20,
        blk.tags
            .get("reverse_access_lc_acquired")
            .copied()
            .unwrap_or(0),
        blk.tags
            .get("reverse_access_lc_chip_delta")
            .copied()
            .unwrap_or(0),
    );
}

fn create_capture_writer(
    dir: &PathBuf,
    sample_rate_hz: usize,
    chip_start: u64,
) -> Result<(PathBuf, PathBuf, WavWriter<BufWriter<std::fs::File>>), Error> {
    fs::create_dir_all(dir)?;
    let wav_path = dir.join(format!("{chip_start}.wav"));
    let metadata_path = dir.join(format!("{chip_start}.json"));
    info!("rx: capture writing to {}", wav_path.display());
    let writer = BufWriter::new(std::fs::File::create(&wav_path)?);
    Ok((
        wav_path,
        metadata_path,
        WavWriter::new(
            writer,
            WavSpec {
                channels: 2,
                sample_rate: sample_rate_hz as u32,
                bits_per_sample: 16,
                sample_format: SampleFormat::Int,
            },
        )?,
    ))
}

fn write_capture_block(
    wav: &mut WavWriter<BufWriter<std::fs::File>>,
    samples: &[Complex32],
) -> Result<(), Error> {
    // The SDR delivers normalized IQ in [-1.0, 1.0], which maps straight onto
    // the full 16-bit range.
    for sample in samples {
        let re = (sample.re.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        let im = (sample.im.clamp(-1.0, 1.0) * i16::MAX as f32) as i16;
        wav.write_sample(re)?;
        wav.write_sample(im)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::rx_events::extract_access_rc_preferences;
    use super::{
        HRPD_SLOT_CHIPS, HRPD_TRAFFIC_FRAME_CHIPS, HRPD_TRAFFIC_MAX_INTERPOLATED_GAP_SLOTS,
        HrpdTrafficMaskCandidate, PCG_CHIPS, RxCarrierSlice, StageTiming, build_access_event,
        build_hrpd_access_indication, build_traffic_event, build_traffic_voice_event,
        carrier_slice_anti_alias_taps, effective_rx_target_batch_samples,
        extract_addressing_fields, extract_imsi_from_class_fields,
        hrpd_reverse_traffic_pilot_metric_at_offset, reconcile_traffic_stream_continuity,
        reconcile_traffic_stream_continuity_with_max_insert, reverse_frame_content_from_rate_bps,
        run_sub_chain_timed, scaled_rx_sample_delay,
    };
    use crate::bts::evdo::{EvdoMode, HrpdSectorId};
    use crate::bts::launcher::{BtsLaunchOptions, build_bts_launch_parts};
    use crate::bts::{BtsNodeConfig, RadioConfig};
    use crate::lac::message_types::{MessageId, WireChannel};
    use crate::receiver::access_layer3::AccessMessage;
    use crate::receiver::access_pdu::RcschAddressingFields;
    use crate::receiver::hrpd::reverse_spread::{
        HrpdReversePilotReferenceConfig, hrpd_reverse_pilot_reference_chips,
    };
    use crate::receiver::pipelined::{
        HrpdReverseAccessSettings, SampleBlock, hrpd_reverse_access_chain,
    };
    use crate::sdr::NoopRadio;
    use cdma_common::bits::Bitstream;
    use cdma_common::hrpd::air::default_reverse_traffic_long_code_masks;
    use num_complex::Complex32;
    use std::collections::HashMap;
    use std::path::PathBuf;

    #[test]
    fn scaled_rx_sample_delay_scales_from_4x_basis() {
        assert_eq!(scaled_rx_sample_delay(100, 4), 100);
        assert_eq!(scaled_rx_sample_delay(100, 8), 200);
        assert_eq!(scaled_rx_sample_delay(100, 16), 400);
        assert_eq!(scaled_rx_sample_delay(-40, 8), -80);
    }

    #[test]
    fn hrpd_rx_batch_is_capped_to_one_slot() {
        let sample_rate_hz = 4 * 1_228_800usize;
        let chip_rate_hz = 1_228_800usize;
        assert_eq!(
            effective_rx_target_batch_samples(sample_rate_hz, chip_rate_hz, 2, true),
            4 * HRPD_SLOT_CHIPS
        );
        assert_eq!(
            effective_rx_target_batch_samples(sample_rate_hz, chip_rate_hz, 2, false),
            4 * 2 * PCG_CHIPS
        );
    }

    #[test]
    fn hrpd_only_runtime_omits_one_x_receiver_pipeline() {
        let mut bts = BtsNodeConfig::default();
        bts.radio = RadioConfig::Noop;
        bts.channel.cdma_channel = 777;
        bts.evdo.enabled = true;
        bts.evdo.channel = Some(630);
        bts.evdo.mode = EvdoMode::HrpdOnly;
        bts.evdo.overhead.sector_id = Some(HrpdSectorId::new([0; 16]));
        bts.evdo.overhead.subnet_mask = Some(26);
        bts.evdo.overhead.color_code = Some(26);
        bts.rf = crate::bts::config::BtsRfProfile::derive(bts.channel, &bts.evdo)
            .expect("derive HRPD-only RF profile");
        bts.runtime.tx_sample_rate_hz = bts.rf.tx_sample_rate_hz;
        bts.runtime.tx_bandwidth_hz = bts.rf.tx_bandwidth_hz;

        let mut parts = build_bts_launch_parts(
            bts,
            Box::new(NoopRadio::new()),
            BtsLaunchOptions {
                paging_ack_timeout_ms: 100,
                paging_max_retries: 0,
            },
        )
        .expect("build HRPD-only launch parts");
        let rx = parts.bts.config.rx.take().expect("HRPD RX settings");
        let runtime = super::open_rx_runtime(rx).expect("open HRPD-only RX runtime");

        assert!(runtime.one_x_rx_slice.is_none());
        assert!(runtime.processors.is_empty());
        assert!(runtime.stage_timings.is_empty());
        assert!(runtime.hrpd_rx_slice.is_some());
        assert!(runtime.hrpd_processors.is_some());
    }

    #[test]
    fn synthetic_hrpd_reverse_traffic_pilot_metric_locks_spec_reference() {
        let oversample = 4usize;
        let frame_start_chip = 0x1abc0000u64;
        let frame_start_chip =
            frame_start_chip - (frame_start_chip % HRPD_TRAFFIC_FRAME_CHIPS as u64);
        let uati = 0x1a058001;
        let (i_mask, q_mask) = default_reverse_traffic_long_code_masks(uati);
        let reference = hrpd_reverse_pilot_reference_chips(HrpdReversePilotReferenceConfig {
            start_chip: frame_start_chip,
            len: HRPD_TRAFFIC_FRAME_CHIPS,
            i_mask,
            q_mask,
            reference_chip_offset: 0,
            pn_phase_offset_chips: 0,
            lc_phase_offset_chips: 0,
            q_sign: -1.0,
            q_pair_phase: 0,
        });
        let mut samples = Vec::with_capacity(reference.len() * oversample);
        for chip in reference {
            samples.extend(std::iter::repeat_n(chip, oversample));
        }
        let metric = hrpd_reverse_traffic_pilot_metric_at_offset(
            &samples,
            frame_start_chip * oversample as u64,
            oversample,
            frame_start_chip,
            0,
            HrpdTrafficMaskCandidate {
                i_mask,
                q_mask,
                q_sign: -1.0,
                q_pair_phase: 0,
                label: "synthetic",
            },
            16,
        )
        .expect("synthetic metric");
        assert!(
            metric.coherence > 0.99,
            "coherence={} snr_db={}",
            metric.coherence,
            metric.snr_db
        );
        assert!(
            metric.snr_db > 40.0,
            "coherence={} snr_db={}",
            metric.coherence,
            metric.snr_db
        );
    }

    #[test]
    fn reverse_frame_content_maps_rc3_subrates() {
        use cdma_abis::bearer::{
            REVERSE_FRAME_CONTENT_EIGHTH_RATE, REVERSE_FRAME_CONTENT_FULL_RATE,
            REVERSE_FRAME_CONTENT_HALF_RATE, REVERSE_FRAME_CONTENT_QUARTER_RATE,
        };

        assert_eq!(
            reverse_frame_content_from_rate_bps(9600),
            REVERSE_FRAME_CONTENT_FULL_RATE
        );
        assert_eq!(
            reverse_frame_content_from_rate_bps(4800),
            REVERSE_FRAME_CONTENT_HALF_RATE
        );
        assert_eq!(
            reverse_frame_content_from_rate_bps(2700),
            REVERSE_FRAME_CONTENT_QUARTER_RATE
        );
        assert_eq!(
            reverse_frame_content_from_rate_bps(2400),
            REVERSE_FRAME_CONTENT_QUARTER_RATE
        );
        assert_eq!(
            reverse_frame_content_from_rate_bps(1500),
            REVERSE_FRAME_CONTENT_EIGHTH_RATE
        );
        assert_eq!(
            reverse_frame_content_from_rate_bps(1200),
            REVERSE_FRAME_CONTENT_EIGHTH_RATE
        );
    }

    #[test]
    fn rx_carrier_slice_decimates_8x_to_4x_and_retags_samples() {
        let mut slice =
            RxCarrierSlice::new(2_205_000, 9_830_400, 1_228_800).expect("8x carrier slice builds");
        assert_eq!(slice.decimation, 2);
        assert_eq!(slice.output_sample_rate_hz, 4_915_200);
        assert_eq!(slice.output_oversample, 4);

        let raw_abs = 7_198_559_219_644_671u64;
        let samples = vec![Complex32::new(1.0, 0.0); 32];
        let out = slice.process(samples, 101, raw_abs);
        assert_eq!(out.sample_rate_hz, 4_915_200);
        assert_eq!(out.absolute_sample_start, (raw_abs + 1) / 2);
        assert_eq!(out.relative_sample_start, 51);
        assert_eq!(out.absolute_chip_start, out.absolute_sample_start / 4);
        assert_eq!(out.samples.len(), 16);
    }

    /// Independent reference for the carrier slice: exact per-sample mix,
    /// full-rate convolution, then decimate aligned to the absolute grid.
    /// Mirrors the pre-fusion algorithm so the fused implementation can be
    /// checked for equivalence.
    fn carrier_slice_reference(
        carrier_shift_hz: i64,
        sample_rate_hz: usize,
        chip_rate_hz: usize,
        history: &[Complex32],
        block: &[Complex32],
        raw_absolute_sample_start: u64,
    ) -> Vec<Complex32> {
        let input_oversample = sample_rate_hz / chip_rate_hz.max(1);
        let decimation = (input_oversample / 4).max(1);
        let phase_step = if carrier_shift_hz == 0 {
            0.0
        } else {
            -2.0 * std::f64::consts::PI * carrier_shift_hz as f64 / sample_rate_hz as f64
        };
        let taps = (decimation > 1)
            .then(|| carrier_slice_anti_alias_taps(decimation, sample_rate_hz, chip_rate_hz));
        // Mix the whole stream (history precedes the block) with an exact NCO.
        let total = history.len() + block.len();
        let mut mixed = Vec::with_capacity(total);
        let mut phase = 0.0f64;
        for s in history.iter().chain(block.iter()) {
            let rot = Complex32::new(phase.cos() as f32, phase.sin() as f32);
            mixed.push(*s * rot);
            phase += phase_step;
        }
        // Convolve at full rate (causal, symmetric taps).
        let filtered: Vec<Complex32> = if let Some(taps) = &taps {
            (0..total)
                .map(|m| {
                    let mut acc = num::complex::Complex::<f64>::new(0.0, 0.0);
                    for (i, &tap) in taps.iter().enumerate() {
                        if m >= i {
                            let s = mixed[m - i];
                            acc.re += s.re as f64 * tap;
                            acc.im += s.im as f64 * tap;
                        }
                    }
                    Complex32::new(acc.re as f32, acc.im as f32)
                })
                .collect()
        } else {
            mixed
        };
        // Keep block positions whose absolute index is a multiple of decimation.
        let block_start = history.len();
        let d = decimation as u64;
        let first_idx = ((d - raw_absolute_sample_start % d) % d) as usize;
        (first_idx..block.len())
            .step_by(decimation)
            .map(|m| filtered[block_start + m])
            .collect()
    }

    #[test]
    fn rx_carrier_slice_matches_reference_with_shift_and_decimation() {
        let (shift, rate, chip) = (2_205_000i64, 9_830_400usize, 1_228_800usize);
        let mut slice = RxCarrierSlice::new(shift, rate, chip).expect("slice builds");
        // Drive several contiguous blocks so filter/NCO continuity is exercised.
        let raw_abs0 = 7_198_559_219_644_670u64;
        let mut history: Vec<Complex32> = Vec::new();
        let mut max_err = 0.0f32;
        for blk in 0..6u64 {
            let len = 1000usize + (blk as usize) * 37;
            let block: Vec<Complex32> = (0..len)
                .map(|i| {
                    let t = (history.len() + i) as f32;
                    Complex32::new((0.013 * t).sin(), (0.019 * t + 0.4).cos())
                })
                .collect();
            let raw_abs = raw_abs0 + history.len() as u64;
            let expected = carrier_slice_reference(shift, rate, chip, &history, &block, raw_abs);
            let got = slice.process(block.clone(), 0, raw_abs);
            assert_eq!(got.samples.len(), expected.len(), "block {blk} length");
            for (a, b) in got.samples.iter().zip(expected.iter()) {
                max_err = max_err.max((a - b).norm());
            }
            history.extend(block);
        }
        assert!(
            max_err < 1e-3,
            "fused slice diverged from reference: {max_err}"
        );
    }

    #[test]
    fn carrier_slice_filter_has_unity_dc_gain() {
        let taps = carrier_slice_anti_alias_taps(2, 9_830_400, 1_228_800);
        let gain: f64 = taps.iter().sum();
        assert!((gain - 1.0).abs() < 1e-9, "gain={gain}");
    }

    fn class1_imsi_bits(imsi_addr_num: u8, mcc: u16, imsi_11_12: u8, imsi_s: u64) -> Bitstream {
        let mut bits = Bitstream::new();
        bits.write_u8(1, 1); // IMSI_CLASS = 1
        bits.write_u8(1, 1); // CLASS_1_TYPE = 1
        bits.write_u8(imsi_addr_num, 3);
        bits.write_u32(mcc as u32, 10);
        bits.write_u8(imsi_11_12, 7);
        bits.write_u64(imsi_s, 34);
        bits
    }

    fn access_block_from_bits(bits: &Bitstream, raw_msg_type: u8) -> SampleBlock {
        let mut tags = HashMap::new();
        tags.insert("access_crc_valid", 1);
        tags.insert("access_pd", 1);
        tags.insert("access_msg_type", raw_msg_type as i64);
        let samples = bits
            .bits()
            .iter()
            .map(|bit| Complex32::new(*bit as f32, 0.0))
            .collect();
        SampleBlock::new(samples, 0).with_tags(tags)
    }

    fn truncated_pd01_origination_bits() -> Bitstream {
        let mut bits = Bitstream::new();
        bits.write_u8(0x44, 8); // PD=01, MSG_TAG=Origination
        bits.write_u8(2, 5); // LAC_LENGTH
        bits.write_u8(0b101, 3); // ACK_SEQ
        bits.write_u8(0b011, 3); // MSG_SEQ
        bits.write_u8(1, 1); // ACK_REQ
        bits.write_u8(0, 1); // VALID_ACK
        bits.write_u8(0b010, 3); // ACK_TYPE
        bits.write_u8(7, 6); // ACTIVE_PILOT_STRENGTH
        bits.write_u8(1, 1); // FIRST_IS_ACTIVE
        bits.write_u8(0, 1); // FIRST_IS_PTA
        bits.write_u8(0, 3); // NUM_ADD_PILOTS
        bits.write_u8(0xa5, 8); // truncated Origination SDU
        bits
    }

    #[test]
    fn build_access_event_drops_layer3_decode_failure() {
        let bits = truncated_pd01_origination_bits();
        let block = access_block_from_bits(&bits, 0x04);

        let event = build_access_event(&block, 1_228_800, 0, 0, 6, 310, 0);

        assert!(event.is_none());
    }

    #[test]
    fn build_access_event_drops_unmapped_msg_tag_without_gem_fallback() {
        let bits = truncated_pd01_origination_bits();
        let block = access_block_from_bits(&bits, 0x0b);

        let event = build_access_event(&block, 1_228_800, 0, 0, 6, 310, 0);

        assert!(event.is_none());
    }

    #[test]
    fn extract_imsi_from_class_fields_supports_class1_type1() {
        let imsi_s1 = 0x91989e;
        let imsi_s2 = 0x326;
        let imsi_s = ((imsi_s2 as u64) << 24) | (imsi_s1 as u64);
        let mut bits = class1_imsi_bits(6, 310, 0x7f, imsi_s);

        let fields = extract_imsi_from_class_fields(&mut bits).expect("class-1 IMSI should parse");
        assert_eq!(fields.imsi_class, 1);
        assert_eq!(fields.imsi_m_s1, imsi_s1);
        assert_eq!(fields.imsi_m_s2, imsi_s2);
        assert_eq!(fields.imsi_addr_num, Some(6));
        assert_eq!(fields.mcc, Some(310));
        assert_eq!(fields.imsi_11_12, Some(0x7f));
    }

    #[test]
    fn extract_addressing_fields_preserves_class1_imsi_and_esn() {
        let imsi_s1 = 0x91989e;
        let imsi_s2 = 0x326;
        let imsi_s = ((imsi_s2 as u64) << 24) | (imsi_s1 as u64);
        let mut msid_raw = Bitstream::new();
        msid_raw.write_u32(0x4cdc1d09, 32);
        let imsi_bits = class1_imsi_bits(6, 310, 0x7f, imsi_s);
        for &bit in imsi_bits.bits() {
            msid_raw.write_u8(bit, 1);
        }

        let addr = RcschAddressingFields {
            raw: msid_raw.clone(),
            msid_type: 0b011,
            ext_msid_type: None,
            msid_len_octets: msid_raw.len().div_ceil(8) as u8,
            actual_msid_octets: msid_raw.len().div_ceil(8),
            msid_raw,
        };

        let extracted = extract_addressing_fields(Some(&addr));
        assert_eq!(extracted.esn, Some(0x4cdc1d09));
        assert_eq!(extracted.imsi_class, Some(1));
        assert_eq!(extracted.imsi_addr_num, Some(6));
        assert_eq!(extracted.imsi_m_s1, Some(imsi_s1));
        assert_eq!(extracted.imsi_m_s2, Some(imsi_s2));
        assert_eq!(extracted.mcc, Some(310));
        assert_eq!(extracted.imsi_11_12, Some(0x7f));
    }

    fn traffic_frame_from_pdu_bits(bits: &Bitstream) -> SampleBlock {
        let samples = bits
            .bits()
            .iter()
            .map(|&bit| {
                if bit == 0 {
                    Complex32::new(0.0, 0.0)
                } else {
                    Complex32::new(1.0, 0.0)
                }
            })
            .collect();

        SampleBlock {
            chip_start: 12345,
            samples,
            sample_rate_hz: 0.0,
            rx_sample_time: None,
            tags: [
                ("traffic_crc_valid", 1),
                ("traffic_walsh_code", 10),
                ("absolute_chip_start", 987654321),
            ]
            .into_iter()
            .collect(),
            pcg_signal_snr_db: None,
            active_pcg_mask: None,
            pcg_pilot_metrics: None,
        }
    }

    fn build_rdsch_order_bits(ack_seq: u8, msg_seq: u8, ack_req: bool) -> Bitstream {
        let mut bits = Bitstream::new();
        bits.write_u32(
            MessageId::Order
                .wire_type(WireChannel::ReverseDedicated)
                .expect("reverse dedicated order wire type") as u32,
            8,
        );
        bits.write_u32(ack_seq as u32, 3);
        bits.write_u32(msg_seq as u32, 3);
        bits.write_u32(ack_req as u32, 1);
        bits.write_u32(0, 2); // ENCRYPTION
        bits.write_u32(0b010000, 6); // Mobile Station Acknowledgment
        bits.write_u32(0, 3); // ADD_RECORD_LEN
        bits.write_u32(0, 3); // padding
        bits
    }

    #[test]
    fn build_traffic_event_marks_non_sentinel_ack_seq_as_valid() {
        let bits = build_rdsch_order_bits(3, 1, false);
        let blk = traffic_frame_from_pdu_bits(&bits);

        let event = build_traffic_event(&blk, 1_228_800, 0).expect("traffic event");

        assert_eq!(event.ack_seq, Some(3));
        assert!(event.valid_ack);
    }

    #[test]
    fn build_traffic_event_preserves_all_ones_ack_seq_as_candidate_ack() {
        let bits = build_rdsch_order_bits(0b111, 1, false);
        let blk = traffic_frame_from_pdu_bits(&bits);

        let event = build_traffic_event(&blk, 1_228_800, 0).expect("traffic event");

        assert_eq!(event.ack_seq, Some(0b111));
        assert!(event.valid_ack);
    }

    #[test]
    fn extract_access_rc_preferences_supports_page_response() {
        let msg =
            AccessMessage::PageResponse(crate::receiver::access_layer3::PageResponseMessage {
                header: crate::receiver::access_layer3::AccessMessageHeader {
                    pd: 0,
                    message_id: MessageId::PageResponse,
                },
                mob_term: false,
                slot_cycle_index: 2,
                mob_p_rev: 6,
                scm: 0x3a,
                request_mode: 1,
                service_option: 6,
                pm: false,
                nar_an_cap: false,
                encryption_supported: Some(1),
                num_alt_so: 0,
                alt_service_options: Vec::new(),
                uzid_incl: Some(false),
                uzid: None,
                ch_ind: Some(1),
                otd_supported: Some(false),
                qpch_supported: Some(true),
                enhanced_rc: Some(true),
                for_rc_pref: Some(3),
                rev_rc_pref: Some(3),
                fch_supported: Some(true),
                fch_capability: None,
                dcch_supported: Some(false),
                dcch_capability: None,
                rev_fch_gating_req: Some(true),
                sts_supported: None,
                cch_3x_supported: None,
                wll_incl: None,
                wll_device_type: None,
                hook_status: None,
                enc_info_incl: None,
                sig_encrypt_sup: None,
                d_sig_encrypt_req: None,
                c_sig_encrypt_req: None,
                new_sseq_h: None,
                new_sseq_h_sig: None,
                ui_encrypt_req: None,
                ui_encrypt_sup: None,
                sync_id_incl: None,
                sync_id_len: None,
                sync_id: None,
                so_bitmap_ind: None,
                so_group_num: None,
                so_bitmap: None,
                alt_band_class_sup: None,
                msg_int_info_incl: None,
                sig_integrity_sup_incl: None,
                sig_integrity_sup: None,
                sig_integrity_req: None,
                new_key_id: None,
                new_sseq_h_incl: None,
                for_pdch_supported: None,
                for_pdch_capability: None,
                ext_ch_ind: None,
                sign_slot_cycle_index: None,
                bcmc_incl: None,
                bcmc_pref_incl: None,
                bcmc: None,
                rev_pdch_supported: None,
                rev_pdch_capability: None,
                band_sub_rep_incl: None,
                num_band_subclass: None,
                band_subclass_sup: None,
                remaining_bits: 0,
            });

        assert_eq!(
            extract_access_rc_preferences(&msg),
            (Some(3), Some(3), Some(true))
        );
    }

    #[test]
    fn build_traffic_voice_event_exposes_primary_payload_bits() {
        let samples = vec![
            Complex32::new(1.0, 0.0),
            Complex32::new(0.0, 0.0),
            Complex32::new(1.0, 0.0),
            Complex32::new(1.0, 0.0),
        ];
        let blk = SampleBlock {
            chip_start: 54321,
            samples,
            sample_rate_hz: 0.0,
            rx_sample_time: None,
            tags: [
                ("traffic_phy_frame", 1),
                ("traffic_walsh_code", 11),
                ("traffic_info_bits", 4),
                ("traffic_rate_bps", 9600),
                ("traffic_mux_signaling_bits", 0),
                ("absolute_chip_start", 123456789),
            ]
            .into_iter()
            .collect(),
            pcg_signal_snr_db: None,
            active_pcg_mask: None,
            pcg_pilot_metrics: None,
        };

        let event = build_traffic_voice_event(&blk, 1_228_800, 0).expect("voice frame event");

        assert_eq!(event.traffic_walsh_code, Some(11));
        assert_eq!(event.traffic_primary_bits, Some(vec![1, 0, 1, 1]));
        assert_eq!(event.traffic_primary_rate_bps, Some(9600));
        assert_eq!(event.traffic_voice_bits, Some(vec![1, 0, 1, 1]));
        assert_eq!(event.traffic_voice_rate_bps, Some(9600));
    }

    #[test]
    fn reconcile_traffic_stream_continuity_inserts_linear_gap_samples() {
        let out = reconcile_traffic_stream_continuity(
            vec![Complex32::new(3.0, -3.0), Complex32::new(4.0, -4.0)],
            110,
            Some(106),
            Some(Complex32::new(1.0, -1.0)),
        );

        assert_eq!(out.absolute_sample_start, 106);
        assert_eq!(out.inserted_samples, 4);
        assert_eq!(out.dropped_samples, 0);
        assert_eq!(out.samples.len(), 6);
        let expected = [
            Complex32::new(1.4, -1.4),
            Complex32::new(1.8, -1.8),
            Complex32::new(2.2, -2.2),
            Complex32::new(2.6, -2.6),
            Complex32::new(3.0, -3.0),
            Complex32::new(4.0, -4.0),
        ];
        for (actual, expected) in out.samples.iter().zip(expected.iter()) {
            assert!((actual.re - expected.re).abs() < 1e-6);
            assert!((actual.im - expected.im).abs() < 1e-6);
        }
    }

    #[test]
    fn reconcile_traffic_stream_continuity_gap_limit_resets_large_gap() {
        let raw_samples = vec![Complex32::new(3.0, -3.0), Complex32::new(4.0, -4.0)];
        let out = reconcile_traffic_stream_continuity_with_max_insert(
            raw_samples.clone(),
            110,
            Some(106),
            Some(Complex32::new(1.0, -1.0)),
            Some(2),
        );

        assert_eq!(out.absolute_sample_start, 110);
        assert_eq!(out.inserted_samples, 0);
        assert_eq!(out.dropped_samples, 0);
        assert_eq!(out.samples, raw_samples);
    }

    #[test]
    fn reconcile_traffic_stream_continuity_corrects_live_sized_hrpd_gap() {
        let oversample = 4usize;
        let gap_samples = oversample * HRPD_SLOT_CHIPS * 4 + oversample * 8;
        let max_insert_samples =
            oversample * HRPD_SLOT_CHIPS * HRPD_TRAFFIC_MAX_INTERPOLATED_GAP_SLOTS;
        let raw_samples = vec![Complex32::new(3.0, -3.0), Complex32::new(4.0, -4.0)];
        let out = reconcile_traffic_stream_continuity_with_max_insert(
            raw_samples,
            1_000 + gap_samples as u64,
            Some(1_000),
            Some(Complex32::new(1.0, -1.0)),
            Some(max_insert_samples),
        );

        assert_eq!(out.absolute_sample_start, 1_000);
        assert_eq!(out.inserted_samples, gap_samples);
        assert_eq!(out.dropped_samples, 0);
        assert_eq!(out.samples.len(), gap_samples + 2);
    }

    #[test]
    fn reconcile_traffic_stream_continuity_trims_overlapping_prefix() {
        let out = reconcile_traffic_stream_continuity(
            vec![
                Complex32::new(10.0, 0.0),
                Complex32::new(11.0, 0.0),
                Complex32::new(12.0, 0.0),
                Complex32::new(13.0, 0.0),
            ],
            200,
            Some(202),
            Some(Complex32::new(9.0, 0.0)),
        );

        assert_eq!(out.absolute_sample_start, 202);
        assert_eq!(out.inserted_samples, 0);
        assert_eq!(out.dropped_samples, 2);
        assert_eq!(
            out.samples,
            vec![Complex32::new(12.0, 0.0), Complex32::new(13.0, 0.0)]
        );
    }

    #[derive(Default)]
    struct HrpdTrafficCaptureFixture {
        label: &'static str,
        wav_path: PathBuf,
        sample_rate_hz: usize,
        chip_rate_hz: usize,
        first_absolute_sample_start: u64,
        start_sample_offset: usize,
        hrpd_rx_shift_hz: i64,
        expected_uati: u32,
        mac_index: u8,
        min_drc_events: usize,
        physical_layer_subtype: u16,
        reverse_traffic_mac_subtype: u16,
        /// Exact decoded reverse Stream 0 / Stream 1 event counts, when the
        /// capture has a locked baseline (None skips the assertion).
        expected_stream0_events: Option<usize>,
        expected_stream1_events: Option<usize>,
        /// Payloads that must appear among the decoded Stream 0 events
        /// (exact match) and Stream 1 events (prefix match).
        expected_stream0_payloads: &'static [&'static [u8]],
        expected_stream1_payload_prefixes: &'static [&'static [u8]],
    }

    struct HrpdAccessCaptureFixture {
        label: &'static str,
        wav_path: PathBuf,
        sample_rate_hz: usize,
        chip_rate_hz: usize,
        first_absolute_sample_start: u64,
        hrpd_rx_shift_hz: i64,
        expected_packet_chips: &'static [u64],
        expected_message_counts: &'static [usize],
    }

    fn drive_hrpd_access_worker_capture(fixture: HrpdAccessCaptureFixture) {
        let _ = env_logger::builder().is_test(true).try_init();

        assert_eq!(
            fixture.expected_packet_chips.len(),
            fixture.expected_message_counts.len()
        );
        let mut reader = hound::WavReader::open(&fixture.wav_path)
            .unwrap_or_else(|e| panic!("failed to open {}: {e}", fixture.wav_path.display()));
        let spec = reader.spec();
        assert_eq!(spec.channels, 2);
        assert_eq!(spec.sample_rate as usize, fixture.sample_rate_hz);
        assert_eq!(spec.bits_per_sample, 16);
        assert_eq!(spec.sample_format, hound::SampleFormat::Int);

        let mut slice = RxCarrierSlice::new(
            fixture.hrpd_rx_shift_hz,
            fixture.sample_rate_hz,
            fixture.chip_rate_hz,
        )
        .expect("RxCarrierSlice for HRPD access capture");
        let mut processors = hrpd_reverse_access_chain(HrpdReverseAccessSettings {
            oversample: slice.output_oversample,
            access_cycle_number: 0,
            sector_id_lsb: 0,
            color_code: 26,
            reanchor_origin: true,
            snr_threshold: None,
            finger_pool_size: 8,
            preamble_frames: crate::receiver::hrpd::access::HRPD_DEFAULT_ACCESS_PREAMBLE_FRAMES,
            enhanced_access_rates: false,
        });
        let stage_timings = processors
            .iter()
            .map(|processor| StageTiming {
                name: processor.name(),
                total_us: 0,
                calls: 0,
                max_us: 0,
            })
            .collect::<Vec<_>>();
        let mut stage_timings = stage_timings;

        const SAMPLES_PER_BLOCK: usize = 98_304; // 10 ms at 9.8304 Msps.
        let mut samples_iter = reader.samples::<i16>();
        let mut raw_relative_sample_start = 0usize;
        let mut raw_absolute_sample_start = fixture.first_absolute_sample_start;
        let mut events = Vec::new();
        let mut total_samples_pushed = 0usize;

        loop {
            let mut block: Vec<Complex32> = Vec::with_capacity(SAMPLES_PER_BLOCK);
            for _ in 0..SAMPLES_PER_BLOCK {
                let Some(i) = samples_iter.next() else { break };
                let Some(q) = samples_iter.next() else { break };
                let i = i.unwrap_or(0) as f32 / i16::MAX as f32;
                let q = q.unwrap_or(0) as f32 / i16::MAX as f32;
                block.push(Complex32::new(i, q));
            }
            if block.is_empty() {
                break;
            }
            let len = block.len();
            let hrpd_block =
                slice.process(block, raw_relative_sample_start, raw_absolute_sample_start);
            let mut sample_block =
                SampleBlock::new(hrpd_block.samples, hrpd_block.relative_sample_start)
                    .with_sample_rate_hz(hrpd_block.sample_rate_hz as f64);
            sample_block
                .tags
                .insert("absolute_chip_start", hrpd_block.absolute_chip_start as i64);
            sample_block.tags.insert(
                "absolute_sample_start",
                hrpd_block.absolute_sample_start as i64,
            );
            let mut emitter = crate::receiver::pipelined::VecEmitter::new();
            let mut outputs = run_sub_chain_timed(
                &mut processors,
                sample_block,
                &mut stage_timings,
                &mut emitter,
            );
            outputs.extend(emitter.blocks);
            for output in outputs {
                if let Some(indication) = build_hrpd_access_indication(&output, 26, 0) {
                    events.push(indication);
                }
            }
            raw_relative_sample_start = raw_relative_sample_start.saturating_add(len);
            raw_absolute_sample_start = raw_absolute_sample_start.saturating_add(len as u64);
            total_samples_pushed = total_samples_pushed.saturating_add(len);
        }

        let chips = events
            .iter()
            .map(|event| event.absolute_chip)
            .collect::<Vec<_>>();
        let message_counts = events
            .iter()
            .map(|event| event.messages.len())
            .collect::<Vec<_>>();
        eprintln!(
            "HRPD {} access: streamed {} samples decoded={} chips={:?} message_counts={:?}",
            fixture.label,
            total_samples_pushed,
            events.len(),
            chips,
            message_counts
        );
        assert_eq!(
            chips, fixture.expected_packet_chips,
            "decoded HRPD access burst chips did not match capture"
        );
        assert_eq!(
            message_counts, fixture.expected_message_counts,
            "decoded HRPD access message counts did not match capture"
        );
    }

    fn drive_hrpd_reverse_traffic_worker_capture(fixture: HrpdTrafficCaptureFixture) {
        let _ = env_logger::builder().is_test(true).try_init();
        use cdma_common::hrpd::air::{HrpdTrafficAssignmentRequest, HrpdTrafficEvent};
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        use std::time::{Duration, Instant};
        use tokio::sync::mpsc as tokio_mpsc;

        let mut reader = hound::WavReader::open(&fixture.wav_path)
            .unwrap_or_else(|e| panic!("failed to open {}: {e}", fixture.wav_path.display()));
        let spec = reader.spec();
        assert_eq!(spec.channels, 2);
        assert_eq!(spec.sample_rate as usize, fixture.sample_rate_hz);
        assert_eq!(spec.bits_per_sample, 16);
        assert_eq!(spec.sample_format, hound::SampleFormat::Int);

        let (reverse_long_code_mask_i, reverse_long_code_mask_q) =
            default_reverse_traffic_long_code_masks(fixture.expected_uati);
        let assignment = HrpdTrafficAssignmentRequest {
            session_uati: fixture.expected_uati,
            uati: fixture.expected_uati,
            mac_index: fixture.mac_index,
            reverse_rate_limit_bps: 153_600,
            reverse_long_code_mask_i,
            reverse_long_code_mask_q,
            drc_lock: true,
            physical_layer_subtype: fixture.physical_layer_subtype,
            reverse_traffic_mac_subtype: fixture.reverse_traffic_mac_subtype,
            frame_offset: 0,
            drc_cover: 0,
            drc_length: 1,
        };

        let mut slice = RxCarrierSlice::new(
            fixture.hrpd_rx_shift_hz,
            fixture.sample_rate_hz,
            fixture.chip_rate_hz,
        )
        .expect("RxCarrierSlice for HRPD reverse capture");
        let (event_tx, mut event_rx) = tokio_mpsc::unbounded_channel::<HrpdTrafficEvent>();
        let shutdown = Arc::new(AtomicBool::new(false));
        // Feed the worker a real HARQ bus so the per-slot reverse power-control
        // loop runs against real IQ and emits its `rpc_control` diagnostics.
        let harq_bus = Arc::new(crate::bts::hrpd::HarqBus::new());
        let thread = super::spawn_hrpd_traffic_rx_thread(
            slice.output_oversample.max(1),
            assignment.clone(),
            Some(event_tx),
            Some(harq_bus.clone()),
            shutdown.clone(),
        );

        // Stream WAV in ~85 ms blocks (the same order of magnitude the live
        // BTS main RX loop hands the worker on real hardware) so the worker
        // processes incrementally rather than against one giant buffer.
        const SAMPLES_PER_BLOCK: usize = 1 << 18; // 262144 samples ≈ 53 ms
        let mut samples_iter = reader.samples::<i16>();
        for _ in 0..fixture.start_sample_offset {
            let _ = samples_iter.next();
            let _ = samples_iter.next();
        }
        let mut raw_relative_sample_start: usize = fixture.start_sample_offset;
        let mut raw_absolute_sample_start: u64 = fixture
            .first_absolute_sample_start
            .saturating_add(fixture.start_sample_offset as u64);
        let mut pilot_events: Vec<(u64, i16)> = Vec::new();
        let mut all_events: Vec<HrpdTrafficEvent> = Vec::new();
        let mut last_event_check = Instant::now();
        let stream_start = Instant::now();
        let mut total_samples_pushed: usize = 0;

        loop {
            let mut block: Vec<Complex32> = Vec::with_capacity(SAMPLES_PER_BLOCK);
            for _ in 0..SAMPLES_PER_BLOCK {
                let Some(i) = samples_iter.next() else { break };
                let Some(q) = samples_iter.next() else { break };
                let i = i.unwrap_or(0) as f32 / i16::MAX as f32;
                let q = q.unwrap_or(0) as f32 / i16::MAX as f32;
                block.push(Complex32::new(i, q));
            }
            if block.is_empty() {
                break;
            }
            let len = block.len();
            let hrpd_block =
                slice.process(block, raw_relative_sample_start, raw_absolute_sample_start);
            let blk = super::HrpdTrafficRxBlock {
                samples: hrpd_block.samples,
                absolute_sample_start: hrpd_block.absolute_sample_start,
                sample_rate_hz: hrpd_block.sample_rate_hz,
                rx_read_completed_at: Instant::now(),
                enqueue_time: Instant::now(),
            };
            if thread.tx.send(blk).is_err() {
                panic!("HRPD traffic worker channel disconnected");
            }
            raw_relative_sample_start = raw_relative_sample_start.saturating_add(len);
            raw_absolute_sample_start += len as u64;
            total_samples_pushed += len;

            // Drain whatever events have already arrived so we can short-
            // circuit once pilot is acquired.
            if last_event_check.elapsed() >= Duration::from_millis(50) {
                last_event_check = Instant::now();
                while let Ok(event) = event_rx.try_recv() {
                    if let HrpdTrafficEvent::ReversePilot {
                        absolute_chip,
                        snr_db_tenths,
                        ..
                    } = event
                    {
                        pilot_events.push((absolute_chip, snr_db_tenths));
                    }
                    all_events.push(event);
                }
            }
        }

        // Real-time processing ratio. The worker is lossless and is the
        // processing bottleneck, so the wall time from the first enqueue until
        // its mailbox fully drains is how long it took to process the entire
        // capture. Compare against the capture's own airtime. The capture is
        // pushed faster than realtime, so a backlog builds during the push and
        // this measures how fast the worker chews through it.
        let backlog_at_push_done = thread.tx.len();
        let process_drain_deadline = Instant::now() + Duration::from_secs(180);
        loop {
            // Keep the event mailbox bounded while we wait for the worker.
            while let Ok(event) = event_rx.try_recv() {
                if let HrpdTrafficEvent::ReversePilot {
                    absolute_chip,
                    snr_db_tenths,
                    ..
                } = event
                {
                    pilot_events.push((absolute_chip, snr_db_tenths));
                }
                all_events.push(event);
            }
            if thread.tx.is_empty() {
                // Let the final coalesced batch finish before declaring done.
                std::thread::sleep(Duration::from_millis(50));
                if thread.tx.is_empty() {
                    break;
                }
            }
            if Instant::now() >= process_drain_deadline {
                eprintln!(
                    "HRPD {}: WARNING worker did not drain within 180s (backlog={})",
                    fixture.label,
                    thread.tx.len()
                );
                break;
            }
            std::thread::sleep(Duration::from_millis(2));
        }
        let processing_wall = stream_start.elapsed();
        let capture_airtime_s = total_samples_pushed as f64 / fixture.sample_rate_hz as f64;
        let rt_ratio = capture_airtime_s / processing_wall.as_secs_f64().max(1e-9);
        eprintln!(
            "HRPD {}: REAL-TIME RATIO capture_airtime={:.3}s processing_wall={:.3}s rt_ratio={:.2}x (>1 = faster than realtime) backlog_at_push_done={} blocks",
            fixture.label,
            capture_airtime_s,
            processing_wall.as_secs_f64(),
            rt_ratio,
            backlog_at_push_done,
        );
        const MIN_REALTIME_RATIO: f64 = 5.0;
        const CI_MIN_REALTIME_RATIO: f64 = 4.0;
        let min_realtime_ratio = if std::env::var_os("CI").is_some() {
            CI_MIN_REALTIME_RATIO
        } else {
            MIN_REALTIME_RATIO
        };
        assert!(
            rt_ratio >= min_realtime_ratio,
            "HRPD {}: reverse-traffic RX below the {min_realtime_ratio:.2}x realtime floor (rt_ratio={rt_ratio:.2}x, airtime={capture_airtime_s:.3}s wall={:.3}s)",
            fixture.label,
            processing_wall.as_secs_f64(),
        );

        // Let the unbounded worker mailbox drain until the receiver has
        // proved traffic-pilot lock and is decoding continuous reverse DRC.
        // The capture is pushed faster than realtime in this test; production
        // SDR input arrives incrementally.
        let drain_deadline = Instant::now() + Duration::from_secs(25);
        loop {
            while let Ok(event) = event_rx.try_recv() {
                if let HrpdTrafficEvent::ReversePilot {
                    absolute_chip,
                    snr_db_tenths,
                    ..
                } = event
                {
                    pilot_events.push((absolute_chip, snr_db_tenths));
                }
                all_events.push(event);
            }
            let drc_events = all_events
                .iter()
                .filter(|event| matches!(event, HrpdTrafficEvent::Drc { .. }))
                .count();
            if !pilot_events.is_empty() && drc_events >= fixture.min_drc_events {
                break;
            }
            if Instant::now() >= drain_deadline {
                break;
            }
            std::thread::sleep(Duration::from_millis(50));
        }

        shutdown.store(true, Ordering::Relaxed);

        eprintln!(
            "HRPD {}: streamed {} samples in {:.1}s, pilot_events={}",
            fixture.label,
            total_samples_pushed,
            stream_start.elapsed().as_secs_f64(),
            pilot_events.len()
        );
        for (chip, snr) in &pilot_events {
            eprintln!(
                "  ReversePilot chip={chip} snr={:.1} dB",
                *snr as f32 / 10.0
            );
        }
        let mut pilot_count = 0usize;
        let mut drc_count = 0usize;
        let mut ack_count = 0usize;
        let mut stream0_count = 0usize;
        let mut stream1_count = 0usize;
        for ev in &all_events {
            match ev {
                HrpdTrafficEvent::ReversePilot { .. } => pilot_count += 1,
                HrpdTrafficEvent::ReversePilotLost { .. } => {}
                HrpdTrafficEvent::Drc { .. } => drc_count += 1,
                HrpdTrafficEvent::Ack { .. } => ack_count += 1,
                HrpdTrafficEvent::Stream0Signaling { .. } => stream0_count += 1,
                HrpdTrafficEvent::Stream1Packet { .. } => stream1_count += 1,
            }
        }
        eprintln!(
            "HRPD {}: total events: pilot={pilot_count} drc={drc_count} ack={ack_count} stream0={stream0_count} stream1={stream1_count}",
            fixture.label
        );
        let mut stream0_payloads: Vec<&[u8]> = Vec::new();
        let mut stream1_payloads: Vec<&[u8]> = Vec::new();
        for ev in &all_events {
            match ev {
                HrpdTrafficEvent::Stream0Signaling { payload, .. } => {
                    eprintln!(
                        "  stream0 len={} {:02x?}",
                        payload.len(),
                        &payload[..payload.len().min(16)]
                    );
                    stream0_payloads.push(payload);
                }
                HrpdTrafficEvent::Stream1Packet { payload, .. } => {
                    eprintln!(
                        "  stream1 len={} {:02x?}",
                        payload.len(),
                        &payload[..payload.len().min(16)]
                    );
                    stream1_payloads.push(payload);
                }
                _ => {}
            }
        }
        if let Some(expected) = fixture.expected_stream0_events {
            assert_eq!(
                stream0_count, expected,
                "{}: decoded Stream 0 signaling event count changed",
                fixture.label
            );
        }
        if let Some(expected) = fixture.expected_stream1_events {
            assert_eq!(
                stream1_count, expected,
                "{}: decoded Stream 1 packet event count changed",
                fixture.label
            );
        }
        for expected in fixture.expected_stream0_payloads {
            assert!(
                stream0_payloads.iter().any(|p| p == expected),
                "{}: expected Stream 0 payload {expected:02x?} not decoded",
                fixture.label
            );
        }
        for prefix in fixture.expected_stream1_payload_prefixes {
            assert!(
                stream1_payloads.iter().any(|p| p.starts_with(prefix)),
                "{}: expected Stream 1 payload prefix {prefix:02x?} not decoded",
                fixture.label
            );
        }
        // DRC histogram across the whole capture. The post-RPC capture shows
        // the AT requesting DRC 0x3 (153.6 kbps) almost exclusively, with a
        // handful of garbage decodes during early frames before pilot is
        // fully locked.
        let mut drc_hist = [0usize; 16];
        for ev in &all_events {
            if let HrpdTrafficEvent::Drc { drc_index, .. } = ev {
                if (*drc_index as usize) < drc_hist.len() {
                    drc_hist[*drc_index as usize] += 1;
                }
            }
        }
        for (idx, count) in drc_hist.iter().enumerate() {
            if *count > 0 {
                eprintln!("  drc_index=0x{idx:x} count={count}");
            }
        }

        if let Some(metrics_handle) =
            crate::receiver::hrpd::reverse_correlator_base::get_metrics_handle("hrpd_traffic")
        {
            let metrics = metrics_handle.lock().expect("metrics mutex");
            eprintln!(
                "HRPD {}: metrics per_block={}us append={}us fft={}us(n={}) spawn={}us(n={})",
                fixture.label,
                metrics.per_block_avg_us(),
                metrics.append_block_avg_us(),
                metrics.fft_scan_avg_us(),
                metrics.fft_scan_calls,
                metrics.spawn_finger_avg_us(),
                metrics.spawn_finger_calls,
            );
            let total = metrics.searcher_total_ns().max(1);
            let pct = |ns: u64| ns.saturating_mul(100) / total;
            eprintln!(
                "HRPD {}: fft_stages(scan_n={}) ref_setup={}us({}%,miss={}) signal_fft={}us({}%) ifft_mult={}us({}%) peak_find={}us({}%)",
                fixture.label,
                metrics.searcher_scan_window_calls,
                metrics.ref_setup_avg_us(),
                pct(metrics.searcher_ref_setup_ns),
                metrics.searcher_ref_setup_calls,
                metrics.signal_fft_avg_us(),
                pct(metrics.searcher_signal_fft_ns),
                metrics.ifft_mult_avg_us(),
                pct(metrics.searcher_ifft_mult_ns),
                metrics.peak_find_avg_us(),
                pct(metrics.searcher_peak_find_ns),
            );
        }

        assert!(
            !pilot_events.is_empty(),
            "expected at least one HrpdTrafficEvent::ReversePilot for UATI 0x{:08x} in {}",
            fixture.expected_uati,
            fixture.label
        );
        // ReversePilot is emitted by the traffic finger only after the FFT
        // hit is verified by a coherent, spec-derived pilot despread. The
        // event SNR field is a coarse diagnostic and is not used as the lock
        // gate for this capture.

        // Lock in the traffic correlator's accumulated timing counters as a
        // regression guard. Budgets at ~2x measured. If the worker has been
        // running long enough to register the metrics handle (it always
        // should be by this point), assert the per-section budgets.
        if let Some(metrics_handle) =
            crate::receiver::hrpd::reverse_correlator_base::get_metrics_handle("hrpd_traffic")
        {
            let metrics = metrics_handle.lock().expect("metrics mutex");
            let per_block_us = metrics.per_block_avg_us();
            let fft_scan_us = metrics.fft_scan_avg_us();
            let spawn_us = metrics.spawn_finger_avg_us();
            let append_us = metrics.append_block_avg_us();
            eprintln!(
                "HRPD {} correlator timing: per_block_avg={}us append_avg={}us fft_scan_avg={}us(n={}) spawn_avg={}us(n={})",
                fixture.label,
                per_block_us,
                append_us,
                fft_scan_us,
                metrics.fft_scan_calls,
                spawn_us,
                metrics.spawn_finger_calls,
            );
            assert!(
                per_block_us < 200_000,
                "hrpd_traffic per_block_avg too slow: {per_block_us}us (budget 200000us)",
            );
            assert!(
                fft_scan_us < 200_000,
                "hrpd_traffic fft_scan_avg too slow: {fft_scan_us}us (budget 200000us)",
            );
            assert!(
                spawn_us < 500_000,
                "hrpd_traffic spawn_finger_avg too slow: {spawn_us}us (budget 500000us)",
            );
        }
    }

    /// Replay the full composite IQ capture through the HRPD reverse-access
    /// pipeline and assert every valid access retry in the WAV is recovered.
    #[test]
    fn capture_hrpd_reverse_access_1801219902363798_recovers_all_bursts() {
        let wav_path: PathBuf = [
            env!("CARGO_MANIFEST_DIR"),
            "..",
            "..",
            "test",
            "capture",
            "1801219902363798.wav",
        ]
        .iter()
        .collect();
        // The live log only showed the packets that reached the access worker
        // while no reverse traffic pilot was locked. Replaying the full WAV
        // shows all valid access retries present in the capture.
        drive_hrpd_access_worker_capture(HrpdAccessCaptureFixture {
            label: "1801219902363798-access",
            wav_path,
            sample_rate_hz: 9_830_400,
            chip_rate_hz: 1_228_800,
            first_absolute_sample_start: 14_409_759_218_910_385,
            hrpd_rx_shift_hz: -2_205_000,
            expected_packet_chips: &[
                1_801_219_906_797_568,
                1_801_219_907_256_320,
                1_801_219_907_584_000,
                1_801_219_910_336_512,
                1_801_219_910_926_336,
                1_801_219_911_516_160,
                1_801_219_912_007_680,
                1_801_219_912_564_736,
                1_801_219_913_121_792,
                1_801_219_913_711_616,
                1_801_219_914_203_136,
                1_801_219_914_694_656,
                1_801_219_915_153_408,
                1_801_219_915_677_696,
                1_801_219_916_136_448,
                1_801_219_919_773_696,
                1_801_219_920_199_680,
                1_801_219_920_658_432,
            ],
            expected_message_counts: &[2, 2, 3, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 4, 2, 2, 2],
        });
    }

    /// Rev A packet-data session capture `1803459647969769.wav` (UATI
    /// 0x1a1e58d5): recover the reverse HRPD access bursts present in the WAV
    /// before decoding the subtype-2 traffic frames. Metadata sidecar
    /// `1803459647969769.json`.
    #[test]
    fn capture_hrpd_reverse_access_reva_traffic_session() {
        let wav_path: PathBuf = [
            env!("CARGO_MANIFEST_DIR"),
            "..",
            "..",
            "test",
            "capture",
            "1803459647969769.wav",
        ]
        .iter()
        .collect();
        drive_hrpd_access_worker_capture(HrpdAccessCaptureFixture {
            label: "1803459647969769-reva-access",
            wav_path,
            sample_rate_hz: 9_830_400,
            chip_rate_hz: 1_228_800,
            first_absolute_sample_start: 14_427_677_183_758_154,
            hrpd_rx_shift_hz: -2_205_000,
            // The ConnectionRequest access probe that opens the Rev A traffic
            // connection is the only access burst inside this capture window;
            // the earlier session-setup probes predate the capture start.
            expected_packet_chips: &[1_803_459_652_747_264],
            expected_message_counts: &[2],
        });
    }

    /// Rev A packet-data session `1803459647969769.wav` (UATI 0x1a1e58d5),
    /// subtype-2 physical layer / RTC MAC subtype 3, MAC=6. The reverse pilot
    /// acquired at chip 1803459652911104 (~39.5M samples in) with mask q-/p1;
    /// `start_sample_offset` skips the earlier subtype-0 (MAC=5) connection on
    /// the same UATI/long code so the worker locks the Rev A connection.
    #[test]
    fn capture_hrpd_reverse_traffic_reva_traffic_session() {
        let wav_path: PathBuf = [
            env!("CARGO_MANIFEST_DIR"),
            "..",
            "..",
            "test",
            "capture",
            "1803459647969769.wav",
        ]
        .iter()
        .collect();
        drive_hrpd_reverse_traffic_worker_capture(HrpdTrafficCaptureFixture {
            label: "1803459647969769-reva-traffic",
            wav_path,
            sample_rate_hz: 9_830_400,
            chip_rate_hz: 1_228_800,
            first_absolute_sample_start: 14_427_677_183_758_154,
            start_sample_offset: 39_000_000,
            hrpd_rx_shift_hz: -2_205_000,
            expected_uati: 0x1a1e_58d5,
            mac_index: 6,
            min_drc_events: 200,
            physical_layer_subtype: 2,
            reverse_traffic_mac_subtype: 3,
            // Locked baseline for the fixed subtype-2 RRI: 50 signaling and
            // 38 data-stream packets decode from this session, including the
            // AT's TrafficChannelComplete, a RouteUpdate, and the PPP LCP
            // Configure-Request train.
            expected_stream0_events: Some(50),
            expected_stream1_events: Some(38),
            expected_stream0_payloads: &[
                // SLP-D reliable, Route Update (0x0e), TrafficChannelComplete.
                &[0x01, 0x88, 0x0e, 0x02, 0x00],
                // SLP-D reliable, Route Update (0x0e), RouteUpdate message.
                &[0x01, 0x09, 0x0e, 0x00, 0x01, 0x00, 0x01, 0x00],
            ],
            expected_stream1_payload_prefixes: &[
                // HDLC flag + PPP LCP (0xc021) Configure-Request.
                &[0x7e, 0xff, 0x7d, 0x23, 0xc0, 0x21],
            ],
        });
    }

    #[test]
    fn capture_hrpd_reverse_traffic_1800274243299352() {
        let wav_path: PathBuf = [
            env!("CARGO_MANIFEST_DIR"),
            "..",
            "..",
            "test",
            "capture",
            "1800274243299352.wav",
        ]
        .iter()
        .collect();
        drive_hrpd_reverse_traffic_worker_capture(HrpdTrafficCaptureFixture {
            label: "1800274243299352",
            wav_path: wav_path.clone(),
            sample_rate_hz: 9_830_400,
            chip_rate_hz: 1_228_800,
            first_absolute_sample_start: 14_402_193_946_394_817,
            start_sample_offset: 0,
            hrpd_rx_shift_hz: -2_205_000,
            expected_uati: 0x1a05_8001,
            mac_index: 5,
            min_drc_events: 2,
            physical_layer_subtype: 0,
            reverse_traffic_mac_subtype: 0,
            ..Default::default()
        });
        drive_hrpd_reverse_traffic_worker_capture(HrpdTrafficCaptureFixture {
            label: "1800274243299352-second-assignment-window",
            wav_path,
            sample_rate_hz: 9_830_400,
            chip_rate_hz: 1_228_800,
            first_absolute_sample_start: 14_402_193_946_394_817,
            start_sample_offset: 10_000_000,
            hrpd_rx_shift_hz: -2_205_000,
            expected_uati: 0x1a05_8001,
            mac_index: 5,
            min_drc_events: 2,
            physical_layer_subtype: 0,
            reverse_traffic_mac_subtype: 0,
            ..Default::default()
        });
    }

    /// 2026-06-10 bring-up capture with three consecutive reverse traffic
    /// sessions (MAC 5/6/7) at very high RX level (~95% full scale, ~2% of
    /// samples at the limiter ceiling). The live run detected the reverse
    /// pilot via FFT each frame but every finger stayed below the per-frame
    /// coherence validation gate and pruned unvalidated.
    #[test]
    fn capture_hrpd_reverse_traffic_1800354308350520() {
        let wav_path: PathBuf = [
            env!("CARGO_MANIFEST_DIR"),
            "..",
            "..",
            "test",
            "capture",
            "1800354308350520.wav",
        ]
        .iter()
        .collect();
        // Session windows (offsets from capture start at 13:08:06.87):
        //   MAC 5 / UATI 0x1a058001: ~2.9 s .. 9.3 s
        //   MAC 6 / UATI 0x1a058002: ~9.3 s .. 12.8 s
        //   MAC 7 / UATI 0x1a058003: ~12.8 s .. 19.0 s
        for (label, uati, mac_index, start_seconds) in [
            ("1800354308350520-m5", 0x1a05_8001u32, 5u8, 2.5f64),
            ("1800354308350520-m6", 0x1a05_8002, 6, 9.4),
            ("1800354308350520-m7", 0x1a05_8003, 7, 12.9),
        ] {
            let start_sample_offset = (start_seconds * 9_830_400.0) as usize;
            drive_hrpd_reverse_traffic_worker_capture(HrpdTrafficCaptureFixture {
                label,
                wav_path: wav_path.clone(),
                sample_rate_hz: 9_830_400,
                chip_rate_hz: 1_228_800,
                first_absolute_sample_start: 14_402_834_466_804_165,
                start_sample_offset,
                hrpd_rx_shift_hz: -2_205_000,
                expected_uati: uati,
                mac_index,
                min_drc_events: 1,
                physical_layer_subtype: 0,
                reverse_traffic_mac_subtype: 0,
                ..Default::default()
            });
        }
    }
}
