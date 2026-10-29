use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tonic::Code;
use uuid::Uuid;

use crate::auth::{
    CallKeyMaterial, CaveAuthOutcome, CaveCredentials, CaveVerdict, GlobalChallenge,
    UniqueChallenge,
};
use crate::model::*;
use crate::proto;

const CAVE_UNSUPPORTED: &str = "CAVE authentication is not supported by this repository";

#[async_trait]
pub trait HlrRepository: Send + Sync {
    // Subscriber
    async fn upsert_subscriber(
        &self,
        phone_number: &str,
        display_name: &str,
        status: &str,
        number_type: NumberType,
        number_plan: NumberPlan,
    ) -> Result<Subscriber, String>;

    async fn get_subscriber_by_phone_number(
        &self,
        phone_number: &str,
    ) -> Result<Option<ResolvedSubscriber>, String>;

    async fn get_subscriber_by_id(
        &self,
        subscriber_id: Uuid,
    ) -> Result<Option<ResolvedSubscriber>, String>;

    async fn update_subscriber(
        &self,
        subscriber_id: Uuid,
        phone_number: &str,
        display_name: &str,
        status: &str,
        number_type: NumberType,
        number_plan: NumberPlan,
    ) -> Result<Option<Subscriber>, String>;

    async fn list_subscribers(
        &self,
        limit: u32,
        offset: u32,
    ) -> Result<(Vec<Subscriber>, u32), String>;

    async fn delete_subscriber(&self, subscriber_id: Uuid) -> Result<bool, String>;

    // Identity
    async fn upsert_identity(
        &self,
        subscriber_id: Uuid,
        imsi: Option<&str>,
        esn: Option<u32>,
        meid: Option<&str>,
    ) -> Result<SubscriberIdentity, String>;

    async fn replace_primary_identity(
        &self,
        subscriber_id: Uuid,
        imsi: Option<&str>,
        esn: Option<u32>,
        meid: Option<&str>,
    ) -> Result<SubscriberIdentity, String>;

    async fn get_identities_for_subscriber(
        &self,
        subscriber_id: Uuid,
    ) -> Result<Vec<SubscriberIdentity>, String>;

    async fn resolve_by_identity(
        &self,
        identity: &MobileIdentityKey,
    ) -> Result<Option<ResolvedSubscriber>, String>;

    async fn resolve_registered_imsi(
        &self,
        _imsi: &str,
        _serving_bs_id: &str,
    ) -> Result<Option<ResolvedSubscriber>, tonic::Status> {
        Err(tonic::Status::unimplemented(
            "registered IMSI lookup is unavailable",
        ))
    }

    async fn resolve_by_hardware_identity(
        &self,
        esn: Option<u32>,
        meid: Option<&str>,
    ) -> Result<Option<ResolvedSubscriber>, String>;

    // Mobile sightings
    async fn upsert_mobile_seen(
        &self,
        identity: &MobileIdentityKey,
        mob_p_rev: Option<u8>,
    ) -> Result<MobileSeenUpsert, String>;

    // Registration binding
    async fn upsert_registration_binding(
        &self,
        binding: RegistrationBinding,
    ) -> Result<RegistrationBinding, String>;

    async fn get_registration_binding(
        &self,
        subscriber_id: Uuid,
    ) -> Result<Option<RegistrationBinding>, String>;

    // Ringtone
    /// Persist a custom ringtone for one subscriber. The implementation
    /// preencodes the WAV into every supported voice codec and stores the
    /// resulting frame streams.
    async fn set_ringtone(
        &self,
        subscriber_id: Uuid,
        wav_bytes: Vec<u8>,
        original_filename: &str,
    ) -> Result<SetRingtoneOutcome, String>;

    async fn clear_ringtone(&self, subscriber_id: Uuid) -> Result<(), String>;

    async fn get_ringtone_codec(
        &self,
        subscriber_id: Uuid,
        codec: &str,
    ) -> Result<Option<SubscriberRingtoneCodecBlob>, String>;

    // PRL management
    async fn list_prls(
        &self,
        limit: u32,
        offset: u32,
        filter: PrlListFilter,
    ) -> Result<(Vec<Prl>, u32), String>;
    async fn get_prl(&self, prl_id: Uuid) -> Result<Option<Prl>, String>;
    async fn get_default_prl(&self) -> Result<Option<Prl>, String>;
    /// Inserts a PRL. Caller is responsible for decoding the bytes and
    /// passing the validated `pr_list_id` and `sspr_p_rev` cached
    /// columns. Returns a "name conflict" error on duplicate active name.
    async fn create_prl(
        &self,
        name: &str,
        raw_bytes: &[u8],
        pr_list_id: i32,
        sspr_p_rev: i16,
        notes: &str,
    ) -> Result<Prl, String>;
    async fn update_prl(
        &self,
        prl_id: Uuid,
        name: Option<&str>,
        raw_bytes: Option<&[u8]>,
        pr_list_id_sspr: Option<(i32, i16)>,
        notes: Option<&str>,
    ) -> Result<Prl, String>;
    /// Soft-deletes a PRL. Returns `PrlDeleteBlocked::Referenced` when
    /// at least one subscriber still references it as override; the
    /// gRPC layer converts that to `FAILED_PRECONDITION` with details.
    async fn soft_delete_prl(&self, prl_id: Uuid) -> Result<Result<(), PrlDeleteBlocked>, String>;
    async fn set_default_prl(&self, prl_id: Uuid) -> Result<(), String>;
    async fn set_subscriber_prl_override(
        &self,
        subscriber_id: Uuid,
        prl_id: Option<Uuid>,
    ) -> Result<(), String>;
    async fn set_subscriber_spc(
        &self,
        subscriber_id: Uuid,
        spc: Option<String>,
    ) -> Result<(), String>;
    async fn set_subscriber_firstchp_override(
        &self,
        subscriber_id: Uuid,
        firstchp: Option<u16>,
    ) -> Result<(), String>;

    async fn set_subscriber_a_key(
        &self,
        _identity: &MobileIdentityKey,
        _a_key: Option<Vec<u8>>,
        _ssd_a: Option<Vec<u8>>,
        _ssd_b: Option<Vec<u8>>,
        _auth_algo: AuthAlgorithm,
    ) -> Result<(), String> {
        Err("set_subscriber_a_key is not supported by this repository".into())
    }

    /// Never returns the A-key or SSD material itself.
    async fn get_subscriber_auth_state(
        &self,
        _identity: &MobileIdentityKey,
    ) -> Result<Option<IdentityAuthState>, String> {
        Ok(None)
    }

    async fn verify_cave_global_challenge(
        &self,
        _identity: &MobileIdentityKey,
        _challenge: &GlobalChallenge,
    ) -> Result<CaveVerdict, String> {
        Err(CAVE_UNSUPPORTED.into())
    }

    async fn verify_cave_unique_challenge(
        &self,
        _identity: &MobileIdentityKey,
        _challenge: &UniqueChallenge,
    ) -> Result<CaveVerdict, String> {
        Err(CAVE_UNSUPPORTED.into())
    }

    /// Stores a fresh RANDSSD as the pending SSD update and returns it.
    async fn begin_cave_ssd_update(&self, _identity: &MobileIdentityKey) -> Result<u64, String> {
        Err(CAVE_UNSUPPORTED.into())
    }

    async fn cave_base_station_challenge(
        &self,
        _identity: &MobileIdentityKey,
        _esn: u32,
        _randbs: u32,
        _imsi_s1: u32,
    ) -> Result<u32, String> {
        Err(CAVE_UNSUPPORTED.into())
    }

    /// Replaces the stored SSD with the pending one when `confirmed`, and clears the pending
    /// update either way.
    async fn finish_cave_ssd_update(
        &self,
        _identity: &MobileIdentityKey,
        _esn: u32,
        _confirmed: bool,
    ) -> Result<(), String> {
        Err(CAVE_UNSUPPORTED.into())
    }

    async fn increment_cave_count(&self, _identity: &MobileIdentityKey) -> Result<u8, String> {
        Err(CAVE_UNSUPPORTED.into())
    }

    /// Starts an OTASP A-key exchange and returns the MS Key Request and Key Generation
    /// Request values (C.S0016-D §4.5.1.3, §4.5.1.4).
    async fn begin_cave_key_exchange(
        &self,
        _identity: &MobileIdentityKey,
        _a_key_p_rev: u8,
    ) -> Result<crate::key_exchange::KeyExchangeOffer, String> {
        Err(CAVE_UNSUPPORTED.into())
    }

    /// Derives the A-key from MS_RESULT and holds it until `commit_cave_key`.
    async fn complete_cave_key_exchange(
        &self,
        _identity: &MobileIdentityKey,
        _ms_result: &[u8],
    ) -> Result<(), String> {
        Err(CAVE_UNSUPPORTED.into())
    }

    /// Makes the exchanged A-key and its SSD current after the mobile accepts the Commit.
    async fn commit_cave_key(&self, _identity: &MobileIdentityKey) -> Result<(), String> {
        Err(CAVE_UNSUPPORTED.into())
    }

    /// Discards an uncommitted key exchange and the SSD derived from it.
    async fn abort_cave_key_exchange(&self, _identity: &MobileIdentityKey) -> Result<(), String> {
        Err(CAVE_UNSUPPORTED.into())
    }

    // OTASP session history
    /// Persists a completed OTASP session. Called by MSC when a session
    /// ends. The `events_proto` blob is `events.v1.OtaspRecordedEvents`
    /// prost-encoded so the schema doesn't fan out per event variant.
    async fn save_otasp_session(&self, row: &OtaspSessionRow) -> Result<(), String>;
    /// Returns (rows, total). `total` is the unfiltered count for the
    /// applied filter so the UI can render "Showing N–M of T".
    async fn list_otasp_sessions(
        &self,
        filter: OtaspSessionFilter,
        limit: u32,
        offset: u32,
    ) -> Result<(Vec<OtaspSessionRow>, u32), String>;
    async fn get_otasp_session(&self, session_id: Uuid) -> Result<Option<OtaspSessionRow>, String>;
}

/// gRPC-backed HLR repository adapter.
///
/// This lets existing BSC/MSC code keep using `HlrRepository` while runtime
/// wiring goes through the HLR service boundary.
pub struct GrpcHlrRepository {
    client: proto::hlr_service_client::HlrServiceClient<tonic::transport::Channel>,
}

impl GrpcHlrRepository {
    /// Connect to an HLR gRPC endpoint and return a repository that reuses the channel.
    pub async fn connect(endpoint: String) -> Result<Self, String> {
        let client = proto::hlr_service_client::HlrServiceClient::connect(endpoint.clone())
            .await
            .map_err(|e| format!("connect HLR gRPC {endpoint}: {e}"))?;
        Ok(Self { client })
    }

    /// Connect to an HLR gRPC endpoint given a socket address.
    pub async fn connect_addr(addr: std::net::SocketAddr) -> Result<Self, String> {
        Self::connect(format!("http://{addr}")).await
    }

    /// Build a repository over a lazy channel: the endpoint is only parsed
    /// here and the TCP connection is established on the first RPC, so the
    /// caller starts whether or not the HLR is already listening.
    pub fn connect_lazy(endpoint: &str) -> Result<Self, tonic::transport::Error> {
        let channel = tonic::transport::Endpoint::from_shared(endpoint.to_string())?.connect_lazy();
        Ok(Self {
            client: proto::hlr_service_client::HlrServiceClient::new(channel),
        })
    }

    fn client(&self) -> proto::hlr_service_client::HlrServiceClient<tonic::transport::Channel> {
        self.client.clone()
    }
}

fn timestamp_to_datetime(ts: Option<prost_types::Timestamp>) -> Result<DateTime<Utc>, String> {
    let ts = ts.ok_or_else(|| "missing timestamp".to_string())?;
    DateTime::<Utc>::from_timestamp(ts.seconds, ts.nanos as u32)
        .ok_or_else(|| "invalid timestamp".to_string())
}

fn prl_from_summary_proto(s: proto::PrlSummary) -> Result<Prl, String> {
    // ListPrls returns summary-only rows; the upstream consumer
    // (HlrServiceImpl::list_prls) maps the model back to PrlSummary and
    // reads `raw_bytes.len()` for `raw_bytes_size`. We preserve the
    // length with a zero-filled buffer; the actual bytes are not needed
    // on this path.
    let raw_bytes = vec![0u8; s.raw_bytes_size as usize];
    Ok(Prl {
        prl_id: Uuid::parse_str(&s.prl_id).map_err(|e| format!("invalid prl_id: {e}"))?,
        name: s.name,
        pr_list_id: s.pr_list_id as i32,
        sspr_p_rev: s.sspr_p_rev as i16,
        is_default: s.is_default,
        raw_bytes,
        notes: s.notes,
        created_at: timestamp_to_datetime(s.created_at)?,
        updated_at: timestamp_to_datetime(s.updated_at)?,
    })
}

fn datetime_to_timestamp(t: DateTime<Utc>) -> prost_types::Timestamp {
    prost_types::Timestamp {
        seconds: t.timestamp(),
        nanos: t.timestamp_subsec_nanos() as i32,
    }
}

fn otasp_session_summary_to_proto(row: &OtaspSessionRow) -> proto::OtaspSessionSummary {
    proto::OtaspSessionSummary {
        session_id: row.session_id.to_string(),
        subscriber_id: row.subscriber_id.map(|u| u.to_string()),
        esn: row.esn,
        meid: row.meid.clone(),
        started_at: Some(datetime_to_timestamp(row.started_at)),
        ended_at: row.ended_at.map(datetime_to_timestamp),
        outcome: row.outcome as i32,
        feature_code: row.feature_code.clone(),
        service_option: row.service_option.map(|v| v as u32),
        completed_blocks: row.completed_blocks as u32,
        event_count: row.event_count as u32,
    }
}

fn otasp_session_summary_from_proto(
    s: proto::OtaspSessionSummary,
    events_proto: Vec<u8>,
) -> Result<OtaspSessionRow, String> {
    Ok(OtaspSessionRow {
        session_id: Uuid::parse_str(&s.session_id)
            .map_err(|e| format!("invalid session_id: {e}"))?,
        subscriber_id: s
            .subscriber_id
            .as_deref()
            .filter(|v| !v.is_empty())
            .map(Uuid::parse_str)
            .transpose()
            .map_err(|e| format!("invalid subscriber_id: {e}"))?,
        esn: s.esn,
        meid: s.meid,
        started_at: timestamp_to_datetime(s.started_at)?,
        ended_at: match s.ended_at {
            Some(ts) => Some(timestamp_to_datetime(Some(ts))?),
            None => None,
        },
        outcome: s.outcome as i16,
        feature_code: s.feature_code,
        service_option: s.service_option.map(|v| v as i32),
        completed_blocks: s.completed_blocks as i32,
        event_count: s.event_count as i32,
        events_proto,
    })
}

fn prl_from_full_proto(p: proto::Prl) -> Result<Prl, String> {
    let summary = p.summary.ok_or_else(|| "Prl.summary missing".to_string())?;
    Ok(Prl {
        prl_id: Uuid::parse_str(&summary.prl_id).map_err(|e| format!("invalid prl_id: {e}"))?,
        name: summary.name,
        pr_list_id: summary.pr_list_id as i32,
        sspr_p_rev: summary.sspr_p_rev as i16,
        is_default: summary.is_default,
        raw_bytes: p.raw_bytes,
        notes: summary.notes,
        created_at: timestamp_to_datetime(summary.created_at)?,
        updated_at: timestamp_to_datetime(summary.updated_at)?,
    })
}

/// Maps proto enum (UNSPECIFIED → default) to the model enum.
pub(crate) fn number_type_from_proto(value: i32) -> NumberType {
    match proto::NumberType::try_from(value).unwrap_or(proto::NumberType::Unspecified) {
        proto::NumberType::Unspecified => NumberType::default(),
        proto::NumberType::Unknown => NumberType::Unknown,
        proto::NumberType::International => NumberType::International,
        proto::NumberType::National => NumberType::National,
        proto::NumberType::NetworkSpecific => NumberType::NetworkSpecific,
        proto::NumberType::Subscriber => NumberType::Subscriber,
        proto::NumberType::Abbreviated => NumberType::Abbreviated,
    }
}

/// Maps proto enum (UNSPECIFIED → default) to the model enum.
pub(crate) fn number_plan_from_proto(value: i32) -> NumberPlan {
    match proto::NumberPlan::try_from(value).unwrap_or(proto::NumberPlan::Unspecified) {
        proto::NumberPlan::Unspecified => NumberPlan::default(),
        proto::NumberPlan::Unknown => NumberPlan::Unknown,
        proto::NumberPlan::IsdnE164 => NumberPlan::IsdnE164,
        proto::NumberPlan::Data => NumberPlan::Data,
        proto::NumberPlan::Telex => NumberPlan::Telex,
        proto::NumberPlan::Private => NumberPlan::Private,
    }
}

