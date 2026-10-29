//! MSC runtime event loop for A1 signaling and management control.
//!
//! This was extracted from the BSC monolith (`cdma-bsc/src/main.rs`). The MSC
//! runtime owns the A1 call-control state machine and processes both
//! operator-initiated (management) and BSC-initiated (A1) events.
//!
//! `MscRuntime` is a thin actor shell that owns service structs and the
//! `select!` loop. Each service owns its own state; cross-service calls pass
//! `&mut` references rather than `Arc`.

mod m2m;

use std::sync::Arc;
use std::time::Duration;

use log::{debug, info, warn};

use cdma_common::consts::{
    SERVICE_OPTION_ASYNC_DATA, SERVICE_OPTION_BASIC_VOICE, SERVICE_OPTION_OTASP,
    SERVICE_OPTION_PACKET_DATA, SERVICE_OPTION_PACKET_DATA_RC3, SERVICE_OPTION_SMS,
};
use cdma_hlr::model::{RegistrationBinding, SubscriberIdentity};
use cdma_ios::{EncodedA1Message, VoiceBearerFrame, VoiceBearerManager};

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
    send_gateway_clear_command, send_progress_tones_off, stop_media_for_call,
};
use crate::mo_call::{MoCallService, select_mt_voice_service_option};
use crate::mt_call::MtCallService;

pub use crate::base_station::MscA1Endpoint;
use crate::base_station::{A1Event, BaseStationId};

#[derive(Copy, Clone, Debug)]
pub(crate) enum PagePurpose {
    Initial,
    Retry,
    RepageAfterAf { failed_leg: MscVoiceLeg },
}

fn is_sms_traffic_service_option(so: u16) -> bool {
    matches!(so, SERVICE_OPTION_SMS | 14)
}

fn is_packet_data_service_option(so: u16) -> bool {
    matches!(
        so,
        SERVICE_OPTION_PACKET_DATA | SERVICE_OPTION_ASYNC_DATA | SERVICE_OPTION_PACKET_DATA_RC3
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

fn resolve_mo_service_option(requested: u16) -> u16 {
    requested
}

fn assignment_channel_type(so: u16) -> cdma_ios::ChannelType {
    if is_packet_data_service_option(so) {
        cdma_ios::ChannelType::packet_data()
    } else {
        cdma_ios::ChannelType {
            speech_or_data_indicator: cdma_ios::ChannelType::SPEECH_OR_DATA_SPEECH,
            channel_rate_and_type: cdma_ios::ChannelType::RATE_FULL_RATE_TCH_BM,
            coding: cdma_ios::ChannelType::CODING_QCELP_13K,
        }
    }
}

impl PagePurpose {
    fn tag(self) -> &'static str {
        match self {
            Self::Initial => "",
            Self::Retry => " (retry)",
            Self::RepageAfterAf { .. } => " (re-page after AssignmentFailure)",
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
    /// The subscriber's serving base station is not attached to this MSC.
    ServingNodeUnavailable(uuid::Uuid, String),
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
            Self::ServingNodeUnavailable(id, node) => ManagementError::Rejected(format!(
                "subscriber {id} is served by base station {node}, which is not attached"
            )),
            Self::A1Closed => ManagementError::Unavailable("A1 edge to BSC is closed"),
            Self::Other(msg) => ManagementError::Rejected(msg),
        }
    }

    pub(crate) fn to_sip_status(&self) -> u16 {
        match self {
            Self::SubscriberInactive(_) => 404,
            Self::NotRegistered(_) | Self::NotPageable(_, _) | Self::NoPageableImsi(_) => 480,
            Self::ServingNodeUnavailable(_, _) | Self::A1Closed | Self::Other(_) => 503,
        }
    }
}

/// A.S0014-D Table 4.2.34-1 Reject Cause `03H` Illegal MS.
const REJECT_CAUSE_ILLEGAL_MS: u8 = 0x03;
const SIP_STATUS_FORBIDDEN: u16 = 403;

/// Configuration for the MSC runtime.
pub struct MscRuntimeConfig {
    /// HLR repository for subscriber lookups.
    pub hlr_repo: Arc<dyn cdma_hlr::repository::HlrRepository>,
    /// Optional SMSC repository — enables the MSC SMS coordinator when present.
    pub smsc_repo: Option<Arc<dyn cdma_smsc::repository::SmscRepository>>,
    /// Default service option for MT voice calls.
    pub default_voice_service_option: u16,
    /// Voice service options permitted by MSC policy.
    pub supported_voice_service_options: Vec<u16>,
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
    pub authentication: crate::authentication::AuthenticationConfig,
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
            supported_voice_service_options: config.voice.supported_service_options.clone(),
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
            authentication: config.authentication.clone(),
        }
    }
}

/// A.S0014-D Table 5.2-1 T3280 default, in seconds.
const PRIVACY_MODE_TIMEOUT: Duration = Duration::from_secs(15);

#[derive(Clone, Copy, PartialEq, Eq)]
enum AccessPurpose {
    Service,
    Provisioning,
}

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
    pub(crate) otasp_history: std::sync::Arc<crate::otasp::OtaspHistory>,
    pub(crate) otasp_event_tx:
        tokio::sync::broadcast::Sender<crate::grpc::events_proto::v1::MscNetworkEvent>,
    pub(crate) otapa_routes: std::collections::HashMap<
        crate::network_otasp::ProvisioningIdentity,
        crate::network_otasp::ProvisioningRoute,
    >,
    pub(crate) pending_otapa_calls:
        std::collections::HashMap<CallId, cdma_hlr::model::SubscriberIdentity>,
    /// C.S0016-D §3.2.1 permits a voice/data SO for user-initiated OTASP.
    pub(crate) pending_otasp_originations:
        std::collections::HashMap<CallId, PendingOtaspOrigination>,
    pub(crate) deliveries: Option<crate::sms_delivery::DeliveryLink>,
    pub(crate) pending_sms_traffic:
        std::collections::HashMap<CallId, crate::sms::SmsTrafficDelivery>,
    pub(crate) call_keys: std::collections::HashMap<CallId, cdma_hlr::auth::CallKeyMaterial>,
    privacy_deadlines: std::collections::HashMap<CallId, tokio::time::Instant>,
    pending_privacy: std::collections::HashMap<CallId, cdma_ios::EncryptionInformation>,
    privacy_authenticated: std::collections::HashSet<CallId>,
    traffic_auth_started: std::collections::HashSet<CallId>,
    pub(crate) call_auth_context:
        std::collections::HashMap<CallId, crate::authentication::CallAuthContext>,
    pub(crate) traffic_auth:
        std::collections::HashMap<CallId, crate::authentication::TrafficAuthentication>,
    /// Calls that failed the global challenge and must pass an SSD update and
    /// unique challenge on their traffic channel before they connect.
    pub(crate) pending_ssd_calls: std::collections::HashSet<CallId>,
    pub(crate) held_assignments: std::collections::HashMap<CallId, HeldAssignment>,
    /// The mobile a held call dials, paged only once the call passes.
    pub(crate) held_m2m_callees:
        std::collections::HashMap<CallId, (cdma_hlr::model::ResolvedSubscriber, Option<String>)>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) struct HeldAssignment {
    completed_circuit_id: Option<u16>,
    completed_leg: Option<MscVoiceLeg>,
}

async fn recv_delivery(
    link: Option<&mut crate::sms_delivery::DeliveryLink>,
) -> Option<cdma_smsc::proto::DeliveryCommand> {
    match link {
        Some(link) => link.commands.recv().await,
        None => std::future::pending().await,
    }
}

pub(crate) struct PendingOtaspOrigination {
    pub device: crate::otasp::HardwareIdentity,
    pub feature_code: String,
    pub mobile_identity_imsi: cdma_ios::MobileIdentity,
    pub service_option: u16,
    /// Whether the `*228` origination verified. None when it was not authenticated.
    pub origination_verified: Option<bool>,
}

impl MscRuntime {
    pub fn set_mo_submitter(&mut self, submitter: Arc<dyn cdma_smsc::client::MoSubmitter>) {
        if let Some(smsc) = self.smsc.as_mut() {
            smsc.set_mo_submitter(submitter);
        }
    }

    pub fn set_delivery_link(&mut self, link: crate::sms_delivery::DeliveryLink) {
        if let Some(smsc) = self.smsc.as_mut() {
            smsc.set_outcome_reporter(link.outcomes.clone());
        }
        self.deliveries = Some(link);
    }

    pub fn new(config: MscRuntimeConfig) -> Self {
        let smsc = config.smsc_repo.as_ref().map(|smsc_repo| {
            crate::sms::MscSmsCoordinator::new(Arc::clone(smsc_repo), Arc::clone(&config.hlr_repo))
        });
        let otasp_history = crate::otasp::OtaspHistory::new();
        let (otasp_event_tx, _) =
            tokio::sync::broadcast::channel::<crate::grpc::events_proto::v1::MscNetworkEvent>(256);
        let otasp = match config.otasp.as_ref() {
            Some(otasp_cfg) => Some(crate::otasp::OtaspCoordinator::with_history(
                otasp_cfg.clone(),
                otasp_cfg.home_network.clone(),
                Arc::clone(&config.hlr_repo),
                Arc::clone(&otasp_history),
                Some(otasp_event_tx.clone()),
            )),
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
            otapa_routes: std::collections::HashMap::new(),
            pending_otapa_calls: std::collections::HashMap::new(),
            pending_otasp_originations: std::collections::HashMap::new(),
            deliveries: None,
            pending_sms_traffic: std::collections::HashMap::new(),
            call_keys: std::collections::HashMap::new(),
            privacy_deadlines: std::collections::HashMap::new(),
            pending_privacy: std::collections::HashMap::new(),
            privacy_authenticated: std::collections::HashSet::new(),
            traffic_auth_started: std::collections::HashSet::new(),
            call_auth_context: std::collections::HashMap::new(),
            traffic_auth: std::collections::HashMap::new(),
            pending_ssd_calls: std::collections::HashSet::new(),
            held_assignments: std::collections::HashMap::new(),
            held_m2m_callees: std::collections::HashMap::new(),
        }
    }

