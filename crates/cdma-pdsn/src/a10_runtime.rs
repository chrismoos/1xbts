//! PDSN side of the A10 bearer.
//!
//! One UDP-encapsulated GRE socket serves every PCF. Each session keeps its
//! own return address: seeded from the A11 registration, then replaced by the
//! source of the session's uplink packets whenever the two differ.

use std::{
    collections::HashMap,
    net::{IpAddr, SocketAddr},
};

use cdma_common::error::Error;
use log::{info, warn};

const HRPD_BEARER_MAX_DATAGRAMS_PER_PASS: usize = 64;
const HRPD_BEARER_EVENT_QUEUE_DEPTH: usize = 256;

enum HrpdPdsnA10Command {
    Register {
        key: cdma_a11::SessionKey,
        bearer: cdma_a10::BearerSession,
        peer: SocketAddr,
        uplink_tx: tokio::sync::mpsc::Sender<Vec<u8>>,
        downlink_rx: tokio::sync::mpsc::Receiver<Vec<u8>>,
    },
    Release {
        key: cdma_a11::SessionKey,
    },
}

enum HrpdPdsnA10DownlinkEvent {
    Payload {
        session_id: u32,
        registration_id: u64,
        payload: Vec<u8>,
    },
    Closed {
        session_id: u32,
        registration_id: u64,
    },
}

#[derive(Clone)]
pub struct HrpdPdsnA10Runtime {
    tx: tokio::sync::mpsc::UnboundedSender<HrpdPdsnA10Command>,
}

impl HrpdPdsnA10Runtime {
    /// Installs a session whose downlink goes to `peer` until an uplink packet
    /// arrives from somewhere else.
    pub fn register(
        &self,
        key: cdma_a11::SessionKey,
        bearer: cdma_a10::BearerSession,
        peer: SocketAddr,
        uplink_tx: tokio::sync::mpsc::Sender<Vec<u8>>,
        downlink_rx: tokio::sync::mpsc::Receiver<Vec<u8>>,
    ) {
        if self
            .tx
            .send(HrpdPdsnA10Command::Register {
                key,
                bearer,
                peer,
                uplink_tx,
                downlink_rx,
            })
            .is_err()
        {
            warn!("HRPD PDSN A10 runtime stopped before session registration");
        }
    }

    /// Removes a session and its learned return address.
    pub fn release(&self, key: cdma_a11::SessionKey) {
        if self.tx.send(HrpdPdsnA10Command::Release { key }).is_err() {
            warn!("HRPD PDSN A10 runtime stopped before session release");
        }
    }
}

struct HrpdPdsnA10Session {
    key: cdma_a11::SessionKey,
    registration_id: u64,
    /// Where this session's downlink is sent.
    peer: SocketAddr,
    uplink_tx: tokio::sync::mpsc::Sender<Vec<u8>>,
}

/// Return address for a session that is being registered again.
///
/// A refresh from the same PCF must not undo what its uplink packets taught
/// us, so the learned address survives when the seed names the same host.
fn peer_for_registration(
    existing: Option<&HrpdPdsnA10Session>,
    key: cdma_a11::SessionKey,
    seed: SocketAddr,
) -> SocketAddr {
    match existing {
        Some(session) if session.key == key && session.peer.ip() == seed.ip() => session.peer,
        _ => seed,
    }
}

