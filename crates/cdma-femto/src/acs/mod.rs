use crate::cell::FemtoCellState;
use crate::config::{
    AcsConfig, CpeCredential, CwmpFapValue, CwmpObject, CwmpValueSource, CwmpValueTransform,
    ProvisioningStage, TimezoneField,
};
use crate::config::{CwmpInformResponseStyle, FapImplementationKind};
use crate::fap::{FapImplementation, SharedFapRegistry, implementation};
use crate::sip::FapRouteHint;
use axum::Router;
use axum::body::Bytes;
use axum::extract::State;
use axum::http::{HeaderMap, StatusCode};
use axum::response::Response;
use axum::routing::any;
use cdma_cwmp::{
    AddObject, CwmpMessage, DeleteObject, GetParameterNames, GetParameterValues, InformResponse,
    ParameterValue, Reboot, SetParameterValues,
};
use hyper::server::conn::http1;
use hyper_util::rt::TokioIo;
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::IoSlice;
use std::net::SocketAddr;
use std::pin::Pin;
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};
use std::task::{Context, Poll};
use tokio::io::{AsyncRead, AsyncWrite, ReadBuf};
use tower::Service;

mod http;
mod server;
mod split;
mod transport;
mod values;

pub use server::AcsServer;
use split::split_request;
use values::{
    administrative_states, cwmp_timezone_value, cwmp_values_match, gps_degrees_minutes_to_decimal,
    gps_distance_meters, gps_latlon_to_iana, gps_longitude_to_timezone, is_sensitive_parameter,
    leaf_name,
};

const SESSION_COOKIE: &str = "1xbts-acs-session";
const GPS_REFERENCE_LATITUDE_SUFFIX: &str = "aGPSReferenceLatitude";
const GPS_REFERENCE_LONGITUDE_SUFFIX: &str = "aGPSReferenceLongitude";
const ADMIN_STATE_SUFFIX: &str = "AdminState";
const GPS_CHANGED_LOCATION_DISTANCE_PARAMETER: &str =
    "InternetGatewayDevice.X_0005B9_GPSStatus.ChangedLocationDistance";
const SOAP_ACTION_TITLE_CASE: &[u8] = b"Soapaction:";
const SOAP_ACTION_LEGACY_CASE: &[u8] = b"SOAPAction:";
/// Trailing bytes the Airvana LocationString carries after the SID and NID.
const LOCATION_STRING_TRAILER: &str = "000040";
const UTC_TIMEZONE: &str = "+00:00 UTC";
const DEGREES_PER_TIMEZONE_HOUR: f64 = 15.0;

