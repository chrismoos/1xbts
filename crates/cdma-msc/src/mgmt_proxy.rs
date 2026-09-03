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

use log::warn;
use tokio_stream::{Stream, StreamExt, StreamMap};
use tonic::transport::Channel;
use tonic::{Request, Response, Status};

use crate::config::BaseStationConfig;
use crate::grpc::an::v1::an_service_client::AnServiceClient;
use crate::grpc::bsc::v1 as bsc;
use crate::grpc::bsc::v1::bsc_service_client::BscServiceClient;
use crate::grpc::bsc_management::v1::bsc_management_service_client::BscManagementServiceClient;
use crate::grpc::bts_management::v1 as bts_mgmt;
use crate::grpc::bts_management::v1::bts_management_service_client::BtsManagementServiceClient;
use crate::grpc::management::v1 as management;
use crate::grpc::management::v1::management_facade_service_client::ManagementFacadeServiceClient;
use crate::grpc::mgmt::v1 as mgmt;
use crate::grpc::mgmt::v1::network_management_service_server::NetworkManagementService;
use crate::grpc::packet::v1 as packet;
use crate::grpc::packet::v1::packet_service_client::PacketServiceClient;
use crate::grpc::pcf_management::v1 as pcf_mgmt;
use crate::grpc::pdsn_management::v1 as pdsn_mgmt;

type BoxStream<T> = Pin<Box<dyn Stream<Item = Result<T, Status>> + Send>>;

/// Forwards management traffic to base stations and the packet core.
pub struct NetworkManagement {
    /// Base-station id to a lazily-connected channel to its management gRPC.
    base_stations: HashMap<String, Channel>,
    /// Base-station ids in config order, for aggregation fan-out.
    order: Vec<String>,
    /// Lazily-connected channel to the packet core (PDSN) gRPC.
    packet: Channel,
}

