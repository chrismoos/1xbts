#![allow(dead_code, unused_imports)]

use std::time::Duration;

use crate::addressing::is_packet_data_so;
use crate::config::{PagingRetryConfig, TrafficAssignmentConfig, TrafficRetryConfig};
use cdma_abis::bearer::{ChannelFamily, FrameContent, ReverseFchDcchFrame};
use cdma_bts::bts::PagingChannelSettings;
use cdma_common::error::Error;
use cdma_common::lac::paging_messages::MsAddress;
use cdma_common::lac::paging_messages::MsPageAddress;
use cdma_common::lac::paging_messages::PagingChannelMessage;
use log::debug;
use tokio::sync::{broadcast, mpsc};
use uuid::Uuid;

use super::*;
use crate::abis_edge::BtsControlClient;
use crate::abis_edge::PchTransferAckEvent;
use crate::abis_edge::network::{NetworkBtsControlClient, NetworkClientConfig};
use crate::addressing::{select_imsi_class0_forward_address, select_initial_traffic_rcs};
use cdma_abis::control::typed::CellId;
use cdma_bts::bts::abis_agent::AbisAgentConfig;
use cdma_bts::bts::{
    TrafficChannelPool, TrafficResourceController, TrafficRxPool, TrafficRxRemovals, WalshAllocator,
};
use cdma_bts::lac as bts_lac;
use cdma_common::access::{AccessMessage, AccessMessageHeader, OriginationMessage};
use cdma_common::bits::Bitstream;
use cdma_common::events::AccessChannelEvent;
use cdma_common::formatting::format_dtmf_digits;
use cdma_common::lac;
use cdma_common::lac::message_types::MessageId;
use cdma_hlr::model::{
    RegistrationBinding, RegistrationState, Subscriber, SubscriberIdentity, SubscriberStatus,
};
use cdma_hlr::repository::HlrRepository;
use cdma_smsc::model::{
    DeliveryAttemptState, MoSmsFingerprint, SmsDeliveryAttempt, SmsState, SmsSubmission,
};
use cdma_smsc::repository::SmscRepository;
use itertools::Itertools;
use parking_lot::Mutex;
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

fn test_voice_policy() -> Arc<dyn cdma_msc::VoicePolicy> {
    Arc::new(cdma_msc::StaticVoicePolicy::new(
        cdma_msc::VoiceConfig::default(),
    ))
}

fn test_msc_client() -> Arc<dyn crate::a1_edge::MscClient> {
    let (client, _) = crate::a1_edge::InProcessMscClient::pair(32);
    Arc::new(client)
}

#[derive(Default)]
struct CapturingBtsClient {
    pch_messages: Mutex<Vec<cdma_abis::control::typed::PchMessageTransferMessage>>,
}

struct CapturingNetworkBtsClient {
    inner: NetworkBtsControlClient,
    pch_messages: Mutex<Vec<cdma_abis::control::typed::PchMessageTransferMessage>>,
}

#[async_trait::async_trait]
impl BtsControlClient for CapturingBtsClient {
    async fn allocate_rc1_traffic(
        &self,
        _lc_generator: cdma_common::phy::long_code::LongCodeGenerator,
        _initial_lc_chip: u64,
        _esn: u32,
    ) -> Option<crate::abis_edge::BtsTrafficChannelHandle> {
        None
    }

    async fn allocate_rc3_traffic(
        &self,
        _lc_generator: cdma_common::phy::long_code::LongCodeGenerator,
        _initial_lc_chip: u64,
        _fpc_subchan_gain: u8,
        _esn: u32,
    ) -> Option<crate::abis_edge::BtsTrafficChannelHandle> {
        None
    }

    async fn allocate_rc3_sch(
        &self,
        _lc_generator: cdma_common::phy::long_code::LongCodeGenerator,
        _sch_gain_linear: f32,
    ) -> Option<(u8, cdma_bts::bts::SchWalshChannelRc3)> {
        None
    }

    async fn deallocate_traffic(&self, _walsh_code: u8) {}

    async fn deallocate_sch(&self, _w32_code: u8) {}

    async fn set_traffic_gain(&self, _walsh_code: u8, _gain_linear: f32) -> bool {
        false
    }

    async fn install_rx_request(&self, _request: cdma_common::traffic::TrafficRxRequest) {}

    async fn drop_pending_rx_request(&self, _walsh_code: u8) {}

    async fn request_rx_removal(&self, _walsh_code: u8) {}

    fn send_pch_message(
        &self,
        message: cdma_abis::control::typed::PchMessageTransferMessage,
    ) -> Result<(), String> {
        self.pch_messages.lock().push(message);
        Ok(())
    }

    fn traffic_queue_len(&self, _walsh_code: u8) -> Option<usize> {
        None
    }

    fn last_traffic_enqueue_at(&self, _walsh_code: u8) -> Option<Instant> {
        None
    }
}

#[async_trait::async_trait]
impl BtsControlClient for CapturingNetworkBtsClient {
    fn bearer_client(&self) -> Option<&dyn crate::abis_edge::BtsBearerClient> {
        self.inner.bearer_client()
    }

    async fn allocate_rc1_traffic(
        &self,
        lc_generator: cdma_common::phy::long_code::LongCodeGenerator,
        initial_lc_chip: u64,
        esn: u32,
    ) -> Option<crate::abis_edge::BtsTrafficChannelHandle> {
        self.inner
            .allocate_rc1_traffic(lc_generator, initial_lc_chip, esn)
            .await
    }

    async fn allocate_rc3_traffic(
        &self,
        lc_generator: cdma_common::phy::long_code::LongCodeGenerator,
        initial_lc_chip: u64,
        fpc_subchan_gain: u8,
        esn: u32,
    ) -> Option<crate::abis_edge::BtsTrafficChannelHandle> {
        self.inner
            .allocate_rc3_traffic(lc_generator, initial_lc_chip, fpc_subchan_gain, esn)
            .await
    }

    async fn allocate_rc3_sch(
        &self,
        lc_generator: cdma_common::phy::long_code::LongCodeGenerator,
        sch_gain_linear: f32,
    ) -> Option<(u8, cdma_bts::bts::SchWalshChannelRc3)> {
        self.inner
            .allocate_rc3_sch(lc_generator, sch_gain_linear)
            .await
    }

    async fn deallocate_traffic(&self, walsh_code: u8) {
        self.inner.deallocate_traffic(walsh_code).await;
    }

    async fn deallocate_sch(&self, w32_code: u8) {
        self.inner.deallocate_sch(w32_code).await;
    }

    async fn set_traffic_gain(&self, walsh_code: u8, gain_linear: f32) -> bool {
        self.inner.set_traffic_gain(walsh_code, gain_linear).await
    }

    async fn install_rx_request(&self, request: cdma_common::traffic::TrafficRxRequest) {
        self.inner.install_rx_request(request).await;
    }

    async fn drop_pending_rx_request(&self, walsh_code: u8) {
        self.inner.drop_pending_rx_request(walsh_code).await;
    }

    async fn request_rx_removal(&self, walsh_code: u8) {
        self.inner.request_rx_removal(walsh_code).await;
    }

    fn send_pch_message(
        &self,
        message: cdma_abis::control::typed::PchMessageTransferMessage,
    ) -> Result<(), String> {
        self.pch_messages.lock().push(message.clone());
        self.inner.send_pch_message(message)
    }

    fn drain_pch_transfer_acks(&self) -> Vec<crate::abis_edge::PchTransferAckEvent> {
        self.inner.drain_pch_transfer_acks()
    }

    fn traffic_queue_len(&self, walsh_code: u8) -> Option<usize> {
        self.inner.traffic_queue_len(walsh_code)
    }

    fn last_traffic_enqueue_at(&self, walsh_code: u8) -> Option<Instant> {
        self.inner.last_traffic_enqueue_at(walsh_code)
    }
}

fn test_agent_config() -> AbisAgentConfig {
    AbisAgentConfig {
        pilot_pn: 0,
        cell_id: CellId { cell: 1, sector: 1 },
        mscid: 1,
    }
}

fn test_network_client_config() -> NetworkClientConfig {
    NetworkClientConfig {
        cell_id: CellId { cell: 1, sector: 1 },
        mscid: 1,
        pilot_pn: 0,
        auth_mode: 0,
        p_rev_in_use: 6,
        market_id: 1,
        generating_entity_id: 1,
    }
}

/// Wrap the four shared pool `Arc`s into a `BtsControlClient` backed by
/// an in-memory Abis transport and AbisAgent. Uses the same two-phase
/// allocation path as the real network client.
fn test_bts_client(
    walsh_allocator: Arc<Mutex<WalshAllocator>>,
    traffic_channels: TrafficChannelPool,
    traffic_rx_pool: TrafficRxPool,
    traffic_rx_removals: TrafficRxRemovals,
) -> Arc<dyn BtsControlClient> {
    let controller = Arc::new(TrafficResourceController::from_pools(
        walsh_allocator,
        traffic_channels,
        traffic_rx_pool,
        traffic_rx_removals,
    ));
    Arc::new(NetworkBtsControlClient::spawn_in_process(
        controller,
        test_agent_config(),
        test_network_client_config(),
    ))
}

fn test_capturing_bts_client(
    walsh_allocator: Arc<Mutex<WalshAllocator>>,
    traffic_channels: TrafficChannelPool,
    traffic_rx_pool: TrafficRxPool,
    traffic_rx_removals: TrafficRxRemovals,
) -> Arc<CapturingNetworkBtsClient> {
    let controller = Arc::new(TrafficResourceController::from_pools(
        walsh_allocator,
        traffic_channels,
        traffic_rx_pool,
        traffic_rx_removals,
    ));
    Arc::new(CapturingNetworkBtsClient {
        inner: NetworkBtsControlClient::spawn_in_process(
            controller,
            test_agent_config(),
            test_network_client_config(),
        ),
        pch_messages: Mutex::new(Vec::new()),
    })
}

fn pch_message_types(
    client: &CapturingNetworkBtsClient,
) -> Vec<cdma_common::lac::message_types::MessageId> {
    client
        .pch_messages
        .lock()
        .iter()
        .filter_map(|message| {
            let aim = message.air_interface_message.as_ref()?;
            MessageId::from_wire(
                cdma_common::lac::message_types::WireChannel::ForwardCommon,
                aim.message_type,
            )
        })
        .collect()
}

fn test_access_event() -> AccessChannelEvent {
    AccessChannelEvent {
        event_id: "test-access-event".to_string(),
        chip_start: 0,
        absolute_chip_start: None,
        receive_time: None,
        preamble_frames: 0,
        pd: 1,
        message_id: MessageId::Registration, // default; overridden per test
        msg_type_name: String::new(),
        address: None,
        resolved_address: None,
        subscriber_id: None,
        l3_summary: None,
        decoded_l3: None,
        pdu_summary: String::new(),
        msg_seq: None,
        ack_seq: None,
        ack_req: false,
        valid_ack: false,
        msid_type: None,
        esn: None,
        imsi: None,
        imsi_m_s1: None,
        imsi_m_s2: None,
        imsi_class: None,
        imsi_addr_num: None,
        imsi_mcc: None,
        imsi_11_12: None,
        mob_p_rev: None,
        slot_cycle_index: None,
        scm: None,
        wall_clock_us: chrono::Utc::now().timestamp_micros() as u64,
        rx_wall_time: None,
        rx_hw_time_ns: None,
        snr_db: None,
        signal_power_db: None,
        reverse_pilot_ec_io_db: None,
        raw_power_db: None,
        demod_quality_pct: None,
        pcg_signal_snr_db: None,
        active_pcg_mask: None,
        traffic_phy_valid: None,
        traffic_fqi_valid: None,
        traffic_tail_valid: None,
        traffic_fqi_bits: None,
        traffic_ml_tail_match: None,
        burst_type: None,
        data_burst_fields: None,
        data_burst_num_msgs: None,
        data_burst_msg_number: None,
        traffic_primary_bits: None,
        traffic_primary_rate_bps: None,
        traffic_primary_bearer_routed: false,
        traffic_voice_bits: None,
        traffic_voice_rate_bps: None,
        order_code: None,
        service_option: None,
        for_rc_pref: None,
        rev_rc_pref: None,
        rev_fch_gating_req: None,
        traffic_walsh_code: None,
        is_preamble_only: false,
        is_traffic_pcg_measurement: false,
        is_traffic_phy_status: false,
        traffic_measurement_age_chips: None,
        for_supported_rcs: Vec::new(),
        rev_supported_rcs: Vec::new(),
        decoded_rdsch: None,
        raw_pdu_bits: None,
    }
}

fn test_origination_l3(service_option: u16, sr_id: u8) -> AccessMessage {
    AccessMessage::Origination(OriginationMessage {
        header: AccessMessageHeader {
            pd: 1,
            message_id: MessageId::Origination,
        },
        mob_term: true,
        slot_cycle_index: 2,
        mob_p_rev: 6,
        scm: 0x2a,
        request_mode: 1,
        special_service: true,
        service_option: Some(service_option),
        pm: false,
        digit_mode: false,
        number_type: None,
        number_plan: None,
        more_fields: false,
        num_fields: 0,
        digits: Vec::new(),
        nar_an_cap: false,
        paca_reorig: false,
        return_cause: 0,
        more_records: false,
        encryption_supported: None,
        paca_supported: false,
        num_alt_so: 0,
        alt_service_options: Vec::new(),
        drs: None,
        uzid_incl: None,
        uzid: None,
        ch_ind: Some(1),
        sr_id: Some(sr_id),
        otd_supported: None,
        qpch_supported: None,
        enhanced_rc: None,
        for_rc_pref: None,
        rev_rc_pref: None,
        fch_supported: None,
        fch_capability: None,
        dcch_supported: None,
        dcch_capability: None,
        geo_loc_incl: None,
        geo_loc_type: None,
        rev_fch_gating_req: None,
        orig_reason: None,
        orig_count: None,
        sts_supported: None,
        cch_3x_supported: None,
        wll_incl: None,
        wll_device_type: None,
        global_emergency_call: None,
        ms_init_pos_loc_ind: None,
        qos_parms_incl: None,
        qos_parms_len: None,
        qos_parms: Vec::new(),
        enc_info_incl: None,
        sig_encrypt_sup: None,
        d_sig_encrypt_req: None,
        c_sig_encrypt_req: None,
        new_sseq_h: None,
        new_sseq_h_sig: None,
        ui_encrypt_req: None,
        ui_encrypt_sup: None,
        sync_id_incl: None,
        sync_id_len: None,
        sync_id: None,
        prev_sid_incl: None,
        prev_sid: None,
        prev_nid_incl: None,
        prev_nid: None,
        prev_pzid_incl: None,
        prev_pzid: None,
        so_bitmap_ind: None,
        so_group_num: None,
        so_bitmap: None,
        sdb_desired_only: None,
        alt_band_class_sup: None,
        msg_int_info_incl: None,
        sig_integrity_sup_incl: None,
        sig_integrity_sup: None,
        sig_integrity_req: None,
        new_key_id: None,
        new_sseq_h_incl: None,
        for_pdch_supported: None,
        for_pdch_capability: None,
        ext_ch_ind: None,
        sign_slot_cycle_index: None,
        add_serv_instance_incl: None,
        add_service_instances: Vec::new(),
        bcmc_incl: None,
        bcmc: None,
        rev_pdch_supported: None,
        rev_pdch_capability: None,
        band_sub_rep_incl: None,
        num_band_subclass: None,
        band_subclass_sup: Vec::new(),
        add_geo_loc_incl: None,
        add_geo_loc_type_len_ind: None,
        add_geo_loc_type: None,
        remaining_bits: 0,
    })
}

fn test_registration_event_with_esn(esn: u32, msg_seq: u8) -> AccessChannelEvent {
    let mut registration = test_access_event();
    registration.message_id = MessageId::Registration;
    registration.msg_type_name = "Registration Message".to_string();
    registration.msg_seq = Some(msg_seq);
    registration.ack_req = true;
    registration.esn = Some(esn);
    registration.imsi_m_s1 = Some(0x0091_989e);
    registration.imsi_m_s2 = Some(0x0326);
    registration.imsi_class = Some(0);
    registration.imsi_mcc = Some(310);
    registration.imsi_11_12 = Some(99);
    registration.mob_p_rev = Some(6);
    registration.slot_cycle_index = Some(2);
    registration.scm = Some(0x2a);
    registration
}

fn test_bsc_with_max_slot_cycle_index(max_slot_cycle_index: u8) -> Bsc {
    let mut overhead = OverheadParameters::default();
    overhead.max_slot_cycle_index = max_slot_cycle_index;

    Bsc::new(Config {
        pilot_offset: 0,
        overhead,
        paging: PagingChannelSettings::default(),
        traffic_assignment: TrafficAssignmentConfig::default(),
        access_event_rx: None,
        access_event_broadcast: None,
        sms_request_rx: None,
        sms_request_tx: None,
        data_request_rx: None,
        data_request_tx: None,
        power_override_request_rx: None,
        power_override_request_tx: None,
        mobiles_tx: None,
        paging_broadcast: None,
        traffic_broadcast: None,
        rx_reference_dbm: None,
        hlr_repo: None,
        msc_client: test_msc_client(),
        bts_client: None,
        traffic_retry: TrafficRetryConfig::default(),
        paging_retry: PagingRetryConfig::default(),
        voice_policy: test_voice_policy(),
        pcf_client: None,
        mobile_idle_timeout_s: 0,
        bts_paging_state: None,
        node_id: "bsc-test".to_string(),
        msc_voice_bearer: None,
    })
}

