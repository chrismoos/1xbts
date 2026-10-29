use std::sync::Arc;
use std::time::Duration;

use uuid::Uuid;

use crate::delivery::{AttemptResult, DeliveryHub, PushResult};
use crate::model::SmsSubmission;
use crate::proto::{DeliveryCommand, DeliveryTarget, delivery_target::Identity};
use crate::repository::{DeliveryCompletion, SmscRepository};
use crate::routing::{MoSmsRoute, Resolution, Router, route};
use cdma_interconnect::{InterconnectEgress, InterconnectError, InterconnectSms};

const INTERCONNECT_SUBMIT_TIMEOUT: Duration = Duration::from_secs(45);

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Deferral {
    NoMsc,
    Offline,
    UnknownDestination,
    NotAddressable,
    Storage(String),
    Interconnect(String),
    Undeliverable(String),
}

struct DeliveryAddress {
    identity: Identity,
    subscriber_id: Option<Uuid>,
    serving_node: Option<String>,
}

pub struct Dispatcher {
    repo: Arc<dyn SmscRepository>,
    router: Router,
    deliveries: Arc<DeliveryHub>,
    interconnect: Option<Arc<dyn InterconnectEgress>>,
}

impl Dispatcher {
    pub fn new(
        repo: Arc<dyn SmscRepository>,
        router: Router,
        deliveries: Arc<DeliveryHub>,
    ) -> Self {
        Self {
            repo,
            router,
            deliveries,
            interconnect: None,
        }
    }

    pub fn with_interconnect(mut self, egress: Arc<dyn InterconnectEgress>) -> Self {
        self.interconnect = Some(egress);
        self
    }

    pub fn deliveries(&self) -> Arc<DeliveryHub> {
        self.deliveries.clone()
    }

    pub async fn dispatch(&self, submission: &SmsSubmission) -> Result<(), Deferral> {
        let Some(number) = submission.destination_number.as_deref() else {
            return self.try_deliver(submission).await;
        };
        let local = match submission.destination_subscriber_id {
            Some(id) => Some(id),
            None => match self.router.resolve_number(number).await {
                Resolution::Ready { subscriber_id, .. }
                | Resolution::Deferred { subscriber_id } => Some(subscriber_id),
                Resolution::Unknown => None,
                Resolution::Unavailable(error) => return Err(Deferral::Storage(error)),
            },
        };
        match route(
            number,
            local,
            self.interconnect.is_some(),
            submission.from_interconnect,
        ) {
            MoSmsRoute::Local { .. } => self.try_deliver(submission).await,
            MoSmsRoute::Interconnect => self.forward(submission).await,
            MoSmsRoute::Undeliverable(cause) => {
                let reason = cause.reason().to_string();
                if let Some(attempt) = self
                    .repo
                    .claim_delivery(submission.sms_id, local)
                    .await
                    .map_err(Deferral::Storage)?
                {
                    self.finish(
                        submission.sms_id,
                        attempt.sms_delivery_attempt_id,
                        DeliveryCompletion::Failed(Some(reason.clone())),
                    )
                    .await?;
                }
                Err(Deferral::Undeliverable(reason))
            }
        }
    }

