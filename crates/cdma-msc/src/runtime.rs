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

use cdma_hlr::model::{RegistrationBinding, SubscriberIdentity};
use cdma_ios::{A1TransportError, EncodedA1Message, VoiceBearerFrame, VoiceBearerManager};

use crate::call_control::{CallDirection, CallId, MscCallController};
use crate::circuit::{CircuitService, CircuitSession, DeferredPagingResponse, MscVoiceLeg};
use crate::config::MediaRingbackType;
use crate::management::{
    InitiateCallAccepted, InitiateCallRequest, ManagementError, MtCallPlan, PendingControlRequest,
};
use crate::media::MediaService;
use crate::media_gateway::{CreateCallRequest, MediaGatewayClient, ReleaseCause, VocoderFrame};
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
    /// Voice bearer manager for MSC<->BSC per-circuit RTP voice sessions.
    pub voice_bearer: Option<Arc<VoiceBearerManager>>,
    /// Optional MSC-owned media gateway client for external voice legs.
    pub media_gateway: Option<Arc<dyn MediaGatewayClient>>,
    /// Welcome SMS sent to mobiles on first registration.
    pub welcome_sms: Option<crate::config::WelcomeSmsConfig>,
    /// MT SMS retry sweep configuration.
    pub sms_retry: crate::config::SmsRetryConfig,
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
            voice_bearer: Some(Arc::new(VoiceBearerManager::new(
                config.voice.voice_bearer_bind_ip,
            ))),
            media_gateway,
            welcome_sms: Some(config.welcome_sms.clone()),
            sms_retry: config.sms_retry.clone(),
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
    pub(crate) smsc: Option<crate::sms::MscSmsCoordinator>,
}

