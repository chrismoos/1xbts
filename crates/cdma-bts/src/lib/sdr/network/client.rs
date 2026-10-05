//! Network radio client for a remote SDR served by `cdma-radiod`. All
//! hardware times cross the network in nanoseconds — the server converts
//! to the inner radio's native tick units.

use std::net::{SocketAddr, ToSocketAddrs, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use cdma_common::error::Error;
use crossbeam_channel::{Receiver, Sender, TrySendError};
use log::{debug, info, warn};
use num_complex::Complex32;
use tonic::transport::{Channel, Endpoint};

use super::clock::NetworkClock;
use super::proto::radio_control_service_client::RadioControlServiceClient;
use super::wire::{
    self, FLAG_RX_DISCONTINUITY, FLAG_TX_BLOCK_END, FLAG_TX_BLOCK_START, FLAG_TX_DISABLE,
    FLAG_TX_NO_TICK, PacketHeader, StreamKind, TxTransport, decode_header, decode_samples_into,
    encode_packet, samples_to_ns,
};
use super::{NETWORK_TICK_RATE, proto};
use crate::sdr::{Radio, RadioRx, RadioTx, RxReadResult};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);
const RPC_TIMEOUT: Duration = Duration::from_secs(5);
const CLOCK_SYNC_SAMPLES: usize = 5;
const HELLO_REPEATS: usize = 3;
const RX_PACKET_QUEUE_CAPACITY: usize = 512;
const RECV_BUF_LEN: usize = 65536;
const RECEIVER_POLL: Duration = Duration::from_millis(250);
const SOCKET_BUFFER_BYTES: usize = 4 * 1024 * 1024;
const MAX_TX_BLOCK_SAMPLES: usize = 32_768;
// Pace UDP writes to avoid socket overflow.
const TX_PACING_NUM: u32 = 21;
const TX_PACING_DEN: u32 = 20;
const TX_PACING_SLACK: Duration = Duration::from_micros(500);
const TX_PACING_IDLE: Duration = Duration::from_millis(50);

#[derive(Debug, Clone)]
pub struct NetworkRadioOptions {
    pub samples_per_packet: usize,
    pub tx_transport: TxTransport,
    pub data_host: Option<String>,
}

impl Default for NetworkRadioOptions {
    fn default() -> Self {
        Self {
            samples_per_packet: wire::DEFAULT_SAMPLES_PER_PACKET,
            tx_transport: TxTransport::default(),
            data_host: None,
        }
    }
}

struct RxPacket {
    seq: u32,
    time_ns: u64,
    discontinuity: bool,
    samples: Vec<Complex32>,
}

pub struct NetworkRadio {
    rt: Arc<RpcRuntime>,
    client: RadioControlServiceClient<Channel>,
    info: proto::RadioInfo,
    control_host: String,
    options: NetworkRadioOptions,
    tx_sample_rate_hz: u64,
    rx_sample_rate_hz: u64,
    rx_configured: bool,
}

fn rpc_err(context: &str, status: tonic::Status) -> Error {
    format!("network radio: {context}: {status}").into()
}

enum RpcRuntime {
    Borrowed(tokio::runtime::Handle),
    Owned(tokio::runtime::Runtime),
}

impl RpcRuntime {
    fn new() -> Result<Self, Error> {
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => Ok(Self::Borrowed(handle)),
            Err(_) => Ok(Self::Owned(
                tokio::runtime::Builder::new_multi_thread()
                    .worker_threads(1)
                    .thread_name("netradio-rpc")
                    .enable_all()
                    .build()?,
            )),
        }
    }

    fn handle(&self) -> tokio::runtime::Handle {
        match self {
            Self::Borrowed(handle) => handle.clone(),
            Self::Owned(runtime) => runtime.handle().clone(),
        }
    }
}

fn block_on<F: Future>(rt: &RpcRuntime, fut: F) -> F::Output {
    let handle = rt.handle();
    if tokio::runtime::Handle::try_current().is_ok() {
        tokio::task::block_in_place(|| handle.block_on(fut))
    } else {
        handle.block_on(fut)
    }
}

