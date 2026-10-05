mod support;

use std::sync::Arc;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::time::{Duration, Instant};

use cdma_common::band_class::BandClass;
use cdma_ms::engine::{EngineConfig, ScanConfig, ScanMode};
use cdma_ms::event::{ChannelSink, EventSink};
use cdma_ms::forward_rx::ForwardRxConfig;
use cdma_ms::iq_radio::IqSourceRadio;
use cdma_ms::ms::MsEvent;
use cdma_ms::prl_scan::{NidMatch, PrlScanPlan, PrlSystem};
use cdma_ms::radio::Radio;
use cdma_ms::station::MobileStation;
use cdma_ms::tx::MsAccessIdentity;
use num_complex::Complex32;

const ESN: u32 = 0x4CDC1D09;
const IMSI_S: u64 = 1234567890;
const NODE_BLOCKS: usize = 24_000;
const DWELL_MS: u32 = 300;
const EVENT_TIMEOUT: Duration = Duration::from_secs(60);
const VERIZON_PRL: &str = concat!(
    env!("CARGO_MANIFEST_DIR"),
    "/../cdma-otasp/tests/fixtures/verizon_50408.prl"
);

fn node_system(preferred: bool) -> PrlSystem {
    PrlSystem {
        index: 0,
        sid: support::SID,
        nid: NidMatch::Any,
        preferred,
        geo: 1,
        acq_index: 0,
        roaming_indicator: Some(0),
        more_desirable: false,
    }
}

async fn node_iq() -> Vec<Complex32> {
    let node = support::boot();
    let iq = support::collect_forward_iq(node, NODE_BLOCKS).await;
    assert!(!iq.is_empty(), "booted node produced no forward IQ");
    iq
}

fn start(
    radio: IqSourceRadio,
    plan: PrlScanPlan,
    forward_rx: ForwardRxConfig,
) -> (MobileStation, Receiver<MsEvent>) {
    start_mode(radio, plan, forward_rx, ScanMode::Camp)
}

fn start_mode(
    radio: IqSourceRadio,
    plan: PrlScanPlan,
    forward_rx: ForwardRxConfig,
    mode: ScanMode,
) -> (MobileStation, Receiver<MsEvent>) {
    let station = MobileStation::start(
        Box::new(radio),
        EngineConfig {
            identity: MsAccessIdentity {
                esn: ESN,
                imsi_s: IMSI_S,
                ..Default::default()
            },
            sample_rate_hz: support::SAMPLE_RATE_HZ as f64,
            rx_reference_dbm: 0.0,
            forward_rx,
            scan: Some(
                ScanConfig::new(Arc::new(plan))
                    .with_dwell_ms(DWELL_MS)
                    .with_mode(mode),
            ),
            ..Default::default()
        },
    );
    let (sink, events) = ChannelSink::new();
    station.add_event_sink(sink as Arc<dyn EventSink>);
    station.power_on();
    (station, events)
}

fn collect_until(
    events: &Receiver<MsEvent>,
    done: impl Fn(&MsEvent) -> bool,
    timeout: Duration,
) -> Vec<MsEvent> {
    let deadline = Instant::now() + timeout;
    let mut out = Vec::new();
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        match events.recv_timeout(left) {
            Ok(ev) => {
                eprintln!("  [event] {ev:?}");
                let stop = done(&ev);
                out.push(ev);
                if stop {
                    return out;
                }
            }
            Err(RecvTimeoutError::Timeout) | Err(RecvTimeoutError::Disconnected) => return out,
        }
    }
}

fn scan_finished(ev: &MsEvent) -> bool {
    matches!(ev, MsEvent::ScanFinished { .. })
}

fn tuned_channels(events: &[MsEvent]) -> Vec<u16> {
    events
        .iter()
        .filter_map(|e| match e {
            MsEvent::ChannelTuned { channel, .. } => Some(*channel),
            _ => None,
        })
        .collect()
}

fn pilot_searches(events: &[MsEvent]) -> Vec<(u16, bool)> {
    events
        .iter()
        .filter_map(|e| match e {
            MsEvent::PilotSearch { channel, found, .. } => Some((*channel, *found)),
            _ => None,
        })
        .collect()
}

fn assert_plausible_ec_io(ec_io: Option<f32>) {
    let db = ec_io.expect("a found pilot reports Ec/Io");
    assert!(
        (-12.0..=-0.5).contains(&db),
        "pilot Ec/Io {db} dB is not plausible for the loopback carrier"
    );
}

fn verdicts(events: &[MsEvent]) -> Vec<(u16, u16, bool, String)> {
    events
        .iter()
        .filter_map(|e| match e {
            MsEvent::PrlVerdict {
                sid,
                nid,
                permitted,
                reason,
                ..
            } => Some((*sid, *nid, *permitted, reason.clone())),
            _ => None,
        })
        .collect()
}

