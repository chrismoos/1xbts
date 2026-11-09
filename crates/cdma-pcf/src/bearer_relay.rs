use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, SocketAddr};

use cdma_common::error::Error;
use log::{info, warn};

const A10_BEARER_MAX_DATAGRAMS_PER_PASS: usize = 64;

/// Setup-A8 includes only the AN's IP (A.S0016-D §4.2.16), so a GRE-in-UDP port
/// is learned from reverse traffic and reused for that A9 client.
#[derive(Default)]
struct A8Peers {
    by_session: HashMap<u32, SocketAddr>,
    client_of_session: HashMap<u32, SocketAddr>,
    by_client: HashMap<SocketAddr, SocketAddr>,
}

impl A8Peers {
    fn register(&mut self, a8_id: u32, peer: SocketAddr, client: SocketAddr) {
        let peer = self
            .by_client
            .get(&client)
            .copied()
            .filter(|learned| learned.ip() == peer.ip())
            .unwrap_or(peer);
        // A refresh must preserve a learned UDP port on the same signaling host.
        match self.by_session.get_mut(&a8_id) {
            Some(learned) if learned.ip() == peer.ip() => {}
            Some(learned) => *learned = peer,
            None => {
                self.by_session.insert(a8_id, peer);
            }
        }
        self.client_of_session.insert(a8_id, client);
    }

    fn learn(&mut self, a8_id: u32, from: SocketAddr) {
        self.by_session.insert(a8_id, from);
        if let Some(client) = self.client_of_session.get(&a8_id) {
            self.by_client.insert(*client, from);
        }
    }

    fn get(&self, a8_id: u32) -> Option<SocketAddr> {
        self.by_session.get(&a8_id).copied()
    }

    fn remove(&mut self, a8_id: u32) {
        self.by_session.remove(&a8_id);
        self.client_of_session.remove(&a8_id);
    }
}

/// Raw GRE sockets on one address each receive every GRE frame sent to it.
struct RelayLinks {
    a8: cdma_a8::GreSocket,
    a10: cdma_a8::GreSocket,
    a10_peer: SocketAddr,
    pdsn_ip: IpAddr,
}

impl RelayLinks {
    fn recv_a8(&self, buf: &mut [u8]) -> RelayRecv {
        let Some((wire, from)) = self.a8.try_recv_wire(buf, "HRPD PCF A8") else {
            return RelayRecv::Empty;
        };
        if self.a8.mode() == cdma_a8::BearerTransportMode::RawGre && from.ip() == self.pdsn_ip {
            return RelayRecv::NotForThisLink;
        }
        RelayRecv::Frame(wire, from)
    }

    fn recv_a10(&self, buf: &mut [u8]) -> RelayRecv {
        let Some((wire, from)) = self.a10.try_recv_wire(buf, "HRPD PCF A10") else {
            return RelayRecv::Empty;
        };
        if self.a10.mode() == cdma_a8::BearerTransportMode::RawGre && from.ip() != self.pdsn_ip {
            return RelayRecv::NotForThisLink;
        }
        RelayRecv::Frame(wire, from)
    }
}

/// Only Segmentation Indication describes the user traffic.
fn reverse_attributes_for_a10(
    attributes: Vec<cdma_a8::GreAttribute>,
) -> Vec<cdma_a8::GreAttribute> {
    attributes
        .into_iter()
        .filter(|attribute| matches!(attribute, cdma_a8::GreAttribute::Segmentation(_)))
        .collect()
}

/// A.S0012-D §3.4.3 allows only IP Flow Discriminator and Segmentation
/// Indication on A8.
fn forward_attributes_for_a8(attributes: Vec<cdma_a8::GreAttribute>) -> Vec<cdma_a8::GreAttribute> {
    attributes
        .into_iter()
        .filter(|attribute| {
            matches!(
                attribute,
                cdma_a8::GreAttribute::IpFlowDiscriminator { .. }
                    | cdma_a8::GreAttribute::Segmentation(_)
            )
        })
        .collect()
}

enum RelayRecv {
    Empty,
    NotForThisLink,
    Frame(Vec<u8>, SocketAddr),
}

