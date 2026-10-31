//! Runtime glue between MSC A1 and the OTASP session driver.
//!
//! Owns active OTASP sessions keyed by ESN/MEID. Each session is driven by
//! inbound ADDS Transfer messages with `burst_type = 0x04` and emits outbound
//! ADDS Deliver messages on the same A1 endpoint.

use std::collections::HashMap;
use std::sync::Arc;

use log::{info, warn};
use uuid::Uuid;

use cdma_hlr::repository::HlrRepository;
use cdma_ios::{AddsDeliverMessage, AddsTransferMessage, AddsUserPart};

use crate::config::{HomeNetworkConfig, OtaspConfig};
use crate::grpc::events_proto::v1 as events_proto;
use crate::otasp::event::{HardwareIdentity, OtaspEvent};
use crate::otasp::history::OtaspHistory;
use crate::otasp::nam::{ResolvedPrlMeta, ResolvedSubscriberInput};
use crate::otasp::proto_conv::{to_proto_event as to_proto_otasp_event, to_proto_msc_event};
use crate::otasp::session::{
    KeyExchangeOffer, OtaspAuthRequest, OtaspSession, OtaspTransport, OutboundOtasp, SessionOutcome,
};
use crate::runtime::MscA1Endpoint;

/// Per-session in-memory state plus pending outbound bytes for the next A1 flush.
struct SessionEntry {
    initiation: crate::otasp::session::SessionInitiation,
    session: OtaspSession,
    pending_out: Vec<OutboundOtasp>,
    events: Vec<OtaspEvent>,
    call_id: u64,
    /// History-buffer identifier assigned at session start; used to append
    /// subsequent events to the same record.
    record_id: Uuid,
    /// Wall-clock time of the most recent inbound or outbound message. Used by
    /// `tick_timeouts` to release calls where the MS has gone silent.
    last_activity: std::time::Instant,
    /// Brief label for the phase the session was last in, so the timeout
    /// event can name what we were waiting on.
    last_phase: String,
    requests: Vec<OtaspAuthRequest>,
    identity: Option<cdma_hlr::model::MobileIdentityKey>,
    imsi: Option<String>,
    air_imsi: Option<String>,
    esn: Option<u32>,
    /// Whether the `*228` origination verified. None when it was not authenticated.
    origination_verified: Option<bool>,
}

#[derive(Debug, Clone)]
pub(crate) struct OtaspSsdUpdate {
    pub call_id: u64,
    pub context: crate::authentication::CallAuthContext,
}

/// Default inbound silence threshold before a session is force-released.
pub const DEFAULT_INBOUND_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(5);

/// Silence allowed while the mobile computes an MS Key Response or Key Generation
/// Response. C.S0016-D §3.3.1 gives it 30 seconds after receiving the request, and the
/// extra seconds cover the request's delivery and the response's return.
pub const KEY_EXCHANGE_RESPONSE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(35);

struct Recorder<'a> {
    out: &'a mut Vec<OutboundOtasp>,
    events: &'a mut Vec<OtaspEvent>,
    requests: &'a mut Vec<OtaspAuthRequest>,
}

impl<'a> OtaspTransport for Recorder<'a> {
    fn send(&mut self, message: OutboundOtasp) {
        self.out.push(message);
    }
    fn emit(&mut self, event: OtaspEvent) {
        self.events.push(event);
    }
    fn request(&mut self, request: OtaspAuthRequest) {
        self.requests.push(request);
    }
}

/// OTASP session coordinator owned by the MSC runtime.
pub struct OtaspCoordinator {
    cfg: OtaspConfig,
    home_network: HomeNetworkConfig,
    hlr: Arc<dyn HlrRepository>,
    sessions: HashMap<SessionKey, SessionEntry>,
    history: Arc<OtaspHistory>,
    event_tx: Option<tokio::sync::broadcast::Sender<events_proto::MscNetworkEvent>>,
    /// Tags MSC has put on outbound ADDS Deliver messages and is still
    /// waiting on a BSC `AddsDeliverAck` for. Mapped to the session key
    /// so an incoming failure ack can be routed to the right session.
    /// Successful acks just remove the entry.
    pending_acks: HashMap<u32, SessionKey>,
    /// Monotonic source for outbound ADDS Deliver tags. High bit is set
    /// to namespace OTASP tags away from SMS tags (which start at 0),
    /// so the MSC runtime's `AddsDeliverAck` dispatcher can route by
    /// inspecting the top bit.
    next_tag: u32,
    ssd_updates: Vec<OtaspSsdUpdate>,
}

/// High bit set on OTASP-owned ADDS Deliver tags. Lets the MSC
/// runtime distinguish OTASP acks from SMS acks (which use the low
/// half of the u32 space).
pub const OTASP_TAG_NAMESPACE_BIT: u32 = 0x8000_0000;

/// Returns true when the tag was allocated by OTASP. Used by the
/// MSC runtime dispatcher.
pub fn tag_belongs_to_otasp(tag: u32) -> bool {
    (tag & OTASP_TAG_NAMESPACE_BIT) != 0
}

#[derive(Debug, Clone, Hash, PartialEq, Eq)]
pub enum SessionKey {
    Esn(u32),
    Meid(String),
}

impl SessionKey {
    pub fn from_transfer(msg: &AddsTransferMessage) -> Option<Self> {
        if let Some(cdma_ios::MobileIdentity::Meid(bytes)) = msg.mobile_identity_meid.as_ref() {
            return Some(Self::Meid(encode_meid_hex(bytes)));
        }
        if let Some(cdma_ios::MobileIdentity::Esn(esn)) = msg.mobile_identity_esn.as_ref() {
            return Some(Self::Esn(*esn));
        }
        None
    }
}

