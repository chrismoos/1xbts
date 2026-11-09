//! usrsctp tests use UDP because raw IP needs privileges and on Linux competes
//! with the kernel for protocol 132.

use std::net::SocketAddr;
use std::time::Duration;

#[cfg(any(feature = "usrsctp", not(target_os = "linux")))]
use cdma_sctp::UdpEncapsulation;
use cdma_sctp::{
    CloseReason, Delivery, Error, SctpAssociation, SctpBackend, SctpConfig, SctpListener, SendInfo,
};

const TEST_TIMEOUT: Duration = Duration::from_secs(10);

/// One process-wide usrsctp stack serves every usrsctp test, so they share one
/// encapsulation port.
#[cfg(any(feature = "usrsctp", not(target_os = "linux")))]
fn usrsctp_udp_port() -> u16 {
    static PORT: std::sync::OnceLock<u16> = std::sync::OnceLock::new();
    *PORT.get_or_init(|| {
        std::net::UdpSocket::bind("0.0.0.0:0")
            .and_then(|socket| socket.local_addr())
            .expect("a free UDP port")
            .port()
    })
}

fn kernel_config() -> SctpConfig {
    SctpConfig {
        backend: SctpBackend::Kernel,
        heartbeat_interval_ms: Some(1_000),
        path_max_retransmissions: Some(3),
        rto_initial_ms: Some(200),
        rto_min_ms: Some(100),
        rto_max_ms: Some(1_000),
        ..SctpConfig::default()
    }
}

#[cfg(any(feature = "usrsctp", not(target_os = "linux")))]
fn usrsctp_config() -> SctpConfig {
    SctpConfig {
        backend: SctpBackend::Usrsctp,
        udp_encapsulation: Some(UdpEncapsulation {
            local_port: usrsctp_udp_port(),
            remote_port: usrsctp_udp_port(),
        }),
        rto_initial_ms: Some(200),
        rto_min_ms: Some(100),
        rto_max_ms: Some(1_000),
        ..SctpConfig::default()
    }
}

fn loopback(port: u16) -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], port))
}

fn bind_or_skip(addrs: &[SocketAddr], config: &SctpConfig) -> Option<SctpListener> {
    match SctpListener::bind(addrs, config) {
        Ok(listener) => Some(listener),
        Err(Error::BackendUnavailable { reason, .. }) => {
            eprintln!("skipping: {reason}");
            None
        }
        Err(err) => panic!("bind failed: {err}"),
    }
}

async fn connected_pair(
    config: &SctpConfig,
    listen_addrs: &[SocketAddr],
) -> Option<(SctpAssociation, SctpAssociation)> {
    let listener = bind_or_skip(listen_addrs, config)?;
    let port = listener.local_addrs().unwrap()[0].port();
    let remote: Vec<SocketAddr> = listen_addrs
        .iter()
        .map(|addr| SocketAddr::new(addr.ip(), port))
        .collect();
    let (client, accepted) = tokio::join!(
        SctpAssociation::connect(&remote, &[], config),
        listener.accept()
    );
    Some((client.unwrap(), accepted.unwrap().0))
}

async fn messages_keep_stream_ppid_and_order(config: SctpConfig) {
    let Some((client, server)) = connected_pair(&config, &[loopback(0)]).await else {
        return;
    };
    let sends = [
        (b"aspup".as_slice(), 0, 4, Delivery::Ordered),
        (b"data-1".as_slice(), 3, 4, Delivery::Ordered),
        (b"data-2".as_slice(), 3, 4, Delivery::Ordered),
        (b"urgent".as_slice(), 7, 0x0102_0304, Delivery::Unordered),
    ];
    for (data, stream_id, ppid, delivery) in sends {
        client
            .send(
                data,
                SendInfo {
                    stream_id,
                    ppid,
                    delivery,
                },
            )
            .await
            .unwrap();
    }
    let mut received = Vec::new();
    for _ in 0..sends.len() {
        let message = server.recv().await.unwrap();
        received.push((
            message.data,
            message.stream_id,
            message.ppid,
            message.delivery,
        ));
    }
    let ordered_on_stream_3: Vec<&[u8]> = received
        .iter()
        .filter(|message| message.1 == 3)
        .map(|message| message.0.as_slice())
        .collect();
    assert_eq!(ordered_on_stream_3, vec![b"data-1".as_slice(), b"data-2"]);
    for (data, stream_id, ppid, delivery) in sends {
        assert!(
            received.contains(&(data.to_vec(), stream_id, ppid, delivery)),
            "missing {:?}",
            String::from_utf8_lossy(data)
        );
    }

    server
        .send(
            b"ack",
            SendInfo {
                stream_id: 0,
                ppid: 4,
                ..SendInfo::default()
            },
        )
        .await
        .unwrap();
    let reply = client.recv().await.unwrap();
    assert_eq!(
        (reply.data.as_slice(), reply.stream_id, reply.ppid),
        (b"ack".as_slice(), 0, 4)
    );
    assert_eq!(client.outbound_streams(), config.outbound_streams);
}

