//! The network management interface.
//!
//! `NetworkManagementService` is the single management endpoint the web dials.
//! It forwards per-base-station requests to that base station's management
//! gRPC, proxies the packet core (PCF/PDSN) directly, and fans out aggregation
//! requests across every base station. The whole surface lives here so it can
//! later move into a standalone management-plane gateway without changing the
//! wire contract.

use std::collections::HashMap;
use std::pin::Pin;

use cdma_rpc_error::ErrorReason;
use log::{info, warn};
use tokio_stream::{Stream, StreamExt, StreamMap};
use tonic::transport::Channel;
use tonic::{Request, Response, Status};

use crate::config::BaseStationConfig;
use crate::grpc::an::v1::an_service_client::AnServiceClient;
use crate::grpc::bsc::v1 as bsc;
use crate::grpc::bsc_management::v1::bsc_management_service_client::BscManagementServiceClient;
use crate::grpc::bts_management::v1 as bts_mgmt;
use crate::grpc::bts_management::v1::bts_management_service_client::BtsManagementServiceClient;
use crate::grpc::management::v1 as management;
use crate::grpc::management::v1::management_facade_service_client::ManagementFacadeServiceClient;
use crate::grpc::mgmt::v1 as mgmt;
use crate::grpc::mgmt::v1::network_management_service_server::NetworkManagementService;
use crate::grpc::packet::v1 as packet;
use crate::grpc::packet::v1::packet_service_client::PacketServiceClient;
use crate::grpc::pcf::v1 as pcf;
use crate::grpc::pcf::v1::pcf_service_client::PcfServiceClient;
use crate::grpc::pcf_management::v1 as pcf_mgmt;
use crate::grpc::pdsn_management::v1 as pdsn_mgmt;
use crate::management::MSC;

type BoxStream<T> = Pin<Box<dyn Stream<Item = Result<T, Status>> + Send>>;

/// Forwards management traffic to base stations and the packet core.
#[derive(Clone)]
pub struct NetworkManagement {
    /// Base-station id to a lazily-connected channel to its management gRPC.
    base_stations: HashMap<String, Channel>,
    /// Base-station ids in config order, for aggregation fan-out.
    order: Vec<String>,
    /// Lazily-connected channel to the packet core (PDSN) gRPC.
    packet: Channel,
    pcf: Channel,
    control: Option<tokio::sync::mpsc::Sender<crate::management::PendingControlRequest>>,
}

impl NetworkManagement {
    /// Builds the interface. Channels connect lazily, so a base station that
    /// is down refuses per call rather than blocking here.
    pub fn new(
        base_stations_config: &[BaseStationConfig],
        packet_endpoint: &str,
        pcf_endpoint: &str,
    ) -> Self {
        let mut base_stations = HashMap::new();
        let mut order = Vec::new();
        for node in base_stations_config {
            let id = node.id().to_string();
            match Channel::from_shared(node.management_endpoint.clone()) {
                Ok(endpoint) => {
                    let channel = endpoint.connect_lazy();
                    // A selector may name the base station by its id or its
                    // management endpoint, so both resolve to the same channel.
                    if node.management_endpoint != id {
                        base_stations.insert(node.management_endpoint.clone(), channel.clone());
                    }
                    base_stations.insert(id.clone(), channel);
                    order.push(id);
                }
                Err(e) => warn!(
                    "MSC management: base station {id} endpoint {} is invalid: {e}",
                    node.management_endpoint
                ),
            }
        }
        let packet = Channel::from_shared(packet_endpoint.to_string())
            .unwrap_or_else(|e| panic!("msc.packet_endpoint is invalid: {e}"))
            .connect_lazy();
        let pcf = Channel::from_shared(pcf_endpoint.to_string())
            .unwrap_or_else(|e| panic!("msc.pcf_endpoint is invalid: {e}"))
            .connect_lazy();
        Self {
            base_stations,
            order,
            packet,
            pcf,
            control: None,
        }
    }

    pub(crate) fn with_control(
        mut self,
        control: tokio::sync::mpsc::Sender<crate::management::PendingControlRequest>,
    ) -> Self {
        self.control = Some(control);
        self
    }

