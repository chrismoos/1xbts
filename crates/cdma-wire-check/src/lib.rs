//! Writes IOS messages into pcap files and runs `tshark` over them.
//!
//! The capture tests skip when `tshark` is not installed, unless
//! `REQUIRE_TSHARK` is set, in which case a missing `tshark` fails them.

use std::fs;
use std::io;
use std::net::Ipv4Addr;
use std::path::{Path, PathBuf};
use std::process::Command;

/// pcap LINKTYPE_IPV4: each record starts at the IPv4 header.
const LINKTYPE_IPV4: u32 = 228;
const PCAP_MAGIC: u32 = 0xa1b2_c3d4;
const PCAP_VERSION_MAJOR: u16 = 2;
const PCAP_VERSION_MINOR: u16 = 4;
const PCAP_SNAPLEN: u32 = 65_535;

const IPV4_VERSION_IHL: u8 = 0x45;
const IPV4_HEADER_LEN: usize = 20;
const IPV4_DEFAULT_TTL: u8 = 64;
pub const IP_PROTOCOL_UDP: u8 = 17;
pub const IP_PROTOCOL_GRE: u8 = 47;
pub const IP_PROTOCOL_SCTP: u8 = 132;

const UDP_HEADER_LEN: usize = 8;
const SCTP_COMMON_HEADER_LEN: usize = 12;
const SCTP_DATA_CHUNK_HEADER_LEN: usize = 16;
const SCTP_CHUNK_DATA: u8 = 0;
/// RFC 9260 §3.3.1 DATA flags: an unfragmented message (B and E set).
const SCTP_DATA_FLAGS_UNFRAGMENTED: u8 = 0x03;
const SCTP_CHUNK_ALIGNMENT: usize = 4;
const SCTP_VERIFICATION_TAG: u32 = 0x0102_0304;

pub struct Packet {
    pub src: Ipv4Addr,
    pub dst: Ipv4Addr,
    pub protocol: u8,
    pub payload: Vec<u8>,
}

impl Packet {
    pub fn udp(src: (Ipv4Addr, u16), dst: (Ipv4Addr, u16), payload: &[u8]) -> Self {
        let mut udp = Vec::with_capacity(UDP_HEADER_LEN + payload.len());
        udp.extend_from_slice(&src.1.to_be_bytes());
        udp.extend_from_slice(&dst.1.to_be_bytes());
        udp.extend_from_slice(&((UDP_HEADER_LEN + payload.len()) as u16).to_be_bytes());
        udp.extend_from_slice(&0u16.to_be_bytes());
        udp.extend_from_slice(payload);
        Self {
            src: src.0,
            dst: dst.0,
            protocol: IP_PROTOCOL_UDP,
            payload: udp,
        }
    }

    pub fn gre(src: Ipv4Addr, dst: Ipv4Addr, frame: &[u8]) -> Self {
        Self {
            src,
            dst,
            protocol: IP_PROTOCOL_GRE,
            payload: frame.to_vec(),
        }
    }

    /// One SCTP DATA chunk. The CRC32c field is left zero, which `tshark`
    /// does not check by default.
    pub fn sctp_data(
        src: (Ipv4Addr, u16),
        dst: (Ipv4Addr, u16),
        tsn: u32,
        stream: u16,
        ppid: u32,
        data: &[u8],
    ) -> Self {
        let mut sctp = Vec::new();
        sctp.extend_from_slice(&src.1.to_be_bytes());
        sctp.extend_from_slice(&dst.1.to_be_bytes());
        sctp.extend_from_slice(&SCTP_VERIFICATION_TAG.to_be_bytes());
        sctp.extend_from_slice(&0u32.to_be_bytes());
        debug_assert_eq!(sctp.len(), SCTP_COMMON_HEADER_LEN);
        sctp.push(SCTP_CHUNK_DATA);
        sctp.push(SCTP_DATA_FLAGS_UNFRAGMENTED);
        sctp.extend_from_slice(&((SCTP_DATA_CHUNK_HEADER_LEN + data.len()) as u16).to_be_bytes());
        sctp.extend_from_slice(&tsn.to_be_bytes());
        sctp.extend_from_slice(&stream.to_be_bytes());
        sctp.extend_from_slice(&(tsn as u16).to_be_bytes());
        sctp.extend_from_slice(&ppid.to_be_bytes());
        sctp.extend_from_slice(data);
        while sctp.len() % SCTP_CHUNK_ALIGNMENT != 0 {
            sctp.push(0);
        }
        Self {
            src: src.0,
            dst: dst.0,
            protocol: IP_PROTOCOL_SCTP,
            payload: sctp,
        }
    }

