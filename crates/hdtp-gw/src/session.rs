//! HDTP session management: the session-creation handshake and the table of
//! live sessions.
//!
//! Session creation is a three-way handshake (HDTP 1.1 draft, "Session
//! Creation"): the handset sends a SessionRequest to session 0 with a client
//! nonce; the gateway answers with a SessionReply carrying a server session id,
//! a server nonce, and ClientNonce+1; the handset closes with a SessionComplete
//! echoing ServerNonce+1. Under the negotiated Cipher 0 (No encryption) the
//! challenge/response is carried in the clear, so no shared secret is required.
//!
//! Sessions are keyed by the server session id the gateway assigns; that id is
//! what the handset places in the long header of every subsequent request.
//!
//! The leading byte of every server→client message is the SUGP **C-SID**, which
//! Phone.com patent US6148405 defines as an encryption-mode selector, not a
//! client session id: `0` = clear-text session, `1` = shared-secret-key
//! encrypted, `2` = session-key encrypted. Our sessions negotiate Cipher 0, so
//! replies carry C-SID `0`. (This corrected an earlier reading that treated the
//! byte as a routable client session id; the handset ignored replies addressed
//! to `0x23`/`0x10` because neither is a valid C-SID.)

use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

use crate::cipher::Cipher;
use crate::header::Headers;
use crate::pdu::{ServerMessage, ServerPdu, SessionReply, SessionReplyLayout, SessionRequest};

/// First server session id handed to a user session. Ids `0x00000001..=0x0F`
/// are reserved by the spec; `0` is the creation meta-session.
const FIRST_USER_SESSION_ID: u32 = 0x0000_0010;

/// SUGP C-SID for a clear-text (Cipher 0) session — the leading byte of every
/// server→client message (US6148405).
const CSID_CLEARTEXT: u8 = 0x00;

/// Lifecycle of a session as the gateway sees it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionState {
    /// SessionReply sent, awaiting the handset's SessionComplete.
    Proto,
    /// Handshake complete; the session carries requests.
    Active,
}

/// One handset session.
#[derive(Debug, Clone)]
pub struct Session {
    pub peer: SocketAddr,
    pub server_session_id: u32,
    /// Every client-session-id the reply is addressed to (see module docs).
    pub client_session_ids: Vec<u8>,
    pub cipher: Cipher,
    pub server_nonce: u16,
    pub client_nonce: u16,
    pub state: SessionState,
    pub current_request_id: u8,
    pub capabilities: Headers,
    /// The exact SessionRequest contents that created this session. A later
    /// SessionRequest with identical bytes is a retransmit and gets the same
    /// reply; one that differs is a new session creation and gets fresh server
    /// material.
    pub request_raw: Vec<u8>,
}

/// Which SessionRequest trailer bytes to treat as the C-nonce whose `+1` the
/// reply answers with.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NonceChoice {
    /// The last two contents bytes.
    Last,
    /// The two contents bytes before the last (C-nonce ahead of C-nonceModified).
    Prev,
    /// Answer both (sends one reply per reading).
    Both,
}

/// How to shape the SessionReply(s) while the exact 3.1 form is pinned down.
///
/// The handset treats several SessionReplies with differing derivatives for one
/// session as a replay/security fault and cancels, so the default is a single
/// reply. The layout and nonce reading are configurable so they can be swept
/// on-air without rebuilding.
#[derive(Debug, Clone)]
pub struct ReplyConfig {
    pub layouts: Vec<SessionReplyLayout>,
    pub nonce: NonceChoice,
    /// When set, answer a SessionRequest with an Error PDU of this code instead
    /// of a SessionReply. Code 2 (Key Error, invalid shared secret key) is the
    /// crypto-ignition trigger; used to probe whether the handset initiates a
    /// key exchange. `None` grants the session normally.
    pub cold_start_error_code: Option<u16>,
}

impl Default for ReplyConfig {
    fn default() -> Self {
        // The SessionRequest trailer is the pair `C-nonce, C-nonceModified`
        // where `C-nonceModified = C-nonce ⊕ 0x85e2` (a constant, observed across
        // sessions). The derivative the handset validates is `C-nonce + 1`, so
        // the reply must answer the *prev* two bytes (C-nonce), not the last two
        // (C-nonceModified).
        ReplyConfig {
            layouts: vec![SessionReplyLayout::Hdtp11],
            nonce: NonceChoice::Prev,
            cold_start_error_code: None,
        }
    }
}

