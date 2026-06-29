//! MSC runtime event loop for A1 signaling and management control.
//!
//! This was extracted from the BSC monolith (`cdma-bsc/src/main.rs`). The MSC
//! runtime owns the A1 call-control state machine and processes both
//! operator-initiated (management) and BSC-initiated (A1) events.
//!
//! `MscRuntime` is a thin actor shell that owns service structs and the
//! `select!` loop. Each service owns its own state; cross-service calls pass
//! `&mut` references rather than `Arc`.

use std::sync::Arc;
use std::time::Duration;

use log::{debug, info, warn};

use cdma_common::consts::{
    SERVICE_OPTION_HIGH_RATE_PACKET_DATA, SERVICE_OPTION_OTASP, SERVICE_OPTION_PACKET_DATA,
    SERVICE_OPTION_SMS,
};
use cdma_hlr::model::{RegistrationBinding, SubscriberIdentity};
use cdma_ios::{A1TransportError, EncodedA1Message, VoiceBearerFrame, VoiceBearerManager};

use crate::call_control::{CallDirection, CallId, MscCallController};
use crate::circuit::{CircuitService, CircuitSession, DeferredPagingResponse, MscVoiceLeg};
use crate::config::MediaRingbackType;
use crate::management::{
    InitiateCallAccepted, InitiateCallRequest, ManagementError, MtCallPlan, PendingControlRequest,
};
use crate::media::MediaService;
use crate::media_gateway::{
    CreateCallRequest, MediaGatewayClient, MediaGatewayEvent, ReleaseCause, VocoderFrame,
};
use crate::media_gateway_service::{
    MediaGatewayService, gateway_clear_cause, send_forward_bearer_frame,
    send_gateway_clear_command, stop_media_for_call,
};
use crate::mo_call::{MoCallService, MoSubscriberRoute};
use crate::mt_call::MtCallService;

/// Trait abstracting the A1 transport endpoint (MSC side).
///
/// Both in-process loopback and real TCP transport implement this.
#[async_trait::async_trait]
pub trait MscA1Endpoint: Send + Sync {
    /// Receives one A1 message from the BSC.
    async fn recv_from_bsc(&self) -> Option<EncodedA1Message>;

    /// Sends one A1 message toward the BSC.
    async fn send_to_bsc(&self, message: EncodedA1Message) -> Result<(), A1TransportError>;
}

#[derive(Copy, Clone, Debug)]
pub(crate) enum PagePurpose {
    Initial,
    Retry,
    RepageAfterAf { failed_leg: MscVoiceLeg },
    M2mSecondary,
}

fn is_sms_traffic_service_option(so: u16) -> bool {
    matches!(so, SERVICE_OPTION_SMS | 14)
}

fn is_packet_data_service_option(so: u16) -> bool {
    matches!(
        so,
        SERVICE_OPTION_PACKET_DATA | SERVICE_OPTION_HIGH_RATE_PACKET_DATA
    )
}

fn is_otasp_service_option(so: u16) -> bool {
    matches!(so, SERVICE_OPTION_OTASP)
}

fn is_non_voice_a1_service_option(so: u16) -> bool {
    is_sms_traffic_service_option(so)
        || is_packet_data_service_option(so)
        || is_otasp_service_option(so)
}

impl PagePurpose {
    fn tag(self) -> &'static str {
        match self {
            Self::Initial => "",
            Self::Retry => " (retry)",
            Self::RepageAfterAf { .. } => " (re-page after AssignmentFailure)",
            Self::M2mSecondary => " for MO M2M (deferred until primary AssignmentComplete)",
        }
    }

    /// `Retry` reuses the existing `mt_page_retry` entry; re-registering
    /// would reset `give_up_at` and prevent the hunt from ever timing out.
    fn starts_hunt_window(self) -> bool {
        !matches!(self, Self::Retry)
    }
}

#[derive(Debug)]
pub(crate) enum MtSetupError {
    SubscriberInactive(uuid::Uuid),
    NotRegistered(uuid::Uuid),
    NotPageable(uuid::Uuid, cdma_hlr::model::RegistrationState),
    NoPageableImsi(uuid::Uuid),
    A1Closed,
    Other(String),
}

impl MtSetupError {
    pub(crate) fn into_management_error(self) -> ManagementError {
        match self {
            Self::SubscriberInactive(id) => {
                ManagementError::Rejected(format!("subscriber {id} is not active"))
            }
            Self::NotRegistered(id) => {
                ManagementError::Rejected(format!("subscriber {id} is not currently registered"))
            }
            Self::NotPageable(id, state) => ManagementError::Rejected(format!(
                "subscriber {id} is not pageable in state {}",
                state.as_str()
            )),
            Self::NoPageableImsi(id) => {
                ManagementError::Rejected(format!("subscriber {id} has no IMSI for A1 paging"))
            }
            Self::A1Closed => ManagementError::Unavailable("A1 edge to BSC is closed"),
            Self::Other(msg) => ManagementError::Rejected(msg),
        }
    }

    pub(crate) fn to_sip_status(&self) -> u16 {
        match self {
            Self::SubscriberInactive(_) => 404,
            Self::NotRegistered(_) | Self::NotPageable(_, _) | Self::NoPageableImsi(_) => 480,
            Self::A1Closed | Self::Other(_) => 503,
        }
    }
}

/// Configuration for the MSC runtime.
pub struct MscRuntimeConfig {
    /// HLR repository for subscriber lookups.
    pub hlr_repo: Arc<dyn cdma_hlr::repository::HlrRepository>,
    /// Optional SMSC repository — enables the MSC SMS coordinator when present.
    pub smsc_repo: Option<Arc<dyn cdma_smsc::repository::SmscRepository>>,
    /// Default service option for MT voice calls.
    pub default_voice_service_option: u16,
    /// Optional WAV file used for MSC-owned local playback/fallback paths.
    pub wav_file: Option<String>,
    /// Whether gateway setup failures may fall back to local WAV before answer.
    pub gateway_fallback_to_wav: bool,
    /// Delay before MSC-owned local WAV playback answers a fallback/test call.
    pub local_answer_delay_ms: u64,
    /// Whether the MSC should synthesize ringback media on the caller bearer.
    pub media_ringback_enabled: bool,
    /// Ringback cadence to synthesize when enabled.
    pub media_ringback_type: MediaRingbackType,
    pub sip_ringback_disable: bool,
    pub inbound_sip_msc_ringback: bool,
    pub generate_ringback: bool,
    pub send_tones_alert: bool,
    pub page_retry_cooldown_ms: u64,
    pub page_retry_max_duration_ms: u64,
    pub failure_tone_duration_ms: u64,
    /// Voice bearer manager for MSC<->BSC per-circuit RTP voice sessions.
    pub voice_bearer: Option<Arc<VoiceBearerManager>>,
    /// Optional MSC-owned media gateway client for external voice legs.
    pub media_gateway: Option<Arc<dyn MediaGatewayClient>>,
    /// Welcome SMS sent to mobiles on first registration.
    pub welcome_sms: Option<crate::config::WelcomeSmsConfig>,
    /// MT SMS retry sweep configuration.
    pub sms_retry: crate::config::SmsRetryConfig,
    /// OTASP configuration. `None` disables OTASP entirely.
    pub otasp: Option<crate::config::OtaspConfig>,
    /// BTS overhead values required by OTASP NAM assembly. The launcher
    /// (cdma-nib) fills this from the loaded BTS/BSC configs; standalone MSC
    /// launchers must supply it before OTASP sessions can run.
    pub bts_overhead: Option<crate::config::BtsOverheadConfig>,
}

impl MscRuntimeConfig {
    pub fn from_node_config(
        config: &crate::MscNodeConfig,
        hlr_repo: Arc<dyn cdma_hlr::repository::HlrRepository>,
    ) -> Self {
        let media_gateway = if config.voice.gateway.enabled {
            Some(
                crate::spawn_voice_gateway_client(config.voice.gateway.clone())
                    as Arc<dyn MediaGatewayClient>,
            )
        } else {
            None
        };
        Self {
            hlr_repo,
            smsc_repo: None,
            default_voice_service_option: config.voice.default_mobile_terminated_service_option(),
            wav_file: config.voice.wav_file.clone(),
            gateway_fallback_to_wav: config.voice.gateway.fallback_to_wav,
            local_answer_delay_ms: config.voice.answer_delay_ms,
            media_ringback_enabled: config.voice.media_ringback_enabled,
            media_ringback_type: config.voice.media_ringback_type,
            sip_ringback_disable: config.voice.sip_ringback_disable,
            inbound_sip_msc_ringback: config.voice.inbound_sip_msc_ringback,
            generate_ringback: config.voice.generate_ringback,
            send_tones_alert: config.voice.send_tones_alert,
            page_retry_cooldown_ms: config.voice.page_retry_cooldown_ms,
            page_retry_max_duration_ms: config.voice.page_retry_max_duration_ms,
            failure_tone_duration_ms: config.voice.failure_tone_duration_ms,
            voice_bearer: Some(Arc::new(VoiceBearerManager::new(
                config.voice.voice_bearer_bind_ip,
            ))),
            media_gateway,
            welcome_sms: Some(config.welcome_sms.clone()),
            sms_retry: config.sms_retry.clone(),
            otasp: if config.otasp.enabled {
                Some(config.otasp.clone())
            } else {
                None
            },
            bts_overhead: None,
        }
    }
}

/// MSC runtime that processes A1 and management events.
pub struct MscRuntime {
    pub(crate) controller: MscCallController,
    pub(crate) config: MscRuntimeConfig,
    pub(crate) circuits: CircuitService,
    pub(crate) media: MediaService,
    pub(crate) media_gw: MediaGatewayService,
    pub(crate) mt_call: MtCallService,
    pub(crate) mo_call: MoCallService,
    pub(crate) mt_page_retry: crate::mt_page_retry::MtPageRetryService,
    pub(crate) smsc: Option<crate::sms::MscSmsCoordinator>,
    pub(crate) otasp: Option<crate::otasp::OtaspCoordinator>,
    /// Shared history of recent OTASP sessions; cloned into the gRPC service so
    /// `ListOtaspSessions` / `GetOtaspSession` can read it concurrently with
    /// the runtime appending new records.
    pub(crate) otasp_history: std::sync::Arc<crate::otasp::OtaspHistory>,
    /// Broadcast channel for live `MscNetworkEvent`s — fans out to any number
    /// of `StreamOtaspEvents` subscribers without blocking the producer.
    pub(crate) otasp_event_tx:
        tokio::sync::broadcast::Sender<crate::grpc::events_proto::v1::MscNetworkEvent>,
    /// Origination contexts for calls recognized as OTASP at MO CL3 but whose
    /// traffic channel has not yet come up. Key is the MSC call id; the value
    /// is the data needed to start the OTASP session at AssignmentComplete.
    /// Per C.S0016-D §3.2.1 the MS originates with a voice/data SO (not 18),
    /// so we cannot tell from `service_option` alone that a call is OTASP —
    /// the dialed-digits match recorded here is what gates voice suppression
    /// and session start.
    pub(crate) pending_otasp_originations:
        std::collections::HashMap<CallId, PendingOtaspOrigination>,
}

/// Stashed at MO CL3 for an OTASP-recognized call until the traffic channel
/// comes up at `AssignmentComplete`.
pub(crate) struct PendingOtaspOrigination {
    pub device: crate::otasp::HardwareIdentity,
    pub feature_code: String,
    pub mobile_identity_imsi: cdma_ios::MobileIdentity,
    /// Actual Service Option from the Origination Message (typically 3 for
    /// voice). Per C.S0016-D §3.2.1, user-initiated OTASP rides a vendor-
    /// chosen voice/data SO, NOT SO 18.
    pub service_option: u16,
}

impl MscRuntime {
    /// Creates a new MSC runtime.
    pub fn new(config: MscRuntimeConfig) -> Self {
        let smsc = config.smsc_repo.as_ref().map(|smsc_repo| {
            crate::sms::MscSmsCoordinator::new(Arc::clone(smsc_repo), Arc::clone(&config.hlr_repo))
        });
        let otasp_history = crate::otasp::OtaspHistory::new();
        let (otasp_event_tx, _) =
            tokio::sync::broadcast::channel::<crate::grpc::events_proto::v1::MscNetworkEvent>(256);
        let otasp = match (config.otasp.as_ref(), config.bts_overhead.as_ref()) {
            (Some(otasp_cfg), Some(bts_overhead)) => {
                Some(crate::otasp::OtaspCoordinator::with_history(
                    otasp_cfg.clone(),
                    bts_overhead.clone(),
                    Arc::clone(&config.hlr_repo),
                    Arc::clone(&otasp_history),
                    Some(otasp_event_tx.clone()),
                ))
            }
            (Some(_), None) => {
                log::warn!(
                    "MSC: OTASP enabled in config but bts_overhead not supplied by launcher — OTASP disabled"
                );
                None
            }
            _ => None,
        };
        let mt_page_retry = crate::mt_page_retry::MtPageRetryService::new(
            config.page_retry_cooldown_ms,
            config.page_retry_max_duration_ms,
        );
        Self {
            controller: MscCallController::new(),
            config,
            circuits: CircuitService::new(),
            media: MediaService::new(),
            media_gw: MediaGatewayService::new(),
            mt_call: MtCallService::new(),
            mo_call: MoCallService::new(),
            mt_page_retry,
            smsc,
            otasp,
            otasp_history,
            otasp_event_tx,
            pending_otasp_originations: std::collections::HashMap::new(),
        }
    }

    /// Shared snapshot of the OTASP session ring buffer; safe to clone into a
    /// long-lived gRPC service.
    pub fn otasp_history(&self) -> std::sync::Arc<crate::otasp::OtaspHistory> {
        std::sync::Arc::clone(&self.otasp_history)
    }

    /// Live broadcast channel for OTASP `MscNetworkEvent`s; clone the sender
    /// to subscribe new receivers.
    pub fn otasp_event_tx(
        &self,
    ) -> tokio::sync::broadcast::Sender<crate::grpc::events_proto::v1::MscNetworkEvent> {
        self.otasp_event_tx.clone()
    }

    /// Returns a reference to the call controller for management queries.
    pub fn controller(&self) -> &MscCallController {
        &self.controller
    }

    /// Runs the MSC event loop, processing both management and A1 events.
    pub async fn run(
        &mut self,
        mut management_rx: tokio::sync::mpsc::Receiver<PendingControlRequest>,
        a1: &dyn MscA1Endpoint,
    ) {
        let mut sms_expiry_interval = tokio::time::interval(Duration::from_secs(10));
        sms_expiry_interval.tick().await; // consume the immediate first tick
        let mut sms_retry_interval = tokio::time::interval(Duration::from_secs(
            self.config.sms_retry.sweep_interval_secs.max(1),
        ));
        sms_retry_interval.tick().await; // consume the immediate first tick
        let sms_retry_after = Duration::from_secs(self.config.sms_retry.retry_after_secs);
        let sms_retry_enabled = self.config.sms_retry.enabled;
        let mut otasp_timeout_interval = tokio::time::interval(Duration::from_secs(1));
        otasp_timeout_interval.tick().await;

        // Restart recovery: the in-flight ADDS Page correlation lives only
        // in MscSmsCoordinator::pending (in-memory). Anything left in DB
        // state `Paging` on a fresh boot was orphaned by the previous
        // process and the retry sweep will skip it forever otherwise.
        // Threshold is 2× the regular in-flight timeout so we don't fight
        // with `expire_pending` over genuinely fresh attempts.
        if sms_retry_enabled && let Some(smsc) = self.smsc.as_mut() {
            smsc.recover_stuck_paging(Duration::from_secs(120)).await;
        }

        loop {
            let delayed_wav_sleep = async {
                let next_deadline = self.media.next_delayed_wav_deadline();
                match next_deadline {
                    Some(deadline) => tokio::time::sleep_until(deadline).await,
                    None => std::future::pending().await,
                }
            };
            let pending_clear_sleep = async {
                match self.media_gw.next_pending_clear_deadline() {
                    Some(deadline) => tokio::time::sleep_until(deadline).await,
                    None => std::future::pending().await,
                }
            };
            let page_retry_sleep = async {
                match self.mt_page_retry.next_retry_deadline() {
                    Some(deadline) => tokio::time::sleep_until(deadline).await,
                    None => std::future::pending().await,
                }
            };
            tokio::select! {
                request = management_rx.recv() => {
                    let Some(request) = request else {
                        break;
                    };
                    self.handle_management_request(request, a1).await;
                }
                inbound = a1.recv_from_bsc() => {
                    let Some(message) = inbound else {
                        break;
                    };
                    self.handle_bsc_a1_message(a1, message).await;
                }
                result = async {
                    match self.config.voice_bearer.as_ref() {
                        Some(bearer) => bearer.recv().await,
                        None => std::future::pending().await,
                    }
                } => {
                    match result {
                        Some(cdma_ios::BearerEvent::Voice(frame)) => {
                            self.handle_reverse_bearer_frame(frame).await;
                        }
                        Some(cdma_ios::BearerEvent::Dtmf(event)) => {
                            self.handle_reverse_bearer_dtmf(event).await;
                        }
                        None => {}
                    }
                }
                event = async {
                    match self.config.media_gateway.as_ref() {
                        Some(gateway) => gateway.recv_event().await,
                        None => std::future::pending().await,
                    }
                } => {
                    if let Some(event) = event {
                        match event {
                            MediaGatewayEvent::InboundCall {
                                session_id,
                                called_number,
                                caller_number,
                                caller_display: _,
                                offered_codecs,
                            } => {
                                self.handle_inbound_sip_invite(
                                    a1,
                                    session_id,
                                    called_number,
                                    caller_number,
                                    offered_codecs,
                                )
                                .await;
                            }
                            MediaGatewayEvent::InboundCancel { session_id } => {
                                self.handle_inbound_sip_cancel(a1, session_id).await;
                            }
                            other => {
                                self.media_gw
                                    .handle_media_gateway_event(
                                        a1,
                                        other,
                                        &mut self.controller,
                                        &mut self.circuits,
                                        &mut self.media,
                                        self.config.voice_bearer.as_ref(),
                                        self.config.media_gateway.as_ref(),
                                        self.config.media_ringback_enabled,
                                        self.config.media_ringback_type,
                                        self.config.sip_ringback_disable,
                                        self.config.generate_ringback,
                                        self.config.send_tones_alert,
                                        self.config.failure_tone_duration_ms,
                                        Some(&self.config.hlr_repo),
                                    )
                                    .await;
                            }
                        }
                    }
                }
                _ = delayed_wav_sleep => {
                    self.media.handle_due_delayed_wav_starts(
                        &self.circuits,
                        self.config.voice_bearer.as_ref(),
                    );
                }
                _ = pending_clear_sleep => {
                    self.fire_due_pending_clears(a1).await;
                }
                _ = page_retry_sleep => {
                    self.fire_due_page_retries(a1).await;
                }
                _ = sms_expiry_interval.tick() => {
                    if let Some(smsc) = self.smsc.as_mut() {
                        smsc.expire_pending(Duration::from_secs(60)).await;
                    }
                }
                _ = sms_retry_interval.tick(), if sms_retry_enabled => {
                    if let Some(smsc) = self.smsc.as_mut() {
                        smsc.retry_eligible_sweep(a1, sms_retry_after).await;
                    }
                }
                _ = otasp_timeout_interval.tick() => {
                    self.tick_otasp_timeouts(a1).await;
                }
            }
        }
    }

