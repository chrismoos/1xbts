use parking_lot::Mutex;
use std::collections::BTreeMap;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU16, AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use cdma_voice::evrc::{EvrcDecoder, EvrcEncoder};
use cdma_voice::evrc_b_wb::{EvrcBDecoder, EvrcBEncoder, EvrcWbDecoder, EvrcWbEncoder};
use cdma_voice::{VoiceCodec, VoiceRate};
use tokio::net::{UdpSocket, lookup_host};
use tokio::sync::broadcast;
use tokio::time::{MissedTickBehavior, interval, sleep, timeout};

use crate::proto::{GatewayVoiceFrame, VoiceFrameRate};
use crate::stun;

const RTP_HEADER_LEN: usize = 12;
const RTP_TIMESTAMP_STEP_8KHZ_20MS: u32 = 160;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum G711Codec {
    Pcmu,
    Pcma,
}

impl G711Codec {
    pub fn payload_type(self) -> u8 {
        match self {
            Self::Pcmu => 0,
            Self::Pcma => 8,
        }
    }

    pub fn from_name(name: &str) -> Option<Self> {
        match name.to_ascii_uppercase().as_str() {
            "PCMU" | "ULAW" | "MULAW" => Some(Self::Pcmu),
            "PCMA" | "ALAW" => Some(Self::Pcma),
            _ => None,
        }
    }

    fn from_payload_type(payload_type: u8) -> Option<Self> {
        match payload_type {
            0 => Some(Self::Pcmu),
            8 => Some(Self::Pcma),
            _ => None,
        }
    }
}

enum GatewayVoiceDecoder {
    EvrcA(EvrcDecoder),
    EvrcB(EvrcBDecoder),
    EvrcWb(EvrcWbDecoder),
}

impl GatewayVoiceDecoder {
    fn new(codec: VoiceCodec) -> Result<Self, String> {
        match codec {
            VoiceCodec::EvrcA => EvrcDecoder::new()
                .map(Self::EvrcA)
                .map_err(|err| format!("EVRC-A decoder init: {err}")),
            VoiceCodec::EvrcB => EvrcBDecoder::new()
                .map(Self::EvrcB)
                .map_err(|err| format!("EVRC-B decoder init: {err}")),
            VoiceCodec::EvrcWb => EvrcWbDecoder::new()
                .map(Self::EvrcWb)
                .map_err(|err| format!("EVRC-WB decoder init: {err}")),
        }
    }

    fn decode(&mut self, rate: VoiceRate, packet: &[u8]) -> Result<[i16; 160], String> {
        match self {
            Self::EvrcA(decoder) => decoder.decode(packet),
            Self::EvrcB(decoder) => decoder.decode(rate, packet),
            Self::EvrcWb(decoder) => decoder.decode_to_8k(rate, packet),
        }
    }
}

enum GatewayVoiceEncoder {
    EvrcA(EvrcEncoder),
    EvrcB(EvrcBEncoder),
    EvrcWb(EvrcWbEncoder),
}

impl GatewayVoiceEncoder {
    fn new(codec: VoiceCodec) -> Result<Self, String> {
        match codec {
            VoiceCodec::EvrcA => EvrcEncoder::new()
                .map(Self::EvrcA)
                .map_err(|err| format!("EVRC-A encoder init: {err}")),
            VoiceCodec::EvrcB => EvrcBEncoder::new()
                .map(Self::EvrcB)
                .map_err(|err| format!("EVRC-B encoder init: {err}")),
            VoiceCodec::EvrcWb => EvrcWbEncoder::new()
                .map(Self::EvrcWb)
                .map_err(|err| format!("EVRC-WB encoder init: {err}")),
        }
    }

