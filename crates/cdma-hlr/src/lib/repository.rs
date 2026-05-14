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
    ) -> Result<Option<Subscriber>, String>;

    async fn get_subscriber_by_id(&self, subscriber_id: Uuid)
    -> Result<Option<Subscriber>, String>;

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
    ) -> Result<SubscriberIdentity, String>;

    async fn replace_primary_identity(
        &self,
        subscriber_id: Uuid,
        imsi: Option<&str>,
        esn: Option<u32>,
    ) -> Result<SubscriberIdentity, String>;

    async fn get_identities_for_subscriber(
        &self,
        subscriber_id: Uuid,
    ) -> Result<Vec<SubscriberIdentity>, String>;

    async fn resolve_by_identity(
        &self,
        esn: Option<u32>,
        imsi: Option<&str>,
    ) -> Result<Option<Subscriber>, String>;

    // Mobile sightings
    async fn upsert_mobile_seen(
        &self,
        esn: Option<u32>,
        imsi: Option<&str>,
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
        status: SubscriberStatus::from_str(&value.status)?,
        created_at: timestamp_to_datetime(value.created_at)?,
        updated_at: timestamp_to_datetime(value.updated_at)?,
        number_type: number_type_from_proto(value.number_type),
        number_plan: number_plan_from_proto(value.number_plan),
    })
}