fn apply_hrpd_pdsn_a10_command(
    command: HrpdPdsnA10Command,
    table: &mut cdma_a10::BearerTable,
    sessions: &mut HashMap<u32, HrpdPdsnA10Session>,
    downlink_event_tx: &tokio::sync::mpsc::Sender<HrpdPdsnA10DownlinkEvent>,
    next_registration_id: &mut u64,
) {
    match command {
        HrpdPdsnA10Command::Register {
            key,
            bearer,
            peer,
            uplink_tx,
            mut downlink_rx,
        } => match table.apply_session(bearer) {
            Ok(outcome) => {
                let registration_id = *next_registration_id;
                *next_registration_id = next_registration_id.wrapping_add(1);
                let peer = peer_for_registration(sessions.get(&key.pcf_session_id), key, peer);
                sessions.insert(
                    key.pcf_session_id,
                    HrpdPdsnA10Session {
                        key,
                        registration_id,
                        peer,
                        uplink_tx,
                    },
                );
                let task_event_tx = downlink_event_tx.clone();
                tokio::spawn(async move {
                    while let Some(payload) = downlink_rx.recv().await {
                        if task_event_tx
                            .send(HrpdPdsnA10DownlinkEvent::Payload {
                                session_id: key.pcf_session_id,
                                registration_id,
                                payload,
                            })
                            .await
                            .is_err()
                        {
                            return;
                        }
                    }
                    let _ = task_event_tx
                        .send(HrpdPdsnA10DownlinkEvent::Closed {
                            session_id: key.pcf_session_id,
                            registration_id,
                        })
                        .await;
                });
                info!("HRPD PDSN A10: registered key={key:?} peer={peer} outcome={outcome:?}");
            }
            Err(err) => warn!("HRPD PDSN A10: failed to register key={key:?}: {err}"),
        },
        HrpdPdsnA10Command::Release { key } => {
            let is_registered = sessions
                .get(&key.pcf_session_id)
                .is_some_and(|session| session.key == key);
            if !is_registered {
                return;
            }
            sessions.remove(&key.pcf_session_id);
            table.remove_session_if_present(key.pcf_session_id);
            info!("HRPD PDSN A10: released key={key:?}");
        }
    }
}

async fn handle_hrpd_pdsn_a10_downlink_event(
    event: HrpdPdsnA10DownlinkEvent,
    bearer: &cdma_a8::TokioUdpGreEndpoint,
    table: &mut cdma_a10::BearerTable,
    sessions: &mut HashMap<u32, HrpdPdsnA10Session>,
    session_closed_tx: &tokio::sync::mpsc::UnboundedSender<cdma_a11::SessionKey>,
) {
    match event {
        HrpdPdsnA10DownlinkEvent::Payload {
            session_id,
            registration_id,
            payload,
        } => {
            let Some(session) = sessions.get(&session_id) else {
                return;
            };
            if session.registration_id != registration_id {
                return;
            }
            let key = session.key;
            let peer = session.peer;
            let outbound = match table.build_outbound_packet(session_id, payload) {
                Ok(outbound) => outbound,
                Err(err) => {
                    warn!("HRPD PDSN A10: failed to encode downlink key={key:?}: {err}");
                    return;
                }
            };
            if let Err(err) = bearer.send_wire_packet_to(&outbound.wire_bytes, peer).await {
                warn!("HRPD PDSN A10: send to {peer} failed key={key:?}: {err}");
            }
        }
        HrpdPdsnA10DownlinkEvent::Closed {
            session_id,
            registration_id,
        } => {
            let is_current = sessions
                .get(&session_id)
                .is_some_and(|session| session.registration_id == registration_id);
            if !is_current {
                return;
            }
            let Some(session) = sessions.remove(&session_id) else {
                return;
            };
            warn!(
                "HRPD PDSN A10: packet session downlink closed key={:?}",
                session.key
            );
            if session_closed_tx.send(session.key).is_err() {
                warn!(
                    "HRPD PDSN A10: A11 close-notification receiver dropped key={:?}",
                    session.key
                );
            }
            table.remove_session_if_present(session_id);
        }
    }
}

/// Decodes an uplink datagram, rebinding its session to the datagram's source
/// when the PCF is not the one the session was seeded with.
fn decode_uplink(
    table: &mut cdma_a10::BearerTable,
    rx_endpoint: cdma_a10::BearerEndpoint,
    wire: &[u8],
) -> cdma_a10::Result<cdma_a10::InboundPacket> {
    match table.decode_for_session(rx_endpoint, wire) {
        Err(cdma_a10::Error::EndpointMismatch { session_id }) => {
            table.rebind_session(session_id, rx_endpoint)?;
            table.decode_for_session(rx_endpoint, wire)
        }
        decoded => decoded,
    }
}