impl MscRuntime {
    /// Creates a new MSC runtime.
    pub fn new(config: MscRuntimeConfig) -> Self {
        let smsc = config.smsc_repo.as_ref().map(|smsc_repo| {
            crate::sms::MscSmsCoordinator::new(Arc::clone(smsc_repo), Arc::clone(&config.hlr_repo))
        });
        Self {
            controller: MscCallController::new(),
            config,
            circuits: CircuitService::new(),
            media: MediaService::new(),
            media_gw: MediaGatewayService::new(),
            mt_call: MtCallService::new(),
            mo_call: MoCallService::new(),
            smsc,
        }
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
        loop {
            let delayed_wav_sleep = async {
                let next_deadline = self.media.next_delayed_wav_deadline();
                match next_deadline {
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
                    if let Some(frame) = result {
                        self.handle_reverse_bearer_frame(frame).await;
                    }
                }
                event = async {
                    match self.config.media_gateway.as_ref() {
                        Some(gateway) => gateway.recv_event().await,
                        None => std::future::pending().await,
                    }
                } => {
                    if let Some(event) = event {
                        self.media_gw.handle_media_gateway_event(
                            a1,
                            event,
                            &mut self.controller,
                            &mut self.circuits,
                            &mut self.media,
                            self.config.voice_bearer.as_ref(),
                            self.config.media_gateway.as_ref(),
                            self.config.media_ringback_enabled,
                            self.config.media_ringback_type,
                        ).await;
                    }
                }
                _ = delayed_wav_sleep => {
                    self.media.handle_due_delayed_wav_starts(
                        &self.circuits,
                        self.config.voice_bearer.as_ref(),
                    );
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
            }
        }
    }

    /// Runs the MSC event loop with an embedded gRPC management server.
    ///
    /// The gRPC server accepts management requests (initiate_call, list_calls)
    /// and feeds them into the runtime's event loop via an internal channel.
    pub async fn run_with_grpc(&mut self, mgmt_addr: std::net::SocketAddr, a1: &dyn MscA1Endpoint) {
        let (mgmt_tx, mgmt_rx) = tokio::sync::mpsc::channel::<PendingControlRequest>(16);
        let service = crate::grpc::MscManagementServiceImpl::from_channel(mgmt_tx);
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
        let Some(subscriber) = self
            .config
            .hlr_repo
            .get_subscriber_by_id(request.subscriber_id)
            .await
            .map_err(ManagementError::Rejected)?
        else {
            return Err(ManagementError::UnknownSubscriber(request.subscriber_id));
        };
        if !matches!(subscriber.status, cdma_hlr::model::SubscriberStatus::Active) {
            return Err(ManagementError::Rejected(format!(
                "subscriber {} is not active",
                subscriber.subscriber_id
            )));
        }

        let Some(binding) = self
            .config
            .hlr_repo
            .get_registration_binding(subscriber.subscriber_id)
            .await
            .map_err(ManagementError::Rejected)?
        else {
            return Err(ManagementError::Rejected(format!(
                "subscriber {} is not currently registered",
                subscriber.subscriber_id
            )));
        };
        if !matches!(
            binding.state,
            cdma_hlr::model::RegistrationState::Registered
                | cdma_hlr::model::RegistrationState::PageResponseReceived
        ) {
            return Err(ManagementError::Rejected(format!(
                "subscriber {} is not pageable in state {}",
                subscriber.subscriber_id,
                binding.state.as_str()
            )));
        }

        let identities = self
            .config
            .hlr_repo
            .get_identities_for_subscriber(subscriber.subscriber_id)
            .await
            .map_err(ManagementError::Rejected)?;
        let imsi = select_pageable_imsi(&identities, &binding).ok_or_else(|| {
            ManagementError::Rejected(format!(
                "subscriber {} has no IMSI for A1 paging",
                subscriber.subscriber_id
            ))
        })?;

        let call_id = self.controller.create_call(
            CallDirection::MobileTerminated,
            Some(cdma_ios::MobileIdentity::Imsi(imsi.to_string())),
        );
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
        self.controller
            .apply_from_msc(
                call_id,
                &cdma_ios::ProcedureMessage::PagingRequest(paging_request.clone()),
            )
            .map_err(|e| ManagementError::Rejected(format!("msc paging state error: {e}")))?;
        self.mt_call.mt_plans.insert(
            tag.0,
            MtCallPlan {
                subscriber_id: subscriber.subscriber_id,
                imsi: imsi.to_string(),
                audio_file: request.audio_file,
                caller_number: request.caller_number,
                service_option: self.config.default_voice_service_option,
            },
        );
        self.circuits
            .paging_requests
            .insert(call_id, paging_request.clone());
        info!("MSC: A1 tx PagingRequest call_id={}", call_id.0);
        a1.send_to_bsc(EncodedA1Message::from_message_for_call(
            &cdma_ios::Message::new(
                cdma_ios::MessageType::PagingRequest,
                paging_request.encode().map_err(|e| {
                    ManagementError::Rejected(format!("encode A1 Paging Request: {e}"))
                })?,
            ),
            Some(call_id.0),
        ))
        .await
        .map_err(|_| ManagementError::Unavailable("A1 edge to BSC is closed"))?;

        Ok(InitiateCallAccepted { call_id })
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
                    );
                } else if completed_leg == Some(MscVoiceLeg::Primary) {
                    self.media.start_ringback_for_call(
                        call_id,
                        &self.controller,
                        &self.circuits,
                        self.config.voice_bearer.as_ref(),
                        self.config.media_ringback_enabled,
                        self.config.media_ringback_type,
                    );
                }
                self.flush_deferred_paging_response(a1, call_id).await;
                if completed_leg == Some(MscVoiceLeg::Primary)
                    && self.controller.snapshot(call_id).is_some_and(|snapshot| {
                        snapshot.direction == CallDirection::MobileOriginated
                    })
                {
                    self.flush_deferred_paging_request(a1, call_id).await;
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
                if let Err(error) = self
                    .controller
                    .apply_from_bsc(call_id, &cdma_ios::ProcedureMessage::Connect(connect))
                {
                    warn!("MSC: failed to apply A1 Connect: {}", error);
                }
                self.media.stop_ringback_for_call(call_id, &self.circuits);
                self.media.start_media_for_call(
                    call_id,
                    &self.circuits,
                    self.config.voice_bearer.as_ref(),
                );
            }
            cdma_ios::MessageType::ClearRequest => {
                let clear_request = match cdma_ios::ClearRequestMessage::decode(&decoded.payload) {
                    Ok(clear_request) => clear_request,
                    Err(error) => {
                        warn!("MSC: failed to decode A1 Clear Request: {}", error);
                        return;
                    }
                };
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
                    warn!("MSC: failed to apply A1 Clear Complete: {}", error);
                    return;
                }
                if self.controller.remove_call(call_id).is_none() {
                    warn!("MSC: no call found to remove after Clear Complete");
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
                let calling_number = self
                    .mo_call
                    .resolve_mo_calling_number(
                        cm_service_request.as_ref(),
                        self.config.hlr_repo.as_ref(),
                    )
                    .await;

                let mobile_identity = cm_service_request
                    .as_ref()
                    .map(|req| req.mobile_identity_imsi.clone());
                let call_id = self.controller.create_call_with_id(
                    call_id,
                    CallDirection::MobileOriginated,
                    mobile_identity,
                );
                if let Some(number) = calling_number.clone() {
                    self.mo_call.mo_calling_numbers.insert(call_id, number);
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

                let mut audio_file = None;
                let media_gateway_handle = if !routes_to_subscriber {
                    if let (Some(gateway), Some(called_number)) =
                        (self.config.media_gateway.as_ref(), called_number.as_deref())
                    {
                        match gateway
                            .create_call(CreateCallRequest {
                                call_id: call_id.0,
                                calling_party: calling_number.clone(),
                                called_party: Some(called_number.to_string()),
                                service_option,
                            })
                            .await
                        {
                            Ok(handle) => {
                                self.media_gw.media_gateway_calls.insert(handle, call_id);
                                if let Err(error) =
                                    self.controller.attach_media_gateway_handle(call_id, handle)
                                {
                                    warn!(
                                        "MSC: failed to attach media gateway handle to call_id={}: {}",
                                        call_id.0, error
                                    );
                                }
                                Some(handle)
                            }
                            Err(error) => {
                                warn!(
                                    "MSC: failed to create media gateway call for MO call_id={}: {}",
                                    call_id.0, error
                                );
                                if self.config.gateway_fallback_to_wav {
                                    audio_file = self.config.wav_file.clone();
                                    if audio_file.is_some() {
                                        info!(
                                            "MSC: falling back to WAV playback for MO call_id={} after media gateway setup failure",
                                            call_id.0
                                        );
                                    } else {
                                        warn!(
                                            "MSC: media gateway fallback enabled for MO call_id={} but no WAV file is configured",
                                            call_id.0
                                        );
                                    }
                                } else {
                                    send_gateway_clear_command(
                                        a1,
                                        call_id,
                                        &mut self.controller,
                                        gateway_clear_cause(ReleaseCause::SetupFailed, None),
                                    )
                                    .await;
                                    return;
                                }
                                None
                            }
                        }
                    } else {
                        audio_file = self.config.wav_file.clone();
                        None
                    }
                } else {
                    None
                };

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

                let a2p_bearer_format_params =
                    a2p_bearer_session_params
                        .as_ref()
                        .map(|_| cdma_ios::A2pBearerFormatParams {
                            formats: vec![cdma_ios::BearerFormatEntry {
                                bearer_format_tag_type: 1,
                                bearer_format_id: 0,
                                rtp_payload_type: cdma_ios::voice_bearer::EVRC_RTP_PAYLOAD_TYPE,
                                bearer_addr: None,
                            }],
                        });
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
                    calling_party_ascii_number: None,
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
            )
            .await;
    }