/// Serialize the shared-secret table as `peer<TAB>hex` lines.
fn serialize_ssk_store(map: &HashMap<SocketAddr, Vec<u8>>) -> String {
    let mut out = String::new();
    for (peer, ssk) in map {
        let hex: String = ssk.iter().map(|b| format!("{b:02x}")).collect();
        out.push_str(&peer.to_string());
        out.push('\t');
        out.push_str(&hex);
        out.push('\n');
    }
    out
}

/// Load a shared-secret table written by [`serialize_ssk_store`]. A missing,
/// unreadable, or malformed store degrades to no restored keys (a cold start).
fn load_ssk_store(path: &Path) -> HashMap<SocketAddr, Vec<u8>> {
    let mut map = HashMap::new();
    let Ok(text) = std::fs::read_to_string(path) else {
        return map;
    };
    for line in text.lines() {
        let Some((peer, hex)) = line.split_once('\t') else {
            continue;
        };
        if let (Ok(peer), Some(ssk)) = (peer.parse::<SocketAddr>(), decode_hex(hex)) {
            map.insert(peer, ssk);
        }
    }
    map
}

fn decode_hex(s: &str) -> Option<Vec<u8>> {
    if !s.len().is_multiple_of(2) {
        return None;
    }
    (0..s.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(&s[i..i + 2], 16).ok())
        .collect()
}

/// Write the store atomically (temp file + rename) with owner-only permissions,
/// since it holds session secrets at rest.
fn write_ssk_store(path: &Path, text: &str) -> std::io::Result<()> {
    if let Some(parent) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
        std::fs::create_dir_all(parent)?;
    }
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, text)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&tmp, std::fs::Permissions::from_mode(0o600))?;
    }
    std::fs::rename(&tmp, path)
}

/// The gateway's session table plus the id/nonce allocators.
pub struct SessionManager {
    next_server_id: AtomicU32,
    nonce_counter: AtomicU32,
    sessions: Mutex<HashMap<u32, Session>>,
    /// Maps a peer to its current session so retransmitted SessionRequests
    /// reuse one server session id and nonce instead of churning new ones.
    by_peer: Mutex<HashMap<SocketAddr, u32>>,
    /// Shared secret established with a peer by the crypto-ignition key exchange,
    /// used to key the RC5 cipher for the encrypted (cipher 2) session.
    ssk_by_peer: Mutex<HashMap<SocketAddr, Vec<u8>>>,
    /// Optional file the `ssk_by_peer` map is persisted to, so an established
    /// encrypted session survives a gateway restart (the handset resumes with an
    /// encrypted request the gateway could otherwise no longer decrypt). Point it
    /// at a mounted volume in a container. Unset keeps the secrets in memory only.
    ssk_store: Option<PathBuf>,
    /// Fallback session key for a cipher-2 peer that never ran a key exchange
    /// (a handset whose default boot uses a cached constant key). Seeded only by
    /// the test-only [`Self::set_default_ssk`], which is compiled out of release
    /// builds, so a shipping gateway never keys a session from a preseeded key.
    #[cfg(any(test, feature = "test-harness"))]
    default_ssk: Mutex<Option<Vec<u8>>>,
    reply: ReplyConfig,
}

impl Default for SessionManager {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionManager {
    pub fn new() -> Self {
        Self::with_reply(ReplyConfig::default())
    }

    pub fn with_reply(reply: ReplyConfig) -> Self {
        Self::with_reply_and_store(reply, None)
    }

    /// Build a manager that persists established shared secrets to `ssk_store`
    /// (loading any existing ones on startup) so encrypted sessions survive a
    /// restart. `None` keeps them in memory only.
    pub fn with_reply_and_store(reply: ReplyConfig, ssk_store: Option<PathBuf>) -> Self {
        let ssk_by_peer = ssk_store.as_deref().map(load_ssk_store).unwrap_or_default();
        if let Some(path) = ssk_store.as_deref()
            && !ssk_by_peer.is_empty()
        {
            tracing::info!(sessions = ssk_by_peer.len(), path = %path.display(), "restored persisted session keys");
        }
        SessionManager {
            next_server_id: AtomicU32::new(FIRST_USER_SESSION_ID),
            nonce_counter: AtomicU32::new(0x1a2b),
            sessions: Mutex::new(HashMap::new()),
            by_peer: Mutex::new(HashMap::new()),
            ssk_by_peer: Mutex::new(ssk_by_peer),
            ssk_store,
            #[cfg(any(test, feature = "test-harness"))]
            default_ssk: Mutex::new(None),
            reply,
        }
    }

