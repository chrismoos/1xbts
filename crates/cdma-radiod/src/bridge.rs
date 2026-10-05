use std::net::{SocketAddr, UdpSocket};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};
use std::time::{Duration, Instant};

use cdma_bts::sdr::network::wire::{
    self, FLAG_RX_DISCONTINUITY, FLAG_TX_BLOCK_END, FLAG_TX_DISABLE, FLAG_TX_NO_TICK, PacketHeader,
    StreamKind, decode_header, decode_samples_into, encode_packet, samples_to_ns,
};
use cdma_bts::sdr::{RadioRx, RadioTx, TxRadioHealth};
use log::{debug, info, warn};
use num_complex::Complex32;
use parking_lot::Mutex;
use tokio::sync::oneshot;

const TX_SOCKET_POLL: Duration = Duration::from_micros(250);
const HEARTBEAT_INTERVAL: Duration = Duration::from_millis(100);
const RECV_BUF_LEN: usize = 65536;
const RX_READ_PACKETS: usize = 4;
const RX_READ_TIMEOUT_US: i64 = 100_000;
const IDLE_COMMAND_POLL: Duration = Duration::from_millis(100);
const TX_BLOCK_REORDER_GRACE: Duration = Duration::from_micros(500);
const TX_HEALTH_INTERVAL: Duration = Duration::from_millis(500);
/// Sample flow is the only client liveness signal. TX is disabled after this
/// much silence, so a dead client cannot leave the radio transmitting.
const TX_SILENCE_TIMEOUT: Duration = Duration::from_millis(500);
const TCP_ACCEPT_POLL: Duration = Duration::from_millis(20);
const MAX_TX_BLOCK_SAMPLES: usize = 1 << 20;
/// Queue timed TX ahead of driver blocking so socket buffers do not hold the burst backlog.
const TX_PACKET_QUEUE_CAPACITY: usize = 32_768;

fn block_id_is_newer(candidate: u32, reference: u32) -> bool {
    let forward = candidate.wrapping_sub(reference);
    forward != 0 && forward <= u32::MAX / 2
}

pub fn inner_to_ns(ticks: u64, tick_rate: u64) -> u64 {
    if tick_rate == 0 {
        return 0;
    }
    ((ticks as u128 * 1_000_000_000u128) / tick_rate as u128).min(u64::MAX as u128) as u64
}

pub fn ns_to_inner(ns: u64, tick_rate: u64) -> u64 {
    ((ns as u128 * tick_rate as u128) / 1_000_000_000u128).min(u64::MAX as u128) as u64
}

pub enum TxCommand {
    GetHardwareTime(oneshot::Sender<Result<u64, String>>),
    SetHardwareTime {
        time_ns: u64,
        reply: oneshot::Sender<Result<(), String>>,
    },
    EnableTransmit {
        enable: bool,
        time_ns: Option<u64>,
        reply: oneshot::Sender<Result<(), String>>,
    },
    SetFrequencyHz {
        frequency_hz: f64,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Shutdown,
}

pub enum RxCommand {
    Activate {
        time_ns: Option<u64>,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Deactivate {
        reply: oneshot::Sender<Result<(), String>>,
    },
    SetFrequency {
        frequency_hz: f64,
        reply: oneshot::Sender<Result<(), String>>,
    },
    SetGain {
        gain_db: f64,
        reply: oneshot::Sender<Result<(), String>>,
    },
    Shutdown,
}

#[derive(Debug)]
pub struct CompletedTxBlock {
    pub block_id: u32,
    pub samples: Vec<Complex32>,
    pub time_ns: Option<u64>,
    pub missing_samples: usize,
}

struct PendingTxBlock {
    block_id: u32,
    samples: Vec<Complex32>,
    received: Vec<bool>,
    received_count: usize,
    time_ns: Option<u64>,
    end_seen_at: Option<Instant>,
    last_packet_at: Instant,
}

impl PendingTxBlock {
    fn finish(self) -> CompletedTxBlock {
        CompletedTxBlock {
            block_id: self.block_id,
            missing_samples: self.samples.len().saturating_sub(self.received_count),
            samples: self.samples,
            time_ns: self.time_ns,
        }
    }
}

#[derive(Default)]
struct TxStreamDiag {
    last_seq: Option<u32>,
    lost_packets: u64,
    late_packets: u64,
    reordered_packets: u64,
}

impl TxStreamDiag {
    fn observe(&mut self, seq: u32) {
        if let Some(previous) = self.last_seq {
            let advance = seq.wrapping_sub(previous);
            if advance == 0 || advance > u32::MAX / 2 {
                self.reordered_packets += 1;
                return;
            }
            self.lost_packets += u64::from(advance - 1);
        }
        self.last_seq = Some(seq);
    }

