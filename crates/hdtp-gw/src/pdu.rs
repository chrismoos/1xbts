//! HDTP message framing and protocol data units (HDTP 1.1 draft, "Message
//! Formats").
//!
//! A message is a header, a body of one or more PDUs, and a trailer. Messages
//! to the server carry a **long header** (4-byte server SessionId); messages to
//! the client carry a **short header** (1-byte client SessionId). Every PDU
//! begins with a one-byte RequestId and a byte splitting into a NotLast flag
//! (bit 7) and a 7-bit Type. Session 0 (creation/meta) is unencrypted and its
//! trailer has length zero, which is the only ciphering this gateway needs.

use crate::cipher::Cipher;
use crate::header::Headers;

/// PDU type numbers (HDTP 1.1 draft, Table A-3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum PduType {
    SessionRequest = 1,
    SessionReply = 2,
    Get = 3,
    Reply = 4,
    GetNotification = 5,
    Error = 6,
    Post = 7,
    HoldOn = 8,
    Cancel = 9,
    Signal = 10,
    Ack = 11,
    SessionComplete = 13,
    Redirect = 14,
    Options = 16,
    Head = 17,
    Put = 18,
    Delete = 19,
}

impl PduType {
    pub fn from_u8(v: u8) -> Option<PduType> {
        use PduType::*;
        Some(match v {
            1 => SessionRequest,
            2 => SessionReply,
            3 => Get,
            4 => Reply,
            5 => GetNotification,
            6 => Error,
            7 => Post,
            8 => HoldOn,
            9 => Cancel,
            10 => Signal,
            11 => Ack,
            13 => SessionComplete,
            14 => Redirect,
            16 => Options,
            17 => Head,
            18 => Put,
            19 => Delete,
            _ => return None,
        })
    }
}

/// The special session used for session creation and meta-functions. It is
/// unencrypted and unauthenticated and its trailer is empty.
pub const SESSION_META: u32 = 0;

