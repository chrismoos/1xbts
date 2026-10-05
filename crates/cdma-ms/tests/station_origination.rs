mod support;

use std::sync::Arc;
use std::time::Duration;

use cdma_ms::engine::EngineConfig;
use cdma_ms::event::{ChannelSink, EventSink};
use cdma_ms::ms::MsEvent;
use cdma_ms::station::MobileStation;
use cdma_ms::tx::{MsAccessIdentity, SERVICE_OPTION_SMS};

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn appliance_originates_and_is_granted_an_rc3_channel() {
    let _ = env_logger::builder().is_test(true).try_init();
    let esn: u32 = 0x4CDC_1D09;

    let (radio, mut sim) = support::boot_sim(400_000);

    let ms = MobileStation::start(
        Box::new(radio),
        EngineConfig {
            identity: MsAccessIdentity {
                esn,
                imsi_s: 1234567890,
                ..Default::default()
            },
            sample_rate_hz: support::SAMPLE_RATE_HZ as f64,
            power_up_delay: Duration::ZERO,
            ..Default::default()
        },
    );
    let (sink, events) = ChannelSink::new();
    ms.add_event_sink(sink as Arc<dyn EventSink>);
    ms.power_on();

    let registered = sim.wait_for_mobile(esn, Duration::from_secs(30)).await;
    assert!(
        registered.is_some(),
        "MS should register before originating"
    );
    ms.originate(SERVICE_OPTION_SMS, String::new());

    let assignment = tokio::time::timeout(
        Duration::from_secs(20),
        tokio::task::spawn_blocking(move || {
            while let Ok(ev) = events.recv() {
                eprintln!("MS event: {ev:?}");
                if let MsEvent::ChannelAssignment {
                    walsh_code,
                    frame_offset,
                    for_rc,
                    rev_rc,
                } = ev
                {
                    return Some((walsh_code, frame_offset, for_rc, rev_rc));
                }
            }
            None
        }),
    )
    .await
    .expect("timed out waiting for channel assignment")
    .expect("event task panicked");

    ms.shutdown();
    sim.shutdown().await;

    let (walsh_code, _frame_offset, for_rc, rev_rc) =
        assignment.expect("MS should decode an Extended Channel Assignment Message");
    assert!(walsh_code > 0, "assigned a non-pilot Walsh code");
    assert_eq!(for_rc, 3, "granted forward RC3");
    assert_eq!(rev_rc, 3, "granted reverse RC3");
}
