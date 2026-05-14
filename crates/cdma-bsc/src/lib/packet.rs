//! BSC-facing PCF client boundary for packet data.
//!
//! BSC packet-data control must go through this boundary instead of directly
//! owning packet-core services. The legacy adapter keeps the old in-process
//! packet engine available while PCF/PDSN standards procedures are wired in.

/// Re-export generated packet protobufs at the path tonic-build uses for
/// cross-package references from management protos.
pub mod v1 {
    pub use crate::grpc::packet_proto::*;
}

use std::sync::{
    Arc, Mutex,
    atomic::{AtomicU32, Ordering},
};

use log::{info, warn};
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

    /// Toggle F-SCH downlink frame generation on a running session.
    /// Called by the BSC after a successful F-SCH allocation (active=true)
    /// and during teardown / SCH release (active=false). Errors are
    /// non-fatal: the BSC logs and continues with FCH-only.
    async fn set_sch_active(
        &self,
        session_id: &str,
        active: bool,
        rate_bps: u32,
    ) -> Result<(), String>;
}

/// Legacy in-process PCF adapter backed by the old packet service.
pub struct LegacyPcfClient {
    inner: Arc<cdma_packet::grpc::PacketServiceImpl>,
}

impl LegacyPcfClient {
    pub fn new(inner: Arc<cdma_packet::grpc::PacketServiceImpl>) -> Self {
        Self { inner }
    }
}

#[async_trait::async_trait]
impl PcfClient for LegacyPcfClient {
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
    > {
        let metadata = cdma_packet::session_task::SessionMetadata {
            mobile_address: metadata.mobile_address,
            subscriber_id: metadata.subscriber_id.map(|u| u.to_string()),
            phone_number: metadata.phone_number,
            imsi: metadata.imsi,
            esn: metadata.esn,
            traffic_walsh_code: metadata.traffic_walsh_code,
        };
        let (legacy_uplink_tx, mut legacy_downlink_rx) =
            self.inner
                .open_session_direct(session_id, service_option, metadata)?;
        let (uplink_tx, mut uplink_rx) = mpsc::channel::<PacketBearerFrame>(256);
        let (downlink_tx, downlink_rx) = mpsc::channel::<PacketBearerFrame>(256);

        tokio::spawn(async move {
            while let Some(frame) = uplink_rx.recv().await {
                let legacy = cdma_packet::proto::SessionFrame {
                    session_id: frame.session_id,
                    bits: frame.bits,
                    num_bits: frame.num_bits,
                    rate_bps: frame.rate_bps,
                };
                if legacy_uplink_tx.send(legacy).await.is_err() {
                    break;
                }
            }
        });

        tokio::spawn(async move {
            while let Some(frame) = legacy_downlink_rx.recv().await {
                let mapped = PacketBearerFrame {
                    session_id: frame.session_id,
                    bits: frame.bits,
                    num_bits: frame.num_bits,
                    rate_bps: frame.rate_bps,
                };
                if downlink_tx.send(mapped).await.is_err() {
                    break;
                }
            }
        });

        Ok((uplink_tx, downlink_rx))
    }

    async fn close_packet_session(&self, session_id: &str) {
        self.inner.close_session_direct(session_id).await;
    }

    async fn set_sch_active(
        &self,
        session_id: &str,
        active: bool,
        rate_bps: u32,
    ) -> Result<(), String> {
        self.inner
            .set_session_sch_active(session_id, active, rate_bps)
            .await
    }
}

/// In-process PCF/PDSN adapter for the monolith runtime.
///
/// This is the Track C migration bridge: BSC talks to a PCF client; the adapter
/// creates PCF/PDSN-owned session and bearer state before delegating the
/// still-legacy PPP/TUN packet-core task to `LegacyPcfClient`.
pub struct InProcessPcfClient {
    pcf: Mutex<cdma_pcf::PcfSessionManager>,
    pdsn: Mutex<cdma_pdsn::PdsnSessionManager>,
    legacy: LegacyPcfClient,
    next_packet_session: AtomicU32,
}

