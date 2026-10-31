use std::time::Duration;

use cdma_common::events::{AccessCellId, AccessChannelEvent};
use cdma_common::lac::{message_types::MessageId, paging_messages::MsAddress};
use log::{info, warn};
use tokio::{sync::mpsc, time::Instant};
use uuid::Uuid;

use super::access::{
    access_response_tx_time, compute_pgslot_from_event, event_last_heard_ms, resolve_imsi_overhead,
};
use super::meid::{FIRST_MEID_COMPATIBLE_P_REV, MeidQueryChannel, can_discover_meid};
use super::{AccessRegistrationUpdate, Bsc};

// C.S0005-E Table 2.7.1-1 sets T42m to 12 seconds. Reserve one second for the final L3 reply.
const IDENTITY_TIMEOUT: Duration = Duration::from_secs(11);
const MAX_PENDING_ACCESS_REQUESTS: usize = 256;

pub(crate) struct AccessIdentityWakeup {
    token: Uuid,
}

struct PendingAccessIdentity {
    token: Uuid,
    addr: MsAddress,
    event: AccessChannelEvent,
    started: Instant,
    deadline: Instant,
    auth_required: bool,
}

pub(crate) struct AccessIdentityService {
    pending: Vec<PendingAccessIdentity>,
    sender: mpsc::Sender<AccessIdentityWakeup>,
    pub(crate) wakeups: mpsc::Receiver<AccessIdentityWakeup>,
}

impl AccessIdentityService {
    pub(crate) fn new() -> Self {
        let (sender, wakeups) = mpsc::channel(MAX_PENDING_ACCESS_REQUESTS);
        Self {
            pending: Vec::new(),
            sender,
            wakeups,
        }
    }

    pub(crate) fn is_pending(&self, addr: &MsAddress, cell: Option<AccessCellId>) -> bool {
        self.pending
            .iter()
            .any(|pending| pending.addr == *addr && pending.event.cell == cell)
    }

    pub(crate) fn next_deadline(&self) -> Option<Instant> {
        self.pending.iter().map(|pending| pending.deadline).min()
    }

    pub(crate) fn cancel_cell(&mut self, cell: AccessCellId) -> Vec<MsAddress> {
        let addresses = self
            .pending
            .iter()
            .filter(|pending| pending.event.cell == Some(cell))
            .map(|pending| pending.addr.clone())
            .collect();
        self.pending
            .retain(|pending| pending.event.cell != Some(cell));
        addresses
    }
}

impl Bsc {
    pub(crate) fn access_authentication_required(&self, event: &AccessChannelEvent) -> bool {
        self.dispatch_auth_required
            .unwrap_or_else(|| self.config.bts.auth_mode(event.cell) != 0)
    }