impl NetworkRadio {
    pub fn connect(control_addr: &str, options: NetworkRadioOptions) -> Result<Self, Error> {
        let rt = Arc::new(RpcRuntime::new()?);
        let control_host = control_addr
            .rsplit_once(':')
            .map(|(host, _)| host.to_string())
            .ok_or_else(|| Error::from(format!("network radio: bad address {control_addr}")))?;
        let endpoint = Endpoint::from_shared(format!("http://{control_addr}"))
            .map_err(|e| format!("network radio: bad address {control_addr}: {e}"))?
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(RPC_TIMEOUT);
        let channel = block_on(&rt, endpoint.connect())
            .map_err(|e| format!("network radio: connect {control_addr}: {e}"))?;
        let mut client = RadioControlServiceClient::new(channel);
        let info = block_on(&rt, client.get_info(()))
            .map_err(|status| rpc_err("get_info", status))?
            .into_inner();
        info!(
            "network radio: connected to {control_addr} backend={} rx_supported={} max_samples_per_packet={}",
            info.backend, info.rx_supported, info.max_samples_per_packet
        );
        Ok(Self {
            rt,
            client,
            info,
            control_host,
            options,
            tx_sample_rate_hz: 0,
            rx_sample_rate_hz: 0,
            rx_configured: false,
        })
    }

    fn block_on<F, T>(&self, context: &str, fut: F) -> Result<T, Error>
    where
        F: Future<Output = Result<tonic::Response<T>, tonic::Status>>,
    {
        block_on(&self.rt, fut)
            .map(|resp| resp.into_inner())
            .map_err(|status| rpc_err(context, status))
    }
}

impl Radio for NetworkRadio {
    fn tick_rate(&self) -> u64 {
        NETWORK_TICK_RATE
    }

    fn tx_sample_delay(&self) -> Option<i64> {
        self.info.tx_sample_delay
    }

    fn tx_full_scale_power_estimate_dbm(&self) -> Option<f32> {
        self.info
            .tx_full_scale_power_estimate_dbm
            .map(|dbm| dbm as f32)
    }

    fn rx_reference_dbm(&self) -> Option<f32> {
        self.info.rx_reference_dbm.map(|dbm| dbm as f32)
    }

    fn set_tx_frequency(&mut self, center_frequency: usize) -> Result<(), Error> {
        let mut client = self.client.clone();
        self.block_on(
            "set_tx_frequency",
            client.set_tx_frequency(proto::SetTxFrequencyRequest {
                frequency_hz: center_frequency as u64,
            }),
        )?;
        Ok(())
    }

    fn set_tx_sample_rate(&mut self, sample_rate: usize) -> Result<usize, Error> {
        let mut client = self.client.clone();
        let response = self.block_on(
            "set_tx_sample_rate",
            client.set_tx_sample_rate(proto::SetTxSampleRateRequest {
                sample_rate_hz: sample_rate as u64,
            }),
        )?;
        let actual = usize::try_from(response.actual_sample_rate_hz)
            .map_err(|_| Error::from("network radio: actual TX sample rate exceeds usize"))?;
        self.tx_sample_rate_hz = response.actual_sample_rate_hz;
        Ok(actual)
    }

    fn set_tx_bandwidth(&mut self, bandwidth: usize) -> Result<(), Error> {
        let mut client = self.client.clone();
        self.block_on(
            "set_tx_bandwidth",
            client.set_tx_bandwidth(proto::SetTxBandwidthRequest {
                bandwidth_hz: bandwidth as u64,
            }),
        )?;
        Ok(())
    }

    fn set_tx_lo_offset_hz(&mut self, offset_hz: i64) -> Result<(), Error> {
        let mut client = self.client.clone();
        self.block_on(
            "set_tx_lo_offset",
            client.set_tx_lo_offset(proto::SetTxLoOffsetRequest { offset_hz }),
        )?;
        Ok(())
    }

    fn setup_rx(
        &mut self,
        channel: usize,
        antenna: &str,
        frequency_hz: f64,
        sample_rate_hz: f64,
        bandwidth_hz: f64,
        gain_db: Option<f64>,
    ) -> Result<(), Error> {
        if !self.info.rx_supported {
            return Err("network radio: server backend has no RX".into());
        }
        let mut client = self.client.clone();
        self.block_on(
            "setup_rx",
            client.setup_rx(proto::SetupRxRequest {
                channel: channel as u32,
                antenna: antenna.to_string(),
                frequency_hz,
                sample_rate_hz,
                bandwidth_hz,
                gain_db,
            }),
        )?;
        self.rx_sample_rate_hz = sample_rate_hz as u64;
        self.rx_configured = true;
        Ok(())
    }

