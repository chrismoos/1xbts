use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
    time::{SystemTime, UNIX_EPOCH},
};

use cdma_a8::HrpdA9ClientConfig;
use cdma_common::error::Error;
use log::{info, warn};

use crate::{PcfEvent, PcfSessionId, PcfSessionPhase, spawn_hrpd_pcf_bearer_relay};

const HRPD_A11_REGISTRATION_LIFETIME_SECS: u16 = 600;
const HRPD_A11_WAIT_TIMEOUT: std::time::Duration = std::time::Duration::from_millis(700);
const HRPD_A11_MAX_ATTEMPTS: usize = 3;
const A11_PROTOCOL_TYPE_UNSTRUCTURED_BYTE_STREAM: u16 = 0x8881;
const A11_MSID_TYPE_IMSI: u16 = 0x0006;
const HRPD_A9_RELEASE_A8_CAUSE_PPP_SESSION_CLOSED_BY_MS: u8 = 0x77;

fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

async fn send_a9_payload(
    endpoint: &cdma_a9::UdpSignalingEndpoint,
    peer: SocketAddr,
    metadata: cdma_a9::TransportMetadata,
    payload: Vec<u8>,
    label: &str,
) -> Result<(), String> {
    let datagram = cdma_a9::UdpSignalingDatagram::new(metadata, payload)
        .map_err(|err| format!("{label}: encode A9 UDP datagram: {err}"))?;
    endpoint
        .send_datagram(peer, &datagram)
        .await
        .map_err(|err| format!("{label}: send A9 UDP datagram to {peer}: {err}"))?;
    Ok(())
}

#[derive(Clone, Debug)]
struct ConnectedA8 {
    session_id: PcfSessionId,
    connect: cdma_a9::ConnectA8Message,
    peer: SocketAddr,
    metadata: cdma_a9::TransportMetadata,
}

pub fn socket_ipv4_octets(addr: SocketAddr, label: &str) -> Result<[u8; 4], String> {
    match addr.ip() {
        IpAddr::V4(ip) => Ok(ip.octets()),
        IpAddr::V6(_) => Err(format!(
            "{label} must be IPv4 for the current HRPD A8/A9 path"
        )),
    }
}

pub fn configured_a8_ipv4_pair(
    bearer: &cdma_a8::BearerTransportConfig,
    label: &str,
) -> Result<([u8; 4], [u8; 4]), String> {
    let bind = bearer
        .udp_bind_addr
        .ok_or_else(|| format!("{label} must use udp_encapsulated_gre in cdma-nib"))?;
    let peer = bearer
        .udp_peer_addr
        .ok_or_else(|| format!("{label} must set udp_peer_addr in cdma-nib"))?;
    Ok((
        socket_ipv4_octets(bind, &format!("{label}.udp_bind_addr"))?,
        socket_ipv4_octets(peer, &format!("{label}.udp_peer_addr"))?,
    ))
}

fn configured_a10_ipv4_pair(
    bearer: &cdma_a10::BearerTransportConfig,
    label: &str,
) -> Result<([u8; 4], [u8; 4]), String> {
    configured_a8_ipv4_pair(bearer, label)
}

pub fn inverted_udp_gre_bearer(
    bearer: cdma_a8::BearerTransportConfig,
    label: &str,
) -> Result<cdma_a8::BearerTransportConfig, String> {
    bearer.validate(label)?;
    let bind = bearer
        .udp_peer_addr
        .ok_or_else(|| format!("{label}: missing peer addr for inverse endpoint"))?;
    let peer = bearer
        .udp_bind_addr
        .ok_or_else(|| format!("{label}: missing bind addr for inverse endpoint"))?;
    Ok(cdma_a8::BearerTransportConfig::udp_encapsulated_gre(
        bind, peer,
    ))
}

fn a9_mobile_identity_bytes(setup: &cdma_a9::SetupA8Message) -> Option<Vec<u8>> {
    if let Some(imsi) = setup.imsi.as_ref() {
        return cdma_a9::MobileIdentity::Imsi(imsi.clone()).encode().ok();
    }
    if let Some(esn) = setup.esn {
        return cdma_a9::MobileIdentity::Esn(esn).encode().ok();
    }
    setup
        .meid
        .map(cdma_a9::MobileIdentity::Meid)
        .and_then(|identity| identity.encode().ok())
}

fn imsi_to_a11_msid_bcd(imsi: &str) -> Result<Vec<u8>, String> {
    let digits = imsi
        .trim()
        .bytes()
        .map(|byte| match byte {
            b'0'..=b'9' => Ok(byte - b'0'),
            _ => Err(format!(
                "IMSI contains non-decimal digit '{}'",
                byte as char
            )),
        })
        .collect::<Result<Vec<_>, _>>()?;
    if !(10..=15).contains(&digits.len()) {
        return Err(format!(
            "A11 IMSI MSID must contain 10..=15 digits, got {}",
            digits.len()
        ));
    }

    let mut out = Vec::with_capacity(1 + digits.len().div_ceil(2));
    let odd_even = if digits.len() % 2 == 0 { 0 } else { 1 };
    out.push((digits[0] << 4) | odd_even);
    let mut idx = 1;
    while idx < digits.len() {
        let low = digits[idx];
        let high = if idx + 1 < digits.len() {
            digits[idx + 1]
        } else {
            0x0f
        };
        out.push((high << 4) | low);
        idx += 2;
    }
    Ok(out)
}

fn a11_identification() -> u64 {
    let duration = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    (duration.as_nanos() as u64).max(1)
}

