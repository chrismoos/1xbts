use std::{path::PathBuf, sync::Arc};

use cdma_bts::bts::{
    BtsCommand, BtsPowerControlRegistry, BtsRuntimeSettings, PchTransmitEvent,
    RxMetrics as BtsRxMetrics, TxMetrics as BtsTxMetrics,
};
use cdma_common::events::AccessChannelEvent;
use cdma_hlr::repository::HlrRepository;
use cdma_smsc::repository::SmscRepository;
use tokio::sync::{broadcast, mpsc, watch};

use crate::abis_edge::BtsControlClient;
use crate::bsc::{
    DataCallRequest, MobileInfo, OverheadParameters, PagingEvent, SmsRequest, TrafficEvent,
    TrafficPowerOverrideRequest,
};

/// Shared state that the gRPC server reads from.
///
/// Created in main.rs, shared between BSC and gRPC handlers via Arc.
pub struct BscState {
    pub tx_metrics: watch::Receiver<BtsTxMetrics>,
    pub rx_metrics: watch::Receiver<BtsRxMetrics>,
    pub bts_config: Arc<BtsRuntimeSettings>,
    pub channel: cdma_common::band_class::ChannelPlan,
    pub tx_center_frequency_hz: usize,
    pub rx_center_frequency_hz: usize,
    pub overhead: OverheadParameters,
    pub timezone: cdma_common::timezone::TimezoneConfig,
    pub pilot_offset: usize,
    pub access_broadcast: broadcast::Sender<AccessChannelEvent>,
    pub mobiles: watch::Receiver<Vec<MobileInfo>>,
    pub bts_commands: mpsc::Sender<BtsCommand>,
    pub bts_power_control: BtsPowerControlRegistry,
    pub iq_capture_dir: PathBuf,
    pub sms_request_tx: mpsc::Sender<SmsRequest>,
    pub data_request_tx: mpsc::Sender<DataCallRequest>,
    pub power_override_request_tx: mpsc::Sender<TrafficPowerOverrideRequest>,
    pub paging_broadcast: broadcast::Sender<PagingEvent>,
    pub pch_transmit_broadcast: Option<broadcast::Sender<PchTransmitEvent>>,
    pub traffic_broadcast: broadcast::Sender<TrafficEvent>,
    pub hlr_repo: Arc<dyn HlrRepository>,
    pub smsc_repo: Arc<dyn SmscRepository>,
    pub packet_endpoint: String,
    pub bts_client: Arc<dyn BtsControlClient>,
    /// Stable node identifier for this BSC instance, used in HLR registrations
    /// and management events. Must be unique across all BSC instances.
    pub node_id: String,
}
