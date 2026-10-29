use super::*;

const RESPONSE_WAIT: Duration = Duration::from_secs(1);

#[tokio::test]
async fn provisioning_page_bypasses_missing_or_invalid_auth_but_voice_does_not() {
    for authr in [None, Some(0), Some(1)] {
        for provisioning in [false, true] {
            let hlr = Arc::new(M2mHlrRepo::new());
            let mut runtime = m2m_runtime(hlr.clone());
            runtime.config.authentication.mode = crate::authentication::AuthenticationMode::Enforce;
            let cfg = crate::config::OtaspConfig {
                enabled: true,
                ..Default::default()
            };
            runtime.otasp = Some(crate::otasp::OtaspCoordinator::new(
                cfg.clone(),
                cfg.home_network.clone(),
                hlr.clone(),
            ));
            let (client, endpoint) = cdma_bsc_a1_edge_compat::InProcessMscClient::pair(32);
            let mut events = runtime.otasp_event_tx.subscribe();
            let call_id = if provisioning {
                runtime
                    .initiate_otasp(&endpoint, hlr.subscriber_id, None)
                    .await
                    .unwrap()
                    .call_id
            } else {
                let resolved = runtime
                    .config
                    .hlr_repo
                    .get_subscriber_by_id(hlr.subscriber_id)
                    .await
                    .unwrap()
                    .unwrap();
                runtime
                    .start_mt_call(&endpoint, resolved, None, None)
                    .await
                    .unwrap()
            };
            let page = client.poll_a1().await.unwrap();
            let page = PagingRequestMessage::decode(&page.decode().unwrap().payload).unwrap();
            assert_eq!(
                page.service_option.unwrap().0,
                if provisioning {
                    SERVICE_OPTION_OTASP
                } else {
                    3
                }
            );
            let mut response = paging_response();
            response.mobile_identity_imsi = MobileIdentity::Imsi(hlr.imsi.into());
            response.mobile_identity_esn = Some(MobileIdentity::Esn(AUTH_TEST_ESN));
            response.tag = page.tag;
            response.service_option = Some(ServiceOption(if provisioning {
                SERVICE_OPTION_OTASP
            } else {
                3
            }));
            response.authentication_event = Some(cdma_ios::AuthenticationEvent(1));
            response.authentication_response_parameter = authr.map(|value| {
                cdma_ios::AuthenticationResponseParameter::new(
                    cdma_ios::AuthSignatureType::Authr,
                    value,
                )
            });
            response.authentication_confirmation_parameter = Some(
                cdma_ios::AuthenticationConfirmationParameter((AUTH_TEST_RAND >> 24) as u8),
            );
            response.authentication_challenge_parameter =
                Some(cdma_ios::AuthenticationChallengeParameter::new(
                    cdma_ios::RandomNumberType::Rand,
                    u64::from(AUTH_TEST_RAND),
                ));
            runtime
                .handle_bsc_a1_message(
                    &endpoint,
                    &test_node(),
                    EncodedA1Message::from_message_for_call(
                        &cdma_ios::Message::new(
                            cdma_ios::MessageType::PagingResponse,
                            response.encode().unwrap(),
                        ),
                        Some(call_id.0),
                    ),
                )
                .await;
            let reply = timeout(RESPONSE_WAIT, client.poll_a1())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                reply.message_type(),
                if provisioning {
                    cdma_ios::MessageType::AssignmentRequest
                } else {
                    cdma_ios::MessageType::ClearCommand
                }
            );
            let mut emitted = Vec::new();
            while let Ok(event) = events.try_recv() {
                emitted.push(event);
            }
            assert_eq!(
                authentication_results(&emitted).last().unwrap().1,
                !provisioning
            );
            assert!(hlr.refreshed_binding.lock().unwrap().is_none());
            assert!(!runtime.pending_ssd_calls.contains(&call_id));
            assert!(!runtime.call_auth_context.contains_key(&call_id));
            if provisioning {
                assert!(
                    runtime
                        .initiate_otasp(&endpoint, hlr.subscriber_id, None)
                        .await
                        .is_err()
                );
                runtime.stop_media_for_call(call_id);
                assert!(!runtime.pending_otapa_calls.contains_key(&call_id));
                assert!(!runtime.pending_otasp_originations.contains_key(&call_id));
            }
        }
    }
}

#[tokio::test]
async fn provisioning_requires_enabled_otasp_and_a_known_subscriber() {
    let hlr = Arc::new(M2mHlrRepo::new());
    let mut runtime = m2m_runtime(hlr.clone());
    let (_, endpoint) = cdma_bsc_a1_edge_compat::InProcessMscClient::pair(8);
    assert!(
        runtime
            .initiate_otasp(&endpoint, hlr.subscriber_id, None)
            .await
            .is_err()
    );
    let cfg = crate::config::OtaspConfig {
        enabled: true,
        ..Default::default()
    };
    runtime.otasp = Some(crate::otasp::OtaspCoordinator::new(
        cfg.clone(),
        cfg.home_network.clone(),
        hlr.clone(),
    ));
    assert!(
        runtime
            .initiate_otasp(&endpoint, hlr.subscriber_id, Some("310009999999999"))
            .await
            .is_err()
    );
    assert!(matches!(
        runtime
            .initiate_otasp(&endpoint, uuid::Uuid::nil(), None)
            .await,
        Err(ManagementError::UnknownSubscriber(_))
    ));
}