    fn ipv4(&self) -> Vec<u8> {
        let total = IPV4_HEADER_LEN + self.payload.len();
        let mut ip = Vec::with_capacity(total);
        ip.push(IPV4_VERSION_IHL);
        ip.push(0);
        ip.extend_from_slice(&(total as u16).to_be_bytes());
        ip.extend_from_slice(&[0, 0, 0, 0]);
        ip.push(IPV4_DEFAULT_TTL);
        ip.push(self.protocol);
        ip.extend_from_slice(&[0, 0]);
        ip.extend_from_slice(&self.src.octets());
        ip.extend_from_slice(&self.dst.octets());
        let checksum = ipv4_checksum(&ip);
        ip[10..12].copy_from_slice(&checksum.to_be_bytes());
        ip.extend_from_slice(&self.payload);
        ip
    }
}

fn ipv4_checksum(header: &[u8]) -> u16 {
    let mut sum: u32 = header
        .chunks(2)
        .map(|word| u32::from(u16::from_be_bytes([word[0], word[1]])))
        .sum();
    while sum > 0xffff {
        sum = (sum & 0xffff) + (sum >> 16);
    }
    !(sum as u16)
}

/// Encodes packets as a little-endian pcap file, one millisecond apart.
pub fn pcap(packets: &[Packet]) -> Vec<u8> {
    let mut out = Vec::new();
    out.extend_from_slice(&PCAP_MAGIC.to_le_bytes());
    out.extend_from_slice(&PCAP_VERSION_MAJOR.to_le_bytes());
    out.extend_from_slice(&PCAP_VERSION_MINOR.to_le_bytes());
    out.extend_from_slice(&0i32.to_le_bytes());
    out.extend_from_slice(&0u32.to_le_bytes());
    out.extend_from_slice(&PCAP_SNAPLEN.to_le_bytes());
    out.extend_from_slice(&LINKTYPE_IPV4.to_le_bytes());
    for (index, packet) in packets.iter().enumerate() {
        let bytes = packet.ipv4();
        out.extend_from_slice(&0u32.to_le_bytes());
        out.extend_from_slice(&((index as u32) * 1000).to_le_bytes());
        out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
        out.extend_from_slice(&bytes);
    }
    out
}

/// Whether `tshark` runs. Panics when it does not and `REQUIRE_TSHARK` is
/// set.
pub fn tshark_available() -> bool {
    let available = Command::new("tshark")
        .arg("-v")
        .output()
        .is_ok_and(|output| output.status.success());
    if !available && std::env::var_os("REQUIRE_TSHARK").is_some() {
        panic!("REQUIRE_TSHARK is set but tshark did not run");
    }
    available
}

pub struct Capture {
    path: PathBuf,
    options: Vec<String>,
}

impl Capture {
    pub fn write(dir: &Path, name: &str, packets: &[Packet]) -> io::Result<Self> {
        fs::create_dir_all(dir)?;
        let path = dir.join(format!("{name}.pcap"));
        fs::write(&path, pcap(packets))?;
        Ok(Self {
            path,
            options: Vec::new(),
        })
    }

    pub fn preference(mut self, preference: &str) -> Self {
        self.options.push("-o".into());
        self.options.push(preference.into());
        self
    }

    pub fn decode_as(mut self, rule: &str) -> Self {
        self.options.push("-d".into());
        self.options.push(rule.into());
        self
    }

    fn tshark(&self, args: &[&str]) -> String {
        let output = Command::new("tshark")
            .arg("-n")
            .arg("-r")
            .arg(&self.path)
            .args(&self.options)
            .args(args)
            .output()
            .expect("tshark runs");
        let stdout = String::from_utf8_lossy(&output.stdout).into_owned();
        assert!(
            output.status.success(),
            "tshark failed on {}: {}{}",
            self.path.display(),
            stdout,
            String::from_utf8_lossy(&output.stderr)
        );
        stdout
    }

    /// One line per packet holding the given fields, tab separated. A field
    /// that occurs more than once in a packet is joined with commas.
    pub fn fields(&self, fields: &[&str]) -> Vec<String> {
        let mut args = vec!["-T", "fields", "-E", "occurrence=a", "-E", "aggregator=,"];
        for field in fields {
            args.push("-e");
            args.push(field);
        }
        self.tshark(&args).lines().map(str::to_owned).collect()
    }

    /// Fails when any dissector reports a malformed packet or an expert
    /// warning or error.
    pub fn assert_clean(&self) {
        let report = self.tshark(&["-q", "-z", "expert,warn"]);
        assert!(
            !report.contains("Errors (") && !report.contains("Warns ("),
            "tshark reported problems in {}:\n{report}",
            self.path.display()
        );
    }
}
