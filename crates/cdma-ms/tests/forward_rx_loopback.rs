use std::sync::mpsc::channel;
use std::thread;
use std::time::Duration;

use num_complex::Complex32;

use cdma_bts::bts::{self, Bts, BtsRuntimeSettings};
use cdma_bts::receiver::sync::SyncChannelMessage;
use cdma_bts::sdr::RadioPipe;
use cdma_bts::{lac, mac};
use cdma_common::overhead::OverheadParameters;

use cdma_ms::forward_rx::{ForwardEvent, ForwardReceiver, ForwardRxConfig};

const SAMPLE_RATE_HZ: u32 = 1_228_800 * 4;
const CDMA_FREQ: u16 = 384;
const TEST_SID: u16 = 42;
const TEST_NID: u16 = 7;

fn sync_template() -> SyncChannelMessage {
    SyncChannelMessage {
        pd: 0,
        msg_type: 1,
        p_rev: 6,
        min_p_rev: 6,
        sid: TEST_SID,
        nid: TEST_NID,
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

fn direct_overhead() -> OverheadParameters {
    OverheadParameters {
        cdma_freq: Some(CDMA_FREQ),
        ext_cdma_freq: Some(0),
        ..Default::default()
    }
}

#[tokio::test]
async fn ms_forward_rx_decodes_sync_from_bts_loopback() {
    // MAC/LAC pair. With a sync template on the BTS, sync is generated
    // directly. the workers only need to run so the BTS's fragment path does
    // not stall.
    let (mac_to_lac_tx, mac_to_lac_rx) = channel();
    let (lac_to_mac_tx, lac_to_mac_rx) = channel();
    let lac_layer = lac::Layer2Lac::new(lac_to_mac_tx, mac_to_lac_rx);
    let mac_layer = mac::Layer2Mac::new(lac_to_mac_rx, mac_to_lac_tx);

    let lac_worker = {
        let lac = lac_layer.clone();
        thread::spawn(move || lac.run_for(50_000, Duration::from_secs(2)))
    };
    let mac_worker = {
        let mac = mac_layer.clone();
        thread::spawn(move || mac.run_for(50_000, Duration::from_secs(2)))
    };

    let (radio, pipe_handle) = RadioPipe::new(256);
    let (bts, bts_handle) = Bts::new_with_radio_pipe(
        radio,
        bts::Config {
            tx_center_frequency_hz: 881_520_000,
            pilot_offset: 0,
            mac_layer,
            start_system_time: None,
            sync_channel_template: Some(sync_template()),
            timezone: cdma_common::timezone::TimezoneConfig::default(),
            overhead: direct_overhead(),
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

    bts.run_for_blocks(8_000).await.expect("bts run failed");
    drop(bts_handle);

    let _ = lac_worker.join().unwrap();
    let _ = mac_worker.join().unwrap();
    let iq_samples: Vec<Complex32> = drain.join().expect("drain thread panicked");
    eprintln!("loopback drained {} forward IQ samples", iq_samples.len());
    assert!(!iq_samples.is_empty(), "BTS produced no forward IQ");

    let mut rx = ForwardReceiver::new(ForwardRxConfig::default(), SAMPLE_RATE_HZ as f64);
    let mut sync_events = 0usize;
    let mut saw_msg_type_1 = false;
    let mut saw_pilot_pn_0 = false;
    for ev in rx.decode_all(&iq_samples) {
        if let ForwardEvent::Sync(sync) = ev {
            let (msg_type, pilot_pn, sys_time_chips, lc_state) =
                (sync.msg_type, sync.pilot_pn, sync.sys_time, sync.lc_state);
            sync_events += 1;
            saw_msg_type_1 |= msg_type == 1;
            saw_pilot_pn_0 |= pilot_pn == 0;
            eprintln!(
                "sync #{sync_events}: msg_type={msg_type} pilot_pn={pilot_pn} sys_time={sys_time_chips} lc_state={lc_state}"
            );
        }
    }

    eprintln!(
        "forward-rx loopback: sync_events={} msg_type_1={} pilot_pn_0={}",
        sync_events, saw_msg_type_1, saw_pilot_pn_0
    );
    assert!(
        sync_events > 0,
        "MS decoded no sync events from BTS loopback"
    );
    assert!(saw_msg_type_1, "expected a sync message with msg_type=1");
    assert!(saw_pilot_pn_0, "expected a sync message with pilot_pn=0");
}
