use std::time::{Duration, Instant};

use cdma_common::error::Error;
use log::{debug, info, warn};

use super::mobiles::StaleTrafficChannel;
use super::traffic_channel::ChannelState;
use super::voice::VoiceAlertMode;
use super::{A1ClearState, Bsc, TrafficChannelAction, recv_or_pending, recv_unbounded_or_pending};
use crate::addressing::is_packet_data_so;

/// A.S0014-D §4.2.16 Cause values for Clear Request.
const CLEAR_CAUSE_RADIO_INTERFACE_MESSAGE_FAILURE: u8 = 0x00;
pub(crate) const CLEAR_CAUSE_RADIO_INTERFACE_FAILURE: u8 = 0x01;

impl Bsc {
    pub async fn run(mut self) -> Result<(), Error> {
        debug!("BSC starting.");

        let mut access_rx = self.config.access_event_rx.take();
        let mut cell_detach_rx = self.config.cell_detach_rx.take();
        let mut sms_rx = self.config.sms_request_rx.take();
        let mut data_rx = self.config.data_request_rx.take();
        let mut power_override_rx = self.config.power_override_request_rx.take();
        let msc_client = self.config.msc_client.clone();
        let traffic_timeout = Duration::from_secs(self.config.traffic_assignment.idle_timeout_s);
        let ms_ack_timeout =
            Duration::from_millis(self.config.traffic_assignment.ms_ack_timeout_ms);
        let packet_service_connect_timeout = Duration::from_millis(
            self.config
                .traffic_assignment
                .packet_service_connect_timeout_ms,
        );
        let mut stale_channel_interval = tokio::time::interval(Duration::from_secs(1));
        let bts_release_request_signal = self.config.bts.bts_release_request_signal();
        let mut bearer_poll_interval = tokio::time::interval(Duration::from_millis(20));

        loop {
            self.drain_pch_transfer_acks().await;

            let retry_sleep = async {
                match self.paging.next_retry_at() {
                    Some(deadline) => tokio::time::sleep_until(deadline).await,
                    None => std::future::pending().await,
                }
            };

            // Paging retries are now handled by the BTS (OTA retransmission
            // with L2 ARQ). The BSC is notified of outcomes via
            // PchMessageTransferAck (cause on failure, bts_l2_termination on
            // success).

            let voice_poll_deadline = self.next_voice_poll_deadline();
            let voice_poll_sleep = async {
                match voice_poll_deadline {
                    Some(deadline) => tokio::time::sleep_until(deadline).await,
                    None => std::future::pending().await,
                }
            };

            let traffic_lifecycle_deadline = self
                .next_traffic_lifecycle_deadline(ms_ack_timeout)
                .into_iter()
                .chain(self.next_packet_service_connecting_deadline(packet_service_connect_timeout))
                .min();
            let traffic_lifecycle_sleep = async {
                match traffic_lifecycle_deadline {
                    Some(deadline) => tokio::time::sleep_until(deadline).await,
                    None => std::future::pending().await,
                }
            };

            let held_bs_ack_due = self.access_tx.next_held_bs_ack_due();
            let held_bs_ack_sleep = async {
                match held_bs_ack_due {
                    Some(deadline) => tokio::time::sleep_until(deadline).await,
                    None => std::future::pending().await,
                }
            };

            let identity_deadline = self.access_identity.next_deadline();
            let identity_sleep = async {
                match identity_deadline {
                    Some(deadline) => tokio::time::sleep_until(deadline).await,
                    None => std::future::pending().await,
                }
            };

            tokio::select! {
                Some(result) = self.access_identity.wakeups.recv() => {
                    self.handle_access_identity_wakeup(result).await;
                }
                _ = identity_sleep => {
                    self.expire_access_identities(tokio::time::Instant::now()).await;
                }
                Some(event) = recv_unbounded_or_pending(access_rx.as_mut()) => {
                    let event = self.enrich_uplink_event(event);
                    if !event.is_traffic_phy_status {
                        self.events.publish_access_event(event.clone());
                    }
                    self.handle_access_event(event).await;
                }
                Some(sms_req) = recv_or_pending(sms_rx.as_mut()) => {
                    self.handle_sms_request(sms_req);
                }
                Some(data_req) = recv_or_pending(data_rx.as_mut()) => {
                    self.initiate_bs_data_call(data_req);
                }
                Some(power_req) = recv_or_pending(power_override_rx.as_mut()) => {
                    self.handle_traffic_power_override_request(power_req);
                }
                () = bts_release_request_signal.notified() => {
                    self.handle_bts_release_requests().await;
                }
                Some(cell) = recv_unbounded_or_pending(cell_detach_rx.as_mut()) => {
                    self.handle_cell_detached(cell).await;
                }
                result = async {
                    match self.config.msc_voice_bearer.as_ref() {
                        Some(bearer) => bearer.recv().await,
                        None => std::future::pending().await,
                    }
                } => {
                    match result {
                        Some(cdma_ios::BearerEvent::Voice(frame)) => {
                            self.handle_forward_bearer_frame(frame);
                        }
                        Some(cdma_ios::BearerEvent::Dtmf(event)) => {
                            self.handle_forward_bearer_dtmf(event);
                        }
                        None => {}
                    }
                }
                Some(resolution) = self.hlr_result_rx.recv() => {
                    self.apply_hlr_resolution(resolution);
                }
                Some(message) = async {
                    msc_client.poll_a1().await.ok().flatten()
                } => {
                    self.handle_incoming_a1_message(message).await;
                }
                _ = retry_sleep => {
                    self.handle_page_retry();
                }
                _ = held_bs_ack_sleep => {
                    self.access_tx
                        .send_due_bs_acks(tokio::time::Instant::now());
                }
                _ = voice_poll_sleep => {
                    self.poll_voice_calls().await;
                }
                _ = traffic_lifecycle_sleep => {
                    self.poll_traffic_channel_lifecycle(ms_ack_timeout).await;
                    self.poll_packet_service_connecting(packet_service_connect_timeout).await;
                }
                _ = stale_channel_interval.tick() => {
                    self.drain_pch_transfer_acks().await;
                    self.teardown_stale_traffic_channels(traffic_timeout).await;
                    self.evict_stale_mobiles();
                    self.expire_stale_pending_a1_failures();
                    self.expire_pending_registrations();
                    self.poll_access_measurements().await;
                    self.refresh_alerting_calls();
                }
                _ = bearer_poll_interval.tick() => {
                    self.poll_reverse_bearer_preambles().await;
                    self.apply_rx_measurements();
                    self.poll_privacy_transitions();
                }
            }
        }
    }