fn build_hrpd_a11_session_from_imsi(
    session_id: PcfSessionId,
    imsi: &str,
) -> Result<cdma_a11::SessionSpecificExtension, String> {
    let pcf_session_id = u32::try_from(session_id.0)
        .map_err(|_| format!("PCF session id {} exceeds A11 u32 range", session_id.0))?;
    if pcf_session_id == 0 {
        return Err("PCF session id must be non-zero for A11".to_string());
    }
    Ok(cdma_a11::SessionSpecificExtension {
        protocol_type: A11_PROTOCOL_TYPE_UNSTRUCTURED_BYTE_STREAM,
        pcf_session_id,
        session_id_version: 1,
        mn_session_reference_id: 1,
        mn_id_type: A11_MSID_TYPE_IMSI,
        mn_id: imsi_to_a11_msid_bcd(imsi)?,
    })
}

fn build_hrpd_a11_registration_request_for_imsi(
    session_id: PcfSessionId,
    imsi: &str,
    a11: cdma_a11::A11TransportConfig,
    security: &cdma_a11::A11SecurityAssociation,
    lifetime: u16,
) -> Result<cdma_a11::Message, String> {
    let care_of_address = socket_ipv4_octets(a11.bind_addr, "pcf.a11.bind_addr")?;
    let home_agent = socket_ipv4_octets(a11.peer_addr, "pcf.a11.peer_addr")?;
    let mut message = cdma_a11::Message::RegistrationRequest(cdma_a11::RegistrationRequest {
        flags: 0x0a,
        lifetime,
        home_address: [0, 0, 0, 0],
        home_agent,
        care_of_address,
        identification: a11_identification(),
        session: build_hrpd_a11_session_from_imsi(session_id, imsi)?,
        extensions: vec![cdma_a11::Extension::Authentication(
            security.placeholder_authentication(cdma_a11::AuthenticationExtensionType::MobileHome),
        )],
    });
    security
        .sign_message(&mut message)
        .map_err(|err| format!("sign A11 Registration Request: {err}"))?;
    Ok(message)
}

fn build_hrpd_a11_registration_acknowledge(
    update: &cdma_a11::RegistrationUpdate,
    pcf_config: &crate::PcfNodeConfig,
    security: &cdma_a11::A11SecurityAssociation,
) -> Result<cdma_a11::Message, String> {
    let mut message =
        cdma_a11::Message::RegistrationAcknowledge(cdma_a11::RegistrationAcknowledge {
            reserved: [0; 2],
            status: 0,
            home_address: [0; 4],
            care_of_address: socket_ipv4_octets(pcf_config.a11.bind_addr, "pcf.a11.bind_addr")?,
            identification: update.identification,
            session: update.session.clone(),
            authentication_extension: security.placeholder_authentication(
                cdma_a11::AuthenticationExtensionType::RegistrationUpdate,
            ),
        });
    security
        .sign_message(&mut message)
        .map_err(|err| format!("sign A11 Registration Acknowledge: {err}"))?;
    Ok(message)
}

pub fn build_hrpd_a11_registration_request(
    session_id: PcfSessionId,
    setup: &cdma_a9::SetupA8Message,
    a11: cdma_a11::A11TransportConfig,
    security: &cdma_a11::A11SecurityAssociation,
) -> Result<cdma_a11::Message, String> {
    let imsi = setup
        .imsi
        .as_deref()
        .ok_or("A11 registration requires HLR-resolved IMSI in SetupA8")?;
    build_hrpd_a11_registration_request_for_imsi(
        session_id,
        imsi,
        a11,
        security,
        HRPD_A11_REGISTRATION_LIFETIME_SECS,
    )
}

async fn register_hrpd_a11_session(
    manager: &mut crate::PcfSessionManager,
    procedures: &mut cdma_a11::SessionProcedureTable,
    endpoint: &cdma_a11::UdpEndpoint,
    pcf_config: &crate::PcfNodeConfig,
    security: &cdma_a11::A11SecurityAssociation,
    session_id: PcfSessionId,
    setup: &cdma_a9::SetupA8Message,
    a10_endpoint: cdma_a10::BearerEndpoint,
) -> Result<cdma_a11::SessionKey, String> {
    let request = build_hrpd_a11_registration_request(session_id, setup, pcf_config.a11, security)?;
    let key = match &request {
        cdma_a11::Message::RegistrationRequest(request) => {
            cdma_a11::SessionKey::from_session(&request.session)
        }
        _ => unreachable!("builder returns registration request"),
    };
    manager
        .enqueue_a11(session_id, request)
        .map_err(|err| format!("queue A11 registration: {err}"))?;
    let request = manager
        .pop_pending_a11()
        .ok_or("A11 registration queue unexpectedly empty")?;
    let now = unix_seconds();
    procedures
        .apply(now, cdma_a11::Direction::Outbound, &request)
        .map_err(|err| format!("apply outbound A11 Registration Request: {err}"))?;
    let mut buf = vec![0u8; 4096];
    let mut received = None;
    for attempt in 1..=HRPD_A11_MAX_ATTEMPTS {
        endpoint
            .send_message(pcf_config.a11.peer_addr, request.clone())
            .await
            .map_err(|err| format!("send A11 Registration Request: {err}"))?;
        match tokio::time::timeout(
            HRPD_A11_WAIT_TIMEOUT,
            endpoint.recv_message_verified(&mut buf, security),
        )
        .await
        {
            Ok(Ok((reply, peer))) => {
                received = Some((reply.into_message(), peer));
                break;
            }
            Ok(Err(err)) => return Err(format!("receive A11 Registration Reply: {err}")),
            Err(_) => {
                if attempt == HRPD_A11_MAX_ATTEMPTS {
                    return Err(format!(
                        "timed out waiting for A11 Registration Reply from {} after {} attempts",
                        pcf_config.a11.peer_addr, HRPD_A11_MAX_ATTEMPTS
                    ));
                }
            }
        }
    }
    let (reply, peer) = received.ok_or_else(|| {
        format!(
            "timed out waiting for A11 Registration Reply from {}",
            pcf_config.a11.peer_addr
        )
    })?;
    if peer != pcf_config.a11.peer_addr {
        return Err(format!(
            "A11 Registration Reply came from unexpected peer {peer}, expected {}",
            pcf_config.a11.peer_addr
        ));
    }
    let cdma_a11::Message::RegistrationReply(reply_body) = &reply else {
        return Err(format!(
            "expected A11 Registration Reply, got {:?}",
            reply.message_type()
        ));
    };
    if reply_body.code != 0 {
        return Err(format!(
            "PDSN rejected A11 Registration Request with code {}",
            reply_body.code
        ));
    }
    procedures
        .apply(unix_seconds(), cdma_a11::Direction::Inbound, &reply)
        .map_err(|err| format!("apply inbound A11 Registration Reply: {err}"))?;
    manager
        .complete_a11_registration(session_id, key)
        .map_err(|err| format!("complete PCF A11 registration: {err}"))?;
    manager
        .bind_a10_bearer(
            session_id,
            cdma_a10::BearerSession::new(key.pcf_session_id, a10_endpoint),
        )
        .map_err(|err| format!("bind PCF A10 bearer: {err}"))?;
    Ok(key)
}

