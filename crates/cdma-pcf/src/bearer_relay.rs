use std::collections::HashMap;

use cdma_common::error::Error;
use log::{info, warn};

const HRPD_BEARER_MAX_DATAGRAMS_PER_PASS: usize = 64;

enum HrpdPcfBearerCommand {
    Register {
        a8: cdma_a8::BearerSession,
        a10: cdma_a10::BearerSession,
    },
    Release {
        a8_id: u32,
        a10_id: Option<u32>,
    },
}

#[derive(Clone)]
pub struct HrpdPcfBearerRuntime {
    tx: tokio::sync::mpsc::UnboundedSender<HrpdPcfBearerCommand>,
}

impl HrpdPcfBearerRuntime {
    pub fn register(&self, a8: cdma_a8::BearerSession, a10: cdma_a10::BearerSession) {
        if self
            .tx
            .send(HrpdPcfBearerCommand::Register { a8, a10 })
            .is_err()
        {
            warn!("HRPD PCF bearer relay stopped before session registration");
        }
    }

    pub fn release(&self, a8_id: u32, a10_id: Option<u32>) {
        if self
            .tx
            .send(HrpdPcfBearerCommand::Release { a8_id, a10_id })
            .is_err()
        {
            warn!("HRPD PCF bearer relay stopped before session release");
        }
    }
}

fn apply_hrpd_pcf_bearer_command(
    command: HrpdPcfBearerCommand,
    a8_table: &mut cdma_a8::BearerTable,
    a10_table: &mut cdma_a10::BearerTable,
    a8_to_a10: &mut HashMap<u32, u32>,
    a10_to_a8: &mut HashMap<u32, u32>,
) {
    match command {
        HrpdPcfBearerCommand::Register { a8, a10 } => {
            let a8_id = a8.session_id;
            let a10_id = a10.session_id;
            match a8_table.apply_session(a8) {
                Ok(a8_outcome) => match a10_table.apply_session(a10) {
                    Ok(a10_outcome) => {
                        a8_to_a10.insert(a8_id, a10_id);
                        a10_to_a8.insert(a10_id, a8_id);
                        info!(
                            "HRPD PCF bearer: registered A8=0x{a8_id:08x} A10=0x{a10_id:08x} a8={a8_outcome:?} a10={a10_outcome:?}"
                        );
                    }
                    Err(err) => {
                        warn!("HRPD PCF bearer: failed A10 register session=0x{a10_id:08x}: {err}")
                    }
                },
                Err(err) => {
                    warn!("HRPD PCF bearer: failed A8 register session=0x{a8_id:08x}: {err}")
                }
            }
        }
        HrpdPcfBearerCommand::Release { a8_id, a10_id } => {
            let mapped_a10_id = a8_to_a10.remove(&a8_id);
            let a10_id = a10_id.or(mapped_a10_id);
            if let Some(a10_id) = a10_id {
                a10_to_a8.remove(&a10_id);
                a10_table.remove_session_if_present(a10_id);
            }
            a8_table.remove_session_if_present(a8_id);
            info!(
                "HRPD PCF bearer: released A8=0x{a8_id:08x} A10={}",
                a10_id
                    .map(|id| format!("0x{id:08x}"))
                    .unwrap_or_else(|| "unknown".to_string())
            );
        }
    }
}

fn drain_hrpd_pcf_bearer_commands(
    rx: &mut tokio::sync::mpsc::UnboundedReceiver<HrpdPcfBearerCommand>,
    a8_table: &mut cdma_a8::BearerTable,
    a10_table: &mut cdma_a10::BearerTable,
    a8_to_a10: &mut HashMap<u32, u32>,
    a10_to_a8: &mut HashMap<u32, u32>,
) -> usize {
    let mut drained = 0;
    while let Ok(command) = rx.try_recv() {
        drained += 1;
        apply_hrpd_pcf_bearer_command(command, a8_table, a10_table, a8_to_a10, a10_to_a8);
    }
    drained
}

