use super::*;

const PRIVACY_TEST_CALL_ID: CallId = CallId(77);
const PRIVACY_TEST_RESPONSE_TIMEOUT: Duration = Duration::from_secs(1);
const PRIVACY_TEST_NO_COMMAND_WAIT: Duration = Duration::from_millis(20);
const PRIVACY_TEST_MASK: u64 = (1 << 40) | 0x1234;

fn privacy_test_runtime() -> MscRuntime {
    let mut runtime = authenticating_runtime(crate::authentication::AuthenticationMode::Monitor);
    runtime.config.authentication.unique_challenge_on_call = false;
    runtime.config.authentication.count_update_on_call = false;
    runtime.controller.create_call_with_id(
        PRIVACY_TEST_CALL_ID,
        CallDirection::MobileOriginated,
        Some(MobileIdentity::Imsi(AUTH_TEST_IMSI.to_string())),
    );
    runtime
        .call_auth_context
        .insert(PRIVACY_TEST_CALL_ID, traffic_test_context());
    runtime.call_keys.insert(
        PRIVACY_TEST_CALL_ID,
        cdma_hlr::auth::CallKeyMaterial {
            cmea_key: [0; 8],
            private_long_code_mask: PRIVACY_TEST_MASK,
        },
    );
    runtime
}

async fn verify_privacy_test_authu(
    runtime: &mut MscRuntime,
    endpoint: &dyn MscA1Endpoint,
    challenge: EncodedA1Message,
    authu_flip: u32,
) {
    assert_eq!(
        challenge.message_type(),
        cdma_ios::MessageType::AuthenticationRequest
    );
    let request =
        cdma_ios::AuthenticationRequestMessage::decode(&challenge.decode().unwrap().payload)
            .unwrap();
    let cdma_ios::AuthenticationRequestMessage::Dtap(request) = request else {
        panic!("expected DTAP challenge")
    };
    let randu = request
        .authentication_challenge_parameter_randu
        .randu()
        .unwrap();
    let (s1, s2) = cdma_common::paging::imsi_s_from_imsi(AUTH_TEST_IMSI).unwrap();
    let authu = cdma_cave::authu(
        &STUB_AUTH_SSD.ssd_a,
        randu,
        s1,
        s2,
        cdma_cave::Esn(AUTH_TEST_ESN),
    )
    .unwrap()
    .signature;
    let response = cdma_ios::AuthenticationResponseMessage::Dtap(
        cdma_ios::AuthenticationResponseDtapMessage {
            authentication_response_parameter_authu: cdma_ios::AuthenticationResponseParameter::new(
                cdma_ios::AuthSignatureType::Authu,
                authu ^ authu_flip,
            ),
        },
    );
    runtime
        .handle_traffic_authentication_reply(
            endpoint,
            PRIVACY_TEST_CALL_ID,
            cdma_ios::MessageType::AuthenticationResponse,
            &response.encode().unwrap(),
        )
        .await;
}

