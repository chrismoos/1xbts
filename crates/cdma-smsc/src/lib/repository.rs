use async_trait::async_trait;
use chrono::{DateTime, Utc};
use sqlx::PgPool;
use tonic::Code;
use uuid::Uuid;

use crate::model::{
    DeliveryAttemptState, MoSmsFingerprint, SmsDeliveryAttempt, SmsDestination, SmsState,
    SmsSubmission,
};
use crate::proto;

/// Window within which duplicate MO SMS submissions are de-duplicated.
const MO_SMS_DEDUP_WINDOW_MINUTES: i32 = 10;

#[async_trait]
pub trait SmscRepository: Send + Sync {
    async fn create_submission(
        &self,
        originating_number: &str,
        destination: SmsDestination,
        text: &str,
        originating_subscriber_id: Option<Uuid>,
        destination_subscriber_id: Option<Uuid>,
    ) -> Result<SmsSubmission, String>;

    async fn create_or_get_recent_mo_submission(
        &self,
        originating_number: &str,
        destination_number: &str,
        text: &str,
        originating_subscriber_id: Option<Uuid>,
        destination_subscriber_id: Option<Uuid>,
        fingerprint: &MoSmsFingerprint,
    ) -> Result<(SmsSubmission, bool), String>;

    async fn update_submission_state(
        &self,
        sms_id: Uuid,
        state: SmsState,
        failure_reason: Option<String>,
    ) -> Result<SmsSubmission, String>;

    async fn get_submission(&self, sms_id: Uuid) -> Result<Option<SmsSubmission>, String>;

    async fn list_submissions(
        &self,
        limit: u32,
        offset: u32,
        destination_number: Option<&str>,
        destination_esn: Option<u32>,
        destination_imsi: Option<&str>,
        state: Option<&str>,
    ) -> Result<(Vec<SmsSubmission>, u32), String>;

    async fn create_delivery_attempt(
        &self,
        sms_id: Uuid,
        target_subscriber_id: Option<Uuid>,
    ) -> Result<SmsDeliveryAttempt, String>;

    async fn update_delivery_attempt_state(
        &self,
        attempt_id: Uuid,
        state: DeliveryAttemptState,
        failure_reason: Option<String>,
    ) -> Result<SmsDeliveryAttempt, String>;

    async fn get_delivery_attempts(&self, sms_id: Uuid) -> Result<Vec<SmsDeliveryAttempt>, String>;

    async fn update_destination_subscriber(
        &self,
        sms_id: Uuid,
        destination_subscriber_id: Uuid,
    ) -> Result<(), String>;
}

/// gRPC-backed SMSC repository adapter.
///
/// Existing BSC code continues to consume `SmscRepository`; runtime wiring uses
/// this adapter so the SMSC owns the database and service boundary.
pub struct GrpcSmscRepository {
    client: proto::smsc_service_client::SmscServiceClient<tonic::transport::Channel>,
}

impl GrpcSmscRepository {
    /// Connect to an SMSC gRPC endpoint and return a repository that reuses the channel.
    pub async fn connect(endpoint: String) -> Result<Self, String> {
        let client = proto::smsc_service_client::SmscServiceClient::connect(endpoint.clone())
            .await
            .map_err(|e| format!("connect SMSC gRPC {endpoint}: {e}"))?;
        Ok(Self { client })
    }

    /// Connect to an SMSC gRPC endpoint given a socket address.
    pub async fn connect_addr(addr: std::net::SocketAddr) -> Result<Self, String> {
        Self::connect(format!("http://{addr}")).await
    }

    fn client(&self) -> proto::smsc_service_client::SmscServiceClient<tonic::transport::Channel> {
        self.client.clone()
    }
}

fn timestamp_to_datetime(ts: Option<prost_types::Timestamp>) -> Result<DateTime<Utc>, String> {
    let ts = ts.ok_or_else(|| "missing timestamp".to_string())?;
    DateTime::<Utc>::from_timestamp(ts.seconds, ts.nanos as u32)
        .ok_or_else(|| "invalid timestamp".to_string())
}

