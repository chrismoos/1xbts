//! TCP-backed [`BtsControlClient`] that speaks Abis control messages
//! over the spec transport (A.S0003-A §4.5.6.4).
//!
//! Each trait method encodes the appropriate Abis control message(s),
//! sends them through the `TransportSender`, waits for the expected
//! response(s) from the BTS Abis agent, and returns the result.

use std::collections::{HashMap, VecDeque};
use std::net::SocketAddr;
use std::sync::{
    Arc, Mutex as StdMutex,
    atomic::{AtomicU64, Ordering},
};
use std::time::Duration;

use async_trait::async_trait;
use log::{info, warn};
use tokio::net::TcpListener;
use tokio::sync::{Mutex, mpsc, oneshot};

use cdma_abis::bearer::TrafficFrame;
use cdma_abis::bearer_transport::BearerTransport;
use cdma_abis::control::typed::{
    A3RemoveInformation, BurstCommitMessage, BurstRequestMessage, BurstResponseMessage,
    CdmaServingOneWayDelay, CellId, CellIdWithMscId, ForwardBurstRadioInfo, MobileIdentity,
    PhysicalChannelInfo, PhysicalChannelType, PilotGatingRate,
};
use cdma_abis::control::{
    AbisMessage, AchMessageTransferMessage, BtsReleaseMessage, BtsSetupAckMessage, BtsSetupMessage,
    CallConnectionReference, ConnectAckMessage, ConnectMessage, MessageType, RemoveMessage, decode,
    encode,
};
use cdma_abis::transport::{TransportEvent, TransportSender};
use cdma_abis::udp_bearer::UdpBearerDatagram;

use cdma_bts::bts::TrafficResourceService;
use cdma_common::events::AccessChannelEvent;
use cdma_common::phy::long_code::LongCodeGenerator;
use cdma_common::traffic::TrafficRxRequest;

use super::{
    BearerFrame, BearerStats, BtsBearerClient, BtsControlClient, BtsTrafficChannelHandle,
    ForwardBearerQueue, PchTransferAckEvent,
};

const FLAG_SIGNALING_QUEUE: u8 = 0x01;

/// Pending request waiting for a BTS response keyed by CCR.
struct PendingSetup {
    tx: oneshot::Sender<SetupResult>,
}

struct PendingBurst {
    tx: oneshot::Sender<ForwardBurstRadioInfo>,
}

struct SetupResult {
    walsh_code: u8,
    /// Setup-time SCH code, if returned by legacy transports.
    sch_walsh_code: Option<u8>,
}

/// Network-backed Abis control client.
///
/// Connects to a BTS Abis agent over TCP port 5604, encodes BSC commands
/// as Abis control messages, and processes BTS responses.
pub struct NetworkBtsControlClient {
    sender: TransportSender,
    inner: Arc<Mutex<NetworkClientInner>>,
    config: NetworkClientConfig,
    bearer: Option<TransportBearerClient>,
    /// Direct controller access for in-process deployments (bearer routing,
    /// queue inspection). `None` for TCP-backed clients.
    local_controller: Option<Arc<TrafficResourceService>>,
    local_bearer: Option<LocalBearerClient>,
    _shutdown_tx: tokio::sync::watch::Sender<bool>,
}

/// Configuration for the network client.
#[derive(Debug, Clone)]
pub struct NetworkClientConfig {
    /// Cell identifier for this BTS.
    pub cell_id: CellId,
    /// MSC identifier.
    pub mscid: u32,
    /// Pilot PN offset.
    pub pilot_pn: u16,
    /// System AUTH_MODE from the serving overhead state.
    pub auth_mode: u8,
    /// Serving P_REV_IN_USE for exact reverse access-channel L3 decode.
    pub p_rev_in_use: u8,
    /// Market ID for CCR generation.
    pub market_id: u16,
    /// Generating entity ID for CCR generation.
    pub generating_entity_id: u16,
}

struct NetworkClientInner {
    next_ccr: u32,
    pending_setups: HashMap<CallConnectionReference, PendingSetup>,
    pending_bursts: HashMap<CallConnectionReference, PendingBurst>,
    pending_releases: HashMap<CallConnectionReference, oneshot::Sender<()>>,
    walsh_to_ccr: HashMap<u8, CallConnectionReference>,
    access_event_tx: Option<mpsc::UnboundedSender<AccessChannelEvent>>,
    pch_ack_events: VecDeque<PchTransferAckEvent>,
}

impl NetworkBtsControlClient {
    /// Connect to a remote BTS Abis agent.
    pub async fn connect(addr: SocketAddr, config: NetworkClientConfig) -> std::io::Result<Self> {
        let (sender, events_rx) = cdma_abis::transport::connect(addr).await?;
        Ok(Self::from_transport(sender, events_rx, config))
    }

    /// Connect to a remote BTS Abis agent with a pre-built bearer transport.
    pub async fn connect_with_bearer(
        addr: SocketAddr,
        config: NetworkClientConfig,
        transport: Arc<BearerTransport>,
    ) -> std::io::Result<Self> {
        let (sender, events_rx) = cdma_abis::transport::connect(addr).await?;
        let mut client = Self::from_transport(sender, events_rx, config);
        client.bearer = Some(TransportBearerClient::new(transport));
        Ok(client)
    }

    /// Connect to a remote BTS Abis agent with bearer transport and access event forwarding.
    pub async fn connect_with_bearer_and_access(
        addr: SocketAddr,
        config: NetworkClientConfig,
        transport: Arc<BearerTransport>,
        access_event_tx: mpsc::UnboundedSender<AccessChannelEvent>,
    ) -> std::io::Result<Self> {
        let (sender, events_rx) = cdma_abis::transport::connect(addr).await?;
        let mut client =
            Self::from_transport_with_access(sender, events_rx, config, Some(access_event_tx));
        client.bearer = Some(TransportBearerClient::new(transport));
        Ok(client)
    }