    /// Seed the fallback key used for a cipher-2 peer that never ran a key
    /// exchange. Test and harness builds only, never compiled into a release
    /// gateway.
    #[cfg(any(test, feature = "test-harness"))]
    pub fn set_default_ssk(&self, key: Vec<u8>) {
        *self.default_ssk.lock().unwrap() = Some(key);
    }

    /// The seeded fallback key, if any. Test and harness builds only.
    #[cfg(any(test, feature = "test-harness"))]
    pub fn default_ssk(&self) -> Option<Vec<u8>> {
        self.default_ssk.lock().unwrap().clone()
    }

    /// Record the shared secret established with a peer by the key exchange, and
    /// persist the table when a store is configured.
    pub fn store_ssk(&self, peer: SocketAddr, ssk: Vec<u8>) {
        let snapshot = {
            let mut map = self.ssk_by_peer.lock().unwrap();
            map.insert(peer, ssk);
            self.ssk_store.as_ref().map(|_| serialize_ssk_store(&map))
        };
        if let (Some(path), Some(text)) = (self.ssk_store.as_deref(), snapshot)
            && let Err(e) = write_ssk_store(path, &text)
        {
            tracing::warn!(path = %path.display(), error = %e, "failed to persist session keys");
        }
    }

    /// The shared secret for a peer, if a key exchange has completed.
    pub fn ssk(&self, peer: SocketAddr) -> Option<Vec<u8>> {
        self.ssk_by_peer.lock().unwrap().get(&peer).cloned()
    }

    /// Whether the session with this id negotiated the encrypted (cipher 2)
    /// cipher, or `None` if no such session is known (e.g. after a restart that
    /// cleared the in-memory table). A cleartext (cipher 0) session returns
    /// `Some(false)` so its content requests are not misrouted to the encrypted
    /// handler.
    pub fn session_is_cipher2(&self, server_session_id: u32) -> Option<bool> {
        self.sessions
            .lock()
            .unwrap()
            .get(&server_session_id)
            .map(|s| s.cipher.algorithm == crate::cipher::CIPHER_RC5_SESSION)
    }

    fn alloc_server_id(&self) -> u32 {
        self.next_server_id.fetch_add(1, Ordering::Relaxed)
    }

    fn next_nonce(&self) -> u16 {
        (self.nonce_counter.fetch_add(1, Ordering::Relaxed) & 0xffff) as u16
    }

    /// The C-SID (leading reply byte) for this request's negotiated cipher.
    /// Cipher 0 (the only one supported) is a clear-text session, C-SID 0.
    fn reply_csid(req: &SessionRequest) -> u8 {
        match req.cipher.algorithm {
            crate::cipher::CIPHER_NONE => CSID_CLEARTEXT,
            _ => CSID_CLEARTEXT,
        }
    }

    /// C-nonce values to answer with `+1`, per the configured reading. The SR
    /// trailer is the pair `C-nonce, C-nonceModified`; `Last` reads the final
    /// two contents bytes, `Prev` the two before them.
    fn nonce_candidates(&self, req: &SessionRequest) -> Vec<u16> {
        let r = &req.raw;
        let last = req.client_nonce;
        let prev = if r.len() >= 4 {
            u16::from_be_bytes([r[r.len() - 4], r[r.len() - 3]])
        } else {
            last
        };
        match self.reply.nonce {
            NonceChoice::Last => vec![last],
            NonceChoice::Prev => vec![prev],
            NonceChoice::Both if prev != last => vec![last, prev],
            NonceChoice::Both => vec![last],
        }
    }

