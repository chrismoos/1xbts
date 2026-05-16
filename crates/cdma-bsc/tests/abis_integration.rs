use std::sync::Arc;
use std::time::Duration;

use tokio::net::TcpListener;

use cdma_abis::bearer::{ChannelFamily, ForwardFchDcchFrame, FrameContent, TrafficFrame};
use cdma_abis::control::typed::CellId;
use cdma_abis::transport::{TransportEvent, accept};
use cdma_bts::bts::TrafficResourceService;
use cdma_bts::bts::abis_agent::{AbisAgent, AbisAgentConfig};
use cdma_bts::phy::coding::long_code::LongCodeGenerator;

use cdma_abis::control::typed::{
    AirInterfaceMessagePayload, MobileIdentity, PchMessageTransferMessage,
};
use cdma_bsc::abis_edge::network::{NetworkBtsControlClient, NetworkClientConfig};
use cdma_bsc::abis_edge::{BearerFrame, BtsControlClient, ForwardBearerQueue};
use cdma_bts::lac::paging_messages::ExtendedChannelAssignmentMessage;

fn agent_config() -> AbisAgentConfig {
    AbisAgentConfig {
        pilot_pn: 0,
        cell_id: CellId {
            cell: 0x100,
            sector: 0x01,
        },
        mscid: 0x001234,
    }
}

fn client_config() -> NetworkClientConfig {
    NetworkClientConfig {
        cell_id: CellId {
            cell: 0x100,
            sector: 0x01,
        },
        mscid: 0x001234,
        pilot_pn: 0,
        auth_mode: 0,
        p_rev_in_use: 6,
        market_id: 100,
        generating_entity_id: 200,
    }
}

/// Abis test harness that manages server and client lifecycle.
///
/// Spawns a BTS-side Abis agent on a loopback TCP listener and provides
/// a `NetworkBtsControlClient` connected to it. The server task processes
/// messages until the client is dropped (triggering TCP disconnect) or until
/// `shutdown` is called. All operations are bounded by a 10s timeout.
struct AbisTestHarness {
    client: Option<NetworkBtsControlClient>,
    server_handle: tokio::task::JoinHandle<AbisAgent>,
    controller: Arc<TrafficResourceService>,
}

impl AbisTestHarness {
    async fn new() -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let controller = Arc::new(TrafficResourceService::new());
        controller
            .walsh_allocator()
            .lock()
            .reserve_system_channels(0, 1, 32);

        let controller_clone = controller.clone();
        let server_handle = tokio::spawn(async move {
            let (sender, mut events_rx) = accept(&listener).await.unwrap();
            let mut agent = AbisAgent::new(agent_config(), controller_clone);

            while let Some(event) = events_rx.recv().await {
                match event {
                    TransportEvent::Message(msg) => {
                        let (responses, _events) = agent.handle_message(&msg);
                        for resp in responses {
                            let _ = sender.send(&resp).await;
                        }
                    }
                    TransportEvent::Disconnected(_) => break,
                }
            }

            agent
        });

        let client = NetworkBtsControlClient::connect(addr, client_config())
            .await
            .unwrap();

        Self {
            client: Some(client),
            server_handle,
            controller,
        }
    }

    fn client(&self) -> &NetworkBtsControlClient {
        self.client.as_ref().unwrap()
    }

    /// Drops the client, waits for the server to observe the disconnect,
    /// and returns the server-side agent for assertions.
    async fn shutdown(mut self) -> AbisAgent {
        self.client.take();
        tokio::time::timeout(Duration::from_secs(10), self.server_handle)
            .await
            .expect("server should shut down within 10s")
            .expect("server task should not panic")
    }
}

