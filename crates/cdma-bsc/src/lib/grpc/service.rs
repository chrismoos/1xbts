use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;

use crate::abis_edge::BearerStats;
use cdma_bts::bts::{
    BtsCommand, BtsPowerControlSnapshot, BtsRuntimeSettings, IqCaptureStatus as BtsIqCaptureStatus,
    RxMetrics as BtsRxMetrics, TxMetrics as BtsTxMetrics,
};
use cdma_common::access::AccessMessage;
use cdma_common::consts::{SERVICE_OPTION_HIGH_RATE_PACKET_DATA, SERVICE_OPTION_PACKET_DATA};
use cdma_common::events::AccessChannelEvent;
use cdma_common::lac::{
    message_types::{MessageId, WireChannel},
    paging_messages::{GeneralPageRecord, MsAddress, PagingChannelMessage},
};
use log::info;
use tokio::sync::oneshot;
use tokio_stream::{Stream, StreamExt};
use tonic::transport::{Certificate, Identity, ServerTlsConfig};
use tonic::{Request, Response, Status};

use super::bsc_management_proto::bsc_management_service_server::{
    BscManagementService, BscManagementServiceServer,
};
use super::bts_management_proto::bts_management_service_server::{
    BtsManagementService, BtsManagementServiceServer,
};
use super::bts_management_proto::{ReversePowerControlList, ReversePowerControlRequest};
use super::management_proto::management_facade_service_server::{
    ManagementFacadeService, ManagementFacadeServiceServer,
};
use super::management_proto::{ManagementEvent, NodeHealth, SystemOverview, management_event};
use super::pcf_management_proto::pcf_management_service_server::{
    PcfManagementService, PcfManagementServiceServer,
};
use super::pcf_management_proto::{GetPcfSessionRequest, PcfSessionList};
use super::pdsn_management_proto::pdsn_management_service_server::{
    PdsnManagementService, PdsnManagementServiceServer,
};
use super::pdsn_management_proto::{
    GetPdsnSessionRequest, PdsnSessionList, SetPacketTraceCaptureRequest,
};
use super::proto;
use super::proto::bsc_service_server::{BscService, BscServiceServer};
use super::state::BscState;
use crate::bsc::traffic_events::forward_order_display_name;
use crate::bsc::{DataCallRequest, PagingEvent, TrafficEvent, TrafficPowerOverrideAction};
use crate::config::MtlsConfig;
use crate::power_control::TrafficChannelPowerSnapshot;
use cdma_common::formatting::{
    bitstream_to_hex, bytes_to_hex, format_dtmf_digits, forward_order_name,
    mobile_station_reject_reason, rejected_pdu_type_name,
};
use cdma_hlr::proto::hlr_service_server::HlrServiceServer;
use cdma_hlr::service::HlrServiceImpl;
use cdma_packet::proto::packet_service_client::PacketServiceClient;
use cdma_packet::proto::packet_service_server::{PacketService, PacketServiceServer};
use cdma_smsc::proto::smsc_service_server::SmscServiceServer;
use cdma_smsc::service::SmscServiceImpl;
use uuid::Uuid;

#[derive(Clone)]
pub struct BscServiceImpl {
    state: Arc<BscState>,
}

#[derive(Clone)]
struct PacketServiceProxy {
    client: PacketServiceClient<tonic::transport::Channel>,
}

impl PacketServiceProxy {
    fn new(endpoint: String) -> Result<Self, Status> {
        let channel = tonic::transport::Endpoint::new(endpoint)
            .map_err(|e| Status::unavailable(format!("invalid packet gRPC endpoint: {e}")))?
            .connect_lazy();
        Ok(Self {
            client: PacketServiceClient::new(channel),
        })
    }

    fn client(&self) -> PacketServiceClient<tonic::transport::Channel> {
        self.client.clone()
    }
}

#[tonic::async_trait]
impl PacketService for PacketServiceProxy {
    async fn open_session(
        &self,
        request: Request<cdma_packet::proto::OpenSessionRequest>,
    ) -> Result<Response<cdma_packet::proto::OpenSessionResponse>, Status> {
        self.client().open_session(request).await
    }

    async fn close_session(
        &self,
        request: Request<cdma_packet::proto::CloseSessionRequest>,
    ) -> Result<Response<cdma_packet::proto::CloseSessionResponse>, Status> {
        self.client().close_session(request).await
    }

    type StreamSessionStream =
        Pin<Box<dyn Stream<Item = Result<cdma_packet::proto::SessionFrame, Status>> + Send>>;

    async fn stream_session(
        &self,
        request: Request<tonic::Streaming<cdma_packet::proto::SessionFrame>>,
    ) -> Result<Response<Self::StreamSessionStream>, Status> {
        let mut inbound = request.into_inner();
        let outbound = async_stream::stream! {
            while let Some(frame) = inbound.next().await {
                match frame {
                    Ok(frame) => yield frame,
                    Err(_) => break,
                }
            }
        };
        let response = self.client().stream_session(outbound).await?;
        Ok(Response::new(Box::pin(response.into_inner())))
    }