pub(super) async fn test_bsc_with_active_traffic_channel(
    service_option: u16,
) -> (Bsc, broadcast::Receiver<TrafficEvent>, u8) {
    use std::sync::Arc;
    let traffic_channels = Arc::new(Mutex::new(Vec::new()));
    let walsh_allocator = Arc::new(Mutex::new(WalshAllocator::new()));
    let traffic_rx_pool = Arc::new(Mutex::new(Vec::new()));
    let traffic_rx_removals = Arc::new(Mutex::new(Vec::new()));
    let (traffic_tx, traffic_rx) = broadcast::channel(32);

    let mut bsc = Bsc::new(Config {
        pilot_offset: 0,
        overhead: OverheadParameters::default(),
        paging: PagingChannelSettings::default(),
        traffic_assignment: TrafficAssignmentConfig::default(),
        access_event_rx: None,
        access_event_broadcast: None,
        sms_request_rx: None,
        sms_request_tx: None,
        data_request_rx: None,
        data_request_tx: None,
        power_override_request_rx: None,
        power_override_request_tx: None,
        mobiles_tx: None,
        paging_broadcast: None,
        traffic_broadcast: Some(traffic_tx),
        rx_reference_dbm: None,
        hlr_repo: None,
        msc_client: test_msc_client(),
        bts_client: Some(test_bts_client(
            walsh_allocator,
            traffic_channels,
            traffic_rx_pool,
            traffic_rx_removals,
        )),
        traffic_retry: TrafficRetryConfig::default(),
        paging_retry: PagingRetryConfig::default(),
        voice_policy: test_voice_policy(),
        pcf_client: None,
        mobile_idle_timeout_s: 0,
        bts_paging_state: None,
        node_id: "bsc-test".to_string(),
        msc_voice_bearer: None,
    });

    let esn = 0x1234_5678;
    let mut event = test_access_event();
    event.message_id = MessageId::Origination;
    event.msg_type_name = "Origination Message".to_string();
    event.msg_seq = Some(2);
    event.ack_req = true;
    event.esn = Some(esn);
    event.imsi_m_s1 = Some(0x0091_989e);
    event.imsi_m_s2 = Some(0x0326);
    event.imsi_class = Some(0);
    event.imsi_mcc = Some(310);
    event.imsi_11_12 = Some(99);
    event.mob_p_rev = Some(6);
    event.slot_cycle_index = Some(2);
    event.scm = Some(0x2a);
    event.service_option = Some(service_option);
    event.for_supported_rcs = vec![1, 2, 3, 4, 5];
    event.rev_supported_rcs = vec![1, 2, 3, 4];

    if is_packet_data_so(service_option) {
        bsc.inject_access_event(event).await;
    } else {
        event.message_id = MessageId::Registration;
        event.msg_type_name = "Registration Message".to_string();
        event.service_option = None;
        bsc.inject_access_event(event).await;
        let addr = bsc.mobiles[0].fwd_address.clone();
        bsc.allocate_voice_channel_for_mobile(
            &addr,
            service_option,
            2,
            None,
            None,
            None,
            None,
            None,
        )
        .await
        .expect("voice traffic channel should be assigned");
    }

    let walsh_code = bsc.mobiles[0]
        .traffic_channel
        .as_ref()
        .expect("traffic channel should be assigned")
        .walsh_code;
    bsc.mobiles[0].set_state(MsState::TrafficActive);
    if let Some(tc) = bsc.mobiles[0].traffic_channel.as_mut() {
        tc.channel_state = ChannelState::Active;
    }

    (bsc, traffic_rx, walsh_code)
}

#[tokio::test]
async fn mt_voice_on_existing_so33_uses_traffic_service_negotiation() {
    let (mut bsc, mut traffic_rx, walsh_code) = test_bsc_with_active_traffic_channel(33).await;
    bsc.mobiles[0].subscriber_id = Some(Uuid::new_v4());
    bsc.mobiles[0].phone_number = Some("5551234567".to_string());

    let addr = bsc.mobiles[0].fwd_address.clone();
    bsc.start_bs_voice_call_for_mobile(&addr, 3, Some("5550001111".to_string()), None, None, None);

    assert!(
        !bsc.paging.has_pending_voice_page(),
        "active traffic MT call setup must not page on the common channel"
    );
    let tc = bsc.mobiles[0]
        .traffic_channel
        .as_ref()
        .expect("SO33 traffic channel should remain assigned");
    assert_eq!(tc.walsh_code, walsh_code);
    assert_eq!(
        tc.service_option, 33,
        "SO33 packet connection stays primary"
    );
    assert_eq!(tc.voice_service_option, Some(3));
    assert_eq!(tc.voice_connection_ref, Some(0));
    assert_eq!(tc.voice_service_ref_id, Some(2));
    assert!(matches!(
        tc.channel_state,
        ChannelState::WaitingServiceResponse { .. }
    ));

    let event = traffic_rx
        .try_recv()
        .expect("Service Request should be sent on F-TCH");
    let service_request = event
        .service_request
        .expect("traffic event should carry Service Request params");
    let cfg = service_request
        .service_config
        .expect("Service Request propose should include service config");
    let service_options: Vec<u16> = cfg
        .connections
        .iter()
        .map(|connection| connection.service_option)
        .collect();
    assert_eq!(service_options, vec![3]);
    assert_eq!(cfg.connections[0].con_ref, 0);
    assert_eq!(cfg.connections[0].sr_id, 2);
}

#[tokio::test]
async fn assigned_channel_preamble_sends_bs_ack_even_if_mobile_state_drifted() {
    use std::sync::Arc;
    let traffic_channels = Arc::new(Mutex::new(Vec::new()));
    let walsh_allocator = Arc::new(Mutex::new(WalshAllocator::new()));
    let traffic_rx_pool = Arc::new(Mutex::new(Vec::new()));
    let traffic_rx_removals = Arc::new(Mutex::new(Vec::new()));
    let (traffic_tx, mut traffic_rx) = broadcast::channel(32);

    let mut bsc = Bsc::new(Config {
        pilot_offset: 0,
        overhead: OverheadParameters::default(),
        paging: PagingChannelSettings::default(),
        traffic_assignment: TrafficAssignmentConfig::default(),
        access_event_rx: None,
        access_event_broadcast: None,
        sms_request_rx: None,
        sms_request_tx: None,
        data_request_rx: None,
        data_request_tx: None,
        power_override_request_rx: None,
        power_override_request_tx: None,
        mobiles_tx: None,
        paging_broadcast: None,
        traffic_broadcast: Some(traffic_tx),
        rx_reference_dbm: None,
        hlr_repo: None,
        msc_client: test_msc_client(),
        bts_client: Some(test_bts_client(
            walsh_allocator,
            traffic_channels,
            traffic_rx_pool,
            traffic_rx_removals,
        )),
        traffic_retry: TrafficRetryConfig::default(),
        paging_retry: PagingRetryConfig::default(),
        voice_policy: test_voice_policy(),
        pcf_client: None,
        mobile_idle_timeout_s: 0,
        bts_paging_state: None,
        node_id: "bsc-test".to_string(),
        msc_voice_bearer: None,
    });

    let mut registration = test_access_event();
    registration.message_id = MessageId::Registration;
    registration.msg_type_name = "Registration Message".to_string();
    registration.msg_seq = Some(2);
    registration.ack_req = true;
    registration.esn = Some(0x1234_5678);
    registration.imsi_m_s1 = Some(0x0091_989e);
    registration.imsi_m_s2 = Some(0x0326);
    registration.imsi_class = Some(0);
    registration.imsi_mcc = Some(310);
    registration.imsi_11_12 = Some(99);
    registration.mob_p_rev = Some(6);
    registration.slot_cycle_index = Some(2);
    registration.scm = Some(0x2a);
    registration.for_supported_rcs = vec![1, 2, 3, 4, 5];
    registration.rev_supported_rcs = vec![1, 2, 3, 4];

    bsc.inject_access_event(registration).await;
    let addr = bsc.mobiles[0].fwd_address.clone();
    bsc.allocate_voice_channel_for_mobile(&addr, 3, 2, None, None, None, None, None)
        .await
        .expect("voice traffic channel should be assigned");

    let walsh_code = bsc.mobiles[0]
        .traffic_channel
        .as_ref()
        .expect("traffic channel should be assigned")
        .walsh_code;
    assert!(matches!(
        bsc.mobiles[0]
            .traffic_channel
            .as_ref()
            .unwrap()
            .channel_state,
        ChannelState::Assigned { .. }
    ));

    bsc.mobiles[0].set_state(MsState::Registered);

    let mut preamble = test_access_event();
    preamble.message_id = MessageId::GeneralExtension;
    preamble.msg_type_name = "Preamble".to_string();
    preamble.traffic_walsh_code = Some(walsh_code);
    preamble.is_preamble_only = true;

    bsc.inject_access_event(preamble).await;

    assert!(matches!(
        bsc.mobiles[0]
            .traffic_channel
            .as_ref()
            .unwrap()
            .channel_state,
        ChannelState::WaitingMsAck { .. }
    ));

    let ack_event = traffic_rx
        .try_recv()
        .expect("reverse preamble should send BS Ack on F-TCH");
    assert_eq!(ack_event.mcsb.message_id, MessageId::Order);
    assert_eq!(ack_event.mcsb.ack_seq, 0b111);
    assert!(ack_event.mcsb.ack_req);
}

#[tokio::test]
async fn registration_marks_mobile_active_for_idle_eviction() {
    let mut bsc = test_bsc_with_max_slot_cycle_index(2);
    bsc.config.mobile_idle_timeout_s = 1;

    bsc.inject_access_event(test_registration_event_with_esn(0x1234_5678, 3))
        .await;

    assert_eq!(bsc.mobiles.tracked_count(), 1);
    assert!(
        bsc.mobiles[0].last_access_activity.is_some(),
        "registration should stamp mobile activity"
    );
    assert_eq!(
        bsc.evict_idle_mobiles(),
        0,
        "fresh registration must survive the next eviction tick"
    );
    assert_eq!(bsc.mobiles.tracked_count(), 1);
}

#[tokio::test]
async fn first_origination_notifies_msc_with_lur() {
    // The mobile in this test never sends an explicit Registration Message;
    // it origins straight away. The BSC must still notify the MSC via a
    // CompleteLayer3Information carrying a LocationUpdatingRequest (no
    // call_id) so the MSC can update mobiles_seen and decide on welcome SMS.
    let (client, endpoint) = crate::a1_edge::InProcessMscClient::pair(32);
    let mut bsc = test_bsc_with_max_slot_cycle_index(2);
    bsc.config.msc_client = std::sync::Arc::new(client);
    bsc.a1.msc_client = bsc.config.msc_client.clone();

    let mut origination = test_registration_event_with_esn(0x1234_5678, 4);
    origination.message_id = MessageId::Origination;
    origination.msg_type_name = "Origination Message".to_string();
    bsc.inject_access_event(origination).await;

    // Drain A1 outbound until we see a CL3 LUR notification (no call_id).
    // The MO call path also sends a CL3 (with call_id) — accept either order.
    let mut found_lur = false;
    for _ in 0..4 {
        let msg = match tokio::time::timeout(
            std::time::Duration::from_millis(200),
            endpoint.recv_from_bsc(),
        )
        .await
        {
            Ok(Some(m)) => m,
            _ => break,
        };
        if msg.message_type() == cdma_ios::MessageType::CompleteLayer3Information
            && msg.call_id().is_none()
        {
            let decoded = msg.decode().expect("decode CLI3 envelope");
            let cli3 = cdma_ios::CompleteLayer3InformationMessage::decode(&decoded.payload)
                .expect("decode CLI3 body");
            cli3.layer3_information
                .decode_location_updating_request()
                .expect("CL3-without-call_id must carry a LocationUpdatingRequest");
            found_lur = true;
            break;
        }
    }
    assert!(
        found_lur,
        "first Origination from a new mobile must produce a LUR notification to the MSC"
    );
}

#[tokio::test]
async fn implicit_origination_marks_mobile_active_for_idle_eviction() {
    let mut bsc = test_bsc_with_max_slot_cycle_index(2);
    bsc.config.mobile_idle_timeout_s = 1;

    let mut origination = test_registration_event_with_esn(0x1234_5678, 4);
    origination.message_id = MessageId::Origination;
    origination.msg_type_name = "Origination Message".to_string();

    bsc.inject_access_event(origination).await;

    assert_eq!(bsc.mobiles.tracked_count(), 1);
    assert!(
        bsc.mobiles[0].last_access_activity.is_some(),
        "implicit registration should stamp mobile activity"
    );
    assert_eq!(
        bsc.evict_idle_mobiles(),
        0,
        "fresh implicit registration must survive the next eviction tick"
    );
    assert_eq!(bsc.mobiles.tracked_count(), 1);
}

#[tokio::test]
async fn access_order_refreshes_registered_mobile_activity() {
    let mut bsc = test_bsc_with_max_slot_cycle_index(2);
    bsc.config.mobile_idle_timeout_s = 10;

    bsc.inject_access_event(test_registration_event_with_esn(0x1234_5678, 3))
        .await;
    let stale_activity = Instant::now() - Duration::from_secs(20);
    bsc.mobiles[0].last_access_activity = Some(stale_activity);

    let mut order = test_registration_event_with_esn(0x1234_5678, 4);
    order.message_id = MessageId::Order;
    order.msg_type_name = "Order Message".to_string();
    bsc.inject_access_event(order).await;

    let refreshed = bsc.mobiles[0]
        .last_access_activity
        .expect("order should refresh mobile activity");
    assert!(
        refreshed > stale_activity,
        "access-channel activity should refresh stale mobile activity"
    );
    assert_eq!(
        bsc.evict_idle_mobiles(),
        0,
        "recent access activity must keep the mobile registered"
    );
    assert_eq!(bsc.mobiles.tracked_count(), 1);
}

#[tokio::test]
async fn ms_ack_order_gets_bsc_bs_ack_response() {
    let bts_client = Arc::new(CapturingBtsClient::default());
    let mut bsc = Bsc::new(Config {
        pilot_offset: 0,
        overhead: OverheadParameters::default(),
        paging: PagingChannelSettings::default(),
        traffic_assignment: TrafficAssignmentConfig::default(),
        access_event_rx: None,
        access_event_broadcast: None,
        sms_request_rx: None,
        sms_request_tx: None,
        data_request_rx: None,
        data_request_tx: None,
        power_override_request_rx: None,
        power_override_request_tx: None,
        mobiles_tx: None,
        paging_broadcast: None,
        traffic_broadcast: None,
        rx_reference_dbm: None,
        hlr_repo: None,
        msc_client: test_msc_client(),
        bts_client: Some(bts_client.clone() as Arc<dyn BtsControlClient>),
        traffic_retry: TrafficRetryConfig::default(),
        paging_retry: PagingRetryConfig::default(),
        voice_policy: test_voice_policy(),
        pcf_client: None,
        mobile_idle_timeout_s: 0,
        bts_paging_state: None,
        node_id: "bsc-test".to_string(),
        msc_voice_bearer: None,
    });

    let mut order = test_registration_event_with_esn(0x1234_5678, 5);
    order.message_id = MessageId::Order;
    order.msg_type_name = "Order Message".to_string();
    order.order_code = Some(0b010000);
    order.ack_req = true;
    order.valid_ack = true;
    order.ack_seq = Some(2);

    bsc.inject_access_event(order).await;

    let messages = bts_client.pch_messages.lock();
    assert_eq!(
        messages.len(),
        1,
        "MS Ack Order should produce one BSC-originated BS Ack Order"
    );
    let aim = messages[0]
        .air_interface_message
        .as_ref()
        .expect("BS Ack should carry an air-interface message");
    assert_eq!(aim.message_type, 0x07);
    assert_eq!(aim.message, vec![0x40, 0x00]);
    assert!(messages[0].layer2_ack_request_results.is_none());
    assert!(messages[0].abis_ack_notify.is_none());
}

#[test]
fn paging_slot_cycle_index_is_capped_to_bs_maximum() {
    let bsc = test_bsc_with_max_slot_cycle_index(0);
    assert_eq!(bsc.effective_slot_cycle_index(2), 0);

    let bsc = test_bsc_with_max_slot_cycle_index(3);
    assert_eq!(bsc.effective_slot_cycle_index(2), 2);
    assert_eq!(bsc.effective_slot_cycle_index(5), 3);
}

#[test]
fn assigned_paging_slot_chip_uses_capped_slot_cycle_index() {
    let bsc = test_bsc_with_max_slot_cycle_index(0);
    let chip_rate_hz = 1_228_800u64;
    let search_from = 1_792_422_000_000_000u64;
    let pgslot = 1769u16;

    let expected =
        cdma_common::paging::next_assigned_slot_chip(search_from, pgslot, 0, chip_rate_hz);
    let actual = bsc.assigned_paging_slot_chip(search_from, pgslot, 2, chip_rate_hz);

    assert_eq!(actual, expected);
}

#[derive(Clone)]
struct FakeSmscRepository {
    submissions: Arc<Mutex<HashMap<Uuid, SmsSubmission>>>,
    attempts: Arc<Mutex<HashMap<Uuid, SmsDeliveryAttempt>>>,
}

impl FakeSmscRepository {
    fn new(submissions: Vec<SmsSubmission>, attempts: Vec<SmsDeliveryAttempt>) -> Self {
        Self {
            submissions: Arc::new(Mutex::new(
                submissions.into_iter().map(|s| (s.sms_id, s)).collect(),
            )),
            attempts: Arc::new(Mutex::new(
                attempts
                    .into_iter()
                    .map(|a| (a.sms_delivery_attempt_id, a))
                    .collect(),
            )),
        }
    }

    fn submission(&self, sms_id: Uuid) -> SmsSubmission {
        self.submissions.lock().get(&sms_id).unwrap().clone()
    }

    fn attempt(&self, attempt_id: Uuid) -> SmsDeliveryAttempt {
        self.attempts.lock().get(&attempt_id).unwrap().clone()
    }

    fn set_state(&self, sms_id: Uuid, state: SmsState) {
        self.submissions.lock().get_mut(&sms_id).unwrap().state = state;
    }
}

#[tonic::async_trait]
impl SmscRepository for FakeSmscRepository {
    async fn create_submission(
        &self,
        originating_number: &str,
        destination: cdma_smsc::model::SmsDestination,
        text: &str,
        originating_subscriber_id: Option<Uuid>,
        destination_subscriber_id: Option<Uuid>,
    ) -> Result<SmsSubmission, String> {
        let (dest_number, dest_esn, dest_imsi) = match destination {
            cdma_smsc::model::SmsDestination::PhoneNumber(n) => (Some(n), None, None),
            cdma_smsc::model::SmsDestination::Esn(esn) => (None, Some(esn), None),
            cdma_smsc::model::SmsDestination::Imsi(imsi) => (None, None, Some(imsi)),
        };
        let now = chrono::Utc::now();
        let submission = SmsSubmission {
            sms_id: Uuid::new_v4(),
            originating_number: originating_number.to_string(),
            destination_number: dest_number,
            destination_esn: dest_esn,
            destination_imsi: dest_imsi,
            originating_subscriber_id,
            destination_subscriber_id,
            text: text.to_string(),

            state: SmsState::Accepted,
            failure_reason: None,
            created_at: now,
            updated_at: now,
        };
        self.submissions
            .lock()
            .insert(submission.sms_id, submission.clone());
        Ok(submission)
    }

