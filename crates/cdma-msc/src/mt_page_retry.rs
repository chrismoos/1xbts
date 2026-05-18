//! MSC-level paging retry ("hunt") for MT calls.
//!
//! BSC owns air-interface page repeats per cdma2000. When that burst exhausts,
//! BSC notifies MSC via `ClearRequest` with cause `PAGE_RESP_TIMEOUT`. MSC then
//! pauses `cooldown_ms` and re-sends the original PagingRequest, repeating
//! until `max_duration_ms` elapses.

use std::collections::HashMap;
use std::time::Duration;

use tokio::time::Instant;

use crate::call_control::CallId;

/// IOS A.S0014 Cause IE value used by the BSC to signal a paging-response timeout.
pub(crate) const A1_CAUSE_PAGE_RESP_TIMEOUT: u8 = 0x6E;

#[derive(Debug, Clone)]
pub(crate) struct MtPageRetryState {
    pub(crate) give_up_at: Instant,
    pub(crate) next_retry_at: Option<Instant>,
    pub(crate) paging_request: cdma_ios::PagingRequestMessage,
}

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum PageTimeoutOutcome {
    /// Re-send the paging request when `next_retry_at` elapses.
    Retry(Instant),
    /// Hunt window exhausted — caller should drive the call-failed path.
    GiveUp,
    /// Call wasn't in the retry registry; treat the ClearRequest normally.
    Unknown,
}

pub(crate) struct MtPageRetryService {
    cooldown: Duration,
    max_duration: Duration,
    entries: HashMap<CallId, MtPageRetryState>,
}

impl MtPageRetryService {
    pub(crate) fn new(cooldown_ms: u64, max_duration_ms: u64) -> Self {
        Self {
            cooldown: Duration::from_millis(cooldown_ms),
            max_duration: Duration::from_millis(max_duration_ms),
            entries: HashMap::new(),
        }
    }

    pub(crate) fn register(
        &mut self,
        call_id: CallId,
        paging_request: cdma_ios::PagingRequestMessage,
    ) {
        let give_up_at = Instant::now() + self.max_duration;
        self.entries.insert(
            call_id,
            MtPageRetryState {
                give_up_at,
                next_retry_at: None,
                paging_request,
            },
        );
    }

    pub(crate) fn cancel(&mut self, call_id: CallId) {
        self.entries.remove(&call_id);
    }

    /// Called when BSC reports a page-response timeout. Returns whether to
    /// schedule another burst, give up, or treat the request as a normal clear.
    pub(crate) fn handle_page_timeout(&mut self, call_id: CallId) -> PageTimeoutOutcome {
        let Some(state) = self.entries.get_mut(&call_id) else {
            return PageTimeoutOutcome::Unknown;
        };
        let now = Instant::now();
        if now + self.cooldown >= state.give_up_at {
            self.entries.remove(&call_id);
            return PageTimeoutOutcome::GiveUp;
        }
        let next = now + self.cooldown;
        state.next_retry_at = Some(next);
        PageTimeoutOutcome::Retry(next)
    }

    /// Drain entries whose `next_retry_at` has elapsed; the caller re-sends
    /// the paging request for each.
    pub(crate) fn drain_due(
        &mut self,
        now: Instant,
    ) -> Vec<(CallId, cdma_ios::PagingRequestMessage)> {
        let mut due = Vec::new();
        for (call_id, state) in self.entries.iter_mut() {
            if state.next_retry_at.is_some_and(|t| t <= now) {
                state.next_retry_at = None;
                due.push((*call_id, state.paging_request.clone()));
            }
        }
        due
    }

    pub(crate) fn next_retry_deadline(&self) -> Option<Instant> {
        self.entries
            .values()
            .filter_map(|state| state.next_retry_at)
            .min()
    }

    #[cfg(test)]
    pub(crate) fn contains(&self, call_id: CallId) -> bool {
        self.entries.contains_key(&call_id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn req() -> cdma_ios::PagingRequestMessage {
        cdma_ios::PagingRequestMessage {
            mobile_identity_imsi: cdma_ios::MobileIdentity::Imsi("310150123456789".to_string()),
            tag: Some(cdma_ios::Tag(1)),
            cell_identifier_list: None,
            slot_cycle_index: None,
            service_option: Some(cdma_ios::ServiceOption(3)),
            is2000_mobile_capabilities: None,
        }
    }

    #[tokio::test]
    async fn first_timeout_schedules_retry() {
        let mut svc = MtPageRetryService::new(1000, 60_000);
        let call_id = CallId(1);
        svc.register(call_id, req());
        assert!(matches!(
            svc.handle_page_timeout(call_id),
            PageTimeoutOutcome::Retry(_)
        ));
        assert!(svc.contains(call_id));
    }

    #[tokio::test]
    async fn gives_up_when_cooldown_would_exceed_deadline() {
        // max_duration < cooldown: first timeout must give up immediately.
        let mut svc = MtPageRetryService::new(2000, 1000);
        let call_id = CallId(1);
        svc.register(call_id, req());
        assert_eq!(svc.handle_page_timeout(call_id), PageTimeoutOutcome::GiveUp);
        assert!(!svc.contains(call_id));
    }

    #[tokio::test]
    async fn unknown_call_returns_unknown() {
        let mut svc = MtPageRetryService::new(100, 1000);
        assert_eq!(
            svc.handle_page_timeout(CallId(42)),
            PageTimeoutOutcome::Unknown
        );
    }

    #[tokio::test]
    async fn cancel_drops_entry() {
        let mut svc = MtPageRetryService::new(1000, 60_000);
        let call_id = CallId(7);
        svc.register(call_id, req());
        assert!(svc.contains(call_id));
        svc.cancel(call_id);
        assert!(!svc.contains(call_id));
    }
}