#[tokio::test]
async fn privacy_assignment_preloads_keys_and_activates_only_after_verified_authu() {
    let (client, endpoint) = cdma_bsc_a1_edge_compat::InProcessMscClient::pair(4);
    let mut runtime = privacy_test_runtime();
    let assignment = runtime
        .assignment_privacy(PRIVACY_TEST_CALL_ID, true)
        .unwrap();
    let parameters = assignment.private_long_code_parameter().unwrap().unwrap();
    assert_eq!(parameters.mask, PRIVACY_TEST_MASK);
    assert!(!parameters.active);
    assert!(runtime.held_assignments.is_empty());
    runtime
        .start_traffic_authentication(&endpoint, &BaseStationId("bs".into()), PRIVACY_TEST_CALL_ID)
        .await;
    let challenge = timeout(PRIVACY_TEST_RESPONSE_TIMEOUT, client.poll_a1())
        .await
        .expect("expected A1 response")
        .unwrap();
    verify_privacy_test_authu(&mut runtime, &endpoint, challenge, 0).await;
    let activation = timeout(PRIVACY_TEST_RESPONSE_TIMEOUT, client.poll_a1())
        .await
        .expect("expected A1 response")
        .unwrap();
    assert_eq!(
        activation.message_type(),
        cdma_ios::MessageType::PrivacyModeCommand
    );
    let command =
        cdma_ios::PrivacyModeCommandMessage::decode(&activation.decode().unwrap().payload).unwrap();
    let parameters = command
        .encryption_information
        .private_long_code_parameter()
        .unwrap()
        .unwrap();
    assert!(parameters.active);
    assert_eq!(parameters.mask, PRIVACY_TEST_MASK);
    assert!(runtime.pending_privacy.is_empty());
    assert!(
        runtime
            .privacy_authenticated
            .contains(&PRIVACY_TEST_CALL_ID)
    );
    runtime
        .start_traffic_authentication(&endpoint, &BaseStationId("bs".into()), PRIVACY_TEST_CALL_ID)
        .await;
    assert!(
        timeout(PRIVACY_TEST_NO_COMMAND_WAIT, client.poll_a1())
            .await
            .is_err()
    );
    runtime.stop_media_for_call(PRIVACY_TEST_CALL_ID);
    assert!(runtime.privacy_authenticated.is_empty());
    assert!(runtime.privacy_deadlines.is_empty());
    assert!(runtime.call_auth_context.is_empty());
    assert!(runtime.traffic_auth_started.is_empty());
}

#[tokio::test]
async fn calls_without_requested_privacy_keep_unique_challenge_disabled() {
    for policy in [
        crate::authentication::VoicePrivacyPolicy::Optional,
        crate::authentication::VoicePrivacyPolicy::Never,
    ] {
        let (client, endpoint) = cdma_bsc_a1_edge_compat::InProcessMscClient::pair(4);
        let mut runtime = privacy_test_runtime();
        runtime.config.authentication.voice_privacy = policy;
        runtime.assignment_privacy(
            PRIVACY_TEST_CALL_ID,
            policy == crate::authentication::VoicePrivacyPolicy::Never,
        );
        runtime
            .start_traffic_authentication(&endpoint, &test_node(), PRIVACY_TEST_CALL_ID)
            .await;
        assert!(
            timeout(PRIVACY_TEST_NO_COMMAND_WAIT, client.poll_a1())
                .await
                .is_err()
        );
        assert!(runtime.pending_privacy.is_empty());
        assert!(runtime.traffic_auth.is_empty());
    }
}

#[test]
fn a_revised_assignment_without_privacy_cancels_pending_activation() {
    let mut runtime = privacy_test_runtime();
    runtime
        .assignment_privacy(PRIVACY_TEST_CALL_ID, true)
        .unwrap();
    assert!(runtime.pending_privacy.contains_key(&PRIVACY_TEST_CALL_ID));
    let assignment = runtime
        .assignment_privacy(PRIVACY_TEST_CALL_ID, false)
        .unwrap();
    assert!(
        !assignment
            .private_long_code_parameter()
            .unwrap()
            .unwrap()
            .active
    );
    assert!(runtime.pending_privacy.is_empty());
}

#[tokio::test]
async fn wrong_authu_does_not_activate_privacy_in_monitor_mode() {
    let (client, endpoint) = cdma_bsc_a1_edge_compat::InProcessMscClient::pair(4);
    let mut runtime = privacy_test_runtime();
    runtime
        .assignment_privacy(PRIVACY_TEST_CALL_ID, true)
        .unwrap();
    runtime
        .start_traffic_authentication(&endpoint, &BaseStationId("bs".into()), PRIVACY_TEST_CALL_ID)
        .await;
    let challenge = timeout(PRIVACY_TEST_RESPONSE_TIMEOUT, client.poll_a1())
        .await
        .expect("expected A1 response")
        .unwrap();
    verify_privacy_test_authu(&mut runtime, &endpoint, challenge, 1).await;
    assert!(
        timeout(PRIVACY_TEST_NO_COMMAND_WAIT, client.poll_a1())
            .await
            .is_err()
    );
    assert!(runtime.pending_privacy.is_empty());
    assert!(runtime.privacy_authenticated.is_empty());
    assert!(runtime.privacy_deadlines.is_empty());
}