    /// Handle a SessionRequest: reuse or create the peer's session and build a
    /// SessionReply addressed to each candidate client session id.
    pub fn on_session_request(&self, peer: SocketAddr, req: &SessionRequest) -> Vec<ServerMessage> {
        let client_ids = vec![Self::reply_csid(req)];

        // Probe mode: answer with an Error PDU instead of granting the session.
        // The Error Tag for a session-0 SessionRequest is the four bytes starting
        // at the ClientNonce, i.e. the request's trailing nonce pair.
        if let Some(code) = self.reply.cold_start_error_code {
            let raw = &req.raw;
            let tag = if raw.len() >= 4 {
                [
                    raw[raw.len() - 4],
                    raw[raw.len() - 3],
                    raw[raw.len() - 2],
                    raw[raw.len() - 1],
                ]
            } else {
                [0; 4]
            };
            return client_ids
                .iter()
                .map(|&client_session_id| ServerMessage {
                    client_session_id,
                    request_id: 0,
                    not_last: false,
                    pdu: ServerPdu::Error(crate::pdu::ErrorPdu {
                        tag,
                        code,
                        headers: Headers::new(),
                        data: Vec::new(),
                    }),
                })
                .collect();
        }

        // Reuse the peer's session only for a byte-identical retransmit, so a
        // retransmit is idempotent (same server session id and server nonce)
        // while a new or changed SessionRequest — including the handset's
        // reduced-capability re-request — is a fresh session creation that gets
        // fresh server material. Handing a new crypto-ignition attempt a server
        // nonce minted for an earlier one reads as a replay and the handset
        // cancels.
        let retransmit = self
            .by_peer
            .lock()
            .unwrap()
            .get(&peer)
            .copied()
            .and_then(|sid| {
                let table = self.sessions.lock().unwrap();
                table
                    .get(&sid)
                    .filter(|s| s.request_raw == req.raw)
                    .map(|s| s.server_session_id)
            });
        let session = if let Some(sid) = retransmit {
            self.sessions.lock().unwrap().get(&sid).unwrap().clone()
        } else {
            let server_session_id = self.alloc_server_id();
            let s = Session {
                peer,
                server_session_id,
                client_session_ids: client_ids.clone(),
                cipher: req.cipher,
                server_nonce: self.next_nonce(),
                client_nonce: req.client_nonce,
                state: SessionState::Proto,
                current_request_id: 0,
                capabilities: req.headers.clone(),
                request_raw: req.raw.clone(),
            };
            self.sessions
                .lock()
                .unwrap()
                .insert(server_session_id, s.clone());
            self.by_peer.lock().unwrap().insert(peer, server_session_id);
            s
        };

        // Emit a reply per (nonce candidate x layout) under the C-SID, as
        // configured. The handset treats several replies with differing
        // derivatives for one session as a replay fault and cancels, so the
        // default is a single (SUGP, last-nonce) reply; the sweep options exist
        // to pin the form on-air.
        let layouts = &self.reply.layouts;
        let nonces = self.nonce_candidates(req);
        let reply_headers = session_reply_headers(req);
        let mut out = Vec::with_capacity(client_ids.len() * layouts.len() * nonces.len());
        for &csid in &client_ids {
            for &nonce in &nonces {
                for &layout in layouts {
                    out.push(ServerMessage {
                        client_session_id: csid,
                        request_id: 0,
                        not_last: false,
                        pdu: ServerPdu::SessionReply(SessionReply {
                            layout,
                            server_nonce: session.server_nonce,
                            client_nonce_plus: nonce.wrapping_add(1),
                            server_session_id: session.server_session_id,
                            cipher: req.cipher,
                            session_key: Vec::new(),
                            headers: reply_headers.clone(),
                        }),
                    });
                }
            }
        }
        out
    }

    /// Mark a session Active on a valid SessionComplete (ServerNonce+1).
    pub fn on_session_complete(&self, server_session_id: u32, server_nonce_plus: u16) -> bool {
        let mut table = self.sessions.lock().unwrap();
        let Some(s) = table.get_mut(&server_session_id) else {
            return false;
        };
        if server_nonce_plus == s.server_nonce.wrapping_add(1) {
            s.state = SessionState::Active;
            true
        } else {
            false
        }
    }

    /// Snapshot the current session for a peer, if one exists.
    pub fn session_for_peer(&self, peer: SocketAddr) -> Option<Session> {
        let sid = *self.by_peer.lock().unwrap().get(&peer)?;
        self.sessions.lock().unwrap().get(&sid).cloned()
    }

    /// Snapshot a session by server session id.
    pub fn get(&self, server_session_id: u32) -> Option<Session> {
        self.sessions
            .lock()
            .unwrap()
            .get(&server_session_id)
            .cloned()
    }