    fn split(self: Box<Self>) -> Result<(Box<dyn RadioTx>, Option<Box<dyn RadioRx>>), Error> {
        if self.tx_sample_rate_hz == 0 {
            return Err("network radio: set_tx_sample_rate must run before split".into());
        }
        let mut client = self.client.clone();
        let requested = self
            .options
            .samples_per_packet
            .min(wire::MAX_SAMPLES_PER_PACKET);
        let session = self.block_on(
            "open_session",
            client.open_session(proto::OpenSessionRequest {
                samples_per_packet: requested as u32,
            }),
        )?;
        let samples_per_packet = session.samples_per_packet as usize;
        if samples_per_packet == 0 || samples_per_packet > wire::MAX_SAMPLES_PER_PACKET {
            return Err(format!(
                "network radio: server negotiated bad samples_per_packet {samples_per_packet}"
            )
            .into());
        }

        let data_host = self
            .options
            .data_host
            .clone()
            .unwrap_or_else(|| self.control_host.clone());
        let data_addr = resolve_data_addr(&data_host, session.data_port as u16)?;
        let socket = bind_matching_family(&data_addr)?;
        set_socket_buffers(&socket);
        socket.connect(data_addr)?;
        info!(
            "network radio: data plane {} -> {} session={:#010x} samples_per_packet={}",
            socket.local_addr()?,
            data_addr,
            session.session_id,
            samples_per_packet
        );

        let clock = Arc::new(NetworkClock::new(self.sync_clock(&mut client)?));

        let mut hello = Vec::new();
        for seq in 0..HELLO_REPEATS as u32 {
            encode_packet(
                &PacketHeader {
                    stream: StreamKind::Hello,
                    flags: 0,
                    session_id: session.session_id,
                    seq,
                    time_ns: 0,
                    sample_count: 0,
                    block_id: 0,
                    block_offset: 0,
                    block_sample_count: 0,
                },
                &[],
                &mut hello,
            );
            socket.send(&hello)?;
        }

        let tx_stream = match self.options.tx_transport {
            TxTransport::Udp => None,
            TxTransport::Tcp => {
                let stream = std::net::TcpStream::connect(data_addr).map_err(|e| {
                    Error::from(format!("network radio: TX stream connect {data_addr}: {e}"))
                })?;
                stream.set_nodelay(true)?;
                info!("network radio: transmitting over TCP to {data_addr}");
                Some(stream)
            }
        };

        let shutdown = Arc::new(AtomicBool::new(false));
        let queue_gap = Arc::new(AtomicBool::new(false));
        let (rx_packet_tx, rx_packet_rx) = crossbeam_channel::bounded(RX_PACKET_QUEUE_CAPACITY);
        spawn_receiver(
            socket.try_clone()?,
            session.session_id,
            clock.clone(),
            rx_packet_tx,
            queue_gap.clone(),
            shutdown.clone(),
        )?;

        let tx = NetworkRadioTx {
            rt: self.rt.clone(),
            client: self.client.clone(),
            clock: clock.clone(),
            socket,
            session_id: session.session_id,
            seq: 0,
            block_id: 0,
            samples_per_packet,
            tx_sample_rate_hz: self.tx_sample_rate_hz,
            scratch: Vec::new(),
            pace_next: Instant::now(),
            tx_stream,
            shutdown,
        };
        let rx = self.rx_configured.then(|| {
            Box::new(NetworkRadioRx {
                rt: self.rt.clone(),
                client: self.client.clone(),
                clock,
                packets: rx_packet_rx,
                pending: None,
                expected_seq: None,
                minimum_time_ns: None,
                pending_discontinuity: false,
                queue_gap,
                rx_sample_rate_hz: self.rx_sample_rate_hz,
            }) as Box<dyn RadioRx>
        });
        Ok((Box::new(tx), rx))
    }
}

