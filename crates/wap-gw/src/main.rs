//! HTML-to-WML transcoding proxy.
//!
//! Runs beside Kannel in the packet-data gateway. wapbox is configured to fetch
//! through this process, so every page a WAP browser requests arrives here as an
//! ordinary forward-proxy request, is fetched and reduced to a deck, and goes
//! back as WML for wapbox to compile to WBXML. Content a handset renders on its
//! own goes back exactly as the site sent it.

use std::net::SocketAddr;
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::{HeaderMap, Method, StatusCode, header};
use axum::response::{IntoResponse, Response};
use clap::Parser;
use gw_transcode::doc::{Deck, notice_deck};
use gw_transcode::proxy::{
    FetchBody, MAX_BODY_BYTES, Proxy, ProxyError, forwardable_request_headers,
    forwardable_response_headers,
};
use gw_transcode::transcode::{self, Limits};
use tracing::{info, warn};

use wap_gw::is_handset_native;
use wap_gw::wml::{CTYPE_WML, to_wml};

/// Default upstream fetch timeout.
const DEFAULT_TIMEOUT_SECS: u64 = 20;

/// Longest reason shown on a handset. A card holds a few short lines, so a long
/// cause is trimmed rather than pushing the rest of the notice off the screen.
const MAX_REASON_CHARS: usize = 120;

/// Flatten an error and its causes into one line. The outermost message reads
/// the same whether the name did not resolve, the connection was refused, the
/// server never answered or the certificate failed to verify, so the cause is
/// the only part worth logging.
fn causes(e: &dyn std::error::Error) -> String {
    let mut out = e.to_string();
    let mut src = e.source();
    while let Some(cause) = src {
        out.push_str(": ");
        out.push_str(&cause.to_string());
        src = cause.source();
    }
    out
}

/// The innermost cause, which is the one that names what actually went wrong.
fn reason(e: &dyn std::error::Error) -> String {
    let mut deepest = e;
    while let Some(cause) = deepest.source() {
        deepest = cause;
    }
    let text = deepest.to_string();
    match text.char_indices().nth(MAX_REASON_CHARS) {
        Some((cut, _)) => format!("{}...", &text[..cut]),
        None => text,
    }
}

#[derive(Parser, Debug)]
#[command(
    name = "wap-gw",
    about = "HTML-to-WML transcoding proxy for the WAP gateway"
)]
struct Args {
    /// Address to serve the forward proxy on.
    #[arg(long, env = "WAP_GW_BIND", default_value = "127.0.0.1:8090")]
    bind: SocketAddr,

    /// Maximum blocks emitted for one page. Defaults to the transcoder's
    /// shared cap. WML carries more per-element overhead than HDML, so lower
    /// it here if decks come out too large for the handset.
    #[arg(long, env = "WAP_GW_MAX_BLOCKS", default_value_t = Limits::default().max_blocks)]
    max_blocks: usize,

    /// Maximum input fields carried from one form.
    #[arg(long, env = "WAP_GW_MAX_FIELDS", default_value_t = Limits::default().max_fields)]
    max_fields: usize,

    /// Upstream fetch timeout.
    #[arg(long, env = "WAP_GW_TIMEOUT_SECS", default_value_t = DEFAULT_TIMEOUT_SECS)]
    timeout_secs: u64,

    /// User-Agent presented to the origin server when the handset's request
    /// carries none.
    #[arg(
        long,
        env = "WAP_GW_USER_AGENT",
        default_value = "Mozilla/5.0 (compatible; 1xBTS WAP gateway)"
    )]
    user_agent: String,
}

struct AppState {
    proxy: Proxy,
    limits: Limits,
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
    let limits = Limits {
        max_blocks: args.max_blocks,
        max_fields: args.max_fields,
        ..Limits::default()
    };
    let state = Arc::new(AppState {
        proxy: Proxy::new(&args.user_agent, Duration::from_secs(args.timeout_secs))?,
        limits,
    });

    let app = Router::new().fallback(handle).with_state(state);
    let listener = tokio::net::TcpListener::bind(args.bind).await?;
    info!(
        bind = %args.bind,
        max_blocks = limits.max_blocks,
        "wap-gw: HTML->WML proxy listening"
    );
    axum::serve(listener, app).await?;
    Ok(())
}

