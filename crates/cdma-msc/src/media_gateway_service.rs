//! Media gateway event handling for the MSC runtime.
//!
//! Owns the mapping from media gateway call handles to MSC call IDs, and
//! processes gateway events (ringing, answered, failed, released, media frames).

use std::collections::HashMap;
use std::sync::Arc;

use log::{debug, info, warn};

use cdma_ios::{EncodedA1Message, VoiceBearerFrame, VoiceBearerManager};

use crate::call_control::{CallId, MscCallController};
use crate::circuit::CircuitService;
use crate::media::MediaService;
use crate::media_gateway::{CallHandle, MediaGatewayClient, MediaGatewayEvent, ReleaseCause};
use crate::runtime::MscA1Endpoint;

/// Manages media gateway call handle correlation and gateway-initiated cleanup.
pub(crate) struct MediaGatewayService {
    /// Media-gateway handle -> call ID.
    pub(crate) media_gateway_calls: HashMap<CallHandle, CallId>,
}

impl MediaGatewayService {
    pub(crate) fn new() -> Self {
        Self {
            media_gateway_calls: HashMap::new(),
        }
    }

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
        hlr_repo: Option<&Arc<dyn cdma_hlr::repository::HlrRepository>>,
    ) {
        match event {
            MediaGatewayEvent::Ringing { handle, sip_status } => {
                debug!(
                    "MSC: media gateway ringing handle={:?} sip_status={}",
                    handle, sip_status
                );
                if let Some(call_id) = self.media_gateway_calls.get(&handle).copied() {
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
                    "MSC: media gateway answered handle={:?} sip_status={} codec={}",
                    handle, sip_status, codec
                );
                if let Some(call_id) = self.media_gateway_calls.get(&handle).copied() {
                    media.stop_ringback_for_call(call_id, circuits);
                }
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
                if let Some(call_id) = self.media_gateway_calls.remove(&handle) {
                    send_gateway_clear_command(
                        a1,
                        call_id,
                        controller,
                        gateway_clear_cause(cause, sip_status),
                    )
                    .await;
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
            MediaGatewayEvent::Released { handle, cause } => {
                info!(
                    "MSC: media gateway released handle={:?} cause={:?}",
                    handle, cause
                );
                if let Some(call_id) = self.media_gateway_calls.remove(&handle) {
                    send_gateway_clear_command(
                        a1,
                        call_id,
                        controller,
                        gateway_clear_cause(cause, None),
                    )
                    .await;
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
                let Some((&circuit_id, _)) = circuits
                    .circuits
                    .iter()
                    .find(|(_, session)| session.call_id == call_id)
                else {
                    return;
                };
                send_forward_bearer_frame(
                    &VoiceBearerFrame {
                        circuit_id,
                        rate_bps: payload.rate_bps,
                        payload: payload.payload,
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

    /// Clean up all gateway state associated with a call.
    pub(crate) fn cleanup_call(
        &mut self,
        call_id: CallId,
        media_gateway: Option<&Arc<dyn MediaGatewayClient>>,
    ) {
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

/// Combined cleanup across all services for a call being released.
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
        return;
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
