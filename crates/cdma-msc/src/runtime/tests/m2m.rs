use super::*;

const A1_WAIT: Duration = Duration::from_secs(1);
const RINGBACK_SIGNAL: u8 = 0x01;
const CALLER_NUMBER: &str = "5551234567";

async fn receive(client: &cdma_bsc_a1_edge_compat::InProcessMscClient) -> EncodedA1Message {
    timeout(A1_WAIT, client.poll_a1())
        .await
        .expect("MSC A1 response")
        .expect("A1 transport remains open")
}

async fn assert_no_message(client: &cdma_bsc_a1_edge_compat::InProcessMscClient) {
    assert!(matches!(
        client.inbound_rx.lock().await.try_recv(),
        Err(tokio::sync::mpsc::error::TryRecvError::Empty)
    ));
}

fn page_response_for(setup: &M2mSetup) -> EncodedA1Message {
    let mut response = paging_response();
    response.tag = setup.page.tag;
    response.mobile_identity_imsi = setup.page.mobile_identity_imsi.clone();
    EncodedA1Message::from_message_for_call(
        &cdma_ios::Message::new(
            cdma_ios::MessageType::PagingResponse,
            response.encode().unwrap(),
        ),
        Some(setup.callee.0),
    )
}

fn connect(call_id: CallId) -> EncodedA1Message {
    EncodedA1Message::from_message_for_call(
        &cdma_ios::Message::new(
            cdma_ios::MessageType::Connect,
            ConnectMessage.encode().unwrap(),
        ),
        Some(call_id.0),
    )
}

fn clear_request(call_id: CallId, cause: u8) -> EncodedA1Message {
    EncodedA1Message::from_message_for_call(
        &cdma_ios::Message::new(
            cdma_ios::MessageType::ClearRequest,
            cdma_ios::ClearRequestMessage {
                cause: Cause(cause),
                cause_layer3: None,
            }
            .encode()
            .unwrap(),
        ),
        Some(call_id.0),
    )
}

async fn assign_callee(
    runtime: &mut MscRuntime,
    endpoint: &dyn MscA1Endpoint,
    client: &cdma_bsc_a1_edge_compat::InProcessMscClient,
    setup: &M2mSetup,
) -> AssignmentRequestMessage {
    runtime
        .handle_bsc_a1_message(endpoint, &test_node(), page_response_for(setup))
        .await;
    let assignment = receive(client).await;
    assert_eq!(
        assignment.message_type(),
        cdma_ios::MessageType::AssignmentRequest
    );
    assert_eq!(assignment.call_id(), Some(setup.callee.0));
    AssignmentRequestMessage::decode(&assignment.decode().unwrap().payload).unwrap()
}

#[tokio::test]
async fn ringback_and_caller_id_follow_the_correct_context_in_either_completion_order() {
    for caller_first in [true, false] {
        let (client, endpoint) = cdma_bsc_a1_edge_compat::InProcessMscClient::pair(8);
        let mut runtime = m2m_runtime(Arc::new(M2mHlrRepo::new()));
        runtime.config.send_tones_alert = true;
        let setup = begin_m2m_setup(&mut runtime, &endpoint, &client).await;
        runtime
            .mt_call
            .mt_plans
            .get_mut(&setup.page.tag.unwrap().0)
            .unwrap()
            .caller_number = Some(CALLER_NUMBER.to_string());
        let assignment = assign_callee(&mut runtime, &endpoint, &client, &setup).await;
        assert!(assignment.ms_information_records.is_some());
        let order = if caller_first {
            [setup.caller, setup.callee]
        } else {
            [setup.callee, setup.caller]
        };
        for leg in order {
            runtime
                .handle_bsc_a1_message(&endpoint, &test_node(), m2m_assignment_complete(leg))
                .await;
            if leg == setup.caller {
                let progress = receive(&client).await;
                assert_eq!(progress.call_id(), Some(setup.caller.0));
                let progress =
                    cdma_ios::ProgressMessage::decode(&progress.decode().unwrap().payload).unwrap();
                assert_eq!(
                    progress.signal,
                    Some(cdma_ios::Signal {
                        signal_value: RINGBACK_SIGNAL,
                        alert_pitch: 0
                    })
                );
            }
            if leg == order[0] {
                assert_no_message(&client).await;
            }
        }
        let alert = receive(&client).await;
        assert_eq!(alert.call_id(), Some(setup.callee.0));
        let alert = cdma_ios::AlertWithInformationMessage::decode(&alert.decode().unwrap().payload)
            .unwrap();
        assert_eq!(
            alert.ms_information_records,
            assignment.ms_information_records
        );
        for leg in order {
            runtime
                .handle_bsc_a1_message(&endpoint, &test_node(), m2m_assignment_complete(leg))
                .await;
            runtime
                .handle_bsc_a1_message(&endpoint, &test_node(), assignment_failure_msg(leg.0))
                .await;
        }
        runtime
            .handle_bsc_a1_message(&endpoint, &test_node(), page_response_for(&setup))
            .await;
        assert_no_message(&client).await;
        assert_eq!(runtime.circuits.circuits.len(), 2);
    }
}