const FACTORY_IMSI: &str = "310000000006203";
const PROVISIONING_TEST_ESN: u32 = 0x1234_5678;
const PROVISIONING_TEST_MEID: &str = "A0000025F07973";

#[tokio::test]
async fn factory_imsi_pages_and_validates_by_meid_or_real_esn() {
    for meid in [None, Some(PROVISIONING_TEST_MEID)] {
        for correct_hardware in [false, true] {
            let mut repo = M2mHlrRepo::new();
            repo.esn = Some(PROVISIONING_TEST_ESN);
            repo.meid = meid;
            let hlr = Arc::new(repo);
            let mut runtime = m2m_runtime(hlr.clone());
            runtime.config.authentication.mode = crate::authentication::AuthenticationMode::Enforce;
            let cfg = crate::config::OtaspConfig {
                enabled: true,
                ..Default::default()
            };
            runtime.otasp = Some(crate::otasp::OtaspCoordinator::new(
                cfg.clone(),
                cfg.home_network.clone(),
                hlr.clone(),
            ));
            let key =
                crate::network_otasp::ProvisioningIdentity::from_parts(FACTORY_IMSI, hlr.esn, meid);
            runtime.otapa_routes.insert(
                key,
                crate::network_otasp::ProvisioningRoute {
                    node: test_node(),
                    imsi: FACTORY_IMSI.into(),
                    meid: meid.map(str::to_string),
                    slot_cycle_index: Some(2),
                },
            );
            let (client, endpoint) = cdma_bsc_a1_edge_compat::InProcessMscClient::pair(32);
            let call_id = runtime
                .initiate_otasp(&endpoint, hlr.subscriber_id, Some(FACTORY_IMSI))
                .await
                .unwrap()
                .call_id;
            let message = client.poll_a1().await.unwrap();
            let page = PagingRequestMessage::decode(&message.decode().unwrap().payload).unwrap();
            assert_eq!(
                page.mobile_identity_imsi,
                MobileIdentity::Imsi(FACTORY_IMSI.into())
            );
            assert_eq!(page.mobile_identity_meid.is_some(), meid.is_some());
            let mut response = paging_response();
            response.mobile_identity_imsi = MobileIdentity::Imsi(FACTORY_IMSI.into());
            response.mobile_identity_esn = Some(MobileIdentity::Esn(if correct_hardware {
                PROVISIONING_TEST_ESN
            } else {
                PROVISIONING_TEST_ESN + 1
            }));
            response.mobile_identity_meid = meid.map(|_| {
                MobileIdentity::Meid(if correct_hardware {
                    [0xa0, 0, 0, 0x25, 0xf0, 0x79, 0x73]
                } else {
                    [0; 7]
                })
            });
            response.tag = page.tag;
            response.service_option = Some(ServiceOption(SERVICE_OPTION_OTASP));
            runtime
                .handle_bsc_a1_message(
                    &endpoint,
                    &test_node(),
                    EncodedA1Message::from_message_for_call(
                        &cdma_ios::Message::new(
                            cdma_ios::MessageType::PagingResponse,
                            response.encode().unwrap(),
                        ),
                        Some(call_id.0),
                    ),
                )
                .await;
            let reply = timeout(RESPONSE_WAIT, client.poll_a1())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                reply.message_type(),
                if correct_hardware {
                    cdma_ios::MessageType::AssignmentRequest
                } else {
                    cdma_ios::MessageType::ClearCommand
                }
            );
            assert!(hlr.refreshed_binding.lock().unwrap().is_none());
        }
    }
}

#[test]
fn provisioning_hardware_keys_distinguish_factory_imsi_and_pseudo_esn_collisions() {
    use crate::network_otasp::ProvisioningIdentity;
    let first = ProvisioningIdentity::from_parts(
        FACTORY_IMSI,
        Some(AUTH_TEST_ESN),
        Some(PROVISIONING_TEST_MEID),
    );
    let second =
        ProvisioningIdentity::from_parts(FACTORY_IMSI, Some(AUTH_TEST_ESN), Some("A0000025F07974"));
    assert!(first != second);
    assert!(!first.matches(FACTORY_IMSI, Some(AUTH_TEST_ESN), None));
    let pseudo = ProvisioningIdentity::from_parts(FACTORY_IMSI, Some(AUTH_TEST_ESN), None);
    assert!(matches!(pseudo, ProvisioningIdentity::Imsi(_)));
}

