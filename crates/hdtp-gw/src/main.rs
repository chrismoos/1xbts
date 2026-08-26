//! `hdtp-gw` — the Openwave UP.Link gateway daemon.

use std::net::SocketAddr;
use std::time::Duration;

use clap::Parser;
use hdtp_gw::pdu::SessionReplyLayout;
use hdtp_gw::server::{Gateway, GatewayConfig};
use hdtp_gw::session::{NonceChoice, ReplyConfig};

#[derive(Parser, Debug)]
#[command(name = "hdtp-gw", about = "Openwave UP.Link (HDTP) gateway")]
struct Args {
    /// UDP address to bind. This is the proxy address the handset targets.
    #[arg(long, env = "HDTP_BIND", default_value = "0.0.0.0:1905")]
    bind: SocketAddr,

    /// User-Agent used for outbound fetches.
    #[arg(
        long,
        env = "HDTP_USER_AGENT",
        default_value = "Mozilla/5.0 (compatible; hdtp-gw/0.1; UP.Link)"
    )]
    user_agent: String,

    /// Outbound fetch timeout, in seconds.
    #[arg(long, env = "HDTP_FETCH_TIMEOUT_SECS", default_value_t = 20)]
    fetch_timeout_secs: u64,

    /// Cap on the serialized HDML reply, in bytes, so it fits the handset PDU.
    #[arg(long, env = "HDTP_MAX_REPLY_BYTES", default_value_t = 1300)]
    max_reply_bytes: usize,

    /// Content-Type for content replies: e.g. `text/x-hdml`, `text/hdml`,
    /// `application/x-hdmlc`.
    #[arg(long, env = "HDTP_CONTENT_TYPE", default_value = "text/x-hdml")]
    content_type: String,

    /// SessionReply layout(s) to send: `sugp`, `hdtp11`, or `both`.
    #[arg(long, env = "HDTP_REPLY_LAYOUT", default_value = "hdtp11")]
    reply_layout: String,

    /// Which SessionRequest bytes are the C-nonce: `last`, `prev`, or `both`.
    #[arg(long, env = "HDTP_REPLY_NONCE", default_value = "prev")]
    reply_nonce: String,

    /// Diagnostic: answer a SessionRequest with an Error PDU of this code
    /// instead of granting the session (2 = Key Error / crypto-ignition probe).
    #[arg(long, env = "HDTP_COLD_START_ERROR")]
    cold_start_error: Option<u16>,

    /// File to persist established session keys to, so encrypted sessions survive
    /// a restart. In a container, point it at a path on a mounted volume. Unset
    /// keeps the keys in memory only.
    #[arg(long, env = "HDTP_SSK_STORE")]
    ssk_store: Option<std::path::PathBuf>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    let args = Args::parse();
    let layouts = match args.reply_layout.to_ascii_lowercase().as_str() {
        "hdtp11" => vec![SessionReplyLayout::Hdtp11],
        "both" => vec![SessionReplyLayout::Sugp, SessionReplyLayout::Hdtp11],
        _ => vec![SessionReplyLayout::Sugp],
    };
    let nonce = match args.reply_nonce.to_ascii_lowercase().as_str() {
        "prev" => NonceChoice::Prev,
        "both" => NonceChoice::Both,
        _ => NonceChoice::Last,
    };
    let cfg = GatewayConfig {
        user_agent: args.user_agent,
        fetch_timeout: Duration::from_secs(args.fetch_timeout_secs),
        max_reply_bytes: args.max_reply_bytes,
        content_type: args.content_type,
        reply: ReplyConfig {
            layouts,
            nonce,
            cold_start_error_code: args.cold_start_error,
        },
        ssk_store: args.ssk_store,
    };
    let gateway = Gateway::new(cfg)?;

    // Test-harness builds can seed a fallback key for a handset that boots with
    // a cached constant key and never runs a key exchange. Absent from release.
    #[cfg(feature = "test-harness")]
    if let Ok(path) = std::env::var("HDTP_TEST_SSK_FILE") {
        let key = std::fs::read(&path)?;
        gateway.seed_test_ssk(key);
        tracing::info!(path = %path, "seeded test-harness session key");
    }

    gateway.run(args.bind).await
}