    fn note_late(&mut self) {
        self.late_packets += 1;
    }

    fn summary(&self) -> String {
        format!(
            "lost={} late={} reordered={}",
            self.lost_packets, self.late_packets, self.reordered_packets
        )
    }
}

#[derive(Default)]
pub struct TxBlockAssembler {
    pending: Option<PendingTxBlock>,
    last_completed_id: Option<u32>,
}

impl TxBlockAssembler {
    pub fn reset(&mut self) {
        self.pending = None;
        self.last_completed_id = None;
    }

    pub fn push(
        &mut self,
        header: &PacketHeader,
        packet_samples: &[Complex32],
        sample_rate_hz: u64,
        now: Instant,
    ) -> Result<Vec<CompletedTxBlock>, String> {
        let total = header.block_sample_count as usize;
        let offset = header.block_offset as usize;
        if total == 0 || total > MAX_TX_BLOCK_SAMPLES {
            return Err(format!("invalid TX block sample count {total}"));
        }
        if offset
            .checked_add(packet_samples.len())
            .is_none_or(|end| end > total)
        {
            return Err(format!(
                "TX block range {}..{} exceeds total {total}",
                offset,
                offset.saturating_add(packet_samples.len())
            ));
        }

        let mut completed = Vec::new();
        if self.pending.as_ref().map(|block| block.block_id) != Some(header.block_id) {
            if let Some(pending) = &self.pending {
                if !block_id_is_newer(header.block_id, pending.block_id) {
                    return Ok(completed);
                }
            }
            if let Some(last) = self.last_completed_id {
                if !block_id_is_newer(header.block_id, last) {
                    return Ok(completed);
                }
            }
            if let Some(pending) = self.pending.take() {
                self.last_completed_id = Some(pending.block_id);
                completed.push(pending.finish());
            }
            let time_ns = if header.flags & FLAG_TX_NO_TICK != 0 {
                None
            } else {
                Some(
                    header
                        .time_ns
                        .saturating_sub(samples_to_ns(offset as u64, sample_rate_hz)),
                )
            };
            self.pending = Some(PendingTxBlock {
                block_id: header.block_id,
                samples: vec![Complex32::default(); total],
                received: vec![false; total],
                received_count: 0,
                time_ns,
                end_seen_at: None,
                last_packet_at: now,
            });
        }

        let pending = self.pending.as_mut().expect("TX block created above");
        if pending.samples.len() != total {
            return Err(format!(
                "TX block {} total changed from {} to {total}",
                header.block_id,
                pending.samples.len()
            ));
        }
        pending.last_packet_at = now;
        for (index, sample) in packet_samples.iter().enumerate() {
            let target = offset + index;
            if !pending.received[target] {
                pending.received[target] = true;
                pending.received_count += 1;
                pending.samples[target] = *sample;
            }
        }
        if header.flags & FLAG_TX_BLOCK_END != 0 {
            pending.end_seen_at = Some(now);
        }
        if pending.received_count == pending.samples.len() {
            let pending = self.pending.take().expect("complete TX block");
            self.last_completed_id = Some(pending.block_id);
            completed.push(pending.finish());
        }
        Ok(completed)
    }

    pub fn is_late(&self, block_id: u32) -> bool {
        if self.pending.as_ref().map(|block| block.block_id) == Some(block_id) {
            return false;
        }
        if let Some(pending) = &self.pending {
            if !block_id_is_newer(block_id, pending.block_id) {
                return true;
            }
        }
        if let Some(last) = self.last_completed_id {
            if !block_id_is_newer(block_id, last) {
                return true;
            }
        }
        false
    }

    pub fn pending_shortfall(&self) -> Option<(u32, usize)> {
        self.pending
            .as_ref()
            .map(|block| (block.block_id, block.samples.len() - block.received_count))
    }

    pub fn expire(&mut self, now: Instant) -> Option<CompletedTxBlock> {
        let expired = self
            .pending
            .as_ref()
            .and_then(|block| block.end_seen_at)
            .is_some_and(|end_seen| now.duration_since(end_seen) >= TX_BLOCK_REORDER_GRACE);
        if !expired {
            return None;
        }
        let pending = self.pending.take().expect("expired TX block");
        self.last_completed_id = Some(pending.block_id);
        Some(pending.finish())
    }

    pub fn drop_stalled(&mut self, now: Instant) -> Option<(u32, usize)> {
        let stalled = self.pending.as_ref().is_some_and(|block| {
            block.end_seen_at.is_none()
                && now.duration_since(block.last_packet_at) >= TX_SILENCE_TIMEOUT
        });
        if !stalled {
            return None;
        }
        self.discard_pending()
    }