const NOT_LAST_BIT: u8 = 0x80;
const TYPE_MASK: u8 = 0x7f;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PduError {
    #[error("datagram too short: need {need} bytes, have {have}")]
    Short { need: usize, have: usize },
    #[error("unknown PDU type {0}")]
    UnknownType(u8),
    #[error("malformed {pdu} PDU")]
    Malformed { pdu: &'static str },
    #[error(transparent)]
    Cipher(#[from] crate::cipher::CipherError),
}

fn be16(b: &[u8], at: usize) -> Result<u16, PduError> {
    b.get(at..at + 2)
        .map(|s| u16::from_be_bytes([s[0], s[1]]))
        .ok_or(PduError::Short {
            need: at + 2,
            have: b.len(),
        })
}

fn be32(b: &[u8], at: usize) -> Result<u32, PduError> {
    b.get(at..at + 4)
        .map(|s| u32::from_be_bytes([s[0], s[1], s[2], s[3]]))
        .ok_or(PduError::Short {
            need: at + 4,
            have: b.len(),
        })
}

/// A parsed message from the handset (long header + one PDU).
///
/// Only the first PDU is parsed; HDTP permits piggybacking additional PDUs
/// after a NotLast flag, which the handset does not use for the request path.
#[derive(Debug, Clone)]
pub struct ClientMessage {
    pub session_id: u32,
    pub request_id: u8,
    pub not_last: bool,
    pub pdu: ClientPdu,
}

#[derive(Debug, Clone)]
pub enum ClientPdu {
    SessionRequest(SessionRequest),
    Get(Get),
    Post(Post),
    GetNotification,
    SessionComplete(SessionComplete),
    Ack,
    Cancel,
    /// A recognized type this gateway does not act on, kept for logging.
    Other {
        ty: PduType,
        body: Vec<u8>,
    },
}

impl ClientMessage {
    /// Parse a datagram carrying a long header. `cipher` decrypts the body for
    /// an encrypted user session; for session 0 and Cipher 0 it is the identity.
    pub fn decode(datagram: &[u8], cipher: Cipher) -> Result<ClientMessage, PduError> {
        if datagram.len() < 6 {
            return Err(PduError::Short {
                need: 6,
                have: datagram.len(),
            });
        }
        let session_id = be32(datagram, 0)?;
        let request_id = datagram[4];
        let type_byte = datagram[5];
        let not_last = type_byte & NOT_LAST_BIT != 0;
        let ty = PduType::from_u8(type_byte & TYPE_MASK)
            .ok_or(PduError::UnknownType(type_byte & TYPE_MASK))?;

        // The PDU contents run to the end of the datagram minus the cipher
        // trailer. Cipher 0 has no trailer.
        let end = datagram.len() - cipher.trailer_len();
        let mut body = datagram[6..end].to_vec();
        cipher.decrypt(&mut body)?;

        let pdu = match ty {
            PduType::SessionRequest => ClientPdu::SessionRequest(SessionRequest::decode(&body)?),
            PduType::Get => ClientPdu::Get(Get::decode(&body)?),
            PduType::Options => ClientPdu::Get(Get::decode(&body)?),
            PduType::Head => ClientPdu::Get(Get::decode(&body)?),
            PduType::Post => ClientPdu::Post(Post::decode(&body)?),
            PduType::GetNotification => ClientPdu::GetNotification,
            PduType::SessionComplete => ClientPdu::SessionComplete(SessionComplete::decode(&body)?),
            PduType::Ack => ClientPdu::Ack,
            PduType::Cancel => ClientPdu::Cancel,
            other => ClientPdu::Other { ty: other, body },
        };
        Ok(ClientMessage {
            session_id,
            request_id,
            not_last,
            pdu,
        })
    }
}

/// SessionRequest (Type 1): the handset asks session 0 to create a session.
///
/// The 1997 draft fixes the field order Cipher, Version, ClientSessionId,
/// DeviceIdLen, HeadersLen, DeviceId, Headers, ClientNonce, EncryptionTrailer.
/// The deployed UP.Browser 3.1 datagram does not byte-align to that order in
/// the fixed fields between the PDU header and the header block, so decoding
/// recovers the three values the gateway needs without trusting those offsets:
///   * Cipher — the first two contents bytes (observed `00 00`, No encryption);
///   * ClientNonce — the last two contents bytes (plaintext under Cipher 0, no
///     trailer), used to answer the challenge with ClientNonce+1;
///   * Headers — located as the longest well-known-header run ending at the
///     nonce, which also yields the client's capability advertisement.
///
/// ClientSessionId is read at the draft offset and is confirmed on-air.
#[derive(Debug, Clone)]
pub struct SessionRequest {
    pub cipher: Cipher,
    pub version: u8,
    pub client_session_id: u8,
    pub client_nonce: u16,
    pub headers: Headers,
    pub raw: Vec<u8>,
}

impl SessionRequest {
    fn decode(body: &[u8]) -> Result<SessionRequest, PduError> {
        if body.len() < 4 {
            return Err(PduError::Malformed {
                pdu: "SessionRequest",
            });
        }
        let cipher = Cipher::from_bytes([body[0], body[1]]);
        let version = body[2];
        let client_session_id = body.get(3).copied().unwrap_or(0);
        // Under Cipher 0 there is no encryption trailer, so the ClientNonce is
        // the final two bytes in the clear.
        let nonce_at = body.len() - 2;
        let client_nonce = be16(body, nonce_at)?;
        let headers = find_header_run(&body[..nonce_at]);
        Ok(SessionRequest {
            cipher,
            version,
            client_session_id,
            client_nonce,
            headers,
            raw: body.to_vec(),
        })
    }
}

/// Locate the handset's capability header block within the SessionRequest
/// contents. The run begins at the earliest single-octet well-known key
/// (`>= 0x80`) and is consumed greedily as `key 0x00 value 0x00` pairs, so it
/// tolerates the fixed contents fields ahead of it and any padding after it
/// (the capture carries a stray `0x00` between the last header and the nonce).
fn find_header_run(region: &[u8]) -> Headers {
    for start in 0..region.len() {
        if region[start] < 0x80 {
            continue;
        }
        let run = greedy_well_known_headers(&region[start..]);
        if !run.is_empty() {
            return run;
        }
    }
    Headers::new()
}

/// Consume consecutive well-known-key header pairs, stopping at the first byte
/// that does not begin one (a non-well-known key octet, missing terminator, or
/// padding).
fn greedy_well_known_headers(buf: &[u8]) -> Headers {
    let mut headers = Headers::new();
    let mut i = 0;
    while i + 1 < buf.len() {
        let key_code = buf[i];
        if key_code < 0x80 || buf[i + 1] != 0 {
            break;
        }
        i += 2;
        let Some(rel) = buf[i..].iter().position(|&b| b == 0) else {
            break;
        };
        let value = String::from_utf8_lossy(&buf[i..i + rel]).into_owned();
        i += rel + 1;
        let name = crate::header::well_known_key_name(key_code)
            .map(str::to_owned)
            .unwrap_or_else(|| format!("0x{key_code:02x}"));
        headers.push(name, value);
    }
    headers
}

/// Get (Type 3): request the resource at a URL. Options and Head share the
/// layout. (HDTP 1.1 draft, Table 2-8.)
#[derive(Debug, Clone)]
pub struct Get {
    pub headers: Headers,
    pub url: String,
}

impl Get {
    fn decode(body: &[u8]) -> Result<Get, PduError> {
        // On the wire the length fields are UrlLen then HeadersLen (confirmed
        // from live captures), and the headers block precedes the URL.
        let url_len = be16(body, 0)? as usize;
        let headers_len = be16(body, 2)? as usize;
        let hstart = 4;
        let ustart = hstart + headers_len;
        let uend = ustart + url_len;
        if body.len() < uend {
            return Err(PduError::Short {
                need: uend,
                have: body.len(),
            });
        }
        let headers = Headers::decode(&body[hstart..ustart]);
        let url = String::from_utf8_lossy(&body[ustart..uend]).into_owned();
        Ok(Get { headers, url })
    }
}

/// Post (Type 7): like Get with an enclosed entity.
#[derive(Debug, Clone)]
pub struct Post {
    pub headers: Headers,
    pub url: String,
    pub data: Vec<u8>,
}

impl Post {
    fn decode(body: &[u8]) -> Result<Post, PduError> {
        // UrlLen leads, matching the observed Get layout; the DataLen field and
        // the headers/url/data ordering are inferred from Get (no Post capture
        // yet).
        let url_len = be16(body, 0)? as usize;
        let headers_len = be16(body, 2)? as usize;
        let data_len = be16(body, 4)? as usize;
        let hstart = 6;
        let ustart = hstart + headers_len;
        let dstart = ustart + url_len;
        let dend = dstart + data_len;
        if body.len() < dend {
            return Err(PduError::Short {
                need: dend,
                have: body.len(),
            });
        }
        let headers = Headers::decode(&body[hstart..ustart]);
        let url = String::from_utf8_lossy(&body[ustart..dstart]).into_owned();
        Ok(Post {
            headers,
            url,
            data: body[dstart..dend].to_vec(),
        })
    }
}

/// SessionComplete (Type 13): the handset's acknowledgment closing the
/// three-way session-creation handshake, echoing ServerNonce+1.
#[derive(Debug, Clone)]
pub struct SessionComplete {
    pub server_nonce_plus: u16,
}

impl SessionComplete {
    fn decode(body: &[u8]) -> Result<SessionComplete, PduError> {
        // RID(1) then the incremented server nonce; tolerate a bare nonce too.
        let server_nonce_plus = if body.len() >= 3 {
            be16(body, 1)?
        } else {
            be16(body, 0)?
        };
        Ok(SessionComplete { server_nonce_plus })
    }
}

/// A message the gateway sends to the handset (short header + one PDU).
#[derive(Debug, Clone)]
pub struct ServerMessage {
    pub client_session_id: u8,
    pub request_id: u8,
    pub not_last: bool,
    pub pdu: ServerPdu,
}

#[derive(Debug, Clone)]
pub enum ServerPdu {
    SessionReply(SessionReply),
    Reply(Reply),
    Error(ErrorPdu),
    Redirect(Redirect),
    /// A PDU whose type byte and body are set explicitly, for wire forms not
    /// yet fully pinned down (e.g. the crypto-ignition KeyReply).
    Raw {
        ty: u8,
        body: Vec<u8>,
    },
}

impl ServerPdu {
    fn type_num(&self) -> u8 {
        match self {
            ServerPdu::SessionReply(_) => PduType::SessionReply as u8,
            ServerPdu::Reply(_) => PduType::Reply as u8,
            ServerPdu::Error(_) => PduType::Error as u8,
            ServerPdu::Redirect(_) => PduType::Redirect as u8,
            ServerPdu::Raw { ty, .. } => *ty,
        }
    }

    fn encode_body(&self) -> Vec<u8> {
        match self {
            ServerPdu::SessionReply(p) => p.encode_body(),
            ServerPdu::Reply(p) => p.encode_body(),
            ServerPdu::Error(p) => p.encode_body(),
            ServerPdu::Redirect(p) => p.encode_body(),
            ServerPdu::Raw { body, .. } => body.clone(),
        }
    }
}

impl ServerMessage {
    /// Encode to a datagram. `cipher` encrypts the body and its trailer; Cipher
    /// 0 is the identity with an empty trailer.
    pub fn encode(&self, cipher: Cipher) -> Vec<u8> {
        let mut out = Vec::new();
        out.push(self.client_session_id);
        out.push(self.request_id);
        let mut type_byte = self.pdu.type_num();
        if self.not_last {
            type_byte |= NOT_LAST_BIT;
        }
        out.push(type_byte);
        let mut body = self.pdu.encode_body();
        // Cipher 0 leaves the body unchanged and appends no trailer.
        let _ = cipher.encrypt(&mut body);
        out.extend_from_slice(&body);
        out
    }
}

/// Which byte layout to encode a SessionReply in.
///
/// The 1997 HDTP 1.1 draft and the earlier SUGP that UP.Browser 3.1 speaks order
/// the SessionReply fields differently, and the handset only accepts its own.
/// The gateway can emit either (or both) while the correct 3.1 layout is pinned
/// down on-air.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum SessionReplyLayout {
    /// HDTP 1.1 draft, Table 2-14: ServerNonce, ClientNonce+1, ServerSessionId,
    /// Cipher, SessionKeyLen, HeadersLen, SessionKey, Headers.
    Hdtp11,
    /// SUGP "SP" field order (Phone.com patents US6065120/US6263437):
    /// ServerSessionId, SessionKeyLen, SessionKey, S-nonce, ClientNonce
    /// derivative, Cipher. The C-SID that routes the reply is the short header.
    Sugp,
}

/// SessionReply (Type 2): the server's half of session creation.
#[derive(Debug, Clone)]
pub struct SessionReply {
    pub layout: SessionReplyLayout,
    pub server_nonce: u16,
    pub client_nonce_plus: u16,
    pub server_session_id: u32,
    pub cipher: Cipher,
    pub session_key: Vec<u8>,
    pub headers: Headers,
}

impl SessionReply {
    pub fn encode_body(&self) -> Vec<u8> {
        match self.layout {
            SessionReplyLayout::Hdtp11 => self.encode_hdtp11(),
            SessionReplyLayout::Sugp => self.encode_sugp(),
        }
    }

    fn encode_hdtp11(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&self.server_nonce.to_be_bytes());
        out.extend_from_slice(&self.client_nonce_plus.to_be_bytes());
        out.extend_from_slice(&self.server_session_id.to_be_bytes());
        out.extend_from_slice(&self.cipher.to_bytes());
        out.push(self.session_key.len() as u8);
        let hbytes = self.headers.encode();
        out.extend_from_slice(&(hbytes.len() as u16).to_be_bytes());
        out.extend_from_slice(&self.session_key);
        out.extend_from_slice(&hbytes);
        out
    }

    fn encode_sugp(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&self.server_session_id.to_be_bytes());
        out.push(self.session_key.len() as u8);
        out.extend_from_slice(&self.session_key);
        out.extend_from_slice(&self.server_nonce.to_be_bytes());
        out.extend_from_slice(&self.client_nonce_plus.to_be_bytes());
        out.extend_from_slice(&self.cipher.to_bytes());
        out
    }
}