/// Resolve which PRL to push for this OTASP session: subscriber's
/// override first, then the system default, else `None`. Failure to
/// reach the HLR for either lookup is logged but not fatal — the
/// session continues without a PRL push (the existing factory PRL on
/// the MS stays in place).
async fn resolve_prl_for_subscriber(
    hlr: &dyn HlrRepository,
    subscriber: &cdma_hlr::model::Subscriber,
) -> (Option<Vec<u8>>, Option<ResolvedPrlMeta>) {
    let chosen = match subscriber.prl_override_id {
        Some(override_id) => match hlr.get_prl(override_id).await {
            Ok(Some(p)) => Some(p),
            Ok(None) => {
                warn!(
                    "OTASP: subscriber {} has prl_override_id {} but the PRL is missing or deleted; falling back to default",
                    subscriber.subscriber_id, override_id
                );
                match hlr.get_default_prl().await {
                    Ok(p) => p,
                    Err(e) => {
                        warn!("OTASP: get_default_prl failed: {e}");
                        None
                    }
                }
            }
            Err(e) => {
                warn!("OTASP: get_prl(override) failed: {e}");
                None
            }
        },
        None => match hlr.get_default_prl().await {
            Ok(p) => p,
            Err(e) => {
                warn!("OTASP: get_default_prl failed: {e}");
                None
            }
        },
    };
    match chosen {
        Some(p) => {
            let meta = ResolvedPrlMeta {
                pr_list_id: p.pr_list_id as u16,
                sspr_p_rev: p.sspr_p_rev as u8,
            };
            (Some(p.raw_bytes), Some(meta))
        }
        None => (None, None),
    }
}

fn encode_meid_hex(bytes: &[u8; 7]) -> String {
    let mut s = String::with_capacity(14);
    for b in bytes {
        use std::fmt::Write;
        write!(&mut s, "{:02x}", b).expect("write hex");
    }
    s
}

impl OtaspCoordinator {
    pub fn new(
        cfg: OtaspConfig,
        home_network: HomeNetworkConfig,
        hlr: Arc<dyn HlrRepository>,
    ) -> Self {
        Self::with_history(cfg, home_network, hlr, OtaspHistory::new(), None)
    }

    /// Construct a coordinator that shares an `OtaspHistory` with management
    /// readers and optionally broadcasts each event to a live event stream.
    pub fn with_history(
        cfg: OtaspConfig,
        home_network: HomeNetworkConfig,
        hlr: Arc<dyn HlrRepository>,
        history: Arc<OtaspHistory>,
        event_tx: Option<tokio::sync::broadcast::Sender<events_proto::MscNetworkEvent>>,
    ) -> Self {
        Self {
            cfg,
            home_network,
            hlr,
            sessions: HashMap::new(),
            history,
            event_tx,
            pending_acks: HashMap::new(),
            next_tag: 0,
            ssd_updates: Vec::new(),
        }
    }

    pub fn owns_ack_tag(&self, tag: u32) -> bool {
        self.pending_acks.contains_key(&tag)
    }

    fn allocate_otasp_tag(&mut self) -> u32 {
        // Sequence a 31-bit counter and OR the namespace bit so the
        // value never collides with SMS-allocated tags. 2^31 worth of
        // tags before wraparound is more than enough for the
        // lifetime of any deployment.
        let raw = OTASP_TAG_NAMESPACE_BIT | (self.next_tag & !OTASP_TAG_NAMESPACE_BIT);
        self.next_tag = self.next_tag.wrapping_add(1);
        raw
    }

    /// Called by the MSC runtime when a BSC `AddsDeliverAck` for an
    /// OTASP-owned tag arrives.
    ///
    /// On `cause = None` the delivery succeeded — drop the pending
    /// entry and continue. On `cause = Some(_)` the MS either
    /// rejected the DBM at L3 (Mobile Station Reject Order with
    /// `REJECTED_TYPE = 0x04`) or the BSC reported a transport-level
    /// failure (channel torn down, L2 ack timeout, etc.). The active
    /// OTASP session for that tag is notified so it can emit a
    /// `BlockSkipped` event for the current phase and advance to the
    /// next plan step instead of waiting on the 5 s inbound-silence
    /// timeout.
    pub async fn handle_adds_deliver_ack(
        &mut self,
        ack: &cdma_ios::AddsDeliverAckMessage,
        a1: &dyn MscA1Endpoint,
    ) {
        let Some(tag) = ack.tag.map(|t| t.0) else {
            return;
        };
        let Some(session_key) = self.pending_acks.remove(&tag) else {
            return;
        };
        let Some(cause) = ack.cause else {
            // Success — nothing more to do.
            return;
        };
        // Failure: ask the session to handle it. Currently this
        // emits a `BlockSkipped` event for the current phase and
        // advances. We follow the same outbound-flush + record
        // pattern as `handle_adds_transfer`.
        let (call_id, record_id, mobile_identity_for_deliver, new_events, mut out) = {
            let Some(entry) = self.sessions.get_mut(&session_key) else {
                warn!(
                    "OTASP: AddsDeliverAck(failure cause=0x{:02x}) for unknown session_key {:?} — dropped",
                    cause.0, session_key
                );
                return;
            };
            let mut rec = Recorder {
                out: &mut entry.pending_out,
                events: &mut entry.events,
                requests: &mut entry.requests,
            };
            entry.session.on_dbm_failed(cause.0, &mut rec);
            entry.last_activity = std::time::Instant::now();
            entry.last_phase =
                label_for_latest_phase(&entry.events).unwrap_or(entry.last_phase.clone());
            // Use the last known IMSI for the outbound flush. The
            // OTASP path uses MEID/ESN for routing and the IMSI is
            // optional here.
            let imsi_for_deliver = cdma_ios::MobileIdentity::Imsi(String::new());
            (
                entry.call_id,
                entry.record_id,
                imsi_for_deliver,
                entry.events.split_off(0),
                entry.pending_out.split_off(0),
            )
        };
        log_events(&new_events);
        self.broadcast_events(&new_events);
        for ev in &new_events {
            self.history.append(record_id, ev.clone());
        }
        if !new_events.is_empty() {
            self.persist_network_progress(record_id).await;
        }
        self.flush_outbound(
            &mut out,
            &mobile_identity_for_deliver,
            call_id,
            &session_key,
            a1,
        )
        .await;
        if let Some(outcome) = self.process_auth_requests(&session_key, a1).await {
            self.finalize_session(&session_key, record_id, outcome)
                .await;
        }
    }

    /// Returns the shared session-history buffer (for management RPCs).
    pub fn history(&self) -> Arc<OtaspHistory> {
        Arc::clone(&self.history)
    }

    /// Return the matched feature code prefix for `dialed_digits`, if any.
    pub fn matched_feature_code(&self, dialed_digits: &str) -> Option<String> {
        self.cfg
            .feature_codes
            .iter()
            .find(|prefix| dialed_digits.starts_with(prefix.as_str()))
            .cloned()
    }

