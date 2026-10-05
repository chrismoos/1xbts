use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use cdma_bts::sdr::Radio;
use cdma_bts::sdr::network::proto::radio_control_service_server::{
    RadioControlService, RadioControlServiceServer,
};
use cdma_bts::sdr::network::{NETWORK_TICK_RATE, proto, wire};
use cdma_common::consts::SR1_CHIP_RATE_HZ;
use cdma_common::error::Error;
use log::{info, warn};
use parking_lot::Mutex;
use tokio::sync::oneshot;
use tonic::{Request, Response, Status};

use crate::bridge::{RxBridge, RxCommand, TxBridge, TxCommand};

fn default_rx_read_rate() -> u32 {
    (SR1_CHIP_RATE_HZ * 4) as u32
}

#[derive(Clone, Debug, Default)]
pub struct RxDefaults {
    pub antenna: String,
    pub gain_db: Option<f64>,
}

#[derive(Clone, Copy, Debug, Default)]
pub struct RadioCalibration {
    pub tx_sample_delay: Option<i64>,
    pub tx_full_scale_power_estimate_dbm: Option<f32>,
    pub rx_reference_dbm: Option<f32>,
}

/// Radio settings applied before the session opened. A reconnecting client
/// replays its setup RPCs. Matching values succeed, conflicting ones fail.
#[derive(Clone, Debug, Default, PartialEq)]
struct RecordedSettings {
    tx_frequency_hz: Option<u64>,
    tx_sample_rate_hz: Option<u64>,
    actual_tx_sample_rate_hz: Option<u64>,
    tx_bandwidth_hz: Option<u64>,
    tx_lo_offset_hz: Option<i64>,
    rx: Option<proto::SetupRxRequest>,
}

struct IdleState {
    radio: Box<dyn Radio>,
    recorded: RecordedSettings,
}

struct RunningState {
    tx_cmds: crossbeam_channel::Sender<TxCommand>,
    rx_cmds: Option<crossbeam_channel::Sender<RxCommand>>,
    samples_per_packet: usize,
    recorded: RecordedSettings,
}

enum State {
    Idle(IdleState),
    Running(RunningState),
    Unavailable,
}

struct Shared {
    state: Mutex<State>,
    session_id: Arc<AtomicU32>,
    peer: Arc<Mutex<Option<SocketAddr>>>,
    data_socket: UdpSocket,
    data_listener: Arc<std::net::TcpListener>,
    data_port: u16,
    inner_tick_rate: u64,
    rx_supported: bool,
    backend: String,
    rx_defaults: RxDefaults,
    radio_calibration: RadioCalibration,
    rx_sample_rate_hz: Arc<AtomicU32>,
}

pub struct RadioControlSvc {
    shared: Arc<Shared>,
}

fn internal(e: impl std::fmt::Display) -> Status {
    Status::internal(e.to_string())
}

fn new_session_id() -> u32 {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    nanos ^ std::process::id()
}

impl Shared {
    fn apply_setting<T: PartialEq + Copy + std::fmt::Debug>(
        &self,
        name: &str,
        value: T,
        field: impl Fn(&mut RecordedSettings) -> &mut Option<T>,
        apply: impl FnOnce(&mut dyn Radio) -> Result<(), Error>,
    ) -> Result<(), Status> {
        let mut state = self.state.lock();
        match &mut *state {
            State::Idle(idle) => {
                apply(idle.radio.as_mut()).map_err(internal)?;
                *field(&mut idle.recorded) = Some(value);
                Ok(())
            }
            State::Running(running) => {
                if *field(&mut running.recorded) == Some(value) {
                    Ok(())
                } else {
                    Err(Status::failed_precondition(format!(
                        "cannot change {name} while a session is active"
                    )))
                }
            }
            State::Unavailable => Err(Status::unavailable("radio unavailable")),
        }
    }

    fn tx_command(&self, cmd: TxCommand) -> Result<(), Status> {
        let state = self.state.lock();
        match &*state {
            State::Running(running) => running
                .tx_cmds
                .send(cmd)
                .map_err(|_| Status::unavailable("radio TX bridge stopped")),
            _ => Err(Status::failed_precondition("no active session")),
        }
    }

