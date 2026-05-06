use std::{
    fs,
    io::BufWriter,
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
};

use cdma_common::{error::Error, time};
use hound::{SampleFormat, WavSpec, WavWriter};
use log::{info, warn};
use num::complex::Complex32;
use serde::Serialize;
use tokio::sync::{mpsc as tokio_mpsc, oneshot};

use super::super::{BtsCommand, IqCaptureControlResult, IqCaptureStatus};
use super::RxRuntime;

pub(super) const WAV_CAPTURE_PEAK: f32 = 0.95;

pub(super) struct PendingCaptureStart {
    pub(super) directory: PathBuf,
    pub(super) respond_to: oneshot::Sender<Result<IqCaptureControlResult, String>>,
}

#[derive(Clone, Debug)]
pub(super) struct ActiveCapture {
    pub(super) directory: PathBuf,
    pub(super) wav_path: PathBuf,
    pub(super) metadata_path: PathBuf,
    pub(super) first_absolute_chip_start: u64,
    pub(super) first_absolute_sample_start: u64,
    pub(super) first_sample_system_time: time::CdmaSystemTime,
    pub(super) first_hardware_time_ns: u64,
}

#[derive(Serialize)]
pub(super) struct CaptureMetadataFile {
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

pub(super) fn capture_status_from_active(
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

pub(super) fn write_capture_metadata(
    runtime: &RxRuntime,
    active: &ActiveCapture,
) -> Result<(), Error> {
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

pub(super) fn respond_pending_capture_start(
    runtime: &RxRuntime,
    active: &ActiveCapture,
    pending: PendingCaptureStart,
) {
    let _ = pending.respond_to.send(Ok(IqCaptureControlResult {
        status: capture_status_from_active(runtime, active, true),
        message: format!("IQ capture started: {}", active.wav_path.display()),
    }));
}

pub(super) fn idle_capture_status(runtime: &RxRuntime, directory: PathBuf) -> IqCaptureStatus {
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

pub(super) fn cancel_pending_capture_start(runtime: &mut RxRuntime, reason: &str) {
    if let Some(pending) = runtime.pending_capture_start.take() {
        let _ = pending.respond_to.send(Err(reason.to_string()));
    }
}

pub(super) fn stop_active_capture(
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

pub(super) fn handle_bts_command(
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

pub(super) fn drain_bts_commands(
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

pub(super) fn finalize_capture(runtime: &mut RxRuntime) {
    cancel_pending_capture_start(runtime, "RX loop stopped before IQ capture could start");
    if let Err(err) = stop_active_capture(runtime, "RX loop shutting down") {
        warn!("rx: capture finalize error: {}", err);
    }
}

pub(super) fn maybe_write_capture(
    runtime: &mut RxRuntime,
    samples: &[Complex32],
) -> Result<(), Error> {
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

pub(super) fn create_capture_writer(
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

pub(super) fn write_capture_block(
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