    /// Returns true if the dialed digits start with any configured feature code.
    /// Service option is **not** checked: per C.S0016-D §3.2.1, user-initiated
    /// `*228` originates with a vendor-chosen voice or async-data SO, not
    /// SO 18. (SO 18 is OTAPA, the network-initiated flow.)
    pub fn is_otasp_origination(&self, dialed_digits: &str) -> bool {
        if !self.cfg.enabled {
            return false;
        }
        self.cfg
            .feature_codes
            .iter()
            .any(|prefix| dialed_digits.starts_with(prefix.as_str()))
    }

    pub async fn begin_session(
        &mut self,
        device: HardwareIdentity,
        feature_code: String,
        actual_service_option: u16,
        mobile_identity_for_deliver: cdma_ios::MobileIdentity,
        origination_verified: Option<bool>,
        call_id: u64,
        a1: &dyn MscA1Endpoint,
    ) -> bool {
        self.begin_session_with_initiation(
            crate::otasp::session::SessionInitiation::User,
            device,
            feature_code,
            actual_service_option,
            mobile_identity_for_deliver,
            origination_verified,
            call_id,
            a1,
            None,
        )
        .await
    }

    /// Starts a session. Returns false when no session started, so the caller
    /// must release the call.
    pub async fn begin_session_with_initiation(
        &mut self,
        initiation: crate::otasp::session::SessionInitiation,
        device: HardwareIdentity,
        feature_code: String,
        actual_service_option: u16,
        mobile_identity_for_deliver: cdma_ios::MobileIdentity,
        origination_verified: Option<bool>,
        call_id: u64,
        a1: &dyn MscA1Endpoint,
        target: Option<(Uuid, cdma_hlr::model::MobileIdentityKey)>,
    ) -> bool {
        let mut hlr_lookup = match &target {
            Some((subscriber_id, _)) => match self.hlr.get_subscriber_by_id(*subscriber_id).await {
                Ok(Some(resolved)) => Ok(Some(resolved)),
                Ok(None) => Err(format!(
                    "authorized subscriber {subscriber_id} no longer exists"
                )),
                Err(error) => Err(error),
            },
            None => self
                .hlr
                .resolve_by_hardware_identity(
                    if device.meid.is_some() {
                        None
                    } else {
                        device.esn
                    },
                    device.meid.as_deref(),
                )
                .await
                .map_err(|error| error.to_string()),
        };
        if target.is_some() && hlr_lookup.is_err() {
            warn!(
                "OTASP: session for call_id={call_id} not started: {}",
                hlr_lookup.err().unwrap_or_default()
            );
            return false;
        }
        if target.is_none()
            && matches!(hlr_lookup, Ok(None))
            && let cdma_ios::MobileIdentity::Imsi(imsi) = &mobile_identity_for_deliver
            && let Ok(identity) = cdma_hlr::model::MobileIdentityKey::from_parts(
                Some(imsi.as_str()),
                device.esn,
                device.meid.as_deref(),
            )
        {
            hlr_lookup = self.hlr.resolve_by_identity(&identity).await;
        }
        let mut subscriber_id_for_record: Option<Uuid> = None;
        let hlr = match hlr_lookup {
            Ok(Some(resolved)) => {
                subscriber_id_for_record = Some(resolved.subscriber.subscriber_id);
                let (prl_bytes, prl_meta) =
                    resolve_prl_for_subscriber(self.hlr.as_ref(), &resolved.subscriber).await;
                Some(ResolvedSubscriberInput {
                    imsi: resolved
                        .primary_identity
                        .as_ref()
                        .and_then(|p| p.imsi.clone())
                        .or_else(|| resolved.binding.as_ref().and_then(|b| b.imsi.clone()))
                        .unwrap_or_default(),
                    phone_number: resolved.subscriber.phone_number,
                    prl_bytes,
                    prl_meta,
                    service_programming_code: resolved.subscriber.service_programming_code,
                    firstchp_override: resolved.subscriber.firstchp_override,
                })
            }
            Ok(None) => {
                warn!(
                    "OTASP: no HLR record for device esn={:?} meid={:?} — releasing",
                    device.esn, device.meid
                );
                None
            }
            Err(e) => {
                warn!("OTASP: HLR lookup failed for device {:?}: {e}", device);
                None
            }
        };
        let imsi = hlr
            .as_ref()
            .map(|h| h.imsi.clone())
            .filter(|imsi| !imsi.is_empty());
        let identity = match target {
            Some((_, identity)) => Some(identity),
            None => cdma_hlr::model::MobileIdentityKey::from_parts(
                imsi.as_deref(),
                device.esn,
                device.meid.as_deref(),
            )
            .ok(),
        };
        let key = match (&device.esn, &device.meid) {
            (_, Some(m)) => SessionKey::Meid(m.to_ascii_lowercase()),
            (Some(e), None) => SessionKey::Esn(*e),
            (None, None) => {
                warn!("OTASP: begin_session with no hardware identity — dropped");
                return false;
            }
        };
        let mut session = OtaspSession::new(
            self.cfg.clone(),
            self.home_network.clone(),
            device.clone(),
            feature_code,
            actual_service_option,
            hlr,
        );
        let mut pending_out = Vec::new();
        let mut events = Vec::new();
        let mut requests = Vec::new();
        {
            let mut rec = Recorder {
                out: &mut pending_out,
                events: &mut events,
                requests: &mut requests,
            };
            session.start_with_initiation(initiation, &mut rec);
        }
        let record_id = self.record_session_start(&device, subscriber_id_for_record, &events);
        log_events(&events);
        self.broadcast_events(&events);
        // Remaining events (anything after the SessionStart we already opened
        // the record for) still need to be appended to the history.
        if events.len() > 1 {
            for ev in &events[1..] {
                self.history.append(record_id, ev.clone());
            }
        }
        self.flush_outbound(
            &mut pending_out,
            &mobile_identity_for_deliver,
            call_id,
            &key,
            a1,
        )
        .await;
        self.sessions.insert(
            key,
            SessionEntry {
                initiation,
                session,
                pending_out,
                events: Vec::new(),
                call_id,
                record_id,
                last_activity: std::time::Instant::now(),
                last_phase: if initiation == crate::otasp::session::SessionInitiation::Network {
                    "OTAPA Start"
                } else {
                    "Protocol Capability"
                }
                .to_string(),
                requests,
                identity,
                imsi,
                air_imsi: match &mobile_identity_for_deliver {
                    cdma_ios::MobileIdentity::Imsi(imsi) => Some(imsi.clone()),
                    _ => None,
                },
                esn: device.esn,
                origination_verified,
            },
        );
        self.persist_network_progress(record_id).await;
        info!("OTASP: session started for device {:?}", device);
        true
    }