    fn rx_command(&self, cmd: RxCommand) -> Result<(), Status> {
        let state = self.state.lock();
        match &*state {
            State::Running(running) => match &running.rx_cmds {
                Some(rx_cmds) => rx_cmds
                    .send(cmd)
                    .map_err(|_| Status::unavailable("radio RX bridge stopped")),
                None => Err(Status::failed_precondition("radio has no RX chain")),
            },
            _ => Err(Status::failed_precondition("no active session")),
        }
    }
}

async fn await_reply<T>(rx: oneshot::Receiver<Result<T, String>>) -> Result<T, Status> {
    rx.await
        .map_err(|_| Status::unavailable("radio bridge dropped the request"))?
        .map_err(Status::internal)
}

#[tonic::async_trait]
impl RadioControlService for RadioControlSvc {
    async fn get_info(&self, _request: Request<()>) -> Result<Response<proto::RadioInfo>, Status> {
        Ok(Response::new(proto::RadioInfo {
            tick_rate: NETWORK_TICK_RATE,
            rx_supported: self.shared.rx_supported,
            max_samples_per_packet: wire::MAX_SAMPLES_PER_PACKET as u32,
            backend: self.shared.backend.clone(),
            tx_sample_delay: self.shared.radio_calibration.tx_sample_delay,
            tx_full_scale_power_estimate_dbm: self
                .shared
                .radio_calibration
                .tx_full_scale_power_estimate_dbm
                .map(f64::from),
            rx_reference_dbm: self
                .shared
                .radio_calibration
                .rx_reference_dbm
                .map(f64::from),
        }))
    }

    async fn set_tx_frequency(
        &self,
        request: Request<proto::SetTxFrequencyRequest>,
    ) -> Result<Response<()>, Status> {
        let frequency_hz = request.into_inner().frequency_hz;
        let running = matches!(&*self.shared.state.lock(), State::Running(_));
        if running {
            let (reply, reply_rx) = oneshot::channel();
            self.shared.tx_command(TxCommand::SetFrequencyHz {
                frequency_hz: frequency_hz as f64,
                reply,
            })?;
            await_reply(reply_rx).await?;
            return Ok(Response::new(()));
        }
        self.shared.apply_setting(
            "TX frequency",
            frequency_hz,
            |r| &mut r.tx_frequency_hz,
            |radio| radio.set_tx_frequency(frequency_hz as usize),
        )?;
        Ok(Response::new(()))
    }

    async fn set_tx_sample_rate(
        &self,
        request: Request<proto::SetTxSampleRateRequest>,
    ) -> Result<Response<proto::SetTxSampleRateResponse>, Status> {
        let sample_rate_hz = request.into_inner().sample_rate_hz;
        let requested_sample_rate = usize::try_from(sample_rate_hz)
            .map_err(|_| Status::invalid_argument("TX sample rate exceeds platform usize"))?;
        let mut state = self.shared.state.lock();
        let actual_sample_rate_hz = match &mut *state {
            State::Idle(idle) => {
                let actual = idle
                    .radio
                    .set_tx_sample_rate(requested_sample_rate)
                    .map_err(internal)? as u64;
                idle.recorded.tx_sample_rate_hz = Some(sample_rate_hz);
                idle.recorded.actual_tx_sample_rate_hz = Some(actual);
                actual
            }
            State::Running(running)
                if running.recorded.tx_sample_rate_hz == Some(sample_rate_hz) =>
            {
                running
                    .recorded
                    .actual_tx_sample_rate_hz
                    .ok_or_else(|| Status::internal("actual TX sample rate was not recorded"))?
            }
            State::Running(_) => {
                return Err(Status::failed_precondition(
                    "cannot change TX sample rate while a session is active",
                ));
            }
            State::Unavailable => return Err(Status::unavailable("radio unavailable")),
        };
        Ok(Response::new(proto::SetTxSampleRateResponse {
            actual_sample_rate_hz,
        }))
    }

    async fn set_tx_bandwidth(
        &self,
        request: Request<proto::SetTxBandwidthRequest>,
    ) -> Result<Response<()>, Status> {
        let bandwidth_hz = request.into_inner().bandwidth_hz;
        self.shared.apply_setting(
            "TX bandwidth",
            bandwidth_hz,
            |r| &mut r.tx_bandwidth_hz,
            |radio| radio.set_tx_bandwidth(bandwidth_hz as usize),
        )?;
        Ok(Response::new(()))
    }

