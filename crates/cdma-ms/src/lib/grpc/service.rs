use std::collections::HashMap;
use std::net::SocketAddr;
use std::path::Path;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use tokio::sync::broadcast::error::RecvError;
use tokio_stream::Stream;
use tonic::{Request, Response, Status};

use super::proto::ms_service_server::{MsService, MsServiceServer};
use super::proto::{
    ChannelList, ChannelResult as ProtoChannelResult, Diagnostics as ProtoDiagnostics,
    DialDataRequest, DialDataResponse, DumpForwardRequest, MobileEntry, MobileList,
    MsEvent as ProtoMsEvent, MsMetrics, MsStatus, OriginateRequest, OriginateResponse,
    Overhead as ProtoOverhead, PageWithSmsRequest, PageWithSmsResponse,
    PagingStats as ProtoPagingStats, PilotStatus, PowerOnRequest, PowerOnResponse,
    PrlVerdictRequest, PrlVerdictResponse, PrlVerdictResult, RadioStats, ScanChannelInfo,
    ScanReport as ProtoScanReport, SendDtmfBurstRequest, SendSmsRequest, SendSmsResponse,
    SetRxGainRequest, StartScanRequest, StartScanResponse, SyncParameters as ProtoSyncParameters,
    TxCalibrationState, TxCalibrationTrim, VoicePcmFrame,
};
use super::proto::{
    MobileState as ProtoMobileState, MsState as ProtoMsState,
    PrlVerdictReason as ProtoPrlVerdictReason, ScanMode as ProtoScanMode,
};
use crate::appliance::{LoopbackControl, MsAppliance};
use crate::config::MsNodeConfig;
use crate::engine::{ChannelResult, Diagnostics, ScanConfig, ScanMode, ScanReport};
use crate::forward_rx::{PilotMeasurement, SyncParameters};
use crate::ms::{MsEvent, OverheadCache, PagingStats};
use crate::prl_scan::{PrlScanPlan, PrlVerdict};
use cdma_common::band_class::BandClass;

pub struct MsServiceImpl {
    appliance: MsAppliance,
    config: Arc<MsNodeConfig>,
    plan: Mutex<Option<Arc<PrlScanPlan>>>,
    loopback: Option<Arc<dyn LoopbackControl>>,
}

impl MsServiceImpl {
    pub fn new(appliance: MsAppliance, config: Arc<MsNodeConfig>) -> Self {
        MsServiceImpl {
            appliance,
            config,
            plan: Mutex::new(None),
            loopback: None,
        }
    }

    pub fn with_plan(self, plan: Arc<PrlScanPlan>) -> Self {
        *self.plan.lock().unwrap() = Some(plan);
        self
    }

    pub fn with_loopback(mut self, loopback: Arc<dyn LoopbackControl>) -> Self {
        self.loopback = Some(loopback);
        self
    }

    pub fn into_server(self) -> MsServiceServer<Self> {
        MsServiceServer::new(self)
    }

    pub fn power_on(&self) {
        self.appliance.power_on();
    }

    fn current_plan(&self) -> Result<Arc<PrlScanPlan>, Status> {
        self.plan
            .lock()
            .unwrap()
            .clone()
            .ok_or_else(|| Status::failed_precondition("no PRL or channel list loaded"))
    }
}

fn parse_band_classes(names: &[String]) -> Result<Vec<BandClass>, Status> {
    names
        .iter()
        .map(|n| {
            serde_json::from_value(serde_json::Value::String(n.to_ascii_lowercase()))
                .map_err(|_| Status::invalid_argument(format!("unknown band class '{n}'")))
        })
        .collect()
}

fn pilot_to_proto(m: &PilotMeasurement) -> PilotStatus {
    PilotStatus {
        locked: m.locked,
        measured: !m.ec_io_db.is_nan(),
        ec_io_db: if m.ec_io_db.is_nan() {
            0.0
        } else {
            m.ec_io_db as f64
        },
        rx_power_dbfs: m.rx_power_dbfs as f64,
        pilot_symbols: m.pilot_symbols,
    }
}

