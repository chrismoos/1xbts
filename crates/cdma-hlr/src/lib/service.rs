use std::net::SocketAddr;
use std::sync::Arc;

use chrono::{DateTime, Utc};
use tonic::transport::Server;
use tonic::{Request, Response, Status};
use uuid::Uuid;

use crate::model;
use crate::proto;
use crate::repository::{
    HlrRepository, PostgresHlrRepository, number_plan_from_proto, number_plan_to_proto,
    number_type_from_proto, number_type_to_proto,
};

const DEFAULT_MOBILES_SEEN_PAGE_SIZE: u32 = 50;
const MAX_MOBILES_SEEN_PAGE_SIZE: u32 = 500;
const MAX_ADMISSION_LABEL_LEN: usize = 64;

pub struct HlrServiceImpl {
    repo: Arc<dyn HlrRepository>,
}

impl HlrServiceImpl {
    pub fn new(repo: Arc<dyn HlrRepository>) -> Self {
        Self { repo }
    }

    async fn apply_identity_imei(
        &self,
        identity: model::SubscriberIdentity,
        imei: Option<&str>,
    ) -> Result<model::SubscriberIdentity, Status> {
        if identity.imei.as_deref() == imei {
            return Ok(identity);
        }
        self.repo
            .set_identity_imei(identity.subscriber_identity_id, imei)
            .await
            .map_err(map_repo_error)
    }
}

pub async fn run_grpc_server(
    addr: SocketAddr,
    repo: Arc<dyn HlrRepository>,
) -> Result<(), tonic::transport::Error> {
    // Raw ringtone WAV uploads can be up to a few MB. Default tonic decode
    // cap (4 MiB) is too tight for that path.
    let svc = proto::hlr_service_server::HlrServiceServer::new(HlrServiceImpl::new(repo))
        .max_decoding_message_size(8 * 1024 * 1024);
    Server::builder().add_service(svc).serve(addr).await
}

/// Binds the HLR gRPC listener and serves it on a background task.
///
/// The returned handle resolves when the server stops, which for a bind
/// failure is almost immediately. A caller that waits only on a shutdown
/// signal and ignores it leaves the process up with a dead listener.
pub async fn spawn_configured_hlr_service(
    config: crate::HlrNodeConfig,
) -> Result<(SocketAddr, tokio::task::JoinHandle<()>), String> {
    let addr = config.grpc_listen_addr;
    let repo = PostgresHlrRepository::connect_from_config(&config).await?;
    let served = tokio::spawn(async move {
        if let Err(error) = run_grpc_server(addr, Arc::new(repo)).await {
            log::error!("HLR gRPC server error: {error}");
        }
    });
    Ok((addr, served))
}

// ─── Conversion helpers ────────────────────────────────────────

fn datetime_to_timestamp(dt: DateTime<Utc>) -> prost_types::Timestamp {
    prost_types::Timestamp {
        seconds: dt.timestamp(),
        nanos: dt.timestamp_subsec_nanos() as i32,
    }
}

fn subscriber_to_proto(s: &model::Subscriber) -> proto::Subscriber {
    proto::Subscriber {
        subscriber_id: s.subscriber_id.to_string(),
        phone_number: s.phone_number.clone(),
        display_name: s.display_name.clone(),
        status: status_to_proto(&s.status) as i32,
        created_at: Some(datetime_to_timestamp(s.created_at)),
        updated_at: Some(datetime_to_timestamp(s.updated_at)),
        number_type: number_type_to_proto(s.number_type) as i32,
        number_plan: number_plan_to_proto(s.number_plan) as i32,
        has_ringtone: s.has_ringtone,
        ringtone_duration_ms: s.ringtone_duration_ms,
        prl_override_id: s.prl_override_id.map(|u| u.to_string()),
        service_programming_code: s.service_programming_code.clone(),
        firstchp_override: s.firstchp_override.map(u32::from),
    }
}

fn status_to_proto(s: &model::SubscriberStatus) -> proto::SubscriberStatus {
    match s {
        model::SubscriberStatus::Active => proto::SubscriberStatus::Active,
        model::SubscriberStatus::Suspended => proto::SubscriberStatus::Suspended,
        model::SubscriberStatus::Disabled => proto::SubscriberStatus::Disabled,
    }
}

fn status_from_proto_i32(value: i32) -> Result<model::SubscriberStatus, Status> {
    match proto::SubscriberStatus::try_from(value) {
        Ok(proto::SubscriberStatus::Active) => Ok(model::SubscriberStatus::Active),
        Ok(proto::SubscriberStatus::Suspended) => Ok(model::SubscriberStatus::Suspended),
        Ok(proto::SubscriberStatus::Disabled) => Ok(model::SubscriberStatus::Disabled),
        Ok(proto::SubscriberStatus::Unspecified) | Err(_) => Err(Status::invalid_argument(
            format!("invalid subscriber status: {value}"),
        )),
    }
}

fn identity_to_proto(i: &model::SubscriberIdentity) -> proto::SubscriberIdentity {
    proto::SubscriberIdentity {
        subscriber_identity_id: i.subscriber_identity_id.to_string(),
        subscriber_id: i.subscriber_id.to_string(),
        imsi: i.imsi.clone(),
        esn: i.esn,
        meid: i.meid.clone(),
        imei: i.imei.clone(),
        is_primary: i.is_primary,
        created_at: Some(datetime_to_timestamp(i.created_at)),
    }
}

fn auth_algo_from_wire(value: i32) -> model::AuthAlgorithm {
    match proto::AuthAlgorithm::try_from(value) {
        Ok(proto::AuthAlgorithm::Cave) => model::AuthAlgorithm::Cave,
        _ => model::AuthAlgorithm::None,
    }
}

fn auth_algo_to_wire(algo: model::AuthAlgorithm) -> i32 {
    let variant = match algo {
        model::AuthAlgorithm::None => proto::AuthAlgorithm::None,
        model::AuthAlgorithm::Cave => proto::AuthAlgorithm::Cave,
    };
    variant as i32
}

fn non_empty_key(field: &str, value: Option<Vec<u8>>) -> Result<Option<Vec<u8>>, Status> {
    match value {
        None => Ok(None),
        Some(bytes) if bytes.is_empty() => Ok(None),
        Some(bytes) => {
            model::validate_auth_key(&bytes)
                .map_err(|e| Status::invalid_argument(format!("invalid {field}: {e}")))?;
            Ok(Some(bytes))
        }
    }
}

fn binding_to_proto(b: &model::RegistrationBinding) -> proto::RegistrationBinding {
    proto::RegistrationBinding {
        subscriber_id: b.subscriber_id.to_string(),
        serving_bs_id: b.serving_bs_id.clone(),
        state: b.state.as_str().to_string(),
        imsi: b.imsi.clone(),
        esn: b.esn,
        meid: b.meid.clone(),
        mob_p_rev: b.mob_p_rev,
        pgslot: b.pgslot,
        slot_cycle_index: b.slot_cycle_index,
        last_msg_seq: b.last_msg_seq,
        last_registered_at: Some(datetime_to_timestamp(b.last_registered_at)),
        last_seen_at: Some(datetime_to_timestamp(b.last_seen_at)),
        updated_at: Some(datetime_to_timestamp(b.updated_at)),
    }
}

fn mobile_seen_to_proto(result: &model::MobileSeenUpsert) -> proto::MobileSeenUpsert {
    proto::MobileSeenUpsert {
        is_new: result.is_new,
        previous_last_seen_at: result.previous_last_seen_at.map(datetime_to_timestamp),
    }
}

fn parse_uuid(s: &str) -> Result<Uuid, Status> {
    Uuid::parse_str(s).map_err(|_| Status::invalid_argument(format!("invalid UUID: {s}")))
}

fn validate_optional_imsi(imsi: Option<&str>) -> Result<(), Status> {
    if let Some(imsi) = imsi {
        model::validate_imsi(imsi)
            .map_err(|e| Status::invalid_argument(format!("invalid IMSI: {e}")))?;
    }
    Ok(())
}

fn validate_optional_meid(meid: Option<&str>) -> Result<(), Status> {
    if let Some(meid) = meid {
        model::normalize_meid(meid)
            .map_err(|e| Status::invalid_argument(format!("invalid MEID: {e}")))?;
    }
    Ok(())
}

fn validate_optional_imei(imei: Option<&str>) -> Result<(), Status> {
    if let Some(imei) = imei {
        model::normalize_imei(imei)
            .map_err(|e| Status::invalid_argument(format!("invalid IMEI: {e}")))?;
    }
    Ok(())
}

fn parse_identity_key(
    identity: Option<proto::MobileIdentityKey>,
) -> Result<model::MobileIdentityKey, Status> {
    let identity = identity.ok_or_else(|| Status::invalid_argument("identity is required"))?;
    model::MobileIdentityKey::from_parts(
        identity.imsi.as_deref(),
        identity.esn,
        identity.meid.as_deref(),
    )
    .map_err(Status::invalid_argument)
}

