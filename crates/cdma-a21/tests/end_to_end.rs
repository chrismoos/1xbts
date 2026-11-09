//! A21 messages exchanged over loopback UDP.

use std::time::Duration;

use cdma_a21::{
    A21Cause, A21Endpoint, A21EventKind, A21Message, A21MessageType, MessageTransmissionControl,
    MobileIdentity,
};

const RECV_TIMEOUT: Duration = Duration::from_secs(2);

#[tokio::test]
async fn iws_page_is_acked_from_the_port_it_was_sent_to() {
    let an = A21Endpoint::bind("127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let iws = A21Endpoint::bind("127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let identity = MobileIdentity::Imsi("001010123456789".into());
    let page = A21Message::air_interface_signaling(
        42,
        identity.clone(),
        MessageTransmissionControl {
            paging_message: true,
            ..MessageTransmissionControl::default()
        },
        vec![vec![0x01, 0x02, 0x03]],
    );
    iws.send(an.local_addr().unwrap(), &page).await.unwrap();

    let (from, received) = tokio::time::timeout(RECV_TIMEOUT, an.recv()).await.unwrap();
    let received = received.unwrap();
    assert_eq!(from, iws.local_addr().unwrap());
    assert_eq!(received, page);
    assert_eq!(
        received.lac_pdus().collect::<Vec<_>>(),
        vec![&[1u8, 2, 3][..]]
    );

    let ack = A21Message::ack(
        received.correlation_id(),
        received.mobile_identity().cloned(),
        Some(A21Cause::TunnelingNotAvailable),
    );
    an.send(from, &ack).await.unwrap();
    let (_, reply) = tokio::time::timeout(RECV_TIMEOUT, iws.recv())
        .await
        .unwrap();
    let reply = reply.unwrap();
    assert_eq!(reply.message_type, A21MessageType::Ack);
    assert_eq!(reply.correlation_id(), Some(42));
    assert_eq!(reply.cause(), Some(A21Cause::TunnelingNotAvailable));
}

#[tokio::test]
async fn malformed_datagram_is_reported_with_its_sender() {
    let receiver = A21Endpoint::bind("127.0.0.1:0".parse().unwrap())
        .await
        .unwrap();
    let sender = tokio::net::UdpSocket::bind("127.0.0.1:0").await.unwrap();
    sender
        .send_to(&[0x7f], receiver.local_addr().unwrap())
        .await
        .unwrap();
    let (from, result) = tokio::time::timeout(RECV_TIMEOUT, receiver.recv())
        .await
        .unwrap();
    assert_eq!(from, sender.local_addr().unwrap());
    assert!(result.is_err());
    let event = A21Message::event_notification(
        1,
        MobileIdentity::Esn(2),
        A21EventKind::NoLongerPresentInAn,
    );
    assert_eq!(
        A21Message::decode(&event.encode().unwrap())
            .unwrap()
            .event(),
        Some(A21EventKind::NoLongerPresentInAn)
    );
}
