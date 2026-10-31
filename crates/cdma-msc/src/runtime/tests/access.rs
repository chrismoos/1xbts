use super::*;

use crate::admission::{AccessConfig, AccessMode, RegisterPolicy};
use cdma_hlr::model::{MobileAdmissionClass, SubscriberStatus};

const REPLY_WAIT: Duration = Duration::from_millis(200);
const GENUINE_ESN: u32 = 0x9F12_3456;
const CONFIGURED_IMSI: &str = "310005550000007";
const DEFAULT_IMSI: &str = "000000000000000";

fn subscriber(
    imsi: &str,
    esn: Option<u32>,
    meid: Option<&str>,
    status: SubscriberStatus,
) -> cdma_hlr::model::ResolvedSubscriber {
    let subscriber_id = uuid::Uuid::new_v4();
    let identity = cdma_hlr::model::SubscriberIdentity {
        subscriber_identity_id: uuid::Uuid::new_v4(),
        subscriber_id,
        imsi: Some(imsi.to_string()),
        esn,
        meid: meid.map(str::to_string),
        imei: None,
        is_primary: true,
        created_at: chrono::Utc::now(),
    };
    cdma_hlr::model::ResolvedSubscriber {
        subscriber: cdma_hlr::model::Subscriber {
            subscriber_id,
            phone_number: "5550000007".to_string(),
            display_name: String::new(),
            status,
            created_at: chrono::Utc::now(),
            updated_at: chrono::Utc::now(),
            number_type: cdma_hlr::model::NumberType::default(),
            number_plan: cdma_hlr::model::NumberPlan::default(),
            has_ringtone: false,
            ringtone_duration_ms: None,
            prl_override_id: None,
            service_programming_code: None,
            firstchp_override: None,
        },
        identities: vec![identity.clone()],
        primary_identity: Some(identity),
        binding: None,
    }
}

fn closed_runtime(
    mode: crate::authentication::AuthenticationMode,
    policy: RegisterPolicy,
    hlr: Arc<StubHlrRepo>,
) -> MscRuntime {
    let mut runtime = authenticating_runtime_with_hlr(mode, hlr);
    runtime.config.access = AccessConfig {
        mode: AccessMode::Closed,
        register_policy: policy,
    };
    runtime
}

fn registration(imsi: Option<&str>, esn: Option<u32>) -> EncodedA1Message {
    let request = LocationUpdatingRequestMessage {
        mobile_identity_imsi: MobileIdentity::Imsi(imsi.unwrap_or(DEFAULT_IMSI).to_string()),
        location_area_identification: None,
        classmark_information_type_2: None,
        registration_type: Some(RegistrationType(1)),
        mobile_identity_esn: esn.map(MobileIdentity::Esn),
        mobile_identity_meid: None,
        slot_cycle_index: Some(SlotCycleIndex(2)),
        authentication_response_parameter: None,
        authentication_confirmation_parameter: None,
        authentication_parameter_count: None,
        authentication_challenge_parameter: None,
        authentication_event: None,
        user_zone_id: None,
        is2000_mobile_capabilities: None,
    };
    let cli3 = CompleteLayer3InformationMessage {
        cell_identifier: cdma_ios::CellId {
            cell: 0x123,
            sector: 0,
        },
        layer3_information: cdma_ios::Layer3Information::from_location_updating_request(&request)
            .unwrap(),
    };
    EncodedA1Message::from_message_for_call(
        &cdma_ios::Message::new(
            cdma_ios::MessageType::CompleteLayer3Information,
            cli3.encode().unwrap(),
        ),
        Some(91),
    )
}

async fn register(
    runtime: &mut MscRuntime,
    message: EncodedA1Message,
) -> (cdma_ios::MessageType, Option<u8>) {
    let (client, endpoint) = cdma_bsc_a1_edge_compat::InProcessMscClient::pair(8);
    runtime
        .handle_bsc_a1_message(&endpoint, &test_node(), message)
        .await;
    runtime.flush_admission_history().await;
    let reply = timeout(REPLY_WAIT, client.poll_a1())
        .await
        .expect("a Location Updating result should be sent")
        .unwrap();
    let cause =
        (reply.message_type() == cdma_ios::MessageType::LocationUpdatingReject).then(|| {
            cdma_ios::LocationUpdatingRejectMessage::decode(&reply.decode().unwrap().payload)
                .unwrap()
                .reject_cause
                .0
        });
    (reply.message_type(), cause)
}

fn admission_classes(
    events: &[crate::grpc::events_proto::v1::MscNetworkEvent],
) -> Vec<(i32, bool)> {
    events
        .iter()
        .filter_map(|event| match &event.event {
            Some(crate::grpc::events_proto::v1::msc_network_event::Event::Admission(admission)) => {
                Some((admission.admission_class, admission.service_rejected))
            }
            _ => None,
        })
        .collect()
}

