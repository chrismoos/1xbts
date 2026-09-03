//! The HRPD Access Network: A9/A8 session orchestration plus the air-event
//! path that drives the AN's upper layers from the BTS HRPD MAC.
//!
//! `C.S0024-100-C` §1.11 makes the access network equivalent to a base
//! station, so the AN's session, connection and stream layers run in the BTS
//! process and reach the air controller by direct call. The gRPC service is
//! the management view of the same state, not a step on the air path.

use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use super::hrpd::scheduler::{ForwardTrafficPacket, ForwardTrafficSender};
use super::{BtsNodeConfig, evdo};
use cdma_a8::HrpdA9ClientConfig;
use cdma_an::air::{HrpdAirController, HrpdSessionConfigurationCompleteEvent, HrpdTrafficOutcome};
use cdma_an::grpc::{AnServiceImpl, SessionStore, SharedUatiAllocator};
use cdma_an::{
    HrpdA9MobileIdentity, HrpdA9ReleaseContext, HrpdAnA8Runtime, HrpdAnA9Client,
    HrpdAnForwardTrafficPacket, HrpdDerivedImsiConfig, HrpdHardwareIdentity,
    hardware_identity_from_response, resolve_hrpd_a9_identity, spawn_hrpd_an_a8_runtime,
};
use cdma_an::{UatiAllocator, UatiSubnet};
use cdma_common::error::Error;
use cdma_common::hrpd::air as hrpd_air;
#[cfg(test)]
use cdma_pcf::spawn_hrpd_pcf_a9_service;
#[cfg(test)]
use cdma_pdsn::spawn_hrpd_pdsn_a11_service;
use log::{error, info, warn};
use tokio::sync::Mutex;

// HRPD default-packet protocol/RLP codes and A9/A8 orchestration constants.
const HRPD_DEFAULT_PACKET_STREAM1_PROTOCOL_TYPE: u8 = 0x15;
const HRPD_DEFAULT_PACKET_STREAM2_PROTOCOL_TYPE: u8 = 0x16;
const HRPD_DEFAULT_PACKET_STREAM3_PROTOCOL_TYPE: u8 = 0x17;
const HRPD_DEFAULT_PACKET_DATA_READY_ACK: u8 = 0x0c;
const HRPD_DEFAULT_PACKET_XON_REQUEST: u8 = 0x07;
const HRPD_DEFAULT_PACKET_XOFF_REQUEST: u8 = 0x09;
const HRPD_ADDRESS_MANAGEMENT_PROTOCOL_TYPE: u8 = 0x11;
const HRPD_UATI_ASSIGNMENT_MESSAGE_ID: u8 = 0x01;
const HRPD_ENHANCED_IDLE_DEFAULT_PAGE_PERIOD_CYCLES: u16 = 12;
/// Management/session gRPC listener for the AN when `evdo.an_grpc_bind_addr`
/// names none. The web UI reads HRPD sessions and UATI allocation from here.
const HRPD_AN_MANAGEMENT_GRPC_ADDR: &str = "127.0.0.1:17030";
/// Cadence of the AN session/connection timer tick.
const HRPD_AN_TIMER_INTERVAL: Duration = Duration::from_millis(100);
/// Reporting window for the Stream 1 path timing summary.
const HRPD_STREAM1_TIMING_WINDOW: Duration = Duration::from_secs(5);

#[derive(Clone, Debug)]
pub(crate) struct PendingHrpdA9Session {
    session_uati: u32,
    request: hrpd_air::HrpdTrafficAssignmentRequest,
    identity: Option<HrpdA9MobileIdentity>,
    session_configuration_complete: bool,
}

// Only UATI-keyed identities may reach A9: SetupA8 identifies the addressed
// mobile, so an unresolved setup stays pending until HardwareIDResponse
// resolves it rather than borrowing another AT's identity.
pub(crate) fn cached_hrpd_a9_identity(
    identities: &HashMap<u32, HrpdA9MobileIdentity>,
    session_uati: u32,
    traffic_uati: u32,
) -> Option<HrpdA9MobileIdentity> {
    identities
        .get(&session_uati)
        .or_else(|| identities.get(&traffic_uati))
        .cloned()
}

pub(crate) fn pending_hrpd_a9_identity_needs_imsi(pending: &PendingHrpdA9Session) -> bool {
    pending
        .identity
        .as_ref()
        .and_then(|identity| identity.imsi.as_ref())
        .is_none()
}

#[derive(Clone, Debug)]
pub(crate) struct HrpdA9ReleaseRequest {
    uati: u32,
    reason: String,
}

pub(crate) fn uati_from_access_ati(
    ati: hrpd_air::AccessTerminalIdentifier,
    color_code: u8,
) -> Option<u32> {
    if ati.ati_type != hrpd_air::AccessTerminalIdentifierType::Uati {
        return None;
    }
    if ((ati.value >> 24) as u8) != color_code {
        return None;
    }
    Some(ati.value & 0x00ff_ffff)
}

pub(crate) fn session_uati_from_hrpd_traffic_uati(traffic_uati: u32) -> u32 {
    traffic_uati & 0x00ff_ffff
}

pub(crate) fn hrpd_traffic_uati_from_session_uati(session_uati: u32, color_code: u8) -> u32 {
    (u32::from(color_code) << 24) | (session_uati & 0x00ff_ffff)
}

pub(crate) fn default_packet_flow_open_for_pending(
    open_uatis: &HashSet<u32>,
    pending: &PendingHrpdA9Session,
) -> bool {
    open_uatis.contains(&pending.request.uati) || open_uatis.contains(&pending.session_uati)
}

pub(crate) fn remember_default_packet_flow_open(open_uatis: &mut HashSet<u32>, uati: u32) {
    open_uatis.insert(uati);
    open_uatis.insert(session_uati_from_hrpd_traffic_uati(uati));
}

pub(crate) fn forget_default_packet_flow_open(open_uatis: &mut HashSet<u32>, uati: u32) {
    open_uatis.remove(&uati);
    open_uatis.remove(&session_uati_from_hrpd_traffic_uati(uati));
}

pub(crate) fn access_default_packet_data_ready_acks(
    indication: &hrpd_air::HrpdAccessIndication,
) -> Vec<u8> {
    indication
        .messages
        .iter()
        .filter_map(|message| match message {
            hrpd_air::HrpdAccessMessage::DefaultPacketDataReadyAck(ack) => Some(ack.transaction_id),
            hrpd_air::HrpdAccessMessage::Unknown {
                protocol_type,
                message_id: Some(message_id),
                payload,
            } if is_hrpd_default_packet_stream_protocol_type(*protocol_type)
                && *message_id == HRPD_DEFAULT_PACKET_DATA_READY_ACK =>
            {
                payload.get(1).copied()
            }
            _ => None,
        })
        .collect()
}

pub(crate) fn access_default_packet_flow_requests(
    indication: &hrpd_air::HrpdAccessIndication,
) -> Vec<bool> {
    indication
        .messages
        .iter()
        .filter_map(|message| match message {
            hrpd_air::HrpdAccessMessage::DefaultPacketXonRequest => Some(true),
            hrpd_air::HrpdAccessMessage::DefaultPacketXoffRequest => Some(false),
            hrpd_air::HrpdAccessMessage::Unknown {
                protocol_type,
                message_id: Some(message_id),
                ..
            } if is_hrpd_default_packet_stream_protocol_type(*protocol_type) => match *message_id {
                HRPD_DEFAULT_PACKET_XON_REQUEST => Some(true),
                HRPD_DEFAULT_PACKET_XOFF_REQUEST => Some(false),
                _ => None,
            },
            _ => None,
        })
        .collect()
}

pub(crate) fn is_hrpd_default_packet_stream_protocol_type(protocol_type: u8) -> bool {
    matches!(
        protocol_type,
        HRPD_DEFAULT_PACKET_STREAM1_PROTOCOL_TYPE
            | HRPD_DEFAULT_PACKET_STREAM2_PROTOCOL_TYPE
            | HRPD_DEFAULT_PACKET_STREAM3_PROTOCOL_TYPE
    )
}

pub(crate) fn hrpd_uati_subnet_assignment(
    sector_id: [u8; 16],
    on_air_subnet_mask: u8,
) -> hrpd_air::HrpdUatiSubnetAssignment {
    let mut uati104 = [0u8; 13];
    // C.S0024-500 Address Management: explicit UATI assignment carries
    // UATI104 = UATI[127:24]. Keep UATI024 allocator-local, but bind the
    // upper 104 bits to the sector subnet so the AT's open-state subnet check
    // matches SectorID/SubnetMask.
    uati104.copy_from_slice(&sector_id[..13]);
    hrpd_air::HrpdUatiSubnetAssignment {
        uati_subnet_mask: on_air_subnet_mask,
        uati104,
    }
}

pub(crate) async fn try_complete_hrpd_a9_setup(
    pending: PendingHrpdA9Session,
    a9_config: Option<HrpdA9ClientConfig>,
    a9_endpoint: Option<&cdma_a9::UdpSignalingEndpoint>,
    an_a8_runtime: Option<&HrpdAnA8Runtime>,
    traffic_open_uatis: &HashSet<u32>,
    default_packet_flow_open_uatis: &HashSet<u32>,
    sequence_no: &mut u32,
    reason: &str,
) -> Result<HrpdA9ReleaseContext, PendingHrpdA9Session> {
    let (Some(a9_config), Some(a9_endpoint)) = (a9_config, a9_endpoint) else {
        return Err(pending);
    };
    if !pending.session_configuration_complete {
        info!(
            "HRPD AN: deferring A9 SetupA8 UATI=0x{:08x}; waiting for SessionConfigurationComplete",
            pending.request.uati
        );
        return Err(pending);
    }
    if pending
        .identity
        .as_ref()
        .and_then(|identity| identity.imsi.as_ref())
        .is_none()
    {
        info!(
            "HRPD AN: deferring A9 SetupA8 UATI=0x{:08x}; waiting for IMSI-format A11 MSID",
            pending.request.uati
        );
        return Err(pending);
    }
    let identity = pending.identity.clone();
    let mut client = HrpdAnA9Client::with_sequence(a9_endpoint, a9_config, *sequence_no);
    let result = client.setup_a8(&pending.request, identity.as_ref()).await;
    *sequence_no = client.sequence_no();
    match result {
        Ok(context) => {
            info!(
                "HRPD AN: A9 SetupA8 connected after {reason} UATI=0x{:08x} MAC={} A8Key=0x{:08x}",
                pending.request.uati,
                pending.request.mac_index,
                context.a8_key()
            );
            if let Some(runtime) = an_a8_runtime {
                runtime.register(
                    pending.session_uati,
                    pending.request.uati,
                    pending.request.mac_index,
                    cdma_a8::BearerSession::new(context.a8_key(), client.an_a8_endpoint()),
                );
                if default_packet_flow_open_for_pending(default_packet_flow_open_uatis, &pending) {
                    runtime.set_default_packet_flow_open(pending.request.uati, true);
                }
                if traffic_open_uatis.contains(&pending.request.uati) {
                    runtime.set_traffic_channel_open(pending.request.uati, true);
                }
            }
            Ok(context)
        }
        Err(err) => {
            warn!("{err}");
            Err(pending)
        }
    }
}

pub(crate) async fn release_hrpd_a9_and_a10_for_uati(
    uati: u32,
    reason: &str,
    active_a9_sessions: &mut HashMap<u32, HrpdA9ReleaseContext>,
    a9_config: Option<HrpdA9ClientConfig>,
    a9_endpoint: Option<&cdma_a9::UdpSignalingEndpoint>,
    a9_sequence_no: &mut u32,
) {
    if let Some(context) = active_a9_sessions.remove(&uati) {
        match (a9_config, a9_endpoint) {
            (Some(config), Some(endpoint)) => {
                let mut client = HrpdAnA9Client::with_sequence(endpoint, config, *a9_sequence_no);
                let result = client.release_a8(uati, &context, reason).await;
                *a9_sequence_no = client.sequence_no();
                if let Err(err) = result {
                    warn!("{err}");
                }
            }
            _ => {
                info!(
                    "HRPD AN: no A9 endpoint available while releasing UATI=0x{uati:08x} after {reason}"
                );
            }
        }
    } else {
        info!("HRPD AN: no active A9 state for UATI=0x{uati:08x} after {reason}");
    }
}

