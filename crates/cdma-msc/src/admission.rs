use cdma_hlr::model::{
    MobileAdmissionClass, MobileIdentityKey, ResolvedSubscriber, SubscriberIdentity,
    SubscriberStatus,
};
use cdma_hlr::repository::HlrRepository;
use log::warn;
use serde::{Deserialize, Serialize};

use crate::grpc::events_proto::v1 as events_proto;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AccessMode {
    #[default]
    Open,
    Closed,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RegisterPolicy {
    #[default]
    SubscriberOnly,
    Any,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AccessConfig {
    pub mode: AccessMode,
    /// Registration of mobiles that are not subscribers. Ignored in open mode.
    pub register_policy: RegisterPolicy,
}

impl AccessMode {
    pub fn config_name(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
        }
    }
}

impl RegisterPolicy {
    pub fn config_name(self) -> &'static str {
        match self {
            Self::SubscriberOnly => "subscriber_only",
            Self::Any => "any",
        }
    }
}

impl std::fmt::Display for AccessConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(
            f,
            "mode={} register_policy={}",
            self.mode.config_name(),
            self.register_policy.config_name()
        )
    }
}

impl AccessConfig {
    pub fn closed(self) -> bool {
        self.mode == AccessMode::Closed
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum AdmissionClass {
    Admitted,
    Limited,
    Provisioning,
    RegistrationOnly,
    Denied,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdmissionReason {
    NotASubscriber,
    SubscriberSuspended,
    SubscriberDisabled,
    HardwareStaged,
    HlrError,
    ServiceNotAllowed,
    RegistrationPolicyAny,
    IdentityConflict,
    AmbiguousIdentity,
    AuthenticationRecoveryRequired,
    AuthenticationRequired,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AdmissionService {
    Registration,
    Voice,
    Data,
    Sms,
    Otasp,
    PageResponse,
    PageResponseSms,
}

impl AdmissionClass {
    pub fn label(self) -> &'static str {
        match self {
            Self::Admitted => "admitted",
            Self::Limited => "limited",
            Self::Provisioning => "provisioning",
            Self::RegistrationOnly => "registration only",
            Self::Denied => "denied",
        }
    }

    pub fn permits(self, service: AdmissionService) -> bool {
        use AdmissionService as S;
        match self {
            Self::Admitted => true,
            Self::Limited => matches!(service, S::Registration | S::PageResponseSms),
            Self::Provisioning => matches!(service, S::Registration | S::Otasp),
            Self::RegistrationOnly => service == S::Registration,
            Self::Denied => false,
        }
    }

    fn to_hlr(self) -> MobileAdmissionClass {
        match self {
            Self::Admitted => MobileAdmissionClass::Admitted,
            Self::Limited => MobileAdmissionClass::Limited,
            Self::Provisioning => MobileAdmissionClass::Provisioning,
            Self::RegistrationOnly => MobileAdmissionClass::RegistrationOnly,
            Self::Denied => MobileAdmissionClass::Denied,
        }
    }

    fn to_proto(self) -> events_proto::AdmissionClass {
        match self {
            Self::Admitted => events_proto::AdmissionClass::Admitted,
            Self::Limited => events_proto::AdmissionClass::Limited,
            Self::Provisioning => events_proto::AdmissionClass::Provisioning,
            Self::RegistrationOnly => events_proto::AdmissionClass::RegistrationOnly,
            Self::Denied => events_proto::AdmissionClass::Denied,
        }
    }
}

impl AdmissionReason {
    pub fn config_name(self) -> &'static str {
        match self {
            Self::NotASubscriber => "not_a_subscriber",
            Self::SubscriberSuspended => "subscriber_suspended",
            Self::SubscriberDisabled => "subscriber_disabled",
            Self::HardwareStaged => "hardware_staged",
            Self::HlrError => "hlr_error",
            Self::ServiceNotAllowed => "service_not_allowed",
            Self::RegistrationPolicyAny => "registration_policy_any",
            Self::IdentityConflict => "identity_conflict",
            Self::AmbiguousIdentity => "ambiguous_identity",
            Self::AuthenticationRecoveryRequired => "authentication_recovery_required",
            Self::AuthenticationRequired => "authentication_required",
        }
    }

    fn to_proto(self) -> events_proto::AdmissionReason {
        match self {
            Self::NotASubscriber => events_proto::AdmissionReason::NotASubscriber,
            Self::SubscriberSuspended => events_proto::AdmissionReason::SubscriberSuspended,
            Self::SubscriberDisabled => events_proto::AdmissionReason::SubscriberDisabled,
            Self::HardwareStaged => events_proto::AdmissionReason::HardwareStaged,
            Self::HlrError => events_proto::AdmissionReason::HlrError,
            Self::ServiceNotAllowed => events_proto::AdmissionReason::ServiceNotAllowed,
            Self::RegistrationPolicyAny => events_proto::AdmissionReason::RegistrationPolicyAny,
            Self::IdentityConflict => events_proto::AdmissionReason::IdentityConflict,
            Self::AmbiguousIdentity => events_proto::AdmissionReason::AmbiguousIdentity,
            Self::AuthenticationRecoveryRequired => {
                events_proto::AdmissionReason::AuthenticationRecoveryRequired
            }
            Self::AuthenticationRequired => events_proto::AdmissionReason::AuthenticationRequired,
        }
    }

    /// A.S0014-D Table 4.2.34-1 cause for a refused Location Updating Request.
    pub fn reject_cause(self) -> u8 {
        match self {
            Self::NotASubscriber => cdma_ios::REJECT_CAUSE_IMSI_UNKNOWN_IN_HLR,
            Self::HlrError => cdma_ios::REJECT_CAUSE_NETWORK_FAILURE,
            _ => cdma_ios::REJECT_CAUSE_ILLEGAL_MS,
        }
    }
}

impl AdmissionService {
    pub fn label(self) -> &'static str {
        match self {
            Self::Registration => "registration",
            Self::Voice => "voice",
            Self::Data => "data",
            Self::Sms => "SMS",
            Self::Otasp => "OTASP",
            Self::PageResponse => "page response",
            Self::PageResponseSms => "SMS page response",
        }
    }

    fn to_proto(self) -> events_proto::AdmissionService {
        match self {
            Self::Registration => events_proto::AdmissionService::Registration,
            Self::Voice => events_proto::AdmissionService::Voice,
            Self::Data => events_proto::AdmissionService::Data,
            Self::Sms | Self::PageResponseSms => events_proto::AdmissionService::Sms,
            Self::Otasp => events_proto::AdmissionService::Otasp,
            Self::PageResponse => events_proto::AdmissionService::PageResponse,
        }
    }
}

#[derive(Clone, Copy, Debug, Default)]
pub struct AccessSubject<'a> {
    pub imsi: Option<&'a str>,
    pub esn: Option<u32>,
    pub meid: Option<&'a str>,
}

