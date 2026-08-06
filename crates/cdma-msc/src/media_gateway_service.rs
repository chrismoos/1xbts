//! Media gateway event handling for the MSC runtime.
//!
//! Owns the mapping from media gateway call handles to MSC call IDs, and
//! processes gateway events (ringing, answered, failed, released, media frames).

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use log::{debug, info, warn};

use cdma_ios::{
    AlertWithInformationMessage, CallControlState, EncodedA1Message, Message, MessageType,
    MsInformationRecords, ProcedureMessage, ProgressMessage, Signal, VoiceBearerFrame,
    VoiceBearerManager,
};

use crate::call_control::{CallId, MscCallController};
use crate::circuit::CircuitService;
use crate::media::MediaService;
use crate::media_gateway::{CallHandle, MediaGatewayClient, MediaGatewayEvent, ReleaseCause};
use crate::runtime::MscA1Endpoint;

pub(crate) struct MediaGatewayService {
    pub(crate) media_gateway_calls: HashMap<CallHandle, CallId>,
    pub(crate) alert_sent: HashSet<CallId>,
    answered_locally: HashSet<CallId>,
    failure_signaled: HashSet<CallId>,
    pending_clears: HashMap<CallId, PendingClear>,
    pending_post_assignment: HashMap<CallId, PendingPostAssignment>,
    inbound_sessions: HashMap<String, InboundSessionMeta>,
    pub(crate) inbound_by_call: HashMap<CallId, String>,
    active_subscribers: HashMap<uuid::Uuid, CallId>,
    subscriber_by_call: HashMap<CallId, uuid::Uuid>,
}

#[derive(Debug, Clone)]
pub(crate) struct InboundSessionMeta {
    pub session_id: String,
    pub codec: String,
    pub progress_sent: bool,
}

#[derive(Debug, Clone)]
struct PendingClear {
    deadline: tokio::time::Instant,
    cause: GatewayClearCause,
}

#[derive(Debug, Clone, Copy, Default)]
struct PendingPostAssignment {
    send_alert: bool,
}

fn ready_for_alert(state: Option<CallControlState>) -> bool {
    matches!(
        state,
        Some(CallControlState::Assigned | CallControlState::Alerting | CallControlState::Connected)
    )
}

impl MediaGatewayService {
    pub(crate) fn new() -> Self {
        Self {
            media_gateway_calls: HashMap::new(),
            alert_sent: HashSet::new(),
            answered_locally: HashSet::new(),
            failure_signaled: HashSet::new(),
            pending_clears: HashMap::new(),
            pending_post_assignment: HashMap::new(),
            inbound_sessions: HashMap::new(),
            inbound_by_call: HashMap::new(),
            active_subscribers: HashMap::new(),
            subscriber_by_call: HashMap::new(),
        }
    }

    pub(crate) fn register_active_subscriber(
        &mut self,
        subscriber_id: uuid::Uuid,
        call_id: CallId,
    ) {
        self.active_subscribers.insert(subscriber_id, call_id);
        self.subscriber_by_call.insert(call_id, subscriber_id);
    }

    pub(crate) fn register_inbound_session(
        &mut self,
        session_id: String,
        call_id: CallId,
        codec: String,
    ) {
        self.inbound_by_call.insert(call_id, session_id.clone());
        self.inbound_sessions.insert(
            session_id.clone(),
            InboundSessionMeta {
                session_id,
                codec,
                progress_sent: false,
            },
        );
    }

    pub(crate) fn is_subscriber_busy(&self, subscriber_id: uuid::Uuid) -> bool {
        self.active_subscribers.contains_key(&subscriber_id)
    }

    /// Set once MSC drives a local-side answer for `call_id`. `cleanup_call`
    /// reads this to skip the pre-answer 487 reject.
    pub(crate) fn mark_answered(&mut self, call_id: CallId) {
        self.answered_locally.insert(call_id);
    }

    pub(crate) fn inbound_session_for_call(&self, call_id: CallId) -> Option<&InboundSessionMeta> {
        self.inbound_by_call
            .get(&call_id)
            .and_then(|sid| self.inbound_sessions.get(sid))
    }

    pub(crate) fn mark_inbound_progress_sent(&mut self, call_id: CallId) {
        if let Some(sid) = self.inbound_by_call.get(&call_id).cloned()
            && let Some(meta) = self.inbound_sessions.get_mut(&sid)
        {
            meta.progress_sent = true;
        }
    }

