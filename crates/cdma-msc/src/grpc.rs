//! MSC management gRPC server and client.
//!
//! The MSC hosts its own management gRPC endpoint so that `initiate_call`,
//! `list_calls`, and `send_sms` no longer proxy through the BSC.

use tonic::{Request, Response, Status};

use crate::management::InitiateCallRequest;

pub mod bsc {
    pub mod v1 {
        tonic::include_proto!("bsc.v1");
    }
}

pub mod events {
    pub mod v1 {
        tonic::include_proto!("events.v1");
    }
}

// Backwards-compatible alias used by `crate::otasp::*` modules.
pub use events as events_proto;

pub mod base_station {
    pub mod v1 {
        tonic::include_proto!("base_station.v1");
    }
}

pub mod msc_management {
    pub mod v1 {
        tonic::include_proto!("msc_management.v1");
    }
}

pub mod voice_gateway {
    pub mod v1 {
        tonic::include_proto!("voice_gateway.v1");
    }
}

pub mod an {
    pub mod v1 {
        tonic::include_proto!("an.v1");
    }
}

pub mod packet {
    pub mod v1 {
        tonic::include_proto!("packet.v1");
    }
}

pub mod bts_management {
    pub mod v1 {
        tonic::include_proto!("bts_management.v1");
    }
}

pub mod bsc_management {
    pub mod v1 {
        tonic::include_proto!("bsc_management.v1");
    }
}

pub mod pcf_management {
    pub mod v1 {
        tonic::include_proto!("pcf_management.v1");
    }
}

pub mod pdsn_management {
    pub mod v1 {
        tonic::include_proto!("pdsn_management.v1");
    }
}

pub mod management {
    pub mod v1 {
        tonic::include_proto!("management.v1");
    }
}

// The single network management interface. Its implementation lives in
// `crate::mgmt_proxy`.
pub mod mgmt {
    pub mod v1 {
        tonic::include_proto!("mgmt.v1");
    }
}

use bsc::v1 as proto;
use msc_management::v1 as mgmt_proto;
use msc_management::v1::CallList;
use msc_management::v1::msc_management_service_server::MscManagementService;

/// MSC-side gRPC management service backed by a channel into the MSC runtime.
pub struct MscManagementServiceImpl {
    mgmt_tx: tokio::sync::mpsc::Sender<crate::management::PendingControlRequest>,
    otasp_event_tx: Option<tokio::sync::broadcast::Sender<events_proto::v1::MscNetworkEvent>>,
    authentication_history: Option<std::sync::Arc<crate::authentication::AuthenticationHistory>>,
    admission_history: Option<std::sync::Arc<crate::admission::AdmissionHistory>>,
    access: crate::admission::AccessConfig,
    authentication_mode: crate::authentication::AuthenticationMode,
    base_stations: Option<std::sync::Arc<crate::base_station::BaseStations>>,
}

impl MscManagementServiceImpl {
    /// Creates a gRPC service that feeds management requests into the MSC runtime channel.
    pub fn from_channel(
        mgmt_tx: tokio::sync::mpsc::Sender<crate::management::PendingControlRequest>,
    ) -> Self {
        Self {
            mgmt_tx,
            otasp_event_tx: None,
            authentication_history: None,
            admission_history: None,
            access: Default::default(),
            authentication_mode: Default::default(),
            base_stations: None,
        }
    }

    pub fn with_policy(
        mut self,
        access: crate::admission::AccessConfig,
        authentication_mode: crate::authentication::AuthenticationMode,
    ) -> Self {
        self.access = access;
        self.authentication_mode = authentication_mode;
        self
    }

    pub fn with_base_stations(
        mut self,
        base_stations: std::sync::Arc<crate::base_station::BaseStations>,
    ) -> Self {
        self.base_stations = Some(base_stations);
        self
    }

    pub fn with_admission_history(
        mut self,
        history: std::sync::Arc<crate::admission::AdmissionHistory>,
    ) -> Self {
        self.admission_history = Some(history);
        self
    }

    pub fn with_otasp(
        mut self,
        event_tx: tokio::sync::broadcast::Sender<events_proto::v1::MscNetworkEvent>,
    ) -> Self {
        self.authentication_history = Some(
            crate::authentication::AuthenticationHistory::recording(&event_tx),
        );
        self.otasp_event_tx = Some(event_tx);
        self
    }
}

