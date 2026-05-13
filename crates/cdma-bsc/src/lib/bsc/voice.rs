//! BSC voice radio-leg helpers.

use std::time::{Duration, Instant};

use cdma_abis::control::typed::CallConnectionReference;
use cdma_common::channel::TrafficRate;
use cdma_common::error::Error;
use cdma_common::lac::{
    message_types::MessageId,
    paging_messages::{
        AlertWithInformationMessage, CallingPartyNumberRecord, MsAddress, SignalInfoRecord,
    },
};
use cdma_common::overhead::OverheadParameters;
use log::{info, warn};
use uuid::Uuid;

use crate::addressing::format_ms_address;
use crate::voice_bearer_bits::{mux_voice_bits_for_air, normalize_air_voice_bits};

use super::{
    A1ClearState, Bsc, DEFAULT_PAGE_TIMEOUT_MS, MsState, PendingVoicePage, VoicePollAction,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VoiceAlertMode {
    WaitForConnectOrder,
    WaitForPeerAnswer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VoiceSessionKind {
    MobileOriginatedExternal,
    MscControlledMt,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum VoiceLegRole {
    Caller,
    Callee,
}

#[derive(Debug, Clone)]
pub(crate) struct VoiceCallParty {
    pub(crate) address: MsAddress,
    pub(crate) subscriber_id: Option<Uuid>,
    pub(crate) phone_number: Option<String>,
    pub(crate) walsh_code: Option<u8>,
    pub(crate) service_connected: bool,
    pub(crate) answered: bool,
}

#[derive(Debug, Clone)]
pub(crate) struct VoiceCallSession {
    pub(crate) id: Uuid,
    pub(crate) kind: VoiceSessionKind,
    pub(crate) service_option: u16,
    pub(crate) caller: Option<VoiceCallParty>,
    pub(crate) callee: Option<VoiceCallParty>,
    /// Explicit caller ID digits for the AWIM Calling Party Number record.
    /// For BS-originated calls this comes from the UI; for M2M it's the
    /// caller's phone_number.
    pub(crate) caller_number: Option<String>,
    /// Dialed external number for MSC-controlled mobile-originated calls.
    pub(crate) called_number: Option<String>,
}

pub(crate) struct VoiceService {
    pub(crate) sessions: Vec<VoiceCallSession>,
    pub(crate) next_call_connection_ref: u32,
    pub(crate) next_mo_call_id: u64,
}

impl Default for VoiceService {
    fn default() -> Self {
        Self {
            sessions: Vec::new(),
            next_call_connection_ref: 1,
            next_mo_call_id: 0x1_0000_0000,
        }
    }
}

impl VoiceService {
    pub(crate) fn session(&self, session_id: Uuid) -> Option<&VoiceCallSession> {
        self.sessions
            .iter()
            .find(|session| session.id == session_id)
    }

    pub(crate) fn session_mut(&mut self, session_id: Uuid) -> Option<&mut VoiceCallSession> {
        self.sessions
            .iter_mut()
            .find(|session| session.id == session_id)
    }

    pub(crate) fn push_session(&mut self, session: VoiceCallSession) {
        self.sessions.push(session);
    }

    pub(crate) fn retain_sessions(&mut self, mut keep: impl FnMut(&VoiceCallSession) -> bool) {
        self.sessions.retain(|session| keep(session));
    }

    pub(crate) fn allocate_mo_call_id(&mut self) -> u64 {
        let id = self.next_mo_call_id;
        self.next_mo_call_id = id.wrapping_add(1);
        id
    }

    pub(crate) fn allocate_call_connection_ref(
        &mut self,
        overhead: &OverheadParameters,
    ) -> CallConnectionReference {
        let ccr = self.next_call_connection_ref;
        self.next_call_connection_ref = ccr.wrapping_add(1);
        CallConnectionReference {
            market_id: overhead.sid,
            generating_entity_id: overhead.base_id,
            call_connection_reference: ccr,
        }
    }
}

pub(crate) fn traffic_rate_from_bps(rate_bps: u32) -> Option<TrafficRate> {
    match rate_bps {
        9600 => Some(TrafficRate::Full),
        4800 => Some(TrafficRate::Half),
        2400 | 2700 => Some(TrafficRate::Quarter),
        1200 | 1500 => Some(TrafficRate::Eighth),
        _ => None,
    }
}

pub(crate) fn peer_role(role: VoiceLegRole) -> VoiceLegRole {
    match role {
        VoiceLegRole::Caller => VoiceLegRole::Callee,
        VoiceLegRole::Callee => VoiceLegRole::Caller,
    }
}

impl Bsc {
    pub(crate) fn begin_voice_release(
        &mut self,
        fwd_address: &cdma_common::lac::paging_messages::MsAddress,
        ack_seq: u8,
        reason: &str,
    ) {
        let Some(target) = self
            .mobiles
            .get(fwd_address)
            .and_then(|ms| ms.voice_release_target())
        else {
            return;
        };
        let walsh_code = target.walsh_code;
        if target.release_voice_service_only {
            info!(
                "BSC: releasing voice service connection on existing packet TCH walsh={} for {} ({})",
                walsh_code,
                format_ms_address(fwd_address),
                reason
            );
            self.mobiles.update_tc(walsh_code, |_, tc| {
                tc.clear_voice_service_connection();
            });
            if let Err(e) = self.send_service_request(walsh_code, ack_seq) {
                warn!(
                    "BSC: failed to send Service Request removing voice connection on walsh={}: {}",
                    walsh_code, e
                );
            } else {
                self.mobiles.update_tc(walsh_code, |_, tc| {
                    tc.mark_waiting_service_response();
                });
            }
            self.publish_mobiles();
            return;
        }
        info!(
            "BSC: initiating voice release on walsh={} for {} ({})",
            walsh_code,
            format_ms_address(fwd_address),
            reason
        );
        if let (Some(call_id), A1ClearState::Idle) = (target.a1_call_id, target.a1_clear_state) {
            self.a1.send_clear_request(call_id, 0);
            self.mobiles.update_tc(walsh_code, |_, tc| {
                tc.mark_a1_clear_request_sent();
            });
        }
        if let Err(e) = self.send_traffic_release_order(walsh_code, ack_seq) {
            warn!(
                "BSC: failed to send Release Order on walsh={} during {}: {}",
                walsh_code, reason, e
            );
        }
        self.mobiles.update_tc(walsh_code, |_, tc| {
            tc.mark_releasing();
        });
    }

    pub(crate) fn on_voice_leg_released(
        &mut self,
        session_id: Option<Uuid>,
        leg_role: Option<VoiceLegRole>,
    ) {
        let (Some(session_id), Some(leg_role)) = (session_id, leg_role) else {
            return;
        };

        // Gateway release is MSC-owned. The BSC only releases the radio leg.

        let peer_role = peer_role(leg_role);
        let peer_addr = self
            .mobiles
            .get_by_session_leg(session_id, peer_role)
            .map(|ms| ms.fwd_address.clone());
        if let Some(addr) = peer_addr {
            self.begin_voice_release(&addr, 0b111, "peer leg released");
        }

        if !self.mobiles.has_voice_session(session_id) {
            self.voice
                .retain_sessions(|session| session.id != session_id);
        }
    }

    // -----------------------------------------------------------------------
    // Voice call signaling
    // -----------------------------------------------------------------------

    /// Send Alert With Information Message (ringback) on the forward traffic
    /// channel. Per C.S0005-E 3.7.3.3.2.3, tells the MS to play a call
    /// progress tone (normal ringback for MO calls).
    pub(crate) fn send_alert_with_info(
        &mut self,
        walsh_code: u8,
        ack_seq: u8,
        caller_number: Option<&str>,
    ) -> Result<(), Error> {
        let mut awim = AlertWithInformationMessage::ringback();
        awim.calling_party = caller_number.map(|digits| {
            CallingPartyNumberRecord {
                number_type: 3,            // network-specific
                number_plan: 1,            // ISDN/telephony (E.164)
                presentation_indicator: 0, // presentation allowed
                screening_indicator: 3,    // network provided
                digits: digits.to_string(),
            }
        });
        let sdu = awim.to_ftch_sdu();

        info!(
            "BSC: sending Alert With Information (ringback) on F-TCH walsh={} ack_seq={}",
            walsh_code, ack_seq
        );

        self.send_traffic_signaling(
            walsh_code,
            sdu,
            MessageId::AlertWithInformation,
            ack_seq,
            true,
            None,
            None,
            None,
            None,
            Some(awim),
        )
    }

    /// Send AWIM "tones off" on the forward traffic channel.
    ///
    /// Per C.S0005-E Annex B call flow: after the answer delay, the BS sends
    /// an AWIM with SIGNAL_TYPE='00' (Tone), SIGNAL='111111' (Tones off) to
    /// stop ringback and transition the MS to conversation.
    pub(crate) fn send_tones_off(&mut self, walsh_code: u8, ack_seq: u8) -> Result<(), Error> {
        let awim = AlertWithInformationMessage {
            signal_info: Some(SignalInfoRecord {
                signal_type: 0x00, // Tone signal ('00')
                alert_pitch: 0x00, // ignored
                signal: 0x3F,      // Tones off ('111111')
            }),
            calling_party: None,
        };
        let sdu = awim.to_ftch_sdu();

        info!(
            "BSC: sending AWIM tones-off on F-TCH walsh={} ack_seq={}",
            walsh_code, ack_seq
        );

        self.send_traffic_signaling(
            walsh_code,
            sdu,
            MessageId::AlertWithInformation,
            ack_seq,
            true,
            None,
            None,
            None,
            None,
            Some(awim),
        )
    }

    /// Poll voice call timers and advance voice call state machines.
    ///
    /// Called periodically from the BSC run loop. Handles:
    /// - Service negotiation timeout
    /// - Release timeout
    /// - Guard release for any non-MSC-bridged voice leg
    pub(crate) async fn poll_voice_calls(&mut self) {
        let voice_policy = self.voice_policy();
        let service_connect_timeout =
            Duration::from_millis(voice_policy.service_connect_timeout_ms);
        let release_timeout = Duration::from_millis(voice_policy.release_timeout_ms);

        // Walsh codes uniquely identify each active voice traffic channel.
        let voice_walsh_codes = self.mobiles.active_voice_walsh_codes();

        for voice_walsh in voice_walsh_codes {
            let context = self.mobiles.get_by_walsh(voice_walsh).and_then(|ms| {
                let action = ms
                    .find_traffic_channel_by_walsh(voice_walsh)?
                    .voice_poll_action(service_connect_timeout, release_timeout);
                Some((ms.fwd_address.clone(), action))
            });
            let Some((fwd_address, action)) = context else {
                continue;
            };

            match action {
                VoicePollAction::ReleaseUnbridged => {
                    info!(
                        "BSC: unbridged local voice leg on walsh={}, sending Release Order",
                        voice_walsh
                    );
                    self.begin_voice_release(&fwd_address, 0b111, "unbridged local voice leg");
                }
                VoicePollAction::Teardown { reason, timeout_ms } => {
                    warn!(
                        "BSC: {} on walsh={} ({}ms), tearing down",
                        reason, voice_walsh, timeout_ms
                    );
                    let (session_id, leg_role) = self
                        .mobiles
                        .get_by_walsh(voice_walsh)
                        .and_then(|ms| ms.traffic_voice_context_by_walsh(voice_walsh))
                        .unwrap_or((None, None));
                    self.teardown_traffic_channel(voice_walsh).await;
                    self.on_voice_leg_released(session_id, leg_role);
                }
                VoicePollAction::None => {}
            }
        }
    }

    /// Compute the next deadline for voice call polling.
    ///
    /// Returns the earliest time at which a voice call state machine needs
    /// attention, or `None` if there are no active voice calls.
    pub(crate) fn next_voice_poll_deadline(&self) -> Option<tokio::time::Instant> {
        let voice_policy = self.voice_policy();
        let service_connect_timeout =
            Duration::from_millis(voice_policy.service_connect_timeout_ms);
        let release_timeout = Duration::from_millis(voice_policy.release_timeout_ms);
        // Poll at half-frame intervals when connected so the pre-fill loop
        // keeps the TX queue topped up even if previous polls were delayed.
        let connected_poll_interval = Duration::from_millis(5);

        let mut earliest: Option<Instant> = None;

        for ms in &self.mobiles {
            let Some(tc) = ms.voice_traffic_channel() else {
                continue;
            };
            let Some(deadline) = tc.next_voice_poll_deadline(
                service_connect_timeout,
                release_timeout,
                connected_poll_interval,
                Instant::now(),
            ) else {
                continue;
            };
            earliest = Some(earliest.map_or(deadline, |e: Instant| e.min(deadline)));
        }

        earliest.map(|e| tokio::time::Instant::from_std(e))
    }

    pub(crate) fn create_voice_party_from_mobile(
        &self,
        fwd_address: &cdma_common::lac::paging_messages::MsAddress,
    ) -> Option<VoiceCallParty> {
        let ms = self.mobiles.get(fwd_address)?;
        Some(VoiceCallParty {
            address: ms.fwd_address.clone(),
            subscriber_id: ms.subscriber_id,
            phone_number: ms.phone_number.clone(),
            walsh_code: None,
            service_connected: false,
            answered: false,
        })
    }

    pub(crate) fn note_msc_external_call_after_service_connect(
        &mut self,
        session_id: Uuid,
        _walsh_code: u8,
    ) {
        let Some(session) = self.voice.session(session_id).cloned() else {
            return;
        };
        let called_number = session.called_number.clone().unwrap_or_default();
        info!(
            "BSC: external MO call session={} to={} is MSC-owned after Service Connect",
            session_id, called_number
        );
    }

    /// Handles a forward voice bearer frame from the MSC and relays it to the BTS.
    pub(crate) fn handle_forward_bearer_frame(&mut self, frame: cdma_ios::VoiceBearerFrame) {
        let Some(rate) = traffic_rate_from_bps(frame.rate_bps) else {
            return;
        };
        let Some((fwd_address, walsh_code)) = self.mobiles.locate_msc_circuit(frame.circuit_id)
        else {
            log::debug!(
                "BSC: forward bearer frame for unknown circuit_id={}",
                frame.circuit_id,
            );
            return;
        };
        let voice_connected = self
            .mobiles
            .get(&fwd_address)
            .map(|ms| ms.is_voice_connected())
            .unwrap_or(false);
        let waiting_for_mt_connect = self
            .mobiles
            .get_traffic_channel(walsh_code)
            .is_some_and(|tc| tc.is_waiting_for_mt_connect_order());
        if waiting_for_mt_connect {
            log::debug!(
                "BSC: dropping pre-answer MSC bearer frame circuit_id={} walsh={} while waiting for MS Connect Order",
                frame.circuit_id,
                walsh_code
            );
            return;
        }
        if !voice_connected {
            if let Err(e) = self.send_tones_off(walsh_code, 0b111) {
                warn!(
                    "BSC: failed to send tones-off for MSC bearer circuit_id={} walsh={}: {}",
                    frame.circuit_id, walsh_code, e
                );
                self.begin_voice_release(&fwd_address, 0b111, "failed tones-off for MSC bearer");
                return;
            }
            self.mobiles.update_tc(walsh_code, |_, tc| {
                tc.mark_voice_connected(true);
            });
        }
        let Some((mux_bits, _)) = mux_voice_bits_for_air(&frame.payload, frame.rate_bps) else {
            return;
        };
        if let Err(e) = self.send_forward_fch_traffic_bits(walsh_code, mux_bits, rate) {
            log::warn!(
                "BSC: failed to relay MSC bearer frame to BTS walsh={}: {}",
                walsh_code,
                e
            );
        } else {
            log::debug!(
                "BSC: relayed MSC bearer frame circuit_id={} walsh={} rate_bps={} bits={}",
                frame.circuit_id,
                walsh_code,
                frame.rate_bps,
                frame.payload.len()
            );
        }
    }

    /// Relays a reverse voice frame to the MSC via the voice bearer transport.
    ///
    /// Returns `true` if the frame was sent (bearer configured and circuit_id known),
    /// `false` if the MSC-controlled frame must be dropped.
    pub(crate) fn relay_reverse_frame_to_msc(
        &self,
        fwd_address: &cdma_common::lac::paging_messages::MsAddress,
        bits: &[u8],
        rate_bps: u32,
    ) -> bool {
        let Some(bearer) = self.config.msc_voice_bearer.as_ref() else {
            return false;
        };
        let Some(circuit_id) = self
            .mobiles
            .get(fwd_address)
            .and_then(|ms| ms.msc_circuit_id())
        else {
            return false;
        };
        let normalized = normalize_air_voice_bits(bits, rate_bps);
        let frame = cdma_ios::VoiceBearerFrame {
            circuit_id,
            rate_bps,
            payload: normalized,
        };
        match bearer.try_send_frame(&frame) {
            Ok(_) => true,
            Err(e) => {
                log::warn!(
                    "BSC: failed to relay reverse voice frame to MSC circuit_id={}: {}",
                    circuit_id,
                    e
                );
                false
            }
        }
    }

    pub(crate) fn is_voice_session_msc_media_controlled(&self, session_id: Option<Uuid>) -> bool {
        let Some(session_id) = session_id else {
            return false;
        };
        self.mobiles.has_msc_media_for_session(session_id)
    }

    pub(crate) fn send_standard_alert(
        &mut self,
        walsh_code: u8,
        ack_seq: u8,
        caller_number: Option<&str>,
    ) -> Result<(), Error> {
        let awim = AlertWithInformationMessage {
            signal_info: Some(SignalInfoRecord {
                signal_type: 0x02,
                alert_pitch: 0x00,
                signal: 0x01,
            }),
            calling_party: caller_number.map(|digits| CallingPartyNumberRecord {
                number_type: 3,
                number_plan: 1,
                presentation_indicator: 0,
                screening_indicator: 3,
                digits: digits.to_string(),
            }),
        };
        let sdu = awim.to_ftch_sdu();
        self.send_traffic_signaling(
            walsh_code,
            sdu,
            MessageId::AlertWithInformation,
            ack_seq,
            true,
            None,
            None,
            None,
            None,
            Some(awim),
        )
    }

    pub(crate) fn queue_voice_page_for_mobile(
        &mut self,
        fwd_address: &cdma_common::lac::paging_messages::MsAddress,
        session_id: Uuid,
        service_option: u16,
        leg_role: VoiceLegRole,
        a1_tag: Option<cdma_ios::Tag>,
        a1_call_id: Option<u64>,
        imsi: Option<String>,
    ) {
        if self.paging.has_pending_page() {
            warn!("BSC: page already in progress — cannot queue voice page");
            return;
        }
        let Some((page_address, pgslot, slot_cycle_index)) =
            self.mobiles.get(fwd_address).and_then(|ms| {
                ms.page_address()
                    .map(|p| (p, ms.pgslot, ms.slot_cycle_index))
            })
        else {
            warn!("BSC: mobile has no pageable address for voice page");
            return;
        };
        let fwd_address = fwd_address.clone();
        self.mobiles.set_state(&fwd_address, MsState::Paged);
        let timeout = Duration::from_millis(DEFAULT_PAGE_TIMEOUT_MS);
        self.paging.queue_voice_page(PendingVoicePage {
            session_id,
            page_address: page_address.clone(),
            fwd_address,
            pgslot,
            slot_cycle_index,
            started_at: Instant::now(),
            timeout,
            retry_count: 0,
            next_retry_at: tokio::time::Instant::now(),
            last_target_chip: None,
            service_option,
            leg_role,
            a1_tag,
            a1_call_id,
            imsi,
            page_msg_seq: None,
            page_correlation_id: None,
        });
        self.publish_mobiles();
        match self.send_page_for_voice(
            &page_address,
            pgslot,
            slot_cycle_index,
            None,
            service_option,
            None,
        ) {
            Ok((target_chip, page_seq, page_correlation_id)) => {
                let next_retry_at =
                    self.compute_next_retry_at(pgslot, slot_cycle_index, target_chip);
                self.paging.record_voice_page_sent(
                    target_chip,
                    next_retry_at,
                    page_seq,
                    page_correlation_id,
                );
            }
            Err(e) => warn!("BSC: failed to send voice page: {}", e),
        }
    }

    pub(crate) fn start_bs_voice_call_for_mobile(
        &mut self,
        fwd_address: &cdma_common::lac::paging_messages::MsAddress,
        service_option: u16,
        caller_number: Option<String>,
        a1_tag: Option<cdma_ios::Tag>,
        a1_call_id: Option<u64>,
        imsi: Option<String>,
    ) {
        let session_id = Uuid::new_v4();
        let (subscriber_id, has_tc) = self
            .mobiles
            .get(fwd_address)
            .map(|ms| (ms.subscriber_id, ms.has_traffic_channel()))
            .unwrap_or((None, false));
        info!(
            "BSC: initiating MSC-controlled MT voice call session={} subscriber={:?} caller_number={:?}",
            session_id, subscriber_id, caller_number,
        );
        let callee = self.create_voice_party_from_mobile(fwd_address);
        self.voice.push_session(VoiceCallSession {
            id: session_id,
            kind: VoiceSessionKind::MscControlledMt,
            service_option,
            caller: None,
            callee,
            caller_number,
            called_number: None,
        });
        if has_tc {
            if let Some(call_id) = a1_call_id {
                if self.send_existing_traffic_paging_response(
                    call_id,
                    fwd_address,
                    service_option,
                    session_id,
                    VoiceLegRole::Callee,
                ) {
                    info!(
                        "BSC: answered A1 Paging Request from existing F-TCH for session={}",
                        session_id
                    );
                    self.publish_mobiles();
                    return;
                }
                warn!(
                    "BSC: failed to answer A1 Paging Request from existing F-TCH; falling back to local service negotiation"
                );
            }
            match self.start_mt_voice_on_existing_traffic(
                fwd_address,
                service_option,
                session_id,
                VoiceLegRole::Callee,
                a1_call_id,
            ) {
                Ok(walsh_code) => {
                    if let Some(session) = self.voice.session_mut(session_id) {
                        if let Some(callee) = session.callee.as_mut() {
                            callee.walsh_code = Some(walsh_code);
                        }
                    }
                    info!(
                        "BSC: initiated MT voice service negotiation on existing F-TCH walsh={} session={}",
                        walsh_code, session_id
                    );
                }
                Err(error) => {
                    warn!(
                        "BSC: failed to start MT voice on existing traffic channel: {}",
                        error
                    );
                }
            }
            self.publish_mobiles();
            return;
        }
        self.queue_voice_page_for_mobile(
            fwd_address,
            session_id,
            service_option,
            VoiceLegRole::Callee,
            a1_tag,
            a1_call_id,
            imsi,
        );
    }

    pub(crate) fn start_msc_controlled_mo_session(
        &mut self,
        fwd_address: &cdma_common::lac::paging_messages::MsAddress,
        service_option: u16,
        called_number: String,
    ) -> Uuid {
        let session_id = Uuid::new_v4();
        let caller_number = self
            .mobiles
            .get(fwd_address)
            .and_then(|ms| ms.phone_number.clone());
        let caller = self.create_voice_party_from_mobile(fwd_address);
        self.voice.push_session(VoiceCallSession {
            id: session_id,
            kind: VoiceSessionKind::MobileOriginatedExternal,
            service_option,
            caller,
            callee: None,
            caller_number,
            called_number: (!called_number.is_empty()).then_some(called_number),
        });
        session_id
    }
}