async fn deliver_uplink(
    wire: &[u8],
    from: SocketAddr,
    local_ipv4: [u8; 4],
    table: &mut cdma_a10::BearerTable,
    sessions: &mut HashMap<u32, HrpdPdsnA10Session>,
) {
    let IpAddr::V4(from_ip) = from.ip() else {
        warn!("HRPD PDSN A10: ignoring bearer packet from IPv6 source {from}");
        return;
    };
    let rx_endpoint = cdma_a10::BearerEndpoint::new(local_ipv4, from_ip.octets());
    let inbound = match decode_uplink(table, rx_endpoint, wire) {
        Ok(inbound) => inbound,
        Err(err) => {
            warn!("HRPD PDSN A10: bearer packet from {from} rejected: {err}");
            return;
        }
    };
    let Some(session) = sessions.get_mut(&inbound.session_id) else {
        warn!(
            "HRPD PDSN A10: decoded packet for unknown session=0x{:08x}",
            inbound.session_id
        );
        return;
    };
    if session.peer != from {
        warn!(
            "HRPD PDSN A10: return address for key={:?} changed from {} to {}",
            session.key, session.peer, from
        );
        session.peer = from;
    }
    let key = session.key;
    if session.uplink_tx.send(inbound.payload).await.is_err() {
        warn!("HRPD PDSN A10: packet session uplink closed key={key:?}");
    }
}

/// Starts the A10 bearer on `config`, which may omit `udp_peer_addr` because
/// every session carries its own PCF address. `local_ipv4` is the bearer's own
/// address as sessions are bound to it.
pub fn spawn_hrpd_pdsn_a10_runtime(
    config: cdma_a10::BearerTransportConfig,
    local_ipv4: [u8; 4],
    session_closed_tx: tokio::sync::mpsc::UnboundedSender<cdma_a11::SessionKey>,
) -> Result<HrpdPdsnA10Runtime, Error> {
    let bearer = cdma_a8::UdpGreEndpoint::bind_multi_peer(config, "pdsn.a10_bearer")
        .map_err(|err| Error::from(format!("HRPD PDSN A10 bind failed: {err}")))?
        .into_tokio()
        .map_err(|err| Error::from(format!("HRPD PDSN A10 Tokio setup failed: {err}")))?;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        let mut table = cdma_a10::BearerTable::new();
        let mut sessions: HashMap<u32, HrpdPdsnA10Session> = HashMap::new();
        let (downlink_event_tx, mut downlink_event_rx) =
            tokio::sync::mpsc::channel(HRPD_BEARER_EVENT_QUEUE_DEPTH);
        let mut next_registration_id = 0u64;
        let mut pending_command = None;
        let mut pending_downlink_event = None;
        let mut buf = vec![0u8; 8192];
        info!("HRPD PDSN A10 bearer listener started");
        loop {
            let mut pass_active = false;
            while let Some(command) = pending_command.take().or_else(|| rx.try_recv().ok()) {
                pass_active = true;
                apply_hrpd_pdsn_a10_command(
                    command,
                    &mut table,
                    &mut sessions,
                    &downlink_event_tx,
                    &mut next_registration_id,
                );
            }

            for _ in 0..HRPD_BEARER_MAX_DATAGRAMS_PER_PASS {
                let Some((wire, from)) = recv_udp_gre_packet(&bearer, &mut buf, "HRPD PDSN A10")
                else {
                    break;
                };
                pass_active = true;
                deliver_uplink(&wire, from, local_ipv4, &mut table, &mut sessions).await;
            }

            for _ in 0..HRPD_BEARER_MAX_DATAGRAMS_PER_PASS {
                let event = pending_downlink_event
                    .take()
                    .or_else(|| downlink_event_rx.try_recv().ok());
                let Some(event) = event else {
                    break;
                };
                pass_active = true;
                handle_hrpd_pdsn_a10_downlink_event(
                    event,
                    &bearer,
                    &mut table,
                    &mut sessions,
                    &session_closed_tx,
                )
                .await;
            }

            if pass_active {
                continue;
            }
            tokio::select! {
                command = rx.recv() => {
                    let Some(command) = command else {
                        info!("HRPD PDSN A10 bearer listener stopped");
                        return;
                    };
                    pending_command = Some(command);
                }
                event = downlink_event_rx.recv() => {
                    pending_downlink_event = event;
                }
                result = bearer.readable() => {
                    if let Err(err) = result {
                        warn!("HRPD PDSN A10 readiness failed: {err}");
                        return;
                    }
                }
            }
        }
    });
    Ok(HrpdPdsnA10Runtime { tx })
}

