use std::net::SocketAddr;
use std::time::Duration;

use cdma_a9::{
    A8TrafficId, A9Transport, CallConnectionReference, CauseValue, ConRef, ConnectA8Message,
    CorrelationId, DelimitedReader, Error, MessageType, SignalingEndpoint, SignalingMessage,
    TCP_DELIMITER_LEN, delimit,
};

const RECV_TIMEOUT: Duration = Duration::from_secs(5);

fn connect_message() -> ConnectA8Message {
    ConnectA8Message {
        call_connection_reference: Some(CallConnectionReference::new(0x0102, 0x0304, 0x05060708)),
        correlation_id: Some(CorrelationId([9, 10, 11, 12])),
        imsi: Some("123456789012345".into()),
        esn: Some(0x01020304),
        meid: None,
        con_ref: ConRef(0x01),
        a8_traffic_id: A8TrafficId::gre_ppp(0x01020304, [192, 0, 2, 10]),
        cause: CauseValue(0x13),
        pdsn_ip_address: None,
        a9_indicators: None,
    }
}

fn loopback() -> SocketAddr {
    "127.0.0.1:0".parse().unwrap()
}

#[test]
fn signaling_message_reads_type_from_first_octet() {
    let payload = connect_message().encode().unwrap();
    let message = SignalingMessage::new(payload.clone()).unwrap();
    assert_eq!(message.message_type, MessageType::ConnectA8);
    assert_eq!(message.payload, payload);
    assert_eq!(
        ConnectA8Message::decode(&message.payload).unwrap(),
        connect_message()
    );
}

#[test]
fn signaling_message_rejects_empty_and_unknown_type() {
    assert_eq!(
        SignalingMessage::new(Vec::new()).unwrap_err(),
        Error::EmptyMessage
    );
    assert_eq!(
        SignalingMessage::new(vec![0xff, 0x00]).unwrap_err(),
        Error::UnknownMessageType(0xff)
    );
}

#[test]
fn tcp_delimiter_is_version_1_and_four_octet_length() {
    let payload = connect_message().encode().unwrap();
    let framed = delimit(&payload).unwrap();
    let len = payload.len() as u32;
    assert_eq!(framed[0], 0x01);
    assert_eq!(&framed[1..TCP_DELIMITER_LEN], &len.to_be_bytes());
    assert_eq!(&framed[TCP_DELIMITER_LEN..], payload.as_slice());
}

#[test]
fn delimited_reader_splits_messages_across_arbitrary_reads() {
    let first = connect_message().encode().unwrap();
    let second = vec![MessageType::ReleaseA8Complete as u8, 0x00];
    let mut stream = delimit(&first).unwrap();
    stream.extend(delimit(&second).unwrap());

    let mut reader = DelimitedReader::default();
    let mut messages = Vec::new();
    for byte in stream {
        reader.push(&[byte]);
        while let Some(message) = reader.next_message().unwrap() {
            messages.push(message);
        }
    }
    assert_eq!(messages, vec![first, second]);
}

#[test]
fn delimited_reader_rejects_unknown_version() {
    let mut reader = DelimitedReader::default();
    reader.push(&[0x02, 0, 0, 0, 1, 0x0b]);
    assert!(matches!(
        reader.next_message(),
        Err(Error::InvalidValue { value: 2, .. })
    ));
}

#[tokio::test]
async fn udp_datagram_is_the_bare_a9_message() {
    let pcf = SignalingEndpoint::listen(loopback(), A9Transport::Udp)
        .await
        .unwrap();
    let bs = SignalingEndpoint::client(loopback(), A9Transport::Udp)
        .await
        .unwrap();
    let raw = tokio::net::UdpSocket::bind(loopback()).await.unwrap();
    let payload = connect_message().encode().unwrap();

    pcf.send_payload(raw.local_addr().unwrap(), &payload)
        .await
        .unwrap();
    let mut buf = [0u8; 1024];
    let (len, _) = raw.recv_from(&mut buf).await.unwrap();
    assert_eq!(&buf[..len], payload.as_slice());

    bs.send_payload(pcf.local_addr().unwrap(), &payload)
        .await
        .unwrap();
    let (received, peer) = tokio::time::timeout(RECV_TIMEOUT, pcf.recv(&mut buf))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(peer, bs.local_addr().unwrap());
    assert_eq!(received.payload, payload);
}

#[tokio::test]
async fn tcp_pcf_replies_on_the_connection_the_bs_opened() {
    let pcf = SignalingEndpoint::listen(loopback(), A9Transport::Tcp)
        .await
        .unwrap();
    let pcf_addr = pcf.local_addr().unwrap();
    let bs = SignalingEndpoint::client(loopback(), A9Transport::Tcp)
        .await
        .unwrap();
    let setup = vec![MessageType::SetupA8 as u8, 0x00];
    let connect = connect_message().encode().unwrap();
    let mut buf = [0u8; 1024];

    bs.send_payload(pcf_addr, &setup).await.unwrap();
    let (received, bs_peer) = tokio::time::timeout(RECV_TIMEOUT, pcf.recv(&mut buf))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(received.message_type, MessageType::SetupA8);
    assert_eq!(received.payload, setup);

    pcf.send_payload(bs_peer, &connect).await.unwrap();
    let (reply, from) = tokio::time::timeout(RECV_TIMEOUT, bs.recv(&mut buf))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(from, pcf_addr);
    assert_eq!(reply.message_type, MessageType::ConnectA8);
    assert_eq!(reply.payload, connect);
}

#[tokio::test]
async fn tcp_wire_bytes_carry_the_ios_delimiter() {
    let listener = tokio::net::TcpListener::bind(loopback()).await.unwrap();
    let pcf_addr = listener.local_addr().unwrap();
    let bs = SignalingEndpoint::client(loopback(), A9Transport::Tcp)
        .await
        .unwrap();
    let payload = connect_message().encode().unwrap();
    let (sent, accepted) = tokio::join!(bs.send_payload(pcf_addr, &payload), listener.accept());
    sent.unwrap();
    let (mut stream, _) = accepted.unwrap();
    let mut wire = vec![0u8; TCP_DELIMITER_LEN + payload.len()];
    tokio::io::AsyncReadExt::read_exact(&mut stream, &mut wire)
        .await
        .unwrap();
    assert_eq!(wire, delimit(&payload).unwrap());
}

#[tokio::test]
async fn tcp_pcf_cannot_reach_a_bs_without_a_connection() {
    let pcf = SignalingEndpoint::listen(loopback(), A9Transport::Tcp)
        .await
        .unwrap();
    let unreachable: SocketAddr = "127.0.0.1:1".parse().unwrap();
    let payload = connect_message().encode().unwrap();
    assert!(pcf.send_payload(unreachable, &payload).await.is_err());
}
