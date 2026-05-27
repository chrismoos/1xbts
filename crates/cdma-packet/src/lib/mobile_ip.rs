//! Mobile IPv4 control packets used by cdma2000 packet data.

use std::net::Ipv4Addr;
use std::time::{SystemTime, UNIX_EPOCH};

pub const UDP_PORT_MOBILE_IP: u16 = 434;

const DEFAULT_AGENT_ADDRESS: Ipv4Addr = Ipv4Addr::new(10, 55, 0, 1);
const DEFAULT_ADVERTISEMENT_COUNT: u8 = 3;
const DEFAULT_REGISTRATION_LIFETIME_SECS: u16 = 1200;

const IP_PROTO_ICMP: u8 = 1;
const IP_PROTO_UDP: u8 = 17;
const IPV4_VERSION: u8 = 4;
const IPV4_VERSION_SHIFT: u8 = 4;
const IPV4_IHL_MASK: u8 = 0x0f;
const IPV4_IHL_WORD_BYTES: usize = 4;
const IPV4_MIN_HEADER_LEN: usize = 20;
const IPV4_DEFAULT_TTL: u8 = 64;
const IPV4_HEADER_VERSION_IHL_NO_OPTIONS: u8 = 0x45;
const IPV4_TOS_DEFAULT: u8 = 0;
const IPV4_IDENTIFICATION_DEFAULT: u16 = 0;
const IPV4_FLAGS_FRAGMENT_OFFSET_DEFAULT: u16 = 0;
const IPV4_TOTAL_LEN_OFFSET: usize = 2;
const IPV4_PROTOCOL_OFFSET: usize = 9;
const IPV4_CHECKSUM_OFFSET: usize = 10;
const IPV4_SOURCE_OFFSET: usize = 12;
const IPV4_DESTINATION_OFFSET: usize = 16;
const CHECKSUM_PLACEHOLDER: u16 = 0;

const ICMP_ROUTER_ADVERTISEMENT: u8 = 9;
const ICMP_ROUTER_SOLICITATION: u8 = 10;
const ICMP_CODE_DEFAULT: u8 = 0;
const ICMP_CHECKSUM_OFFSET: usize = 2;
const ICMP_ROUTER_ADVERTISEMENT_ADDR_COUNT: u8 = 1;
const ICMP_ROUTER_ADVERTISEMENT_ENTRY_SIZE_WORDS: u8 = 2;
const ICMP_ROUTER_ADVERTISEMENT_LIFETIME_SECS: u16 = 9000;

const MIP_AGENT_ADVERTISEMENT_EXT: u8 = 16;
const MIP_AGENT_ADVERTISEMENT_EXT_LEN: u8 = 10;
const MIP_AGENT_ADVERTISEMENT_FLAG_FOREIGN_AGENT: u8 = 0x10;
const MIP_AGENT_ADVERTISEMENT_RESERVED: u8 = 0;
const MIP_AGENT_ADVERTISEMENT_CHALLENGE_EXT: u8 = 24;
const MIP_MOBILE_HOME_AUTH_EXT: u8 = 32;
const MIP_MOBILE_FOREIGN_AUTH_EXT: u8 = 33;
const MIP_GENERALIZED_AUTH_EXT: u8 = 36;
const MIP_GENERALIZED_AUTH_SUBTYPE_MN_AAA: u8 = 1;
const MIP_GENERALIZED_AUTH_HEADER_LEN: usize = 4;
const MIP_GENERALIZED_AUTH_SUBTYPE_OFFSET: usize = 1;
const MIP_GENERALIZED_AUTH_LENGTH_OFFSET: usize = 2;
const MIP_GENERALIZED_AUTH_VALUE_OFFSET: usize = 4;
const MIP_MN_NAI_EXT: u8 = 131;
const MIP_MOBILE_FOREIGN_CHALLENGE_EXT: u8 = 132;
const MIP_EXTENSION_PAD: u8 = 0;
const MIP_EXTENSION_HEADER_LEN: usize = 2;
const MIP_CHALLENGE_LEN: usize = 16;

const MIP_RRQ_TYPE: u8 = 1;
const MIP_RRP_TYPE: u8 = 3;
const MIP_RRQ_FIXED_LEN: usize = 24;
#[cfg(test)]
const MIP_RRP_FIXED_LEN: usize = 20;
const MIP_RRQ_FLAGS_OFFSET: usize = 1;
const MIP_RRQ_LIFETIME_OFFSET: usize = 2;
const MIP_RRQ_HOME_ADDRESS_OFFSET: usize = 4;
const MIP_RRQ_HOME_AGENT_OFFSET: usize = 8;
const MIP_RRQ_CARE_OF_ADDRESS_OFFSET: usize = 12;
const MIP_RRQ_IDENTIFICATION_OFFSET: usize = 16;
const MIP_RRQ_EXTENSIONS_OFFSET: usize = MIP_RRQ_FIXED_LEN;
const MIP_LIFETIME_DEREGISTER: u16 = 0;

#[cfg(test)]
const RRQ_FLAG_NONE: u8 = 0;
const RRQ_FLAG_SIMULTANEOUS_BINDINGS: u8 = 0x80;
const RRQ_FLAG_BROADCAST_DATAGRAMS: u8 = 0x40;
const RRQ_FLAG_D: u8 = 0x20;
const RRQ_FLAG_MINIMAL_ENCAPSULATION: u8 = 0x10;
const RRQ_FLAG_GRE_ENCAPSULATION: u8 = 0x08;
const RRQ_FLAG_VJ_COMPRESSION: u8 = 0x04;
const RRQ_FLAG_REVERSE_TUNNEL: u8 = 0x02;
const RRQ_FLAG_RESERVED_0X01: u8 = 0x01;

