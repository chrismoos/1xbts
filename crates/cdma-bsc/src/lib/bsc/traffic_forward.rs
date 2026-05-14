//! Forward traffic-channel signaling builders and bearer enqueue.

use cdma_bts::lac as bts_lac;
use cdma_common::channel::TrafficRate;
use cdma_common::error::Error;
use cdma_common::lac::{
    MessageControlStatusBlock,
    message_types::MessageId,
    paging_messages::{
        AlertWithInformationMessage, EscamParams, ForSchConfig, ForwardDataBurstMessage, MsAddress,
        NonNegServiceConfig, OrderMessage, ServiceConnectCallAssignment,
        ServiceConnectConnectionRecord, ServiceConnectParams, ServiceRequestConfig,
        ServiceRequestParams,
    },
};
use cdma_common::mac::ChannelType;
use cdma_voice::VoiceCodec;
use log::info;
use uuid::Uuid;

use crate::abis_edge::ForwardBearerQueue;
use crate::addressing::{format_ms_address, is_packet_data_so};
use cdma_common::sch::Rc3FschProfile;

use super::traffic_bearer::send_forward_fch_bits_with_bearer_client;
use super::{
    Bsc, MsState, VOICE_REPLACEMENT_CON_REF, VOICE_TRAFFIC_CON_REF, VOICE_TRAFFIC_SR_ID,
    VoiceLegRole,
};

const FSCH_ESCAM_START_DELAY_FRAMES: u64 = 12;

pub(crate) fn fsch_escam_start_time_mod32() -> u8 {
    let start = cdma_common::time::system_time_now()
        + chrono::Duration::milliseconds((FSCH_ESCAM_START_DELAY_FRAMES * 20) as i64);
    (cdma_common::time::system_time_20ms_frames(start) % 32) as u8
}

/// Result of `send_forward_signaling_paging_or_traffic`.
pub(crate) enum ForwardSignalingRoute {
    /// Message was sent on the forward dedicated signaling channel (F-DSCH)
    /// because the MS has an active traffic channel.
    SentOnTraffic { msg_seq: u8 },
    /// MS does not have an active traffic channel; caller should initiate
    /// a page cycle and deliver on the common signaling channel (F-PCH).
    NeedsPaging,
}

pub(crate) fn voice_service_option_for_channel(tc: &super::TrafficChannelInfo) -> Option<u16> {
    tc.voice_service_option
        .or_else(|| VoiceCodec::from_service_option(tc.service_option).map(|_| tc.service_option))
}

impl Bsc {
    fn traffic_service_connections(
        &self,
        walsh_code: u8,
    ) -> Result<Vec<ServiceConnectConnectionRecord>, Error> {
        let tc = self
            .mobiles
            .get_traffic_channel(walsh_code)
            .ok_or("no traffic channel for service configuration")?;
        if let Some(voice_so) = tc.voice_service_option {
            if is_packet_data_so(tc.service_option) {
                return Ok(vec![ServiceConnectConnectionRecord {
                    con_ref: tc.voice_connection_ref.unwrap_or(VOICE_REPLACEMENT_CON_REF),
                    service_option: voice_so,
                    for_traffic: 1,
                    rev_traffic: 1,
                    ui_encrypt_mode: 0,
                    sr_id: tc.voice_service_ref_id.unwrap_or(VOICE_TRAFFIC_SR_ID),
                    rlp_info_incl: false,
                    rlp_blob: None,
                    qos_parms: None,
                }]);
            }
        }
        let mut connections = Vec::new();
        connections.push(ServiceConnectConnectionRecord {
            con_ref: 0,
            service_option: tc.service_option,
            for_traffic: 1,
            rev_traffic: 1,
            ui_encrypt_mode: 0,
            sr_id: tc.service_ref_id,
            rlp_info_incl: false,
            rlp_blob: None,
            qos_parms: None,
        });

        if let Some(voice_so) = voice_service_option_for_channel(tc) {
            if voice_so != tc.service_option {
                connections.push(ServiceConnectConnectionRecord {
                    con_ref: tc.voice_connection_ref.unwrap_or(VOICE_TRAFFIC_CON_REF),
                    service_option: voice_so,
                    for_traffic: 1,
                    rev_traffic: 1,
                    ui_encrypt_mode: 0,
                    sr_id: tc.voice_service_ref_id.unwrap_or(VOICE_TRAFFIC_SR_ID),
                    rlp_info_incl: false,
                    rlp_blob: None,
                    qos_parms: None,
                });
            }
        }

        Ok(connections)
    }