    pub(crate) fn take_inbound_session(&mut self, call_id: CallId) -> Option<InboundSessionMeta> {
        let sid = self.inbound_by_call.remove(&call_id)?;
        self.inbound_sessions.remove(&sid)
    }

    pub(crate) async fn flush_pending_post_assignment(
        &mut self,
        a1: &dyn MscA1Endpoint,
        call_id: CallId,
        controller: &mut MscCallController,
        send_tones_alert: bool,
    ) {
        let Some(pending) = self.pending_post_assignment.remove(&call_id) else {
            return;
        };
        if pending.send_alert && send_tones_alert && self.alert_sent.insert(call_id) {
            send_progress_ringback(a1, call_id, controller).await;
        }
    }

    pub(crate) fn next_pending_clear_deadline(&self) -> Option<tokio::time::Instant> {
        self.pending_clears.values().map(|p| p.deadline).min()
    }

    pub(crate) fn drain_due_pending_clears(&mut self) -> Vec<(CallId, GatewayClearCause)> {
        let now = tokio::time::Instant::now();
        let due: Vec<CallId> = self
            .pending_clears
            .iter()
            .filter_map(|(call_id, pc)| (pc.deadline <= now).then_some(*call_id))
            .collect();
        due.into_iter()
            .filter_map(|cid| self.pending_clears.remove(&cid).map(|pc| (cid, pc.cause)))
            .collect()
    }

    /// Pre-answer failure: Progress+Signal tone (A.S0014-D §2.1.6) then
    /// delayed ClearCommand (§3.1.14).
    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn signal_call_failure(
        &mut self,
        a1: &dyn MscA1Endpoint,
        call_id: CallId,
        controller: &mut MscCallController,
        circuits: &mut CircuitService,
        media: &mut MediaService,
        _voice_bearer: Option<&Arc<VoiceBearerManager>>,
        failure_tone_duration_ms: u64,
        cause: ReleaseCause,
        sip_status: Option<u32>,
    ) {
        if !self.failure_signaled.insert(call_id) {
            return;
        }
        let clear_cause = gateway_clear_cause(cause, sip_status);
        media.stop_ringback_for_call(call_id, circuits);
        if failure_tone_duration_ms > 0 {
            let signal = signal_for_cause(&clear_cause);
            send_progress_with_signal(a1, call_id, controller, signal).await;
            self.pending_clears.insert(
                call_id,
                PendingClear {
                    deadline: tokio::time::Instant::now()
                        + std::time::Duration::from_millis(failure_tone_duration_ms),
                    cause: clear_cause,
                },
            );
        } else {
            send_gateway_clear_command(a1, call_id, controller, clear_cause).await;
        }
    }

