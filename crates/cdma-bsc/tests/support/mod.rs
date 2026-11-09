//! A real PCF and PDSN on loopback GRE-in-UDP for BSC packet-data tests.

use std::net::SocketAddr;
use std::sync::Arc;

use cdma_bsc::a9_edge::PacketA9Config;
use cdma_packet::grpc::PacketServiceImpl;

pub struct PacketCore {
    pub packet_a9: PacketA9Config,
    pub packet_service: Arc<PacketServiceImpl>,
}

fn free_udp_addr() -> SocketAddr {
    std::net::UdpSocket::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
}

fn a11_security() -> cdma_a11::A11SecurityConfig {
    cdma_a11::A11SecurityConfig {
        spi: 256,
        shared_secret_hex: "31786274732d6131312d7368617265642d736563726574".to_string(),
    }
}

pub async fn start_packet_core() -> PacketCore {
    let pcf_a8 = free_udp_addr();
    let bsc_a8 = free_udp_addr();
    let pcf_a10 = free_udp_addr();
    let pdsn_a10 = free_udp_addr();
    let mut packet = cdma_pdsn::PacketTransportConfig::default();
    packet.transport = "fou_tcp".to_string();
    packet.fou_remote = Some("127.0.0.1:17012".to_string());
    let pdsn_config = cdma_pdsn::PdsnNodeConfig {
        packet_grpc_listen_addr: "127.0.0.1:0".parse().unwrap(),
        a10_bearer: cdma_a10::BearerTransportConfig::udp_encapsulated_gre(pdsn_a10, pcf_a10),
        a11: cdma_a11::A11TransportConfig::new(
            "127.0.0.1:0".parse().unwrap(),
            "127.0.0.1:0".parse().unwrap(),
        ),
        a11_security: a11_security(),
        a11_peers: Vec::new(),
        packet,
        ppp_session_timeout_secs: 1_800,
        events_endpoint: None,
        a12: Default::default(),
        hlr_endpoint: None,
    };
    let packet_service = cdma_pdsn::build_packet_service_with_sink(&pdsn_config, None).unwrap();
    let pdsn_a11 = cdma_pdsn::spawn_pdsn_a11_service(pdsn_config, packet_service.clone())
        .await
        .unwrap();

    let pcf_config = cdma_pcf::PcfNodeConfig {
        a9_bind_addr: "127.0.0.1:0".parse().unwrap(),
        a9_transport: cdma_a9::A9Transport::Udp,
        management_bind_addr: "127.0.0.1:0".parse().unwrap(),
        a8_bearer: cdma_a8::BearerTransportConfig::udp_encapsulated_gre(pcf_a8, bsc_a8),
        a10_bearer: cdma_a10::BearerTransportConfig::udp_encapsulated_gre(pcf_a10, pdsn_a10),
        a11: cdma_a11::A11TransportConfig::new("127.0.0.1:0".parse().unwrap(), pdsn_a11),
        a11_security: a11_security(),
    };
    let pcf = cdma_pcf::spawn_hrpd_pcf_a9_service(pcf_config)
        .await
        .unwrap();
    PacketCore {
        packet_a9: PacketA9Config {
            pcf_addr: pcf.pcf_addr,
            a9_transport: pcf.a9_transport,
            pcf_a8_addr: pcf_a8,
            a8_bind_addr: bsc_a8,
            a8_transport: cdma_a8::BearerTransportMode::UdpEncapsulatedGre,
            max_gre_payload: None,
        },
        packet_service,
    }
}
