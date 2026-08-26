//! The UDP service loop.
//!
//! The gateway binds a UDP socket (the address the handset targets for its
//! UP.Link proxy) and, per datagram, decodes an HDTP message, advances the
//! session state machine, and for content requests fetches the URL and returns
//! a transcoded HDML deck. Each datagram is handled on its own task so a slow
//! upstream fetch does not stall the receive loop.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use tokio::net::UdpSocket;
use tracing::{info, warn};

use crate::cipher::{self, Cipher};
use crate::hdml::{Deck, notice_deck};
use crate::header::{CTYPE_X_HDML, CTYPE_X_HDMLC, Headers};
use crate::pdu::{
    ClientMessage, ClientPdu, PduType, Reply, ServerMessage, ServerPdu, SessionRequest,
};
use crate::proxy::{FetchBody, Proxy};
use crate::session::{ReplyConfig, SessionManager};
use crate::transcode;

/// Receive buffer size; handset requests are well under this.
const RECV_BUF: usize = 4096;

/// Tunables for the gateway.
#[derive(Debug, Clone)]
pub struct GatewayConfig {
    pub user_agent: String,
    pub fetch_timeout: Duration,
    /// Cap on the serialized HDML reply so it fits the handset's PDU buffer.
    pub max_reply_bytes: usize,
    /// Content-Type sent with content replies (e.g. `text/x-hdml` or
    /// `application/x-hdmlc`), tunable while the accepted type is pinned down.
    pub content_type: String,
    /// How to shape the SessionReply while the 3.1 form is pinned down.
    pub reply: ReplyConfig,
    /// Optional file the established shared secrets are persisted to, so
    /// encrypted sessions survive a gateway restart. Point it at a mounted
    /// volume in a container; unset keeps the secrets in memory only.
    pub ssk_store: Option<std::path::PathBuf>,
}

impl Default for GatewayConfig {
    fn default() -> Self {
        GatewayConfig {
            user_agent: "Mozilla/5.0 (compatible; hdtp-gw/0.1; UP.Link)".to_string(),
            fetch_timeout: Duration::from_secs(20),
            max_reply_bytes: 1300,
            content_type: CTYPE_X_HDML.to_string(),
            reply: ReplyConfig::default(),
            ssk_store: None,
        }
    }
}

/// The running gateway.
pub struct Gateway {
    sessions: Arc<SessionManager>,
    proxy: Arc<Proxy>,
    cfg: Arc<GatewayConfig>,
}

impl Gateway {
    pub fn new(cfg: GatewayConfig) -> anyhow::Result<Self> {
        let proxy = Proxy::new(&cfg.user_agent, cfg.fetch_timeout)?;
        Ok(Gateway {
            sessions: Arc::new(SessionManager::with_reply_and_store(
                cfg.reply.clone(),
                cfg.ssk_store.clone(),
            )),
            proxy: Arc::new(proxy),
            cfg: Arc::new(cfg),
        })
    }

    /// Seed the test-only fallback key for cipher-2 peers that never run a key
    /// exchange. Test and harness builds only.
    #[cfg(any(test, feature = "test-harness"))]
    pub fn seed_test_ssk(&self, key: Vec<u8>) {
        self.sessions.set_default_ssk(key);
    }

    /// Bind and serve until the socket errors.
    pub async fn run(self, bind: SocketAddr) -> anyhow::Result<()> {
        let sock = Arc::new(UdpSocket::bind(bind).await?);
        info!(%bind, "HDTP gateway listening");
        let mut buf = vec![0u8; RECV_BUF];
        loop {
            let (n, peer) = sock.recv_from(&mut buf).await?;
            let datagram = buf[..n].to_vec();
            let sessions = Arc::clone(&self.sessions);
            let proxy = Arc::clone(&self.proxy);
            let cfg = Arc::clone(&self.cfg);
            let sock = Arc::clone(&sock);
            tokio::spawn(async move {
                if let Err(e) =
                    handle_datagram(&datagram, peer, &sessions, &proxy, &cfg, &sock).await
                {
                    warn!(%peer, error = %e, "datagram handling failed");
                }
            });
        }
    }
}