    /// Record the request id on a request and return the client session ids to
    /// address replies to, if the session exists.
    pub fn note_request(&self, server_session_id: u32, request_id: u8) -> Option<Vec<u8>> {
        let mut table = self.sessions.lock().unwrap();
        let s = table.get_mut(&server_session_id)?;
        s.current_request_id = request_id;
        Some(s.client_session_ids.clone())
    }

    /// Like [`note_request`], but if the session is unknown adopt it as an
    /// active clear-text session. The handset caches its session across gateway
    /// restarts and resumes with a request rather than a fresh handshake, so an
    /// unknown session id is a live session this instance has simply forgotten;
    /// honoring it lets browsing survive a gateway restart. Returns the C-SID(s)
    /// to address replies to.
    pub fn note_or_adopt_request(
        &self,
        server_session_id: u32,
        request_id: u8,
        peer: SocketAddr,
    ) -> Vec<u8> {
        let mut table = self.sessions.lock().unwrap();
        let s = table.entry(server_session_id).or_insert_with(|| Session {
            peer,
            server_session_id,
            client_session_ids: vec![CSID_CLEARTEXT],
            cipher: Cipher::NONE,
            server_nonce: 0,
            client_nonce: 0,
            state: SessionState::Active,
            current_request_id: request_id,
            capabilities: Headers::new(),
            request_raw: Vec::new(),
        });
        s.peer = peer;
        s.current_request_id = request_id;
        if s.client_session_ids.is_empty() {
            s.client_session_ids.push(CSID_CLEARTEXT);
        }
        s.client_session_ids.clone()
    }

