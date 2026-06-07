//! Traffic-channel assignment paths for the BSC.
//!
//! Allocation of forward traffic channels (RC1 / RC3 / SCH), Channel
//! Assignment / Extended Channel Assignment message construction, and
//! manual power-override entrypoints. The natural home for the
//! Abis-Connect / Connect-Ack / Status state machines as WS-1 lands.
//! WS-0 PR3 sibling module per
//! `docs/architecture-update/09-pr3-method-map.md`.

use cdma_common::consts::{SERVICE_OPTION_HIGH_RATE_PACKET_DATA, SERVICE_OPTION_SMS};
use cdma_common::error::Error;
use cdma_common::events::AccessChannelEvent;
use cdma_common::lac::message_types::MessageId;
use cdma_common::lac::paging_messages::{
    ChannelAssignmentMessage, ExtendedChannelAssignmentMessage, MsAddress, PagingChannelMessage,
};
use cdma_common::overhead::OverheadParameters;
use cdma_common::phy::long_code::LongCodeGenerator;
use cdma_common::traffic::TrafficRxRequest;
use log::info;
use uuid::Uuid;

use crate::abis_edge::BtsTrafficChannelHandle;
use crate::addressing::{format_ms_address, is_packet_data_so, select_initial_traffic_rcs};

use super::{
    Bsc, MobileRegistryService, MsState, ServiceNegotiationMode, TrafficChannelInfo,
    TrafficPowerOverrideAction, TrafficPowerOverrideRequest, VoiceLegRole, VoiceService,
    traffic_channel_power_snapshot,
};

#[derive(Default)]
pub(crate) struct TrafficAssignmentService;

impl TrafficAssignmentService {
    pub(crate) fn assign_channel_to_mobile(
        &self,
        mobiles: &mut MobileRegistryService,
        voice: &mut VoiceService,
        overhead: &OverheadParameters,
        fwd_address: &MsAddress,
        handle: BtsTrafficChannelHandle,
        service_option: u16,
        origination_service_option: Option<u16>,
        service_ref_id: u8,
        service_negotiation_mode: ServiceNegotiationMode,
        active_set_pns: Vec<u16>,
        session_id: Option<Uuid>,
        leg_role: Option<VoiceLegRole>,
        a1_call_id: Option<u64>,
    ) -> (u8, (u8, u8)) {
        let walsh_code = handle.walsh_code;
        let assigned_rcs = (handle.for_rc, handle.rev_rc);
        let ccr = voice.allocate_call_connection_ref(overhead);
        mobiles.update(fwd_address, |ms| {
            ms.set_state(MsState::TrafficAssigning);
            ms.assign_traffic_channel(TrafficChannelInfo::new(
                ccr,
                handle,
                service_option,
                origination_service_option,
                service_ref_id,
                service_negotiation_mode,
                active_set_pns,
                session_id,
                leg_role,
                a1_call_id,
            ));
        });
        (walsh_code, assigned_rcs)
    }

    pub(crate) fn service_negotiation_mode_for_mob_p_rev(mob_p_rev: u8) -> ServiceNegotiationMode {
        if mob_p_rev >= 6 {
            ServiceNegotiationMode::ServiceNegotiation
        } else {
            ServiceNegotiationMode::ServiceOptionNegotiation
        }
    }
}

