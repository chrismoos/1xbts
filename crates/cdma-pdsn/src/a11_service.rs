use std::{
    collections::HashMap,
    future::Future,
    net::{IpAddr, Ipv4Addr, SocketAddr},
    pin::Pin,
    sync::Arc,
};

use cdma_common::error::Error;
use log::{info, warn};

use crate::{PdsnNodeConfig, PdsnSessionManager, spawn_pdsn_a10_runtime};

const A11_MSID_TYPE_IMSI: u16 = 0x0006;
const SERVICE_OPTION_PACKET_DATA_RC3: u32 = 33;
/// PCF A10 bearer port assumed when `pdsn.a10_bearer.udp_peer_addr` is not
/// set, matching the default assignment in `docs/PORTS.md`.
const DEFAULT_PCF_A10_UDP_PORT: u16 = 17042;

pub(crate) struct PdsnA10ByteStream {
    pub uplink_tx: tokio::sync::mpsc::Sender<Vec<u8>>,
    pub downlink_rx: tokio::sync::mpsc::Receiver<Vec<u8>>,
}

/// A10 carries keyed GRE across the A* boundary even when this interface is process-local.
pub(crate) trait PdsnA10ByteStreamService: Send + Sync {
    fn open_pdsn_a10_byte_stream(
        &self,
        session_id: String,
        service_option: u32,
        metadata: cdma_packet::session_task::SessionMetadata,
    ) -> Result<PdsnA10ByteStream, String>;

    fn close_pdsn_a10_byte_stream<'a>(
        &'a self,
        session_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
}

pub struct PacketServicePdsnA10Adapter {
    packet_service: Arc<cdma_packet::grpc::PacketServiceImpl>,
}

impl PacketServicePdsnA10Adapter {
    pub fn new(packet_service: Arc<cdma_packet::grpc::PacketServiceImpl>) -> Self {
        Self { packet_service }
    }
}

impl PdsnA10ByteStreamService for PacketServicePdsnA10Adapter {
    fn open_pdsn_a10_byte_stream(
        &self,
        session_id: String,
        service_option: u32,
        metadata: cdma_packet::session_task::SessionMetadata,
    ) -> Result<PdsnA10ByteStream, String> {
        let (uplink_tx, downlink_rx) = self
            .packet_service
            .open_pdsn_a10_byte_stream_session_direct(session_id, service_option, metadata)?;
        Ok(PdsnA10ByteStream {
            uplink_tx,
            downlink_rx,
        })
    }

    fn close_pdsn_a10_byte_stream<'a>(
        &'a self,
        session_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            self.packet_service
                .close_pdsn_a10_byte_stream_session_direct(session_id)
                .await;
        })
    }
}

fn socket_ipv4_octets(addr: SocketAddr, label: &str) -> Result<[u8; 4], String> {
    match addr.ip() {
        IpAddr::V4(ip) => Ok(ip.octets()),
        IpAddr::V6(_) => Err(format!("{label} must be IPv4 for the current A10/A11 path")),
    }
}

/// A peer names its association with the SPI it signs with, so an inbound
/// message selects one before its authenticator is checked and the answer is
/// signed with the same association.
pub struct A11SecurityAssociations {
    default: cdma_a11::A11SecurityAssociation,
    peers: HashMap<u32, (cdma_a11::A11SecurityAssociation, Option<String>)>,
}

impl A11SecurityAssociations {
    pub fn from_config(config: &crate::config::PdsnNodeConfig) -> Result<Self, String> {
        let default = cdma_a11::A11SecurityAssociation::from_config(&config.a11_security)?;
        let mut peers = HashMap::new();
        for peer in &config.a11_peers {
            let association = cdma_a11::A11SecurityAssociation::from_config(&peer.security)?;
            peers.insert(peer.security.spi, (association, peer.feid.clone()));
        }
        Ok(Self { default, peers })
    }

    fn for_spi(&self, spi: u32) -> &cdma_a11::A11SecurityAssociation {
        self.peers
            .get(&spi)
            .map(|(association, _)| association)
            .unwrap_or(&self.default)
    }