fn submission_from_proto(value: proto::SmsSubmission) -> Result<SmsSubmission, String> {
    Ok(SmsSubmission {
        sms_id: Uuid::parse_str(&value.sms_id).map_err(|e| format!("invalid sms_id: {e}"))?,
        originating_number: value.originating_number,
        destination_number: value.destination_number,
        destination_esn: value.destination_esn.map(|v| v as u32),
        destination_imsi: value.destination_imsi,
        originating_subscriber_id: value
            .originating_subscriber_id
            .map(|id| {
                Uuid::parse_str(&id).map_err(|e| format!("invalid originating_subscriber_id: {e}"))
            })
            .transpose()?,
        destination_subscriber_id: value
            .destination_subscriber_id
            .map(|id| {
                Uuid::parse_str(&id).map_err(|e| format!("invalid destination_subscriber_id: {e}"))
            })
            .transpose()?,
        text: value.text,
        state: SmsState::from_str(&value.state)
            .ok_or_else(|| format!("unknown SmsState: {}", value.state))?,
        failure_reason: value.failure_reason,
        created_at: timestamp_to_datetime(value.created_at)?,
        updated_at: timestamp_to_datetime(value.updated_at)?,
    })
}

fn delivery_attempt_from_proto(
    value: proto::SmsDeliveryAttempt,
) -> Result<SmsDeliveryAttempt, String> {
    Ok(SmsDeliveryAttempt {
        sms_delivery_attempt_id: Uuid::parse_str(&value.sms_delivery_attempt_id)
            .map_err(|e| format!("invalid sms_delivery_attempt_id: {e}"))?,
        sms_id: Uuid::parse_str(&value.sms_id).map_err(|e| format!("invalid sms_id: {e}"))?,
        attempt_number: value.attempt_number,
        state: DeliveryAttemptState::from_str(&value.state)
            .ok_or_else(|| format!("unknown DeliveryAttemptState: {}", value.state))?,
        target_subscriber_id: value
            .target_subscriber_id
            .map(|id| {
                Uuid::parse_str(&id).map_err(|e| format!("invalid target_subscriber_id: {e}"))
            })
            .transpose()?,
        failure_reason: value.failure_reason,
        requested_at: timestamp_to_datetime(value.requested_at)?,
        completed_at: value
            .completed_at
            .map(|ts| timestamp_to_datetime(Some(ts)))
            .transpose()?,
        created_at: timestamp_to_datetime(value.created_at)?,
        updated_at: timestamp_to_datetime(value.updated_at)?,
    })
}

fn destination_to_proto(
    destination: SmsDestination,
) -> (Option<String>, Option<u64>, Option<String>) {
    match destination {
        SmsDestination::PhoneNumber(number) => (Some(number), None, None),
        SmsDestination::Esn(esn) => (None, Some(esn as u64), None),
        SmsDestination::Imsi(imsi) => (None, None, Some(imsi)),
    }
}

#[async_trait]
impl SmscRepository for GrpcSmscRepository {
    async fn create_submission(
        &self,
        originating_number: &str,
        destination: SmsDestination,
        text: &str,
        originating_subscriber_id: Option<Uuid>,
        destination_subscriber_id: Option<Uuid>,
    ) -> Result<SmsSubmission, String> {
        let (destination_number, destination_esn, destination_imsi) =
            destination_to_proto(destination);
        let mut client = self.client();
        let response = client
            .create_sms_submission(proto::CreateSmsSubmissionRequest {
                originating_number: originating_number.to_string(),
                destination_number,
                text: text.to_string(),
                originating_subscriber_id: originating_subscriber_id.map(|id| id.to_string()),
                destination_subscriber_id: destination_subscriber_id.map(|id| id.to_string()),
                destination_esn,
                destination_imsi,
            })
            .await
            .map_err(|e| format!("SMSC CreateSmsSubmission: {e}"))?
            .into_inner();
        submission_from_proto(
            response
                .submission
                .ok_or_else(|| "missing submission".to_string())?,
        )
    }

