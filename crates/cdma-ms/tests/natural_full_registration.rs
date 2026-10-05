mod support;

use std::time::Duration;

use cdma_ms::forward_rx::{ForwardEvent, ForwardReceiver, ForwardRxConfig};
use cdma_ms::ms::{AccessReason, MsCore, MsProtocolState, REG_TYPE_POWER_UP};
use cdma_ms::tx::{
    AccessArq, AccessChannelConfig, MsAccessIdentity, ServingSystem, build_registration_capsule,
    default_lc_state_at, modulate_access_probe, pulse_shape,
};

const CHIPS_PER_FRAME: u64 = 24_576;
const MIN_PREAMBLE_FRAMES: usize = 8;

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ms_acquires_reads_overhead_and_registers_in_one_flow() {
    let esn: u32 = 0x4CDC_1D09;

    let node = support::boot();
    let mut running = support::start_running(node);

    let captured = running
        .wait_forward_samples(6_100_000, Duration::from_secs(20))
        .await;
    assert!(
        captured >= 6_000_000,
        "node did not stream enough forward IQ (captured {captured})"
    );
    let iq = running.snapshot_forward_iq();

    let mut ms = MsCore::new(MsAccessIdentity {
        esn,
        imsi_s: 1234567890,
        ..Default::default()
    });
    ms.power_on();

    let mut rx = ForwardReceiver::new(ForwardRxConfig::default(), support::SAMPLE_RATE_HZ as f64);
    let mut acquired = false;
    for ev in rx.decode_all(&iq) {
        match ev {
            ForwardEvent::Sync(decoded) if !acquired => {
                acquired = true;
                ms.on_pilot_acquired(decoded.pilot_pn as u32);
                ms.on_sync_decoded(decoded);
            }
            ForwardEvent::Paging(msg) => ms.ingest_paging_message(&msg, 0),
            _ => {}
        }
    }
    let _ = acquired;

    assert_eq!(
        *ms.state(),
        MsProtocolState::SystemAccess {
            reason: AccessReason::Registration {
                reg_type: REG_TYPE_POWER_UP
            }
        },
        "MS should have decided to register from live overhead"
    );
    let ap = ms
        .access_params()
        .expect("access params from live APM")
        .clone();
    let sync = ms.sync_info().expect("sync info").clone();

    let frame_start_chip = {
        let lead = CHIPS_PER_FRAME * 4;
        ((sync.sys_time + lead) / CHIPS_PER_FRAME) * CHIPS_PER_FRAME
    };
    let preamble_frames = (ap.pam_sz as usize + 1).max(MIN_PREAMBLE_FRAMES);

    let id = MsAccessIdentity {
        esn,
        ..Default::default()
    };
    let capsule = build_registration_capsule(
        &id,
        REG_TYPE_POWER_UP,
        &AccessArq::assured(0),
        &ServingSystem::at_p_rev(6),
    );
    let cfg = AccessChannelConfig::default();
    let probe = pulse_shape(&modulate_access_probe(
        &cfg,
        capsule.bits(),
        frame_start_chip,
        default_lc_state_at(frame_start_chip),
        preamble_frames,
        4,
    ));
    eprintln!(
        "MS transmitting probe: frame_start_chip={frame_start_chip} preamble_frames={preamble_frames} samples={}",
        probe.len()
    );
    running.inject_reverse(probe, frame_start_chip);

    let mobile = running.wait_for_mobile(esn, Duration::from_secs(15)).await;
    running.shutdown().await;

    let mobile = mobile.expect("BSC should register the MS from the probe it timed off live sync");
    eprintln!(
        "registered mobile: esn={:?} state={}",
        mobile.esn, mobile.state
    );
    assert_eq!(mobile.esn, Some(esn));
}
