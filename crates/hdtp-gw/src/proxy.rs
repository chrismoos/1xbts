//! Outbound HTTP fetch for the gateway.
//!
//! A handset Get/Post names an absolute URL; the gateway fetches it over
//! ordinary HTTP(S), following redirects, and classifies the result so the
//! caller can transcode HTML, wrap plain text, or report an unsupported type.

use std::time::Duration;

/// Maximum response body the gateway will read. A handset renders a few lines;
/// anything larger is transcoded from a truncated prefix.
const MAX_BODY_BYTES: usize = 512 * 1024;

/// A fetched resource, classified by content type.
#[derive(Debug)]
pub struct FetchedPage {
    pub final_url: String,
    pub content_type: String,
    pub body: FetchBody,
}

#[derive(Debug)]
pub enum FetchBody {
    Html(String),
    Text(String),
    /// A type the gateway does not transcode (image, binary, ...).
    Other {
        bytes: Vec<u8>,
    },
}

#[derive(Debug, thiserror::Error)]
pub enum ProxyError {
    #[error("invalid URL: {0}")]
    Url(String),
    #[error("request failed: {0}")]
    Request(#[from] reqwest::Error),
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
            .build()?;
        Ok(Proxy { client })
    }

    /// Fetch a URL with the GET method.
    pub async fn get(&self, url: &str) -> Result<FetchedPage, ProxyError> {
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err(ProxyError::Url(url.to_string()));
        }
        let resp = self.client.get(url).send().await?;
        self.read_response(resp).await
    }

    /// POST an entity to a URL.
    pub async fn post(&self, url: &str, body: Vec<u8>) -> Result<FetchedPage, ProxyError> {
        if !(url.starts_with("http://") || url.starts_with("https://")) {
            return Err(ProxyError::Url(url.to_string()));
        }
        let resp = self.client.post(url).body(body).send().await?;
        self.read_response(resp).await
    }

    async fn read_response(&self, resp: reqwest::Response) -> Result<FetchedPage, ProxyError> {
        let final_url = resp.url().to_string();
        let content_type = resp
            .headers()
            .get(reqwest::header::CONTENT_TYPE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or("application/octet-stream")
            .to_string();

        let bytes = resp.bytes().await?;
        let bytes = if bytes.len() > MAX_BODY_BYTES {
            bytes.slice(0..MAX_BODY_BYTES)
        } else {
            bytes
        };

        let ctype_main = content_type
            .split(';')
            .next()
            .unwrap_or("")
            .trim()
            .to_ascii_lowercase();
        let body = if ctype_main == "text/html" || ctype_main == "application/xhtml+xml" {
            FetchBody::Html(String::from_utf8_lossy(&bytes).into_owned())
        } else if ctype_main.starts_with("text/") {
            FetchBody::Text(String::from_utf8_lossy(&bytes).into_owned())
        } else {
            FetchBody::Other {
                bytes: bytes.to_vec(),
            }
        };

        Ok(FetchedPage {
            final_url,
            content_type,
            body,
        })
    }
}
