use std::collections::HashMap;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio_stream::{Stream, StreamExt};
use tonic::{Request, Response, Status, Streaming};

use crate::fou_tcp_transport::{FouTcpSessionTransport, FouTcpTunnel};
use crate::fou_transport::{FouSessionTransport, FouTunnel};
use crate::ip_allocator::{IpAllocator, SubnetIpAllocator};
use crate::ip_transport::IpTransportConfig;
use crate::proto::packet_service_server::PacketService;
use crate::proto::{
    CloseSessionRequest, CloseSessionResponse, GetSessionStatusRequest, GetSessionStatusResponse,
    ListSessionsRequest, ListSessionsResponse, OpenSessionRequest, OpenSessionResponse,
    PacketSessionDetail, PacketSessionInfo, PacketTraceEvent as ProtoPacketTraceEvent,
    SessionFrame, SetSchActiveRequest, SetSchActiveResponse, SetSessionCaptureRequest,
    SetSessionCaptureResponse,
};
use crate::session_task::{self, SessionControl, SessionMetadata, SessionStatus};
use crate::tun_transport::TunTransport;

#[allow(dead_code)]
struct SessionEntry {
    uplink_tx: mpsc::Sender<SessionFrame>,
    downlink_rx: Arc<Mutex<Option<mpsc::Receiver<SessionFrame>>>>,
    status: Arc<Mutex<SessionStatus>>,
    task_handle: JoinHandle<()>,
    service_option: u32,
    /// Out-of-band control sender — F-SCH activation, etc. The session task
    /// selects on this alongside the bearer channels.
    control_tx: mpsc::Sender<SessionControl>,
}