fn sync_to_proto(s: &SyncParameters) -> ProtoSyncParameters {
    ProtoSyncParameters {
        p_rev: s.p_rev as u32,
        min_p_rev: s.min_p_rev as u32,
        sid: s.sid as u32,
        nid: s.nid as u32,
        pilot_pn: s.pilot_pn as u32,
        lc_state: s.lc_state,
        sys_time: s.sys_time,
        system_time: s
            .system_time()
            .to_rfc3339_opts(chrono::SecondsFormat::Millis, true),
        lp_sec: s.lp_sec as u32,
        ltm_off: s.ltm_off as i32,
        daylt: s.daylt,
        prat: s.prat as u32,
        paging_rate_bps: s.paging_rate_bps(),
        cdma_freq: s.cdma_freq as u32,
    }
}

fn json_or_empty<T: serde::Serialize>(v: &Option<T>) -> String {
    v.as_ref()
        .and_then(|m| serde_json::to_string(m).ok())
        .unwrap_or_default()
}

fn overhead_to_proto(o: &OverheadCache) -> ProtoOverhead {
    ProtoOverhead {
        received: o.received().iter().map(|s| s.to_string()).collect(),
        system_parameters: json_or_empty(&o.system_parameters),
        access_parameters: json_or_empty(&o.access_parameters),
        extended_system_parameters: json_or_empty(&o.extended_system_parameters),
        cdma_channel_list: json_or_empty(&o.cdma_channel_list),
        neighbor_list: json_or_empty(&o.neighbor_list),
        extended_neighbor_list: json_or_empty(&o.extended_neighbor_list),
    }
}

fn paging_to_proto(p: &PagingStats) -> ProtoPagingStats {
    ProtoPagingStats {
        crc_valid: p.crc_valid,
        crc_failed: p.crc_failed,
        messages: p
            .messages
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect::<HashMap<_, _>>(),
        pages_for_me: p.pages_for_me,
        undecodable: p
            .undecodable
            .iter()
            .map(|(k, v)| (k.clone(), *v))
            .collect::<HashMap<_, _>>(),
    }
}

fn verdict_to_proto(v: &PrlVerdict) -> PrlVerdictResult {
    let (record, geo, roaming_indicator) = match v {
        PrlVerdict::Permitted {
            record,
            geo,
            roaming_indicator,
        } => (*record, *geo, *roaming_indicator),
        PrlVerdict::Negative { record } => (Some(*record), None, None),
        PrlVerdict::Unlisted => (None, None, None),
    };
    let reason = match v {
        PrlVerdict::Permitted {
            record: Some(_), ..
        } => ProtoPrlVerdictReason::Preferred,
        PrlVerdict::Permitted { record: None, .. } => ProtoPrlVerdictReason::UnlistedAllowed,
        PrlVerdict::Negative { .. } => ProtoPrlVerdictReason::Negative,
        PrlVerdict::Unlisted => ProtoPrlVerdictReason::Unlisted,
    };
    PrlVerdictResult {
        permitted: v.permitted(),
        reason: reason.into(),
        record: record.map(|r| r as u32),
        geo: geo.map(|g| g as u32),
        roaming_indicator: roaming_indicator.map(|r| r as u32),
    }
}

fn channel_result_to_proto(c: &ChannelResult) -> ProtoChannelResult {
    ProtoChannelResult {
        band_class: c.channel.band_class.as_str().to_ascii_lowercase(),
        channel: c.channel.channel as u32,
        frequency_hz: c.channel.frequency_hz,
        acq_index: c.channel.acq_index as u32,
        pilot: match c.pilot {
            None => "searching",
            Some(true) => "found",
            Some(false) => "none",
        }
        .to_string(),
        measurement: Some(pilot_to_proto(&c.measurement)),
        sync: c.sync.as_ref().map(sync_to_proto),
        verdict: c.verdict.as_ref().map(verdict_to_proto),
    }
}

fn scan_to_proto(r: &ScanReport) -> ProtoScanReport {
    ProtoScanReport {
        prl_id: r.prl_id as u32,
        mode: scan_mode_to_proto(r.mode).into(),
        running: r.running,
        result: r.result.clone().unwrap_or_default(),
        total_channels: r.total_channels as u32,
        channels: r.channels.iter().map(channel_result_to_proto).collect(),
    }
}

fn scan_mode_to_proto(mode: ScanMode) -> ProtoScanMode {
    match mode {
        ScanMode::Camp => ProtoScanMode::Camp,
        ScanMode::Survey => ProtoScanMode::Survey,
    }
}

