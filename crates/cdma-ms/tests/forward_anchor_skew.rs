mod support;

use cdma_ms::forward_rx::{ForwardEvent, ForwardReceiver, ForwardRxConfig};
use num_complex::Complex32;

const SKEW_PERIOD: usize = 25_000;

fn network_sample_of(receive_index: i64) -> i64 {
    receive_index + receive_index / (SKEW_PERIOD as i64 - 1)
}

fn drop_clock_samples(iq: &[Complex32]) -> Vec<Complex32> {
    iq.iter()
        .enumerate()
        .filter(|(i, _)| i % SKEW_PERIOD != SKEW_PERIOD - 1)
        .map(|(_, s)| *s)
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn the_timing_anchor_holds_against_a_skewed_receive_clock() {
    let node = support::boot();
    let raw = support::collect_forward_iq(node, 60_000).await;
    println!("generated {} forward samples", raw.len());
    let iq = drop_clock_samples(&raw);
    assert!(iq.len() > 12_000_000, "short stream: {}", iq.len());

    let mut rx = ForwardReceiver::new(ForwardRxConfig::default(), support::SAMPLE_RATE_HZ as f64);

    let mut anchors: Vec<(u64, u64, i64)> = Vec::new();
    for chunk in iq.chunks(98_304) {
        for ev in rx.push(chunk) {
            if matches!(ev, ForwardEvent::Sync(_)) {
                if let Some(a) = rx.sync_time_anchor() {
                    anchors.push((a.sync_chip, a.input_sample, rx.pilot_slew()));
                }
            }
        }
    }
    println!("anchors={} slew {:?}", anchors.len(), anchors.last());
    assert!(
        anchors.len() >= 3,
        "need several anchors, got {}",
        anchors.len()
    );

    let offsets: Vec<i64> = anchors
        .iter()
        .map(|&(chip, sample, slew)| chip as i64 - network_sample_of(sample as i64 - slew) / 4)
        .collect();
    for (&(_, _, slew), offset) in anchors.iter().zip(&offsets) {
        println!("slew={slew:>6}  offset={offset}");
    }
    let spread = offsets.iter().max().unwrap() - offsets.iter().min().unwrap();
    let slew_span = anchors.last().unwrap().2 - anchors.first().unwrap().2;
    println!("spread {spread} chips over {slew_span} samples of slew");
    assert!(
        spread <= 2,
        "the timing anchor drifted {spread} chips over {slew_span} samples of tracker slew"
    );
}
