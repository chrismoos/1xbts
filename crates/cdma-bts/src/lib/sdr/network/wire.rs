//! UDP data-plane framing: fixed 36-byte big-endian header followed by
//! `sample_count` complex samples packed as Q15 i16 I/Q pairs.

use cdma_common::error::Error;
use num_complex::Complex32;

pub const WIRE_VERSION: u8 = 2;
pub const HEADER_LEN: usize = 36;
pub const BYTES_PER_SAMPLE: usize = 4;

/// Default keeps header + payload under a 1500-byte MTU with IP/UDP overhead.
pub const DEFAULT_SAMPLES_PER_PACKET: usize = 350;
/// Cap fits a jumbo (9000-byte) frame.
pub const MAX_SAMPLES_PER_PACKET: usize = 2200;

const Q15_SCALE: f32 = 32767.0;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TxTransport {
    /// Datagrams. Lowest overhead, and whatever a hop drops is gone: the
    /// server zero-fills the hole and transmits it.
    #[default]
    Udp,
    Tcp,
}

/// The receive stream saw a discontinuity (packet loss or hardware
/// overflow) immediately before this packet's first sample.
pub const FLAG_RX_DISCONTINUITY: u16 = 1 << 0;
pub const FLAG_TX_NO_TICK: u16 = 1 << 1;
/// A block is one `RadioTx::transmit(_at)` call, split across packets.
pub const FLAG_TX_BLOCK_START: u16 = 1 << 2;
pub const FLAG_TX_BLOCK_END: u16 = 1 << 3;
/// Stop the transmitter after every preceding packet on this data stream.
pub const FLAG_TX_DISABLE: u16 = 1 << 4;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamKind {
    /// Client → server. Teaches the server the client's data-plane address.
    Hello = 1,
    TxSamples = 2,
    RxSamples = 3,
    ClockHeartbeat = 4,
}

