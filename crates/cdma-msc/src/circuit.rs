//! Circuit session management for the MSC runtime.
//!
//! Owns per-circuit metadata, assignment-complete correlation, secondary-leg
//! procedure engines, and deferred paging responses.

use std::collections::{HashMap, VecDeque};

use crate::runtime::assignment_circuit_identity_code_with_offset;

use cdma_ios::{ProcedureDirection, ProcedureEngine};

use crate::call_control::CallId;

/// Which voice leg a circuit belongs to within a multi-leg call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum MscVoiceLeg {
    /// The first (originating or terminating) leg.
    Primary,
    /// The second leg added by a mobile-to-mobile page response.
    Secondary,
}

/// Composite key identifying a single leg within a call.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct MscLegKey {
    pub(crate) call_id: CallId,
    pub(crate) leg_role: MscVoiceLeg,
}

/// Per-circuit metadata tracked by the MSC.
pub(crate) struct CircuitSession {
    pub(crate) call_id: CallId,
    pub(crate) audio_file: Option<String>,
    pub(crate) service_option: u16,
    pub(crate) leg_role: MscVoiceLeg,
    pub(crate) peer_circuit_id: Option<u16>,
    pub(crate) bearer_remote_ready: bool,
    pub(crate) media_gateway_handle: Option<crate::media_gateway::CallHandle>,
    /// Called party number for this leg, when known. Used to look up the
    /// subscriber's custom ringtone for ringback playback.
    pub(crate) called_number: Option<String>,
}

/// A paging response waiting for the active assignment to finish.
pub(crate) struct DeferredPagingResponse {
    pub(crate) response: cdma_ios::PagingResponseMessage,
}

/// Manages circuit sessions, assignment correlation, leg procedures, and
/// deferred paging responses.
pub(crate) struct CircuitService {
    /// Circuit ID -> session metadata, populated when AssignmentRequest is sent.
    pub(crate) circuits: HashMap<u16, CircuitSession>,
    /// Leg procedure engines for non-primary legs that share the same A1 call.
    pub(crate) leg_procedures: HashMap<MscLegKey, ProcedureEngine>,
    /// Leg -> AssignmentRequest circuit ID awaiting AssignmentComplete.
    pub(crate) pending_assignment_completes: HashMap<MscLegKey, u16>,
    /// Call ID -> currently outstanding assignment leg.
    pub(crate) active_assignment_legs: HashMap<CallId, MscVoiceLeg>,
    /// Secondary leg page responses waiting for the active assignment to finish.
    ///
    /// Used by MT multi-leg flows where a second PagingResponse can arrive
    /// while the first leg's AssignmentComplete is still pending. MO M2M no
    /// longer reaches this path because the secondary-leg PagingRequest is
    /// itself deferred (`deferred_paging_requests`) until the MO leg
    /// completes — so no callee response can race the MO assignment.
    pub(crate) deferred_paging_responses: HashMap<CallId, VecDeque<DeferredPagingResponse>>,
    /// Outgoing MO M2M PagingRequests held until the primary (MO) leg's
    /// AssignmentComplete arrives, so the callee is never paged before the
    /// caller is fully on the traffic channel.
    pub(crate) deferred_paging_requests: HashMap<CallId, cdma_ios::PagingRequestMessage>,
    /// Initial Paging Request per MT call, reused to initialize per-leg A1 procedure engines.
    pub(crate) paging_requests: HashMap<CallId, cdma_ios::PagingRequestMessage>,
    /// Per-call retry counter for MT-leg AssignmentFailure-driven re-pages.
    /// Reset on successful AssignmentComplete or call cleanup.
    pub(crate) mt_assignment_failure_retries: HashMap<CallId, u8>,
}

impl CircuitService {
    pub(crate) fn new() -> Self {
        Self {
            circuits: HashMap::new(),
            leg_procedures: HashMap::new(),
            pending_assignment_completes: HashMap::new(),
            active_assignment_legs: HashMap::new(),
            deferred_paging_responses: HashMap::new(),
            deferred_paging_requests: HashMap::new(),
            paging_requests: HashMap::new(),
            mt_assignment_failure_retries: HashMap::new(),
        }
    }

