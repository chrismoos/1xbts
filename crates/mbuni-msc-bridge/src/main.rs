//! Mbuni → MSC bridge: a Kannel-compatible `sendsms` HTTP shim that forwards
//! SMS submissions from Mbuni's MMSC into the MSC management gRPC service.
//! See README.md.

use std::env;
use std::net::SocketAddr;
use std::sync::Arc;

use axum::Router;
use axum::extract::{Request, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::routing::get;
use log::{error, info, warn};
use tonic::transport::{Channel, Endpoint};

// Module layout mirrors the proto package hierarchy so tonic-generated
// cross-package references (`crate::events::v1::...`, `crate::bsc::v1::...`)
// resolve.
pub mod events {
    pub mod v1 {
        tonic::include_proto!("events.v1");
    }
}

pub mod msc_management {
    pub mod v1 {
        tonic::include_proto!("msc_management.v1");
    }
}

pub mod bsc {
    pub mod v1 {
        tonic::include_proto!("bsc.v1");
    }
}

use msc_management::v1::SendSmsRequest;
use msc_management::v1::msc_management_service_client::MscManagementServiceClient;

/// C.S0015-B teleservice ID for Wireless Application Protocol (WAP).
/// Used to deliver MMS M-Notification.ind to handsets per WAP-259 §6.5.
const TELESERVICE_WAP: u32 = 0x1004;

/// WSP connectionless push port. Destination port handsets listen on for
/// WAP Push (Service Indication / MMS notification).
const DEFAULT_WSP_DST_PORT: u16 = 0x0B84;

/// Maximum single-segment payload in the bearer-data User Data subparameter.
/// F-PCH Data Bursts and the C.S0015-B PARAMETER_LEN byte both cap the
/// user-data sub-parameter at 255 octets; we reserve some headroom for the
/// MSG_ENCODING/NUM_FIELDS bit prefix and round down.
const MAX_USER_DATA_BYTES: usize = 240;

#[derive(Clone)]
struct BridgeState {
    msc: MscManagementServiceClient<Channel>,
}

/// Kannel-compatible sendsms parameters parsed from the raw query string
/// rather than via `serde_urlencoded`, because Mbuni's MMSC URL-encodes
/// the binary WAP Push PDU into the `text` field. axum's `Query<String>`
/// uses lossy UTF-8 decoding and would replace non-ASCII bytes with
/// U+FFFD, destroying the payload.
#[derive(Debug, Default)]
struct SendSmsParams {
    from: Option<Vec<u8>>,
    to: Option<Vec<u8>>,
    text: Option<Vec<u8>>,
    coding: Option<u32>,
    udh: Option<Vec<u8>>,
    data: Option<Vec<u8>>,
}

fn parse_query(raw: &str) -> SendSmsParams {
    let mut out = SendSmsParams::default();
    for pair in raw.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (k, v) = pair.split_once('=').unwrap_or((pair, ""));
        let value = percent_decode_bytes(v);
        match k {
            "from" => out.from = Some(value),
            "to" => out.to = Some(value),
            "text" => out.text = Some(value),
            "udh" => out.udh = Some(value),
            "data" => out.data = Some(value),
            "coding" => {
                out.coding = std::str::from_utf8(&value)
                    .ok()
                    .and_then(|s| s.parse::<u32>().ok());
            }
            // username/password and any other Kannel fields are accepted
            // and ignored — bridge is loopback-only.
            _ => {}
        }
    }
    out
}

/// Percent-decode a URL component to raw bytes, replacing `+` with space
/// per `application/x-www-form-urlencoded`. Invalid `%XX` escapes are
/// passed through literally.
fn percent_decode_bytes(s: &str) -> Vec<u8> {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => {
                out.push(b' ');
                i += 1;
            }
            b'%' if i + 2 < bytes.len() => {
                let hex = &bytes[i + 1..i + 3];
                match (hex_nibble(hex[0]), hex_nibble(hex[1])) {
                    (Some(h), Some(l)) => {
                        out.push((h << 4) | l);
                        i += 3;
                    }
                    _ => {
                        out.push(bytes[i]);
                        i += 1;
                    }
                }
            }
            b => {
                out.push(b);
                i += 1;
            }
        }
    }
    out
}