impl Bsc {
    pub(crate) async fn try_assign_access_sms_traffic(
        &mut self,
        fwd_address: &MsAddress,
        event: &AccessChannelEvent,
        last_msg_seq: u8,
    ) -> bool {
        if event.service_option != Some(SERVICE_OPTION_SMS) {
            return false;
        }

        let bts_client = self.config.bts_client.clone();
        let ack_deadline = self.access_ack_deadline(event);

        let Some(ms) = self.mobiles.get(fwd_address) else {
            return false;
        };
        let esn = ms.esn.unwrap_or(0);
        let traffic_lc = LongCodeGenerator::new_traffic_channel(esn);

        if let Some(existing_tc) = ms.pending_traffic_assignment() {
            let walsh_code = existing_tc.walsh_code;
            let assigned_rcs = (existing_tc.for_rc, existing_tc.rev_rc);
            let rc_label = existing_tc.rc_label;
            info!(
                "BSC: reusing pending {} traffic channel walsh={} for {} (SO6 retry)",
                rc_label,
                walsh_code,
                format_ms_address(fwd_address)
            );

            match self.traffic_assignment.send_channel_assignment(
                &self.mobiles,
                &self.access_tx,
                self.config.pilot_offset,
                &self.config.overhead,
                &self.config.traffic_assignment,
                fwd_address,
                last_msg_seq,
                walsh_code,
                Some(assigned_rcs),
                super::access::access_response_tx_time(event),
                ack_deadline,
            ) {
                Ok(()) => {
                    self.mobiles.update(fwd_address, |ms| {
                        ms.mark_traffic_channel_assigned(walsh_code);
                    });
                }
                Err(e) => {
                    log::warn!(
                        "BSC: failed to resend Channel Assignment for {} on walsh={}: {}",
                        format_ms_address(fwd_address),
                        walsh_code,
                        e
                    );
                }
            }
            return true;
        }

        let Some(bts_client) = bts_client.as_ref() else {
            log::warn!("BSC: traffic channels not configured, falling back to BS Ack");
            return false;
        };

        let selected_rcs = {
            let ms = self.mobiles.get(fwd_address).expect("mobile exists");
            select_initial_traffic_rcs(
                &self.config.traffic_assignment,
                &ms.for_supported_rcs,
                &ms.rev_supported_rcs,
                ms.for_preferred_rc,
                ms.rev_preferred_rc,
                ms.mob_p_rev,
            )
        };
        let Some(selected_rcs) = selected_rcs else {
            let ms = self.mobiles.get(fwd_address).expect("mobile exists");
            log::warn!(
                "BSC: no configured traffic RC pair matches mobile {} capabilities for_rcs={:?} rev_rcs={:?} prefs=({:?},{:?}); falling back to BS Ack",
                format_ms_address(fwd_address),
                ms.for_supported_rcs,
                ms.rev_supported_rcs,
                ms.for_preferred_rc,
                ms.rev_preferred_rc,
            );
            return false;
        };
        let use_rc3 = selected_rcs == (3, 3);

        // F-SCH is only for SO33 packet data.
        let alloc_result = if use_rc3 {
            bts_client
                .allocate_rc3_traffic(traffic_lc.clone(), 0, 12, esn, false)
                .await
        } else {
            bts_client
                .allocate_rc1_traffic(traffic_lc.clone(), 0, esn)
                .await
        };

        let Some(handle) = alloc_result else {
            log::warn!("BSC: no Walsh codes available for traffic channel, falling back to BS Ack");
            return false;
        };

        let walsh_code = handle.walsh_code;
        info!(
            "BSC: allocated {} traffic channel walsh={} for {} (SO6 SMS)",
            if use_rc3 { "RC3" } else { "RC1" },
            walsh_code,
            format_ms_address(fwd_address)
        );

        let old_walsh = self
            .mobiles
            .get(fwd_address)
            .and_then(|ms| ms.current_traffic_walsh());
        if let Some(old_walsh) = old_walsh {
            info!(
                "BSC: tearing down existing traffic channel walsh={} before SO6 assignment",
                old_walsh
            );
            self.teardown_traffic_channel(old_walsh).await;
        }

        let (walsh_code, assigned_rcs) = self.assign_traffic_channel_to_mobile(
            fwd_address,
            handle,
            6,
            event.service_option,
            1,
            None,
            None,
            None,
        );

        let ms_gating = self
            .mobiles
            .get(fwd_address)
            .map(|ms| ms.rev_fch_gating_req)
            .unwrap_or(false);
        bts_client
            .install_rx_request(TrafficRxRequest {
                walsh_code,
                esn,
                assigned_rev_rc: assigned_rcs.1,
                preamble_num_pcgs: None,
                rev_fch_gating_mode: ms_gating && (3..=6).contains(&assigned_rcs.1),
            })
            .await;
        info!("BSC: requested reverse traffic RX for walsh={}", walsh_code);

        if let Err(e) = self.traffic_assignment.send_channel_assignment(
            &self.mobiles,
            &self.access_tx,
            self.config.pilot_offset,
            &self.config.overhead,
            &self.config.traffic_assignment,
            fwd_address,
            last_msg_seq,
            walsh_code,
            Some(assigned_rcs),
            super::access::access_response_tx_time(event),
            ack_deadline,
        ) {
            log::warn!(
                "BSC: failed to send Channel Assignment for {}: {}",
                format_ms_address(fwd_address),
                e
            );
            let bearer = self.config.msc_voice_bearer.clone();
            self.mobiles.update(fwd_address, |ms| {
                ms.set_state(MsState::Registered);
                ms.remove_traffic_channel_by_walsh(walsh_code, bearer.as_ref());
            });
            bts_client.deallocate_traffic(walsh_code).await;
            bts_client.drop_pending_rx_request(walsh_code).await;
            bts_client.request_rx_removal(walsh_code).await;
        }
        true
    }