fn parse_hardware_identity_key(
    identity: Option<proto::HardwareIdentityKey>,
) -> Result<(Option<u32>, Option<String>), Status> {
    let identity = identity.ok_or_else(|| Status::invalid_argument("identity is required"))?;
    if identity.esn.is_none() && identity.meid.is_none() {
        return Err(Status::invalid_argument(
            "hardware identity requires ESN or MEID",
        ));
    }
    let meid = identity
        .meid
        .map(|meid| {
            model::normalize_meid(&meid)
                .map_err(|e| Status::invalid_argument(format!("invalid MEID: {e}")))
        })
        .transpose()?;
    Ok((identity.esn, meid))
}

fn validate_phone_number(phone_number: &str) -> Result<(), Status> {
    model::validate_phone_number(phone_number)
        .map_err(|e| Status::invalid_argument(format!("invalid phone number: {e}")))
}

// ─── gRPC Service Implementation ──────────────────────────────

#[tonic::async_trait]
impl proto::hlr_service_server::HlrService for HlrServiceImpl {
    async fn upsert_subscriber(
        &self,
        request: Request<proto::UpsertSubscriberRequest>,
    ) -> Result<Response<proto::UpsertSubscriberResponse>, Status> {
        let req = request.into_inner();
        validate_phone_number(&req.phone_number)?;
        validate_optional_imsi(req.imsi.as_deref())?;
        validate_optional_meid(req.meid.as_deref())?;
        validate_optional_imei(req.imei.as_deref())?;

        let number_type = number_type_from_proto(req.number_type);
        let number_plan = number_plan_from_proto(req.number_plan);
        let status = status_from_proto_i32(req.status)?;

        let subscriber = self
            .repo
            .upsert_subscriber(
                &req.phone_number,
                &req.display_name,
                status.as_str(),
                number_type,
                number_plan,
            )
            .await
            .map_err(|e| {
                log::error!("HLR: {e}");
                Status::internal("internal error")
            })?;

        let has_identity = req.imsi.is_some() || req.esn.is_some() || req.meid.is_some();

        let identity = if has_identity {
            let id = self
                .repo
                .upsert_identity(
                    subscriber.subscriber_id,
                    req.imsi.as_deref(),
                    req.esn,
                    req.meid.as_deref(),
                )
                .await
                .map_err(map_repo_error)?;
            let id = self.apply_identity_imei(id, req.imei.as_deref()).await?;
            Some(identity_to_proto(&id))
        } else {
            None
        };

        Ok(Response::new(proto::UpsertSubscriberResponse {
            subscriber: Some(subscriber_to_proto(&subscriber)),
            identity,
        }))
    }

    async fn upsert_identity(
        &self,
        request: Request<proto::UpsertIdentityRequest>,
    ) -> Result<Response<proto::UpsertIdentityResponse>, Status> {
        let req = request.into_inner();
        let subscriber_id = parse_uuid(&req.subscriber_id)?;
        validate_optional_imsi(req.imsi.as_deref())?;
        validate_optional_meid(req.meid.as_deref())?;
        let identity = self
            .repo
            .upsert_identity(
                subscriber_id,
                req.imsi.as_deref(),
                req.esn,
                req.meid.as_deref(),
            )
            .await
            .map_err(map_repo_error)?;
        Ok(Response::new(proto::UpsertIdentityResponse {
            identity: Some(identity_to_proto(&identity)),
        }))
    }

    async fn replace_primary_identity(
        &self,
        request: Request<proto::ReplacePrimaryIdentityRequest>,
    ) -> Result<Response<proto::ReplacePrimaryIdentityResponse>, Status> {
        let req = request.into_inner();
        let subscriber_id = parse_uuid(&req.subscriber_id)?;
        validate_optional_imsi(req.imsi.as_deref())?;
        validate_optional_meid(req.meid.as_deref())?;
        let identity = self
            .repo
            .replace_primary_identity(
                subscriber_id,
                req.imsi.as_deref(),
                req.esn,
                req.meid.as_deref(),
            )
            .await
            .map_err(map_repo_error)?;
        Ok(Response::new(proto::ReplacePrimaryIdentityResponse {
            identity: Some(identity_to_proto(&identity)),
        }))
    }

    async fn get_identities_for_subscriber(
        &self,
        request: Request<proto::GetIdentitiesForSubscriberRequest>,
    ) -> Result<Response<proto::GetIdentitiesForSubscriberResponse>, Status> {
        let subscriber_id = parse_uuid(&request.into_inner().subscriber_id)?;
        let identities = self
            .repo
            .get_identities_for_subscriber(subscriber_id)
            .await
            .map_err(Status::internal)?;
        Ok(Response::new(proto::GetIdentitiesForSubscriberResponse {
            identities: identities.iter().map(identity_to_proto).collect(),
        }))
    }

    async fn upsert_mobile_seen(
        &self,
        request: Request<proto::UpsertMobileSeenRequest>,
    ) -> Result<Response<proto::UpsertMobileSeenResponse>, Status> {
        let req = request.into_inner();
        let identity = parse_identity_key(req.identity)?;
        let mob_p_rev = req
            .mob_p_rev
            .map(|value| {
                u8::try_from(value).map_err(|_| Status::invalid_argument("mob_p_rev > 255"))
            })
            .transpose()?;
        let result = self
            .repo
            .upsert_mobile_seen(&identity, mob_p_rev)
            .await
            .map_err(Status::internal)?;
        Ok(Response::new(proto::UpsertMobileSeenResponse {
            result: Some(mobile_seen_to_proto(&result)),
        }))
    }

    async fn record_hardware_admission(
        &self,
        request: Request<proto::RecordHardwareAdmissionRequest>,
    ) -> Result<Response<proto::RecordHardwareAdmissionResponse>, Status> {
        let req = request.into_inner();
        let (esn, meid) = parse_hardware_identity_key(req.identity)?;
        let class = crate::repository::admission_class_from_proto(req.admission_class)
            .ok_or_else(|| Status::invalid_argument("admission_class is required"))?;
        if req.reason.len() > MAX_ADMISSION_LABEL_LEN
            || req.base_station.len() > MAX_ADMISSION_LABEL_LEN
        {
            return Err(Status::invalid_argument(
                "admission reason and base station are limited to 64 bytes",
            ));
        }
        self.repo
            .record_hardware_admission(
                esn,
                meid.as_deref(),
                &model::MobileAdmission {
                    class,
                    reason: req.reason,
                    base_station: req.base_station,
                },
            )
            .await
            .map_err(Status::internal)?;
        Ok(Response::new(proto::RecordHardwareAdmissionResponse {}))
    }

    async fn record_mobile_admission(
        &self,
        request: Request<proto::RecordMobileAdmissionRequest>,
    ) -> Result<Response<proto::RecordMobileAdmissionResponse>, Status> {
        let req = request.into_inner();
        let identity = parse_identity_key(req.identity)?;
        let class = crate::repository::admission_class_from_proto(req.admission_class)
            .ok_or_else(|| Status::invalid_argument("admission_class is required"))?;
        if req.reason.len() > MAX_ADMISSION_LABEL_LEN
            || req.base_station.len() > MAX_ADMISSION_LABEL_LEN
        {
            return Err(Status::invalid_argument(
                "admission reason and base station are limited to 64 bytes",
            ));
        }
        let result = self
            .repo
            .record_mobile_admission(
                &identity,
                &model::MobileAdmission {
                    class,
                    reason: req.reason,
                    base_station: req.base_station,
                },
            )
            .await
            .map_err(Status::internal)?;
        Ok(Response::new(proto::RecordMobileAdmissionResponse {
            result: Some(mobile_seen_to_proto(&result)),
        }))
    }

    async fn list_mobiles_seen(
        &self,
        request: Request<proto::ListMobilesSeenRequest>,
    ) -> Result<Response<proto::ListMobilesSeenResponse>, Status> {
        let req = request.into_inner();
        let page_size = match req.page_size {
            0 => DEFAULT_MOBILES_SEEN_PAGE_SIZE,
            size => size.min(MAX_MOBILES_SEEN_PAGE_SIZE),
        };
        let (mobiles, total) = self
            .repo
            .list_mobiles_seen(
                crate::repository::mobile_seen_filter_from_proto(req.filter),
                page_size,
                req.offset,
            )
            .await
            .map_err(Status::internal)?;
        Ok(Response::new(proto::ListMobilesSeenResponse {
            mobiles: mobiles
                .iter()
                .map(crate::repository::observed_mobile_to_proto)
                .collect(),
            total,
        }))
    }

    async fn get_subscriber_by_phone_number(
        &self,
        request: Request<proto::GetSubscriberByPhoneNumberRequest>,
    ) -> Result<Response<proto::GetSubscriberByPhoneNumberResponse>, Status> {
        let req = request.into_inner();

        let resolved = self
            .repo
            .get_subscriber_by_phone_number(&req.phone_number)
            .await
            .map_err(|e| {
                log::error!("HLR: {e}");
                Status::internal("internal error")
            })?
            .ok_or_else(|| Status::not_found("subscriber not found"))?;

        Ok(Response::new(proto::GetSubscriberByPhoneNumberResponse {
            subscriber: Some(subscriber_to_proto(&resolved.subscriber)),
            identities: resolved.identities.iter().map(identity_to_proto).collect(),
            binding: resolved.binding.as_ref().map(binding_to_proto),
            primary_identity: resolved.primary_identity.as_ref().map(identity_to_proto),
        }))
    }