    /// The channel to the base station a selector names. An absent or empty
    /// selector resolves to the only base station when there is exactly one.
    fn channel(&self, selector: &Option<mgmt::BsSelector>) -> Result<Channel, Status> {
        let id = selector
            .as_ref()
            .map(|s| s.base_station.as_str())
            .unwrap_or("");
        if id.is_empty() {
            if self.order.len() == 1 {
                return Ok(self.base_stations[&self.order[0]].clone());
            }
            return Err(MSC
                .error(
                    tonic::Code::InvalidArgument,
                    ErrorReason::BaseStationRequired,
                    "More than one base station is configured, so choose one",
                )
                .status());
        }
        self.base_stations.get(id).cloned().ok_or_else(|| {
            MSC.not_found(
                ErrorReason::BaseStationNotFound,
                format!("Base station {id} is not configured"),
            )
            .with("base_station", id)
            .status()
        })
    }

    fn base_station_error(&self, status: Status, selector: &Option<mgmt::BsSelector>) -> Status {
        if cdma_rpc_error::error_info(&status).is_some() || !is_unreachable(&status) {
            return status;
        }
        let id = selector
            .as_ref()
            .map(|s| s.base_station.clone())
            .filter(|id| !id.is_empty())
            .or_else(|| (self.order.len() == 1).then(|| self.order[0].clone()))
            .unwrap_or_default();
        warn!("MSC management: base station {id} unreachable: {status}");
        MSC.unavailable(
            ErrorReason::BaseStationUnreachable,
            format!("Base station {id} could not be reached"),
        )
        .with("base_station", id)
        .status()
    }

    fn bts(
        &self,
        selector: &Option<mgmt::BsSelector>,
    ) -> Result<BtsManagementServiceClient<Channel>, Status> {
        Ok(BtsManagementServiceClient::new(self.channel(selector)?))
    }

    fn bsc_mgmt(
        &self,
        selector: &Option<mgmt::BsSelector>,
    ) -> Result<BscManagementServiceClient<Channel>, Status> {
        Ok(BscManagementServiceClient::new(self.channel(selector)?))
    }

    fn packet_client(&self) -> PacketServiceClient<Channel> {
        PacketServiceClient::new(self.packet.clone())
    }
}

fn cell_request(request: Option<bts_mgmt::CellRequest>) -> bts_mgmt::CellRequest {
    request.unwrap_or_default()
}

#[tonic::async_trait]
impl NetworkManagementService for NetworkManagement {
    type StreamRadioMetricsStream = BoxStream<bsc::RadioMetrics>;
    type StreamPchTransmissionsStream = BoxStream<bsc::PagingEvent>;
    type StreamAccessEventsStream = BoxStream<bsc::AccessEvent>;
    type StreamPagingEventsStream = BoxStream<bsc::PagingEvent>;
    type StreamTrafficEventsStream = BoxStream<bsc::TrafficEvent>;
    type StreamSystemEventsStream = BoxStream<management::ManagementEvent>;

    // Per-selector proxy: BTS management

    async fn list_bts(
        &self,
        request: Request<mgmt::BsRequest>,
    ) -> Result<Response<bts_mgmt::BtsList>, Status> {
        let selector = request.into_inner().selector;
        self.bts(&selector)?
            .list_bts(())
            .await
            .map_err(|status| self.base_station_error(status, &selector))
    }

    async fn get_bts_config(
        &self,
        request: Request<mgmt::CellScopedRequest>,
    ) -> Result<Response<bsc::BtsConfig>, Status> {
        let req = request.into_inner();
        self.bts(&req.selector)?
            .get_bts_config(cell_request(req.request))
            .await
            .map_err(|status| self.base_station_error(status, &req.selector))
    }

    async fn get_radio_metrics(
        &self,
        request: Request<mgmt::CellScopedRequest>,
    ) -> Result<Response<bsc::RadioMetrics>, Status> {
        let req = request.into_inner();
        self.bts(&req.selector)?
            .get_radio_metrics(cell_request(req.request))
            .await
            .map_err(|status| self.base_station_error(status, &req.selector))
    }

    async fn stream_radio_metrics(
        &self,
        request: Request<mgmt::CellScopedRequest>,
    ) -> Result<Response<Self::StreamRadioMetricsStream>, Status> {
        let req = request.into_inner();
        let upstream = self
            .bts(&req.selector)?
            .stream_radio_metrics(cell_request(req.request))
            .await
            .map_err(|status| self.base_station_error(status, &req.selector))?
            .into_inner();
        Ok(Response::new(Box::pin(upstream)))
    }