    /// Accept a connection from a BTS on the given listener.
    pub async fn accept(
        listener: &TcpListener,
        config: NetworkClientConfig,
    ) -> std::io::Result<Self> {
        let (sender, events_rx) = cdma_abis::transport::accept(listener).await?;
        Ok(Self::from_transport(sender, events_rx, config))
    }

    /// Build a client from an already-established transport pair.
    pub fn from_transport(
        sender: TransportSender,
        events_rx: mpsc::Receiver<TransportEvent>,
        config: NetworkClientConfig,
    ) -> Self {
        Self::from_transport_with_access(sender, events_rx, config, None)
    }

    /// Build a client from a transport pair with access-event forwarding.
    pub fn from_transport_with_access(
        sender: TransportSender,
        events_rx: mpsc::Receiver<TransportEvent>,
        config: NetworkClientConfig,
        access_event_tx: Option<mpsc::UnboundedSender<AccessChannelEvent>>,
    ) -> Self {
        let inner = Arc::new(Mutex::new(NetworkClientInner {
            next_ccr: 1,
            pending_setups: HashMap::new(),
            pending_bursts: HashMap::new(),
            pending_releases: HashMap::new(),
            walsh_to_ccr: HashMap::new(),
            access_event_tx,
            pch_ack_events: VecDeque::new(),
        }));
        let (shutdown_tx, shutdown_rx) = tokio::sync::watch::channel(false);

        let inner_clone = inner.clone();
        let event_config = config.clone();
        let sender_clone = sender.clone();
        tokio::spawn(async move {
            Self::event_loop(
                events_rx,
                inner_clone,
                sender_clone,
                event_config,
                shutdown_rx,
            )
            .await;
        });

        Self {
            sender,
            inner,
            config,
            bearer: None,
            local_controller: None,
            local_bearer: None,
            _shutdown_tx: shutdown_tx,
        }
    }

    /// Create an in-process client backed by a channel transport and a local
    /// [`AbisAgent`]. The agent runs in a background tokio task, processing
    /// Abis messages identically to the TCP path — same two-phase allocation,
    /// same ECAM commit, same PCH handling.
    ///
    /// [`AbisAgent`]: cdma_bts::bts::abis_agent::AbisAgent
    /// Create an in-process client backed by a channel transport and a local
    /// [`AbisAgent`]. The agent runs in a background tokio task, processing
    /// Abis messages identically to the TCP path.
    ///
    /// Bearer frames and queue inspection go directly through the shared
    /// `TrafficResourceService`, bypassing Abis (same as the old
    /// `InProcessBtsControlClient`).
    ///
    /// [`AbisAgent`]: cdma_bts::bts::abis_agent::AbisAgent
    pub fn spawn_in_process(
        controller: Arc<TrafficResourceService>,
        agent_config: cdma_bts::bts::abis_agent::AbisAgentConfig,
        config: NetworkClientConfig,
    ) -> Self {
        use cdma_abis::transport::spawn_channel_transport;
        use cdma_bts::bts::abis_agent::AbisAgent;

        let (client_sender, client_events, server_sender, mut server_events) =
            spawn_channel_transport();

        let controller_for_agent = controller.clone();
        tokio::spawn(async move {
            let mut agent = AbisAgent::new(agent_config, controller_for_agent);
            while let Some(event) = server_events.recv().await {
                match event {
                    cdma_abis::transport::TransportEvent::Message(msg) => {
                        let (responses, _events) = agent.handle_message(&msg);
                        for resp in responses {
                            if server_sender.send(&resp).await.is_err() {
                                return;
                            }
                        }
                    }
                    cdma_abis::transport::TransportEvent::Disconnected(_) => return,
                }
            }
        });

        let mut client = Self::from_transport(client_sender, client_events, config);
        client.local_bearer = Some(LocalBearerClient {
            controller: controller.clone(),
            received_frames: StdMutex::new(VecDeque::new()),
        });
        client.local_controller = Some(controller);
        client
    }

    /// When running in-process, send an ECAM via PchMessageTransfer to commit
    /// the reserved walsh code through the AbisAgent's two-phase flow.
    fn send_ecam_commit(&self, walsh_code: u8, for_rc: u8, rev_rc: u8) {
        use cdma_abis::control::typed::{AirInterfaceMessagePayload, PchMessageTransferMessage};
        use cdma_common::lac::paging_messages::ExtendedChannelAssignmentMessage;

        let ecam = ExtendedChannelAssignmentMessage::new_f_fch_r_fch_assignment(
            0, walsh_code, 0, for_rc, rev_rc, false,
        );
        let sdu = ecam.to_sdu();
        let sdu_bytes = sdu.to_packed_bytes();
        let aim = AirInterfaceMessagePayload::new(0x15, sdu_bytes).unwrap();
        let pch = PchMessageTransferMessage {
            correlation_id: None,
            mobile_identities: vec![MobileIdentity::Esn(0)],
            cell_identifier_list: None,
            air_interface_message: Some(aim),
            layer2_ack_request_results: None,
            abis_ack_notify: None,
        };
        let _ = self.send_pch_message(pch);
    }

