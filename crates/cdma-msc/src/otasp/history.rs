//! In-flight assembly buffer for OTASP sessions.
//!
//! Persisted history lives in the `otasp_sessions` HLR table. What's
//! here is just a per-session scratchpad while a `*228` call is on the
//! air — open on `SessionStart`, append on each event, take the
//! assembled record on `SessionEnded` and hand it to
//! `hlr_repo.save_otasp_session()`.
//!
//! Records are keyed by the MSC-allocated `session_id` UUID so the
//! coordinator can append events without re-keying through ESN/MEID
//! on every callback.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;

use uuid::Uuid;

use crate::otasp::event::{HardwareIdentity, OtaspEvent, SessionOutcomeKind};

/// One recorded event plus the wall-clock time it was appended.
#[derive(Debug, Clone)]
pub struct RecordedEvent {
    pub recorded_at: SystemTime,
    pub event: OtaspEvent,
}

/// One OTASP session record. Created on `SessionStart`, mutated on each
/// subsequent event, finalized and removed on `SessionEnded`.
#[derive(Debug, Clone)]
pub struct OtaspSessionRecord {
    pub session_id: Uuid,
    pub device: HardwareIdentity,
    /// Resolved from the HLR lookup at session start. `None` for HlrMiss
    /// sessions where the device wasn't provisioned.
    pub subscriber_id: Option<Uuid>,
    pub started_at: SystemTime,
    pub ended_at: Option<SystemTime>,
    pub outcome: Option<SessionOutcomeKind>,
    pub events: Vec<RecordedEvent>,
}

impl OtaspSessionRecord {
    fn new(
        session_id: Uuid,
        device: HardwareIdentity,
        subscriber_id: Option<Uuid>,
        started_at: SystemTime,
    ) -> Self {
        Self {
            session_id,
            device,
            subscriber_id,
            started_at,
            ended_at: None,
            outcome: None,
            events: Vec::new(),
        }
    }
}

/// Thread-safe in-flight session map. The coordinator holds an `Arc<Self>`.
#[derive(Debug, Default)]
pub struct OtaspHistory {
    inner: Mutex<HashMap<Uuid, OtaspSessionRecord>>,
}

impl OtaspHistory {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// Begin tracking a new session and append the leading `SessionStart`
    /// event. Returns the assigned session id.
    pub fn open_session(
        &self,
        device: HardwareIdentity,
        subscriber_id: Option<Uuid>,
        start: OtaspEvent,
    ) -> Uuid {
        let now = SystemTime::now();
        let id = Uuid::new_v4();
        let mut rec = OtaspSessionRecord::new(id, device, subscriber_id, now);
        rec.events.push(RecordedEvent {
            recorded_at: now,
            event: start,
        });
        let mut inner = self.inner.lock().expect("OtaspHistory mutex poisoned");
        inner.insert(id, rec);
        id
    }

    /// Append an event to an open session. Silently no-ops if the session id
    /// is unknown (e.g. duplicate SessionEnded). Sets ended_at + outcome on
    /// `SessionEnded` so `take_finished` returns a complete record.
    pub fn append(&self, session_id: Uuid, event: OtaspEvent) {
        let now = SystemTime::now();
        let mut inner = self.inner.lock().expect("OtaspHistory mutex poisoned");
        if let Some(rec) = inner.get_mut(&session_id) {
            if let OtaspEvent::SessionEnded { outcome, .. } = &event {
                rec.ended_at = Some(now);
                rec.outcome = Some(*outcome);
            }
            rec.events.push(RecordedEvent {
                recorded_at: now,
                event,
            });
        }
    }

    /// Remove and return the assembled record for a finished session.
    /// Returns `None` if the session id isn't tracked.
    pub fn take_finished(&self, session_id: Uuid) -> Option<OtaspSessionRecord> {
        let mut inner = self.inner.lock().expect("OtaspHistory mutex poisoned");
        inner.remove(&session_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dev(esn: u32) -> HardwareIdentity {
        HardwareIdentity {
            esn: Some(esn),
            meid: None,
        }
    }

    fn start_for(esn: u32) -> OtaspEvent {
        OtaspEvent::SessionStart {
            device: dev(esn),
            feature_code: "*228".into(),
            service_option: 18,
        }
    }

    #[test]
    fn open_append_take_round_trip() {
        let h = OtaspHistory::new();
        let id = h.open_session(dev(0x1234_5678), None, start_for(0x1234_5678));
        h.append(id, OtaspEvent::SpcVerified);
        h.append(
            id,
            OtaspEvent::SessionEnded {
                completed_blocks: 1,
                outcome: SessionOutcomeKind::Committed,
            },
        );
        let rec = h.take_finished(id).expect("record present");
        assert_eq!(rec.events.len(), 3);
        assert!(matches!(rec.outcome, Some(SessionOutcomeKind::Committed)));
        assert!(rec.ended_at.is_some());
        // Second take is a no-op.
        assert!(h.take_finished(id).is_none());
    }

    #[test]
    fn open_session_captures_subscriber_id() {
        let h = OtaspHistory::new();
        let sub = Uuid::new_v4();
        let id = h.open_session(dev(1), Some(sub), start_for(1));
        let rec = h.take_finished(id).unwrap();
        assert_eq!(rec.subscriber_id, Some(sub));
    }
}
