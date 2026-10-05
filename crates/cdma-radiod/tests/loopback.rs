use std::net::{Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use cdma_bts::sdr::network::wire::TxTransport;
use cdma_bts::sdr::network::{NetworkRadio, NetworkRadioOptions};
use cdma_bts::sdr::{Radio, RadioRx, RadioTx, RxReadResult};
use cdma_common::error::Error;
use cdma_radiod::{RadioCalibration, RxDefaults, serve};
use num_complex::Complex32;
use parking_lot::Mutex;

/// UHD-style master clock: 40 ticks per chip, not nanoseconds.
const INNER_TICK_RATE: u64 = 49_152_000;
const TX_SAMPLE_DELAY: i64 = -70;
const TX_RATE_HZ: u64 = 4_915_200;
const TX_BLOCK_SAMPLES: usize = 12_288;
const RX_RATE_HZ: f64 = 4_915_200.0;
const RX_CHUNK: usize = 2048;

const COMPOSITE_TX_RATE_HZ: u64 = 9_830_400;
const COMPOSITE_BANDWIDTH_HZ: usize = 5_890_000;
const COMPOSITE_RX_CENTER_HZ: f64 = 846_105_000.0;
const COMPOSITE_BLOCK_SAMPLES: usize = 24_576;

#[derive(Default)]
struct FakeState {
    tx_frequency: Option<usize>,
    tx_sample_rate: Option<usize>,
    tx_bandwidth: Option<usize>,
    rx_setup: Option<(usize, String, f64, f64, f64, Option<f64>)>,
    transmit_enabled: Option<(bool, Option<u64>)>,
    transmits: Vec<(Option<u64>, Vec<Complex32>)>,
    rx_activated: bool,
}

struct FakeClock {
    start: Instant,
}

impl FakeClock {
    fn now_ticks(&self) -> u64 {
        (self.start.elapsed().as_nanos() as u128 * INNER_TICK_RATE as u128 / 1_000_000_000) as u64
    }
}

struct FakeRadio {
    state: Arc<Mutex<FakeState>>,
    clock: Arc<FakeClock>,
}

impl Radio for FakeRadio {
    fn tick_rate(&self) -> u64 {
        INNER_TICK_RATE
    }
    fn set_tx_frequency(&mut self, center_frequency: usize) -> Result<(), Error> {
        self.state.lock().tx_frequency = Some(center_frequency);
        Ok(())
    }
    fn set_tx_sample_rate(&mut self, sample_rate: usize) -> Result<usize, Error> {
        self.state.lock().tx_sample_rate = Some(sample_rate);
        Ok(sample_rate)
    }
    fn set_tx_bandwidth(&mut self, bandwidth: usize) -> Result<(), Error> {
        self.state.lock().tx_bandwidth = Some(bandwidth);
        Ok(())
    }
    fn setup_rx(
        &mut self,
        channel: usize,
        antenna: &str,
        frequency_hz: f64,
        sample_rate_hz: f64,
        bandwidth_hz: f64,
        gain_db: Option<f64>,
    ) -> Result<(), Error> {
        self.state.lock().rx_setup = Some((
            channel,
            antenna.to_string(),
            frequency_hz,
            sample_rate_hz,
            bandwidth_hz,
            gain_db,
        ));
        Ok(())
    }
    fn split(self: Box<Self>) -> Result<(Box<dyn RadioTx>, Option<Box<dyn RadioRx>>), Error> {
        let active = Arc::new(AtomicBool::new(false));
        Ok((
            Box::new(FakeTx {
                state: self.state.clone(),
                clock: self.clock.clone(),
            }),
            Some(Box::new(FakeRx {
                state: self.state,
                clock: self.clock,
                active,
                next_sample: 0,
                stream_start_tick: None,
            })),
        ))
    }
}

struct FakeTx {
    state: Arc<Mutex<FakeState>>,
    clock: Arc<FakeClock>,
}

impl RadioTx for FakeTx {
    fn tick_rate(&self) -> u64 {
        INNER_TICK_RATE
    }
    fn get_hardware_time(&self) -> Result<u64, Error> {
        Ok(self.clock.now_ticks())
    }
    fn transmit(&mut self, samples: &[Complex32]) -> Result<(), Error> {
        self.state.lock().transmits.push((None, samples.to_vec()));
        Ok(())
    }
    fn transmit_at(&mut self, samples: &[Complex32], tick: Option<u64>) -> Result<(), Error> {
        self.state.lock().transmits.push((tick, samples.to_vec()));
        Ok(())
    }
    fn enable_transmit(&mut self, enable: bool) -> Result<(), Error> {
        self.state.lock().transmit_enabled = Some((enable, None));
        Ok(())
    }
    fn enable_transmit_at(&mut self, enable: bool, tick: Option<u64>) -> Result<(), Error> {
        self.state.lock().transmit_enabled = Some((enable, tick));
        Ok(())
    }
}

struct FakeRx {
    state: Arc<Mutex<FakeState>>,
    clock: Arc<FakeClock>,
    active: Arc<AtomicBool>,
    next_sample: u64,
    stream_start_tick: Option<u64>,
}

fn ramp_value(n: u64) -> f32 {
    (n % 1000) as f32 / 1000.0
}

impl RadioRx for FakeRx {
    fn tick_rate(&self) -> u64 {
        INNER_TICK_RATE
    }
    fn get_hardware_time(&self) -> Result<u64, Error> {
        Ok(self.clock.now_ticks())
    }
    fn rx_read(&mut self, buf: &mut [Complex32], _timeout_us: i64) -> Result<RxReadResult, Error> {
        if !self.active.load(Ordering::Relaxed) {
            std::thread::sleep(Duration::from_millis(1));
            return Ok(RxReadResult {
                samples_read: 0,
                time_ticks: self.clock.now_ticks(),
                overflow: false,
            });
        }
        std::thread::sleep(Duration::from_micros(
            (RX_CHUNK as f64 / RX_RATE_HZ * 1_000_000.0) as u64,
        ));
        let start_tick = *self
            .stream_start_tick
            .get_or_insert_with(|| self.clock.now_ticks());
        let n = buf.len().min(RX_CHUNK);
        for (i, slot) in buf[..n].iter_mut().enumerate() {
            let sample = self.next_sample + i as u64;
            *slot = Complex32::new(ramp_value(sample), 0.0);
        }
        let time_ticks = start_tick
            + (self.next_sample as u128 * INNER_TICK_RATE as u128 / RX_RATE_HZ as u128) as u64;
        self.next_sample += n as u64;
        Ok(RxReadResult {
            samples_read: n,
            time_ticks,
            overflow: false,
        })
    }
    fn rx_activate(&mut self, _time_ticks: Option<u64>) -> Result<(), Error> {
        self.state.lock().rx_activated = true;
        self.active.store(true, Ordering::Relaxed);
        Ok(())
    }
    fn rx_deactivate(&mut self) -> Result<(), Error> {
        self.state.lock().rx_activated = false;
        self.active.store(false, Ordering::Relaxed);
        Ok(())
    }
}

fn start_server(state: Arc<Mutex<FakeState>>) -> (std::thread::JoinHandle<()>, SocketAddr) {
    start_server_with_calibration(
        state,
        RadioCalibration {
            tx_sample_delay: Some(TX_SAMPLE_DELAY),
            tx_full_scale_power_estimate_dbm: Some(-6.0),
            rx_reference_dbm: Some(-39.0),
        },
    )
}

fn start_server_with_calibration(
    state: Arc<Mutex<FakeState>>,
    calibration: RadioCalibration,
) -> (std::thread::JoinHandle<()>, SocketAddr) {
    let radio = Box::new(FakeRadio {
        state,
        clock: Arc::new(FakeClock {
            start: Instant::now(),
        }),
    });
    let (ready_tx, ready_rx) = std::sync::mpsc::channel();
    let handle = std::thread::spawn(move || {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()
            .expect("server runtime");
        rt.block_on(async move {
            let (tonic_ready_tx, tonic_ready_rx) = tokio::sync::oneshot::channel();
            let localhost = SocketAddr::new(Ipv4Addr::LOCALHOST.into(), 0);
            let serve_fut = serve(
                radio,
                true,
                "fake".to_string(),
                RxDefaults {
                    antenna: "FAKE_RX".to_string(),
                    gain_db: Some(30.0),
                },
                calibration,
                localhost,
                localhost,
                Some(tonic_ready_tx),
                std::future::pending(),
            );
            tokio::pin!(serve_fut);
            tokio::select! {
                result = &mut serve_fut => panic!("server exited early: {result:?}"),
                ready = tonic_ready_rx => {
                    let (control_addr, _data_addr) = ready.expect("server ready");
                    ready_tx.send(control_addr).expect("report ready");
                    let _ = serve_fut.await;
                }
            }
        });
    });
    let control_addr = ready_rx
        .recv_timeout(Duration::from_secs(10))
        .expect("server came up");
    (handle, control_addr)
}

#[test]
fn client_connects_from_multithreaded_tokio_runtime() {
    let state = Arc::new(Mutex::new(FakeState::default()));
    let (_server, control_addr) = start_server(state);
    let rt = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .expect("client runtime");

    rt.block_on(async {
        let mut radio =
            NetworkRadio::connect(&control_addr.to_string(), NetworkRadioOptions::default())
                .expect("client connects from async runtime");
        radio
            .set_tx_frequency(881_520_000)
            .expect("control RPC from async runtime");
    });
}

#[test]
fn loopback_tx_rx_end_to_end() {
    let state = Arc::new(Mutex::new(FakeState::default()));
    let (_server, control_addr) = start_server(state.clone());

    let mut radio = Box::new(
        NetworkRadio::connect(&control_addr.to_string(), NetworkRadioOptions::default())
            .expect("client connects"),
    );
    assert_eq!(radio.tick_rate(), 1_000_000_000);
    assert_eq!(radio.tx_sample_delay(), Some(TX_SAMPLE_DELAY));
    assert_eq!(radio.tx_full_scale_power_estimate_dbm(), Some(-6.0));
    assert_eq!(radio.rx_reference_dbm(), Some(-39.0));

    radio.set_tx_bandwidth(1_250_000).expect("bandwidth");
    radio
        .set_tx_sample_rate(TX_RATE_HZ as usize)
        .expect("sample rate");
    radio.set_tx_frequency(881_520_000).expect("frequency");
    radio
        .setup_rx(0, "", 836_520_000.0, RX_RATE_HZ, 2_500_000.0, None)
        .expect("setup rx");
    {
        let state = state.lock();
        assert_eq!(state.tx_frequency, Some(881_520_000));
        assert_eq!(state.tx_sample_rate, Some(TX_RATE_HZ as usize));
        assert_eq!(state.tx_bandwidth, Some(1_250_000));
        // Empty antenna and unset gain resolve to the server-side defaults.
        let rx = state.rx_setup.as_ref().expect("rx setup reached inner");
        assert_eq!(rx.1, "FAKE_RX");
        assert_eq!(rx.5, Some(30.0));
    }

    let (mut tx, rx) = (radio as Box<dyn Radio>).split().expect("split");
    let mut rx = rx.expect("rx half");

    let t0 = tx.get_hardware_time().expect("time");
    std::thread::sleep(Duration::from_millis(5));
    let t1 = tx.get_hardware_time().expect("time");
    assert!(t1 > t0, "clock not advancing: {t0} {t1}");
    assert!(t1 - t0 >= 4_000_000, "clock too slow: {t0} {t1}");
    assert!(t1 - t0 < 50_000_000, "clock too fast: {t0} {t1}");

    tx.enable_transmit_at(true, Some(t1 + 100_000_000))
        .expect("enable tx");

    let batch_ns = TX_BLOCK_SAMPLES as u64 * 1_000_000_000 / TX_RATE_HZ;
    let start_ns = t1 + 200_000_000;
    for batch in 0..2u64 {
        let samples: Vec<Complex32> = (0..TX_BLOCK_SAMPLES)
            .map(|i| {
                let v = ((batch * TX_BLOCK_SAMPLES as u64 + i as u64) % 500) as f32 / 500.0 - 0.5;
                Complex32::new(v, -v)
            })
            .collect();
        tx.transmit_at(&samples, Some(start_ns + batch * batch_ns))
            .expect("transmit");
    }

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let total: usize = state
            .lock()
            .transmits
            .iter()
            .map(|(_, samples)| samples.len())
            .sum();
        if total >= 2 * TX_BLOCK_SAMPLES {
            break;
        }
        assert!(Instant::now() < deadline, "TX samples never arrived");
        std::thread::sleep(Duration::from_millis(10));
    }

    {
        let state = state.lock();
        let (enabled, enable_tick) = state.transmit_enabled.expect("enable reached inner");
        assert!(enabled);
        assert_eq!(
            state.transmits.len(),
            2,
            "each network TX block must become one driver call"
        );
        assert!(
            state
                .transmits
                .iter()
                .all(|(_, samples)| samples.len() == TX_BLOCK_SAMPLES)
        );
        let expected =
            ((t1 + 100_000_000) as u128 * INNER_TICK_RATE as u128 / 1_000_000_000) as u64;
        let tick = enable_tick.expect("timed enable");
        assert!(tick.abs_diff(expected) <= 1, "{tick} vs {expected}");

        let (first_tick, _) = &state.transmits[0];
        let expected_first = (start_ns as u128 * INNER_TICK_RATE as u128 / 1_000_000_000) as u64;
        assert!(
            first_tick.expect("timed transmit").abs_diff(expected_first) <= 1,
            "{first_tick:?} vs {expected_first}"
        );

        let flat: Vec<Complex32> = state
            .transmits
            .iter()
            .flat_map(|(_, samples)| samples.iter().copied())
            .collect();
        for (i, sample) in flat.iter().take(2 * TX_BLOCK_SAMPLES).enumerate() {
            let v = (i % 500) as f32 / 500.0 - 0.5;
            assert!(
                (sample.re - v).abs() < 2.0 / 32767.0,
                "sample {i}: {} vs {v}",
                sample.re
            );
            assert!((sample.im + v).abs() < 2.0 / 32767.0);
        }

        let ticks: Vec<u64> = state
            .transmits
            .iter()
            .map(|(tick, _)| tick.expect("timed"))
            .collect();
        let samples_per: Vec<usize> = state
            .transmits
            .iter()
            .map(|(_, samples)| samples.len())
            .collect();
        for i in 1..ticks.len() {
            let expected_delta =
                (samples_per[i - 1] as u128 * INNER_TICK_RATE as u128 / TX_RATE_HZ as u128) as u64;
            let delta = ticks[i] - ticks[i - 1];
            assert!(
                delta.abs_diff(expected_delta) <= 1,
                "packet {i}: delta {delta} vs {expected_delta}"
            );
        }
    }

    rx.rx_activate(None).expect("rx activate");
    assert!(state.lock().rx_activated);

    let mut collected: Vec<Complex32> = Vec::new();
    let mut reads: Vec<(u64, usize)> = Vec::new();
    let mut buf = vec![Complex32::default(); 12_288];
    let deadline = Instant::now() + Duration::from_secs(10);
    while collected.len() < 40_000 {
        assert!(Instant::now() < deadline, "RX samples never arrived");
        let result = rx.rx_read(&mut buf, 250_000).expect("rx read");
        if result.samples_read == 0 {
            continue;
        }
        assert!(!result.overflow, "unexpected RX discontinuity on loopback");
        reads.push((result.time_ticks, result.samples_read));
        collected.extend_from_slice(&buf[..result.samples_read]);
    }

    for (i, sample) in collected.iter().enumerate() {
        let expected = ramp_value(i as u64);
        assert!(
            (sample.re - expected).abs() < 2.0 / 32767.0,
            "rx sample {i}: {} vs {expected}",
            sample.re
        );
    }
    let (first_ns, _) = reads[0];
    let mut offset = 0u64;
    for (time_ns, count) in &reads {
        let expected = first_ns + (offset as u128 * 1_000_000_000 / RX_RATE_HZ as u128) as u64;
        assert!(
            time_ns.abs_diff(expected) <= 1_000,
            "read at offset {offset}: {time_ns} vs {expected}"
        );
        offset += *count as u64;
    }

    rx.rx_deactivate().expect("rx deactivate");
    assert!(!state.lock().rx_activated);
    tx.enable_transmit(false).expect("disable tx");
}

