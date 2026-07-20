//! End-to-end A21 client/server integration over a real TCP socket on an
//! ephemeral port. Asserts every message variant is observed by the handler
//! in the order it was sent.

use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use cdma_a21::{A21Client, A21Connection, A21Handler, A21Message, A21Server, PagingSource};
use tokio::sync::Mutex;

#[derive(Default)]
struct RecordingHandler {
    seen: Arc<Mutex<Vec<A21Message>>>,
}

impl A21Handler for RecordingHandler {
    async fn on_identity_binding(
        &self,
        _peer: SocketAddr,
        _conn: A21Connection,
        imsi: u64,
    ) -> cdma_a21::Result<()> {
        self.seen
            .lock()
            .await
            .push(A21Message::IdentityBinding { imsi });
        Ok(())
    }

    async fn on_identity_release(
        &self,
        _peer: SocketAddr,
        _conn: A21Connection,
        imsi: u64,
    ) -> cdma_a21::Result<()> {
        self.seen
            .lock()
            .await
            .push(A21Message::IdentityRelease { imsi });
        Ok(())
    }

    async fn on_cross_page_request(
        &self,
        _peer: SocketAddr,
        conn: A21Connection,
        imsi: u64,
        source: PagingSource,
        payload: Vec<u8>,
    ) -> cdma_a21::Result<()> {
        self.seen.lock().await.push(A21Message::CrossPageRequest {
            imsi,
            source,
            payload,
        });
        // Exercise reverse direction on the same connection.
        conn.send(&A21Message::CrossPageAck {
            imsi,
            accepted: true,
            reason: None,
        })
        .await
    }

    async fn on_cross_page_ack(
        &self,
        _peer: SocketAddr,
        _conn: A21Connection,
        imsi: u64,
        accepted: bool,
        reason: Option<String>,
    ) -> cdma_a21::Result<()> {
        self.seen.lock().await.push(A21Message::CrossPageAck {
            imsi,
            accepted,
            reason,
        });
        Ok(())
    }

    async fn on_suppression_start(
        &self,
        _peer: SocketAddr,
        _conn: A21Connection,
        imsi: u64,
        source: PagingSource,
    ) -> cdma_a21::Result<()> {
        self.seen
            .lock()
            .await
            .push(A21Message::SuppressionStart { imsi, source });
        Ok(())
    }

    async fn on_suppression_end(
        &self,
        _peer: SocketAddr,
        _conn: A21Connection,
        imsi: u64,
    ) -> cdma_a21::Result<()> {
        self.seen
            .lock()
            .await
            .push(A21Message::SuppressionEnd { imsi });
        Ok(())
    }
}

#[tokio::test]
async fn client_server_roundtrip_over_real_tcp() {
    let server = A21Server::bind(SocketAddr::from((Ipv4Addr::LOCALHOST, 0)))
        .await
        .expect("bind");
    let addr = server.local_addr().expect("local_addr");

    let seen = Arc::new(Mutex::new(Vec::new()));
    let handler = RecordingHandler {
        seen: Arc::clone(&seen),
    };
    tokio::spawn(async move {
        let _ = server.serve(handler).await;
    });

    let mut client = A21Client::connect(addr).await.expect("connect");

    let outbound = vec![
        A21Message::IdentityBinding {
            imsi: 310_260_000_000_001,
        },
        A21Message::IdentityRelease {
            imsi: 310_260_000_000_002,
        },
        A21Message::CrossPageRequest {
            imsi: 310_260_000_000_003,
            source: PagingSource::OneX,
            payload: vec![0xde, 0xad, 0xbe, 0xef],
        },
        A21Message::CrossPageAck {
            imsi: 310_260_000_000_004,
            accepted: false,
            reason: Some("HRPD session absent".into()),
        },
        A21Message::SuppressionStart {
            imsi: 310_260_000_000_005,
            source: PagingSource::Hrpd,
        },
        A21Message::SuppressionEnd {
            imsi: 310_260_000_000_005,
        },
    ];

    for m in &outbound {
        client.send(m).await.expect("send");
    }

    // The CrossPageRequest triggers a server-side reply; consume it.
    let reply = tokio::time::timeout(Duration::from_secs(2), client.recv())
        .await
        .expect("recv timeout")
        .expect("recv");
    assert_eq!(
        reply,
        A21Message::CrossPageAck {
            imsi: 310_260_000_000_003,
            accepted: true,
            reason: None,
        }
    );

    // Drain the handler. Poll until all 6 messages observed or timeout.
    let deadline = std::time::Instant::now() + Duration::from_secs(2);
    loop {
        if seen.lock().await.len() == outbound.len() {
            break;
        }
        if std::time::Instant::now() >= deadline {
            panic!(
                "timeout: handler observed {}/{} messages",
                seen.lock().await.len(),
                outbound.len()
            );
        }
        tokio::time::sleep(Duration::from_millis(10)).await;
    }

    let observed = seen.lock().await.clone();
    assert_eq!(observed, outbound);
}
