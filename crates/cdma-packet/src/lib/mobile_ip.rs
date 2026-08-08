//! Mobile IPv4 control packets used by cdma2000 packet data.

use std::net::Ipv4Addr;

use md5::{Digest, Md5};
use uuid::Uuid;

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
#[cfg(test)]
const IPV4_TTL_OFFSET: usize = 8;
#[cfg(test)]
const ICMP_MOBILITY_AGENT_FLAGS_OFFSET: usize = 22;
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
const MIP_AGENT_ADVERTISEMENT_FLAG_REGISTRATION_REQUIRED: u8 = 0x80;
const MIP_AGENT_ADVERTISEMENT_FLAG_FOREIGN_AGENT: u8 = 0x10;
const MIP_AGENT_ADVERTISEMENT_FLAG_REVERSE_TUNNEL: u8 = 0x01;
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
const MIP_NORMAL_VENDOR_ORG_SPECIFIC_EXT: u8 = 134;
const MIP_3GPP2_DNS_NVSE_LENGTH: u8 = 22;
const MIP_3GPP2_VENDOR_ORG_ID: u32 = 5535;
const MIP_3GPP2_DNS_NVSE_TYPE: u16 = 17;
const MIP_3GPP2_DNS_ENTITY_HOME_AGENT: u8 = 3;
const MIP_3GPP2_DNS_PRIMARY_SUBTYPE: u8 = 1;
const MIP_3GPP2_DNS_SECONDARY_SUBTYPE: u8 = 2;
const MIP_3GPP2_DNS_SUBTYPE_LENGTH: u8 = 6;
const MIP_3GPP2_DNS_UNUSED: u8 = 0;
const MIP_NVSE_RESERVED: u16 = 0;
const MIP_EXTENSION_PAD: u8 = 0;
const MIP_EXTENSION_HEADER_LEN: usize = 2;
const MIP_CHALLENGE_LEN: usize = 16;
// RFC 3012 defines two recently advertised values as the default challenge window.
const MIP_CHALLENGE_WINDOW: usize = 2;
const MIP_AUTHENTICATOR_LEN: usize = 16;
const MIP_AUTH_EXTENSION_VALUE_LEN: u8 = (AUTH_EXTENSION_SPI_LEN + MIP_AUTHENTICATOR_LEN) as u8;

const MIP_RRQ_TYPE: u8 = 1;
const MIP_RRP_TYPE: u8 = 3;
#[cfg(test)]
const MIP_RRP_TYPE_OFFSET: usize = 0;
#[cfg(test)]
const MIP_RRP_CODE_OFFSET: usize = 1;
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
const RRP_CODE_UNKNOWN_MN_FA_CHALLENGE: u8 = 104;
const RRP_CODE_MISSING_MN_FA_CHALLENGE: u8 = 105;
const RRP_CODE_STALE_MN_FA_CHALLENGE: u8 = 106;
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
    MnHa,
}

#[derive(Clone, PartialEq, Eq)]
pub struct MobileIpSecurityAssociation {
    pub spi: u32,
    shared_secret: Vec<u8>,
}

impl std::fmt::Debug for MobileIpSecurityAssociation {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("MobileIpSecurityAssociation")
            .field("spi", &self.spi)
            .field("shared_secret", &"<redacted>")
            .finish()
    }
}

