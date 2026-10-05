mod support;

use cdma_ms::forward_rx::{ForwardEvent, ForwardReceiver, ForwardRxConfig};
use num_complex::Complex32;

const SYNTHESIS_BLOCKS: usize = 60_000;
const STEADY_SAMPLES: usize = support::SAMPLE_RATE_HZ;
const SKEW_PERIOD: usize = 25_000;
const CHUNK_SAMPLES: usize = 65_536;

fn paging_count(iq: &[Complex32]) -> (usize, usize, i64) {
    let mut rx = ForwardReceiver::new(ForwardRxConfig::default(), support::SAMPLE_RATE_HZ as f64);
    let mut good = 0;
    let mut bad = 0;
    for chunk in iq.chunks(CHUNK_SAMPLES) {
        for event in rx.push(chunk) {
            match event {
                ForwardEvent::Paging(_) => good += 1,
                ForwardEvent::PagingCrcFail { .. } | ForwardEvent::PagingDecodeError { .. } => {
                    bad += 1
                }
                _ => {}
            }
        }
    }
    (good, bad, rx.pilot_slew())
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn paging_survives_accumulated_sample_clock_drift() {
    let raw = support::collect_forward_iq(support::boot(), SYNTHESIS_BLOCKS).await;
    let reference = paging_count(&raw);
    assert!(
        reference.0 > 10,
        "insufficient reference paging: {reference:?}"
    );
    for direction in [-1, 1] {
        let mut skewed = Vec::with_capacity(raw.len() + raw.len() / SKEW_PERIOD);
        for (index, &sample) in raw.iter().enumerate() {
            let adjust = index >= STEADY_SAMPLES && index % SKEW_PERIOD == 0;
            if !adjust || direction < 0 {
                skewed.push(sample);
            }
            if adjust && direction < 0 {
                skewed.push(sample);
            }
        }
        let measured = paging_count(&skewed);
        println!("direction={direction} reference={reference:?} skewed={measured:?}");
        assert!(
            measured.2.abs() > 256,
            "clock drift did not exceed a Walsh symbol"
        );
        assert_eq!(
            measured.0, reference.0,
            "paging disappeared as clock drift accumulated"
        );
        assert_eq!(measured.1, reference.1, "clock drift corrupted paging");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn paging_recovers_after_pilot_reacquisition() {
    const RECOVERY_BLOCKS: usize = 100_000;
    const LOSS_START: usize = support::SAMPLE_RATE_HZ;
    const LOSS_END: usize = LOSS_START + support::SAMPLE_RATE_HZ / 4;
    let mut raw = support::collect_forward_iq(support::boot(), RECOVERY_BLOCKS).await;
    raw[LOSS_START..LOSS_END].fill(Complex32::new(0.0, 0.0));
    let mut rx = ForwardReceiver::new(ForwardRxConfig::default(), support::SAMPLE_RATE_HZ as f64);
    let before = rx.push(&raw[..LOSS_START]);
    assert!(before.iter().any(|e| matches!(e, ForwardEvent::Paging(_))));
    let old_anchor = rx.sync_time_anchor().expect("initial sync");
    let mut lost = 0;
    let mut recovered = 0;
    for chunk in raw[LOSS_START..].chunks(CHUNK_SAMPLES) {
        for event in rx.push(chunk) {
            match event {
                ForwardEvent::PilotLost => lost += 1,
                ForwardEvent::Paging(_) if lost > 0 => recovered += 1,
                _ => {}
            }
        }
    }
    println!("lock losses={lost}, recovered paging={recovered}");
    assert_eq!(lost, 1);
    assert!(
        recovered > 50,
        "paging did not recover after pilot reacquisition"
    );
    let new_anchor = rx.sync_time_anchor().expect("recovered sync");
    assert!(new_anchor.input_sample > old_anchor.input_sample);
    assert!(new_anchor.sync_chip > old_anchor.sync_chip);
    let elapsed_samples = new_anchor.input_sample as i64 - old_anchor.input_sample as i64;
    let elapsed_chips = new_anchor.sync_chip as i64 - old_anchor.sync_chip as i64;
    assert!((elapsed_samples - elapsed_chips * 4).abs() <= 4);
    assert_eq!(rx.samples_accepted(), raw.len() as u64);
}