impl InProcessPcfClient {
    pub fn new(legacy_packet_service: Arc<cdma_packet::grpc::PacketServiceImpl>) -> Self {
        Self {
            pcf: Mutex::new(cdma_pcf::PcfSessionManager::new()),
            pdsn: Mutex::new(cdma_pdsn::PdsnSessionManager::new()),
            legacy: LegacyPcfClient::new(legacy_packet_service),
            next_packet_session: AtomicU32::new(1),
        }
    }
}

#[async_trait::async_trait]
impl PcfClient for InProcessPcfClient {
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
    > {
        let ordinal = self.next_packet_session.fetch_add(1, Ordering::Relaxed);
        let pcf_session_id = {
            let mut pcf = self
                .pcf
                .lock()
                .map_err(|_| "pcf session manager lock poisoned".to_string())?;
            let event = pcf
                .create_from_a9(Some(metadata.mobile_address.as_bytes().to_vec()))
                .map_err(|e| format!("pcf create_from_a9 failed: {e}"))?;
            let cdma_pcf::PcfEvent::SessionCreated { id } = event else {
                return Err("pcf create_from_a9 returned unexpected event".to_string());
            };

            let a8_endpoint = cdma_a8::BearerEndpoint::new([127, 0, 0, 1], [127, 0, 0, 1]);
            let a8_bearer = cdma_a8::BearerSession::with_directional_keys(
                id.0 as u32,
                ordinal,
                ordinal,
                a8_endpoint,
                cdma_a8::BearerProfile::standard_packet_data(),
            );
            pcf.bind_a8_bearer(id, a8_bearer)
                .map_err(|e| format!("pcf bind_a8_bearer failed: {e}"))?;
            id
        };

        let a11_key = cdma_a11::SessionKey {
            pcf_session_id: pcf_session_id.0 as u32,
            mn_session_reference_id: (ordinal & 0xffff) as u16,
        };

        {
            let mut pdsn = self
                .pdsn
                .lock()
                .map_err(|_| "pdsn session manager lock poisoned".to_string())?;
            pdsn.install_registered_session(a11_key)
                .map_err(|e| format!("pdsn install_registered_session failed: {e}"))?;
            let a10_endpoint = cdma_a10::BearerEndpoint::new([127, 0, 0, 1], [127, 0, 0, 1]);
            let a10_bearer = cdma_a10::BearerSession::with_directional_keys(
                pcf_session_id.0 as u32,
                ordinal,
                ordinal,
                a10_endpoint,
                cdma_a10::BearerProfile::standard_packet_data(),
            );
            pdsn.bind_a10_bearer(a11_key, a10_bearer)
                .map_err(|e| format!("pdsn bind_a10_bearer failed: {e}"))?;
        }

        {
            let mut pcf = self
                .pcf
                .lock()
                .map_err(|_| "pcf session manager lock poisoned".to_string())?;
            let a10_endpoint = cdma_a10::BearerEndpoint::new([127, 0, 0, 1], [127, 0, 0, 1]);
            let a10_bearer = cdma_a10::BearerSession::with_directional_keys(
                pcf_session_id.0 as u32,
                ordinal,
                ordinal,
                a10_endpoint,
                cdma_a10::BearerProfile::standard_packet_data(),
            );
            pcf.bind_a10_bearer(pcf_session_id, a10_bearer)
                .map_err(|e| format!("pcf bind_a10_bearer failed: {e}"))?;
            pcf.complete_a11_registration(pcf_session_id, a11_key)
                .map_err(|e| format!("pcf complete_a11_registration failed: {e}"))?;
        }

        self.legacy
            .open_packet_session(session_id, service_option, metadata)
            .await
    }

    async fn close_packet_session(&self, session_id: &str) {
        self.legacy.close_packet_session(session_id).await;
    }

    async fn set_sch_active(
        &self,
        session_id: &str,
        active: bool,
        rate_bps: u32,
    ) -> Result<(), String> {
        self.legacy
            .set_sch_active(session_id, active, rate_bps)
            .await
    }
}

/// gRPC-backed PCF client for network-separated packet data.
pub struct GrpcPcfClient {
    endpoint: String,
}

impl GrpcPcfClient {
    pub fn new(endpoint: String) -> Self {
        Self { endpoint }
    }
}

