//! BSC-side Abis edge: async client traits the BSC uses to control BTS
//! resources through the Abis transport (A.S0003-A §4.5.6.4).
//!
//! [`NetworkBtsControlClient`] encodes Abis control messages and drives
//! setup/release procedures. It supports both TCP (real deployments) and
//! in-memory channel transports (tests / monolithic mode) via
//! [`NetworkBtsControlClient::spawn_in_process`].

pub mod network;

use async_trait::async_trait;
use std::time::Instant;

use cdma_abis::bearer::{ChannelFamily, TrafficFrame};
use cdma_abis::control::typed::PchMessageTransferMessage;
use cdma_abis::udp_bearer::UdpBearerDatagram;
use cdma_bts::bts::SchWalshChannelRc3;
use cdma_common::phy::long_code::LongCodeGenerator;
use cdma_common::traffic::TrafficRxRequest;

pub use network::NetworkBtsControlClient;

/// Typed BSC-side view of an Abis `PchMessageTransferAck`.
///
/// The BTS sends an initial accept/reject ack for each PCH transfer, and may
/// later send another ack with `bts_l2_termination=true` when the MS confirms
/// an assured F-PCH message over the air.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PchTransferAckEvent {
    pub correlation_id: Option<u32>,
    pub cause: Option<u8>,
    pub bts_l2_termination: Option<bool>,
}

/// Forward-bearer queue selected at the BTS after UDP bearer routing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ForwardBearerQueue {
    /// Dedicated signaling queue; transmitted before ordinary traffic frames.
    Signaling,
    /// Ordinary traffic queue.
    Traffic,
}

/// BSC-facing bearer frame before the transport-only UDP wrapper is applied.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BearerFrame {
    pub channel_family: ChannelFamily,
    pub bearer_id: u32,
    pub tx_frame_number: u32,
    pub traffic_frame: TrafficFrame,
    pub queue: ForwardBearerQueue,
}

/// Per-BTS bearer transport counters surfaced to management/diagnostics.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BearerStats {
    pub tx_frames: u64,
    pub rx_accepted: u64,
    pub duplicate_drop: u64,
    pub late_drop: u64,
    pub encode_errors: u64,
    pub route_errors: u64,
    pub delivery_errors: u64,
}

/// Opaque BTS traffic-channel allocation returned to the BSC.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BtsTrafficChannelHandle {
    pub walsh_code: u8,
    pub bearer_id: u32,
    pub for_rc: u8,
    pub rev_rc: u8,
    pub rc_label: &'static str,
    pub power_control_delay_pcgs: u64,
}

/// BSC-facing traffic bearer for a single BTS peer.
///
/// The payload is an encoded Abis bearer traffic message. The UDP header is
/// transport-only; the in-process adapter still runs frames through the same
/// encode/decode/router path used by the network bearer implementation.
pub trait BtsBearerClient: Send + Sync {
    fn send_frame(&self, frame: BearerFrame) -> Result<(), String>;
    fn receive_frame(&self, frame: BearerFrame) -> Result<Option<BearerFrame>, String>;
    fn receive_datagram(&self, datagram: UdpBearerDatagram) -> Result<Option<BearerFrame>, String>;
    fn drain_received_frames(&self) -> Vec<BearerFrame>;
    fn stats(&self) -> BearerStats;
}

/// BSC-facing async control client for a single BTS peer.
///
/// All BSC code that needs to allocate, configure, or tear down BTS
/// traffic-channel resources goes through this trait.
/// [`NetworkBtsControlClient`] translates each call into Abis control
/// messages, over either TCP or an in-memory channel transport.
#[async_trait]
pub trait BtsControlClient: Send + Sync {
    /// Optional bearer client associated with this BTS peer.
    fn bearer_client(&self) -> Option<&dyn BtsBearerClient> {
        None
    }

    /// Allocate an RC1 forward traffic channel.
    async fn allocate_rc1_traffic(
        &self,
        lc_generator: LongCodeGenerator,
        initial_lc_chip: u64,
        esn: u32,
    ) -> Option<BtsTrafficChannelHandle>;

    /// Allocate an RC3 forward traffic channel.
    async fn allocate_rc3_traffic(
        &self,
        lc_generator: LongCodeGenerator,
        initial_lc_chip: u64,
        fpc_subchan_gain: u8,
        esn: u32,
    ) -> Option<BtsTrafficChannelHandle>;

    /// Allocate an RC3 forward Supplemental Channel (F-SCH).
    async fn allocate_rc3_sch(
        &self,
        lc_generator: LongCodeGenerator,
        sch_gain_linear: f32,
    ) -> Option<(u8, SchWalshChannelRc3)>;

    /// Deallocate an RC1 / RC3 forward traffic channel by Walsh code.
    async fn deallocate_traffic(&self, walsh_code: u8);

    /// Deallocate an F-SCH by W(32) code.
    async fn deallocate_sch(&self, w32_code: u8);

    /// Update an allocated traffic channel's composite gain.
    async fn set_traffic_gain(&self, walsh_code: u8, gain_linear: f32) -> bool;

    /// Install a reverse-traffic receiver request.
    async fn install_rx_request(&self, request: TrafficRxRequest);

    /// Drop any pending reverse-traffic RX request matching `walsh_code`.
    async fn drop_pending_rx_request(&self, walsh_code: u8);

    /// Queue a Walsh code for reverse-traffic RX teardown.
    async fn request_rx_removal(&self, walsh_code: u8);

    /// Send a paging channel message transfer to the BTS via Abis.
    fn send_pch_message(&self, message: PchMessageTransferMessage) -> Result<(), String>;

    /// Drain received PCH transfer acknowledgments from the BTS.
    fn drain_pch_transfer_acks(&self) -> Vec<PchTransferAckEvent> {
        Vec::new()
    }

    /// Forward traffic queue depth for bearer pacing.
    fn traffic_queue_len(&self, walsh_code: u8) -> Option<usize>;

    /// Last time the BTS-owned forward traffic queue accepted a frame.
    fn last_traffic_enqueue_at(&self, walsh_code: u8) -> Option<Instant>;

    /// Drain access-channel signal quality measurements collected by the BTS.
    fn drain_rx_measurements(
        &self,
    ) -> Vec<(
        cdma_common::metrics::RxMeasurementKey,
        cdma_common::metrics::RxMeasurement,
    )> {
        Vec::new()
    }
}
