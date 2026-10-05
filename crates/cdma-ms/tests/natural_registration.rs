mod support;

use std::time::Duration;

use cdma_ms::tx::{
    AccessArq, AccessChannelConfig, MsAccessIdentity, ServingSystem, build_registration_capsule,
    default_lc_state_at, modulate_access_probe, pulse_shape,
};

const CHIPS_PER_FRAME: u64 = 24_576;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ms_registers_via_reverse_access_probe() {
    let esn: u32 = 0x4CDC_1D09;

    let node = support::boot();
    let mut running = support::start_running(node);

    tokio::time::sleep(Duration::from_millis(200)).await;

    let id = MsAccessIdentity {
        esn,
        ..Default::default()
    };
    let capsule =
        build_registration_capsule(&id, 1, &AccessArq::assured(0), &ServingSystem::at_p_rev(6));
    let cfg = AccessChannelConfig::default();
    let frame_start_chip = CHIPS_PER_FRAME * 100;
    let iq = pulse_shape(&modulate_access_probe(
        &cfg,
        capsule.bits(),
        frame_start_chip,
        default_lc_state_at(frame_start_chip),
        8,
        4,
    ));
    eprintln!("MS injecting {} reverse IQ samples", iq.len());
    running.inject_reverse(iq, frame_start_chip);

    let mobile = running.wait_for_mobile(esn, Duration::from_secs(15)).await;
    running.shutdown().await;

    let mobile = mobile.expect("BSC should register the MS from its reverse access probe alone");
    eprintln!(
        "registered mobile: esn={:?} state={}",
        mobile.esn, mobile.state
    );
    assert_eq!(mobile.esn, Some(esn), "registered mobile ESN should match");
}