    pub(crate) fn defer_access_for_identity(&mut self, event: &AccessChannelEvent) -> bool {
        if !matches!(
            event.message_id,
            MessageId::Registration
                | MessageId::Origination
                | MessageId::PageResponse
                | MessageId::Reconnect
                | MessageId::CallRecoveryRequest
                | MessageId::DataBurst
        ) {
            return false;
        }
        let Some(addr) = self.extract_fwd_address(event) else {
            return false;
        };
        let mobile = self.mobiles.get(&addr);
        let same_hardware = mobile.is_some_and(|ms| {
            event.esn.is_none_or(|esn| ms.esn == Some(esn))
                && event.meid.as_deref().is_none_or(|meid| {
                    ms.meid
                        .as_deref()
                        .is_some_and(|cached| cached.eq_ignore_ascii_case(meid))
                })
        });
        self.access_identity.pending.retain(|pending| {
            pending.addr != addr || (same_hardware && pending.event.cell == event.cell)
        });
        if same_hardware
            && mobile.is_some_and(|ms| ms.meid.is_some())
            && !self.access_identity.is_pending(&addr, event.cell)
        {
            return false;
        }
        let revision = event
            .mob_p_rev
            .or_else(|| mobile.map(|ms| ms.mob_p_rev))
            .unwrap_or(FIRST_MEID_COMPATIBLE_P_REV);
        let scm = event.scm.or_else(|| mobile.and_then(|ms| ms.scm));
        if !can_discover_meid(revision, scm) {
            return false;
        }
        let imsi = self.derive_registration_imsi(event);
        let meid = event.meid.as_deref().or_else(|| {
            same_hardware
                .then(|| mobile.and_then(|ms| ms.meid.as_deref()))
                .flatten()
        });
        if meid.is_some() {
            return false;
        }
        let same_cell = mobile.is_some_and(|ms| ms.serving_cell == event.cell);
        let replaces_procedure = self.access_identity.pending.iter().any(|pending| {
            pending.addr == addr
                && same_hardware
                && same_cell
                && pending.event.message_id != event.message_id
                && pending.event.message_id != MessageId::DataBurst
                && event.message_id != MessageId::DataBurst
        });
        self.access_identity.pending.retain(|pending| {
            pending.addr != addr
                || (same_hardware
                    && same_cell
                    && (event.message_id == MessageId::DataBurst
                        || pending.event.message_id == MessageId::DataBurst))
        });
        if self.access_identity.pending.len() >= MAX_PENDING_ACCESS_REQUESTS {
            warn!("BSC: identity discovery capacity reached, using existing access policy");
            return false;
        }
        let params = self.config.bts.params(event.cell);
        let (imsi_mcc, imsi_11_12) = resolve_imsi_overhead(event, params.mcc, params.imsi_11_12);
        if replaces_procedure || mobile.is_some_and(|ms| ms.serving_cell != event.cell) {
            self.mobiles.update(&addr, |ms| ms.meid_query = None);
        }
        if !same_hardware {
            self.mobiles.update(&addr, |ms| {
                ms.meid = None;
                ms.meid_query = None;
                ms.subscriber_id = None;
                ms.phone_number = None;
                ms.subscriber_display_name = None;
            });
        }
        self.mobiles.apply_access_registration(
            event,
            AccessRegistrationUpdate {
                fwd_address: addr.clone(),
                registration_imsi: imsi,
                imsi_mcc,
                imsi_11_12,
                mob_p_rev: revision,
                last_msg_seq: event.msg_seq.unwrap_or_default(),
                slot_cycle_index: event.slot_cycle_index.unwrap_or_default(),
                pgslot: compute_pgslot_from_event(event),
                activity_now: std::time::Instant::now(),
                last_heard_ms: event_last_heard_ms(event),
                explicit_registration: false,
            },
        );
        if event.ack_req
            && let Some(seq) = event.msg_seq
        {
            self.access_tx.hold_bs_ack_order(
                event.cell,
                &addr,
                seq,
                access_response_tx_time(event),
                self.access_ack_deadline(event),
            );
        }
        let token = Uuid::new_v4();
        let started = Instant::now();
        self.access_identity.pending.push(PendingAccessIdentity {
            token,
            addr,
            event: event.clone(),
            started,
            deadline: started + IDENTITY_TIMEOUT,
            auth_required: self.access_authentication_required(event),
        });
        let _ = self
            .access_identity
            .sender
            .try_send(AccessIdentityWakeup { token });
        true
    }

    pub(crate) fn wake_access_identity(&mut self, addr: &MsAddress, cell: Option<AccessCellId>) {
        let Some(ms) = self.mobiles.get(addr) else {
            return;
        };
        if ms.meid.is_none() {
            return;
        }
        for pending in &self.access_identity.pending {
            if pending.addr == *addr && pending.event.cell == cell {
                let _ = self.access_identity.sender.try_send(AccessIdentityWakeup {
                    token: pending.token,
                });
            }
        }
    }