async fn handle_datagram(
    datagram: &[u8],
    peer: SocketAddr,
    sessions: &SessionManager,
    proxy: &Proxy,
    cfg: &GatewayConfig,
    sock: &UdpSocket,
) -> anyhow::Result<()> {
    // Crypto-ignition KeyRequest (PDU Type 15) does not fit the normal decoder.
    // Answer with a KeyReply (PDU Type 12, the handset's key-exchange reply type)
    // carrying the server's Diffie-Hellman public value, and remember the derived
    // shared secret to key the encrypted (cipher 2) session that follows.
    const KEYREQUEST_TYPE: u8 = 15;
    const KEYREPLY_TYPE: u8 = 12;
    const PDU_TYPE_MASK: u8 = 0x3f;
    if datagram.len() >= 6 && datagram[5] & PDU_TYPE_MASK == KEYREQUEST_TYPE {
        let body_in = &datagram[6..];
        if let Some(client_pub) = crate::keyexch::parse_client_public(body_in) {
            const SERVER_PRIV: [u8; 32] = [0x5a; 32];
            let ka = crate::keyexch::agree(&client_pub, &SERVER_PRIV);
            sessions.store_ssk(peer, ka.shared_secret.clone());
            // KeyReply: C-SID 0, echoed request id, type 12, then the
            // public-value length and the server public value. The handset reads
            // the length at data[3] and the public value at data[4] (no algo
            // byte, unlike the KeyRequest); a stray algo byte here shifts the
            // length read onto it, so the handset decodes a truncated public and
            // derives a different shared secret.
            let mut dg = vec![0u8, datagram[4], KEYREPLY_TYPE];
            dg.push(crate::keyexch::DH_BYTES as u8);
            dg.extend_from_slice(&ka.server_public);
            info!(
                %peer,
                client_pub = %hex(&client_pub),
                server_pub = %hex(&ka.server_public),
                ssk = %hex(&ka.shared_secret),
                "KeyRequest -> KeyReply (type 12)"
            );
            sock.send_to(&dg, peer).await?;
        } else {
            info!(%peer, raw = %hex(datagram), "KeyRequest parse failed");
        }
        return Ok(());
    }

    // Encrypted content requests on an established cipher-2 session (e.g. a Get
    // on the granted session id) don't fit the cleartext decoder; they are
    // decrypted, fetched, and answered cipher-2 wrapped on their own path. Only
    // route encrypted sessions here: cleartext (cipher 0) sessions get ids in the
    // same range, so gate on the session's negotiated cipher (falling back to
    // whether a shared secret exists when the in-memory session was lost to a
    // restart) to avoid misrouting a cleartext request into the cipher-2 handler.
    if datagram.len() >= 6 {
        let sid = u32::from_be_bytes([datagram[0], datagram[1], datagram[2], datagram[3]]);
        let is_cipher2 = sessions
            .session_is_cipher2(sid)
            .unwrap_or_else(|| session_key(sessions, peer).is_some());
        if sid >= CIPHER2_MIN_SESSION_ID && is_cipher2 {
            return handle_cipher2_session_message(sessions, proxy, cfg, peer, datagram, sock)
                .await;
        }
    }

    // Only Cipher 0 is supported, so the body is never encrypted.
    let msg = match ClientMessage::decode(datagram, Cipher::NONE) {
        Ok(m) => m,
        Err(e) => {
            info!(%peer, error = %e, len = datagram.len(), raw = %hex(datagram), "undecodable datagram");
            return Ok(());
        }
    };

    // An encrypted (cipher 2) SessionRequest carries its ClientNonce inside the
    // RC5 trailer rather than in the clear, and its reply must carry a matching
    // trailer. It is handled on its own path.
    if let ClientPdu::SessionRequest(ref req) = msg.pdu
        && req.cipher.algorithm == cipher::CIPHER_RC5_SESSION
    {
        info!(
            %peer,
            hdr = %hex(&datagram[..datagram.len().min(6)]),
            session_id = msg.session_id,
            request_id = msg.request_id,
            len = datagram.len(),
            "cipher-2 SessionRequest datagram header"
        );
        return handle_cipher2_session_request(sessions, peer, req, sock).await;
    }

    let replies: Vec<ServerMessage> = match msg.pdu {
        ClientPdu::SessionRequest(req) => {
            // Log the raw contents and parse so a capture can be correlated to
            // exactly what the gateway saw and answered.
            info!(
                %peer,
                client_session = req.client_session_id,
                nonce = format_args!("0x{:04x}", req.client_nonce),
                cipher = req.cipher.algorithm,
                headers = req.headers.0.len(),
                raw = %hex(&req.raw),
                "SessionRequest"
            );
            sessions.on_session_request(peer, &req)
        }
        ClientPdu::SessionComplete(sc) => {
            let ok = sessions.on_session_complete(msg.session_id, sc.server_nonce_plus);
            info!(%peer, session = msg.session_id, ok, "SessionComplete");
            Vec::new()
        }
        ClientPdu::Get(get) => {
            info!(%peer, session = msg.session_id, url = %get.url, "Get");
            handle_fetch(
                sessions,
                proxy,
                cfg,
                peer,
                msg.session_id,
                msg.request_id,
                &get.url,
                None,
            )
            .await
        }
        ClientPdu::Post(post) => {
            info!(%peer, session = msg.session_id, url = %post.url, "Post");
            handle_fetch(
                sessions,
                proxy,
                cfg,
                peer,
                msg.session_id,
                msg.request_id,
                &post.url,
                Some(post.data),
            )
            .await
        }
        ClientPdu::GetNotification => {
            // No push queue; answer with an empty notice so the handset stops
            // polling this transaction.
            reply_for_session(
                sessions,
                msg.session_id,
                msg.request_id,
                notice_deck("No messages", "There are no notifications."),
                cfg,
            )
        }
        ClientPdu::Ack => {
            info!(%peer, session = msg.session_id, rid = msg.request_id, "Ack");
            Vec::new()
        }
        ClientPdu::Cancel => {
            info!(%peer, session = msg.session_id, rid = msg.request_id, "Cancel");
            Vec::new()
        }
        ClientPdu::Other { ty, ref body } => {
            info!(%peer, session = msg.session_id, ?ty, raw = %hex(body), "unhandled PDU type");
            Vec::new()
        }
    };

    for reply in replies {
        let bytes = reply.encode(Cipher::NONE);
        sock.send_to(&bytes, peer).await?;
    }
    Ok(())
}