    async fn get_subscriber(
        &self,
        request: Request<proto::GetSubscriberRequest>,
    ) -> Result<Response<proto::GetSubscriberResponse>, Status> {
        let req = request.into_inner();
        let subscriber_id = parse_uuid(&req.subscriber_id)?;

        let resolved = self
            .repo
            .get_subscriber_by_id(subscriber_id)
            .await
            .map_err(|e| {
                log::error!("HLR: {e}");
                Status::internal("internal error")
            })?
            .ok_or_else(|| Status::not_found("subscriber not found"))?;

        Ok(Response::new(proto::GetSubscriberResponse {
            subscriber: Some(subscriber_to_proto(&resolved.subscriber)),
            identities: resolved.identities.iter().map(identity_to_proto).collect(),
            binding: resolved.binding.as_ref().map(binding_to_proto),
            primary_identity: resolved.primary_identity.as_ref().map(identity_to_proto),
        }))
    }

    async fn resolve_subscriber_by_identity(
        &self,
        request: Request<proto::ResolveSubscriberByIdentityRequest>,
    ) -> Result<Response<proto::ResolveSubscriberByIdentityResponse>, Status> {
        let req = request.into_inner();
        let identity = parse_identity_key(req.identity)?;

        let resolved = self
            .repo
            .resolve_by_identity(&identity)
            .await
            .map_err(|e| {
                log::error!("HLR: {e}");
                Status::internal("internal error")
            })?;

        Ok(Response::new(proto::ResolveSubscriberByIdentityResponse {
            subscriber: resolved
                .as_ref()
                .map(|r| subscriber_to_proto(&r.subscriber)),
            binding: resolved
                .as_ref()
                .and_then(|r| r.binding.as_ref())
                .map(binding_to_proto),
            primary_identity: resolved
                .as_ref()
                .and_then(|r| r.primary_identity.as_ref())
                .map(identity_to_proto),
        }))
    }

    async fn resolve_subscriber_by_registered_imsi(
        &self,
        request: Request<proto::ResolveSubscriberByRegisteredImsiRequest>,
    ) -> Result<Response<proto::ResolveSubscriberByIdentityResponse>, Status> {
        let req = request.into_inner();
        model::validate_imsi(&req.imsi).map_err(Status::invalid_argument)?;
        if req.serving_bs_id.trim().is_empty() {
            return Err(Status::invalid_argument("serving node is required"));
        }
        let resolved = self
            .repo
            .resolve_registered_imsi(&req.imsi, &req.serving_bs_id)
            .await?;
        Ok(Response::new(proto::ResolveSubscriberByIdentityResponse {
            subscriber: resolved
                .as_ref()
                .map(|r| subscriber_to_proto(&r.subscriber)),
            binding: resolved
                .as_ref()
                .and_then(|r| r.binding.as_ref())
                .map(binding_to_proto),
            primary_identity: resolved
                .as_ref()
                .and_then(|r| r.primary_identity.as_ref())
                .map(identity_to_proto),
        }))
    }

    async fn resolve_subscriber_by_hardware_identity(
        &self,
        request: Request<proto::ResolveSubscriberByHardwareIdentityRequest>,
    ) -> Result<Response<proto::ResolveSubscriberByHardwareIdentityResponse>, Status> {
        let req = request.into_inner();
        let (esn, meid) = parse_hardware_identity_key(req.identity)?;

        let resolved = if req.match_imei {
            self.repo
                .resolve_by_hrpd_hardware_identity(esn, meid.as_deref())
                .await
        } else {
            self.repo
                .resolve_by_hardware_identity(esn, meid.as_deref())
                .await
        };
        let resolved = resolved.map_err(|e| {
            log::error!("HLR: {e}");
            match e {
                crate::repository::HardwareLookupError::Ambiguous => {
                    Status::failed_precondition("ambiguous hardware identity")
                }
                crate::repository::HardwareLookupError::Unavailable(_) => {
                    Status::internal("hardware identity lookup failed")
                }
            }
        })?;

        Ok(Response::new(
            proto::ResolveSubscriberByHardwareIdentityResponse {
                identities: resolved
                    .as_ref()
                    .map(|resolved| resolved.identities.iter().map(identity_to_proto).collect())
                    .unwrap_or_default(),
                subscriber: resolved
                    .as_ref()
                    .map(|r| subscriber_to_proto(&r.subscriber)),
                binding: resolved
                    .as_ref()
                    .and_then(|r| r.binding.as_ref())
                    .map(binding_to_proto),
                primary_identity: resolved
                    .as_ref()
                    .and_then(|r| r.primary_identity.as_ref())
                    .map(identity_to_proto),
            },
        ))
    }

    async fn upsert_registration_binding(
        &self,
        request: Request<proto::UpsertRegistrationBindingRequest>,
    ) -> Result<Response<proto::UpsertRegistrationBindingResponse>, Status> {
        let req = request.into_inner();
        let subscriber_id = parse_uuid(&req.subscriber_id)?;
        let now = Utc::now();

        let binding = model::RegistrationBinding {
            subscriber_id,
            serving_bs_id: req.serving_bs_id,
            state: model::RegistrationState::from_str(&req.state).map_err(|e| {
                log::error!("HLR: {e}");
                Status::internal("internal error")
            })?,
            imsi: req.imsi,
            esn: req.esn,
            meid: req.meid,
            mob_p_rev: req.mob_p_rev,
            pgslot: req.pgslot,
            slot_cycle_index: req.slot_cycle_index,
            last_msg_seq: req.last_msg_seq,
            last_registered_at: now,
            last_seen_at: now,
            updated_at: now,
        };

        let result = self
            .repo
            .upsert_registration_binding(binding)
            .await
            .map_err(|e| {
                log::error!("HLR: {e}");
                Status::internal("internal error")
            })?;

        Ok(Response::new(proto::UpsertRegistrationBindingResponse {
            binding: Some(binding_to_proto(&result)),
        }))
    }

    async fn resolve_sms_target(
        &self,
        request: Request<proto::ResolveSmsTargetRequest>,
    ) -> Result<Response<proto::ResolveSmsTargetResponse>, Status> {
        let req = request.into_inner();

        let resolved = self
            .repo
            .get_subscriber_by_phone_number(&req.destination_number)
            .await
            .map_err(|e| {
                log::error!("HLR: {e}");
                Status::internal("internal error")
            })?
            .ok_or_else(|| Status::not_found("subscriber not found"))?;

        Ok(Response::new(proto::ResolveSmsTargetResponse {
            subscriber: Some(subscriber_to_proto(&resolved.subscriber)),
            binding: resolved.binding.as_ref().map(binding_to_proto),
        }))
    }

    async fn list_subscribers(
        &self,
        request: Request<proto::ListSubscribersRequest>,
    ) -> Result<Response<proto::ListSubscribersResponse>, Status> {
        let req = request.into_inner();
        const MAX_LIMIT: u32 = 10_000;
        let limit = req.limit.unwrap_or(50).min(MAX_LIMIT);
        let offset = req.offset.unwrap_or(0);

        let (subscribers, total) =
            self.repo
                .list_subscribers(limit, offset)
                .await
                .map_err(|e| {
                    log::error!("HLR: {e}");
                    Status::internal("internal error")
                })?;

        Ok(Response::new(proto::ListSubscribersResponse {
            subscribers: subscribers.iter().map(subscriber_to_proto).collect(),
            total,
        }))
    }

    async fn update_subscriber(
        &self,
        request: Request<proto::UpdateSubscriberRequest>,
    ) -> Result<Response<proto::UpdateSubscriberResponse>, Status> {
        let req = request.into_inner();
        let subscriber_id = parse_uuid(&req.subscriber_id)?;
        validate_phone_number(&req.phone_number)?;

        let number_type = number_type_from_proto(req.number_type);
        let number_plan = number_plan_from_proto(req.number_plan);
        let status = status_from_proto_i32(req.status)?;

        let subscriber = self
            .repo
            .update_subscriber(
                subscriber_id,
                &req.phone_number,
                &req.display_name,
                status.as_str(),
                number_type,
                number_plan,
            )
            .await
            .map_err(|e| {
                log::error!("HLR: {e}");
                Status::internal("internal error")
            })?
            .ok_or_else(|| Status::not_found("subscriber not found"))?;

        validate_optional_imsi(req.imsi.as_deref())?;
        validate_optional_meid(req.meid.as_deref())?;
        validate_optional_imei(req.imei.as_deref())?;

        let has_identity = req.imsi.is_some() || req.esn.is_some() || req.meid.is_some();

        let identity = if has_identity {
            let identity = self
                .repo
                .replace_primary_identity(
                    subscriber_id,
                    req.imsi.as_deref(),
                    req.esn,
                    req.meid.as_deref(),
                )
                .await
                .map_err(|e| {
                    log::error!("HLR: {e}");
                    Status::internal("internal error")
                })?;
            Some(
                self.apply_identity_imei(identity, req.imei.as_deref())
                    .await?,
            )
        } else {
            None
        };

        let binding = self
            .repo
            .get_registration_binding(subscriber_id)
            .await
            .map_err(|e| {
                log::error!("HLR: {e}");
                Status::internal("internal error")
            })?;

        Ok(Response::new(proto::UpdateSubscriberResponse {
            subscriber: Some(subscriber_to_proto(&subscriber)),
            identity: identity.as_ref().map(identity_to_proto),
            binding: binding.as_ref().map(binding_to_proto),
        }))
    }