pub(crate) fn number_type_to_proto(value: NumberType) -> proto::NumberType {
    match value {
        NumberType::Unknown => proto::NumberType::Unknown,
        NumberType::International => proto::NumberType::International,
        NumberType::National => proto::NumberType::National,
        NumberType::NetworkSpecific => proto::NumberType::NetworkSpecific,
        NumberType::Subscriber => proto::NumberType::Subscriber,
        NumberType::Abbreviated => proto::NumberType::Abbreviated,
    }
}

pub(crate) fn number_plan_to_proto(value: NumberPlan) -> proto::NumberPlan {
    match value {
        NumberPlan::Unknown => proto::NumberPlan::Unknown,
        NumberPlan::IsdnE164 => proto::NumberPlan::IsdnE164,
        NumberPlan::Data => proto::NumberPlan::Data,
        NumberPlan::Telex => proto::NumberPlan::Telex,
        NumberPlan::Private => proto::NumberPlan::Private,
    }
}

fn subscriber_from_proto(value: proto::Subscriber) -> Result<Subscriber, String> {
    Ok(Subscriber {
        subscriber_id: Uuid::parse_str(&value.subscriber_id)
            .map_err(|e| format!("invalid subscriber_id: {e}"))?,
        phone_number: value.phone_number,
        display_name: value.display_name,
        status: subscriber_status_from_proto(value.status)?,
        created_at: timestamp_to_datetime(value.created_at)?,
        updated_at: timestamp_to_datetime(value.updated_at)?,
        number_type: number_type_from_proto(value.number_type),
        number_plan: number_plan_from_proto(value.number_plan),
        has_ringtone: value.has_ringtone,
        ringtone_duration_ms: value.ringtone_duration_ms,
        prl_override_id: value
            .prl_override_id
            .as_deref()
            .map(Uuid::parse_str)
            .transpose()
            .map_err(|e| format!("invalid prl_override_id: {e}"))?,
        service_programming_code: value.service_programming_code,
        firstchp_override: value.firstchp_override.map(|v| v as u16),
    })
}

fn subscriber_status_from_proto(value: i32) -> Result<SubscriberStatus, String> {
    match proto::SubscriberStatus::try_from(value) {
        Ok(proto::SubscriberStatus::Active) => Ok(SubscriberStatus::Active),
        Ok(proto::SubscriberStatus::Suspended) => Ok(SubscriberStatus::Suspended),
        Ok(proto::SubscriberStatus::Disabled) => Ok(SubscriberStatus::Disabled),
        Ok(proto::SubscriberStatus::Unspecified) | Err(_) => {
            Err(format!("unknown subscriber status: {value}"))
        }
    }
}

pub(crate) fn subscriber_status_to_proto(value: &SubscriberStatus) -> i32 {
    match value {
        SubscriberStatus::Active => proto::SubscriberStatus::Active as i32,
        SubscriberStatus::Suspended => proto::SubscriberStatus::Suspended as i32,
        SubscriberStatus::Disabled => proto::SubscriberStatus::Disabled as i32,
    }
}

fn identity_from_proto(value: proto::SubscriberIdentity) -> Result<SubscriberIdentity, String> {
    Ok(SubscriberIdentity {
        subscriber_identity_id: Uuid::parse_str(&value.subscriber_identity_id)
            .map_err(|e| format!("invalid subscriber_identity_id: {e}"))?,
        subscriber_id: Uuid::parse_str(&value.subscriber_id)
            .map_err(|e| format!("invalid subscriber_id: {e}"))?,
        imsi: value.imsi,
        esn: value.esn,
        meid: value.meid,
        is_primary: value.is_primary,
        created_at: timestamp_to_datetime(value.created_at)?,
    })
}

fn identity_key_to_proto(value: &MobileIdentityKey) -> proto::MobileIdentityKey {
    proto::MobileIdentityKey {
        imsi: Some(value.imsi().to_string()),
        esn: value.esn(),
        meid: value.meid().map(ToOwned::to_owned),
    }
}

fn auth_algo_to_proto(algo: AuthAlgorithm) -> i32 {
    let variant = match algo {
        AuthAlgorithm::None => proto::AuthAlgorithm::None,
        AuthAlgorithm::Cave => proto::AuthAlgorithm::Cave,
    };
    variant as i32
}

fn auth_algo_from_proto(value: i32) -> AuthAlgorithm {
    match proto::AuthAlgorithm::try_from(value) {
        Ok(proto::AuthAlgorithm::Cave) => AuthAlgorithm::Cave,
        _ => AuthAlgorithm::None,
    }
}

pub(crate) fn cave_outcome_to_proto(outcome: CaveAuthOutcome) -> proto::CaveAuthOutcome {
    match outcome {
        CaveAuthOutcome::Success => proto::CaveAuthOutcome::Success,
        CaveAuthOutcome::SignatureMismatch => proto::CaveAuthOutcome::SignatureMismatch,
        CaveAuthOutcome::CountMismatch => proto::CaveAuthOutcome::CountMismatch,
        CaveAuthOutcome::NotProvisioned => proto::CaveAuthOutcome::NotProvisioned,
    }
}

pub(crate) fn cave_verdict_to_proto(verdict: CaveVerdict) -> proto::CaveVerdict {
    proto::CaveVerdict {
        outcome: cave_outcome_to_proto(verdict.outcome) as i32,
        cmea_key: verdict.call_keys.map(|k| k.cmea_key.to_vec()),
        private_long_code_mask: verdict.call_keys.map(|k| k.private_long_code_mask),
    }
}

fn cave_verdict_from_proto(verdict: proto::CaveVerdict) -> Result<CaveVerdict, String> {
    let outcome = match proto::CaveAuthOutcome::try_from(verdict.outcome) {
        Ok(proto::CaveAuthOutcome::Success) => CaveAuthOutcome::Success,
        Ok(proto::CaveAuthOutcome::SignatureMismatch) => CaveAuthOutcome::SignatureMismatch,
        Ok(proto::CaveAuthOutcome::CountMismatch) => CaveAuthOutcome::CountMismatch,
        Ok(proto::CaveAuthOutcome::NotProvisioned) => CaveAuthOutcome::NotProvisioned,
        _ => return Err(format!("unknown CAVE outcome {}", verdict.outcome)),
    };
    let call_keys = match (verdict.cmea_key, verdict.private_long_code_mask) {
        (Some(key), Some(mask)) => Some(CallKeyMaterial {
            cmea_key: key
                .try_into()
                .map_err(|_| "CMEA key must be 8 bytes".to_string())?,
            private_long_code_mask: mask,
        }),
        _ => None,
    };
    Ok(CaveVerdict { outcome, call_keys })
}

fn resolved_from_proto(
    subscriber: Option<proto::Subscriber>,
    primary_identity: Option<proto::SubscriberIdentity>,
    binding: Option<proto::RegistrationBinding>,
) -> Result<Option<ResolvedSubscriber>, String> {
    let Some(subscriber_proto) = subscriber else {
        return Ok(None);
    };
    let subscriber = subscriber_from_proto(subscriber_proto)?;
    let primary_identity = primary_identity.map(identity_from_proto).transpose()?;
    let binding = binding.map(binding_from_proto).transpose()?;
    Ok(Some(ResolvedSubscriber {
        subscriber,
        identities: Vec::new(),
        primary_identity,
        binding,
    }))
}

fn binding_from_proto(value: proto::RegistrationBinding) -> Result<RegistrationBinding, String> {
    Ok(RegistrationBinding {
        subscriber_id: Uuid::parse_str(&value.subscriber_id)
            .map_err(|e| format!("invalid subscriber_id: {e}"))?,
        serving_bs_id: value.serving_bs_id,
        state: RegistrationState::from_str(&value.state)
            .map_err(|e| format!("invalid registration state: {e}"))?,
        imsi: value.imsi,
        esn: value.esn,
        meid: value.meid,
        mob_p_rev: value.mob_p_rev,
        pgslot: value.pgslot,
        slot_cycle_index: value.slot_cycle_index,
        last_msg_seq: value.last_msg_seq,
        last_registered_at: timestamp_to_datetime(value.last_registered_at)?,
        last_seen_at: timestamp_to_datetime(value.last_seen_at)?,
        updated_at: timestamp_to_datetime(value.updated_at)?,
    })
}

fn mobile_seen_from_proto(value: proto::MobileSeenUpsert) -> Result<MobileSeenUpsert, String> {
    Ok(MobileSeenUpsert {
        is_new: value.is_new,
        previous_last_seen_at: value
            .previous_last_seen_at
            .map(|ts| timestamp_to_datetime(Some(ts)))
            .transpose()?,
    })
}

#[async_trait]
impl HlrRepository for GrpcHlrRepository {
    async fn upsert_subscriber(
        &self,
        phone_number: &str,
        display_name: &str,
        status: &str,
        number_type: NumberType,
        number_plan: NumberPlan,
    ) -> Result<Subscriber, String> {
        let status = SubscriberStatus::from_str(status)?;
        let mut client = self.client();
        let response = client
            .upsert_subscriber(proto::UpsertSubscriberRequest {
                phone_number: phone_number.to_string(),
                display_name: display_name.to_string(),
                status: subscriber_status_to_proto(&status),
                imsi: None,
                esn: None,
                meid: None,
                number_type: number_type_to_proto(number_type) as i32,
                number_plan: number_plan_to_proto(number_plan) as i32,
            })
            .await
            .map_err(|e| format!("HLR UpsertSubscriber: {e}"))?
            .into_inner();
        subscriber_from_proto(
            response
                .subscriber
                .ok_or_else(|| "missing subscriber".to_string())?,
        )
    }

    async fn get_subscriber_by_phone_number(
        &self,
        phone_number: &str,
    ) -> Result<Option<ResolvedSubscriber>, String> {
        let mut client = self.client();
        match client
            .get_subscriber_by_phone_number(proto::GetSubscriberByPhoneNumberRequest {
                phone_number: phone_number.to_string(),
            })
            .await
        {
            Ok(response) => {
                let inner = response.into_inner();
                let subscriber = subscriber_from_proto(
                    inner
                        .subscriber
                        .ok_or_else(|| "missing subscriber".to_string())?,
                )?;
                let identities = inner
                    .identities
                    .into_iter()
                    .map(identity_from_proto)
                    .collect::<Result<Vec<_>, _>>()?;
                let primary_identity = inner
                    .primary_identity
                    .map(identity_from_proto)
                    .transpose()?;
                let binding = inner.binding.map(binding_from_proto).transpose()?;
                Ok(Some(ResolvedSubscriber {
                    subscriber,
                    identities,
                    primary_identity,
                    binding,
                }))
            }
            Err(status) if status.code() == Code::NotFound => Ok(None),
            Err(status) => Err(format!("HLR GetSubscriberByPhoneNumber: {status}")),
        }
    }

    async fn get_subscriber_by_id(
        &self,
        subscriber_id: Uuid,
    ) -> Result<Option<ResolvedSubscriber>, String> {
        let mut client = self.client();
        match client
            .get_subscriber(proto::GetSubscriberRequest {
                subscriber_id: subscriber_id.to_string(),
            })
            .await
        {
            Ok(response) => {
                let inner = response.into_inner();
                let subscriber = subscriber_from_proto(
                    inner
                        .subscriber
                        .ok_or_else(|| "missing subscriber".to_string())?,
                )?;
                let identities = inner
                    .identities
                    .into_iter()
                    .map(identity_from_proto)
                    .collect::<Result<Vec<_>, _>>()?;
                let primary_identity = inner
                    .primary_identity
                    .map(identity_from_proto)
                    .transpose()?;
                let binding = inner.binding.map(binding_from_proto).transpose()?;
                Ok(Some(ResolvedSubscriber {
                    subscriber,
                    identities,
                    primary_identity,
                    binding,
                }))
            }
            Err(status) if status.code() == Code::NotFound => Ok(None),
            Err(status) => Err(format!("HLR GetSubscriber: {status}")),
        }
    }

    async fn update_subscriber(
        &self,
        subscriber_id: Uuid,
        phone_number: &str,
        display_name: &str,
        status: &str,
        number_type: NumberType,
        number_plan: NumberPlan,
    ) -> Result<Option<Subscriber>, String> {
        let status = SubscriberStatus::from_str(status)?;
        let mut client = self.client();
        match client
            .update_subscriber(proto::UpdateSubscriberRequest {
                subscriber_id: subscriber_id.to_string(),
                phone_number: phone_number.to_string(),
                display_name: display_name.to_string(),
                status: subscriber_status_to_proto(&status),
                imsi: None,
                esn: None,
                meid: None,
                number_type: number_type_to_proto(number_type) as i32,
                number_plan: number_plan_to_proto(number_plan) as i32,
            })
            .await
        {
            Ok(response) => subscriber_from_proto(
                response
                    .into_inner()
                    .subscriber
                    .ok_or_else(|| "missing subscriber".to_string())?,
            )
            .map(Some),
            Err(status) if status.code() == Code::NotFound => Ok(None),
            Err(status) => Err(format!("HLR UpdateSubscriber: {status}")),
        }
    }

    async fn list_subscribers(
        &self,
        limit: u32,
        offset: u32,
    ) -> Result<(Vec<Subscriber>, u32), String> {
        let mut client = self.client();
        let response = client
            .list_subscribers(proto::ListSubscribersRequest {
                limit: Some(limit),
                offset: Some(offset),
            })
            .await
            .map_err(|e| format!("HLR ListSubscribers: {e}"))?
            .into_inner();
        let subscribers = response
            .subscribers
            .into_iter()
            .map(subscriber_from_proto)
            .collect::<Result<Vec<_>, _>>()?;
        Ok((subscribers, response.total))
    }

    async fn delete_subscriber(&self, subscriber_id: Uuid) -> Result<bool, String> {
        let mut client = self.client();
        match client
            .delete_subscriber(proto::DeleteSubscriberRequest {
                subscriber_id: subscriber_id.to_string(),
            })
            .await
        {
            Ok(_) => Ok(true),
            Err(status) if status.code() == Code::NotFound => Ok(false),
            Err(status) => Err(format!("HLR DeleteSubscriber: {status}")),
        }
    }

    async fn upsert_identity(
        &self,
        subscriber_id: Uuid,
        imsi: Option<&str>,
        esn: Option<u32>,
        meid: Option<&str>,
    ) -> Result<SubscriberIdentity, String> {
        let mut client = self.client();
        let response = client
            .upsert_identity(proto::UpsertIdentityRequest {
                subscriber_id: subscriber_id.to_string(),
                imsi: imsi.map(ToOwned::to_owned),
                esn,
                meid: meid.map(ToOwned::to_owned),
            })
            .await
            .map_err(|e| format!("HLR UpsertIdentity: {e}"))?
            .into_inner();
        identity_from_proto(
            response
                .identity
                .ok_or_else(|| "missing identity".to_string())?,
        )
    }

    async fn replace_primary_identity(
        &self,
        subscriber_id: Uuid,
        imsi: Option<&str>,
        esn: Option<u32>,
        meid: Option<&str>,
    ) -> Result<SubscriberIdentity, String> {
        let mut client = self.client();
        let response = client
            .replace_primary_identity(proto::ReplacePrimaryIdentityRequest {
                subscriber_id: subscriber_id.to_string(),
                imsi: imsi.map(ToOwned::to_owned),
                esn,
                meid: meid.map(ToOwned::to_owned),
            })
            .await
            .map_err(|e| format!("HLR ReplacePrimaryIdentity: {e}"))?
            .into_inner();
        identity_from_proto(
            response
                .identity
                .ok_or_else(|| "missing identity".to_string())?,
        )
    }

    async fn get_identities_for_subscriber(
        &self,
        subscriber_id: Uuid,
    ) -> Result<Vec<SubscriberIdentity>, String> {
        let mut client = self.client();
        let response = client
            .get_identities_for_subscriber(proto::GetIdentitiesForSubscriberRequest {
                subscriber_id: subscriber_id.to_string(),
            })
            .await
            .map_err(|e| format!("HLR GetIdentitiesForSubscriber: {e}"))?
            .into_inner();
        response
            .identities
            .into_iter()
            .map(identity_from_proto)
            .collect()
    }