    /// Poll until the committed channel appears in the traffic pool.
    async fn wait_for_commit(&self, walsh_code: u8) {
        let ctrl = self.local_controller.as_ref().unwrap();
        for _ in 0..50 {
            tokio::task::yield_now().await;
            let found = ctrl
                .traffic_channels_pool()
                .lock()
                .iter()
                .any(|s| s.walsh_code == walsh_code);
            if found {
                return;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        warn!(
            "abis_network: ECAM commit for walsh={} did not appear in pool within timeout",
            walsh_code
        );
    }

    fn allocate_ccr(&self, inner: &mut NetworkClientInner) -> CallConnectionReference {
        let ccr = CallConnectionReference {
            market_id: self.config.market_id,
            generating_entity_id: self.config.generating_entity_id,
            call_connection_reference: inner.next_ccr,
        };
        inner.next_ccr += 1;
        ccr
    }

    async fn event_loop(
        mut events_rx: mpsc::Receiver<TransportEvent>,
        inner: Arc<Mutex<NetworkClientInner>>,
        sender: TransportSender,
        config: NetworkClientConfig,
        mut shutdown_rx: tokio::sync::watch::Receiver<bool>,
    ) {
        loop {
            tokio::select! {
                event = events_rx.recv() => {
                    match event {
                        Some(TransportEvent::Message(msg)) => {
                            Self::handle_bts_response(&inner, &sender, &config, &msg).await;
                        }
                        Some(TransportEvent::Disconnected(e)) => {
                            warn!("abis_network: BTS disconnected: {e}");
                            break;
                        }
                        None => break,
                    }
                }
                _ = shutdown_rx.changed() => {
                    info!("abis_network: shutdown requested");
                    break;
                }
            }
        }
    }

    async fn handle_bts_response(
        inner: &Arc<Mutex<NetworkClientInner>>,
        sender: &TransportSender,
        config: &NetworkClientConfig,
        msg: &AbisMessage,
    ) {
        match msg.message_type {
            MessageType::Connect => {
                let Ok(bytes) = encode(msg) else { return };
                let Ok(connect) = ConnectMessage::decode(&bytes) else {
                    return;
                };
                let ccr = connect.call_connection_reference;
                info!("abis_network: received Connect for CCR {:?}", ccr);

                // Extract the FCH walsh by matching on PhysicalChannelType so a
                // BtsSetup that requested both Fch and Sch surfaces both codes
                // unambiguously regardless of order.
                let extract_code = |ty: PhysicalChannelType| -> Option<u8> {
                    connect
                        .connect_information
                        .iter()
                        .find(|ci| ci.physical_channel_type == ty)
                        .and_then(|ci| ci.cell_info_records.first())
                        .map(|r| r.code_channel)
                };
                let walsh_code = extract_code(PhysicalChannelType::Fch).unwrap_or_else(|| {
                    // Fallback: legacy BTS responding without explicit
                    // physical_channel_type tagging — take the first entry.
                    connect
                        .connect_information
                        .first()
                        .and_then(|ci| ci.cell_info_records.first())
                        .map(|r| r.code_channel)
                        .unwrap_or(0)
                });
                let sch_walsh_code = extract_code(PhysicalChannelType::Sch);

                let ack = ConnectAckMessage {
                    call_connection_reference: ccr,
                    correlation_id: connect.correlation_id,
                    connect_ack_information: connect
                        .connect_information
                        .iter()
                        .map(|ci| cdma_abis::control::typed::A3ConnectAckInformation {
                            soft_handoff_leg: 0,
                            pmc_cause: None,
                            transmit_tch_status: false,
                            traffic_circuit_id: ci.traffic_circuit_id,
                            channel_element_id: ci.channel_element_id.clone(),
                            a3_originating_id: ci.a3_originating_id,
                            a3_destination_id: 1,
                        })
                        .collect(),
                };
                if let Ok(ack_bytes) = ack.encode() {
                    if let Ok(ack_msg) = decode(&ack_bytes) {
                        let _ = sender.send(&ack_msg).await;
                    }
                }

                let mut guard = inner.lock().await;
                if let Some(pending) = guard.pending_setups.remove(&ccr) {
                    let _ = pending.tx.send(SetupResult {
                        walsh_code,
                        sch_walsh_code,
                    });
                }
            }
            MessageType::BtsSetupAck => {
                let Ok(bytes) = encode(msg) else { return };
                let Ok(ack) = BtsSetupAckMessage::decode(&bytes) else {
                    return;
                };
                info!(
                    "abis_network: received BtsSetupAck for CCR {:?}",
                    ack.call_connection_reference
                );
            }
            MessageType::TrafficChannelStatus => {
                info!("abis_network: received TrafficChannelStatus");
            }
            MessageType::BtsReleaseAck => {
                info!("abis_network: received BtsReleaseAck");
            }
            MessageType::BurstResponse => {
                let Ok(bytes) = encode(msg) else { return };
                let Ok(response) = BurstResponseMessage::decode(&bytes) else {
                    return;
                };
                let Some(ccr) = response.call_connection_reference else {
                    return;
                };
                let Some(info) = response.forward_burst_radio_info else {
                    warn!(
                        "abis_network: BurstResponse for CCR {:?} missing ForwardBurstRadioInfo",
                        ccr
                    );
                    return;
                };
                info!(
                    "abis_network: received BurstResponse for CCR {:?} sch_code={} rate_idx={}",
                    ccr, info.forward_code_channel_index, info.forward_supplemental_channel_rate
                );
                let commit = BurstCommitMessage {
                    call_connection_reference: Some(ccr),
                    correlation_id: response.correlation_id,
                    forward_cell_identifier_list: response.committed_cell_identifier_list.clone(),
                    reverse_cell_identifier_list: Some(Vec::new()),
                    forward_burst_radio_info: Some(info),
                    reverse_burst_radio_info: None,
                    is2000_forward_power_control_mode: None,
                    is2000_fpc_gain_ratio_info: None,
                    abis_destination_id: response.abis_destination_id.clone(),
                };
                if let Ok(commit_bytes) = commit.encode()
                    && let Ok(commit_msg) = decode(&commit_bytes)
                {
                    let _ = sender.send(&commit_msg).await;
                }
                let mut guard = inner.lock().await;
                if let Some(pending) = guard.pending_bursts.remove(&ccr) {
                    let _ = pending.tx.send(info);
                }
            }
            MessageType::RemoveAck => {
                let Ok(bytes) = encode(msg) else { return };
                let Ok(ack) = cdma_abis::control::RemoveAckMessage::decode(&bytes) else {
                    return;
                };
                let ccr = ack.call_connection_reference;
                info!("abis_network: received RemoveAck for CCR {:?}", ccr);
                let mut guard = inner.lock().await;
                if let Some(tx) = guard.pending_releases.remove(&ccr) {
                    let _ = tx.send(());
                }
            }
            MessageType::PchMessageTransferAck => {
                let Ok(bytes) = encode(msg) else { return };
                let Ok(ack) =
                    cdma_abis::control::typed::PchMessageTransferAckMessage::decode(&bytes)
                else {
                    warn!("abis_network: failed to decode PchMsgTransferAck");
                    return;
                };
                info!(
                    "abis_network: received PchMsgTransferAck corr={:?} cause={:?} bts_l2_termination={:?}",
                    ack.correlation_id.map(|c| c.0),
                    ack.cause,
                    ack.bts_l2_termination
                );
                let mut guard = inner.lock().await;
                guard.pch_ack_events.push_back(PchTransferAckEvent {
                    correlation_id: ack.correlation_id.map(|c| c.0),
                    cause: ack.cause,
                    bts_l2_termination: ack.bts_l2_termination,
                });
            }
            MessageType::AchMessageTransfer => {
                let Ok(bytes) = encode(msg) else { return };
                let Ok(ach) = AchMessageTransferMessage::decode(&bytes) else {
                    warn!("abis_network: failed to decode AchMessageTransfer");
                    return;
                };
                let event = match Self::ach_to_access_event(&ach, config) {
                    Ok(event) => event,
                    Err(e) => {
                        warn!(
                            "abis_network: failed to convert AchMessageTransfer to access event: {e}"
                        );
                        return;
                    }
                };
                info!(
                    "abis_network: received AchMessageTransfer type=\"{}\" addr={}",
                    event.msg_type_name,
                    event.address.as_deref().unwrap_or("none"),
                );
                let guard = inner.lock().await;
                if let Some(ref tx) = guard.access_event_tx {
                    if tx.send(event).is_err() {
                        warn!("abis_network: access event channel closed");
                    }
                }
            }
            other => {
                warn!("abis_network: unexpected message type {:?}", other);
            }
        }
    }

    /// Convert an Abis ACH Message Transfer into an `AccessChannelEvent`.
    ///
    /// Re-decodes the IS-2000 PDU from the Air Interface Message IE to extract
    /// all L3, ARQ, and addressing fields. Fields not carried by the Abis
    /// message (RX measurements: snr_db, signal_power_db, raw_power_db,
    /// demod_quality_pct; timing: chip_start, absolute_chip_start, rx_wall_time,
    /// rx_hw_time_ns) are set to None/default. The BSC uses wall-clock time for
    /// T56 response scheduling, which is sufficient on localhost TCP.
    fn ach_to_access_event(
        ach: &AchMessageTransferMessage,
        config: &NetworkClientConfig,
    ) -> Result<AccessChannelEvent, String> {
        use cdma_bts::receiver::access_pdu::ReverseAccessPdu;
        use cdma_common::access::{AccessDecodeContext, AccessMessage};
        use cdma_common::bits::Bitstream;
        use cdma_common::lac::message_types::{MessageId, WireChannel};

        let now = chrono::Utc::now();
        let receive_time = Some(cdma_common::time::CdmaSystemTime::from(now));

        let esn = ach.mobile_identities.iter().find_map(|id| match id {
            MobileIdentity::Esn(e) => Some(*e),
            _ => None,
        });
        let mobile_identity_imsi = ach.mobile_identities.iter().find_map(|id| match id {
            MobileIdentity::Imsi(imsi) => Some(imsi.clone()),
            _ => None,
        });

        let raw_msg_type = ach
            .air_interface_message
            .as_ref()
            .map(|aim| aim.message_type)
            .unwrap_or(0);

        let payload_bits: Vec<u8> = ach
            .air_interface_message
            .as_ref()
            .map(|aim| {
                let mut bits = Vec::with_capacity(aim.message.len() * 8);
                for byte in &aim.message {
                    for bit_idx in (0..8).rev() {
                        bits.push((byte >> bit_idx) & 1);
                    }
                }
                bits
            })
            .unwrap_or_default();

        let msg_type_id = MessageId::from_wire(WireChannel::ReverseCommon, raw_msg_type)
            .ok_or_else(|| format!("unsupported reverse-common MSG_TAG 0x{raw_msg_type:02x}"))?;

        let decode_ctx =
            AccessDecodeContext::new(Some(config.auth_mode), Some(config.p_rev_in_use));
        let bs = Bitstream::new_init(&payload_bits);
        let pdu = ReverseAccessPdu::decode(&bs)
            .map_err(|err| format!("access PDU decode failed: {err}"))?;
        let decoded_l3 = Self::decode_access_l3_from_pdu(&pdu, decode_ctx)?;
        let address = cdma_bts::bts::rx::extract_address(&pdu);
        let l3_summary = Some(decoded_l3.summary());
        let pdu_summary = pdu.summary();

        let (
            arq_msg_seq,
            arq_ack_seq,
            arq_ack_req,
            arq_valid_ack,
            msid_type,
            pdu_esn,
            imsi_m_s1,
            imsi_m_s2,
            imsi_class,
            imsi_addr_num,
            imsi_mcc,
            imsi_11_12,
        ) = match &pdu {
            ReverseAccessPdu::Pd01PRev6(p) => {
                let msg_seq = p.arq.as_ref().map(|a| a.msg_seq);
                let ack_seq = p.arq.as_ref().map(|a| a.ack_seq);
                let ack_req = p.arq.as_ref().map_or(false, |a| a.ack_req);
                let valid_ack = p.arq.as_ref().map_or(false, |a| a.valid_ack);
                let ea = cdma_bts::bts::rx::extract_addressing_fields(p.addressing.as_ref());
                (
                    msg_seq,
                    ack_seq,
                    ack_req,
                    valid_ack,
                    ea.msid_type,
                    ea.esn,
                    ea.imsi_m_s1,
                    ea.imsi_m_s2,
                    ea.imsi_class,
                    ea.imsi_addr_num,
                    ea.mcc,
                    ea.imsi_11_12,
                )
            }
            ReverseAccessPdu::Pd00Legacy(p) => {
                let msg_seq = p.arq.as_ref().map(|a| a.msg_seq);
                let ack_seq = p.arq.as_ref().map(|a| a.ack_seq);
                let ack_req = p.arq.as_ref().map_or(false, |a| a.ack_req);
                let valid_ack = p.arq.as_ref().map_or(false, |a| a.valid_ack);
                let ea = cdma_bts::bts::rx::extract_addressing_fields(p.addressing.as_ref());
                (
                    msg_seq,
                    ack_seq,
                    ack_req,
                    valid_ack,
                    ea.msid_type,
                    ea.esn,
                    ea.imsi_m_s1,
                    ea.imsi_m_s2,
                    ea.imsi_class,
                    ea.imsi_addr_num,
                    ea.mcc,
                    ea.imsi_11_12,
                )
            }
            _ => (
                None, None, false, false, None, None, None, None, None, None, None, None,
            ),
        };

        let pd = match &pdu {
            ReverseAccessPdu::Pd00Legacy(_) => 0u8,
            ReverseAccessPdu::Pd01PRev6(_) => 1u8,
            ReverseAccessPdu::Pd10Modern { .. } => 2u8,
        };

        let mob_p_rev = decoded_l3.mob_p_rev();
        let slot_cycle_index = decoded_l3.slot_cycle_index();
        let scm = decoded_l3.scm();
        let service_option = decoded_l3.service_option();

        let (for_rc_pref, rev_rc_pref) = match &decoded_l3 {
            AccessMessage::Origination(m) => Some((m.for_rc_pref, m.rev_rc_pref)),
            AccessMessage::PageResponse(m) => Some((m.for_rc_pref, m.rev_rc_pref)),
            _ => None,
        }
        .unwrap_or((None, None));

        let rev_fch_gating_req = match &decoded_l3 {
            AccessMessage::Origination(m) => m.rev_fch_gating_req,
            AccessMessage::PageResponse(m) => m.rev_fch_gating_req,
            _ => None,
        };

        let order_code = decoded_l3.order_code();

        let (for_supported_rcs, rev_supported_rcs) = (
            decoded_l3.for_supported_rcs(),
            decoded_l3.rev_supported_rcs(),
        );

        let msg_type_name = cdma_common::access::access_message_type_name(raw_msg_type).to_string();

        Ok(AccessChannelEvent {
            event_id: uuid::Uuid::new_v4().to_string(),
            chip_start: 0,
            absolute_chip_start: None,
            receive_time,
            preamble_frames: 0,
            pd,
            message_id: msg_type_id,
            msg_type_name,
            address,
            resolved_address: None,
            subscriber_id: None,
            l3_summary,
            decoded_l3: Some(decoded_l3),
            pdu_summary,
            msg_seq: arq_msg_seq,
            ack_seq: arq_ack_seq,
            ack_req: arq_ack_req,
            valid_ack: arq_valid_ack,
            msid_type,
            esn: pdu_esn.or(esn),
            imsi: mobile_identity_imsi,
            imsi_m_s1,
            imsi_m_s2,
            imsi_class,
            imsi_addr_num,
            imsi_mcc,
            imsi_11_12,
            mob_p_rev,
            slot_cycle_index,
            scm,
            burst_type: None,
            data_burst_fields: None,
            data_burst_num_msgs: None,
            data_burst_msg_number: None,
            wall_clock_us: now.timestamp_micros() as u64,
            rx_wall_time: Some(std::time::Instant::now()),
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
            order_code,
            service_option,
            for_rc_pref,
            rev_rc_pref,
            rev_fch_gating_req,
            traffic_walsh_code: None,
            is_preamble_only: false,
            is_traffic_pcg_measurement: false,
            is_traffic_phy_status: false,
            traffic_measurement_age_chips: None,
            for_supported_rcs,
            rev_supported_rcs,
            decoded_rdsch: None,
            traffic_primary_bits: None,
            traffic_primary_rate_bps: None,
            traffic_primary_bearer_routed: false,
            traffic_voice_bits: None,
            traffic_voice_rate_bps: None,
            raw_pdu_bits: Some(payload_bits),
        })
    }

    fn decode_access_l3_from_pdu(
        pdu: &cdma_bts::receiver::access_pdu::ReverseAccessPdu,
        ctx: cdma_common::access::AccessDecodeContext,
    ) -> Result<cdma_common::access::AccessMessage, String> {
        use cdma_bts::receiver::access_pdu::ReverseAccessPdu;
        use cdma_common::access::{AccessMessage, AccessMessageHeader};
        use cdma_common::lac::message_types::{MessageId, WireChannel};

        match pdu {
            ReverseAccessPdu::Pd01PRev6(p) => {
                let message_id =
                    MessageId::from_wire(WireChannel::ReverseCommon, p.header.msg_type)
                        .ok_or_else(|| {
                            format!(
                                "unsupported reverse-common MSG_TAG 0x{:02x} in decoded PDU",
                                p.header.msg_type
                            )
                        })?;
                AccessMessage::decode_sdu_with_context(
                    AccessMessageHeader {
                        pd: p.header.pd,
                        message_id,
                    },
                    &p.sdu_plus_padding_raw,
                    ctx,
                )
                .map_err(|err| format!("access Layer 3 decode failed: {err}"))
            }
            ReverseAccessPdu::Pd00Legacy(p) => {
                let message_id =
                    MessageId::from_wire(WireChannel::ReverseCommon, p.header.msg_type)
                        .ok_or_else(|| {
                            format!(
                                "unsupported reverse-common MSG_TAG 0x{:02x} in decoded PDU",
                                p.header.msg_type
                            )
                        })?;
                AccessMessage::decode_sdu_with_context(
                    AccessMessageHeader {
                        pd: p.header.pd,
                        message_id,
                    },
                    &p.sdu_plus_padding_raw,
                    ctx,
                )
                .map_err(|err| format!("access Layer 3 decode failed: {err}"))
            }
            ReverseAccessPdu::Pd10Modern { .. } => Err(
                "access Layer 3 decode failed: PD=10 reverse-common PDU body is unsupported"
                    .to_string(),
            ),
        }
    }

    async fn send_bts_setup(
        &self,
        ccr: CallConnectionReference,
        esn: u32,
        include_sch: bool,
    ) -> Option<SetupResult> {
        if include_sch {
            log::info!(
                "abis_edge: ignoring legacy setup-time SCH request; SCH uses Abis Burst allocation"
            );
        }
        let physical_channels = vec![PhysicalChannelType::Fch];
        let setup = BtsSetupMessage {
            call_connection_reference: ccr,
            band_class: None,
            privacy_info: None,
            sdu_id: None,
            mobile_identities: vec![MobileIdentity::Esn(esn)],
            physical_channel_info: Some(PhysicalChannelInfo {
                frame_offset: 0,
                pilot_gating_rate: PilotGatingRate::Full,
                arfcn: 0,
                otd: false,
                physical_channels,
            }),
            service_option: None,
            paca_timestamp: None,
            quality_of_service_parameters: None,
            connect_information: Vec::new(),
            abis_originating_id: None,
            cdma_serving_one_way_delay: CdmaServingOneWayDelay {
                cell: self.config.cell_id,
                delay_100ns: 0,
            },
            cdma_target_one_way_delay: None,
            walsh_code_assignment_request: true,
        };
        let bytes = setup.encode().ok()?;
        let msg = decode(&bytes).ok()?;

        let (tx, rx) = oneshot::channel();
        {
            let mut guard = self.inner.lock().await;
            guard.pending_setups.insert(ccr, PendingSetup { tx });
        }

        if self.sender.send(&msg).await.is_err() {
            let mut guard = self.inner.lock().await;
            guard.pending_setups.remove(&ccr);
            return None;
        }

        match tokio::time::timeout(Duration::from_secs(5), rx).await {
            Ok(Ok(result)) => Some(result),
            _ => {
                let mut guard = self.inner.lock().await;
                guard.pending_setups.remove(&ccr);
                None
            }
        }
    }

    async fn send_release_and_remove(&self, ccr: CallConnectionReference) {
        let release = BtsReleaseMessage {
            call_connection_reference: ccr,
            cell_identifier_list: None,
            correlation_id: None,
        };
        if let Ok(bytes) = release.encode() {
            if let Ok(msg) = decode(&bytes) {
                let _ = self.sender.send(&msg).await;
            }
        }

        tokio::time::sleep(Duration::from_millis(10)).await;

        let remove = RemoveMessage {
            call_connection_reference: ccr,
            correlation_id: None,
            sdu_id: None,
            remove_information: vec![A3RemoveInformation {
                traffic_circuit_id: cdma_abis::control::typed::TrafficCircuitId {
                    traffic_circuit_identifier: 0,
                    traffic_connection_identifier: 0,
                },
                cells_to_be_removed: vec![CellIdWithMscId {
                    mscid: self.config.mscid,
                    cell: self.config.cell_id.cell,
                    sector: self.config.cell_id.sector,
                }],
                a3_destination_id: 1,
                a7_destination_id: 0,
            }],
        };
        let (tx, rx) = oneshot::channel();
        {
            let mut guard = self.inner.lock().await;
            guard.pending_releases.insert(ccr, tx);
        }
        if let Ok(bytes) = remove.encode() {
            if let Ok(msg) = decode(&bytes) {
                let _ = self.sender.send(&msg).await;
            }
        }
        let _ = tokio::time::timeout(Duration::from_secs(5), rx).await;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use cdma_abis::control::typed::{AirInterfaceMessagePayload, CdmaServingOneWayDelay};

    fn test_config() -> NetworkClientConfig {
        NetworkClientConfig {
            cell_id: CellId { cell: 1, sector: 0 },
            mscid: 1,
            pilot_pn: 42,
            auth_mode: 0,
            p_rev_in_use: 6,
            market_id: 1,
            generating_entity_id: 1,
        }
    }

    #[test]
    fn ach_to_access_event_rejects_unmapped_reverse_common_msg_tag() {
        let ach = AchMessageTransferMessage {
            correlation_id: None,
            mobile_identities: Vec::new(),
            cell_identifier: None,
            bts_l2_termination: None,
            air_interface_message: Some(AirInterfaceMessagePayload {
                message_type: 0x0b,
                message: vec![0],
            }),
            cdma_serving_one_way_delay: CdmaServingOneWayDelay {
                cell: CellId { cell: 1, sector: 0 },
                delay_100ns: 0,
            },
            authentication_challenge_parameter: None,
        };

        let err = NetworkBtsControlClient::ach_to_access_event(&ach, &test_config()).unwrap_err();

        assert!(err.contains("unsupported reverse-common MSG_TAG"));
    }

    #[test]
    fn ach_to_access_event_rejects_layer3_decode_failure() {
        let mut pdu_bits = cdma_common::bits::Bitstream::new();
        pdu_bits.write_u8(0x44, 8); // PD=01, MSG_TAG=Origination
        pdu_bits.write_u8(2, 5); // LAC_LENGTH
        pdu_bits.write_u8(0b101, 3); // ACK_SEQ
        pdu_bits.write_u8(0b011, 3); // MSG_SEQ
        pdu_bits.write_u8(1, 1); // ACK_REQ
        pdu_bits.write_u8(0, 1); // VALID_ACK
        pdu_bits.write_u8(0b010, 3); // ACK_TYPE
        pdu_bits.write_u8(7, 6); // ACTIVE_PILOT_STRENGTH
        pdu_bits.write_u8(1, 1); // FIRST_IS_ACTIVE
        pdu_bits.write_u8(0, 1); // FIRST_IS_PTA
        pdu_bits.write_u8(0, 3); // NUM_ADD_PILOTS
        pdu_bits.write_u8(0xa5, 8); // Truncated Origination SDU

        let ach = AchMessageTransferMessage {
            correlation_id: None,
            mobile_identities: Vec::new(),
            cell_identifier: None,
            bts_l2_termination: None,
            air_interface_message: Some(AirInterfaceMessagePayload {
                message_type: 0x04,
                message: pdu_bits.to_packed_bytes(),
            }),
            cdma_serving_one_way_delay: CdmaServingOneWayDelay {
                cell: CellId { cell: 1, sector: 0 },
                delay_100ns: 0,
            },
            authentication_challenge_parameter: None,
        };

        let err = NetworkBtsControlClient::ach_to_access_event(&ach, &test_config()).unwrap_err();

        assert!(err.contains("access Layer 3 decode failed"));
    }
}

/// BSC-side bearer client backed by a [`BearerTransport`].
pub struct TransportBearerClient {
    transport: Arc<BearerTransport>,
    bearer_sequence: AtomicU64,
}

impl TransportBearerClient {
    pub fn new(transport: Arc<BearerTransport>) -> Self {
        Self {
            transport,
            bearer_sequence: AtomicU64::new(0),
        }
    }

    fn next_bearer_sequence(&self) -> u32 {
        self.bearer_sequence.fetch_add(1, Ordering::Relaxed) as u32
    }
}

impl BtsBearerClient for TransportBearerClient {
    fn send_frame(&self, frame: BearerFrame) -> Result<(), String> {
        let payload = frame
            .traffic_frame
            .encode()
            .map_err(|e| format!("bearer encode: {e}"))?;
        let datagram = UdpBearerDatagram {
            flags: if frame.queue == ForwardBearerQueue::Signaling {
                FLAG_SIGNALING_QUEUE
            } else {
                0
            },
            channel_family: frame.channel_family,
            direction: frame.traffic_frame.direction(),
            bts_id: self.transport.bts_id(),
            cell_id: self.transport.cell_id(),
            bearer_id: frame.bearer_id,
            sequence_no: self.next_bearer_sequence(),
            tx_frame_number: frame.tx_frame_number,
            payload,
        };
        self.transport.send(&datagram)
    }

    fn receive_frame(&self, _frame: BearerFrame) -> Result<Option<BearerFrame>, String> {
        Ok(None)
    }

    fn receive_datagram(
        &self,
        _datagram: UdpBearerDatagram,
    ) -> Result<Option<BearerFrame>, String> {
        Ok(None)
    }

    fn drain_received_frames(&self) -> Vec<BearerFrame> {
        self.transport
            .drain()
            .into_iter()
            .filter_map(|d| datagram_to_bearer_frame(d).ok())
            .collect()
    }

    fn stats(&self) -> BearerStats {
        let ts = self.transport.stats();
        BearerStats {
            tx_frames: ts.tx_datagrams,
            rx_accepted: ts.rx_accepted,
            duplicate_drop: ts.rx_duplicate_drop,
            late_drop: ts.rx_late_drop,
            encode_errors: 0,
            route_errors: ts.rx_route_errors + ts.rx_decode_errors,
            delivery_errors: ts.tx_errors,
        }
    }
}

fn datagram_to_bearer_frame(datagram: UdpBearerDatagram) -> Result<BearerFrame, String> {
    let traffic_frame = TrafficFrame::decode(
        datagram.channel_family,
        datagram.direction,
        &datagram.payload,
    )
    .map_err(|e| format!("bearer payload decode: {e}"))?;
    Ok(BearerFrame {
        channel_family: datagram.channel_family,
        bearer_id: datagram.bearer_id,
        tx_frame_number: datagram.tx_frame_number,
        traffic_frame,
        queue: if datagram.flags & FLAG_SIGNALING_QUEUE != 0 {
            ForwardBearerQueue::Signaling
        } else {
            ForwardBearerQueue::Traffic
        },
    })
}

/// Bearer client backed by direct access to a BTS `TrafficResourceService`.
/// Used by `spawn_in_process` to route bearer frames without UDP.
struct LocalBearerClient {
    controller: Arc<TrafficResourceService>,
    received_frames: StdMutex<VecDeque<BearerFrame>>,
}

impl BtsBearerClient for LocalBearerClient {
    fn send_frame(&self, frame: BearerFrame) -> Result<(), String> {
        let is_signaling = frame.queue == ForwardBearerQueue::Signaling;
        let walsh_code = frame.bearer_id as u8;
        cdma_bts::bts::bearer_agent::deliver_forward_frame(
            &self.controller,
            walsh_code,
            frame.traffic_frame,
            is_signaling,
        )
    }

    fn receive_frame(&self, frame: BearerFrame) -> Result<Option<BearerFrame>, String> {
        self.received_frames
            .lock()
            .map_err(|_| "local bearer receive queue poisoned".to_string())?
            .push_back(frame);
        Ok(None)
    }

    fn receive_datagram(
        &self,
        _datagram: UdpBearerDatagram,
    ) -> Result<Option<BearerFrame>, String> {
        Ok(None)
    }

    fn drain_received_frames(&self) -> Vec<BearerFrame> {
        match self.received_frames.lock() {
            Ok(mut frames) => frames.drain(..).collect(),
            Err(_) => Vec::new(),
        }
    }

    fn stats(&self) -> BearerStats {
        BearerStats::default()
    }
}

#[async_trait]
impl BtsControlClient for NetworkBtsControlClient {
    fn bearer_client(&self) -> Option<&dyn BtsBearerClient> {
        if let Some(ref local) = self.local_bearer {
            Some(local as &dyn BtsBearerClient)
        } else {
            self.bearer
                .as_ref()
                .map(|bearer| bearer as &dyn BtsBearerClient)
        }
    }

    async fn allocate_rc1_traffic(
        &self,
        _lc_generator: LongCodeGenerator,
        _initial_lc_chip: u64,
        esn: u32,
    ) -> Option<BtsTrafficChannelHandle> {
        let ccr = {
            let mut guard = self.inner.lock().await;
            self.allocate_ccr(&mut guard)
        };
        // RC1 calls never carry F-SCH (Phase 1 is RC3-only).
        let result = self.send_bts_setup(ccr, esn, false).await?;
        {
            let mut guard = self.inner.lock().await;
            guard.walsh_to_ccr.insert(result.walsh_code, ccr);
        }
        if self.local_controller.is_some() {
            self.send_ecam_commit(result.walsh_code, 1, 1);
            self.wait_for_commit(result.walsh_code).await;
        }
        Some(BtsTrafficChannelHandle {
            walsh_code: result.walsh_code,
            bearer_id: result.walsh_code as u32,
            for_rc: 1,
            rev_rc: 1,
            rc_label: "RC1",
            power_control_delay_pcgs: 8,
            sch_walsh_code: None,
        })
    }

    async fn allocate_rc3_traffic(
        &self,
        _lc_generator: LongCodeGenerator,
        _initial_lc_chip: u64,
        _fpc_subchan_gain: u8,
        esn: u32,
        include_sch: bool,
    ) -> Option<BtsTrafficChannelHandle> {
        let ccr = {
            let mut guard = self.inner.lock().await;
            self.allocate_ccr(&mut guard)
        };
        let result = self.send_bts_setup(ccr, esn, include_sch).await?;
        {
            let mut guard = self.inner.lock().await;
            guard.walsh_to_ccr.insert(result.walsh_code, ccr);
        }
        if self.local_controller.is_some() {
            self.send_ecam_commit(result.walsh_code, 3, 3);
            self.wait_for_commit(result.walsh_code).await;
        }
        Some(BtsTrafficChannelHandle {
            walsh_code: result.walsh_code,
            bearer_id: result.walsh_code as u32,
            for_rc: 3,
            rev_rc: 3,
            rc_label: "RC3",
            power_control_delay_pcgs: 8,
            sch_walsh_code: result.sch_walsh_code,
        })
    }

    async fn deallocate_traffic(&self, walsh_code: u8) {
        let ccr = {
            let mut guard = self.inner.lock().await;
            guard.walsh_to_ccr.remove(&walsh_code)
        };
        match ccr {
            Some(ccr) => {
                info!(
                    "abis_network: deallocate_traffic walsh={} -> CCR {:?}",
                    walsh_code, ccr
                );
                self.send_release_and_remove(ccr).await;
            }
            None => {
                warn!(
                    "abis_network: deallocate_traffic walsh={} has no CCR mapping",
                    walsh_code
                );
            }
        }
    }

    async fn commit_forward_sch_burst(
        &self,
        walsh_code: u8,
        request: ForwardBurstRadioInfo,
    ) -> Option<ForwardBurstRadioInfo> {
        let ccr = {
            let guard = self.inner.lock().await;
            *guard.walsh_to_ccr.get(&walsh_code)?
        };
        let (tx, rx) = oneshot::channel();
        {
            let mut guard = self.inner.lock().await;
            guard.pending_bursts.insert(ccr, PendingBurst { tx });
        }
        let msg = BurstRequestMessage {
            call_connection_reference: Some(ccr),
            band_class: None,
            downlink_radio_environment: None,
            cdma_serving_one_way_delay: None,
            privacy_info: None,
            correlation_id: Some(cdma_abis::control::CorrelationId(
                walsh_code as u32 | ((request.forward_supplemental_channel_rate as u32) << 8),
            )),
            sdu_id: None,
            mobile_identities: Vec::new(),
            cell_identifier_list: Some(vec![self.config.cell_id]),
            forward_burst_radio_info: Some(request),
            reverse_burst_radio_info: None,
            abis_destination_id: None,
        };
        let bytes = msg.encode().ok()?;
        let abis = decode(&bytes).ok()?;
        if self.sender.send(&abis).await.is_err() {
            let mut guard = self.inner.lock().await;
            guard.pending_bursts.remove(&ccr);
            return None;
        }
        match tokio::time::timeout(Duration::from_millis(500), rx).await {
            Ok(Ok(info)) => Some(info),
            _ => {
                let mut guard = self.inner.lock().await;
                guard.pending_bursts.remove(&ccr);
                None
            }
        }
    }

    async fn set_traffic_gain(&self, walsh_code: u8, gain_linear: f32) -> bool {
        if let Some(ref ctrl) = self.local_controller {
            ctrl.set_traffic_gain(walsh_code, gain_linear)
        } else {
            false
        }
    }

    async fn install_rx_request(&self, request: TrafficRxRequest) {
        if let Some(ref ctrl) = self.local_controller {
            let mut pool = ctrl.traffic_rx_pool().lock();
            pool.retain(|r| r.walsh_code != request.walsh_code);
            pool.push(request);
        }
    }

    async fn drop_pending_rx_request(&self, walsh_code: u8) {
        if let Some(ref ctrl) = self.local_controller {
            ctrl.drop_pending_rx_request(walsh_code);
        }
    }

    async fn request_rx_removal(&self, walsh_code: u8) {
        if let Some(ref ctrl) = self.local_controller {
            ctrl.request_rx_removal(walsh_code);
        }
    }

    fn send_pch_message(
        &self,
        message: cdma_abis::control::typed::PchMessageTransferMessage,
    ) -> Result<(), String> {
        let bytes = message.encode().map_err(|e| format!("PCH encode: {e}"))?;
        let msg =
            cdma_abis::control::decode(&bytes).map_err(|e| format!("PCH wire decode: {e}"))?;
        self.sender
            .try_send(&msg)
            .map_err(|e| format!("PCH send: {e:?}"))
    }

    fn drain_pch_transfer_acks(&self) -> Vec<PchTransferAckEvent> {
        match self.inner.try_lock() {
            Ok(mut guard) => guard.pch_ack_events.drain(..).collect(),
            Err(_) => Vec::new(),
        }
    }

    fn traffic_queue_len(&self, walsh_code: u8) -> Option<usize> {
        self.local_controller.as_ref().and_then(|ctrl| {
            ctrl.traffic_channels_pool()
                .lock()
                .iter()
                .find(|s| s.walsh_code == walsh_code)
                .map(|s| s.channel.queue_len())
        })
    }

    fn last_traffic_enqueue_at(&self, walsh_code: u8) -> Option<std::time::Instant> {
        self.local_controller.as_ref().and_then(|ctrl| {
            ctrl.traffic_channels_pool()
                .lock()
                .iter()
                .find(|s| s.walsh_code == walsh_code)
                .and_then(|s| s.channel.last_enqueue_at())
        })
    }
}