/// The session key for a cipher-2 peer: the Diffie-Hellman shared secret from a
/// completed key exchange, or the test-only seeded fallback for a handset whose
/// default boot uses a cached constant key with no exchange. RC5 uses the first
/// 5 bytes and the session key is the first 16. A release build has no seeded
/// fallback, so it keys only from a genuine key exchange.
fn session_key(sessions: &SessionManager, peer: SocketAddr) -> Option<Vec<u8>> {
    sessions
        .ssk(peer)
        .filter(|k| k.len() >= 5)
        .or_else(|| seeded_fallback_key(sessions))
}

/// The test-only seeded fallback key. Compiled out of release builds, where a
/// cipher-2 session must come from a real key exchange.
#[cfg(any(test, feature = "test-harness"))]
fn seeded_fallback_key(sessions: &SessionManager) -> Option<Vec<u8>> {
    sessions.default_ssk().filter(|k| k.len() >= 5)
}

#[cfg(not(any(test, feature = "test-harness")))]
fn seeded_fallback_key(_sessions: &SessionManager) -> Option<Vec<u8>> {
    None
}

/// Established cipher-2 sessions carry a 4-byte session id at or above this
/// value; a datagram whose leading id is this large is an encrypted content
/// request on a granted session, not a cleartext session-creation datagram.
const CIPHER2_MIN_SESSION_ID: u32 = 0x10;