fn drain(
    events: &mut tokio::sync::broadcast::Receiver<crate::grpc::events_proto::v1::MscNetworkEvent>,
) -> Vec<crate::grpc::events_proto::v1::MscNetworkEvent> {
    let mut seen = Vec::new();
    while let Ok(event) = events.try_recv() {
        seen.push(event);
    }
    seen
}

#[tokio::test]
async fn open_mode_admits_an_unknown_mobile() {
    let hlr = Arc::new(StubHlrRepo::default());
    let mut runtime = authenticating_runtime_with_hlr(
        crate::authentication::AuthenticationMode::Off,
        hlr.clone(),
    );
    let (reply, _) = register(
        &mut runtime,
        registration(Some(AUTH_TEST_IMSI), Some(GENUINE_ESN)),
    )
    .await;
    assert_eq!(reply, cdma_ios::MessageType::LocationUpdatingAccept);
    assert!(hlr.admissions.lock().unwrap().is_empty());
}

#[tokio::test]
async fn closed_mode_rejects_an_unknown_mobile_and_records_the_denial() {
    let hlr = Arc::new(StubHlrRepo::default());
    let mut runtime = closed_runtime(
        crate::authentication::AuthenticationMode::Off,
        RegisterPolicy::SubscriberOnly,
        hlr.clone(),
    );
    let mut events = runtime.otasp_event_tx.subscribe();
    let (reply, cause) = register(
        &mut runtime,
        registration(Some(AUTH_TEST_IMSI), Some(GENUINE_ESN)),
    )
    .await;
    assert_eq!(reply, cdma_ios::MessageType::LocationUpdatingReject);
    assert_eq!(cause, Some(cdma_ios::REJECT_CAUSE_IMSI_UNKNOWN_IN_HLR));
    assert_eq!(
        admission_classes(&drain(&mut events)),
        vec![(
            crate::grpc::events_proto::v1::AdmissionClass::Denied as i32,
            true
        )]
    );
    let admissions = hlr.admissions.lock().unwrap().clone();
    assert_eq!(admissions.len(), 1, "a failed history write still rejects");
    assert_eq!(admissions[0].class, MobileAdmissionClass::Denied);
    assert_eq!(admissions[0].reason, "not_a_subscriber");
    assert_eq!(
        *hlr.seen.lock().unwrap(),
        0,
        "a denied mobile gets no sighting binding"
    );
}

#[tokio::test]
async fn closed_mode_classifies_subscribers_by_status() {
    for (status, expected, cause) in [
        (
            SubscriberStatus::Active,
            cdma_ios::MessageType::LocationUpdatingAccept,
            None,
        ),
        (
            SubscriberStatus::Suspended,
            cdma_ios::MessageType::LocationUpdatingAccept,
            None,
        ),
        (
            SubscriberStatus::Disabled,
            cdma_ios::MessageType::LocationUpdatingReject,
            Some(cdma_ios::REJECT_CAUSE_ILLEGAL_MS),
        ),
    ] {
        for policy in [RegisterPolicy::SubscriberOnly, RegisterPolicy::Any] {
            let hlr = Arc::new(StubHlrRepo::default());
            hlr.subscribers.lock().unwrap().push(subscriber(
                AUTH_TEST_IMSI,
                Some(GENUINE_ESN),
                None,
                status.clone(),
            ));
            let mut runtime =
                closed_runtime(crate::authentication::AuthenticationMode::Off, policy, hlr);
            let (reply, reject_cause) = register(
                &mut runtime,
                registration(Some(AUTH_TEST_IMSI), Some(GENUINE_ESN)),
            )
            .await;
            assert_eq!(reply, expected, "{status:?} {policy:?}");
            assert_eq!(reject_cause, cause, "{status:?} {policy:?}");
        }
    }
}

#[tokio::test]
async fn register_policy_any_accepts_an_unknown_mobile_for_registration_only() {
    let hlr = Arc::new(StubHlrRepo::default());
    let mut runtime = closed_runtime(
        crate::authentication::AuthenticationMode::Off,
        RegisterPolicy::Any,
        hlr.clone(),
    );
    let (reply, _) = register(
        &mut runtime,
        registration(Some(AUTH_TEST_IMSI), Some(GENUINE_ESN)),
    )
    .await;
    assert_eq!(reply, cdma_ios::MessageType::LocationUpdatingAccept);
    assert_eq!(
        hlr.admissions.lock().unwrap()[0].class,
        MobileAdmissionClass::RegistrationOnly
    );
    assert_eq!(
        *hlr.seen.lock().unwrap(),
        0,
        "a restricted registration creates no service binding"
    );
}

#[tokio::test]
async fn staged_hardware_registers_for_provisioning_with_another_imsi() {
    let hlr = Arc::new(StubHlrRepo::default());
    hlr.subscribers.lock().unwrap().push(subscriber(
        CONFIGURED_IMSI,
        Some(GENUINE_ESN),
        None,
        SubscriberStatus::Active,
    ));
    let mut runtime = closed_runtime(
        crate::authentication::AuthenticationMode::Enforce,
        RegisterPolicy::SubscriberOnly,
        hlr.clone(),
    );
    let (reply, _) = register(
        &mut runtime,
        registration(Some(DEFAULT_IMSI), Some(GENUINE_ESN)),
    )
    .await;
    assert_eq!(reply, cdma_ios::MessageType::LocationUpdatingAccept);
    assert_eq!(
        hlr.admissions.lock().unwrap()[0].class,
        MobileAdmissionClass::Provisioning
    );
}