    #[allow(clippy::too_many_arguments)]
    pub(crate) async fn handle_media_gateway_event(
        &mut self,
        a1: &dyn MscA1Endpoint,
        event: MediaGatewayEvent,
        controller: &mut MscCallController,
        circuits: &mut CircuitService,
        media: &mut MediaService,
        voice_bearer: Option<&Arc<VoiceBearerManager>>,
        media_gateway: Option<&Arc<dyn MediaGatewayClient>>,
        media_ringback_enabled: bool,
        media_ringback_type: crate::config::MediaRingbackType,
        sip_ringback_disable: bool,
        generate_ringback: bool,
        send_tones_alert: bool,
        failure_tone_duration_ms: u64,
        hlr_repo: Option<&Arc<dyn cdma_hlr::repository::HlrRepository>>,
    ) {
        match event {
            MediaGatewayEvent::Ringing {
                handle,
                sip_status,
                codec,
            } => {
                debug!(
                    "MSC: media gateway ringing handle={:?} sip_status={} codec={} disable_local_rb={}",
                    handle, sip_status, codec, sip_ringback_disable
                );
                let Some(call_id) = self.media_gateway_calls.get(&handle).copied() else {
                    return;
                };
                if sip_ringback_disable || (!generate_ringback && !send_tones_alert) {
                    return;
                }
                if !ready_for_alert(controller.state(call_id)) {
                    // Defer the Signal IE until primary AssignmentComplete
                    // (flushed in `flush_pending_post_assignment`); bearer
                    // ringback can start immediately on the circuit's bearer.
                    self.pending_post_assignment
                        .entry(call_id)
                        .or_default()
                        .send_alert = send_tones_alert;
                    if generate_ringback {
                        media.start_ringback_for_call(
                            call_id,
                            controller,
                            circuits,
                            voice_bearer,
                            media_ringback_enabled,
                            media_ringback_type,
                            hlr_repo,
                        );
                    }
                    return;
                }
                // A bare AWIM toward the caller is ambiguous — some MS
                // firmwares interpret it as an incoming-call alert. Only the
                // Ringback Signal IE is unambiguous.
                if send_tones_alert && self.alert_sent.insert(call_id) {
                    send_progress_ringback(a1, call_id, controller).await;
                }
                if generate_ringback {
                    media.start_ringback_for_call(
                        call_id,
                        controller,
                        circuits,
                        voice_bearer,
                        media_ringback_enabled,
                        media_ringback_type,
                        hlr_repo,
                    );
                }
            }
            MediaGatewayEvent::Answered {
                handle,
                sip_status,
                codec,
            } => {
                info!(
                    "MSC: media gateway answered handle={:?} sip_status={} codec={} disable_local_rb={}",
                    handle, sip_status, codec, sip_ringback_disable
                );
                let Some(call_id) = self.media_gateway_calls.get(&handle).copied() else {
                    return;
                };
                media.stop_ringback_for_call(call_id, circuits);
                self.answered_locally.insert(call_id);
                // Tones Off on answer — kills any ringback tone the MS is
                // playing before bearer audio flows. The 200 OK itself is
                // implicit per A.S0014-D §2.1.8 (no Connect to BSC).
                send_progress_tones_off(a1, call_id, controller).await;
            }
            MediaGatewayEvent::Failed {
                handle,
                sip_status,
                cause,
                reason,
            } => {
                warn!(
                    "MSC: media gateway failed handle={:?} sip_status={:?} cause={:?}: {}",
                    handle, sip_status, cause, reason
                );
                let Some(call_id) = self.media_gateway_calls.remove(&handle) else {
                    return;
                };
                let already_answered = self.answered_locally.contains(&call_id);
                if already_answered {
                    let clear_cause = gateway_clear_cause(cause, sip_status);
                    send_gateway_clear_command(a1, call_id, controller, clear_cause).await;
                    stop_media_for_call(
                        call_id,
                        controller,
                        circuits,
                        media,
                        self,
                        voice_bearer,
                        media_gateway,
                    );
                } else {
                    self.signal_call_failure(
                        a1,
                        call_id,
                        controller,
                        circuits,
                        media,
                        voice_bearer,
                        failure_tone_duration_ms,
                        cause,
                        sip_status,
                    )
                    .await;
                }
            }
            MediaGatewayEvent::Released { handle, cause } => {
                info!(
                    "MSC: media gateway released handle={:?} cause={:?}",
                    handle, cause
                );
                if let Some(call_id) = self.media_gateway_calls.remove(&handle) {
                    let clear_cause = gateway_clear_cause(cause, None);
                    send_gateway_clear_command(a1, call_id, controller, clear_cause).await;
                    stop_media_for_call(
                        call_id,
                        controller,
                        circuits,
                        media,
                        self,
                        voice_bearer,
                        media_gateway,
                    );
                }
            }
            MediaGatewayEvent::InboundCall { .. } | MediaGatewayEvent::InboundCancel { .. } => {
                // Routed directly by the runtime event loop.
                debug!("MSC: media_gw event arm reached unexpectedly for inbound SIP variant");
            }
            MediaGatewayEvent::MediaFrame {
                handle,
                payload,
                sequence,
                service_option: _,
            } => {
                let Some(call_id) = self.media_gateway_calls.get(&handle).copied() else {
                    return;
                };
                media.stop_ringback_for_call(call_id, circuits);
                let Some((&circuit_id, session)) = circuits
                    .circuits
                    .iter()
                    .find(|(_, session)| session.call_id == call_id)
                else {
                    return;
                };
                // Bearer remote isn't known until AssignmentComplete; drop
                // pre-AC SIP frames silently instead of warning per frame.
                if !session.bearer_remote_ready {
                    debug!(
                        "MSC: dropping gateway media frame (bearer not ready) handle={:?} call_id={} circuit_id={} seq={}",
                        handle, call_id.0, circuit_id, sequence
                    );
                    return;
                }
                let Some(codec) =
                    cdma_voice::VoiceCodec::from_service_option(session.service_option)
                else {
                    warn!(
                        "MSC: dropping gateway media for unsupported service option {}",
                        session.service_option
                    );
                    return;
                };
                let Some(rate) = codec.rate_from_bps(payload.rate_bps) else {
                    warn!(
                        "MSC: dropping gateway media rate {} for {:?}",
                        payload.rate_bps, codec
                    );
                    return;
                };
                let expected_bits = codec.primary_traffic_bits(rate);
                if payload.payload.len() != expected_bits {
                    warn!(
                        "MSC: dropping gateway media with {} bits, expected {} for {:?} {:?}",
                        payload.payload.len(),
                        expected_bits,
                        codec,
                        rate
                    );
                    return;
                }
                let packed_payload = cdma_voice::pack_voice_bits(&payload.payload, expected_bits);
                send_forward_bearer_frame(
                    &VoiceBearerFrame {
                        circuit_id,
                        rate_bps: payload.rate_bps,
                        payload: packed_payload,
                    },
                    voice_bearer,
                )
                .await;
                debug!(
                    "MSC: forwarded gateway media handle={:?} call_id={} circuit_id={} seq={}",
                    handle, call_id.0, circuit_id, sequence
                );
            }
        }
    }