impl StreamKind {
    fn from_wire(value: u8) -> Result<Self, Error> {
        match value {
            1 => Ok(Self::Hello),
            2 => Ok(Self::TxSamples),
            3 => Ok(Self::RxSamples),
            4 => Ok(Self::ClockHeartbeat),
            other => Err(format!("network radio: unknown stream kind {other}").into()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PacketHeader {
    pub stream: StreamKind,
    pub flags: u16,
    pub session_id: u32,
    pub seq: u32,
    pub time_ns: u64,
    pub sample_count: u16,
    /// Zero for non-TX packet kinds.
    pub block_id: u32,
    pub block_offset: u32,
    pub block_sample_count: u32,
}

pub fn encode_packet(header: &PacketHeader, samples: &[Complex32], out: &mut Vec<u8>) {
    debug_assert_eq!(header.sample_count as usize, samples.len());
    out.clear();
    out.reserve(HEADER_LEN + samples.len() * BYTES_PER_SAMPLE);
    out.push(WIRE_VERSION);
    out.push(header.stream as u8);
    out.extend_from_slice(&header.flags.to_be_bytes());
    out.extend_from_slice(&header.session_id.to_be_bytes());
    out.extend_from_slice(&header.seq.to_be_bytes());
    out.extend_from_slice(&header.time_ns.to_be_bytes());
    out.extend_from_slice(&header.sample_count.to_be_bytes());
    out.extend_from_slice(&0u16.to_be_bytes());
    out.extend_from_slice(&header.block_id.to_be_bytes());
    out.extend_from_slice(&header.block_offset.to_be_bytes());
    out.extend_from_slice(&header.block_sample_count.to_be_bytes());
    for sample in samples {
        out.extend_from_slice(&f32_to_q15(sample.re).to_be_bytes());
        out.extend_from_slice(&f32_to_q15(sample.im).to_be_bytes());
    }
}

pub fn decode_header(buf: &[u8]) -> Result<PacketHeader, Error> {
    if buf.len() < HEADER_LEN {
        return Err(format!(
            "network radio: packet shorter than header: {} < {HEADER_LEN}",
            buf.len()
        )
        .into());
    }
    if buf[0] != WIRE_VERSION {
        return Err(format!("network radio: unsupported wire version {}", buf[0]).into());
    }
    let header = PacketHeader {
        stream: StreamKind::from_wire(buf[1])?,
        flags: u16::from_be_bytes(buf[2..4].try_into().unwrap()),
        session_id: u32::from_be_bytes(buf[4..8].try_into().unwrap()),
        seq: u32::from_be_bytes(buf[8..12].try_into().unwrap()),
        time_ns: u64::from_be_bytes(buf[12..20].try_into().unwrap()),
        sample_count: u16::from_be_bytes(buf[20..22].try_into().unwrap()),
        block_id: u32::from_be_bytes(buf[24..28].try_into().unwrap()),
        block_offset: u32::from_be_bytes(buf[28..32].try_into().unwrap()),
        block_sample_count: u32::from_be_bytes(buf[32..36].try_into().unwrap()),
    };
    let expected = HEADER_LEN + header.sample_count as usize * BYTES_PER_SAMPLE;
    if buf.len() < expected {
        return Err(format!(
            "network radio: truncated payload: {} < {expected}",
            buf.len()
        )
        .into());
    }
    Ok(header)
}

pub fn payload_len(buf: &[u8]) -> Result<usize, Error> {
    if buf.len() < HEADER_LEN {
        return Err(format!(
            "network radio: packet shorter than header: {} < {HEADER_LEN}",
            buf.len()
        )
        .into());
    }
    if buf[0] != WIRE_VERSION {
        return Err(format!("network radio: unsupported wire version {}", buf[0]).into());
    }
    let sample_count = u16::from_be_bytes(buf[20..22].try_into().unwrap()) as usize;
    Ok(sample_count * BYTES_PER_SAMPLE)
}

pub fn decode_samples_into(buf: &[u8], header: &PacketHeader, out: &mut Vec<Complex32>) {
    out.clear();
    out.reserve(header.sample_count as usize);
    let payload = &buf[HEADER_LEN..HEADER_LEN + header.sample_count as usize * BYTES_PER_SAMPLE];
    for pair in payload.chunks_exact(BYTES_PER_SAMPLE) {
        let re = i16::from_be_bytes(pair[0..2].try_into().unwrap());
        let im = i16::from_be_bytes(pair[2..4].try_into().unwrap());
        out.push(Complex32::new(q15_to_f32(re), q15_to_f32(im)));
    }
}

fn f32_to_q15(value: f32) -> i16 {
    (value.clamp(-1.0, 1.0) * Q15_SCALE).round() as i16
}

fn q15_to_f32(value: i16) -> f32 {
    value as f32 / Q15_SCALE
}

pub fn samples_to_ns(samples: u64, sample_rate_hz: u64) -> u64 {
    if sample_rate_hz == 0 {
        return 0;
    }
    ((samples as u128 * 1_000_000_000u128) / sample_rate_hz as u128).min(u64::MAX as u128) as u64
}

pub fn ns_to_samples(ns: u64, sample_rate_hz: u64) -> u64 {
    ((ns as u128 * sample_rate_hz as u128) / 1_000_000_000u128).min(u64::MAX as u128) as u64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn header(sample_count: u16) -> PacketHeader {
        PacketHeader {
            stream: StreamKind::TxSamples,
            flags: FLAG_RX_DISCONTINUITY,
            session_id: 0xA1B2C3D4,
            seq: 42,
            time_ns: 123_456_789_000,
            sample_count,
            block_id: 7,
            block_offset: 350,
            block_sample_count: 700,
        }
    }

    #[test]
    fn packet_round_trip_preserves_header_and_samples() {
        let samples: Vec<Complex32> = (0..350)
            .map(|i| Complex32::new((i as f32 / 350.0) - 0.5, 0.5 - (i as f32 / 350.0)))
            .collect();
        let hdr = header(samples.len() as u16);
        let mut buf = Vec::new();
        encode_packet(&hdr, &samples, &mut buf);
        assert_eq!(buf.len(), HEADER_LEN + samples.len() * BYTES_PER_SAMPLE);

        let decoded_hdr = decode_header(&buf).expect("decode header");
        assert_eq!(decoded_hdr, hdr);
        let mut decoded = Vec::new();
        decode_samples_into(&buf, &decoded_hdr, &mut decoded);
        assert_eq!(decoded.len(), samples.len());
        for (a, b) in samples.iter().zip(decoded.iter()) {
            assert!((a.re - b.re).abs() < 1.0 / Q15_SCALE);
            assert!((a.im - b.im).abs() < 1.0 / Q15_SCALE);
        }
    }

    #[test]
    fn empty_tx_control_packet_preserves_disable_flag() {
        let mut hdr = header(0);
        hdr.flags = FLAG_TX_DISABLE;
        hdr.block_id = 0;
        hdr.block_offset = 0;
        hdr.block_sample_count = 0;
        let mut buf = Vec::new();
        encode_packet(&hdr, &[], &mut buf);

        assert_eq!(buf.len(), HEADER_LEN);
        assert_eq!(decode_header(&buf).expect("decode header"), hdr);
    }

    #[test]
    fn q15_clamps_out_of_range_values() {
        assert_eq!(f32_to_q15(2.0), i16::MAX);
        assert_eq!(f32_to_q15(-2.0), -i16::MAX);
    }

    #[test]
    fn default_packet_fits_standard_mtu() {
        const MAX_UDP_PAYLOAD_STANDARD_MTU: usize = 1472;
        const {
            assert!(
                HEADER_LEN + DEFAULT_SAMPLES_PER_PACKET * BYTES_PER_SAMPLE
                    <= MAX_UDP_PAYLOAD_STANDARD_MTU
            );
        }
    }

    #[test]
    fn decode_rejects_bad_version_and_truncation() {
        let samples = vec![Complex32::new(0.1, -0.1); 8];
        let hdr = header(samples.len() as u16);
        let mut buf = Vec::new();
        encode_packet(&hdr, &samples, &mut buf);

        let mut bad_version = buf.clone();
        bad_version[0] = 9;
        assert!(decode_header(&bad_version).is_err());

        assert!(decode_header(&buf[..HEADER_LEN - 1]).is_err());
        assert!(decode_header(&buf[..buf.len() - 1]).is_err());
    }

    #[test]
    fn sample_time_conversions_round_trip() {
        let rate = 9_830_400u64;
        assert_eq!(ns_to_samples(samples_to_ns(12_288, rate), rate), 12_288);
        assert_eq!(samples_to_ns(0, rate), 0);
        assert_eq!(samples_to_ns(5, 0), 0);
    }
}