#[tokio::test]
async fn pseudo_esn_ambiguous_and_failed_lookups_never_fall_back() {
    let pseudo_esn = 0x80AB_CDEF;
    let hlr = Arc::new(StubHlrRepo::default());
    hlr.subscribers.lock().unwrap().push(subscriber(
        CONFIGURED_IMSI,
        Some(pseudo_esn),
        None,
        SubscriberStatus::Active,
    ));
    let mut runtime = closed_runtime(
        crate::authentication::AuthenticationMode::Off,
        RegisterPolicy::SubscriberOnly,
        hlr.clone(),
    );
    let (reply, _) = register(
        &mut runtime,
        registration(Some(DEFAULT_IMSI), Some(pseudo_esn)),
    )
    .await;
    assert_eq!(reply, cdma_ios::MessageType::LocationUpdatingReject);

    let hlr = Arc::new(StubHlrRepo::default());
    for imsi in [CONFIGURED_IMSI, "310005550000008"] {
        hlr.subscribers.lock().unwrap().push(subscriber(
            imsi,
            Some(GENUINE_ESN),
            None,
            SubscriberStatus::Active,
        ));
    }
    let mut runtime = closed_runtime(
        crate::authentication::AuthenticationMode::Off,
        RegisterPolicy::Any,
        hlr.clone(),
    );
    let (reply, _) = register(
        &mut runtime,
        registration(Some(DEFAULT_IMSI), Some(GENUINE_ESN)),
    )
    .await;
    assert_eq!(
        reply,
        cdma_ios::MessageType::LocationUpdatingReject,
        "an ambiguous match is denied even under register_policy any"
    );
    assert_eq!(
        hlr.admissions.lock().unwrap()[0].reason,
        "ambiguous_identity"
    );

    let hlr = Arc::new(StubHlrRepo::default());
    hlr.lookups_fail
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let mut runtime = closed_runtime(
        crate::authentication::AuthenticationMode::Off,
        RegisterPolicy::Any,
        hlr,
    );
    let (reply, cause) = register(
        &mut runtime,
        registration(Some(AUTH_TEST_IMSI), Some(GENUINE_ESN)),
    )
    .await;
    assert_eq!(reply, cdma_ios::MessageType::LocationUpdatingReject);
    assert_eq!(cause, Some(cdma_ios::REJECT_CAUSE_NETWORK_FAILURE));
}

#[tokio::test]
async fn admitted_transition_is_reported_once() {
    let hlr = Arc::new(StubHlrRepo::default());
    hlr.subscribers.lock().unwrap().push(subscriber(
        AUTH_TEST_IMSI,
        Some(GENUINE_ESN),
        None,
        SubscriberStatus::Active,
    ));
    let mut runtime = closed_runtime(
        crate::authentication::AuthenticationMode::Off,
        RegisterPolicy::SubscriberOnly,
        hlr.clone(),
    );
    let mut events = runtime.otasp_event_tx.subscribe();
    for _ in 0..3 {
        let (reply, _) = register(
            &mut runtime,
            registration(Some(AUTH_TEST_IMSI), Some(GENUINE_ESN)),
        )
        .await;
        assert_eq!(reply, cdma_ios::MessageType::LocationUpdatingAccept);
    }
    assert_eq!(
        admission_classes(&drain(&mut events)),
        vec![(
            crate::grpc::events_proto::v1::AdmissionClass::Admitted as i32,
            false
        )]
    );
    assert_eq!(hlr.admissions.lock().unwrap().len(), 1);
}

fn origination(imsi: &str, esn: u32, dialed: &[u8], service_option: u16) -> EncodedA1Message {
    let mut cli3 = cm_service_request_cli3(None);
    let mut request = cli3.layer3_information.decode_cm_service_request().unwrap();
    request.mobile_identity_imsi = MobileIdentity::Imsi(imsi.to_string());
    request.mobile_identity_esn = Some(MobileIdentity::Esn(esn));
    request.called_party_ascii_number = None;
    request.called_party_bcd_number = Some(cdma_ios::CalledPartyBcdNumber(dialed.to_vec()));
    request.service_option = Some(ServiceOption(service_option));
    cli3.layer3_information =
        cdma_ios::Layer3Information::from_cm_service_request(&request).unwrap();
    EncodedA1Message::from_message_for_call(
        &cdma_ios::Message::new(
            cdma_ios::MessageType::CompleteLayer3Information,
            cli3.encode().unwrap(),
        ),
        Some(77),
    )
}

