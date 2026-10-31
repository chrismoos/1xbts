//! A12 Access-Request client for an HRPD access network.
//!
//! Sends the CHAP result from AN-stream access authentication to the AN-AAA
//! as a RADIUS Access-Request (RFC 2865) and returns Accept or Reject. The
//! Callback-Id of an Access-Accept holds the IMSI to use as the MN ID
//! (A.S0008-C §2.4.2, call flow §3.1.1 steps e-g).

use std::net::{IpAddr, SocketAddr};
use std::time::Duration;

use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};
use tokio::net::UdpSocket;

use crate::{
    ACCESS_ACCEPT, ACCESS_REJECT, ACCESS_REQUEST, AUTHENTICATOR_LEN, CALLBACK_ID, CHAP_CHALLENGE,
    CHAP_PASSWORD, HEADER_LEN, NAS_IP_ADDRESS, USER_NAME, VENDOR_3GPP2, VENDOR_SPECIFIC,
    VSA_HARDWARE_ID, constant_time_eq,
};

const RADIUS_MAX_PACKET: usize = 4096;
const ATTRIBUTE_MAX_VALUE: usize = 253;
/// Callback-Id is "0" followed by the IMSI.
const CALLBACK_ID_IMSI_PREFIX: char = '0';

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct A12ClientConfig {
    /// AN-AAA address, `host:port`.
    pub server: String,
    pub radius_secret: String,
    #[serde(default = "default_timeout_ms")]
    pub timeout_ms: u64,
    /// Retransmissions after the first attempt.
    #[serde(default = "default_retries")]
    pub retries: u8,
    /// NAS-IP-Address sent in each request. Omitted when unset.
    #[serde(default)]
    pub nas_ip: Option<std::net::Ipv4Addr>,
}

fn default_timeout_ms() -> u64 {
    3000
}