    async fn delete_subscriber(
        &self,
        request: Request<proto::DeleteSubscriberRequest>,
    ) -> Result<Response<()>, Status> {
        let req = request.into_inner();
        let subscriber_id = parse_uuid(&req.subscriber_id)?;

        let deleted = self
            .repo
            .delete_subscriber(subscriber_id)
            .await
            .map_err(|e| {
                log::error!("HLR: {e}");
                Status::internal("internal error")
            })?;

        if !deleted {
            return Err(Status::not_found("subscriber not found"));
        }

        Ok(Response::new(()))
    }

    async fn get_registration_binding(
        &self,
        request: Request<proto::GetRegistrationBindingRequest>,
    ) -> Result<Response<proto::GetRegistrationBindingResponse>, Status> {
        let req = request.into_inner();
        let subscriber_id = parse_uuid(&req.subscriber_id)?;

        let binding = self
            .repo
            .get_registration_binding(subscriber_id)
            .await
            .map_err(|e| {
                log::error!("HLR: {e}");
                Status::internal("internal error")
            })?;

        Ok(Response::new(proto::GetRegistrationBindingResponse {
            binding: binding.as_ref().map(binding_to_proto),
        }))
    }

    async fn set_subscriber_ringtone(
        &self,
        request: Request<proto::SetSubscriberRingtoneRequest>,
    ) -> Result<Response<proto::SetSubscriberRingtoneResponse>, Status> {
        let req = request.into_inner();
        let subscriber_id = parse_uuid(&req.subscriber_id)?;
        if req.wav_bytes.is_empty() {
            return Err(Status::invalid_argument("wav_bytes is empty"));
        }
        if req.original_filename.is_empty() {
            return Err(Status::invalid_argument("original_filename is empty"));
        }
        if req.original_filename.len() > 255 {
            return Err(Status::invalid_argument(
                "original_filename exceeds 255 chars",
            ));
        }

        let outcome = self
            .repo
            .set_ringtone(subscriber_id, req.wav_bytes, &req.original_filename)
            .await
            .map_err(|e| {
                log::warn!("HLR set_ringtone: {e}");
                if e.contains("preencode") {
                    Status::invalid_argument(e)
                } else if e.contains("not found") {
                    Status::not_found("subscriber not found")
                } else {
                    Status::internal("internal error")
                }
            })?;

        Ok(Response::new(proto::SetSubscriberRingtoneResponse {
            codecs: outcome
                .codecs
                .into_iter()
                .map(|c| proto::RingtoneCodecInfo {
                    codec: c.codec,
                    encoded_bytes: c.encoded_bytes,
                    frame_count: c.frame_count,
                })
                .collect(),
            duration_ms: outcome.duration_ms,
        }))
    }

    async fn clear_subscriber_ringtone(
        &self,
        request: Request<proto::ClearSubscriberRingtoneRequest>,
    ) -> Result<Response<()>, Status> {
        let req = request.into_inner();
        let subscriber_id = parse_uuid(&req.subscriber_id)?;
        self.repo.clear_ringtone(subscriber_id).await.map_err(|e| {
            log::error!("HLR: {e}");
            Status::internal("internal error")
        })?;
        Ok(Response::new(()))
    }

    async fn get_subscriber_ringtone_codec(
        &self,
        request: Request<proto::GetSubscriberRingtoneCodecRequest>,
    ) -> Result<Response<proto::GetSubscriberRingtoneCodecResponse>, Status> {
        let req = request.into_inner();
        let subscriber_id = parse_uuid(&req.subscriber_id)?;
        if !is_valid_codec_name(&req.codec) {
            return Err(Status::invalid_argument(format!(
                "unknown codec: {}",
                req.codec
            )));
        }
        let blob = self
            .repo
            .get_ringtone_codec(subscriber_id, &req.codec)
            .await
            .map_err(|e| {
                log::error!("HLR: {e}");
                Status::internal("internal error")
            })?
            .ok_or_else(|| Status::not_found("ringtone not found"))?;
        Ok(Response::new(proto::GetSubscriberRingtoneCodecResponse {
            encoded_frames: blob.encoded_frames,
            frame_count: blob.frame_count,
            duration_ms: blob.duration_ms,
        }))
    }

    // ─── PRL management ─────────────────────────────────────────

    async fn list_prls(
        &self,
        request: Request<proto::ListPrlsRequest>,
    ) -> Result<Response<proto::ListPrlsResponse>, Status> {
        let req = request.into_inner();
        let filter = model::PrlListFilter {
            pr_list_id: req.pr_list_id,
            sspr_p_rev: req.sspr_p_rev,
        };
        let limit = req.limit.clamp(1, 200);
        let (rows, total) = self
            .repo
            .list_prls(limit, req.offset, filter)
            .await
            .map_err(|e| {
                log::error!("HLR list_prls: {e}");
                Status::internal("internal error")
            })?;
        let prls = rows.iter().map(prl_to_summary).collect();
        Ok(Response::new(proto::ListPrlsResponse { prls, total }))
    }

    async fn get_prl(
        &self,
        request: Request<proto::GetPrlRequest>,
    ) -> Result<Response<proto::GetPrlResponse>, Status> {
        let req = request.into_inner();
        let prl_id = parse_uuid(&req.prl_id)?;
        let row = self
            .repo
            .get_prl(prl_id)
            .await
            .map_err(|e| {
                log::error!("HLR get_prl: {e}");
                Status::internal("internal error")
            })?
            .ok_or_else(|| Status::not_found("PRL not found"))?;
        let prl = build_full_prl(row)?;
        Ok(Response::new(proto::GetPrlResponse { prl: Some(prl) }))
    }

    async fn create_prl(
        &self,
        request: Request<proto::CreatePrlRequest>,
    ) -> Result<Response<proto::CreatePrlResponse>, Status> {
        let req = request.into_inner();
        let (raw_bytes, pr_list_id, sspr_p_rev) = resolve_create_or_update_body(req.source)?;
        let row = self
            .repo
            .create_prl(&req.name, &raw_bytes, pr_list_id, sspr_p_rev, &req.notes)
            .await
            .map_err(map_repo_error)?;
        let prl = build_full_prl(row)?;
        Ok(Response::new(proto::CreatePrlResponse { prl: Some(prl) }))
    }

    async fn update_prl(
        &self,
        request: Request<proto::UpdatePrlRequest>,
    ) -> Result<Response<proto::UpdatePrlResponse>, Status> {
        let req = request.into_inner();
        let prl_id = parse_uuid(&req.prl_id)?;
        let body_update = match req.body_update {
            Some(b) => {
                let (raw_bytes, pr_list_id, sspr_p_rev) = resolve_create_or_update_body_body(b)?;
                Some((raw_bytes, pr_list_id, sspr_p_rev))
            }
            None => None,
        };
        let (raw_bytes_ref, pr_list_id_sspr) = match &body_update {
            Some((bytes, pr_list_id, sspr_p_rev)) => {
                (Some(bytes.as_slice()), Some((*pr_list_id, *sspr_p_rev)))
            }
            None => (None, None),
        };
        let row = self
            .repo
            .update_prl(
                prl_id,
                req.name.as_deref(),
                raw_bytes_ref,
                pr_list_id_sspr,
                req.notes.as_deref(),
            )
            .await
            .map_err(map_repo_error)?;
        let prl = build_full_prl(row)?;
        Ok(Response::new(proto::UpdatePrlResponse { prl: Some(prl) }))
    }

    async fn delete_prl(
        &self,
        request: Request<proto::DeletePrlRequest>,
    ) -> Result<Response<()>, Status> {
        let req = request.into_inner();
        let prl_id = parse_uuid(&req.prl_id)?;
        let result = self.repo.soft_delete_prl(prl_id).await.map_err(|e| {
            log::error!("HLR delete_prl: {e}");
            Status::internal("internal error")
        })?;
        match result {
            Ok(()) => Ok(Response::new(())),
            Err(model::PrlDeleteBlocked::Referenced { count, sample }) => {
                let detail = proto::PrlDeleteBlockedError {
                    referencing_subscribers: count,
                    sample_subscriber_ids: sample.iter().map(|u| u.to_string()).collect(),
                };
                Err(Status::failed_precondition(format!(
                    "PRL is referenced by {} subscribers (sample: {:?})",
                    detail.referencing_subscribers, detail.sample_subscriber_ids
                )))
            }
        }
    }

