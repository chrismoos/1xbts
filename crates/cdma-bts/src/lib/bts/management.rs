//! BTS-local management gRPC service.
//!
//! The BTS owns its radio parameters, so it answers `BtsManagementService`
//! for the one cell it operates. A BSC serves the same API across every cell
//! enrolled with it and forwards each request here, which lets one client
//! address either endpoint.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::pin::Pin;
use std::sync::Arc;

use cdma_common::band_class::ChannelPlan;
use cdma_common::error::Error;
use cdma_common::formatting::{bitstream_to_hex, forward_order_name};
use cdma_common::lac::MessageControlStatusBlock;
use cdma_common::lac::message_types::WireChannel;
use cdma_common::lac::paging_messages::{
    CHANNEL_ASSIGN_MODE_EXTENDED_TRAFFIC, CHANNEL_ASSIGN_MODE_TRAFFIC,
    CHANNEL_DEFAULT_CONFIG_EXPLICIT_RCS, CHANNEL_DEFAULT_CONFIG_RC1_RC1,
    CHANNEL_DEFAULT_CONFIG_RC1_RC2, CHANNEL_DEFAULT_CONFIG_RC2_RC1, CHANNEL_DEFAULT_CONFIG_RC2_RC2,
    ChannelAssignmentGrantedMode, ChannelAssignmentMessage, GeneralPageRecord, MsAddress,
    PagingChannelMessage,
};
use cdma_common::mac::ChannelType;
use cdma_common::overhead::OverheadParameters;
use cdma_common::timezone::TimezoneConfig;
use log::{info, warn};
use tokio::sync::{broadcast, mpsc, oneshot, watch};
use tokio_stream::Stream;
use tonic::{Request, Response, Status};

use crate::bts::evdo::{EvdoTxMode, ResolvedEvdoConfig, resolve_evdo_config};
use crate::bts::{
    BtsCommand, BtsNodeConfig, BtsPowerControlRegistry, BtsPowerControlSnapshot,
    BtsRuntimeSettings, IqCaptureStatus as BtsIqCaptureStatus, PchTransmitEvent,
    RxMetrics as BtsRxMetrics, TxMetrics as BtsTxMetrics,
};

/// Generated types for the BSC service proto (`bsc.v1`).
///
/// Nested one module per package segment because the generated
/// `bts_management.v1` code reaches its `bsc.v1` types through that path.
pub mod bsc {
    pub mod v1 {
        tonic::include_proto!("bsc.v1");
    }
}

/// Generated types for the BTS management proto (`bts_management.v1`).
pub mod bts_management {
    pub mod v1 {
        tonic::include_proto!("bts_management.v1");
    }
}

pub use bsc::v1 as proto;
pub use bts_management::v1 as bts_management_proto;

use bts_management_proto::bts_management_service_server::{
    BtsManagementService, BtsManagementServiceServer,
};
use bts_management_proto::{
    BtsAttachState, BtsList, BtsSummary, CellRequest, EnrollRequest, EnrollResponse,
    ReversePowerControlEntry, ReversePowerControlList, ReversePowerControlRequest,
};
use proto::CellId;

type GrpcStream<T> = Pin<Box<dyn Stream<Item = Result<T, Status>> + Send>>;

/// Everything the management service reads out of a running BTS.
///
/// The values that never change for the life of the process are held by
/// value. Live state is reached through the same handles the BTS itself uses.
pub struct BtsManagementState {
    /// Cell number broadcast as BASE_ID.
    pub cell: u32,
    /// Sector within the cell.
    pub sector: u32,
    pub tx_metrics: watch::Receiver<BtsTxMetrics>,
    pub rx_metrics: watch::Receiver<BtsRxMetrics>,
    pub config: Arc<BtsRuntimeSettings>,
    pub channel: ChannelPlan,
    pub tx_center_frequency_hz: usize,
    pub rx_center_frequency_hz: usize,
    /// Resolved EV-DO (HRPD) carrier, when EV-DO is enabled.
    pub evdo: Option<ResolvedEvdoConfig>,
    pub overhead: OverheadParameters,
    pub timezone: TimezoneConfig,
    pub pilot_offset: usize,
    /// Reverse-link power reference, when the radio is calibrated.
    pub rx_reference_dbm: Option<f64>,
    /// Where this cell's HRPD access network serves its session API.
    pub an_grpc_addr: Option<String>,
    /// Paging retransmission budget this BTS applies.
    pub paging_retry: crate::bts::config::BtsPagingRetryConfig,
    /// Whether a BSC holds the Abis signaling connection. The cell reports
    /// itself in service only while it does.
    pub abis_connected: Option<Arc<std::sync::atomic::AtomicBool>>,
    pub commands: mpsc::Sender<BtsCommand>,
    pub power_control: BtsPowerControlRegistry,
    pub iq_capture_dir: PathBuf,
    /// TX tap on the BTS paging supplier, carrying every SDU that went out on
    /// F-PCH with the MSG_SEQ the supplier assigned it.
    pub pch_transmit: broadcast::Sender<PchTransmitEvent>,
    /// Abis bearer transport, present once the Abis endpoint is up.
    pub bearer: Option<Arc<cdma_abis::bearer_transport::BearerTransport>>,
}

impl BtsManagementState {
    /// Radio parameters derived from the BTS node configuration, with the
    /// live handles supplied by the caller.
    ///
    /// The BSC builds its own view of the cell from what `Enroll` serves, so a
    /// carrier the BTS transmits but cannot describe here would leave the two
    /// disagreeing. An unresolvable `evdo` block is therefore an error rather
    /// than an absent carrier.
    // The arguments are the live handles the BTS wires in exactly once. A
    // struct would be built and destructured at the same single call site.
    #[allow(clippy::too_many_arguments)]
    pub fn from_node_config(
        node: &BtsNodeConfig,
        runtime: Arc<BtsRuntimeSettings>,
        tx_metrics: watch::Receiver<BtsTxMetrics>,
        rx_metrics: watch::Receiver<BtsRxMetrics>,
        commands: mpsc::Sender<BtsCommand>,
        power_control: BtsPowerControlRegistry,
        iq_capture_dir: PathBuf,
        pch_transmit: broadcast::Sender<PchTransmitEvent>,
    ) -> Result<Self, Error> {
        let tx_center_frequency_hz = node
            .runtime
            .tx_freq_hz_override
            .unwrap_or_else(|| node.channel.downlink_hz() as usize);
        let evdo = resolve_evdo_config(
            &node.evdo,
            node.pilot_offset,
            node.channel,
            node.runtime.tx_sample_rate_hz,
            node.runtime.tx_bandwidth_hz,
        )?;
        Ok(Self {
            cell: u32::from(node.overhead.base_id),
            sector: u32::from(node.sector),
            tx_metrics,
            rx_metrics,
            config: runtime,
            channel: node.channel,
            tx_center_frequency_hz,
            rx_center_frequency_hz: node.channel.uplink_hz() as usize,
            evdo,
            overhead: node.overhead.clone(),
            timezone: node.timezone.clone(),
            pilot_offset: node.pilot_offset,
            rx_reference_dbm: node.radio.rx_reference_dbm(),
            an_grpc_addr: None,
            paging_retry: node.paging_retry.clone(),
            abis_connected: None,
            commands,
            power_control,
            iq_capture_dir,
            pch_transmit,
            bearer: None,
        })
    }

