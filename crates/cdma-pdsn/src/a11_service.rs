use std::{
    collections::HashMap,
    future::Future,
    net::{IpAddr, SocketAddr},
    pin::Pin,
    sync::Arc,
};

use cdma_common::error::Error;
use log::{info, warn};

use crate::{PdsnNodeConfig, PdsnSessionManager, spawn_hrpd_pdsn_a10_runtime};

const A11_MSID_TYPE_IMSI: u16 = 0x0006;
const SERVICE_OPTION_HIGH_RATE_PACKET_DATA: u32 = 33;

pub(crate) struct HrpdA10ByteStream {
    pub uplink_tx: tokio::sync::mpsc::Sender<Vec<u8>>,
    pub downlink_rx: tokio::sync::mpsc::Receiver<Vec<u8>>,
}

/// Internal PDSN boundary from A11 session control into the packet engine.
///
/// The A10 user plane remains keyed GRE on the A* boundary. This trait is only
/// the process-local seam that lets the PDSN A11 service stop owning packet
/// service internals directly; a future process split can install a GRE or
/// remote transport adapter here.
pub(crate) trait HrpdA10ByteStreamService: Send + Sync {
    fn open_hrpd_a10_byte_stream(
        &self,
        session_id: String,
        service_option: u32,
        metadata: cdma_packet::session_task::SessionMetadata,
    ) -> Result<HrpdA10ByteStream, String>;

    fn close_hrpd_a10_byte_stream<'a>(
        &'a self,
        session_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>>;
}

pub struct PacketServiceHrpdA10Adapter {
    packet_service: Arc<cdma_packet::grpc::PacketServiceImpl>,
}

impl PacketServiceHrpdA10Adapter {
    pub fn new(packet_service: Arc<cdma_packet::grpc::PacketServiceImpl>) -> Self {
        Self { packet_service }
    }
}

impl HrpdA10ByteStreamService for PacketServiceHrpdA10Adapter {
    fn open_hrpd_a10_byte_stream(
        &self,
        session_id: String,
        service_option: u32,
        metadata: cdma_packet::session_task::SessionMetadata,
    ) -> Result<HrpdA10ByteStream, String> {
        let (uplink_tx, downlink_rx) = self
            .packet_service
            .open_hrpd_a10_byte_stream_session_direct(session_id, service_option, metadata)?;
        Ok(HrpdA10ByteStream {
            uplink_tx,
            downlink_rx,
        })
    }

    fn close_hrpd_a10_byte_stream<'a>(
        &'a self,
        session_id: &'a str,
    ) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
        Box::pin(async move {
            self.packet_service
                .close_hrpd_a10_byte_stream_session_direct(session_id)
                .await;
        })
    }
}

fn socket_ipv4_octets(addr: SocketAddr, label: &str) -> Result<[u8; 4], String> {
    match addr.ip() {
        IpAddr::V4(ip) => Ok(ip.octets()),
        IpAddr::V6(_) => Err(format!(
            "{label} must be IPv4 for the current HRPD A10/A11 path"
        )),
    }
}