    /// Set up an SO6 traffic channel so the BSC can re-deliver an oversized
    /// SMS on F-DSCH instead of giving up. Returns `true` when escalation
    /// took over the outstanding ack; `false` means the caller's generic
    /// failure path should run.
    pub(crate) async fn try_escalate_oversized_sms_to_so6(&mut self, correlation_id: u32) -> bool {
        use super::sms::SmsAckKey;
        let key = SmsAckKey::PchCorrelation(correlation_id);
        let pos = match self
            .sms
            .pending_acks
            .iter()
            .position(|p| p.key == key && p.escalation.is_some())
        {
            Some(p) => p,
            None => {
                log::debug!(
                    "BSC: oversize escalation: no escalatable pending SMS for correlation_id={}",
                    correlation_id
                );
                return false;
            }
        };
        let fwd_address = self.sms.pending_acks[pos].addr.clone();

        let Some(ms) = self.mobiles.get(&fwd_address) else {
            log::warn!(
                "BSC: oversize escalation: mobile {} no longer registered",
                format_ms_address(&fwd_address)
            );
            return false;
        };
        if !matches!(
            ms.state,
            MsState::PageResponseReceived | MsState::Registered | MsState::Paged
        ) {
            log::warn!(
                "BSC: oversize escalation: mobile {} not in a state ready for traffic assignment (state={:?})",
                format_ms_address(&fwd_address),
                ms.state
            );
            return false;
        }

        let Some(bts_client) = self.config.bts_client.clone() else {
            log::warn!("BSC: oversize escalation: no BTS client configured");
            return false;
        };
        let esn = ms.esn.unwrap_or(0);
        let traffic_lc = LongCodeGenerator::new_traffic_channel(esn);

        // Pick the best RC pair the MS and config agree on (Page Response
        // already populated the MS capability fields). RC3/RC3 when the
        // negotiator prefers it; otherwise fall back to RC1/RC1.
        let Some(selected_rcs) = select_initial_traffic_rcs(
            &self.config.traffic_assignment,
            &ms.for_supported_rcs,
            &ms.rev_supported_rcs,
            ms.for_preferred_rc,
            ms.rev_preferred_rc,
            ms.mob_p_rev,
        ) else {
            log::warn!(
                "BSC: oversize escalation: no traffic RC pair matches mobile {} capabilities",
                format_ms_address(&fwd_address)
            );
            return false;
        };
        let use_rc3 = selected_rcs == (3, 3);

        let alloc_result = if use_rc3 {
            bts_client
                .allocate_rc3_traffic(traffic_lc.clone(), 0, 12, esn, false)
                .await
        } else {
            bts_client
                .allocate_rc1_traffic(traffic_lc.clone(), 0, esn)
                .await
        };
        let Some(handle) = alloc_result else {
            log::warn!(
                "BSC: oversize escalation: no walsh codes available for SO6 traffic, falling through to fail"
            );
            return false;
        };
        let walsh_code = handle.walsh_code;
        info!(
            "BSC: oversize escalation: allocated {} traffic walsh={} for {} (SO6 SMS re-delivery)",
            if use_rc3 { "RC3" } else { "RC1" },
            walsh_code,
            format_ms_address(&fwd_address)
        );

        let (walsh_code, assigned_rcs) = self.assign_traffic_channel_to_mobile(
            &fwd_address,
            handle,
            SERVICE_OPTION_SMS,
            Some(SERVICE_OPTION_SMS),
            1,
            None,
            None,
            None,
        );

        let ms_gating = self
            .mobiles
            .get(&fwd_address)
            .map(|ms| ms.rev_fch_gating_req)
            .unwrap_or(false);
        bts_client
            .install_rx_request(cdma_common::traffic::TrafficRxRequest {
                walsh_code,
                esn,
                assigned_rev_rc: assigned_rcs.1,
                preamble_num_pcgs: None,
                rev_fch_gating_mode: ms_gating && (3..=6).contains(&assigned_rcs.1),
            })
            .await;

        // ack_msg_seq is just for logging; BTS stamps real ARQ values per
        // address. We don't have an access event in this MT path, so pass 0.
        if let Err(e) = self.traffic_assignment.send_channel_assignment(
            &self.mobiles,
            &self.access_tx,
            self.config.pilot_offset,
            &self.config.overhead,
            &self.config.traffic_assignment,
            &fwd_address,
            0,
            walsh_code,
            Some(assigned_rcs),
            None,
            None,
        ) {
            log::warn!(
                "BSC: oversize escalation: failed to send Channel Assignment for {}: {}",
                format_ms_address(&fwd_address),
                e
            );
            let bearer = self.config.msc_voice_bearer.clone();
            self.mobiles.update(&fwd_address, |ms| {
                ms.set_state(MsState::Registered);
                ms.remove_traffic_channel_by_walsh(walsh_code, bearer.as_ref());
            });
            bts_client.deallocate_traffic(walsh_code).await;
            bts_client.drop_pending_rx_request(walsh_code).await;
            bts_client.request_rx_removal(walsh_code).await;
            return false;
        }

        // Move pending ack from PchCorrelation keying to walsh-keyed escalation
        // queue. On Service Connect Completion the BSC pops it and re-sends
        // the bytes on F-DSCH; `track_pending_traffic_ack` then re-installs
        // the F-TCH ack tracker with the real msg_seq.
        let pending = self.sms.pending_acks.remove(pos);
        let sms_id = pending.sms_id;
        self.pending_sms_escalations.insert(walsh_code, pending);
        log::info!(
            "BSC: oversize escalation: SMS {:?} parked on walsh={} pending Service Connect Completion",
            sms_id,
            walsh_code
        );
        true
    }

