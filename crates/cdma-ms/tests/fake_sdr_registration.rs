mod support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use cdma_common::consts::BURST_TYPE_SMS;
use cdma_common::sms::decode_mo_sms;

use cdma_ms::engine::EngineConfig;
use cdma_ms::event::{ChannelSink, EventSink};
use cdma_ms::radio::TxCalibration;
use cdma_ms::station::MobileStation;
use cdma_ms::tx::MsAccessIdentity;

const FAKE_SDR_EVENT_TIMEOUT: Duration = Duration::from_secs(20);
const FAKE_BTS_SHUTDOWN_BUDGET: Duration = Duration::from_secs(10);

fn calibration(tx_delay_samples: i64) -> TxCalibration {
    TxCalibration {
        tx_reference_dbm: 0.0,
        estimated_full_scale_dbm: None,
        tx_delay_samples,
        power_control: false,
        peak_limit: 0.8,
        relative_access_power: false,
        access_initial_backoff_db: 0.0,
    }
}

async fn run_registration(group_delay_chips: i64, tx_delay_samples: i64, pilot_pn: u16) -> bool {
    run_registration_with_calibration(group_delay_chips, pilot_pn, calibration(tx_delay_samples))
        .await
}

async fn run_registration_with_calibration(
    group_delay_chips: i64,
    pilot_pn: u16,
    calibration: TxCalibration,
) -> bool {
    let (radio, mut node) =
        support::boot_fake_sdr_with_pilot_pn(4_000_000, group_delay_chips, calibration, pilot_pn);

    let esn: u32 = 0x4CDC_1D09;
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

    let registered = node.wait_for_mobile(esn, FAKE_SDR_EVENT_TIMEOUT).await;

    ms.shutdown();
    let shutdown_started = Instant::now();
    node.shutdown().await;
    assert!(
        shutdown_started.elapsed() < FAKE_BTS_SHUTDOWN_BUDGET,
        "fake BTS should stop promptly when injected RX closes"
    );
    registered.is_some()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fake_sdr_registers_with_reduced_peak_and_relative_access_ramp() {
    let mut calibration = calibration(0);
    calibration.peak_limit = 0.4;
    calibration.relative_access_power = true;
    calibration.access_initial_backoff_db = 12.0;
    assert!(run_registration_with_calibration(0, 0, calibration).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fake_sdr_registers_with_no_group_delay() {
    let _ = env_logger::builder().is_test(true).try_init();
    assert!(
        run_registration(0, 0, 0).await,
        "MS should register over the fake SDR with the reverse placed by hardware time"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fake_sdr_registers_with_group_delay_cancelled_by_tx_delay() {
    let _ = env_logger::builder().is_test(true).try_init();
    let os = (support::SAMPLE_RATE_HZ / support::CHIP_RATE_HZ) as i64;
    assert!(
        run_registration(1, -os, 0).await,
        "a group delay cancelled by tx_delay should still register"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fake_sdr_registers_with_pilot_pn_510() {
    let _ = env_logger::builder().is_test(true).try_init();
    assert!(
        run_registration(0, 0, 510).await,
        "MS should register over the fake SDR at pilot PN 510"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn fake_sdr_delivers_mo_sms_over_hardware_time() {
    let _ = env_logger::builder().is_test(true).try_init();
    let esn: u32 = 0x4CDC_1D09;

    let (radio, mut node) = support::boot_fake_sdr(6_000_000, 0, calibration(0));

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

    let registered = node.wait_for_mobile(esn, FAKE_SDR_EVENT_TIMEOUT).await;
    assert!(
        registered.is_some(),
        "MS should register before originating"
    );

    ms.originate_sms("5559876".to_string(), "Hi".to_string());
    let sms_bytes = node
        .wait_for_mo_sms(BURST_TYPE_SMS, FAKE_SDR_EVENT_TIMEOUT)
        .await;

    ms.shutdown();
    node.shutdown().await;

    let sms_bytes = sms_bytes.expect("BSC should forward the MO SMS to the MSC as an ADDS Deliver");
    let decoded = decode_mo_sms(&sms_bytes).expect("MSC ADDS payload should decode as MO SMS");
    assert_eq!(decoded.destination_number, "5559876");
    assert_eq!(decoded.text, "Hi");
}
