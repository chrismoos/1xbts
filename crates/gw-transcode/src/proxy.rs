//! Outbound HTTP fetch for the gateways.
//!
//! A handset Get/Post names an absolute URL. The gateway fetches it over
//! ordinary HTTP(S), following redirects, and hands back the response with its
//! headers and raw body so the caller can transcode HTML, wrap plain text, or
//! pass native content through untouched. The handset's own request headers
//! travel with the fetch, so a site sees the real User-Agent and cookies.
//!
//! The fetch refuses destinations inside the gateway's own network: loopback,
//! RFC 1918 and the other private, link-local and reserved IPv4 ranges, and
//! every IPv6 address. Host names go through a filtering resolver so the
//! address that passes the check is the one connected to, and each redirect
//! hop is checked the same way.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use reqwest::header::{CONNECTION, CONTENT_TYPE, HeaderMap, HeaderName};
use reqwest::redirect;
use url::{Host, Url};

pub use reqwest::header;

/// Maximum response body the gateway accepts. Nothing is truncated: a larger
/// body is refused outright, so a handset never gets a cut-off page or a
/// partial binary served under the origin's content type.
pub const MAX_BODY_BYTES: usize = 4 * 1024 * 1024;

/// Redirect hops followed before the fetch gives up.
const MAX_REDIRECTS: usize = 10;

/// Headers that describe the connection rather than the resource, never
/// forwarded in either direction.
const HOP_BY_HOP: &[&str] = &[
    "connection",
    "keep-alive",
    "proxy-authenticate",
    "proxy-authorization",
    "proxy-connection",
    "te",
    "trailer",
    "transfer-encoding",
    "upgrade",
];

/// Request headers the fetch sets itself. `Accept-Encoding` stays with the
/// client because it also decodes the response.
const REQUEST_OWNED: &[&str] = &["host", "content-length", "accept-encoding"];

/// Response headers that describe the encoded wire body, which the client has
/// already decoded.
const RESPONSE_OWNED: &[&str] = &["content-length", "content-encoding"];

/// A fetched resource: the final URL after redirects, the origin's status and
/// headers, and the body exactly as the origin sent it (after transfer
/// decoding).
#[derive(Debug)]
pub struct FetchedPage {
    pub final_url: String,
    pub status: u16,
    pub content_type: String,
    pub headers: HeaderMap,
    pub bytes: Vec<u8>,
}

impl FetchedPage {
    /// The media type without parameters, lowercased.
    pub fn media_type(&self) -> String {
        media_type(&self.content_type)
    }

    /// The body classified for transcoding. Text is decoded lossily. A caller
    /// that needs the exact bytes reads `bytes` instead.
    pub fn body(&self) -> FetchBody {
        let main = self.media_type();
        if main == "text/html" || main == "application/xhtml+xml" {
            FetchBody::Html(String::from_utf8_lossy(&self.bytes).into_owned())
        } else if main.starts_with("text/") {
            FetchBody::Text(String::from_utf8_lossy(&self.bytes).into_owned())
        } else {
            FetchBody::Other
        }
    }
}

#[derive(Debug)]
pub enum FetchBody {
    Html(String),
    Text(String),
    /// A type the gateway does not transcode (image, binary, ...).
    Other,
}

#[derive(Debug, thiserror::Error)]
pub enum ProxyError {
    #[error("invalid URL: {0}")]
    Url(String),
    #[error("destination not allowed: {0}")]
    Blocked(String),
    #[error("response larger than {0} bytes")]
    TooLarge(usize),
    #[error("request failed: {0}")]
    Request(#[from] reqwest::Error),
}

/// The media type of a `Content-Type` value without its parameters, lowercased.
pub fn media_type(content_type: &str) -> String {
    content_type
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase()
}

/// Whether the gateway may connect to `ip`. Everything that could reach the
/// gateway host, its containers or the operator's network is refused, as is
/// all of IPv6.
pub fn ip_allowed(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => ipv4_allowed(v4),
        IpAddr::V6(_) => false,
    }
}