    async fn create_or_get_recent_mo_submission(
        &self,
        originating_number: &str,
        destination_number: &str,
        text: &str,
        originating_subscriber_id: Option<Uuid>,
        destination_subscriber_id: Option<Uuid>,
        _fingerprint: &MoSmsFingerprint,
    ) -> Result<(SmsSubmission, bool), String> {
        let cutoff = chrono::Utc::now() - chrono::Duration::minutes(10);
        if let Some(existing) = self
            .submissions
            .lock()
            .values()
            .filter(|sub| {
                sub.originating_number == originating_number
                    && sub.destination_number.as_deref() == Some(destination_number)
                    && sub.originating_subscriber_id == originating_subscriber_id
                    && sub.text == text
                    && sub.created_at >= cutoff
            })
            .max_by_key(|sub| sub.created_at)
            .cloned()
        {
            return Ok((existing, false));
        }

        let now = chrono::Utc::now();
        let submission = SmsSubmission {
            sms_id: Uuid::new_v4(),
            originating_number: originating_number.to_string(),
            destination_number: Some(destination_number.to_string()),
            destination_esn: None,
            destination_imsi: None,
            originating_subscriber_id,
            destination_subscriber_id,
            text: text.to_string(),
            state: SmsState::Accepted,
            failure_reason: None,
            created_at: now,
            updated_at: now,
        };
        self.submissions
            .lock()
            .insert(submission.sms_id, submission.clone());
        Ok((submission, true))
    }

    async fn update_submission_state(
        &self,
        sms_id: Uuid,
        state: SmsState,
        failure_reason: Option<String>,
    ) -> Result<SmsSubmission, String> {
        let mut submissions = self.submissions.lock();
        let submission = submissions
            .get_mut(&sms_id)
            .ok_or_else(|| format!("submission {} not found", sms_id))?;
        submission.state = state;
        submission.failure_reason = failure_reason;
        submission.updated_at = chrono::Utc::now();
        Ok(submission.clone())
    }

    async fn get_submission(&self, sms_id: Uuid) -> Result<Option<SmsSubmission>, String> {
        Ok(self.submissions.lock().get(&sms_id).cloned())
    }

    async fn list_submissions(
        &self,
        limit: u32,
        offset: u32,
        destination_number: Option<&str>,
        destination_esn: Option<u32>,
        destination_imsi: Option<&str>,
        state: Option<&str>,
    ) -> Result<(Vec<SmsSubmission>, u32), String> {
        let mut submissions: Vec<_> = self
            .submissions
            .lock()
            .values()
            .filter(|sub| {
                destination_number.is_none_or(|dn| sub.destination_number.as_deref() == Some(dn))
                    && destination_esn.is_none_or(|esn| sub.destination_esn == Some(esn))
                    && destination_imsi
                        .is_none_or(|imsi| sub.destination_imsi.as_deref() == Some(imsi))
                    && state.is_none_or(|st| sub.state.as_str() == st)
            })
            .cloned()
            .collect();
        submissions.sort_by_key(|sub| std::cmp::Reverse(sub.created_at));
        let total = submissions.len() as u32;
        let start = offset as usize;
        let end = (start + limit as usize).min(submissions.len());
        let page = if start < submissions.len() {
            submissions[start..end].to_vec()
        } else {
            Vec::new()
        };
        Ok((page, total))
    }

    async fn create_delivery_attempt(
        &self,
        sms_id: Uuid,
        target_subscriber_id: Option<Uuid>,
    ) -> Result<SmsDeliveryAttempt, String> {
        let attempt_number = self
            .attempts
            .lock()
            .values()
            .filter(|a| a.sms_id == sms_id)
            .count() as u32
            + 1;
        let now = chrono::Utc::now();
        let attempt = SmsDeliveryAttempt {
            sms_delivery_attempt_id: Uuid::new_v4(),
            sms_id,
            attempt_number,
            state: DeliveryAttemptState::Queued,
            target_subscriber_id,
            failure_reason: None,
            requested_at: now,
            completed_at: None,
            created_at: now,
            updated_at: now,
        };
        self.attempts
            .lock()
            .insert(attempt.sms_delivery_attempt_id, attempt.clone());
        Ok(attempt)
    }

    async fn update_delivery_attempt_state(
        &self,
        attempt_id: Uuid,
        state: DeliveryAttemptState,
        failure_reason: Option<String>,
    ) -> Result<SmsDeliveryAttempt, String> {
        let mut attempts = self.attempts.lock();
        let attempt = attempts
            .get_mut(&attempt_id)
            .ok_or_else(|| format!("attempt {} not found", attempt_id))?;
        attempt.state = state;
        attempt.failure_reason = failure_reason;
        attempt.updated_at = chrono::Utc::now();
        Ok(attempt.clone())
    }

    async fn get_delivery_attempts(&self, sms_id: Uuid) -> Result<Vec<SmsDeliveryAttempt>, String> {
        Ok(self
            .attempts
            .lock()
            .values()
            .filter(|attempt| attempt.sms_id == sms_id)
            .cloned()
            .collect())
    }

    async fn update_destination_subscriber(
        &self,
        sms_id: Uuid,
        destination_subscriber_id: Uuid,
    ) -> Result<(), String> {
        let mut submissions = self.submissions.lock();
        let submission = submissions
            .get_mut(&sms_id)
            .ok_or_else(|| format!("submission {} not found", sms_id))?;
        submission.destination_subscriber_id = Some(destination_subscriber_id);
        submission.updated_at = chrono::Utc::now();
        Ok(())
    }
}

#[derive(Clone)]
struct FakeHlrRepository {
    subscriber: Subscriber,
    binding: RegistrationBinding,
    mobile_seen_result: cdma_hlr::MobileSeenUpsert,
}

#[tonic::async_trait]
impl HlrRepository for FakeHlrRepository {
    async fn upsert_subscriber(
        &self,
        _phone_number: &str,
        _display_name: &str,
        _status: &str,
    ) -> Result<Subscriber, String> {
        Ok(self.subscriber.clone())
    }

    async fn get_subscriber_by_phone_number(
        &self,
        phone_number: &str,
    ) -> Result<Option<Subscriber>, String> {
        Ok((self.subscriber.phone_number == phone_number).then_some(self.subscriber.clone()))
    }

    async fn get_subscriber_by_id(
        &self,
        subscriber_id: Uuid,
    ) -> Result<Option<Subscriber>, String> {
        Ok((self.subscriber.subscriber_id == subscriber_id).then_some(self.subscriber.clone()))
    }

    async fn update_subscriber(
        &self,
        _subscriber_id: Uuid,
        _phone_number: &str,
        _display_name: &str,
        _status: &str,
    ) -> Result<Option<Subscriber>, String> {
        Ok(Some(self.subscriber.clone()))
    }

    async fn list_subscribers(
        &self,
        _limit: u32,
        _offset: u32,
    ) -> Result<(Vec<Subscriber>, u32), String> {
        Ok((vec![self.subscriber.clone()], 1))
    }

    async fn delete_subscriber(&self, _subscriber_id: Uuid) -> Result<bool, String> {
        Ok(false)
    }

    async fn upsert_identity(
        &self,
        _subscriber_id: Uuid,
        _imsi: Option<&str>,
        _esn: Option<u32>,
    ) -> Result<SubscriberIdentity, String> {
        Err("not implemented in test".to_string())
    }

    async fn replace_primary_identity(
        &self,
        _subscriber_id: Uuid,
        _imsi: Option<&str>,
        _esn: Option<u32>,
    ) -> Result<SubscriberIdentity, String> {
        Err("not implemented in test".to_string())
    }

    async fn get_identities_for_subscriber(
        &self,
        _subscriber_id: Uuid,
    ) -> Result<Vec<SubscriberIdentity>, String> {
        Ok(Vec::new())
    }

    async fn resolve_by_identity(
        &self,
        _esn: Option<u32>,
        _imsi: Option<&str>,
    ) -> Result<Option<Subscriber>, String> {
        Ok(Some(self.subscriber.clone()))
    }

    async fn upsert_registration_binding(
        &self,
        binding: RegistrationBinding,
    ) -> Result<RegistrationBinding, String> {
        Ok(binding)
    }

    async fn get_registration_binding(
        &self,
        subscriber_id: Uuid,
    ) -> Result<Option<RegistrationBinding>, String> {
        Ok((self.binding.subscriber_id == subscriber_id).then_some(self.binding.clone()))
    }

    async fn upsert_mobile_seen(
        &self,
        _esn: Option<u32>,
        _imsi: Option<&str>,
        _mob_p_rev: Option<u8>,
    ) -> Result<cdma_hlr::MobileSeenUpsert, String> {
        Ok(self.mobile_seen_result.clone())
    }
}

fn test_sms_submission(sms_id: Uuid, state: SmsState, destination_number: &str) -> SmsSubmission {
    let now = chrono::Utc::now();
    SmsSubmission {
        sms_id,
        originating_number: "5551234".to_string(),
        destination_number: Some(destination_number.to_string()),
        destination_esn: None,
        destination_imsi: None,
        originating_subscriber_id: None,
        destination_subscriber_id: None,
        text: "pending sms".to_string(),

        state,
        failure_reason: None,
        created_at: now,
        updated_at: now,
    }
}

fn test_sms_submission_for_esn(
    sms_id: Uuid,
    state: SmsState,
    destination_esn: u32,
) -> SmsSubmission {
    let now = chrono::Utc::now();
    SmsSubmission {
        sms_id,
        originating_number: "5551234".to_string(),
        destination_number: None,
        destination_esn: Some(destination_esn),
        destination_imsi: None,
        originating_subscriber_id: None,
        destination_subscriber_id: None,
        text: "pending sms".to_string(),

        state,
        failure_reason: None,
        created_at: now,
        updated_at: now,
    }
}

fn test_sms_attempt(
    attempt_id: Uuid,
    sms_id: Uuid,
    target_subscriber_id: Uuid,
) -> SmsDeliveryAttempt {
    let now = chrono::Utc::now();
    SmsDeliveryAttempt {
        sms_delivery_attempt_id: attempt_id,
        sms_id,
        attempt_number: 1,
        state: DeliveryAttemptState::Paging,
        target_subscriber_id: Some(target_subscriber_id),
        failure_reason: None,
        requested_at: now,
        completed_at: None,
        created_at: now,
        updated_at: now,
    }
}

fn test_bsc_with_smsc(_repo: Arc<FakeSmscRepository>) -> Bsc {
    Bsc::new(Config {
        pilot_offset: 0,
        overhead: OverheadParameters::default(),
        paging: PagingChannelSettings::default(),
        traffic_assignment: TrafficAssignmentConfig::default(),
        access_event_rx: None,
        access_event_broadcast: None,
        sms_request_rx: None,
        sms_request_tx: None,
        data_request_rx: None,
        data_request_tx: None,
        power_override_request_rx: None,
        power_override_request_tx: None,
        mobiles_tx: None,
        paging_broadcast: None,
        traffic_broadcast: None,
        rx_reference_dbm: None,
        hlr_repo: None,
        msc_client: test_msc_client(),
        bts_client: None,
        traffic_retry: TrafficRetryConfig::default(),
        paging_retry: PagingRetryConfig::default(),
        voice_policy: test_voice_policy(),
        pcf_client: None,
        msc_voice_bearer: None,
        mobile_idle_timeout_s: 0,
        bts_paging_state: None,
        node_id: "bsc-test".to_string(),
    })
}

#[test]
fn dtmf_digits_decode_correctly() {
    // DTMF mode: 1–9 map to '1'–'9', 0xA='0', 0xB='*', 0xC='#'
    assert_eq!(format_dtmf_digits(&[5, 5, 5, 1, 2, 2], false), "555122");
    assert_eq!(format_dtmf_digits(&[0xA, 1, 2, 3], false), "0123");
    assert_eq!(format_dtmf_digits(&[9, 0xA, 0xB, 0xC], false), "90*#");
    assert_eq!(format_dtmf_digits(&[], false), "");
    // ASCII mode
    assert_eq!(
        format_dtmf_digits(&[b'5', b'5', b'5', b'1', b'2', b'2'], true),
        "555122"
    );
}

#[test]
fn traffic_assignment_policy_prefers_rc1_before_rc3() {
    let policy = TrafficAssignmentConfig::default();

    let selected = select_initial_traffic_rcs(&policy, &[1, 3], &[1, 3], None, None, 6);

    assert_eq!(selected, Some((1, 1)));
}

#[test]
fn traffic_assignment_policy_can_force_rc3_only() {
    let policy = TrafficAssignmentConfig {
        supported_for_rcs: vec![3],
        supported_rev_rcs: vec![3],
        preferred_pairs: vec![crate::config::RcPairConfig::new(3, 3)],
        idle_timeout_s: TrafficAssignmentConfig::default().idle_timeout_s,
        rev_fch_gating_mode: false,
    };

    let selected = select_initial_traffic_rcs(&policy, &[1, 3], &[1, 3], None, None, 6);

    assert_eq!(selected, Some((3, 3)));
}

#[test]
fn traffic_assignment_falls_back_to_preferred_pair_for_p_rev6_mobile() {
    // A mob_p_rev=6 mobile that only lists high-rate forward RCs and no
    // reverse RCs should still negotiate via the implicit baseline fallback.
    let policy = TrafficAssignmentConfig::default();

    let selected = select_initial_traffic_rcs(&policy, &[4, 5, 6, 9], &[], Some(5), Some(28), 6);

    // Policy default preferred pair is (1,1); mob_p_rev>=6 implies baseline support.
    assert_eq!(selected, Some((1, 1)));
}

// ---- extract_page_address: overhead resolution ----
//
// Per C.S0005-E 2.6.2.2.5: None in access event means "equals
// overhead".  Page address must store fully-resolved values so
// send_general_page() can pick the minimum GPM subclass at page time.

#[test]
fn extract_page_address_class0_full_imsi_stores_resolved() {
    // MS sends MCC=310, IMSI_11_12=0x7f explicitly.
    let mut event = test_access_event();
    event.esn = Some(0x1234_5678);
    event.imsi_m_s1 = Some(0x91989e);
    event.imsi_m_s2 = Some(0x326);
    event.imsi_class = Some(0);
    event.imsi_mcc = Some(310);
    event.imsi_11_12 = Some(0x7f);

    let page_addr = super::access::extract_page_address(&event, 310, 0x7f);
    match page_addr {
        Some(MsPageAddress::ImsiS {
            imsi_m_s1,
            imsi_m_s2,
            mcc,
            imsi_11_12,
        }) => {
            assert_eq!(imsi_m_s1, 0x91989e);
            assert_eq!(imsi_m_s2, 0x326);
            assert_eq!(mcc, Some(310));
            assert_eq!(imsi_11_12, Some(0x7f));
        }
        other => panic!("expected class-0 IMSI page address, got {:?}", other),
    }
}

#[test]
fn extract_page_address_class0_omitted_fields_resolve_to_overhead() {
    // Per C.S0004-E 2.1.1.3.1.3: home subscriber omits MCC and
    // IMSI_11_12 because they match overhead.  Page address must
    // store resolved overhead values (310, 15).
    let mut event = test_access_event();
    event.imsi_m_s1 = Some(0x91989e);
    event.imsi_m_s2 = Some(0x326);
    event.imsi_class = Some(0);
    event.imsi_mcc = None;
    event.imsi_11_12 = None;

    let page_addr = super::access::extract_page_address(&event, 310, 15);
    match page_addr {
        Some(MsPageAddress::ImsiS {
            mcc, imsi_11_12, ..
        }) => {
            assert_eq!(mcc, Some(310), "omitted MCC must resolve to overhead");
            assert_eq!(
                imsi_11_12,
                Some(15),
                "omitted IMSI_11_12 must resolve to overhead"
            );
        }
        other => panic!("expected resolved class-0 page address, got {:?}", other),
    }
}

#[test]
fn extract_page_address_class0_roamer_preserves_foreign_mcc() {
    // Per C.S0004-E 2.1.1.3.1.3 type '10': roaming MS sends
    // MCC=450 explicitly (differs from overhead MCC=310), omits
    // IMSI_11_12 (matches overhead wildcard 0x7f).
    let mut event = test_access_event();
    event.imsi_m_s1 = Some(0x91989e);
    event.imsi_m_s2 = Some(0x326);
    event.imsi_class = Some(0);
    event.imsi_mcc = Some(450);
    event.imsi_11_12 = None;

    let page_addr = super::access::extract_page_address(&event, 310, 0x7f);
    match page_addr {
        Some(MsPageAddress::ImsiS {
            mcc, imsi_11_12, ..
        }) => {
            assert_eq!(mcc, Some(450), "roamer MCC must be preserved as-is");
            assert_eq!(
                imsi_11_12,
                Some(0x7f),
                "omitted IMSI_11_12 resolves to overhead"
            );
        }
        other => panic!("expected roamer page address, got {:?}", other),
    }
}

#[test]
fn extract_page_address_class1_prefers_imsi_s_over_esn() {
    let mut event = test_access_event();
    event.esn = Some(0x1234_5678);
    event.imsi_m_s1 = Some(0x91989e);
    event.imsi_m_s2 = Some(0x326);
    event.imsi_class = Some(1);
    event.imsi_addr_num = Some(6);

    let page_addr = super::access::extract_page_address(&event, 310, 0x7f);
    assert!(matches!(
        page_addr,
        Some(MsPageAddress::ImsiS {
            imsi_m_s1: 0x91989e,
            imsi_m_s2: 0x326,
            ..
        })
    ));
}

#[test]
fn extract_page_address_class1_falls_back_to_esn() {
    let mut event = test_access_event();
    event.esn = Some(0x1234_5678);
    event.imsi_m_s1 = None;
    event.imsi_m_s2 = None;
    event.imsi_class = Some(1);

    let page_addr = super::access::extract_page_address(&event, 310, 0x7f);
    assert!(matches!(page_addr, Some(MsPageAddress::Esn(0x1234_5678))));
}

// ---- forward address: type selection ----

#[test]
fn fwd_address_type00_home_subscriber_wildcard_imsi_11_12() {
    // Home subscriber (MCC=310) on cell with MCC=310, IMSI_11_12=wildcard.
    // Both implied → type 00.
    let addr = select_imsi_class0_forward_address(0x91989e, 0x326, Some(310), Some(99), 310, 0x7f);
    assert_eq!(
        addr,
        MsAddress::ImsiClass0 {
            imsi_m_s1: 0x91989e,
            imsi_m_s2: 0x326,
            mcc: 310,
            imsi_11_12: 99,
        }
    );
}

#[test]
fn fwd_address_type01_imsi_11_12_differs() {
    // MCC matches overhead, IMSI_11_12 differs.
    let addr = select_imsi_class0_forward_address(0x91989e, 0x326, Some(310), Some(99), 310, 0x62);
    assert_eq!(
        addr,
        MsAddress::ImsiClass0 {
            imsi_m_s1: 0x91989e,
            imsi_m_s2: 0x326,
            mcc: 310,
            imsi_11_12: 99,
        }
    );
}