#[tokio::test]
async fn callee_assignment_retry_preserves_the_pending_caller_and_relinks_the_new_circuit() {
    let (client, endpoint) = cdma_bsc_a1_edge_compat::InProcessMscClient::pair(8);
    let mut runtime = m2m_runtime(Arc::new(M2mHlrRepo::new()));
    let setup = begin_m2m_setup(&mut runtime, &endpoint, &client).await;
    let first = assign_callee(&mut runtime, &endpoint, &client, &setup).await;
    runtime
        .handle_bsc_a1_message(
            &endpoint,
            &test_node(),
            assignment_failure_msg(setup.callee.0),
        )
        .await;
    let page = receive(&client).await;
    assert_eq!(page.message_type(), cdma_ios::MessageType::PagingRequest);
    assert_eq!(page.call_id(), Some(setup.callee.0));
    assert_eq!(
        PagingRequestMessage::decode(&page.decode().unwrap().payload).unwrap(),
        setup.page
    );
    assert!(
        runtime
            .circuits
            .has_pending_assignment_complete(setup.caller)
    );
    assert!(
        !runtime
            .circuits
            .circuits
            .contains_key(&first.circuit_identity_code.to_packed())
    );
    let second = assign_callee(&mut runtime, &endpoint, &client, &setup).await;
    let new_circuit = second.circuit_identity_code.to_packed();
    assert_ne!(first.circuit_identity_code, second.circuit_identity_code);
    assert_eq!(
        runtime.circuits.circuits[&setup.caller_circuit].peer_circuit_id,
        Some(new_circuit)
    );
    for leg in [setup.callee, setup.caller] {
        runtime
            .handle_bsc_a1_message(&endpoint, &test_node(), m2m_assignment_complete(leg))
            .await;
    }
    let alert = receive(&client).await;
    assert_eq!(
        alert.message_type(),
        cdma_ios::MessageType::AlertWithInformation
    );
    assert_eq!(alert.call_id(), Some(setup.callee.0));
    assert_no_message(&client).await;
}

#[derive(Clone, Copy, Debug)]
enum SetupStage {
    Paging,
    Assigning,
    CallerReady,
    CalleeReady,
    Alerting,
    Connected,
}

