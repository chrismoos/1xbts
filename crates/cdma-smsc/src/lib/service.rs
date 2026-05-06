use std::net::SocketAddr;
use std::sync::Arc;

use tonic::transport::Server;
use tonic::{Request, Response, Status};
use uuid::Uuid;

use crate::model::{
    DeliveryAttemptState, SmsDeliveryAttempt, SmsDestination, SmsState, SmsSubmission,
};
use crate::proto::{
    CreateDeliveryAttemptRequest, CreateDeliveryAttemptResponse, CreateSmsSubmissionRequest,
    CreateSmsSubmissionResponse, GetSmsSubmissionRequest, GetSmsSubmissionResponse,
    ListSmsSubmissionsRequest, ListSmsSubmissionsResponse, UpdateDeliveryAttemptStateRequest,
    UpdateDeliveryAttemptStateResponse, UpdateSmsSubmissionStateRequest,
    UpdateSmsSubmissionStateResponse, smsc_service_server::SmscService,
};
use crate::repository::{PostgresSmscRepository, SmscRepository};

pub struct SmscServiceImpl {
    repo: Arc<dyn SmscRepository>,
}

impl SmscServiceImpl {
    pub fn new(repo: Arc<dyn SmscRepository>) -> Self {
        Self { repo }
    }
}

pub async fn run_grpc_server(
    addr: SocketAddr,
    repo: Arc<dyn SmscRepository>,
) -> Result<(), tonic::transport::Error> {
    Server::builder()
        .add_service(crate::proto::smsc_service_server::SmscServiceServer::new(
            SmscServiceImpl::new(repo),
        ))
        .serve(addr)
        .await
}

pub async fn spawn_configured_smsc_service(
    config: crate::SmscNodeConfig,
) -> Result<SocketAddr, String> {
    let addr = config.grpc_listen_addr;
    let repo = PostgresSmscRepository::connect_from_config(&config).await?;
    tokio::spawn(async move {
        if let Err(error) = run_grpc_server(addr, Arc::new(repo)).await {
            log::error!("SMSC gRPC server error: {error}");
        }
    });
    Ok(addr)
}

fn datetime_to_timestamp(dt: chrono::DateTime<chrono::Utc>) -> prost_types::Timestamp {
    prost_types::Timestamp {
        seconds: dt.timestamp(),
        nanos: dt.timestamp_subsec_nanos() as i32,
    }
}

fn submission_to_proto(s: &SmsSubmission) -> crate::proto::SmsSubmission {
    crate::proto::SmsSubmission {
        sms_id: s.sms_id.to_string(),
        originating_number: s.originating_number.clone(),
        destination_number: s.destination_number.clone(),
        destination_esn: s.destination_esn.map(|v| v as u64),
        destination_imsi: s.destination_imsi.clone(),
        originating_subscriber_id: s.originating_subscriber_id.map(|id| id.to_string()),
        destination_subscriber_id: s.destination_subscriber_id.map(|id| id.to_string()),
        text: s.text.clone(),
        state: s.state.as_str().to_string(),
        failure_reason: s.failure_reason.clone(),
        created_at: Some(datetime_to_timestamp(s.created_at)),
        updated_at: Some(datetime_to_timestamp(s.updated_at)),
    }
}

fn delivery_attempt_to_proto(a: &SmsDeliveryAttempt) -> crate::proto::SmsDeliveryAttempt {
    crate::proto::SmsDeliveryAttempt {
        sms_delivery_attempt_id: a.sms_delivery_attempt_id.to_string(),
        sms_id: a.sms_id.to_string(),
        attempt_number: a.attempt_number,
        state: a.state.as_str().to_string(),
        target_subscriber_id: a.target_subscriber_id.map(|id| id.to_string()),
        failure_reason: a.failure_reason.clone(),
        requested_at: Some(datetime_to_timestamp(a.requested_at)),
        completed_at: a.completed_at.map(datetime_to_timestamp),
        created_at: Some(datetime_to_timestamp(a.created_at)),
        updated_at: Some(datetime_to_timestamp(a.updated_at)),
    }
}

fn parse_uuid(s: &str, field: &str) -> Result<Uuid, Status> {
    Uuid::parse_str(s)
        .map_err(|_| Status::invalid_argument(format!("invalid UUID for {}: {}", field, s)))
}