async fn refresh_hrpd_a11_session_after_a8_retarget(
    manager: &mut crate::PcfSessionManager,
    procedures: &mut cdma_a11::SessionProcedureTable,
    endpoint: &cdma_a11::UdpEndpoint,
    pcf_config: &crate::PcfNodeConfig,
    security: &cdma_a11::A11SecurityAssociation,
    session_id: PcfSessionId,
    setup: &cdma_a9::SetupA8Message,
    a10_endpoint: cdma_a10::BearerEndpoint,
) {
    if setup.imsi.is_none() {
        warn!(
            "HRPD PCF A11: cannot refresh retargeted session={session_id:?}; SetupA8 has no IMSI"
        );
        return;
    }

    match register_hrpd_a11_session(
        manager,
        procedures,
        endpoint,
        pcf_config,
        security,
        session_id,
        setup,
        a10_endpoint,
    )
    .await
    {
        Ok(a11_key) => info!(
            "HRPD PCF A11: refreshed retargeted session={session_id:?} key={a11_key:?}; A10 packet state is available for resumed traffic"
        ),
        Err(err) => {
            warn!("HRPD PCF A11: refresh failed for retargeted session={session_id:?}: {err}")
        }
    }
}

async fn deregister_hrpd_a11_session(
    manager: &mut crate::PcfSessionManager,
    procedures: &mut cdma_a11::SessionProcedureTable,
    endpoint: &cdma_a11::UdpEndpoint,
    pcf_config: &crate::PcfNodeConfig,
    security: &cdma_a11::A11SecurityAssociation,
    session_id: PcfSessionId,
    imsi: &str,
) -> Result<(), String> {
    let request = build_hrpd_a11_registration_request_for_imsi(
        session_id,
        imsi,
        pcf_config.a11,
        security,
        0,
    )?;
    manager
        .enqueue_a11(session_id, request)
        .map_err(|err| format!("queue A11 deregistration: {err}"))?;
    let request = manager
        .pop_pending_a11()
        .ok_or("A11 deregistration queue unexpectedly empty")?;
    procedures
        .apply(unix_seconds(), cdma_a11::Direction::Outbound, &request)
        .map_err(|err| format!("apply outbound A11 deregistration: {err}"))?;
    let mut buf = vec![0u8; 4096];
    let mut received = None;
    for attempt in 1..=HRPD_A11_MAX_ATTEMPTS {
        endpoint
            .send_message(pcf_config.a11.peer_addr, request.clone())
            .await
            .map_err(|err| format!("send A11 deregistration: {err}"))?;
        match tokio::time::timeout(
            HRPD_A11_WAIT_TIMEOUT,
            endpoint.recv_message_verified(&mut buf, security),
        )
        .await
        {
            Ok(Ok((reply, peer))) => {
                received = Some((reply.into_message(), peer));
                break;
            }
            Ok(Err(err)) => return Err(format!("receive A11 deregistration Reply: {err}")),
            Err(_) => {
                if attempt == HRPD_A11_MAX_ATTEMPTS {
                    return Err(format!(
                        "timed out waiting for A11 deregistration Reply from {} after {} attempts",
                        pcf_config.a11.peer_addr, HRPD_A11_MAX_ATTEMPTS
                    ));
                }
            }
        }
    }
    let (reply, peer) = received.ok_or_else(|| {
        format!(
            "timed out waiting for A11 deregistration Reply from {}",
            pcf_config.a11.peer_addr
        )
    })?;
    if peer != pcf_config.a11.peer_addr {
        return Err(format!(
            "A11 deregistration Reply came from unexpected peer {peer}, expected {}",
            pcf_config.a11.peer_addr
        ));
    }
    procedures
        .apply(unix_seconds(), cdma_a11::Direction::Inbound, &reply)
        .map_err(|err| format!("apply inbound A11 deregistration Reply: {err}"))?;
    info!("HRPD PCF A11: deregistered session={session_id:?}");
    Ok(())
}