    fn for_request(
        &self,
        request: &cdma_a11::RegistrationRequest,
    ) -> &cdma_a11::A11SecurityAssociation {
        match request_spi(request) {
            Some(spi) => self.for_spi(spi),
            None => &self.default,
        }
    }

    fn label_for_request(&self, request: &cdma_a11::RegistrationRequest) -> Option<&str> {
        let spi = request_spi(request)?;
        self.peers.get(&spi)?.1.as_deref()
    }
}

impl cdma_a11::AuthenticationVerifier for A11SecurityAssociations {
    fn verify_authentication(
        &self,
        wire_bytes: &[u8],
        message: &cdma_a11::Message,
        authentication: &cdma_a11::AuthenticationExtension,
    ) -> cdma_a11::Result<()> {
        self.for_spi(authentication.security_parameter_index)
            .verify_authentication(wire_bytes, message, authentication)
    }
}

fn request_spi(request: &cdma_a11::RegistrationRequest) -> Option<u32> {
    request
        .extensions
        .iter()
        .find_map(|extension| match extension {
            cdma_a11::Extension::Authentication(authentication) => {
                Some(authentication.security_parameter_index)
            }
            _ => None,
        })
}

/// The bearer's own IPv4 address.
///
/// UDP encapsulation carries it in `udp_bind_addr`. Raw GRE has no addresses of
/// its own, so the node's A11 bind address names the interface signaling and
/// bearer share.
fn configured_a10_local_ipv4(
    bearer: &cdma_a10::BearerTransportConfig,
    a11_bind: SocketAddr,
    label: &str,
) -> Result<[u8; 4], Error> {
    match bearer.mode {
        cdma_a10::BearerTransportMode::UdpEncapsulatedGre => {
            let bind = bearer.udp_bind_addr.ok_or_else(|| {
                Error::from(format!(
                    "{label}.udp_bind_addr is required for udp_encapsulated_gre"
                ))
            })?;
            socket_ipv4_octets(bind, &format!("{label}.udp_bind_addr")).map_err(Error::from)
        }
        cdma_a10::BearerTransportMode::RawGre => {
            socket_ipv4_octets(a11_bind, "pdsn.a11.bind_addr").map_err(Error::from)
        }
    }
}

/// Where a session's A10 downlink goes until its first uplink packet arrives.
///
/// The PCF that sent the A11 Registration Request owns the session, so its
/// address is the bearer peer. A11 carries no bearer port, so the port comes
/// from the configured `udp_peer_addr` or the standard PCF A10 port.
fn seeded_pcf_a10_addr(a11_source: SocketAddr, configured_peer: Option<SocketAddr>) -> SocketAddr {
    let port = configured_peer.map_or(DEFAULT_PCF_A10_UDP_PORT, |peer| peer.port());
    SocketAddr::new(a11_source.ip(), port)
}