    async fn set_default_prl(
        &self,
        request: Request<proto::SetDefaultPrlRequest>,
    ) -> Result<Response<()>, Status> {
        let req = request.into_inner();
        let prl_id = parse_uuid(&req.prl_id)?;
        self.repo
            .set_default_prl(prl_id)
            .await
            .map_err(map_repo_error)?;
        Ok(Response::new(()))
    }

    async fn get_default_prl(
        &self,
        _request: Request<proto::GetDefaultPrlRequest>,
    ) -> Result<Response<proto::GetDefaultPrlResponse>, Status> {
        let row = self.repo.get_default_prl().await.map_err(|e| {
            log::error!("HLR get_default_prl: {e}");
            Status::internal("internal error")
        })?;
        let prl = match row {
            Some(p) => Some(build_full_prl(p)?),
            None => None,
        };
        Ok(Response::new(proto::GetDefaultPrlResponse { prl }))
    }

    async fn set_subscriber_prl_override(
        &self,
        request: Request<proto::SetSubscriberPrlOverrideRequest>,
    ) -> Result<Response<()>, Status> {
        let req = request.into_inner();
        let subscriber_id = parse_uuid(&req.subscriber_id)?;
        let prl_id = match req.prl_id.as_deref() {
            None | Some("") => None,
            Some(s) => Some(parse_uuid(s)?),
        };
        self.repo
            .set_subscriber_prl_override(subscriber_id, prl_id)
            .await
            .map_err(map_repo_error)?;
        Ok(Response::new(()))
    }

    async fn set_subscriber_spc(
        &self,
        request: Request<proto::SetSubscriberSpcRequest>,
    ) -> Result<Response<()>, Status> {
        let req = request.into_inner();
        let subscriber_id = parse_uuid(&req.subscriber_id)?;
        let spc = match req.service_programming_code.as_deref() {
            None | Some("") => None,
            Some(s) => Some(s.to_string()),
        };
        self.repo
            .set_subscriber_spc(subscriber_id, spc)
            .await
            .map_err(map_repo_error)?;
        Ok(Response::new(()))
    }

    async fn set_subscriber_firstchp_override(
        &self,
        request: Request<proto::SetSubscriberFirstchpOverrideRequest>,
    ) -> Result<Response<()>, Status> {
        let req = request.into_inner();
        let subscriber_id = parse_uuid(&req.subscriber_id)?;
        let firstchp = match req.firstchp_override {
            None => None,
            Some(v) if v <= 2047 => Some(v as u16),
            Some(v) => {
                return Err(Status::invalid_argument(format!(
                    "firstchp_override {v} out of range 0..=2047"
                )));
            }
        };
        self.repo
            .set_subscriber_firstchp_override(subscriber_id, firstchp)
            .await
            .map_err(map_repo_error)?;
        Ok(Response::new(()))
    }

    async fn set_subscriber_a_key(
        &self,
        request: Request<proto::SetSubscriberAKeyRequest>,
    ) -> Result<Response<()>, Status> {
        let req = request.into_inner();
        let identity = parse_identity_key(req.identity)?;
        let mut a_key = non_empty_key("a_key", req.a_key)?;
        let mut ssd_a = non_empty_key("ssd_a", req.ssd_a)?;
        let mut ssd_b = non_empty_key("ssd_b", req.ssd_b)?;
        if let Some(digits) = req.a_key_digits.filter(|d| !d.trim().is_empty()) {
            if a_key.is_some() {
                return Err(Status::invalid_argument(
                    "a_key and a_key_digits are mutually exclusive",
                ));
            }
            let esn = identity.esn().ok_or_else(|| {
                Status::invalid_argument("A-key check digits are computed over an ESN")
            })?;
            let verified = cdma_cave::AKey::from_entry_digits(&digits, cdma_cave::Esn(esn))
                .map_err(|e| Status::invalid_argument(e.to_string()))?;
            a_key = Some(verified.0.to_vec());
            ssd_a = None;
            ssd_b = None;
        }
        let requested = auth_algo_from_wire(req.auth_algo);
        let auth_algo = if a_key.is_some() && requested == model::AuthAlgorithm::None {
            model::AuthAlgorithm::Cave
        } else {
            requested
        };
        self.repo
            .set_subscriber_a_key(&identity, a_key, ssd_a, ssd_b, auth_algo)
            .await
            .map_err(map_repo_error)?;
        Ok(Response::new(()))
    }

    async fn get_subscriber_auth_state(
        &self,
        request: Request<proto::GetSubscriberAuthStateRequest>,
    ) -> Result<Response<proto::GetSubscriberAuthStateResponse>, Status> {
        let req = request.into_inner();
        let identity = parse_identity_key(req.identity)?;
        let state = self
            .repo
            .get_subscriber_auth_state(&identity)
            .await
            .map_err(map_repo_error)?
            .ok_or_else(|| Status::not_found("identity not found"))?;
        Ok(Response::new(proto::GetSubscriberAuthStateResponse {
            subscriber_identity_id: state.subscriber_identity_id.to_string(),
            credentials_provisioned: state.credentials_provisioned,
            ssd_provisioned: state.ssd_provisioned,
            auth_algo: auth_algo_to_wire(state.auth_algo),
            auth_count: u32::from(state.auth_count),
            auth_failure_count: state.auth_failure_count,
            last_auth_at: state.last_auth_at.map(datetime_to_timestamp),
        }))
    }

    async fn verify_cave_global_challenge(
        &self,
        request: Request<proto::VerifyCaveGlobalChallengeRequest>,
    ) -> Result<Response<proto::CaveVerdict>, Status> {
        let req = request.into_inner();
        let identity = parse_identity_key(req.identity)?;
        let challenge = crate::auth::GlobalChallenge {
            esn: req.esn,
            rand: req.rand,
            authr: req.authr,
            count: req
                .count
                .map(|count| u8::try_from(count).map_err(|_| Status::invalid_argument("count")))
                .transpose()?,
            auth_data: req.auth_data,
            generate_call_keys: req.generate_call_keys,
            otasp_reauthentication: req.otasp_reauthentication,
        };
        let verdict = self
            .repo
            .verify_cave_global_challenge(&identity, &challenge)
            .await
            .map_err(map_repo_error)?;
        Ok(Response::new(crate::repository::cave_verdict_to_proto(
            verdict,
        )))
    }

    async fn verify_cave_unique_challenge(
        &self,
        request: Request<proto::VerifyCaveUniqueChallengeRequest>,
    ) -> Result<Response<proto::CaveVerdict>, Status> {
        let req = request.into_inner();
        let identity = parse_identity_key(req.identity)?;
        let challenge = crate::auth::UniqueChallenge {
            esn: req.esn,
            randu: req.randu,
            authu: req.authu,
            imsi_s1: req.imsi_s1,
            imsi_s2: u16::try_from(req.imsi_s2).map_err(|_| Status::invalid_argument("imsi_s2"))?,
            generate_call_keys: req.generate_call_keys,
        };
        let verdict = self
            .repo
            .verify_cave_unique_challenge(&identity, &challenge)
            .await
            .map_err(map_repo_error)?;
        Ok(Response::new(crate::repository::cave_verdict_to_proto(
            verdict,
        )))
    }

    async fn begin_cave_ssd_update(
        &self,
        request: Request<proto::BeginCaveSsdUpdateRequest>,
    ) -> Result<Response<proto::BeginCaveSsdUpdateResponse>, Status> {
        let identity = parse_identity_key(request.into_inner().identity)?;
        let randssd = self
            .repo
            .begin_cave_ssd_update(&identity)
            .await
            .map_err(map_repo_error)?;
        Ok(Response::new(proto::BeginCaveSsdUpdateResponse { randssd }))
    }

    async fn compute_otapa_signature(
        &self,
        request: Request<proto::ComputeOtapaSignatureRequest>,
    ) -> Result<Response<proto::ComputeOtapaSignatureResponse>, Status> {
        let request = request.into_inner();
        let identity = parse_identity_key(request.identity)?;
        let auth_otapa = self
            .repo
            .compute_otapa_signature(&identity, request.esn, request.rand_otapa, request.imsi_s1)
            .await?;
        Ok(Response::new(proto::ComputeOtapaSignatureResponse {
            auth_otapa,
        }))
    }

    async fn cave_base_station_challenge(
        &self,
        request: Request<proto::CaveBaseStationChallengeRequest>,
    ) -> Result<Response<proto::CaveBaseStationChallengeResponse>, Status> {
        let req = request.into_inner();
        let identity = parse_identity_key(req.identity)?;
        let authbs = self
            .repo
            .cave_base_station_challenge(&identity, req.esn, req.randbs, req.imsi_s1)
            .await
            .map_err(map_repo_error)?;
        Ok(Response::new(proto::CaveBaseStationChallengeResponse {
            authbs,
        }))
    }

    async fn finish_cave_ssd_update(
        &self,
        request: Request<proto::FinishCaveSsdUpdateRequest>,
    ) -> Result<Response<()>, Status> {
        let req = request.into_inner();
        let identity = parse_identity_key(req.identity)?;
        self.repo
            .finish_cave_ssd_update(&identity, req.esn, req.confirmed)
            .await
            .map_err(map_repo_error)?;
        Ok(Response::new(()))
    }

