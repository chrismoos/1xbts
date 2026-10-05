mod support;

use std::time::Duration;

use cdma_ms::engine::{Engine, EngineConfig};
use cdma_ms::event::ChannelSink;
use cdma_ms::ms::MsEvent;
use cdma_ms::tx::MsAccessIdentity;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn engine_registers_from_live_forward() {
    let esn: u32 = 0x4CDC_1D09;

    let node = support::boot();
    let mut running = support::start_running(node);
    running
        .wait_forward_samples(6_100_000, Duration::from_secs(20))
        .await;
    let iq = running.snapshot_forward_iq();

    let mut engine = Engine::new(EngineConfig {
        identity: MsAccessIdentity {
            esn,
            imsi_s: 1234567890,
            ..Default::default()
        },
        sample_rate_hz: support::SAMPLE_RATE_HZ as f64,
        power_up_delay: Duration::ZERO,
        ..Default::default()
    });
    let (sink, events) = ChannelSink::new();
    engine.add_event_sink(sink);

    engine.power_on();
    engine.handle_forward(&iq);
    engine.flush_forward();

    let burst = engine
        .poll_transmit()
        .expect("engine should produce a registration probe");
    running.inject_reverse(burst.samples, burst.absolute_chip_start);

    let mobile = running.wait_for_mobile(esn, Duration::from_secs(15)).await;
    running.shutdown().await;

    let collected: Vec<MsEvent> = events.try_iter().collect();
    assert!(
        collected
            .iter()
            .any(|e| matches!(e, MsEvent::RegistrationNeeded { .. })),
        "engine should emit RegistrationNeeded"
    );
    assert_eq!(
        mobile
            .expect("BSC should register the MS from the engine's probe")
            .esn,
        Some(esn)
    );
}
