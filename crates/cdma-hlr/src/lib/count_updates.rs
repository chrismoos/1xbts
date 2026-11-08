//! Persistent scheduling and reconciliation of C.S0005 Parameter Update Orders.

use std::time::Duration;

use sqlx::PgPool;
use tonic::Status;
use uuid::Uuid;

const RETRY_BACKOFF_SECS: u64 = 60;

fn storage_error(error: sqlx::Error) -> Status {
    log::error!("HLR COUNT storage error: {error}");
    Status::internal("COUNT update storage failed")
}

pub(crate) async fn reserve(
    pool: &PgPool,
    identity: Uuid,
    minimum_interval: Duration,
    confirmation_timeout: Duration,
) -> Result<Option<Uuid>, Status> {
    let interval = i64::try_from(minimum_interval.as_secs())
        .map_err(|_| Status::invalid_argument("COUNT interval too large"))?;
    let timeout = i64::try_from(confirmation_timeout.as_secs())
        .map_err(|_| Status::invalid_argument("COUNT timeout too large"))?;
    let retry = i64::try_from(
        confirmation_timeout
            .as_secs()
            .saturating_add(RETRY_BACKOFF_SECS),
    )
    .map_err(|_| Status::invalid_argument("COUNT retry interval too large"))?;
    let token = Uuid::new_v4();
    sqlx::query_scalar(
        "UPDATE subscriber_identities SET count_pending_id = $2, count_pending_base = auth_count,
            count_pending_until = now() + $4 * interval '1 second',
            count_retry_after = now() + $5 * interval '1 second'
         WHERE subscriber_identity_id = $1 AND count_pending_id IS NULL
            AND (count_updated_at IS NULL OR count_updated_at <= now() - $3 * interval '1 second')
            AND (count_retry_after IS NULL OR count_retry_after <= now())
         RETURNING count_pending_id",
    )
    .bind(identity)
    .bind(token)
    .bind(interval)
    .bind(timeout)
    .bind(retry)
    .fetch_optional(pool)
    .await
    .map_err(storage_error)
}

pub(crate) async fn confirm(pool: &PgPool, identity: Uuid, token: Uuid) -> Result<u8, Status> {
    let count: Option<i16> = sqlx::query_scalar(
        "UPDATE subscriber_identities SET auth_count = (count_pending_base + 1) % $3,
            count_updated_at = now(), count_completed_id = count_pending_id,
            count_pending_id = NULL, count_pending_base = NULL, count_pending_until = NULL,
            count_retry_after = NULL
         WHERE subscriber_identity_id = $1 AND count_pending_id = $2 RETURNING auth_count",
    )
    .bind(identity)
    .bind(token)
    .bind(i16::from(crate::auth::AUTH_COUNT_MODULUS))
    .fetch_optional(pool)
    .await
    .map_err(storage_error)?;
    if let Some(count) = count {
        return Ok(count as u8);
    }
    let completed: Option<i16> = sqlx::query_scalar(
        "SELECT auth_count FROM subscriber_identities
         WHERE subscriber_identity_id = $1 AND count_completed_id = $2",
    )
    .bind(identity)
    .bind(token)
    .fetch_optional(pool)
    .await
    .map_err(storage_error)?;
    completed.map(|count| count as u8).ok_or_else(|| {
        Status::failed_precondition("COUNT confirmation has no matching pending update")
    })
}

/// Only a verified global challenge may reconcile an unconfirmed update.
pub(crate) async fn reconcile(pool: &PgPool, identity: Uuid, reported: u8) -> Result<(), Status> {
    if reported >= crate::auth::AUTH_COUNT_MODULUS {
        return Err(Status::invalid_argument("COUNT exceeds modulo-64 range"));
    }
    sqlx::query(
        "UPDATE subscriber_identities SET auth_count = $2,
            count_updated_at = CASE WHEN count_pending_id IS NOT NULL
                AND $2 = (count_pending_base + 1) % $3 THEN now() ELSE count_updated_at END,
            count_completed_id = CASE WHEN count_pending_id IS NOT NULL
                AND $2 = (count_pending_base + 1) % $3 THEN count_pending_id ELSE count_completed_id END,
            count_pending_id = NULL, count_pending_base = NULL, count_pending_until = NULL
         WHERE subscriber_identity_id = $1
            AND (count_pending_id IS NULL OR count_pending_until <= now())
            AND (auth_count <> $2 OR count_pending_id IS NOT NULL)",
    ).bind(identity).bind(i16::from(reported)).bind(i16::from(crate::auth::AUTH_COUNT_MODULUS))
    .execute(pool).await.map_err(storage_error)?;
    Ok(())
}