    /// Later fragments of the discarded block are treated as late.
    pub fn discard_pending(&mut self) -> Option<(u32, usize)> {
        let shortfall = self.pending_shortfall()?;
        self.pending = None;
        self.last_completed_id = Some(shortfall.0);
        Some(shortfall)
    }
}

#[derive(Default)]
struct TxSilenceWatchdog {
    last_activity: Option<Instant>,
}

impl TxSilenceWatchdog {
    fn arm(&mut self, now: Instant) {
        self.last_activity = Some(now);
    }

    fn disarm(&mut self) {
        self.last_activity = None;
    }

    fn note_samples(&mut self, now: Instant) {
        if let Some(last) = self.last_activity.as_mut() {
            *last = now;
        }
    }

    fn expired(&self, now: Instant) -> Option<Duration> {
        self.last_activity
            .map(|last| now.duration_since(last))
            .filter(|silent| *silent >= TX_SILENCE_TIMEOUT)
    }
}

pub struct TxBridge {
    pub session_id: Arc<AtomicU32>,
    pub peer: Arc<Mutex<Option<SocketAddr>>>,
    pub inner_tick_rate: u64,
    pub tx_sample_rate_hz: u64,
    pub tcp_listener: Arc<std::net::TcpListener>,
}

fn submit_tx_block(
    tx: &mut dyn RadioTx,
    block: CompletedTxBlock,
    inner_tick_rate: u64,
    diag: &TxStreamDiag,
) {
    if block.missing_samples > 0 {
        warn!(
            "radiod: TX block {} missing {} of {} samples ({}); zero-filled before driver write",
            block.block_id,
            block.missing_samples,
            block.samples.len(),
            diag.summary()
        );
    }
    let result = match block.time_ns {
        Some(time_ns) => {
            tx.transmit_at(&block.samples, Some(ns_to_inner(time_ns, inner_tick_rate)))
        }
        None => tx.transmit(&block.samples),
    };
    if let Err(e) = result {
        warn!("radiod: transmit block {} failed: {e}", block.block_id);
    }
}

fn report_tx_health(tx: &mut dyn RadioTx, last: &mut TxRadioHealth) {
    let now = match tx.tx_health() {
        Ok(health) => health,
        Err(e) => {
            debug!("radiod: TX status unavailable: {e}");
            return;
        }
    };
    let late = now.late_packets.saturating_sub(last.late_packets);
    let underflows = now.underflows.saturating_sub(last.underflows);
    let sequence_errors = now.sequence_errors.saturating_sub(last.sequence_errors);
    let dropped = now.dropped_packets.saturating_sub(last.dropped_packets);
    let unknown = now.unknown_events.saturating_sub(last.unknown_events);
    if late > 0 {
        warn!(
            "radiod: inner radio discarded {late} late TX burst(s), total {} \
             (underflows {}, sequence errors {})",
            now.late_packets, now.underflows, now.sequence_errors
        );
    } else if underflows > 0 || sequence_errors > 0 || dropped > 0 || unknown > 0 {
        warn!(
            "radiod: TX status underflows +{underflows} sequence_errors +{sequence_errors} \
             dropped +{dropped} unknown +{unknown}"
        );
    }
    *last = now;
}

struct SocketReaderGuard(Arc<AtomicBool>);

impl Drop for SocketReaderGuard {
    fn drop(&mut self) {
        self.0.store(true, Ordering::Relaxed);
    }
}

type PacketReceiver = crossbeam_channel::Receiver<(Vec<u8>, SocketAddr)>;

/// A failed read leaves no packet boundary. Drop the stream before another packet read.
fn read_fully(stream: &mut std::net::TcpStream, buf: &mut [u8], stop: &AtomicBool) -> bool {
    use std::io::Read;
    let mut filled = 0;
    while filled < buf.len() {
        if stop.load(Ordering::Relaxed) {
            return false;
        }
        match stream.read(&mut buf[filled..]) {
            Ok(0) => return false,
            Ok(read) => filled += read,
            Err(e)
                if e.kind() == std::io::ErrorKind::WouldBlock
                    || e.kind() == std::io::ErrorKind::TimedOut =>
            {
                continue;
            }
            Err(e) => {
                warn!("radiod: TX data-plane TCP read failed: {e}");
                return false;
            }
        }
    }
    true
}

/// Take one client's TX packets off a TCP connection. The framing is the same
/// as on the datagram path: a fixed header whose sample count gives the length
/// of the payload behind it.
fn read_tcp_packets(
    mut stream: std::net::TcpStream,
    peer: SocketAddr,
    packets: &crossbeam_channel::Sender<(Vec<u8>, SocketAddr)>,
    stop: &AtomicBool,
) {
    let mut buf = Vec::new();
    loop {
        buf.clear();
        buf.resize(wire::HEADER_LEN, 0);
        if !read_fully(&mut stream, &mut buf, stop) {
            return;
        }
        let payload = match wire::payload_len(&buf) {
            Ok(payload) => payload,
            Err(e) => {
                warn!("radiod: TX data-plane TCP framing lost: {e}");
                return;
            }
        };
        buf.resize(wire::HEADER_LEN + payload, 0);
        if !read_fully(&mut stream, &mut buf[wire::HEADER_LEN..], stop) {
            return;
        }
        let header = match decode_header(&buf) {
            Ok(header) => header,
            Err(e) => {
                warn!("radiod: TX data-plane TCP framing lost: {e}");
                return;
            }
        };
        // The datagram path teaches the server where to send RX and heartbeats.
        // A stream carries samples only, so the client's datagram address is
        // left as the Hello found it.
        if header.stream != StreamKind::TxSamples {
            continue;
        }
        if packets.send((buf.clone(), peer)).is_err() {
            return;
        }
    }
}

fn spawn_tcp_reader(
    listener: Arc<std::net::TcpListener>,
    packets: crossbeam_channel::Sender<(Vec<u8>, SocketAddr)>,
    stop: Arc<AtomicBool>,
) -> std::io::Result<()> {
    listener.set_nonblocking(true)?;
    std::thread::Builder::new()
        .name("radiod-tx-tcp".into())
        .spawn(move || {
            loop {
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                match listener.accept() {
                    Ok((stream, peer)) => {
                        if stream.set_nonblocking(false).is_err() {
                            continue;
                        }
                        let _ = stream.set_nodelay(true);
                        let _ = stream.set_read_timeout(Some(TCP_ACCEPT_POLL));
                        info!("radiod: TX data plane carried over TCP from {peer}");
                        read_tcp_packets(stream, peer, &packets, &stop);
                        info!("radiod: TX data-plane TCP client {peer} went away");
                    }
                    Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                        std::thread::sleep(TCP_ACCEPT_POLL);
                    }
                    Err(e) => {
                        warn!("radiod: TX data-plane accept failed: {e}");
                        return;
                    }
                }
            }
        })?;
    Ok(())
}

fn spawn_socket_reader(
    socket: &UdpSocket,
) -> std::io::Result<(
    PacketReceiver,
    crossbeam_channel::Sender<(Vec<u8>, SocketAddr)>,
    SocketReaderGuard,
)> {
    let stop = Arc::new(AtomicBool::new(false));
    let guard = SocketReaderGuard(Arc::clone(&stop));
    let socket = socket.try_clone()?;
    socket.set_read_timeout(Some(IDLE_COMMAND_POLL))?;
    let (tx, rx) = crossbeam_channel::bounded(TX_PACKET_QUEUE_CAPACITY);
    let shared_sender = tx.clone();
    std::thread::Builder::new()
        .name("radiod-tx-recv".into())
        .spawn(move || {
            let mut buf = vec![0u8; RECV_BUF_LEN];
            let mut dropped: u64 = 0;
            loop {
                if stop.load(Ordering::Relaxed) {
                    return;
                }
                let (len, from) = match socket.recv_from(&mut buf) {
                    Ok(ok) => ok,
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            || e.kind() == std::io::ErrorKind::TimedOut =>
                    {
                        if stop.load(Ordering::Relaxed) {
                            return;
                        }
                        continue;
                    }
                    Err(e) => {
                        warn!("radiod: data socket recv failed: {e}");
                        return;
                    }
                };
                match tx.try_send((buf[..len].to_vec(), from)) {
                    Ok(()) => {}
                    Err(crossbeam_channel::TrySendError::Full(_)) => {
                        dropped += 1;
                        if dropped.is_power_of_two() {
                            warn!("radiod: TX packet queue full, dropped {dropped} packets total");
                        }
                    }
                    Err(crossbeam_channel::TrySendError::Disconnected(_)) => return,
                }
            }
        })?;
    Ok((rx, shared_sender, guard))
}

impl TxBridge {
    pub fn run(
        self,
        mut tx: Box<dyn RadioTx>,
        socket: UdpSocket,
        cmds: crossbeam_channel::Receiver<TxCommand>,
    ) {
        let (packets, packet_sender, _reader_guard) = match spawn_socket_reader(&socket) {
            Ok(reader) => reader,
            Err(e) => {
                warn!("radiod: tx bridge could not start its socket reader: {e}");
                return;
            }
        };
        let tcp_stop = Arc::new(AtomicBool::new(false));
        let _tcp_guard = SocketReaderGuard(Arc::clone(&tcp_stop));
        if let Err(e) = spawn_tcp_reader(
            Arc::clone(&self.tcp_listener),
            packet_sender,
            Arc::clone(&tcp_stop),
        ) {
            warn!("radiod: tx bridge could not accept stream clients: {e}");
        }
        let mut samples: Vec<Complex32> = Vec::new();
        let mut scratch: Vec<u8> = Vec::new();
        let mut assembler = TxBlockAssembler::default();
        let mut diag = TxStreamDiag::default();
        let mut last_session = self.session_id.load(Ordering::Relaxed);
        let mut last_heartbeat = Instant::now() - HEARTBEAT_INTERVAL;
        let mut heartbeat_seq: u32 = 0;
        let mut last_health = TxRadioHealth::default();
        let mut last_health_poll = Instant::now();
        let mut watchdog = TxSilenceWatchdog::default();

        loop {
            loop {
                match cmds.try_recv() {
                    Ok(TxCommand::GetHardwareTime(reply)) => {
                        let result = tx
                            .get_hardware_time()
                            .map(|ticks| inner_to_ns(ticks, self.inner_tick_rate))
                            .map_err(|e| e.to_string());
                        let _ = reply.send(result);
                    }
                    Ok(TxCommand::SetHardwareTime { time_ns, reply }) => {
                        let result = tx
                            .set_hardware_time(ns_to_inner(time_ns, self.inner_tick_rate))
                            .map_err(|e| e.to_string());
                        let _ = reply.send(result);
                    }
                    Ok(TxCommand::EnableTransmit {
                        enable,
                        time_ns,
                        reply,
                    }) => {
                        let tick = time_ns.map(|ns| ns_to_inner(ns, self.inner_tick_rate));
                        let result = tx
                            .enable_transmit_at(enable, tick)
                            .map_err(|e| e.to_string());
                        if !enable {
                            watchdog.disarm();
                        } else if result.is_ok() {
                            watchdog.arm(Instant::now());
                        }
                        let _ = reply.send(result);
                    }
                    Ok(TxCommand::SetFrequencyHz {
                        frequency_hz,
                        reply,
                    }) => {
                        let result = tx
                            .set_tx_frequency_hz(frequency_hz)
                            .map_err(|e| e.to_string());
                        let _ = reply.send(result);
                    }
                    Ok(TxCommand::Shutdown)
                    | Err(crossbeam_channel::TryRecvError::Disconnected) => {
                        return;
                    }
                    Err(crossbeam_channel::TryRecvError::Empty) => break,
                }
            }

            let now = Instant::now();
            if let Some(block) = assembler.expire(now) {
                submit_tx_block(&mut *tx, block, self.inner_tick_rate, &diag);
            }
            if let Some((block_id, shortfall)) = assembler.drop_stalled(now) {
                warn!(
                    "radiod: dropped TX block {block_id} ({shortfall} samples short), no END packet within {} ms ({})",
                    TX_SILENCE_TIMEOUT.as_millis(),
                    diag.summary()
                );
            }
            if let Some(silent) = watchdog.expired(now) {
                watchdog.disarm();
                warn!(
                    "radiod: no TX samples for {} ms while transmit enabled, disabling transmit",
                    silent.as_millis()
                );
                if let Err(e) = tx.enable_transmit(false) {
                    warn!("radiod: TX silence disable failed: {e}");
                }
            }

            if last_health_poll.elapsed() >= TX_HEALTH_INTERVAL {
                last_health_poll = Instant::now();
                report_tx_health(&mut *tx, &mut last_health);
            }

            let session = self.session_id.load(Ordering::Relaxed);
            if session != last_session {
                last_session = session;
                assembler.reset();
                diag = TxStreamDiag::default();
            }

            if last_heartbeat.elapsed() >= HEARTBEAT_INTERVAL {
                let peer = *self.peer.lock();
                if let Some(peer) = peer {
                    match tx.get_hardware_time() {
                        Ok(ticks) => {
                            encode_packet(
                                &PacketHeader {
                                    stream: StreamKind::ClockHeartbeat,
                                    flags: 0,
                                    session_id: session,
                                    seq: heartbeat_seq,
                                    time_ns: inner_to_ns(ticks, self.inner_tick_rate),
                                    sample_count: 0,
                                    block_id: 0,
                                    block_offset: 0,
                                    block_sample_count: 0,
                                },
                                &[],
                                &mut scratch,
                            );
                            heartbeat_seq = heartbeat_seq.wrapping_add(1);
                            if let Err(e) = socket.send_to(&scratch, peer) {
                                debug!("radiod: heartbeat send failed: {e}");
                            }
                        }
                        Err(e) => debug!("radiod: heartbeat time read failed: {e}"),
                    }
                }
                last_heartbeat = Instant::now();
            }

            let (buf, from) = match packets.recv_timeout(TX_SOCKET_POLL) {
                Ok(packet) => packet,
                Err(crossbeam_channel::RecvTimeoutError::Timeout) => continue,
                Err(crossbeam_channel::RecvTimeoutError::Disconnected) => {
                    warn!("radiod: data socket reader stopped");
                    return;
                }
            };
            let len = buf.len();
            let header = match decode_header(&buf[..len]) {
                Ok(header) => header,
                Err(e) => {
                    debug!("radiod: dropping undecodable packet: {e}");
                    continue;
                }
            };
            if header.session_id != session {
                continue;
            }
            match header.stream {
                StreamKind::Hello => {
                    let mut peer = self.peer.lock();
                    if *peer != Some(from) {
                        info!("radiod: data-plane client is {from}");
                        *peer = Some(from);
                    }
                    last_heartbeat = Instant::now() - HEARTBEAT_INTERVAL;
                }
                StreamKind::TxSamples => {
                    diag.observe(header.seq);
                    if header.flags & FLAG_TX_DISABLE != 0 {
                        if header.sample_count != 0 {
                            warn!(
                                "radiod: dropping TX disable packet with {} samples",
                                header.sample_count
                            );
                            continue;
                        }
                        if let Some(block) = assembler.expire(Instant::now()) {
                            submit_tx_block(&mut *tx, block, self.inner_tick_rate, &diag);
                        }
                        if let Some((block_id, shortfall)) = assembler.discard_pending() {
                            warn!(
                                "radiod: TX disable reached incomplete block {block_id} ({shortfall} samples short), dropping it"
                            );
                        }
                        watchdog.disarm();
                        if let Err(e) = tx.enable_transmit(false) {
                            warn!("radiod: ordered TX disable failed: {e}");
                        }
                        continue;
                    }
                    watchdog.note_samples(Instant::now());
                    if assembler.is_late(header.block_id) {
                        diag.note_late();
                        if let Some((pending_id, shortfall)) = assembler.pending_shortfall() {
                            debug!(
                                "radiod: TX packet seq {} for block {} arrived after that block went out, assembling {} ({} samples short)",
                                header.seq, header.block_id, pending_id, shortfall
                            );
                        }
                    }
                    decode_samples_into(&buf[..len], &header, &mut samples);
                    match assembler.push(&header, &samples, self.tx_sample_rate_hz, Instant::now())
                    {
                        Ok(blocks) => {
                            for block in blocks {
                                submit_tx_block(&mut *tx, block, self.inner_tick_rate, &diag);
                            }
                        }
                        Err(e) => warn!("radiod: dropping invalid TX packet: {e}"),
                    }
                }
                StreamKind::RxSamples | StreamKind::ClockHeartbeat => {}
            }
        }
    }
}

pub struct RxBridge {
    pub session_id: Arc<AtomicU32>,
    pub peer: Arc<Mutex<Option<SocketAddr>>>,
    pub inner_tick_rate: u64,
    pub rx_sample_rate_hz: Arc<AtomicU32>,
    pub samples_per_packet: usize,
}

impl RxBridge {
    pub fn run(
        self,
        mut rx: Box<dyn RadioRx>,
        socket: UdpSocket,
        cmds: crossbeam_channel::Receiver<RxCommand>,
    ) {
        let mut buf = vec![Complex32::default(); self.samples_per_packet * RX_READ_PACKETS];
        let mut scratch: Vec<u8> = Vec::new();
        let mut seq: u32 = 0;
        let mut active = false;

        loop {
            if active {
                loop {
                    match cmds.try_recv() {
                        Ok(cmd) => {
                            if self.handle_command(cmd, &mut rx, &mut active) {
                                return;
                            }
                        }
                        Err(crossbeam_channel::TryRecvError::Empty) => break,
                        Err(crossbeam_channel::TryRecvError::Disconnected) => return,
                    }
                }
                if !active {
                    continue;
                }
                let result = match rx.rx_read(&mut buf, RX_READ_TIMEOUT_US) {
                    Ok(result) => result,
                    Err(e) => {
                        warn!("radiod: rx_read failed: {e}");
                        std::thread::sleep(Duration::from_millis(10));
                        continue;
                    }
                };
                if result.samples_read == 0 {
                    continue;
                }
                let peer = *self.peer.lock();
                let Some(peer) = peer else { continue };
                let session = self.session_id.load(Ordering::Relaxed);
                let rate = self.rx_sample_rate_hz.load(Ordering::Relaxed) as u64;
                let read_ns = inner_to_ns(result.time_ticks, self.inner_tick_rate);
                let mut offset = 0usize;
                while offset < result.samples_read {
                    let end = (offset + self.samples_per_packet).min(result.samples_read);
                    let flags = if offset == 0 && result.overflow {
                        FLAG_RX_DISCONTINUITY
                    } else {
                        0
                    };
                    encode_packet(
                        &PacketHeader {
                            stream: StreamKind::RxSamples,
                            flags,
                            session_id: session,
                            seq,
                            time_ns: read_ns.saturating_add(samples_to_ns(offset as u64, rate)),
                            sample_count: (end - offset) as u16,
                            block_id: 0,
                            block_offset: 0,
                            block_sample_count: 0,
                        },
                        &buf[offset..end],
                        &mut scratch,
                    );
                    seq = seq.wrapping_add(1);
                    if let Err(e) = socket.send_to(&scratch, peer) {
                        debug!("radiod: rx send failed: {e}");
                        break;
                    }
                    offset = end;
                }
            } else {
                match cmds.recv_timeout(IDLE_COMMAND_POLL) {
                    Ok(cmd) => {
                        if self.handle_command(cmd, &mut rx, &mut active) {
                            return;
                        }
                    }
                    Err(crossbeam_channel::RecvTimeoutError::Timeout) => {}
                    Err(crossbeam_channel::RecvTimeoutError::Disconnected) => return,
                }
            }
        }
    }

