use cdma_cave::{AKey, Esn, RandSsd};
use cdma_hlr::auth::{CaveAuthOutcome, GlobalChallenge, UniqueChallenge};
use cdma_hlr::model::{AuthAlgorithm, MobileIdentityKey, NumberPlan, NumberType};
use cdma_hlr::repository::{HlrRepository, PostgresHlrRepository};
use sqlx::{PgPool, postgres::PgPoolOptions};
use uuid::Uuid;

const IMSI: &str = "310005550001234";
const ESN: u32 = 0xD75A_96EC;
const IMSI_S1: u32 = 0x79_2971;
const IMSI_S2: u16 = 0x1A5;
const A_KEY: u64 = 0xC442_F56B_E9E1_7158;

fn global(ssd_a: &[u8; 8], rand: u32, count: u8) -> GlobalChallenge {
    GlobalChallenge {
        esn: ESN,
        rand,
        authr: cdma_cave::auth_signature(rand, IMSI_S1, ssd_a, Esn(ESN))
            .unwrap()
            .signature,
        count: Some(count),
        auth_data: IMSI_S1,
        generate_call_keys: true,
        otasp_reauthentication: false,
    }
}

async fn verify(pool: PgPool) {
    let repo = PostgresHlrRepository::new(pool.clone());
    repo.run_migrations().await.unwrap();
    let subscriber = repo
        .upsert_subscriber(
            "555200",
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
    let identity = MobileIdentityKey::from_parts(Some(IMSI), Some(ESN), None).unwrap();

    let unprovisioned = repo
        .verify_cave_global_challenge(&identity, &global(&[0; 8], 1, 0))
        .await
        .unwrap();
    assert_eq!(unprovisioned.outcome, CaveAuthOutcome::NotProvisioned);

    repo.set_subscriber_a_key(
        &identity,
        Some(A_KEY.to_be_bytes().to_vec()),
        None,
        None,
        AuthAlgorithm::Cave,
    )
    .await
    .unwrap();

    let zero_ssd = repo
        .verify_cave_global_challenge(&identity, &global(&[0; 8], 0x1111_2222, 0))
        .await
        .unwrap();
    assert_eq!(zero_ssd.outcome, CaveAuthOutcome::Success);
    assert!(zero_ssd.call_keys.is_some());

    let randssd = repo.begin_cave_ssd_update(&identity).await.unwrap();
    let ssd_new = cdma_cave::generate_ssd(
        &AKey::from_u64(A_KEY),
        Esn(ESN),
        &RandSsd::from_u64(randssd).unwrap(),
    );
    let authbs = repo
        .cave_base_station_challenge(&identity, ESN, 0xCAFE_F00D, IMSI_S1)
        .await
        .unwrap();
    assert_eq!(
        authbs,
        cdma_cave::authbs(&ssd_new.ssd_a, 0xCAFE_F00D, IMSI_S1, Esn(ESN)).unwrap()
    );

    let still_zero = repo
        .verify_cave_global_challenge(&identity, &global(&[0; 8], 0x3333_4444, 0))
        .await
        .unwrap();
    assert_eq!(
        still_zero.outcome,
        CaveAuthOutcome::Success,
        "the pending SSD must not replace the stored SSD before confirmation"
    );

    repo.finish_cave_ssd_update(&identity, ESN, true)
        .await
        .unwrap();
    assert!(
        repo.cave_base_station_challenge(&identity, ESN, 1, IMSI_S1)
            .await
            .is_err()
    );
    let with_new = repo
        .verify_cave_global_challenge(&identity, &global(&ssd_new.ssd_a, 0x5555_6666, 0))
        .await
        .unwrap();
    assert_eq!(with_new.outcome, CaveAuthOutcome::Success);

    let authu = cdma_cave::authu(&ssd_new.ssd_a, 0x00AB_CDEF, IMSI_S1, IMSI_S2, Esn(ESN))
        .unwrap()
        .signature;
    let unique = repo
        .verify_cave_unique_challenge(
            &identity,
            &UniqueChallenge {
                esn: ESN,
                randu: 0x00AB_CDEF,
                authu,
                imsi_s1: IMSI_S1,
                imsi_s2: IMSI_S2,
                generate_call_keys: false,
            },
        )
        .await
        .unwrap();
    assert_eq!(unique.outcome, CaveAuthOutcome::Success);
    assert!(unique.call_keys.is_none());

    assert_eq!(repo.increment_cave_count(&identity).await.unwrap(), 1);
    let stale_count = repo
        .verify_cave_global_challenge(&identity, &global(&ssd_new.ssd_a, 0x7777_8888, 0))
        .await
        .unwrap();
    assert_eq!(
        stale_count.outcome,
        CaveAuthOutcome::Success,
        "a COUNT mismatch is adopted, not rejected"
    );
    let state = repo
        .get_subscriber_auth_state(&identity)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        state.auth_count, 0,
        "the mobile's COUNT replaces the stored one"
    );
    assert_eq!(state.auth_failure_count, 0);
    assert!(state.ssd_provisioned);
    assert_eq!(repo.increment_cave_count(&identity).await.unwrap(), 1);

    verify_otasp_key_exchange(&repo, &identity, &ssd_new).await;
    verify_keypad_a_key_entry(repo).await;
}