fn identity_from_proto(value: proto::SubscriberIdentity) -> Result<SubscriberIdentity, String> {
    Ok(SubscriberIdentity {
        subscriber_identity_id: Uuid::parse_str(&value.subscriber_identity_id)
            .map_err(|e| format!("invalid subscriber_identity_id: {e}"))?,
        subscriber_id: Uuid::parse_str(&value.subscriber_id)
            .map_err(|e| format!("invalid subscriber_id: {e}"))?,
        imsi: value.imsi,
        esn: value.esn,
        is_primary: value.is_primary,
        created_at: timestamp_to_datetime(value.created_at)?,
    })
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
        let mut client = self.client();
        let response = client
            .upsert_subscriber(proto::UpsertSubscriberRequest {
                phone_number: phone_number.to_string(),
                display_name: display_name.to_string(),
                status: status.to_string(),
                imsi: None,
                esn: None,
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
    ) -> Result<Option<Subscriber>, String> {
        let mut client = self.client();
        match client
            .get_subscriber_by_phone_number(proto::GetSubscriberByPhoneNumberRequest {
                phone_number: phone_number.to_string(),
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
            Err(status) => Err(format!("HLR GetSubscriberByPhoneNumber: {status}")),
        }
    }

    async fn get_subscriber_by_id(
        &self,
        subscriber_id: Uuid,
    ) -> Result<Option<Subscriber>, String> {
        let mut client = self.client();
        match client
            .get_subscriber(proto::GetSubscriberRequest {
                subscriber_id: subscriber_id.to_string(),
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
        let mut client = self.client();
        match client
            .update_subscriber(proto::UpdateSubscriberRequest {
                subscriber_id: subscriber_id.to_string(),
                phone_number: phone_number.to_string(),
                display_name: display_name.to_string(),
                status: status.to_string(),
                imsi: None,
                esn: None,
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
    ) -> Result<SubscriberIdentity, String> {
        let mut client = self.client();
        let response = client
            .upsert_identity(proto::UpsertIdentityRequest {
                subscriber_id: subscriber_id.to_string(),
                imsi: imsi.map(ToOwned::to_owned),
                esn,
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
    ) -> Result<SubscriberIdentity, String> {
        let mut client = self.client();
        let response = client
            .replace_primary_identity(proto::ReplacePrimaryIdentityRequest {
                subscriber_id: subscriber_id.to_string(),
                imsi: imsi.map(ToOwned::to_owned),
                esn,
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
        esn: Option<u32>,
        imsi: Option<&str>,
    ) -> Result<Option<Subscriber>, String> {
        let mut client = self.client();
        let response = client
            .resolve_subscriber_by_identity(proto::ResolveSubscriberByIdentityRequest {
                esn,
                imsi: imsi.map(ToOwned::to_owned),
                imsi_m_s1: None,
                imsi_m_s2: None,
            })
            .await
            .map_err(|e| format!("HLR ResolveSubscriberByIdentity: {e}"))?
            .into_inner();
        response.subscriber.map(subscriber_from_proto).transpose()
    }

    async fn upsert_mobile_seen(
        &self,
        esn: Option<u32>,
        imsi: Option<&str>,
        mob_p_rev: Option<u8>,
    ) -> Result<MobileSeenUpsert, String> {
        let mut client = self.client();
        let response = client
            .upsert_mobile_seen(proto::UpsertMobileSeenRequest {
                esn,
                imsi: imsi.map(ToOwned::to_owned),
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

    pub async fn run_migrations(&self) -> Result<(), String> {
        sqlx::query("CREATE SCHEMA IF NOT EXISTS hlr")
            .execute(&self.pool)
            .await
            .map_err(|e| format!("create schema hlr: {e}"))?;
        sqlx::migrate!("./migrations")
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
                created_at, updated_at, number_type, number_plan
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
    ) -> Result<Option<Subscriber>, String> {
        let row = sqlx::query_as::<_, SubscriberRow>(
            "SELECT subscriber_id, phone_number, display_name, status, created_at, updated_at, number_type, number_plan FROM subscribers WHERE phone_number = $1",
        )
        .bind(phone_number)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| format!("get_subscriber_by_phone_number: {e}"))?;
        row.map(TryInto::try_into).transpose()
    }

    async fn get_subscriber_by_id(
        &self,
        subscriber_id: Uuid,
    ) -> Result<Option<Subscriber>, String> {
        let row = sqlx::query_as::<_, SubscriberRow>(
            "SELECT subscriber_id, phone_number, display_name, status, created_at, updated_at, number_type, number_plan FROM subscribers WHERE subscriber_id = $1",
        )
        .bind(subscriber_id)
        .fetch_optional(&self.pool)
        .await
        .map_err(|e| format!("get_subscriber_by_id: {e}"))?;
        row.map(TryInto::try_into).transpose()
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
            UPDATE subscribers
            SET phone_number = $2,
                display_name = $3,
                status = $4,
                updated_at = $5,
                number_type = $6,
                number_plan = $7
            WHERE subscriber_id = $1
            RETURNING subscriber_id, phone_number, display_name, status,
                created_at, updated_at, number_type, number_plan
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
            "SELECT subscriber_id, phone_number, display_name, status, created_at, updated_at, number_type, number_plan FROM subscribers ORDER BY created_at DESC LIMIT $1 OFFSET $2",
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
    ) -> Result<SubscriberIdentity, String> {
        let now = Utc::now();
        let id = Uuid::new_v4();
        let mut tx = self
            .pool
            .begin()
            .await
            .map_err(|e| format!("upsert_identity begin: {e}"))?;

        let existing = sqlx::query_as::<_, IdentityRow>(
            "SELECT subscriber_identity_id, subscriber_id, imsi, esn, is_primary, created_at FROM subscriber_identities WHERE subscriber_id = $1 LIMIT 1",
        )
        .bind(subscriber_id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| format!("upsert_identity lookup: {e}"))?;

        let row = if let Some(existing) = existing {
            sqlx::query_as::<_, IdentityRow>(
                r#"
                UPDATE subscriber_identities SET
                    imsi = COALESCE($2, imsi),
                    esn = COALESCE($3, esn)
                WHERE subscriber_identity_id = $1
                RETURNING subscriber_identity_id, subscriber_id, imsi, esn, is_primary, created_at
                "#,
            )
            .bind(existing.subscriber_identity_id)
            .bind(imsi)
            .bind(esn.map(|v| v as i64))
            .fetch_one(&mut *tx)
            .await
            .map_err(|e| format!("upsert_identity update: {e}"))?
        } else {
            let is_primary = true;
            sqlx::query_as::<_, IdentityRow>(
                r#"
                INSERT INTO subscriber_identities (subscriber_identity_id, subscriber_id, imsi, esn, is_primary, created_at)
                VALUES ($1, $2, $3, $4, $5, $6)
                RETURNING subscriber_identity_id, subscriber_id, imsi, esn, is_primary, created_at
                "#,
            )
            .bind(id)
            .bind(subscriber_id)
            .bind(imsi)
            .bind(esn.map(|v| v as i64))
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
    ) -> Result<SubscriberIdentity, String> {
        let now = Utc::now();
        let id = Uuid::new_v4();
        let existing = sqlx::query_as::<_, IdentityRow>(
            r#"
            SELECT subscriber_identity_id, subscriber_id, imsi, esn, is_primary, created_at
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
                    is_primary = true
                WHERE subscriber_identity_id = $1
                RETURNING subscriber_identity_id, subscriber_id, imsi, esn, is_primary, created_at
                "#,
            )
            .bind(existing.subscriber_identity_id)
            .bind(imsi)
            .bind(esn.map(|v| v as i64))
            .fetch_one(&self.pool)
            .await
            .map_err(|e| format!("replace_primary_identity update: {e}"))?;
            Ok(row.into())
        } else {
            let row = sqlx::query_as::<_, IdentityRow>(
                r#"
                INSERT INTO subscriber_identities (
                    subscriber_identity_id, subscriber_id, imsi, esn, is_primary, created_at
                )
                VALUES ($1, $2, $3, $4, true, $5)
                RETURNING subscriber_identity_id, subscriber_id, imsi, esn, is_primary, created_at
                "#,
            )
            .bind(id)
            .bind(subscriber_id)
            .bind(imsi)
            .bind(esn.map(|v| v as i64))
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
            "SELECT subscriber_identity_id, subscriber_id, imsi, esn, is_primary, created_at FROM subscriber_identities WHERE subscriber_id = $1 ORDER BY is_primary DESC, created_at ASC",
        )
        .bind(subscriber_id)
        .fetch_all(&self.pool)
        .await
        .map_err(|e| format!("get_identities: {e}"))?;
        Ok(rows.into_iter().map(Into::into).collect())
    }

    async fn resolve_by_identity(
        &self,
        esn: Option<u32>,
        imsi: Option<&str>,
    ) -> Result<Option<Subscriber>, String> {
        if let Some(esn_val) = esn {
            let row = sqlx::query_as::<_, SubscriberRow>(
                r#"
                SELECT s.subscriber_id, s.phone_number, s.display_name, s.status, s.created_at, s.updated_at, s.number_type, s.number_plan
                FROM subscribers s
                JOIN subscriber_identities i ON s.subscriber_id = i.subscriber_id
                WHERE i.esn = $1
                LIMIT 1
                "#,
            )
            .bind(esn_val as i64)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| format!("resolve_by_identity esn: {e}"))?;
            if row.is_some() {
                return row.map(TryInto::try_into).transpose();
            }
        }
        if let Some(imsi_val) = imsi {
            let row = sqlx::query_as::<_, SubscriberRow>(
                r#"
                SELECT s.subscriber_id, s.phone_number, s.display_name, s.status, s.created_at, s.updated_at, s.number_type, s.number_plan
                FROM subscribers s
                JOIN subscriber_identities i ON s.subscriber_id = i.subscriber_id
                WHERE i.imsi = $1
                LIMIT 1
                "#,
            )
            .bind(imsi_val)
            .fetch_optional(&self.pool)
            .await
            .map_err(|e| format!("resolve_by_identity imsi: {e}"))?;
            if row.is_some() {
                return row.map(TryInto::try_into).transpose();
            }
        }
        Ok(None)
    }

    async fn upsert_mobile_seen(
        &self,
        esn: Option<u32>,
        imsi: Option<&str>,
        mob_p_rev: Option<u8>,
    ) -> Result<MobileSeenUpsert, String> {
        if esn.is_none() && imsi.is_none() {
            return Err("upsert_mobile_seen: need ESN or IMSI".to_string());
        }

        let row = if let Some(esn_val) = esn {
            sqlx::query_as::<_, MobileSeenRow>(
                r#"
                WITH prev AS (
                    SELECT last_seen_at FROM mobiles_seen WHERE esn = $1
                )
                INSERT INTO mobiles_seen (esn, imsi, mob_p_rev)
                VALUES ($1, $2, $3)
                ON CONFLICT (esn) WHERE esn IS NOT NULL DO UPDATE SET
                    imsi = COALESCE(EXCLUDED.imsi, mobiles_seen.imsi),
                    mob_p_rev = COALESCE(EXCLUDED.mob_p_rev, mobiles_seen.mob_p_rev),
                    last_seen_at = NOW()
                RETURNING
                    (SELECT last_seen_at FROM prev) AS previous_last_seen_at,
                    (xmax = 0) AS is_new
                "#,
            )
            .bind(esn_val as i64)
            .bind(imsi)
            .bind(mob_p_rev.map(|v| v as i32))
            .fetch_one(&self.pool)
            .await
            .map_err(|e| format!("upsert_mobile_seen (esn): {e}"))?
        } else {
            sqlx::query_as::<_, MobileSeenRow>(
                r#"
                WITH prev AS (
                    SELECT last_seen_at FROM mobiles_seen WHERE imsi = $1
                )
                INSERT INTO mobiles_seen (imsi, mob_p_rev)
                VALUES ($1, $2)
                ON CONFLICT (imsi) WHERE imsi IS NOT NULL DO UPDATE SET
                    mob_p_rev = COALESCE(EXCLUDED.mob_p_rev, mobiles_seen.mob_p_rev),
                    last_seen_at = NOW()
                RETURNING
                    (SELECT last_seen_at FROM prev) AS previous_last_seen_at,
                    (xmax = 0) AS is_new
                "#,
            )
            .bind(imsi)
            .bind(mob_p_rev.map(|v| v as i32))
            .fetch_one(&self.pool)
            .await
            .map_err(|e| format!("upsert_mobile_seen (imsi): {e}"))?
        };

        Ok(MobileSeenUpsert {
            is_new: row.is_new,
            previous_last_seen_at: row.previous_last_seen_at,
        })
    }

    async fn upsert_registration_binding(
        &self,
        binding: RegistrationBinding,
    ) -> Result<RegistrationBinding, String> {
        let row = sqlx::query_as::<_, BindingRow>(
            r#"
            INSERT INTO registration_bindings (
                subscriber_id, serving_node_id, state, imsi, esn,
                mob_p_rev, pgslot, slot_cycle_index, last_msg_seq,
                last_registered_at, last_seen_at, updated_at
            ) VALUES ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10, $11, $12)
            ON CONFLICT (subscriber_id) DO UPDATE SET
                serving_node_id = EXCLUDED.serving_node_id,
                state = EXCLUDED.state,
                imsi = EXCLUDED.imsi,
                esn = EXCLUDED.esn,
                mob_p_rev = EXCLUDED.mob_p_rev,
                pgslot = EXCLUDED.pgslot,
                slot_cycle_index = EXCLUDED.slot_cycle_index,
                last_msg_seq = EXCLUDED.last_msg_seq,
                last_registered_at = EXCLUDED.last_registered_at,
                last_seen_at = EXCLUDED.last_seen_at,
                updated_at = EXCLUDED.updated_at
            RETURNING subscriber_id, serving_node_id, state, imsi, esn,
                mob_p_rev, pgslot, slot_cycle_index, last_msg_seq,
                last_registered_at, last_seen_at, updated_at
            "#,
        )
        .bind(binding.subscriber_id)
        .bind(&binding.serving_node_id)
        .bind(binding.state.as_str())
        .bind(binding.imsi.as_deref())
        .bind(binding.esn.map(|v| v as i64))
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
                mob_p_rev, pgslot, slot_cycle_index, last_msg_seq,
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
        })
    }
}

#[derive(sqlx::FromRow)]
struct IdentityRow {
    subscriber_identity_id: Uuid,
    subscriber_id: Uuid,
    imsi: Option<String>,
    esn: Option<i64>,
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
            is_primary: r.is_primary,
            created_at: r.created_at,
        }
    }
}

#[derive(sqlx::FromRow)]
struct MobileSeenRow {
    previous_last_seen_at: Option<DateTime<Utc>>,
    is_new: bool,
}

#[derive(sqlx::FromRow)]
struct BindingRow {
    subscriber_id: Uuid,
    serving_node_id: String,
    state: String,
    imsi: Option<String>,
    esn: Option<i64>,
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