#[tokio::test]
async fn either_mobile_can_clear_at_every_setup_stage_without_late_messages_reviving_the_call() {
    const NORMAL_RELEASE: u8 = 0;
    for stage in [
        SetupStage::Paging,
        SetupStage::Assigning,
        SetupStage::CallerReady,
        SetupStage::CalleeReady,
        SetupStage::Alerting,
        SetupStage::Connected,
    ] {
        for caller_releases in [true, false] {
            let (client, endpoint) = cdma_bsc_a1_edge_compat::InProcessMscClient::pair(8);
            let hlr = Arc::new(M2mHlrRepo::new());
            let mut runtime = m2m_runtime(hlr.clone());
            let setup = begin_m2m_setup(&mut runtime, &endpoint, &client).await;
            if !matches!(stage, SetupStage::Paging) {
                assign_callee(&mut runtime, &endpoint, &client, &setup).await;
            }
            if matches!(
                stage,
                SetupStage::CallerReady | SetupStage::Alerting | SetupStage::Connected
            ) {
                runtime
                    .handle_bsc_a1_message(
                        &endpoint,
                        &test_node(),
                        m2m_assignment_complete(setup.caller),
                    )
                    .await;
            }
            if matches!(
                stage,
                SetupStage::CalleeReady | SetupStage::Alerting | SetupStage::Connected
            ) {
                runtime
                    .handle_bsc_a1_message(
                        &endpoint,
                        &test_node(),
                        m2m_assignment_complete(setup.callee),
                    )
                    .await;
            }
            if matches!(stage, SetupStage::Alerting | SetupStage::Connected) {
                assert_eq!(
                    receive(&client).await.message_type(),
                    cdma_ios::MessageType::AlertWithInformation
                );
            }
            if matches!(stage, SetupStage::Connected) {
                runtime
                    .handle_bsc_a1_message(&endpoint, &test_node(), connect(setup.callee))
                    .await;
                assert_eq!(
                    receive(&client).await.message_type(),
                    cdma_ios::MessageType::Progress
                );
            }
            let releasing = if caller_releases {
                setup.caller
            } else {
                setup.callee
            };
            runtime
                .handle_bsc_a1_message(
                    &endpoint,
                    &test_node(),
                    clear_request(releasing, NORMAL_RELEASE),
                )
                .await;
            for leg in [setup.caller, setup.callee] {
                let clear = receive(&client).await;
                assert_eq!(
                    clear.message_type(),
                    cdma_ios::MessageType::ClearCommand,
                    "{stage:?}"
                );
                assert_eq!(clear.call_id(), Some(leg.0));
                assert!(runtime.controller.snapshot(leg).is_none());
                assert!(!runtime.mt_page_retry.contains(leg));
            }
            assert!(!runtime.media_gw.is_subscriber_busy(hlr.subscriber_id));
            for message in [
                page_response_for(&setup),
                m2m_assignment_complete(setup.caller),
                m2m_assignment_complete(setup.callee),
                connect(setup.callee),
                assignment_failure_msg(setup.callee.0),
            ] {
                runtime
                    .handle_bsc_a1_message(&endpoint, &test_node(), message)
                    .await;
            }
            assert_no_message(&client).await;
            assert!(runtime.circuits.m2m_calls.is_empty());
            assert!(runtime.circuits.circuits.is_empty());
            assert!(runtime.circuits.mt_assignment_failure_retries.is_empty());
            assert!(runtime.mt_call.mt_plans.is_empty());
            let next = begin_m2m_setup(&mut runtime, &endpoint, &client).await;
            assert_ne!(next.callee, setup.callee);
        }
    }
}

#[tokio::test]
async fn paging_timeout_exhaustion_clears_the_caller_too() {
    let (client, endpoint) = cdma_bsc_a1_edge_compat::InProcessMscClient::pair(8);
    let mut runtime = m2m_runtime(Arc::new(M2mHlrRepo::new()));
    runtime.mt_page_retry = crate::mt_page_retry::MtPageRetryService::new(0, 0);
    let setup = begin_m2m_setup(&mut runtime, &endpoint, &client).await;
    runtime
        .handle_bsc_a1_message(
            &endpoint,
            &test_node(),
            clear_request(
                setup.callee,
                crate::mt_page_retry::A1_CAUSE_PAGE_RESP_TIMEOUT,
            ),
        )
        .await;
    for leg in [setup.caller, setup.callee] {
        let clear = receive(&client).await;
        assert_eq!(clear.message_type(), cdma_ios::MessageType::ClearCommand);
        assert_eq!(clear.call_id(), Some(leg.0));
        assert!(runtime.controller.snapshot(leg).is_none());
    }
    assert_no_message(&client).await;
}