enum HrpdPcfBearerCommand {
    Register {
        a8: cdma_a8::BearerSession,
        a10: cdma_a10::BearerSession,
        peer: std::net::SocketAddr,
        client: std::net::SocketAddr,
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
    pub fn register(
        &self,
        a8: cdma_a8::BearerSession,
        a10: cdma_a10::BearerSession,
        peer: std::net::SocketAddr,
        client: std::net::SocketAddr,
    ) {
        if self
            .tx
            .send(HrpdPcfBearerCommand::Register {
                a8,
                a10,
                peer,
                client,
            })
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
    a8_peers: &mut A8Peers,
) {
    match command {
        HrpdPcfBearerCommand::Register {
            a8,
            a10,
            peer,
            client,
        } => {
            let a8_id = a8.session_id;
            let a10_id = a10.session_id;
            match a8_table.apply_session(a8) {
                Ok(a8_outcome) => match a10_table.apply_session(a10) {
                    Ok(a10_outcome) => {
                        a8_peers.register(a8_id, peer, client);
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
            // The return address dies with the session, so a later session
            // reusing this GRE key learns its own access network from its
            // first packet.
            a8_peers.remove(a8_id);
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
    a8_peers: &mut A8Peers,
) -> usize {
    let mut drained = 0;
    while let Ok(command) = rx.try_recv() {
        drained += 1;
        apply_hrpd_pcf_bearer_command(command, a8_table, a10_table, a8_to_a10, a10_to_a8, a8_peers);
    }
    drained
}

pub fn spawn_hrpd_pcf_bearer_relay(
    a8_config: cdma_a8::BearerTransportConfig,
    a8_endpoint: cdma_a8::BearerEndpoint,
    a10_config: cdma_a10::BearerTransportConfig,
    a10_endpoint: cdma_a10::BearerEndpoint,
) -> Result<HrpdPcfBearerRuntime, Error> {
    let a8 = cdma_a8::GreSocket::bind(a8_config, a8_endpoint.local_ip, "pcf.a8_bearer")
        .map_err(|err| Error::from(format!("HRPD PCF A8 bind failed: {err}")))?;
    let a10_local_ip = IpAddr::V4(Ipv4Addr::from(a10_endpoint.local_ipv4));
    let a10 = cdma_a8::GreSocket::bind(a10_config, a10_local_ip, "pcf.a10_bearer")
        .map_err(|err| Error::from(format!("HRPD PCF A10 bind failed: {err}")))?;
    let pdsn_ip = IpAddr::V4(Ipv4Addr::from(a10_endpoint.remote_ipv4));
    let a10_peer = match a10_config.mode {
        cdma_a10::BearerTransportMode::RawGre => SocketAddr::new(pdsn_ip, 0),
        cdma_a10::BearerTransportMode::UdpEncapsulatedGre => a10_config
            .udp_peer_addr
            .ok_or_else(|| Error::from("pcf.a10_bearer: GRE-in-UDP requires udp_peer_addr"))?,
    };
    let links = RelayLinks {
        a8,
        a10,
        a10_peer,
        pdsn_ip,
    };
    let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
    tokio::spawn(async move {
        let mut a8_table = cdma_a8::BearerTable::new();
        let mut a10_table = cdma_a10::BearerTable::new();
        let mut a8_to_a10: HashMap<u32, u32> = HashMap::new();
        let mut a10_to_a8: HashMap<u32, u32> = HashMap::new();
        let mut a8_peers = A8Peers::default();
        let mut buf = vec![0u8; 8192];
        info!("HRPD PCF A8/A10 bearer relay started");
        loop {
            let mut pass_active = drain_hrpd_pcf_bearer_commands(
                &mut rx,
                &mut a8_table,
                &mut a10_table,
                &mut a8_to_a10,
                &mut a10_to_a8,
                &mut a8_peers,
            ) > 0;

            for _ in 0..A10_BEARER_MAX_DATAGRAMS_PER_PASS {
                if !relay_one_a8_to_a10(
                    &links,
                    &mut a8_table,
                    &mut a10_table,
                    a8_endpoint,
                    &a8_to_a10,
                    &mut a8_peers,
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
                &mut a8_peers,
            ) > 0;
            for _ in 0..A10_BEARER_MAX_DATAGRAMS_PER_PASS {
                if !relay_one_a10_to_a8(
                    &links,
                    &mut rx,
                    &mut a10_table,
                    &mut a8_table,
                    a10_endpoint,
                    &mut a8_to_a10,
                    &mut a10_to_a8,
                    &mut a8_peers,
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
                            &mut a8_peers,
                        );
                    }
                    result = links.a8.readable() => {
                        if let Err(err) = result {
                            warn!("HRPD PCF A8 readiness failed: {err}");
                            return;
                        }
                    }
                    result = links.a10.readable() => {
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

async fn relay_one_a8_to_a10(
    links: &RelayLinks,
    a8_table: &mut cdma_a8::BearerTable,
    a10_table: &mut cdma_a10::BearerTable,
    a8_endpoint: cdma_a8::BearerEndpoint,
    a8_to_a10: &HashMap<u32, u32>,
    a8_peers: &mut A8Peers,
    buf: &mut [u8],
) -> bool {
    let (wire, from) = match links.recv_a8(buf) {
        RelayRecv::Empty => return false,
        RelayRecv::NotForThisLink => return true,
        RelayRecv::Frame(wire, from) => (wire, from),
    };
    let inbound = match a8_table.decode_for_session(a8_endpoint, &wire) {
        Ok(inbound) => inbound,
        Err(err) => {
            warn!("HRPD PCF A8: bearer packet rejected: {err}");
            return true;
        }
    };
    a8_peers.learn(inbound.session_id, from);
    let Some(a10_session) = a8_to_a10.get(&inbound.session_id).copied() else {
        warn!(
            "HRPD PCF A8: no A10 mapping for A8 session=0x{:08x}",
            inbound.session_id
        );
        return true;
    };
    let attributes = reverse_attributes_for_a10(inbound.attributes);
    if inbound.payload.is_empty() && attributes.is_empty() {
        return true;
    }
    let outbound = match a10_table.build_outbound_packet_with_attributes(
        a10_session,
        inbound.payload,
        attributes,
    ) {
        Ok(outbound) => outbound,
        Err(err) => {
            warn!("HRPD PCF A10: failed to encode outbound packet: {err}");
            return true;
        }
    };
    if let Err(err) = links
        .a10
        .send_wire_to(&outbound.wire_bytes, links.a10_peer)
        .await
    {
        warn!("HRPD PCF A10: send failed: {err}");
    }
    true
}

async fn relay_one_a10_to_a8(
    links: &RelayLinks,
    command_rx: &mut tokio::sync::mpsc::UnboundedReceiver<HrpdPcfBearerCommand>,
    a10_table: &mut cdma_a10::BearerTable,
    a8_table: &mut cdma_a8::BearerTable,
    a10_endpoint: cdma_a10::BearerEndpoint,
    a8_to_a10: &mut HashMap<u32, u32>,
    a10_to_a8: &mut HashMap<u32, u32>,
    a8_peers: &mut A8Peers,
    buf: &mut [u8],
) -> bool {
    let wire = match links.recv_a10(buf) {
        RelayRecv::Empty => return false,
        RelayRecv::NotForThisLink => return true,
        RelayRecv::Frame(wire, _) => wire,
    };
    let inbound = match a10_table.decode_for_session(a10_endpoint, &wire) {
        Ok(inbound) => inbound,
        Err(err) => {
            let drained = drain_hrpd_pcf_bearer_commands(
                command_rx, a8_table, a10_table, a8_to_a10, a10_to_a8, a8_peers,
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
    for attribute in &inbound.attributes {
        if let cdma_a8::GreAttribute::FlowControl(_) | cdma_a8::GreAttribute::ShortDataIndication =
            attribute
        {
            log::debug!(
                "HRPD PCF A10: GRE attribute A10=0x{:08x} {attribute:?}",
                inbound.session_id
            );
        }
    }
    let attributes = forward_attributes_for_a8(inbound.attributes);
    if inbound.payload.is_empty() && attributes.is_empty() {
        return true;
    }
    let outbound = match a8_table.build_outbound_packet_with_attributes(
        a8_session,
        inbound.payload,
        attributes,
    ) {
        Ok(outbound) => outbound,
        Err(err) => {
            warn!("HRPD PCF A8: failed to encode outbound packet: {err}");
            return true;
        }
    };
    let Some(peer) = a8_peers.get(a8_session) else {
        warn!("HRPD PCF A8: no return address for A8 session=0x{a8_session:08x}, dropping");
        return true;
    };
    if let Err(err) = links.a8.send_wire_to(&outbound.wire_bytes, peer).await {
        warn!("HRPD PCF A8: send to {peer} failed: {err}");
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::UdpSocket;
    use std::time::Duration;

    fn free_udp_addr() -> SocketAddr {
        UdpSocket::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap()
    }

    #[tokio::test]
    async fn downlink_starts_before_uplink_and_preserves_learned_peers_on_refresh() {
        const A8_ID: u32 = 11;
        const A10_ID: u32 = 21;
        const OTHER_A8_ID: u32 = 12;
        const OTHER_A10_ID: u32 = 22;
        const LOOPBACK: [u8; 4] = [127, 0, 0, 1];
        const RECV_TIMEOUT: Duration = Duration::from_secs(2);
        let an_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let learned_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let other_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let pdsn_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let an_addr = an_socket.local_addr().unwrap();
        let other_addr = other_socket.local_addr().unwrap();
        let pdsn_addr = pdsn_socket.local_addr().unwrap();
        let pcf_a8_addr = free_udp_addr();
        let pcf_a10_addr = free_udp_addr();
        let endpoint = cdma_a8::BearerEndpoint::new(LOOPBACK, LOOPBACK);
        let a10_endpoint = cdma_a10::BearerEndpoint::new(LOOPBACK, LOOPBACK);
        let runtime = spawn_hrpd_pcf_bearer_relay(
            cdma_a8::BearerTransportConfig::udp_encapsulated_gre(pcf_a8_addr, an_addr),
            endpoint,
            cdma_a10::BearerTransportConfig::udp_encapsulated_gre(pcf_a10_addr, pdsn_addr),
            a10_endpoint,
        )
        .unwrap();
        let register = |a8_id, a10_id, peer| {
            runtime.register(
                cdma_a8::BearerSession::new(a8_id, endpoint),
                cdma_a10::BearerSession::new(a10_id, a10_endpoint),
                peer,
                peer,
            )
        };
        register(A8_ID, A10_ID, an_addr);
        register(OTHER_A8_ID, OTHER_A10_ID, other_addr);
        let an = cdma_a8::UdpGreEndpoint::from_socket(an_socket, pcf_a8_addr)
            .into_tokio()
            .unwrap();
        let learned = cdma_a8::UdpGreEndpoint::from_socket(learned_socket, pcf_a8_addr)
            .into_tokio()
            .unwrap();
        let other = cdma_a8::UdpGreEndpoint::from_socket(other_socket, pcf_a8_addr)
            .into_tokio()
            .unwrap();
        let pdsn = cdma_a8::UdpGreEndpoint::from_socket(pdsn_socket, pcf_a10_addr)
            .into_tokio()
            .unwrap();
        let mut buf = [0_u8; 128];
        pdsn.send_gre_packet(&cdma_a8::GrePacket::octet_stream(A10_ID, Some(0), [1]))
            .await
            .unwrap();
        let (packet, _) = tokio::time::timeout(RECV_TIMEOUT, an.recv_gre_packet(&mut buf))
            .await
            .unwrap()
            .unwrap();
        assert_eq!((packet.key, packet.payload), (Some(A8_ID), vec![1]));
        pdsn.send_gre_packet(&cdma_a8::GrePacket::octet_stream(
            OTHER_A10_ID,
            Some(0),
            [2],
        ))
        .await
        .unwrap();
        let (packet, _) = tokio::time::timeout(RECV_TIMEOUT, other.recv_gre_packet(&mut buf))
            .await
            .unwrap()
            .unwrap();
        assert_eq!((packet.key, packet.payload), (Some(OTHER_A8_ID), vec![2]));
        learned
            .send_gre_packet(&cdma_a8::GrePacket::octet_stream(A8_ID, Some(0), [3]))
            .await
            .unwrap();
        let (packet, _) = tokio::time::timeout(RECV_TIMEOUT, pdsn.recv_gre_packet(&mut buf))
            .await
            .unwrap()
            .unwrap();
        assert_eq!(packet.payload, [3]);
        register(A8_ID, A10_ID, an_addr);
        pdsn.send_gre_packet(&cdma_a8::GrePacket::octet_stream(A10_ID, Some(1), [4]))
            .await
            .unwrap();
        let (packet, _) = tokio::time::timeout(RECV_TIMEOUT, learned.recv_gre_packet(&mut buf))
            .await
            .unwrap()
            .unwrap();
        assert_eq!((packet.key, packet.payload), (Some(A8_ID), vec![4]));
    }

    #[tokio::test]
    async fn later_sessions_from_a_client_use_its_learned_port() {
        const FIRST_A8: u32 = 31;
        const FIRST_A10: u32 = 41;
        const SECOND_A8: u32 = 32;
        const SECOND_A10: u32 = 42;
        const LOOPBACK: [u8; 4] = [127, 0, 0, 1];
        const RECV_TIMEOUT: Duration = Duration::from_secs(2);
        let default_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let client_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let pdsn_socket = UdpSocket::bind("127.0.0.1:0").unwrap();
        let default_addr = default_socket.local_addr().unwrap();
        let pdsn_addr = pdsn_socket.local_addr().unwrap();
        let a9_client: SocketAddr = "127.0.0.1:40000".parse().unwrap();
        let pcf_a8_addr = free_udp_addr();
        let pcf_a10_addr = free_udp_addr();
        let endpoint = cdma_a8::BearerEndpoint::new(LOOPBACK, LOOPBACK);
        let a10_endpoint = cdma_a10::BearerEndpoint::new(LOOPBACK, LOOPBACK);
        let runtime = spawn_hrpd_pcf_bearer_relay(
            cdma_a8::BearerTransportConfig::udp_encapsulated_gre(pcf_a8_addr, default_addr),
            endpoint,
            cdma_a10::BearerTransportConfig::udp_encapsulated_gre(pcf_a10_addr, pdsn_addr),
            a10_endpoint,
        )
        .unwrap();
        let client = cdma_a8::UdpGreEndpoint::from_socket(client_socket, pcf_a8_addr)
            .into_tokio()
            .unwrap();
        let pdsn = cdma_a8::UdpGreEndpoint::from_socket(pdsn_socket, pcf_a10_addr)
            .into_tokio()
            .unwrap();
        let mut buf = [0_u8; 128];

        runtime.register(
            cdma_a8::BearerSession::new(FIRST_A8, endpoint),
            cdma_a10::BearerSession::new(FIRST_A10, a10_endpoint),
            default_addr,
            a9_client,
        );
        client
            .send_gre_packet(&cdma_a8::GrePacket::octet_stream(FIRST_A8, Some(0), [1]))
            .await
            .unwrap();
        tokio::time::timeout(RECV_TIMEOUT, pdsn.recv_gre_packet(&mut buf))
            .await
            .unwrap()
            .unwrap();
        runtime.release(FIRST_A8, Some(FIRST_A10));

        runtime.register(
            cdma_a8::BearerSession::new(SECOND_A8, endpoint),
            cdma_a10::BearerSession::new(SECOND_A10, a10_endpoint),
            default_addr,
            a9_client,
        );
        pdsn.send_gre_packet(&cdma_a8::GrePacket::octet_stream(SECOND_A10, Some(0), [2]))
            .await
            .unwrap();
        let (packet, _) = tokio::time::timeout(RECV_TIMEOUT, client.recv_gre_packet(&mut buf))
            .await
            .expect("first forward packet of the second session reaches the client's port")
            .unwrap();
        assert_eq!((packet.key, packet.payload), (Some(SECOND_A8), vec![2]));
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
            an_addr,
            an_addr,
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

    async fn recv_raw(socket: &cdma_a8::GreSocket) -> (cdma_a8::GrePacket, SocketAddr) {
        let mut buf = vec![0u8; 2048];
        loop {
            if let Some((wire, from)) = socket.try_recv_wire(&mut buf, "test") {
                return (cdma_a8::GrePacket::decode(&wire).unwrap(), from);
            }
            socket.readable().await.unwrap();
        }
    }

    /// Skips without CAP_NET_RAW.
    #[tokio::test]
    async fn raw_gre_relay_carries_frames_and_a8_attributes_between_an_and_pdsn() {
        const AN_IP: [u8; 4] = [127, 0, 0, 1];
        const PCF_IP: [u8; 4] = [127, 0, 0, 2];
        const PDSN_IP: [u8; 4] = [127, 0, 0, 3];
        const A8_KEY: u32 = 0x8005_8001;
        const A10_KEY: u32 = 0x0000_0042;
        const RECV_TIMEOUT: Duration = Duration::from_secs(2);
        let an_ip = IpAddr::V4(Ipv4Addr::from(AN_IP));
        let pdsn_ip = IpAddr::V4(Ipv4Addr::from(PDSN_IP));
        let raw = cdma_a8::BearerTransportConfig::raw_gre();
        let an = match cdma_a8::GreSocket::bind(raw, an_ip, "test.an") {
            Ok(socket) => socket,
            Err(err) => {
                eprintln!("skipping raw GRE relay test: {err}");
                return;
            }
        };
        let pdsn = cdma_a8::GreSocket::bind(raw, pdsn_ip, "test.pdsn").unwrap();
        let a8_endpoint = cdma_a8::BearerEndpoint::new(PCF_IP, PCF_IP);
        let a10_endpoint = cdma_a10::BearerEndpoint::new(PCF_IP, PDSN_IP);
        let runtime = spawn_hrpd_pcf_bearer_relay(raw, a8_endpoint, raw, a10_endpoint).unwrap();
        runtime.register(
            cdma_a8::BearerSession::new(A8_KEY, a8_endpoint),
            cdma_a10::BearerSession::new(A10_KEY, a10_endpoint),
            SocketAddr::new(an_ip, 0),
            SocketAddr::new(an_ip, 0),
        );
        tokio::time::sleep(Duration::from_millis(20)).await;
        let pcf = SocketAddr::new(IpAddr::V4(Ipv4Addr::from(PCF_IP)), 0);

        let uplink = cdma_a8::GrePacket::octet_stream(A8_KEY, Some(0), [0x7e, 0x01]);
        an.send_wire_to(&uplink.encode().unwrap(), pcf)
            .await
            .unwrap();
        let (packet, from) = tokio::time::timeout(RECV_TIMEOUT, recv_raw(&pdsn))
            .await
            .unwrap();
        assert_eq!(from.ip(), pcf.ip());
        assert_eq!(
            (packet.key, packet.payload),
            (Some(A10_KEY), vec![0x7e, 0x01])
        );

        let downlink = cdma_a8::GrePacket::with_attributes(
            A10_KEY,
            Some(0),
            vec![
                cdma_a8::GreAttribute::IpFlowDiscriminator { flow_id: 3 },
                cdma_a8::GreAttribute::FlowControl(cdma_a8::FlowControl::Xon),
            ],
            [0x7e, 0x02],
        );
        pdsn.send_wire_to(&downlink.encode().unwrap(), pcf)
            .await
            .unwrap();
        let (packet, from) = tokio::time::timeout(RECV_TIMEOUT, recv_raw(&an))
            .await
            .unwrap();
        assert_eq!(from.ip(), pcf.ip());
        assert_eq!(packet.key, Some(A8_KEY));
        assert_eq!(
            packet.attributes,
            vec![cdma_a8::GreAttribute::IpFlowDiscriminator { flow_id: 3 }]
        );
        assert_eq!(packet.payload, [0x7e, 0x02]);
    }
}