async fn send_hrpd_a11_lifetime_zero_after_registration_update(
    endpoint: &cdma_a11::UdpEndpoint,
    pcf_config: &crate::PcfNodeConfig,
    security: &cdma_a11::A11SecurityAssociation,
    session_id: PcfSessionId,
    imsi: &str,
) -> Result<(), String> {
    let request = build_hrpd_a11_registration_request_for_imsi(
        session_id,
        imsi,
        pcf_config.a11,
        security,
        0,
    )?;
    let mut buf = vec![0u8; 4096];
    let mut received = None;
    for attempt in 1..=HRPD_A11_MAX_ATTEMPTS {
        endpoint
            .send_message(pcf_config.a11.peer_addr, request.clone())
            .await
            .map_err(|err| {
                format!("send A11 lifetime-zero request after Registration Update: {err}")
            })?;
        match tokio::time::timeout(
            HRPD_A11_WAIT_TIMEOUT,
            endpoint.recv_message_verified(&mut buf, security),
        )
        .await
        {
            Ok(Ok((reply, peer))) => {
                received = Some((reply.into_message(), peer));
                break;
            }
            Ok(Err(err)) => {
                return Err(format!(
                    "receive A11 lifetime-zero Reply after Registration Update: {err}"
                ));
            }
            Err(_) => {
                if attempt == HRPD_A11_MAX_ATTEMPTS {
                    return Err(format!(
                        "timed out waiting for A11 lifetime-zero Reply from {} after Registration Update after {} attempts",
                        pcf_config.a11.peer_addr, HRPD_A11_MAX_ATTEMPTS
                    ));
                }
            }
        }
    }
    let (reply, peer) = received.ok_or_else(|| {
        format!(
            "timed out waiting for A11 lifetime-zero Reply from {} after Registration Update",
            pcf_config.a11.peer_addr
        )
    })?;
    if peer != pcf_config.a11.peer_addr {
        return Err(format!(
            "A11 lifetime-zero Reply after Registration Update came from unexpected peer {peer}, expected {}",
            pcf_config.a11.peer_addr
        ));
    }
    let cdma_a11::Message::RegistrationReply(reply) = reply else {
        return Err(format!(
            "expected A11 lifetime-zero Registration Reply after Registration Update, got {:?}",
            reply.message_type()
        ));
    };
    if reply.lifetime != 0 || reply.code != 0 {
        return Err(format!(
            "PDSN rejected A11 lifetime-zero request after Registration Update code={} lifetime={}",
            reply.code, reply.lifetime
        ));
    }
    info!(
        "HRPD PCF A11: completed lifetime-zero release after Registration Update session={session_id:?}"
    );
    Ok(())
}

async fn send_hrpd_disconnect_a8(
    manager: &mut crate::PcfSessionManager,
    endpoint: &cdma_a9::UdpSignalingEndpoint,
    key: u32,
    connected: &mut ConnectedA8,
    reason: &str,
) -> Result<(), String> {
    let disconnect = cdma_a9::DisconnectA8Message {
        call_connection_reference: connected.connect.call_connection_reference,
        correlation_id: connected.connect.correlation_id,
        imsi: connected.connect.imsi.clone(),
        esn: connected.connect.esn,
        meid: connected.connect.meid,
        con_ref: connected.connect.con_ref,
        a8_traffic_id: connected.connect.a8_traffic_id.clone(),
        cause: cdma_a9::CauseValue(HRPD_A9_RELEASE_A8_CAUSE_PPP_SESSION_CLOSED_BY_MS),
    };
    if let Err(err) =
        manager.apply_outbound_a9(cdma_a9::ProcedureMessage::DisconnectA8(disconnect.clone()))
    {
        warn!(
            "HRPD PCF A9: DisconnectA8 procedure state rejected session={:?} key=0x{key:08x}: {err}",
            connected.session_id
        );
    }
    let payload = disconnect
        .encode()
        .map_err(|err| format!("HRPD PCF A9: encode DisconnectA8: {err}"))?;
    connected.metadata.sequence_no = connected.metadata.sequence_no.wrapping_add(1);
    send_a9_payload(
        endpoint,
        connected.peer,
        connected.metadata,
        payload,
        "HRPD PCF A9",
    )
    .await?;
    info!(
        "HRPD PCF A9: sent DisconnectA8 session={:?} key=0x{key:08x} peer={} reason={reason}",
        connected.session_id, connected.peer
    );
    Ok(())
}

struct HrpdPcfA9State {
    manager: crate::PcfSessionManager,
    a11_procedures: cdma_a11::SessionProcedureTable,
    connected: HashMap<u32, ConnectedA8>,
    a9_buf: Vec<u8>,
    a11_buf: Vec<u8>,
}

impl HrpdPcfA9State {
    fn new() -> Self {
        Self {
            manager: crate::PcfSessionManager::new(),
            a11_procedures: cdma_a11::SessionProcedureTable::new(),
            connected: HashMap::new(),
            a9_buf: vec![0u8; 4096],
            a11_buf: vec![0u8; 4096],
        }
    }
}

