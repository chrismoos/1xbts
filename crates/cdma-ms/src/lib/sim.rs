#![allow(dead_code)]

use std::collections::VecDeque;
use std::sync::Arc;
use std::sync::Mutex as StdMutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::channel;
use std::thread;
use std::time::{Duration, Instant};

use crate::engine::ReverseBurst;
use crate::radio::Radio;

use cdma_abis::control::typed::CellId;
use cdma_abis::transport::{TransportEvent, spawn_channel_transport};
use cdma_bsc::abis_edge::BtsControlClient;
use cdma_bsc::abis_edge::network::{NetworkBtsControlClient, NetworkClientConfig};
use cdma_bsc::bsc::{
    Bsc, BtsCellParams, BtsRegistry, Config as BscConfig, LoopbackHlrRepository, RecordingMscClient,
};
use cdma_bsc::config::{RcPairConfig, TrafficAssignmentConfig, TrafficRetryConfig};
use cdma_bts::bts::abis_agent::{AbisAgent, AbisAgentConfig};
use cdma_bts::bts::paging_supplier::{
    PagingRetryConfig as BtsPagingRetryConfig, PagingSupplierState, build_bts_paging_supplier,
};
use cdma_bts::bts::{
    self, Bts, BtsRuntimeSettings, PagingChannelSettings, TrafficResourceService, TxRxAnchor,
};
use cdma_bts::receiver::sync::SyncChannelMessage;
use cdma_bts::sdr::{RadioPipe, RadioPipeHandle, RadioRx, RadioTx, RxReadResult};
use cdma_bts::{lac, mac};
use num_complex::Complex32;
use tokio::sync::watch;

pub use cdma_bsc::bsc::{AddsDeliverRecord, MobileInfo, SmsRequest};

pub const SID: u16 = 42;
pub const NID: u16 = 7;
pub const CDMA_FREQ: u16 = 384;
pub const PILOT_PN: u16 = 0;
pub const SAMPLE_RATE_HZ: usize = 1_228_800 * 4;
pub const CHIP_RATE_HZ: usize = 1_228_800;

pub struct NodeKeepAlive {
    _bsc_task: tokio::task::JoinHandle<()>,
    _lac_worker: thread::JoinHandle<()>,
    _mac_worker: thread::JoinHandle<()>,
    _lac_layer: lac::Layer2LacRef,
    _mac_layer: mac::Layer2MacRef,
}

pub struct BootedNode {
    pub bts: Bts,
    pub pipe: RadioPipeHandle,
    pub mobiles: watch::Receiver<Vec<MobileInfo>>,
    pub sms_tx: tokio::sync::mpsc::Sender<SmsRequest>,
    pub msc: Arc<RecordingMscClient>,
    pub clock: Instant,
    pub tx_rx_anchor: Arc<TxRxAnchor>,
    pub keepalive: NodeKeepAlive,
}