    async fn create_or_get_recent_mo_submission(
        &self,
        originating_number: &str,
        destination_number: &str,
        text: &str,
        originating_subscriber_id: Option<Uuid>,
        destination_subscriber_id: Option<Uuid>,
        fingerprint: &MoSmsFingerprint,
    ) -> Result<(SmsSubmission, bool), String> {
        let mut client = self.client();
        let response = client
            .create_or_get_recent_mo_submission(proto::CreateOrGetRecentMoSubmissionRequest {
                originating_number: originating_number.to_string(),
                destination_number: destination_number.to_string(),
                text: text.to_string(),
                originating_subscriber_id: originating_subscriber_id.map(|id| id.to_string()),
                destination_subscriber_id: destination_subscriber_id.map(|id| id.to_string()),
                fingerprint: Some(proto::MoSmsFingerprint {
                    teleservice_id: u32::from(fingerprint.teleservice_id),
                    message_type: u32::from(fingerprint.message_type),
                    message_id: u32::from(fingerprint.message_id),
                }),
            })
            .await
            .map_err(|e| format!("SMSC CreateOrGetRecentMoSubmission: {e}"))?
            .into_inner();
        Ok((
            submission_from_proto(
                response
                    .submission
                    .ok_or_else(|| "missing submission".to_string())?,
            )?,
            response.created,
        ))
    }

    async fn update_submission_state(
        &self,
        sms_id: Uuid,
        state: SmsState,
        failure_reason: Option<String>,
    ) -> Result<SmsSubmission, String> {
        let mut client = self.client();
        let response = client
            .update_sms_submission_state(proto::UpdateSmsSubmissionStateRequest {
                sms_id: sms_id.to_string(),
                state: state.as_str().to_string(),
                failure_reason,
            })
            .await
            .map_err(|e| format!("SMSC UpdateSmsSubmissionState: {e}"))?
            .into_inner();
        submission_from_proto(
            response
                .submission
                .ok_or_else(|| "missing submission".to_string())?,
        )
    }

    async fn get_submission(&self, sms_id: Uuid) -> Result<Option<SmsSubmission>, String> {
        let mut client = self.client();
        match client
            .get_sms_submission(proto::GetSmsSubmissionRequest {
                sms_id: sms_id.to_string(),
            })
            .await
        {
            Ok(response) => submission_from_proto(
                response
                    .into_inner()
                    .submission
                    .ok_or_else(|| "missing submission".to_string())?,
            )
            .map(Some),
            Err(status) if status.code() == Code::NotFound => Ok(None),
            Err(status) => Err(format!("SMSC GetSmsSubmission: {status}")),
        }
    }

    async fn list_submissions(
        &self,
        limit: u32,
        offset: u32,
        destination_number: Option<&str>,
        destination_esn: Option<u32>,
        destination_imsi: Option<&str>,
        state: Option<&str>,
    ) -> Result<(Vec<SmsSubmission>, u32), String> {
        let mut client = self.client();
        let response = client
            .list_sms_submissions(proto::ListSmsSubmissionsRequest {
                limit: Some(limit),
                offset: Some(offset),
                destination_number: destination_number.map(ToOwned::to_owned),
                state: state.map(ToOwned::to_owned),
                destination_esn: destination_esn.map(u64::from),
                destination_imsi: destination_imsi.map(ToOwned::to_owned),
            })
            .await
            .map_err(|e| format!("SMSC ListSmsSubmissions: {e}"))?
            .into_inner();
        let submissions = response
            .submissions
            .into_iter()
            .map(submission_from_proto)
            .collect::<Result<Vec<_>, _>>()?;
        Ok((submissions, response.total))
    }