impl NetworkRadio {
    /// RTT-smallest of a few time queries, midpoint-compensated.
    fn sync_clock(&self, client: &mut RadioControlServiceClient<Channel>) -> Result<u64, Error> {
        let mut best: Option<(Duration, u64, Instant)> = None;
        for _ in 0..CLOCK_SYNC_SAMPLES {
            let started = Instant::now();
            let time = self.block_on("get_hardware_time", client.get_hardware_time(()))?;
            let rtt = started.elapsed();
            if best.as_ref().map(|(b, _, _)| rtt < *b).unwrap_or(true) {
                best = Some((rtt, time.time_ns, started + rtt / 2));
            }
        }
        let (rtt, midpoint_ns, midpoint_at) = best.expect("at least one clock sample");
        debug!(
            "network radio: clock sync rtt={}us server_ns={}",
            rtt.as_micros(),
            midpoint_ns
        );
        Ok(midpoint_ns.saturating_add(midpoint_at.elapsed().as_nanos() as u64))
    }
}

fn resolve_data_addr(host: &str, port: u16) -> Result<SocketAddr, Error> {
    let trimmed = host.trim_start_matches('[').trim_end_matches(']');
    (trimmed, port)
        .to_socket_addrs()
        .map_err(|e| Error::from(format!("network radio: resolve {host}:{port}: {e}")))?
        .next()
        .ok_or_else(|| format!("network radio: no address for {host}:{port}").into())
}

fn bind_matching_family(peer: &SocketAddr) -> Result<UdpSocket, Error> {
    let bind_addr = if peer.is_ipv4() {
        "0.0.0.0:0"
    } else {
        "[::]:0"
    };
    Ok(UdpSocket::bind(bind_addr)?)
}

fn set_socket_buffers(socket: &UdpSocket) {
    let socket = socket2::SockRef::from(socket);
    for result in [
        socket.set_recv_buffer_size(SOCKET_BUFFER_BYTES),
        socket.set_send_buffer_size(SOCKET_BUFFER_BYTES),
    ] {
        if let Err(error) = result {
            debug!("network radio: socket buffer setting failed: {error}");
        }
    }
}

fn spawn_receiver(
    socket: UdpSocket,
    session_id: u32,
    clock: Arc<NetworkClock>,
    rx_packet_tx: Sender<RxPacket>,
    queue_gap: Arc<AtomicBool>,
    shutdown: Arc<AtomicBool>,
) -> Result<(), Error> {
    socket.set_read_timeout(Some(RECEIVER_POLL))?;
    std::thread::Builder::new()
        .name("netradio-recv".into())
        .spawn(move || {
            let mut buf = vec![0u8; RECV_BUF_LEN];
            let mut dropped: u64 = 0;
            loop {
                if shutdown.load(Ordering::Relaxed) {
                    return;
                }
                let len = match socket.recv(&mut buf) {
                    Ok(len) => len,
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            || e.kind() == std::io::ErrorKind::TimedOut
                            || e.kind() == std::io::ErrorKind::Interrupted =>
                    {
                        continue;
                    }
                    Err(e) => {
                        warn!("network radio: data socket recv failed: {e}");
                        return;
                    }
                };
                let header = match decode_header(&buf[..len]) {
                    Ok(header) => header,
                    Err(e) => {
                        debug!("network radio: dropping undecodable packet: {e}");
                        continue;
                    }
                };
                if header.session_id != session_id {
                    continue;
                }
                match header.stream {
                    StreamKind::ClockHeartbeat => clock.observe_server_time(header.time_ns),
                    StreamKind::RxSamples => {
                        let mut samples = Vec::new();
                        decode_samples_into(&buf[..len], &header, &mut samples);
                        let packet = RxPacket {
                            seq: header.seq,
                            time_ns: header.time_ns,
                            discontinuity: header.flags & FLAG_RX_DISCONTINUITY != 0,
                            samples,
                        };
                        match rx_packet_tx.try_send(packet) {
                            Ok(()) => {}
                            Err(TrySendError::Full(_)) => {
                                queue_gap.store(true, Ordering::Relaxed);
                                dropped += 1;
                                if dropped.is_power_of_two() {
                                    warn!(
                                        "network radio: RX queue full, dropped {dropped} packets total"
                                    );
                                }
                            }
                            Err(TrySendError::Disconnected(_)) => return,
                        }
                    }
                    StreamKind::Hello | StreamKind::TxSamples => {}
                }
            }
        })?;
    Ok(())
}