fn sync_template(pilot_pn: u16) -> SyncChannelMessage {
    SyncChannelMessage {
        pd: 0,
        msg_type: 1,
        p_rev: 6,
        min_p_rev: 6,
        sid: SID,
        nid: NID,
        pilot_pn,
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

fn bts_overhead() -> cdma_common::overhead::OverheadParameters {
    cdma_common::overhead::OverheadParameters {
        sid: SID,
        nid: NID,
        cdma_freq: Some(CDMA_FREQ),
        ext_cdma_freq: Some(0),
        ..Default::default()
    }
}

fn rx_settings(pilot_pn: u16) -> bts::RxSettings {
    bts::RxSettings {
        sample_rate_hz: SAMPLE_RATE_HZ,
        rx_center_frequency_hz: None,
        one_x_enabled: true,
        one_x_reverse_frequency_hz: None,
        one_x_rx_shift_hz: 0,
        hrpd_reverse_frequency_hz: None,
        hrpd_rx_shift_hz: None,
        auth_mode: 0,
        p_rev_in_use: 6,
        capture_iq_wav: None,
        capture_seconds: None,
        access_channel_number: 0,
        paging_channel_number: 1,
        base_id: 1,
        pilot_pn,
        chip_rate_hz: CHIP_RATE_HZ,
        absolute_chip_start: 0,
        hardware_start_time_ns: 0,
        tick_rate: 1_000_000_000,
        access_event_tx: None,
        hrpd_access_event_tx: None,
        hrpd_traffic_event_tx: None,
        hrpd_access_cycle_number: 0,
        hrpd_access_sector_id_lsb: 0,
        hrpd_access_color_code: 0,
        hrpd_access_preamble_frames: 0,
        hrpd_access_enhanced_rates: false,
        reverse_bearer_tx: None,
        rx_metrics_tx: None,
        reanchor_origin: false,
        traffic_rx_pool: None,
        hrpd_traffic_rx_queue: None,
        hrpd_harq_bus: None,
        hrpd_power_control: None,
        traffic_channels: None,
        power_control: None,
        traffic_rx_removals: None,
        traffic_rx_continuity: false,
        overhead_mcc: 0x03ff,
        overhead_imsi_11_12: 0x7f,
        rx_sample_delay: 0,
        rx_batch_pcgs: 2,
        tx_rx_anchor: None,
        reverse_access_finger_pool_size: 1,
        global_finger_pool_size: 1,
        traffic_ack_seq_tx: None,
        rx_measurements: None,
    }
}

fn spawn_loopback_abis_client(
    controller: Arc<TrafficResourceService>,
    agent_config: AbisAgentConfig,
    config: NetworkClientConfig,
    paging_state: Arc<parking_lot::Mutex<PagingSupplierState>>,
) -> NetworkBtsControlClient {
    let (client_sender, client_events, server_sender, mut server_events) =
        spawn_channel_transport();
    let bearer_controller = controller.clone();
    tokio::spawn(async move {
        let mut agent = AbisAgent::new(agent_config, controller);
        agent.set_paging_state(paging_state);
        while let Some(event) = server_events.recv().await {
            match event {
                TransportEvent::Message(msg) => {
                    let (responses, _events) = agent.handle_message(&msg);
                    for resp in responses {
                        if server_sender.send(&resp).await.is_err() {
                            return;
                        }
                    }
                }
                TransportEvent::Disconnected(_) => return,
            }
        }
    });
    NetworkBtsControlClient::from_transport(client_sender, client_events, config)
        .with_local_bearer(bearer_controller)
}

/// Boot the loopback node. Must be called inside a Tokio runtime (e.g. a
/// `#[tokio::test]` or the CLI's runtime), since the BSC runtime is spawned as a
/// task.
pub fn boot() -> BootedNode {
    boot_with_pilot_pn(PILOT_PN)
}

pub fn boot_with_pilot_pn(pilot_pn: u16) -> BootedNode {
    let (mac_to_lac_tx, mac_to_lac_rx) = channel();
    let (lac_to_mac_tx, lac_to_mac_rx) = channel();
    let lac_layer = lac::Layer2Lac::new(lac_to_mac_tx, mac_to_lac_rx);
    let mac_layer = mac::Layer2Mac::new(lac_to_mac_rx, mac_to_lac_tx);

    let clock = Instant::now();
    let (radio, pipe) = RadioPipe::with_clock(1024, clock);
    let (bts, bts_handle) = Bts::new_with_radio_pipe(
        radio,
        bts::Config {
            tx_center_frequency_hz: 881_520_000,
            pilot_offset: pilot_pn as usize,
            mac_layer: mac_layer.clone(),
            start_system_time: Some(cdma_common::time::cdma_epoch()),
            sync_channel_template: Some(sync_template(pilot_pn)),
            timezone: cdma_common::timezone::TimezoneConfig::default(),
            overhead: bts_overhead(),
            rx: Some(rx_settings(pilot_pn)),
            evdo: None,
        },
        BtsRuntimeSettings::default(),
    );
    let tx_rx_anchor = bts.tx_rx_anchor();

    let bts::BtsHandle {
        access_events,
        walsh_allocator,
        traffic_channels,
        traffic_rx_pool,
        traffic_rx_removals,
        power_control,
        ..
    } = bts_handle;

    let stamped_access_events = {
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        let mut src = access_events;
        tokio::spawn(async move {
            while let Some(mut ev) = src.recv().await {
                ev.cell
                    .get_or_insert(cdma_common::events::AccessCellId { cell: 1, sector: 1 });
                if tx.send(ev).is_err() {
                    break;
                }
            }
        });
        rx
    };

    let (mobiles_tx, mobiles_rx) = watch::channel(Vec::<MobileInfo>::new());
    let (sms_tx, sms_rx) = tokio::sync::mpsc::channel::<SmsRequest>(32);
    let msc = Arc::new(RecordingMscClient::new());

    let paging_state = Arc::new(parking_lot::Mutex::new(
        PagingSupplierState::new_with_retry_config(BtsPagingRetryConfig::default(), 0x03ff, 0x7f),
    ));

    let control_client: Arc<dyn BtsControlClient> = Arc::new(spawn_loopback_abis_client(
        Arc::new(TrafficResourceService::from_pools(
            walsh_allocator.clone(),
            traffic_channels.clone(),
            traffic_rx_pool.clone(),
            traffic_rx_removals.clone(),
            power_control.clone(),
        )),
        AbisAgentConfig {
            pilot_pn,
            cell_id: CellId { cell: 1, sector: 1 },
            mscid: 1,
        },
        NetworkClientConfig {
            cell_id: CellId { cell: 1, sector: 1 },
            mscid: 1,
            pilot_pn,
            auth_mode: 0,
            p_rev_in_use: 6,
            market_id: 1,
            generating_entity_id: 1,
        },
        paging_state.clone(),
    ));

    let registry = BtsRegistry::new();
    let entry = registry
        .enroll(
            BtsCellParams {
                cell: cdma_common::events::AccessCellId { cell: 1, sector: 1 },
                pilot_offset: pilot_pn as usize,
                mcc: 0x03ff,
                imsi_11_12: 0x7f,
                ..BtsCellParams::from_overhead(bts_overhead())
            },
            None,
            None,
        )
        .expect("loopback cell enrolls without conflict");
    entry.set_control(control_client);

    lac_layer.set_paging_supplier(build_bts_paging_supplier(
        bts_overhead(),
        PagingChannelSettings::default(),
        pilot_pn as usize,
        None,
        paging_state.clone(),
    ));

    let bsc = Bsc::new(BscConfig {
        bts: registry,
        traffic_assignment: TrafficAssignmentConfig {
            preferred_pairs: vec![RcPairConfig::new(3, 3), RcPairConfig::new(1, 1)],
            ..TrafficAssignmentConfig::default()
        },
        access_event_rx: Some(stamped_access_events),
        cell_detach_rx: None,
        access_event_broadcast: None,
        sms_request_rx: Some(sms_rx),
        sms_request_tx: None,
        data_request_rx: None,
        data_request_tx: None,
        power_override_request_rx: None,
        power_override_request_tx: None,
        mobiles_tx: Some(mobiles_tx),
        paging_broadcast: None,
        traffic_broadcast: None,
        hlr_repo: Some(Arc::new(LoopbackHlrRepository::new("5551000"))),
        msc_client: msc.clone(),
        msc_voice_bearer: None,
        traffic_retry: TrafficRetryConfig::default(),
        voice_timeouts: Default::default(),
        pcf_client: None,
        mobile_idle_timeout_s: 0,
        bts_paging_state: Some(paging_state.clone()),
        node_id: "ms-loopback-bsc".to_string(),
    });

    let bsc_task = tokio::spawn(async move {
        let _ = bsc.run().await;
    });

    let lac_worker = {
        let lac = lac_layer.clone();
        thread::spawn(move || {
            let _ = lac.run_for(2_000_000, Duration::from_secs(15));
        })
    };
    let mac_worker = {
        let mac = mac_layer.clone();
        thread::spawn(move || {
            let _ = mac.run_for(2_000_000, Duration::from_secs(15));
        })
    };

    BootedNode {
        bts,
        pipe,
        mobiles: mobiles_rx,
        sms_tx,
        msc,
        clock,
        tx_rx_anchor,
        keepalive: NodeKeepAlive {
            _bsc_task: bsc_task,
            _lac_worker: lac_worker,
            _mac_worker: mac_worker,
            _lac_layer: lac_layer,
            _mac_layer: mac_layer,
        },
    }
}

pub async fn collect_forward_iq(node: BootedNode, blocks: usize) -> Vec<Complex32> {
    let BootedNode {
        bts,
        pipe,
        mobiles: _,
        sms_tx: _,
        msc: _,
        clock: _,
        tx_rx_anchor: _,
        keepalive,
    } = node;

    let drain = thread::spawn(move || {
        let mut all = Vec::new();
        while let Some(block) = pipe.recv_tx() {
            all.extend(block.samples);
        }
        all
    });

    let bts_task = tokio::task::spawn_blocking(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("bts test runtime");
        rt.block_on(async move { bts.run_for_blocks(blocks).await })
    });
    let _ = bts_task.await.expect("bts task panicked");

    let iq = drain.join().expect("drain thread panicked");
    drop(keepalive);
    iq
}

const FORWARD_CAPTURE_CAP: usize = 7_000_000;

pub struct RunningNode {
    pipe: Arc<StdMutex<RadioPipeHandle>>,
    drain_stop: Arc<AtomicBool>,
    drain: Option<thread::JoinHandle<()>>,
    bts_task: Option<tokio::task::JoinHandle<()>>,
    forward: Arc<StdMutex<Vec<Complex32>>>,
    pub mobiles: watch::Receiver<Vec<MobileInfo>>,
    pub sms_tx: tokio::sync::mpsc::Sender<SmsRequest>,
    keepalive: Option<NodeKeepAlive>,
}

pub fn start_running(node: BootedNode) -> RunningNode {
    let BootedNode {
        bts,
        pipe,
        mobiles,
        sms_tx,
        msc: _,
        clock: _,
        tx_rx_anchor: _,
        keepalive,
    } = node;
    let pipe = Arc::new(StdMutex::new(pipe));

    let drain_stop = Arc::new(AtomicBool::new(false));
    let forward = Arc::new(StdMutex::new(Vec::<Complex32>::new()));
    let drain = {
        let pipe = pipe.clone();
        let stop = drain_stop.clone();
        let forward = forward.clone();
        thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let samples = {
                    let ph = pipe.lock().unwrap();
                    ph.drain_tx_samples()
                };
                if !samples.is_empty() {
                    let mut acc = forward.lock().unwrap();
                    if acc.len() < FORWARD_CAPTURE_CAP {
                        acc.extend(samples);
                    }
                }
                thread::sleep(Duration::from_millis(5));
            }
        })
    };

    let bts_task = tokio::task::spawn_blocking(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("bts test runtime");
        rt.block_on(async move {
            let _ = bts.start_unpaced().await;
        });
    });

    RunningNode {
        pipe,
        drain_stop,
        drain: Some(drain),
        bts_task: Some(bts_task),
        forward,
        mobiles,
        sms_tx,
        keepalive: Some(keepalive),
    }
}