#[derive(Clone)]
pub struct PacketServiceImpl {
    sessions: Arc<Mutex<HashMap<String, SessionEntry>>>,
    transport_config: IpTransportConfig,
    fou_tunnel: Option<Arc<FouTunnel>>,
    fou_tcp_tunnel: Option<Arc<FouTcpTunnel>>,
    allocator: Arc<dyn IpAllocator>,
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
        }
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

    /// Open a session and return the bearer channels directly (for in-process use).
    /// The gRPC StreamSession RPC provides the same functionality over the network.
    pub fn open_session_direct(
        &self,
        session_id: String,
        service_option: u32,
        metadata: SessionMetadata,
    ) -> Result<(mpsc::Sender<SessionFrame>, mpsc::Receiver<SessionFrame>), String> {
        // Check for duplicate
        {
            let sessions = self.sessions.lock().unwrap();
            if sessions.contains_key(&session_id) {
                return Err(format!("session {} already exists", session_id));
            }
        }

        let (uplink_tx, uplink_rx) = mpsc::channel(256);
        let (downlink_tx, downlink_rx) = mpsc::channel(256);
        let (control_tx, control_rx) = mpsc::channel::<SessionControl>(8);

        let status = Arc::new(Mutex::new(SessionStatus::new(service_option, metadata)));
        let status_clone = status.clone();
        let sid = session_id.clone();
        let so = service_option;

        let transport = self.create_transport();
        let alloc = Arc::clone(&self.allocator);
        let task_handle = tokio::spawn(async move {
            session_task::run_session(
                sid,
                so,
                transport,
                uplink_rx,
                downlink_tx,
                status_clone,
                alloc,
                control_rx,
            )
            .await;
        });

        let entry = SessionEntry {
            uplink_tx: uplink_tx.clone(),
            downlink_rx: Arc::new(Mutex::new(None)), // consumed by direct caller
            status,
            task_handle,
            service_option,
            control_tx,
        };

        {
            let mut sessions = self.sessions.lock().unwrap();
            sessions.insert(session_id, entry);
        }

        Ok((uplink_tx, downlink_rx))
    }

    /// Toggle F-SCH downlink generation on a running session. Sends a
    /// `SessionControl::SetSchActive` to the session task; the actual flip
    /// of `PacketSession::sch_active` happens inside the task. Returns
    /// `Err` if the session is unknown or its control channel is closed
    /// (which only happens if the session task has already exited).
    pub async fn set_session_sch_active(
        &self,
        session_id: &str,
        active: bool,
        rate_bps: u32,
    ) -> Result<(), String> {
        let control_tx = {
            let sessions = self.sessions.lock().unwrap();
            sessions
                .get(session_id)
                .map(|entry| entry.control_tx.clone())
                .ok_or_else(|| format!("session {} not found", session_id))?
        };
        control_tx
            .send(SessionControl::SetSchActive { active, rate_bps })
            .await
            .map_err(|e| format!("session {} control send failed: {e}", session_id))
    }

    /// Close a session (for in-process use).
    pub fn close_session_direct(&self, session_id: &str) {
        let entry = {
            let mut sessions = self.sessions.lock().unwrap();
            sessions.remove(session_id)
        };
        if let Some(entry) = entry {
            drop(entry.uplink_tx);
            entry.task_handle.abort();
            log::info!("packet-service: closed session {}", session_id);
        }
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
        traffic_walsh_code: status.traffic_walsh_code,
        rlp_state: status.rlp_state.clone(),
        lcp_state: status.lcp_state.clone(),
        ipcp_state: status.ipcp_state.clone(),
        capture_enabled: status.capture_enabled,
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
    async fn open_session(
        &self,
        request: Request<OpenSessionRequest>,
    ) -> Result<Response<OpenSessionResponse>, Status> {
        let req = request.into_inner();
        let session_id = req.session_id.clone();

        // Check for duplicate
        {
            let sessions = self.sessions.lock().unwrap();
            if sessions.contains_key(&session_id) {
                return Err(Status::already_exists(format!(
                    "session {} already exists",
                    session_id
                )));
            }
        }

        let (uplink_tx, uplink_rx) = mpsc::channel(256);
        let (downlink_tx, downlink_rx) = mpsc::channel(256);
        let (control_tx, control_rx) = mpsc::channel::<SessionControl>(8);

        let status = Arc::new(Mutex::new(SessionStatus::new(
            req.service_option,
            SessionMetadata {
                mobile_address: req.mobile_address.clone(),
                subscriber_id: req.subscriber_id.clone(),
                phone_number: req.phone_number.clone(),
                traffic_walsh_code: req.traffic_walsh_code,
            },
        )));
        let status_clone = status.clone();
        let sid = session_id.clone();
        let so = req.service_option;

        let transport = self.create_transport();
        let alloc = Arc::clone(&self.allocator);
        let task_handle = tokio::spawn(async move {
            session_task::run_session(
                sid,
                so,
                transport,
                uplink_rx,
                downlink_tx,
                status_clone,
                alloc,
                control_rx,
            )
            .await;
        });

        let entry = SessionEntry {
            uplink_tx,
            downlink_rx: Arc::new(Mutex::new(Some(downlink_rx))),
            status,
            task_handle,
            service_option: req.service_option,
            control_tx,
        };

        {
            let mut sessions = self.sessions.lock().unwrap();
            sessions.insert(session_id.clone(), entry);
        }

        log::info!(
            "packet-service: opened session {} (SO {})",
            session_id,
            req.service_option
        );

        Ok(Response::new(OpenSessionResponse { session_id }))
    }

    async fn close_session(
        &self,
        request: Request<CloseSessionRequest>,
    ) -> Result<Response<CloseSessionResponse>, Status> {
        let req = request.into_inner();
        let entry = {
            let mut sessions = self.sessions.lock().unwrap();
            sessions.remove(&req.session_id)
        };

        match entry {
            Some(entry) => {
                // Drop the uplink sender -- this causes the session task to exit
                drop(entry.uplink_tx);
                // Wait for the task to finish (with timeout)
                let _ = tokio::time::timeout(std::time::Duration::from_secs(5), entry.task_handle)
                    .await;
                log::info!("packet-service: closed session {}", req.session_id);
                Ok(Response::new(CloseSessionResponse {}))
            }
            None => Err(Status::not_found(format!(
                "session {} not found",
                req.session_id
            ))),
        }
    }

    type StreamSessionStream =
        Pin<Box<dyn Stream<Item = Result<SessionFrame, Status>> + Send + 'static>>;

    async fn stream_session(
        &self,
        request: Request<Streaming<SessionFrame>>,
    ) -> Result<Response<Self::StreamSessionStream>, Status> {
        let mut inbound = request.into_inner();

        // Read the first frame to determine the session_id
        let first_frame = inbound
            .next()
            .await
            .ok_or_else(|| Status::invalid_argument("empty stream"))?
            .map_err(|e| Status::internal(format!("stream error: {}", e)))?;

        let session_id = first_frame.session_id.clone();

        // Look up the session and get the channels
        let (uplink_tx, downlink_rx) = {
            let sessions = self.sessions.lock().unwrap();
            let entry = sessions
                .get(&session_id)
                .ok_or_else(|| Status::not_found(format!("session {} not found", session_id)))?;
            let downlink_rx = entry.downlink_rx.lock().unwrap().take().ok_or_else(|| {
                Status::already_exists(format!(
                    "session {} already has an active stream",
                    session_id
                ))
            })?;
            (entry.uplink_tx.clone(), downlink_rx)
        };

        // Forward the first frame
        let _ = uplink_tx.send(first_frame).await;

        // Spawn a task to forward inbound frames to the session task
        let uplink_tx_clone = uplink_tx.clone();
        tokio::spawn(async move {
            while let Some(Ok(frame)) = inbound.next().await {
                if uplink_tx_clone.send(frame).await.is_err() {
                    break;
                }
            }
        });

        // Return the downlink stream
        let output_stream =
            tokio_stream::wrappers::ReceiverStream::new(downlink_rx).map(|frame| Ok(frame));

        Ok(Response::new(Box::pin(output_stream)))
    }

    async fn get_session_status(
        &self,
        request: Request<GetSessionStatusRequest>,
    ) -> Result<Response<GetSessionStatusResponse>, Status> {
        let req = request.into_inner();
        let info = self
            .get_session_detail(&req.session_id)
            .ok_or_else(|| Status::not_found(format!("session {} not found", req.session_id)))?;

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

    async fn set_session_capture(
        &self,
        request: Request<SetSessionCaptureRequest>,
    ) -> Result<Response<SetSessionCaptureResponse>, Status> {
        let req = request.into_inner();
        let session = self
            .set_session_capture(&req.session_id, req.enabled)
            .ok_or_else(|| Status::not_found(format!("session {} not found", req.session_id)))?;

        Ok(Response::new(SetSessionCaptureResponse {
            session: Some(session),
        }))
    }

    async fn set_sch_active(
        &self,
        request: Request<SetSchActiveRequest>,
    ) -> Result<Response<SetSchActiveResponse>, Status> {
        let req = request.into_inner();
        let rate_bps = if req.rate_bps == 0 {
            cdma_common::sch::DEFAULT_RC3_F_SCH_RATE_BPS
        } else {
            req.rate_bps
        };
        self.set_session_sch_active(&req.session_id, req.active, rate_bps)
            .await
            .map_err(|e| {
                if e.contains("not found") {
                    Status::not_found(e)
                } else {
                    Status::failed_precondition(e)
                }
            })?;
        Ok(Response::new(SetSchActiveResponse {}))
    }
}
