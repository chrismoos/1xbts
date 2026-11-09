//! BSC-side A9 edge.
//!
//! 1x packet data to the PCF over A9 and A8, with RLP at the BSC (A.S0017-D).

use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};
use std::time::Duration;

use cdma_packet::rlp_link::RlpLink;
use log::{debug, info, warn};
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

pub use crate::packet::{PacketBearerFrame, PacketSessionMetadata, PcfClient};

/// Must not overlap the HRPD AN's UATI keys, since the PCF indexes A8 by key.
const BSC_A8_KEY_BASE: u32 = 0x4000_0000;
const A9_CAUSE_NORMAL_RELEASE: u8 = 0x14;
const A9_REPLY_TIMEOUT: Duration = Duration::from_millis(1_500);
const RLP_FRAME_PERIOD: Duration = Duration::from_millis(20);
const MAX_A9_MESSAGE_LEN: usize = 4096;
const MAX_GRE_FRAME_LEN: usize = 65_536;
const SESSION_CHANNEL_DEPTH: usize = 256;
const SINGLE_HOST_PCF_A8_PORT: u16 = 17041;
/// The HRPD AN uses 17040, so the BSC takes a different port.
const SINGLE_HOST_BSC_A8_PORT: u16 = 17048;

/// Between hosts, A9 uses port 5603 and A8 uses native GRE.
#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default)]
pub struct PacketA9Config {
    pub pcf_addr: SocketAddr,
    /// Must match the PCF's `a9_transport`.
    pub a9_transport: cdma_a9::A9Transport,
    /// Raw GRE uses only its IP.
    pub pcf_a8_addr: SocketAddr,
    /// Must differ from any HRPD AN's A8 address on the host.
    pub a8_bind_addr: SocketAddr,
    pub a8_transport: cdma_a8::BearerTransportMode,
    /// Largest PPP payload per A8 GRE frame. Defaults to 1400.
    pub max_gre_payload: Option<u16>,
}

impl Default for PacketA9Config {
    fn default() -> Self {
        Self {
            pcf_addr: SocketAddr::from((Ipv4Addr::LOCALHOST, cdma_a9::A9_SIGNALING_PORT)),
            a9_transport: cdma_a9::A9Transport::default(),
            pcf_a8_addr: SocketAddr::from((Ipv4Addr::LOCALHOST, SINGLE_HOST_PCF_A8_PORT)),
            a8_bind_addr: SocketAddr::from((Ipv4Addr::LOCALHOST, SINGLE_HOST_BSC_A8_PORT)),
            a8_transport: cdma_a8::BearerTransportMode::default(),
            max_gre_payload: None,
        }
    }
}

struct A8Session {
    session_id: String,
    release: cdma_a9::ReleaseA8Message,
    task: tokio::task::JoinHandle<()>,
    sch_tx: mpsc::UnboundedSender<Option<u32>>,
}

struct Shared {
    config: PacketA9Config,
    a9: tokio::sync::Mutex<cdma_a9::SignalingEndpoint>,
    gre: cdma_a8::GreSocket,
    bearers: Mutex<cdma_a8::BearerTable>,
    forward: Mutex<HashMap<u32, mpsc::UnboundedSender<Vec<u8>>>>,
    sessions: Mutex<HashMap<String, A8Session>>,
    next_key: AtomicU32,
}

pub struct A9PcfClient {
    shared: Arc<Shared>,
}

impl A9PcfClient {
    pub async fn bind(config: PacketA9Config) -> Result<Self, String> {
        let a9_bind = SocketAddr::new(config.a8_bind_addr.ip(), 0);
        let a9 = cdma_a9::SignalingEndpoint::client(a9_bind, config.a9_transport)
            .await
            .map_err(|e| format!("BSC A9: bind {a9_bind}: {e}"))?;
        let bearer = match config.a8_transport {
            cdma_a8::BearerTransportMode::RawGre => cdma_a8::BearerTransportConfig::raw_gre(),
            cdma_a8::BearerTransportMode::UdpEncapsulatedGre => {
                cdma_a8::BearerTransportConfig::udp_encapsulated_gre(
                    config.a8_bind_addr,
                    config.pcf_a8_addr,
                )
            }
        };
        let gre = cdma_a8::GreSocket::bind(bearer, config.a8_bind_addr.ip(), "BSC A8")
            .map_err(|e| format!("BSC A8: bind {}: {e}", config.a8_bind_addr))?;
        info!(
            "BSC packet data via PCF: A9 {} ({:?}), A8 {} -> {} ({:?})",
            config.pcf_addr,
            config.a9_transport,
            config.a8_bind_addr,
            config.pcf_a8_addr,
            config.a8_transport
        );
        let shared = Arc::new(Shared {
            config,
            a9: tokio::sync::Mutex::new(a9),
            gre,
            bearers: Mutex::new(cdma_a8::BearerTable::new()),
            forward: Mutex::new(HashMap::new()),
            sessions: Mutex::new(HashMap::new()),
            next_key: AtomicU32::new(0),
        });
        tokio::spawn(receive_a8(shared.clone()));
        Ok(Self { shared })
    }
}

