use std::{net::Ipv4Addr, path::PathBuf, sync::Arc};

use cdma_bts::bts::{
    BtsCommand, BtsPowerControlRegistry, BtsRuntimeSettings, PagingChannelSettings,
    PchTransmitEvent, RxMetrics, TxMetrics,
};
use cdma_common::events::AccessChannelEvent;
use cdma_hlr::repository::HlrRepository;
use cdma_msc::VoicePolicy;
use cdma_smsc::repository::SmscRepository;
use tokio::sync::{broadcast, mpsc, watch};

use crate::{
    a1_edge::MscClient,
    abis_edge::{
        BtsControlClient,
        network::{NetworkBtsControlClient, NetworkClientConfig},
    },
    config::{BscNodeConfig, PagingRetryConfig, TrafficAssignmentConfig, TrafficRetryConfig},
    grpc::BscState,
    packet::PcfClient,
};

use super::{
    Bsc, Config, DataCallRequest, MobileInfo, OverheadParameters, PagingEvent, SmsRequest,
    TrafficEvent, TrafficPowerOverrideRequest,
};

pub async fn connect_configured_bts_client(
    bsc_config: &BscNodeConfig,
    bts_config: &cdma_bts::bts::BtsNodeConfig,
) -> Result<
    (
        Arc<dyn BtsControlClient>,
        mpsc::UnboundedReceiver<AccessChannelEvent>,
    ),
    String,
> {
    let bearer_config = cdma_abis::bearer_transport::BearerTransportConfig {
        bind_addr: bsc_config.bearer.bind_addr,
        remote_addr: bsc_config.bearer.remote_addr,
        bts_id: bts_config.overhead.base_id as u32,
        cell_id: 1,
    };
    let bearer = Arc::new(
        cdma_abis::bearer_transport::BearerTransport::new(&bearer_config)
            .map_err(|e| format!("failed to create BSC bearer transport: {e}"))?,
    );
    let net_config = NetworkClientConfig {
        cell_id: cdma_abis::control::typed::CellId {
            cell: bts_config.overhead.base_id,
            sector: 0x01,
        },
        mscid: bts_config.overhead.sid as u32,
        pilot_pn: bts_config.pilot_offset as u16,
        auth_mode: bts_config.overhead.auth_mode,
        p_rev_in_use: bts_config.overhead.p_rev,
        market_id: bts_config.overhead.sid,
        generating_entity_id: bts_config.overhead.base_id,
    };
    let (access_tx, access_rx) = mpsc::unbounded_channel();
    let client = NetworkBtsControlClient::connect_with_bearer_and_access(
        bsc_config.abis.remote_addr,
        net_config,
        bearer,
        access_tx,
    )
    .await
    .map_err(|e| format!("failed to connect to BTS Abis agent: {e}"))?;
    Ok((Arc::new(client), access_rx))
}

pub struct BscLaunchInputs {
    pub pilot_offset: usize,
    pub overhead: OverheadParameters,
    pub timezone: cdma_common::timezone::TimezoneConfig,
    pub paging: PagingChannelSettings,
    pub traffic_assignment: TrafficAssignmentConfig,
    pub traffic_retry: TrafficRetryConfig,
    pub paging_retry: PagingRetryConfig,
    pub mobile_idle_timeout_s: u64,
    pub rx_reference_dbm: Option<f64>,
    pub access_event_rx: mpsc::UnboundedReceiver<AccessChannelEvent>,
    pub tx_metrics: watch::Receiver<TxMetrics>,
    pub rx_metrics: watch::Receiver<RxMetrics>,
    pub bts_config: Arc<BtsRuntimeSettings>,
    pub bts_commands: mpsc::Sender<BtsCommand>,
    pub bts_power_control: BtsPowerControlRegistry,
    pub iq_capture_dir: PathBuf,
    pub hlr_repo: Arc<dyn HlrRepository>,
    /// SMSC repository — passed to BscState for gRPC history queries only.
    /// The BSC itself no longer does SMSC state updates; MSC owns SMS coordination.
    pub smsc_repo: Arc<dyn SmscRepository>,
    pub packet_endpoint: String,
    pub bts_client: Arc<dyn BtsControlClient>,
    pub msc_client: Arc<dyn MscClient>,
    pub voice_policy: Arc<dyn VoicePolicy>,
    pub pcf_client: Arc<dyn PcfClient>,
    pub pch_transmit_tx: broadcast::Sender<PchTransmitEvent>,
    /// Local IP that voice bearer UDP sockets bind to.
    /// Use 127.0.0.1 for single-host deployments; set to the host's
    /// network-facing IP when BSC and voice gateway are on separate hosts.
    pub voice_bearer_bind_ip: Ipv4Addr,
    /// Stable node identifier for this BSC. Must be unique across all BSC instances.
    pub node_id: String,
}

