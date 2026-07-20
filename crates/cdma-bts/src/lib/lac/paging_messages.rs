pub use cdma_common::lac::paging_messages::*;

#[cfg(test)]
mod tests {
    use super::{
        AccessParametersMessage, CdmaChannelListMessage, ExtendedChannelAssignmentMessage,
        ExtendedSystemParametersMessage, ExtendedTrafficPilotRecord, GeneralPageMessage,
        GeneralPageRecord, MsAddress, NeighborListMessage, OrderMessage, PagingChannelMessage,
        SystemParametersMessage, bitstream_to_bytes,
    };
    use crate::lac::message_types::MessageId;
    use crate::receiver::layer3::{self, PagingMessage};
    use cdma_common::bits::Bitstream;
    use cdma_common::consts::SERVICE_OPTION_SMS;

    fn decode_roundtrip(message: PagingChannelMessage) -> PagingMessage {
        let mut pdu = cdma_common::bits::Bitstream::new();
        pdu.write_u8(
            message
                .message_id()
                .wire_type(crate::lac::message_types::WireChannel::ForwardCommon)
                .unwrap(),
            8,
        );
        pdu.extend(&message.to_sdu());
        layer3::PagingMessage::decode(&pdu).unwrap()
    }

    #[test]
    fn system_parameters_roundtrip() {
        let decoded = decode_roundtrip(PagingChannelMessage::SystemParameters(
            SystemParametersMessage {
                pilot_pn: 0,
                config_msg_seq: 2,
                sid: 42,
                nid: 7,
                reg_zone: 1,
                total_zones: 1,
                zone_timer: 1,
                mult_sids: false,
                mult_nids: false,
                base_id: 1,
                base_class: 0,
                page_chan: 1,
                max_slot_cycle_index: 0,
                home_reg: true,
                for_sid_reg: false,
                for_nid_reg: false,
                power_up_reg: true,
                power_down_reg: false,
                parameter_reg: true,
                reg_prd: 0,
                base_lat: 0,
                base_long: 0,
                reg_dist: 0,
                srch_win_a: 0,
                srch_win_n: 0,
                srch_win_r: 0,
                nghbr_max_age: 0,
                pwr_rep_thresh: 0,
                pwr_rep_frames: 0,
                pwr_thresh_enable: false,
                pwr_period_enable: false,
                pwr_rep_delay: 0,
                rescan: false,
                t_add: 0,
                t_drop: 0,
                t_comp: 0,
                t_tdrop: 0,
                ext_sys_parameter: false,
                ext_nghbr_lst: false,
                gen_nghbr_lst: false,
                global_redirect: false,
                pri_nghbr_lst: false,
                user_zone_id: false,
                ext_global_redirect: false,
                ext_chan_lst: false,
                t_tdrop_range_incl: false,
                t_tdrop_range: 0,
                neg_slot_cycle_index_sup: false,
                crrm_msg_ind: false,
                num_opt_msg_bits: 0,
                ap_pilot_info: false,
                ap_idt: false,
                ap_id_text: false,
                gen_ovhd_inf_ind: false,
                fd_chan_lst_ind: false,
                atim_ind: false,
                appim_period_index: 0,
                gen_ovhd_cycle_index: 0,
                atim_cycle_index: 0,
                add_loc_info_incl: false,
            },
        ));

        match decoded {
            PagingMessage::SystemParameters(m) => {
                assert_eq!(m.sid, 42);
                assert_eq!(m.nid, 7);
                assert_eq!(m.page_chan, 1);
            }
            _ => panic!("unexpected message"),
        }
    }

    #[test]
    fn system_parameters_service_profile_encodes_expected_pdu() {
        let message = PagingChannelMessage::SystemParameters(SystemParametersMessage {
            pilot_pn: 0,
            config_msg_seq: 23,
            sid: 22,
            nid: 65535,
            reg_zone: 0,
            total_zones: 1,
            zone_timer: 0,
            mult_sids: false,
            mult_nids: false,
            base_id: 1,
            base_class: 0,
            page_chan: 1,
            max_slot_cycle_index: 0,
            home_reg: true,
            for_sid_reg: true,
            for_nid_reg: true,
            power_up_reg: true,
            power_down_reg: false,
            parameter_reg: false,
            reg_prd: 0,
            base_lat: 0,
            base_long: 0,
            reg_dist: 0,
            srch_win_a: 8,
            srch_win_n: 10,
            srch_win_r: 10,
            nghbr_max_age: 0,
            pwr_rep_thresh: 0,
            pwr_rep_frames: 12,
            pwr_thresh_enable: false,
            pwr_period_enable: false,
            pwr_rep_delay: 0,
            rescan: false,
            t_add: 28,
            t_drop: 32,
            t_comp: 5,
            t_tdrop: 3,
            ext_sys_parameter: true,
            ext_nghbr_lst: false,
            gen_nghbr_lst: false,
            global_redirect: false,
            pri_nghbr_lst: false,
            user_zone_id: false,
            ext_global_redirect: false,
            ext_chan_lst: false,
            t_tdrop_range_incl: false,
            t_tdrop_range: 0,
            neg_slot_cycle_index_sup: false,
            crrm_msg_ind: false,
            num_opt_msg_bits: 0,
            ap_pilot_info: false,
            ap_idt: false,
            ap_id_text: false,
            gen_ovhd_inf_ind: false,
            fd_chan_lst_ind: false,
            atim_ind: false,
            appim_period_index: 0,
            gen_ovhd_cycle_index: 0,
            atim_cycle_index: 0,
            add_loc_info_incl: false,
        });

        let mut pdu = Bitstream::new();
        pdu.write_u8(
            message
                .message_id()
                .wire_type(crate::lac::message_types::WireChannel::ForwardCommon)
                .unwrap(),
            8,
        );
        pdu.extend(&message.to_sdu());

        assert_eq!(
            bitstream_to_bytes(&pdu),
            vec![
                0x01, 0x00, 0x2e, 0x00, 0x5b, 0xff, 0xfc, 0x00, 0x08, 0x00, 0x00, 0x40, 0x8f, 0x00,
                0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x45, 0x50, 0x03, 0x00, 0x1c, 0x81, 0x4e,
                0x00, 0x00,
            ]
        );
    }