    async fn resolve_by_identity(
        &self,
        identity: &MobileIdentityKey,
    ) -> Result<Option<ResolvedSubscriber>, String> {
        let mut client = self.client();
        let response = client
            .resolve_subscriber_by_identity(proto::ResolveSubscriberByIdentityRequest {
                identity: Some(identity_key_to_proto(identity)),
            })
            .await
            .map_err(|e| format!("HLR ResolveSubscriberByIdentity: {e}"))?
            .into_inner();
        // The resolve response intentionally doesn't include the full
        // identity list — only the primary one. Callers that need the
        // whole list should use GetSubscriber.
        resolved_from_proto(
            response.subscriber,
            response.primary_identity,
            response.binding,
        )
    }

    async fn resolve_registered_imsi(
        &self,
        imsi: &str,
        serving_bs_id: &str,
    ) -> Result<Option<ResolvedSubscriber>, tonic::Status> {
        let response = self
            .client()
            .resolve_subscriber_by_registered_imsi(
                proto::ResolveSubscriberByRegisteredImsiRequest {
                    imsi: imsi.to_string(),
                    serving_bs_id: serving_bs_id.to_string(),
                },
            )
            .await?
            .into_inner();
        resolved_from_proto(
            response.subscriber,
            response.primary_identity,
            response.binding,
        )
        .map_err(tonic::Status::internal)
    }

    async fn resolve_by_hardware_identity(
        &self,
        esn: Option<u32>,
        meid: Option<&str>,
    ) -> Result<Option<ResolvedSubscriber>, String> {
        let mut client = self.client();
        let response = client
            .resolve_subscriber_by_hardware_identity(
                proto::ResolveSubscriberByHardwareIdentityRequest {
                    identity: Some(proto::HardwareIdentityKey {
                        esn,
                        meid: meid.map(ToOwned::to_owned),
                    }),
                },
            )
            .await
            .map_err(|e| format!("HLR ResolveSubscriberByHardwareIdentity: {e}"))?
            .into_inner();
        resolved_from_proto(
            response.subscriber,
            response.primary_identity,
            response.binding,
        )
    }

    async fn upsert_mobile_seen(
        &self,
        identity: &MobileIdentityKey,
        mob_p_rev: Option<u8>,
    ) -> Result<MobileSeenUpsert, String> {
        let mut client = self.client();
        let response = client
            .upsert_mobile_seen(proto::UpsertMobileSeenRequest {
                identity: Some(identity_key_to_proto(identity)),
                mob_p_rev: mob_p_rev.map(u32::from),
            })
            .await
            .map_err(|e| format!("HLR UpsertMobileSeen: {e}"))?
            .into_inner();
        mobile_seen_from_proto(
            response
                .result
                .ok_or_else(|| "missing result".to_string())?,
        )
    }

    async fn upsert_registration_binding(
        &self,
        binding: RegistrationBinding,
    ) -> Result<RegistrationBinding, String> {
        let mut client = self.client();
        let response = client
            .upsert_registration_binding(proto::UpsertRegistrationBindingRequest {
                subscriber_id: binding.subscriber_id.to_string(),
                serving_bs_id: binding.serving_bs_id,
                state: binding.state.as_str().to_string(),
                imsi: binding.imsi,
                esn: binding.esn,
                meid: binding.meid,
                mob_p_rev: binding.mob_p_rev,
                pgslot: binding.pgslot,
                slot_cycle_index: binding.slot_cycle_index,
                last_msg_seq: binding.last_msg_seq,
            })
            .await
            .map_err(|e| format!("HLR UpsertRegistrationBinding: {e}"))?
            .into_inner();
        binding_from_proto(
            response
                .binding
                .ok_or_else(|| "missing binding".to_string())?,
        )
    }

    async fn get_registration_binding(
        &self,
        subscriber_id: Uuid,
    ) -> Result<Option<RegistrationBinding>, String> {
        let mut client = self.client();
        let response = client
            .get_registration_binding(proto::GetRegistrationBindingRequest {
                subscriber_id: subscriber_id.to_string(),
            })
            .await
            .map_err(|e| format!("HLR GetRegistrationBinding: {e}"))?
            .into_inner();
        response.binding.map(binding_from_proto).transpose()
    }

    async fn set_ringtone(
        &self,
        subscriber_id: Uuid,
        wav_bytes: Vec<u8>,
        original_filename: &str,
    ) -> Result<SetRingtoneOutcome, String> {
        let mut client = self.client();
        let response = client
            .set_subscriber_ringtone(proto::SetSubscriberRingtoneRequest {
                subscriber_id: subscriber_id.to_string(),
                wav_bytes,
                original_filename: original_filename.to_string(),
            })
            .await
            .map_err(|e| format!("HLR SetSubscriberRingtone: {e}"))?
            .into_inner();
        Ok(SetRingtoneOutcome {
            codecs: response
                .codecs
                .into_iter()
                .map(|c| SetRingtoneCodecOutcome {
                    codec: c.codec,
                    encoded_bytes: c.encoded_bytes,
                    frame_count: c.frame_count,
                })
                .collect(),
            duration_ms: response.duration_ms,
        })
    }

    async fn clear_ringtone(&self, subscriber_id: Uuid) -> Result<(), String> {
        let mut client = self.client();
        client
            .clear_subscriber_ringtone(proto::ClearSubscriberRingtoneRequest {
                subscriber_id: subscriber_id.to_string(),
            })
            .await
            .map_err(|e| format!("HLR ClearSubscriberRingtone: {e}"))?;
        Ok(())
    }

    async fn get_ringtone_codec(
        &self,
        subscriber_id: Uuid,
        codec: &str,
    ) -> Result<Option<SubscriberRingtoneCodecBlob>, String> {
        let mut client = self.client();
        match client
            .get_subscriber_ringtone_codec(proto::GetSubscriberRingtoneCodecRequest {
                subscriber_id: subscriber_id.to_string(),
                codec: codec.to_string(),
            })
            .await
        {
            Ok(response) => {
                let r = response.into_inner();
                Ok(Some(SubscriberRingtoneCodecBlob {
                    codec: codec.to_string(),
                    encoded_frames: r.encoded_frames,
                    frame_count: r.frame_count,
                    duration_ms: r.duration_ms,
                }))
            }
            Err(status) if status.code() == Code::NotFound => Ok(None),
            Err(status) => Err(format!("HLR GetSubscriberRingtoneCodec: {status}")),
        }
    }

    // PRL management — proxy each call to the upstream HLR service. The
    // BSC management gRPC port re-exports `HlrServiceImpl` backed by this
    // adapter, so all PRL traffic from the web UI lands here and needs to
    // forward through the channel.
    async fn list_prls(
        &self,
        limit: u32,
        offset: u32,
        filter: PrlListFilter,
    ) -> Result<(Vec<Prl>, u32), String> {
        let mut client = self.client();
        let resp = client
            .list_prls(proto::ListPrlsRequest {
                limit,
                offset,
                pr_list_id: filter.pr_list_id,
                sspr_p_rev: filter.sspr_p_rev,
            })
            .await
            .map_err(|status| format!("HLR ListPrls: {status}"))?;
        let r = resp.into_inner();
        let prls = r
            .prls
            .into_iter()
            .map(prl_from_summary_proto)
            .collect::<Result<Vec<_>, _>>()?;
        Ok((prls, r.total))
    }
    async fn get_prl(&self, prl_id: Uuid) -> Result<Option<Prl>, String> {
        let mut client = self.client();
        match client
            .get_prl(proto::GetPrlRequest {
                prl_id: prl_id.to_string(),
            })
            .await
        {
            Ok(resp) => resp.into_inner().prl.map(prl_from_full_proto).transpose(),
            Err(status) if status.code() == Code::NotFound => Ok(None),
            Err(status) => Err(format!("HLR GetPrl: {status}")),
        }
    }
    async fn get_default_prl(&self) -> Result<Option<Prl>, String> {
        let mut client = self.client();
        let resp = client
            .get_default_prl(proto::GetDefaultPrlRequest {})
            .await
            .map_err(|status| format!("HLR GetDefaultPrl: {status}"))?;
        resp.into_inner().prl.map(prl_from_full_proto).transpose()
    }
    async fn create_prl(
        &self,
        name: &str,
        raw_bytes: &[u8],
        _pr_list_id: i32,
        _sspr_p_rev: i16,
        notes: &str,
    ) -> Result<Prl, String> {
        // Upstream re-decodes the bytes to recompute the cached cols, so
        // we don't need to (and can't reliably) ship them across the wire.
        let mut client = self.client();
        let resp = client
            .create_prl(proto::CreatePrlRequest {
                name: name.to_string(),
                notes: notes.to_string(),
                source: Some(proto::create_prl_request::Source::RawBytes(
                    raw_bytes.to_vec(),
                )),
            })
            .await
            .map_err(|status| format!("HLR CreatePrl: {status}"))?;
        resp.into_inner()
            .prl
            .ok_or_else(|| "HLR CreatePrl: empty response".to_string())
            .and_then(prl_from_full_proto)
    }
    async fn update_prl(
        &self,
        prl_id: Uuid,
        name: Option<&str>,
        raw_bytes: Option<&[u8]>,
        _pr_list_id_sspr: Option<(i32, i16)>,
        notes: Option<&str>,
    ) -> Result<Prl, String> {
        let mut client = self.client();
        let body_update =
            raw_bytes.map(|bytes| proto::update_prl_request::BodyUpdate::RawBytes(bytes.to_vec()));
        let resp = client
            .update_prl(proto::UpdatePrlRequest {
                prl_id: prl_id.to_string(),
                name: name.map(str::to_string),
                notes: notes.map(str::to_string),
                body_update,
            })
            .await
            .map_err(|status| format!("HLR UpdatePrl: {status}"))?;
        resp.into_inner()
            .prl
            .ok_or_else(|| "HLR UpdatePrl: empty response".to_string())
            .and_then(prl_from_full_proto)
    }
    async fn soft_delete_prl(&self, prl_id: Uuid) -> Result<Result<(), PrlDeleteBlocked>, String> {
        let mut client = self.client();
        match client
            .delete_prl(proto::DeletePrlRequest {
                prl_id: prl_id.to_string(),
            })
            .await
        {
            Ok(_) => Ok(Ok(())),
            Err(status) if status.code() == Code::FailedPrecondition => {
                // Upstream encodes the referencing-subscriber details in
                // the message string only. The web UI shows the toast
                // regardless of count, so report zero with no samples.
                Ok(Err(PrlDeleteBlocked::Referenced {
                    count: 0,
                    sample: vec![],
                }))
            }
            Err(status) => Err(format!("HLR DeletePrl: {status}")),
        }
    }
    async fn set_default_prl(&self, prl_id: Uuid) -> Result<(), String> {
        let mut client = self.client();
        client
            .set_default_prl(proto::SetDefaultPrlRequest {
                prl_id: prl_id.to_string(),
            })
            .await
            .map(|_| ())
            .map_err(|status| format!("HLR SetDefaultPrl: {status}"))
    }
    async fn set_subscriber_prl_override(
        &self,
        subscriber_id: Uuid,
        prl_id: Option<Uuid>,
    ) -> Result<(), String> {
        let mut client = self.client();
        client
            .set_subscriber_prl_override(proto::SetSubscriberPrlOverrideRequest {
                subscriber_id: subscriber_id.to_string(),
                prl_id: prl_id.map(|u| u.to_string()),
            })
            .await
            .map(|_| ())
            .map_err(|status| format!("HLR SetSubscriberPrlOverride: {status}"))
    }
    async fn set_subscriber_spc(
        &self,
        subscriber_id: Uuid,
        spc: Option<String>,
    ) -> Result<(), String> {
        let mut client = self.client();
        client
            .set_subscriber_spc(proto::SetSubscriberSpcRequest {
                subscriber_id: subscriber_id.to_string(),
                service_programming_code: spc,
            })
            .await
            .map(|_| ())
            .map_err(|status| format!("HLR SetSubscriberSpc: {status}"))
    }
    async fn set_subscriber_firstchp_override(
        &self,
        subscriber_id: Uuid,
        firstchp: Option<u16>,
    ) -> Result<(), String> {
        let mut client = self.client();
        client
            .set_subscriber_firstchp_override(proto::SetSubscriberFirstchpOverrideRequest {
                subscriber_id: subscriber_id.to_string(),
                firstchp_override: firstchp.map(u32::from),
            })
            .await
            .map(|_| ())
            .map_err(|status| format!("HLR SetSubscriberFirstchpOverride: {status}"))
    }

    async fn set_subscriber_a_key(
        &self,
        identity: &MobileIdentityKey,
        a_key: Option<Vec<u8>>,
        ssd_a: Option<Vec<u8>>,
        ssd_b: Option<Vec<u8>>,
        auth_algo: AuthAlgorithm,
    ) -> Result<(), String> {
        let mut client = self.client();
        client
            .set_subscriber_a_key(proto::SetSubscriberAKeyRequest {
                identity: Some(identity_key_to_proto(identity)),
                a_key,
                ssd_a,
                ssd_b,
                auth_algo: auth_algo_to_proto(auth_algo),
                a_key_digits: None,
            })
            .await
            .map(|_| ())
            .map_err(|status| format!("HLR SetSubscriberAKey: {status}"))
    }

    async fn get_subscriber_auth_state(
        &self,
        identity: &MobileIdentityKey,
    ) -> Result<Option<IdentityAuthState>, String> {
        let mut client = self.client();
        let resp = match client
            .get_subscriber_auth_state(proto::GetSubscriberAuthStateRequest {
                identity: Some(identity_key_to_proto(identity)),
            })
            .await
        {
            Ok(resp) => resp.into_inner(),
            Err(status) if status.code() == Code::NotFound => return Ok(None),
            Err(status) => return Err(format!("HLR GetSubscriberAuthState: {status}")),
        };
        Ok(Some(IdentityAuthState {
            subscriber_identity_id: Uuid::parse_str(&resp.subscriber_identity_id)
                .map_err(|e| format!("invalid subscriber_identity_id: {e}"))?,
            credentials_provisioned: resp.credentials_provisioned,
            ssd_provisioned: resp.ssd_provisioned,
            auth_algo: auth_algo_from_proto(resp.auth_algo),
            auth_count: resp.auth_count as u8,
            auth_failure_count: resp.auth_failure_count,
            last_auth_at: resp
                .last_auth_at
                .map(|ts| timestamp_to_datetime(Some(ts)))
                .transpose()?,
        }))
    }

    async fn verify_cave_global_challenge(
        &self,
        identity: &MobileIdentityKey,
        challenge: &GlobalChallenge,
    ) -> Result<CaveVerdict, String> {
        let mut client = self.client();
        let resp = client
            .verify_cave_global_challenge(proto::VerifyCaveGlobalChallengeRequest {
                identity: Some(identity_key_to_proto(identity)),
                esn: challenge.esn,
                rand: challenge.rand,
                authr: challenge.authr,
                count: challenge.count.map(u32::from),
                auth_data: challenge.auth_data,
                generate_call_keys: challenge.generate_call_keys,
                otasp_reauthentication: challenge.otasp_reauthentication,
            })
            .await
            .map_err(|status| format!("HLR VerifyCaveGlobalChallenge: {status}"))?;
        cave_verdict_from_proto(resp.into_inner())
    }

    async fn verify_cave_unique_challenge(
        &self,
        identity: &MobileIdentityKey,
        challenge: &UniqueChallenge,
    ) -> Result<CaveVerdict, String> {
        let mut client = self.client();
        let resp = client
            .verify_cave_unique_challenge(proto::VerifyCaveUniqueChallengeRequest {
                identity: Some(identity_key_to_proto(identity)),
                esn: challenge.esn,
                randu: challenge.randu,
                authu: challenge.authu,
                imsi_s1: challenge.imsi_s1,
                imsi_s2: u32::from(challenge.imsi_s2),
                generate_call_keys: challenge.generate_call_keys,
            })
            .await
            .map_err(|status| format!("HLR VerifyCaveUniqueChallenge: {status}"))?;
        cave_verdict_from_proto(resp.into_inner())
    }

    async fn begin_cave_ssd_update(&self, identity: &MobileIdentityKey) -> Result<u64, String> {
        let mut client = self.client();
        client
            .begin_cave_ssd_update(proto::BeginCaveSsdUpdateRequest {
                identity: Some(identity_key_to_proto(identity)),
            })
            .await
            .map(|resp| resp.into_inner().randssd)
            .map_err(|status| format!("HLR BeginCaveSsdUpdate: {status}"))
    }