    pub(crate) fn cleanup_call(
        &mut self,
        call_id: CallId,
        media_gateway: Option<&Arc<dyn MediaGatewayClient>>,
    ) {
        self.alert_sent.remove(&call_id);
        let answered = self.answered_locally.remove(&call_id);
        self.failure_signaled.remove(&call_id);
        self.pending_clears.remove(&call_id);
        self.pending_post_assignment.remove(&call_id);
        if let Some(sub) = self.subscriber_by_call.remove(&call_id) {
            self.active_subscribers.remove(&sub);
        }
        if let Some(meta) = self.take_inbound_session(call_id)
            && !answered
            && let Some(gateway) = media_gateway.cloned()
        {
            let session_id = meta.session_id;
            tokio::spawn(async move {
                let _ = gateway.inbound_reject(&session_id, 487).await;
            });
        }
        let gateway_handles: Vec<CallHandle> = self
            .media_gateway_calls
            .iter()
            .filter_map(|(handle, cid)| (*cid == call_id).then_some(*handle))
            .collect();
        for handle in gateway_handles {
            self.media_gateway_calls.remove(&handle);
            if let Some(gateway) = media_gateway.cloned() {
                tokio::spawn(async move {
                    let _ = gateway
                        .release_call(handle, ReleaseCause::RadioReleased)
                        .await;
                });
            }
        }
    }
}

/// C.S0005-E §3.7.5.5 SIGNAL = Ringback (0x01).
pub(crate) async fn send_progress_ringback(
    a1: &dyn MscA1Endpoint,
    call_id: CallId,
    controller: &mut MscCallController,
) {
    send_progress_with_signal(
        a1,
        call_id,
        controller,
        Signal {
            signal_value: 0x01,
            alert_pitch: 0,
        },
    )
    .await;
}

/// C.S0005-E §3.7.5.5 SIGNAL = Tones Off (0x3F). Sent on call answer/connect
/// so the MS stops any in-progress ringback or progress tone.
pub(crate) async fn send_progress_tones_off(
    a1: &dyn MscA1Endpoint,
    call_id: CallId,
    controller: &mut MscCallController,
) {
    send_progress_with_signal(
        a1,
        call_id,
        controller,
        Signal {
            signal_value: 0x3F,
            alert_pitch: 0,
        },
    )
    .await;
}