#[tonic::async_trait]
impl SmscService for SmscServiceImpl {
    async fn create_sms_submission(
        &self,
        request: Request<CreateSmsSubmissionRequest>,
    ) -> Result<Response<CreateSmsSubmissionResponse>, Status> {
        let req = request.into_inner();

        let orig_sub_id = match req.originating_subscriber_id {
            Some(ref id) => Some(parse_uuid(id, "originating_subscriber_id")?),
            None => None,
        };
        let dest_sub_id = match req.destination_subscriber_id {
            Some(ref id) => Some(parse_uuid(id, "destination_subscriber_id")?),
            None => None,
        };

        let destination = if let Some(imsi) = req.destination_imsi {
            SmsDestination::Imsi(imsi)
        } else if let Some(esn) = req.destination_esn {
            SmsDestination::Esn(esn as u32)
        } else if let Some(number) = req.destination_number {
            SmsDestination::PhoneNumber(number)
        } else {
            return Err(Status::invalid_argument(
                "one of destination_number, destination_esn, or destination_imsi is required",
            ));
        };

        let submission = self
            .repo
            .create_submission(
                &req.originating_number,
                destination,
                &req.text,
                orig_sub_id,
                dest_sub_id,
            )
            .await
            .map_err(|e| {
                log::error!("db error: {e}");
                Status::internal("internal error")
            })?;

        Ok(Response::new(CreateSmsSubmissionResponse {
            submission: Some(submission_to_proto(&submission)),
        }))
    }

    async fn create_or_get_recent_mo_submission(
        &self,
        request: Request<crate::proto::CreateOrGetRecentMoSubmissionRequest>,
    ) -> Result<Response<crate::proto::CreateOrGetRecentMoSubmissionResponse>, Status> {
        let req = request.into_inner();
        let orig_sub_id = match req.originating_subscriber_id {
            Some(ref id) => Some(parse_uuid(id, "originating_subscriber_id")?),
            None => None,
        };
        let dest_sub_id = match req.destination_subscriber_id {
            Some(ref id) => Some(parse_uuid(id, "destination_subscriber_id")?),
            None => None,
        };
        let fingerprint = req
            .fingerprint
            .ok_or_else(|| Status::invalid_argument("fingerprint is required"))?;
        let fingerprint = crate::model::MoSmsFingerprint {
            teleservice_id: u16::try_from(fingerprint.teleservice_id)
                .map_err(|_| Status::invalid_argument("teleservice_id > 65535"))?,
            message_type: u8::try_from(fingerprint.message_type)
                .map_err(|_| Status::invalid_argument("message_type > 255"))?,
            message_id: u16::try_from(fingerprint.message_id)
                .map_err(|_| Status::invalid_argument("message_id > 65535"))?,
        };

        let (submission, created) = self
            .repo
            .create_or_get_recent_mo_submission(
                &req.originating_number,
                &req.destination_number,
                &req.text,
                orig_sub_id,
                dest_sub_id,
                &fingerprint,
            )
            .await
            .map_err(|e| {
                log::error!("db error: {e}");
                Status::internal("internal error")
            })?;

        Ok(Response::new(
            crate::proto::CreateOrGetRecentMoSubmissionResponse {
                submission: Some(submission_to_proto(&submission)),
                created,
            },
        ))
    }

    async fn update_sms_submission_state(
        &self,
        request: Request<UpdateSmsSubmissionStateRequest>,
    ) -> Result<Response<UpdateSmsSubmissionStateResponse>, Status> {
        let req = request.into_inner();
        let sms_id = parse_uuid(&req.sms_id, "sms_id")?;
        let state = SmsState::from_str(&req.state)
            .ok_or_else(|| Status::invalid_argument(format!("invalid state: {}", req.state)))?;

        let submission = self
            .repo
            .update_submission_state(sms_id, state, req.failure_reason)
            .await
            .map_err(|e| {
                log::error!("db error: {e}");
                Status::internal("internal error")
            })?;

        Ok(Response::new(UpdateSmsSubmissionStateResponse {
            submission: Some(submission_to_proto(&submission)),
        }))
    }