fn ipv4_allowed(ip: Ipv4Addr) -> bool {
    let o = ip.octets();
    let this_network = o[0] == 0;
    let carrier_nat = o[0] == 100 && (o[1] & 0xC0) == 64;
    let ietf_assignments = o[0] == 192 && o[1] == 0 && o[2] == 0;
    let reserved = o[0] >= 240;
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_broadcast()
        || ip.is_unspecified()
        || ip.is_multicast()
        || ip.is_documentation()
        || this_network
        || carrier_nat
        || ietf_assignments
        || reserved)
}

/// Check a URL before it is fetched or followed. A literal IP host is checked
/// here. A host name is checked when it resolves.
fn check_url(url: &Url) -> Result<(), ProxyError> {
    if !matches!(url.scheme(), "http" | "https") {
        return Err(ProxyError::Url(url.to_string()));
    }
    match url.host() {
        Some(Host::Domain(_)) => Ok(()),
        Some(Host::Ipv4(ip)) if ipv4_allowed(ip) => Ok(()),
        Some(Host::Ipv4(ip)) => Err(ProxyError::Blocked(ip.to_string())),
        Some(Host::Ipv6(ip)) => Err(ProxyError::Blocked(ip.to_string())),
        None => Err(ProxyError::Url(url.to_string())),
    }
}

/// Resolver that drops every address `ip_allowed` refuses, so a name that
/// points into the gateway's network fails to resolve rather than connecting.
/// Filtering at resolution time also defeats a name that changes its answer
/// between a check and the connect.
struct FilteringResolver;

#[derive(Debug)]
struct NoAllowedAddress(String);

impl std::fmt::Display for NoAllowedAddress {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "destination not allowed: {}", self.0)
    }
}

impl std::error::Error for NoAllowedAddress {}

impl Resolve for FilteringResolver {
    fn resolve(&self, name: Name) -> Resolving {
        Box::pin(async move {
            let host = name.as_str().to_string();
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await?
                .filter(|a| ip_allowed(a.ip()))
                .collect();
            if addrs.is_empty() {
                return Err(
                    Box::new(NoAllowedAddress(host)) as Box<dyn std::error::Error + Send + Sync>
                );
            }
            Ok(Box::new(addrs.into_iter()) as Addrs)
        })
    }
}

/// The request headers worth sending on to the origin: everything the handset
/// sent except connection-level headers and the ones the fetch sets itself.
pub fn forwardable_request_headers(src: &HeaderMap) -> HeaderMap {
    strip(src, &[HOP_BY_HOP, REQUEST_OWNED])
}

/// The response headers worth sending back to the handset: everything the
/// origin sent except connection-level headers and the wire-encoding ones.
pub fn forwardable_response_headers(src: &HeaderMap) -> HeaderMap {
    strip(src, &[HOP_BY_HOP, RESPONSE_OWNED])
}

fn strip(src: &HeaderMap, drop: &[&[&str]]) -> HeaderMap {
    // Names listed in `Connection` are hop-by-hop for this message too.
    let listed: Vec<String> = src
        .get_all(CONNECTION)
        .iter()
        .filter_map(|v| v.to_str().ok())
        .flat_map(|v| v.split(','))
        .map(|n| n.trim().to_ascii_lowercase())
        .collect();
    let dropped = |name: &HeaderName| {
        let n = name.as_str();
        drop.iter().any(|set| set.contains(&n)) || listed.iter().any(|l| l == n)
    };
    let mut out = HeaderMap::new();
    for (name, value) in src {
        if !dropped(name) {
            out.append(name.clone(), value.clone());
        }
    }
    out
}

/// The outbound HTTP client.
pub struct Proxy {
    client: reqwest::Client,
}