impl AccessSubject<'_> {
    pub fn identity_key(&self) -> Option<MobileIdentityKey> {
        MobileIdentityKey::from_parts(self.imsi, self.esn, self.meid).ok()
    }

    pub fn device_key(&self) -> String {
        if let Some(meid) = self.meid.filter(|meid| !meid.is_empty()) {
            return format!("meid:{}", meid.to_ascii_uppercase());
        }
        if let Some(esn) = self.genuine_esn() {
            return format!("esn:{esn:08X}");
        }
        format!("imsi:{}/esn:{:?}", self.imsi.unwrap_or_default(), self.esn)
    }

    fn genuine_esn(&self) -> Option<u32> {
        self.esn.filter(|esn| !is_pseudo_esn(*esn))
    }
}

/// C.S0072 §2.1.1.1 reserves manufacturer code 0x80 for non-unique pseudo-ESNs.
const PSEUDO_ESN_PREFIX: u32 = 0x80;
const ESN_MANUFACTURER_SHIFT: u32 = 24;

pub(crate) fn is_pseudo_esn(esn: u32) -> bool {
    esn >> ESN_MANUFACTURER_SHIFT == PSEUDO_ESN_PREFIX
}

#[derive(Clone, Debug)]
pub struct Admission {
    pub class: AdmissionClass,
    pub reason: Option<AdmissionReason>,
    pub subscriber: Option<ResolvedSubscriber>,
    /// The configured identity the mobile matched. Differs from what the
    /// mobile presented when it matched by hardware.
    pub identity: Option<MobileIdentityKey>,
}

