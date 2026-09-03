use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;

use cdma_common::access::AccessMessage;
use cdma_common::consts::{
    SERVICE_OPTION_ASYNC_DATA, SERVICE_OPTION_HIGH_RATE_PACKET_DATA, SERVICE_OPTION_PACKET_DATA,
};
use cdma_common::events::AccessChannelEvent;
use cdma_common::lac::{
    message_types::{MessageId, WireChannel},
    paging_messages::{
        CHANNEL_ASSIGN_MODE_EXTENDED_TRAFFIC, CHANNEL_ASSIGN_MODE_TRAFFIC,
        CHANNEL_DEFAULT_CONFIG_EXPLICIT_RCS, CHANNEL_DEFAULT_CONFIG_RC1_RC1,
        CHANNEL_DEFAULT_CONFIG_RC1_RC2, CHANNEL_DEFAULT_CONFIG_RC2_RC1,
        CHANNEL_DEFAULT_CONFIG_RC2_RC2, ChannelAssignmentGrantedMode, ChannelAssignmentMessage,
        GeneralPageRecord, MsAddress, PagingChannelMessage,
    },
};
use log::{info, warn};
use parking_lot::Mutex;
use tokio::sync::broadcast;
use tokio_stream::{Stream, StreamExt};
use tonic::transport::{Certificate, Identity, ServerTlsConfig};
use tonic::{Request, Response, Status};

use super::base_station_proto::base_station_service_server::{
    BaseStationService, BaseStationServiceServer,
};
use super::base_station_proto::{
    EnrollRequest as BaseStationEnrollRequest, EnrollResponse as BaseStationEnrollResponse,
    ServedCell,
};
use super::bsc_management_proto::bsc_management_service_server::{
    BscManagementService, BscManagementServiceServer,
};
use super::bts_management_proto::bts_management_service_server::{
    BtsManagementService, BtsManagementServiceServer,
};
use super::bts_management_proto::{
    BtsAttachState as ProtoBtsAttachState, BtsList, BtsSummary, CellRequest, EnrollRequest,
    EnrollResponse, ReversePowerControlEntry, ReversePowerControlList, ReversePowerControlRequest,
};
use super::management_proto::management_facade_service_server::{
    ManagementFacadeService, ManagementFacadeServiceServer,
};
use super::management_proto::{ManagementEvent, NodeHealth, SystemOverview, management_event};
use super::proto;
use super::proto::bsc_service_server::{BscService, BscServiceServer};
use super::state::BscState;
use crate::bsc::traffic_events::forward_order_display_name;
use crate::bsc::{
    AccessCellId, BtsAttachState, BtsEntry, BtsOamClient, BtsRegistry, DataCallRequest,
    PagingEvent, TrafficEvent,
};
use crate::config::MtlsConfig;
use crate::power_control::TrafficChannelPowerSnapshot;
use cdma_common::formatting::{
    bitstream_to_hex, bytes_to_hex, format_dtmf_digits, forward_order_name,
    mobile_station_reject_reason, rejected_pdu_type_name,
};
use uuid::Uuid;

/// Channel type label the BTS uses for an active traffic channel.
const TRAFFIC_CHANNEL_TYPE: &str = "traffic";

/// How often the BSC checks for an enrolled cell it is not yet following
/// paging-channel transmissions from.
const PCH_SUBSCRIBE_POLL: Duration = Duration::from_secs(2);
/// How long the aggregate radio-metrics stream waits before retrying when no
/// cell has a management channel yet.
const RADIO_METRICS_RETRY: Duration = Duration::from_secs(1);
/// How often the aggregate radio-metrics stream rescans the registry to pick
/// up a cell that enrolled after it started following the others.
const RADIO_METRICS_RESCAN: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub struct BscServiceImpl {
    state: Arc<BscState>,
    /// Paging-channel transmissions republished from every enrolled cell.
    pch_transmissions: broadcast::Sender<proto::PagingEvent>,
}

/// A request for whichever cell the endpoint resolves on its own.
fn any_cell() -> Request<CellRequest> {
    Request::new(CellRequest {
        cell: None,
        peer_id: None,
    })
}

fn cell_request(cell: proto::CellId) -> CellRequest {
    CellRequest {
        cell: Some(cell),
        peer_id: None,
    }
}

fn to_proto_cell_id(cell: AccessCellId) -> proto::CellId {
    proto::CellId {
        cell: u32::from(cell.cell),
        sector: u32::from(cell.sector),
    }
}

fn from_proto_cell_id(cell: &proto::CellId) -> Result<AccessCellId, Status> {
    Ok(AccessCellId {
        cell: u16::try_from(cell.cell)
            .map_err(|_| Status::invalid_argument("cell must fit in 16 bits"))?,
        sector: u8::try_from(cell.sector)
            .map_err(|_| Status::invalid_argument("sector must fit in 8 bits"))?,
    })
}

fn to_proto_attach_state(state: BtsAttachState) -> ProtoBtsAttachState {
    match state {
        BtsAttachState::Disconnected => ProtoBtsAttachState::Disconnected,
        BtsAttachState::Enrolled => ProtoBtsAttachState::Enrolled,
        BtsAttachState::InService => ProtoBtsAttachState::InService,
    }
}

fn attach_state_message(state: BtsAttachState) -> &'static str {
    match state {
        BtsAttachState::Disconnected => "not reachable",
        BtsAttachState::Enrolled => "enrolled, Abis signaling not established",
        BtsAttachState::InService => "in service",
    }
}

impl BscServiceImpl {
    /// The registry entry a request addresses.
    ///
    /// A request that names no cell resolves to the sole enrolled cell, which
    /// keeps a single-cell deployment addressable without naming its cell.
    fn resolve_entry(&self, cell: Option<proto::CellId>) -> Result<Arc<BtsEntry>, Status> {
        let registry = &self.state.bts;
        match cell {
            Some(cell) => {
                let id = from_proto_cell_id(&cell)?;
                registry.get(id).ok_or_else(|| {
                    Status::not_found(format!(
                        "cell {}/{} is not enrolled",
                        cell.cell, cell.sector
                    ))
                })
            }
            None => registry.sole_entry().ok_or_else(|| {
                if registry.is_empty() {
                    Status::failed_precondition("no cell is enrolled")
                } else {
                    Status::failed_precondition(
                        "more than one cell is enrolled; name the cell in the request",
                    )
                }
            }),
        }
    }

    /// Management client for the cell a request addresses, with the cell
    /// identifier to forward so the BTS rejects a misrouted request.
    fn oam_for(
        &self,
        cell: Option<proto::CellId>,
    ) -> Result<(proto::CellId, BtsOamClient), Status> {
        let entry = self.resolve_entry(cell)?;
        let id = to_proto_cell_id(entry.cell());
        let client = entry.oam().ok_or_else(|| {
            Status::unavailable(format!(
                "cell {}/{} has no management channel: {}",
                id.cell,
                id.sector,
                entry
                    .status_detail()
                    .unwrap_or_else(|| attach_state_message(entry.attach_state()).to_string())
            ))
        })?;
        Ok((id, client))
    }

    /// The OAM client a management request addresses. A request naming a
    /// `peer_id` resolves to that peer's current cell,
    /// failing fast when the peer is offline so the call never blocks on an
    /// inactive node. A request naming a `cell` (or neither) uses the cell.
    fn oam_for_request(
        &self,
        request: CellRequest,
    ) -> Result<(proto::CellId, BtsOamClient), Status> {
        if let Some(peer_id) = request.peer_id.as_deref() {
            let cell = self
                .state
                .bts
                .peer_cell(peer_id)
                .ok_or_else(|| Status::unavailable(format!("BTS {peer_id} is offline")))?;
            return self.oam_for(Some(to_proto_cell_id(cell)));
        }
        self.oam_for(request.cell)
    }

    /// The cell that answers a request naming none, when there is exactly one.
    fn default_cell(&self) -> Option<AccessCellId> {
        self.state.bts.sole_entry().map(|entry| entry.cell())
    }

    /// The BSC's own power-control state for a traffic Walsh code on `cell`,
    /// driven by the forward outer loop from the mobile's PMRM reports.
    ///
    /// Walsh codes are allocated per cell, so every cell hands out the same
    /// codes. Without the cell the first mobile holding the code anywhere on
    /// the BSC would answer.
    fn bsc_power_snapshot(
        &self,
        cell: AccessCellId,
        walsh_code: u8,
    ) -> Option<TrafficChannelPowerSnapshot> {
        let default_cell = self.default_cell();
        self.state
            .mobiles
            .borrow()
            .iter()
            .find(|mobile| {
                mobile.traffic_walsh_code == Some(walsh_code)
                    && mobile.serving_cell.or(default_cell) == Some(cell)
            })
            .and_then(|mobile| mobile.traffic_power.clone())
    }

    /// Reverse power-control state for every reachable cell, keyed by cell and
    /// traffic Walsh code. One request per cell replaces a lookup per mobile.
    async fn reverse_power_by_cell_walsh(
        &self,
    ) -> HashMap<(AccessCellId, u8), proto::TrafficChannelPower> {
        let mut by_cell_walsh = HashMap::new();
        for entry in self.state.bts.entries() {
            let Some(mut client) = entry.oam() else {
                continue;
            };
            let cell = entry.cell();
            let list = match client
                .list_reverse_power_controls(cell_request(to_proto_cell_id(cell)))
                .await
            {
                Ok(response) => response.into_inner(),
                Err(e) => {
                    warn!(
                        "cell {}/{} reverse power-control list failed: {e}",
                        cell.cell, cell.sector
                    );
                    continue;
                }
            };
            for item in list.entries {
                let Ok(walsh) = u8::try_from(item.walsh_code) else {
                    continue;
                };
                if let Some(power) = item.power {
                    by_cell_walsh.insert((cell, walsh), power);
                }
            }
        }
        by_cell_walsh
    }

    /// Radio metrics merged from every enrolled cell.
    ///
    /// Follows all cells at once, so the aggregate telemetry heartbeat keeps
    /// flowing whether the BSC serves one cell or several — a consumer merging
    /// this never loses it. It rescans to pick up a cell that enrolls later,
    /// drops a cell whose stream ends, and waits rather than ending when no
    /// cell has a management channel yet.
    async fn radio_metrics_stream(&self) -> GrpcStream<proto::RadioMetrics> {
        let service = self.clone();
        Box::pin(async_stream::stream! {
            let mut merged = tokio_stream::StreamMap::new();
            loop {
                for entry in service.state.bts.entries() {
                    let cell = to_proto_cell_id(entry.cell());
                    let key = (cell.cell, cell.sector);
                    if merged.contains_key(&key) {
                        continue;
                    }
                    let Some(mut client) = entry.oam() else {
                        continue;
                    };
                    match client.stream_radio_metrics(cell_request(cell)).await {
                        Ok(response) => {
                            merged.insert(key, response.into_inner());
                        }
                        Err(e) => warn!(
                            "cell {}/{} radio metrics stream unavailable: {e}",
                            cell.cell, cell.sector
                        ),
                    }
                }
                if merged.is_empty() {
                    tokio::time::sleep(RADIO_METRICS_RETRY).await;
                    continue;
                }
                tokio::select! {
                    item = merged.next() => match item {
                        Some((_, Ok(metrics))) => yield Ok(metrics),
                        Some((key, Err(e))) => {
                            warn!("cell {}/{} radio metrics stream ended: {e}", key.0, key.1);
                            merged.remove(&key);
                        }
                        None => {}
                    },
                    _ = tokio::time::sleep(RADIO_METRICS_RESCAN) => {}
                }
            }
        })
    }
}