    async fn get_iq_capture_status(
        &self,
        request: Request<mgmt::CellScopedRequest>,
    ) -> Result<Response<bsc::IqCaptureStatus>, Status> {
        let req = request.into_inner();
        self.bts(&req.selector)?
            .get_iq_capture_status(cell_request(req.request))
            .await
            .map_err(|status| self.base_station_error(status, &req.selector))
    }

    async fn start_iq_capture(
        &self,
        request: Request<mgmt::CellScopedRequest>,
    ) -> Result<Response<bsc::IqCaptureStatus>, Status> {
        let req = request.into_inner();
        self.bts(&req.selector)?
            .start_iq_capture(cell_request(req.request))
            .await
            .map_err(|status| self.base_station_error(status, &req.selector))
    }

    async fn stop_iq_capture(
        &self,
        request: Request<mgmt::CellScopedRequest>,
    ) -> Result<Response<bsc::IqCaptureStatus>, Status> {
        let req = request.into_inner();
        self.bts(&req.selector)?
            .stop_iq_capture(cell_request(req.request))
            .await
            .map_err(|status| self.base_station_error(status, &req.selector))
    }

    async fn list_local_radio_resources(
        &self,
        request: Request<mgmt::CellScopedRequest>,
    ) -> Result<Response<bsc::ChannelList>, Status> {
        let req = request.into_inner();
        self.bts(&req.selector)?
            .list_local_radio_resources(cell_request(req.request))
            .await
            .map_err(|status| self.base_station_error(status, &req.selector))
    }

    async fn get_reverse_power_control(
        &self,
        request: Request<mgmt::ReversePowerControlScopedRequest>,
    ) -> Result<Response<bsc::TrafficChannelPower>, Status> {
        let req = request.into_inner();
        let inner = req.request.unwrap_or_default();
        self.bts(&req.selector)?
            .get_reverse_power_control(inner)
            .await
            .map_err(|status| self.base_station_error(status, &req.selector))
    }

    async fn list_reverse_power_controls(
        &self,
        request: Request<mgmt::CellScopedRequest>,
    ) -> Result<Response<bts_mgmt::ReversePowerControlList>, Status> {
        let req = request.into_inner();
        self.bts(&req.selector)?
            .list_reverse_power_controls(cell_request(req.request))
            .await
            .map_err(|status| self.base_station_error(status, &req.selector))
    }

    async fn set_reverse_power_control_override(
        &self,
        request: Request<mgmt::PowerOverrideScopedRequest>,
    ) -> Result<Response<bsc::SetTrafficChannelPowerOverrideResponse>, Status> {
        let req = request.into_inner();
        let inner = req.request.unwrap_or_default();
        self.bts(&req.selector)?
            .set_reverse_power_control_override(inner)
            .await
            .map_err(|status| self.base_station_error(status, &req.selector))
    }

    async fn stream_pch_transmissions(
        &self,
        request: Request<mgmt::CellScopedRequest>,
    ) -> Result<Response<Self::StreamPchTransmissionsStream>, Status> {
        let req = request.into_inner();
        let upstream = self
            .bts(&req.selector)?
            .stream_pch_transmissions(cell_request(req.request))
            .await
            .map_err(|status| self.base_station_error(status, &req.selector))?
            .into_inner();
        Ok(Response::new(Box::pin(upstream)))
    }

    // Per-selector proxy: BSC management

    async fn list_mobiles(
        &self,
        request: Request<mgmt::BsRequest>,
    ) -> Result<Response<bsc::MobileList>, Status> {
        let selector = request.into_inner().selector;
        self.bsc_mgmt(&selector)?
            .list_mobiles(())
            .await
            .map_err(|status| self.base_station_error(status, &selector))
    }

    async fn list_channels(
        &self,
        request: Request<mgmt::BsRequest>,
    ) -> Result<Response<bsc::ChannelList>, Status> {
        let selector = request.into_inner().selector;
        self.bsc_mgmt(&selector)?
            .list_channels(())
            .await
            .map_err(|status| self.base_station_error(status, &selector))
    }