    async fn get_session_status(
        &self,
        request: Request<cdma_packet::proto::GetSessionStatusRequest>,
    ) -> Result<Response<cdma_packet::proto::GetSessionStatusResponse>, Status> {
        self.client().get_session_status(request).await
    }

    async fn list_sessions(
        &self,
        request: Request<cdma_packet::proto::ListSessionsRequest>,
    ) -> Result<Response<cdma_packet::proto::ListSessionsResponse>, Status> {
        self.client().list_sessions(request).await
    }

    async fn set_session_capture(
        &self,
        request: Request<cdma_packet::proto::SetSessionCaptureRequest>,
    ) -> Result<Response<cdma_packet::proto::SetSessionCaptureResponse>, Status> {
        self.client().set_session_capture(request).await
    }

    async fn set_sch_active(
        &self,
        request: Request<cdma_packet::proto::SetSchActiveRequest>,
    ) -> Result<Response<cdma_packet::proto::SetSchActiveResponse>, Status> {
        self.client().set_sch_active(request).await
    }
}

// ─── Conversions ────────────────────────────────────────────────

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
    }
}

fn to_proto_bts_reverse_power(
    snapshot: &BtsPowerControlSnapshot,
    bsc_snapshot: Option<&TrafficChannelPowerSnapshot>,
) -> proto::TrafficChannelPower {
    proto::TrafficChannelPower {
        target_eb_nt_db: snapshot.target_eb_nt_db,
        effective_target_eb_nt_db: snapshot.effective_target_eb_nt_db,
        manual_target_override_db: snapshot.manual_target_override_db,
        last_pcg_snr_db: bsc_snapshot
            .and_then(|s| s.last_pcg_snr_db)
            .map(|arr| arr.to_vec())
            .unwrap_or_default(),
        last_active_pcg_mask: bsc_snapshot
            .and_then(|s| s.last_active_pcg_mask)
            .map(|arr| arr.to_vec())
            .unwrap_or_default(),
        last_pcbs: snapshot.last_pcbs.iter().map(|b| *b as u32).collect(),
        reverse_pilot_ec_io_db: bsc_snapshot.and_then(|s| s.reverse_pilot_ec_io_db),
        fer_pct: snapshot.fer_pct,
        frames_total: snapshot.frames_total,
        frames_crc_error: snapshot.frames_crc_error,
        forward_gain_offset_db: bsc_snapshot.map_or(0.0, |s| s.forward_gain_offset_db),
        forward_last_fer_pct: bsc_snapshot
            .and_then(|s| s.forward_last_fer_pct)
            .unwrap_or(0.0),
        forward_last_pmrm_errors: bsc_snapshot.map_or(0, |s| s.forward_last_pmrm_errors),
        forward_last_pmrm_frames: bsc_snapshot.map_or(0, |s| s.forward_last_pmrm_frames),
        forward_pmrm_count: bsc_snapshot.map_or(0, |s| s.forward_pmrm_count),
        forward_pilot_ec_io_db: bsc_snapshot
            .map(|s| s.forward_pilot_ec_io_db.clone())
            .unwrap_or_default(),
        last_pcg_pilot_ec_nt_db: snapshot.last_pcg_pilot_ec_nt_db.to_vec(),
        reverse_radio_config: bsc_snapshot.map_or(0, |s| s.reverse_radio_config),
        power_history: bsc_snapshot
            .map(|s| {
                s.power_history
                    .iter()
                    .map(|e| proto::PowerControlSample {
                        timestamp_ms: e.timestamp_ms,
                        measured_eb_nt_db: e.measured_mean_db,
                        target_eb_nt_db: e.target_db,
                        forward_gain_db: e.forward_gain_db,
                        fer_pct: e.fer_pct,
                    })
                    .collect()
            })
            .unwrap_or_default(),
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
        synth_pilot_us: m.synth_pilot_us,
        synth_sync_us: m.synth_sync_us,
        synth_paging_us: m.synth_paging_us,
        synth_spread_us: m.synth_spread_us,
        sync_fragments_sent: m.sync_fragments_sent,
        paging_fragments_sent: m.paging_fragments_sent,
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
    }
}