impl NetworkManagement {
    /// Builds the interface. Channels connect lazily, so a base station that
    /// is down refuses per call rather than blocking here.
    pub fn new(base_stations_config: &[BaseStationConfig], packet_endpoint: &str) -> Self {
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
        Self {
            base_stations,
            order,
            packet,
        }
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
            return Err(Status::invalid_argument(
                "base_station selector is required when more than one is configured",
            ));
        }
        self.base_stations
            .get(id)
            .cloned()
            .ok_or_else(|| Status::not_found(format!("unknown base station {id}")))
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
        self.bts(&selector)?.list_bts(()).await
    }

    async fn get_bts_status(
        &self,
        request: Request<mgmt::CellScopedRequest>,
    ) -> Result<Response<bsc::SystemStatus>, Status> {
        let req = request.into_inner();
        self.bts(&req.selector)?
            .get_bts_status(cell_request(req.request))
            .await
    }

    async fn get_bts_config(
        &self,
        request: Request<mgmt::CellScopedRequest>,
    ) -> Result<Response<bsc::BtsConfig>, Status> {
        let req = request.into_inner();
        self.bts(&req.selector)?
            .get_bts_config(cell_request(req.request))
            .await
    }

    async fn get_radio_metrics(
        &self,
        request: Request<mgmt::CellScopedRequest>,
    ) -> Result<Response<bsc::RadioMetrics>, Status> {
        let req = request.into_inner();
        self.bts(&req.selector)?
            .get_radio_metrics(cell_request(req.request))
            .await
    }

    async fn stream_radio_metrics(
        &self,
        request: Request<mgmt::CellScopedRequest>,
    ) -> Result<Response<Self::StreamRadioMetricsStream>, Status> {
        let req = request.into_inner();
        let upstream = self
            .bts(&req.selector)?
            .stream_radio_metrics(cell_request(req.request))
            .await?
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
    }

    async fn start_iq_capture(
        &self,
        request: Request<mgmt::CellScopedRequest>,
    ) -> Result<Response<bsc::IqCaptureStatus>, Status> {
        let req = request.into_inner();
        self.bts(&req.selector)?
            .start_iq_capture(cell_request(req.request))
            .await
    }

    async fn stop_iq_capture(
        &self,
        request: Request<mgmt::CellScopedRequest>,
    ) -> Result<Response<bsc::IqCaptureStatus>, Status> {
        let req = request.into_inner();
        self.bts(&req.selector)?
            .stop_iq_capture(cell_request(req.request))
            .await
    }

    async fn list_local_radio_resources(
        &self,
        request: Request<mgmt::CellScopedRequest>,
    ) -> Result<Response<bsc::ChannelList>, Status> {
        let req = request.into_inner();
        self.bts(&req.selector)?
            .list_local_radio_resources(cell_request(req.request))
            .await
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
    }

    async fn list_reverse_power_controls(
        &self,
        request: Request<mgmt::CellScopedRequest>,
    ) -> Result<Response<bts_mgmt::ReversePowerControlList>, Status> {
        let req = request.into_inner();
        self.bts(&req.selector)?
            .list_reverse_power_controls(cell_request(req.request))
            .await
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
    }

    async fn stream_pch_transmissions(
        &self,
        request: Request<mgmt::CellScopedRequest>,
    ) -> Result<Response<Self::StreamPchTransmissionsStream>, Status> {
        let req = request.into_inner();
        let upstream = self
            .bts(&req.selector)?
            .stream_pch_transmissions(cell_request(req.request))
            .await?
            .into_inner();
        Ok(Response::new(Box::pin(upstream)))
    }

    // Per-selector proxy: BSC management

    async fn get_bsc_status(
        &self,
        request: Request<mgmt::BsRequest>,
    ) -> Result<Response<bsc::SystemStatus>, Status> {
        let selector = request.into_inner().selector;
        self.bsc_mgmt(&selector)?.get_bsc_status(()).await
    }

    async fn list_mobiles(
        &self,
        request: Request<mgmt::BsRequest>,
    ) -> Result<Response<bsc::MobileList>, Status> {
        let selector = request.into_inner().selector;
        self.bsc_mgmt(&selector)?.list_mobiles(()).await
    }

    async fn list_channels(
        &self,
        request: Request<mgmt::BsRequest>,
    ) -> Result<Response<bsc::ChannelList>, Status> {
        let selector = request.into_inner().selector;
        self.bsc_mgmt(&selector)?.list_channels(()).await
    }

    async fn stream_access_events(
        &self,
        request: Request<mgmt::BsRequest>,
    ) -> Result<Response<Self::StreamAccessEventsStream>, Status> {
        let selector = request.into_inner().selector;
        let upstream = self
            .bsc_mgmt(&selector)?
            .stream_access_events(())
            .await?
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
            .await?
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
            .await?
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
    }

    // Per-selector: data-call setup

    async fn initiate_data_call(
        &self,
        request: Request<mgmt::InitiateDataCallScopedRequest>,
    ) -> Result<Response<bsc::InitiateDataCallResponse>, Status> {
        let req = request.into_inner();
        let inner = req.request.unwrap_or_default();
        BscServiceClient::new(self.channel(&req.selector)?)
            .initiate_data_call(inner)
            .await
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
    }

    // Global interface: PCF

    async fn list_pcf_sessions(
        &self,
        _: Request<()>,
    ) -> Result<Response<pcf_mgmt::PcfSessionList>, Status> {
        let sessions = self
            .packet_client()
            .list_sessions(packet::ListSessionsRequest {})
            .await?
            .into_inner()
            .sessions;
        Ok(Response::new(pcf_mgmt::PcfSessionList { sessions }))
    }

    async fn get_pcf_session(
        &self,
        request: Request<pcf_mgmt::GetPcfSessionRequest>,
    ) -> Result<Response<packet::GetSessionStatusResponse>, Status> {
        let session_id = request.into_inner().session_id;
        self.packet_client()
            .get_session_status(packet::GetSessionStatusRequest { session_id })
            .await
    }

    // Global interface: PDSN

    async fn list_pdsn_sessions(
        &self,
        _: Request<()>,
    ) -> Result<Response<pdsn_mgmt::PdsnSessionList>, Status> {
        let sessions = self
            .packet_client()
            .list_sessions(packet::ListSessionsRequest {})
            .await?
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
    }

    async fn get_pdsn_session_by_ip(
        &self,
        request: Request<pdsn_mgmt::GetPdsnSessionByIpRequest>,
    ) -> Result<Response<packet::GetSessionByIpResponse>, Status> {
        let peer_ip = request.into_inner().peer_ip;
        self.packet_client()
            .get_session_by_ip(packet::GetSessionByIpRequest { peer_ip })
            .await
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
            .ok_or_else(|| Status::not_found("cell not found on base station"))?;
        summary
            .an_grpc_addr
            .filter(|a| !a.is_empty())
            .ok_or_else(|| Status::unavailable("cell has no HRPD access network"))
    }
}

async fn an_client(addr: &str) -> Result<AnServiceClient<Channel>, Status> {
    let endpoint = normalize_grpc_uri(addr);
    let channel = Channel::from_shared(endpoint)
        .map_err(|e| Status::unavailable(format!("invalid AN address: {e}")))?
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
            }],
            "http://127.0.0.1:17021",
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
            }],
            "http://127.0.0.1:17021",
        );
        assert!(one.channel(&None).is_ok());

        let two = NetworkManagement::new(
            &[
                BaseStationConfig {
                    management_endpoint: "http://127.0.0.1:17016".to_string(),
                    id: None,
                },
                BaseStationConfig {
                    management_endpoint: "http://127.0.0.1:17026".to_string(),
                    id: None,
                },
            ],
            "http://127.0.0.1:17021",
        );
        assert!(two.channel(&None).is_err());
    }
}