impl MobileIpSecurityAssociation {
    pub fn new(spi: u32, shared_secret: Vec<u8>) -> Self {
        Self { spi, shared_secret }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileIpConfig {
    pub enabled: bool,
    pub fa_address: Ipv4Addr,
    pub home_agent_address: Ipv4Addr,
    pub advertisement_count: u8,
    pub advertisement_lifetime_secs: u16,
    pub registration_lifetime_secs: u16,
    pub primary_dns: Ipv4Addr,
    pub secondary_dns: Ipv4Addr,
    pub auth_mode: MobileIpAuthMode,
    pub mn_ha_security: Option<Box<MobileIpSecurityAssociation>>,
    pub allow_unverified_mn_aaa: bool,
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
            primary_dns: DEFAULT_AGENT_ADDRESS,
            secondary_dns: DEFAULT_AGENT_ADDRESS,
            auth_mode: MobileIpAuthMode::Insecure,
            mn_ha_security: None,
            allow_unverified_mn_aaa: false,
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
    challenge: Vec<u8>,
    issued_challenges: Vec<IssuedChallenge>,
    binding: Option<MobileIpBinding>,
    last_registration: Option<Box<CompletedRegistration>>,
}

#[derive(Debug, Clone)]
struct IssuedChallenge {
    value: Vec<u8>,
    used: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ChallengeStatus {
    Unused,
    Used,
    Unknown,
}

#[derive(Debug, Clone)]
struct CompletedRegistration {
    request: Vec<u8>,
    binding: MobileIpBinding,
    reply: Vec<u8>,
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
    wire_data: Vec<u8>,
}

impl RegistrationRequest {
    pub fn nai(&self) -> Option<String> {
        self.mn_nai()
            .map(|bytes| String::from_utf8_lossy(bytes).into_owned())
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

    fn mn_fa_challenge(&self) -> Option<&[u8]> {
        self.extensions.iter().find_map(|ext| match ext {
            MobileIpExtension::MnFaChallenge(value) => Some(value.as_slice()),
            _ => None,
        })
    }

    fn mn_ha_auth(&self) -> Option<&MobileIpAuthentication> {
        self.extensions.iter().find_map(|ext| match ext {
            MobileIpExtension::MobileHomeAuth(authentication) => Some(authentication),
            _ => None,
        })
    }

    fn mn_aaa_auth(&self) -> Option<&MobileIpAuthentication> {
        self.extensions.iter().find_map(|ext| match ext {
            MobileIpExtension::MnAaaAuth(authentication) => Some(authentication),
            _ => None,
        })
    }

    fn mn_nai(&self) -> Option<&[u8]> {
        self.extensions.iter().find_map(|ext| match ext {
            MobileIpExtension::MnNai(bytes) => Some(bytes.as_slice()),
            _ => None,
        })
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
    MobileHomeAuth(MobileIpAuthentication),
    MobileForeignAuth(MobileIpAuthentication),
    MnAaaAuth(MobileIpAuthentication),
    MnNai(Vec<u8>),
    Unknown {
        extension_type: u8,
        subtype: Option<u8>,
        data: Vec<u8>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MobileIpAuthentication {
    pub spi: u32,
    pub authenticator: Vec<u8>,
    protected_data_len: usize,
}

impl MobileIpSession {
    pub fn new(config: MobileIpConfig) -> Self {
        let mut session = Self {
            config,
            advertisements_sent: 0,
            sequence: 0,
            challenge: Vec::new(),
            issued_challenges: Vec::with_capacity(MIP_CHALLENGE_WINDOW),
            binding: None,
            last_registration: None,
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

        if let Some(completed) = self
            .last_registration
            .as_deref()
            .filter(|completed| completed.request == rrq.wire_data)
        {
            log::info!(
                "MIP4 RX: retransmitted accepted RRQ id=0x{:016x}; resending cached RRP",
                rrq.identification
            );
            return MobileIpPacketResult::Registered {
                binding: completed.binding.clone(),
                reply: completed.reply.clone(),
            };
        }

        let mut challenge_rotated = false;
        match self.authenticate_registration_request(&rrq) {
            RegistrationAuthentication::Accepted => {
                if let Some(challenge) = rrq.mn_fa_challenge() {
                    let challenge = challenge.to_vec();
                    self.mark_challenge_used(&challenge);
                    self.rotate_challenge();
                    challenge_rotated = true;
                }
            }
            RegistrationAuthentication::InfrastructureRequired(reason) => {
                log::warn!(
                    "MIP4: RRQ requires unavailable authentication infrastructure ({reason}); falling back to Simple IP"
                );
                return MobileIpPacketResult::AuthenticationRequired;
            }
            RegistrationAuthentication::Rejected { code, reason } => {
                log::warn!(
                    "MIP4: RRQ authentication rejected code={}({}): {}",
                    code,
                    registration_reply_code_label(code),
                    reason
                );
                let reply = self.registration_reply_packet(
                    &ip,
                    udp.source_port,
                    &rrq,
                    code,
                    rrq.home_address,
                );
                return MobileIpPacketResult::Reply(reply);
            }
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
            self.last_registration = None;
            if !challenge_rotated {
                self.rotate_challenge();
            }
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
        if !challenge_rotated {
            self.rotate_challenge();
        }
        let reply = self.registration_reply_packet(
            &ip,
            udp.source_port,
            &rrq,
            RRP_CODE_ACCEPTED,
            home_address,
        );
        self.last_registration = Some(Box::new(CompletedRegistration {
            request: rrq.wire_data,
            binding: binding.clone(),
            reply: reply.clone(),
        }));
        MobileIpPacketResult::Registered { binding, reply }
    }

    fn selected_home_agent(&self, requested: Ipv4Addr) -> Ipv4Addr {
        if requested.is_unspecified() || requested == Ipv4Addr::BROADCAST {
            self.config.home_agent_address
        } else {
            requested
        }
    }

    fn authenticate_registration_request(
        &self,
        rrq: &RegistrationRequest,
    ) -> RegistrationAuthentication {
        if self.config.auth_mode == MobileIpAuthMode::Insecure {
            if rrq.has_mn_ha_auth() || rrq.has_mn_aaa_auth() || rrq.has_mn_fa_auth() {
                return RegistrationAuthentication::InfrastructureRequired(
                    "auth_mode=insecure cannot verify authenticated registrations",
                );
            }
            return RegistrationAuthentication::Accepted;
        }

        let Some(security) = self.config.mn_ha_security.as_deref() else {
            return RegistrationAuthentication::InfrastructureRequired(
                "MN-HA security association is not configured",
            );
        };
        let Some(authentication) = rrq.mn_ha_auth() else {
            return RegistrationAuthentication::Rejected {
                code: RRP_CODE_HA_MOBILE_NODE_AUTH_FAILED,
                reason: "missing MN-HA Authentication Extension",
            };
        };
        if rrq
            .extensions
            .iter()
            .filter(|extension| matches!(extension, MobileIpExtension::MobileHomeAuth(_)))
            .count()
            != 1
        {
            return RegistrationAuthentication::Rejected {
                code: RRP_CODE_HA_MOBILE_NODE_AUTH_FAILED,
                reason: "multiple MN-HA Authentication Extensions",
            };
        }
        if authentication.spi != security.spi {
            return RegistrationAuthentication::Rejected {
                code: RRP_CODE_HA_MOBILE_NODE_AUTH_FAILED,
                reason: "MN-HA SPI does not select the configured security association",
            };
        }
        if authentication.authenticator.len() != MIP_AUTHENTICATOR_LEN {
            return RegistrationAuthentication::Rejected {
                code: RRP_CODE_HA_MOBILE_NODE_AUTH_FAILED,
                reason: "MN-HA authenticator length is not 16 bytes",
            };
        }
        let expected = keyed_md5_prefix_suffix(
            &security.shared_secret,
            &rrq.wire_data[..authentication.protected_data_len],
        );
        if !constant_time_eq(&expected, &authentication.authenticator) {
            return RegistrationAuthentication::Rejected {
                code: RRP_CODE_HA_MOBILE_NODE_AUTH_FAILED,
                reason: "MN-HA authenticator does not match",
            };
        }

        let challenge_count = rrq
            .extensions
            .iter()
            .filter(|extension| matches!(extension, MobileIpExtension::MnFaChallenge(_)))
            .count();
        let Some(challenge) = rrq.mn_fa_challenge() else {
            return RegistrationAuthentication::Rejected {
                code: RRP_CODE_MISSING_MN_FA_CHALLENGE,
                reason: "missing MN-FA Challenge Extension",
            };
        };
        if challenge_count != 1 {
            return RegistrationAuthentication::Rejected {
                code: RRP_CODE_MOBILE_NODE_AUTH_FAILED,
                reason: "multiple MN-FA Challenge Extensions",
            };
        }
        match self.challenge_status(challenge) {
            ChallengeStatus::Unused => {}
            ChallengeStatus::Used => {
                return RegistrationAuthentication::Rejected {
                    code: RRP_CODE_STALE_MN_FA_CHALLENGE,
                    reason: "MN-FA challenge was already used",
                };
            }
            ChallengeStatus::Unknown => {
                return RegistrationAuthentication::Rejected {
                    code: RRP_CODE_UNKNOWN_MN_FA_CHALLENGE,
                    reason: "MN-FA challenge was not issued by this agent",
                };
            }
        }
        if !rrq.has_mn_aaa_auth() && !rrq.has_mn_fa_auth() {
            return RegistrationAuthentication::Rejected {
                code: RRP_CODE_MOBILE_NODE_AUTH_FAILED,
                reason: "MN-FA challenge is not followed by an authentication extension",
            };
        }
        let mn_ha_position = rrq
            .extensions
            .iter()
            .position(|extension| matches!(extension, MobileIpExtension::MobileHomeAuth(_)))
            .expect("MN-HA authentication was checked above");
        let challenge_position = rrq
            .extensions
            .iter()
            .position(|extension| matches!(extension, MobileIpExtension::MnFaChallenge(_)))
            .expect("MN-FA challenge was checked above");
        let following_auth_position = rrq.extensions.iter().position(|extension| {
            matches!(
                extension,
                MobileIpExtension::MnAaaAuth(_) | MobileIpExtension::MobileForeignAuth(_)
            )
        });
        if mn_ha_position >= challenge_position
            || following_auth_position.is_none_or(|position| position <= challenge_position)
        {
            return RegistrationAuthentication::Rejected {
                code: RRP_CODE_MOBILE_NODE_AUTH_FAILED,
                reason: "registration authentication extensions are out of order",
            };
        }
        let mn_aaa_count = rrq
            .extensions
            .iter()
            .filter(|extension| matches!(extension, MobileIpExtension::MnAaaAuth(_)))
            .count();
        let mn_fa_auth_count = rrq
            .extensions
            .iter()
            .filter(|extension| matches!(extension, MobileIpExtension::MobileForeignAuth(_)))
            .count();
        if mn_aaa_count + mn_fa_auth_count != 1 {
            return RegistrationAuthentication::Rejected {
                code: RRP_CODE_MOBILE_NODE_AUTH_FAILED,
                reason: "MN-FA challenge must have exactly one following authenticator",
            };
        }
        if rrq.mn_aaa_auth().is_some_and(|authentication| {
            authentication.authenticator.len() != MIP_AUTHENTICATOR_LEN
        }) {
            return RegistrationAuthentication::Rejected {
                code: RRP_CODE_MOBILE_NODE_AUTH_FAILED,
                reason: "MN-AAA authenticator length is not 16 bytes",
            };
        }
        if rrq.has_mn_fa_auth() {
            return RegistrationAuthentication::InfrastructureRequired(
                "MN-FA security association is not configured",
            );
        }
        if rrq.has_mn_aaa_auth() && !self.config.allow_unverified_mn_aaa {
            return RegistrationAuthentication::InfrastructureRequired(
                "MN-AAA verification is required by configuration",
            );
        }
        if rrq.has_mn_aaa_auth() {
            log::warn!(
                "MIP4: accepting MN-AAA authentication without verification because allow_unverified_mn_aaa=true"
            );
        }
        RegistrationAuthentication::Accepted
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
        icmp.push(
            MIP_AGENT_ADVERTISEMENT_FLAG_REGISTRATION_REQUIRED
                | MIP_AGENT_ADVERTISEMENT_FLAG_FOREIGN_AGENT
                | MIP_AGENT_ADVERTISEMENT_FLAG_REVERSE_TUNNEL,
        );
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
        build_ipv4_packet_with_ttl(self.config.fa_address, destination, IP_PROTO_ICMP, &icmp, 1)
    }

    fn registration_reply_packet(
        &self,
        ip: &ParsedIpv4Packet<'_>,
        destination_port: u16,
        rrq: &RegistrationRequest,
        code: u8,
        home_address: Ipv4Addr,
    ) -> Vec<u8> {
        let registration_accepted = matches!(
            code,
            RRP_CODE_ACCEPTED | RRP_CODE_ACCEPTED_SIMULTANEOUS_UNSUPPORTED
        );
        let lifetime_secs = if registration_accepted {
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
        // RFC 2794 requires the HA to return the MN-NAI when it was present in the RRQ.
        if let Some(nai) = rrq.mn_nai() {
            rrp.push(MIP_MN_NAI_EXT);
            rrp.push(nai.len() as u8);
            rrp.extend_from_slice(nai);
        }
        if registration_accepted && lifetime_secs != MIP_LIFETIME_DEREGISTER {
            append_3gpp2_dns_nvse(&mut rrp, self.config.primary_dns, self.config.secondary_dns);
        }
        if let Some(security) = self.config.mn_ha_security.as_deref() {
            rrp.push(MIP_MOBILE_HOME_AUTH_EXT);
            rrp.push(MIP_AUTH_EXTENSION_VALUE_LEN);
            let authenticator = keyed_md5_prefix_suffix(&security.shared_secret, &rrp);
            rrp.extend_from_slice(&security.spi.to_be_bytes());
            rrp.extend_from_slice(&authenticator);
        }
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
        let udp = build_udp_packet_with_ipv4_checksum(
            self.config.fa_address,
            destination,
            UDP_PORT_MOBILE_IP,
            destination_port,
            &rrp,
        );
        build_ipv4_packet(self.config.fa_address, destination, IP_PROTO_UDP, &udp)
    }

    fn rotate_challenge(&mut self) {
        let challenge = loop {
            let candidate = Uuid::new_v4().as_bytes().to_vec();
            if !self
                .issued_challenges
                .iter()
                .any(|issued| issued.value == candidate)
            {
                break candidate;
            }
        };
        self.issue_challenge(challenge);
    }

    fn issue_challenge(&mut self, challenge: Vec<u8>) {
        debug_assert_eq!(challenge.len(), MIP_CHALLENGE_LEN);
        if self.issued_challenges.len() == MIP_CHALLENGE_WINDOW {
            self.issued_challenges.remove(0);
        }
        self.challenge = challenge.clone();
        self.issued_challenges.push(IssuedChallenge {
            value: challenge,
            used: false,
        });
    }

    fn challenge_status(&self, challenge: &[u8]) -> ChallengeStatus {
        self.issued_challenges
            .iter()
            .rev()
            .find(|issued| issued.value == challenge)
            .map_or(ChallengeStatus::Unknown, |issued| {
                if issued.used {
                    ChallengeStatus::Used
                } else {
                    ChallengeStatus::Unused
                }
            })
    }

    fn mark_challenge_used(&mut self, challenge: &[u8]) {
        if let Some(issued) = self
            .issued_challenges
            .iter_mut()
            .rev()
            .find(|issued| issued.value == challenge)
        {
            issued.used = true;
        }
    }
}

fn append_3gpp2_dns_nvse(message: &mut Vec<u8>, primary_dns: Ipv4Addr, secondary_dns: Ipv4Addr) {
    message.push(MIP_NORMAL_VENDOR_ORG_SPECIFIC_EXT);
    message.push(MIP_3GPP2_DNS_NVSE_LENGTH);
    message.extend_from_slice(&MIP_NVSE_RESERVED.to_be_bytes());
    message.extend_from_slice(&MIP_3GPP2_VENDOR_ORG_ID.to_be_bytes());
    message.extend_from_slice(&MIP_3GPP2_DNS_NVSE_TYPE.to_be_bytes());
    // X.S0011-002 identifies locally configured DNS as Home Agent entity 3.
    message.push(MIP_3GPP2_DNS_ENTITY_HOME_AGENT);
    message.push(MIP_3GPP2_DNS_PRIMARY_SUBTYPE);
    message.push(MIP_3GPP2_DNS_SUBTYPE_LENGTH);
    message.extend_from_slice(&primary_dns.octets());
    message.push(MIP_3GPP2_DNS_SECONDARY_SUBTYPE);
    message.push(MIP_3GPP2_DNS_SUBTYPE_LENGTH);
    message.extend_from_slice(&secondary_dns.octets());
    message.push(MIP_3GPP2_DNS_UNUSED);
}

enum RegistrationAuthentication {
    Accepted,
    InfrastructureRequired(&'static str),
    Rejected { code: u8, reason: &'static str },
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
    let extensions = parse_extensions(data, MIP_RRQ_EXTENSIONS_OFFSET)?;
    Some(RegistrationRequest {
        flags,
        lifetime_secs,
        home_address,
        home_agent,
        care_of_address,
        identification,
        extensions,
        wire_data: data.to_vec(),
    })
}

fn parse_extensions(data: &[u8], mut offset: usize) -> Option<Vec<MobileIpExtension>> {
    let mut extensions = Vec::new();
    while offset < data.len() {
        if data[offset] == MIP_EXTENSION_PAD {
            offset += 1;
            continue;
        }
        if data[offset] == MIP_GENERALIZED_AUTH_EXT {
            if data.len() - offset < MIP_GENERALIZED_AUTH_HEADER_LEN {
                return None;
            }
            let subtype = data[offset + MIP_GENERALIZED_AUTH_SUBTYPE_OFFSET];
            let len = u16::from_be_bytes([
                data[offset + MIP_GENERALIZED_AUTH_LENGTH_OFFSET],
                data[offset + MIP_GENERALIZED_AUTH_LENGTH_OFFSET + 1],
            ]) as usize;
            if data.len() - offset < MIP_GENERALIZED_AUTH_HEADER_LEN + len {
                return None;
            }
            let value_start = offset + MIP_GENERALIZED_AUTH_VALUE_OFFSET;
            let value = &data[value_start..value_start + len];
            let extension = match (
                subtype,
                parse_authentication(value, value_start + AUTH_EXTENSION_SPI_LEN),
            ) {
                (MIP_GENERALIZED_AUTH_SUBTYPE_MN_AAA, Some(authentication)) => {
                    MobileIpExtension::MnAaaAuth(authentication)
                }
                _ => MobileIpExtension::Unknown {
                    extension_type: MIP_GENERALIZED_AUTH_EXT,
                    subtype: Some(subtype),
                    data: value.to_vec(),
                },
            };
            extensions.push(extension);
            offset += MIP_GENERALIZED_AUTH_HEADER_LEN + len;
            continue;
        }
        if data.len() - offset < MIP_EXTENSION_HEADER_LEN {
            return None;
        }
        let extension_type = data[offset];
        let len = data[offset + 1] as usize;
        if data.len() - offset < MIP_EXTENSION_HEADER_LEN + len {
            return None;
        }
        let value_start = offset + MIP_EXTENSION_HEADER_LEN;
        let value = &data[value_start..value_start + len];
        let extension = match extension_type {
            MIP_MOBILE_FOREIGN_CHALLENGE_EXT => MobileIpExtension::MnFaChallenge(value.to_vec()),
            MIP_MOBILE_HOME_AUTH_EXT => match parse_authentication(value, value_start) {
                Some(authentication) => MobileIpExtension::MobileHomeAuth(authentication),
                None => MobileIpExtension::Unknown {
                    extension_type,
                    subtype: None,
                    data: value.to_vec(),
                },
            },
            MIP_MOBILE_FOREIGN_AUTH_EXT => match parse_authentication(value, value_start) {
                Some(authentication) => MobileIpExtension::MobileForeignAuth(authentication),
                None => MobileIpExtension::Unknown {
                    extension_type,
                    subtype: None,
                    data: value.to_vec(),
                },
            },
            MIP_MN_NAI_EXT => MobileIpExtension::MnNai(value.to_vec()),
            _ => MobileIpExtension::Unknown {
                extension_type,
                subtype: None,
                data: value.to_vec(),
            },
        };
        extensions.push(extension);
        offset += MIP_EXTENSION_HEADER_LEN + len;
    }
    Some(extensions)
}

fn parse_authentication(value: &[u8], protected_data_len: usize) -> Option<MobileIpAuthentication> {
    if value.len() < AUTH_EXTENSION_SPI_LEN {
        return None;
    }
    Some(MobileIpAuthentication {
        spi: u32::from_be_bytes([value[0], value[1], value[2], value[3]]),
        authenticator: value[AUTH_EXTENSION_SPI_LEN..].to_vec(),
        protected_data_len,
    })
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
            MobileIpExtension::MobileHomeAuth(authentication) => {
                parts.push(format_auth_extension("mn_ha", authentication));
            }
            MobileIpExtension::MobileForeignAuth(authentication) => {
                parts.push(format_auth_extension("mn_fa", authentication));
            }
            MobileIpExtension::MnAaaAuth(authentication) => {
                parts.push(format_auth_extension("mn_aaa", authentication));
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

fn format_auth_extension(name: &str, authentication: &MobileIpAuthentication) -> String {
    format!(
        "{}:spi=0x{:0width$x},auth_len={},auth_prefix={}",
        name,
        authentication.spi,
        authentication.authenticator.len(),
        format_hex_prefix(&authentication.authenticator, AUTH_EXTENSION_SPI_LEN),
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
        RRP_CODE_UNKNOWN_MN_FA_CHALLENGE => "unknown_mn_fa_challenge",
        RRP_CODE_MISSING_MN_FA_CHALLENGE => "missing_mn_fa_challenge",
        RRP_CODE_STALE_MN_FA_CHALLENGE => "stale_mn_fa_challenge",
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
    build_ipv4_packet_with_ttl(source, destination, protocol, payload, IPV4_DEFAULT_TTL)
}

fn build_ipv4_packet_with_ttl(
    source: Ipv4Addr,
    destination: Ipv4Addr,
    protocol: u8,
    payload: &[u8],
    ttl: u8,
) -> Vec<u8> {
    let total_len = IPV4_MIN_HEADER_LEN + payload.len();
    let mut packet = Vec::with_capacity(total_len);
    packet.push(IPV4_HEADER_VERSION_IHL_NO_OPTIONS);
    packet.push(IPV4_TOS_DEFAULT);
    packet.extend_from_slice(&(total_len as u16).to_be_bytes());
    packet.extend_from_slice(&IPV4_IDENTIFICATION_DEFAULT.to_be_bytes());
    packet.extend_from_slice(&IPV4_FLAGS_FRAGMENT_OFFSET_DEFAULT.to_be_bytes());
    packet.push(ttl);
    packet.push(protocol);
    packet.extend_from_slice(&CHECKSUM_PLACEHOLDER.to_be_bytes());
    packet.extend_from_slice(&source.octets());
    packet.extend_from_slice(&destination.octets());
    fill_checksum(&mut packet, IPV4_CHECKSUM_OFFSET);
    packet.extend_from_slice(payload);
    packet
}

fn keyed_md5_prefix_suffix(secret: &[u8], protected_data: &[u8]) -> [u8; MIP_AUTHENTICATOR_LEN] {
    let mut md5 = Md5::new();
    md5.update(secret);
    md5.update(protected_data);
    md5.update(secret);
    md5.finalize().into()
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    if left.len() != right.len() {
        return false;
    }
    let mut difference = 0u8;
    for (left_byte, right_byte) in left.iter().zip(right) {
        difference |= left_byte ^ right_byte;
    }
    difference == 0
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

fn build_udp_packet_with_ipv4_checksum(
    source: Ipv4Addr,
    destination: Ipv4Addr,
    source_port: u16,
    destination_port: u16,
    payload: &[u8],
) -> Vec<u8> {
    let mut packet = build_udp_packet(source_port, destination_port, payload);
    let mut protected = Vec::with_capacity(12 + packet.len());
    protected.extend_from_slice(&source.octets());
    protected.extend_from_slice(&destination.octets());
    protected.push(0);
    protected.push(IP_PROTO_UDP);
    protected.extend_from_slice(&(packet.len() as u16).to_be_bytes());
    protected.extend_from_slice(&packet);
    let checksum = match checksum(&protected) {
        UDP_CHECKSUM_UNUSED => u16::MAX,
        checksum => checksum,
    };
    packet[UDP_CHECKSUM_OFFSET..UDP_CHECKSUM_OFFSET + CHECKSUM_WORD_BYTES]
        .copy_from_slice(&checksum.to_be_bytes());
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
    const TEST_SPRINT_NAI: &[u8] = b"A0000027E3DF90@hcm.sprintpcs.com";
    const TEST_SPRINT_IDENTIFICATION: u64 = 0xee20_a86a_015e_0cc0;
    const TEST_SPRINT_CHALLENGE: [u8; MIP_CHALLENGE_LEN] = [
        0x18, 0xc9, 0x9a, 0xbf, 0x67, 0xb8, 0x58, 0xf0, 0, 0, 0, 0, 0, 0, 0, 1,
    ];
    const TEST_MN_HA_AUTH: [u8; 20] = [0; 20];
    const TEST_MN_FA_CHALLENGE: [u8; 4] = [1, 2, 3, 4];
    const TEST_MN_AAA_AUTH: [u8; 20] = [0; 20];
    const TEST_UNKNOWN_EXT: [u8; 3] = [0xaa, 0xbb, 0xcc];
    const TEST_3GPP2_DNS_NVSE: [u8; 24] = [
        0x86, 0x16, 0x00, 0x00, 0x00, 0x00, 0x15, 0x9f, 0x00, 0x11, 0x03, 0x01, 0x06, 0x0a, 0x37,
        0x00, 0x01, 0x02, 0x06, 0x0a, 0x37, 0x00, 0x01, 0x00,
    ];
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

    fn sprint_authenticated_rrq_payload() -> Vec<u8> {
        sprint_authenticated_rrq_payload_with(TEST_SPRINT_IDENTIFICATION, &TEST_SPRINT_CHALLENGE)
    }

    fn sprint_authenticated_rrq_payload_with(identification: u64, challenge: &[u8]) -> Vec<u8> {
        let mut rrq = Vec::new();
        rrq.push(MIP_RRQ_TYPE);
        rrq.push(RRQ_FLAG_REVERSE_TUNNEL);
        rrq.extend_from_slice(&1200u16.to_be_bytes());
        rrq.extend_from_slice(&Ipv4Addr::UNSPECIFIED.octets());
        rrq.extend_from_slice(&Ipv4Addr::new(68, 28, 15, 12).octets());
        rrq.extend_from_slice(&DEFAULT_AGENT_ADDRESS.octets());
        rrq.extend_from_slice(&identification.to_be_bytes());
        append_extension(&mut rrq, MIP_MN_NAI_EXT, TEST_SPRINT_NAI);

        rrq.push(MIP_MOBILE_HOME_AUTH_EXT);
        rrq.push(MIP_AUTH_EXTENSION_VALUE_LEN);
        let mn_ha_authenticator = keyed_md5_prefix_suffix(b"secret", &rrq);
        rrq.extend_from_slice(&1234u32.to_be_bytes());
        rrq.extend_from_slice(&mn_ha_authenticator);
        append_extension(&mut rrq, MIP_MOBILE_FOREIGN_CHALLENGE_EXT, challenge);
        let mut mn_aaa = 1234u32.to_be_bytes().to_vec();
        mn_aaa.extend_from_slice(&[
            0x1f, 0x40, 0xbc, 0xc3, 0x43, 0x0b, 0x29, 0x21, 0xb7, 0xd2, 0xe0, 0x6a, 0x38, 0xc6,
            0xd3, 0xd3,
        ]);
        append_generalized_auth_extension(&mut rrq, MIP_GENERALIZED_AUTH_SUBTYPE_MN_AAA, &mn_aaa);
        rrq
    }

    fn sprint_authenticated_config(allow_unverified_mn_aaa: bool) -> MobileIpConfig {
        MobileIpConfig {
            enabled: true,
            auth_mode: MobileIpAuthMode::MnHa,
            mn_ha_security: Some(Box::new(MobileIpSecurityAssociation::new(
                1234,
                b"secret".to_vec(),
            ))),
            allow_unverified_mn_aaa,
            ..MobileIpConfig::default()
        }
    }

    fn registration_request_packet(payload: &[u8]) -> Vec<u8> {
        let udp = build_udp_packet(UDP_PORT_MOBILE_IP, UDP_PORT_MOBILE_IP, payload);
        build_ipv4_packet(
            Ipv4Addr::UNSPECIFIED,
            DEFAULT_AGENT_ADDRESS,
            IP_PROTO_UDP,
            &udp,
        )
    }

    fn registration_reply_payload(packet: &[u8]) -> &[u8] {
        let ip = ParsedIpv4Packet::parse(packet).expect("reply IPv4 packet");
        ParsedUdpPacket::parse(ip.payload)
            .expect("reply UDP packet")
            .payload
    }

    fn assert_valid_ipv4_udp_checksum(ip: &ParsedIpv4Packet<'_>) {
        let mut protected = Vec::with_capacity(12 + ip.payload.len());
        protected.extend_from_slice(&ip.source.octets());
        protected.extend_from_slice(&ip.destination.octets());
        protected.push(0);
        protected.push(IP_PROTO_UDP);
        protected.extend_from_slice(&(ip.payload.len() as u16).to_be_bytes());
        protected.extend_from_slice(ip.payload);
        assert_eq!(checksum(&protected), 0);
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
        let MobileIpPacketResult::Reply(advertisement) = result else {
            panic!("unexpected result: {result:?}");
        };
        assert_eq!(advertisement[IPV4_TTL_OFFSET], 1);
        let ip = ParsedIpv4Packet::parse(&advertisement).expect("advertisement IPv4 packet");
        let mobility_extension_flags = ip.payload[ICMP_MOBILITY_AGENT_FLAGS_OFFSET];
        assert_ne!(
            mobility_extension_flags & MIP_AGENT_ADVERTISEMENT_FLAG_REGISTRATION_REQUIRED,
            0
        );
        assert_ne!(
            mobility_extension_flags & MIP_AGENT_ADVERTISEMENT_FLAG_REVERSE_TUNNEL,
            0
        );
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
                assert_valid_ipv4_udp_checksum(&ip);
                let udp_len = u16::from_be_bytes([
                    ip.payload[UDP_LEN_OFFSET],
                    ip.payload[UDP_LEN_OFFSET + 1],
                ]) as usize;
                assert_eq!(udp_len, ip.payload.len());
                assert_ne!(
                    u16::from_be_bytes([
                        ip.payload[UDP_CHECKSUM_OFFSET],
                        ip.payload[UDP_CHECKSUM_OFFSET + 1],
                    ]),
                    UDP_CHECKSUM_UNUSED,
                    "Mobile IP replies must carry a UDP checksum"
                );
                assert_eq!(udp.payload[MIP_RRP_FIXED_LEN], MIP_MN_NAI_EXT);
                assert_eq!(
                    &udp.payload[MIP_RRP_FIXED_LEN + MIP_EXTENSION_HEADER_LEN
                        ..MIP_RRP_FIXED_LEN + MIP_EXTENSION_HEADER_LEN + TEST_NAI.len()],
                    TEST_NAI
                );
                let dns_offset = MIP_RRP_FIXED_LEN + MIP_EXTENSION_HEADER_LEN + TEST_NAI.len();
                assert_eq!(
                    &udp.payload[dns_offset..dns_offset + TEST_3GPP2_DNS_NVSE.len()],
                    &TEST_3GPP2_DNS_NVSE
                );
                assert_eq!(
                    udp.payload[dns_offset + TEST_3GPP2_DNS_NVSE.len()],
                    MIP_MOBILE_FOREIGN_CHALLENGE_EXT
                );
            }
            other => panic!("unexpected result: {other:?}"),
        }
    }

    #[test]
    fn ipv4_udp_checksum_handles_odd_length_payload() {
        let udp = build_udp_packet_with_ipv4_checksum(
            Ipv4Addr::new(192, 0, 2, 1),
            Ipv4Addr::new(198, 51, 100, 2),
            12_345,
            UDP_PORT_MOBILE_IP,
            &[1, 2, 3],
        );
        assert_eq!(
            u16::from_be_bytes([udp[UDP_CHECKSUM_OFFSET], udp[UDP_CHECKSUM_OFFSET + 1]]),
            0xddb3
        );
    }

    #[test]
    fn sprint_rrq_authenticates_and_receives_signed_reply() {
        let payload = sprint_authenticated_rrq_payload();
        let rrq = parse_registration_request(&payload).expect("Sprint RRQ should parse");
        let authentication = rrq.mn_ha_auth().expect("MN-HA authentication");
        assert_eq!(authentication.spi, 1234);
        assert_eq!(
            keyed_md5_prefix_suffix(b"secret", &payload[..authentication.protected_data_len]),
            authentication.authenticator.as_slice()
        );

        let mut session = MobileIpSession::new(sprint_authenticated_config(true));
        session.issue_challenge(rrq.mn_fa_challenge().unwrap().to_vec());
        let packet = registration_request_packet(&payload);
        let result = session.handle_ipv4_packet(&packet, TEST_ASSIGNED_HOME);
        let MobileIpPacketResult::Registered { binding, reply } = result else {
            panic!("unexpected result: {result:?}");
        };
        assert_eq!(binding.home_address, TEST_ASSIGNED_HOME);
        assert_eq!(binding.home_agent, Ipv4Addr::new(68, 28, 15, 12));

        let reply = registration_reply_payload(&reply);
        assert_eq!(reply[MIP_RRP_TYPE_OFFSET], MIP_RRP_TYPE);
        assert_eq!(reply[MIP_RRP_CODE_OFFSET], RRP_CODE_ACCEPTED);
        assert_eq!(reply[MIP_RRP_FIXED_LEN], MIP_MN_NAI_EXT);
        assert_eq!(reply[MIP_RRP_FIXED_LEN + 1] as usize, TEST_SPRINT_NAI.len());
        let nai_start = MIP_RRP_FIXED_LEN + MIP_EXTENSION_HEADER_LEN;
        assert_eq!(
            &reply[nai_start..nai_start + TEST_SPRINT_NAI.len()],
            TEST_SPRINT_NAI
        );
        let dns_offset = nai_start + TEST_SPRINT_NAI.len();
        assert_eq!(
            &reply[dns_offset..dns_offset + TEST_3GPP2_DNS_NVSE.len()],
            &TEST_3GPP2_DNS_NVSE
        );
        let mn_ha_offset = dns_offset + TEST_3GPP2_DNS_NVSE.len();
        assert_eq!(reply[mn_ha_offset], MIP_MOBILE_HOME_AUTH_EXT);
        assert_eq!(reply[mn_ha_offset + 1], MIP_AUTH_EXTENSION_VALUE_LEN);
        let spi_offset = mn_ha_offset + MIP_EXTENSION_HEADER_LEN;
        assert_eq!(
            u32::from_be_bytes(
                reply[spi_offset..spi_offset + AUTH_EXTENSION_SPI_LEN]
                    .try_into()
                    .unwrap()
            ),
            1234
        );
        let authenticator_offset = spi_offset + AUTH_EXTENSION_SPI_LEN;
        let expected_authenticator = [
            0xb3, 0x52, 0x97, 0xb9, 0x11, 0x6b, 0x28, 0x0d, 0x19, 0x4f, 0xd8, 0xaf, 0xaa, 0x47,
            0x7f, 0x74,
        ];
        assert_eq!(
            &reply[authenticator_offset..authenticator_offset + MIP_AUTHENTICATOR_LEN],
            &expected_authenticator
        );
        assert_eq!(
            reply[authenticator_offset + MIP_AUTHENTICATOR_LEN],
            MIP_MOBILE_FOREIGN_CHALLENGE_EXT
        );

        let retransmission = session.handle_ipv4_packet(&packet, TEST_ASSIGNED_HOME);
        let MobileIpPacketResult::Registered {
            reply: retransmitted_reply,
            ..
        } = retransmission
        else {
            panic!("unexpected retransmission result: {retransmission:?}");
        };
        assert_eq!(registration_reply_payload(&retransmitted_reply), reply);
    }

    #[test]
    fn unused_challenge_inside_window_is_accepted() {
        let older_challenge = [0xa1; MIP_CHALLENGE_LEN];
        let newer_challenge = [0xb2; MIP_CHALLENGE_LEN];
        let mut session = MobileIpSession::new(sprint_authenticated_config(true));
        session.issue_challenge(older_challenge.to_vec());
        session.issue_challenge(newer_challenge.to_vec());

        let payload =
            sprint_authenticated_rrq_payload_with(TEST_SPRINT_IDENTIFICATION, &older_challenge);
        assert!(matches!(
            session.handle_ipv4_packet(&registration_request_packet(&payload), TEST_ASSIGNED_HOME),
            MobileIpPacketResult::Registered { .. }
        ));
    }

    #[test]
    fn used_challenge_with_new_identification_is_stale() {
        let challenge = [0xa1; MIP_CHALLENGE_LEN];
        let mut session = MobileIpSession::new(sprint_authenticated_config(true));
        session.issue_challenge(challenge.to_vec());

        let first_payload =
            sprint_authenticated_rrq_payload_with(TEST_SPRINT_IDENTIFICATION, &challenge);
        assert!(matches!(
            session.handle_ipv4_packet(
                &registration_request_packet(&first_payload),
                TEST_ASSIGNED_HOME
            ),
            MobileIpPacketResult::Registered { .. }
        ));

        let second_payload = sprint_authenticated_rrq_payload_with(
            TEST_SPRINT_IDENTIFICATION.wrapping_add(1),
            &challenge,
        );
        let result = session.handle_ipv4_packet(
            &registration_request_packet(&second_payload),
            TEST_ASSIGNED_HOME,
        );
        let MobileIpPacketResult::Reply(reply) = result else {
            panic!("unexpected result: {result:?}");
        };
        assert_eq!(
            registration_reply_payload(&reply)[MIP_RRP_CODE_OFFSET],
            RRP_CODE_STALE_MN_FA_CHALLENGE
        );
        assert_eq!(
            session.binding().map(|binding| binding.identification),
            Some(TEST_SPRINT_IDENTIFICATION)
        );
    }

    #[test]
    fn challenge_outside_window_is_unknown() {
        let old_challenge = [0xa1; MIP_CHALLENGE_LEN];
        let mut session = MobileIpSession::new(sprint_authenticated_config(true));
        session.issue_challenge(old_challenge.to_vec());
        session.issue_challenge(vec![0xb2; MIP_CHALLENGE_LEN]);
        session.issue_challenge(vec![0xc3; MIP_CHALLENGE_LEN]);

        let payload =
            sprint_authenticated_rrq_payload_with(TEST_SPRINT_IDENTIFICATION, &old_challenge);
        let result =
            session.handle_ipv4_packet(&registration_request_packet(&payload), TEST_ASSIGNED_HOME);
        let MobileIpPacketResult::Reply(reply) = result else {
            panic!("unexpected result: {result:?}");
        };
        assert_eq!(
            registration_reply_payload(&reply)[MIP_RRP_CODE_OFFSET],
            RRP_CODE_UNKNOWN_MN_FA_CHALLENGE
        );
        assert!(session.binding().is_none());
    }

    #[test]
    fn invalid_mn_ha_authenticator_is_rejected() {
        let mut payload = sprint_authenticated_rrq_payload();
        let rrq = parse_registration_request(&payload).unwrap();
        let authenticator_offset =
            rrq.mn_ha_auth().unwrap().protected_data_len + AUTH_EXTENSION_SPI_LEN;
        payload[authenticator_offset] ^= 0xff;

        let mut session = MobileIpSession::new(sprint_authenticated_config(true));
        session.issue_challenge(rrq.mn_fa_challenge().unwrap().to_vec());
        let result =
            session.handle_ipv4_packet(&registration_request_packet(&payload), TEST_ASSIGNED_HOME);
        let MobileIpPacketResult::Reply(reply) = result else {
            panic!("unexpected result: {result:?}");
        };
        assert_eq!(
            registration_reply_payload(&reply)[MIP_RRP_CODE_OFFSET],
            RRP_CODE_HA_MOBILE_NODE_AUTH_FAILED
        );
        assert!(session.binding().is_none());
    }

    #[test]
    fn mn_aaa_requires_explicit_relaxed_policy() {
        let payload = sprint_authenticated_rrq_payload();
        let rrq = parse_registration_request(&payload).unwrap();
        let mut session = MobileIpSession::new(sprint_authenticated_config(false));
        session.issue_challenge(rrq.mn_fa_challenge().unwrap().to_vec());
        assert_eq!(
            session.handle_ipv4_packet(&registration_request_packet(&payload), TEST_ASSIGNED_HOME,),
            MobileIpPacketResult::AuthenticationRequired
        );
    }
}