/// Reply (Type 4): content returned for a Get or Post.
#[derive(Debug, Clone)]
pub struct Reply {
    pub headers: Headers,
    pub data: Vec<u8>,
}

impl Reply {
    pub fn encode_body(&self) -> Vec<u8> {
        let mut out = Vec::new();
        let hbytes = self.headers.encode();
        out.extend_from_slice(&(hbytes.len() as u16).to_be_bytes());
        out.extend_from_slice(&(self.data.len() as u16).to_be_bytes());
        out.extend_from_slice(&hbytes);
        out.extend_from_slice(&self.data);
        out
    }
}

/// Error codes (HDTP 1.1 draft, Table A-7).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u16)]
pub enum ErrorCode {
    Device = 1,
    Key = 2,
    Session = 3,
    Transaction = 4,
}

/// Error (Type 6): a request could not be serviced.
#[derive(Debug, Clone)]
pub struct ErrorPdu {
    pub tag: [u8; 4],
    pub code: u16,
    pub headers: Headers,
    pub data: Vec<u8>,
}

impl ErrorPdu {
    fn encode_body(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&self.tag);
        out.extend_from_slice(&self.code.to_be_bytes());
        let hbytes = self.headers.encode();
        out.extend_from_slice(&(hbytes.len() as u16).to_be_bytes());
        out.extend_from_slice(&(self.data.len() as u16).to_be_bytes());
        out.extend_from_slice(&hbytes);
        out.extend_from_slice(&self.data);
        out
    }
}