/// The MCC and IMSI_11_12 the AN derives HRPD subscriber identities from,
/// read out of the broadcast Extended System Parameters so a derived IMSI
/// matches what the cell advertises.
pub fn hrpd_derived_imsi_config(
    bts_config: &BtsNodeConfig,
) -> Result<HrpdDerivedImsiConfig, Error> {
    let esp = &bts_config
        .runtime
        .downlink
        .paging
        .message_defaults
        .extended_system_parameters;
    let mcc = cdma_common::paging::mcc_to_digits(esp.mcc)
        .ok_or_else(|| Error::from(format!("invalid BTS overhead MCC encoding {}", esp.mcc)))?;
    let imsi_11_12 =
        cdma_common::paging::imsi_11_12_to_digits(esp.imsi_11_12).ok_or_else(|| {
            Error::from(format!(
                "invalid BTS overhead IMSI_11_12 encoding {}",
                esp.imsi_11_12
            ))
        })?;
    Ok(HrpdDerivedImsiConfig { mcc, imsi_11_12 })
}

/// The BTS-side HRPD air channels the AN drives. Field names match the
/// `hrpd_*` members of `BtsHandle` they are taken from.
pub struct HrpdAirChannels {
    pub access_events: tokio::sync::mpsc::UnboundedReceiver<hrpd_air::HrpdAccessIndication>,
    pub traffic_events: tokio::sync::mpsc::UnboundedReceiver<hrpd_air::HrpdTrafficEvent>,
    pub forward_signaling:
        tokio::sync::mpsc::UnboundedSender<hrpd_air::HrpdForwardSignalingRequest>,
    pub traffic_assignments:
        tokio::sync::mpsc::UnboundedSender<hrpd_air::HrpdTrafficAssignmentRequest>,
    pub traffic_releases: tokio::sync::mpsc::UnboundedSender<hrpd_air::HrpdTrafficReleaseRequest>,
    pub forward_traffic: ForwardTrafficSender,
}

/// What a started AN hands back to the process that owns it.
pub struct HrpdAn {
    /// Address of the AN's management/session gRPC listener.
    pub grpc_addr: SocketAddr,
    /// The AN's in-process handle. Session and UATI state stay reachable
    /// through it, and it is the only route to the air controller.
    pub service: AnServiceImpl,
}

/// Start the HRPD Access Network for this sector and bind it to the BTS air
/// channels. Returns `Ok(None)` when EV-DO is disabled in `bts.json`.
///
/// Color code, UATI subnet, pilot offset and the advertised HRPD channel
/// record all come from the `evdo` block, so the AN's on-air identity matches
/// what the BTS transmits.
pub async fn spawn_hrpd_an(
    bts_config: &BtsNodeConfig,
    events_endpoint: Option<&str>,
    a9_config: Option<HrpdA9ClientConfig>,
    hlr_repo: Option<Arc<dyn cdma_hlr::repository::HlrRepository>>,
    derived_imsi_config: HrpdDerivedImsiConfig,
    channels: HrpdAirChannels,
) -> Result<Option<HrpdAn>, Error> {
    if !bts_config.evdo.enabled {
        return Ok(None);
    }
    let overhead = bts_config.evdo.overhead.resolve()?;
    let configured_addr: SocketAddr = match bts_config.evdo.an_grpc_bind_addr {
        Some(addr) => addr,
        None => HRPD_AN_MANAGEMENT_GRPC_ADDR
            .parse()
            .expect("default AN gRPC address should parse"),
    };
    // Bind before serving so a second EV-DO BTS on this host fails startup
    // with the address conflict named, rather than reporting a running AN
    // whose listener never came up.
    let listener = tokio::net::TcpListener::bind(configured_addr)
        .await
        .map_err(|e| Error::from(format!("AN gRPC bind {configured_addr} failed: {e}")))?;
    let addr = listener
        .local_addr()
        .map_err(|e| Error::from(format!("AN gRPC local address unavailable: {e}")))?;
    let uati_subnet_assignment =
        hrpd_uati_subnet_assignment(overhead.sector_id, overhead.subnet_mask);
    let subnet = UatiSubnet {
        color_code: overhead.color_code,
        uati104: uati_subnet_assignment.uati104,
        subnet_mask: uati_subnet_assignment.uati_subnet_mask,
    };
    let resolved_evdo = evdo::resolve_evdo_config(
        &bts_config.evdo,
        bts_config.pilot_offset,
        bts_config.channel,
        bts_config.runtime.tx_sample_rate_hz,
        bts_config.runtime.tx_bandwidth_hz,
    )?
    .ok_or_else(|| Error::from("HRPD AN enabled but EV-DO carrier did not resolve"))?;
    let hrpd_channel = hrpd_air::HrpdChannelRecord {
        system_type: 0x00,
        band_class: resolved_evdo.evdo_band_class & 0x1f,
        channel_number: resolved_evdo.evdo_channel & 0x07ff,
    };
    let sessions: SessionStore = Arc::new(Mutex::new(HashMap::new()));
    let uati: SharedUatiAllocator = Arc::new(Mutex::new(UatiAllocator::new(subnet)));
    let air = Arc::new(Mutex::new(HrpdAirController::with_sector_and_uati_subnet(
        overhead.color_code,
        bts_config.pilot_offset as u16,
        Some(hrpd_channel),
        Some(uati_subnet_assignment),
    )));
    let service = AnServiceImpl::new_with_air(sessions, uati, air);
    let cell = cdma_events::proto::CellId {
        cell: u32::from(bts_config.overhead.base_id),
        sector: u32::from(bts_config.sector),
    };
    let service = match events_endpoint {
        Some(endpoint) => {
            // One AN runs per cell, so the bus identity names the cell it
            // serves and stays distinct across the ANs on a BSC.
            let producer_instance = format!("an-{}-{}", cell.cell, cell.sector);
            let publisher = cdma_events::EventPublisher::spawn(
                cdma_events::EventPublisherConfig::new(endpoint.to_string(), producer_instance),
            )
            .map_err(|e| Error::from(format!("invalid AN events endpoint: {e}")))?;
            info!("HRPD AN events publishing to {endpoint}");
            let sink = Arc::new(cdma_an::events::AnEventSink::new(
                publisher,
                u32::from(overhead.color_code),
                cell,
            ));
            service.with_events(sink)
        }
        None => service,
    };
    let grpc_service = service.clone();
    tokio::spawn(async move {
        info!("AN gRPC session/management service listening on {addr}");
        if let Err(err) = tonic::transport::Server::builder()
            .add_service(grpc_service.into_server())
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
            .await
        {
            log::error!("AN gRPC server error: {err}");
        }
    });
    info!(
        "HRPD AN enabled: random UATI024 allocator on_air_mask=/{} color_code={} management=http://{} traffic_channel=system_type=0x{:02x}/bc{}/ch{}",
        overhead.subnet_mask,
        subnet.color_code,
        addr,
        hrpd_channel.system_type,
        hrpd_channel.band_class,
        hrpd_channel.channel_number
    );
    spawn_hrpd_air_tasks(
        service.clone(),
        channels,
        a9_config,
        hlr_repo,
        overhead.color_code,
        derived_imsi_config,
    );
    Ok(Some(HrpdAn {
        grpc_addr: addr,
        service,
    }))
}

#[derive(Default)]
pub(crate) struct HrpdStream1BridgeTiming {
    window_started: Option<Instant>,
    samples: u64,
    decoded_timestamp_samples: u64,
    air_timestamp_samples: u64,
    octets: u64,
    a8_packets: u64,
    a8_octets: u64,
    queue_us_sum: u128,
    queue_us_max: u128,
    an_us_sum: u128,
    an_us_max: u128,
    total_us_sum: u128,
    total_us_max: u128,
    air_to_decode_us_sum: u128,
    air_to_decode_us_max: u128,
    air_to_an_done_us_sum: u128,
    air_to_an_done_us_max: u128,
    backlog_max: usize,
}

impl HrpdStream1BridgeTiming {
    // The arguments are the per-event timing samples. Bundling them would
    // allocate on the reverse hot path for one call site.
    #[allow(clippy::too_many_arguments)]
    fn record(
        &mut self,
        air_frame_end_received_at: Option<Instant>,
        decoded_at: Option<Instant>,
        dequeued_at: Instant,
        an_elapsed: Duration,
        octets: usize,
        a8_packets: usize,
        a8_octets: usize,
        backlog: usize,
    ) {
        let now = Instant::now();
        let window_started = *self.window_started.get_or_insert(now);
        self.samples += 1;
        self.octets += octets as u64;
        self.a8_packets += a8_packets as u64;
        self.a8_octets += a8_octets as u64;
        self.an_us_sum += an_elapsed.as_micros();
        self.an_us_max = self.an_us_max.max(an_elapsed.as_micros());
        self.backlog_max = self.backlog_max.max(backlog);
        if let Some(decoded_at) = decoded_at {
            let queue_us = dequeued_at
                .saturating_duration_since(decoded_at)
                .as_micros();
            let total_us = now.saturating_duration_since(decoded_at).as_micros();
            self.decoded_timestamp_samples += 1;
            self.queue_us_sum += queue_us;
            self.queue_us_max = self.queue_us_max.max(queue_us);
            self.total_us_sum += total_us;
            self.total_us_max = self.total_us_max.max(total_us);
        }
        if let Some(air_received_at) = air_frame_end_received_at {
            self.air_timestamp_samples += 1;
            if let Some(decoded_at) = decoded_at {
                let air_to_decode_us = decoded_at
                    .saturating_duration_since(air_received_at)
                    .as_micros();
                self.air_to_decode_us_sum += air_to_decode_us;
                self.air_to_decode_us_max = self.air_to_decode_us_max.max(air_to_decode_us);
            }
            let air_to_an_done_us = now.saturating_duration_since(air_received_at).as_micros();
            self.air_to_an_done_us_sum += air_to_an_done_us;
            self.air_to_an_done_us_max = self.air_to_an_done_us_max.max(air_to_an_done_us);
        }
        if window_started.elapsed() < HRPD_STREAM1_TIMING_WINDOW {
            return;
        }
        let decoded_samples = u128::from(self.decoded_timestamp_samples.max(1));
        let air_samples = u128::from(self.air_timestamp_samples.max(1));
        log::info!(
            "HRPD Stream1 path timing: samples={} octets={} air_to_decode_us_avg={:.1} air_to_decode_us_max={} decode_to_an_us_avg={:.1} decode_to_an_us_max={} an_handle_us_avg={:.1} an_handle_us_max={} decode_to_an_done_us_avg={:.1} decode_to_an_done_us_max={} air_to_an_done_us_avg={:.1} air_to_an_done_us_max={} queue_backlog_max={} a8_packets={} a8_octets={}",
            self.samples,
            self.octets,
            self.air_to_decode_us_sum as f64 / air_samples as f64,
            self.air_to_decode_us_max,
            self.queue_us_sum as f64 / decoded_samples as f64,
            self.queue_us_max,
            self.an_us_sum as f64 / self.samples as f64,
            self.an_us_max,
            self.total_us_sum as f64 / decoded_samples as f64,
            self.total_us_max,
            self.air_to_an_done_us_sum as f64 / air_samples as f64,
            self.air_to_an_done_us_max,
            self.backlog_max,
            self.a8_packets,
            self.a8_octets,
        );
        *self = Self::default();
    }
}