fn default_retries() -> u8 {
    1
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct A12AccessRequest {
    pub nai: String,
    pub chap_id: u8,
    pub challenge: Vec<u8>,
    pub response: Vec<u8>,
    /// Hardware ID octets for the 3GPP2 Hardware ID VSA: a 7-octet MEID or a
    /// 4-octet ESN.
    pub hardware_id: Option<Vec<u8>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum A12AccessResponse {
    Accept { imsi: Option<String> },
    Reject,
}

#[derive(Debug, thiserror::Error)]
pub enum A12ClientError {
    #[error("A12 AN-AAA address {0} is invalid")]
    InvalidServer(String),
    #[error("A12 radius_secret is empty")]
    MissingSecret,
    #[error("A12 attribute {0} is longer than 253 octets")]
    AttributeTooLong(&'static str),
    #[error("A12 socket error: {0}")]
    Io(#[from] std::io::Error),
    #[error("A12 AN-AAA did not answer after {attempts} attempts")]
    Timeout { attempts: u8 },
}

pub async fn send_access_request(
    config: &A12ClientConfig,
    request: &A12AccessRequest,
    identifier: u8,
) -> Result<A12AccessResponse, A12ClientError> {
    if config.radius_secret.is_empty() {
        return Err(A12ClientError::MissingSecret);
    }
    let server: SocketAddr = tokio::net::lookup_host(config.server.as_str())
        .await
        .ok()
        .and_then(|mut addrs| addrs.next())
        .ok_or_else(|| A12ClientError::InvalidServer(config.server.clone()))?;
    let mut authenticator = [0u8; AUTHENTICATOR_LEN];
    getrandom::getrandom(&mut authenticator)
        .map_err(|error| std::io::Error::other(error.to_string()))?;
    let packet = encode_access_request(config, request, identifier, &authenticator)?;
    let bind: SocketAddr = match server.ip() {
        IpAddr::V4(_) => "0.0.0.0:0".parse().expect("valid wildcard"),
        IpAddr::V6(_) => "[::]:0".parse().expect("valid wildcard"),
    };
    let socket = UdpSocket::bind(bind).await?;
    socket.connect(server).await?;
    let timeout = Duration::from_millis(config.timeout_ms);
    let attempts = config.retries.saturating_add(1);
    let mut buffer = [0u8; RADIUS_MAX_PACKET];
    for attempt in 1..=attempts {
        socket.send(&packet).await?;
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let received = tokio::time::timeout_at(deadline, socket.recv(&mut buffer)).await;
            let Ok(length) = received else {
                log::info!(
                    "A12 client: no answer from {server} attempt={attempt}/{attempts} id={identifier}"
                );
                break;
            };
            let length = length?;
            match decode_access_response(
                &buffer[..length],
                identifier,
                &authenticator,
                config.radius_secret.as_bytes(),
            ) {
                Some(response) => return Ok(response),
                None => log::warn!(
                    "A12 client: discarding unverified or mismatched reply from {server} len={length}"
                ),
            }
        }
    }
    Err(A12ClientError::Timeout { attempts })
}

fn push_attribute(
    packet: &mut Vec<u8>,
    kind: u8,
    value: &[u8],
    name: &'static str,
) -> Result<(), A12ClientError> {
    if value.len() > ATTRIBUTE_MAX_VALUE {
        return Err(A12ClientError::AttributeTooLong(name));
    }
    packet.push(kind);
    packet.push((value.len() + 2) as u8);
    packet.extend_from_slice(value);
    Ok(())
}

pub(crate) fn encode_access_request(
    config: &A12ClientConfig,
    request: &A12AccessRequest,
    identifier: u8,
    authenticator: &[u8; AUTHENTICATOR_LEN],
) -> Result<Vec<u8>, A12ClientError> {
    let mut packet = vec![ACCESS_REQUEST, identifier, 0, 0];
    packet.extend_from_slice(authenticator);
    push_attribute(&mut packet, USER_NAME, request.nai.as_bytes(), "User-Name")?;
    let mut chap_password = Vec::with_capacity(1 + request.response.len());
    chap_password.push(request.chap_id);
    chap_password.extend_from_slice(&request.response);
    push_attribute(&mut packet, CHAP_PASSWORD, &chap_password, "CHAP-Password")?;
    push_attribute(
        &mut packet,
        CHAP_CHALLENGE,
        &request.challenge,
        "CHAP-Challenge",
    )?;
    if let Some(nas_ip) = config.nas_ip {
        push_attribute(
            &mut packet,
            NAS_IP_ADDRESS,
            &nas_ip.octets(),
            "NAS-IP-Address",
        )?;
    }
    if let Some(hardware_id) = &request.hardware_id {
        let mut vsa = VENDOR_3GPP2.to_be_bytes().to_vec();
        vsa.push(VSA_HARDWARE_ID);
        vsa.push((hardware_id.len() + 2) as u8);
        vsa.extend_from_slice(hardware_id);
        push_attribute(&mut packet, VENDOR_SPECIFIC, &vsa, "Hardware-ID")?;
    }
    let length = packet.len() as u16;
    packet[2..4].copy_from_slice(&length.to_be_bytes());
    Ok(packet)
}

pub(crate) fn decode_access_response(
    wire: &[u8],
    identifier: u8,
    request_authenticator: &[u8; AUTHENTICATOR_LEN],
    secret: &[u8],
) -> Option<A12AccessResponse> {
    if wire.len() < HEADER_LEN || wire[1] != identifier {
        return None;
    }
    let length = usize::from(u16::from_be_bytes([wire[2], wire[3]]));
    if length < HEADER_LEN || length > wire.len() {
        return None;
    }
    let wire = &wire[..length];
    let mut md5 = Md5::new();
    md5.update(&wire[..4]);
    md5.update(request_authenticator);
    md5.update(&wire[HEADER_LEN..]);
    md5.update(secret);
    if !constant_time_eq(&md5.finalize(), &wire[4..HEADER_LEN]) {
        return None;
    }
    match wire[0] {
        ACCESS_ACCEPT => Some(A12AccessResponse::Accept {
            imsi: callback_imsi(&wire[HEADER_LEN..]),
        }),
        ACCESS_REJECT => Some(A12AccessResponse::Reject),
        _ => None,
    }
}

fn callback_imsi(mut attributes: &[u8]) -> Option<String> {
    while attributes.len() >= 2 {
        let length = usize::from(attributes[1]);
        if length < 2 || length > attributes.len() {
            return None;
        }
        if attributes[0] == CALLBACK_ID {
            let value = std::str::from_utf8(&attributes[2..length]).ok()?;
            let imsi = value.strip_prefix(CALLBACK_ID_IMSI_PREFIX)?;
            return (!imsi.is_empty() && imsi.bytes().all(|byte| byte.is_ascii_digit()))
                .then(|| imsi.to_string());
        }
        attributes = &attributes[length..];
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{A12Config, A12Subscriber, handle_access_request};

    const SECRET: &str = "radius-secret";
    const PASSWORD: &str = "at-password";

    fn client_config() -> A12ClientConfig {
        A12ClientConfig {
            server: "127.0.0.1:1812".into(),
            radius_secret: SECRET.into(),
            timeout_ms: 100,
            retries: 0,
            nas_ip: Some("10.0.0.1".parse().unwrap()),
        }
    }

    fn request(password: &str) -> A12AccessRequest {
        let challenge = vec![0x42; 16];
        let mut md5 = Md5::new();
        md5.update([9u8]);
        md5.update(password.as_bytes());
        md5.update(&challenge);
        A12AccessRequest {
            nai: "a0000025f07973@hrpd.example".into(),
            chap_id: 9,
            challenge,
            response: md5.finalize().to_vec(),
            hardware_id: Some(vec![0xa0, 0x00, 0x00, 0x25, 0xf0, 0x79, 0x73]),
        }
    }

    fn server_config(validate: bool) -> A12Config {
        A12Config {
            enabled: true,
            listen: "127.0.0.1:1812".into(),
            radius_secret: SECRET.into(),
            accept_unverified_chap: false,
            validate_subscribers: validate,
            subscribers: vec![A12Subscriber {
                nai: "a0000025f07973@hrpd.example".into(),
                imsi: "310550000000042".into(),
                hardware_id: Some("a0000025f07973".into()),
                chap_password: Some(PASSWORD.into()),
            }],
        }
    }

    fn round_trip(password: &str, validate: bool) -> Option<A12AccessResponse> {
        let authenticator = [0x11; AUTHENTICATOR_LEN];
        let wire =
            encode_access_request(&client_config(), &request(password), 5, &authenticator).unwrap();
        let result = handle_access_request(&server_config(validate), &wire, None).unwrap();
        decode_access_response(&result.response, 5, &authenticator, SECRET.as_bytes())
    }

    #[test]
    fn server_accepts_a_valid_chap_response_and_returns_the_imsi() {
        assert_eq!(
            round_trip(PASSWORD, true),
            Some(A12AccessResponse::Accept {
                imsi: Some("310550000000042".into())
            })
        );
    }

    #[test]
    fn server_rejects_a_wrong_password() {
        assert_eq!(round_trip("wrong", true), Some(A12AccessResponse::Reject));
    }

    #[test]
    fn reply_with_a_bad_authenticator_is_discarded() {
        let authenticator = [0x11; AUTHENTICATOR_LEN];
        let wire =
            encode_access_request(&client_config(), &request(PASSWORD), 5, &authenticator).unwrap();
        let result = handle_access_request(&server_config(true), &wire, None).unwrap();
        assert_eq!(
            decode_access_response(&result.response, 5, &authenticator, b"other-secret"),
            None
        );
        assert_eq!(
            decode_access_response(&result.response, 6, &authenticator, SECRET.as_bytes()),
            None
        );
    }

    #[test]
    fn request_carries_the_hardware_id_vsa() {
        let wire = encode_access_request(
            &client_config(),
            &request(PASSWORD),
            5,
            &[0; AUTHENTICATOR_LEN],
        )
        .unwrap();
        let vsa = [
            VENDOR_SPECIFIC,
            15,
            0x00,
            0x00,
            0x15,
            0x9f,
            VSA_HARDWARE_ID,
            9,
            0xa0,
            0x00,
            0x00,
            0x25,
            0xf0,
            0x79,
            0x73,
        ];
        assert!(wire.windows(vsa.len()).any(|window| window == vsa));
    }
}