    fn handle_command(&self, cmd: RxCommand, rx: &mut Box<dyn RadioRx>, active: &mut bool) -> bool {
        match cmd {
            RxCommand::Activate { time_ns, reply } => {
                let tick = time_ns.map(|ns| ns_to_inner(ns, self.inner_tick_rate));
                let result = rx.rx_activate(tick).map_err(|e| e.to_string());
                if result.is_ok() {
                    *active = true;
                }
                let _ = reply.send(result);
                false
            }
            RxCommand::SetFrequency {
                frequency_hz,
                reply,
            } => {
                let result = rx.set_rx_frequency(frequency_hz).map_err(|e| e.to_string());
                let _ = reply.send(result);
                false
            }
            RxCommand::SetGain { gain_db, reply } => {
                let result = rx.set_rx_gain(gain_db).map_err(|e| e.to_string());
                let _ = reply.send(result);
                false
            }
            RxCommand::Deactivate { reply } => {
                let result = rx.rx_deactivate().map_err(|e| e.to_string());
                *active = false;
                let _ = reply.send(result);
                false
            }
            RxCommand::Shutdown => true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cdma_bts::sdr::network::wire::{FLAG_TX_BLOCK_END, FLAG_TX_BLOCK_START};

    const RATE: u64 = 4_915_200;

    fn tx_header(
        block_id: u32,
        offset: u32,
        total: u32,
        flags: u16,
        start_ns: u64,
        count: usize,
    ) -> PacketHeader {
        PacketHeader {
            stream: StreamKind::TxSamples,
            flags,
            session_id: 1,
            seq: offset,
            time_ns: start_ns + samples_to_ns(offset as u64, RATE),
            sample_count: count as u16,
            block_id,
            block_offset: offset,
            block_sample_count: total,
        }
    }

    #[test]
    fn assembler_reorders_packets_and_submits_one_complete_block() {
        let now = Instant::now();
        let start_ns = 10_000_000;
        let mut assembler = TxBlockAssembler::default();
        let tail = [Complex32::new(3.0, 0.0), Complex32::new(4.0, 0.0)];
        let head = [Complex32::new(1.0, 0.0), Complex32::new(2.0, 0.0)];

        assert!(
            assembler
                .push(
                    &tx_header(7, 2, 4, FLAG_TX_BLOCK_END, start_ns, tail.len()),
                    &tail,
                    RATE,
                    now,
                )
                .unwrap()
                .is_empty()
        );
        let completed = assembler
            .push(
                &tx_header(7, 0, 4, FLAG_TX_BLOCK_START, start_ns, head.len()),
                &head,
                RATE,
                now,
            )
            .unwrap();
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].missing_samples, 0);
        assert_eq!(completed[0].time_ns, Some(start_ns));
        assert_eq!(completed[0].samples, [head, tail].concat());
    }

