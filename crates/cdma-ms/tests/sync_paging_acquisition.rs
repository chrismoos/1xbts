//! Sync and paging boundaries follow the PN roll and even-second grid (C.S0002-E §3.1.3.3.1, §3.1.3.4.1).

mod support;

use std::time::Instant;

use cdma_ms::forward_rx::{ForwardEvent, ForwardReceiver, ForwardRxConfig};
use cdma_ms::ms::MsCore;
use num_complex::Complex32;

const NODE_BLOCKS: usize = 24_000;
const MAX_LOCK_TO_SYNC_MS: f64 = 400.0;
const MAX_SYNC_TO_OVERHEAD_MS: f64 = 600.0;
const MAX_WALL_SECS: f64 = 10.0;

fn air_ms(samples: usize) -> f64 {
    samples as f64 * 1000.0 / support::SAMPLE_RATE_HZ as f64
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn sync_and_paging_decode_on_the_fixed_frame_timeline() {
    let _ = env_logger::builder().is_test(true).try_init();
    let started = Instant::now();
    let node = support::boot();
    let iq: Vec<Complex32> = support::collect_forward_iq(node, NODE_BLOCKS).await;
    assert!(!iq.is_empty(), "booted node produced no forward IQ");

    let mut rx = ForwardReceiver::new(ForwardRxConfig::default(), support::SAMPLE_RATE_HZ as f64);
    let mut core = MsCore::new(cdma_ms::tx::MsAccessIdentity {
        esn: 0x4CDC_1D09,
        imsi_s: 1234567890,
        ..Default::default()
    });
    core.power_on();

    let chunk = 65_536;
    let mut fed = 0usize;
    let mut lock_at = None;
    let mut sync_at = None;
    let mut overhead_at = None;
    let mut sync_params = None;
    let mut paging_crc_valid = 0usize;
    let mut paging_crc_failed = 0usize;
    for samples in iq.chunks(chunk) {
        fed += samples.len();
        for ev in rx.push(samples) {
            match ev {
                ForwardEvent::PilotLock => {
                    lock_at.get_or_insert(fed);
                }
                ForwardEvent::Sync(params) => {
                    if sync_at.is_none() {
                        sync_at = Some(fed);
                        core.on_pilot_acquired(params.pilot_pn as u32);
                        core.on_sync_decoded(params.clone());
                        sync_params = Some(params);
                    }
                }
                ForwardEvent::Paging(msg) => {
                    paging_crc_valid += 1;
                    core.ingest_paging_message(&msg, 0);
                    if core.overhead().system_parameters.is_some() && overhead_at.is_none() {
                        overhead_at = Some(fed);
                    }
                }
                ForwardEvent::PagingCrcFail { .. } => paging_crc_failed += 1,
                _ => {}
            }
        }
    }
    let processing_secs = started.elapsed().as_secs_f64();

    let lock_at = lock_at.expect("pilot tracker never locked");
    let sync_at = sync_at.expect("no sync message decoded");
    let overhead_at = overhead_at.expect("no System Parameters Message decoded");
    let lock_to_sync = air_ms(sync_at - lock_at);
    let sync_to_overhead = air_ms(overhead_at - sync_at);
    eprintln!(
        "acquisition: lock at {:.0} ms, sync +{:.0} ms, overhead +{:.0} ms, paging crc valid={} failed={}, wall {:.2} s",
        air_ms(lock_at),
        lock_to_sync,
        sync_to_overhead,
        paging_crc_valid,
        paging_crc_failed,
        processing_secs
    );

    let sync = sync_params.unwrap();
    assert_eq!((sync.sid, sync.nid), (support::SID, support::NID));
    assert_eq!(sync.pilot_pn, support::PILOT_PN);
    assert!(
        lock_to_sync <= MAX_LOCK_TO_SYNC_MS,
        "sync took {lock_to_sync:.0} ms after pilot lock"
    );
    assert!(
        sync_to_overhead <= MAX_SYNC_TO_OVERHEAD_MS,
        "overhead took {sync_to_overhead:.0} ms after sync"
    );
    let overhead = core.overhead();
    assert!(
        overhead.access_parameters.is_some(),
        "no Access Parameters Message"
    );
    assert!(
        paging_crc_valid >= 4,
        "only {paging_crc_valid} CRC-valid paging messages"
    );
    assert!(
        paging_crc_failed <= paging_crc_valid,
        "paging CRC failures ({paging_crc_failed}) outnumber successes ({paging_crc_valid})"
    );
    assert!(
        processing_secs <= MAX_WALL_SECS,
        "test took {processing_secs:.1} s, over the {MAX_WALL_SECS} s bound"
    );
}
