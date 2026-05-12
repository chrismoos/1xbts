use std::{
    fs,
    io::BufWriter,
    path::PathBuf,
    sync::Arc,
    sync::atomic::{AtomicBool, AtomicU64, Ordering},
    sync::mpsc,
    time::Instant,
};

use cdma_abis::{
    bearer::{ChannelFamily, Direction, FrameContent, ReverseFchDcchFrame, TrafficFrame},
    udp_bearer::UdpBearerDatagram,
};
use cdma_common::{
    bits::Bitstream,
    diagnostics::{power_control_verbose_enabled_for_walsh, power_control_verbose_summary_every},
    error::Error,
    paging::{imsi_11_12_to_digits, imsi_s_to_digits_checked, mcc_to_digits},
    time,
};
use hound::{SampleFormat, WavSpec, WavWriter};
use log::{debug, info, trace, warn};
use num::complex::Complex32;
use serde::Serialize;
use tokio::sync::{mpsc as tokio_mpsc, oneshot};

use crate::lac::message_types::MessageId;
use crate::receiver::{
    access_layer3::{AccessMessage, AccessMessageHeader, RdschPdu, access_message_type_name},
    access_pdu::ReverseAccessPdu,
    pipelined::{
        PipelineEmitter, PipelineProcessorShared, ReverseAccessSettings, SampleBlock, VecEmitter,
        flush_sub_chain, reverse_access_chain, run_sub_chain,
    },
};

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
    debug!(
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

/// Handle to a running traffic RX thread.
struct TrafficRxThread {
    walsh_code: u8,
    tx: mpsc::Sender<TrafficRxBlock>,
    shutdown: Arc<AtomicBool>,
}

const PCG_CHIPS: usize = 1536;
const WAV_CAPTURE_PEAK: f32 = 0.95;

fn rx_target_batch_samples(sample_rate_hz: usize, chip_rate_hz: usize, batch_pcgs: usize) -> usize {
    let oversample = (sample_rate_hz / chip_rate_hz.max(1)).max(1);
    oversample.saturating_mul(PCG_CHIPS * batch_pcgs.max(1))
}

fn spawn_traffic_rx_thread(
    oversample: usize,
    walsh_code: u8,
    esn: u32,
    preamble_num_pcgs: Option<usize>,
    use_rc3: bool,
    rev_fch_gating_mode: bool,
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
    /// Deferred capture stop: the StopCapture command sets this so the
    /// capture continues until the next RX buffer is written, preventing
    /// truncation of samples already buffered in the reader channel.
    pending_capture_stop: Option<oneshot::Sender<Result<IqCaptureControlResult, String>>>,
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
        "rx: sample_rate_hz={} chip_rate_hz={} capture=<ui-driven> hw_start_ns={:?} absolute_chip_start={:?} rx_sample_delay={}",
        rx.sample_rate_hz,
        rx.chip_rate_hz,
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
    let absolute_chip_origin = rx.absolute_chip_start;
    let absolute_sample_origin = absolute_chip_origin.saturating_mul(oversample as u64);
    let capture_writer = None;
    let capture_target_samples = None;
    let processors = reverse_access_chain(ReverseAccessSettings {
        oversample,
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
    info!(
        "rx: stream already active from prime (oversample={} absolute_chip_origin={} absolute_sample_origin={})",
        oversample, absolute_chip_origin, absolute_sample_origin
    );

    Ok(RxRuntime {
        config: rx,
        stage_timings: processors
            .iter()
            .map(|p| StageTiming {
                name: p.name(),
                total_us: 0,
                calls: 0,
                max_us: 0,
            })
            .collect(),
        processors,
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
        pending_capture_stop: None,
    })
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
    let bearer_bts_id = runtime.config.base_id as u32;
    let bearer_cell_id: u32 = 1;
    let target_batch_samples = rx_target_batch_samples(
        runtime.config.sample_rate_hz,
        runtime.config.chip_rate_hz,
        runtime.config.rx_batch_pcgs,
    );
    let target_batch_ms =
        target_batch_samples as f64 * 1000.0 / runtime.config.sample_rate_hz.max(1) as f64;
    info!(
        "rx: live SDR target_batch_samples={} target_batch_ms={:.3}",
        target_batch_samples, target_batch_ms
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
            let mut last_end_ticks: Option<u64> = None;
            let mut overflow_count: u64 = 0;
            while !shutdown_reader.load(Ordering::Relaxed) {
                let since_last = last_read.elapsed();
                let result = radio_rx.rx_read(&mut buffer, 250_000)?;
                last_read = Instant::now();
                if result.overflow {
                    overflow_count += 1;
                    let gap_us = since_last.as_micros() as u64;
                    if let Some(prev_end) = last_end_ticks {
                        let gap_ticks = result.time_ticks.saturating_sub(prev_end);
                        let gap_samples = (gap_ticks as u128 * sample_rate_hz as u128
                            / tick_rate.max(1) as u128)
                            as usize;
                        if gap_samples > 2 {
                            log::warn!(
                                "rx: SDR overflow #{} — zero-filling {} samples \
                                 ({:.3} ms). read_count={} gap_since_last_read={}us \
                                 max_gap={}us",
                                overflow_count,
                                gap_samples,
                                gap_samples as f64 * 1000.0 / sample_rate_hz.max(1) as f64,
                                read_count,
                                gap_us,
                                max_gap_us,
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
                    } else {
                        log::warn!(
                            "rx: SDR overflow #{} on first read — \
                             cannot zero-fill (no prior timestamp). \
                             read_count={} gap_since_last_read={}us",
                            overflow_count,
                            read_count,
                            gap_us,
                        );
                    }
                }
                let n = result.samples_read;
                if n == 0 {
                    continue;
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
    // Subtract the calibrated RX pipeline delay so a received sample is
    // labeled with the absolute sample number at which it was transmitted.
    let elapsed_samples =
        raw_elapsed_samples.saturating_sub(runtime.config.rx_sample_delay as u128) as u64;
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
    let capture_start = Instant::now();
    maybe_write_capture(runtime, &msg.samples)?;
    let capture_us = capture_start.elapsed().as_micros() as u64;

    let relative_sample_start = runtime.next_sample_index;

    // Clone samples for traffic RX threads if any are active or pending.
    let has_traffic = !traffic_threads.is_empty()
        || runtime
            .config
            .traffic_rx_pool
            .as_ref()
            .is_some_and(|p| !p.lock().is_empty());
    let traffic_block_msg = if has_traffic {
        Some(TrafficRxBlock {
            samples: msg.samples.clone(),
            relative_sample_start,
            absolute_chip_start,
            absolute_sample_start,
            sample_rate_hz: runtime.config.sample_rate_hz,
            hw_time_ns: msg.time_ns,
            enqueue_time: std::time::Instant::now(),
        })
    } else {
        None
    };

    let mut block = SampleBlock::new(msg.samples, relative_sample_start)
        .with_sample_rate_hz(runtime.config.sample_rate_hz as f64);
    block
        .tags
        .insert("absolute_chip_start", absolute_chip_start as i64);
    block
        .tags
        .insert("absolute_sample_start", absolute_sample_start as i64);
    runtime.next_sample_index = relative_sample_start.saturating_add(n);

    let pipeline_start = Instant::now();
    let mut access_emitter = VecEmitter::new();
    let mut outputs = run_sub_chain_timed(
        &mut runtime.processors,
        block,
        &mut runtime.stage_timings,
        &mut access_emitter,
    );
    outputs.extend(access_emitter.blocks);
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
    if let Some(ref removals) = runtime.config.traffic_rx_removals {
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
    if let Some(ref pool) = runtime.config.traffic_rx_pool {
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
                oversample,
                req.walsh_code,
                req.esn,
                req.preamble_num_pcgs,
                use_rc3,
                req.rev_fch_gating_mode,
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
        let top_stage = runtime
            .stage_timings
            .iter()
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
        debug!(
            "rx_hardware_heartbeat: hw_time_ns={} absolute_chip_start={} t20={} abs_sample_start={}",
            runtime.last_hardware_time_ns,
            runtime.last_absolute_chip_start,
            time::system_time_20ms_frames(time::system_time_from_chips(
                runtime.last_absolute_chip_start,
                runtime.config.chip_rate_hz as u64
            )),
            runtime.last_absolute_sample_start
        );
        debug!(
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
            debug!(
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
                debug!(
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
        for stage in &mut runtime.stage_timings {
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
                debug!(
                    "rx_traffic[w{}]: PREAMBLE DETECTED pcgs={} abs_chip={}",
                    walsh_code, preamble_pcgs, abs_chip
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
            write_capture_metadata(runtime, &active)?;
            respond_pending_capture_start(runtime, &active, pending);
        }
    }
    if runtime.capture_writer.is_none() {
        return Ok(());
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
                ea.imsi_m_s1,
                ea.imsi_m_s2,
                ea.imsi_class,
                ea.imsi_addr_num,
                ea.mcc,
                ea.imsi_11_12,
            )
        }
        _ => (
            None, None, false, false, None, None, None, None, None, None, None, None,
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
        } else if is_signaling || primary_rate >= 4800 {
            tail_valid && event.traffic_phy_valid.unwrap_or(true)
        } else {
            tail_valid && event.traffic_ml_tail_match.unwrap_or(true)
        }
    } else if is_signaling || primary_rate >= 4800 {
        event.traffic_phy_valid.unwrap_or(true)
    } else {
        event.traffic_ml_tail_match.unwrap_or(true)
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

#[allow(dead_code)]
fn extract_access_rc_preferences(msg: &AccessMessage) -> (Option<u8>, Option<u8>, Option<bool>) {
    match msg {
        AccessMessage::Origination(m) => (m.for_rc_pref, m.rev_rc_pref, m.rev_fch_gating_req),
        AccessMessage::PageResponse(m) => (m.for_rc_pref, m.rev_rc_pref, m.rev_fch_gating_req),
        _ => (None, None, None),
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
                Some(0b010) if addr.msid_raw.len() >= 32 => {
                    // IMSI+ESN+MEID: ESN(32) + MEID(56) + IMSI
                    let mut bits = addr.msid_raw.clone();
                    esn = bits.read_bits(32).ok().map(|v| v as u32);
                    let _ = bits.read_bits(56); // skip MEID
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
                    let _ = bits.read_bits(56); // skip MEID
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
    for sample in samples {
        let re = (sample.re * WAV_CAPTURE_PEAK).clamp(-1.0, 1.0);
        let im = (sample.im * WAV_CAPTURE_PEAK).clamp(-1.0, 1.0);
        wav.write_sample((re * i16::MAX as f32) as i16)?;
        wav.write_sample((im * i16::MAX as f32) as i16)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{
        build_access_event, build_traffic_event, build_traffic_voice_event,
        extract_access_rc_preferences, extract_addressing_fields, extract_imsi_from_class_fields,
        reconcile_traffic_stream_continuity, reverse_frame_content_from_rate_bps,
    };
    use crate::lac::message_types::{MessageId, WireChannel};
    use crate::receiver::access_layer3::AccessMessage;
    use crate::receiver::access_pdu::RcschAddressingFields;
    use crate::receiver::pipelined::SampleBlock;
    use cdma_common::bits::Bitstream;
    use num_complex::Complex32;
    use std::collections::HashMap;

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
}