/// Redirect (Type 14): send the SessionRequest to a different address.
#[derive(Debug, Clone)]
pub struct Redirect {
    pub tag: [u8; 4],
    pub addresses: Vec<u8>,
}

impl Redirect {
    fn encode_body(&self) -> Vec<u8> {
        let mut out = Vec::new();
        out.extend_from_slice(&self.tag);
        out.extend_from_slice(&(self.addresses.len() as u16).to_be_bytes());
        out.extend_from_slice(&self.addresses);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::header::CTYPE_X_HDML;

    /// The full 114-byte SessionRequest UDP payload from the live capture.
    const CAPTURE: &[u8] = &[
        0x00, 0x00, 0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x10, 0x23, 0x54, 0x53, 0x43, 0x41, 0x0f,
        0x00, 0x4e, 0x02, 0x00, 0x0c, 0x00, 0x00, 0x02, 0x04, 0x08, 0x04, 0x01, 0x07, 0x08, 0x09,
        0x07, 0x09, 0x91, 0x00, 0x33, 0x2e, 0x31, 0x2c, 0x32, 0x2e, 0x30, 0x00, 0x8c, 0x00, 0x00,
        0xa0, 0x00, 0x33, 0x00, 0x89, 0x00, 0x65, 0x6e, 0x2d, 0x75, 0x73, 0x00, 0x93, 0x00, 0x32,
        0x00, 0x94, 0x00, 0x35, 0x00, 0x95, 0x00, 0x31, 0x32, 0x2c, 0x33, 0x00, 0x96, 0x00, 0x31,
        0x32, 0x2c, 0x32, 0x00, 0x97, 0x00, 0x31, 0x00, 0x98, 0x00, 0x30, 0x00, 0x99, 0x00, 0x31,
        0x00, 0x9a, 0x00, 0x30, 0x00, 0x9b, 0x00, 0x31, 0x34, 0x39, 0x32, 0x00, 0x85, 0x00, 0x30,
        0x32, 0x00, 0x9c, 0x00, 0x31, 0x00, 0x00, 0x85, 0xe2,
    ];

    #[test]
    fn decodes_captured_session_request() {
        let msg = ClientMessage::decode(CAPTURE, Cipher::NONE).unwrap();
        assert_eq!(msg.session_id, SESSION_META);
        assert_eq!(msg.request_id, 0);
        let ClientPdu::SessionRequest(sr) = msg.pdu else {
            panic!("expected SessionRequest");
        };
        assert!(
            sr.cipher.is_none(),
            "handset proposes Cipher 0 (No encryption)"
        );
        assert_eq!(sr.client_nonce, 0x85e2);
        assert_eq!(sr.headers.0.len(), 15);
        assert_eq!(sr.headers.get("Accept-Language").unwrap(), "en-us");
    }

    fn sample_reply(layout: SessionReplyLayout) -> ServerMessage {
        ServerMessage {
            client_session_id: 0x23,
            request_id: 0,
            not_last: false,
            pdu: ServerPdu::SessionReply(SessionReply {
                layout,
                server_nonce: 0x1111,
                client_nonce_plus: 0x85e3,
                server_session_id: 0x00000010,
                cipher: Cipher::NONE,
                session_key: vec![],
                headers: Headers::new(),
            }),
        }
    }

    #[test]
    fn session_reply_hdtp11_layout() {
        let bytes = sample_reply(SessionReplyLayout::Hdtp11).encode(Cipher::NONE);
        // Short header client-session-id, RequestId, Type=2.
        assert_eq!(bytes[0], 0x23);
        assert_eq!(bytes[1], 0);
        assert_eq!(bytes[2], PduType::SessionReply as u8);
        // ServerNonce, ClientNonce+1, ServerSessionId.
        assert_eq!(&bytes[3..5], &[0x11, 0x11]);
        assert_eq!(&bytes[5..7], &[0x85, 0xe3]);
        assert_eq!(&bytes[7..11], &[0, 0, 0, 0x10]);
    }

    #[test]
    fn session_reply_sugp_layout() {
        let bytes = sample_reply(SessionReplyLayout::Sugp).encode(Cipher::NONE);
        assert_eq!(bytes[2], PduType::SessionReply as u8);
        // SUGP order: ServerSessionId, SessionKeyLen, S-nonce, ClientNonce+1, Cipher.
        assert_eq!(&bytes[3..7], &[0, 0, 0, 0x10]);
        assert_eq!(bytes[7], 0); // empty session key
        assert_eq!(&bytes[8..10], &[0x11, 0x11]);
        assert_eq!(&bytes[10..12], &[0x85, 0xe3]);
        assert_eq!(&bytes[12..14], &[0, 0]); // cipher 0
    }

    #[test]
    fn get_roundtrips() {
        let mut headers = Headers::new();
        headers.push("Accept", CTYPE_X_HDML);
        // Build a Get body in the wire order: UrlLen, HeadersLen, Headers, Url.
        let url = "http://example.com/";
        let hbytes = headers.encode();
        let mut body = Vec::new();
        body.extend_from_slice(&(url.len() as u16).to_be_bytes());
        body.extend_from_slice(&(hbytes.len() as u16).to_be_bytes());
        body.extend_from_slice(&hbytes);
        body.extend_from_slice(url.as_bytes());
        let get = Get::decode(&body).unwrap();
        assert_eq!(get.url, url);
        assert_eq!(get.headers.get("Accept").unwrap(), CTYPE_X_HDML);
    }

    #[test]
    fn decodes_captured_get_full_url() {
        // Real handset Get body: UrlLen(0x0014) HeadersLen(0x000a) then a
        // 10-byte header run and the 20-byte URL. Regression against reading the
        // URL length from the wrong field, which chopped the URL's front.
        let body = [
            0x00, 0x14, 0x00, 0x0a, 0x81, 0x00, 0x31, 0x00, 0x8b, 0x00, 0x30, 0x30, 0x30, 0x30,
            b'h', b't', b't', b'p', b':', b'/', b'/', b'f', b'r', b'o', b'g', b'f', b'i', b'n',
            b'd', b'.', b'c', b'o', b'm', b'/',
        ];
        let get = Get::decode(&body).unwrap();
        assert_eq!(get.url, "http://frogfind.com/");
    }

    #[test]
    fn reply_body_layout() {
        let mut headers = Headers::new();
        headers.push("Content-Type", CTYPE_X_HDML);
        let reply = Reply {
            headers,
            data: b"hi".to_vec(),
        };
        let body = reply.encode_body();
        let hbytes_len = u16::from_be_bytes([body[0], body[1]]) as usize;
        let data_len = u16::from_be_bytes([body[2], body[3]]) as usize;
        assert_eq!(data_len, 2);
        assert_eq!(&body[4 + hbytes_len..], b"hi");
    }
}
