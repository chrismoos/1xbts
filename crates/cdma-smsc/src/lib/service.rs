use std::net::SocketAddr;
use std::sync::Arc;

use cdma_rpc_error::{Domain, ErrorReason};
use tonic::transport::Server;
use tonic::{Request, Response, Status};
use uuid::Uuid;

use crate::delivery::{AttemptResult, DeliveryHub};
use crate::dispatcher::{DispatchQueue, Dispatcher};
use crate::model::{
    DeliveryAttemptState, SmsDeliveryAttempt, SmsDestination, SmsState, SmsSubmission,
};
use crate::proto::{
    CreateDeliveryAttemptRequest, CreateDeliveryAttemptResponse, CreateSmsSubmissionRequest,
    CreateSmsSubmissionResponse, DeliveryCommand, DeliveryOutcome, GetSmsSubmissionRequest,
    GetSmsSubmissionResponse, ListSmsSubmissionsRequest, ListSmsSubmissionsResponse,
    SubmitMoRequest, SubmitMoResponse, SubmitMtRequest, SubmitMtResponse,
    UpdateDeliveryAttemptStateRequest, UpdateDeliveryAttemptStateResponse,
    UpdateSmsSubmissionStateRequest, UpdateSmsSubmissionStateResponse,
    smsc_service_server::SmscService,
};
use crate::repository::{PostgresSmscRepository, SmscRepository};
use cdma_interconnect_smpp::InterconnectLink;

pub struct SmscServiceImpl {
    repo: Arc<dyn SmscRepository>,
    deliveries: Arc<DeliveryHub>,
    /// Accepted messages must have a dispatcher or they are never delivered.
    dispatcher: Option<Arc<Dispatcher>>,
    dispatch_queue: Option<DispatchQueue>,
}

impl SmscServiceImpl {
    pub fn new(repo: Arc<dyn SmscRepository>) -> Self {
        Self {
            repo,
            deliveries: DeliveryHub::new(),
            dispatcher: None,
            dispatch_queue: None,
        }
    }

    pub fn with_hub(repo: Arc<dyn SmscRepository>, deliveries: Arc<DeliveryHub>) -> Self {
        Self {
            repo,
            deliveries,
            dispatcher: None,
            dispatch_queue: None,
        }
    }

    pub fn with_dispatcher(repo: Arc<dyn SmscRepository>, dispatcher: Arc<Dispatcher>) -> Self {
        Self {
            repo,
            deliveries: dispatcher.deliveries(),
            dispatch_queue: Some(DispatchQueue::start(dispatcher.clone())),
            dispatcher: Some(dispatcher),
        }
    }

    pub fn deliveries(&self) -> Arc<DeliveryHub> {
        self.deliveries.clone()
    }
}

pub async fn run_grpc_server(
    addr: SocketAddr,
    repo: Arc<dyn SmscRepository>,
) -> Result<(), tonic::transport::Error> {
    run_grpc_server_with_hub(addr, repo, DeliveryHub::new()).await
}

pub async fn run_grpc_server_with_hub(
    addr: SocketAddr,
    repo: Arc<dyn SmscRepository>,
    deliveries: Arc<DeliveryHub>,
) -> Result<(), tonic::transport::Error> {
    Server::builder()
        .add_service(crate::proto::smsc_service_server::SmscServiceServer::new(
            SmscServiceImpl::with_hub(repo, deliveries),
        ))
        .serve(addr)
        .await
}

pub async fn run_grpc_server_with_dispatcher(
    addr: SocketAddr,
    repo: Arc<dyn SmscRepository>,
    dispatcher: Arc<Dispatcher>,
) -> Result<(), tonic::transport::Error> {
    Server::builder()
        .add_service(crate::proto::smsc_service_server::SmscServiceServer::new(
            SmscServiceImpl::with_dispatcher(repo, dispatcher),
        ))
        .serve(addr)
        .await
}

