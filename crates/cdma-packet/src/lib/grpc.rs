use cdma_rpc_error::{Domain, ErrorReason};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;
use tonic::{Request, Response, Status};

use crate::fou_tcp_transport::{FouTcpSessionTransport, FouTcpTunnel};
use crate::fou_transport::{FouSessionTransport, FouTunnel};
use crate::ip_allocator::{IpAllocator, SubnetIpAllocator};
use crate::ip_transport::IpTransportConfig;
use crate::proto::packet_service_server::PacketService;
use crate::proto::{
    AccessTechnology, GetSessionByIpRequest, GetSessionByIpResponse, GetSessionStatusRequest,
    GetSessionStatusResponse, ListSessionsRequest, ListSessionsResponse, MnIdSource,
    PacketSessionDetail, PacketSessionInfo, PacketTraceEvent as ProtoPacketTraceEvent,
    SetSessionCaptureRequest, SetSessionCaptureResponse,
};
use crate::session_lifecycle::{NullSink, SessionLifecycleSink};
use crate::session_task::{PppSessionStore, SessionMetadata, SessionStatus};
use crate::tun_transport::TunTransport;

const PPP_SESSION_REAPER_MAX_INTERVAL: Duration = Duration::from_secs(60);

struct PacketSessionEntry {
    uplink_tx: mpsc::Sender<Vec<u8>>,
    shutdown_tx: oneshot::Sender<()>,
    status: Arc<Mutex<SessionStatus>>,
    task_handle: JoinHandle<()>,
}

#[derive(Clone)]
pub struct PacketServiceImpl {
    sessions: Arc<Mutex<HashMap<String, PacketSessionEntry>>>,
    transport_config: IpTransportConfig,
    fou_tunnel: Option<Arc<FouTunnel>>,
    fou_tcp_tunnel: Option<Arc<FouTcpTunnel>>,
    allocator: Arc<dyn IpAllocator>,
    lifecycle_sink: Arc<dyn SessionLifecycleSink>,
    ppp_session_store: Option<Arc<PppSessionStore>>,
    ppp_session_timeout: Duration,
}