const RRP_CODE_ACCEPTED: u8 = 0;
const RRP_CODE_ACCEPTED_SIMULTANEOUS_UNSUPPORTED: u8 = 1;
const RRP_CODE_POORLY_FORMED_REQUEST: u8 = 64;
const RRP_CODE_ADMINISTRATIVELY_PROHIBITED: u8 = 65;
const RRP_CODE_INSUFFICIENT_RESOURCES: u8 = 66;
const RRP_CODE_MOBILE_NODE_AUTH_FAILED: u8 = 67;
const RRP_CODE_HOME_AGENT_AUTH_FAILED: u8 = 68;
const RRP_CODE_LIFETIME_TOO_LONG: u8 = 69;
const RRP_CODE_POORLY_FORMED_REPLY: u8 = 70;
const RRP_CODE_POORLY_FORMED_REQUEST_REVERSE_TUNNEL: u8 = 71;
const RRP_CODE_FA_ENCAPSULATION_UNAVAILABLE: u8 = 72;
const RRP_CODE_VJ_COMPRESSION_UNAVAILABLE: u8 = 73;
const RRP_CODE_REVERSE_TUNNEL_UNAVAILABLE: u8 = 74;
const RRP_CODE_REVERSE_TUNNEL_MANDATORY: u8 = 75;
const RRP_CODE_DELIVERY_STYLE_UNSUPPORTED: u8 = 76;
const RRP_CODE_MISSING_NAI: u8 = 77;
const RRP_CODE_MISSING_HOME_AGENT: u8 = 78;
const RRP_CODE_MISSING_HOME_ADDRESS: u8 = 79;
const RRP_CODE_UNKNOWN_CHALLENGE: u8 = 80;
const RRP_CODE_MISSING_CHALLENGE: u8 = 81;
const RRP_CODE_STALE_CHALLENGE: u8 = 82;
const RRP_CODE_UNKNOWN_HOME_AGENT: u8 = 88;
const RRP_CODE_REQUESTED_HOME_AGENT_UNAVAILABLE: u8 = 89;
const RRP_CODE_NONZERO_HOME_ADDRESS_REQUIRED: u8 = 96;
const RRP_CODE_MISSING_HOME_AGENT_NAI: u8 = 97;
const RRP_CODE_MISSING_HOME_ADDRESS_NAI: u8 = 98;
const RRP_CODE_MISSING_NAI_HOME_AGENT_HOME_ADDRESS: u8 = 99;
const RRP_CODE_HA_REASON_UNSPECIFIED: u8 = 128;
const RRP_CODE_HA_ADMIN_PROHIBITED: u8 = 129;
const RRP_CODE_HA_INSUFFICIENT_RESOURCES: u8 = 130;
const RRP_CODE_HA_MOBILE_NODE_AUTH_FAILED: u8 = 131;
const RRP_CODE_HA_FOREIGN_AGENT_AUTH_FAILED: u8 = 132;
const RRP_CODE_HA_REGISTRATION_ID_MISMATCH: u8 = 133;
const RRP_CODE_HA_POORLY_FORMED_REQUEST: u8 = 134;
const RRP_CODE_HA_TOO_MANY_BINDINGS: u8 = 135;
const RRP_CODE_HA_UNKNOWN_HOME_AGENT_ADDRESS: u8 = 136;
const RRP_CODE_HA_REVERSE_TUNNEL_UNAVAILABLE: u8 = 137;
const RRP_CODE_HA_REVERSE_TUNNEL_MANDATORY: u8 = 138;
const RRP_CODE_HA_ENCAPSULATION_UNAVAILABLE: u8 = 139;

const UDP_HEADER_LEN: usize = 8;
const UDP_LEN_OFFSET: usize = 4;
const UDP_SOURCE_PORT_OFFSET: usize = 0;
const UDP_DESTINATION_PORT_OFFSET: usize = 2;
#[cfg(test)]
const UDP_CHECKSUM_OFFSET: usize = 6;
const UDP_PAYLOAD_OFFSET: usize = UDP_HEADER_LEN;
const UDP_CHECKSUM_UNUSED: u16 = 0;