#[async_trait::async_trait]
impl PcfClient for GrpcPcfClient {
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
    > {
        use crate::grpc::packet_proto::{
            self as proto, packet_service_client::PacketServiceClient,
        };

        let mut client = PacketServiceClient::connect(self.endpoint.clone())
            .await
            .map_err(|e| format!("pcf gRPC connect failed: {e}"))?;

        let open_req = proto::OpenSessionRequest {
            session_id: session_id.clone(),
            service_option,
            esn: metadata.esn.unwrap_or(0),
            imsi: metadata.imsi.clone().unwrap_or_default(),
            mobile_address: metadata.mobile_address,
            // Proto3 represents "not set" as empty string. The receiver
            // converts back to `Option<String>` at the gRPC boundary.
            subscriber_id: metadata
                .subscriber_id
                .map(|u| u.to_string())
                .unwrap_or_default(),
            phone_number: metadata.phone_number,
            traffic_walsh_code: metadata.traffic_walsh_code,
        };
        client
            .open_session(open_req)
            .await
            .map_err(|e| format!("pcf OpenSession RPC failed: {e}"))?;

        let (uplink_tx, mut uplink_rx) = mpsc::channel::<PacketBearerFrame>(256);
        let (downlink_tx, downlink_rx) = mpsc::channel::<PacketBearerFrame>(256);

        let sid_for_stream = session_id.clone();
        let mut stream_client = PacketServiceClient::connect(self.endpoint.clone())
            .await
            .map_err(|e| format!("pcf gRPC stream connect failed: {e}"))?;

        tokio::spawn(async move {
            let outbound = async_stream::stream! {
                // First frame identifies the session
                yield proto::SessionFrame {
                    session_id: sid_for_stream.clone(),
                    bits: Vec::new(),
                    num_bits: 0,
                    rate_bps: 0,
                };
                while let Some(frame) = uplink_rx.recv().await {
                    yield proto::SessionFrame {
                        session_id: frame.session_id,
                        bits: frame.bits,
                        num_bits: frame.num_bits,
                        rate_bps: frame.rate_bps,
                    };
                }
            };
            match stream_client.stream_session(outbound).await {
                Ok(response) => {
                    let mut inbound = response.into_inner();
                    while let Ok(Some(frame)) = inbound.message().await {
                        let mapped = PacketBearerFrame {
                            session_id: frame.session_id,
                            bits: frame.bits,
                            num_bits: frame.num_bits,
                            rate_bps: frame.rate_bps,
                        };
                        if downlink_tx.send(mapped).await.is_err() {
                            break;
                        }
                    }
                }
                Err(e) => {
                    warn!("pcf StreamSession RPC failed: {e}");
                }
            }
        });

        info!("BSC: gRPC packet session {} opened", session_id);
        Ok((uplink_tx, downlink_rx))
    }

    async fn close_packet_session(&self, session_id: &str) {
        use crate::grpc::packet_proto::{
            self as proto, packet_service_client::PacketServiceClient,
        };

        match PacketServiceClient::connect(self.endpoint.clone()).await {
            Ok(mut client) => {
                let req = proto::CloseSessionRequest {
                    session_id: session_id.to_string(),
                };
                if let Err(e) = client.close_session(req).await {
                    warn!("pcf CloseSession RPC failed: {e}");
                }
            }
            Err(e) => {
                warn!("pcf gRPC connect failed for close: {e}");
            }
        }
    }

    async fn set_sch_active(
        &self,
        session_id: &str,
        active: bool,
        rate_bps: u32,
    ) -> Result<(), String> {
        use crate::grpc::packet_proto::{
            self as proto, packet_service_client::PacketServiceClient,
        };

        let mut client = PacketServiceClient::connect(self.endpoint.clone())
            .await
            .map_err(|e| format!("pcf gRPC connect failed for set_sch_active: {e}"))?;
        let req = proto::SetSchActiveRequest {
            session_id: session_id.to_string(),
            active,
            rate_bps,
        };
        client
            .set_sch_active(req)
            .await
            .map_err(|e| format!("pcf SetSchActive RPC failed: {e}"))?;
        Ok(())
    }
}