fn mn_id_hex(msid: &[u8]) -> String {
    msid.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn digits_to_imsi(digits: Vec<u8>) -> Result<String, String> {
    let imsi = digits
        .into_iter()
        .map(|digit| char::from(b'0' + digit))
        .collect::<String>();
    if !(10..=15).contains(&imsi.len()) {
        return Err(format!(
            "A11 IMSI MSID decoded to invalid length {}",
            imsi.len()
        ));
    }
    Ok(imsi)
}

fn a11_msid_tbcd_to_imsi(msid: &[u8]) -> Result<String, String> {
    if msid.is_empty() {
        return Err("A11 IMSI MSID is empty".to_string());
    }
    let mut digits = Vec::with_capacity(msid.len() * 2);
    for byte in msid.iter().copied() {
        let low = byte & 0x0f;
        let high = byte >> 4;
        if low == 0x0f {
            break;
        }
        if low > 9 {
            return Err("A11 IMSI TBCD low digit is not decimal".to_string());
        }
        digits.push(low);
        if high == 0x0f {
            break;
        }
        if high > 9 {
            return Err("A11 IMSI TBCD high digit is not decimal".to_string());
        }
        digits.push(high);
    }
    digits_to_imsi(digits)
}

fn a11_msid_bcd_to_imsi(msid: &[u8]) -> Result<String, String> {
    if msid.is_empty() {
        return Err("A11 IMSI MSID is empty".to_string());
    }
    let first = msid[0];
    let first_digit = first >> 4;
    if first_digit > 9 {
        return Err("A11 IMSI MSID first digit is not decimal".to_string());
    }
    let odd = (first & 1) != 0;
    if first & 0x0e != 0 {
        return Err("A11 IMSI MSID odd/even reserved bits are non-zero".to_string());
    }
    let mut digits = Vec::with_capacity(msid.len() * 2);
    digits.push(first_digit);
    for (idx, byte) in msid.iter().copied().enumerate().skip(1) {
        let low = byte & 0x0f;
        let high = byte >> 4;
        if low > 9 {
            return Err("A11 IMSI MSID low digit is not decimal".to_string());
        }
        digits.push(low);
        let is_last = idx == msid.len() - 1;
        if is_last && odd && high == 0x0f {
            continue;
        }
        if high > 9 {
            return Err("A11 IMSI MSID high digit is not decimal".to_string());
        }
        digits.push(high);
    }
    digits_to_imsi(digits)
}

fn packet_metadata_from_a11(
    key: cdma_a11::SessionKey,
    request: &cdma_a11::RegistrationRequest,
) -> cdma_packet::session_task::SessionMetadata {
    let (imsi, mn_id) = if request.session.mn_id_type == A11_MSID_TYPE_IMSI {
        match a11_msid_bcd_to_imsi(&request.session.mn_id) {
            Ok(id) => (None, Some(id)),
            Err(_) => match a11_msid_tbcd_to_imsi(&request.session.mn_id) {
                Ok(id) => (Some(id.clone()), Some(id)),
                Err(err) => {
                    warn!(
                        "PDSN A11: failed to decode IMSI MSID: {err} type={} mn_id={}",
                        request.session.mn_id_type,
                        mn_id_hex(&request.session.mn_id)
                    );
                    (None, None)
                }
            },
        }
    } else {
        (None, None)
    };
    cdma_packet::session_task::SessionMetadata {
        access_technology: "unspecified".to_string(),
        mobile_address: format!("pcf-session:{:08x}", key.pcf_session_id),
        subscriber_id: None,
        phone_number: String::new(),
        imsi,
        esn: None,
        meid: None,
        mn_id,
        mn_id_source: Some("a11".to_string()),
        subscriber_imsi: None,
        a10: Some(cdma_packet::session_task::A10Reference {
            pcf_session_id: key.pcf_session_id,
            mn_session_reference_id: key.mn_session_reference_id,
        }),
        traffic_walsh_code: key.pcf_session_id,
    }
}

/// 0xffff is reserved and the codec rejects it, so an infinite request
/// lifetime is answered with the largest legal value.
const A11_MAX_LIFETIME: u16 = 0xfffe;

fn accepted_reply_lifetime(requested: u16) -> u16 {
    if requested == u16::MAX {
        A11_MAX_LIFETIME
    } else {
        requested
    }
}

fn build_a11_registration_reply(
    request: &cdma_a11::RegistrationRequest,
    security: &cdma_a11::A11SecurityAssociation,
) -> Result<cdma_a11::Message, String> {
    let mut message = cdma_a11::Message::RegistrationReply(cdma_a11::RegistrationReply {
        code: 0,
        lifetime: accepted_reply_lifetime(request.lifetime),
        home_address: request.home_address,
        home_agent: request.home_agent,
        identification: request.identification,
        session: request.session.clone(),
        extensions: vec![cdma_a11::Extension::Authentication(
            security.placeholder_authentication(cdma_a11::AuthenticationExtensionType::MobileHome),
        )],
    });
    security
        .sign_message(&mut message)
        .map_err(|err| format!("sign A11 Registration Reply: {err}"))?;
    Ok(message)
}

fn build_hrpd_a11_registration_update(
    request: &cdma_a11::RegistrationRequest,
    security: &cdma_a11::A11SecurityAssociation,
) -> Result<cdma_a11::Message, String> {
    let mut message = cdma_a11::Message::RegistrationUpdate(cdma_a11::RegistrationUpdate {
        reserved: [0; 3],
        home_address: [0; 4],
        home_agent: request.home_agent,
        // A.S0008/A.S0007 Registration Update for PDSN-initiated A10 release
        // references the active registration transaction. The PCF procedure
        // table validates this against the committed A11 session.
        identification: request.identification,
        session: request.session.clone(),
        nvses: Vec::new(),
        authentication_extension: security
            .placeholder_authentication(cdma_a11::AuthenticationExtensionType::RegistrationUpdate),
    });
    security
        .sign_message(&mut message)
        .map_err(|err| format!("sign A11 Registration Update: {err}"))?;
    Ok(message)
}

fn a10_packet_session_id(key: cdma_a11::SessionKey) -> String {
    format!(
        "hrpd-a10-{:08x}-{:04x}",
        key.pcf_session_id, key.mn_session_reference_id
    )
}

pub async fn spawn_pdsn_a11_service(
    pdsn_config: PdsnNodeConfig,
    packet_service: Arc<cdma_packet::grpc::PacketServiceImpl>,
) -> Result<SocketAddr, Error> {
    spawn_pdsn_a11_service_with_a10_service(
        pdsn_config,
        Arc::new(PacketServicePdsnA10Adapter::new(packet_service)),
    )
    .await
}

pub(crate) async fn spawn_pdsn_a11_service_with_a10_service(
    pdsn_config: PdsnNodeConfig,
    a10_byte_stream_service: Arc<dyn PdsnA10ByteStreamService>,
) -> Result<SocketAddr, Error> {
    let a10_local_ipv4 = configured_a10_local_ipv4(
        &pdsn_config.a10_bearer,
        pdsn_config.a11.bind_addr,
        "pdsn.a10_bearer",
    )
    .map_err(|err| Error::from(format!("PDSN A10 config: {err}")))?;
    let a10_configured_peer = pdsn_config.a10_bearer.udp_peer_addr;
    if a10_configured_peer.is_none() {
        info!(
            "PDSN A10: pdsn.a10_bearer.udp_peer_addr not set, assuming PCF A10 port {DEFAULT_PCF_A10_UDP_PORT} until each session's first uplink packet"
        );
    }
    let (a10_session_closed_tx, mut a10_session_closed_rx) =
        tokio::sync::mpsc::unbounded_channel::<cdma_a11::SessionKey>();
    let a10_runtime = spawn_pdsn_a10_runtime(
        pdsn_config.a10_bearer,
        a10_local_ipv4,
        a10_session_closed_tx,
    )?;
    let requested_a11_bind_addr = pdsn_config.a11.bind_addr;
    let endpoint =
        cdma_a11::UdpEndpoint::bind_with_framing(requested_a11_bind_addr, pdsn_config.a11.framing)
            .await
            .map_err(|err| {
                Error::from(format!(
                    "failed to bind PDSN A11 {requested_a11_bind_addr}: {err}"
                ))
            })?;
    let a11_bind_addr = endpoint
        .local_addr()
        .map_err(|err| Error::from(format!("failed to read PDSN A11 local addr: {err}")))?;
    let a11_security = A11SecurityAssociations::from_config(&pdsn_config)
        .map_err(|err| Error::from(format!("invalid PDSN A11 security config: {err}")))?;
    let expected_peer = pdsn_config.a11.peer_addr;
    tokio::spawn(async move {
        let mut manager = PdsnSessionManager::new();
        let mut registrations: HashMap<
            cdma_a11::SessionKey,
            (cdma_a11::RegistrationRequest, SocketAddr),
        > = HashMap::new();
        let mut buf = vec![0u8; 4096];
        info!("PDSN A11 signaling listener bound on {a11_bind_addr}");
        loop {
            let (message, peer) = tokio::select! {
                closed = a10_session_closed_rx.recv() => {
                    let Some(key) = closed else {
                        continue;
                    };
                    let Some((request, peer)) = registrations.get(&key).cloned() else {
                        warn!("PDSN A11: packet session closed for unregistered {key:?}; cannot send Registration Update");
                        continue;
                    };
                    let update = match build_hrpd_a11_registration_update(&request, a11_security.for_request(&request)) {
                        Ok(update) => update,
                        Err(err) => {
                            warn!("PDSN A11: failed to build Registration Update for {key:?}: {err}");
                            continue;
                        }
                    };
                    info!(
                        "PDSN A11: packet session closed for {key:?}; sending Registration Update to {peer}"
                    );
                    if let Err(err) = endpoint.send_message(peer, update).await {
                        warn!("PDSN A11: failed to send Registration Update for {key:?} to {peer}: {err}");
                    }
                    continue;
                }
                received = endpoint.recv_message_verified(&mut buf, &a11_security) => {
                    match received {
                        Ok((message, peer)) => (message.into_message(), peer),
                        Err(err) => {
                            warn!("PDSN A11: failed to receive message: {err}");
                            continue;
                        }
                    }
                }
            };
            if expected_peer.port() != 0 && peer != expected_peer {
                warn!(
                    "PDSN A11: message came from unexpected peer {peer}, expected {expected_peer}"
                );
            }
            if let cdma_a11::Message::RegistrationAcknowledge(ack) = &message {
                let key = cdma_a11::SessionKey::from_session(&ack.session);
                if ack.status == 0 {
                    info!("PDSN A11: Registration Update acknowledged for {key:?}");
                } else {
                    warn!(
                        "PDSN A11: Registration Update rejected for {key:?} status=0x{:02x}",
                        ack.status
                    );
                }
                continue;
            }
            let cdma_a11::Message::RegistrationRequest(request) = message else {
                warn!(
                    "PDSN A11: ignoring unsupported message {:?} from {peer}",
                    message.message_type()
                );
                continue;
            };
            let key = cdma_a11::SessionKey::from_session(&request.session);
            if request.lifetime == 0 {
                let session_id = a10_packet_session_id(key);
                info!(
                    "PDSN A11: received deregistration for {key:?}; closing packet session {session_id}"
                );
                registrations.remove(&key);
                a10_runtime.release(key);
                a10_byte_stream_service
                    .close_pdsn_a10_byte_stream(&session_id)
                    .await;
                let _ = manager.apply_a11(
                    0,
                    cdma_a11::Direction::Inbound,
                    &cdma_a11::Message::RegistrationRequest(request.clone()),
                );
                let reply = match build_a11_registration_reply(
                    &request,
                    a11_security.for_request(&request),
                ) {
                    Ok(reply) => reply,
                    Err(err) => {
                        warn!("PDSN A11: failed to build deregistration Reply to {peer}: {err}");
                        continue;
                    }
                };
                if let Err(err) = endpoint.send_message(peer, reply).await {
                    warn!("PDSN A11: failed to send deregistration Reply to {peer}: {err}");
                }
                continue;
            }
            registrations.insert(key, (request.clone(), peer));
            if manager.session(key).is_none() {
                match manager.install_registered_session(key) {
                    Ok(event) => match a11_security.label_for_request(&request) {
                        Some(peer_label) => {
                            info!(
                                "PDSN A11: {event:?} peer={peer_label} lifetime={}",
                                request.lifetime
                            )
                        }
                        None => info!("PDSN A11: {event:?} lifetime={}", request.lifetime),
                    },
                    Err(err) => {
                        warn!("PDSN A11: failed to install session {key:?}: {err}");
                        continue;
                    }
                }
            }
            let pcf_a10_addr = seeded_pcf_a10_addr(peer, a10_configured_peer);
            let IpAddr::V4(pcf_a10_ipv4) = pcf_a10_addr.ip() else {
                warn!(
                    "PDSN A11: cannot bind A10 for {key:?}, PCF {peer} must be IPv4 for the current A10/A11 path"
                );
                continue;
            };
            let a10 = cdma_a10::BearerSession::new(
                key.pcf_session_id,
                cdma_a10::BearerEndpoint::new(a10_local_ipv4, pcf_a10_ipv4.octets()),
            );
            match manager.bind_a10_bearer(key, a10) {
                Ok(event) => info!(
                    "PDSN A11: {event:?}; A10 bound {} -> {pcf_a10_addr} key=0x{:08x}",
                    Ipv4Addr::from(a10_local_ipv4),
                    key.pcf_session_id
                ),
                Err(err) => {
                    warn!("PDSN A11: failed to bind A10 for {key:?}: {err}");
                    continue;
                }
            }
            let session_id = a10_packet_session_id(key);
            let metadata = packet_metadata_from_a11(key, &request);
            match a10_byte_stream_service.open_pdsn_a10_byte_stream(
                session_id,
                SERVICE_OPTION_PACKET_DATA_RC3,
                metadata,
            ) {
                Ok(stream) => {
                    a10_runtime.register(
                        key,
                        a10,
                        pcf_a10_addr,
                        stream.uplink_tx,
                        stream.downlink_rx,
                    );
                }
                Err(err) if err.contains("already exists") => {
                    info!(
                        "PDSN A11: keeping packet session for {key:?} lifetime={}",
                        request.lifetime
                    );
                }
                Err(err) => {
                    warn!("PDSN A10: packet session not opened for {key:?}: {err}");
                }
            }
            let reply =
                match build_a11_registration_reply(&request, a11_security.for_request(&request)) {
                    Ok(reply) => reply,
                    Err(err) => {
                        warn!("PDSN A11: failed to build Registration Reply to {peer}: {err}");
                        continue;
                    }
                };
            if let Err(err) = endpoint.send_message(peer, reply).await {
                warn!("PDSN A11: failed to send Registration Reply to {peer}: {err}");
            }
        }
    });
    Ok(a11_bind_addr)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registration_request_with_imsi_mnid() -> cdma_a11::RegistrationRequest {
        cdma_a11::RegistrationRequest {
            flags: 0,
            lifetime: 600,
            home_address: [0, 0, 0, 0],
            home_agent: [192, 0, 2, 1],
            care_of_address: [192, 0, 2, 2],
            identification: 0x0102_0304_0506_0708,
            session: cdma_a11::SessionSpecificExtension {
                protocol_type: 0x8881,
                pcf_session_id: 0x1a05_8001,
                session_id_version: 1,
                mn_session_reference_id: 7,
                mn_id_type: A11_MSID_TYPE_IMSI,
                mn_id: vec![0x31, 0x01, 0x55, 0x86, 0x89, 0x10, 0x37, 0x23],
            },
            extensions: Vec::new(),
        }
    }

    #[test]
    fn a11_imsi_msid_is_mn_id_not_subscriber_imsi() {
        let request = registration_request_with_imsi_mnid();
        let key = cdma_a11::SessionKey::from_session(&request.session);

        let metadata = packet_metadata_from_a11(key, &request);

        assert_eq!(metadata.imsi, None);
        assert_eq!(metadata.subscriber_imsi, None);
        assert_eq!(metadata.mn_id.as_deref(), Some("310556898017332"));
        assert_eq!(metadata.mn_id_source.as_deref(), Some("a11"));
        assert_eq!(metadata.mobile_address, "pcf-session:1a058001");
        assert_eq!(
            metadata.a10,
            Some(cdma_packet::session_task::A10Reference {
                pcf_session_id: key.pcf_session_id,
                mn_session_reference_id: key.mn_session_reference_id,
            })
        );
    }

    #[test]
    fn a11_imsi_tbcd_decodes_1x_msid() {
        let msid = vec![0x13, 0x00, 0x50, 0x55, 0x21, 0x93, 0x99, 0xf9];
        assert_eq!(a11_msid_tbcd_to_imsi(&msid).unwrap(), "310005551239999");
        assert!(a11_msid_bcd_to_imsi(&msid).is_err());
        let mut request = registration_request_with_imsi_mnid();
        request.session.mn_id = msid;
        let key = cdma_a11::SessionKey::from_session(&request.session);
        let metadata = packet_metadata_from_a11(key, &request);
        assert_eq!(metadata.imsi.as_deref(), Some("310005551239999"));
        assert_eq!(metadata.mn_id.as_deref(), Some("310005551239999"));
    }

    fn associations() -> A11SecurityAssociations {
        let make = |spi, secret: &str| {
            cdma_a11::A11SecurityAssociation::from_config(&cdma_a11::A11SecurityConfig {
                spi,
                shared_secret_hex: secret.to_string(),
            })
            .unwrap()
        };
        A11SecurityAssociations {
            default: make(256, "00112233"),
            peers: HashMap::from([(512, (make(512, "44556677"), Some("airave-1".to_string())))]),
        }
    }

    fn request_signed_with_spi(spi: u32) -> cdma_a11::RegistrationRequest {
        let mut request = registration_request_with_imsi_mnid();
        request.extensions.push(cdma_a11::Extension::Authentication(
            cdma_a11::AuthenticationExtension {
                extension_type: cdma_a11::AuthenticationExtensionType::MobileHome,
                security_parameter_index: spi,
                authenticator: vec![0u8; 16],
            },
        ));
        request
    }

    #[test]
    fn an_infinite_request_lifetime_is_answered_with_a_legal_one() {
        assert_eq!(accepted_reply_lifetime(u16::MAX), 0xfffe);
        assert_eq!(accepted_reply_lifetime(1800), 1800);
    }

    #[test]
    fn a_peer_spi_selects_that_peers_association() {
        let associations = associations();
        let request = request_signed_with_spi(512);

        assert_eq!(associations.for_request(&request).spi(), 512);
        assert_eq!(associations.label_for_request(&request), Some("airave-1"));
    }

    #[test]
    fn an_unknown_spi_falls_back_to_the_default_association() {
        let associations = associations();
        let request = request_signed_with_spi(999);

        assert_eq!(associations.for_request(&request).spi(), 256);
        assert_eq!(associations.label_for_request(&request), None);
    }

    #[test]
    fn raw_gre_takes_its_local_address_from_the_a11_bind() {
        let a11_bind: SocketAddr = "10.56.0.2:699".parse().unwrap();
        let bearer = cdma_a10::BearerTransportConfig::raw_gre();

        assert_eq!(
            configured_a10_local_ipv4(&bearer, a11_bind, "pdsn.a10_bearer").unwrap(),
            [10, 56, 0, 2]
        );
    }

    #[test]
    fn udp_encapsulation_takes_its_local_address_from_the_bearer_bind() {
        let a11_bind: SocketAddr = "10.56.0.2:699".parse().unwrap();
        let bearer = cdma_a10::BearerTransportConfig::udp_encapsulated_gre(
            "127.0.0.1:17043".parse().unwrap(),
            "127.0.0.1:17042".parse().unwrap(),
        );

        assert_eq!(
            configured_a10_local_ipv4(&bearer, a11_bind, "pdsn.a10_bearer").unwrap(),
            [127, 0, 0, 1]
        );
    }

    #[test]
    fn seeded_a10_peer_takes_pcf_ip_from_a11_and_port_from_config_or_default() {
        let a11_source: SocketAddr = "192.0.2.7:17044".parse().unwrap();
        let configured: SocketAddr = "127.0.0.1:17042".parse().unwrap();

        assert_eq!(
            seeded_pcf_a10_addr(a11_source, Some(configured)),
            "192.0.2.7:17042".parse().unwrap()
        );
        assert_eq!(
            seeded_pcf_a10_addr(a11_source, None),
            SocketAddr::new(a11_source.ip(), DEFAULT_PCF_A10_UDP_PORT)
        );
    }
}