    async fn cave_base_station_challenge(
        &self,
        identity: &MobileIdentityKey,
        esn: u32,
        randbs: u32,
        imsi_s1: u32,
    ) -> Result<u32, String> {
        let mut client = self.client();
        client
            .cave_base_station_challenge(proto::CaveBaseStationChallengeRequest {
                identity: Some(identity_key_to_proto(identity)),
                esn,
                randbs,
                imsi_s1,
            })
            .await
            .map(|resp| resp.into_inner().authbs)
            .map_err(|status| format!("HLR CaveBaseStationChallenge: {status}"))
    }

    async fn finish_cave_ssd_update(
        &self,
        identity: &MobileIdentityKey,
        esn: u32,
        confirmed: bool,
    ) -> Result<(), String> {
        let mut client = self.client();
        client
            .finish_cave_ssd_update(proto::FinishCaveSsdUpdateRequest {
                identity: Some(identity_key_to_proto(identity)),
                esn,
                confirmed,
            })
            .await
            .map(|_| ())
            .map_err(|status| format!("HLR FinishCaveSsdUpdate: {status}"))
    }

    async fn increment_cave_count(&self, identity: &MobileIdentityKey) -> Result<u8, String> {
        let mut client = self.client();
        client
            .increment_cave_count(proto::IncrementCaveCountRequest {
                identity: Some(identity_key_to_proto(identity)),
            })
            .await
            .map(|resp| resp.into_inner().auth_count as u8)
            .map_err(|status| format!("HLR IncrementCaveCount: {status}"))
    }

    async fn begin_cave_key_exchange(
        &self,
        identity: &MobileIdentityKey,
        a_key_p_rev: u8,
    ) -> Result<crate::key_exchange::KeyExchangeOffer, String> {
        let mut client = self.client();
        let resp = client
            .begin_cave_key_exchange(proto::BeginCaveKeyExchangeRequest {
                identity: Some(identity_key_to_proto(identity)),
                a_key_p_rev: u32::from(a_key_p_rev),
            })
            .await
            .map_err(|status| format!("HLR BeginCaveKeyExchange: {status}"))?
            .into_inner();
        Ok(crate::key_exchange::KeyExchangeOffer {
            a_key_p_rev,
            params: match (resp.param_p, resp.param_g) {
                (Some(p), Some(g)) => Some((p, g)),
                _ => None,
            },
            bs_result: resp.bs_result,
        })
    }

    async fn complete_cave_key_exchange(
        &self,
        identity: &MobileIdentityKey,
        ms_result: &[u8],
    ) -> Result<(), String> {
        let mut client = self.client();
        client
            .complete_cave_key_exchange(proto::CompleteCaveKeyExchangeRequest {
                identity: Some(identity_key_to_proto(identity)),
                ms_result: ms_result.to_vec(),
            })
            .await
            .map(|_| ())
            .map_err(|status| format!("HLR CompleteCaveKeyExchange: {status}"))
    }

    async fn commit_cave_key(&self, identity: &MobileIdentityKey) -> Result<(), String> {
        let mut client = self.client();
        client
            .commit_cave_key(proto::CaveKeyIdentityRequest {
                identity: Some(identity_key_to_proto(identity)),
            })
            .await
            .map(|_| ())
            .map_err(|status| format!("HLR CommitCaveKey: {status}"))
    }

    async fn abort_cave_key_exchange(&self, identity: &MobileIdentityKey) -> Result<(), String> {
        let mut client = self.client();
        client
            .abort_cave_key_exchange(proto::CaveKeyIdentityRequest {
                identity: Some(identity_key_to_proto(identity)),
            })
            .await
            .map(|_| ())
            .map_err(|status| format!("HLR AbortCaveKeyExchange: {status}"))
    }

    async fn save_otasp_session(&self, row: &OtaspSessionRow) -> Result<(), String> {
        let mut client = self.client();
        let summary = otasp_session_summary_to_proto(row);
        client
            .save_otasp_session(proto::SaveOtaspSessionRequest {
                summary: Some(summary),
                events_proto: row.events_proto.clone(),
            })
            .await
            .map(|_| ())
            .map_err(|status| format!("HLR SaveOtaspSession: {status}"))
    }

    async fn list_otasp_sessions(
        &self,
        filter: OtaspSessionFilter,
        limit: u32,
        offset: u32,
    ) -> Result<(Vec<OtaspSessionRow>, u32), String> {
        let mut client = self.client();
        let resp = client
            .list_otasp_sessions(proto::ListOtaspSessionsRequest {
                subscriber_id: filter.subscriber_id.map(|u| u.to_string()),
                esn: filter.esn,
                meid: filter.meid,
                limit,
                offset,
            })
            .await
            .map_err(|status| format!("HLR ListOtaspSessions: {status}"))?;
        let r = resp.into_inner();
        // ListOtaspSessions returns summaries only — no events_proto.
        // Callers that want the timeline must call get_otasp_session.
        let rows = r
            .sessions
            .into_iter()
            .map(|s| otasp_session_summary_from_proto(s, Vec::new()))
            .collect::<Result<Vec<_>, _>>()?;
        Ok((rows, r.total))
    }

    async fn get_otasp_session(&self, session_id: Uuid) -> Result<Option<OtaspSessionRow>, String> {
        let mut client = self.client();
        match client
            .get_otasp_session(proto::GetOtaspSessionRequest {
                session_id: session_id.to_string(),
            })
            .await
        {
            Ok(resp) => {
                let detail = match resp.into_inner().session {
                    Some(d) => d,
                    None => return Ok(None),
                };
                let summary = match detail.summary {
                    Some(s) => s,
                    None => return Err("HLR GetOtaspSession: response missing summary".into()),
                };
                // Detail events live in the response as decoded records;
                // re-encode them so the row carries the same blob shape
                // the Postgres path produces.
                use crate::proto_root::events::v1 as events_proto;
                use prost::Message;
                let wrap = events_proto::OtaspRecordedEvents {
                    events: detail.events,
                };
                let mut buf = Vec::with_capacity(wrap.encoded_len());
                wrap.encode(&mut buf)
                    .map_err(|e| format!("re-encode OtaspRecordedEvents: {e}"))?;
                otasp_session_summary_from_proto(summary, buf).map(Some)
            }
            Err(status) if status.code() == Code::NotFound => Ok(None),
            Err(status) => Err(format!("HLR GetOtaspSession: {status}")),
        }
    }
}

// ─── PostgreSQL Implementation ─────────────────────────────────

pub struct PostgresHlrRepository {
    pool: sqlx::PgPool,
    /// Secret exponents of running OTASP key exchanges, keyed by identity row. They are
    /// never persisted, so an HLR restart abandons the exchange.
    key_exchanges:
        std::sync::Mutex<std::collections::HashMap<Uuid, crate::key_exchange::KeyExchange>>,
}

impl PostgresHlrRepository {
    async fn load_cave_credentials(
        &self,
        identity: &MobileIdentityKey,
    ) -> Result<(Uuid, CaveCredentials), String> {
        let Some(identity_id) = resolve_identity_row_id(&self.pool, identity).await? else {
            return Err(format!("{VALIDATION_FAILED_PREFIX}identity not found"));
        };
        let row = sqlx::query_as::<_, CaveCredentialsRow>(
            r#"
            SELECT a_key, ssd_a, ssd_b, auth_count, auth_algo, randssd,
                pending_a_key, pending_ssd_a, pending_ssd_b
            FROM subscriber_identities
            WHERE subscriber_identity_id = $1
            "#,
        )
        .bind(identity_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| format!("load_cave_credentials: {e}"))?;
        Ok((identity_id, row.try_into()?))
    }

    async fn record_cave_outcome(
        &self,
        identity_id: Uuid,
        outcome: CaveAuthOutcome,
    ) -> Result<(), String> {
        let sql = if outcome.is_success() {
            "UPDATE subscriber_identities SET last_auth_at = NOW(), auth_failure_count = 0 \
             WHERE subscriber_identity_id = $1"
        } else {
            "UPDATE subscriber_identities SET auth_failure_count = auth_failure_count + 1 \
             WHERE subscriber_identity_id = $1"
        };
        sqlx::query(sql)
            .bind(identity_id)
            .execute(&self.pool)
            .await
            .map_err(|e| format!("record_cave_outcome: {e}"))?;
        Ok(())
    }

    pub fn new(pool: sqlx::PgPool) -> Self {
        Self {
            pool,
            key_exchanges: Default::default(),
        }
    }

    pub async fn connect_from_config(config: &crate::HlrNodeConfig) -> Result<Self, String> {
        let dsn = config
            .postgres_dsn
            .as_deref()
            .ok_or("HLR postgres_dsn is not configured; set HLR_POSTGRES_DSN or add postgres_dsn to hlr.json")?;
        let pool = sqlx::postgres::PgPoolOptions::new()
            .after_connect(|conn, _meta| {
                Box::pin(async move {
                    sqlx::Executor::execute(conn, "SET search_path TO hlr, public")
                        .await
                        .map(|_| ())
                })
            })
            .connect(dsn)
            .await
            .map_err(|e| format_postgres_connect_error("HLR", &e))?;
        let repo = Self::new(pool);
        repo.run_migrations()
            .await
            .map_err(|e| format!("HLR migrations failed: {e}"))?;
        Ok(repo)
    }

    /// Build a `ResolvedSubscriber` by fetching the primary identity and
    /// current registration binding (if any) for a subscriber that we
    /// already loaded. This is the in-process equivalent of the bundle
    /// the gRPC service returns to callers — keeps Postgres callers
    /// (BSC/MSC/nib via in-process repo) symmetric with the gRPC path.
    async fn assemble_resolved(
        &self,
        subscriber: Subscriber,
        subscriber_id: Uuid,
    ) -> Result<ResolvedSubscriber, String> {
        let identities = self.get_identities_for_subscriber(subscriber_id).await?;
        let primary_identity = identities.iter().find(|i| i.is_primary).cloned();
        let binding = self.get_registration_binding(subscriber_id).await?;
        Ok(ResolvedSubscriber {
            subscriber,
            identities,
            primary_identity,
            binding,
        })
    }

    pub async fn run_migrations(&self) -> Result<(), String> {
        sqlx::query("CREATE SCHEMA IF NOT EXISTS hlr")
            .execute(&self.pool)
            .await
            .map_err(|e| format!("create schema hlr: {e}"))?;
        let mut migrator = sqlx::migrate!("./migrations");
        migrator.set_ignore_missing(true);
        migrator
            .run(&self.pool)
            .await
            .map_err(|e| format!("migration error: {e}"))?;
        Ok(())
    }
}

const RINGTONE_MAX_BYTES_PER_CODEC: usize = 256 * 1024;

fn voice_codec_to_str(codec: cdma_voice::VoiceCodec) -> &'static str {
    match codec {
        cdma_voice::VoiceCodec::Tia96 => "tia96",
        cdma_voice::VoiceCodec::EvrcA => "evrc_a",
        cdma_voice::VoiceCodec::EvrcB => "evrc_b",
        cdma_voice::VoiceCodec::EvrcWb => "evrc_wb",
        cdma_voice::VoiceCodec::Qcelp13k => "qcelp_13k",
    }
}

fn format_postgres_connect_error(component: &str, error: &sqlx::Error) -> String {
    format!(
        "failed to connect to {component} database: {error}; ensure PostgreSQL is running and reachable (default dev database: `docker compose up -d postgres`)"
    )
}

#[derive(sqlx::FromRow)]
struct MobileSeenIdentityRow {
    id: Uuid,
    last_seen_at: DateTime<Utc>,
}

/// Soft uniqueness check: refuse to assign an ESN or MEID already
/// owned by a different subscriber. Schema-level enforcement is on
/// the follow-up list; this is the app-side guard.
///
/// Returns an error prefixed with `VALIDATION_FAILED:` so the gRPC
/// layer maps it to `INVALID_ARGUMENT` and the web UI returns a real
/// 400 with the offending hardware ID.
async fn check_hardware_identity_unique(
    pool: &sqlx::PgPool,
    subscriber_id: Uuid,
    esn: Option<u32>,
    meid: Option<&str>,
) -> Result<(), String> {
    if let Some(esn) = esn {
        let row: Option<(Uuid,)> = sqlx::query_as(
            "SELECT subscriber_id FROM subscriber_identities \
             WHERE esn = $1 AND subscriber_id <> $2 LIMIT 1",
        )
        .bind(esn as i64)
        .bind(subscriber_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| format!("check_hardware_identity_unique esn: {e}"))?;
        if row.is_some() {
            return Err(format!(
                "{VALIDATION_FAILED_PREFIX}ESN 0x{esn:08X} is already assigned to another subscriber"
            ));
        }
    }
    if let Some(meid) = meid {
        let row: Option<(Uuid,)> = sqlx::query_as(
            "SELECT subscriber_id FROM subscriber_identities \
             WHERE meid = $1 AND subscriber_id <> $2 LIMIT 1",
        )
        .bind(meid)
        .bind(subscriber_id)
        .fetch_optional(pool)
        .await
        .map_err(|e| format!("check_hardware_identity_unique meid: {e}"))?;
        if row.is_some() {
            return Err(format!(
                "{VALIDATION_FAILED_PREFIX}MEID {meid} is already assigned to another subscriber"
            ));
        }
    }
    Ok(())
}

pub(crate) const VALIDATION_FAILED_PREFIX: &str = "VALIDATION_FAILED: ";

async fn resolve_identity_row_id(
    pool: &sqlx::PgPool,
    identity: &MobileIdentityKey,
) -> Result<Option<Uuid>, String> {
    let row: Option<(Uuid,)> = match identity {
        MobileIdentityKey::ImsiEsn { imsi, esn } => sqlx::query_as(
            "SELECT subscriber_identity_id FROM subscriber_identities \
             WHERE imsi = $1 AND esn = $2 LIMIT 1",
        )
        .bind(imsi)
        .bind(*esn as i64)
        .fetch_optional(pool)
        .await
        .map_err(|e| format!("resolve_identity_row_id: {e}"))?,
        MobileIdentityKey::ImsiMeid { imsi, meid } => sqlx::query_as(
            "SELECT subscriber_identity_id FROM subscriber_identities \
             WHERE imsi = $1 AND meid = $2 LIMIT 1",
        )
        .bind(imsi)
        .bind(meid)
        .fetch_optional(pool)
        .await
        .map_err(|e| format!("resolve_identity_row_id: {e}"))?,
        MobileIdentityKey::ImsiEsnMeid { imsi, esn, meid } => sqlx::query_as(
            "SELECT subscriber_identity_id FROM subscriber_identities \
             WHERE imsi = $1 AND (esn = $2 OR esn IS NULL) \
               AND (meid = $3 OR meid IS NULL) \
               AND (esn IS NOT NULL OR meid IS NOT NULL) LIMIT 1",
        )
        .bind(imsi)
        .bind(i64::from(*esn))
        .bind(meid)
        .fetch_optional(pool)
        .await
        .map_err(|error| format!("resolve_identity_row_id: {error}"))?,
    };
    Ok(row.map(|r| r.0))
}

async fn select_mobile_seen_by_key(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    identity: &MobileIdentityKey,
) -> Result<Option<MobileSeenIdentityRow>, String> {
    match identity {
        MobileIdentityKey::ImsiEsn { imsi, esn } => sqlx::query_as::<_, MobileSeenIdentityRow>(
            r#"
            SELECT id, last_seen_at
            FROM mobiles_seen
            WHERE imsi = $1 AND esn = $2
              AND (meid IS NULL OR (
                  SELECT COUNT(*) FROM mobiles_seen WHERE imsi = $1 AND esn = $2
              ) = 1)
            ORDER BY meid NULLS FIRST
            LIMIT 1
            "#,
        )
        .bind(imsi)
        .bind(*esn as i64)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|e| format!("select_mobile_seen imsi+esn: {e}")),
        MobileIdentityKey::ImsiMeid { imsi, meid } => sqlx::query_as::<_, MobileSeenIdentityRow>(
            r#"
            SELECT id, last_seen_at
            FROM mobiles_seen
            WHERE imsi = $1 AND esn IS NULL AND meid = $2
            LIMIT 1
            "#,
        )
        .bind(imsi)
        .bind(meid)
        .fetch_optional(&mut **tx)
        .await
        .map_err(|e| format!("select_mobile_seen imsi+meid: {e}")),
        MobileIdentityKey::ImsiEsnMeid { imsi, esn, meid } => {
            sqlx::query_as::<_, MobileSeenIdentityRow>(
                r#"
                SELECT id, last_seen_at
                FROM mobiles_seen
                WHERE imsi = $1 AND esn = $2 AND (meid = $3 OR meid IS NULL)
                ORDER BY meid NULLS LAST
                LIMIT 1
                "#,
            )
            .bind(imsi)
            .bind(*esn as i64)
            .bind(meid)
            .fetch_optional(&mut **tx)
            .await
            .map_err(|e| format!("select_mobile_seen imsi+esn+meid: {e}"))
        }
    }
}