    async fn forward(&self, submission: &SmsSubmission) -> Result<(), Deferral> {
        let Some(egress) = self.interconnect.as_ref() else {
            return Err(Deferral::UnknownDestination);
        };
        if !egress.is_available() {
            return Err(Deferral::Interconnect(
                "the configured interconnect is not connected".into(),
            ));
        }
        let Some(attempt) = self
            .repo
            .claim_delivery(submission.sms_id, None)
            .await
            .map_err(Deferral::Storage)?
        else {
            return Ok(());
        };
        let message = InterconnectSms {
            sms_id: submission.sms_id,
            source: submission.originating_number.clone(),
            destination: submission.destination_number.clone().unwrap_or_default(),
            text: submission.text.clone(),
            bearer_data: submission.raw_user_data.clone(),
            teleservice: submission.teleservice_id,
        };
        let result = tokio::time::timeout(INTERCONNECT_SUBMIT_TIMEOUT, egress.submit(message))
            .await
            .unwrap_or_else(|_| {
                Err(InterconnectError::Transient(
                    "interconnect submission timed out".into(),
                ))
            });
        let (outcome, error) = match result {
            Ok(()) => (DeliveryCompletion::Forwarded, None),
            Err(InterconnectError::Rejected(reason)) => (
                DeliveryCompletion::Failed(Some(reason.clone())),
                Some(Deferral::Undeliverable(reason)),
            ),
            Err(error) => {
                let reason = error.to_string();
                (
                    DeliveryCompletion::Retry(Some(reason.clone())),
                    Some(Deferral::Interconnect(reason)),
                )
            }
        };
        self.finish(submission.sms_id, attempt.sms_delivery_attempt_id, outcome)
            .await?;
        match error {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }

    pub async fn deliver_at(
        &self,
        submission: &SmsSubmission,
        imsi: &str,
        serving_node: &str,
    ) -> Result<(), Deferral> {
        self.push(
            submission,
            Identity::Imsi(imsi.to_string()),
            submission.destination_subscriber_id,
            Some(serving_node.to_string()),
        )
        .await
    }

    pub async fn try_deliver(&self, submission: &SmsSubmission) -> Result<(), Deferral> {
        let address = self.address(submission).await?;
        self.push(
            submission,
            address.identity,
            address.subscriber_id,
            address.serving_node,
        )
        .await
    }

    async fn push(
        &self,
        submission: &SmsSubmission,
        identity: Identity,
        subscriber_id: Option<Uuid>,
        serving_node: Option<String>,
    ) -> Result<(), Deferral> {
        let Some(attempt) = self
            .repo
            .claim_delivery(submission.sms_id, subscriber_id)
            .await
            .map_err(Deferral::Storage)?
        else {
            return Ok(());
        };
        let command = DeliveryCommand {
            sms_id: submission.sms_id.to_string(),
            attempt_id: attempt.sms_delivery_attempt_id.to_string(),
            target: Some(DeliveryTarget {
                identity: Some(identity),
                serving_node,
            }),
            originating_number: submission.originating_number.clone(),
            text: submission.text.clone(),
            teleservice_id: submission.teleservice_id.map(u32::from),
            raw_user_data: submission.raw_user_data.clone(),
        };
        match self.deliveries.push(command).await {
            PushResult::Sent => Ok(()),
            PushResult::NoMsc | PushResult::Backlogged => {
                self.finish(
                    submission.sms_id,
                    attempt.sms_delivery_attempt_id,
                    DeliveryCompletion::Retry(Some("no MSC available to deliver it".into())),
                )
                .await?;
                Err(Deferral::NoMsc)
            }
        }
    }

    async fn address(&self, submission: &SmsSubmission) -> Result<DeliveryAddress, Deferral> {
        if let Some(imsi) = submission.destination_imsi.as_deref() {
            return Ok(DeliveryAddress {
                identity: Identity::Imsi(imsi.to_string()),
                subscriber_id: submission.destination_subscriber_id,
                serving_node: None,
            });
        }
        if let Some(esn) = submission.destination_esn {
            return Ok(DeliveryAddress {
                identity: Identity::Esn(u64::from(esn)),
                subscriber_id: submission.destination_subscriber_id,
                serving_node: None,
            });
        }
        let Some(number) = submission.destination_number.as_deref() else {
            return Err(Deferral::NotAddressable);
        };
        match self.router.resolve_number(number).await {
            Resolution::Ready {
                imsi,
                serving_node,
                subscriber_id,
            } => Ok(DeliveryAddress {
                identity: Identity::Imsi(imsi),
                subscriber_id: Some(subscriber_id),
                serving_node: Some(serving_node),
            }),
            Resolution::Deferred { .. } => Err(Deferral::Offline),
            Resolution::Unknown => Err(Deferral::UnknownDestination),
            Resolution::Unavailable(error) => Err(Deferral::Storage(error)),
        }
    }

    async fn finish(
        &self,
        sms_id: Uuid,
        attempt_id: Uuid,
        outcome: DeliveryCompletion,
    ) -> Result<(), Deferral> {
        if !self
            .repo
            .finish_delivery(sms_id, attempt_id, outcome)
            .await
            .map_err(Deferral::Storage)?
        {
            log::debug!("SMSC: ignored stale outcome for {sms_id}/{attempt_id}");
        }
        Ok(())
    }

    pub async fn record_outcome(&self, sms_id: Uuid, attempt_id: Uuid, result: AttemptResult) {
        let outcome = match result {
            AttemptResult::Delivered => DeliveryCompletion::Delivered,
            AttemptResult::Failed(detail) => DeliveryCompletion::Failed(detail),
            AttemptResult::NotReachable(detail) => DeliveryCompletion::Retry(detail),
        };
        if let Err(error) = self.finish(sms_id, attempt_id, outcome).await {
            log::error!("SMSC: could not record outcome for {sms_id}/{attempt_id}: {error:?}");
        }
    }
}

const DISPATCH_QUEUE: usize = 64;

pub struct DispatchQueue {
    sender: tokio::sync::mpsc::Sender<DispatchRequest>,
}

struct DispatchRequest {
    submission: SmsSubmission,
    serving_node: Option<String>,
}

impl DispatchQueue {
    pub fn start(dispatcher: Arc<Dispatcher>) -> Self {
        let (sender, mut requests) = tokio::sync::mpsc::channel::<DispatchRequest>(DISPATCH_QUEUE);
        tokio::spawn(async move {
            while let Some(request) = requests.recv().await {
                let result = match (
                    request.serving_node.as_deref(),
                    request.submission.destination_imsi.as_deref(),
                ) {
                    (Some(node), Some(imsi)) => {
                        dispatcher.deliver_at(&request.submission, imsi, node).await
                    }
                    _ => dispatcher.dispatch(&request.submission).await,
                };
                if let Err(reason) = result {
                    log::info!(
                        "SMSC: dispatch for {}: {reason:?}",
                        request.submission.sms_id
                    );
                }
            }
        });
        Self { sender }
    }

    pub fn enqueue(&self, submission: SmsSubmission, serving_node: Option<String>) {
        let sms_id = submission.sms_id;
        if self
            .sender
            .try_send(DispatchRequest {
                submission,
                serving_node,
            })
            .is_err()
        {
            // Acceptance is durable. The retry sweep drains messages beyond this queue's capacity.
            log::warn!("SMSC: immediate dispatch queue is full, retaining {sms_id} for the sweep");
        }
    }
}
