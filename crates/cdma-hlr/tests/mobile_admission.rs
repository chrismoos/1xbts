use std::sync::Arc;

use cdma_hlr::model::{
    MobileAdmission, MobileAdmissionClass, MobileIdentityKey, MobileSeenFilter, NumberPlan,
    NumberType,
};
use cdma_hlr::repository::{HlrRepository, PostgresHlrRepository};
use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

const DEFAULT_IMSI: &str = "000000000000000";

fn admission(class: MobileAdmissionClass, reason: &str) -> MobileAdmission {
    MobileAdmission {
        class,
        reason: reason.to_string(),
        base_station: "bts-1".to_string(),
    }
}

async fn verify(pool: PgPool) {
    let repo = Arc::new(PostgresHlrRepository::new(pool.clone()));
    repo.run_migrations().await.unwrap();
    let first = MobileIdentityKey::from_parts(Some(DEFAULT_IMSI), Some(0x9F00_0001), None).unwrap();
    let second =
        MobileIdentityKey::from_parts(Some(DEFAULT_IMSI), Some(0x9F00_0002), None).unwrap();

    for _ in 0..2 {
        repo.record_mobile_admission(
            &first,
            &admission(MobileAdmissionClass::Denied, "not_a_subscriber"),
        )
        .await
        .unwrap();
    }
    repo.record_mobile_admission(
        &second,
        &admission(MobileAdmissionClass::Provisioning, "hardware_staged"),
    )
    .await
    .unwrap();

    let (all, total) = repo
        .list_mobiles_seen(MobileSeenFilter::All, 50, 0)
        .await
        .unwrap();
    assert_eq!(total, 2, "two devices sharing an IMSI stay distinct");
    assert_eq!(all.len(), 2);
    let denied = all
        .iter()
        .find(|mobile| mobile.esn == Some(0x9F00_0001))
        .unwrap();
    assert_eq!(denied.admission_class, Some(MobileAdmissionClass::Denied));
    assert_eq!(denied.denied_count, 2);
    assert_eq!(
        denied.last_denial_reason.as_deref(),
        Some("not_a_subscriber")
    );

    let (restricted, total) = repo
        .list_mobiles_seen(MobileSeenFilter::CurrentlyRestricted, 50, 0)
        .await
        .unwrap();
    assert_eq!((restricted.len(), total), (2, 2));

    repo.record_mobile_admission(&first, &admission(MobileAdmissionClass::Admitted, ""))
        .await
        .unwrap();
    let (current, total) = repo
        .list_mobiles_seen(MobileSeenFilter::CurrentlyDenied, 50, 0)
        .await
        .unwrap();
    assert_eq!(
        (current.len(), total),
        (0, 0),
        "admission clears the current denial"
    );
    let (history, _) = repo
        .list_mobiles_seen(MobileSeenFilter::EverDenied, 50, 0)
        .await
        .unwrap();
    assert_eq!(history.len(), 1, "the denial history remains");
    assert_eq!(history[0].denied_count, 2);
    assert_eq!(
        history[0].admission_class,
        Some(MobileAdmissionClass::Admitted)
    );

    let (page, total) = repo
        .list_mobiles_seen(MobileSeenFilter::All, 1, 1)
        .await
        .unwrap();
    assert_eq!((page.len(), total), (1, 2));
}