    /// How far this cell has progressed toward carrying traffic, as the BTS
    /// can observe it. Abis is the only step it sees: enrollment is the BSC's
    /// record, so a BTS that no BSC has attached to still reports `Enrolled`.
    fn attach_state(&self) -> BtsAttachState {
        match &self.abis_connected {
            // Relaxed: a lone flag publishing no other data.
            Some(flag) if flag.load(std::sync::atomic::Ordering::Relaxed) => {
                BtsAttachState::InService
            }
            _ => BtsAttachState::Enrolled,
        }
    }

    fn cell_id(&self) -> CellId {
        CellId {
            cell: self.cell,
            sector: self.sector,
        }
    }

    /// Accept a request that names no cell or names this one, and reject any
    /// other cell so a misrouted request fails loudly.
    fn check_cell(&self, cell: Option<CellId>) -> Result<(), Status> {
        match cell {
            None => Ok(()),
            Some(cell) if cell.cell == self.cell && cell.sector == self.sector => Ok(()),
            Some(cell) => Err(Status::not_found(format!(
                "cell {}/{} is not served by this BTS (serves {}/{})",
                cell.cell, cell.sector, self.cell, self.sector
            ))),
        }
    }

    fn bts_config(&self) -> proto::BtsConfig {
        to_proto_config(
            &self.config,
            self.channel,
            self.tx_center_frequency_hz,
            self.rx_center_frequency_hz,
            self.evdo.as_ref(),
            &self.overhead,
            &self.timezone,
            self.pilot_offset,
        )
    }

    fn radio_metrics(&self) -> proto::RadioMetrics {
        // Snapshot before converting: the conversions allocate, and a watch
        // borrow held across them blocks the TX loop's next metrics publish.
        let tx = self.tx_metrics.borrow().clone();
        let rx = self.rx_metrics.borrow().clone();
        proto::RadioMetrics {
            tx: Some(to_proto_tx_metrics(&tx)),
            rx: Some(to_proto_rx_metrics(&rx)),
            bearer: self
                .bearer
                .as_ref()
                .map(|bearer| to_proto_bearer_metrics(&bearer.stats())),
        }
    }

    async fn capture(&self, command: CaptureCommand) -> Result<proto::IqCaptureStatus, Status> {
        let (respond_to, rx) = oneshot::channel();
        let command = match command {
            CaptureCommand::Status => BtsCommand::GetCaptureStatus {
                directory: self.iq_capture_dir.clone(),
                respond_to,
            },
            CaptureCommand::Start => BtsCommand::StartCapture {
                directory: self.iq_capture_dir.clone(),
                respond_to,
            },
            CaptureCommand::Stop => BtsCommand::StopCapture { respond_to },
        };
        self.commands
            .send(command)
            .await
            .map_err(|e| Status::unavailable(format!("BTS command queue unavailable: {e}")))?;
        let result = rx
            .await
            .map_err(|_| Status::unavailable("BTS RX thread dropped capture response"))?
            .map_err(Status::failed_precondition)?;
        Ok(to_proto_iq_capture_status(&result.status))
    }
}

enum CaptureCommand {
    Status,
    Start,
    Stop,
}

/// `BtsManagementService` served by a BTS for its own cell.
#[derive(Clone)]
pub struct BtsManagementServiceImpl {
    state: Arc<BtsManagementState>,
}

impl BtsManagementServiceImpl {
    pub fn new(state: Arc<BtsManagementState>) -> Self {
        Self { state }
    }

    /// Server ready to be added to a `tonic` router alongside other services.
    pub fn into_server(self) -> BtsManagementServiceServer<Self> {
        BtsManagementServiceServer::new(self)
    }
}

/// Serve `BtsManagementService` on `addr` and return the bound address.
///
/// Binding happens before the function returns, so a caller that passed port
/// 0 learns the assigned port and a bind failure is reported synchronously.
pub async fn spawn_bts_management_service(
    addr: SocketAddr,
    state: Arc<BtsManagementState>,
) -> Result<SocketAddr, Error> {
    let listener = tokio::net::TcpListener::bind(addr)
        .await
        .map_err(|e| Error::from(format!("BTS management gRPC bind {addr} failed: {e}")))?;
    let local_addr = listener.local_addr().map_err(|e| {
        Error::from(format!(
            "BTS management gRPC local address unavailable: {e}"
        ))
    })?;
    let service = BtsManagementServiceImpl::new(state);
    info!("BTS management gRPC server on {local_addr}");
    tokio::spawn(async move {
        // Exit if the listener dies rather than staying up unenrollable, so
        // a supervisor sees the failure instead of a healthy-looking process.
        match tonic::transport::Server::builder()
            .add_service(service.into_server())
            .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
            .await
        {
            Ok(()) => log::error!("BTS management gRPC server on {local_addr} stopped"),
            Err(e) => log::error!("BTS management gRPC server on {local_addr} stopped: {e}"),
        }
        std::process::exit(1);
    });
    Ok(local_addr)
}

#[tonic::async_trait]
impl BtsManagementService for BtsManagementServiceImpl {
    async fn enroll(&self, _: Request<EnrollRequest>) -> Result<Response<EnrollResponse>, Status> {
        Ok(Response::new(EnrollResponse {
            cell: Some(self.state.cell_id()),
            config: Some(self.state.bts_config()),
            rx_reference_dbm: self.state.rx_reference_dbm,
            an_grpc_addr: self.state.an_grpc_addr.clone(),
            paging_retry: Some(bts_management_proto::PagingRetryConfig {
                ack_timeout_ms: self.state.paging_retry.ack_timeout_ms as u32,
                max_retries: self.state.paging_retry.max_retries,
            }),
        }))
    }

    async fn list_bts(&self, _: Request<()>) -> Result<Response<BtsList>, Status> {
        let state = &self.state;
        Ok(Response::new(BtsList {
            bts: vec![BtsSummary {
                // The BSC assigns the peer id and endpoint, so the BTS leaves
                // them empty.
                peer_id: String::new(),
                management_endpoint: String::new(),
                cell: Some(state.cell_id()),
                state: state.attach_state() as i32,
                pilot_pn: state.pilot_offset as u32,
                band_class: state.channel.band_class.as_str().to_string(),
                cdma_channel: u32::from(state.channel.cdma_channel),
                sid: u32::from(state.overhead.sid),
                nid: u32::from(state.overhead.nid),
                evdo_enabled: state.evdo.is_some(),
                evdo_color_code: state
                    .evdo
                    .as_ref()
                    .map(|evdo| u32::from(evdo.overhead.color_code)),
                an_grpc_addr: state.an_grpc_addr.clone(),
                served_mobiles: None,
                status_detail: None,
            }],
        }))
    }