/// Overlay the BSC-owned loop state onto a BTS reverse power-control
/// snapshot.
///
/// The BTS measures the reverse link and owns those fields. The BSC drives
/// the forward outer loop from PMRM reports. `power_history` stays empty
/// because it accumulates per mobile rather than per traffic channel.
fn merge_bsc_power_fields(
    mut power: proto::TrafficChannelPower,
    bsc: Option<&TrafficChannelPowerSnapshot>,
) -> proto::TrafficChannelPower {
    let Some(bsc) = bsc else {
        return power;
    };
    power.last_pcg_snr_db = bsc
        .last_pcg_snr_db
        .map(|arr| arr.to_vec())
        .unwrap_or_default();
    power.last_active_pcg_mask = bsc
        .last_active_pcg_mask
        .map(|arr| arr.to_vec())
        .unwrap_or_default();
    power.reverse_pilot_ec_io_db = bsc.reverse_pilot_ec_io_db;
    power.forward_gain_offset_db = bsc.forward_gain_offset_db;
    power.forward_last_fer_pct = bsc.forward_last_fer_pct.unwrap_or(0.0);
    power.forward_last_pmrm_errors = bsc.forward_last_pmrm_errors;
    power.forward_last_pmrm_frames = bsc.forward_last_pmrm_frames;
    power.forward_pmrm_count = bsc.forward_pmrm_count;
    power.forward_pilot_ec_io_db = bsc.forward_pilot_ec_io_db.clone();
    power.forward_radio_config = bsc.forward_radio_config;
    power.reverse_radio_config = bsc.reverse_radio_config;
    power
}

/// Follow every enrolled cell's paging-channel transmissions and republish
/// them, so the BSC's paging stream shows what actually went out on air.
///
/// The BTS owns General Page assembly, slot placement and page retry, so the
/// Abis exchange alone does not say what was transmitted.
fn spawn_pch_transmission_bridge(
    registry: Arc<BtsRegistry>,
    sink: broadcast::Sender<proto::PagingEvent>,
) {
    let followed: Arc<Mutex<HashSet<AccessCellId>>> = Arc::new(Mutex::new(HashSet::new()));
    tokio::spawn(async move {
        loop {
            for entry in registry.entries() {
                let cell = entry.cell();
                let Some(mut client) = entry.oam() else {
                    continue;
                };
                if !followed.lock().insert(cell) {
                    continue;
                }
                let followed = followed.clone();
                let sink = sink.clone();
                tokio::spawn(async move {
                    match client
                        .stream_pch_transmissions(cell_request(to_proto_cell_id(cell)))
                        .await
                    {
                        Ok(response) => {
                            let mut stream = response.into_inner();
                            while let Some(event) = stream.next().await {
                                match event {
                                    Ok(event) => {
                                        let _ = sink.send(event);
                                    }
                                    Err(e) => {
                                        warn!(
                                            "cell {}/{} PCH transmission stream ended: {e}",
                                            cell.cell, cell.sector
                                        );
                                        break;
                                    }
                                }
                            }
                        }
                        Err(e) => warn!(
                            "cell {}/{} PCH transmission subscribe failed: {e}",
                            cell.cell, cell.sector
                        ),
                    }
                    followed.lock().remove(&cell);
                });
            }
            tokio::time::sleep(PCH_SUBSCRIBE_POLL).await;
        }
    });
}

#[tonic::async_trait]
impl BaseStationService for BscServiceImpl {
    async fn enroll(
        &self,
        _: Request<BaseStationEnrollRequest>,
    ) -> Result<Response<BaseStationEnrollResponse>, Status> {
        let cells = self
            .state
            .bts
            .entries()
            .into_iter()
            .map(|entry| {
                let params = entry.params();
                ServedCell {
                    cell: Some(to_proto_cell_id(entry.cell())),
                    sid: u32::from(params.overhead.sid),
                    nid: u32::from(params.overhead.nid),
                    mcc_digits: cdma_common::paging::mcc_to_digits(params.mcc).unwrap_or_default(),
                    imsi_11_12_digits: cdma_common::paging::imsi_11_12_to_digits(params.imsi_11_12)
                        .unwrap_or_default(),
                    in_service: entry.is_in_service(),
                }
            })
            .collect();
        Ok(Response::new(BaseStationEnrollResponse {
            node_id: self.state.node_id.clone(),
            a1_addr: self.state.a1_bind_addr.to_string(),
            cells,
        }))
    }
}

fn to_proto_traffic_channel_power(tp: &TrafficChannelPowerSnapshot) -> proto::TrafficChannelPower {
    proto::TrafficChannelPower {
        target_eb_nt_db: tp.target_eb_nt_db,
        effective_target_eb_nt_db: tp.effective_target_eb_nt_db,
        manual_target_override_db: tp.manual_target_override_db,
        last_pcg_snr_db: tp
            .last_pcg_snr_db
            .map(|arr| arr.to_vec())
            .unwrap_or_default(),
        last_active_pcg_mask: tp
            .last_active_pcg_mask
            .map(|arr| arr.to_vec())
            .unwrap_or_default(),
        last_pcbs: tp.last_pcbs.iter().map(|b| *b as u32).collect(),
        reverse_pilot_ec_io_db: tp.reverse_pilot_ec_io_db,
        fer_pct: tp.fer_pct,
        frames_total: tp.frames_total,
        frames_crc_error: tp.frames_crc_error,
        forward_gain_offset_db: tp.forward_gain_offset_db,
        forward_last_fer_pct: tp.forward_last_fer_pct.unwrap_or(0.0),
        forward_last_pmrm_errors: tp.forward_last_pmrm_errors,
        forward_last_pmrm_frames: tp.forward_last_pmrm_frames,
        forward_pmrm_count: tp.forward_pmrm_count,
        forward_pilot_ec_io_db: tp.forward_pilot_ec_io_db.clone(),
        last_pcg_pilot_ec_nt_db: tp
            .last_pcg_pilot_ec_nt_db
            .map(|arr| arr.to_vec())
            .unwrap_or_default(),
        forward_radio_config: tp.forward_radio_config,
        reverse_radio_config: tp.reverse_radio_config,
        power_history: tp
            .power_history
            .iter()
            .map(|e| proto::PowerControlSample {
                timestamp_ms: e.timestamp_ms,
                measured_eb_nt_db: e.measured_mean_db,
                target_eb_nt_db: e.target_db,
                forward_gain_db: e.forward_gain_db,
                fer_pct: e.fer_pct,
            })
            .collect(),
        measured_inner_loop_1s_db: None,
        measured_inner_loop_1s_timestamp_ms: None,
    }
}

/// Format origination digits for gRPC display, appending raw hex for DTMF mode.
fn format_origination_digits(digit_mode: bool, digits: &[u8]) -> String {
    if digits.is_empty() {
        return String::new();
    }
    let rendered = format_dtmf_digits(digits, digit_mode);
    if digit_mode {
        return rendered;
    }
    let raw = digits
        .iter()
        .map(|d| format!("{:X}", d))
        .collect::<Vec<_>>()
        .join("");
    format!("{rendered} (raw={raw})")
}

fn should_stream_access_event(event: &AccessChannelEvent) -> bool {
    event.traffic_voice_bits.is_none()
        && !event.is_traffic_pcg_measurement
        && !event.is_traffic_phy_status
}