#[tokio::test]
async fn a_base_station_disconnect_clears_both_mobiles() {
    for caller_disconnects in [true, false] {
        let (client, endpoint) = cdma_bsc_a1_edge_compat::InProcessMscClient::pair(8);
        let mut runtime = m2m_runtime(Arc::new(M2mHlrRepo::new()));
        let setup = begin_m2m_setup(&mut runtime, &endpoint, &client).await;
        assign_callee(&mut runtime, &endpoint, &client, &setup).await;
        let disconnected = if caller_disconnects {
            setup.caller
        } else {
            setup.callee
        };
        runtime
            .handle_base_station_detached(&endpoint, &test_node(), vec![disconnected])
            .await;
        for leg in [setup.caller, setup.callee] {
            let clear = receive(&client).await;
            assert_eq!(clear.message_type(), cdma_ios::MessageType::ClearCommand);
            assert_eq!(clear.call_id(), Some(leg.0));
            assert!(runtime.controller.snapshot(leg).is_none());
        }
        assert_no_message(&client).await;
    }
}

#[tokio::test]
async fn external_origination_starts_one_sip_call_only_after_assignment_completes() {
    let (client, endpoint) = cdma_bsc_a1_edge_compat::InProcessMscClient::pair(8);
    let mut runtime = m2m_runtime(Arc::new(M2mHlrRepo::new()));
    let gateway = Arc::new(StubMediaGateway::default());
    runtime.config.media_gateway = Some(gateway.clone());
    let caller = CallId(4242);
    let cli3 = cm_service_request_cli3(Some(vec![0x81, 0x00, 0x00, 0x00, 0x00, 0x00]));
    runtime
        .handle_bsc_a1_message(
            &endpoint,
            &test_node(),
            EncodedA1Message::from_message_for_call(
                &cdma_ios::Message::new(
                    cdma_ios::MessageType::CompleteLayer3Information,
                    cli3.encode().unwrap(),
                ),
                Some(caller.0),
            ),
        )
        .await;
    assert_eq!(
        receive(&client).await.message_type(),
        cdma_ios::MessageType::AssignmentRequest
    );
    assert!(gateway.created.lock().unwrap().is_empty());
    assert_no_message(&client).await;
    for _ in 0..2 {
        runtime
            .handle_bsc_a1_message(&endpoint, &test_node(), m2m_assignment_complete(caller))
            .await;
    }
    {
        let requests = gateway.created.lock().unwrap();
        assert_eq!(requests.len(), 1);
        assert_eq!(requests[0].call_id, caller.0);
        assert_eq!(requests[0].called_party.as_deref(), Some("0000000000"));
    }
    assert!(runtime.circuits.m2m_calls.is_empty());
    assert_no_message(&client).await;
}

#[tokio::test]
async fn inbound_sip_still_pages_alerts_and_answers_one_mobile() {
    const SESSION: &str = "inbound-call";
    const CODEC: &str = "PCMU";
    let (client, endpoint) = cdma_bsc_a1_edge_compat::InProcessMscClient::pair(8);
    let hlr = Arc::new(M2mHlrRepo::new());
    let mut runtime = m2m_runtime(hlr.clone());
    let gateway = Arc::new(RecordingInboundGateway::default());
    runtime.config.media_gateway = Some(gateway.clone());
    runtime
        .handle_inbound_sip_invite(
            &endpoint,
            SESSION.to_string(),
            hlr.phone_number.to_string(),
            CALLER_NUMBER.to_string(),
            vec![CODEC.to_string()],
        )
        .await;
    let page = receive(&client).await;
    assert_eq!(page.message_type(), cdma_ios::MessageType::PagingRequest);
    let callee = CallId(page.call_id().unwrap());
    let mut response = paging_response();
    response.tag = PagingRequestMessage::decode(&page.decode().unwrap().payload)
        .unwrap()
        .tag;
    runtime
        .handle_bsc_a1_message(
            &endpoint,
            &test_node(),
            EncodedA1Message::from_message_for_call(
                &cdma_ios::Message::new(
                    cdma_ios::MessageType::PagingResponse,
                    response.encode().unwrap(),
                ),
                Some(callee.0),
            ),
        )
        .await;
    assert_eq!(
        receive(&client).await.message_type(),
        cdma_ios::MessageType::AssignmentRequest
    );
    runtime
        .handle_bsc_a1_message(&endpoint, &test_node(), m2m_assignment_complete(callee))
        .await;
    let alert = receive(&client).await;
    assert_eq!(
        alert.message_type(),
        cdma_ios::MessageType::AlertWithInformation
    );
    assert_eq!(alert.call_id(), Some(callee.0));
    assert_eq!(*gateway.progress.lock().unwrap(), vec![SESSION]);
    assert!(gateway.answer.lock().unwrap().is_empty());
    runtime
        .handle_bsc_a1_message(&endpoint, &test_node(), connect(callee))
        .await;
    assert_eq!(
        receive(&client).await.message_type(),
        cdma_ios::MessageType::Progress
    );
    assert_eq!(
        *gateway.answer.lock().unwrap(),
        vec![(SESSION.to_string(), CODEC.to_string())]
    );
    assert_eq!(
        runtime.controller.state(callee),
        Some(cdma_ios::CallControlState::Connected)
    );
    assert!(runtime.circuits.m2m_calls.is_empty());
    assert_no_message(&client).await;
}