    async fn stream_access_events(
        &self,
        request: Request<mgmt::BsRequest>,
    ) -> Result<Response<Self::StreamAccessEventsStream>, Status> {
        let selector = request.into_inner().selector;
        let upstream = self
            .bsc_mgmt(&selector)?
            .stream_access_events(())
            .await
            .map_err(|status| self.base_station_error(status, &selector))?
            .into_inner();
        Ok(Response::new(Box::pin(upstream)))
    }

    async fn stream_paging_events(
        &self,
        request: Request<mgmt::BsRequest>,
    ) -> Result<Response<Self::StreamPagingEventsStream>, Status> {
        let selector = request.into_inner().selector;
        let upstream = self
            .bsc_mgmt(&selector)?
            .stream_paging_events(())
            .await
            .map_err(|status| self.base_station_error(status, &selector))?
            .into_inner();
        Ok(Response::new(Box::pin(upstream)))
    }

    async fn stream_traffic_events(
        &self,
        request: Request<mgmt::BsRequest>,
    ) -> Result<Response<Self::StreamTrafficEventsStream>, Status> {
        let selector = request.into_inner().selector;
        let upstream = self
            .bsc_mgmt(&selector)?
            .stream_traffic_events(())
            .await
            .map_err(|status| self.base_station_error(status, &selector))?
            .into_inner();
        Ok(Response::new(Box::pin(upstream)))
    }

    async fn set_traffic_channel_power_override(
        &self,
        request: Request<mgmt::PowerOverrideScopedRequest>,
    ) -> Result<Response<bsc::SetTrafficChannelPowerOverrideResponse>, Status> {
        let req = request.into_inner();
        let inner = req.request.unwrap_or_default();
        self.bsc_mgmt(&req.selector)?
            .set_traffic_channel_power_override(inner)
            .await
            .map_err(|status| self.base_station_error(status, &req.selector))
    }

    // Per-selector: data-call setup

    async fn initiate_data_call(
        &self,
        request: Request<mgmt::InitiateDataCallScopedRequest>,
    ) -> Result<Response<bsc::InitiateDataCallResponse>, Status> {
        let req = request.into_inner();
        self.channel(&req.selector)?;
        let inner = req
            .request
            .ok_or_else(|| MSC.invalid_field("request", "A data call request is required"))?;
        let subscriber_id = uuid::Uuid::parse_str(&inner.subscriber_id)
            .map_err(|_| MSC.invalid_field("subscriber_id", "The subscriber ID is not valid"))?;
        let service_option = if inner.service_option == 0 {
            cdma_common::consts::SERVICE_OPTION_PACKET_DATA_RC3
        } else {
            u16::try_from(inner.service_option).map_err(|_| {
                MSC.invalid_field("service_option", "The service option must be 65535 or less")
            })?
        };
        let control = self.control.as_ref().ok_or_else(|| MSC.stopping())?;
        let (response_tx, response_rx) = tokio::sync::oneshot::channel();
        control
            .send(crate::management::PendingControlRequest::InitiateDataCall {
                subscriber_id,
                service_option,
                response_tx,
            })
            .await
            .map_err(|_| MSC.stopping())?;
        let result = response_rx
            .await
            .map_err(|_| MSC.stopping())?
            .map_err(crate::management::ManagementError::into_status)?;
        Ok(Response::new(bsc::InitiateDataCallResponse {
            accepted: true,
            message: format!("call_id={}", result.call_id.0),
        }))
    }

    // Per-selector proxy: HRPD access network (routed behind the BSC)

    async fn get_an_sessions(
        &self,
        request: Request<mgmt::AnSessionsScopedRequest>,
    ) -> Result<Response<crate::grpc::an::v1::GetSessionsResponse>, Status> {
        let req = request.into_inner();
        let addr = self.an_addr(&req.selector, req.cell).await?;
        an_client(&addr)
            .await?
            .get_sessions(req.request.unwrap_or_default())
            .await
            .map_err(|status| downstream_error(status, Downstream::AccessNetwork))
    }