fn to_proto_access_event(e: &AccessChannelEvent) -> proto::AccessEvent {
    let timestamp_us = e.wall_clock_us;
    let body = match e.decoded_l3.as_ref() {
        Some(AccessMessage::Registration(m)) => Some(proto::access_event::Body::Registration(
            proto::AccessRegistration {
                reg_type: m.reg_type as u32,
                mob_term: m.mob_term,
                slot_cycle_index: m.slot_cycle_index as u32,
                mob_p_rev: m.mob_p_rev as u32,
                scm: m.scm as u32,
                return_cause: m.return_cause as u32,
                remaining_bits: m.remaining_bits as u32,
            },
        )),
        Some(AccessMessage::Origination(m)) => Some(proto::access_event::Body::Origination(
            proto::AccessOrigination {
                mob_term: m.mob_term,
                slot_cycle_index: m.slot_cycle_index as u32,
                mob_p_rev: m.mob_p_rev as u32,
                scm: m.scm as u32,
                request_mode: m.request_mode as u32,
                special_service: m.special_service,
                service_option: m.service_option.map(|v| v as u32),
                pm: m.pm,
                digit_mode: m.digit_mode,
                number_type: m.number_type.map(|v| v as u32),
                number_plan: m.number_plan.map(|v| v as u32),
                more_fields: m.more_fields,
                num_fields: m.num_fields as u32,
                digits: format_origination_digits(m.digit_mode, &m.digits),
                nar_an_cap: m.nar_an_cap,
                paca_reorig: m.paca_reorig,
                return_cause: m.return_cause as u32,
                more_records: m.more_records,
                encryption_supported: m.encryption_supported.map(|v| v as u32),
                paca_supported: m.paca_supported,
                alt_service_options: m.alt_service_options.iter().map(|v| *v as u32).collect(),
                drs: m.drs,
                uzid_incl: m.uzid_incl,
                uzid: m.uzid.map(|v| v as u32),
                ch_ind: m.ch_ind.map(|v| v as u32),
                sr_id: m.sr_id.map(|v| v as u32),
                otd_supported: m.otd_supported,
                qpch_supported: m.qpch_supported,
                enhanced_rc: m.enhanced_rc,
                for_rc_pref: m.for_rc_pref.map(|v| v as u32),
                rev_rc_pref: m.rev_rc_pref.map(|v| v as u32),
                fch_supported: m.fch_supported,
                fch: m
                    .fch_capability
                    .as_ref()
                    .map(|cap| proto::AccessFchCapability {
                        frame_size_5ms_supported: cap.frame_size_5ms_supported,
                        for_supported_rcs: cap
                            .for_supported_rcs
                            .iter()
                            .map(|v| *v as u32)
                            .collect(),
                        rev_supported_rcs: cap
                            .rev_supported_rcs
                            .iter()
                            .map(|v| *v as u32)
                            .collect(),
                    }),
                dcch_supported: m.dcch_supported,
                dcch: m
                    .dcch_capability
                    .as_ref()
                    .map(|cap| proto::AccessDcchCapability {
                        frame_size_mode: cap.frame_size_mode as u32,
                        for_supported_rcs: cap
                            .for_supported_rcs
                            .iter()
                            .map(|v| *v as u32)
                            .collect(),
                        rev_supported_rcs: cap
                            .rev_supported_rcs
                            .iter()
                            .map(|v| *v as u32)
                            .collect(),
                    }),
                geo_loc_incl: m.geo_loc_incl,
                geo_loc_type: m.geo_loc_type.map(|v| v as u32),
                rev_fch_gating_req: m.rev_fch_gating_req,
                orig_reason: m.orig_reason,
                orig_count: m.orig_count.map(|v| v as u32),
                remaining_bits: m.remaining_bits as u32,
            },
        )),
        Some(AccessMessage::PageResponse(m)) => Some(proto::access_event::Body::PageResponse(
            proto::AccessPageResponse {
                mob_term: m.mob_term,
                slot_cycle_index: m.slot_cycle_index as u32,
                mob_p_rev: m.mob_p_rev as u32,
                scm: m.scm as u32,
                request_mode: m.request_mode as u32,
                service_option: m.service_option as u32,
                pm: m.pm,
                nar_an_cap: m.nar_an_cap,
                alt_service_options: m.alt_service_options.iter().map(|v| *v as u32).collect(),
                remaining_bits: m.remaining_bits as u32,
            },
        )),
        Some(AccessMessage::Order(m)) => {
            let fwd_ch = if e.traffic_walsh_code.is_some() {
                WireChannel::ForwardDedicated
            } else {
                WireChannel::ForwardCommon
            };
            let reject = m.parse_mobile_station_reject_order(fwd_ch).map(|detail| {
                proto::AccessMobileReject {
                    ordq: detail.ordq as u32,
                    ordq_name: mobile_station_reject_reason(detail.ordq).to_string(),
                    rejected_type: detail.rejected_type as u32,
                    rejected_type_name: MessageId::from_wire(fwd_ch, detail.rejected_type)
                        .map(|id| id.name().to_string())
                        .unwrap_or_else(|| format!("Unknown(0x{:02x})", detail.rejected_type)),
                    rejected_order: detail.rejected_order.map(|v| v as u32),
                    rejected_order_name: detail
                        .rejected_order
                        .map(forward_order_name)
                        .map(str::to_string),
                    rejected_ordq: detail.rejected_ordq.map(|v| v as u32),
                    rejected_record: detail.rejected_record.map(|v| v as u32),
                    con_ref: detail.con_ref.map(|v| v as u32),
                    tag: detail.tag.map(|v| v as u32),
                    rejected_pdu_type: detail.rejected_pdu_type.map(|v| v as u32),
                    rejected_pdu_type_name: detail
                        .rejected_pdu_type
                        .map(rejected_pdu_type_name)
                        .map(str::to_string),
                    trailing_hex: bytes_to_hex(&detail.trailing_bytes),
                }
            });
            Some(proto::access_event::Body::Order(proto::AccessOrder {
                order: m.order as u32,
                add_record_len: m.add_record_len as u32,
                order_name: m.order_name().to_string(),
                detail: m.order_detail(fwd_ch),
                order_specific_hex: bytes_to_hex(&m.order_specific),
                reject,
                remaining_bits: m.remaining_bits as u32,
            }))
        }
        Some(AccessMessage::DataBurst(m)) => {
            let decoded_sms = if m.burst_type == 3 {
                cdma_common::sms::decode_mo_sms(&m.fields).map(|d| proto::DecodedSms {
                    teleservice_id: d.teleservice_id as u32,
                    destination_number: d.destination_number,
                    originating_number: String::new(), // resolved by BSC, not available here
                    message_type: d.message_type as u32,
                    message_id: d.message_id as u32,
                    text: d.text,
                    user_data: d.user_data,
                })
            } else {
                None
            };
            Some(proto::access_event::Body::DataBurst(
                proto::AccessDataBurst {
                    msg_number: m.msg_number as u32,
                    burst_type: m.burst_type as u32,
                    burst_type_name: m.burst_type_name().to_string(),
                    num_msgs: m.num_msgs as u32,
                    num_fields: m.num_fields as u32,
                    payload_bytes: m.fields.len() as u32,
                    payload_hex: bytes_to_hex(&m.fields),
                    remaining_bits: m.remaining_bits as u32,
                    decoded_sms,
                },
            ))
        }
        Some(AccessMessage::ServiceConnectCompletion(m)) => {
            Some(proto::access_event::Body::ServiceConnectCompletion(
                proto::AccessServiceConnectCompletion {
                    serv_con_seq: m.serv_con_seq as u32,
                },
            ))
        }
        Some(AccessMessage::ServiceResponse(m)) => {
            let purpose_name = match m.resp_purpose {
                0b0000 => "accept",
                0b0001 => "reject",
                0b0010 => "counter-propose",
                _ => "unknown",
            };
            let so = m
                .service_config
                .as_ref()
                .and_then(|cfg| cfg.connection_records.first())
                .map(|cr| cr.service_option as u32);
            Some(proto::access_event::Body::ServiceResponse(
                proto::AccessServiceResponse {
                    serv_req_seq: m.serv_req_seq as u32,
                    resp_purpose: m.resp_purpose as u32,
                    resp_purpose_name: purpose_name.to_string(),
                    service_option: so,
                },
            ))
        }
        Some(AccessMessage::PowerMeasurementReport(m)) => {
            Some(proto::access_event::Body::PowerMeasurementReport(
                proto::AccessPowerMeasurementReport {
                    errors_detected: m.errors_detected as u32,
                    pwr_meas_frames: m.pwr_meas_frames as u32,
                    last_hdm_seq: m.last_hdm_seq as u32,
                    pilot_strengths: m.pilot_strengths.iter().map(|&s| s as u32).collect(),
                    dcch_pwr_meas_incl: m.dcch_pwr_meas_incl,
                    dcch_pwr_meas_frames: m.dcch_pwr_meas_frames.map(|v| v as u32),
                    dcch_errors_detected: m.dcch_errors_detected.map(|v| v as u32),
                    sch_pwr_meas_incl: m.sch_pwr_meas_incl,
                    sch_id: m.sch_id.map(|v| v as u32),
                    sch_pwr_meas_frames: m.sch_pwr_meas_frames.map(|v| v as u32),
                    sch_errors_detected: m.sch_errors_detected.map(|v| v as u32),
                },
            ))
        }
        _ => None,
    };

    let (rdsch_summary, rdsch_msg_type_name) = e
        .decoded_rdsch
        .as_ref()
        .map(|rdsch| {
            (
                Some(rdsch.summary()),
                Some(rdsch.msg_type_name().to_string()),
            )
        })
        .unwrap_or((None, None));

    proto::AccessEvent {
        chip_start: e.chip_start as u64,
        preamble_frames: e.preamble_frames,
        pd: e.pd as u32,
        msg_type: e
            .message_id
            .wire_type(WireChannel::ReverseCommon)
            .unwrap_or(0) as u32,
        msg_type_name: e.msg_type_name.clone(),
        address: e.address.clone(),
        resolved_address: e.resolved_address.clone(),
        subscriber_id: e.subscriber_id.clone(),
        l3_summary: e.l3_summary.clone(),
        pdu_summary: e.pdu_summary.clone(),
        msg_seq: e.msg_seq.map(|v| v as u32),
        ack_seq: e.ack_seq.map(|v| v as u32),
        ack_req: e.ack_req,
        valid_ack: e.valid_ack,
        msid_type: e.msid_type.map(|v| v as u32),
        esn: e.esn,
        imsi_m_s1: e.imsi_m_s1,
        imsi_m_s2: e.imsi_m_s2.map(|v| v as u32),
        meid: e.meid.clone(),
        mob_p_rev: e.mob_p_rev.map(|v| v as u32),
        timestamp_us,
        snr_db: e.snr_db,
        signal_power_db: e.signal_power_db,
        demod_quality_pct: e.demod_quality_pct,
        rx_power_dbm: None, // only computed for mobile summary (requires config offset)
        event_id: e.event_id.clone(),
        body,
        traffic_walsh_code: e.traffic_walsh_code.map(|v| v as u32),
        is_preamble_only: e.is_preamble_only,
        rdsch_summary,
        rdsch_msg_type_name,
        cell: e.cell.map(to_proto_cell_id),
    }
}