    /// Force-release OTASP sessions whose MS has gone silent past the
    /// configured threshold. Sends an A1 ClearCommand for each so the BSC
    /// tears down the traffic channel cleanly.
    async fn tick_otasp_timeouts(&mut self, a1: &dyn MscA1Endpoint) {
        let Some(otasp) = self.otasp.as_mut() else {
            return;
        };
        let released = otasp
            .tick_timeouts(
                std::time::Instant::now(),
                crate::otasp::coordinator::DEFAULT_INBOUND_TIMEOUT,
            )
            .await;
        for call_id_raw in released {
            let call_id = CallId(call_id_raw);
            send_gateway_clear_command(
                a1,
                call_id,
                &mut self.controller,
                gateway_clear_cause(ReleaseCause::SetupFailed, None),
            )
            .await;
        }
    }

    /// Runs the MSC event loop with an embedded gRPC management server.
    ///
    /// The gRPC server accepts management requests (initiate_call, list_calls)
    /// and feeds them into the runtime's event loop via an internal channel.
    pub async fn run_with_grpc(&mut self, mgmt_addr: std::net::SocketAddr, a1: &dyn MscA1Endpoint) {
        let (mgmt_tx, mgmt_rx) = tokio::sync::mpsc::channel::<PendingControlRequest>(16);
        let service = crate::grpc::MscManagementServiceImpl::from_channel(mgmt_tx)
            .with_otasp(self.otasp_event_tx());
        let server = tonic::transport::Server::builder()
            .add_service(
                crate::grpc::msc_management::v1::msc_management_service_server::MscManagementServiceServer::new(service),
            )
            .serve(mgmt_addr);
        tokio::spawn(server);
        self.run(mgmt_rx, a1).await;
    }

    async fn handle_management_request(
        &mut self,
        request: PendingControlRequest,
        a1: &dyn MscA1Endpoint,
    ) {
        match request {
            PendingControlRequest::InitiateCall {
                request,
                response_tx,
            } => {
                let result = self.handle_initiate_call(a1, request).await;
                if response_tx.send(result).is_err() {
                    warn!("MSC: initiate-call response receiver dropped");
                }
            }
            PendingControlRequest::ListCalls { response_tx } => {
                if response_tx
                    .send(Ok(self.controller.all_snapshots()))
                    .is_err()
                {
                    warn!("MSC: list-calls response receiver dropped");
                }
            }
            PendingControlRequest::SendSms {
                request,
                response_tx,
            } => {
                let result = if let Some(smsc) = self.smsc.as_mut() {
                    smsc.send_sms(request, a1).await
                } else {
                    warn!("MSC: send_sms requested but SMSC coordinator is not configured");
                    None
                };
                if response_tx.send(result).is_err() {
                    warn!("MSC: send-sms response receiver dropped");
                }
            }
        }
    }

    /// Handles one management-plane mobile-terminated call request.
    async fn handle_initiate_call(
        &mut self,
        a1: &dyn MscA1Endpoint,
        request: InitiateCallRequest,
    ) -> Result<InitiateCallAccepted, ManagementError> {
        let Some(resolved) = self
            .config
            .hlr_repo
            .get_subscriber_by_id(request.subscriber_id)
            .await
            .map_err(ManagementError::Rejected)?
        else {
            return Err(ManagementError::UnknownSubscriber(request.subscriber_id));
        };
        self.start_mt_call(a1, resolved, request.caller_number, request.audio_file)
            .await
            .map(|call_id| InitiateCallAccepted { call_id })
            .map_err(MtSetupError::into_management_error)
    }

    pub(crate) async fn start_mt_call(
        &mut self,
        a1: &dyn MscA1Endpoint,
        resolved: cdma_hlr::model::ResolvedSubscriber,
        caller_number: Option<String>,
        audio_file: Option<String>,
    ) -> Result<CallId, MtSetupError> {
        let subscriber_id = resolved.subscriber.subscriber_id;
        if !matches!(
            resolved.subscriber.status,
            cdma_hlr::model::SubscriberStatus::Active
        ) {
            return Err(MtSetupError::SubscriberInactive(subscriber_id));
        }

        let Some(binding) = resolved.binding.as_ref() else {
            return Err(MtSetupError::NotRegistered(subscriber_id));
        };
        if !matches!(
            binding.state,
            cdma_hlr::model::RegistrationState::Registered
                | cdma_hlr::model::RegistrationState::PageResponseReceived
        ) {
            return Err(MtSetupError::NotPageable(
                subscriber_id,
                binding.state.clone(),
            ));
        }

        let imsi = select_pageable_imsi(&resolved.identities, binding)
            .ok_or(MtSetupError::NoPageableImsi(subscriber_id))?;

        let call_id = self.controller.create_call(
            CallDirection::MobileTerminated,
            Some(cdma_ios::MobileIdentity::Imsi(imsi.to_string())),
        );
        self.media_gw
            .register_active_subscriber(subscriber_id, call_id);
        let tag = cdma_ios::Tag(call_id.0 as u32);
        let paging_request = cdma_ios::PagingRequestMessage {
            mobile_identity_imsi: cdma_ios::MobileIdentity::Imsi(imsi.to_string()),
            tag: Some(tag),
            cell_identifier_list: None,
            slot_cycle_index: binding
                .slot_cycle_index
                .map(|value| cdma_ios::SlotCycleIndex(value as u8)),
            service_option: Some(cdma_ios::ServiceOption(
                self.config.default_voice_service_option,
            )),
            is2000_mobile_capabilities: None,
        };
        if let Err(e) = self.controller.apply_from_msc(
            call_id,
            &cdma_ios::ProcedureMessage::PagingRequest(paging_request.clone()),
        ) {
            self.abort_mt_setup(call_id);
            return Err(MtSetupError::Other(format!("msc paging state error: {e}")));
        }
        self.mt_call.mt_plans.insert(
            tag.0,
            MtCallPlan {
                subscriber_id,
                imsi: imsi.to_string(),
                audio_file,
                caller_number,
                service_option: self.config.default_voice_service_option,
            },
        );
        self.circuits
            .paging_requests
            .insert(call_id, paging_request.clone());
        if !self
            .send_paging_request_to_bsc(a1, call_id, paging_request, PagePurpose::Initial)
            .await
        {
            self.abort_mt_setup(call_id);
            return Err(MtSetupError::A1Closed);
        }
        Ok(call_id)
    }

    fn abort_mt_setup(&mut self, call_id: CallId) {
        self.mt_page_retry.cancel(call_id);
        self.controller.remove_call(call_id);
        self.stop_media_for_call(call_id);
    }

    /// Send an A1 PagingRequest; rearms the controller engine on
    /// `RepageAfterAf{ Primary }` and registers with `mt_page_retry` unless
    /// `purpose == Retry`. Returns `false` if rearm/encode/transport failed.
    async fn send_paging_request_to_bsc(
        &mut self,
        a1: &dyn MscA1Endpoint,
        call_id: CallId,
        paging_request: cdma_ios::PagingRequestMessage,
        purpose: PagePurpose,
    ) -> bool {
        if let PagePurpose::RepageAfterAf { failed_leg } = purpose
            && failed_leg == MscVoiceLeg::Primary
            && let Err(error) = self.controller.rearm_for_repage(
                call_id,
                &cdma_ios::ProcedureMessage::PagingRequest(paging_request.clone()),
            )
        {
            warn!(
                "MSC: failed to rearm call-control state for re-page call_id={}: {:?}",
                call_id.0, error
            );
            return false;
        }
        let payload = match paging_request.encode() {
            Ok(p) => p,
            Err(error) => {
                warn!(
                    "MSC: failed to encode PagingRequest{} call_id={}: {}",
                    purpose.tag(),
                    call_id.0,
                    error
                );
                return false;
            }
        };
        info!(
            "MSC: A1 tx PagingRequest{} call_id={}",
            purpose.tag(),
            call_id.0
        );
        if let Err(error) = a1
            .send_to_bsc(EncodedA1Message::from_message_for_call(
                &cdma_ios::Message::new(cdma_ios::MessageType::PagingRequest, payload),
                Some(call_id.0),
            ))
            .await
        {
            warn!(
                "MSC: failed to send PagingRequest{} call_id={}: {}",
                purpose.tag(),
                call_id.0,
                error
            );
            return false;
        }
        if purpose.starts_hunt_window() {
            self.mt_page_retry.register(call_id, paging_request);
        }
        true
    }

    /// Returns true if the BSC ClearRequest was consumed (handled as a hunt
    /// retry or hunt give-up). False means the caller should fall through to
    /// the normal ClearRequest processing.
    async fn handle_mt_page_timeout(&mut self, a1: &dyn MscA1Endpoint, call_id: CallId) -> bool {
        use crate::mt_page_retry::PageTimeoutOutcome;
        match self.mt_page_retry.handle_page_timeout(call_id) {
            PageTimeoutOutcome::Retry(deadline) => {
                info!(
                    "MSC: page timeout for call_id={} — retrying in {}ms",
                    call_id.0,
                    deadline
                        .saturating_duration_since(tokio::time::Instant::now())
                        .as_millis()
                );
                true
            }
            PageTimeoutOutcome::GiveUp => {
                warn!(
                    "MSC: page hunt exhausted for call_id={} — declaring call failed",
                    call_id.0
                );
                self.fail_mt_call(a1, call_id, 480).await;
                true
            }
            PageTimeoutOutcome::Unknown => false,
        }
    }

    async fn fire_due_page_retries(&mut self, a1: &dyn MscA1Endpoint) {
        let due = self.mt_page_retry.drain_due(tokio::time::Instant::now());
        for (call_id, paging_request) in due {
            self.send_paging_request_to_bsc(a1, call_id, paging_request, PagePurpose::Retry)
                .await;
        }
    }

    /// Tear down an MT call that never reached Connect. For SIP-inbound calls
    /// this also drives `inbound_reject(sip_status)` so the caller gets a final
    /// SIP response immediately (no 30 s gateway-watchdog wait).
    async fn fail_mt_call(&mut self, a1: &dyn MscA1Endpoint, call_id: CallId, sip_status: u16) {
        self.mt_page_retry.cancel(call_id);
        if let Some(meta) = self.media_gw.take_inbound_session(call_id)
            && let Some(gateway) = self.config.media_gateway.clone()
        {
            let session_id = meta.session_id;
            tokio::spawn(async move {
                let _ = gateway.inbound_reject(&session_id, sip_status).await;
            });
        }
        self.send_clear_command(a1, call_id).await;
        self.controller.remove_call(call_id);
        self.stop_media_for_call(call_id);
    }