pub struct NetworkRadioTx {
    rt: Arc<RpcRuntime>,
    client: RadioControlServiceClient<Channel>,
    clock: Arc<NetworkClock>,
    socket: UdpSocket,
    session_id: u32,
    seq: u32,
    block_id: u32,
    samples_per_packet: usize,
    tx_sample_rate_hz: u64,
    scratch: Vec<u8>,
    pace_next: Instant,
    tx_stream: Option<std::net::TcpStream>,
    shutdown: Arc<AtomicBool>,
}

impl NetworkRadioTx {
    fn send_samples(&mut self, samples: &[Complex32], tick: Option<u64>) -> Result<(), Error> {
        if samples.len() <= MAX_TX_BLOCK_SAMPLES {
            return self.send_block(samples, tick);
        }
        let mut sent = 0usize;
        for block in samples.chunks(MAX_TX_BLOCK_SAMPLES) {
            let offset_ns = samples_to_ns(sent as u64, self.tx_sample_rate_hz);
            let block_tick = tick.map(|tick| tick.saturating_add(offset_ns));
            self.send_block(block, block_tick)?;
            sent += block.len();
        }
        Ok(())
    }

    fn pace(&mut self, samples: usize) {
        let now = Instant::now();
        if self.pace_next + TX_PACING_IDLE < now {
            self.pace_next = now;
        }
        if let Some(ahead) = self.pace_next.checked_duration_since(now)
            && ahead > TX_PACING_SLACK
        {
            std::thread::sleep(ahead);
        }
        let airtime = Duration::from_nanos(samples_to_ns(samples as u64, self.tx_sample_rate_hz));
        self.pace_next += airtime * TX_PACING_DEN / TX_PACING_NUM;
    }

    fn send_block(&mut self, samples: &[Complex32], tick: Option<u64>) -> Result<(), Error> {
        if samples.is_empty() {
            return Ok(());
        }
        let block_sample_count = u32::try_from(samples.len())
            .map_err(|_| Error::from("network radio: TX block exceeds u32 sample count"))?;
        let block_id = self.block_id;
        self.block_id = self.block_id.wrapping_add(1);
        let mut offset = 0usize;
        while offset < samples.len() {
            let chunk = &samples[offset..(offset + self.samples_per_packet).min(samples.len())];
            let (mut flags, time_ns) = match tick {
                Some(tick) => (
                    0,
                    tick.saturating_add(samples_to_ns(offset as u64, self.tx_sample_rate_hz)),
                ),
                None => (FLAG_TX_NO_TICK, 0),
            };
            if offset == 0 {
                flags |= FLAG_TX_BLOCK_START;
            }
            if offset + chunk.len() == samples.len() {
                flags |= FLAG_TX_BLOCK_END;
            }
            encode_packet(
                &PacketHeader {
                    stream: StreamKind::TxSamples,
                    flags,
                    session_id: self.session_id,
                    seq: self.seq,
                    time_ns,
                    sample_count: chunk.len() as u16,
                    block_id,
                    block_offset: offset as u32,
                    block_sample_count,
                },
                chunk,
                &mut self.scratch,
            );
            match self.tx_stream.as_mut() {
                Some(stream) => {
                    use std::io::Write;
                    stream.write_all(&self.scratch)?;
                }
                None => {
                    self.pace(chunk.len());
                    self.socket.send(&self.scratch)?;
                }
            }
            self.seq = self.seq.wrapping_add(1);
            offset += chunk.len();
        }
        Ok(())
    }

    fn send_disable_fence(&mut self) -> Result<(), Error> {
        let Some(stream) = self.tx_stream.as_mut() else {
            return Err("network radio: TX disable fence requires TCP".into());
        };
        encode_packet(
            &PacketHeader {
                stream: StreamKind::TxSamples,
                flags: FLAG_TX_DISABLE,
                session_id: self.session_id,
                seq: self.seq,
                time_ns: 0,
                sample_count: 0,
                block_id: 0,
                block_offset: 0,
                block_sample_count: 0,
            },
            &[],
            &mut self.scratch,
        );
        use std::io::Write;
        stream.write_all(&self.scratch)?;
        self.seq = self.seq.wrapping_add(1);
        Ok(())
    }

