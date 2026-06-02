use std::sync::{
    Arc,
    atomic::{AtomicU32, AtomicU64, Ordering},
};

use cdma_bts::bts::{AccessChannelEvent, PagingChannelSettings};
use cdma_common::lac::paging_messages::MsAddress;
use cdma_common::overhead::OverheadParameters;
use cdma_hlr::repository::HlrRepository;
use tokio::sync::{broadcast, mpsc, watch};

use crate::config::{PagingRetryConfig, TrafficAssignmentConfig, TrafficRetryConfig};

use super::{
    A1Service, AccessService, DataCallRequest, EventService, HlrResolution, MobileInfo,
    MobileRegistryService, PacketService, PagingEvent, PagingService, PendingAssignmentFailure,
    SmsRequest, SmsService, TrafficAssignmentService, TrafficBearerService, TrafficEvent,
    TrafficLifecycleService, TrafficPowerOverrideRequest, TrafficSignalingService, VoiceService,
};

pub(crate) async fn recv_or_pending<T>(
    rx: Option<&mut tokio::sync::mpsc::Receiver<T>>,
) -> Option<T> {
    match rx {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

pub(crate) async fn recv_unbounded_or_pending<T>(
    rx: Option<&mut tokio::sync::mpsc::UnboundedReceiver<T>>,
) -> Option<T> {
    match rx {
        Some(rx) => rx.recv().await,
        None => std::future::pending().await,
    }
}

pub(crate) const DEFAULT_PAGE_TIMEOUT_MS: u64 = 30_000;
/// Wake ahead of the assigned paging slot and enqueue the future-timed GPM
/// early. This avoids skipping the intended slot when the retry task fires a
/// few milliseconds late and `Utc::now()` has already crossed the slot start.
pub(crate) const PAGE_RETRY_GUARD_MS: u64 = 250;
static BSC_EVENT_SEQ: AtomicU64 = AtomicU64::new(1);
static PCH_CORRELATION_SEQ: AtomicU32 = AtomicU32::new(1);

pub(crate) fn next_pch_correlation_id() -> u32 {
    PCH_CORRELATION_SEQ.fetch_add(1, Ordering::Relaxed)
}

pub(crate) fn next_bsc_event_id(prefix: &str) -> String {
    format!(
        "{}-{:016x}",
        prefix,
        BSC_EVENT_SEQ.fetch_add(1, Ordering::Relaxed)
    )
}

pub struct Config {
    pub pilot_offset: usize,
    pub overhead: OverheadParameters,
    pub paging: PagingChannelSettings,
    pub traffic_assignment: TrafficAssignmentConfig,
    pub access_event_rx: Option<mpsc::UnboundedReceiver<AccessChannelEvent>>,
    /// When set, access events are re-broadcast for gRPC streaming subscribers.
    pub access_event_broadcast: Option<broadcast::Sender<AccessChannelEvent>>,
    /// Optional channel for external SMS requests.
    pub sms_request_rx: Option<mpsc::Receiver<SmsRequest>>,
    /// Sender side of the SMS request channel -- used by async SMSC delivery tasks.
    pub sms_request_tx: Option<mpsc::Sender<SmsRequest>>,
    /// Optional channel for external BS-originated data call requests.
    pub data_request_rx: Option<mpsc::Receiver<DataCallRequest>>,
    /// Sender side of the data call request channel.
    pub data_request_tx: Option<mpsc::Sender<DataCallRequest>>,
    /// Optional channel for traffic-channel power override requests.
    pub power_override_request_rx: Option<mpsc::Receiver<TrafficPowerOverrideRequest>>,
    /// Sender side of the traffic-channel power override request channel.
    pub power_override_request_tx: Option<mpsc::Sender<TrafficPowerOverrideRequest>>,
    /// Watch channel for publishing mobile list snapshots to gRPC.
    pub mobiles_tx: Option<watch::Sender<Vec<MobileInfo>>>,
    /// Broadcast channel for forward-link paging events.
    pub paging_broadcast: Option<broadcast::Sender<PagingEvent>>,
    /// Broadcast channel for forward-link traffic signaling events.
    pub traffic_broadcast: Option<broadcast::Sender<TrafficEvent>>,
    /// Reference dBm offset for converting raw_power_db to absolute dBm.
    pub rx_reference_dbm: Option<f64>,
    /// HLR repository for subscriber resolution and registration binding.
    pub hlr_repo: Option<Arc<dyn HlrRepository>>,
    /// Required MSC/A1 seam client. Voice call routing and media policy live at the MSC.
    pub msc_client: Arc<dyn crate::a1_edge::MscClient>,
    /// BTS control client. The BSC requests Walsh allocation, traffic
    /// channel teardown, reverse-traffic RX setup, and gain updates
    /// through this trait via Abis control messages.
    /// `None` is supported for unit tests that don't exercise BTS
    /// resource paths.
    pub bts_client: Option<Arc<dyn crate::abis_edge::BtsControlClient>>,
    /// Configuration for retransmitting unacknowledged forward traffic channel messages.
    pub traffic_retry: TrafficRetryConfig,
    /// Configuration for retransmitting unacknowledged forward paging channel messages.
    pub paging_retry: PagingRetryConfig,
    /// MSC-owned voice policy consumed through an explicit dependency.
    pub voice_policy: Arc<dyn cdma_msc::VoicePolicy>,
    /// Packet data service client (BSC->PCF boundary).
    pub pcf_client: Option<Arc<dyn crate::packet::PcfClient>>,
    /// Evict idle registered mobiles after this many seconds of no access
    /// activity (and no active traffic channel). 0 = disabled. Default: 3600.
    pub mobile_idle_timeout_s: u64,
    /// BTS paging supplier shared state. When set, the BSC pushes page
    /// records here instead of using its own internal slot scheduler.
    pub bts_paging_state:
        Option<Arc<parking_lot::Mutex<cdma_bts::bts::paging_supplier::PagingSupplierState>>>,
    /// MSC voice bearer manager for per-circuit A2p RTP voice sessions.
    /// When set, reverse traffic frames are relayed to the MSC via this
    /// bearer, and forward frames from the MSC are relayed to the BTS.
    pub msc_voice_bearer: Option<std::sync::Arc<cdma_ios::VoiceBearerManager>>,
    /// Stable node identifier written to the HLR on registration and used
    /// in management events. Must be unique across all BSC instances.
    pub node_id: String,
}

pub struct Bsc {
    pub(crate) config: Config,
    pub(crate) events: EventService,
    pub(crate) access_tx: super::AccessTx,
    pub(crate) access_service: AccessService,
    pub(crate) mobiles: MobileRegistryService,
    pub(crate) paging: PagingService,
    pub(crate) a1: A1Service,
    pub(crate) sms: SmsService,
    #[allow(dead_code)]
    pub(crate) packet: PacketService,
    #[allow(dead_code)]
    pub(crate) traffic_assignment: TrafficAssignmentService,
    #[allow(dead_code)]
    pub(crate) traffic_lifecycle: TrafficLifecycleService,
    /// Channel for receiving async HLR resolution results.
    pub(crate) hlr_result_tx: mpsc::Sender<HlrResolution>,
    pub(crate) hlr_result_rx: mpsc::Receiver<HlrResolution>,
    pub(crate) voice: VoiceService,
    pub(crate) traffic_signaling: TrafficSignalingService,
    pub(crate) traffic_bearer: TrafficBearerService,
    pub(crate) pending_a1_failure_after_release: Vec<(MsAddress, PendingAssignmentFailure)>,
    /// SMS submissions parked on an SO6 traffic channel after the BTS
    /// rejected the original F-PCH attempt. Keyed by walsh_code; consumed
    /// on Service Connect Completion to re-deliver the SMS over F-DSCH.
    pub(crate) pending_sms_escalations: std::collections::HashMap<u8, super::sms::PendingSmsAck>,
    /// Per-MS FIFO of SMS waiting for the in-flight delivery to the same
    /// destination to ack. Keyed by `format_ms_address` (MsAddress isn't Hash).
    pub(crate) pending_sms_queue:
        std::collections::HashMap<String, std::collections::VecDeque<SmsRequest>>,
    /// Per-walsh tracker for the most recent **OTASP** ADDS Deliver
    /// (`burst_type = 0x04`) the BSC sent on the F-TCH that's still
    /// awaiting an L2 ack or L3 reject from the MS. Records the A1
    /// `Tag` the MSC put on the deliver so the BSC can correlate its
    /// outbound `AddsDeliverAck` back to that same deliver. OTASP is
    /// request-response with at most one outbound DBM in flight per
    /// walsh, so a single-slot tracker is sufficient.
    pub(crate) pending_otasp_dbm:
        std::collections::HashMap<u8, super::traffic_signaling::PendingOtaspDbm>,
}

impl Bsc {
    pub(crate) fn voice_policy(&self) -> cdma_msc::VoicePolicySnapshot {
        self.config.voice_policy.snapshot()
    }

    pub fn new(config: Config) -> Bsc {
        let (hlr_result_tx, hlr_result_rx) = mpsc::channel(32);
        let events = EventService::new(
            config.mobiles_tx.clone(),
            config.paging_broadcast.clone(),
            config.traffic_broadcast.clone(),
            config.access_event_broadcast.clone(),
        );
        let access_tx = super::AccessTx::new(config.bts_client.clone());
        let a1 = A1Service::new(config.msc_client.clone());
        let sms = SmsService::new();
        Bsc {
            config,
            events: events.clone(),
            access_tx,
            access_service: AccessService::new(),
            mobiles: MobileRegistryService::new(events.clone()),
            paging: PagingService::new(),
            a1,
            sms,
            packet: PacketService,
            traffic_assignment: TrafficAssignmentService,
            traffic_lifecycle: TrafficLifecycleService,
            hlr_result_tx,
            hlr_result_rx,
            voice: VoiceService::default(),
            traffic_signaling: TrafficSignalingService::default(),
            traffic_bearer: TrafficBearerService::default(),
            pending_a1_failure_after_release: Vec::new(),
            pending_sms_escalations: std::collections::HashMap::new(),
            pending_sms_queue: std::collections::HashMap::new(),
            pending_otasp_dbm: std::collections::HashMap::new(),
        }
    }
}