#[test]
fn fwd_address_type10_roamer_mcc_differs() {
    // Per C.S0004-E 2.1.1.3.1.3 IMSI_CLASS_0_TYPE='10':
    // Roaming MS (MCC_O=0x0d1) on cell with MCC=310, wildcard IMSI_11_12.
    // MCC differs, IMSI_11_12 resolved from mobile's explicit value.
    let addr =
        select_imsi_class0_forward_address(0x91989e, 0x326, Some(0x0d1), Some(0x63), 310, 0x7f);
    assert_eq!(
        addr,
        MsAddress::ImsiClass0 {
            imsi_m_s1: 0x91989e,
            imsi_m_s2: 0x326,
            mcc: 0x0d1,
            imsi_11_12: 0x63,
        }
    );
}

#[test]
fn fwd_address_type11_roamer_both_differ() {
    // Roaming MS: MCC and IMSI_11_12 both differ from overhead.
    let addr =
        select_imsi_class0_forward_address(0x91989e, 0x326, Some(0x0d1), Some(0x63), 310, 0x62);
    assert_eq!(
        addr,
        MsAddress::ImsiClass0 {
            imsi_m_s1: 0x91989e,
            imsi_m_s2: 0x326,
            mcc: 0x0d1,
            imsi_11_12: 0x63,
        }
    );
}

#[test]
fn fwd_address_home_subscriber_omits_both_resolved_from_overhead() {
    // Home subscriber omits MCC and IMSI_11_12 (None = matches overhead).
    // Resolved to overhead values.
    let addr = select_imsi_class0_forward_address(0x91989e, 0x326, None, None, 310, 0x7f);
    assert_eq!(
        addr,
        MsAddress::ImsiClass0 {
            imsi_m_s1: 0x91989e,
            imsi_m_s2: 0x326,
            mcc: 310,
            imsi_11_12: 0x7f,
        }
    );
}

#[test]
fn reverse_regular_msg_seq_tracker_detects_duplicates_and_advances_window() {
    let mut msg_seq_rcvd = [false; 8];

    assert!(!mark_reverse_regular_msg_seq_received(&mut msg_seq_rcvd, 3));
    assert!(msg_seq_rcvd[3]);
    assert!(!msg_seq_rcvd[7]);

    assert!(mark_reverse_regular_msg_seq_received(&mut msg_seq_rcvd, 3));

    assert!(!mark_reverse_regular_msg_seq_received(&mut msg_seq_rcvd, 7));
    assert!(msg_seq_rcvd[7]);
    assert!(!msg_seq_rcvd[3]);
}

#[tokio::test]
async fn duplicate_reverse_traffic_data_burst_is_acked_but_not_reprocessed() {
    use std::sync::{Arc, mpsc::channel};
    let traffic_channels = Arc::new(Mutex::new(Vec::new()));
    let walsh_allocator = Arc::new(Mutex::new(WalshAllocator::new()));
    let traffic_rx_pool = Arc::new(Mutex::new(Vec::new()));
    let traffic_rx_removals = Arc::new(Mutex::new(Vec::new()));
    let (traffic_tx, mut traffic_rx) = broadcast::channel(32);

    let mut bsc = Bsc::new(Config {
        pilot_offset: 0,
        overhead: OverheadParameters::default(),
        paging: PagingChannelSettings::default(),
        traffic_assignment: TrafficAssignmentConfig::default(),
        access_event_rx: None,
        access_event_broadcast: None,
        sms_request_rx: None,
        sms_request_tx: None,
        data_request_rx: None,
        data_request_tx: None,
        power_override_request_rx: None,
        power_override_request_tx: None,
        mobiles_tx: None,
        paging_broadcast: None,
        traffic_broadcast: Some(traffic_tx),
        rx_reference_dbm: None,
        hlr_repo: None,
        msc_client: test_msc_client(),
        bts_client: Some(test_bts_client(
            walsh_allocator,
            traffic_channels,
            traffic_rx_pool,
            traffic_rx_removals,
        )),
        traffic_retry: TrafficRetryConfig::default(),
        paging_retry: PagingRetryConfig::default(),
        voice_policy: test_voice_policy(),
        pcf_client: None,
        mobile_idle_timeout_s: 0,
        bts_paging_state: None,
        node_id: "bsc-test".to_string(),
        msc_voice_bearer: None,
    });

    let esn = 0x1234_5678;
    let mut origination = test_access_event();
    origination.message_id = MessageId::Origination;
    origination.msg_type_name = "Origination Message".to_string();
    origination.msg_seq = Some(2);
    origination.ack_req = true;
    origination.esn = Some(esn);
    origination.imsi_m_s1 = Some(0x0091_989e);
    origination.imsi_m_s2 = Some(0x0326);
    origination.imsi_class = Some(0);
    origination.imsi_mcc = Some(310);
    origination.imsi_11_12 = Some(99);
    origination.mob_p_rev = Some(6);
    origination.slot_cycle_index = Some(2);
    origination.scm = Some(0x2a);
    origination.service_option = Some(6);
    origination.for_supported_rcs = vec![1, 2, 3, 4, 5];
    origination.rev_supported_rcs = vec![1, 2, 3, 4];

    bsc.inject_access_event(origination).await;

    let walsh_code = bsc.mobiles[0]
        .traffic_channel
        .as_ref()
        .expect("traffic channel should be assigned")
        .walsh_code;
    bsc.mobiles[0].set_state(MsState::TrafficActive);
    bsc.mobiles[0].phone_number = Some("5551234567".to_string());
    if let Some(tc) = bsc.mobiles[0].traffic_channel.as_mut() {
        tc.channel_state = ChannelState::Active;
    }

    while traffic_rx.try_recv().is_ok() {}

    let mut data_burst = test_access_event();
    data_burst.message_id = MessageId::DataBurst;
    data_burst.msg_type_name = "Data Burst Message".to_string();
    data_burst.msg_seq = Some(1);
    data_burst.ack_seq = Some(3);
    data_burst.ack_req = true;
    data_burst.valid_ack = true;
    data_burst.traffic_walsh_code = Some(walsh_code);
    data_burst.burst_type = Some(3);
    data_burst.data_burst_num_msgs = Some(1);
    data_burst.data_burst_msg_number = Some(1);
    data_burst.data_burst_fields = Some(vec![
        0x00, 0x00, 0x02, 0x10, 0x02, 0x04, 0x05, 0x01, 0x95, 0x54, 0x48, 0x80, 0x06, 0x01, 0x84,
        0x08, 0x18, 0x00, 0x03, 0x20, 0x0A, 0x90, 0x01, 0x05, 0x10, 0x1C, 0x8D, 0x3A, 0x40, 0x0A,
        0x01, 0x40, 0x0E, 0x07, 0x05, 0x48, 0xBB, 0x49, 0xB1, 0x34, 0x80,
    ]);

    bsc.inject_access_event(data_burst.clone()).await;
    bsc.inject_access_event(data_burst).await;

    let tc = bsc.mobiles[0]
        .traffic_channel
        .as_ref()
        .expect("traffic channel should remain assigned");
    assert!(tc.reverse_regular_msg_seq_rcvd_ack[1]);
    assert!(!tc.reverse_regular_msg_seq_rcvd_ack[5]);

    let mut order_count = 0;
    let mut data_burst_count = 0;
    while let Ok(event) = traffic_rx.try_recv() {
        match event.mcsb.message_id {
            MessageId::Order => order_count += 1,
            MessageId::DataBurst => data_burst_count += 1,
            _ => {}
        }
    }

    assert_eq!(order_count, 2, "expected both reverse PDUs to be ACKed");
    // 1 Cause Code Data Burst from the first (non-duplicate) reverse
    // Data Burst. MO SMS is now routed to the MSC via ADDS Transfer
    // (not delivered back via traffic channel), so no forward SMS
    // DataBurst is generated here. The duplicate reverse Data Burst
    // does NOT generate a second Cause Code.
    assert_eq!(
        data_burst_count, 1,
        "expected one Cause Code only; MO SMS forwarded to MSC via ADDS Transfer"
    );
}

#[tokio::test]
async fn traffic_mo_sms_without_subscriber_sends_temporary_cause_code() {
    let (mut bsc, mut traffic_rx, walsh_code) = test_bsc_with_active_traffic_channel(6).await;
    bsc.mobiles[0].phone_number = None;
    bsc.mobiles[0].subscriber_id = None;

    while traffic_rx.try_recv().is_ok() {}

    let mut data_burst = test_access_event();
    data_burst.message_id = MessageId::DataBurst;
    data_burst.msg_type_name = "Data Burst Message".to_string();
    data_burst.msg_seq = Some(1);
    data_burst.ack_seq = Some(3);
    data_burst.ack_req = true;
    data_burst.valid_ack = true;
    data_burst.traffic_walsh_code = Some(walsh_code);
    data_burst.burst_type = Some(3);
    data_burst.data_burst_num_msgs = Some(1);
    data_burst.data_burst_msg_number = Some(1);
    data_burst.data_burst_fields = Some(vec![
        0x00, 0x00, 0x02, 0x10, 0x02, 0x04, 0x05, 0x01, 0x95, 0x54, 0x48, 0x80, 0x06, 0x01, 0x84,
        0x08, 0x18, 0x00, 0x03, 0x20, 0x0A, 0x90, 0x01, 0x05, 0x10, 0x1C, 0x8D, 0x3A, 0x40, 0x0A,
        0x01, 0x40, 0x0E, 0x07, 0x05, 0x48, 0xBB, 0x49, 0xB1, 0x34, 0x80,
    ]);

    bsc.inject_access_event(data_burst).await;

    let mut saw_bs_ack = false;
    let mut cause_fields = None;
    while let Ok(event) = traffic_rx.try_recv() {
        match event.mcsb.message_id {
            MessageId::Order => saw_bs_ack = true,
            MessageId::DataBurst => {
                cause_fields = event.data_burst.as_ref().map(|db| db.fields.clone());
            }
            _ => {}
        }
    }

    assert!(saw_bs_ack, "expected L2 BS Ack for reverse DBM");
    let fields = cause_fields.expect("expected SMS Cause Code DBM");
    assert_eq!(fields[0], 0x02, "SMS Acknowledge message type");
    assert_eq!(fields[1], 0x07, "Cause Codes parameter");
    assert_eq!(fields[2], 0x02, "temporary error includes CAUSE_CODE octet");
    assert_eq!(fields[3] & 0x03, 0b10, "temporary ERROR_CLASS");
    assert_eq!(fields[4], 0x03, "SMS_CauseCode NetworkFailure");
}

#[tokio::test]
async fn pmrm_ack_of_bs_ack_advances_waiting_ms_ack() {
    let (mut bsc, _traffic_rx, walsh_code) = test_bsc_with_active_traffic_channel(33).await;

    bsc.send_traffic_bs_ack(walsh_code, 0b111)
        .expect("BS Ack should send via bearer");

    if let Some(tc) = bsc.mobiles[0].traffic_channel.as_mut() {
        tc.channel_state = ChannelState::WaitingMsAck {
            bs_ack_sent_at: Instant::now(),
        };
    }

    let mut pmrm = test_access_event();
    pmrm.message_id = MessageId::PowerMeasurementReport;
    pmrm.msg_type_name = "Power Measurement Report Message".to_string();
    pmrm.msg_seq = Some(3);
    pmrm.ack_seq = Some(0);
    pmrm.ack_req = false;
    pmrm.traffic_walsh_code = Some(walsh_code);

    bsc.inject_access_event(pmrm).await;
    assert!(
        bsc.mobiles[0]
            .traffic_channel
            .as_ref()
            .is_some_and(|tc| matches!(tc.channel_state, ChannelState::ServiceConnecting { .. })),
        "matching ACK_SEQ should move WaitingMsAck into ServiceConnecting"
    );
}

#[tokio::test]
async fn service_connect_completion_ack_clears_pending_traffic_retry() {
    use std::sync::{Arc, mpsc::channel};
    let traffic_channels = Arc::new(Mutex::new(Vec::new()));
    let walsh_allocator = Arc::new(Mutex::new(WalshAllocator::new()));
    let traffic_rx_pool = Arc::new(Mutex::new(Vec::new()));
    let traffic_rx_removals = Arc::new(Mutex::new(Vec::new()));

    let mut bsc = Bsc::new(Config {
        pilot_offset: 0,
        overhead: OverheadParameters::default(),
        paging: PagingChannelSettings::default(),
        traffic_assignment: TrafficAssignmentConfig::default(),
        access_event_rx: None,
        access_event_broadcast: None,
        sms_request_rx: None,
        sms_request_tx: None,
        data_request_rx: None,
        data_request_tx: None,
        power_override_request_rx: None,
        power_override_request_tx: None,
        mobiles_tx: None,
        paging_broadcast: None,
        traffic_broadcast: None,
        rx_reference_dbm: None,
        hlr_repo: None,
        msc_client: test_msc_client(),
        bts_client: Some(test_bts_client(
            walsh_allocator,
            traffic_channels,
            traffic_rx_pool,
            traffic_rx_removals,
        )),
        traffic_retry: TrafficRetryConfig::default(),
        paging_retry: PagingRetryConfig::default(),
        voice_policy: test_voice_policy(),
        pcf_client: None,
        mobile_idle_timeout_s: 0,
        bts_paging_state: None,
        node_id: "bsc-test".to_string(),
        msc_voice_bearer: None,
    });

    let esn = 0x1234_5678;
    let mut origination = test_access_event();
    origination.message_id = MessageId::Origination;
    origination.msg_type_name = "Origination Message".to_string();
    origination.msg_seq = Some(2);
    origination.ack_req = true;
    origination.esn = Some(esn);
    origination.imsi_m_s1 = Some(0x0091_989e);
    origination.imsi_m_s2 = Some(0x0326);
    origination.imsi_class = Some(0);
    origination.imsi_mcc = Some(310);
    origination.imsi_11_12 = Some(99);
    origination.mob_p_rev = Some(6);
    origination.slot_cycle_index = Some(2);
    origination.scm = Some(0x2a);
    origination.service_option = Some(6);
    origination.for_supported_rcs = vec![1, 2, 3, 4, 5];
    origination.rev_supported_rcs = vec![1, 2, 3, 4];

    bsc.inject_access_event(origination).await;

    let walsh_code = bsc.mobiles[0]
        .traffic_channel
        .as_ref()
        .expect("traffic channel should be assigned")
        .walsh_code;
    bsc.mobiles[0].set_state(MsState::TrafficActive);
    bsc.mobiles[0].phone_number = Some("5551234567".to_string());
    if let Some(tc) = bsc.mobiles[0].traffic_channel.as_mut() {
        tc.channel_state = ChannelState::ServiceConnecting {
            sc_sent_at: Instant::now(),
        };
    }

    bsc.send_service_connect(walsh_code, 0)
        .expect("Service Connect should produce FchForward");

    let mut completion = test_access_event();
    completion.message_id = MessageId::ServiceConnectCompletion;
    completion.msg_type_name = "Service Connect Completion Message".to_string();
    completion.msg_seq = Some(0);
    completion.valid_ack = true;
    completion.ack_seq = Some(0);
    completion.traffic_walsh_code = Some(walsh_code);

    bsc.inject_access_event(completion).await;

    assert!(
        bsc.mobiles[0]
            .traffic_channel
            .as_ref()
            .is_some_and(|tc| tc.channel_state.is_service_negotiated()),
        "Service Connect Completion should still negotiate service"
    );
}

#[tokio::test]
async fn service_connect_completion_ack_seq_7_clears_pending_traffic_retry() {
    use std::sync::{Arc, mpsc::channel};
    let traffic_channels = Arc::new(Mutex::new(Vec::new()));
    let walsh_allocator = Arc::new(Mutex::new(WalshAllocator::new()));
    let traffic_rx_pool = Arc::new(Mutex::new(Vec::new()));
    let traffic_rx_removals = Arc::new(Mutex::new(Vec::new()));

    let mut bsc = Bsc::new(Config {
        pilot_offset: 0,
        overhead: OverheadParameters::default(),
        paging: PagingChannelSettings::default(),
        traffic_assignment: TrafficAssignmentConfig::default(),
        access_event_rx: None,
        access_event_broadcast: None,
        sms_request_rx: None,
        sms_request_tx: None,
        data_request_rx: None,
        data_request_tx: None,
        power_override_request_rx: None,
        power_override_request_tx: None,
        mobiles_tx: None,
        paging_broadcast: None,
        traffic_broadcast: None,
        rx_reference_dbm: None,
        hlr_repo: None,
        msc_client: test_msc_client(),
        bts_client: Some(test_bts_client(
            walsh_allocator,
            traffic_channels,
            traffic_rx_pool,
            traffic_rx_removals,
        )),
        traffic_retry: TrafficRetryConfig::default(),
        paging_retry: PagingRetryConfig::default(),
        voice_policy: test_voice_policy(),
        pcf_client: None,
        mobile_idle_timeout_s: 0,
        bts_paging_state: None,
        node_id: "bsc-test".to_string(),
        msc_voice_bearer: None,
    });

    let esn = 0x1234_5678;
    let mut origination = test_access_event();
    origination.message_id = MessageId::Origination;
    origination.msg_type_name = "Origination Message".to_string();
    origination.msg_seq = Some(2);
    origination.ack_req = true;
    origination.esn = Some(esn);
    origination.imsi_m_s1 = Some(0x0091_989e);
    origination.imsi_m_s2 = Some(0x0326);
    origination.imsi_class = Some(0);
    origination.imsi_mcc = Some(310);
    origination.imsi_11_12 = Some(99);
    origination.mob_p_rev = Some(6);
    origination.slot_cycle_index = Some(2);
    origination.scm = Some(0x2a);
    origination.service_option = Some(6);
    origination.for_supported_rcs = vec![1, 2, 3, 4, 5];
    origination.rev_supported_rcs = vec![1, 2, 3, 4];

    bsc.inject_access_event(origination).await;

    let walsh_code = bsc.mobiles[0]
        .traffic_channel
        .as_ref()
        .expect("traffic channel should be assigned")
        .walsh_code;
    bsc.mobiles[0].set_state(MsState::TrafficActive);
    bsc.mobiles[0].phone_number = Some("5551234567".to_string());
    if let Some(tc) = bsc.mobiles[0].traffic_channel.as_mut() {
        tc.channel_state = ChannelState::ServiceConnecting {
            sc_sent_at: Instant::now(),
        };
    }

    bsc.send_service_connect(walsh_code, 0)
        .expect("Service Connect should produce FchForward");

    let mut completion = test_access_event();
    completion.message_id = MessageId::ServiceConnectCompletion;
    completion.msg_type_name = "Service Connect Completion Message".to_string();
    completion.msg_seq = Some(0);
    completion.valid_ack = false;
    completion.ack_seq = Some(0b111);
    completion.traffic_walsh_code = Some(walsh_code);

    bsc.inject_access_event(completion).await;

    assert!(
        bsc.mobiles[0]
            .traffic_channel
            .as_ref()
            .is_some_and(|tc| tc.channel_state.is_service_negotiated()),
        "Service Connect Completion should still negotiate service"
    );
}

