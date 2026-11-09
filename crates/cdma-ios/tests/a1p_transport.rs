//! usrsctp endpoints in this process must share one UDP encapsulation port.

use std::net::SocketAddr;
use std::time::Duration;

use cdma_ios::sua::{self, SuaMessage, SuaMessageKind, tag};
use cdma_ios::transport::{
    self, A1Listener, A1Role, A1TransportEvent, A1TransportSender, A1pConfig,
};
use cdma_ios::{
    CellId, ClassmarkInformationType2, ClearCommandMessage, ClearCompleteMessage,
    CmServiceRequestMessage, CmServiceType, CompleteLayer3InformationMessage, EncodedA1Message,
    Layer3Information, LocationUpdatingAcceptMessage, LocationUpdatingRequestMessage, Message,
    MessageType, MobileIdentity, PagingRequestMessage, PagingResponseMessage, Tag,
};
use tokio::sync::mpsc::Receiver;

const RECV_TIMEOUT: Duration = Duration::from_secs(5);
const TEST_RTO_INITIAL_MS: u32 = 200;
const TEST_RTO_MIN_MS: u32 = 200;
const TEST_RTO_MAX_MS: u32 = 1_000;

fn in_process_udp_port() -> u16 {
    static PORT: std::sync::OnceLock<u16> = std::sync::OnceLock::new();
    *PORT.get_or_init(|| {
        std::net::UdpSocket::bind("0.0.0.0:0")
            .and_then(|socket| socket.local_addr())
            .expect("a free UDP port")
            .port()
    })
}

fn in_process(mut config: A1pConfig) -> A1pConfig {
    config.sctp.fallback_udp_encapsulation.local_port = in_process_udp_port();
    config.sctp.fallback_udp_encapsulation.remote_port = in_process_udp_port();
    config.sctp.rto_initial_ms = Some(TEST_RTO_INITIAL_MS);
    config.sctp.rto_min_ms = Some(TEST_RTO_MIN_MS);
    config.sctp.rto_max_ms = Some(TEST_RTO_MAX_MS);
    config
}
const IMSI: &str = "310009176936269";

fn cell() -> CellId {
    CellId {
        cell: 0x123,
        sector: 1,
    }
}

fn classmark() -> ClassmarkInformationType2 {
    ClassmarkInformationType2(vec![
        0xc1, 0x00, 0x66, 0x00, 0x03, 0x01, 0x31, 0x01, 0x03, 0x00, 0x01, 0x06,
    ])
}

fn encoded(message_type: MessageType, payload: Vec<u8>, call_id: Option<u64>) -> EncodedA1Message {
    EncodedA1Message::from_message_for_call(&Message::new(message_type, payload), call_id)
}

fn cm_service_request() -> Vec<u8> {
    let dtap = CmServiceRequestMessage {
        cm_service_type: CmServiceType::MobileOriginatingCallEstablishment,
        classmark_information_type_2: classmark(),
        mobile_identity_imsi: MobileIdentity::Imsi(IMSI.into()),
        called_party_bcd_number: None,
        tag: None,
        mobile_identity_esn: None,
        mobile_identity_meid: None,
        slot_cycle_index: None,
        authentication_response_parameter: None,
        authentication_confirmation_parameter: None,
        authentication_parameter_count: None,
        authentication_challenge_parameter: None,
        service_option: None,
        voice_privacy_request: false,
        radio_environment_and_resources: None,
        called_party_ascii_number: None,
        circuit_identity_code: None,
        cdma_serving_one_way_delay: None,
        authentication_event: None,
        authentication_data: None,
        paca_reorigination_indicator: false,
        user_zone_id: None,
        is2000_mobile_capabilities: None,
    };
    CompleteLayer3InformationMessage {
        cell_identifier: cell(),
        layer3_information: Layer3Information::from_cm_service_request(&dtap).unwrap(),
    }
    .encode()
    .unwrap()
}

