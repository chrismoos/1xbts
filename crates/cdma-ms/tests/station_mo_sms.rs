mod support;

use std::sync::Arc;
use std::time::Duration;

use cdma_common::consts::BURST_TYPE_SMS;
use cdma_common::sms::decode_mo_sms;

use cdma_ms::engine::EngineConfig;
use cdma_ms::event::{ChannelSink, EventSink};
use cdma_ms::station::MobileStation;
use cdma_ms::tx::MsAccessIdentity;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn appliance_delivers_mo_sms_over_rc3_traffic() {
    let _ = env_logger::builder().is_test(true).try_init();
    let esn: u32 = 0x4CDC_1D09;

    let (radio, mut sim) = support::boot_sim(2_000_000);

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
    let (sink, _events) = ChannelSink::new();
    ms.add_event_sink(sink as Arc<dyn EventSink>);
    ms.power_on();

    let registered = sim.wait_for_mobile(esn, Duration::from_secs(30)).await;
    assert!(
        registered.is_some(),
        "MS should register before originating"
    );

    ms.originate_sms("5559876".to_string(), "Hi".to_string());

    let sms_bytes = sim
        .wait_for_mo_sms(BURST_TYPE_SMS, Duration::from_secs(60))
        .await;

    ms.shutdown();
    sim.shutdown().await;

    let sms_bytes = sms_bytes.expect("BSC should forward the MO SMS to the MSC as an ADDS Deliver");
    let decoded = decode_mo_sms(&sms_bytes).expect("MSC ADDS payload should decode as MO SMS");
    assert_eq!(decoded.destination_number, "5559876");
    assert_eq!(decoded.text, "Hi");
}