    async fn get_bts_status(
        &self,
        request: Request<CellRequest>,
    ) -> Result<Response<proto::SystemStatus>, Status> {
        self.state.check_cell(request.into_inner().cell)?;
        let oh = &self.state.overhead;
        Ok(Response::new(proto::SystemStatus {
            running: true,
            sid: u32::from(oh.sid),
            nid: u32::from(oh.nid),
            base_id: u32::from(oh.base_id),
            pilot_pn: self.state.pilot_offset as u32,
            reg_zone: u32::from(oh.reg_zone),
        }))
    }

    async fn get_bts_config(
        &self,
        request: Request<CellRequest>,
    ) -> Result<Response<proto::BtsConfig>, Status> {
        self.state.check_cell(request.into_inner().cell)?;
        Ok(Response::new(self.state.bts_config()))
    }

    async fn get_radio_metrics(
        &self,
        request: Request<CellRequest>,
    ) -> Result<Response<proto::RadioMetrics>, Status> {
        self.state.check_cell(request.into_inner().cell)?;
        Ok(Response::new(self.state.radio_metrics()))
    }

    type StreamRadioMetricsStream = GrpcStream<proto::RadioMetrics>;

    async fn stream_radio_metrics(
        &self,
        request: Request<CellRequest>,
    ) -> Result<Response<Self::StreamRadioMetricsStream>, Status> {
        self.state.check_cell(request.into_inner().cell)?;
        let state = self.state.clone();
        let mut tx_metrics = state.tx_metrics.clone();
        let stream = async_stream::stream! {
            loop {
                if tx_metrics.changed().await.is_err() {
                    break;
                }
                yield Ok(state.radio_metrics());
            }
        };
        Ok(Response::new(Box::pin(stream)))
    }

    async fn get_iq_capture_status(
        &self,
        request: Request<CellRequest>,
    ) -> Result<Response<proto::IqCaptureStatus>, Status> {
        self.state.check_cell(request.into_inner().cell)?;
        Ok(Response::new(
            self.state.capture(CaptureCommand::Status).await?,
        ))
    }

    async fn start_iq_capture(
        &self,
        request: Request<CellRequest>,
    ) -> Result<Response<proto::IqCaptureStatus>, Status> {
        self.state.check_cell(request.into_inner().cell)?;
        Ok(Response::new(
            self.state.capture(CaptureCommand::Start).await?,
        ))
    }

    async fn stop_iq_capture(
        &self,
        request: Request<CellRequest>,
    ) -> Result<Response<proto::IqCaptureStatus>, Status> {
        self.state.check_cell(request.into_inner().cell)?;
        Ok(Response::new(
            self.state.capture(CaptureCommand::Stop).await?,
        ))
    }

    async fn list_local_radio_resources(
        &self,
        request: Request<CellRequest>,
    ) -> Result<Response<proto::ChannelList>, Status> {
        self.state.check_cell(request.into_inner().cell)?;
        let cfg = &self.state.config;
        let cell = self.state.cell_id();
        let mut channels = vec![
            proto::Channel {
                walsh_code: Some(cfg.downlink.pilot.walsh_code as u32),
                channel_type: "pilot".into(),
                direction: "forward".into(),
                power_fraction: Some(cfg.downlink.pilot.power_fraction),
                data_rate_bps: None,
                paging_channel_number: None,
                access_channel_number: None,
                mobile: None,
                service_option: None,
                traffic_power: None,
                cell: Some(cell),
            },
            proto::Channel {
                walsh_code: Some(cfg.downlink.sync.walsh_code as u32),
                channel_type: "sync".into(),
                direction: "forward".into(),
                power_fraction: Some(cfg.downlink.sync.power_fraction),
                data_rate_bps: Some(cfg.downlink.sync.data_rate_bps as u32),
                paging_channel_number: None,
                access_channel_number: None,
                mobile: None,
                service_option: None,
                traffic_power: None,
                cell: Some(cell),
            },
            proto::Channel {
                walsh_code: Some(cfg.downlink.paging.walsh_code as u32),
                channel_type: "paging".into(),
                direction: "forward".into(),
                power_fraction: Some(cfg.downlink.paging.power_fraction),
                data_rate_bps: Some(cfg.downlink.paging.data_rate_bps as u32),
                paging_channel_number: Some(u32::from(cfg.downlink.paging.paging_channel_number)),
                access_channel_number: None,
                mobile: None,
                service_option: None,
                traffic_power: None,
                cell: Some(cell),
            },
        ];

        for &acc_num in &cfg.uplink.access_channel_numbers {
            channels.push(proto::Channel {
                walsh_code: None,
                channel_type: "access".into(),
                direction: "reverse".into(),
                power_fraction: None,
                data_rate_bps: Some(cfg.uplink.access_channel_rate_bps as u32),
                paging_channel_number: None,
                access_channel_number: Some(u32::from(acc_num)),
                mobile: None,
                service_option: None,
                traffic_power: None,
                cell: Some(cell),
            });
        }

        for snapshot in self.state.power_control.snapshots() {
            channels.push(proto::Channel {
                walsh_code: Some(u32::from(snapshot.walsh_code)),
                channel_type: "traffic".into(),
                direction: "forward".into(),
                power_fraction: None,
                data_rate_bps: None,
                paging_channel_number: None,
                access_channel_number: None,
                mobile: None,
                service_option: None,
                traffic_power: Some(to_proto_bts_reverse_power(&snapshot)),
                cell: Some(cell),
            });
        }

        channels.sort_by_key(|c| (c.direction.clone(), c.walsh_code.unwrap_or(u32::MAX)));

        let total_walsh_codes = cfg.orthogonal_code_length as u32;
        Ok(Response::new(proto::ChannelList {
            channels,
            total_walsh_codes,
            cell_walsh_capacity: vec![proto::CellWalshCapacity {
                cell: Some(cell),
                total_walsh_codes,
            }],
        }))
    }

    async fn get_reverse_power_control(
        &self,
        request: Request<ReversePowerControlRequest>,
    ) -> Result<Response<proto::TrafficChannelPower>, Status> {
        let req = request.into_inner();
        self.state.check_cell(req.cell)?;
        let walsh_code = u8::try_from(req.walsh_code)
            .map_err(|_| Status::invalid_argument("walsh_code must fit in u8"))?;
        let snapshot = self
            .state
            .power_control
            .snapshot(walsh_code)
            .ok_or_else(|| Status::not_found("active BTS power-control state not found"))?;
        Ok(Response::new(to_proto_bts_reverse_power(&snapshot)))
    }