fn recv_udp_gre_packet(
    endpoint: &cdma_a8::TokioUdpGreEndpoint,
    buf: &mut [u8],
    label: &str,
) -> Option<(Vec<u8>, SocketAddr)> {
    match endpoint.try_recv_gre_packet(buf) {
        Ok((packet, from)) => match packet.encode() {
            Ok(wire) => Some((wire, from)),
            Err(err) => {
                warn!("{label}: failed to reserialize inbound GRE packet: {err}");
                None
            }
        },
        Err(cdma_a8::Error::UdpTransport(err)) if is_recv_timeout(&err) => None,
        Err(err) => {
            warn!("{label}: receive/decode failed: {err}");
            None
        }
    }
}

fn is_recv_timeout(err: &str) -> bool {
    let err = err.to_ascii_lowercase();
    err.contains("wouldblock")
        || err.contains("would block")
        || err.contains("resource temporarily unavailable")
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{net::UdpSocket, time::Duration};

    const LOOPBACK: [u8; 4] = [127, 0, 0, 1];
    const RECV_TIMEOUT: Duration = Duration::from_millis(250);

    struct TestPcf {
        addr: SocketAddr,
        bearer: cdma_a8::TokioUdpGreEndpoint,
    }

    impl TestPcf {
        fn new(pdsn_addr: SocketAddr) -> Self {
            let socket = UdpSocket::bind("127.0.0.1:0").unwrap();
            let addr = socket.local_addr().unwrap();
            let bearer = cdma_a8::UdpGreEndpoint::from_socket(socket, pdsn_addr)
                .into_tokio()
                .unwrap();
            Self { addr, bearer }
        }

        async fn recv(&self) -> Option<cdma_a8::GrePacket> {
            tokio::time::timeout(RECV_TIMEOUT, async {
                let mut buf = [0_u8; 128];
                self.bearer.recv_gre_packet(&mut buf).await.unwrap().0
            })
            .await
            .ok()
        }

        async fn send_uplink(&self, key: u32, payload: impl Into<Vec<u8>>) {
            self.bearer
                .send_gre_packet(&cdma_a8::GrePacket::octet_stream(key, Some(0), payload))
                .await
                .unwrap();
        }
    }

    struct TestSession {
        key: cdma_a11::SessionKey,
        uplink_rx: tokio::sync::mpsc::Receiver<Vec<u8>>,
        downlink_tx: tokio::sync::mpsc::Sender<Vec<u8>>,
    }

    fn register_session(
        runtime: &HrpdPdsnA10Runtime,
        pcf_session_id: u32,
        peer: SocketAddr,
    ) -> TestSession {
        let key = cdma_a11::SessionKey {
            pcf_session_id,
            mn_session_reference_id: 7,
        };
        let IpAddr::V4(peer_ip) = peer.ip() else {
            panic!("test peers are IPv4");
        };
        let (uplink_tx, uplink_rx) = tokio::sync::mpsc::channel(8);
        let (downlink_tx, downlink_rx) = tokio::sync::mpsc::channel(8);
        runtime.register(
            key,
            cdma_a10::BearerSession::new(
                pcf_session_id,
                cdma_a10::BearerEndpoint::new(LOOPBACK, peer_ip.octets()),
            ),
            peer,
            uplink_tx,
            downlink_rx,
        );
        TestSession {
            key,
            uplink_rx,
            downlink_tx,
        }
    }

    fn free_udp_addr() -> SocketAddr {
        UdpSocket::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
    }

    fn multi_peer_config(pdsn_addr: SocketAddr) -> cdma_a10::BearerTransportConfig {
        cdma_a10::BearerTransportConfig {
            mode: cdma_a10::BearerTransportMode::UdpEncapsulatedGre,
            udp_bind_addr: Some(pdsn_addr),
            udp_peer_addr: None,
        }
    }

    async fn recv_uplink(session: &mut TestSession) -> Option<Vec<u8>> {
        tokio::time::timeout(RECV_TIMEOUT, session.uplink_rx.recv())
            .await
            .ok()
            .flatten()
    }

    #[tokio::test]
    async fn a10_runtime_wakes_for_socket_downlink_and_session_close_events() {
        let peer_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let peer_addr = peer_socket.local_addr().unwrap();
        let pdsn_addr = free_udp_addr();
        let key = cdma_a11::SessionKey {
            pcf_session_id: 1,
            mn_session_reference_id: 7,
        };
        let (session_closed_tx, mut session_closed_rx) = tokio::sync::mpsc::unbounded_channel();
        let runtime = spawn_hrpd_pdsn_a10_runtime(
            cdma_a10::BearerTransportConfig::udp_encapsulated_gre(pdsn_addr, peer_addr),
            LOOPBACK,
            session_closed_tx,
        )
        .unwrap();
        let (uplink_tx, mut uplink_rx) = tokio::sync::mpsc::channel(8);
        let (downlink_tx, downlink_rx) = tokio::sync::mpsc::channel(8);
        runtime.register(
            key,
            cdma_a10::BearerSession::new(
                key.pcf_session_id,
                cdma_a10::BearerEndpoint::new(LOOPBACK, LOOPBACK),
            ),
            peer_addr,
            uplink_tx,
            downlink_rx,
        );
        let peer = cdma_a8::UdpGreEndpoint::from_socket(peer_socket, pdsn_addr)
            .into_tokio()
            .unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;

        downlink_tx.send(vec![0xde, 0xad]).await.unwrap();
        let (packet, _) = tokio::time::timeout(RECV_TIMEOUT, async {
            let mut buf = [0_u8; 128];
            peer.recv_gre_packet(&mut buf).await
        })
        .await
        .unwrap()
        .unwrap();
        assert_eq!(packet.key, Some(key.pcf_session_id));
        assert_eq!(packet.payload, [0xde, 0xad]);

        peer.send_gre_packet(&cdma_a8::GrePacket::octet_stream(
            key.pcf_session_id,
            Some(0),
            [0xbe, 0xef],
        ))
        .await
        .unwrap();
        assert_eq!(
            tokio::time::timeout(RECV_TIMEOUT, uplink_rx.recv())
                .await
                .unwrap()
                .unwrap(),
            [0xbe, 0xef]
        );

        drop(downlink_tx);
        assert_eq!(
            tokio::time::timeout(RECV_TIMEOUT, session_closed_rx.recv())
                .await
                .unwrap()
                .unwrap(),
            key
        );
    }

    #[tokio::test]
    async fn a10_downlink_reaches_each_sessions_own_pcf_before_any_uplink() {
        let pdsn_addr = free_udp_addr();
        let pcf_a = TestPcf::new(pdsn_addr);
        let pcf_b = TestPcf::new(pdsn_addr);
        let (session_closed_tx, _session_closed_rx) = tokio::sync::mpsc::unbounded_channel();
        let runtime =
            spawn_hrpd_pdsn_a10_runtime(multi_peer_config(pdsn_addr), LOOPBACK, session_closed_tx)
                .unwrap();
        let session_a = register_session(&runtime, 0xa, pcf_a.addr);
        let session_b = register_session(&runtime, 0xb, pcf_b.addr);
        tokio::time::sleep(Duration::from_millis(10)).await;

        session_a.downlink_tx.send(vec![0xaa]).await.unwrap();
        session_b.downlink_tx.send(vec![0xbb]).await.unwrap();

        let to_a = pcf_a.recv().await.expect("PCF A receives its session");
        assert_eq!(to_a.key, Some(session_a.key.pcf_session_id));
        assert_eq!(to_a.payload, [0xaa]);
        let to_b = pcf_b.recv().await.expect("PCF B receives its session");
        assert_eq!(to_b.key, Some(session_b.key.pcf_session_id));
        assert_eq!(to_b.payload, [0xbb]);
        assert!(
            pcf_a.recv().await.is_none(),
            "PCF A got another PCF's traffic"
        );
        assert!(
            pcf_b.recv().await.is_none(),
            "PCF B got another PCF's traffic"
        );
    }

    #[tokio::test]
    async fn a10_uplink_source_replaces_seeded_peer_and_survives_refresh() {
        let pdsn_addr = free_udp_addr();
        let pcf = TestPcf::new(pdsn_addr);
        let stale_seed = free_udp_addr();
        let (session_closed_tx, _session_closed_rx) = tokio::sync::mpsc::unbounded_channel();
        let runtime =
            spawn_hrpd_pdsn_a10_runtime(multi_peer_config(pdsn_addr), LOOPBACK, session_closed_tx)
                .unwrap();
        let mut session = register_session(&runtime, 0x1, stale_seed);
        tokio::time::sleep(Duration::from_millis(10)).await;

        pcf.send_uplink(session.key.pcf_session_id, [0x01]).await;
        assert_eq!(recv_uplink(&mut session).await.unwrap(), [0x01]);

        session.downlink_tx.send(vec![0x02]).await.unwrap();
        let packet = pcf
            .recv()
            .await
            .expect("downlink follows the uplink source");
        assert_eq!(packet.payload, [0x02]);

        let refreshed = register_session(&runtime, 0x1, stale_seed);
        tokio::time::sleep(Duration::from_millis(10)).await;
        refreshed.downlink_tx.send(vec![0x03]).await.unwrap();
        let packet = pcf
            .recv()
            .await
            .expect("learned address survives a refresh");
        assert_eq!(packet.payload, [0x03]);
    }

    #[tokio::test]
    async fn a10_release_drops_session_and_its_peer() {
        let pdsn_addr = free_udp_addr();
        let pcf = TestPcf::new(pdsn_addr);
        let (session_closed_tx, mut session_closed_rx) = tokio::sync::mpsc::unbounded_channel();
        let runtime =
            spawn_hrpd_pdsn_a10_runtime(multi_peer_config(pdsn_addr), LOOPBACK, session_closed_tx)
                .unwrap();
        let mut session = register_session(&runtime, 0x1, pcf.addr);
        tokio::time::sleep(Duration::from_millis(10)).await;
        session.downlink_tx.send(vec![0x01]).await.unwrap();
        assert!(pcf.recv().await.is_some());

        runtime.release(session.key);
        tokio::time::sleep(Duration::from_millis(10)).await;

        session.downlink_tx.send(vec![0x02]).await.unwrap();
        assert!(pcf.recv().await.is_none(), "released session still sends");
        pcf.send_uplink(session.key.pcf_session_id, [0x03]).await;
        assert!(
            recv_uplink(&mut session).await.is_none(),
            "released session still receives"
        );
        drop(session.downlink_tx);
        assert!(
            tokio::time::timeout(RECV_TIMEOUT, session_closed_rx.recv())
                .await
                .is_err(),
            "released session reported closed again"
        );
    }
}