    async fn get_an_session(
        &self,
        request: Request<mgmt::AnSessionScopedRequest>,
    ) -> Result<Response<crate::grpc::an::v1::GetSessionResponse>, Status> {
        let req = request.into_inner();
        let addr = self.an_addr(&req.selector, req.cell).await?;
        an_client(&addr)
            .await?
            .get_session(req.request.unwrap_or_default())
            .await
            .map_err(|status| downstream_error(status, Downstream::AccessNetwork))
    }

    async fn get_an_uati_allocation(
        &self,
        request: Request<mgmt::AnUatiAllocationScopedRequest>,
    ) -> Result<Response<crate::grpc::an::v1::GetUatiAllocationResponse>, Status> {
        let req = request.into_inner();
        let addr = self.an_addr(&req.selector, req.cell).await?;
        an_client(&addr)
            .await?
            .get_uati_allocation(req.request.unwrap_or_default())
            .await
            .map_err(|status| downstream_error(status, Downstream::AccessNetwork))
    }

    async fn get_an_session_events(
        &self,
        request: Request<mgmt::AnSessionEventsScopedRequest>,
    ) -> Result<Response<crate::grpc::an::v1::GetSessionEventsResponse>, Status> {
        let req = request.into_inner();
        let addr = self.an_addr(&req.selector, req.cell).await?;
        an_client(&addr)
            .await?
            .get_session_events(req.request.unwrap_or_default())
            .await
            .map_err(|status| downstream_error(status, Downstream::AccessNetwork))
    }

    // Global interface: PCF

    async fn list_pcf_sessions(
        &self,
        _: Request<()>,
    ) -> Result<Response<pcf_mgmt::PcfSessionList>, Status> {
        let sessions = self
            .packet_client()
            .list_sessions(packet::ListSessionsRequest {})
            .await
            .map_err(|status| downstream_error(status, Downstream::Packet))?
            .into_inner()
            .sessions;
        Ok(Response::new(pcf_mgmt::PcfSessionList { sessions }))
    }

    async fn list_pcf_a10_sessions(
        &self,
        _: Request<()>,
    ) -> Result<Response<pcf::ListA10SessionsResponse>, Status> {
        PcfServiceClient::new(self.pcf.clone())
            .list_a10_sessions(pcf::ListA10SessionsRequest {})
            .await
            .map_err(|status| downstream_error(status, Downstream::Pcf))
    }

    async fn get_pcf_session(
        &self,
        request: Request<pcf_mgmt::GetPcfSessionRequest>,
    ) -> Result<Response<packet::GetSessionStatusResponse>, Status> {
        let session_id = request.into_inner().session_id;
        self.packet_client()
            .get_session_status(packet::GetSessionStatusRequest { session_id })
            .await
            .map_err(|status| downstream_error(status, Downstream::Packet))
    }

    // Global interface: PDSN

    async fn list_pdsn_sessions(
        &self,
        _: Request<()>,
    ) -> Result<Response<pdsn_mgmt::PdsnSessionList>, Status> {
        let sessions = self
            .packet_client()
            .list_sessions(packet::ListSessionsRequest {})
            .await
            .map_err(|status| downstream_error(status, Downstream::Packet))?
            .into_inner()
            .sessions;
        Ok(Response::new(pdsn_mgmt::PdsnSessionList { sessions }))
    }

    async fn get_pdsn_session(
        &self,
        request: Request<pdsn_mgmt::GetPdsnSessionRequest>,
    ) -> Result<Response<packet::GetSessionStatusResponse>, Status> {
        let session_id = request.into_inner().session_id;
        self.packet_client()
            .get_session_status(packet::GetSessionStatusRequest { session_id })
            .await
            .map_err(|status| downstream_error(status, Downstream::Packet))
    }

    async fn get_pdsn_session_by_ip(
        &self,
        request: Request<pdsn_mgmt::GetPdsnSessionByIpRequest>,
    ) -> Result<Response<packet::GetSessionByIpResponse>, Status> {
        let peer_ip = request.into_inner().peer_ip;
        self.packet_client()
            .get_session_by_ip(packet::GetSessionByIpRequest { peer_ip })
            .await
            .map_err(|status| downstream_error(status, Downstream::Packet))
    }

    async fn set_packet_trace_capture(
        &self,
        request: Request<pdsn_mgmt::SetPacketTraceCaptureRequest>,
    ) -> Result<Response<packet::SetSessionCaptureResponse>, Status> {
        let req = request.into_inner();
        self.packet_client()
            .set_session_capture(packet::SetSessionCaptureRequest {
                session_id: req.session_id,
                enabled: req.enabled,
            })
            .await
            .map_err(|status| downstream_error(status, Downstream::Packet))
    }