const ACS_DIGEST_NONCE: &str = "1xbts-acs-nonce";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AcsPhase {
    AwaitingInform,
    AwaitingEmptyPost,
    CredentialQuerySent,
    ConfigurationQuerySent,
    ObjectProbeSent,
    ObjectCreationSent,
    ObjectDiscoverySent,
    ObjectDeletionSent,
    ReplacementCreationSent,
    ProvisioningSent,
    StatusQuerySent,
    DiscoveryQuerySent,
    ElementSettleQuerySent,
    Done,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BracketStep {
    ConfigAfterDown(usize),
    /// Polls remaining while awaiting the admin-downed element to leave operating state.
    AwaitElementDown(usize, u32),
    UpAfterConfig(usize),
    AdvanceAfterUp(usize),
    EndAfterRestore,
}

#[derive(Debug, Clone, PartialEq)]
pub enum AcsAction {
    Respond(Box<CwmpMessage>),
    EndSession,
}

pub struct AcsSession {
    implementation: Arc<dyn FapImplementation>,
    phase: AcsPhase,
    stage: usize,
    object: usize,
    discovery: usize,
    should_provision: bool,
    recovering_unlock: bool,
    configuration_verified: bool,
    pending_parameters: HashSet<String>,
    drifted_parameters: HashSet<String>,
    reapply_stages: HashSet<String>,
    created_objects: HashSet<String>,
    instance_remap: Vec<(String, String)>,
    derived_values: HashMap<String, String>,
    read_sip_password: bool,
    fap_context: Option<FapProvisioningContext>,
    a11_provisioning: Option<crate::config::A11Provisioning>,
    bracket: Option<BracketStep>,
    outbox: VecDeque<CwmpMessage>,
    gpv_accumulator: Vec<ParameterValue>,
    batch_queries: bool,
    feid: Option<String>,
}

fn log_outgoing(feid: Option<&str>, action: &AcsAction) {
    let AcsAction::Respond(msg) = action else {
        return;
    };
    let feid = feid.unwrap_or("?");
    match msg.as_ref() {
        CwmpMessage::SetParameterValues(request) => {
            if let [parameter] = request.parameter_list.as_slice() {
                log::info!(
                    "ACS: [{feid}] -> SetParameterValues {} = {}",
                    parameter.name,
                    if is_sensitive_parameter(&parameter.name) {
                        "<redacted>"
                    } else {
                        &parameter.value
                    }
                );
            }
        }
        CwmpMessage::GetParameterValues(request) => {
            if let [name] = request.parameter_names.as_slice() {
                log::info!("ACS: [{feid}] -> GetParameterValues {name}");
            }
        }
        _ => {}
    }
}

#[derive(Debug, Clone)]
struct FapProvisioningContext {
    feid: String,
    sid: u32,
    nid: u32,
    band_class: u32,
    cdma_channel: u32,
    max_sector_power_dbm: Option<i32>,
    sip_password: Option<String>,
    pn_offset: Option<u32>,
    onex_enabled: bool,
    evdo_enabled: bool,
    evdo_band_class: Option<u32>,
    evdo_channel: Option<u32>,
    evdo_cell_id: Option<u32>,
    evdo_sector_id: Option<String>,
    evdo_color_code: Option<u32>,
    evdo_tx_power: Option<i32>,
    evdo_pn_offset: Option<u32>,
    term_auth_psk: Option<String>,
    timezone: Option<String>,
}

const EVDO_SECTOR_ADMIN_UP: &str = "2";
const EVDO_SECTOR_ADMIN_DOWN: &str = "1";
const ONEX_ADMIN_UP: &str = "unlock";
const ONEX_ADMIN_DOWN: &str = "lock";

impl AcsSession {
    fn resolve_fap_context_value(&self, value: &CwmpFapValue) -> Option<String> {
        match value {
            CwmpFapValue::A11Spi => {
                return self.a11_provisioning.as_ref().map(|a| a.0.spi.to_string());
            }
            CwmpFapValue::A11SharedSecretHex => {
                return self
                    .a11_provisioning
                    .as_ref()
                    .map(|a| a.0.shared_secret_hex.clone());
            }
            _ => {}
        }
        let context = self.fap_context.as_ref()?;
        match value {
            CwmpFapValue::A11Spi | CwmpFapValue::A11SharedSecretHex => None,
            CwmpFapValue::Feid => Some(context.feid.clone()),
            CwmpFapValue::SiteSid => Some(context.sid.to_string()),
            CwmpFapValue::SiteNid => Some(context.nid.to_string()),
            CwmpFapValue::SiteBandClass => Some(context.band_class.to_string()),
            CwmpFapValue::SiteCdmaChannel => Some(context.cdma_channel.to_string()),
            CwmpFapValue::SiteMaxSectorPowerDbm => {
                context.max_sector_power_dbm.map(|dbm| dbm.to_string())
            }
            CwmpFapValue::SiteSipPassword => context.sip_password.clone(),
            CwmpFapValue::SiteLocationString => Some(format!(
                "{:04X}{:04X}{LOCATION_STRING_TRAILER}",
                context.sid as u16, context.nid as u16
            )),
            CwmpFapValue::SitePnOffset => context.pn_offset.map(|v| v.to_string()),
            CwmpFapValue::SiteEvdoBandClass => context.evdo_band_class.map(|v| v.to_string()),
            CwmpFapValue::SiteEvdoChannel => context.evdo_channel.map(|v| v.to_string()),
            CwmpFapValue::SiteEvdoCellId => context.evdo_cell_id.map(|v| v.to_string()),
            CwmpFapValue::SiteEvdoSectorId => context.evdo_sector_id.clone(),
            CwmpFapValue::SiteEvdoColorCode => context.evdo_color_code.map(|v| v.to_string()),
            CwmpFapValue::SiteEvdoTxPower => context.evdo_tx_power.map(|v| v.to_string()),
            CwmpFapValue::SiteEvdoPnOffset => context.evdo_pn_offset.map(|v| v.to_string()),
            CwmpFapValue::SiteEvdoAdminStatus => Some(
                if context.evdo_enabled {
                    EVDO_SECTOR_ADMIN_UP
                } else {
                    EVDO_SECTOR_ADMIN_DOWN
                }
                .to_string(),
            ),
            CwmpFapValue::TermAuthPsk => context.term_auth_psk.clone(),
            CwmpFapValue::SiteOneXAdminState => Some(
                if context.onex_enabled {
                    ONEX_ADMIN_UP
                } else {
                    ONEX_ADMIN_DOWN
                }
                .to_string(),
            ),
        }
    }

    fn resolve_timezone_field(
        &self,
        latitude: &str,
        longitude: &str,
        field: TimezoneField,
        readback: &[ParameterValue],
    ) -> Option<String> {
        let zone_name = self.resolve_timezone_name(latitude, longitude, readback)?;
        let value = cwmp_timezone_value(&zone_name, field);
        if value.is_none() {
            log::warn!(
                "ACS: [{}] timezone {zone_name:?} yields no value for {field:?}",
                self.feid_tag()
            );
        }
        value
    }

    fn resolve_timezone_name(
        &self,
        latitude: &str,
        longitude: &str,
        readback: &[ParameterValue],
    ) -> Option<String> {
        if let Some(override_zone) = self
            .fap_context
            .as_ref()
            .and_then(|context| context.timezone.clone())
        {
            return Some(override_zone);
        }
        let reading = |name: &str| {
            readback
                .iter()
                .find(|value| value.name == name)
                .map(|value| value.value.clone())
        };
        let latitude_reading = reading(latitude)?;
        let longitude_reading = reading(longitude)?;
        let name = gps_latlon_to_iana(&latitude_reading, &longitude_reading);
        if name.is_none() {
            log::warn!(
                "ACS: [{}] GPS {latitude_reading},{longitude_reading} did not resolve to a timezone",
                self.feid_tag()
            );
        }
        name
    }

    fn derive_values(&mut self, stages: &[ProvisioningStage], readback: &[ParameterValue]) {
        self.derived_values.clear();
        for parameter in stages.iter().flat_map(|stage| &stage.params) {
            let Some(source) = &parameter.value_from else {
                continue;
            };
            let (derived, source_description) = match source {
                CwmpValueSource::CpeParameter {
                    parameter: source_parameter,
                    transform,
                } => {
                    let reading = readback
                        .iter()
                        .find(|value| value.name == *source_parameter)
                        .map(|value| value.value.as_str());
                    let derived = match transform {
                        CwmpValueTransform::GpsDegreesMinutesToDecimalDegrees => {
                            reading.and_then(gps_degrees_minutes_to_decimal)
                        }
                        CwmpValueTransform::GpsLongitudeToTimezone => Some(
                            reading
                                .and_then(gps_longitude_to_timezone)
                                .unwrap_or_else(|| UTC_TIMEZONE.to_string()),
                        ),
                    };
                    (derived, "current CPE reading")
                }
                CwmpValueSource::FapContext { context: value } => (
                    self.resolve_fap_context_value(value),
                    "per-FAP provisioning context",
                ),
                CwmpValueSource::Timezone {
                    latitude,
                    longitude,
                    field,
                } => (
                    self.resolve_timezone_field(latitude, longitude, *field, readback),
                    "GPS position or site timezone override",
                ),
            };
            match derived {
                Some(value) => {
                    log::info!(
                        "ACS: [{}] derived {} from {source_description}",
                        self.feid_tag(),
                        parameter.name
                    );
                    self.derived_values.insert(parameter.name.clone(), value);
                }
                None => log::warn!(
                    "ACS: [{}] cannot derive {} from its configured source",
                    self.feid_tag(),
                    parameter.name
                ),
            }
        }
        self.reconcile_gps_reference(readback);
    }

    fn reconcile_gps_reference(&mut self, readback: &[ParameterValue]) {
        let Some(latitude_name) = self
            .derived_values
            .keys()
            .find(|name| name.ends_with(GPS_REFERENCE_LATITUDE_SUFFIX))
            .cloned()
        else {
            return;
        };
        let Some(longitude_name) = self
            .derived_values
            .keys()
            .find(|name| name.ends_with(GPS_REFERENCE_LONGITUDE_SUFFIX))
            .cloned()
        else {
            return;
        };
        let values = || {
            Some((
                self.derived_values.get(&latitude_name)?.parse().ok()?,
                self.derived_values.get(&longitude_name)?.parse().ok()?,
                readback
                    .iter()
                    .find(|value| value.name == latitude_name)?
                    .value
                    .parse()
                    .ok()?,
                readback
                    .iter()
                    .find(|value| value.name == longitude_name)?
                    .value
                    .parse()
                    .ok()?,
            ))
        };
        let Some((latitude, longitude, reference_latitude, reference_longitude)) = values() else {
            return;
        };
        let Some(reconcile_distance) = readback
            .iter()
            .find(|value| value.name == GPS_CHANGED_LOCATION_DISTANCE_PARAMETER)
            .and_then(|value| value.value.parse::<f64>().ok())
            .filter(|distance| distance.is_finite() && *distance >= 0.0)
        else {
            log::warn!(
                "ACS: [{}] cannot reconcile GPS reference without the CPE distance threshold",
                self.feid_tag()
            );
            return;
        };
        let distance =
            gps_distance_meters(latitude, longitude, reference_latitude, reference_longitude);
        if distance > reconcile_distance {
            return;
        }
        let reference_latitude = readback
            .iter()
            .find(|value| value.name == latitude_name)
            .expect("validated GPS reference latitude");
        let reference_longitude = readback
            .iter()
            .find(|value| value.name == longitude_name)
            .expect("validated GPS reference longitude");
        self.derived_values
            .insert(latitude_name, reference_latitude.value.clone());
        self.derived_values
            .insert(longitude_name, reference_longitude.value.clone());
        log::info!(
            "ACS: [{}] retained GPS reference within CPE reconcile distance ({distance:.1} m <= {reconcile_distance:.1} m)",
            self.feid_tag()
        );
    }

    fn is_transition_stage(label: &str) -> bool {
        let label = label.to_ascii_lowercase();
        label.starts_with("lock") || label.starts_with("unlock")
    }

    fn is_unlock_stage(label: &str) -> bool {
        label.to_ascii_lowercase().starts_with("unlock")
    }

    pub fn new() -> Self {
        Self {
            implementation: implementation(FapImplementationKind::StandardXs0059),
            phase: AcsPhase::AwaitingInform,
            stage: 0,
            object: 0,
            discovery: 0,
            should_provision: false,
            recovering_unlock: false,
            configuration_verified: false,
            pending_parameters: HashSet::new(),
            drifted_parameters: HashSet::new(),
            reapply_stages: HashSet::new(),
            created_objects: HashSet::new(),
            instance_remap: Vec::new(),
            derived_values: HashMap::new(),
            read_sip_password: false,
            fap_context: None,
            a11_provisioning: None,
            bracket: None,
            outbox: VecDeque::new(),
            gpv_accumulator: Vec::new(),
            batch_queries: true,
            feid: None,
        }
    }

    fn set_implementation(&mut self, implementation: Arc<dyn FapImplementation>) {
        self.implementation = implementation;
    }

    fn set_batch_queries(&mut self, batch_queries: bool) {
        self.batch_queries = batch_queries;
    }

    fn set_feid(&mut self, feid: Option<String>) {
        self.feid = feid;
    }

    fn feid_tag(&self) -> &str {
        self.feid.as_deref().unwrap_or("?")
    }

    fn set_read_sip_password(&mut self, enabled: bool) {
        self.read_sip_password = enabled;
    }

    fn set_fap_context(
        &mut self,
        definition: &crate::fap::FapRuntimeDefinition,
        term_auth_psk: Option<String>,
    ) {
        let site = &definition.site;
        self.fap_context = Some(FapProvisioningContext {
            feid: definition.feid.clone(),
            sid: site.sid,
            nid: site.nid,
            band_class: site.band_class,
            cdma_channel: site.cdma_channel,
            max_sector_power_dbm: site.max_sector_power_dbm,
            sip_password: site.sip_password.clone(),
            pn_offset: site.pn_offset,
            onex_enabled: site.onex_enabled.unwrap_or(true),
            evdo_enabled: site.evdo_enabled,
            evdo_band_class: site.evdo_band_class,
            evdo_channel: site.evdo_channel,
            evdo_cell_id: site.evdo_cell_id,
            evdo_sector_id: site.evdo_sector_id.clone(),
            evdo_color_code: site.evdo_color_code,
            evdo_tx_power: site.evdo_tx_power,
            evdo_pn_offset: site.evdo_pn_offset,
            term_auth_psk,
            timezone: site.timezone.clone(),
        });
    }

    fn set_reapply_stages(&mut self, stages: &[String]) {
        self.reapply_stages = stages.iter().cloned().collect();
    }

    pub fn phase(&self) -> AcsPhase {
        self.phase
    }

    pub fn configuration_verified(&self) -> bool {
        self.configuration_verified
    }

    fn send_stage(&mut self, stages: &[ProvisioningStage], idx: usize) -> Option<AcsAction> {
        let stage = &stages[idx];
        if let Some(object) = &stage.replace_object {
            if self.created_objects.contains(&object.name) {
                log::info!(
                    "ACS: [{}] stage {} skipped because {} was created in this session",
                    self.feid_tag(),
                    stage.label,
                    object.name
                );
                return None;
            }
            let instance = object.required_instance?;
            let instance_prefix = format!("{}{instance}.", object.name);
            if self
                .drifted_parameters
                .iter()
                .any(|name| name.starts_with(&instance_prefix))
            {
                log::info!(
                    "ACS: [{}] stage {} -> DeleteObject {instance_prefix}",
                    self.feid_tag(),
                    stage.label
                );
                self.stage = idx;
                self.phase = AcsPhase::ObjectDeletionSent;
                return Some(AcsAction::Respond(Box::new(CwmpMessage::DeleteObject(
                    DeleteObject {
                        object_name: instance_prefix,
                        parameter_key: "1xbts-femto".to_string(),
                    },
                ))));
            }
            return None;
        }
        let parameters = self.stage_send_parameters(stages, idx);
        if parameters.is_empty() {
            return None;
        }
        let label = Self::stage_label(stage, idx);
        if let Some(bracket) = &stage.bracket_admin {
            if parameters.iter().any(|p| p.name != bracket.parameter) {
                log::info!(
                    "ACS: [{}] stage {label} -> admin down ({}={})",
                    self.feid_tag(),
                    leaf_name(&bracket.parameter),
                    bracket.down_value
                );
                self.stage = idx;
                self.phase = AcsPhase::ProvisioningSent;
                self.bracket = Some(BracketStep::ConfigAfterDown(idx));
                return Some(self.set_admin_action(bracket, &bracket.down_value));
            }
        }
        self.log_stage_send(&label, &parameters);
        self.stage = idx;
        self.phase = AcsPhase::ProvisioningSent;
        Some(self.set_parameter_values_action(&parameters))
    }

    fn stage_send_parameters<'a>(
        &self,
        stages: &'a [ProvisioningStage],
        idx: usize,
    ) -> Vec<&'a crate::config::CwmpParam> {
        let stage = &stages[idx];
        let transition_stage = Self::is_transition_stage(&stage.label);
        self.implementation
            .provisioning()
            .stage_parameters(stage, &self.pending_parameters, transition_stage)
            .into_iter()
            .filter(|parameter| {
                parameter.value_from.is_none() || self.derived_values.contains_key(&parameter.name)
            })
            .collect()
    }

    fn stage_label(stage: &ProvisioningStage, idx: usize) -> String {
        if stage.label.is_empty() {
            format!("#{idx}")
        } else {
            stage.label.clone()
        }
    }

    fn log_stage_send(&self, label: &str, parameters: &[&crate::config::CwmpParam]) {
        let summary = parameters
            .iter()
            .map(|p| leaf_name(&p.name).to_string())
            .collect::<Vec<_>>()
            .join(", ");
        log::info!(
            "ACS: [{}] stage {label} -> SetParameterValues ({} param(s)): {summary}",
            self.feid_tag(),
            parameters.len()
        );
    }

    fn set_parameter_values_action(&self, parameters: &[&crate::config::CwmpParam]) -> AcsAction {
        AcsAction::Respond(Box::new(CwmpMessage::SetParameterValues(
            SetParameterValues {
                parameter_list: parameters
                    .iter()
                    .map(|p| ParameterValue {
                        name: self.remap_name(&p.name),
                        value: if p.value_from.is_some() {
                            self.derived_values
                                .get(&p.name)
                                .cloned()
                                .unwrap_or_else(|| p.value.clone())
                        } else {
                            p.value.clone()
                        },
                        xsi_type: p.xsd_type.clone(),
                    })
                    .collect(),
                parameter_key: "1xbts-femto".to_string(),
            },
        )))
    }

    fn set_admin_action(&self, bracket: &crate::config::BracketAdmin, value: &str) -> AcsAction {
        AcsAction::Respond(Box::new(CwmpMessage::SetParameterValues(
            SetParameterValues {
                parameter_list: vec![ParameterValue {
                    name: bracket.parameter.clone(),
                    value: value.to_string(),
                    xsi_type: bracket.xsd_type.clone(),
                }],
                parameter_key: "1xbts-femto".to_string(),
            },
        )))
    }

    fn advance_bracket(
        &mut self,
        step: BracketStep,
        stages: &[ProvisioningStage],
        discovery_query: &[CwmpObject],
        status_query: &[String],
    ) -> AcsAction {
        match step {
            BracketStep::ConfigAfterDown(idx) => {
                if let Some(settle) = stages[idx]
                    .bracket_admin
                    .as_ref()
                    .and_then(|b| b.settle_parameter.clone())
                {
                    let polls = stages[idx]
                        .bracket_admin
                        .as_ref()
                        .map(|b| b.settle_polls)
                        .unwrap_or(0);
                    return self.poll_element_settle(idx, settle, polls);
                }
                self.send_bracket_config(stages, idx)
            }
            BracketStep::AwaitElementDown(idx, _) => self.send_bracket_config(stages, idx),
            BracketStep::UpAfterConfig(idx) => {
                self.send_bracket_up(stages, idx, BracketStep::AdvanceAfterUp(idx))
            }
            BracketStep::AdvanceAfterUp(idx) => {
                self.bracket = None;
                if idx + 1 < stages.len() {
                    self.send_next_stage(stages, idx + 1, discovery_query, status_query)
                } else {
                    self.finish_or_read(discovery_query, status_query)
                }
            }
            BracketStep::EndAfterRestore => {
                self.bracket = None;
                self.finish_or_read(discovery_query, status_query)
            }
        }
    }

    fn poll_element_settle(&mut self, idx: usize, settle: String, polls: u32) -> AcsAction {
        log::info!(
            "ACS: [{}] polling {} for element admin-down ({} poll(s) left)",
            self.feid_tag(),
            leaf_name(&settle),
            polls
        );
        self.stage = idx;
        self.phase = AcsPhase::ElementSettleQuerySent;
        self.bracket = Some(BracketStep::AwaitElementDown(idx, polls));
        AcsAction::Respond(Box::new(CwmpMessage::GetParameterValues(
            GetParameterValues {
                parameter_names: vec![settle],
            },
        )))
    }

    fn send_bracket_config(&mut self, stages: &[ProvisioningStage], idx: usize) -> AcsAction {
        let bracket_parameter = stages[idx]
            .bracket_admin
            .as_ref()
            .map(|bracket| bracket.parameter.clone());
        let parameters = self
            .stage_send_parameters(stages, idx)
            .into_iter()
            .filter(|p| Some(&p.name) != bracket_parameter.as_ref())
            .collect::<Vec<_>>();
        if parameters.is_empty() {
            return self.send_bracket_up(stages, idx, BracketStep::AdvanceAfterUp(idx));
        }
        let label = Self::stage_label(&stages[idx], idx);
        self.log_stage_send(&label, &parameters);
        self.stage = idx;
        self.phase = AcsPhase::ProvisioningSent;
        self.bracket = Some(BracketStep::UpAfterConfig(idx));
        self.set_parameter_values_action(&parameters)
    }

    fn send_bracket_up(
        &mut self,
        stages: &[ProvisioningStage],
        idx: usize,
        next: BracketStep,
    ) -> AcsAction {
        let stage = &stages[idx];
        let bracket = stage
            .bracket_admin
            .as_ref()
            .expect("bracketed stage has bracket_admin");
        // Restore the configured value so disabled sectors stay disabled.
        let desired = stage
            .params
            .iter()
            .find(|parameter| parameter.name == bracket.parameter)
            .and_then(|parameter| parameter.value_from.as_ref())
            .and_then(|source| match source {
                crate::config::CwmpValueSource::FapContext { context } => {
                    self.resolve_fap_context_value(context)
                }
                crate::config::CwmpValueSource::CpeParameter { .. }
                | crate::config::CwmpValueSource::Timezone { .. } => None,
            });
        let restore_value = desired.as_deref().unwrap_or(bracket.up_value.as_str());
        let label = Self::stage_label(stage, idx);
        let direction = if restore_value == bracket.down_value {
            "down"
        } else {
            "up"
        };
        log::info!(
            "ACS: [{}] stage {label} -> admin {direction} ({}={restore_value})",
            self.feid_tag(),
            leaf_name(&bracket.parameter),
        );
        self.stage = idx;
        self.phase = AcsPhase::ProvisioningSent;
        self.bracket = Some(next);
        self.set_admin_action(bracket, restore_value)
    }

    fn send_next_stage(
        &mut self,
        stages: &[ProvisioningStage],
        start: usize,
        discovery_query: &[CwmpObject],
        status_query: &[String],
    ) -> AcsAction {
        for idx in start..stages.len() {
            if let Some(action) = self.send_stage(stages, idx) {
                return action;
            }
        }
        self.finish_or_read(discovery_query, status_query)
    }

    fn send_next_unlock_stage(
        &mut self,
        stages: &[ProvisioningStage],
        start: usize,
        discovery_query: &[CwmpObject],
        status_query: &[String],
    ) -> AcsAction {
        if let Some((idx, _)) = stages
            .iter()
            .enumerate()
            .skip(start)
            .find(|(_, stage)| Self::is_unlock_stage(&stage.label))
        {
            return self
                .send_stage(stages, idx)
                .unwrap_or_else(|| self.finish_or_read(discovery_query, status_query));
        }
        self.finish_or_read(discovery_query, status_query)
    }

    fn send_object(&mut self, objects: &[CwmpObject], idx: usize) -> AcsAction {
        let object = &objects[idx];
        log::info!(
            "ACS: [{}] ensuring CWMP object {}",
            self.feid_tag(),
            object.name
        );
        self.object = idx;
        self.phase = AcsPhase::ObjectCreationSent;
        AcsAction::Respond(Box::new(CwmpMessage::AddObject(AddObject {
            object_name: object.name.clone(),
            parameter_key: "1xbts-femto".to_string(),
        })))
    }

    fn probe_object(&mut self, objects: &[CwmpObject], idx: usize) -> AcsAction {
        let object = &objects[idx];
        self.object = idx;
        self.phase = AcsPhase::ObjectProbeSent;
        // Some legacy firmware faults GetParameterNames on an existing instance, so
        // a name probe would recreate and leak it every session.
        if let (Some(instance), Some(leaf)) = (
            object.required_instance,
            object.existence_probe_leaf.as_deref(),
        ) {
            let name = format!("{}{instance}.{leaf}", object.name);
            log::info!(
                "ACS: [{}] probing instance {name} to decide whether to AddObject",
                self.feid_tag()
            );
            return AcsAction::Respond(Box::new(CwmpMessage::GetParameterValues(
                GetParameterValues {
                    parameter_names: vec![name],
                },
            )));
        }
        log::info!(
            "ACS: [{}] checking CWMP object {}",
            self.feid_tag(),
            object.name
        );
        AcsAction::Respond(Box::new(CwmpMessage::GetParameterNames(
            GetParameterNames {
                parameter_path: object.name.clone(),
                next_level: object.next_level,
            },
        )))
    }

    fn queries_parameter(&self, parameter: &crate::config::CwmpParam) -> bool {
        match &parameter.value_from {
            Some(CwmpValueSource::FapContext { context }) => {
                self.resolve_fap_context_value(context).is_some()
            }
            _ => true,
        }
    }

    fn remap_name(&self, name: &str) -> String {
        for (from, to) in &self.instance_remap {
            if let Some(rest) = name.strip_prefix(from.as_str()) {
                return format!("{to}{rest}");
            }
        }
        name.to_string()
    }

    fn settled_parameters(&self, stages: &[ProvisioningStage]) -> Vec<ParameterValue> {
        let mut parameters = Vec::<ParameterValue>::new();
        let mut indexes = HashMap::<&str, usize>::new();
        for parameter in stages
            .iter()
            .flat_map(|stage| &stage.params)
            .filter(|parameter| parameter.verify && !parameter.name.ends_with("SecurityKey"))
            .filter(|parameter| {
                parameter.value_from.is_none() || self.derived_values.contains_key(&parameter.name)
            })
        {
            let value = ParameterValue {
                name: self.remap_name(&parameter.name),
                value: self
                    .derived_values
                    .get(&parameter.name)
                    .cloned()
                    .unwrap_or_else(|| parameter.value.clone()),
                xsi_type: parameter.xsd_type.clone(),
            };
            if let Some(index) = indexes.get(parameter.name.as_str()).copied() {
                parameters[index] = value;
            } else {
                indexes.insert(parameter.name.as_str(), parameters.len());
                parameters.push(value);
            }
        }
        parameters
    }

    fn begin_verification(
        &mut self,
        objects: &[CwmpObject],
        stages: &[ProvisioningStage],
        discovery_query: &[CwmpObject],
        status_query: &[String],
    ) -> AcsAction {
        if !objects.is_empty() {
            self.probe_object(objects, 0)
        } else if let Some(action) = self.query_configuration(stages) {
            action
        } else {
            self.finish_or_read(discovery_query, status_query)
        }
    }

    fn query_configuration(&mut self, stages: &[ProvisioningStage]) -> Option<AcsAction> {
        let parameters = self.settled_parameters(stages);
        let parameter_names = parameters
            .into_iter()
            .map(|parameter| parameter.name)
            .chain(
                stages
                    .iter()
                    .flat_map(|stage| &stage.params)
                    .filter_map(|parameter| parameter.value_from.as_ref())
                    .flat_map(|source| source.cpe_parameters())
                    .map(str::to_string),
            )
            .chain(
                stages
                    .iter()
                    .flat_map(|stage| &stage.params)
                    .filter(|parameter| {
                        parameter.verify
                            && parameter.value_from.is_some()
                            && self.queries_parameter(parameter)
                    })
                    .map(|parameter| parameter.name.clone()),
            )
            .chain(
                stages
                    .iter()
                    .any(|stage| {
                        stage.params.iter().any(|parameter| {
                            parameter.name.ends_with(GPS_REFERENCE_LATITUDE_SUFFIX)
                                || parameter.name.ends_with(GPS_REFERENCE_LONGITUDE_SUFFIX)
                        })
                    })
                    .then(|| GPS_CHANGED_LOCATION_DISTANCE_PARAMETER.to_string()),
            )
            .collect::<HashSet<_>>()
            .into_iter()
            .collect::<Vec<_>>();
        if parameter_names.is_empty() {
            return None;
        }
        log::info!(
            "ACS: [{}] reading {} configured parameter(s) before provisioning",
            self.feid_tag(),
            parameter_names.len()
        );
        self.configuration_verified = false;
        self.phase = AcsPhase::ConfigurationQuerySent;
        Some(AcsAction::Respond(Box::new(
            CwmpMessage::GetParameterValues(GetParameterValues { parameter_names }),
        )))
    }

    fn continue_after_object(
        &mut self,
        objects: &[CwmpObject],
        stages: &[ProvisioningStage],
        discovery_query: &[CwmpObject],
        status_query: &[String],
    ) -> AcsAction {
        if self.object + 1 < objects.len() {
            self.probe_object(objects, self.object + 1)
        } else {
            self.query_configuration(stages)
                .unwrap_or_else(|| self.finish_or_read(discovery_query, status_query))
        }
    }

    fn discover_object(&mut self, objects: &[CwmpObject]) -> AcsAction {
        self.phase = AcsPhase::ObjectDiscoverySent;
        AcsAction::Respond(Box::new(CwmpMessage::GetParameterNames(
            GetParameterNames {
                parameter_path: objects[self.object].name.clone(),
                next_level: false,
            },
        )))
    }

    fn finish_or_read(
        &mut self,
        discovery_query: &[CwmpObject],
        status_query: &[String],
    ) -> AcsAction {
        if let Some(query) = discovery_query.get(self.discovery) {
            let scope = if query.next_level {
                "immediate children of"
            } else {
                "descendants of"
            };
            log::info!(
                "ACS: [{}] enumerating {scope} CWMP object {}",
                self.feid_tag(),
                query.name
            );
            self.phase = AcsPhase::DiscoveryQuerySent;
            AcsAction::Respond(Box::new(CwmpMessage::GetParameterNames(
                GetParameterNames {
                    parameter_path: query.name.clone(),
                    next_level: query.next_level,
                },
            )))
        } else if status_query.is_empty() {
            self.phase = AcsPhase::Done;
            AcsAction::EndSession
        } else {
            let parameter_names = status_query
                .iter()
                .cloned()
                .collect::<HashSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            log::info!(
                "ACS: [{}] reading back {} status param(s) (GPS/oper state)",
                self.feid_tag(),
                parameter_names.len()
            );
            self.phase = AcsPhase::StatusQuerySent;
            AcsAction::Respond(Box::new(CwmpMessage::GetParameterValues(
                GetParameterValues { parameter_names },
            )))
        }
    }

    pub fn step(
        &mut self,
        msg: &CwmpMessage,
        stages: &[ProvisioningStage],
        status_query: &[String],
        reboot: bool,
        service_enabled: bool,
    ) -> AcsAction {
        self.step_with_objects(msg, &[], stages, &[], status_query, reboot, service_enabled)
    }

    pub fn step_with_objects(
        &mut self,
        msg: &CwmpMessage,
        objects: &[CwmpObject],
        stages: &[ProvisioningStage],
        discovery_query: &[CwmpObject],
        status_query: &[String],
        reboot: bool,
        service_enabled: bool,
    ) -> AcsAction {
        if !self.outbox.is_empty() {
            if matches!(msg, CwmpMessage::Fault(_)) {
                self.outbox.clear();
                self.gpv_accumulator.clear();
            } else {
                if let CwmpMessage::GetParameterValuesResponse(response) = msg {
                    self.gpv_accumulator
                        .extend(response.parameters.iter().cloned());
                }
                if let Some(next) = self.outbox.pop_front() {
                    return AcsAction::Respond(Box::new(next));
                }
            }
        }

        let merged;
        let msg = if self.gpv_accumulator.is_empty() {
            msg
        } else if let CwmpMessage::GetParameterValuesResponse(response) = msg {
            let mut parameters = std::mem::take(&mut self.gpv_accumulator);
            parameters.extend(response.parameters.iter().cloned());
            merged =
                CwmpMessage::GetParameterValuesResponse(cdma_cwmp::GetParameterValuesResponse {
                    parameters,
                });
            &merged
        } else {
            self.gpv_accumulator.clear();
            msg
        };

        let action = self.advance(
            msg,
            objects,
            stages,
            discovery_query,
            status_query,
            reboot,
            service_enabled,
        );

        if let AcsAction::Respond(message) = &action {
            let chunks = split_request((**message).clone(), self.batch_queries);
            if chunks.len() > 1 {
                self.outbox.clear();
                self.gpv_accumulator.clear();
                let mut chunks = chunks.into_iter();
                let first = chunks.next().expect("split yields at least one chunk");
                self.outbox.extend(chunks);
                log::info!(
                    "ACS: [{}] request split into {} single-parameter message(s)",
                    self.feid_tag(),
                    self.outbox.len() + 1
                );
                return AcsAction::Respond(Box::new(first));
            }
        }
        action
    }

    #[allow(clippy::too_many_arguments)]
    fn advance(
        &mut self,
        msg: &CwmpMessage,
        objects: &[CwmpObject],
        stages: &[ProvisioningStage],
        discovery_query: &[CwmpObject],
        status_query: &[String],
        reboot: bool,
        service_enabled: bool,
    ) -> AcsAction {
        match (self.phase, msg) {
            (AcsPhase::AwaitingInform, CwmpMessage::Inform(inform)) => {
                let events = inform
                    .events
                    .iter()
                    .map(|e| e.event_code.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                log::info!(
                    "ACS: [{}] Inform from {} [{}] -> InformResponse",
                    self.feid_tag(),
                    inform.device_id.serial_number,
                    events
                );
                let changed = inform
                    .parameter_list
                    .iter()
                    .filter(|parameter| !is_sensitive_parameter(&parameter.name))
                    .map(|parameter| {
                        let leaf = parameter.name.rsplit('.').next().unwrap_or(&parameter.name);
                        format!("{leaf}={}", parameter.value)
                    })
                    .collect::<Vec<_>>();
                if !changed.is_empty() {
                    log::info!(
                        "ACS: [{}] Inform parameters from {}: {}",
                        self.feid_tag(),
                        inform.device_id.serial_number,
                        changed.join(", ")
                    );
                }
                self.should_provision = true;
                self.phase = AcsPhase::AwaitingEmptyPost;
                AcsAction::Respond(Box::new(CwmpMessage::InformResponse(InformResponse {
                    max_envelopes: 1,
                })))
            }
            (AcsPhase::AwaitingEmptyPost, CwmpMessage::Empty) if reboot => {
                log::warn!(
                    "ACS: [{}] issuing one-shot Reboot to the CPE",
                    self.feid_tag()
                );
                self.phase = AcsPhase::Done;
                AcsAction::Respond(Box::new(CwmpMessage::Reboot(Reboot {
                    command_key: "1xbts-reboot".to_string(),
                })))
            }
            (AcsPhase::AwaitingEmptyPost, CwmpMessage::Empty) => {
                if self.read_sip_password
                    && let Some(parameter) =
                        self.implementation.provisioning().sip_password_parameter()
                {
                    self.phase = AcsPhase::CredentialQuerySent;
                    AcsAction::Respond(Box::new(CwmpMessage::GetParameterValues(
                        GetParameterValues {
                            parameter_names: vec![parameter.to_string()],
                        },
                    )))
                } else if service_enabled && self.should_provision {
                    self.begin_verification(objects, stages, discovery_query, status_query)
                } else {
                    log::info!(
                        "ACS: [{}] nothing to provision (service_enabled={service_enabled}, \
                         requested={}, {} stage(s)) -> EndSession",
                        self.feid_tag(),
                        self.should_provision,
                        stages.len()
                    );
                    self.finish_or_read(discovery_query, status_query)
                }
            }
            (AcsPhase::CredentialQuerySent, CwmpMessage::GetParameterValuesResponse(_)) => {
                if service_enabled && self.should_provision {
                    self.begin_verification(objects, stages, discovery_query, status_query)
                } else {
                    self.finish_or_read(discovery_query, status_query)
                }
            }
            (AcsPhase::ConfigurationQuerySent, CwmpMessage::GetParameterValuesResponse(resp)) => {
                self.bracket = None;
                if self.a11_provisioning.is_none()
                    && stages.iter().flat_map(|s| &s.params).any(|p| {
                        matches!(
                            p.value_from,
                            Some(CwmpValueSource::FapContext {
                                context: CwmpFapValue::A11Spi | CwmpFapValue::A11SharedSecretHex
                            })
                        )
                    })
                {
                    log::error!(
                        "ACS: [{}] A11 provisioning credentials are missing",
                        self.feid_tag()
                    );
                    self.configuration_verified = false;
                    self.phase = AcsPhase::Done;
                    return AcsAction::EndSession;
                }
                self.derive_values(stages, &resp.parameters);
                let desired = self.settled_parameters(stages);
                let reapplied = stages
                    .iter()
                    .filter(|stage| {
                        self.reapply_stages.contains(&stage.label)
                            || stage.params.iter().any(|p| {
                                matches!(
                                    p.value_from,
                                    Some(CwmpValueSource::FapContext {
                                        context: CwmpFapValue::A11SharedSecretHex
                                    })
                                )
                            })
                    })
                    .flat_map(|stage| stage.params.iter().map(|parameter| parameter.name.clone()))
                    .collect::<HashSet<_>>();
                let matches = desired.iter().all(|expected| {
                    resp.parameters.iter().any(|actual| {
                        actual.name == expected.name
                            && cwmp_values_match(&expected.name, &expected.value, &actual.value)
                    })
                });
                let mismatched = desired
                    .iter()
                    .filter(|expected| {
                        !resp.parameters.iter().any(|actual| {
                            actual.name == expected.name
                                && cwmp_values_match(&expected.name, &expected.value, &actual.value)
                        })
                    })
                    .map(|parameter| parameter.name.clone())
                    .collect::<Vec<_>>();
                self.drifted_parameters = mismatched.iter().cloned().collect();
                let transient_admin = mismatched
                    .iter()
                    .filter(|name| {
                        desired
                            .iter()
                            .find(|parameter| parameter.name == **name)
                            .zip(
                                resp.parameters
                                    .iter()
                                    .find(|parameter| parameter.name == **name),
                            )
                            .is_some_and(|(expected, actual)| {
                                administrative_states(&expected.value, &actual.value).is_some()
                            })
                    })
                    .cloned()
                    .collect::<HashSet<_>>();
                let durable_mismatched = mismatched
                    .iter()
                    .filter(|name| !transient_admin.contains(*name))
                    .cloned()
                    .collect::<Vec<_>>();
                if !durable_mismatched.is_empty() {
                    log::info!(
                        "ACS: [{}] configured parameter drift: {}",
                        self.feid_tag(),
                        durable_mismatched.join(", ")
                    );
                }
                if durable_mismatched.is_empty()
                    && !transient_admin.is_empty()
                    && reapplied.is_empty()
                {
                    self.configuration_verified = true;
                    self.pending_parameters = transient_admin;
                    log::info!(
                        "ACS: [{}] durable configuration matches; applying final administrative state",
                        self.feid_tag()
                    );
                    self.send_next_unlock_stage(stages, 0, discovery_query, status_query)
                } else if matches && reapplied.is_empty() {
                    self.configuration_verified = true;
                    log::info!(
                        "ACS: [{}] configured parameters already match; skipping provisioning stages",
                        self.feid_tag()
                    );
                    self.finish_or_read(discovery_query, status_query)
                } else {
                    self.configuration_verified = false;
                    self.pending_parameters = mismatched.into_iter().chain(reapplied).collect();
                    if matches {
                        log::info!(
                            "ACS: [{}] reapplying configured provisioning stages",
                            self.feid_tag()
                        );
                    } else {
                        log::info!(
                            "ACS: [{}] configuration drift detected; applying provisioning stages",
                            self.feid_tag()
                        );
                    }
                    self.send_next_stage(stages, 0, discovery_query, status_query)
                }
            }
            (AcsPhase::ObjectProbeSent, CwmpMessage::GetParameterNamesResponse(resp)) => {
                let object = &objects[self.object];
                let required_prefix = object
                    .required_instance
                    .map(|instance| format!("{}{instance}.", object.name));
                let present = resp.parameters.iter().any(|parameter| {
                    parameter
                        .name
                        .starts_with(required_prefix.as_deref().unwrap_or(object.name.as_str()))
                });
                if present {
                    log::info!(
                        "ACS: [{}] CWMP object {} already exists",
                        self.feid_tag(),
                        object.name
                    );
                    self.continue_after_object(objects, stages, discovery_query, status_query)
                } else {
                    self.send_object(objects, self.object)
                }
            }
            (AcsPhase::ObjectProbeSent, CwmpMessage::GetParameterValuesResponse(_)) => {
                let object = &objects[self.object];
                log::info!(
                    "ACS: [{}] instance {}{}. present; skipping AddObject",
                    self.feid_tag(),
                    object.name,
                    object.required_instance.unwrap_or_default()
                );
                self.continue_after_object(objects, stages, discovery_query, status_query)
            }
            (AcsPhase::ObjectCreationSent, CwmpMessage::AddObjectResponse(resp)) => {
                log::info!(
                    "ACS: [{}] AddObjectResponse instance={} status={}",
                    self.feid_tag(),
                    resp.instance_number,
                    resp.status
                );
                let object = &objects[self.object];
                self.created_objects.insert(object.name.clone());
                // Firmware allocates instance numbers monotonically and never reuses
                // them, so AddObject may return an instance other than the required one.
                if let Some(required) = object.required_instance {
                    if resp.instance_number != required {
                        log::warn!(
                            "ACS: [{}] {} created instance {} (required {}); remapping writes to {}",
                            self.feid_tag(),
                            object.name,
                            resp.instance_number,
                            required,
                            resp.instance_number,
                        );
                        self.instance_remap.push((
                            format!("{}{required}.", object.name),
                            format!("{}{}.", object.name, resp.instance_number),
                        ));
                    }
                }
                self.discover_object(objects)
            }
            (AcsPhase::ObjectDiscoverySent, CwmpMessage::GetParameterNamesResponse(resp)) => {
                let names = resp
                    .parameters
                    .iter()
                    .map(|parameter| parameter.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                log::info!("ACS: [{}] ensured object exposes: {names}", self.feid_tag());
                self.continue_after_object(objects, stages, discovery_query, status_query)
            }
            (AcsPhase::ObjectDeletionSent, CwmpMessage::DeleteObjectResponse(resp)) => {
                let object = stages[self.stage]
                    .replace_object
                    .as_ref()
                    .expect("replacement stage has an object");
                log::info!(
                    "ACS: [{}] DeleteObjectResponse status={}",
                    self.feid_tag(),
                    resp.status
                );
                self.phase = AcsPhase::ReplacementCreationSent;
                AcsAction::Respond(Box::new(CwmpMessage::AddObject(AddObject {
                    object_name: object.name.clone(),
                    parameter_key: "1xbts-femto".to_string(),
                })))
            }
            (AcsPhase::ReplacementCreationSent, CwmpMessage::AddObjectResponse(resp)) => {
                let object = stages[self.stage]
                    .replace_object
                    .as_ref()
                    .expect("replacement stage has an object");
                if object.required_instance != Some(resp.instance_number) {
                    log::error!(
                        "ACS: [{}] replacement allocated instance {}, expected {:?}; attempting unlock",
                        self.feid_tag(),
                        resp.instance_number,
                        object.required_instance
                    );
                    self.recovering_unlock = true;
                    self.send_next_unlock_stage(
                        stages,
                        self.stage + 1,
                        discovery_query,
                        status_query,
                    )
                } else {
                    log::info!(
                        "ACS: [{}] replacement AddObjectResponse instance={} status={}",
                        self.feid_tag(),
                        resp.instance_number,
                        resp.status
                    );
                    self.created_objects.insert(object.name.clone());
                    self.send_next_stage(stages, self.stage + 1, discovery_query, status_query)
                }
            }
            (AcsPhase::ProvisioningSent, CwmpMessage::SetParameterValuesResponse(resp)) => {
                log::info!(
                    "ACS: [{}] SetParameterValuesResponse status={} ({})",
                    self.feid_tag(),
                    resp.status,
                    if resp.status == 0 {
                        "applied"
                    } else {
                        "applied after reboot"
                    }
                );
                if let Some(step) = self.bracket {
                    self.advance_bracket(step, stages, discovery_query, status_query)
                } else if self.recovering_unlock {
                    self.send_next_unlock_stage(
                        stages,
                        self.stage + 1,
                        discovery_query,
                        status_query,
                    )
                } else if self.stage + 1 < stages.len() {
                    let next = self.stage + 1;
                    self.send_next_stage(stages, next, discovery_query, status_query)
                } else {
                    self.finish_or_read(discovery_query, status_query)
                }
            }
            (AcsPhase::StatusQuerySent, CwmpMessage::GetParameterValuesResponse(resp)) => {
                let values = resp
                    .parameters
                    .iter()
                    .map(|p| {
                        let leaf = p.name.rsplit('.').next().unwrap_or(&p.name);
                        if is_sensitive_parameter(&p.name) {
                            format!("{leaf}=[REDACTED]")
                        } else {
                            format!("{leaf}={}", p.value)
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(", ");
                log::info!("ACS: [{}] status readback: {values}", self.feid_tag());
                self.phase = AcsPhase::Done;
                AcsAction::EndSession
            }
            (AcsPhase::ElementSettleQuerySent, CwmpMessage::GetParameterValuesResponse(resp)) => {
                let Some(BracketStep::AwaitElementDown(idx, polls)) = self.bracket else {
                    return self.finish_or_read(discovery_query, status_query);
                };
                let bracket = stages[idx].bracket_admin.as_ref();
                let settle_param = bracket.and_then(|b| b.settle_parameter.clone());
                let operating = bracket.and_then(|b| b.settle_operating_value.clone());
                let current = settle_param
                    .as_ref()
                    .and_then(|name| resp.parameters.iter().find(|p| &p.name == name))
                    .map(|p| p.value.clone());
                // With no operating reference or an unreadable status, treat the
                // element as ready.
                let left_operating = match (&operating, &current) {
                    (Some(op), Some(cur)) => cur != op,
                    _ => true,
                };
                if left_operating {
                    log::info!(
                        "ACS: [{}] element {} = {}; applying bracketed config",
                        self.feid_tag(),
                        settle_param.as_deref().map(leaf_name).unwrap_or("status"),
                        current.as_deref().unwrap_or("?")
                    );
                    self.send_bracket_config(stages, idx)
                } else if polls > 0 {
                    let settle = settle_param.expect("settle parameter present while polling");
                    self.poll_element_settle(idx, settle, polls - 1)
                } else {
                    log::warn!(
                        "ACS: [{}] element still operating after settle polls; applying config anyway",
                        self.feid_tag()
                    );
                    self.send_bracket_config(stages, idx)
                }
            }
            (AcsPhase::DiscoveryQuerySent, CwmpMessage::GetParameterNamesResponse(resp)) => {
                let writable = resp
                    .parameters
                    .iter()
                    .filter(|parameter| parameter.writable)
                    .count();
                let names = resp
                    .parameters
                    .iter()
                    .map(|parameter| parameter.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ");
                log::info!(
                    "ACS: [{}] discovery returned {} name(s), {writable} writable: {names}",
                    self.feid_tag(),
                    resp.parameters.len()
                );
                self.discovery += 1;
                self.finish_or_read(discovery_query, status_query)
            }
            (phase, CwmpMessage::Fault(fault)) => {
                for parameter in &fault.parameter_faults {
                    log::warn!(
                        "ACS: [{}] parameter fault {}: {} {}",
                        self.feid_tag(),
                        parameter.parameter_name,
                        parameter.fault_code,
                        parameter.fault_string
                    );
                }
                // A fault mid-bracket must still bring the element back up, not leave it admin-down.
                if let Some(step) = self.bracket {
                    return match step {
                        BracketStep::ConfigAfterDown(idx)
                        | BracketStep::AwaitElementDown(idx, _)
                        | BracketStep::UpAfterConfig(idx) => {
                            log::warn!(
                                "ACS: [{}] fault {} {} during bracketed stage; restoring admin up",
                                self.feid_tag(),
                                fault.fault_code,
                                fault.fault_string
                            );
                            self.send_bracket_up(stages, idx, BracketStep::EndAfterRestore)
                        }
                        BracketStep::AdvanceAfterUp(_) | BracketStep::EndAfterRestore => {
                            self.bracket = None;
                            self.finish_or_read(discovery_query, status_query)
                        }
                    };
                }
                if phase == AcsPhase::ConfigurationQuerySent {
                    log::warn!(
                        "ACS: [{}] configuration read fault {} {}; applying provisioning stages",
                        self.feid_tag(),
                        fault.fault_code,
                        fault.fault_string
                    );
                    self.derive_values(stages, &[]);
                    self.pending_parameters = self
                        .settled_parameters(stages)
                        .into_iter()
                        .map(|parameter| parameter.name)
                        .collect();
                    self.send_next_stage(stages, 0, discovery_query, status_query)
                } else if phase == AcsPhase::ObjectProbeSent {
                    log::info!(
                        "ACS: [{}] CWMP object {} is absent; creating it",
                        self.feid_tag(),
                        objects[self.object].name
                    );
                    self.send_object(objects, self.object)
                } else if matches!(
                    phase,
                    AcsPhase::ObjectCreationSent | AcsPhase::ObjectDiscoverySent
                ) {
                    log::warn!(
                        "ACS: [{}] AddObject fault {} {}; continuing with parameter provisioning",
                        self.feid_tag(),
                        fault.fault_code,
                        fault.fault_string
                    );
                    self.continue_after_object(objects, stages, discovery_query, status_query)
                } else if phase == AcsPhase::ObjectDeletionSent {
                    log::warn!(
                        "ACS: [{}] DeleteObject fault {} {}; attempting full object configuration",
                        self.feid_tag(),
                        fault.fault_code,
                        fault.fault_string
                    );
                    self.send_next_stage(stages, self.stage + 1, discovery_query, status_query)
                } else if matches!(
                    phase,
                    AcsPhase::ProvisioningSent | AcsPhase::ReplacementCreationSent
                ) && !self.recovering_unlock
                    && let Some(unlock) =
                        stages
                            .iter()
                            .enumerate()
                            .skip(self.stage + 1)
                            .find_map(|(index, stage)| {
                                Self::is_unlock_stage(&stage.label).then_some(index)
                            })
                {
                    log::warn!(
                        "ACS: [{}] CPE Fault in provisioning stage {}: {} {}; attempting unlock stage",
                        self.feid_tag(),
                        self.stage,
                        fault.fault_code,
                        fault.fault_string
                    );
                    self.recovering_unlock = true;
                    self.send_stage(stages, unlock)
                        .unwrap_or_else(|| self.finish_or_read(discovery_query, status_query))
                } else {
                    log::warn!(
                        "ACS: [{}] CPE Fault in phase {phase:?}: {} {} -> EndSession",
                        self.feid_tag(),
                        fault.fault_code,
                        fault.fault_string
                    );
                    self.phase = AcsPhase::Done;
                    AcsAction::EndSession
                }
            }
            (phase, _) => {
                log::debug!(
                    "ACS: [{}] unexpected message in phase {phase:?} -> EndSession",
                    self.feid_tag()
                );
                self.phase = AcsPhase::Done;
                AcsAction::EndSession
            }
        }
    }
}

impl Default for AcsSession {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::http::scoped_session_key;
    use super::*;
    use crate::config::{
        CwmpParam, FapAcsConfig, FapDefinition, FapProfileConfig, FemtoSiteConfig,
    };
    use cdma_cwmp::{DeviceId, EventStruct, Fault, Inform};
    use std::collections::BTreeMap;

    #[test]
    fn cwmp_program_is_selected_per_fap_profile() {
        let config: AcsConfig = serde_json::from_str(r#"{"listen":"127.0.0.1:7003"}"#).unwrap();
        let profiles = BTreeMap::from([
            (
                "airave".to_string(),
                FapProfileConfig {
                    implementation: FapImplementationKind::AirvanaAirave2,
                    connection_request: None,
                    acs: None,
                },
            ),
            (
                "standard".to_string(),
                FapProfileConfig {
                    implementation: FapImplementationKind::StandardXs0059,
                    connection_request: None,
                    acs: Some(FapAcsConfig {
                        stages: vec![ProvisioningStage {
                            label: "standard".to_string(),
                            atomic: false,
                            replace_object: None,
                            bracket_admin: None,
                            params: Vec::new(),
                        }],
                        ..FapAcsConfig::default()
                    }),
                },
            ),
        ]);
        let sites = BTreeMap::from([(
            "site".to_string(),
            FemtoSiteConfig {
                mcc: "310".to_string(),
                imsi_11_12: "00".to_string(),
                cell_id: 403,
                sector: 0,
                ..FemtoSiteConfig::default()
            },
        )]);
        let faps = BTreeMap::from([
            (
                "airave".to_string(),
                FapDefinition {
                    profile: "airave".to_string(),
                    site: "site".to_string(),
                    name: None,
                    cwmp_inform_response_style: CwmpInformResponseStyle::Standard,
                },
            ),
            (
                "standard".to_string(),
                FapDefinition {
                    profile: "standard".to_string(),
                    site: "site".to_string(),
                    name: None,
                    cwmp_inform_response_style: CwmpInformResponseStyle::Standard,
                },
            ),
        ]);
        let registry = Arc::new(std::sync::RwLock::new(Arc::new(
            crate::fap::FapRegistry::from_config(&profiles, &sites, &faps).unwrap(),
        )));
        let server = AcsServer::new(&config).with_fap_registry(registry);

        assert_eq!(server.program(Some("standard")).stages[0].label, "standard");
        assert!(server.program(Some("airave")).stages.is_empty());
        assert!(server.program(None).stages.is_empty());
    }

    #[test]
    fn identical_cwmp_cookies_are_scoped_to_the_authenticated_fap() {
        assert_ne!(
            scoped_session_key("fap-a", "session-1"),
            scoped_session_key("fap-b", "session-1")
        );
    }

    fn inform() -> CwmpMessage {
        inform_with_event("0 BOOTSTRAP")
    }

    fn inform_with_event(event_code: &str) -> CwmpMessage {
        CwmpMessage::Inform(Inform {
            device_id: DeviceId {
                manufacturer: "Airvana".to_string(),
                oui: "0005B9".to_string(),
                product_class: "cdmaFAP".to_string(),
                serial_number: "0005B9AABBCC".to_string(),
            },
            events: vec![EventStruct {
                event_code: event_code.to_string(),
                command_key: String::new(),
            }],
            max_envelopes: 1,
            current_time: String::new(),
            retry_count: 0,
            parameter_list: vec![],
        })
    }

    fn prov() -> Vec<ProvisioningStage> {
        vec![ProvisioningStage {
            label: "unlock".to_string(),
            atomic: false,
            replace_object: None,
            bracket_admin: None,
            params: vec![CwmpParam {
                name:
                    "InternetGatewayDevice.Services.X_0005B9_OneXMO.X_0005B9_OneXRTTMO.AdminState"
                        .to_string(),
                value: "unlock".to_string(),
                value_from: None,
                xsd_type: "xsd:string".to_string(),
                verify: true,
            }],
        }]
    }

    fn two_stages() -> Vec<ProvisioningStage> {
        let p = |n: &str, v: &str| CwmpParam {
            name: n.to_string(),
            value: v.to_string(),
            value_from: None,
            xsd_type: "xsd:string".to_string(),
            verify: true,
        };
        vec![
            ProvisioningStage {
                label: "lock".to_string(),
                atomic: false,
                replace_object: None,
                bracket_admin: None,
                params: vec![p("A.OneXRTTMO.AdminState", "lock")],
            },
            ProvisioningStage {
                label: "unlock".to_string(),
                atomic: false,
                replace_object: None,
                bracket_admin: None,
                params: vec![p("A.OneXRTTMO.AdminState", "unlock")],
            },
        ]
    }

    fn report_configuration_drift(
        session: &mut AcsSession,
        objects: &[CwmpObject],
        stages: &[ProvisioningStage],
        status: &[String],
    ) -> AcsAction {
        let query = session.step_with_objects(
            &CwmpMessage::Empty,
            objects,
            stages,
            &[],
            status,
            false,
            true,
        );
        assert!(matches!(
            query,
            AcsAction::Respond(ref message)
                if matches!(**message, CwmpMessage::GetParameterValues(_))
        ));
        session.step_with_objects(
            &CwmpMessage::GetParameterValuesResponse(Default::default()),
            objects,
            stages,
            &[],
            status,
            false,
            true,
        )
    }

    fn airave_session() -> AcsSession {
        let mut session = AcsSession::new();
        session.set_implementation(implementation(FapImplementationKind::AirvanaAirave2));
        session
    }

    #[test]
    fn new_session_uses_standard_behavior() {
        assert_eq!(
            AcsSession::new().implementation.kind(),
            FapImplementationKind::StandardXs0059
        );
    }

    #[test]
    fn full_provisioning_session_when_service_enabled() {
        let mut s = airave_session();
        let a = s.step(&inform(), &prov(), &[], false, true);
        assert!(matches!(
            a,
            AcsAction::Respond(ref m) if matches!(**m, CwmpMessage::InformResponse(_))
        ));
        let a = report_configuration_drift(&mut s, &[], &prov(), &[]);
        match a {
            AcsAction::Respond(m) => match *m {
                CwmpMessage::SetParameterValues(spv) => {
                    assert_eq!(spv.parameter_list.len(), 1);
                    assert!(spv.parameter_list[0].name.contains("AdminState"));
                }
                other => panic!("expected SetParameterValues, got {other:?}"),
            },
            other => panic!("expected Respond, got {other:?}"),
        }
        assert_eq!(
            s.step(
                &CwmpMessage::SetParameterValuesResponse(Default::default()),
                &prov(),
                &[],
                false,
                true
            ),
            AcsAction::EndSession
        );
        assert_eq!(s.phase(), AcsPhase::Done);
    }

    #[test]
    fn advances_through_provisioning_stages_in_order() {
        let stages = two_stages();
        let mut s = airave_session();
        s.step(&inform(), &stages, &[], false, true);
        let a = report_configuration_drift(&mut s, &[], &stages, &[]);
        match a {
            AcsAction::Respond(m) => match *m {
                CwmpMessage::SetParameterValues(spv) => {
                    assert_eq!(spv.parameter_list[0].value, "lock");
                }
                other => panic!("expected SPV(lock), got {other:?}"),
            },
            other => panic!("expected Respond, got {other:?}"),
        }
        let a = s.step(
            &CwmpMessage::SetParameterValuesResponse(Default::default()),
            &stages,
            &[],
            false,
            true,
        );
        match a {
            AcsAction::Respond(m) => match *m {
                CwmpMessage::SetParameterValues(spv) => {
                    assert_eq!(spv.parameter_list[0].value, "unlock");
                }
                other => panic!("expected SPV(unlock), got {other:?}"),
            },
            other => panic!("expected Respond, got {other:?}"),
        }
        assert_eq!(
            s.step(
                &CwmpMessage::SetParameterValuesResponse(Default::default()),
                &stages,
                &[],
                false,
                true
            ),
            AcsAction::EndSession
        );
        assert_eq!(s.phase(), AcsPhase::Done);
    }

    #[test]
    fn matching_configuration_skips_lock_and_unlock_stages() {
        let stages = two_stages();
        let mut session = airave_session();
        session.step(&inform(), &stages, &[], false, true);

        let query = session.step(&CwmpMessage::Empty, &stages, &[], false, true);
        assert!(matches!(
            query,
            AcsAction::Respond(ref message)
                if matches!(**message, CwmpMessage::GetParameterValues(_))
        ));

        let response =
            CwmpMessage::GetParameterValuesResponse(cdma_cwmp::GetParameterValuesResponse {
                parameters: vec![ParameterValue {
                    name: "A.OneXRTTMO.AdminState".to_string(),
                    value: "unlock".to_string(),
                    xsi_type: "xsd:string".to_string(),
                }],
            });
        assert_eq!(
            session.step(&response, &stages, &[], false, true),
            AcsAction::EndSession
        );
        assert_eq!(session.phase(), AcsPhase::Done);
        assert!(session.configuration_verified());
    }

    #[test]
    fn matching_configuration_reapplies_selected_stage_with_transitions() {
        let stages = two_stages();
        let mut session = airave_session();
        session.set_reapply_stages(&["unlock".to_string()]);
        session.step(&inform(), &stages, &[], false, true);
        session.step(&CwmpMessage::Empty, &stages, &[], false, true);

        let response =
            CwmpMessage::GetParameterValuesResponse(cdma_cwmp::GetParameterValuesResponse {
                parameters: vec![ParameterValue {
                    name: "A.OneXRTTMO.AdminState".to_string(),
                    value: "unlock".to_string(),
                    xsi_type: "xsd:string".to_string(),
                }],
            });
        let action = session.step(&response, &stages, &[], false, true);
        assert!(matches!(
            action,
            AcsAction::Respond(ref message)
                if matches!(
                    **message,
                    CwmpMessage::SetParameterValues(ref request)
                        if request.parameter_list[0].value == "lock"
                )
        ));

        let action = session.step(
            &CwmpMessage::SetParameterValuesResponse(Default::default()),
            &stages,
            &[],
            false,
            true,
        );
        assert!(matches!(
            action,
            AcsAction::Respond(ref message)
                if matches!(
                    **message,
                    CwmpMessage::SetParameterValues(ref request)
                        if request.parameter_list[0].value == "unlock"
                )
        ));
    }

    #[test]
    fn derives_agps_reference_from_current_location() {
        let current = "Device.GPS.CurrentLatitude";
        let reference = "Device.GPS.ReferenceLatitude";
        let stages = vec![ProvisioningStage {
            label: "configure".to_string(),
            atomic: false,
            replace_object: None,
            bracket_admin: None,
            params: vec![CwmpParam {
                name: reference.to_string(),
                value: String::new(),
                value_from: Some(crate::config::CwmpValueSource::CpeParameter {
                    parameter: current.to_string(),
                    transform: CwmpValueTransform::GpsDegreesMinutesToDecimalDegrees,
                }),
                xsd_type: "xsd:string".to_string(),
                verify: true,
            }],
        }];
        let mut session = airave_session();
        session.step(&inform(), &stages, &[], false, true);
        let query = session.step(&CwmpMessage::Empty, &stages, &[], false, true);
        assert!(matches!(
            query,
            AcsAction::Respond(ref message)
                if matches!(
                    **message,
                    CwmpMessage::GetParameterValues(ref request)
                        if request.parameter_names.contains(&current.to_string())
                            && request.parameter_names.contains(&reference.to_string())
                )
        ));

        let response =
            CwmpMessage::GetParameterValuesResponse(cdma_cwmp::GetParameterValuesResponse {
                parameters: vec![
                    ParameterValue {
                        name: current.to_string(),
                        value: "+4000.000000".to_string(),
                        xsi_type: "xsd:string".to_string(),
                    },
                    ParameterValue {
                        name: reference.to_string(),
                        value: "NONE".to_string(),
                        xsi_type: "xsd:string".to_string(),
                    },
                ],
            });
        let action = session.step(&response, &stages, &[], false, true);
        assert!(matches!(
            action,
            AcsAction::Respond(ref message)
                if matches!(
                    **message,
                    CwmpMessage::SetParameterValues(ref request)
                        if request.parameter_list[0].value == "+40.000000"
                )
        ));
    }

    #[test]
    fn value_change_unlocks_when_durable_matches() {
        let stages = two_stages();
        let mut session = airave_session();
        session.step(
            &inform_with_event("4 VALUE CHANGE"),
            &stages,
            &[],
            false,
            true,
        );
        session.step(&CwmpMessage::Empty, &stages, &[], false, true);

        let response =
            CwmpMessage::GetParameterValuesResponse(cdma_cwmp::GetParameterValuesResponse {
                parameters: vec![ParameterValue {
                    name: "A.OneXRTTMO.AdminState".to_string(),
                    value: "locked".to_string(),
                    xsi_type: "xsd:string".to_string(),
                }],
            });

        let action = session.step(&response, &stages, &[], false, true);
        assert!(matches!(
            action,
            AcsAction::Respond(ref message)
                if matches!(
                    **message,
                    CwmpMessage::SetParameterValues(ref request)
                        if request.parameter_list.len() == 1
                            && request.parameter_list[0].name == "A.OneXRTTMO.AdminState"
                            && request.parameter_list[0].value == "unlock"
                )
        ));
    }

    #[test]
    fn connection_request_unlocks_when_durable_matches() {
        let stages = two_stages();
        let mut session = airave_session();
        session.step(
            &inform_with_event("6 CONNECTION REQUEST"),
            &stages,
            &[],
            false,
            true,
        );
        session.step(&CwmpMessage::Empty, &stages, &[], false, true);

        let response =
            CwmpMessage::GetParameterValuesResponse(cdma_cwmp::GetParameterValuesResponse {
                parameters: vec![ParameterValue {
                    name: "A.OneXRTTMO.AdminState".to_string(),
                    value: "locked".to_string(),
                    xsi_type: "xsd:string".to_string(),
                }],
            });

        let action = session.step(&response, &stages, &[], false, true);
        assert!(matches!(
            action,
            AcsAction::Respond(ref message)
                if matches!(
                    **message,
                    CwmpMessage::SetParameterValues(ref request)
                        if request.parameter_list.len() == 1
                            && request.parameter_list[0].name == "A.OneXRTTMO.AdminState"
                            && request.parameter_list[0].value == "unlock"
                )
        ));
        assert!(session.configuration_verified());
    }

    #[test]
    fn boot_time_locked_state_does_not_trigger_provisioning() {
        let stages = two_stages();
        let mut session = airave_session();
        session.step(&inform(), &stages, &[], false, true);
        session.step(&CwmpMessage::Empty, &stages, &[], false, true);

        let response =
            CwmpMessage::GetParameterValuesResponse(cdma_cwmp::GetParameterValuesResponse {
                parameters: vec![ParameterValue {
                    name: "A.OneXRTTMO.AdminState".to_string(),
                    value: "locked".to_string(),
                    xsi_type: "xsd:string".to_string(),
                }],
            });

        let action = session.step(&response, &stages, &[], false, true);
        assert!(matches!(
            action,
            AcsAction::Respond(ref message)
                if matches!(
                    **message,
                    CwmpMessage::SetParameterValues(ref request)
                        if request.parameter_list.len() == 1
                            && request.parameter_list[0].name == "A.OneXRTTMO.AdminState"
                            && request.parameter_list[0].value == "unlock"
                )
        ));
        assert!(session.configuration_verified());
    }

    #[test]
    fn retains_gps_reference_within_reconcile_distance() {
        let latitude = "Device.GPS.Latitude";
        let longitude = "Device.GPS.Longitude";
        let reference_latitude = "Device.GPS.aGPSReferenceLatitude";
        let reference_longitude = "Device.GPS.aGPSReferenceLongitude";
        let derived = |name: &str, source: &str| CwmpParam {
            name: name.to_string(),
            value: String::new(),
            value_from: Some(CwmpValueSource::CpeParameter {
                parameter: source.to_string(),
                transform: CwmpValueTransform::GpsDegreesMinutesToDecimalDegrees,
            }),
            xsd_type: "xsd:string".to_string(),
            verify: true,
        };
        let stages = vec![ProvisioningStage {
            label: "configure".to_string(),
            atomic: false,
            replace_object: None,
            bracket_admin: None,
            params: vec![
                derived(reference_latitude, latitude),
                derived(reference_longitude, longitude),
            ],
        }];
        let value = |name: &str, value: &str| ParameterValue {
            name: name.to_string(),
            value: value.to_string(),
            xsi_type: "xsd:string".to_string(),
        };
        let readback = vec![
            value(latitude, "+4000.001000"),
            value(longitude, "-07500.001000"),
            value(reference_latitude, "+40.000000"),
            value(reference_longitude, "-75.000000"),
            value(GPS_CHANGED_LOCATION_DISTANCE_PARAMETER, "100"),
        ];
        let mut session = airave_session();

        session.derive_values(&stages, &readback);

        let settled = session.settled_parameters(&stages);
        assert!(settled.iter().any(|parameter| {
            parameter.name == reference_latitude && parameter.value == "+40.000000"
        }));
        assert!(settled.iter().any(|parameter| {
            parameter.name == reference_longitude && parameter.value == "-75.000000"
        }));

        let relocated = vec![
            value(latitude, "+4001.000000"),
            value(longitude, "-07500.001000"),
            value(reference_latitude, "+40.000000"),
            value(reference_longitude, "-75.000000"),
            value(GPS_CHANGED_LOCATION_DISTANCE_PARAMETER, "100"),
        ];
        session.derive_values(&stages, &relocated);
        let settled = session.settled_parameters(&stages);
        assert!(settled.iter().any(|parameter| {
            parameter.name == reference_latitude && parameter.value == "+40.016667"
        }));
    }

    #[test]
    fn derives_fap_and_site_values_without_querying_cpe_sources() {
        let derived = |name: &str, context| CwmpParam {
            name: name.to_string(),
            value: String::new(),
            value_from: Some(crate::config::CwmpValueSource::FapContext { context }),
            xsd_type: "xsd:string".to_string(),
            verify: true,
        };
        let stages = vec![ProvisioningStage {
            label: "configure".to_string(),
            atomic: true,
            replace_object: None,
            bracket_admin: None,
            params: vec![
                derived("Iwf.DefaultImpi", CwmpFapValue::Feid),
                derived("Sector.SID", CwmpFapValue::SiteSid),
                derived("Sector.NID", CwmpFapValue::SiteNid),
                derived("Sector.BandClass", CwmpFapValue::SiteBandClass),
                derived("Sector.Channel", CwmpFapValue::SiteCdmaChannel),
            ],
        }];
        let mut session = airave_session();
        session.fap_context = Some(FapProvisioningContext {
            feid: "0005B9DDEEFF".to_string(),
            sid: 2,
            nid: 65_535,
            band_class: 1,
            cdma_channel: 50,
            max_sector_power_dbm: None,
            sip_password: None,
            pn_offset: None,
            onex_enabled: true,
            evdo_enabled: false,
            evdo_band_class: None,
            evdo_channel: None,
            evdo_cell_id: None,
            evdo_sector_id: None,
            evdo_color_code: None,
            evdo_tx_power: None,
            evdo_pn_offset: None,
            term_auth_psk: None,
            timezone: None,
        });
        session.step(&inform(), &stages, &[], false, true);
        let query = session.step(&CwmpMessage::Empty, &stages, &[], false, true);
        let AcsAction::Respond(message) = query else {
            panic!("expected configuration query");
        };
        let CwmpMessage::GetParameterValues(request) = *message else {
            panic!("expected GetParameterValues");
        };
        assert_eq!(request.parameter_names.len(), stages[0].params.len());

        let response =
            CwmpMessage::GetParameterValuesResponse(cdma_cwmp::GetParameterValuesResponse {
                parameters: stages[0]
                    .params
                    .iter()
                    .map(|parameter| ParameterValue {
                        name: parameter.name.clone(),
                        value: "stale".to_string(),
                        xsi_type: parameter.xsd_type.clone(),
                    })
                    .collect(),
            });
        let action = session.step(&response, &stages, &[], false, true);
        let AcsAction::Respond(message) = action else {
            panic!("expected provisioning write");
        };
        let CwmpMessage::SetParameterValues(request) = *message else {
            panic!("expected SetParameterValues");
        };
        let values = request
            .parameter_list
            .into_iter()
            .map(|parameter| (parameter.name, parameter.value))
            .collect::<HashMap<_, _>>();
        assert_eq!(values["Iwf.DefaultImpi"], "0005B9DDEEFF");
        assert_eq!(values["Sector.SID"], "2");
        assert_eq!(values["Sector.NID"], "65535");
        assert_eq!(values["Sector.BandClass"], "1");
        assert_eq!(values["Sector.Channel"], "50");
    }

    #[test]
    fn omits_max_sector_power_unless_the_site_sets_it() {
        let derived = |name: &str, context| CwmpParam {
            name: name.to_string(),
            value: String::new(),
            value_from: Some(crate::config::CwmpValueSource::FapContext { context }),
            xsd_type: "xsd:int".to_string(),
            verify: true,
        };
        let stages = vec![ProvisioningStage {
            label: "configure".to_string(),
            atomic: false,
            replace_object: None,
            bracket_admin: None,
            params: vec![
                derived("Sector.SID", CwmpFapValue::SiteSid),
                derived("Sector.MaxPower", CwmpFapValue::SiteMaxSectorPowerDbm),
            ],
        }];
        let mut session = airave_session();
        session.fap_context = Some(FapProvisioningContext {
            feid: "0005B9AABBCC".to_string(),
            sid: 4107,
            nid: 65_535,
            band_class: 1,
            cdma_channel: 50,
            max_sector_power_dbm: None,
            sip_password: None,
            pn_offset: None,
            onex_enabled: true,
            evdo_enabled: false,
            evdo_band_class: None,
            evdo_channel: None,
            evdo_cell_id: None,
            evdo_sector_id: None,
            evdo_color_code: None,
            evdo_tx_power: None,
            evdo_pn_offset: None,
            term_auth_psk: None,
            timezone: None,
        });
        session.step(&inform(), &stages, &[], false, true);
        let query = session.step(&CwmpMessage::Empty, &stages, &[], false, true);
        let AcsAction::Respond(message) = query else {
            panic!("expected configuration query");
        };
        let CwmpMessage::GetParameterValues(request) = *message else {
            panic!("expected GetParameterValues");
        };
        assert_eq!(request.parameter_names, vec!["Sector.SID".to_string()]);
        session.derive_values(&stages, &[]);
        let settled = session.settled_parameters(&stages);
        assert_eq!(settled.len(), 1);
        assert_eq!(settled[0].name, "Sector.SID");

        session
            .fap_context
            .as_mut()
            .expect("context")
            .max_sector_power_dbm = Some(1);
        session.derive_values(&stages, &[]);
        let settled = session.settled_parameters(&stages);
        assert_eq!(
            settled
                .iter()
                .find(|parameter| parameter.name == "Sector.MaxPower")
                .map(|parameter| parameter.value.as_str()),
            Some("1")
        );

        session
            .fap_context
            .as_mut()
            .expect("context")
            .max_sector_power_dbm = Some(-8);
        session.derive_values(&stages, &[]);
        let settled = session.settled_parameters(&stages);
        assert_eq!(
            settled
                .iter()
                .find(|parameter| parameter.name == "Sector.MaxPower")
                .map(|parameter| parameter.value.as_str()),
            Some("-8")
        );
    }

    #[test]
    fn omits_sip_password_unless_the_site_sets_it() {
        let derived = |name: &str, context| CwmpParam {
            name: name.to_string(),
            value: String::new(),
            value_from: Some(crate::config::CwmpValueSource::FapContext { context }),
            xsd_type: "xsd:string".to_string(),
            verify: true,
        };
        let stages = vec![ProvisioningStage {
            label: "configure".to_string(),
            atomic: false,
            replace_object: None,
            bracket_admin: None,
            params: vec![
                derived("Sector.SID", CwmpFapValue::SiteSid),
                derived("Sip.Password", CwmpFapValue::SiteSipPassword),
            ],
        }];
        let mut session = airave_session();
        session.fap_context = Some(FapProvisioningContext {
            feid: "0005B9AABBCC".to_string(),
            sid: 4107,
            nid: 65_535,
            band_class: 1,
            cdma_channel: 50,
            max_sector_power_dbm: None,
            sip_password: None,
            pn_offset: None,
            onex_enabled: true,
            evdo_enabled: false,
            evdo_band_class: None,
            evdo_channel: None,
            evdo_cell_id: None,
            evdo_sector_id: None,
            evdo_color_code: None,
            evdo_tx_power: None,
            evdo_pn_offset: None,
            term_auth_psk: None,
            timezone: None,
        });
        session.step(&inform(), &stages, &[], false, true);
        let query = session.step(&CwmpMessage::Empty, &stages, &[], false, true);
        let AcsAction::Respond(message) = query else {
            panic!("expected configuration query");
        };
        let CwmpMessage::GetParameterValues(request) = *message else {
            panic!("expected GetParameterValues");
        };
        assert_eq!(request.parameter_names, vec!["Sector.SID".to_string()]);
        session.derive_values(&stages, &[]);
        let settled = session.settled_parameters(&stages);
        assert_eq!(settled.len(), 1);
        assert_eq!(settled[0].name, "Sector.SID");

        session.fap_context.as_mut().expect("context").sip_password =
            Some("3731427934".to_string());
        session.derive_values(&stages, &[]);
        let settled = session.settled_parameters(&stages);
        assert_eq!(
            settled
                .iter()
                .find(|parameter| parameter.name == "Sip.Password")
                .map(|parameter| parameter.value.as_str()),
            Some("3731427934")
        );
    }

    #[test]
    fn provisioning_stage_sends_only_drifted_parameters() {
        let p = |name: &str, value: &str| CwmpParam {
            name: name.to_string(),
            value: value.to_string(),
            value_from: None,
            xsd_type: "xsd:string".to_string(),
            verify: true,
        };
        let write_only = |name: &str, value: &str| CwmpParam {
            name: name.to_string(),
            value: value.to_string(),
            value_from: None,
            xsd_type: "xsd:string".to_string(),
            verify: false,
        };
        let stages = vec![
            ProvisioningStage {
                label: "lock".to_string(),
                atomic: false,
                replace_object: None,
                bracket_admin: None,
                params: vec![p("Radio.AdminState", "lock")],
            },
            ProvisioningStage {
                label: "configure-pdsn".to_string(),
                atomic: false,
                replace_object: None,
                bracket_admin: None,
                params: vec![
                    p("Pdsn.Address", "10.56.0.2"),
                    p("Pdsn.SecurityParameterIndex", "256"),
                    write_only("Pdsn.SecurityKey", "not-logged"),
                ],
            },
            ProvisioningStage {
                label: "unlock".to_string(),
                atomic: false,
                replace_object: None,
                bracket_admin: None,
                params: vec![p("Radio.AdminState", "unlock")],
            },
        ];
        let mut session = airave_session();
        session.step(&inform(), &stages, &[], false, true);
        session.step(&CwmpMessage::Empty, &stages, &[], false, true);

        let response =
            CwmpMessage::GetParameterValuesResponse(cdma_cwmp::GetParameterValuesResponse {
                parameters: vec![
                    ParameterValue {
                        name: "Radio.AdminState".to_string(),
                        value: "unlock".to_string(),
                        xsi_type: "xsd:string".to_string(),
                    },
                    ParameterValue {
                        name: "Pdsn.Address".to_string(),
                        value: "10.4.58.140".to_string(),
                        xsi_type: "xsd:string".to_string(),
                    },
                    ParameterValue {
                        name: "Pdsn.SecurityParameterIndex".to_string(),
                        value: "256".to_string(),
                        xsi_type: "xsd:string".to_string(),
                    },
                ],
            });
        let action = session.step(&response, &stages, &[], false, true);
        assert!(matches!(
            action,
            AcsAction::Respond(ref message)
                if matches!(
                    **message,
                    CwmpMessage::SetParameterValues(ref request)
                        if request.parameter_list.len() == 1
                            && request.parameter_list[0].name == "Radio.AdminState"
                )
        ));

        let action = session.step(
            &CwmpMessage::SetParameterValuesResponse(Default::default()),
            &stages,
            &[],
            false,
            true,
        );
        assert!(matches!(
            action,
            AcsAction::Respond(ref message)
                if matches!(
                    **message,
                    CwmpMessage::SetParameterValues(ref request)
                        if request.parameter_list.len() == 1
                            && request.parameter_list[0].name == "Pdsn.Address"
                )
        ));
    }

    #[test]
    fn replaces_drifted_object_between_lock_and_configuration() {
        let object_name = "Device.Pdsn.Entry.";
        let address_name = "Device.Pdsn.Entry.2.Address";
        let parameter = |name: &str, value: &str| CwmpParam {
            name: name.to_string(),
            value: value.to_string(),
            value_from: None,
            xsd_type: "xsd:string".to_string(),
            verify: true,
        };
        let stages = vec![
            ProvisioningStage {
                label: "lock".to_string(),
                atomic: false,
                replace_object: None,
                bracket_admin: None,
                params: vec![parameter("Device.Radio.Admin", "lock")],
            },
            ProvisioningStage {
                label: "replace-pdsn".to_string(),
                atomic: false,
                replace_object: Some(CwmpObject {
                    name: object_name.to_string(),
                    next_level: false,
                    required_instance: Some(2),
                    existence_probe_leaf: None,
                }),
                bracket_admin: None,
                params: vec![],
            },
            ProvisioningStage {
                label: "configure-pdsn".to_string(),
                atomic: false,
                replace_object: None,
                bracket_admin: None,
                params: vec![parameter(address_name, "172.31.0.1")],
            },
            ProvisioningStage {
                label: "unlock".to_string(),
                atomic: false,
                replace_object: None,
                bracket_admin: None,
                params: vec![parameter("Device.Radio.Admin", "unlock")],
            },
        ];
        let mut session = airave_session();
        session.step(&inform(), &stages, &[], false, true);
        session.step(&CwmpMessage::Empty, &stages, &[], false, true);
        let drift =
            CwmpMessage::GetParameterValuesResponse(cdma_cwmp::GetParameterValuesResponse {
                parameters: vec![
                    ParameterValue {
                        name: "Device.Radio.Admin".to_string(),
                        value: "unlock".to_string(),
                        xsi_type: "xsd:string".to_string(),
                    },
                    ParameterValue {
                        name: address_name.to_string(),
                        value: "10.56.0.2".to_string(),
                        xsi_type: "xsd:string".to_string(),
                    },
                ],
            });
        assert!(matches!(
            session.step(&drift, &stages, &[], false, true),
            AcsAction::Respond(ref message)
                if matches!(**message, CwmpMessage::SetParameterValues(_))
        ));
        let delete = session.step(
            &CwmpMessage::SetParameterValuesResponse(cdma_cwmp::SetParameterValuesResponse {
                status: 0,
            }),
            &stages,
            &[],
            false,
            true,
        );
        assert!(matches!(
            delete,
            AcsAction::Respond(ref message)
                if matches!(
                    **message,
                    CwmpMessage::DeleteObject(ref request)
                        if request.object_name == "Device.Pdsn.Entry.2."
                )
        ));
        let add = session.step(
            &CwmpMessage::DeleteObjectResponse(cdma_cwmp::DeleteObjectResponse { status: 0 }),
            &stages,
            &[],
            false,
            true,
        );
        assert!(matches!(
            add,
            AcsAction::Respond(ref message)
                if matches!(
                    **message,
                    CwmpMessage::AddObject(ref request) if request.object_name == object_name
                )
        ));
        let configure = session.step(
            &CwmpMessage::AddObjectResponse(cdma_cwmp::AddObjectResponse {
                instance_number: 2,
                status: 0,
            }),
            &stages,
            &[],
            false,
            true,
        );
        assert!(matches!(
            configure,
            AcsAction::Respond(ref message)
                if matches!(
                    **message,
                    CwmpMessage::SetParameterValues(ref request)
                        if request.parameter_list[0].name == address_name
                )
        ));
    }

    #[test]
    fn atomic_provisioning_stage_sends_complete_parameter_group() {
        let parameter = |name: &str| CwmpParam {
            name: name.to_string(),
            value: "configured".to_string(),
            value_from: None,
            xsd_type: "xsd:string".to_string(),
            verify: true,
        };
        let stages = vec![ProvisioningStage {
            label: "configure-pdsn".to_string(),
            atomic: true,
            replace_object: None,
            bracket_admin: None,
            params: vec![
                parameter("Pdsn.PdsnStartIpAddress"),
                parameter("Pdsn.PdsnEndIpAddress"),
                parameter("Pdsn.SecurityParameterIndex"),
                parameter("Pdsn.SecurityKey"),
            ],
        }];
        let mut session = airave_session();
        session
            .pending_parameters
            .insert("Pdsn.PdsnStartIpAddress".to_string());

        let action = session
            .send_stage(&stages, 0)
            .expect("atomic stage should be sent");
        assert!(matches!(
            action,
            AcsAction::Respond(ref message)
                if matches!(
                    **message,
                    CwmpMessage::SetParameterValues(ref request)
                        if request
                            .parameter_list
                            .iter()
                            .map(|parameter| parameter.name.as_str())
                            .eq([
                                "Pdsn.SecurityKey",
                                "Pdsn.PdsnEndIpAddress",
                                "Pdsn.PdsnStartIpAddress",
                                "Pdsn.SecurityParameterIndex",
                            ])
                )
        ));
    }

    #[test]
    fn bracketed_stage_locks_down_applies_then_restores_up() {
        fn set_values(action: &AcsAction) -> Vec<(String, String)> {
            match action {
                AcsAction::Respond(message) => match &**message {
                    CwmpMessage::SetParameterValues(request) => request
                        .parameter_list
                        .iter()
                        .map(|p| (p.name.clone(), p.value.clone()))
                        .collect(),
                    _ => Vec::new(),
                },
                _ => Vec::new(),
            }
        }
        let admin = "InternetGatewayDevice.Services.X_0005B9_DOMO.\
                     X_0005B9_is856SectorElement.sectorElementAdminStatus";
        let param = |name: &str, value: &str| CwmpParam {
            name: name.to_string(),
            value: value.to_string(),
            value_from: None,
            xsd_type: "xsd:string".to_string(),
            verify: true,
        };
        let stages = vec![ProvisioningStage {
            label: "configure-do".to_string(),
            atomic: false,
            replace_object: None,
            bracket_admin: Some(crate::config::BracketAdmin {
                parameter: admin.to_string(),
                down_value: "1".to_string(),
                up_value: "2".to_string(),
                xsd_type: "xsd:string".to_string(),
                settle_parameter: None,
                settle_operating_value: None,
                settle_polls: 20,
            }),
            params: vec![param("Sector.ColorCode", "58"), param(admin, "2")],
        }];
        let mut session = airave_session();
        session
            .pending_parameters
            .insert("Sector.ColorCode".to_string());
        session.pending_parameters.insert(admin.to_string());

        let action = session
            .send_stage(&stages, 0)
            .expect("bracketed stage sends admin down");
        assert_eq!(
            set_values(&action),
            vec![(admin.to_string(), "1".to_string())]
        );
        assert_eq!(session.bracket, Some(BracketStep::ConfigAfterDown(0)));

        let action = session.advance_bracket(BracketStep::ConfigAfterDown(0), &stages, &[], &[]);
        let names = set_values(&action)
            .into_iter()
            .map(|(name, _)| name)
            .collect::<Vec<_>>();
        assert!(names.iter().any(|name| name == "Sector.ColorCode"));
        assert!(!names.iter().any(|name| name == admin));
        assert_eq!(session.bracket, Some(BracketStep::UpAfterConfig(0)));

        let action = session.advance_bracket(BracketStep::UpAfterConfig(0), &stages, &[], &[]);
        assert_eq!(
            set_values(&action),
            vec![(admin.to_string(), "2".to_string())]
        );
        assert_eq!(session.bracket, Some(BracketStep::AdvanceAfterUp(0)));

        let _ = session.advance_bracket(BracketStep::AdvanceAfterUp(0), &stages, &[], &[]);
        assert_eq!(session.bracket, None);
    }

    const DO_ADMIN: &str = "InternetGatewayDevice.Services.X_0005B9_DOMO.\
                            X_0005B9_is856SectorElement.sectorElementAdminStatus";
    const DO_CHANNEL: &str = "InternetGatewayDevice.Services.X_0005B9_DOMO.\
                              X_0005B9_is856Carrier.is856CarChannelNumber";
    const DO_OPER: &str = "InternetGatewayDevice.Services.X_0005B9_DOMO.\
                           X_0005B9_is856SectorElement.sectorElementOperStatus";

    fn do_channel_bracket_stage(settle_polls: u32, with_settle: bool) -> Vec<ProvisioningStage> {
        let param = |name: &str, value: &str| CwmpParam {
            name: name.to_string(),
            value: value.to_string(),
            value_from: None,
            xsd_type: "xsd:string".to_string(),
            verify: true,
        };
        vec![ProvisioningStage {
            label: "configure-do".to_string(),
            atomic: false,
            replace_object: None,
            bracket_admin: Some(crate::config::BracketAdmin {
                parameter: DO_ADMIN.to_string(),
                down_value: "1".to_string(),
                up_value: "2".to_string(),
                xsd_type: "xsd:string".to_string(),
                settle_parameter: with_settle.then(|| DO_OPER.to_string()),
                settle_operating_value: with_settle.then(|| "2".to_string()),
                settle_polls,
            }),
            params: vec![param(DO_CHANNEL, "75"), param(DO_ADMIN, "2")],
        }]
    }

    fn armed_do_session(stages: &[ProvisioningStage]) -> AcsSession {
        let mut session = airave_session();
        session.pending_parameters.insert(DO_CHANNEL.to_string());
        session.pending_parameters.insert(DO_ADMIN.to_string());
        let action = session.send_stage(stages, 0).expect("sends admin down");
        assert_eq!(spv_pairs(&action), vec![(DO_ADMIN.to_string(), "1".into())]);
        session
    }

    fn spv_pairs(action: &AcsAction) -> Vec<(String, String)> {
        match action {
            AcsAction::Respond(m) => match m.as_ref() {
                CwmpMessage::SetParameterValues(spv) => spv
                    .parameter_list
                    .iter()
                    .map(|p| (p.name.clone(), p.value.clone()))
                    .collect(),
                _ => Vec::new(),
            },
            _ => Vec::new(),
        }
    }

    fn gpv_names(action: &AcsAction) -> Vec<String> {
        match action {
            AcsAction::Respond(m) => match m.as_ref() {
                CwmpMessage::GetParameterValues(g) => g.parameter_names.clone(),
                _ => Vec::new(),
            },
            _ => Vec::new(),
        }
    }

    fn oper_reading(value: &str) -> CwmpMessage {
        CwmpMessage::GetParameterValuesResponse(cdma_cwmp::GetParameterValuesResponse {
            parameters: vec![ParameterValue {
                name: DO_OPER.to_string(),
                value: value.to_string(),
                xsi_type: "xsd:string".to_string(),
            }],
        })
    }

    #[test]
    fn bracketed_config_waits_for_element_to_leave_operating_then_applies_config() {
        let stages = do_channel_bracket_stage(20, true);
        let mut s = armed_do_session(&stages);

        let a = s.step(
            &CwmpMessage::SetParameterValuesResponse(Default::default()),
            &stages,
            &[],
            false,
            true,
        );
        assert_eq!(gpv_names(&a), vec![DO_OPER.to_string()]);
        assert!(matches!(
            s.bracket,
            Some(BracketStep::AwaitElementDown(0, _))
        ));

        let a = s.step(&oper_reading("2"), &stages, &[], false, true);
        assert_eq!(gpv_names(&a), vec![DO_OPER.to_string()]);
        assert!(matches!(
            s.bracket,
            Some(BracketStep::AwaitElementDown(0, _))
        ));

        let a = s.step(&oper_reading("1"), &stages, &[], false, true);
        assert!(spv_pairs(&a).iter().any(|(n, _)| n == DO_CHANNEL));
        assert_eq!(s.bracket, Some(BracketStep::UpAfterConfig(0)));

        let a = s.step(
            &CwmpMessage::SetParameterValuesResponse(Default::default()),
            &stages,
            &[],
            false,
            true,
        );
        assert_eq!(spv_pairs(&a), vec![(DO_ADMIN.to_string(), "2".into())]);
    }

    #[test]
    fn bracketed_config_applies_after_settle_polls_are_exhausted() {
        let stages = do_channel_bracket_stage(1, true);
        let mut s = armed_do_session(&stages);
        let a = s.step(
            &CwmpMessage::SetParameterValuesResponse(Default::default()),
            &stages,
            &[],
            false,
            true,
        );
        assert_eq!(gpv_names(&a), vec![DO_OPER.to_string()]);

        let a = s.step(&oper_reading("2"), &stages, &[], false, true);
        assert_eq!(gpv_names(&a), vec![DO_OPER.to_string()]);
        let a = s.step(&oper_reading("2"), &stages, &[], false, true);
        assert!(spv_pairs(&a).iter().any(|(n, _)| n == DO_CHANNEL));
        assert_eq!(s.bracket, Some(BracketStep::UpAfterConfig(0)));
    }

    #[test]
    fn bracketed_config_without_settle_parameter_applies_immediately() {
        let stages = do_channel_bracket_stage(20, false);
        let mut s = armed_do_session(&stages);
        let a = s.step(
            &CwmpMessage::SetParameterValuesResponse(Default::default()),
            &stages,
            &[],
            false,
            true,
        );
        assert!(spv_pairs(&a).iter().any(|(n, _)| n == DO_CHANNEL));
        assert_eq!(s.bracket, Some(BracketStep::UpAfterConfig(0)));
    }

    #[test]
    fn bracketed_config_fault_restores_admin_up() {
        let stages = do_channel_bracket_stage(20, false);
        let mut s = armed_do_session(&stages);
        s.step(
            &CwmpMessage::SetParameterValuesResponse(Default::default()),
            &stages,
            &[],
            false,
            true,
        );
        let a = s.step(
            &CwmpMessage::Fault(Fault {
                fault_code: "9002".to_string(),
                fault_string: "Access denied in current sector state".to_string(),
                ..Default::default()
            }),
            &stages,
            &[],
            false,
            true,
        );
        assert_eq!(spv_pairs(&a), vec![(DO_ADMIN.to_string(), "2".into())]);
        assert_eq!(s.bracket, Some(BracketStep::EndAfterRestore));
    }

    #[test]
    fn reads_status_after_provisioning() {
        let status = vec![
            "InternetGatewayDevice.X_0005B9_GPSStatus.GPSGotFix".to_string(),
            "InternetGatewayDevice.Services.X_0005B9_OneXMO.X_0005B9_OneXRTTMO.OneXRTTOperState"
                .to_string(),
        ];
        let mut s = airave_session();
        s.step(&inform(), &prov(), &status, false, true);
        report_configuration_drift(&mut s, &[], &prov(), &status);
        let a = s.step(
            &CwmpMessage::SetParameterValuesResponse(Default::default()),
            &prov(),
            &status,
            false,
            true,
        );
        match a {
            AcsAction::Respond(m) => match *m {
                CwmpMessage::GetParameterValues(gpv) => {
                    assert_eq!(
                        gpv.parameter_names.iter().collect::<HashSet<_>>(),
                        status.iter().collect::<HashSet<_>>()
                    );
                }
                other => panic!("expected GetParameterValues, got {other:?}"),
            },
            other => panic!("expected Respond, got {other:?}"),
        }
        assert_eq!(s.phase(), AcsPhase::StatusQuerySent);
        assert_eq!(
            s.step(
                &CwmpMessage::GetParameterValuesResponse(Default::default()),
                &prov(),
                &status,
                false,
                true
            ),
            AcsAction::EndSession
        );
        assert_eq!(s.phase(), AcsPhase::Done);
    }

    #[test]
    fn runtime_credential_store_enables_airave_sip_password_readback() {
        let mut session = airave_session();
        session.set_read_sip_password(true);
        session.step(&inform(), &[], &[], false, true);
        let action = session.step(&CwmpMessage::Empty, &[], &[], false, true);
        assert!(matches!(
            action,
            AcsAction::Respond(ref message)
                if matches!(
                    **message,
                    CwmpMessage::GetParameterValues(ref request)
                        if request.parameter_names.len() == 1
                            && request.parameter_names[0].ends_with("DefaultSipPassword")
                )
        ));
    }

    #[test]
    fn issues_one_shot_reboot_when_armed() {
        let mut s = airave_session();
        s.step(&inform(), &prov(), &[], true, true);
        let a = s.step(&CwmpMessage::Empty, &prov(), &[], true, true);
        match a {
            AcsAction::Respond(m) => assert!(matches!(*m, CwmpMessage::Reboot(_))),
            other => panic!("expected Reboot, got {other:?}"),
        }
        assert_eq!(s.phase(), AcsPhase::Done);
    }

    #[test]
    fn holds_service_off_until_site_approved() {
        let mut s = airave_session();
        s.step(&inform(), &prov(), &[], false, false);
        assert_eq!(
            s.step(&CwmpMessage::Empty, &prov(), &[], false, false),
            AcsAction::EndSession
        );
    }

    #[test]
    fn value_change_does_not_repeat_provisioning() {
        let mut session = airave_session();
        session.step(
            &inform_with_event("4 VALUE CHANGE"),
            &two_stages(),
            &[],
            false,
            true,
        );

        let read = session.step(&CwmpMessage::Empty, &two_stages(), &[], false, true);
        assert!(matches!(
            read,
            AcsAction::Respond(ref message)
                if matches!(**message, CwmpMessage::GetParameterValues(_))
        ));

        let response =
            CwmpMessage::GetParameterValuesResponse(cdma_cwmp::GetParameterValuesResponse {
                parameters: vec![ParameterValue {
                    name: "A.OneXRTTMO.AdminState".to_string(),
                    value: "unlock".to_string(),
                    xsi_type: "xsd:string".to_string(),
                }],
            });
        assert_eq!(
            session.step(&response, &two_stages(), &[], false, true),
            AcsAction::EndSession
        );
    }

    #[test]
    fn lock_stage_keeps_its_static_value_despite_same_named_value_from() {
        let admin = "InternetGatewayDevice.Services.X_0005B9_OneXMO.X_0005B9_OneXRTTMO.AdminState";
        let static_param = |name: &str, value: &str| CwmpParam {
            name: name.to_string(),
            value: value.to_string(),
            value_from: None,
            xsd_type: "xsd:string".to_string(),
            verify: true,
        };
        let stages = vec![
            ProvisioningStage {
                label: "lock-onex".to_string(),
                atomic: false,
                replace_object: None,
                bracket_admin: None,
                params: vec![static_param(admin, "lock")],
            },
            ProvisioningStage {
                label: "configure".to_string(),
                atomic: true,
                replace_object: None,
                bracket_admin: None,
                params: vec![static_param("Sector.SID", "2")],
            },
            ProvisioningStage {
                label: "unlock".to_string(),
                atomic: false,
                replace_object: None,
                bracket_admin: None,
                params: vec![CwmpParam {
                    name: admin.to_string(),
                    value: String::new(),
                    value_from: Some(crate::config::CwmpValueSource::FapContext {
                        context: CwmpFapValue::SiteOneXAdminState,
                    }),
                    xsd_type: "xsd:string".to_string(),
                    verify: true,
                }],
            },
        ];
        let mut session = airave_session();
        session.fap_context = Some(FapProvisioningContext {
            feid: "0005B9CCDDEE".to_string(),
            sid: 2,
            nid: 65_535,
            band_class: 1,
            cdma_channel: 50,
            max_sector_power_dbm: None,
            sip_password: None,
            pn_offset: None,
            onex_enabled: true,
            evdo_enabled: false,
            evdo_band_class: None,
            evdo_channel: None,
            evdo_cell_id: None,
            evdo_sector_id: None,
            evdo_color_code: None,
            evdo_tx_power: None,
            evdo_pn_offset: None,
            term_auth_psk: None,
            timezone: None,
        });
        session.step(&inform(), &stages, &[], false, true);
        let action = report_configuration_drift(&mut session, &[], &stages, &[]);
        match action {
            AcsAction::Respond(message) => match *message {
                CwmpMessage::SetParameterValues(request) => {
                    let admin_state = request
                        .parameter_list
                        .iter()
                        .find(|p| p.name.ends_with("AdminState"))
                        .expect("lock stage sends AdminState");
                    assert_eq!(
                        admin_state.value, "lock",
                        "the lock stage must send lock, not the unlock stage's derived value"
                    );
                }
                other => panic!("expected the lock stage SetParameterValues, got {other:?}"),
            },
            other => panic!("expected Respond, got {other:?}"),
        }
    }

    #[test]
    fn provisioning_fault_attempts_the_unlock_stage() {
        let stages = two_stages();
        let mut session = airave_session();
        session.step(&inform(), &stages, &[], false, true);
        report_configuration_drift(&mut session, &[], &stages, &[]);

        let action = session.step(
            &CwmpMessage::Fault(Fault {
                fault_code: "9002".to_string(),
                fault_string: "Internal Error".to_string(),
                ..Default::default()
            }),
            &stages,
            &[],
            false,
            true,
        );

        match action {
            AcsAction::Respond(message) => match *message {
                CwmpMessage::SetParameterValues(request) => {
                    assert_eq!(request.parameter_list[0].value, "unlock");
                }
                other => panic!("expected SPV(unlock), got {other:?}"),
            },
            other => panic!("expected Respond, got {other:?}"),
        }
    }

    #[test]
    fn provisioning_fault_runs_all_remaining_unlock_stages() {
        let parameter = |name: &str, value: &str| CwmpParam {
            name: name.to_string(),
            value: value.to_string(),
            value_from: None,
            xsd_type: "xsd:string".to_string(),
            verify: true,
        };
        let stages = vec![
            ProvisioningStage {
                label: "lock-radio".to_string(),
                atomic: false,
                replace_object: None,
                bracket_admin: None,
                params: vec![parameter("Radio.Admin", "lock")],
            },
            ProvisioningStage {
                label: "unlock-pdsn".to_string(),
                atomic: false,
                replace_object: None,
                bracket_admin: None,
                params: vec![parameter("Pdsn.Admin", "up")],
            },
            ProvisioningStage {
                label: "unlock-radio".to_string(),
                atomic: false,
                replace_object: None,
                bracket_admin: None,
                params: vec![parameter("Radio.Admin", "unlock")],
            },
        ];
        let mut session = airave_session();
        session.step(&inform(), &stages, &[], false, true);
        report_configuration_drift(&mut session, &[], &stages, &[]);

        let action = session.step(
            &CwmpMessage::Fault(Fault {
                fault_code: "9002".to_string(),
                fault_string: "Internal Error".to_string(),
                ..Default::default()
            }),
            &stages,
            &[],
            false,
            true,
        );
        assert!(matches!(
            action,
            AcsAction::Respond(ref message)
                if matches!(
                    **message,
                    CwmpMessage::SetParameterValues(ref request)
                        if request.parameter_list[0].name == "Pdsn.Admin"
                )
        ));

        let action = session.step(
            &CwmpMessage::SetParameterValuesResponse(Default::default()),
            &stages,
            &[],
            false,
            true,
        );
        assert!(matches!(
            action,
            AcsAction::Respond(ref message)
                if matches!(
                    **message,
                    CwmpMessage::SetParameterValues(ref request)
                        if request.parameter_list[0].name == "Radio.Admin"
                            && request.parameter_list[0].value == "unlock"
                )
        ));
    }

    #[test]
    fn creates_configured_object_before_parameter_stages() {
        let objects = vec![CwmpObject {
            name: "InternetGatewayDevice.Services.UserProfile.".to_string(),
            next_level: false,
            required_instance: None,
            existence_probe_leaf: None,
        }];
        let stages = two_stages();
        let mut session = airave_session();
        session.step_with_objects(&inform(), &objects, &stages, &[], &[], false, true);

        let action = session.step_with_objects(
            &CwmpMessage::Empty,
            &objects,
            &stages,
            &[],
            &[],
            false,
            true,
        );
        match action {
            AcsAction::Respond(message) => match *message {
                CwmpMessage::GetParameterNames(request) => {
                    assert_eq!(request.parameter_path, objects[0].name);
                    assert_eq!(request.next_level, objects[0].next_level);
                }
                other => panic!("expected object probe, got {other:?}"),
            },
            other => panic!("expected Respond, got {other:?}"),
        }

        let action = session.step_with_objects(
            &CwmpMessage::Fault(Fault {
                fault_code: "9005".to_string(),
                fault_string: "Invalid Parameter Name".to_string(),
                ..Default::default()
            }),
            &objects,
            &stages,
            &[],
            &[],
            false,
            true,
        );
        match action {
            AcsAction::Respond(message) => match *message {
                CwmpMessage::AddObject(request) => {
                    assert_eq!(request.object_name, objects[0].name);
                }
                other => panic!("expected AddObject, got {other:?}"),
            },
            other => panic!("expected Respond, got {other:?}"),
        }

        let action = session.step_with_objects(
            &CwmpMessage::AddObjectResponse(cdma_cwmp::AddObjectResponse {
                instance_number: 0,
                status: 0,
            }),
            &objects,
            &stages,
            &[],
            &[],
            false,
            true,
        );
        match action {
            AcsAction::Respond(message) => match *message {
                CwmpMessage::GetParameterNames(request) => {
                    assert_eq!(request.parameter_path, objects[0].name);
                }
                other => panic!("expected object discovery, got {other:?}"),
            },
            other => panic!("expected Respond, got {other:?}"),
        }

        let action = session.step_with_objects(
            &CwmpMessage::GetParameterNamesResponse(cdma_cwmp::GetParameterNamesResponse {
                parameters: vec![cdma_cwmp::ParameterInfo {
                    name: format!("{}1.Impi", objects[0].name),
                    writable: true,
                }],
            }),
            &objects,
            &stages,
            &[],
            &[],
            false,
            true,
        );
        match action {
            AcsAction::Respond(message) => match *message {
                CwmpMessage::GetParameterValues(request) => {
                    assert_eq!(request.parameter_names.len(), 1);
                }
                other => panic!("expected configuration query, got {other:?}"),
            },
            other => panic!("expected Respond, got {other:?}"),
        }

        let action = session.step_with_objects(
            &CwmpMessage::GetParameterValuesResponse(Default::default()),
            &objects,
            &stages,
            &[],
            &[],
            false,
            true,
        );
        match action {
            AcsAction::Respond(message) => match *message {
                CwmpMessage::SetParameterValues(request) => {
                    assert_eq!(request.parameter_list[0].value, "lock");
                }
                other => panic!("expected first provisioning stage, got {other:?}"),
            },
            other => panic!("expected Respond, got {other:?}"),
        }
    }

    #[test]
    fn creates_required_instance_when_other_instances_exist() {
        let objects = vec![CwmpObject {
            name: "InternetGatewayDevice.Services.Pdsn.Entry.".to_string(),
            next_level: true,
            required_instance: Some(2),
            existence_probe_leaf: None,
        }];
        let mut session = airave_session();
        session.step_with_objects(&inform(), &objects, &[], &[], &[], false, true);
        session.step_with_objects(&CwmpMessage::Empty, &objects, &[], &[], &[], false, true);

        let action = session.step_with_objects(
            &CwmpMessage::GetParameterNamesResponse(cdma_cwmp::GetParameterNamesResponse {
                parameters: vec![cdma_cwmp::ParameterInfo {
                    name: "InternetGatewayDevice.Services.Pdsn.Entry.1.".to_string(),
                    writable: true,
                }],
            }),
            &objects,
            &[],
            &[],
            &[],
            false,
            true,
        );
        assert!(matches!(
            action,
            AcsAction::Respond(ref message)
                if matches!(**message, CwmpMessage::AddObject(_))
        ));
    }

    #[test]
    fn existing_object_is_verified_before_stages_are_skipped() {
        let object = CwmpObject {
            name: "InternetGatewayDevice.Services.Pdsn.Entry.".to_string(),
            next_level: true,
            required_instance: None,
            existence_probe_leaf: None,
        };
        let parameter = CwmpParam {
            name: format!("{}0.Address", object.name),
            value: "10.4.58.140".to_string(),
            value_from: None,
            xsd_type: "xsd:string".to_string(),
            verify: true,
        };
        let stages = vec![ProvisioningStage {
            label: "configure".to_string(),
            atomic: false,
            replace_object: None,
            bracket_admin: None,
            params: vec![parameter.clone()],
        }];
        let objects = vec![object.clone()];
        let mut session = airave_session();
        session.step_with_objects(&inform(), &objects, &stages, &[], &[], false, true);

        let action = session.step_with_objects(
            &CwmpMessage::Empty,
            &objects,
            &stages,
            &[],
            &[],
            false,
            true,
        );
        assert!(matches!(
            action,
            AcsAction::Respond(ref message)
                if matches!(**message, CwmpMessage::GetParameterNames(_))
        ));
        if let AcsAction::Respond(message) = action {
            if let CwmpMessage::GetParameterNames(request) = *message {
                assert!(request.next_level);
            }
        }

        let action = session.step_with_objects(
            &CwmpMessage::GetParameterNamesResponse(cdma_cwmp::GetParameterNamesResponse {
                parameters: vec![cdma_cwmp::ParameterInfo {
                    name: parameter.name.clone(),
                    writable: true,
                }],
            }),
            &objects,
            &stages,
            &[],
            &[],
            false,
            true,
        );
        match action {
            AcsAction::Respond(message) => match *message {
                CwmpMessage::GetParameterValues(request) => {
                    assert_eq!(request.parameter_names, vec![parameter.name.clone()]);
                }
                other => panic!("expected configuration query, got {other:?}"),
            },
            other => panic!("expected Respond, got {other:?}"),
        }

        let action = session.step_with_objects(
            &CwmpMessage::GetParameterValuesResponse(cdma_cwmp::GetParameterValuesResponse {
                parameters: vec![ParameterValue {
                    name: parameter.name,
                    value: parameter.value,
                    xsi_type: parameter.xsd_type,
                }],
            }),
            &objects,
            &stages,
            &[],
            &[],
            false,
            true,
        );
        assert_eq!(action, AcsAction::EndSession);
        assert!(session.configuration_verified());
    }

    #[test]
    fn write_only_parameters_are_not_read_during_verification() {
        let mut stages = two_stages();
        stages[0].params[0].verify = false;
        let mut session = airave_session();
        session.step(&inform(), &stages, &[], false, true);

        let action = session.step(&CwmpMessage::Empty, &stages, &[], false, true);
        match action {
            AcsAction::Respond(message) => match *message {
                CwmpMessage::GetParameterValues(request) => {
                    assert_eq!(
                        request.parameter_names,
                        vec![stages[1].params[0].name.clone()]
                    );
                }
                other => panic!("expected configuration query, got {other:?}"),
            },
            other => panic!("expected Respond, got {other:?}"),
        }
    }

    #[test]
    fn recursively_discovers_configured_objects() {
        let discovery = vec![CwmpObject {
            name: "InternetGatewayDevice.Services.".to_string(),
            next_level: false,
            required_instance: None,
            existence_probe_leaf: None,
        }];
        let mut session = airave_session();
        session.step_with_objects(&inform(), &[], &[], &discovery, &[], false, true);

        let action =
            session.step_with_objects(&CwmpMessage::Empty, &[], &[], &discovery, &[], false, true);
        match action {
            AcsAction::Respond(message) => match *message {
                CwmpMessage::GetParameterNames(request) => {
                    assert_eq!(request.parameter_path, discovery[0].name);
                    assert!(!request.next_level);
                }
                other => panic!("expected object discovery, got {other:?}"),
            },
            other => panic!("expected Respond, got {other:?}"),
        }

        let action = session.step_with_objects(
            &CwmpMessage::GetParameterNamesResponse(cdma_cwmp::GetParameterNamesResponse {
                parameters: vec![cdma_cwmp::ParameterInfo {
                    name: "InternetGatewayDevice.Services.Example.".to_string(),
                    writable: true,
                }],
            }),
            &[],
            &[],
            &discovery,
            &[],
            false,
            true,
        );
        assert_eq!(action, AcsAction::EndSession);
        assert_eq!(session.phase(), AcsPhase::Done);
    }
    #[test]
    fn a11_keys_are_reapplied_when_spi_matches_and_after_key_rotation() {
        let mut stages = vec![ProvisioningStage {
            label: "pdsn".to_string(),
            atomic: true,
            replace_object: None,
            bracket_admin: None,
            params: vec![
                CwmpParam {
                    name: "Pdsn.1.SecurityParameterIndex".to_string(),
                    value: String::new(),
                    value_from: Some(CwmpValueSource::FapContext {
                        context: CwmpFapValue::A11Spi,
                    }),
                    xsd_type: "xsd:unsignedInt".to_string(),
                    verify: true,
                },
                CwmpParam {
                    name: "Pdsn.1.SecurityKey".to_string(),
                    value: String::new(),
                    value_from: Some(CwmpValueSource::FapContext {
                        context: CwmpFapValue::A11SharedSecretHex,
                    }),
                    xsd_type: "xsd:string".to_string(),
                    verify: true,
                },
            ],
        }];
        stages.insert(
            0,
            ProvisioningStage {
                label: "replace-pdsn".to_string(),
                atomic: false,
                replace_object: Some(CwmpObject {
                    name: "Pdsn.".to_string(),
                    existence_probe_leaf: None,
                    next_level: true,
                    required_instance: Some(1),
                }),
                bracket_admin: None,
                params: vec![],
            },
        );
        for secret in ["00112233", "aabbccdd"] {
            let mut session = airave_session();
            session.a11_provisioning = Some(crate::config::A11Provisioning(
                cdma_a11::A11SecurityConfig {
                    spi: 256,
                    shared_secret_hex: secret.to_string(),
                },
            ));
            session.step(&inform(), &stages, &[], false, true);
            session.step(&CwmpMessage::Empty, &stages, &[], false, true);
            let response =
                CwmpMessage::GetParameterValuesResponse(cdma_cwmp::GetParameterValuesResponse {
                    parameters: vec![ParameterValue {
                        name: "Pdsn.1.SecurityParameterIndex".to_string(),
                        value: "256".to_string(),
                        xsi_type: "xsd:unsignedInt".to_string(),
                    }],
                });
            let AcsAction::Respond(message) = session.step(&response, &stages, &[], false, true)
            else {
                panic!("expected key write despite matching SPI");
            };
            let CwmpMessage::SetParameterValues(request) = *message else {
                panic!("expected SetParameterValues");
            };
            assert_eq!(
                request
                    .parameter_list
                    .iter()
                    .map(|p| (p.name.as_str(), p.value.as_str()))
                    .collect::<Vec<_>>(),
                vec![
                    ("Pdsn.1.SecurityKey", secret),
                    ("Pdsn.1.SecurityParameterIndex", "256")
                ]
            );
            session.step(
                &CwmpMessage::SetParameterValuesResponse(Default::default()),
                &stages,
                &[],
                false,
                true,
            );
            assert_eq!(session.phase(), AcsPhase::Done);
        }
        let mut session = airave_session();
        session.step(&inform(), &stages, &[], false, true);
        session.step(&CwmpMessage::Empty, &stages, &[], false, true);
        let action = session.step(
            &CwmpMessage::GetParameterValuesResponse(Default::default()),
            &stages,
            &[],
            false,
            true,
        );
        assert_eq!(action, AcsAction::EndSession);
        assert!(!session.configuration_verified());
    }
}