    async fn increment_cave_count(
        &self,
        request: Request<proto::IncrementCaveCountRequest>,
    ) -> Result<Response<proto::IncrementCaveCountResponse>, Status> {
        let identity = parse_identity_key(request.into_inner().identity)?;
        let auth_count = self
            .repo
            .increment_cave_count(&identity)
            .await
            .map_err(map_repo_error)?;
        Ok(Response::new(proto::IncrementCaveCountResponse {
            auth_count: u32::from(auth_count),
        }))
    }

    async fn begin_cave_key_exchange(
        &self,
        request: Request<proto::BeginCaveKeyExchangeRequest>,
    ) -> Result<Response<proto::BeginCaveKeyExchangeResponse>, Status> {
        let req = request.into_inner();
        let identity = parse_identity_key(req.identity)?;
        let a_key_p_rev =
            u8::try_from(req.a_key_p_rev).map_err(|_| Status::invalid_argument("a_key_p_rev"))?;
        let offer = self
            .repo
            .begin_cave_key_exchange(&identity, a_key_p_rev)
            .await
            .map_err(map_repo_error)?;
        let (param_p, param_g) = match offer.params {
            Some((p, g)) => (Some(p), Some(g)),
            None => (None, None),
        };
        Ok(Response::new(proto::BeginCaveKeyExchangeResponse {
            param_p,
            param_g,
            bs_result: offer.bs_result,
        }))
    }

    async fn complete_cave_key_exchange(
        &self,
        request: Request<proto::CompleteCaveKeyExchangeRequest>,
    ) -> Result<Response<()>, Status> {
        let req = request.into_inner();
        let identity = parse_identity_key(req.identity)?;
        self.repo
            .complete_cave_key_exchange(&identity, &req.ms_result)
            .await
            .map_err(map_repo_error)?;
        Ok(Response::new(()))
    }

    async fn commit_cave_key(
        &self,
        request: Request<proto::CaveKeyIdentityRequest>,
    ) -> Result<Response<()>, Status> {
        let identity = parse_identity_key(request.into_inner().identity)?;
        self.repo
            .commit_cave_key(&identity)
            .await
            .map_err(map_repo_error)?;
        Ok(Response::new(()))
    }

    async fn abort_cave_key_exchange(
        &self,
        request: Request<proto::CaveKeyIdentityRequest>,
    ) -> Result<Response<()>, Status> {
        let identity = parse_identity_key(request.into_inner().identity)?;
        self.repo
            .abort_cave_key_exchange(&identity)
            .await
            .map_err(map_repo_error)?;
        Ok(Response::new(()))
    }

    // ─── OTASP session history ──────────────────────────────────

    async fn save_otasp_session(
        &self,
        request: Request<proto::SaveOtaspSessionRequest>,
    ) -> Result<Response<()>, Status> {
        let req = request.into_inner();
        let summary = req
            .summary
            .ok_or_else(|| Status::invalid_argument("SaveOtaspSession: summary missing"))?;
        let row = otasp_row_from_request(summary, req.events_proto)?;
        self.repo
            .save_otasp_session(&row)
            .await
            .map_err(map_repo_error)?;
        Ok(Response::new(()))
    }

    async fn list_otasp_sessions(
        &self,
        request: Request<proto::ListOtaspSessionsRequest>,
    ) -> Result<Response<proto::ListOtaspSessionsResponse>, Status> {
        let req = request.into_inner();
        let filter = model::OtaspSessionFilter {
            subscriber_id: match req.subscriber_id.as_deref() {
                None | Some("") => None,
                Some(s) => Some(parse_uuid(s)?),
            },
            esn: req.esn,
            meid: req.meid,
        };
        let limit = if req.limit == 0 {
            10
        } else {
            req.limit.min(100)
        };
        let (rows, total) = self
            .repo
            .list_otasp_sessions(filter, limit, req.offset)
            .await
            .map_err(map_repo_error)?;
        let sessions = rows.iter().map(otasp_session_row_to_summary).collect();
        Ok(Response::new(proto::ListOtaspSessionsResponse {
            sessions,
            total,
        }))
    }

    async fn get_otasp_session(
        &self,
        request: Request<proto::GetOtaspSessionRequest>,
    ) -> Result<Response<proto::GetOtaspSessionResponse>, Status> {
        let req = request.into_inner();
        let session_id = parse_uuid(&req.session_id)?;
        let row = self
            .repo
            .get_otasp_session(session_id)
            .await
            .map_err(map_repo_error)?
            .ok_or_else(|| Status::not_found("OTASP session not found"))?;
        let summary = otasp_session_row_to_summary(&row);
        // Decode the events_proto blob into the typed timeline so the
        // client doesn't have to.
        use crate::proto_root::events::v1 as events_proto;
        use prost::Message;
        let wrap = events_proto::OtaspRecordedEvents::decode(row.events_proto.as_slice()).map_err(
            |e| {
                log::error!("HLR get_otasp_session: events_proto decode failed: {e}");
                Status::internal("internal error")
            },
        )?;
        Ok(Response::new(proto::GetOtaspSessionResponse {
            session: Some(proto::OtaspSessionDetail {
                summary: Some(summary),
                events: wrap.events,
            }),
        }))
    }
}

fn otasp_session_row_to_summary(r: &model::OtaspSessionRow) -> proto::OtaspSessionSummary {
    proto::OtaspSessionSummary {
        session_id: r.session_id.to_string(),
        subscriber_id: r.subscriber_id.map(|u| u.to_string()),
        esn: r.esn,
        meid: r.meid.clone(),
        started_at: Some(datetime_to_timestamp(r.started_at)),
        ended_at: r.ended_at.map(datetime_to_timestamp),
        outcome: r.outcome as i32,
        feature_code: r.feature_code.clone(),
        service_option: r.service_option.map(|v| v as u32),
        completed_blocks: r.completed_blocks as u32,
        event_count: r.event_count as u32,
    }
}

fn otasp_row_from_request(
    summary: proto::OtaspSessionSummary,
    events_proto: Vec<u8>,
) -> Result<model::OtaspSessionRow, Status> {
    Ok(model::OtaspSessionRow {
        session_id: parse_uuid(&summary.session_id)?,
        subscriber_id: match summary.subscriber_id.as_deref() {
            None | Some("") => None,
            Some(s) => Some(parse_uuid(s)?),
        },
        esn: summary.esn,
        meid: summary.meid,
        started_at: summary
            .started_at
            .map(|ts| chrono::DateTime::<chrono::Utc>::from_timestamp(ts.seconds, ts.nanos as u32))
            .ok_or_else(|| Status::invalid_argument("started_at missing"))?
            .ok_or_else(|| Status::invalid_argument("invalid started_at"))?,
        ended_at: summary.ended_at.and_then(|ts| {
            chrono::DateTime::<chrono::Utc>::from_timestamp(ts.seconds, ts.nanos as u32)
        }),
        outcome: summary.outcome as i16,
        feature_code: summary.feature_code,
        service_option: summary.service_option.map(|v| v as i32),
        completed_blocks: summary.completed_blocks as i32,
        event_count: summary.event_count as i32,
        events_proto,
    })
}

fn prl_to_summary(p: &model::Prl) -> proto::PrlSummary {
    proto::PrlSummary {
        prl_id: p.prl_id.to_string(),
        name: p.name.clone(),
        pr_list_id: p.pr_list_id as u32,
        sspr_p_rev: p.sspr_p_rev as u32,
        is_default: p.is_default,
        raw_bytes_size: p.raw_bytes.len() as u32,
        notes: p.notes.clone(),
        created_at: Some(datetime_to_timestamp(p.created_at)),
        updated_at: Some(datetime_to_timestamp(p.updated_at)),
    }
}

fn build_full_prl(p: model::Prl) -> Result<proto::Prl, Status> {
    let summary = prl_to_summary(&p);
    crate::prl_proto::proto_from_raw_bytes(summary, p.raw_bytes.clone())
        .map_err(map_validation_error)
}

fn resolve_create_or_update_body(
    source: Option<proto::create_prl_request::Source>,
) -> Result<(Vec<u8>, i32, i16), Status> {
    let src = source.ok_or_else(|| Status::invalid_argument("CreatePrl source missing"))?;
    match src {
        proto::create_prl_request::Source::RawBytes(bytes) => resolve_raw(bytes),
        proto::create_prl_request::Source::Built(d) => resolve_built(&d),
    }
}

fn resolve_create_or_update_body_body(
    body: proto::update_prl_request::BodyUpdate,
) -> Result<(Vec<u8>, i32, i16), Status> {
    match body {
        proto::update_prl_request::BodyUpdate::RawBytes(bytes) => resolve_raw(bytes),
        proto::update_prl_request::BodyUpdate::Built(d) => resolve_built(&d),
    }
}

