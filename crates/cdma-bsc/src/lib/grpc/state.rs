use std::sync::Arc;

use cdma_common::events::AccessChannelEvent;
use cdma_hlr::repository::HlrRepository;
use cdma_smsc::repository::SmscRepository;
use tokio::sync::{broadcast, mpsc, watch};

use crate::bsc::{
    BtsRegistry, DataCallRequest, MobileInfo, PagingEvent, SmsRequest, TrafficEvent,
    TrafficPowerOverrideRequest,
};

/// Shared state that the gRPC server reads from.
///
/// Radio state is not held here: each BTS serves `BtsManagementService` for
/// the cell it operates, and the management service reaches it through the
/// registry's per-cell OAM client.
pub struct BscState {
    /// Every BTS this BSC serves, keyed by cell.
    pub bts: Arc<BtsRegistry>,
    pub access_broadcast: broadcast::Sender<AccessChannelEvent>,
    pub mobiles: watch::Receiver<Vec<MobileInfo>>,
    pub sms_request_tx: mpsc::Sender<SmsRequest>,
    pub data_request_tx: mpsc::Sender<DataCallRequest>,
    pub power_override_request_tx: mpsc::Sender<TrafficPowerOverrideRequest>,
    pub paging_broadcast: broadcast::Sender<PagingEvent>,
    pub traffic_broadcast: broadcast::Sender<TrafficEvent>,
    pub hlr_repo: Arc<dyn HlrRepository>,
    pub smsc_repo: Arc<dyn SmscRepository>,
    pub packet_endpoint: String,
    /// Stable node identifier for this BSC instance, used in HLR registrations
    /// and management events. Must be unique across all BSC instances.
    pub node_id: String,
    /// Address the A1 listener accepts the MSC on, reported at enrollment.
    pub a1_bind_addr: std::net::SocketAddr,
}
