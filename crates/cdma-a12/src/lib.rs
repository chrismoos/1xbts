pub mod client;

use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::{io, net::SocketAddr};
use tokio::net::UdpSocket;

use cdma_hlr::repository::HlrRepository;

const ACCESS_REQUEST: u8 = 1;
const ACCESS_ACCEPT: u8 = 2;
const ACCESS_REJECT: u8 = 3;
const HEADER_LEN: usize = 20;
const AUTHENTICATOR_LEN: usize = 16;
const USER_NAME: u8 = 1;
const CHAP_PASSWORD: u8 = 3;
const NAS_IP_ADDRESS: u8 = 4;
const CALLBACK_ID: u8 = 20;
const VENDOR_SPECIFIC: u8 = 26;
const CHAP_CHALLENGE: u8 = 60;
const VENDOR_3GPP2: u32 = 5535;
const VSA_HRPD_ACCESS_AUTH: u8 = 60;
const VSA_HARDWARE_ID: u8 = 61;
const ESN_HEX_DIGITS: usize = 8;
const MEID_HEX_DIGITS: usize = 14;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct A12Config {
    #[serde(default)]
    pub enabled: bool,
    #[serde(default = "default_a12_listen")]
    pub listen: String,
    #[serde(default)]
    pub radius_secret: String,
    #[serde(default)]
    pub accept_unverified_chap: bool,
    #[serde(default)]
    pub validate_subscribers: bool,
    #[serde(default)]
    pub subscribers: Vec<A12Subscriber>,
}