async fn handle(State(state): State<Arc<AppState>>, req: Request) -> Response {
    let (parts, body) = req.into_parts();
    let method = parts.method;
    let uri = parts.uri.to_string();
    let req_headers = forwardable_request_headers(&parts.headers);

    // A forward proxy is addressed in absolute form. Anything else is a client
    // that reached us directly and has no page to name.
    if !(uri.starts_with("http://") || uri.starts_with("https://")) {
        return wml_response(
            StatusCode::BAD_REQUEST,
            &notice_deck("Bad request", "This is a proxy; ask for a full URL."),
            None,
        );
    }

    let page = match method {
        Method::GET => state.proxy.get(&uri, &req_headers).await,
        Method::POST => {
            let bytes = match axum::body::to_bytes(body, MAX_BODY_BYTES).await {
                Ok(b) => b,
                Err(e) => {
                    // Submitting the form without its fields would look like a
                    // server-side failure to the handset, so say so instead.
                    warn!(%uri, error = %e, "wap-gw: unreadable request body");
                    return wml_response(
                        StatusCode::OK,
                        &notice_deck("Too large", "That submission was too big to send."),
                        None,
                    );
                }
            };
            state.proxy.post(&uri, &req_headers, bytes.to_vec()).await
        }
        other => {
            warn!(%other, %uri, "wap-gw: unsupported method");
            return wml_response(
                StatusCode::METHOD_NOT_ALLOWED,
                &notice_deck("Not supported", "That request type is not supported."),
                None,
            );
        }
    };

    let page = match page {
        Ok(page) => page,
        Err(e) => {
            warn!(%uri, error = %causes(&e), "wap-gw: fetch failed");
            let deck = match e {
                ProxyError::TooLarge(_) => {
                    notice_deck("Too large", "That page is too big for the gateway to load.")
                }
                ProxyError::Blocked(_) => {
                    notice_deck("Not allowed", "That address is not reachable from here.")
                }
                _ => notice_deck(
                    "Fetch failed",
                    &format!("Could not load the page. {}", reason(&e)),
                ),
            };
            // A fetch failure is still a deck: the handset renders the notice,
            // which is more use than the browser's own error card for a status
            // it cannot explain.
            return wml_response(StatusCode::OK, &deck, None);
        }
    };

    info!(
        %uri,
        final_url = %page.final_url,
        status = page.status,
        ctype = %page.content_type,
        bytes = page.bytes.len(),
        "wap-gw: fetched"
    );

    if is_handset_native(&page.content_type) {
        let status = StatusCode::from_u16(page.status).unwrap_or(StatusCode::BAD_GATEWAY);
        return with_origin_headers(
            (status, page.bytes.clone()).into_response(),
            &page.headers,
            &page.content_type,
        );
    }

    let deck = match page.body() {
        FetchBody::Html(html) => transcode::html_to_deck(&html, &page.final_url, &state.limits),
        FetchBody::Text(text) => transcode::text_to_deck(&text, "Page", &state.limits),
        FetchBody::Other => notice_deck(
            "Not shown",
            &format!(
                "This page is {}, which a phone cannot show.",
                page.content_type
            ),
        ),
    };
    wml_response(StatusCode::OK, &deck, Some(&page.headers))
}

/// A WML reply. `origin` carries the fetched page's headers on to the handset
/// (cookies, caching), with the content type replaced by the deck's own.
fn wml_response(status: StatusCode, deck: &Deck, origin: Option<&HeaderMap>) -> Response {
    let resp = (status, to_wml(deck)).into_response();
    match origin {
        Some(h) => with_origin_headers(resp, h, CTYPE_WML),
        None => with_origin_headers(resp, &HeaderMap::new(), CTYPE_WML),
    }
}

fn with_origin_headers(mut resp: Response, origin: &HeaderMap, content_type: &str) -> Response {
    let headers = resp.headers_mut();
    for (name, value) in &forwardable_response_headers(origin) {
        headers.append(name.clone(), value.clone());
    }
    match header::HeaderValue::from_str(content_type) {
        Ok(v) => {
            headers.insert(header::CONTENT_TYPE, v);
        }
        Err(_) => {
            headers.remove(header::CONTENT_TYPE);
        }
    }
    resp
}