async fn verify_keypad_a_key_entry(repo: PostgresHlrRepository) {
    use cdma_hlr::proto;
    use cdma_hlr::proto::hlr_service_server::HlrService;
    let service = cdma_hlr::service::HlrServiceImpl::new(std::sync::Arc::new(repo));
    let request = |digits: &str| {
        tonic::Request::new(proto::SetSubscriberAKeyRequest {
            identity: Some(proto::MobileIdentityKey {
                imsi: Some(IMSI.to_string()),
                esn: Some(ESN),
                meid: None,
            }),
            a_key: None,
            ssd_a: None,
            ssd_b: None,
            auth_algo: proto::AuthAlgorithm::Unspecified as i32,
            a_key_digits: Some(digits.to_string()),
        })
    };
    // S.S0053-0 §3.1 entry for ESN D75A96EC.
    service
        .set_subscriber_a_key(request("14142135623730950488086500"))
        .await
        .unwrap();
    let status = service
        .set_subscriber_a_key(request("14142135623730950488086501"))
        .await
        .unwrap_err();
    assert_eq!(status.code(), tonic::Code::InvalidArgument);
    let state = service
        .get_subscriber_auth_state(tonic::Request::new(proto::GetSubscriberAuthStateRequest {
            identity: Some(proto::MobileIdentityKey {
                imsi: Some(IMSI.to_string()),
                esn: Some(ESN),
                meid: None,
            }),
        }))
        .await
        .unwrap()
        .into_inner();
    assert!(state.credentials_provisioned);
    assert!(!state.ssd_provisioned, "keypad entry clears the SSD");
}

/// OTASP A-key exchange, SSD update with the exchanged key, re-authentication and commit
/// (C.S0016-D §3.3.2).
async fn verify_otasp_key_exchange(
    repo: &PostgresHlrRepository,
    identity: &MobileIdentityKey,
    previous_ssd: &cdma_cave::Ssd,
) {
    use cdma_hlr::key_exchange::{A_KEY_P_REV_2G, mobile_station_key_exchange};
    let offer = repo
        .begin_cave_key_exchange(identity, A_KEY_P_REV_2G)
        .await
        .unwrap();
    let (a_key_temp, ms_result) = mobile_station_key_exchange(&offer).unwrap();
    repo.complete_cave_key_exchange(identity, &ms_result)
        .await
        .unwrap();

    let randssd = repo.begin_cave_ssd_update(identity).await.unwrap();
    let session_ssd = cdma_cave::generate_ssd(
        &AKey(a_key_temp),
        Esn(ESN),
        &RandSsd::from_u64(randssd).unwrap(),
    );
    assert_eq!(
        repo.cave_base_station_challenge(identity, ESN, 0x0BAD_F00D, IMSI_S1)
            .await
            .unwrap(),
        cdma_cave::authbs(&session_ssd.ssd_a, 0x0BAD_F00D, IMSI_S1, Esn(ESN)).unwrap()
    );
    repo.finish_cave_ssd_update(identity, ESN, true)
        .await
        .unwrap();

    let mut reauth = global(&session_ssd.ssd_a, 0x2468_ACE0, 1);
    reauth.otasp_reauthentication = true;
    assert_eq!(
        repo.verify_cave_global_challenge(identity, &reauth)
            .await
            .unwrap()
            .outcome,
        CaveAuthOutcome::Success
    );
    assert_eq!(
        repo.verify_cave_global_challenge(identity, &global(&previous_ssd.ssd_a, 0x1357_9BDF, 1))
            .await
            .unwrap()
            .outcome,
        CaveAuthOutcome::Success,
        "the committed SSD stays in use until the mobile commits"
    );

    repo.commit_cave_key(identity).await.unwrap();
    assert_eq!(
        repo.verify_cave_global_challenge(identity, &global(&session_ssd.ssd_a, 0x1122_3344, 1))
            .await
            .unwrap()
            .outcome,
        CaveAuthOutcome::Success
    );
    assert!(repo.commit_cave_key(identity).await.is_err());

    repo.begin_cave_key_exchange(identity, 3).await.unwrap();
    repo.abort_cave_key_exchange(identity).await.unwrap();
    assert!(
        repo.complete_cave_key_exchange(identity, &[2; 128])
            .await
            .is_err()
    );
}

#[tokio::test]
#[ignore = "requires HLR_TEST_DSN with permission to create a temporary database"]
async fn cave_authentication_center_round_trip() {
    let admin_dsn = std::env::var("HLR_TEST_DSN").expect("set HLR_TEST_DSN");
    let admin = PgPoolOptions::new().connect(&admin_dsn).await.unwrap();
    let database = format!("hlr_cave_test_{}", Uuid::new_v4().simple());
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