    async fn create_delivery_attempt(
        &self,
        sms_id: Uuid,
        target_subscriber_id: Option<Uuid>,
    ) -> Result<SmsDeliveryAttempt, String> {
        let mut client = self.client();
        let response = client
            .create_delivery_attempt(proto::CreateDeliveryAttemptRequest {
                sms_id: sms_id.to_string(),
                target_subscriber_id: target_subscriber_id.map(|id| id.to_string()),
            })
            .await
            .map_err(|e| format!("SMSC CreateDeliveryAttempt: {e}"))?
            .into_inner();
        delivery_attempt_from_proto(
            response
                .attempt
                .ok_or_else(|| "missing attempt".to_string())?,
        )
    }

    async fn update_delivery_attempt_state(
        &self,
        attempt_id: Uuid,
        state: DeliveryAttemptState,
        failure_reason: Option<String>,
    ) -> Result<SmsDeliveryAttempt, String> {
        let mut client = self.client();
        let response = client
            .update_delivery_attempt_state(proto::UpdateDeliveryAttemptStateRequest {
                sms_delivery_attempt_id: attempt_id.to_string(),
                state: state.as_str().to_string(),
                failure_reason,
            })
            .await
            .map_err(|e| format!("SMSC UpdateDeliveryAttemptState: {e}"))?
            .into_inner();
        delivery_attempt_from_proto(
            response
                .attempt
                .ok_or_else(|| "missing attempt".to_string())?,
        )
    }

    async fn get_delivery_attempts(&self, sms_id: Uuid) -> Result<Vec<SmsDeliveryAttempt>, String> {
        let mut client = self.client();
        let response = client
            .get_sms_submission(proto::GetSmsSubmissionRequest {
                sms_id: sms_id.to_string(),
            })
            .await
            .map_err(|e| format!("SMSC GetSmsSubmission: {e}"))?
            .into_inner();
        response
            .delivery_attempts
            .into_iter()
            .map(delivery_attempt_from_proto)
            .collect()
    }