const CHECKSUM_U16_MAX: u32 = 0xffff;
const CHECKSUM_WORD_BYTES: usize = 2;
const AUTH_EXTENSION_SPI_LEN: usize = 4;
const HEX_BYTE_CHARS: usize = 2;
const HEX_U32_WIDTH: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MobileIpAuthMode {
    Insecure,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileIpConfig {
    pub enabled: bool,
    pub fa_address: Ipv4Addr,
    pub home_agent_address: Ipv4Addr,
    pub advertisement_count: u8,
    pub advertisement_lifetime_secs: u16,
    pub registration_lifetime_secs: u16,
    pub auth_mode: MobileIpAuthMode,
}

impl Default for MobileIpConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            fa_address: DEFAULT_AGENT_ADDRESS,
            home_agent_address: DEFAULT_AGENT_ADDRESS,
            advertisement_count: DEFAULT_ADVERTISEMENT_COUNT,
            advertisement_lifetime_secs: ICMP_ROUTER_ADVERTISEMENT_LIFETIME_SECS,
            registration_lifetime_secs: DEFAULT_REGISTRATION_LIFETIME_SECS,
            auth_mode: MobileIpAuthMode::Insecure,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileIpBinding {
    pub nai: Option<String>,
    pub home_address: Ipv4Addr,
    pub home_agent: Ipv4Addr,
    pub identification: u64,
    pub lifetime_secs: u16,
}

#[derive(Debug, Clone)]
pub struct MobileIpSession {
    config: MobileIpConfig,
    advertisements_sent: u8,
    sequence: u16,
    challenge_counter: u64,
    challenge: Vec<u8>,
    binding: Option<MobileIpBinding>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MobileIpPacketResult {
    NotMobileIp,
    Ignored,
    AuthenticationRequired,
    Reply(Vec<u8>),
    Deregistered {
        reply: Vec<u8>,
    },
    Registered {
        binding: MobileIpBinding,
        reply: Vec<u8>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RegistrationRequest {
    pub flags: u8,
    pub lifetime_secs: u16,
    pub home_address: Ipv4Addr,
    pub home_agent: Ipv4Addr,
    pub care_of_address: Ipv4Addr,
    pub identification: u64,
    pub extensions: Vec<MobileIpExtension>,
}

impl RegistrationRequest {
    pub fn nai(&self) -> Option<String> {
        self.extensions.iter().find_map(|ext| match ext {
            MobileIpExtension::MnNai(bytes) => Some(String::from_utf8_lossy(bytes).into_owned()),
            _ => None,
        })
    }

    pub fn has_mn_ha_auth(&self) -> bool {
        self.extensions
            .iter()
            .any(|ext| matches!(ext, MobileIpExtension::MobileHomeAuth(_)))
    }

    pub fn has_mn_aaa_auth(&self) -> bool {
        self.extensions
            .iter()
            .any(|ext| matches!(ext, MobileIpExtension::MnAaaAuth(_)))
    }

    pub fn has_mn_fa_challenge(&self) -> bool {
        self.extensions
            .iter()
            .any(|ext| matches!(ext, MobileIpExtension::MnFaChallenge(_)))
    }

    pub fn has_mn_fa_auth(&self) -> bool {
        self.extensions
            .iter()
            .any(|ext| matches!(ext, MobileIpExtension::MobileForeignAuth(_)))
    }

    pub fn unknown_extension_count(&self) -> usize {
        self.extensions
            .iter()
            .filter(|ext| matches!(ext, MobileIpExtension::Unknown { .. }))
            .count()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MobileIpExtension {
    MnFaChallenge(Vec<u8>),
    MobileHomeAuth(Vec<u8>),
    MobileForeignAuth(Vec<u8>),
    MnAaaAuth(Vec<u8>),
    MnNai(Vec<u8>),
    Unknown {
        extension_type: u8,
        subtype: Option<u8>,
        data: Vec<u8>,
    },
}

impl MobileIpSession {
    pub fn new(config: MobileIpConfig) -> Self {
        let mut session = Self {
            config,
            advertisements_sent: 0,
            sequence: 0,
            challenge_counter: 0,
            challenge: Vec::new(),
            binding: None,
        };
        session.rotate_challenge();
        session
    }

    pub fn config(&self) -> &MobileIpConfig {
        &self.config
    }

    pub fn reset(&mut self, config: MobileIpConfig) {
        *self = Self::new(config);
    }

    pub fn binding(&self) -> Option<&MobileIpBinding> {
        self.binding.as_ref()
    }

    pub fn next_unsolicited_advertisement(&mut self) -> Option<Vec<u8>> {
        if !self.config.enabled || self.advertisements_sent >= self.config.advertisement_count {
            return None;
        }
        self.advertisements_sent = self.advertisements_sent.saturating_add(1);
        Some(self.agent_advertisement_packet(Ipv4Addr::BROADCAST))
    }

    pub fn handle_ipv4_packet(
        &mut self,
        packet: &[u8],
        assigned_home_address: Ipv4Addr,
    ) -> MobileIpPacketResult {
        if !self.config.enabled {
            return MobileIpPacketResult::NotMobileIp;
        }
        let Some(ip) = ParsedIpv4Packet::parse(packet) else {
            return MobileIpPacketResult::Ignored;
        };

        if ip.protocol == IP_PROTO_ICMP
            && ip.payload.first().copied() == Some(ICMP_ROUTER_SOLICITATION)
        {
            log::info!("MIP4 RX: Agent Solicitation src={}", ip.source);
            return MobileIpPacketResult::Reply(
                self.agent_advertisement_packet(ip.reply_address()),
            );
        }

        if ip.protocol != IP_PROTO_UDP {
            return MobileIpPacketResult::NotMobileIp;
        }
        let Some(udp) = ParsedUdpPacket::parse(ip.payload) else {
            return MobileIpPacketResult::Ignored;
        };
        if udp.destination_port != UDP_PORT_MOBILE_IP {
            return MobileIpPacketResult::NotMobileIp;
        }
        let Some(rrq) = parse_registration_request(udp.payload) else {
            return MobileIpPacketResult::Ignored;
        };
        let nai = rrq.nai();
        log::info!(
            "MIP4 RX: RRQ src={} dst={} udp={}->{} nai={} home={} ha={} coa={} lifetime={} id=0x{:016x} flags=0x{:02x}({}) mn_fa_challenge={} auth_details=[{}] unknown_exts={} unknown_ext_details=[{}] ext_count={}",
            ip.source,
            ip.destination,
            udp.source_port,
            udp.destination_port,
            nai.as_deref().unwrap_or("<none>"),
            rrq.home_address,
            rrq.home_agent,
            rrq.care_of_address,
            rrq.lifetime_secs,
            rrq.identification,
            rrq.flags,
            format_rrq_flags(rrq.flags),
            rrq.has_mn_fa_challenge(),
            format_auth_extensions(&rrq.extensions),
            rrq.unknown_extension_count(),
            format_unknown_extensions(&rrq.extensions),
            rrq.extensions.len()
        );

        if self.config.auth_mode == MobileIpAuthMode::Insecure
            && (rrq.has_mn_ha_auth() || rrq.has_mn_aaa_auth() || rrq.has_mn_fa_auth())
        {
            log::warn!(
                "MIP4: RRQ requires authentication but auth_mode={:?}; falling back to Simple IP",
                self.config.auth_mode
            );
            return MobileIpPacketResult::AuthenticationRequired;
        }

        if rrq.flags & RRQ_FLAG_D != 0 {
            let reply = self.registration_reply_packet(
                &ip,
                udp.source_port,
                &rrq,
                RRP_CODE_ADMINISTRATIVELY_PROHIBITED,
                rrq.home_address,
            );
            return MobileIpPacketResult::Reply(reply);
        }

        let home_address = if rrq.home_address.is_unspecified() {
            assigned_home_address
        } else {
            rrq.home_address
        };
        if rrq.lifetime_secs == MIP_LIFETIME_DEREGISTER {
            self.binding = None;
            self.rotate_challenge();
            let reply = self.registration_reply_packet(
                &ip,
                udp.source_port,
                &rrq,
                RRP_CODE_ACCEPTED,
                home_address,
            );
            return MobileIpPacketResult::Deregistered { reply };
        }
        let lifetime_secs = rrq
            .lifetime_secs
            .min(self.config.registration_lifetime_secs);
        let binding = MobileIpBinding {
            nai,
            home_address,
            home_agent: self.selected_home_agent(rrq.home_agent),
            identification: rrq.identification,
            lifetime_secs,
        };
        self.binding = Some(binding.clone());
        self.rotate_challenge();
        let reply = self.registration_reply_packet(
            &ip,
            udp.source_port,
            &rrq,
            RRP_CODE_ACCEPTED,
            home_address,
        );
        MobileIpPacketResult::Registered { binding, reply }
    }

    fn selected_home_agent(&self, requested: Ipv4Addr) -> Ipv4Addr {
        if requested.is_unspecified() || requested == Ipv4Addr::BROADCAST {
            self.config.home_agent_address
        } else {
            requested
        }
    }

    fn agent_advertisement_packet(&mut self, destination: Ipv4Addr) -> Vec<u8> {
        self.sequence = self.sequence.wrapping_add(1);
        let mut icmp = Vec::new();
        icmp.push(ICMP_ROUTER_ADVERTISEMENT);
        icmp.push(ICMP_CODE_DEFAULT);
        icmp.extend_from_slice(&0u16.to_be_bytes());
        icmp.push(ICMP_ROUTER_ADVERTISEMENT_ADDR_COUNT);
        icmp.push(ICMP_ROUTER_ADVERTISEMENT_ENTRY_SIZE_WORDS);
        icmp.extend_from_slice(&self.config.advertisement_lifetime_secs.to_be_bytes());
        icmp.extend_from_slice(&self.config.fa_address.octets());
        icmp.extend_from_slice(&0u32.to_be_bytes());
        icmp.push(MIP_AGENT_ADVERTISEMENT_EXT);
        icmp.push(MIP_AGENT_ADVERTISEMENT_EXT_LEN);
        icmp.extend_from_slice(&self.sequence.to_be_bytes());
        icmp.extend_from_slice(&self.config.registration_lifetime_secs.to_be_bytes());
        icmp.push(MIP_AGENT_ADVERTISEMENT_FLAG_FOREIGN_AGENT);
        icmp.push(MIP_AGENT_ADVERTISEMENT_RESERVED);
        icmp.extend_from_slice(&self.config.fa_address.octets());
        icmp.push(MIP_AGENT_ADVERTISEMENT_CHALLENGE_EXT);
        icmp.push(self.challenge.len() as u8);
        icmp.extend_from_slice(&self.challenge);
        fill_checksum(&mut icmp, ICMP_CHECKSUM_OFFSET);
        log::info!(
            "MIP4 TX: Agent Advertisement dst={} fa={} lifetime={} challenge_len={}",
            destination,
            self.config.fa_address,
            self.config.registration_lifetime_secs,
            self.challenge.len()
        );
        build_ipv4_packet(self.config.fa_address, destination, IP_PROTO_ICMP, &icmp)
    }

    fn registration_reply_packet(
        &self,
        ip: &ParsedIpv4Packet<'_>,
        destination_port: u16,
        rrq: &RegistrationRequest,
        code: u8,
        home_address: Ipv4Addr,
    ) -> Vec<u8> {
        let lifetime_secs = if code == RRP_CODE_ACCEPTED {
            rrq.lifetime_secs
                .min(self.config.registration_lifetime_secs)
        } else {
            MIP_LIFETIME_DEREGISTER
        };
        let mut rrp = Vec::new();
        rrp.push(MIP_RRP_TYPE);
        rrp.push(code);
        rrp.extend_from_slice(&lifetime_secs.to_be_bytes());
        rrp.extend_from_slice(&home_address.octets());
        rrp.extend_from_slice(&self.selected_home_agent(rrq.home_agent).octets());
        rrp.extend_from_slice(&rrq.identification.to_be_bytes());
        rrp.push(MIP_MOBILE_FOREIGN_CHALLENGE_EXT);
        rrp.push(self.challenge.len() as u8);
        rrp.extend_from_slice(&self.challenge);
        let destination = registration_reply_destination(ip, home_address);
        log::info!(
            "MIP4 TX: RRP code={}({}) dst={} udp={}->{} home={} ha={} lifetime={} id=0x{:016x} challenge_len={} auth_mode={:?}",
            code,
            registration_reply_code_label(code),
            destination,
            UDP_PORT_MOBILE_IP,
            destination_port,
            home_address,
            self.selected_home_agent(rrq.home_agent),
            lifetime_secs,
            rrq.identification,
            self.challenge.len(),
            self.config.auth_mode
        );
        let udp = build_udp_packet(UDP_PORT_MOBILE_IP, destination_port, &rrp);
        build_ipv4_packet(self.config.fa_address, destination, IP_PROTO_UDP, &udp)
    }

    fn rotate_challenge(&mut self) {
        self.challenge_counter = self.challenge_counter.wrapping_add(1);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_nanos() as u64)
            .unwrap_or(0);
        let mut challenge = Vec::with_capacity(MIP_CHALLENGE_LEN);
        challenge.extend_from_slice(&nanos.to_be_bytes());
        challenge.extend_from_slice(&self.challenge_counter.to_be_bytes());
        self.challenge = challenge;
    }
}

pub fn parse_registration_request(data: &[u8]) -> Option<RegistrationRequest> {
    if data.len() < MIP_RRQ_FIXED_LEN || data[0] != MIP_RRQ_TYPE {
        return None;
    }
    let flags = data[MIP_RRQ_FLAGS_OFFSET];
    let lifetime_secs = u16::from_be_bytes([
        data[MIP_RRQ_LIFETIME_OFFSET],
        data[MIP_RRQ_LIFETIME_OFFSET + 1],
    ]);
    let home_address = Ipv4Addr::new(
        data[MIP_RRQ_HOME_ADDRESS_OFFSET],
        data[MIP_RRQ_HOME_ADDRESS_OFFSET + 1],
        data[MIP_RRQ_HOME_ADDRESS_OFFSET + 2],
        data[MIP_RRQ_HOME_ADDRESS_OFFSET + 3],
    );
    let home_agent = Ipv4Addr::new(
        data[MIP_RRQ_HOME_AGENT_OFFSET],
        data[MIP_RRQ_HOME_AGENT_OFFSET + 1],
        data[MIP_RRQ_HOME_AGENT_OFFSET + 2],
        data[MIP_RRQ_HOME_AGENT_OFFSET + 3],
    );
    let care_of_address = Ipv4Addr::new(
        data[MIP_RRQ_CARE_OF_ADDRESS_OFFSET],
        data[MIP_RRQ_CARE_OF_ADDRESS_OFFSET + 1],
        data[MIP_RRQ_CARE_OF_ADDRESS_OFFSET + 2],
        data[MIP_RRQ_CARE_OF_ADDRESS_OFFSET + 3],
    );
    let identification = u64::from_be_bytes([
        data[MIP_RRQ_IDENTIFICATION_OFFSET],
        data[MIP_RRQ_IDENTIFICATION_OFFSET + 1],
        data[MIP_RRQ_IDENTIFICATION_OFFSET + 2],
        data[MIP_RRQ_IDENTIFICATION_OFFSET + 3],
        data[MIP_RRQ_IDENTIFICATION_OFFSET + 4],
        data[MIP_RRQ_IDENTIFICATION_OFFSET + 5],
        data[MIP_RRQ_IDENTIFICATION_OFFSET + 6],
        data[MIP_RRQ_IDENTIFICATION_OFFSET + 7],
    ]);
    Some(RegistrationRequest {
        flags,
        lifetime_secs,
        home_address,
        home_agent,
        care_of_address,
        identification,
        extensions: parse_extensions(&data[MIP_RRQ_EXTENSIONS_OFFSET..]),
    })
}

fn parse_extensions(mut data: &[u8]) -> Vec<MobileIpExtension> {
    let mut extensions = Vec::new();
    while !data.is_empty() {
        if data[0] == MIP_EXTENSION_PAD {
            data = &data[1..];
            continue;
        }
        if data[0] == MIP_GENERALIZED_AUTH_EXT {
            if data.len() < MIP_GENERALIZED_AUTH_HEADER_LEN {
                break;
            }
            let subtype = data[MIP_GENERALIZED_AUTH_SUBTYPE_OFFSET];
            let len = u16::from_be_bytes([
                data[MIP_GENERALIZED_AUTH_LENGTH_OFFSET],
                data[MIP_GENERALIZED_AUTH_LENGTH_OFFSET + 1],
            ]) as usize;
            if data.len() < MIP_GENERALIZED_AUTH_HEADER_LEN + len {
                break;
            }
            let value = data
                [MIP_GENERALIZED_AUTH_VALUE_OFFSET..MIP_GENERALIZED_AUTH_VALUE_OFFSET + len]
                .to_vec();
            let extension = match subtype {
                MIP_GENERALIZED_AUTH_SUBTYPE_MN_AAA => MobileIpExtension::MnAaaAuth(value),
                _ => MobileIpExtension::Unknown {
                    extension_type: MIP_GENERALIZED_AUTH_EXT,
                    subtype: Some(subtype),
                    data: value,
                },
            };
            extensions.push(extension);
            data = &data[MIP_GENERALIZED_AUTH_HEADER_LEN + len..];
            continue;
        }
        if data.len() < MIP_EXTENSION_HEADER_LEN {
            break;
        }
        let extension_type = data[0];
        let len = data[1] as usize;
        if data.len() < MIP_EXTENSION_HEADER_LEN + len {
            break;
        }
        let value = data[MIP_EXTENSION_HEADER_LEN..MIP_EXTENSION_HEADER_LEN + len].to_vec();
        let extension = match extension_type {
            MIP_MOBILE_FOREIGN_CHALLENGE_EXT => MobileIpExtension::MnFaChallenge(value),
            MIP_MOBILE_HOME_AUTH_EXT => MobileIpExtension::MobileHomeAuth(value),
            MIP_MOBILE_FOREIGN_AUTH_EXT => MobileIpExtension::MobileForeignAuth(value),
            MIP_MN_NAI_EXT => MobileIpExtension::MnNai(value),
            _ => MobileIpExtension::Unknown {
                extension_type,
                subtype: None,
                data: value,
            },
        };
        extensions.push(extension);
        data = &data[MIP_EXTENSION_HEADER_LEN + len..];
    }
    extensions
}

fn registration_reply_destination(ip: &ParsedIpv4Packet<'_>, home_address: Ipv4Addr) -> Ipv4Addr {
    if !home_address.is_unspecified() {
        home_address
    } else {
        ip.reply_address()
    }
}

fn format_unknown_extensions(extensions: &[MobileIpExtension]) -> String {
    let mut parts = Vec::new();
    for ext in extensions {
        if let MobileIpExtension::Unknown {
            extension_type,
            subtype,
            data,
        } = ext
        {
            let subtype = subtype
                .map(|value| format!(",subtype={value}"))
                .unwrap_or_default();
            parts.push(format!(
                "type={}{} len={} hex={}",
                extension_type,
                subtype,
                data.len(),
                format_hex(data)
            ));
        }
    }
    if parts.is_empty() {
        "none".to_string()
    } else {
        parts.join(";")
    }
}

fn format_auth_extensions(extensions: &[MobileIpExtension]) -> String {
    let mut parts = Vec::new();
    for ext in extensions {
        match ext {
            MobileIpExtension::MobileHomeAuth(data) => {
                parts.push(format_auth_extension("mn_ha", data));
            }
            MobileIpExtension::MobileForeignAuth(data) => {
                parts.push(format_auth_extension("mn_fa", data));
            }
            MobileIpExtension::MnAaaAuth(data) => {
                parts.push(format_auth_extension("mn_aaa", data));
            }
            _ => {}
        }
    }
    if parts.is_empty() {
        "none".to_string()
    } else {
        parts.join(";")
    }
}

fn format_auth_extension(name: &str, data: &[u8]) -> String {
    if data.len() < AUTH_EXTENSION_SPI_LEN {
        return format!("{}:malformed_len={}", name, data.len());
    }
    let spi = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
    let authenticator = &data[AUTH_EXTENSION_SPI_LEN..];
    format!(
        "{}:spi=0x{:0width$x},auth_len={},auth_prefix={}",
        name,
        spi,
        authenticator.len(),
        format_hex_prefix(authenticator, AUTH_EXTENSION_SPI_LEN),
        width = HEX_U32_WIDTH
    )
}

fn format_hex(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().saturating_mul(2));
    for byte in data {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{byte:02x}");
    }
    out
}

fn format_hex_prefix(data: &[u8], max_bytes: usize) -> String {
    if data.is_empty() {
        return "none".to_string();
    }
    let prefix_len = data.len().min(max_bytes);
    let mut out = String::with_capacity(prefix_len.saturating_mul(HEX_BYTE_CHARS));
    for byte in &data[..prefix_len] {
        use std::fmt::Write as _;
        let _ = write!(&mut out, "{byte:02x}");
    }
    if data.len() > prefix_len {
        out.push_str("...");
    }
    out
}

fn format_rrq_flags(flags: u8) -> String {
    let mut names = Vec::new();
    if flags & RRQ_FLAG_SIMULTANEOUS_BINDINGS != 0 {
        names.push("simultaneous");
    }
    if flags & RRQ_FLAG_BROADCAST_DATAGRAMS != 0 {
        names.push("broadcast");
    }
    if flags & RRQ_FLAG_D != 0 {
        names.push("co_located_coa");
    }
    if flags & RRQ_FLAG_MINIMAL_ENCAPSULATION != 0 {
        names.push("minimal_encap");
    }
    if flags & RRQ_FLAG_GRE_ENCAPSULATION != 0 {
        names.push("gre");
    }
    if flags & RRQ_FLAG_VJ_COMPRESSION != 0 {
        names.push("vj");
    }
    if flags & RRQ_FLAG_REVERSE_TUNNEL != 0 {
        names.push("reverse_tunnel");
    }
    if flags & RRQ_FLAG_RESERVED_0X01 != 0 {
        names.push("reserved_0x01");
    }
    if names.is_empty() {
        "none".to_string()
    } else {
        names.join("|")
    }
}

fn registration_reply_code_label(code: u8) -> &'static str {
    match code {
        RRP_CODE_ACCEPTED => "accepted",
        RRP_CODE_ACCEPTED_SIMULTANEOUS_UNSUPPORTED => "accepted_simultaneous_unsupported",
        RRP_CODE_POORLY_FORMED_REQUEST => "poorly_formed_request",
        RRP_CODE_ADMINISTRATIVELY_PROHIBITED => "administratively_prohibited",
        RRP_CODE_INSUFFICIENT_RESOURCES => "insufficient_resources",
        RRP_CODE_MOBILE_NODE_AUTH_FAILED => "mobile_node_auth_failed",
        RRP_CODE_HOME_AGENT_AUTH_FAILED => "home_agent_auth_failed",
        RRP_CODE_LIFETIME_TOO_LONG => "lifetime_too_long",
        RRP_CODE_POORLY_FORMED_REPLY => "poorly_formed_reply",
        RRP_CODE_POORLY_FORMED_REQUEST_REVERSE_TUNNEL => "poorly_formed_request_reverse_tunnel",
        RRP_CODE_FA_ENCAPSULATION_UNAVAILABLE => "foreign_agent_encapsulation_unavailable",
        RRP_CODE_VJ_COMPRESSION_UNAVAILABLE => "vj_compression_unavailable",
        RRP_CODE_REVERSE_TUNNEL_UNAVAILABLE => "reverse_tunnel_unavailable",
        RRP_CODE_REVERSE_TUNNEL_MANDATORY => "reverse_tunnel_mandatory",
        RRP_CODE_DELIVERY_STYLE_UNSUPPORTED => "delivery_style_unsupported",
        RRP_CODE_MISSING_NAI => "missing_nai",
        RRP_CODE_MISSING_HOME_AGENT => "missing_home_agent",
        RRP_CODE_MISSING_HOME_ADDRESS => "missing_home_address",
        RRP_CODE_UNKNOWN_CHALLENGE => "unknown_challenge",
        RRP_CODE_MISSING_CHALLENGE => "missing_challenge",
        RRP_CODE_STALE_CHALLENGE => "stale_challenge",
        RRP_CODE_UNKNOWN_HOME_AGENT => "unknown_home_agent",
        RRP_CODE_REQUESTED_HOME_AGENT_UNAVAILABLE => "requested_home_agent_unavailable",
        RRP_CODE_NONZERO_HOME_ADDRESS_REQUIRED => "nonzero_home_address_required",
        RRP_CODE_MISSING_HOME_AGENT_NAI => "missing_home_agent_nai",
        RRP_CODE_MISSING_HOME_ADDRESS_NAI => "missing_home_address_nai",
        RRP_CODE_MISSING_NAI_HOME_AGENT_HOME_ADDRESS => "missing_nai_home_agent_home_address",
        RRP_CODE_HA_REASON_UNSPECIFIED => "home_agent_reason_unspecified",
        RRP_CODE_HA_ADMIN_PROHIBITED => "home_agent_admin_prohibited",
        RRP_CODE_HA_INSUFFICIENT_RESOURCES => "home_agent_insufficient_resources",
        RRP_CODE_HA_MOBILE_NODE_AUTH_FAILED => "mobile_node_auth_failed_by_ha",
        RRP_CODE_HA_FOREIGN_AGENT_AUTH_FAILED => "foreign_agent_auth_failed_by_ha",
        RRP_CODE_HA_REGISTRATION_ID_MISMATCH => "registration_id_mismatch",
        RRP_CODE_HA_POORLY_FORMED_REQUEST => "poorly_formed_request_to_ha",
        RRP_CODE_HA_TOO_MANY_BINDINGS => "too_many_bindings",
        RRP_CODE_HA_UNKNOWN_HOME_AGENT_ADDRESS => "unknown_home_agent_address",
        RRP_CODE_HA_REVERSE_TUNNEL_UNAVAILABLE => "reverse_tunnel_unavailable_at_ha",
        RRP_CODE_HA_REVERSE_TUNNEL_MANDATORY => "reverse_tunnel_mandatory_at_ha",
        RRP_CODE_HA_ENCAPSULATION_UNAVAILABLE => "encapsulation_unavailable_at_ha",
        _ => "unknown",
    }
}

#[derive(Debug)]
struct ParsedIpv4Packet<'a> {
    source: Ipv4Addr,
    destination: Ipv4Addr,
    protocol: u8,
    payload: &'a [u8],
}

impl<'a> ParsedIpv4Packet<'a> {
    fn parse(packet: &'a [u8]) -> Option<Self> {
        if packet.len() < IPV4_MIN_HEADER_LEN {
            return None;
        }
        if packet[0] >> IPV4_VERSION_SHIFT != IPV4_VERSION {
            return None;
        }
        let ihl = ((packet[0] & IPV4_IHL_MASK) as usize) * IPV4_IHL_WORD_BYTES;
        if ihl < IPV4_MIN_HEADER_LEN || packet.len() < ihl {
            return None;
        }
        let total_len = u16::from_be_bytes([
            packet[IPV4_TOTAL_LEN_OFFSET],
            packet[IPV4_TOTAL_LEN_OFFSET + 1],
        ]) as usize;
        if total_len < ihl || packet.len() < total_len {
            return None;
        }
        Some(Self {
            source: Ipv4Addr::new(
                packet[IPV4_SOURCE_OFFSET],
                packet[IPV4_SOURCE_OFFSET + 1],
                packet[IPV4_SOURCE_OFFSET + 2],
                packet[IPV4_SOURCE_OFFSET + 3],
            ),
            destination: Ipv4Addr::new(
                packet[IPV4_DESTINATION_OFFSET],
                packet[IPV4_DESTINATION_OFFSET + 1],
                packet[IPV4_DESTINATION_OFFSET + 2],
                packet[IPV4_DESTINATION_OFFSET + 3],
            ),
            protocol: packet[IPV4_PROTOCOL_OFFSET],
            payload: &packet[ihl..total_len],
        })
    }

    fn reply_address(&self) -> Ipv4Addr {
        if self.source.is_unspecified() {
            Ipv4Addr::BROADCAST
        } else {
            self.source
        }
    }
}

#[derive(Debug)]
struct ParsedUdpPacket<'a> {
    source_port: u16,
    destination_port: u16,
    payload: &'a [u8],
}

impl<'a> ParsedUdpPacket<'a> {
    fn parse(packet: &'a [u8]) -> Option<Self> {
        if packet.len() < UDP_HEADER_LEN {
            return None;
        }
        let len = u16::from_be_bytes([packet[UDP_LEN_OFFSET], packet[UDP_LEN_OFFSET + 1]]) as usize;
        if len < UDP_HEADER_LEN || packet.len() < len {
            return None;
        }
        Some(Self {
            source_port: u16::from_be_bytes([
                packet[UDP_SOURCE_PORT_OFFSET],
                packet[UDP_SOURCE_PORT_OFFSET + 1],
            ]),
            destination_port: u16::from_be_bytes([
                packet[UDP_DESTINATION_PORT_OFFSET],
                packet[UDP_DESTINATION_PORT_OFFSET + 1],
            ]),
            payload: &packet[UDP_PAYLOAD_OFFSET..len],
        })
    }
}

pub fn build_ipv4_packet(
    source: Ipv4Addr,
    destination: Ipv4Addr,
    protocol: u8,
    payload: &[u8],
) -> Vec<u8> {
    let total_len = IPV4_MIN_HEADER_LEN + payload.len();
    let mut packet = Vec::with_capacity(total_len);
    packet.push(IPV4_HEADER_VERSION_IHL_NO_OPTIONS);
    packet.push(IPV4_TOS_DEFAULT);
    packet.extend_from_slice(&(total_len as u16).to_be_bytes());
    packet.extend_from_slice(&IPV4_IDENTIFICATION_DEFAULT.to_be_bytes());
    packet.extend_from_slice(&IPV4_FLAGS_FRAGMENT_OFFSET_DEFAULT.to_be_bytes());
    packet.push(IPV4_DEFAULT_TTL);
    packet.push(protocol);
    packet.extend_from_slice(&CHECKSUM_PLACEHOLDER.to_be_bytes());
    packet.extend_from_slice(&source.octets());
    packet.extend_from_slice(&destination.octets());
    fill_checksum(&mut packet, IPV4_CHECKSUM_OFFSET);
    packet.extend_from_slice(payload);
    packet
}

pub fn build_udp_packet(source_port: u16, destination_port: u16, payload: &[u8]) -> Vec<u8> {
    let len = UDP_HEADER_LEN + payload.len();
    let mut packet = Vec::with_capacity(len);
    packet.extend_from_slice(&source_port.to_be_bytes());
    packet.extend_from_slice(&destination_port.to_be_bytes());
    packet.extend_from_slice(&(len as u16).to_be_bytes());
    packet.extend_from_slice(&UDP_CHECKSUM_UNUSED.to_be_bytes());
    packet.extend_from_slice(payload);
    packet
}

fn fill_checksum(packet: &mut [u8], offset: usize) {
    packet[offset] = 0;
    packet[offset + 1] = 0;
    let sum = checksum(packet);
    packet[offset] = (sum >> 8) as u8;
    packet[offset + 1] = sum as u8;
}

fn checksum(data: &[u8]) -> u16 {
    let mut sum = 0u32;
    for chunk in data.chunks(CHECKSUM_WORD_BYTES) {
        let word = if chunk.len() == CHECKSUM_WORD_BYTES {
            u16::from_be_bytes([chunk[0], chunk[1]]) as u32
        } else {
            (chunk[0] as u32) << 8
        };
        sum = sum.wrapping_add(word);
        while sum > CHECKSUM_U16_MAX {
            sum = (sum & CHECKSUM_U16_MAX) + (sum >> 16);
        }
    }
    !(sum as u16)
}

#[cfg(test)]
mod tests {
    use super::*;

    const TEST_RRQ_LIFETIME_SECS: u16 = 1800;
    const TEST_RRQ_IDENTIFICATION: u64 = 0x1122_3344_5566_7788;
    const TEST_NAI: &[u8] = b"user@example.net";
    const TEST_MN_HA_AUTH: [u8; 20] = [0; 20];
    const TEST_MN_FA_CHALLENGE: [u8; 4] = [1, 2, 3, 4];
    const TEST_MN_AAA_AUTH: [u8; 20] = [0; 20];
    const TEST_UNKNOWN_EXT: [u8; 3] = [0xaa, 0xbb, 0xcc];
    const TEST_ALL_ROUTERS_MULTICAST: Ipv4Addr = Ipv4Addr::new(224, 0, 0, 1);
    const TEST_ASSIGNED_HOME: Ipv4Addr = Ipv4Addr::new(10, 55, 0, 9);

    fn append_extension(out: &mut Vec<u8>, extension_type: u8, value: &[u8]) {
        out.push(extension_type);
        out.push(value.len() as u8);
        out.extend_from_slice(value);
    }

    fn append_generalized_auth_extension(out: &mut Vec<u8>, subtype: u8, value: &[u8]) {
        out.push(MIP_GENERALIZED_AUTH_EXT);
        out.push(subtype);
        out.extend_from_slice(&(value.len() as u16).to_be_bytes());
        out.extend_from_slice(value);
    }

    fn rrq_payload() -> Vec<u8> {
        let mut rrq = rrq_payload_without_auth();
        append_extension(&mut rrq, MIP_MOBILE_HOME_AUTH_EXT, &TEST_MN_HA_AUTH);
        append_extension(
            &mut rrq,
            MIP_MOBILE_FOREIGN_CHALLENGE_EXT,
            &TEST_MN_FA_CHALLENGE,
        );
        append_generalized_auth_extension(
            &mut rrq,
            MIP_GENERALIZED_AUTH_SUBTYPE_MN_AAA,
            &TEST_MN_AAA_AUTH,
        );
        append_extension(&mut rrq, 250, &TEST_UNKNOWN_EXT);
        rrq
    }

    fn rrq_payload_without_auth() -> Vec<u8> {
        let mut rrq = Vec::new();
        rrq.push(MIP_RRQ_TYPE);
        rrq.push(RRQ_FLAG_NONE);
        rrq.extend_from_slice(&TEST_RRQ_LIFETIME_SECS.to_be_bytes());
        rrq.extend_from_slice(&Ipv4Addr::UNSPECIFIED.octets());
        rrq.extend_from_slice(&Ipv4Addr::BROADCAST.octets());
        rrq.extend_from_slice(&DEFAULT_AGENT_ADDRESS.octets());
        rrq.extend_from_slice(&TEST_RRQ_IDENTIFICATION.to_be_bytes());
        append_extension(&mut rrq, MIP_MN_NAI_EXT, TEST_NAI);
        rrq
    }

    #[test]
    fn parses_registration_request_extensions() {
        let rrq = parse_registration_request(&rrq_payload()).expect("rrq should parse");
        assert_eq!(rrq.home_address, Ipv4Addr::UNSPECIFIED);
        assert_eq!(rrq.home_agent, Ipv4Addr::BROADCAST);
        assert_eq!(rrq.nai().as_deref(), Some("user@example.net"));
        assert!(rrq.has_mn_ha_auth());
        assert!(rrq.has_mn_fa_challenge());
        assert!(rrq.has_mn_aaa_auth());
        assert_eq!(rrq.unknown_extension_count(), 1);
        assert_eq!(
            format_unknown_extensions(&rrq.extensions),
            "type=250 len=3 hex=aabbcc"
        );
    }

    #[test]
    fn agent_solicitation_gets_advertisement() {
        let mut session = MobileIpSession::new(MobileIpConfig {
            enabled: true,
            ..MobileIpConfig::default()
        });
        let solicitation = build_ipv4_packet(
            Ipv4Addr::UNSPECIFIED,
            TEST_ALL_ROUTERS_MULTICAST,
            IP_PROTO_ICMP,
            &[
                ICMP_ROUTER_SOLICITATION,
                ICMP_CODE_DEFAULT,
                CHECKSUM_PLACEHOLDER.to_be_bytes()[0],
                CHECKSUM_PLACEHOLDER.to_be_bytes()[1],
            ],
        );
        let result = session.handle_ipv4_packet(&solicitation, TEST_ASSIGNED_HOME);
        assert!(matches!(result, MobileIpPacketResult::Reply(_)));
    }

    #[test]
    fn rrq_registers_dynamic_home_address_in_insecure_mode() {
        let mut session = MobileIpSession::new(MobileIpConfig {
            enabled: true,
            ..MobileIpConfig::default()
        });
        let udp = build_udp_packet(
            UDP_PORT_MOBILE_IP,
            UDP_PORT_MOBILE_IP,
            &rrq_payload_without_auth(),
        );
        let ip = build_ipv4_packet(
            Ipv4Addr::UNSPECIFIED,
            DEFAULT_AGENT_ADDRESS,
            IP_PROTO_UDP,
            &udp,
        );
        let result = session.handle_ipv4_packet(&ip, TEST_ASSIGNED_HOME);
        match result {
            MobileIpPacketResult::Registered { binding, reply } => {
                assert_eq!(binding.home_address, TEST_ASSIGNED_HOME);
                assert!(!reply.is_empty());
                let ip = ParsedIpv4Packet::parse(&reply).expect("reply ipv4");
                assert_eq!(ip.source, DEFAULT_AGENT_ADDRESS);
                assert_eq!(ip.destination, TEST_ASSIGNED_HOME);
                assert_eq!(ip.protocol, IP_PROTO_UDP);
                assert_eq!(
                    checksum(&reply[..IPV4_MIN_HEADER_LEN]),
                    CHECKSUM_PLACEHOLDER
                );
                let total_len = u16::from_be_bytes([
                    reply[IPV4_TOTAL_LEN_OFFSET],
                    reply[IPV4_TOTAL_LEN_OFFSET + 1],
                ]) as usize;
                assert_eq!(total_len, reply.len());
                let udp = ParsedUdpPacket::parse(ip.payload).expect("reply udp");
                assert_eq!(udp.source_port, UDP_PORT_MOBILE_IP);
                assert_eq!(udp.destination_port, UDP_PORT_MOBILE_IP);
                let udp_len = u16::from_be_bytes([
                    ip.payload[UDP_LEN_OFFSET],
                    ip.payload[UDP_LEN_OFFSET + 1],
                ]) as usize;
                assert_eq!(udp_len, ip.payload.len());
                assert_eq!(
                    u16::from_be_bytes([
                        ip.payload[UDP_CHECKSUM_OFFSET],
                        ip.payload[UDP_CHECKSUM_OFFSET + 1],
                    ]),
                    UDP_CHECKSUM_UNUSED
                );
                assert_eq!(
                    udp.payload[MIP_RRP_FIXED_LEN],
                    MIP_MOBILE_FOREIGN_CHALLENGE_EXT
                );
            }
            other => panic!("unexpected result: {other:?}"),
        }
    }
}