    async fn get_sms_submission(
        &self,
        request: Request<GetSmsSubmissionRequest>,
    ) -> Result<Response<GetSmsSubmissionResponse>, Status> {
        let req = request.into_inner();
        let sms_id = parse_uuid(&req.sms_id, "sms_id")?;

        let submission = self
            .repo
            .get_submission(sms_id)
            .await
            .map_err(|e| {
                log::error!("db error: {e}");
                Status::internal("internal error")
            })?
            .ok_or_else(|| Status::not_found(format!("submission {} not found", sms_id)))?;

        let attempts = self.repo.get_delivery_attempts(sms_id).await.map_err(|e| {
            log::error!("db error: {e}");
            Status::internal("internal error")
        })?;

        Ok(Response::new(GetSmsSubmissionResponse {
            submission: Some(submission_to_proto(&submission)),
            delivery_attempts: attempts.iter().map(delivery_attempt_to_proto).collect(),
        }))
    }

    async fn list_sms_submissions(
        &self,
        request: Request<ListSmsSubmissionsRequest>,
    ) -> Result<Response<ListSmsSubmissionsResponse>, Status> {
        let req = request.into_inner();
        const MAX_LIMIT: u32 = 10_000;
        let limit = req.limit.unwrap_or(50).min(MAX_LIMIT);
        let offset = req.offset.unwrap_or(0);

        let (submissions, total) = self
            .repo
            .list_submissions(
                limit,
                offset,
                req.destination_number.as_deref(),
                req.destination_esn.map(|v| v as u32),
                req.destination_imsi.as_deref(),
                req.state.as_deref(),
            )
            .await
            .map_err(|e| {
                log::error!("db error: {e}");
                Status::internal("internal error")
            })?;

        Ok(Response::new(ListSmsSubmissionsResponse {
            submissions: submissions.iter().map(submission_to_proto).collect(),
            total,
        }))
    }

    async fn create_delivery_attempt(
        &self,
        request: Request<CreateDeliveryAttemptRequest>,
    ) -> Result<Response<CreateDeliveryAttemptResponse>, Status> {
        let req = request.into_inner();
        let sms_id = parse_uuid(&req.sms_id, "sms_id")?;
        let target_subscriber_id = req
            .target_subscriber_id
            .as_deref()
            .map(|s| parse_uuid(s, "target_subscriber_id"))
            .transpose()?;

        let attempt = self
            .repo
            .create_delivery_attempt(sms_id, target_subscriber_id)
            .await
            .map_err(|e| {
                log::error!("db error: {e}");
                Status::internal("internal error")
            })?;

        Ok(Response::new(CreateDeliveryAttemptResponse {
            attempt: Some(delivery_attempt_to_proto(&attempt)),
        }))
    }

    async fn update_delivery_attempt_state(
        &self,
        request: Request<UpdateDeliveryAttemptStateRequest>,
    ) -> Result<Response<UpdateDeliveryAttemptStateResponse>, Status> {
        let req = request.into_inner();
        let attempt_id = parse_uuid(&req.sms_delivery_attempt_id, "sms_delivery_attempt_id")?;
        let state = DeliveryAttemptState::from_str(&req.state)
            .ok_or_else(|| Status::invalid_argument(format!("invalid state: {}", req.state)))?;

        let attempt = self
            .repo
            .update_delivery_attempt_state(attempt_id, state, req.failure_reason)
            .await
            .map_err(|e| {
                log::error!("db error: {e}");
                Status::internal("internal error")
            })?;

        Ok(Response::new(UpdateDeliveryAttemptStateResponse {
            attempt: Some(delivery_attempt_to_proto(&attempt)),
        }))
    }

    async fn update_destination_subscriber(
        &self,
        request: Request<crate::proto::UpdateDestinationSubscriberRequest>,
    ) -> Result<Response<()>, Status> {
        let req = request.into_inner();
        let sms_id = parse_uuid(&req.sms_id, "sms_id")?;
        let destination_subscriber_id =
            parse_uuid(&req.destination_subscriber_id, "destination_subscriber_id")?;
        self.repo
            .update_destination_subscriber(sms_id, destination_subscriber_id)
            .await
            .map_err(|e| {
                log::error!("db error: {e}");
                Status::internal("internal error")
            })?;
        Ok(Response::new(()))
    }
}