    pub(crate) fn bump_assignment_failure_retry(&mut self, call_id: CallId) -> u8 {
        let entry = self
            .mt_assignment_failure_retries
            .entry(call_id)
            .or_insert(0);
        *entry += 1;
        *entry
    }

    pub(crate) fn reset_assignment_failure_retries(&mut self, call_id: CallId) {
        self.mt_assignment_failure_retries.remove(&call_id);
    }

    pub(crate) fn insert_circuit_session(&mut self, circuit_id: u16, mut session: CircuitSession) {
        let peer_circuit_id = self
            .circuits
            .iter()
            .find(|(cid, peer)| {
                **cid != circuit_id
                    && peer.call_id == session.call_id
                    && peer.peer_circuit_id.is_none()
            })
            .map(|(cid, _)| *cid);
        if let Some(peer_circuit_id) = peer_circuit_id {
            session.peer_circuit_id = Some(peer_circuit_id);
            if let Some(peer) = self.circuits.get_mut(&peer_circuit_id) {
                peer.peer_circuit_id = Some(circuit_id);
            }
        }
        self.circuits.insert(circuit_id, session);
    }

    pub(crate) fn has_pending_assignment_complete(&self, call_id: CallId) -> bool {
        self.active_assignment_legs.contains_key(&call_id)
    }

    pub(crate) fn assignment_circuit_identity_code_for_next_leg(
        &self,
        call_id: CallId,
    ) -> cdma_ios::CircuitIdentityCode {
        let leg_offset = self
            .circuits
            .values()
            .filter(|session| session.call_id == call_id)
            .count() as u16;
        assignment_circuit_identity_code_with_offset(call_id, leg_offset)
    }

    pub(crate) fn queue_assignment_complete_circuit(
        &mut self,
        call_id: CallId,
        leg_role: MscVoiceLeg,
        circuit_id: u16,
    ) {
        let leg = MscLegKey { call_id, leg_role };
        self.pending_assignment_completes.insert(leg, circuit_id);
        self.active_assignment_legs.insert(call_id, leg_role);
    }

    pub(crate) fn cancel_assignment_complete_circuit(
        &mut self,
        call_id: CallId,
        leg_role: MscVoiceLeg,
    ) {
        self.pending_assignment_completes
            .remove(&MscLegKey { call_id, leg_role });
        if self.active_assignment_legs.get(&call_id) == Some(&leg_role) {
            self.active_assignment_legs.remove(&call_id);
        }
    }

    pub(crate) fn assignment_complete_circuit(&mut self, call_id: CallId) -> Option<u16> {
        let Some(leg_role) = self.active_assignment_legs.remove(&call_id) else {
            return None;
        };
        let leg = MscLegKey { call_id, leg_role };
        self.pending_assignment_completes.remove(&leg)
    }

    pub(crate) fn apply_secondary_leg_from_bsc(
        &mut self,
        call_id: CallId,
        message: &cdma_ios::ProcedureMessage,
    ) -> Result<cdma_ios::EngineEvent, cdma_ios::ProcedureError> {
        let key = MscLegKey {
            call_id,
            leg_role: MscVoiceLeg::Secondary,
        };
        if !self.leg_procedures.contains_key(&key) {
            let Some(paging_request) = self.paging_requests.get(&call_id).cloned() else {
                return Err(cdma_ios::ProcedureError::InvalidTransition {
                    procedure: "CallControl",
                    state: "Idle",
                    reason: "secondary leg has no originating Paging Request",
                });
            };
            let mut engine = ProcedureEngine::new();
            engine.apply(
                ProcedureDirection::MscToBsc,
                &cdma_ios::ProcedureMessage::PagingRequest(paging_request),
            )?;
            self.leg_procedures.insert(key, engine);
        }
        self.leg_procedures
            .get_mut(&key)
            .ok_or(cdma_ios::ProcedureError::InvalidTransition {
                procedure: "secondary_leg",
                state: "missing",
                reason: "secondary leg procedure must exist",
            })?
            .apply(ProcedureDirection::BscToMsc, message)
    }

