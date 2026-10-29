use cdma_hlr::model::{MobileIdentityKey, NumberPlan, NumberType};
use cdma_hlr::repository::{HlrRepository, PostgresHlrRepository};
use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

const IMSI: &str = "310005551239876";
const ESN: u32 = 0x8041_8ec6;
const MEID: &str = "a0000012345678";

async fn verify(pool: PgPool) {
    let repo = PostgresHlrRepository::new(pool.clone());
    repo.run_migrations().await.unwrap();
    let subscriber = repo
        .upsert_subscriber(
            "555110",
            "",
            "active",
            NumberType::NetworkSpecific,
            NumberPlan::IsdnE164,
        )
        .await
        .unwrap();
    repo.upsert_identity(subscriber.subscriber_id, Some(IMSI), Some(ESN), None)
        .await
        .unwrap();
    let partial = MobileIdentityKey::from_parts(Some(IMSI), Some(ESN), None).unwrap();
    let full = MobileIdentityKey::from_parts(Some(IMSI), Some(ESN), Some(MEID)).unwrap();
    assert_eq!(
        repo.resolve_by_identity(&full)
            .await
            .unwrap()
            .unwrap()
            .subscriber
            .subscriber_id,
        subscriber.subscriber_id
    );
    sqlx::query("INSERT INTO registration_bindings (subscriber_id, serving_bs_id, state, imsi, esn) VALUES ($1, 'bsc', 'registered', $2, $3)")
        .bind(subscriber.subscriber_id).bind(IMSI).bind(i64::from(ESN)).execute(&pool).await.unwrap();
    assert!(
        repo.upsert_mobile_seen(&partial, Some(6))
            .await
            .unwrap()
            .is_new
    );
    assert!(
        !repo
            .upsert_mobile_seen(&full, Some(6))
            .await
            .unwrap()
            .is_new
    );
    assert!(
        !repo
            .upsert_mobile_seen(&partial, Some(6))
            .await
            .unwrap()
            .is_new
    );
    assert!(
        repo.resolve_registered_imsi(IMSI, "bsc")
            .await
            .unwrap()
            .is_some()
    );
    let seen: Vec<(Option<String>,)> =
        sqlx::query_as("SELECT meid FROM mobiles_seen WHERE imsi=$1 AND esn=$2")
            .bind(IMSI)
            .bind(i64::from(ESN))
            .fetch_all(&pool)
            .await
            .unwrap();
    assert_eq!(seen, vec![(Some(MEID.to_string()),)]);
    sqlx::query("UPDATE subscriber_identities SET meid=$1 WHERE subscriber_id=$2")
        .bind("a0000099999999")
        .bind(subscriber.subscriber_id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(repo.resolve_by_identity(&full).await.unwrap().is_none());
    assert!(
        repo.resolve_registered_imsi(IMSI, "bsc")
            .await
            .unwrap()
            .is_none()
    );
    let mut binding = repo
        .get_registration_binding(subscriber.subscriber_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(binding.meid.as_deref(), Some(MEID));
    binding.meid = None;
    let binding = repo.upsert_registration_binding(binding).await.unwrap();
    assert_eq!(binding.meid.as_deref(), Some(MEID));
    let mut replacement = binding;
    replacement.esn = Some(ESN + 1);
    replacement.meid = None;
    assert!(
        repo.upsert_registration_binding(replacement)
            .await
            .unwrap()
            .meid
            .is_none()
    );
    let other =
        MobileIdentityKey::from_parts(Some(IMSI), Some(ESN), Some("a0000088888888")).unwrap();
    assert!(
        repo.upsert_mobile_seen(&other, Some(6))
            .await
            .unwrap()
            .is_new
    );
    let count: (i64,) =
        sqlx::query_as("SELECT COUNT(*) FROM mobiles_seen WHERE imsi=$1 AND esn=$2")
            .bind(IMSI)
            .bind(i64::from(ESN))
            .fetch_one(&pool)
            .await
            .unwrap();
    assert_eq!(count.0, 2);
    sqlx::query("UPDATE subscriber_identities SET esn=NULL, meid=$1 WHERE subscriber_id=$2")
        .bind(MEID)
        .bind(subscriber.subscriber_id)
        .execute(&pool)
        .await
        .unwrap();
    assert_eq!(
        repo.resolve_by_identity(&full)
            .await
            .unwrap()
            .unwrap()
            .subscriber
            .subscriber_id,
        subscriber.subscriber_id
    );
    let mut binding = repo
        .get_registration_binding(subscriber.subscriber_id)
        .await
        .unwrap()
        .unwrap();
    binding.esn = Some(ESN);
    binding.meid = Some(MEID.to_string());
    repo.upsert_registration_binding(binding).await.unwrap();
    assert!(
        repo.resolve_registered_imsi(IMSI, "bsc")
            .await
            .unwrap()
            .is_some()
    );
    assert!(repo.resolve_by_identity(&other).await.unwrap().is_none());
    sqlx::query("UPDATE subscriber_identities SET esn=$1 WHERE subscriber_id=$2")
        .bind(i64::from(ESN + 1))
        .bind(subscriber.subscriber_id)
        .execute(&pool)
        .await
        .unwrap();
    assert!(repo.resolve_by_identity(&full).await.unwrap().is_none());
}

#[tokio::test]
#[ignore = "requires HLR_TEST_DSN with permission to create a temporary database"]
async fn meid_discovery_preserves_provisioning_and_seen_identity() {
    let admin_dsn = std::env::var("HLR_TEST_DSN").expect("set HLR_TEST_DSN");
    let admin = PgPoolOptions::new().connect(&admin_dsn).await.unwrap();
    let database = format!("hlr_meid_test_{}", Uuid::new_v4().simple());
    sqlx::query(&format!("CREATE DATABASE {database}"))
        .execute(&admin)
        .await
        .unwrap();
    let (base, _) = admin_dsn.rsplit_once('/').unwrap();
    let pool = PgPoolOptions::new()
        .after_connect(|connection, _| {
            Box::pin(async move {
                sqlx::query("SET search_path TO hlr, public")
                    .execute(connection)
                    .await?;
                Ok(())
            })
        })
        .connect(&format!("{base}/{database}"))
        .await
        .unwrap();
    let result = tokio::spawn(verify(pool.clone())).await;
    pool.close().await;
    sqlx::query(&format!("DROP DATABASE {database} WITH (FORCE)"))
        .execute(&admin)
        .await
        .unwrap();
    admin.close().await;
    result.unwrap();
}