async fn select_legacy_mobile_seen_by_imsi(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    imsi: &str,
) -> Result<Option<MobileSeenIdentityRow>, String> {
    sqlx::query_as::<_, MobileSeenIdentityRow>(
        r#"
        SELECT id, last_seen_at
        FROM mobiles_seen
        WHERE imsi = $1 AND esn IS NULL AND meid IS NULL
        LIMIT 1
        "#,
    )
    .bind(imsi)
    .fetch_optional(&mut **tx)
    .await
    .map_err(|e| format!("select legacy mobile_seen by imsi: {e}"))
}

async fn update_mobile_seen_row(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    id: Uuid,
    mob_p_rev: Option<u8>,
    identity: &MobileIdentityKey,
) -> Result<(), String> {
    sqlx::query(
        r#"
        UPDATE mobiles_seen
        SET imsi = $2,
            esn = $3,
            meid = COALESCE($4, meid),
            mob_p_rev = COALESCE($5, mob_p_rev),
            last_seen_at = NOW()
        WHERE id = $1
        "#,
    )
    .bind(id)
    .bind(identity.imsi())
    .bind(identity.esn().map(|v| v as i64))
    .bind(identity.meid())
    .bind(mob_p_rev.map(|v| v as i32))
    .execute(&mut **tx)
    .await
    .map(|_| ())
    .map_err(|e| format!("update_mobile_seen row: {e}"))
}

async fn delete_legacy_mobile_seen(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    imsi: &str,
    except_id: Option<Uuid>,
) -> Result<(), String> {
    sqlx::query(
        r#"
        DELETE FROM mobiles_seen
        WHERE imsi = $1
          AND esn IS NULL
          AND meid IS NULL
          AND ($2::uuid IS NULL OR id <> $2)
        "#,
    )
    .bind(imsi)
    .bind(except_id)
    .execute(&mut **tx)
    .await
    .map(|_| ())
    .map_err(|e| format!("delete legacy mobile_seen: {e}"))
}

async fn insert_mobile_seen(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    mob_p_rev: Option<u8>,
    identity: &MobileIdentityKey,
) -> Result<(), String> {
    sqlx::query(
        r#"
        INSERT INTO mobiles_seen (imsi, esn, meid, mob_p_rev)
        VALUES ($1, $2, $3, $4)
        "#,
    )
    .bind(identity.imsi())
    .bind(identity.esn().map(|v| v as i64))
    .bind(identity.meid())
    .bind(mob_p_rev.map(|v| v as i32))
    .execute(&mut **tx)
    .await
    .map(|_| ())
    .map_err(|e| format!("insert mobile_seen: {e}"))
}