    async fn list_reverse_power_controls(
        &self,
        request: Request<CellRequest>,
    ) -> Result<Response<ReversePowerControlList>, Status> {
        self.state.check_cell(request.into_inner().cell)?;
        Ok(Response::new(ReversePowerControlList {
            entries: self
                .state
                .power_control
                .snapshots()
                .iter()
                .map(|snapshot| ReversePowerControlEntry {
                    walsh_code: u32::from(snapshot.walsh_code),
                    power: Some(to_proto_bts_reverse_power(snapshot)),
                })
                .collect(),
        }))
    }

    async fn set_reverse_power_control_override(
        &self,
        request: Request<proto::SetTrafficChannelPowerOverrideRequest>,
    ) -> Result<Response<proto::SetTrafficChannelPowerOverrideResponse>, Status> {
        let req = request.into_inner();
        self.state.check_cell(req.cell)?;
        let walsh_code = u8::try_from(req.walsh_code)
            .map_err(|_| Status::invalid_argument("walsh_code must fit in u8"))?;
        let power_control = &self.state.power_control;
        match req.action {
            Some(proto::set_traffic_channel_power_override_request::Action::SetTargetEbNtDb(
                target_db,
            )) => power_control.set_target(walsh_code, target_db, true),
            Some(proto::set_traffic_channel_power_override_request::Action::Clear(_)) => {
                let target_db = power_control
                    .snapshot(walsh_code)
                    .map(|snapshot| snapshot.target_eb_nt_db)
                    .ok_or_else(|| Status::not_found("active BTS power-control state not found"))?;
                power_control.set_target(walsh_code, target_db, false);
            }
            None => {
                return Err(Status::invalid_argument(
                    "one of set_target_eb_nt_db or clear is required",
                ));
            }
        }

        let snapshot = power_control
            .snapshot(walsh_code)
            .ok_or_else(|| Status::not_found("active BTS power-control state not found"))?;
        let message = match snapshot.manual_target_override_db {
            Some(manual_db) => {
                format!("manual reverse target pinned at {manual_db:.2} dB on walsh {walsh_code}")
            }
            None => format!(
                "manual reverse target cleared on walsh {walsh_code}; auto resumed at {:.2} dB",
                snapshot.target_eb_nt_db
            ),
        };
        Ok(Response::new(
            proto::SetTrafficChannelPowerOverrideResponse {
                accepted: true,
                message,
                traffic_power: Some(to_proto_bts_reverse_power(&snapshot)),
            },
        ))
    }

    type StreamPchTransmissionsStream = GrpcStream<proto::PagingEvent>;