    async fn set_tx_lo_offset(
        &self,
        request: Request<proto::SetTxLoOffsetRequest>,
    ) -> Result<Response<()>, Status> {
        let offset_hz = request.into_inner().offset_hz;
        self.shared.apply_setting(
            "TX LO offset",
            offset_hz,
            |r| &mut r.tx_lo_offset_hz,
            |radio| radio.set_tx_lo_offset_hz(offset_hz),
        )?;
        Ok(Response::new(()))
    }

    async fn setup_rx(
        &self,
        request: Request<proto::SetupRxRequest>,
    ) -> Result<Response<()>, Status> {
        if !self.shared.rx_supported {
            return Err(Status::failed_precondition("radio has no RX chain"));
        }
        let mut req = request.into_inner();
        if req.antenna.is_empty() {
            req.antenna = self.shared.rx_defaults.antenna.clone();
        }
        if req.gain_db.is_none() {
            req.gain_db = self.shared.rx_defaults.gain_db;
        }

        // A running session holds the RX half in its bridge, so only what the
        // bridge can change live is accepted: the frequency and the gain.
        let retune = {
            let mut state = self.shared.state.lock();
            match &mut *state {
                State::Idle(idle) => {
                    idle.radio
                        .setup_rx(
                            req.channel as usize,
                            &req.antenna,
                            req.frequency_hz,
                            req.sample_rate_hz,
                            req.bandwidth_hz,
                            req.gain_db,
                        )
                        .map_err(internal)?;
                    self.shared
                        .rx_sample_rate_hz
                        .store(req.sample_rate_hz as u32, Ordering::Relaxed);
                    info!(
                        "radiod: rx configured antenna='{}' freq={} rate={} bw={} gain={:?}",
                        req.antenna,
                        req.frequency_hz,
                        req.sample_rate_hz,
                        req.bandwidth_hz,
                        req.gain_db
                    );
                    idle.recorded.rx = Some(req);
                    return Ok(Response::new(()));
                }
                State::Running(running) => match running.recorded.rx.as_ref() {
                    Some(current) if *current == req => return Ok(Response::new(())),
                    Some(current)
                        if current.channel == req.channel
                            && current.antenna == req.antenna
                            && current.sample_rate_hz == req.sample_rate_hz
                            && current.bandwidth_hz == req.bandwidth_hz =>
                    {
                        (req.frequency_hz, req.gain_db)
                    }
                    _ => {
                        return Err(Status::failed_precondition(
                            "cannot change the RX channel, antenna, rate or bandwidth while a session is active",
                        ));
                    }
                },
                State::Unavailable => return Err(Status::unavailable("radio unavailable")),
            }
        };

        let (frequency_hz, gain_db) = retune;
        let (reply, reply_rx) = oneshot::channel();
        self.shared.rx_command(RxCommand::SetFrequency {
            frequency_hz,
            reply,
        })?;
        await_reply(reply_rx).await?;
        if let Some(gain_db) = gain_db {
            let (reply, reply_rx) = oneshot::channel();
            self.shared
                .rx_command(RxCommand::SetGain { gain_db, reply })?;
            await_reply(reply_rx).await?;
        }
        info!("radiod: rx retuned for a new client freq={frequency_hz} gain={gain_db:?}");
        if let State::Running(running) = &mut *self.shared.state.lock() {
            running.recorded.rx = Some(req);
        }
        Ok(Response::new(()))
    }