    const MT_ASSIGNMENT_FAILURE_MAX_RETRIES: u8 = 3;

    async fn handle_assignment_failure(&mut self, a1: &dyn MscA1Endpoint, call_id: CallId) {
        let abandoned = self
            .circuits
            .cancel_secondary_leg(call_id, self.config.voice_bearer.as_ref());
        let attempts = self.circuits.bump_assignment_failure_retry(call_id);
        info!(
            "MSC: A1 rx AssignmentFailure call_id={} (attempt {}/{}); abandoned circuit_id={:?}",
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
        self.reissue_paging_request(a1, call_id).await;
    }

    async fn reissue_paging_request(&mut self, a1: &dyn MscA1Endpoint, call_id: CallId) {
        let Some(paging_request) = self.circuits.paging_requests.get(&call_id).cloned() else {
            warn!(
                "MSC: cannot re-page call_id={} — no original PagingRequest retained",
                call_id.0
            );
            return;
        };
        let payload = match paging_request.encode() {
            Ok(payload) => payload,
            Err(error) => {
                warn!(
                    "MSC: failed to encode re-page PagingRequest call_id={}: {}",
                    call_id.0, error
                );
                return;
            }
        };
        info!(
            "MSC: A1 tx PagingRequest call_id={} (re-page after AssignmentFailure)",
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
                "MSC: failed to send re-page PagingRequest call_id={}: {}",
                call_id.0, error
            );
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
        let payload = match paging_request.encode() {
            Ok(payload) => payload,
            Err(error) => {
                warn!(
                    "MSC: failed to encode deferred MO M2M Paging Request call_id={}: {}",
                    call_id.0, error
                );
                self.circuits.paging_requests.remove(&call_id);
                return;
            }
        };
        info!(
            "MSC: A1 tx PagingRequest for MO M2M call_id={} (deferred until primary AssignmentComplete)",
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
                "MSC: failed to send deferred MO M2M Paging Request call_id={}: {}",
                call_id.0, error
            );
            self.circuits.paging_requests.remove(&call_id);
        }
    }