    #[test]
    fn assembler_ignores_duplicates_and_zero_fills_loss_after_grace() {
        let now = Instant::now();
        let start_ns = 20_000_000;
        let mut assembler = TxBlockAssembler::default();
        let head = [Complex32::new(1.0, 0.0), Complex32::new(2.0, 0.0)];
        let header = tx_header(
            8,
            0,
            4,
            FLAG_TX_BLOCK_START | FLAG_TX_BLOCK_END,
            start_ns,
            head.len(),
        );
        assert!(
            assembler
                .push(&header, &head, RATE, now)
                .unwrap()
                .is_empty()
        );
        assert!(
            assembler
                .push(&header, &head, RATE, now)
                .unwrap()
                .is_empty()
        );
        let block = assembler
            .expire(now + TX_BLOCK_REORDER_GRACE)
            .expect("incomplete block expires");
        assert_eq!(block.missing_samples, 2);
        assert_eq!(&block.samples[..2], &head);
        assert_eq!(&block.samples[2..], &[Complex32::default(); 2]);
    }

    #[test]
    fn assembler_ignores_late_block_without_displacing_pending_block() {
        let now = Instant::now();
        let mut assembler = TxBlockAssembler::default();
        let sample = [Complex32::new(9.0, 0.0)];

        assembler
            .push(
                &tx_header(10, 0, 2, FLAG_TX_BLOCK_START, 30_000_000, 1),
                &sample,
                RATE,
                now,
            )
            .unwrap();
        let completed = assembler
            .push(
                &tx_header(
                    9,
                    0,
                    1,
                    FLAG_TX_BLOCK_START | FLAG_TX_BLOCK_END,
                    20_000_000,
                    1,
                ),
                &sample,
                RATE,
                now,
            )
            .unwrap();
        assert!(completed.is_empty());

        let tail = [Complex32::new(10.0, 0.0)];
        let completed = assembler
            .push(
                &tx_header(10, 1, 2, FLAG_TX_BLOCK_END, 30_000_000, 1),
                &tail,
                RATE,
                now,
            )
            .unwrap();
        assert_eq!(completed.len(), 1);
        assert_eq!(completed[0].block_id, 10);
        assert_eq!(completed[0].samples, [sample, tail].concat());
    }