#[async_trait]
impl HlrRepository for PostgresHlrRepository {
    async fn upsert_subscriber(
        &self,
        phone_number: &str,
        display_name: &str,
        status: &str,
        number_type: NumberType,
        number_plan: NumberPlan,
    ) -> Result<Subscriber, String> {
        let now = Utc::now();
        let id = Uuid::new_v4();
        let row = sqlx::query_as::<_, SubscriberRow>(
            r#"
            WITH up AS (
                INSERT INTO subscribers (
                    subscriber_id, phone_number, display_name, status,
                    created_at, updated_at, number_type, number_plan
                )
                VALUES ($1, $2, $3, $4, $5, $5, $6, $7)
                ON CONFLICT (phone_number) DO UPDATE SET
                    display_name = EXCLUDED.display_name,
                    status = EXCLUDED.status,
                    updated_at = EXCLUDED.updated_at,
                    number_type = EXCLUDED.number_type,
                    number_plan = EXCLUDED.number_plan
                RETURNING subscriber_id, phone_number, display_name, status,
                    created_at, updated_at, number_type, number_plan, prl_override_id, service_programming_code, firstchp_override
            )
            SELECT up.*,
                EXISTS (SELECT 1 FROM subscriber_ringtones r WHERE r.subscriber_id = up.subscriber_id) AS has_ringtone,
                (SELECT MIN(duration_ms) FROM subscriber_ringtones r WHERE r.subscriber_id = up.subscriber_id) AS ringtone_duration_ms
            FROM up
            "#,
        )
        .bind(id)
        .bind(phone_number)
        .bind(display_name)
        .bind(status)
        .bind(now)
        .bind(number_type)
        .bind(number_plan)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| format!("upsert_subscriber: {e}"))?;
        Ok(row.try_into()?)
    }

    async fn get_subscriber_by_phone_number(
        &self,
        phone_number: &str,
    ) -> Result<Option<ResolvedSubscriber>, String> {
        let row = sqlx::query_as::<_, SubscriberRow>(
            r#"
            SELECT s.subscriber_id, s.phone_number, s.display_name, s.status, s.created_at, s.updated_at,
                s.number_type, s.number_plan, s.prl_override_id, s.service_programming_code, s.firstchp_override,
                EXISTS (SELECT 1 FROM subscriber_ringtones r WHERE r.subscriber_id = s.subscriber_id) AS has_ringtone,
                (SELECT MIN(duration_ms) FROM subscriber_ringtones r WHERE r.subscriber_id = s.subscriber_id) AS ringtone_duration_ms
            FROM subscribers s WHERE s.phone_number = $1
            "#,
        )
        .bind(phone_number)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| format!("get_subscriber_by_phone_number: {e}"))?;
        let Some(row) = row else {
            return Ok(None);
        };
        let subscriber: Subscriber = row.try_into()?;
        let sid = subscriber.subscriber_id;
        Ok(Some(self.assemble_resolved(subscriber, sid).await?))
    }

    async fn get_subscriber_by_id(
        &self,
        subscriber_id: Uuid,
    ) -> Result<Option<ResolvedSubscriber>, String> {
        let row = sqlx::query_as::<_, SubscriberRow>(
            r#"
            SELECT s.subscriber_id, s.phone_number, s.display_name, s.status, s.created_at, s.updated_at,
                s.number_type, s.number_plan, s.prl_override_id, s.service_programming_code, s.firstchp_override,
                EXISTS (SELECT 1 FROM subscriber_ringtones r WHERE r.subscriber_id = s.subscriber_id) AS has_ringtone,
                (SELECT MIN(duration_ms) FROM subscriber_ringtones r WHERE r.subscriber_id = s.subscriber_id) AS ringtone_duration_ms
            FROM subscribers s WHERE s.subscriber_id = $1
            "#,
        )
        .bind(subscriber_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| format!("get_subscriber_by_id: {e}"))?;
        let Some(row) = row else {
            return Ok(None);
        };
        let subscriber: Subscriber = row.try_into()?;
        Ok(Some(
            self.assemble_resolved(subscriber, subscriber_id).await?,
        ))
    }

    async fn update_subscriber(
        &self,
        subscriber_id: Uuid,
        phone_number: &str,
        display_name: &str,
        status: &str,
        number_type: NumberType,
        number_plan: NumberPlan,
    ) -> Result<Option<Subscriber>, String> {
        let now = Utc::now();
        let row = sqlx::query_as::<_, SubscriberRow>(
            r#"
            WITH up AS (
                UPDATE subscribers
                SET phone_number = $2,
                    display_name = $3,
                    status = $4,
                    updated_at = $5,
                    number_type = $6,
                    number_plan = $7
                WHERE subscriber_id = $1
                RETURNING subscriber_id, phone_number, display_name, status,
                    created_at, updated_at, number_type, number_plan, prl_override_id, service_programming_code, firstchp_override
            )
            SELECT up.*,
                EXISTS (SELECT 1 FROM subscriber_ringtones r WHERE r.subscriber_id = up.subscriber_id) AS has_ringtone,
                (SELECT MIN(duration_ms) FROM subscriber_ringtones r WHERE r.subscriber_id = up.subscriber_id) AS ringtone_duration_ms
            FROM up
            "#,
        )
        .bind(subscriber_id)
        .bind(phone_number)
        .bind(display_name)
        .bind(status)
        .bind(now)
        .bind(number_type)
        .bind(number_plan)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| format!("update_subscriber: {e}"))?;
        row.map(TryInto::try_into).transpose()
    }

    async fn list_subscribers(
        &self,
        limit: u32,
        offset: u32,
    ) -> Result<(Vec<Subscriber>, u32), String> {
        let total: (i64,) = sqlx::query_as("SELECT COUNT(*) FROM subscribers")
            .fetch_one(&self.pool)
            .await
            .map_err(|e| format!("list_subscribers count: {e}"))?;
        let rows = sqlx::query_as::<_, SubscriberRow>(
            r#"
            SELECT s.subscriber_id, s.phone_number, s.display_name, s.status, s.created_at, s.updated_at,
                s.number_type, s.number_plan, s.prl_override_id, s.service_programming_code, s.firstchp_override,
                EXISTS (SELECT 1 FROM subscriber_ringtones r WHERE r.subscriber_id = s.subscriber_id) AS has_ringtone,
                (SELECT MIN(duration_ms) FROM subscriber_ringtones r WHERE r.subscriber_id = s.subscriber_id) AS ringtone_duration_ms
            FROM subscribers s ORDER BY s.created_at DESC LIMIT $1 OFFSET $2
            "#,
        )
        .bind(limit as i64)
        .bind(offset as i64)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| format!("list_subscribers: {e}"))?;
        let subscribers: Result<Vec<Subscriber>, String> =
            rows.into_iter().map(TryInto::try_into).collect();
        Ok((subscribers?, total.0 as u32))
    }

    async fn delete_subscriber(&self, subscriber_id: Uuid) -> Result<bool, String> {
        let result = sqlx::query("DELETE FROM subscribers WHERE subscriber_id = $1")
            .bind(subscriber_id)
            .execute(&self.pool)
            .await
            .map_err(|e| format!("delete_subscriber: {e}"))?;
        Ok(result.rows_affected() > 0)
    }

    async fn upsert_identity(
        &self,
        subscriber_id: Uuid,
        imsi: Option<&str>,
        esn: Option<u32>,
        meid: Option<&str>,
    ) -> Result<SubscriberIdentity, String> {
        let identity_key = MobileIdentityKey::from_parts(imsi, esn, meid)?;
        let imsi = identity_key.imsi();
        let esn = identity_key.esn();
        let meid = identity_key.meid();
        let now = Utc::now();
        let id = Uuid::new_v4();
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| format!("upsert_identity begin: {e}"))?;

        check_hardware_identity_unique(&self.pool, subscriber_id, esn, meid).await?;

        let existing = sqlx::query_as::<_, IdentityRow>(
            "SELECT subscriber_identity_id, subscriber_id, imsi, esn, meid, is_primary, created_at FROM subscriber_identities WHERE subscriber_id = $1 LIMIT 1",
        )
        .bind(subscriber_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| format!("upsert_identity lookup: {e}"))?;

        let row = if let Some(existing) = existing {
            sqlx::query_as::<_, IdentityRow>(
                r#"
                UPDATE subscriber_identities SET
                    imsi = $2,
                    esn = $3,
                    meid = $4
                WHERE subscriber_identity_id = $1
                RETURNING subscriber_identity_id, subscriber_id, imsi, esn, meid, is_primary, created_at
                "#,
            )
            .bind(existing.subscriber_identity_id)
            .bind(imsi)
            .bind(esn.map(|v| v as i64))
            .bind(meid)
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| format!("upsert_identity update: {e}"))?
        } else {
            let is_primary = true;
            sqlx::query_as::<_, IdentityRow>(
                r#"
                INSERT INTO subscriber_identities (subscriber_identity_id, subscriber_id, imsi, esn, meid, is_primary, created_at)
                VALUES ($1, $2, $3, $4, $5, $6, $7)
                RETURNING subscriber_identity_id, subscriber_id, imsi, esn, meid, is_primary, created_at
                "#,
            )
            .bind(id)
            .bind(subscriber_id)
            .bind(imsi)
            .bind(esn.map(|v| v as i64))
            .bind(meid)
            .bind(is_primary)
            .bind(now)
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| format!("upsert_identity insert: {e}"))?
        };

        tx.commit()
            .await
            .map_err(|e| format!("upsert_identity commit: {e}"))?;
        Ok(row.into())
    }

    async fn replace_primary_identity(
        &self,
        subscriber_id: Uuid,
        imsi: Option<&str>,
        esn: Option<u32>,
        meid: Option<&str>,
    ) -> Result<SubscriberIdentity, String> {
        let identity_key = MobileIdentityKey::from_parts(imsi, esn, meid)?;
        let imsi = identity_key.imsi();
        let esn = identity_key.esn();
        let meid = identity_key.meid();
        let now = Utc::now();
        let id = Uuid::new_v4();
        check_hardware_identity_unique(&self.pool, subscriber_id, esn, meid).await?;
        let existing = sqlx::query_as::<_, IdentityRow>(
            r#"
            SELECT subscriber_identity_id, subscriber_id, imsi, esn, meid, is_primary, created_at
            FROM subscriber_identities
            WHERE subscriber_id = $1
            ORDER BY is_primary DESC, created_at ASC
            LIMIT 1
            "#,
        )
        .bind(subscriber_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| format!("replace_primary_identity lookup: {e}"))?;

        if let Some(existing) = existing {
            let row = sqlx::query_as::<_, IdentityRow>(
                r#"
                UPDATE subscriber_identities
                SET imsi = $2,
                    esn = $3,
                    meid = $4,
                    is_primary = true
                WHERE subscriber_identity_id = $1
                RETURNING subscriber_identity_id, subscriber_id, imsi, esn, meid, is_primary, created_at
                "#,
            )
            .bind(existing.subscriber_identity_id)
            .bind(imsi)
            .bind(esn.map(|v| v as i64))
            .bind(meid)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| format!("replace_primary_identity update: {e}"))?;
            Ok(row.into())
        } else {
            let row = sqlx::query_as::<_, IdentityRow>(
                r#"
                INSERT INTO subscriber_identities (
                    subscriber_identity_id, subscriber_id, imsi, esn, meid, is_primary, created_at
                )
                VALUES ($1, $2, $3, $4, $5, true, $6)
                RETURNING subscriber_identity_id, subscriber_id, imsi, esn, meid, is_primary, created_at
                "#,
            )
            .bind(id)
            .bind(subscriber_id)
            .bind(imsi)
            .bind(esn.map(|v| v as i64))
            .bind(meid)
            .bind(now)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| format!("replace_primary_identity insert: {e}"))?;
            Ok(row.into())
        }
    }

    async fn get_identities_for_subscriber(
        &self,
        subscriber_id: Uuid,
    ) -> Result<Vec<SubscriberIdentity>, String> {
        let rows = sqlx::query_as::<_, IdentityRow>(
            "SELECT subscriber_identity_id, subscriber_id, imsi, esn, meid, is_primary, created_at FROM subscriber_identities WHERE subscriber_id = $1 ORDER BY is_primary DESC, created_at ASC",
        )
        .bind(subscriber_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| format!("get_identities: {e}"))?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    async fn resolve_by_identity(
        &self,
        identity: &MobileIdentityKey,
    ) -> Result<Option<ResolvedSubscriber>, String> {
        let row = match identity {
            MobileIdentityKey::ImsiEsn { imsi, esn } => sqlx::query_as::<_, SubscriberRow>(
                r#"
                SELECT s.subscriber_id, s.phone_number, s.display_name, s.status, s.created_at, s.updated_at,
                    s.number_type, s.number_plan, s.prl_override_id, s.service_programming_code, s.service_programming_code, s.prl_override_id, s.firstchp_override,
                    EXISTS (SELECT 1 FROM subscriber_ringtones r WHERE r.subscriber_id = s.subscriber_id) AS has_ringtone,
                    (SELECT MIN(duration_ms) FROM subscriber_ringtones r WHERE r.subscriber_id = s.subscriber_id) AS ringtone_duration_ms
                FROM subscribers s
                JOIN subscriber_identities i ON s.subscriber_id = i.subscriber_id
                WHERE i.imsi = $1 AND i.esn = $2
                LIMIT 1
                "#,
            )
            .bind(imsi)
            .bind(*esn as i64)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| format!("resolve_by_identity imsi+esn: {e}"))?,
            MobileIdentityKey::ImsiMeid { imsi, meid } => sqlx::query_as::<_, SubscriberRow>(
                r#"
                SELECT s.subscriber_id, s.phone_number, s.display_name, s.status, s.created_at, s.updated_at,
                    s.number_type, s.number_plan, s.prl_override_id, s.service_programming_code, s.service_programming_code, s.prl_override_id, s.firstchp_override,
                    EXISTS (SELECT 1 FROM subscriber_ringtones r WHERE r.subscriber_id = s.subscriber_id) AS has_ringtone,
                    (SELECT MIN(duration_ms) FROM subscriber_ringtones r WHERE r.subscriber_id = s.subscriber_id) AS ringtone_duration_ms
                FROM subscribers s
                JOIN subscriber_identities i ON s.subscriber_id = i.subscriber_id
                WHERE i.imsi = $1 AND i.meid = $2
                LIMIT 1
                "#,
            )
            .bind(imsi)
            .bind(meid)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| format!("resolve_by_identity imsi+meid: {e}"))?,
            MobileIdentityKey::ImsiEsnMeid { .. } => {
                let Some(identity_id) = resolve_identity_row_id(&self.pool, identity).await? else {
                    return Ok(None);
                };
                sqlx::query_as::<_, SubscriberRow>(
                    r#"
                    SELECT s.subscriber_id, s.phone_number, s.display_name, s.status, s.created_at, s.updated_at,
                        s.number_type, s.number_plan, s.prl_override_id, s.service_programming_code, s.service_programming_code, s.prl_override_id, s.firstchp_override,
                        EXISTS (SELECT 1 FROM subscriber_ringtones r WHERE r.subscriber_id = s.subscriber_id) AS has_ringtone,
                        (SELECT MIN(duration_ms) FROM subscriber_ringtones r WHERE r.subscriber_id = s.subscriber_id) AS ringtone_duration_ms
                    FROM subscribers s
                    JOIN subscriber_identities i ON s.subscriber_id = i.subscriber_id
                    WHERE i.subscriber_identity_id = $1
                    LIMIT 1
                    "#,
                )
                .bind(identity_id)
                .fetch_optional(&self.pool)
                .await
                .map_err(|e| format!("resolve_by_identity imsi+esn+meid: {e}"))?
            }
        };
        let Some(row) = row else { return Ok(None) };
        let subscriber: Subscriber = row.try_into()?;
        let sid = subscriber.subscriber_id;
        Ok(Some(self.assemble_resolved(subscriber, sid).await?))
    }

    async fn resolve_registered_imsi(
        &self,
        imsi: &str,
        serving_bs_id: &str,
    ) -> Result<Option<ResolvedSubscriber>, tonic::Status> {
        validate_imsi(imsi).map_err(tonic::Status::invalid_argument)?;
        if serving_bs_id.trim().is_empty() {
            return Err(tonic::Status::invalid_argument("serving node is required"));
        }
        const AMBIGUITY_DETECTION_LIMIT: i64 = 2;
        let ids = sqlx::query_scalar::<_, Uuid>(
            r#"
            SELECT DISTINCT b.subscriber_id
            FROM registration_bindings b
            JOIN subscriber_identities i ON i.subscriber_id = b.subscriber_id AND i.imsi = b.imsi
            WHERE b.imsi = $1 AND b.serving_bs_id = $2
              AND b.state = ANY($3)
              AND (b.esn IS NOT NULL OR b.meid IS NOT NULL)
              AND (b.esn IS NULL OR b.esn = i.esn
                   OR (i.esn IS NULL AND b.meid = i.meid))
              AND (b.meid IS NULL OR b.meid = i.meid
                   OR (i.meid IS NULL AND b.esn = i.esn))
            LIMIT $4
            "#,
        )
        .bind(imsi)
        .bind(serving_bs_id)
        .bind(
            [
                RegistrationState::Registered.as_str(),
                RegistrationState::Paged.as_str(),
                RegistrationState::PageResponseReceived.as_str(),
            ]
            .as_slice(),
        )
        .bind(AMBIGUITY_DETECTION_LIMIT)
        .fetch_all(&self.pool)
        .await
        .map_err(|error| {
            log::error!("HLR registered IMSI lookup failed: {error}");
            tonic::Status::internal("registered IMSI lookup failed")
        })?;
        if ids.len() > 1 {
            return Err(tonic::Status::failed_precondition(
                "multiple subscribers share this registered IMSI",
            ));
        }
        let Some(id) = ids.first() else {
            return Ok(None);
        };
        self.get_subscriber_by_id(*id).await.map_err(|error| {
            log::error!("HLR registered subscriber lookup failed: {error}");
            tonic::Status::internal("registered subscriber lookup failed")
        })
    }

    async fn resolve_by_hardware_identity(
        &self,
        esn: Option<u32>,
        meid: Option<&str>,
    ) -> Result<Option<ResolvedSubscriber>, String> {
        if esn.is_none() && meid.is_none() {
            return Err(
                "resolve_by_hardware_identity requires at least one of ESN or MEID".to_string(),
            );
        }
        let meid = meid.map(normalize_meid).transpose()?;
        let rows = sqlx::query_as::<_, SubscriberRow>(
            r#"
            SELECT DISTINCT s.subscriber_id, s.phone_number, s.display_name, s.status, s.created_at, s.updated_at,
                s.number_type, s.number_plan, s.prl_override_id, s.service_programming_code, s.firstchp_override,
                EXISTS (SELECT 1 FROM subscriber_ringtones r WHERE r.subscriber_id = s.subscriber_id) AS has_ringtone,
                (SELECT MIN(duration_ms) FROM subscriber_ringtones r WHERE r.subscriber_id = s.subscriber_id) AS ringtone_duration_ms
            FROM subscribers s
            JOIN subscriber_identities i ON s.subscriber_id = i.subscriber_id
            WHERE ($1::bigint IS NOT NULL AND i.esn = $1)
               OR ($2::text   IS NOT NULL AND i.meid = $2)
            LIMIT 2
            "#,
        )
        .bind(esn.map(|v| v as i64))
        .bind(meid.as_deref())
        .fetch_all(&self.pool)
        .await
        .map_err(|e| format!("resolve_by_hardware_identity: {e}"))?;
        if rows.len() > 1 {
            return Err(match (esn, meid.as_deref()) {
                (Some(esn), Some(_)) => format!(
                    "ambiguous match: ESN 0x{esn:08X} and MEID resolve to different subscribers"
                ),
                (Some(esn), None) => {
                    format!("ambiguous match: multiple subscribers share ESN 0x{esn:08X}")
                }
                (None, Some(_)) => {
                    "ambiguous match: multiple subscribers share this MEID".to_string()
                }
                (None, None) => unreachable!("guarded by earlier check"),
            });
        }
        let Some(row) = rows.into_iter().next() else {
            return Ok(None);
        };
        let subscriber: Subscriber = row.try_into()?;
        let sid = subscriber.subscriber_id;
        Ok(Some(self.assemble_resolved(subscriber, sid).await?))
    }

    async fn upsert_mobile_seen(
        &self,
        identity: &MobileIdentityKey,
        mob_p_rev: Option<u8>,
    ) -> Result<MobileSeenUpsert, String> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| format!("upsert_mobile_seen begin: {e}"))?;
        if let MobileIdentityKey::ImsiEsnMeid { imsi, esn, meid } = identity {
            sqlx::query(
                "UPDATE registration_bindings SET meid = $3 \
                 WHERE imsi = $1 AND esn = $2 AND (meid IS NULL OR meid = $3)",
            )
            .bind(imsi)
            .bind(i64::from(*esn))
            .bind(meid)
            .execute(&mut *tx)
            .await
            .map_err(|error| format!("upsert_mobile_seen binding MEID: {error}"))?;
        }
        let existing = select_mobile_seen_by_key(&mut tx, identity).await?;
        if let Some(row) = existing {
            update_mobile_seen_row(&mut tx, row.id, mob_p_rev, identity).await?;
            delete_legacy_mobile_seen(&mut tx, identity.imsi(), Some(row.id)).await?;
            tx.commit()
                .await
                .map_err(|e| format!("upsert_mobile_seen commit: {e}"))?;
            return Ok(MobileSeenUpsert {
                is_new: false,
                previous_last_seen_at: Some(row.last_seen_at),
            });
        }

        if let Some(row) = select_legacy_mobile_seen_by_imsi(&mut tx, identity.imsi()).await? {
            update_mobile_seen_row(&mut tx, row.id, mob_p_rev, identity).await?;
            tx.commit()
                .await
                .map_err(|e| format!("upsert_mobile_seen legacy commit: {e}"))?;
            return Ok(MobileSeenUpsert {
                is_new: false,
                previous_last_seen_at: Some(row.last_seen_at),
            });
        }

        insert_mobile_seen(&mut tx, mob_p_rev, identity).await?;
        tx.commit()
            .await
            .map_err(|e| format!("upsert_mobile_seen insert commit: {e}"))?;
        Ok(MobileSeenUpsert {
            is_new: true,
            previous_last_seen_at: None,
        })
    }

    async fn upsert_registration_binding(
        &self,
        binding: RegistrationBinding,
    ) -> Result<RegistrationBinding, String> {
        let row = sqlx::query_as::<_, BindingRow>(
            r#"
            INSERT INTO registration_bindings (
                subscriber_id, serving_bs_id, state, imsi, esn, meid,
                mob_p_rev, pgslot, slot_cycle_index, last_msg_seq,
                last_registered_at, last_seen_at, updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
            ON CONFLICT (subscriber_id) DO UPDATE SET
                serving_bs_id = EXCLUDED.serving_bs_id,
                state = EXCLUDED.state,
                imsi = EXCLUDED.imsi,
                esn = EXCLUDED.esn,
                meid = CASE
                    WHEN registration_bindings.imsi IS NOT DISTINCT FROM EXCLUDED.imsi
                     AND registration_bindings.esn IS NOT DISTINCT FROM EXCLUDED.esn
                    THEN COALESCE(EXCLUDED.meid, registration_bindings.meid)
                    ELSE EXCLUDED.meid
                END,
                mob_p_rev = EXCLUDED.mob_p_rev,
                pgslot = EXCLUDED.pgslot,
                slot_cycle_index = EXCLUDED.slot_cycle_index,
                last_msg_seq = EXCLUDED.last_msg_seq,
                last_registered_at = EXCLUDED.last_registered_at,
                last_seen_at = EXCLUDED.last_seen_at,
                updated_at = EXCLUDED.updated_at
            RETURNING subscriber_id, serving_bs_id, state, imsi, esn, meid,
                mob_p_rev, pgslot, slot_cycle_index, last_msg_seq,
                last_registered_at, last_seen_at, updated_at
            "#,
        )
        .bind(binding.subscriber_id)
        .bind(&binding.serving_bs_id)
        .bind(binding.state.as_str())
        .bind(binding.imsi.as_deref())
        .bind(binding.esn.map(|v| v as i64))
        .bind(binding.meid.as_deref())
        .bind(binding.mob_p_rev.map(|v| v as i64))
        .bind(binding.pgslot.map(|v| v as i64))
        .bind(binding.slot_cycle_index.map(|v| v as i64))
        .bind(binding.last_msg_seq.map(|v| v as i64))
        .bind(binding.last_registered_at)
        .bind(binding.last_seen_at)
        .bind(binding.updated_at)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| format!("upsert_registration_binding: {e}"))?;
        row.try_into()
            .map_err(|e| format!("upsert_registration_binding: {e}"))
    }

    async fn get_registration_binding(
        &self,
        subscriber_id: Uuid,
    ) -> Result<Option<RegistrationBinding>, String> {
        let row = sqlx::query_as::<_, BindingRow>(
            r#"
            SELECT subscriber_id, serving_bs_id, state, imsi, esn,
                meid, mob_p_rev, pgslot, slot_cycle_index, last_msg_seq,
                last_registered_at, last_seen_at, updated_at
            FROM registration_bindings WHERE subscriber_id = $1
            "#,
        )
        .bind(subscriber_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| format!("get_registration_binding: {e}"))?;
        row.map(TryInto::try_into)
            .transpose()
            .map_err(|e| format!("get_registration_binding: {e}"))
    }

    async fn set_ringtone(
        &self,
        subscriber_id: Uuid,
        wav_bytes: Vec<u8>,
        original_filename: &str,
    ) -> Result<SetRingtoneOutcome, String> {
        if wav_bytes.is_empty() {
            return Err("set_ringtone: wav_bytes is empty".to_string());
        }
        // Preencoding is CPU-bound; run
        // on a blocking thread so we don't stall the tokio worker.
        let preencoded = tokio::task::spawn_blocking(move || {
            cdma_voice::ringtone_preencode::preencode_wav_all_codecs(
                &wav_bytes,
                RINGTONE_MAX_BYTES_PER_CODEC,
            )
        })
        .await
        .map_err(|e| format!("set_ringtone preencode join: {e}"))?
        .map_err(|e| format!("set_ringtone preencode: {e}"))?;
        if preencoded.is_empty() {
            return Err("set_ringtone: preencode produced no codecs".to_string());
        }
        let duration_ms = preencoded[0].duration_ms as u64;

        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| format!("set_ringtone begin: {e}"))?;
        sqlx::query("DELETE FROM subscriber_ringtones WHERE subscriber_id = $1")
            .bind(subscriber_id)
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("set_ringtone delete: {e}"))?;
        let mut codecs = Vec::with_capacity(preencoded.len());
        for p in &preencoded {
            let codec_str = voice_codec_to_str(p.codec);
            sqlx::query(
                r#"
                INSERT INTO subscriber_ringtones
                    (subscriber_id, codec, encoded_frames, frame_count, duration_ms, original_filename)
                VALUES ($1, $2, $3, $4, $5, $6)
                "#,
            )
            .bind(subscriber_id)
            .bind(codec_str)
            .bind(&p.bytes)
            .bind(p.frame_count as i64)
            .bind(p.duration_ms as i64)
            .bind(original_filename)
            .execute(&mut *tx)
            .await
            .map_err(|e| format!("set_ringtone insert: {e}"))?;
            codecs.push(SetRingtoneCodecOutcome {
                codec: codec_str.to_string(),
                encoded_bytes: p.bytes.len() as u32,
                frame_count: p.frame_count as u64,
            });
        }
        tx.commit()
            .await
            .map_err(|e| format!("set_ringtone commit: {e}"))?;
        Ok(SetRingtoneOutcome {
            codecs,
            duration_ms,
        })
    }

    async fn clear_ringtone(&self, subscriber_id: Uuid) -> Result<(), String> {
        sqlx::query("DELETE FROM subscriber_ringtones WHERE subscriber_id = $1")
            .bind(subscriber_id)
            .execute(&self.pool)
            .await
            .map_err(|e| format!("clear_ringtone: {e}"))?;
        Ok(())
    }

    async fn get_ringtone_codec(
        &self,
        subscriber_id: Uuid,
        codec: &str,
    ) -> Result<Option<SubscriberRingtoneCodecBlob>, String> {
        let row: Option<(Vec<u8>, i64, i64)> = sqlx::query_as(
            r#"
            SELECT encoded_frames, frame_count, duration_ms
            FROM subscriber_ringtones
            WHERE subscriber_id = $1 AND codec = $2
            "#,
        )
        .bind(subscriber_id)
        .bind(codec)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| format!("get_ringtone_codec: {e}"))?;
        Ok(row.map(
            |(encoded_frames, frame_count, duration_ms)| SubscriberRingtoneCodecBlob {
                codec: codec.to_string(),
                encoded_frames,
                frame_count: frame_count as u64,
                duration_ms: duration_ms as u64,
            },
        ))
    }

    // ─── PRL management ─────────────────────────────────────────

    async fn list_prls(
        &self,
        limit: u32,
        offset: u32,
        filter: PrlListFilter,
    ) -> Result<(Vec<Prl>, u32), String> {
        // Build a single dynamic WHERE clause for the optional filters.
        let mut q = sqlx::QueryBuilder::<sqlx::Postgres>::new(
            "SELECT prl_id, name, pr_list_id, sspr_p_rev, is_default, raw_bytes, notes, \
             created_at, updated_at FROM prls WHERE deleted_at IS NULL",
        );
        if let Some(pr_list_id) = filter.pr_list_id {
            q.push(" AND pr_list_id = ").push_bind(pr_list_id as i32);
        }
        if let Some(sspr_p_rev) = filter.sspr_p_rev {
            q.push(" AND sspr_p_rev = ").push_bind(sspr_p_rev as i16);
        }
        q.push(" ORDER BY updated_at DESC, prl_id LIMIT ")
            .push_bind(limit as i64)
            .push(" OFFSET ")
            .push_bind(offset as i64);
        let rows: Vec<PrlRow> = q
            .build_query_as()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| format!("list_prls: {e}"))?;

        let mut count_q = sqlx::QueryBuilder::<sqlx::Postgres>::new(
            "SELECT COUNT(*) FROM prls WHERE deleted_at IS NULL",
        );
        if let Some(pr_list_id) = filter.pr_list_id {
            count_q
                .push(" AND pr_list_id = ")
                .push_bind(pr_list_id as i32);
        }
        if let Some(sspr_p_rev) = filter.sspr_p_rev {
            count_q
                .push(" AND sspr_p_rev = ")
                .push_bind(sspr_p_rev as i16);
        }
        let (total,): (i64,) = count_q
            .build_query_as()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| format!("list_prls count: {e}"))?;
        let prls = rows.into_iter().map(Prl::from).collect();
        Ok((prls, total as u32))
    }

    async fn get_prl(&self, prl_id: Uuid) -> Result<Option<Prl>, String> {
        let row: Option<PrlRow> = sqlx::query_as(
            "SELECT prl_id, name, pr_list_id, sspr_p_rev, is_default, raw_bytes, notes, \
             created_at, updated_at FROM prls WHERE prl_id = $1 AND deleted_at IS NULL",
        )
        .bind(prl_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| format!("get_prl: {e}"))?;
        Ok(row.map(Prl::from))
    }

    async fn get_default_prl(&self) -> Result<Option<Prl>, String> {
        let row: Option<PrlRow> = sqlx::query_as(
            "SELECT prl_id, name, pr_list_id, sspr_p_rev, is_default, raw_bytes, notes, \
             created_at, updated_at FROM prls WHERE is_default = TRUE AND deleted_at IS NULL",
        )
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| format!("get_default_prl: {e}"))?;
        Ok(row.map(Prl::from))
    }

    async fn create_prl(
        &self,
        name: &str,
        raw_bytes: &[u8],
        pr_list_id: i32,
        sspr_p_rev: i16,
        notes: &str,
    ) -> Result<Prl, String> {
        let prl_id = Uuid::new_v4();
        let row: PrlRow = sqlx::query_as(
            "INSERT INTO prls (prl_id, name, pr_list_id, sspr_p_rev, raw_bytes, notes) \
             VALUES ($1, $2, $3, $4, $5, $6) \
             RETURNING prl_id, name, pr_list_id, sspr_p_rev, is_default, raw_bytes, notes, \
                       created_at, updated_at",
        )
        .bind(prl_id)
        .bind(name)
        .bind(pr_list_id)
        .bind(sspr_p_rev)
        .bind(raw_bytes)
        .bind(notes)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| format!("create_prl: {e}"))?;
        Ok(row.into())
    }

    async fn update_prl(
        &self,
        prl_id: Uuid,
        name: Option<&str>,
        raw_bytes: Option<&[u8]>,
        pr_list_id_sspr: Option<(i32, i16)>,
        notes: Option<&str>,
    ) -> Result<Prl, String> {
        let mut q = sqlx::QueryBuilder::<sqlx::Postgres>::new("UPDATE prls SET updated_at = NOW()");
        if let Some(n) = name {
            q.push(", name = ").push_bind(n.to_string());
        }
        if let Some(bytes) = raw_bytes {
            q.push(", raw_bytes = ").push_bind(bytes.to_vec());
        }
        if let Some((pr_list_id, sspr_p_rev)) = pr_list_id_sspr {
            q.push(", pr_list_id = ").push_bind(pr_list_id);
            q.push(", sspr_p_rev = ").push_bind(sspr_p_rev);
        }
        if let Some(n) = notes {
            q.push(", notes = ").push_bind(n.to_string());
        }
        q.push(" WHERE prl_id = ")
            .push_bind(prl_id)
            .push(" AND deleted_at IS NULL ")
            .push(
                " RETURNING prl_id, name, pr_list_id, sspr_p_rev, is_default, raw_bytes, notes, \
                   created_at, updated_at",
            );
        let row: PrlRow = q
            .build_query_as()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| format!("update_prl: {e}"))?;
        Ok(row.into())
    }

    async fn soft_delete_prl(&self, prl_id: Uuid) -> Result<Result<(), PrlDeleteBlocked>, String> {
        const SAMPLE_LIMIT: i64 = 5;
        let sample: Vec<(Uuid,)> = sqlx::query_as(
            "SELECT subscriber_id FROM subscribers WHERE prl_override_id = $1 LIMIT $2",
        )
        .bind(prl_id)
        .bind(SAMPLE_LIMIT)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| format!("soft_delete_prl sample: {e}"))?;
        if !sample.is_empty() {
            let (count,): (i64,) =
                sqlx::query_as("SELECT COUNT(*) FROM subscribers WHERE prl_override_id = $1")
                    .bind(prl_id)
                    .fetch_one(&self.pool)
                    .await
                    .map_err(|e| format!("soft_delete_prl count: {e}"))?;
            return Ok(Err(PrlDeleteBlocked::Referenced {
                count: count as u32,
                sample: sample.into_iter().map(|(u,)| u).collect(),
            }));
        }
        let affected = sqlx::query(
            "UPDATE prls SET deleted_at = NOW(), updated_at = NOW() WHERE prl_id = $1 AND deleted_at IS NULL",
        )
        .bind(prl_id)
        .execute(&self.pool)
        .await
        .map_err(|e| format!("soft_delete_prl: {e}"))?;
        if affected.rows_affected() == 0 {
            return Err("PRL not found".into());
        }
        Ok(Ok(()))
    }

    async fn set_default_prl(&self, prl_id: Uuid) -> Result<(), String> {
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| format!("set_default_prl begin: {e}"))?;
        sqlx::query(
            "UPDATE prls SET is_default = FALSE, updated_at = NOW() \
             WHERE is_default = TRUE AND deleted_at IS NULL AND prl_id <> $1",
        )
        .bind(prl_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| format!("set_default_prl clear: {e}"))?;
        let affected = sqlx::query(
            "UPDATE prls SET is_default = TRUE, updated_at = NOW() \
             WHERE prl_id = $1 AND deleted_at IS NULL",
        )
        .bind(prl_id)
        .execute(&mut *tx)
        .await
        .map_err(|e| format!("set_default_prl set: {e}"))?;
        if affected.rows_affected() == 0 {
            return Err("PRL not found".into());
        }
        tx.commit()
            .await
            .map_err(|e| format!("set_default_prl commit: {e}"))?;
        Ok(())
    }

    async fn set_subscriber_prl_override(
        &self,
        subscriber_id: Uuid,
        prl_id: Option<Uuid>,
    ) -> Result<(), String> {
        // If setting (not clearing), refuse pointers at soft-deleted PRLs.
        if let Some(pid) = prl_id {
            let (alive,): (bool,) = sqlx::query_as(
                "SELECT EXISTS(SELECT 1 FROM prls WHERE prl_id = $1 AND deleted_at IS NULL)",
            )
            .bind(pid)
            .fetch_one(&self.pool)
            .await
            .map_err(|e| format!("set_subscriber_prl_override exists check: {e}"))?;
            if !alive {
                return Err("PRL not found or deleted".into());
            }
        }
        let affected = sqlx::query(
            "UPDATE subscribers SET prl_override_id = $1, updated_at = NOW() WHERE subscriber_id = $2",
        )
        .bind(prl_id)
        .bind(subscriber_id)
        .execute(&self.pool)
        .await
        .map_err(|e| format!("set_subscriber_prl_override: {e}"))?;
        if affected.rows_affected() == 0 {
            return Err("subscriber not found".into());
        }
        Ok(())
    }

    async fn set_subscriber_spc(
        &self,
        subscriber_id: Uuid,
        spc: Option<String>,
    ) -> Result<(), String> {
        if let Some(s) = spc.as_deref()
            && !(s.len() == 6 && s.bytes().all(|b| b.is_ascii_digit()))
        {
            return Err("SPC must be exactly 6 digits".into());
        }
        let affected = sqlx::query(
            "UPDATE subscribers SET service_programming_code = $1, updated_at = NOW() WHERE subscriber_id = $2",
        )
        .bind(spc)
        .bind(subscriber_id)
        .execute(&self.pool)
        .await
        .map_err(|e| format!("set_subscriber_spc: {e}"))?;
        if affected.rows_affected() == 0 {
            return Err("subscriber not found".into());
        }
        Ok(())
    }

    async fn set_subscriber_firstchp_override(
        &self,
        subscriber_id: Uuid,
        firstchp: Option<u16>,
    ) -> Result<(), String> {
        if let Some(v) = firstchp
            && v > 2047
        {
            return Err("FIRSTCHP override must be in 0..=2047".into());
        }
        let affected = sqlx::query(
            "UPDATE subscribers SET firstchp_override = $1, updated_at = NOW() WHERE subscriber_id = $2",
        )
        .bind(firstchp.map(i32::from))
        .bind(subscriber_id)
        .execute(&self.pool)
        .await
        .map_err(|e| format!("set_subscriber_firstchp_override: {e}"))?;
        if affected.rows_affected() == 0 {
            return Err("subscriber not found".into());
        }
        Ok(())
    }

    async fn set_subscriber_a_key(
        &self,
        identity: &MobileIdentityKey,
        a_key: Option<Vec<u8>>,
        ssd_a: Option<Vec<u8>>,
        ssd_b: Option<Vec<u8>>,
        auth_algo: AuthAlgorithm,
    ) -> Result<(), String> {
        for key in [&a_key, &ssd_a, &ssd_b].into_iter().flatten() {
            validate_auth_key(key)?;
        }
        // Clearing the A-key resets the identity to unauthenticated: drop
        // the SSD halves and running challenge/counter state along with it.
        let (ssd_a, ssd_b, algo) = if a_key.is_none() {
            (None, None, AuthAlgorithm::None)
        } else {
            (ssd_a, ssd_b, auth_algo)
        };
        let Some(identity_id) = resolve_identity_row_id(&self.pool, identity).await? else {
            return Err("identity not found".into());
        };
        sqlx::query(
            r#"
            UPDATE subscriber_identities SET
                a_key = $2,
                ssd_a = $3,
                ssd_b = $4,
                auth_algo = $5,
                auth_count = CASE WHEN $2 IS NULL THEN 0 ELSE auth_count END,
                auth_failure_count = CASE WHEN $2 IS NULL THEN 0 ELSE auth_failure_count END,
                randssd = NULL,
                pending_rand = CASE WHEN $2 IS NULL THEN NULL ELSE pending_rand END,
                pending_randu = CASE WHEN $2 IS NULL THEN NULL ELSE pending_randu END,
                pending_a_key = NULL,
                pending_ssd_a = NULL,
                pending_ssd_b = NULL
            WHERE subscriber_identity_id = $1
            "#,
        )
        .bind(identity_id)
        .bind(a_key)
        .bind(ssd_a)
        .bind(ssd_b)
        .bind(algo.as_str())
        .execute(&self.pool)
        .await
        .map_err(|e| format!("set_subscriber_a_key: {e}"))?;
        Ok(())
    }

    async fn get_subscriber_auth_state(
        &self,
        identity: &MobileIdentityKey,
    ) -> Result<Option<IdentityAuthState>, String> {
        let Some(identity_id) = resolve_identity_row_id(&self.pool, identity).await? else {
            return Ok(None);
        };
        let row = sqlx::query_as::<_, AuthStateRow>(
            r#"
            SELECT subscriber_identity_id,
                (a_key IS NOT NULL) AS credentials_provisioned,
                (ssd_a IS NOT NULL AND ssd_b IS NOT NULL) AS ssd_provisioned,
                auth_algo, auth_count, auth_failure_count, last_auth_at
            FROM subscriber_identities
            WHERE subscriber_identity_id = $1
            "#,
        )
        .bind(identity_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| format!("get_subscriber_auth_state: {e}"))?;
        row.map(TryInto::try_into).transpose()
    }

    async fn verify_cave_global_challenge(
        &self,
        identity: &MobileIdentityKey,
        challenge: &GlobalChallenge,
    ) -> Result<CaveVerdict, String> {
        let (identity_id, creds) = self.load_cave_credentials(identity).await?;
        let verdict = crate::auth::verify_global_challenge(&creds, challenge)?;
        if verdict.outcome.is_success()
            && !challenge.otasp_reauthentication
            && let Some(reported) = crate::auth::resynced_count(&creds, challenge)
        {
            log::warn!(
                "HLR: COUNT mismatch for {identity:?}: stored {}, mobile reported {reported}. Adopting the mobile's COUNT. Repeated mismatches can indicate a cloned mobile.",
                creds.auth_count
            );
            sqlx::query(
                "UPDATE subscriber_identities SET auth_count = $2 WHERE subscriber_identity_id = $1",
            )
            .bind(identity_id)
            .bind(i16::from(reported))
            .execute(&self.pool)
            .await
            .map_err(|e| format!("resync auth_count: {e}"))?;
        }
        self.record_cave_outcome(identity_id, verdict.outcome)
            .await?;
        Ok(verdict)
    }

    async fn verify_cave_unique_challenge(
        &self,
        identity: &MobileIdentityKey,
        challenge: &UniqueChallenge,
    ) -> Result<CaveVerdict, String> {
        let (identity_id, creds) = self.load_cave_credentials(identity).await?;
        let verdict = crate::auth::verify_unique_challenge(&creds, challenge)?;
        self.record_cave_outcome(identity_id, verdict.outcome)
            .await?;
        Ok(verdict)
    }

    async fn begin_cave_ssd_update(&self, identity: &MobileIdentityKey) -> Result<u64, String> {
        let (identity_id, creds) = self.load_cave_credentials(identity).await?;
        if !creds.can_update_ssd() {
            return Err(format!(
                "{VALIDATION_FAILED_PREFIX}identity has no CAVE A-key"
            ));
        }
        let randssd = rand::random::<u64>() & ((1 << cdma_cave::RANDSSD_BITS) - 1);
        let bytes = cdma_cave::RandSsd::from_u64(randssd)
            .map_err(|e| e.to_string())?
            .0;
        sqlx::query(
            "UPDATE subscriber_identities SET randssd = $2 WHERE subscriber_identity_id = $1",
        )
        .bind(identity_id)
        .bind(bytes.to_vec())
        .execute(&self.pool)
        .await
        .map_err(|e| format!("begin_cave_ssd_update: {e}"))?;
        Ok(randssd)
    }

    async fn cave_base_station_challenge(
        &self,
        identity: &MobileIdentityKey,
        esn: u32,
        randbs: u32,
        imsi_s1: u32,
    ) -> Result<u32, String> {
        let (_, creds) = self.load_cave_credentials(identity).await?;
        crate::auth::base_station_challenge(&creds, esn, randbs, imsi_s1)
            .map_err(|e| format!("{VALIDATION_FAILED_PREFIX}{e}"))
    }

    async fn finish_cave_ssd_update(
        &self,
        identity: &MobileIdentityKey,
        esn: u32,
        confirmed: bool,
    ) -> Result<(), String> {
        let (identity_id, creds) = self.load_cave_credentials(identity).await?;
        let new_ssd = if confirmed {
            Some(
                crate::auth::committed_ssd(&creds, esn)
                    .map_err(|e| format!("{VALIDATION_FAILED_PREFIX}{e}"))?,
            )
        } else {
            None
        };
        // During an OTASP session the new SSD waits for the Commit with the new A-key.
        let sql = if creds.pending_a_key.is_some() {
            r#"
            UPDATE subscriber_identities SET
                pending_ssd_a = COALESCE($2, pending_ssd_a),
                pending_ssd_b = COALESCE($3, pending_ssd_b),
                randssd = NULL
            WHERE subscriber_identity_id = $1
            "#
        } else {
            r#"
            UPDATE subscriber_identities SET
                ssd_a = COALESCE($2, ssd_a),
                ssd_b = COALESCE($3, ssd_b),
                randssd = NULL
            WHERE subscriber_identity_id = $1
            "#
        };
        sqlx::query(sql)
            .bind(identity_id)
            .bind(new_ssd.map(|ssd| ssd.ssd_a.to_vec()))
            .bind(new_ssd.map(|ssd| ssd.ssd_b.to_vec()))
            .execute(&self.pool)
            .await
            .map_err(|e| format!("finish_cave_ssd_update: {e}"))?;
        Ok(())
    }

    async fn increment_cave_count(&self, identity: &MobileIdentityKey) -> Result<u8, String> {
        let Some(identity_id) = resolve_identity_row_id(&self.pool, identity).await? else {
            return Err(format!("{VALIDATION_FAILED_PREFIX}identity not found"));
        };
        let (count,): (i16,) = sqlx::query_as(
            "UPDATE subscriber_identities SET auth_count = (auth_count + 1) % $2 \
             WHERE subscriber_identity_id = $1 RETURNING auth_count",
        )
        .bind(identity_id)
        .bind(i16::from(crate::auth::AUTH_COUNT_MODULUS))
        .fetch_one(&self.pool)
        .await
        .map_err(|e| format!("increment_cave_count: {e}"))?;
        Ok(count as u8)
    }

    async fn begin_cave_key_exchange(
        &self,
        identity: &MobileIdentityKey,
        a_key_p_rev: u8,
    ) -> Result<crate::key_exchange::KeyExchangeOffer, String> {
        let Some(identity_id) = resolve_identity_row_id(&self.pool, identity).await? else {
            return Err(format!("{VALIDATION_FAILED_PREFIX}identity not found"));
        };
        let exchange =
            tokio::task::spawn_blocking(move || crate::key_exchange::KeyExchange::new(a_key_p_rev))
                .await
                .map_err(|e| format!("begin_cave_key_exchange: {e}"))?
                .map_err(|e| format!("{VALIDATION_FAILED_PREFIX}{e}"))?;
        let offer = exchange.offer();
        self.key_exchanges
            .lock()
            .map_err(|_| "key exchange table poisoned".to_string())?
            .insert(identity_id, exchange);
        Ok(offer)
    }

    async fn complete_cave_key_exchange(
        &self,
        identity: &MobileIdentityKey,
        ms_result: &[u8],
    ) -> Result<(), String> {
        let Some(identity_id) = resolve_identity_row_id(&self.pool, identity).await? else {
            return Err(format!("{VALIDATION_FAILED_PREFIX}identity not found"));
        };
        let exchange = self
            .key_exchanges
            .lock()
            .map_err(|_| "key exchange table poisoned".to_string())?
            .remove(&identity_id)
            .ok_or_else(|| format!("{VALIDATION_FAILED_PREFIX}no key exchange in progress"))?;
        let a_key = exchange
            .a_key(ms_result)
            .map_err(|e| format!("{VALIDATION_FAILED_PREFIX}{e}"))?;
        sqlx::query(
            r#"
            UPDATE subscriber_identities SET
                pending_a_key = $2,
                pending_ssd_a = NULL,
                pending_ssd_b = NULL
            WHERE subscriber_identity_id = $1
            "#,
        )
        .bind(identity_id)
        .bind(a_key.to_vec())
        .execute(&self.pool)
        .await
        .map_err(|e| format!("complete_cave_key_exchange: {e}"))?;
        Ok(())
    }

    async fn commit_cave_key(&self, identity: &MobileIdentityKey) -> Result<(), String> {
        let (identity_id, creds) = self.load_cave_credentials(identity).await?;
        if creds.pending_a_key.is_none() {
            return Err(format!(
                "{VALIDATION_FAILED_PREFIX}no exchanged A-key to commit"
            ));
        }
        // Without an SSD update in the session the mobile keeps its previous SSD, which
        // the HLR does not know, so the stored SSD is cleared and the next call updates it.
        sqlx::query(
            r#"
            UPDATE subscriber_identities SET
                a_key = pending_a_key,
                ssd_a = pending_ssd_a,
                ssd_b = pending_ssd_b,
                auth_algo = 'cave',
                pending_a_key = NULL,
                pending_ssd_a = NULL,
                pending_ssd_b = NULL,
                randssd = NULL,
                auth_failure_count = 0
            WHERE subscriber_identity_id = $1
            "#,
        )
        .bind(identity_id)
        .execute(&self.pool)
        .await
        .map_err(|e| format!("commit_cave_key: {e}"))?;
        Ok(())
    }

    async fn abort_cave_key_exchange(&self, identity: &MobileIdentityKey) -> Result<(), String> {
        let Some(identity_id) = resolve_identity_row_id(&self.pool, identity).await? else {
            return Ok(());
        };
        if let Ok(mut exchanges) = self.key_exchanges.lock() {
            exchanges.remove(&identity_id);
        }
        sqlx::query(
            r#"
            UPDATE subscriber_identities SET
                pending_a_key = NULL,
                pending_ssd_a = NULL,
                pending_ssd_b = NULL,
                randssd = NULL
            WHERE subscriber_identity_id = $1
            "#,
        )
        .bind(identity_id)
        .execute(&self.pool)
        .await
        .map_err(|e| format!("abort_cave_key_exchange: {e}"))?;
        Ok(())
    }

    async fn save_otasp_session(&self, row: &OtaspSessionRow) -> Result<(), String> {
        sqlx::query(
            "INSERT INTO otasp_sessions (
                 session_id, subscriber_id, esn, meid, started_at, ended_at,
                 outcome, feature_code, service_option, completed_blocks,
                 event_count, events_proto
             ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
             ON CONFLICT (session_id) DO UPDATE SET
                 ended_at = EXCLUDED.ended_at,
                 outcome = EXCLUDED.outcome,
                 feature_code = EXCLUDED.feature_code,
                 service_option = EXCLUDED.service_option,
                 completed_blocks = EXCLUDED.completed_blocks,
                 event_count = EXCLUDED.event_count,
                 events_proto = EXCLUDED.events_proto",
        )
        .bind(row.session_id)
        .bind(row.subscriber_id)
        .bind(row.esn.map(|v| v as i64))
        .bind(row.meid.as_deref())
        .bind(row.started_at)
        .bind(row.ended_at)
        .bind(row.outcome)
        .bind(row.feature_code.as_deref())
        .bind(row.service_option)
        .bind(row.completed_blocks)
        .bind(row.event_count)
        .bind(&row.events_proto)
        .execute(&self.pool)
        .await
        .map_err(|e| format!("save_otasp_session: {e}"))?;
        Ok(())
    }

    async fn list_otasp_sessions(
        &self,
        filter: OtaspSessionFilter,
        limit: u32,
        offset: u32,
    ) -> Result<(Vec<OtaspSessionRow>, u32), String> {
        let mut q = sqlx::QueryBuilder::<sqlx::Postgres>::new(
            "SELECT session_id, subscriber_id, esn, meid, started_at, ended_at, \
             outcome, feature_code, service_option, completed_blocks, event_count, \
             ''::BYTEA AS events_proto FROM otasp_sessions WHERE 1=1",
        );
        if let Some(sub) = filter.subscriber_id {
            q.push(" AND subscriber_id = ").push_bind(sub);
        }
        if let Some(esn) = filter.esn {
            q.push(" AND esn = ").push_bind(esn as i64);
        }
        if let Some(meid) = filter.meid.as_deref() {
            q.push(" AND meid = ").push_bind(meid);
        }
        q.push(" ORDER BY started_at DESC, session_id LIMIT ")
            .push_bind(limit as i64)
            .push(" OFFSET ")
            .push_bind(offset as i64);
        let rows: Vec<OtaspSessionDbRow> = q
            .build_query_as()
            .fetch_all(&self.pool)
            .await
            .map_err(|e| format!("list_otasp_sessions: {e}"))?;

        let mut count_q = sqlx::QueryBuilder::<sqlx::Postgres>::new(
            "SELECT COUNT(*) FROM otasp_sessions WHERE 1=1",
        );
        if let Some(sub) = filter.subscriber_id {
            count_q.push(" AND subscriber_id = ").push_bind(sub);
        }
        if let Some(esn) = filter.esn {
            count_q.push(" AND esn = ").push_bind(esn as i64);
        }
        if let Some(meid) = filter.meid.as_deref() {
            count_q.push(" AND meid = ").push_bind(meid);
        }
        let (total,): (i64,) = count_q
            .build_query_as()
            .fetch_one(&self.pool)
            .await
            .map_err(|e| format!("list_otasp_sessions count: {e}"))?;
        Ok((rows.into_iter().map(Into::into).collect(), total as u32))
    }

    async fn get_otasp_session(&self, session_id: Uuid) -> Result<Option<OtaspSessionRow>, String> {
        let row: Option<OtaspSessionDbRow> = sqlx::query_as(
            "SELECT session_id, subscriber_id, esn, meid, started_at, ended_at, \
             outcome, feature_code, service_option, completed_blocks, event_count, \
             events_proto FROM otasp_sessions WHERE session_id = $1",
        )
        .bind(session_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| format!("get_otasp_session: {e}"))?;
        Ok(row.map(Into::into))
    }
}