/// `*228` as Called Party BCD Number digits (A.S0014-D §4.2.43).
const OTASP_BCD: [u8; 3] = [0x81, 0x2A, 0x82];
const VOICE_BCD: [u8; 6] = [0x81, 0x55, 0x95, 0x78, 0x56, 0x34];

async fn originate(
    runtime: &mut MscRuntime,
    message: EncodedA1Message,
) -> Option<cdma_ios::MessageType> {
    let (client, endpoint) = cdma_bsc_a1_edge_compat::InProcessMscClient::pair(8);
    runtime
        .handle_bsc_a1_message(&endpoint, &test_node(), message)
        .await;
    runtime.flush_admission_history().await;
    timeout(REPLY_WAIT, client.poll_a1())
        .await
        .ok()
        .flatten()
        .map(|reply| reply.message_type())
}

fn enable_otasp(runtime: &mut MscRuntime) {
    let cfg = crate::config::OtaspConfig {
        enabled: true,
        ..Default::default()
    };
    runtime.otasp = Some(crate::otasp::OtaspCoordinator::new(
        cfg.clone(),
        cfg.home_network.clone(),
        runtime.config.hlr_repo.clone(),
    ));
}

#[tokio::test]
async fn closed_mode_clears_originations_the_class_does_not_permit() {
    let hlr = Arc::new(StubHlrRepo::default());
    hlr.subscribers.lock().unwrap().push(subscriber(
        CONFIGURED_IMSI,
        Some(GENUINE_ESN),
        None,
        SubscriberStatus::Active,
    ));
    let mut runtime = closed_runtime(
        crate::authentication::AuthenticationMode::Off,
        RegisterPolicy::Any,
        hlr,
    );
    enable_otasp(&mut runtime);
    assert_eq!(
        originate(
            &mut runtime,
            origination(
                AUTH_TEST_IMSI,
                0x9F00_0001,
                &VOICE_BCD,
                SERVICE_OPTION_BASIC_VOICE
            )
        )
        .await,
        Some(cdma_ios::MessageType::ClearCommand),
        "an unknown mobile cannot place a call even under register_policy any"
    );
    assert_eq!(
        originate(
            &mut runtime,
            origination(
                DEFAULT_IMSI,
                GENUINE_ESN,
                &VOICE_BCD,
                SERVICE_OPTION_BASIC_VOICE
            )
        )
        .await,
        Some(cdma_ios::MessageType::ClearCommand),
        "a provisioning mobile cannot place an ordinary call"
    );
    assert_eq!(
        originate(
            &mut runtime,
            origination(DEFAULT_IMSI, GENUINE_ESN, &OTASP_BCD, SERVICE_OPTION_OTASP)
        )
        .await,
        Some(cdma_ios::MessageType::AssignmentRequest),
        "a provisioning mobile with no A-key may run OTASP"
    );
}

#[tokio::test]
async fn keyed_otasp_origination_requires_proof_unless_authentication_is_off() {
    for (mode, expected) in [
        (
            crate::authentication::AuthenticationMode::Off,
            cdma_ios::MessageType::AssignmentRequest,
        ),
        (
            crate::authentication::AuthenticationMode::Monitor,
            cdma_ios::MessageType::ClearCommand,
        ),
        (
            crate::authentication::AuthenticationMode::Enforce,
            cdma_ios::MessageType::ClearCommand,
        ),
    ] {
        for access in [AccessMode::Open, AccessMode::Closed] {
            let hlr = Arc::new(StubHlrRepo::default());
            hlr.subscribers.lock().unwrap().push(subscriber(
                AUTH_TEST_IMSI,
                Some(GENUINE_ESN),
                None,
                SubscriberStatus::Active,
            ));
            let mut runtime = authenticating_runtime_with_hlr(mode, hlr);
            runtime.config.access.mode = access;
            enable_otasp(&mut runtime);
            let mut message = origination(
                AUTH_TEST_IMSI,
                GENUINE_ESN,
                &OTASP_BCD,
                SERVICE_OPTION_OTASP,
            );
            if mode != crate::authentication::AuthenticationMode::Off {
                let decoded = message.decode().unwrap();
                let mut cli3 = CompleteLayer3InformationMessage::decode(&decoded.payload).unwrap();
                let mut request = cli3.layer3_information.decode_cm_service_request().unwrap();
                request.authentication_event = Some(cdma_ios::AuthenticationEvent(
                    cdma_ios::AUTHENTICATION_EVENT_PARAMETERS_NOT_RECEIVED,
                ));
                cli3.layer3_information =
                    cdma_ios::Layer3Information::from_cm_service_request(&request).unwrap();
                message = EncodedA1Message::from_message_for_call(
                    &cdma_ios::Message::new(
                        cdma_ios::MessageType::CompleteLayer3Information,
                        cli3.encode().unwrap(),
                    ),
                    Some(77),
                );
            }
            assert_eq!(
                originate(&mut runtime, message).await,
                Some(expected),
                "{mode:?} {access:?}"
            );
        }
    }
}

