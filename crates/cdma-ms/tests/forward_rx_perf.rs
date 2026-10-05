use std::sync::mpsc::channel;
use std::thread;
use std::time::{Duration, Instant};

use num_complex::Complex32;

use cdma_bts::bts::{self, Bts, BtsRuntimeSettings};
use cdma_bts::receiver::sync::SyncChannelMessage;
use cdma_bts::sdr::RadioPipe;
use cdma_bts::{lac, mac};
use cdma_common::overhead::OverheadParameters;

use cdma_ms::forward_rx::{ForwardReceiver, ForwardRxConfig};

const SAMPLE_RATE_HZ: u32 = 1_228_800 * 4;
const CDMA_FREQ: u16 = 384;

fn sync_template() -> SyncChannelMessage {
    SyncChannelMessage {
        pd: 0,
        msg_type: 1,
        p_rev: 6,
        min_p_rev: 6,
        sid: 42,
        nid: 7,
        pilot_pn: 0,
        lc_state: 0,
        sys_time: 0,
        lp_sec: 0,
        ltm_off: 0,
        daylt: 0,
        prat: 0,
        cdma_freq: CDMA_FREQ,
        ext_cdma_freq: 0,
        sr1_bcch_non_td_incl: false,
        sr1_td_incl: false,
        sr3_incl: false,
        ds_incl: false,
    }
}

fn overhead() -> OverheadParameters {
    OverheadParameters {
        cdma_freq: Some(CDMA_FREQ),
        ext_cdma_freq: Some(0),
        ..Default::default()
    }
}

async fn forward_iq() -> Vec<Complex32> {
    let (mac_to_lac_tx, mac_to_lac_rx) = channel();
    let (lac_to_mac_tx, lac_to_mac_rx) = channel();
    let lac_layer = lac::Layer2Lac::new(lac_to_mac_tx, mac_to_lac_rx);
    let mac_layer = mac::Layer2Mac::new(lac_to_mac_rx, mac_to_lac_tx);

    let lac_worker = {
        let lac = lac_layer.clone();
        thread::spawn(move || lac.run_for(2_000_000, Duration::from_secs(30)))
    };
    let mac_worker = {
        let mac = mac_layer.clone();
        thread::spawn(move || mac.run_for(2_000_000, Duration::from_secs(30)))
    };

    let (radio, pipe_handle) = RadioPipe::new(1024);
    let (bts, bts_handle) = Bts::new_with_radio_pipe(
        radio,
        bts::Config {
            tx_center_frequency_hz: 881_520_000,
            pilot_offset: 0,
            mac_layer,
            start_system_time: None,
            sync_channel_template: Some(sync_template()),
            timezone: cdma_common::timezone::TimezoneConfig::default(),
            overhead: overhead(),
            rx: None,
            evdo: None,
        },
        BtsRuntimeSettings::default(),
    );

    let drain = thread::spawn(move || {
        let mut all = Vec::new();
        while let Some(block) = pipe_handle.recv_tx() {
            all.extend(block.samples);
        }
        all
    });

    bts.run_for_blocks(70_000).await.expect("bts run failed");
    drop(bts_handle);
    let _ = lac_worker.join().unwrap();
    let _ = mac_worker.join().unwrap();
    let iq: Vec<Complex32> = drain.join().expect("drain thread panicked");
    assert!(!iq.is_empty(), "BTS produced no forward IQ");
    iq
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn forward_rx_runs_at_least_2x_real_time() {
    let iq = forward_iq().await;

    let warmup = (SAMPLE_RATE_HZ as usize) * 3 / 2;
    assert!(iq.len() > warmup * 2, "need enough IQ past warmup");
    let (warm, steady) = iq.split_at(warmup.min(iq.len()));
    let steady_secs = steady.len() as f64 / SAMPLE_RATE_HZ as f64;

    let mut rx = ForwardReceiver::new(ForwardRxConfig::default(), SAMPLE_RATE_HZ as f64);
    let _ = rx.push(warm); // acquire (untimed)

    let start = Instant::now();
    let events = rx.push(steady);
    let proc_secs = start.elapsed().as_secs_f64();
    let rt_ratio = steady_secs / proc_secs;

    eprintln!(
        "forward-rx perf: steady={:.2}s processing={:.2}s rt_ratio={:.2}x events={}",
        steady_secs,
        proc_secs,
        rt_ratio,
        events.len()
    );

    assert!(
        rt_ratio >= 2.0,
        "MS forward-RX must sustain >= 2.0x real time (measured {rt_ratio:.2}x over {steady_secs:.2}s of steady-state air)"
    );
}