    async fn stream_pch_transmissions(
        &self,
        request: Request<CellRequest>,
    ) -> Result<Response<Self::StreamPchTransmissionsStream>, Status> {
        self.state.check_cell(request.into_inner().cell)?;
        let mut rx = self.state.pch_transmit.subscribe();
        let cell = self.state.cell_id();
        let stream = async_stream::stream! {
            loop {
                match rx.recv().await {
                    Ok(evt) => match pch_transmit_event_to_proto(&evt, cell) {
                        Ok(event) => yield Ok(event),
                        Err(e) => yield Err(Status::internal(format!(
                            "PCH transmit reconstruction failed: {e}"
                        ))),
                    },
                    Err(broadcast::error::RecvError::Lagged(n)) => {
                        warn!("PCH transmit stream lagged, skipped {n} events");
                    }
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
        };
        Ok(Response::new(Box::pin(stream)))
    }
}

/// Reconstruct the paging-channel message a `PchTransmitEvent` carries.
///
/// Decode failures are returned to the caller. A placeholder message would
/// hide a wire bug.
fn pch_transmit_event_to_proto(
    evt: &PchTransmitEvent,
    cell: proto::CellId,
) -> Result<proto::PagingEvent, Error> {
    let mut bs = cdma_common::bits::Bitstream::new_bytes(&evt.sdu_bytes);
    if evt.length_bits > bs.len() {
        return Err(format!(
            "PCH {} SDU length_bits={} exceeds packed payload bits={}",
            evt.message_id.tag(),
            evt.length_bits,
            bs.len()
        )
        .into());
    }
    if evt.length_bits < bs.len() {
        let _ = bs.drain(evt.length_bits..bs.len());
    }
    let message = PagingChannelMessage::from_sdu(evt.message_id, &mut bs).map_err(|e| {
        format!(
            "PCH {} body decode failed: {e}; sdu={}",
            evt.message_id.tag(),
            evt.sdu_bytes
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<Vec<_>>()
                .join("")
        )
    })?;
    let mcsb = MessageControlStatusBlock {
        channel: ChannelType::FPch,
        length_bits: evt.length_bits,
        mobile_p_rev: None,
        extended_encryption: false,
        message_id: evt.message_id,
        requested_tx_time: None,
        tx_deadline: None,
        address: evt.address.clone(),
        ack_seq: evt.ack_seq,
        msg_seq: evt.msg_seq,
        ack_req: evt.ack_req,
        valid_ack: true,
        overhead_mcc: evt.overhead_mcc,
        overhead_imsi_11_12: evt.overhead_imsi_11_12,
    };
    Ok(to_proto_paging_event(&message, &mcsb, cell))
}

fn to_proto_tx_metrics(m: &BtsTxMetrics) -> proto::TxMetrics {
    proto::TxMetrics {
        timestamp_ns: m.timestamp_ns as i64,
        chip_cursor: m.chip_cursor,
        blocks_transmitted: m.blocks_transmitted,
        rt_ratio: m.rt_ratio,
        gen_avg_us: m.gen_avg_us,
        gen_max_us: m.gen_max_us,
        tx_avg_us: m.tx_avg_us,
        tx_max_us: m.tx_max_us,
        pulse_avg_us: m.pulse_avg_us,
        pulse_max_us: m.pulse_max_us,
        synth_pilot_us: m.synth_pilot_us,
        synth_sync_us: m.synth_sync_us,
        synth_paging_us: m.synth_paging_us,
        synth_spread_us: m.synth_spread_us,
        sync_fragments_sent: m.sync_fragments_sent,
        paging_fragments_sent: m.paging_fragments_sent,
        hw_margin_min_us: m.hw_margin_min_us,
        hw_margin_avg_us: m.hw_margin_avg_us,
        hw_margin_max_us: m.hw_margin_max_us,
        late_batches: m.late_batches,
        radio_health: Some(proto::TxRadioHealth {
            underflows: m.radio_health.underflows,
            late_packets: m.radio_health.late_packets,
            sequence_errors: m.radio_health.sequence_errors,
            burst_acks: m.radio_health.burst_acks,
            dropped_packets: m.radio_health.dropped_packets,
            unknown_events: m.radio_health.unknown_events,
        }),
        realtime_degraded_events: m.realtime_degraded_events,
        finalized_queue_airtime_us: m.finalized_queue_airtime_us,
    }
}

fn to_proto_rx_metrics(m: &BtsRxMetrics) -> proto::RxMetrics {
    proto::RxMetrics {
        reads: m.reads,
        samples: m.samples,
        rt_ratio: m.rt_ratio,
        capture_us: m.capture_us,
        pipeline_us: m.pipeline_us,
        total_us: m.total_us,
        total_max_us: m.total_max_us,
        stages: m
            .stages
            .iter()
            .map(|s| proto::StageMetrics {
                name: s.name.clone(),
                total_us: s.total_us,
                calls: s.calls,
                max_us: s.max_us,
                pct_pipeline: s.pct_pipeline,
            })
            .collect(),
        deficit_ms: m.deficit_ms,
        queues: m
            .queues
            .iter()
            .map(|q| proto::RxQueueMetrics {
                name: q.name.clone(),
                queued_samples: q.queued_samples,
                queued_airtime_us: q.queued_airtime_us,
                max_queued_samples: q.max_queued_samples,
                max_residency_us: q.max_residency_us,
            })
            .collect(),
    }
}

fn to_proto_bearer_metrics(
    stats: &cdma_abis::bearer_transport::BearerTransportStats,
) -> proto::BearerMetrics {
    proto::BearerMetrics {
        tx_frames: stats.tx_datagrams,
        rx_accepted: stats.rx_accepted,
        duplicate_drop: stats.rx_duplicate_drop,
        late_drop: stats.rx_late_drop,
        encode_errors: 0,
        route_errors: stats.rx_route_errors + stats.rx_decode_errors,
        delivery_errors: stats.tx_errors,
    }
}

/// Reverse power-control state as the BTS measures it.
///
/// The forward-loop and per-PCG history fields stay empty here. The BSC
/// drives them from PMRM reports and fills them in when it joins this
/// snapshot onto a mobile.
fn to_proto_bts_reverse_power(snapshot: &BtsPowerControlSnapshot) -> proto::TrafficChannelPower {
    proto::TrafficChannelPower {
        target_eb_nt_db: snapshot.target_eb_nt_db,
        effective_target_eb_nt_db: snapshot.effective_target_eb_nt_db,
        manual_target_override_db: snapshot.manual_target_override_db,
        last_pcg_snr_db: Vec::new(),
        last_active_pcg_mask: Vec::new(),
        last_pcbs: snapshot.last_pcbs.iter().map(|b| u32::from(*b)).collect(),
        reverse_pilot_ec_io_db: None,
        fer_pct: snapshot.fer_pct,
        frames_total: snapshot.frames_total,
        frames_crc_error: snapshot.frames_crc_error,
        forward_gain_offset_db: 0.0,
        forward_last_fer_pct: 0.0,
        forward_last_pmrm_errors: 0,
        forward_last_pmrm_frames: 0,
        forward_pmrm_count: 0,
        forward_pilot_ec_io_db: Vec::new(),
        last_pcg_pilot_ec_nt_db: snapshot.last_pcg_pilot_ec_nt_db.to_vec(),
        forward_radio_config: 0,
        reverse_radio_config: 0,
        power_history: Vec::new(),
        measured_inner_loop_1s_db: snapshot
            .measured_inner_loop_1s
            .map(|measurement| measurement.mean_db),
        measured_inner_loop_1s_timestamp_ms: snapshot
            .measured_inner_loop_1s
            .map(|measurement| measurement.timestamp_ms),
    }
}

fn to_proto_iq_capture_status(status: &BtsIqCaptureStatus) -> proto::IqCaptureStatus {
    proto::IqCaptureStatus {
        active: status.active,
        directory: status.directory.display().to_string(),
        wav_path: status.wav_path.as_ref().map(|p| p.display().to_string()),
        metadata_path: status
            .metadata_path
            .as_ref()
            .map(|p| p.display().to_string()),
        first_absolute_chip_start: status.first_absolute_chip_start,
        // Omitted: at 8x capture this exceeds JS's safe integer range. Exact
        // sample time stays in the sidecar metadata JSON.
        first_absolute_sample_start: None,
        first_sample_system_time: status
            .first_sample_system_time
            .as_ref()
            .map(|t| t.to_rfc3339()),
        first_hardware_time_ns: status.first_hardware_time_ns.map(|v| v as i64),
        captured_samples: status.captured_samples,
        captured_seconds: status.captured_samples as f64 / status.sample_rate_hz.max(1) as f64,
        sample_rate_hz: status.sample_rate_hz as u32,
        chip_rate_hz: status.chip_rate_hz as u32,
    }
}

fn to_proto_evdo(r: &ResolvedEvdoConfig) -> proto::EvdoCarrierConfig {
    let mode = match r.tx_mode {
        EvdoTxMode::AdjacentComposite => proto::EvdoTxMode::AdjacentComposite,
        EvdoTxMode::HrpdOnly => proto::EvdoTxMode::HrpdOnly,
    };
    let sector_id = r
        .overhead
        .sector_id
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect::<String>();
    proto::EvdoCarrierConfig {
        channel: u32::from(r.evdo_channel),
        band_class: u32::from(r.evdo_band_class),
        frequency_hz: r.evdo_frequency_hz as u64,
        reverse_frequency_hz: r.evdo_reverse_frequency_hz as u64,
        composite_center_frequency_hz: r.composite_center_frequency_hz as u64,
        mode: mode as i32,
        gain: r.gain,
        advertise_on_1x: r.advertise_on_1x,
        sector_id,
        color_code: u32::from(r.overhead.color_code),
        subnet_mask: u32::from(r.overhead.subnet_mask),
    }
}

// The arguments mirror the proto message's field groups one-to-one.
#[allow(clippy::too_many_arguments)]
fn to_proto_config(
    cfg: &BtsRuntimeSettings,
    channel: ChannelPlan,
    tx_center_frequency_hz: usize,
    rx_center_frequency_hz: usize,
    evdo: Option<&ResolvedEvdoConfig>,
    overhead: &OverheadParameters,
    timezone: &TimezoneConfig,
    pilot_offset: usize,
) -> proto::BtsConfig {
    use cdma_common::timezone::TimezoneSource;
    let resolved = cdma_common::timezone::resolve(timezone, overhead, chrono::Utc::now());
    let (source_str, tz_name) = match &timezone.source {
        TimezoneSource::Overhead => ("overhead".to_string(), None),
        TimezoneSource::System => (
            "system".to_string(),
            cdma_common::timezone::host_iana_name(),
        ),
        TimezoneSource::User { tz } => ("user".to_string(), Some(tz.clone())),
    };
    proto::BtsConfig {
        pilot_offset: pilot_offset as u32,
        spreading_rate: format!("{:?}", cfg.spreading_rate),
        chip_rate_hz: cfg.chip_rate_hz as u32,
        tx_sample_rate_hz: cfg.tx_sample_rate_hz as u32,
        tx_bandwidth_hz: cfg.tx_bandwidth_hz as u32,
        tx_center_frequency_hz: tx_center_frequency_hz as u32,
        rx_center_frequency_hz: rx_center_frequency_hz as u32,
        band_class: channel.band_class.as_str().to_string(),
        cdma_channel: u32::from(channel.cdma_channel),
        band_subclass: u32::from(channel.band_subclass),
        tx_digital_backoff: cfg.tx_digital_backoff,
        block_size_chips: cfg.block_size_chips as u32,
        pilot: Some(proto::PilotConfig {
            walsh_code: cfg.downlink.pilot.walsh_code as u32,
            power_fraction: cfg.downlink.pilot.power_fraction,
        }),
        sync: Some(proto::SyncConfig {
            walsh_code: cfg.downlink.sync.walsh_code as u32,
            data_rate_bps: cfg.downlink.sync.data_rate_bps as u32,
            power_fraction: cfg.downlink.sync.power_fraction,
        }),
        paging: Some(proto::PagingConfig {
            walsh_code: cfg.downlink.paging.walsh_code as u32,
            paging_channel_number: u32::from(cfg.downlink.paging.paging_channel_number),
            data_rate_bps: cfg.downlink.paging.data_rate_bps as u32,
            power_fraction: cfg.downlink.paging.power_fraction,
        }),
        overhead: Some(proto::OverheadConfig {
            sid: u32::from(overhead.sid),
            nid: u32::from(overhead.nid),
            base_id: u32::from(overhead.base_id),
            reg_zone: u32::from(overhead.reg_zone),
            total_zones: u32::from(overhead.total_zones),
            zone_timer: u32::from(overhead.zone_timer),
            max_slot_cycle_index: u32::from(overhead.max_slot_cycle_index),
            page_chan: u32::from(overhead.page_chan),
            config_seq: u32::from(overhead.config_seq),
            acc_config_seq: u32::from(overhead.acc_config_seq),
            power_up_reg: overhead.power_up_reg,
            parameter_reg: overhead.parameter_reg,
            auth_mode: u32::from(overhead.auth_mode),
            lp_sec: u32::from(overhead.lp_sec),
            ltm_off: i32::from(overhead.ltm_off),
            daylt: u32::from(overhead.daylt),
            p_rev: u32::from(overhead.p_rev),
            min_p_rev: u32::from(overhead.min_p_rev),
            cdma_freq: overhead.cdma_freq.map(u32::from),
            ext_cdma_freq: overhead.ext_cdma_freq.map(u32::from),
            band_class_override: overhead.band_class.map(u32::from),
            mcc_digits: {
                let esp = &cfg
                    .downlink
                    .paging
                    .message_defaults
                    .extended_system_parameters;
                cdma_common::paging::mcc_to_digits(esp.mcc).unwrap_or_default()
            },
            imsi_11_12_digits: {
                let esp = &cfg
                    .downlink
                    .paging
                    .message_defaults
                    .extended_system_parameters;
                cdma_common::paging::imsi_11_12_to_digits(esp.imsi_11_12).unwrap_or_default()
            },
        }),
        timezone: Some(proto::TimezoneConfig {
            source: source_str.clone(),
            tz: match &timezone.source {
                TimezoneSource::User { tz } => Some(tz.clone()),
                _ => None,
            },
        }),
        timezone_status: Some(proto::TimezoneStatus {
            source: source_str,
            tz: tz_name,
            ltm_off: resolved.ltm_off as i32,
            daylt: u32::from(resolved.daylt),
            lp_sec: u32::from(resolved.lp_sec),
            utc_offset_seconds: i32::from(resolved.local_time_offset_minutes) * 60,
        }),
        evdo: evdo.map(to_proto_evdo),
    }
}

fn format_ms_address(addr: &MsAddress) -> String {
    match addr {
        MsAddress::Esn(esn) => format!("ESN:0x{esn:08X}"),
        MsAddress::ImsiS {
            imsi_m_s1,
            imsi_m_s2,
        } => format!("IMSI_S:s1={imsi_m_s1},s2={imsi_m_s2}"),
        MsAddress::ImsiClass0 {
            imsi_m_s1,
            imsi_m_s2,
            mcc,
            imsi_11_12,
        } => format!("IMSI_CLASS0:s1={imsi_m_s1},s2={imsi_m_s2},mcc={mcc},imsi_11_12={imsi_11_12}"),
    }
}

fn ms_address_to_proto(addr: &MsAddress) -> proto::PagingAddress {
    match addr {
        MsAddress::Esn(esn) => proto::PagingAddress {
            addr: Some(proto::paging_address::Addr::Esn(*esn)),
        },
        MsAddress::ImsiS {
            imsi_m_s1,
            imsi_m_s2,
        } => proto::PagingAddress {
            addr: Some(proto::paging_address::Addr::ImsiS(proto::ImsiS {
                imsi_m_s1: *imsi_m_s1,
                imsi_m_s2: u32::from(*imsi_m_s2),
            })),
        },
        MsAddress::ImsiClass0 {
            imsi_m_s1,
            imsi_m_s2,
            ..
        } => proto::PagingAddress {
            addr: Some(proto::paging_address::Addr::ImsiClass0(proto::ImsiClass0 {
                imsi_m_s1: *imsi_m_s1,
                imsi_m_s2: u32::from(*imsi_m_s2),
            })),
        },
    }
}

fn cam_effective_radio_config(cam: &ChannelAssignmentMessage) -> Option<(u8, u8)> {
    const DEFAULT_CONFIGURATION: u8 = ChannelAssignmentGrantedMode::DefaultConfiguration as u8;
    const REQUESTED_SERVICE: u8 = ChannelAssignmentGrantedMode::RequestedService as u8;
    match (cam.assign_mode, cam.granted_mode, cam.default_config) {
        (CHANNEL_ASSIGN_MODE_TRAFFIC, _, _) => Some((1, 1)),
        (
            CHANNEL_ASSIGN_MODE_EXTENDED_TRAFFIC,
            Some(DEFAULT_CONFIGURATION),
            Some(CHANNEL_DEFAULT_CONFIG_RC1_RC1),
        ) => Some((1, 1)),
        (
            CHANNEL_ASSIGN_MODE_EXTENDED_TRAFFIC,
            Some(DEFAULT_CONFIGURATION),
            Some(CHANNEL_DEFAULT_CONFIG_RC2_RC2),
        ) => Some((2, 2)),
        // The supported requested-service CAM is the P_REV 3 QCELP 13K
        // assignment. DEFAULT_CONFIG remains the RC1 fallback.
        (
            CHANNEL_ASSIGN_MODE_EXTENDED_TRAFFIC,
            Some(REQUESTED_SERVICE),
            Some(CHANNEL_DEFAULT_CONFIG_RC1_RC1),
        ) => Some((2, 2)),
        _ => None,
    }
}

fn next_pch_event_id() -> String {
    use std::sync::atomic::{AtomicU64, Ordering};
    static SEQ: AtomicU64 = AtomicU64::new(1);
    format!("pch-{:016x}", SEQ.fetch_add(1, Ordering::Relaxed))
}

fn to_proto_paging_event(
    message: &PagingChannelMessage,
    mcsb: &MessageControlStatusBlock,
    cell: proto::CellId,
) -> proto::PagingEvent {
    let header = proto::PagingPduHeader {
        msg_tag: u32::from(
            mcsb.message_id
                .wire_type(WireChannel::ForwardCommon)
                .unwrap_or(0),
        ),
        msg_type_name: mcsb.message_id.name().to_string(),
        sdu_length_bits: mcsb.length_bits as u32,
        address: mcsb.address.as_ref().map(ms_address_to_proto),
        msg_seq: u32::from(mcsb.msg_seq),
        ack_seq: u32::from(mcsb.ack_seq),
        ack_req: mcsb.ack_req,
        valid_ack: mcsb.valid_ack,
        resolved_address: mcsb
            .address
            .as_ref()
            .map(format_ms_address)
            .unwrap_or_default(),
    };

    let body = match message {
        PagingChannelMessage::SystemParameters(m) => Some(
            proto::paging_event::Body::SystemParameters(proto::PagingSystemParameters {
                pilot_pn: u32::from(m.pilot_pn),
                sid: u32::from(m.sid),
                nid: u32::from(m.nid),
                base_id: u32::from(m.base_id),
                reg_zone: u32::from(m.reg_zone),
                total_zones: u32::from(m.total_zones),
                page_chan: u32::from(m.page_chan),
                max_slot_cycle_index: u32::from(m.max_slot_cycle_index),
                power_up_reg: m.power_up_reg,
                parameter_reg: m.parameter_reg,
            }),
        ),
        PagingChannelMessage::AccessParameters(m) => Some(
            proto::paging_event::Body::AccessParameters(proto::PagingAccessParameters {
                pilot_pn: u32::from(m.pilot_pn),
                acc_chan: u32::from(m.acc_chan),
                nom_pwr: i32::from(m.nom_pwr),
                init_pwr: i32::from(m.init_pwr),
                pwr_step: u32::from(m.pwr_step),
                num_step: u32::from(m.num_step),
                max_cap_sz: u32::from(m.max_cap_sz),
                auth: u32::from(m.auth),
            }),
        ),
        PagingChannelMessage::NeighborList(m) => Some(proto::paging_event::Body::NeighborList(
            proto::PagingNeighborList {
                pilot_pn: u32::from(m.pilot_pn),
                pilot_inc: u32::from(m.pilot_inc),
                neighbors: m.neighbors.iter().map(|n| u32::from(*n)).collect(),
            },
        )),
        PagingChannelMessage::CdmaChannelList(m) => Some(
            proto::paging_event::Body::CdmaChannelList(proto::PagingCdmaChannelList {
                pilot_pn: u32::from(m.pilot_pn),
                channels: m.channels.iter().map(|c| u32::from(*c)).collect(),
            }),
        ),
        PagingChannelMessage::ExtendedSystemParameters(m) => {
            Some(proto::paging_event::Body::ExtendedSystemParameters(
                proto::PagingExtendedSystemParameters {
                    pilot_pn: u32::from(m.pilot_pn),
                    p_rev: u32::from(m.p_rev),
                    min_p_rev: u32::from(m.min_p_rev),
                    mcc: u32::from(m.mcc),
                    imsi_11_12: u32::from(m.imsi_11_12),
                    use_tmsi: m.use_tmsi,
                    pref_msid_type: u32::from(m.pref_msid_type),
                    max_num_alt_so: u32::from(m.max_num_alt_so),
                    ext_pref_msid_type: m.ext_pref_msid_type.map(u32::from),
                    meid_reqd: m.meid_reqd,
                },
            ))
        }
        PagingChannelMessage::GeneralPage(m) => {
            let records = m
                .page_records
                .iter()
                .map(|r| {
                    let record = match r {
                        GeneralPageRecord::Class0 {
                            page_subclass,
                            msg_seq,
                            imsi_s,
                            imsi_m_s1,
                            imsi_m_s2,
                            mcc,
                            imsi_addr_num,
                            special_service,
                            service_option,
                            ..
                        } => proto::page_record::Record::Class0(proto::PageRecordClass0 {
                            page_subclass: u32::from(*page_subclass),
                            msg_seq: u32::from(*msg_seq),
                            imsi_m_s1: *imsi_m_s1,
                            imsi_m_s2: imsi_m_s2.map(u32::from),
                            mcc: mcc.map(u32::from),
                            imsi_addr_num: imsi_addr_num.map(u32::from),
                            special_service: *special_service,
                            service_option: service_option.map(u32::from),
                            imsi_s: *imsi_s,
                        }),
                        GeneralPageRecord::Class1 {
                            msg_seq,
                            esn,
                            special_service,
                            service_option,
                        } => proto::page_record::Record::Class1(proto::PageRecordClass1 {
                            msg_seq: u32::from(*msg_seq),
                            esn: *esn,
                            special_service: *special_service,
                            service_option: service_option.map(u32::from),
                        }),
                        GeneralPageRecord::Tmsi {
                            msg_seq,
                            tmsi_code_addr,
                            special_service,
                            service_option,
                        } => proto::page_record::Record::Tmsi(proto::PageRecordTmsi {
                            msg_seq: u32::from(*msg_seq),
                            tmsi_code_addr: *tmsi_code_addr,
                            special_service: *special_service,
                            service_option: service_option.map(u32::from),
                        }),
                        GeneralPageRecord::Broadcast { bc_addr } => {
                            proto::page_record::Record::Broadcast(proto::PageRecordBroadcast {
                                bc_addr: *bc_addr as u32,
                            })
                        }
                    };
                    proto::PageRecord {
                        record: Some(record),
                    }
                })
                .collect();

            Some(proto::paging_event::Body::GeneralPage(
                proto::PagingGeneralPage {
                    config_msg_seq: u32::from(m.config_msg_seq),
                    acc_msg_seq: u32::from(m.acc_msg_seq),
                    class_0_done: m.class_0_done,
                    class_1_done: m.class_1_done,
                    tmsi_done: m.tmsi_done,
                    page_records: records,
                },
            ))
        }
        PagingChannelMessage::Order(m) => {
            Some(proto::paging_event::Body::Order(proto::PagingOrder {
                order: u32::from(m.order),
                ordq: u32::from(m.ordq),
                order_name: forward_order_name(m.order).to_string(),
            }))
        }
        PagingChannelMessage::DataBurst(m) => {
            const SMS_BURST_TYPE: u8 = 3;
            const TL_MSG_TYPE_DELIVERY_ACK: u8 = 0x02;
            let decoded_sms = if m.burst_type == SMS_BURST_TYPE {
                cdma_common::sms::decode_mt_sms(&m.fields).map(|d| proto::DecodedSms {
                    teleservice_id: u32::from(d.teleservice_id),
                    destination_number: String::new(),
                    originating_number: d.originating_number,
                    message_type: u32::from(d.message_type),
                    message_id: u32::from(d.message_id),
                    text: if d.tl_msg_type == TL_MSG_TYPE_DELIVERY_ACK {
                        format!(
                            "Cause Code (reply_seq={}, error_class={})",
                            d.reply_seq.unwrap_or(0),
                            d.error_class.unwrap_or(0)
                        )
                    } else {
                        d.text
                    },
                    user_data: d.user_data,
                })
            } else {
                None
            };
            Some(proto::paging_event::Body::DataBurst(
                proto::PagingDataBurst {
                    burst_type: u32::from(m.burst_type),
                    msg_number: u32::from(m.msg_number),
                    num_msgs: u32::from(m.num_msgs),
                    payload_bytes: m.fields.len() as u32,
                    decoded_sms,
                },
            ))
        }
        PagingChannelMessage::ChannelAssignment(m) => {
            let effective_rcs = cam_effective_radio_config(m);
            let assign_mode_name = match m.assign_mode {
                CHANNEL_ASSIGN_MODE_TRAFFIC => "IS-95 Traffic",
                CHANNEL_ASSIGN_MODE_EXTENDED_TRAFFIC => "Extended Traffic (IS-2000)",
                _ => "Unknown",
            };
            let default_config_name = match m.default_config {
                Some(CHANNEL_DEFAULT_CONFIG_RC1_RC1) => "RC1/RC1",
                Some(CHANNEL_DEFAULT_CONFIG_RC2_RC2) => "RC2/RC2",
                Some(CHANNEL_DEFAULT_CONFIG_RC1_RC2) => "RC1/RC2",
                Some(CHANNEL_DEFAULT_CONFIG_RC2_RC1) => "RC2/RC1",
                Some(CHANNEL_DEFAULT_CONFIG_EXPLICIT_RCS) => "Explicit FOR_RC/REV_RC",
                _ => "",
            };
            Some(proto::paging_event::Body::ChannelAssignment(
                proto::PagingChannelAssignment {
                    assign_mode: u32::from(m.assign_mode),
                    code_chan: u32::from(m.code_chan),
                    frame_offset: u32::from(m.frame_offset),
                    encrypt_mode: u32::from(m.encrypt_mode),
                    freq_incl: m.freq_incl,
                    band_class: m.band_class.map(u32::from),
                    cdma_freq: m.cdma_freq.map(u32::from),
                    bypass_alert_answer: m.bypass_alert_answer,
                    default_config: m.default_config.map(u32::from),
                    granted_mode: m.granted_mode.map(u32::from),
                    assign_mode_name: assign_mode_name.to_string(),
                    default_config_name: default_config_name.to_string(),
                    direct_ch_assign_ind: None,
                    for_rc: effective_rcs.map(|(for_rc, _)| u32::from(for_rc)),
                    rev_rc: effective_rcs.map(|(_, rev_rc)| u32::from(rev_rc)),
                    fpc_subchan_gain: None,
                    rlgain_adj: None,
                    ch_ind: None,
                    ch_record_len_octets: None,
                    fpc_fch_init_setpt: None,
                    fpc_fch_fer: None,
                    fpc_fch_min_setpt: None,
                    fpc_fch_max_setpt: None,
                    rev_fch_gating_mode: None,
                    plcm_type: m.plcm_type.map(u32::from),
                    early_rl_transmit_ind: None,
                    tx_pwr_limit: None,
                    pilots: Vec::new(),
                    sdu_hex: Some(bitstream_to_hex(&m.to_sdu())),
                },
            ))
        }
        PagingChannelMessage::ExtendedChannelAssignment(m) => Some(
            proto::paging_event::Body::ChannelAssignment(proto::PagingChannelAssignment {
                assign_mode: u32::from(m.assign_mode),
                code_chan: m
                    .pilots
                    .first()
                    .map(|p| u32::from(p.code_chan_fch))
                    .unwrap_or_default(),
                frame_offset: u32::from(m.frame_offset),
                encrypt_mode: u32::from(m.encrypt_mode),
                freq_incl: m.freq_incl,
                band_class: m.band_class.map(u32::from),
                cdma_freq: m.cdma_freq.map(u32::from),
                bypass_alert_answer: Some(m.bypass_alert_answer),
                default_config: Some(u32::from(m.default_config)),
                granted_mode: Some(u32::from(m.granted_mode)),
                assign_mode_name: "ECAM".to_string(),
                default_config_name: if m.default_config == CHANNEL_DEFAULT_CONFIG_EXPLICIT_RCS {
                    "Explicit FOR_RC/REV_RC".to_string()
                } else {
                    String::new()
                },
                direct_ch_assign_ind: Some(m.direct_ch_assign_ind),
                for_rc: Some(u32::from(m.for_rc)),
                rev_rc: Some(u32::from(m.rev_rc)),
                fpc_subchan_gain: Some(u32::from(m.fpc_subchan_gain)),
                rlgain_adj: Some(i32::from(m.rlgain_adj)),
                ch_ind: Some(u32::from(m.ch_ind)),
                ch_record_len_octets: Some(m.ch_record_len_octets() as u32),
                fpc_fch_init_setpt: Some(u32::from(m.fpc_fch_init_setpt)),
                fpc_fch_fer: Some(u32::from(m.fpc_fch_fer)),
                fpc_fch_min_setpt: Some(u32::from(m.fpc_fch_min_setpt)),
                fpc_fch_max_setpt: Some(u32::from(m.fpc_fch_max_setpt)),
                rev_fch_gating_mode: Some(m.rev_fch_gating_mode),
                plcm_type: Some(u32::from(m.plcm_type)),
                early_rl_transmit_ind: Some(m.early_rl_transmit_ind),
                tx_pwr_limit: m.tx_pwr_limit.map(u32::from),
                pilots: m
                    .pilots
                    .iter()
                    .map(|pilot| proto::PagingTrafficPilot {
                        pilot_pn: u32::from(pilot.pilot_pn),
                        pwr_comb_ind: pilot.pwr_comb_ind,
                        code_chan_fch: u32::from(pilot.code_chan_fch),
                        qof_mask_id_fch: u32::from(pilot.qof_mask_id_fch),
                    })
                    .collect(),
                sdu_hex: Some(bitstream_to_hex(&m.to_sdu())),
            }),
        ),
        _ => None,
    };

    proto::PagingEvent {
        header: Some(header),
        timestamp_us: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap_or_default()
            .as_micros() as u64,
        event_id: next_pch_event_id(),
        body,
        cell: Some(cell),
    }
}