impl PacketServiceImpl {
    pub fn new(
        transport_config: IpTransportConfig,
        fou_tunnel: Option<Arc<FouTunnel>>,
        fou_tcp_tunnel: Option<Arc<FouTcpTunnel>>,
    ) -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            transport_config,
            fou_tunnel,
            fou_tcp_tunnel,
            allocator: Arc::new(SubnetIpAllocator::default_subnet()),
            lifecycle_sink: Arc::new(NullSink),
            ppp_session_store: None,
            ppp_session_timeout: Duration::ZERO,
        }
    }

    pub fn with_allocator(
        transport_config: IpTransportConfig,
        fou_tunnel: Option<Arc<FouTunnel>>,
        fou_tcp_tunnel: Option<Arc<FouTcpTunnel>>,
        allocator: Arc<dyn IpAllocator>,
    ) -> Self {
        Self {
            sessions: Arc::new(Mutex::new(HashMap::new())),
            transport_config,
            fou_tunnel,
            fou_tcp_tunnel,
            allocator,
            lifecycle_sink: Arc::new(NullSink),
            ppp_session_store: None,
            ppp_session_timeout: Duration::ZERO,
        }
    }

    /// Attaches a lifecycle sink. The sink fires on session bind (after IP
    /// allocation) and unbind. Calling this replaces any previously
    /// installed sink.
    pub fn with_lifecycle_sink(mut self, sink: Arc<dyn SessionLifecycleSink>) -> Self {
        self.lifecycle_sink = sink;
        self
    }

    pub fn with_ppp_session_cache(mut self, timeout: Duration) -> Self {
        let store = Arc::new(PppSessionStore::new());
        spawn_ppp_session_reaper(store.clone(), Arc::clone(&self.allocator), timeout);
        self.ppp_session_store = Some(store);
        self.ppp_session_timeout = timeout;
        self
    }

    fn create_transport(&self) -> Box<dyn crate::ip_transport::IpTransport> {
        match &self.transport_config {
            IpTransportConfig::Tun { nat_interface } => {
                Box::new(TunTransport::new(nat_interface.clone()))
            }
            IpTransportConfig::Fou { .. } => {
                let tunnel = self
                    .fou_tunnel
                    .as_ref()
                    .expect("FOU tunnel not initialized");
                Box::new(FouSessionTransport::new(tunnel.clone()))
            }
            IpTransportConfig::FouTcp { .. } => {
                let tunnel = self
                    .fou_tcp_tunnel
                    .as_ref()
                    .expect("FOU TCP tunnel not initialized");
                Box::new(FouTcpSessionTransport::new(tunnel.clone()))
            }
        }
    }

    /// Open an HRPD A10 Unstructured Byte Stream session and return the A10
    /// bearer payload channels directly.
    ///
    /// The returned uplink sender accepts decapsulated A10 `0x8881`
    /// byte-stream payload octets from the PCF. The returned downlink receiver
    /// emits byte-stream payload octets to be keyed-GRE encapsulated toward the
    /// PCF. HRPD Default Packet RLP wrapping stays on the AN/BTS air side.
    pub fn open_pdsn_a10_byte_stream_session_direct(
        &self,
        session_id: String,
        service_option: u32,
        metadata: SessionMetadata,
    ) -> Result<(mpsc::Sender<Vec<u8>>, mpsc::Receiver<Vec<u8>>), String> {
        {
            let mut sessions = self.sessions.lock().unwrap();
            if sessions
                .get(&session_id)
                .is_some_and(|entry| entry.task_handle.is_finished())
            {
                sessions.remove(&session_id);
            }
            if sessions.contains_key(&session_id) {
                return Err(format!("packet session {} already exists", session_id));
            }
        }

        let (uplink_tx, uplink_rx) = mpsc::channel(256);
        let (downlink_tx, downlink_rx) = mpsc::channel(256);
        let (shutdown_tx, shutdown_rx) = oneshot::channel();
        let status = Arc::new(Mutex::new(SessionStatus::new(
            service_option,
            metadata.clone(),
        )));
        let sid = session_id.clone();
        let so = service_option;
        let transport = self.create_transport();
        let alloc = Arc::clone(&self.allocator);
        let sink = Arc::clone(&self.lifecycle_sink);
        let ppp_store = self.ppp_session_store.clone();
        let ppp_timeout = self.ppp_session_timeout;
        let task_status = Arc::clone(&status);
        let task_handle = tokio::spawn(async move {
            crate::hrpd_session_task::run_hrpd_a10_byte_stream_session(
                sid,
                so,
                transport,
                uplink_rx,
                downlink_tx,
                shutdown_rx,
                task_status,
                alloc,
                metadata,
                sink,
                ppp_store,
                ppp_timeout,
            )
            .await;
        });

        {
            let mut sessions = self.sessions.lock().unwrap();
            sessions.insert(
                session_id.clone(),
                PacketSessionEntry {
                    uplink_tx: uplink_tx.clone(),
                    shutdown_tx,
                    status,
                    task_handle,
                },
            );
        }

        log::info!(
            "packet-service: opened HRPD A10 byte-stream session {} (SO {})",
            session_id,
            service_option
        );

        Ok((uplink_tx, downlink_rx))
    }

    /// Closes a session and waits up to 5 s for its task to exit. The task's
    /// cleanup fires the lifecycle sink's `on_unbound`, so aborting it would
    /// drop the unbind event.
    pub async fn close_session_direct(&self, session_id: &str) {
        let entry = {
            let mut sessions = self.sessions.lock().unwrap();
            sessions.remove(session_id)
        };
        if let Some(entry) = entry {
            close_session_entry(session_id, entry).await;
        }
    }

    /// Close an in-process HRPD A10 Unstructured Byte Stream session.
    pub async fn close_pdsn_a10_byte_stream_session_direct(&self, session_id: &str) {
        self.close_session_direct(session_id).await;
    }

    /// Get a snapshot of session info for a given session.
    pub fn get_session_info(&self, session_id: &str) -> Option<PacketSessionInfo> {
        let sessions = self.sessions.lock().unwrap();
        sessions.get(session_id).map(|entry| {
            let status = entry.status.lock().unwrap();
            to_proto_session_info(session_id, &status)
        })
    }

    pub fn get_session_detail(&self, session_id: &str) -> Option<PacketSessionDetail> {
        let sessions = self.sessions.lock().unwrap();
        sessions.get(session_id).map(|entry| {
            let status = entry.status.lock().unwrap();
            to_proto_session_detail(session_id, &status)
        })
    }

    /// List all sessions.
    pub fn list_all_sessions(&self) -> Vec<PacketSessionInfo> {
        let sessions = self.sessions.lock().unwrap();
        sessions
            .iter()
            .map(|(id, entry)| {
                let status = entry.status.lock().unwrap();
                to_proto_session_info(id, &status)
            })
            .collect()
    }

    pub fn set_session_capture(
        &self,
        session_id: &str,
        enabled: bool,
    ) -> Option<PacketSessionDetail> {
        let sessions = self.sessions.lock().unwrap();
        let entry = sessions.get(session_id)?;
        let mut status = entry.status.lock().unwrap();
        status.set_capture_enabled(enabled);
        Some(to_proto_session_detail(session_id, &status))
    }

    pub fn find_session_by_peer_ip(&self, peer_ip: &str) -> Option<PacketSessionInfo> {
        let sessions = self.sessions.lock().unwrap();
        for (id, entry) in sessions.iter() {
            let status = entry.status.lock().unwrap();
            let info = to_proto_session_info(id, &status);
            if info.peer_ip == peer_ip {
                return Some(info);
            }
        }
        None
    }
}