    async fn update_destination_subscriber(
        &self,
        sms_id: Uuid,
        destination_subscriber_id: Uuid,
    ) -> Result<(), String> {
        let mut client = self.client();
        client
            .update_destination_subscriber(proto::UpdateDestinationSubscriberRequest {
                sms_id: sms_id.to_string(),
                destination_subscriber_id: destination_subscriber_id.to_string(),
            })
            .await
            .map_err(|e| format!("SMSC UpdateDestinationSubscriber: {e}"))?;
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// PostgreSQL implementation
// ---------------------------------------------------------------------------

const SUBMISSION_COLUMNS: &str = "sms_id, originating_number, \
    destination_number, destination_esn, destination_imsi, \
    originating_subscriber_id, destination_subscriber_id, text, \
    mo_teleservice_id, mo_message_type, mo_message_id, \
    state, failure_reason, created_at, updated_at";

#[derive(sqlx::FromRow)]
struct SmsSubmissionRow {
    pub sms_id: Uuid,
    pub originating_number: String,
    pub destination_number: Option<String>,
    pub destination_esn: Option<i64>,
    pub destination_imsi: Option<String>,
    pub originating_subscriber_id: Option<Uuid>,
    pub destination_subscriber_id: Option<Uuid>,
    pub text: String,
    #[sqlx(rename = "mo_teleservice_id")]
    pub _mo_teleservice_id: Option<i32>,
    #[sqlx(rename = "mo_message_type")]
    pub _mo_message_type: Option<i32>,
    #[sqlx(rename = "mo_message_id")]
    pub _mo_message_id: Option<i32>,
    pub state: String,
    pub failure_reason: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl SmsSubmissionRow {
    fn try_into_submission(self) -> Result<SmsSubmission, String> {
        let state = SmsState::from_str(&self.state)
            .ok_or_else(|| format!("unknown SmsState: {}", self.state))?;
        Ok(SmsSubmission {
            sms_id: self.sms_id,
            originating_number: self.originating_number,
            destination_number: self.destination_number,
            destination_esn: self.destination_esn.map(|v| v as u32),
            destination_imsi: self.destination_imsi,
            originating_subscriber_id: self.originating_subscriber_id,
            destination_subscriber_id: self.destination_subscriber_id,
            text: self.text,
            state,
            failure_reason: self.failure_reason,
            created_at: self.created_at,
            updated_at: self.updated_at,
        })
    }
}

#[derive(sqlx::FromRow)]
struct SmsDeliveryAttemptRow {
    pub sms_delivery_attempt_id: Uuid,
    pub sms_id: Uuid,
    pub attempt_number: i64,
    pub state: String,
    pub target_subscriber_id: Option<Uuid>,
    pub failure_reason: Option<String>,
    pub requested_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

impl SmsDeliveryAttemptRow {
    fn try_into_attempt(self) -> Result<SmsDeliveryAttempt, String> {
        let state = DeliveryAttemptState::from_str(&self.state)
            .ok_or_else(|| format!("unknown DeliveryAttemptState: {}", self.state))?;
        Ok(SmsDeliveryAttempt {
            sms_delivery_attempt_id: self.sms_delivery_attempt_id,
            sms_id: self.sms_id,
            attempt_number: self.attempt_number as u32,
            state,
            target_subscriber_id: self.target_subscriber_id,
            failure_reason: self.failure_reason,
            requested_at: self.requested_at,
            completed_at: self.completed_at,
            created_at: self.created_at,
            updated_at: self.updated_at,
        })
    }
}

pub struct PostgresSmscRepository {
    pool: PgPool,
}

impl PostgresSmscRepository {
    pub fn new(pool: PgPool) -> Self {
        Self { pool }
    }

    pub async fn connect_from_config(config: &crate::SmscNodeConfig) -> Result<Self, String> {
        let dsn = config
            .postgres_dsn
            .as_deref()
            .ok_or("SMSC postgres_dsn is not configured; set SMSC_POSTGRES_DSN or add postgres_dsn to smsc.json")?;
        let pool = sqlx::postgres::PgPoolOptions::new()
            .after_connect(|conn, _meta| {
                Box::pin(async move {
                    sqlx::Executor::execute(conn, "SET search_path TO smsc, public")
                        .await
                        .map(|_| ())
                })
            })
            .connect(dsn)
            .await
            .map_err(|e| format_postgres_connect_error("SMSC", &e))?;
        let repo = Self::new(pool);
        repo.run_migrations()
            .await
            .map_err(|e| format!("SMSC migrations failed: {e}"))?;
        Ok(repo)
    }

    pub async fn run_migrations(&self) -> Result<(), String> {
        sqlx::query("CREATE SCHEMA IF NOT EXISTS smsc")
            .execute(&self.pool)
            .await
            .map_err(|e| format!("create schema smsc: {e}"))?;
        let mut migrator = sqlx::migrate!("./migrations");
        migrator.set_ignore_missing(true);
        migrator
            .run(&self.pool)
            .await
            .map_err(|e| format!("migration error: {e}"))?;
        Ok(())
    }
}

fn format_postgres_connect_error(component: &str, error: &sqlx::Error) -> String {
    format!(
        "failed to connect to {component} database: {error}; ensure PostgreSQL is running and reachable (default dev database: `docker compose up -d postgres`)"
    )
}

#[async_trait]
impl SmscRepository for PostgresSmscRepository {
    async fn create_submission(
        &self,
        originating_number: &str,
        destination: SmsDestination,
        text: &str,
        originating_subscriber_id: Option<Uuid>,
        destination_subscriber_id: Option<Uuid>,
    ) -> Result<SmsSubmission, String> {
        let (dest_number, dest_esn, dest_imsi): (Option<String>, Option<i64>, Option<String>) =
            match destination {
                SmsDestination::PhoneNumber(n) => (Some(n), None, None),
                SmsDestination::Esn(esn) => (None, Some(esn as i64), None),
                SmsDestination::Imsi(imsi) => (None, None, Some(imsi)),
            };

        let id = Uuid::new_v4();
        let row = sqlx::query_as::<_, SmsSubmissionRow>(&format!(
            r#"
            INSERT INTO sms_submissions (
                sms_id, originating_number,
                destination_number, destination_esn, destination_imsi,
                text, originating_subscriber_id, destination_subscriber_id,
                mo_teleservice_id, mo_message_type, mo_message_id
            )
            VALUES ($1, $2, $3, $4, $5, $6, $7, $8, NULL, NULL, NULL)
            RETURNING {SUBMISSION_COLUMNS}
            "#,
            SUBMISSION_COLUMNS = SUBMISSION_COLUMNS
        ))
        .bind(id)
        .bind(originating_number)
        .bind(dest_number)
        .bind(dest_esn)
        .bind(dest_imsi)
        .bind(text)
        .bind(originating_subscriber_id)
        .bind(destination_subscriber_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| format!("db error: {}", e))?;

        row.try_into_submission()
    }

    async fn create_or_get_recent_mo_submission(
        &self,
        originating_number: &str,
        destination_number: &str,
        text: &str,
        originating_subscriber_id: Option<Uuid>,
        destination_subscriber_id: Option<Uuid>,
        fingerprint: &MoSmsFingerprint,
    ) -> Result<(SmsSubmission, bool), String> {
        let query = format!(
            r#"
            SELECT {cols}
            FROM sms_submissions
            WHERE originating_subscriber_id IS NOT DISTINCT FROM $1
              AND originating_number = $2
              AND destination_number = $3
              AND text = $4
              AND mo_teleservice_id = $5
              AND mo_message_type = $6
              AND mo_message_id = $7
              AND created_at >= NOW() - INTERVAL '{window} minutes'
            ORDER BY created_at DESC
            LIMIT 1
            "#,
            cols = SUBMISSION_COLUMNS,
            window = MO_SMS_DEDUP_WINDOW_MINUTES,
        );
        let existing = sqlx::query_as::<_, SmsSubmissionRow>(&query)
            .bind(originating_subscriber_id)
            .bind(originating_number)
            .bind(destination_number)
            .bind(text)
            .bind(fingerprint.teleservice_id as i32)
            .bind(fingerprint.message_type as i32)
            .bind(fingerprint.message_id as i32)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| format!("db error: {}", e))?;

        if let Some(row) = existing {
            return Ok((row.try_into_submission()?, false));
        }

        let id = Uuid::new_v4();
        let row = sqlx::query_as::<_, SmsSubmissionRow>(&format!(
            r#"
            INSERT INTO sms_submissions (
                sms_id, originating_number,
                destination_number, destination_esn, destination_imsi,
                text, originating_subscriber_id, destination_subscriber_id,
                mo_teleservice_id, mo_message_type, mo_message_id
            )
            VALUES ($1, $2, $3, NULL, NULL, $4, $5, $6, $7, $8, $9)
            RETURNING {SUBMISSION_COLUMNS}
            "#,
            SUBMISSION_COLUMNS = SUBMISSION_COLUMNS
        ))
        .bind(id)
        .bind(originating_number)
        .bind(destination_number)
        .bind(text)
        .bind(originating_subscriber_id)
        .bind(destination_subscriber_id)
        .bind(fingerprint.teleservice_id as i32)
        .bind(fingerprint.message_type as i32)
        .bind(fingerprint.message_id as i32)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| format!("db error: {}", e))?;

        Ok((row.try_into_submission()?, true))
    }

    async fn update_submission_state(
        &self,
        sms_id: Uuid,
        state: SmsState,
        failure_reason: Option<String>,
    ) -> Result<SmsSubmission, String> {
        let row = sqlx::query_as::<_, SmsSubmissionRow>(&format!(
            r#"
            UPDATE sms_submissions
            SET state = $2, failure_reason = $3, updated_at = NOW()
            WHERE sms_id = $1
            RETURNING {SUBMISSION_COLUMNS}
            "#,
            SUBMISSION_COLUMNS = SUBMISSION_COLUMNS
        ))
        .bind(sms_id)
        .bind(state.as_str())
        .bind(failure_reason)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| format!("db error: {}", e))?
        .ok_or_else(|| format!("submission {} not found", sms_id))?;

        row.try_into_submission()
    }

    async fn get_submission(&self, sms_id: Uuid) -> Result<Option<SmsSubmission>, String> {
        let row = sqlx::query_as::<_, SmsSubmissionRow>(&format!(
            r#"
            SELECT {SUBMISSION_COLUMNS}
            FROM sms_submissions
            WHERE sms_id = $1
            "#,
            SUBMISSION_COLUMNS = SUBMISSION_COLUMNS
        ))
        .bind(sms_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| format!("db error: {}", e))?;

        row.map(|r| r.try_into_submission()).transpose()
    }

    async fn list_submissions(
        &self,
        limit: u32,
        offset: u32,
        destination_number: Option<&str>,
        destination_esn: Option<u32>,
        destination_imsi: Option<&str>,
        state: Option<&str>,
    ) -> Result<(Vec<SmsSubmission>, u32), String> {
        let mut count_sql = String::from("SELECT COUNT(*) as cnt FROM sms_submissions WHERE 1=1");
        let mut query_sql = format!(
            "SELECT {SUBMISSION_COLUMNS} FROM sms_submissions WHERE 1=1",
            SUBMISSION_COLUMNS = SUBMISSION_COLUMNS
        );

        let mut param_idx = 1u32;
        let mut conditions = Vec::new();

        if destination_number.is_some() {
            conditions.push(format!(" AND destination_number = ${}", param_idx));
            param_idx += 1;
        }
        if destination_esn.is_some() {
            conditions.push(format!(" AND destination_esn = ${}", param_idx));
            param_idx += 1;
        }
        if destination_imsi.is_some() {
            conditions.push(format!(" AND destination_imsi = ${}", param_idx));
            param_idx += 1;
        }
        if state.is_some() {
            conditions.push(format!(" AND state = ${}", param_idx));
            param_idx += 1;
        }

        for c in &conditions {
            count_sql.push_str(c);
            query_sql.push_str(c);
        }

        query_sql.push_str(&format!(
            " ORDER BY created_at DESC LIMIT ${} OFFSET ${}",
            param_idx,
            param_idx + 1
        ));

        let dest_esn_i64 = destination_esn.map(|v| v as i64);

        let mut count_q = sqlx::query_scalar::<_, i64>(&count_sql);
        if let Some(dn) = destination_number {
            count_q = count_q.bind(dn);
        }
        if let Some(esn) = dest_esn_i64 {
            count_q = count_q.bind(esn);
        }
        if let Some(imsi) = destination_imsi {
            count_q = count_q.bind(imsi);
        }
        if let Some(st) = state {
            count_q = count_q.bind(st);
        }
        let total = count_q
            .fetch_one(&self.pool)
            .await
            .map_err(|e| format!("db error: {}", e))? as u32;

        let mut main_q = sqlx::query_as::<_, SmsSubmissionRow>(&query_sql);
        if let Some(dn) = destination_number {
            main_q = main_q.bind(dn);
        }
        if let Some(esn) = dest_esn_i64 {
            main_q = main_q.bind(esn);
        }
        if let Some(imsi) = destination_imsi {
            main_q = main_q.bind(imsi);
        }
        if let Some(st) = state {
            main_q = main_q.bind(st);
        }
        main_q = main_q.bind(limit as i64).bind(offset as i64);

        let rows = main_q
            .fetch_all(&self.pool)
            .await
            .map_err(|e| format!("db error: {}", e))?;

        let submissions: Vec<SmsSubmission> = rows
            .into_iter()
            .map(|r| r.try_into_submission())
            .collect::<Result<_, _>>()?;
        Ok((submissions, total))
    }

    async fn create_delivery_attempt(
        &self,
        sms_id: Uuid,
        target_subscriber_id: Option<Uuid>,
    ) -> Result<SmsDeliveryAttempt, String> {
        let id = Uuid::new_v4();
        let row = sqlx::query_as::<_, SmsDeliveryAttemptRow>(
            r#"
            INSERT INTO sms_delivery_attempts (sms_delivery_attempt_id, sms_id, target_subscriber_id, attempt_number)
            VALUES ($1, $2, $3, COALESCE((SELECT MAX(attempt_number) FROM sms_delivery_attempts WHERE sms_id = $2), 0) + 1)
            RETURNING sms_delivery_attempt_id, sms_id, attempt_number::bigint as attempt_number, state, target_subscriber_id, failure_reason, requested_at, completed_at, created_at, updated_at
            "#,
        )
        .bind(id)
        .bind(sms_id)
        .bind(target_subscriber_id)
        .fetch_one(&self.pool)
        .await
        .map_err(|e| format!("db error: {}", e))?;

        row.try_into_attempt()
    }

    async fn update_delivery_attempt_state(
        &self,
        attempt_id: Uuid,
        state: DeliveryAttemptState,
        failure_reason: Option<String>,
    ) -> Result<SmsDeliveryAttempt, String> {
        let is_terminal = matches!(
            state,
            DeliveryAttemptState::ForwardSent
                | DeliveryAttemptState::Failed
                | DeliveryAttemptState::Expired
        );

        let row = if is_terminal {
            sqlx::query_as::<_, SmsDeliveryAttemptRow>(
                r#"
                UPDATE sms_delivery_attempts
                SET state = $2, failure_reason = $3, completed_at = NOW(), updated_at = NOW()
                WHERE sms_delivery_attempt_id = $1
                RETURNING sms_delivery_attempt_id, sms_id, attempt_number::bigint as attempt_number, state, target_subscriber_id, failure_reason, requested_at, completed_at, created_at, updated_at
                "#,
            )
            .bind(attempt_id)
            .bind(state.as_str())
            .bind(failure_reason)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| format!("db error: {}", e))?
        } else {
            sqlx::query_as::<_, SmsDeliveryAttemptRow>(
                r#"
                UPDATE sms_delivery_attempts
                SET state = $2, failure_reason = $3, updated_at = NOW()
                WHERE sms_delivery_attempt_id = $1
                RETURNING sms_delivery_attempt_id, sms_id, attempt_number::bigint as attempt_number, state, target_subscriber_id, failure_reason, requested_at, completed_at, created_at, updated_at
                "#,
            )
            .bind(attempt_id)
            .bind(state.as_str())
            .bind(failure_reason)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| format!("db error: {}", e))?
        };