    pub(crate) async fn try_assign_access_packet_data_traffic(
        &mut self,
        fwd_address: &MsAddress,
        event: &AccessChannelEvent,
        last_msg_seq: u8,
        a1_call_id: Option<u64>,
    ) -> bool {
        let digits = self
            .decoded_origination(event)
            .map(|msg| self.format_origination_digits(msg))
            .unwrap_or_default();
        let packet_sr_id = self
            .decoded_origination(event)
            .and_then(|msg| msg.sr_id)
            .unwrap_or(1);
        let so = if digits == "#777" || digits == "777" {
            info!("BSC: #777 dialed, defaulting packet data to SO33");
            SERVICE_OPTION_HIGH_RATE_PACKET_DATA
        } else {
            event.service_option.unwrap_or(0)
        };
        if !is_packet_data_so(so) {
            return false;
        }

        let bts_client = self.config.bts_client.clone();
        let ack_deadline = self.access_ack_deadline(event);

        let Some(ms) = self.mobiles.get(fwd_address) else {
            return false;
        };
        let esn = ms.esn.unwrap_or(0);
        let traffic_lc = LongCodeGenerator::new_traffic_channel(esn);

        let stale_pending_packet = ms
            .pending_packet_traffic_assignment()
            .map(|tc| (tc.walsh_code, tc.rc_label));
        if let Some((walsh_code, rc_label)) = stale_pending_packet {
            info!(
                "BSC: tearing down stale pending {} packet-data channel walsh={} for {} before retry",
                rc_label,
                walsh_code,
                format_ms_address(fwd_address)
            );
            self.teardown_traffic_channel(walsh_code).await;
        }

        let Some(bts_client) = bts_client.as_ref() else {
            log::warn!("BSC: traffic channels not configured, falling back to BS Ack");
            return false;
        };

        let selected_rcs = {
            let ms = self.mobiles.get(fwd_address).expect("mobile exists");
            select_initial_traffic_rcs(
                &self.config.traffic_assignment,
                &ms.for_supported_rcs,
                &ms.rev_supported_rcs,
                ms.for_preferred_rc,
                ms.rev_preferred_rc,
                ms.mob_p_rev,
            )
        };
        let Some(selected_rcs) = selected_rcs else {
            let ms = self.mobiles.get(fwd_address).expect("mobile exists");
            log::warn!(
                "BSC: no configured traffic RC pair matches mobile {} capabilities for packet data for_rcs={:?} rev_rcs={:?} prefs=({:?},{:?}); falling back to BS Ack",
                format_ms_address(fwd_address),
                ms.for_supported_rcs,
                ms.rev_supported_rcs,
                ms.for_preferred_rc,
                ms.rev_preferred_rc,
            );
            return false;
        };
        let use_rc3 = selected_rcs == (3, 3);
        // Keep traffic setup FCH-only. F-SCH is allocated separately through the
        // rate-aware Abis Burst Request path after the packet service is up.
        let include_sch = false;
        let alloc_result = if use_rc3 {
            bts_client
                .allocate_rc3_traffic(traffic_lc.clone(), 0, 12, esn, include_sch)
                .await
        } else {
            bts_client
                .allocate_rc1_traffic(traffic_lc.clone(), 0, esn)
                .await
        };

        let Some(handle) = alloc_result else {
            log::warn!(
                "BSC: no Walsh codes available for packet-data traffic channel, falling back to BS Ack"
            );
            return false;
        };

        let walsh_code = handle.walsh_code;
        if let Some(sch_code) = handle.sch_walsh_code {
            info!(
                "BSC: allocated setup-time F-SCH code {} alongside walsh={} for {} (SO{} packet data)",
                sch_code,
                walsh_code,
                format_ms_address(fwd_address),
                so
            );
        }
        info!(
            "BSC: allocated {} traffic channel walsh={} for {} (SO{} packet data)",
            if use_rc3 { "RC3" } else { "RC1" },
            walsh_code,
            format_ms_address(fwd_address),
            so
        );

        let old_walsh = self
            .mobiles
            .get(fwd_address)
            .and_then(|ms| ms.current_traffic_walsh());
        if let Some(old_walsh) = old_walsh {
            info!(
                "BSC: tearing down existing traffic channel walsh={} before packet data assignment",
                old_walsh
            );
            self.teardown_traffic_channel(old_walsh).await;
        }

        let (walsh_code, assigned_rcs) = self.assign_traffic_channel_to_mobile(
            fwd_address,
            handle,
            so,
            event.service_option,
            packet_sr_id,
            None,
            None,
            a1_call_id,
        );

        let ms_gating = self
            .mobiles
            .get(fwd_address)
            .map(|ms| ms.rev_fch_gating_req)
            .unwrap_or(false);
        bts_client
            .install_rx_request(TrafficRxRequest {
                walsh_code,
                esn,
                assigned_rev_rc: assigned_rcs.1,
                preamble_num_pcgs: None,
                rev_fch_gating_mode: ms_gating && (3..=6).contains(&assigned_rcs.1),
            })
            .await;
        info!(
            "BSC: requested reverse traffic RX for packet-data walsh={}",
            walsh_code
        );

        if let Err(e) = self.traffic_assignment.send_channel_assignment(
            &self.mobiles,
            &self.access_tx,
            self.config.pilot_offset,
            &self.config.overhead,
            &self.config.traffic_assignment,
            fwd_address,
            last_msg_seq,
            walsh_code,
            Some(assigned_rcs),
            super::access::access_response_tx_time(event),
            ack_deadline,
        ) {
            log::warn!(
                "BSC: failed to send packet-data Channel Assignment for {}: {}",
                format_ms_address(fwd_address),
                e
            );
            let bearer = self.config.msc_voice_bearer.clone();
            self.mobiles.update(fwd_address, |ms| {
                ms.set_state(MsState::Registered);
                ms.remove_traffic_channel_by_walsh(walsh_code, bearer.as_ref());
            });
            bts_client.deallocate_traffic(walsh_code).await;
            bts_client.drop_pending_rx_request(walsh_code).await;
            bts_client.request_rx_removal(walsh_code).await;
        }
        true
    }

