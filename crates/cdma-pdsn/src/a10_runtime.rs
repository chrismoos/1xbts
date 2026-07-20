use std::collections::HashMap;

use cdma_common::error::Error;
use log::{info, warn};

const HRPD_BEARER_MAX_DATAGRAMS_PER_PASS: usize = 64;
const HRPD_BEARER_EVENT_QUEUE_DEPTH: usize = 256;

enum HrpdPdsnA10Command {
    Register {
        key: cdma_a11::SessionKey,
        bearer: cdma_a10::BearerSession,
        uplink_tx: tokio::sync::mpsc::Sender<Vec<u8>>,
        downlink_rx: tokio::sync::mpsc::Receiver<Vec<u8>>,
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
    pub fn register(
        &self,
        key: cdma_a11::SessionKey,
        bearer: cdma_a10::BearerSession,
        uplink_tx: tokio::sync::mpsc::Sender<Vec<u8>>,
        downlink_rx: tokio::sync::mpsc::Receiver<Vec<u8>>,
    ) {
        if self
            .tx
            .send(HrpdPdsnA10Command::Register {
                key,
                bearer,
                uplink_tx,
                downlink_rx,
            })
            .is_err()
        {
            warn!("HRPD PDSN A10 runtime stopped before session registration");
        }
    }
}

struct HrpdPdsnA10Session {
    key: cdma_a11::SessionKey,
    registration_id: u64,
    uplink_tx: tokio::sync::mpsc::Sender<Vec<u8>>,
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
            uplink_tx,
            mut downlink_rx,
        } => match table.apply_session(bearer) {
            Ok(outcome) => {
                let registration_id = *next_registration_id;
                *next_registration_id = next_registration_id.wrapping_add(1);
                sessions.insert(
                    key.pcf_session_id,
                    HrpdPdsnA10Session {
                        key,
                        registration_id,
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
                info!("HRPD PDSN A10: registered key={key:?} outcome={outcome:?}");
            }
            Err(err) => warn!("HRPD PDSN A10: failed to register key={key:?}: {err}"),
        },
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
            let outbound = match table.build_outbound_packet(session_id, payload) {
                Ok(outbound) => outbound,
                Err(err) => {
                    warn!("HRPD PDSN A10: failed to encode downlink key={key:?}: {err}");
                    return;
                }
            };
            if let Err(err) = bearer.send_wire_packet(&outbound.wire_bytes).await {
                warn!("HRPD PDSN A10: send failed key={key:?}: {err}");
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

pub fn spawn_hrpd_pdsn_a10_runtime(
    config: cdma_a10::BearerTransportConfig,
    endpoint: cdma_a10::BearerEndpoint,
    session_closed_tx: tokio::sync::mpsc::UnboundedSender<cdma_a11::SessionKey>,
) -> Result<HrpdPdsnA10Runtime, Error> {
    let bearer = cdma_a8::UdpGreEndpoint::bind(config, "pdsn.a10_bearer")
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
                let Some(wire) = recv_udp_gre_packet(&bearer, &mut buf, "HRPD PDSN A10") else {
                    break;
                };
                pass_active = true;
                match table.decode_for_session(endpoint, &wire) {
                    Ok(inbound) => {
                        let Some((key, uplink_tx)) = sessions
                            .get(&inbound.session_id)
                            .map(|session| (session.key, session.uplink_tx.clone()))
                        else {
                            warn!(
                                "HRPD PDSN A10: decoded packet for unknown session=0x{:08x}",
                                inbound.session_id
                            );
                            continue;
                        };
                        if uplink_tx.send(inbound.payload).await.is_err() {
                            warn!("HRPD PDSN A10: packet session uplink closed key={key:?}");
                        }
                    }
                    Err(err) => warn!("HRPD PDSN A10: bearer packet rejected: {err}"),
                }
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
) -> Option<Vec<u8>> {
    match endpoint.try_recv_gre_packet(buf) {
        Ok((packet, _)) => match packet.encode() {
            Ok(wire) => Some(wire),
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

    #[tokio::test]
    async fn a10_runtime_wakes_for_socket_downlink_and_session_close_events() {
        let peer_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let peer_addr = peer_socket.local_addr().unwrap();
        let pdsn_addr = UdpSocket::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        let endpoint = cdma_a10::BearerEndpoint::new([127, 0, 0, 1], [127, 0, 0, 1]);
        let key = cdma_a11::SessionKey {
            pcf_session_id: 1,
            mn_session_reference_id: 7,
        };
        let (session_closed_tx, mut session_closed_rx) = tokio::sync::mpsc::unbounded_channel();
        let runtime = spawn_hrpd_pdsn_a10_runtime(
            cdma_a10::BearerTransportConfig::udp_encapsulated_gre(pdsn_addr, peer_addr),
            endpoint,
            session_closed_tx,
        )
        .unwrap();
        let (uplink_tx, mut uplink_rx) = tokio::sync::mpsc::channel(8);
        let (downlink_tx, downlink_rx) = tokio::sync::mpsc::channel(8);
        runtime.register(
            key,
            cdma_a10::BearerSession::new(key.pcf_session_id, endpoint),
            uplink_tx,
            downlink_rx,
        );
        let peer = cdma_a8::UdpGreEndpoint::from_socket(peer_socket, pdsn_addr)
            .into_tokio()
            .unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;

        downlink_tx.send(vec![0xde, 0xad]).await.unwrap();
        let (packet, _) = tokio::time::timeout(Duration::from_millis(250), async {
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
            tokio::time::timeout(Duration::from_millis(250), uplink_rx.recv())
                .await
                .unwrap()
                .unwrap(),
            [0xbe, 0xef]
        );

        drop(downlink_tx);
        assert_eq!(
            tokio::time::timeout(Duration::from_millis(250), session_closed_rx.recv())
                .await
                .unwrap()
                .unwrap(),
            key
        );
    }
}