fn paging_response(tag: u32) -> Vec<u8> {
    let response = PagingResponseMessage {
        classmark_information_type_2: classmark(),
        mobile_identity_imsi: MobileIdentity::Imsi(IMSI.into()),
        tag: Some(Tag(tag)),
        mobile_identity_esn: None,
        mobile_identity_meid: None,
        slot_cycle_index: None,
        authentication_response_parameter: None,
        authentication_confirmation_parameter: None,
        authentication_parameter_count: None,
        authentication_challenge_parameter: None,
        service_option: None,
        voice_privacy_request: false,
        circuit_identity_code: None,
        cdma_serving_one_way_delay: None,
        authentication_event: None,
        radio_environment_and_resources: None,
        user_zone_id: None,
        is2000_mobile_capabilities: None,
    };
    CompleteLayer3InformationMessage {
        cell_identifier: cell(),
        layer3_information: Layer3Information::from_paging_response(&response).unwrap(),
    }
    .encode()
    .unwrap()
}

fn location_updating_request() -> Vec<u8> {
    let request = LocationUpdatingRequestMessage {
        mobile_identity_imsi: MobileIdentity::Imsi(IMSI.into()),
        location_area_identification: None,
        classmark_information_type_2: None,
        registration_type: None,
        mobile_identity_esn: None,
        mobile_identity_meid: None,
        slot_cycle_index: None,
        authentication_response_parameter: None,
        authentication_confirmation_parameter: None,
        authentication_parameter_count: None,
        authentication_challenge_parameter: None,
        authentication_event: None,
        user_zone_id: None,
        is2000_mobile_capabilities: None,
    };
    CompleteLayer3InformationMessage {
        cell_identifier: cell(),
        layer3_information: Layer3Information::from_location_updating_request(&request).unwrap(),
    }
    .encode()
    .unwrap()
}

struct Pair {
    bs: A1TransportSender,
    bs_rx: Receiver<A1TransportEvent>,
    msc: A1TransportSender,
    msc_rx: Receiver<A1TransportEvent>,
}

/// A base station listening and an MSC dialing it, as in a deployment.
async fn pair() -> Option<Pair> {
    let listener = match A1Listener::bind(
        &["127.0.0.1:0".parse().unwrap()],
        in_process(A1pConfig::for_role(A1Role::BaseStation)),
        A1Role::BaseStation,
    ) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("skipping: {error}");
            return None;
        }
    };
    let addr = listener.local_addr().unwrap();
    let msc_config = in_process(A1pConfig::for_role(A1Role::Msc));
    let (accepted, connected) = tokio::join!(
        transport::accept(&listener),
        transport::connect(addr, &msc_config, A1Role::Msc)
    );
    let (bs, bs_rx) = accepted.unwrap();
    let (msc, msc_rx) = connected.unwrap();
    Some(Pair {
        bs,
        bs_rx,
        msc,
        msc_rx,
    })
}

async fn next(rx: &mut Receiver<A1TransportEvent>) -> EncodedA1Message {
    match tokio::time::timeout(RECV_TIMEOUT, rx.recv()).await.unwrap() {
        Some(A1TransportEvent::Message(message)) => message,
        other => panic!("expected an A1 message, got {other:?}"),
    }
}