    /// Falls back to a fresh record with no events if the session driver did
    /// not produce a SessionStart (defensive — shouldn't happen).
    fn record_session_start(
        &self,
        device: &HardwareIdentity,
        subscriber_id: Option<Uuid>,
        events: &[OtaspEvent],
    ) -> Uuid {
        let start_event = events
            .iter()
            .find(|e| matches!(e, OtaspEvent::SessionStart { .. }))
            .cloned()
            .unwrap_or_else(|| OtaspEvent::SessionStart {
                device: device.clone(),
                feature_code: String::new(),
                service_option: cdma_common::consts::SERVICE_OPTION_OTASP,
            });
        self.history
            .open_session(device.clone(), subscriber_id, start_event)
    }

    fn broadcast_events(&self, events: &[OtaspEvent]) {
        if let Some(tx) = self.event_tx.as_ref() {
            for ev in events {
                let _ = tx.send(to_proto_msc_event(ev));
            }
        }
    }

    /// Feed an inbound ADDS Transfer with `burst_type = 0x04` into the
    /// appropriate session. Returns `Some(call_id)` if the session reached a
    /// terminal state and the caller should release the A1 call; returns
    /// `None` otherwise (still active or message dropped).
    pub async fn handle_adds_transfer(
        &mut self,
        msg: &AddsTransferMessage,
        a1: &dyn MscA1Endpoint,
    ) -> Option<u64> {
        if msg.adds_user_part.burst_type != 0x04 {
            return None;
        }
        let Some(key) = SessionKey::from_transfer(msg) else {
            warn!("OTASP: ADDS Transfer burst_type=4 with no hardware identity — dropped");
            return None;
        };
        let mobile_identity_for_deliver = msg.mobile_identity_imsi.clone();
        let (outcome, call_id, record_id, new_events, mut out) = {
            let Some(entry) = self.sessions.get_mut(&key) else {
                warn!(
                    "OTASP: ADDS Transfer for unknown session key {:?} — dropped",
                    key
                );
                return None;
            };
            let outcome = {
                let mut rec = Recorder {
                    out: &mut entry.pending_out,
                    events: &mut entry.events,
                    requests: &mut entry.requests,
                };
                entry.session.on_inbound(&msg.adds_user_part.data, &mut rec)
            };
            let now = std::time::Instant::now();
            stamp_mobile_compute_time(&mut entry.events, now.duration_since(entry.last_activity));
            entry.last_activity = now;
            entry.last_phase =
                label_for_latest_phase(&entry.events).unwrap_or(entry.last_phase.clone());
            (
                outcome,
                entry.call_id,
                entry.record_id,
                entry.events.split_off(0),
                entry.pending_out.split_off(0),
            )
        };
        log_events(&new_events);
        self.broadcast_events(&new_events);
        for ev in &new_events {
            self.history.append(record_id, ev.clone());
        }
        if !new_events.is_empty() {
            self.persist_network_progress(record_id).await;
        }
        self.flush_outbound(&mut out, &mobile_identity_for_deliver, call_id, &key, a1)
            .await;
        let follow_up = self.process_auth_requests(&key, a1).await;
        if let Some(o) = outcome.or(follow_up) {
            self.finalize_session(&key, record_id, o).await;
            return Some(call_id);
        }
        None
    }

    pub(crate) fn take_ssd_updates(&mut self) -> Vec<OtaspSsdUpdate> {
        std::mem::take(&mut self.ssd_updates)
    }

    pub fn owns_call(&self, call_id: u64) -> bool {
        self.sessions.values().any(|e| e.call_id == call_id)
    }

    pub async fn on_ssd_update_finished(
        &mut self,
        call_id: u64,
        ok: bool,
        a1: &dyn MscA1Endpoint,
    ) -> Option<u64> {
        let key = self
            .sessions
            .iter()
            .find(|(_, e)| e.call_id == call_id)
            .map(|(k, _)| k.clone())?;
        let outcome = self
            .with_session(&key, a1, |session, rec| {
                session.on_ssd_update_finished(ok, rec)
            })
            .await;
        let follow_up = self.process_auth_requests(&key, a1).await;
        let outcome = outcome.or(follow_up)?;
        let record_id = self.sessions.get(&key)?.record_id;
        self.finalize_session(&key, record_id, outcome).await;
        Some(call_id)
    }