/// Full BtsSetup -> Connect -> ConnectAck -> BtsSetupAck flow over TCP loopback.
/// Two-phase: allocate reserves walsh, ECAM commit creates the channel.
#[tokio::test]
async fn abis_tcp_setup_allocates_walsh() {
    let _ = env_logger::try_init();

    let harness = AbisTestHarness::new().await;

    let lc = LongCodeGenerator::new_traffic_channel(0xDEAD);
    let result = harness.client().allocate_rc1_traffic(lc, 0, 0xDEAD).await;
    assert!(
        result.is_some(),
        "network client should return an opaque bearer handle"
    );
    let handle = result.unwrap();

    let controller = harness.controller.clone();
    // Phase 1: walsh reserved, session created, but pool empty until ECAM
    assert_eq!(controller.traffic_channels_pool().len(), 0);

    // Phase 2: send ECAM to commit the traffic channel
    send_ecam_commit(harness.client(), handle.walsh_code);
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(controller.traffic_channels_pool().len(), 1);

    let agent = harness.shutdown().await;
    assert_eq!(agent.active_session_count(), 1);
}

/// Full setup -> ECAM commit -> deallocate flow over Abis.
#[tokio::test]
async fn abis_tcp_setup_then_release() {
    let _ = env_logger::try_init();

    let harness = AbisTestHarness::new().await;

    let lc = LongCodeGenerator::new_traffic_channel(0xBEEF);
    let handle = harness
        .client()
        .allocate_rc1_traffic(lc, 0, 0xBEEF)
        .await
        .expect("allocation should succeed");

    // Commit the channel via ECAM
    send_ecam_commit(harness.client(), handle.walsh_code);
    tokio::time::sleep(Duration::from_millis(50)).await;

    let controller = harness.controller.clone();
    assert_eq!(controller.traffic_channels_pool().len(), 1);

    // Deallocate sends BtsRelease + Remove over the wire; the agent
    // processes both and frees the walsh code back to the pool.
    harness.client().deallocate_traffic(handle.walsh_code).await;

    // Give the agent time to process the Remove response.
    tokio::time::sleep(Duration::from_millis(50)).await;

    let agent = harness.shutdown().await;
    assert_eq!(agent.active_session_count(), 0);
    assert!(controller.traffic_channels_pool().is_empty());
}

