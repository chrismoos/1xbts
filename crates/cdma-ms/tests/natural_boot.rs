mod support;

use num_complex::Complex32;

use cdma_ms::forward_rx::{ForwardEvent, ForwardReceiver, ForwardRxConfig};

#[ignore = "the production finger acquires the pilot but the sync sub-chain does not yet decode from its output"]
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ms_naturally_acquires_sync_via_production_finger() {
    let _ = env_logger::builder().is_test(true).try_init();
    let node = support::boot();
    let iq: Vec<Complex32> = support::collect_forward_iq(node, 24_000).await;
    eprintln!("finger boot: drained {} forward IQ samples", iq.len());
    assert!(!iq.is_empty(), "booted node produced no forward IQ");

    let mut rx = ForwardReceiver::new(
        ForwardRxConfig {
            use_production_finger: true,
            ..Default::default()
        },
        support::SAMPLE_RATE_HZ as f64,
    );

    let mut sync_events = 0usize;
    let mut saw_pilot_pn = false;
    let mut paging_events = 0usize;
    let mut paging_crc_valid = 0usize;
    for ev in rx.decode_all(&iq) {
        match ev {
            ForwardEvent::Sync(sync) => {
                sync_events += 1;
                if sync.pilot_pn == support::PILOT_PN {
                    saw_pilot_pn = true;
                }
            }
            ForwardEvent::Paging(_msg) => {
                paging_events += 1;
                paging_crc_valid += 1;
            }
            ForwardEvent::PagingCrcFail { .. } | ForwardEvent::PagingDecodeError { .. } => {
                paging_events += 1
            }
            ForwardEvent::Directed(_) => {}
            ForwardEvent::TrafficSignaling(_)
            | ForwardEvent::TrafficUnsupported(_)
            | ForwardEvent::TrafficPowerControl { .. }
            | ForwardEvent::TrafficVoice { .. }
            | ForwardEvent::TrafficErasure => {}
            ForwardEvent::PilotLock
            | ForwardEvent::PilotLost
            | ForwardEvent::PilotMeasurement(_) => {}
        }
    }
    eprintln!(
        "finger boot: sync_events={} pilot_pn_ok={} paging_events={} paging_crc_valid={}",
        sync_events, saw_pilot_pn, paging_events, paging_crc_valid
    );
    assert!(sync_events > 0, "finger MS decoded no sync");
    assert!(
        saw_pilot_pn,
        "finger MS expected sync with the node's pilot PN"
    );
    assert!(paging_events > 0, "finger MS decoded no paging events");
    assert!(
        paging_crc_valid > 0,
        "finger MS decoded no CRC-valid paging"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ms_naturally_acquires_sync_from_booted_node() {
    let node = support::boot();
    let iq: Vec<Complex32> = support::collect_forward_iq(node, 24_000).await;

    eprintln!("natural boot: drained {} forward IQ samples", iq.len());
    assert!(!iq.is_empty(), "booted node produced no forward IQ");

    let mut rx = ForwardReceiver::new(ForwardRxConfig::default(), support::SAMPLE_RATE_HZ as f64);

    let mut sync_events = 0usize;
    let mut saw_pilot_pn = false;
    let mut paging_events = 0usize;
    let mut paging_crc_valid = 0usize;
    for ev in rx.decode_all(&iq) {
        match ev {
            ForwardEvent::Sync(sync) => {
                sync_events += 1;
                if sync.pilot_pn == support::PILOT_PN {
                    saw_pilot_pn = true;
                }
            }
            ForwardEvent::Paging(_msg) => {
                paging_events += 1;
                paging_crc_valid += 1;
            }
            ForwardEvent::PagingCrcFail { .. } | ForwardEvent::PagingDecodeError { .. } => {
                paging_events += 1
            }
            ForwardEvent::Directed(_) => {}
            ForwardEvent::TrafficSignaling(_)
            | ForwardEvent::TrafficUnsupported(_)
            | ForwardEvent::TrafficPowerControl { .. }
            | ForwardEvent::TrafficVoice { .. }
            | ForwardEvent::TrafficErasure => {}
            ForwardEvent::PilotLock
            | ForwardEvent::PilotLost
            | ForwardEvent::PilotMeasurement(_) => {}
        }
    }

    eprintln!(
        "natural boot: sync_events={} pilot_pn_ok={} paging_events={} paging_crc_valid={}",
        sync_events, saw_pilot_pn, paging_events, paging_crc_valid
    );
    assert!(
        sync_events > 0,
        "MS decoded no sync from the naturally booted node"
    );
    assert!(saw_pilot_pn, "expected sync with the node's pilot PN");
    assert!(
        paging_events > 0,
        "MS decoded no paging events from the naturally booted node"
    );
    assert!(
        paging_crc_valid > 0,
        "MS decoded no CRC-valid overhead paging from the naturally booted node"
    );
}