impl Admission {
    fn of(class: AdmissionClass, reason: Option<AdmissionReason>) -> Self {
        Self {
            class,
            reason,
            subscriber: None,
            identity: None,
        }
    }

    pub fn open() -> Self {
        Self::of(AdmissionClass::Admitted, None)
    }

    pub fn subscriber_id(&self) -> Option<uuid::Uuid> {
        self.subscriber
            .as_ref()
            .map(|resolved| resolved.subscriber.subscriber_id)
    }

    pub fn for_registration(mut self, policy: RegisterPolicy) -> Self {
        if self.class == AdmissionClass::Denied
            && self.reason == Some(AdmissionReason::NotASubscriber)
            && policy == RegisterPolicy::Any
        {
            self.class = AdmissionClass::RegistrationOnly;
            self.reason = Some(AdmissionReason::RegistrationPolicyAny);
        }
        self
    }

    pub fn refusal_reason(&self, service: AdmissionService) -> AdmissionReason {
        match (self.class, self.reason) {
            (AdmissionClass::Denied, Some(reason)) => reason,
            (_, _) if !self.class.permits(service) => AdmissionReason::ServiceNotAllowed,
            (_, Some(reason)) => reason,
            _ => AdmissionReason::ServiceNotAllowed,
        }
    }
}

/// The full identity (IMSI with ESN or MEID) is looked up first. Only a clean
/// miss falls back to hardware, which needs a MEID or a genuine ESN, because a
/// pseudo-ESN is not unique. HLR errors and ambiguous matches never fall back.
pub async fn classify(hlr: &dyn HlrRepository, subject: &AccessSubject<'_>) -> Admission {
    if let Some(key) = subject.identity_key() {
        match hlr.resolve_by_identity(&key).await {
            Ok(Some(resolved)) => return by_status(resolved, key, false),
            Ok(None) => {}
            Err(error) => {
                warn!("MSC: admission identity lookup failed: {error}");
                return Admission::of(AdmissionClass::Denied, Some(AdmissionReason::HlrError));
            }
        }
    }
    let meid = subject.meid;
    let esn = subject.genuine_esn();
    if esn.is_none() && meid.is_none() {
        return Admission::of(
            AdmissionClass::Denied,
            Some(AdmissionReason::NotASubscriber),
        );
    }
    let resolved = match hlr.resolve_by_hardware_identity(esn, meid).await {
        Ok(Some(resolved)) => resolved,
        Ok(None) => {
            return Admission::of(
                AdmissionClass::Denied,
                Some(AdmissionReason::NotASubscriber),
            );
        }
        Err(cdma_hlr::repository::HardwareLookupError::Ambiguous) => {
            warn!("MSC: admission hardware lookup is ambiguous");
            return Admission::of(
                AdmissionClass::Denied,
                Some(AdmissionReason::AmbiguousIdentity),
            );
        }
        Err(error) => {
            warn!("MSC: admission hardware lookup failed: {error}");
            return Admission::of(AdmissionClass::Denied, Some(AdmissionReason::HlrError));
        }
    };
    let Some(identity) = hardware_identity(&resolved.identities, subject) else {
        return Admission {
            subscriber: Some(resolved),
            ..Admission::of(
                AdmissionClass::Denied,
                Some(AdmissionReason::IdentityConflict),
            )
        };
    };
    let Ok(key) = MobileIdentityKey::from_parts(
        identity.imsi.as_deref(),
        identity.esn,
        identity.meid.as_deref(),
    ) else {
        return Admission {
            subscriber: Some(resolved),
            ..Admission::of(
                AdmissionClass::Denied,
                Some(AdmissionReason::IdentityConflict),
            )
        };
    };
    by_status(resolved, key, true)
}