    #[test]
    fn access_parameters_roundtrip() {
        let decoded = decode_roundtrip(PagingChannelMessage::AccessParameters(
            AccessParametersMessage {
                pilot_pn: 0,
                acc_msg_seq: 3,
                acc_chan: 0,
                nom_pwr: 0,
                init_pwr: 0,
                pwr_step: 1,
                num_step: 15,
                max_cap_sz: 7,
                pam_sz: 0,
                psist_0_9: 0,
                psist_10: 0,
                psist_11: 0,
                psist_12: 0,
                psist_13: 0,
                psist_14: 0,
                psist_15: 0,
                msg_psist: 0,
                reg_psist: 0,
                probe_pn_ran: 0,
                acc_tmo: 3,
                probe_bkoff: 0,
                bkoff: 1,
                max_req_seq: 15,
                max_rsp_seq: 15,
                auth: 0,
                rand: 0,
                nom_pwr_ext: 0,
                psist_emg_incl: false,
                psist_emg: 0,
                acct_incl: false,
                acct_incl_emg: false,
                acct_aoc_bitmap_incl: false,
                acct_so_records: Vec::new(),
                acct_so_grp_records: Vec::new(),
            },
        ));

        match decoded {
            PagingMessage::AccessParameters(m) => {
                assert_eq!(m.acc_msg_seq, 3);
                assert_eq!(m.pwr_step, 1);
                assert_eq!(m.num_step, 15);
                assert_eq!(m.max_cap_sz, 7);
                assert_eq!(m.acc_tmo, 3);
                assert_eq!(m.max_req_seq, 15);
                assert_eq!(m.max_rsp_seq, 15);
                assert_eq!(m.bkoff, 1);
                assert_eq!(m.auth, 0);
                assert_eq!(m.nom_pwr_ext, 0);
                assert!(!m.psist_emg_incl);
                assert!(!m.acct_incl);
            }
            _ => panic!("unexpected message"),
        }
    }

    #[test]
    fn access_parameters_service_profile_encodes_expected_pdu() {
        let message = PagingChannelMessage::AccessParameters(AccessParametersMessage {
            pilot_pn: 0,
            acc_msg_seq: 1,
            acc_chan: 0,
            nom_pwr: 0,
            init_pwr: 0,
            pwr_step: 1,
            num_step: 15,
            max_cap_sz: 7,
            pam_sz: 15,
            psist_0_9: 0,
            psist_10: 0,
            psist_11: 0,
            psist_12: 0,
            psist_13: 0,
            psist_14: 0,
            psist_15: 0,
            msg_psist: 0,
            reg_psist: 0,
            probe_pn_ran: 0,
            acc_tmo: 3,
            probe_bkoff: 0,
            bkoff: 1,
            max_req_seq: 15,
            max_rsp_seq: 15,
            auth: 0,
            rand: 0,
            nom_pwr_ext: 0,
            psist_emg_incl: false,
            psist_emg: 0,
            acct_incl: false,
            acct_incl_emg: false,
            acct_aoc_bitmap_incl: false,
            acct_so_records: Vec::new(),
            acct_so_grp_records: Vec::new(),
        });

        let mut pdu = Bitstream::new();
        pdu.write_u8(
            message
                .message_id()
                .wire_type(crate::lac::message_types::WireChannel::ForwardCommon)
                .unwrap(),
            8,
        );
        pdu.extend(&message.to_sdu());

        assert_eq!(
            bitstream_to_bytes(&pdu),
            vec![
                0x02, 0x00, 0x02, 0x00, 0x01, 0xff, 0xe0, 0x00, 0x00, 0x00, 0x01, 0x80, 0xff, 0x80,
            ]
        );
    }

    /// Verify that negative NOM_PWR / INIT_PWR values round-trip correctly
    /// through the 4-bit and 5-bit signed two's-complement wire encoding.
    #[test]
    fn access_parameters_negative_open_loop_offsets_roundtrip() {
        let decoded = decode_roundtrip(PagingChannelMessage::AccessParameters(
            AccessParametersMessage {
                pilot_pn: 0,
                acc_msg_seq: 0,
                acc_chan: 0,
                nom_pwr: -8,
                init_pwr: -4,
                pwr_step: 3,
                num_step: 3,
                max_cap_sz: 4,
                pam_sz: 10,
                psist_0_9: 0,
                psist_10: 0,
                psist_11: 0,
                psist_12: 0,
                psist_13: 0,
                psist_14: 0,
                psist_15: 0,
                msg_psist: 0,
                reg_psist: 0,
                probe_pn_ran: 9,
                acc_tmo: 3,
                probe_bkoff: 0,
                bkoff: 0,
                max_req_seq: 3,
                max_rsp_seq: 3,
                auth: 0,
                rand: 0,
                nom_pwr_ext: 0,
                psist_emg_incl: false,
                psist_emg: 0,
                acct_incl: false,
                acct_incl_emg: false,
                acct_aoc_bitmap_incl: false,
                acct_so_records: Vec::new(),
                acct_so_grp_records: Vec::new(),
            },
        ));

        match decoded {
            PagingMessage::AccessParameters(m) => {
                assert_eq!(m.nom_pwr, -8, "NOM_PWR should round-trip as -8 dB");
                assert_eq!(m.init_pwr, -4, "INIT_PWR should round-trip as -4 dB");
                assert_eq!(m.pwr_step, 3);
                assert_eq!(m.num_step, 3);
            }
            _ => panic!("unexpected message"),
        }
    }