fn completion_with_bearer(call_id: CallId, remote: std::net::SocketAddr) -> EncodedA1Message {
    let mut complete = AssignmentCompleteMessage::decode(
        &m2m_assignment_complete(call_id).decode().unwrap().payload,
    )
    .unwrap();
    complete.a2p_bearer_session_params = Some(cdma_ios::A2pBearerSessionParams {
        ip_address: std::net::Ipv4Addr::LOCALHOST,
        udp_port: remote.port(),
    });
    EncodedA1Message::from_message_for_call(
        &cdma_ios::Message::new(
            cdma_ios::MessageType::AssignmentComplete,
            complete.encode().unwrap(),
        ),
        Some(call_id.0),
    )
}

#[tokio::test]
async fn independent_bearers_stay_silent_until_answer_and_route_both_directions() {
    const SILENCE_WAIT: Duration = Duration::from_millis(20);
    for caller_first in [true, false] {
        let (client, endpoint) = cdma_bsc_a1_edge_compat::InProcessMscClient::pair(8);
        let mut runtime = m2m_runtime(Arc::new(M2mHlrRepo::new()));
        runtime.config.voice_bearer = Some(Arc::new(VoiceBearerManager::new(
            std::net::Ipv4Addr::LOCALHOST,
        )));
        let setup = begin_m2m_setup(&mut runtime, &endpoint, &client).await;
        let assignment = assign_callee(&mut runtime, &endpoint, &client, &setup).await;
        let callee_circuit = assignment.circuit_identity_code.to_packed();
        let receiver = VoiceBearerManager::new(std::net::Ipv4Addr::LOCALHOST);
        let caller_remote = receiver
            .open_circuit(setup.caller_circuit, None)
            .await
            .unwrap();
        let callee_remote = receiver.open_circuit(callee_circuit, None).await.unwrap();
        for circuit in [setup.caller_circuit, callee_circuit] {
            receiver.set_circuit_payload_types(
                circuit,
                cdma_ios::BearerPayloadTypes {
                    voice: cdma_ios::VoiceBearerPayloadType {
                        format: cdma_ios::VoiceBearerFormat::Evrc,
                        payload_type: cdma_ios::voice_bearer::VOICE_RTP_PAYLOAD_TYPE,
                    },
                    telephone_event: None,
                },
            );
        }
        let mut encoder = cdma_voice::VoiceEncoder::new(cdma_voice::VoiceCodec::EvrcA).unwrap();
        let (rate, payload) = encoder.encode(&[0; cdma_voice::SAMPLES_PER_FRAME]).unwrap();
        let frames = [setup.caller_circuit, callee_circuit].map(|circuit_id| VoiceBearerFrame {
            circuit_id,
            rate_bps: cdma_voice::VoiceCodec::EvrcA.rate_bps(rate),
            payload: payload.clone(),
        });
        let completions = if caller_first {
            [(setup.caller, caller_remote), (setup.callee, callee_remote)]
        } else {
            [(setup.callee, callee_remote), (setup.caller, caller_remote)]
        };
        for (leg, remote) in completions {
            runtime
                .handle_bsc_a1_message(&endpoint, &test_node(), completion_with_bearer(leg, remote))
                .await;
            for frame in &frames {
                runtime.handle_reverse_bearer_frame(frame.clone()).await;
            }
            assert!(timeout(SILENCE_WAIT, receiver.recv()).await.is_err());
        }
        assert_eq!(
            receive(&client).await.message_type(),
            cdma_ios::MessageType::AlertWithInformation
        );
        runtime
            .handle_bsc_a1_message(&endpoint, &test_node(), connect(setup.callee))
            .await;
        assert_eq!(
            receive(&client).await.message_type(),
            cdma_ios::MessageType::Progress
        );
        for (frame, peer) in frames.iter().zip([callee_circuit, setup.caller_circuit]) {
            runtime.handle_reverse_bearer_frame(frame.clone()).await;
            let event = timeout(A1_WAIT, receiver.recv()).await.unwrap().unwrap();
            let cdma_ios::BearerEvent::Voice(received) = event else {
                panic!("expected voice")
            };
            assert_eq!(received.circuit_id, peer);
            assert_eq!(received.payload, frame.payload);
            assert_eq!(received.rate_bps, frame.rate_bps);
        }
        runtime.send_clear_command(&endpoint, setup.caller).await;
        for frame in frames {
            runtime.handle_reverse_bearer_frame(frame).await;
        }
        assert!(timeout(SILENCE_WAIT, receiver.recv()).await.is_err());
    }
}