#[tokio::test]
async fn voice_service_connect_retry_targets_voice_channel() {
    use std::sync::{Arc, mpsc::channel};
    let traffic_channels = Arc::new(Mutex::new(Vec::new()));
    let walsh_allocator = Arc::new(Mutex::new(WalshAllocator::new()));
    let traffic_rx_pool = Arc::new(Mutex::new(Vec::new()));
    let traffic_rx_removals = Arc::new(Mutex::new(Vec::new()));

    let mut bsc = Bsc::new(Config {
        pilot_offset: 0,
        overhead: OverheadParameters::default(),
        paging: PagingChannelSettings::default(),
        traffic_assignment: TrafficAssignmentConfig::default(),
        access_event_rx: None,
        access_event_broadcast: None,
        sms_request_rx: None,
        sms_request_tx: None,
        data_request_rx: None,
        data_request_tx: None,
        power_override_request_rx: None,
        power_override_request_tx: None,
        mobiles_tx: None,
        paging_broadcast: None,
        traffic_broadcast: None,
        rx_reference_dbm: None,
        hlr_repo: None,
        msc_client: test_msc_client(),
        bts_client: Some(test_bts_client(
            walsh_allocator,
            traffic_channels,
            traffic_rx_pool,
            traffic_rx_removals,
        )),
        traffic_retry: TrafficRetryConfig::default(),
        paging_retry: PagingRetryConfig::default(),
        voice_policy: test_voice_policy(),
        pcf_client: None,
        mobile_idle_timeout_s: 0,
        bts_paging_state: None,
        node_id: "bsc-test".to_string(),
        msc_voice_bearer: None,
    });

    let mut registration = test_access_event();
    registration.message_id = MessageId::Registration;
    registration.msg_type_name = "Registration Message".to_string();
    registration.msg_seq = Some(1);
    registration.ack_req = true;
    registration.esn = Some(0x1234_5678);
    registration.imsi_m_s1 = Some(0x0091_989e);
    registration.imsi_m_s2 = Some(0x0326);
    registration.imsi_class = Some(0);
    registration.imsi_mcc = Some(310);
    registration.imsi_11_12 = Some(99);
    registration.mob_p_rev = Some(6);
    registration.slot_cycle_index = Some(2);
    registration.scm = Some(0x2a);
    registration.for_supported_rcs = vec![1, 2, 3, 4, 5];
    registration.rev_supported_rcs = vec![1, 2, 3, 4];

    bsc.inject_access_event(registration).await;
    bsc.mobiles[0].phone_number = Some("5551234567".to_string());

    let addr = bsc.mobiles[0].fwd_address.clone();
    bsc.allocate_voice_channel_for_mobile(&addr, 3, 1, None, None, None, None, None)
        .await
        .expect("voice traffic channel should be assigned");
    bsc.mobiles[0].set_state(MsState::TrafficActive);

    let walsh_code = bsc.mobiles[0]
        .traffic_channel
        .as_ref()
        .expect("voice channel should be assigned")
        .walsh_code;

    bsc.send_service_connect(walsh_code, 0)
        .expect("Service Connect should produce FchForward");

    assert!(
        bsc.mobiles[0].traffic_channel.is_some(),
        "voice traffic channel should remain assigned"
    );
}

#[tokio::test]
async fn origination_retry_reuses_pending_traffic_channel() {
    use std::sync::{Arc, mpsc::channel};
    let traffic_channels = Arc::new(Mutex::new(Vec::new()));
    let walsh_allocator = Arc::new(Mutex::new(WalshAllocator::new()));
    let traffic_rx_pool = Arc::new(Mutex::new(Vec::new()));
    let traffic_rx_removals = Arc::new(Mutex::new(Vec::new()));
    let bts_client = test_capturing_bts_client(
        walsh_allocator,
        traffic_channels.clone(),
        traffic_rx_pool.clone(),
        traffic_rx_removals,
    );

    let mut bsc = Bsc::new(Config {
        pilot_offset: 0,
        overhead: OverheadParameters::default(),
        paging: PagingChannelSettings::default(),
        traffic_assignment: TrafficAssignmentConfig::default(),
        access_event_rx: None,
        access_event_broadcast: None,
        sms_request_rx: None,
        sms_request_tx: None,
        data_request_rx: None,
        data_request_tx: None,
        power_override_request_rx: None,
        power_override_request_tx: None,
        mobiles_tx: None,
        paging_broadcast: None,
        traffic_broadcast: None,
        rx_reference_dbm: None,
        hlr_repo: None,
        msc_client: test_msc_client(),
        bts_client: Some(bts_client.clone() as Arc<dyn BtsControlClient>),
        traffic_retry: TrafficRetryConfig::default(),
        paging_retry: PagingRetryConfig::default(),
        voice_policy: test_voice_policy(),
        pcf_client: None,
        mobile_idle_timeout_s: 0,
        bts_paging_state: None,
        node_id: "bsc-test".to_string(),
        msc_voice_bearer: None,
    });

    let esn = 0x1234_5678;
    let mut origination = test_access_event();
    origination.message_id = MessageId::Origination;
    origination.msg_type_name = "Origination Message".to_string();
    origination.msg_seq = Some(2);
    origination.ack_req = true;
    origination.esn = Some(esn);
    origination.imsi_m_s1 = Some(0x0091_989e);
    origination.imsi_m_s2 = Some(0x0326);
    origination.imsi_class = Some(0);
    origination.imsi_mcc = Some(310);
    origination.imsi_11_12 = Some(99);
    origination.mob_p_rev = Some(6);
    origination.slot_cycle_index = Some(2);
    origination.scm = Some(0x2a);
    origination.service_option = Some(6);
    origination.for_supported_rcs = vec![1, 2, 3, 4, 5];
    origination.rev_supported_rcs = vec![1, 2, 3, 4];

    bsc.inject_access_event(origination.clone()).await;

    let first_walsh = bsc.mobiles[0]
        .traffic_channel
        .as_ref()
        .expect("traffic channel should be assigned")
        .walsh_code;
    assert_eq!(bsc.mobiles[0].traffic_channel.as_ref().unwrap().for_rc, 1);
    assert_eq!(bsc.mobiles[0].traffic_channel.as_ref().unwrap().rev_rc, 1);
    assert_eq!(bsc.mobiles[0].state, MsState::TrafficAssigning);
    assert_eq!(traffic_channels.lock().len(), 1);
    assert_eq!(traffic_rx_pool.lock().len(), 1);
    assert_eq!(traffic_rx_pool.lock()[0].walsh_code, first_walsh);

    let first_assigned_at = match bsc.mobiles[0]
        .traffic_channel
        .as_ref()
        .unwrap()
        .channel_state
    {
        ChannelState::Assigned { assigned_at } => assigned_at,
        _ => panic!("expected Assigned state"),
    };

    let mut retry = origination.clone();
    retry.msg_seq = Some(3);
    bsc.inject_access_event(retry).await;

    let tc = bsc.mobiles[0]
        .traffic_channel
        .as_ref()
        .expect("traffic channel should remain assigned");
    assert_eq!(bsc.mobiles[0].state, MsState::TrafficAssigning);
    assert_eq!(tc.walsh_code, first_walsh);
    match tc.channel_state {
        ChannelState::Assigned { assigned_at } => assert!(assigned_at >= first_assigned_at),
        _ => panic!("expected Assigned state"),
    }
    assert_eq!(traffic_channels.lock().len(), 1);
    assert_eq!(traffic_rx_pool.lock().len(), 1);
    assert_eq!(traffic_rx_pool.lock()[0].walsh_code, first_walsh);

    let pch_types = pch_message_types(&bts_client);
    assert_eq!(
        pch_types
            .iter()
            .filter(|&&msg| msg == MessageId::ExtChannelAssignment)
            .count(),
        2
    );
    let messages = bts_client.pch_messages.lock();
    let assignment_ack_req_count = messages
        .iter()
        .filter(|message| {
            message.air_interface_message.as_ref().and_then(|aim| {
                MessageId::from_wire(
                    cdma_common::lac::message_types::WireChannel::ForwardCommon,
                    aim.message_type,
                )
            }) == Some(MessageId::ExtChannelAssignment)
                && message.layer2_ack_request_results.is_some()
        })
        .count();
    assert_eq!(assignment_ack_req_count, 2);
}

#[tokio::test]
async fn legacy_origination_uses_cam_for_rc1_assignment() {
    use std::sync::{Arc, mpsc::channel};
    let traffic_channels = Arc::new(Mutex::new(Vec::new()));
    let walsh_allocator = Arc::new(Mutex::new(WalshAllocator::new()));
    let traffic_rx_pool = Arc::new(Mutex::new(Vec::new()));
    let traffic_rx_removals = Arc::new(Mutex::new(Vec::new()));
    let bts_client = test_capturing_bts_client(
        walsh_allocator,
        traffic_channels,
        traffic_rx_pool.clone(),
        traffic_rx_removals,
    );

    let mut bsc = Bsc::new(Config {
        pilot_offset: 0,
        overhead: OverheadParameters::default(),
        paging: PagingChannelSettings::default(),
        traffic_assignment: TrafficAssignmentConfig::default(),
        access_event_rx: None,
        access_event_broadcast: None,
        sms_request_rx: None,
        sms_request_tx: None,
        data_request_rx: None,
        data_request_tx: None,
        power_override_request_rx: None,
        power_override_request_tx: None,
        mobiles_tx: None,
        paging_broadcast: None,
        traffic_broadcast: None,
        rx_reference_dbm: None,
        hlr_repo: None,
        msc_client: test_msc_client(),
        bts_client: Some(bts_client.clone() as Arc<dyn BtsControlClient>),
        traffic_retry: TrafficRetryConfig::default(),
        paging_retry: PagingRetryConfig::default(),
        voice_policy: test_voice_policy(),
        pcf_client: None,
        mobile_idle_timeout_s: 0,
        bts_paging_state: None,
        node_id: "bsc-test".to_string(),
        msc_voice_bearer: None,
    });

    let mut origination = test_access_event();
    origination.message_id = MessageId::Origination;
    origination.msg_type_name = "Origination Message".to_string();
    origination.msg_seq = Some(2);
    origination.ack_req = true;
    origination.esn = Some(0x1234_5678);
    origination.imsi_m_s1 = Some(0x0091_989e);
    origination.imsi_m_s2 = Some(0x0326);
    origination.imsi_class = Some(0);
    origination.imsi_mcc = Some(310);
    origination.imsi_11_12 = Some(99);
    origination.mob_p_rev = Some(5);
    origination.slot_cycle_index = Some(2);
    origination.scm = Some(0x2a);
    origination.service_option = Some(6);
    origination.for_supported_rcs = vec![1];
    origination.rev_supported_rcs = vec![1];

    bsc.inject_access_event(origination).await;

    let tc = bsc.mobiles[0]
        .traffic_channel
        .as_ref()
        .expect("traffic channel should be assigned");
    assert_eq!((tc.for_rc, tc.rev_rc), (1, 1));
    assert_eq!(tc.rc_label, "RC1");
    assert_eq!(traffic_rx_pool.lock()[0].assigned_rev_rc, 1);

    let pch_types = pch_message_types(&bts_client);
    assert!(pch_types.contains(&MessageId::ChannelAssignment));
    assert!(!pch_types.contains(&MessageId::ExtChannelAssignment));
}

#[tokio::test]
async fn legacy_origination_fails_before_cam_when_selected_rc_is_not_rc1() {
    use std::sync::{Arc, mpsc::channel};
    let traffic_channels = Arc::new(Mutex::new(Vec::new()));
    let walsh_allocator = Arc::new(Mutex::new(WalshAllocator::new()));
    let traffic_rx_pool = Arc::new(Mutex::new(Vec::new()));
    let traffic_rx_removals = Arc::new(Mutex::new(Vec::new()));
    let bts_client = test_capturing_bts_client(
        walsh_allocator,
        traffic_channels.clone(),
        traffic_rx_pool.clone(),
        traffic_rx_removals,
    );

    let mut bsc = Bsc::new(Config {
        pilot_offset: 0,
        overhead: OverheadParameters::default(),
        paging: PagingChannelSettings::default(),
        traffic_assignment: TrafficAssignmentConfig {
            supported_for_rcs: vec![3],
            supported_rev_rcs: vec![3],
            preferred_pairs: vec![crate::config::RcPairConfig::new(3, 3)],
            idle_timeout_s: TrafficAssignmentConfig::default().idle_timeout_s,
            rev_fch_gating_mode: false,
        },
        access_event_rx: None,
        access_event_broadcast: None,
        sms_request_rx: None,
        sms_request_tx: None,
        data_request_rx: None,
        data_request_tx: None,
        power_override_request_rx: None,
        power_override_request_tx: None,
        mobiles_tx: None,
        paging_broadcast: None,
        traffic_broadcast: None,
        rx_reference_dbm: None,
        hlr_repo: None,
        msc_client: test_msc_client(),
        bts_client: Some(bts_client.clone() as Arc<dyn BtsControlClient>),
        traffic_retry: TrafficRetryConfig::default(),
        paging_retry: PagingRetryConfig::default(),
        voice_policy: test_voice_policy(),
        pcf_client: None,
        mobile_idle_timeout_s: 0,
        bts_paging_state: None,
        node_id: "bsc-test".to_string(),
        msc_voice_bearer: None,
    });

    let mut origination = test_access_event();
    origination.message_id = MessageId::Origination;
    origination.msg_type_name = "Origination Message".to_string();
    origination.msg_seq = Some(2);
    origination.ack_req = true;
    origination.esn = Some(0x1234_5678);
    origination.imsi_m_s1 = Some(0x0091_989e);
    origination.imsi_m_s2 = Some(0x0326);
    origination.imsi_class = Some(0);
    origination.imsi_mcc = Some(310);
    origination.imsi_11_12 = Some(99);
    origination.mob_p_rev = Some(5);
    origination.slot_cycle_index = Some(2);
    origination.scm = Some(0x2a);
    origination.service_option = Some(6);
    origination.for_supported_rcs = vec![1, 3];
    origination.rev_supported_rcs = vec![1, 3];

    bsc.inject_access_event(origination).await;

    assert!(bsc.mobiles[0].traffic_channel.is_none());
    assert_eq!(bsc.mobiles[0].state, MsState::Registered);
    assert!(traffic_channels.lock().is_empty());
    assert!(traffic_rx_pool.lock().is_empty());

    let pch_types = pch_message_types(&bts_client);
    assert!(!pch_types.contains(&MessageId::ChannelAssignment));
    assert!(!pch_types.contains(&MessageId::ExtChannelAssignment));
}

#[tokio::test]
async fn origination_prefers_policy_rc1_over_mobile_rc3_preference() {
    use std::sync::{Arc, mpsc::channel};
    let traffic_channels = Arc::new(Mutex::new(Vec::new()));
    let walsh_allocator = Arc::new(Mutex::new(WalshAllocator::new()));
    let traffic_rx_pool = Arc::new(Mutex::new(Vec::new()));
    let traffic_rx_removals = Arc::new(Mutex::new(Vec::new()));

    let mut bsc = Bsc::new(Config {
        pilot_offset: 0,
        overhead: OverheadParameters::default(),
        paging: PagingChannelSettings::default(),
        traffic_assignment: TrafficAssignmentConfig::default(),
        access_event_rx: None,
        access_event_broadcast: None,
        sms_request_rx: None,
        sms_request_tx: None,
        data_request_rx: None,
        data_request_tx: None,
        power_override_request_rx: None,
        power_override_request_tx: None,
        mobiles_tx: None,
        paging_broadcast: None,
        traffic_broadcast: None,
        rx_reference_dbm: None,
        hlr_repo: None,
        msc_client: test_msc_client(),
        bts_client: Some(test_bts_client(
            walsh_allocator,
            traffic_channels,
            traffic_rx_pool.clone(),
            traffic_rx_removals,
        )),
        traffic_retry: TrafficRetryConfig::default(),
        paging_retry: PagingRetryConfig::default(),
        voice_policy: test_voice_policy(),
        pcf_client: None,
        mobile_idle_timeout_s: 0,
        bts_paging_state: None,
        node_id: "bsc-test".to_string(),
        msc_voice_bearer: None,
    });

    let mut origination = test_access_event();
    origination.message_id = MessageId::Origination;
    origination.msg_type_name = "Origination Message".to_string();
    origination.msg_seq = Some(2);
    origination.ack_req = true;
    origination.esn = Some(0x1234_5678);
    origination.imsi_m_s1 = Some(0x0091_989e);
    origination.imsi_m_s2 = Some(0x0326);
    origination.imsi_class = Some(0);
    origination.imsi_mcc = Some(310);
    origination.imsi_11_12 = Some(99);
    origination.mob_p_rev = Some(6);
    origination.slot_cycle_index = Some(2);
    origination.scm = Some(0x2a);
    origination.service_option = Some(6);
    origination.for_supported_rcs = vec![1, 2, 3, 4, 5];
    origination.rev_supported_rcs = vec![1, 2, 3, 4];
    origination.for_rc_pref = Some(3);
    origination.rev_rc_pref = Some(3);

    bsc.inject_access_event(origination).await;

    let tc = bsc.mobiles[0]
        .traffic_channel
        .as_ref()
        .expect("traffic channel should be assigned");
    assert_eq!((tc.for_rc, tc.rev_rc), (1, 1));
    assert_eq!(tc.rc_label, "RC1");
    assert_eq!(traffic_rx_pool.lock()[0].assigned_rev_rc, 1);
}