fn to_proto_traffic_event(ev: &TrafficEvent) -> proto::TrafficEvent {
    let mcsb = &ev.mcsb;
    let header = proto::PagingPduHeader {
        msg_tag: mcsb
            .message_id
            .wire_type(WireChannel::ForwardDedicated)
            .unwrap_or(0) as u32,
        msg_type_name: mcsb.message_id.name().to_string(),
        sdu_length_bits: mcsb.length_bits as u32,
        address: mcsb.address.as_ref().map(ms_address_to_proto),
        msg_seq: mcsb.msg_seq as u32,
        ack_seq: mcsb.ack_seq as u32,
        ack_req: mcsb.ack_req,
        valid_ack: mcsb.valid_ack,
        resolved_address: mcsb
            .address
            .as_ref()
            .map(crate::addressing::format_ms_address)
            .unwrap_or_default(),
    };

    let body = if let Some(order) = ev.order.as_ref() {
        Some(proto::traffic_event::Body::Order(proto::PagingOrder {
            order: order.order as u32,
            ordq: order.ordq as u32,
            order_name: forward_order_display_name(order).to_string(),
        }))
    } else if let Some(sr) = ev.service_request.as_ref() {
        let (so, for_mux, rev_mux, for_rc, rev_rc) = sr
            .service_config
            .as_ref()
            .map(|cfg| {
                let so = cfg.connections.first().map(|c| c.service_option as u32);
                (
                    so,
                    Some(cfg.for_mux_option as u32),
                    Some(cfg.rev_mux_option as u32),
                    Some(cfg.for_fch_rc as u32),
                    Some(cfg.rev_fch_rc as u32),
                )
            })
            .unwrap_or((None, None, None, None, None));
        Some(proto::traffic_event::Body::ServiceRequest(
            proto::TrafficServiceRequest {
                serv_req_seq: sr.serv_req_seq as u32,
                req_purpose: sr.req_purpose as u32,
                service_option: so,
                for_mux_option: for_mux,
                rev_mux_option: rev_mux,
                for_fch_rc: for_rc,
                rev_fch_rc: rev_rc,
            },
        ))
    } else if let Some(sc) = ev.service_connect.as_ref() {
        Some(proto::traffic_event::Body::ServiceConnect(
            proto::TrafficServiceConnect {
                serv_con_seq: sc.serv_con_seq as u32,
                for_mux_option: sc.for_mux_option as u32,
                rev_mux_option: sc.rev_mux_option as u32,
                for_rates: sc.for_rates as u32,
                rev_rates: sc.rev_rates as u32,
                connections: sc
                    .connections
                    .iter()
                    .map(|c| proto::TrafficServiceConnectConnection {
                        con_ref: c.con_ref as u32,
                        service_option: c.service_option as u32,
                        for_traffic: c.for_traffic as u32,
                        rev_traffic: c.rev_traffic as u32,
                        ui_encrypt_mode: c.ui_encrypt_mode as u32,
                        sr_id: c.sr_id as u32,
                        rlp_info_incl: c.rlp_info_incl,
                    })
                    .collect(),
                fch_frame_size: Some(sc.fch_frame_size as u32),
                for_fch_rc: Some(sc.for_fch_rc as u32),
                rev_fch_rc: Some(sc.rev_fch_rc as u32),
                non_neg_hex: sc.non_neg.as_ref().map(|nn| {
                    nn.encode()
                        .iter()
                        .map(|byte| format!("{:02X}", byte))
                        .collect::<String>()
                }),
            },
        ))
    } else if let Some(db) = ev.data_burst.as_ref() {
        let decoded_sms = if db.burst_type == 3 {
            cdma_common::sms::decode_mt_sms(&db.fields).map(|d| proto::DecodedSms {
                teleservice_id: d.teleservice_id as u32,
                destination_number: String::new(),
                originating_number: d.originating_number,
                message_type: d.message_type as u32,
                message_id: d.message_id as u32,
                text: if d.tl_msg_type == 0x02 {
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
        Some(proto::traffic_event::Body::DataBurst(
            proto::PagingDataBurst {
                burst_type: db.burst_type as u32,
                msg_number: db.msg_number as u32,
                num_msgs: db.num_msgs as u32,
                payload_bytes: db.fields.len() as u32,
                decoded_sms,
            },
        ))
    } else if let Some(awim) = ev.alert_with_info.as_ref() {
        let signal_info = awim.signal_info.as_ref().map(|sig| {
            let signal_type_name = match sig.signal_type {
                0x00 => "Tone Signal",
                0x01 => "IS-54B Alerting",
                0x02 => "IS-54B ISDN Alerting",
                0x03 => "IS-54B IS-CP Alerting",
                _ => "Unknown",
            };
            let alert_pitch_name = match sig.alert_pitch {
                0x00 => "Medium",
                0x01 => "High",
                0x02 => "Low",
                _ => "Reserved",
            };
            let signal_name = match (sig.signal_type, sig.signal) {
                (0x01, 0x00) => "Normal Ringback",
                (0x01, 0x01) => "Intergroup Ringback",
                (0x01, 0x02) => "Special/Priority Ringback",
                (0x01, 0x03) => "No Ringback",
                (0x00, 0x00) => "Dial Tone",
                (0x00, 0x01) => "Ringback Tone",
                (0x00, 0x02) => "Intercept Tone",
                (0x00, 0x03) => "Abbreviated Intercept",
                (0x00, 0x04) => "Reorder Tone",
                (0x00, 0x3F) => "Tones Off",
                _ => "Unknown",
            };
            proto::TrafficSignalInfoRecord {
                signal_type: sig.signal_type as u32,
                alert_pitch: sig.alert_pitch as u32,
                signal: sig.signal as u32,
                signal_type_name: signal_type_name.to_string(),
                alert_pitch_name: alert_pitch_name.to_string(),
                signal_name: signal_name.to_string(),
            }
        });
        let calling_party =
            awim.calling_party
                .as_ref()
                .map(|cpn| proto::TrafficCallingPartyRecord {
                    number_type: cpn.number_type as u32,
                    number_plan: cpn.number_plan as u32,
                    presentation_indicator: cpn.presentation_indicator as u32,
                    screening_indicator: cpn.screening_indicator as u32,
                    digits: cpn.digits.clone(),
                });
        let mut num_records = 0u32;
        if awim.signal_info.is_some() {
            num_records += 1;
        }
        if awim.calling_party.is_some() {
            num_records += 1;
        }
        Some(proto::traffic_event::Body::AlertWithInfo(
            proto::TrafficAlertWithInfo {
                num_info_records: num_records,
                signal_info,
                calling_party,
            },
        ))
    } else {
        None
    };

    proto::TrafficEvent {
        header: Some(header),
        timestamp_us: ev.timestamp_us,
        event_id: ev.event_id.clone(),
        walsh_code: ev.walsh_code as u32,
        service_option: ev.service_option.map(|v| v as u32),
        channel_name: format!("F-TCH W{}", ev.walsh_code),
        rc_name: ev.rc_label.clone(),
        address: Some(ev.address.clone()),
        l3_summary: ev.l3_summary.clone(),
        pdu_summary: ev.pdu_summary.clone(),
        sdu_hex: Some(ev.sdu_hex.clone()),
        pdu_hex: Some(ev.pdu_hex.clone()),
        body,
        voice_call_state: ev.voice_call_state.clone(),
        cell: Some(to_proto_cell_id(ev.cell)),
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
                imsi_m_s2: *imsi_m_s2 as u32,
            })),
        },
        MsAddress::ImsiClass0 {
            imsi_m_s1,
            imsi_m_s2,
            ..
        } => proto::PagingAddress {
            addr: Some(proto::paging_address::Addr::ImsiClass0(proto::ImsiClass0 {
                imsi_m_s1: *imsi_m_s1,
                imsi_m_s2: *imsi_m_s2 as u32,
            })),
        },
    }
}

fn ms_address_to_mobile_forward_proto(addr: &MsAddress) -> proto::MobileForwardAddress {
    match addr {
        MsAddress::Esn(esn) => proto::MobileForwardAddress {
            addr: Some(proto::mobile_forward_address::Addr::Esn(*esn)),
        },
        MsAddress::ImsiS {
            imsi_m_s1,
            imsi_m_s2,
        } => proto::MobileForwardAddress {
            addr: Some(proto::mobile_forward_address::Addr::ImsiS(proto::ImsiS {
                imsi_m_s1: *imsi_m_s1,
                imsi_m_s2: *imsi_m_s2 as u32,
            })),
        },
        MsAddress::ImsiClass0 {
            imsi_m_s1,
            imsi_m_s2,
            ..
        } => proto::MobileForwardAddress {
            addr: Some(proto::mobile_forward_address::Addr::ImsiClass0(
                proto::ImsiClass0 {
                    imsi_m_s1: *imsi_m_s1,
                    imsi_m_s2: *imsi_m_s2 as u32,
                },
            )),
        },
    }
}

fn ms_page_address_to_proto(
    addr: &cdma_common::lac::paging_messages::MsPageAddress,
) -> proto::MobilePageAddress {
    match addr {
        cdma_common::lac::paging_messages::MsPageAddress::Esn(esn) => proto::MobilePageAddress {
            addr: Some(proto::mobile_page_address::Addr::Esn(*esn)),
        },
        cdma_common::lac::paging_messages::MsPageAddress::ImsiS {
            imsi_m_s1,
            imsi_m_s2,
            mcc,
            imsi_11_12,
        } => proto::MobilePageAddress {
            addr: Some(proto::mobile_page_address::Addr::ImsiS(
                proto::MobilePageImsiS {
                    imsi_m_s1: *imsi_m_s1,
                    imsi_m_s2: *imsi_m_s2 as u32,
                    mcc: mcc.map(|v| v as u32),
                    imsi_11_12: imsi_11_12.map(|v| v as u32),
                },
            )),
        },
    }
}