#[tonic::async_trait]
impl MscManagementService for MscManagementServiceImpl {
    async fn list_base_stations(
        &self,
        _: Request<()>,
    ) -> Result<Response<mgmt_proto::BaseStationList>, Status> {
        let Some(base_stations) = self.base_stations.as_ref() else {
            return Err(Status::unavailable("base station registry not attached"));
        };
        let nodes = base_stations
            .summaries()
            .into_iter()
            .map(|summary| mgmt_proto::BaseStationSummary {
                management_endpoint: summary.management_endpoint,
                node_id: summary.node_id.map(|id| id.0).unwrap_or_default(),
                a1_addr: summary
                    .a1_addr
                    .map(|addr| addr.to_string())
                    .unwrap_or_default(),
                attached: summary.attached,
                cells: summary
                    .cells
                    .into_iter()
                    .map(|cell| base_station::v1::ServedCell {
                        cell: Some(proto::CellId {
                            cell: u32::from(cell.cell.cell),
                            sector: u32::from(cell.cell.sector),
                        }),
                        sid: u32::from(cell.sid),
                        nid: u32::from(cell.nid),
                        mcc_digits: cell.mcc_digits,
                        imsi_11_12_digits: cell.imsi_11_12_digits,
                        in_service: cell.in_service,
                    })
                    .collect(),
                status_detail: summary.status_detail,
            })
            .collect();
        Ok(Response::new(mgmt_proto::BaseStationList { nodes }))
    }

    async fn initiate_otasp(
        &self,
        request: Request<mgmt_proto::InitiateOtaspRequest>,
    ) -> Result<Response<mgmt_proto::InitiateOtaspResponse>, Status> {
        let request = request.into_inner();
        let subscriber_id = uuid::Uuid::parse_str(&request.subscriber_id)
            .map_err(|error| Status::invalid_argument(error.to_string()))?;
        let (response_tx, response_rx) = tokio::sync::oneshot::channel();
        self.mgmt_tx
            .send(crate::management::PendingControlRequest::InitiateOtasp {
                subscriber_id,
                imsi: request.imsi,
                response_tx,
            })
            .await
            .map_err(|_| Status::unavailable("MSC runtime is shutting down"))?;
        let accepted = response_rx
            .await
            .map_err(|_| Status::unavailable("MSC runtime dropped response"))?
            .map_err(|error| Status::failed_precondition(error.to_string()))?;
        Ok(Response::new(mgmt_proto::InitiateOtaspResponse {
            call_id: accepted.call_id.0,
        }))
    }

    async fn initiate_call(
        &self,
        request: Request<proto::InitiateCallRequest>,
    ) -> Result<Response<proto::InitiateCallResponse>, Status> {
        let inner = request.into_inner();
        let subscriber_id = uuid::Uuid::parse_str(&inner.subscriber_id)
            .map_err(|e| Status::invalid_argument(format!("invalid subscriber_id: {e}")))?;
        let (response_tx, response_rx) = tokio::sync::oneshot::channel();
        self.mgmt_tx
            .send(crate::management::PendingControlRequest::InitiateCall {
                request: InitiateCallRequest {
                    subscriber_id,
                    audio_file: inner.audio_file,
                    caller_number: inner.caller_number,
                },
                response_tx,
            })
            .await
            .map_err(|_| Status::unavailable("MSC runtime is shutting down"))?;
        let result = response_rx
            .await
            .map_err(|_| Status::internal("MSC runtime dropped response channel"))?
            .map_err(|e| Status::internal(e.to_string()))?;
        Ok(Response::new(proto::InitiateCallResponse {
            accepted: true,
            message: format!("call_id={}", result.call_id.0),
        }))
    }

    async fn send_sms(
        &self,
        request: Request<mgmt_proto::SendSmsRequest>,
    ) -> Result<Response<mgmt_proto::SendSmsResponse>, Status> {
        let inner = request.into_inner();
        let destination = match inner.destination {
            Some(mgmt_proto::send_sms_request::Destination::DestinationNumber(num)) => {
                crate::sms::SmsDestinationKey::PhoneNumber(num)
            }
            Some(mgmt_proto::send_sms_request::Destination::DestinationImsi(imsi)) => {
                crate::sms::SmsDestinationKey::Imsi(imsi)
            }
            None => {
                return Err(Status::invalid_argument(
                    "destination_number or destination_imsi is required",
                ));
            }
        };
        let teleservice_id = inner
            .teleservice_id
            .map(|v| {
                u16::try_from(v).map_err(|_| Status::invalid_argument("teleservice_id > 65535"))
            })
            .transpose()?;
        let (response_tx, response_rx) = tokio::sync::oneshot::channel();
        self.mgmt_tx
            .send(crate::management::PendingControlRequest::SendSms {
                request: crate::sms::SmsSendRequest {
                    originating_number: inner.originating_number,
                    text: inner.text,
                    destination,
                    timeout_ms: inner.timeout_ms.unwrap_or(30_000),
                    teleservice_id,
                    raw_user_data: inner.raw_user_data,
                    serving_node: None,
                },
                response_tx,
            })
            .await
            .map_err(|_| Status::unavailable("MSC runtime is shutting down"))?;
        let sms_id = response_rx
            .await
            .map_err(|_| Status::internal("MSC runtime dropped response channel"))?;
        Ok(Response::new(mgmt_proto::SendSmsResponse {
            accepted: sms_id.is_some(),
            message: sms_id.map(|id| format!("sms_id={id}")).unwrap_or_else(|| {
                "SMS delivery failed (mobile unreachable or SMSC unavailable)".to_string()
            }),
        }))
    }