async fn close_session_entry(session_id: &str, entry: PacketSessionEntry) {
    drop(entry.uplink_tx);
    let _ = entry.shutdown_tx.send(());

    match tokio::time::timeout(std::time::Duration::from_secs(5), entry.task_handle).await {
        Ok(Ok(())) => {
            log::info!("packet-service: closed session {}", session_id);
        }
        Ok(Err(err)) => {
            log::warn!(
                "packet-service: session {} task panicked on close: {}",
                session_id,
                err
            );
        }
        Err(_) => {
            log::warn!(
                "packet-service: session {} task did not exit within 5s",
                session_id
            );
        }
    }
}

fn spawn_ppp_session_reaper(
    store: Arc<PppSessionStore>,
    allocator: Arc<dyn IpAllocator>,
    timeout: Duration,
) {
    if timeout.is_zero() {
        return;
    }

    let interval = ppp_session_reaper_interval(timeout);
    let Ok(handle) = tokio::runtime::Handle::try_current() else {
        log::warn!(
            "packet-service: PPP session cache reaper not started because no Tokio runtime is active"
        );
        return;
    };

    handle.spawn(async move {
        let mut ticker = tokio::time::interval(interval);
        ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            ticker.tick().await;
            for expired in store.reap_expired(timeout) {
                log::info!(
                    "packet-service: PPP cache reaper expired identity={} peer_ip={} idle_secs={} allocation_key={}",
                    expired.identity_key,
                    expired.peer_ip,
                    expired.idle_for.as_secs(),
                    expired.allocation_key
                );
                allocator.release(&expired.allocation_key);
            }
        }
    });
}

fn ppp_session_reaper_interval(timeout: Duration) -> Duration {
    timeout
        .min(PPP_SESSION_REAPER_MAX_INTERVAL)
        .max(Duration::from_secs(1))
}

fn to_proto_trace_event(event: &crate::engine::PacketTraceEvent) -> ProtoPacketTraceEvent {
    ProtoPacketTraceEvent {
        timestamp_ms: event.timestamp_ms,
        layer: event.layer.clone(),
        direction: event.direction.clone(),
        summary: event.summary.clone(),
        detail: event.detail.clone(),
        payload_hex: event.payload_hex.clone(),
    }
}

/// Map the domain access-technology label to the wire enum.
fn access_technology_to_proto(label: &str) -> i32 {
    let tech = match label {
        "HRPD" => AccessTechnology::Hrpd,
        "1x" => AccessTechnology::Cdma1x,
        _ => AccessTechnology::Unspecified,
    };
    tech as i32
}