    fn stop_media_for_call(&mut self, call_id: CallId) {
        self.mo_call.cleanup_call(call_id);
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

    /// Routes an inbound ADDS message from the BSC to the SMS coordinator.
    async fn handle_adds_message(&mut self, _a1: &dyn MscA1Endpoint, message: EncodedA1Message) {
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
        let decoded = match message.decode() {
            Ok(d) => d,
            Err(e) => {
                warn!("MSC: failed to decode ADDS message: {e}");
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
                    Ok(msg) => smsc.handle_adds_deliver_ack(&msg).await,
                    Err(e) => warn!("MSC: failed to decode ADDS Deliver Ack: {e}"),
                }
            }
            cdma_ios::MessageType::AddsTransfer => {
                match cdma_ios::AddsTransferMessage::decode(&decoded.payload) {
                    Ok(msg) => smsc.handle_adds_transfer(&msg).await,
                    Err(e) => warn!("MSC: failed to decode ADDS Transfer: {e}"),
                }
            }
            cdma_ios::MessageType::AddsDeliver => {
                // BS→MSC direction: MO SMS on traffic channel. ADDS Deliver carries no
                // Mobile Identity; resolve it from the active call session via call_id.
                match cdma_ios::AddsDeliverMessage::decode(&decoded.payload) {
                    Ok(msg) => {
                        let call_id_raw = message.call_id().unwrap_or(0);
                        let mobile_identity = self
                            .controller
                            .snapshot(CallId(call_id_raw))
                            .and_then(|snap| snap.mobile_identity.clone())
                            .unwrap_or_else(|| {
                                cdma_ios::MobileIdentity::Imsi(format!("UNKNOWN-{call_id_raw}"))
                            });
                        smsc.handle_adds_deliver_mo(&msg, &mobile_identity).await;
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
        let (esn, imsi) = match &lur.mobile_identity_imsi {
            cdma_ios::MobileIdentity::Imsi(s) if s != "UNKNOWN" => (None, Some(s.as_str())),
            cdma_ios::MobileIdentity::Esn(e) => (Some(*e), None),
            _ => {
                warn!(
                    "MSC: registration notification has no usable identity — welcome SMS skipped"
                );
                return;
            }
        };
        let upsert = match self
            .config
            .hlr_repo
            .upsert_mobile_seen(esn, imsi, None)
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
        let subscriber = match self.config.hlr_repo.resolve_by_identity(esn, imsi).await {
            Ok(Some(s)) => Some(s),
            Ok(None) => None,
            Err(e) => {
                warn!("MSC: registration HLR lookup failed: {e}");
                return;
            }
        };
        let destination = if let Some(ref subscriber) = subscriber {
            info!(
                "MSC: sending welcome SMS to {} on registration",
                subscriber.phone_number
            );
            crate::sms::SmsDestinationKey::PhoneNumber(subscriber.phone_number.clone())
        } else if let Some(imsi) = imsi {
            info!(
                "MSC: sending welcome SMS to non-subscriber by IMSI {} on registration",
                imsi
            );
            crate::sms::SmsDestinationKey::Imsi(imsi.to_string())
        } else {
            // ADDS Page requires an IMSI on the wire — ESN-only mobiles cannot
            // be welcomed until they next provide an IMSI.
            info!(
                "MSC: registration: ESN-only mobile (esn={:?}) and no HLR record — welcome SMS skipped",
                esn
            );
            return;
        };
        smsc.send_sms(
            crate::sms::SmsSendRequest {
                originating_number: welcome_cfg.originating_number.clone(),
                text: welcome_cfg.text.clone(),
                destination,
                timeout_ms: 30_000,
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
    decode_called_party_bcd_number(request.called_party_bcd_number.as_ref()).or_else(|| {
        request
            .called_party_ascii_number
            .as_ref()
            .and_then(|number| String::from_utf8(number.0.clone()).ok())
            .filter(|number| !number.is_empty())
    })
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
            service_option: Some(ServiceOption(3)),
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
        ) -> Result<cdma_hlr::model::Subscriber, String> {
            unimplemented!()
        }
        async fn get_subscriber_by_phone_number(
            &self,
            _: &str,
        ) -> Result<Option<cdma_hlr::model::Subscriber>, String> {
            Ok(None)
        }
        async fn get_subscriber_by_id(
            &self,
            _: uuid::Uuid,
        ) -> Result<Option<cdma_hlr::model::Subscriber>, String> {
            unimplemented!()
        }
        async fn update_subscriber(
            &self,
            _: uuid::Uuid,
            _: &str,
            _: &str,
            _: &str,
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
        ) -> Result<cdma_hlr::model::SubscriberIdentity, String> {
            unimplemented!()
        }
        async fn replace_primary_identity(
            &self,
            _: uuid::Uuid,
            _: Option<&str>,
            _: Option<u32>,
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
            _: Option<u32>,
            _: Option<&str>,
        ) -> Result<Option<cdma_hlr::model::Subscriber>, String> {
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
            _: Option<u32>,
            _: Option<&str>,
            _: Option<u8>,
        ) -> Result<cdma_hlr::MobileSeenUpsert, String> {
            Ok(cdma_hlr::MobileSeenUpsert {
                is_new: true,
                previous_last_seen_at: None,
            })
        }
    }

    #[async_trait::async_trait]
    impl cdma_hlr::repository::HlrRepository for M2mHlrRepo {
        async fn upsert_subscriber(
            &self,
            _: &str,
            _: &str,
            _: &str,
        ) -> Result<cdma_hlr::model::Subscriber, String> {
            unimplemented!()
        }
        async fn get_subscriber_by_phone_number(
            &self,
            phone_number: &str,
        ) -> Result<Option<cdma_hlr::model::Subscriber>, String> {
            if phone_number == self.phone_number {
                Ok(Some(cdma_hlr::model::Subscriber {
                    subscriber_id: self.subscriber_id,
                    phone_number: self.phone_number.to_string(),
                    display_name: "M2M Test".to_string(),
                    status: cdma_hlr::model::SubscriberStatus::Active,
                    created_at: chrono::Utc::now(),
                    updated_at: chrono::Utc::now(),
                }))
            } else {
                Ok(None)
            }
        }
        async fn get_subscriber_by_id(
            &self,
            _: uuid::Uuid,
        ) -> Result<Option<cdma_hlr::model::Subscriber>, String> {
            unimplemented!()
        }
        async fn update_subscriber(
            &self,
            _: uuid::Uuid,
            _: &str,
            _: &str,
            _: &str,
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
        ) -> Result<cdma_hlr::model::SubscriberIdentity, String> {
            unimplemented!()
        }
        async fn replace_primary_identity(
            &self,
            _: uuid::Uuid,
            _: Option<&str>,
            _: Option<u32>,
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
                is_primary: true,
                created_at: chrono::Utc::now(),
            }])
        }
        async fn resolve_by_identity(
            &self,
            _: Option<u32>,
            _: Option<&str>,
        ) -> Result<Option<cdma_hlr::model::Subscriber>, String> {
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
            _: Option<u32>,
            _: Option<&str>,
            _: Option<u8>,
        ) -> Result<cdma_hlr::MobileSeenUpsert, String> {
            Ok(cdma_hlr::MobileSeenUpsert {
                is_new: true,
                previous_last_seen_at: None,
            })
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
            service_option: Some(ServiceOption(3)),
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
            voice_bearer: None,
            media_gateway: None,
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
                    calling_party_ascii_number: None,
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
            voice_bearer: None,
            media_gateway: None,
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
            voice_bearer: None,
            media_gateway: None,
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
            voice_bearer: None,
            media_gateway: None,
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
            voice_bearer: None,
            media_gateway: None,
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
    async fn mo_cli3_falls_back_to_wav_when_gateway_create_fails() {
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
            voice_bearer: None,
            media_gateway: Some(Arc::new(FailingMediaGateway)),
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
            .expect("AssignmentRequest should be sent after WAV fallback")
            .unwrap();
        assert_eq!(
            outbound.message_type(),
            cdma_ios::MessageType::AssignmentRequest
        );
        assert_eq!(runtime.media_gw.media_gateway_calls.len(), 0);
        let circuit = runtime
            .circuits
            .circuits
            .values()
            .find(|session| session.call_id == CallId(call_id))
            .expect("MO circuit should be inserted");
        assert_eq!(circuit.audio_file.as_deref(), Some("sample-sound.wav"));
        assert_eq!(circuit.media_gateway_handle, None);
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
            voice_bearer: None,
            media_gateway: Some(gateway.clone()),
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
            voice_bearer: None,
            media_gateway: None,
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
}
