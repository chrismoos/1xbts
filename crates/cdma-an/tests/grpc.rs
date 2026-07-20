use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;

use cdma_an::grpc::AnServiceImpl;
use cdma_an::protocols::REV0_DEFAULTS;
use cdma_an::session::{Session, SessionState};
use cdma_an::subnet::{UatiAllocator, UatiSubnet};
use cdma_an::uati::Uati;
use tokio::net::TcpListener;
use tokio::sync::Mutex;

use cdma_an::proto::an::v1::an_service_client::AnServiceClient;
use cdma_an::proto::an::v1::{GetSessionRequest, GetSessionsRequest, SessionState as ProtoState};

#[tokio::test]
async fn grpc_round_trip_get_sessions_and_get_session() {
    let sessions = Arc::new(Mutex::new(HashMap::new()));
    let uati = Arc::new(Mutex::new(UatiAllocator::new(UatiSubnet {
        color_code: 7,
        uati104: [0; 13],
        subnet_mask: 24,
    })));

    {
        let mut s = sessions.lock().await;
        let mut sess = Session::new(
            Uati::from_compact(0x0034_5678, [0; 13], 7, 24),
            7,
            REV0_DEFAULTS,
        );
        sess.state = SessionState::Open;
        s.insert(0x0034_5678, sess);
    }

    let svc = AnServiceImpl::new(Arc::clone(&sessions), Arc::clone(&uati));
    // Probe an ephemeral port, drop the listener, then let tonic bind it
    // directly (the brief OS reuse window is acceptable for a unit test).
    let probe = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = probe.local_addr().unwrap();
    drop(probe);

    tokio::spawn(async move {
        let _ = tonic::transport::Server::builder()
            .add_service(svc.into_server())
            .serve(addr)
            .await;
    });

    tokio::time::sleep(Duration::from_millis(50)).await;

    let mut client = AnServiceClient::connect(format!("http://{addr}"))
        .await
        .unwrap();

    let list = client
        .get_sessions(GetSessionsRequest {
            state_filter: ProtoState::Unspecified as i32,
        })
        .await
        .unwrap()
        .into_inner();
    assert_eq!(list.sessions.len(), 1);
    assert_eq!(list.sessions[0].uati, 0x0034_5678);

    let one = client
        .get_session(GetSessionRequest { uati: 0x0034_5678 })
        .await
        .unwrap()
        .into_inner();
    assert!(one.session.is_some());

    let err = client
        .get_session(GetSessionRequest { uati: 0xDEAD })
        .await
        .unwrap_err();
    assert_eq!(err.code(), tonic::Code::NotFound);
}