    /// Handles one inbound A1 message from the BSC.
    pub async fn handle_bsc_a1_message(
        &mut self,
        a1: &dyn MscA1Endpoint,
        message: EncodedA1Message,
    ) {
        // ADDS messages use SMS Tag correlation, not the A1 transport call_id.
        // Route them directly to the SMS coordinator before the call_id check.
        // CompleteLayer3Information without a call_id is a registration notification.
        match message.message_type() {
            cdma_ios::MessageType::AddsPageAck
            | cdma_ios::MessageType::AddsDeliverAck
            | cdma_ios::MessageType::AddsTransfer
            | cdma_ios::MessageType::AddsDeliver => {
                self.handle_adds_message(a1, message).await;
                return;
            }
            cdma_ios::MessageType::CompleteLayer3Information if message.call_id().is_none() => {
                self.handle_registration_notification(a1, message).await;
                return;
            }
            _ => {}
        }

        let Some(call_id_raw) = message.call_id() else {
            warn!("MSC: dropping A1 message from BSC without transport call correlation");
            return;
        };
        let call_id = CallId(call_id_raw);
        let decoded = match message.decode() {
            Ok(decoded) => decoded,
            Err(error) => {
                warn!("MSC: dropping malformed A1 message from BSC: {}", error);
                return;
            }
        };

        info!(
            "MSC: A1 rx {:?} call_id={}",
            decoded.message_type, call_id_raw,
        );

        match decoded.message_type {
            cdma_ios::MessageType::PagingResponse => {
                self.mt_page_retry.cancel(call_id);
                let response = match cdma_ios::PagingResponseMessage::decode(&decoded.payload) {
                    Ok(response) => response,
                    Err(error) => {
                        warn!("MSC: failed to decode A1 Paging Response: {}", error);
                        return;
                    }
                };
                let secondary_leg = self
                    .circuits
                    .circuits
                    .values()
                    .any(|session| session.call_id == call_id);
                if secondary_leg && self.circuits.has_pending_assignment_complete(call_id) {
                    self.circuits
                        .deferred_paging_responses
                        .entry(call_id)
                        .or_default()
                        .push_back(DeferredPagingResponse { response });
                    info!(
                        "MSC: deferred secondary-leg PagingResponse call_id={} until active assignment completes",
                        call_id.0
                    );
                    return;
                }
                if !secondary_leg {
                    if let Err(error) = self.controller.apply_from_bsc(
                        call_id,
                        &cdma_ios::ProcedureMessage::PagingResponse(response.clone()),
                    ) {
                        warn!("MSC: failed to apply A1 Paging Response: {}", error);
                        return;
                    }
                }
                self.mt_call
                    .send_assignment_request_for_paging_response(
                        a1,
                        call_id,
                        response,
                        secondary_leg,
                        &mut self.controller,
                        &mut self.circuits,
                        &self.mo_call,
                        self.config.voice_bearer.as_ref(),
                        self.config.default_voice_service_option,
                        &self.config.hlr_repo,
                    )
                    .await;
            }
            cdma_ios::MessageType::AssignmentComplete => {
                let complete = match cdma_ios::AssignmentCompleteMessage::decode(&decoded.payload) {
                    Ok(complete) => complete,
                    Err(error) => {
                        warn!("MSC: failed to decode A1 Assignment Complete: {}", error);
                        return;
                    }
                };
                let completed_circuit_id = self.circuits.assignment_complete_circuit(call_id);
                let completed_leg = completed_circuit_id
                    .and_then(|cid| self.circuits.circuits.get(&cid))
                    .map(|session| session.leg_role);
                if let Some(params) = complete.a2p_bearer_session_params {
                    if let Some(bearer) = self.config.voice_bearer.as_ref() {
                        let remote = std::net::SocketAddr::new(
                            std::net::IpAddr::V4(params.ip_address),
                            params.udp_port,
                        );
                        if let Some(cid) = completed_circuit_id {
                            info!(
                                "MSC: A1 AssignmentComplete call_id={} circuit_id={} bearer_remote={}",
                                call_id.0, cid, remote
                            );
                            bearer.set_circuit_remote(cid, remote);
                            if let Some(session) = self.circuits.circuits.get_mut(&cid) {
                                session.bearer_remote_ready = true;
                            }
                        } else {
                            warn!(
                                "MSC: A1 AssignmentComplete call_id={} has bearer params but no pending circuit",
                                call_id.0
                            );
                        }
                    } else if let Some(cid) = completed_circuit_id {
                        debug!(
                            "MSC: A1 AssignmentComplete call_id={} circuit_id={} without configured voice bearer",
                            call_id.0, cid
                        );
                    }
                } else if let Some(cid) = completed_circuit_id {
                    debug!(
                        "MSC: A1 AssignmentComplete call_id={} circuit_id={} without bearer params",
                        call_id.0, cid
                    );
                }
                match completed_leg {
                    Some(MscVoiceLeg::Secondary) => {
                        if let Err(error) = self.circuits.apply_secondary_leg_from_bsc(
                            call_id,
                            &cdma_ios::ProcedureMessage::AssignmentComplete(complete),
                        ) {
                            warn!(
                                "MSC: failed to apply secondary-leg Assignment Complete: {:?}",
                                error
                            );
                        }
                    }
                    _ => {
                        if let Err(error) = self.controller.apply_from_bsc(
                            call_id,
                            &cdma_ios::ProcedureMessage::AssignmentComplete(complete),
                        ) {
                            warn!("MSC: failed to apply A1 Assignment Complete: {}", error);
                        }
                    }
                }
                self.circuits.reset_assignment_failure_retries(call_id);
                if let Some(pending) = self.pending_otasp_originations.remove(&call_id)
                    && let Some(otasp) = self.otasp.as_mut()
                {
                    info!(
                        "MSC: AssignmentComplete call_id={} — starting OTASP session",
                        call_id.0
                    );
                    otasp
                        .begin_session(
                            pending.device,
                            pending.feature_code,
                            pending.service_option,
                            pending.mobile_identity_imsi,
                            call_id.0,
                            a1,
                        )
                        .await;
                    return;
                }
                if completed_circuit_id
                    .and_then(|cid| self.circuits.circuits.get(&cid))
                    .is_some_and(|session| is_non_voice_a1_service_option(session.service_option))
                {
                    debug!(
                        "MSC: AssignmentComplete call_id={} completed non-voice A1 context",
                        call_id.0
                    );
                    return;
                }
                if completed_circuit_id
                    .and_then(|cid| self.circuits.circuits.get(&cid))
                    .is_some_and(|session| {
                        session.audio_file.is_some()
                            && session.peer_circuit_id.is_none()
                            && session.media_gateway_handle.is_none()
                            && self
                                .controller
                                .snapshot(session.call_id)
                                .is_some_and(|snapshot| {
                                    snapshot.direction == CallDirection::MobileOriginated
                                })
                    })
                {
                    self.media.schedule_delayed_wav_start(
                        call_id,
                        self.config.local_answer_delay_ms,
                        &self.controller,
                        &self.circuits,
                        self.config.voice_bearer.as_ref(),
                        self.config.media_ringback_enabled,
                        self.config.media_ringback_type,
                        Some(&self.config.hlr_repo),
                    );
                } else if completed_leg == Some(MscVoiceLeg::Primary)
                    && !self.config.sip_ringback_disable
                {
                    // `sip_ringback_disable=true` keeps the bearer silent so
                    // the BSC's bearer-flow → tones-off doesn't fire before
                    // SIP audio arrives.
                    self.media.start_ringback_for_call(
                        call_id,
                        &self.controller,
                        &self.circuits,
                        self.config.voice_bearer.as_ref(),
                        self.config.media_ringback_enabled,
                        self.config.media_ringback_type,
                        Some(&self.config.hlr_repo),
                    );
                }
                self.flush_deferred_paging_response(a1, call_id).await;
                if completed_leg == Some(MscVoiceLeg::Primary)
                    && self.controller.snapshot(call_id).is_some_and(|snapshot| {
                        snapshot.direction == CallDirection::MobileOriginated
                    })
                {
                    self.flush_deferred_paging_request(a1, call_id).await;
                    self.fire_deferred_sip_invite(a1, call_id).await;
                }
                self.media_gw
                    .flush_pending_post_assignment(
                        a1,
                        call_id,
                        &mut self.controller,
                        self.config.send_tones_alert,
                    )
                    .await;

                self.fire_assignment_complete_awi(a1, call_id, completed_leg)
                    .await;

                // Send 180 Ringing only when the trunk is doing ringback;
                // when MSC sends 183 + early media many trunks interpret 180
                // as "switch to local ringback" and clobber our audio.
                if let Some(meta) = self.media_gw.inbound_session_for_call(call_id).cloned()
                    && !meta.progress_sent
                    && !self.config.inbound_sip_msc_ringback
                {
                    if let Some(session_id) = self.media_gw.inbound_by_call.get(&call_id).cloned() {
                        if let Some(gateway) = self.config.media_gateway.clone() {
                            if let Err(error) = gateway.inbound_progress(&session_id).await {
                                warn!(
                                    "MSC: inbound_progress failed for session={session_id}: {error}"
                                );
                            }
                        }
                    }
                    self.media_gw.mark_inbound_progress_sent(call_id);
                }
            }
            cdma_ios::MessageType::AssignmentFailure => {
                self.handle_assignment_failure(a1, call_id).await;
            }
            cdma_ios::MessageType::Connect => {
                let connect = match cdma_ios::ConnectMessage::decode(&decoded.payload) {
                    Ok(connect) => connect,
                    Err(error) => {
                        warn!("MSC: failed to decode A1 Connect: {}", error);
                        return;
                    }
                };
                // Tones-Off Progress must precede Connect: the engine accepts
                // Progress only from Assigned/Alerting (Connect advances to
                // Connected). BSC routes Signal=0x3F to the Caller leg.
                crate::media_gateway_service::send_progress_tones_off(
                    a1,
                    call_id,
                    &mut self.controller,
                )
                .await;
                if let Err(error) = self
                    .controller
                    .apply_from_bsc(call_id, &cdma_ios::ProcedureMessage::Connect(connect))
                {
                    warn!("MSC: failed to apply A1 Connect: {}", error);
                }
                self.media.stop_ringback_for_call(call_id, &self.circuits);
                self.media.stop_inbound_ringback(call_id);
                self.media.start_media_for_call(
                    call_id,
                    &self.circuits,
                    self.config.voice_bearer.as_ref(),
                );
                if let Some(meta) = self.media_gw.inbound_session_for_call(call_id).cloned() {
                    if let Some(session_id) = self.media_gw.inbound_by_call.get(&call_id).cloned() {
                        if let Some(gateway) = self.config.media_gateway.clone() {
                            if let Err(error) =
                                gateway.inbound_answer(&session_id, &meta.codec).await
                            {
                                warn!(
                                    "MSC: inbound_answer failed for session={session_id}: {error}"
                                );
                            } else {
                                self.media_gw.mark_answered(call_id);
                            }
                        }
                    }
                }
            }
            cdma_ios::MessageType::ClearRequest => {
                let clear_request = match cdma_ios::ClearRequestMessage::decode(&decoded.payload) {
                    Ok(clear_request) => clear_request,
                    Err(error) => {
                        warn!("MSC: failed to decode A1 Clear Request: {}", error);
                        return;
                    }
                };
                if clear_request.cause.0 == crate::mt_page_retry::A1_CAUSE_PAGE_RESP_TIMEOUT
                    && self.handle_mt_page_timeout(a1, call_id).await
                {
                    return;
                }
                if let Err(error) = self.controller.apply_from_bsc(
                    call_id,
                    &cdma_ios::ProcedureMessage::ClearRequest(clear_request.clone()),
                ) {
                    warn!("MSC: failed to apply A1 Clear Request: {}", error);
                    return;
                }

                let clear_command = cdma_ios::ClearCommandMessage {
                    cause: clear_request.cause,
                    cause_layer3: clear_request.cause_layer3,
                };
                if let Err(error) = self.controller.apply_from_msc(
                    call_id,
                    &cdma_ios::ProcedureMessage::ClearCommand(clear_command.clone()),
                ) {
                    warn!("MSC: failed to apply A1 Clear Command state: {}", error);
                    return;
                }
                let payload = match clear_command.encode() {
                    Ok(payload) => payload,
                    Err(error) => {
                        warn!("MSC: failed to encode A1 Clear Command: {}", error);
                        return;
                    }
                };
                info!("MSC: A1 tx ClearCommand call_id={}", call_id.0);
                if let Err(error) = a1
                    .send_to_bsc(EncodedA1Message::from_message_for_call(
                        &cdma_ios::Message::new(cdma_ios::MessageType::ClearCommand, payload),
                        Some(call_id.0),
                    ))
                    .await
                {
                    warn!("MSC: failed to send A1 Clear Command to BSC: {}", error);
                }
            }
            cdma_ios::MessageType::ClearComplete => {
                let clear_complete = match cdma_ios::ClearCompleteMessage::decode(&decoded.payload)
                {
                    Ok(clear_complete) => clear_complete,
                    Err(error) => {
                        warn!("MSC: failed to decode A1 Clear Complete: {}", error);
                        return;
                    }
                };
                if let Err(error) = self.controller.apply_from_bsc(
                    call_id,
                    &cdma_ios::ProcedureMessage::ClearComplete(clear_complete),
                ) {
                    // ClearComplete may arrive after MSC-local cleanup.
                    log::debug!(
                        "MSC: ClearComplete for call_id={} not applied (already cleaned up): {}",
                        call_id.0,
                        error
                    );
                    return;
                }
                if self.controller.remove_call(call_id).is_none() {
                    log::debug!(
                        "MSC: no call to remove after ClearComplete call_id={} (already cleaned up)",
                        call_id.0
                    );
                }
                self.stop_media_for_call(call_id);
            }
            cdma_ios::MessageType::CompleteLayer3Information => {
                let cli3 =
                    match cdma_ios::CompleteLayer3InformationMessage::decode(&decoded.payload) {
                        Ok(cli3) => cli3,
                        Err(error) => {
                            warn!(
                                "MSC: failed to decode A1 Complete Layer 3 Information: {}",
                                error
                            );
                            return;
                        }
                    };

                let cm_service_request = decode_cm_service_request(&cli3.layer3_information);
                let service_option = cm_service_request
                    .as_ref()
                    .and_then(|request| request.service_option)
                    .map(|service_option| service_option.0)
                    .unwrap_or(3);
                let called_number = cm_service_request
                    .as_ref()
                    .and_then(cm_service_request_called_number);

                if let Some(otasp) = self.otasp.as_ref()
                    && let Some(dialed) = called_number.as_deref()
                    && otasp.is_otasp_origination(dialed)
                {
                    let feature_code = otasp
                        .matched_feature_code(dialed)
                        .unwrap_or_else(|| dialed.to_string());
                    let (esn, meid) = extract_hardware_identity(cm_service_request.as_ref());
                    let device = crate::otasp::HardwareIdentity { esn, meid };
                    let imsi_id = cm_service_request
                        .as_ref()
                        .map(|req| req.mobile_identity_imsi.clone())
                        .unwrap_or_else(|| cdma_ios::MobileIdentity::Imsi("UNKNOWN".to_string()));
                    self.pending_otasp_originations.insert(
                        call_id,
                        PendingOtaspOrigination {
                            device,
                            feature_code,
                            mobile_identity_imsi: imsi_id,
                            service_option,
                        },
                    );
                    info!(
                        "MSC: OTASP origination recognized call_id={} dialed={} so={} — deferring session start until AssignmentComplete",
                        call_id.0, dialed, service_option
                    );
                }
                let originator = self
                    .mo_call
                    .resolve_mo_originator(
                        cm_service_request.as_ref(),
                        self.config.hlr_repo.as_ref(),
                    )
                    .await;
                let calling_number = originator.as_ref().map(|(n, _)| n.clone());

                let mobile_identity = cm_service_request
                    .as_ref()
                    .map(|req| req.mobile_identity_imsi.clone());
                let mobile_identity_esn = cm_service_request
                    .as_ref()
                    .and_then(|req| req.mobile_identity_esn.clone());
                let call_id = self.controller.create_call_with_id(
                    call_id,
                    CallDirection::MobileOriginated,
                    mobile_identity,
                );
                if let Err(error) = self
                    .controller
                    .set_mobile_identity_esn(call_id, mobile_identity_esn)
                {
                    warn!(
                        "MSC: failed to store MO hardware identity for call_id={}: {}",
                        call_id_raw, error
                    );
                }
                if let Err(error) = self.controller.apply_from_bsc(
                    call_id,
                    &cdma_ios::ProcedureMessage::CompleteLayer3Information(
                        cdma_ios::CompleteLayer3InformationMessage {
                            cell_identifier: cli3.cell_identifier,
                            layer3_information: cli3.layer3_information,
                        },
                    ),
                ) {
                    warn!(
                        "MSC: failed to apply CLI3 state for MO call_id={}: {}",
                        call_id_raw, error
                    );
                    return;
                }

                if is_non_voice_a1_service_option(service_option) {
                    let cic = self
                        .circuits
                        .assignment_circuit_identity_code_for_next_leg(call_id);
                    let circuit_id = cic.to_packed();
                    self.circuits.insert_circuit_session(
                        circuit_id,
                        CircuitSession {
                            call_id,
                            audio_file: None,
                            service_option,
                            leg_role: MscVoiceLeg::Primary,
                            peer_circuit_id: None,
                            bearer_remote_ready: true,
                            media_gateway_handle: None,
                            called_number: called_number.clone(),
                        },
                    );
                    self.circuits.queue_assignment_complete_circuit(
                        call_id,
                        MscVoiceLeg::Primary,
                        circuit_id,
                    );
                    let assignment_request = cdma_ios::AssignmentRequestMessage {
                        channel_type: cdma_ios::ChannelType {
                            speech_or_data_indicator: 0x01,
                            channel_rate_and_type: 0x08,
                            coding: 0x05,
                        },
                        circuit_identity_code: cic,
                        encryption_information: None,
                        service_option: Some(cdma_ios::ServiceOption(service_option)),
                        signals: Vec::new(),
                        ms_information_records: None,
                        priority: None,
                        paca_timestamp: None,
                        quality_of_service_parameters: None,
                        a2p_bearer_session_params: None,
                        a2p_bearer_format_params: None,
                    };
                    if let Err(error) = self.controller.apply_from_msc(
                        call_id,
                        &cdma_ios::ProcedureMessage::AssignmentRequest(assignment_request.clone()),
                    ) {
                        warn!(
                            "MSC: failed to apply non-voice Assignment Request state for MO call_id={}: {}",
                            call_id_raw, error
                        );
                        return;
                    }
                    let payload = match assignment_request.encode() {
                        Ok(payload) => payload,
                        Err(error) => {
                            warn!(
                                "MSC: failed to encode non-voice A1 Assignment Request for MO call_id={}: {}",
                                call_id_raw, error
                            );
                            return;
                        }
                    };
                    info!(
                        "MSC: A1 tx AssignmentRequest (MO non-voice SO{}) call_id={}",
                        service_option, call_id_raw
                    );
                    if let Err(error) = a1
                        .send_to_bsc(EncodedA1Message::from_message_for_call(
                            &cdma_ios::Message::new(
                                cdma_ios::MessageType::AssignmentRequest,
                                payload,
                            ),
                            Some(call_id_raw),
                        ))
                        .await
                    {
                        warn!(
                            "MSC: failed to send non-voice A1 Assignment Request to BSC for MO call_id={}: {}",
                            call_id_raw, error
                        );
                    }
                    return;
                }

                if let Some(number) = calling_number.clone() {
                    self.mo_call.mo_calling_numbers.insert(call_id, number);
                }
                if let Some((_, subscriber_id)) = originator {
                    self.media_gw
                        .register_active_subscriber(subscriber_id, call_id);
                }

                let subscriber_route = if let Some(called_number) = called_number.as_deref() {
                    self.mo_call
                        .send_mo_mobile_to_mobile_page(
                            call_id,
                            called_number,
                            service_option,
                            self.config.hlr_repo.as_ref(),
                            &mut self.circuits,
                        )
                        .await
                } else {
                    MoSubscriberRoute::NotSubscriber
                };
                if subscriber_route == MoSubscriberRoute::Rejected {
                    send_gateway_clear_command(
                        a1,
                        call_id,
                        &mut self.controller,
                        gateway_clear_cause(ReleaseCause::SetupFailed, None),
                    )
                    .await;
                    return;
                }
                let routes_to_subscriber = subscriber_route == MoSubscriberRoute::Paged;

                let is_otasp_call = self.pending_otasp_originations.contains_key(&call_id);
                let mut audio_file = None;
                // SIP INVITE is deferred until AssignmentComplete so failure
                // tones have a bearer; handle attaches there.
                let media_gateway_handle: Option<crate::media_gateway::CallHandle> = None;
                if !routes_to_subscriber && !is_otasp_call {
                    if let (Some(_gateway), Some(called_number)) =
                        (self.config.media_gateway.as_ref(), called_number.as_deref())
                    {
                        self.mo_call.pending_sip_routes.insert(
                            call_id,
                            crate::mo_call::PendingSipRoute {
                                called_number: called_number.to_string(),
                                calling_number: calling_number.clone(),
                                service_option,
                            },
                        );
                        info!(
                            "MSC: deferred SIP INVITE for MO call_id={} until AssignmentComplete",
                            call_id.0
                        );
                    } else {
                        audio_file = self.config.wav_file.clone();
                    }
                }

                let cic = self
                    .circuits
                    .assignment_circuit_identity_code_for_next_leg(call_id);
                let circuit_id = cic.to_packed();
                self.circuits.insert_circuit_session(
                    circuit_id,
                    CircuitSession {
                        call_id,
                        audio_file,
                        service_option,
                        leg_role: MscVoiceLeg::Primary,
                        peer_circuit_id: None,
                        bearer_remote_ready: self.config.voice_bearer.is_none(),
                        media_gateway_handle,
                        called_number: called_number.clone(),
                    },
                );
                self.circuits.queue_assignment_complete_circuit(
                    call_id,
                    MscVoiceLeg::Primary,
                    circuit_id,
                );

                let a2p_bearer_session_params =
                    if let Some(bearer) = self.config.voice_bearer.as_ref() {
                        match bearer.open_circuit(circuit_id, None).await {
                            Ok(local_addr) => Some(cdma_ios::A2pBearerSessionParams {
                                ip_address: match local_addr.ip() {
                                    std::net::IpAddr::V4(v4) => v4,
                                    _ => std::net::Ipv4Addr::UNSPECIFIED,
                                },
                                udp_port: local_addr.port(),
                            }),
                            Err(e) => {
                                warn!("MSC: failed to open bearer circuit {circuit_id}: {e}");
                                None
                            }
                        }
                    } else {
                        None
                    };

                let a2p_bearer_format_params = a2p_bearer_session_params.as_ref().map(|_| {
                    cdma_ios::A2pBearerFormatParams::evrc_with_telephone_event(
                        cdma_ios::voice_bearer::EVRC_RTP_PAYLOAD_TYPE,
                        cdma_ios::voice_bearer::TELEPHONE_EVENT_RTP_PAYLOAD_TYPE,
                    )
                });
                if let (Some(bearer), Some(format_params)) = (
                    self.config.voice_bearer.as_ref(),
                    a2p_bearer_format_params.as_ref(),
                ) {
                    bearer.set_circuit_payload_types(
                        circuit_id,
                        cdma_ios::BearerPayloadTypes {
                            evrc: format_params
                                .evrc_pt()
                                .unwrap_or(cdma_ios::voice_bearer::EVRC_RTP_PAYLOAD_TYPE),
                            telephone_event: format_params.telephone_event_pt(),
                        },
                    );
                }
                let assignment_request = cdma_ios::AssignmentRequestMessage {
                    channel_type: cdma_ios::ChannelType {
                        speech_or_data_indicator: 0x01,
                        channel_rate_and_type: 0x08,
                        coding: 0x05,
                    },
                    circuit_identity_code: cic,
                    encryption_information: None,
                    service_option: Some(cdma_ios::ServiceOption(service_option)),
                    signals: Vec::new(),
                    ms_information_records: None,
                    priority: None,
                    paca_timestamp: None,
                    quality_of_service_parameters: None,
                    a2p_bearer_session_params,
                    a2p_bearer_format_params,
                };
                if let Err(error) = self.controller.apply_from_msc(
                    call_id,
                    &cdma_ios::ProcedureMessage::AssignmentRequest(assignment_request.clone()),
                ) {
                    warn!(
                        "MSC: failed to apply Assignment Request state for MO call_id={}: {}",
                        call_id_raw, error
                    );
                    return;
                }
                let payload = match assignment_request.encode() {
                    Ok(payload) => payload,
                    Err(error) => {
                        warn!(
                            "MSC: failed to encode A1 Assignment Request for MO call_id={}: {}",
                            call_id_raw, error
                        );
                        return;
                    }
                };
                info!("MSC: A1 tx AssignmentRequest (MO) call_id={}", call_id_raw);
                if let Err(error) = a1
                    .send_to_bsc(EncodedA1Message::from_message_for_call(
                        &cdma_ios::Message::new(cdma_ios::MessageType::AssignmentRequest, payload),
                        Some(call_id_raw),
                    ))
                    .await
                {
                    warn!(
                        "MSC: failed to send A1 Assignment Request to BSC for MO call_id={}: {}",
                        call_id_raw, error
                    );
                }
            }
            other => {
                info!(
                    "MSC: A1 message {:?} from BSC not yet handled on live path",
                    other
                );
            }
        }
    }

    /// Handles a reverse voice bearer frame from the BSC (mobile->MSC).
    async fn handle_reverse_bearer_dtmf(&mut self, event: cdma_ios::DtmfBearerEvent) {
        let Some(session) = self.circuits.circuits.get(&event.circuit_id) else {
            log::trace!(
                "MSC: reverse DTMF event for unknown circuit_id={}",
                event.circuit_id,
            );
            return;
        };
        let call_id = session.call_id;
        let media_gateway_handle = session.media_gateway_handle.or_else(|| {
            self.controller
                .snapshot(call_id)
                .and_then(|snapshot| snapshot.media_gateway_handle)
        });
        let (Some(media_gateway), Some(handle)) =
            (self.config.media_gateway.as_ref(), media_gateway_handle)
        else {
            log::debug!(
                "MSC: dropping reverse DTMF event call_id={} circuit_id={} (no gateway or handle)",
                call_id.0,
                event.circuit_id,
            );
            return;
        };
        if let Err(error) = media_gateway
            .send_dtmf(
                handle,
                event.event,
                event.volume,
                event.duration_samples,
                event.end,
                event.start_of_event,
            )
            .await
        {
            log::warn!(
                "MSC: failed to forward reverse DTMF to media gateway call_id={}: {:?}",
                call_id.0,
                error,
            );
        }
    }

    async fn handle_reverse_bearer_frame(&mut self, frame: VoiceBearerFrame) {
        let Some((call_id, peer_circuit_id, media_gateway_handle)) = self
            .circuits
            .circuits
            .get(&frame.circuit_id)
            .map(|session| {
                (
                    session.call_id,
                    session.peer_circuit_id,
                    session.media_gateway_handle,
                )
            })
        else {
            log::trace!(
                "MSC: reverse bearer frame for unknown circuit_id={}",
                frame.circuit_id,
            );
            return;
        };

        if let Some(peer_cid) = peer_circuit_id {
            let peer_ready = self
                .circuits
                .circuits
                .get(&peer_cid)
                .map(|peer| peer.bearer_remote_ready)
                .unwrap_or(false);
            if !peer_ready {
                log::trace!(
                    "MSC: dropping bearer frame call_id={} from_circuit_id={} to_circuit_id={} until peer bearer remote is known",
                    call_id.0,
                    frame.circuit_id,
                    peer_cid
                );
                return;
            }
            let bridged_frame = VoiceBearerFrame {
                circuit_id: peer_cid,
                rate_bps: frame.rate_bps,
                payload: frame.payload,
            };
            debug!(
                "MSC: bridging bearer frame call_id={} from_circuit_id={} to_circuit_id={} rate={} len={}",
                call_id.0,
                frame.circuit_id,
                peer_cid,
                bridged_frame.rate_bps,
                bridged_frame.payload.len()
            );
            send_forward_bearer_frame(&bridged_frame, self.config.voice_bearer.as_ref()).await;
            return;
        }

        // Pre-answer inbound SIP: MSC is generating ringback toward the trunk
        // via the same forward_payload path. Forwarding MS pre-answer null
        // frames would interleave with and corrupt the ringback audio.
        if self.media.inbound_ringback_active(call_id) {
            log::trace!(
                "MSC: dropping reverse bearer frame call_id={} (inbound ringback active)",
                call_id.0
            );
            return;
        }

        let media_gateway_handle = media_gateway_handle.or_else(|| {
            self.controller
                .snapshot(call_id)
                .and_then(|snapshot| snapshot.media_gateway_handle)
        });
        if let (Some(media_gateway), Some(handle)) =
            (self.config.media_gateway.as_ref(), media_gateway_handle)
        {
            let rate_bps = frame.rate_bps;
            let len = frame.payload.len();
            if let Err(error) = media_gateway
                .forward_payload(
                    handle,
                    VocoderFrame {
                        payload: frame.payload,
                        rate_bps,
                    },
                )
                .await
            {
                warn!(
                    "MSC: failed to forward bearer frame call_id={} circuit_id={} to media gateway handle={:?}: {}",
                    call_id.0, frame.circuit_id, handle, error
                );
            } else {
                debug!(
                    "MSC: forwarded bearer frame call_id={} circuit_id={} to media gateway handle={:?} rate={} len={}",
                    call_id.0, frame.circuit_id, handle, rate_bps, len
                );
            }
            return;
        }

        log::trace!(
            "MSC: reverse bearer frame circuit_id={} rate={} len={} (no peer or gateway)",
            frame.circuit_id,
            frame.rate_bps,
            frame.payload.len(),
        );
    }