fn resolve_raw(bytes: Vec<u8>) -> Result<(Vec<u8>, i32, i16), Status> {
    // Decode to populate cached cols + verify CRC.
    let decoded = crate::prl_proto::decode_to_proto(&bytes).map_err(map_validation_error)?;
    let (pr_list_id, sspr_p_rev) = match &decoded.body {
        Some(proto::prl_decoded::Body::Classic(c)) => (c.pr_list_id as i32, 1i16),
        Some(proto::prl_decoded::Body::Extended(e)) => {
            (e.pr_list_id as i32, e.cur_sspr_p_rev as i16)
        }
        None => return Err(Status::invalid_argument("decoded body missing")),
    };
    // Reject CRC mismatch up-front so we never persist a row whose CRC
    // doesn't recompute.
    let crc_ok = match &decoded.body {
        Some(proto::prl_decoded::Body::Classic(c)) => c.crc_ok,
        Some(proto::prl_decoded::Body::Extended(e)) => e.crc_ok,
        None => false,
    };
    if !crc_ok {
        return Err(map_validation_error(
            model::PrlValidationFailure::CrcMismatch {
                ms_crc: 0,
                computed_crc: 0,
            },
        ));
    }
    Ok((bytes, pr_list_id, sspr_p_rev))
}

fn resolve_built(decoded: &proto::PrlDecoded) -> Result<(Vec<u8>, i32, i16), Status> {
    let bytes = crate::prl_proto::encode_proto_to_bytes(decoded).map_err(map_validation_error)?;
    let (pr_list_id, sspr_p_rev) = match &decoded.body {
        Some(proto::prl_decoded::Body::Classic(c)) => (c.pr_list_id as i32, 1i16),
        Some(proto::prl_decoded::Body::Extended(e)) => {
            (e.pr_list_id as i32, e.cur_sspr_p_rev as i16)
        }
        None => return Err(Status::invalid_argument("PrlDecoded.body missing")),
    };
    Ok((bytes, pr_list_id, sspr_p_rev))
}

fn map_validation_error(e: model::PrlValidationFailure) -> Status {
    let (kind, detail) = match &e {
        model::PrlValidationFailure::DecodeFailed(d) => {
            (proto::prl_validation_error::Kind::DecodeFailed, d.clone())
        }
        model::PrlValidationFailure::CrcMismatch { .. } => (
            proto::prl_validation_error::Kind::CrcMismatch,
            "PRL CRC mismatch".to_string(),
        ),
        model::PrlValidationFailure::UnsupportedRev(v) => (
            proto::prl_validation_error::Kind::UnsupportedRev,
            format!("unsupported SSPR_P_REV 0x{:02x}", v),
        ),
        model::PrlValidationFailure::EncodeFailed(d) => {
            (proto::prl_validation_error::Kind::EncodeFailed, d.clone())
        }
    };
    Status::invalid_argument(format!("PRL validation: {:?} — {}", kind, detail))
}

fn map_repo_error(e: String) -> Status {
    if let Some(msg) = e.strip_prefix(crate::repository::VALIDATION_FAILED_PREFIX) {
        log::warn!("HLR repo validation: {msg}");
        return Status::invalid_argument(msg.to_string());
    }
    log::error!("HLR repo: {e}");
    Status::internal("internal error")
}