#[test]
fn otasp_transfer_sessions_do_not_alias_on_pseudo_esn() {
    use crate::otasp::SessionKey;
    let mut message = cdma_ios::AddsTransferMessage {
        mobile_identity_imsi: MobileIdentity::Imsi(FACTORY_IMSI.into()),
        adds_user_part: cdma_ios::AddsUserPart {
            burst_type: cdma_common::consts::BURST_TYPE_OTASP,
            data: Vec::new(),
        },
        mobile_identity_esn: Some(MobileIdentity::Esn(AUTH_TEST_ESN)),
        mobile_identity_meid: Some(MobileIdentity::Meid([0xa0, 0, 0, 0x25, 0xf0, 0x79, 0x73])),
        authentication_response_parameter: None,
        authentication_confirmation_parameter: None,
        authentication_parameter_count: None,
        authentication_challenge_parameter: None,
        authentication_event: None,
        authentication_data: None,
        cell_identifier: None,
        tag: None,
        service_option: None,
    };
    let first = SessionKey::from_transfer(&message).unwrap();
    message.mobile_identity_meid = Some(MobileIdentity::Meid([0xa0, 0, 0, 0x25, 0xf0, 0x79, 0x74]));
    assert_ne!(SessionKey::from_transfer(&message).unwrap(), first);
    message.mobile_identity_meid = None;
    assert_eq!(
        SessionKey::from_transfer(&message),
        Some(SessionKey::Esn(AUTH_TEST_ESN))
    );
}

#[tokio::test]
async fn provisioning_uses_configured_esn_when_response_adds_unconfigured_meid() {
    for esn in [AUTH_TEST_ESN, PROVISIONING_TEST_ESN] {
        for correct_esn in [false, true] {
            let mut repo = M2mHlrRepo::new();
            repo.esn = Some(esn);
            repo.strict_hardware_lookup = true;
            let hlr = Arc::new(repo);
            let mut runtime = m2m_runtime(hlr.clone());
            runtime.config.authentication.mode = crate::authentication::AuthenticationMode::Enforce;
            let cfg = crate::config::OtaspConfig {
                enabled: true,
                ..Default::default()
            };
            runtime.otasp = Some(crate::otasp::OtaspCoordinator::new(
                cfg.clone(),
                cfg.home_network.clone(),
                hlr.clone(),
            ));
            runtime.otapa_routes.insert(
                crate::network_otasp::ProvisioningIdentity::from_parts(hlr.imsi, hlr.esn, None),
                crate::network_otasp::ProvisioningRoute {
                    node: test_node(),
                    imsi: hlr.imsi.into(),
                    meid: None,
                    slot_cycle_index: Some(2),
                },
            );
            let (client, endpoint) = cdma_bsc_a1_edge_compat::InProcessMscClient::pair(32);
            let call_id = runtime
                .initiate_otasp(&endpoint, hlr.subscriber_id, None)
                .await
                .unwrap()
                .call_id;
            let message = client.poll_a1().await.unwrap();
            let page = PagingRequestMessage::decode(&message.decode().unwrap().payload).unwrap();
            let mut response = paging_response();
            response.mobile_identity_imsi = MobileIdentity::Imsi(hlr.imsi.into());
            response.mobile_identity_esn =
                Some(MobileIdentity::Esn(if correct_esn { esn } else { esn + 1 }));
            response.mobile_identity_meid =
                Some(MobileIdentity::Meid([0xa0, 0, 0, 0x25, 0xf0, 0x79, 0x73]));
            response.tag = page.tag;
            response.service_option = Some(ServiceOption(SERVICE_OPTION_OTASP));
            runtime
                .handle_bsc_a1_message(
                    &endpoint,
                    &test_node(),
                    EncodedA1Message::from_message_for_call(
                        &cdma_ios::Message::new(
                            cdma_ios::MessageType::PagingResponse,
                            response.encode().unwrap(),
                        ),
                        Some(call_id.0),
                    ),
                )
                .await;
            let reply = timeout(RESPONSE_WAIT, client.poll_a1())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(
                reply.message_type(),
                if correct_esn {
                    cdma_ios::MessageType::AssignmentRequest
                } else {
                    cdma_ios::MessageType::ClearCommand
                }
            );
        }
    }
}

#[tokio::test]
async fn otapa_starts_when_learned_meid_is_not_in_the_subscriber_record() {
    let mut repo = M2mHlrRepo::new();
    repo.esn = Some(AUTH_TEST_ESN);
    repo.strict_hardware_lookup = true;
    let hlr = Arc::new(repo);
    let cfg = crate::config::OtaspConfig {
        enabled: true,
        ..Default::default()
    };
    let mut coordinator =
        crate::otasp::OtaspCoordinator::new(cfg.clone(), cfg.home_network.clone(), hlr.clone());
    let (client, endpoint) = cdma_bsc_a1_edge_compat::InProcessMscClient::pair(32);
    coordinator
        .begin_session_with_initiation(
            crate::otasp::session::SessionInitiation::Network,
            crate::otasp::HardwareIdentity {
                esn: hlr.esn,
                meid: Some(PROVISIONING_TEST_MEID.into()),
            },
            String::new(),
            SERVICE_OPTION_OTASP,
            MobileIdentity::Imsi(hlr.imsi.into()),
            Some(false),
            1,
            &endpoint,
        )
        .await;
    let reply = timeout(RESPONSE_WAIT, client.poll_a1())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(reply.message_type(), cdma_ios::MessageType::AddsDeliver);
}