#[tokio::test]
async fn origination_can_prefer_rc3_when_policy_requests_it() {
    use std::sync::{Arc, mpsc::channel};
    let traffic_channels = Arc::new(Mutex::new(Vec::new()));
    let walsh_allocator = Arc::new(Mutex::new(WalshAllocator::new()));
    let traffic_rx_pool = Arc::new(Mutex::new(Vec::new()));
    let traffic_rx_removals = Arc::new(Mutex::new(Vec::new()));

    let mut bsc = Bsc::new(Config {
        pilot_offset: 0,
        overhead: OverheadParameters::default(),
        paging: PagingChannelSettings::default(),
        traffic_assignment: TrafficAssignmentConfig {
            supported_for_rcs: vec![1, 3],
            supported_rev_rcs: vec![1, 3],
            preferred_pairs: vec![crate::config::RcPairConfig::new(3, 3)],
            idle_timeout_s: TrafficAssignmentConfig::default().idle_timeout_s,
            rev_fch_gating_mode: false,
        },
        access_event_rx: None,
        access_event_broadcast: None,
        sms_request_rx: None,
        sms_request_tx: None,
        data_request_rx: None,
        data_request_tx: None,
        power_override_request_rx: None,
        power_override_request_tx: None,
        mobiles_tx: None,
        paging_broadcast: None,
        traffic_broadcast: None,
        rx_reference_dbm: None,
        hlr_repo: None,
        msc_client: test_msc_client(),
        bts_client: Some(test_bts_client(
            walsh_allocator,
            traffic_channels,
            traffic_rx_pool.clone(),
            traffic_rx_removals,
        )),
        traffic_retry: TrafficRetryConfig::default(),
        paging_retry: PagingRetryConfig::default(),
        voice_policy: test_voice_policy(),
        pcf_client: None,
        mobile_idle_timeout_s: 0,
        bts_paging_state: None,
        node_id: "bsc-test".to_string(),
        msc_voice_bearer: None,
    });

    let mut origination = test_access_event();
    origination.message_id = MessageId::Origination;
    origination.msg_type_name = "Origination Message".to_string();
    origination.msg_seq = Some(2);
    origination.ack_req = true;
    origination.esn = Some(0x1234_5678);
    origination.imsi_m_s1 = Some(0x0091_989e);
    origination.imsi_m_s2 = Some(0x0326);
    origination.imsi_class = Some(0);
    origination.imsi_mcc = Some(310);
    origination.imsi_11_12 = Some(99);
    origination.mob_p_rev = Some(6);
    origination.slot_cycle_index = Some(2);
    origination.scm = Some(0x2a);
    origination.service_option = Some(6);
    origination.for_supported_rcs = vec![1, 2, 3, 4, 5];
    origination.rev_supported_rcs = vec![1, 2, 3, 4];
    origination.for_rc_pref = Some(1);
    origination.rev_rc_pref = Some(1);

    bsc.inject_access_event(origination).await;

    let tc = bsc.mobiles[0]
        .traffic_channel
        .as_ref()
        .expect("traffic channel should be assigned");
    assert_eq!((tc.for_rc, tc.rev_rc), (3, 3));
    assert_eq!(tc.rc_label, "RC3");
    assert_eq!(traffic_rx_pool.lock()[0].assigned_rev_rc, 3);
}

#[tokio::test]
async fn packet_data_origination_assigns_non_voice_traffic_channel() {
    use std::sync::{Arc, mpsc::channel};
    let traffic_channels = Arc::new(Mutex::new(Vec::new()));
    let walsh_allocator = Arc::new(Mutex::new(WalshAllocator::new()));
    let traffic_rx_pool = Arc::new(Mutex::new(Vec::new()));
    let traffic_rx_removals = Arc::new(Mutex::new(Vec::new()));

    let mut bsc = Bsc::new(Config {
        pilot_offset: 0,
        overhead: OverheadParameters::default(),
        paging: PagingChannelSettings::default(),
        traffic_assignment: TrafficAssignmentConfig::default(),
        access_event_rx: None,
        access_event_broadcast: None,
        sms_request_rx: None,
        sms_request_tx: None,
        data_request_rx: None,
        data_request_tx: None,
        power_override_request_rx: None,
        power_override_request_tx: None,
        mobiles_tx: None,
        paging_broadcast: None,
        traffic_broadcast: None,
        rx_reference_dbm: None,
        hlr_repo: None,
        msc_client: test_msc_client(),
        bts_client: Some(test_bts_client(
            walsh_allocator,
            traffic_channels,
            traffic_rx_pool.clone(),
            traffic_rx_removals,
        )),
        traffic_retry: TrafficRetryConfig::default(),
        paging_retry: PagingRetryConfig::default(),
        voice_policy: test_voice_policy(),
        pcf_client: None,
        mobile_idle_timeout_s: 0,
        bts_paging_state: None,
        node_id: "bsc-test".to_string(),
        msc_voice_bearer: None,
    });

    let mut origination = test_access_event();
    origination.message_id = MessageId::Origination;
    origination.msg_type_name = "Origination Message".to_string();
    origination.msg_seq = Some(2);
    origination.ack_req = true;
    origination.esn = Some(0x1234_5678);
    origination.imsi_m_s1 = Some(0x0091_989e);
    origination.imsi_m_s2 = Some(0x0326);
    origination.imsi_class = Some(0);
    origination.imsi_mcc = Some(310);
    origination.imsi_11_12 = Some(99);
    origination.mob_p_rev = Some(6);
    origination.slot_cycle_index = Some(2);
    origination.scm = Some(0x2a);
    origination.service_option = Some(7);
    origination.for_supported_rcs = vec![1, 2, 3, 4, 5];
    origination.rev_supported_rcs = vec![1, 2, 3, 4];

    bsc.inject_access_event(origination).await;

    let tc = bsc.mobiles[0]
        .traffic_channel
        .as_ref()
        .expect("packet-data traffic channel should be assigned");
    assert_eq!(bsc.mobiles[0].state, MsState::TrafficAssigning);
    assert_eq!(tc.service_option, 7);
    assert!(matches!(tc.channel_state, ChannelState::Assigned { .. }));
    assert!(tc.recent_primary_frames.is_empty());
    assert_eq!(traffic_rx_pool.lock().len(), 1);
}

#[tokio::test]
async fn packet_data_service_connect_completion_marks_channel_active() {
    use std::sync::{Arc, mpsc::channel};
    let traffic_channels = Arc::new(Mutex::new(Vec::new()));
    let walsh_allocator = Arc::new(Mutex::new(WalshAllocator::new()));
    let traffic_rx_pool = Arc::new(Mutex::new(Vec::new()));
    let traffic_rx_removals = Arc::new(Mutex::new(Vec::new()));

    let mut bsc = Bsc::new(Config {
        pilot_offset: 0,
        overhead: OverheadParameters::default(),
        paging: PagingChannelSettings::default(),
        traffic_assignment: TrafficAssignmentConfig::default(),
        access_event_rx: None,
        access_event_broadcast: None,
        sms_request_rx: None,
        sms_request_tx: None,
        data_request_rx: None,
        data_request_tx: None,
        power_override_request_rx: None,
        power_override_request_tx: None,
        mobiles_tx: None,
        paging_broadcast: None,
        traffic_broadcast: None,
        rx_reference_dbm: None,
        hlr_repo: None,
        msc_client: test_msc_client(),
        bts_client: Some(test_bts_client(
            walsh_allocator,
            traffic_channels,
            traffic_rx_pool,
            traffic_rx_removals,
        )),
        traffic_retry: TrafficRetryConfig::default(),
        paging_retry: PagingRetryConfig::default(),
        voice_policy: test_voice_policy(),
        pcf_client: None,
        mobile_idle_timeout_s: 0,
        bts_paging_state: None,
        node_id: "bsc-test".to_string(),
        msc_voice_bearer: None,
    });

    let mut origination = test_access_event();
    origination.message_id = MessageId::Origination;
    origination.msg_type_name = "Origination Message".to_string();
    origination.msg_seq = Some(2);
    origination.ack_req = true;
    origination.esn = Some(0x1234_5678);
    origination.imsi_m_s1 = Some(0x0091_989e);
    origination.imsi_m_s2 = Some(0x0326);
    origination.imsi_class = Some(0);
    origination.imsi_mcc = Some(310);
    origination.imsi_11_12 = Some(99);
    origination.mob_p_rev = Some(6);
    origination.slot_cycle_index = Some(2);
    origination.scm = Some(0x2a);
    origination.service_option = Some(7);
    origination.for_supported_rcs = vec![1, 2, 3, 4, 5];
    origination.rev_supported_rcs = vec![1, 2, 3, 4];

    bsc.inject_access_event(origination).await;

    let walsh_code = bsc.mobiles[0]
        .traffic_channel
        .as_ref()
        .expect("packet-data traffic channel should be assigned")
        .walsh_code;
    bsc.mobiles[0].set_state(MsState::TrafficActive);

    let mut completion = test_access_event();
    completion.message_id = MessageId::ServiceConnectCompletion;
    completion.msg_type_name = "Service Connect Completion Message".to_string();
    completion.traffic_walsh_code = Some(walsh_code);

    bsc.inject_access_event(completion).await;

    let tc = bsc.mobiles[0]
        .traffic_channel
        .as_ref()
        .expect("packet-data traffic channel should remain assigned");
    assert!(matches!(tc.channel_state, ChannelState::Active));
}

#[tokio::test]
async fn packet_data_send_service_connect_uses_origination_sr_id_and_omits_optional_fields() {
    use std::sync::{Arc, mpsc::channel};
    let traffic_channels = Arc::new(Mutex::new(Vec::new()));
    let walsh_allocator = Arc::new(Mutex::new(WalshAllocator::new()));
    let traffic_rx_pool = Arc::new(Mutex::new(Vec::new()));
    let traffic_rx_removals = Arc::new(Mutex::new(Vec::new()));

    let mut bsc = Bsc::new(Config {
        pilot_offset: 0,
        overhead: OverheadParameters::default(),
        paging: PagingChannelSettings::default(),
        traffic_assignment: TrafficAssignmentConfig::default(),
        access_event_rx: None,
        access_event_broadcast: None,
        sms_request_rx: None,
        sms_request_tx: None,
        data_request_rx: None,
        data_request_tx: None,
        power_override_request_rx: None,
        power_override_request_tx: None,
        mobiles_tx: None,
        paging_broadcast: None,
        traffic_broadcast: None,
        rx_reference_dbm: None,
        hlr_repo: None,
        msc_client: test_msc_client(),
        bts_client: Some(test_bts_client(
            walsh_allocator,
            traffic_channels,
            traffic_rx_pool,
            traffic_rx_removals,
        )),
        traffic_retry: TrafficRetryConfig::default(),
        paging_retry: PagingRetryConfig::default(),
        voice_policy: test_voice_policy(),
        pcf_client: None,
        mobile_idle_timeout_s: 0,
        bts_paging_state: None,
        node_id: "bsc-test".to_string(),
        msc_voice_bearer: None,
    });

    let mut origination = test_access_event();
    origination.message_id = MessageId::Origination;
    origination.msg_type_name = "Origination Message".to_string();
    origination.msg_seq = Some(2);
    origination.ack_req = true;
    origination.esn = Some(0x1234_5678);
    origination.imsi_m_s1 = Some(0x0091_989e);
    origination.imsi_m_s2 = Some(0x0326);
    origination.imsi_class = Some(0);
    origination.imsi_mcc = Some(310);
    origination.imsi_11_12 = Some(99);
    origination.mob_p_rev = Some(6);
    origination.slot_cycle_index = Some(2);
    origination.scm = Some(0x2a);
    origination.service_option = Some(7);
    origination.decoded_l3 = Some(test_origination_l3(7, 3));
    origination.for_supported_rcs = vec![1, 2, 3, 4, 5];
    origination.rev_supported_rcs = vec![1, 2, 3, 4];

    bsc.inject_access_event(origination).await;

    let walsh_code = bsc.mobiles[0]
        .traffic_channel
        .as_ref()
        .expect("packet-data traffic channel should be assigned")
        .walsh_code;
    bsc.mobiles[0].set_state(MsState::TrafficActive);
    if let Some(tc) = bsc.mobiles[0].traffic_channel.as_mut() {
        tc.channel_state = ChannelState::ServiceConnecting {
            sc_sent_at: Instant::now(),
        };
    }

    bsc.send_service_connect(walsh_code, 0)
        .expect("packet-data Service Connect should produce FchForward");
}

#[tokio::test]
async fn so33_service_connect_omits_rlp_blob_and_uses_origination_sr_id() {
    use std::sync::{Arc, mpsc::channel};
    let traffic_channels = Arc::new(Mutex::new(Vec::new()));
    let walsh_allocator = Arc::new(Mutex::new(WalshAllocator::new()));
    let traffic_rx_pool = Arc::new(Mutex::new(Vec::new()));
    let traffic_rx_removals = Arc::new(Mutex::new(Vec::new()));

    let mut bsc = Bsc::new(Config {
        pilot_offset: 0,
        overhead: OverheadParameters::default(),
        paging: PagingChannelSettings::default(),
        traffic_assignment: TrafficAssignmentConfig::default(),
        access_event_rx: None,
        access_event_broadcast: None,
        sms_request_rx: None,
        sms_request_tx: None,
        data_request_rx: None,
        data_request_tx: None,
        power_override_request_rx: None,
        power_override_request_tx: None,
        mobiles_tx: None,
        paging_broadcast: None,
        traffic_broadcast: None,
        rx_reference_dbm: None,
        hlr_repo: None,
        msc_client: test_msc_client(),
        bts_client: Some(test_bts_client(
            walsh_allocator,
            traffic_channels,
            traffic_rx_pool,
            traffic_rx_removals,
        )),
        traffic_retry: TrafficRetryConfig::default(),
        paging_retry: PagingRetryConfig::default(),
        voice_policy: test_voice_policy(),
        pcf_client: None,
        mobile_idle_timeout_s: 0,
        bts_paging_state: None,
        node_id: "bsc-test".to_string(),
        msc_voice_bearer: None,
    });

    let mut origination = test_access_event();
    origination.message_id = MessageId::Origination;
    origination.msg_type_name = "Origination Message".to_string();
    origination.msg_seq = Some(2);
    origination.ack_req = true;
    origination.esn = Some(0x1234_5678);
    origination.imsi_m_s1 = Some(0x0091_989e);
    origination.imsi_m_s2 = Some(0x0326);
    origination.imsi_class = Some(0);
    origination.imsi_mcc = Some(310);
    origination.imsi_11_12 = Some(99);
    origination.mob_p_rev = Some(6);
    origination.slot_cycle_index = Some(2);
    origination.scm = Some(0x2a);
    origination.service_option = Some(33);
    origination.decoded_l3 = Some(test_origination_l3(33, 1));
    origination.for_supported_rcs = vec![1, 2, 3, 4, 5];
    origination.rev_supported_rcs = vec![1, 2, 3, 4];

    bsc.inject_access_event(origination).await;

    let walsh_code = bsc.mobiles[0]
        .traffic_channel
        .as_ref()
        .expect("packet-data traffic channel should be assigned")
        .walsh_code;
    bsc.mobiles[0].set_state(MsState::TrafficActive);
    if let Some(tc) = bsc.mobiles[0].traffic_channel.as_mut() {
        tc.channel_state = ChannelState::ServiceConnecting {
            sc_sent_at: Instant::now(),
        };
    }

    bsc.send_service_connect(walsh_code, 0)
        .expect("SO33 Service Connect should produce FchForward");
}

#[tokio::test]
async fn packet_data_reverse_bearer_primary_frame_feeds_packet_session() {
    use std::sync::Arc;
    let traffic_channels = Arc::new(Mutex::new(Vec::new()));
    let walsh_allocator = Arc::new(Mutex::new(WalshAllocator::new()));
    let traffic_rx_pool = Arc::new(Mutex::new(Vec::new()));
    let traffic_rx_removals = Arc::new(Mutex::new(Vec::new()));

    let mut bsc = Bsc::new(Config {
        pilot_offset: 0,
        overhead: OverheadParameters::default(),
        paging: PagingChannelSettings::default(),
        traffic_assignment: TrafficAssignmentConfig::default(),
        access_event_rx: None,
        access_event_broadcast: None,
        sms_request_rx: None,
        sms_request_tx: None,
        data_request_rx: None,
        data_request_tx: None,
        power_override_request_rx: None,
        power_override_request_tx: None,
        mobiles_tx: None,
        paging_broadcast: None,
        traffic_broadcast: None,
        rx_reference_dbm: None,
        hlr_repo: None,
        msc_client: test_msc_client(),
        bts_client: Some(test_bts_client(
            walsh_allocator,
            traffic_channels,
            traffic_rx_pool,
            traffic_rx_removals,
        )),
        traffic_retry: TrafficRetryConfig::default(),
        paging_retry: PagingRetryConfig::default(),
        voice_policy: test_voice_policy(),
        pcf_client: None,
        mobile_idle_timeout_s: 0,
        bts_paging_state: None,
        node_id: "bsc-test".to_string(),
        msc_voice_bearer: None,
    });

    let mut origination = test_access_event();
    origination.message_id = MessageId::Origination;
    origination.msg_type_name = "Origination Message".to_string();
    origination.msg_seq = Some(2);
    origination.ack_req = true;
    origination.esn = Some(0x1234_5678);
    origination.imsi_m_s1 = Some(0x0091_989e);
    origination.imsi_m_s2 = Some(0x0326);
    origination.imsi_class = Some(0);
    origination.imsi_mcc = Some(310);
    origination.imsi_11_12 = Some(99);
    origination.mob_p_rev = Some(6);
    origination.slot_cycle_index = Some(2);
    origination.scm = Some(0x2a);
    origination.service_option = Some(7);
    origination.for_supported_rcs = vec![1, 2, 3, 4, 5];
    origination.rev_supported_rcs = vec![1, 2, 3, 4];

    bsc.inject_access_event(origination).await;

    let walsh_code = bsc.mobiles[0]
        .traffic_channel
        .as_ref()
        .expect("packet-data traffic channel should be assigned")
        .walsh_code;
    bsc.mobiles[0].set_state(MsState::TrafficActive);
    if let Some(tc) = bsc.mobiles[0].traffic_channel.as_mut() {
        tc.channel_state = ChannelState::Active;
        tc.packet_session_id = Some("packet-test-session".to_string());
    }

    let (packet_uplink_tx, mut packet_uplink_rx) =
        tokio::sync::mpsc::channel::<crate::packet::PacketBearerFrame>(4);
    bsc.mobiles[0]
        .traffic_channel
        .as_mut()
        .expect("packet-data traffic channel should remain assigned")
        .packet_uplink_tx = Some(packet_uplink_tx);

    let mut information = vec![0u8]; // MUX1 MM=0, primary traffic only.
    information.extend(vec![1u8; 171]);
    let fch = ReverseFchDcchFrame {
        channel_family: ChannelFamily::Fch,
        soft_handoff_leg: 0,
        fsn: 0,
        fqi: true,
        reverse_link_quality: 0,
        scaling: 0,
        packet_arrival_time_error: 0,
        frame_content: FrameContent::FchRc3_9600,
        fpc_s: 0,
        eib: false,
        reverse_link_information: information,
        message_crc: 0,
    };

    assert!(bsc.route_reverse_bearer_packet_primary(walsh_code, &fch));

    let tc = bsc.mobiles[0]
        .traffic_channel
        .as_ref()
        .expect("packet-data traffic channel should remain assigned");
    assert_eq!(tc.recent_primary_frames.len(), 1);
    assert_eq!(tc.recent_primary_frames[0].bits, vec![1u8; 171]);
    assert_eq!(tc.recent_primary_frames[0].rate_bps, 9600);

    let packet_frame = packet_uplink_rx
        .try_recv()
        .expect("packet session should receive reverse bearer primary frame");
    assert_eq!(packet_frame.session_id, "packet-test-session");
    assert_eq!(packet_frame.bits, vec![1u8; 171]);
    assert_eq!(packet_frame.num_bits, 171);
    assert_eq!(packet_frame.rate_bps, 9600);
}