    #[test]
    fn assembler_rejects_fragment_outside_declared_block() {
        let mut assembler = TxBlockAssembler::default();
        let samples = [Complex32::new(1.0, 0.0); 2];
        let error = assembler
            .push(
                &tx_header(11, 3, 4, FLAG_TX_BLOCK_END, 40_000_000, samples.len()),
                &samples,
                RATE,
                Instant::now(),
            )
            .expect_err("range must be rejected");
        assert!(error.contains("exceeds total 4"), "{error}");
    }

    #[test]
    fn assembler_drops_block_without_end_after_silence_timeout() {
        let now = Instant::now();
        let mut assembler = TxBlockAssembler::default();
        let head = [Complex32::new(1.0, 0.0); 2];
        assembler
            .push(
                &tx_header(12, 0, 4, FLAG_TX_BLOCK_START, 50_000_000, head.len()),
                &head,
                RATE,
                now,
            )
            .unwrap();
        assert_eq!(assembler.drop_stalled(now + TX_BLOCK_REORDER_GRACE), None);
        assert_eq!(
            assembler.drop_stalled(now + TX_SILENCE_TIMEOUT),
            Some((12, 2))
        );
        assert_eq!(assembler.pending_shortfall(), None);
        assert!(assembler.is_late(12));
    }

