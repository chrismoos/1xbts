mod support;

use std::time::Duration;

use cdma_ms::engine::{Engine, EngineConfig, ReverseBurst};
use cdma_ms::tx::{MsAccessIdentity, SERVICE_OPTION_SMS};
use num_complex::Complex32;

const SKEW_PERIOD: usize = 25_000;
const SKEW_START: usize = 5_000_000;
const SKEW_END: usize = 10_000_000;

const STREAM_SAMPLES: usize = 20_000_000;

fn network_sample_of(skewed_index: u64) -> u64 {
    let dropped = skewed_index.saturating_sub(SKEW_START as u64) / (SKEW_PERIOD as u64 - 1);
    skewed_index + dropped.min(((SKEW_END - SKEW_START) / SKEW_PERIOD) as u64)
}

fn drop_clock_samples(iq: &[Complex32]) -> Vec<Complex32> {
    iq.iter()
        .enumerate()
        .filter(|(i, _)| !(SKEW_START..SKEW_END).contains(i) || i % SKEW_PERIOD != SKEW_PERIOD - 1)
        .map(|(_, s)| *s)
        .collect()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reverse_bursts_name_the_chip_they_leave_at() {
    let esn: u32 = 0x4CDC_1D09;

    let node = support::boot();
    let raw = support::collect_forward_iq(node, 80_000).await;
    let iq = drop_clock_samples(&raw);
    assert!(
        iq.len() > STREAM_SAMPLES * 4 / 5,
        "need a long enough stream to accumulate slew, got {}",
        iq.len()
    );

    let mut engine = Engine::new(EngineConfig {
        identity: MsAccessIdentity {
            esn,
            imsi_s: 1234567890,
            ..Default::default()
        },
        sample_rate_hz: support::SAMPLE_RATE_HZ as f64,
        ..Default::default()
    });
    engine.power_on();

    const CHUNK: usize = 98_304;
    let mut bursts: Vec<ReverseBurst> = Vec::new();
    for (n, chunk) in iq.chunks(CHUNK).enumerate() {
        engine.handle_forward(chunk);
        if n % 30 == 10 {
            engine.originate(SERVICE_OPTION_SMS, "5551234".into());
        }
        while let Some(b) = engine.poll_transmit() {
            bursts.push(b);
        }
    }
    engine.flush_forward();
    while let Some(b) = engine.poll_transmit() {
        bursts.push(b);
    }

    println!("state={:?} bursts={}", engine.state(), bursts.len());
    assert!(
        bursts.len() >= 3,
        "need several bursts spread over the stream, got {}",
        bursts.len()
    );

    bursts.retain(|burst| {
        burst.stream_sample_start < SKEW_START as u64 || burst.stream_sample_start > SKEW_END as u64
    });
    assert!(bursts.len() >= 2);
    let first = &bursts[0];
    assert!(first.stream_sample_start < SKEW_START as u64);
    assert!(bursts.last().unwrap().stream_sample_start > SKEW_END as u64);
    let base_network = network_sample_of(first.stream_sample_start);
    let oversample = (support::SAMPLE_RATE_HZ / support::CHIP_RATE_HZ) as i64;
    let mut worst = 0i64;
    for b in &bursts {
        let network = network_sample_of(b.stream_sample_start);
        let expected =
            first.absolute_chip_start as i64 + (network as i64 - base_network as i64) / oversample;
        let error = b.absolute_chip_start as i64 - expected;
        println!(
            "burst {:>10} sample={:>12} chip={:>16} error={:>8} chips",
            b.label, b.stream_sample_start, b.absolute_chip_start, error
        );
        worst = worst.max(error.abs());
    }
    assert!(
        worst <= 2,
        "reverse timing drifted {worst} chips from the network's clock"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn reverse_bursts_reanchor_after_an_unfilled_radio_gap() {
    const GAP_START: usize = support::SAMPLE_RATE_HZ;
    const GAP_SAMPLES: usize = support::SAMPLE_RATE_HZ / 2;
    const CHUNK: usize = 98_304;
    let raw = support::collect_forward_iq(support::boot(), 80_000).await;
    let mut engine = Engine::new(EngineConfig {
        sample_rate_hz: support::SAMPLE_RATE_HZ as f64,
        power_up_delay: Duration::ZERO,
        ..Default::default()
    });
    engine.power_on();
    let mut before = Vec::new();
    for chunk in raw[..GAP_START].chunks(CHUNK) {
        engine.handle_forward(chunk);
        while let Some(burst) = engine.poll_transmit() {
            before.push(burst);
        }
    }
    let first = before.first().expect("registration before the receive gap");
    engine.on_forward_discontinuity();
    let mut after = Vec::new();
    for chunk in raw[GAP_START + GAP_SAMPLES..].chunks(CHUNK) {
        engine.handle_forward(chunk);
        while let Some(burst) = engine.poll_transmit() {
            // The gap queues an empty end-of-burst that aborts TX. It carries no chips.
            if !burst.samples.is_empty() {
                after.push(burst);
            }
        }
    }
    assert!(
        !after.is_empty(),
        "registration did not resume after the gap"
    );
    let oversample = (support::SAMPLE_RATE_HZ / support::CHIP_RATE_HZ) as i64;
    for burst in after {
        let elapsed_samples = burst.stream_sample_start as i64 + GAP_SAMPLES as i64
            - first.stream_sample_start as i64;
        let elapsed_chips = burst.absolute_chip_start as i64 - first.absolute_chip_start as i64;
        assert!(
            (elapsed_samples - elapsed_chips * oversample).abs() <= oversample,
            "reverse timing retained the pre-gap anchor"
        );
    }
}