        row.map(|r| r.try_into_attempt())
            .transpose()?
            .ok_or_else(|| format!("delivery attempt {} not found", attempt_id))
    }

    async fn get_delivery_attempts(&self, sms_id: Uuid) -> Result<Vec<SmsDeliveryAttempt>, String> {
        let rows = sqlx::query_as::<_, SmsDeliveryAttemptRow>(
            r#"
            SELECT sms_delivery_attempt_id, sms_id, attempt_number::bigint as attempt_number, state, target_subscriber_id, failure_reason, requested_at, completed_at, created_at, updated_at
            FROM sms_delivery_attempts
            WHERE sms_id = $1
            ORDER BY attempt_number ASC
            "#,
        )
        .bind(sms_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| format!("db error: {}", e))?;

        rows.into_iter().map(|r| r.try_into_attempt()).collect()
    }

    async fn update_destination_subscriber(
        &self,
        sms_id: Uuid,
        destination_subscriber_id: Uuid,
    ) -> Result<(), String> {
        sqlx::query(
            "UPDATE sms_submissions SET destination_subscriber_id = $2, updated_at = NOW() WHERE sms_id = $1",
        )
        .bind(sms_id)
        .bind(destination_subscriber_id)
        .execute(&self.pool)
        .await
        .map_err(|e| format!("db error: {}", e))?;
        Ok(())
    }
}