    fn rpc<F, T>(&self, context: &str, fut: F) -> Result<T, Error>
    where
        F: Future<Output = Result<tonic::Response<T>, tonic::Status>>,
    {
        block_on(&self.rt, fut)
            .map(|resp| resp.into_inner())
            .map_err(|status| rpc_err(context, status))
    }
}

impl Drop for NetworkRadioTx {
    fn drop(&mut self) {
        self.shutdown.store(true, Ordering::Relaxed);
    }
}

impl RadioTx for NetworkRadioTx {
    fn tick_rate(&self) -> u64 {
        NETWORK_TICK_RATE
    }

    fn get_hardware_time(&self) -> Result<u64, Error> {
        Ok(self.clock.now_ns())
    }

    fn set_hardware_time(&self, ticks: u64) -> Result<(), Error> {
        let mut client = self.client.clone();
        self.rpc(
            "set_hardware_time",
            client.set_hardware_time(proto::HardwareTime { time_ns: ticks }),
        )?;
        self.clock.reanchor(ticks);
        Ok(())
    }

    fn transmit(&mut self, samples: &[Complex32]) -> Result<(), Error> {
        self.send_samples(samples, None)
    }

    fn transmit_at(&mut self, samples: &[Complex32], tick: Option<u64>) -> Result<(), Error> {
        self.send_samples(samples, tick)
    }

    fn enable_transmit(&mut self, enable: bool) -> Result<(), Error> {
        self.enable_transmit_at(enable, None)
    }

    fn enable_transmit_at(&mut self, enable: bool, tick: Option<u64>) -> Result<(), Error> {
        if !enable && self.tx_stream.is_some() {
            return self.send_disable_fence();
        }
        let mut client = self.client.clone();
        self.rpc(
            "enable_transmit",
            client.enable_transmit(proto::EnableTransmitRequest {
                enable,
                time_ns: tick,
            }),
        )?;
        Ok(())
    }

    fn set_tx_frequency_hz(&mut self, frequency_hz: f64) -> Result<(), Error> {
        let mut client = self.client.clone();
        self.rpc(
            "set_tx_frequency",
            client.set_tx_frequency(proto::SetTxFrequencyRequest {
                frequency_hz: frequency_hz as u64,
            }),
        )?;
        Ok(())
    }
}

pub struct NetworkRadioRx {
    rt: Arc<RpcRuntime>,
    client: RadioControlServiceClient<Channel>,
    clock: Arc<NetworkClock>,
    packets: Receiver<RxPacket>,
    pending: Option<(RxPacket, usize)>,
    expected_seq: Option<u32>,
    minimum_time_ns: Option<u64>,
    pending_discontinuity: bool,
    queue_gap: Arc<AtomicBool>,
    rx_sample_rate_hz: u64,
}

impl NetworkRadioRx {
    fn rpc<F, T>(&self, context: &str, fut: F) -> Result<T, Error>
    where
        F: Future<Output = Result<tonic::Response<T>, tonic::Status>>,
    {
        block_on(&self.rt, fut)
            .map(|resp| resp.into_inner())
            .map_err(|status| rpc_err(context, status))
    }

    fn next_packet(&mut self, wait: Option<Duration>) -> Option<(RxPacket, usize)> {
        let deadline = wait.map(|timeout| Instant::now() + timeout);
        loop {
            let (packet, consumed) = if let Some(pending) = self.pending.take() {
                pending
            } else {
                let packet = match deadline {
                    Some(deadline) => self
                        .packets
                        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                        .ok()?,
                    None => self.packets.try_recv().ok()?,
                };
                (packet, 0)
            };
            if let Some(minimum_time_ns) = self.minimum_time_ns {
                let end_ns = packet.time_ns.saturating_add(samples_to_ns(
                    packet.samples.len() as u64,
                    self.rx_sample_rate_hz,
                ));
                if end_ns <= minimum_time_ns {
                    continue;
                }
                let skip = if packet.time_ns < minimum_time_ns {
                    ((minimum_time_ns - packet.time_ns) as u128 * self.rx_sample_rate_hz as u128
                        / 1_000_000_000) as usize
                } else {
                    0
                };
                self.minimum_time_ns = None;
                return Some((packet, consumed.max(skip)));
            }
            return Some((packet, consumed));
        }
    }
}