// ─── Row types for sqlx ────────────────────────────────────────

#[derive(sqlx::FromRow)]
struct SubscriberRow {
    subscriber_id: Uuid,
    phone_number: String,
    display_name: String,
    status: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
    number_type: NumberType,
    number_plan: NumberPlan,
    #[sqlx(default)]
    has_ringtone: Option<bool>,
    #[sqlx(default)]
    ringtone_duration_ms: Option<i64>,
    #[sqlx(default)]
    prl_override_id: Option<Uuid>,
    #[sqlx(default)]
    service_programming_code: Option<String>,
    #[sqlx(default)]
    firstchp_override: Option<i32>,
}

impl TryFrom<SubscriberRow> for Subscriber {
    type Error = String;

    fn try_from(r: SubscriberRow) -> Result<Self, Self::Error> {
        Ok(Subscriber {
            subscriber_id: r.subscriber_id,
            phone_number: r.phone_number,
            display_name: r.display_name,
            status: SubscriberStatus::from_str(&r.status)?,
            created_at: r.created_at,
            updated_at: r.updated_at,
            number_type: r.number_type,
            number_plan: r.number_plan,
            has_ringtone: r.has_ringtone.unwrap_or(false),
            ringtone_duration_ms: r.ringtone_duration_ms.map(|v| v as u64),
            prl_override_id: r.prl_override_id,
            service_programming_code: r.service_programming_code,
            firstchp_override: r.firstchp_override.map(|v| v as u16),
        })
    }
}