    pub(super) fn start_mt_voice_on_existing_traffic(
        &mut self,
        fwd_address: &MsAddress,
        voice_service_option: u16,
        session_id: Uuid,
        leg_role: VoiceLegRole,
        a1_call_id: Option<u64>,
    ) -> Result<u8, Error> {
        let walsh_code = self
            .mobiles
            .get(fwd_address)
            .and_then(|ms| ms.current_traffic_walsh())
            .ok_or("mobile has no existing traffic channel")?;

        let setup_result = self.mobiles.update_tc(walsh_code, |_, tc| {
            if tc.is_releasing() {
                return Err::<(), Error>("traffic channel is releasing".into());
            }
            tc.voice_service_option = Some(voice_service_option);
            tc.voice_connection_ref = Some(if is_packet_data_so(tc.service_option) {
                VOICE_REPLACEMENT_CON_REF
            } else {
                VOICE_TRAFFIC_CON_REF
            });
            tc.voice_service_ref_id = Some(VOICE_TRAFFIC_SR_ID);
            tc.voice_session_id = Some(session_id);
            tc.voice_leg_role = Some(leg_role);
            tc.a1_call_id = a1_call_id;
            Ok(())
        });
        match setup_result {
            Some(Ok(())) => {}
            Some(Err(e)) => return Err(e),
            None => return Err("mobile has no existing traffic channel".into()),
        }

        self.send_service_request(walsh_code, 0b111)?;
        self.mobiles.update_tc(walsh_code, |_, tc| {
            tc.mark_waiting_service_response();
        });
        Ok(walsh_code)
    }

    /// Route a forward Data Burst to the appropriate signaling channel.
    ///
    /// If the addressed MS has an active traffic channel, the message is sent
    /// immediately on the F-DSCH and `SentOnTraffic` is returned. Otherwise
    /// `NeedsPaging` is returned so the caller can initiate a page cycle and
    /// deliver on the common signaling channel later.
    pub(crate) fn send_forward_signaling_paging_or_traffic(
        &mut self,
        fwd_address: &MsAddress,
        data_burst: ForwardDataBurstMessage,
    ) -> Result<ForwardSignalingRoute, Error> {
        let Some(ms) = self.mobiles.get(fwd_address) else {
            return Ok(ForwardSignalingRoute::NeedsPaging);
        };
        if ms.state != MsState::TrafficActive {
            return Ok(ForwardSignalingRoute::NeedsPaging);
        }
        let walsh_code = match ms.current_traffic_walsh() {
            Some(walsh_code) => walsh_code,
            None => return Ok(ForwardSignalingRoute::NeedsPaging),
        };

        let msg_seq = self
            .mobiles
            .get_traffic_channel(walsh_code)
            .map(|tc| tc.forward_msg_seq_ack)
            .unwrap_or(0);

        let sdu = data_burst.to_sdu();
        self.send_traffic_signaling(
            walsh_code,
            sdu,
            MessageId::DataBurst,
            0,
            true,
            None,
            None,
            None,
            Some(data_burst),
            None,
        )?;

        Ok(ForwardSignalingRoute::SentOnTraffic { msg_seq })
    }

    /// Decide whether to negotiate F-SCH for an SO33 packet-data call.
    ///
    /// Uses an implicit gate: enabled config, SO33, RC3, MOB_P_REV >= 6, and
    /// RC3 mobile capability. Returns `None` for FCH-only Service Connect.
    pub(crate) fn fsch_for_service_connect(&self, walsh_code: u8) -> Option<ForSchConfig> {
        let tc = self.mobiles.get_traffic_channel(walsh_code)?;
        let ms = self.mobiles.get_by_walsh(walsh_code)?;
        if !crate::addressing::ms_eligible_for_fsch_phase1(
            self.config.traffic_assignment.enable_f_sch,
            tc.service_option,
            tc.for_rc,
            ms.mob_p_rev,
            ms.for_preferred_rc,
            &ms.for_supported_rcs,
        ) {
            return None;
        }
        let profile = Rc3FschProfile::from_rate_bps(self.config.traffic_assignment.f_sch_rate_bps)
            .unwrap_or_else(Rc3FschProfile::default_19k2);
        Some(ForSchConfig {
            sch_id: 0,
            mux_option: profile.mux_option,
            rc: 3,
            coding: 0, // Convolutional.
            rate: profile.num_bits_idx,
        })
    }