    #[test]
    fn assembler_discard_pending_clears_incomplete_block() {
        let mut assembler = TxBlockAssembler::default();
        let head = [Complex32::new(1.0, 0.0)];
        assembler
            .push(
                &tx_header(13, 0, 3, FLAG_TX_BLOCK_START, 60_000_000, head.len()),
                &head,
                RATE,
                Instant::now(),
            )
            .unwrap();
        assert_eq!(assembler.discard_pending(), Some((13, 2)));
        assert_eq!(assembler.discard_pending(), None);
    }

    #[test]
    fn silence_watchdog_fires_only_while_armed_and_quiet() {
        let start = Instant::now();
        let mut watchdog = TxSilenceWatchdog::default();
        assert_eq!(watchdog.expired(start + TX_SILENCE_TIMEOUT * 2), None);

        watchdog.arm(start);
        let half = TX_SILENCE_TIMEOUT / 2;
        assert_eq!(watchdog.expired(start + half), None);
        watchdog.note_samples(start + half);
        assert_eq!(watchdog.expired(start + TX_SILENCE_TIMEOUT), None);
        assert_eq!(
            watchdog.expired(start + half + TX_SILENCE_TIMEOUT),
            Some(TX_SILENCE_TIMEOUT)
        );

        watchdog.disarm();
        watchdog.note_samples(start);
        assert_eq!(watchdog.expired(start + TX_SILENCE_TIMEOUT * 4), None);
    }

    #[test]
    fn tick_conversions_round_trip_within_one_tick() {
        for tick_rate in [49_152_000u64, 4_915_200, 1_000_000_000] {
            let ns = 123_456_789_012u64;
            let inner = ns_to_inner(ns, tick_rate);
            let back = inner_to_ns(inner, tick_rate);
            let one_tick_ns = 1_000_000_000 / tick_rate + 1;
            assert!(ns.abs_diff(back) <= one_tick_ns, "tick_rate={tick_rate}");
        }
    }
}