/// The handset reads the reply-data header keyed by this byte as the URL to
/// fetch next; the SessionReply carries it set to the home deck's URL.
const HOME_URL_HEADER_KEY: u8 = 0x87;

/// The URL the handset fetches for its home page.
const HOME_URL: &str = "device:home";

/// Answer an encrypted (cipher 2) SessionRequest. The ClientNonce is recovered
/// by decrypting the request's RC5 trailer with the session key; the reply is
/// the HDTP 1.1 SessionReply body in the clear followed by an RC5 trailer that
/// proves the gateway holds the same key.
async fn handle_cipher2_session_request(
    sessions: &SessionManager,
    peer: SocketAddr,
    req: &SessionRequest,
    sock: &UdpSocket,
) -> anyhow::Result<()> {
    let key = match session_key(sessions, peer) {
        Some(k) => k,
        None => {
            warn!(%peer, "cipher-2 SessionRequest but no session key (no completed key exchange)");
            return Ok(());
        }
    };

    let raw = &req.raw;
    if raw.len() < 8 {
        warn!(%peer, len = raw.len(), "cipher-2 SessionRequest too short for a trailer");
        return Ok(());
    }
    let mut trailer_in = [0u8; 8];
    trailer_in.copy_from_slice(&raw[raw.len() - 8..]);
    let (client_nonce, word1) = cipher::cipher2_recover_nonce(&key, &trailer_in);
    let trailer_valid = word1 == client_nonce ^ cipher::CIPHER2_NONCE_MOD;

    // Grant or reuse the session (reusing the cleartext bookkeeping), then read
    // back the assigned server session id and nonce.
    let _ = sessions.on_session_request(peer, req);
    let session = match sessions.session_for_peer(peer) {
        Some(s) => s,
        None => {
            warn!(%peer, "cipher-2 session not created");
            return Ok(());
        }
    };

    // Build the cipher-2 SessionReply as a nested-CBC datagram: `[C-SID=2]` plus
    // a CBC-encrypted region. In the handset the region decrypts to the inner
    // SessionReply `[rid][type=2][body]` followed by a 4-byte XOR-fold MAC over
    // `[C-SID] ++ inner`.
    let csid = cipher::CIPHER_RC5_SESSION;

    // The body's data field is a list of `[key\0][value\0]` header pairs. The
    // handset reads the entry keyed by `HOME_URL_HEADER_KEY` as the URL to fetch
    // next; without it it resolves nothing and raises System Error (0x2300).
    let mut data: Vec<u8> = Vec::new();
    data.push(HOME_URL_HEADER_KEY);
    data.push(0);
    data.extend_from_slice(HOME_URL.as_bytes());
    data.push(0);

    // Carry the 16-byte session key with SessionKeyLen=16: the handset stores
    // that length, and its cipher context rejects every later encrypted content
    // PDU if the stored key length is 0.
    let key16 = &key[..16.min(key.len())];
    let mut body: Vec<u8> = Vec::new();
    body.extend_from_slice(&session.server_nonce.to_be_bytes());
    body.extend_from_slice(&client_nonce.wrapping_add(1).to_be_bytes());
    body.extend_from_slice(&session.server_session_id.to_be_bytes());
    body.extend_from_slice(&req.cipher.to_bytes());
    body.push(key16.len() as u8); // session-key length
    body.extend_from_slice(&(data.len() as u16).to_be_bytes()); // data length
    body.extend_from_slice(key16); // session key
    body.extend_from_slice(&data);

    let mut inner: Vec<u8> = vec![0u8, PduType::SessionReply as u8];
    inner.extend_from_slice(&body);
    let dg = cbcenc_datagram(&key, csid, &inner);
    info!(
        %peer,
        client_nonce = format_args!("0x{client_nonce:04x}"),
        trailer_valid,
        server_session = session.server_session_id,
        total = dg.len(),
        "cipher-2 SessionRequest -> SessionReply [cbcenc]"
    );
    sock.send_to(&dg, peer).await?;
    Ok(())
}

