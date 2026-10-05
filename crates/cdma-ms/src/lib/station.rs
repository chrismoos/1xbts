use std::fs::File;
use std::io::BufWriter;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, SyncSender, TryRecvError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use num_complex::Complex32;
use tokio::sync::broadcast;

use crate::engine::{Diagnostics, Engine, EngineConfig, ScanConfig};
use crate::event::EventSink;
use crate::ms::MsEvent;
use crate::radio::Radio;

const QUERY_TIMEOUT: Duration = Duration::from_secs(5);
const DUMP_TARGET_PEAK: f32 = 0.9;

enum Command {
    PowerOn,
    PowerOff,
    Register,
    Originate {
        service_option: u16,
        digits: String,
    },
    SendDtmfBurst {
        burst: crate::traffic::DtmfBurst,
        done: SyncSender<Result<(), String>>,
    },
    HangUp,
    Answer,
    VoicePcm(Box<[i16; cdma_voice::SAMPLES_PER_FRAME]>),
    OriginateSms {
        destination: String,
        text: String,
    },
    AddSink(Arc<dyn EventSink>),
    SetScan(Option<ScanConfig>),
    Diagnostics(SyncSender<Diagnostics>),
    SetRxGain {
        gain_db: f64,
        done: SyncSender<Result<(), String>>,
    },
    DumpForward {
        path: PathBuf,
        seconds: f64,
        done: SyncSender<Result<(), String>>,
    },
    TrimTxCalibration {
        trim: crate::radio::TxTrim,
        done: SyncSender<Result<Option<crate::radio::TxCalibration>, String>>,
    },
    Shutdown,
}

#[derive(Default)]
pub struct StationCounters {
    samples_fed: AtomicU64,
    rx_overflows: AtomicU64,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct StationStats {
    pub samples_fed: u64,
    pub rx_overflows: u64,
    pub uptime_secs: f64,
    /// Forward samples consumed per second of wall time, over the sample
    /// rate. Above 1 means the engine keeps up with real time.
    pub real_time_ratio: f64,
}

pub struct MobileStation {
    cmd_tx: mpsc::Sender<Command>,
    thread: Option<JoinHandle<()>>,
    counters: Arc<StationCounters>,
    sample_rate_hz: f64,
    started: Instant,
    voice_output: broadcast::Sender<[i16; cdma_voice::SAMPLES_PER_FRAME]>,
}

impl MobileStation {
    pub fn start(radio: Box<dyn Radio>, config: EngineConfig) -> MobileStation {
        let (cmd_tx, cmd_rx) = mpsc::channel();
        let sample_rate_hz = config.sample_rate_hz;
        let engine = Engine::new(config);
        let (voice_output, _keep) = broadcast::channel(64);
        let driver_voice_output = voice_output.clone();
        let counters = Arc::new(StationCounters::default());
        let thread_counters = counters.clone();
        let thread = thread::Builder::new()
            .name("cdma-ms".into())
            .spawn(move || driver_loop(radio, engine, cmd_rx, thread_counters, driver_voice_output))
            .expect("spawn cdma-ms driver thread");
        MobileStation {
            cmd_tx,
            thread: Some(thread),
            counters,
            sample_rate_hz,
            started: Instant::now(),
            voice_output,
        }
    }

    pub fn power_on(&self) {
        let _ = self.cmd_tx.send(Command::PowerOn);
    }

    pub fn power_off(&self) {
        let _ = self.cmd_tx.send(Command::PowerOff);
    }

    pub fn register(&self) {
        let _ = self.cmd_tx.send(Command::Register);
    }

    pub fn originate(&self, service_option: u16, digits: String) {
        let _ = self.cmd_tx.send(Command::Originate {
            service_option,
            digits,
        });
    }