fn ms_state_to_proto(label: &str) -> ProtoMsState {
    match label {
        "off" => ProtoMsState::Off,
        "system_determination" => ProtoMsState::SystemDetermination,
        "pilot_acquisition" => ProtoMsState::PilotAcquisition,
        "sync_acquisition" => ProtoMsState::SyncAcquisition,
        "timing_change" => ProtoMsState::TimingChange,
        "idle" => ProtoMsState::Idle,
        "system_access" => ProtoMsState::SystemAccess,
        "traffic_channel_init" => ProtoMsState::TrafficChannelInit,
        "traffic_channel" => ProtoMsState::TrafficChannel,
        _ => ProtoMsState::Unspecified,
    }
}

fn mobile_state_to_proto(state: &str) -> ProtoMobileState {
    match state {
        "Registered" => ProtoMobileState::Registered,
        "Paged" => ProtoMobileState::Paged,
        "PageResponseReceived" => ProtoMobileState::PageResponseReceived,
        "TrafficAssigning" => ProtoMobileState::TrafficAssigning,
        "TrafficActive" => ProtoMobileState::TrafficActive,
        _ => ProtoMobileState::Unspecified,
    }
}

fn diagnostics_to_proto(d: &Diagnostics, radio: RadioStats) -> ProtoDiagnostics {
    ProtoDiagnostics {
        state: ms_state_to_proto(&d.state).into(),
        registered: d.registered,
        pilot: Some(pilot_to_proto(&d.pilot)),
        sync: d.sync.as_ref().map(sync_to_proto),
        overhead: Some(overhead_to_proto(&d.overhead)),
        paging: Some(paging_to_proto(&d.paging)),
        scan: d.scan.as_ref().map(scan_to_proto),
        radio: Some(radio),
    }
}

fn event_to_proto(ev: &MsEvent) -> ProtoMsEvent {
    ProtoMsEvent {
        timestamp: None,
        event_type: ev.name().to_string(),
        detail: serde_json::to_string(ev).unwrap_or_default(),
    }
}

type EventStream = Pin<Box<dyn Stream<Item = Result<ProtoMsEvent, Status>> + Send>>;
type MetricsStream = Pin<Box<dyn Stream<Item = Result<MsMetrics, Status>> + Send>>;
type VoiceStream = Pin<Box<dyn Stream<Item = Result<VoicePcmFrame, Status>> + Send>>;

#[tonic::async_trait]
impl MsService for MsServiceImpl {
    async fn power_on(
        &self,
        _request: Request<PowerOnRequest>,
    ) -> Result<Response<PowerOnResponse>, Status> {
        self.appliance.power_on();
        Ok(Response::new(PowerOnResponse {
            success: true,
            message: "powered on".to_string(),
        }))
    }

    async fn power_off(&self, _request: Request<()>) -> Result<Response<()>, Status> {
        self.appliance.power_off();
        Ok(Response::new(()))
    }

    async fn get_status(&self, _request: Request<()>) -> Result<Response<MsStatus>, Status> {
        let s = self.appliance.status();
        let pilot = self
            .appliance
            .diagnostics()
            .map(|d| d.pilot)
            .unwrap_or_default();
        Ok(Response::new(MsStatus {
            state: ms_state_to_proto(&s.state).into(),
            pilot_pn: s.pilot_pn as u32,
            sid: s.sid as u32,
            nid: s.nid as u32,
            base_id: s.base_id as u32,
            registered: s.registered,
            paging_channel: 1,
            system_time_chips: s.system_time_chips,
            pilot_ec_io_db: if pilot.ec_io_db.is_nan() {
                0.0
            } else {
                pilot.ec_io_db as f64
            },
            call_state: s.call_state,
            voice_service_option: s.voice_service_option as u32,
            caller_number: s.caller_number,
        }))
    }

    async fn get_diagnostics(
        &self,
        _request: Request<()>,
    ) -> Result<Response<ProtoDiagnostics>, Status> {
        let d = self.appliance.diagnostics().map_err(Status::unavailable)?;
        let stats = self.appliance.stats();
        let radio = RadioStats {
            samples_fed: stats.samples_fed,
            rx_overflows: stats.rx_overflows,
            uptime_secs: stats.uptime_secs,
            real_time_ratio: stats.real_time_ratio,
        };
        Ok(Response::new(diagnostics_to_proto(&d, radio)))
    }

