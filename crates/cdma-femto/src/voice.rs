use std::net::{IpAddr, Ipv4Addr, SocketAddr, UdpSocket as StdUdpSocket};
use std::sync::atomic::{AtomicU8, AtomicU16, Ordering};
use std::sync::{Arc, RwLock};

use tokio::net::UdpSocket;
use tokio::sync::watch;

use crate::fap::MediaPacketFormat;

const RTP_MAX_DATAGRAM: usize = 2_048;
const RTP_FIXED_HEADER_LEN: usize = 12;
// RFC 3550 §5.1.
const RTP_VERSION: u8 = 2;
const RTP_VERSION_SHIFT: u32 = 6;
const RTP_CSRC_COUNT_MASK: u8 = 0x0f;
const RTP_EXTENSION_FLAG: u8 = 0x10;
const RTP_PAYLOAD_TYPE_MASK: u8 = 0x7f;
const RTP_MARKER_MASK: u8 = 0x80;
const RTP_WORD_LEN: usize = 4;
const RTP_EXTENSION_HEADER_LEN: usize = 4;
const RTP_EXTENSION_LENGTH_OFFSET: usize = 2;
const SDP_AUDIO_MIN_FIELDS: usize = 3;
const SDP_ORIGIN_MIN_FIELDS: usize = 6;
const SDP_ORIGIN_ADDRESS_TYPE_INDEX: usize = 4;
const SDP_ORIGIN_ADDRESS_INDEX: usize = 5;
const RTP_CLOCK_RATE: u32 = 8_000;
const EVRC_PAYLOAD_TYPE: u8 = 96;
const DTMF_PAYLOAD_TYPE: u8 = 101;