    pub fn send_dtmf_burst(&self, burst: crate::traffic::DtmfBurst) -> Result<(), String> {
        let (done, reply) = mpsc::sync_channel(1);
        self.cmd_tx
            .send(Command::SendDtmfBurst { burst, done })
            .map_err(|_| "station is not running".to_string())?;
        reply
            .recv_timeout(QUERY_TIMEOUT)
            .map_err(|_| "station did not answer".to_string())?
    }

    pub fn hang_up(&self) {
        let _ = self.cmd_tx.send(Command::HangUp);
    }

    pub fn answer(&self) {
        let _ = self.cmd_tx.send(Command::Answer);
    }

    pub fn push_voice_pcm(&self, pcm: [i16; cdma_voice::SAMPLES_PER_FRAME]) {
        let _ = self.cmd_tx.send(Command::VoicePcm(Box::new(pcm)));
    }

    pub fn subscribe_voice(&self) -> broadcast::Receiver<[i16; cdma_voice::SAMPLES_PER_FRAME]> {
        self.voice_output.subscribe()
    }

    pub fn originate_sms(&self, destination: String, text: String) {
        let _ = self
            .cmd_tx
            .send(Command::OriginateSms { destination, text });
    }

    pub fn add_event_sink(&self, sink: Arc<dyn EventSink>) {
        let _ = self.cmd_tx.send(Command::AddSink(sink));
    }

    pub fn set_scan(&self, scan: Option<ScanConfig>) {
        let _ = self.cmd_tx.send(Command::SetScan(scan));
    }

    pub fn diagnostics(&self) -> Result<Diagnostics, String> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.cmd_tx
            .send(Command::Diagnostics(tx))
            .map_err(|_| "station is not running".to_string())?;
        rx.recv_timeout(QUERY_TIMEOUT)
            .map_err(|_| "station did not answer".to_string())
    }

    pub fn trim_tx_calibration(
        &self,
        trim: crate::radio::TxTrim,
    ) -> Result<Option<crate::radio::TxCalibration>, String> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.cmd_tx
            .send(Command::TrimTxCalibration { trim, done: tx })
            .map_err(|_| "station is not running".to_string())?;
        rx.recv_timeout(QUERY_TIMEOUT)
            .map_err(|_| "station did not answer".to_string())?
    }

    pub fn set_rx_gain(&self, gain_db: f64) -> Result<(), String> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.cmd_tx
            .send(Command::SetRxGain { gain_db, done: tx })
            .map_err(|_| "station is not running".to_string())?;
        rx.recv_timeout(QUERY_TIMEOUT)
            .map_err(|_| "station did not answer".to_string())?
    }

    pub fn dump_forward(&self, path: &Path, seconds: f64) -> Result<(), String> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.cmd_tx
            .send(Command::DumpForward {
                path: path.to_path_buf(),
                seconds,
                done: tx,
            })
            .map_err(|_| "station is not running".to_string())?;
        rx.recv_timeout(QUERY_TIMEOUT)
            .map_err(|_| "station did not answer".to_string())?
    }

    pub fn query_handle(&self) -> StationQuery {
        StationQuery {
            cmd_tx: self.cmd_tx.clone(),
        }
    }

    pub fn stats(&self) -> StationStats {
        let uptime = self.started.elapsed().as_secs_f64();
        let samples_fed = self.counters.samples_fed.load(Ordering::Relaxed);
        let air_secs = samples_fed as f64 / self.sample_rate_hz;
        StationStats {
            samples_fed,
            rx_overflows: self.counters.rx_overflows.load(Ordering::Relaxed),
            uptime_secs: uptime,
            real_time_ratio: if uptime > 0.0 { air_secs / uptime } else { 0.0 },
        }
    }

    pub fn shutdown(mut self) {
        self.stop();
    }

    fn stop(&mut self) {
        let _ = self.cmd_tx.send(Command::Shutdown);
        if let Some(t) = self.thread.take() {
            let _ = t.join();
        }
    }
}