fn decision(
    result: crate::authentication::AuthenticationResult,
) -> crate::authentication::AuthenticationDecision {
    crate::authentication::AuthenticationDecision {
        procedure: crate::authentication::AuthenticationProcedure::PageResponse,
        result,
        allow: true,
        call_keys: None,
        pending_ssd_update: false,
        inputs: None,
    }
}

#[tokio::test]
async fn keyed_otapa_without_proof_waits_for_a_unique_challenge_that_must_pass() {
    let hlr = Arc::new(StubHlrRepo::default());
    let resolved = subscriber(
        AUTH_TEST_IMSI,
        Some(AUTH_TEST_ESN),
        None,
        SubscriberStatus::Active,
    );
    let subscriber_id = resolved.subscriber.subscriber_id;
    hlr.subscribers.lock().unwrap().push(resolved);
    let mut runtime =
        authenticating_runtime_with_hlr(crate::authentication::AuthenticationMode::Monitor, hlr);
    let (client, endpoint) = cdma_bsc_a1_edge_compat::InProcessMscClient::pair(8);
    let subject = crate::admission::AccessSubject {
        imsi: Some(AUTH_TEST_IMSI),
        esn: Some(AUTH_TEST_ESN),
        meid: None,
    };
    let target = cdma_hlr::model::MobileIdentityKey::from_parts(
        Some(AUTH_TEST_IMSI),
        Some(AUTH_TEST_ESN),
        None,
    )
    .unwrap();
    for (call, result, authorized) in [
        (
            CallId(501),
            crate::authentication::AuthenticationResult::SignatureMismatch,
            false,
        ),
        (
            CallId(502),
            crate::authentication::AuthenticationResult::NotRequested,
            true,
        ),
    ] {
        runtime.mt_call.mt_plans.insert(
            call.0 as u32,
            crate::management::MtCallPlan {
                subscriber_id,
                imsi: AUTH_TEST_IMSI.to_string(),
                audio_file: None,
                caller_number: None,
                service_option: SERVICE_OPTION_OTASP,
            },
        );
        assert_eq!(
            runtime
                .authorize_otapa_response(
                    &endpoint,
                    &test_node(),
                    call,
                    &subject,
                    Some(target.clone()),
                    &decision(result),
                )
                .await,
            authorized,
            "{result:?}"
        );
        assert_eq!(
            runtime.provisioning_challenges.contains_key(&call),
            authorized
        );
    }
    while timeout(Duration::from_millis(10), client.poll_a1())
        .await
        .is_ok_and(|reply| reply.is_some())
    {}

    let call = CallId(502);
    assert!(
        runtime
            .hold_for_provisioning_challenge(
                &endpoint,
                &test_node(),
                call,
                HeldAssignment {
                    completed_circuit_id: None,
                    completed_leg: None,
                },
            )
            .await
    );
    let challenge = timeout(REPLY_WAIT, client.poll_a1())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        challenge.message_type(),
        cdma_ios::MessageType::AuthenticationRequest
    );
    let wrong = cdma_ios::AuthenticationResponseMessage::Dtap(
        cdma_ios::AuthenticationResponseDtapMessage {
            authentication_response_parameter_authu: cdma_ios::AuthenticationResponseParameter::new(
                cdma_ios::AuthSignatureType::Authu,
                0,
            ),
        },
    );
    runtime
        .handle_traffic_authentication_reply(
            &endpoint,
            call,
            cdma_ios::MessageType::AuthenticationResponse,
            &wrong.encode().unwrap(),
        )
        .await;
    let cleared = timeout(REPLY_WAIT, client.poll_a1())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        cleared.message_type(),
        cdma_ios::MessageType::ClearCommand,
        "a failed provisioning challenge clears the call even under monitor"
    );
    assert!(!runtime.provisioning_holds.contains_key(&call));
    assert!(!runtime.provisioning_authorizations.contains_key(&call));
}

#[derive(Default)]
struct SmsSubmissions(std::sync::atomic::AtomicUsize);

#[async_trait::async_trait]
impl cdma_smsc::client::MoSubmitter for SmsSubmissions {
    async fn submit_mo(
        &self,
        _: cdma_smsc::client::MoSubmission,
    ) -> Result<(uuid::Uuid, bool), String> {
        self.0.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        Ok((uuid::Uuid::new_v4(), true))
    }
    async fn submit_mt(&self, _: cdma_smsc::client::MtSubmission) -> Result<uuid::Uuid, String> {
        Ok(uuid::Uuid::new_v4())
    }
}