fn forward_traffic_from_air_packet(
    packet: hrpd_air::HrpdForwardTrafficPacket,
) -> ForwardTrafficPacket {
    ForwardTrafficPacket {
        mac_index: packet.mac_index,
        physical_layer_subtype: packet.physical_layer_subtype,
        forward_traffic_mac_subtype: packet.forward_traffic_mac_subtype,
        high_priority: false,
        payload: packet.payload_bits,
    }
}

fn forward_traffic_from_an_packet(packet: HrpdAnForwardTrafficPacket) -> ForwardTrafficPacket {
    ForwardTrafficPacket {
        mac_index: packet.mac_index,
        physical_layer_subtype: packet.physical_layer_subtype,
        forward_traffic_mac_subtype: packet.forward_traffic_mac_subtype,
        high_priority: packet.high_priority,
        payload: packet.payload,
    }
}

/// Relay AN-side forward-traffic packets to the BTS scheduler, converting each
/// to the scheduler's packet format. Runs until the AN or BTS queue closes.
async fn relay_an_forward_traffic_to_bts(
    mut an_rx: tokio::sync::mpsc::UnboundedReceiver<HrpdAnForwardTrafficPacket>,
    bts_tx: ForwardTrafficSender,
) {
    while let Some(packet) = an_rx.recv().await {
        if bts_tx.send(forward_traffic_from_an_packet(packet)).is_err() {
            warn!("HRPD AN: BTS forward-traffic queue closed");
            break;
        }
    }
}

fn spawn_hrpd_air_tasks(
    service: AnServiceImpl,
    channels: HrpdAirChannels,
    a9_config: Option<HrpdA9ClientConfig>,
    hlr_repo: Option<Arc<dyn cdma_hlr::repository::HlrRepository>>,
    color_code: u8,
    derived_imsi_config: HrpdDerivedImsiConfig,
) {
    let HrpdAirChannels {
        access_events: access_rx,
        traffic_events: traffic_rx,
        forward_signaling: forward_tx,
        traffic_assignments: traffic_assignment_tx,
        traffic_releases: traffic_release_tx,
        forward_traffic: forward_traffic_tx,
    } = channels;
    tokio::spawn(async move {
        let stream1_timing = HrpdStream1BridgeTiming::default();
        let (an_forward_traffic_tx, an_forward_traffic_rx) =
            tokio::sync::mpsc::unbounded_channel::<HrpdAnForwardTrafficPacket>();
        let an_forward_traffic_to_bts_tx = forward_traffic_tx.clone();
        tokio::spawn(relay_an_forward_traffic_to_bts(
            an_forward_traffic_rx,
            an_forward_traffic_to_bts_tx,
        ));
        let an_a8_runtime = match a9_config {
            Some(config) => match spawn_hrpd_an_a8_runtime(
                config.an_a8_bearer,
                config.an_a8_endpoint,
                forward_tx.clone(),
                an_forward_traffic_tx,
            ) {
                Ok(runtime) => Some(runtime),
                Err(err) => {
                    error!(
                        "HRPD AN: A8 bearer bind failed, packet data is unavailable on this cell: {err}"
                    );
                    None
                }
            },
            None => None,
        };
        let a9_endpoint = match a9_config {
            // Wildcard, not loopback: the PCF may be on another host, and a
            // loopback-bound socket cannot send off-host.
            Some(_) => match "0.0.0.0:0".parse::<SocketAddr>() {
                Ok(bind_addr) => match cdma_a9::UdpSignalingEndpoint::bind(bind_addr).await {
                    Ok(endpoint) => Some(endpoint),
                    Err(err) => {
                        warn!("HRPD AN: failed to bind ephemeral A9 client: {err}");
                        None
                    }
                },
                Err(err) => {
                    warn!("HRPD AN: invalid ephemeral A9 bind address: {err}");
                    None
                }
            },
            None => None,
        };
        let pending_a9_sessions: Arc<Mutex<HashMap<u32, PendingHrpdA9Session>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let session_a9_identities: Arc<Mutex<HashMap<u32, HrpdA9MobileIdentity>>> =
            Arc::new(Mutex::new(HashMap::new()));
        let session_a9_config_complete: Arc<Mutex<HashSet<u32>>> =
            Arc::new(Mutex::new(HashSet::new()));
        let (a9_release_tx, a9_release_rx) =
            tokio::sync::mpsc::unbounded_channel::<HrpdA9ReleaseRequest>();
        let traffic_service = service.clone();
        let traffic_a8_runtime = an_a8_runtime.clone();
        let traffic_forward_tx = forward_tx.clone();
        let traffic_forward_traffic_tx = forward_traffic_tx.clone();
        let traffic_release_tx_for_traffic = traffic_release_tx.clone();
        let traffic_pending_a9_sessions = pending_a9_sessions.clone();
        let traffic_session_a9_identities = session_a9_identities.clone();
        let traffic_session_a9_config_complete = session_a9_config_complete.clone();
        let traffic_hlr_repo = hlr_repo.clone();
        let traffic_derived_imsi_config = derived_imsi_config.clone();
        tokio::spawn(hrpd_an_traffic_event_task(
            traffic_service,
            a9_config,
            a9_endpoint,
            traffic_rx,
            a9_release_rx,
            stream1_timing,
            traffic_a8_runtime,
            traffic_forward_tx,
            traffic_forward_traffic_tx,
            traffic_release_tx_for_traffic,
            traffic_pending_a9_sessions,
            traffic_session_a9_identities,
            traffic_session_a9_config_complete,
            traffic_hlr_repo,
            traffic_derived_imsi_config,
        ));
        hrpd_an_access_task(
            service,
            access_rx,
            forward_tx,
            traffic_assignment_tx,
            traffic_release_tx,
            forward_traffic_tx,
            a9_config,
            hlr_repo,
            color_code,
            derived_imsi_config,
            an_a8_runtime,
            pending_a9_sessions,
            session_a9_identities,
            session_a9_config_complete,
            a9_release_tx,
        )
        .await;
    });
}