fn finish_result(events: &[MsEvent]) -> Option<String> {
    events.iter().find_map(|e| match e {
        MsEvent::ScanFinished { result, .. } => Some(result.clone()),
        _ => None,
    })
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scan_skips_empty_channels_then_camps_on_the_permitted_system() {
    let _ = env_logger::builder().is_test(true).try_init();
    let iq = node_iq().await;
    // A looped capture restarts the cell's system time at the seam, so paging
    // goes silent and T30m drops the overhead this test inspects.
    let radio = IqSourceRadio::new(
        iq,
        support::SAMPLE_RATE_HZ as f64,
        support::node_forward_hz(),
    )
    .with_loop(false);
    let plan = PrlScanPlan::from_parts(
        7,
        true,
        &[
            (BandClass::Bc0, 777),
            (BandClass::Bc0, 283),
            (BandClass::Bc0, support::CDMA_FREQ),
        ],
        vec![node_system(true)],
    );
    let (station, events) = start(radio, plan, ForwardRxConfig::default());
    let ev = collect_until(&events, scan_finished, EVENT_TIMEOUT);

    assert!(
        ev.iter()
            .any(|e| matches!(e, MsEvent::ScanStarted { channels: 3, .. }))
    );
    assert_eq!(tuned_channels(&ev), vec![777, 283, support::CDMA_FREQ]);
    assert_eq!(
        pilot_searches(&ev),
        vec![(777, false), (283, false), (support::CDMA_FREQ, true)]
    );
    assert!(ev.iter().any(|e| matches!(
        e,
        MsEvent::SyncDecoded(s) if s.sid == support::SID && s.nid == support::NID
    )));
    assert_eq!(
        verdicts(&ev),
        vec![(support::SID, support::NID, true, "preferred".to_string())]
    );
    assert_eq!(finish_result(&ev).as_deref(), Some("camped"));
    assert!(
        ev.iter()
            .any(|e| matches!(e, MsEvent::StateChange { to, .. } if to == "idle"))
    );

    let after = collect_until(
        &events,
        |e| matches!(e, MsEvent::OverheadUpdated { .. }),
        EVENT_TIMEOUT,
    );
    assert!(
        after.iter().any(|e| matches!(
            e,
            MsEvent::OverheadUpdated { sid, nid, .. } if *sid == support::SID && *nid == support::NID
        )),
        "no overhead decoded after camping"
    );
    let _ = collect_until(
        &events,
        |e| matches!(e, MsEvent::AccessParametersUpdated),
        Duration::from_secs(20),
    );
    let d = station.diagnostics().expect("diagnostics");
    assert!(
        d.state == "idle" || d.state == "system_access",
        "camped MS should be idle or registering, got {}",
        d.state
    );
    assert!(d.pilot.locked);
    // Capture wraparound distorts live Ec/Io.
    assert!(!d.pilot.ec_io_db.is_nan(), "live Ec/Io not measured");
    let sync = d.sync.expect("sync parameters kept after camping");
    assert_eq!(
        (sync.sid, sync.nid, sync.pilot_pn),
        (support::SID, support::NID, support::PILOT_PN)
    );
    assert_eq!(sync.p_rev, 6);
    assert_eq!(sync.paging_rate_bps(), 9600);
    assert_eq!(sync.cdma_freq, support::CDMA_FREQ);
    assert_eq!(
        sync.system_time().format("%Y-%m-%d").to_string(),
        "1980-01-06"
    );
    let spm = d
        .overhead
        .system_parameters
        .expect("System Parameters Message cached");
    assert_eq!((spm.sid, spm.nid), (support::SID, support::NID));
    assert!(spm.power_up_reg);
    let apm = d
        .overhead
        .access_parameters
        .expect("Access Parameters Message cached");
    assert!(apm.num_step > 0);
    assert!(d.paging.crc_valid >= 2, "paging stats {:?}", d.paging);
    assert!(d.paging.messages.contains_key("SPM"));
    let scan = d.scan.expect("scan report");
    assert_eq!(scan.result.as_deref(), Some("camped"));
    assert_eq!(scan.channels.len(), 3);
    assert_eq!(scan.channels[2].pilot, Some(true));
    assert!(
        scan.channels[2]
            .verdict
            .as_ref()
            .is_some_and(|v| v.permitted())
    );
    station.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn survey_visits_every_channel_and_camps_on_nothing() {
    let _ = env_logger::builder().is_test(true).try_init();
    let iq = node_iq().await;
    let radio = IqSourceRadio::new(
        iq,
        support::SAMPLE_RATE_HZ as f64,
        support::node_forward_hz(),
    );
    let plan = PrlScanPlan::from_parts(
        7,
        true,
        &[(BandClass::Bc0, support::CDMA_FREQ), (BandClass::Bc0, 777)],
        vec![node_system(true)],
    );
    let (station, events) = start_mode(radio, plan, ForwardRxConfig::default(), ScanMode::Survey);
    let ev = collect_until(&events, scan_finished, EVENT_TIMEOUT);

    assert_eq!(
        pilot_searches(&ev),
        vec![(support::CDMA_FREQ, true), (777, false)]
    );
    assert_eq!(
        verdicts(&ev),
        vec![(support::SID, support::NID, true, "preferred".to_string())]
    );
    assert!(
        ev.iter()
            .any(|e| matches!(e, MsEvent::SyncDecoded(s) if s.sid == support::SID))
    );
    assert_eq!(finish_result(&ev).as_deref(), Some("survey"));
    assert!(
        !ev.iter()
            .any(|e| matches!(e, MsEvent::StateChange { to, .. } if to == "idle"))
    );

    let d = station.diagnostics().expect("diagnostics");
    assert_eq!(d.state, "system_determination");
    let scan = d.scan.expect("scan report");
    assert_eq!(scan.mode, ScanMode::Survey);
    assert_eq!(scan.result.as_deref(), Some("survey"));
    assert_eq!(scan.channels.len(), 2);
    assert_eq!(scan.channels[0].pilot, Some(true));
    assert_plausible_ec_io(Some(scan.channels[0].measurement.ec_io_db));
    assert_eq!(
        scan.channels[0].sync.as_ref().map(|s| s.sid),
        Some(support::SID)
    );
    assert_eq!(scan.channels[1].pilot, Some(false));
    station.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dump_records_forward_iq_to_a_wav() {
    let _ = env_logger::builder().is_test(true).try_init();
    let iq = node_iq().await;
    let radio = IqSourceRadio::new(
        iq,
        support::SAMPLE_RATE_HZ as f64,
        support::node_forward_hz(),
    );
    let plan = PrlScanPlan::from_parts(
        7,
        true,
        &[(BandClass::Bc0, support::CDMA_FREQ)],
        vec![node_system(true)],
    );
    let (station, events) = start(radio, plan, ForwardRxConfig::default());
    let _ = collect_until(&events, scan_finished, EVENT_TIMEOUT);

    let dir = std::env::temp_dir().join(format!("cdma-ms-dump-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("forward.wav");
    let seconds = 0.25;
    station.dump_forward(&path, seconds).expect("start dump");
    let ev = collect_until(
        &events,
        |e| matches!(e, MsEvent::DumpFinished { .. }),
        EVENT_TIMEOUT,
    );
    let samples = ev
        .iter()
        .find_map(|e| match e {
            MsEvent::DumpFinished { samples, .. } => Some(*samples),
            _ => None,
        })
        .expect("dump finished");
    let expected = (seconds * support::SAMPLE_RATE_HZ as f64) as u64;
    assert_eq!(samples, expected);

    let reader = hound::WavReader::open(&path).expect("open dump");
    let spec = reader.spec();
    assert_eq!(spec.channels, 2);
    assert_eq!(spec.sample_rate, support::SAMPLE_RATE_HZ as u32);
    assert_eq!(reader.len() as u64, expected * 2);
    let replay = IqSourceRadio::from_wav(&path, support::node_forward_hz()).expect("reload dump");
    assert!((replay.sample_rate_hz() - support::SAMPLE_RATE_HZ as f64).abs() < 1.0);
    std::fs::remove_dir_all(&dir).ok();
    station.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scan_rejects_a_negative_system_and_reports_no_permitted_system() {
    let _ = env_logger::builder().is_test(true).try_init();
    let iq = node_iq().await;
    let radio = IqSourceRadio::new(
        iq,
        support::SAMPLE_RATE_HZ as f64,
        support::node_forward_hz(),
    );
    let plan = PrlScanPlan::from_parts(
        7,
        true,
        &[(BandClass::Bc0, support::CDMA_FREQ)],
        vec![node_system(false)],
    );
    let (station, events) = start(radio, plan, ForwardRxConfig::default());
    let ev = collect_until(&events, scan_finished, EVENT_TIMEOUT);

    assert_eq!(pilot_searches(&ev), vec![(support::CDMA_FREQ, true)]);
    assert_eq!(
        verdicts(&ev),
        vec![(support::SID, support::NID, false, "negative".to_string())]
    );
    assert_eq!(finish_result(&ev).as_deref(), Some("no_permitted_system"));
    assert!(
        ev.iter()
            .rev()
            .find_map(|e| match e {
                MsEvent::StateChange { to, .. } => Some(to.clone()),
                _ => None,
            })
            .as_deref()
            == Some("system_determination"),
        "the mobile should end in System Determination"
    );
    station.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn scan_with_the_verizon_prl_camps_on_the_node() {
    let _ = env_logger::builder().is_test(true).try_init();
    let iq = node_iq().await;
    let radio = IqSourceRadio::new(
        iq,
        support::SAMPLE_RATE_HZ as f64,
        support::node_forward_hz(),
    );
    let plan = PrlScanPlan::load(std::path::Path::new(VERIZON_PRL)).expect("load Verizon PRL");
    assert_eq!(plan.channels[0].channel, support::CDMA_FREQ);
    let (station, events) = start(radio, plan, ForwardRxConfig::default());
    let ev = collect_until(&events, scan_finished, EVENT_TIMEOUT);

    assert_eq!(tuned_channels(&ev), vec![support::CDMA_FREQ]);
    assert!(ev.iter().any(|e| matches!(
        e,
        MsEvent::PrlVerdict {
            sid,
            permitted: true,
            roaming_indicator: Some(66),
            ..
        } if *sid == support::SID
    )));
    assert_eq!(finish_result(&ev).as_deref(), Some("camped"));
    station.shutdown();
}
