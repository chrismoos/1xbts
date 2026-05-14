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

pub struct HlrServiceImpl {
    repo: Arc<dyn HlrRepository>,
}

impl HlrServiceImpl {
    pub fn new(repo: Arc<dyn HlrRepository>) -> Self {
        Self { repo }
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

pub async fn spawn_configured_hlr_service(
    config: crate::HlrNodeConfig,
) -> Result<SocketAddr, String> {
    let addr = config.grpc_listen_addr;
    let repo = PostgresHlrRepository::connect_from_config(&config).await?;
    tokio::spawn(async move {
        if let Err(error) = run_grpc_server(addr, Arc::new(repo)).await {
            eprintln!("HLR gRPC server error: {error}");
        }
    });
    Ok(addr)
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
        status: s.status.as_str().to_string(),
        created_at: Some(datetime_to_timestamp(s.created_at)),
        updated_at: Some(datetime_to_timestamp(s.updated_at)),
        number_type: number_type_to_proto(s.number_type) as i32,
        number_plan: number_plan_to_proto(s.number_plan) as i32,
        has_ringtone: s.has_ringtone,
        ringtone_duration_ms: s.ringtone_duration_ms,
    }
}

fn identity_to_proto(i: &model::SubscriberIdentity) -> proto::SubscriberIdentity {
    proto::SubscriberIdentity {
        subscriber_identity_id: i.subscriber_identity_id.to_string(),
        subscriber_id: i.subscriber_id.to_string(),
        imsi: i.imsi.clone(),
        esn: i.esn,
        is_primary: i.is_primary,
        created_at: Some(datetime_to_timestamp(i.created_at)),
    }
}

fn binding_to_proto(b: &model::RegistrationBinding) -> proto::RegistrationBinding {
    proto::RegistrationBinding {
        subscriber_id: b.subscriber_id.to_string(),
        serving_node_id: b.serving_node_id.clone(),
        state: b.state.as_str().to_string(),
        imsi: b.imsi.clone(),
        esn: b.esn,
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

        let number_type = number_type_from_proto(req.number_type);
        let number_plan = number_plan_from_proto(req.number_plan);

        let subscriber = self
            .repo
            .upsert_subscriber(
                &req.phone_number,
                &req.display_name,
                &req.status,
                number_type,
                number_plan,
            )
            .await
            .map_err(|e| {
                log::error!("HLR: {e}");
                Status::internal("internal error")
            })?;

        let has_identity = req.imsi.is_some() || req.esn.is_some();

        let identity = if has_identity {
            let id = self
                .repo
                .upsert_identity(subscriber.subscriber_id, req.imsi.as_deref(), req.esn)
                .await
                .map_err(|e| {
                    log::error!("HLR: {e}");
                    Status::internal("internal error")
                })?;
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
        let identity = self
            .repo
            .upsert_identity(subscriber_id, req.imsi.as_deref(), req.esn)
            .await
            .map_err(Status::internal)?;
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
        let identity = self
            .repo
            .replace_primary_identity(subscriber_id, req.imsi.as_deref(), req.esn)
            .await
            .map_err(Status::internal)?;
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
        validate_optional_imsi(req.imsi.as_deref())?;
        let mob_p_rev = req
            .mob_p_rev
            .map(|value| {
                u8::try_from(value).map_err(|_| Status::invalid_argument("mob_p_rev > 255"))
            })
            .transpose()?;
        let result = self
            .repo
            .upsert_mobile_seen(req.esn, req.imsi.as_deref(), mob_p_rev)
            .await
            .map_err(Status::internal)?;
        Ok(Response::new(proto::UpsertMobileSeenResponse {
            result: Some(mobile_seen_to_proto(&result)),
        }))
    }

    async fn get_subscriber_by_phone_number(
        &self,
        request: Request<proto::GetSubscriberByPhoneNumberRequest>,
    ) -> Result<Response<proto::GetSubscriberByPhoneNumberResponse>, Status> {
        let req = request.into_inner();

        let subscriber = self
            .repo
            .get_subscriber_by_phone_number(&req.phone_number)
            .await
            .map_err(|e| {
                log::error!("HLR: {e}");
                Status::internal("internal error")
            })?
            .ok_or_else(|| Status::not_found("subscriber not found"))?;

        let identities = self
            .repo
            .get_identities_for_subscriber(subscriber.subscriber_id)
            .await
            .map_err(|e| {
                log::error!("HLR: {e}");
                Status::internal("internal error")
            })?;

        let binding = self
            .repo
            .get_registration_binding(subscriber.subscriber_id)
            .await
            .map_err(|e| {
                log::error!("HLR: {e}");
                Status::internal("internal error")
            })?;

        Ok(Response::new(proto::GetSubscriberByPhoneNumberResponse {
            subscriber: Some(subscriber_to_proto(&subscriber)),
            identities: identities.iter().map(identity_to_proto).collect(),
            binding: binding.as_ref().map(binding_to_proto),
        }))
    }

    async fn get_subscriber(
        &self,
        request: Request<proto::GetSubscriberRequest>,
    ) -> Result<Response<proto::GetSubscriberResponse>, Status> {
        let req = request.into_inner();
        let subscriber_id = parse_uuid(&req.subscriber_id)?;

        let subscriber = self
            .repo
            .get_subscriber_by_id(subscriber_id)
            .await
            .map_err(|e| {
                log::error!("HLR: {e}");
                Status::internal("internal error")
            })?
            .ok_or_else(|| Status::not_found("subscriber not found"))?;

        let identities = self
            .repo
            .get_identities_for_subscriber(subscriber_id)
            .await
            .map_err(|e| {
                log::error!("HLR: {e}");
                Status::internal("internal error")
            })?;

        let binding = self
            .repo
            .get_registration_binding(subscriber_id)
            .await
            .map_err(|e| {
                log::error!("HLR: {e}");
                Status::internal("internal error")
            })?;

        Ok(Response::new(proto::GetSubscriberResponse {
            subscriber: Some(subscriber_to_proto(&subscriber)),
            identities: identities.iter().map(identity_to_proto).collect(),
            binding: binding.as_ref().map(binding_to_proto),
        }))
    }

    async fn resolve_subscriber_by_identity(
        &self,
        request: Request<proto::ResolveSubscriberByIdentityRequest>,
    ) -> Result<Response<proto::ResolveSubscriberByIdentityResponse>, Status> {
        let req = request.into_inner();

        let subscriber = self
            .repo
            .resolve_by_identity(req.esn, req.imsi.as_deref())
            .await
            .map_err(|e| {
                log::error!("HLR: {e}");
                Status::internal("internal error")
            })?;

        let binding = if let Some(ref sub) = subscriber {
            self.repo
                .get_registration_binding(sub.subscriber_id)
                .await
                .map_err(|e| {
                    log::error!("HLR: {e}");
                    Status::internal("internal error")
                })?
        } else {
            None
        };

        Ok(Response::new(proto::ResolveSubscriberByIdentityResponse {
            subscriber: subscriber.as_ref().map(subscriber_to_proto),
            binding: binding.as_ref().map(binding_to_proto),
        }))
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
            serving_node_id: req.serving_node_id,
            state: model::RegistrationState::from_str(&req.state).map_err(|e| {
                log::error!("HLR: {e}");
                Status::internal("internal error")
            })?,
            imsi: req.imsi,
            esn: req.esn,
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

        let subscriber = self
            .repo
            .get_subscriber_by_phone_number(&req.destination_number)
            .await
            .map_err(|e| {
                log::error!("HLR: {e}");
                Status::internal("internal error")
            })?
            .ok_or_else(|| Status::not_found("subscriber not found"))?;

        let binding = self
            .repo
            .get_registration_binding(subscriber.subscriber_id)
            .await
            .map_err(|e| {
                log::error!("HLR: {e}");
                Status::internal("internal error")
            })?;

        Ok(Response::new(proto::ResolveSmsTargetResponse {
            subscriber: Some(subscriber_to_proto(&subscriber)),
            binding: binding.as_ref().map(binding_to_proto),
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

        let subscriber = self
            .repo
            .update_subscriber(
                subscriber_id,
                &req.phone_number,
                &req.display_name,
                &req.status,
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

        let has_identity = req.imsi.is_some() || req.esn.is_some();

        let identity = if has_identity {
            Some(
                self.repo
                    .replace_primary_identity(subscriber_id, req.imsi.as_deref(), req.esn)
                    .await
                    .map_err(|e| {
                        log::error!("HLR: {e}");
                        Status::internal("internal error")
                    })?,
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
}

fn is_valid_codec_name(name: &str) -> bool {
    matches!(name, "evrc_a" | "evrc_b" | "evrc_wb")
}