fn a8_endpoint(config: &PacketA9Config) -> cdma_a8::BearerEndpoint {
    cdma_a8::BearerEndpoint::from_ip(config.a8_bind_addr.ip(), config.pcf_a8_addr.ip())
}

/// The PCF sends forward traffic to this IP (A.S0016-D §4.2.16).
fn a8_traffic_id(key: u32, config: &PacketA9Config) -> cdma_a9::A8TrafficId {
    match config.a8_bind_addr.ip() {
        IpAddr::V4(ip) => cdma_a9::A8TrafficId::gre_ppp(key, ip.octets()),
        IpAddr::V6(ip) => cdma_a9::A8TrafficId::gre_ppp_ipv6(key, ip.octets()),
    }
}

impl Shared {
    async fn transact(
        &self,
        payload: Vec<u8>,
        expected: cdma_a9::MessageType,
    ) -> Result<Vec<u8>, String> {
        let a9 = self.a9.lock().await;
        a9.send_payload(self.config.pcf_addr, &payload)
            .await
            .map_err(|e| format!("BSC A9: send to {}: {e}", self.config.pcf_addr))?;
        let mut buf = vec![0u8; MAX_A9_MESSAGE_LEN];
        let deadline = tokio::time::Instant::now() + A9_REPLY_TIMEOUT;
        loop {
            let (message, peer) = tokio::time::timeout_at(deadline, a9.recv(&mut buf))
                .await
                .map_err(|_| format!("BSC A9: no {expected:?} from {}", self.config.pcf_addr))?
                .map_err(|e| format!("BSC A9: receive: {e}"))?;
            if peer.ip() != self.config.pcf_addr.ip() {
                warn!("BSC A9: ignoring a message from {peer}");
                continue;
            }
            if message.message_type != expected {
                warn!(
                    "BSC A9: ignoring {:?} while waiting for {expected:?}",
                    message.message_type
                );
                continue;
            }
            return Ok(message.payload);
        }
    }

    async fn setup_a8(
        &self,
        key: u32,
        service_option: u32,
        metadata: &PacketSessionMetadata,
    ) -> Result<cdma_a9::ConnectA8Message, String> {
        let setup = cdma_a9::SetupA8Message {
            call_connection_reference: None,
            correlation_id: Some(cdma_a9::CorrelationId(key.to_be_bytes())),
            imsi: metadata.imsi.clone(),
            esn: metadata.esn,
            meid: None,
            con_ref: cdma_a9::ConRef(metadata.traffic_walsh_code as u8),
            quality_of_service_parameters: None,
            bsc_id: cdma_a9::BscId(b"1XBTS".to_vec()),
            a8_traffic_id: a8_traffic_id(key, &self.config),
            service_option: cdma_a9::ServiceOptionValue(service_option as u16),
            // Segments are safe because A8 payloads join one RLP byte stream
            // (A.S0008-C §4.2.17).
            a9_indicators: cdma_a9::A9Indicators {
                packet_boundary_supported: false,
                gre_segmentation_supported: true,
                sdb_supported: false,
                ccpd_mode: false,
                data_ready: true,
                handoff: false,
            },
            user_zone_id: None,
        };
        let payload = setup
            .encode()
            .map_err(|e| format!("BSC A9: encode Setup-A8: {e}"))?;
        let reply = self
            .transact(payload, cdma_a9::MessageType::ConnectA8)
            .await?;
        let connect = cdma_a9::ConnectA8Message::decode(&reply)
            .map_err(|e| format!("BSC A9: decode Connect-A8: {e}"))?;
        if connect.correlation_id != setup.correlation_id {
            return Err("BSC A9: Connect-A8 answers a different Setup-A8".to_string());
        }
        Ok(connect)
    }

