//! 1x packet data through a PCF over A9 and A8, against the real PCF and
//! PDSN services over loopback GRE-in-UDP.

mod support;

use std::time::Duration;

use cdma_bsc::a9_edge::{A9PcfClient, PacketSessionMetadata, PcfClient};

const SERVICE_OPTION_PACKET_DATA_RC3: u32 = 33;
const WAIT: Duration = Duration::from_secs(5);

#[tokio::test(flavor = "multi_thread")]
async fn packet_session_sets_up_a8_runs_rlp_and_releases() {
    let _ = env_logger::try_init();
    let core = support::start_packet_core().await;
    let client = A9PcfClient::bind(core.packet_a9.clone()).await.unwrap();

    let metadata = PacketSessionMetadata {
        imsi: Some("310009176936269".to_string()),
        esn: Some(0x4cdc_1d09),
        traffic_walsh_code: 12,
        ..PacketSessionMetadata::default()
    };
    let (_uplink, mut downlink) = tokio::time::timeout(
        WAIT,
        client.open_packet_session("w12".to_string(), SERVICE_OPTION_PACKET_DATA_RC3, metadata),
    )
    .await
    .expect("Setup-A8 answered")
    .expect("A8 connection set up");

    // RLP Type 3 starts by sending SYNC on the forward FCH every frame.
    let frame = tokio::time::timeout(WAIT, downlink.recv())
        .await
        .expect("a forward RLP frame within the wait")
        .expect("downlink open");
    assert_eq!(frame.session_id, "w12");
    assert!(frame.num_bits > 0);
    assert!(frame.rate_bps > 0);

    assert_eq!(
        core.packet_service.list_all_sessions().len(),
        1,
        "the PDSN runs PPP for the session"
    );
    assert!(client.set_sch_active("w12", true, 12_345).await.is_err());
    client.set_sch_active("w12", true, 19_200).await.unwrap();
    client.set_sch_active("w12", false, 19_200).await.unwrap();

    tokio::time::timeout(WAIT, client.close_packet_session("w12"))
        .await
        .expect("Release-A8 answered");
}