    async fn with_session<F>(
        &mut self,
        key: &SessionKey,
        a1: &dyn MscA1Endpoint,
        f: F,
    ) -> Option<SessionOutcome>
    where
        F: FnOnce(&mut OtaspSession, &mut Recorder<'_>) -> Option<SessionOutcome>,
    {
        let (outcome, call_id, record_id, new_events, mut out) = {
            let entry = self.sessions.get_mut(key)?;
            let outcome = {
                let mut rec = Recorder {
                    out: &mut entry.pending_out,
                    events: &mut entry.events,
                    requests: &mut entry.requests,
                };
                f(&mut entry.session, &mut rec)
            };
            entry.last_activity = std::time::Instant::now();
            entry.last_phase =
                label_for_latest_phase(&entry.events).unwrap_or(entry.last_phase.clone());
            (
                outcome,
                entry.call_id,
                entry.record_id,
                entry.events.split_off(0),
                entry.pending_out.split_off(0),
            )
        };
        log_events(&new_events);
        self.broadcast_events(&new_events);
        for ev in &new_events {
            self.history.append(record_id, ev.clone());
        }
        if !new_events.is_empty() {
            self.persist_network_progress(record_id).await;
        }
        let deliver_identity = cdma_ios::MobileIdentity::Imsi(String::new());
        self.flush_outbound(&mut out, &deliver_identity, call_id, key, a1)
            .await;
        outcome
    }

    fn emit_authentication(
        &self,
        key: &SessionKey,
        procedure: events_proto::AuthenticationProcedure,
        result: events_proto::AuthenticationResult,
    ) {
        let Some(tx) = self.event_tx.as_ref() else {
            return;
        };
        let Some(entry) = self.sessions.get(key) else {
            return;
        };
        let esn = entry.esn;
        let _ = tx.send(crate::authentication::authentication_event(
            procedure,
            result,
            false,
            entry.imsi.as_deref(),
            esn,
            "",
        ));
    }

    async fn a_key_on_file(
        &self,
        key: &SessionKey,
        identity: Option<&cdma_hlr::model::MobileIdentityKey>,
    ) -> Option<crate::otasp::event::AKeySkipReason> {
        use crate::otasp::event::AKeySkipReason;
        match self.hlr.get_subscriber_auth_state(identity?).await {
            Ok(Some(state)) if state.credentials_provisioned => {}
            Ok(_) => return None,
            Err(error) => {
                warn!("OTASP: HLR auth state lookup failed: {error}");
                return None;
            }
        }
        Some(
            match self
                .sessions
                .get(key)
                .and_then(|entry| entry.origination_verified)
            {
                Some(true) => AKeySkipReason::KeyOnFileVerified,
                Some(false) => AKeySkipReason::KeyOnFileUnverified,
                None => AKeySkipReason::KeyOnFileNotChecked,
            },
        )
    }

    async fn process_auth_requests(
        &mut self,
        key: &SessionKey,
        a1: &dyn MscA1Endpoint,
    ) -> Option<SessionOutcome> {
        use events_proto::{AuthenticationProcedure, AuthenticationResult};
        loop {
            let (request, identity, imsi, esn, call_id) = {
                let entry = self.sessions.get_mut(key)?;
                if entry.requests.is_empty() {
                    return None;
                }
                (
                    entry.requests.remove(0),
                    entry.identity.clone(),
                    entry.air_imsi.clone().or_else(|| entry.imsi.clone()),
                    entry.esn,
                    entry.call_id,
                )
            };
            let outcome = match request {
                OtaspAuthRequest::ValidateSpasm { rand_otapa } => {
                    let signature = match (identity.as_ref(), imsi.as_deref(), esn) {
                        (Some(identity), Some(imsi), Some(esn)) => {
                            if let Some((imsi_s1, _)) = cdma_common::paging::imsi_s_from_imsi(imsi)
                            {
                                match self
                                    .hlr
                                    .compute_otapa_signature(identity, esn, rand_otapa, imsi_s1)
                                    .await
                                {
                                    Ok(signature) => Some(signature),
                                    Err(error) => {
                                        warn!("OTAPA: cannot validate handset NAM lock: {error}");
                                        None
                                    }
                                }
                            } else {
                                None
                            }
                        }
                        _ => None,
                    };
                    self.with_session(key, a1, |session, rec| {
                        session.on_spasm_signature(signature, rec)
                    })
                    .await
                }
                OtaspAuthRequest::BeginKeyExchange { a_key_p_rev } => {
                    if let Some(reason) = self.a_key_on_file(key, identity.as_ref()).await {
                        info!(
                            "OTASP: keeping the A-key on file for {:?}, no key exchange ({:?})",
                            key, reason
                        );
                        return self
                            .with_session(key, a1, |session, rec| {
                                session.on_key_exchange_skipped(reason, rec)
                            })
                            .await;
                    }
                    let offer = match identity.as_ref() {
                        Some(identity) => {
                            match self
                                .hlr
                                .begin_cave_key_exchange(identity, a_key_p_rev)
                                .await
                            {
                                Ok(offer) => Some(KeyExchangeOffer {
                                    a_key_p_rev: offer.a_key_p_rev,
                                    params: offer.params,
                                    bs_result: offer.bs_result,
                                }),
                                Err(error) => {
                                    warn!("OTASP: HLR could not start key exchange: {error}");
                                    None
                                }
                            }
                        }
                        None => {
                            warn!("OTASP: no complete identity for A-key provisioning");
                            None
                        }
                    };
                    if offer.is_none() {
                        self.emit_authentication(
                            key,
                            AuthenticationProcedure::OtaspKeyExchange,
                            AuthenticationResult::HlrError,
                        );
                    }
                    self.with_session(key, a1, |session, rec| {
                        session.on_key_exchange_offer(offer, rec)
                    })
                    .await
                }
                OtaspAuthRequest::CompleteKeyExchange { ms_result } => {
                    let ok = match identity.as_ref() {
                        Some(identity) => {
                            match self
                                .hlr
                                .complete_cave_key_exchange(identity, &ms_result)
                                .await
                            {
                                Ok(()) => true,
                                Err(error) => {
                                    warn!("OTASP: key exchange failed: {error}");
                                    false
                                }
                            }
                        }
                        None => false,
                    };
                    self.emit_authentication(
                        key,
                        AuthenticationProcedure::OtaspKeyExchange,
                        if ok {
                            AuthenticationResult::Success
                        } else {
                            AuthenticationResult::HlrError
                        },
                    );
                    self.with_session(key, a1, |session, rec| {
                        session.on_key_exchange_completed(ok, rec)
                    })
                    .await
                }
                OtaspAuthRequest::SsdUpdate => match (identity, imsi, esn) {
                    (Some(identity), Some(imsi), Some(esn)) => {
                        self.ssd_updates.push(OtaspSsdUpdate {
                            call_id,
                            context: crate::authentication::CallAuthContext {
                                identity,
                                imsi,
                                esn,
                                global_result: crate::authentication::AuthenticationResult::Success,
                            },
                        });
                        None
                    }
                    _ => {
                        self.with_session(key, a1, |session, rec| {
                            session.on_ssd_update_finished(false, rec)
                        })
                        .await
                    }
                },
                OtaspAuthRequest::VerifyReauthentication { rand, response } => {
                    const RANDC_SHIFT: u32 = 24;
                    let randc_matches = (rand >> RANDC_SHIFT) as u8 == response.randc;
                    let verdict = match (identity.as_ref(), esn) {
                        (Some(identity), Some(esn)) if randc_matches => self
                            .hlr
                            .verify_cave_global_challenge(
                                identity,
                                &cdma_hlr::auth::GlobalChallenge {
                                    esn,
                                    rand,
                                    authr: response.authr,
                                    count: Some(response.count),
                                    auth_data: response.auth_data,
                                    generate_call_keys: false,
                                    otasp_reauthentication: true,
                                },
                            )
                            .await
                            .map(|v| v.outcome)
                            .map_err(|error| warn!("OTASP: re-authentication failed: {error}"))
                            .ok(),
                        _ => None,
                    };
                    let ok = verdict == Some(cdma_hlr::auth::CaveAuthOutcome::Success);
                    self.emit_authentication(
                        key,
                        AuthenticationProcedure::OtaspReauthentication,
                        match verdict {
                            Some(cdma_hlr::auth::CaveAuthOutcome::Success) => {
                                AuthenticationResult::Success
                            }
                            Some(cdma_hlr::auth::CaveAuthOutcome::CountMismatch) => {
                                AuthenticationResult::CountMismatch
                            }
                            Some(_) => AuthenticationResult::SignatureMismatch,
                            None if !randc_matches => AuthenticationResult::RandMismatch,
                            None => AuthenticationResult::HlrError,
                        },
                    );
                    self.with_session(key, a1, |session, rec| {
                        session.on_reauthentication_verified(ok, rec)
                    })
                    .await
                }
                OtaspAuthRequest::CommitKey => {
                    if let Some(identity) = identity.as_ref() {
                        match self.hlr.commit_cave_key(identity).await {
                            Ok(()) => {
                                info!("OTASP: new A-key committed for {:?}", key);
                                self.emit_authentication(
                                    key,
                                    AuthenticationProcedure::OtaspKeyExchange,
                                    AuthenticationResult::Success,
                                );
                            }
                            Err(error) => warn!("OTASP: HLR failed to commit A-key: {error}"),
                        }
                    }
                    None
                }
                OtaspAuthRequest::AbortKeyExchange => {
                    if let Some(identity) = identity.as_ref()
                        && let Err(error) = self.hlr.abort_cave_key_exchange(identity).await
                    {
                        warn!("OTASP: failed to abandon key exchange: {error}");
                    }
                    None
                }
            };
            if outcome.is_some() {
                return outcome;
            }
        }
    }

    async fn finalize_session(
        &mut self,
        key: &SessionKey,
        record_id: Uuid,
        outcome: SessionOutcome,
    ) {
        info!(
            "OTASP: session {:?} ended: kind={:?} blocks={}",
            key, outcome.kind, outcome.completed_blocks
        );
        if let Some(entry) = self.sessions.remove(key)
            && entry.session.key_exchange_started()
            && outcome.kind != crate::otasp::event::SessionOutcomeKind::Committed
            && let Some(identity) = entry.identity.as_ref()
            && let Err(error) = self.hlr.abort_cave_key_exchange(identity).await
        {
            warn!("OTASP: failed to abandon key exchange: {error}");
        }
        if let Some(rec) = self.history.take_finished(record_id) {
            self.persist_session_record(rec).await;
        }
    }

    /// Encode an in-flight record to an `OtaspSessionRow` and hand it to
    /// the HLR for durable storage. Failures are logged and dropped —
    /// session history is best-effort.
    async fn persist_network_progress(&self, record_id: Uuid) {
        if self.sessions.values().any(|entry| {
            entry.record_id == record_id
                && entry.initiation == crate::otasp::session::SessionInitiation::Network
        }) && let Some(record) = self.history.snapshot(record_id)
        {
            self.persist_session_record(record).await;
        }
    }

    async fn persist_session_record(&self, rec: crate::otasp::history::OtaspSessionRecord) {
        let row = otasp_record_to_row(&rec, cdma_common::consts::SERVICE_OPTION_OTASP);
        if let Err(e) = self.hlr.save_otasp_session(&row).await {
            warn!(
                "OTASP: save_otasp_session failed for {}: {e}",
                rec.session_id
            );
        }
    }

    /// Scan active sessions and force-release any whose last activity exceeds
    /// `threshold`. Emits `Timeout` + `SessionEnded` events, persists the
    /// assembled record via the HLR, and returns the list of call_ids the
    /// caller should release at the call-control layer.
    pub async fn tick_timeouts(
        &mut self,
        now: std::time::Instant,
        threshold: std::time::Duration,
    ) -> Vec<u64> {
        let mut released = Vec::new();
        let stale_keys: Vec<SessionKey> = self
            .sessions
            .iter()
            .filter(|(_, e)| now.duration_since(e.last_activity) >= session_timeout(e, threshold))
            .map(|(k, _)| k.clone())
            .collect();
        let mut to_persist = Vec::new();
        for key in stale_keys {
            if let Some(entry) = self.sessions.remove(&key) {
                if entry.session.key_exchange_started()
                    && let Some(identity) = entry.identity.as_ref()
                    && let Err(error) = self.hlr.abort_cave_key_exchange(identity).await
                {
                    warn!("OTASP: failed to abandon key exchange: {error}");
                }
                let phase = entry
                    .session
                    .awaited_key_exchange_response()
                    .map(str::to_string)
                    .unwrap_or_else(|| entry.last_phase.clone());
                warn!(
                    "OTASP: session call_id={} timed out after {:?} waiting on '{}' — releasing",
                    entry.call_id,
                    session_timeout(&entry, threshold),
                    phase
                );
                let events = vec![
                    OtaspEvent::Timeout {
                        phase: phase.clone(),
                    },
                    OtaspEvent::SessionEnded {
                        completed_blocks: 0,
                        outcome: crate::otasp::event::SessionOutcomeKind::TimedOut,
                    },
                ];
                log_events(&events);
                self.broadcast_events(&events);
                for ev in &events {
                    self.history.append(entry.record_id, ev.clone());
                }
                if let Some(rec) = self.history.take_finished(entry.record_id) {
                    to_persist.push(rec);
                }
                released.push(entry.call_id);
            }
        }
        for rec in to_persist {
            self.persist_session_record(rec).await;
        }
        released
    }

    async fn flush_outbound(
        &mut self,
        pending: &mut Vec<OutboundOtasp>,
        mobile_identity_imsi: &cdma_ios::MobileIdentity,
        call_id: u64,
        session_key: &SessionKey,
        a1: &dyn MscA1Endpoint,
    ) {
        for out in pending.drain(..) {
            // Allocate a Tag in the OTASP namespace so the BSC can
            // correlate its eventual AddsDeliverAck back to this
            // DBM and the runtime can route it to OTASP rather than
            // SMS. Per A.S0001 §6.1.7.5 the BSC only sends the Ack
            // when ADDS Deliver carries a Tag.
            let tag_raw = self.allocate_otasp_tag();
            self.pending_acks.insert(tag_raw, session_key.clone());
            let deliver = AddsDeliverMessage {
                adds_user_part: AddsUserPart {
                    burst_type: cdma_common::consts::BURST_TYPE_OTASP,
                    data: out.bytes,
                },
                tag: Some(cdma_ios::Tag(tag_raw)),
            };
            match deliver.encode() {
                Ok(payload) => {
                    let msg = cdma_ios::EncodedA1Message::from_message_for_call(
                        &cdma_ios::Message::new(cdma_ios::MessageType::AddsDeliver, payload),
                        Some(call_id),
                    );
                    if let Err(e) = a1.send(msg).await {
                        warn!("OTASP: failed to send ADDS Deliver to BSC: {e}");
                    }
                }
                Err(e) => warn!("OTASP: failed to encode ADDS Deliver: {e}"),
            }
        }
        let _ = mobile_identity_imsi;
    }
}

/// Convert an in-flight session record to the HLR row shape, including a
/// fresh prost encoding of the event timeline as `events.v1.OtaspRecordedEvents`.
fn otasp_record_to_row(
    rec: &crate::otasp::history::OtaspSessionRecord,
    default_service_option: u16,
) -> cdma_hlr::model::OtaspSessionRow {
    use prost::Message;
    let wrap = events_proto::OtaspRecordedEvents {
        events: rec
            .events
            .iter()
            .map(|e| events_proto::OtaspRecordedEvent {
                timestamp: Some(system_time_to_timestamp(e.recorded_at)),
                event: Some(to_proto_otasp_event(&e.event)),
            })
            .collect(),
    };
    let mut buf = Vec::with_capacity(wrap.encoded_len());
    // Encoding a prost message into a Vec only fails on length overflow,
    // which we can't hit with reasonable session sizes.
    wrap.encode(&mut buf).expect("encode OtaspRecordedEvents");

    // Pull the feature_code + service_option from the leading SessionStart.
    let (feature_code, service_option) = rec
        .events
        .iter()
        .find_map(|re| match &re.event {
            OtaspEvent::SessionStart {
                feature_code,
                service_option,
                ..
            } => Some((Some(feature_code.clone()), Some(*service_option as i32))),
            _ => None,
        })
        .unwrap_or((None, Some(default_service_option as i32)));

    // Count successful block downloads as a quick "did this session
    // actually do anything" signal for the list view.
    let completed_blocks = rec
        .events
        .iter()
        .filter(|re| matches!(&re.event, OtaspEvent::BlockDownloaded { .. }))
        .count() as i32;

    let outcome_discriminant = rec
        .outcome
        .map(|o| session_outcome_to_proto(o) as i16)
        .unwrap_or(0);

    cdma_hlr::model::OtaspSessionRow {
        session_id: rec.session_id,
        subscriber_id: rec.subscriber_id,
        esn: rec.device.esn,
        meid: rec.device.meid.clone(),
        started_at: system_time_to_chrono(rec.started_at),
        ended_at: rec.ended_at.map(system_time_to_chrono),
        outcome: outcome_discriminant,
        feature_code,
        service_option,
        completed_blocks,
        event_count: rec.events.len() as i32,
        events_proto: buf,
    }
}

fn system_time_to_timestamp(t: std::time::SystemTime) -> prost_types::Timestamp {
    match t.duration_since(std::time::UNIX_EPOCH) {
        Ok(d) => prost_types::Timestamp {
            seconds: d.as_secs() as i64,
            nanos: d.subsec_nanos() as i32,
        },
        Err(_) => prost_types::Timestamp::default(),
    }
}

fn system_time_to_chrono(t: std::time::SystemTime) -> chrono::DateTime<chrono::Utc> {
    let d = t.duration_since(std::time::UNIX_EPOCH).unwrap_or_default();
    chrono::DateTime::<chrono::Utc>::from_timestamp(d.as_secs() as i64, d.subsec_nanos())
        .unwrap_or_else(chrono::Utc::now)
}

fn session_outcome_to_proto(o: crate::otasp::event::SessionOutcomeKind) -> i32 {
    use crate::otasp::event::SessionOutcomeKind;
    match o {
        SessionOutcomeKind::Committed => events_proto::OtaspSessionOutcome::Committed as i32,
        SessionOutcomeKind::NothingToCommit => {
            events_proto::OtaspSessionOutcome::NothingToCommit as i32
        }
        SessionOutcomeKind::SpcRejected => events_proto::OtaspSessionOutcome::SpcRejected as i32,
        SessionOutcomeKind::HlrUnknown => events_proto::OtaspSessionOutcome::HlrUnknown as i32,
        SessionOutcomeKind::Rejected => events_proto::OtaspSessionOutcome::Rejected as i32,
        SessionOutcomeKind::NoCapacity => events_proto::OtaspSessionOutcome::NoCapacity as i32,
        SessionOutcomeKind::ProtocolError => {
            events_proto::OtaspSessionOutcome::ProtocolError as i32
        }
        SessionOutcomeKind::TimedOut => events_proto::OtaspSessionOutcome::TimedOut as i32,
        SessionOutcomeKind::KeyExchangeFailed => {
            events_proto::OtaspSessionOutcome::KeyExchangeFailed as i32
        }
    }
}

fn stamp_mobile_compute_time(events: &mut [OtaspEvent], waited: std::time::Duration) {
    for event in events {
        if let OtaspEvent::AKeyProgress {
            step, elapsed_ms, ..
        } = event
            && step.mobile_computes()
        {
            *elapsed_ms = Some(u32::try_from(waited.as_millis()).unwrap_or(u32::MAX));
        }
    }
}

fn session_timeout(entry: &SessionEntry, threshold: std::time::Duration) -> std::time::Duration {
    if entry.session.awaited_key_exchange_response().is_some() {
        threshold.max(KEY_EXCHANGE_RESPONSE_TIMEOUT)
    } else {
        threshold
    }
}

/// Pick the most useful phase label from the latest events for a session,
/// so timeout messages name what we were waiting on.
fn label_for_latest_phase(events: &[OtaspEvent]) -> Option<String> {
    events.iter().rev().find_map(|e| match e {
        OtaspEvent::ProtocolCapabilityReceived { .. } => Some("Validation Response".to_string()),
        OtaspEvent::SpcVerified => Some("Block Configuration Response".to_string()),
        OtaspEvent::BlockDownloaded { block_id, .. } => {
            Some(format!("Block {:#04x} response", block_id))
        }
        OtaspEvent::BlockSkipped { .. } => Some("Block Configuration Response".to_string()),
        OtaspEvent::CommitResult { .. } => Some("Session release".to_string()),
        _ => None,
    })
}

fn log_events(events: &[OtaspEvent]) {
    for e in events {
        match e {
            OtaspEvent::SessionStart {
                device,
                feature_code,
                service_option,
            } => {
                info!(
                    "OTASP event: session_start device={:?} code={} so={}",
                    device, feature_code, service_option
                );
            }
            OtaspEvent::ProtocolCapabilityReceived {
                mob_firm_rev,
                mob_model,
                band_mode_cap,
                otasp_p_rev,
                features,
            } => {
                let feature_str = features
                    .iter()
                    .map(|(id, rev)| format!("0x{:02x}@{}", id, rev))
                    .collect::<Vec<_>>()
                    .join(",");
                info!(
                    "OTASP event: protocol_capability firm_rev={:#x} model={:#x} bands={:#04x} otasp_p_rev={:?} features=[{}]",
                    mob_firm_rev, mob_model, band_mode_cap.raw, otasp_p_rev, feature_str
                );
            }
            OtaspEvent::StationClassMark(scm) => info!(
                "OTASP event: SCM raw={:#04x} ext={:?} dual={:?} slotted={:?} meid={:?} bw25={} tx={:?} pwr={}",
                scm.raw,
                scm.extended,
                scm.dual_mode,
                scm.slotted_class,
                scm.meid_support,
                scm.bandwidth_25mhz,
                scm.transmission,
                scm.analog_power_class
            ),
            OtaspEvent::SpcMismatch => warn!("OTASP event: SPC mismatch"),
            OtaspEvent::SpcVerified => info!("OTASP event: SPC verified"),
            OtaspEvent::NamReadback {
                block_id,
                label,
                fields,
                feature: _,
            } => {
                let dump = fields
                    .iter()
                    .map(|(k, v)| format!("{}={}", k, v))
                    .collect::<Vec<_>>()
                    .join(" ");
                info!(
                    "OTASP event: NAM read-back block={:#x} ({}): {}",
                    block_id, label, dump
                );
            }
            OtaspEvent::BlockSkipped {
                block_id,
                reason,
                feature: _,
            } => warn!("OTASP event: block {:#x} skipped — {}", block_id, reason),
            OtaspEvent::Timeout { phase } => {
                warn!(
                    "OTASP event: inbound silence timeout waiting on '{}'",
                    phase
                )
            }
            OtaspEvent::HlrMiss { device } => warn!("OTASP event: HLR miss device={:?}", device),
            OtaspEvent::NoNamCapacity {
                block_id,
                feature: _,
            } => warn!(
                "OTASP event: NAM block {:#x} reports MAX_SID_NID=0",
                block_id
            ),
            OtaspEvent::PrlReadback(rb) => {
                use crate::otasp::event::PrlOutcome;
                match &rb.outcome {
                    PrlOutcome::Decoded(p) => info!(
                        "OTASP event: PRL read-back classic id=0x{:04x} size={} acq={} sys={} crc_ok={}",
                        p.pr_list_id,
                        p.pr_list_size,
                        p.acquisition_records.len(),
                        p.system_records.len(),
                        p.crc_ok()
                    ),
                    PrlOutcome::DecodedExtended { prl, raw_bytes } => info!(
                        "OTASP event: PRL read-back extended id=0x{:04x} size={} p_rev={} acq={} subnet={} sys={} bytes={} crc_ok={}",
                        prl.pr_list_id,
                        prl.pr_list_size,
                        prl.cur_sspr_p_rev,
                        prl.acquisition_records.len(),
                        prl.common_subnet_records.len(),
                        prl.system_records.len(),
                        raw_bytes.len(),
                        prl.crc_ok()
                    ),
                    PrlOutcome::Absent => info!("OTASP event: PRL read-back — MS reports no PRL"),
                    PrlOutcome::FeatureNotAdvertised => {
                        info!("OTASP event: PRL read-back skipped — MS did not advertise SSPR")
                    }
                    PrlOutcome::Rejected {
                        block_id,
                        result_code,
                    } => warn!(
                        "OTASP event: PRL read-back rejected block={:#x} code={:#x}",
                        block_id, result_code
                    ),
                    PrlOutcome::DecodeFailed { reason, raw_bytes } => warn!(
                        "OTASP event: PRL decode failed ({}, {} raw bytes)",
                        reason,
                        raw_bytes.len()
                    ),
                }
            }
            OtaspEvent::BlockDownloaded {
                block_id,
                result_code,
                feature: _,
                fields: _,
            } => info!(
                "OTASP event: block {:#x} downloaded (code={:#x})",
                block_id, result_code
            ),
            OtaspEvent::BlockRejected {
                block_id,
                result_code,
                feature: _,
            } => warn!(
                "OTASP event: block {:#x} rejected (code={:#x})",
                block_id, result_code
            ),
            OtaspEvent::CommitResult { result_code } => {
                info!("OTASP event: commit result_code={:#x}", result_code)
            }
            OtaspEvent::SessionEnded {
                completed_blocks,
                outcome,
            } => info!(
                "OTASP event: session ended blocks={} outcome={:?}",
                completed_blocks, outcome
            ),
            OtaspEvent::AKeyProgress {
                step,
                success,
                a_key_p_rev,
                elapsed_ms,
                result_code,
                skip_reason,
            } => info!(
                "OTASP event: A-key {:?} success={} a_key_p_rev={} elapsed_ms={:?} result_code={:?} skip_reason={:?}",
                step, success, a_key_p_rev, elapsed_ms, result_code, skip_reason
            ),
        }
    }
}