#[tokio::test]
async fn handset_privacy_request_during_count_update_waits_for_a_verified_challenge() {
    let (client, endpoint) = cdma_bsc_a1_edge_compat::InProcessMscClient::pair(4);
    let mut runtime = privacy_test_runtime();
    runtime.config.authentication.count_update_on_call = true;
    runtime
        .start_traffic_authentication(&endpoint, &test_node(), PRIVACY_TEST_CALL_ID)
        .await;
    assert_eq!(
        timeout(PRIVACY_TEST_RESPONSE_TIMEOUT, client.poll_a1())
            .await
            .expect("expected A1 response")
            .unwrap()
            .message_type(),
        cdma_ios::MessageType::ParameterUpdateRequest
    );
    let request = cdma_ios::PrivacyModeCompleteMessage {
        encryption_information: None,
        voice_privacy_request: true,
    };
    runtime
        .handle_bsc_a1_message(
            &endpoint,
            &test_node(),
            EncodedA1Message::from_message_for_call(
                &cdma_ios::Message::new(
                    cdma_ios::MessageType::PrivacyModeComplete,
                    request.encode().unwrap(),
                ),
                Some(PRIVACY_TEST_CALL_ID.0),
            ),
        )
        .await;
    assert!(
        timeout(PRIVACY_TEST_NO_COMMAND_WAIT, client.poll_a1())
            .await
            .is_err()
    );
    let confirm = cdma_ios::ParameterUpdateConfirmMessage;
    runtime
        .handle_traffic_authentication_reply(
            &endpoint,
            PRIVACY_TEST_CALL_ID,
            cdma_ios::MessageType::ParameterUpdateConfirm,
            &confirm.encode().unwrap(),
        )
        .await;
    let challenge = timeout(PRIVACY_TEST_RESPONSE_TIMEOUT, client.poll_a1())
        .await
        .expect("expected A1 response")
        .unwrap();
    verify_privacy_test_authu(&mut runtime, &endpoint, challenge, 0).await;
    assert_eq!(
        timeout(PRIVACY_TEST_RESPONSE_TIMEOUT, client.poll_a1())
            .await
            .expect("expected A1 response")
            .unwrap()
            .message_type(),
        cdma_ios::MessageType::PrivacyModeCommand
    );
}

#[tokio::test]
async fn timed_out_unique_challenge_does_not_activate_privacy() {
    let (client, endpoint) = cdma_bsc_a1_edge_compat::InProcessMscClient::pair(4);
    let mut runtime = privacy_test_runtime();
    runtime
        .assignment_privacy(PRIVACY_TEST_CALL_ID, true)
        .unwrap();
    runtime
        .start_traffic_authentication(&endpoint, &BaseStationId("bs".into()), PRIVACY_TEST_CALL_ID)
        .await;
    assert_eq!(
        timeout(PRIVACY_TEST_RESPONSE_TIMEOUT, client.poll_a1())
            .await
            .expect("expected A1 response")
            .unwrap()
            .message_type(),
        cdma_ios::MessageType::AuthenticationRequest
    );
    runtime
        .traffic_auth
        .get_mut(&PRIVACY_TEST_CALL_ID)
        .unwrap()
        .deadline = tokio::time::Instant::now();
    runtime.tick_traffic_authentication(&endpoint).await;
    assert!(
        timeout(PRIVACY_TEST_NO_COMMAND_WAIT, client.poll_a1())
            .await
            .is_err()
    );
    assert!(runtime.pending_privacy.is_empty());
    assert!(runtime.privacy_authenticated.is_empty());
}