fn by_status(resolved: ResolvedSubscriber, key: MobileIdentityKey, by_hardware: bool) -> Admission {
    let (class, reason) = match (&resolved.subscriber.status, by_hardware) {
        (SubscriberStatus::Active, false) => (AdmissionClass::Admitted, None),
        (SubscriberStatus::Active, true) => (
            AdmissionClass::Provisioning,
            Some(AdmissionReason::HardwareStaged),
        ),
        (SubscriberStatus::Suspended, false) => (
            AdmissionClass::Limited,
            Some(AdmissionReason::SubscriberSuspended),
        ),
        (SubscriberStatus::Suspended, true) => (
            AdmissionClass::Denied,
            Some(AdmissionReason::SubscriberSuspended),
        ),
        (SubscriberStatus::Disabled, _) => (
            AdmissionClass::Denied,
            Some(AdmissionReason::SubscriberDisabled),
        ),
    };
    Admission {
        class,
        reason,
        subscriber: Some(resolved),
        identity: Some(key),
    }
}

/// The configured identity that the presented hardware matches without
/// contradicting it. A configured MEID or ESN that differs from the presented
/// one is a conflict, so the identity does not match.
fn hardware_identity<'a>(
    identities: &'a [SubscriberIdentity],
    subject: &AccessSubject<'_>,
) -> Option<&'a SubscriberIdentity> {
    let meid_eq = |configured: &str, presented: &str| configured.eq_ignore_ascii_case(presented);
    identities.iter().find(|identity| {
        let meid_match = matches!(
            (identity.meid.as_deref(), subject.meid),
            (Some(configured), Some(presented)) if meid_eq(configured, presented)
        );
        let esn_match = subject
            .genuine_esn()
            .is_some_and(|esn| identity.esn == Some(esn));
        let meid_conflict = matches!(
            (identity.meid.as_deref(), subject.meid),
            (Some(configured), Some(presented)) if !meid_eq(configured, presented)
        );
        let esn_conflict = matches!(
            (identity.esn, subject.esn),
            (Some(configured), Some(presented)) if configured != presented
        );
        (meid_match || esn_match) && !meid_conflict && !esn_conflict
    })
}

pub(crate) fn admission_event(
    procedure: events_proto::AuthenticationProcedure,
    service: AdmissionService,
    subject: &AccessSubject<'_>,
    admission: &Admission,
    reason: Option<AdmissionReason>,
    service_rejected: bool,
    base_station: &str,
) -> events_proto::MscNetworkEvent {
    let configured_imsi = admission
        .identity
        .as_ref()
        .map(|key| key.imsi().to_string())
        .unwrap_or_default();
    events_proto::MscNetworkEvent {
        event: Some(events_proto::msc_network_event::Event::Admission(
            events_proto::AdmissionEvent {
                time: Some(prost_types::Timestamp::from(std::time::SystemTime::now())),
                procedure: procedure as i32,
                admission_class: admission.class.to_proto() as i32,
                reason: reason
                    .map(|reason| reason.to_proto() as i32)
                    .unwrap_or_default(),
                service: service.to_proto() as i32,
                service_rejected,
                imsi: subject.imsi.unwrap_or_default().to_string(),
                esn: subject.esn,
                meid: subject
                    .meid
                    .map(str::to_ascii_uppercase)
                    .unwrap_or_default(),
                base_station: base_station.to_string(),
                subscriber_id: admission
                    .subscriber_id()
                    .map(|id| id.to_string())
                    .unwrap_or_default(),
                configured_imsi,
            },
        )),
    }
}