    /// Allocate a forward traffic channel (RC1 or RC3 per the mobile's
    /// supported RC sets), install the matching reverse-traffic RX request,
    /// send the Channel Assignment / Extended Channel Assignment, and roll
    /// back the resources on failure.
    pub(crate) async fn allocate_voice_channel_for_mobile(
        &mut self,
        fwd_address: &MsAddress,
        service_option: u16,
        origination_service_option: Option<u16>,
        service_ref_id: u8,
        ack_msg_seq: u8,
        requested_tx_time: Option<cdma_common::time::CdmaSystemTime>,
        tx_deadline: Option<cdma_common::time::CdmaSystemTime>,
        session_id: Option<Uuid>,
        leg_role: Option<VoiceLegRole>,
        a1_call_id: Option<u64>,
    ) -> Result<(), Error> {
        let bts_client = self.config.bts_client.clone();
        let Some(esn) = self.mobiles.get(fwd_address).map(|ms| ms.esn.unwrap_or(0)) else {
            return Err("mobile no longer registered".into());
        };
        let traffic_lc = LongCodeGenerator::new_traffic_channel(esn);

        let old_walsh_and_voice = self.mobiles.get(fwd_address).and_then(|ms| {
            ms.current_traffic_walsh()
                .map(|w| (w, ms.traffic_voice_context()))
        });
        if let Some((old_walsh, voice_ctx)) = old_walsh_and_voice {
            info!(
                "BSC: tearing down existing traffic channel walsh={} before new voice allocation",
                old_walsh
            );
            let (old_session, old_leg) = voice_ctx.unwrap_or((None, None));
            self.teardown_traffic_channel(old_walsh).await;
            self.on_voice_leg_released(old_session, old_leg);
        }

        let Some(bts_client) = bts_client.as_ref() else {
            return Err("bts_client not configured".into());
        };
        let selected_rcs = {
            let ms = self
                .mobiles
                .get(fwd_address)
                .ok_or_else(|| "mobile no longer registered".to_string())?;
            select_initial_traffic_rcs(
                &self.config.traffic_assignment,
                &ms.for_supported_rcs,
                &ms.rev_supported_rcs,
                ms.for_preferred_rc,
                ms.rev_preferred_rc,
                ms.mob_p_rev,
            )
        }
        .ok_or_else(|| "no configured traffic RC pair matches mobile capabilities".to_string())?;
        let use_rc3 = selected_rcs == (3, 3);
        // F-SCH is only for SO33 packet data.
        let alloc_result = if use_rc3 {
            bts_client
                .allocate_rc3_traffic(traffic_lc.clone(), 0, 12, esn, false)
                .await
        } else {
            bts_client
                .allocate_rc1_traffic(traffic_lc.clone(), 0, esn)
                .await
        };
        let handle = alloc_result
            .ok_or_else(|| "no Walsh codes available for voice traffic channel".to_string())?;
        let (walsh_code, assigned_rcs) = self.assign_traffic_channel_to_mobile(
            fwd_address,
            handle,
            service_option,
            origination_service_option,
            service_ref_id,
            session_id,
            leg_role,
            a1_call_id,
        );

        let ms_gating = self
            .mobiles
            .get(fwd_address)
            .map(|ms| ms.rev_fch_gating_req)
            .unwrap_or(false);
        bts_client
            .install_rx_request(TrafficRxRequest {
                walsh_code,
                esn,
                assigned_rev_rc: assigned_rcs.1,
                preamble_num_pcgs: None,
                rev_fch_gating_mode: ms_gating && (3..=6).contains(&assigned_rcs.1),
            })
            .await;

        if let Err(e) = self.traffic_assignment.send_channel_assignment(
            &self.mobiles,
            &self.access_tx,
            self.config.pilot_offset,
            &self.config.overhead,
            &self.config.traffic_assignment,
            fwd_address,
            ack_msg_seq,
            walsh_code,
            Some(assigned_rcs),
            requested_tx_time,
            tx_deadline,
        ) {
            let bearer = self.config.msc_voice_bearer.clone();
            self.mobiles.update(fwd_address, |ms| {
                ms.set_state(MsState::Registered);
                ms.remove_traffic_channel_by_walsh(walsh_code, bearer.as_ref());
            });
            bts_client.deallocate_traffic(walsh_code).await;
            bts_client.drop_pending_rx_request(walsh_code).await;
            bts_client.request_rx_removal(walsh_code).await;
            return Err(e);
        }

        if let Some(session_id) = session_id {
            if let Some(session) = self.voice.session_mut(session_id) {
                let party = if leg_role == Some(VoiceLegRole::Caller) {
                    session.caller.as_mut()
                } else {
                    session.callee.as_mut()
                };
                if let Some(party) = party {
                    party.walsh_code = Some(walsh_code);
                }
            }
        }

        Ok(())
    }
}