impl RunningNode {
    pub fn snapshot_forward_iq(&self) -> Vec<Complex32> {
        self.forward.lock().unwrap().clone()
    }

    pub fn clear_forward(&self) {
        self.forward.lock().unwrap().clear();
    }

    pub async fn wait_forward_samples(&self, n: usize, timeout: Duration) -> usize {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let have = self.forward.lock().unwrap().len();
            if have >= n || tokio::time::Instant::now() >= deadline {
                return have;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    /// Inject pulse-shaped reverse-link IQ into the live BTS receiver, streamed
    /// as fixed-size blocks each tagged with the absolute chip position of its
    /// first sample (matching how a real SDR delivers RX samples).
    pub fn inject_reverse(&self, samples: Vec<Complex32>, absolute_chip_start: u64) {
        const BLOCK_SAMPLES: usize = 32_768;
        let oversample = SAMPLE_RATE_HZ / CHIP_RATE_HZ;
        let mut offset = 0usize;
        while offset < samples.len() {
            let end = (offset + BLOCK_SAMPLES).min(samples.len());
            let chip_start = absolute_chip_start + (offset / oversample) as u64;
            let ph = self.pipe.lock().unwrap();
            ph.inject_rx(InjectedRxBlock {
                samples: samples[offset..end].to_vec(),
                time_ns: 0,
                absolute_chip_start: Some(chip_start),
            })
            .expect("inject_rx failed");
            offset = end;
        }
    }

    pub async fn wait_for_mobile(&mut self, esn: u32, timeout: Duration) -> Option<MobileInfo> {
        self.wait_for_mobile_where(esn, timeout, |_| true).await
    }

    pub async fn wait_for_mobile_where(
        &mut self,
        esn: u32,
        timeout: Duration,
        pred: impl Fn(&MobileInfo) -> bool,
    ) -> Option<MobileInfo> {
        tokio::time::timeout(timeout, async {
            loop {
                if let Some(m) = self
                    .mobiles
                    .borrow()
                    .iter()
                    .find(|m| m.esn == Some(esn) && pred(m))
                    .cloned()
                {
                    return m;
                }
                if self.mobiles.changed().await.is_err() {
                    futures_pending().await;
                }
            }
        })
        .await
        .ok()
    }

    pub async fn shutdown(mut self) {
        {
            let mut ph = self.pipe.lock().unwrap();
            ph.close_rx();
        }
        self.drain_stop.store(true, Ordering::Relaxed);
        if let Some(d) = self.drain.take() {
            let _ = d.join();
        }
        if let Some(t) = self.bts_task.take() {
            let _ = t.await;
        }
        self.keepalive.take();
    }
}

impl Drop for RunningNode {
    /// Without closing the pipe an unpaced BTS never ends and runtime teardown hangs.
    fn drop(&mut self) {
        if let Ok(mut ph) = self.pipe.lock() {
            ph.close_rx();
        }
        self.drain_stop.store(true, Ordering::Relaxed);
    }
}

async fn futures_pending() {
    std::future::pending::<()>().await
}

use cdma_bts::bts::rx::InjectedRxBlock;

pub struct SimRadio {
    pipe: Arc<StdMutex<RadioPipeHandle>>,
    forward: Arc<StdMutex<VecDeque<Complex32>>>,
    reverse_chip: u64,
    on_channel: bool,
}

pub fn node_forward_hz() -> f64 {
    cdma_common::band_class::ChannelPlan::new(cdma_common::band_class::BandClass::Bc0, 0, CDMA_FREQ)
        .downlink_hz() as f64
}

const ON_CHANNEL_TOLERANCE_HZ: f64 = 200_000.0;

pub struct SimHandle {
    pub mobiles: watch::Receiver<Vec<MobileInfo>>,
    pub sms_tx: tokio::sync::mpsc::Sender<SmsRequest>,
    pub msc: Arc<RecordingMscClient>,
    pipe: Arc<StdMutex<RadioPipeHandle>>,
    drain_stop: Arc<AtomicBool>,
    drain: Option<thread::JoinHandle<()>>,
    bts_task: Option<tokio::task::JoinHandle<()>>,
    keepalive: Option<NodeKeepAlive>,
}

pub fn boot_sim(bts_blocks: usize) -> (SimRadio, SimHandle) {
    let BootedNode {
        bts,
        pipe,
        mobiles,
        sms_tx,
        msc,
        clock: _,
        tx_rx_anchor: _,
        keepalive,
    } = boot();
    let pipe = Arc::new(StdMutex::new(pipe));
    let forward = Arc::new(StdMutex::new(VecDeque::<Complex32>::new()));

    let drain_stop = Arc::new(AtomicBool::new(false));
    let drain = {
        let pipe = pipe.clone();
        let forward = forward.clone();
        let stop = drain_stop.clone();
        thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let samples = {
                    let ph = pipe.lock().unwrap();
                    ph.drain_tx_samples()
                };
                if samples.is_empty() {
                    thread::sleep(Duration::from_millis(2));
                } else {
                    forward.lock().unwrap().extend(samples);
                }
            }
        })
    };

    let bts_task = tokio::task::spawn_blocking(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("bts sim runtime");
        rt.block_on(async move {
            let _ = bts.run_for_blocks_paced(bts_blocks).await;
        });
    });

    let radio = SimRadio {
        pipe: pipe.clone(),
        forward,
        reverse_chip: 0,
        on_channel: true,
    };
    let handle = SimHandle {
        mobiles,
        sms_tx,
        msc,
        pipe,
        drain_stop,
        drain: Some(drain),
        bts_task: Some(bts_task),
        keepalive: Some(keepalive),
    };
    (radio, handle)
}