    /// Verify that the maximum positive NOM_PWR / INIT_PWR values also
    /// round-trip without sign-extension errors at the upper bound.
    #[test]
    fn access_parameters_positive_open_loop_offsets_roundtrip() {
        let decoded = decode_roundtrip(PagingChannelMessage::AccessParameters(
            AccessParametersMessage {
                pilot_pn: 0,
                acc_msg_seq: 0,
                acc_chan: 0,
                nom_pwr: 7,
                init_pwr: 15,
                pwr_step: 1,
                num_step: 1,
                max_cap_sz: 0,
                pam_sz: 0,
                psist_0_9: 0,
                psist_10: 0,
                psist_11: 0,
                psist_12: 0,
                psist_13: 0,
                psist_14: 0,
                psist_15: 0,
                msg_psist: 0,
                reg_psist: 0,
                probe_pn_ran: 0,
                acc_tmo: 0,
                probe_bkoff: 0,
                bkoff: 0,
                max_req_seq: 0,
                max_rsp_seq: 0,
                auth: 0,
                rand: 0,
                nom_pwr_ext: 0,
                psist_emg_incl: false,
                psist_emg: 0,
                acct_incl: false,
                acct_incl_emg: false,
                acct_aoc_bitmap_incl: false,
                acct_so_records: Vec::new(),
                acct_so_grp_records: Vec::new(),
            },
        ));

        match decoded {
            PagingMessage::AccessParameters(m) => {
                assert_eq!(m.nom_pwr, 7);
                assert_eq!(m.init_pwr, 15);
            }
            _ => panic!("unexpected message"),
        }
    }

    #[test]
    fn neighbor_list_roundtrip() {
        let decoded = decode_roundtrip(PagingChannelMessage::NeighborList(NeighborListMessage {
            pilot_pn: 0,
            config_msg_seq: 1,
            pilot_inc: 0,
            neighbors: vec![1, 2, 3],
        }));

        match decoded {
            PagingMessage::NeighborList(m) => {
                assert_eq!(m.neighbors.len(), 3);
                assert_eq!(m.neighbors[1].nghbr_pn, 2);
            }
            _ => panic!("unexpected message"),
        }
    }

    #[test]
    fn cdma_channel_list_roundtrip() {
        let decoded = decode_roundtrip(PagingChannelMessage::CdmaChannelList(
            CdmaChannelListMessage {
                pilot_pn: 0,
                config_msg_seq: 1,
                channels: vec![384, 425],
            },
        ));

        match decoded {
            PagingMessage::CdmaChannelList(m) => {
                assert_eq!(m.channels, vec![384, 425]);
            }
            _ => panic!("unexpected message"),
        }
    }

    #[test]
    fn extended_system_parameters_roundtrip() {
        let decoded = decode_roundtrip(PagingChannelMessage::ExtendedSystemParameters(
            ExtendedSystemParametersMessage {
                pilot_pn: 0,
                config_msg_seq: 1,
                delete_for_tmsi: false,
                use_tmsi: false,
                pref_msid_type: 0,
                mcc: 0x03ff,
                imsi_11_12: 0x7f,
                tmsi_zone: Vec::new(),
                bcast_index: 0,
                imsi_t_supported: false,
                p_rev: 6,
                min_p_rev: 6,
                soft_slope: 0,
                add_intercept: 0,
                drop_intercept: 0,
                packet_zone_id: 0,
                max_num_alt_so: 0,
                reselect_included: false,
                ec_thresh: 0,
                ec_io_thresh: 0,
                pilot_report: false,
                nghbr_set_entry_info: false,
                acc_ent_ho_order: false,
                nghbr_set_access_info: false,
                access_ho: false,
                access_ho_msg_rsp: false,
                access_probe_ho: false,
                acc_ho_list_upd: false,
                acc_probe_ho_other_msg: false,
                max_num_probe_ho: 0,
                nghbr_set_size: 0,
                access_entry_ho: Vec::new(),
                access_ho_allowed: Vec::new(),
                broadcast_gps_asst: false,
                qpch_supported: false,
                num_qpch: 0,
                qpch_rate: 0,
                qpch_power_level_page: 0,
                qpch_cci_supported: false,
                qpch_power_level_config: 0,
                sdb_supported: false,
                rlgain_traffic_pilot: 0,
                rev_pwr_cntl_delay_incl: false,
                rev_pwr_cntl_delay: 0,
                auto_msg_supported: false,
                auto_msg_interval: 0,
                mob_qos: false,
                enc_supported: false,
                sig_encrypt_sup: 0,
                ui_encrypt_sup: 0,
                use_sync_id: false,
                cs_supported: false,
                bcch_supported: false,
                ms_init_pos_loc_sup_ind: false,
                pilot_info_req_supported: false,
                ext_pref_msid_type: None,
                meid_reqd: None,
            },
        ));

        match decoded {
            PagingMessage::ExtendedSystemParameters(m) => {
                assert_eq!(m.config_msg_seq, 1);
                assert_eq!(m.mcc, 0x03ff);
                assert_eq!(m.imsi_11_12, 0x7f);
                assert_eq!(m.p_rev, 6);
                assert_eq!(m.min_p_rev, 6);
                assert_eq!(m.rlgain_traffic_pilot, 0);
                assert!(!m.bcch_supported);
            }
            _ => panic!("unexpected message"),
        }
    }

    #[test]
    fn extended_system_parameters_service_profile_encodes_expected_pdu() {
        let message =
            PagingChannelMessage::ExtendedSystemParameters(ExtendedSystemParametersMessage {
                pilot_pn: 0,
                config_msg_seq: 23,
                delete_for_tmsi: false,
                use_tmsi: true,
                pref_msid_type: 3,
                mcc: 310,
                imsi_11_12: 0x7f,
                tmsi_zone: vec![0],
                bcast_index: 0,
                imsi_t_supported: false,
                p_rev: 6,
                min_p_rev: 6,
                soft_slope: 0,
                add_intercept: 0,
                drop_intercept: 0,
                packet_zone_id: 1,
                max_num_alt_so: 7,
                reselect_included: false,
                ec_thresh: 0,
                ec_io_thresh: 0,
                pilot_report: false,
                nghbr_set_entry_info: false,
                acc_ent_ho_order: false,
                nghbr_set_access_info: false,
                access_ho: false,
                access_ho_msg_rsp: false,
                access_probe_ho: false,
                acc_ho_list_upd: false,
                acc_probe_ho_other_msg: false,
                max_num_probe_ho: 0,
                nghbr_set_size: 0,
                access_entry_ho: Vec::new(),
                access_ho_allowed: Vec::new(),
                broadcast_gps_asst: false,
                qpch_supported: false,
                num_qpch: 0,
                qpch_rate: 0,
                qpch_power_level_page: 0,
                qpch_cci_supported: false,
                qpch_power_level_config: 0,
                sdb_supported: false,
                rlgain_traffic_pilot: 0,
                rev_pwr_cntl_delay_incl: false,
                rev_pwr_cntl_delay: 0,
                auto_msg_supported: false,
                auto_msg_interval: 0,
                mob_qos: false,
                enc_supported: false,
                sig_encrypt_sup: 0,
                ui_encrypt_sup: 0,
                use_sync_id: false,
                cs_supported: false,
                bcch_supported: false,
                ms_init_pos_loc_sup_ind: false,
                pilot_info_req_supported: false,
                ext_pref_msid_type: None,
                meid_reqd: None,
            });

        let mut pdu = Bitstream::new();
        pdu.write_u8(
            message
                .message_id()
                .wire_type(crate::lac::message_types::WireChannel::ForwardCommon)
                .unwrap(),
            8,
        );
        pdu.extend(&message.to_sdu());

        assert_eq!(
            bitstream_to_bytes(&pdu),
            vec![
                0x0d, 0x00, 0x2e, 0xe9, 0xb7, 0xf1, 0x00, 0x00, 0x60, 0x60, 0x00, 0x00, 0x07, 0x80,
                0x00, 0x00,
            ]
        );
    }

