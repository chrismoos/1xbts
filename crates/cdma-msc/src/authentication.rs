//! MSC subscriber authentication policy over the HLR Authentication Center.
//!
//! The base station forwards the mobile's global-challenge response in the A1
//! Location Updating Request, CM Service Request or Paging Response
//! (A.S0014-D §4.2.33-§4.2.37, §4.2.61, §4.2.62). The MSC asks the HLR to
//! verify it and applies the configured mode.

use cdma_hlr::auth::{CallKeyMaterial, CaveAuthOutcome, GlobalChallenge};
use cdma_hlr::model::MobileIdentityKey;
use cdma_hlr::repository::HlrRepository;
use log::{info, warn};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AuthenticationMode {
    Off,
    #[default]
    Monitor,
    Enforce,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SsdUpdatePolicy {
    Never,
    /// Update when the HLR holds an A-key but no SSD, or when the call's AUTHR failed.
    #[default]
    WhenNeeded,
    EveryCall,
}

/// Voice privacy policy applied only when authenticated call keys are available.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum VoicePrivacyPolicy {
    /// Preload authenticated keys and enable privacy only on handset request.
    #[default]
    Optional,
    /// Withhold private masks and refuse handset privacy requests.
    Never,
    /// Request privacy on authenticated voice calls, subject to handset acceptance.
    Always,
}

impl VoicePrivacyPolicy {
    fn config_name(self) -> &'static str {
        match self {
            Self::Optional => "optional",
            Self::Never => "never",
            Self::Always => "always",
        }
    }

    pub(crate) fn encryption_information(
        self,
        keys: Option<&CallKeyMaterial>,
        handset_requested: bool,
    ) -> Option<cdma_ios::EncryptionInformation> {
        if self == Self::Never {
            return None;
        }
        let keys = keys?;
        Some(
            cdma_ios::EncryptionInformation::private_long_code(
                keys.private_long_code_mask,
                self == Self::Always || handset_requested,
            )
            .expect("CAVE produces a 42-bit private long code mask"),
        )
    }
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct AuthenticationConfig {
    pub mode: AuthenticationMode,
    pub voice_privacy: VoicePrivacyPolicy,
    /// Under `enforce`, serve mobiles that have no CAVE credentials in the HLR.
    pub allow_unprovisioned: bool,
    /// SSD update with base station challenge on an authenticated call's traffic channel.
    pub ssd_update: SsdUpdatePolicy,
    /// Challenges every authenticated call when enabled. SSD updates and privacy activation
    /// require a unique challenge even when this is disabled.
    pub unique_challenge_on_call: bool,
    /// Parameter Update Order, which increments COUNT, on every authenticated call.
    pub count_update_on_call: bool,
    /// Time the MSC waits for each reply of a traffic-channel procedure.
    pub procedure_timeout_ms: u64,
}

const DEFAULT_PROCEDURE_TIMEOUT_MS: u64 = 10_000;

impl AuthenticationMode {
    pub fn config_name(self) -> &'static str {
        match self {
            Self::Off => "off",
            Self::Monitor => "monitor",
            Self::Enforce => "enforce",
        }
    }
}

impl SsdUpdatePolicy {
    pub fn config_name(self) -> &'static str {
        match self {
            Self::Never => "never",
            Self::WhenNeeded => "when_needed",
            Self::EveryCall => "every_call",
        }
    }
}

impl std::fmt::Display for AuthenticationConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "mode={} voice_privacy={} allow_unprovisioned={} ssd_update={} unique_challenge_on_call={} count_update_on_call={} procedure_timeout_ms={}",
            self.mode.config_name(),
            self.voice_privacy.config_name(),
            self.allow_unprovisioned,
            self.ssd_update.config_name(),
            self.unique_challenge_on_call,
            self.count_update_on_call,
            self.procedure_timeout_ms,
        )
    }
}