    async fn release_a8(&self, release: &cdma_a9::ReleaseA8Message) -> Result<(), String> {
        let payload = release
            .encode()
            .map_err(|e| format!("BSC A9: encode Release-A8: {e}"))?;
        self.transact(payload, cdma_a9::MessageType::ReleaseA8Complete)
            .await
            .map(|_| ())
    }

    async fn send_a8(&self, key: u32, chunk: cdma_a8::BearerChunk) {
        let wire = match self.bearers.lock().build_outbound_chunk(key, chunk) {
            Ok(outbound) => outbound.wire_bytes,
            Err(e) => {
                warn!("BSC A8: encode for key 0x{key:08x}: {e}");
                return;
            }
        };
        if let Err(e) = self.gre.send_wire_to(&wire, self.config.pcf_a8_addr).await {
            warn!("BSC A8: send key 0x{key:08x}: {e}");
        }
    }
}

async fn receive_a8(shared: Arc<Shared>) {
    let endpoint = a8_endpoint(&shared.config);
    let mut buf = vec![0u8; MAX_GRE_FRAME_LEN];
    loop {
        if let Err(e) = shared.gre.readable().await {
            warn!("BSC A8: receive stopped: {e}");
            return;
        }
        while let Some((wire, from)) = shared.gre.try_recv_wire(&mut buf, "BSC A8") {
            if from.ip() != shared.config.pcf_a8_addr.ip() {
                continue;
            }
            let inbound = match shared.bearers.lock().decode_for_session(endpoint, &wire) {
                Ok(inbound) => inbound,
                // The raw socket also receives other A8 connections to this
                // address, such as the HRPD AN's.
                Err(e) => {
                    debug!("BSC A8: dropping frame from {from}: {e}");
                    continue;
                }
            };
            let forward = shared.forward.lock().get(&inbound.session_id).cloned();
            if let Some(forward) = forward {
                let _ = forward.send(inbound.payload);
            }
        }
    }
}

async fn run_rlp(
    shared: Arc<Shared>,
    key: u32,
    session_id: String,
    service_option: u32,
    mut reverse_aligner: cdma_a8::PppFrameAligner,
    mut uplink_rx: mpsc::Receiver<PacketBearerFrame>,
    mut forward_rx: mpsc::UnboundedReceiver<Vec<u8>>,
    mut sch_rx: mpsc::UnboundedReceiver<Option<u32>>,
    downlink_tx: mpsc::Sender<PacketBearerFrame>,
) {
    let mut rlp = RlpLink::for_service_option(service_option);
    rlp.set_log_context(session_id.clone());
    let mut pending_uplink = std::collections::VecDeque::new();
    let mut tick = tokio::time::interval(RLP_FRAME_PERIOD);
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        tokio::select! {
            frame = uplink_rx.recv() => match frame {
                Some(frame) => pending_uplink.push_back(frame),
                None => break,
            },
            octets = forward_rx.recv() => match octets {
                Some(octets) => rlp.enqueue(&octets),
                None => break,
            },
            Some(rate_bps) = sch_rx.recv() => {
                if let Err(e) = rlp.set_sch(rate_bps) {
                    warn!("BSC packet session {session_id}: {e}");
                }
            },
            _ = tick.tick() => {
                let frame = pending_uplink.pop_front();
                let delivered = rlp.receive(frame.as_ref().map(|f| (f.bits.as_slice(), f.rate_bps)));
                if let Some(octets) = delivered {
                    for chunk in reverse_aligner.push(&octets) {
                        shared.send_a8(key, chunk).await;
                    }
                }
                let frames = rlp.next_frames();
                let mut closed = false;
                for (bits, rate_bps) in std::iter::once(frames.fch).chain(frames.sch) {
                    if bits.is_empty() {
                        continue;
                    }
                    let frame = PacketBearerFrame {
                        session_id: session_id.clone(),
                        num_bits: bits.len() as u32,
                        bits,
                        rate_bps,
                    };
                    if downlink_tx.send(frame).await.is_err() {
                        closed = true;
                        break;
                    }
                }
                if closed {
                    break;
                }
            }
        }
    }
    debug!("BSC packet session {session_id}: RLP stopped");
}