/// Map the domain MN-ID source label to the wire enum.
fn mn_id_source_to_proto(label: &str) -> i32 {
    let source = match label {
        "a11" => MnIdSource::A11,
        "derived_hardware" => MnIdSource::DerivedHardware,
        _ => MnIdSource::Unspecified,
    };
    source as i32
}

/// Map the wire MN-ID source enum back to the domain label, treating the
/// unspecified variant as absent.
fn to_proto_session_info(session_id: &str, status: &SessionStatus) -> PacketSessionInfo {
    PacketSessionInfo {
        session_id: session_id.to_string(),
        phase: status.phase.clone(),
        service_option: status.service_option,
        peer_ip: status.peer_ip.clone(),
        our_ip: status.our_ip.clone(),
        tun_device: status.tun_device.clone(),
        uplink_frames: status.uplink_frames,
        downlink_frames: status.downlink_frames,
        uplink_bytes: status.uplink_bytes,
        downlink_bytes: status.downlink_bytes,
        created_at_ms: status.created_at_ms,
        last_phase_change_at_ms: status.last_phase_change_at_ms,
        last_uplink_at_ms: status.last_uplink_at_ms,
        last_downlink_at_ms: status.last_downlink_at_ms,
        last_activity_at_ms: status.last_activity_at_ms,
        last_uplink_rate_bps: status.last_uplink_rate_bps,
        last_downlink_rate_bps: status.last_downlink_rate_bps,
        mobile_address: status.mobile_address.clone(),
        subscriber_id: status.subscriber_id.clone(),
        phone_number: status.phone_number.clone(),
        imsi: status.imsi.clone(),
        esn: status.esn,
        meid: status.meid.clone(),
        mn_id: status.mn_id.clone(),
        mn_id_source: mn_id_source_to_proto(&status.mn_id_source),
        subscriber_imsi: status.subscriber_imsi.clone(),
        nai: status.nai.clone(),
        pcf_session_id: status.a10.map(|a10| a10.pcf_session_id),
        mn_session_reference_id: status.a10.map(|a10| u32::from(a10.mn_session_reference_id)),
        traffic_walsh_code: status.traffic_walsh_code,
        rlp_state: status.rlp_state.clone(),
        lcp_state: status.lcp_state.clone(),
        ipcp_state: status.ipcp_state.clone(),
        capture_enabled: status.capture_enabled,
        access_technology: access_technology_to_proto(&status.access_technology),
    }
}

fn to_proto_session_detail(session_id: &str, status: &SessionStatus) -> PacketSessionDetail {
    PacketSessionDetail {
        summary: Some(to_proto_session_info(session_id, status)),
        last_rx_control: status.last_rx_control.clone(),
        last_tx_control: status.last_tx_control.clone(),
        last_rx_control_repeats: status.last_rx_control_repeats,
        last_tx_control_repeats: status.last_tx_control_repeats,
        recent_ppp_events: status
            .recent_ppp_events
            .iter()
            .map(to_proto_trace_event)
            .collect(),
        capture_events: status
            .capture_events
            .iter()
            .map(to_proto_trace_event)
            .collect(),
    }
}

#[tonic::async_trait]
impl PacketService for PacketServiceImpl {
    async fn get_session_status(
        &self,
        request: Request<GetSessionStatusRequest>,
    ) -> Result<Response<GetSessionStatusResponse>, Status> {
        let req = request.into_inner();
        let info = self
            .get_session_detail(&req.session_id)
            .ok_or_else(|| session_not_found(&req.session_id))?;

        Ok(Response::new(GetSessionStatusResponse {
            session: Some(info),
        }))
    }

    async fn list_sessions(
        &self,
        _request: Request<ListSessionsRequest>,
    ) -> Result<Response<ListSessionsResponse>, Status> {
        let sessions = self.list_all_sessions();
        Ok(Response::new(ListSessionsResponse { sessions }))
    }