impl Default for AuthenticationConfig {
    fn default() -> Self {
        Self {
            mode: AuthenticationMode::Monitor,
            voice_privacy: VoicePrivacyPolicy::Optional,
            allow_unprovisioned: false,
            ssd_update: SsdUpdatePolicy::WhenNeeded,
            unique_challenge_on_call: true,
            count_update_on_call: true,
            procedure_timeout_ms: DEFAULT_PROCEDURE_TIMEOUT_MS,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthenticationProcedure {
    Registration,
    Origination,
    PageResponse,
    DataBurst,
}

impl AuthenticationProcedure {
    fn label(self) -> &'static str {
        match self {
            Self::Registration => "registration",
            Self::Origination => "origination",
            Self::PageResponse => "page response",
            Self::DataBurst => "SMS",
        }
    }

    /// Key_VPM_Generation needs the saved registers of the authentication that sets up the call.
    fn sets_up_call(self) -> bool {
        matches!(self, Self::Origination | Self::PageResponse)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthenticationResult {
    NotRequested,
    Success,
    SignatureMismatch,
    CountMismatch,
    NotProvisioned,
    ParametersMissing,
    RandMismatch,
    HlrError,
}

impl AuthenticationResult {
    pub fn label(self) -> &'static str {
        match self {
            Self::NotRequested => "not requested",
            Self::Success => "success",
            Self::SignatureMismatch => "AUTHR mismatch",
            Self::CountMismatch => "COUNT mismatch",
            Self::NotProvisioned => "no CAVE credentials",
            Self::ParametersMissing => "authentication parameters missing",
            Self::RandMismatch => "RANDC matches no broadcast RAND",
            Self::HlrError => "HLR error",
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AuthenticationDecision {
    pub procedure: AuthenticationProcedure,
    pub result: AuthenticationResult,
    pub allow: bool,
    pub call_keys: Option<CallKeyMaterial>,
    /// Service is allowed without a verified signature. A call must run an SSD
    /// update and unique challenge before it connects.
    pub pending_ssd_update: bool,
    pub inputs: Option<ChallengeInputs>,
}

/// Where AUTH_DATA came from (C.S0004-E §2.1.1.1.2.2).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AuthDataSource {
    ImsiS1,
    DialedDigits,
    DialedDigitsZeroFilled,
    BaseStation,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ChallengeInputs {
    pub rand: u32,
    pub auth_data: u32,
    pub auth_data_source: AuthDataSource,
    pub count: Option<u8>,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GlobalChallengeIes {
    pub authr: Option<u32>,
    pub randc: Option<u8>,
    pub count: Option<u8>,
    pub rand: Option<u32>,
    pub event: Option<u8>,
    pub auth_data: Option<u32>,
}

impl GlobalChallengeIes {
    fn new(
        response: Option<cdma_ios::AuthenticationResponseParameter>,
        randc: Option<cdma_ios::AuthenticationConfirmationParameter>,
        count: Option<cdma_ios::AuthenticationParameterCount>,
        challenge: Option<cdma_ios::AuthenticationChallengeParameter>,
        event: Option<cdma_ios::AuthenticationEvent>,
    ) -> Self {
        Self {
            authr: response
                .filter(|r| r.signature_type() == Some(cdma_ios::AuthSignatureType::Authr))
                .map(|r| r.signature()),
            randc: randc.map(|r| r.0),
            count: count.map(|c| c.0 & COUNT_MASK),
            rand: challenge.and_then(|c| c.rand()),
            event: event.map(|e| e.0),
            auth_data: None,
        }
    }

    pub fn from_location_updating_request(m: &cdma_ios::LocationUpdatingRequestMessage) -> Self {
        Self::new(
            m.authentication_response_parameter,
            m.authentication_confirmation_parameter,
            m.authentication_parameter_count,
            m.authentication_challenge_parameter,
            m.authentication_event,
        )
    }

    pub fn from_cm_service_request(m: &cdma_ios::CmServiceRequestMessage) -> Self {
        Self {
            auth_data: m
                .authentication_data
                .map(|d| u32::from_be_bytes([0, d.0[0], d.0[1], d.0[2]])),
            ..Self::new(
                m.authentication_response_parameter,
                m.authentication_confirmation_parameter,
                m.authentication_parameter_count,
                m.authentication_challenge_parameter,
                m.authentication_event,
            )
        }
    }

    pub fn from_adds_transfer(m: &cdma_ios::AddsTransferMessage) -> Self {
        Self {
            auth_data: m
                .authentication_data
                .map(|d| u32::from_be_bytes([0, d.0[0], d.0[1], d.0[2]])),
            ..Self::new(
                m.authentication_response_parameter,
                m.authentication_confirmation_parameter,
                m.authentication_parameter_count,
                m.authentication_challenge_parameter,
                m.authentication_event,
            )
        }
    }

    pub fn from_paging_response(m: &cdma_ios::PagingResponseMessage) -> Self {
        Self::new(
            m.authentication_response_parameter,
            m.authentication_confirmation_parameter,
            m.authentication_parameter_count,
            m.authentication_challenge_parameter,
            m.authentication_event,
        )
    }

    fn requested(&self) -> bool {
        self.authr.is_some() || self.event.is_some()
    }
}

/// COUNT is 6 bits (A.S0014-D §4.2.37).
const COUNT_MASK: u8 = 0b0011_1111;
const RANDC_SHIFT: u32 = 24;

pub struct AuthenticationSubject<'a> {
    pub identity: Option<&'a MobileIdentityKey>,
    pub imsi: Option<&'a str>,
    pub esn: Option<u32>,
    /// Dialed digits of an origination, or the destination address of an SMS
    /// (C.S0015-B §3.3.3.1), used when the BS omits Authentication Data.
    pub dialed_digits: Option<&'a str>,
}

fn imsi_s1(imsi: &str) -> Option<u32> {
    cdma_common::paging::imsi_s_from_imsi(imsi).map(|(s1, _)| s1)
}

pub async fn authenticate(
    config: &AuthenticationConfig,
    hlr: &dyn HlrRepository,
    procedure: AuthenticationProcedure,
    subject: &AuthenticationSubject<'_>,
    ies: &GlobalChallengeIes,
) -> AuthenticationDecision {
    let decide = |result: AuthenticationResult,
                  call_keys: Option<CallKeyMaterial>,
                  inputs: Option<ChallengeInputs>,
                  pending_ssd_update: bool| {
        let allow = pending_ssd_update
            || match result {
                AuthenticationResult::NotRequested | AuthenticationResult::Success => true,
                AuthenticationResult::NotProvisioned => {
                    config.mode != AuthenticationMode::Enforce || config.allow_unprovisioned
                }
                _ => config.mode != AuthenticationMode::Enforce,
            };
        AuthenticationDecision {
            procedure,
            result,
            allow,
            call_keys,
            pending_ssd_update,
            inputs,
        }
    };
    if config.mode == AuthenticationMode::Off || !ies.requested() {
        return decide(AuthenticationResult::NotRequested, None, None, false);
    }
    let (result, call_keys, inputs) = match verify(hlr, procedure, subject, ies).await {
        Ok(verified) => verified,
        Err(result) => (result, None, None),
    };
    let pending_ssd_update = config.mode == AuthenticationMode::Enforce
        && result == AuthenticationResult::SignatureMismatch
        && ssd_update_can_recover(hlr, procedure, subject).await;
    let decision = decide(result, call_keys, inputs, pending_ssd_update);
    let who = subject.imsi.unwrap_or("unknown IMSI");
    if decision.result == AuthenticationResult::Success {
        info!(
            "MSC: authentication {} for {} {}",
            decision.result.label(),
            procedure.label(),
            who
        );
    } else {
        warn!(
            "MSC: authentication failed for {} {}: {} ({})",
            procedure.label(),
            who,
            decision.result.label(),
            if decision.pending_ssd_update {
                "service allowed pending an SSD update"
            } else if decision.allow {
                "service allowed"
            } else {
                "service rejected"
            }
        );
    }
    decision
}

/// Whether a mobile that failed the global challenge under `enforce` keeps service
/// until an SSD update settles it. X.S0004-640 leaves the choice between denying
/// access and starting an SSD update to the authentication center. IOS runs SSD
/// update only on a traffic channel (A.S0013-D Table 3.20.1-1), so a call gets its
/// channel and connects only after the update and a unique challenge pass. A
/// registration, which has no traffic channel, is accepted only while the HLR holds
/// an A-key without SSD, so the mobile can still originate the call that sets one.
async fn ssd_update_can_recover(
    hlr: &dyn HlrRepository,
    procedure: AuthenticationProcedure,
    subject: &AuthenticationSubject<'_>,
) -> bool {
    let Some(identity) = subject.identity else {
        return false;
    };
    let state = match hlr.get_subscriber_auth_state(identity).await {
        Ok(Some(state)) if state.credentials_provisioned => state,
        Ok(_) => return false,
        Err(error) => {
            warn!("MSC: HLR auth state lookup failed: {error}");
            return false;
        }
    };
    match procedure {
        AuthenticationProcedure::Registration => !state.ssd_provisioned,
        AuthenticationProcedure::Origination | AuthenticationProcedure::PageResponse => true,
        AuthenticationProcedure::DataBurst => false,
    }
}

/// Dialed digits AUTH_DATA holds (C.S0004-E §2.1.1.1.2.2).
const DIALED_AUTH_DATA_DIGITS: usize = 6;

/// AUTH_DATA for fewer than six dialed digits with zeros in the nibbles C.S0004-E
/// §2.1.1.1.2.2 fills from IMSI_S1. Some mobiles fill them with zeros, so a failed
/// origination or SMS is checked once more with this value.
fn zero_filled_auth_data(
    procedure: AuthenticationProcedure,
    subject: &AuthenticationSubject<'_>,
) -> Option<u32> {
    if !matches!(
        procedure,
        AuthenticationProcedure::Origination | AuthenticationProcedure::DataBurst
    ) {
        return None;
    }
    let digits = subject.dialed_digits?;
    if digits.is_empty() || digits.chars().count() >= DIALED_AUTH_DATA_DIGITS {
        return None;
    }
    cdma_cave::AuthData::Dialed {
        imsi_s1: 0,
        digits: digits.to_string(),
    }
    .to_u24()
    .ok()
}

async fn verify(
    hlr: &dyn HlrRepository,
    procedure: AuthenticationProcedure,
    subject: &AuthenticationSubject<'_>,
    ies: &GlobalChallengeIes,
) -> Result<
    (
        AuthenticationResult,
        Option<CallKeyMaterial>,
        Option<ChallengeInputs>,
    ),
    AuthenticationResult,
> {
    if ies.event == Some(cdma_ios::AUTHENTICATION_EVENT_RANDC_MISMATCH) {
        return Err(AuthenticationResult::RandMismatch);
    }
    let (Some(authr), Some(randc)) = (ies.authr, ies.randc) else {
        return Err(AuthenticationResult::ParametersMissing);
    };
    let rand = ies.rand.ok_or(AuthenticationResult::RandMismatch)?;
    if (rand >> RANDC_SHIFT) as u8 != randc {
        return Err(AuthenticationResult::RandMismatch);
    }
    let (Some(identity), Some(esn), Some(imsi)) = (subject.identity, subject.esn, subject.imsi)
    else {
        return Err(AuthenticationResult::ParametersMissing);
    };
    let imsi_s1 = imsi_s1(imsi).ok_or(AuthenticationResult::ParametersMissing)?;
    let (auth_data, mut auth_data_source) = match procedure {
        AuthenticationProcedure::Origination | AuthenticationProcedure::DataBurst => {
            match ies.auth_data {
                Some(auth_data) => (auth_data, AuthDataSource::BaseStation),
                None => (
                    cdma_cave::AuthData::Dialed {
                        imsi_s1,
                        digits: subject.dialed_digits.unwrap_or_default().to_string(),
                    }
                    .to_u24()
                    .map_err(|_| AuthenticationResult::ParametersMissing)?,
                    AuthDataSource::DialedDigits,
                ),
            }
        }
        _ => (imsi_s1, AuthDataSource::ImsiS1),
    };
    let challenge = GlobalChallenge {
        esn,
        rand,
        authr,
        count: ies.count,
        auth_data,
        generate_call_keys: procedure.sets_up_call(),
        otasp_reauthentication: false,
    };
    let mut verdict = hlr.verify_cave_global_challenge(identity, &challenge).await;
    if let (Ok(v), Some(zero_filled)) = (&verdict, zero_filled_auth_data(procedure, subject))
        && v.outcome == CaveAuthOutcome::SignatureMismatch
        && zero_filled != challenge.auth_data
    {
        let retry = GlobalChallenge {
            auth_data: zero_filled,
            ..challenge
        };
        let retried = hlr.verify_cave_global_challenge(identity, &retry).await;
        if retried
            .as_ref()
            .is_ok_and(|v| v.outcome == CaveAuthOutcome::Success)
        {
            info!(
                "MSC: {} {imsi} verified with zero-filled AUTH_DATA=0x{zero_filled:06X}. The mobile does not fill short dialed digits from IMSI_S1 (C.S0004-E 2.1.1.1.2.2).",
                procedure.label(),
            );
            verdict = retried;
            auth_data_source = AuthDataSource::DialedDigitsZeroFilled;
        }
    }
    let inputs = ChallengeInputs {
        rand,
        auth_data: if auth_data_source == AuthDataSource::DialedDigitsZeroFilled {
            zero_filled_auth_data(procedure, subject).unwrap_or(auth_data)
        } else {
            auth_data
        },
        auth_data_source,
        count: ies.count,
    };
    if let Ok(v) = &verdict
        && v.outcome == CaveAuthOutcome::SignatureMismatch
    {
        warn!(
            "MSC: AUTHR inputs for {} {imsi}: RAND=0x{:08X} AUTH_DATA=0x{:06X} ESN=0x{:08X} AUTHR=0x{:05X} COUNT={:?}",
            procedure.label(),
            challenge.rand,
            challenge.auth_data,
            challenge.esn,
            challenge.authr,
            challenge.count,
        );
    }
    match verdict {
        Ok(verdict) => Ok((
            match verdict.outcome {
                CaveAuthOutcome::Success => AuthenticationResult::Success,
                CaveAuthOutcome::SignatureMismatch => AuthenticationResult::SignatureMismatch,
                CaveAuthOutcome::CountMismatch => AuthenticationResult::CountMismatch,
                CaveAuthOutcome::NotProvisioned => AuthenticationResult::NotProvisioned,
            },
            verdict.call_keys,
            Some(inputs),
        )),
        Err(error) if error.contains("identity not found") => {
            Err(AuthenticationResult::NotProvisioned)
        }
        Err(error) => {
            warn!("MSC: HLR global-challenge verification failed: {error}");
            Err(AuthenticationResult::HlrError)
        }
    }
}

use crate::grpc::events_proto::v1 as events_proto;

impl AuthenticationProcedure {
    fn to_proto(self) -> events_proto::AuthenticationProcedure {
        match self {
            Self::Registration => events_proto::AuthenticationProcedure::Registration,
            Self::Origination => events_proto::AuthenticationProcedure::Origination,
            Self::PageResponse => events_proto::AuthenticationProcedure::PageResponse,
            Self::DataBurst => events_proto::AuthenticationProcedure::DataBurst,
        }
    }
}

impl AuthDataSource {
    fn to_proto(self) -> events_proto::AuthDataSource {
        match self {
            Self::ImsiS1 => events_proto::AuthDataSource::ImsiS1,
            Self::DialedDigits => events_proto::AuthDataSource::DialedDigits,
            Self::DialedDigitsZeroFilled => events_proto::AuthDataSource::DialedDigitsZeroFilled,
            Self::BaseStation => events_proto::AuthDataSource::BaseStation,
        }
    }
}

impl AuthenticationResult {
    fn to_proto(self) -> Option<events_proto::AuthenticationResult> {
        Some(match self {
            Self::NotRequested => return None,
            Self::Success => events_proto::AuthenticationResult::Success,
            Self::SignatureMismatch => events_proto::AuthenticationResult::SignatureMismatch,
            Self::CountMismatch => events_proto::AuthenticationResult::CountMismatch,
            Self::NotProvisioned => events_proto::AuthenticationResult::NotProvisioned,
            Self::ParametersMissing => events_proto::AuthenticationResult::ParametersMissing,
            Self::RandMismatch => events_proto::AuthenticationResult::RandMismatch,
            Self::HlrError => events_proto::AuthenticationResult::HlrError,
        })
    }
}

pub(crate) fn authentication_event(
    procedure: events_proto::AuthenticationProcedure,
    result: events_proto::AuthenticationResult,
    service_rejected: bool,
    imsi: Option<&str>,
    esn: Option<u32>,
    base_station: &str,
) -> events_proto::MscNetworkEvent {
    let now = std::time::SystemTime::now();
    events_proto::MscNetworkEvent {
        event: Some(events_proto::msc_network_event::Event::Authentication(
            events_proto::AuthenticationEvent {
                time: Some(prost_types::Timestamp::from(now)),
                procedure: procedure as i32,
                result: result as i32,
                service_rejected,
                imsi: imsi.unwrap_or_default().to_string(),
                esn,
                base_station: base_station.to_string(),
                ..Default::default()
            },
        )),
    }
}

impl AuthenticationDecision {
    /// `None` when no authentication was requested.
    pub(crate) fn to_event(
        &self,
        subject: &AuthenticationSubject<'_>,
        base_station: &str,
    ) -> Option<events_proto::MscNetworkEvent> {
        let mut event = authentication_event(
            self.procedure.to_proto(),
            self.result.to_proto()?,
            !self.allow,
            subject.imsi,
            subject.esn,
            base_station,
        );
        if let Some(events_proto::msc_network_event::Event::Authentication(auth)) =
            event.event.as_mut()
        {
            auth.pending_ssd_update = self.pending_ssd_update;
            if let Some(inputs) = self.inputs {
                auth.rand = Some(inputs.rand);
                auth.auth_data = Some(inputs.auth_data);
                auth.auth_data_source = inputs.auth_data_source.to_proto() as i32;
                auth.count = inputs.count.map(u32::from);
            }
        }
        Some(event)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CallAuthContext {
    pub identity: MobileIdentityKey,
    pub imsi: String,
    pub esn: u32,
    pub global_result: AuthenticationResult,
}

impl CallAuthContext {
    pub(crate) fn from_subject(
        subject: &AuthenticationSubject<'_>,
        decision: &AuthenticationDecision,
    ) -> Option<Self> {
        if decision.result == AuthenticationResult::NotRequested {
            return None;
        }
        Some(Self {
            identity: subject.identity?.clone(),
            imsi: subject.imsi?.to_string(),
            esn: subject.esn?,
            global_result: decision.result,
        })
    }

    fn imsi_s(&self) -> Option<(u32, u16)> {
        cdma_common::paging::imsi_s_from_imsi(&self.imsi)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TrafficProcedure {
    SsdUpdate,
    UniqueChallenge,
    CountUpdate,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum TrafficAwaiting {
    BaseStationChallenge,
    SsdUpdateResponse,
    AuthenticationResponse { randu: u32 },
    ParameterUpdateConfirm,
}

impl TrafficAwaiting {
    fn procedure(self) -> events_proto::AuthenticationProcedure {
        match self {
            Self::BaseStationChallenge | Self::SsdUpdateResponse => {
                events_proto::AuthenticationProcedure::SsdUpdate
            }
            Self::AuthenticationResponse { .. } => {
                events_proto::AuthenticationProcedure::UniqueChallenge
            }
            Self::ParameterUpdateConfirm => events_proto::AuthenticationProcedure::CountUpdate,
        }
    }
}

/// The traffic-channel authentication procedures of one call, run one at a time.
#[derive(Debug)]
pub(crate) struct TrafficAuthentication {
    pub context: CallAuthContext,
    pub base_station: String,
    pending: std::collections::VecDeque<TrafficProcedure>,
    pub awaiting: Option<TrafficAwaiting>,
    pub deadline: tokio::time::Instant,
}

/// RANDU is 24 bits (C.S0005-E §2.3.12.1.4).
const RANDU_MASK: u32 = (1 << cdma_cave::RANDU_BITS) - 1;
/// A.S0014-D Table 4.2.35-1 Random Number Type for RANDSSD.
const RANDOM_NUMBER_TYPE_RANDSSD: u8 = 0b0100;

#[derive(Debug, PartialEq, Eq)]
pub(crate) enum TrafficAction {
    Send(cdma_ios::MessageType, Vec<u8>),
    Report {
        procedure: events_proto::AuthenticationProcedure,
        result: events_proto::AuthenticationResult,
        /// Clear the call because a unique challenge failed under `enforce`.
        clear_call: bool,
    },
    Done,
}

impl TrafficAuthentication {
    /// `ssd_unset` is true when the HLR holds an A-key without SSD.
    pub(crate) fn plan(
        config: &AuthenticationConfig,
        context: CallAuthContext,
        base_station: String,
        ssd_unset: bool,
    ) -> Option<Self> {
        let failed = matches!(
            context.global_result,
            AuthenticationResult::SignatureMismatch | AuthenticationResult::CountMismatch
        );
        let ssd_update = match config.ssd_update {
            SsdUpdatePolicy::Never => false,
            SsdUpdatePolicy::WhenNeeded => {
                ssd_unset || context.global_result == AuthenticationResult::SignatureMismatch
            }
            SsdUpdatePolicy::EveryCall => true,
        };
        let mut pending = std::collections::VecDeque::new();
        if ssd_update {
            pending.push_back(TrafficProcedure::SsdUpdate);
        }
        if ssd_update || failed || config.unique_challenge_on_call {
            pending.push_back(TrafficProcedure::UniqueChallenge);
        }
        if config.count_update_on_call {
            pending.push_back(TrafficProcedure::CountUpdate);
        }
        if pending.is_empty() || context.global_result == AuthenticationResult::NotProvisioned {
            return None;
        }
        Some(Self {
            context,
            base_station,
            pending,
            awaiting: None,
            deadline: tokio::time::Instant::now(),
        })
    }

    /// An SSD update alone, for OTASP A-key provisioning (C.S0016-D §3.3.2).
    pub(crate) fn ssd_update_only(context: CallAuthContext, base_station: String) -> Self {
        Self {
            context,
            base_station,
            pending: std::collections::VecDeque::from([TrafficProcedure::SsdUpdate]),
            awaiting: None,
            deadline: tokio::time::Instant::now(),
        }
    }

    fn arm(&mut self, awaiting: TrafficAwaiting, config: &AuthenticationConfig) {
        self.awaiting = Some(awaiting);
        self.deadline = tokio::time::Instant::now()
            + std::time::Duration::from_millis(config.procedure_timeout_ms);
    }

    pub(crate) async fn advance(
        &mut self,
        config: &AuthenticationConfig,
        hlr: &dyn HlrRepository,
    ) -> TrafficAction {
        self.awaiting = None;
        let Some(next) = self.pending.pop_front() else {
            return TrafficAction::Done;
        };
        let encoded = match next {
            TrafficProcedure::SsdUpdate => {
                let randssd = match hlr.begin_cave_ssd_update(&self.context.identity).await {
                    Ok(randssd) => randssd,
                    Err(error) => {
                        warn!("MSC: cannot start SSD update: {error}");
                        return TrafficAction::Report {
                            procedure: events_proto::AuthenticationProcedure::SsdUpdate,
                            result: events_proto::AuthenticationResult::HlrError,
                            clear_call: false,
                        };
                    }
                };
                self.arm(TrafficAwaiting::BaseStationChallenge, config);
                let mut raw = [0u8; 8];
                raw.copy_from_slice(&randssd.to_be_bytes());
                raw[0] = RANDOM_NUMBER_TYPE_RANDSSD;
                (
                    cdma_ios::MessageType::SsdUpdateRequest,
                    cdma_ios::SsdUpdateRequestMessage {
                        authentication_challenge_parameter_randssd:
                            cdma_ios::SsdUpdateChallengeParameter(raw),
                    }
                    .encode(),
                )
            }
            TrafficProcedure::UniqueChallenge => {
                let randu = rand::random::<u32>() & RANDU_MASK;
                self.arm(TrafficAwaiting::AuthenticationResponse { randu }, config);
                (
                    cdma_ios::MessageType::AuthenticationRequest,
                    cdma_ios::AuthenticationRequestMessage::Dtap(
                        cdma_ios::AuthenticationRequestDtapMessage {
                            authentication_challenge_parameter_randu:
                                cdma_ios::AuthenticationChallengeParameter::new(
                                    cdma_ios::RandomNumberType::Randu,
                                    u64::from(randu),
                                ),
                        },
                    )
                    .encode(),
                )
            }
            TrafficProcedure::CountUpdate => {
                self.arm(TrafficAwaiting::ParameterUpdateConfirm, config);
                (
                    cdma_ios::MessageType::ParameterUpdateRequest,
                    cdma_ios::ParameterUpdateRequestMessage.encode(),
                )
            }
        };
        match encoded {
            (message_type, Ok(payload)) => TrafficAction::Send(message_type, payload),
            (message_type, Err(error)) => {
                warn!("MSC: failed to encode {message_type:?}: {error}");
                TrafficAction::Done
            }
        }
    }

    pub(crate) async fn on_base_station_challenge(
        &mut self,
        config: &AuthenticationConfig,
        hlr: &dyn HlrRepository,
        randbs: u32,
    ) -> TrafficAction {
        if self.awaiting != Some(TrafficAwaiting::BaseStationChallenge) {
            warn!("MSC: unexpected Base Station Challenge, ignoring");
            return TrafficAction::Done;
        }
        let Some((imsi_s1, _)) = self.context.imsi_s() else {
            return self
                .fail_ssd_update(hlr, events_proto::AuthenticationResult::ParametersMissing)
                .await;
        };
        match hlr
            .cave_base_station_challenge(&self.context.identity, self.context.esn, randbs, imsi_s1)
            .await
        {
            Ok(authbs) => {
                self.arm(TrafficAwaiting::SsdUpdateResponse, config);
                match (cdma_ios::BaseStationChallengeResponseMessage {
                    authentication_response_parameter_authbs:
                        cdma_ios::AuthenticationResponseParameter::new(
                            cdma_ios::AuthSignatureType::Authbs,
                            authbs,
                        ),
                })
                .encode()
                {
                    Ok(payload) => TrafficAction::Send(
                        cdma_ios::MessageType::BaseStationChallengeResponse,
                        payload,
                    ),
                    Err(error) => {
                        warn!("MSC: failed to encode Base Station Challenge Response: {error}");
                        TrafficAction::Done
                    }
                }
            }
            Err(error) => {
                warn!("MSC: HLR could not compute AUTHBS: {error}");
                self.fail_ssd_update(hlr, events_proto::AuthenticationResult::HlrError)
                    .await
            }
        }
    }

    async fn fail_ssd_update(
        &mut self,
        hlr: &dyn HlrRepository,
        result: events_proto::AuthenticationResult,
    ) -> TrafficAction {
        if let Err(error) = hlr
            .finish_cave_ssd_update(&self.context.identity, self.context.esn, false)
            .await
        {
            warn!("MSC: failed to abandon SSD update: {error}");
        }
        self.pending
            .retain(|p| *p != TrafficProcedure::UniqueChallenge);
        self.awaiting = None;
        TrafficAction::Report {
            procedure: events_proto::AuthenticationProcedure::SsdUpdate,
            result,
            clear_call: false,
        }
    }

    pub(crate) async fn on_ssd_update_response(
        &mut self,
        hlr: &dyn HlrRepository,
        confirmed: bool,
    ) -> TrafficAction {
        if self.awaiting != Some(TrafficAwaiting::SsdUpdateResponse) {
            warn!("MSC: unexpected SSD Update Response, ignoring");
            return TrafficAction::Done;
        }
        if !confirmed {
            return self
                .fail_ssd_update(hlr, events_proto::AuthenticationResult::SsdUpdateRejected)
                .await;
        }
        match hlr
            .finish_cave_ssd_update(&self.context.identity, self.context.esn, true)
            .await
        {
            Ok(()) => {
                info!("MSC: SSD updated for IMSI {}", self.context.imsi);
                self.awaiting = None;
                TrafficAction::Report {
                    procedure: events_proto::AuthenticationProcedure::SsdUpdate,
                    result: events_proto::AuthenticationResult::Success,
                    clear_call: false,
                }
            }
            Err(error) => {
                warn!("MSC: HLR failed to commit SSD: {error}");
                self.fail_ssd_update(hlr, events_proto::AuthenticationResult::HlrError)
                    .await
            }
        }
    }

    pub(crate) async fn on_authentication_response(
        &mut self,
        config: &AuthenticationConfig,
        hlr: &dyn HlrRepository,
        authu: u32,
    ) -> TrafficAction {
        let Some(TrafficAwaiting::AuthenticationResponse { randu }) = self.awaiting else {
            warn!("MSC: unexpected Authentication Response, ignoring");
            return TrafficAction::Done;
        };
        self.awaiting = None;
        let report = |result: events_proto::AuthenticationResult| TrafficAction::Report {
            procedure: events_proto::AuthenticationProcedure::UniqueChallenge,
            result,
            clear_call: result != events_proto::AuthenticationResult::Success
                && config.mode == AuthenticationMode::Enforce,
        };
        let Some((imsi_s1, imsi_s2)) = self.context.imsi_s() else {
            return report(events_proto::AuthenticationResult::ParametersMissing);
        };
        let challenge = cdma_hlr::auth::UniqueChallenge {
            esn: self.context.esn,
            randu,
            authu,
            imsi_s1,
            imsi_s2,
            generate_call_keys: false,
        };
        match hlr
            .verify_cave_unique_challenge(&self.context.identity, &challenge)
            .await
        {
            Ok(verdict) => report(match verdict.outcome {
                CaveAuthOutcome::Success => events_proto::AuthenticationResult::Success,
                CaveAuthOutcome::SignatureMismatch | CaveAuthOutcome::CountMismatch => {
                    events_proto::AuthenticationResult::SignatureMismatch
                }
                CaveAuthOutcome::NotProvisioned => {
                    events_proto::AuthenticationResult::NotProvisioned
                }
            }),
            Err(error) => {
                warn!("MSC: HLR unique-challenge verification failed: {error}");
                report(events_proto::AuthenticationResult::HlrError)
            }
        }
    }

    pub(crate) async fn on_parameter_update_confirm(
        &mut self,
        hlr: &dyn HlrRepository,
    ) -> TrafficAction {
        if self.awaiting != Some(TrafficAwaiting::ParameterUpdateConfirm) {
            warn!("MSC: unexpected Parameter Update Confirm, ignoring");
            return TrafficAction::Done;
        }
        self.awaiting = None;
        let result = match hlr.increment_cave_count(&self.context.identity).await {
            Ok(count) => {
                info!("MSC: COUNT for IMSI {} is now {count}", self.context.imsi);
                events_proto::AuthenticationResult::Success
            }
            Err(error) => {
                warn!("MSC: HLR failed to increment COUNT: {error}");
                events_proto::AuthenticationResult::HlrError
            }
        };
        TrafficAction::Report {
            procedure: events_proto::AuthenticationProcedure::CountUpdate,
            result,
            clear_call: false,
        }
    }

    pub(crate) async fn on_timeout(&mut self, hlr: &dyn HlrRepository) -> TrafficAction {
        let Some(awaiting) = self.awaiting else {
            return TrafficAction::Done;
        };
        warn!(
            "MSC: {:?} timed out for IMSI {}",
            awaiting, self.context.imsi
        );
        if awaiting.procedure() == events_proto::AuthenticationProcedure::SsdUpdate {
            return self
                .fail_ssd_update(hlr, events_proto::AuthenticationResult::Timeout)
                .await;
        }
        self.awaiting = None;
        TrafficAction::Report {
            procedure: awaiting.procedure(),
            result: events_proto::AuthenticationResult::Timeout,
            clear_call: false,
        }
    }
}

#[derive(Default)]
pub struct AuthenticationHistory {
    events: std::sync::Mutex<std::collections::VecDeque<events_proto::AuthenticationEvent>>,
}

const AUTHENTICATION_HISTORY_CAPACITY: usize = 512;

impl AuthenticationHistory {
    pub fn recording(
        events: &tokio::sync::broadcast::Sender<events_proto::MscNetworkEvent>,
    ) -> std::sync::Arc<Self> {
        let history = std::sync::Arc::new(Self::default());
        let mut rx = events.subscribe();
        let recorder = history.clone();
        tokio::spawn(async move {
            use tokio::sync::broadcast::error::RecvError;
            loop {
                match rx.recv().await {
                    Ok(events_proto::MscNetworkEvent {
                        event: Some(events_proto::msc_network_event::Event::Authentication(event)),
                    }) => recorder.record(event),
                    Ok(_) | Err(RecvError::Lagged(_)) => {}
                    Err(RecvError::Closed) => break,
                }
            }
        });
        history
    }

    pub fn record(&self, event: events_proto::AuthenticationEvent) {
        let mut events = self.events.lock().expect("authentication history lock");
        if events.len() == AUTHENTICATION_HISTORY_CAPACITY {
            events.pop_front();
        }
        events.push_back(event);
    }

    /// Events for any of `imsis` or for `esn`, newest first. A `limit` of zero returns all.
    pub fn matching(
        &self,
        imsis: &[String],
        esn: Option<u32>,
        limit: usize,
    ) -> Vec<events_proto::AuthenticationEvent> {
        let events = self.events.lock().expect("authentication history lock");
        let matches = events.iter().rev().filter(|event| {
            imsis.iter().any(|imsi| *imsi == event.imsi) || (esn.is_some() && event.esn == esn)
        });
        if limit == 0 {
            matches.cloned().collect()
        } else {
            matches.take(limit).cloned().collect()
        }
    }
}

#[cfg(test)]
mod history_tests {
    use super::*;

    fn event(
        imsi: &str,
        esn: u32,
        result: events_proto::AuthenticationResult,
    ) -> events_proto::AuthenticationEvent {
        events_proto::AuthenticationEvent {
            imsi: imsi.to_string(),
            esn: Some(esn),
            result: result as i32,
            ..Default::default()
        }
    }

    #[test]
    fn history_returns_matches_newest_first_and_drops_the_oldest() {
        let history = AuthenticationHistory::default();
        history.record(event(
            "310005550000001",
            1,
            events_proto::AuthenticationResult::Success,
        ));
        history.record(event(
            "310005550000002",
            2,
            events_proto::AuthenticationResult::Success,
        ));
        history.record(event(
            "310005550000001",
            1,
            events_proto::AuthenticationResult::SignatureMismatch,
        ));
        let found = history.matching(&["310005550000001".to_string()], None, 0);
        assert_eq!(found.len(), 2);
        assert_eq!(
            found[0].result,
            events_proto::AuthenticationResult::SignatureMismatch as i32
        );
        assert_eq!(history.matching(&[], Some(2), 0).len(), 1);
        assert_eq!(
            history
                .matching(&["310005550000001".to_string()], None, 1)
                .len(),
            1
        );

        for _ in 0..AUTHENTICATION_HISTORY_CAPACITY {
            history.record(event(
                "310005550000003",
                3,
                events_proto::AuthenticationResult::Success,
            ));
        }
        assert!(
            history
                .matching(&["310005550000001".to_string()], Some(1), 0)
                .is_empty()
        );
    }
}

#[cfg(test)]
mod voice_privacy_tests {
    use super::*;

    #[test]
    fn policy_defaults_to_optional_and_requires_authenticated_keys() {
        let config: AuthenticationConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(config.voice_privacy, VoicePrivacyPolicy::Optional);
        let keys = CallKeyMaterial {
            cmea_key: [0; 8],
            private_long_code_mask: 0x147_80cc_b8df,
        };
        for (policy, requested, active) in [
            (VoicePrivacyPolicy::Optional, false, false),
            (VoicePrivacyPolicy::Optional, true, true),
            (VoicePrivacyPolicy::Always, false, true),
            (VoicePrivacyPolicy::Always, true, true),
        ] {
            assert!(policy.encryption_information(None, requested).is_none());
            let info = policy
                .encryption_information(Some(&keys), requested)
                .unwrap();
            assert_eq!(
                info.private_long_code_parameter().unwrap().unwrap().active,
                active
            );
        }
        for requested in [true, false] {
            assert!(
                VoicePrivacyPolicy::Never
                    .encryption_information(Some(&keys), requested)
                    .is_none()
            );
        }
        for policy in ["optional", "never", "always"] {
            let config: AuthenticationConfig =
                serde_json::from_value(serde_json::json!({"voice_privacy": policy})).unwrap();
            assert_eq!(serde_json::to_value(config.voice_privacy).unwrap(), policy);
        }
    }
}