    // Aggregation across every base station

    async fn list_all_mobiles(&self, _: Request<()>) -> Result<Response<bsc::MobileList>, Status> {
        let mut mobiles = Vec::new();
        for id in &self.order {
            let mut client = BscManagementServiceClient::new(self.base_stations[id].clone());
            if let Ok(list) = client.list_mobiles(()).await {
                mobiles.extend(list.into_inner().mobiles);
            }
        }
        Ok(Response::new(bsc::MobileList { mobiles }))
    }

    async fn list_all_channels(
        &self,
        _: Request<()>,
    ) -> Result<Response<bsc::ChannelList>, Status> {
        let mut merged = bsc::ChannelList::default();
        for id in &self.order {
            let mut client = BscManagementServiceClient::new(self.base_stations[id].clone());
            if let Ok(list) = client.list_channels(()).await {
                let list = list.into_inner();
                merged.channels.extend(list.channels);
                merged.total_walsh_codes += list.total_walsh_codes;
                merged.cell_walsh_capacity.extend(list.cell_walsh_capacity);
            }
        }
        Ok(Response::new(merged))
    }

    async fn stream_system_events(
        &self,
        _: Request<()>,
    ) -> Result<Response<Self::StreamSystemEventsStream>, Status> {
        let mut streams = StreamMap::new();
        for id in &self.order {
            let mut client = ManagementFacadeServiceClient::new(self.base_stations[id].clone());
            match client.stream_system_events(()).await {
                Ok(response) => {
                    streams.insert(id.clone(), response.into_inner());
                }
                Err(e) => warn!("MSC management: base station {id} event stream unavailable: {e}"),
            }
        }
        let merged = streams.map(|(_, event)| event);
        Ok(Response::new(Box::pin(merged)))
    }
}

impl NetworkManagement {
    /// An access network that cannot be reached keeps its current mode, which
    /// is closed until it is first set.
    pub async fn push_hrpd_access_mode(&self, closed: bool) {
        use crate::grpc::an::v1::{AnAccessMode, SetAccessPolicyRequest};
        let mode = if closed {
            AnAccessMode::Closed
        } else {
            AnAccessMode::Open
        };
        for id in &self.order {
            let mut bts = BtsManagementServiceClient::new(self.base_stations[id].clone());
            let cells = match bts.list_bts(()).await {
                Ok(list) => list.into_inner().bts,
                Err(error) => {
                    log::debug!("MSC: cannot list cells on base station {id}: {error}");
                    continue;
                }
            };
            for cell in cells {
                let Some(addr) = cell.an_grpc_addr.filter(|addr| !addr.is_empty()) else {
                    continue;
                };
                let result = match an_client(&addr).await {
                    Ok(mut client) => client
                        .set_access_policy(SetAccessPolicyRequest { mode: mode as i32 })
                        .await
                        .map(|response| response.into_inner().changed),
                    Err(error) => Err(error),
                };
                match result {
                    Ok(true) => info!(
                        "MSC: HRPD access network {addr} on base station {id} is now {}",
                        if closed { "closed" } else { "open" }
                    ),
                    Ok(false) => {}
                    Err(error) => log::debug!(
                        "MSC: cannot set access mode on HRPD access network {addr}: {error}"
                    ),
                }
            }
        }
    }

    /// Resolves the HRPD access-network gRPC address for one cell of one base
    /// station by asking the base station for that cell's summary.
    async fn an_addr(
        &self,
        selector: &Option<mgmt::BsSelector>,
        cell: Option<bts_mgmt::CellRequest>,
    ) -> Result<String, Status> {
        let cell = cell_request(cell);
        let list = self.bts(selector)?.list_bts(()).await?.into_inner();
        let want_peer = cell.peer_id.clone();
        let want_cell = cell.cell.clone();
        let summary = list
            .bts
            .into_iter()
            .find(|s| match (&want_peer, &want_cell) {
                (Some(p), _) => &s.peer_id == p,
                (None, Some(c)) => s.cell.as_ref() == Some(c),
                (None, None) => true,
            })
            .ok_or_else(|| {
                MSC.not_found(
                    ErrorReason::CellNotFound,
                    "The cell is not on this base station",
                )
                .status()
            })?;
        summary
            .an_grpc_addr
            .filter(|a| !a.is_empty())
            .ok_or_else(|| {
                MSC.precondition(
                    ErrorReason::EvdoNotEnabled,
                    "EV-DO is not enabled on this cell",
                )
                .status()
            })
    }
}