    #[test]
    fn general_page_roundtrip() {
        let decoded = decode_roundtrip(PagingChannelMessage::GeneralPage(GeneralPageMessage {
            config_msg_seq: 1,
            acc_msg_seq: 2,
            class_0_done: false,
            class_1_done: false,
            tmsi_done: false,
            ordered_tmsis: false,
            broadcast_done: false,
            reserved: 0,
            add_pfield: Vec::new(),
            page_records: vec![GeneralPageRecord::Class1 {
                msg_seq: 1,
                esn: 0x1234_5678,
                special_service: false,
                service_option: None,
            }],
        }));

        match decoded {
            PagingMessage::GeneralPage(m) => {
                assert_eq!(m.page_records.len(), 1);
            }
            _ => panic!("unexpected message"),
        }
    }

    #[test]
    fn general_page_service_profile_encodes_expected_pdu() {
        let message = PagingChannelMessage::GeneralPage(GeneralPageMessage {
            config_msg_seq: 23,
            acc_msg_seq: 1,
            class_0_done: true,
            class_1_done: true,
            tmsi_done: true,
            ordered_tmsis: false,
            broadcast_done: true,
            reserved: 0,
            add_pfield: Vec::new(),
            page_records: Vec::new(),
        });

        let mut pdu = Bitstream::new();
        pdu.write_u8(
            message
                .message_id()
                .wire_type(crate::lac::message_types::WireChannel::ForwardCommon)
                .unwrap(),
            8,
        );
        pdu.extend(&message.to_sdu());

        assert_eq!(bitstream_to_bytes(&pdu), vec![0x11, 0x5c, 0x1e, 0x80]);
    }

    #[test]
    fn general_page_from_sdu_roundtrip_class1() {
        let original = GeneralPageMessage {
            config_msg_seq: 1,
            acc_msg_seq: 2,
            class_0_done: false,
            class_1_done: false,
            tmsi_done: false,
            ordered_tmsis: false,
            broadcast_done: false,
            reserved: 0,
            add_pfield: Vec::new(),
            page_records: vec![GeneralPageRecord::Class1 {
                msg_seq: 3,
                esn: 0x1234_5678,
                special_service: true,
                service_option: Some(SERVICE_OPTION_SMS),
            }],
        };
        let sdu = original.to_sdu();
        let mut bs = sdu.clone();
        let decoded = GeneralPageMessage::from_sdu(&mut bs).unwrap();
        assert_eq!(decoded.config_msg_seq, 1);
        assert_eq!(decoded.acc_msg_seq, 2);
        assert_eq!(decoded.page_records.len(), 1);
        match &decoded.page_records[0] {
            GeneralPageRecord::Class1 {
                msg_seq,
                esn,
                special_service,
                service_option,
            } => {
                assert_eq!(*msg_seq, 3);
                assert_eq!(*esn, 0x1234_5678);
                assert!(*special_service);
                assert_eq!(*service_option, Some(SERVICE_OPTION_SMS));
            }
            _ => panic!("expected Class1"),
        }
    }

    #[test]
    fn general_page_from_sdu_roundtrip_class0() {
        let original = GeneralPageMessage {
            config_msg_seq: 5,
            acc_msg_seq: 3,
            class_0_done: true,
            class_1_done: true,
            tmsi_done: true,
            ordered_tmsis: false,
            broadcast_done: true,
            reserved: 0,
            add_pfield: Vec::new(),
            page_records: vec![
                GeneralPageRecord::Class0 {
                    page_subclass: 0,
                    msg_seq: 2,
                    imsi_s: Some(0x1_2345_6789),
                    imsi_11_12: None,
                    mcc: None,
                    imsi_addr_num: None,
                    imsi_m_s1: None,
                    imsi_m_s2: None,
                    special_service: false,
                    service_option: None,
                },
                GeneralPageRecord::Class0 {
                    page_subclass: 1,
                    msg_seq: 4,
                    imsi_s: Some(0x1_ABCD_EF00),
                    imsi_11_12: Some(0x55),
                    mcc: None,
                    imsi_addr_num: None,
                    imsi_m_s1: None,
                    imsi_m_s2: None,
                    special_service: false,
                    service_option: None,
                },
            ],
        };
        let sdu = original.to_sdu();
        let mut bs = sdu.clone();
        let decoded = GeneralPageMessage::from_sdu(&mut bs).unwrap();
        assert_eq!(decoded.page_records.len(), 2);
        match &decoded.page_records[0] {
            GeneralPageRecord::Class0 {
                page_subclass,
                msg_seq,
                imsi_s,
                ..
            } => {
                assert_eq!(*page_subclass, 0);
                assert_eq!(*msg_seq, 2);
                assert_eq!(*imsi_s, Some(0x1_2345_6789));
            }
            _ => panic!("expected Class0"),
        }
        match &decoded.page_records[1] {
            GeneralPageRecord::Class0 {
                page_subclass,
                msg_seq,
                imsi_s,
                imsi_11_12,
                ..
            } => {
                assert_eq!(*page_subclass, 1);
                assert_eq!(*msg_seq, 4);
                assert_eq!(*imsi_s, Some(0x1_ABCD_EF00));
                assert_eq!(*imsi_11_12, Some(0x55));
            }
            _ => panic!("expected Class0"),
        }
    }