    type StreamOtaspEventsStream = std::pin::Pin<
        Box<
            dyn tokio_stream::Stream<Item = Result<events_proto::v1::MscNetworkEvent, Status>>
                + Send
                + 'static,
        >,
    >;

    async fn stream_otasp_events(
        &self,
        _: Request<()>,
    ) -> Result<Response<Self::StreamOtaspEventsStream>, Status> {
        let Some(tx) = self.otasp_event_tx.as_ref() else {
            return Err(Status::unavailable("OTASP event stream not configured"));
        };
        let mut rx = tx.subscribe();
        let stream = async_stream::stream! {
            loop {
                match rx.recv().await {
                    Ok(ev) => yield Ok(ev),
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        };
        Ok(Response::new(Box::pin(stream)))
    }

    async fn list_authentication_events(
        &self,
        request: Request<mgmt_proto::ListAuthenticationEventsRequest>,
    ) -> Result<Response<mgmt_proto::AuthenticationEventList>, Status> {
        let Some(history) = self.authentication_history.as_ref() else {
            return Err(Status::unavailable("authentication history not configured"));
        };
        let request = request.into_inner();
        Ok(Response::new(mgmt_proto::AuthenticationEventList {
            events: history.matching(&request.imsis, request.esn, request.limit as usize),
        }))
    }

    async fn list_admission_events(
        &self,
        request: Request<mgmt_proto::ListAdmissionEventsRequest>,
    ) -> Result<Response<mgmt_proto::AdmissionEventList>, Status> {
        let Some(history) = self.admission_history.as_ref() else {
            return Err(Status::unavailable("admission history not configured"));
        };
        let request = request.into_inner();
        let meid = (!request.meid.is_empty()).then_some(request.meid.as_str());
        Ok(Response::new(mgmt_proto::AdmissionEventList {
            events: if request.current_only {
                history.current_matching(&request.imsis, request.esn, meid, request.limit as usize)
            } else {
                history.matching(&request.imsis, request.esn, meid, request.limit as usize)
            },
            current_snapshot: request.current_only,
            access_mode: self.access.mode.config_name().to_string(),
            register_policy: self.access.register_policy.config_name().to_string(),
            authentication_mode: self.authentication_mode.config_name().to_string(),
        }))
    }

    async fn list_calls(&self, _: Request<()>) -> Result<Response<CallList>, Status> {
        let (response_tx, response_rx) = tokio::sync::oneshot::channel();
        self.mgmt_tx
            .send(crate::management::PendingControlRequest::ListCalls { response_tx })
            .await
            .map_err(|_| Status::unavailable("MSC runtime is shutting down"))?;
        let snapshots = response_rx
            .await
            .map_err(|_| Status::internal("MSC runtime dropped response channel"))?
            .map_err(|e| Status::internal(e.to_string()))?;
        Ok(Response::new(CallList {
            call_ids: snapshots.iter().map(|s| s.id.0.to_string()).collect(),
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn current_admission_rpc_returns_snapshot_without_activity_history() {
        let history = std::sync::Arc::new(crate::admission::AdmissionHistory::default());
        let denied = events_proto::v1::AdmissionEvent {
            imsi: "310006026392922".into(),
            esn: Some(0x3626_2F13),
            admission_class: events_proto::v1::AdmissionClass::Denied as i32,
            ..Default::default()
        };
        history.update_current(&denied);
        let (tx, _) = tokio::sync::mpsc::channel(1);
        let service = MscManagementServiceImpl::from_channel(tx).with_admission_history(history);
        let request = |current_only| {
            Request::new(mgmt_proto::ListAdmissionEventsRequest {
                current_only,
                ..Default::default()
            })
        };
        let snapshot = service
            .list_admission_events(request(true))
            .await
            .unwrap()
            .into_inner();
        assert!(snapshot.current_snapshot);
        assert_eq!(snapshot.events, vec![denied]);
        let activity = service
            .list_admission_events(request(false))
            .await
            .unwrap()
            .into_inner();
        assert!(activity.events.is_empty());
    }
}