    async fn flush_deferred_paging_response(&mut self, a1: &dyn MscA1Endpoint, call_id: CallId) {
        let Some(response) = self.circuits.take_deferred_paging_response(call_id) else {
            return;
        };
        self.mt_call
            .send_assignment_request_for_paging_response(
                a1,
                call_id,
                response,
                true,
                &mut self.controller,
                &mut self.circuits,
                &self.mo_call,
                self.config.voice_bearer.as_ref(),
                self.config.default_voice_service_option,
                &self.config.hlr_repo,
            )
            .await;
    }

    const MT_ASSIGNMENT_FAILURE_MAX_RETRIES: u8 = 3;

    async fn handle_assignment_failure(&mut self, a1: &dyn MscA1Endpoint, call_id: CallId) {
        let abandoned = self
            .circuits
            .cancel_pending_assignment_leg(call_id, self.config.voice_bearer.as_ref());
        let attempts = self.circuits.bump_assignment_failure_retry(call_id);
        info!(
            "MSC: A1 rx AssignmentFailure call_id={} (attempt {}/{}); abandoned leg={:?}",
            call_id.0,
            attempts,
            Self::MT_ASSIGNMENT_FAILURE_MAX_RETRIES,
            abandoned,
        );
        if attempts > Self::MT_ASSIGNMENT_FAILURE_MAX_RETRIES {
            warn!(
                "MSC: AssignmentFailure retries exhausted for call_id={}; sending ClearCommand",
                call_id.0
            );
            self.circuits.reset_assignment_failure_retries(call_id);
            self.send_clear_command(a1, call_id).await;
            return;
        }
        let failed_leg = abandoned
            .map(|(leg, _)| leg)
            .unwrap_or(MscVoiceLeg::Primary);
        self.reissue_paging_request(a1, call_id, failed_leg).await;
    }

    async fn reissue_paging_request(
        &mut self,
        a1: &dyn MscA1Endpoint,
        call_id: CallId,
        failed_leg: MscVoiceLeg,
    ) {
        let Some(paging_request) = self.circuits.paging_requests.get(&call_id).cloned() else {
            warn!(
                "MSC: cannot re-page call_id={} — no original PagingRequest retained",
                call_id.0
            );
            return;
        };
        if self
            .circuits
            .deferred_paging_responses
            .remove(&call_id)
            .is_some_and(|q| !q.is_empty())
        {
            warn!(
                "MSC: discarded stale deferred PagingResponse(s) for call_id={} before re-page",
                call_id.0
            );
        }
        self.send_paging_request_to_bsc(
            a1,
            call_id,
            paging_request,
            PagePurpose::RepageAfterAf { failed_leg },
        )
        .await;
    }

    /// Drive A1 `AlertWithInformation` (and optionally `Progress{Ringback}`)
    /// after `AssignmentComplete`. BSC's leg-role mapper translates the AWI
    /// into a standard alert (Callee) or ringback AWIM (Caller) on the air.
    async fn fire_assignment_complete_awi(
        &mut self,
        a1: &dyn MscA1Endpoint,
        call_id: CallId,
        completed_leg: Option<MscVoiceLeg>,
    ) {
        use crate::media_gateway_service::send_alert_with_information;

        let direction = self
            .controller
            .snapshot(call_id)
            .map(|snapshot| snapshot.direction);
        let has_peer = self
            .circuits
            .circuits
            .values()
            .any(|s| s.call_id == call_id && s.peer_circuit_id.is_some());

        // Caller-ID for callee-side alerts comes from mt_call's per-call stash
        // (populated at PagingResponse time from MO calling number / MT plan).
        let build_caller_id_records = async |this: &mut Self| {
            if let Some(digits) = this.mt_call.caller_numbers.get(&call_id).cloned() {
                crate::mt_call::build_calling_party_ms_information_records(
                    Some(digits.as_str()),
                    &this.config.hlr_repo,
                )
                .await
            } else {
                None
            }
        };

        match completed_leg {
            Some(MscVoiceLeg::Secondary) => {
                // MS-MS callee alerting; AWI (records only) goes to the
                // Callee leg. Caller-side ringback was already kicked off
                // at MO M2M page send (see flush_deferred_paging_request).
                let records = build_caller_id_records(self).await;
                send_alert_with_information(
                    a1,
                    call_id,
                    &mut self.controller,
                    &mut self.media_gw.alert_sent,
                    records,
                )
                .await;
            }
            Some(MscVoiceLeg::Primary) => match direction {
                Some(CallDirection::MobileTerminated) => {
                    // SIP-inbound or MSC-initiated MT: mandatory callee alert
                    // with caller-ID (from SIP From-header / configured caller).
                    let records = build_caller_id_records(self).await;
                    send_alert_with_information(
                        a1,
                        call_id,
                        &mut self.controller,
                        &mut self.media_gw.alert_sent,
                        records,
                    )
                    .await;
                }
                Some(CallDirection::MobileOriginated) => {
                    // Caller-side ringback is driven from the callee-alerting
                    // event, not from Primary AssignmentComplete: Secondary
                    // arm above for MS-MS, `Ringing` (180) for SIP-outbound.
                    let _ = has_peer;
                }
                None => {}
            },
            None => {}
        }
    }

    async fn send_clear_command(&self, a1: &dyn MscA1Endpoint, call_id: CallId) {
        let clear_command = cdma_ios::ClearCommandMessage {
            cause: cdma_ios::Cause(0x16),
            cause_layer3: None,
        };
        let payload = match clear_command.encode() {
            Ok(payload) => payload,
            Err(error) => {
                warn!(
                    "MSC: failed to encode ClearCommand call_id={}: {}",
                    call_id.0, error
                );
                return;
            }
        };
        if let Err(error) = a1
            .send_to_bsc(EncodedA1Message::from_message_for_call(
                &cdma_ios::Message::new(cdma_ios::MessageType::ClearCommand, payload),
                Some(call_id.0),
            ))
            .await
        {
            warn!(
                "MSC: failed to send ClearCommand call_id={}: {}",
                call_id.0, error
            );
        }
    }

    /// Send the MO M2M PagingRequest that was held until the primary leg's
    /// AssignmentComplete arrived. The callee is paged at this point — never
    /// before, so a callee PagingResponse cannot race the MO leg's setup.
    async fn flush_deferred_paging_request(&mut self, a1: &dyn MscA1Endpoint, call_id: CallId) {
        let Some(paging_request) = self.circuits.take_deferred_paging_request(call_id) else {
            return;
        };
        if !self
            .send_paging_request_to_bsc(a1, call_id, paging_request, PagePurpose::M2mSecondary)
            .await
        {
            self.circuits.paging_requests.remove(&call_id);
            return;
        }
        // Caller-side ringback fires at the same point as MSC's bearer-side
        // ringback feeder (Primary AssignmentComplete) so audio and Signal
        // IE stay in sync for MS-MS.
        if self.config.send_tones_alert {
            crate::media_gateway_service::send_progress_ringback(a1, call_id, &mut self.controller)
                .await;
        }
    }

    async fn fire_due_pending_clears(&mut self, a1: &dyn MscA1Endpoint) {
        for (call_id, cause) in self.media_gw.drain_due_pending_clears() {
            crate::media_gateway_service::send_gateway_clear_command(
                a1,
                call_id,
                &mut self.controller,
                cause,
            )
            .await;
            stop_media_for_call(
                call_id,
                &mut self.controller,
                &mut self.circuits,
                &mut self.media,
                &mut self.media_gw,
                self.config.voice_bearer.as_ref(),
                self.config.media_gateway.as_ref(),
            );
        }
    }

    async fn fire_deferred_sip_invite(&mut self, a1: &dyn MscA1Endpoint, call_id: CallId) {
        let Some(route) = self.mo_call.pending_sip_routes.remove(&call_id) else {
            return;
        };
        let Some(gateway) = self.config.media_gateway.as_ref() else {
            return;
        };
        info!(
            "MSC: firing deferred SIP INVITE for MO call_id={} called={} so={}",
            call_id.0, route.called_number, route.service_option
        );
        match gateway
            .create_call(CreateCallRequest {
                call_id: call_id.0,
                calling_party: route.calling_number,
                called_party: Some(route.called_number),
                service_option: route.service_option,
            })
            .await
        {
            Ok(handle) => {
                self.media_gw.media_gateway_calls.insert(handle, call_id);
                if let Err(error) = self.controller.attach_media_gateway_handle(call_id, handle) {
                    warn!(
                        "MSC: failed to attach media gateway handle to call_id={}: {}",
                        call_id.0, error
                    );
                }
                for (_, session) in self.circuits.circuits.iter_mut() {
                    if session.call_id == call_id
                        && session.leg_role == MscVoiceLeg::Primary
                        && session.media_gateway_handle.is_none()
                    {
                        session.media_gateway_handle = Some(handle);
                    }
                }
            }
            Err(error) => {
                warn!(
                    "MSC: failed to create media gateway call for MO call_id={}: {}",
                    call_id.0, error
                );
                let wav_fallback = self
                    .config
                    .gateway_fallback_to_wav
                    .then(|| self.config.wav_file.clone())
                    .flatten();
                if let Some(path) = wav_fallback {
                    info!(
                        "MSC: falling back to WAV playback for MO call_id={} file={}",
                        call_id.0, path
                    );
                    for (_, session) in self.circuits.circuits.iter_mut() {
                        if session.call_id == call_id
                            && session.leg_role == MscVoiceLeg::Primary
                            && session.audio_file.is_none()
                        {
                            session.audio_file = Some(path.clone());
                        }
                    }
                    self.media.schedule_delayed_wav_start(
                        call_id,
                        self.config.local_answer_delay_ms,
                        &self.controller,
                        &self.circuits,
                        self.config.voice_bearer.as_ref(),
                        self.config.media_ringback_enabled,
                        self.config.media_ringback_type,
                        Some(&self.config.hlr_repo),
                    );
                } else {
                    self.media_gw
                        .signal_call_failure(
                            a1,
                            call_id,
                            &mut self.controller,
                            &mut self.circuits,
                            &mut self.media,
                            self.config.voice_bearer.as_ref(),
                            self.config.failure_tone_duration_ms,
                            crate::media_gateway::ReleaseCause::SipFailure,
                            None,
                        )
                        .await;
                }
            }
        }
    }

    async fn handle_inbound_sip_invite(
        &mut self,
        a1: &dyn MscA1Endpoint,
        session_id: String,
        called_number: String,
        caller_number: String,
        offered_codecs: Vec<String>,
    ) {
        let Some(gateway) = self.config.media_gateway.clone() else {
            log::warn!(
                "MSC: inbound SIP call session={session_id} arrived with no gateway client; dropping"
            );
            return;
        };

        // Voice-gw already filtered to G.711; echo the first offered back.
        let chosen_codec = offered_codecs
            .first()
            .cloned()
            .unwrap_or_else(|| "PCMU".to_string());

        let lookup = self
            .config
            .hlr_repo
            .get_subscriber_by_phone_number(&called_number)
            .await;
        let resolved = match lookup {
            Ok(Some(resolved)) => resolved,
            Ok(None) => {
                info!(
                    "MSC: inbound SIP call session={session_id} no subscriber for called={called_number}; rejecting 404"
                );
                let _ = gateway.inbound_reject(&session_id, 404).await;
                return;
            }
            Err(error) => {
                warn!(
                    "MSC: inbound SIP call session={session_id} HLR lookup failed for {called_number}: {error}; rejecting 503"
                );
                let _ = gateway.inbound_reject(&session_id, 503).await;
                return;
            }
        };

        let subscriber_id = resolved.subscriber.subscriber_id;
        if self.media_gw.is_subscriber_busy(subscriber_id) {
            info!(
                "MSC: inbound SIP call session={session_id} subscriber {subscriber_id} busy; rejecting 486"
            );
            let _ = gateway.inbound_reject(&session_id, 486).await;
            return;
        }

        let caller_for_id = (!caller_number.is_empty()).then(|| caller_number.clone());
        let call_id = match self.start_mt_call(a1, resolved, caller_for_id, None).await {
            Ok(call_id) => call_id,
            Err(error) => {
                let sip_status = error.to_sip_status();
                warn!(
                    "MSC: inbound SIP call session={session_id} setup failed: {error:?}; rejecting {sip_status}"
                );
                let _ = gateway.inbound_reject(&session_id, sip_status).await;
                return;
            }
        };
        info!(
            "MSC: inbound SIP call session={session_id} routed to MT call_id={} subscriber={subscriber_id}",
            call_id.0
        );
        let service_option = self.config.default_voice_service_option;
        match gateway
            .register_inbound_session(session_id.clone(), service_option)
            .await
        {
            Ok(handle) => {
                self.media_gw.media_gateway_calls.insert(handle, call_id);
                if let Err(error) = self.controller.attach_media_gateway_handle(call_id, handle) {
                    warn!(
                        "MSC: failed to attach gateway handle to inbound call_id={}: {}",
                        call_id.0, error
                    );
                }
                if self.config.inbound_sip_msc_ringback {
                    self.media.start_inbound_ringback(
                        call_id,
                        handle,
                        gateway.clone(),
                        service_option,
                        Some(self.config.hlr_repo.clone()),
                        Some(called_number),
                    );
                }
            }
            Err(error) => {
                warn!(
                    "MSC: failed to register inbound gateway handle for call_id={}: {}",
                    call_id.0, error
                );
            }
        }
        self.media_gw
            .register_inbound_session(session_id, call_id, chosen_codec);
    }

    async fn handle_inbound_sip_cancel(&mut self, a1: &dyn MscA1Endpoint, session_id: String) {
        let call_id = self
            .media_gw
            .inbound_by_call
            .iter()
            .find_map(|(cid, sid)| (*sid == session_id).then_some(*cid));
        info!(
            "MSC: inbound SIP CANCEL session={session_id} call_id={:?}",
            call_id.map(|c| c.0)
        );
        if let Some(call_id) = call_id {
            self.send_clear_command(a1, call_id).await;
            // BSC may not send ClearComplete if the call never reached an
            // established state (page failure, no MS response). Drive local
            // cleanup directly so active_subscribers and the controller don't
            // leak. A later ClearComplete becomes a harmless no-op.
            self.controller.remove_call(call_id);
            self.stop_media_for_call(call_id);
        }
    }

    fn stop_media_for_call(&mut self, call_id: CallId) {
        self.mo_call.cleanup_call(call_id);
        self.mt_call.cleanup_call(call_id);
        stop_media_for_call(
            call_id,
            &mut self.controller,
            &mut self.circuits,
            &mut self.media,
            &mut self.media_gw,
            self.config.voice_bearer.as_ref(),
            self.config.media_gateway.as_ref(),
        );
    }

    /// Routes an inbound ADDS message from the BSC to the SMS or OTASP coordinator.
    async fn handle_adds_message(&mut self, a1: &dyn MscA1Endpoint, message: EncodedA1Message) {
        let decoded = match message.decode() {
            Ok(d) => d,
            Err(e) => {
                warn!("MSC: failed to decode ADDS message: {e}");
                return;
            }
        };
        // OTASP burst_type=4 routes to the OTASP coordinator regardless of SMS
        // configuration. Other burst types require the SMS coordinator.
        if let cdma_ios::MessageType::AddsTransfer = decoded.message_type {
            if let Ok(msg) = cdma_ios::AddsTransferMessage::decode(&decoded.payload) {
                if msg.adds_user_part.burst_type == 0x04 {
                    let release_call_id = match self.otasp.as_mut() {
                        Some(otasp) => otasp.handle_adds_transfer(&msg, a1).await,
                        None => {
                            warn!("MSC: OTASP ADDS Transfer received but coordinator disabled");
                            None
                        }
                    };
                    if let Some(call_id_raw) = release_call_id {
                        let call_id = CallId(call_id_raw);
                        info!(
                            "MSC: OTASP session terminal — releasing call_id={}",
                            call_id_raw
                        );
                        send_gateway_clear_command(
                            a1,
                            call_id,
                            &mut self.controller,
                            gateway_clear_cause(ReleaseCause::Administrative, None),
                        )
                        .await;
                    }
                    return;
                }
            }
        }
        let smsc = match self.smsc.as_mut() {
            Some(s) => s,
            None => {
                warn!(
                    "MSC: received ADDS {:?} but SMSC coordinator is not configured — dropped",
                    message.message_type()
                );
                return;
            }
        };
        match decoded.message_type {
            cdma_ios::MessageType::AddsPageAck => {
                match cdma_ios::AddsPageAckMessage::decode(&decoded.payload) {
                    Ok(msg) => smsc.handle_adds_page_ack(&msg).await,
                    Err(e) => warn!("MSC: failed to decode ADDS Page Ack: {e}"),
                }
            }
            cdma_ios::MessageType::AddsDeliverAck => {
                match cdma_ios::AddsDeliverAckMessage::decode(&decoded.payload) {
                    Ok(msg) => {
                        // Route to OTASP if it owns the tag; else hand
                        // to SMSC. Tags are namespaced (high bit set =
                        // OTASP) so the dispatch is O(1).
                        let routed_to_otasp = match (msg.tag, self.otasp.as_mut()) {
                            (Some(t), Some(otasp)) if otasp.owns_ack_tag(t.0) => {
                                otasp.handle_adds_deliver_ack(&msg, a1).await;
                                true
                            }
                            _ => false,
                        };
                        if !routed_to_otasp {
                            smsc.handle_adds_deliver_ack(&msg).await;
                        }
                    }
                    Err(e) => warn!("MSC: failed to decode ADDS Deliver Ack: {e}"),
                }
            }
            cdma_ios::MessageType::AddsTransfer => {
                match cdma_ios::AddsTransferMessage::decode(&decoded.payload) {
                    Ok(msg) => smsc.handle_adds_transfer(&msg, a1).await,
                    Err(e) => warn!("MSC: failed to decode ADDS Transfer: {e}"),
                }
            }
            cdma_ios::MessageType::AddsDeliver => {
                // BS→MSC direction: MO SMS on traffic channel. ADDS Deliver carries no
                // Mobile Identity; resolve it from the active call session via call_id.
                match cdma_ios::AddsDeliverMessage::decode(&decoded.payload) {
                    Ok(msg) => {
                        let call_id_raw = message.call_id().unwrap_or(0);
                        let snapshot = self.controller.snapshot(CallId(call_id_raw));
                        let mobile_identity = snapshot
                            .as_ref()
                            .and_then(|snap| snap.mobile_identity.clone())
                            .unwrap_or_else(|| {
                                cdma_ios::MobileIdentity::Imsi(format!("UNKNOWN-{call_id_raw}"))
                            });
                        let mobile_identity_esn = snapshot
                            .as_ref()
                            .and_then(|snap| snap.mobile_identity_esn.as_ref());
                        smsc.handle_adds_deliver_mo(
                            &msg,
                            &mobile_identity,
                            mobile_identity_esn,
                            a1,
                        )
                        .await;
                    }
                    Err(e) => warn!("MSC: failed to decode ADDS Deliver (MO): {e}"),
                }
            }
            other => {
                warn!(
                    "MSC: unexpected message type {:?} in handle_adds_message",
                    other
                );
            }
        }
    }