impl RadioRx for NetworkRadioRx {
    fn tick_rate(&self) -> u64 {
        NETWORK_TICK_RATE
    }

    fn get_hardware_time(&self) -> Result<u64, Error> {
        Ok(self.clock.now_ns())
    }

    fn rx_read(&mut self, buf: &mut [Complex32], timeout_us: i64) -> Result<RxReadResult, Error> {
        let deadline = Instant::now() + Duration::from_micros(timeout_us.max(0) as u64);
        let mut written = 0usize;
        let mut first_time_ns = None;
        let mut overflow = self.pending_discontinuity;
        self.pending_discontinuity = false;
        if self.queue_gap.swap(false, Ordering::Relaxed) {
            overflow = true;
        }

        while written < buf.len() {
            // Fill the buffer like a hardware backend does. Returning whatever
            // is queued hands the receive chain blocks of a packet or two, and
            // its per-block work then outruns real time.
            let wait = Some(deadline.saturating_duration_since(Instant::now()));
            let Some((packet, consumed)) = self.next_packet(wait) else {
                break;
            };

            if consumed == 0 {
                let gap = packet.discontinuity
                    || self
                        .expected_seq
                        .map(|expected| packet.seq != expected)
                        .unwrap_or(false);
                self.expected_seq = Some(packet.seq.wrapping_add(1));
                if gap {
                    if written > 0 {
                        // Never mix discontinuous data in one read: report
                        // the gap at the start of the next read instead.
                        self.pending = Some((packet, 0));
                        self.pending_discontinuity = true;
                        break;
                    }
                    overflow = true;
                }
            }

            if first_time_ns.is_none() {
                first_time_ns = Some(
                    packet
                        .time_ns
                        .saturating_add(samples_to_ns(consumed as u64, self.rx_sample_rate_hz)),
                );
            }
            let take = (packet.samples.len() - consumed).min(buf.len() - written);
            buf[written..written + take]
                .copy_from_slice(&packet.samples[consumed..consumed + take]);
            written += take;
            if consumed + take < packet.samples.len() {
                self.pending = Some((packet, consumed + take));
            }
        }

        Ok(RxReadResult {
            samples_read: written,
            time_ticks: first_time_ns.unwrap_or_else(|| self.clock.now_ns()),
            overflow,
        })
    }

    fn rx_activate(&mut self, time_ticks: Option<u64>) -> Result<(), Error> {
        let mut client = self.client.clone();
        self.rpc(
            "rx_activate",
            client.rx_activate(proto::RxActivateRequest {
                time_ns: time_ticks,
            }),
        )?;
        self.expected_seq = None;
        self.pending = None;
        self.minimum_time_ns = None;
        self.pending_discontinuity = false;
        Ok(())
    }

    fn rx_deactivate(&mut self) -> Result<(), Error> {
        let mut client = self.client.clone();
        self.rpc("rx_deactivate", client.rx_deactivate(()))?;
        Ok(())
    }

    fn set_rx_frequency(&mut self, frequency_hz: f64) -> Result<(), Error> {
        let mut client = self.client.clone();
        self.rpc(
            "set_rx_frequency",
            client.set_rx_frequency(proto::SetRxFrequencyRequest { frequency_hz }),
        )?;
        self.minimum_time_ns = Some(self.clock.now_ns());
        self.pending = None;
        self.expected_seq = None;
        self.pending_discontinuity = true;
        Ok(())
    }