pub(crate) fn hlr_admission(
    admission: &Admission,
    reason: Option<AdmissionReason>,
    base_station: &str,
) -> cdma_hlr::model::MobileAdmission {
    cdma_hlr::model::MobileAdmission {
        class: admission.class.to_hlr(),
        reason: reason
            .map(|reason| reason.config_name().to_string())
            .unwrap_or_default(),
        base_station: base_station.to_string(),
    }
}

/// Credential state of a provisioning target, read when provisioning is
/// authorized and read again before the session starts.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CredentialState {
    pub keyed: bool,
    pub ssd: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProvisioningCheck {
    Allowed,
    Verified,
    /// The subscriber holds an A-key and the access included no proof. A unique
    /// challenge on the traffic channel must pass before the session starts.
    ChallengeRequired,
    Refused(AdmissionReason),
}

/// Keyed-device provisioning rule. With an A-key on file, `monitor` and
/// `enforce` both require proof from the mobile, which is stricter than
/// ordinary service under `monitor`. `allow_unprovisioned` does not apply.
pub fn provisioning_check(
    mode: crate::authentication::AuthenticationMode,
    credentials: CredentialState,
    result: crate::authentication::AuthenticationResult,
    challenge_allowed: bool,
) -> ProvisioningCheck {
    use crate::authentication::{AuthenticationMode, AuthenticationResult};
    if mode == AuthenticationMode::Off || !credentials.keyed {
        return ProvisioningCheck::Allowed;
    }
    if !credentials.ssd {
        return ProvisioningCheck::Refused(AdmissionReason::AuthenticationRecoveryRequired);
    }
    match result {
        AuthenticationResult::Success => ProvisioningCheck::Verified,
        AuthenticationResult::NotRequested | AuthenticationResult::ParametersMissing
            if challenge_allowed =>
        {
            ProvisioningCheck::ChallengeRequired
        }
        _ => ProvisioningCheck::Refused(AdmissionReason::AuthenticationRequired),
    }
}

pub async fn credential_state(
    hlr: &dyn HlrRepository,
    identity: &MobileIdentityKey,
) -> Result<CredentialState, String> {
    Ok(match hlr.get_subscriber_auth_state(identity).await? {
        Some(state) => CredentialState {
            keyed: state.credentials_provisioned,
            ssd: state.ssd_provisioned,
        },
        None => CredentialState {
            keyed: false,
            ssd: false,
        },
    })
}

pub(crate) enum AdmissionWrite {
    Record {
        identity: MobileIdentityKey,
        admission: cdma_hlr::model::MobileAdmission,
    },
    #[cfg(test)]
    Flush(tokio::sync::oneshot::Sender<()>),
}

#[derive(Default)]
pub struct AdmissionHistory {
    events: std::sync::Mutex<std::collections::VecDeque<events_proto::AdmissionEvent>>,
    current: std::sync::Mutex<std::collections::HashMap<String, events_proto::AdmissionEvent>>,
}

const ADMISSION_HISTORY_CAPACITY: usize = 512;
const CURRENT_ADMISSION_CAPACITY: usize = 4096;
pub(crate) const ADMISSION_PERSISTENCE_CAPACITY: usize = 256;
pub(crate) const ADMISSION_PERSISTENCE_TIMEOUT: std::time::Duration =
    std::time::Duration::from_secs(2);

impl AdmissionHistory {
    pub fn update_current(&self, event: &events_proto::AdmissionEvent) -> bool {
        let key = AccessSubject {
            imsi: Some(&event.imsi),
            esn: event.esn,
            meid: Some(&event.meid),
        }
        .device_key();
        let mut current = self.current.lock().expect("current admission lock");
        if !current.contains_key(&key) && current.len() >= CURRENT_ADMISSION_CAPACITY {
            if let Some(oldest) = current
                .iter()
                .min_by_key(|(_, event)| event.time.as_ref().map(|time| (time.seconds, time.nanos)))
                .map(|(key, _)| key.clone())
            {
                current.remove(&oldest);
            }
        }
        let previous = current.insert(key, event.clone());
        previous.is_none_or(|previous| previous.admission_class != event.admission_class)
    }