pub fn spawn_hrpd_pcf_bearer_relay(
    a8_config: cdma_a8::BearerTransportConfig,
    a8_endpoint: cdma_a8::BearerEndpoint,
    a10_config: cdma_a10::BearerTransportConfig,
    a10_endpoint: cdma_a10::BearerEndpoint,
) -> Result<HrpdPcfBearerRuntime, Error> {
    let a8 = cdma_a8::UdpGreEndpoint::bind(a8_config, "pcf.a8_bearer")
        .map_err(|err| Error::from(format!("HRPD PCF A8 bind failed: {err}")))?
        .into_tokio()
        .map_err(|err| Error::from(format!("HRPD PCF A8 Tokio setup failed: {err}")))?;
    let a10 = cdma_a8::UdpGreEndpoint::bind(a10_config, "pcf.a10_bearer")
        .map_err(|err| Error::from(format!("HRPD PCF A10 bind failed: {err}")))?
        .into_tokio()
        .map_err(|err| Error::from(format!("HRPD PCF A10 Tokio setup failed: {err}")))?;
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        let mut a8_table = cdma_a8::BearerTable::new();
        let mut a10_table = cdma_a10::BearerTable::new();
        let mut a8_to_a10: HashMap<u32, u32> = HashMap::new();
        let mut a10_to_a8: HashMap<u32, u32> = HashMap::new();
        let mut buf = vec![0u8; 8192];
        info!("HRPD PCF A8/A10 bearer relay started");
        loop {
            let mut pass_active = drain_hrpd_pcf_bearer_commands(
                &mut rx,
                &mut a8_table,
                &mut a10_table,
                &mut a8_to_a10,
                &mut a10_to_a8,
            ) > 0;

            for _ in 0..HRPD_BEARER_MAX_DATAGRAMS_PER_PASS {
                if !relay_one_a8_to_a10(
                    &a8,
                    &a10,
                    &mut a8_table,
                    &mut a10_table,
                    a8_endpoint,
                    &a8_to_a10,
                    &mut buf,
                )
                .await
                {
                    break;
                }
                pass_active = true;
            }
            pass_active |= drain_hrpd_pcf_bearer_commands(
                &mut rx,
                &mut a8_table,
                &mut a10_table,
                &mut a8_to_a10,
                &mut a10_to_a8,
            ) > 0;
            for _ in 0..HRPD_BEARER_MAX_DATAGRAMS_PER_PASS {
                if !relay_one_a10_to_a8(
                    &a10,
                    &a8,
                    &mut rx,
                    &mut a10_table,
                    &mut a8_table,
                    a10_endpoint,
                    &mut a8_to_a10,
                    &mut a10_to_a8,
                    &mut buf,
                )
                .await
                {
                    break;
                }
                pass_active = true;
            }
            if !pass_active {
                tokio::select! {
                    command = rx.recv() => {
                        let Some(command) = command else {
                            info!("HRPD PCF A8/A10 bearer relay stopped");
                            return;
                        };
                        apply_hrpd_pcf_bearer_command(
                            command,
                            &mut a8_table,
                            &mut a10_table,
                            &mut a8_to_a10,
                            &mut a10_to_a8,
                        );
                    }
                    result = a8.readable() => {
                        if let Err(err) = result {
                            warn!("HRPD PCF A8 readiness failed: {err}");
                            return;
                        }
                    }
                    result = a10.readable() => {
                        if let Err(err) = result {
                            warn!("HRPD PCF A10 readiness failed: {err}");
                            return;
                        }
                    }
                }
            }
        }
    });
    Ok(HrpdPcfBearerRuntime { tx })
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
        || err.contains("timed out")
        || err.contains("resource temporarily unavailable")
}

async fn relay_one_a8_to_a10(
    a8_rx: &cdma_a8::TokioUdpGreEndpoint,
    a10_tx: &cdma_a8::TokioUdpGreEndpoint,
    a8_table: &mut cdma_a8::BearerTable,
    a10_table: &mut cdma_a10::BearerTable,
    a8_endpoint: cdma_a8::BearerEndpoint,
    a8_to_a10: &HashMap<u32, u32>,
    buf: &mut [u8],
) -> bool {
    let Some(wire) = recv_udp_gre_packet(a8_rx, buf, "HRPD PCF A8") else {
        return false;
    };
    let inbound = match a8_table.decode_for_session(a8_endpoint, &wire) {
        Ok(inbound) => inbound,
        Err(err) => {
            warn!("HRPD PCF A8: bearer packet rejected: {err}");
            return true;
        }
    };
    let Some(a10_session) = a8_to_a10.get(&inbound.session_id).copied() else {
        warn!(
            "HRPD PCF A8: no A10 mapping for A8 session=0x{:08x}",
            inbound.session_id
        );
        return true;
    };
    let outbound = match a10_table.build_outbound_packet(a10_session, inbound.payload) {
        Ok(outbound) => outbound,
        Err(err) => {
            warn!("HRPD PCF A10: failed to encode outbound packet: {err}");
            return true;
        }
    };
    if let Err(err) = a10_tx.send_wire_packet(&outbound.wire_bytes).await {
        warn!("HRPD PCF A10: send failed: {err}");
    }
    true
}