fn hex_nibble(b: u8) -> Option<u8> {
    match b {
        b'0'..=b'9' => Some(b - b'0'),
        b'a'..=b'f' => Some(b - b'a' + 10),
        b'A'..=b'F' => Some(b - b'A' + 10),
        _ => None,
    }
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let bind: SocketAddr = env::var("BIND_ADDR")
        .unwrap_or_else(|_| "127.0.0.1:8081".to_string())
        .parse()?;
    let msc_addr = env::var("MSC_GRPC_ADDR").unwrap_or_else(|_| "127.0.0.1:17017".to_string());
    let msc_endpoint = if msc_addr.starts_with("http://") || msc_addr.starts_with("https://") {
        msc_addr.clone()
    } else {
        format!("http://{msc_addr}")
    };

    info!("mbuni-msc-bridge: connecting to MSC at {msc_endpoint}");
    let channel = Endpoint::from_shared(msc_endpoint.clone())?.connect_lazy();
    let msc = MscManagementServiceClient::new(channel);

    let state = Arc::new(BridgeState { msc });
    let app = Router::new()
        .route("/cgi-bin/sendsms", get(handle_sendsms).post(handle_sendsms))
        .route("/sendsms", get(handle_sendsms).post(handle_sendsms))
        .route("/healthz", get(handle_healthz))
        .with_state(state);

    info!("mbuni-msc-bridge: listening on {bind}");
    let listener = tokio::net::TcpListener::bind(bind).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

async fn handle_healthz() -> impl IntoResponse {
    (StatusCode::OK, "ok\n")
}

async fn handle_sendsms(
    State(state): State<Arc<BridgeState>>,
    request: Request,
) -> impl IntoResponse {
    let raw_query = request.uri().query().unwrap_or("").to_string();
    let params = parse_query(&raw_query);

    let from = params
        .from
        .as_deref()
        .map(|b| String::from_utf8_lossy(b).into_owned())
        .unwrap_or_default();
    let to = match params.to.as_deref() {
        Some(b) if !b.is_empty() => String::from_utf8_lossy(b).into_owned(),
        _ => return kannel_error("Missing recipient (to)"),
    };

    // A non-empty UDH always implies binary content (GSM convention), and
    // Mbuni's MMSC sends WAP Push notifications that way without ever
    // setting `coding=1`. Route on either signal so the PDU never lands
    // in the `text` TEXT column where embedded NULs would fail to insert.
    let coding = params.coding.unwrap_or(0);
    let udh_present = params.udh.as_deref().is_some_and(|s| !s.is_empty());
    let is_binary = coding == 1 || udh_present;
    let (text, raw_user_data, teleservice_id) = if is_binary {
        // Binary: Kannel-style request from Mbuni's MMSC. Parse the
        // GSM-style UDH (if any) for the WSP source/destination ports,
        // then re-frame per WAP-259 §6.5 for CDMA IS-637 SMS:
        //   MSG_TYPE(1) TOTAL_SEGMENTS(1) SEGMENT_NUMBER(1)
        //   SOURCE_PORT(2) DESTINATION_PORT(2) DATA(N)
        // No GSM UDH on the wire — CDMA WAP teleservice carries WDP
        // ports inline in the User Data subparameter.
        // Kannel-proper sends `udh` as a hex-encoded ASCII string
        // ("0605040B..."); Mbuni's MMSC sends the same UDH as raw
        // percent-encoded bytes (after our percent_decode_bytes, the
        // payload starts with byte 0x06 which is not a hex char). Try
        // hex first; if it doesn't parse, treat the bytes as raw UDH.
        let udh_bytes = decode_hex_or_raw(params.udh.as_deref());
        // Same for `data`. Mbuni doesn't send it at all — it packs the
        // WAP Push PDU into `text` — so also fall back to `text` if
        // both are empty.
        let data_bytes = decode_hex_or_raw(params.data.as_deref());
        let data_bytes = if data_bytes.is_empty() {
            params.text.clone().unwrap_or_default()
        } else {
            data_bytes
        };
        if data_bytes.is_empty() {
            return kannel_error("Binary message requires data or text");
        }
        let (src_port, dst_port) = match extract_wsp_ports(&udh_bytes) {
            Ok(ports) => ports,
            Err(e) => return kannel_error(&e),
        };
        let framed = build_wdp_user_data(src_port, dst_port, &data_bytes);
        if framed.len() > MAX_USER_DATA_BYTES {
            return kannel_error(&format!(
                "Framed WAP Push payload {} bytes exceeds single-segment limit {}",
                framed.len(),
                MAX_USER_DATA_BYTES,
            ));
        }
        (String::new(), Some(framed), Some(TELESERVICE_WAP))
    } else if coding == 0 || coding == 2 {
        let text_bytes = params.text.unwrap_or_default();
        if text_bytes.is_empty() {
            return kannel_error("Missing message text");
        }
        // Coding 0/2 is genuine text; UTF-8-decode (lossily, for robustness
        // against GSM 7-bit escapes Mbuni may have already converted).
        (
            String::from_utf8_lossy(&text_bytes).into_owned(),
            None,
            None,
        )
    } else {
        return kannel_error(&format!("Unsupported coding={coding}"));
    };

    // Recipient is a phone number — Mbuni's `to` parameter is the MSISDN.
    // The MSC resolves it through HLR.
    let request = SendSmsRequest {
        originating_number: from,
        text,
        timeout_ms: None,
        destination: Some(
            msc_management::v1::send_sms_request::Destination::DestinationNumber(to.clone()),
        ),
        teleservice_id,
        raw_user_data,
    };

    let mut client = state.msc.clone();
    match client.send_sms(request).await {
        Ok(response) => {
            let resp = response.into_inner();
            if resp.accepted {
                info!("mbuni-msc-bridge: dispatched to={to} ({})", resp.message);
                (
                    StatusCode::ACCEPTED,
                    "0: Accepted for delivery\n".to_string(),
                )
            } else {
                warn!(
                    "mbuni-msc-bridge: MSC rejected delivery to={to}: {}",
                    resp.message
                );
                kannel_error(&format!("MSC rejected: {}", resp.message))
            }
        }
        Err(status) => {
            error!("mbuni-msc-bridge: MSC gRPC error: {status}");
            kannel_error(&format!("MSC error: {status}"))
        }
    }
}

/// Build the User Data subparameter octet payload for a single-segment
/// WAP Push per WAP-259 §6.5.1/§6.5.2.
fn build_wdp_user_data(src_port: u16, dst_port: u16, data: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(7 + data.len());
    out.push(0x00); // MSG_TYPE = WDP
    out.push(0x01); // TOTAL_SEGMENTS = 1
    out.push(0x00); // SEGMENT_NUMBER = 0
    out.extend_from_slice(&src_port.to_be_bytes());
    out.extend_from_slice(&dst_port.to_be_bytes());
    out.extend_from_slice(data);
    out
}

/// Extract (source_port, destination_port) from a GSM-style SMS UDH containing
/// IEI 0x05 "Application port addressing, 16-bit address" per 3GPP 23.040.
/// When `udh` is empty, defaults to (0, DEFAULT_WSP_DST_PORT).
///
/// UDH layout for the 16-bit port IE:
///   UDHL(1) IEI=0x05(1) IEDL=0x04(1) dst_hi(1) dst_lo(1) src_hi(1) src_lo(1)
/// UDH may contain other IEs before/after; we scan for IEI 0x05.
fn extract_wsp_ports(udh: &[u8]) -> Result<(u16, u16), String> {
    if udh.is_empty() {
        return Ok((0, DEFAULT_WSP_DST_PORT));
    }
    if udh.len() < 2 {
        return Err("UDH too short".to_string());
    }
    let udhl = udh[0] as usize;
    if udhl + 1 != udh.len() {
        return Err(format!(
            "UDH length byte {} does not match UDH size {}",
            udhl,
            udh.len() - 1,
        ));
    }
    // Walk IEs starting after the UDHL byte.
    let mut p = 1;
    while p + 2 <= udh.len() {
        let iei = udh[p];
        let iedl = udh[p + 1] as usize;
        let start = p + 2;
        let end = start + iedl;
        if end > udh.len() {
            return Err("UDH IE truncated".to_string());
        }
        if iei == 0x05 && iedl == 4 {
            let dst = u16::from_be_bytes([udh[start], udh[start + 1]]);
            let src = u16::from_be_bytes([udh[start + 2], udh[start + 3]]);
            return Ok((src, dst));
        }
        p = end;
    }
    // No 16-bit port IE found — fall back to the WAP Push default destination.
    Ok((0, DEFAULT_WSP_DST_PORT))
}

fn decode_hex_opt(s: Option<&str>) -> Result<Vec<u8>, hex::FromHexError> {
    let s = match s {
        Some(s) if !s.is_empty() => s,
        _ => return Ok(Vec::new()),
    };
    let cleaned: String = s.chars().filter(|c| !c.is_whitespace()).collect();
    hex::decode(cleaned)
}

/// Decode a value that is either a hex-encoded ASCII string (Kannel
/// convention) or already raw percent-decoded bytes (Mbuni convention).
/// Returns the raw byte form for downstream consumers.
fn decode_hex_or_raw(b: Option<&[u8]>) -> Vec<u8> {
    let b = match b {
        Some(b) if !b.is_empty() => b,
        _ => return Vec::new(),
    };
    if let Ok(s) = std::str::from_utf8(b) {
        if let Ok(decoded) = decode_hex_opt(Some(s)) {
            return decoded;
        }
    }
    b.to_vec()
}

fn kannel_error(detail: &str) -> (StatusCode, String) {
    warn!("mbuni-msc-bridge: rejecting request: {detail}");
    (StatusCode::BAD_REQUEST, format!("Rejected: {detail}\n"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;
    use msc_management::v1::msc_management_service_server::{
        MscManagementService, MscManagementServiceServer,
    };
    use msc_management::v1::{
        CallList, SendSmsRequest as ProtoSendSmsRequest, SendSmsResponse,
        send_sms_request::Destination,
    };
    use std::sync::Mutex;
    use tonic::transport::Server;
    use tonic::{Request, Response, Status};

    #[derive(Default)]
    struct CapturingMsc {
        last_request: Mutex<Option<ProtoSendSmsRequest>>,
    }

    #[async_trait]
    impl MscManagementService for CapturingMsc {
        async fn initiate_call(
            &self,
            _: Request<bsc::v1::InitiateCallRequest>,
        ) -> Result<Response<bsc::v1::InitiateCallResponse>, Status> {
            Err(Status::unimplemented(""))
        }

        async fn send_sms(
            &self,
            request: Request<ProtoSendSmsRequest>,
        ) -> Result<Response<SendSmsResponse>, Status> {
            let req = request.into_inner();
            *self.last_request.lock().unwrap() = Some(req);
            Ok(Response::new(SendSmsResponse {
                accepted: true,
                message: "sms_id=00000000-0000-0000-0000-000000000001".to_string(),
            }))
        }

        async fn list_calls(&self, _: Request<()>) -> Result<Response<CallList>, Status> {
            Err(Status::unimplemented(""))
        }

        type StreamOtaspEventsStream = std::pin::Pin<
            Box<
                dyn tokio_stream::Stream<Item = Result<events::v1::MscNetworkEvent, Status>>
                    + Send
                    + 'static,
            >,
        >;

        async fn stream_otasp_events(
            &self,
            _: Request<()>,
        ) -> Result<Response<Self::StreamOtaspEventsStream>, Status> {
            Err(Status::unimplemented(""))
        }
    }

    async fn start_servers() -> (Arc<CapturingMsc>, SocketAddr) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let msc_addr = listener.local_addr().unwrap();
        let msc = Arc::new(CapturingMsc::default());
        let msc_clone = Arc::clone(&msc);

        tokio::spawn(async move {
            struct Svc(Arc<CapturingMsc>);
            #[async_trait]
            impl MscManagementService for Svc {
                async fn initiate_call(
                    &self,
                    request: Request<bsc::v1::InitiateCallRequest>,
                ) -> Result<Response<bsc::v1::InitiateCallResponse>, Status> {
                    self.0.initiate_call(request).await
                }
                async fn send_sms(
                    &self,
                    request: Request<ProtoSendSmsRequest>,
                ) -> Result<Response<SendSmsResponse>, Status> {
                    self.0.send_sms(request).await
                }
                async fn list_calls(
                    &self,
                    request: Request<()>,
                ) -> Result<Response<CallList>, Status> {
                    self.0.list_calls(request).await
                }
                type StreamOtaspEventsStream = std::pin::Pin<
                    Box<
                        dyn tokio_stream::Stream<Item = Result<events::v1::MscNetworkEvent, Status>>
                            + Send
                            + 'static,
                    >,
                >;
                async fn stream_otasp_events(
                    &self,
                    request: Request<()>,
                ) -> Result<Response<Self::StreamOtaspEventsStream>, Status> {
                    self.0.stream_otasp_events(request).await
                }
            }
            let _ = Server::builder()
                .add_service(MscManagementServiceServer::new(Svc(msc_clone)))
                .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
                .await;
        });
        (msc, msc_addr)
    }

    async fn start_bridge(msc_addr: SocketAddr) -> SocketAddr {
        let endpoint = Endpoint::from_shared(format!("http://{msc_addr}"))
            .unwrap()
            .connect_lazy();
        let state = Arc::new(BridgeState {
            msc: MscManagementServiceClient::new(endpoint),
        });
        let app = Router::new()
            .route("/cgi-bin/sendsms", get(handle_sendsms).post(handle_sendsms))
            .route("/healthz", get(handle_healthz))
            .with_state(state);
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });
        addr
    }

    #[tokio::test]
    async fn binary_wap_push_uses_wap_259_framing() {
        let (msc, msc_addr) = start_servers().await;
        let bridge_addr = start_bridge(msc_addr).await;

        // Kannel-style UDH for 16-bit WSP ports per 3GPP 23.040 IEI 0x05:
        // UDHL=06 IEI=05 IEDL=04 dst_hi=0B dst_lo=84 src_hi=23 src_lo=F0
        let udh = "0605040B8423F0";
        let data = "01060304AE84B486C39500";
        let url = format!(
            "http://{bridge_addr}/cgi-bin/sendsms?from=1234&to=5551212&coding=1&udh={udh}&data={data}"
        );
        let resp = reqwest_get(&url).await;
        assert_eq!(resp.status, 202, "body={}", resp.body);

        for _ in 0..50 {
            if msc.last_request.lock().unwrap().is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let captured = msc.last_request.lock().unwrap().clone().expect("request");
        assert_eq!(captured.teleservice_id, Some(TELESERVICE_WAP));

        // Expected framed payload per WAP-259 §6.5.1/§6.5.2:
        //   00 (MSG_TYPE=WDP) 01 (TOTAL_SEGMENTS) 00 (SEGMENT_NUMBER)
        //   23 F0 (SOURCE_PORT) 0B 84 (DESTINATION_PORT) ... DATA
        let mut expected: Vec<u8> = vec![0x00, 0x01, 0x00, 0x23, 0xF0, 0x0B, 0x84];
        expected.extend_from_slice(&hex::decode(data).unwrap());
        assert_eq!(captured.raw_user_data.as_deref(), Some(&expected[..]));
        assert!(matches!(
            captured.destination,
            Some(Destination::DestinationNumber(ref n)) if n == "5551212"
        ));
    }

    #[tokio::test]
    async fn binary_wap_push_without_udh_defaults_to_wsp_port() {
        let (msc, msc_addr) = start_servers().await;
        let bridge_addr = start_bridge(msc_addr).await;

        let data = "AABBCC";
        let url = format!("http://{bridge_addr}/cgi-bin/sendsms?from=1&to=2&coding=1&data={data}");
        let resp = reqwest_get(&url).await;
        assert_eq!(resp.status, 202, "body={}", resp.body);
        for _ in 0..50 {
            if msc.last_request.lock().unwrap().is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let captured = msc.last_request.lock().unwrap().clone().expect("request");
        assert_eq!(
            captured.raw_user_data.as_deref(),
            Some(&[0x00, 0x01, 0x00, 0x00, 0x00, 0x0B, 0x84, 0xAA, 0xBB, 0xCC][..]),
        );
        assert_eq!(captured.teleservice_id, Some(TELESERVICE_WAP));
    }

    #[tokio::test]
    async fn binary_request_with_raw_percent_encoded_udh() {
        // Mbuni sends the UDH as raw percent-encoded bytes
        // (%06%05%04%0B%84%23%F0), not as a hex-encoded ASCII string.
        // After percent-decoding the first byte is 0x06 — not a hex
        // char — so a strict hex parse fails; the bridge must fall back
        // to treating the bytes as the raw UDH directly.
        let (msc, msc_addr) = start_servers().await;
        let bridge_addr = start_bridge(msc_addr).await;

        let url = format!(
            "http://{bridge_addr}/cgi-bin/sendsms?from=1&to=5551212&udh=%06%05%04%0B%84%23%F0&text=%01%06%03%00"
        );
        let resp = reqwest_get(&url).await;
        assert_eq!(resp.status, 202, "body={}", resp.body);
        for _ in 0..50 {
            if msc.last_request.lock().unwrap().is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let captured = msc.last_request.lock().unwrap().clone().expect("request");
        assert_eq!(captured.teleservice_id, Some(TELESERVICE_WAP));
        let expected: &[u8] = &[
            0x00, 0x01, 0x00, 0x23, 0xF0, 0x0B, 0x84, 0x01, 0x06, 0x03, 0x00,
        ];
        assert_eq!(captured.raw_user_data.as_deref(), Some(expected));
    }

    #[tokio::test]
    async fn binary_request_via_text_param_with_udh_routes_through_binary_path() {
        // Mbuni's MMSC sends the WAP Push PDU in the `text` query param
        // with raw percent-encoded bytes (including NULs) and never sets
        // `coding=1`. The bridge must still treat it as binary so the
        // PDU doesn't end up in the SMSC `text TEXT` column.
        let (msc, msc_addr) = start_servers().await;
        let bridge_addr = start_bridge(msc_addr).await;

        let udh = "0605040B8423F0";
        // Percent-encoded raw PDU bytes including a NUL.
        let url = format!(
            "http://{bridge_addr}/cgi-bin/sendsms?from=1&to=5551212&udh={udh}&text=%01%06%03%00%AA"
        );
        let resp = reqwest_get(&url).await;
        assert_eq!(resp.status, 202, "body={}", resp.body);
        for _ in 0..50 {
            if msc.last_request.lock().unwrap().is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let captured = msc.last_request.lock().unwrap().clone().expect("request");
        assert_eq!(captured.teleservice_id, Some(TELESERVICE_WAP));
        assert_eq!(captured.text, "");
        let expected: &[u8] = &[
            0x00, 0x01, 0x00, 0x23, 0xF0, 0x0B, 0x84, 0x01, 0x06, 0x03, 0x00, 0xAA,
        ];
        assert_eq!(captured.raw_user_data.as_deref(), Some(expected));
    }

    #[tokio::test]
    async fn text_request_uses_default_teleservice() {
        let (msc, msc_addr) = start_servers().await;
        let bridge_addr = start_bridge(msc_addr).await;

        let url =
            format!("http://{bridge_addr}/cgi-bin/sendsms?from=12025550100&to=5551212&text=hello");
        let resp = reqwest_get(&url).await;
        assert_eq!(resp.status, 202);
        for _ in 0..50 {
            if msc.last_request.lock().unwrap().is_some() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
        let captured = msc.last_request.lock().unwrap().clone().expect("request");
        assert_eq!(captured.teleservice_id, None);
        assert_eq!(captured.raw_user_data, None);
        assert_eq!(captured.text, "hello");
    }

    #[tokio::test]
    async fn missing_recipient_returns_400() {
        let (_msc, msc_addr) = start_servers().await;
        let bridge_addr = start_bridge(msc_addr).await;
        let url = format!("http://{bridge_addr}/cgi-bin/sendsms?from=1&text=x");
        let resp = reqwest_get(&url).await;
        assert_eq!(resp.status, 400);
    }

    #[test]
    fn extract_wsp_ports_parses_kannel_udh() {
        // UDHL=06 IEI=05 IEDL=04 dst=0B84 src=23F0
        let udh = [0x06, 0x05, 0x04, 0x0B, 0x84, 0x23, 0xF0];
        assert_eq!(extract_wsp_ports(&udh).unwrap(), (0x23F0, 0x0B84));
    }

    #[test]
    fn extract_wsp_ports_defaults_when_empty() {
        assert_eq!(extract_wsp_ports(&[]).unwrap(), (0, DEFAULT_WSP_DST_PORT));
    }

    #[test]
    fn extract_wsp_ports_rejects_bad_udhl() {
        let udh = [0x05, 0x05, 0x04, 0x00];
        assert!(extract_wsp_ports(&udh).is_err());
    }

    // Tiny GET helper that uses tokio + std net to avoid pulling in reqwest.
    struct HttpResp {
        status: u16,
        body: String,
    }
    async fn reqwest_get(url: &str) -> HttpResp {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let url = url.strip_prefix("http://").unwrap();
        let (host_port, path) = url.split_once('/').unwrap();
        let path = format!("/{}", path);
        let mut stream = tokio::net::TcpStream::connect(host_port).await.unwrap();
        let req = format!("GET {path} HTTP/1.1\r\nHost: {host_port}\r\nConnection: close\r\n\r\n");
        stream.write_all(req.as_bytes()).await.unwrap();
        let mut buf = Vec::new();
        stream.read_to_end(&mut buf).await.unwrap();
        let text = String::from_utf8_lossy(&buf).into_owned();
        let (head, body) = text.split_once("\r\n\r\n").unwrap_or((&text, ""));
        let status_line = head.lines().next().unwrap_or("");
        let status: u16 = status_line
            .split_whitespace()
            .nth(1)
            .and_then(|s| s.parse().ok())
            .unwrap_or(0);
        HttpResp {
            status,
            body: body.to_string(),
        }
    }
}