    async fn start_scan(
        &self,
        request: Request<StartScanRequest>,
    ) -> Result<Response<StartScanResponse>, Status> {
        let req = request.into_inner();
        let mode = match ProtoScanMode::try_from(req.mode) {
            Ok(ProtoScanMode::Unspecified | ProtoScanMode::Camp) => ScanMode::Camp,
            Ok(ProtoScanMode::Survey) => ScanMode::Survey,
            Err(_) => {
                return Err(Status::invalid_argument(format!(
                    "scan mode {} is not camp or survey",
                    req.mode
                )));
            }
        };
        let mut plan = if req.channels.is_empty() {
            if req.prl_path.is_empty() {
                return Err(Status::invalid_argument(
                    "give a prl_path or a channel list",
                ));
            }
            PrlScanPlan::load(Path::new(&req.prl_path))
                .map_err(|e| Status::invalid_argument(e.to_string()))?
        } else {
            let mut channels = Vec::with_capacity(req.channels.len());
            for c in &req.channels {
                let bc = parse_band_classes(std::slice::from_ref(&c.band_class))?[0];
                channels.push((bc, c.channel as u16));
            }
            PrlScanPlan::from_parts(0, false, &channels, Vec::new())
        };
        if !req.band_classes.is_empty() {
            plan.retain_band_classes(&parse_band_classes(&req.band_classes)?);
        }
        if plan.channels.is_empty() {
            return Err(Status::invalid_argument("the plan yields no channels"));
        }
        let summary = plan.summary();
        let plan = Arc::new(plan);
        let dwell = if req.dwell_ms > 0 {
            req.dwell_ms
        } else {
            self.config.acquisition.scan_dwell_ms.unwrap_or(
                if self.config.radio.is_hardware_radio() {
                    crate::config::DEFAULT_SDR_SCAN_DWELL_MS
                } else {
                    crate::config::DEFAULT_SCAN_DWELL_MS
                },
            )
        };
        let scan = ScanConfig::new(plan.clone())
            .with_mode(mode)
            .with_dwell_ms(dwell);
        *self.plan.lock().unwrap() = Some(plan.clone());
        self.appliance.set_scan(Some(scan));
        self.appliance.power_off();
        self.appliance.power_on();
        Ok(Response::new(StartScanResponse {
            channels: plan.channels.len() as u32,
            summary,
        }))
    }

    async fn list_channels(&self, _request: Request<()>) -> Result<Response<ChannelList>, Status> {
        let plan = self.current_plan()?;
        Ok(Response::new(ChannelList {
            summary: plan.summary(),
            channels: plan
                .channels
                .iter()
                .map(|c| ScanChannelInfo {
                    band_class: c.band_class.as_str().to_ascii_lowercase(),
                    channel: c.channel as u32,
                    frequency_hz: c.frequency_hz,
                    acq_index: c.acq_index as u32,
                })
                .collect(),
        }))
    }

    async fn prl_verdict(
        &self,
        request: Request<PrlVerdictRequest>,
    ) -> Result<Response<PrlVerdictResponse>, Status> {
        let req = request.into_inner();
        let plan = self.current_plan()?;
        let verdict = plan.verdict(req.sid as u16, req.nid as u16);
        Ok(Response::new(PrlVerdictResponse {
            verdict: Some(verdict_to_proto(&verdict)),
        }))
    }

    async fn set_rx_gain(
        &self,
        request: Request<SetRxGainRequest>,
    ) -> Result<Response<()>, Status> {
        self.appliance
            .set_rx_gain(request.into_inner().gain_db)
            .map_err(Status::failed_precondition)?;
        Ok(Response::new(()))
    }

    async fn trim_tx_calibration(
        &self,
        request: Request<TxCalibrationTrim>,
    ) -> Result<Response<TxCalibrationState>, Status> {
        let req = request.into_inner();
        let trim = crate::radio::TxTrim {
            tx_reference_dbm: req.tx_reference_dbm.map(|v| v as f32),
            tx_delay_samples: req.tx_delay_samples,
            power_control: req.power_control,
        };
        let cal = self
            .appliance
            .trim_tx_calibration(trim)
            .map_err(Status::failed_precondition)?;
        Ok(Response::new(match cal {
            Some(c) => TxCalibrationState {
                supported: true,
                tx_reference_dbm: c.tx_reference_dbm as f64,
                tx_delay_samples: c.tx_delay_samples,
                power_control: c.power_control,
            },
            None => TxCalibrationState {
                supported: false,
                tx_reference_dbm: 0.0,
                tx_delay_samples: 0,
                power_control: false,
            },
        }))
    }