#[tokio::test]
async fn traffic_sms_rechecks_subscriber_status_in_every_authentication_mode() {
    for authentication in [
        crate::authentication::AuthenticationMode::Off,
        crate::authentication::AuthenticationMode::Monitor,
        crate::authentication::AuthenticationMode::Enforce,
    ] {
        for access in [AccessMode::Open, AccessMode::Closed] {
            for status in [
                SubscriberStatus::Active,
                SubscriberStatus::Suspended,
                SubscriberStatus::Disabled,
            ] {
                let hlr = Arc::new(StubHlrRepo::default());
                let mut resolved = subscriber(
                    AUTH_TEST_IMSI,
                    Some(GENUINE_ESN),
                    None,
                    SubscriberStatus::Active,
                );
                resolved.subscriber.status = status.clone();
                hlr.subscribers.lock().unwrap().push(resolved);
                let mut runtime =
                    closed_runtime(authentication, RegisterPolicy::SubscriberOnly, hlr.clone());
                runtime.config.access.mode = access;
                let repo =
                    cdma_smsc::repository::GrpcSmscRepository::connect_lazy("http://127.0.0.1:1")
                        .unwrap();
                runtime.smsc = Some(crate::sms::MscSmsCoordinator::new(
                    Arc::new(repo),
                    hlr,
                    access,
                ));
                let recorder = Arc::new(SmsSubmissions::default());
                runtime.set_mo_submitter(recorder.clone());
                let call = runtime.controller.create_call(
                    CallDirection::MobileOriginated,
                    Some(MobileIdentity::Imsi(AUTH_TEST_IMSI.into())),
                );
                runtime
                    .controller
                    .set_mobile_identity_esn(call, Some(MobileIdentity::Esn(GENUINE_ESN)))
                    .unwrap();
                let message = cdma_ios::AddsDeliverMessage {
                    adds_user_part: cdma_ios::AddsUserPart {
                        burst_type: cdma_common::consts::BURST_TYPE_SMS,
                        data: cdma_common::sms::encode_mo_sms_submit(
                            "5550000007",
                            "status regression",
                            1,
                        ),
                    },
                    tag: None,
                };
                let (_, endpoint) = cdma_bsc_a1_edge_compat::InProcessMscClient::pair(8);
                runtime
                    .handle_bsc_a1_message(
                        &endpoint,
                        &test_node(),
                        EncodedA1Message::from_message_for_call(
                            &cdma_ios::Message::new(
                                cdma_ios::MessageType::AddsDeliver,
                                message.encode().unwrap(),
                            ),
                            Some(call.0),
                        ),
                    )
                    .await;
                tokio::task::yield_now().await;
                assert_eq!(
                    recorder.0.load(std::sync::atomic::Ordering::Relaxed),
                    usize::from(access == AccessMode::Open || status == SubscriberStatus::Active),
                    "{authentication:?} {access:?} {status:?}"
                );
            }
        }
    }
}

#[tokio::test]
async fn mt_sms_rechecks_current_status_before_submission_and_delivery() {
    for access in [AccessMode::Open, AccessMode::Closed] {
        for raw_user_data in [None, Some(vec![1])] {
            let hlr = Arc::new(StubHlrRepo::default());
            let mut resolved = subscriber(
                AUTH_TEST_IMSI,
                Some(GENUINE_ESN),
                None,
                SubscriberStatus::Active,
            );
            resolved.binding = Some(cdma_hlr::model::RegistrationBinding {
                subscriber_id: resolved.subscriber.subscriber_id,
                serving_bs_id: test_node().0,
                state: cdma_hlr::model::RegistrationState::Registered,
                imsi: Some(AUTH_TEST_IMSI.to_string()),
                esn: Some(GENUINE_ESN),
                meid: None,
                mob_p_rev: None,
                pgslot: None,
                slot_cycle_index: None,
                last_msg_seq: None,
                last_registered_at: chrono::Utc::now(),
                last_seen_at: chrono::Utc::now(),
                updated_at: chrono::Utc::now(),
            });
            hlr.subscribers.lock().unwrap().push(resolved);
            let repo =
                cdma_smsc::repository::GrpcSmscRepository::connect_lazy("http://127.0.0.1:1")
                    .unwrap();
            let mut sms = crate::sms::MscSmsCoordinator::new(Arc::new(repo), hlr.clone(), access);
            sms.record_mobile_contact(AUTH_TEST_IMSI, &test_node());
            let (outcomes, mut results) = tokio::sync::mpsc::channel(8);
            sms.set_outcome_reporter(outcomes);
            let (client, endpoint) = cdma_bsc_a1_edge_compat::InProcessMscClient::pair(8);
            for status in [
                SubscriberStatus::Active,
                SubscriberStatus::Suspended,
                SubscriberStatus::Disabled,
            ] {
                hlr.subscribers.lock().unwrap()[0].subscriber.status = status.clone();
                let allowed = access == AccessMode::Open || status != SubscriberStatus::Disabled;
                for destination in [
                    crate::sms::SmsDestinationKey::PhoneNumber("5550000007".into()),
                    crate::sms::SmsDestinationKey::Imsi(AUTH_TEST_IMSI.into()),
                ] {
                    assert_eq!(sms.destination_admitted(&destination).await, allowed);
                }
                let command = cdma_smsc::proto::DeliveryCommand {
                    sms_id: uuid::Uuid::new_v4().to_string(),
                    attempt_id: uuid::Uuid::new_v4().to_string(),
                    target: Some(cdma_smsc::proto::DeliveryTarget {
                        identity: Some(cdma_smsc::proto::delivery_target::Identity::Imsi(
                            AUTH_TEST_IMSI.into(),
                        )),
                        serving_node: Some(test_node().0),
                    }),
                    originating_number: "2222".into(),
                    text: "delivery status regression".into(),
                    teleservice_id: None,
                    raw_user_data: raw_user_data.clone(),
                };
                sms.deliver_from_smsc(command, &endpoint).await;
                if allowed {
                    if raw_user_data.is_some() {
                        assert_eq!(sms.take_traffic_deliveries().len(), 1);
                    } else {
                        assert_eq!(
                            client.poll_a1().await.unwrap().message_type(),
                            cdma_ios::MessageType::AddsPage
                        );
                    }
                } else {
                    assert_eq!(
                        results.recv().await.unwrap().result,
                        cdma_smsc::proto::delivery_outcome::Result::Failed as i32
                    );
                    assert!(sms.take_traffic_deliveries().is_empty());
                    assert!(client.inbound_rx.lock().await.try_recv().is_err());
                }
            }
            hlr.lookups_fail
                .store(true, std::sync::atomic::Ordering::Relaxed);
            assert_eq!(
                sms.destination_admitted(&crate::sms::SmsDestinationKey::PhoneNumber(
                    "5550000007".into()
                ))
                .await,
                access == AccessMode::Open
            );
        }
    }
}