fn to_proto_paging_event(ev: &PagingEvent) -> proto::PagingEvent {
    let mcsb = &ev.mcsb;

    let header = proto::PagingPduHeader {
        msg_tag: mcsb
            .message_id
            .wire_type(WireChannel::ForwardCommon)
            .unwrap_or(0) as u32,
        msg_type_name: mcsb.message_id.name().to_string(),
        sdu_length_bits: mcsb.length_bits as u32,
        address: mcsb.address.as_ref().map(ms_address_to_proto),
        msg_seq: mcsb.msg_seq as u32,
        ack_seq: mcsb.ack_seq as u32,
        ack_req: mcsb.ack_req,
        valid_ack: mcsb.valid_ack,
        resolved_address: mcsb
            .address
            .as_ref()
            .map(crate::addressing::format_ms_address)
            .unwrap_or_default(),
    };

    let body = match &ev.message {
        PagingChannelMessage::SystemParameters(m) => Some(
            proto::paging_event::Body::SystemParameters(proto::PagingSystemParameters {
                pilot_pn: m.pilot_pn as u32,
                sid: m.sid as u32,
                nid: m.nid as u32,
                base_id: m.base_id as u32,
                reg_zone: m.reg_zone as u32,
                total_zones: m.total_zones as u32,
                page_chan: m.page_chan as u32,
                max_slot_cycle_index: m.max_slot_cycle_index as u32,
                power_up_reg: m.power_up_reg,
                parameter_reg: m.parameter_reg,
            }),
        ),
        PagingChannelMessage::AccessParameters(m) => Some(
            proto::paging_event::Body::AccessParameters(proto::PagingAccessParameters {
                pilot_pn: m.pilot_pn as u32,
                acc_chan: m.acc_chan as u32,
                nom_pwr: m.nom_pwr as i32,
                init_pwr: m.init_pwr as i32,
                pwr_step: m.pwr_step as u32,
                num_step: m.num_step as u32,
                max_cap_sz: m.max_cap_sz as u32,
                auth: m.auth as u32,
            }),
        ),
        PagingChannelMessage::NeighborList(m) => Some(proto::paging_event::Body::NeighborList(
            proto::PagingNeighborList {
                pilot_pn: m.pilot_pn as u32,
                pilot_inc: m.pilot_inc as u32,
                neighbors: m.neighbors.iter().map(|n| *n as u32).collect(),
            },
        )),
        PagingChannelMessage::CdmaChannelList(m) => Some(
            proto::paging_event::Body::CdmaChannelList(proto::PagingCdmaChannelList {
                pilot_pn: m.pilot_pn as u32,
                channels: m.channels.iter().map(|c| *c as u32).collect(),
            }),
        ),
        PagingChannelMessage::ExtendedSystemParameters(m) => {
            Some(proto::paging_event::Body::ExtendedSystemParameters(
                proto::PagingExtendedSystemParameters {
                    pilot_pn: m.pilot_pn as u32,
                    p_rev: m.p_rev as u32,
                    min_p_rev: m.min_p_rev as u32,
                    mcc: m.mcc as u32,
                    imsi_11_12: m.imsi_11_12 as u32,
                    use_tmsi: m.use_tmsi,
                    pref_msid_type: m.pref_msid_type as u32,
                    max_num_alt_so: m.max_num_alt_so as u32,
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
                            page_subclass: *page_subclass as u32,
                            msg_seq: *msg_seq as u32,
                            imsi_m_s1: *imsi_m_s1,
                            imsi_m_s2: imsi_m_s2.map(|v| v as u32),
                            mcc: mcc.map(|v| v as u32),
                            imsi_addr_num: imsi_addr_num.map(|v| v as u32),
                            special_service: *special_service,
                            service_option: service_option.map(|v| v as u32),
                            imsi_s: *imsi_s,
                        }),
                        GeneralPageRecord::Class1 {
                            msg_seq,
                            esn,
                            special_service,
                            service_option,
                        } => proto::page_record::Record::Class1(proto::PageRecordClass1 {
                            msg_seq: *msg_seq as u32,
                            esn: *esn,
                            special_service: *special_service,
                            service_option: service_option.map(|v| v as u32),
                        }),
                        GeneralPageRecord::Tmsi {
                            msg_seq,
                            tmsi_code_addr,
                            special_service,
                            service_option,
                        } => proto::page_record::Record::Tmsi(proto::PageRecordTmsi {
                            msg_seq: *msg_seq as u32,
                            tmsi_code_addr: *tmsi_code_addr,
                            special_service: *special_service,
                            service_option: service_option.map(|v| v as u32),
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
                    config_msg_seq: m.config_msg_seq as u32,
                    acc_msg_seq: m.acc_msg_seq as u32,
                    class_0_done: m.class_0_done,
                    class_1_done: m.class_1_done,
                    tmsi_done: m.tmsi_done,
                    page_records: records,
                },
            ))
        }
        PagingChannelMessage::Order(m) => {
            let order_name = forward_order_name(m.order);
            Some(proto::paging_event::Body::Order(proto::PagingOrder {
                order: m.order as u32,
                ordq: m.ordq as u32,
                order_name: order_name.to_string(),
            }))
        }
        PagingChannelMessage::DataBurst(m) => {
            let decoded_sms = if m.burst_type == 3 {
                cdma_common::sms::decode_mt_sms(&m.fields).map(|d| proto::DecodedSms {
                    teleservice_id: d.teleservice_id as u32,
                    destination_number: String::new(),
                    originating_number: d.originating_number,
                    message_type: d.message_type as u32,
                    message_id: d.message_id as u32,
                    text: if d.tl_msg_type == 0x02 {
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
                    burst_type: m.burst_type as u32,
                    msg_number: m.msg_number as u32,
                    num_msgs: m.num_msgs as u32,
                    payload_bytes: m.fields.len() as u32,
                    decoded_sms,
                },
            ))
        }
        PagingChannelMessage::AuthenticationChallenge(_) => None,
        PagingChannelMessage::SsdUpdate(_) => None,
        PagingChannelMessage::FeatureNotification(_) => None,
        PagingChannelMessage::ExtendedNeighborList(_) => None,
        PagingChannelMessage::StatusRequest(_) => None,
        PagingChannelMessage::ServiceRedirection(_) => None,
        PagingChannelMessage::GlobalServiceRedirection(_) => None,
        PagingChannelMessage::TmsiAssignment(_) => None,
        PagingChannelMessage::Paca(_) => None,
        PagingChannelMessage::GeneralNeighborList(_) => None,
        PagingChannelMessage::UserZoneIdentification(_) => None,
        PagingChannelMessage::PrivateNeighborList(_) => None,
        PagingChannelMessage::ExtendedGlobalServiceRedirection(_) => None,
        PagingChannelMessage::ExtendedCdmaChannelList(_) => None,
        PagingChannelMessage::UserZoneReject(_) => None,
        PagingChannelMessage::Ansi41SystemParameters(_) => None,
        PagingChannelMessage::McRrParameters(_) => None,
        PagingChannelMessage::Ansi41Rand(_) => None,
        PagingChannelMessage::EnhancedAccessParameters(_) => None,
        PagingChannelMessage::UniversalNeighborList(_) => None,
        PagingChannelMessage::SecurityModeCommand(_) => None,
        PagingChannelMessage::UniversalPage(_) => None,
        PagingChannelMessage::UniversalPageFirstSegment(_) => None,
        PagingChannelMessage::UniversalPageMiddleSegment(_) => None,
        PagingChannelMessage::UniversalPageFinalSegment(_) => None,
        PagingChannelMessage::AuthenticationRequest(_) => None,
        PagingChannelMessage::AlternativeTechnologiesInformation(_) => None,
        PagingChannelMessage::GeneralExtension(_) => None,
        PagingChannelMessage::GeneralOverheadInformation(_) => None,
        PagingChannelMessage::AccessPointIdentifier(_) => None,
        PagingChannelMessage::AccessPointIdentifierText(_) => None,
        PagingChannelMessage::AccessPointPilotInformation(_) => None,
        PagingChannelMessage::FlexDuplexCdmaChannelList(_) => None,
        PagingChannelMessage::BroadcastServiceParameters(_) => None,
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
                    assign_mode: m.assign_mode as u32,
                    code_chan: m.code_chan as u32,
                    frame_offset: m.frame_offset as u32,
                    encrypt_mode: m.encrypt_mode as u32,
                    freq_incl: m.freq_incl,
                    band_class: m.band_class.map(|v| v as u32),
                    cdma_freq: m.cdma_freq.map(|v| v as u32),
                    bypass_alert_answer: m.bypass_alert_answer,
                    default_config: m.default_config.map(|v| v as u32),
                    granted_mode: m.granted_mode.map(|v| v as u32),
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
                    plcm_type: m.plcm_type.map(|v| v as u32),
                    early_rl_transmit_ind: None,
                    tx_pwr_limit: None,
                    pilots: Vec::new(),
                    sdu_hex: Some(bitstream_to_hex(&m.to_sdu())),
                },
            ))
        }
        PagingChannelMessage::ExtendedChannelAssignment(m) => Some(
            proto::paging_event::Body::ChannelAssignment(proto::PagingChannelAssignment {
                assign_mode: m.assign_mode as u32,
                code_chan: m
                    .pilots
                    .first()
                    .map(|p| p.code_chan_fch as u32)
                    .unwrap_or_default(),
                frame_offset: m.frame_offset as u32,
                encrypt_mode: m.encrypt_mode as u32,
                freq_incl: m.freq_incl,
                band_class: m.band_class.map(|v| v as u32),
                cdma_freq: m.cdma_freq.map(|v| v as u32),
                bypass_alert_answer: Some(m.bypass_alert_answer),
                default_config: Some(m.default_config as u32),
                granted_mode: Some(m.granted_mode as u32),
                assign_mode_name: "ECAM".to_string(),
                default_config_name: if m.default_config == CHANNEL_DEFAULT_CONFIG_EXPLICIT_RCS {
                    "Explicit FOR_RC/REV_RC".to_string()
                } else {
                    "".to_string()
                },
                direct_ch_assign_ind: Some(m.direct_ch_assign_ind),
                for_rc: Some(m.for_rc as u32),
                rev_rc: Some(m.rev_rc as u32),
                fpc_subchan_gain: Some(m.fpc_subchan_gain as u32),
                rlgain_adj: Some(m.rlgain_adj as i32),
                ch_ind: Some(m.ch_ind as u32),
                ch_record_len_octets: Some(m.ch_record_len_octets() as u32),
                fpc_fch_init_setpt: Some(m.fpc_fch_init_setpt as u32),
                fpc_fch_fer: Some(m.fpc_fch_fer as u32),
                fpc_fch_min_setpt: Some(m.fpc_fch_min_setpt as u32),
                fpc_fch_max_setpt: Some(m.fpc_fch_max_setpt as u32),
                rev_fch_gating_mode: Some(m.rev_fch_gating_mode),
                plcm_type: Some(m.plcm_type as u32),
                early_rl_transmit_ind: Some(m.early_rl_transmit_ind),
                tx_pwr_limit: m.tx_pwr_limit.map(|v| v as u32),
                pilots: m
                    .pilots
                    .iter()
                    .map(|pilot| proto::PagingTrafficPilot {
                        pilot_pn: pilot.pilot_pn as u32,
                        pwr_comb_ind: pilot.pwr_comb_ind,
                        code_chan_fch: pilot.code_chan_fch as u32,
                        qof_mask_id_fch: pilot.qof_mask_id_fch as u32,
                    })
                    .collect(),
                sdu_hex: Some(bitstream_to_hex(&m.to_sdu())),
            }),
        ),
    };

    proto::PagingEvent {
        header: Some(header),
        timestamp_us: ev.timestamp_us,
        event_id: ev.event_id.clone(),
        body,
        // The Abis-level paging event does not say which cell carried the
        // record. The per-cell transmission stream carries the cell that
        // actually put it on air.
        cell: None,
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

type GrpcStream<T> = Pin<Box<dyn Stream<Item = Result<T, Status>> + Send>>;

#[tonic::async_trait]
impl BscService for BscServiceImpl {
    async fn get_system_status(
        &self,
        _: Request<()>,
    ) -> Result<Response<proto::SystemStatus>, Status> {
        // The BSC is up whatever the cell count, so report that regardless.
        // Fill per-cell identity only for a sole enrolled cell — with several,
        // the per-cell list carries identity and these stay zero.
        let params = self.state.bts.sole_entry().map(|entry| entry.params());
        let overhead = params.as_ref().map(|params| &params.overhead);
        Ok(Response::new(proto::SystemStatus {
            running: true,
            sid: overhead.map_or(0, |oh| u32::from(oh.sid)),
            nid: overhead.map_or(0, |oh| u32::from(oh.nid)),
            base_id: overhead.map_or(0, |oh| u32::from(oh.base_id)),
            pilot_pn: params
                .as_ref()
                .map_or(0, |params| params.pilot_offset as u32),
            reg_zone: overhead.map_or(0, |oh| u32::from(oh.reg_zone)),
        }))
    }

    async fn get_config(&self, _: Request<()>) -> Result<Response<proto::BtsConfig>, Status> {
        <Self as BtsManagementService>::get_bts_config(self, any_cell()).await
    }

    async fn get_radio_metrics(
        &self,
        _: Request<()>,
    ) -> Result<Response<proto::RadioMetrics>, Status> {
        <Self as BtsManagementService>::get_radio_metrics(self, any_cell()).await
    }

    async fn get_iq_capture_status(
        &self,
        _: Request<()>,
    ) -> Result<Response<proto::IqCaptureStatus>, Status> {
        <Self as BtsManagementService>::get_iq_capture_status(self, any_cell()).await
    }

    async fn start_iq_capture(
        &self,
        _: Request<()>,
    ) -> Result<Response<proto::IqCaptureStatus>, Status> {
        <Self as BtsManagementService>::start_iq_capture(self, any_cell()).await
    }

    async fn stop_iq_capture(
        &self,
        _: Request<()>,
    ) -> Result<Response<proto::IqCaptureStatus>, Status> {
        <Self as BtsManagementService>::stop_iq_capture(self, any_cell()).await
    }

    type StreamRadioMetricsStream = GrpcStream<proto::RadioMetrics>;

    async fn stream_radio_metrics(
        &self,
        _: Request<()>,
    ) -> Result<Response<Self::StreamRadioMetricsStream>, Status> {
        <Self as BtsManagementService>::stream_radio_metrics(self, any_cell()).await
    }

    type StreamAccessEventsStream = GrpcStream<proto::AccessEvent>;

    async fn stream_access_events(
        &self,
        _: Request<()>,
    ) -> Result<Response<Self::StreamAccessEventsStream>, Status> {
        let mut rx = self.state.access_broadcast.subscribe();
        let stream = async_stream::stream! {
            while let Ok(event) = rx.recv().await {
                if !should_stream_access_event(&event) {
                    continue;
                }
                yield Ok(to_proto_access_event(&event));
            }
        };
        Ok(Response::new(Box::pin(stream)))
    }

    type StreamPagingEventsStream = GrpcStream<proto::PagingEvent>;

    /// Streams both what the BSC sent toward the paging channel and what each
    /// BTS actually transmitted on it.
    async fn stream_paging_events(
        &self,
        _: Request<()>,
    ) -> Result<Response<Self::StreamPagingEventsStream>, Status> {
        let mut rx = self.state.paging_broadcast.subscribe();
        let mut pch_rx = self.pch_transmissions.subscribe();
        let stream = async_stream::stream! {
            loop {
                tokio::select! {
                    event = rx.recv() => match event {
                        Ok(event) => yield Ok(to_proto_paging_event(&event)),
                        Err(broadcast::error::RecvError::Lagged(_)) => {}
                        Err(broadcast::error::RecvError::Closed) => break,
                    },
                    event = pch_rx.recv() => match event {
                        Ok(event) => yield Ok(event),
                        Err(broadcast::error::RecvError::Lagged(_)) => {}
                        Err(broadcast::error::RecvError::Closed) => break,
                    },
                }
            }
        };
        Ok(Response::new(Box::pin(stream)))
    }

    type StreamTrafficEventsStream = GrpcStream<proto::TrafficEvent>;

    async fn stream_traffic_events(
        &self,
        _: Request<()>,
    ) -> Result<Response<Self::StreamTrafficEventsStream>, Status> {
        let mut rx = self.state.traffic_broadcast.subscribe();
        let stream = async_stream::stream! {
            while let Ok(event) = rx.recv().await {
                yield Ok(to_proto_traffic_event(&event));
            }
        };
        Ok(Response::new(Box::pin(stream)))
    }

    async fn list_mobiles(&self, _: Request<()>) -> Result<Response<proto::MobileList>, Status> {
        let mobiles = self.state.mobiles.borrow().clone();
        let bts_power = self.reverse_power_by_cell_walsh().await;
        let default_cell = self.default_cell();
        Ok(Response::new(proto::MobileList {
            mobiles: mobiles
                .into_iter()
                .map(|m| {
                    let cell = m.serving_cell.or(default_cell);
                    let traffic_power = m
                        .traffic_walsh_code
                        .zip(cell)
                        .and_then(|(walsh, cell)| bts_power.get(&(cell, walsh)).cloned())
                        .map(|power| merge_bsc_power_fields(power, m.traffic_power.as_ref()))
                        .or_else(|| m.traffic_power.as_ref().map(to_proto_traffic_channel_power));
                    proto::MobileInfo {
                        address: m.address,
                        page_address: m.page_address,
                        state: m.state,
                        mob_p_rev: m.mob_p_rev as u32,
                        esn: m.esn,
                        imsi: m.imsi.clone(),
                        meid: m.meid.clone(),
                        pgslot: m.pgslot.map(|v| v as u32),
                        slot_cycle_index: m.slot_cycle_index as u32,
                        snr_db: m.snr_db,
                        signal_power_db: m.signal_power_db,
                        demod_quality_pct: m.demod_quality_pct,
                        last_heard_ms: m.last_heard_ms,
                        rx_power_dbm: m.rx_power_dbm,
                        rx_level_dbfs: m.rx_level_dbfs,
                        forward_address: m
                            .forward_address
                            .as_ref()
                            .map(ms_address_to_mobile_forward_proto),
                        page_address_detail: m
                            .page_address_detail
                            .as_ref()
                            .map(ms_page_address_to_proto),
                        phone_number: m.phone_number.clone(),
                        subscriber_display_name: m.subscriber_display_name.clone(),
                        subscriber_id: m.subscriber_id.clone(),
                        traffic_walsh_code: m.traffic_walsh_code.map(|w| w as u32),
                        traffic_service_option: m.traffic_service_option.map(|s| s as u32),
                        voice_call_state: m.voice_call_state.clone(),
                        traffic_power,
                        serving_cell: m.serving_cell.map(to_proto_cell_id),
                    }
                })
                .collect(),
        }))
    }

    /// Every cell's local radio resources plus the traffic channels the BSC
    /// has mobiles on, so one list covers the whole BSC.
    async fn list_channels(&self, _: Request<()>) -> Result<Response<proto::ChannelList>, Status> {
        let mut channels = Vec::new();
        let mut cell_walsh_capacity = Vec::new();
        for entry in self.state.bts.entries() {
            let Some(mut client) = entry.oam() else {
                continue;
            };
            let cell = to_proto_cell_id(entry.cell());
            match client
                .list_local_radio_resources(CellRequest {
                    cell: Some(cell),
                    peer_id: None,
                })
                .await
            {
                Ok(response) => {
                    let list = response.into_inner();
                    cell_walsh_capacity.push(proto::CellWalshCapacity {
                        cell: Some(cell),
                        total_walsh_codes: list.total_walsh_codes,
                    });
                    channels.extend(
                        list.channels
                            .into_iter()
                            .filter(|c| c.channel_type != TRAFFIC_CHANNEL_TYPE),
                    );
                }
                Err(e) => warn!(
                    "cell {}/{} local radio resource list failed: {e}",
                    cell.cell, cell.sector
                ),
            }
        }

        let mobiles = self.state.mobiles.borrow().clone();
        let bts_power = self.reverse_power_by_cell_walsh().await;
        let default_cell = self.default_cell();
        for m in &mobiles {
            if let Some(walsh) = m.traffic_walsh_code {
                let channel_cell = m.serving_cell.or(default_cell);
                let traffic_power = channel_cell
                    .and_then(|cell| bts_power.get(&(cell, walsh)).cloned())
                    .map(|power| merge_bsc_power_fields(power, m.traffic_power.as_ref()))
                    .or_else(|| m.traffic_power.as_ref().map(to_proto_traffic_channel_power));
                channels.push(proto::Channel {
                    walsh_code: Some(walsh as u32),
                    channel_type: TRAFFIC_CHANNEL_TYPE.into(),
                    direction: "forward".into(),
                    power_fraction: None,
                    data_rate_bps: None,
                    paging_channel_number: None,
                    access_channel_number: None,
                    mobile: Some(proto::ChannelMobile {
                        address: m.address.clone(),
                        state: m.state.clone(),
                        phone_number: m.phone_number.clone(),
                        snr_db: m.snr_db,
                        rx_power_dbm: m.rx_power_dbm,
                        rx_level_dbfs: m.rx_level_dbfs,
                        signal_power_db: m.signal_power_db,
                        demod_quality_pct: m.demod_quality_pct,
                        voice_call_state: m.voice_call_state.clone(),
                    }),
                    service_option: m.traffic_service_option.map(|s| s as u32),
                    traffic_power,
                    cell: channel_cell.map(to_proto_cell_id),
                });
            }
        }

        channels.sort_by_key(|c| (c.direction.clone(), c.walsh_code.unwrap_or(u32::MAX)));

        Ok(Response::new(proto::ChannelList {
            channels,
            total_walsh_codes: cell_walsh_capacity
                .iter()
                .map(|entry| entry.total_walsh_codes)
                .sum(),
            cell_walsh_capacity,
        }))
    }

    async fn set_traffic_channel_power_override(
        &self,
        request: Request<proto::SetTrafficChannelPowerOverrideRequest>,
    ) -> Result<Response<proto::SetTrafficChannelPowerOverrideResponse>, Status> {
        let req = request.into_inner();
        let walsh_code = u8::try_from(req.walsh_code)
            .map_err(|_| Status::invalid_argument("walsh_code must fit in u8"))?;
        // Each cell allocates Walsh codes from its own pool, so the code alone
        // does not identify a channel. An unnamed cell resolves to the only
        // enrolled one, which is how the rest of the cell-addressed API reads
        // an absent cell.
        let requested_cell = match req.cell {
            Some(cell) => Some(from_proto_cell_id(&cell)?),
            None => self
                .state
                .bts
                .sole_entry()
                .map(|entry| entry.cell())
                .ok_or_else(|| {
                    Status::failed_precondition(
                        "more than one cell is enrolled, so the request must name one",
                    )
                })
                .map(Some)?,
        };
        let bsc_snapshot = {
            self.state
                .mobiles
                .borrow()
                .iter()
                .find(|mobile| {
                    mobile.traffic_walsh_code == Some(walsh_code)
                        && mobile.serving_cell == requested_cell
                })
                .map(|mobile| mobile.traffic_power.clone())
                .ok_or_else(|| Status::not_found("active traffic channel not found"))?
        };

        let (cell, mut client) = self.oam_for(requested_cell.map(to_proto_cell_id))?;
        // Forward with the resolved cell so the BTS-side check sees the cell
        // this BSC actually addressed.
        let mut response = client
            .set_reverse_power_control_override(proto::SetTrafficChannelPowerOverrideRequest {
                cell: Some(cell),
                ..req
            })
            .await?
            .into_inner();
        response.traffic_power = response
            .traffic_power
            .map(|power| merge_bsc_power_fields(power, bsc_snapshot.as_ref()));
        Ok(Response::new(response))
    }

    async fn initiate_data_call(
        &self,
        request: Request<proto::InitiateDataCallRequest>,
    ) -> Result<Response<proto::InitiateDataCallResponse>, Status> {
        let req = request.into_inner();
        let subscriber_id = Uuid::parse_str(&req.subscriber_id)
            .map_err(|_| Status::invalid_argument("subscriber_id must be a valid UUID"))?;
        let service_option = match req.service_option {
            value if value == u32::from(SERVICE_OPTION_PACKET_DATA) => SERVICE_OPTION_PACKET_DATA,
            value if value == u32::from(SERVICE_OPTION_ASYNC_DATA) => SERVICE_OPTION_ASYNC_DATA,
            _ => SERVICE_OPTION_HIGH_RATE_PACKET_DATA,
        };

        self.state
            .data_request_tx
            .send(DataCallRequest {
                subscriber_id,
                service_option,
            })
            .await
            .map_err(|e| Status::unavailable(format!("data request queue unavailable: {}", e)))?;

        Ok(Response::new(proto::InitiateDataCallResponse {
            accepted: true,
            message: format!(
                "data call (SO {}) request accepted for subscriber {}",
                service_option, subscriber_id
            ),
        }))
    }
}

#[tonic::async_trait]
impl ManagementFacadeService for BscServiceImpl {
    async fn get_system_overview(
        &self,
        request: Request<()>,
    ) -> Result<Response<SystemOverview>, Status> {
        // Node health matters most when no cell resolves — a BTS down, or
        // several enrolled — which is exactly when per-cell system status
        // refuses. The identity block is therefore optional, not a
        // precondition for the whole overview.
        let status = <Self as BscService>::get_system_status(self, request)
            .await
            .ok()
            .map(|response| response.into_inner());
        let mut nodes: Vec<NodeHealth> = self
            .state
            .bts
            .summaries()
            .into_iter()
            .map(|summary| NodeHealth {
                node_id: format!("bts-{}-{}", summary.cell.cell, summary.cell.sector),
                node_type: "BTS".into(),
                healthy: summary.state == BtsAttachState::InService,
                message: summary
                    .status_detail
                    .unwrap_or_else(|| attach_state_message(summary.state).to_string()),
            })
            .collect();
        nodes.push(NodeHealth {
            node_id: self.state.node_id.clone(),
            node_type: "BSC".into(),
            healthy: true,
            message: "running".into(),
        });
        Ok(Response::new(SystemOverview {
            bsc_status: status,
            nodes,
        }))
    }

    type StreamSystemEventsStream = GrpcStream<ManagementEvent>;

    async fn stream_system_events(
        &self,
        _: Request<()>,
    ) -> Result<Response<Self::StreamSystemEventsStream>, Status> {
        let mut access_rx = self.state.access_broadcast.subscribe();
        let mut paging_rx = self.state.paging_broadcast.subscribe();
        let mut pch_rx = self.pch_transmissions.subscribe();
        let mut traffic_rx = self.state.traffic_broadcast.subscribe();
        let node_id = self.state.node_id.clone();
        let mut metrics = self.radio_metrics_stream().await;

        let stream = async_stream::stream! {
            loop {
                tokio::select! {
                    metrics_event = metrics.next() => {
                        match metrics_event {
                            Some(Ok(radio_metrics)) => {
                                yield Ok(ManagementEvent {
                                    source_node_id: node_id.clone(),
                                    source_node_type: "BTS".into(),
                                    classification: "telemetry".into(),
                                    body: Some(management_event::Body::RadioMetrics(radio_metrics)),
                                });
                            }
                            Some(Err(e)) => {
                                warn!("radio metrics stream ended: {e}");
                                metrics = Box::pin(tokio_stream::pending());
                            }
                            None => metrics = Box::pin(tokio_stream::pending()),
                        }
                    }
                    event = access_rx.recv() => {
                        match event {
                            Ok(event) => {
                                if should_stream_access_event(&event) {
                                    yield Ok(ManagementEvent {
                                        source_node_id: node_id.clone(),
                                        source_node_type: "BSC".into(),
                                        classification: "standards_event_with_diagnostics".into(),
                                        body: Some(management_event::Body::AccessEvent(to_proto_access_event(&event))),
                                    });
                                }
                            }
                            Err(broadcast::error::RecvError::Lagged(_)) => {}
                            Err(broadcast::error::RecvError::Closed) => break,
                        }
                    }
                    event = paging_rx.recv() => {
                        match event {
                            Ok(event) => {
                                yield Ok(ManagementEvent {
                                    source_node_id: node_id.clone(),
                                    source_node_type: "BSC".into(),
                                    classification: "standards_event_with_diagnostics".into(),
                                    body: Some(management_event::Body::PagingEvent(to_proto_paging_event(&event))),
                                });
                            }
                            Err(broadcast::error::RecvError::Lagged(_)) => {}
                            Err(broadcast::error::RecvError::Closed) => break,
                        }
                    }
                    event = pch_rx.recv() => {
                        match event {
                            Ok(event) => {
                                yield Ok(ManagementEvent {
                                    source_node_id: node_id.clone(),
                                    source_node_type: "BTS".into(),
                                    classification: "standards_event_with_diagnostics".into(),
                                    body: Some(management_event::Body::PagingEvent(event)),
                                });
                            }
                            Err(broadcast::error::RecvError::Lagged(_)) => {}
                            Err(broadcast::error::RecvError::Closed) => break,
                        }
                    }
                    event = traffic_rx.recv() => {
                        match event {
                            Ok(event) => {
                                yield Ok(ManagementEvent {
                                    source_node_id: node_id.clone(),
                                    source_node_type: "BSC".into(),
                                    classification: "standards_event_with_diagnostics".into(),
                                    body: Some(management_event::Body::TrafficEvent(to_proto_traffic_event(&event))),
                                });
                            }
                            Err(broadcast::error::RecvError::Lagged(_)) => {}
                            Err(broadcast::error::RecvError::Closed) => break,
                        }
                    }
                }
            }
        };

        Ok(Response::new(Box::pin(stream)))
    }
}

/// Per-cell forwarding implementation of the API each BTS serves for itself.
///
/// A request that names no cell is answered by the sole enrolled cell, so a
/// single-cell deployment addresses the BSC exactly as it addresses a BTS.
#[tonic::async_trait]
impl BtsManagementService for BscServiceImpl {
    async fn enroll(&self, _: Request<EnrollRequest>) -> Result<Response<EnrollResponse>, Status> {
        let (_, mut client) = self.oam_for(None)?;
        client.enroll(EnrollRequest {}).await
    }

    async fn list_bts(&self, _: Request<()>) -> Result<Response<BtsList>, Status> {
        let mobiles = self.state.mobiles.borrow().clone();
        let default_cell = self.default_cell();
        // One row per configured peer, so offline peers still show.
        let cell_summaries: std::collections::HashMap<_, _> = self
            .state
            .bts
            .summaries()
            .into_iter()
            .map(|summary| (summary.cell, summary))
            .collect();
        let bts = self
            .state
            .bts
            .peer_summaries()
            .into_iter()
            .map(
                |peer| match peer.cell.and_then(|cell| cell_summaries.get(&cell)) {
                    Some(summary) => {
                        let served_mobiles = mobiles
                            .iter()
                            .filter(|m| m.serving_cell.or(default_cell) == Some(summary.cell))
                            .count() as u32;
                        BtsSummary {
                            peer_id: peer.peer_id,
                            management_endpoint: peer.oam_endpoint,
                            cell: Some(to_proto_cell_id(summary.cell)),
                            state: to_proto_attach_state(summary.state) as i32,
                            pilot_pn: u32::from(summary.pilot_pn),
                            band_class: summary.band_class.clone(),
                            cdma_channel: u32::from(summary.cdma_channel),
                            sid: u32::from(summary.sid),
                            nid: u32::from(summary.nid),
                            evdo_enabled: summary.evdo_enabled,
                            evdo_color_code: summary.evdo_color_code.map(u32::from),
                            an_grpc_addr: summary.an_grpc_addr.clone(),
                            served_mobiles: Some(served_mobiles),
                            status_detail: summary.status_detail.clone(),
                        }
                    }
                    None => BtsSummary {
                        peer_id: peer.peer_id,
                        management_endpoint: peer.oam_endpoint,
                        cell: None,
                        state: to_proto_attach_state(peer.state) as i32,
                        pilot_pn: 0,
                        band_class: String::new(),
                        cdma_channel: 0,
                        sid: 0,
                        nid: 0,
                        evdo_enabled: false,
                        evdo_color_code: None,
                        an_grpc_addr: None,
                        served_mobiles: None,
                        status_detail: peer.status_detail,
                    },
                },
            )
            .collect();
        Ok(Response::new(BtsList { bts }))
    }

    async fn get_bts_status(
        &self,
        request: Request<CellRequest>,
    ) -> Result<Response<proto::SystemStatus>, Status> {
        let (cell, mut client) = self.oam_for_request(request.into_inner())?;
        client.get_bts_status(cell_request(cell)).await
    }

    async fn get_bts_config(
        &self,
        request: Request<CellRequest>,
    ) -> Result<Response<proto::BtsConfig>, Status> {
        let (cell, mut client) = self.oam_for_request(request.into_inner())?;
        client.get_bts_config(cell_request(cell)).await
    }

    async fn get_radio_metrics(
        &self,
        request: Request<CellRequest>,
    ) -> Result<Response<proto::RadioMetrics>, Status> {
        let (cell, mut client) = self.oam_for_request(request.into_inner())?;
        client.get_radio_metrics(cell_request(cell)).await
    }

    type StreamRadioMetricsStream = GrpcStream<proto::RadioMetrics>;

    async fn stream_radio_metrics(
        &self,
        request: Request<CellRequest>,
    ) -> Result<Response<Self::StreamRadioMetricsStream>, Status> {
        let (cell, mut client) = self.oam_for_request(request.into_inner())?;
        let stream = client
            .stream_radio_metrics(cell_request(cell))
            .await?
            .into_inner();
        Ok(Response::new(Box::pin(stream)))
    }

    async fn get_iq_capture_status(
        &self,
        request: Request<CellRequest>,
    ) -> Result<Response<proto::IqCaptureStatus>, Status> {
        let (cell, mut client) = self.oam_for_request(request.into_inner())?;
        client.get_iq_capture_status(cell_request(cell)).await
    }

    async fn start_iq_capture(
        &self,
        request: Request<CellRequest>,
    ) -> Result<Response<proto::IqCaptureStatus>, Status> {
        let (cell, mut client) = self.oam_for_request(request.into_inner())?;
        client.start_iq_capture(cell_request(cell)).await
    }

    async fn stop_iq_capture(
        &self,
        request: Request<CellRequest>,
    ) -> Result<Response<proto::IqCaptureStatus>, Status> {
        let (cell, mut client) = self.oam_for_request(request.into_inner())?;
        client.stop_iq_capture(cell_request(cell)).await
    }

    async fn list_local_radio_resources(
        &self,
        request: Request<CellRequest>,
    ) -> Result<Response<proto::ChannelList>, Status> {
        let (cell, mut client) = self.oam_for_request(request.into_inner())?;
        client.list_local_radio_resources(cell_request(cell)).await
    }

    async fn get_reverse_power_control(
        &self,
        request: Request<ReversePowerControlRequest>,
    ) -> Result<Response<proto::TrafficChannelPower>, Status> {
        let req = request.into_inner();
        let walsh_code = u8::try_from(req.walsh_code)
            .map_err(|_| Status::invalid_argument("walsh_code must fit in u8"))?;
        let (cell, mut client) = self.oam_for(req.cell)?;
        let power = client
            .get_reverse_power_control(ReversePowerControlRequest {
                walsh_code: req.walsh_code,
                cell: Some(cell),
            })
            .await?
            .into_inner();
        let bsc_snapshot = self.bsc_power_snapshot(from_proto_cell_id(&cell)?, walsh_code);
        Ok(Response::new(merge_bsc_power_fields(
            power,
            bsc_snapshot.as_ref(),
        )))
    }

    async fn list_reverse_power_controls(
        &self,
        request: Request<CellRequest>,
    ) -> Result<Response<ReversePowerControlList>, Status> {
        let (cell, mut client) = self.oam_for_request(request.into_inner())?;
        let cell_id = from_proto_cell_id(&cell)?;
        let list = client
            .list_reverse_power_controls(cell_request(cell))
            .await?
            .into_inner();
        let entries = list
            .entries
            .into_iter()
            .map(|entry| {
                let bsc_snapshot = u8::try_from(entry.walsh_code)
                    .ok()
                    .and_then(|walsh| self.bsc_power_snapshot(cell_id, walsh));
                ReversePowerControlEntry {
                    walsh_code: entry.walsh_code,
                    power: entry
                        .power
                        .map(|power| merge_bsc_power_fields(power, bsc_snapshot.as_ref())),
                }
            })
            .collect();
        Ok(Response::new(ReversePowerControlList { entries }))
    }

    async fn set_reverse_power_control_override(
        &self,
        request: Request<proto::SetTrafficChannelPowerOverrideRequest>,
    ) -> Result<Response<proto::SetTrafficChannelPowerOverrideResponse>, Status> {
        let req = request.into_inner();
        let (cell, mut client) = self.oam_for(req.cell.clone())?;
        let walsh_code = u8::try_from(req.walsh_code)
            .map_err(|_| Status::invalid_argument("walsh_code must fit in u8"))?;
        let bsc_snapshot = self.bsc_power_snapshot(from_proto_cell_id(&cell)?, walsh_code);
        // Forward with the resolved cell so the BTS-side check sees the cell
        // this BSC actually addressed.
        let mut response = client
            .set_reverse_power_control_override(proto::SetTrafficChannelPowerOverrideRequest {
                cell: Some(cell),
                ..req
            })
            .await?
            .into_inner();
        response.traffic_power = response
            .traffic_power
            .map(|power| merge_bsc_power_fields(power, bsc_snapshot.as_ref()));
        Ok(Response::new(response))
    }

    type StreamPchTransmissionsStream = GrpcStream<proto::PagingEvent>;

    async fn stream_pch_transmissions(
        &self,
        request: Request<CellRequest>,
    ) -> Result<Response<Self::StreamPchTransmissionsStream>, Status> {
        let (cell, mut client) = self.oam_for_request(request.into_inner())?;
        let stream = client
            .stream_pch_transmissions(cell_request(cell))
            .await?
            .into_inner();
        Ok(Response::new(Box::pin(stream)))
    }
}

#[tonic::async_trait]
impl BscManagementService for BscServiceImpl {
    async fn get_bsc_status(
        &self,
        request: Request<()>,
    ) -> Result<Response<proto::SystemStatus>, Status> {
        <Self as BscService>::get_system_status(self, request).await
    }

    async fn list_mobiles(
        &self,
        request: Request<()>,
    ) -> Result<Response<proto::MobileList>, Status> {
        <Self as BscService>::list_mobiles(self, request).await
    }

    async fn list_channels(
        &self,
        request: Request<()>,
    ) -> Result<Response<proto::ChannelList>, Status> {
        <Self as BscService>::list_channels(self, request).await
    }

    type StreamAccessEventsStream = GrpcStream<proto::AccessEvent>;

    async fn stream_access_events(
        &self,
        request: Request<()>,
    ) -> Result<Response<Self::StreamAccessEventsStream>, Status> {
        <Self as BscService>::stream_access_events(self, request).await
    }

    type StreamPagingEventsStream = GrpcStream<proto::PagingEvent>;

    async fn stream_paging_events(
        &self,
        request: Request<()>,
    ) -> Result<Response<Self::StreamPagingEventsStream>, Status> {
        <Self as BscService>::stream_paging_events(self, request).await
    }

    type StreamTrafficEventsStream = GrpcStream<proto::TrafficEvent>;

    async fn stream_traffic_events(
        &self,
        request: Request<()>,
    ) -> Result<Response<Self::StreamTrafficEventsStream>, Status> {
        <Self as BscService>::stream_traffic_events(self, request).await
    }

    async fn set_traffic_channel_power_override(
        &self,
        request: Request<proto::SetTrafficChannelPowerOverrideRequest>,
    ) -> Result<Response<proto::SetTrafficChannelPowerOverrideResponse>, Status> {
        <Self as BscService>::set_traffic_channel_power_override(self, request).await
    }
}

fn load_server_tls_config(
    mtls: &MtlsConfig,
) -> Result<ServerTlsConfig, Box<dyn std::error::Error>> {
    let cert = std::fs::read(&mtls.cert_path)?;
    let key = std::fs::read(&mtls.key_path)?;
    let client_ca = std::fs::read(&mtls.client_ca_path)?;
    Ok(ServerTlsConfig::new()
        .identity(Identity::from_pem(cert, key))
        .client_ca_root(Certificate::from_pem(client_ca)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use cdma_common::lac::paging_messages::ChannelAssignmentGrantedMode;

    #[test]
    fn merging_bsc_loop_state_keeps_the_bts_reverse_measurements() {
        let bts = proto::TrafficChannelPower {
            target_eb_nt_db: 8.0,
            effective_target_eb_nt_db: 8.0,
            manual_target_override_db: None,
            last_pcg_snr_db: Vec::new(),
            last_active_pcg_mask: Vec::new(),
            last_pcbs: vec![1; 16],
            reverse_pilot_ec_io_db: None,
            fer_pct: 0.0,
            frames_total: 100,
            frames_crc_error: 0,
            forward_gain_offset_db: 0.0,
            forward_last_fer_pct: 0.0,
            forward_last_pmrm_errors: 0,
            forward_last_pmrm_frames: 0,
            forward_pmrm_count: 0,
            forward_pilot_ec_io_db: Vec::new(),
            last_pcg_pilot_ec_nt_db: vec![7.5; 16],
            forward_radio_config: 0,
            reverse_radio_config: 0,
            power_history: Vec::new(),
            measured_inner_loop_1s_db: Some(0.25),
            measured_inner_loop_1s_timestamp_ms: Some(1_234),
        };
        let bsc = TrafficChannelPowerSnapshot {
            target_eb_nt_db: 8.0,
            effective_target_eb_nt_db: 8.0,
            manual_target_override_db: None,
            last_pcg_snr_db: None,
            last_active_pcg_mask: None,
            last_pcbs: [1; 16],
            reverse_pilot_ec_io_db: None,
            fer_pct: 0.0,
            frames_total: 100,
            frames_crc_error: 0,
            forward_gain_offset_db: 0.0,
            forward_last_fer_pct: None,
            forward_last_pmrm_errors: 0,
            forward_last_pmrm_frames: 0,
            forward_pmrm_count: 0,
            forward_pilot_ec_io_db: Vec::new(),
            last_pcg_pilot_ec_nt_db: Some([1.0; 16]),
            forward_radio_config: 2,
            reverse_radio_config: 2,
            power_history: vec![crate::power_control::PowerControlHistoryEntry {
                timestamp_ms: 1,
                measured_mean_db: 1.0,
                target_db: 8.0,
                forward_gain_db: 0.0,
                fer_pct: 0.0,
            }],
        };

        let merged = merge_bsc_power_fields(bts, Some(&bsc));

        assert_eq!(merged.last_pcg_pilot_ec_nt_db, vec![7.5; 16]);
        assert!(merged.power_history.is_empty());
        assert_eq!(merged.measured_inner_loop_1s_db, Some(0.25));
        assert_eq!(merged.measured_inner_loop_1s_timestamp_ms, Some(1_234));
        assert_eq!(merged.forward_radio_config, 2);
    }

    #[test]
    fn requested_service_cam_reports_effective_rc2_and_preserves_rc1_fallback() {
        let cam = ChannelAssignmentMessage::new_extended_traffic_assignment(
            11,
            0,
            CHANNEL_DEFAULT_CONFIG_RC1_RC1,
            ChannelAssignmentGrantedMode::RequestedService,
            false,
        );

        assert_eq!(cam.default_config, Some(CHANNEL_DEFAULT_CONFIG_RC1_RC1));
        assert_eq!(cam_effective_radio_config(&cam), Some((2, 2)));
    }

    #[test]
    fn default_configuration_cam_reports_selected_pair() {
        let rc1 = ChannelAssignmentMessage::new_traffic_assignment(10, 0);
        assert_eq!(cam_effective_radio_config(&rc1), Some((1, 1)));

        let rc2 = ChannelAssignmentMessage::new_extended_traffic_assignment(
            11,
            0,
            CHANNEL_DEFAULT_CONFIG_RC2_RC2,
            ChannelAssignmentGrantedMode::DefaultConfiguration,
            false,
        );
        assert_eq!(cam_effective_radio_config(&rc2), Some((2, 2)));
    }
}

/// Capacity of the republished paging-channel transmission channel. Deep
/// enough that a slow management client does not drop pages during a burst.
const PCH_BROADCAST_CAPACITY: usize = 256;

/// Start the BSC management gRPC server on the given address.
pub async fn run_grpc_server(
    state: Arc<BscState>,
    addr: SocketAddr,
    mtls: Option<MtlsConfig>,
) -> Result<(), Box<dyn std::error::Error>> {
    let (pch_transmissions, _) = broadcast::channel(PCH_BROADCAST_CAPACITY);
    spawn_pch_transmission_bridge(state.bts.clone(), pch_transmissions.clone());

    let bsc_service = BscServiceImpl {
        state,
        pch_transmissions,
    };
    let management_facade_service = bsc_service.clone();
    let base_station_service = bsc_service.clone();
    let bts_management_service = bsc_service.clone();
    let bsc_management_service = bsc_service.clone();

    info!("BSC management gRPC server listening on {addr}");
    let mut server = tonic::transport::Server::builder();
    if let Some(mtls) = mtls.as_ref() {
        server = server.tls_config(load_server_tls_config(mtls)?)?;
        info!("management gRPC mTLS enabled");
    }

    server
        .add_service(ManagementFacadeServiceServer::new(
            management_facade_service,
        ))
        .add_service(BaseStationServiceServer::new(base_station_service))
        .add_service(BtsManagementServiceServer::new(bts_management_service))
        .add_service(BscManagementServiceServer::new(bsc_management_service))
        .add_service(BscServiceServer::new(bsc_service))
        .serve(addr)
        .await?;
    Ok(())
}