fn to_proto_bearer_metrics(m: BearerStats) -> proto::BearerMetrics {
    proto::BearerMetrics {
        tx_frames: m.tx_frames,
        rx_accepted: m.rx_accepted,
        duplicate_drop: m.duplicate_drop,
        late_drop: m.late_drop,
        encode_errors: m.encode_errors,
        route_errors: m.route_errors,
        delivery_errors: m.delivery_errors,
    }
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
            let assign_mode_name = match m.assign_mode {
                0b000 => "IS-95 Traffic",
                0b100 => "Extended Traffic (IS-2000)",
                _ => "Unknown",
            };
            let default_config_name = match m.default_config {
                Some(0b000) => "RC1/RC1",
                Some(0b001) => "RC2/RC2",
                Some(0b010) => "RC3/RC3",
                Some(0b011) => "RC4/RC3",
                Some(0b100) => "Explicit FOR_RC/REV_RC",
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
                    for_rc: None,
                    rev_rc: None,
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
                default_config_name: if m.default_config == 0b100 {
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
    }
}

fn to_proto_config(
    cfg: &BtsRuntimeSettings,
    channel: cdma_common::band_class::ChannelPlan,
    tx_center_frequency_hz: usize,
    rx_center_frequency_hz: usize,
    overhead: &crate::bsc::OverheadParameters,
    timezone: &cdma_common::timezone::TimezoneConfig,
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
        cdma_channel: channel.cdma_channel as u32,
        band_subclass: channel.band_subclass as u32,
        tx_digital_backoff: cfg.tx_digital_backoff,
        block_size_chips: cfg.block_size_chips as u32,
        pilot: Some(proto::PilotConfig {
            walsh_code: cfg.downlink.pilot.walsh_code as u32,
            gain: cfg.downlink.pilot.gain,
        }),
        sync: Some(proto::SyncConfig {
            walsh_code: cfg.downlink.sync.walsh_code as u32,
            data_rate_bps: cfg.downlink.sync.data_rate_bps as u32,
            gain: cfg.downlink.sync.gain,
        }),
        paging: Some(proto::PagingConfig {
            walsh_code: cfg.downlink.paging.walsh_code as u32,
            paging_channel_number: cfg.downlink.paging.paging_channel_number as u32,
            data_rate_bps: cfg.downlink.paging.data_rate_bps as u32,
            gain: cfg.downlink.paging.gain,
        }),
        overhead: Some(proto::OverheadConfig {
            sid: overhead.sid as u32,
            nid: overhead.nid as u32,
            base_id: overhead.base_id as u32,
            reg_zone: overhead.reg_zone as u32,
            total_zones: overhead.total_zones as u32,
            zone_timer: overhead.zone_timer as u32,
            max_slot_cycle_index: overhead.max_slot_cycle_index as u32,
            page_chan: overhead.page_chan as u32,
            config_seq: overhead.config_seq as u32,
            acc_config_seq: overhead.acc_config_seq as u32,
            power_up_reg: overhead.power_up_reg,
            parameter_reg: overhead.parameter_reg,
            auth_mode: overhead.auth_mode as u32,
            lp_sec: overhead.lp_sec as u32,
            ltm_off: overhead.ltm_off as i32,
            daylt: overhead.daylt as u32,
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
            daylt: resolved.daylt as u32,
            lp_sec: resolved.lp_sec as u32,
            utc_offset_seconds: (resolved.ltm_off as i32) * 1800,
        }),
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
        first_absolute_sample_start: status.first_absolute_sample_start,
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

// ─── Service Implementation ─────────────────────────────────────

type GrpcStream<T> = Pin<Box<dyn Stream<Item = Result<T, Status>> + Send>>;

#[tonic::async_trait]
impl BscService for BscServiceImpl {
    async fn get_system_status(
        &self,
        _: Request<()>,
    ) -> Result<Response<proto::SystemStatus>, Status> {
        let oh = &self.state.overhead;
        Ok(Response::new(proto::SystemStatus {
            running: true,
            sid: oh.sid as u32,
            nid: oh.nid as u32,
            base_id: oh.base_id as u32,
            pilot_pn: self.state.pilot_offset as u32,
            reg_zone: oh.reg_zone as u32,
        }))
    }

    async fn get_config(&self, _: Request<()>) -> Result<Response<proto::BtsConfig>, Status> {
        let cfg = to_proto_config(
            &self.state.bts_config,
            self.state.channel,
            self.state.tx_center_frequency_hz,
            self.state.rx_center_frequency_hz,
            &self.state.overhead,
            &self.state.timezone,
            self.state.pilot_offset,
        );
        Ok(Response::new(cfg))
    }

    async fn get_radio_metrics(
        &self,
        _: Request<()>,
    ) -> Result<Response<proto::RadioMetrics>, Status> {
        let tx = self.state.tx_metrics.borrow().clone();
        let rx = self.state.rx_metrics.borrow().clone();
        Ok(Response::new(proto::RadioMetrics {
            tx: Some(to_proto_tx_metrics(&tx)),
            rx: Some(to_proto_rx_metrics(&rx)),
            bearer: self
                .state
                .bts_client
                .bearer_client()
                .map(|client| to_proto_bearer_metrics(client.stats())),
        }))
    }

    async fn get_iq_capture_status(
        &self,
        _: Request<()>,
    ) -> Result<Response<proto::IqCaptureStatus>, Status> {
        let (respond_to, rx) = oneshot::channel();
        self.state
            .bts_commands
            .send(BtsCommand::GetCaptureStatus {
                directory: self.state.iq_capture_dir.clone(),
                respond_to,
            })
            .await
            .map_err(|e| Status::unavailable(format!("BTS command queue unavailable: {}", e)))?;

        let result = rx
            .await
            .map_err(|_| Status::unavailable("BTS RX thread dropped capture response"))?;
        let result = result.map_err(Status::failed_precondition)?;
        Ok(Response::new(to_proto_iq_capture_status(&result.status)))
    }

    async fn start_iq_capture(
        &self,
        _: Request<()>,
    ) -> Result<Response<proto::IqCaptureStatus>, Status> {
        let (respond_to, rx) = oneshot::channel();
        self.state
            .bts_commands
            .send(BtsCommand::StartCapture {
                directory: self.state.iq_capture_dir.clone(),
                respond_to,
            })
            .await
            .map_err(|e| Status::unavailable(format!("BTS command queue unavailable: {}", e)))?;

        let result = rx
            .await
            .map_err(|_| Status::unavailable("BTS RX thread dropped capture response"))?;
        let result = result.map_err(Status::failed_precondition)?;
        Ok(Response::new(to_proto_iq_capture_status(&result.status)))
    }

    async fn stop_iq_capture(
        &self,
        _: Request<()>,
    ) -> Result<Response<proto::IqCaptureStatus>, Status> {
        let (respond_to, rx) = oneshot::channel();
        self.state
            .bts_commands
            .send(BtsCommand::StopCapture { respond_to })
            .await
            .map_err(|e| Status::unavailable(format!("BTS command queue unavailable: {}", e)))?;

        let result = rx
            .await
            .map_err(|_| Status::unavailable("BTS RX thread dropped capture response"))?;
        let result = result.map_err(Status::failed_precondition)?;
        Ok(Response::new(to_proto_iq_capture_status(&result.status)))
    }

    type StreamRadioMetricsStream = GrpcStream<proto::RadioMetrics>;

    async fn stream_radio_metrics(
        &self,
        _: Request<()>,
    ) -> Result<Response<Self::StreamRadioMetricsStream>, Status> {
        let mut tx_rx = self.state.tx_metrics.clone();
        let rx_rx = self.state.rx_metrics.clone();
        let bts_client = self.state.bts_client.clone();
        let stream = async_stream::stream! {
            loop {
                if tx_rx.changed().await.is_err() {
                    break;
                }
                let tx = tx_rx.borrow().clone();
                let rx = rx_rx.borrow().clone();
                yield Ok(proto::RadioMetrics {
                    tx: Some(to_proto_tx_metrics(&tx)),
                    rx: Some(to_proto_rx_metrics(&rx)),
                    bearer: bts_client
                        .bearer_client()
                        .map(|client| to_proto_bearer_metrics(client.stats())),
                });
            }
        };
        Ok(Response::new(Box::pin(stream)))
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

    async fn stream_paging_events(
        &self,
        _: Request<()>,
    ) -> Result<Response<Self::StreamPagingEventsStream>, Status> {
        let mut rx = self.state.paging_broadcast.subscribe();
        let stream = async_stream::stream! {
            while let Ok(event) = rx.recv().await {
                yield Ok(to_proto_paging_event(&event));
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
        Ok(Response::new(proto::MobileList {
            mobiles: mobiles
                .into_iter()
                .map(|m| {
                    let traffic_power = m
                        .traffic_walsh_code
                        .and_then(|walsh| self.state.bts_power_control.snapshot(walsh))
                        .map(|snapshot| {
                            to_proto_bts_reverse_power(&snapshot, m.traffic_power.as_ref())
                        })
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
                    }
                })
                .collect(),
        }))
    }

    async fn list_channels(&self, _: Request<()>) -> Result<Response<proto::ChannelList>, Status> {
        let cfg = &self.state.bts_config;
        let mut channels = Vec::new();

        // Forward-link overhead channels
        channels.push(proto::Channel {
            walsh_code: Some(cfg.downlink.pilot.walsh_code as u32),
            channel_type: "pilot".into(),
            direction: "forward".into(),
            gain: Some(cfg.downlink.pilot.gain),
            data_rate_bps: None,
            paging_channel_number: None,
            access_channel_number: None,
            mobile: None,
            service_option: None,
            traffic_power: None,
        });
        channels.push(proto::Channel {
            walsh_code: Some(cfg.downlink.sync.walsh_code as u32),
            channel_type: "sync".into(),
            direction: "forward".into(),
            gain: Some(cfg.downlink.sync.gain),
            data_rate_bps: Some(cfg.downlink.sync.data_rate_bps as u32),
            paging_channel_number: None,
            access_channel_number: None,
            mobile: None,
            service_option: None,
            traffic_power: None,
        });
        channels.push(proto::Channel {
            walsh_code: Some(cfg.downlink.paging.walsh_code as u32),
            channel_type: "paging".into(),
            direction: "forward".into(),
            gain: Some(cfg.downlink.paging.gain),
            data_rate_bps: Some(cfg.downlink.paging.data_rate_bps as u32),
            paging_channel_number: Some(cfg.downlink.paging.paging_channel_number as u32),
            access_channel_number: None,
            mobile: None,
            service_option: None,
            traffic_power: None,
        });

        // Reverse-link access channels
        for &acc_num in &cfg.uplink.access_channel_numbers {
            channels.push(proto::Channel {
                walsh_code: None,
                channel_type: "access".into(),
                direction: "reverse".into(),
                gain: None,
                data_rate_bps: Some(cfg.uplink.access_channel_rate_bps as u32),
                paging_channel_number: None,
                access_channel_number: Some(acc_num as u32),
                mobile: None,
                service_option: None,
                traffic_power: None,
            });
        }

        // Traffic channels from active mobiles
        let mobiles = self.state.mobiles.borrow().clone();
        for m in &mobiles {
            if let Some(walsh) = m.traffic_walsh_code {
                let traffic_power = self
                    .state
                    .bts_power_control
                    .snapshot(walsh)
                    .map(|snapshot| to_proto_bts_reverse_power(&snapshot, m.traffic_power.as_ref()))
                    .or_else(|| m.traffic_power.as_ref().map(to_proto_traffic_channel_power));
                channels.push(proto::Channel {
                    walsh_code: Some(walsh as u32),
                    channel_type: "traffic".into(),
                    direction: "forward".into(),
                    gain: None,
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
                });
            }
        }

        channels.sort_by_key(|c| (c.direction.clone(), c.walsh_code.unwrap_or(u32::MAX)));

        Ok(Response::new(proto::ChannelList {
            channels,
            total_walsh_codes: cfg.orthogonal_code_length as u32,
        }))
    }

    async fn set_traffic_channel_power_override(
        &self,
        request: Request<proto::SetTrafficChannelPowerOverrideRequest>,
    ) -> Result<Response<proto::SetTrafficChannelPowerOverrideResponse>, Status> {
        let req = request.into_inner();
        let walsh_code = u8::try_from(req.walsh_code)
            .map_err(|_| Status::invalid_argument("walsh_code must fit in u8"))?;
        let action = match req.action {
            Some(proto::set_traffic_channel_power_override_request::Action::SetTargetEbNtDb(
                target_db,
            )) => TrafficPowerOverrideAction::SetTargetEbNtDb(target_db),
            Some(proto::set_traffic_channel_power_override_request::Action::Clear(_)) => {
                TrafficPowerOverrideAction::Clear
            }
            None => {
                return Err(Status::invalid_argument(
                    "one of set_target_eb_nt_db or clear is required",
                ));
            }
        };
        if !self
            .state
            .mobiles
            .borrow()
            .iter()
            .any(|mobile| mobile.traffic_walsh_code == Some(walsh_code))
        {
            return Err(Status::not_found("active traffic channel not found"));
        }

        match action {
            TrafficPowerOverrideAction::SetTargetEbNtDb(target_db) => {
                self.state
                    .bts_power_control
                    .set_target(walsh_code, target_db, true);
            }
            TrafficPowerOverrideAction::Clear => {
                let target_db = self
                    .state
                    .bts_power_control
                    .snapshot(walsh_code)
                    .map(|snapshot| snapshot.target_eb_nt_db)
                    .ok_or_else(|| Status::not_found("active BTS power-control state not found"))?;
                self.state
                    .bts_power_control
                    .set_target(walsh_code, target_db, false);
            }
        }

        let snapshot = self
            .state
            .bts_power_control
            .snapshot(walsh_code)
            .ok_or_else(|| Status::not_found("active BTS power-control state not found"))?;
        let bsc_snapshot = self
            .state
            .mobiles
            .borrow()
            .iter()
            .find(|mobile| mobile.traffic_walsh_code == Some(walsh_code))
            .and_then(|mobile| mobile.traffic_power.clone());

        let message = if let Some(manual_db) = snapshot.manual_target_override_db {
            format!(
                "manual reverse target pinned at {:.2} dB on walsh {}",
                manual_db, walsh_code
            )
        } else {
            format!(
                "manual reverse target cleared on walsh {}; auto resumed at {:.2} dB",
                walsh_code, snapshot.target_eb_nt_db
            )
        };

        Ok(Response::new(
            proto::SetTrafficChannelPowerOverrideResponse {
                accepted: true,
                message,
                traffic_power: Some(to_proto_bts_reverse_power(&snapshot, bsc_snapshot.as_ref())),
            },
        ))
    }

    async fn initiate_data_call(
        &self,
        request: Request<proto::InitiateDataCallRequest>,
    ) -> Result<Response<proto::InitiateDataCallResponse>, Status> {
        let req = request.into_inner();
        let subscriber_id = Uuid::parse_str(&req.subscriber_id)
            .map_err(|_| Status::invalid_argument("subscriber_id must be a valid UUID"))?;
        let service_option = if req.service_option == u32::from(SERVICE_OPTION_PACKET_DATA) {
            SERVICE_OPTION_PACKET_DATA
        } else {
            SERVICE_OPTION_HIGH_RATE_PACKET_DATA
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
        let status = <Self as BscService>::get_system_status(self, request)
            .await?
            .into_inner();
        Ok(Response::new(SystemOverview {
            bsc_status: Some(status),
            nodes: vec![
                NodeHealth {
                    node_id: self.state.node_id.clone(),
                    node_type: "BTS".into(),
                    healthy: true,
                    message: "hosted in current BSC process".into(),
                },
                NodeHealth {
                    node_id: self.state.node_id.clone(),
                    node_type: "BSC".into(),
                    healthy: true,
                    message: "hosted in current BSC process".into(),
                },
            ],
        }))
    }

    type StreamSystemEventsStream = GrpcStream<ManagementEvent>;

    async fn stream_system_events(
        &self,
        _: Request<()>,
    ) -> Result<Response<Self::StreamSystemEventsStream>, Status> {
        let mut tx_rx = self.state.tx_metrics.clone();
        let rx_rx = self.state.rx_metrics.clone();
        let mut access_rx = self.state.access_broadcast.subscribe();
        let mut paging_rx = self.state.paging_broadcast.subscribe();
        let mut traffic_rx = self.state.traffic_broadcast.subscribe();
        let bts_client = self.state.bts_client.clone();
        let node_id = self.state.node_id.clone();

        let stream = async_stream::stream! {
            loop {
                tokio::select! {
                    changed = tx_rx.changed() => {
                        if changed.is_err() {
                            break;
                        }
                        let tx = tx_rx.borrow().clone();
                        let rx = rx_rx.borrow().clone();
                        yield Ok(ManagementEvent {
                            source_node_id: node_id.clone(),
                            source_node_type: "BTS".into(),
                            classification: "telemetry".into(),
                            body: Some(management_event::Body::RadioMetrics(proto::RadioMetrics {
                                tx: Some(to_proto_tx_metrics(&tx)),
                                rx: Some(to_proto_rx_metrics(&rx)),
                                bearer: bts_client
                                    .bearer_client()
                                    .map(|client| to_proto_bearer_metrics(client.stats())),
                            })),
                        });
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
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
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
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
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
                            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => {}
                            Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                        }
                    }
                }
            }
        };

        Ok(Response::new(Box::pin(stream)))
    }
}

#[tonic::async_trait]
impl BtsManagementService for BscServiceImpl {
    async fn get_bts_status(
        &self,
        request: Request<()>,
    ) -> Result<Response<proto::SystemStatus>, Status> {
        <Self as BscService>::get_system_status(self, request).await
    }

    async fn get_bts_config(
        &self,
        request: Request<()>,
    ) -> Result<Response<proto::BtsConfig>, Status> {
        <Self as BscService>::get_config(self, request).await
    }

    async fn get_radio_metrics(
        &self,
        request: Request<()>,
    ) -> Result<Response<proto::RadioMetrics>, Status> {
        <Self as BscService>::get_radio_metrics(self, request).await
    }

    type StreamRadioMetricsStream = GrpcStream<proto::RadioMetrics>;

    async fn stream_radio_metrics(
        &self,
        request: Request<()>,
    ) -> Result<Response<Self::StreamRadioMetricsStream>, Status> {
        <Self as BscService>::stream_radio_metrics(self, request).await
    }

    async fn get_iq_capture_status(
        &self,
        request: Request<()>,
    ) -> Result<Response<proto::IqCaptureStatus>, Status> {
        <Self as BscService>::get_iq_capture_status(self, request).await
    }

    async fn start_iq_capture(
        &self,
        request: Request<()>,
    ) -> Result<Response<proto::IqCaptureStatus>, Status> {
        <Self as BscService>::start_iq_capture(self, request).await
    }

    async fn stop_iq_capture(
        &self,
        request: Request<()>,
    ) -> Result<Response<proto::IqCaptureStatus>, Status> {
        <Self as BscService>::stop_iq_capture(self, request).await
    }

    async fn list_local_radio_resources(
        &self,
        request: Request<()>,
    ) -> Result<Response<proto::ChannelList>, Status> {
        <Self as BscService>::list_channels(self, request).await
    }

    async fn get_reverse_power_control(
        &self,
        request: Request<ReversePowerControlRequest>,
    ) -> Result<Response<proto::TrafficChannelPower>, Status> {
        let walsh_code = u8::try_from(request.into_inner().walsh_code)
            .map_err(|_| Status::invalid_argument("walsh_code must fit in u8"))?;
        let snapshot = self
            .state
            .bts_power_control
            .snapshot(walsh_code)
            .ok_or_else(|| Status::not_found("active BTS power-control state not found"))?;
        let bsc_snapshot = self
            .state
            .mobiles
            .borrow()
            .iter()
            .find(|mobile| mobile.traffic_walsh_code == Some(walsh_code))
            .and_then(|mobile| mobile.traffic_power.clone());
        Ok(Response::new(to_proto_bts_reverse_power(
            &snapshot,
            bsc_snapshot.as_ref(),
        )))
    }

    async fn list_reverse_power_controls(
        &self,
        _: Request<()>,
    ) -> Result<Response<ReversePowerControlList>, Status> {
        let mobiles = self.state.mobiles.borrow().clone();
        let power_controls = self
            .state
            .bts_power_control
            .snapshots()
            .iter()
            .map(|snapshot| {
                let bsc_snapshot = mobiles
                    .iter()
                    .find(|mobile| mobile.traffic_walsh_code == Some(snapshot.walsh_code))
                    .and_then(|mobile| mobile.traffic_power.as_ref());
                to_proto_bts_reverse_power(snapshot, bsc_snapshot)
            })
            .collect();
        Ok(Response::new(ReversePowerControlList { power_controls }))
    }

    async fn set_reverse_power_control_override(
        &self,
        request: Request<proto::SetTrafficChannelPowerOverrideRequest>,
    ) -> Result<Response<proto::SetTrafficChannelPowerOverrideResponse>, Status> {
        <Self as BscService>::set_traffic_channel_power_override(self, request).await
    }

    type StreamPchTransmissionsStream = GrpcStream<proto::PagingEvent>;

    async fn stream_pch_transmissions(
        &self,
        _: Request<()>,
    ) -> Result<Response<Self::StreamPchTransmissionsStream>, Status> {
        let pch_tx = self
            .state
            .pch_transmit_broadcast
            .as_ref()
            .ok_or_else(|| Status::unavailable("PCH transmit broadcast not configured"))?;
        let mut rx = pch_tx.subscribe();
        let stream = async_stream::stream! {
            while let Ok(evt) = rx.recv().await {
                match crate::bsc::pch_transmit_event_to_paging_event(&evt) {
                    Ok(paging_event) => yield Ok(to_proto_paging_event(&paging_event)),
                    Err(e) => yield Err(Status::internal(format!(
                        "PCH transmit reconstruction failed: {e}"
                    ))),
                }
            }
        };
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

async fn packet_list_sessions(
    endpoint: &str,
) -> Result<Vec<cdma_packet::proto::PacketSessionInfo>, Status> {
    let mut client = PacketServiceClient::connect(endpoint.to_string())
        .await
        .map_err(|e| Status::unavailable(format!("packet gRPC connect failed: {e}")))?;
    Ok(client
        .list_sessions(cdma_packet::proto::ListSessionsRequest {})
        .await?
        .into_inner()
        .sessions)
}

async fn packet_get_session_detail(
    endpoint: &str,
    session_id: String,
) -> Result<cdma_packet::proto::PacketSessionDetail, Status> {
    let mut client = PacketServiceClient::connect(endpoint.to_string())
        .await
        .map_err(|e| Status::unavailable(format!("packet gRPC connect failed: {e}")))?;
    client
        .get_session_status(cdma_packet::proto::GetSessionStatusRequest { session_id })
        .await?
        .into_inner()
        .session
        .ok_or_else(|| Status::not_found("packet session not found"))
}

async fn packet_set_capture(
    endpoint: &str,
    session_id: String,
    enabled: bool,
) -> Result<cdma_packet::proto::PacketSessionDetail, Status> {
    let mut client = PacketServiceClient::connect(endpoint.to_string())
        .await
        .map_err(|e| Status::unavailable(format!("packet gRPC connect failed: {e}")))?;
    client
        .set_session_capture(cdma_packet::proto::SetSessionCaptureRequest {
            session_id,
            enabled,
        })
        .await?
        .into_inner()
        .session
        .ok_or_else(|| Status::not_found("packet session not found"))
}

#[tonic::async_trait]
impl PcfManagementService for BscServiceImpl {
    async fn initiate_data_call(
        &self,
        request: Request<proto::InitiateDataCallRequest>,
    ) -> Result<Response<proto::InitiateDataCallResponse>, Status> {
        <Self as BscService>::initiate_data_call(self, request).await
    }

    async fn list_pcf_sessions(&self, _: Request<()>) -> Result<Response<PcfSessionList>, Status> {
        Ok(Response::new(PcfSessionList {
            sessions: packet_list_sessions(&self.state.packet_endpoint)
                .await?
                .into_iter()
                .map(to_management_packet_session_info)
                .collect(),
        }))
    }

    async fn get_pcf_session(
        &self,
        request: Request<GetPcfSessionRequest>,
    ) -> Result<Response<super::packet_proto::GetSessionStatusResponse>, Status> {
        let session_id = request.into_inner().session_id;
        let session = packet_get_session_detail(&self.state.packet_endpoint, session_id)
            .await
            .map(to_management_packet_session_detail)?;
        Ok(Response::new(
            super::packet_proto::GetSessionStatusResponse {
                session: Some(session),
            },
        ))
    }
}

#[tonic::async_trait]
impl PdsnManagementService for BscServiceImpl {
    async fn list_pdsn_sessions(
        &self,
        _: Request<()>,
    ) -> Result<Response<PdsnSessionList>, Status> {
        Ok(Response::new(PdsnSessionList {
            sessions: packet_list_sessions(&self.state.packet_endpoint)
                .await?
                .into_iter()
                .map(to_management_packet_session_info)
                .collect(),
        }))
    }

    async fn get_pdsn_session(
        &self,
        request: Request<GetPdsnSessionRequest>,
    ) -> Result<Response<super::packet_proto::GetSessionStatusResponse>, Status> {
        let session_id = request.into_inner().session_id;
        let session = packet_get_session_detail(&self.state.packet_endpoint, session_id)
            .await
            .map(to_management_packet_session_detail)?;
        Ok(Response::new(
            super::packet_proto::GetSessionStatusResponse {
                session: Some(session),
            },
        ))
    }

    async fn set_packet_trace_capture(
        &self,
        request: Request<SetPacketTraceCaptureRequest>,
    ) -> Result<Response<super::packet_proto::SetSessionCaptureResponse>, Status> {
        let req = request.into_inner();
        let session = packet_set_capture(&self.state.packet_endpoint, req.session_id, req.enabled)
            .await
            .map(to_management_packet_session_detail)?;
        Ok(Response::new(
            super::packet_proto::SetSessionCaptureResponse {
                session: Some(session),
            },
        ))
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

fn to_management_packet_session_info(
    session: cdma_packet::proto::PacketSessionInfo,
) -> super::packet_proto::PacketSessionInfo {
    super::packet_proto::PacketSessionInfo {
        session_id: session.session_id,
        phase: session.phase,
        service_option: session.service_option,
        peer_ip: session.peer_ip,
        our_ip: session.our_ip,
        tun_device: session.tun_device,
        uplink_frames: session.uplink_frames,
        downlink_frames: session.downlink_frames,
        uplink_bytes: session.uplink_bytes,
        downlink_bytes: session.downlink_bytes,
        created_at_ms: session.created_at_ms,
        last_phase_change_at_ms: session.last_phase_change_at_ms,
        last_uplink_at_ms: session.last_uplink_at_ms,
        last_downlink_at_ms: session.last_downlink_at_ms,
        last_activity_at_ms: session.last_activity_at_ms,
        last_uplink_rate_bps: session.last_uplink_rate_bps,
        last_downlink_rate_bps: session.last_downlink_rate_bps,
        mobile_address: session.mobile_address,
        subscriber_id: session.subscriber_id,
        phone_number: session.phone_number,
        traffic_walsh_code: session.traffic_walsh_code,
        rlp_state: session.rlp_state,
        lcp_state: session.lcp_state,
        ipcp_state: session.ipcp_state,
        capture_enabled: session.capture_enabled,
    }
}

fn to_management_packet_trace_event(
    event: cdma_packet::proto::PacketTraceEvent,
) -> super::packet_proto::PacketTraceEvent {
    super::packet_proto::PacketTraceEvent {
        timestamp_ms: event.timestamp_ms,
        layer: event.layer,
        direction: event.direction,
        summary: event.summary,
        detail: event.detail,
        payload_hex: event.payload_hex,
    }
}

fn to_management_packet_session_detail(
    detail: cdma_packet::proto::PacketSessionDetail,
) -> super::packet_proto::PacketSessionDetail {
    super::packet_proto::PacketSessionDetail {
        summary: detail.summary.map(to_management_packet_session_info),
        last_rx_control: detail.last_rx_control,
        last_tx_control: detail.last_tx_control,
        last_rx_control_repeats: detail.last_rx_control_repeats,
        last_tx_control_repeats: detail.last_tx_control_repeats,
        recent_ppp_events: detail
            .recent_ppp_events
            .into_iter()
            .map(to_management_packet_trace_event)
            .collect(),
        capture_events: detail
            .capture_events
            .into_iter()
            .map(to_management_packet_trace_event)
            .collect(),
    }
}

/// Start the gRPC server on the given address, serving BSC, HLR, and SMSC services.
pub async fn run_grpc_server(
    state: Arc<BscState>,
    packet_endpoint: String,
    addr: SocketAddr,
    mtls: Option<MtlsConfig>,
) -> Result<(), Box<dyn std::error::Error>> {
    if let Some(ref pch_tx) = state.pch_transmit_broadcast {
        let mut pch_rx = pch_tx.subscribe();
        let paging_tx = state.paging_broadcast.clone();
        tokio::spawn(async move {
            loop {
                match pch_rx.recv().await {
                    Ok(evt) => match crate::bsc::pch_transmit_event_to_paging_event(&evt) {
                        Ok(event) => {
                            let _ = paging_tx.send(event);
                        }
                        Err(e) => {
                            log::warn!("dropping undecodable PCH transmit event: {e}");
                        }
                    },
                    Err(tokio::sync::broadcast::error::RecvError::Lagged(n)) => {
                        log::warn!("PCH->paging bridge lagged, skipped {n} events");
                    }
                    Err(tokio::sync::broadcast::error::RecvError::Closed) => break,
                }
            }
        });
    }

    let hlr_service = HlrServiceImpl::new(state.hlr_repo.clone());
    let smsc_service = SmscServiceImpl::new(state.smsc_repo.clone());
    let packet_service = PacketServiceProxy::new(packet_endpoint)?;
    let bsc_service = BscServiceImpl { state };
    let management_facade_service = bsc_service.clone();
    let bts_management_service = bsc_service.clone();
    let bsc_management_service = bsc_service.clone();
    let pcf_management_service = bsc_service.clone();
    let pdsn_management_service = bsc_service.clone();

    info!(
        "gRPC server listening on {} (management + BSC + HLR + SMSC + Packet)",
        addr
    );
    let mut server = tonic::transport::Server::builder();
    if let Some(mtls) = mtls.as_ref() {
        server = server.tls_config(load_server_tls_config(mtls)?)?;
        info!("management gRPC mTLS enabled");
    }

    server
        .add_service(ManagementFacadeServiceServer::new(
            management_facade_service,
        ))
        .add_service(BtsManagementServiceServer::new(bts_management_service))
        .add_service(BscManagementServiceServer::new(bsc_management_service))
        .add_service(PcfManagementServiceServer::new(pcf_management_service))
        .add_service(PdsnManagementServiceServer::new(pdsn_management_service))
        .add_service(BscServiceServer::new(bsc_service))
        .add_service(HlrServiceServer::new(hlr_service))
        .add_service(SmscServiceServer::new(smsc_service))
        .add_service(PacketServiceServer::new(packet_service))
        .serve(addr)
        .await?;
    Ok(())
}