#[tokio::test]
async fn traffic_sms_preserves_meid_identity_from_an_existing_mt_call() {
    const MEID: &str = "a0000012345678";
    const PSEUDO_ESN: u32 = 0x8012_3456;
    for status in [
        SubscriberStatus::Active,
        SubscriberStatus::Suspended,
        SubscriberStatus::Disabled,
    ] {
        let hlr = Arc::new(StubHlrRepo::default());
        hlr.subscribers.lock().unwrap().push(subscriber(
            AUTH_TEST_IMSI,
            None,
            Some(MEID),
            SubscriberStatus::Active,
        ));
        let mut runtime = closed_runtime(
            crate::authentication::AuthenticationMode::Off,
            RegisterPolicy::SubscriberOnly,
            hlr.clone(),
        );
        let repo =
            cdma_smsc::repository::GrpcSmscRepository::connect_lazy("http://127.0.0.1:1").unwrap();
        runtime.smsc = Some(crate::sms::MscSmsCoordinator::new(
            Arc::new(repo),
            hlr.clone(),
            AccessMode::Closed,
        ));
        let recorder = Arc::new(SmsSubmissions::default());
        runtime.set_mo_submitter(recorder.clone());
        let call = runtime.controller.create_call(
            CallDirection::MobileTerminated,
            Some(MobileIdentity::Imsi(AUTH_TEST_IMSI.into())),
        );
        runtime.call_auth_context.insert(
            call,
            crate::authentication::CallAuthContext {
                identity: cdma_hlr::model::MobileIdentityKey::from_parts(
                    Some(AUTH_TEST_IMSI),
                    Some(PSEUDO_ESN),
                    Some(MEID),
                )
                .unwrap(),
                imsi: AUTH_TEST_IMSI.into(),
                esn: PSEUDO_ESN,
                global_result: crate::authentication::AuthenticationResult::NotRequested,
            },
        );
        hlr.subscribers.lock().unwrap()[0].subscriber.status = status.clone();
        let message = cdma_ios::AddsDeliverMessage {
            adds_user_part: cdma_ios::AddsUserPart {
                burst_type: cdma_common::consts::BURST_TYPE_SMS,
                data: cdma_common::sms::encode_mo_sms_submit(
                    "5550000007",
                    "MEID status regression",
                    1,
                ),
            },
            tag: None,
        };
        let (_, endpoint) = cdma_bsc_a1_edge_compat::InProcessMscClient::pair(8);
        runtime
            .handle_bsc_a1_message(
                &endpoint,
                &test_node(),
                EncodedA1Message::from_message_for_call(
                    &cdma_ios::Message::new(
                        cdma_ios::MessageType::AddsDeliver,
                        message.encode().unwrap(),
                    ),
                    Some(call.0),
                ),
            )
            .await;
        tokio::task::yield_now().await;
        assert_eq!(
            recorder.0.load(std::sync::atomic::Ordering::Relaxed),
            usize::from(status == SubscriberStatus::Active),
            "{status:?}"
        );
        assert!(runtime.controller.snapshot(call).is_some());
    }
}

