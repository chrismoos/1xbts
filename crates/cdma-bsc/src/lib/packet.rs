//! BSC-facing PCF client boundary for packet data.

/// Re-export generated packet protobufs at the path tonic-build uses for
/// cross-package references from management protos.
pub mod v1 {
    pub use crate::grpc::packet_proto::*;
}

use tokio::sync::mpsc;

/// Packet bearer frame crossing the BSC <-> PCF boundary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PacketBearerFrame {
    pub session_id: String,
    pub bits: Vec<u8>,
    pub num_bits: u32,
    pub rate_bps: u32,
}

/// Radio-edge metadata supplied when BSC asks PCF to establish packet data.
///
/// `subscriber_id` is `None` for unprovisioned/roaming mobiles — those
/// sessions still open, but the envelope's `Subscriber` field stays empty
/// and the bus relies on forward-enrichment from `imsi`/`esn`.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct PacketSessionMetadata {
    pub mobile_address: String,
    pub subscriber_id: Option<uuid::Uuid>,
    pub phone_number: String,
    /// IMSI of the handset, if available.
    pub imsi: Option<String>,
    /// ESN of the handset, if available.
    pub esn: Option<u32>,
    pub traffic_walsh_code: u32,
}

/// BSC-facing packet control boundary.
#[async_trait::async_trait]
pub trait PcfClient: Send + Sync {
    async fn open_packet_session(
        &self,
        session_id: String,
        service_option: u32,
        metadata: PacketSessionMetadata,
    ) -> Result<
        (
            mpsc::Sender<PacketBearerFrame>,
            mpsc::Receiver<PacketBearerFrame>,
        ),
        String,
    >;

    async fn close_packet_session(&self, session_id: &str);

    /// An error leaves the session on the fundamental channel.
    async fn set_sch_active(
        &self,
        session_id: &str,
        active: bool,
        rate_bps: u32,
    ) -> Result<(), String>;
}