async fn send_progress_with_signal(
    a1: &dyn MscA1Endpoint,
    call_id: CallId,
    controller: &mut MscCallController,
    signal: Signal,
) {
    let msg = ProgressMessage {
        signal: Some(signal),
        ms_information_records: None,
    };
    if let Err(error) = controller.apply_from_msc(call_id, &ProcedureMessage::Progress(msg.clone()))
    {
        warn!(
            "MSC: failed to apply Progress state call_id={}: {}",
            call_id.0, error
        );
    }
    let payload = match msg.encode() {
        Ok(payload) => payload,
        Err(error) => {
            warn!(
                "MSC: failed to encode Progress call_id={}: {}",
                call_id.0, error
            );
            return;
        }
    };
    info!(
        "MSC: A1 tx Progress call_id={} signal=0x{:02x}",
        call_id.0, signal.signal_value
    );
    if let Err(error) = a1
        .send_to_bsc(EncodedA1Message::from_message_for_call(
            &Message::new(MessageType::Progress, payload),
            Some(call_id.0),
        ))
        .await
    {
        warn!(
            "MSC: failed to send Progress call_id={}: {}",
            call_id.0, error
        );
    }
}

/// Always Busy (C.S0005-E §3.7.5.5 SIGNAL=0x04). Spec defines reorder, intercept,
/// etc., but most CDMA handsets only implement Busy; the Q.931 cause is still
/// delivered via `ClearCommand.cause_layer3`.
fn signal_for_cause(_cause: &GatewayClearCause) -> Signal {
    Signal {
        signal_value: 0x04,
        alert_pitch: 0,
    }
}

pub(crate) async fn send_alert_with_information(
    a1: &dyn MscA1Endpoint,
    call_id: CallId,
    controller: &mut MscCallController,
    already_sent: &mut HashSet<CallId>,
    ms_information_records: Option<MsInformationRecords>,
) {
    if !already_sent.insert(call_id) {
        return;
    }
    let msg = AlertWithInformationMessage {
        ms_information_records,
    };
    if let Err(error) = controller.apply_from_msc(
        call_id,
        &ProcedureMessage::AlertWithInformation(msg.clone()),
    ) {
        warn!(
            "MSC: failed to apply AlertWithInformation state call_id={}: {}",
            call_id.0, error
        );
    }
    let payload = match msg.encode() {
        Ok(payload) => payload,
        Err(error) => {
            warn!(
                "MSC: failed to encode AlertWithInformation call_id={}: {}",
                call_id.0, error
            );
            return;
        }
    };
    info!(
        "MSC: A1 tx AlertWithInformation for media-gateway progress call_id={}",
        call_id.0
    );
    if let Err(error) = a1
        .send_to_bsc(EncodedA1Message::from_message_for_call(
            &Message::new(MessageType::AlertWithInformation, payload),
            Some(call_id.0),
        ))
        .await
    {
        warn!(
            "MSC: failed to send AlertWithInformation call_id={}: {}",
            call_id.0, error
        );
    }
}

pub(crate) fn stop_media_for_call(
    call_id: CallId,
    _controller: &mut MscCallController,
    circuits: &mut CircuitService,
    media: &mut MediaService,
    media_gw: &mut MediaGatewayService,
    voice_bearer: Option<&Arc<VoiceBearerManager>>,
    media_gateway: Option<&Arc<dyn MediaGatewayClient>>,
) {
    media_gw.cleanup_call(call_id, media_gateway);
    let circuit_ids = circuits.cleanup_call(call_id, voice_bearer);
    media.cleanup_call(call_id, &circuit_ids);
}

#[derive(Debug, Clone, Copy)]
pub(crate) struct GatewayClearCause {
    pub(crate) a1_cause: cdma_ios::Cause,
    pub(crate) layer3: Option<cdma_ios::CauseLayer3>,
}

pub(crate) async fn send_gateway_clear_command(
    a1: &dyn MscA1Endpoint,
    call_id: CallId,
    controller: &mut MscCallController,
    cause: GatewayClearCause,
) {
    let clear_command = cdma_ios::ClearCommandMessage {
        cause: cause.a1_cause,
        cause_layer3: cause.layer3,
    };
    if let Err(error) = controller.apply_from_msc(
        call_id,
        &cdma_ios::ProcedureMessage::ClearCommand(clear_command.clone()),
    ) {
        warn!(
            "MSC: failed to apply media-gateway Clear Command state call_id={}: {}",
            call_id.0, error
        );
    }
    let payload = match clear_command.encode() {
        Ok(payload) => payload,
        Err(error) => {
            warn!(
                "MSC: failed to encode media-gateway Clear Command call_id={}: {}",
                call_id.0, error
            );
            return;
        }
    };
    info!(
        "MSC: A1 tx ClearCommand after media-gateway release call_id={}",
        call_id.0
    );
    if let Err(error) = a1
        .send_to_bsc(EncodedA1Message::from_message_for_call(
            &cdma_ios::Message::new(cdma_ios::MessageType::ClearCommand, payload),
            Some(call_id.0),
        ))
        .await
    {
        warn!(
            "MSC: failed to send media-gateway Clear Command call_id={}: {}",
            call_id.0, error
        );
    }
}