    pub(crate) fn apply_secondary_leg_from_msc(
        &mut self,
        call_id: CallId,
        message: &cdma_ios::ProcedureMessage,
    ) -> Result<cdma_ios::EngineEvent, cdma_ios::ProcedureError> {
        let key = MscLegKey {
            call_id,
            leg_role: MscVoiceLeg::Secondary,
        };
        self.leg_procedures
            .get_mut(&key)
            .ok_or(cdma_ios::ProcedureError::InvalidTransition {
                procedure: "secondary_leg",
                state: "missing",
                reason: "secondary leg procedure must exist before MSC-originated message",
            })?
            .apply(ProcedureDirection::MscToBsc, message)
    }

    /// Flush one deferred paging response for a call if no assignment is pending.
    ///
    /// Returns `Some(response)` if one was dequeued, `None` otherwise.
    pub(crate) fn take_deferred_paging_response(
        &mut self,
        call_id: CallId,
    ) -> Option<cdma_ios::PagingResponseMessage> {
        if self.has_pending_assignment_complete(call_id) {
            return None;
        }
        let response = self
            .deferred_paging_responses
            .get_mut(&call_id)
            .and_then(|queue| queue.pop_front());
        if self
            .deferred_paging_responses
            .get(&call_id)
            .is_some_and(VecDeque::is_empty)
        {
            self.deferred_paging_responses.remove(&call_id);
        }
        response.map(|r| r.response)
    }

    /// Take the deferred outgoing PagingRequest for a call, if any.
    ///
    /// Returns `Some(request)` if one was stored, `None` otherwise.
    pub(crate) fn take_deferred_paging_request(
        &mut self,
        call_id: CallId,
    ) -> Option<cdma_ios::PagingRequestMessage> {
        self.deferred_paging_requests.remove(&call_id)
    }

    /// Wipe all secondary-leg state for `call_id` so the next inbound
    /// PagingResponse hits the lazy-init path in
    /// `apply_secondary_leg_from_bsc`. Returns the abandoned circuit_id.
    pub(crate) fn cancel_secondary_leg(
        &mut self,
        call_id: CallId,
        voice_bearer: Option<&std::sync::Arc<cdma_ios::VoiceBearerManager>>,
    ) -> Option<u16> {
        let key = MscLegKey {
            call_id,
            leg_role: MscVoiceLeg::Secondary,
        };
        let pending_circuit = self.pending_assignment_completes.remove(&key);
        if self.active_assignment_legs.get(&call_id) == Some(&MscVoiceLeg::Secondary) {
            self.active_assignment_legs.remove(&call_id);
        }
        self.leg_procedures.remove(&key);
        if let Some(circuit_id) = pending_circuit {
            if let Some(bearer) = voice_bearer {
                bearer.close_circuit(circuit_id);
            }
            self.circuits.remove(&circuit_id);
        }
        pending_circuit
    }

    /// Clean up all circuit state associated with a call.
    pub(crate) fn cleanup_call(
        &mut self,
        call_id: CallId,
        voice_bearer: Option<&std::sync::Arc<cdma_ios::VoiceBearerManager>>,
    ) -> Vec<u16> {
        self.pending_assignment_completes
            .retain(|leg, _| leg.call_id != call_id);
        self.active_assignment_legs.remove(&call_id);
        self.deferred_paging_responses.remove(&call_id);
        self.deferred_paging_requests.remove(&call_id);
        self.paging_requests.remove(&call_id);
        self.leg_procedures.retain(|leg, _| leg.call_id != call_id);
        self.mt_assignment_failure_retries.remove(&call_id);

        let circuit_ids: Vec<u16> = self
            .circuits
            .iter()
            .filter(|(_, s)| s.call_id == call_id)
            .map(|(&cid, _)| cid)
            .collect();
        for cid in &circuit_ids {
            if let Some(bearer) = voice_bearer {
                bearer.close_circuit(*cid);
            }
            self.circuits.remove(cid);
        }
        circuit_ids
    }
}