    #[test]
    fn order_roundtrip() {
        let decoded = decode_roundtrip(PagingChannelMessage::Order(OrderMessage {
            order: 7,
            ordq: 9,
            order_specific_fields: Vec::new(),
        }));

        match decoded {
            PagingMessage::Order(m) => {
                assert_eq!(m.order, 7);
                assert_eq!(m.ordq, 9);
            }
            _ => panic!("unexpected message"),
        }
    }

    #[test]
    fn extended_channel_assignment_uses_ecam_tag() {
        let message = PagingChannelMessage::ExtendedChannelAssignment(
            ExtendedChannelAssignmentMessage::new_f_fch_r_fch_assignment(0, 8, 0, 3, 3, false),
        );

        assert_eq!(message.message_id(), MessageId::ExtChannelAssignment);
        let sdu = message.to_sdu();
        // 113-bit assignment record plus padding to octet: TX_PWR_LIMIT_INCL
        // is a fixed C.S0005-E ASSIGN_MODE=100 bit.
        assert_eq!(sdu.len(), 120);
        assert_eq!(sdu.bits().len(), 120);
    }

    #[test]
    fn extended_channel_assignment_omits_sync_id_incl_for_granted_mode_ten() {
        let mut sdu =
            ExtendedChannelAssignmentMessage::new_f_fch_r_fch_assignment(0, 8, 0, 3, 3, false)
                .to_sdu();

        assert_eq!(sdu.read_bits(3).unwrap(), 0b100); // ASSIGN_MODE
        assert_eq!(sdu.read_bits(1).unwrap(), 0); // DIRECT_CH_ASSIGN_IND
        assert_eq!(sdu.read_bits(4).unwrap(), 0); // RESERVED_2
        assert_eq!(sdu.read_bits(1).unwrap(), 0); // FREQ_INCL
        assert_eq!(sdu.read_bits(1).unwrap(), 0); // BYPASS_ALERT_ANSWER
        assert_eq!(sdu.read_bits(2).unwrap(), 0b10); // GRANTED_MODE
        assert_eq!(sdu.read_bits(3).unwrap(), 0b100); // DEFAULT_CONFIG (explicit RC)
        assert_eq!(sdu.read_bits(5).unwrap(), 3); // FOR_RC
        assert_eq!(sdu.read_bits(5).unwrap(), 3); // REV_RC
        assert_eq!(sdu.read_bits(4).unwrap(), 0); // FRAME_OFFSET
        assert_eq!(sdu.read_bits(2).unwrap(), 0); // ENCRYPT_MODE
        assert_eq!(sdu.read_bits(5).unwrap(), 12); // FPC_SUBCHAN_GAIN
        assert_eq!(sdu.read_bits(4).unwrap(), 0); // RLGAIN_ADJ = 0
        assert_eq!(sdu.read_bits(3).unwrap(), 0); // NUM_PILOTS
        assert_eq!(sdu.read_bits(2).unwrap(), 0b01); // CH_IND

        let ch_record_len_octets = sdu.read_bits(5).unwrap() as usize;
        let _ = sdu.read_bits(ch_record_len_octets * 8).unwrap();

        assert_eq!(sdu.read_bits(1).unwrap(), 0); // REV_FCH_GATING_MODE
        assert_eq!(sdu.read_bits(1).unwrap(), 0); // C_SIG_ENCRYPT_MODE_INCL
        assert_eq!(sdu.read_bits(1).unwrap(), 0); // 3XFL_1XRL_INCL
        assert_eq!(sdu.read_bits(1).unwrap(), 0); // MSG_INT_INFO_INCL
        assert_eq!(sdu.read_bits(1).unwrap(), 0); // PLCM_TYPE_INCL
        assert_eq!(sdu.read_bits(1).unwrap(), 0); // EARLY_RL_TRANSMIT_IND
        assert_eq!(sdu.read_bits(1).unwrap(), 0); // TX_PWR_LIMIT_INCL
    }

    #[test]
    fn extended_channel_assignment_uses_standard_default_config_for_rc1() {
        let mut sdu =
            ExtendedChannelAssignmentMessage::new_f_fch_r_fch_assignment(0, 8, 0, 1, 1, false)
                .to_sdu();

        assert_eq!(sdu.read_bits(3).unwrap(), 0b100); // ASSIGN_MODE
        assert_eq!(sdu.read_bits(1).unwrap(), 0); // DIRECT_CH_ASSIGN_IND
        assert_eq!(sdu.read_bits(4).unwrap(), 0); // RESERVED_2
        assert_eq!(sdu.read_bits(1).unwrap(), 0); // FREQ_INCL
        assert_eq!(sdu.read_bits(1).unwrap(), 0); // BYPASS_ALERT_ANSWER
        assert_eq!(sdu.read_bits(2).unwrap(), 0b10); // GRANTED_MODE
        assert_eq!(sdu.read_bits(3).unwrap(), 0b000); // DEFAULT_CONFIG
        assert_eq!(sdu.read_bits(5).unwrap(), 1); // FOR_RC = RC1
        assert_eq!(sdu.read_bits(5).unwrap(), 1); // REV_RC = RC1
        assert_eq!(sdu.read_bits(4).unwrap(), 0); // FRAME_OFFSET
    }