fn configured_a10_ipv4_pair(
    bearer: &cdma_a10::BearerTransportConfig,
    label: &str,
) -> Result<([u8; 4], [u8; 4]), String> {
    let bind = bearer
        .udp_bind_addr
        .ok_or_else(|| format!("{label} must use udp_encapsulated_gre"))?;
    let peer = bearer
        .udp_peer_addr
        .ok_or_else(|| format!("{label} must set udp_peer_addr"))?;
    Ok((
        socket_ipv4_octets(bind, &format!("{label}.udp_bind_addr"))?,
        socket_ipv4_octets(peer, &format!("{label}.udp_peer_addr"))?,
    ))
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

fn hrpd_packet_metadata_from_a11(
    key: cdma_a11::SessionKey,
    request: &cdma_a11::RegistrationRequest,
) -> cdma_packet::session_task::SessionMetadata {
    let hrpd_mn_id = if request.session.mn_id_type == A11_MSID_TYPE_IMSI {
        match a11_msid_bcd_to_imsi(&request.session.mn_id) {
            Ok(imsi) => Some(imsi),
            Err(err) => {
                warn!("HRPD PDSN A11: failed to decode IMSI MSID: {err}");
                None
            }
        }
    } else {
        None
    };
    cdma_packet::session_task::SessionMetadata {
        access_technology: "HRPD".to_string(),
        mobile_address: format!("hrpd-uati-session:{:08x}", key.pcf_session_id),
        subscriber_id: None,
        phone_number: String::new(),
        imsi: None,
        esn: None,
        meid: None,
        hrpd_mn_id,
        hrpd_mn_id_source: Some("a11".to_string()),
        subscriber_imsi: None,
        traffic_walsh_code: key.pcf_session_id,
    }
}

fn build_hrpd_a11_registration_reply(
    request: &cdma_a11::RegistrationRequest,
    security: &cdma_a11::A11SecurityAssociation,
) -> Result<cdma_a11::Message, String> {
    let mut message = cdma_a11::Message::RegistrationReply(cdma_a11::RegistrationReply {
        code: 0,
        lifetime: request.lifetime,
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

fn hrpd_a10_packet_session_id(key: cdma_a11::SessionKey) -> String {
    format!(
        "hrpd-a10-{:08x}-{:04x}",
        key.pcf_session_id, key.mn_session_reference_id
    )
}

pub async fn spawn_hrpd_pdsn_a11_service(
    pdsn_config: PdsnNodeConfig,
    packet_service: Arc<cdma_packet::grpc::PacketServiceImpl>,
) -> Result<SocketAddr, Error> {
    spawn_hrpd_pdsn_a11_service_with_a10_service(
        pdsn_config,
        Arc::new(PacketServiceHrpdA10Adapter::new(packet_service)),
    )
    .await
}

pub(crate) async fn spawn_hrpd_pdsn_a11_service_with_a10_service(
    pdsn_config: PdsnNodeConfig,
    a10_byte_stream_service: Arc<dyn HrpdA10ByteStreamService>,
) -> Result<SocketAddr, Error> {
    let (a10_local_ipv4, a10_peer_ipv4) =
        configured_a10_ipv4_pair(&pdsn_config.a10_bearer, "pdsn.a10_bearer")
            .map_err(|err| Error::from(format!("HRPD PDSN A10 config: {err}")))?;
    let a10_endpoint = cdma_a10::BearerEndpoint::new(a10_local_ipv4, a10_peer_ipv4);
    let (a10_session_closed_tx, mut a10_session_closed_rx) =
        tokio::sync::mpsc::unbounded_channel::<cdma_a11::SessionKey>();
    let a10_runtime =
        spawn_hrpd_pdsn_a10_runtime(pdsn_config.a10_bearer, a10_endpoint, a10_session_closed_tx)?;
    let requested_a11_bind_addr = pdsn_config.a11.bind_addr;
    let endpoint = cdma_a11::UdpEndpoint::bind(requested_a11_bind_addr)
        .await
        .map_err(|err| {
            Error::from(format!(
                "failed to bind HRPD PDSN A11 {requested_a11_bind_addr}: {err}"
            ))
        })?;
    let a11_bind_addr = endpoint
        .local_addr()
        .map_err(|err| Error::from(format!("failed to read HRPD PDSN A11 local addr: {err}")))?;
    let a11_security = cdma_a11::A11SecurityAssociation::from_config(&pdsn_config.a11_security)
        .map_err(|err| Error::from(format!("invalid HRPD PDSN A11 security config: {err}")))?;
    let expected_peer = pdsn_config.a11.peer_addr;
    tokio::spawn(async move {
        let mut manager = PdsnSessionManager::new();
        let mut registrations: HashMap<
            cdma_a11::SessionKey,
            (cdma_a11::RegistrationRequest, SocketAddr),
        > = HashMap::new();
        let mut buf = vec![0u8; 4096];
        info!("HRPD PDSN A11 signaling listener bound on {a11_bind_addr}");
        loop {
            let (message, peer) = tokio::select! {
                closed = a10_session_closed_rx.recv() => {
                    let Some(key) = closed else {
                        continue;
                    };
                    let Some((request, peer)) = registrations.get(&key).cloned() else {
                        warn!("HRPD PDSN A11: packet session closed for unregistered {key:?}; cannot send Registration Update");
                        continue;
                    };
                    let update = match build_hrpd_a11_registration_update(&request, &a11_security) {
                        Ok(update) => update,
                        Err(err) => {
                            warn!("HRPD PDSN A11: failed to build Registration Update for {key:?}: {err}");
                            continue;
                        }
                    };
                    info!(
                        "HRPD PDSN A11: packet session closed for {key:?}; sending Registration Update to {peer}"
                    );
                    if let Err(err) = endpoint.send_message(peer, update).await {
                        warn!("HRPD PDSN A11: failed to send Registration Update for {key:?} to {peer}: {err}");
                    }
                    continue;
                }
                received = endpoint.recv_message_verified(&mut buf, &a11_security) => {
                    match received {
                        Ok((message, peer)) => (message.into_message(), peer),
                        Err(err) => {
                            warn!("HRPD PDSN A11: failed to receive message: {err}");
                            continue;
                        }
                    }
                }
            };
            if expected_peer.port() != 0 && peer != expected_peer {
                warn!(
                    "HRPD PDSN A11: message came from unexpected peer {peer}, expected {expected_peer}"
                );
            }
            if let cdma_a11::Message::RegistrationAcknowledge(ack) = &message {
                let key = cdma_a11::SessionKey::from_session(&ack.session);
                if ack.status == 0 {
                    info!("HRPD PDSN A11: Registration Update acknowledged for {key:?}");
                } else {
                    warn!(
                        "HRPD PDSN A11: Registration Update rejected for {key:?} status=0x{:02x}",
                        ack.status
                    );
                }
                continue;
            }
            let cdma_a11::Message::RegistrationRequest(request) = message else {
                warn!(
                    "HRPD PDSN A11: ignoring unsupported message {:?} from {peer}",
                    message.message_type()
                );
                continue;
            };
            let key = cdma_a11::SessionKey::from_session(&request.session);
            if request.lifetime == 0 {
                let session_id = hrpd_a10_packet_session_id(key);
                info!(
                    "HRPD PDSN A11: received deregistration for {key:?}; closing packet session {session_id}"
                );
                registrations.remove(&key);
                a10_byte_stream_service
                    .close_hrpd_a10_byte_stream(&session_id)
                    .await;
                let _ = manager.apply_a11(
                    0,
                    cdma_a11::Direction::Inbound,
                    &cdma_a11::Message::RegistrationRequest(request.clone()),
                );
                let reply = match build_hrpd_a11_registration_reply(&request, &a11_security) {
                    Ok(reply) => reply,
                    Err(err) => {
                        warn!(
                            "HRPD PDSN A11: failed to build deregistration Reply to {peer}: {err}"
                        );
                        continue;
                    }
                };
                if let Err(err) = endpoint.send_message(peer, reply).await {
                    warn!("HRPD PDSN A11: failed to send deregistration Reply to {peer}: {err}");
                }
                continue;
            }
            registrations.insert(key, (request.clone(), peer));
            if manager.session(key).is_none() {
                match manager.install_registered_session(key) {
                    Ok(event) => info!("HRPD PDSN A11: {event:?}"),
                    Err(err) => {
                        warn!("HRPD PDSN A11: failed to install session {key:?}: {err}");
                        continue;
                    }
                }
            }
            let a10 = cdma_a10::BearerSession::new(key.pcf_session_id, a10_endpoint);
            match manager.bind_a10_bearer(key, a10) {
                Ok(event) => info!(
                    "HRPD PDSN A11: {event:?}; A10 bound {}.{}.{}.{} -> {}.{}.{}.{} key=0x{:08x}",
                    a10_local_ipv4[0],
                    a10_local_ipv4[1],
                    a10_local_ipv4[2],
                    a10_local_ipv4[3],
                    a10_peer_ipv4[0],
                    a10_peer_ipv4[1],
                    a10_peer_ipv4[2],
                    a10_peer_ipv4[3],
                    key.pcf_session_id
                ),
                Err(err) => {
                    warn!("HRPD PDSN A11: failed to bind A10 for {key:?}: {err}");
                    continue;
                }
            }
            let session_id = hrpd_a10_packet_session_id(key);
            match a10_byte_stream_service.open_hrpd_a10_byte_stream(
                session_id,
                SERVICE_OPTION_HIGH_RATE_PACKET_DATA,
                hrpd_packet_metadata_from_a11(key, &request),
            ) {
                Ok(stream) => {
                    a10_runtime.register(key, a10, stream.uplink_tx, stream.downlink_rx);
                }
                Err(err) => {
                    warn!("HRPD PDSN A10: packet session not opened for {key:?}: {err}");
                }
            }
            let reply = match build_hrpd_a11_registration_reply(&request, &a11_security) {
                Ok(reply) => reply,
                Err(err) => {
                    warn!("HRPD PDSN A11: failed to build Registration Reply to {peer}: {err}");
                    continue;
                }
            };
            if let Err(err) = endpoint.send_message(peer, reply).await {
                warn!("HRPD PDSN A11: failed to send Registration Reply to {peer}: {err}");
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
    fn a11_imsi_msid_is_hrpd_mn_id_not_subscriber_imsi() {
        let request = registration_request_with_imsi_mnid();
        let key = cdma_a11::SessionKey::from_session(&request.session);

        let metadata = hrpd_packet_metadata_from_a11(key, &request);

        assert_eq!(metadata.imsi, None);
        assert_eq!(metadata.subscriber_imsi, None);
        assert_eq!(metadata.hrpd_mn_id.as_deref(), Some("310556898017332"));
        assert_eq!(metadata.hrpd_mn_id_source.as_deref(), Some("a11"));
        assert_eq!(metadata.mobile_address, "hrpd-uati-session:1a058001");
    }
}
