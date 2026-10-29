use super::*;

const SMS_ROUTE_WAIT: Duration = Duration::from_secs(1);
const SMS_ROUTE_TEST_TEXT: &str = "routing regression";

fn sms_route_runtime() -> MscRuntime {
    let mut runtime = authenticating_runtime(crate::authentication::AuthenticationMode::Enforce);
    let smsc =
        cdma_smsc::repository::GrpcSmscRepository::connect_lazy("http://127.0.0.1:1").unwrap();
    runtime.smsc = Some(crate::sms::MscSmsCoordinator::new(
        Arc::new(smsc),
        runtime.config.hlr_repo.clone(),
    ));
    runtime
}

fn sms_route_command() -> cdma_smsc::proto::DeliveryCommand {
    cdma_smsc::proto::DeliveryCommand {
        sms_id: uuid::Uuid::new_v4().to_string(),
        attempt_id: uuid::Uuid::new_v4().to_string(),
        target: Some(cdma_smsc::proto::DeliveryTarget {
            identity: Some(cdma_smsc::proto::delivery_target::Identity::Imsi(
                AUTH_TEST_IMSI.to_string(),
            )),
            serving_node: None,
        }),
        originating_number: "2222".to_string(),
        text: SMS_ROUTE_TEST_TEXT.to_string(),
        teleservice_id: None,
        raw_user_data: None,
    }
}

#[tokio::test]
async fn sms_routes_follow_accepted_access_without_a_subscriber_and_clear_on_detach() {
    let (client, endpoint) = cdma_bsc_a1_edge_compat::InProcessMscClient::pair(4);
    let mut runtime = sms_route_runtime();
    let (outcomes, mut results) = tokio::sync::mpsc::channel(4);
    runtime
        .smsc
        .as_mut()
        .unwrap()
        .set_outcome_reporter(outcomes);
    runtime
        .smsc
        .as_mut()
        .unwrap()
        .deliver_from_smsc(sms_route_command(), &endpoint)
        .await;
    assert_eq!(
        results.recv().await.unwrap().result,
        cdma_smsc::proto::delivery_outcome::Result::NotReachable as i32
    );

    runtime
        .handle_bsc_a1_message(&endpoint, &test_node(), authenticated_registration(Some(0)))
        .await;
    assert_eq!(
        client.poll_a1().await.unwrap().message_type(),
        cdma_ios::MessageType::LocationUpdatingAccept
    );
    for _ in 0..2 {
        runtime
            .smsc
            .as_mut()
            .unwrap()
            .deliver_from_smsc(sms_route_command(), &endpoint)
            .await;
        let page = timeout(SMS_ROUTE_WAIT, client.poll_a1())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(page.message_type(), cdma_ios::MessageType::AddsPage);
        let decoded = cdma_ios::AddsPageMessage::decode(&page.decode().unwrap().payload).unwrap();
        assert_eq!(
            decoded.mobile_identity,
            MobileIdentity::Imsi(AUTH_TEST_IMSI.to_string())
        );
    }
    runtime
        .handle_base_station_detached(&endpoint, &test_node(), vec![])
        .await;
    runtime
        .smsc
        .as_mut()
        .unwrap()
        .deliver_from_smsc(sms_route_command(), &endpoint)
        .await;
    assert_eq!(
        results.recv().await.unwrap().result,
        cdma_smsc::proto::delivery_outcome::Result::NotReachable as i32
    );
    assert!(client.inbound_rx.lock().await.try_recv().is_err());
}

#[tokio::test]
async fn rejected_access_does_not_create_an_sms_route() {
    let (client, endpoint) = cdma_bsc_a1_edge_compat::InProcessMscClient::pair(4);
    let mut runtime = sms_route_runtime();
    let (outcomes, mut results) = tokio::sync::mpsc::channel(4);
    runtime
        .smsc
        .as_mut()
        .unwrap()
        .set_outcome_reporter(outcomes);
    runtime
        .handle_bsc_a1_message(&endpoint, &test_node(), authenticated_registration(Some(1)))
        .await;
    assert_eq!(
        client.poll_a1().await.unwrap().message_type(),
        cdma_ios::MessageType::LocationUpdatingReject
    );
    runtime
        .smsc
        .as_mut()
        .unwrap()
        .deliver_from_smsc(sms_route_command(), &endpoint)
        .await;
    assert_eq!(
        results.recv().await.unwrap().result,
        cdma_smsc::proto::delivery_outcome::Result::NotReachable as i32
    );
    assert!(client.inbound_rx.lock().await.try_recv().is_err());
}

#[tokio::test]
async fn explicit_sms_routes_are_not_replaced_and_detach_keeps_other_nodes() {
    let (client, endpoint) = cdma_bsc_a1_edge_compat::InProcessMscClient::pair(4);
    let mut runtime = sms_route_runtime();
    let coordinator = runtime.smsc.as_mut().unwrap();
    let (outcomes, mut results) = tokio::sync::mpsc::channel(4);
    coordinator.set_outcome_reporter(outcomes);
    let other = BaseStationId("other-bsc".to_string());
    coordinator.record_mobile_contact(AUTH_TEST_IMSI, &other);
    coordinator.record_mobile_contact(AUTH_TEST_IMSI, &test_node());
    coordinator.remove_serving_node(&other);
    let mut explicit = sms_route_command();
    explicit.target.as_mut().unwrap().serving_node = Some(other.0);
    coordinator.deliver_from_smsc(explicit, &endpoint).await;
    assert_eq!(
        results.recv().await.unwrap().result,
        cdma_smsc::proto::delivery_outcome::Result::NotReachable as i32
    );
    coordinator
        .deliver_from_smsc(sms_route_command(), &endpoint)
        .await;
    assert_eq!(
        timeout(SMS_ROUTE_WAIT, client.poll_a1())
            .await
            .unwrap()
            .unwrap()
            .message_type(),
        cdma_ios::MessageType::AddsPage
    );
}