#[derive(Debug, thiserror::Error)]
pub enum VoiceError {
    #[error("SDP is not UTF-8: {0}")]
    InvalidUtf8(#[from] std::str::Utf8Error),
    #[error("invalid SDP IPv4 address {value}: {source}")]
    InvalidAddress {
        value: String,
        #[source]
        source: std::net::AddrParseError,
    },
    #[error("invalid SDP audio port {value}: {source}")]
    InvalidPort {
        value: String,
        #[source]
        source: std::num::ParseIntError,
    },
    #[error("unsupported SDP audio media line")]
    UnsupportedAudioMedia,
    #[error("SDP has no IPv4 connection address")]
    MissingConnection,
    #[error("SDP has no audio media line")]
    MissingAudio,
    #[error("SDP audio media line has no transport or payloads")]
    MissingAudioTransport,
    #[error("FCS RTP port range {start}..={end} must contain at least two non-zero ports")]
    InvalidPortRange { start: u16, end: u16 },
    #[error("binding RTP relay port {port}: {source}")]
    Bind {
        port: u16,
        #[source]
        source: std::io::Error,
    },
    #[error("FCS RTP relay port range {start}..={end} is exhausted on {bind_ip}")]
    PortRangeExhausted {
        bind_ip: Ipv4Addr,
        start: u16,
        end: u16,
    },
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum RelayCodec {
    Evrc,
    Qcelp13k,
}

impl RelayCodec {
    fn for_service_option(service_option: u16) -> Self {
        if service_option == cdma_ios::ServiceOption::QCELP_13K.0 {
            Self::Qcelp13k
        } else {
            Self::Evrc
        }
    }

    fn bearer_format(self) -> cdma_ios::VoiceBearerFormat {
        match self {
            Self::Evrc => cdma_ios::VoiceBearerFormat::Evrc,
            Self::Qcelp13k => cdma_ios::VoiceBearerFormat::Vocoder13k,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SdpAudioEndpoint {
    pub address: Ipv4Addr,
    pub port: u16,
    pub payload_types: Vec<u8>,
}

pub fn parse_sdp_audio(body: &[u8]) -> Result<SdpAudioEndpoint, VoiceError> {
    let text = std::str::from_utf8(body)?;
    let mut address = None;
    let mut port = None;
    let mut payload_types = Vec::new();
    for line in text.lines().map(str::trim) {
        if let Some(value) = line.strip_prefix("c=IN IP4 ") {
            address = Some(value.trim().parse::<Ipv4Addr>().map_err(|source| {
                VoiceError::InvalidAddress {
                    value: value.to_string(),
                    source,
                }
            })?);
        } else if let Some(value) = line.strip_prefix("m=audio ") {
            let fields = value.split_whitespace().collect::<Vec<_>>();
            if fields.len() < SDP_AUDIO_MIN_FIELDS || fields[1] != "RTP/AVP" {
                return Err(VoiceError::UnsupportedAudioMedia);
            }
            port = Some(
                fields[0]
                    .parse::<u16>()
                    .map_err(|source| VoiceError::InvalidPort {
                        value: fields[0].to_string(),
                        source,
                    })?,
            );
            payload_types = fields[2..]
                .iter()
                .filter_map(|value| value.parse::<u8>().ok())
                .collect();
        }
    }
    Ok(SdpAudioEndpoint {
        address: address.ok_or(VoiceError::MissingConnection)?,
        port: port.ok_or(VoiceError::MissingAudio)?,
        payload_types,
    })
}

pub fn sdp_speech_payload_types(body: &[u8]) -> Vec<u8> {
    let Ok(text) = std::str::from_utf8(body) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| line.trim().strip_prefix("a=rtpmap:"))
        .filter_map(|value| {
            let (payload_type, encoding) = value.split_once(char::is_whitespace)?;
            (!encoding
                .trim_start()
                .to_ascii_lowercase()
                .starts_with("telephone-event/"))
            .then(|| payload_type.parse::<u8>().ok())
            .flatten()
        })
        .collect()
}

pub fn rewrite_sdp_transport(
    body: &[u8],
    address: Ipv4Addr,
    port: u16,
) -> Result<Vec<u8>, VoiceError> {
    let text = std::str::from_utf8(body)?;
    let line_ending = if text.contains("\r\n") { "\r\n" } else { "\n" };
    let had_trailing_line_ending = text.ends_with(line_ending);
    let mut found_connection = false;
    let mut found_audio = false;
    let mut rewritten = Vec::new();
    for line in text.lines() {
        if line.starts_with("c=IN IP4 ") {
            rewritten.push(format!("c=IN IP4 {address}"));
            found_connection = true;
        } else if let Some(value) = line.strip_prefix("m=audio ") {
            let mut fields = value.split_whitespace();
            let _old_port = fields.next();
            let remainder = fields.collect::<Vec<_>>().join(" ");
            if remainder.is_empty() {
                return Err(VoiceError::MissingAudioTransport);
            }
            rewritten.push(format!("m=audio {port} {remainder}"));
            found_audio = true;
        } else if let Some(value) = line.strip_prefix("o=") {
            let fields = value.split_whitespace().collect::<Vec<_>>();
            if fields.len() >= SDP_ORIGIN_MIN_FIELDS
                && fields[SDP_ORIGIN_ADDRESS_TYPE_INDEX] == "IP4"
            {
                rewritten.push(format!(
                    "o={} {address}",
                    fields[..SDP_ORIGIN_ADDRESS_INDEX].join(" ")
                ));
            } else {
                rewritten.push(line.to_string());
            }
        } else {
            rewritten.push(line.to_string());
        }
    }
    if !found_connection {
        return Err(VoiceError::MissingConnection);
    }
    if !found_audio {
        return Err(VoiceError::MissingAudio);
    }
    let mut result = rewritten.join(line_ending);
    if had_trailing_line_ending {
        result.push_str(line_ending);
    }
    Ok(result.into_bytes())
}

pub fn build_sdp_offer(address: Ipv4Addr, port: u16, service_option: u16) -> Vec<u8> {
    let codec = if service_option == cdma_ios::ServiceOption::QCELP_13K.0 {
        "QCELP/8000/1"
    } else {
        "EVRC0/8000"
    };
    format!(
        "v=0\r\no=1xbts 1 1 IN IP4 {address}\r\ns=1xBTS Femto Voice\r\nc=IN IP4 {address}\r\nt=0 0\r\nm=audio {port} RTP/AVP {EVRC_PAYLOAD_TYPE} {DTMF_PAYLOAD_TYPE}\r\na=rtpmap:{EVRC_PAYLOAD_TYPE} {codec}\r\na=rtpmap:{DTMF_PAYLOAD_TYPE} telephone-event/{RTP_CLOCK_RATE}\r\na=fmtp:{DTMF_PAYLOAD_TYPE} 0-15\r\na=sendrecv\r\n"
    )
    .into_bytes()
}

#[derive(Clone)]
pub struct RtpRelayHandle {
    fap_local: SocketAddr,
    msc_local: SocketAddr,
    fap_remote: Arc<RwLock<Option<SocketAddr>>>,
    msc_remote: Arc<RwLock<Option<SocketAddr>>>,
    codec: Arc<RwLock<RelayCodec>>,
    fap_voice_payload_type: Arc<AtomicU8>,
    stop_tx: watch::Sender<bool>,
}

impl RtpRelayHandle {
    pub fn fap_local(&self) -> SocketAddr {
        self.fap_local
    }

    pub fn msc_local(&self) -> SocketAddr {
        self.msc_local
    }

    pub fn set_fap_remote(&self, remote: SocketAddr) {
        *self.fap_remote.write().expect("FAP RTP remote lock") = Some(remote);
    }

    pub fn set_msc_remote(&self, remote: SocketAddr) {
        *self.msc_remote.write().expect("MSC RTP remote lock") = Some(remote);
    }

    pub fn set_service_option(&self, service_option: u16) {
        *self.codec.write().expect("RTP relay codec lock") =
            RelayCodec::for_service_option(service_option);
    }

    pub fn set_fap_voice_payload_type(&self, payload_type: u8) {
        self.fap_voice_payload_type
            .store(payload_type, Ordering::Relaxed);
    }
}

impl Drop for RtpRelayHandle {
    fn drop(&mut self) {
        if Arc::strong_count(&self.fap_remote) == 1 {
            let _ = self.stop_tx.send(true);
        }
    }
}

pub struct RtpRelayPool {
    fap_bind_ip: Ipv4Addr,
    msc_bind_ip: Ipv4Addr,
    port_start: u16,
    port_end: u16,
    next_port: AtomicU16,
}

impl RtpRelayPool {
    pub fn new(
        fap_bind_ip: Ipv4Addr,
        msc_bind_ip: Ipv4Addr,
        port_start: u16,
        port_end: u16,
    ) -> Result<Self, VoiceError> {
        if port_start == 0 || port_end <= port_start {
            return Err(VoiceError::InvalidPortRange {
                start: port_start,
                end: port_end,
            });
        }
        Ok(Self {
            fap_bind_ip,
            msc_bind_ip,
            port_start,
            port_end,
            next_port: AtomicU16::new(port_start),
        })
    }

    pub fn open(&self, service_option: u16) -> Result<RtpRelayHandle, VoiceError> {
        self.open_with_format(service_option, MediaPacketFormat::StandardRtp)
    }

    pub fn open_with_format(
        &self,
        service_option: u16,
        packet_format: MediaPacketFormat,
    ) -> Result<RtpRelayHandle, VoiceError> {
        let fap = self.bind_socket(self.fap_bind_ip)?;
        let msc = self.bind_socket(self.msc_bind_ip)?;
        let fap_local = fap.local_addr()?;
        let msc_local = msc.local_addr()?;
        fap.set_nonblocking(true)?;
        msc.set_nonblocking(true)?;
        let fap = UdpSocket::from_std(fap)?;
        let msc = UdpSocket::from_std(msc)?;
        let fap_remote = Arc::new(RwLock::new(None));
        let msc_remote = Arc::new(RwLock::new(None));
        let (stop_tx, stop_rx) = watch::channel(false);
        let codec = Arc::new(RwLock::new(RelayCodec::for_service_option(service_option)));
        let fap_voice_payload_type = Arc::new(AtomicU8::new(EVRC_PAYLOAD_TYPE));
        tokio::spawn(run_relay(
            fap,
            msc,
            codec.clone(),
            fap_remote.clone(),
            msc_remote.clone(),
            fap_voice_payload_type.clone(),
            packet_format,
            stop_rx,
        ));
        Ok(RtpRelayHandle {
            fap_local,
            msc_local,
            fap_remote,
            msc_remote,
            codec,
            fap_voice_payload_type,
            stop_tx,
        })
    }

    fn bind_socket(&self, bind_ip: Ipv4Addr) -> Result<StdUdpSocket, VoiceError> {
        let attempts = usize::from(self.port_end - self.port_start) + 1;
        for _ in 0..attempts {
            let port = self
                .next_port
                .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |port| {
                    Some(if port >= self.port_end {
                        self.port_start
                    } else {
                        port + 1
                    })
                })
                .unwrap_or(self.port_start);
            match StdUdpSocket::bind(SocketAddr::new(IpAddr::V4(bind_ip), port)) {
                Ok(socket) => return Ok(socket),
                Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => continue,
                Err(source) => return Err(VoiceError::Bind { port, source }),
            }
        }
        Err(VoiceError::PortRangeExhausted {
            bind_ip,
            start: self.port_start,
            end: self.port_end,
        })
    }
}

async fn run_relay(
    fap: UdpSocket,
    msc: UdpSocket,
    codec: Arc<RwLock<RelayCodec>>,
    fap_remote: Arc<RwLock<Option<SocketAddr>>>,
    msc_remote: Arc<RwLock<Option<SocketAddr>>>,
    fap_voice_payload_type: Arc<AtomicU8>,
    packet_format: MediaPacketFormat,
    mut stop_rx: watch::Receiver<bool>,
) {
    let mut fap_buffer = [0u8; RTP_MAX_DATAGRAM];
    let mut msc_buffer = [0u8; RTP_MAX_DATAGRAM];
    let mut logged_malformed_fap = false;
    let mut logged_malformed_msc = false;
    loop {
        tokio::select! {
            result = fap.recv_from(&mut fap_buffer) => {
                match result {
                    Ok((length, source)) => {
                        *fap_remote.write().expect("FAP RTP remote lock") = Some(source);
                        let remote = *msc_remote.read().expect("MSC RTP remote lock");
                        if let Some(remote) = remote {
                            let codec = *codec.read().expect("RTP relay codec lock");
                            let fap_payload_type = fap_voice_payload_type.load(Ordering::Relaxed);
                            let converted = fap_to_core_packet(
                                &fap_buffer[..length],
                                codec,
                                fap_payload_type,
                                packet_format,
                            );
                            match converted {
                                Some(packet) => {
                                    if let Err(error) = msc.send_to(&packet, remote).await {
                                        log::warn!("FCS: forwarding FAP RTP to MSC {remote}: {error}");
                                    }
                                }
                                None => {
                                    if !logged_malformed_fap {
                                        log::warn!(
                                            "FCS: dropping malformed FAP RTP packet ({length} bytes, prefix={:02x?}, codec={codec:?})",
                                            &fap_buffer[..length.min(24)]
                                        );
                                        logged_malformed_fap = true;
                                    }
                                }
                            }
                        }
                    }
                    Err(error) => log::warn!("FCS: receiving FAP RTP: {error}"),
                }
            }
            result = msc.recv_from(&mut msc_buffer) => {
                match result {
                    Ok((length, source)) => {
                        *msc_remote.write().expect("MSC RTP remote lock") = Some(source);
                        let remote = *fap_remote.read().expect("FAP RTP remote lock");
                        if let Some(remote) = remote {
                            let codec = *codec.read().expect("RTP relay codec lock");
                            let fap_payload_type = fap_voice_payload_type.load(Ordering::Relaxed);
                            let converted = core_to_fap_packet(
                                &msc_buffer[..length],
                                codec,
                                fap_payload_type,
                                packet_format,
                            );
                            match converted {
                                Some(packet) => {
                                    if let Err(error) = fap.send_to(&packet, remote).await {
                                        log::warn!("FCS: forwarding MSC RTP to FAP {remote}: {error}");
                                    }
                                }
                                None => {
                                    if !logged_malformed_msc {
                                        log::warn!(
                                            "FCS: dropping malformed MSC RTP packet ({length} bytes, prefix={:02x?}, codec={codec:?})",
                                            &msc_buffer[..length.min(24)]
                                        );
                                        logged_malformed_msc = true;
                                    }
                                }
                            }
                        }
                    }
                    Err(error) => log::warn!("FCS: receiving MSC RTP: {error}"),
                }
            }
            changed = stop_rx.changed() => {
                if changed.is_err() || *stop_rx.borrow() {
                    break;
                }
            }
        }
    }
}

fn fap_to_core_packet(
    packet: &[u8],
    codec: RelayCodec,
    fap_payload_type: u8,
    packet_format: MediaPacketFormat,
) -> Option<Vec<u8>> {
    match packet_format {
        MediaPacketFormat::StandardRtp => Some(packet.to_vec()),
        MediaPacketFormat::AirvanaCompact => add_a2p_toc(packet, codec, fap_payload_type),
    }
}

fn core_to_fap_packet(
    packet: &[u8],
    codec: RelayCodec,
    fap_payload_type: u8,
    packet_format: MediaPacketFormat,
) -> Option<Vec<u8>> {
    match packet_format {
        MediaPacketFormat::StandardRtp => Some(packet.to_vec()),
        MediaPacketFormat::AirvanaCompact => remove_a2p_toc(packet, codec, fap_payload_type),
    }
}

fn rtp_payload_offset(packet: &[u8]) -> Option<usize> {
    if packet.len() < RTP_FIXED_HEADER_LEN || packet[0] >> RTP_VERSION_SHIFT != RTP_VERSION {
        return None;
    }
    let csrc_count = usize::from(packet[0] & RTP_CSRC_COUNT_MASK);
    let mut offset = RTP_FIXED_HEADER_LEN.checked_add(csrc_count.checked_mul(RTP_WORD_LEN)?)?;
    if packet.len() < offset {
        return None;
    }
    if packet[0] & RTP_EXTENSION_FLAG != 0 {
        if packet.len() < offset + RTP_EXTENSION_HEADER_LEN {
            return None;
        }
        let extension_words = usize::from(u16::from_be_bytes([
            packet[offset + RTP_EXTENSION_LENGTH_OFFSET],
            packet[offset + RTP_EXTENSION_LENGTH_OFFSET + 1],
        ]));
        offset = offset
            .checked_add(RTP_EXTENSION_HEADER_LEN + extension_words.checked_mul(RTP_WORD_LEN)?)?;
    }
    (packet.len() >= offset).then_some(offset)
}

/// The FAP sends compact frames with no payload header (A.S0012-D §3.2.3).
fn add_a2p_toc(packet: &[u8], codec: RelayCodec, fap_payload_type: u8) -> Option<Vec<u8>> {
    let offset = rtp_payload_offset(packet)?;
    let payload_type = packet[1] & RTP_PAYLOAD_TYPE_MASK;
    if payload_type != fap_payload_type && payload_type != EVRC_PAYLOAD_TYPE {
        return Some(packet.to_vec());
    }
    let format = codec.bearer_format();
    let frame = &packet[offset..];
    let frame_type = if frame.is_empty() {
        cdma_ios::EvrcFrameType::Blank
    } else {
        format.frame_type_for_len(frame.len())?
    };
    let body = format.encode_payload(frame_type, frame)?;
    let mut converted = Vec::with_capacity(offset + body.len());
    converted.extend_from_slice(&packet[..offset]);
    converted[1] = (converted[1] & RTP_MARKER_MASK) | EVRC_PAYLOAD_TYPE;
    converted.extend_from_slice(&body);
    Some(converted)
}

fn remove_a2p_toc(packet: &[u8], codec: RelayCodec, fap_payload_type: u8) -> Option<Vec<u8>> {
    let offset = rtp_payload_offset(packet)?;
    if packet[1] & RTP_PAYLOAD_TYPE_MASK != EVRC_PAYLOAD_TYPE {
        return Some(packet.to_vec());
    }
    let mut frames = codec.bearer_format().decode_payload(&packet[offset..])?;
    if frames.len() != 1 {
        return None;
    }
    let (_, frame) = frames.pop()?;
    let mut converted = Vec::with_capacity(offset + frame.len());
    converted.extend_from_slice(&packet[..offset]);
    converted[1] = (converted[1] & RTP_MARKER_MASK) | fap_payload_type;
    converted.extend_from_slice(&frame);
    Some(converted)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_audio_endpoint() {
        let endpoint =
            parse_sdp_audio(b"v=0\r\nc=IN IP4 198.51.100.1\r\nm=audio 19000 RTP/AVP 96 101\r\n")
                .expect("SDP endpoint");
        assert_eq!(endpoint.address, Ipv4Addr::new(198, 51, 100, 1));
        assert_eq!(endpoint.port, 19_000);
        assert_eq!(endpoint.payload_types, vec![96, 101]);
    }

    #[test]
    fn preserves_codec_attributes_when_rewriting_transport() {
        let original = b"v=0\r\no=HubBub 256 2 IN IP4 198.51.100.1\r\ns=Airave\r\nc=IN IP4 198.51.100.1\r\nt=0 0\r\nm=audio 5102 RTP/AVP 100 97\r\na=rtpmap:100 EVRC0/800\r\na=fmtp:100 maxinterleave=0\r\na=ptime:20\r\na=rtpmap:97 telephone-event/8000\r\n";

        let rewritten = rewrite_sdp_transport(original, Ipv4Addr::new(192, 0, 2, 140), 18_002)
            .expect("rewritten SDP");
        let text = String::from_utf8(rewritten).expect("UTF-8 SDP");

        assert!(text.contains("o=HubBub 256 2 IN IP4 192.0.2.140\r\n"));
        assert!(text.contains("c=IN IP4 192.0.2.140\r\n"));
        assert!(text.contains("m=audio 18002 RTP/AVP 100 97\r\n"));
        assert!(text.contains("a=rtpmap:100 EVRC0/800\r\n"));
        assert!(text.contains("a=fmtp:100 maxinterleave=0\r\n"));
        assert!(text.contains("a=ptime:20\r\n"));
        assert_eq!(sdp_speech_payload_types(text.as_bytes()), vec![100]);
    }

    #[test]
    fn builds_qcelp_offer() {
        let offer = String::from_utf8(build_sdp_offer(
            Ipv4Addr::new(192, 0, 2, 140),
            18_000,
            cdma_ios::ServiceOption::QCELP_13K.0,
        ))
        .expect("UTF-8 SDP");
        assert!(offer.contains("m=audio 18000 RTP/AVP 96 101"));
        assert!(offer.contains("a=rtpmap:96 QCELP/8000/1"));
    }

    #[tokio::test]
    async fn relay_codec_follows_assignment_service_option() {
        let pool = RtpRelayPool::new(Ipv4Addr::LOCALHOST, Ipv4Addr::LOCALHOST, 28_000, 28_010)
            .expect("relay pool");
        let relay = pool
            .open(cdma_ios::ServiceOption::QCELP_13K.0)
            .expect("relay");
        assert_eq!(*relay.codec.read().expect("codec"), RelayCodec::Qcelp13k);

        relay.set_service_option(cdma_ios::ServiceOption::EVRC_A.0);

        assert_eq!(*relay.codec.read().expect("codec"), RelayCodec::Evrc);
    }

    #[test]
    fn converts_airave_evrc_rtp_to_a2p_and_back() {
        for (payload_len, frame_type) in [(2, 1), (5, 2), (10, 3), (22, 4)] {
            let mut airave = vec![0x80, EVRC_PAYLOAD_TYPE, 0, 1, 0, 0, 0, 160, 1, 2, 3, 4];
            airave.extend(vec![0x5a; payload_len]);

            let a2p =
                add_a2p_toc(&airave, RelayCodec::Evrc, EVRC_PAYLOAD_TYPE).expect("A2p packet");
            assert_eq!(a2p.len(), airave.len() + 3);
            assert_eq!(
                &a2p[RTP_FIXED_HEADER_LEN..RTP_FIXED_HEADER_LEN + 3],
                &[0x00, 0x00, frame_type << 4]
            );
            assert_eq!(
                remove_a2p_toc(&a2p, RelayCodec::Evrc, EVRC_PAYLOAD_TYPE).expect("Airave packet"),
                airave
            );
        }
    }

    #[test]
    fn translates_negotiated_fap_voice_payload_type() {
        let fap_payload_type = 100;
        let mut airave = vec![0x80, fap_payload_type, 0, 1, 0, 0, 0, 160, 1, 2, 3, 4];
        airave.extend([0x5a; 2]);

        let a2p = add_a2p_toc(&airave, RelayCodec::Evrc, fap_payload_type).expect("A2p packet");
        assert_eq!(a2p[1] & 0x7f, EVRC_PAYLOAD_TYPE);
        let returned =
            remove_a2p_toc(&a2p, RelayCodec::Evrc, fap_payload_type).expect("FAP packet");
        assert_eq!(returned, airave);
    }

    #[test]
    fn converts_airave_qcelp_rtp_to_a2p_and_back() {
        for (payload_len, frame_type) in [(3, 1), (7, 2), (16, 3), (34, 4)] {
            let mut airave = vec![0x80, EVRC_PAYLOAD_TYPE, 0, 1, 0, 0, 0, 160, 1, 2, 3, 4];
            airave.extend(vec![0xa5; payload_len]);

            let a2p =
                add_a2p_toc(&airave, RelayCodec::Qcelp13k, EVRC_PAYLOAD_TYPE).expect("A2p packet");
            assert_eq!(
                &a2p[RTP_FIXED_HEADER_LEN..RTP_FIXED_HEADER_LEN + 2],
                &[0x00, frame_type]
            );
            assert_eq!(
                remove_a2p_toc(&a2p, RelayCodec::Qcelp13k, EVRC_PAYLOAD_TYPE)
                    .expect("Airave packet"),
                airave
            );
        }
    }

    #[test]
    fn leaves_telephone_event_rtp_unchanged() {
        let packet = vec![
            0x80,
            DTMF_PAYLOAD_TYPE,
            0,
            1,
            0,
            0,
            0,
            160,
            1,
            2,
            3,
            4,
            5,
            0,
            0,
            160,
        ];
        assert_eq!(
            add_a2p_toc(&packet, RelayCodec::Evrc, EVRC_PAYLOAD_TYPE),
            Some(packet.clone())
        );
        assert_eq!(
            remove_a2p_toc(&packet, RelayCodec::Evrc, EVRC_PAYLOAD_TYPE),
            Some(packet)
        );
    }

    #[test]
    fn standard_media_profile_does_not_apply_airave_conversion() {
        let mut packet = vec![0x80, EVRC_PAYLOAD_TYPE, 0, 1, 0, 0, 0, 160, 1, 2, 3, 4];
        packet.extend([0x5a; 22]);

        assert_eq!(
            fap_to_core_packet(
                &packet,
                RelayCodec::Evrc,
                EVRC_PAYLOAD_TYPE,
                MediaPacketFormat::StandardRtp,
            ),
            Some(packet.clone())
        );
        assert_eq!(
            core_to_fap_packet(
                &packet,
                RelayCodec::Evrc,
                EVRC_PAYLOAD_TYPE,
                MediaPacketFormat::StandardRtp,
            ),
            Some(packet)
        );
    }

    #[tokio::test]
    async fn binds_fap_and_msc_relay_sockets_on_separate_addresses() {
        let pool = RtpRelayPool::new(Ipv4Addr::LOCALHOST, Ipv4Addr::UNSPECIFIED, 19_000, 19_003)
            .expect("relay pool");

        let relay = pool.open(cdma_ios::ServiceOption::EVRC_A.0).expect("relay");

        assert_eq!(relay.fap_local().ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));
        assert_eq!(relay.msc_local().ip(), IpAddr::V4(Ipv4Addr::UNSPECIFIED));
    }
}