#[derive(Clone)]
pub struct StationQuery {
    cmd_tx: mpsc::Sender<Command>,
}

impl StationQuery {
    pub fn diagnostics(&self) -> Result<Diagnostics, String> {
        let (tx, rx) = mpsc::sync_channel(1);
        self.cmd_tx
            .send(Command::Diagnostics(tx))
            .map_err(|_| "station is not running".to_string())?;
        rx.recv_timeout(QUERY_TIMEOUT)
            .map_err(|_| "station did not answer".to_string())
    }
}

impl Drop for MobileStation {
    fn drop(&mut self) {
        self.stop();
    }
}

struct Dump {
    path: PathBuf,
    writer: hound::WavWriter<BufWriter<File>>,
    remaining: u64,
    written: u64,
}

impl Dump {
    fn open(path: &Path, seconds: f64, sample_rate_hz: f64) -> Result<Self, String> {
        let spec = hound::WavSpec {
            channels: 2,
            sample_rate: sample_rate_hz as u32,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        let writer = hound::WavWriter::create(path, spec)
            .map_err(|e| format!("create {}: {}", path.display(), e))?;
        Ok(Dump {
            path: path.to_path_buf(),
            writer,
            remaining: (seconds * sample_rate_hz) as u64,
            written: 0,
        })
    }

    fn write(&mut self, samples: &[Complex32]) -> bool {
        let take = (self.remaining as usize).min(samples.len());
        for s in &samples[..take] {
            let re = (s.re * DUMP_TARGET_PEAK).clamp(-1.0, 1.0);
            let im = (s.im * DUMP_TARGET_PEAK).clamp(-1.0, 1.0);
            let _ = self.writer.write_sample((re * i16::MAX as f32) as i16);
            let _ = self.writer.write_sample((im * i16::MAX as f32) as i16);
        }
        self.remaining -= take as u64;
        self.written += take as u64;
        self.remaining == 0
    }
}

fn driver_loop(
    mut radio: Box<dyn Radio>,
    mut engine: Engine,
    cmd_rx: mpsc::Receiver<Command>,
    counters: Arc<StationCounters>,
    voice_output: broadcast::Sender<[i16; cdma_voice::SAMPLES_PER_FRAME]>,
) {
    if let Err(e) = radio.start() {
        log::error!("cdma-ms: radio failed to start: {}", e);
        return;
    }
    let sample_rate_hz = radio.sample_rate_hz();
    let mut buf = Vec::new();
    let mut exhausted_reported = false;
    let mut overflows_reported = 0u64;
    let mut slowest_forward = std::time::Duration::ZERO;
    let mut dump: Option<Dump> = None;
    loop {
        loop {
            match cmd_rx.try_recv() {
                Ok(Command::PowerOn) => engine.power_on(),
                Ok(Command::PowerOff) => engine.power_off(),
                Ok(Command::Register) => engine.register(),
                Ok(Command::Originate {
                    service_option,
                    digits,
                }) => engine.originate(service_option, digits),
                Ok(Command::SendDtmfBurst { burst, done }) => {
                    let _ = done.send(engine.send_dtmf_burst(burst));
                }
                Ok(Command::HangUp) => engine.hang_up(),
                Ok(Command::Answer) => {
                    engine.answer();
                }
                Ok(Command::VoicePcm(pcm)) => engine.push_voice_pcm(*pcm),
                Ok(Command::OriginateSms { destination, text }) => {
                    engine.originate_sms(destination, text)
                }
                Ok(Command::AddSink(sink)) => engine.add_event_sink(sink),
                Ok(Command::SetScan(scan)) => engine.set_scan(scan),
                Ok(Command::Diagnostics(reply)) => {
                    let _ = reply.send(engine.diagnostics());
                }
                Ok(Command::SetRxGain { gain_db, done }) => {
                    let _ = done.send(radio.set_rx_gain(gain_db).map_err(|e| e.to_string()));
                }
                Ok(Command::TrimTxCalibration { trim, done }) => {
                    let result = if trim == crate::radio::TxTrim::default() {
                        Ok(radio.tx_calibration())
                    } else {
                        radio
                            .set_tx_calibration(&trim)
                            .map(|()| radio.tx_calibration())
                            .map_err(|e| e.to_string())
                    };
                    let _ = done.send(result);
                }
                Ok(Command::DumpForward {
                    path,
                    seconds,
                    done,
                }) => {
                    let result = if dump.is_some() {
                        Err("a dump is already in progress".to_string())
                    } else {
                        Dump::open(&path, seconds, sample_rate_hz).map(|d| {
                            log::info!(
                                "cdma-ms: dumping {:.2} s of forward IQ to {}",
                                seconds,
                                path.display()
                            );
                            dump = Some(d);
                        })
                    };
                    let _ = done.send(result);
                }
                Ok(Command::Shutdown) | Err(TryRecvError::Disconnected) => {
                    radio.stop();
                    return;
                }
                Err(TryRecvError::Empty) => break,
            }
        }
        for pcm in engine.drain_voice_pcm() {
            let _ = voice_output.send(pcm);
        }

        if let Some(request) = engine.pending_tune() {
            let result = radio
                .tune_forward(request.channel.frequency_hz)
                .map_err(|e| e.to_string());
            engine.on_tuned(&request, result);
        }

        // Tune the transmitter to the acquired channel's uplink once the scan
        // camps, not on every channel it visits (an SDR may recalibrate on a
        // TX retune, which would stall the driver across a whole scan).
        if let Some(reverse_hz) = engine.pending_reverse_tune() {
            if let Err(e) = radio.tune_reverse(reverse_hz) {
                log::warn!(
                    "cdma-ms: reverse tune to {:.3} MHz failed: {}",
                    reverse_hz / 1e6,
                    e
                );
            } else {
                log::info!("cdma-ms: reverse link on {:.3} MHz", reverse_hz / 1e6);
            }
        }

        buf.clear();
        radio.read_forward(&mut buf);
        if !buf.is_empty() {
            if radio.forward_discontinuity() {
                engine.on_forward_discontinuity();
            }
            counters
                .samples_fed
                .fetch_add(buf.len() as u64, Ordering::Relaxed);
            if let Some(d) = dump.as_mut() {
                if d.write(&buf) {
                    let finished = dump.take().expect("dump in progress");
                    let (path, samples) = (finished.path, finished.written);
                    match finished.writer.finalize() {
                        Ok(()) => log::info!(
                            "cdma-ms: dump finished, {} samples in {}",
                            samples,
                            path.display()
                        ),
                        Err(e) => log::warn!("cdma-ms: finalizing {}: {}", path.display(), e),
                    }
                    engine.emit_event(MsEvent::DumpFinished {
                        path: path.display().to_string(),
                        samples,
                    });
                }
            }
            engine.set_tx_backlog_samples(radio.forward_backlog_samples());
            let started = std::time::Instant::now();
            engine.handle_forward(&buf);
            let took = started.elapsed();
            if took > slowest_forward {
                slowest_forward = took;
                log::debug!(
                    "cdma-ms: forward batch took {:.1} ms ({} samples)",
                    took.as_secs_f64() * 1e3,
                    buf.len()
                );
            }
        } else if radio.forward_exhausted() && !exhausted_reported {
            exhausted_reported = true;
            engine.on_forward_exhausted();
        }
        let overflows = radio.rx_overflows();
        if overflows != overflows_reported {
            log::warn!(
                "cdma-ms: radio RX overflow count {} (+{})",
                overflows,
                overflows - overflows_reported
            );
            overflows_reported = overflows;
            counters.rx_overflows.store(overflows, Ordering::Relaxed);
        }

        while let Some(burst) = engine.poll_transmit() {
            radio.write_reverse(&burst);
        }
    }
}