    fn encode(&mut self, pcm: &[i16; 160]) -> Result<(VoiceRate, Vec<u8>), String> {
        match self {
            Self::EvrcA(encoder) => encoder.encode(pcm),
            Self::EvrcB(encoder) => encoder.encode(pcm),
            Self::EvrcWb(encoder) => encoder.encode_8k_input(pcm),
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RtpMediaStats {
    pub session_id: String,
    pub local_addr: Option<SocketAddr>,
    pub remote_addr: Option<SocketAddr>,
    pub codec: G711Codec,
    pub voice_codec: VoiceCodec,
    pub duration_ms: u128,
    pub idle_ms: u128,
    pub rtp_tx_packets: u64,
    pub rtp_tx_silence_packets: u64,
    pub rtp_rx_packets: u64,
    pub rtp_rx_dropped_packets: u64,
    pub rtp_late_or_duplicate_packets: u64,
    pub rtp_missing_packets: u64,
    pub gateway_voice_frames: u64,
    pub gateway_silence_frames: u64,
    pub evrc_decode_failures: u64,
    pub erasure_frames: u64,
}

pub struct RtpMediaSession {
    session_id: String,
    socket: Arc<UdpSocket>,
    remote_addr: Mutex<Option<SocketAddr>>,
    codec: Mutex<G711Codec>,
    voice_codec: VoiceCodec,
    decoder: Mutex<GatewayVoiceDecoder>,
    encoder: Mutex<GatewayVoiceEncoder>,
    gateway_voice_tx: broadcast::Sender<GatewayVoiceFrame>,
    jitter_buffer_ms: u64,
    log_media_frames: bool,
    log_media_summary: bool,
    created_at: Instant,
    last_activity_at: Mutex<Instant>,
    summary_logged: AtomicBool,
    send_sequence: AtomicU16,
    send_timestamp: AtomicU32,
    rtp_tx_packets: AtomicU64,
    rtp_tx_silence_packets: AtomicU64,
    rtp_rx_packets: AtomicU64,
    rtp_rx_dropped_packets: AtomicU64,
    rtp_late_or_duplicate_packets: AtomicU64,
    rtp_missing_packets: AtomicU64,
    gateway_silence_frames: AtomicU64,
    evrc_decode_failures: AtomicU64,
    erasure_frames: AtomicU64,
    recv_sequence: AtomicU64,
    ssrc: u32,
    /// RFC 4733 §2.5.1.2 — captured at the start of an in-flight DTMF event
    /// so every packet of the event carries the same RTP timestamp.
    dtmf_event_timestamp: Mutex<Option<u32>>,
    telephone_event_payload_type: Option<u8>,
}

impl RtpMediaSession {
    pub async fn bind(
        session_id: String,
        listen_addr: &str,
        port: u16,
        initial_codec: G711Codec,
        voice_codec: VoiceCodec,
        jitter_buffer_ms: u64,
        log_media_frames: bool,
        log_media_summary: bool,
        gateway_voice_tx: broadcast::Sender<GatewayVoiceFrame>,
        telephone_event_payload_type: Option<u8>,
    ) -> Result<Arc<Self>, String> {
        let bind_addr: SocketAddr = format!("{listen_addr}:{port}")
            .parse()
            .map_err(|err| format!("invalid RTP bind address {listen_addr}:{port}: {err}"))?;
        let socket = UdpSocket::bind(bind_addr)
            .await
            .map_err(|err| format!("failed to bind RTP socket {bind_addr}: {err}"))?;
        let decoder = GatewayVoiceDecoder::new(voice_codec)?;
        let encoder = GatewayVoiceEncoder::new(voice_codec)?;
        let ssrc = stable_ssrc(&session_id);
        let now = Instant::now();

        Ok(Arc::new(Self {
            session_id,
            socket: Arc::new(socket),
            remote_addr: Mutex::new(None),
            codec: Mutex::new(initial_codec),
            voice_codec,
            decoder: Mutex::new(decoder),
            encoder: Mutex::new(encoder),
            gateway_voice_tx,
            jitter_buffer_ms,
            log_media_frames,
            log_media_summary,
            created_at: now,
            last_activity_at: Mutex::new(now),
            summary_logged: AtomicBool::new(false),
            send_sequence: AtomicU16::new(1),
            send_timestamp: AtomicU32::new(0),
            rtp_tx_packets: AtomicU64::new(0),
            rtp_tx_silence_packets: AtomicU64::new(0),
            rtp_rx_packets: AtomicU64::new(0),
            rtp_rx_dropped_packets: AtomicU64::new(0),
            rtp_late_or_duplicate_packets: AtomicU64::new(0),
            rtp_missing_packets: AtomicU64::new(0),
            gateway_silence_frames: AtomicU64::new(0),
            evrc_decode_failures: AtomicU64::new(0),
            erasure_frames: AtomicU64::new(0),
            recv_sequence: AtomicU64::new(0),
            ssrc,
            dtmf_event_timestamp: Mutex::new(None),
            telephone_event_payload_type,
        }))
    }

    pub fn local_addr(&self) -> Option<SocketAddr> {
        self.socket.local_addr().ok()
    }

    pub async fn discover_mapped_addr(
        &self,
        stun_server: &str,
        timeout_duration: Duration,
    ) -> Result<SocketAddr, String> {
        let stun_addrs = lookup_host(stun_server)
            .await
            .map_err(|err| format!("failed to resolve STUN server {stun_server}: {err}"))?
            .collect::<Vec<_>>();
        let stun_addr = select_stun_addr(&stun_addrs, self.local_addr())
            .ok_or_else(|| format!("STUN server {stun_server} did not resolve"))?;
        let transaction_id = self.stun_transaction_id();
        let request = stun::binding_request(transaction_id);

        self.socket
            .send_to(&request, stun_addr)
            .await
            .map_err(|err| format!("STUN send to {stun_addr} failed: {err}"))?;

        let mut buf = vec![0u8; 2048];
        timeout(timeout_duration, async {
            loop {
                let (len, from) = self
                    .socket
                    .recv_from(&mut buf)
                    .await
                    .map_err(|err| format!("STUN receive failed: {err}"))?;

                if from != stun_addr || !stun::is_stun_message(&buf[..len]) {
                    continue;
                }

                match stun::parse_binding_response(&buf[..len], transaction_id) {
                    Ok(mapped) => return Ok(mapped),
                    Err(err) if err.contains("transaction ID mismatch") => continue,
                    Err(err) => return Err(err),
                }
            }
        })
        .await
        .map_err(|_| format!("STUN request to {stun_addr} timed out"))?
    }

    pub fn set_remote_from_sdp(&self, sdp: &[u8], codec: G711Codec) -> Result<SocketAddr, String> {
        let remote = parse_sdp_rtp_endpoint_result(sdp)?;
        *self.remote_addr.lock() = Some(remote);
        *self.codec.lock() = codec;
        Ok(remote)
    }

    pub fn last_activity_elapsed(&self) -> Duration {
        self.last_activity_at.lock().elapsed()
    }

    pub fn stats(&self) -> RtpMediaStats {
        let last_activity_at = *self.last_activity_at.lock();
        RtpMediaStats {
            session_id: self.session_id.clone(),
            local_addr: self.local_addr(),
            remote_addr: *self.remote_addr.lock(),
            codec: *self.codec.lock(),
            voice_codec: self.voice_codec,
            duration_ms: self.created_at.elapsed().as_millis(),
            idle_ms: last_activity_at.elapsed().as_millis(),
            rtp_tx_packets: self.rtp_tx_packets.load(Ordering::Relaxed),
            rtp_tx_silence_packets: self.rtp_tx_silence_packets.load(Ordering::Relaxed),
            rtp_rx_packets: self.rtp_rx_packets.load(Ordering::Relaxed),
            rtp_rx_dropped_packets: self.rtp_rx_dropped_packets.load(Ordering::Relaxed),
            rtp_late_or_duplicate_packets: self
                .rtp_late_or_duplicate_packets
                .load(Ordering::Relaxed),
            rtp_missing_packets: self.rtp_missing_packets.load(Ordering::Relaxed),
            gateway_voice_frames: self.recv_sequence.load(Ordering::Relaxed),
            gateway_silence_frames: self.gateway_silence_frames.load(Ordering::Relaxed),
            evrc_decode_failures: self.evrc_decode_failures.load(Ordering::Relaxed),
            erasure_frames: self.erasure_frames.load(Ordering::Relaxed),
        }
    }

    pub fn log_summary_once(&self, reason: &str) -> Option<RtpMediaStats> {
        if self.summary_logged.swap(true, Ordering::SeqCst) {
            return None;
        }

        let stats = self.stats();
        if self.log_media_summary {
            log::info!(
                "VoiceGW RTP media summary session={} reason={} duration_ms={} idle_ms={} local={} remote={} codec={:?} rtp_tx={} rtp_tx_silence={} rtp_rx={} rtp_rx_dropped={} jitter_late_or_duplicate={} jitter_missing={} gateway_frames={} gateway_silence={} evrc_decode_failures={} erasure_frames={}",
                stats.session_id,
                reason,
                stats.duration_ms,
                stats.idle_ms,
                stats
                    .local_addr
                    .map(|addr| addr.to_string())
                    .unwrap_or_else(|| "<unknown>".to_string()),
                stats
                    .remote_addr
                    .map(|addr| addr.to_string())
                    .unwrap_or_else(|| "<unset>".to_string()),
                stats.codec,
                stats.rtp_tx_packets,
                stats.rtp_tx_silence_packets,
                stats.rtp_rx_packets,
                stats.rtp_rx_dropped_packets,
                stats.rtp_late_or_duplicate_packets,
                stats.rtp_missing_packets,
                stats.gateway_voice_frames,
                stats.gateway_silence_frames,
                stats.evrc_decode_failures,
                stats.erasure_frames
            );
        }
        Some(stats)
    }

    pub async fn send_latch_frames(&self, count: u8, interval: Duration) -> Result<(), String> {
        for idx in 0..count {
            self.send_silence_frame().await?;
            if idx + 1 < count {
                sleep(interval).await;
            }
        }
        Ok(())
    }

    pub async fn send_air_frame(
        &self,
        bits: &[u8],
        rate_bps: u32,
        service_option: u32,
    ) -> Result<(), String> {
        let remote = match *self.remote_addr.lock() {
            Some(remote) => remote,
            None => return Ok(()),
        };
        let codec = *self.codec.lock();
        if rate_bps == 0 {
            self.erasure_frames.fetch_add(1, Ordering::Relaxed);
            return Ok(());
        }
        if service_option != 0 && service_option != u32::from(self.voice_codec.service_option()) {
            log::debug!(
                "VoiceGW media service option differs from session codec session={} frame_so={} session_so={}",
                self.session_id,
                service_option,
                self.voice_codec.service_option()
            );
        }
        let voice_rate = voice_rate_for_rate_bps(rate_bps)?;
        let evrc_packet = evrc_bits_to_packet_bytes(bits, rate_bps)?;
        let (pcm, sent_silence) = {
            let mut decoder = self.decoder.lock();
            match decoder.decode(voice_rate, &evrc_packet) {
                Ok(pcm) => (pcm, false),
                Err(err) => {
                    let failures = self.evrc_decode_failures.fetch_add(1, Ordering::Relaxed) + 1;
                    if self.log_media_frames || should_log_media_counter(failures) {
                        log::warn!(
                            "VoiceGW EVRC decode failed; sending RTP silence session={} failures={} rate_bps={} bits={} packet_bytes={}: {}",
                            self.session_id,
                            failures,
                            rate_bps,
                            bits.len(),
                            evrc_packet.len(),
                            err
                        );
                    }
                    ([0i16; 160], true)
                }
            }
        };
        let payload = encode_g711_frame(codec, &pcm);
        let sequence = self.send_sequence.fetch_add(1, Ordering::Relaxed);
        let timestamp = self
            .send_timestamp
            .fetch_add(RTP_TIMESTAMP_STEP_8KHZ_20MS, Ordering::Relaxed);
        let rtp = build_rtp_packet(
            codec.payload_type(),
            sequence,
            timestamp,
            self.ssrc,
            &payload,
        );

        self.socket
            .send_to(&rtp, remote)
            .await
            .map_err(|err| format!("RTP send to {remote} failed: {err}"))?;
        let tx_count = self.rtp_tx_packets.fetch_add(1, Ordering::Relaxed) + 1;
        if sent_silence {
            self.rtp_tx_silence_packets.fetch_add(1, Ordering::Relaxed);
        }
        if self.log_media_frames && should_log_media_counter(tx_count) {
            log::debug!(
                "VoiceGW RTP tx session={} count={} to={} payload_type={} seq={} ts={} payload_bytes={} source_rate_bps={} source_bits={}",
                self.session_id,
                tx_count,
                remote,
                codec.payload_type(),
                sequence,
                timestamp,
                payload.len(),
                rate_bps,
                bits.len()
            );
        }
        self.mark_activity();
        Ok(())
    }

    /// Emits one RFC 4733 telephone-event RTP packet on the session's SIP
    /// RTP socket. Same SSRC and sequence stream as voice; the event's RTP
    /// timestamp is captured on `start_of_event` and reused on subsequent
    /// continuation / end-of-event packets (RFC 4733 §2.5.1.2).
    pub async fn send_dtmf_event(
        &self,
        event_code: u8,
        volume: u8,
        duration_samples: u16,
        end: bool,
        start_of_event: bool,
    ) -> Result<(), String> {
        let Some(pt) = self.telephone_event_payload_type else {
            return Err("telephone-event payload type not configured".to_string());
        };
        let remote = match *self.remote_addr.lock() {
            Some(remote) => remote,
            None => return Ok(()),
        };
        let timestamp = {
            let mut held = self.dtmf_event_timestamp.lock();
            if start_of_event || held.is_none() {
                *held = Some(self.send_timestamp.load(Ordering::Relaxed));
            }
            held.expect("dtmf_event_timestamp set above")
        };
        let sequence = self.send_sequence.fetch_add(1, Ordering::Relaxed);
        let body = [
            event_code,
            (if end { 0x80 } else { 0x00 }) | (volume & 0x3F),
            (duration_samples >> 8) as u8,
            duration_samples as u8,
        ];
        let rtp =
            build_rtp_packet_with_marker(pt, start_of_event, sequence, timestamp, self.ssrc, &body);
        self.socket
            .send_to(&rtp, remote)
            .await
            .map_err(|err| format!("RTP DTMF send to {remote} failed: {err}"))?;
        self.mark_activity();
        Ok(())
    }

    async fn send_silence_frame(&self) -> Result<(), String> {
        let remote = match *self.remote_addr.lock() {
            Some(remote) => remote,
            None => return Ok(()),
        };
        let codec = *self.codec.lock();
        let frame = [0i16; 160];
        let payload = encode_g711_frame(codec, &frame);
        let sequence = self.send_sequence.fetch_add(1, Ordering::Relaxed);
        let timestamp = self
            .send_timestamp
            .fetch_add(RTP_TIMESTAMP_STEP_8KHZ_20MS, Ordering::Relaxed);
        let rtp = build_rtp_packet(
            codec.payload_type(),
            sequence,
            timestamp,
            self.ssrc,
            &payload,
        );

        self.socket
            .send_to(&rtp, remote)
            .await
            .map_err(|err| format!("RTP silence send to {remote} failed: {err}"))?;
        let tx_count = self.rtp_tx_packets.fetch_add(1, Ordering::Relaxed) + 1;
        self.rtp_tx_silence_packets.fetch_add(1, Ordering::Relaxed);
        if self.log_media_frames && should_log_media_counter(tx_count) {
            log::debug!(
                "VoiceGW RTP tx silence session={} count={} to={} payload_type={} seq={} ts={} payload_bytes={}",
                self.session_id,
                tx_count,
                remote,
                codec.payload_type(),
                sequence,
                timestamp,
                payload.len()
            );
        }
        self.mark_activity();
        Ok(())
    }

    pub async fn receive_loop(self: Arc<Self>) {
        let mut buf = vec![0u8; 2048];
        let mut jitter = RtpJitterBuffer::new(jitter_depth_frames(self.jitter_buffer_ms));
        let mut tick = interval(Duration::from_millis(20));
        tick.set_missed_tick_behavior(MissedTickBehavior::Skip);

        loop {
            tokio::select! {
                packet = self.socket.recv_from(&mut buf) => {
                    let (len, from) = match packet {
                        Ok(result) => result,
                        Err(err) => {
                            log::warn!(
                                "VoiceGW RTP receive failed for session {}: {}",
                                self.session_id,
                                err
                            );
                            break;
                        }
                    };

                    if stun::is_stun_message(&buf[..len]) {
                        log::trace!(
                            "VoiceGW RTP socket ignored late STUN packet for session {} from {}",
                            self.session_id,
                            from
                        );
                        continue;
                    }

                    match self.decode_rtp_packet(&buf[..len], from).await {
                        Ok(frame) => {
                            self.mark_activity();
                            if let Some(dropped) = jitter.insert(frame) {
                                self.rtp_late_or_duplicate_packets
                                    .fetch_add(1, Ordering::Relaxed);
                                log::debug!(
                                    "VoiceGW RTP jitter buffer dropped late/duplicate packet for session {} rtp_seq={}",
                                    self.session_id,
                                    dropped
                                );
                            }
                        }
                        Err(err) => {
                            self.rtp_rx_dropped_packets
                                .fetch_add(1, Ordering::Relaxed);
                            log::debug!(
                                "VoiceGW RTP packet dropped for session {} from {}: {}",
                                self.session_id,
                                from,
                                err
                            );
                        }
                    }
                }
                _ = tick.tick() => {
                    match jitter.pop_due() {
                        Some(JitterOutput::Frame(frame)) => self.publish_decoded_frame(frame),
                        Some(JitterOutput::Missing) => {
                            self.rtp_missing_packets.fetch_add(1, Ordering::Relaxed);
                            if let Err(err) = self.publish_silence_gateway_frame() {
                                log::warn!(
                                    "VoiceGW failed to publish silence frame for session {}: {}",
                                    self.session_id,
                                    err
                                );
                            }
                        }
                        None => {}
                    }
                }
            }
        }
    }

    async fn decode_rtp_packet(
        &self,
        packet: &[u8],
        from: SocketAddr,
    ) -> Result<DecodedVoiceFrame, String> {
        let parsed = parse_rtp_packet(packet)?;
        let codec = G711Codec::from_payload_type(parsed.payload_type)
            .ok_or_else(|| format!("unsupported RTP payload type {}", parsed.payload_type))?;
        let rx_count = self.rtp_rx_packets.fetch_add(1, Ordering::Relaxed) + 1;
        if self.log_media_frames && should_log_media_counter(rx_count) {
            log::debug!(
                "VoiceGW RTP rx session={} count={} from={} payload_type={} seq={} ts={} payload_bytes={} codec={:?}",
                self.session_id,
                rx_count,
                from,
                parsed.payload_type,
                parsed.sequence,
                parsed.timestamp,
                parsed.payload.len(),
                codec
            );
        }
        let mut remote_addr = self.remote_addr.lock();
        if remote_addr.is_none() {
            *remote_addr = Some(from);
        }

        let pcm = decode_g711_payload(codec, parsed.payload);
        if pcm.len() < 160 {
            return Err(format!("G.711 payload too short: {} samples", pcm.len()));
        }
        let mut frame = [0i16; 160];
        frame.copy_from_slice(&pcm[..160]);
        let (rate, packet_data) = {
            let mut encoder = self.encoder.lock();
            encoder
                .encode(&frame)
                .map_err(|err| format!("EVRC encode failed: {err}"))?
        };
        let bits = evrc_packet_bytes_to_bits(&packet_data, rate);
        Ok(DecodedVoiceFrame {
            rtp_sequence: parsed.sequence,
            bits,
            rate: voice_rate_to_proto(rate) as i32,
            service_option: u32::from(self.voice_codec.service_option()),
        })
    }

    fn publish_decoded_frame(&self, frame: DecodedVoiceFrame) {
        let sequence = self.recv_sequence.fetch_add(1, Ordering::Relaxed);
        let event = GatewayVoiceFrame {
            session_id: self.session_id.clone(),
            num_bits: frame.bits.len() as u32,
            bits: frame.bits,
            rate: frame.rate,
            sequence,
            service_option: frame.service_option,
        };
        if let Err(err) = self.gateway_voice_tx.send(event) {
            log::trace!(
                "VoiceGW RTP frame had no MSC media subscribers session={}: {}",
                self.session_id,
                err
            );
        }
    }

    fn publish_silence_gateway_frame(&self) -> Result<(), String> {
        let frame = [0i16; 160];
        let (rate, packet_data) = {
            let mut encoder = self.encoder.lock();
            encoder
                .encode(&frame)
                .map_err(|err| format!("EVRC silence encode failed: {err}"))?
        };
        let bits = evrc_packet_bytes_to_bits(&packet_data, rate);
        self.gateway_silence_frames.fetch_add(1, Ordering::Relaxed);
        self.publish_decoded_frame(DecodedVoiceFrame {
            rtp_sequence: 0,
            bits,
            rate: voice_rate_to_proto(rate) as i32,
            service_option: u32::from(self.voice_codec.service_option()),
        });
        Ok(())
    }

    fn stun_transaction_id(&self) -> stun::TransactionId {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_nanos())
            .unwrap_or(0);
        let mut id = [0u8; 12];
        id[..4].copy_from_slice(&self.ssrc.to_be_bytes());
        id[4..12].copy_from_slice(&(now as u64).to_be_bytes());
        id
    }

    pub fn mark_activity(&self) {
        *self.last_activity_at.lock() = Instant::now();
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
struct DecodedVoiceFrame {
    rtp_sequence: u16,
    bits: Vec<u8>,
    rate: i32,
    service_option: u32,
}

#[derive(Clone, Debug, Eq, PartialEq)]
enum JitterOutput {
    Frame(DecodedVoiceFrame),
    Missing,
}

#[derive(Debug)]
struct RtpJitterBuffer {
    target_depth: usize,
    max_missing: usize,
    frames: BTreeMap<u16, DecodedVoiceFrame>,
    next_sequence: Option<u16>,
    consecutive_missing: usize,
}

impl RtpJitterBuffer {
    fn new(target_depth: usize) -> Self {
        Self {
            target_depth,
            max_missing: target_depth.saturating_mul(2).max(3),
            frames: BTreeMap::new(),
            next_sequence: None,
            consecutive_missing: 0,
        }
    }

    fn insert(&mut self, frame: DecodedVoiceFrame) -> Option<u16> {
        if let Some(next) = self.next_sequence {
            if sequence_before(frame.rtp_sequence, next) {
                return Some(frame.rtp_sequence);
            }
        }

        let sequence = frame.rtp_sequence;
        if self.frames.insert(sequence, frame).is_some() {
            return Some(sequence);
        }

        if self.next_sequence.is_none() && self.frames.len() >= self.target_depth.max(1) {
            self.next_sequence = self.frames.keys().next().copied();
        }

        None
    }

    fn pop_due(&mut self) -> Option<JitterOutput> {
        let next = self.next_sequence?;

        if let Some(frame) = self.frames.remove(&next) {
            self.next_sequence = Some(next.wrapping_add(1));
            self.consecutive_missing = 0;
            return Some(JitterOutput::Frame(frame));
        }

        self.consecutive_missing += 1;
        if self.consecutive_missing > self.max_missing {
            self.next_sequence = None;
            self.consecutive_missing = 0;
            return None;
        }

        self.next_sequence = Some(next.wrapping_add(1));
        Some(JitterOutput::Missing)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct RtpPacket<'a> {
    pub payload_type: u8,
    pub sequence: u16,
    pub timestamp: u32,
    pub ssrc: u32,
    pub payload: &'a [u8],
}

pub fn parse_rtp_packet(packet: &[u8]) -> Result<RtpPacket<'_>, String> {
    if packet.len() < RTP_HEADER_LEN {
        return Err("RTP packet too short".to_string());
    }
    if packet[0] >> 6 != 2 {
        return Err("unsupported RTP version".to_string());
    }

    let csrc_count = (packet[0] & 0x0f) as usize;
    let extension = packet[0] & 0x10 != 0;
    let mut header_len = RTP_HEADER_LEN + csrc_count * 4;
    if packet.len() < header_len {
        return Err("RTP CSRC header exceeds packet length".to_string());
    }
    if extension {
        if packet.len() < header_len + 4 {
            return Err("RTP extension header exceeds packet length".to_string());
        }
        let extension_words =
            u16::from_be_bytes([packet[header_len + 2], packet[header_len + 3]]) as usize;
        header_len += 4 + extension_words * 4;
        if packet.len() < header_len {
            return Err("RTP extension payload exceeds packet length".to_string());
        }
    }

    let payload_type = packet[1] & 0x7f;
    let sequence = u16::from_be_bytes([packet[2], packet[3]]);
    let timestamp = u32::from_be_bytes([packet[4], packet[5], packet[6], packet[7]]);
    let ssrc = u32::from_be_bytes([packet[8], packet[9], packet[10], packet[11]]);

    Ok(RtpPacket {
        payload_type,
        sequence,
        timestamp,
        ssrc,
        payload: &packet[header_len..],
    })
}

pub fn build_rtp_packet(
    payload_type: u8,
    sequence: u16,
    timestamp: u32,
    ssrc: u32,
    payload: &[u8],
) -> Vec<u8> {
    build_rtp_packet_with_marker(payload_type, false, sequence, timestamp, ssrc, payload)
}

pub fn build_rtp_packet_with_marker(
    payload_type: u8,
    marker: bool,
    sequence: u16,
    timestamp: u32,
    ssrc: u32,
    payload: &[u8],
) -> Vec<u8> {
    let mut packet = Vec::with_capacity(RTP_HEADER_LEN + payload.len());
    packet.push(0x80);
    let mb: u8 = if marker { 0x80 } else { 0x00 };
    packet.push(mb | (payload_type & 0x7f));
    packet.extend_from_slice(&sequence.to_be_bytes());
    packet.extend_from_slice(&timestamp.to_be_bytes());
    packet.extend_from_slice(&ssrc.to_be_bytes());
    packet.extend_from_slice(payload);
    packet
}

pub fn parse_sdp_rtp_endpoint(sdp: &[u8]) -> Option<SocketAddr> {
    parse_sdp_rtp_endpoint_result(sdp).ok()
}

pub fn parse_sdp_rtp_endpoint_result(sdp: &[u8]) -> Result<SocketAddr, String> {
    let text = std::str::from_utf8(sdp).map_err(|err| format!("SDP is not UTF-8: {err}"))?;
    let mut session_connection_addr = None;
    let mut audio_connection_addr = None;
    let mut audio_port = None;
    let mut in_audio = false;

    for raw_line in text.lines() {
        let line = raw_line.trim();
        if let Some(rest) = line.strip_prefix("m=") {
            in_audio = rest.starts_with("audio ");
            if in_audio {
                let port = rest
                    .strip_prefix("audio ")
                    .and_then(|value| value.split_whitespace().next())
                    .ok_or_else(|| "SDP m=audio line missing port".to_string())?
                    .parse::<u16>()
                    .map_err(|err| format!("SDP m=audio port is invalid: {err}"))?;
                if port == 0 {
                    return Err("SDP m=audio port is zero".to_string());
                }
                audio_port = Some(port);
            }
            continue;
        }

        if let Some(addr) = parse_sdp_connection_addr(line)? {
            if in_audio {
                audio_connection_addr = Some(addr);
            } else {
                session_connection_addr = Some(addr);
            }
        }
    }

    let ip = audio_connection_addr
        .or(session_connection_addr)
        .ok_or_else(|| "SDP missing c=IN connection address".to_string())?;
    let port = audio_port.ok_or_else(|| "SDP missing m=audio port".to_string())?;

    Ok(SocketAddr::new(ip, port))
}

fn parse_sdp_connection_addr(line: &str) -> Result<Option<IpAddr>, String> {
    let Some(rest) = line
        .strip_prefix("c=IN IP4 ")
        .or_else(|| line.strip_prefix("c=IN IP6 "))
    else {
        return Ok(None);
    };
    let addr = rest
        .split_whitespace()
        .next()
        .ok_or_else(|| "SDP c=IN line missing address".to_string())?;
    addr.parse::<IpAddr>()
        .map(Some)
        .map_err(|err| format!("SDP connection address {addr:?} is invalid: {err}"))
}

fn jitter_depth_frames(jitter_buffer_ms: u64) -> usize {
    usize::try_from(jitter_buffer_ms / 20).unwrap_or(usize::MAX)
}

fn sequence_before(left: u16, right: u16) -> bool {
    left != right && left.wrapping_sub(right) > 0x8000
}

fn should_log_media_counter(count: u64) -> bool {
    count <= 5 || count % 250 == 0
}

pub fn evrc_bits_to_packet_bytes(bits: &[u8], rate_bps: u32) -> Result<Vec<u8>, String> {
    let Some(shape) = evrc_payload_shape_for_rate_bps(rate_bps) else {
        return Err(format!("unsupported EVRC rate_bps {rate_bps}"));
    };
    let expected_bits = shape.traffic_bits;
    if bits.len() < expected_bits {
        return Err(format!(
            "EVRC frame has {} bits, expected at least {} for {}bps",
            bits.len(),
            expected_bits,
            rate_bps
        ));
    }

    let mut out = vec![0u8; shape.data_bytes];
    for (idx, bit) in bits.iter().take(expected_bits).enumerate() {
        if bit & 1 != 0 {
            out[idx / 8] |= 1 << (7 - (idx % 8));
        }
    }
    Ok(out)
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
struct EvrcPayloadShape {
    traffic_bits: usize,
    data_bytes: usize,
}

fn evrc_payload_shape_for_rate_bps(rate_bps: u32) -> Option<EvrcPayloadShape> {
    let shape = match rate_bps {
        9600 => EvrcPayloadShape {
            traffic_bits: 171,
            data_bytes: 22,
        },
        4800 => EvrcPayloadShape {
            traffic_bits: 80,
            data_bytes: 10,
        },
        // RC1 names this EVRC quarter-rate payload 2400 bps. RC3 names the
        // same 40 primary traffic bits 2700 bps because its FQI/tail framing
        // differs before the BSC strips framing for the gateway.
        2400 | 2700 => EvrcPayloadShape {
            traffic_bits: 40,
            data_bytes: 5,
        },
        // Same for eighth rate: EVRC carries 16 primary traffic bits, while
        // RC3 reports the over-the-air frame rate as 1500 bps.
        1200 | 1500 => EvrcPayloadShape {
            traffic_bits: 16,
            data_bytes: 2,
        },
        _ => return None,
    };
    Some(shape)
}

fn voice_rate_for_rate_bps(rate_bps: u32) -> Result<VoiceRate, String> {
    match rate_bps {
        9600 => Ok(VoiceRate::Full),
        4800 => Ok(VoiceRate::Half),
        2400 | 2700 => Ok(VoiceRate::Quarter),
        1200 | 1500 => Ok(VoiceRate::Eighth),
        _ => Err(format!("unsupported EVRC rate_bps {rate_bps}")),
    }
}

pub fn evrc_packet_bytes_to_bits(packet: &[u8], rate: VoiceRate) -> Vec<u8> {
    let traffic_bits = rate.primary_traffic_bits();
    let mut bits = Vec::with_capacity(traffic_bits);
    for byte in packet {
        for bit_idx in (0..8).rev() {
            bits.push((byte >> bit_idx) & 1);
        }
    }
    bits.truncate(traffic_bits);
    while bits.len() < traffic_bits {
        bits.push(0);
    }
    bits
}

pub fn encode_g711_frame(codec: G711Codec, pcm: &[i16; 160]) -> Vec<u8> {
    pcm.iter()
        .map(|sample| match codec {
            G711Codec::Pcmu => linear_to_ulaw(*sample),
            G711Codec::Pcma => linear_to_alaw(*sample),
        })
        .collect()
}

pub fn decode_g711_payload(codec: G711Codec, payload: &[u8]) -> Vec<i16> {
    payload
        .iter()
        .map(|sample| match codec {
            G711Codec::Pcmu => ulaw_to_linear(*sample),
            G711Codec::Pcma => alaw_to_linear(*sample),
        })
        .collect()
}

fn voice_rate_to_proto(rate: VoiceRate) -> VoiceFrameRate {
    match rate {
        VoiceRate::Full => VoiceFrameRate::Full,
        VoiceRate::Half => VoiceFrameRate::Half,
        VoiceRate::Quarter => VoiceFrameRate::Quarter,
        VoiceRate::Eighth => VoiceFrameRate::Eighth,
    }
}

fn stable_ssrc(session_id: &str) -> u32 {
    let mut hash = 0x811c9dc5u32;
    for byte in session_id.bytes() {
        hash ^= u32::from(byte);
        hash = hash.wrapping_mul(0x01000193);
    }
    hash
}

fn select_stun_addr(
    candidates: &[SocketAddr],
    local_addr: Option<SocketAddr>,
) -> Option<SocketAddr> {
    match local_addr.map(|addr| addr.ip()) {
        Some(IpAddr::V4(_)) => candidates.iter().copied().find(SocketAddr::is_ipv4),
        Some(IpAddr::V6(_)) => candidates.iter().copied().find(SocketAddr::is_ipv6),
        None => candidates.first().copied(),
    }
    .or_else(|| candidates.first().copied())
}

fn linear_to_ulaw(sample: i16) -> u8 {
    const BIAS: i32 = 0x84;
    const CLIP: i32 = 32635;

    let mut pcm = i32::from(sample);
    let mask = if pcm < 0 {
        pcm = (-pcm).min(CLIP);
        0x7f
    } else {
        pcm = pcm.min(CLIP);
        0xff
    };
    pcm += BIAS;

    let segment = search_segment(pcm, &ULAW_SEG_END);
    if segment >= 8 {
        return (0x7f ^ mask) as u8;
    }
    let mantissa = (pcm >> (segment + 3)) & 0x0f;
    ((segment << 4 | mantissa) ^ mask) as u8
}

fn ulaw_to_linear(sample: u8) -> i16 {
    const BIAS: i32 = 0x84;
    let value = !sample;
    let mut t = ((i32::from(value & 0x0f)) << 3) + BIAS;
    t <<= i32::from((value & 0x70) >> 4);
    if value & 0x80 != 0 {
        (BIAS - t) as i16
    } else {
        (t - BIAS) as i16
    }
}

fn linear_to_alaw(sample: i16) -> u8 {
    let mut pcm = i32::from(sample);
    let mask = if pcm >= 0 {
        0xd5
    } else {
        pcm = -pcm - 1;
        0x55
    };

    let segment = search_segment(pcm, &ALAW_SEG_END);
    if segment >= 8 {
        return (0x7f ^ mask) as u8;
    }

    let aval = if segment < 2 {
        (segment << 4) | ((pcm >> 4) & 0x0f)
    } else {
        (segment << 4) | ((pcm >> (segment + 3)) & 0x0f)
    };
    (aval ^ mask) as u8
}

fn alaw_to_linear(sample: u8) -> i16 {
    let value = sample ^ 0x55;
    let mut t = i32::from(value & 0x0f) << 4;
    let segment = i32::from((value & 0x70) >> 4);
    match segment {
        0 => t += 8,
        1 => t += 0x108,
        _ => {
            t += 0x108;
            t <<= segment - 1;
        }
    }
    if value & 0x80 != 0 {
        t as i16
    } else {
        (-t) as i16
    }
}

const ULAW_SEG_END: [i32; 8] = [0xff, 0x1ff, 0x3ff, 0x7ff, 0xfff, 0x1fff, 0x3fff, 0x7fff];
const ALAW_SEG_END: [i32; 8] = [0x1f, 0x3f, 0x7f, 0xff, 0x1ff, 0x3ff, 0x7ff, 0xfff];

fn search_segment(value: i32, table: &[i32; 8]) -> i32 {
    table
        .iter()
        .position(|end| value <= *end)
        .map(|idx| idx as i32)
        .unwrap_or(8)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sdp_rtp_endpoint() {
        let sdp = b"v=0\r\nc=IN IP4 192.0.2.10\r\nm=audio 49170 RTP/AVP 0 8\r\n";
        let endpoint = parse_sdp_rtp_endpoint(sdp).unwrap();
        assert_eq!(endpoint.to_string(), "192.0.2.10:49170");
    }

    #[test]
    fn parses_media_level_sdp_rtp_endpoint_and_ipv6() {
        let sdp =
            b"v=0\r\nc=IN IP4 192.0.2.10\r\nm=audio 49170 RTP/AVP 0\r\nc=IN IP6 2001:db8::10\r\n";
        let endpoint = parse_sdp_rtp_endpoint_result(sdp).unwrap();
        assert_eq!(endpoint.to_string(), "[2001:db8::10]:49170");
    }

    #[test]
    fn rejects_sdp_without_audio_endpoint() {
        let err = parse_sdp_rtp_endpoint_result(b"v=0\r\nc=IN IP4 192.0.2.10\r\n").unwrap_err();
        assert!(err.contains("m=audio"));
    }

    #[test]
    fn jitter_buffer_reorders_and_inserts_missing_frames() {
        let mut buffer = RtpJitterBuffer::new(2);
        let frame_1 = DecodedVoiceFrame {
            rtp_sequence: 10,
            bits: vec![1],
            rate: VoiceFrameRate::Full as i32,
            service_option: 3,
        };
        let frame_2 = DecodedVoiceFrame {
            rtp_sequence: 11,
            bits: vec![2],
            rate: VoiceFrameRate::Full as i32,
            service_option: 3,
        };
        let frame_4 = DecodedVoiceFrame {
            rtp_sequence: 13,
            bits: vec![4],
            rate: VoiceFrameRate::Full as i32,
            service_option: 3,
        };

        assert_eq!(buffer.insert(frame_2.clone()), None);
        assert_eq!(buffer.pop_due(), None);
        assert_eq!(buffer.insert(frame_1.clone()), None);
        assert_eq!(buffer.pop_due(), Some(JitterOutput::Frame(frame_1)));
        assert_eq!(buffer.insert(frame_4.clone()), None);
        assert_eq!(buffer.pop_due(), Some(JitterOutput::Frame(frame_2)));
        assert_eq!(buffer.pop_due(), Some(JitterOutput::Missing));
        assert_eq!(buffer.pop_due(), Some(JitterOutput::Frame(frame_4)));
    }

    #[test]
    fn builds_and_parses_rtp_packet() {
        let packet = build_rtp_packet(0, 42, 1600, 0x11223344, &[1, 2, 3]);
        let parsed = parse_rtp_packet(&packet).unwrap();
        assert_eq!(parsed.payload_type, 0);
        assert_eq!(parsed.sequence, 42);
        assert_eq!(parsed.timestamp, 1600);
        assert_eq!(parsed.ssrc, 0x11223344);
        assert_eq!(parsed.payload, &[1, 2, 3]);
    }

    #[test]
    fn packs_full_rate_evrc_bits_to_payload_bytes() {
        let mut bits = vec![0u8; 171];
        bits[0] = 1;
        bits[8] = 1;
        let packet = evrc_bits_to_packet_bytes(&bits, 9600).unwrap();
        assert_eq!(packet.len(), 22);
        assert_eq!(packet[0], 0x80);
        assert_eq!(packet[1], 0x80);
    }

    #[test]
    fn packs_rc3_low_rate_evrc_aliases_to_payload_bytes() {
        let mut quarter_bits = vec![0u8; 40];
        quarter_bits[0] = 1;
        quarter_bits[39] = 1;
        let quarter_packet = evrc_bits_to_packet_bytes(&quarter_bits, 2700).unwrap();
        assert_eq!(quarter_packet.len(), 5);
        assert_eq!(quarter_packet[0], 0x80);
        assert_eq!(quarter_packet[4], 0x01);

        let mut eighth_bits = vec![0u8; 16];
        eighth_bits[0] = 1;
        eighth_bits[15] = 1;
        let eighth_packet = evrc_bits_to_packet_bytes(&eighth_bits, 1500).unwrap();
        assert_eq!(eighth_packet.len(), 2);
        assert_eq!(eighth_packet, [0x80, 0x01]);
    }

    #[test]
    fn unpacks_evrc_payload_bytes_to_rate_bits() {
        let bits = evrc_packet_bytes_to_bits(&[0x80, 0x01], VoiceRate::Eighth);
        assert_eq!(bits.len(), 16);
        assert_eq!(bits[0], 1);
        assert_eq!(bits[15], 1);
    }

    #[test]
    fn g711_roundtrip_preserves_sample_sign() {
        let samples = [-12000i16, -1000, 1000, 12000];
        for sample in samples {
            let ulaw = ulaw_to_linear(linear_to_ulaw(sample));
            let alaw = alaw_to_linear(linear_to_alaw(sample));
            assert_eq!(ulaw.signum(), sample.signum());
            assert_eq!(alaw.signum(), sample.signum());
        }
    }

    #[test]
    fn maps_static_payload_types_to_codecs() {
        assert_eq!(G711Codec::from_payload_type(0), Some(G711Codec::Pcmu));
        assert_eq!(G711Codec::from_payload_type(8), Some(G711Codec::Pcma));
        assert_eq!(G711Codec::from_payload_type(111), None);
    }

    #[tokio::test]
    async fn mark_activity_resets_idle_timer() {
        let (gateway_voice_tx, _) = broadcast::channel(4);
        let media = RtpMediaSession::bind(
            "activity-test".to_string(),
            "127.0.0.1",
            0,
            G711Codec::Pcmu,
            VoiceCodec::EvrcA,
            0,
            false,
            false,
            gateway_voice_tx,
            None,
        )
        .await
        .unwrap();

        tokio::time::sleep(Duration::from_millis(10)).await;
        let before = media.last_activity_elapsed();
        media.mark_activity();
        let after = media.last_activity_elapsed();

        assert!(before >= Duration::from_millis(5));
        assert!(after < before);
    }

    #[test]
    fn selects_stun_addr_matching_local_socket_family() {
        let v6 = "[2001:db8::1]:3478".parse().unwrap();
        let v4 = "192.0.2.10:3478".parse().unwrap();

        assert_eq!(
            select_stun_addr(&[v6, v4], Some("0.0.0.0:10000".parse().unwrap())),
            Some(v4)
        );
        assert_eq!(
            select_stun_addr(&[v4, v6], Some("[::]:10000".parse().unwrap())),
            Some(v6)
        );
        assert_eq!(select_stun_addr(&[v4, v6], None), Some(v4));
    }

    #[tokio::test]
    async fn sends_latch_frames_to_remote_endpoint() {
        let peer = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let peer_addr = peer.local_addr().unwrap();
        let (gateway_voice_tx, _) = broadcast::channel(4);
        let media = RtpMediaSession::bind(
            "latch-test".to_string(),
            "127.0.0.1",
            0,
            G711Codec::Pcmu,
            VoiceCodec::EvrcA,
            0,
            false,
            false,
            gateway_voice_tx,
            None,
        )
        .await
        .unwrap();
        let sdp = format!(
            "v=0\r\nc=IN IP4 127.0.0.1\r\nm=audio {} RTP/AVP 0\r\n",
            peer_addr.port()
        );

        assert_eq!(
            media.set_remote_from_sdp(sdp.as_bytes(), G711Codec::Pcmu),
            Ok(peer_addr)
        );
        media
            .send_latch_frames(2, Duration::from_millis(0))
            .await
            .unwrap();

        let mut buf = [0u8; 2048];
        let (first_len, from) =
            tokio::time::timeout(Duration::from_secs(1), peer.recv_from(&mut buf))
                .await
                .expect("timed out waiting for first RTP latch packet")
                .unwrap();
        assert_eq!(from.port(), media.local_addr().unwrap().port());
        let first = parse_rtp_packet(&buf[..first_len]).unwrap();
        assert_eq!(first.payload_type, G711Codec::Pcmu.payload_type());
        assert_eq!(first.sequence, 1);
        assert_eq!(first.payload.len(), 160);

        let (second_len, _) =
            tokio::time::timeout(Duration::from_secs(1), peer.recv_from(&mut buf))
                .await
                .expect("timed out waiting for second RTP latch packet")
                .unwrap();
        let second = parse_rtp_packet(&buf[..second_len]).unwrap();
        assert_eq!(second.payload_type, G711Codec::Pcmu.payload_type());
        assert_eq!(second.sequence, 2);
        assert_eq!(second.payload.len(), 160);

        let stats = media.stats();
        assert_eq!(stats.session_id, "latch-test");
        assert_eq!(stats.remote_addr, Some(peer_addr));
        assert_eq!(stats.codec, G711Codec::Pcmu);
        assert_eq!(stats.rtp_tx_packets, 2);
        assert_eq!(stats.rtp_tx_silence_packets, 2);
        assert_eq!(stats.rtp_rx_packets, 0);
        assert_eq!(stats.evrc_decode_failures, 0);

        let summary = media.log_summary_once("test").unwrap();
        assert_eq!(summary.rtp_tx_packets, 2);
        assert!(media.log_summary_once("duplicate").is_none());
    }

    #[tokio::test]
    async fn sends_rtp_for_rc3_eighth_air_frame() {
        let peer = UdpSocket::bind("127.0.0.1:0").await.unwrap();
        let peer_addr = peer.local_addr().unwrap();
        let (gateway_voice_tx, _) = broadcast::channel(4);
        let media = RtpMediaSession::bind(
            "air-frame-test".to_string(),
            "127.0.0.1",
            0,
            G711Codec::Pcmu,
            VoiceCodec::EvrcA,
            0,
            false,
            false,
            gateway_voice_tx,
            None,
        )
        .await
        .unwrap();
        let sdp = format!(
            "v=0\r\nc=IN IP4 127.0.0.1\r\nm=audio {} RTP/AVP 0\r\n",
            peer_addr.port()
        );
        media
            .set_remote_from_sdp(sdp.as_bytes(), G711Codec::Pcmu)
            .unwrap();

        media.send_air_frame(&[0u8; 16], 1500, 3).await.unwrap();

        let mut buf = [0u8; 2048];
        let (len, from) = tokio::time::timeout(Duration::from_secs(1), peer.recv_from(&mut buf))
            .await
            .expect("timed out waiting for RTP air frame")
            .unwrap();
        assert_eq!(from.port(), media.local_addr().unwrap().port());
        let packet = parse_rtp_packet(&buf[..len]).unwrap();
        assert_eq!(packet.payload_type, G711Codec::Pcmu.payload_type());
        assert_eq!(packet.payload.len(), 160);

        let stats = media.stats();
        assert_eq!(stats.rtp_tx_packets, 1);
        assert_eq!(stats.rtp_rx_packets, 0);
    }
}