    async fn get_session_by_ip(
        &self,
        request: Request<GetSessionByIpRequest>,
    ) -> Result<Response<GetSessionByIpResponse>, Status> {
        let peer_ip = request.into_inner().peer_ip;
        let session = self.find_session_by_peer_ip(&peer_ip);
        Ok(Response::new(GetSessionByIpResponse { session }))
    }

    async fn set_session_capture(
        &self,
        request: Request<SetSessionCaptureRequest>,
    ) -> Result<Response<SetSessionCaptureResponse>, Status> {
        let req = request.into_inner();
        let session = self
            .set_session_capture(&req.session_id, req.enabled)
            .ok_or_else(|| session_not_found(&req.session_id))?;

        Ok(Response::new(SetSessionCaptureResponse {
            session: Some(session),
        }))
    }
}

const PACKET: Domain = Domain("packet");

fn session_not_found(session_id: &str) -> Status {
    PACKET
        .not_found(
            ErrorReason::PacketSessionNotFound,
            "The packet session has ended or does not exist",
        )
        .with("session_id", session_id)
        .status()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ppp_session_reaper_interval_is_bounded() {
        assert_eq!(
            ppp_session_reaper_interval(Duration::from_millis(500)),
            Duration::from_secs(1)
        );
        assert_eq!(
            ppp_session_reaper_interval(Duration::from_secs(30)),
            Duration::from_secs(30)
        );
        assert_eq!(
            ppp_session_reaper_interval(Duration::from_secs(1800)),
            Duration::from_secs(60)
        );
    }

    #[tokio::test]
    async fn hrpd_a10_sessions_are_visible_in_common_session_list() {
        let service = PacketServiceImpl::new(
            crate::ip_transport::IpTransportConfig::FouTcp {
                remote_addr: "127.0.0.1:1".parse().unwrap(),
            },
            None,
            Some(crate::fou_tcp_transport::FouTcpTunnel::new(
                "127.0.0.1:1".parse().unwrap(),
            )),
        );
        let session_id = "hrpd-a10-test".to_string();
        let (_uplink_tx, _downlink_rx) = service
            .open_pdsn_a10_byte_stream_session_direct(
                session_id.clone(),
                u32::from(cdma_common::consts::SERVICE_OPTION_PACKET_DATA_RC3),
                SessionMetadata {
                    access_technology: "HRPD".to_string(),
                    mobile_address: "hrpd-uati-session:0000002a".to_string(),
                    subscriber_id: None,
                    phone_number: String::new(),
                    imsi: None,
                    esn: None,
                    meid: None,
                    mn_id: Some("310556898017332".to_string()),
                    mn_id_source: Some("a11".to_string()),
                    subscriber_imsi: None,
                    a10: Some(crate::session_task::A10Reference {
                        pcf_session_id: 42,
                        mn_session_reference_id: 1,
                    }),
                    traffic_walsh_code: 42,
                },
            )
            .expect("open HRPD A10 session");

        let listed = service
            .list_all_sessions()
            .into_iter()
            .find(|session| session.session_id == session_id)
            .expect("HRPD A10 session should be in common list");
        assert_eq!(listed.access_technology(), AccessTechnology::Hrpd);
        assert_eq!(listed.imsi, "");
        assert_eq!(listed.mn_id, "310556898017332");
        assert_eq!(listed.mn_id_source(), MnIdSource::A11);
        assert_eq!(listed.pcf_session_id, Some(42));
        assert_eq!(listed.mn_session_reference_id, Some(1));

        for _ in 0..20 {
            let ready = service
                .get_session_info(&session_id)
                .is_some_and(|session| !session.peer_ip.is_empty());
            if ready {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        let detail = service
            .get_session_detail(&session_id)
            .expect("HRPD A10 detail should be available");
        let summary = detail.summary.expect("detail should have summary");
        assert_eq!(summary.access_technology(), AccessTechnology::Hrpd);
        assert!(!summary.peer_ip.is_empty());
        assert_eq!(
            service
                .find_session_by_peer_ip(&summary.peer_ip)
                .expect("peer IP lookup should include HRPD sessions")
                .session_id,
            session_id
        );
        service
            .close_pdsn_a10_byte_stream_session_direct(&session_id)
            .await;
    }
}