async fn an_client(addr: &str) -> Result<AnServiceClient<Channel>, Status> {
    let endpoint = normalize_grpc_uri(addr);
    let channel = Channel::from_shared(endpoint)
        .map_err(|e| {
            MSC.internal(format!(
                "The EV-DO access network address {addr} is not valid: {e}"
            ))
        })?
        .connect_lazy();
    Ok(AnServiceClient::new(channel))
}

/// A bare `host:port` from a cell summary becomes an `http://` URI so tonic can
/// dial it.
fn normalize_grpc_uri(addr: &str) -> String {
    if addr.contains("://") {
        addr.to_string()
    } else {
        format!("http://{addr}")
    }
}

#[derive(Clone, Copy, Debug)]
enum Downstream {
    AccessNetwork,
    Packet,
    Pcf,
}

impl Downstream {
    fn name(self) -> &'static str {
        match self {
            Self::AccessNetwork => "an",
            Self::Packet => "packet",
            Self::Pcf => "pcf",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::AccessNetwork => "EV-DO access network",
            Self::Packet => "packet data service",
            Self::Pcf => "PCF",
        }
    }
}

fn is_unreachable(status: &Status) -> bool {
    matches!(
        status.code(),
        tonic::Code::Unavailable | tonic::Code::DeadlineExceeded
    )
}