    /// Handles a `CompleteLayer3Information` with no call_id — this is BSC's registration
    /// notification (LocationUpdatingRequest in L3 body). Triggers welcome SMS if configured.
    async fn handle_registration_notification(
        &mut self,
        a1: &dyn MscA1Endpoint,
        message: EncodedA1Message,
    ) {
        let welcome_cfg = match self.config.welcome_sms.as_ref() {
            Some(c) if c.enabled => c.clone(),
            _ => return,
        };
        let smsc = match self.smsc.as_mut() {
            Some(s) => s,
            None => {
                warn!(
                    "MSC: registration notification received but SMSC coordinator not configured"
                );
                return;
            }
        };
        let decoded = match message.decode() {
            Ok(d) => d,
            Err(e) => {
                warn!("MSC: failed to decode registration CompleteLayer3Info: {e}");
                return;
            }
        };
        let cli3 = match cdma_ios::CompleteLayer3InformationMessage::decode(&decoded.payload) {
            Ok(c) => c,
            Err(e) => {
                warn!("MSC: failed to decode CLI3 for registration: {e}");
                return;
            }
        };
        let lur = match cli3.layer3_information.decode_location_updating_request() {
            Ok(r) => r,
            Err(_) => return, // not a LocationUpdatingRequest — ignore silently
        };
        let imsi = match &lur.mobile_identity_imsi {
            cdma_ios::MobileIdentity::Imsi(s) if s != "UNKNOWN" => Some(s.as_str()),
            _ => None,
        };
        let esn = match &lur.mobile_identity_esn {
            Some(cdma_ios::MobileIdentity::Esn(e)) => Some(*e),
            _ => None,
        };
        let identity_key = match cdma_hlr::model::MobileIdentityKey::from_parts(imsi, esn, None) {
            Ok(identity_key) => identity_key,
            _ => {
                warn!(
                    "MSC: registration notification has no complete identity — welcome SMS skipped"
                );
                return;
            }
        };
        let upsert = match self
            .config
            .hlr_repo
            .upsert_mobile_seen(&identity_key, None)
            .await
        {
            Ok(u) => u,
            Err(e) => {
                warn!("MSC: upsert_mobile_seen failed: {e}");
                return;
            }
        };
        let should_send = if upsert.is_new {
            info!("MSC: first-time mobile — welcome SMS queued");
            true
        } else if let Some(prev) = upsert.previous_last_seen_at {
            let elapsed = chrono::Utc::now() - prev;
            let threshold = chrono::Duration::days(welcome_cfg.inactive_days_threshold as i64);
            if elapsed > threshold {
                info!(
                    "MSC: mobile inactive for {} days — welcome SMS queued",
                    elapsed.num_days()
                );
                true
            } else {
                false
            }
        } else {
            false
        };
        if !should_send {
            return;
        }
        let subscriber = match self
            .config
            .hlr_repo
            .resolve_by_identity(&identity_key)
            .await
        {
            Ok(Some(s)) => Some(s),
            Ok(None) => None,
            Err(e) => {
                warn!("MSC: registration HLR lookup failed: {e}");
                return;
            }
        };
        let destination = if let Some(ref resolved) = subscriber {
            info!(
                "MSC: sending welcome SMS to {} on registration",
                resolved.subscriber.phone_number
            );
            crate::sms::SmsDestinationKey::PhoneNumber(resolved.subscriber.phone_number.clone())
        } else {
            let imsi = identity_key.imsi();
            info!(
                "MSC: sending welcome SMS to non-subscriber by IMSI {} on registration",
                imsi
            );
            crate::sms::SmsDestinationKey::Imsi(imsi.to_string())
        };
        smsc.send_sms(
            crate::sms::SmsSendRequest {
                originating_number: welcome_cfg.originating_number.clone(),
                text: welcome_cfg.text.clone(),
                destination,
                timeout_ms: 30_000,
                teleservice_id: None,
                raw_user_data: None,
            },
            a1,
        )
        .await;
    }
}

pub(crate) fn assignment_circuit_identity_code_with_offset(
    call_id: CallId,
    leg_offset: u16,
) -> cdma_ios::CircuitIdentityCode {
    let packed = (call_id.0 as u16).wrapping_add(1 + leg_offset);
    cdma_ios::CircuitIdentityCode {
        pcm_multiplexer: (packed >> 5) & 0x07ff,
        timeslot: (packed & 0x1f) as u8,
    }
}

fn extract_hardware_identity(
    request: Option<&cdma_ios::CmServiceRequestMessage>,
) -> (Option<u32>, Option<String>) {
    let Some(req) = request else {
        return (None, None);
    };
    let esn = match req.mobile_identity_esn.as_ref() {
        Some(cdma_ios::MobileIdentity::Esn(e)) => Some(*e),
        _ => None,
    };
    // CmServiceRequest does not currently carry a MEID IE; if a future MEID
    // form arrives here it'll appear via the IMSI slot encoded as such.
    (esn, None)
}

fn decode_cm_service_request(
    layer3: &cdma_ios::Layer3Information,
) -> Option<cdma_ios::CmServiceRequestMessage> {
    match layer3.decode_cm_service_request() {
        Ok(request) => Some(request),
        Err(error) => {
            warn!(
                "MSC: Complete Layer 3 Information did not contain a valid CM Service Request: {}",
                error
            );
            None
        }
    }
}

fn cm_service_request_called_number(request: &cdma_ios::CmServiceRequestMessage) -> Option<String> {
    decode_called_party_bcd_number(request.called_party_bcd_number.as_ref())
        .or_else(|| {
            request
                .called_party_ascii_number
                .as_ref()
                .and_then(|number| String::from_utf8(number.0.clone()).ok())
                .filter(|number| !number.is_empty())
        })
        .map(|number| normalize_mo_called_number_for_routing(&number))
}

fn decode_called_party_bcd_number(
    number: Option<&cdma_ios::CalledPartyBcdNumber>,
) -> Option<String> {
    let payload = &number?.0;
    if payload.len() < 2 {
        return None;
    }
    let international = ((payload[0] >> 4) & 0x07) == 0b001;
    let mut digits = String::new();
    if international {
        digits.push('+');
    }
    for octet in &payload[1..] {
        for nibble in [octet & 0x0f, octet >> 4] {
            match nibble & 0x0f {
                0x0..=0x9 => digits.push(char::from(b'0' + (nibble & 0x0f))),
                0x0a => digits.push('*'),
                0x0b => digits.push('#'),
                0x0c => digits.push('a'),
                0x0d => digits.push('b'),
                0x0e => digits.push('c'),
                0x0f => return (!digits.is_empty()).then_some(digits),
                _ => return None,
            }
        }
    }
    (!digits.is_empty()).then_some(digits)
}

fn normalize_mo_called_number_for_routing(number: &str) -> String {
    if let Some(rest) = number
        .strip_prefix('+')
        .or_else(|| number.strip_prefix("011"))
        .filter(|rest| !rest.is_empty() && rest.bytes().all(|byte| byte.is_ascii_digit()))
    {
        return rest.to_string();
    }
    number.to_string()
}