#[test]
fn composite_rate_tx_block_arrives_intact() {
    let state = Arc::new(Mutex::new(FakeState::default()));
    let (_server, control_addr) = start_server(state.clone());

    let mut radio = Box::new(
        NetworkRadio::connect(&control_addr.to_string(), NetworkRadioOptions::default())
            .expect("client connects"),
    ) as Box<dyn Radio>;
    radio
        .set_tx_bandwidth(COMPOSITE_BANDWIDTH_HZ)
        .expect("bandwidth");
    assert_eq!(
        radio
            .set_tx_sample_rate(COMPOSITE_TX_RATE_HZ as usize)
            .expect("sample rate"),
        COMPOSITE_TX_RATE_HZ as usize
    );
    radio
        .setup_rx(
            0,
            "",
            COMPOSITE_RX_CENTER_HZ,
            COMPOSITE_TX_RATE_HZ as f64,
            COMPOSITE_BANDWIDTH_HZ as f64,
            None,
        )
        .expect("setup rx");
    let (mut tx, rx) = radio.split().expect("split");
    assert!(rx.is_some(), "composite reverse capture needs an RX half");

    let t0 = tx.get_hardware_time().expect("time");
    let start_ns = t0 + 200_000_000;
    let samples: Vec<Complex32> = (0..COMPOSITE_BLOCK_SAMPLES)
        .map(|i| {
            let v = (i % 500) as f32 / 500.0 - 0.5;
            Complex32::new(v, -v)
        })
        .collect();
    tx.transmit_at(&samples, Some(start_ns)).expect("transmit");

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        {
            let state = state.lock();
            if let Some((tick, received)) = state.transmits.first() {
                assert_eq!(received.len(), COMPOSITE_BLOCK_SAMPLES);
                let expected_tick =
                    (start_ns as u128 * INNER_TICK_RATE as u128 / 1_000_000_000) as u64;
                assert!(
                    tick.expect("timed transmit").abs_diff(expected_tick) <= 1,
                    "{tick:?} vs {expected_tick}"
                );
                for (i, sample) in received.iter().enumerate() {
                    let v = (i % 500) as f32 / 500.0 - 0.5;
                    assert!(
                        (sample.re - v).abs() < 2.0 / 32767.0,
                        "sample {i}: {} vs {v}",
                        sample.re
                    );
                    assert!((sample.im + v).abs() < 2.0 / 32767.0);
                }
                break;
            }
        }
        assert!(
            Instant::now() < deadline,
            "composite TX block never arrived"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    let state = state.lock();
    assert_eq!(
        state.tx_sample_rate,
        Some(COMPOSITE_TX_RATE_HZ as usize),
        "server must apply the composite TX rate to the inner radio"
    );
    let rx_setup = state.rx_setup.as_ref().expect("rx setup reached inner");
    assert_eq!(rx_setup.2, COMPOSITE_RX_CENTER_HZ);
    assert_eq!(rx_setup.3, COMPOSITE_TX_RATE_HZ as f64);
    assert_eq!(rx_setup.4, COMPOSITE_BANDWIDTH_HZ as f64);
}

#[test]
fn split_without_negotiated_tx_rate_is_rejected() {
    let state = Arc::new(Mutex::new(FakeState::default()));
    let (_server, control_addr) = start_server(state);

    let radio = Box::new(
        NetworkRadio::connect(&control_addr.to_string(), NetworkRadioOptions::default())
            .expect("client connects"),
    ) as Box<dyn Radio>;
    let error = radio.split().err().expect("split must reject");
    assert!(error.to_string().contains("set_tx_sample_rate"), "{error}");
}

#[test]
fn tcp_transport_delivers_whole_tx_blocks() {
    let state = Arc::new(Mutex::new(FakeState::default()));
    let (_server, control_addr) = start_server(state.clone());

    let mut radio = Box::new(
        NetworkRadio::connect(
            &control_addr.to_string(),
            NetworkRadioOptions {
                tx_transport: TxTransport::Tcp,
                ..NetworkRadioOptions::default()
            },
        )
        .expect("client connects"),
    );
    radio
        .set_tx_sample_rate(TX_RATE_HZ as usize)
        .expect("sample rate");

    let (mut tx, _rx) = (radio as Box<dyn Radio>).split().expect("split");
    let t0 = tx.get_hardware_time().expect("time");
    tx.enable_transmit_at(true, Some(t0 + 100_000_000))
        .expect("enable tx");

    let batch_ns = TX_BLOCK_SAMPLES as u64 * 1_000_000_000 / TX_RATE_HZ;
    let start_ns = t0 + 200_000_000;
    for batch in 0..2u64 {
        let samples: Vec<Complex32> = (0..TX_BLOCK_SAMPLES)
            .map(|i| {
                let v = ((batch * TX_BLOCK_SAMPLES as u64 + i as u64) % 500) as f32 / 500.0 - 0.5;
                Complex32::new(v, -v)
            })
            .collect();
        tx.transmit_at(&samples, Some(start_ns + batch * batch_ns))
            .expect("transmit");
    }

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        let total: usize = state
            .lock()
            .transmits
            .iter()
            .map(|(_, samples)| samples.len())
            .sum();
        if total >= 2 * TX_BLOCK_SAMPLES {
            break;
        }
        assert!(
            Instant::now() < deadline,
            "TX samples never arrived over TCP"
        );
        std::thread::sleep(Duration::from_millis(10));
    }

    let state = state.lock();
    assert_eq!(
        state.transmits.len(),
        2,
        "each network TX block must become one driver call"
    );
    let flat: Vec<Complex32> = state
        .transmits
        .iter()
        .flat_map(|(_, samples)| samples.iter().copied())
        .collect();
    for (i, sample) in flat.iter().take(2 * TX_BLOCK_SAMPLES).enumerate() {
        let v = (i % 500) as f32 / 500.0 - 0.5;
        assert!(
            (sample.re - v).abs() < 2.0 / 32767.0,
            "sample {i}: {} vs {v}",
            sample.re
        );
        assert!((sample.im + v).abs() < 2.0 / 32767.0);
    }
}