    pub(crate) async fn handle_access_identity_wakeup(&mut self, result: AccessIdentityWakeup) {
        let Some(index) = self
            .access_identity
            .pending
            .iter()
            .position(|pending| pending.token == result.token)
        else {
            return;
        };
        if self.access_identity.pending[index].deadline <= Instant::now() {
            self.finish_access_identity(index).await;
            return;
        }
        let addr = self.access_identity.pending[index].addr.clone();
        if self.mobiles.get(&addr).is_some_and(|ms| ms.meid.is_none()) {
            self.request_meid(&addr, MeidQueryChannel::Paging);
            if self
                .mobiles
                .get(&addr)
                .is_some_and(|ms| ms.meid_query.is_some())
            {
                return;
            }
        }
        self.finish_access_identity(index).await;
    }

    async fn finish_access_identity(&mut self, index: usize) {
        let pending = self.access_identity.pending.remove(index);
        let Some(ms) = self.mobiles.get(&pending.addr) else {
            return;
        };
        if ms.serving_cell != pending.event.cell
            || pending.event.esn.is_some_and(|esn| ms.esn != Some(esn))
        {
            return;
        }
        if !self
            .access_identity
            .is_pending(&pending.addr, pending.event.cell)
        {
            self.mobiles
                .update(&pending.addr, |ms| ms.meid_query = None);
        }
        let Some(ms) = self.mobiles.get(&pending.addr) else {
            return;
        };
        let mut event = pending.event.clone();
        // AUTHR remains tied to the original request, but L2 must acknowledge the latest reverse PDU.
        event.msg_seq = Some(ms.last_msg_seq);
        event.meid = ms.meid.clone().or(event.meid);
        info!(
            "BSC: access identity ready addr={} type={} elapsed_ms={} meid_present={}",
            crate::addressing::format_ms_address(&pending.addr),
            event.msg_type_name,
            pending.started.elapsed().as_millis(),
            event.meid.is_some()
        );
        // Deferred L3 replies must not reuse the original access transmit deadline.
        event.ack_req = false;
        event.receive_time = None;
        let previous_auth_required = self.dispatch_auth_required.replace(pending.auth_required);
        let mut access = std::mem::take(&mut self.access_service);
        access.dispatch_access_event(self, event).await;
        self.access_service = access;
        self.dispatch_auth_required = previous_auth_required;
    }

    pub(crate) async fn handle_access_identity_order(&mut self, event: &AccessChannelEvent) {
        let Some(addr) = self.extract_fwd_address(event) else {
            return;
        };
        if event.order_code == Some(super::RELEASE_ORDER_CODE) {
            self.access_identity.pending.retain(|pending| {
                pending.addr != addr
                    || pending.event.cell != event.cell
                    || pending.event.message_id == MessageId::DataBurst
            });
            if !self.access_identity.is_pending(&addr, event.cell) {
                self.mobiles.update(&addr, |ms| ms.meid_query = None);
            }
            return;
        }
        let rejected_status = match event.decoded_l3.as_ref() {
            Some(cdma_common::access::AccessMessage::Order(order)) => order
                .parse_mobile_station_reject_order(
                    cdma_common::lac::message_types::WireChannel::ForwardCommon,
                )
                .is_some_and(|detail| {
                    Some(detail.rejected_type)
                        == MessageId::StatusRequest
                            .wire_type(cdma_common::lac::message_types::WireChannel::ForwardCommon)
                }),
            _ => false,
        };
        if !rejected_status
            || !self
                .mobiles
                .get(&addr)
                .is_some_and(|ms| ms.serving_cell == event.cell && ms.meid_query.is_some())
        {
            return;
        }
        if let Some(seq) = event.msg_seq {
            self.mobiles.update(&addr, |ms| ms.last_msg_seq = seq);
        }
        while let Some(index) = self
            .access_identity
            .pending
            .iter()
            .position(|pending| pending.addr == addr && pending.event.cell == event.cell)
        {
            Box::pin(self.finish_access_identity(index)).await;
        }
    }

    pub(crate) async fn expire_access_identities(&mut self, now: Instant) {
        while let Some(index) = self
            .access_identity
            .pending
            .iter()
            .position(|pending| pending.deadline <= now)
        {
            warn!("BSC: access identity discovery timed out");
            self.finish_access_identity(index).await;
        }
    }
}