async fn large_message_is_reassembled(config: SctpConfig) {
    let Some((client, server)) = connected_pair(&config, &[loopback(0)]).await else {
        return;
    };
    let data: Vec<u8> = (0..200_000u32).map(|i| (i % 251) as u8).collect();
    let (sent, received) = tokio::join!(
        client.send(
            &data,
            SendInfo {
                stream_id: 1,
                ppid: 4,
                ..SendInfo::default()
            }
        ),
        server.recv()
    );
    sent.unwrap();
    let received = received.unwrap();
    assert_eq!(received.data.len(), data.len());
    assert_eq!(received.data, data);
}

async fn multihomed_peer_reports_every_address(config: SctpConfig) {
    let addrs = [
        loopback(0),
        SocketAddr::from((std::net::Ipv6Addr::LOCALHOST, 0)),
    ];
    let Some(listener) = bind_or_skip(&addrs, &config) else {
        return;
    };
    let local = listener.local_addrs().unwrap();
    assert_eq!(local.len(), 2, "listener addrs {local:?}");
    let port = local[0].port();
    let remote: Vec<SocketAddr> = addrs
        .iter()
        .map(|addr| SocketAddr::new(addr.ip(), port))
        .collect();
    let (client, accepted) = tokio::join!(
        SctpAssociation::connect(&remote, &[], &config),
        listener.accept()
    );
    let client = client.unwrap();
    let _server = accepted.unwrap().0;
    let mut peers = client.peer_addrs().unwrap();
    peers.sort();
    let mut expected = remote.clone();
    expected.sort();
    assert_eq!(peers, expected);
}

async fn client_bound_to_one_address_advertises_only_it(config: SctpConfig) {
    let Some(listener) = bind_or_skip(&[loopback(0)], &config) else {
        return;
    };
    let remote = [loopback(listener.local_addrs().unwrap()[0].port())];
    let local = [loopback(0)];
    let (client, accepted) = tokio::join!(
        SctpAssociation::connect(&remote, &local, &config),
        listener.accept()
    );
    let _client = client.unwrap();
    let server = accepted.unwrap().0;

    let peers = server.peer_addrs().unwrap();

    assert_eq!(peers.len(), 1, "server sees {peers:?}");
    assert_eq!(peers[0].ip(), loopback(0).ip());
}

async fn graceful_shutdown_closes_peer(config: SctpConfig) {
    let Some((client, server)) = connected_pair(&config, &[loopback(0)]).await else {
        return;
    };
    client
        .send(
            b"last",
            SendInfo {
                stream_id: 1,
                ppid: 4,
                ..SendInfo::default()
            },
        )
        .await
        .unwrap();
    client.shutdown().unwrap();
    assert_eq!(server.recv().await.unwrap().data, b"last");
    assert!(matches!(
        server.recv().await,
        Err(Error::Closed(CloseReason::Shutdown))
    ));
    assert!(matches!(server.recv().await, Err(Error::Closed(_))));
}

async fn connect_without_listener_fails(config: SctpConfig) {
    let Some(probe) = bind_or_skip(&[loopback(0)], &config) else {
        return;
    };
    let port = probe.local_addrs().unwrap()[0].port();
    drop(probe);
    let result = SctpAssociation::connect(&[loopback(port)], &[], &config).await;
    assert!(result.is_err(), "connect to a closed port succeeded");
}