/// Drive the AN-side traffic-event loop: unsolicited A9 datagrams, reverse
/// traffic events, and the periodic air timer, until the BTS traffic stream
/// closes.
#[allow(clippy::too_many_arguments)]
async fn hrpd_an_traffic_event_task(
    service: AnServiceImpl,
    a9_config: Option<HrpdA9ClientConfig>,
    a9_endpoint: Option<cdma_a9::UdpSignalingEndpoint>,
    mut traffic_rx: tokio::sync::mpsc::UnboundedReceiver<hrpd_air::HrpdTrafficEvent>,
    mut a9_release_rx: tokio::sync::mpsc::UnboundedReceiver<HrpdA9ReleaseRequest>,
    mut stream1_timing: HrpdStream1BridgeTiming,
    traffic_a8_runtime: Option<HrpdAnA8Runtime>,
    traffic_forward_tx: tokio::sync::mpsc::UnboundedSender<hrpd_air::HrpdForwardSignalingRequest>,
    traffic_forward_traffic_tx: ForwardTrafficSender,
    traffic_release_tx_for_traffic: tokio::sync::mpsc::UnboundedSender<
        hrpd_air::HrpdTrafficReleaseRequest,
    >,
    traffic_pending_a9_sessions: Arc<Mutex<HashMap<u32, PendingHrpdA9Session>>>,
    traffic_session_a9_identities: Arc<Mutex<HashMap<u32, HrpdA9MobileIdentity>>>,
    traffic_session_a9_config_complete: Arc<Mutex<HashSet<u32>>>,
    traffic_hlr_repo: Option<Arc<dyn cdma_hlr::repository::HlrRepository>>,
    traffic_derived_imsi_config: HrpdDerivedImsiConfig,
) {
    let mut a9_sequence_no = 0u32;
    let mut traffic_open_uatis: HashSet<u32> = HashSet::new();
    let mut default_packet_flow_open_uatis: HashSet<u32> = HashSet::new();
    let mut active_a9_sessions: HashMap<u32, HrpdA9ReleaseContext> = HashMap::new();
    let mut unsolicited_a9_buf = vec![0u8; 4096];
    let mut air_timer = tokio::time::interval(HRPD_AN_TIMER_INTERVAL);
    air_timer.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
    loop {
        let outcome: HrpdTrafficOutcome = tokio::select! {
            a9 = async {
                match a9_endpoint.as_ref() {
                    Some(endpoint) => endpoint.recv_datagram(&mut unsolicited_a9_buf).await,
                    None => std::future::pending().await,
                }
            } => {
                let (datagram, peer) = match a9 {
                    Ok(value) => value,
                    Err(err) => {
                        warn!("HRPD AN A9: failed to receive unsolicited datagram: {err}");
                        continue;
                    }
                };
                if let Some(config) = a9_config && peer != config.pcf_addr {
                    warn!(
                        "HRPD AN A9: unsolicited message came from unexpected peer {peer}, expected {}",
                        config.pcf_addr
                    );
                    continue;
                }
                if datagram.message_type != cdma_a9::MessageType::DisconnectA8 {
                    warn!(
                        "HRPD AN A9: ignoring unsupported unsolicited message {:?} from {peer}",
                        datagram.message_type
                    );
                    continue;
                }
                let disconnect = match cdma_a9::DisconnectA8Message::decode(&datagram.payload) {
                    Ok(disconnect) => disconnect,
                    Err(err) => {
                        warn!("HRPD AN A9: invalid DisconnectA8 from {peer}: {err}");
                        continue;
                    }
                };
                let uati = disconnect.a8_traffic_id.key;
                let context = active_a9_sessions.remove(&uati).unwrap_or_else(|| {
                    warn!(
                        "HRPD AN A9: DisconnectA8 for unknown active A8 key=0x{uati:08x}; replying from message fields"
                    );
                    HrpdA9ReleaseContext::from_parts(
                        disconnect.call_connection_reference,
                        disconnect.correlation_id,
                        HrpdA9MobileIdentity {
                            imsi: disconnect.imsi.clone(),
                            esn: disconnect.esn,
                            meid: disconnect.meid,
                        },
                        disconnect.con_ref,
                        disconnect.a8_traffic_id.clone(),
                    )
                });
                if let (Some(config), Some(endpoint)) = (a9_config, a9_endpoint.as_ref()) {
                    let mut client =
                        HrpdAnA9Client::with_sequence(endpoint, config, a9_sequence_no);
                    let result = client
                        .release_a8_with_cause(
                            uati,
                            &context,
                            "PCF DisconnectA8",
                            disconnect.cause,
                        )
                        .await;
                    a9_sequence_no = client.sequence_no();
                    if let Err(err) = result {
                        warn!("{err}");
                    }
                }
                if let Some(runtime) = traffic_a8_runtime.as_ref() {
                    runtime.release_a8(uati);
                }
                traffic_open_uatis.remove(&uati);
                forget_default_packet_flow_open(&mut default_packet_flow_open_uatis, uati);
                let outcome = service
                    .handle_a9_disconnect_a8(uati, context.con_ref().0, disconnect.cause.0)
                    .await;
                info!(
                    "HRPD AN A9: handled DisconnectA8 UATI=0x{uati:08x} MAC={} cause=0x{:02x}",
                    context.con_ref().0,
                    disconnect.cause.0
                );
                outcome
            }
            release = a9_release_rx.recv() => {
                let Some(release) = release else {
                    continue;
                };
                traffic_open_uatis.remove(&release.uati);
                forget_default_packet_flow_open(
                    &mut default_packet_flow_open_uatis,
                    release.uati,
                );
                release_hrpd_a9_and_a10_for_uati(
                    release.uati,
                    &release.reason,
                    &mut active_a9_sessions,
                    a9_config,
                    a9_endpoint.as_ref(),
                    &mut a9_sequence_no,
                )
                .await;
                if let Some(runtime) = traffic_a8_runtime.as_ref() {
                    runtime.release_session(release.uati);
                }
                continue;
            }
            event = traffic_rx.recv() => {
                let Some(event) = event else {
                    break;
                };
                let stream1_context = match &event {
                    hrpd_air::HrpdTrafficEvent::Stream1Packet {
                        payload,
                        decoded_at,
                        air_frame_end_received_at,
                        ..
                    } => Some((
                        *air_frame_end_received_at,
                        *decoded_at,
                        payload.len(),
                        traffic_rx.len(),
                    )),
                    _ => None,
                };
                if let Some(runtime) = traffic_a8_runtime.as_ref()
                    && let hrpd_air::HrpdTrafficEvent::Drc {
                        uati, drc_index, ..
                    } = &event
                {
                    runtime.update_drc(*uati, *drc_index);
                }
                let dequeued_at = Instant::now();
                let outcome = service.handle_traffic(&event).await;
                if let Some((air_received_at, decoded_at, octets, backlog)) = stream1_context {
                    stream1_timing.record(
                        air_received_at,
                        decoded_at,
                        dequeued_at,
                        dequeued_at.elapsed(),
                        octets,
                        outcome.a8_uplink.len(),
                        outcome
                            .a8_uplink
                            .iter()
                            .map(|packet| packet.payload.len())
                            .sum(),
                        backlog,
                    );
                }
                outcome
            }
            _ = air_timer.tick() => {
                let outcome = service.handle_air_timer(Instant::now()).await;
                if outcome == HrpdTrafficOutcome::default() {
                    continue;
                }
                outcome
            }
        };
        if outcome.unknown_session_count > 0 {
            warn!(
                "HRPD AN: traffic event for {} unknown HRPD session(s)",
                outcome.unknown_session_count
            );
        }
        let hardware_id_responses = outcome.hardware_id_responses;
        for uati in outcome.session_configuration_pending_uatis {
            if let Some(runtime) = traffic_a8_runtime.as_ref() {
                runtime.set_traffic_configuration_pending(uati, true);
            }
        }
        let session_configuration_complete_uatis = outcome.session_configuration_complete_uatis;
        let mut session_configuration_complete_events =
            outcome.session_configuration_complete_events;
        if session_configuration_complete_events.is_empty() {
            session_configuration_complete_events = session_configuration_complete_uatis
                .into_iter()
                .map(|uati| HrpdSessionConfigurationCompleteEvent {
                    uati,
                    physical_layer_subtype: 0,
                    forward_traffic_mac_subtype: 0,
                    idle_preferred_control_channel_cycle: None,
                    idle_page_period_cycles: HRPD_ENHANCED_IDLE_DEFAULT_PAGE_PERIOD_CYCLES,
                })
                .collect();
        }
        let mut released_traffic_uatis: HashSet<u32> = HashSet::new();
        for uati in outcome.session_closed_uatis {
            released_traffic_uatis.insert(uati);
            traffic_open_uatis.remove(&uati);
            forget_default_packet_flow_open(&mut default_packet_flow_open_uatis, uati);
            traffic_pending_a9_sessions.lock().await.remove(&uati);
            release_hrpd_a9_and_a10_for_uati(
                uati,
                "traffic SessionClose",
                &mut active_a9_sessions,
                a9_config,
                a9_endpoint.as_ref(),
                &mut a9_sequence_no,
            )
            .await;
            if let Some(runtime) = traffic_a8_runtime.as_ref() {
                runtime.release_session(uati);
            }
        }
        for uati in outcome.traffic_channel_closed_uatis {
            let first_close_for_uati = released_traffic_uatis.insert(uati);
            traffic_open_uatis.remove(&uati);
            forget_default_packet_flow_open(&mut default_packet_flow_open_uatis, uati);
            traffic_pending_a9_sessions.lock().await.remove(&uati);
            if first_close_for_uati && let Some(runtime) = traffic_a8_runtime.as_ref() {
                runtime.set_traffic_channel_open(uati, false);
            }
        }
        for release in outcome.traffic_releases {
            let first_close_for_uati = released_traffic_uatis.insert(release.uati);
            traffic_open_uatis.remove(&release.uati);
            forget_default_packet_flow_open(&mut default_packet_flow_open_uatis, release.uati);
            traffic_pending_a9_sessions
                .lock()
                .await
                .remove(&release.uati);
            if first_close_for_uati && let Some(runtime) = traffic_a8_runtime.as_ref() {
                runtime.set_traffic_channel_open(release.uati, false);
            }
            if traffic_release_tx_for_traffic.send(release).is_err() {
                warn!("HRPD AN: BTS traffic-release queue closed");
                return;
            }
        }
        for uati in outcome.default_packet_flow_open_uatis {
            remember_default_packet_flow_open(&mut default_packet_flow_open_uatis, uati);
            if let Some(runtime) = traffic_a8_runtime.as_ref() {
                runtime.set_default_packet_flow_open(uati, true);
            }
        }
        for uati in outcome.traffic_channel_open_uatis {
            traffic_open_uatis.insert(uati);
            if let Some(runtime) = traffic_a8_runtime.as_ref() {
                runtime.set_traffic_channel_open(uati, true);
            }
            if a9_config.is_some() {
                info!(
                    "HRPD AN: traffic channel open UATI=0x{uati:08x}; A9 SetupA8 waits for identity + SessionConfigurationComplete"
                );
            }
            let pending = {
                let mut sessions = traffic_pending_a9_sessions.lock().await;
                sessions.remove(&uati)
            };
            let Some(mut pending) = pending else {
                continue;
            };
            if pending_hrpd_a9_identity_needs_imsi(&pending) {
                let identity = {
                    let identities = traffic_session_a9_identities.lock().await;
                    cached_hrpd_a9_identity(&identities, pending.session_uati, pending.request.uati)
                };
                if let Some(identity) = identity {
                    pending.identity = Some(identity);
                }
            }
            if !pending.session_configuration_complete {
                let complete = {
                    let complete = traffic_session_a9_config_complete.lock().await;
                    complete.contains(&pending.session_uati)
                        || complete.contains(&pending.request.uati)
                };
                pending.session_configuration_complete = complete;
            }
            match try_complete_hrpd_a9_setup(
                pending,
                a9_config,
                a9_endpoint.as_ref(),
                traffic_a8_runtime.as_ref(),
                &traffic_open_uatis,
                &default_packet_flow_open_uatis,
                &mut a9_sequence_no,
                "TrafficChannelOpen + cached identity/config",
            )
            .await
            {
                Ok(context) => {
                    active_a9_sessions.insert(context.a8_key(), context);
                }
                Err(pending) => {
                    let mut sessions = traffic_pending_a9_sessions.lock().await;
                    sessions.insert(uati, pending);
                }
            }
        }
        for uati in outcome.default_packet_flow_closed_uatis {
            forget_default_packet_flow_open(&mut default_packet_flow_open_uatis, uati);
            if let Some(runtime) = traffic_a8_runtime.as_ref() {
                runtime.set_default_packet_flow_open(uati, false);
            }
        }
        for config in outcome.default_packet_stream_configurations {
            if let Some(runtime) = traffic_a8_runtime.as_ref() {
                runtime.set_default_packet_stream_configuration(
                    config.uati,
                    config.stream_id,
                    config.protocol_type,
                );
            }
        }
        for ack in outcome.default_packet_data_ready_acks {
            if let Some(runtime) = traffic_a8_runtime.as_ref() {
                runtime.default_packet_data_ready_ack(ack.uati, ack.transaction_id);
            }
        }
        for uati in outcome.default_packet_rlp_reset_uatis {
            if let Some(runtime) = traffic_a8_runtime.as_ref() {
                runtime.reset_default_packet_rlp(uati);
            }
        }
        for nak in outcome.default_packet_rlp_naks {
            if let Some(runtime) = traffic_a8_runtime.as_ref() {
                runtime.retransmit_default_packet_rlp(nak.uati, nak.requests);
            }
        }
        let count = outcome.a8_uplink.len();
        for packet in outcome.a8_uplink {
            if let Some(runtime) = traffic_a8_runtime.as_ref() {
                runtime.send_uplink(packet.uati, packet.payload);
            } else {
                warn!(
                    "HRPD AN: dropping Stream 1 uplink UATI=0x{:08x}; A8 runtime unavailable",
                    packet.uati
                );
            }
        }
        if count > 0 {
            log::debug!("HRPD AN: queued {count} A8 uplink packet(s)");
        }
        let count = outcome.forward_signaling.len();
        for request in outcome.forward_signaling {
            if traffic_forward_tx.send(request).is_err() {
                warn!("HRPD AN: forward signaling channel closed while handling traffic event");
            }
        }
        if count > 0 {
            info!("HRPD AN: queued {count} forward signaling packet(s) from traffic event");
        }
        let count = outcome.forward_traffic.len();
        for packet in outcome.forward_traffic {
            if released_traffic_uatis.contains(&packet.uati) {
                info!(
                    "HRPD AN: dropping forward traffic packet for released UATI=0x{:08x} mac_index={}",
                    packet.uati, packet.mac_index
                );
                continue;
            }
            if traffic_forward_traffic_tx
                .send(forward_traffic_from_air_packet(packet))
                .is_err()
            {
                warn!("HRPD AN: forward traffic channel closed while handling traffic event");
            }
        }
        if count > 0 {
            log::debug!("HRPD AN: queued {count} forward traffic packet(s) from traffic event");
        }
        for hardware in hardware_id_responses {
            let Some(hardware_identity) = hardware_identity_from_response(&hardware.response)
            else {
                warn!(
                    "HRPD AN: unsupported/null traffic HardwareIDResponse UATI=0x{:08x} type=0x{:06x} len={}",
                    hardware.uati,
                    hardware.response.hardware_id_type,
                    hardware.response.hardware_id_value.len()
                );
                continue;
            };
            let pending = {
                let mut sessions = traffic_pending_a9_sessions.lock().await;
                sessions.remove(&hardware.uati)
            };
            let Some(mut pending) = pending else {
                info!(
                    "HRPD AN: observed HardwareIDResponse UATI=0x{:08x} with no pending A9 setup hardware={hardware_identity:?}",
                    hardware.uati
                );
                continue;
            };
            let identity = resolve_hrpd_a9_identity(
                traffic_hlr_repo.as_ref(),
                &traffic_derived_imsi_config,
                &hardware_identity,
            )
            .await;
            info!(
                "HRPD AN: A9 identity for UATI=0x{:08x}: imsi_present={} esn={:?} meid_present={}",
                hardware.uati,
                identity.imsi.is_some(),
                identity.esn,
                identity.meid.is_some()
            );
            {
                let mut identities = traffic_session_a9_identities.lock().await;
                identities.insert(pending.session_uati, identity.clone());
                identities.insert(pending.request.uati, identity.clone());
            }
            pending.identity = Some(identity);
            if pending.session_configuration_complete {
                match try_complete_hrpd_a9_setup(
                    pending,
                    a9_config,
                    a9_endpoint.as_ref(),
                    traffic_a8_runtime.as_ref(),
                    &traffic_open_uatis,
                    &default_packet_flow_open_uatis,
                    &mut a9_sequence_no,
                    "HardwareIDResponse + SessionConfigurationComplete",
                )
                .await
                {
                    Ok(context) => {
                        active_a9_sessions.insert(context.a8_key(), context);
                    }
                    Err(pending) => {
                        let mut sessions = traffic_pending_a9_sessions.lock().await;
                        sessions.insert(hardware.uati, pending);
                    }
                }
            } else {
                info!(
                    "HRPD AN: deferring A9 SetupA8 UATI=0x{:08x}; waiting for SessionConfigurationComplete",
                    hardware.uati
                );
                let mut sessions = traffic_pending_a9_sessions.lock().await;
                sessions.insert(hardware.uati, pending);
            }
        }
        for event in session_configuration_complete_events {
            let uati = event.uati;
            let session_uati = session_uati_from_hrpd_traffic_uati(uati);
            let idle_page_period_cycles = if event.idle_page_period_cycles > 0 {
                event.idle_page_period_cycles
            } else {
                warn!(
                    "HRPD AN: SessionConfigurationComplete UATI=0x{uati:08x} carries a zero idle page period; using default {HRPD_ENHANCED_IDLE_DEFAULT_PAGE_PERIOD_CYCLES}"
                );
                HRPD_ENHANCED_IDLE_DEFAULT_PAGE_PERIOD_CYCLES
            };
            if let Some(runtime) = traffic_a8_runtime.as_ref() {
                runtime.set_session_configuration_complete(
                    uati,
                    true,
                    event.physical_layer_subtype,
                    event.forward_traffic_mac_subtype,
                    event.idle_preferred_control_channel_cycle,
                    idle_page_period_cycles,
                );
            }
            let pending = {
                let mut sessions = traffic_pending_a9_sessions.lock().await;
                sessions.remove(&uati).map(|mut pending| {
                    pending.session_configuration_complete = true;
                    pending
                })
            };
            {
                let mut complete = traffic_session_a9_config_complete.lock().await;
                complete.insert(session_uati);
                complete.insert(uati);
            }
            let Some(mut pending) = pending else {
                info!(
                    "HRPD AN: observed SessionConfigurationComplete UATI=0x{uati:08x} session_uati=0x{session_uati:08x} with no pending A9 setup"
                );
                continue;
            };
            if pending_hrpd_a9_identity_needs_imsi(&pending) {
                let identity = {
                    let identities = traffic_session_a9_identities.lock().await;
                    cached_hrpd_a9_identity(&identities, pending.session_uati, pending.request.uati)
                };
                if let Some(identity) = identity {
                    pending.identity = Some(identity);
                }
            }
            match try_complete_hrpd_a9_setup(
                pending,
                a9_config,
                a9_endpoint.as_ref(),
                traffic_a8_runtime.as_ref(),
                &traffic_open_uatis,
                &default_packet_flow_open_uatis,
                &mut a9_sequence_no,
                "SessionConfigurationComplete + identity",
            )
            .await
            {
                Ok(context) => {
                    active_a9_sessions.insert(context.a8_key(), context);
                }
                Err(pending) => {
                    let mut sessions = traffic_pending_a9_sessions.lock().await;
                    sessions.insert(uati, pending);
                }
            }
        }
    }
    info!("HRPD AN stopped: BTS traffic event channel closed");
}