    fn set_rx_gain(&mut self, gain_db: f64) -> Result<(), Error> {
        let mut client = self.client.clone();
        self.rpc(
            "set_rx_gain",
            client.set_rx_gain(proto::SetRxGainRequest { gain_db }),
        )?;
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packet(seq: u32, time_ns: u64, len: usize, discontinuity: bool) -> RxPacket {
        RxPacket {
            seq,
            time_ns,
            discontinuity,
            samples: vec![Complex32::new(seq as f32, 0.0); len],
        }
    }

    fn test_rx(packets: Receiver<RxPacket>) -> NetworkRadioRx {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let channel = {
            let _guard = runtime.enter();
            Endpoint::from_static("http://127.0.0.1:1").connect_lazy()
        };
        NetworkRadioRx {
            rt: Arc::new(RpcRuntime::Owned(runtime)),
            client: RadioControlServiceClient::new(channel),
            clock: Arc::new(NetworkClock::new(0)),
            packets,
            pending: None,
            expected_seq: None,
            minimum_time_ns: None,
            pending_discontinuity: false,
            queue_gap: Arc::new(AtomicBool::new(false)),
            rx_sample_rate_hz: 1_000_000,
        }
    }

    #[test]
    fn rx_read_assembles_contiguous_packets() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        tx.send(packet(0, 1_000_000, 100, false)).unwrap();
        tx.send(packet(1, 1_100_000, 100, false)).unwrap();
        let mut radio_rx = test_rx(rx);

        let mut buf = vec![Complex32::default(); 150];
        let result = radio_rx.rx_read(&mut buf, 10_000).unwrap();
        assert_eq!(result.samples_read, 150);
        assert_eq!(result.time_ticks, 1_000_000);
        assert!(!result.overflow);
        assert_eq!(buf[0].re, 0.0);
        assert_eq!(buf[149].re, 1.0);

        let result = radio_rx.rx_read(&mut buf, 10_000).unwrap();
        assert_eq!(result.samples_read, 50);
        assert_eq!(result.time_ticks, 1_100_000 + 50_000);
        assert!(!result.overflow);
    }

    #[test]
    fn rx_read_ends_at_seq_gap_and_flags_next_read() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        tx.send(packet(0, 1_000_000, 100, false)).unwrap();
        tx.send(packet(5, 9_000_000, 100, false)).unwrap();
        let mut radio_rx = test_rx(rx);

        let mut buf = vec![Complex32::default(); 400];
        let result = radio_rx.rx_read(&mut buf, 10_000).unwrap();
        assert_eq!(result.samples_read, 100);
        assert!(!result.overflow);

        let result = radio_rx.rx_read(&mut buf, 10_000).unwrap();
        assert_eq!(result.samples_read, 100);
        assert_eq!(result.time_ticks, 9_000_000);
        assert!(result.overflow);
    }

    #[test]
    fn rx_read_times_out_empty() {
        let (_tx, rx) = crossbeam_channel::bounded::<RxPacket>(16);
        let mut radio_rx = test_rx(rx);
        let mut buf = vec![Complex32::default(); 64];
        let started = Instant::now();
        let result = radio_rx.rx_read(&mut buf, 20_000).unwrap();
        assert_eq!(result.samples_read, 0);
        assert!(started.elapsed() >= Duration::from_millis(19));
    }

    #[test]
    fn rx_read_reports_receiver_queue_drops() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        tx.send(packet(0, 1_000_000, 10, false)).unwrap();
        let mut radio_rx = test_rx(rx);
        radio_rx.queue_gap.store(true, Ordering::Relaxed);
        let mut buf = vec![Complex32::default(); 10];
        let result = radio_rx.rx_read(&mut buf, 10_000).unwrap();
        assert!(result.overflow);
    }

    #[test]
    fn rx_read_discards_packets_queued_before_retune() {
        let (tx, rx) = crossbeam_channel::bounded(16);
        tx.send(packet(0, 1_000_000, 100, false)).unwrap();
        tx.send(packet(1, 1_100_000, 100, false)).unwrap();
        tx.send(packet(2, 1_200_000, 100, false)).unwrap();
        let mut radio_rx = test_rx(rx);
        radio_rx.minimum_time_ns = Some(1_150_000);
        radio_rx.pending_discontinuity = true;
        let mut buf = vec![Complex32::default(); 150];
        let result = radio_rx.rx_read(&mut buf, 0).unwrap();
        assert_eq!(result.samples_read, 150);
        assert_eq!(result.time_ticks, 1_150_000);
        assert!(result.overflow);
        assert_eq!(buf[0].re, 1.0);
        assert_eq!(buf[50].re, 2.0);
    }
}
