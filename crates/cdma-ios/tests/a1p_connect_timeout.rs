//! A1p connect gives up on a peer that never answers INIT. Runs in its own
//! process because the usrsctp stack binds one UDP port per process.

use std::io::ErrorKind;
use std::net::UdpSocket;
use std::time::Duration;

use cdma_ios::transport::{self, A1Role, A1pConfig};
use cdma_sctp::{SctpBackend, SctpListener, UdpEncapsulation};

const UPPER_BOUND: Duration = Duration::from_secs(30);

fn free_udp_port() -> u16 {
    UdpSocket::bind("0.0.0.0:0")
        .and_then(|socket| socket.local_addr())
        .expect("ephemeral UDP port")
        .port()
}

#[tokio::test]
async fn connect_times_out_when_init_is_never_answered() {
    let silent_peer = UdpSocket::bind("127.0.0.1:0").expect("bind silent UDP peer");
    let silent_port = silent_peer.local_addr().unwrap().port();

    let mut config = A1pConfig::for_role(A1Role::Msc);
    config.sctp.backend = SctpBackend::Usrsctp;
    config.sctp.udp_encapsulation = Some(UdpEncapsulation {
        local_port: free_udp_port(),
        remote_port: silent_port,
    });
    let probe = SctpListener::bind(&["127.0.0.1:0".parse().unwrap()], &config.sctp)
        .expect("bind ephemeral SCTP probe");
    let addr = probe.local_addrs().unwrap()[0];
    drop(probe);

    let result = tokio::time::timeout(UPPER_BOUND, transport::connect(addr, &config, A1Role::Msc))
        .await
        .expect("connect must give up on its own");
    let error = result.err().expect("an unanswered INIT must not connect");
    assert_eq!(error.kind(), ErrorKind::TimedOut, "{error}");
}