#[tokio::test]
async fn busy_and_stale_destinations_clear_the_caller_without_paging_or_sip_fallback() {
    use cdma_hlr::model::RegistrationState;
    const EXISTING_CALL: CallId = CallId(100);
    const ORIGINATING_CALL: CallId = CallId(4242);
    for state in [RegistrationState::Registered, RegistrationState::Stale] {
        let (client, endpoint) = cdma_bsc_a1_edge_compat::InProcessMscClient::pair(8);
        let mut hlr = M2mHlrRepo::new();
        hlr.registration_state = state.clone();
        let hlr = Arc::new(hlr);
        let mut runtime = m2m_runtime(hlr.clone());
        let gateway = Arc::new(StubMediaGateway::default());
        runtime.config.media_gateway = Some(gateway.clone());
        if state == RegistrationState::Registered {
            runtime
                .media_gw
                .register_active_subscriber(hlr.subscriber_id, EXISTING_CALL);
        }
        let cli3 = cm_service_request_cli3(Some(vec![0x81, 0x55, 0x95, 0x78, 0x56, 0x34]));
        runtime
            .handle_bsc_a1_message(
                &endpoint,
                &test_node(),
                EncodedA1Message::from_message_for_call(
                    &cdma_ios::Message::new(
                        cdma_ios::MessageType::CompleteLayer3Information,
                        cli3.encode().unwrap(),
                    ),
                    Some(ORIGINATING_CALL.0),
                ),
            )
            .await;
        assert_eq!(
            receive(&client).await.message_type(),
            cdma_ios::MessageType::AssignmentRequest
        );
        let clear = receive(&client).await;
        assert_eq!(clear.message_type(), cdma_ios::MessageType::ClearCommand);
        assert_eq!(clear.call_id(), Some(ORIGINATING_CALL.0));
        assert_no_message(&client).await;
        assert!(gateway.created.lock().unwrap().is_empty());
        assert!(runtime.circuits.m2m_calls.is_empty());
        assert!(runtime.mt_call.mt_plans.is_empty());
        assert_eq!(
            runtime.media_gw.is_subscriber_busy(hlr.subscriber_id),
            state == RegistrationState::Registered
        );
    }
}