/// Drive the AN-side access-indication loop: run each access capsule through
/// the AN and act on the outcome, until the BTS access event stream closes.
#[allow(clippy::too_many_arguments)]
async fn hrpd_an_access_task(
    service: AnServiceImpl,
    mut access_rx: tokio::sync::mpsc::UnboundedReceiver<hrpd_air::HrpdAccessIndication>,
    forward_tx: tokio::sync::mpsc::UnboundedSender<hrpd_air::HrpdForwardSignalingRequest>,
    traffic_assignment_tx: tokio::sync::mpsc::UnboundedSender<
        hrpd_air::HrpdTrafficAssignmentRequest,
    >,
    traffic_release_tx: tokio::sync::mpsc::UnboundedSender<hrpd_air::HrpdTrafficReleaseRequest>,
    forward_traffic_tx: ForwardTrafficSender,
    a9_config: Option<HrpdA9ClientConfig>,
    hlr_repo: Option<Arc<dyn cdma_hlr::repository::HlrRepository>>,
    color_code: u8,
    derived_imsi_config: HrpdDerivedImsiConfig,
    an_a8_runtime: Option<HrpdAnA8Runtime>,
    pending_a9_sessions: Arc<Mutex<HashMap<u32, PendingHrpdA9Session>>>,
    session_a9_identities: Arc<Mutex<HashMap<u32, HrpdA9MobileIdentity>>>,
    session_a9_config_complete: Arc<Mutex<HashSet<u32>>>,
    a9_release_tx: tokio::sync::mpsc::UnboundedSender<HrpdA9ReleaseRequest>,
) {
    let mut hardware_by_uati: HashMap<u32, HrpdHardwareIdentity> = HashMap::new();
    let access_session_a9_identities = session_a9_identities.clone();
    let access_session_a9_config_complete = session_a9_config_complete.clone();
    let access_a9_release_tx = a9_release_tx.clone();
    while let Some(indication) = access_rx.recv().await {
        let access_uati = uati_from_access_ati(indication.ati, color_code);
        let access_uati_complete_uati = access_uati.filter(|_| {
            indication
                .messages
                .iter()
                .any(|message| matches!(message, hrpd_air::HrpdAccessMessage::UatiComplete(_)))
        });
        let mut access_data_ready_acks = Vec::new();
        if let Some(uati) = access_uati {
            let data_ready_acks = access_default_packet_data_ready_acks(&indication);
            if !data_ready_acks.is_empty() {
                info!(
                    "HRPD AN: decoded access DefaultPacket DataReadyAck UATI=0x{uati:08x} transactions={}",
                    data_ready_acks
                        .iter()
                        .map(|transaction| format!("0x{transaction:02x}"))
                        .collect::<Vec<_>>()
                        .join(",")
                );
                access_data_ready_acks = data_ready_acks
                    .into_iter()
                    .map(|transaction_id| (uati, transaction_id))
                    .collect();
            }
            for open in access_default_packet_flow_requests(&indication) {
                let traffic_uati = hrpd_traffic_uati_from_session_uati(uati, color_code);
                info!(
                    "HRPD AN: decoded access DefaultPacket {} UATI=0x{uati:08x} traffic_uati=0x{traffic_uati:08x}",
                    if open { "XonRequest" } else { "XoffRequest" }
                );
                if let Some(runtime) = an_a8_runtime.as_ref() {
                    runtime.set_default_packet_flow_open(traffic_uati, open);
                }
            }
            for message in &indication.messages {
                if let hrpd_air::HrpdAccessMessage::HardwareIdResponse(response) = message {
                    if let Some(hardware) = hardware_identity_from_response(response) {
                        info!(
                            "HRPD AN: observed HardwareIDResponse UATI=0x{uati:08x} hardware={hardware:?}"
                        );
                        hardware_by_uati.insert(uati, hardware.clone());
                        let identity = resolve_hrpd_a9_identity(
                            hlr_repo.as_ref(),
                            &derived_imsi_config,
                            &hardware,
                        )
                        .await;
                        info!(
                            "HRPD AN: A9 identity from access HardwareIDResponse UATI=0x{uati:08x}: imsi_present={} esn={:?} meid_present={}",
                            identity.imsi.is_some(),
                            identity.esn,
                            identity.meid.is_some()
                        );
                        {
                            let mut identities = access_session_a9_identities.lock().await;
                            identities.insert(uati, identity.clone());
                        }
                        let pending_session_uati = {
                            let mut sessions = pending_a9_sessions.lock().await;
                            sessions.get_mut(&uati).map(|pending| {
                                    if pending.identity.is_none() {
                                        info!(
                                            "HRPD AN: applying access HardwareIDResponse identity to pending A9 setup UATI=0x{uati:08x}"
                                        );
                                    }
                                    pending.identity = Some(identity.clone());
                                    pending.session_uati
                                })
                        };
                        if let Some(session_uati) = pending_session_uati {
                            let mut identities = access_session_a9_identities.lock().await;
                            identities.insert(session_uati, identity);
                        }
                    } else {
                        warn!(
                            "HRPD AN: unsupported/null HardwareIDResponse UATI=0x{uati:08x} type=0x{:06x} len={}",
                            response.hardware_id_type,
                            response.hardware_id_value.len()
                        );
                    }
                }
            }
        }
        let outcome = match service.handle_access(&indication).await {
            Ok(outcome) => outcome,
            Err(err) => {
                warn!("HRPD AN: access indication rejected: {err}");
                continue;
            }
        };
        if !outcome.connection_requests.is_empty()
            || !outcome.traffic_assignments.is_empty()
            || !outcome.traffic_releases.is_empty()
            || !outcome.forward_traffic.is_empty()
        {
            info!(
                "HRPD AN: access outcome connection_requests={} traffic_assignments={} traffic_releases={} forward_signaling={} forward_traffic={}",
                outcome.connection_requests.len(),
                outcome.traffic_assignments.len(),
                outcome.traffic_releases.len(),
                outcome.forward_signaling.len(),
                outcome.forward_traffic.len()
            );
        }
        let traffic_assignment_uatis = outcome
            .traffic_assignments
            .iter()
            .map(|request| request.uati)
            .collect::<HashSet<_>>();
        let address_assignment_uatis = outcome
            .forward_signaling
            .iter()
            .filter(|request| {
                request.protocol_type == HRPD_ADDRESS_MANAGEMENT_PROTOCOL_TYPE
                    && request.payload.first().copied() == Some(HRPD_UATI_ASSIGNMENT_MESSAGE_ID)
            })
            .filter_map(|request| request.uati)
            .collect::<Vec<_>>();
        if let Some(old_uati) = access_uati
            && !address_assignment_uatis.is_empty()
            && let Some(runtime) = an_a8_runtime.as_ref()
        {
            let assignments = address_assignment_uatis
                .iter()
                .map(|uati| format!("0x{uati:08x}"))
                .collect::<Vec<_>>()
                .join(",");
            let changes_receive_uati = address_assignment_uatis
                .iter()
                .any(|assigned| (assigned & 0x00ff_ffff) != (old_uati & 0x00ff_ffff));
            if changes_receive_uati {
                info!(
                    "HRPD AN: address management pending old_uati=0x{old_uati:08x} assignments=[{assignments}]; quiescing old-UATI packet-data paging"
                );
            } else {
                info!(
                    "HRPD AN: address management reaffirmed UATI=0x{old_uati:08x} assignments=[{assignments}]; holding packet-data paging until UATIComplete"
                );
            }
            runtime.set_address_management_pending(old_uati, true);
        }
        let accepted_uati_complete = !outcome.uati_completes.is_empty();
        if let Some(uati) = access_uati_complete_uati
            && let Some(runtime) = an_a8_runtime.as_ref()
        {
            if !accepted_uati_complete {
                info!(
                    "HRPD AN: observed UATIComplete UATI=0x{uati:08x} but AN did not accept it as current; not retargeting A8 downlink"
                );
            } else if traffic_assignment_uatis.contains(&uati) {
                info!(
                    "HRPD AN: UATIComplete confirmed active UATI=0x{uati:08x} with traffic assignment pending; deferring stale A8 downlink retarget until traffic setup is marked pending"
                );
                runtime.set_traffic_setup_pending(uati, true);
                runtime.set_address_management_pending(uati, false);
            } else {
                info!(
                    "HRPD AN: UATIComplete confirmed active UATI=0x{uati:08x}; retargeting stale A8 downlink"
                );
                runtime.set_address_management_pending(uati, false);
                runtime.retarget_stale_downlink_to_active_uati(uati);
            }
        }
        if accepted_uati_complete
            && access_uati_complete_uati.is_none()
            && let Some(uati) = access_uati
            && let Some(runtime) = an_a8_runtime.as_ref()
        {
            info!(
                "HRPD AN: synthesized UATIComplete accepted active UATI=0x{uati:08x}; clearing A8 address-management hold"
            );
            if traffic_assignment_uatis.contains(&uati) {
                runtime.set_traffic_setup_pending(uati, true);
            }
            runtime.set_address_management_pending(uati, false);
        }
        if let Some(runtime) = an_a8_runtime.as_ref() {
            for (uati, transaction_id) in &access_data_ready_acks {
                runtime.default_packet_data_ready_ack(*uati, *transaction_id);
            }
        }
        for (uati, _) in access_data_ready_acks {
            if traffic_assignment_uatis.contains(&uati) {
                info!(
                    "HRPD AN: access DataReadyAck confirms reachable UATI=0x{uati:08x} with traffic assignment pending; deferring stale A8 downlink retarget until traffic setup is marked pending"
                );
            } else if let Some(runtime) = an_a8_runtime.as_ref() {
                info!(
                    "HRPD AN: access DataReadyAck confirms reachable UATI=0x{uati:08x}; retargeting stale A8 downlink"
                );
                runtime.retarget_stale_downlink_to_active_uati(uati);
            }
        }
        let count = outcome.forward_signaling.len();
        for request in outcome.forward_signaling {
            if forward_tx.send(request).is_err() {
                warn!("HRPD AN: BTS forward-signaling queue closed");
                return;
            }
        }
        if count > 0 {
            info!("HRPD AN: queued {count} forward signaling message(s)");
        }
        let count = outcome.forward_traffic.len();
        for packet in outcome.forward_traffic {
            if forward_traffic_tx
                .send(forward_traffic_from_air_packet(packet))
                .is_err()
            {
                warn!("HRPD AN: BTS forward-traffic queue closed");
                return;
            }
        }
        if count > 0 {
            log::debug!("HRPD AN: queued {count} forward traffic packet(s)");
        }
        let count = outcome.traffic_releases.len();
        for release in outcome.traffic_releases {
            if let Some(runtime) = an_a8_runtime.as_ref() {
                runtime.set_traffic_channel_open(release.uati, false);
            }
            let uati = release.uati;
            if traffic_release_tx.send(release).is_err() {
                warn!("HRPD AN: BTS traffic-release queue closed");
                return;
            }
            pending_a9_sessions.lock().await.remove(&uati);
        }
        if count > 0 {
            info!("HRPD AN: queued {count} traffic release(s)");
        }
        for uati in outcome.session_closed_uatis {
            let reason = "access SessionClose";
            if access_a9_release_tx
                .send(HrpdA9ReleaseRequest {
                    uati,
                    reason: reason.to_string(),
                })
                .is_err()
            {
                warn!(
                    "HRPD AN: A9 release owner stopped before access SessionClose UATI=0x{uati:08x}; releasing local AN A8 state only"
                );
                if let Some(runtime) = an_a8_runtime.as_ref() {
                    runtime.release_session(uati);
                }
            }
            pending_a9_sessions.lock().await.remove(&uati);
        }
        let count = outcome.traffic_assignments.len();
        for request in outcome.traffic_assignments {
            let session_uati = request.session_uati;
            let identity = {
                let identities = access_session_a9_identities.lock().await;
                cached_hrpd_a9_identity(&identities, session_uati, request.uati)
            };
            let session_configuration_complete = access_session_a9_config_complete
                .lock()
                .await
                .contains(&session_uati);
            if let Some(runtime) = an_a8_runtime.as_ref() {
                runtime.set_traffic_mac_index(request.uati, request.mac_index);
                runtime.set_traffic_setup_pending(request.uati, true);
                runtime.retarget_stale_downlink_to_active_uati(request.uati);
            }
            if a9_config.is_some() {
                let identity_cached = identity.is_some();
                pending_a9_sessions.lock().await.insert(
                    request.uati,
                    PendingHrpdA9Session {
                        session_uati,
                        request: request.clone(),
                        identity,
                        session_configuration_complete,
                    },
                );
                info!(
                    "HRPD AN: deferred A9 SetupA8 UATI=0x{:08x} session_uati=0x{session_uati:08x} MAC={} identity_cached={} session_config_complete={session_configuration_complete}",
                    request.uati, request.mac_index, identity_cached
                );
            }
            if traffic_assignment_tx.send(request).is_err() {
                warn!("HRPD AN: BTS traffic-assignment queue closed");
                return;
            }
        }
        if count > 0 {
            info!("HRPD AN: queued {count} traffic assignment(s)");
        }
    }
    info!("HRPD AN stopped: BTS access event channel closed");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn access_default_packet_flow_requests_preserve_access_capsule_order() {
        let indication = hrpd_air::HrpdAccessIndication {
            absolute_chip: 0,
            color_code: 0,
            sector_pilot_pn: 0,
            session_configuration_token: 0,
            ati: hrpd_air::AccessTerminalIdentifier {
                ati_type: hrpd_air::AccessTerminalIdentifierType::Uati,
                value: 0x001a_7e1c,
            },
            security_layer_format: false,
            connection_layer_format: false,
            security_payload: Vec::new(),
            messages: vec![
                hrpd_air::HrpdAccessMessage::DefaultPacketXoffRequest,
                hrpd_air::HrpdAccessMessage::DefaultPacketDataReadyAck(
                    hrpd_air::HrpdDefaultPacketDataReadyAck {
                        transaction_id: 0x01,
                    },
                ),
                hrpd_air::HrpdAccessMessage::DefaultPacketXonRequest,
            ],
        };

        assert_eq!(
            access_default_packet_flow_requests(&indication),
            vec![false, true]
        );
        assert_eq!(
            access_default_packet_data_ready_acks(&indication),
            vec![0x01]
        );
    }

    fn test_hrpd_a9_config() -> HrpdA9ClientConfig {
        let pcf_a8 = cdma_a8::BearerTransportConfig::udp_encapsulated_gre(
            "127.0.0.1:17042".parse().unwrap(),
            "127.0.0.1:17041".parse().unwrap(),
        );
        HrpdA9ClientConfig {
            pcf_addr: "127.0.0.1:17046".parse().unwrap(),
            a8_peer_ipv4: [127, 0, 0, 1],
            an_a8_bearer: cdma_pcf::inverted_udp_gre_bearer(pcf_a8, "test.an_a8").unwrap(),
            an_a8_endpoint: cdma_a8::BearerEndpoint::new([127, 0, 0, 1], [127, 0, 0, 1]),
        }
    }

    fn free_udp_addr() -> SocketAddr {
        std::net::UdpSocket::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
    }

    fn test_a11_security_config() -> cdma_a11::A11SecurityConfig {
        // Test fixture only. Live PCF/PDSN configs must carry a11_security explicitly.
        cdma_a11::A11SecurityConfig {
            spi: 256,
            shared_secret_hex: "31786274732d6131312d7368617265642d736563726574".to_string(),
        }
    }

    fn test_pcf_node_config() -> cdma_pcf::PcfNodeConfig {
        cdma_pcf::PcfNodeConfig {
            packet_grpc_endpoint: "http://127.0.0.1:17021".to_string(),
            a9_bind_addr: "127.0.0.1:17046".parse().unwrap(),
            a8_bearer: cdma_a8::BearerTransportConfig::udp_encapsulated_gre(
                "127.0.0.1:17041".parse().unwrap(),
                "127.0.0.1:17040".parse().unwrap(),
            ),
            a10_bearer: cdma_a10::BearerTransportConfig::udp_encapsulated_gre(
                "127.0.0.1:17042".parse().unwrap(),
                "127.0.0.1:17043".parse().unwrap(),
            ),
            a11: cdma_a11::A11TransportConfig::new(
                "127.0.0.1:17044".parse().unwrap(),
                "127.0.0.1:17045".parse().unwrap(),
            ),
            a11_security: test_a11_security_config(),
        }
    }

    fn test_a11_auth(
        extension_type: cdma_a11::AuthenticationExtensionType,
    ) -> cdma_a11::AuthenticationExtension {
        cdma_a11::AuthenticationExtension {
            extension_type,
            security_parameter_index: 1,
            authenticator: vec![0; 16],
        }
    }

    fn test_a11_registration_reply(request: &cdma_a11::RegistrationRequest) -> cdma_a11::Message {
        let mut message = cdma_a11::Message::RegistrationReply(cdma_a11::RegistrationReply {
            code: 0,
            lifetime: request.lifetime,
            home_address: request.home_address,
            home_agent: request.home_agent,
            identification: request.identification,
            session: request.session.clone(),
            extensions: vec![cdma_a11::Extension::Authentication(test_a11_auth(
                cdma_a11::AuthenticationExtensionType::MobileHome,
            ))],
        });
        // The PCF verifies the reply (recv_message_verified), so sign it with
        // the same security association the PCF task is configured with.
        cdma_a11::A11SecurityAssociation::from_config(&test_a11_security_config())
            .expect("test A11 security config")
            .sign_message(&mut message)
            .expect("sign test A11 registration reply");
        message
    }

    fn test_a11_registration_update(request: &cdma_a11::RegistrationRequest) -> cdma_a11::Message {
        let mut message = cdma_a11::Message::RegistrationUpdate(cdma_a11::RegistrationUpdate {
            reserved: [0; 3],
            home_address: [0; 4],
            home_agent: request.home_agent,
            identification: request.identification,
            session: request.session.clone(),
            nvses: Vec::new(),
            authentication_extension: test_a11_auth(
                cdma_a11::AuthenticationExtensionType::RegistrationUpdate,
            ),
        });
        // The PCF verifies inbound A11, so sign with the configured association.
        cdma_a11::A11SecurityAssociation::from_config(&test_a11_security_config())
            .expect("test A11 security config")
            .sign_message(&mut message)
            .expect("sign test A11 registration update");
        message
    }

    #[test]
    fn hrpd_uati_subnet_assignment_uses_sector_uati104() {
        let assignment = hrpd_uati_subnet_assignment(
            [
                0x00, 0x80, 0x05, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00,
            ],
            26,
        );

        assert_eq!(assignment.uati_subnet_mask, 26);
        assert_eq!(
            assignment.uati104,
            [
                0x00, 0x80, 0x05, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00
            ]
        );
    }

    #[test]
    fn hrpd_setup_a8_uses_assignment_as_a8_key_and_con_ref() {
        let config = test_hrpd_a9_config();
        let assignment = hrpd_air::HrpdTrafficAssignmentRequest {
            session_uati: 0x0080_0580,
            uati: 0x0080_0580,
            mac_index: 7,
            reverse_rate_limit_bps: 153_600,
            reverse_long_code_mask_i: 0,
            reverse_long_code_mask_q: 0,
            drc_lock: true,
            physical_layer_subtype: 0,
            reverse_traffic_mac_subtype: 0,
            frame_offset: 0,
            drc_cover: 0,
            drc_length: 1,
        };

        let setup = cdma_an::build_setup_a8(config, &assignment, None);

        assert_eq!(setup.con_ref.0, assignment.mac_index);
        assert_eq!(setup.a8_traffic_id.key, assignment.uati);
        assert_eq!(
            setup.a8_traffic_id.ip_address,
            cdma_a9::A8IpAddress::V4([127, 0, 0, 1])
        );
        assert_eq!(
            setup.service_option,
            cdma_a9::ServiceOptionValue::HIGH_RATE_PACKET_DATA
        );
        assert!(setup.a9_indicators.data_ready);
        assert!(!setup.a9_indicators.packet_boundary_supported);
        assert!(setup.imsi.is_none());
        assert!(setup.esn.is_none());
        assert!(setup.meid.is_none());
    }

    #[test]
    fn hrpd_setup_a8_carries_resolved_mobile_identity() {
        let config = test_hrpd_a9_config();
        let assignment = hrpd_air::HrpdTrafficAssignmentRequest {
            session_uati: 0x8005_8001,
            uati: 0x8005_8001,
            mac_index: 7,
            reverse_rate_limit_bps: 153_600,
            reverse_long_code_mask_i: 0,
            reverse_long_code_mask_q: 0,
            drc_lock: true,
            physical_layer_subtype: 0,
            reverse_traffic_mac_subtype: 0,
            frame_offset: 0,
            drc_cover: 0,
            drc_length: 1,
        };
        let identity = HrpdA9MobileIdentity {
            imsi: Some("310009176936269".to_string()),
            esn: Some(0x4cdc_1d09),
            meid: None,
        };

        let setup = cdma_an::build_setup_a8(config, &assignment, Some(&identity));

        assert_eq!(setup.imsi.as_deref(), Some("310009176936269"));
        assert_eq!(setup.esn, Some(0x4cdc_1d09));
        assert!(setup.meid.is_none());
    }

    #[test]
    fn hrpd_setup_a8_omits_hardware_only_identity() {
        let config = test_hrpd_a9_config();
        let assignment = hrpd_air::HrpdTrafficAssignmentRequest {
            session_uati: 0x8005_8001,
            uati: 0x8005_8001,
            mac_index: 7,
            reverse_rate_limit_bps: 153_600,
            reverse_long_code_mask_i: 0,
            reverse_long_code_mask_q: 0,
            drc_lock: true,
            physical_layer_subtype: 0,
            reverse_traffic_mac_subtype: 0,
            frame_offset: 0,
            drc_cover: 0,
            drc_length: 1,
        };
        let identity = HrpdA9MobileIdentity {
            imsi: None,
            esn: None,
            meid: Some(cdma_a9::Meid([0x35, 0x51, 0x26, 0x06, 0x02, 0x34, 0x34])),
        };

        let setup = cdma_an::build_setup_a8(config, &assignment, Some(&identity));

        assert!(setup.imsi.is_none());
        assert!(setup.esn.is_none());
        assert!(setup.meid.is_none());
        setup
            .encode()
            .expect("hardware-only identity must not make SetupA8 invalid");
    }

    #[test]
    fn derives_stable_hrpd_imsi_from_hardware_identity() {
        let config = HrpdDerivedImsiConfig {
            mcc: "310".to_string(),
            imsi_11_12: "55".to_string(),
        };

        assert_eq!(
            cdma_an::derive_hrpd_imsi(&config, &HrpdHardwareIdentity::Esn(0x4cdc_1d09)),
            "310559151749291"
        );
        assert_eq!(
            cdma_an::derive_hrpd_imsi(
                &config,
                &HrpdHardwareIdentity::Meid(cdma_a9::Meid([
                    0x35, 0x51, 0x26, 0x06, 0x02, 0x34, 0x34
                ]))
            ),
            "310556898017332"
        );
    }

    #[test]
    fn hrpd_setup_a8_accepts_derived_imsi_identity() {
        let config = test_hrpd_a9_config();
        let assignment = hrpd_air::HrpdTrafficAssignmentRequest {
            session_uati: 0x8005_8001,
            uati: 0x8005_8001,
            mac_index: 7,
            reverse_rate_limit_bps: 153_600,
            reverse_long_code_mask_i: 0,
            reverse_long_code_mask_q: 0,
            drc_lock: true,
            physical_layer_subtype: 0,
            reverse_traffic_mac_subtype: 0,
            frame_offset: 0,
            drc_cover: 0,
            drc_length: 1,
        };
        let derived_config = HrpdDerivedImsiConfig {
            mcc: "310".to_string(),
            imsi_11_12: "55".to_string(),
        };
        let identity = HrpdA9MobileIdentity {
            imsi: Some(cdma_an::derive_hrpd_imsi(
                &derived_config,
                &HrpdHardwareIdentity::Esn(0x4cdc_1d09),
            )),
            esn: Some(0x4cdc_1d09),
            meid: None,
        };

        let setup = cdma_an::build_setup_a8(config, &assignment, Some(&identity));

        assert_eq!(setup.imsi.as_deref(), Some("310559151749291"));
        assert_eq!(setup.esn, Some(0x4cdc_1d09));
        setup.encode().expect("derived IMSI must encode");
    }

    #[test]
    fn cached_hrpd_a9_identity_recovers_session_uati_identity_for_traffic_setup() {
        let session_uati = 0x006c_4362;
        let traffic_uati = 0x1a6c_4362;
        let identity = HrpdA9MobileIdentity {
            imsi: Some("310556898017332".to_string()),
            esn: None,
            meid: Some(cdma_a9::Meid([0x35, 0x51, 0x26, 0x06, 0x02, 0x34, 0x34])),
        };
        let mut identities = HashMap::new();
        identities.insert(session_uati, identity.clone());
        let mut pending = PendingHrpdA9Session {
            session_uati,
            request: hrpd_air::HrpdTrafficAssignmentRequest {
                session_uati,
                uati: traffic_uati,
                mac_index: 6,
                reverse_rate_limit_bps: 153_600,
                reverse_long_code_mask_i: 0,
                reverse_long_code_mask_q: 0,
                drc_lock: true,
                physical_layer_subtype: 2,
                reverse_traffic_mac_subtype: 3,
                frame_offset: 0,
                drc_cover: 0,
                drc_length: 1,
            },
            identity: None,
            session_configuration_complete: true,
        };

        assert!(pending_hrpd_a9_identity_needs_imsi(&pending));
        pending.identity =
            cached_hrpd_a9_identity(&identities, pending.session_uati, pending.request.uati);

        assert_eq!(pending.identity, Some(identity));
        assert!(!pending_hrpd_a9_identity_needs_imsi(&pending));
    }

    #[test]
    fn builds_spec_shaped_hrpd_a11_registration_request() {
        let config = test_hrpd_a9_config();
        let assignment = hrpd_air::HrpdTrafficAssignmentRequest {
            session_uati: 0x8005_8001,
            uati: 0x8005_8001,
            mac_index: 7,
            reverse_rate_limit_bps: 153_600,
            reverse_long_code_mask_i: 0,
            reverse_long_code_mask_q: 0,
            drc_lock: true,
            physical_layer_subtype: 0,
            reverse_traffic_mac_subtype: 0,
            frame_offset: 0,
            drc_cover: 0,
            drc_length: 1,
        };
        let identity = HrpdA9MobileIdentity {
            imsi: Some("310009176936269".to_string()),
            esn: Some(0x4cdc_1d09),
            meid: None,
        };
        let setup = cdma_an::build_setup_a8(config, &assignment, Some(&identity));
        let a11 = cdma_a11::A11TransportConfig::new(
            "127.0.0.1:17044".parse().unwrap(),
            "127.0.0.1:17045".parse().unwrap(),
        );
        let security =
            cdma_a11::A11SecurityAssociation::from_config(&test_a11_security_config()).unwrap();

        let message = cdma_pcf::build_hrpd_a11_registration_request(
            cdma_pcf::PcfSessionId(7),
            &setup,
            a11,
            &security,
        )
        .unwrap();
        let encoded = cdma_a11::encode(&message).unwrap();
        let decoded =
            cdma_a11::decode_unverified(&encoded, cdma_a11::UnverifiedDecodeReason::TestFixture)
                .unwrap();

        let cdma_a11::Message::RegistrationRequest(request) = decoded else {
            panic!("expected registration request");
        };
        assert_eq!(request.flags, 0x0a);
        assert_eq!(request.lifetime, 600);
        assert_eq!(request.home_address, [0, 0, 0, 0]);
        assert_eq!(request.home_agent, [127, 0, 0, 1]);
        assert_eq!(request.care_of_address, [127, 0, 0, 1]);
        assert_eq!(request.session.protocol_type, 0x8881);
        assert_eq!(request.session.pcf_session_id, 7);
        assert_eq!(request.session.session_id_version, 1);
        assert_eq!(request.session.mn_session_reference_id, 1);
        assert_eq!(request.session.mn_id_type, 0x0006);
        assert_eq!(
            request.session.mn_id,
            vec![0x31, 0x01, 0x00, 0x19, 0x67, 0x39, 0x26, 0x96]
        );
    }

    #[test]
    fn parses_hrpd_hardware_id_response_values() {
        let esn = hardware_identity_from_response(&hrpd_air::HrpdHardwareIdResponse {
            transaction_id: 1,
            hardware_id_type: 0x010000,
            hardware_id_value: vec![0x4c, 0xdc, 0x1d, 0x09],
        });
        assert_eq!(esn, Some(HrpdHardwareIdentity::Esn(0x4cdc_1d09)));

        let meid = hardware_identity_from_response(&hrpd_air::HrpdHardwareIdResponse {
            transaction_id: 2,
            hardware_id_type: 0x00ffff,
            hardware_id_value: vec![0xa1, 0xb2, 0xc3, 0xd4, 0xe5, 0xf6, 0x70],
        });
        assert_eq!(
            meid,
            Some(HrpdHardwareIdentity::Meid(cdma_a9::Meid([
                0xa1, 0xb2, 0xc3, 0xd4, 0xe5, 0xf6, 0x70
            ])))
        );
    }

    #[test]
    fn preserves_on_air_uati_from_access_ati() {
        let uati = uati_from_access_ati(
            hrpd_air::AccessTerminalIdentifier {
                ati_type: hrpd_air::AccessTerminalIdentifierType::Uati,
                value: 0x1a05_8001,
            },
            0x1a,
        );
        assert_eq!(uati, Some(0x0005_8001));
    }

    #[test]
    fn session_uati_from_hrpd_traffic_uati_clears_color_code() {
        assert_eq!(
            session_uati_from_hrpd_traffic_uati(0x1a6b_8f30),
            0x006b_8f30
        );
    }

    #[test]
    fn hrpd_traffic_uati_from_session_uati_applies_color_code() {
        assert_eq!(
            hrpd_traffic_uati_from_session_uati(0x006b_8f30, 0x1a),
            0x1a6b_8f30
        );
        assert_eq!(
            hrpd_traffic_uati_from_session_uati(0x116b_8f30, 0x1a),
            0x1a6b_8f30
        );
    }

    #[test]
    fn default_packet_flow_open_matches_session_or_traffic_uati() {
        let session_uati = 0x006b_8f30;
        let traffic_uati = 0x1a6b_8f30;
        let pending = PendingHrpdA9Session {
            session_uati,
            request: hrpd_air::HrpdTrafficAssignmentRequest {
                session_uati,
                uati: traffic_uati,
                mac_index: 6,
                reverse_rate_limit_bps: 153_600,
                reverse_long_code_mask_i: 0,
                reverse_long_code_mask_q: 0,
                drc_lock: true,
                physical_layer_subtype: 2,
                reverse_traffic_mac_subtype: 3,
                frame_offset: 0,
                drc_cover: 0,
                drc_length: 1,
            },
            identity: None,
            session_configuration_complete: true,
        };
        let mut open_uatis = HashSet::new();

        open_uatis.insert(session_uati);
        assert!(default_packet_flow_open_for_pending(&open_uatis, &pending));

        open_uatis.clear();
        remember_default_packet_flow_open(&mut open_uatis, traffic_uati);
        assert!(open_uatis.contains(&traffic_uati));
        assert!(open_uatis.contains(&session_uati));
        assert!(default_packet_flow_open_for_pending(&open_uatis, &pending));

        forget_default_packet_flow_open(&mut open_uatis, traffic_uati);
        assert!(!default_packet_flow_open_for_pending(&open_uatis, &pending));
    }

    #[test]
    fn pcf_a8_ipv4_pair_comes_from_configured_udp_exact_gre_endpoints() {
        let bearer = cdma_a8::BearerTransportConfig::udp_encapsulated_gre(
            "192.0.2.10:17041".parse().unwrap(),
            "192.0.2.11:17040".parse().unwrap(),
        );

        let (local, peer) = cdma_pcf::configured_a8_ipv4_pair(&bearer, "pcf.a8_bearer").unwrap();

        assert_eq!(local, [192, 0, 2, 10]);
        assert_eq!(peer, [192, 0, 2, 11]);
    }

    #[tokio::test]
    async fn hrpd_setup_a8_roundtrips_through_pcf_a9_listener() {
        let pcf_a8 = free_udp_addr();
        let an_a8 = free_udp_addr();
        let pcf_a10 = free_udp_addr();
        let pdsn_a10 = free_udp_addr();
        let mut pcf_config = test_pcf_node_config();
        pcf_config.a9_bind_addr = "127.0.0.1:0".parse().unwrap();
        pcf_config.a8_bearer = cdma_a8::BearerTransportConfig::udp_encapsulated_gre(pcf_a8, an_a8);
        pcf_config.a10_bearer =
            cdma_a10::BearerTransportConfig::udp_encapsulated_gre(pcf_a10, pdsn_a10);
        pcf_config.a11 = cdma_a11::A11TransportConfig::new(free_udp_addr(), free_udp_addr());
        let a9_config = spawn_hrpd_pcf_a9_service(pcf_config).await.unwrap();
        let endpoint = cdma_a9::UdpSignalingEndpoint::bind("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let assignment = hrpd_air::HrpdTrafficAssignmentRequest {
            session_uati: 0x0080_0580,
            uati: 0x0080_0580,
            mac_index: 9,
            reverse_rate_limit_bps: 153_600,
            reverse_long_code_mask_i: 0,
            reverse_long_code_mask_q: 0,
            drc_lock: true,
            physical_layer_subtype: 0,
            reverse_traffic_mac_subtype: 0,
            frame_offset: 0,
            drc_cover: 0,
            drc_length: 1,
        };
        let mut client = HrpdAnA9Client::new(&endpoint, a9_config);
        let context = client.setup_a8(&assignment, None).await.unwrap();

        assert_eq!(context.con_ref().0, assignment.mac_index);
        assert_eq!(context.a8_key(), assignment.uati);
        assert_eq!(client.sequence_no(), 1);
    }

    #[tokio::test]
    async fn hrpd_release_a8_roundtrips_through_pcf_a9_listener() {
        let pcf_a8 = free_udp_addr();
        let an_a8 = free_udp_addr();
        let pcf_a10 = free_udp_addr();
        let pdsn_a10 = free_udp_addr();
        let mut pcf_config = test_pcf_node_config();
        pcf_config.a9_bind_addr = "127.0.0.1:0".parse().unwrap();
        pcf_config.a8_bearer = cdma_a8::BearerTransportConfig::udp_encapsulated_gre(pcf_a8, an_a8);
        pcf_config.a10_bearer =
            cdma_a10::BearerTransportConfig::udp_encapsulated_gre(pcf_a10, pdsn_a10);
        pcf_config.a11 = cdma_a11::A11TransportConfig::new(free_udp_addr(), free_udp_addr());
        let a9_config = spawn_hrpd_pcf_a9_service(pcf_config).await.unwrap();
        let endpoint = cdma_a9::UdpSignalingEndpoint::bind("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let assignment = hrpd_air::HrpdTrafficAssignmentRequest {
            session_uati: 0x0080_0580,
            uati: 0x0080_0580,
            mac_index: 9,
            reverse_rate_limit_bps: 153_600,
            reverse_long_code_mask_i: 0,
            reverse_long_code_mask_q: 0,
            drc_lock: true,
            physical_layer_subtype: 0,
            reverse_traffic_mac_subtype: 0,
            frame_offset: 0,
            drc_cover: 0,
            drc_length: 1,
        };
        let mut client = HrpdAnA9Client::new(&endpoint, a9_config);
        let release_context = client.setup_a8(&assignment, None).await.unwrap();

        client
            .release_a8(assignment.uati, &release_context, "test SessionClose")
            .await
            .unwrap();

        assert_eq!(client.sequence_no(), 2);
    }

    #[tokio::test]
    async fn pdsn_registration_update_drives_pcf_disconnect_a8() {
        let pcf_a8 = free_udp_addr();
        let an_a8 = free_udp_addr();
        let pcf_a10 = free_udp_addr();
        let pdsn_a10 = free_udp_addr();
        let pdsn_a11 = cdma_a11::UdpEndpoint::bind("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let pdsn_a11_addr = pdsn_a11.local_addr().unwrap();
        let mut pcf_config = test_pcf_node_config();
        pcf_config.a9_bind_addr = "127.0.0.1:0".parse().unwrap();
        pcf_config.a8_bearer = cdma_a8::BearerTransportConfig::udp_encapsulated_gre(pcf_a8, an_a8);
        pcf_config.a10_bearer =
            cdma_a10::BearerTransportConfig::udp_encapsulated_gre(pcf_a10, pdsn_a10);
        pcf_config.a11 = cdma_a11::A11TransportConfig::new(free_udp_addr(), pdsn_a11_addr);
        let a9_config = spawn_hrpd_pcf_a9_service(pcf_config).await.unwrap();
        let endpoint = cdma_a9::UdpSignalingEndpoint::bind("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let assignment = hrpd_air::HrpdTrafficAssignmentRequest {
            session_uati: 0x0080_0580,
            uati: 0x0080_0580,
            mac_index: 9,
            reverse_rate_limit_bps: 153_600,
            reverse_long_code_mask_i: 0,
            reverse_long_code_mask_q: 0,
            drc_lock: true,
            physical_layer_subtype: 0,
            reverse_traffic_mac_subtype: 0,
            frame_offset: 0,
            drc_cover: 0,
            drc_length: 1,
        };
        let identity = HrpdA9MobileIdentity {
            imsi: Some("310009176936269".to_string()),
            esn: Some(0x4cdc_1d09),
            meid: None,
        };
        let (send_update, receive_update) = tokio::sync::oneshot::channel::<()>();
        let pdsn_task = tokio::spawn(async move {
            let mut buf = vec![0u8; 4096];
            let (message, pcf_peer) = tokio::time::timeout(
                Duration::from_secs(1),
                pdsn_a11.recv_message_unverified(
                    &mut buf,
                    cdma_a11::UnverifiedDecodeReason::TestFixture,
                ),
            )
            .await
            .unwrap()
            .unwrap();
            let cdma_a11::Message::RegistrationRequest(request) = message else {
                panic!("expected initial A11 Registration Request");
            };
            assert_eq!(request.lifetime, 600);
            assert_eq!(request.session.pcf_session_id, 1);
            pdsn_a11
                .send_message(pcf_peer, test_a11_registration_reply(&request))
                .await
                .unwrap();

            receive_update.await.unwrap();
            pdsn_a11
                .send_message(pcf_peer, test_a11_registration_update(&request))
                .await
                .unwrap();

            let (message, _) = tokio::time::timeout(
                Duration::from_secs(1),
                pdsn_a11.recv_message_unverified(
                    &mut buf,
                    cdma_a11::UnverifiedDecodeReason::TestFixture,
                ),
            )
            .await
            .unwrap()
            .unwrap();
            let cdma_a11::Message::RegistrationAcknowledge(ack) = message else {
                panic!("expected A11 Registration Acknowledge");
            };
            assert_eq!(ack.status, 0);
            assert_eq!(ack.session.pcf_session_id, request.session.pcf_session_id);

            let (message, _) = tokio::time::timeout(
                Duration::from_secs(1),
                pdsn_a11.recv_message_unverified(
                    &mut buf,
                    cdma_a11::UnverifiedDecodeReason::TestFixture,
                ),
            )
            .await
            .unwrap()
            .unwrap();
            let cdma_a11::Message::RegistrationRequest(deregistration) = message else {
                panic!("expected A11 lifetime-zero Registration Request");
            };
            assert_eq!(deregistration.lifetime, 0);
            assert_eq!(
                deregistration.session.pcf_session_id,
                request.session.pcf_session_id
            );
            pdsn_a11
                .send_message(pcf_peer, test_a11_registration_reply(&deregistration))
                .await
                .unwrap();
        });

        let mut client = HrpdAnA9Client::new(&endpoint, a9_config);
        let release_context = client.setup_a8(&assignment, Some(&identity)).await.unwrap();
        assert!(release_context.pdsn_ip_address().is_some());

        send_update.send(()).unwrap();
        let mut buf = vec![0u8; 4096];
        let (datagram, peer) =
            tokio::time::timeout(Duration::from_secs(1), endpoint.recv_datagram(&mut buf))
                .await
                .unwrap()
                .unwrap();
        assert_eq!(peer, a9_config.pcf_addr);
        assert_eq!(datagram.message_type, cdma_a9::MessageType::DisconnectA8);
        let disconnect = cdma_a9::DisconnectA8Message::decode(&datagram.payload).unwrap();
        assert_eq!(disconnect.a8_traffic_id.key, assignment.uati);
        assert_eq!(disconnect.con_ref.0, assignment.mac_index);
        assert_eq!(disconnect.cause.0, 0x77);

        client
            .release_a8_with_cause(
                assignment.uati,
                &release_context,
                "test PCF DisconnectA8",
                disconnect.cause,
            )
            .await
            .unwrap();

        pdsn_task.await.unwrap();
    }

    #[tokio::test]
    async fn hrpd_setup_a8_with_identity_registers_a11_and_returns_pdsn_ip() {
        let pcf_a8 = free_udp_addr();
        let an_a8 = free_udp_addr();
        let pcf_a10 = free_udp_addr();
        let pdsn_a10 = free_udp_addr();
        let mut packet = cdma_pdsn::PacketTransportConfig::default();
        packet.transport = "fou_tcp".to_string();
        packet.fou_remote = Some("127.0.0.1:17012".to_string());
        let pdsn_config = cdma_pdsn::PdsnNodeConfig {
            packet_grpc_listen_addr: "127.0.0.1:0".parse().unwrap(),
            a10_bearer: cdma_a10::BearerTransportConfig::udp_encapsulated_gre(pdsn_a10, pcf_a10),
            a11: cdma_a11::A11TransportConfig::new(
                "127.0.0.1:0".parse().unwrap(),
                "127.0.0.1:0".parse().unwrap(),
            ),
            a11_security: test_a11_security_config(),
            packet,
            ppp_session_timeout_secs: 1_800,
            events_endpoint: None,
        };
        let packet_service = cdma_pdsn::build_packet_service_with_sink(&pdsn_config, None).unwrap();
        let pdsn_a11_addr = spawn_hrpd_pdsn_a11_service(pdsn_config, packet_service)
            .await
            .unwrap();

        let mut pcf_config = test_pcf_node_config();
        pcf_config.a9_bind_addr = "127.0.0.1:0".parse().unwrap();
        pcf_config.a8_bearer = cdma_a8::BearerTransportConfig::udp_encapsulated_gre(pcf_a8, an_a8);
        pcf_config.a10_bearer =
            cdma_a10::BearerTransportConfig::udp_encapsulated_gre(pcf_a10, pdsn_a10);
        pcf_config.a11 =
            cdma_a11::A11TransportConfig::new("127.0.0.1:0".parse().unwrap(), pdsn_a11_addr);
        let a9_config = spawn_hrpd_pcf_a9_service(pcf_config).await.unwrap();
        let endpoint = cdma_a9::UdpSignalingEndpoint::bind("127.0.0.1:0".parse().unwrap())
            .await
            .unwrap();
        let assignment = hrpd_air::HrpdTrafficAssignmentRequest {
            session_uati: 0x8005_8001,
            uati: 0x8005_8001,
            mac_index: 9,
            reverse_rate_limit_bps: 153_600,
            reverse_long_code_mask_i: 0,
            reverse_long_code_mask_q: 0,
            drc_lock: true,
            physical_layer_subtype: 0,
            reverse_traffic_mac_subtype: 0,
            frame_offset: 0,
            drc_cover: 0,
            drc_length: 1,
        };
        let identity = HrpdA9MobileIdentity {
            imsi: Some("310009176936269".to_string()),
            esn: Some(0x4cdc_1d09),
            meid: None,
        };
        let mut client = HrpdAnA9Client::new(&endpoint, a9_config);
        let context = client.setup_a8(&assignment, Some(&identity)).await.unwrap();

        assert_eq!(context.con_ref().0, assignment.mac_index);
        assert_eq!(context.a8_key(), assignment.uati);
        assert_eq!(
            context.pdsn_ip_address(),
            Some(cdma_a9::PdsnIpAddress([127, 0, 0, 1]))
        );
    }
}
