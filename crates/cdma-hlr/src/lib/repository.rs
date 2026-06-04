use async_trait::async_trait;
use chrono::{DateTime, Utc};
use tonic::Code;
use uuid::Uuid;

use crate::model::*;
use crate::proto;

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

fn binding_from_proto(value: proto::RegistrationBinding) -> Result<RegistrationBinding, String> {
    Ok(RegistrationBinding {
        subscriber_id: Uuid::parse_str(&value.subscriber_id)
            .map_err(|e| format!("invalid subscriber_id: {e}"))?,
        serving_node_id: value.serving_node_id,
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
        let Some(subscriber_proto) = response.subscriber else {
            return Ok(None);
        };
        let subscriber = subscriber_from_proto(subscriber_proto)?;
        let primary_identity = response
            .primary_identity
            .map(identity_from_proto)
            .transpose()?;
        let binding = response.binding.map(binding_from_proto).transpose()?;
        // The resolve response intentionally doesn't include the full
        // identity list — only the primary one. Callers that need the
        // whole list should use GetSubscriber.
        Ok(Some(ResolvedSubscriber {
            subscriber,
            identities: Vec::new(),
            primary_identity,
            binding,
        }))
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
                    esn,
                    meid: meid.map(ToOwned::to_owned),
                },
            )
            .await
            .map_err(|e| format!("HLR ResolveSubscriberByHardwareIdentity: {e}"))?
            .into_inner();
        let Some(subscriber_proto) = response.subscriber else {
            return Ok(None);
        };
        let subscriber = subscriber_from_proto(subscriber_proto)?;
        let primary_identity = response
            .primary_identity
            .map(identity_from_proto)
            .transpose()?;
        let binding = response.binding.map(binding_from_proto).transpose()?;
        Ok(Some(ResolvedSubscriber {
            subscriber,
            identities: Vec::new(),
            primary_identity,
            binding,
        }))
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
                serving_node_id: binding.serving_node_id,
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
}

impl PostgresHlrRepository {
    pub fn new(pool: sqlx::PgPool) -> Self {
        Self { pool }
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
        cdma_voice::VoiceCodec::EvrcA => "evrc_a",
        cdma_voice::VoiceCodec::EvrcB => "evrc_b",
        cdma_voice::VoiceCodec::EvrcWb => "evrc_wb",
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

async fn select_mobile_seen_by_key(
    tx: &mut sqlx::Transaction<'_, sqlx::Postgres>,
    identity: &MobileIdentityKey,
) -> Result<Option<MobileSeenIdentityRow>, String> {
    match identity {
        MobileIdentityKey::ImsiEsn { imsi, esn } => sqlx::query_as::<_, MobileSeenIdentityRow>(
            r#"
            SELECT id, last_seen_at
            FROM mobiles_seen
            WHERE imsi = $1 AND esn = $2 AND meid IS NULL
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
                WHERE imsi = $1 AND esn = $2 AND meid = $3
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
            meid = $4,
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
                    created_at, updated_at, number_type, number_plan, prl_override_id, service_programming_code
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
                s.number_type, s.number_plan, s.prl_override_id, s.service_programming_code,
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
                s.number_type, s.number_plan, s.prl_override_id, s.service_programming_code,
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
                    created_at, updated_at, number_type, number_plan, prl_override_id, service_programming_code
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
                s.number_type, s.number_plan, s.prl_override_id, s.service_programming_code,
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
                    s.number_type, s.number_plan, s.prl_override_id, s.service_programming_code, s.service_programming_code, s.prl_override_id,
                    EXISTS (SELECT 1 FROM subscriber_ringtones r WHERE r.subscriber_id = s.subscriber_id) AS has_ringtone,
                    (SELECT MIN(duration_ms) FROM subscriber_ringtones r WHERE r.subscriber_id = s.subscriber_id) AS ringtone_duration_ms
                FROM subscribers s
                JOIN subscriber_identities i ON s.subscriber_id = i.subscriber_id
                WHERE i.imsi = $1 AND i.esn = $2 AND i.meid IS NULL
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
                    s.number_type, s.number_plan, s.prl_override_id, s.service_programming_code, s.service_programming_code, s.prl_override_id,
                    EXISTS (SELECT 1 FROM subscriber_ringtones r WHERE r.subscriber_id = s.subscriber_id) AS has_ringtone,
                    (SELECT MIN(duration_ms) FROM subscriber_ringtones r WHERE r.subscriber_id = s.subscriber_id) AS ringtone_duration_ms
                FROM subscribers s
                JOIN subscriber_identities i ON s.subscriber_id = i.subscriber_id
                WHERE i.imsi = $1 AND i.esn IS NULL AND i.meid = $2
                LIMIT 1
                "#,
            )
            .bind(imsi)
            .bind(meid)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| format!("resolve_by_identity imsi+meid: {e}"))?,
            MobileIdentityKey::ImsiEsnMeid { imsi, esn, meid } => {
                sqlx::query_as::<_, SubscriberRow>(
                    r#"
                    SELECT s.subscriber_id, s.phone_number, s.display_name, s.status, s.created_at, s.updated_at,
                        s.number_type, s.number_plan, s.prl_override_id, s.service_programming_code, s.service_programming_code, s.prl_override_id,
                        EXISTS (SELECT 1 FROM subscriber_ringtones r WHERE r.subscriber_id = s.subscriber_id) AS has_ringtone,
                        (SELECT MIN(duration_ms) FROM subscriber_ringtones r WHERE r.subscriber_id = s.subscriber_id) AS ringtone_duration_ms
                    FROM subscribers s
                    JOIN subscriber_identities i ON s.subscriber_id = i.subscriber_id
                    WHERE i.imsi = $1 AND i.esn = $2 AND i.meid = $3
                    LIMIT 1
                    "#,
                )
                .bind(imsi)
                .bind(*esn as i64)
                .bind(meid)
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
                s.number_type, s.number_plan, s.prl_override_id, s.service_programming_code,
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
                subscriber_id, serving_node_id, state, imsi, esn, meid,
                mob_p_rev, pgslot, slot_cycle_index, last_msg_seq,
                last_registered_at, last_seen_at, updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12, $13)
            ON CONFLICT (subscriber_id) DO UPDATE SET
                serving_node_id = EXCLUDED.serving_node_id,
                state = EXCLUDED.state,
                imsi = EXCLUDED.imsi,
                esn = EXCLUDED.esn,
                meid = EXCLUDED.meid,
                mob_p_rev = EXCLUDED.mob_p_rev,
                pgslot = EXCLUDED.pgslot,
                slot_cycle_index = EXCLUDED.slot_cycle_index,
                last_msg_seq = EXCLUDED.last_msg_seq,
                last_registered_at = EXCLUDED.last_registered_at,
                last_seen_at = EXCLUDED.last_seen_at,
                updated_at = EXCLUDED.updated_at
            RETURNING subscriber_id, serving_node_id, state, imsi, esn, meid,
                mob_p_rev, pgslot, slot_cycle_index, last_msg_seq,
                last_registered_at, last_seen_at, updated_at
            "#,
        )
        .bind(binding.subscriber_id)
        .bind(&binding.serving_node_id)
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
            SELECT subscriber_id, serving_node_id, state, imsi, esn,
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
        // Preencode is CPU-bound (WAV decode + resample + 3× EVRC encode); run
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
struct BindingRow {
    subscriber_id: Uuid,
    serving_node_id: String,
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
            serving_node_id: r.serving_node_id,
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