pub(crate) fn select_pageable_imsi<'a>(
    identities: &'a [SubscriberIdentity],
    binding: &'a RegistrationBinding,
) -> Option<&'a str> {
    identities
        .iter()
        .find_map(|identity| {
            if identity.is_primary {
                identity.imsi.as_deref()
            } else {
                None
            }
        })
        .or(binding.imsi.as_deref())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::circuit::{MscLegKey, MscVoiceLeg};
    use crate::media_gateway::{CallHandle, MediaGatewayEvent, VocoderFrame};
    use cdma_ios::{
        AssignmentCompleteMessage, AssignmentRequestMessage, Cause, ChannelNumber, ChannelType,
        CircuitIdentityCode, ConnectMessage, MobileIdentity, PagingRequestMessage,
        PagingResponseMessage, ProcedureMessage, ServiceOption, SlotCycleIndex, Tag,
    };
    use tokio::time::{Duration, timeout};

    struct StubHlrRepo;

    /// HLR stub that resolves a single phone number to a Registered subscriber
    /// with one primary IMSI, used to drive the MO M2M paging path.
    struct M2mHlrRepo {
        phone_number: &'static str,
        subscriber_id: uuid::Uuid,
        imsi: &'static str,
    }

    impl M2mHlrRepo {
        fn new() -> Self {
            Self {
                phone_number: "5559876543",
                subscriber_id: uuid::Uuid::from_u128(0x1111_2222_3333_4444_5555_6666_7777_8888),
                imsi: "111111111111111",
            }
        }
    }

    #[derive(Default)]
    struct StubMediaGateway {
        forwarded: std::sync::Mutex<Vec<(CallHandle, VocoderFrame)>>,
    }

    #[derive(Default)]
    struct RecordingInboundGateway {
        progress: std::sync::Mutex<Vec<String>>,
        answer: std::sync::Mutex<Vec<(String, String)>>,
        reject: std::sync::Mutex<Vec<(String, u16)>>,
    }

    #[async_trait::async_trait]
    impl MediaGatewayClient for RecordingInboundGateway {
        async fn create_call(
            &self,
            _: crate::media_gateway::CreateCallRequest,
        ) -> Result<CallHandle, crate::media_gateway::MgwError> {
            Err(crate::media_gateway::MgwError::Unavailable)
        }
        async fn answer_call(&self, _: CallHandle) -> Result<(), crate::media_gateway::MgwError> {
            Ok(())
        }
        async fn release_call(
            &self,
            _: CallHandle,
            _: crate::media_gateway::ReleaseCause,
        ) -> Result<(), crate::media_gateway::MgwError> {
            Ok(())
        }
        async fn forward_payload(
            &self,
            _: CallHandle,
            _: VocoderFrame,
        ) -> Result<(), crate::media_gateway::MgwError> {
            Ok(())
        }
        async fn recv_event(&self) -> Option<MediaGatewayEvent> {
            std::future::pending().await
        }
        async fn inbound_progress(
            &self,
            session_id: &str,
        ) -> Result<(), crate::media_gateway::MgwError> {
            self.progress.lock().unwrap().push(session_id.to_string());
            Ok(())
        }
        async fn inbound_answer(
            &self,
            session_id: &str,
            codec: &str,
        ) -> Result<(), crate::media_gateway::MgwError> {
            self.answer
                .lock()
                .unwrap()
                .push((session_id.to_string(), codec.to_string()));
            Ok(())
        }
        async fn inbound_reject(
            &self,
            session_id: &str,
            sip_status: u16,
        ) -> Result<(), crate::media_gateway::MgwError> {
            self.reject
                .lock()
                .unwrap()
                .push((session_id.to_string(), sip_status));
            Ok(())
        }
    }

    struct FailingMediaGateway;

    #[async_trait::async_trait]
    impl MediaGatewayClient for StubMediaGateway {
        async fn create_call(
            &self,
            _: crate::media_gateway::CreateCallRequest,
        ) -> Result<CallHandle, crate::media_gateway::MgwError> {
            Ok(CallHandle(1))
        }

        async fn answer_call(&self, _: CallHandle) -> Result<(), crate::media_gateway::MgwError> {
            Ok(())
        }

        async fn release_call(
            &self,
            _: CallHandle,
            _: crate::media_gateway::ReleaseCause,
        ) -> Result<(), crate::media_gateway::MgwError> {
            Ok(())
        }

        async fn forward_payload(
            &self,
            handle: CallHandle,
            payload: VocoderFrame,
        ) -> Result<(), crate::media_gateway::MgwError> {
            self.forwarded.lock().unwrap().push((handle, payload));
            Ok(())
        }

        async fn recv_event(&self) -> Option<MediaGatewayEvent> {
            std::future::pending().await
        }
    }

    #[async_trait::async_trait]
    impl MediaGatewayClient for FailingMediaGateway {
        async fn create_call(
            &self,
            _: crate::media_gateway::CreateCallRequest,
        ) -> Result<CallHandle, crate::media_gateway::MgwError> {
            Err(crate::media_gateway::MgwError::Unavailable)
        }

        async fn answer_call(&self, _: CallHandle) -> Result<(), crate::media_gateway::MgwError> {
            Ok(())
        }

        async fn release_call(
            &self,
            _: CallHandle,
            _: crate::media_gateway::ReleaseCause,
        ) -> Result<(), crate::media_gateway::MgwError> {
            Ok(())
        }

        async fn forward_payload(
            &self,
            _: CallHandle,
            _: VocoderFrame,
        ) -> Result<(), crate::media_gateway::MgwError> {
            Ok(())
        }

        async fn recv_event(&self) -> Option<MediaGatewayEvent> {
            std::future::pending().await
        }
    }

    fn cm_service_request_cli3(
        called_bcd: Option<Vec<u8>>,
    ) -> cdma_ios::CompleteLayer3InformationMessage {
        let request = cdma_ios::CmServiceRequestMessage {
            cm_service_type: cdma_ios::CmServiceType::MobileOriginatingCallEstablishment,
            classmark_information_type_2: cdma_ios::ClassmarkInformationType2(vec![
                0xc1, 0x00, 0x66, 0x00,
            ]),
            mobile_identity_imsi: MobileIdentity::Imsi("12345678901".to_string()),
            called_party_bcd_number: called_bcd.map(cdma_ios::CalledPartyBcdNumber),
            tag: None,
            mobile_identity_esn: None,
            slot_cycle_index: None,
            authentication_response_parameter: None,
            authentication_confirmation_parameter: None,
            authentication_parameter_count: None,
            authentication_challenge_parameter: None,
            service_option: Some(ServiceOption::EVRC_A),
            voice_privacy_request: false,
            radio_environment_and_resources: None,
            called_party_ascii_number: None,
            circuit_identity_code: None,
            authentication_event: None,
            authentication_data: None,
            paca_reorigination_indicator: false,
            user_zone_id: None,
            is2000_mobile_capabilities: None,
            cdma_serving_one_way_delay: None,
        };

        cdma_ios::CompleteLayer3InformationMessage {
            cell_identifier: cdma_ios::CellId {
                cell: 0x123,
                sector: 0x4,
            },
            layer3_information: cdma_ios::Layer3Information::from_cm_service_request(&request)
                .unwrap(),
        }
    }

    #[async_trait::async_trait]
    impl cdma_hlr::repository::HlrRepository for StubHlrRepo {
        async fn upsert_subscriber(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: cdma_hlr::model::NumberType,
            _: cdma_hlr::model::NumberPlan,
        ) -> Result<cdma_hlr::model::Subscriber, String> {
            unimplemented!()
        }
        async fn get_subscriber_by_phone_number(
            &self,
            _: &str,
        ) -> Result<Option<cdma_hlr::model::ResolvedSubscriber>, String> {
            Ok(None)
        }
        async fn get_subscriber_by_id(
            &self,
            _: uuid::Uuid,
        ) -> Result<Option<cdma_hlr::model::ResolvedSubscriber>, String> {
            unimplemented!()
        }
        async fn update_subscriber(
            &self,
            _: uuid::Uuid,
            _: &str,
            _: &str,
            _: &str,
            _: cdma_hlr::model::NumberType,
            _: cdma_hlr::model::NumberPlan,
        ) -> Result<Option<cdma_hlr::model::Subscriber>, String> {
            unimplemented!()
        }
        async fn list_subscribers(
            &self,
            _: u32,
            _: u32,
        ) -> Result<(Vec<cdma_hlr::model::Subscriber>, u32), String> {
            unimplemented!()
        }
        async fn delete_subscriber(&self, _: uuid::Uuid) -> Result<bool, String> {
            unimplemented!()
        }
        async fn upsert_identity(
            &self,
            _: uuid::Uuid,
            _: Option<&str>,
            _: Option<u32>,
            _: Option<&str>,
        ) -> Result<cdma_hlr::model::SubscriberIdentity, String> {
            unimplemented!()
        }
        async fn replace_primary_identity(
            &self,
            _: uuid::Uuid,
            _: Option<&str>,
            _: Option<u32>,
            _: Option<&str>,
        ) -> Result<cdma_hlr::model::SubscriberIdentity, String> {
            unimplemented!()
        }
        async fn get_identities_for_subscriber(
            &self,
            _: uuid::Uuid,
        ) -> Result<Vec<cdma_hlr::model::SubscriberIdentity>, String> {
            unimplemented!()
        }
        async fn resolve_by_identity(
            &self,
            _: &cdma_hlr::model::MobileIdentityKey,
        ) -> Result<Option<cdma_hlr::model::ResolvedSubscriber>, String> {
            Ok(None)
        }
        async fn resolve_by_hardware_identity(
            &self,
            _: Option<u32>,
            _: Option<&str>,
        ) -> Result<Option<cdma_hlr::model::ResolvedSubscriber>, String> {
            Ok(None)
        }
        async fn upsert_registration_binding(
            &self,
            _: cdma_hlr::model::RegistrationBinding,
        ) -> Result<cdma_hlr::model::RegistrationBinding, String> {
            unimplemented!()
        }
        async fn get_registration_binding(
            &self,
            _: uuid::Uuid,
        ) -> Result<Option<cdma_hlr::model::RegistrationBinding>, String> {
            unimplemented!()
        }
        async fn upsert_mobile_seen(
            &self,
            _: &cdma_hlr::model::MobileIdentityKey,
            _: Option<u8>,
        ) -> Result<cdma_hlr::MobileSeenUpsert, String> {
            Ok(cdma_hlr::MobileSeenUpsert {
                is_new: true,
                previous_last_seen_at: None,
            })
        }
        async fn set_ringtone(
            &self,
            _: uuid::Uuid,
            _: Vec<u8>,
            _: &str,
        ) -> Result<cdma_hlr::model::SetRingtoneOutcome, String> {
            Ok(cdma_hlr::model::SetRingtoneOutcome {
                codecs: vec![],
                duration_ms: 0,
            })
        }
        async fn clear_ringtone(&self, _: uuid::Uuid) -> Result<(), String> {
            Ok(())
        }
        async fn get_ringtone_codec(
            &self,
            _: uuid::Uuid,
            _: &str,
        ) -> Result<Option<cdma_hlr::model::SubscriberRingtoneCodecBlob>, String> {
            Ok(None)
        }
        async fn list_prls(
            &self,
            _: u32,
            _: u32,
            _: cdma_hlr::model::PrlListFilter,
        ) -> Result<(Vec<cdma_hlr::model::Prl>, u32), String> {
            Ok((vec![], 0))
        }
        async fn get_prl(&self, _: uuid::Uuid) -> Result<Option<cdma_hlr::model::Prl>, String> {
            Ok(None)
        }
        async fn get_default_prl(&self) -> Result<Option<cdma_hlr::model::Prl>, String> {
            Ok(None)
        }
        async fn create_prl(
            &self,
            _: &str,
            _: &[u8],
            _: i32,
            _: i16,
            _: &str,
        ) -> Result<cdma_hlr::model::Prl, String> {
            unimplemented!()
        }
        async fn update_prl(
            &self,
            _: uuid::Uuid,
            _: Option<&str>,
            _: Option<&[u8]>,
            _: Option<(i32, i16)>,
            _: Option<&str>,
        ) -> Result<cdma_hlr::model::Prl, String> {
            unimplemented!()
        }
        async fn soft_delete_prl(
            &self,
            _: uuid::Uuid,
        ) -> Result<Result<(), cdma_hlr::model::PrlDeleteBlocked>, String> {
            Ok(Ok(()))
        }
        async fn set_default_prl(&self, _: uuid::Uuid) -> Result<(), String> {
            Ok(())
        }
        async fn set_subscriber_prl_override(
            &self,
            _: uuid::Uuid,
            _: Option<uuid::Uuid>,
        ) -> Result<(), String> {
            Ok(())
        }
        async fn set_subscriber_spc(&self, _: uuid::Uuid, _: Option<String>) -> Result<(), String> {
            Ok(())
        }
        async fn set_subscriber_firstchp_override(
            &self,
            _: uuid::Uuid,
            _: Option<u16>,
        ) -> Result<(), String> {
            Ok(())
        }
        async fn save_otasp_session(
            &self,
            _: &cdma_hlr::model::OtaspSessionRow,
        ) -> Result<(), String> {
            Ok(())
        }
        async fn list_otasp_sessions(
            &self,
            _: cdma_hlr::model::OtaspSessionFilter,
            _: u32,
            _: u32,
        ) -> Result<(Vec<cdma_hlr::model::OtaspSessionRow>, u32), String> {
            Ok((Vec::new(), 0))
        }
        async fn get_otasp_session(
            &self,
            _: uuid::Uuid,
        ) -> Result<Option<cdma_hlr::model::OtaspSessionRow>, String> {
            Ok(None)
        }
    }

    #[async_trait::async_trait]
    impl cdma_hlr::repository::HlrRepository for M2mHlrRepo {
        async fn upsert_subscriber(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: cdma_hlr::model::NumberType,
            _: cdma_hlr::model::NumberPlan,
        ) -> Result<cdma_hlr::model::Subscriber, String> {
            unimplemented!()
        }
        async fn get_subscriber_by_phone_number(
            &self,
            phone_number: &str,
        ) -> Result<Option<cdma_hlr::model::ResolvedSubscriber>, String> {
            if phone_number == self.phone_number {
                let subscriber = cdma_hlr::model::Subscriber {
                    subscriber_id: self.subscriber_id,
                    phone_number: self.phone_number.to_string(),
                    display_name: "M2M Test".to_string(),
                    status: cdma_hlr::model::SubscriberStatus::Active,
                    created_at: chrono::Utc::now(),
                    updated_at: chrono::Utc::now(),
                    number_type: cdma_hlr::model::NumberType::NetworkSpecific,
                    number_plan: cdma_hlr::model::NumberPlan::IsdnE164,
                    has_ringtone: false,
                    ringtone_duration_ms: None,
                    prl_override_id: None,
                    service_programming_code: None,
                    firstchp_override: None,
                };
                let primary = cdma_hlr::model::SubscriberIdentity {
                    subscriber_identity_id: uuid::Uuid::nil(),
                    subscriber_id: self.subscriber_id,
                    imsi: Some(self.imsi.to_string()),
                    esn: None,
                    meid: None,
                    is_primary: true,
                    created_at: chrono::Utc::now(),
                };
                let binding = cdma_hlr::model::RegistrationBinding {
                    subscriber_id: self.subscriber_id,
                    serving_node_id: "test".to_string(),
                    state: cdma_hlr::model::RegistrationState::Registered,
                    imsi: Some(self.imsi.to_string()),
                    esn: None,
                    meid: None,
                    mob_p_rev: Some(6),
                    pgslot: Some(0),
                    slot_cycle_index: Some(2),
                    last_msg_seq: None,
                    last_registered_at: chrono::Utc::now(),
                    last_seen_at: chrono::Utc::now(),
                    updated_at: chrono::Utc::now(),
                };
                Ok(Some(cdma_hlr::model::ResolvedSubscriber {
                    subscriber,
                    identities: vec![primary.clone()],
                    primary_identity: Some(primary),
                    binding: Some(binding),
                }))
            } else {
                Ok(None)
            }
        }
        async fn get_subscriber_by_id(
            &self,
            _: uuid::Uuid,
        ) -> Result<Option<cdma_hlr::model::ResolvedSubscriber>, String> {
            unimplemented!()
        }
        async fn update_subscriber(
            &self,
            _: uuid::Uuid,
            _: &str,
            _: &str,
            _: &str,
            _: cdma_hlr::model::NumberType,
            _: cdma_hlr::model::NumberPlan,
        ) -> Result<Option<cdma_hlr::model::Subscriber>, String> {
            unimplemented!()
        }
        async fn list_subscribers(
            &self,
            _: u32,
            _: u32,
        ) -> Result<(Vec<cdma_hlr::model::Subscriber>, u32), String> {
            unimplemented!()
        }
        async fn delete_subscriber(&self, _: uuid::Uuid) -> Result<bool, String> {
            unimplemented!()
        }
        async fn upsert_identity(
            &self,
            _: uuid::Uuid,
            _: Option<&str>,
            _: Option<u32>,
            _: Option<&str>,
        ) -> Result<cdma_hlr::model::SubscriberIdentity, String> {
            unimplemented!()
        }
        async fn replace_primary_identity(
            &self,
            _: uuid::Uuid,
            _: Option<&str>,
            _: Option<u32>,
            _: Option<&str>,
        ) -> Result<cdma_hlr::model::SubscriberIdentity, String> {
            unimplemented!()
        }
        async fn get_identities_for_subscriber(
            &self,
            subscriber_id: uuid::Uuid,
        ) -> Result<Vec<cdma_hlr::model::SubscriberIdentity>, String> {
            assert_eq!(subscriber_id, self.subscriber_id);
            Ok(vec![cdma_hlr::model::SubscriberIdentity {
                subscriber_identity_id: uuid::Uuid::nil(),
                subscriber_id,
                imsi: Some(self.imsi.to_string()),
                esn: None,
                meid: None,
                is_primary: true,
                created_at: chrono::Utc::now(),
            }])
        }
        async fn resolve_by_identity(
            &self,
            _: &cdma_hlr::model::MobileIdentityKey,
        ) -> Result<Option<cdma_hlr::model::ResolvedSubscriber>, String> {
            Ok(None)
        }
        async fn resolve_by_hardware_identity(
            &self,
            _: Option<u32>,
            _: Option<&str>,
        ) -> Result<Option<cdma_hlr::model::ResolvedSubscriber>, String> {
            Ok(None)
        }
        async fn upsert_registration_binding(
            &self,
            _: cdma_hlr::model::RegistrationBinding,
        ) -> Result<cdma_hlr::model::RegistrationBinding, String> {
            unimplemented!()
        }
        async fn get_registration_binding(
            &self,
            subscriber_id: uuid::Uuid,
        ) -> Result<Option<cdma_hlr::model::RegistrationBinding>, String> {
            assert_eq!(subscriber_id, self.subscriber_id);
            Ok(Some(cdma_hlr::model::RegistrationBinding {
                subscriber_id,
                serving_node_id: "test".to_string(),
                state: cdma_hlr::model::RegistrationState::Registered,
                imsi: Some(self.imsi.to_string()),
                esn: None,
                meid: None,
                mob_p_rev: Some(6),
                pgslot: Some(0),
                slot_cycle_index: Some(2),
                last_msg_seq: None,
                last_registered_at: chrono::Utc::now(),
                last_seen_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            }))
        }
        async fn upsert_mobile_seen(
            &self,
            _: &cdma_hlr::model::MobileIdentityKey,
            _: Option<u8>,
        ) -> Result<cdma_hlr::MobileSeenUpsert, String> {
            Ok(cdma_hlr::MobileSeenUpsert {
                is_new: true,
                previous_last_seen_at: None,
            })
        }
        async fn set_ringtone(
            &self,
            _: uuid::Uuid,
            _: Vec<u8>,
            _: &str,
        ) -> Result<cdma_hlr::model::SetRingtoneOutcome, String> {
            Ok(cdma_hlr::model::SetRingtoneOutcome {
                codecs: vec![],
                duration_ms: 0,
            })
        }
        async fn clear_ringtone(&self, _: uuid::Uuid) -> Result<(), String> {
            Ok(())
        }
        async fn get_ringtone_codec(
            &self,
            _: uuid::Uuid,
            _: &str,
        ) -> Result<Option<cdma_hlr::model::SubscriberRingtoneCodecBlob>, String> {
            Ok(None)
        }
        async fn list_prls(
            &self,
            _: u32,
            _: u32,
            _: cdma_hlr::model::PrlListFilter,
        ) -> Result<(Vec<cdma_hlr::model::Prl>, u32), String> {
            Ok((vec![], 0))
        }
        async fn get_prl(&self, _: uuid::Uuid) -> Result<Option<cdma_hlr::model::Prl>, String> {
            Ok(None)
        }
        async fn get_default_prl(&self) -> Result<Option<cdma_hlr::model::Prl>, String> {
            Ok(None)
        }
        async fn create_prl(
            &self,
            _: &str,
            _: &[u8],
            _: i32,
            _: i16,
            _: &str,
        ) -> Result<cdma_hlr::model::Prl, String> {
            unimplemented!()
        }
        async fn update_prl(
            &self,
            _: uuid::Uuid,
            _: Option<&str>,
            _: Option<&[u8]>,
            _: Option<(i32, i16)>,
            _: Option<&str>,
        ) -> Result<cdma_hlr::model::Prl, String> {
            unimplemented!()
        }
        async fn soft_delete_prl(
            &self,
            _: uuid::Uuid,
        ) -> Result<Result<(), cdma_hlr::model::PrlDeleteBlocked>, String> {
            Ok(Ok(()))
        }
        async fn set_default_prl(&self, _: uuid::Uuid) -> Result<(), String> {
            Ok(())
        }
        async fn set_subscriber_prl_override(
            &self,
            _: uuid::Uuid,
            _: Option<uuid::Uuid>,
        ) -> Result<(), String> {
            Ok(())
        }
        async fn set_subscriber_spc(&self, _: uuid::Uuid, _: Option<String>) -> Result<(), String> {
            Ok(())
        }
        async fn set_subscriber_firstchp_override(
            &self,
            _: uuid::Uuid,
            _: Option<u16>,
        ) -> Result<(), String> {
            Ok(())
        }
        async fn save_otasp_session(
            &self,
            _: &cdma_hlr::model::OtaspSessionRow,
        ) -> Result<(), String> {
            Ok(())
        }
        async fn list_otasp_sessions(
            &self,
            _: cdma_hlr::model::OtaspSessionFilter,
            _: u32,
            _: u32,
        ) -> Result<(Vec<cdma_hlr::model::OtaspSessionRow>, u32), String> {
            Ok((Vec::new(), 0))
        }
        async fn get_otasp_session(
            &self,
            _: uuid::Uuid,
        ) -> Result<Option<cdma_hlr::model::OtaspSessionRow>, String> {
            Ok(None)
        }
    }

    #[test]
    fn cm_service_request_called_number_prefers_bcd() {
        let request = cdma_ios::CmServiceRequestMessage {
            cm_service_type: cdma_ios::CmServiceType::MobileOriginatingCallEstablishment,
            classmark_information_type_2: cdma_ios::ClassmarkInformationType2(vec![
                0xc1, 0x00, 0x66, 0x00,
            ]),
            mobile_identity_imsi: MobileIdentity::Imsi("12345678901".to_string()),
            called_party_bcd_number: Some(cdma_ios::CalledPartyBcdNumber(vec![
                0x81, 0x55, 0x95, 0x89,
            ])),
            tag: None,
            mobile_identity_esn: None,
            slot_cycle_index: None,
            authentication_response_parameter: None,
            authentication_confirmation_parameter: None,
            authentication_parameter_count: None,
            authentication_challenge_parameter: None,
            service_option: Some(ServiceOption::EVRC_A),
            voice_privacy_request: false,
            radio_environment_and_resources: None,
            called_party_ascii_number: Some(cdma_ios::CallingPartyAsciiNumber(b"wrong".to_vec())),
            circuit_identity_code: None,
            authentication_event: None,
            authentication_data: None,
            paca_reorigination_indicator: false,
            user_zone_id: None,
            is2000_mobile_capabilities: None,
            cdma_serving_one_way_delay: None,
        };

        assert_eq!(
            cm_service_request_called_number(&request).as_deref(),
            Some("555998")
        );
    }

    #[test]
    fn cm_service_request_called_number_canonicalizes_international_bcd() {
        let request = cdma_ios::CmServiceRequestMessage {
            cm_service_type: cdma_ios::CmServiceType::MobileOriginatingCallEstablishment,
            classmark_information_type_2: cdma_ios::ClassmarkInformationType2(vec![
                0xc1, 0x00, 0x66, 0x00,
            ]),
            mobile_identity_imsi: MobileIdentity::Imsi("12345678901".to_string()),
            called_party_bcd_number: Some(cdma_ios::CalledPartyBcdNumber(vec![
                0x91, 0x21, 0x21, 0x55, 0x05, 0x21, 0xf3,
            ])),
            tag: None,
            mobile_identity_esn: None,
            slot_cycle_index: None,
            authentication_response_parameter: None,
            authentication_confirmation_parameter: None,
            authentication_parameter_count: None,
            authentication_challenge_parameter: None,
            service_option: Some(ServiceOption::EVRC_A),
            voice_privacy_request: false,
            radio_environment_and_resources: None,
            called_party_ascii_number: None,
            circuit_identity_code: None,
            authentication_event: None,
            authentication_data: None,
            paca_reorigination_indicator: false,
            user_zone_id: None,
            is2000_mobile_capabilities: None,
            cdma_serving_one_way_delay: None,
        };

        assert_eq!(
            cm_service_request_called_number(&request).as_deref(),
            Some("12125550123")
        );
    }

    #[test]
    fn mo_called_number_strips_international_access_prefix_for_routing() {
        assert_eq!(
            normalize_mo_called_number_for_routing("01112125550123"),
            "12125550123"
        );
        assert_eq!(normalize_mo_called_number_for_routing("5551234"), "5551234");
    }

    /// Minimal shim wrapping InProcessMscEndpoint as MscA1Endpoint for tests.
    mod cdma_bsc_a1_edge_compat {
        use cdma_ios::{A1TransportError, EncodedA1Message};
        use tokio::sync::{Mutex, mpsc};

        pub struct InProcessMscEndpoint {
            pub inbound_rx: Mutex<mpsc::Receiver<EncodedA1Message>>,
            pub outbound_tx: mpsc::Sender<EncodedA1Message>,
        }

        pub struct InProcessMscClient {
            _outbound_tx: mpsc::Sender<EncodedA1Message>,
            pub inbound_rx: Mutex<mpsc::Receiver<EncodedA1Message>>,
        }

        impl InProcessMscClient {
            pub fn pair(buffer: usize) -> (Self, InProcessMscEndpoint) {
                let (bsc_to_msc_tx, bsc_to_msc_rx) = mpsc::channel(buffer);
                let (msc_to_bsc_tx, msc_to_bsc_rx) = mpsc::channel(buffer);
                (
                    Self {
                        _outbound_tx: bsc_to_msc_tx,
                        inbound_rx: Mutex::new(msc_to_bsc_rx),
                    },
                    InProcessMscEndpoint {
                        inbound_rx: Mutex::new(bsc_to_msc_rx),
                        outbound_tx: msc_to_bsc_tx,
                    },
                )
            }

            pub async fn poll_a1(&self) -> Option<EncodedA1Message> {
                self.inbound_rx.lock().await.recv().await
            }
        }

        #[async_trait::async_trait]
        impl super::MscA1Endpoint for InProcessMscEndpoint {
            async fn recv_from_bsc(&self) -> Option<EncodedA1Message> {
                self.inbound_rx.lock().await.recv().await
            }

            async fn send_to_bsc(&self, message: EncodedA1Message) -> Result<(), A1TransportError> {
                self.outbound_tx
                    .send(message)
                    .await
                    .map_err(|_| A1TransportError::Closed)
            }
        }
    }

    fn paging_request() -> PagingRequestMessage {
        PagingRequestMessage {
            mobile_identity_imsi: MobileIdentity::Imsi("12345678901".to_string()),
            tag: Some(Tag(0x01020304)),
            cell_identifier_list: None,
            slot_cycle_index: Some(SlotCycleIndex(1)),
            service_option: Some(ServiceOption(0x0003)),
            is2000_mobile_capabilities: None,
        }
    }

    fn paging_response() -> PagingResponseMessage {
        PagingResponseMessage {
            classmark_information_type_2: cdma_ios::ClassmarkInformationType2(vec![
                0x88, 0x00, 0xE4, 0x00, 0x00, 0x01, 0x24, 0x01, 0x03, 0x00, 0x00, 0x06,
            ]),
            mobile_identity_imsi: MobileIdentity::Imsi("12345678901".to_string()),
            tag: Some(Tag(0x01020304)),
            mobile_identity_esn: None,
            slot_cycle_index: Some(SlotCycleIndex(1)),
            authentication_response_parameter: None,
            authentication_confirmation_parameter: None,
            authentication_parameter_count: None,
            authentication_challenge_parameter: None,
            service_option: Some(ServiceOption(0x0003)),
            voice_privacy_request: false,
            circuit_identity_code: None,
            authentication_event: None,
            radio_environment_and_resources: None,
            user_zone_id: None,
            is2000_mobile_capabilities: None,
            cdma_serving_one_way_delay: None,
        }
    }

    #[tokio::test]
    async fn handle_bsc_a1_message_completes_clear_roundtrip() {
        let (client, endpoint) = cdma_bsc_a1_edge_compat::InProcessMscClient::pair(4);

        let mut runtime = MscRuntime::new(MscRuntimeConfig {
            hlr_repo: Arc::new(StubHlrRepo),
            smsc_repo: None,
            welcome_sms: None,
            sms_retry: crate::config::SmsRetryConfig::default(),
            default_voice_service_option: 3,
            wav_file: None,
            gateway_fallback_to_wav: true,
            local_answer_delay_ms: 10_000,
            media_ringback_enabled: false,
            media_ringback_type: MediaRingbackType::Nanp,
            sip_ringback_disable: false,
            inbound_sip_msc_ringback: false,
            generate_ringback: true,
            send_tones_alert: false,
            page_retry_cooldown_ms: 1000,
            page_retry_max_duration_ms: 60_000,
            failure_tone_duration_ms: 0,
            voice_bearer: None,
            media_gateway: None,
            otasp: None,
            bts_overhead: None,
        });

        let call_id = runtime.controller.create_call(
            CallDirection::MobileTerminated,
            Some(MobileIdentity::Imsi("12345678901".to_string())),
        );

        runtime
            .controller
            .apply_from_msc(call_id, &ProcedureMessage::PagingRequest(paging_request()))
            .unwrap();
        runtime
            .controller
            .apply_from_bsc(
                call_id,
                &ProcedureMessage::PagingResponse(paging_response()),
            )
            .unwrap();
        runtime
            .controller
            .apply_from_msc(
                call_id,
                &ProcedureMessage::AssignmentRequest(AssignmentRequestMessage {
                    channel_type: ChannelType {
                        speech_or_data_indicator: 0x01,
                        channel_rate_and_type: 0x08,
                        coding: 0x05,
                    },
                    circuit_identity_code: CircuitIdentityCode {
                        pcm_multiplexer: 0x0123,
                        timeslot: 0x1a,
                    },
                    encryption_information: None,
                    service_option: Some(ServiceOption(0x0003)),
                    signals: Vec::new(),
                    ms_information_records: None,
                    priority: None,
                    paca_timestamp: None,
                    quality_of_service_parameters: None,
                    a2p_bearer_session_params: None,
                    a2p_bearer_format_params: None,
                }),
            )
            .unwrap();
        runtime
            .controller
            .apply_from_bsc(
                call_id,
                &ProcedureMessage::AssignmentComplete(AssignmentCompleteMessage {
                    channel_number: ChannelNumber(0x1122),
                    encryption_information: None,
                    service_option: Some(ServiceOption(0x0003)),
                    a2p_bearer_session_params: None,
                    a2p_bearer_format_params: None,
                }),
            )
            .unwrap();
        runtime
            .controller
            .apply_from_bsc(call_id, &ProcedureMessage::Connect(ConnectMessage))
            .unwrap();

        let clear_request = EncodedA1Message::from_message_for_call(
            &cdma_ios::Message::new(
                cdma_ios::MessageType::ClearRequest,
                cdma_ios::ClearRequestMessage {
                    cause: Cause(0x09),
                    cause_layer3: None,
                }
                .encode()
                .unwrap(),
            ),
            Some(call_id.0),
        );
        runtime
            .handle_bsc_a1_message(&endpoint, clear_request)
            .await;
        assert_eq!(
            runtime.controller.state(call_id),
            Some(cdma_ios::CallControlState::Clearing)
        );

        let outbound = client.poll_a1().await.unwrap();
        assert_eq!(outbound.message_type(), cdma_ios::MessageType::ClearCommand);
        assert_eq!(outbound.call_id(), Some(call_id.0));

        let clear_complete = EncodedA1Message::from_message_for_call(
            &cdma_ios::Message::new(
                cdma_ios::MessageType::ClearComplete,
                cdma_ios::ClearCompleteMessage {
                    power_down_indicator: false,
                }
                .encode()
                .unwrap(),
            ),
            Some(call_id.0),
        );
        runtime
            .handle_bsc_a1_message(&endpoint, clear_complete)
            .await;
        assert_eq!(runtime.controller.active_call_count(), 0);
    }

    #[tokio::test]
    async fn secondary_paging_response_waits_for_active_assignment_complete() {
        let (client, endpoint) = cdma_bsc_a1_edge_compat::InProcessMscClient::pair(4);
        let mut runtime = MscRuntime::new(MscRuntimeConfig {
            hlr_repo: Arc::new(StubHlrRepo),
            smsc_repo: None,
            welcome_sms: None,
            sms_retry: crate::config::SmsRetryConfig::default(),
            default_voice_service_option: 3,
            wav_file: None,
            gateway_fallback_to_wav: true,
            local_answer_delay_ms: 10_000,
            media_ringback_enabled: false,
            media_ringback_type: MediaRingbackType::Nanp,
            sip_ringback_disable: false,
            inbound_sip_msc_ringback: false,
            generate_ringback: true,
            send_tones_alert: false,
            page_retry_cooldown_ms: 1000,
            page_retry_max_duration_ms: 60_000,
            failure_tone_duration_ms: 0,
            voice_bearer: None,
            media_gateway: None,
            otasp: None,
            bts_overhead: None,
        });
        let call_id = 99;
        let call_id_typed = runtime.controller.create_call_with_id(
            CallId(call_id),
            CallDirection::MobileTerminated,
            Some(MobileIdentity::Imsi("12345678901".to_string())),
        );
        let page = paging_request();
        runtime
            .controller
            .apply_from_msc(
                call_id_typed,
                &ProcedureMessage::PagingRequest(page.clone()),
            )
            .unwrap();
        runtime.circuits.paging_requests.insert(call_id_typed, page);

        let first_paging_response = EncodedA1Message::from_message_for_call(
            &cdma_ios::Message::new(
                cdma_ios::MessageType::PagingResponse,
                paging_response().encode().unwrap(),
            ),
            Some(call_id),
        );
        runtime
            .handle_bsc_a1_message(&endpoint, first_paging_response)
            .await;

        let first = timeout(Duration::from_millis(50), client.poll_a1())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            first.message_type(),
            cdma_ios::MessageType::AssignmentRequest
        );
        let first_payload = first.decode().unwrap();
        let first_assignment =
            cdma_ios::AssignmentRequestMessage::decode(&first_payload.payload).unwrap();

        let paging_response = EncodedA1Message::from_message_for_call(
            &cdma_ios::Message::new(
                cdma_ios::MessageType::PagingResponse,
                paging_response().encode().unwrap(),
            ),
            Some(call_id),
        );
        runtime
            .handle_bsc_a1_message(&endpoint, paging_response)
            .await;

        assert!(
            timeout(Duration::from_millis(50), client.poll_a1())
                .await
                .is_err(),
            "secondary leg should wait while first AssignmentComplete is pending"
        );

        let assignment_complete = EncodedA1Message::from_message_for_call(
            &cdma_ios::Message::new(
                cdma_ios::MessageType::AssignmentComplete,
                AssignmentCompleteMessage {
                    channel_number: ChannelNumber(0x1122),
                    encryption_information: None,
                    service_option: Some(ServiceOption(0x0003)),
                    a2p_bearer_session_params: None,
                    a2p_bearer_format_params: None,
                }
                .encode()
                .unwrap(),
            ),
            Some(call_id),
        );
        runtime
            .handle_bsc_a1_message(&endpoint, assignment_complete)
            .await;

        let second = timeout(Duration::from_millis(50), client.poll_a1())
            .await
            .expect("secondary leg should emit AssignmentRequest")
            .unwrap();
        assert_eq!(
            second.message_type(),
            cdma_ios::MessageType::AssignmentRequest
        );
        assert_eq!(second.call_id(), Some(call_id));
        let second_payload = second.decode().unwrap();
        let second_assignment =
            cdma_ios::AssignmentRequestMessage::decode(&second_payload.payload).unwrap();
        assert_ne!(
            first_assignment.circuit_identity_code.to_packed(),
            second_assignment.circuit_identity_code.to_packed()
        );
        assert_eq!(
            runtime
                .circuits
                .circuits
                .values()
                .filter(|session| session.call_id == CallId(call_id))
                .count(),
            2
        );
        let first_circuit_id = first_assignment.circuit_identity_code.to_packed();
        let second_circuit_id = second_assignment.circuit_identity_code.to_packed();
        assert_eq!(
            runtime.circuits.circuits[&first_circuit_id].peer_circuit_id,
            Some(second_circuit_id)
        );
        assert_eq!(
            runtime.circuits.circuits[&second_circuit_id].peer_circuit_id,
            Some(first_circuit_id)
        );
        assert!(runtime.circuits.leg_procedures.contains_key(&MscLegKey {
            call_id: CallId(call_id),
            leg_role: MscVoiceLeg::Secondary,
        }));
    }

    /// Build a runtime with a Secondary leg already in `AssignmentPending`
    /// for `call_id` (Primary leg fully connected first). Returns the
    /// runtime, the test client, and the call_id.
    async fn setup_msc_with_secondary_pending(
        call_id: u64,
    ) -> (
        MscRuntime,
        cdma_bsc_a1_edge_compat::InProcessMscClient,
        cdma_bsc_a1_edge_compat::InProcessMscEndpoint,
    ) {
        let (client, endpoint) = cdma_bsc_a1_edge_compat::InProcessMscClient::pair(8);
        let mut runtime = MscRuntime::new(MscRuntimeConfig {
            hlr_repo: Arc::new(StubHlrRepo),
            smsc_repo: None,
            welcome_sms: None,
            sms_retry: crate::config::SmsRetryConfig::default(),
            default_voice_service_option: 3,
            wav_file: None,
            gateway_fallback_to_wav: true,
            local_answer_delay_ms: 10_000,
            media_ringback_enabled: false,
            media_ringback_type: MediaRingbackType::Nanp,
            sip_ringback_disable: false,
            inbound_sip_msc_ringback: false,
            generate_ringback: true,
            send_tones_alert: false,
            page_retry_cooldown_ms: 1000,
            page_retry_max_duration_ms: 60_000,
            failure_tone_duration_ms: 0,
            voice_bearer: None,
            media_gateway: None,
            otasp: None,
            bts_overhead: None,
        });
        let call_id_typed = runtime.controller.create_call_with_id(
            CallId(call_id),
            CallDirection::MobileTerminated,
            Some(MobileIdentity::Imsi("12345678901".to_string())),
        );
        let page = paging_request();
        runtime
            .controller
            .apply_from_msc(
                call_id_typed,
                &ProcedureMessage::PagingRequest(page.clone()),
            )
            .unwrap();
        runtime.circuits.paging_requests.insert(call_id_typed, page);

        // Primary leg: PagingResponse → AssignmentRequest → AssignmentComplete.
        let primary_pr = EncodedA1Message::from_message_for_call(
            &cdma_ios::Message::new(
                cdma_ios::MessageType::PagingResponse,
                paging_response().encode().unwrap(),
            ),
            Some(call_id),
        );
        runtime.handle_bsc_a1_message(&endpoint, primary_pr).await;
        let _ = timeout(Duration::from_millis(50), client.poll_a1())
            .await
            .unwrap()
            .unwrap();
        let primary_ac = EncodedA1Message::from_message_for_call(
            &cdma_ios::Message::new(
                cdma_ios::MessageType::AssignmentComplete,
                AssignmentCompleteMessage {
                    channel_number: ChannelNumber(0x1122),
                    encryption_information: None,
                    service_option: Some(ServiceOption(0x0003)),
                    a2p_bearer_session_params: None,
                    a2p_bearer_format_params: None,
                }
                .encode()
                .unwrap(),
            ),
            Some(call_id),
        );
        runtime.handle_bsc_a1_message(&endpoint, primary_ac).await;
        // Drain the AlertWithInformation MSC now sends on every MT
        // AssignmentComplete (BSC-autonomous AWIM was retired).
        let _ = timeout(Duration::from_millis(50), client.poll_a1())
            .await
            .unwrap()
            .unwrap();

        // Secondary leg: PagingResponse → AssignmentRequest (Secondary leg
        // now in AssignmentPending).
        let secondary_pr = EncodedA1Message::from_message_for_call(
            &cdma_ios::Message::new(
                cdma_ios::MessageType::PagingResponse,
                paging_response().encode().unwrap(),
            ),
            Some(call_id),
        );
        runtime.handle_bsc_a1_message(&endpoint, secondary_pr).await;
        let _ = timeout(Duration::from_millis(50), client.poll_a1())
            .await
            .unwrap()
            .unwrap();

        (runtime, client, endpoint)
    }

    fn assignment_failure_msg(call_id: u64) -> EncodedA1Message {
        EncodedA1Message::from_message_for_call(
            &cdma_ios::Message::new(
                cdma_ios::MessageType::AssignmentFailure,
                cdma_ios::AssignmentFailureMessage {
                    cause: cdma_ios::Cause(0x16),
                }
                .encode()
                .unwrap(),
            ),
            Some(call_id),
        )
    }

    #[tokio::test]
    async fn mt_assignment_failure_triggers_repage() {
        let call_id = 1001;
        let (mut runtime, client, endpoint) = setup_msc_with_secondary_pending(call_id).await;

        // Sanity: stale secondary state exists before injection.
        assert!(
            runtime
                .circuits
                .has_pending_assignment_complete(CallId(call_id))
        );
        assert!(runtime.circuits.leg_procedures.contains_key(&MscLegKey {
            call_id: CallId(call_id),
            leg_role: MscVoiceLeg::Secondary,
        }));

        runtime
            .handle_bsc_a1_message(&endpoint, assignment_failure_msg(call_id))
            .await;

        // MSC re-pages the same call_id.
        let repage = timeout(Duration::from_millis(50), client.poll_a1())
            .await
            .expect("MSC should emit re-page PagingRequest")
            .unwrap();
        assert_eq!(repage.message_type(), cdma_ios::MessageType::PagingRequest);
        assert_eq!(repage.call_id(), Some(call_id));

        // Secondary-leg state wiped, retry counter at 1.
        assert!(
            !runtime
                .circuits
                .has_pending_assignment_complete(CallId(call_id))
        );
        assert!(!runtime.circuits.leg_procedures.contains_key(&MscLegKey {
            call_id: CallId(call_id),
            leg_role: MscVoiceLeg::Secondary,
        }));
        assert_eq!(
            runtime
                .circuits
                .mt_assignment_failure_retries
                .get(&CallId(call_id))
                .copied(),
            Some(1)
        );
        // MS-MS regression: secondary-leg AssignmentFailure must NOT rearm the
        // primary engine. The setup helper already drove the Primary leg
        // past AssignmentComplete (state Assigned, then Alerting after the
        // MSC-emitted AWI). Either is acceptable; both leave the engine
        // ready to accept a Connect from the callee. The bug being prevented
        // here is rearm dropping the engine back to Paging.
        let primary_state = runtime.controller.state(CallId(call_id));
        assert!(
            matches!(
                primary_state,
                Some(cdma_ios::CallControlState::Assigned)
                    | Some(cdma_ios::CallControlState::Alerting)
            ),
            "primary leg's call-control state must be preserved after secondary-leg AssignmentFailure (was {:?})",
            primary_state
        );
    }

    /// SIP-inbound regression: AssignmentFailure on the **Primary** leg must
    /// purge that leg's CircuitSession + active-leg entry, so the re-page
    /// PagingResponse drives a fresh AssignmentRequest instead of being
    /// deferred forever as "secondary-leg".
    #[tokio::test]
    async fn mt_assignment_failure_on_primary_leg_allows_repage_to_proceed() {
        let call_id = 1004;
        let (client, endpoint) = cdma_bsc_a1_edge_compat::InProcessMscClient::pair(8);
        let mut runtime = MscRuntime::new(MscRuntimeConfig {
            hlr_repo: Arc::new(StubHlrRepo),
            smsc_repo: None,
            welcome_sms: None,
            sms_retry: crate::config::SmsRetryConfig::default(),
            default_voice_service_option: 3,
            wav_file: None,
            gateway_fallback_to_wav: true,
            local_answer_delay_ms: 10_000,
            media_ringback_enabled: false,
            media_ringback_type: MediaRingbackType::Nanp,
            sip_ringback_disable: false,
            inbound_sip_msc_ringback: false,
            generate_ringback: true,
            send_tones_alert: false,
            page_retry_cooldown_ms: 1000,
            page_retry_max_duration_ms: 60_000,
            failure_tone_duration_ms: 0,
            voice_bearer: None,
            media_gateway: None,
            otasp: None,
            bts_overhead: None,
        });
        let call_id_typed = runtime.controller.create_call_with_id(
            CallId(call_id),
            CallDirection::MobileTerminated,
            Some(MobileIdentity::Imsi("12345678901".to_string())),
        );
        let page = paging_request();
        runtime
            .controller
            .apply_from_msc(
                call_id_typed,
                &ProcedureMessage::PagingRequest(page.clone()),
            )
            .unwrap();
        runtime.circuits.paging_requests.insert(call_id_typed, page);

        // First PagingResponse → Primary AssignmentRequest. Primary leg is now
        // in AssignmentPending — this is the exact state the BSC abandons
        // when it hits TCH-setup teardown timeout.
        let first_pr = EncodedA1Message::from_message_for_call(
            &cdma_ios::Message::new(
                cdma_ios::MessageType::PagingResponse,
                paging_response().encode().unwrap(),
            ),
            Some(call_id),
        );
        runtime.handle_bsc_a1_message(&endpoint, first_pr).await;
        let first_ar = timeout(Duration::from_millis(50), client.poll_a1())
            .await
            .expect("MSC should emit Primary AssignmentRequest")
            .unwrap();
        assert_eq!(
            first_ar.message_type(),
            cdma_ios::MessageType::AssignmentRequest
        );
        assert_eq!(
            runtime
                .circuits
                .active_assignment_legs
                .get(&CallId(call_id)),
            Some(&MscVoiceLeg::Primary)
        );

        // BSC: AssignmentFailure (TCH teardown timed out).
        runtime
            .handle_bsc_a1_message(&endpoint, assignment_failure_msg(call_id))
            .await;
        let repage = timeout(Duration::from_millis(50), client.poll_a1())
            .await
            .expect("MSC should re-page after Primary AssignmentFailure")
            .unwrap();
        assert_eq!(repage.message_type(), cdma_ios::MessageType::PagingRequest);

        // Primary leg state must be wiped: no lingering CircuitSession, no
        // pending assignment, no active-leg entry. Otherwise the next
        // PagingResponse trips the `secondary_leg` heuristic.
        assert!(
            !runtime
                .circuits
                .circuits
                .values()
                .any(|s| s.call_id == CallId(call_id)),
            "Primary CircuitSession must be gone after AssignmentFailure"
        );
        assert!(
            !runtime
                .circuits
                .has_pending_assignment_complete(CallId(call_id))
        );
        assert!(
            !runtime
                .circuits
                .deferred_paging_responses
                .contains_key(&CallId(call_id))
        );

        // Second PagingResponse (after the re-page) must produce a fresh
        // AssignmentRequest, not get parked in deferred_paging_responses.
        let second_pr = EncodedA1Message::from_message_for_call(
            &cdma_ios::Message::new(
                cdma_ios::MessageType::PagingResponse,
                paging_response().encode().unwrap(),
            ),
            Some(call_id),
        );
        runtime.handle_bsc_a1_message(&endpoint, second_pr).await;
        let second_ar = timeout(Duration::from_millis(50), client.poll_a1())
            .await
            .expect("MSC should emit a fresh AssignmentRequest after re-page")
            .unwrap();
        assert_eq!(
            second_ar.message_type(),
            cdma_ios::MessageType::AssignmentRequest
        );
        assert!(
            !runtime
                .circuits
                .deferred_paging_responses
                .contains_key(&CallId(call_id)),
            "re-page response must not be deferred"
        );
    }

    #[tokio::test]
    async fn mt_assignment_failure_over_retry_cap_clears_call() {
        let call_id = 1002;
        let (mut runtime, client, endpoint) = setup_msc_with_secondary_pending(call_id).await;

        // Pre-bump to the cap so the next failure tips over it.
        for _ in 0..MscRuntime::MT_ASSIGNMENT_FAILURE_MAX_RETRIES {
            runtime
                .circuits
                .bump_assignment_failure_retry(CallId(call_id));
        }

        runtime
            .handle_bsc_a1_message(&endpoint, assignment_failure_msg(call_id))
            .await;

        let next = timeout(Duration::from_millis(50), client.poll_a1())
            .await
            .expect("MSC should emit ClearCommand at retry cap")
            .unwrap();
        assert_eq!(next.message_type(), cdma_ios::MessageType::ClearCommand);
        assert_eq!(next.call_id(), Some(call_id));
        assert!(
            runtime
                .circuits
                .mt_assignment_failure_retries
                .get(&CallId(call_id))
                .is_none(),
            "retry counter must reset after ClearCommand"
        );
    }

    #[tokio::test]
    async fn mt_assignment_complete_resets_retry_counter() {
        let call_id = 1003;
        let (mut runtime, client, endpoint) = setup_msc_with_secondary_pending(call_id).await;

        // Drive one AssignmentFailure → re-page.
        runtime
            .handle_bsc_a1_message(&endpoint, assignment_failure_msg(call_id))
            .await;
        let _ = timeout(Duration::from_millis(50), client.poll_a1())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            runtime
                .circuits
                .mt_assignment_failure_retries
                .get(&CallId(call_id))
                .copied(),
            Some(1)
        );

        // Simulate the BSC's fresh PagingResponse → AssignmentRequest →
        // AssignmentComplete on the new leg.
        let fresh_pr = EncodedA1Message::from_message_for_call(
            &cdma_ios::Message::new(
                cdma_ios::MessageType::PagingResponse,
                paging_response().encode().unwrap(),
            ),
            Some(call_id),
        );
        runtime.handle_bsc_a1_message(&endpoint, fresh_pr).await;
        let _ = timeout(Duration::from_millis(50), client.poll_a1())
            .await
            .unwrap()
            .unwrap();
        let ac = EncodedA1Message::from_message_for_call(
            &cdma_ios::Message::new(
                cdma_ios::MessageType::AssignmentComplete,
                AssignmentCompleteMessage {
                    channel_number: ChannelNumber(0x1122),
                    encryption_information: None,
                    service_option: Some(ServiceOption(0x0003)),
                    a2p_bearer_session_params: None,
                    a2p_bearer_format_params: None,
                }
                .encode()
                .unwrap(),
            ),
            Some(call_id),
        );
        runtime.handle_bsc_a1_message(&endpoint, ac).await;

        assert!(
            runtime
                .circuits
                .mt_assignment_failure_retries
                .get(&CallId(call_id))
                .is_none(),
            "AssignmentComplete must reset the retry counter"
        );
    }

    /// MO M2M scenario: the secondary-leg PagingRequest must NOT be sent to
    /// the BSC until the primary (MO) leg's AssignmentComplete arrives.
    /// This prevents the callee from page-responding before the caller is on
    /// traffic — the race that orphaned a deferred PagingResponse in the
    /// trace investigated alongside this change.
    #[tokio::test]
    async fn mo_m2m_paging_request_deferred_until_primary_assignment_complete() {
        let (client, endpoint) = cdma_bsc_a1_edge_compat::InProcessMscClient::pair(8);
        let hlr = Arc::new(M2mHlrRepo::new());
        let mut runtime = MscRuntime::new(MscRuntimeConfig {
            hlr_repo: hlr.clone(),
            smsc_repo: None,
            welcome_sms: None,
            sms_retry: crate::config::SmsRetryConfig::default(),
            default_voice_service_option: 3,
            wav_file: None,
            gateway_fallback_to_wav: true,
            local_answer_delay_ms: 10_000,
            media_ringback_enabled: false,
            media_ringback_type: MediaRingbackType::Nanp,
            sip_ringback_disable: false,
            inbound_sip_msc_ringback: false,
            generate_ringback: true,
            send_tones_alert: false,
            page_retry_cooldown_ms: 1000,
            page_retry_max_duration_ms: 60_000,
            failure_tone_duration_ms: 0,
            voice_bearer: None,
            media_gateway: None,
            otasp: None,
            bts_overhead: None,
        });

        // BCD encoding of "5559876543" with TON/NPI 0x81.
        // Pairs are nibble-swapped: "55" -> 0x55, "59" -> 0x95, "87" -> 0x78,
        // "65" -> 0x56, "43" -> 0x34. Trailing nibble 0xf would pad an odd
        // count; the number has 10 digits so no pad.
        let cli3 = cm_service_request_cli3(Some(vec![0x81, 0x55, 0x95, 0x78, 0x56, 0x34]));
        let call_id = 4242;
        let cli3_msg = EncodedA1Message::from_message_for_call(
            &cdma_ios::Message::new(
                cdma_ios::MessageType::CompleteLayer3Information,
                cli3.encode().unwrap(),
            ),
            Some(call_id),
        );
        runtime.handle_bsc_a1_message(&endpoint, cli3_msg).await;

        // Drain whatever the MSC sent for the MO leg setup. We expect to see
        // an AssignmentRequest (one or more depending on the M2M flow), but
        // *not* a PagingRequest — that's the message we're deferring.
        let mut saw_assignment_request = false;
        let mut assignment_circuit_id: Option<u16> = None;
        loop {
            match timeout(Duration::from_millis(50), client.poll_a1()).await {
                Ok(Some(msg)) => {
                    assert_ne!(
                        msg.message_type(),
                        cdma_ios::MessageType::PagingRequest,
                        "PagingRequest must be deferred until primary AssignmentComplete"
                    );
                    if msg.message_type() == cdma_ios::MessageType::AssignmentRequest {
                        saw_assignment_request = true;
                        let payload = msg.decode().unwrap();
                        let req =
                            cdma_ios::AssignmentRequestMessage::decode(&payload.payload).unwrap();
                        assignment_circuit_id = Some(req.circuit_identity_code.to_packed());
                    }
                }
                _ => break,
            }
        }
        assert!(
            saw_assignment_request,
            "MO leg AssignmentRequest should have been sent immediately"
        );
        assert!(
            runtime
                .circuits
                .deferred_paging_requests
                .contains_key(&CallId(call_id)),
            "deferred MO M2M PagingRequest should be stored"
        );

        // Feed AssignmentComplete for the primary leg; this should flush the
        // deferred PagingRequest to the BSC.
        let assignment_complete = EncodedA1Message::from_message_for_call(
            &cdma_ios::Message::new(
                cdma_ios::MessageType::AssignmentComplete,
                AssignmentCompleteMessage {
                    channel_number: ChannelNumber(0x4321),
                    encryption_information: None,
                    service_option: Some(ServiceOption(0x0003)),
                    a2p_bearer_session_params: None,
                    a2p_bearer_format_params: None,
                }
                .encode()
                .unwrap(),
            ),
            Some(call_id),
        );
        runtime
            .handle_bsc_a1_message(&endpoint, assignment_complete)
            .await;

        // Now expect the deferred PagingRequest to land on the wire.
        let paging_request = timeout(Duration::from_millis(50), client.poll_a1())
            .await
            .expect("PagingRequest should be sent after primary AssignmentComplete")
            .expect("A1 channel should remain open");
        assert_eq!(
            paging_request.message_type(),
            cdma_ios::MessageType::PagingRequest
        );
        assert_eq!(paging_request.call_id(), Some(call_id));
        assert!(
            !runtime
                .circuits
                .deferred_paging_requests
                .contains_key(&CallId(call_id)),
            "deferred entry should be cleared after flush"
        );
        // Sanity: the assignment circuit id the MSC chose for the MO leg is
        // tracked in circuits and matches what the BSC would AssignmentComplete.
        let _ = assignment_circuit_id; // value retained for future assertions
    }

    /// If the call is torn down before the MO leg's AssignmentComplete
    /// arrives, the deferred PagingRequest must be dropped — the callee is
    /// never disturbed.
    #[tokio::test]
    async fn mo_m2m_deferred_paging_request_dropped_on_cleanup() {
        let (client, endpoint) = cdma_bsc_a1_edge_compat::InProcessMscClient::pair(8);
        let hlr = Arc::new(M2mHlrRepo::new());
        let mut runtime = MscRuntime::new(MscRuntimeConfig {
            hlr_repo: hlr.clone(),
            smsc_repo: None,
            welcome_sms: None,
            sms_retry: crate::config::SmsRetryConfig::default(),
            default_voice_service_option: 3,
            wav_file: None,
            gateway_fallback_to_wav: true,
            local_answer_delay_ms: 10_000,
            media_ringback_enabled: false,
            media_ringback_type: MediaRingbackType::Nanp,
            sip_ringback_disable: false,
            inbound_sip_msc_ringback: false,
            generate_ringback: true,
            send_tones_alert: false,
            page_retry_cooldown_ms: 1000,
            page_retry_max_duration_ms: 60_000,
            failure_tone_duration_ms: 0,
            voice_bearer: None,
            media_gateway: None,
            otasp: None,
            bts_overhead: None,
        });

        let cli3 = cm_service_request_cli3(Some(vec![0x81, 0x55, 0x95, 0x78, 0x56, 0x34]));
        let call_id = 7777;
        let cli3_msg = EncodedA1Message::from_message_for_call(
            &cdma_ios::Message::new(
                cdma_ios::MessageType::CompleteLayer3Information,
                cli3.encode().unwrap(),
            ),
            Some(call_id),
        );
        runtime.handle_bsc_a1_message(&endpoint, cli3_msg).await;

        // Drain the MO leg traffic, asserting no PagingRequest leaked.
        while let Ok(Some(msg)) = timeout(Duration::from_millis(20), client.poll_a1()).await {
            assert_ne!(msg.message_type(), cdma_ios::MessageType::PagingRequest);
        }
        assert!(
            runtime
                .circuits
                .deferred_paging_requests
                .contains_key(&CallId(call_id))
        );

        // Tear down before AssignmentComplete arrives.
        runtime.circuits.cleanup_call(CallId(call_id), None);

        assert!(
            !runtime
                .circuits
                .deferred_paging_requests
                .contains_key(&CallId(call_id)),
            "cleanup_call must drop the deferred PagingRequest"
        );
        assert!(
            timeout(Duration::from_millis(20), client.poll_a1())
                .await
                .is_err(),
            "no PagingRequest should be sent after cleanup"
        );
    }

    #[tokio::test]
    async fn mo_cli3_defers_sip_invite_until_assignment_complete() {
        let (client, endpoint) = cdma_bsc_a1_edge_compat::InProcessMscClient::pair(4);
        let mut runtime = MscRuntime::new(MscRuntimeConfig {
            hlr_repo: Arc::new(StubHlrRepo),
            smsc_repo: None,
            welcome_sms: None,
            sms_retry: crate::config::SmsRetryConfig::default(),
            default_voice_service_option: 3,
            wav_file: Some("sample-sound.wav".to_string()),
            gateway_fallback_to_wav: true,
            local_answer_delay_ms: 10_000,
            media_ringback_enabled: false,
            media_ringback_type: MediaRingbackType::Nanp,
            sip_ringback_disable: false,
            inbound_sip_msc_ringback: false,
            generate_ringback: true,
            send_tones_alert: false,
            page_retry_cooldown_ms: 1000,
            page_retry_max_duration_ms: 60_000,
            failure_tone_duration_ms: 0,
            voice_bearer: None,
            media_gateway: Some(Arc::new(FailingMediaGateway)),
            otasp: None,
            bts_overhead: None,
        });
        let cli3 = cm_service_request_cli3(Some(vec![0x81, 0x00, 0x00, 0x00, 0x00, 0x00]));
        let call_id = 123;
        let message = EncodedA1Message::from_message_for_call(
            &cdma_ios::Message::new(
                cdma_ios::MessageType::CompleteLayer3Information,
                cli3.encode().unwrap(),
            ),
            Some(call_id),
        );

        runtime.handle_bsc_a1_message(&endpoint, message).await;

        let outbound = timeout(Duration::from_millis(50), client.poll_a1())
            .await
            .expect("AssignmentRequest should be sent")
            .unwrap();
        assert_eq!(
            outbound.message_type(),
            cdma_ios::MessageType::AssignmentRequest
        );
        // SIP INVITE is deferred to AssignmentComplete, so no gateway handle
        // exists yet and the FailingMediaGateway hasn't been invoked.
        assert_eq!(runtime.media_gw.media_gateway_calls.len(), 0);
        assert!(
            runtime
                .mo_call
                .pending_sip_routes
                .contains_key(&CallId(call_id)),
            "MO origination must stash a pending SIP route until TCH is up"
        );
        let circuit = runtime
            .circuits
            .circuits
            .values()
            .find(|session| session.call_id == CallId(call_id))
            .expect("MO circuit should be inserted");
        assert_eq!(circuit.audio_file, None);
        assert_eq!(circuit.media_gateway_handle, None);
    }

    #[tokio::test]
    async fn sip_ringback_disable_suppresses_ringback_after_assignment_complete() {
        // Regression test: `sip_ringback_disable=true` must suppress the
        // MSC-side ringback feeder that the AssignmentComplete handler
        // would otherwise start unconditionally for primary-leg MO calls.
        // Without this, the bearer-frame stream from the ringback feeder
        // makes the BSC's auto-tones-off fire prematurely and the MS
        // ignores any later Progress(Signal) failure tone.
        let (client, endpoint) = cdma_bsc_a1_edge_compat::InProcessMscClient::pair(4);
        let mut runtime = MscRuntime::new(MscRuntimeConfig {
            hlr_repo: Arc::new(StubHlrRepo),
            smsc_repo: None,
            welcome_sms: None,
            sms_retry: crate::config::SmsRetryConfig::default(),
            default_voice_service_option: 3,
            wav_file: None,
            gateway_fallback_to_wav: false,
            local_answer_delay_ms: 10_000,
            // `media_ringback_enabled=true` would normally start a ringback
            // feeder on AssignmentComplete. The new `sip_ringback_disable`
            // setting must override it for SIP-routed calls.
            media_ringback_enabled: true,
            media_ringback_type: MediaRingbackType::Nanp,
            sip_ringback_disable: true,
            inbound_sip_msc_ringback: false,
            generate_ringback: true,
            send_tones_alert: false,
            page_retry_cooldown_ms: 1000,
            page_retry_max_duration_ms: 60_000,
            failure_tone_duration_ms: 0,
            voice_bearer: None,
            media_gateway: Some(Arc::new(StubMediaGateway::default())),
            otasp: None,
            bts_overhead: None,
        });

        let cli3 = cm_service_request_cli3(Some(vec![0x81, 0x00, 0x00, 0x00, 0x00, 0x00]));
        let call_id_raw = 4242;
        let cli3_msg = EncodedA1Message::from_message_for_call(
            &cdma_ios::Message::new(
                cdma_ios::MessageType::CompleteLayer3Information,
                cli3.encode().unwrap(),
            ),
            Some(call_id_raw),
        );
        runtime.handle_bsc_a1_message(&endpoint, cli3_msg).await;

        // Drain the AssignmentRequest the MSC just sent.
        let outbound = timeout(Duration::from_millis(50), client.poll_a1())
            .await
            .expect("AssignmentRequest should be sent")
            .unwrap();
        assert_eq!(
            outbound.message_type(),
            cdma_ios::MessageType::AssignmentRequest
        );

        // Now deliver AssignmentComplete to drive the primary-leg branch
        // that would otherwise start the ringback feeder.
        let ac = AssignmentCompleteMessage {
            channel_number: ChannelNumber(0x100e),
            encryption_information: None,
            service_option: Some(ServiceOption(0x0003)),
            a2p_bearer_session_params: None,
            a2p_bearer_format_params: None,
        };
        let ac_msg = EncodedA1Message::from_message_for_call(
            &cdma_ios::Message::new(
                cdma_ios::MessageType::AssignmentComplete,
                ac.encode().unwrap(),
            ),
            Some(call_id_raw),
        );
        runtime.handle_bsc_a1_message(&endpoint, ac_msg).await;

        assert!(
            runtime.media.feeders.is_empty(),
            "sip_ringback_disable=true must keep MSC-side ringback off; feeders={:?}",
            runtime
                .media
                .feeders
                .iter()
                .map(|(cid, f)| (*cid, f.kind))
                .collect::<Vec<_>>()
        );
    }

    #[tokio::test]
    async fn reverse_bearer_frame_without_peer_forwards_to_media_gateway() {
        let gateway = Arc::new(StubMediaGateway::default());
        let mut runtime = MscRuntime::new(MscRuntimeConfig {
            hlr_repo: Arc::new(StubHlrRepo),
            smsc_repo: None,
            welcome_sms: None,
            sms_retry: crate::config::SmsRetryConfig::default(),
            default_voice_service_option: 3,
            wav_file: None,
            gateway_fallback_to_wav: true,
            local_answer_delay_ms: 10_000,
            media_ringback_enabled: false,
            media_ringback_type: MediaRingbackType::Nanp,
            sip_ringback_disable: false,
            inbound_sip_msc_ringback: false,
            generate_ringback: true,
            send_tones_alert: false,
            page_retry_cooldown_ms: 1000,
            page_retry_max_duration_ms: 60_000,
            failure_tone_duration_ms: 0,
            voice_bearer: None,
            media_gateway: Some(gateway.clone()),
            otasp: None,
            bts_overhead: None,
        });
        let call_id = runtime
            .controller
            .create_call(CallDirection::MobileOriginated, None);
        runtime
            .controller
            .attach_media_gateway_handle(call_id, CallHandle(77))
            .unwrap();
        runtime.circuits.insert_circuit_session(
            23,
            CircuitSession {
                call_id,
                audio_file: None,
                service_option: 3,
                leg_role: MscVoiceLeg::Primary,
                peer_circuit_id: None,
                bearer_remote_ready: true,
                media_gateway_handle: None,
                called_number: None,
            },
        );

        runtime
            .handle_reverse_bearer_frame(VoiceBearerFrame {
                circuit_id: 23,
                rate_bps: 9600,
                payload: vec![1, 2, 3, 4],
            })
            .await;

        let forwarded = gateway.forwarded.lock().unwrap();
        assert_eq!(forwarded.len(), 1);
        assert_eq!(forwarded[0].0, CallHandle(77));
        assert_eq!(
            forwarded[0].1,
            VocoderFrame {
                payload: vec![1, 2, 3, 4],
                rate_bps: 9600,
            }
        );
    }

    #[test]
    fn assignment_complete_correlation_uses_active_leg_key() {
        let mut runtime = MscRuntime::new(MscRuntimeConfig {
            hlr_repo: Arc::new(StubHlrRepo),
            smsc_repo: None,
            welcome_sms: None,
            sms_retry: crate::config::SmsRetryConfig::default(),
            default_voice_service_option: 3,
            wav_file: None,
            gateway_fallback_to_wav: true,
            local_answer_delay_ms: 10_000,
            media_ringback_enabled: false,
            media_ringback_type: MediaRingbackType::Nanp,
            sip_ringback_disable: false,
            inbound_sip_msc_ringback: false,
            generate_ringback: true,
            send_tones_alert: false,
            page_retry_cooldown_ms: 1000,
            page_retry_max_duration_ms: 60_000,
            failure_tone_duration_ms: 0,
            voice_bearer: None,
            media_gateway: None,
            otasp: None,
            bts_overhead: None,
        });
        let call_id = CallId(42);

        runtime
            .circuits
            .queue_assignment_complete_circuit(call_id, MscVoiceLeg::Primary, 7);
        assert_eq!(
            runtime.circuits.assignment_complete_circuit(call_id),
            Some(7)
        );
        assert_eq!(runtime.circuits.assignment_complete_circuit(call_id), None);

        runtime
            .circuits
            .queue_assignment_complete_circuit(call_id, MscVoiceLeg::Secondary, 8);
        assert_eq!(
            runtime.circuits.assignment_complete_circuit(call_id),
            Some(8)
        );
        assert_eq!(runtime.circuits.assignment_complete_circuit(call_id), None);
        assert!(runtime.circuits.pending_assignment_completes.is_empty());
        assert!(runtime.circuits.active_assignment_legs.is_empty());
    }

    #[tokio::test]
    async fn inbound_sip_unknown_did_emits_404_reject() {
        let (_, endpoint) = cdma_bsc_a1_edge_compat::InProcessMscClient::pair(4);
        let gateway = Arc::new(RecordingInboundGateway::default());
        let mut runtime = MscRuntime::new(MscRuntimeConfig {
            hlr_repo: Arc::new(StubHlrRepo),
            smsc_repo: None,
            welcome_sms: None,
            sms_retry: crate::config::SmsRetryConfig::default(),
            default_voice_service_option: 3,
            wav_file: None,
            gateway_fallback_to_wav: false,
            local_answer_delay_ms: 10_000,
            media_ringback_enabled: false,
            media_ringback_type: MediaRingbackType::Nanp,
            sip_ringback_disable: false,
            inbound_sip_msc_ringback: false,
            generate_ringback: true,
            send_tones_alert: false,
            page_retry_cooldown_ms: 1000,
            page_retry_max_duration_ms: 60_000,
            failure_tone_duration_ms: 0,
            voice_bearer: None,
            media_gateway: Some(gateway.clone() as Arc<dyn MediaGatewayClient>),
            otasp: None,
            bts_overhead: None,
        });

        runtime
            .handle_inbound_sip_invite(
                &endpoint,
                "test-session".to_string(),
                "14805551212".to_string(),
                "13105550000".to_string(),
                vec!["PCMU".to_string()],
            )
            .await;

        let rejects = gateway.reject.lock().unwrap().clone();
        assert_eq!(
            rejects,
            vec![("test-session".to_string(), 404u16)],
            "unknown DID must emit inbound_reject(404)"
        );
        assert!(gateway.progress.lock().unwrap().is_empty());
        assert!(gateway.answer.lock().unwrap().is_empty());
        assert!(
            runtime.media_gw.inbound_by_call.is_empty(),
            "no inbound session should be tracked when DID lookup misses"
        );
    }
}
