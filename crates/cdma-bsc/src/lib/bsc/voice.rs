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
use crate::voice_bearer_bits::{mux_voice_bits_for_air, pack_voice_bits_for_bearer};

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
    /// AWIM Calling Party Number information record (C.S0005-E 3.7.5.3)
    /// supplied by the MSC inside the A1 MS Information Records IE. Emitted
    /// verbatim in the AWIM SDU on F-TCH alerting.
    pub(crate) calling_party_record: Option<CallingPartyNumberRecord>,
    /// Dialed external number for MSC-controlled mobile-originated calls.
    pub(crate) called_number: Option<String>,
}

/// Strip non-digits and truncate to E.164's 15-digit maximum.
/// Logs when characters were dropped so stale stored data surfaces.
pub(crate) fn sanitize_e164_digits(input: &str) -> String {
    const MAX: usize = 15;
    let mut out: String = input.chars().filter(|c| c.is_ascii_digit()).collect();
    if out.len() > MAX {
        out.truncate(MAX);
    }
    if out.len() != input.len() {
        warn!(
            "BSC: AWIM caller digits sanitized: {:?} -> {:?}",
            input, out
        );
    }
    out
}

/// Defensive sanitization at the AWIM emit boundary regardless of source.
pub(crate) fn sanitize_record(mut record: CallingPartyNumberRecord) -> CallingPartyNumberRecord {
    record.digits = sanitize_e164_digits(&record.digits);
    record
}

/// MT call_id stashed when an SR-to-add-voice is refused; the BSC emits
/// A1 `AssignmentFailure(call_id)` once the TCH teardown returns the MS
/// to `Registered`.
#[derive(Debug, Clone, Copy)]
pub(crate) struct PendingAssignmentFailure {
    pub(crate) call_id: u64,
    pub(crate) queued_at: Instant,
}

pub(crate) struct DeferredVoicePage {
    pub(crate) fwd_address: MsAddress,
    pub(crate) session_id: Uuid,
    pub(crate) service_option: u16,
    pub(crate) leg_role: VoiceLegRole,
    pub(crate) a1_tag: Option<cdma_ios::Tag>,
    pub(crate) a1_call_id: Option<u64>,
    pub(crate) imsi: Option<String>,
}

pub(crate) struct VoiceService {
    pub(crate) sessions: Vec<VoiceCallSession>,
    pub(crate) next_call_connection_ref: u32,
    pub(crate) next_mo_call_id: u64,
    pub(crate) deferred_page_after_release: Option<DeferredVoicePage>,
}