    pub fn len(&self) -> usize {
        self.sessions.lock().unwrap().len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

/// SUGP capability key carrying the version the handset advertises in its
/// SessionRequest (`0x91` on the wire; this crate's HTTP-derived key table does
/// not name it).
const CAP_VERSION_KEY: u8 = 0x91;

/// Session headers to return in the SessionReply. The protocol version is
/// negotiated during session creation (HDTP 1.1, "Versioning"), so the reply
/// confirms the version the handset advertised. Without a confirmation the
/// handset re-requests with reduced capabilities and then cancels the
/// cold-start session.
pub fn session_reply_headers(req: &SessionRequest) -> Headers {
    let mut h = Headers::new();
    if let Some(version) = raw_capability(&req.raw, CAP_VERSION_KEY) {
        h.push(format!("0x{CAP_VERSION_KEY:02x}"), version);
    }
    h
}

/// The value of a `code 0x00 <value> 0x00` capability pair in raw SessionRequest
/// contents, read by wire code rather than by this crate's key names.
fn raw_capability(raw: &[u8], code: u8) -> Option<String> {
    let mut i = 0;
    while i + 1 < raw.len() {
        if raw[i] == code && raw[i + 1] == 0 {
            let start = i + 2;
            let end = start + raw[start..].iter().position(|&b| b == 0)?;
            return Some(String::from_utf8_lossy(&raw[start..end]).into_owned());
        }
        i += 1;
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::pdu::{ClientMessage, ClientPdu};

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

    fn peer() -> SocketAddr {
        "10.55.0.2:8502".parse().unwrap()
    }

    fn request() -> SessionRequest {
        let msg = ClientMessage::decode(CAPTURE, Cipher::NONE).unwrap();
        match msg.pdu {
            ClientPdu::SessionRequest(sr) => sr,
            _ => panic!("expected SessionRequest"),
        }
    }

    #[test]
    fn session_reply_confirms_advertised_version() {
        let headers = session_reply_headers(&request());
        // The captured request advertises version 0x91 = "3.1,2.0"; the reply
        // echoes it, encoding back to the same wire bytes.
        assert_eq!(
            headers.encode(),
            [0x91, 0x00, b'3', b'.', b'1', b',', b'2', b'.', b'0', 0x00]
        );
    }

    #[test]
    fn default_emits_single_hdtp11_reply_with_cleartext_csid() {
        let mgr = SessionManager::new();
        let replies = mgr.on_session_request(peer(), &request());
        assert_eq!(
            replies.len(),
            1,
            "default is one reply to avoid a replay-cancel"
        );
        assert_eq!(replies[0].client_session_id, 0x00);
        let ServerPdu::SessionReply(sr) = &replies[0].pdu else {
            panic!("expected SessionReply");
        };
        assert_eq!(sr.layout, SessionReplyLayout::Hdtp11);
        // Default answers the C-nonce (prev-2 bytes = 0x0000 in the capture), so
        // the derivative is C-nonce + 1.
        assert_eq!(sr.client_nonce_plus, 0x0000u16.wrapping_add(1));
    }

    #[test]
    fn sweep_config_emits_layout_and_nonce_matrix() {
        let mgr = SessionManager::with_reply(ReplyConfig {
            layouts: vec![SessionReplyLayout::Hdtp11, SessionReplyLayout::Sugp],
            nonce: NonceChoice::Both,
            cold_start_error_code: None,
        });
        let replies = mgr.on_session_request(peer(), &request());
        assert_eq!(replies.len(), 4); // 2 layouts x 2 nonce readings
        assert!(replies.iter().all(|m| m.client_session_id == 0x00));
        let derivatives: std::collections::HashSet<u16> = replies
            .iter()
            .map(|m| match &m.pdu {
                ServerPdu::SessionReply(sr) => sr.client_nonce_plus,
                _ => panic!(),
            })
            .collect();
        assert!(derivatives.contains(&0x85e2u16.wrapping_add(1)));
        assert!(derivatives.contains(&0x0000u16.wrapping_add(1)));
    }

    #[test]
    fn retransmit_reuses_one_session() {
        let mgr = SessionManager::new();
        let first = mgr.on_session_request(peer(), &request());
        let sid1 = match &first[0].pdu {
            ServerPdu::SessionReply(sr) => sr.server_session_id,
            _ => panic!(),
        };
        let second = mgr.on_session_request(peer(), &request());
        let sid2 = match &second[0].pdu {
            ServerPdu::SessionReply(sr) => sr.server_session_id,
            _ => panic!(),
        };
        assert_eq!(
            sid1, sid2,
            "retransmit must reuse the same server session id"
        );
        assert_eq!(mgr.len(), 1);
    }

    #[test]
    fn handshake_completes() {
        let mgr = SessionManager::new();
        let replies = mgr.on_session_request(peer(), &request());
        let ServerPdu::SessionReply(sr) = &replies[0].pdu else {
            panic!();
        };
        let sid = sr.server_session_id;
        assert_eq!(mgr.get(sid).unwrap().state, SessionState::Proto);
        assert!(mgr.on_session_complete(sid, sr.server_nonce.wrapping_add(1)));
        assert_eq!(mgr.get(sid).unwrap().state, SessionState::Active);
        assert!(!mgr.on_session_complete(sid, 0xdead));
    }

    #[test]
    fn cleartext_session_is_not_cipher2() {
        // A cleartext (cipher 0) session must report Some(false) so its content
        // requests are routed to the cleartext handler, not the cipher-2 one, even
        // though its id is in the same >= 0x10 range as an encrypted session.
        let mgr = SessionManager::new();
        let replies = mgr.on_session_request(peer(), &request());
        let ServerPdu::SessionReply(sr) = &replies[0].pdu else {
            panic!();
        };
        assert_eq!(mgr.session_is_cipher2(sr.server_session_id), Some(false));
        assert_eq!(mgr.session_is_cipher2(0xdead_beef), None);
    }

    #[test]
    fn ssk_persists_across_restart() {
        let path = std::env::temp_dir().join(format!("hdtp_ssk_test_{}.txt", std::process::id()));
        let _ = std::fs::remove_file(&path);
        let secret = vec![0xdeu8, 0xad, 0xbe, 0xef, 0x01, 0x02, 0x03, 0x04];

        let first =
            SessionManager::with_reply_and_store(ReplyConfig::default(), Some(path.clone()));
        first.store_ssk(peer(), secret.clone());
        assert_eq!(first.ssk(peer()), Some(secret.clone()));

        // A fresh manager (a restarted gateway) reloads the secret from the store.
        let restarted =
            SessionManager::with_reply_and_store(ReplyConfig::default(), Some(path.clone()));
        assert_eq!(restarted.ssk(peer()), Some(secret));
        // An unknown peer stays absent.
        assert_eq!(restarted.ssk("10.55.0.9:8502".parse().unwrap()), None);

        let _ = std::fs::remove_file(&path);
    }
}