#[tokio::test]
async fn current_admission_changes_after_subscriber_deletion() {
    let hlr = Arc::new(StubHlrRepo::default());
    hlr.subscribers.lock().unwrap().push(subscriber(
        AUTH_TEST_IMSI,
        Some(GENUINE_ESN),
        None,
        SubscriberStatus::Active,
    ));
    let mut runtime = closed_runtime(
        crate::authentication::AuthenticationMode::Off,
        RegisterPolicy::SubscriberOnly,
        hlr.clone(),
    );
    for _ in 0..2 {
        let (reply, _) = register(
            &mut runtime,
            registration(Some(AUTH_TEST_IMSI), Some(GENUINE_ESN)),
        )
        .await;
        assert_eq!(reply, cdma_ios::MessageType::LocationUpdatingAccept);
    }
    assert_eq!(
        runtime.admission_history.matching(&[], None, None, 0).len(),
        1
    );
    hlr.subscribers.lock().unwrap().clear();
    let (reply, _) = register(
        &mut runtime,
        registration(Some(AUTH_TEST_IMSI), Some(GENUINE_ESN)),
    )
    .await;
    assert_eq!(reply, cdma_ios::MessageType::LocationUpdatingReject);
    let current = runtime
        .admission_history
        .current_matching(&[], Some(GENUINE_ESN), None, 0);
    assert_eq!(current.len(), 1);
    assert_eq!(
        current[0].admission_class,
        crate::grpc::events_proto::v1::AdmissionClass::Denied as i32
    );
    assert!(current[0].subscriber_id.is_empty());
}

#[tokio::test]
async fn open_otasp_rejects_ambiguous_and_conflicting_hardware() {
    for ambiguous in [false, true] {
        let hlr = Arc::new(StubHlrRepo::default());
        hlr.subscribers.lock().unwrap().push(subscriber(
            CONFIGURED_IMSI,
            Some(GENUINE_ESN),
            Some("A0000000000001"),
            SubscriberStatus::Active,
        ));
        if ambiguous {
            hlr.subscribers.lock().unwrap().push(subscriber(
                "310005550000008",
                Some(GENUINE_ESN),
                None,
                SubscriberStatus::Active,
            ));
        }
        let mut runtime =
            authenticating_runtime_with_hlr(crate::authentication::AuthenticationMode::Off, hlr);
        enable_otasp(&mut runtime);
        let mut message = origination(DEFAULT_IMSI, GENUINE_ESN, &OTASP_BCD, SERVICE_OPTION_OTASP);
        if !ambiguous {
            let cli3 = CompleteLayer3InformationMessage::decode(&message.decode().unwrap().payload)
                .unwrap();
            let mut request = cli3.layer3_information.decode_cm_service_request().unwrap();
            request.mobile_identity_meid = Some(MobileIdentity::Meid([0xA0, 0, 0, 0, 0, 0, 2]));
            let cli3 = CompleteLayer3InformationMessage {
                layer3_information: cdma_ios::Layer3Information::from_cm_service_request(&request)
                    .unwrap(),
                ..cli3
            };
            message = EncodedA1Message::from_message_for_call(
                &cdma_ios::Message::new(
                    cdma_ios::MessageType::CompleteLayer3Information,
                    cli3.encode().unwrap(),
                ),
                message.call_id(),
            );
        }
        assert_eq!(
            originate(&mut runtime, message).await,
            Some(cdma_ios::MessageType::ClearCommand)
        );
        assert!(runtime.provisioning_authorizations.is_empty());
    }
}

#[tokio::test]
async fn admission_rejection_does_not_wait_for_history_storage() {
    let hlr = Arc::new(StubHlrRepo::default());
    hlr.block_admission_writes
        .store(true, std::sync::atomic::Ordering::Relaxed);
    let mut runtime = closed_runtime(
        crate::authentication::AuthenticationMode::Off,
        RegisterPolicy::SubscriberOnly,
        hlr,
    );
    let (client, endpoint) = cdma_bsc_a1_edge_compat::InProcessMscClient::pair(8);
    timeout(
        Duration::from_millis(100),
        runtime.handle_bsc_a1_message(
            &endpoint,
            &test_node(),
            registration(Some(DEFAULT_IMSI), Some(GENUINE_ESN)),
        ),
    )
    .await
    .expect("history storage cannot block registration");
    assert_eq!(
        timeout(REPLY_WAIT, client.poll_a1())
            .await
            .unwrap()
            .unwrap()
            .message_type(),
        cdma_ios::MessageType::LocationUpdatingReject
    );
}

#[tokio::test]
async fn network_data_calls_refuse_inactive_subscribers_before_paging() {
    for status in [SubscriberStatus::Suspended, SubscriberStatus::Disabled] {
        let hlr = Arc::new(StubHlrRepo::default());
        let target = subscriber(CONFIGURED_IMSI, Some(GENUINE_ESN), None, status);
        let id = target.subscriber.subscriber_id;
        hlr.subscribers.lock().unwrap().push(target);
        let mut runtime = closed_runtime(
            crate::authentication::AuthenticationMode::Off,
            RegisterPolicy::SubscriberOnly,
            hlr,
        );
        let (client, endpoint) = cdma_bsc_a1_edge_compat::InProcessMscClient::pair(8);
        assert!(
            runtime
                .initiate_data_call(&endpoint, id, SERVICE_OPTION_PACKET_DATA_RC3)
                .await
                .is_err()
        );
        assert!(
            timeout(Duration::from_millis(10), client.poll_a1())
                .await
                .is_err()
        );
    }
}