async fn relay_one_a10_to_a8(
    a10_rx: &cdma_a8::TokioUdpGreEndpoint,
    a8_tx: &cdma_a8::TokioUdpGreEndpoint,
    command_rx: &mut tokio::sync::mpsc::UnboundedReceiver<HrpdPcfBearerCommand>,
    a10_table: &mut cdma_a10::BearerTable,
    a8_table: &mut cdma_a8::BearerTable,
    a10_endpoint: cdma_a10::BearerEndpoint,
    a8_to_a10: &mut HashMap<u32, u32>,
    a10_to_a8: &mut HashMap<u32, u32>,
    buf: &mut [u8],
) -> bool {
    let Some(wire) = recv_udp_gre_packet(a10_rx, buf, "HRPD PCF A10") else {
        return false;
    };
    let inbound = match a10_table.decode_for_session(a10_endpoint, &wire) {
        Ok(inbound) => inbound,
        Err(err) => {
            let drained = drain_hrpd_pcf_bearer_commands(
                command_rx, a8_table, a10_table, a8_to_a10, a10_to_a8,
            );
            if drained == 0 {
                warn!("HRPD PCF A10: bearer packet rejected: {err}");
                return true;
            }
            match a10_table.decode_for_session(a10_endpoint, &wire) {
                Ok(inbound) => inbound,
                Err(retry_err) => {
                    warn!(
                        "HRPD PCF A10: bearer packet rejected after applying {drained} pending registration(s): {retry_err}"
                    );
                    return true;
                }
            }
        }
    };
    let Some(a8_session) = a10_to_a8.get(&inbound.session_id).copied() else {
        warn!(
            "HRPD PCF A10: no A8 mapping for A10 session=0x{:08x}",
            inbound.session_id
        );
        return true;
    };
    let outbound = match a8_table.build_outbound_packet(a8_session, inbound.payload) {
        Ok(outbound) => outbound,
        Err(err) => {
            warn!("HRPD PCF A8: failed to encode outbound packet: {err}");
            return true;
        }
    };
    if let Err(err) = a8_tx.send_wire_packet(&outbound.wire_bytes).await {
        warn!("HRPD PCF A8: send failed: {err}");
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{SocketAddr, UdpSocket};
    use std::time::Duration;

    fn free_udp_addr() -> SocketAddr {
        UdpSocket::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
    }

    #[tokio::test]
    async fn bearer_relay_drains_multiple_bounded_batches_in_both_directions() {
        const DATAGRAMS: u32 = 160;
        let an_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let pdsn_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let an_addr = an_socket.local_addr().unwrap();
        let pdsn_addr = pdsn_socket.local_addr().unwrap();
        let pcf_a8_addr = free_udp_addr();
        let pcf_a10_addr = loop {
            let candidate = free_udp_addr();
            if candidate != pcf_a8_addr {
                break candidate;
            }
        };
        let a8_endpoint = cdma_a8::BearerEndpoint::new([127, 0, 0, 1], [127, 0, 0, 1]);
        let a10_endpoint = cdma_a10::BearerEndpoint::new([127, 0, 0, 1], [127, 0, 0, 1]);
        let a8_session_id = 0x8005_8001;
        let a10_session_id = 0x0000_0001;
        let a8_key = 0xa800_0001;
        let a10_key = 0xa100_0001;
        let runtime = spawn_hrpd_pcf_bearer_relay(
            cdma_a8::BearerTransportConfig::udp_encapsulated_gre(pcf_a8_addr, an_addr),
            a8_endpoint,
            cdma_a10::BearerTransportConfig::udp_encapsulated_gre(pcf_a10_addr, pdsn_addr),
            a10_endpoint,
        )
        .unwrap();
        runtime.register(
            cdma_a8::BearerSession::with_directional_keys(
                a8_session_id,
                a8_key,
                a8_key,
                a8_endpoint,
                cdma_a8::BearerProfile::standard_packet_data(),
            ),
            cdma_a10::BearerSession::with_directional_keys(
                a10_session_id,
                a10_key,
                a10_key,
                a10_endpoint,
                cdma_a10::BearerProfile::standard_packet_data(),
            ),
        );
        let an = cdma_a8::UdpGreEndpoint::from_socket(an_socket, pcf_a8_addr)
            .into_tokio()
            .unwrap();
        let pdsn = cdma_a8::UdpGreEndpoint::from_socket(pdsn_socket, pcf_a10_addr)
            .into_tokio()
            .unwrap();
        tokio::time::sleep(Duration::from_millis(10)).await;

        for sequence in 0..DATAGRAMS {
            an.send_gre_packet(&cdma_a8::GrePacket::octet_stream(
                a8_key,
                Some(sequence),
                sequence.to_be_bytes(),
            ))
            .await
            .unwrap();
        }
        tokio::time::timeout(Duration::from_secs(2), async {
            let mut buf = [0_u8; 128];
            for sequence in 0..DATAGRAMS {
                let (packet, _) = pdsn.recv_gre_packet(&mut buf).await.unwrap();
                assert_eq!(packet.key, Some(a10_key));
                assert_eq!(packet.payload, sequence.to_be_bytes());
            }
        })
        .await
        .unwrap();

        for sequence in 0..DATAGRAMS {
            pdsn.send_gre_packet(&cdma_a8::GrePacket::octet_stream(
                a10_key,
                Some(sequence),
                sequence.to_le_bytes(),
            ))
            .await
            .unwrap();
        }
        tokio::time::timeout(Duration::from_secs(2), async {
            let mut buf = [0_u8; 128];
            for sequence in 0..DATAGRAMS {
                let (packet, _) = an.recv_gre_packet(&mut buf).await.unwrap();
                assert_eq!(packet.key, Some(a8_key));
                assert_eq!(packet.payload, sequence.to_le_bytes());
            }
        })
        .await
        .unwrap();
    }
}