pub(crate) fn gateway_clear_cause(
    cause: ReleaseCause,
    sip_status: Option<u32>,
) -> GatewayClearCause {
    GatewayClearCause {
        a1_cause: cdma_ios::Cause(0x09),
        layer3: Some(cdma_ios::CauseLayer3 {
            coding_standard: 0,
            location: 2,
            cause_value: gateway_q931_cause(cause, sip_status),
        }),
    }
}

fn gateway_q931_cause(cause: ReleaseCause, sip_status: Option<u32>) -> u8 {
    if let Some(status) = sip_status {
        match status {
            400 | 404 | 484 => return 28,
            401 | 403 | 407 => return 21,
            408 | 480 => return 18,
            486 | 600 => return 17,
            487 => return 31,
            488 | 606 => return 58,
            500 | 502 | 503 | 504 => return 41,
            _ if (400..500).contains(&status) => return 31,
            _ if (500..600).contains(&status) => return 41,
            _ if status >= 600 => return 21,
            _ => {}
        }
    }

    match cause {
        ReleaseCause::RadioReleased | ReleaseCause::RemoteReleased => 16,
        ReleaseCause::SipFailure | ReleaseCause::SetupFailed => 41,
        ReleaseCause::GatewayTimeout => 18,
        ReleaseCause::MediaError => 41,
        ReleaseCause::Administrative => 31,
    }
}