    async fn dump_forward(
        &self,
        request: Request<DumpForwardRequest>,
    ) -> Result<Response<()>, Status> {
        let req = request.into_inner();
        if req.seconds <= 0.0 {
            return Err(Status::invalid_argument("seconds must be positive"));
        }
        self.appliance
            .dump_forward(Path::new(&req.path), req.seconds)
            .map_err(Status::failed_precondition)?;
        Ok(Response::new(()))
    }

    async fn originate(
        &self,
        request: Request<OriginateRequest>,
    ) -> Result<Response<OriginateResponse>, Status> {
        let req = request.into_inner();
        self.appliance
            .originate(req.service_option as u16, req.dialed_digits);
        Ok(Response::new(OriginateResponse {
            success: true,
            message: "origination queued".to_string(),
        }))
    }

    async fn register(&self, _request: Request<()>) -> Result<Response<()>, Status> {
        self.appliance.register();
        Ok(Response::new(()))
    }

    async fn send_dtmf_burst(
        &self,
        request: Request<SendDtmfBurstRequest>,
    ) -> Result<Response<()>, Status> {
        let burst = crate::traffic::DtmfBurst::new(&request.into_inner().digits)
            .map_err(Status::invalid_argument)?;
        self.appliance
            .send_dtmf_burst(burst)
            .map_err(Status::failed_precondition)?;
        Ok(Response::new(()))
    }

    async fn hang_up(&self, _request: Request<()>) -> Result<Response<()>, Status> {
        self.appliance.hang_up();
        Ok(Response::new(()))
    }

    async fn answer(&self, _request: Request<()>) -> Result<Response<()>, Status> {
        self.appliance.answer();
        Ok(Response::new(()))
    }

    async fn push_voice_audio(
        &self,
        request: Request<VoicePcmFrame>,
    ) -> Result<Response<()>, Status> {
        let samples = request.into_inner().samples;
        let pcm: [i16; cdma_voice::SAMPLES_PER_FRAME] = samples
            .into_iter()
            .map(|sample| sample.clamp(i16::MIN as i32, i16::MAX as i32) as i16)
            .collect::<Vec<_>>()
            .try_into()
            .map_err(|samples: Vec<i16>| {
                Status::invalid_argument(format!(
                    "voice frame has {} samples, expected {}",
                    samples.len(),
                    cdma_voice::SAMPLES_PER_FRAME
                ))
            })?;
        self.appliance.push_voice_pcm(pcm);
        Ok(Response::new(()))
    }

    type StreamVoiceAudioStream = VoiceStream;

    async fn stream_voice_audio(
        &self,
        _request: Request<()>,
    ) -> Result<Response<Self::StreamVoiceAudioStream>, Status> {
        let mut rx = self.appliance.subscribe_voice();
        let stream = async_stream::stream! {
            let mut sequence = 0u64;
            loop {
                match rx.recv().await {
                    Ok(pcm) => {
                        yield Ok(VoicePcmFrame {
                            samples: pcm.into_iter().map(i32::from).collect(),
                            sequence,
                        });
                        sequence = sequence.wrapping_add(1);
                    }
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => break,
                }
            }
        };
        Ok(Response::new(Box::pin(stream)))
    }

    async fn send_sms(
        &self,
        request: Request<SendSmsRequest>,
    ) -> Result<Response<SendSmsResponse>, Status> {
        let req = request.into_inner();
        self.appliance.originate_sms(req.destination, req.text);
        Ok(Response::new(SendSmsResponse {
            success: true,
            message: "SMS origination queued".to_string(),
        }))
    }

    async fn dial_data(
        &self,
        _request: Request<DialDataRequest>,
    ) -> Result<Response<DialDataResponse>, Status> {
        Err(Status::unimplemented("packet data not yet implemented"))
    }

    async fn page_with_sms(
        &self,
        request: Request<PageWithSmsRequest>,
    ) -> Result<Response<PageWithSmsResponse>, Status> {
        let loopback = self
            .loopback
            .as_ref()
            .ok_or_else(|| Status::failed_precondition("needs the loopback sim radio"))?;
        let text = request.into_inner().text;
        let text = if text.is_empty() { "ping" } else { &text };
        loopback
            .page_with_sms(self.config.identity.esn, text)
            .map_err(Status::unavailable)?;
        Ok(Response::new(PageWithSmsResponse {
            success: true,
            message: format!("queued MT SMS to ESN 0x{:08X}", self.config.identity.esn),
        }))
    }