    pub(crate) fn next_traffic_lifecycle_deadline(
        &self,
        ms_ack_timeout: Duration,
    ) -> Option<tokio::time::Instant> {
        self.mobiles
            .iter()
            .filter_map(|ms| {
                ms.traffic_channel()
                    .and_then(|tc| tc.next_traffic_lifecycle_deadline(ms_ack_timeout))
            })
            .min()
            .map(tokio::time::Instant::from_std)
    }

    /// The mobile restarts its T53m answer timer on every Alert With Information
    /// Message (C.S0005-E §2.6.10.1.2).
    pub(crate) fn refresh_alerting_calls(&mut self) {
        let interval = match self.config.traffic_assignment.mt_alert_refresh_s {
            0 => return,
            seconds => Duration::from_secs(seconds),
        };
        let now = Instant::now();
        let due: Vec<_> = self
            .mobiles
            .iter()
            .filter_map(|ms| {
                let tc = ms.traffic_channel()?;
                match tc.channel_state {
                    ChannelState::VoiceAlerting {
                        awim_sent_at,
                        mode: VoiceAlertMode::WaitForConnectOrder,
                    } if now.duration_since(awim_sent_at) >= interval => {
                        Some((ms.serving_cell?, tc.walsh_code, tc.voice_session_id))
                    }
                    _ => None,
                }
            })
            .collect();
        for (cell, walsh_code, session_id) in due {
            let calling_party = session_id
                .and_then(|id| self.voice.session(id))
                .and_then(|session| session.calling_party_record.clone());
            match self.send_standard_alert(
                cell,
                walsh_code,
                super::DEFAULT_TRAFFIC_ACK_SEQ,
                calling_party,
            ) {
                Ok(()) => {
                    debug!(
                        "BSC: re-sent the alert on walsh={} to restart the mobile's answer timer",
                        walsh_code
                    );
                    if let Some(tc) = self.mobiles.get_traffic_channel_mut(cell, walsh_code) {
                        tc.mark_voice_alerting(VoiceAlertMode::WaitForConnectOrder);
                    }
                }
                Err(error) => warn!(
                    "BSC: failed to re-send the alert on walsh={}: {}",
                    walsh_code, error
                ),
            }
        }
    }