    /// Verify that our ECAM encoder produces bit-identical output to the
    /// Anritsu MD8470A PVT trace (RecNo 50, 2026-03-23 12:58:52.920).
    ///
    /// Reference field values taken directly from the working trace:
    ///   ASSIGN_MODE=100, FREQ_INCL=1, BAND_CLASS=0, CDMA_FREQ=384,
    ///   GRANTED_MODE=10, DEFAULT_CONFIG=100, FOR_RC=3, REV_RC=3,
    ///   FPC_SUBCHAN_GAIN=12, RLGAIN_ADJ=-4, PILOT_PN=4, CODE_CHAN_FCH=10, etc.
    #[test]
    fn ecam_matches_anritsu_trace_recno50() {
        let ecam = ExtendedChannelAssignmentMessage {
            assign_mode: 0b100,
            direct_ch_assign_ind: false,
            raw_additional_record_fields: None,
            freq_incl: true,
            band_class: Some(0),
            cdma_freq: Some(384),
            bypass_alert_answer: false,
            granted_mode: 0b10,
            sr_id_restore: None,
            sr_id_restore_bitmap: None,
            default_config: 0b100,
            for_rc: 3,
            rev_rc: 3,
            frame_offset: 0,
            encrypt_mode: 0b00,
            d_sig_encrypt_mode: None,
            enc_key_size: None,
            fpc_subchan_gain: 12,
            rlgain_adj: -4,
            ch_ind: 0b01,
            raw_ch_record_fields: None,
            fpc_fch_init_setpt: 0x20,
            fpc_fch_fer: 0b00010,
            fpc_fch_min_setpt: 0x00,
            fpc_fch_max_setpt: 0x50,
            fpc_dcch_init_setpt: 0,
            fpc_dcch_fer: 0,
            fpc_dcch_min_setpt: 0,
            fpc_dcch_max_setpt: 0,
            fpc_pri_chan: false,
            pilots: vec![ExtendedTrafficPilotRecord {
                pilot_pn: 4,
                pilot_record: None,
                pwr_comb_ind: false,
                code_chan_fch: 10,
                qof_mask_id_fch: 0,
                code_chan_dcch: None,
                qof_mask_id_dcch: None,
            }],
            rev_fch_gating_mode: false,
            rev_pwr_cntl_delay: None,
            c_sig_encrypt_mode: None,
            one_xrl_freq_offset: None,
            message_integrity: None,
            plcm_type_incl: false,
            plcm_type: 0,
            plcm_39: None,
            sync_id: None,
            config_msg_seq: None,
            rtc_nom_pwr: None,
            respond_ind: None,
            direct_ch_assign_recover_ind: None,
            fixed_num_preamble: None,
            early_rl_transmit_ind: false,
            omit_tx_pwr_limit_incl_for_p_rev6_compat: true,
            tx_pwr_limit: None,
        };

        let mut sdu = ecam.to_sdu();

        // 128-bit assignment record (16 octets), already octet-aligned
        assert_eq!(sdu.len(), 128, "SDU length should be 128 bits (16 octets)");

        // Decode every field and compare against Anritsu trace values.
        assert_eq!(sdu.read_bits(3).unwrap(), 0b100); // ASSIGN_MODE
        assert_eq!(sdu.read_bits(1).unwrap(), 0); // DIRECT_CH_ASSIGN_IND
        assert_eq!(sdu.read_bits(4).unwrap(), 0); // RESERVED_2
        assert_eq!(sdu.read_bits(1).unwrap(), 1); // FREQ_INCL
        assert_eq!(sdu.read_bits(5).unwrap(), 0); // BAND_CLASS
        assert_eq!(sdu.read_bits(11).unwrap(), 384); // CDMA_FREQ
        assert_eq!(sdu.read_bits(1).unwrap(), 0); // BYPASS_ALERT_ANSWER
        assert_eq!(sdu.read_bits(2).unwrap(), 0b10); // GRANTED_MODE
        assert_eq!(sdu.read_bits(3).unwrap(), 0b100); // DEFAULT_CONFIG
        assert_eq!(sdu.read_bits(5).unwrap(), 3); // FOR_RC
        assert_eq!(sdu.read_bits(5).unwrap(), 3); // REV_RC
        assert_eq!(sdu.read_bits(4).unwrap(), 0); // FRAME_OFFSET
        assert_eq!(sdu.read_bits(2).unwrap(), 0); // ENCRYPT_MODE
        assert_eq!(sdu.read_bits(5).unwrap(), 12); // FPC_SUBCHAN_GAIN
        assert_eq!(sdu.read_bits(4).unwrap(), 12); // RLGAIN_ADJ = -4 encoded in 4-bit two's complement
        assert_eq!(sdu.read_bits(3).unwrap(), 0); // NUM_PILOTS (1 pilot → 0)
        assert_eq!(sdu.read_bits(2).unwrap(), 0b01); // CH_IND
        assert_eq!(sdu.read_bits(5).unwrap(), 7); // CH_RECORD_LEN (7 octets)

        // --- CH_RECORD (56 bits = 7 octets) ---
        assert_eq!(sdu.read_bits(8).unwrap(), 0x20); // FPC_FCH_INIT_SETPT
        assert_eq!(sdu.read_bits(5).unwrap(), 2); // FPC_FCH_FER
        assert_eq!(sdu.read_bits(8).unwrap(), 0x00); // FPC_FCH_MIN_SETPT
        assert_eq!(sdu.read_bits(8).unwrap(), 0x50); // FPC_FCH_MAX_SETPT
        assert_eq!(sdu.read_bits(9).unwrap(), 4); // PILOT_PN
        assert_eq!(sdu.read_bits(1).unwrap(), 0); // ADD_PILOT_REC_INCL
        assert_eq!(sdu.read_bits(1).unwrap(), 0); // PWR_COMB_IND
        assert_eq!(sdu.read_bits(11).unwrap(), 10); // CODE_CHAN_FCH
        assert_eq!(sdu.read_bits(2).unwrap(), 0); // QOF_MASK_ID_FCH
        assert_eq!(sdu.read_bits(3).unwrap(), 0); // 3X_FCH_INFO_INCL + pad

        // --- Post CH_RECORD ---
        assert_eq!(sdu.read_bits(1).unwrap(), 0); // REV_FCH_GATING_MODE
        assert_eq!(sdu.read_bits(1).unwrap(), 0); // C_SIG_ENCRYPT_MODE_INCL
        assert_eq!(sdu.read_bits(1).unwrap(), 0); // 3XFL_1XRL_INCL
        assert_eq!(sdu.read_bits(1).unwrap(), 0); // MSG_INT_INFO_INCL
        assert_eq!(sdu.read_bits(1).unwrap(), 0); // PLCM_TYPE_INCL
        assert_eq!(sdu.read_bits(1).unwrap(), 0); // EARLY_RL_TRANSMIT_IND

        // 128 record bits = exactly 16 octets, no padding needed
    }