impl Default for VoiceService {
    fn default() -> Self {
        Self {
            sessions: Vec::new(),
            next_call_connection_ref: 1,
            next_mo_call_id: 0x1_0000_0000,
            deferred_page_after_release: None,
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

    pub(crate) fn take_deferred_page_for_a1_call(
        &mut self,
        call_id: u64,
    ) -> Option<DeferredVoicePage> {
        self.deferred_page_after_release
            .as_ref()
            .is_some_and(|pending| pending.a1_call_id == Some(call_id))
            .then(|| self.deferred_page_after_release.take())
            .flatten()
    }
}

pub(crate) fn traffic_rate_from_bps(rate_bps: u32) -> Option<TrafficRate> {
    match rate_bps {
        // RS1 (RC1 / RC3 voice): 9600/4800/2400/1200, with RC3 emitting
        // 2700/1500 for the sub-rate tiers.
        9_600 => Some(TrafficRate::Full),
        4_800 => Some(TrafficRate::Half),
        2_400 | 2_700 => Some(TrafficRate::Quarter),
        1_200 | 1_500 => Some(TrafficRate::Eighth),
        // RS2 (RC2 / RC5): 14400/7200/3600/1800. Used by QCELP-13K
        // (SO 32768) over RC2.
        14_400 => Some(TrafficRate::Full),
        7_200 => Some(TrafficRate::Half),
        3_600 => Some(TrafficRate::Quarter),
        1_800 => Some(TrafficRate::Eighth),
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

    /// Packet TCHs have no A1 `a1_call_id`, so no A1 ClearRequest is sent.
    pub(crate) fn release_tch_for_assignment_failure(&mut self, walsh_code: u8, reason: &str) {
        info!(
            "BSC: releasing walsh={} for MT assignment-failure signal ({})",
            walsh_code, reason
        );
        if let Err(e) = self.send_traffic_release_order(walsh_code, super::DEFAULT_TRAFFIC_ACK_SEQ)
        {
            warn!(
                "BSC: failed to send Release Order on walsh={} during {}: {}",
                walsh_code, reason, e
            );
        }
        self.mobiles.update_tc(walsh_code, |_, tc| {
            tc.mark_releasing();
        });
    }

    pub(crate) fn fire_pending_a1_failure_after_release(
        &mut self,
        fwd_address: &cdma_common::lac::paging_messages::MsAddress,
    ) {
        let Some(idx) = self
            .pending_a1_failure_after_release
            .iter()
            .position(|(addr, _)| addr == fwd_address)
        else {
            return;
        };
        let (_, entry) = self.pending_a1_failure_after_release.remove(idx);
        info!(
            "BSC: sending A1 AssignmentFailure for {} call_id={} after TCH teardown",
            format_ms_address(fwd_address),
            entry.call_id,
        );
        self.a1.send_assignment_failure(entry.call_id, 0x16);
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
            self.begin_voice_release(&addr, super::DEFAULT_TRAFFIC_ACK_SEQ, "peer leg released");
        }

        if !self.mobiles.has_voice_session(session_id) {
            self.voice
                .retain_sessions(|session| session.id != session_id);
        }
    }

    // -----------------------------------------------------------------------
    // Voice call signaling
    // -----------------------------------------------------------------------

    /// Send AWIM on F-TCH (C.S0005-E 3.7.3.3.2.3) without a Signal IE. Use
    /// `send_alert_with_info_signal` to embed a tone.
    pub(crate) fn send_alert_with_info(
        &mut self,
        walsh_code: u8,
        ack_seq: u8,
        calling_party: Option<CallingPartyNumberRecord>,
    ) -> Result<(), Error> {
        let awim = AlertWithInformationMessage {
            signal_info: None,
            calling_party: calling_party.map(sanitize_record),
        };
        let sdu = awim.to_ftch_sdu();

        info!(
            "BSC: sending Alert With Information on F-TCH walsh={} ack_seq={}",
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

    /// AWIM carrying a caller-supplied Signal Info Record (A.S0014-D §2.1.6
    /// DTAP Progress relay).
    pub(crate) fn send_alert_with_info_signal(
        &mut self,
        walsh_code: u8,
        ack_seq: u8,
        signal_info: SignalInfoRecord,
    ) -> Result<(), Error> {
        let signal_value = signal_info.signal;
        let awim = AlertWithInformationMessage {
            signal_info: Some(signal_info),
            calling_party: None,
        };
        let sdu = awim.to_ftch_sdu();

        info!(
            "BSC: sending AWIM Progress signal=0x{:02x} on F-TCH walsh={} ack_seq={}",
            signal_value, walsh_code, ack_seq
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
                    self.begin_voice_release(
                        &fwd_address,
                        super::DEFAULT_TRAFFIC_ACK_SEQ,
                        "unbridged local voice leg",
                    );
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

    /// Records a forward telephone-event report from the MSC. The audible
    /// rendition lives at the SIP edge; the BSC only sees these for
    /// MSC-originated DTMF such as supplementary-service tones.
    pub(crate) fn handle_forward_bearer_dtmf(&mut self, event: cdma_ios::DtmfBearerEvent) {
        log::info!(
            "BSC: forward DTMF on A2p circuit_id={} event={} duration={} end={}",
            event.circuit_id,
            event.event,
            event.duration_samples,
            event.end,
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
        if self
            .mobiles
            .get_traffic_channel(walsh_code)
            .is_some_and(|tc| tc.is_releasing())
        {
            log::debug!(
                "BSC: dropping MSC bearer frame circuit_id={} walsh={} while releasing",
                frame.circuit_id,
                walsh_code
            );
            return;
        }
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
            // Mark connected; tones-off is MSC-driven (Progress{Signal=0x3F}
            // on Connect).
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

    /// Plays a BDTMFM digit sequence (C.S0005-E §2.7.2.3.2.7) on the A2p
    /// bearer as RFC 4733 telephone-event packets. Spawned on a tokio task
    /// so on/off-length sleeps do not block the traffic dispatcher.
    pub(crate) fn play_bdtmfm_sequence(
        &self,
        walsh_code: u8,
        msg: &cdma_common::access::SendBurstDtmfMessage,
    ) {
        let Some(bearer) = self.config.msc_voice_bearer.clone() else {
            return;
        };
        let Some(circuit_id) = self
            .mobiles
            .get_traffic_channel(walsh_code)
            .and_then(|tc| tc.msc_circuit_id)
        else {
            log::debug!(
                "BSC: cannot play BDTMFM on walsh={} — no msc_circuit_id",
                walsh_code
            );
            return;
        };
        let on_ms = cdma_common::access::bdtmfm_on_length_ms(msg.dtmf_on_length).unwrap_or(150);
        let off_ms = cdma_common::access::bdtmfm_off_length_ms(msg.dtmf_off_length).unwrap_or(100);
        let digits = msg.digits.clone();
        tokio::spawn(async move {
            for digit in digits {
                let Some(event_code) = cdma_ios::DtmfBearerEvent::event_from_cdma_digit(digit)
                else {
                    log::warn!(
                        "BSC: dropping BDTMFM digit code 0x{:02x} (reserved per Table 2.7.1.3.2.4-4)",
                        digit
                    );
                    continue;
                };
                emit_bdtmfm_digit(&bearer, circuit_id, event_code, on_ms).await;
                tokio::time::sleep(std::time::Duration::from_millis(off_ms as u64)).await;
            }
        });
    }

    /// Marker packet on `start = true`, three end-of-event repeats with a
    /// fixed `STOP_DURATION_SAMPLES` on `start = false`. No refresh pump
    /// during the hold, so the SIP-side tone length does not track the MS
    /// hold time.
    pub(crate) fn emit_continuous_dtmf_order(&self, walsh_code: u8, digit: u8, start: bool) {
        let Some(event_code) = cdma_ios::DtmfBearerEvent::event_from_cdma_digit(digit) else {
            log::warn!(
                "BSC: dropping Continuous DTMF Order digit code 0x{:02x} (reserved per Table 2.7.1.3.2.4-4)",
                digit
            );
            return;
        };
        let Some(bearer) = self.config.msc_voice_bearer.clone() else {
            return;
        };
        let Some(circuit_id) = self
            .mobiles
            .get_traffic_channel(walsh_code)
            .and_then(|tc| tc.msc_circuit_id)
        else {
            return;
        };
        tokio::spawn(async move {
            if start {
                let _ = bearer
                    .send_dtmf_event(circuit_id, event_code, BDTMFM_VOLUME_DBM0, 0, false, true)
                    .await;
            } else {
                // No timed duration tracking for continuous DTMF; emit the
                // RFC 4733 end-of-event trio at a representative duration.
                const STOP_DURATION_SAMPLES: u16 = 1600;
                for _ in 0..cdma_ios::voice_bearer::RFC4733_END_REPEAT_COUNT {
                    let _ = bearer
                        .send_dtmf_event(
                            circuit_id,
                            event_code,
                            BDTMFM_VOLUME_DBM0,
                            STOP_DURATION_SAMPLES,
                            true,
                            false,
                        )
                        .await;
                }
            }
        });
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
        let Some(payload) = pack_voice_bits_for_bearer(bits, rate_bps) else {
            return false;
        };
        let frame = cdma_ios::VoiceBearerFrame {
            circuit_id,
            rate_bps,
            payload,
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

    pub(crate) fn reverse_voice_media_enabled(&self, walsh_code: u8) -> bool {
        let Some(tc) = self.mobiles.get_traffic_channel(walsh_code) else {
            return false;
        };
        !tc.is_releasing()
            && tc.msc_circuit_id.is_some()
            && self.is_voice_session_msc_media_controlled(tc.voice_session_id)
    }

    pub(crate) fn send_standard_alert(
        &mut self,
        walsh_code: u8,
        ack_seq: u8,
        calling_party: Option<CallingPartyNumberRecord>,
    ) -> Result<(), Error> {
        let awim = AlertWithInformationMessage {
            signal_info: Some(SignalInfoRecord {
                signal_type: 0x02,
                alert_pitch: 0x00,
                signal: 0x01,
            }),
            calling_party: calling_party.map(sanitize_record),
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
        // Page-from-unregistered IMSI falls back to SCI=0 (non-slotted: MS
        // listens every slot) so we don't need a registered PGSLOT.
        let (page_address, pgslot, slot_cycle_index) = self
            .mobiles
            .get(fwd_address)
            .and_then(|ms| {
                ms.page_address()
                    .map(|p| (p, ms.pgslot, ms.slot_cycle_index))
            })
            .unwrap_or_else(|| {
                (
                    cdma_common::lac::paging_messages::MsPageAddress::from(fwd_address),
                    None,
                    0,
                )
            });
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
        a1_tag: Option<cdma_ios::Tag>,
        a1_call_id: Option<u64>,
        imsi: Option<String>,
    ) {
        if a1_call_id.is_some()
            && self
                .voice
                .deferred_page_after_release
                .as_ref()
                .is_some_and(|pending| pending.a1_call_id == a1_call_id)
        {
            info!(
                "BSC: ignoring duplicate Paging Request while legacy traffic release is pending call_id={:?}",
                a1_call_id
            );
            return;
        }
        let session_id = Uuid::new_v4();
        let (subscriber_id, has_tc, legacy_traffic) = self
            .mobiles
            .get(fwd_address)
            .map(|ms| (ms.subscriber_id, ms.has_traffic_channel(), ms.mob_p_rev < 6))
            .unwrap_or((None, false, false));
        info!(
            "BSC: initiating MSC-controlled MT voice call session={} subscriber={:?}",
            session_id, subscriber_id,
        );
        let callee = self.create_voice_party_from_mobile(fwd_address);
        self.voice.push_session(VoiceCallSession {
            id: session_id,
            kind: VoiceSessionKind::MscControlledMt,
            service_option,
            caller: None,
            callee,
            calling_party_record: None,
            called_number: None,
        });
        if has_tc {
            if legacy_traffic {
                if self.voice.deferred_page_after_release.is_some() {
                    warn!(
                        "BSC: cannot defer MT voice page for legacy mobile; another release-before-page is pending"
                    );
                    self.voice
                        .retain_sessions(|session| session.id != session_id);
                    return;
                }
                self.voice.deferred_page_after_release = Some(DeferredVoicePage {
                    fwd_address: fwd_address.clone(),
                    session_id,
                    service_option,
                    leg_role: VoiceLegRole::Callee,
                    a1_tag,
                    a1_call_id,
                    imsi,
                });
                info!(
                    "BSC: releasing pre-IS-2000 traffic channel before MT page for session={}",
                    session_id
                );
                self.begin_voice_release(
                    fwd_address,
                    super::DEFAULT_TRAFFIC_ACK_SEQ,
                    "pre-IS-2000 release before MT page",
                );
                return;
            }
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

    pub(crate) fn resume_voice_page_after_release(&mut self, fwd_address: &MsAddress) {
        let Some(pending) = self
            .voice
            .deferred_page_after_release
            .take_if(|pending| pending.fwd_address == *fwd_address)
        else {
            return;
        };
        info!(
            "BSC: traffic release complete; paging pre-IS-2000 mobile for session={}",
            pending.session_id
        );
        self.queue_voice_page_for_mobile(
            &pending.fwd_address,
            pending.session_id,
            pending.service_option,
            pending.leg_role,
            pending.a1_tag,
            pending.a1_call_id,
            pending.imsi,
        );
    }

    pub(crate) fn start_msc_controlled_mo_session(
        &mut self,
        fwd_address: &cdma_common::lac::paging_messages::MsAddress,
        service_option: u16,
        called_number: String,
    ) -> Uuid {
        let session_id = Uuid::new_v4();
        let caller = self.create_voice_party_from_mobile(fwd_address);
        self.voice.push_session(VoiceCallSession {
            id: session_id,
            kind: VoiceSessionKind::MobileOriginatedExternal,
            service_option,
            caller,
            callee: None,
            calling_party_record: None,
            called_number: (!called_number.is_empty()).then_some(called_number),
        });
        session_id
    }
}

/// RFC 4733 §2.5.1.3 recommended default volume (10 dBm0).
const BDTMFM_VOLUME_DBM0: u8 = 10;

/// Sample rate × 1 ms for the bearer's 8 kHz clock.
const SAMPLES_PER_MS: u32 = 8;

/// RFC 4733 §2.5.1.4 refresh cadence within one event.
const REFRESH_PERIOD_MS: u32 = 20;

/// Emits one BDTMFM digit: start packet, refresh packets across the
/// on-length window, then three end-of-event repeats per
/// RFC 4733 §2.5.1.4. Errors are logged and skipped — a single failed
/// digit does not abort the rest of the burst.
async fn emit_bdtmfm_digit(
    bearer: &cdma_ios::VoiceBearerManager,
    circuit_id: u16,
    event_code: u8,
    on_ms: u32,
) {
    let mut elapsed_ms: u32 = 0;
    if let Err(err) = bearer
        .send_dtmf_event(circuit_id, event_code, BDTMFM_VOLUME_DBM0, 0, false, true)
        .await
    {
        log::warn!(
            "BSC: failed BDTMFM start packet circuit_id={} event={}: {}",
            circuit_id,
            event_code,
            err
        );
        return;
    }
    while elapsed_ms + REFRESH_PERIOD_MS < on_ms {
        tokio::time::sleep(std::time::Duration::from_millis(REFRESH_PERIOD_MS as u64)).await;
        elapsed_ms += REFRESH_PERIOD_MS;
        let duration = (elapsed_ms * SAMPLES_PER_MS).min(u16::MAX as u32) as u16;
        let _ = bearer
            .send_dtmf_event(
                circuit_id,
                event_code,
                BDTMFM_VOLUME_DBM0,
                duration,
                false,
                false,
            )
            .await;
    }
    let final_duration = (on_ms * SAMPLES_PER_MS).min(u16::MAX as u32) as u16;
    for _ in 0..cdma_ios::voice_bearer::RFC4733_END_REPEAT_COUNT {
        let _ = bearer
            .send_dtmf_event(
                circuit_id,
                event_code,
                BDTMFM_VOLUME_DBM0,
                final_duration,
                true,
                false,
            )
            .await;
    }
}

#[cfg(test)]
mod tests {
    use super::{sanitize_e164_digits, traffic_rate_from_bps};
    use cdma_common::channel::TrafficRate;

    #[test]
    fn traffic_rate_from_bps_maps_rs1_rates() {
        assert_eq!(traffic_rate_from_bps(9_600), Some(TrafficRate::Full));
        assert_eq!(traffic_rate_from_bps(4_800), Some(TrafficRate::Half));
        assert_eq!(traffic_rate_from_bps(2_400), Some(TrafficRate::Quarter));
        assert_eq!(traffic_rate_from_bps(2_700), Some(TrafficRate::Quarter));
        assert_eq!(traffic_rate_from_bps(1_200), Some(TrafficRate::Eighth));
        assert_eq!(traffic_rate_from_bps(1_500), Some(TrafficRate::Eighth));
    }

    #[test]
    fn traffic_rate_from_bps_maps_rs2_rates_for_qcelp_over_rc2() {
        // Without these arms a QCELP-13K voice bearer frame (14400 bps Full)
        // would silently drop at handle_forward_bearer_frame: the BSC could
        // never deliver QCELP audio to the BTS RC2 forward channel.
        assert_eq!(traffic_rate_from_bps(14_400), Some(TrafficRate::Full));
        assert_eq!(traffic_rate_from_bps(7_200), Some(TrafficRate::Half));
        assert_eq!(traffic_rate_from_bps(3_600), Some(TrafficRate::Quarter));
        assert_eq!(traffic_rate_from_bps(1_800), Some(TrafficRate::Eighth));
    }

    #[test]
    fn traffic_rate_from_bps_rejects_unknown_rates() {
        assert_eq!(traffic_rate_from_bps(0), None);
        assert_eq!(traffic_rate_from_bps(6_000), None);
        assert_eq!(traffic_rate_from_bps(38_400), None);
    }

    #[test]
    fn sanitize_strips_non_digits() {
        assert_eq!(sanitize_e164_digits("555-123-4567"), "5551234567");
        assert_eq!(
            sanitize_e164_digits("(555) 123 4567 ext.99"),
            "555123456799"
        );
        assert_eq!(sanitize_e164_digits("+1 555 123 4567"), "15551234567");
    }

    #[test]
    fn sanitize_truncates_to_15_digits() {
        assert_eq!(
            sanitize_e164_digits("1234567890123456789"),
            "123456789012345"
        );
    }

    #[test]
    fn sanitize_returns_empty_for_no_digits() {
        assert_eq!(sanitize_e164_digits("abc-def"), "");
        assert_eq!(sanitize_e164_digits(""), "");
    }

    #[test]
    fn sanitize_passes_clean_digits_unchanged() {
        assert_eq!(sanitize_e164_digits("5551234567"), "5551234567");
    }
}