    async fn open_session(
        &self,
        request: Request<proto::OpenSessionRequest>,
    ) -> Result<Response<proto::OpenSessionResponse>, Status> {
        let requested = request.into_inner().samples_per_packet as usize;
        let shared = &self.shared;
        let mut state = shared.state.lock();
        match std::mem::replace(&mut *state, State::Unavailable) {
            State::Idle(idle) => {
                let Some(tx_sample_rate_hz) = idle.recorded.actual_tx_sample_rate_hz else {
                    *state = State::Idle(idle);
                    return Err(Status::failed_precondition(
                        "SetTxSampleRate must be called before OpenSession",
                    ));
                };
                let samples_per_packet = requested.clamp(1, wire::MAX_SAMPLES_PER_PACKET);
                let (tx_half, rx_half) = match idle.radio.split() {
                    Ok(halves) => halves,
                    Err(e) => {
                        warn!("radiod: radio split failed: {e}");
                        return Err(internal(e));
                    }
                };

                let session_id = new_session_id();
                shared.session_id.store(session_id, Ordering::Relaxed);
                *shared.peer.lock() = None;

                let (tx_cmds, tx_cmd_rx) = crossbeam_channel::unbounded();
                let tx_bridge = TxBridge {
                    session_id: shared.session_id.clone(),
                    peer: shared.peer.clone(),
                    inner_tick_rate: shared.inner_tick_rate,
                    tx_sample_rate_hz,
                    tcp_listener: shared.data_listener.clone(),
                };
                let tx_socket = shared.data_socket.try_clone().map_err(internal)?;
                std::thread::Builder::new()
                    .name("radiod-tx".into())
                    .spawn(move || tx_bridge.run(tx_half, tx_socket, tx_cmd_rx))
                    .map_err(internal)?;

                let rx_cmds = match rx_half {
                    Some(rx_half) => {
                        let (rx_cmds, rx_cmd_rx) = crossbeam_channel::unbounded();
                        let rx_bridge = RxBridge {
                            session_id: shared.session_id.clone(),
                            peer: shared.peer.clone(),
                            inner_tick_rate: shared.inner_tick_rate,
                            rx_sample_rate_hz: shared.rx_sample_rate_hz.clone(),
                            samples_per_packet,
                        };
                        let rx_socket = shared.data_socket.try_clone().map_err(internal)?;
                        std::thread::Builder::new()
                            .name("radiod-rx".into())
                            .spawn(move || rx_bridge.run(rx_half, rx_socket, rx_cmd_rx))
                            .map_err(internal)?;
                        Some(rx_cmds)
                    }
                    None => None,
                };

                info!(
                    "radiod: session {session_id:#010x} open, samples_per_packet={samples_per_packet}"
                );
                *state = State::Running(RunningState {
                    tx_cmds,
                    rx_cmds,
                    samples_per_packet,
                    recorded: idle.recorded,
                });
                Ok(Response::new(proto::OpenSessionResponse {
                    session_id,
                    data_port: shared.data_port as u32,
                    samples_per_packet: samples_per_packet as u32,
                }))
            }
            running @ State::Running(_) => {
                let State::Running(ref session) = running else {
                    unreachable!()
                };
                let session_id = new_session_id();
                let samples_per_packet = session.samples_per_packet;
                shared.session_id.store(session_id, Ordering::Relaxed);
                *shared.peer.lock() = None;
                warn!(
                    "radiod: superseding active session with {session_id:#010x} (client reconnect)"
                );
                *state = running;
                Ok(Response::new(proto::OpenSessionResponse {
                    session_id,
                    data_port: shared.data_port as u32,
                    samples_per_packet: samples_per_packet as u32,
                }))
            }
            State::Unavailable => Err(Status::unavailable("radio unavailable")),
        }
    }

    async fn get_hardware_time(
        &self,
        _request: Request<()>,
    ) -> Result<Response<proto::HardwareTime>, Status> {
        let (reply, reply_rx) = oneshot::channel();
        self.shared.tx_command(TxCommand::GetHardwareTime(reply))?;
        let time_ns = await_reply(reply_rx).await?;
        Ok(Response::new(proto::HardwareTime { time_ns }))
    }

    async fn set_hardware_time(
        &self,
        request: Request<proto::HardwareTime>,
    ) -> Result<Response<()>, Status> {
        let time_ns = request.into_inner().time_ns;
        let (reply, reply_rx) = oneshot::channel();
        self.shared
            .tx_command(TxCommand::SetHardwareTime { time_ns, reply })?;
        await_reply(reply_rx).await?;
        Ok(Response::new(()))
    }

    async fn enable_transmit(
        &self,
        request: Request<proto::EnableTransmitRequest>,
    ) -> Result<Response<()>, Status> {
        let req = request.into_inner();
        let (reply, reply_rx) = oneshot::channel();
        self.shared.tx_command(TxCommand::EnableTransmit {
            enable: req.enable,
            time_ns: req.time_ns,
            reply,
        })?;
        await_reply(reply_rx).await?;
        Ok(Response::new(()))
    }