impl SimRadio {
    fn inject_chunked(&self, samples: &[Complex32], mut chip: u64) {
        const BLOCK: usize = 32_768;
        let oversample = (SAMPLE_RATE_HZ / CHIP_RATE_HZ) as u64;
        let mut off = 0;
        while off < samples.len() {
            let end = (off + BLOCK).min(samples.len());
            let ph = self.pipe.lock().unwrap();
            let _ = ph.inject_rx(InjectedRxBlock {
                samples: samples[off..end].to_vec(),
                time_ns: 0,
                absolute_chip_start: Some(chip),
            });
            chip += ((end - off) / oversample as usize) as u64;
            off = end;
        }
    }
}

impl Radio for SimRadio {
    fn sample_rate_hz(&self) -> f64 {
        SAMPLE_RATE_HZ as f64
    }

    fn tune_forward(&mut self, frequency_hz: f64) -> Result<(), cdma_common::error::Error> {
        self.on_channel = (frequency_hz - node_forward_hz()).abs() <= ON_CHANNEL_TOLERANCE_HZ;
        Ok(())
    }

    fn read_forward(&mut self, out: &mut Vec<Complex32>) {
        const CHUNK: usize = 65_536;
        let mut buf = self.forward.lock().unwrap();
        if buf.is_empty() {
            drop(buf);
            thread::sleep(Duration::from_millis(1));
            return;
        }
        let take = CHUNK.min(buf.len());
        if self.on_channel {
            out.extend(buf.drain(..take));
        } else {
            buf.drain(..take);
            out.resize(out.len() + take, Complex32::new(0.0, 0.0));
        }
    }