    async fn list_mobiles(&self, _request: Request<()>) -> Result<Response<MobileList>, Status> {
        let loopback = self
            .loopback
            .as_ref()
            .ok_or_else(|| Status::failed_precondition("needs the loopback sim radio"))?;
        Ok(Response::new(MobileList {
            mobiles: loopback
                .mobiles()
                .into_iter()
                .map(|(esn, imsi, state)| MobileEntry {
                    esn,
                    imsi,
                    state: mobile_state_to_proto(&state).into(),
                })
                .collect(),
        }))
    }

    type StreamEventsStream = EventStream;

    async fn stream_events(
        &self,
        _request: Request<()>,
    ) -> Result<Response<Self::StreamEventsStream>, Status> {
        let mut rx = self.appliance.subscribe();
        let stream = async_stream::stream! {
            loop {
                match rx.recv().await {
                    Ok(ev) => yield Ok(event_to_proto(&ev)),
                    Err(RecvError::Lagged(_)) => continue,
                    Err(RecvError::Closed) => break,
                }
            }
        };
        Ok(Response::new(Box::pin(stream)))
    }

    type StreamMetricsStream = MetricsStream;

    async fn stream_metrics(
        &self,
        _request: Request<()>,
    ) -> Result<Response<Self::StreamMetricsStream>, Status> {
        let status = self.appliance.status_handle();
        let station = self.appliance.diagnostics_handle();
        let stream = async_stream::stream! {
            let mut tick = tokio::time::interval(std::time::Duration::from_secs(1));
            loop {
                tick.tick().await;
                let s = status.lock().unwrap().clone();
                let pilot = station().map(|d| d.pilot).unwrap_or_default();
                yield Ok(MsMetrics {
                            rx_power_dbfs: pilot.rx_power_dbfs as f64,
                    sync_messages_decoded: s.sync_decoded,
                    paging_frames_decoded: s.overhead_updated,
                    access_probes_sent: s.access_probes,
                    pilot_ec_io_db: if pilot.ec_io_db.is_nan() { 0.0 } else { pilot.ec_io_db as f64 },
                });
            }
        };
        Ok(Response::new(Box::pin(stream)))
    }
}

pub async fn run_grpc_server(
    addr: SocketAddr,
    service: MsServiceImpl,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tonic::transport::Server::builder()
        .add_service(service.into_server())
        .serve(addr)
        .await?;
    Ok(())
}

pub async fn serve_on_listener(
    listener: tokio::net::TcpListener,
    service: MsServiceImpl,
) -> Result<(), Box<dyn std::error::Error + Send + Sync>> {
    tonic::transport::Server::builder()
        .add_service(service.into_server())
        .serve_with_incoming(tokio_stream::wrappers::TcpListenerStream::new(listener))
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ms::{AccessReason, ChannelAssignment, MsProtocolState};

    #[test]
    fn every_protocol_state_label_has_a_wire_value() {
        let assignment = ChannelAssignment {
            walsh_code: 10,
            frame_offset: 0,
            for_rc: 1,
            rev_rc: 1,
            pilot_pn: 0,
        };
        let states = [
            (MsProtocolState::PowerOff, ProtoMsState::Off),
            (
                MsProtocolState::SystemDetermination,
                ProtoMsState::SystemDetermination,
            ),
            (
                MsProtocolState::PilotAcquisition,
                ProtoMsState::PilotAcquisition,
            ),
            (
                MsProtocolState::SyncAcquisition,
                ProtoMsState::SyncAcquisition,
            ),
            (MsProtocolState::TimingChange, ProtoMsState::TimingChange),
            (MsProtocolState::Idle, ProtoMsState::Idle),
            (
                MsProtocolState::SystemAccess {
                    reason: AccessReason::AckOrder { ack_seq: 0 },
                },
                ProtoMsState::SystemAccess,
            ),
            (
                MsProtocolState::TrafficChannelInit {
                    assignment: assignment.clone(),
                },
                ProtoMsState::TrafficChannelInit,
            ),
            (
                MsProtocolState::TrafficChannel { assignment },
                ProtoMsState::TrafficChannel,
            ),
        ];
        for (state, wire) in states {
            assert_eq!(ms_state_to_proto(state.label()), wire, "{state:?}");
            assert_eq!(wire.label(), state.label());
        }
    }
}