#[test]
fn rc3_mux1_signaling_frame_exposes_only_primary_as_voice_payload() {
    let mut information = vec![1, 0, 1, 0]; // MUX1 MM=1010: 16 primary + 152 signaling.
    information.extend(vec![1u8; 16]);
    information.extend(vec![0u8; 152]);
    let fch = ReverseFchDcchFrame {
        channel_family: ChannelFamily::Fch,
        soft_handoff_leg: 0,
        fsn: 0,
        fqi: true,
        reverse_link_quality: 0,
        scaling: 0,
        packet_arrival_time_error: 0,
        frame_content: FrameContent::FchRc3_9600,
        fpc_s: 0,
        eib: false,
        reverse_link_information: information,
        message_crc: 0,
    };

    let event = Bsc::bearer_reverse_primary_to_event(10, &fch, 0)
        .expect("MUX1 frame should expose the primary portion");

    assert_eq!(event.traffic_primary_rate_bps, Some(1500));
    assert_eq!(
        event.traffic_primary_bits.as_deref(),
        Some(&vec![1u8; 16][..])
    );
    assert_eq!(event.traffic_voice_rate_bps, Some(1500));
    assert_eq!(
        event.traffic_voice_bits.as_deref(),
        Some(&vec![1u8; 16][..])
    );
}

#[test]
fn rc3_mux1_signaling_only_frame_is_not_voice_payload() {
    let mut information = vec![1, 0, 1, 1]; // MUX1 MM=1011: signaling only.
    information.extend(vec![0u8; 168]);
    let fch = ReverseFchDcchFrame {
        channel_family: ChannelFamily::Fch,
        soft_handoff_leg: 0,
        fsn: 0,
        fqi: true,
        reverse_link_quality: 0,
        scaling: 0,
        packet_arrival_time_error: 0,
        frame_content: FrameContent::FchRc3_9600,
        fpc_s: 0,
        eib: false,
        reverse_link_information: information,
        message_crc: 0,
    };

    assert!(Bsc::bearer_reverse_primary_to_event(10, &fch, 0).is_none());
}

#[tokio::test]
async fn registration_from_paged_ms_cancels_retry_and_delivers_sms() {
    let bts_client = Arc::new(CapturingBtsClient::default());

    let mut bsc = Bsc::new(Config {
        pilot_offset: 0,
        overhead: OverheadParameters::default(),
        paging: PagingChannelSettings::default(),
        traffic_assignment: TrafficAssignmentConfig::default(),
        access_event_rx: None,
        access_event_broadcast: None,
        sms_request_rx: None,
        sms_request_tx: None,
        data_request_rx: None,
        data_request_tx: None,
        power_override_request_rx: None,
        power_override_request_tx: None,
        mobiles_tx: None,
        paging_broadcast: None,
        traffic_broadcast: None,
        rx_reference_dbm: None,
        hlr_repo: None,
        msc_client: test_msc_client(),
        bts_client: Some(bts_client.clone() as Arc<dyn BtsControlClient>),
        traffic_retry: TrafficRetryConfig::default(),
        paging_retry: PagingRetryConfig::default(),
        voice_policy: test_voice_policy(),
        pcf_client: None,
        mobile_idle_timeout_s: 0,
        bts_paging_state: None,
        node_id: "bsc-test".to_string(),
        msc_voice_bearer: None,
    });

    let esn = 0x1234_5678;
    let mut registration = test_access_event();
    registration.message_id = MessageId::Registration;
    registration.msg_type_name = "Registration Message".to_string();
    registration.msg_seq = Some(3);
    registration.ack_req = true;
    registration.esn = Some(esn);
    registration.imsi_m_s1 = Some(0x0091_989e);
    registration.imsi_m_s2 = Some(0x0326);
    registration.imsi_class = Some(0);
    registration.imsi_mcc = Some(310);
    registration.imsi_11_12 = Some(99);
    registration.mob_p_rev = Some(6);
    registration.slot_cycle_index = Some(2);
    registration.scm = Some(0x2a);

    bsc.inject_access_event(registration.clone()).await;
    bts_client.pch_messages.lock().clear();

    bsc.inject_sms_request(SmsRequest {
        originating_number: "5551234".to_string(),
        text: "pending sms".to_string(),
        target_address: Some(format!("ESN:0x{:08X}", esn)),
        target_subscriber_id: None,
        timeout_ms: Some(60_000),
        destination_number: None,
        sms_id: None,
        delivery_attempt_id: None,
        a1_tag: None,
        raw_payload: None,
    });
    assert!(bsc.paging.has_pending_page(), "expected page to be pending");
    bts_client.pch_messages.lock().clear();

    registration.msg_seq = Some(4);
    bsc.inject_access_event(registration).await;

    assert!(
        !bsc.paging.has_pending_page(),
        "expected pending page to be cleared after same-MS registration"
    );

    let saw_data_burst = bts_client
        .pch_messages
        .lock()
        .iter()
        .filter_map(|message| message.air_interface_message.as_ref())
        .any(|aim| {
            MessageId::from_wire(
                cdma_common::lac::message_types::WireChannel::ForwardCommon,
                aim.message_type,
            ) == Some(MessageId::DataBurst)
        });
    assert!(
        saw_data_burst,
        "expected SMS Data Burst to be emitted after same-MS registration"
    );
}

#[tokio::test]
async fn sms_target_imsi_s_matches_registered_imsi_class0_mobile() {
    let (paging_tx, mut paging_rx) = broadcast::channel(16);

    let mut bsc = Bsc::new(Config {
        pilot_offset: 0,
        overhead: OverheadParameters::default(),
        paging: PagingChannelSettings::default(),
        traffic_assignment: TrafficAssignmentConfig::default(),
        access_event_rx: None,
        access_event_broadcast: None,
        sms_request_rx: None,
        sms_request_tx: None,
        data_request_rx: None,
        data_request_tx: None,
        power_override_request_rx: None,
        power_override_request_tx: None,
        mobiles_tx: None,
        paging_broadcast: Some(paging_tx),
        traffic_broadcast: None,
        rx_reference_dbm: None,
        hlr_repo: None,
        msc_client: test_msc_client(),
        bts_client: None,
        traffic_retry: TrafficRetryConfig::default(),
        paging_retry: PagingRetryConfig::default(),
        voice_policy: test_voice_policy(),
        pcf_client: None,
        mobile_idle_timeout_s: 0,
        bts_paging_state: None,
        node_id: "bsc-test".to_string(),
        msc_voice_bearer: None,
    });

    let mut registration = test_access_event();
    registration.message_id = MessageId::Registration;
    registration.msg_type_name = "Registration Message".to_string();
    registration.msg_seq = Some(3);
    registration.ack_req = true;
    registration.esn = Some(0x1234_5678);
    registration.imsi_m_s1 = Some(0x0091_989e);
    registration.imsi_m_s2 = Some(0x0326);
    registration.imsi_class = Some(0);
    registration.imsi_mcc = Some(310);
    registration.imsi_11_12 = Some(99);
    registration.mob_p_rev = Some(6);
    registration.slot_cycle_index = Some(2);
    registration.scm = Some(0x2a);

    bsc.inject_access_event(registration).await;
    while paging_rx.try_recv().is_ok() {}

    bsc.inject_sms_request(SmsRequest {
        originating_number: "5551234".to_string(),
        text: "imsi target sms".to_string(),
        target_address: Some("IMSI_S:s1=9541790,s2=806".to_string()),
        target_subscriber_id: None,
        timeout_ms: Some(60_000),
        destination_number: None,
        sms_id: None,
        delivery_attempt_id: None,
        a1_tag: None,
        raw_payload: None,
    });

    assert!(
        bsc.paging.has_pending_page(),
        "expected page to be pending for IMSI_S target against class-0 registered mobile"
    );
    // SMS delivery registers a pending page record. The structural paging
    // supplier folds those records into the first GPM emitted in each
    // paging slot; it is not pushed as an immediate directed paging event.
    while let Ok(event) = paging_rx.try_recv() {
        assert!(
            !matches!(event.message, PagingChannelMessage::GeneralPage(_)),
            "did not expect an immediate GPM before the paging supplier runs"
        );
    }
}

// deliver_pending_sms_for_destination_includes_stuck_paging_submissions removed:
// SMSC redelivery sweep is now MSC-owned; BSC no longer calls deliver_pending_sms_for_destination.

#[tokio::test]
async fn duplicate_access_probe_gets_l2_ack_only() {
    let mut bsc = Bsc::new(Config {
        pilot_offset: 0,
        overhead: OverheadParameters::default(),
        paging: PagingChannelSettings::default(),
        traffic_assignment: TrafficAssignmentConfig::default(),
        access_event_rx: None,
        access_event_broadcast: None,
        sms_request_rx: None,
        sms_request_tx: None,
        data_request_rx: None,
        data_request_tx: None,
        power_override_request_rx: None,
        power_override_request_tx: None,
        mobiles_tx: None,
        paging_broadcast: None,
        traffic_broadcast: None,
        rx_reference_dbm: None,
        hlr_repo: None,
        msc_client: test_msc_client(),
        bts_client: None,
        traffic_retry: TrafficRetryConfig::default(),
        paging_retry: PagingRetryConfig::default(),
        voice_policy: test_voice_policy(),
        pcf_client: None,
        mobile_idle_timeout_s: 0,
        bts_paging_state: None,
        node_id: "bsc-test".to_string(),
        msc_voice_bearer: None,
    });

    let esn = 0x1234_5678;
    let mut registration = test_access_event();
    registration.message_id = MessageId::Registration;
    registration.msg_type_name = "Registration Message".to_string();
    registration.msg_seq = Some(3);
    registration.ack_req = true;
    registration.esn = Some(esn);
    registration.imsi_m_s1 = Some(0x0091_989e);
    registration.imsi_m_s2 = Some(0x0326);
    registration.imsi_class = Some(0);
    registration.imsi_mcc = Some(310);
    registration.imsi_11_12 = Some(99);
    registration.mob_p_rev = Some(6);
    registration.slot_cycle_index = Some(2);
    registration.scm = Some(0x2a);
    bsc.inject_access_event(registration).await;

    bsc.inject_sms_request(SmsRequest {
        originating_number: "5551234".to_string(),
        text: "page response sms".to_string(),
        target_address: Some(format!("ESN:0x{esn:08X}")),
        target_subscriber_id: None,
        timeout_ms: Some(60_000),
        destination_number: None,
        sms_id: None,
        delivery_attempt_id: None,
        a1_tag: None,
        raw_payload: None,
    });
    assert!(bsc.paging.has_pending_page(), "expected page to be pending");

    let mut page_response = test_access_event();
    page_response.message_id = MessageId::PageResponse;
    page_response.msg_type_name = "Page Response Message".to_string();
    page_response.msg_seq = Some(4);
    page_response.ack_req = true;
    page_response.esn = Some(esn);
    page_response.imsi_m_s1 = Some(0x0091_989e);
    page_response.imsi_m_s2 = Some(0x0326);
    page_response.imsi_class = Some(0);
    page_response.imsi_mcc = Some(310);
    page_response.imsi_11_12 = Some(99);
    page_response.mob_p_rev = Some(6);
    page_response.slot_cycle_index = Some(2);
    page_response.scm = Some(0x2a);

    bsc.inject_access_event(page_response.clone()).await;
    assert!(
        !bsc.paging.has_pending_page(),
        "expected pending page to clear after page response"
    );

    // Per C.S0004-E 3.1.1.2.2.2: duplicate access probe gets L2 ack
    // only — the SDU is discarded, not re-processed. Verify the
    // duplicate is detected (MSG_SEQ_RCVD[4] should be true).
    assert!(
        bsc.mobiles[0].access_msg_seq_rcvd[4],
        "expected MSG_SEQ=4 to be marked as received"
    );

    // Inject the same page response again — should be treated as
    // duplicate and acknowledged at L2 without re-processing.
    bsc.inject_access_event(page_response).await;
    // The duplicate should not change registration count or cause errors.
    assert_eq!(bsc.mobiles.tracked_count(), 1);
}

#[tokio::test]
async fn sms_page_gpm_includes_so6_special_service() {
    let bts_client = Arc::new(CapturingBtsClient::default());

    let mut bsc = Bsc::new(Config {
        pilot_offset: 0,
        overhead: OverheadParameters::default(),
        paging: PagingChannelSettings::default(),
        traffic_assignment: TrafficAssignmentConfig::default(),
        access_event_rx: None,
        access_event_broadcast: None,
        sms_request_rx: None,
        sms_request_tx: None,
        data_request_rx: None,
        data_request_tx: None,
        power_override_request_rx: None,
        power_override_request_tx: None,
        mobiles_tx: None,
        paging_broadcast: None,
        traffic_broadcast: None,
        rx_reference_dbm: None,
        hlr_repo: None,
        msc_client: test_msc_client(),
        bts_client: Some(bts_client.clone() as Arc<dyn BtsControlClient>),
        traffic_retry: TrafficRetryConfig::default(),
        paging_retry: PagingRetryConfig::default(),
        voice_policy: test_voice_policy(),
        pcf_client: None,
        mobile_idle_timeout_s: 0,
        bts_paging_state: None,
        node_id: "bsc-test".to_string(),
        msc_voice_bearer: None,
    });

    let esn = 0x1234_5678;
    let mut registration = test_access_event();
    registration.message_id = MessageId::Registration;
    registration.msg_type_name = "Registration Message".to_string();
    registration.msg_seq = Some(3);
    registration.ack_req = true;
    registration.esn = Some(esn);
    registration.mob_p_rev = Some(3);
    registration.slot_cycle_index = Some(1);
    registration.scm = Some(0x6a);

    bsc.inject_access_event(registration).await;
    bts_client.pch_messages.lock().clear();

    bsc.inject_sms_request(SmsRequest {
        originating_number: "5551234".to_string(),
        text: "pending sms".to_string(),
        target_address: Some(format!("ESN:0x{:08X}", esn)),
        target_subscriber_id: None,
        timeout_ms: Some(60_000),
        destination_number: None,
        sms_id: None,
        delivery_attempt_id: None,
        a1_tag: None,
        raw_payload: None,
    });

    let messages = bts_client.pch_messages.lock();
    let aim = messages
        .iter()
        .filter_map(|message| message.air_interface_message.as_ref())
        .find(|aim| {
            MessageId::from_wire(
                cdma_common::lac::message_types::WireChannel::ForwardCommon,
                aim.message_type,
            ) == Some(MessageId::GeneralPage)
        })
        .expect("SMS page should send a General Page Message via Abis");

    let mut bits = Bitstream::new_bytes(&aim.message);
    let gpm = lac::paging_messages::GeneralPageMessage::from_sdu(&mut bits)
        .expect("captured GPM should decode");
    assert_eq!(gpm.page_records.len(), 1);
    match &gpm.page_records[0] {
        lac::paging_messages::GeneralPageRecord::Class1 {
            special_service,
            service_option,
            ..
        } => {
            assert!(*special_service, "SMS page must set SPECIAL_SERVICE");
            assert_eq!(*service_option, Some(6), "SMS page must announce SO6");
        }
        record => panic!("expected ESN Class1 page record, got {record:?}"),
    }
}

#[test]
fn access_duplicate_detection_clears_after_inactivity_timeout() {
    let mut access = AccessService::new();
    let mut mobile = MobileStation::new_for_test(
        MsAddress::Esn(0x1234_5678),
        Some(0x1234_5678),
        None,
        6,
        MsState::Registered,
        2,
        None,
    );
    let last_activity = Instant::now();
    mobile.last_access_activity = Some(last_activity);
    mobile.access_msg_seq_rcvd[4] = true;
    mobile.access_msg_seq_rcvd[0] = true;

    let mut event = test_access_event();
    event.msg_seq = Some(4);
    let decision = access.handle_known_mobile_msg_seq(
        &mut mobile,
        &event,
        last_activity + access::ACCESS_INACTIVITY_TIMEOUT + Duration::from_millis(1),
    );

    assert!(matches!(decision, access::AccessDuplicateDecision::NewSdu));
    assert!(
        mobile.access_msg_seq_rcvd[4],
        "current MSG_SEQ should be marked after reset"
    );
    assert!(
        !mobile.access_msg_seq_rcvd[0],
        "stale MSG_SEQ state should be cleared after inactivity"
    );
}

// order_ack_excludes_just_delivered_sms_from_immediate_redelivery_query removed:
// schedule_redelivery_for_mobile is removed; SMSC redelivery is MSC-owned.

// active_traffic_sms_sweep_enqueues_address_targeted_submission removed:
// schedule_redelivery_for_active_traffic_mobiles is removed; MSC owns SMS coordination.

#[test]
fn pch_l2_ack_clears_pending_sms_ack() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let sms_id = Uuid::new_v4();
        let attempt_id = Uuid::new_v4();
        let bts_client = Arc::new(CapturingBtsClient::default());
        let mut bsc = test_bsc_with_max_slot_cycle_index(2);
        bsc.access_tx = AccessTx::new(Some(bts_client.clone() as Arc<dyn BtsControlClient>));
        let addr = MsAddress::ImsiS {
            imsi_m_s1: 16369843,
            imsi_m_s2: 999,
        };
        bsc.mobiles.push(MobileStation::new_for_test(
            addr.clone(),
            None,
            None,
            6,
            MsState::PageResponseReceived,
            2,
            Some(100),
        ));

        bsc.sms.pending_acks.push(PendingSmsAck {
            key: SmsAckKey::PchCorrelation(77),
            sms_id: Some(sms_id),
            delivery_attempt_id: Some(attempt_id),
            addr: addr.clone(),
            sent_at: Instant::now(),
            a1_tag: None,
        });

        bsc.handle_pch_transfer_ack(PchTransferAckEvent {
            correlation_id: Some(77),
            cause: None,
            bts_l2_termination: Some(true),
        });

        assert!(bsc.sms.pending_acks.is_empty());
        assert_eq!(
            bsc.mobiles.get(&addr).map(|ms| ms.state.clone()),
            Some(MsState::Registered)
        );

        let messages = bts_client.pch_messages.lock();
        assert_eq!(
            messages.len(),
            1,
            "SMS ACK should trigger one Release Order"
        );
        let aim = messages[0]
            .air_interface_message
            .as_ref()
            .expect("Release Order should carry an air-interface message");
        assert_eq!(
            MessageId::from_wire(
                cdma_common::lac::message_types::WireChannel::ForwardCommon,
                aim.message_type,
            ),
            Some(MessageId::Order)
        );
        let mut bits = Bitstream::new_bytes(&aim.message);
        let order = lac::paging_messages::OrderMessage::from_sdu(&mut bits)
            .expect("Release Order should decode");
        assert_eq!(order.order, 0b010101);
        assert_eq!(order.ordq, 0);
        assert!(order.order_specific_fields.is_empty());
        assert!(messages[0].layer2_ack_request_results.is_none());
        assert!(messages[0].abis_ack_notify.is_none());
    });
}