/// Wrap an inner PDU (`[rid][type][body]`) as a cipher-2 nested-CBC datagram:
/// append the 4-byte XOR-fold MAC over `[csid] ++ inner`, pad to an 8-byte
/// multiple, CBC-encrypt into the region, and prepend the C-SID.
fn cbcenc_datagram(key: &[u8], csid: u8, inner_content: &[u8]) -> Vec<u8> {
    let mut inner = inner_content.to_vec();
    let mut macd = vec![csid];
    macd.extend_from_slice(&inner);
    let mac = cipher::xor_fold4(&macd);
    inner.extend_from_slice(&mac);
    while !inner.len().is_multiple_of(8) {
        inner.push(0);
    }
    let region = cipher::cipher2_cbc_encrypt(key, &inner);
    let mut dg = vec![csid];
    dg.extend_from_slice(&region);
    dg
}

/// Answer an encrypted content request on an established cipher-2 session by
/// fetching the requested URL (or serving the portal deck for `device:home`),
/// wrapped as a cipher-2 Reply.
async fn handle_cipher2_session_message(
    sessions: &SessionManager,
    proxy: &Proxy,
    cfg: &GatewayConfig,
    peer: SocketAddr,
    datagram: &[u8],
    sock: &UdpSocket,
) -> anyhow::Result<()> {
    let key = match session_key(sessions, peer) {
        Some(k) => k,
        None => return Ok(()),
    };
    // The handset frames an established-session request as [session_id(4)]
    // followed by the nested-CBC region. Decrypt the region to recover the inner
    // request, which after a 4-byte transaction-id prefix is an ordinary
    // cleartext message `[session_id(4)][rid(1)][type(1)][body]` (a 4-byte
    // XOR-fold MAC and padding follow, which the PDU decoders ignore). The
    // handset matches the reply against the inner request id, so echo that.
    const TXN_ID_PREFIX: usize = 4;
    const REQ_RID_OFFSET: usize = 8;
    let region = &datagram[4..];
    let plain = cipher::cipher2_cbc_decrypt(&key, region);

    // Recover the requested URL (or POST) and fetch it, exactly like the
    // cleartext path. Fall back to the home deck if the inner request doesn't
    // decode or isn't a content request.
    let (rid, url, post_body) = match plain
        .get(TXN_ID_PREFIX..)
        .map(|inner| ClientMessage::decode(inner, Cipher::NONE))
    {
        Some(Ok(msg)) => match msg.pdu {
            ClientPdu::Get(get) => (msg.request_id, Some(get.url), None),
            ClientPdu::Post(post) => (msg.request_id, Some(post.url), Some(post.data)),
            _ => (msg.request_id, None, None),
        },
        _ => (plain.get(REQ_RID_OFFSET).copied().unwrap_or(0), None, None),
    };

    let deck = match &url {
        Some(u) => fetch_deck(proxy, u, post_body).await,
        None => home_deck(),
    };
    let data = fit_hdmlc(deck, cfg.max_reply_bytes);
    let mut headers = Headers::new();
    headers.push("Content-Type", CTYPE_X_HDMLC);
    let reply = Reply {
        headers,
        data: data.clone(),
    };
    let mut inner = vec![rid, PduType::Reply as u8];
    inner.extend_from_slice(&reply.encode_body());
    let dg = cbcenc_datagram(&key, cipher::CIPHER_RC5_SESSION, &inner);
    info!(
        %peer,
        rid,
        url = url.as_deref().unwrap_or("device:home"),
        deck_bytes = data.len(),
        total = dg.len(),
        "cipher-2 session Get -> Reply [cbcenc]"
    );
    sock.send_to(&dg, peer).await?;
    Ok(())
}

