use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};

use crate::ppp::framing::{FrameOptions, PppPacket};

const PCAP_MAGIC_USEC: u32 = 0xa1b2c3d4;
const PCAP_VERSION_MAJOR: u16 = 2;
const PCAP_VERSION_MINOR: u16 = 4;
const PCAP_SNAPLEN: u32 = 65535;

const LINKTYPE_PPP_WITH_DIR: u32 = 204;
const LINKTYPE_USER0: u32 = 147;

const RLP_RECORD_MAGIC: &[u8; 4] = b"CRLP";
const RLP_RECORD_VERSION: u8 = 1;

const DEFAULT_PPP_PCAP_PATH: &str = "/tmp/cdma-packet-ppp.pcap";
const DEFAULT_RLP_PCAP_PATH: &str = "/tmp/cdma-packet-rlp.pcap";

#[derive(Clone, Copy)]
pub enum Direction {
    Uplink,
    Downlink,
}

impl Direction {
    fn as_u8(self) -> u8 {
        match self {
            Self::Uplink => 0,
            Self::Downlink => 1,
        }
    }
}

struct PcapWriter {
    writer: BufWriter<File>,
}

impl PcapWriter {
    fn create(path: &Path, linktype: u32) -> std::io::Result<Self> {
        let file = File::create(path)?;
        let mut writer = BufWriter::new(file);
        writer.write_all(&PCAP_MAGIC_USEC.to_le_bytes())?;
        writer.write_all(&PCAP_VERSION_MAJOR.to_le_bytes())?;
        writer.write_all(&PCAP_VERSION_MINOR.to_le_bytes())?;
        writer.write_all(&0i32.to_le_bytes())?;
        writer.write_all(&0u32.to_le_bytes())?;
        writer.write_all(&PCAP_SNAPLEN.to_le_bytes())?;
        writer.write_all(&linktype.to_le_bytes())?;
        writer.flush()?;
        Ok(Self { writer })
    }

    fn write_packet(&mut self, payload: &[u8]) {
        let now = match SystemTime::now().duration_since(UNIX_EPOCH) {
            Ok(d) => d,
            Err(e) => {
                log::debug!("capture: timestamp error: {}", e);
                return;
            }
        };
        let ts_sec = now.as_secs() as u32;
        let ts_usec = now.subsec_micros();
        let len = payload.len() as u32;
        if let Err(e) = self.writer.write_all(&ts_sec.to_le_bytes()) {
            log::debug!("capture: failed writing ts_sec: {}", e);
            return;
        }
        if let Err(e) = self.writer.write_all(&ts_usec.to_le_bytes()) {
            log::debug!("capture: failed writing ts_usec: {}", e);
            return;
        }
        if let Err(e) = self.writer.write_all(&len.to_le_bytes()) {
            log::debug!("capture: failed writing incl_len: {}", e);
            return;
        }
        if let Err(e) = self.writer.write_all(&len.to_le_bytes()) {
            log::debug!("capture: failed writing orig_len: {}", e);
            return;
        }
        if let Err(e) = self.writer.write_all(payload) {
            log::debug!("capture: failed writing payload: {}", e);
            return;
        }
        if let Err(e) = self.writer.flush() {
            log::debug!("capture: failed flushing packet: {}", e);
        }
    }
}

static PPP_WRITER: OnceLock<Option<Mutex<PcapWriter>>> = OnceLock::new();
static RLP_WRITER: OnceLock<Option<Mutex<PcapWriter>>> = OnceLock::new();
static CAPTURE_ENABLED: OnceLock<bool> = OnceLock::new();

fn capture_enabled() -> bool {
    *CAPTURE_ENABLED.get_or_init(|| {
        std::env::var("CDMA_PACKET_CAPTURE")
            .ok()
            .map(|v| {
                matches!(
                    v.as_str(),
                    "1" | "true" | "TRUE" | "yes" | "YES" | "on" | "ON"
                )
            })
            .unwrap_or(false)
    })
}

fn ppp_writer() -> Option<&'static Mutex<PcapWriter>> {
    if !capture_enabled() {
        return None;
    }
    PPP_WRITER
        .get_or_init(|| open_writer("PPP PCAP", DEFAULT_PPP_PCAP_PATH, LINKTYPE_PPP_WITH_DIR))
        .as_ref()
}

fn rlp_writer() -> Option<&'static Mutex<PcapWriter>> {
    if !capture_enabled() {
        return None;
    }
    RLP_WRITER
        .get_or_init(|| open_writer("RLP PCAP", DEFAULT_RLP_PCAP_PATH, LINKTYPE_USER0))
        .as_ref()
}

fn open_writer(label: &str, default_path: &str, linktype: u32) -> Option<Mutex<PcapWriter>> {
    let path = PathBuf::from(std::env::var("CDMA_PACKET_CAPTURE_DIR").ok().map_or_else(
        || default_path.to_string(),
        |dir| {
            let filename = if linktype == LINKTYPE_PPP_WITH_DIR {
                "cdma-packet-ppp.pcap"
            } else {
                "cdma-packet-rlp.pcap"
            };
            Path::new(&dir)
                .join(filename)
                .to_string_lossy()
                .into_owned()
        },
    ));
    match PcapWriter::create(&path, linktype) {
        Ok(writer) => {
            log::info!("capture: {} writing to {}", label, path.display());
            Some(Mutex::new(writer))
        }
        Err(e) => {
            log::warn!(
                "capture: failed to open {} at {}: {}",
                label,
                path.display(),
                e
            );
            None
        }
    }
}

pub fn write_ppp_packet(direction: Direction, packet: &PppPacket, opts: &FrameOptions) {
    if let Some(writer) = ppp_writer()
        && let Ok(mut writer) = writer.lock()
    {
        writer.write_packet(&encode_ppp_packet_with_direction(direction, packet, opts));
    }
}

pub fn write_rlp_frame(direction: Direction, rate_bps: u32, bits: &[u8]) {
    if let Some(writer) = rlp_writer()
        && let Ok(mut writer) = writer.lock()
    {
        let mut record = Vec::with_capacity(14 + bits.len());
        record.extend_from_slice(RLP_RECORD_MAGIC);
        record.push(RLP_RECORD_VERSION);
        record.push(direction.as_u8());
        record.extend_from_slice(&0u16.to_le_bytes());
        record.extend_from_slice(&rate_bps.to_le_bytes());
        record.extend_from_slice(&(bits.len() as u16).to_le_bytes());
        record.extend_from_slice(&0u16.to_le_bytes());
        record.extend_from_slice(bits);
        writer.write_packet(&record);
    }
}

fn encode_ppp_packet_with_direction(
    direction: Direction,
    packet: &PppPacket,
    opts: &FrameOptions,
) -> Vec<u8> {
    let mut out = Vec::with_capacity(1 + 4 + packet.payload.len());
    out.push(direction.as_u8());

    let is_lcp = packet.protocol == crate::ppp::lcp::LCP_PROTOCOL;

    if !opts.acfc || is_lcp {
        out.push(0xff);
        out.push(0x03);
    }

    if opts.pfc && !is_lcp && (packet.protocol >> 8) == 0 && (packet.protocol & 0x01) != 0 {
        out.push(packet.protocol as u8);
    } else {
        out.push((packet.protocol >> 8) as u8);
        out.push((packet.protocol & 0xff) as u8);
    }

    out.extend_from_slice(&packet.payload);
    out
}