pub async fn spawn_hrpd_pcf_a9_service(
    pcf_config: crate::PcfNodeConfig,
) -> Result<HrpdA9ClientConfig, Error> {
    let (a8_local_ipv4, a8_peer_ipv4) =
        configured_a8_ipv4_pair(&pcf_config.a8_bearer, "pcf.a8_bearer")
            .map_err(|err| Error::from(format!("HRPD PCF A9 config: {err}")))?;
    let a8_endpoint = cdma_a8::BearerEndpoint::new(a8_local_ipv4, a8_peer_ipv4);
    let an_a8_bearer = inverted_udp_gre_bearer(pcf_config.a8_bearer, "an.a8_bearer")
        .map_err(|err| Error::from(format!("HRPD AN A8 config: {err}")))?;
    let an_a8_endpoint = cdma_a8::BearerEndpoint::new(a8_peer_ipv4, a8_local_ipv4);
    let (a10_local_ipv4, a10_peer_ipv4) =
        configured_a10_ipv4_pair(&pcf_config.a10_bearer, "pcf.a10_bearer")
            .map_err(|err| Error::from(format!("HRPD PCF A10 config: {err}")))?;
    let a10_endpoint = cdma_a10::BearerEndpoint::new(a10_local_ipv4, a10_peer_ipv4);
    let bearer_relay = spawn_hrpd_pcf_bearer_relay(
        pcf_config.a8_bearer,
        a8_endpoint,
        pcf_config.a10_bearer,
        a10_endpoint,
    )?;
    let requested_a11_bind_addr = pcf_config.a11.bind_addr;
    let a11_endpoint = cdma_a11::UdpEndpoint::bind(requested_a11_bind_addr)
        .await
        .map_err(|err| {
            Error::from(format!(
                "failed to bind HRPD PCF A11 {requested_a11_bind_addr}: {err}"
            ))
        })?;
    let a11_bind_addr = a11_endpoint
        .local_addr()
        .map_err(|err| Error::from(format!("failed to read HRPD PCF A11 local addr: {err}")))?;
    let a11_security = cdma_a11::A11SecurityAssociation::from_config(&pcf_config.a11_security)
        .map_err(|err| Error::from(format!("invalid HRPD PCF A11 security config: {err}")))?;
    let requested_a9_bind_addr = pcf_config.a9_bind_addr;
    let endpoint = cdma_a9::UdpSignalingEndpoint::bind(requested_a9_bind_addr)
        .await
        .map_err(|err| {
            Error::from(format!(
                "failed to bind HRPD PCF A9 {requested_a9_bind_addr}: {err}"
            ))
        })?;
    let a9_bind_addr = endpoint
        .local_addr()
        .map_err(|err| Error::from(format!("failed to read HRPD PCF A9 local addr: {err}")))?;
    let pcf_config_for_task = pcf_config.clone();
    tokio::spawn(async move {
        let mut state = HrpdPcfA9State::new();
        info!("HRPD PCF A9 signaling listener bound on {a9_bind_addr}");
        info!(
            "HRPD PCF A11 signaling endpoint bound on {a11_bind_addr}, peer {}",
            pcf_config_for_task.a11.peer_addr
        );
        loop {
            let (datagram, peer) = tokio::select! {
                a11 = a11_endpoint.recv_message_verified(&mut state.a11_buf, &a11_security) => {
                    let (message, peer) = match a11 {
                        Ok((message, peer)) => (message.into_message(), peer),
                        Err(err) => {
                            warn!("HRPD PCF A11: failed to receive message: {err}");
                            continue;
                        }
                    };
                    if peer != pcf_config_for_task.a11.peer_addr {
                        warn!(
                            "HRPD PCF A11: message came from unexpected peer {peer}, expected {}",
                            pcf_config_for_task.a11.peer_addr
                        );
                    }
                    let cdma_a11::Message::RegistrationUpdate(update) = message else {
                        warn!(
                            "HRPD PCF A11: ignoring unsupported message {:?} from {peer}",
                            message.message_type()
                        );
                        continue;
                    };
                    let key = cdma_a11::SessionKey::from_session(&update.session);
                    if let Err(err) = state.a11_procedures.apply(
                        unix_seconds(),
                        cdma_a11::Direction::Inbound,
                        &cdma_a11::Message::RegistrationUpdate(update.clone()),
                    ) {
                        warn!("HRPD PCF A11: Registration Update procedure rejected for {key:?}: {err}");
                    }
                    match build_hrpd_a11_registration_acknowledge(
                        &update,
                        &pcf_config_for_task,
                        &a11_security,
                    ) {
                        Ok(ack) => {
                            if let Err(err) = state.a11_procedures.apply(
                                unix_seconds(),
                                cdma_a11::Direction::Outbound,
                                &ack,
                            ) {
                                warn!("HRPD PCF A11: Registration Acknowledge procedure rejected for {key:?}: {err}");
                            }
                            if let Err(err) = a11_endpoint.send_message(peer, ack).await {
                                warn!("HRPD PCF A11: failed to send Registration Acknowledge for {key:?}: {err}");
                            }
                        }
                        Err(err) => warn!("HRPD PCF A11: cannot build Registration Acknowledge for {key:?}: {err}"),
                    }
                    let session_id = PcfSessionId(u64::from(update.session.pcf_session_id));
                    let connected_key = state.connected
                        .iter()
                        .find_map(|(a8_key, entry)| (entry.session_id == session_id).then_some(*a8_key));
                    let update_imsi = connected_key
                        .and_then(|a8_key| state.connected.get(&a8_key))
                        .and_then(|entry| entry.connect.imsi.clone());
                    if let Some(imsi) = update_imsi.as_deref()
                        && let Err(err) = send_hrpd_a11_lifetime_zero_after_registration_update(
                            &a11_endpoint,
                            &pcf_config_for_task,
                            &a11_security,
                            session_id,
                            imsi,
                        )
                        .await
                    {
                        warn!("HRPD PCF A11: deregistration failed after Registration Update session={session_id:?}: {err}");
                    }
                    if let Err(err) = state.manager.start_release(session_id) {
                        warn!("HRPD PCF A11: failed to mark session={session_id:?} releasing after Registration Update: {err}");
                    }
                    if let Some(a8_key) = connected_key {
                        bearer_relay.release(a8_key, Some(update.session.pcf_session_id));
                        if let Some(entry) = state.connected.get_mut(&a8_key)
                            && let Err(err) = send_hrpd_disconnect_a8(
                                &mut state.manager,
                                &endpoint,
                                a8_key,
                                entry,
                                "PDSN Registration Update",
                            )
                            .await
                        {
                            warn!("{err}");
                        }
                    } else {
                        warn!(
                            "HRPD PCF A11: Registration Update for {key:?} has no connected A8; removing PCF session state"
                        );
                        if let Err(err) = state.manager.remove_session(session_id) {
                            warn!("HRPD PCF A11: failed to remove session={session_id:?}: {err}");
                        }
                    }
                    continue;
                }
                a9 = endpoint.recv_datagram(&mut state.a9_buf) => {
                    match a9 {
                        Ok(value) => value,
                        Err(err) => {
                            warn!("HRPD PCF A9: failed to receive datagram: {err}");
                            continue;
                        }
                    }
                }
            };
            if datagram.message_type == cdma_a9::MessageType::ReleaseA8 {
                let release = match cdma_a9::ReleaseA8Message::decode(&datagram.payload) {
                    Ok(release) => release,
                    Err(err) => {
                        warn!("HRPD PCF A9: invalid ReleaseA8 from {peer}: {err}");
                        continue;
                    }
                };
                let key = release.a8_traffic_id.key;
                let Some(entry) = state.connected.remove(&key) else {
                    warn!("HRPD PCF A9: ReleaseA8 for unknown A8 key=0x{key:08x} from {peer}");
                    continue;
                };
                let session_id = entry.session_id;
                let connect = entry.connect;
                let was_releasing = state
                    .manager
                    .session(session_id)
                    .map(|session| matches!(session.phase, PcfSessionPhase::Releasing))
                    .unwrap_or(false);
                if let Err(err) = state
                    .manager
                    .apply_inbound_a9(cdma_a9::ProcedureMessage::ReleaseA8(release.clone()))
                {
                    warn!(
                        "HRPD PCF A9: ReleaseA8 procedure state rejected session={session_id:?} key=0x{key:08x}: {err}"
                    );
                }
                let complete = cdma_a9::ReleaseA8CompleteMessage {
                    call_connection_reference: release.call_connection_reference,
                    correlation_id: release.correlation_id,
                };
                if let Err(err) =
                    state
                        .manager
                        .apply_outbound_a9(cdma_a9::ProcedureMessage::ReleaseA8Complete(
                            complete.clone(),
                        ))
                {
                    warn!(
                        "HRPD PCF A9: ReleaseA8Complete procedure state rejected session={session_id:?} key=0x{key:08x}: {err}"
                    );
                }
                let payload = match complete.encode() {
                    Ok(payload) => payload,
                    Err(err) => {
                        warn!("HRPD PCF A9: failed to encode ReleaseA8Complete: {err}");
                        continue;
                    }
                };
                let metadata = cdma_a9::TransportMetadata {
                    flags: 0,
                    session_id: datagram.metadata.session_id,
                    sequence_no: datagram.metadata.sequence_no.wrapping_add(1),
                };
                if let Err(err) =
                    send_a9_payload(&endpoint, peer, metadata, payload, "HRPD PCF A9").await
                {
                    warn!("{err}");
                    continue;
                }
                if !was_releasing
                    && let Some(imsi) = connect.imsi.as_deref()
                    && let Err(err) = deregister_hrpd_a11_session(
                        &mut state.manager,
                        &mut state.a11_procedures,
                        &a11_endpoint,
                        &pcf_config_for_task,
                        &a11_security,
                        session_id,
                        imsi,
                    )
                    .await
                {
                    warn!(
                        "HRPD PCF A11: deregistration failed for session={session_id:?} key=0x{key:08x}: {err}"
                    );
                }
                bearer_relay.release(key, u32::try_from(session_id.0).ok());
                if let Err(err) = state.manager.start_release(session_id) {
                    warn!("HRPD PCF A9: failed to mark session={session_id:?} releasing: {err}");
                }
                if let Err(err) = state.manager.remove_session(session_id) {
                    warn!("HRPD PCF A9: failed to remove session={session_id:?}: {err}");
                }
                info!(
                    "HRPD PCF A9: released A8 session={session_id:?} key=0x{key:08x} peer={peer}"
                );
                continue;
            }
            if datagram.message_type != cdma_a9::MessageType::SetupA8 {
                warn!(
                    "HRPD PCF A9: ignoring unsupported message {:?} from {peer}",
                    datagram.message_type
                );
                continue;
            }
            let setup = match cdma_a9::SetupA8Message::decode(&datagram.payload) {
                Ok(setup) => setup,
                Err(err) => {
                    warn!("HRPD PCF A9: invalid SetupA8 from {peer}: {err}");
                    continue;
                }
            };
            if setup.service_option != cdma_a9::ServiceOptionValue::HIGH_RATE_PACKET_DATA {
                warn!(
                    "HRPD PCF A9: rejecting SetupA8 SO={} from {peer}; expected SO33",
                    setup.service_option.0
                );
                continue;
            }

            let key = setup.a8_traffic_id.key;
            if let Some(cached_entry) = state.connected.get(&key).cloned() {
                let session_id = cached_entry.session_id;
                let cached_connect = cached_entry.connect;
                let connect = if cached_connect.con_ref == setup.con_ref {
                    cached_connect.clone()
                } else {
                    let connect = cdma_a9::ConnectA8Message {
                        call_connection_reference: setup.call_connection_reference,
                        correlation_id: setup.correlation_id,
                        imsi: setup.imsi.clone(),
                        esn: setup.esn,
                        meid: setup.meid,
                        con_ref: setup.con_ref,
                        a8_traffic_id: setup.a8_traffic_id.clone(),
                        cause: cached_connect.cause,
                        pdsn_ip_address: cached_connect.pdsn_ip_address,
                    };
                    info!(
                        "HRPD PCF A9: retargeting cached ConnectA8 session={:?} key=0x{key:08x} con_ref {} -> {}",
                        session_id, cached_connect.con_ref.0, connect.con_ref.0
                    );
                    connect
                };
                if cached_connect.con_ref != setup.con_ref
                    && let Err(err) = state.manager.retarget_connected_a9(&setup, &connect)
                {
                    warn!(
                        "HRPD PCF A9: failed to retarget A9 procedure session={session_id:?} key=0x{key:08x}: {err}"
                    );
                    continue;
                }
                // A.S0008 couples each A8 connection to A10. Refresh A10 before
                // completing cached A8 setup so resumed packets cannot outrun PDSN state.
                refresh_hrpd_a11_session_after_a8_retarget(
                    &mut state.manager,
                    &mut state.a11_procedures,
                    &a11_endpoint,
                    &pcf_config_for_task,
                    &a11_security,
                    session_id,
                    &setup,
                    a10_endpoint,
                )
                .await;
                let payload = match connect.encode() {
                    Ok(payload) => payload,
                    Err(err) => {
                        warn!("HRPD PCF A9: cached ConnectA8 encode failed: {err}");
                        continue;
                    }
                };
                let metadata = cdma_a9::TransportMetadata {
                    flags: 0,
                    session_id: datagram.metadata.session_id,
                    sequence_no: datagram.metadata.sequence_no.wrapping_add(1),
                };
                state.connected.insert(
                    key,
                    ConnectedA8 {
                        session_id,
                        connect: connect.clone(),
                        peer,
                        metadata,
                    },
                );
                if let Err(err) =
                    send_a9_payload(&endpoint, peer, metadata, payload, "HRPD PCF A9").await
                {
                    state.connected.remove(&key);
                    warn!("{err}");
                } else {
                    info!(
                        "HRPD PCF A9: resent cached ConnectA8 session={:?} key=0x{key:08x}",
                        session_id
                    );
                }
                continue;
            }

            let mobile_identity = a9_mobile_identity_bytes(&setup);
            if let Err(err) = state
                .manager
                .apply_inbound_a9(cdma_a9::ProcedureMessage::SetupA8(setup.clone()))
            {
                if let Some(identity) = mobile_identity.as_ref()
                    && let Some(cached_session_id) =
                        state.manager.session_id_by_mobile_identity(identity)
                    && let Some(cached_entry) = state
                        .connected
                        .iter()
                        .find(|(_, entry)| entry.session_id == cached_session_id)
                        .map(|(_, entry)| entry.clone())
                    && let Ok(a10_session_id) = u32::try_from(cached_entry.session_id.0)
                {
                    let session_id = cached_entry.session_id;
                    let cached_connect = cached_entry.connect;
                    let bearer = cdma_a8::BearerSession::new(key, a8_endpoint);
                    bearer_relay.register(
                        bearer,
                        cdma_a10::BearerSession::new(a10_session_id, a10_endpoint),
                    );
                    let connect = cdma_a9::ConnectA8Message {
                        call_connection_reference: setup.call_connection_reference,
                        correlation_id: setup.correlation_id,
                        imsi: setup.imsi.clone(),
                        esn: setup.esn,
                        meid: setup.meid,
                        con_ref: setup.con_ref,
                        a8_traffic_id: setup.a8_traffic_id.clone(),
                        cause: cdma_a9::CauseValue(0x13),
                        pdsn_ip_address: cached_connect.pdsn_ip_address,
                    };
                    if let Err(retarget_err) = state.manager.retarget_same_mobile_connected_a9_from(
                        session_id,
                        &cached_connect.a8_traffic_id,
                        identity,
                        &setup,
                        &connect,
                    ) {
                        warn!(
                            "HRPD PCF A9: failed to retarget duplicate-mobile A9 procedure session={session_id:?} key=0x{key:08x}: {retarget_err}"
                        );
                        continue;
                    }
                    // Keep the duplicate-mobile rebind on the same A8-before-A10
                    // boundary as a normal A8 setup: PDSN state first, then ConnectA8.
                    refresh_hrpd_a11_session_after_a8_retarget(
                        &mut state.manager,
                        &mut state.a11_procedures,
                        &a11_endpoint,
                        &pcf_config_for_task,
                        &a11_security,
                        session_id,
                        &setup,
                        a10_endpoint,
                    )
                    .await;
                    let payload = match connect.encode() {
                        Ok(payload) => payload,
                        Err(err) => {
                            warn!("HRPD PCF A9: duplicate-mobile ConnectA8 encode failed: {err}");
                            continue;
                        }
                    };
                    let metadata = cdma_a9::TransportMetadata {
                        flags: 0,
                        session_id: datagram.metadata.session_id,
                        sequence_no: datagram.metadata.sequence_no.wrapping_add(1),
                    };
                    state.connected.insert(
                        key,
                        ConnectedA8 {
                            session_id,
                            connect: connect.clone(),
                            peer,
                            metadata,
                        },
                    );
                    if let Err(err) =
                        send_a9_payload(&endpoint, peer, metadata, payload, "HRPD PCF A9").await
                    {
                        state.connected.remove(&key);
                        warn!("{err}");
                    } else {
                        info!(
                            "HRPD PCF A9: rebound existing session={:?} to new A8 key=0x{key:08x} after duplicate SetupA8 for same mobile",
                            session_id
                        );
                    }
                    continue;
                }
                warn!("HRPD PCF A9: SetupA8 procedure rejected key=0x{key:08x}: {err}");
                continue;
            }
            let session_id = match state.manager.create_from_a9(mobile_identity.clone()) {
                Ok(PcfEvent::SessionCreated { id }) => id,
                Ok(event) => {
                    warn!("HRPD PCF A9: unexpected PCF event after SetupA8: {event:?}");
                    continue;
                }
                Err(err) => {
                    warn!("HRPD PCF A9: failed to create PCF session: {err}");
                    continue;
                }
            };
            let bearer = cdma_a8::BearerSession::new(key, a8_endpoint);
            if let Err(err) = state.manager.bind_a8_bearer(session_id, bearer) {
                warn!(
                    "HRPD PCF A9: failed to bind A8 bearer session={:?} key=0x{key:08x}: {err}",
                    session_id
                );
                continue;
            }
            let mut pdsn_ip_address = None;
            if setup.imsi.is_some() {
                let a10_session_id = match u32::try_from(session_id.0) {
                    Ok(session_id) if session_id != 0 => session_id,
                    Ok(_) => {
                        warn!("HRPD PCF A9: cannot pre-register A10 for zero session id");
                        continue;
                    }
                    Err(_) => {
                        warn!(
                            "HRPD PCF A9: cannot pre-register A10; session id {} exceeds u32",
                            session_id.0
                        );
                        continue;
                    }
                };
                bearer_relay.register(
                    bearer,
                    cdma_a10::BearerSession::new(a10_session_id, a10_endpoint),
                );
                match register_hrpd_a11_session(
                    &mut state.manager,
                    &mut state.a11_procedures,
                    &a11_endpoint,
                    &pcf_config_for_task,
                    &a11_security,
                    session_id,
                    &setup,
                    a10_endpoint,
                )
                .await
                {
                    Ok(a11_key) => {
                        pdsn_ip_address = socket_ipv4_octets(
                            pcf_config_for_task.a11.peer_addr,
                            "pcf.a11.peer_addr",
                        )
                        .ok()
                        .map(cdma_a9::PdsnIpAddress);
                        info!(
                            "HRPD PCF A11: registered session={:?} key={a11_key:?}; A10 bound {}.{}.{}.{} -> {}.{}.{}.{} key=0x{:08x}",
                            session_id,
                            a10_local_ipv4[0],
                            a10_local_ipv4[1],
                            a10_local_ipv4[2],
                            a10_local_ipv4[3],
                            a10_peer_ipv4[0],
                            a10_peer_ipv4[1],
                            a10_peer_ipv4[2],
                            a10_peer_ipv4[3],
                            a11_key.pcf_session_id
                        );
                    }
                    Err(err) => {
                        warn!(
                            "HRPD PCF A11: registration failed for session={:?} key=0x{key:08x}: {err}",
                            session_id
                        );
                    }
                }
            }
            let connect = cdma_a9::ConnectA8Message {
                call_connection_reference: setup.call_connection_reference,
                correlation_id: setup.correlation_id,
                imsi: setup.imsi.clone(),
                esn: setup.esn,
                meid: setup.meid,
                con_ref: setup.con_ref,
                a8_traffic_id: setup.a8_traffic_id.clone(),
                cause: cdma_a9::CauseValue(0x13),
                pdsn_ip_address,
            };
            if let Err(err) = state
                .manager
                .apply_outbound_a9(cdma_a9::ProcedureMessage::ConnectA8(connect.clone()))
            {
                warn!(
                    "HRPD PCF A9: failed to apply outbound ConnectA8 session={:?}: {err}",
                    session_id
                );
                continue;
            }
            let payload = match connect.encode() {
                Ok(payload) => payload,
                Err(err) => {
                    warn!("HRPD PCF A9: failed to encode ConnectA8: {err}");
                    continue;
                }
            };
            let metadata = cdma_a9::TransportMetadata {
                flags: 0,
                session_id: datagram.metadata.session_id,
                sequence_no: datagram.metadata.sequence_no.wrapping_add(1),
            };
            state.connected.insert(
                key,
                ConnectedA8 {
                    session_id,
                    connect: connect.clone(),
                    peer,
                    metadata,
                },
            );
            if let Err(err) =
                send_a9_payload(&endpoint, peer, metadata, payload, "HRPD PCF A9").await
            {
                state.connected.remove(&key);
                warn!("{err}");
                continue;
            }
            info!(
                "HRPD PCF A9: accepted SetupA8 session={:?} con_ref={} key=0x{key:08x} peer={peer}; A8 bound {}.{}.{}.{} -> {}.{}.{}.{}",
                session_id,
                setup.con_ref.0,
                a8_local_ipv4[0],
                a8_local_ipv4[1],
                a8_local_ipv4[2],
                a8_local_ipv4[3],
                a8_peer_ipv4[0],
                a8_peer_ipv4[1],
                a8_peer_ipv4[2],
                a8_peer_ipv4[3],
            );
            if setup.imsi.is_none() {
                warn!(
                    "HRPD PCF A9: A11 registration remains deferred for session={:?}; no real IMSI was present in SetupA8 (esn={:?} meid_present={})",
                    session_id,
                    setup.esn,
                    setup.meid.is_some()
                );
            }
        }
    });
    Ok(HrpdA9ClientConfig {
        pcf_addr: a9_bind_addr,
        a8_peer_ipv4,
        an_a8_bearer,
        an_a8_endpoint,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn imsi_to_a11_msid_bcd_matches_a11_session_format() {
        assert_eq!(
            imsi_to_a11_msid_bcd("2345678901").unwrap(),
            vec![0x20, 0x43, 0x65, 0x87, 0x09, 0xf1]
        );
        assert_eq!(
            imsi_to_a11_msid_bcd("310009176936269").unwrap(),
            vec![0x31, 0x01, 0x00, 0x19, 0x67, 0x39, 0x26, 0x96]
        );
    }
}