    pub(crate) async fn poll_traffic_channel_lifecycle(&mut self, ms_ack_timeout: Duration) {
        let now = Instant::now();
        let actions: Vec<_> = self
            .mobiles
            .iter()
            .filter_map(|ms| {
                let tc = ms.traffic_channel()?;
                match tc.traffic_lifecycle_action(ms_ack_timeout, now) {
                    TrafficChannelAction::Teardown { reason, timeout_ms } => Some((
                        ms.serving_cell?,
                        tc.walsh_code,
                        tc.voice_session_id,
                        tc.voice_leg_role,
                        reason,
                        timeout_ms,
                    )),
                    TrafficChannelAction::None => None,
                }
            })
            .collect();

        for (cell, walsh_code, voice_session_id, voice_leg_role, reason, timeout_ms) in actions {
            warn!(
                "BSC: {} on walsh={} ({}ms), tearing down",
                reason, walsh_code, timeout_ms
            );
            self.teardown_traffic_channel(cell, walsh_code).await;
            self.on_voice_leg_released(voice_session_id, voice_leg_role);
        }
    }

    pub(crate) fn next_packet_service_connecting_deadline(
        &self,
        packet_service_connect_timeout: Duration,
    ) -> Option<tokio::time::Instant> {
        self.mobiles
            .iter()
            .filter_map(|ms| {
                let tc = ms.traffic_channel()?;
                if !is_packet_data_so(tc.service_option) {
                    return None;
                }
                tc.next_packet_service_connecting_deadline(packet_service_connect_timeout)
            })
            .min()
            .map(tokio::time::Instant::from_std)
    }

    pub(crate) async fn poll_packet_service_connecting(
        &mut self,
        packet_service_connect_timeout: Duration,
    ) {
        let now = Instant::now();
        let actions: Vec<_> = self
            .mobiles
            .iter()
            .filter_map(|ms| {
                let tc = ms.traffic_channel()?;
                if !is_packet_data_so(tc.service_option) {
                    return None;
                }
                match tc.packet_service_connecting_action(packet_service_connect_timeout, now) {
                    TrafficChannelAction::Teardown { reason, timeout_ms } => Some((
                        ms.serving_cell?,
                        tc.walsh_code,
                        tc.voice_session_id,
                        tc.voice_leg_role,
                        reason,
                        timeout_ms,
                    )),
                    TrafficChannelAction::None => None,
                }
            })
            .collect();

        for (cell, walsh_code, voice_session_id, voice_leg_role, reason, timeout_ms) in actions {
            warn!(
                "BSC: {} on walsh={} ({}ms), tearing down",
                reason, walsh_code, timeout_ms
            );
            self.teardown_traffic_channel(cell, walsh_code).await;
            self.on_voice_leg_released(voice_session_id, voice_leg_role);
        }
    }

    pub(crate) async fn teardown_stale_traffic_channels(&mut self, traffic_timeout: Duration) {
        let stale_entries = self.mobiles.stale_traffic_channels(traffic_timeout);

        for stale in stale_entries.into_iter().rev() {
            warn!(
                "BSC: traffic channel walsh={} inactive for {}s (channel_state={:?}), releasing",
                stale.walsh_code, stale.inactive_secs, stale.channel_state_label
            );
            self.release_traffic_channel(stale, CLEAR_CAUSE_RADIO_INTERFACE_MESSAGE_FAILURE)
                .await;
        }
    }