macro_rules! backend_tests {
    ($module:ident, $config:expr) => {
        mod $module {
            use super::*;

            #[tokio::test(flavor = "multi_thread")]
            async fn messages_keep_stream_ppid_and_order() {
                tokio::time::timeout(
                    TEST_TIMEOUT,
                    super::messages_keep_stream_ppid_and_order($config),
                )
                .await
                .unwrap();
            }

            #[tokio::test(flavor = "multi_thread")]
            async fn large_message_is_reassembled() {
                tokio::time::timeout(TEST_TIMEOUT, super::large_message_is_reassembled($config))
                    .await
                    .unwrap();
            }

            #[tokio::test(flavor = "multi_thread")]
            async fn multihomed_peer_reports_every_address() {
                tokio::time::timeout(
                    TEST_TIMEOUT,
                    super::multihomed_peer_reports_every_address($config),
                )
                .await
                .unwrap();
            }

            #[tokio::test(flavor = "multi_thread")]
            async fn client_bound_to_one_address_advertises_only_it() {
                tokio::time::timeout(
                    TEST_TIMEOUT,
                    super::client_bound_to_one_address_advertises_only_it($config),
                )
                .await
                .unwrap();
            }

            #[tokio::test(flavor = "multi_thread")]
            async fn graceful_shutdown_closes_peer() {
                tokio::time::timeout(TEST_TIMEOUT, super::graceful_shutdown_closes_peer($config))
                    .await
                    .unwrap();
            }

            #[tokio::test(flavor = "multi_thread")]
            async fn connect_without_listener_fails() {
                tokio::time::timeout(TEST_TIMEOUT, super::connect_without_listener_fails($config))
                    .await
                    .unwrap();
            }
        }
    };
}

#[cfg(target_os = "linux")]
backend_tests!(kernel, kernel_config());

#[cfg(any(feature = "usrsctp", not(target_os = "linux")))]
backend_tests!(usrsctp, usrsctp_config());

#[test]
fn kernel_backend_is_rejected_off_linux() {
    if cfg!(target_os = "linux") {
        return;
    }
    let result = SctpListener::bind(&[loopback(0)], &kernel_config());
    assert!(matches!(result, Err(Error::BackendUnavailable { .. })));
}

/// Runs only when `CDMA_SCTP_KERNEL_UDP_PORT` is set to the `net.sctp.udp_port`
/// sysctl port.
#[cfg(all(target_os = "linux", feature = "usrsctp"))]
#[tokio::test(flavor = "multi_thread")]
async fn kernel_and_usrsctp_interoperate_over_udp_encapsulation() {
    let Ok(kernel_udp_port) = std::env::var("CDMA_SCTP_KERNEL_UDP_PORT") else {
        eprintln!("skipping: CDMA_SCTP_KERNEL_UDP_PORT is not set");
        return;
    };
    let kernel_udp_port: u16 = kernel_udp_port.parse().unwrap();
    let kernel = SctpConfig {
        udp_encapsulation: Some(UdpEncapsulation {
            local_port: kernel_udp_port,
            remote_port: usrsctp_udp_port(),
        }),
        ..kernel_config()
    };
    let usrsctp = SctpConfig {
        udp_encapsulation: Some(UdpEncapsulation {
            local_port: usrsctp_udp_port(),
            remote_port: kernel_udp_port,
        }),
        ..usrsctp_config()
    };
    let run = async {
        let listener = SctpListener::bind(&[loopback(0)], &kernel).unwrap();
        let port = listener.local_addrs().unwrap()[0].port();
        let remote = [loopback(port)];
        let (client, accepted) = tokio::join!(
            SctpAssociation::connect(&remote, &[], &usrsctp),
            listener.accept()
        );
        let client = client.unwrap();
        let server = accepted.unwrap().0;
        let info = SendInfo {
            stream_id: 1,
            ppid: 4,
            ..SendInfo::default()
        };
        client.send(b"from-usrsctp", info).await.unwrap();
        assert_eq!(server.recv().await.unwrap().data, b"from-usrsctp");
        server.send(b"from-kernel", info).await.unwrap();
        assert_eq!(client.recv().await.unwrap().data, b"from-kernel");
    };
    tokio::time::timeout(TEST_TIMEOUT, run).await.unwrap();
}