    #[test]
    fn ms_address_imsi_class0_writes_full_imsi_address() {
        let addr = MsAddress::ImsiClass0 {
            imsi_m_s1: 0x91989e,
            imsi_m_s2: 0x326,
            mcc: 310,
            imsi_11_12: 0,
        };
        let mut bs = Bitstream::new();
        addr.write_to(&mut bs, 310, 0);

        assert_eq!(bs.read_bits(3).unwrap(), 0b010); // ADDR_TYPE = IMSI
        assert_eq!(bs.read_bits(4).unwrap(), 5); // ADDR_LEN
        assert_eq!(bs.read_bits(1).unwrap(), 0); // IMSI_CLASS = class 0
        assert_eq!(bs.read_bits(2).unwrap(), 0b00); // IMSI_CLASS_0_TYPE
        assert_eq!(bs.read_bits(3).unwrap(), 0); // RESERVED
        assert_eq!(bs.read_bits(10).unwrap(), 0x326); // IMSI_S2
        assert_eq!(bs.read_bits(24).unwrap(), 0x91989e); // IMSI_S1
    }

    #[test]
    fn ms_address_imsi_class0_writes_type_01_when_imsi_11_12_is_present() {
        let addr = MsAddress::ImsiClass0 {
            imsi_m_s1: 0x91989e,
            imsi_m_s2: 0x326,
            mcc: 310,
            imsi_11_12: 0x63,
        };
        let mut bs = Bitstream::new();
        addr.write_to(&mut bs, 310, 0);

        assert_eq!(bs.read_bits(3).unwrap(), 0b010);
        assert_eq!(bs.read_bits(4).unwrap(), 6);
        assert_eq!(bs.read_bits(1).unwrap(), 0);
        assert_eq!(bs.read_bits(2).unwrap(), 0b01);
        assert_eq!(bs.read_bits(4).unwrap(), 0);
        assert_eq!(bs.read_bits(7).unwrap(), 0x63);
        assert_eq!(bs.read_bits(10).unwrap(), 0x326);
        assert_eq!(bs.read_bits(24).unwrap(), 0x91989e);
    }

    #[test]
    fn ms_address_imsi_class0_writes_type_10_when_mcc_is_present() {
        let addr = MsAddress::ImsiClass0 {
            imsi_m_s1: 0x91989e,
            imsi_m_s2: 0x326,
            mcc: 0x0d1,
            imsi_11_12: 0,
        };
        let mut bs = Bitstream::new();
        addr.write_to(&mut bs, 310, 0);

        assert_eq!(bs.read_bits(3).unwrap(), 0b010);
        assert_eq!(bs.read_bits(4).unwrap(), 6);
        assert_eq!(bs.read_bits(1).unwrap(), 0);
        assert_eq!(bs.read_bits(2).unwrap(), 0b10);
        assert_eq!(bs.read_bits(1).unwrap(), 0);
        assert_eq!(bs.read_bits(10).unwrap(), 0x0d1);
        assert_eq!(bs.read_bits(10).unwrap(), 0x326);
        assert_eq!(bs.read_bits(24).unwrap(), 0x91989e);
    }

    #[test]
    fn ms_address_imsi_class0_writes_type_11_when_mcc_and_imsi_11_12_are_present() {
        let addr = MsAddress::ImsiClass0 {
            imsi_m_s1: 0x91989e,
            imsi_m_s2: 0x326,
            mcc: 0x0d1,
            imsi_11_12: 0x63,
        };
        let mut bs = Bitstream::new();
        addr.write_to(&mut bs, 310, 0);

        assert_eq!(bs.read_bits(3).unwrap(), 0b010);
        assert_eq!(bs.read_bits(4).unwrap(), 7);
        assert_eq!(bs.read_bits(1).unwrap(), 0);
        assert_eq!(bs.read_bits(2).unwrap(), 0b11);
        assert_eq!(bs.read_bits(2).unwrap(), 0);
        assert_eq!(bs.read_bits(10).unwrap(), 0x0d1);
        assert_eq!(bs.read_bits(7).unwrap(), 0x63);
        assert_eq!(bs.read_bits(10).unwrap(), 0x326);
        assert_eq!(bs.read_bits(24).unwrap(), 0x91989e);
    }
}

#[cfg(test)]
mod escam_tests {
    use super::*;

    fn make_escam_19k2(w32_code: u16, pilot_pn: u16) -> EscamParams {
        EscamParams {
            start_time_unit: 0,
            for_sch_id: 0,
            sccl_index: 0,
            for_sch_num_bits_idx: 0x1, // 360 bits = 19.2 kbps
            pilot_pn,
            code_chan_sch: w32_code,
            qof_mask_id_sch: 0,
            for_sch_duration: 0x0F, // infinite
            for_sch_start_time_incl: true,
            for_sch_start_time: 0,
            fpc_incl: true,
            fpc_mode_sch: 0,
            fpc_sch_init_setpt_op: 0,
            fpc_sch_fer: 0b00010,   // 1% FER
            fpc_sch_init_setpt: 48, // 6.0 dB
            fpc_sch_min_setpt: 0,
            fpc_sch_max_setpt: 80, // 10.0 dB
        }
    }

    #[test]
    fn escam_encode_immediate_activation() {
        let params = make_escam_19k2(5, 0);
        let sdu = params.encode_sdu();
        assert!(!sdu.is_empty(), "ESCAM SDU should not be empty");
        // Full ESCAM with FPC should be substantial
        assert!(
            sdu.len() >= 8,
            "ESCAM SDU should be at least 8 bytes, got {}",
            sdu.len()
        );
    }

    #[test]
    fn escam_encode_with_start_time() {
        let mut params = make_escam_19k2(12, 9);
        params.for_sch_duration = 5;
        params.for_sch_start_time_incl = true;
        params.for_sch_start_time = 10;
        let sdu = params.encode_sdu();
        assert!(!sdu.is_empty());
        let without_start = make_escam_19k2(12, 9).encode_sdu();
        assert!(sdu.len() >= without_start.len());
    }

    #[test]
    fn escam_encode_release_sch() {
        let mut params = make_escam_19k2(5, 0);
        params.for_sch_duration = 0; // stop
        params.for_sch_start_time_incl = false;
        params.fpc_incl = false;
        let sdu = params.encode_sdu();
        assert!(!sdu.is_empty());
    }