impl TrafficAssignmentService {
    /// Build and send a Channel Assignment Message (legacy mobiles) or
    /// Extended Channel Assignment Message (IS-2000 mobiles, mob_p_rev >= 6)
    /// on the F-PCH.
    pub(crate) fn send_channel_assignment(
        &self,
        mobiles: &MobileRegistryService,
        access_tx: &super::AccessTx,
        pilot_offset: usize,
        overhead: &OverheadParameters,
        traffic_config: &crate::config::TrafficAssignmentConfig,
        addr: &MsAddress,
        ack_msg_seq: u8,
        walsh_code: u8,
        assigned_rcs: Option<(u8, u8)>,
        _requested_tx_time: Option<cdma_common::time::CdmaSystemTime>,
        _tx_deadline: Option<cdma_common::time::CdmaSystemTime>,
    ) -> Result<(), Error> {
        let (mob_p_rev, for_rcs, rev_rcs, for_pref, rev_pref, ms_gating_req) = mobiles
            .get(addr)
            .map(|ms| {
                (
                    ms.mob_p_rev,
                    ms.for_supported_rcs.clone(),
                    ms.rev_supported_rcs.clone(),
                    ms.for_preferred_rc,
                    ms.rev_preferred_rc,
                    ms.rev_fch_gating_req,
                )
            })
            .unwrap_or((1, Vec::new(), Vec::new(), None, None, false));

        let (message, msg_id, assign_mode, code_chan) = if mob_p_rev >= 6 {
            let (for_rc, rev_rc) = match assigned_rcs.or_else(|| {
                select_initial_traffic_rcs(
                    traffic_config,
                    &for_rcs,
                    &rev_rcs,
                    for_pref,
                    rev_pref,
                    mob_p_rev,
                )
            }) {
                Some(pair) => pair,
                None => {
                    return Err(format!(
                        "no configured traffic RC pair matches mobile capabilities for_rcs={:?} rev_rcs={:?} prefs=({:?},{:?})",
                        for_rcs, rev_rcs, for_pref, rev_pref
                    )
                    .into());
                }
            };
            let early_rl = false;
            info!(
                "BSC: IS-2000 mobile (mob_p_rev={}), using ECAM for_rc={} rev_rc={} early_rl={} serv_neg={} (for_rcs={:?}, rev_rcs={:?})",
                mob_p_rev,
                for_rc,
                rev_rc,
                early_rl as u8,
                ServiceNegotiationMode::ServiceNegotiation.label(),
                for_rcs,
                rev_rcs
            );
            let mut ecam = ExtendedChannelAssignmentMessage::new_f_fch_r_fch_assignment(
                pilot_offset as u16,
                walsh_code,
                0,
                for_rc,
                rev_rc,
                early_rl,
            );
            ecam.rev_fch_gating_mode = ms_gating_req && (3..=6).contains(&rev_rc);
            ecam.freq_incl = true;
            ecam.band_class = overhead.band_class;
            ecam.cdma_freq = overhead.cdma_freq;
            info!(
                "BSC: ECAM detail walsh={} {} sdu_hex={}",
                walsh_code,
                ecam.describe(),
                ecam.encoded_sdu_hex()
            );
            (
                PagingChannelMessage::ExtendedChannelAssignment(ecam),
                MessageId::ExtChannelAssignment,
                0b100,
                walsh_code,
            )
        } else {
            let (for_rc, rev_rc) = assigned_rcs
                .or_else(|| {
                    select_initial_traffic_rcs(
                        traffic_config,
                        &for_rcs,
                        &rev_rcs,
                        for_pref,
                        rev_pref,
                        mob_p_rev,
                    )
                })
                .ok_or_else(|| {
                    "no configured RC pair available for legacy CAM assignment".to_string()
                })?;
            if (for_rc, rev_rc) != (1, 1) {
                return Err(format!(
                    "legacy CAM ASSIGN_MODE=000 only supports implemented RC1/RC1 path, selected RC{}/{}",
                    for_rc, rev_rc
                )
                .into());
            }
            info!(
                "BSC: legacy mobile (mob_p_rev={}), using CAM ASSIGN_MODE=000 serv_neg={}",
                mob_p_rev,
                ServiceNegotiationMode::ServiceOptionNegotiation.label()
            );
            let cam = ChannelAssignmentMessage::new_traffic_assignment(walsh_code, 0);
            (
                PagingChannelMessage::ChannelAssignment(cam),
                MessageId::ChannelAssignment,
                0b000,
                walsh_code,
            )
        };

        let sdu = message.to_sdu();

        access_tx.send_directed_fpch(addr, msg_id, message, sdu, true)?;

        info!(
            "BSC: sending {} (assign_mode=0b{:03b}, walsh={}, ack_seq={})",
            if msg_id == MessageId::ExtChannelAssignment {
                "Extended Channel Assignment Message"
            } else {
                "Channel Assignment Message"
            },
            assign_mode,
            code_chan,
            ack_msg_seq,
        );
        Ok(())
    }
}