impl Proxy {
    pub fn new(user_agent: &str, timeout: Duration) -> Result<Self, ProxyError> {
        let client = reqwest::Client::builder()
            .user_agent(user_agent.to_string())
            .timeout(timeout)
            .dns_resolver(Arc::new(FilteringResolver))
            .redirect(redirect::Policy::custom(|attempt| {
                if attempt.previous().len() >= MAX_REDIRECTS {
                    return attempt.error("too many redirects");
                }
                match check_url(attempt.url()) {
                    Ok(()) => attempt.follow(),
                    Err(e) => attempt.error(e),
                }
            }))
            .build()?;
        Ok(Proxy { client })
    }

    /// Fetch a URL with the GET method, forwarding `headers` to the origin.
    pub async fn get(&self, url: &str, headers: &HeaderMap) -> Result<FetchedPage, ProxyError> {
        let url = parse_url(url)?;
        let resp = self
            .client
            .get(url)
            .headers(forwardable_request_headers(headers))
            .send()
            .await
            .map_err(classify)?;
        read_response(resp).await
    }

    /// POST an entity to a URL, forwarding `headers` (including the entity's
    /// `Content-Type`) to the origin.
    pub async fn post(
        &self,
        url: &str,
        headers: &HeaderMap,
        body: Vec<u8>,
    ) -> Result<FetchedPage, ProxyError> {
        let url = parse_url(url)?;
        let resp = self
            .client
            .post(url)
            .headers(forwardable_request_headers(headers))
            .body(body)
            .send()
            .await
            .map_err(classify)?;
        read_response(resp).await
    }
}

/// A refusal raised inside the resolver or the redirect policy arrives
/// wrapped in the client's connect error. Report it as the block it is.
fn classify(e: reqwest::Error) -> ProxyError {
    let mut src = std::error::Error::source(&e);
    while let Some(cause) = src {
        if let Some(blocked) = cause.downcast_ref::<NoAllowedAddress>() {
            return ProxyError::Blocked(blocked.0.clone());
        }
        if let Some(ProxyError::Blocked(dest)) = cause.downcast_ref::<ProxyError>() {
            return ProxyError::Blocked(dest.clone());
        }
        src = cause.source();
    }
    ProxyError::Request(e)
}

fn parse_url(url: &str) -> Result<Url, ProxyError> {
    let parsed = Url::parse(url).map_err(|_| ProxyError::Url(url.to_string()))?;
    check_url(&parsed)?;
    Ok(parsed)
}