    /// The channel is torn down when the Release Order is acknowledged or the MSC
    /// answers with Clear Command, whichever comes first.
    pub(crate) async fn release_traffic_channel(
        &mut self,
        chan: StaleTrafficChannel,
        clear_cause: u8,
    ) {
        if let (Some(call_id), A1ClearState::Idle) = (chan.a1_call_id, chan.a1_clear_state) {
            self.a1.send_clear_request(call_id, clear_cause);
            self.mobiles.update_tc(chan.cell, chan.walsh_code, |_, tc| {
                tc.mark_a1_clear_request_sent();
            });
        }
        if let Err(e) = self.send_traffic_release_order(
            chan.cell,
            chan.walsh_code,
            super::DEFAULT_TRAFFIC_ACK_SEQ,
        ) {
            warn!(
                "BSC: failed to send Release Order on F-TCH walsh={}: {}; tearing down immediately",
                chan.walsh_code, e
            );
            self.teardown_traffic_channel(chan.cell, chan.walsh_code)
                .await;
            self.on_voice_leg_released(chan.voice_session_id, chan.voice_leg_role);
            return;
        }
        self.mobiles.update_tc(chan.cell, chan.walsh_code, |_, tc| {
            tc.mark_releasing();
        });
    }

    /// A.S0003-A §5.3: the BSC answers a BTS Release Request with Clear Request
    /// to the MSC.
    pub(crate) async fn handle_bts_release_requests(&mut self) {
        let requests: Vec<_> = self
            .config
            .bts
            .in_service()
            .iter()
            .filter_map(|entry| Some((entry.cell(), entry.control()?)))
            .flat_map(|(cell, client)| {
                client
                    .drain_bts_release_requests()
                    .into_iter()
                    .map(move |request| (cell, request))
            })
            .collect();
        for (cell, request) in requests {
            let Some(chan) = self
                .mobiles
                .traffic_channel_to_release(cell, request.walsh_code)
            else {
                info!(
                    "BSC: BTS Release Request for walsh={} with no channel to release",
                    request.walsh_code
                );
                continue;
            };
            warn!(
                "BSC: BTS lost the radio link on walsh={} (channel_state={:?}, Abis cause={:?}), clearing the call",
                chan.walsh_code, chan.channel_state_label, request.cause
            );
            self.release_traffic_channel(chan, CLEAR_CAUSE_RADIO_INTERFACE_FAILURE)
                .await;
        }
    }

    /// Release everything bound to a cell whose Abis link dropped. The BTS
    /// frees its side when the connection closes, so nothing will send the
    /// Remove or the Release Order for these — tear them down directly and
    /// tell the MSC the calls are gone.
    pub(crate) async fn handle_cell_detached(&mut self, cell: cdma_common::events::AccessCellId) {
        for addr in self.access_identity.cancel_cell(cell) {
            self.mobiles.update(&addr, |ms| ms.meid_query = None);
        }
        let channels = self.mobiles.traffic_channels_on_cell(cell);
        if channels.is_empty() {
            return;
        }
        warn!(
            "BSC: cell {:?} detached with {} active traffic channel(s), releasing them",
            cell,
            channels.len()
        );
        for chan in channels {
            if let (Some(call_id), A1ClearState::Idle) = (chan.a1_call_id, chan.a1_clear_state) {
                self.a1.send_clear_request(call_id, 0);
                self.mobiles.update_tc(chan.cell, chan.walsh_code, |_, tc| {
                    tc.mark_a1_clear_request_sent();
                });
            }
            self.teardown_traffic_channel(chan.cell, chan.walsh_code)
                .await;
            self.on_voice_leg_released(chan.voice_session_id, chan.voice_leg_role);
        }
    }

    fn expire_stale_pending_a1_failures(&mut self) {
        const PENDING_A1_FAILURE_TIMEOUT: Duration = Duration::from_secs(10);
        let now = Instant::now();
        self.pending_a1_failure_after_release.retain(|(addr, entry)| {
            let aged = now.duration_since(entry.queued_at) > PENDING_A1_FAILURE_TIMEOUT;
            if aged {
                warn!(
                    "BSC: discarding pending A1 AssignmentFailure for {} call_id={} (TCH teardown did not complete within {}s)",
                    crate::addressing::format_ms_address(addr),
                    entry.call_id,
                    PENDING_A1_FAILURE_TIMEOUT.as_secs(),
                );
            }
            !aged
        });
    }

    fn evict_stale_mobiles(&mut self) {
        let evicted = self.evict_idle_mobiles();
        if evicted > 0 {
            info!(
                "BSC: evicted {} idle mobile(s) (timeout={}s, remaining={})",
                evicted,
                self.config.mobile_idle_timeout_s,
                self.mobiles.tracked_count(),
            );
            self.publish_mobiles();
        }
    }
}