pub struct BscLaunchParts {
    pub bsc: Bsc,
    pub state: Arc<BscState>,
}

pub fn build_bsc_launch_parts(inputs: BscLaunchInputs) -> BscLaunchParts {
    let (access_broadcast_tx, _) = broadcast::channel(256);
    let (sms_request_tx, sms_request_rx) = mpsc::channel::<SmsRequest>(16);
    let (data_request_tx, data_request_rx) = mpsc::channel::<DataCallRequest>(16);
    let (power_override_request_tx, power_override_request_rx) =
        mpsc::channel::<TrafficPowerOverrideRequest>(16);
    let (mobiles_tx, mobiles_rx) = watch::channel(Vec::<MobileInfo>::new());
    let (paging_broadcast_tx, _) = broadcast::channel::<PagingEvent>(256);
    let (traffic_broadcast_tx, _) = broadcast::channel::<TrafficEvent>(256);

    let state = Arc::new(BscState {
        tx_metrics: inputs.tx_metrics,
        rx_metrics: inputs.rx_metrics,
        bts_config: inputs.bts_config,
        overhead: inputs.overhead.clone(),
        timezone: inputs.timezone.clone(),
        pilot_offset: inputs.pilot_offset,
        access_broadcast: access_broadcast_tx.clone(),
        mobiles: mobiles_rx,
        bts_commands: inputs.bts_commands,
        bts_power_control: inputs.bts_power_control,
        iq_capture_dir: inputs.iq_capture_dir,
        sms_request_tx: sms_request_tx.clone(),
        data_request_tx: data_request_tx.clone(),
        power_override_request_tx: power_override_request_tx.clone(),
        paging_broadcast: paging_broadcast_tx.clone(),
        pch_transmit_broadcast: Some(inputs.pch_transmit_tx.clone()),
        traffic_broadcast: traffic_broadcast_tx.clone(),
        hlr_repo: inputs.hlr_repo.clone(),
        smsc_repo: inputs.smsc_repo.clone(),
        packet_endpoint: inputs.packet_endpoint.clone(),
        bts_client: inputs.bts_client.clone(),
        node_id: inputs.node_id.clone(),
    });

    let bsc = Bsc::new(Config {
        pilot_offset: inputs.pilot_offset,
        overhead: inputs.overhead,
        paging: inputs.paging,
        traffic_assignment: inputs.traffic_assignment,
        access_event_rx: Some(inputs.access_event_rx),
        access_event_broadcast: Some(access_broadcast_tx),
        sms_request_rx: Some(sms_request_rx),
        sms_request_tx: Some(sms_request_tx),
        data_request_rx: Some(data_request_rx),
        data_request_tx: Some(data_request_tx),
        power_override_request_rx: Some(power_override_request_rx),
        power_override_request_tx: Some(power_override_request_tx),
        mobiles_tx: Some(mobiles_tx),
        paging_broadcast: Some(paging_broadcast_tx),
        traffic_broadcast: Some(traffic_broadcast_tx),
        rx_reference_dbm: inputs.rx_reference_dbm,
        hlr_repo: Some(inputs.hlr_repo),
        msc_client: inputs.msc_client,
        bts_client: Some(inputs.bts_client),
        traffic_retry: inputs.traffic_retry,
        paging_retry: inputs.paging_retry,
        voice_policy: inputs.voice_policy,
        pcf_client: Some(inputs.pcf_client),
        mobile_idle_timeout_s: inputs.mobile_idle_timeout_s,
        msc_voice_bearer: Some(Arc::new(cdma_ios::VoiceBearerManager::new(
            inputs.voice_bearer_bind_ip,
        ))),
        bts_paging_state: None,
        node_id: inputs.node_id.clone(),
    });

    BscLaunchParts { bsc, state }
}