/// Lowercase hex of a byte slice, for logging captured PDUs.
fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

/// Fetch a URL and build a Reply (or Error) addressed to the request's session.
#[allow(clippy::too_many_arguments)]
async fn handle_fetch(
    sessions: &SessionManager,
    proxy: &Proxy,
    cfg: &GatewayConfig,
    peer: SocketAddr,
    session_id: u32,
    request_id: u8,
    url: &str,
    post_body: Option<Vec<u8>>,
) -> Vec<ServerMessage> {
    // Adopt an unknown session: the handset caches its session across gateway
    // restarts and resumes with a request, so honor it rather than erroring.
    let client_ids = sessions.note_or_adopt_request(session_id, request_id, peer);
    let deck = fetch_deck(proxy, url, post_body).await;
    reply_messages(&client_ids, request_id, deck, cfg)
}

/// Fetch `url` and transcode it to an HDML deck. Internal handset URLs (e.g. the
/// home request `device:home`) are not web resources, so they return the portal
/// deck instead of being fetched.
async fn fetch_deck(proxy: &Proxy, url: &str, post_body: Option<Vec<u8>>) -> Deck {
    if !is_web_url(url) {
        return home_deck();
    }
    let fetched = match post_body {
        Some(body) => proxy.post(url, body).await,
        None => proxy.get(url).await,
    };
    match fetched {
        Ok(page) => match page.body {
            FetchBody::Html(html) => transcode::html_to_hdml(&html, &page.final_url),
            FetchBody::Text(text) => transcode::text_to_hdml(&text, "Page"),
            FetchBody::Other { .. } => notice_deck(
                "Unsupported",
                &format!("Cannot display {}.", page.content_type),
            ),
        },
        Err(e) => notice_deck("Fetch failed", &format!("Could not load the page: {e}")),
    }
}

fn is_web_url(url: &str) -> bool {
    let u = url.trim().to_ascii_lowercase();
    u.starts_with("http://") || u.starts_with("https://")
}

/// The gateway's portal deck: a title, the current date and time, and a couple
/// of navigable links, which compile to a CHOICE menu with `TASK=GO` entries the
/// handset can follow.
fn home_deck() -> Deck {
    use crate::hdml::{Block, Inline};
    let now = chrono::Local::now();
    let mut d = Deck::new();
    d.push(Block::Line(vec![Inline::Text("1xBTS WWW".to_string())]));
    d.push(Block::Line(vec![Inline::Text("Welcome!!!".to_string())]));
    // Keep the date and time to one line within the ~12-char display.
    d.push(Block::Line(vec![Inline::Text(
        now.format("%m-%d %H:%M").to_string(),
    )]));
    for (label, dest) in [
        ("FrogFind", "http://frogfind.com/"),
        ("68k News", "http://68k.news/"),
        ("First Site", "http://info.cern.ch/"),
    ] {
        d.push(Block::Line(vec![Inline::Link {
            label: label.to_string(),
            dest: dest.to_string(),
        }]));
    }
    d
}

/// Build a Reply per candidate client session id, dropping trailing blocks until
/// the serialized deck fits the PDU cap.
fn reply_messages(
    client_ids: &[u8],
    request_id: u8,
    deck: Deck,
    cfg: &GatewayConfig,
) -> Vec<ServerMessage> {
    // The handset renders only compiled HDMLc, so every content reply is
    // compiled and sent as application/x-hdmlc.
    let data = fit_hdmlc(deck, cfg.max_reply_bytes);
    let mut headers = Headers::new();
    headers.push("Content-Type", CTYPE_X_HDMLC);
    client_ids
        .iter()
        .map(|&client_session_id| ServerMessage {
            client_session_id,
            request_id,
            not_last: false,
            pdu: ServerPdu::Reply(Reply {
                headers: headers.clone(),
                data: data.clone(),
            }),
        })
        .collect()
}