fn is_valid_codec_name(name: &str) -> bool {
    matches!(
        name,
        "tia96" | "evrc_a" | "evrc_b" | "evrc_wb" | "qcelp_13k"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::model::{
        MobileSeenUpsert, NumberPlan, NumberType, RegistrationBinding, ResolvedSubscriber,
        SetRingtoneOutcome, Subscriber, SubscriberIdentity, SubscriberRingtoneCodecBlob,
        SubscriberStatus,
    };
    use crate::proto::hlr_service_server::HlrService;
    use async_trait::async_trait;
    use std::sync::Mutex;
    use tonic::Code;

    #[test]
    fn accepts_every_preencoded_ringtone_codec() {
        for codec in ["tia96", "evrc_a", "evrc_b", "evrc_wb", "qcelp_13k"] {
            assert!(is_valid_codec_name(codec));
        }
        assert!(!is_valid_codec_name("unknown"));
    }

    #[derive(Default)]
    struct StubRepo {
        resolved: Mutex<Option<ResolvedSubscriber>>,
        last_call: Mutex<Option<(Option<u32>, Option<String>)>>,
        ambiguous_hardware: std::sync::atomic::AtomicBool,
    }

    fn sample_subscriber() -> Subscriber {
        Subscriber {
            subscriber_id: Uuid::new_v4(),
            phone_number: "5550001".to_string(),
            display_name: "Test".to_string(),
            status: SubscriberStatus::Active,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            number_type: NumberType::default(),
            number_plan: NumberPlan::default(),
            has_ringtone: false,
            ringtone_duration_ms: None,
            prl_override_id: None,
            service_programming_code: None,
            firstchp_override: None,
        }
    }

    #[async_trait]
    impl HlrRepository for StubRepo {
        async fn upsert_subscriber(
            &self,
            _: &str,
            _: &str,
            _: &str,
            _: NumberType,
            _: NumberPlan,
        ) -> Result<Subscriber, String> {
            unimplemented!()
        }
        async fn get_subscriber_by_phone_number(
            &self,
            _: &str,
        ) -> Result<Option<ResolvedSubscriber>, String> {
            Ok(None)
        }
        async fn get_subscriber_by_id(
            &self,
            _: Uuid,
        ) -> Result<Option<ResolvedSubscriber>, String> {
            Ok(None)
        }
        async fn update_subscriber(
            &self,
            _: Uuid,
            _: &str,
            _: &str,
            _: &str,
            _: NumberType,
            _: NumberPlan,
        ) -> Result<Option<Subscriber>, String> {
            Ok(None)
        }
        async fn list_subscribers(&self, _: u32, _: u32) -> Result<(Vec<Subscriber>, u32), String> {
            Ok((Vec::new(), 0))
        }
        async fn delete_subscriber(&self, _: Uuid) -> Result<bool, String> {
            Ok(false)
        }
        async fn upsert_identity(
            &self,
            _: Uuid,
            _: Option<&str>,
            _: Option<u32>,
            _: Option<&str>,
        ) -> Result<SubscriberIdentity, String> {
            unimplemented!()
        }
        async fn replace_primary_identity(
            &self,
            _: Uuid,
            _: Option<&str>,
            _: Option<u32>,
            _: Option<&str>,
        ) -> Result<SubscriberIdentity, String> {
            unimplemented!()
        }
        async fn get_identities_for_subscriber(
            &self,
            _: Uuid,
        ) -> Result<Vec<SubscriberIdentity>, String> {
            Ok(Vec::new())
        }
        async fn resolve_by_identity(
            &self,
            _: &model::MobileIdentityKey,
        ) -> Result<Option<ResolvedSubscriber>, String> {
            Ok(None)
        }
        async fn resolve_by_hardware_identity(
            &self,
            esn: Option<u32>,
            meid: Option<&str>,
        ) -> Result<Option<ResolvedSubscriber>, crate::repository::HardwareLookupError> {
            if self
                .ambiguous_hardware
                .load(std::sync::atomic::Ordering::Relaxed)
            {
                return Err(crate::repository::HardwareLookupError::Ambiguous);
            }
            *self.last_call.lock().unwrap() = Some((esn, meid.map(ToOwned::to_owned)));
            Ok(self.resolved.lock().unwrap().clone())
        }
        async fn upsert_mobile_seen(
            &self,
            _: &model::MobileIdentityKey,
            _: Option<u8>,
        ) -> Result<MobileSeenUpsert, String> {
            unimplemented!()
        }
        async fn upsert_registration_binding(
            &self,
            _: RegistrationBinding,
        ) -> Result<RegistrationBinding, String> {
            unimplemented!()
        }
        async fn get_registration_binding(
            &self,
            _: Uuid,
        ) -> Result<Option<RegistrationBinding>, String> {
            Ok(None)
        }
        async fn set_ringtone(
            &self,
            _: Uuid,
            _: Vec<u8>,
            _: &str,
        ) -> Result<SetRingtoneOutcome, String> {
            unimplemented!()
        }
        async fn clear_ringtone(&self, _: Uuid) -> Result<(), String> {
            Ok(())
        }
        async fn get_ringtone_codec(
            &self,
            _: Uuid,
            _: &str,
        ) -> Result<Option<SubscriberRingtoneCodecBlob>, String> {
            Ok(None)
        }
        async fn list_prls(
            &self,
            _: u32,
            _: u32,
            _: crate::model::PrlListFilter,
        ) -> Result<(Vec<crate::model::Prl>, u32), String> {
            Ok((vec![], 0))
        }
        async fn get_prl(&self, _: Uuid) -> Result<Option<crate::model::Prl>, String> {
            Ok(None)
        }
        async fn get_default_prl(&self) -> Result<Option<crate::model::Prl>, String> {
            Ok(None)
        }
        async fn create_prl(
            &self,
            _: &str,
            _: &[u8],
            _: i32,
            _: i16,
            _: &str,
        ) -> Result<crate::model::Prl, String> {
            unimplemented!()
        }
        async fn update_prl(
            &self,
            _: Uuid,
            _: Option<&str>,
            _: Option<&[u8]>,
            _: Option<(i32, i16)>,
            _: Option<&str>,
        ) -> Result<crate::model::Prl, String> {
            unimplemented!()
        }
        async fn soft_delete_prl(
            &self,
            _: Uuid,
        ) -> Result<Result<(), crate::model::PrlDeleteBlocked>, String> {
            Ok(Ok(()))
        }
        async fn set_default_prl(&self, _: Uuid) -> Result<(), String> {
            Ok(())
        }
        async fn set_subscriber_prl_override(
            &self,
            _: Uuid,
            _: Option<Uuid>,
        ) -> Result<(), String> {
            Ok(())
        }
        async fn set_subscriber_spc(&self, _: Uuid, _: Option<String>) -> Result<(), String> {
            Ok(())
        }
        async fn set_subscriber_firstchp_override(
            &self,
            _: Uuid,
            _: Option<u16>,
        ) -> Result<(), String> {
            Ok(())
        }
        async fn set_subscriber_a_key(
            &self,
            _: &model::MobileIdentityKey,
            _: Option<Vec<u8>>,
            _: Option<Vec<u8>>,
            _: Option<Vec<u8>>,
            _: model::AuthAlgorithm,
        ) -> Result<(), String> {
            Ok(())
        }
        async fn get_subscriber_auth_state(
            &self,
            _: &model::MobileIdentityKey,
        ) -> Result<Option<model::IdentityAuthState>, String> {
            Ok(None)
        }
        async fn save_otasp_session(
            &self,
            _: &crate::model::OtaspSessionRow,
        ) -> Result<(), String> {
            Ok(())
        }
        async fn list_otasp_sessions(
            &self,
            _: crate::model::OtaspSessionFilter,
            _: u32,
            _: u32,
        ) -> Result<(Vec<crate::model::OtaspSessionRow>, u32), String> {
            Ok((Vec::new(), 0))
        }
        async fn get_otasp_session(
            &self,
            _: Uuid,
        ) -> Result<Option<crate::model::OtaspSessionRow>, String> {
            Ok(None)
        }
    }

    #[tokio::test]
    async fn resolve_by_hardware_identity_rejects_both_none() {
        let svc = HlrServiceImpl::new(Arc::new(StubRepo::default()));
        let resp = svc
            .resolve_subscriber_by_hardware_identity(Request::new(
                proto::ResolveSubscriberByHardwareIdentityRequest {
                    identity: Some(proto::HardwareIdentityKey {
                        esn: None,
                        meid: None,
                    }),
                    match_imei: false,
                },
            ))
            .await;
        let err = resp.unwrap_err();
        assert_eq!(err.code(), Code::InvalidArgument);
    }

    #[tokio::test]
    async fn resolve_by_hardware_identity_rejects_invalid_meid() {
        let svc = HlrServiceImpl::new(Arc::new(StubRepo::default()));
        let resp = svc
            .resolve_subscriber_by_hardware_identity(Request::new(
                proto::ResolveSubscriberByHardwareIdentityRequest {
                    identity: Some(proto::HardwareIdentityKey {
                        esn: None,
                        meid: Some("not-hex".to_string()),
                    }),
                    match_imei: false,
                },
            ))
            .await;
        assert_eq!(resp.unwrap_err().code(), Code::InvalidArgument);
    }

    #[tokio::test]
    async fn set_firstchp_override_accepts_in_range_and_clear() {
        let svc = HlrServiceImpl::new(Arc::new(StubRepo::default()));
        let id = Uuid::nil().to_string();
        // In range.
        svc.set_subscriber_firstchp_override(Request::new(
            proto::SetSubscriberFirstchpOverrideRequest {
                subscriber_id: id.clone(),
                firstchp_override: Some(333),
            },
        ))
        .await
        .unwrap();
        // Cleared (None).
        svc.set_subscriber_firstchp_override(Request::new(
            proto::SetSubscriberFirstchpOverrideRequest {
                subscriber_id: id,
                firstchp_override: None,
            },
        ))
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn set_firstchp_override_rejects_out_of_range() {
        let svc = HlrServiceImpl::new(Arc::new(StubRepo::default()));
        let resp = svc
            .set_subscriber_firstchp_override(Request::new(
                proto::SetSubscriberFirstchpOverrideRequest {
                    subscriber_id: Uuid::nil().to_string(),
                    firstchp_override: Some(2048),
                },
            ))
            .await;
        assert_eq!(resp.unwrap_err().code(), Code::InvalidArgument);
    }

    fn sample_identity() -> proto::MobileIdentityKey {
        proto::MobileIdentityKey {
            imsi: Some("123456789012345".to_string()),
            esn: Some(0x1234_5678),
            meid: None,
        }
    }

    #[tokio::test]
    async fn set_a_key_accepts_valid_key_and_clear() {
        let svc = HlrServiceImpl::new(Arc::new(StubRepo::default()));
        svc.set_subscriber_a_key(Request::new(proto::SetSubscriberAKeyRequest {
            identity: Some(sample_identity()),
            a_key: Some(vec![0u8; 8]),
            ssd_a: None,
            ssd_b: None,
            auth_algo: proto::AuthAlgorithm::Cave as i32,
            a_key_digits: None,
        }))
        .await
        .unwrap();
        svc.set_subscriber_a_key(Request::new(proto::SetSubscriberAKeyRequest {
            identity: Some(sample_identity()),
            a_key: Some(Vec::new()),
            ssd_a: None,
            ssd_b: None,
            auth_algo: proto::AuthAlgorithm::Unspecified as i32,
            a_key_digits: None,
        }))
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn set_a_key_rejects_wrong_length() {
        let svc = HlrServiceImpl::new(Arc::new(StubRepo::default()));
        let resp = svc
            .set_subscriber_a_key(Request::new(proto::SetSubscriberAKeyRequest {
                identity: Some(sample_identity()),
                a_key: Some(vec![0u8; 7]),
                ssd_a: None,
                ssd_b: None,
                auth_algo: proto::AuthAlgorithm::Cave as i32,
                a_key_digits: None,
            }))
            .await;
        assert_eq!(resp.unwrap_err().code(), Code::InvalidArgument);
    }

    #[tokio::test]
    async fn get_auth_state_unknown_identity_is_not_found() {
        let svc = HlrServiceImpl::new(Arc::new(StubRepo::default()));
        let resp = svc
            .get_subscriber_auth_state(Request::new(proto::GetSubscriberAuthStateRequest {
                identity: Some(sample_identity()),
            }))
            .await;
        assert_eq!(resp.unwrap_err().code(), Code::NotFound);
    }

    #[tokio::test]
    async fn resolve_by_hardware_identity_forwards_esn_only() {
        let stub = Arc::new(StubRepo::default());
        let svc = HlrServiceImpl::new(stub.clone());
        let resp = svc
            .resolve_subscriber_by_hardware_identity(Request::new(
                proto::ResolveSubscriberByHardwareIdentityRequest {
                    identity: Some(proto::HardwareIdentityKey {
                        esn: Some(0x1234_5678),
                        meid: None,
                    }),
                    match_imei: false,
                },
            ))
            .await
            .unwrap()
            .into_inner();
        assert!(resp.subscriber.is_none());
        let call = stub.last_call.lock().unwrap().clone().unwrap();
        assert_eq!(call, (Some(0x1234_5678), None));
    }

    #[tokio::test]
    async fn resolve_by_hardware_identity_returns_subscriber() {
        let stub = Arc::new(StubRepo::default());
        let subscriber = sample_subscriber();
        *stub.resolved.lock().unwrap() = Some(ResolvedSubscriber {
            subscriber: subscriber.clone(),
            identities: Vec::new(),
            primary_identity: None,
            binding: None,
        });
        let svc = HlrServiceImpl::new(stub.clone());
        let resp = svc
            .resolve_subscriber_by_hardware_identity(Request::new(
                proto::ResolveSubscriberByHardwareIdentityRequest {
                    identity: Some(proto::HardwareIdentityKey {
                        esn: None,
                        meid: Some("A000000123ABCD".to_string()),
                    }),
                    match_imei: false,
                },
            ))
            .await
            .unwrap()
            .into_inner();
        assert_eq!(
            resp.subscriber.unwrap().subscriber_id,
            subscriber.subscriber_id.to_string()
        );
        let call = stub.last_call.lock().unwrap().clone().unwrap();
        assert_eq!(call, (None, Some("a000000123abcd".to_string())));
    }
    #[tokio::test]
    async fn hardware_ambiguity_has_a_structured_grpc_status() {
        let repo = Arc::new(StubRepo::default());
        repo.ambiguous_hardware
            .store(true, std::sync::atomic::Ordering::Relaxed);
        let service = HlrServiceImpl::new(repo);
        let error = service
            .resolve_subscriber_by_hardware_identity(Request::new(
                proto::ResolveSubscriberByHardwareIdentityRequest {
                    identity: Some(proto::HardwareIdentityKey {
                        esn: Some(0x1234_5678),
                        meid: None,
                    }),
                    match_imei: false,
                },
            ))
            .await
            .unwrap_err();
        assert_eq!(error.code(), Code::FailedPrecondition);
    }
}