#[test]
fn silent_client_has_transmit_disabled_by_server() {
    let state = Arc::new(Mutex::new(FakeState::default()));
    let (_server, control_addr) = start_server(state.clone());

    let mut radio = Box::new(
        NetworkRadio::connect(&control_addr.to_string(), NetworkRadioOptions::default())
            .expect("client connects"),
    ) as Box<dyn Radio>;
    radio
        .set_tx_sample_rate(TX_RATE_HZ as usize)
        .expect("set tx sample rate");
    let (mut tx, _rx) = radio.split().expect("split");
    tx.enable_transmit(true).expect("enable tx");
    assert_eq!(state.lock().transmit_enabled, Some((true, None)));

    let deadline = Instant::now() + Duration::from_secs(5);
    while state.lock().transmit_enabled != Some((false, None)) {
        assert!(
            Instant::now() < deadline,
            "server never disabled TX for a silent client"
        );
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn untimed_transmit_uses_inner_untimed_path() {
    let state = Arc::new(Mutex::new(FakeState::default()));
    let (_server, control_addr) = start_server(state.clone());

    let mut radio = Box::new(
        NetworkRadio::connect(&control_addr.to_string(), NetworkRadioOptions::default())
            .expect("client connects"),
    ) as Box<dyn Radio>;
    radio
        .set_tx_sample_rate(TX_RATE_HZ as usize)
        .expect("set tx sample rate");
    let (mut tx, _rx) = radio.split().expect("split");

    let samples = vec![Complex32::new(0.25, -0.25); 100];
    tx.transmit(&samples).expect("untimed transmit");

    let deadline = Instant::now() + Duration::from_secs(5);
    loop {
        {
            let state = state.lock();
            if let Some((tick, samples)) = state.transmits.first() {
                assert!(tick.is_none(), "untimed transmit must stay untimed");
                assert_eq!(samples.len(), 100);
                break;
            }
        }
        assert!(Instant::now() < deadline, "untimed samples never arrived");
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[test]
fn get_hardware_time_tracks_server_clock() {
    let state = Arc::new(Mutex::new(FakeState::default()));
    let (_server, control_addr) = start_server(state);

    let mut radio = Box::new(
        NetworkRadio::connect(&control_addr.to_string(), NetworkRadioOptions::default())
            .expect("client connects"),
    ) as Box<dyn Radio>;
    radio
        .set_tx_sample_rate(TX_RATE_HZ as usize)
        .expect("set tx sample rate");
    let (tx, _rx) = radio.split().expect("split");

    let mut last = 0u64;
    for _ in 0..1000 {
        let now = tx.get_hardware_time().expect("time");
        assert!(now >= last);
        last = now;
    }
}

#[test]
fn mobile_inherits_server_tx_delay_and_preserves_explicit_overrides() {
    use cdma_bts::bts::config::RadioConfig;
    use cdma_ms::radio::{Radio as MobileRadio, TxCalibration};
    use cdma_ms::sdr_radio::SdrRadio;

    const FALLBACK_DELAY: i64 = -12;
    for (server_delay, client_delay, expected_delay) in [
        (Some(TX_SAMPLE_DELAY), None, TX_SAMPLE_DELAY),
        (Some(TX_SAMPLE_DELAY), Some(0), 0),
        (Some(0), None, 0),
        (None, None, FALLBACK_DELAY),
    ] {
        let state = Arc::new(Mutex::new(FakeState::default()));
        let (_server, control_addr) = start_server_with_calibration(
            state,
            RadioCalibration {
                tx_sample_delay: server_delay,
                ..Default::default()
            },
        );
        let config: RadioConfig = serde_json::from_value(serde_json::json!({
            "kind": "network",
            "addr": control_addr.to_string(),
            "tx_sample_delay": client_delay,
        }))
        .unwrap();
        let radio = SdrRadio::open(
            &config,
            881_520_000.0,
            836_520_000.0,
            RX_RATE_HZ,
            TxCalibration {
                tx_reference_dbm: 0.0,
                estimated_full_scale_dbm: None,
                tx_delay_samples: FALLBACK_DELAY,
                power_control: false,
                peak_limit: 1.0,
                relative_access_power: false,
                access_initial_backoff_db: 0.0,
            },
        )
        .unwrap();
        assert_eq!(
            radio.tx_calibration().unwrap().tx_delay_samples,
            expected_delay
        );
    }
}