    async fn continue_assignment_complete(
        &mut self,
        a1: &dyn MscA1Endpoint,
        call_id: CallId,
        completed_circuit_id: Option<u16>,
        completed_leg: Option<MscVoiceLeg>,
    ) {
        if let Some(call) = self.circuits.m2m_calls.get(&call_id).copied() {
            if completed_circuit_id.is_some() {
                self.complete_m2m_assignment(a1, call, call_id).await;
            }
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
        } else if completed_leg == Some(MscVoiceLeg::Primary) && !self.config.sip_ringback_disable {
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
            && self
                .controller
                .snapshot(call_id)
                .is_some_and(|snapshot| snapshot.direction == CallDirection::MobileOriginated)
        {
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

        // Sending 180 after 183 with early media can make the trunk replace MSC audio with local ringback.
        if let Some(meta) = self.media_gw.inbound_session_for_call(call_id).cloned()
            && !meta.progress_sent
            && !self.config.inbound_sip_msc_ringback
        {
            if let Some(session_id) = self.media_gw.inbound_by_call.get(&call_id).cloned() {
                if let Some(gateway) = self.config.media_gateway.clone() {
                    if let Err(error) = gateway.inbound_progress(&session_id).await {
                        warn!("MSC: inbound_progress failed for session={session_id}: {error}");
                    }
                }
            }
            self.media_gw.mark_inbound_progress_sent(call_id);
        }
    }

    /// Holds a call that failed the global challenge until an SSD update and a
    /// unique challenge on its traffic channel pass.
    async fn hold_until_authenticated(
        &mut self,
        a1: &dyn MscA1Endpoint,
        node: &BaseStationId,
        call_id: CallId,
        held: HeldAssignment,
    ) {
        self.start_traffic_authentication(a1, node, call_id).await;
        if !self.traffic_auth.contains_key(&call_id) {
            warn!(
                "MSC: call_id={} failed authentication and no SSD update could start, clearing",
                call_id.0
            );
            self.send_clear_command(a1, call_id).await;
            return;
        }
        info!(
            "MSC: call_id={} held until the SSD update and unique challenge pass",
            call_id.0
        );
        self.held_assignments.insert(call_id, held);
    }

    fn assignment_privacy(
        &mut self,
        call_id: CallId,
        handset_requested: bool,
    ) -> Option<cdma_ios::EncryptionInformation> {
        self.pending_privacy.remove(&call_id);
        let info = self
            .config
            .authentication
            .voice_privacy
            .encryption_information(self.call_keys.get(&call_id), handset_requested)?;
        let parameters = info.private_long_code_parameter().ok().flatten()?;
        // The private-code request must follow traffic authentication so the handset selects the current call's mask.
        if parameters.active && !self.privacy_authenticated.contains(&call_id) {
            self.pending_privacy.insert(call_id, info);
            cdma_ios::EncryptionInformation::private_long_code(parameters.mask, false).ok()
        } else {
            Some(info)
        }
    }

    async fn send_privacy_command(
        &mut self,
        a1: &dyn MscA1Endpoint,
        call_id: CallId,
        info: cdma_ios::EncryptionInformation,
    ) {
        let command = cdma_ios::PrivacyModeCommandMessage {
            encryption_information: info,
        };
        let payload = match command.encode() {
            Ok(payload) => payload,
            Err(error) => {
                warn!("MSC: failed to encode Privacy Mode Command: {error}");
                return;
            }
        };
        let message = cdma_ios::Message::new(cdma_ios::MessageType::PrivacyModeCommand, payload);
        match a1
            .send(EncodedA1Message::from_message_for_call(
                &message,
                Some(call_id.0),
            ))
            .await
        {
            Ok(()) => {
                info!("MSC: A1 tx PrivacyModeCommand call_id={}", call_id.0);
                self.privacy_deadlines
                    .insert(call_id, tokio::time::Instant::now() + PRIVACY_MODE_TIMEOUT);
            }
            Err(error) => warn!(
                "MSC: failed to send Privacy Mode Command call_id={}: {error}",
                call_id.0
            ),
        }
    }

    async fn start_traffic_authentication(
        &mut self,
        a1: &dyn MscA1Endpoint,
        node: &BaseStationId,
        call_id: CallId,
    ) {
        if self.traffic_auth.contains_key(&call_id)
            || (self.traffic_auth_started.contains(&call_id)
                && !self.pending_privacy.contains_key(&call_id))
        {
            return;
        }
        let Some(context) = self.call_auth_context.get(&call_id).cloned() else {
            return;
        };
        let ssd_unset = match self
            .config
            .hlr_repo
            .get_subscriber_auth_state(&context.identity)
            .await
        {
            Ok(Some(state)) => state.credentials_provisioned && !state.ssd_provisioned,
            Ok(None) => false,
            Err(error) => {
                warn!("MSC: HLR auth state lookup failed: {error}");
                false
            }
        };
        let mut authentication = self.config.authentication.clone();
        authentication.unique_challenge_on_call |= self.pending_privacy.contains_key(&call_id);
        let Some(mut procedures) = crate::authentication::TrafficAuthentication::plan(
            &authentication,
            context,
            node.0.clone(),
            ssd_unset,
        ) else {
            return;
        };
        let action = procedures
            .advance(&self.config.authentication, self.config.hlr_repo.as_ref())
            .await;
        self.traffic_auth_started.insert(call_id);
        self.traffic_auth.insert(call_id, procedures);
        self.apply_traffic_action(a1, call_id, action).await;
    }

    async fn apply_traffic_action(
        &mut self,
        a1: &dyn MscA1Endpoint,
        call_id: CallId,
        mut action: crate::authentication::TrafficAction,
    ) {
        use crate::authentication::TrafficAction;
        loop {
            match action {
                TrafficAction::Send(message_type, payload) => {
                    info!("MSC: A1 tx {:?} call_id={}", message_type, call_id.0);
                    if let Err(error) = a1
                        .send(EncodedA1Message::from_message_for_call(
                            &cdma_ios::Message::new(message_type, payload),
                            Some(call_id.0),
                        ))
                        .await
                    {
                        warn!("MSC: failed to send {message_type:?}: {error}");
                        self.traffic_auth.remove(&call_id);
                    }
                    return;
                }
                TrafficAction::Report {
                    procedure,
                    result,
                    clear_call,
                } => {
                    let Some(entry) = self.traffic_auth.get_mut(&call_id) else {
                        return;
                    };
                    let _ = self
                        .otasp_event_tx
                        .send(crate::authentication::authentication_event(
                            procedure,
                            result,
                            clear_call,
                            Some(&entry.context.imsi),
                            Some(entry.context.esn),
                            &entry.base_station,
                        ));
                    let held_failed = result
                        != crate::grpc::events_proto::v1::AuthenticationResult::Success
                        && matches!(
                            procedure,
                            crate::grpc::events_proto::v1::AuthenticationProcedure::SsdUpdate
                                | crate::grpc::events_proto::v1::AuthenticationProcedure::UniqueChallenge
                        )
                        && self.held_assignments.contains_key(&call_id);
                    if clear_call || held_failed {
                        self.traffic_auth.remove(&call_id);
                        self.held_assignments.remove(&call_id);
                        self.send_clear_command(a1, call_id).await;
                        return;
                    }
                    if procedure
                        == crate::grpc::events_proto::v1::AuthenticationProcedure::SsdUpdate
                        && self
                            .otasp
                            .as_ref()
                            .is_some_and(|otasp| otasp.owns_call(call_id.0))
                    {
                        self.traffic_auth.remove(&call_id);
                        let ok =
                            result == crate::grpc::events_proto::v1::AuthenticationResult::Success;
                        Box::pin(self.finish_otasp_ssd_update(a1, call_id, ok)).await;
                        return;
                    }
                    if procedure
                        == crate::grpc::events_proto::v1::AuthenticationProcedure::UniqueChallenge
                    {
                        if result == crate::grpc::events_proto::v1::AuthenticationResult::Success {
                            self.privacy_authenticated.insert(call_id);
                            if let Some(info) = self.pending_privacy.remove(&call_id) {
                                self.send_privacy_command(a1, call_id, info).await;
                            }
                        } else if self.pending_privacy.remove(&call_id).is_some() {
                            warn!(
                                "MSC: voice privacy remains inactive after failed unique challenge call_id={}",
                                call_id.0
                            );
                        }
                    }
                    let Some(entry) = self.traffic_auth.get_mut(&call_id) else {
                        return;
                    };
                    action = entry
                        .advance(&self.config.authentication, self.config.hlr_repo.as_ref())
                        .await;
                }
                TrafficAction::Done => {
                    let authentication_node = self
                        .traffic_auth
                        .get(&call_id)
                        .map(|entry| BaseStationId(entry.base_station.clone()));
                    if self
                        .traffic_auth
                        .get(&call_id)
                        .is_some_and(|entry| entry.awaiting.is_none())
                    {
                        self.traffic_auth.remove(&call_id);
                    }
                    if self.pending_privacy.contains_key(&call_id)
                        && !self.privacy_authenticated.contains(&call_id)
                        && let Some(node) = authentication_node
                    {
                        Box::pin(self.start_traffic_authentication(a1, &node, call_id)).await;
                    }
                    if let Some(held) = self.held_assignments.remove(&call_id) {
                        info!(
                            "MSC: call_id={} passed the SSD update and unique challenge, connecting",
                            call_id.0
                        );
                        if let Some((destination, calling_number)) =
                            self.held_m2m_callees.remove(&call_id)
                        {
                            Box::pin(self.start_m2m_callee(
                                a1,
                                call_id,
                                destination,
                                calling_number,
                            ))
                            .await;
                        }
                        Box::pin(self.continue_assignment_complete(
                            a1,
                            call_id,
                            held.completed_circuit_id,
                            held.completed_leg,
                        ))
                        .await;
                    }
                    return;
                }
            }
        }
    }

    async fn release_otasp_call(&mut self, a1: &dyn MscA1Endpoint, call_id_raw: u64) {
        info!(
            "MSC: OTASP session terminal — releasing call_id={}",
            call_id_raw
        );
        send_gateway_clear_command(
            a1,
            CallId(call_id_raw),
            &mut self.controller,
            gateway_clear_cause(ReleaseCause::Administrative, None),
        )
        .await;
    }

    async fn start_otasp_ssd_updates(&mut self, a1: &dyn MscA1Endpoint) {
        let Some(otasp) = self.otasp.as_mut() else {
            return;
        };
        for update in otasp.take_ssd_updates() {
            let call_id = CallId(update.call_id);
            let mut procedures = crate::authentication::TrafficAuthentication::ssd_update_only(
                update.context,
                String::new(),
            );
            let action = procedures
                .advance(&self.config.authentication, self.config.hlr_repo.as_ref())
                .await;
            self.traffic_auth.insert(call_id, procedures);
            Box::pin(self.apply_traffic_action(a1, call_id, action)).await;
        }
    }

    async fn finish_otasp_ssd_update(&mut self, a1: &dyn MscA1Endpoint, call_id: CallId, ok: bool) {
        let Some(otasp) = self.otasp.as_mut() else {
            return;
        };
        if let Some(release) = otasp.on_ssd_update_finished(call_id.0, ok, a1).await {
            self.release_otasp_call(a1, release).await;
        }
    }

    async fn tick_traffic_authentication(&mut self, a1: &dyn MscA1Endpoint) {
        let expired: Vec<_> = self
            .privacy_deadlines
            .iter()
            .filter(|(_, deadline)| **deadline <= tokio::time::Instant::now())
            .map(|(id, _)| *id)
            .collect();
        for call_id in expired {
            self.privacy_deadlines.remove(&call_id);
            warn!(
                "MSC: Privacy Mode Complete timed out, clearing call_id={}",
                call_id.0
            );
            self.send_clear_command(a1, call_id).await;
        }

        let now = tokio::time::Instant::now();
        let overdue: Vec<CallId> = self
            .traffic_auth
            .iter()
            .filter(|(_, entry)| entry.awaiting.is_some() && entry.deadline <= now)
            .map(|(call_id, _)| *call_id)
            .collect();
        for call_id in overdue {
            let Some(entry) = self.traffic_auth.get_mut(&call_id) else {
                continue;
            };
            let action = entry.on_timeout(self.config.hlr_repo.as_ref()).await;
            self.apply_traffic_action(a1, call_id, action).await;
        }
    }

    async fn handle_traffic_authentication_reply(
        &mut self,
        a1: &dyn MscA1Endpoint,
        call_id: CallId,
        message_type: cdma_ios::MessageType,
        payload: &[u8],
    ) {
        let Some(entry) = self.traffic_auth.get_mut(&call_id) else {
            warn!(
                "MSC: {:?} for call_id={} with no authentication procedure running",
                message_type, call_id.0
            );
            return;
        };
        let config = &self.config.authentication;
        let hlr = self.config.hlr_repo.as_ref();
        let action = match message_type {
            cdma_ios::MessageType::BaseStationChallenge => {
                match cdma_ios::BaseStationChallengeMessage::decode(payload) {
                    Ok(message) => match message.authentication_challenge_parameter_randbs.randbs()
                    {
                        Some(randbs) => entry.on_base_station_challenge(config, hlr, randbs).await,
                        None => {
                            warn!("MSC: Base Station Challenge without RANDBS");
                            return;
                        }
                    },
                    Err(error) => {
                        warn!("MSC: failed to decode Base Station Challenge: {error}");
                        return;
                    }
                }
            }
            cdma_ios::MessageType::SsdUpdateResponse => {
                match cdma_ios::SsdUpdateResponseMessage::decode(payload) {
                    Ok(message) => {
                        entry
                            .on_ssd_update_response(hlr, message.cause_layer_3.is_none())
                            .await
                    }
                    Err(error) => {
                        warn!("MSC: failed to decode SSD Update Response: {error}");
                        return;
                    }
                }
            }
            cdma_ios::MessageType::AuthenticationResponse => {
                let authu = match cdma_ios::AuthenticationResponseMessage::decode(payload) {
                    Ok(cdma_ios::AuthenticationResponseMessage::Dtap(message)) => {
                        message.authentication_response_parameter_authu.signature()
                    }
                    Ok(cdma_ios::AuthenticationResponseMessage::Bsmap(message)) => {
                        message.authentication_response_parameter_authu.signature()
                    }
                    Err(error) => {
                        warn!("MSC: failed to decode Authentication Response: {error}");
                        return;
                    }
                };
                entry.on_authentication_response(config, hlr, authu).await
            }
            cdma_ios::MessageType::ParameterUpdateConfirm => {
                entry.on_parameter_update_confirm(hlr).await
            }
            _ => return,
        };
        self.apply_traffic_action(a1, call_id, action).await;
    }

    /// Verifies the global challenge of an SMS sent on the access channel. A
    /// rejected SMS is answered with an ADDS Transfer Ack carrying Cause
    /// "Authentication failure" (A.S0014-D §3.6.4) and not delivered. Returns
    /// true when the SMS may be delivered.
    async fn authenticate_adds_transfer(
        &mut self,
        a1: &dyn MscA1Endpoint,
        node: &BaseStationId,
        msg: &cdma_ios::AddsTransferMessage,
    ) -> bool {
        let (imsi, esn, meid) = adds_transfer_identity(a1, node, msg);
        let identity_key =
            cdma_hlr::model::MobileIdentityKey::from_parts(imsi.as_deref(), esn, meid.as_deref())
                .ok();
        let destination = cdma_common::sms::decode_mo_sms(&msg.adds_user_part.data)
            .map(|sms| sms.destination_number);
        let subject = crate::authentication::AuthenticationSubject {
            identity: identity_key.as_ref(),
            imsi: imsi.as_deref(),
            esn,
            dialed_digits: destination.as_deref(),
        };
        let decision = self
            .authenticate_access(
                node,
                crate::authentication::AuthenticationProcedure::DataBurst,
                &subject,
                &crate::authentication::GlobalChallengeIes::from_adds_transfer(msg),
                AccessPurpose::Service,
            )
            .await;
        if decision.allow {
            return true;
        }
        let ack = cdma_ios::AddsTransferAckMessage {
            mobile_identity_imsi: imsi
                .map(cdma_ios::MobileIdentity::Imsi)
                .unwrap_or_else(|| msg.mobile_identity_imsi.clone()),
            tag: msg.tag,
            cause: Some(cdma_ios::Cause(cdma_ios::CAUSE_AUTHENTICATION_FAILURE)),
        };
        match ack.encode() {
            Ok(payload) => {
                let message =
                    cdma_ios::Message::new(cdma_ios::MessageType::AddsTransferAck, payload);
                if let Err(error) = a1
                    .send_to_node(
                        node,
                        EncodedA1Message::from_message_for_call(&message, None),
                    )
                    .await
                {
                    warn!("MSC: failed to send ADDS Transfer Ack: {error}");
                }
            }
            Err(error) => warn!("MSC: failed to encode ADDS Transfer Ack: {error}"),
        }
        false
    }

    async fn authenticate_access(
        &mut self,
        node: &BaseStationId,
        procedure: crate::authentication::AuthenticationProcedure,
        subject: &crate::authentication::AuthenticationSubject<'_>,
        ies: &crate::authentication::GlobalChallengeIes,
        purpose: AccessPurpose,
    ) -> crate::authentication::AuthenticationDecision {
        let mut authentication = self.config.authentication.clone();
        if purpose == AccessPurpose::Provisioning
            && authentication.mode == crate::authentication::AuthenticationMode::Enforce
        {
            authentication.mode = crate::authentication::AuthenticationMode::Monitor;
        }
        let decision = crate::authentication::authenticate(
            &authentication,
            self.config.hlr_repo.as_ref(),
            procedure,
            subject,
            ies,
        )
        .await;
        if let Some(event) = decision.to_event(subject, &node.0) {
            let _ = self.otasp_event_tx.send(event);
        }
        decision
    }

    async fn reject_access_for_authentication(&mut self, a1: &dyn MscA1Endpoint, call_id: CallId) {
        let clear_command = cdma_ios::ClearCommandMessage {
            cause: cdma_ios::Cause(cdma_ios::CAUSE_AUTHENTICATION_FAILURE),
            cause_layer3: None,
        };
        match clear_command.encode() {
            Ok(payload) => {
                if let Err(error) = a1
                    .send(EncodedA1Message::from_message_for_call(
                        &cdma_ios::Message::new(cdma_ios::MessageType::ClearCommand, payload),
                        Some(call_id.0),
                    ))
                    .await
                {
                    warn!(
                        "MSC: failed to send authentication-failure ClearCommand call_id={}: {}",
                        call_id.0, error
                    );
                }
            }
            Err(error) => warn!("MSC: failed to encode ClearCommand: {error}"),
        }
        a1.release_call(call_id);
    }

    async fn record_mobile_contact(
        &mut self,
        node: &BaseStationId,
        contact: MobileContact,
        context: &str,
    ) -> Option<RecordedContact> {
        let hlr = self.config.hlr_repo.as_ref();
        let upsert = match hlr.upsert_mobile_seen(&contact.identity_key, None).await {
            Ok(upsert) => upsert,
            Err(error) => {
                warn!("MSC: upsert_mobile_seen failed during {context}: {error}");
                return None;
            }
        };
        if let Some(smsc) = self.smsc.as_mut() {
            smsc.record_mobile_contact(contact.identity_key.imsi(), node);
        }
        let subscriber = hlr.resolve_by_identity(&contact.identity_key).await;
        match &subscriber {
            Ok(Some(resolved)) => {
                Self::refresh_registration_binding(
                    hlr,
                    node,
                    resolved.subscriber.subscriber_id,
                    contact.imsi,
                    contact.esn,
                    contact.meid,
                    contact.slot_cycle_index,
                )
                .await;
            }
            Ok(None) => {}
            Err(error) => warn!("MSC: HLR lookup failed during {context} refresh: {error}"),
        }
        Some(RecordedContact {
            upsert,
            subscriber: subscriber.map_err(|_| ()),
        })
    }

    async fn send_welcome_sms_if_due(
        &mut self,
        node: &BaseStationId,
        identity_key: &cdma_hlr::model::MobileIdentityKey,
        contact: &RecordedContact,
    ) {
        let welcome_cfg = match self.config.welcome_sms.as_ref() {
            Some(config) if config.enabled => config.clone(),
            _ => return,
        };
        let upsert = &contact.upsert;
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
        let Ok(subscriber) = &contact.subscriber else {
            return;
        };
        let destination = if let Some(resolved) = subscriber {
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
        let smsc = match self.smsc.as_mut() {
            Some(smsc) => smsc,
            None => {
                warn!(
                    "MSC: registration accepted but SMSC coordinator is not configured for welcome SMS"
                );
                return;
            }
        };
        smsc.send_sms(crate::sms::SmsSendRequest {
            originating_number: welcome_cfg.originating_number.clone(),
            text: welcome_cfg.text.clone(),
            destination,
            timeout_ms: 30_000,
            teleservice_id: None,
            raw_user_data: None,
            serving_node: Some(node.clone()),
        })
        .await;
    }

    async fn refresh_registration_binding(
        hlr_repo: &dyn cdma_hlr::repository::HlrRepository,
        serving_node: &BaseStationId,
        subscriber_id: uuid::Uuid,
        imsi: Option<String>,
        esn: Option<u32>,
        meid: Option<String>,
        slot_cycle_index: Option<u32>,
    ) {
        let existing = match hlr_repo.get_registration_binding(subscriber_id).await {
            Ok(binding) => binding,
            Err(error) => {
                warn!("MSC: failed to read registration binding before uplink refresh: {error}");
                return;
            }
        };
        let now = chrono::Utc::now();
        let existing_imsi = existing.as_ref().and_then(|binding| binding.imsi.clone());
        let existing_esn = existing.as_ref().and_then(|binding| binding.esn);
        let existing_meid = existing.as_ref().and_then(|binding| binding.meid.clone());
        let binding = RegistrationBinding {
            subscriber_id,
            serving_bs_id: serving_node.0.clone(),
            state: cdma_hlr::model::RegistrationState::Registered,
            imsi: imsi.or(existing_imsi),
            esn: esn.or(existing_esn),
            meid: meid.or(existing_meid),
            mob_p_rev: existing.as_ref().and_then(|binding| binding.mob_p_rev),
            pgslot: existing.as_ref().and_then(|binding| binding.pgslot),
            slot_cycle_index: slot_cycle_index.or_else(|| {
                existing
                    .as_ref()
                    .and_then(|binding| binding.slot_cycle_index)
            }),
            last_msg_seq: existing.as_ref().and_then(|binding| binding.last_msg_seq),
            last_registered_at: now,
            last_seen_at: now,
            updated_at: now,
        };
        match hlr_repo.upsert_registration_binding(binding).await {
            Ok(_) => info!(
                "MSC: recorded registration binding from subscriber uplink subscriber_id={subscriber_id}"
            ),
            Err(error) => warn!("MSC: failed to refresh registration binding: {error}"),
        }
    }

    pub fn otasp_history(&self) -> std::sync::Arc<crate::otasp::OtaspHistory> {
        std::sync::Arc::clone(&self.otasp_history)
    }

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
        let mut otasp_timeout_interval = tokio::time::interval(Duration::from_secs(1));
        otasp_timeout_interval.tick().await;

        // Restart recovery: the in-flight ADDS Page correlation lives only
        // in MscSmsCoordinator::pending (in-memory). Anything left in DB
        // state `Paging` on a fresh boot was orphaned by the previous
        // process and the retry sweep will skip it forever otherwise.
        // Threshold is 2× the regular in-flight timeout so we don't fight
        // with `expire_pending` over genuinely fresh attempts.

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
                inbound = a1.recv() => {
                    match inbound {
                        Some(A1Event::Message { node, message }) => {
                            self.handle_bsc_a1_message(a1, &node, message).await;
                        }
                        Some(A1Event::Attached(node)) => {
                            info!("MSC: base station {node} attached");
                        }
                        Some(A1Event::Detached { node, calls }) => {
                            self.handle_base_station_detached(a1, &node, calls).await;
                        }
                        None => break,
                    }
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
                    let due = self.media.drain_due_wav_starts();
                    for call_id in due {
                        send_progress_tones_off(a1, call_id, &mut self.controller).await;
                        self.media.start_media_for_call(
                            call_id,
                            &self.circuits,
                            self.config.voice_bearer.as_ref(),
                        );
                    }
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
                _ = otasp_timeout_interval.tick() => {
                    self.tick_otasp_timeouts(a1).await;
                    self.tick_traffic_authentication(a1).await;
                }
                command = recv_delivery(self.deliveries.as_mut()) => {
                    if let Some(command) = command {
                        let deliveries = if let Some(smsc) = self.smsc.as_mut() {
                            smsc.deliver_from_smsc(command, a1).await;
                            smsc.take_traffic_deliveries()
                        } else {
                            Vec::new()
                        };
                        for delivery in deliveries {
                            self.start_mt_sms_traffic(a1, delivery).await;
                        }
                    }
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
    pub async fn run_with_grpc(
        &mut self,
        mgmt_addr: std::net::SocketAddr,
        base_stations: Arc<crate::base_station::BaseStations>,
        net_mgmt: crate::mgmt_proxy::NetworkManagement,
    ) {
        let (mgmt_tx, mgmt_rx) = tokio::sync::mpsc::channel::<PendingControlRequest>(16);
        let service = crate::grpc::MscManagementServiceImpl::from_channel(mgmt_tx)
            .with_otasp(self.otasp_event_tx())
            .with_base_stations(base_stations.clone());
        let server = tonic::transport::Server::builder()
            .add_service(
                crate::grpc::msc_management::v1::msc_management_service_server::MscManagementServiceServer::new(service),
            )
            .add_service(
                crate::grpc::mgmt::v1::network_management_service_server::NetworkManagementServiceServer::new(net_mgmt),
            )
            .serve(mgmt_addr);
        let served = tokio::spawn(server);
        // Return when either side stops. The caller treats a returning
        // runtime as fatal, so a dead management listener stops the MSC
        // instead of leaving a healthy-looking process with no management
        // plane.
        tokio::select! {
            result = served => match result {
                Ok(Err(e)) => log::error!("MSC management gRPC server on {mgmt_addr} stopped: {e}"),
                _ => log::error!("MSC management gRPC server on {mgmt_addr} stopped"),
            },
            _ = self.run(mgmt_rx, base_stations.as_ref()) => {}
        }
    }

    async fn handle_management_request(
        &mut self,
        request: PendingControlRequest,
        a1: &dyn MscA1Endpoint,
    ) {
        match request {
            PendingControlRequest::InitiateOtasp {
                subscriber_id,
                imsi,
                response_tx,
            } => {
                let result = self
                    .initiate_otasp(a1, subscriber_id, imsi.as_deref())
                    .await;
                if response_tx.send(result).is_err() {
                    warn!("MSC: provisioning response receiver dropped");
                }
            }
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
                    smsc.send_sms(request).await
                } else {
                    warn!("MSC: send_sms requested but SMSC coordinator is not configured");
                    None
                };
                if response_tx.send(result).is_err() {
                    warn!("MSC: send-sms response receiver dropped");
                }
                let deliveries = self
                    .smsc
                    .as_mut()
                    .map(|smsc| smsc.take_traffic_deliveries())
                    .unwrap_or_default();
                for delivery in deliveries {
                    self.start_mt_sms_traffic(a1, delivery).await;
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
        // The page goes to the node the mobile last registered through. An
        // unattached node is a refusal, not a broadcast.
        let serving_node = BaseStationId(binding.serving_bs_id.clone());
        if !a1.is_attached(&serving_node) {
            return Err(MtSetupError::ServingNodeUnavailable(
                subscriber_id,
                serving_node.0,
            ));
        }

        let call_id = self.controller.create_call(
            CallDirection::MobileTerminated,
            Some(cdma_ios::MobileIdentity::Imsi(imsi.to_string())),
        );
        a1.bind_call(call_id, &serving_node);
        self.media_gw
            .register_active_subscriber(subscriber_id, call_id);
        let tag = cdma_ios::Tag(call_id.0 as u32);
        let service_option = select_mt_voice_service_option(
            binding.mob_p_rev,
            &self.config.supported_voice_service_options,
            self.config.default_voice_service_option,
        );
        let paging_request = cdma_ios::PagingRequestMessage {
            mobile_identity_meid: None,
            mobile_identity_imsi: cdma_ios::MobileIdentity::Imsi(imsi.to_string()),
            tag: Some(tag),
            cell_identifier_list: None,
            slot_cycle_index: binding
                .slot_cycle_index
                .map(|value| cdma_ios::SlotCycleIndex(value as u8)),
            service_option: Some(cdma_ios::ServiceOption(service_option)),
            is2000_mobile_capabilities: None,
        };
        if let Err(e) = self.controller.apply_from_msc(
            call_id,
            &cdma_ios::ProcedureMessage::PagingRequest(paging_request.clone()),
        ) {
            self.abort_mt_setup(a1, call_id);
            return Err(MtSetupError::Other(format!("msc paging state error: {e}")));
        }
        self.mt_call.mt_plans.insert(
            tag.0,
            MtCallPlan {
                subscriber_id,
                imsi: imsi.to_string(),
                audio_file,
                caller_number,
                service_option,
            },
        );
        self.circuits
            .paging_requests
            .insert(call_id, paging_request.clone());
        if !self
            .send_paging_request_to_bsc(a1, call_id, paging_request, PagePurpose::Initial)
            .await
        {
            self.abort_mt_setup(a1, call_id);
            return Err(MtSetupError::A1Closed);
        }
        Ok(call_id)
    }

    /// Binary SMS requires a traffic channel before ADDS Page delivery to avoid
    /// an idle Airave DSP fault (X.S0059-200 Fig. 22).
    pub(crate) async fn start_mt_sms_traffic(
        &mut self,
        a1: &dyn MscA1Endpoint,
        delivery: crate::sms::SmsTrafficDelivery,
    ) {
        let imsi = match &delivery.mobile_identity {
            cdma_ios::MobileIdentity::Imsi(imsi) => imsi.clone(),
            _ => {
                warn!(
                    "MSC: MT SMS traffic delivery has no IMSI; dropping sms_id={}",
                    delivery.sms_id
                );
                return;
            }
        };
        if !a1.is_attached(&delivery.serving_node) {
            info!(
                "MSC: MT SMS traffic serving node {} unavailable; retry sweep will re-drive sms_id={}",
                delivery.serving_node, delivery.sms_id
            );
            return;
        }
        let call_id = self.controller.create_call(
            CallDirection::MobileTerminated,
            Some(cdma_ios::MobileIdentity::Imsi(imsi.clone())),
        );
        a1.bind_call(call_id, &delivery.serving_node);
        let paging_request = cdma_ios::PagingRequestMessage {
            mobile_identity_meid: None,
            mobile_identity_imsi: cdma_ios::MobileIdentity::Imsi(imsi),
            tag: Some(cdma_ios::Tag(call_id.0 as u32)),
            cell_identifier_list: None,
            slot_cycle_index: None,
            service_option: Some(cdma_ios::ServiceOption::SMS),
            is2000_mobile_capabilities: None,
        };
        if let Err(e) = self.controller.apply_from_msc(
            call_id,
            &cdma_ios::ProcedureMessage::PagingRequest(paging_request.clone()),
        ) {
            warn!("MSC: MT SMS paging state error call_id={}: {e}", call_id.0);
            self.controller.remove_call(call_id);
            a1.release_call(call_id);
            return;
        }
        self.circuits
            .paging_requests
            .insert(call_id, paging_request.clone());
        self.pending_sms_traffic.insert(call_id, delivery);
        info!("MSC: A1 tx PagingRequest (MT SMS) call_id={}", call_id.0);
        if !self
            .send_paging_request_to_bsc(a1, call_id, paging_request, PagePurpose::Initial)
            .await
        {
            self.pending_sms_traffic.remove(&call_id);
            self.controller.remove_call(call_id);
            a1.release_call(call_id);
        }
    }

    pub(crate) fn abort_mt_setup(&mut self, a1: &dyn MscA1Endpoint, call_id: CallId) {
        self.mt_page_retry.cancel(call_id);
        self.controller.remove_call(call_id);
        self.stop_media_for_call(call_id);
        a1.release_call(call_id);
    }

    /// Send an A1 PagingRequest; rearms the controller engine on
    /// `RepageAfterAf{ Primary }` and registers with `mt_page_retry` unless
    /// `purpose == Retry`. Returns `false` if rearm/encode/transport failed.
    pub(crate) async fn send_paging_request_to_bsc(
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
            .send(EncodedA1Message::from_message_for_call(
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
        a1.release_call(call_id);
    }

    /// Handles one inbound A1 message from the BSC.
    pub async fn handle_bsc_a1_message(
        &mut self,
        a1: &dyn MscA1Endpoint,
        node: &BaseStationId,
        message: EncodedA1Message,
    ) {
        // ADDS messages use SMS Tag correlation, not the A1 transport call_id.
        // Route them directly to the SMS coordinator before the call_id check.
        // Location Updating Request is connectionless on A1. The local transport
        // correlation, when present, is returned to the femtocell adapter so it
        // can address the response to the originating mobile.
        match message.message_type() {
            cdma_ios::MessageType::AddsPageAck
            | cdma_ios::MessageType::AddsDeliverAck
            | cdma_ios::MessageType::AddsTransfer
            | cdma_ios::MessageType::AddsDeliver => {
                self.handle_adds_message(a1, node, message).await;
                return;
            }
            cdma_ios::MessageType::CompleteLayer3Information
                if message.decode().ok().is_some_and(|decoded| {
                    cdma_ios::CompleteLayer3InformationMessage::decode(&decoded.payload).is_ok_and(
                        |cli3| {
                            cli3.layer3_information
                                .decode_location_updating_request()
                                .is_ok()
                        },
                    )
                }) =>
            {
                self.handle_registration_notification(a1, node, message)
                    .await;
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

        if decoded.message_type != cdma_ios::MessageType::CompleteLayer3Information
            && self.controller.snapshot(call_id).is_none()
        {
            debug!(
                "MSC: ignoring {:?} for released or unknown call_id={}",
                decoded.message_type, call_id_raw
            );
            a1.release_call(call_id);
            return;
        }

        match decoded.message_type {
            cdma_ios::MessageType::PagingResponse => {
                let response = match cdma_ios::PagingResponseMessage::decode(&decoded.payload) {
                    Ok(response) => response,
                    Err(error) => {
                        warn!("MSC: failed to decode A1 Paging Response: {}", error);
                        return;
                    }
                };
                if (self.circuits.m2m_calls.contains_key(&call_id)
                    || self.pending_otapa_calls.contains_key(&call_id))
                    && self.controller.state(call_id) != Some(cdma_ios::CallControlState::Paging)
                {
                    debug!(
                        "MSC: ignoring repeated M2M PagingResponse call_id={}",
                        call_id.0
                    );
                    return;
                }
                self.mt_page_retry.cancel(call_id);
                let imsi = match &response.mobile_identity_imsi {
                    cdma_ios::MobileIdentity::Imsi(imsi) => Some(
                        a1.serving_imsi_prefix(node)
                            .map(|(mcc, imsi_11_12)| {
                                crate::base_station::complete_imsi(&mcc, &imsi_11_12, imsi)
                            })
                            .unwrap_or_else(|| imsi.clone()),
                    ),
                    _ => None,
                };
                let esn = match response.mobile_identity_esn.as_ref() {
                    Some(cdma_ios::MobileIdentity::Esn(esn)) => Some(*esn),
                    _ => None,
                };
                let meid = response.mobile_identity_meid.as_ref().and_then(meid_hex);
                let auth_identity = cdma_hlr::model::MobileIdentityKey::from_parts(
                    imsi.as_deref(),
                    esn,
                    meid.as_deref(),
                )
                .ok();
                let provisioning = self.pending_otapa_calls.contains_key(&call_id);
                if provisioning {
                    let expected = self
                        .mt_call
                        .mt_plans
                        .get(&(call_id.0 as u32))
                        .map(|plan| plan.subscriber_id);
                    let target_matches =
                        self.pending_otapa_calls
                            .get(&call_id)
                            .is_some_and(|identity| {
                                crate::network_otasp::ProvisioningIdentity::from_parts(
                                    identity.imsi.as_deref().unwrap_or_default(),
                                    identity.esn,
                                    identity.meid.as_deref(),
                                )
                                .matches(
                                    imsi.as_deref().unwrap_or_default(),
                                    esn,
                                    meid.as_deref(),
                                )
                            });
                    let matched = if !target_matches {
                        None
                    } else if let Some(identity) = self.pending_otapa_calls.get(&call_id)
                        && (identity.meid.is_some() || identity.esn.is_some())
                    {
                        self.config
                            .hlr_repo
                            .resolve_by_hardware_identity(
                                if identity.meid.is_some() { None } else { esn },
                                meid.as_deref(),
                            )
                            .await
                            .ok()
                            .flatten()
                    } else {
                        match auth_identity.as_ref() {
                            Some(identity) => self
                                .config
                                .hlr_repo
                                .resolve_by_identity(identity)
                                .await
                                .ok()
                                .flatten(),
                            None => None,
                        }
                    };
                    if matched
                        .as_ref()
                        .map(|subscriber| subscriber.subscriber.subscriber_id)
                        != expected
                        || expected.is_none()
                    {
                        warn!(
                            "MSC: OTAPA page response rejected call_id={} target_matches={} expected_subscriber={:?} resolved_subscriber={:?} received_imsi={:?} received_esn={:?} received_meid={:?} target_imsi={:?} target_esn={:?} target_meid={:?}",
                            call_id.0,
                            target_matches,
                            expected,
                            matched
                                .as_ref()
                                .map(|subscriber| subscriber.subscriber.subscriber_id),
                            imsi,
                            esn,
                            meid,
                            self.pending_otapa_calls
                                .get(&call_id)
                                .and_then(|identity| identity.imsi.as_deref()),
                            self.pending_otapa_calls
                                .get(&call_id)
                                .and_then(|identity| identity.esn),
                            self.pending_otapa_calls
                                .get(&call_id)
                                .and_then(|identity| identity.meid.as_deref()),
                        );
                        self.fail_mt_call(a1, call_id, SIP_STATUS_FORBIDDEN).await;
                        return;
                    }
                }
                let subject = crate::authentication::AuthenticationSubject {
                    identity: auth_identity.as_ref(),
                    imsi: imsi.as_deref(),
                    esn,
                    dialed_digits: None,
                };
                let decision = self
                    .authenticate_access(
                        node,
                        crate::authentication::AuthenticationProcedure::PageResponse,
                        &subject,
                        &crate::authentication::GlobalChallengeIes::from_paging_response(&response),
                        if self.pending_otapa_calls.contains_key(&call_id) {
                            AccessPurpose::Provisioning
                        } else {
                            AccessPurpose::Service
                        },
                    )
                    .await;
                if !provisioning
                    && (!decision.allow
                        || (decision.pending_ssd_update
                            && self.pending_sms_traffic.contains_key(&call_id)))
                {
                    self.fail_mt_call(a1, call_id, SIP_STATUS_FORBIDDEN).await;
                    return;
                }
                if decision.pending_ssd_update && !provisioning {
                    self.pending_ssd_calls.insert(call_id);
                }
                if let Some(keys) = decision.call_keys {
                    self.call_keys.insert(call_id, keys);
                }
                if !provisioning
                    && let Some(context) =
                        crate::authentication::CallAuthContext::from_subject(&subject, &decision)
                {
                    self.privacy_authenticated.remove(&call_id);
                    self.traffic_auth_started.remove(&call_id);
                    self.call_auth_context.insert(call_id, context);
                }
                if !provisioning
                    && let Ok(identity_key) = cdma_hlr::model::MobileIdentityKey::from_parts(
                        imsi.as_deref(),
                        esn,
                        meid.as_deref(),
                    )
                    && let Some(contact) = self
                        .record_mobile_contact(
                            node,
                            MobileContact {
                                identity_key: identity_key.clone(),
                                imsi,
                                esn,
                                meid,
                                slot_cycle_index: response
                                    .slot_cycle_index
                                    .map(|value| u32::from(value.0)),
                            },
                            "paging-response",
                        )
                        .await
                {
                    self.send_welcome_sms_if_due(node, &identity_key, &contact)
                        .await;
                }
                if provisioning {
                    self.pending_otasp_originations.insert(
                        call_id,
                        PendingOtaspOrigination {
                            origination_verified: Some(
                                decision.result
                                    == crate::authentication::AuthenticationResult::Success,
                            ),
                            device: crate::otasp::HardwareIdentity {
                                esn,
                                meid: response.mobile_identity_meid.as_ref().and_then(meid_hex),
                            },
                            feature_code: String::new(),
                            mobile_identity_imsi: response.mobile_identity_imsi.clone(),
                            service_option: SERVICE_OPTION_OTASP,
                        },
                    );
                }
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
                if provisioning
                    && response
                        .service_option
                        .is_some_and(|option| option.0 != SERVICE_OPTION_OTASP)
                {
                    info!(
                        "MSC: handset declined OTAPA service option call_id={} response_so={:?}",
                        call_id.0,
                        response.service_option.map(|option| option.0)
                    );
                    self.fail_mt_call(a1, call_id, SIP_STATUS_FORBIDDEN).await;
                    return;
                }
                let privacy = if provisioning {
                    None
                } else {
                    self.assignment_privacy(call_id, response.voice_privacy_request)
                };
                self.mt_call
                    .send_assignment_request_for_paging_response(
                        a1,
                        call_id,
                        response.clone(),
                        secondary_leg,
                        &mut self.controller,
                        &mut self.circuits,
                        &self.mo_call,
                        self.config.voice_bearer.as_ref(),
                        self.config.default_voice_service_option,
                        &self.config.hlr_repo,
                        privacy,
                    )
                    .await;
            }
            cdma_ios::MessageType::PrivacyModeComplete => {
                let complete = match cdma_ios::PrivacyModeCompleteMessage::decode(&decoded.payload)
                {
                    Ok(complete) => complete,
                    Err(error) => {
                        warn!(
                            "MSC: invalid Privacy Mode Complete call_id={}: {error}",
                            call_id.0
                        );
                        return;
                    }
                };
                self.privacy_deadlines.remove(&call_id);
                if complete.voice_privacy_request {
                    let info = self
                        .config
                        .authentication
                        .voice_privacy
                        .encryption_information(self.call_keys.get(&call_id), true)
                        .unwrap_or_else(|| {
                            cdma_ios::EncryptionInformation::private_long_code(0, false)
                                .expect("zero is a 42-bit mask")
                        });
                    let active = info
                        .private_long_code_parameter()
                        .ok()
                        .flatten()
                        .is_some_and(|p| p.active);
                    if active && !self.privacy_authenticated.contains(&call_id) {
                        self.pending_privacy.insert(call_id, info);
                        self.start_traffic_authentication(a1, node, call_id).await;
                    } else {
                        self.send_privacy_command(a1, call_id, info).await;
                    }
                } else {
                    info!(
                        "MSC: privacy mode result call_id={} parameters={:?}",
                        call_id.0,
                        complete.encryption_information.as_ref().map(|info| info
                            .parameters
                            .iter()
                            .map(|parameter| (
                                parameter.identifier,
                                parameter.status,
                                parameter.available
                            ))
                            .collect::<Vec<_>>())
                    );
                }
            }
            cdma_ios::MessageType::BaseStationChallenge
            | cdma_ios::MessageType::SsdUpdateResponse
            | cdma_ios::MessageType::AuthenticationResponse
            | cdma_ios::MessageType::ParameterUpdateConfirm => {
                self.handle_traffic_authentication_reply(
                    a1,
                    call_id,
                    decoded.message_type,
                    &decoded.payload,
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
                if completed_circuit_id.is_none() && self.circuits.m2m_calls.contains_key(&call_id)
                {
                    debug!(
                        "MSC: ignoring unexpected M2M AssignmentComplete call_id={}",
                        call_id.0
                    );
                    return;
                }
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
                if let Some(delivery) = self.pending_sms_traffic.remove(&call_id) {
                    info!(
                        "MSC: AssignmentComplete call_id={} - delivering MT SMS on the traffic channel",
                        call_id.0
                    );
                    let encoded = EncodedA1Message::from_message_for_call(
                        &cdma_ios::Message::new(
                            cdma_ios::MessageType::AddsPage,
                            delivery.adds_page.clone(),
                        ),
                        Some(call_id.0),
                    );
                    if let Err(e) = a1.send_to_node(&delivery.serving_node, encoded).await {
                        warn!(
                            "MSC: failed to deliver MT SMS on traffic channel call_id={}: {e}",
                            call_id.0
                        );
                    } else if let Some(smsc) = self.smsc.as_mut() {
                        smsc.begin_pending(delivery.tag, delivery.sms_id, delivery.attempt_id);
                    }
                    // Delivery completes on the later ADDS Page Ack. The FAP
                    // keeps the channel until it finishes the burst and its own
                    // timers tear it down.
                    self.mt_page_retry.cancel(call_id);
                    self.controller.remove_call(call_id);
                    self.stop_media_for_call(call_id);
                    a1.release_call(call_id);
                    return;
                }
                if self.pending_otapa_calls.contains_key(&call_id) {
                    self.fire_assignment_complete_awi(a1, call_id, completed_leg)
                        .await;
                    return;
                }
                if let Some(pending) = self.pending_otasp_originations.remove(&call_id)
                    && let Some(otasp) = self.otasp.as_mut()
                {
                    info!(
                        "MSC: AssignmentComplete call_id={} — starting OTASP session",
                        call_id.0
                    );
                    let initiation = if self.pending_otapa_calls.contains_key(&call_id) {
                        crate::otasp::session::SessionInitiation::Network
                    } else {
                        crate::otasp::session::SessionInitiation::User
                    };
                    otasp
                        .begin_session_with_initiation(
                            initiation,
                            pending.device,
                            pending.feature_code,
                            pending.service_option,
                            pending.mobile_identity_imsi,
                            pending.origination_verified,
                            call_id.0,
                            a1,
                        )
                        .await;
                    return;
                }
                let completed_non_voice = completed_circuit_id
                    .and_then(|cid| self.circuits.circuits.get(&cid))
                    .is_some_and(|session| is_non_voice_a1_service_option(session.service_option));
                if completed_non_voice {
                    debug!(
                        "MSC: AssignmentComplete call_id={} completed non-voice A1 context",
                        call_id.0
                    );
                    self.controller.remove_call(call_id);
                    self.stop_media_for_call(call_id);
                    a1.release_call(call_id);
                    return;
                }
                if self.pending_ssd_calls.remove(&call_id) {
                    self.hold_until_authenticated(
                        a1,
                        node,
                        call_id,
                        HeldAssignment {
                            completed_circuit_id,
                            completed_leg,
                        },
                    )
                    .await;
                    return;
                }
                self.start_traffic_authentication(a1, node, call_id).await;
                self.continue_assignment_complete(a1, call_id, completed_circuit_id, completed_leg)
                    .await;
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
                if self.pending_otapa_calls.contains_key(&call_id) {
                    if let Some(pending) = self.pending_otasp_originations.remove(&call_id)
                        && let Some(otasp) = self.otasp.as_mut()
                    {
                        if let Err(error) = self
                            .controller
                            .apply_from_bsc(call_id, &cdma_ios::ProcedureMessage::Connect(connect))
                        {
                            warn!("MSC: OTAPA Connect state error: {error}");
                            self.send_clear_command(a1, call_id).await;
                            return;
                        }
                        otasp
                            .begin_session_with_initiation(
                                crate::otasp::session::SessionInitiation::Network,
                                pending.device,
                                pending.feature_code,
                                pending.service_option,
                                pending.mobile_identity_imsi,
                                pending.origination_verified,
                                call_id.0,
                                a1,
                            )
                            .await;
                    }
                    return;
                }
                if let Some(call) = self.circuits.m2m_calls.get(&call_id).copied() {
                    self.connect_m2m_call(a1, call, call_id).await;
                    return;
                }
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
                if self.circuits.m2m_calls.contains_key(&call_id) {
                    self.send_clear_command(a1, call_id).await;
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
                    .send(EncodedA1Message::from_message_for_call(
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
                if self.circuits.m2m_calls.contains_key(&call_id) {
                    self.send_clear_command(a1, call_id).await;
                    return;
                }
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
                a1.release_call(call_id);
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
                let requested_service_option = cm_service_request
                    .as_ref()
                    .and_then(|request| request.service_option)
                    .map(|service_option| service_option.0)
                    .unwrap_or(SERVICE_OPTION_BASIC_VOICE);
                let service_option = resolve_mo_service_option(requested_service_option);
                if requested_service_option == SERVICE_OPTION_BASIC_VOICE {
                    info!("MSC: default SO1 Origination; preserving legacy service option");
                }
                let called_number = cm_service_request
                    .as_ref()
                    .and_then(cm_service_request_called_number);
                let mut origination_verified = None;

                if let Some(request) = cm_service_request.as_ref() {
                    let imsi = match &request.mobile_identity_imsi {
                        cdma_ios::MobileIdentity::Imsi(imsi) => Some(
                            a1.serving_imsi_prefix(node)
                                .map(|(mcc, imsi_11_12)| {
                                    crate::base_station::complete_imsi(&mcc, &imsi_11_12, imsi)
                                })
                                .unwrap_or_else(|| imsi.clone()),
                        ),
                        _ => None,
                    };
                    let (esn, meid) = extract_hardware_identity(Some(request));
                    let identity = cdma_hlr::model::MobileIdentityKey::from_parts(
                        imsi.as_deref(),
                        esn,
                        meid.as_deref(),
                    )
                    .ok();
                    let dialed = cm_service_request_dialed_digits(request);
                    let subject = crate::authentication::AuthenticationSubject {
                        identity: identity.as_ref(),
                        imsi: imsi.as_deref(),
                        esn,
                        dialed_digits: dialed.as_deref(),
                    };
                    let otasp_origination = self.otasp.as_ref().is_some_and(|otasp| {
                        called_number
                            .as_deref()
                            .is_some_and(|dialed| otasp.is_otasp_origination(dialed))
                    });
                    let decision = self
                        .authenticate_access(
                            node,
                            crate::authentication::AuthenticationProcedure::Origination,
                            &subject,
                            &crate::authentication::GlobalChallengeIes::from_cm_service_request(
                                request,
                            ),
                            if otasp_origination {
                                AccessPurpose::Provisioning
                            } else {
                                AccessPurpose::Service
                            },
                        )
                        .await;
                    origination_verified = match decision.result {
                        crate::authentication::AuthenticationResult::NotRequested => None,
                        result => {
                            Some(result == crate::authentication::AuthenticationResult::Success)
                        }
                    };
                    if !decision.allow && !otasp_origination {
                        self.reject_access_for_authentication(a1, call_id).await;
                        return;
                    }
                    if decision.pending_ssd_update && !otasp_origination {
                        // A non-voice call never runs the traffic-channel procedures.
                        if is_non_voice_a1_service_option(service_option) {
                            self.reject_access_for_authentication(a1, call_id).await;
                            return;
                        }
                        self.pending_ssd_calls.insert(call_id);
                    }
                    if let Some(keys) = decision.call_keys {
                        self.call_keys.insert(call_id, keys);
                    }
                    if !otasp_origination
                        && let Some(context) = crate::authentication::CallAuthContext::from_subject(
                            &subject, &decision,
                        )
                    {
                        self.privacy_authenticated.remove(&call_id);
                        self.traffic_auth_started.remove(&call_id);
                        self.call_auth_context.insert(call_id, context);
                    }
                }

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
                            origination_verified,
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
                {
                    let imsi = cm_service_request.as_ref().and_then(|request| {
                        let cdma_ios::MobileIdentity::Imsi(imsi) = &request.mobile_identity_imsi
                        else {
                            return None;
                        };
                        Some(imsi.clone())
                    });
                    let (esn, meid) = extract_hardware_identity(cm_service_request.as_ref());
                    let slot_cycle_index = cm_service_request
                        .as_ref()
                        .and_then(|request| request.slot_cycle_index)
                        .map(|value| u32::from(value.0));
                    if let Ok(identity_key) = cdma_hlr::model::MobileIdentityKey::from_parts(
                        imsi.as_deref(),
                        esn,
                        meid.as_deref(),
                    ) && let Some(contact) = self
                        .record_mobile_contact(
                            node,
                            MobileContact {
                                identity_key: identity_key.clone(),
                                imsi,
                                esn,
                                meid,
                                slot_cycle_index,
                            },
                            "origination",
                        )
                        .await
                    {
                        self.send_welcome_sms_if_due(node, &identity_key, &contact)
                            .await;
                    }
                }
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
                        .assignment_circuit_identity_code_for_next_leg();
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
                        channel_type: assignment_channel_type(service_option),
                        circuit_identity_code: None,
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
                        .send(EncodedA1Message::from_message_for_call(
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

                let destination = if let Some(called_number) = called_number.as_deref() {
                    self.mo_call
                        .resolve_mo_destination(called_number, self.config.hlr_repo.as_ref())
                        .await
                } else {
                    None
                };
                let routes_to_subscriber = destination.is_some();

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
                    .assignment_circuit_identity_code_for_next_leg();
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

                let a2p_bearer_format_params = a2p_bearer_session_params.as_ref().and_then(|_| {
                    cdma_ios::A2pBearerFormatParams::for_service_option_with_telephone_event(
                        cdma_ios::ServiceOption(service_option),
                        cdma_ios::voice_bearer::VOICE_RTP_PAYLOAD_TYPE,
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
                            voice: format_params.voice_payload_type().unwrap_or(
                                cdma_ios::VoiceBearerPayloadType {
                                    format: cdma_ios::VoiceBearerFormat::Evrc,
                                    payload_type: cdma_ios::voice_bearer::EVRC_RTP_PAYLOAD_TYPE,
                                },
                            ),
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
                    circuit_identity_code: Some(cic),
                    encryption_information: self.assignment_privacy(
                        call_id,
                        cm_service_request
                            .as_ref()
                            .is_some_and(|request| request.voice_privacy_request),
                    ),
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
                    .send(EncodedA1Message::from_message_for_call(
                        &cdma_ios::Message::new(cdma_ios::MessageType::AssignmentRequest, payload),
                        Some(call_id_raw),
                    ))
                    .await
                {
                    warn!(
                        "MSC: failed to send A1 Assignment Request to BSC for MO call_id={}: {}",
                        call_id_raw, error
                    );
                    self.send_clear_command(a1, call_id).await;
                    return;
                }
                if let Some(destination) = destination {
                    if self.pending_ssd_calls.contains(&call_id) {
                        self.held_m2m_callees
                            .insert(call_id, (destination, calling_number));
                    } else {
                        self.start_m2m_callee(a1, call_id, destination, calling_number)
                            .await;
                    }
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
        let Some((call_id, peer_circuit_id, media_gateway_handle, service_option)) = self
            .circuits
            .circuits
            .get(&frame.circuit_id)
            .map(|session| {
                (
                    session.call_id,
                    session.peer_circuit_id,
                    session.media_gateway_handle,
                    session.service_option,
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
            if self.controller.state(call_id) != Some(cdma_ios::CallControlState::Connected) {
                log::trace!(
                    "MSC: dropping pre-connect bearer frame call_id={} from_circuit_id={} to_circuit_id={}",
                    call_id.0,
                    frame.circuit_id,
                    peer_cid
                );
                return;
            }
            let peer = self
                .circuits
                .circuits
                .get(&peer_cid)
                .map(|peer| (peer.bearer_remote_ready, peer.service_option));
            let Some((peer_ready, peer_service_option)) = peer else {
                warn!(
                    "MSC: dropping bearer frame call_id={} from_circuit_id={} because peer circuit_id={} is missing",
                    call_id.0, frame.circuit_id, peer_cid
                );
                return;
            };
            if !peer_ready {
                log::trace!(
                    "MSC: dropping bearer frame call_id={} from_circuit_id={} to_circuit_id={} until peer bearer remote is known",
                    call_id.0,
                    frame.circuit_id,
                    peer_cid
                );
                return;
            }
            let Some(source_codec) = cdma_voice::VoiceCodec::from_service_option(service_option)
            else {
                warn!(
                    "MSC: dropping bearer frame for unsupported source service option {}",
                    service_option
                );
                return;
            };
            let Some(target_codec) =
                cdma_voice::VoiceCodec::from_service_option(peer_service_option)
            else {
                warn!(
                    "MSC: dropping bearer frame for unsupported target service option {}",
                    peer_service_option
                );
                return;
            };
            let source_circuit_id = frame.circuit_id;
            let bridged_frame = match self.circuits.bridge_voice_frame(
                source_circuit_id,
                peer_cid,
                source_codec,
                target_codec,
                frame,
            ) {
                Ok(frame) => frame,
                Err(error) => {
                    warn!(
                        "MSC: voice transcode failed call_id={} from_circuit_id={} to_circuit_id={} {:?}->{:?}: {}",
                        call_id.0, source_circuit_id, peer_cid, source_codec, target_codec, error
                    );
                    return;
                }
            };
            debug!(
                "MSC: bridging bearer frame call_id={} from_circuit_id={} to_circuit_id={} {:?}->{:?} rate={} len={}",
                call_id.0,
                source_circuit_id,
                peer_cid,
                source_codec,
                target_codec,
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
            let Some(codec) = cdma_voice::VoiceCodec::from_service_option(service_option) else {
                warn!(
                    "MSC: dropping bearer frame for unsupported service option {}",
                    service_option
                );
                return;
            };
            let Some(rate) = codec.rate_from_bps(rate_bps) else {
                warn!(
                    "MSC: dropping bearer frame rate {} for {:?}",
                    rate_bps, codec
                );
                return;
            };
            let payload =
                cdma_voice::unpack_voice_bits(&frame.payload, codec.primary_traffic_bits(rate));
            if let Err(error) = media_gateway
                .forward_payload(handle, VocoderFrame { payload, rate_bps })
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
        let privacy = self.assignment_privacy(call_id, response.voice_privacy_request);
        self.mt_call
            .send_assignment_request_for_paging_response(
                a1,
                call_id,
                response.clone(),
                true,
                &mut self.controller,
                &mut self.circuits,
                &self.mo_call,
                self.config.voice_bearer.as_ref(),
                self.config.default_voice_service_option,
                &self.config.hlr_repo,
                privacy,
            )
            .await;
    }

    const MT_ASSIGNMENT_FAILURE_MAX_RETRIES: u8 = 3;

    async fn handle_assignment_failure(&mut self, a1: &dyn MscA1Endpoint, call_id: CallId) {
        if self.circuits.m2m_calls.contains_key(&call_id)
            && !self.circuits.has_pending_assignment_complete(call_id)
        {
            debug!(
                "MSC: ignoring unexpected M2M AssignmentFailure call_id={}",
                call_id.0
            );
            return;
        }
        let abandoned = self
            .circuits
            .cancel_pending_assignment_leg(call_id, self.config.voice_bearer.as_ref());
        let failed_leg = abandoned
            .map(|(leg, _)| leg)
            .unwrap_or(MscVoiceLeg::Primary);
        if failed_leg == MscVoiceLeg::Primary
            && self
                .controller
                .snapshot(call_id)
                .is_some_and(|snapshot| snapshot.direction == CallDirection::MobileOriginated)
        {
            info!(
                "MSC: A1 rx AssignmentFailure call_id={} abandoned MO primary leg; clearing call",
                call_id.0
            );
            self.circuits.reset_assignment_failure_retries(call_id);
            self.send_clear_command(a1, call_id).await;
            return;
        }
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

    async fn send_clear_command_for_leg(&mut self, a1: &dyn MscA1Endpoint, call_id: CallId) {
        let clear_command = cdma_ios::ClearCommandMessage {
            cause: cdma_ios::Cause(0x16),
            cause_layer3: None,
        };
        if let Err(error) = self.controller.apply_from_msc(
            call_id,
            &cdma_ios::ProcedureMessage::ClearCommand(clear_command.clone()),
        ) {
            debug!(
                "MSC: ClearCommand state transition skipped for call_id={}: {}",
                call_id.0, error
            );
        }
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
            .send(EncodedA1Message::from_message_for_call(
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
            a1.release_call(call_id);
        }
    }

    fn stop_media_for_call(&mut self, call_id: CallId) {
        self.pending_otapa_calls.remove(&call_id);
        self.pending_otasp_originations.remove(&call_id);
        self.call_keys.remove(&call_id);
        self.privacy_deadlines.remove(&call_id);
        self.pending_privacy.remove(&call_id);
        self.privacy_authenticated.remove(&call_id);
        self.traffic_auth_started.remove(&call_id);
        self.call_auth_context.remove(&call_id);
        self.traffic_auth.remove(&call_id);
        self.pending_ssd_calls.remove(&call_id);
        self.held_assignments.remove(&call_id);
        self.held_m2m_callees.remove(&call_id);
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
    async fn handle_adds_message(
        &mut self,
        a1: &dyn MscA1Endpoint,
        node: &BaseStationId,
        message: EncodedA1Message,
    ) {
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
                        self.release_otasp_call(a1, call_id_raw).await;
                    }
                    self.start_otasp_ssd_updates(a1).await;
                    return;
                }
                if msg.adds_user_part.burst_type == cdma_common::consts::BURST_TYPE_SMS
                    && !self.authenticate_adds_transfer(a1, node, &msg).await
                {
                    return;
                }
                if let Some(contact) = adds_transfer_contact(a1, node, &msg) {
                    let identity_key = contact.identity_key.clone();
                    if let Some(recorded) = self.record_mobile_contact(node, contact, "ADDS").await
                    {
                        self.send_welcome_sms_if_due(node, &identity_key, &recorded)
                            .await;
                    }
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
                        if let (Some(t), Some(otasp)) = (msg.tag, self.otasp.as_mut())
                            && otasp.owns_ack_tag(t.0)
                        {
                            otasp.handle_adds_deliver_ack(&msg, a1).await;
                            self.start_otasp_ssd_updates(a1).await;
                            return;
                        }
                        smsc.handle_adds_deliver_ack(&msg).await;
                    }
                    Err(e) => warn!("MSC: failed to decode ADDS Deliver Ack: {e}"),
                }
            }
            cdma_ios::MessageType::AddsTransfer => {
                match cdma_ios::AddsTransferMessage::decode(&decoded.payload) {
                    Ok(mut msg) => {
                        if let cdma_ios::MobileIdentity::Imsi(imsi) = &msg.mobile_identity_imsi {
                            let completed = a1
                                .serving_imsi_prefix(node)
                                .map(|(mcc, imsi_11_12)| {
                                    crate::base_station::complete_imsi(&mcc, &imsi_11_12, imsi)
                                })
                                .unwrap_or_else(|| imsi.clone());
                            msg.mobile_identity_imsi = cdma_ios::MobileIdentity::Imsi(completed);
                        }
                        let ack = cdma_ios::AddsTransferAckMessage {
                            mobile_identity_imsi: msg.mobile_identity_imsi.clone(),
                            tag: msg.tag,
                            cause: None,
                        };
                        match ack.encode() {
                            Ok(payload) => {
                                let message = cdma_ios::Message::new(
                                    cdma_ios::MessageType::AddsTransferAck,
                                    payload,
                                );
                                if let Err(error) = a1
                                    .send_to_node(
                                        node,
                                        EncodedA1Message::from_message_for_call(&message, None),
                                    )
                                    .await
                                {
                                    warn!("MSC: failed to send ADDS Transfer Ack: {error}");
                                } else {
                                    info!("MSC: A1 tx ADDS Transfer Ack");
                                }
                            }
                            Err(error) => warn!("MSC: failed to encode ADDS Transfer Ack: {error}"),
                        }
                        smsc.handle_adds_transfer(&msg, node).await;
                    }
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
                        smsc.handle_adds_deliver_mo(&msg, &mobile_identity, mobile_identity_esn)
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

    async fn handle_registration_notification(
        &mut self,
        a1: &dyn MscA1Endpoint,
        node: &BaseStationId,
        message: EncodedA1Message,
    ) {
        let transport_call_id = message.call_id();
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
        // Complete an abbreviated IMSI_S using the serving base station's
        // broadcast MCC/IMSI_11_12. A BSC already sends the full IMSI, so this
        // is a no-op there and matters for a femto peer that sends only IMSI_S.
        let completed_imsi = match &lur.mobile_identity_imsi {
            cdma_ios::MobileIdentity::Imsi(s) if s != "UNKNOWN" => Some(
                a1.serving_imsi_prefix(node)
                    .map(|(mcc, i1112)| crate::base_station::complete_imsi(&mcc, &i1112, s))
                    .unwrap_or_else(|| s.clone()),
            ),
            _ => None,
        };
        let imsi = completed_imsi.as_deref();
        let esn = match &lur.mobile_identity_esn {
            Some(cdma_ios::MobileIdentity::Esn(e)) => Some(*e),
            _ => None,
        };
        let meid = lur.mobile_identity_meid.as_ref().and_then(meid_hex);
        // A rejected registration provides a provisioning route, not a service binding.
        if let Some(imsi) = imsi {
            self.otapa_routes.insert(
                crate::network_otasp::ProvisioningIdentity::from_parts(imsi, esn, meid.as_deref()),
                crate::network_otasp::ProvisioningRoute {
                    node: node.clone(),
                    imsi: imsi.to_string(),
                    meid: meid.clone(),
                    slot_cycle_index: lur.slot_cycle_index.map(|value| value.0),
                },
            );
        }
        let identity_key =
            match cdma_hlr::model::MobileIdentityKey::from_parts(imsi, esn, meid.as_deref()) {
                Ok(identity_key) => identity_key,
                Err(error) => {
                    warn!("MSC: registration notification has no complete identity: {error}");
                    return;
                }
            };
        let subject = crate::authentication::AuthenticationSubject {
            identity: Some(&identity_key),
            imsi,
            esn,
            dialed_digits: None,
        };
        let decision = self
            .authenticate_access(
                node,
                crate::authentication::AuthenticationProcedure::Registration,
                &subject,
                &crate::authentication::GlobalChallengeIes::from_location_updating_request(&lur),
                AccessPurpose::Service,
            )
            .await;
        if !decision.allow {
            let reject = cdma_ios::LocationUpdatingRejectMessage {
                reject_cause: cdma_ios::RejectCause(REJECT_CAUSE_ILLEGAL_MS),
            };
            match reject.encode() {
                Ok(payload) => {
                    if let Err(error) = a1
                        .send_to_node(
                            node,
                            EncodedA1Message::from_message_for_call(
                                &cdma_ios::Message::new(
                                    cdma_ios::MessageType::LocationUpdatingReject,
                                    payload,
                                ),
                                transport_call_id,
                            ),
                        )
                        .await
                    {
                        warn!("MSC: failed to send Location Updating Reject: {error}");
                    }
                }
                Err(error) => warn!("MSC: failed to encode Location Updating Reject: {error}"),
            }
            return;
        }
        let Some(contact) = self
            .record_mobile_contact(
                node,
                MobileContact {
                    identity_key: identity_key.clone(),
                    imsi: imsi.map(str::to_string),
                    esn,
                    meid: meid.clone(),
                    slot_cycle_index: lur.slot_cycle_index.map(|value| u32::from(value.0)),
                },
                "registration",
            )
            .await
        else {
            return;
        };
        let accept = cdma_ios::LocationUpdatingAcceptMessage {
            location_area_identification: lur.location_area_identification,
        };
        let payload = match accept.encode() {
            Ok(payload) => payload,
            Err(error) => {
                warn!("MSC: failed to encode Location Updating Accept: {error}");
                return;
            }
        };
        if let Err(error) = a1
            .send_to_node(
                node,
                EncodedA1Message::from_message_for_call(
                    &cdma_ios::Message::new(cdma_ios::MessageType::LocationUpdatingAccept, payload),
                    transport_call_id,
                ),
            )
            .await
        {
            warn!("MSC: failed to send Location Updating Accept: {error}");
            return;
        }
        info!(
            "MSC: registration accepted for IMSI {}",
            identity_key.imsi()
        );

        self.send_welcome_sms_if_due(node, &identity_key, &contact)
            .await;
    }

    /// Nothing can clear them with the node, so the MSC releases its own
    /// side and lets the node release the air side when it comes back.
    async fn handle_base_station_detached(
        &mut self,
        a1: &dyn MscA1Endpoint,
        node: &BaseStationId,
        calls: Vec<CallId>,
    ) {
        self.otapa_routes.retain(|_, route| &route.node != node);
        warn!(
            "MSC: base station {node} detached, releasing {} call(s)",
            calls.len()
        );
        if let Some(smsc) = self.smsc.as_mut() {
            smsc.remove_serving_node(node);
        }
        for call_id in calls {
            if self.circuits.m2m_calls.contains_key(&call_id) {
                self.send_clear_command(a1, call_id).await;
            }
            self.mt_page_retry.cancel(call_id);
            self.controller.remove_call(call_id);
            self.stop_media_for_call(call_id);
            a1.release_call(call_id);
        }
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
    let meid = req.mobile_identity_meid.as_ref().and_then(meid_hex);
    (esn, meid)
}

/// IMSI (completed from the serving base station's prefix), ESN and MEID of an ADDS Transfer.
fn adds_transfer_identity(
    a1: &dyn MscA1Endpoint,
    node: &BaseStationId,
    msg: &cdma_ios::AddsTransferMessage,
) -> (Option<String>, Option<u32>, Option<String>) {
    let imsi = match &msg.mobile_identity_imsi {
        cdma_ios::MobileIdentity::Imsi(imsi) => Some(
            a1.serving_imsi_prefix(node)
                .map(|(mcc, imsi_11_12)| {
                    crate::base_station::complete_imsi(&mcc, &imsi_11_12, imsi)
                })
                .unwrap_or_else(|| imsi.clone()),
        ),
        _ => None,
    };
    let esn = match msg.mobile_identity_esn.as_ref() {
        Some(cdma_ios::MobileIdentity::Esn(esn)) => Some(*esn),
        _ => None,
    };
    let meid = msg.mobile_identity_meid.as_ref().and_then(meid_hex);
    (imsi, esn, meid)
}

fn adds_transfer_contact(
    a1: &dyn MscA1Endpoint,
    node: &BaseStationId,
    msg: &cdma_ios::AddsTransferMessage,
) -> Option<MobileContact> {
    let (imsi, esn, meid) = adds_transfer_identity(a1, node, msg);
    let identity_key =
        cdma_hlr::model::MobileIdentityKey::from_parts(imsi.as_deref(), esn, meid.as_deref())
            .ok()?;
    Some(MobileContact {
        identity_key,
        imsi,
        esn,
        meid,
        slot_cycle_index: None,
    })
}

struct MobileContact {
    identity_key: cdma_hlr::model::MobileIdentityKey,
    imsi: Option<String>,
    esn: Option<u32>,
    meid: Option<String>,
    slot_cycle_index: Option<u32>,
}

struct RecordedContact {
    upsert: cdma_hlr::model::MobileSeenUpsert,
    subscriber: Result<Option<cdma_hlr::model::ResolvedSubscriber>, ()>,
}

pub(crate) fn meid_hex(identity: &cdma_ios::MobileIdentity) -> Option<String> {
    let cdma_ios::MobileIdentity::Meid(bytes) = identity else {
        return None;
    };
    Some(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
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

fn cm_service_request_dialed_digits(request: &cdma_ios::CmServiceRequestMessage) -> Option<String> {
    decode_called_party_bcd_number(request.called_party_bcd_number.as_ref()).or_else(|| {
        request
            .called_party_ascii_number
            .as_ref()
            .and_then(|number| String::from_utf8(number.0.clone()).ok())
    })
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
    mod m2m;
    mod network_otasp;
    mod privacy;
    mod sms_routes;

    use super::*;
    use crate::circuit::{MscLegKey, MscVoiceLeg};
    use crate::media_gateway::{CallHandle, MediaGatewayEvent, VocoderFrame};
    use cdma_bsc_a1_edge_compat::test_node;
    use cdma_ios::{
        AssignmentCompleteMessage, AssignmentRequestMessage, Cause, ChannelNumber, ChannelType,
        CircuitIdentityCode, CompleteLayer3InformationMessage, ConnectMessage, Layer3Information,
        LocationUpdatingRequestMessage, MobileIdentity, PagingRequestMessage,
        PagingResponseMessage, ProcedureMessage, RegistrationType, ServiceOption, SlotCycleIndex,
        Tag,
    };
    use tokio::time::{Duration, timeout};

    struct StubHlrRepo {
        credentials: std::sync::Mutex<cdma_hlr::auth::CaveCredentials>,
        seen: std::sync::Mutex<usize>,
    }

    impl Default for StubHlrRepo {
        fn default() -> Self {
            Self {
                credentials: std::sync::Mutex::new(cdma_hlr::auth::CaveCredentials {
                    auth_algo: cdma_hlr::model::AuthAlgorithm::Cave,
                    a_key: Some(STUB_A_KEY),
                    ssd: Some(STUB_AUTH_SSD),
                    ..Default::default()
                }),
                seen: std::sync::Mutex::new(0),
            }
        }
    }

    impl StubHlrRepo {
        fn credentials(&self) -> cdma_hlr::auth::CaveCredentials {
            self.credentials.lock().unwrap().clone()
        }
    }

    #[test]
    fn default_so1_origination_preserves_legacy_service_option() {
        assert_eq!(resolve_mo_service_option(SERVICE_OPTION_BASIC_VOICE), 1);
        assert_eq!(resolve_mo_service_option(32768), 32768);
    }

    #[test]
    fn so12_is_non_voice_packet_transport() {
        assert!(is_packet_data_service_option(SERVICE_OPTION_ASYNC_DATA));
        assert!(is_non_voice_a1_service_option(SERVICE_OPTION_ASYNC_DATA));
    }

    #[test]
    fn packet_assignment_uses_data_channel_type() {
        let packet = assignment_channel_type(SERVICE_OPTION_PACKET_DATA_RC3);
        assert_eq!(
            packet.speech_or_data_indicator,
            ChannelType::SPEECH_OR_DATA_DATA
        );
        assert_eq!(packet.coding, ChannelType::CODING_DATA_NON_TRANSPARENT);
        let voice = assignment_channel_type(SERVICE_OPTION_BASIC_VOICE);
        assert_eq!(
            voice.speech_or_data_indicator,
            ChannelType::SPEECH_OR_DATA_SPEECH
        );
        assert_eq!(voice.coding, ChannelType::CODING_QCELP_13K);
    }

    #[tokio::test]
    async fn completed_packet_data_context_releases_its_circuit() {
        let (_client, endpoint) = cdma_bsc_a1_edge_compat::InProcessMscClient::pair(4);
        let mut runtime = MscRuntime::new(MscRuntimeConfig {
            hlr_repo: Arc::new(StubHlrRepo::default()),
            smsc_repo: None,
            welcome_sms: None,
            sms_retry: crate::config::SmsRetryConfig::default(),
            default_voice_service_option: 3,
            supported_voice_service_options: vec![3],
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
            authentication: Default::default(),
        });
        let call_id = CallId(42);
        runtime.controller.create_call_with_id(
            call_id,
            CallDirection::MobileOriginated,
            Some(MobileIdentity::Imsi("12345678901".to_string())),
        );
        runtime.circuits.insert_circuit_session(
            7,
            CircuitSession {
                call_id,
                audio_file: None,
                service_option: SERVICE_OPTION_PACKET_DATA_RC3,
                leg_role: MscVoiceLeg::Primary,
                peer_circuit_id: None,
                bearer_remote_ready: true,
                media_gateway_handle: None,
                called_number: None,
            },
        );
        runtime
            .circuits
            .queue_assignment_complete_circuit(call_id, MscVoiceLeg::Primary, 7);
        let complete = AssignmentCompleteMessage {
            channel_number: ChannelNumber(50),
            encryption_information: None,
            service_option: Some(ServiceOption::HIGH_RATE_PACKET_DATA),
            a2p_bearer_session_params: None,
            a2p_bearer_format_params: None,
        };

        runtime
            .handle_bsc_a1_message(
                &endpoint,
                &test_node(),
                EncodedA1Message::from_message_for_call(
                    &cdma_ios::Message::new(
                        cdma_ios::MessageType::AssignmentComplete,
                        complete.encode().unwrap(),
                    ),
                    Some(call_id.0),
                ),
            )
            .await;

        assert_eq!(runtime.controller.active_call_count(), 0);
        assert!(runtime.circuits.circuits.is_empty());
        assert!(!runtime.circuits.has_pending_assignment_complete(call_id));
    }

    /// HLR stub that resolves a single phone number to a Registered subscriber
    /// with one primary IMSI for MO M2M paging.
    struct M2mHlrRepo {
        phone_number: &'static str,
        subscriber_id: uuid::Uuid,
        imsi: &'static str,
        esn: Option<u32>,
        meid: Option<&'static str>,
        strict_hardware_lookup: bool,
        mob_p_rev: u32,
        refreshed_binding: std::sync::Mutex<Option<RegistrationBinding>>,
        registration_state: cdma_hlr::model::RegistrationState,
    }

    impl M2mHlrRepo {
        fn new() -> Self {
            Self::with_p_rev(6)
        }

        fn with_p_rev(mob_p_rev: u32) -> Self {
            Self {
                phone_number: "5559876543",
                subscriber_id: uuid::Uuid::from_u128(0x1111_2222_3333_4444_5555_6666_7777_8888),
                imsi: "111111111111111",
                esn: None,
                meid: None,
                strict_hardware_lookup: false,
                mob_p_rev,
                refreshed_binding: std::sync::Mutex::new(None),
                registration_state: cdma_hlr::model::RegistrationState::Registered,
            }
        }
    }

    #[tokio::test]
    async fn subscriber_uplink_creates_registration_binding() {
        let repo = M2mHlrRepo::new();

        MscRuntime::refresh_registration_binding(
            &repo,
            &BaseStationId("femto-fcs-1".to_string()),
            repo.subscriber_id,
            Some("310005551239999".to_string()),
            Some(0x80ab_cdef),
            Some("53a18cc200000a".to_string()),
            Some(2),
        )
        .await;

        let binding = repo.refreshed_binding.lock().unwrap().clone().unwrap();
        assert_eq!(binding.subscriber_id, repo.subscriber_id);
        assert_eq!(binding.serving_bs_id, "femto-fcs-1");
        assert_eq!(
            binding.state,
            cdma_hlr::model::RegistrationState::Registered
        );
        assert_eq!(binding.imsi.as_deref(), Some("310005551239999"));
        assert_eq!(binding.esn, Some(0x80ab_cdef));
        assert_eq!(binding.meid.as_deref(), Some("53a18cc200000a"));
        assert_eq!(binding.slot_cycle_index, Some(2));
    }

    #[tokio::test]
    async fn correlated_location_update_returns_location_updating_accept() {
        let (client, endpoint) = cdma_bsc_a1_edge_compat::InProcessMscClient::pair(4);
        let mut runtime = MscRuntime::new(MscRuntimeConfig {
            hlr_repo: Arc::new(StubHlrRepo::default()),
            smsc_repo: None,
            welcome_sms: None,
            sms_retry: crate::config::SmsRetryConfig::default(),
            default_voice_service_option: 3,
            supported_voice_service_options: vec![3],
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
            authentication: Default::default(),
        });
        let transport_call_id = 73;
        let request = LocationUpdatingRequestMessage {
            mobile_identity_imsi: MobileIdentity::Imsi("310005551239999".to_string()),
            location_area_identification: None,
            classmark_information_type_2: None,
            registration_type: Some(RegistrationType(1)),
            mobile_identity_esn: Some(MobileIdentity::Esn(0x80ab_cdef)),
            mobile_identity_meid: None,
            slot_cycle_index: Some(SlotCycleIndex(2)),
            authentication_response_parameter: None,
            authentication_confirmation_parameter: None,
            authentication_parameter_count: None,
            authentication_challenge_parameter: None,
            authentication_event: None,
            user_zone_id: None,
            is2000_mobile_capabilities: None,
        };
        let cli3 = CompleteLayer3InformationMessage {
            cell_identifier: cdma_ios::CellId {
                cell: 0x123,
                sector: 0,
            },
            layer3_information: Layer3Information::from_location_updating_request(&request)
                .unwrap(),
        };
        let message = EncodedA1Message::from_message_for_call(
            &cdma_ios::Message::new(
                cdma_ios::MessageType::CompleteLayer3Information,
                cli3.encode().unwrap(),
            ),
            Some(transport_call_id),
        );

        runtime
            .handle_bsc_a1_message(&endpoint, &test_node(), message)
            .await;

        let outbound = timeout(Duration::from_millis(50), client.poll_a1())
            .await
            .expect("Location Updating Accept should be sent")
            .unwrap();
        assert_eq!(outbound.call_id(), Some(transport_call_id));
        assert_eq!(
            outbound.message_type(),
            cdma_ios::MessageType::LocationUpdatingAccept
        );
        assert_eq!(runtime.controller.active_call_count(), 0);
    }

    const AUTH_TEST_IMSI: &str = "310005551239999";
    const AUTH_TEST_ESN: u32 = 0x80ab_cdef;
    const AUTH_TEST_RAND: u32 = 0x34A2_B05F;

    fn authenticating_runtime(mode: crate::authentication::AuthenticationMode) -> MscRuntime {
        authenticating_runtime_with_hlr(mode, Arc::new(StubHlrRepo::default()))
    }

    fn authenticating_runtime_with_hlr(
        mode: crate::authentication::AuthenticationMode,
        hlr_repo: Arc<StubHlrRepo>,
    ) -> MscRuntime {
        MscRuntime::new(MscRuntimeConfig {
            hlr_repo,
            smsc_repo: None,
            welcome_sms: None,
            sms_retry: crate::config::SmsRetryConfig::default(),
            default_voice_service_option: 3,
            supported_voice_service_options: vec![3],
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
            authentication: crate::authentication::AuthenticationConfig {
                mode,
                ..Default::default()
            },
        })
    }

    /// A Location Updating Request carrying the global-challenge IEs. `authr_flip`
    /// is XORed into the correct AUTHR. `None` sends only Authentication Event 01H.
    fn authenticated_registration(authr_flip: Option<u32>) -> EncodedA1Message {
        let (s1, _) = cdma_common::paging::imsi_s_from_imsi(AUTH_TEST_IMSI).unwrap();
        let authr = cdma_cave::auth_signature(
            AUTH_TEST_RAND,
            s1,
            &STUB_AUTH_SSD.ssd_a,
            cdma_cave::Esn(AUTH_TEST_ESN),
        )
        .unwrap()
        .signature;
        let with_auth = authr_flip.is_some();
        let request = LocationUpdatingRequestMessage {
            mobile_identity_imsi: MobileIdentity::Imsi(AUTH_TEST_IMSI.to_string()),
            location_area_identification: None,
            classmark_information_type_2: None,
            registration_type: Some(RegistrationType(1)),
            mobile_identity_esn: Some(MobileIdentity::Esn(AUTH_TEST_ESN)),
            mobile_identity_meid: None,
            slot_cycle_index: Some(SlotCycleIndex(2)),
            authentication_response_parameter: authr_flip.map(|flip| {
                cdma_ios::AuthenticationResponseParameter::new(
                    cdma_ios::AuthSignatureType::Authr,
                    authr ^ flip,
                )
            }),
            authentication_confirmation_parameter: with_auth.then_some(
                cdma_ios::AuthenticationConfirmationParameter((AUTH_TEST_RAND >> 24) as u8),
            ),
            authentication_parameter_count: with_auth
                .then_some(cdma_ios::AuthenticationParameterCount(0)),
            authentication_challenge_parameter: with_auth.then_some(
                cdma_ios::AuthenticationChallengeParameter::new(
                    cdma_ios::RandomNumberType::Rand,
                    u64::from(AUTH_TEST_RAND),
                ),
            ),
            authentication_event: (!with_auth).then_some(cdma_ios::AuthenticationEvent(
                cdma_ios::AUTHENTICATION_EVENT_PARAMETERS_NOT_RECEIVED,
            )),
            user_zone_id: None,
            is2000_mobile_capabilities: None,
        };
        let cli3 = CompleteLayer3InformationMessage {
            cell_identifier: cdma_ios::CellId {
                cell: 0x123,
                sector: 0,
            },
            layer3_information: Layer3Information::from_location_updating_request(&request)
                .unwrap(),
        };
        EncodedA1Message::from_message_for_call(
            &cdma_ios::Message::new(
                cdma_ios::MessageType::CompleteLayer3Information,
                cli3.encode().unwrap(),
            ),
            Some(91),
        )
    }

    async fn registration_reply(
        mode: crate::authentication::AuthenticationMode,
        authr_flip: Option<u32>,
    ) -> (
        cdma_ios::MessageType,
        Vec<crate::grpc::events_proto::v1::MscNetworkEvent>,
    ) {
        registration_reply_with_hlr(mode, authr_flip, Arc::new(StubHlrRepo::default())).await
    }

    async fn registration_reply_with_hlr(
        mode: crate::authentication::AuthenticationMode,
        authr_flip: Option<u32>,
        hlr: Arc<StubHlrRepo>,
    ) -> (
        cdma_ios::MessageType,
        Vec<crate::grpc::events_proto::v1::MscNetworkEvent>,
    ) {
        let (client, endpoint) = cdma_bsc_a1_edge_compat::InProcessMscClient::pair(4);
        let mut runtime = authenticating_runtime_with_hlr(mode, hlr);
        let mut events = runtime.otasp_event_tx.subscribe();
        runtime
            .handle_bsc_a1_message(
                &endpoint,
                &test_node(),
                authenticated_registration(authr_flip),
            )
            .await;
        let outbound = timeout(Duration::from_millis(50), client.poll_a1())
            .await
            .expect("a Location Updating result should be sent")
            .unwrap();
        assert_eq!(outbound.call_id(), Some(91));
        let mut seen = Vec::new();
        while let Ok(event) = events.try_recv() {
            seen.push(event);
        }
        (outbound.message_type(), seen)
    }

    fn authentication_results(
        events: &[crate::grpc::events_proto::v1::MscNetworkEvent],
    ) -> Vec<(i32, bool)> {
        use crate::grpc::events_proto::v1::msc_network_event::Event;
        events
            .iter()
            .filter_map(|event| match &event.event {
                Some(Event::Authentication(auth)) => Some((auth.result, auth.service_rejected)),
                _ => None,
            })
            .collect()
    }

    const AUTH_TEST_SMS_DESTINATION: &str = "18005551203";

    /// An access-channel SMS ADDS Transfer signed over its destination address
    /// (C.S0015-B §3.3.3.1). `authr_flip` is XORed into the correct AUTHR.
    fn authenticated_sms(authr_flip: u32) -> EncodedA1Message {
        let (s1, _) = cdma_common::paging::imsi_s_from_imsi(AUTH_TEST_IMSI).unwrap();
        signed_sms(AUTH_TEST_SMS_DESTINATION, s1, authr_flip)
    }

    fn signed_sms(destination: &str, fill: u32, authr_flip: u32) -> EncodedA1Message {
        let auth_data = cdma_cave::AuthData::Dialed {
            imsi_s1: fill,
            digits: destination.to_string(),
        }
        .to_u24()
        .unwrap();
        let authr = cdma_cave::auth_signature(
            AUTH_TEST_RAND,
            auth_data,
            &STUB_AUTH_SSD.ssd_a,
            cdma_cave::Esn(AUTH_TEST_ESN),
        )
        .unwrap()
        .signature;
        let transfer = cdma_ios::AddsTransferMessage {
            mobile_identity_imsi: MobileIdentity::Imsi(AUTH_TEST_IMSI.to_string()),
            adds_user_part: cdma_ios::AddsUserPart {
                burst_type: cdma_common::consts::BURST_TYPE_SMS,
                data: cdma_common::sms::encode_mo_sms_submit(destination, "hi", 1),
            },
            mobile_identity_esn: Some(MobileIdentity::Esn(AUTH_TEST_ESN)),
            mobile_identity_meid: None,
            authentication_response_parameter: Some(
                cdma_ios::AuthenticationResponseParameter::new(
                    cdma_ios::AuthSignatureType::Authr,
                    authr ^ authr_flip,
                ),
            ),
            authentication_confirmation_parameter: Some(
                cdma_ios::AuthenticationConfirmationParameter((AUTH_TEST_RAND >> 24) as u8),
            ),
            authentication_parameter_count: Some(cdma_ios::AuthenticationParameterCount(0)),
            authentication_challenge_parameter: Some(
                cdma_ios::AuthenticationChallengeParameter::new(
                    cdma_ios::RandomNumberType::Rand,
                    u64::from(AUTH_TEST_RAND),
                ),
            ),
            authentication_event: None,
            authentication_data: None,
            cell_identifier: None,
            tag: None,
            service_option: None,
        };
        EncodedA1Message::from_message(&cdma_ios::Message::new(
            cdma_ios::MessageType::AddsTransfer,
            transfer.encode().unwrap(),
        ))
    }

    async fn sms_reply(
        authr_flip: u32,
    ) -> (
        Option<EncodedA1Message>,
        Vec<crate::grpc::events_proto::v1::MscNetworkEvent>,
    ) {
        sms_reply_to(authenticated_sms(authr_flip)).await
    }

    async fn sms_reply_to(
        message: EncodedA1Message,
    ) -> (
        Option<EncodedA1Message>,
        Vec<crate::grpc::events_proto::v1::MscNetworkEvent>,
    ) {
        sms_reply_to_with_hlr(message, Arc::new(StubHlrRepo::default())).await
    }

    async fn sms_reply_to_with_hlr(
        message: EncodedA1Message,
        hlr: Arc<StubHlrRepo>,
    ) -> (
        Option<EncodedA1Message>,
        Vec<crate::grpc::events_proto::v1::MscNetworkEvent>,
    ) {
        let (client, endpoint) = cdma_bsc_a1_edge_compat::InProcessMscClient::pair(4);
        let mut runtime = authenticating_runtime_with_hlr(
            crate::authentication::AuthenticationMode::Enforce,
            hlr,
        );
        let mut events = runtime.otasp_event_tx.subscribe();
        runtime
            .handle_bsc_a1_message(&endpoint, &test_node(), message)
            .await;
        let outbound = timeout(Duration::from_millis(50), client.poll_a1())
            .await
            .ok()
            .flatten();
        let mut seen = Vec::new();
        while let Ok(event) = events.try_recv() {
            seen.push(event);
        }
        (outbound, seen)
    }

    #[tokio::test]
    async fn short_destination_signed_with_zero_fill_is_accepted() {
        use crate::grpc::events_proto::v1::AuthenticationResult;
        let (reply, events) = sms_reply_to(signed_sms("222", 0, 0)).await;
        assert!(reply.is_none(), "no rejection is sent");
        assert_eq!(
            authentication_results(&events),
            vec![(AuthenticationResult::Success as i32, false)]
        );
        let (s1, _) = cdma_common::paging::imsi_s_from_imsi(AUTH_TEST_IMSI).unwrap();
        let (reply, _) = sms_reply_to(signed_sms("222", s1, 0)).await;
        assert!(
            reply.is_none(),
            "the IMSI_S1 fill of the spec is still accepted"
        );
        let (reply, _) = sms_reply_to(signed_sms("222", 0, 1)).await;
        assert!(reply.is_some(), "a wrong AUTHR is still rejected");
    }

    #[tokio::test]
    async fn only_an_authenticated_sms_records_the_mobile() {
        let hlr = Arc::new(StubHlrRepo::default());
        sms_reply_to_with_hlr(authenticated_sms(1), hlr.clone()).await;
        assert_eq!(*hlr.seen.lock().unwrap(), 0);
        sms_reply_to_with_hlr(authenticated_sms(0), hlr.clone()).await;
        assert_eq!(*hlr.seen.lock().unwrap(), 1);
    }

    #[tokio::test]
    async fn enforced_sms_with_valid_authr_is_delivered() {
        use crate::grpc::events_proto::v1::AuthenticationResult;
        let (reply, events) = sms_reply(0).await;
        assert!(reply.is_none(), "no rejection is sent for a valid SMS");
        assert_eq!(
            authentication_results(&events),
            vec![(AuthenticationResult::Success as i32, false)]
        );
    }

    #[tokio::test]
    async fn enforced_sms_with_wrong_authr_is_rejected_with_authentication_failure() {
        use crate::grpc::events_proto::v1::AuthenticationResult;
        let (reply, events) = sms_reply(1).await;
        let reply = reply.expect("an ADDS Transfer Ack rejects the SMS");
        assert_eq!(reply.message_type(), cdma_ios::MessageType::AddsTransferAck);
        let ack =
            cdma_ios::AddsTransferAckMessage::decode(&reply.decode().unwrap().payload).unwrap();
        assert_eq!(
            ack.cause,
            Some(cdma_ios::Cause(cdma_ios::CAUSE_AUTHENTICATION_FAILURE))
        );
        assert_eq!(
            authentication_results(&events),
            vec![(AuthenticationResult::SignatureMismatch as i32, true)]
        );
    }

    #[tokio::test]
    async fn enforced_registration_with_valid_authr_is_accepted() {
        use crate::grpc::events_proto::v1::AuthenticationResult;
        let (reply, events) =
            registration_reply(crate::authentication::AuthenticationMode::Enforce, Some(0)).await;
        assert_eq!(reply, cdma_ios::MessageType::LocationUpdatingAccept);
        assert_eq!(
            authentication_results(&events),
            vec![(AuthenticationResult::Success as i32, false)]
        );
    }

    #[tokio::test]
    async fn enforced_registration_with_wrong_authr_is_rejected() {
        use crate::grpc::events_proto::v1::AuthenticationResult;
        let (reply, events) =
            registration_reply(crate::authentication::AuthenticationMode::Enforce, Some(1)).await;
        assert_eq!(reply, cdma_ios::MessageType::LocationUpdatingReject);
        assert_eq!(
            authentication_results(&events),
            vec![(AuthenticationResult::SignatureMismatch as i32, true)]
        );
    }

    fn pending_ssd_updates(events: &[crate::grpc::events_proto::v1::MscNetworkEvent]) -> Vec<bool> {
        use crate::grpc::events_proto::v1::msc_network_event::Event;
        events
            .iter()
            .filter_map(|event| match &event.event {
                Some(Event::Authentication(auth)) => Some(auth.pending_ssd_update),
                _ => None,
            })
            .collect()
    }

    #[tokio::test]
    async fn enforced_registration_with_wrong_authr_is_accepted_while_the_ssd_is_unset() {
        use crate::grpc::events_proto::v1::AuthenticationResult;
        let hlr = Arc::new(StubHlrRepo::default());
        hlr.credentials.lock().unwrap().ssd = None;
        let (reply, events) = registration_reply_with_hlr(
            crate::authentication::AuthenticationMode::Enforce,
            Some(1),
            hlr.clone(),
        )
        .await;
        assert_eq!(reply, cdma_ios::MessageType::LocationUpdatingAccept);
        assert_eq!(
            authentication_results(&events),
            vec![(AuthenticationResult::SignatureMismatch as i32, false)]
        );
        assert_eq!(pending_ssd_updates(&events), vec![true]);

        hlr.credentials.lock().unwrap().a_key = None;
        let (reply, _) = registration_reply_with_hlr(
            crate::authentication::AuthenticationMode::Enforce,
            Some(1),
            hlr.clone(),
        )
        .await;
        assert_eq!(
            reply,
            cdma_ios::MessageType::LocationUpdatingReject,
            "without an A-key no SSD update can follow"
        );
    }

    #[tokio::test]
    async fn rejected_registration_reports_no_pending_ssd_update() {
        let (_, events) =
            registration_reply(crate::authentication::AuthenticationMode::Enforce, Some(1)).await;
        assert_eq!(pending_ssd_updates(&events), vec![false]);
    }

    async fn held_call_after(
        action: crate::authentication::TrafficAction,
    ) -> (MscRuntime, Option<cdma_ios::MessageType>) {
        let (client, endpoint) = cdma_bsc_a1_edge_compat::InProcessMscClient::pair(4);
        let mut runtime =
            authenticating_runtime(crate::authentication::AuthenticationMode::Enforce);
        let call_id = CallId(77);
        let config = runtime.config.authentication.clone();
        let mut context = traffic_test_context();
        context.global_result = crate::authentication::AuthenticationResult::SignatureMismatch;
        let procedures = crate::authentication::TrafficAuthentication::plan(
            &config,
            context,
            "bs".into(),
            false,
        )
        .unwrap();
        runtime.traffic_auth.insert(call_id, procedures);
        runtime.held_assignments.insert(
            call_id,
            HeldAssignment {
                completed_circuit_id: None,
                completed_leg: None,
            },
        );
        runtime
            .apply_traffic_action(&endpoint, call_id, action)
            .await;
        let sent = timeout(Duration::from_millis(50), client.poll_a1())
            .await
            .ok()
            .flatten()
            .map(|message| message.message_type());
        (runtime, sent)
    }

    #[tokio::test]
    async fn a_held_call_is_cleared_when_its_ssd_update_fails() {
        use crate::grpc::events_proto::v1::{AuthenticationProcedure, AuthenticationResult};
        let (runtime, sent) = held_call_after(crate::authentication::TrafficAction::Report {
            procedure: AuthenticationProcedure::SsdUpdate,
            result: AuthenticationResult::SsdUpdateRejected,
            clear_call: false,
        })
        .await;
        assert_eq!(sent, Some(cdma_ios::MessageType::ClearCommand));
        assert!(runtime.held_assignments.is_empty());
        assert!(runtime.traffic_auth.is_empty());
    }

    #[tokio::test]
    async fn a_held_call_resumes_when_its_procedures_finish() {
        let (runtime, sent) = held_call_after(crate::authentication::TrafficAction::Done).await;
        assert_ne!(sent, Some(cdma_ios::MessageType::ClearCommand));
        assert!(runtime.held_assignments.is_empty());
    }

    #[tokio::test]
    async fn enforced_registration_without_parameters_is_rejected() {
        let (reply, _) =
            registration_reply(crate::authentication::AuthenticationMode::Enforce, None).await;
        assert_eq!(reply, cdma_ios::MessageType::LocationUpdatingReject);
    }

    #[tokio::test]
    async fn monitored_registration_with_wrong_authr_is_accepted_and_reported() {
        use crate::grpc::events_proto::v1::AuthenticationResult;
        let (reply, events) =
            registration_reply(crate::authentication::AuthenticationMode::Monitor, Some(1)).await;
        assert_eq!(reply, cdma_ios::MessageType::LocationUpdatingAccept);
        assert_eq!(
            authentication_results(&events),
            vec![(AuthenticationResult::SignatureMismatch as i32, false)]
        );
    }

    #[tokio::test]
    async fn disabled_authentication_ignores_wrong_authr() {
        let (reply, events) =
            registration_reply(crate::authentication::AuthenticationMode::Off, Some(1)).await;
        assert_eq!(reply, cdma_ios::MessageType::LocationUpdatingAccept);
        assert!(authentication_results(&events).is_empty());
    }

    fn traffic_test_context() -> crate::authentication::CallAuthContext {
        crate::authentication::CallAuthContext {
            identity: cdma_hlr::model::MobileIdentityKey::from_parts(
                Some(AUTH_TEST_IMSI),
                Some(AUTH_TEST_ESN),
                None,
            )
            .unwrap(),
            imsi: AUTH_TEST_IMSI.to_string(),
            esn: AUTH_TEST_ESN,
            global_result: crate::authentication::AuthenticationResult::Success,
        }
    }

    fn expect_send(
        action: crate::authentication::TrafficAction,
        message_type: cdma_ios::MessageType,
    ) -> Vec<u8> {
        match action {
            crate::authentication::TrafficAction::Send(sent, payload) if sent == message_type => {
                payload
            }
            other => panic!("expected {message_type:?}, got {other:?}"),
        }
    }

    async fn run_traffic_ssd_update(
        mode: crate::authentication::AuthenticationMode,
        authu_flip: u32,
    ) -> crate::authentication::TrafficAction {
        use crate::authentication::{AuthenticationConfig, TrafficAction, TrafficAuthentication};
        use crate::grpc::events_proto::v1::{AuthenticationProcedure, AuthenticationResult};
        let config = AuthenticationConfig {
            mode,
            count_update_on_call: false,
            ..AuthenticationConfig::default()
        };
        let hlr = StubHlrRepo::default();
        let mut procedures =
            TrafficAuthentication::plan(&config, traffic_test_context(), "bs".into(), true)
                .expect("an unset SSD plans an SSD update");

        let request = cdma_ios::SsdUpdateRequestMessage::decode(&expect_send(
            procedures.advance(&config, &hlr).await,
            cdma_ios::MessageType::SsdUpdateRequest,
        ))
        .unwrap();
        let raw = request.authentication_challenge_parameter_randssd.0;
        assert_eq!(raw[0], 0b0100, "Random Number Type RANDSSD");
        let randssd = cdma_cave::RandSsd(raw[1..].try_into().unwrap());
        let ssd_new = cdma_cave::generate_ssd(
            &cdma_cave::AKey(STUB_A_KEY),
            cdma_cave::Esn(AUTH_TEST_ESN),
            &randssd,
        );

        let (s1, s2) = cdma_common::paging::imsi_s_from_imsi(AUTH_TEST_IMSI).unwrap();
        let randbs = 0x1357_9BDF;
        let response = cdma_ios::BaseStationChallengeResponseMessage::decode(&expect_send(
            procedures
                .on_base_station_challenge(&config, &hlr, randbs)
                .await,
            cdma_ios::MessageType::BaseStationChallengeResponse,
        ))
        .unwrap();
        assert_eq!(
            response
                .authentication_response_parameter_authbs
                .signature(),
            cdma_cave::authbs(&ssd_new.ssd_a, randbs, s1, cdma_cave::Esn(AUTH_TEST_ESN)).unwrap()
        );

        assert_eq!(
            procedures.on_ssd_update_response(&hlr, true).await,
            TrafficAction::Report {
                procedure: AuthenticationProcedure::SsdUpdate,
                result: AuthenticationResult::Success,
                clear_call: false,
            }
        );
        assert_eq!(hlr.credentials().ssd, Some(ssd_new));

        let challenge = match cdma_ios::AuthenticationRequestMessage::decode(&expect_send(
            procedures.advance(&config, &hlr).await,
            cdma_ios::MessageType::AuthenticationRequest,
        ))
        .unwrap()
        {
            cdma_ios::AuthenticationRequestMessage::Dtap(message) => message,
            other => panic!("expected DTAP, got {other:?}"),
        };
        let randu = challenge
            .authentication_challenge_parameter_randu
            .randu()
            .expect("RANDU");
        let authu = cdma_cave::authu(&ssd_new.ssd_a, randu, s1, s2, cdma_cave::Esn(AUTH_TEST_ESN))
            .unwrap()
            .signature;
        let result = procedures
            .on_authentication_response(&config, &hlr, authu ^ authu_flip)
            .await;
        if authu_flip == 0 {
            assert_eq!(procedures.advance(&config, &hlr).await, TrafficAction::Done);
        }
        result
    }

    #[tokio::test]
    async fn traffic_ssd_update_and_unique_challenge_succeed() {
        use crate::grpc::events_proto::v1::{AuthenticationProcedure, AuthenticationResult};
        assert_eq!(
            run_traffic_ssd_update(crate::authentication::AuthenticationMode::Enforce, 0).await,
            crate::authentication::TrafficAction::Report {
                procedure: AuthenticationProcedure::UniqueChallenge,
                result: AuthenticationResult::Success,
                clear_call: false,
            }
        );
    }

    #[tokio::test]
    async fn failed_unique_challenge_clears_the_call_only_under_enforce() {
        use crate::grpc::events_proto::v1::{AuthenticationProcedure, AuthenticationResult};
        assert_eq!(
            run_traffic_ssd_update(crate::authentication::AuthenticationMode::Enforce, 1).await,
            crate::authentication::TrafficAction::Report {
                procedure: AuthenticationProcedure::UniqueChallenge,
                result: AuthenticationResult::SignatureMismatch,
                clear_call: true,
            }
        );
        assert_eq!(
            run_traffic_ssd_update(crate::authentication::AuthenticationMode::Monitor, 1).await,
            crate::authentication::TrafficAction::Report {
                procedure: AuthenticationProcedure::UniqueChallenge,
                result: AuthenticationResult::SignatureMismatch,
                clear_call: false,
            }
        );
    }

    #[tokio::test]
    async fn rejected_ssd_update_keeps_the_old_ssd_and_skips_the_challenge() {
        use crate::authentication::{AuthenticationConfig, TrafficAction, TrafficAuthentication};
        use crate::grpc::events_proto::v1::{AuthenticationProcedure, AuthenticationResult};
        let config = AuthenticationConfig {
            count_update_on_call: false,
            ..AuthenticationConfig::default()
        };
        let hlr = StubHlrRepo::default();
        let mut procedures =
            TrafficAuthentication::plan(&config, traffic_test_context(), "bs".into(), true)
                .unwrap();
        procedures.advance(&config, &hlr).await;
        procedures.on_base_station_challenge(&config, &hlr, 1).await;
        assert_eq!(
            procedures.on_ssd_update_response(&hlr, false).await,
            TrafficAction::Report {
                procedure: AuthenticationProcedure::SsdUpdate,
                result: AuthenticationResult::SsdUpdateRejected,
                clear_call: false,
            }
        );
        assert_eq!(hlr.credentials().ssd, Some(STUB_AUTH_SSD));
        assert_eq!(hlr.credentials().randssd, None);
        assert_eq!(procedures.advance(&config, &hlr).await, TrafficAction::Done);
    }

    #[tokio::test]
    async fn count_update_increments_the_hlr_count() {
        use crate::authentication::{AuthenticationConfig, TrafficAction, TrafficAuthentication};
        let config = AuthenticationConfig {
            ssd_update: crate::authentication::SsdUpdatePolicy::Never,
            unique_challenge_on_call: false,
            count_update_on_call: true,
            ..AuthenticationConfig::default()
        };
        let hlr = StubHlrRepo::default();
        let mut procedures =
            TrafficAuthentication::plan(&config, traffic_test_context(), "bs".into(), false)
                .unwrap();
        expect_send(
            procedures.advance(&config, &hlr).await,
            cdma_ios::MessageType::ParameterUpdateRequest,
        );
        assert!(matches!(
            procedures.on_parameter_update_confirm(&hlr).await,
            TrafficAction::Report { .. }
        ));
        assert_eq!(hlr.credentials().auth_count, 1);
    }

    #[tokio::test]
    async fn default_plan_starts_with_a_unique_challenge_for_a_good_call() {
        let config = crate::authentication::AuthenticationConfig::default();
        let mut procedures = crate::authentication::TrafficAuthentication::plan(
            &config,
            traffic_test_context(),
            "bs".into(),
            false,
        )
        .expect("unique challenge is on by default");
        expect_send(
            procedures.advance(&config, &StubHlrRepo::default()).await,
            cdma_ios::MessageType::AuthenticationRequest,
        );
    }

    #[test]
    fn no_traffic_procedures_planned_when_all_are_off() {
        let config = crate::authentication::AuthenticationConfig {
            unique_challenge_on_call: false,
            count_update_on_call: false,
            ..crate::authentication::AuthenticationConfig::default()
        };
        assert!(
            crate::authentication::TrafficAuthentication::plan(
                &config,
                traffic_test_context(),
                "bs".into(),
                false,
            )
            .is_none()
        );
    }

    #[derive(Default)]
    struct StubMediaGateway {
        created: std::sync::Mutex<Vec<CreateCallRequest>>,
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
        async fn register_inbound_session(
            &self,
            _: String,
            _: u16,
        ) -> Result<CallHandle, crate::media_gateway::MgwError> {
            Ok(CallHandle(1))
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
            request: crate::media_gateway::CreateCallRequest,
        ) -> Result<CallHandle, crate::media_gateway::MgwError> {
            self.created.lock().unwrap().push(request);
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
            mobile_identity_meid: None,
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

    const STUB_AUTH_SSD: cdma_cave::Ssd = cdma_cave::Ssd {
        ssd_a: [0xCC, 0x38, 0x12, 0x94, 0x9F, 0x4D, 0xCD, 0x0D],
        ssd_b: [0x31, 0x05, 0x02, 0x34, 0x58, 0x0E, 0x63, 0xB4],
    };

    const STUB_A_KEY: [u8; 8] = [1; 8];

    #[async_trait::async_trait]
    impl cdma_hlr::repository::HlrRepository for StubHlrRepo {
        async fn get_subscriber_auth_state(
            &self,
            _: &cdma_hlr::model::MobileIdentityKey,
        ) -> Result<Option<cdma_hlr::model::IdentityAuthState>, String> {
            let creds = self.credentials();
            Ok(Some(cdma_hlr::model::IdentityAuthState {
                subscriber_identity_id: uuid::Uuid::nil(),
                credentials_provisioned: creds.a_key.is_some(),
                ssd_provisioned: creds.ssd.is_some(),
                auth_algo: creds.auth_algo,
                auth_count: creds.auth_count,
                auth_failure_count: 0,
                last_auth_at: None,
            }))
        }
        async fn verify_cave_global_challenge(
            &self,
            _: &cdma_hlr::model::MobileIdentityKey,
            challenge: &cdma_hlr::auth::GlobalChallenge,
        ) -> Result<cdma_hlr::auth::CaveVerdict, String> {
            let creds = self.credentials();
            let verdict = cdma_hlr::auth::verify_global_challenge(&creds, challenge)?;
            if verdict.outcome.is_success()
                && let Some(count) = cdma_hlr::auth::resynced_count(&creds, challenge)
            {
                self.credentials.lock().unwrap().auth_count = count;
            }
            Ok(verdict)
        }
        async fn verify_cave_unique_challenge(
            &self,
            _: &cdma_hlr::model::MobileIdentityKey,
            challenge: &cdma_hlr::auth::UniqueChallenge,
        ) -> Result<cdma_hlr::auth::CaveVerdict, String> {
            cdma_hlr::auth::verify_unique_challenge(&self.credentials(), challenge)
        }
        async fn begin_cave_ssd_update(
            &self,
            _: &cdma_hlr::model::MobileIdentityKey,
        ) -> Result<u64, String> {
            const STUB_RANDSSD: u64 = 0x4d_18ee_aa05_895c;
            self.credentials.lock().unwrap().randssd =
                Some(cdma_cave::RandSsd::from_u64(STUB_RANDSSD).unwrap().0);
            Ok(STUB_RANDSSD)
        }
        async fn cave_base_station_challenge(
            &self,
            _: &cdma_hlr::model::MobileIdentityKey,
            esn: u32,
            randbs: u32,
            imsi_s1: u32,
        ) -> Result<u32, String> {
            cdma_hlr::auth::base_station_challenge(&self.credentials(), esn, randbs, imsi_s1)
        }
        async fn finish_cave_ssd_update(
            &self,
            _: &cdma_hlr::model::MobileIdentityKey,
            esn: u32,
            confirmed: bool,
        ) -> Result<(), String> {
            let creds = self.credentials();
            let ssd = if confirmed {
                Some(cdma_hlr::auth::committed_ssd(&creds, esn)?)
            } else {
                creds.ssd
            };
            let mut credentials = self.credentials.lock().unwrap();
            credentials.ssd = ssd;
            credentials.randssd = None;
            Ok(())
        }
        async fn increment_cave_count(
            &self,
            _: &cdma_hlr::model::MobileIdentityKey,
        ) -> Result<u8, String> {
            let mut credentials = self.credentials.lock().unwrap();
            credentials.auth_count = cdma_hlr::auth::next_auth_count(credentials.auth_count);
            Ok(credentials.auth_count)
        }
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
            *self.seen.lock().unwrap() += 1;
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
                    esn: self.esn,
                    meid: self.meid.map(str::to_string),
                    is_primary: true,
                    created_at: chrono::Utc::now(),
                };
                let binding = cdma_hlr::model::RegistrationBinding {
                    subscriber_id: self.subscriber_id,
                    serving_bs_id: test_node().0,
                    state: self.registration_state.clone(),
                    imsi: Some(self.imsi.to_string()),
                    esn: None,
                    meid: None,
                    mob_p_rev: Some(self.mob_p_rev),
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
            id: uuid::Uuid,
        ) -> Result<Option<cdma_hlr::model::ResolvedSubscriber>, String> {
            if id == self.subscriber_id {
                self.get_subscriber_by_phone_number(self.phone_number).await
            } else {
                Ok(None)
            }
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
            identity: &cdma_hlr::model::MobileIdentityKey,
        ) -> Result<Option<cdma_hlr::model::ResolvedSubscriber>, String> {
            if identity.imsi() == self.imsi {
                self.get_subscriber_by_phone_number(self.phone_number).await
            } else {
                Ok(None)
            }
        }
        async fn resolve_by_hardware_identity(
            &self,
            esn: Option<u32>,
            meid: Option<&str>,
        ) -> Result<Option<cdma_hlr::model::ResolvedSubscriber>, String> {
            if self.strict_hardware_lookup
                && esn.is_none_or(|value| Some(value) != self.esn)
                && !meid.is_some_and(|value| {
                    self.meid
                        .is_some_and(|expected| expected.eq_ignore_ascii_case(value))
                })
            {
                return Ok(None);
            }
            self.get_subscriber_by_phone_number(self.phone_number).await
        }
        async fn verify_cave_global_challenge(
            &self,
            _: &cdma_hlr::model::MobileIdentityKey,
            challenge: &cdma_hlr::auth::GlobalChallenge,
        ) -> Result<cdma_hlr::auth::CaveVerdict, String> {
            Ok(cdma_hlr::auth::CaveVerdict {
                call_keys: None,
                outcome: if challenge.authr == 1 {
                    cdma_hlr::auth::CaveAuthOutcome::SignatureMismatch
                } else {
                    cdma_hlr::auth::CaveAuthOutcome::NotProvisioned
                },
            })
        }
        async fn upsert_registration_binding(
            &self,
            binding: cdma_hlr::model::RegistrationBinding,
        ) -> Result<cdma_hlr::model::RegistrationBinding, String> {
            *self.refreshed_binding.lock().unwrap() = Some(binding.clone());
            Ok(binding)
        }
        async fn get_registration_binding(
            &self,
            subscriber_id: uuid::Uuid,
        ) -> Result<Option<cdma_hlr::model::RegistrationBinding>, String> {
            assert_eq!(subscriber_id, self.subscriber_id);
            if let Some(binding) = self.refreshed_binding.lock().unwrap().clone() {
                return Ok(Some(binding));
            }
            Ok(Some(cdma_hlr::model::RegistrationBinding {
                subscriber_id,
                serving_bs_id: test_node().0,
                state: self.registration_state.clone(),
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
            mobile_identity_meid: None,
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
            mobile_identity_meid: None,
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

        use crate::CallId;

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

        /// The single base station every in-process test talks through.
        pub const TEST_NODE: &str = "bsc-test";

        pub fn test_node() -> crate::base_station::BaseStationId {
            crate::base_station::BaseStationId(TEST_NODE.to_string())
        }

        #[async_trait::async_trait]
        impl super::MscA1Endpoint for InProcessMscEndpoint {
            async fn recv(&self) -> Option<crate::base_station::A1Event> {
                self.inbound_rx.lock().await.recv().await.map(|message| {
                    crate::base_station::A1Event::Message {
                        node: test_node(),
                        message,
                    }
                })
            }

            async fn send(&self, message: EncodedA1Message) -> Result<(), A1TransportError> {
                self.outbound_tx
                    .send(message)
                    .await
                    .map_err(|_| A1TransportError::Closed)
            }

            async fn send_to_node(
                &self,
                _: &crate::base_station::BaseStationId,
                message: EncodedA1Message,
            ) -> Result<(), A1TransportError> {
                self.send(message).await
            }

            fn bind_call(&self, _: CallId, _: &crate::base_station::BaseStationId) {}

            fn release_call(&self, _: CallId) {}

            fn node_for_call(&self, _: CallId) -> Option<crate::base_station::BaseStationId> {
                Some(test_node())
            }

            fn is_attached(&self, node: &crate::base_station::BaseStationId) -> bool {
                node.0 == TEST_NODE
            }

            fn served_cell(
                &self,
                _: &crate::base_station::BaseStationId,
                _: cdma_ios::CellId,
            ) -> Option<crate::base_station::ServedCell> {
                None
            }
            fn serving_imsi_prefix(
                &self,
                _: &crate::base_station::BaseStationId,
            ) -> Option<(String, String)> {
                None
            }
        }
    }

    fn paging_request() -> PagingRequestMessage {
        PagingRequestMessage {
            mobile_identity_meid: None,
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
            mobile_identity_meid: None,
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
            hlr_repo: Arc::new(StubHlrRepo::default()),
            smsc_repo: None,
            welcome_sms: None,
            sms_retry: crate::config::SmsRetryConfig::default(),
            default_voice_service_option: 3,
            supported_voice_service_options: vec![3],
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
            authentication: Default::default(),
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
                    circuit_identity_code: Some(CircuitIdentityCode {
                        pcm_multiplexer: 0x0123,
                        timeslot: 0x1a,
                    }),
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
            .handle_bsc_a1_message(&endpoint, &test_node(), clear_request)
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
            .handle_bsc_a1_message(&endpoint, &test_node(), clear_complete)
            .await;
        assert_eq!(runtime.controller.active_call_count(), 0);
    }

    #[tokio::test]
    async fn rejected_paged_evrc_assigns_configured_qcelp13k() {
        use cdma_common::consts::{SERVICE_OPTION_QCELP13, SERVICE_OPTION_REJECTED};

        let (client, endpoint) = cdma_bsc_a1_edge_compat::InProcessMscClient::pair(4);
        let mut runtime = MscRuntime::new(MscRuntimeConfig {
            hlr_repo: Arc::new(StubHlrRepo::default()),
            smsc_repo: None,
            welcome_sms: None,
            sms_retry: crate::config::SmsRetryConfig::default(),
            default_voice_service_option: SERVICE_OPTION_QCELP13,
            supported_voice_service_options: vec![SERVICE_OPTION_QCELP13],
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
            authentication: Default::default(),
        });
        let call_id = runtime.controller.create_call(
            CallDirection::MobileTerminated,
            Some(MobileIdentity::Imsi("12345678901".to_string())),
        );
        let page = paging_request();
        runtime
            .controller
            .apply_from_msc(call_id, &ProcedureMessage::PagingRequest(page.clone()))
            .unwrap();
        runtime.circuits.paging_requests.insert(call_id, page);

        let mut response = paging_response();
        response.service_option = Some(ServiceOption(SERVICE_OPTION_REJECTED));
        runtime
            .handle_bsc_a1_message(
                &endpoint,
                &test_node(),
                EncodedA1Message::from_message_for_call(
                    &cdma_ios::Message::new(
                        cdma_ios::MessageType::PagingResponse,
                        response.encode().unwrap(),
                    ),
                    Some(call_id.0),
                ),
            )
            .await;

        let outbound = timeout(Duration::from_millis(50), client.poll_a1())
            .await
            .expect("rejected page should produce an Assignment Request")
            .expect("Assignment Request should be available");
        let decoded = outbound.decode().unwrap();
        let assignment = AssignmentRequestMessage::decode(&decoded.payload).unwrap();
        assert_eq!(
            assignment.service_option,
            Some(ServiceOption(SERVICE_OPTION_QCELP13))
        );
        assert_eq!(
            runtime.circuits.circuits[&assignment
                .circuit_identity_code
                .expect("voice assignment CIC")
                .to_packed()]
                .service_option,
            SERVICE_OPTION_QCELP13
        );
    }

    #[tokio::test]
    async fn secondary_paging_response_waits_for_active_assignment_complete() {
        let (client, endpoint) = cdma_bsc_a1_edge_compat::InProcessMscClient::pair(4);
        let mut runtime = MscRuntime::new(MscRuntimeConfig {
            hlr_repo: Arc::new(StubHlrRepo::default()),
            smsc_repo: None,
            welcome_sms: None,
            sms_retry: crate::config::SmsRetryConfig::default(),
            default_voice_service_option: 3,
            supported_voice_service_options: vec![3],
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
            authentication: Default::default(),
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
            .handle_bsc_a1_message(&endpoint, &test_node(), first_paging_response)
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
            .handle_bsc_a1_message(&endpoint, &test_node(), paging_response)
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
            .handle_bsc_a1_message(&endpoint, &test_node(), assignment_complete)
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
            first_assignment
                .circuit_identity_code
                .expect("first voice assignment CIC")
                .to_packed(),
            second_assignment
                .circuit_identity_code
                .expect("second voice assignment CIC")
                .to_packed()
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
        let first_circuit_id = first_assignment
            .circuit_identity_code
            .expect("first voice assignment CIC")
            .to_packed();
        let second_circuit_id = second_assignment
            .circuit_identity_code
            .expect("second voice assignment CIC")
            .to_packed();
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
            hlr_repo: Arc::new(StubHlrRepo::default()),
            smsc_repo: None,
            welcome_sms: None,
            sms_retry: crate::config::SmsRetryConfig::default(),
            default_voice_service_option: 3,
            supported_voice_service_options: vec![3],
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
            authentication: Default::default(),
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
        runtime
            .handle_bsc_a1_message(&endpoint, &test_node(), primary_pr)
            .await;
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
        runtime
            .handle_bsc_a1_message(&endpoint, &test_node(), primary_ac)
            .await;
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
        runtime
            .handle_bsc_a1_message(&endpoint, &test_node(), secondary_pr)
            .await;
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
            .handle_bsc_a1_message(&endpoint, &test_node(), assignment_failure_msg(call_id))
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
            hlr_repo: Arc::new(StubHlrRepo::default()),
            smsc_repo: None,
            welcome_sms: None,
            sms_retry: crate::config::SmsRetryConfig::default(),
            default_voice_service_option: 3,
            supported_voice_service_options: vec![3],
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
            authentication: Default::default(),
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
        runtime
            .handle_bsc_a1_message(&endpoint, &test_node(), first_pr)
            .await;
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
            .handle_bsc_a1_message(&endpoint, &test_node(), assignment_failure_msg(call_id))
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
        runtime
            .handle_bsc_a1_message(&endpoint, &test_node(), second_pr)
            .await;
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
            .handle_bsc_a1_message(&endpoint, &test_node(), assignment_failure_msg(call_id))
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
            .handle_bsc_a1_message(&endpoint, &test_node(), assignment_failure_msg(call_id))
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
        runtime
            .handle_bsc_a1_message(&endpoint, &test_node(), fresh_pr)
            .await;
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
        runtime
            .handle_bsc_a1_message(&endpoint, &test_node(), ac)
            .await;

        assert!(
            runtime
                .circuits
                .mt_assignment_failure_retries
                .get(&CallId(call_id))
                .is_none(),
            "AssignmentComplete must reset the retry counter"
        );
    }

    fn m2m_runtime(hlr: Arc<M2mHlrRepo>) -> MscRuntime {
        MscRuntime::new(MscRuntimeConfig {
            hlr_repo: hlr.clone(),
            smsc_repo: None,
            welcome_sms: None,
            sms_retry: crate::config::SmsRetryConfig::default(),
            default_voice_service_option: 3,
            supported_voice_service_options: vec![3],
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
            authentication: Default::default(),
        })
    }

    struct M2mSetup {
        caller: CallId,
        callee: CallId,
        caller_circuit: u16,
        page: PagingRequestMessage,
    }

    async fn begin_m2m_setup(
        runtime: &mut MscRuntime,
        endpoint: &dyn MscA1Endpoint,
        client: &cdma_bsc_a1_edge_compat::InProcessMscClient,
    ) -> M2mSetup {
        let caller = CallId(4242);
        let cli3 = cm_service_request_cli3(Some(vec![0x81, 0x55, 0x95, 0x78, 0x56, 0x34]));
        runtime
            .handle_bsc_a1_message(
                endpoint,
                &test_node(),
                EncodedA1Message::from_message_for_call(
                    &cdma_ios::Message::new(
                        cdma_ios::MessageType::CompleteLayer3Information,
                        cli3.encode().unwrap(),
                    ),
                    Some(caller.0),
                ),
            )
            .await;
        let assignment = timeout(Duration::from_millis(50), client.poll_a1())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            assignment.message_type(),
            cdma_ios::MessageType::AssignmentRequest
        );
        assert_eq!(assignment.call_id(), Some(caller.0));
        let caller_circuit =
            AssignmentRequestMessage::decode(&assignment.decode().unwrap().payload)
                .unwrap()
                .circuit_identity_code
                .expect("voice assignment CIC")
                .to_packed();
        let page = timeout(Duration::from_millis(50), client.poll_a1())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(page.message_type(), cdma_ios::MessageType::PagingRequest);
        let callee = CallId(page.call_id().unwrap());
        assert_ne!(caller, callee);
        M2mSetup {
            caller,
            callee,
            caller_circuit,
            page: PagingRequestMessage::decode(&page.decode().unwrap().payload).unwrap(),
        }
    }

    fn m2m_assignment_complete(call_id: CallId) -> EncodedA1Message {
        EncodedA1Message::from_message_for_call(
            &cdma_ios::Message::new(
                cdma_ios::MessageType::AssignmentComplete,
                AssignmentCompleteMessage {
                    channel_number: ChannelNumber(10),
                    encryption_information: None,
                    service_option: Some(ServiceOption::EVRC_A),
                    a2p_bearer_session_params: None,
                    a2p_bearer_format_params: None,
                }
                .encode()
                .unwrap(),
            ),
            Some(call_id.0),
        )
    }

    #[tokio::test]
    async fn mo_m2m_page_selects_qcelp13_for_registered_is95_callee() {
        let (client, endpoint) = cdma_bsc_a1_edge_compat::InProcessMscClient::pair(8);
        let mut runtime = m2m_runtime(Arc::new(M2mHlrRepo::with_p_rev(3)));
        runtime.config.supported_voice_service_options =
            vec![cdma_common::consts::SERVICE_OPTION_QCELP13];
        runtime.config.default_voice_service_option = cdma_common::consts::SERVICE_OPTION_QCELP13;
        let setup = begin_m2m_setup(&mut runtime, &endpoint, &client).await;
        assert_eq!(
            setup.page.service_option,
            Some(ServiceOption(cdma_common::consts::SERVICE_OPTION_QCELP13))
        );
    }

    #[tokio::test]
    async fn mo_m2m_assigns_both_mobiles_before_either_completes() {
        for caller_first in [true, false] {
            let (client, endpoint) = cdma_bsc_a1_edge_compat::InProcessMscClient::pair(8);
            let mut runtime = m2m_runtime(Arc::new(M2mHlrRepo::new()));
            let setup = begin_m2m_setup(&mut runtime, &endpoint, &client).await;
            let mut response = paging_response();
            response.tag = setup.page.tag;
            let response = EncodedA1Message::from_message_for_call(
                &cdma_ios::Message::new(
                    cdma_ios::MessageType::PagingResponse,
                    response.encode().unwrap(),
                ),
                Some(setup.callee.0),
            );
            runtime
                .handle_bsc_a1_message(&endpoint, &test_node(), response.clone())
                .await;
            let assignment = timeout(Duration::from_millis(50), client.poll_a1())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                assignment.message_type(),
                cdma_ios::MessageType::AssignmentRequest
            );
            assert_eq!(assignment.call_id(), Some(setup.callee.0));
            let callee_circuit =
                AssignmentRequestMessage::decode(&assignment.decode().unwrap().payload)
                    .unwrap()
                    .circuit_identity_code
                    .expect("voice assignment CIC")
                    .to_packed();
            assert!(
                runtime
                    .circuits
                    .has_pending_assignment_complete(setup.caller)
            );
            assert!(
                runtime
                    .circuits
                    .has_pending_assignment_complete(setup.callee)
            );
            assert_eq!(
                runtime.circuits.circuits[&setup.caller_circuit].peer_circuit_id,
                Some(callee_circuit)
            );
            assert_eq!(
                runtime.circuits.circuits[&callee_circuit].peer_circuit_id,
                Some(setup.caller_circuit)
            );

            runtime
                .handle_bsc_a1_message(&endpoint, &test_node(), response)
                .await;
            assert_eq!(runtime.circuits.circuits.len(), 2);
            assert!(runtime.circuits.deferred_paging_responses.is_empty());

            let order = if caller_first {
                [setup.caller, setup.callee]
            } else {
                [setup.callee, setup.caller]
            };
            runtime
                .handle_bsc_a1_message(&endpoint, &test_node(), m2m_assignment_complete(order[0]))
                .await;
            assert!(
                timeout(Duration::from_millis(20), client.poll_a1())
                    .await
                    .is_err()
            );
            runtime
                .handle_bsc_a1_message(&endpoint, &test_node(), m2m_assignment_complete(order[1]))
                .await;
            let alert = timeout(Duration::from_millis(50), client.poll_a1())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                alert.message_type(),
                cdma_ios::MessageType::AlertWithInformation
            );
            assert_eq!(alert.call_id(), Some(setup.callee.0));
            assert!(
                !runtime
                    .circuits
                    .has_pending_assignment_complete(setup.caller)
            );
            assert!(
                !runtime
                    .circuits
                    .has_pending_assignment_complete(setup.callee)
            );

            runtime
                .handle_bsc_a1_message(
                    &endpoint,
                    &test_node(),
                    EncodedA1Message::from_message_for_call(
                        &cdma_ios::Message::new(
                            cdma_ios::MessageType::Connect,
                            cdma_ios::ConnectMessage.encode().unwrap(),
                        ),
                        Some(setup.callee.0),
                    ),
                )
                .await;
            let progress = timeout(Duration::from_millis(50), client.poll_a1())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(progress.message_type(), cdma_ios::MessageType::Progress);
            assert_eq!(progress.call_id(), Some(setup.caller.0));
            for leg in [setup.caller, setup.callee] {
                assert_eq!(
                    runtime.controller.state(leg),
                    Some(cdma_ios::CallControlState::Connected)
                );
            }
        }
    }

    #[tokio::test]
    async fn mo_primary_assignment_failure_clears_both_mobiles() {
        let (client, endpoint) = cdma_bsc_a1_edge_compat::InProcessMscClient::pair(8);
        let mut runtime = m2m_runtime(Arc::new(M2mHlrRepo::new()));
        let setup = begin_m2m_setup(&mut runtime, &endpoint, &client).await;
        runtime
            .handle_bsc_a1_message(
                &endpoint,
                &test_node(),
                assignment_failure_msg(setup.caller.0),
            )
            .await;
        for leg in [setup.caller, setup.callee] {
            let clear = timeout(Duration::from_millis(50), client.poll_a1())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(clear.message_type(), cdma_ios::MessageType::ClearCommand);
            assert_eq!(clear.call_id(), Some(leg.0));
            assert!(runtime.controller.snapshot(leg).is_none());
            assert!(!runtime.mt_page_retry.contains(leg));
        }
        assert!(runtime.circuits.m2m_calls.is_empty());
        assert!(runtime.circuits.circuits.is_empty());
    }

    #[tokio::test]
    async fn mo_m2m_clear_releases_both_contexts_during_setup() {
        let (client, endpoint) = cdma_bsc_a1_edge_compat::InProcessMscClient::pair(8);
        let mut runtime = m2m_runtime(Arc::new(M2mHlrRepo::new()));
        let setup = begin_m2m_setup(&mut runtime, &endpoint, &client).await;
        runtime.send_clear_command(&endpoint, setup.callee).await;
        for leg in [setup.caller, setup.callee] {
            let clear = timeout(Duration::from_millis(50), client.poll_a1())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(clear.message_type(), cdma_ios::MessageType::ClearCommand);
            assert_eq!(clear.call_id(), Some(leg.0));
            runtime
                .handle_bsc_a1_message(
                    &endpoint,
                    &test_node(),
                    EncodedA1Message::from_message_for_call(
                        &cdma_ios::Message::new(
                            cdma_ios::MessageType::ClearComplete,
                            cdma_ios::ClearCompleteMessage {
                                power_down_indicator: false,
                            }
                            .encode()
                            .unwrap(),
                        ),
                        Some(leg.0),
                    ),
                )
                .await;
            assert!(runtime.controller.snapshot(leg).is_none());
            assert!(!runtime.mt_page_retry.contains(leg));
        }
        assert!(runtime.circuits.m2m_calls.is_empty());
        assert!(runtime.circuits.circuits.is_empty());
        assert!(runtime.mt_call.mt_plans.is_empty());
    }

    #[tokio::test]
    async fn mo_cli3_defers_sip_invite_until_assignment_complete() {
        let (client, endpoint) = cdma_bsc_a1_edge_compat::InProcessMscClient::pair(4);
        let mut runtime = MscRuntime::new(MscRuntimeConfig {
            hlr_repo: Arc::new(StubHlrRepo::default()),
            smsc_repo: None,
            welcome_sms: None,
            sms_retry: crate::config::SmsRetryConfig::default(),
            default_voice_service_option: 3,
            supported_voice_service_options: vec![3],
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
            authentication: Default::default(),
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

        runtime
            .handle_bsc_a1_message(&endpoint, &test_node(), message)
            .await;

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
            hlr_repo: Arc::new(StubHlrRepo::default()),
            smsc_repo: None,
            welcome_sms: None,
            sms_retry: crate::config::SmsRetryConfig::default(),
            default_voice_service_option: 3,
            supported_voice_service_options: vec![3],
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
            authentication: Default::default(),
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
        runtime
            .handle_bsc_a1_message(&endpoint, &test_node(), cli3_msg)
            .await;

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
        runtime
            .handle_bsc_a1_message(&endpoint, &test_node(), ac_msg)
            .await;

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
            hlr_repo: Arc::new(StubHlrRepo::default()),
            smsc_repo: None,
            welcome_sms: None,
            sms_retry: crate::config::SmsRetryConfig::default(),
            default_voice_service_option: 3,
            supported_voice_service_options: vec![3],
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
            authentication: Default::default(),
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

        let gateway_bits = (0..171)
            .map(|index| (index % 5 == 0) as u8)
            .collect::<Vec<_>>();
        let bearer_payload = cdma_voice::pack_voice_bits(&gateway_bits, gateway_bits.len());
        runtime
            .handle_reverse_bearer_frame(VoiceBearerFrame {
                circuit_id: 23,
                rate_bps: 9600,
                payload: bearer_payload,
            })
            .await;

        let forwarded = gateway.forwarded.lock().unwrap();
        assert_eq!(forwarded.len(), 1);
        assert_eq!(forwarded[0].0, CallHandle(77));
        assert_eq!(
            forwarded[0].1,
            VocoderFrame {
                payload: gateway_bits,
                rate_bps: 9600,
            }
        );
    }

    #[tokio::test]
    async fn m2m_bearer_transcodes_evrc_and_qcelp_in_both_directions() {
        let msc_bearer = Arc::new(VoiceBearerManager::new(std::net::Ipv4Addr::LOCALHOST));
        let bsc_bearer = VoiceBearerManager::new(std::net::Ipv4Addr::LOCALHOST);
        for (circuit_id, format) in [
            (4, cdma_ios::VoiceBearerFormat::Evrc),
            (5, cdma_ios::VoiceBearerFormat::Vocoder13k),
        ] {
            let msc_local = msc_bearer.open_circuit(circuit_id, None).await.unwrap();
            let bsc_local = bsc_bearer
                .open_circuit(circuit_id, Some(msc_local))
                .await
                .unwrap();
            msc_bearer.set_circuit_remote(circuit_id, bsc_local);
            let payload_types = cdma_ios::BearerPayloadTypes {
                voice: cdma_ios::VoiceBearerPayloadType {
                    format,
                    payload_type: cdma_ios::voice_bearer::VOICE_RTP_PAYLOAD_TYPE,
                },
                telephone_event: None,
            };
            msc_bearer.set_circuit_payload_types(circuit_id, payload_types);
            bsc_bearer.set_circuit_payload_types(circuit_id, payload_types);
        }

        let mut runtime = MscRuntime::new(MscRuntimeConfig {
            hlr_repo: Arc::new(StubHlrRepo::default()),
            smsc_repo: None,
            welcome_sms: None,
            sms_retry: crate::config::SmsRetryConfig::default(),
            default_voice_service_option: 3,
            supported_voice_service_options: vec![3],
            wav_file: None,
            gateway_fallback_to_wav: true,
            local_answer_delay_ms: 10_000,
            media_ringback_enabled: false,
            media_ringback_type: MediaRingbackType::Nanp,
            sip_ringback_disable: false,
            inbound_sip_msc_ringback: false,
            generate_ringback: true,
            send_tones_alert: false,
            page_retry_cooldown_ms: 1_000,
            page_retry_max_duration_ms: 60_000,
            failure_tone_duration_ms: 0,
            voice_bearer: Some(msc_bearer),
            media_gateway: None,
            otasp: None,
            authentication: Default::default(),
        });
        let call_id = runtime.controller.create_call_with_id(
            CallId(91),
            CallDirection::MobileTerminated,
            Some(MobileIdentity::Imsi("12345678901".to_string())),
        );
        for (circuit_id, service_option, leg_role) in [
            (4, 3, MscVoiceLeg::Primary),
            (5, 32768, MscVoiceLeg::Secondary),
        ] {
            runtime.circuits.insert_circuit_session(
                circuit_id,
                CircuitSession {
                    call_id,
                    audio_file: None,
                    service_option,
                    leg_role,
                    peer_circuit_id: None,
                    bearer_remote_ready: true,
                    media_gateway_handle: None,
                    called_number: None,
                },
            );
        }

        let mut pcm = [0i16; cdma_voice::SAMPLES_PER_FRAME];
        for (index, sample) in pcm.iter_mut().enumerate() {
            let angle = 2.0 * std::f64::consts::PI * 500.0 * index as f64 / 8_000.0;
            *sample = (angle.sin() * 8_000.0) as i16;
        }

        let mut evrc_encoder =
            cdma_voice::VoiceEncoder::new(cdma_voice::VoiceCodec::EvrcA).unwrap();
        let (evrc_rate, evrc_payload) = evrc_encoder.encode(&pcm).unwrap();
        let evrc_frame = VoiceBearerFrame {
            circuit_id: 4,
            rate_bps: cdma_voice::VoiceCodec::EvrcA.rate_bps(evrc_rate),
            payload: evrc_payload,
        };
        runtime
            .handle_reverse_bearer_frame(evrc_frame.clone())
            .await;
        assert!(
            timeout(Duration::from_millis(30), bsc_bearer.recv())
                .await
                .is_err(),
            "pre-connect mobile-to-mobile media must not be queued"
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
                    circuit_identity_code: Some(CircuitIdentityCode {
                        pcm_multiplexer: 0x0123,
                        timeslot: 0x1a,
                    }),
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
        runtime.handle_reverse_bearer_frame(evrc_frame).await;
        let to_qcelp =
            match tokio::time::timeout(std::time::Duration::from_secs(1), bsc_bearer.recv())
                .await
                .expect("QCELP bearer timeout")
                .expect("QCELP bearer event")
            {
                cdma_ios::BearerEvent::Voice(frame) => frame,
                cdma_ios::BearerEvent::Dtmf(_) => panic!("expected voice"),
            };
        assert_eq!(to_qcelp.circuit_id, 5);
        let qcelp_rate = cdma_voice::VoiceCodec::Qcelp13k
            .rate_from_bps(to_qcelp.rate_bps)
            .expect("QCELP rate");
        cdma_voice::qcelp13k::Qcelp13kDecoder::new()
            .unwrap()
            .decode(qcelp_rate, &to_qcelp.payload)
            .expect("decode transcoded QCELP");

        let mut qcelp_encoder =
            cdma_voice::VoiceEncoder::new(cdma_voice::VoiceCodec::Qcelp13k).unwrap();
        let (qcelp_rate, qcelp_payload) = qcelp_encoder.encode(&pcm).unwrap();
        runtime
            .handle_reverse_bearer_frame(VoiceBearerFrame {
                circuit_id: 5,
                rate_bps: cdma_voice::VoiceCodec::Qcelp13k.rate_bps(qcelp_rate),
                payload: qcelp_payload,
            })
            .await;
        let to_evrc =
            match tokio::time::timeout(std::time::Duration::from_secs(1), bsc_bearer.recv())
                .await
                .expect("EVRC bearer timeout")
                .expect("EVRC bearer event")
            {
                cdma_ios::BearerEvent::Voice(frame) => frame,
                cdma_ios::BearerEvent::Dtmf(_) => panic!("expected voice"),
            };
        assert_eq!(to_evrc.circuit_id, 4);
        cdma_voice::evrc::EvrcDecoder::new()
            .unwrap()
            .decode(&to_evrc.payload)
            .expect("decode transcoded EVRC");
    }

    #[test]
    fn assignment_complete_correlation_uses_active_leg_key() {
        let mut runtime = MscRuntime::new(MscRuntimeConfig {
            hlr_repo: Arc::new(StubHlrRepo::default()),
            smsc_repo: None,
            welcome_sms: None,
            sms_retry: crate::config::SmsRetryConfig::default(),
            default_voice_service_option: 3,
            supported_voice_service_options: vec![3],
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
            authentication: Default::default(),
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
            hlr_repo: Arc::new(StubHlrRepo::default()),
            smsc_repo: None,
            welcome_sms: None,
            sms_retry: crate::config::SmsRetryConfig::default(),
            default_voice_service_option: 3,
            supported_voice_service_options: vec![3],
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
            authentication: Default::default(),
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