    /// Send ESCAM on F-FCH to activate the configured F-SCH profile.
    pub(crate) fn send_escam_for_fsch(
        &mut self,
        walsh_code: u8,
        sch_code: u8,
        profile: Rc3FschProfile,
    ) -> Result<(), Error> {
        let for_sch_start_time = fsch_escam_start_time_mod32();
        let params = EscamParams {
            start_time_unit: 0,
            for_sch_id: 0,
            sccl_index: 0,
            for_sch_num_bits_idx: profile.num_bits_idx,
            pilot_pn: self.config.pilot_offset as u16, // PN offset index, already in 64-chip units.
            code_chan_sch: sch_code as u16,
            qof_mask_id_sch: 0,
            for_sch_duration: 0x0F, // 0xF = infinite (until next ESCAM).
            for_sch_start_time_incl: true,
            for_sch_start_time,
            // Service Connect already carries baseline forward power-control
            // config. Keep ESCAM to the SCH assignment fields so handsets do
            // not reject optional SCH FPC values while enabling higher rates.
            fpc_incl: false,
            fpc_mode_sch: 0,
            fpc_sch_init_setpt_op: 0,
            fpc_sch_fer: 0b00010,     // 1% target FER.
            fpc_sch_init_setpt: 0x30, // 6.0 dB.
            fpc_sch_min_setpt: 0x00,
            fpc_sch_max_setpt: 0x50, // 10.0 dB.
        };
        let sdu = params.to_ftch_sdu();
        let ack_seq = self
            .mobiles
            .get_traffic_channel(walsh_code)
            .map(|tc| tc.forward_msg_seq_ack & 0x07)
            .unwrap_or(0);
        self.send_traffic_signaling(
            walsh_code,
            sdu,
            MessageId::ExtendedSupplementalChannelAssignment,
            ack_seq,
            true,
            None,
            None,
            None,
            None,
            None,
        )
    }

    /// Send an ESCAM that releases the active F-SCH assignment.
    pub(crate) fn send_escam_release_for_fsch(
        &mut self,
        walsh_code: u8,
        sch_code: u8,
        profile: Rc3FschProfile,
    ) -> Result<(), Error> {
        let params = EscamParams {
            start_time_unit: 0,
            for_sch_id: 0,
            sccl_index: 0,
            for_sch_num_bits_idx: profile.num_bits_idx,
            pilot_pn: self.config.pilot_offset as u16,
            code_chan_sch: sch_code as u16,
            qof_mask_id_sch: 0,
            for_sch_duration: 0,
            for_sch_start_time_incl: false,
            for_sch_start_time: 0,
            fpc_incl: false,
            fpc_mode_sch: 0,
            fpc_sch_init_setpt_op: 0,
            fpc_sch_fer: 0b00010,
            fpc_sch_init_setpt: 0x30,
            fpc_sch_min_setpt: 0x00,
            fpc_sch_max_setpt: 0x50,
        };
        let sdu = params.to_ftch_sdu();
        let ack_seq = self
            .mobiles
            .get_traffic_channel(walsh_code)
            .map(|tc| tc.forward_msg_seq_ack & 0x07)
            .unwrap_or(0);
        self.send_traffic_signaling(
            walsh_code,
            sdu,
            MessageId::ExtendedSupplementalChannelAssignment,
            ack_seq,
            true,
            None,
            None,
            None,
            None,
            None,
        )
    }