async fn read_response(mut resp: reqwest::Response) -> Result<FetchedPage, ProxyError> {
    let final_url = resp.url().to_string();
    let status = resp.status().as_u16();
    let headers = resp.headers().clone();
    let content_type = headers
        .get(CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/octet-stream")
        .to_string();

    if let Some(len) = resp.content_length()
        && len > MAX_BODY_BYTES as u64
    {
        return Err(ProxyError::TooLarge(MAX_BODY_BYTES));
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = resp.chunk().await? {
        if bytes.len() + chunk.len() > MAX_BODY_BYTES {
            return Err(ProxyError::TooLarge(MAX_BODY_BYTES));
        }
        bytes.extend_from_slice(&chunk);
    }

    Ok(FetchedPage {
        final_url,
        status,
        content_type,
        headers,
        bytes,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use reqwest::header::HeaderValue;

    #[test]
    fn private_loopback_and_reserved_ipv4_are_refused() {
        for ip in [
            "127.0.0.1",
            "10.1.2.3",
            "172.16.0.9",
            "192.168.1.1",
            "169.254.10.10",
            "100.64.0.1",
            "0.0.0.0",
            "224.0.0.1",
            "255.255.255.255",
            "192.0.2.5",
            "240.0.0.1",
        ] {
            let ip: IpAddr = ip.parse().unwrap();
            assert!(!ip_allowed(ip), "{ip} should be refused");
        }
    }

    #[test]
    fn public_ipv4_is_allowed_and_all_ipv6_is_refused() {
        assert!(ip_allowed("93.184.216.34".parse().unwrap()));
        assert!(ip_allowed("8.8.8.8".parse().unwrap()));
        for ip in ["::1", "2001:db8::1", "2606:4700::1111", "::ffff:8.8.8.8"] {
            let ip: IpAddr = ip.parse().unwrap();
            assert!(!ip_allowed(ip), "{ip} should be refused");
        }
    }

    #[test]
    fn literal_ip_hosts_are_checked_before_the_fetch() {
        assert!(matches!(
            parse_url("http://127.0.0.1:13000/status"),
            Err(ProxyError::Blocked(_))
        ));
        assert!(matches!(
            parse_url("http://[::1]/"),
            Err(ProxyError::Blocked(_))
        ));
        assert!(matches!(
            parse_url("ftp://example.com/"),
            Err(ProxyError::Url(_))
        ));
        assert!(parse_url("http://example.com/").is_ok());
        assert!(parse_url("http://8.8.8.8/").is_ok());
    }

    #[test]
    fn hop_by_hop_and_owned_headers_are_stripped() {
        let mut h = HeaderMap::new();
        h.insert("cookie", HeaderValue::from_static("a=1"));
        h.insert("user-agent", HeaderValue::from_static("UP.Browser/4.1"));
        h.insert("host", HeaderValue::from_static("wapbox"));
        h.insert("content-length", HeaderValue::from_static("3"));
        h.insert("proxy-connection", HeaderValue::from_static("keep-alive"));
        h.insert("connection", HeaderValue::from_static("x-drop-me"));
        h.insert("x-drop-me", HeaderValue::from_static("1"));
        h.insert("content-type", HeaderValue::from_static("text/plain"));
        let out = forwardable_request_headers(&h);
        assert_eq!(out.get("cookie").unwrap(), "a=1");
        assert_eq!(out.get("user-agent").unwrap(), "UP.Browser/4.1");
        assert_eq!(out.get("content-type").unwrap(), "text/plain");
        for gone in [
            "host",
            "content-length",
            "proxy-connection",
            "connection",
            "x-drop-me",
        ] {
            assert!(out.get(gone).is_none(), "{gone} should be stripped");
        }

        let mut r = HeaderMap::new();
        r.append("set-cookie", HeaderValue::from_static("s=1"));
        r.append("set-cookie", HeaderValue::from_static("t=2"));
        r.insert("content-encoding", HeaderValue::from_static("gzip"));
        r.insert("content-length", HeaderValue::from_static("10"));
        r.insert("transfer-encoding", HeaderValue::from_static("chunked"));
        r.insert("cache-control", HeaderValue::from_static("no-store"));
        let out = forwardable_response_headers(&r);
        assert_eq!(out.get_all("set-cookie").iter().count(), 2);
        assert_eq!(out.get("cache-control").unwrap(), "no-store");
        for gone in ["content-encoding", "content-length", "transfer-encoding"] {
            assert!(out.get(gone).is_none(), "{gone} should be stripped");
        }
    }

    #[test]
    fn body_classification_follows_the_media_type() {
        let page = |ctype: &str, body: &[u8]| FetchedPage {
            final_url: "http://x/".into(),
            status: 200,
            content_type: ctype.into(),
            headers: HeaderMap::new(),
            bytes: body.to_vec(),
        };
        assert!(matches!(
            page("text/html; charset=utf-8", b"<p>").body(),
            FetchBody::Html(_)
        ));
        assert!(matches!(
            page("TEXT/PLAIN", b"x").body(),
            FetchBody::Text(_)
        ));
        assert!(matches!(page("image/gif", b"GIF").body(), FetchBody::Other));
        // Raw bytes are kept exactly, whatever the classification says.
        let latin1 = page("text/vnd.wap.wml; charset=iso-8859-1", b"<p>\xe5</p>");
        assert_eq!(latin1.bytes, b"<p>\xe5</p>");
    }
}
