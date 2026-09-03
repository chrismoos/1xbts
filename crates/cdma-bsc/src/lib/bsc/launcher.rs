use std::{net::Ipv4Addr, sync::Arc};

use cdma_common::events::AccessChannelEvent;
use cdma_hlr::repository::HlrRepository;
use cdma_smsc::repository::SmscRepository;
use tokio::sync::{broadcast, mpsc, watch};

use crate::{
    a1_edge::MscClient,
    config::{TrafficAssignmentConfig, TrafficRetryConfig},
    grpc::BscState,
    packet::PcfClient,
};

use super::{
    Bsc, BtsRegistry, Config, DataCallRequest, MobileInfo, PagingEvent, SmsRequest, TrafficEvent,
    TrafficPowerOverrideRequest,
};

pub struct BscLaunchInputs {
    /// The BTSs this BSC serves, populated by the attach tasks.
    pub bts: Arc<BtsRegistry>,
    pub traffic_assignment: TrafficAssignmentConfig,
    pub traffic_retry: TrafficRetryConfig,
    pub mobile_idle_timeout_s: u64,
    /// Access events from every attached cell. The matching sender is handed
    /// to `spawn_bts_attach`.
    pub access_event_rx: mpsc::UnboundedReceiver<AccessChannelEvent>,
    /// Cells whose Abis link dropped. The matching sender is handed to
    /// `spawn_bts_attach`.
    pub cell_detach_rx: mpsc::UnboundedReceiver<cdma_common::events::AccessCellId>,
    pub hlr_repo: Arc<dyn HlrRepository>,
    /// SMSC repository — read by the management layer for history queries.
    /// The BSC itself does no SMSC state updates. The MSC owns SMS coordination.
    pub smsc_repo: Arc<dyn SmscRepository>,
    pub packet_endpoint: String,
    /// Address the A1 listener accepts the MSC on, reported at enrollment.
    pub a1_bind_addr: std::net::SocketAddr,
    pub msc_client: Arc<dyn MscClient>,
    pub voice_timeouts: crate::config::VoiceTimeoutConfig,
    pub pcf_client: Arc<dyn PcfClient>,
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
        bts: inputs.bts.clone(),
        access_broadcast: access_broadcast_tx.clone(),
        mobiles: mobiles_rx,
        sms_request_tx: sms_request_tx.clone(),
        data_request_tx: data_request_tx.clone(),
        power_override_request_tx: power_override_request_tx.clone(),
        paging_broadcast: paging_broadcast_tx.clone(),
        traffic_broadcast: traffic_broadcast_tx.clone(),
        hlr_repo: inputs.hlr_repo.clone(),
        smsc_repo: inputs.smsc_repo.clone(),
        packet_endpoint: inputs.packet_endpoint.clone(),
        node_id: inputs.node_id.clone(),
        a1_bind_addr: inputs.a1_bind_addr,
    });

    let bsc = Bsc::new(Config {
        bts: inputs.bts,
        traffic_assignment: inputs.traffic_assignment,
        access_event_rx: Some(inputs.access_event_rx),
        cell_detach_rx: Some(inputs.cell_detach_rx),
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
        hlr_repo: Some(inputs.hlr_repo),
        msc_client: inputs.msc_client,
        traffic_retry: inputs.traffic_retry,
        voice_timeouts: inputs.voice_timeouts,
        pcf_client: Some(inputs.pcf_client),
        mobile_idle_timeout_s: inputs.mobile_idle_timeout_s,
        msc_voice_bearer: Some(Arc::new(cdma_ios::VoiceBearerManager::new(
            inputs.voice_bearer_bind_ip,
        ))),
        bts_paging_state: None,
        node_id: inputs.node_id.clone(),
    });

    // 1x/HRPD cross-paging is not implemented: no element serves A21. A
    // dormant HRPD terminal is paged on 1x only.

    BscLaunchParts { bsc, state }
}