/// Full network path for traffic: TCP Abis setup plus UDP bearer delivery into
/// the BTS-owned forward traffic queue.
#[tokio::test]
async fn abis_tcp_setup_and_udp_bearer_delivers_forward_fch() {
    let _ = env_logger::try_init();

    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();

    let controller = Arc::new(TrafficResourceService::new());
    controller
        .walsh_allocator()
        .lock()
        .reserve_system_channels(0, 1, 32);
    let (_reverse_tx, reverse_rx) = std::sync::mpsc::channel();

    // Use ephemeral ports for test isolation.
    let bts_bearer_config = cdma_abis::bearer_transport::BearerTransportConfig {
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        remote_addr: "127.0.0.1:0".parse().unwrap(), // unused by this forward-only test
        bts_id: 1,
        cell_id: 1,
    };
    let bts_bearer = std::sync::Arc::new(
        cdma_abis::bearer_transport::BearerTransport::new(&bts_bearer_config)
            .expect("BTS bearer bind"),
    );
    let bts_bearer_addr = bts_bearer.local_addr().unwrap();

    let bsc_bearer_config = cdma_abis::bearer_transport::BearerTransportConfig {
        bind_addr: "127.0.0.1:0".parse().unwrap(),
        remote_addr: bts_bearer_addr,
        bts_id: 1,
        cell_id: 1,
    };
    let bsc_bearer = std::sync::Arc::new(
        cdma_abis::bearer_transport::BearerTransport::new(&bsc_bearer_config)
            .expect("BSC bearer bind"),
    );
    cdma_bts::bts::bearer_agent::spawn_bts_bearer_agent(bts_bearer, controller.clone(), reverse_rx);

    let controller_clone = controller.clone();
    let server_handle = tokio::spawn(async move {
        let (sender, mut events_rx) = accept(&listener).await.unwrap();
        let mut agent = AbisAgent::new(agent_config(), controller_clone);
        while let Some(event) = events_rx.recv().await {
            match event {
                TransportEvent::Message(msg) => {
                    let (responses, _events) = agent.handle_message(&msg);
                    for resp in responses {
                        let _ = sender.send(&resp).await;
                    }
                }
                TransportEvent::Disconnected(_) => break,
            }
        }
        agent
    });

    let client = NetworkBtsControlClient::connect_with_bearer(addr, client_config(), bsc_bearer)
        .await
        .unwrap();
    let handle = client
        .allocate_rc1_traffic(LongCodeGenerator::new_traffic_channel(0xCAFE), 0, 0xCAFE)
        .await
        .expect("traffic allocation should return bearer handle");

    // Commit the reserved channel via ECAM before sending bearer frames
    send_ecam_commit(&client, handle.walsh_code);
    tokio::time::sleep(Duration::from_millis(50)).await;

    let bearer = client
        .bearer_client()
        .expect("network client should expose UDP bearer");
    bearer
        .send_frame(BearerFrame {
            channel_family: ChannelFamily::Fch,
            bearer_id: handle.bearer_id,
            tx_frame_number: 1,
            traffic_frame: TrafficFrame::ForwardFchDcch(ForwardFchDcchFrame {
                channel_family: ChannelFamily::Fch,
                fpc_slc: 1,
                fsn: 0,
                fpc_gr: 0,
                rpc_olt: 0,
                frame_content: FrameContent::FchRc1_9600,
                forward_link_information: vec![1; 171],
                message_crc: 0,
            }),
            queue: ForwardBearerQueue::Traffic,
        })
        .expect("UDP bearer send should succeed");

    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(harness_queue_len(&controller, handle.walsh_code), Some(1));

    drop(client);
    let _ = tokio::time::timeout(Duration::from_secs(10), server_handle)
        .await
        .unwrap()
        .unwrap();
}

/// Send a PchMessageTransfer carrying an RC1 ECAM for the given walsh code,
/// which triggers the AbisAgent to commit the pending traffic channel.
fn send_ecam_commit(client: &dyn BtsControlClient, walsh_code: u8) {
    let ecam =
        ExtendedChannelAssignmentMessage::new_f_fch_r_fch_assignment(0, walsh_code, 0, 1, 1, false);
    let sdu = ecam.to_sdu();
    let sdu_bytes: Vec<u8> = sdu
        .bits()
        .chunks(8)
        .map(|chunk| chunk.iter().fold(0u8, |acc, &b| (acc << 1) | b))
        .collect();
    let aim = AirInterfaceMessagePayload::new(0x15, sdu_bytes).unwrap();
    let pch = PchMessageTransferMessage {
        correlation_id: None,
        mobile_identities: vec![MobileIdentity::Esn(0)],
        cell_identifier_list: None,
        air_interface_message: Some(aim),
        layer2_ack_request_results: None,
        abis_ack_notify: None,
    };
    client.send_pch_message(pch).unwrap();
}

fn harness_queue_len(controller: &Arc<TrafficResourceService>, walsh_code: u8) -> Option<usize> {
    controller
        .traffic_channels_pool()
        .lookup(walsh_code)
        .map(|slot| match &slot.channel {
            cdma_bts::bts::TrafficChannelWrapper::Rc1(ch) => ch.channel.queue_len(),
            cdma_bts::bts::TrafficChannelWrapper::Rc3(ch) => ch.channel.queue_len(),
            cdma_bts::bts::TrafficChannelWrapper::SchRc3(ch) => ch.channel.queue_len(),
        })
}

/// Deallocate with no prior allocation is a no-op.
#[tokio::test]
async fn abis_tcp_deallocate_unknown_walsh_is_noop() {
    let _ = env_logger::try_init();

    let harness = AbisTestHarness::new().await;
    harness.client().deallocate_traffic(99).await;
    let _agent = harness.shutdown().await;
}