    pub fn current_matching(
        &self,
        imsis: &[String],
        esn: Option<u32>,
        meid: Option<&str>,
        limit: usize,
    ) -> Vec<events_proto::AdmissionEvent> {
        let unfiltered = imsis.is_empty() && esn.is_none() && meid.is_none();
        let current = self.current.lock().expect("current admission lock");
        let mut events: Vec<_> = current
            .values()
            .filter(|event| {
                unfiltered
                    || imsis.iter().any(|imsi| *imsi == event.imsi)
                    || (esn.is_some() && event.esn == esn)
                    || meid.is_some_and(|meid| {
                        !meid.is_empty() && meid.eq_ignore_ascii_case(&event.meid)
                    })
            })
            .cloned()
            .collect();
        drop(current);
        events.sort_by_key(|event| {
            std::cmp::Reverse(event.time.as_ref().map(|time| (time.seconds, time.nanos)))
        });
        if limit != 0 {
            events.truncate(limit);
        }
        events
    }

    pub fn record(&self, event: events_proto::AdmissionEvent) {
        self.update_current(&event);
        let mut events = self.events.lock().expect("admission history lock");
        if events.len() == ADMISSION_HISTORY_CAPACITY {
            events.pop_front();
        }
        events.push_back(event);
    }

    /// Events matching any listed IMSI, the ESN or the MEID, newest first.
    /// With no filter every event matches. A `limit` of zero returns all.
    pub fn matching(
        &self,
        imsis: &[String],
        esn: Option<u32>,
        meid: Option<&str>,
        limit: usize,
    ) -> Vec<events_proto::AdmissionEvent> {
        let unfiltered = imsis.is_empty() && esn.is_none() && meid.is_none();
        let events = self.events.lock().expect("admission history lock");
        let matches = events.iter().rev().filter(|event| {
            unfiltered
                || imsis.iter().any(|imsi| *imsi == event.imsi)
                || (esn.is_some() && event.esn == esn)
                || meid
                    .is_some_and(|meid| !meid.is_empty() && meid.eq_ignore_ascii_case(&event.meid))
        });
        if limit == 0 {
            matches.cloned().collect()
        } else {
            matches.take(limit).cloned().collect()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::authentication::{AuthenticationMode, AuthenticationResult};

    fn identity(imsi: &str, esn: Option<u32>, meid: Option<&str>) -> SubscriberIdentity {
        SubscriberIdentity {
            subscriber_identity_id: uuid::Uuid::nil(),
            subscriber_id: uuid::Uuid::nil(),
            imsi: Some(imsi.to_string()),
            esn,
            meid: meid.map(str::to_string),
            imei: None,
            is_primary: true,
            created_at: chrono::Utc::now(),
        }
    }

    #[test]
    fn config_defaults_to_open_and_subscriber_only() {
        let config: AccessConfig = serde_json::from_str("{}").unwrap();
        assert_eq!(config.mode, AccessMode::Open);
        assert_eq!(config.register_policy, RegisterPolicy::SubscriberOnly);
        let closed: AccessConfig =
            serde_json::from_str(r#"{"mode": "closed", "register_policy": "any"}"#).unwrap();
        assert!(closed.closed());
        assert_eq!(closed.register_policy, RegisterPolicy::Any);
        assert!(serde_json::from_str::<AccessConfig>(r#"{"mode": "shut"}"#).is_err());
        assert!(serde_json::from_str::<AccessConfig>(r#"{"register": "any"}"#).is_err());
    }

    #[test]
    fn classes_permit_only_their_services() {
        use AdmissionService as S;
        assert!(AdmissionClass::Admitted.permits(S::Voice));
        assert!(AdmissionClass::Limited.permits(S::PageResponseSms));
        assert!(!AdmissionClass::Limited.permits(S::Sms));
        assert!(!AdmissionClass::Limited.permits(S::Otasp));
        assert!(!AdmissionClass::Limited.permits(S::Voice));
        assert!(AdmissionClass::Provisioning.permits(S::Otasp));
        assert!(!AdmissionClass::Provisioning.permits(S::Voice));
        assert!(!AdmissionClass::Provisioning.permits(S::PageResponseSms));
        assert!(AdmissionClass::RegistrationOnly.permits(S::Registration));
        assert!(!AdmissionClass::RegistrationOnly.permits(S::Data));
        assert!(!AdmissionClass::Denied.permits(S::Registration));
    }

    #[test]
    fn register_policy_any_changes_unknown_mobiles_only() {
        let unknown = Admission::of(
            AdmissionClass::Denied,
            Some(AdmissionReason::NotASubscriber),
        );
        assert_eq!(
            unknown.clone().for_registration(RegisterPolicy::Any).class,
            AdmissionClass::RegistrationOnly
        );
        assert_eq!(
            unknown
                .for_registration(RegisterPolicy::SubscriberOnly)
                .class,
            AdmissionClass::Denied
        );
        for reason in [
            AdmissionReason::SubscriberDisabled,
            AdmissionReason::HlrError,
            AdmissionReason::AmbiguousIdentity,
            AdmissionReason::IdentityConflict,
        ] {
            let denied = Admission::of(AdmissionClass::Denied, Some(reason));
            assert_eq!(
                denied.for_registration(RegisterPolicy::Any).class,
                AdmissionClass::Denied,
                "{reason:?}"
            );
        }
    }

    #[test]
    fn hardware_match_requires_a_consistent_identity() {
        let configured = [identity(
            "310120000000007",
            Some(0xF274_799A),
            Some("A0000012345678"),
        )];
        let matching = AccessSubject {
            imsi: Some("000000000000000"),
            esn: Some(0xF274_799A),
            meid: None,
        };
        assert!(hardware_identity(&configured, &matching).is_some());
        let conflicting = AccessSubject {
            imsi: None,
            esn: Some(0xF274_799A),
            meid: Some("A0000099999999"),
        };
        assert!(hardware_identity(&configured, &conflicting).is_none());
        let by_meid = AccessSubject {
            imsi: None,
            esn: Some(0x8012_3456),
            meid: Some("a0000012345678"),
        };
        assert!(
            hardware_identity(&configured, &by_meid).is_none(),
            "presented ESN differs from the configured one"
        );
        let meid_only = [identity("310120000000008", None, Some("A0000012345678"))];
        assert!(hardware_identity(&meid_only, &by_meid).is_some());
        let pseudo_only = AccessSubject {
            imsi: None,
            esn: Some(0x8012_3456),
            meid: None,
        };
        let pseudo_configured = [identity("310120000000009", Some(0x8012_3456), None)];
        assert!(
            hardware_identity(&pseudo_configured, &pseudo_only).is_none(),
            "a pseudo-ESN alone never matches"
        );
    }

    #[test]
    fn keyed_provisioning_requires_proof_under_monitor_and_enforce() {
        let keyed = CredentialState {
            keyed: true,
            ssd: true,
        };
        let unkeyed = CredentialState {
            keyed: false,
            ssd: false,
        };
        for mode in [AuthenticationMode::Monitor, AuthenticationMode::Enforce] {
            assert_eq!(
                provisioning_check(mode, keyed, AuthenticationResult::Success, false),
                ProvisioningCheck::Verified
            );
            assert_eq!(
                provisioning_check(mode, keyed, AuthenticationResult::SignatureMismatch, true),
                ProvisioningCheck::Refused(AdmissionReason::AuthenticationRequired)
            );
            assert_eq!(
                provisioning_check(mode, keyed, AuthenticationResult::NotRequested, false),
                ProvisioningCheck::Refused(AdmissionReason::AuthenticationRequired)
            );
            assert_eq!(
                provisioning_check(mode, keyed, AuthenticationResult::NotRequested, true),
                ProvisioningCheck::ChallengeRequired
            );
            assert_eq!(
                provisioning_check(
                    mode,
                    CredentialState {
                        keyed: true,
                        ssd: false,
                    },
                    AuthenticationResult::Success,
                    true
                ),
                ProvisioningCheck::Refused(AdmissionReason::AuthenticationRecoveryRequired)
            );
            assert_eq!(
                provisioning_check(mode, unkeyed, AuthenticationResult::NotProvisioned, false),
                ProvisioningCheck::Allowed
            );
        }
        assert_eq!(
            provisioning_check(
                AuthenticationMode::Off,
                keyed,
                AuthenticationResult::NotRequested,
                false
            ),
            ProvisioningCheck::Allowed
        );
    }
    #[test]
    fn current_admission_survives_activity_history_eviction() {
        let history = AdmissionHistory::default();
        let target = events_proto::AdmissionEvent {
            imsi: "310550000001951".into(),
            esn: Some(0x4CDC_1D09),
            admission_class: events_proto::AdmissionClass::Denied as i32,
            reason: events_proto::AdmissionReason::NotASubscriber as i32,
            ..Default::default()
        };
        history.record(target.clone());
        for esn in 0..ADMISSION_HISTORY_CAPACITY {
            history.record(events_proto::AdmissionEvent {
                esn: Some(esn as u32),
                ..Default::default()
            });
        }
        assert!(history.matching(&[], target.esn, None, 0).is_empty());
        assert_eq!(
            history.current_matching(&[], target.esn, None, 0),
            vec![target]
        );
    }

    #[test]
    fn current_admission_follows_hardware_across_imsi_changes() {
        let history = AdmissionHistory::default();
        let mut event = events_proto::AdmissionEvent {
            imsi: "310009176936269".into(),
            esn: Some(0x4CDC_1D09),
            admission_class: events_proto::AdmissionClass::Provisioning as i32,
            ..Default::default()
        };
        history.record(event.clone());
        event.imsi = "310550000001951".into();
        event.admission_class = events_proto::AdmissionClass::Admitted as i32;
        history.record(event.clone());
        assert_eq!(history.current_matching(&[], None, None, 0), vec![event]);
        assert_eq!(history.matching(&[], None, None, 0).len(), 2);
    }

    #[test]
    fn current_admission_does_not_merge_pseudo_esn_collisions() {
        let history = AdmissionHistory::default();
        for (imsi, meid) in [
            ("310550000001951", ""),
            ("310550000001004", ""),
            ("310550000001951", "a0000012345678"),
            ("310550000001951", "a0000087654321"),
        ] {
            history.record(events_proto::AdmissionEvent {
                imsi: imsi.into(),
                esn: Some(0x8012_3456),
                meid: meid.into(),
                ..Default::default()
            });
        }
        assert_eq!(history.current_matching(&[], None, None, 0).len(), 4);
        assert_eq!(
            history
                .current_matching(&[], None, Some("A0000012345678"), 0)
                .len(),
            1
        );
    }
    #[test]
    fn current_admission_is_bounded_independently_of_activity_history() {
        let history = AdmissionHistory::default();
        for index in 0..=CURRENT_ADMISSION_CAPACITY {
            history.update_current(&events_proto::AdmissionEvent {
                imsi: index.to_string(),
                time: Some(prost_types::Timestamp {
                    seconds: index as i64,
                    nanos: 0,
                }),
                ..Default::default()
            });
        }
        assert_eq!(
            history.current_matching(&[], None, None, 0).len(),
            CURRENT_ADMISSION_CAPACITY
        );
        assert!(
            history
                .current_matching(&["0".into()], None, None, 0)
                .is_empty()
        );
        assert!(history.matching(&[], None, None, 0).is_empty());
    }
}
