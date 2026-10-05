//! Mobile station protocol states (C.S0004-E §2.3).

use std::collections::{BTreeMap, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use cdma_bts::receiver::layer3::{
    AccessParametersMessage, CdmaChannelListMessage, ExtendedNeighborListMessage,
    ExtendedSystemParametersMessage, GeneralPageMessage, NeighborListMessage, PageRecord,
    PagingMessage, SystemParametersMessage, msg_type_name,
};
use cdma_common::consts::{BURST_TYPE_SMS, SR1_CHIP_RATE_HZ};
use cdma_common::lac::paging_messages::AlternativeTechnologiesInformationMessage;
use cdma_common::paging::{imsi_11_12_from_digits, imsi_s_from_imsi, mcc_from_digits};
use cdma_common::sms::decode_mt_sms;

use crate::event::EventSink;
use crate::forward_rx::SyncParameters;
use crate::forward_rx::directed::{
    DirectedBody, DirectedPdu, ORDER_INTERCEPT, ORDER_LOCK_UNTIL_POWER_CYCLED, ORDER_REGISTRATION,
    ORDER_RELEASE, ORDER_REORDER, order_name,
};
use crate::lac::ReceivedSeqs;
use crate::tx::MsAccessIdentity;

// C.S0005-E T42m bounds the delayed Layer 3 response after access acknowledgment.
const T42M_CHIPS: u64 = 12 * SR1_CHIP_RATE_HZ;
/// T30m bounds the interval without a valid common-channel message.
const T30M_CHIPS: u64 = 3 * SR1_CHIP_RATE_HZ;
/// T41m bounds collection of current overhead before an access attempt.
const T41M_CHIPS: u64 = 4 * SR1_CHIP_RATE_HZ;
/// T57m delays power-up registration after entering Idle (C.S0005-E §2.6.5.1.1).
pub const T57M: Duration = Duration::from_secs(20);
const T57M_CHIPS: u64 = T57M.as_secs() * SR1_CHIP_RATE_HZ;
const NANOS_PER_SECOND: u128 = 1_000_000_000;
/// Timer-based registration advances in 80 ms units.
const REGISTRATION_COUNT_CHIPS: u64 = 98_304;

/// MS protocol states (C.S0004-E 2.3).
#[derive(Debug, Clone, PartialEq)]
pub enum MsProtocolState {
    PowerOff,
    /// 2.3.1.1 System Determination
    SystemDetermination,
    /// 2.3.1.2 Pilot Channel Acquisition
    PilotAcquisition,
    /// 2.3.1.3 Sync Channel Acquisition
    SyncAcquisition,
    /// 2.3.1.4 Timing Change
    TimingChange,
    /// 2.3.2 Mobile Station Idle (monitoring the paging channel)
    Idle,
    /// 2.3.3 System Access
    SystemAccess {
        reason: AccessReason,
    },
    /// 2.6.3 Traffic Channel Initialization: assignment received, moving to the
    /// assigned traffic channel.
    TrafficChannelInit {
        assignment: ChannelAssignment,
    },
    /// 2.6.4 Mobile Station Control on the Traffic Channel: the reverse
    /// traffic channel is on the air and Layer 3 runs on the dedicated
    /// channels.
    TrafficChannel {
        assignment: ChannelAssignment,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum MsTimerExpiry {
    CommonChannel,
    AccessOverhead,
}

impl MsProtocolState {
    pub fn label(&self) -> &'static str {
        match self {
            MsProtocolState::PowerOff => "off",
            MsProtocolState::SystemDetermination => "system_determination",
            MsProtocolState::PilotAcquisition => "pilot_acquisition",
            MsProtocolState::SyncAcquisition => "sync_acquisition",
            MsProtocolState::TimingChange => "timing_change",
            MsProtocolState::Idle => "idle",
            MsProtocolState::SystemAccess { .. } => "system_access",
            MsProtocolState::TrafficChannelInit { .. } => "traffic_channel_init",
            MsProtocolState::TrafficChannel { .. } => "traffic_channel",
        }
    }

    pub fn on_traffic(&self) -> bool {
        matches!(
            self,
            MsProtocolState::TrafficChannelInit { .. } | MsProtocolState::TrafficChannel { .. }
        )
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct ChannelAssignment {
    pub walsh_code: u8,
    pub frame_offset: u8,
    pub for_rc: u8,
    pub rev_rc: u8,
    pub pilot_pn: u16,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AccessReason {
    Registration {
        reg_type: u8,
    },
    Origination {
        service_option: u16,
        digits: String,
    },
    PageResponse {
        service_option: u16,
        ack_seq: u8,
    },
    AckOrder {
        ack_seq: u8,
    },
    StatusResponse {
        ack_seq: u8,
        qual_info_type: u8,
        qual_info: Vec<u8>,
        record_types: Vec<u8>,
    },
}

impl AccessReason {
    pub fn expects_assignment(&self) -> bool {
        matches!(
            self,
            AccessReason::Origination { .. } | AccessReason::PageResponse { .. }
        )
    }
}

impl std::fmt::Display for AccessReason {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            AccessReason::Registration { reg_type } => write!(f, "registration(type={reg_type})"),
            AccessReason::Origination {
                service_option,
                digits,
            } => write!(f, "origination(so={service_option},digits={digits})"),
            AccessReason::PageResponse { service_option, .. } => {
                write!(f, "page_response(so={service_option})")
            }
            AccessReason::AckOrder { ack_seq } => write!(f, "ack_order(ack_seq={ack_seq})"),
            AccessReason::StatusResponse { record_types, .. } => {
                write!(f, "status_response(records={})", record_types.len())
            }
        }
    }
}

#[derive(Debug, Clone, Default)]
pub struct SystemParameters {
    pub sid: u16,
    pub nid: u16,
    pub base_id: u16,
    pub reg_zone: u16,
    pub total_zones: u8,
    pub power_up_reg: bool,
    pub power_down_reg: bool,
    pub parameter_reg: bool,
    pub home_reg: bool,
    pub for_sid_reg: bool,
    pub for_nid_reg: bool,
    /// REG_PRD: the registration period is 2^(REG_PRD/4) units of 80 ms, and
    /// zero means the cell asks for no timer-based registration.
    pub reg_prd: u8,
    pub config_msg_seq: u8,
}

#[derive(Debug, Clone, Default)]
pub struct AccessParameters {
    pub acc_msg_seq: u8,
    pub nom_pwr: i8,
    pub nom_pwr_ext: u8,
    pub init_pwr: i8,
    pub pwr_step: u8,
    pub num_step: u8,
    pub pam_sz: u8,
    pub max_cap_sz: u8,
    pub acc_chan: u8,
    pub probe_pn_ran: u8,
    pub acc_tmo: u8,
    pub probe_bkoff: u8,
    pub bkoff: u8,
    pub max_req_seq: u8,
    pub max_rsp_seq: u8,
    pub psist_0_9: u8,
    pub msg_psist: u8,
    pub reg_psist: u8,
}

#[derive(Debug, Clone, Default)]
pub struct OverheadCache {
    pub system_parameters: Option<SystemParametersMessage>,
    pub access_parameters: Option<AccessParametersMessage>,
    pub extended_system_parameters: Option<ExtendedSystemParametersMessage>,
    pub cdma_channel_list: Option<CdmaChannelListMessage>,
    pub neighbor_list: Option<NeighborListMessage>,
    pub extended_neighbor_list: Option<ExtendedNeighborListMessage>,
    pub alternative_technologies_information: Option<AlternativeTechnologiesInformationMessage>,
}

impl OverheadCache {
    pub fn received(&self) -> Vec<&'static str> {
        let mut out = Vec::new();
        if self.system_parameters.is_some() {
            out.push("SPM");
        }
        if self.access_parameters.is_some() {
            out.push("APM");
        }
        if self.extended_system_parameters.is_some() {
            out.push("ESPM");
        }
        if self.cdma_channel_list.is_some() {
            out.push("CCLM");
        }
        if self.neighbor_list.is_some() {
            out.push("NLM");
        }
        if self.extended_neighbor_list.is_some() {
            out.push("ENLM");
        }
        if self.alternative_technologies_information.is_some() {
            out.push("ATIM");
        }
        out
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct PagingStats {
    pub crc_valid: u64,
    pub crc_failed: u64,
    pub messages: BTreeMap<String, u64>,
    pub undecodable: BTreeMap<String, u64>,
    pub pages_for_me: u64,
}

impl PagingStats {
    pub fn record_message(&mut self, name: &str) {
        self.crc_valid += 1;
        *self.messages.entry(name.to_string()).or_default() += 1;
    }

    pub fn decode_failed(&self) -> u64 {
        self.undecodable.values().sum()
    }
}

#[derive(Debug, Clone, Default)]
struct RegistrationState {
    power_up_reg_performed: bool,
    registered: bool,
    last_reg_zone: Option<u16>,
    last_config_msg_seq: Option<u8>,
    last_failure_chip: Option<u64>,
    power_up_deadline_chip: Option<u64>,
    timer_deadline_chip: Option<u64>,
    deferred_registration: Option<u8>,
}

/// Wait after a failed registration attempt before overhead may trigger
/// another. Not a spec timer, it keeps a rejecting base station from being
/// probed continuously.
pub const REGISTRATION_RETRY_HOLDOFF_CHIPS: u64 = 5 * 1_228_800;

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum MsEvent {
    StateChange {
        from: String,
        to: String,
    },
    PilotAcquired {
        pn: u32,
    },
    SyncDecoded(SyncParameters),
    PilotMeasurement {
        ec_io_db: Option<f32>,
        rx_power_dbfs: f32,
    },
    OverheadUpdated {
        sid: u16,
        nid: u16,
        base_id: u16,
    },
    AccessParametersUpdated,
    RegistrationNeeded {
        reg_type: u8,
    },
    PageReceived {
        esn: u32,
        service_option: u16,
    },
    AccessStarted {
        reason: String,
    },
    AccessProbe {
        reason: String,
        /// Layer 2 sequence number of the access PDU carried by this probe.
        msg_seq: u8,
        ack_req: bool,
        /// Forward MSG_SEQ acknowledged by this PDU when VALID_ACK is set.
        ack_seq: Option<u8>,
        sequence: u32,
        /// Probe number within the sequence, which is also PWR_LVL.
        probe: u32,
        power_dbm: f32,
        /// System time chip of the first preamble chip.
        start_chip: u64,
    },
    AccessComplete {
        reason: String,
    },
    AccessFailed {
        reason: String,
        cause: String,
    },
    RegistrationAccepted,
    RegistrationRejected {
        ordq: u8,
    },
    DirectedMessage {
        name: String,
        order: Option<String>,
        msg_seq: u8,
        ack_req: bool,
        /// Reverse MSG_SEQ acknowledged when VALID_ACK is set.
        #[serde(default)]
        ack_seq: Option<u8>,
    },
    SmsReceived {
        originating_number: String,
        text: String,
    },
    ChannelAssignment {
        walsh_code: u8,
        frame_offset: u8,
        for_rc: u8,
        rev_rc: u8,
    },
    TrafficChannelUp {
        walsh_code: u8,
    },
    TrafficMessage {
        name: String,
        order: Option<String>,
        msg_seq: u8,
        ack_seq: u8,
        ack_req: bool,
    },
    TrafficTransmit {
        name: String,
        msg_seq: u8,
        ack_seq: u8,
        ack_req: bool,
        retransmission: bool,
    },
    ServiceConnected {
        service_option: u16,
    },
    CallRinging {
        caller_number: Option<String>,
        service_option: u16,
    },
    CallAnswered,
    StatusRequested {
        record_types: Vec<u8>,
    },
    SmsCauseCode {
        error_class: u8,
    },
    TrafficReleased {
        by: String,
    },
    ScanStarted {
        prl_id: u16,
        channels: u32,
    },
    ChannelTuned {
        band_class: String,
        channel: u16,
        frequency_hz: u64,
        index: u32,
        total: u32,
    },
    PilotSearch {
        band_class: String,
        channel: u16,
        found: bool,
        /// `None` until the meter has a value, or when no pilot was found.
        ec_io_db: Option<f32>,
        rx_power_dbfs: f32,
    },
    PrlVerdict {
        sid: u16,
        nid: u16,
        permitted: bool,
        reason: String,
        record: Option<u32>,
        roaming_indicator: Option<u8>,
    },
    PagingMessageDecoded {
        name: String,
        config_msg_seq: Option<u8>,
    },
    DumpFinished {
        path: String,
        samples: u64,
    },
    /// The scan ended: `camped`, `no_permitted_system`, `survey` or
    /// `source_exhausted`.
    ScanFinished {
        result: String,
        elapsed_ms: u64,
        channels: u32,
        pilots: u32,
        syncs: u32,
    },
}

impl MsEvent {
    pub fn name(&self) -> &'static str {
        match self {
            MsEvent::StateChange { .. } => "state_change",
            MsEvent::PilotAcquired { .. } => "pilot_acquired",
            MsEvent::SyncDecoded(_) => "sync_decoded",
            MsEvent::PilotMeasurement { .. } => "pilot_measurement",
            MsEvent::OverheadUpdated { .. } => "overhead_updated",
            MsEvent::AccessParametersUpdated => "access_parameters_updated",
            MsEvent::RegistrationNeeded { .. } => "registration_needed",
            MsEvent::PageReceived { .. } => "page_received",
            MsEvent::AccessStarted { .. } => "access_started",
            MsEvent::AccessProbe { .. } => "access_probe",
            MsEvent::AccessComplete { .. } => "access_complete",
            MsEvent::AccessFailed { .. } => "access_failed",
            MsEvent::RegistrationAccepted => "registration_accepted",
            MsEvent::RegistrationRejected { .. } => "registration_rejected",
            MsEvent::DirectedMessage { .. } => "directed_message",
            MsEvent::SmsReceived { .. } => "sms_received",
            MsEvent::ChannelAssignment { .. } => "channel_assignment",
            MsEvent::TrafficChannelUp { .. } => "traffic_channel_up",
            MsEvent::TrafficMessage { .. } => "traffic_message",
            MsEvent::TrafficTransmit { .. } => "traffic_transmit",
            MsEvent::ServiceConnected { .. } => "service_connected",
            MsEvent::CallRinging { .. } => "call_ringing",
            MsEvent::CallAnswered => "call_answered",
            MsEvent::StatusRequested { .. } => "status_requested",
            MsEvent::SmsCauseCode { .. } => "sms_cause_code",
            MsEvent::TrafficReleased { .. } => "traffic_released",
            MsEvent::ScanStarted { .. } => "scan_started",
            MsEvent::ChannelTuned { .. } => "channel_tuned",
            MsEvent::PilotSearch { .. } => "pilot_search",
            MsEvent::PrlVerdict { .. } => "prl_verdict",
            MsEvent::PagingMessageDecoded { .. } => "paging_message_decoded",
            MsEvent::DumpFinished { .. } => "dump_finished",
            MsEvent::ScanFinished { .. } => "scan_finished",
        }
    }
}

pub struct MsCore {
    state: MsProtocolState,
    identity: MsAccessIdentity,
    sinks: Vec<Arc<dyn EventSink>>,
    system_params: Option<SystemParameters>,
    access_params: Option<AccessParameters>,
    sync_info: Option<SyncParameters>,
    overhead: OverheadCache,
    paging: PagingStats,
    reg: RegistrationState,
    pending_access: Option<AccessReason>,
    access_response_deadline_chip: Option<u64>,
    registration_response_paging: Option<PagingStats>,
    common_channel_deadline_chip: Option<u64>,
    access_overhead_deadline_chip: Option<u64>,
    service_option: Option<u16>,
    incoming_call: bool,
    power_up_delay_chips: u64,
    pending_acks: VecDeque<u8>,
    rcvd: ReceivedSeqs,
}

impl MsCore {
    pub fn new(identity: MsAccessIdentity) -> Self {
        MsCore {
            state: MsProtocolState::PowerOff,
            identity,
            sinks: Vec::new(),
            system_params: None,
            access_params: None,
            sync_info: None,
            overhead: OverheadCache::default(),
            paging: PagingStats::default(),
            reg: RegistrationState::default(),
            pending_access: None,
            access_response_deadline_chip: None,
            registration_response_paging: None,
            common_channel_deadline_chip: None,
            access_overhead_deadline_chip: None,
            service_option: None,
            incoming_call: false,
            power_up_delay_chips: T57M_CHIPS,
            pending_acks: VecDeque::new(),
            rcvd: ReceivedSeqs::default(),
        }
    }

    /// Zero disables T57m, so power-up registration follows the first System
    /// Parameters Message (C.S0005-E §2.6.5.5.2.1).
    pub fn with_power_up_delay(mut self, delay: Duration) -> Self {
        self.power_up_delay_chips = duration_chips(delay);
        self
    }

    pub fn add_event_sink(&mut self, sink: Arc<dyn EventSink>) {
        self.sinks.push(sink);
    }

    pub(crate) fn emit(&self, event: MsEvent) {
        for sink in &self.sinks {
            sink.emit(&event);
        }
    }

    pub fn state(&self) -> &MsProtocolState {
        &self.state
    }

    pub fn system_params(&self) -> Option<&SystemParameters> {
        self.system_params.as_ref()
    }

    pub fn access_params(&self) -> Option<&AccessParameters> {
        self.access_params.as_ref()
    }

    pub fn overhead(&self) -> &OverheadCache {
        &self.overhead
    }

    pub fn paging_stats(&self) -> &PagingStats {
        &self.paging
    }

    pub fn on_paging_crc_failed(&mut self) {
        self.paging.crc_failed += 1;
    }

    pub fn on_paging_decode_failed(&mut self, msg_type: u8, error: &str) {
        self.paging.crc_valid += 1;
        let name = msg_type_name(msg_type);
        let count = self.paging.undecodable.entry(name.to_string()).or_default();
        *count += 1;
        if *count == 1 {
            log::warn!(
                "ms_rx: paging message type {} ({}) is CRC-valid but does not decode: {}",
                msg_type,
                name,
                error
            );
        }
    }

    pub fn overhead_ready_for_access(&self) -> bool {
        let Some(spm) = self.overhead.system_parameters.as_ref() else {
            return false;
        };
        let espm_ready =
            !spm.ext_sys_parameter || self.overhead.extended_system_parameters.is_some();
        self.system_params.is_some() && self.access_params.is_some() && espm_ready
    }

    /// The serving base station's protocol revision: the Extended System
    /// Parameters Message value once it has arrived, the Sync Channel Message
    /// value until then (C.S0005-E §2.6.2.2.5).
    pub fn base_station_p_rev(&self) -> Option<u8> {
        self.overhead
            .extended_system_parameters
            .as_ref()
            .map(|espm| espm.p_rev)
            .or_else(|| self.sync_info.as_ref().map(|sync| sync.p_rev))
    }

    pub fn sync_info(&self) -> Option<&SyncParameters> {
        self.sync_info.as_ref()
    }

    pub fn pending_access(&self) -> Option<&AccessReason> {
        self.pending_access.as_ref()
    }

    pub fn take_pending_ack(&mut self) -> Option<u8> {
        self.pending_acks.pop_front()
    }

    pub fn pending_service_option(&self) -> Option<u16> {
        self.service_option
    }

    pub fn incoming_call(&self) -> bool {
        self.incoming_call
    }

    pub fn identity(&self) -> &MsAccessIdentity {
        &self.identity
    }

    fn set_state(&mut self, new_state: MsProtocolState) {
        let from = self.state.label().to_string();
        let to = new_state.label().to_string();
        self.state = new_state;
        self.emit(MsEvent::StateChange { from, to });
    }

    fn forget_system(&mut self) {
        self.system_params = None;
        self.access_params = None;
        self.sync_info = None;
        self.overhead = OverheadCache::default();
        self.paging = PagingStats::default();
        self.reg = RegistrationState::default();
        self.pending_access = None;
        self.incoming_call = false;
        self.access_response_deadline_chip = None;
        self.registration_response_paging = None;
        self.common_channel_deadline_chip = None;
        self.access_overhead_deadline_chip = None;
        self.pending_acks.clear();
        self.rcvd.clear();
    }

    pub fn power_on(&mut self) {
        if self.state == MsProtocolState::PowerOff {
            self.forget_system();
            self.set_state(MsProtocolState::SystemDetermination);
            self.set_state(MsProtocolState::PilotAcquisition);
        }
    }

    pub fn power_on_scanning(&mut self) {
        if self.state == MsProtocolState::PowerOff {
            self.forget_system();
            self.set_state(MsProtocolState::SystemDetermination);
        }
    }

    pub fn begin_pilot_search(&mut self) {
        if self.state == MsProtocolState::SystemDetermination {
            self.set_state(MsProtocolState::PilotAcquisition);
        }
    }

    pub fn on_pilot_detected(&mut self) {
        if self.state == MsProtocolState::PilotAcquisition {
            self.set_state(MsProtocolState::SyncAcquisition);
        }
    }

    pub fn on_forward_link_lost(&mut self, reason: &'static str, now_chips: u64) {
        if self.state == MsProtocolState::PowerOff {
            return;
        }
        self.on_access_failed(reason, now_chips);
        self.on_traffic_released(reason);
        let paging = std::mem::take(&mut self.paging);
        self.forget_system();
        self.paging = paging;
        self.set_state(MsProtocolState::PilotAcquisition);
    }

    pub fn on_system_rejected(&mut self) {
        if matches!(
            self.state,
            MsProtocolState::PilotAcquisition | MsProtocolState::SyncAcquisition
        ) {
            self.sync_info = None;
            self.set_state(MsProtocolState::SystemDetermination);
        }
    }

    pub fn power_off(&mut self) {
        if self.state == MsProtocolState::PowerOff {
            return;
        }
        self.pending_access = None;
        self.access_response_deadline_chip = None;
        self.set_state(MsProtocolState::PowerOff);
    }

    pub fn registered(&self) -> bool {
        self.reg.registered
    }

    pub fn on_pilot_acquired(&mut self, pn: u32) {
        if self.sync_info.is_none() {
            self.emit(MsEvent::PilotAcquired { pn });
        }
        if matches!(
            self.state,
            MsProtocolState::SystemDetermination | MsProtocolState::PilotAcquisition
        ) {
            self.set_state(MsProtocolState::SyncAcquisition);
        }
    }

    pub fn on_sync_decoded(&mut self, params: SyncParameters) {
        if self.sync_info.is_none() {
            self.emit(MsEvent::SyncDecoded(params.clone()));
        }
        self.sync_info = Some(params);
        if matches!(
            self.state,
            MsProtocolState::SystemDetermination
                | MsProtocolState::PilotAcquisition
                | MsProtocolState::SyncAcquisition
                | MsProtocolState::TimingChange
        ) {
            self.set_state(MsProtocolState::TimingChange);
            self.set_state(MsProtocolState::Idle);
        }
    }

    pub fn begin_idle_timers(&mut self, now_chips: u64) {
        self.common_channel_deadline_chip = Some(now_chips.saturating_add(T30M_CHIPS));
        if !self.reg.registered
            && self.reg.power_up_deadline_chip.is_none()
            && self.power_up_delay_chips > 0
        {
            self.reg.power_up_deadline_chip =
                Some(now_chips.saturating_add(self.power_up_delay_chips));
        }
    }

    pub(crate) fn maintain_idle_timers(&mut self, now_chips: u64) -> Option<MsTimerExpiry> {
        if self
            .common_channel_deadline_chip
            .is_some_and(|deadline| now_chips >= deadline)
        {
            self.common_channel_deadline_chip = None;
            return Some(MsTimerExpiry::CommonChannel);
        }
        if self.state != MsProtocolState::Idle {
            if matches!(self.state, MsProtocolState::SystemAccess { .. })
                && self.pending_access.is_some()
                && !self.overhead_ready_for_access()
            {
                let deadline = self
                    .access_overhead_deadline_chip
                    .get_or_insert_with(|| now_chips.saturating_add(T41M_CHIPS));
                if now_chips >= *deadline {
                    self.access_overhead_deadline_chip = None;
                    return Some(MsTimerExpiry::AccessOverhead);
                }
            } else {
                self.access_overhead_deadline_chip = None;
            }
            return None;
        }
        if self
            .reg
            .power_up_deadline_chip
            .is_some_and(|deadline| now_chips >= deadline)
        {
            self.reg.power_up_deadline_chip = None;
            if let Some(reg_type) = self.reg.deferred_registration.take() {
                if self.registration_allowed(now_chips) {
                    self.on_registration_needed(reg_type);
                    return None;
                }
            } else if self
                .system_params
                .as_ref()
                .is_some_and(|sp| sp.power_up_reg)
                && self.registration_allowed(now_chips)
            {
                self.on_registration_needed(REG_TYPE_POWER_UP);
                return None;
            }
        }
        if self
            .reg
            .timer_deadline_chip
            .is_some_and(|deadline| now_chips >= deadline)
            && self.registration_allowed(now_chips)
        {
            self.reg.timer_deadline_chip = None;
            self.on_registration_needed(REG_TYPE_TIMER);
        }
        None
    }

    pub fn ingest_paging_message(&mut self, msg: &PagingMessage, now_chips: u64) {
        if matches!(
            self.state,
            MsProtocolState::Idle | MsProtocolState::SystemAccess { .. }
        ) {
            self.common_channel_deadline_chip = Some(now_chips.saturating_add(T30M_CHIPS));
        }
        let (name, config_msg_seq) = match msg {
            PagingMessage::SystemParameters(m) => ("SPM", Some(m.config_msg_seq)),
            PagingMessage::AccessParameters(m) => ("APM", Some(m.acc_msg_seq)),
            PagingMessage::NeighborList(m) => ("NLM", Some(m.config_msg_seq)),
            PagingMessage::ExtendedNeighborList(m) => ("ENLM", Some(m.config_msg_seq)),
            PagingMessage::CdmaChannelList(m) => ("CCLM", Some(m.config_msg_seq)),
            PagingMessage::ExtendedSystemParameters(m) => ("ESPM", Some(m.config_msg_seq)),
            PagingMessage::GeneralPage(m) => ("GPM", Some(m.config_msg_seq)),
            PagingMessage::Order(_) => ("Order", None),
            PagingMessage::ChannelAssignment(_) => ("CAM", None),
            PagingMessage::ExtendedChannelAssignment(_) => ("ECAM", None),
            PagingMessage::AlternativeTechnologiesInformation(m) => {
                ("ATIM", Some(m.config_msg_seq))
            }
        };
        self.paging.record_message(name);
        self.emit(MsEvent::PagingMessageDecoded {
            name: name.to_string(),
            config_msg_seq,
        });
        match msg {
            PagingMessage::SystemParameters(spm) => {
                self.overhead.system_parameters = Some(spm.clone());
                self.apply_system_parameters_at(parse_system_parameters(spm), now_chips)
            }
            PagingMessage::AccessParameters(apm) => {
                self.overhead.access_parameters = Some(apm.clone());
                self.apply_access_parameters(parse_access_parameters(apm))
            }
            PagingMessage::ExtendedSystemParameters(m) => {
                self.overhead.extended_system_parameters = Some(m.clone())
            }
            PagingMessage::CdmaChannelList(m) => self.overhead.cdma_channel_list = Some(m.clone()),
            PagingMessage::NeighborList(m) => self.overhead.neighbor_list = Some(m.clone()),
            PagingMessage::ExtendedNeighborList(m) => {
                self.overhead.extended_neighbor_list = Some(m.clone())
            }
            PagingMessage::AlternativeTechnologiesInformation(m) => {
                self.overhead.alternative_technologies_information = Some(m.clone())
            }
            PagingMessage::GeneralPage(gpm) => self.on_general_page(gpm),
            _ => {}
        }
    }

    fn on_general_page(&mut self, gpm: &GeneralPageMessage) {
        for rec in &gpm.page_records {
            let addressed = match rec {
                PageRecord::Class0 {
                    imsi_s,
                    imsi_11_12,
                    mcc,
                    service_option,
                    msg_seq,
                    ..
                } if self.class0_page_matches(*imsi_s, *imsi_11_12, *mcc) => {
                    Some((*service_option, *msg_seq))
                }
                PageRecord::Class1 {
                    esn,
                    service_option,
                    msg_seq,
                    ..
                } if *esn == self.identity.esn => Some((*service_option, *msg_seq)),
                _ => None,
            };
            if let Some((service_option, msg_seq)) = addressed {
                self.paging.pages_for_me += 1;
                self.on_page_received(self.identity.esn, service_option.unwrap_or(0), msg_seq);
                return;
            }
        }
    }

    fn class0_page_matches(
        &self,
        imsi_s: Option<u64>,
        imsi_11_12: Option<u8>,
        mcc: Option<u16>,
    ) -> bool {
        let Some((s1, s2)) = imsi_s_from_imsi(&format!("{:010}", self.identity.imsi_s)) else {
            return false;
        };
        if imsi_s != Some((u64::from(s2) << 24) | u64::from(s1)) {
            return false;
        }
        let Some(identity_mcc) = mcc_from_digits(&self.identity.mcc) else {
            return false;
        };
        let Some(identity_imsi_11_12) = imsi_11_12_from_digits(&self.identity.imsi_11_12) else {
            return false;
        };
        let overhead = self.overhead.extended_system_parameters.as_ref();
        mcc.or_else(|| overhead.map(|message| message.mcc)) == Some(identity_mcc)
            && imsi_11_12.or_else(|| overhead.map(|message| message.imsi_11_12))
                == Some(identity_imsi_11_12)
    }

    pub fn on_page_received(&mut self, esn: u32, service_option: u16, msg_seq: u8) {
        self.emit(MsEvent::PageReceived {
            esn,
            service_option,
        });
        if self.state == MsProtocolState::Idle {
            let reason = AccessReason::PageResponse {
                service_option,
                ack_seq: msg_seq,
            };
            self.service_option = Some(service_option);
            self.incoming_call = true;
            self.pending_access = Some(reason.clone());
            self.emit(MsEvent::AccessStarted {
                reason: reason.to_string(),
            });
            self.set_state(MsProtocolState::SystemAccess { reason });
        }
    }

    pub fn on_access_acknowledged(&mut self, now_chips: u64) {
        let MsProtocolState::SystemAccess { reason } = self.state.clone() else {
            return;
        };
        self.pending_access = None;
        if matches!(
            reason,
            AccessReason::Origination { .. } | AccessReason::PageResponse { .. }
        ) {
            self.note_successful_registration(now_chips);
        }
        if matches!(reason, AccessReason::Registration { .. }) || reason.expects_assignment() {
            self.access_response_deadline_chip = Some(now_chips + T42M_CHIPS);
            if matches!(reason, AccessReason::Registration { .. }) {
                self.registration_response_paging = Some(self.paging.clone());
                return;
            }
        }
        self.emit(MsEvent::AccessComplete {
            reason: reason.to_string(),
        });
        if reason.expects_assignment() {
            return;
        }
        self.access_response_deadline_chip = None;
        self.set_state(MsProtocolState::Idle);
    }

    /// The probe sequence ran out with nothing acknowledged. A base station
    /// that answers an origination with the channel assignment itself, rather
    /// than acknowledging on receipt, is still setting the call up when the
    /// last probe goes out, so keep waiting for the assignment instead of
    /// giving up on a transaction the network is still working on. Returns
    /// whether the wait was taken up, leaving the mobile in System Access.
    pub fn on_access_probes_exhausted(&mut self, now_chips: u64) -> bool {
        let MsProtocolState::SystemAccess { reason } = self.state.clone() else {
            return false;
        };
        if !reason.expects_assignment() {
            return false;
        }
        log::info!("cdma-ms: access {reason} probes spent, waiting for the assignment");
        self.pending_access = None;
        self.access_response_deadline_chip = Some(now_chips + T42M_CHIPS);
        true
    }

    pub fn expire_access_response_wait(&mut self, now_chips: u64) {
        if self
            .access_response_deadline_chip
            .is_some_and(|deadline| now_chips >= deadline)
        {
            let cause = match &self.state {
                MsProtocolState::SystemAccess {
                    reason: AccessReason::Registration { .. },
                } => {
                    if let Some(start) = &self.registration_response_paging {
                        log::warn!(
                            "cdma-ms: registration response wait paging: crc_valid={} crc_failed={} undecodable={} directed_orders={}",
                            self.paging.crc_valid.saturating_sub(start.crc_valid),
                            self.paging.crc_failed.saturating_sub(start.crc_failed),
                            self.paging
                                .decode_failed()
                                .saturating_sub(start.decode_failed()),
                            self.paging
                                .messages
                                .get("ORDM")
                                .copied()
                                .unwrap_or_default()
                                .saturating_sub(
                                    start.messages.get("ORDM").copied().unwrap_or_default()
                                )
                        );
                    }
                    "registration response timeout"
                }
                _ => "assignment timeout",
            };
            self.on_access_failed(cause, now_chips);
        }
    }

    pub fn on_access_failed(&mut self, cause: &str, now_chips: u64) {
        let MsProtocolState::SystemAccess { reason } = self.state.clone() else {
            return;
        };
        log::warn!("cdma-ms: access {} failed: {}", reason, cause);
        self.emit(MsEvent::AccessFailed {
            reason: reason.to_string(),
            cause: cause.to_string(),
        });
        if matches!(reason, AccessReason::Registration { .. }) {
            self.reg.last_failure_chip = Some(now_chips);
        }
        self.pending_access = None;
        self.access_response_deadline_chip = None;
        self.access_overhead_deadline_chip = None;
        self.registration_response_paging = None;
        self.set_state(MsProtocolState::Idle);
    }

    pub fn note_directed_seq(&mut self, msg_seq: u8) -> bool {
        self.rcvd.mark(msg_seq)
    }

    /// Duplicates must still be acknowledged (C.S0004-E §2.1.1.2.2.2).
    pub fn acknowledge_duplicate(&mut self, msg_seq: u8) {
        if !self.pending_acks.contains(&msg_seq) {
            self.pending_acks.push_back(msg_seq);
        }
    }

    pub fn on_directed(&mut self, pdu: &DirectedPdu, now_chips: u64) {
        let order = match &pdu.body {
            DirectedBody::Order { order, .. } => Some(order_name(*order).to_string()),
            _ => None,
        };
        self.paging.record_message(pdu.name());
        self.emit(MsEvent::DirectedMessage {
            name: pdu.name().to_string(),
            order,
            msg_seq: pdu.msg_seq,
            ack_req: pdu.ack_req,
            ack_seq: pdu.valid_ack.then_some(pdu.ack_seq),
        });
        let mut answered = false;
        match &pdu.body {
            DirectedBody::Order { order, ordq, .. } => {
                self.on_directed_order(*order, *ordq, now_chips)
            }
            DirectedBody::DataBurst {
                burst_type, fields, ..
            } => {
                self.on_data_burst(*burst_type, fields);
            }
            DirectedBody::ChannelAssignment(assignment) => {
                answered = self.on_channel_assignment(assignment.clone());
            }
            DirectedBody::StatusRequest {
                qual_info_type,
                qual_info,
                record_types,
            } => {
                answered = self.on_status_request(
                    pdu.msg_seq,
                    *qual_info_type,
                    qual_info.clone(),
                    record_types.clone(),
                );
            }
            DirectedBody::Unparsed => {}
        }
        if pdu.ack_req && !answered && !self.state.on_traffic() {
            self.pending_acks.push_back(pdu.msg_seq);
        }
    }

    fn on_status_request(
        &mut self,
        ack_seq: u8,
        qual_info_type: u8,
        qual_info: Vec<u8>,
        record_types: Vec<u8>,
    ) -> bool {
        log::info!(
            "cdma-ms: status request for records {:02x?}, answering",
            record_types
        );
        self.emit(MsEvent::StatusRequested {
            record_types: record_types.clone(),
        });
        self.pending_access = Some(AccessReason::StatusResponse {
            ack_seq,
            qual_info_type,
            qual_info,
            record_types,
        });
        true
    }

    fn on_directed_order(&mut self, order: u8, ordq: u8, now_chips: u64) {
        match order {
            ORDER_REGISTRATION => self.on_registration_order(ordq, now_chips),
            ORDER_RELEASE | ORDER_REORDER | ORDER_INTERCEPT => {
                if let MsProtocolState::SystemAccess { reason } = &self.state {
                    if reason.expects_assignment() {
                        let cause = format!(
                            "{} order (ordq {})",
                            order_name(order).to_ascii_lowercase(),
                            ordq
                        );
                        self.on_access_failed(&cause, now_chips);
                    }
                }
            }
            ORDER_LOCK_UNTIL_POWER_CYCLED => {
                log::warn!("cdma-ms: Lock Until Power-Cycled Order received, access locked");
            }
            _ => {}
        }
    }

    fn on_registration_order(&mut self, ordq: u8, now_chips: u64) {
        match ordq {
            0 | 5 | 7 => {
                self.note_successful_registration(now_chips);
                self.emit(MsEvent::RegistrationAccepted);
                if let MsProtocolState::SystemAccess {
                    reason: AccessReason::Registration { reg_type },
                } = &self.state
                {
                    if *reg_type == REG_TYPE_POWER_UP {
                        self.reg.power_up_reg_performed = true;
                    }
                    self.reg.power_up_deadline_chip = None;
                    self.emit(MsEvent::AccessComplete {
                        reason: AccessReason::Registration {
                            reg_type: *reg_type,
                        }
                        .to_string(),
                    });
                    self.pending_access = None;
                    self.access_response_deadline_chip = None;
                    self.registration_response_paging = None;
                    self.set_state(MsProtocolState::Idle);
                }
            }
            1 => self.on_registration_needed(REG_TYPE_ORDERED),
            2 | 4 => {
                self.reg.registered = false;
                self.emit(MsEvent::RegistrationRejected { ordq });
                if matches!(
                    self.state,
                    MsProtocolState::SystemAccess {
                        reason: AccessReason::Registration { .. }
                    }
                ) {
                    self.on_access_failed("registration rejected", now_chips);
                }
            }
            _ => log::warn!("cdma-ms: unsupported Registration Order qualifier {ordq}"),
        }
    }

    fn note_successful_registration(&mut self, now_chips: u64) {
        self.reg.registered = true;
        self.reg.last_failure_chip = None;
        if let Some(sp) = &self.system_params {
            self.reg.last_reg_zone = Some(sp.reg_zone);
            self.reg.last_config_msg_seq = Some(sp.config_msg_seq);
            self.reg.timer_deadline_chip = registration_period_chips(sp.reg_prd)
                .map(|period| now_chips.saturating_add(period));
        }
    }

    /// A Data Burst addressed to this mobile on the paging or traffic channel.
    /// Returns the SMS Cause Code error class when the network responds to an MO SMS.
    pub fn on_data_burst(&mut self, burst_type: u8, fields: &[u8]) -> Option<u8> {
        if burst_type != BURST_TYPE_SMS {
            return None;
        }
        match decode_mt_sms(fields) {
            Some(sms) if sms.tl_msg_type == SMS_CAUSE_CODE_TL_TYPE => {
                let error_class = sms.error_class.unwrap_or(0);
                self.on_sms_cause_code(error_class);
                Some(error_class)
            }
            Some(sms) => {
                log::info!(
                    "cdma-ms: SMS from {}: {:?}",
                    sms.originating_number,
                    sms.text
                );
                self.emit(MsEvent::SmsReceived {
                    originating_number: sms.originating_number,
                    text: sms.text,
                });
                None
            }
            None => {
                log::warn!(
                    "cdma-ms: SMS Data Burst of {} octets did not decode",
                    fields.len()
                );
                None
            }
        }
    }

    pub fn on_traffic_channel_up(&mut self) {
        if let MsProtocolState::TrafficChannelInit { assignment } = self.state.clone() {
            self.emit(MsEvent::TrafficChannelUp {
                walsh_code: assignment.walsh_code,
            });
            self.set_state(MsProtocolState::TrafficChannel { assignment });
        }
    }

    pub fn on_traffic_released(&mut self, by: &str) {
        if self.state.on_traffic() {
            self.emit(MsEvent::TrafficReleased { by: by.to_string() });
            self.incoming_call = false;
            self.pending_acks.clear();
            self.set_state(MsProtocolState::Idle);
        }
    }

    fn registration_allowed(&self, now_chips: u64) -> bool {
        match self.reg.last_failure_chip {
            Some(failed) => now_chips.saturating_sub(failed) >= REGISTRATION_RETRY_HOLDOFF_CHIPS,
            None => true,
        }
    }

    pub fn apply_system_parameters(&mut self, sp: SystemParameters) {
        self.apply_system_parameters_at(sp, 0)
    }

    pub fn apply_system_parameters_at(&mut self, sp: SystemParameters, now_chips: u64) {
        let changed = self
            .system_params
            .as_ref()
            .is_none_or(|prev| prev.config_msg_seq != sp.config_msg_seq);
        if changed {
            log::info!(
                "cdma-ms: system parameters config_msg_seq={} sid={} nid={} base_id={} reg_zone={} \
                 reg_prd={} home_reg={} for_sid_reg={} for_nid_reg={} power_up_reg={} parameter_reg={}",
                sp.config_msg_seq,
                sp.sid,
                sp.nid,
                sp.base_id,
                sp.reg_zone,
                sp.reg_prd,
                sp.home_reg,
                sp.for_sid_reg,
                sp.for_nid_reg,
                sp.power_up_reg,
                sp.parameter_reg,
            );
            self.emit(MsEvent::OverheadUpdated {
                sid: sp.sid,
                nid: sp.nid,
                base_id: sp.base_id,
            });
        }

        let mut reg_needed: Option<u8> = None;
        if sp.power_up_reg && !self.reg.power_up_reg_performed {
            reg_needed = Some(REG_TYPE_POWER_UP);
        }
        if reg_needed.is_none() {
            if sp.total_zones != 0
                && let Some(last_zone) = self.reg.last_reg_zone
            {
                if last_zone != sp.reg_zone {
                    reg_needed = Some(REG_TYPE_ZONE);
                }
            }
        }
        if reg_needed.is_none() && sp.parameter_reg {
            if let Some(last_seq) = self.reg.last_config_msg_seq {
                if last_seq != sp.config_msg_seq {
                    reg_needed = Some(REG_TYPE_PARAMETER);
                }
            }
        }

        self.reg.timer_deadline_chip = match (self.reg.timer_deadline_chip, sp.reg_prd) {
            (_, 0) => None,
            (Some(deadline), _) => Some(deadline),
            (None, reg_prd) if self.reg.registered => {
                registration_period_chips(reg_prd).map(|period| now_chips.saturating_add(period))
            }
            (None, _) => None,
        };
        self.system_params = Some(sp);

        if let Some(reg_type) = reg_needed {
            if self.registration_allowed(now_chips) {
                self.on_registration_needed(reg_type);
            }
        }
    }

    pub fn apply_access_parameters(&mut self, ap: AccessParameters) {
        let changed = self
            .access_params
            .as_ref()
            .is_none_or(|prev| prev.acc_msg_seq != ap.acc_msg_seq);
        self.access_params = Some(ap);
        if changed {
            let ap = self.access_params.as_ref().expect("just set");
            log::info!(
                "cdma-ms: access parameters acc_msg_seq={} nom_pwr={} init_pwr={} pwr_step={} num_step={} \
                 max_req_seq={} max_rsp_seq={} acc_tmo={} probe_bkoff={} bkoff={} probe_pn_ran={} \
                 acc_chan={} pam_sz={} max_cap_sz={} psist_0_9={} msg_psist={} reg_psist={}",
                ap.acc_msg_seq,
                ap.nom_pwr,
                ap.init_pwr,
                ap.pwr_step,
                ap.num_step,
                ap.max_req_seq,
                ap.max_rsp_seq,
                ap.acc_tmo,
                ap.probe_bkoff,
                ap.bkoff,
                ap.probe_pn_ran,
                ap.acc_chan,
                ap.pam_sz,
                ap.max_cap_sz,
                ap.psist_0_9,
                ap.msg_psist,
                ap.reg_psist,
            );
            self.emit(MsEvent::AccessParametersUpdated);
        }
    }

    pub fn originate(&mut self, service_option: u16, digits: String) {
        if self.state == MsProtocolState::Idle {
            let reason = AccessReason::Origination {
                service_option,
                digits,
            };
            self.service_option = Some(service_option);
            self.incoming_call = false;
            self.pending_access = Some(reason.clone());
            self.emit(MsEvent::AccessStarted {
                reason: reason.to_string(),
            });
            self.set_state(MsProtocolState::SystemAccess { reason });
        }
    }

    pub fn on_channel_assignment(&mut self, assignment: ChannelAssignment) -> bool {
        match &self.state {
            MsProtocolState::SystemAccess { reason } if reason.expects_assignment() => {}
            _ => return false,
        }
        self.emit(MsEvent::ChannelAssignment {
            walsh_code: assignment.walsh_code,
            frame_offset: assignment.frame_offset,
            for_rc: assignment.for_rc,
            rev_rc: assignment.rev_rc,
        });
        self.pending_access = None;
        self.access_response_deadline_chip = None;
        self.access_overhead_deadline_chip = None;
        self.common_channel_deadline_chip = None;
        self.pending_acks.clear();
        self.set_state(MsProtocolState::TrafficChannelInit { assignment });
        true
    }

    pub fn on_sms_cause_code(&self, error_class: u8) {
        self.emit(MsEvent::SmsCauseCode { error_class });
    }

    pub fn on_registration_needed(&mut self, reg_type: u8) {
        if self.state != MsProtocolState::Idle {
            return;
        }
        if self.reg.power_up_deadline_chip.is_some() {
            self.reg.deferred_registration = Some(reg_type);
            return;
        }
        self.emit(MsEvent::RegistrationNeeded { reg_type });
        let reason = AccessReason::Registration { reg_type };
        self.pending_access = Some(reason.clone());
        self.emit(MsEvent::AccessStarted {
            reason: reason.to_string(),
        });
        self.set_state(MsProtocolState::SystemAccess { reason });
    }

    pub fn force_registration(&mut self) {
        self.reg.power_up_deadline_chip = None;
        self.reg.deferred_registration = None;
        self.on_registration_needed(REG_TYPE_POWER_UP);
    }

    pub fn esn(&self) -> u32 {
        self.identity.esn
    }
}

/// Registration type codes (C.S0005-E Table 2.7.1.3.2.1-1 / REG_TYPE).
pub const REG_TYPE_TIMER: u8 = 0;
pub const REG_TYPE_POWER_UP: u8 = 1;
pub const REG_TYPE_ZONE: u8 = 2;
pub const REG_TYPE_POWER_DOWN: u8 = 3;
pub const REG_TYPE_PARAMETER: u8 = 4;
pub const REG_TYPE_ORDERED: u8 = 5;
pub const REG_TYPE_DISTANCE: u8 = 6;

/// Transport Layer MSG_TYPE of an SMS Cause Code, the network's
/// acknowledgment of a mobile-originated SMS (C.S0015-B §3.4.3.1).
const SMS_CAUSE_CODE_TL_TYPE: u8 = 0x02;

fn duration_chips(duration: Duration) -> u64 {
    let chips = duration.as_nanos() * u128::from(SR1_CHIP_RATE_HZ) / NANOS_PER_SECOND;
    u64::try_from(chips).unwrap_or(u64::MAX)
}

fn registration_period_chips(reg_prd: u8) -> Option<u64> {
    if reg_prd == 0 {
        return None;
    }
    let count = 2_f64.powf(f64::from(reg_prd) / 4.0).floor();
    Some((count * REGISTRATION_COUNT_CHIPS as f64).min(u64::MAX as f64) as u64)
}

fn parse_system_parameters(spm: &SystemParametersMessage) -> SystemParameters {
    SystemParameters {
        sid: spm.sid,
        nid: spm.nid,
        base_id: spm.base_id,
        reg_zone: spm.reg_zone,
        total_zones: spm.total_zones,
        power_up_reg: spm.power_up_reg,
        power_down_reg: spm.power_down_reg,
        parameter_reg: spm.parameter_reg,
        home_reg: spm.home_reg,
        for_sid_reg: spm.for_sid_reg,
        for_nid_reg: spm.for_nid_reg,
        reg_prd: spm.reg_prd,
        config_msg_seq: spm.config_msg_seq,
    }
}

fn parse_access_parameters(apm: &AccessParametersMessage) -> AccessParameters {
    AccessParameters {
        acc_msg_seq: apm.acc_msg_seq,
        nom_pwr: apm.nom_pwr,
        nom_pwr_ext: apm.nom_pwr_ext,
        init_pwr: apm.init_pwr,
        pwr_step: apm.pwr_step,
        num_step: apm.num_step,
        pam_sz: apm.pam_sz,
        max_cap_sz: apm.max_cap_sz,
        acc_chan: apm.acc_chan,
        probe_pn_ran: apm.probe_pn_ran,
        acc_tmo: apm.acc_tmo,
        probe_bkoff: apm.probe_bkoff,
        bkoff: apm.bkoff,
        max_req_seq: apm.max_req_seq,
        max_rsp_seq: apm.max_rsp_seq,
        psist_0_9: apm.psist_0_9,
        msg_psist: apm.msg_psist,
        reg_psist: apm.reg_psist,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn identity() -> MsAccessIdentity {
        MsAccessIdentity {
            esn: 0x1234_5678,
            imsi_s: 1234567890,
            ..Default::default()
        }
    }

    fn power_up_spm() -> SystemParameters {
        SystemParameters {
            sid: 42,
            nid: 7,
            base_id: 1,
            reg_zone: 0,
            total_zones: 1,
            power_up_reg: true,
            power_down_reg: false,
            parameter_reg: false,
            home_reg: true,
            for_sid_reg: true,
            for_nid_reg: true,
            reg_prd: 0,
            config_msg_seq: 0,
        }
    }

    #[test]
    fn imsi_addressed_page_starts_page_response() {
        use crate::tx::SERVICE_OPTION_SMS;
        use cdma_bts::receiver::layer3::PagingMessageHeader;

        let mut ms = MsCore::new(identity());
        ms.state = MsProtocolState::Idle;
        let (imsi_m_s1, imsi_m_s2) = imsi_s_from_imsi("1234567890").unwrap();
        ms.on_general_page(&GeneralPageMessage {
            header: PagingMessageHeader { pd: 0, msg_type: 0 },
            config_msg_seq: 0,
            acc_msg_seq: 0,
            class_0_done: true,
            class_1_done: true,
            tmsi_done: true,
            ordered_tmsis: false,
            broadcast_done: true,
            reserved: 0,
            add_length: 0,
            add_pfield: Vec::new(),
            page_records: vec![PageRecord::Class0 {
                page_subclass: 3,
                msg_seq: 5,
                imsi_s: Some((u64::from(imsi_m_s2) << 24) | u64::from(imsi_m_s1)),
                imsi_11_12: imsi_11_12_from_digits("00"),
                mcc: mcc_from_digits("310"),
                imsi_addr_num: None,
                imsi_m_s1: Some(imsi_m_s1),
                imsi_m_s2: Some(imsi_m_s2),
                special_service: true,
                service_option: Some(SERVICE_OPTION_SMS),
            }],
        });
        assert_eq!(
            ms.state,
            MsProtocolState::SystemAccess {
                reason: AccessReason::PageResponse {
                    service_option: SERVICE_OPTION_SMS,
                    ack_seq: 5,
                },
            }
        );
    }

    #[test]
    fn power_up_registration_waits_for_t57m() {
        let mut ms = MsCore::new(identity());
        ms.power_on();
        assert_eq!(*ms.state(), MsProtocolState::PilotAcquisition);
        ms.on_pilot_acquired(0);
        assert_eq!(*ms.state(), MsProtocolState::SyncAcquisition);
        ms.on_sync_decoded(SyncParameters::default());
        ms.begin_idle_timers(1_000);
        assert_eq!(*ms.state(), MsProtocolState::Idle);

        ms.apply_system_parameters_at(power_up_spm(), 1_000);
        assert_eq!(*ms.state(), MsProtocolState::Idle);
        let _ = ms.maintain_idle_timers(1_000 + T57M_CHIPS - 1);
        assert_eq!(*ms.state(), MsProtocolState::Idle);
        let _ = ms.maintain_idle_timers(1_000 + T57M_CHIPS);
        match ms.state() {
            MsProtocolState::SystemAccess {
                reason: AccessReason::Registration { reg_type },
            } => assert_eq!(*reg_type, REG_TYPE_POWER_UP),
            other => panic!("expected power-up registration access, got {other:?}"),
        }
        assert_eq!(ms.system_params().unwrap().sid, 42);

        let mut spm2 = power_up_spm();
        spm2.config_msg_seq = 1;
        ms.apply_system_parameters(spm2);
        assert!(matches!(ms.state(), MsProtocolState::SystemAccess { .. }));
    }

    #[test]
    fn disabled_power_up_timer_registers_on_system_parameters() {
        let mut ms = MsCore::new(identity()).with_power_up_delay(Duration::ZERO);
        ms.power_on();
        ms.on_pilot_acquired(0);
        ms.on_sync_decoded(SyncParameters::default());
        ms.begin_idle_timers(1_000);

        ms.apply_system_parameters_at(power_up_spm(), 1_000);
        assert_eq!(
            *ms.state(),
            MsProtocolState::SystemAccess {
                reason: AccessReason::Registration {
                    reg_type: REG_TYPE_POWER_UP
                }
            }
        );
    }

    #[test]
    fn no_registration_until_idle_and_overhead() {
        let mut ms = MsCore::new(identity());
        ms.apply_system_parameters(power_up_spm());
        assert_eq!(*ms.state(), MsProtocolState::PowerOff);
    }

    #[test]
    fn assignment_discards_pending_paging_ack() {
        let mut ms = MsCore::new(identity());
        ms.state = MsProtocolState::SystemAccess {
            reason: AccessReason::Origination {
                service_option: 6,
                digits: "5555".into(),
            },
        };
        ms.pending_acks.push_back(4);

        assert!(ms.on_channel_assignment(ChannelAssignment {
            walsh_code: 10,
            frame_offset: 0,
            for_rc: 3,
            rev_rc: 3,
            pilot_pn: 0,
        }));
        assert!(ms.take_pending_ack().is_none());
        assert!(ms.state().on_traffic());
    }

    #[test]
    fn status_response_carries_the_request_ack_without_an_ack_order() {
        use cdma_common::lac::message_types::MessageId;

        let mut ms = MsCore::new(identity());
        ms.state = MsProtocolState::SystemAccess {
            reason: AccessReason::Registration { reg_type: 1 },
        };
        ms.on_directed(
            &DirectedPdu {
                message_id: MessageId::StatusRequest,
                msg_type: 0x11,
                ack_seq: 0,
                msg_seq: 5,
                ack_req: true,
                valid_ack: true,
                body: DirectedBody::StatusRequest {
                    qual_info_type: 0,
                    qual_info: Vec::new(),
                    record_types: vec![0x0E, 0x1A, 0x17, 0x1B, 0x27],
                },
            },
            0,
        );

        assert!(matches!(
            ms.pending_access(),
            Some(AccessReason::StatusResponse { ack_seq: 5, .. })
        ));
        assert!(ms.take_pending_ack().is_none());
    }

    #[test]
    fn acknowledged_origination_waits_for_assignment_without_another_probe() {
        let mut ms = MsCore::new(identity());
        ms.state = MsProtocolState::SystemAccess {
            reason: AccessReason::Origination {
                service_option: 6,
                digits: "5555".into(),
            },
        };
        ms.pending_access = Some(AccessReason::Origination {
            service_option: 6,
            digits: "5555".into(),
        });

        ms.on_access_acknowledged(1_000);

        assert!(ms.registered());
        assert!(ms.pending_access().is_none());
        assert!(matches!(ms.state(), MsProtocolState::SystemAccess { .. }));
        ms.expire_access_response_wait(1_000 + T42M_CHIPS - 1);
        assert!(matches!(ms.state(), MsProtocolState::SystemAccess { .. }));
        ms.expire_access_response_wait(1_000 + T42M_CHIPS);
        assert_eq!(*ms.state(), MsProtocolState::Idle);
    }

    #[test]
    fn acknowledged_registration_waits_for_accepted_order() {
        let mut ms = MsCore::new(identity());
        ms.state = MsProtocolState::Idle;
        ms.apply_system_parameters(power_up_spm());
        ms.on_registration_needed(REG_TYPE_POWER_UP);

        ms.on_access_acknowledged(1_000);

        assert!(!ms.registered());
        assert!(ms.pending_access().is_none());
        assert!(matches!(ms.state(), MsProtocolState::SystemAccess { .. }));
        ms.on_directed_order(ORDER_REGISTRATION, 0, 2_000);
        assert!(ms.registered());
        assert_eq!(*ms.state(), MsProtocolState::Idle);
        assert!(ms.reg.power_up_reg_performed);
        assert!(ms.access_response_deadline_chip.is_none());
    }

    #[test]
    fn undecodable_paging_message_counts_as_crc_valid() {
        let mut ms = MsCore::new(identity());
        ms.on_paging_decode_failed(47, "unsupported body");
        assert_eq!(ms.paging_stats().crc_valid, 1);
        assert_eq!(ms.paging_stats().decode_failed(), 1);
        assert_eq!(ms.paging_stats().crc_failed, 0);
    }

    #[test]
    fn missing_registration_response_times_out_and_retries() {
        let mut ms = MsCore::new(identity());
        ms.state = MsProtocolState::Idle;
        ms.apply_system_parameters(power_up_spm());
        ms.on_registration_needed(REG_TYPE_POWER_UP);
        ms.on_access_acknowledged(1_000);

        let deadline = 1_000 + T42M_CHIPS;
        ms.expire_access_response_wait(deadline - 1);
        assert!(matches!(ms.state(), MsProtocolState::SystemAccess { .. }));
        ms.expire_access_response_wait(deadline);
        assert!(!ms.registered());
        assert_eq!(*ms.state(), MsProtocolState::Idle);

        ms.apply_system_parameters_at(
            power_up_spm(),
            deadline + REGISTRATION_RETRY_HOLDOFF_CHIPS - 1,
        );
        assert_eq!(*ms.state(), MsProtocolState::Idle);
        ms.apply_system_parameters_at(power_up_spm(), deadline + REGISTRATION_RETRY_HOLDOFF_CHIPS);
        ms.on_registration_needed(REG_TYPE_POWER_UP);
        assert!(matches!(ms.state(), MsProtocolState::SystemAccess { .. }));
    }

    #[test]
    fn timer_registration_uses_reg_prd_after_success() {
        let mut ms = MsCore::new(identity());
        ms.state = MsProtocolState::Idle;
        let mut sp = power_up_spm();
        sp.reg_prd = 4;
        ms.apply_system_parameters_at(sp, 10_000);
        ms.on_registration_needed(REG_TYPE_POWER_UP);
        ms.on_directed_order(ORDER_REGISTRATION, 0, 20_000);

        let period = 2 * REGISTRATION_COUNT_CHIPS;
        let _ = ms.maintain_idle_timers(20_000 + period - 1);
        assert_eq!(*ms.state(), MsProtocolState::Idle);
        let _ = ms.maintain_idle_timers(20_000 + period);
        assert!(matches!(
            ms.state(),
            MsProtocolState::SystemAccess {
                reason: AccessReason::Registration {
                    reg_type: REG_TYPE_TIMER
                }
            }
        ));
    }

    #[test]
    fn common_channel_supervision_expires_at_t30m() {
        let mut ms = MsCore::new(identity());
        ms.state = MsProtocolState::Idle;
        ms.begin_idle_timers(500);

        assert!(ms.maintain_idle_timers(500 + T30M_CHIPS - 1).is_none());
        assert_eq!(
            ms.maintain_idle_timers(500 + T30M_CHIPS),
            Some(MsTimerExpiry::CommonChannel)
        );
    }

    #[test]
    fn access_overhead_collection_expires_at_t41m() {
        let mut ms = MsCore::new(identity());
        ms.state = MsProtocolState::Idle;
        ms.originate(6, "5555".into());

        assert!(ms.maintain_idle_timers(1_000).is_none());
        assert!(ms.maintain_idle_timers(1_000 + T41M_CHIPS - 1).is_none());
        assert_eq!(
            ms.maintain_idle_timers(1_000 + T41M_CHIPS),
            Some(MsTimerExpiry::AccessOverhead)
        );
    }

    #[test]
    fn registration_request_qualifier_starts_ordered_registration() {
        let mut ms = MsCore::new(identity());
        ms.state = MsProtocolState::Idle;

        ms.on_directed_order(ORDER_REGISTRATION, 1, 1_000);

        assert!(matches!(
            ms.state(),
            MsProtocolState::SystemAccess {
                reason: AccessReason::Registration {
                    reg_type: REG_TYPE_ORDERED
                }
            }
        ));
        assert!(!ms.registered());
    }

    #[test]
    fn sms_cause_code_marks_the_transaction_complete() {
        let mut ms = MsCore::new(identity());
        let cause = cdma_common::sms::encode_sms_cause_code(1, 0);

        assert_eq!(ms.on_data_burst(BURST_TYPE_SMS, &cause), Some(0));
    }
}