/// Binds the SMSC gRPC listener and serves it on a background task.
///
/// The returned handle resolves when the server stops, which for a bind
/// failure is almost immediately. A caller that waits only on a shutdown
/// signal and ignores it leaves the process up with a dead listener.
pub async fn spawn_configured_smsc_service(
    config: crate::SmscNodeConfig,
) -> Result<(SocketAddr, tokio::task::JoinHandle<()>), String> {
    let addr = config.grpc_listen_addr;
    let repo: Arc<dyn SmscRepository> =
        Arc::new(PostgresSmscRepository::connect_from_config(&config).await?);
    let hlr = cdma_hlr::repository::GrpcHlrRepository::connect_lazy(&config.hlr_endpoint)
        .map_err(|e| format!("invalid HLR endpoint {:?}: {e}", config.hlr_endpoint))?;
    let mut dispatcher = Dispatcher::new(
        repo.clone(),
        crate::routing::Router::new(Arc::new(hlr)),
        DeliveryHub::new(),
    );
    if let Some(settings) = &config.smpp {
        log::info!(
            "SMSC interconnect as {} to {:?}",
            settings.system_id,
            settings.endpoint()
        );
        dispatcher = dispatcher.with_interconnect(Arc::new(InterconnectLink::start(
            settings.clone(),
            Arc::new(crate::ingress::RepositoryMtSink::new(repo.clone())),
        )));
    }
    let dispatcher = Arc::new(dispatcher);
    tokio::spawn(
        crate::scheduler::Scheduler::new(repo.clone(), dispatcher.clone(), (&config.sweep).into())
            .run(),
    );
    let served = tokio::spawn(async move {
        if let Err(error) = run_grpc_server_with_dispatcher(addr, repo, dispatcher).await {
            log::error!("SMSC gRPC server error: {error}");
        }
    });
    Ok((addr, served))
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
        teleservice_id: s.teleservice_id.map(u32::from),
        raw_user_data: s.raw_user_data.clone(),
        from_interconnect: s.from_interconnect,
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

const SMSC: Domain = Domain("smsc");

fn parse_uuid(s: &str, field: &str) -> Result<Uuid, Status> {
    Uuid::parse_str(s)
        .map_err(|_| SMSC.invalid_field(field, format!("The ID \"{s}\" is not valid")))
}

fn cdma_smsc_lib_options<'a>(
    teleservice_id: Option<u16>,
    raw_user_data: Option<&'a [u8]>,
) -> crate::repository::CreateSubmissionOptions<'a> {
    crate::repository::CreateSubmissionOptions {
        teleservice_id,
        raw_user_data,
        from_interconnect: false,
    }
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
            return Err(SMSC.invalid_field(
                "destination",
                "A destination phone number, ESN or IMSI is required",
            ));
        };

        let teleservice_id = req
            .teleservice_id
            .map(|v| u16::try_from(v).map_err(|_| teleservice_out_of_range()))
            .transpose()?;
        let options = cdma_smsc_lib_options(teleservice_id, req.raw_user_data.as_deref());

        let submission = self
            .repo
            .create_submission(
                &req.originating_number,
                destination,
                &req.text,
                orig_sub_id,
                dest_sub_id,
                options,
            )
            .await
            .map_err(|e| {
                log::error!("db error: {e}");
                SMSC.database()
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
        let fingerprint = req.fingerprint.ok_or_else(|| {
            SMSC.invalid_field("fingerprint", "A message fingerprint is required")
        })?;
        let fingerprint = crate::model::MoSmsFingerprint {
            teleservice_id: u16::try_from(fingerprint.teleservice_id)
                .map_err(|_| teleservice_out_of_range())?,
            message_type: u8::try_from(fingerprint.message_type).map_err(|_| {
                SMSC.invalid_field("message_type", "The message type must be 255 or less")
            })?,
            message_id: u16::try_from(fingerprint.message_id).map_err(|_| {
                SMSC.invalid_field("message_id", "The message ID must be 65535 or less")
            })?,
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
                SMSC.database()
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
        let state = SmsState::from_str(&req.state).ok_or_else(|| {
            SMSC.invalid_field("state", format!("{} is not a valid state", req.state))
        })?;

        let submission = self
            .repo
            .update_submission_state(sms_id, state, req.failure_reason)
            .await
            .map_err(|e| {
                log::error!("db error: {e}");
                SMSC.database()
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
                SMSC.database()
            })?
            .ok_or_else(|| {
                SMSC.not_found(ErrorReason::SmsNotFound, "Message not found")
                    .with("sms_id", sms_id)
                    .status()
            })?;

        let attempts = self.repo.get_delivery_attempts(sms_id).await.map_err(|e| {
            log::error!("db error: {e}");
            SMSC.database()
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
                req.originating_number.as_deref(),
            )
            .await
            .map_err(|e| {
                log::error!("db error: {e}");
                SMSC.database()
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
                SMSC.database()
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
        let state = DeliveryAttemptState::from_str(&req.state).ok_or_else(|| {
            SMSC.invalid_field("state", format!("{} is not a valid state", req.state))
        })?;

        let attempt = self
            .repo
            .update_delivery_attempt_state(attempt_id, state, req.failure_reason)
            .await
            .map_err(|e| {
                log::error!("db error: {e}");
                SMSC.database()
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
                SMSC.database()
            })?;
        Ok(Response::new(()))
    }

    async fn submit_mo(
        &self,
        request: Request<SubmitMoRequest>,
    ) -> Result<Response<SubmitMoResponse>, Status> {
        let req = request.into_inner();
        let originating_subscriber_id = req
            .originating_subscriber_id
            .as_deref()
            .map(|id| parse_uuid(id, "originating_subscriber_id"))
            .transpose()?;
        // Handset retransmissions preserve the teleservice ID, message type, and message ID.
        let wire = req.fingerprint.ok_or_else(|| {
            SMSC.invalid_field("fingerprint", "A message fingerprint is required")
        })?;
        let fingerprint = crate::model::MoSmsFingerprint {
            teleservice_id: u16::try_from(wire.teleservice_id)
                .map_err(|_| teleservice_out_of_range())?,
            message_type: u8::try_from(wire.message_type).map_err(|_| {
                SMSC.invalid_field("message_type", "The message type must be 255 or less")
            })?,
            message_id: u16::try_from(wire.message_id).map_err(|_| {
                SMSC.invalid_field("message_id", "The message ID must be 65535 or less")
            })?,
        };
        let (submission, created) = self
            .repo
            .create_or_get_recent_mo_submission(
                &req.originating_number,
                &req.destination_number,
                &req.text,
                originating_subscriber_id,
                None,
                &fingerprint,
            )
            .await
            .map_err(|e| {
                log::error!("db error: {e}");
                SMSC.database()
            })?;
        if created && let Some(queue) = &self.dispatch_queue {
            queue.enqueue(submission.clone(), None);
        }
        Ok(Response::new(SubmitMoResponse {
            sms_id: submission.sms_id.to_string(),
            accepted: created,
        }))
    }

    async fn submit_mt(
        &self,
        request: Request<SubmitMtRequest>,
    ) -> Result<Response<SubmitMtResponse>, Status> {
        use crate::proto::submit_mt_request::Destination;
        let req = request.into_inner();
        let destination = match req.destination {
            Some(Destination::Number(number)) => SmsDestination::PhoneNumber(number),
            Some(Destination::Esn(esn)) => SmsDestination::Esn(
                u32::try_from(esn)
                    .map_err(|_| SMSC.invalid_field("esn", "The ESN must fit in 32 bits"))?,
            ),
            Some(Destination::Imsi(imsi)) => SmsDestination::Imsi(imsi),
            None => {
                return Err(SMSC.invalid_field(
                    "destination",
                    "A destination phone number, ESN or IMSI is required",
                ));
            }
        };
        let destination_subscriber_id = req
            .destination_subscriber_id
            .as_deref()
            .map(|id| parse_uuid(id, "destination_subscriber_id"))
            .transpose()?;
        let submission = self
            .repo
            .create_submission(
                &req.originating_number,
                destination,
                &req.text,
                None,
                destination_subscriber_id,
                cdma_smsc_lib_options(
                    req.teleservice_id.map(|t| t as u16),
                    req.raw_user_data.as_deref(),
                ),
            )
            .await
            .map_err(|e| {
                log::error!("db error: {e}");
                SMSC.database()
            })?;

        if let Some(queue) = &self.dispatch_queue {
            queue.enqueue(submission.clone(), req.serving_node);
        }
        Ok(Response::new(SubmitMtResponse {
            sms_id: submission.sms_id.to_string(),
        }))
    }

    type DeliveryStreamStream = DeliveryCommandStream;

    async fn delivery_stream(
        &self,
        request: Request<tonic::Streaming<DeliveryOutcome>>,
    ) -> Result<Response<Self::DeliveryStreamStream>, Status> {
        let mut outcomes = request.into_inner();
        let commands = self.deliveries.attach().await;
        log::info!("SMSC: an MSC opened the delivery stream");

        let hub = self.deliveries.clone();
        let dispatcher = self.dispatcher.clone();
        tokio::spawn(async move {
            while let Ok(Some(outcome)) = outcomes.message().await {
                let Ok(sms_id) = Uuid::parse_str(&outcome.sms_id) else {
                    log::warn!("SMSC: an outcome arrived for {}", outcome.sms_id);
                    continue;
                };
                let Ok(attempt_id) = Uuid::parse_str(&outcome.attempt_id) else {
                    log::warn!("SMSC: outcome for {sms_id} has no valid attempt ID");
                    continue;
                };
                let result = AttemptResult::from_outcome(&outcome);
                log::info!("SMSC: {sms_id} attempt result {result:?}");
                match dispatcher.as_ref() {
                    Some(dispatcher) => dispatcher.record_outcome(sms_id, attempt_id, result).await,
                    None => log::error!("SMSC: nothing to record the outcome for {sms_id}"),
                }
            }
            log::info!("SMSC: the delivery stream closed");
            hub.detach().await;
        });

        Ok(Response::new(DeliveryCommandStream(commands)))
    }
}

pub struct DeliveryCommandStream(tokio::sync::mpsc::Receiver<DeliveryCommand>);

impl futures_core::Stream for DeliveryCommandStream {
    type Item = Result<DeliveryCommand, Status>;

    fn poll_next(
        mut self: std::pin::Pin<&mut Self>,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Option<Self::Item>> {
        self.0.poll_recv(cx).map(|next| next.map(Ok))
    }
}

fn teleservice_out_of_range() -> Status {
    SMSC.invalid_field("teleservice_id", "The teleservice ID must be 65535 or less")
}