#[tokio::test]
async fn mobile_originated_call_rides_one_sua_connection() {
    let Some(mut p) = pair().await else {
        return;
    };
    let bs_call = 5;
    p.bs.send(&encoded(
        MessageType::CompleteLayer3Information,
        cm_service_request(),
        Some(bs_call),
    ))
    .await
    .unwrap();
    let cli3 = next(&mut p.msc_rx).await;
    assert_eq!(cli3.message_type(), MessageType::CompleteLayer3Information);
    let msc_call = cli3.call_id().expect("CORE carries a connection");

    p.msc
        .send(&encoded(
            MessageType::ClearCommand,
            ClearCommandMessage {
                cause: cdma_ios::Cause(0x09),
                cause_layer3: None,
            }
            .encode()
            .unwrap(),
            Some(msc_call),
        ))
        .await
        .unwrap();
    let clear = next(&mut p.bs_rx).await;
    assert_eq!(clear.message_type(), MessageType::ClearCommand);
    assert_eq!(clear.call_id(), Some(bs_call));

    p.bs.send(&encoded(
        MessageType::ClearComplete,
        ClearCompleteMessage {
            power_down_indicator: false,
        }
        .encode()
        .unwrap(),
        Some(bs_call),
    ))
    .await
    .unwrap();
    let complete = next(&mut p.msc_rx).await;
    assert_eq!(complete.message_type(), MessageType::ClearComplete);
    assert_eq!(complete.call_id(), Some(msc_call));

    // The MSC released the connection, so the base station's next
    // origination opens a new one under a fresh MSC call id.
    tokio::time::sleep(Duration::from_millis(100)).await;
    p.bs.send(&encoded(
        MessageType::CompleteLayer3Information,
        cm_service_request(),
        Some(bs_call),
    ))
    .await
    .unwrap();
    let again = next(&mut p.msc_rx).await;
    assert_ne!(again.call_id(), Some(msc_call));
}

#[tokio::test]
async fn paging_response_takes_the_call_of_the_page_it_answers() {
    let Some(mut p) = pair().await else {
        return;
    };
    let msc_call = 77;
    let page_tag = 0x0102_0304;
    p.msc
        .send(&encoded(
            MessageType::PagingRequest,
            PagingRequestMessage {
                mobile_identity_imsi: MobileIdentity::Imsi(IMSI.into()),
                mobile_identity_meid: None,
                tag: Some(Tag(page_tag)),
                cell_identifier_list: None,
                slot_cycle_index: None,
                service_option: None,
                is2000_mobile_capabilities: None,
            }
            .encode()
            .unwrap(),
            Some(msc_call),
        ))
        .await
        .unwrap();
    let page = next(&mut p.bs_rx).await;
    assert_eq!(page.message_type(), MessageType::PagingRequest);
    assert_eq!(page.call_id(), None, "a page is connectionless");

    p.bs.send(&encoded(
        MessageType::PagingResponse,
        paging_response(page_tag),
        Some(9),
    ))
    .await
    .unwrap();
    let response = next(&mut p.msc_rx).await;
    assert_eq!(response.message_type(), MessageType::PagingResponse);
    assert_eq!(response.call_id(), Some(msc_call));
}

#[tokio::test]
async fn location_updating_accept_refuses_the_registration_connection() {
    let Some(mut p) = pair().await else {
        return;
    };
    p.bs.send(&encoded(
        MessageType::CompleteLayer3Information,
        location_updating_request(),
        Some(3),
    ))
    .await
    .unwrap();
    let request = next(&mut p.msc_rx).await;
    p.msc
        .send(&encoded(
            MessageType::LocationUpdatingAccept,
            LocationUpdatingAcceptMessage {
                location_area_identification: None,
            }
            .encode()
            .unwrap(),
            request.call_id(),
        ))
        .await
        .unwrap();
    let accept = next(&mut p.bs_rx).await;
    assert_eq!(accept.message_type(), MessageType::LocationUpdatingAccept);
    assert_eq!(accept.call_id(), Some(3));
}