fn downstream_error(status: Status, downstream: Downstream) -> Status {
    if cdma_rpc_error::error_info(&status).is_some() || !is_unreachable(&status) {
        return status;
    }
    warn!(
        "MSC management: {} unreachable: {status}",
        downstream.label()
    );
    MSC.unavailable(
        ErrorReason::ServiceUnreachable,
        format!("The {} could not be reached", downstream.label()),
    )
    .with("service", downstream.name())
    .status()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn selector(id: &str) -> Option<mgmt::BsSelector> {
        Some(mgmt::BsSelector {
            base_station: id.to_string(),
        })
    }

    #[tokio::test]
    async fn a_selector_resolves_by_id_or_by_management_endpoint() {
        let net = NetworkManagement::new(
            &[BaseStationConfig {
                management_endpoint: "http://127.0.0.1:17016".to_string(),
                id: Some("bts-1".to_string()),
                a1p: cdma_ios::transport::A1pConfig::for_role(cdma_ios::transport::A1Role::Msc),
            }],
            "http://127.0.0.1:17021",
            "http://127.0.0.1:17047",
        );
        assert!(net.channel(&selector("bts-1")).is_ok());
        assert!(net.channel(&selector("http://127.0.0.1:17016")).is_ok());
        assert!(net.channel(&selector("nope")).is_err());
    }

    #[tokio::test]
    async fn an_empty_selector_resolves_only_when_one_base_station_is_configured() {
        let one = NetworkManagement::new(
            &[BaseStationConfig {
                management_endpoint: "http://127.0.0.1:17016".to_string(),
                id: None,
                a1p: cdma_ios::transport::A1pConfig::for_role(cdma_ios::transport::A1Role::Msc),
            }],
            "http://127.0.0.1:17021",
            "http://127.0.0.1:17047",
        );
        assert!(one.channel(&None).is_ok());

        let two = NetworkManagement::new(
            &[
                BaseStationConfig {
                    management_endpoint: "http://127.0.0.1:17016".to_string(),
                    id: None,
                    a1p: cdma_ios::transport::A1pConfig::for_role(cdma_ios::transport::A1Role::Msc),
                },
                BaseStationConfig {
                    management_endpoint: "http://127.0.0.1:17026".to_string(),
                    id: None,
                    a1p: cdma_ios::transport::A1pConfig::for_role(cdma_ios::transport::A1Role::Msc),
                },
            ],
            "http://127.0.0.1:17021",
            "http://127.0.0.1:17047",
        );
        assert!(two.channel(&None).is_err());
    }
    #[tokio::test]
    async fn data_call_management_is_executed_by_the_msc_runtime() {
        let (control, mut requests) = tokio::sync::mpsc::channel(1);
        let net = NetworkManagement::new(
            &[BaseStationConfig {
                management_endpoint: "http://127.0.0.1:1".into(),
                id: None,
                a1p: cdma_ios::transport::A1pConfig::for_role(cdma_ios::transport::A1Role::Msc),
            }],
            "http://127.0.0.1:1",
            "http://127.0.0.1:17047",
        )
        .with_control(control);
        let subscriber_id = uuid::Uuid::new_v4();
        let task = tokio::spawn(async move {
            net.initiate_data_call(Request::new(mgmt::InitiateDataCallScopedRequest {
                selector: None,
                request: Some(bsc::InitiateDataCallRequest {
                    subscriber_id: subscriber_id.to_string(),
                    service_option: 0,
                }),
            }))
            .await
        });
        match requests.recv().await.unwrap() {
            crate::management::PendingControlRequest::InitiateDataCall {
                subscriber_id: received,
                service_option,
                response_tx,
            } => {
                assert_eq!(received, subscriber_id);
                assert_eq!(
                    service_option,
                    cdma_common::consts::SERVICE_OPTION_PACKET_DATA_RC3
                );
                response_tx
                    .send(Ok(crate::management::InitiateCallAccepted {
                        call_id: crate::call_control::CallId(1),
                    }))
                    .unwrap();
            }
            other => panic!("unexpected control request: {other:?}"),
        }
        assert!(task.await.unwrap().unwrap().into_inner().accepted);
    }

    fn reason(status: &Status) -> Option<ErrorReason> {
        cdma_rpc_error::error_info(status).map(|info| info.reason)
    }

    #[tokio::test]
    async fn an_unknown_base_station_names_itself() {
        let net = NetworkManagement::new(
            &[BaseStationConfig {
                management_endpoint: "http://127.0.0.1:17016".to_string(),
                id: Some("bts-1".to_string()),
                a1p: cdma_ios::transport::A1pConfig::for_role(cdma_ios::transport::A1Role::Msc),
            }],
            "http://127.0.0.1:17021",
            "http://127.0.0.1:17047",
        );
        let status = net.channel(&selector("nope")).unwrap_err();
        assert_eq!(status.code(), tonic::Code::NotFound);
        let info = cdma_rpc_error::error_info(&status).expect("error info");
        assert_eq!(info.reason, ErrorReason::BaseStationNotFound);
        assert_eq!(info.metadata["base_station"], "nope");
    }

    #[tokio::test]
    async fn an_unreachable_base_station_is_named_in_the_forwarded_error() {
        let net = NetworkManagement::new(
            &[BaseStationConfig {
                management_endpoint: "http://127.0.0.1:1".to_string(),
                id: Some("bts-1".to_string()),
                a1p: cdma_ios::transport::A1pConfig::for_role(cdma_ios::transport::A1Role::Msc),
            }],
            "http://127.0.0.1:1",
            "http://127.0.0.1:1",
        );
        let status = net
            .list_bts(Request::new(mgmt::BsRequest { selector: None }))
            .await
            .unwrap_err();
        assert_eq!(status.code(), tonic::Code::Unavailable);
        let info = cdma_rpc_error::error_info(&status).expect("error info");
        assert_eq!(info.reason, ErrorReason::BaseStationUnreachable);
        assert_eq!(info.metadata["base_station"], "bts-1");
    }

    #[test]
    fn a_forwarded_error_with_a_reason_passes_through() {
        let original = cdma_rpc_error::Domain("packet")
            .not_found(ErrorReason::PacketSessionNotFound, "Session not found")
            .status();
        let forwarded = downstream_error(original, Downstream::Packet);
        assert_eq!(reason(&forwarded), Some(ErrorReason::PacketSessionNotFound));
        let unreachable = downstream_error(Status::unavailable("transport error"), Downstream::Pcf);
        assert_eq!(reason(&unreachable), Some(ErrorReason::ServiceUnreachable));
        let other = downstream_error(Status::internal("boom"), Downstream::Pcf);
        assert_eq!(reason(&other), None);
    }
}
