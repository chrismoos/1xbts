//! Edge MSISDN resolver — HTTP sidecar called by nginx auth_request.
//! See README.md.

use std::env;
use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;

use axum::Router;
use axum::extract::{Query, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::IntoResponse;
use axum::routing::get;
use log::{error, info, warn};
use serde::Deserialize;
use tonic::transport::{Channel, Endpoint};

pub mod pdsn_management {
    pub mod v1 {
        tonic::include_proto!("pdsn_management.v1");
    }
}

// Server stubs need the imported packet.v1 module visible.
pub mod packet {
    pub mod v1 {
        tonic::include_proto!("packet.v1");
    }
}

use pdsn_management::v1::GetPdsnSessionByIpRequest;
use pdsn_management::v1::pdsn_management_service_client::PdsnManagementServiceClient;

/// Header name we set on the resolver response. nginx copies it to the
/// upstream request via `auth_request_set $msisdn $upstream_http_x_1xbts_msisdn`.
const MSISDN_HEADER: &str = "X-1xBTS-MSISDN";

#[derive(Clone)]
struct ResolverState {
    pdsn: PdsnManagementServiceClient<Channel>,
}

#[derive(Debug, Default, Deserialize)]
#[serde(default)]
struct ResolveParams {
    /// Source IP of the upstream request, as `$remote_addr` from nginx.
    ip: Option<String>,
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let bind: SocketAddr = env::var("BIND_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:8088".to_string())
        .parse()?;
    let mgmt_addr = env::var("MGMT_GRPC_ADDR").unwrap_or_else(|_| "127.0.0.1:17016".to_string());
    let mgmt_endpoint = if mgmt_addr.starts_with("http://") || mgmt_addr.starts_with("https://") {
        mgmt_addr.clone()
    } else {
        format!("http://{mgmt_addr}")
    };

    info!("edge-resolver: connecting to BSC PdsnManagementService at {mgmt_endpoint}");
    let channel = Endpoint::from_shared(mgmt_endpoint.clone())?.connect_lazy();
    let pdsn = PdsnManagementServiceClient::new(channel);

    let state = Arc::new(ResolverState { pdsn });
    let app = Router::new()
        .route("/_msisdn", get(handle_resolve))
        .route("/healthz", get(handle_healthz))
        .with_state(state);

    info!("edge-resolver: listening on {bind}");
    let listener = tokio::net::TcpListener::bind(bind).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn handle_healthz() -> impl IntoResponse {
    (StatusCode::OK, "ok\n")
}

async fn handle_resolve(
    State(state): State<Arc<ResolverState>>,
    Query(params): Query<ResolveParams>,
) -> impl IntoResponse {
    let msisdn = resolve_msisdn(&state, &params).await;
    let mut headers = HeaderMap::new();
    // Always emit the header, even when empty, so nginx's
    // auth_request_set captures a defined value rather than the
    // upstream module's missing-variable warning.
    let value = HeaderValue::from_str(&msisdn).unwrap_or_else(|_| HeaderValue::from_static(""));
    headers.insert(MSISDN_HEADER, value);
    (StatusCode::OK, headers)
}

async fn resolve_msisdn(state: &ResolverState, params: &ResolveParams) -> String {
    let raw = match params.ip.as_deref() {
        Some(s) if !s.is_empty() => s,
        _ => {
            warn!("edge-resolver: missing `ip` query parameter; returning empty MSISDN");
            return String::new();
        }
    };
    // Accept any IP we can parse. We don't restrict to the MS subnet
    // here — if PDSN doesn't know it, we'll just return empty.
    let ip = match raw.parse::<IpAddr>() {
        Ok(ip) => ip,
        Err(e) => {
            warn!("edge-resolver: invalid ip query `{raw}`: {e}; returning empty MSISDN");
            return String::new();
        }
    };

    let mut client = state.pdsn.clone();
    let request = GetPdsnSessionByIpRequest {
        peer_ip: ip.to_string(),
    };
    match client.get_pdsn_session_by_ip(request).await {
        Ok(response) => match response.into_inner().session {
            Some(session) if !session.phone_number.is_empty() => {
                info!(
                    "edge-resolver: {} -> {} (session_id={})",
                    ip, session.phone_number, session.session_id
                );
                session.phone_number
            }
            Some(_) => {
                info!("edge-resolver: {} -> known session but no phone_number", ip);
                String::new()
            }
            None => {
                info!("edge-resolver: {} -> no active session", ip);
                String::new()
            }
        },
        Err(status) => {
            error!("edge-resolver: PDSN gRPC error for {ip}: {status}");
            String::new()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use packet::v1::{GetSessionByIpResponse, PacketSessionInfo};
    use pdsn_management::v1::pdsn_management_service_server::{
        PdsnManagementService, PdsnManagementServiceServer,
    };
    use pdsn_management::v1::{
        GetPdsnSessionRequest, PdsnSessionList, SetPacketTraceCaptureRequest,
    };
    use std::sync::Mutex;
    use tonic::transport::Server;
    use tonic::{Request, Response, Status};

    enum MockBehavior {
        Hit(Box<PacketSessionInfo>),
        Miss,
        GrpcError,
    }

    struct MockPdsn {
        last_request: Mutex<Option<String>>,
        behavior: Mutex<MockBehavior>,
    }

    impl MockPdsn {
        fn new(behavior: MockBehavior) -> Self {
            Self {
                last_request: Mutex::new(None),
                behavior: Mutex::new(behavior),
            }
        }
    }

    #[async_trait]
    impl PdsnManagementService for MockPdsn {
        async fn list_pdsn_sessions(
            &self,
            _: Request<()>,
        ) -> Result<Response<PdsnSessionList>, Status> {
            Err(Status::unimplemented(""))
        }

        async fn get_pdsn_session(
            &self,
            _: Request<GetPdsnSessionRequest>,
        ) -> Result<Response<packet::v1::GetSessionStatusResponse>, Status> {
            Err(Status::unimplemented(""))
        }

        async fn get_pdsn_session_by_ip(
            &self,
            request: Request<GetPdsnSessionByIpRequest>,
        ) -> Result<Response<GetSessionByIpResponse>, Status> {
            let req = request.into_inner();
            *self.last_request.lock().unwrap() = Some(req.peer_ip.clone());
            let behavior = self.behavior.lock().unwrap();
            match &*behavior {
                MockBehavior::Hit(session) => Ok(Response::new(GetSessionByIpResponse {
                    session: Some((**session).clone()),
                })),
                MockBehavior::Miss => Ok(Response::new(GetSessionByIpResponse { session: None })),
                MockBehavior::GrpcError => Err(Status::internal("simulated gRPC failure")),
            }
        }

        async fn set_packet_trace_capture(
            &self,
            _: Request<SetPacketTraceCaptureRequest>,
        ) -> Result<Response<packet::v1::SetSessionCaptureResponse>, Status> {
            Err(Status::unimplemented(""))
        }
    }

    async fn start_pdsn(behavior: MockBehavior) -> (Arc<MockPdsn>, SocketAddr) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let pdsn = Arc::new(MockPdsn::new(behavior));
        let pdsn_clone = Arc::clone(&pdsn);
        tokio::spawn(async move {
            struct Svc(Arc<MockPdsn>);
            #[async_trait]
            impl PdsnManagementService for Svc {
                async fn list_pdsn_sessions(
                    &self,
                    request: Request<()>,
                ) -> Result<Response<PdsnSessionList>, Status> {
                    self.0.list_pdsn_sessions(request).await
                }
                async fn get_pdsn_session(
                    &self,
                    request: Request<GetPdsnSessionRequest>,
                ) -> Result<Response<packet::v1::GetSessionStatusResponse>, Status>
                {
                    self.0.get_pdsn_session(request).await
                }
                async fn get_pdsn_session_by_ip(
                    &self,
                    request: Request<GetPdsnSessionByIpRequest>,
                ) -> Result<Response<GetSessionByIpResponse>, Status> {
                    self.0.get_pdsn_session_by_ip(request).await
                }
                async fn set_packet_trace_capture(
                    &self,
                    request: Request<SetPacketTraceCaptureRequest>,
                ) -> Result<Response<packet::v1::SetSessionCaptureResponse>, Status>
                {
                    self.0.set_packet_trace_capture(request).await
                }
            }
            let _ = Server::builder()
                .add_service(PdsnManagementServiceServer::new(Svc(pdsn_clone)))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
                .await;
        });
        (pdsn, addr)
    }

    async fn start_resolver(pdsn_addr: SocketAddr) -> SocketAddr {
        let endpoint = Endpoint::from_shared(format!("http://{pdsn_addr}"))
            .unwrap()
            .connect_lazy();
        let state = Arc::new(ResolverState {
            pdsn: PdsnManagementServiceClient::new(endpoint),
        });
        let app = Router::new()
            .route("/_msisdn", get(handle_resolve))
            .route("/healthz", get(handle_healthz))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        addr
    }

    struct HttpResp {
        status: u16,
        msisdn_header: String,
    }
    async fn get_msisdn(resolver: SocketAddr, ip_query: &str) -> HttpResp {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let path = if ip_query.is_empty() {
            "/_msisdn".to_string()
        } else {
            format!("/_msisdn?ip={}", ip_query)
        };
        let mut stream = tokio::net::TcpStream::connect(resolver).await.unwrap();
        let req = format!("GET {path} HTTP/1.1\r\nHost: x\r\nConnection: close\r\n\r\n");
        stream.write_all(req.as_bytes()).await.unwrap();
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.unwrap();
        let text = String::from_utf8_lossy(&buf).into_owned();
        let head = text.split("\r\n\r\n").next().unwrap_or("");
        let status: u16 = head
            .lines()
            .next()
            .unwrap_or("")
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        let msisdn_header = head
            .lines()
            .find_map(|l| {
                let lower = l.to_ascii_lowercase();
                if lower.starts_with("x-1xbts-msisdn:") {
                    Some(
                        l.split_once(':')
                            .map(|x| x.1)
                            .unwrap_or("")
                            .trim()
                            .to_string(),
                    )
                } else {
                    None
                }
            })
            .unwrap_or_default();
        HttpResp {
            status,
            msisdn_header,
        }
    }

    fn session_with(peer_ip: &str, phone_number: &str) -> PacketSessionInfo {
        PacketSessionInfo {
            session_id: "test-session".to_string(),
            phase: "active".to_string(),
            peer_ip: peer_ip.to_string(),
            phone_number: phone_number.to_string(),
            ..Default::default()
        }
    }

    #[tokio::test]
    async fn resolves_header_for_known_ip() {
        let (pdsn, pdsn_addr) = start_pdsn(MockBehavior::Hit(Box::new(session_with(
            "10.55.0.2",
            "15551234567",
        ))))
        .await;
        let resolver_addr = start_resolver(pdsn_addr).await;

        let resp = get_msisdn(resolver_addr, "10.55.0.2").await;
        assert_eq!(resp.status, 200);
        assert_eq!(resp.msisdn_header, "15551234567");
        assert_eq!(
            pdsn.last_request.lock().unwrap().as_deref(),
            Some("10.55.0.2")
        );
    }

    #[tokio::test]
    async fn returns_empty_header_for_unknown_ip() {
        let (_pdsn, pdsn_addr) = start_pdsn(MockBehavior::Miss).await;
        let resolver_addr = start_resolver(pdsn_addr).await;

        let resp = get_msisdn(resolver_addr, "10.55.0.99").await;
        assert_eq!(resp.status, 200);
        assert_eq!(resp.msisdn_header, "");
    }

    #[tokio::test]
    async fn returns_empty_header_on_invalid_ip_query() {
        let (_pdsn, pdsn_addr) = start_pdsn(MockBehavior::Miss).await;
        let resolver_addr = start_resolver(pdsn_addr).await;

        let resp = get_msisdn(resolver_addr, "not-an-ip").await;
        assert_eq!(resp.status, 200);
        assert_eq!(resp.msisdn_header, "");
    }

    #[tokio::test]
    async fn returns_empty_header_on_missing_ip_query() {
        let (_pdsn, pdsn_addr) = start_pdsn(MockBehavior::Miss).await;
        let resolver_addr = start_resolver(pdsn_addr).await;

        let resp = get_msisdn(resolver_addr, "").await;
        assert_eq!(resp.status, 200);
        assert_eq!(resp.msisdn_header, "");
    }

    #[tokio::test]
    async fn returns_empty_header_on_pdsn_grpc_error() {
        let (_pdsn, pdsn_addr) = start_pdsn(MockBehavior::GrpcError).await;
        let resolver_addr = start_resolver(pdsn_addr).await;

        let resp = get_msisdn(resolver_addr, "10.55.0.2").await;
        assert_eq!(resp.status, 200);
        assert_eq!(resp.msisdn_header, "");
    }
}