#[tokio::test]
async fn raw_sua_peer_completes_ipsp_handshake_and_exchanges_cldt() {
    let config = in_process(A1pConfig::for_role(A1Role::BaseStation));
    let listener = match A1Listener::bind(
        &["127.0.0.1:0".parse().unwrap()],
        config.clone(),
        A1Role::BaseStation,
    ) {
        Ok(listener) => listener,
        Err(error) => {
            eprintln!("skipping: {error}");
            return;
        }
    };
    let addr: SocketAddr = listener.local_addr().unwrap();
    let accept = tokio::spawn(async move {
        let (sender, rx) = transport::accept(&listener).await.unwrap();
        (sender, rx, listener)
    });
    let raw = cdma_sctp::SctpAssociation::connect(&[addr], &[], &config.sctp)
        .await
        .unwrap();
    let send = |message: SuaMessage, stream_id: u16| {
        let bytes = message.encode();
        let raw = &raw;
        async move {
            raw.send(
                &bytes,
                cdma_sctp::SendInfo {
                    stream_id,
                    ppid: sua::SUA_PAYLOAD_PROTOCOL_ID,
                    ..cdma_sctp::SendInfo::default()
                },
            )
            .await
            .unwrap()
        }
    };
    send(SuaMessage::new(SuaMessageKind::AspUp), 0).await;
    let up_ack = raw.recv().await.unwrap();
    assert_eq!(up_ack.ppid, sua::SUA_PAYLOAD_PROTOCOL_ID);
    assert_eq!(up_ack.stream_id, 0);
    assert_eq!(&up_ack.data[..4], &[1, 0, 3, 4]);
    send(
        SuaMessage::new(SuaMessageKind::AspActive)
            .with_u32(tag::TRAFFIC_MODE_TYPE, sua::TRAFFIC_MODE_OVERRIDE)
            .with_u32(tag::ROUTING_CONTEXT, config.routing_context),
        0,
    )
    .await;
    let active_ack = SuaMessage::decode(&raw.recv().await.unwrap().data).unwrap();
    assert_eq!(active_ack.kind, SuaMessageKind::AspActiveAck);
    let (bs, mut bs_rx, _listener) = accept.await.unwrap();

    let paging = PagingRequestMessage {
        mobile_identity_imsi: MobileIdentity::Imsi(IMSI.into()),
        mobile_identity_meid: None,
        tag: Some(Tag(1)),
        cell_identifier_list: None,
        slot_cycle_index: None,
        service_option: None,
        is2000_mobile_capabilities: None,
    }
    .encode()
    .unwrap();
    let msc_address = sua::SuaAddress {
        point_code: config.peer_point_code,
        ssn: sua::BSAP_SSN,
    };
    send(
        SuaMessage::new(SuaMessageKind::ConnectionlessData)
            .with_u32(tag::ROUTING_CONTEXT, config.routing_context)
            .with_u32(tag::PROTOCOL_CLASS, sua::PROTOCOL_CLASS_0)
            .with_address(tag::SOURCE_ADDRESS, msc_address)
            .with_address(
                tag::DESTINATION_ADDRESS,
                sua::SuaAddress {
                    point_code: config.local_point_code,
                    ssn: sua::BSAP_SSN,
                },
            )
            .with_u32(tag::SEQUENCE_CONTROL, 0)
            .with(tag::DATA, paging.clone()),
        1,
    )
    .await;
    let page = next(&mut bs_rx).await;
    assert_eq!(page.message_type(), MessageType::PagingRequest);
    assert_eq!(page.as_bytes(), paging.as_slice());

    bs.send(&encoded(
        MessageType::CompleteLayer3Information,
        cm_service_request(),
        Some(11),
    ))
    .await
    .unwrap();
    let core = raw.recv().await.unwrap();
    assert_ne!(core.stream_id, 0, "data rides a non-zero stream");
    let core = SuaMessage::decode(&core.data).unwrap();
    assert_eq!(core.kind, SuaMessageKind::ConnectionRequest);
    assert_eq!(
        core.u32_parameter(tag::PROTOCOL_CLASS),
        Some(sua::PROTOCOL_CLASS_2)
    );
    assert_eq!(core.address(tag::DESTINATION_ADDRESS), Some(msc_address));
    assert!(core.u32_parameter(tag::SOURCE_REFERENCE_NUMBER).is_some());
    assert_eq!(core.data(), Some(cm_service_request().as_slice()));
}