    async fn rx_activate(
        &self,
        request: Request<proto::RxActivateRequest>,
    ) -> Result<Response<()>, Status> {
        let time_ns = request.into_inner().time_ns;
        let (reply, reply_rx) = oneshot::channel();
        self.shared
            .rx_command(RxCommand::Activate { time_ns, reply })?;
        await_reply(reply_rx).await?;
        Ok(Response::new(()))
    }

    async fn rx_deactivate(&self, _request: Request<()>) -> Result<Response<()>, Status> {
        let (reply, reply_rx) = oneshot::channel();
        self.shared.rx_command(RxCommand::Deactivate { reply })?;
        await_reply(reply_rx).await?;
        Ok(Response::new(()))
    }

    async fn set_rx_frequency(
        &self,
        request: Request<proto::SetRxFrequencyRequest>,
    ) -> Result<Response<()>, Status> {
        let frequency_hz = request.into_inner().frequency_hz;
        let (reply, reply_rx) = oneshot::channel();
        self.shared.rx_command(RxCommand::SetFrequency {
            frequency_hz,
            reply,
        })?;
        await_reply(reply_rx).await?;
        Ok(Response::new(()))
    }

    async fn set_rx_gain(
        &self,
        request: Request<proto::SetRxGainRequest>,
    ) -> Result<Response<()>, Status> {
        let gain_db = request.into_inner().gain_db;
        let (reply, reply_rx) = oneshot::channel();
        self.shared
            .rx_command(RxCommand::SetGain { gain_db, reply })?;
        await_reply(reply_rx).await?;
        Ok(Response::new(()))
    }
}

pub async fn serve(
    radio: Box<dyn Radio>,
    rx_supported: bool,
    backend: String,
    rx_defaults: RxDefaults,
    radio_calibration: RadioCalibration,
    listen: SocketAddr,
    data_listen: SocketAddr,
    ready: Option<oneshot::Sender<(SocketAddr, SocketAddr)>>,
    shutdown: impl Future<Output = ()> + Send,
) -> Result<(), Error> {
    let inner_tick_rate = radio.tick_rate();
    let data_socket = UdpSocket::bind(data_listen)?;
    set_receive_buffer(&data_socket, DATA_SOCKET_RECEIVE_BYTES);
    let data_addr = data_socket.local_addr()?;
    let data_listener = Arc::new(std::net::TcpListener::bind(data_addr)?);

    let listener = tokio::net::TcpListener::bind(listen).await?;
    let control_addr = listener.local_addr()?;

    let shared = Arc::new(Shared {
        state: Mutex::new(State::Idle(IdleState {
            radio,
            recorded: RecordedSettings::default(),
        })),
        session_id: Arc::new(AtomicU32::new(0)),
        peer: Arc::new(Mutex::new(None)),
        data_socket,
        data_listener,
        data_port: data_addr.port(),
        inner_tick_rate,
        rx_supported,
        backend,
        rx_defaults,
        radio_calibration,
        rx_sample_rate_hz: Arc::new(AtomicU32::new(default_rx_read_rate())),
    });

    info!(
        "radiod: control plane on {control_addr}, data plane on {data_addr}, inner tick rate {inner_tick_rate}"
    );
    if let Some(ready) = ready {
        let _ = ready.send((control_addr, data_addr));
    }

    tonic::transport::Server::builder()
        .add_service(RadioControlServiceServer::new(RadioControlSvc { shared }))
        .serve_with_incoming_shutdown(
            tokio_stream::wrappers::TcpListenerStream::new(listener),
            shutdown,
        )
        .await
        .map_err(|e| Error::from(format!("radiod: control plane failed: {e}")))
}

const DATA_SOCKET_RECEIVE_BYTES: usize = 8 * 1024 * 1024;

fn set_receive_buffer(socket: &UdpSocket, bytes: usize) {
    let socket = socket2::SockRef::from(socket);
    if let Err(error) = socket.set_recv_buffer_size(bytes) {
        log::warn!("radiod: could not size the data socket receive buffer: {error}");
    }
    // Kernels may silently cap the receive buffer. Read back its size to expose packet-loss risk.
    match socket.recv_buffer_size() {
        Ok(actual) => {
            let level = if actual < bytes {
                log::Level::Warn
            } else {
                log::Level::Info
            };
            log::log!(
                level,
                "radiod: data socket receive buffer {} bytes (asked for {})",
                actual,
                bytes
            );
        }
        Err(error) => log::warn!("radiod: could not read the data socket receive buffer: {error}"),
    }
}