#[async_trait::async_trait]
impl PcfClient for A9PcfClient {
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
        let shared = &self.shared;
        let key = BSC_A8_KEY_BASE | shared.next_key.fetch_add(1, Ordering::Relaxed);
        let connect = shared.setup_a8(key, service_option, &metadata).await?;
        let pcf_accepts_segmentation = connect
            .a9_indicators
            .is_some_and(|indicators| indicators.gre_segmentation_supported);
        let profile = cdma_a8::BearerProfile {
            packet_boundary_supported: true,
            gre_segmentation_supported: pcf_accepts_segmentation,
            ..cdma_a8::BearerProfile::standard_packet_data()
        };
        shared
            .bearers
            .lock()
            .apply_session(cdma_a8::BearerSession::with_profile(
                key,
                a8_endpoint(&shared.config),
                profile,
            ))
            .map_err(|e| format!("BSC A8: install key 0x{key:08x}: {e}"))?;
        let mut reverse_aligner = cdma_a8::PppFrameAligner::new(
            shared
                .config
                .max_gre_payload
                .unwrap_or(cdma_a8::DEFAULT_MAX_GRE_PAYLOAD),
        );
        reverse_aligner.set_segmentation(pcf_accepts_segmentation);
        let (forward_tx, forward_rx) = mpsc::unbounded_channel();
        shared.forward.lock().insert(key, forward_tx);

        let (uplink_tx, uplink_rx) = mpsc::channel(SESSION_CHANNEL_DEPTH);
        let (downlink_tx, downlink_rx) = mpsc::channel(SESSION_CHANNEL_DEPTH);
        let (sch_tx, sch_rx) = mpsc::unbounded_channel();
        let task = tokio::spawn(run_rlp(
            shared.clone(),
            key,
            session_id.clone(),
            service_option,
            reverse_aligner,
            uplink_rx,
            forward_rx,
            sch_rx,
            downlink_tx,
        ));
        let release = cdma_a9::ReleaseA8Message {
            call_connection_reference: connect.call_connection_reference,
            correlation_id: connect.correlation_id,
            imsi: metadata.imsi.clone(),
            esn: metadata.esn,
            meid: None,
            con_ref: connect.con_ref,
            a8_traffic_id: a8_traffic_id(key, &shared.config),
            cause: cdma_a9::CauseValue(A9_CAUSE_NORMAL_RELEASE),
        };
        let replaced = shared.sessions.lock().insert(
            session_id.clone(),
            A8Session {
                session_id: session_id.clone(),
                release,
                task,
                sch_tx,
            },
        );
        if let Some(stale) = replaced {
            stale.task.abort();
        }
        info!(
            "BSC packet session {session_id}: A8 key 0x{key:08x} SO {service_option} via PCF {}",
            shared.config.pcf_addr
        );
        Ok((uplink_tx, downlink_rx))
    }

    async fn close_packet_session(&self, session_id: &str) {
        let shared = &self.shared;
        let Some(session) = shared.sessions.lock().remove(session_id) else {
            return;
        };
        session.task.abort();
        let key = session.release.a8_traffic_id.key;
        shared.forward.lock().remove(&key);
        let _ = shared.bearers.lock().remove_session_if_present(key);
        match shared.release_a8(&session.release).await {
            Ok(()) => info!(
                "BSC packet session {}: A8 key 0x{key:08x} released",
                session.session_id
            ),
            Err(e) => warn!("BSC packet session {}: {e}", session.session_id),
        }
    }

    async fn set_sch_active(
        &self,
        session_id: &str,
        active: bool,
        rate_bps: u32,
    ) -> Result<(), String> {
        if active && cdma_common::sch::Rc3FschProfile::from_rate_bps(rate_bps).is_none() {
            return Err(format!(
                "packet session {session_id}: no RC3 F-SCH profile for {rate_bps} bps"
            ));
        }
        let sessions = self.shared.sessions.lock();
        let session = sessions
            .get(session_id)
            .ok_or_else(|| format!("packet session {session_id} is not open"))?;
        session
            .sch_tx
            .send(active.then_some(rate_bps))
            .map_err(|_| format!("packet session {session_id}: RLP has stopped"))
    }
}