/// Build replies for a session looked up by server session id, or an empty vec
/// if the session is unknown.
fn reply_for_session(
    sessions: &SessionManager,
    session_id: u32,
    request_id: u8,
    deck: Deck,
    cfg: &GatewayConfig,
) -> Vec<ServerMessage> {
    match sessions.note_request(session_id, request_id) {
        Some(client_ids) => reply_messages(&client_ids, request_id, deck, cfg),
        None => Vec::new(),
    }
}

/// Compile a deck to HDMLc bytes, dropping trailing blocks until it fits. The
/// cap is kept under 250 so the deck length stays a single byte below `0xff`
/// (the `ff`-terminated length can't contain `0xff`).
fn fit_hdmlc(mut deck: Deck, max_bytes: usize) -> Vec<u8> {
    let cap = max_bytes.min(250);
    loop {
        let b = crate::hdmlc::compile_deck(&deck);
        if b.len() <= cap || deck.blocks.len() <= 1 {
            return b;
        }
        deck.blocks.pop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::hdml::{Block, Inline};

    #[test]
    fn session_key_has_no_fallback_until_seeded() {
        let peer: SocketAddr = "10.55.0.3:8502".parse().unwrap();
        let sessions = SessionManager::new();
        // No key exchange and nothing seeded: a cipher-2 peer gets no key.
        assert_eq!(session_key(&sessions, peer), None);
        // The test-only store seeds the fallback the harness relies on.
        sessions.set_default_ssk(vec![1, 2, 3, 4, 5, 6]);
        assert_eq!(session_key(&sessions, peer), Some(vec![1, 2, 3, 4, 5, 6]));
        // A real key exchange still wins over the seeded fallback.
        sessions.store_ssk(peer, vec![9, 9, 9, 9, 9, 9]);
        assert_eq!(session_key(&sessions, peer), Some(vec![9, 9, 9, 9, 9, 9]));
    }

    #[test]
    fn fit_hdmlc_trims_to_cap() {
        let mut deck = Deck::new();
        for i in 0..200 {
            deck.push(Block::Line(vec![Inline::Text(format!("line number {i}"))]));
        }
        let out = fit_hdmlc(deck, 1000);
        // Capped under 250 so the deck length stays a single byte.
        assert!(out.len() <= 250);
        // Valid deck framing: header and deck-close.
        assert_eq!(&out[..4], &[0xcf, 0x01, 0x03, 0x00]);
        assert_eq!(out.last(), Some(&0x89));
    }

    #[test]
    fn decrypted_cipher2_get_yields_url_and_rid() {
        // The decrypted inner request is a 4-byte transaction-id prefix followed
        // by an ordinary cleartext message, with a MAC and padding trailing that
        // the PDU decoders ignore. Decoding `plain[4..]` must recover the URL and
        // the inner request id the reply echoes.
        let url = b"http://info.cern.ch/";
        let mut plain = vec![0x00, 0x01, 0x02, 0x03]; // txnid
        plain.extend_from_slice(&[0x00, 0x00, 0x00, 0x10]); // session id
        plain.push(7); // rid
        plain.push(PduType::Get as u8); // type
        plain.extend_from_slice(&(url.len() as u16).to_be_bytes()); // UrlLen
        plain.extend_from_slice(&0u16.to_be_bytes()); // HeadersLen
        plain.extend_from_slice(url);
        plain.extend_from_slice(&[0xde, 0xad, 0xbe, 0xef, 0x00, 0x00]); // MAC + pad

        let msg = ClientMessage::decode(&plain[4..], Cipher::NONE).unwrap();
        assert_eq!(msg.request_id, 7);
        match msg.pdu {
            ClientPdu::Get(get) => assert_eq!(get.url, "http://info.cern.ch/"),
            other => panic!("expected Get, got {other:?}"),
        }
    }
}