#[test]
fn stale_pch_sms_ack_is_still_cleared_on_bts_l2_result() {
    // PCH acks owned by BTS are only removed via handle_pch_transfer_ack
    // (not by expire_stale_acks, which only touches traffic-owned acks).
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let sms_id = Uuid::new_v4();
        let attempt_id = Uuid::new_v4();
        let mut bsc = test_bsc_with_max_slot_cycle_index(2);

        bsc.sms.pending_acks.push(PendingSmsAck {
            key: SmsAckKey::PchCorrelation(77),
            sms_id: Some(sms_id),
            delivery_attempt_id: Some(attempt_id),
            addr: MsAddress::ImsiS {
                imsi_m_s1: 16369843,
                imsi_m_s2: 999,
            },
            sent_at: Instant::now() - Duration::from_secs(6),
            a1_tag: None,
        });

        // PCH correlation acks are NOT expired by expire_stale_acks
        // (that only removes TrafficMsgSeq keys). The ack should remain.
        assert_eq!(bsc.sms.pending_acks.len(), 1);

        bsc.handle_pch_transfer_ack(PchTransferAckEvent {
            correlation_id: Some(77),
            cause: None,
            bts_l2_termination: Some(true),
        });

        assert!(bsc.sms.pending_acks.is_empty());
    });
}

#[test]
fn pch_failure_clears_pending_sms_ack() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let sms_id = Uuid::new_v4();
        let attempt_id = Uuid::new_v4();
        let mut bsc = test_bsc_with_max_slot_cycle_index(2);

        bsc.sms.pending_acks.push(PendingSmsAck {
            key: SmsAckKey::PchCorrelation(88),
            sms_id: Some(sms_id),
            delivery_attempt_id: Some(attempt_id),
            addr: MsAddress::ImsiS {
                imsi_m_s1: 16369843,
                imsi_m_s2: 999,
            },
            sent_at: Instant::now(),
            a1_tag: None,
        });

        bsc.handle_pch_transfer_ack(PchTransferAckEvent {
            correlation_id: Some(88),
            cause: Some(0x07),
            bts_l2_termination: None,
        });

        assert!(bsc.sms.pending_acks.is_empty());
    });
}

#[test]
fn correlated_gpm_page_response_ack_does_not_clear_pending_sms_page() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let mut bsc = test_bsc_with_max_slot_cycle_index(2);
        bsc.paging.queue_sms_page(PendingPage {
            sms: SmsRequest {
                originating_number: "5551234".to_string(),
                text: "pending".to_string(),
                target_address: Some("ESN:0x11111111".to_string()),
                target_subscriber_id: None,
                timeout_ms: Some(60_000),
                destination_number: Some("5559999".to_string()),
                sms_id: None,
                delivery_attempt_id: None,
                a1_tag: None,
                raw_payload: None,
            },
            page_address: MsPageAddress::Esn(0x1111_1111),
            fwd_address: MsAddress::Esn(0x1111_1111),
            pgslot: Some(100),
            slot_cycle_index: 2,
            started_at: Instant::now(),
            timeout: Duration::from_secs(60),
            retry_count: 0,
            next_retry_at: tokio::time::Instant::now(),
            last_target_chip: None,
            page_msg_seq: Some(3),
            page_correlation_id: Some(77),
        });

        bsc.handle_pch_transfer_ack(PchTransferAckEvent {
            correlation_id: Some(77),
            cause: None,
            bts_l2_termination: Some(true),
        });

        assert!(bsc.paging.has_pending_sms_page());
    });
}

#[test]
fn correlated_gpm_page_failure_clears_pending_sms_page() {
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let mut bsc = test_bsc_with_max_slot_cycle_index(2);
        bsc.paging.queue_sms_page(PendingPage {
            sms: SmsRequest {
                originating_number: "5551234".to_string(),
                text: "pending".to_string(),
                target_address: Some("ESN:0x11111111".to_string()),
                target_subscriber_id: None,
                timeout_ms: Some(60_000),
                destination_number: Some("5559999".to_string()),
                sms_id: None,
                delivery_attempt_id: None,
                a1_tag: None,
                raw_payload: None,
            },
            page_address: MsPageAddress::Esn(0x1111_1111),
            fwd_address: MsAddress::Esn(0x1111_1111),
            pgslot: Some(100),
            slot_cycle_index: 2,
            started_at: Instant::now(),
            timeout: Duration::from_secs(60),
            retry_count: 0,
            next_retry_at: tokio::time::Instant::now(),
            last_target_chip: None,
            page_msg_seq: Some(3),
            page_correlation_id: Some(77),
        });

        bsc.handle_pch_transfer_ack(PchTransferAckEvent {
            correlation_id: Some(77),
            cause: Some(0x07),
            bts_l2_termination: None,
        });

        assert!(!bsc.paging.has_pending_sms_page());
    });
}

#[test]
fn rejected_sms_request_warns_and_does_not_page() {
    // BSC no longer updates SMSC state; it just warns and returns.
    // Verify the page-in-progress guard still prevents double paging.
    let runtime = tokio::runtime::Runtime::new().unwrap();
    runtime.block_on(async {
        let sms_id = Uuid::new_v4();
        let subscriber_id = Uuid::new_v4();

        let mut bsc = test_bsc_with_max_slot_cycle_index(2);

        bsc.mobiles.push(MobileStation::new_for_test(
            MsAddress::Esn(0x1234_5678),
            Some(0x1234_5678),
            None,
            6,
            MsState::Registered,
            2,
            Some(100),
        ));

        bsc.paging.queue_sms_page(PendingPage {
            sms: SmsRequest {
                originating_number: "5551234".to_string(),
                text: "existing".to_string(),
                target_address: Some("ESN:0x11111111".to_string()),
                target_subscriber_id: Some(subscriber_id),
                timeout_ms: Some(60_000),
                destination_number: Some("5559999".to_string()),
                sms_id: None,
                delivery_attempt_id: None,
                a1_tag: None,
                raw_payload: None,
            },
            page_address: MsPageAddress::Esn(0x1111_1111),
            fwd_address: MsAddress::Esn(0x1111_1111),
            pgslot: Some(100),
            slot_cycle_index: 2,
            started_at: Instant::now(),
            timeout: Duration::from_secs(60),
            retry_count: 0,
            next_retry_at: tokio::time::Instant::now(),
            last_target_chip: None,
            page_msg_seq: None,
            page_correlation_id: None,
        });

        bsc.handle_sms_request(SmsRequest {
            originating_number: "5551234".to_string(),
            text: "pending sms".to_string(),
            target_address: Some("ESN:0x12345678".to_string()),
            target_subscriber_id: Some(subscriber_id),
            timeout_ms: Some(60_000),
            destination_number: Some("5550001".to_string()),
            sms_id: Some(sms_id),
            delivery_attempt_id: None,
            a1_tag: None,
            raw_payload: None,
        });

        // The page queue should still have only the original page
        assert!(
            bsc.paging.has_pending_sms_page(),
            "original page must remain queued"
        );
    });
}

// duplicate_mo_sms_reuses_recent_submission_and_attempt was removed.
// MO SMS is now forwarded to MSC via ADDS Transfer (access) or ADDS Deliver
// (traffic). Deduplication is MSC/SMSC responsibility; BSC no longer
// calls record_or_deliver_mo_sms.

pub struct Frame {
    data: Bitstream,
}

pub struct FrameReader {
    sof: bool,
    bits_remaining: usize,
    data: Vec<u8>,
    message_length: usize,
}

impl FrameReader {
    pub fn new() -> FrameReader {
        FrameReader {
            sof: false,
            bits_remaining: 0,
            data: Vec::new(),
            message_length: 0,
        }
    }

    /// Maximum accumulated message size in bits. The 8-bit MSG_LENGTH
    /// field encodes at most 255 octets = 2040 bits; cap slightly above
    /// that to catch corruption without allocating unboundedly.
    const MAX_MESSAGE_BITS: usize = 2048;

    pub fn process(&mut self, frame: &mut Bitstream) -> Result<Option<Frame>, Error> {
        assert_eq!(32, frame.len());
        let som = frame.read_bits(1)?;

        if som == 1 {
            self.data.clear();
            self.data.extend(frame.bits());
            self.message_length = frame.read_bits(8)? as usize * 8;

            if self.message_length < 38 || self.message_length > Self::MAX_MESSAGE_BITS {
                self.data.clear();
                debug!("skipping invalid message_length={}", self.message_length);
                self.message_length = 0;
            }

            Ok(None)
        } else {
            if self.data.is_empty() || self.message_length == 0 {
                Ok(None)
            } else {
                self.data.extend(frame.bits());

                // Guard against runaway accumulation.
                if self.data.len() > Self::MAX_MESSAGE_BITS {
                    debug!("accumulated data exceeds max, resetting");
                    self.data.clear();
                    self.message_length = 0;
                    return Ok(None);
                }

                if self.data.len() >= self.message_length {
                    let crc = bts_lac::crc30(&Bitstream::new_init(
                        &self.data[0..self.message_length - 30],
                    ));
                    let msg_crc = Bitstream::new_init(
                        &self.data[self.message_length - 30..self.message_length],
                    )
                    .read_bits(30)?;

                    let crc_valid = msg_crc as u32 == crc;
                    debug!("CRC {}", if crc_valid { "GOOD" } else { "BAD" });

                    let result = if crc_valid {
                        Some(Frame {
                            data: Bitstream::new_init(&self.data[8..self.message_length - 30]),
                        })
                    } else {
                        None
                    };

                    self.data.clear();
                    self.message_length = 0;
                    return Ok(result);
                }
                Ok(None)
            }
        }
    }
}

/// Paging Channel Frame Reader that handles SCI (Synchronized Capsule Indicator) field
/// Based on C.S0003-E MAC and C.S0004-E LAC specifications
///
/// Supports both 4800 bps (48 bits per half-frame) and 9600 bps (96 bits per half-frame) rates
/// as specified in the CDMA2000 standards.
pub struct PchFrameReader {
    data: Vec<u8>,
    message_length: usize,
    in_message: bool,
    data_rate: PagingChannelRate,
}

#[derive(Clone, Copy, Debug)]
pub enum PagingChannelRate {
    Rate4800, // 48 bits per half-frame (PCH_FRAME_SIZE=96 ÷ 2)
    Rate9600, // 96 bits per half-frame (PCH_FRAME_SIZE=192 ÷ 2)
}

impl PchFrameReader {
    pub fn new() -> PchFrameReader {
        Self::new_with_rate(PagingChannelRate::Rate9600)
    }

    pub fn new_with_rate(data_rate: PagingChannelRate) -> PchFrameReader {
        PchFrameReader {
            data: Vec::new(),
            message_length: 0,
            in_message: false,
            data_rate,
        }
    }

    fn half_frame_bits(&self) -> usize {
        match self.data_rate {
            PagingChannelRate::Rate4800 => 48, // PCH_FRAME_SIZE=96 ÷ 2
            PagingChannelRate::Rate9600 => 96, // PCH_FRAME_SIZE=192 ÷ 2
        }
    }

    /// Process a paging channel half-frame
    /// - 4800 bps: 48 bits per half-frame (1 SCI bit + 48 payload bits)
    /// - 9600 bps: 96 bits per half-frame (1 SCI bit + 96 payload bits)
    /// Each half-frame starts with SCI (Synchronized Capsule Indicator) bit followed by payload
    pub fn process(&mut self, half_frame: &mut Bitstream) -> Result<Option<Frame>, Error> {
        let expected_bits = self.half_frame_bits();
        assert_eq!(
            expected_bits,
            half_frame.len(),
            "Half-frame length mismatch: expected {} bits for {:?}, got {}",
            expected_bits,
            self.data_rate,
            half_frame.len()
        );

        // Read SCI (Synchronized Capsule Indicator) bit
        let sci = half_frame.read_bits(1)?;
        debug!("SCI: {}", sci);

        if sci == 1 {
            // SCI = 1: Start of synchronized message capsule
            debug!("SCI=1: Starting new synchronized message capsule");
            self.data.clear();
            self.data.extend(half_frame.bits());

            // Read message length from the first 8 bits after SCI
            let mut temp_stream = Bitstream::new_init(half_frame.bits());
            self.message_length = temp_stream.read_bits(8)? as usize * 8;
            debug!("Message length: {} bits", self.message_length);

            if self.message_length < 38 {
                // Minimum: 8-bit length + 30-bit CRC
                debug!("Skipping invalid message length: {}", self.message_length);
                self.data.clear();
                self.in_message = false;
                return Ok(None);
            }

            self.in_message = true;
            Ok(None)
        } else {
            // SCI = 0: Continuation of message or unsynchronized data
            if !self.in_message {
                debug!("SCI=0: Discarding half-frame (no active message)");
                return Ok(None);
            }

            debug!("SCI=0: Continuing message");
            self.data.extend(half_frame.bits());

            // Check if we have enough bits for complete message
            if self.data.len() >= self.message_length {
                debug!(
                    "Complete message received, length: {} bits",
                    self.message_length
                );

                // Calculate and verify CRC30
                let crc = bts_lac::crc30(&Bitstream::new_init(
                    &self.data[0..self.message_length - 30],
                ));
                let msg_crc =
                    Bitstream::new_init(&self.data[self.message_length - 30..self.message_length])
                        .read_bits(30)?;

                debug!("CRC calculated: 0x{:08x}, received: 0x{:08x}", crc, msg_crc);

                let crc_valid = msg_crc as u32 == crc;
                debug!("CRC {}", if crc_valid { "VALID" } else { "INVALID" });

                if !crc_valid {
                    debug!("CRC validation failed, discarding message");
                    self.data.clear();
                    self.in_message = false;
                    return Ok(None);
                }

                // Extract message payload (skip 8-bit length field, exclude 30-bit CRC)
                let frame_data = Frame {
                    data: Bitstream::new_init(&self.data[8..self.message_length - 30]),
                };

                self.data.clear();
                self.in_message = false;

                debug!("Returning valid paging channel frame");
                return Ok(Some(frame_data));
            }

            Ok(None)
        }
    }
}

// ── Welcome SMS tests ────────────────────────────────────────────────
// Welcome SMS is now MSC-owned; BSC only does HLR upsert_mobile_seen.
// The tests below verify the HLR path still fires on registration.

fn build_welcome_test_bsc(
    hlr: Arc<FakeHlrRepository>,
    _sms_tx: mpsc::Sender<SmsRequest>,
    _welcome_cfg: Option<cdma_msc::WelcomeSmsConfig>,
) -> Bsc {
    Bsc::new(Config {
        pilot_offset: 0,
        overhead: OverheadParameters::default(),
        paging: PagingChannelSettings::default(),
        traffic_assignment: TrafficAssignmentConfig::default(),
        access_event_rx: None,
        access_event_broadcast: None,
        sms_request_rx: None,
        sms_request_tx: None,
        data_request_rx: None,
        data_request_tx: None,
        power_override_request_rx: None,
        power_override_request_tx: None,
        mobiles_tx: None,
        paging_broadcast: None,
        traffic_broadcast: None,
        rx_reference_dbm: None,
        hlr_repo: Some(hlr as Arc<dyn HlrRepository>),
        msc_client: test_msc_client(),
        bts_client: None,
        traffic_retry: TrafficRetryConfig::default(),
        paging_retry: PagingRetryConfig::default(),
        voice_policy: test_voice_policy(),
        pcf_client: None,
        mobile_idle_timeout_s: 0,
        bts_paging_state: None,
        node_id: "bsc-test".to_string(),
        msc_voice_bearer: None,
    })
}

fn fake_hlr_for_welcome(mobile_seen: cdma_hlr::MobileSeenUpsert) -> Arc<FakeHlrRepository> {
    let now = chrono::Utc::now();
    let subscriber_id = Uuid::new_v4();
    Arc::new(FakeHlrRepository {
        subscriber: Subscriber {
            subscriber_id,
            phone_number: "5550001".to_string(),
            display_name: "Test".to_string(),
            status: SubscriberStatus::Active,
            created_at: now,
            updated_at: now,
        },
        binding: RegistrationBinding {
            subscriber_id,
            serving_node_id: "bsc-test".to_string(),
            state: RegistrationState::Registered,
            imsi: None,
            esn: Some(0x1234_5678),
            mob_p_rev: Some(6),
            pgslot: Some(1769),
            slot_cycle_index: Some(2),
            last_msg_seq: Some(1),
            last_registered_at: now,
            last_seen_at: now,
            updated_at: now,
        },
        mobile_seen_result: mobile_seen,
    })
}

fn registration_event() -> AccessChannelEvent {
    let mut evt = test_access_event();
    evt.message_id = MessageId::Registration;
    evt.msg_type_name = "Registration Message".to_string();
    evt.msg_seq = Some(1);
    evt.ack_req = true;
    evt.esn = Some(0x1234_5678);
    evt.imsi_m_s1 = Some(0x0091_989e);
    evt.imsi_m_s2 = Some(0x0326);
    evt.imsi_class = Some(0);
    evt.imsi_mcc = Some(310);
    evt.imsi_11_12 = Some(99);
    evt.mob_p_rev = Some(6);
    evt.slot_cycle_index = Some(2);
    evt
}

async fn drain_hlr_and_apply(bsc: &mut Bsc) {
    tokio::time::sleep(Duration::from_millis(50)).await;
    while let Ok(resolution) = bsc.hlr_result_rx.try_recv() {
        bsc.apply_hlr_resolution(resolution);
    }
    tokio::time::sleep(Duration::from_millis(50)).await;
}

/// Welcome SMS is now MSC-owned. BSC still calls upsert_mobile_seen on
/// registration so the HLR sightings ledger stays current.
#[tokio::test]
async fn registration_upserts_mobile_seen_in_hlr() {
    let hlr = fake_hlr_for_welcome(cdma_hlr::MobileSeenUpsert {
        is_new: true,
        previous_last_seen_at: None,
    });
    let (sms_tx, _sms_rx) = mpsc::channel(4);
    let mut bsc = build_welcome_test_bsc(hlr, sms_tx, None);

    bsc.inject_access_event(registration_event()).await;
    drain_hlr_and_apply(&mut bsc).await;

    // The mobile should be registered and resolved via HLR.
    assert_eq!(
        bsc.mobiles.tracked_count(),
        1,
        "registration should add a mobile"
    );
    assert!(
        bsc.mobiles[0].phone_number.is_some(),
        "HLR resolution should populate phone_number"
    );
}
