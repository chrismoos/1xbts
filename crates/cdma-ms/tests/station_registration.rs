mod support;

use std::sync::Arc;
use std::time::Duration;

use cdma_ms::engine::EngineConfig;
use cdma_ms::event::{ChannelSink, EventSink};
use cdma_ms::ms::MsEvent;
use cdma_ms::station::MobileStation;
use cdma_ms::tx::MsAccessIdentity;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn appliance_registers_over_the_sim_radio() {
    let esn: u32 = 0x4CDC_1D09;

    let (radio, mut sim) = support::boot_sim(40_000);

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

    let mobile = sim.wait_for_mobile(esn, Duration::from_secs(30)).await;
    ms.shutdown();
    sim.shutdown().await;

    let collected: Vec<MsEvent> = events.try_iter().collect();
    assert!(
        collected
            .iter()
            .any(|e| matches!(e, MsEvent::RegistrationNeeded { .. })),
        "appliance should emit RegistrationNeeded"
    );
    assert_eq!(
        mobile
            .expect("BSC should register the MS driven by the appliance")
            .esn,
        Some(esn)
    );
}