    /// Send a Service Connect Message on the forward traffic channel.
    ///
    /// Negotiates the service option (SO6 for SMS) with the mobile.
    /// Per IS-2000 C.S0004-E 3.7.2.3.2.21, the message contains a Service
    /// Configuration record (type 0x07) describing mux options, radio
    /// configurations, and connection records, plus an optional Non-Negotiable
    /// Service Configuration record (type 0x13) for power control parameters.
    pub(crate) fn send_service_connect(
        &mut self,
        walsh_code: u8,
        ack_seq: u8,
    ) -> Result<(), Error> {
        let tc = self
            .mobiles
            .get_traffic_channel(walsh_code)
            .ok_or("no traffic channel for Service Connect")?;
        let (for_rc, rev_rc) = (tc.for_rc, tc.rev_rc);
        let serv_con_seq = self.traffic_signaling.next_serv_con_seq();
        let connections = self.traffic_service_connections(walsh_code)?;
        let call_assignments = if let Some(voice_so) = voice_service_option_for_channel(tc) {
            if voice_so != tc.service_option {
                vec![ServiceConnectCallAssignment {
                    con_ref: tc.voice_connection_ref.unwrap_or(VOICE_TRAFFIC_CON_REF),
                    response_ind: false,
                    tag: None,
                    bypass_alert_answer: Some(false),
                }]
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        };

        let mux_option: u16 = 0x0001;

        let for_sch_config = self.fsch_for_service_connect(walsh_code);
        let non_neg = Some(if for_rc >= 3 {
            if for_sch_config.is_some() {
                NonNegServiceConfig::rc3_fsch_default()
            } else {
                NonNegServiceConfig::rc3_default()
            }
        } else {
            NonNegServiceConfig::rc1_default()
        });

        let params = ServiceConnectParams {
            serv_con_seq,
            use_old_serv_config: 0,
            for_mux_option: mux_option,
            rev_mux_option: mux_option,
            for_rates: 0xF0,
            rev_rates: 0xF0,
            sync_id: None,
            connections,
            fch_frame_size: 0,
            for_fch_rc: for_rc,
            rev_fch_rc: rev_rc,
            call_assignments,
            use_type0_plcm: false,
            non_neg,
            for_sch_config,
        };

        let sdu = params.to_ftch_sdu();

        info!(
            "BSC: sending Service Connect on F-TCH walsh={} SO={} voice_so={:?} RC{}/{} serv_con_seq={}",
            walsh_code,
            tc.service_option,
            voice_service_option_for_channel(tc),
            for_rc,
            rev_rc,
            serv_con_seq
        );

        self.send_traffic_signaling(
            walsh_code,
            sdu,
            MessageId::ServiceConnect,
            ack_seq,
            true,
            None,
            Some(params),
            None,
            None,
            None,
        )
    }

    /// Send a Service Request Message on the forward traffic channel.
    ///
    /// Used to propose a new service configuration when the BSC's assigned SO
    /// differs from the mobile's origination SO.
    /// Per C.S0005-E 3.7.3.3.2.18: REQ_PURPOSE=0010 (propose).
    pub(crate) fn send_service_request(
        &mut self,
        walsh_code: u8,
        ack_seq: u8,
    ) -> Result<(), Error> {
        let tc = self
            .mobiles
            .get_traffic_channel(walsh_code)
            .ok_or("no traffic channel for Service Request")?;
        let (for_rc, rev_rc) = (tc.for_rc, tc.rev_rc);
        let service_option = tc.service_option;
        let voice_service_option = voice_service_option_for_channel(tc);

        let serv_req_seq = self.traffic_signaling.next_serv_con_seq();
        let connections = self.traffic_service_connections(walsh_code)?;

        let mux_option: u16 = 0x0001;

        let params = ServiceRequestParams {
            serv_req_seq,
            req_purpose: 0b0010,
            service_config: Some(ServiceRequestConfig {
                for_mux_option: mux_option,
                rev_mux_option: mux_option,
                for_rates: 0xF0,
                rev_rates: 0xF0,
                connections,
                fch_frame_size: 0,
                for_fch_rc: for_rc,
                rev_fch_rc: rev_rc,
            }),
        };

        let sdu = params.to_ftch_sdu();

        info!(
            "BSC: sending Service Request on F-TCH walsh={} SO={} voice_so={:?} RC{}/{} serv_req_seq={}",
            walsh_code, service_option, voice_service_option, for_rc, rev_rc, serv_req_seq
        );

        self.send_traffic_signaling(
            walsh_code,
            sdu,
            MessageId::ServiceRequest,
            ack_seq,
            true,
            None,
            None,
            Some(params),
            None,
            None,
        )
    }

    /// Send a BS Ack Order on the forward traffic channel.
    pub(crate) fn send_traffic_bs_ack(&mut self, walsh_code: u8, ack_seq: u8) -> Result<(), Error> {
        let order_msg = OrderMessage {
            order: 0b010000,
            ordq: 0,
            order_specific_fields: Vec::new(),
        };
        let sdu = order_msg.to_ftch_sdu();

        info!(
            "BSC: sending BS Ack Order on F-TCH walsh={} ack_seq={}",
            walsh_code, ack_seq
        );

        self.send_traffic_signaling(
            walsh_code,
            sdu,
            MessageId::Order,
            ack_seq,
            true,
            Some(order_msg),
            None,
            None,
            None,
            None,
        )
    }

    /// Send a signaling L3 SDU on the forward traffic channel via Abis bearer.
    pub(crate) fn send_traffic_signaling(
        &mut self,
        walsh_code: u8,
        sdu: cdma_common::bits::Bitstream,
        msg_id: MessageId,
        ack_seq: u8,
        ack_req: bool,
        order: Option<OrderMessage>,
        service_connect: Option<ServiceConnectParams>,
        service_request: Option<ServiceRequestParams>,
        data_burst: Option<ForwardDataBurstMessage>,
        alert_with_info: Option<AlertWithInformationMessage>,
    ) -> Result<(), Error> {
        let (addr, rc_label, tc_walsh, tc_for_rc, tc_so, tc_voice_label) = {
            let ms = self
                .mobiles
                .get_by_walsh(walsh_code)
                .ok_or("no traffic channel assigned")?;
            let tc = ms
                .find_traffic_channel_by_walsh(walsh_code)
                .ok_or("no traffic channel assigned")?;
            (
                ms.fwd_address.clone(),
                tc.rc_label,
                tc.walsh_code,
                tc.for_rc,
                tc.service_option,
                Some(tc.state_label().to_string()),
            )
        };

        let wire_msg_type = msg_id
            .wire_type(cdma_common::lac::message_types::WireChannel::ForwardDedicated)
            .ok_or("no forward dedicated wire type for message")?;

        let msg_seq = self
            .mobiles
            .update_tc(walsh_code, |_, tc| tc.next_forward_msg_seq(ack_req))
            .ok_or("no traffic channel assigned")?;

        let pdu = bts_lac::assemble_f_dsch_pdu(wire_msg_type, &sdu, ack_seq, msg_seq, ack_req);
        let mux_frames = bts_lac::sar_fragment_ftch_pdu_dsch(&pdu);

        info!(
            "BSC: send_traffic_signaling walsh={} msg_id={} msg_seq={} ack_seq={} ack_req={} sdu_bits={} mux_frames={} addr={} (via bearer FCH Fwd)",
            tc_walsh,
            msg_id.tag(),
            msg_seq,
            ack_seq,
            ack_req,
            sdu.len(),
            mux_frames.len(),
            format_ms_address(&addr),
        );

        for mux_frame in &mux_frames {
            let frame_bits = mux_frame.bits();
            let frame_bytes: Vec<u8> = frame_bits
                .chunks(8)
                .map(|chunk| {
                    let val = chunk.iter().fold(0u8, |acc, &b| (acc << 1) | b);
                    if chunk.len() < 8 {
                        val << (8 - chunk.len())
                    } else {
                        val
                    }
                })
                .collect();
            send_forward_fch_bits_with_bearer_client(
                self.config.bts_client.as_ref(),
                0,
                tc_walsh,
                tc_for_rc,
                frame_bytes,
                TrafficRate::Full,
                ForwardBearerQueue::Signaling,
            )?;
        }

        let esp = &self
            .config
            .paging
            .message_defaults
            .extended_system_parameters;
        let traffic_mcsb = MessageControlStatusBlock {
            channel: ChannelType::FTch,
            length_bits: sdu.len(),
            mobile_p_rev: None,
            extended_encryption: false,
            message_id: msg_id,
            requested_tx_time: None,
            tx_deadline: None,
            address: Some(addr.clone()),
            ack_seq,
            msg_seq,
            ack_req,
            valid_ack: true,
            overhead_mcc: esp.mcc,
            overhead_imsi_11_12: esp.imsi_11_12,
        };

        self.emit_traffic_tx_event(
            tc_walsh,
            tc_so,
            rc_label,
            0,
            traffic_mcsb,
            &addr,
            &sdu,
            &sdu,
            order,
            service_connect,
            service_request,
            data_burst,
            alert_with_info,
            tc_voice_label,
        );

        Ok(())
    }
}