#[derive(sqlx::FromRow)]
struct IdentityRow {
    subscriber_identity_id: Uuid,
    subscriber_id: Uuid,
    imsi: Option<String>,
    esn: Option<i64>,
    meid: Option<String>,
    is_primary: bool,
    created_at: DateTime<Utc>,
}

impl From<IdentityRow> for SubscriberIdentity {
    fn from(r: IdentityRow) -> Self {
        SubscriberIdentity {
            subscriber_identity_id: r.subscriber_identity_id,
            subscriber_id: r.subscriber_id,
            imsi: r.imsi,
            esn: r.esn.map(|v| v as u32),
            meid: r.meid,
            is_primary: r.is_primary,
            created_at: r.created_at,
        }
    }
}

#[derive(sqlx::FromRow)]
struct AuthStateRow {
    subscriber_identity_id: Uuid,
    credentials_provisioned: bool,
    ssd_provisioned: bool,
    auth_algo: String,
    auth_count: i16,
    auth_failure_count: i32,
    last_auth_at: Option<DateTime<Utc>>,
}

impl TryFrom<AuthStateRow> for IdentityAuthState {
    type Error = String;

    fn try_from(r: AuthStateRow) -> Result<Self, Self::Error> {
        Ok(IdentityAuthState {
            subscriber_identity_id: r.subscriber_identity_id,
            credentials_provisioned: r.credentials_provisioned,
            ssd_provisioned: r.ssd_provisioned,
            auth_algo: AuthAlgorithm::from_str(&r.auth_algo)?,
            auth_count: r.auth_count as u8,
            auth_failure_count: r.auth_failure_count as u32,
            last_auth_at: r.last_auth_at,
        })
    }
}

#[derive(sqlx::FromRow)]
struct CaveCredentialsRow {
    a_key: Option<Vec<u8>>,
    ssd_a: Option<Vec<u8>>,
    ssd_b: Option<Vec<u8>>,
    auth_count: i16,
    auth_algo: String,
    randssd: Option<Vec<u8>>,
    pending_a_key: Option<Vec<u8>>,
    pending_ssd_a: Option<Vec<u8>>,
    pending_ssd_b: Option<Vec<u8>>,
}

fn ssd_from(ssd_a: Option<[u8; 8]>, ssd_b: Option<[u8; 8]>) -> Option<cdma_cave::Ssd> {
    match (ssd_a, ssd_b) {
        (Some(ssd_a), Some(ssd_b)) => Some(cdma_cave::Ssd { ssd_a, ssd_b }),
        _ => None,
    }
}

fn fixed_bytes<const N: usize>(
    field: &str,
    bytes: Option<Vec<u8>>,
) -> Result<Option<[u8; N]>, String> {
    bytes
        .map(|b| {
            b.try_into()
                .map_err(|_| format!("stored {field} must be {N} bytes"))
        })
        .transpose()
}

impl TryFrom<CaveCredentialsRow> for CaveCredentials {
    type Error = String;

    fn try_from(r: CaveCredentialsRow) -> Result<Self, Self::Error> {
        Ok(CaveCredentials {
            auth_algo: AuthAlgorithm::from_str(&r.auth_algo)?,
            a_key: fixed_bytes::<8>("a_key", r.a_key)?,
            ssd: ssd_from(
                fixed_bytes::<8>("ssd_a", r.ssd_a)?,
                fixed_bytes::<8>("ssd_b", r.ssd_b)?,
            ),
            auth_count: r.auth_count as u8,
            randssd: fixed_bytes::<7>("randssd", r.randssd)?,
            pending_a_key: fixed_bytes::<8>("pending_a_key", r.pending_a_key)?,
            pending_ssd: ssd_from(
                fixed_bytes::<8>("pending_ssd_a", r.pending_ssd_a)?,
                fixed_bytes::<8>("pending_ssd_b", r.pending_ssd_b)?,
            ),
        })
    }
}

#[derive(sqlx::FromRow)]
struct BindingRow {
    subscriber_id: Uuid,
    serving_bs_id: String,
    state: String,
    imsi: Option<String>,
    esn: Option<i64>,
    meid: Option<String>,
    mob_p_rev: Option<i64>,
    pgslot: Option<i64>,
    slot_cycle_index: Option<i64>,
    last_msg_seq: Option<i64>,
    last_registered_at: DateTime<Utc>,
    last_seen_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl TryFrom<BindingRow> for RegistrationBinding {
    type Error = String;

    fn try_from(r: BindingRow) -> Result<Self, Self::Error> {
        Ok(RegistrationBinding {
            subscriber_id: r.subscriber_id,
            serving_bs_id: r.serving_bs_id,
            state: RegistrationState::from_str(&r.state)?,
            imsi: r.imsi,
            esn: r.esn.map(|v| v as u32),
            meid: r.meid,
            mob_p_rev: r.mob_p_rev.map(|v| v as u32),
            pgslot: r.pgslot.map(|v| v as u32),
            slot_cycle_index: r.slot_cycle_index.map(|v| v as u32),
            last_msg_seq: r.last_msg_seq.map(|v| v as u32),
            last_registered_at: r.last_registered_at,
            last_seen_at: r.last_seen_at,
            updated_at: r.updated_at,
        })
    }
}

#[derive(sqlx::FromRow)]
struct PrlRow {
    prl_id: Uuid,
    name: String,
    pr_list_id: i32,
    sspr_p_rev: i16,
    is_default: bool,
    raw_bytes: Vec<u8>,
    notes: String,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

impl From<PrlRow> for Prl {
    fn from(r: PrlRow) -> Self {
        Prl {
            prl_id: r.prl_id,
            name: r.name,
            pr_list_id: r.pr_list_id,
            sspr_p_rev: r.sspr_p_rev,
            is_default: r.is_default,
            raw_bytes: r.raw_bytes,
            notes: r.notes,
            created_at: r.created_at,
            updated_at: r.updated_at,
        }
    }
}

#[derive(sqlx::FromRow)]
struct OtaspSessionDbRow {
    session_id: Uuid,
    subscriber_id: Option<Uuid>,
    esn: Option<i64>,
    meid: Option<String>,
    started_at: DateTime<Utc>,
    ended_at: Option<DateTime<Utc>>,
    outcome: i16,
    feature_code: Option<String>,
    service_option: Option<i32>,
    completed_blocks: i32,
    event_count: i32,
    events_proto: Vec<u8>,
}

impl From<OtaspSessionDbRow> for OtaspSessionRow {
    fn from(r: OtaspSessionDbRow) -> Self {
        OtaspSessionRow {
            session_id: r.session_id,
            subscriber_id: r.subscriber_id,
            // ESN is 32-bit; we store as bigint for nullability.
            esn: r.esn.map(|v| v as u32),
            meid: r.meid,
            started_at: r.started_at,
            ended_at: r.ended_at,
            outcome: r.outcome,
            feature_code: r.feature_code,
            service_option: r.service_option,
            completed_blocks: r.completed_blocks,
            event_count: r.event_count,
            events_proto: r.events_proto,
        }
    }
}