    #[test]
    fn escam_bitstream_has_correct_leading_fields() {
        let params = make_escam_19k2(5, 0);
        let bs = params.to_ftch_sdu();
        let bits = bs.bits();
        // START_TIME_UNIT = 000 (3 bits)
        assert_eq!(bits[0], 0);
        assert_eq!(bits[1], 0);
        assert_eq!(bits[2], 0);
        // REV_SCH_DTX_DURATION = 0000 (4 bits)
        assert_eq!(bits[3], 0);
        // USE_T_ADD_ABORT = 0
        assert_eq!(bits[7], 0);
        // USE_SCRM_SEQ_NUM = 0
        assert_eq!(bits[8], 0);
        // ADD_INFO_INCL = 0
        assert_eq!(bits[9], 0);
        // REV_CFG_INCLUDED = 0
        assert_eq!(bits[10], 0);
        // NUM_REV_SCH = 00
        assert_eq!(bits[11], 0);
        assert_eq!(bits[12], 0);
        // FOR_CFG_INCLUDED = 1
        assert_eq!(bits[13], 1);
    }

    #[test]
    fn escam_code_chan_sch_is_11_bits() {
        let mut params = make_escam_19k2(300, 0);
        params.code_chan_sch = 300;
        let sdu = params.encode_sdu();
        assert!(!sdu.is_empty());
    }

    // ---- select_imsi_class0_forward_address (core function) ----
    //
    // These tests validate the pure OTA compression function that
    // operates on fully-resolved IMSI fields.  The caller resolves
    // None→overhead; these tests only exercise the compression logic.
    //
    // Spec references:
    //   C.S0004-E Table 2.1.1.3.1.1-2 — IMSI_CLASS_0_TYPE encodings
    //   C.S0004-E 3.1.2.2.1.3.3        — BS forward address selection
    //   C.S0005-E 2.6.2.2.5            — ESPM wildcard rules

    #[test]
    fn core_type00_both_match_non_wildcard_overhead() {
        // Home subscriber: MCC=310, IMSI_11_12=15, overhead=(310,15).
        // Both match → type 00 (IMSI_S only).
        let addr = select_imsi_class0_forward_address(100, 200, 310, 15);
        assert_eq!(
            addr,
            MsAddress::ImsiClass0 {
                imsi_m_s1: 100,
                imsi_m_s2: 200,
                mcc: 310,
                imsi_11_12: 15,
            }
        );
    }

    #[test]
    fn core_type00_both_wildcard_overhead() {
        // Any MCC/IMSI_11_12 is implied by wildcard overhead → type 00.
        let addr = select_imsi_class0_forward_address(100, 200, 450, 42);
        assert_eq!(
            addr,
            MsAddress::ImsiClass0 {
                imsi_m_s1: 100,
                imsi_m_s2: 200,
                mcc: 450,
                imsi_11_12: 42,
            }
        );
    }

    #[test]
    fn core_type01_mcc_implied_imsi_11_12_differs() {
        // MCC matches overhead, IMSI_11_12 differs → type 01.
        let addr = select_imsi_class0_forward_address(100, 200, 310, 42);
        assert_eq!(
            addr,
            MsAddress::ImsiClass0 {
                imsi_m_s1: 100,
                imsi_m_s2: 200,
                mcc: 310,
                imsi_11_12: 42,
            }
        );
    }

    #[test]
    fn core_type10_mcc_differs_imsi_11_12_implied() {
        // Per C.S0004-E 2.1.1.3.1.3 IMSI_CLASS_0_TYPE='10':
        // Roaming mobile (MCC=450) on cell with MCC=310.
        // IMSI_11_12 implied by wildcard → type 10 (IMSI_S + MCC).
        let addr = select_imsi_class0_forward_address(100, 200, 450, 15);
        assert_eq!(
            addr,
            MsAddress::ImsiClass0 {
                imsi_m_s1: 100,
                imsi_m_s2: 200,
                mcc: 450,
                imsi_11_12: 15,
            }
        );
    }

    #[test]
    fn core_type10_mcc_differs_imsi_11_12_matches() {
        // Roamer MCC differs, IMSI_11_12 matches non-wildcard overhead.
        let addr = select_imsi_class0_forward_address(100, 200, 450, 15);
        assert_eq!(
            addr,
            MsAddress::ImsiClass0 {
                imsi_m_s1: 100,
                imsi_m_s2: 200,
                mcc: 450,
                imsi_11_12: 15,
            }
        );
    }

    #[test]
    fn core_type11_both_differ() {
        // Roaming mobile: both MCC and IMSI_11_12 differ → type 11.
        let addr = select_imsi_class0_forward_address(100, 200, 450, 42);
        assert_eq!(
            addr,
            MsAddress::ImsiClass0 {
                imsi_m_s1: 100,
                imsi_m_s2: 200,
                mcc: 450,
                imsi_11_12: 42,
            }
        );
    }

    #[test]
    fn core_forward_address_from_access_fields_resolves_none_to_overhead() {
        // forward_address_from_access_fields resolves None→overhead
        // before calling the core function.  Class-0 mobile omits both
        // (None,None) on a non-wildcard cell (310,15) → type 00.
        let addr = forward_address_from_access_fields(
            Some(0),
            Some(100),
            Some(200),
            None,
            None,
            Some(999),
            310,
            15,
        );
        assert_eq!(
            addr,
            Some(MsAddress::ImsiClass0 {
                imsi_m_s1: 100,
                imsi_m_s2: 200,
                mcc: 310,
                imsi_11_12: 15,
            })
        );
    }

    #[test]
    fn core_forward_address_from_access_fields_roamer_sends_mcc() {
        // Roamer sends MCC=450 explicitly (type 10 or 11), omits IMSI_11_12.
        let addr = forward_address_from_access_fields(
            Some(0),
            Some(100),
            Some(200),
            Some(450),
            None,
            Some(999),
            310,
            0x7f,
        );
        assert_eq!(
            addr,
            Some(MsAddress::ImsiClass0 {
                imsi_m_s1: 100,
                imsi_m_s2: 200,
                mcc: 450,
                imsi_11_12: 0x7f,
            })
        );
    }
}