/// Sends a forward voice bearer frame toward the BSC (MSC->mobile).
pub(crate) async fn send_forward_bearer_frame(
    frame: &VoiceBearerFrame,
    voice_bearer: Option<&Arc<VoiceBearerManager>>,
) {
    if let Some(bearer) = voice_bearer {
        if let Err(e) = bearer.send_frame(frame).await {
            warn!(
                "MSC: failed to send forward bearer frame circuit_id={}: {}",
                frame.circuit_id, e
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::call_control::CallDirection;
    use crate::config::MediaRingbackType;
    use async_trait::async_trait;
    use cdma_ios::A1TransportError;
    use std::sync::Mutex;

    struct CapturingA1 {
        sent: Mutex<Vec<EncodedA1Message>>,
    }

    impl CapturingA1 {
        fn new() -> Self {
            Self {
                sent: Mutex::new(Vec::new()),
            }
        }

        fn message_types(&self) -> Vec<MessageType> {
            self.sent
                .lock()
                .unwrap()
                .iter()
                .map(|m| m.message_type())
                .collect()
        }
    }

    #[async_trait]
    impl MscA1Endpoint for CapturingA1 {
        async fn recv_from_bsc(&self) -> Option<EncodedA1Message> {
            None
        }
        async fn send_to_bsc(&self, message: EncodedA1Message) -> Result<(), A1TransportError> {
            self.sent.lock().unwrap().push(message);
            Ok(())
        }
    }

    fn register_call(
        svc: &mut MediaGatewayService,
        controller: &mut MscCallController,
    ) -> CallHandle {
        let call_id = controller.create_call(CallDirection::MobileOriginated, None);
        let handle = CallHandle(0xa11);
        controller
            .attach_media_gateway_handle(call_id, handle)
            .unwrap();
        svc.media_gateway_calls.insert(handle, call_id);
        // Drive the call through the MO setup procedure so its state reaches
        // `Assigned` — the precondition for AWIM/Progress per IS-2000 IOS.
        controller
            .apply_from_bsc(
                call_id,
                &cdma_ios::ProcedureMessage::CompleteLayer3Information(
                    cdma_ios::CompleteLayer3InformationMessage {
                        cell_identifier: cdma_ios::CellId {
                            cell: 0x123,
                            sector: 0x4,
                        },
                        layer3_information: cdma_ios::Layer3Information(vec![
                            0x03, 0x00, 0x24, 0x01,
                        ]),
                    },
                ),
            )
            .unwrap();
        controller
            .apply_from_msc(
                call_id,
                &cdma_ios::ProcedureMessage::AssignmentRequest(
                    cdma_ios::AssignmentRequestMessage {
                        channel_type: cdma_ios::ChannelType {
                            speech_or_data_indicator: 0x01,
                            channel_rate_and_type: 0x08,
                            coding: 0x05,
                        },
                        circuit_identity_code: cdma_ios::CircuitIdentityCode {
                            pcm_multiplexer: 0x0123,
                            timeslot: 0x1a,
                        },
                        encryption_information: None,
                        service_option: Some(cdma_ios::ServiceOption(0x0003)),
                        signals: Vec::new(),
                        ms_information_records: None,
                        priority: None,
                        paca_timestamp: None,
                        quality_of_service_parameters: None,
                        a2p_bearer_session_params: None,
                        a2p_bearer_format_params: None,
                    },
                ),
            )
            .unwrap();
        controller
            .apply_from_bsc(
                call_id,
                &cdma_ios::ProcedureMessage::AssignmentComplete(
                    cdma_ios::AssignmentCompleteMessage {
                        channel_number: cdma_ios::ChannelNumber(0x1122),
                        encryption_information: None,
                        service_option: Some(cdma_ios::ServiceOption(0x0003)),
                        a2p_bearer_session_params: None,
                        a2p_bearer_format_params: None,
                    },
                ),
            )
            .unwrap();
        handle
    }

    async fn fire_ringing(
        svc: &mut MediaGatewayService,
        a1: &dyn MscA1Endpoint,
        controller: &mut MscCallController,
        handle: CallHandle,
        sip_ringback_disable: bool,
    ) {
        let mut circuits = CircuitService::new();
        let mut media = MediaService::new();
        svc.handle_media_gateway_event(
            a1,
            MediaGatewayEvent::Ringing {
                handle,
                sip_status: 180,
                codec: String::new(),
            },
            controller,
            &mut circuits,
            &mut media,
            None,
            None,
            false,
            MediaRingbackType::Nanp,
            sip_ringback_disable,
            true,
            false,
            0,
            None,
        )
        .await;
    }

    async fn fire_answered(
        svc: &mut MediaGatewayService,
        a1: &dyn MscA1Endpoint,
        controller: &mut MscCallController,
        handle: CallHandle,
        sip_ringback_disable: bool,
    ) {
        let mut circuits = CircuitService::new();
        let mut media = MediaService::new();
        svc.handle_media_gateway_event(
            a1,
            MediaGatewayEvent::Answered {
                handle,
                sip_status: 200,
                codec: "PCMU".to_string(),
            },
            controller,
            &mut circuits,
            &mut media,
            None,
            None,
            false,
            MediaRingbackType::Nanp,
            sip_ringback_disable,
            true,
            false,
            0,
            None,
        )
        .await;
    }

    #[tokio::test]
    async fn ringback_enabled_path_is_silent_on_ringing_without_tones_alert() {
        // Caller-side A1 AWI is suppressed (bare AWIM confuses MS firmware
        // into thinking it's the alerted party). MSC's bearer ringback feeder
        // provides audible ringback; A1 stays quiet until Answer's Tones-Off.
        let a1 = CapturingA1::new();
        let mut svc = MediaGatewayService::new();
        let mut controller = MscCallController::new();
        let handle = register_call(&mut svc, &mut controller);

        fire_ringing(&mut svc, &a1, &mut controller, handle, false).await;
        assert!(a1.message_types().is_empty());

        fire_answered(&mut svc, &a1, &mut controller, handle, false).await;
        assert_eq!(
            a1.message_types(),
            vec![MessageType::Progress],
            "Answered drives a single Tones-Off Progress"
        );
    }

    #[tokio::test]
    async fn ringback_disabled_path_only_emits_tones_off_on_answer() {
        let a1 = CapturingA1::new();
        let mut svc = MediaGatewayService::new();
        let mut controller = MscCallController::new();
        let handle = register_call(&mut svc, &mut controller);

        fire_ringing(&mut svc, &a1, &mut controller, handle, true).await;
        assert!(a1.message_types().is_empty());

        fire_answered(&mut svc, &a1, &mut controller, handle, true).await;
        assert_eq!(a1.message_types(), vec![MessageType::Progress]);
    }

    #[tokio::test]
    async fn repeated_ringing_is_idempotent() {
        let a1 = CapturingA1::new();
        let mut svc = MediaGatewayService::new();
        let mut controller = MscCallController::new();
        let handle = register_call(&mut svc, &mut controller);

        // 180 Ringing followed by 183 Session Progress both surface as Ringing;
        // neither emits A1 in the default (send_tones_alert=false) mode.
        fire_ringing(&mut svc, &a1, &mut controller, handle, false).await;
        fire_ringing(&mut svc, &a1, &mut controller, handle, false).await;
        assert!(a1.message_types().is_empty());
    }

    async fn fire_failed(
        svc: &mut MediaGatewayService,
        a1: &dyn MscA1Endpoint,
        controller: &mut MscCallController,
        handle: CallHandle,
        sip_status: u32,
    ) {
        let mut circuits = CircuitService::new();
        let mut media = MediaService::new();
        svc.handle_media_gateway_event(
            a1,
            MediaGatewayEvent::Failed {
                handle,
                sip_status: Some(sip_status),
                cause: ReleaseCause::SipFailure,
                reason: format!("SIP {sip_status}"),
            },
            controller,
            &mut circuits,
            &mut media,
            None,
            None,
            false,
            MediaRingbackType::Nanp,
            false,
            true,
            false,
            // failure_tone_duration_ms = 0 → skip tone, send Clear directly.
            0,
            None,
        )
        .await;
    }

    #[tokio::test]
    async fn sip_403_emits_clearcommand_directly_when_tone_disabled() {
        // failure_tone_duration_ms = 0 → no Progress tone, just ClearCommand.
        let a1 = CapturingA1::new();
        let mut svc = MediaGatewayService::new();
        let mut controller = MscCallController::new();
        let handle = register_call(&mut svc, &mut controller);
        fire_failed(&mut svc, &a1, &mut controller, handle, 403).await;
        assert_eq!(
            a1.message_types(),
            vec![MessageType::ClearCommand],
            "with tone disabled, SIP 403 must clear immediately with cause"
        );
    }

    #[tokio::test]
    async fn duplicate_failed_event_is_idempotent() {
        let a1 = CapturingA1::new();
        let mut svc = MediaGatewayService::new();
        let mut controller = MscCallController::new();
        let handle = register_call(&mut svc, &mut controller);
        fire_failed(&mut svc, &a1, &mut controller, handle, 403).await;
        // Re-register the handle to exercise duplicate failure suppression.
        svc.media_gateway_calls.insert(
            handle,
            controller.snapshot(CallId(1)).map(|s| s.id).unwrap(),
        );
        fire_failed(&mut svc, &a1, &mut controller, handle, 403).await;
        assert_eq!(
            a1.message_types(),
            vec![MessageType::ClearCommand],
            "duplicate failure must not re-send ClearCommand"
        );
    }

    #[test]
    fn signal_busy_for_all_failure_causes() {
        // Most handsets ignore other Tone-group signals; keep failures Busy.
        for status in [403_u32, 404, 408, 480, 486, 487, 500, 503, 600] {
            let cause = gateway_clear_cause(ReleaseCause::SipFailure, Some(status));
            assert_eq!(
                signal_for_cause(&cause).signal_value,
                0x04,
                "SIP {status} should still map to Busy"
            );
        }
    }

    #[tokio::test]
    async fn answer_does_not_send_msc_to_bsc_connect() {
        // Per A.S0014-D §2.1.8, MO answer is implicit — MSC must not send
        // Connect to the BSC; conversation state is reached when bearer audio
        // flows. MSC does send a Tones-Off Progress on Answer (to silence any
        // ringback the MS is playing), but it is *not* a Connect message.
        let a1 = CapturingA1::new();
        let mut svc = MediaGatewayService::new();
        let mut controller = MscCallController::new();
        let handle = register_call(&mut svc, &mut controller);

        fire_ringing(&mut svc, &a1, &mut controller, handle, false).await;
        fire_answered(&mut svc, &a1, &mut controller, handle, false).await;
        fire_answered(&mut svc, &a1, &mut controller, handle, false).await;
        assert!(
            !a1.message_types().contains(&MessageType::Connect),
            "Answered must not produce any MscToBsc Connect (per IS-2000 IOS); got {:?}",
            a1.message_types()
        );
    }
}