impl Bsc {
    pub(super) fn assign_traffic_channel_to_mobile(
        &mut self,
        fwd_address: &MsAddress,
        handle: BtsTrafficChannelHandle,
        service_option: u16,
        origination_service_option: Option<u16>,
        service_ref_id: u8,
        session_id: Option<Uuid>,
        leg_role: Option<VoiceLegRole>,
        a1_call_id: Option<u64>,
    ) -> (u8, (u8, u8)) {
        let service_negotiation_mode = self
            .mobiles
            .get(fwd_address)
            .map(|ms| {
                TrafficAssignmentService::service_negotiation_mode_for_mob_p_rev(ms.mob_p_rev)
            })
            .unwrap_or(ServiceNegotiationMode::ServiceOptionNegotiation);
        self.traffic_assignment.assign_channel_to_mobile(
            &mut self.mobiles,
            &mut self.voice,
            &self.config.overhead,
            fwd_address,
            handle,
            service_option,
            origination_service_option,
            service_ref_id,
            service_negotiation_mode,
            vec![self.config.pilot_offset as u16],
            session_id,
            leg_role,
            a1_call_id,
        )
    }

    /// Apply an operator-issued reverse-power target override (or
    /// clear) to the traffic channel matching `walsh_code` and reply on
    /// the request's response channel.
    pub(crate) fn handle_traffic_power_override_request(
        &mut self,
        req: TrafficPowerOverrideRequest,
    ) {
        let TrafficPowerOverrideRequest {
            walsh_code,
            action,
            response_tx,
        } = req;
        let mut response = Err(format!(
            "active traffic channel walsh={} not found",
            walsh_code
        ));
        let mut should_publish = false;

        if let Some(snapshot) = self.mobiles.update_tc(walsh_code, |_, tc| {
            match action {
                TrafficPowerOverrideAction::SetTargetEbNtDb(requested_db) => {
                    let applied_db = tc.power_control.set_manual_target_override_db(requested_db);
                    info!(
                        "BSC: set reverse power target override walsh={} SO={} requested={:.2} dB applied={:.2} dB auto_target={:.2} dB",
                        walsh_code,
                        tc.service_option,
                        requested_db,
                        applied_db,
                        tc.power_control.target_eb_nt_db,
                    );
                }
                TrafficPowerOverrideAction::Clear => {
                    let cleared = tc.power_control.clear_manual_target_override_db();
                    info!(
                        "BSC: cleared reverse power target override walsh={} SO={} resumed_auto_target={:.2} dB had_override={}",
                        walsh_code,
                        tc.service_option,
                        tc.power_control.target_eb_nt_db,
                        cleared.is_some(),
                    );
                }
            }
            traffic_channel_power_snapshot(tc)
        }) {
            response = Ok(snapshot);
            should_publish = true;
        }

        if should_publish {
            self.publish_mobiles();
        }

        let _ = response_tx.send(response);
    }
}