    fn write_reverse(&mut self, burst: &ReverseBurst) {
        let oversample = (SAMPLE_RATE_HZ / CHIP_RATE_HZ) as u64;
        let start = burst.absolute_chip_start.max(self.reverse_chip);
        if start > self.reverse_chip {
            let idle =
                vec![Complex32::new(0.0, 0.0); ((start - self.reverse_chip) * oversample) as usize];
            self.inject_chunked(&idle, self.reverse_chip);
        }
        self.inject_chunked(&burst.samples, start);
        self.reverse_chip = start + burst.samples.len() as u64 / oversample;
    }
}

impl SimHandle {
    pub async fn wait_for_mobile(&mut self, esn: u32, timeout: Duration) -> Option<MobileInfo> {
        tokio::time::timeout(timeout, async {
            loop {
                if let Some(m) = self
                    .mobiles
                    .borrow()
                    .iter()
                    .find(|m| m.esn == Some(esn))
                    .cloned()
                {
                    return m;
                }
                if self.mobiles.changed().await.is_err() {
                    futures_pending().await;
                }
            }
        })
        .await
        .ok()
    }

    pub async fn wait_for_mo_sms(&self, burst_type: u8, timeout: Duration) -> Option<Vec<u8>> {
        tokio::time::timeout(timeout, async {
            loop {
                if let Some(rec) = self
                    .msc
                    .adds_deliver()
                    .into_iter()
                    .find(|r| r.burst_type == burst_type)
                {
                    return rec.data;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .ok()
    }

    pub async fn shutdown(mut self) {
        {
            let mut ph = self.pipe.lock().unwrap();
            ph.close_rx();
        }
        self.drain_stop.store(true, Ordering::Relaxed);
        if let Some(d) = self.drain.take() {
            let _ = d.join();
        }
        if let Some(t) = self.bts_task.take() {
            let _ = t.await;
        }
        self.keepalive.take();
    }
}

impl Drop for SimHandle {
    /// Close the pipe before joining. A held handle otherwise stalls runtime teardown until the BTS block limit.
    fn drop(&mut self) {
        if let Ok(mut ph) = self.pipe.lock() {
            ph.close_rx();
        }
        self.drain_stop.store(true, Ordering::Relaxed);
    }
}

impl crate::appliance::LoopbackControl for SimHandle {
    fn page_with_sms(&self, esn: u32, text: &str) -> Result<(), String> {
        let req = SmsRequest {
            originating_number: "5550001".to_string(),
            text: text.to_string(),
            target_address: Some(format!("ESN:0x{esn:08X}")),
            target_subscriber_id: None,
            timeout_ms: None,
            destination_number: None,
            sms_id: None,
            delivery_attempt_id: None,
            a1_tag: None,
            raw_payload: None,
        };
        self.sms_tx
            .try_send(req)
            .map_err(|_| "BSC is not accepting SMS".to_string())
    }

    fn mobiles(&self) -> Vec<(String, String, String)> {
        self.mobiles
            .borrow()
            .iter()
            .map(|m| {
                (
                    m.esn.map(|e| format!("0x{e:08X}")).unwrap_or_default(),
                    m.imsi.clone().unwrap_or_default(),
                    m.state.clone(),
                )
            })
            .collect()
    }
}

use cdma_common::error::Error;

use crate::radio::TxCalibration;
use crate::sdr_radio::SdrRadio;

struct FakeSdrShared {
    pipe: Arc<StdMutex<RadioPipeHandle>>,
    clock: Instant,
    /// The BTS's published hardware-tick ↔ absolute-chip anchor.
    anchor: Arc<TxRxAnchor>,
    tick_rate: u64,
    chip_rate: u64,
    oversample: u64,
    /// Extra one-way group delay applied to reverse samples, in chips. Models
    /// the radio's receive-to-transmit latency plus cable/air delay the mobile
    /// cannot observe, so a correct `tx_delay_samples` calibration cancels it.
    group_delay_chips: i64,
    on_channel: AtomicBool,
}

impl FakeSdrShared {
    fn hardware_time(&self) -> u64 {
        self.clock.elapsed().as_nanos() as u64
    }

    fn tick_to_chip(&self, tick: u64) -> Option<u64> {
        let (anchor_tick, anchor_chip) = self.anchor.try_load()?;
        let delta_ticks = tick as i128 - anchor_tick as i128;
        let delta_chips = delta_ticks * self.chip_rate as i128 / self.tick_rate as i128;
        let chip = anchor_chip as i128 + delta_chips + self.group_delay_chips as i128;
        (chip >= 0).then_some(chip as u64)
    }
}

struct FakeSdrRx {
    shared: Arc<FakeSdrShared>,
    pending: VecDeque<Complex32>,
    pending_tick: u64,
    have_tick: bool,
}

impl RadioRx for FakeSdrRx {
    fn tick_rate(&self) -> u64 {
        self.shared.tick_rate
    }

    fn get_hardware_time(&self) -> Result<u64, Error> {
        Ok(self.shared.hardware_time())
    }

    fn rx_read(&mut self, buf: &mut [Complex32], timeout_us: i64) -> Result<RxReadResult, Error> {
        if self.pending.is_empty() {
            let deadline = Instant::now() + Duration::from_micros(timeout_us.max(0) as u64);
            loop {
                let block = { self.shared.pipe.lock().unwrap().try_recv_tx() };
                if let Some(block) = block {
                    if let Some(t) = block.tick {
                        self.pending_tick = t;
                        self.have_tick = true;
                    }
                    self.pending.extend(block.samples);
                    break;
                }
                if Instant::now() >= deadline {
                    return Ok(RxReadResult {
                        samples_read: 0,
                        time_ticks: 0,
                        overflow: false,
                    });
                }
                thread::sleep(Duration::from_millis(1));
            }
        }

        let n = buf.len().min(self.pending.len());
        let on_channel = self.shared.on_channel.load(Ordering::Relaxed);
        for slot in buf.iter_mut().take(n) {
            let s = self.pending.pop_front().unwrap();
            *slot = if on_channel {
                s
            } else {
                Complex32::new(0.0, 0.0)
            };
        }
        let time_ticks = if self.have_tick { self.pending_tick } else { 0 };
        let advanced =
            (n as f64 * self.shared.tick_rate as f64 / SAMPLE_RATE_HZ as f64).round() as u64;
        self.pending_tick = self.pending_tick.saturating_add(advanced);
        Ok(RxReadResult {
            samples_read: n,
            time_ticks,
            overflow: false,
        })
    }

    fn rx_activate(&mut self, _time_ticks: Option<u64>) -> Result<(), Error> {
        Ok(())
    }

    fn rx_deactivate(&mut self) -> Result<(), Error> {
        Ok(())
    }

    fn set_rx_frequency(&mut self, frequency_hz: f64) -> Result<(), Error> {
        let on = (frequency_hz - node_forward_hz()).abs() <= ON_CHANNEL_TOLERANCE_HZ;
        self.shared.on_channel.store(on, Ordering::Relaxed);
        Ok(())
    }

    fn set_rx_gain(&mut self, _gain_db: f64) -> Result<(), Error> {
        Ok(())
    }
}

struct FakeSdrTx {
    shared: Arc<FakeSdrShared>,
    next_reverse_chip: Option<u64>,
}

impl RadioTx for FakeSdrTx {
    fn tick_rate(&self) -> u64 {
        self.shared.tick_rate
    }

    fn get_hardware_time(&self) -> Result<u64, Error> {
        Ok(self.shared.hardware_time())
    }

    fn transmit(&mut self, samples: &[Complex32]) -> Result<(), Error> {
        self.transmit_shaped_at(samples, None)
    }

    fn transmit_at(&mut self, samples: &[Complex32], tick: Option<u64>) -> Result<(), Error> {
        self.transmit_shaped_at(samples, tick)
    }

    fn enable_transmit(&mut self, _enable: bool) -> Result<(), Error> {
        Ok(())
    }

    fn enable_transmit_at(&mut self, _enable: bool, _tick: Option<u64>) -> Result<(), Error> {
        Ok(())
    }

    fn set_tx_frequency_hz(&mut self, _frequency_hz: f64) -> Result<(), Error> {
        Ok(())
    }

    fn transmit_shaped_at(
        &mut self,
        samples: &[Complex32],
        tick: Option<u64>,
    ) -> Result<(), Error> {
        let Some(tick) = tick else {
            return Ok(());
        };
        let deadline = Instant::now() + Duration::from_secs(2);
        let chip0 = loop {
            if let Some(c) = self.shared.tick_to_chip(tick) {
                break c;
            }
            if Instant::now() >= deadline {
                return Err("fake sdr: BTS TX anchor never published".into());
            }
            thread::sleep(Duration::from_millis(5));
        };
        const BLOCK: usize = 32_768;
        let os = self.shared.oversample;
        let pipe = self.shared.pipe.lock().unwrap();
        if let Some(mut chip) = self.next_reverse_chip {
            if chip0 < chip {
                return Err("fake sdr: reverse bursts overlap".into());
            }
            while chip < chip0 {
                let len = ((chip0 - chip).min(BLOCK as u64 / os) * os) as usize;
                if pipe
                    .inject_rx(InjectedRxBlock {
                        samples: vec![Complex32::new(0.0, 0.0); len],
                        time_ns: 0,
                        absolute_chip_start: Some(chip),
                    })
                    .is_err()
                {
                    return Ok(());
                }
                chip += len as u64 / os;
            }
        }
        let mut off = 0usize;
        while off < samples.len() {
            let end = (off + BLOCK).min(samples.len());
            let _ = pipe.inject_rx(InjectedRxBlock {
                samples: samples[off..end].to_vec(),
                time_ns: 0,
                absolute_chip_start: Some(chip0 + (off as u64 / os)),
            });
            off = end;
        }
        self.next_reverse_chip = Some(chip0 + samples.len() as u64 / os);
        Ok(())
    }
}

pub struct FakeSdrNode {
    pub mobiles: watch::Receiver<Vec<MobileInfo>>,
    pub sms_tx: tokio::sync::mpsc::Sender<SmsRequest>,
    pub msc: Arc<RecordingMscClient>,
    pipe: Arc<StdMutex<RadioPipeHandle>>,
    bts_task: Option<tokio::task::JoinHandle<()>>,
    keepalive: Option<NodeKeepAlive>,
}

pub fn boot_fake_sdr(
    bts_blocks: usize,
    group_delay_chips: i64,
    calibration: TxCalibration,
) -> (SdrRadio, FakeSdrNode) {
    boot_fake_sdr_with_pilot_pn(bts_blocks, group_delay_chips, calibration, PILOT_PN)
}

pub fn boot_fake_sdr_with_pilot_pn(
    bts_blocks: usize,
    group_delay_chips: i64,
    calibration: TxCalibration,
    pilot_pn: u16,
) -> (SdrRadio, FakeSdrNode) {
    let BootedNode {
        bts,
        pipe,
        mobiles,
        sms_tx,
        msc,
        clock,
        tx_rx_anchor,
        keepalive,
    } = boot_with_pilot_pn(pilot_pn);
    let pipe = Arc::new(StdMutex::new(pipe));

    let shared = Arc::new(FakeSdrShared {
        pipe: pipe.clone(),
        clock,
        anchor: tx_rx_anchor,
        tick_rate: 1_000_000_000,
        chip_rate: CHIP_RATE_HZ as u64,
        oversample: (SAMPLE_RATE_HZ / CHIP_RATE_HZ) as u64,
        group_delay_chips,
        on_channel: AtomicBool::new(true),
    });

    let rx = FakeSdrRx {
        shared: shared.clone(),
        pending: VecDeque::new(),
        pending_tick: 0,
        have_tick: false,
    };
    let tx = FakeSdrTx {
        shared: shared.clone(),
        next_reverse_chip: None,
    };

    let radio = SdrRadio::from_parts(
        Box::new(tx),
        Box::new(rx),
        1_000_000_000,
        SAMPLE_RATE_HZ as f64,
        calibration,
    );

    let bts_task = tokio::task::spawn_blocking(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("bts fake-sdr runtime");
        rt.block_on(async move {
            let _ = bts.run_for_blocks_paced(bts_blocks).await;
        });
    });

    let node = FakeSdrNode {
        mobiles,
        sms_tx,
        msc,
        pipe,
        bts_task: Some(bts_task),
        keepalive: Some(keepalive),
    };
    (radio, node)
}

impl FakeSdrNode {
    pub async fn wait_for_mobile(&mut self, esn: u32, timeout: Duration) -> Option<MobileInfo> {
        tokio::time::timeout(timeout, async {
            loop {
                if let Some(m) = self
                    .mobiles
                    .borrow()
                    .iter()
                    .find(|m| m.esn == Some(esn))
                    .cloned()
                {
                    return m;
                }
                if self.mobiles.changed().await.is_err() {
                    futures_pending().await;
                }
            }
        })
        .await
        .ok()
    }

    pub async fn wait_for_mo_sms(&self, burst_type: u8, timeout: Duration) -> Option<Vec<u8>> {
        tokio::time::timeout(timeout, async {
            loop {
                if let Some(rec) = self
                    .msc
                    .adds_deliver()
                    .into_iter()
                    .find(|r| r.burst_type == burst_type)
                {
                    return rec.data;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .ok()
    }

    pub async fn shutdown(mut self) {
        {
            let mut ph = self.pipe.lock().unwrap();
            ph.close_rx();
        }
        if let Some(t) = self.bts_task.take() {
            let _ = t.await;
        }
        self.keepalive.take();
    }
}

impl Drop for FakeSdrNode {
    fn drop(&mut self) {
        if let Ok(mut ph) = self.pipe.lock() {
            ph.close_rx();
        }
    }
}