impl Default for A12Config {
    fn default() -> Self {
        Self {
            enabled: false,
            listen: default_a12_listen(),
            radius_secret: String::new(),
            accept_unverified_chap: false,
            validate_subscribers: false,
            subscribers: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct A12Subscriber {
    pub nai: String,
    pub imsi: String,
    #[serde(default)]
    pub hardware_id: Option<String>,
    #[serde(default)]
    pub chap_password: Option<String>,
}

fn default_a12_listen() -> String {
    "0.0.0.0:1812".to_string()
}

#[derive(Debug, thiserror::Error)]
pub enum A12Error {
    #[error("A12 RADIUS is enabled but radius_secret is empty")]
    MissingSecret,
    #[error("binding A12 RADIUS listener {addr}: {source}")]
    Bind { addr: SocketAddr, source: io::Error },
    #[error("receiving A12 RADIUS datagram: {0}")]
    Receive(io::Error),
}

pub struct A12Server {
    config: A12Config,
    hlr: Option<Arc<dyn HlrRepository>>,
}

impl A12Server {
    pub fn new(config: A12Config) -> Self {
        Self { config, hlr: None }
    }

    pub fn with_hlr(config: A12Config, hlr: Option<Arc<dyn HlrRepository>>) -> Self {
        Self { config, hlr }
    }

    async fn resolve_hlr_imsi(&self, wire: &[u8]) -> Option<String> {
        let hlr = self.hlr.as_ref()?;
        if wire.len() < HEADER_LEN {
            return None;
        }
        let attrs = parse_attributes(&wire[HEADER_LEN..]).ok()?;
        let (esn, meid) = match attrs.hardware_id.as_deref() {
            Some(id) if id.len() == ESN_HEX_DIGITS => (u32::from_str_radix(id, 16).ok(), None),
            Some(id) if id.len() == MEID_HEX_DIGITS => (None, Some(id.to_string())),
            _ => (None, attrs.nai.as_deref().and_then(nai_meid)),
        };
        if esn.is_none() && meid.is_none() {
            return None;
        }
        let label = meid
            .clone()
            .or_else(|| esn.map(|esn| format!("{esn:08x}")))
            .unwrap_or_default();
        match hlr
            .resolve_by_hrpd_hardware_identity(esn, meid.as_deref())
            .await
        {
            Ok(Some(resolved)) => resolved.primary_identity.and_then(|identity| identity.imsi),
            Ok(None) => {
                log::info!("A12 HLR has no subscriber for hardware ID {label}");
                None
            }
            Err(error) => {
                log::warn!("A12 HLR resolution for hardware ID {label} failed: {error}");
                None
            }
        }
    }

    pub async fn serve(&self, addr: SocketAddr) -> Result<(), A12Error> {
        if !self.config.enabled {
            std::future::pending::<()>().await;
            unreachable!();
        }
        if self.config.radius_secret.is_empty() {
            return Err(A12Error::MissingSecret);
        }
        if self.config.accept_unverified_chap {
            log::warn!("A12 RADIUS is accepting unverified CHAP");
        }
        log::info!(
            "A12 RADIUS subscriber validation is {}",
            if self.config.validate_subscribers {
                "on (only provisioned NAIs accepted)"
            } else {
                "off (any NAI accepted)"
            }
        );
        let socket = UdpSocket::bind(addr)
            .await
            .map_err(|source| A12Error::Bind { addr, source })?;
        log::info!("A12 RADIUS listening on {addr}");
        let mut buffer = [0u8; 4096];
        loop {
            let (length, peer) = socket
                .recv_from(&mut buffer)
                .await
                .map_err(A12Error::Receive)?;
            let hlr_imsi = self.resolve_hlr_imsi(&buffer[..length]).await;
            match handle_access_request(&self.config, &buffer[..length], hlr_imsi) {
                Ok(result) => {
                    log::info!(
                        "A12 RADIUS {} peer={} nai={} meid={} imsi={} hardware_id={} nas_ip={}",
                        if result.accepted { "accept" } else { "reject" },
                        peer,
                        result.nai.as_deref().unwrap_or("<missing>"),
                        result.meid.as_deref().unwrap_or("<none>"),
                        result.imsi.as_deref().unwrap_or("<none>"),
                        result.hardware_id.as_deref().unwrap_or("<missing>"),
                        result.nas_ip.as_deref().unwrap_or("<missing>")
                    );
                    if let Err(error) = socket.send_to(&result.response, peer).await {
                        log::warn!("A12 RADIUS response to {peer} failed: {error}");
                    }
                }
                Err(error) => log::warn!("A12 RADIUS malformed request from {peer}: {error}"),
            }
        }
    }
}

struct RadiusResult {
    accepted: bool,
    nai: Option<String>,
    hardware_id: Option<String>,
    nas_ip: Option<String>,
    meid: Option<String>,
    imsi: Option<String>,
    response: Vec<u8>,
}

#[derive(Debug, thiserror::Error)]
enum RadiusError {
    #[error("packet is shorter than the RADIUS header")]
    ShortHeader,
    #[error("invalid RADIUS packet length")]
    InvalidLength,
    #[error("unsupported RADIUS code {0}")]
    UnsupportedCode(u8),
    #[error("malformed RADIUS attribute")]
    MalformedAttribute,
}

#[derive(Default)]
struct RequestAttributes {
    nai: Option<String>,
    chap: Option<Vec<u8>>,
    challenge: Option<Vec<u8>>,
    hardware_id: Option<String>,
    nas_ip: Option<String>,
}

fn handle_access_request(
    config: &A12Config,
    wire: &[u8],
    hlr_imsi: Option<String>,
) -> Result<RadiusResult, RadiusError> {
    if wire.len() < HEADER_LEN {
        return Err(RadiusError::ShortHeader);
    }
    if wire[0] != ACCESS_REQUEST {
        return Err(RadiusError::UnsupportedCode(wire[0]));
    }
    let packet_len = u16::from_be_bytes([wire[2], wire[3]]) as usize;
    if packet_len != wire.len() || packet_len < HEADER_LEN {
        return Err(RadiusError::InvalidLength);
    }
    let attrs = parse_attributes(&wire[HEADER_LEN..])?;
    let meid = attrs.nai.as_deref().and_then(nai_meid);
    let subscriber = attrs
        .nai
        .as_deref()
        .and_then(|nai| config.subscribers.iter().find(|entry| entry.nai == nai));
    let accepted = if config.validate_subscribers {
        let identity_matches = subscriber.is_some_and(|entry| hardware_matches(entry, &attrs));
        let chap_valid = subscriber.is_some_and(|entry| verify_chap(entry, &attrs, &wire[4..20]));
        identity_matches && (chap_valid || config.accept_unverified_chap)
    } else {
        attrs.nai.is_some()
    };
    // Callback-Id is "0" followed by the IMSI.
    let callback = if accepted {
        hlr_imsi
            .map(|imsi| format!("0{imsi}"))
            .or_else(|| subscriber.map(|entry| format!("0{}", entry.imsi)))
    } else {
        None
    };
    let response = build_response(
        if accepted {
            ACCESS_ACCEPT
        } else {
            ACCESS_REJECT
        },
        wire[1],
        &wire[4..20],
        callback.as_deref(),
        config.radius_secret.as_bytes(),
    );
    Ok(RadiusResult {
        accepted,
        nai: attrs.nai,
        hardware_id: attrs.hardware_id,
        nas_ip: attrs.nas_ip,
        meid,
        imsi: callback,
        response,
    })
}

fn nai_meid(nai: &str) -> Option<String> {
    let user = nai.split('@').next()?;
    (user.len() == 14 && user.bytes().all(|byte| byte.is_ascii_hexdigit()))
        .then(|| user.to_ascii_lowercase())
}

fn parse_attributes(mut wire: &[u8]) -> Result<RequestAttributes, RadiusError> {
    let mut parsed = RequestAttributes::default();
    while !wire.is_empty() {
        if wire.len() < 2 {
            return Err(RadiusError::MalformedAttribute);
        }
        let length = wire[1] as usize;
        if length < 2 || length > wire.len() {
            return Err(RadiusError::MalformedAttribute);
        }
        let value = &wire[2..length];
        match wire[0] {
            USER_NAME => parsed.nai = Some(String::from_utf8_lossy(value).into_owned()),
            CHAP_PASSWORD => parsed.chap = Some(value.to_vec()),
            NAS_IP_ADDRESS if value.len() == 4 => {
                parsed.nas_ip = Some(format!(
                    "{}.{}.{}.{}",
                    value[0], value[1], value[2], value[3]
                ));
            }
            CHAP_CHALLENGE => parsed.challenge = Some(value.to_vec()),
            VENDOR_SPECIFIC => parse_vendor_attribute(value, &mut parsed)?,
            _ => {}
        }
        wire = &wire[length..];
    }
    Ok(parsed)
}

fn parse_vendor_attribute(value: &[u8], parsed: &mut RequestAttributes) -> Result<(), RadiusError> {
    if value.len() < 6 {
        return Err(RadiusError::MalformedAttribute);
    }
    let vendor = u32::from_be_bytes(value[..4].try_into().expect("four-byte vendor"));
    let vendor_len = value[5] as usize;
    if vendor_len < 2 || 4 + vendor_len > value.len() {
        return Err(RadiusError::MalformedAttribute);
    }
    if vendor == VENDOR_3GPP2 {
        let vendor_value = &value[6..4 + vendor_len];
        match value[4] {
            VSA_HARDWARE_ID => parsed.hardware_id = Some(format_hex(vendor_value)),
            VSA_HRPD_ACCESS_AUTH => {}
            _ => {}
        }
    }
    Ok(())
}

fn hardware_matches(subscriber: &A12Subscriber, request: &RequestAttributes) -> bool {
    subscriber.hardware_id.as_ref().is_none_or(|expected| {
        request
            .hardware_id
            .as_ref()
            .is_some_and(|actual| actual.eq_ignore_ascii_case(expected))
    })
}

fn verify_chap(
    subscriber: &A12Subscriber,
    request: &RequestAttributes,
    authenticator: &[u8],
) -> bool {
    let Some(chap) = request.chap.as_deref() else {
        return false;
    };
    if chap.len() != AUTHENTICATOR_LEN + 1 {
        return false;
    }
    let Some(password) = subscriber.chap_password.as_deref() else {
        return false;
    };
    let challenge = request.challenge.as_deref().unwrap_or(authenticator);
    let mut md5 = Md5::new();
    md5.update([chap[0]]);
    md5.update(password.as_bytes());
    md5.update(challenge);
    constant_time_eq(&md5.finalize(), &chap[1..])
}

fn build_response(
    code: u8,
    identifier: u8,
    request_authenticator: &[u8],
    callback_id: Option<&str>,
    secret: &[u8],
) -> Vec<u8> {
    let mut response = vec![code, identifier, 0, 0];
    response.extend_from_slice(&[0; AUTHENTICATOR_LEN]);
    if let Some(callback_id) = callback_id {
        response.push(CALLBACK_ID);
        response.push((callback_id.len() + 2) as u8);
        response.extend_from_slice(callback_id.as_bytes());
    }
    let length = response.len() as u16;
    response[2..4].copy_from_slice(&length.to_be_bytes());
    let mut md5 = Md5::new();
    md5.update(&response[..4]);
    md5.update(request_authenticator);
    md5.update(&response[HEADER_LEN..]);
    md5.update(secret);
    response[4..20].copy_from_slice(&md5.finalize());
    response
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    left.len() == right.len()
        && left
            .iter()
            .zip(right)
            .fold(0u8, |difference, (left, right)| difference | (left ^ right))
            == 0
}

fn format_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut output = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        output.push(HEX[(byte >> 4) as usize] as char);
        output.push(HEX[(byte & 0x0f) as usize] as char);
    }
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    const NAI: &str = "device@example.test";
    const IMSI: &str = "310001234567890";
    const PASSWORD: &str = "at-password";
    const RADIUS_SECRET: &str = "radius-secret";
    const REQUEST_AUTHENTICATOR: [u8; AUTHENTICATOR_LEN] = [0x5a; AUTHENTICATOR_LEN];
    const CHALLENGE: [u8; AUTHENTICATOR_LEN] = [0xa5; AUTHENTICATOR_LEN];

    fn config(accept_unverified_chap: bool) -> A12Config {
        A12Config {
            enabled: true,
            listen: "127.0.0.1:1812".to_string(),
            radius_secret: RADIUS_SECRET.to_string(),
            accept_unverified_chap,
            validate_subscribers: true,
            subscribers: vec![A12Subscriber {
                nai: NAI.to_string(),
                imsi: IMSI.to_string(),
                hardware_id: None,
                chap_password: Some(PASSWORD.to_string()),
            }],
        }
    }

    fn attribute(kind: u8, value: &[u8]) -> Vec<u8> {
        let mut attr = vec![kind, (value.len() + 2) as u8];
        attr.extend_from_slice(value);
        attr
    }

    fn access_request(password: &str) -> Vec<u8> {
        let chap_id = 7;
        let mut digest = Md5::new();
        digest.update([chap_id]);
        digest.update(password.as_bytes());
        digest.update(CHALLENGE);
        let mut chap = vec![chap_id];
        chap.extend_from_slice(&digest.finalize());

        let mut packet = vec![ACCESS_REQUEST, 9, 0, 0];
        packet.extend_from_slice(&REQUEST_AUTHENTICATOR);
        packet.extend(attribute(USER_NAME, NAI.as_bytes()));
        packet.extend(attribute(CHAP_PASSWORD, &chap));
        packet.extend(attribute(CHAP_CHALLENGE, &CHALLENGE));
        packet.extend(attribute(NAS_IP_ADDRESS, &[10, 4, 58, 140]));
        let length = packet.len() as u16;
        packet[2..4].copy_from_slice(&length.to_be_bytes());
        packet
    }

    #[test]
    fn valid_chap_returns_accept_with_callback_id_and_authenticator() {
        let result =
            handle_access_request(&config(false), &access_request(PASSWORD), None).unwrap();
        assert!(result.accepted);
        assert_eq!(result.response[0], ACCESS_ACCEPT);
        assert_eq!(result.response[HEADER_LEN], CALLBACK_ID);
        assert_eq!(
            &result.response[HEADER_LEN + 2..],
            format!("0{IMSI}").as_bytes()
        );

        let mut authenticated = result.response.clone();
        authenticated[4..20].copy_from_slice(&REQUEST_AUTHENTICATOR);
        let mut digest = Md5::new();
        digest.update(&authenticated);
        digest.update(RADIUS_SECRET.as_bytes());
        assert_eq!(&result.response[4..20], digest.finalize().as_slice());
    }

    #[test]
    fn strict_policy_rejects_bad_chap() {
        let result = handle_access_request(&config(false), &access_request("wrong"), None).unwrap();
        assert!(!result.accepted);
        assert_eq!(result.response[0], ACCESS_REJECT);
        assert_eq!(result.response.len(), HEADER_LEN);
    }

    #[test]
    fn temporary_policy_accepts_bad_chap_for_known_nai() {
        let result = handle_access_request(&config(true), &access_request("wrong"), None).unwrap();
        assert!(result.accepted);
        assert_eq!(result.response[0], ACCESS_ACCEPT);
    }

    #[test]
    fn open_policy_accepts_unprovisioned_nai_without_imsi() {
        let mut config = config(true);
        config.validate_subscribers = false;
        config.subscribers.clear();
        let result = handle_access_request(&config, &access_request(PASSWORD), None).unwrap();
        assert!(result.accepted);
        assert_eq!(result.response[0], ACCESS_ACCEPT);
        assert!(result.imsi.is_none());
        assert_eq!(result.response.len(), HEADER_LEN);
    }

    #[test]
    fn nai_meid_extracts_hex_user_part() {
        assert_eq!(
            nai_meid("A0000012AB34CD@hcm.sprintpcs.com").as_deref(),
            Some("a0000012ab34cd")
        );
        assert_eq!(nai_meid("not-a-meid@domain"), None);
    }

    #[test]
    fn malformed_attribute_is_rejected_without_a_response() {
        let mut packet = access_request(PASSWORD);
        packet.push(USER_NAME);
        packet.push(1);
        let length = packet.len() as u16;
        packet[2..4].copy_from_slice(&length.to_be_bytes());
        assert!(matches!(
            handle_access_request(&config(false), &packet, None),
            Err(RadiusError::MalformedAttribute)
        ));
    }

    #[test]
    fn parses_3gpp2_hardware_id() {
        let mut packet = access_request(PASSWORD);
        let mut vendor = VENDOR_3GPP2.to_be_bytes().to_vec();
        vendor.extend_from_slice(&[VSA_HARDWARE_ID, 5, 0x01, 0x02, 0x03]);
        packet.extend(attribute(VENDOR_SPECIFIC, &vendor));
        let length = packet.len() as u16;
        packet[2..4].copy_from_slice(&length.to_be_bytes());
        let result = handle_access_request(&config(false), &packet, None).unwrap();
        assert_eq!(result.hardware_id.as_deref(), Some("010203"));
    }
}