async fn with_temporary_database<F, Fut>(verify: F)
where
    F: FnOnce(PgPool) -> Fut + Send + 'static,
    Fut: std::future::Future<Output = ()> + Send + 'static,
{
    let admin_dsn = std::env::var("HLR_TEST_DSN").expect("set HLR_TEST_DSN");
    let admin = PgPoolOptions::new().connect(&admin_dsn).await.unwrap();
    let database = format!("hlr_admission_test_{}", Uuid::new_v4().simple());
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

async fn verify_hrpd_hardware(pool: PgPool) {
    const IMEI: &str = "990001242899516";
    const IMEI_MEID_FORM: &str = "99000124289951";
    const PSEUDO_ESN: u32 = 0x8096_23E0;
    let repo = Arc::new(PostgresHlrRepository::new(pool.clone()));
    repo.run_migrations().await.unwrap();
    let subscriber = repo
        .upsert_subscriber(
            "999",
            "BB Z10",
            "active",
            NumberType::Unknown,
            NumberPlan::Unknown,
        )
        .await
        .unwrap();
    let identity = repo
        .upsert_identity(
            subscriber.subscriber_id,
            Some("310006054426442"),
            Some(PSEUDO_ESN),
            None,
        )
        .await
        .unwrap();
    assert!(
        repo.set_identity_imei(identity.subscriber_identity_id, Some("990001242899515"))
            .await
            .is_err(),
        "a bad check digit is refused"
    );
    let identity = repo
        .set_identity_imei(identity.subscriber_identity_id, Some(IMEI))
        .await
        .unwrap();
    assert_eq!(identity.imei.as_deref(), Some(IMEI));

    let resolved = repo
        .resolve_by_hrpd_hardware_identity(None, Some(IMEI_MEID_FORM))
        .await
        .unwrap()
        .expect("the IMEI form matches on HRPD");
    assert_eq!(resolved.subscriber.subscriber_id, subscriber.subscriber_id);
    assert!(
        repo.resolve_by_hardware_identity(None, Some(IMEI_MEID_FORM))
            .await
            .unwrap()
            .is_none(),
        "1x hardware matching ignores the IMEI"
    );

    let other = repo
        .upsert_subscriber(
            "1000",
            "Other",
            "active",
            NumberType::Unknown,
            NumberPlan::Unknown,
        )
        .await
        .unwrap();
    let other_identity = repo
        .upsert_identity(
            other.subscriber_id,
            Some("310550000000001"),
            Some(0x1234_5678),
            None,
        )
        .await
        .unwrap();
    assert!(
        repo.set_identity_imei(other_identity.subscriber_identity_id, Some(IMEI))
            .await
            .is_err(),
        "an IMEI belongs to one identity"
    );

    let refused = MobileAdmission {
        class: MobileAdmissionClass::Denied,
        reason: "not_a_subscriber".to_string(),
        base_station: String::new(),
    };
    for _ in 0..2 {
        repo.record_hardware_admission(None, Some("a0000099999999"), &refused)
            .await
            .unwrap();
    }
    let seen_1x =
        MobileIdentityKey::from_parts(Some(DEFAULT_IMSI), Some(0x4CDC_1D09), None).unwrap();
    repo.upsert_mobile_seen(&seen_1x, None).await.unwrap();
    repo.record_hardware_admission(Some(0x4CDC_1D09), None, &refused)
        .await
        .unwrap();

    let (all, total) = repo
        .list_mobiles_seen(MobileSeenFilter::CurrentlyDenied, 50, 0)
        .await
        .unwrap();
    assert_eq!(
        total, 2,
        "each Hardware ID gets its own row without an IMSI"
    );
    let hardware_only = all
        .iter()
        .find(|mobile| mobile.meid.as_deref() == Some("a0000099999999"))
        .unwrap();
    assert_eq!(hardware_only.imsi, None);
    assert_eq!(hardware_only.denied_count, 2);
    let hrpd_esn = all
        .iter()
        .find(|mobile| mobile.esn == Some(0x4CDC_1D09))
        .unwrap();
    assert_eq!(hrpd_esn.imsi, None, "the 1x sighting is left alone");
    assert_eq!(hrpd_esn.denied_count, 1);
    let (every, _) = repo
        .list_mobiles_seen(MobileSeenFilter::All, 50, 0)
        .await
        .unwrap();
    let seen_on_1x = every
        .iter()
        .find(|mobile| mobile.imsi.as_deref() == Some(DEFAULT_IMSI))
        .unwrap();
    assert_eq!(seen_on_1x.admission_class, None);
}

#[tokio::test]
#[ignore = "requires HLR_TEST_DSN with permission to create a temporary database"]
async fn hrpd_hardware_ids_match_imei_and_record_without_an_imsi() {
    with_temporary_database(verify_hrpd_hardware).await;
}

#[tokio::test]
#[ignore = "requires HLR_TEST_DSN with permission to create a temporary database"]
async fn admission_decisions_track_current_state_and_denial_history() {
    with_temporary_database(verify).await;
}
