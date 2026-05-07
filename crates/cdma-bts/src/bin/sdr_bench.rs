//! SDR streaming performance benchmark.
//!
//! Isolates SDR TX/RX streaming from the BTS pipeline to find optimal batch
//! sizes and stream parameters that minimize dropped packets and underruns,
//! particularly on LimeSDR over USB 2.0.
//!
//! # Examples
//!
//! ## LimeSDR Mini 2.0 — sweep batch sizes
//! ```sh
//! cargo run --release -p cdma-bts --bin sdr_bench -- \
//!   --radio config/radio_limesdr_mini2_native.json
//! ```
//!
//! ## LimeSDR — sweep batch sizes + throughput tradeoff
//! ```sh
//! cargo run --release -p cdma-bts --bin sdr_bench -- \
//!   --radio config/radio_limesdr_mini2_native.json --sweep-throughput
//! ```
//!
//! ## LimeSDR — sweep batch sizes + FIFO sizes
//! ```sh
//! cargo run --release -p cdma-bts --bin sdr_bench -- \
//!   --radio config/radio_limesdr_mini2_native.json --sweep-fifo
//! ```
//!
//! ## LimeSDR — full duplex stress test
//! ```sh
//! cargo run --release -p cdma-bts --bin sdr_bench -- \
//!   --radio config/radio_limesdr_mini2_native.json --full-duplex
//! ```
//!
//! ## bladeRF Micro 2.0 — sweep batch sizes
//! ```sh
//! cargo run --release -p cdma-bts --bin sdr_bench -- \
//!   --radio config/radio_bladerf_micro2.json
//! ```
//!
//! ## bladeRF Micro 2.0 — full duplex stress test
//! ```sh
//! cargo run --release -p cdma-bts --bin sdr_bench -- \
//!   --radio config/radio_bladerf_micro2.json --full-duplex
//! ```

#![cfg_attr(
    not(any(
        feature = "lime-backend",
        feature = "uhd-backend",
        feature = "bladerf-backend"
    )),
    allow(dead_code, unused_variables, unused_imports, unreachable_code)
)]

#[allow(unused_imports)]
use std::fs;
#[allow(unused_imports)]
use std::path::PathBuf;
#[allow(unused_imports)]
use std::sync::Arc;
#[allow(unused_imports)]
use std::time::{Duration, Instant};

use clap::Parser;
#[allow(unused_imports)]
use log::{debug, info, warn};
use num_complex::Complex32;
use serde::Deserialize;

use cdma_common::consts::SR1_CHIP_RATE_HZ;

/// Samples per PCG at 4x oversample (1 PCG = 1536 chips * 4 = 6144 samples).
const SAMPLES_PER_PCG: usize = 1536 * 4;

/// Default TX frequency (CDMA band class 0 forward).
const DEFAULT_TX_FREQ_HZ: u64 = 881_520_000;

/// Default sample rate at 4x oversample.
const DEFAULT_SAMPLE_RATE_HZ: usize = SR1_CHIP_RATE_HZ as usize * 4;

// ---------------------------------------------------------------------------
// CLI
// ---------------------------------------------------------------------------

#[derive(Parser, Debug)]
#[command(
    author,
    version,
    about = "Benchmark SDR streaming performance (TX drop rate, underruns, FIFO usage)."
)]
struct Cli {
    /// Path to radio config JSON file.
    #[arg(long, value_name = "PATH")]
    radio: PathBuf,

    /// Duration in seconds for each test run.
    #[arg(long, default_value_t = 5)]
    duration: u64,

    /// TX center frequency in Hz.
    #[arg(long, default_value_t = DEFAULT_TX_FREQ_HZ)]
    tx_freq: u64,

    /// Comma-separated list of PCG counts per batch to sweep.
    #[arg(long, default_value = "1,2,4,8,16")]
    batch_pcgs: String,

    /// Also sweep throughput_vs_latency at 0.0, 0.25, 0.5, 0.75, 1.0.
    #[arg(long)]
    sweep_throughput: bool,

    /// Also sweep FIFO sizes at 256K, 512K, 1M, 2M, 4M samples.
    #[arg(long)]
    sweep_fifo: bool,

    /// Run RX simultaneously to stress the USB bus (default: true).
    /// Use --no-full-duplex to disable.
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    full_duplex: bool,

    /// Full matrix: enable --sweep-throughput, --sweep-fifo, and --full-duplex.
    #[arg(long)]
    all: bool,

    /// Extra margin (in ms) beyond the batch duration before a write
    /// is allowed. Total lookahead = batch_time + margin.
    /// Simulates BTS constraint: data can't be computed until close to
    /// real-time. Lower margin = tighter, more likely to drop.
    /// Default: 5ms. Sweep with --sweep-margin.
    #[arg(long, default_value_t = 5)]
    margin_ms: u32,

    /// Simulated signal generation delay in microseconds (fixed).
    /// Added as a busy-wait AFTER the pacing gate opens but BEFORE send(),
    /// representing the BTS pipeline computation time.
    /// Default: 1000 (1ms). Set to 0 to disable.
    #[arg(long, default_value_t = 1000)]
    gen_delay_us: u64,

    /// Sweep margin values: 0, 1, 2, 5, 10, 20, 50 ms.
    #[arg(long)]
    sweep_margin: bool,

    /// Spawn N CPU stress threads (busy-loop) to simulate pipeline load.
    /// Useful to test whether CPU contention causes USB drops.
    #[arg(long, default_value_t = 0)]
    stress_threads: usize,

    /// TX gain override in dB (uses config value if not specified).
    #[arg(long)]
    tx_gain_db: Option<u32>,

    /// RX gain override in dB (for full-duplex mode).
    #[arg(long)]
    rx_gain_db: Option<u32>,
}

// ---------------------------------------------------------------------------
// Radio config JSON parsing
// ---------------------------------------------------------------------------

#[derive(Debug, Deserialize)]
#[serde(rename_all = "snake_case", tag = "kind")]
#[allow(dead_code)]
enum RadioConfigFile {
    Lime {
        #[serde(default)]
        device: Option<String>,
        #[serde(default)]
        channel: Option<usize>,
        #[serde(default)]
        tx_antenna: Option<String>,
        #[serde(default)]
        tx_gain_db: Option<u32>,
        #[serde(default)]
        rx_antenna: Option<String>,
        #[serde(default)]
        rx_gain_db: Option<u32>,
        #[serde(default)]
        rx_freq_hz: Option<u64>,
        #[serde(default)]
        rx_sample_rate_hz: Option<usize>,
        #[serde(default)]
        rx_bandwidth_hz: Option<usize>,
        #[serde(default)]
        oversample: Option<usize>,
    },
    Uhd {
        device: String,
        #[serde(default)]
        channel: Option<usize>,
        #[serde(default)]
        antenna: Option<String>,
        #[serde(default)]
        tx_gain_db: Option<f64>,
        #[serde(default)]
        rx_antenna: Option<String>,
        #[serde(default)]
        rx_gain_db: Option<f64>,
        #[serde(default)]
        rx_freq_hz: Option<u64>,
        #[serde(default)]
        rx_sample_rate_hz: Option<usize>,
        #[serde(default)]
        rx_bandwidth_hz: Option<usize>,
        #[serde(default)]
        master_clock_rate: Option<u64>,
    },
    Soapy {
        device: String,
        #[serde(default)]
        channel: Option<usize>,
        #[serde(default)]
        antenna: Option<String>,
        #[serde(default)]
        tx_gain_db: Option<f64>,
        #[serde(default)]
        rx_antenna: Option<String>,
        #[serde(default)]
        rx_gain_db: Option<f64>,
    },
    BladeRf {
        #[serde(default)]
        device: Option<String>,
        #[serde(default)]
        channel: Option<u32>,
        #[serde(default)]
        tx_antenna: Option<String>,
        #[serde(default)]
        rx_antenna: Option<String>,
        #[serde(default)]
        tx_gain_db: Option<i32>,
        #[serde(default)]
        rx_gain_db: Option<i32>,
        #[serde(default)]
        fpga_path: Option<String>,
        #[serde(default)]
        num_buffers: Option<u32>,
        #[serde(default)]
        buffer_size: Option<u32>,
        #[serde(default)]
        num_transfers: Option<u32>,
        #[serde(default)]
        stream_timeout_ms: Option<u32>,
    },
    #[serde(other)]
    Other,
}

// ---------------------------------------------------------------------------
// Bench result for a single parameter combination
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
#[allow(dead_code)]
struct BenchResult {
    batch_pcgs: usize,
    samples_per_batch: usize,
    throughput_vs_latency: f32,
    fifo_size: u32,
    total_dropped: u32,
    total_underrun: u32,
    total_overrun: u32,
    fifo_avg: f64,
    fifo_max: u32,
    sends: u64,
    rx_reads: u64,
    rx_overflows: u64,
    elapsed_secs: f64,
}

impl BenchResult {
    fn verdict(&self) -> &'static str {
        if self.total_dropped == 0 && self.total_underrun == 0 {
            "PASS"
        } else if self.total_dropped <= 20 && self.total_underrun <= 5 {
            "WARN"
        } else {
            "FAIL"
        }
    }
}

// ---------------------------------------------------------------------------
// Signal generation
// ---------------------------------------------------------------------------

/// Stateful CDMA pilot signal generator matching the BTS TX path exactly:
/// spreader maintains PN state, shaper maintains FIR state across calls.
/// Each `generate()` call produces fresh samples with no discontinuities.
struct PilotSignalGenerator {
    spreader: cdma_bts::phy::spread::Spreader,
    shaper: cdma_bts::sdr::TxPulseShaper,
}

impl PilotSignalGenerator {
    fn new() -> Self {
        use cdma_bts::phy::spread::{PnSequence, Spreader};
        PilotSignalGenerator {
            spreader: Spreader::new(PnSequence::new(0, 32768)),
            shaper: cdma_bts::sdr::TxPulseShaper::new(),
        }
    }

    /// Generate `num_samples` shaped samples at 4× rate (4.9152 MHz).
    fn generate(&mut self, num_samples: usize) -> Vec<Complex32> {
        let oversample = 4;
        let num_chips = (num_samples + oversample - 1) / oversample;
        let pilot_symbol = Complex32::new(1.0, 0.0);

        let chip_samples: Vec<Complex32> = (0..num_chips)
            .map(|_| self.spreader.spread(&pilot_symbol))
            .collect();

        let shaped = self.shaper.shape(&chip_samples);
        shaped
    }
}

// ---------------------------------------------------------------------------
// Lime backend
// ---------------------------------------------------------------------------

#[cfg(feature = "lime-backend")]
mod lime_bench {
    use super::*;

    /// Resolve an antenna name to its LimeSuite index.
    fn resolve_antenna_index(
        device: &limesuite::Device,
        dir_tx: bool,
        chan: usize,
        name: &str,
    ) -> usize {
        if let Ok(list) = device.antenna_list(dir_tx, chan) {
            for (i, entry) in list.iter().enumerate() {
                if entry.eq_ignore_ascii_case(name) {
                    return i;
                }
            }
        }
        if dir_tx {
            0
        } else {
            match name.to_ascii_uppercase().as_str() {
                "LNAH" => 0,
                "LNAL" => 1,
                "LNAW" => 2,
                _ => 2,
            }
        }
    }

    pub fn run_lime_bench(
        cli: &Cli,
        device_str: &str,
        channel: usize,
        tx_antenna: &str,
        rx_antenna: &str,
        tx_gain_db: u32,
        rx_gain_db: u32,
        oversample: usize,
        batch_pcg_list: &[usize],
        throughput_list: &[f32],
        fifo_list: &[u32],
        margin_list: &[u32],
    ) -> Result<Vec<BenchResult>, Box<dyn std::error::Error>> {
        let sample_rate_hz = DEFAULT_SAMPLE_RATE_HZ;
        let mut results = Vec::new();

        println!("=== SDR Bench: LimeSDR ===");
        println!(
            "Sample rate: {} Hz ({}x oversample)",
            sample_rate_hz,
            sample_rate_hz / SR1_CHIP_RATE_HZ as usize
        );
        println!("TX frequency: {:.2} MHz", cli.tx_freq as f64 / 1_000_000.0);
        println!(
            "Full duplex: {}",
            if cli.full_duplex { "yes" } else { "no" }
        );
        println!("Duration per test: {} s", cli.duration);
        println!();

        for &fifo_size in fifo_list {
            for &throughput in throughput_list {
                for &margin_ms in margin_list {
                    for &batch_pcgs in batch_pcg_list {
                        let samples_per_batch = batch_pcgs * SAMPLES_PER_PCG;

                        info!(
                            "Testing: batch_pcgs={} samples={} throughput={:.2} fifo={} margin={}ms",
                            batch_pcgs, samples_per_batch, throughput, fifo_size, margin_ms
                        );

                        match run_single_lime_test(
                            cli,
                            device_str,
                            channel,
                            tx_antenna,
                            rx_antenna,
                            tx_gain_db,
                            rx_gain_db,
                            oversample,
                            sample_rate_hz,
                            batch_pcgs,
                            samples_per_batch,
                            throughput,
                            fifo_size,
                            margin_ms,
                        ) {
                            Ok(result) => {
                                let verdict =
                                    if result.total_dropped == 0 && result.total_underrun == 0 {
                                        "PASS"
                                    } else if result.total_dropped < 10 {
                                        "WARN"
                                    } else {
                                        "FAIL"
                                    };
                                println!(
                                    "  => pcgs={:<2} margin={:<3}ms tput={:.2} fifo={:>7} | dropped={:<6} underrun={:<4} fifo_avg={:<6.0} | {}",
                                    batch_pcgs,
                                    margin_ms,
                                    throughput,
                                    fifo_size,
                                    result.total_dropped,
                                    result.total_underrun,
                                    result.fifo_avg,
                                    verdict
                                );
                                results.push(result);
                            }
                            Err(e) => {
                                println!(
                                    "  => pcgs={:<2} margin={:<3}ms tput={:.2} fifo={:>7} | ERROR: {}",
                                    batch_pcgs, margin_ms, throughput, fifo_size, e
                                );
                            }
                        }

                        // Brief pause between tests to let the device settle.
                        std::thread::sleep(Duration::from_millis(500));
                    }
                } // margin loop
            }
        }

        Ok(results)
    }

    fn run_single_lime_test(
        cli: &Cli,
        device_str: &str,
        channel: usize,
        tx_antenna: &str,
        rx_antenna: &str,
        tx_gain_db: u32,
        rx_gain_db: u32,
        oversample: usize,
        sample_rate_hz: usize,
        batch_pcgs: usize,
        samples_per_batch: usize,
        throughput_vs_latency: f32,
        fifo_size: u32,
        margin_ms: u32,
    ) -> Result<BenchResult, Box<dyn std::error::Error>> {
        // Open device.
        let info = if device_str.is_empty() {
            None
        } else {
            Some(device_str)
        };
        let mut device = limesuite::Device::open(info).map_err(|e| format!("Lime: open: {}", e))?;
        device.init().map_err(|e| format!("Lime: init: {}", e))?;

        // Enable TX channel.
        device
            .enable_channel(true, channel, true)
            .map_err(|e| format!("Lime: enable TX: {}", e))?;

        // Set sample rate.
        device
            .set_sample_rate(sample_rate_hz as f64, oversample)
            .map_err(|e| format!("Lime: set sample rate: {}", e))?;

        // Report clock configuration.
        if let Ok(ref_clk) = device.get_clock_freq(0) {
            info!("REF clock: {:.6} MHz", ref_clk / 1e6);
        }
        if let Ok(cgen_clk) = device.get_clock_freq(3) {
            info!(
                "CGEN clock: {:.6} MHz (ratio to sample rate: {:.4})",
                cgen_clk / 1e6,
                cgen_clk / sample_rate_hz as f64
            );
        }
        if let Ok(actual_tx) = device.get_sample_rate(true, channel) {
            info!(
                "Actual TX sample rate: {:.6} Hz (requested: {})",
                actual_tx, sample_rate_hz
            );
        }
        if let Ok(actual_rx) = device.get_sample_rate(false, channel) {
            info!("Actual RX sample rate: {:.6} Hz", actual_rx);
        }

        // TX antenna.
        let tx_ant_idx = resolve_antenna_index(&device, true, channel, tx_antenna);
        device
            .set_antenna(true, channel, tx_ant_idx)
            .map_err(|e| format!("Lime: set TX antenna: {}", e))?;

        // TX frequency.
        device
            .set_lo_frequency(true, channel, cli.tx_freq as f64)
            .map_err(|e| format!("Lime: set TX freq: {}", e))?;

        // TX gain.
        device
            .set_gain_db(true, channel, tx_gain_db)
            .map_err(|e| format!("Lime: set TX gain: {}", e))?;

        // Calibrate TX.
        device
            .calibrate(true, channel, sample_rate_hz as f64)
            .map_err(|e| format!("Lime: calibrate TX: {}", e))?;

        // RX setup (if full-duplex).
        if cli.full_duplex {
            device
                .enable_channel(false, channel, true)
                .map_err(|e| format!("Lime: enable RX: {}", e))?;

            let rx_ant_idx = resolve_antenna_index(&device, false, channel, rx_antenna);
            device
                .set_antenna(false, channel, rx_ant_idx)
                .map_err(|e| format!("Lime: set RX antenna: {}", e))?;
            device
                .set_lo_frequency(false, channel, cli.tx_freq as f64)
                .map_err(|e| format!("Lime: set RX freq: {}", e))?;
            device
                .set_gain_db(false, channel, rx_gain_db)
                .map_err(|e| format!("Lime: set RX gain: {}", e))?;
            device
                .calibrate(false, channel, sample_rate_hz as f64)
                .map_err(|e| format!("Lime: calibrate RX: {}", e))?;
        }

        // Create TX stream.
        let mut tx_stream = limesuite::TxStream::with_throughput(
            &mut device,
            channel as u32,
            fifo_size,
            throughput_vs_latency,
        )
        .map_err(|e| format!("Lime: create TX stream: {}", e))?;

        // Create RX stream (if full-duplex).
        let mut rx_stream = if cli.full_duplex {
            Some(
                limesuite::RxStream::with_throughput(
                    &mut device,
                    channel as u32,
                    fifo_size,
                    throughput_vs_latency,
                )
                .map_err(|e| format!("Lime: create RX stream: {}", e))?,
            )
        } else {
            None
        };

        // Start streams.
        tx_stream
            .start()
            .map_err(|e| format!("Lime: start TX: {}", e))?;
        if let Some(ref mut rx) = rx_stream {
            rx.start().map_err(|e| format!("Lime: start RX: {}", e))?;
        }

        // Stateful signal generator — produces fresh samples each send.
        let mut sig_gen = PilotSignalGenerator::new();

        // Tracking variables.
        let mut sends: u64 = 0;
        let rx_reads: u64;
        let rx_overflows: u64;
        let mut fifo_sum: f64 = 0.0;
        let mut fifo_max: u32 = 0;
        let mut fifo_polls: u64 = 0;

        // Initial status baseline.
        let init_status = tx_stream
            .status()
            .map_err(|e| format!("Lime: TX status: {}", e))?;
        let init_underrun = init_status.underrun;
        let init_dropped = init_status.dropped_packets;

        // Margin: how far before playout the data becomes "available."
        // Simulates BTS real-time constraint — data can't be written
        // until margin_ms before its scheduled playout.
        let margin_samples = (sample_rate_hz as u64) * (margin_ms as u64) / 1000;

        // Spawn RX thread for true full-duplex (concurrent USB access).
        let rx_shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let rx_read_counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let rx_overflow_counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let rx_dropped_counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
        // Shared RX timestamp — the true free-running hardware clock.
        let rx_hw_clock = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let rx_thread = if let Some(mut rx) = rx_stream.take() {
            let shutdown = rx_shutdown.clone();
            let reads = rx_read_counter.clone();
            let overflows = rx_overflow_counter.clone();
            let rx_drops = rx_dropped_counter.clone();
            let clock = rx_hw_clock.clone();
            let spr_batch = samples_per_batch;
            Some(
                std::thread::Builder::new()
                    .name("sdr-bench-rx".into())
                    .spawn(move || {
                        let mut buf = vec![Complex32::new(0.0, 0.0); spr_batch];
                        let mut last_rx_dropped: u32 = 0;
                        while !shutdown.load(std::sync::atomic::Ordering::Relaxed) {
                            let mut meta = limesuite::StreamMeta::default();
                            match rx.recv(&mut buf, &mut meta, 10) {
                                Ok(n) if n > 0 => {
                                    reads.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                    // Publish RX timestamp as the true hardware clock.
                                    let end_ts = meta.timestamp + n as u64;
                                    clock.store(end_ts, std::sync::atomic::Ordering::Relaxed);
                                    // Periodically check RX stream status for drops.
                                    let r = reads.load(std::sync::atomic::Ordering::Relaxed);
                                    if r % 100 == 0 {
                                        if let Ok(st) = rx.status() {
                                            let new_drops =
                                                st.dropped_packets.saturating_sub(last_rx_dropped);
                                            if new_drops > 0 {
                                                rx_drops.fetch_add(
                                                    new_drops as u64,
                                                    std::sync::atomic::Ordering::Relaxed,
                                                );
                                                last_rx_dropped = st.dropped_packets;
                                            }
                                            if st.overrun > 0 {
                                                overflows.fetch_add(
                                                    1,
                                                    std::sync::atomic::Ordering::Relaxed,
                                                );
                                            }
                                        }
                                    }
                                }
                                Err(_) => {
                                    if let Ok(st) = rx.status() {
                                        if st.overrun > 0 {
                                            overflows
                                                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                        }
                                    }
                                }
                                _ => {}
                            }
                        }
                    })
                    .ok(),
            )
        } else {
            None
        };

        let start = Instant::now();
        let duration = Duration::from_secs(cli.duration);

        // Brief pause to let RX thread start receiving and publish a timestamp.
        std::thread::sleep(Duration::from_millis(100));

        // Anchor TX timestamps off the RX hardware clock, just like the BTS does.
        // The RX clock is free-running and represents true wall time in sample counts.
        let hw_time = rx_hw_clock.load(std::sync::atomic::Ordering::Relaxed);
        if hw_time == 0 {
            return Err(
                "RX thread did not publish a hardware timestamp — is the device streaming?".into(),
            );
        }
        let lead_samples = (sample_rate_hz as u64) * 80 / 1000; // 80ms lead, same as BTS
        let mut tx_timestamp = hw_time + lead_samples;
        info!(
            "Timing: hw_time={} first_tx_timestamp={} lead_samples={} ({}ms)",
            hw_time,
            tx_timestamp,
            lead_samples,
            lead_samples * 1000 / sample_rate_hz as u64,
        );

        let gate_start = Instant::now();
        while start.elapsed() < duration {
            // Don't write until margin_ms before playout time.
            // Uses the RX hardware clock (free-running, real-time).
            loop {
                let now = rx_hw_clock.load(std::sync::atomic::Ordering::Relaxed);
                // Wait until: now >= tx_timestamp - margin_samples
                if now + margin_samples >= tx_timestamp {
                    break;
                }
                std::thread::sleep(Duration::from_micros(100));
            }

            if sends == 0 {
                let waited = gate_start.elapsed();
                let hw_now = rx_hw_clock.load(std::sync::atomic::Ordering::Relaxed);
                info!(
                    "First send: waited {:.1}ms, hw_clock={}, tx_timestamp={}, delta={}",
                    waited.as_secs_f64() * 1000.0,
                    hw_now,
                    tx_timestamp,
                    tx_timestamp.saturating_sub(hw_now),
                );
            }

            // Simulate signal generation delay (BTS pipeline work).
            if cli.gen_delay_us > 0 {
                let deadline = Instant::now() + Duration::from_micros(cli.gen_delay_us);
                while Instant::now() < deadline {
                    std::hint::spin_loop();
                }
            }

            // TX send with precise timestamp.
            let meta = limesuite::StreamMeta {
                timestamp: tx_timestamp,
                wait_for_timestamp: true,
                flush_partial_packet: false,
            };
            let tone = sig_gen.generate(samples_per_batch);
            tx_stream
                .send(&tone, &meta, 1000)
                .map_err(|e| format!("Lime: TX send: {}", e))?;
            tx_timestamp += samples_per_batch as u64;
            sends += 1;

            // Poll stream status frequently to test if status() causes drops.
            // BTS calls this every 800 sends; we call every 100 sends to stress it.
            if sends % 100 == 0 {
                if let Ok(st) = tx_stream.status() {
                    fifo_sum += st.fifo_filled as f64;
                    fifo_polls += 1;
                    if st.fifo_filled > fifo_max {
                        fifo_max = st.fifo_filled;
                    }
                    debug!(
                        "TX status: underrun={} dropped={} fifo={}/{} sends={}",
                        st.underrun, st.dropped_packets, st.fifo_filled, st.fifo_size, sends
                    );
                }
            }
        }

        let elapsed = start.elapsed().as_secs_f64();

        // End-of-test timing diagnostic (RX clock = true wall clock).
        let end_hw_time = rx_hw_clock.load(std::sync::atomic::Ordering::Relaxed);
        let hw_elapsed_samples = end_hw_time.saturating_sub(hw_time);
        let expected_sends = hw_elapsed_samples / samples_per_batch as u64;
        let signal_coverage_pct =
            (sends as f64 * samples_per_batch as f64) / hw_elapsed_samples as f64 * 100.0;
        info!(
            "Timing check: hw_elapsed={} samples ({:.2}s), sends={}, expected_sends={}, coverage={:.1}%",
            hw_elapsed_samples,
            hw_elapsed_samples as f64 / sample_rate_hz as f64,
            sends,
            expected_sends,
            signal_coverage_pct,
        );

        // Shutdown RX thread.
        rx_shutdown.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(Some(handle)) = rx_thread {
            let _ = handle.join();
        }
        rx_reads = rx_read_counter.load(std::sync::atomic::Ordering::Relaxed);
        rx_overflows = rx_overflow_counter.load(std::sync::atomic::Ordering::Relaxed);
        let rx_dropped = rx_dropped_counter.load(std::sync::atomic::Ordering::Relaxed);

        // Final status.
        let final_status = tx_stream
            .status()
            .map_err(|e| format!("Lime: TX final status: {}", e))?;
        let total_underrun = final_status.underrun.saturating_sub(init_underrun);
        let total_dropped = final_status.dropped_packets.saturating_sub(init_dropped);

        info!(
            "Stream drops: tx_dropped={} tx_underrun={} | rx_dropped={} rx_overflows={}",
            total_dropped, total_underrun, rx_dropped, rx_overflows,
        );

        let total_overrun = 0u32;

        // Stop streams.
        let _ = tx_stream.stop();

        drop(tx_stream);
        drop(device);

        let fifo_avg = if fifo_polls > 0 {
            fifo_sum / fifo_polls as f64
        } else {
            0.0
        };

        Ok(BenchResult {
            batch_pcgs,
            samples_per_batch,
            throughput_vs_latency,
            fifo_size,
            total_dropped,
            total_underrun,
            total_overrun,
            fifo_avg,
            fifo_max,
            sends,
            rx_reads,
            rx_overflows,
            elapsed_secs: elapsed,
        })
    }
}

// ---------------------------------------------------------------------------
// UHD backend
// ---------------------------------------------------------------------------

#[cfg(feature = "uhd-backend")]
mod uhd_bench {
    use super::*;

    pub fn run_uhd_bench(
        cli: &Cli,
        device_str: &str,
        channel: usize,
        tx_antenna: &str,
        rx_antenna: &str,
        tx_gain_db: f64,
        rx_gain_db: f64,
        master_clock_rate: u64,
        batch_pcg_list: &[usize],
        margin_list: &[u32],
    ) -> Result<Vec<BenchResult>, Box<dyn std::error::Error>> {
        let sample_rate_hz = DEFAULT_SAMPLE_RATE_HZ;
        let mut results = Vec::new();

        println!("=== SDR Bench: UHD ===");
        println!(
            "Sample rate: {} Hz ({}x oversample)",
            sample_rate_hz,
            sample_rate_hz / SR1_CHIP_RATE_HZ as usize
        );
        println!("TX frequency: {:.2} MHz", cli.tx_freq as f64 / 1_000_000.0);
        println!("Master clock rate: {} Hz", master_clock_rate);
        println!(
            "Full duplex: {}",
            if cli.full_duplex { "yes" } else { "no" }
        );
        println!("Duration per test: {} s", cli.duration);
        println!();

        for &margin_ms in margin_list {
            for &batch_pcgs in batch_pcg_list {
                let samples_per_batch = batch_pcgs * SAMPLES_PER_PCG;

                info!(
                    "Testing: batch_pcgs={} samples={} margin={}ms",
                    batch_pcgs, samples_per_batch, margin_ms
                );

                match run_single_uhd_test(
                    cli,
                    device_str,
                    channel,
                    tx_antenna,
                    rx_antenna,
                    tx_gain_db,
                    rx_gain_db,
                    master_clock_rate,
                    sample_rate_hz,
                    batch_pcgs,
                    samples_per_batch,
                    margin_ms,
                ) {
                    Ok(result) => {
                        println!(
                            "  => pcgs={:<2} margin={:<3}ms | sends={:<6} coverage={:.1}% | UHD lates to stderr",
                            batch_pcgs,
                            margin_ms,
                            result.sends,
                            (result.sends as f64 * samples_per_batch as f64)
                                / (result.elapsed_secs * sample_rate_hz as f64)
                                * 100.0,
                        );
                        results.push(result);
                    }
                    Err(e) => {
                        println!(
                            "  => pcgs={:<2} margin={:<3}ms | ERROR: {}",
                            batch_pcgs, margin_ms, e
                        );
                    }
                }

                std::thread::sleep(Duration::from_millis(500));
            }
        }

        Ok(results)
    }

    /// Convert tick count to UHD TimeSpec.
    fn ticks_to_timespec(ticks: u64, tick_rate: u64) -> (i64, f64) {
        let full_secs = (ticks / tick_rate) as i64;
        let frac_ticks = ticks % tick_rate;
        let frac_secs = frac_ticks as f64 / tick_rate as f64;
        (full_secs, frac_secs)
    }

    /// Convert UHD TimeSpec to tick count.
    fn timespec_to_ticks(seconds: i64, fraction: f64, tick_rate: u64) -> u64 {
        let ticks_full = seconds as u64 * tick_rate;
        let ticks_frac = (fraction * tick_rate as f64).round() as u64;
        ticks_full + ticks_frac
    }

    /// Capture UHD's stderr L/U/D characters by redirecting fd 2 to a pipe.
    /// Returns (late_count, underrun_count, old_stderr_fd) on drop.
    struct StderrCapture {
        late_count: Arc<std::sync::atomic::AtomicU64>,
        underrun_count: Arc<std::sync::atomic::AtomicU64>,
        shutdown: Arc<std::sync::atomic::AtomicBool>,
        _reader_thread: Option<std::thread::JoinHandle<()>>,
        old_stderr_fd: i32,
    }

    impl StderrCapture {
        fn start() -> Result<Self, Box<dyn std::error::Error>> {
            use std::os::unix::io::FromRawFd;

            let mut pipe_fds = [0i32; 2];
            if unsafe { libc::pipe(pipe_fds.as_mut_ptr()) } != 0 {
                return Err("Failed to create pipe".into());
            }
            let read_fd = pipe_fds[0];
            let write_fd = pipe_fds[1];

            // Save old stderr and redirect stderr to our pipe.
            let old_stderr_fd = unsafe { libc::dup(2) };
            if old_stderr_fd < 0 {
                return Err("Failed to dup stderr".into());
            }
            unsafe { libc::dup2(write_fd, 2) };
            unsafe { libc::close(write_fd) };

            let late_count = Arc::new(std::sync::atomic::AtomicU64::new(0));
            let underrun_count = Arc::new(std::sync::atomic::AtomicU64::new(0));
            let shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));

            let lc = late_count.clone();
            let uc = underrun_count.clone();
            let sd = shutdown.clone();
            let old_fd = old_stderr_fd;

            let reader = std::thread::Builder::new()
                .name("uhd-stderr-capture".into())
                .spawn(move || {
                    let mut file = unsafe { std::fs::File::from_raw_fd(read_fd) };
                    use std::io::Read;
                    let mut buf = [0u8; 4096];
                    loop {
                        match file.read(&mut buf) {
                            Ok(0) => break,
                            Ok(n) => {
                                for &b in &buf[..n] {
                                    match b {
                                        b'L' => {
                                            lc.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                        }
                                        b'U' => {
                                            uc.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                        }
                                        _ => {}
                                    }
                                }
                                // Forward to original stderr so user still sees output.
                                unsafe {
                                    libc::write(old_fd, buf.as_ptr() as *const libc::c_void, n);
                                }
                            }
                            Err(_) => break,
                        }
                        if sd.load(std::sync::atomic::Ordering::Relaxed) {
                            break;
                        }
                    }
                })
                .ok();

            Ok(StderrCapture {
                late_count,
                underrun_count,
                shutdown,
                _reader_thread: reader,
                old_stderr_fd,
            })
        }

        fn stop(self) -> (u64, u64) {
            // Restore original stderr — this closes the write end of the pipe,
            // which will cause the reader thread to see EOF.
            unsafe { libc::dup2(self.old_stderr_fd, 2) };
            unsafe { libc::close(self.old_stderr_fd) };
            self.shutdown
                .store(true, std::sync::atomic::Ordering::Relaxed);
            // Give the reader thread a moment to drain.
            std::thread::sleep(Duration::from_millis(50));
            let lates = self.late_count.load(std::sync::atomic::Ordering::Relaxed);
            let underruns = self
                .underrun_count
                .load(std::sync::atomic::Ordering::Relaxed);
            (lates, underruns)
        }
    }

    fn run_single_uhd_test(
        cli: &Cli,
        device_str: &str,
        channel: usize,
        tx_antenna: &str,
        rx_antenna: &str,
        tx_gain_db: f64,
        rx_gain_db: f64,
        master_clock_rate: u64,
        sample_rate_hz: usize,
        batch_pcgs: usize,
        samples_per_batch: usize,
        margin_ms: u32,
    ) -> Result<BenchResult, Box<dyn std::error::Error>> {
        // Start capturing UHD's L/U/D characters from stderr.
        let stderr_capture = StderrCapture::start()?;

        let mut usrp = uhd::Usrp::open(device_str).map_err(|e| format!("UHD: open: {}", e))?;

        usrp.set_master_clock_rate(master_clock_rate as f64, 0)
            .map_err(|e| format!("UHD: set MCR: {}", e))?;

        // TX setup.
        usrp.set_tx_antenna(tx_antenna, channel)
            .map_err(|e| format!("UHD: set TX antenna: {}", e))?;
        usrp.set_tx_frequency(
            &uhd::TuneRequest::with_frequency(cli.tx_freq as f64),
            channel,
        )
        .map_err(|e| format!("UHD: set TX freq: {}", e))?;
        usrp.set_tx_sample_rate(sample_rate_hz as f64, channel)
            .map_err(|e| format!("UHD: set TX rate: {}", e))?;
        usrp.set_tx_gain(tx_gain_db, channel, "")
            .map_err(|e| format!("UHD: set TX gain: {}", e))?;

        // RX setup (if full-duplex).
        if cli.full_duplex {
            usrp.set_rx_antenna(rx_antenna, channel)
                .map_err(|e| format!("UHD: set RX antenna: {}", e))?;
            usrp.set_rx_frequency(
                &uhd::TuneRequest::with_frequency(cli.tx_freq as f64),
                channel,
            )
            .map_err(|e| format!("UHD: set RX freq: {}", e))?;
            usrp.set_rx_sample_rate(sample_rate_hz as f64, channel)
                .map_err(|e| format!("UHD: set RX rate: {}", e))?;
            usrp.set_rx_gain(rx_gain_db, channel, "")
                .map_err(|e| format!("UHD: set RX gain: {}", e))?;
        }

        let mut tx_streamer = usrp
            .get_tx_stream(&uhd::StreamArgs::<Complex32>::new("sc16"))
            .map_err(|e| format!("UHD: TX stream: {}", e))?;

        let mut rx_streamer = if cli.full_duplex {
            let rx = usrp
                .get_rx_stream(&uhd::StreamArgs::<Complex32>::new("sc16"))
                .map_err(|e| format!("UHD: RX stream: {}", e))?;
            Some(rx)
        } else {
            None
        };

        // Activate RX.
        if let Some(ref mut rx) = rx_streamer {
            rx.send_command(&uhd::StreamCommand {
                time: uhd::StreamTime::Now,
                command_type: uhd::StreamCommandType::StartContinuous,
            })
            .map_err(|e| format!("UHD: RX activate: {}", e))?;
        }

        let mut sig_gen = PilotSignalGenerator::new();

        let mut sends: u64 = 0;
        let rx_reads: u64;
        let rx_overflows: u64;

        // Spawn RX thread for true full-duplex (concurrent USB access).
        let rx_shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let rx_read_counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let rx_overflow_counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
        // Shared RX timestamp as ticks at master_clock_rate.
        let rx_hw_clock = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let rx_thread = if let Some(mut rx) = rx_streamer.take() {
            let shutdown = rx_shutdown.clone();
            let reads = rx_read_counter.clone();
            let overflows = rx_overflow_counter.clone();
            let clock = rx_hw_clock.clone();
            let spr_batch = samples_per_batch;
            let mcr = master_clock_rate;
            Some(
                std::thread::Builder::new()
                    .name("sdr-bench-rx".into())
                    .spawn(move || {
                        let mut buf = vec![Complex32::new(0.0, 0.0); spr_batch];
                        while !shutdown.load(std::sync::atomic::Ordering::Relaxed) {
                            match rx.receive(&mut [&mut buf], 0.01, false) {
                                Ok(md) => {
                                    if md.samples() > 0 {
                                        reads.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                        if let Some(ts) = md.time_spec().ok().flatten() {
                                            let ticks =
                                                timespec_to_ticks(ts.seconds, ts.fraction, mcr);
                                            // Advance by samples received.
                                            let end_ticks = ticks
                                                + (md.samples() as u64 * mcr
                                                    / sample_rate_hz as u64);
                                            clock.store(
                                                end_ticks,
                                                std::sync::atomic::Ordering::Relaxed,
                                            );
                                        }
                                    }
                                    if let Some(err) = md.last_error().ok().flatten() {
                                        if matches!(err.kind(), uhd::ReceiveErrorKind::Overflow) {
                                            overflows
                                                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                        }
                                    }
                                }
                                Err(_) => {}
                            }
                        }
                    })
                    .ok(),
            )
        } else {
            None
        };

        let start = Instant::now();
        let duration = Duration::from_secs(cli.duration);

        // Brief pause to let RX thread start receiving and publish a timestamp.
        std::thread::sleep(Duration::from_millis(100));

        // Anchor TX timestamps off the RX hardware clock.
        let hw_time = rx_hw_clock.load(std::sync::atomic::Ordering::Relaxed);
        if hw_time == 0 {
            // Fallback: read directly from USRP.
            let ts = usrp
                .get_current_time(0)
                .map_err(|e| format!("UHD: get_current_time: {}", e))?;
            let _ = timespec_to_ticks(ts.seconds, ts.fraction, master_clock_rate);
            return Err("RX thread did not publish a hardware timestamp".into());
        }
        // Work in master_clock_rate ticks throughout.
        let lead_ticks = master_clock_rate as u64 * 80 / 1000;
        let margin_ticks = master_clock_rate as u64 * margin_ms as u64 / 1000;
        let samples_per_batch_ticks =
            samples_per_batch as u64 * master_clock_rate as u64 / sample_rate_hz as u64;
        let mut tx_tick = hw_time + lead_ticks;

        info!(
            "Timing: hw_time={} first_tx_tick={} lead_ticks={} (80ms) margin_ticks={} ({}ms)",
            hw_time, tx_tick, lead_ticks, margin_ticks, margin_ms,
        );

        let gate_start = Instant::now();
        let mut start_of_burst = true;

        let mut max_gate_us: u64 = 0;
        let mut max_gen_delay_us: u64 = 0;
        let mut max_shape_us: u64 = 0;
        let mut max_send_us: u64 = 0;
        let mut max_total_us: u64 = 0;

        while start.elapsed() < duration {
            let iter_start = Instant::now();

            // Pacing gate: wait until margin_ms before playout time.
            let gate_t0 = Instant::now();
            loop {
                let now = rx_hw_clock.load(std::sync::atomic::Ordering::Relaxed);
                if now + margin_ticks >= tx_tick {
                    break;
                }
                std::thread::sleep(Duration::from_micros(100));
            }
            let gate_us = gate_t0.elapsed().as_micros() as u64;

            if sends == 0 {
                let waited = gate_start.elapsed();
                let hw_now = rx_hw_clock.load(std::sync::atomic::Ordering::Relaxed);
                info!(
                    "First send: waited {:.1}ms, hw_clock={}, tx_tick={}, delta={}",
                    waited.as_secs_f64() * 1000.0,
                    hw_now,
                    tx_tick,
                    tx_tick.saturating_sub(hw_now),
                );
            }

            // Simulate signal generation delay (BTS pipeline work).
            let gen_t0 = Instant::now();
            if cli.gen_delay_us > 0 {
                let deadline = Instant::now() + Duration::from_micros(cli.gen_delay_us);
                while Instant::now() < deadline {
                    std::hint::spin_loop();
                }
            }
            let gen_us = gen_t0.elapsed().as_micros() as u64;

            // Generate shaped samples (like BTS pipeline).
            let shape_t0 = Instant::now();
            let tone = sig_gen.generate(samples_per_batch);
            let shape_us = shape_t0.elapsed().as_micros() as u64;

            // TX send with precise timestamp.
            let send_t0 = Instant::now();
            let (secs, frac) = ticks_to_timespec(tx_tick, master_clock_rate);
            let metadata = uhd::TransmitMetadata::with_time(secs, frac, start_of_burst, false)
                .map_err(|e| format!("UHD: TX metadata: {}", e))?;
            tx_streamer
                .send_with_metadata(&mut [&tone], &metadata, 1.0)
                .map_err(|e| format!("UHD: TX send: {}", e))?;
            let send_us = send_t0.elapsed().as_micros() as u64;

            start_of_burst = false;
            tx_tick += samples_per_batch_ticks;
            sends += 1;

            let total_us = iter_start.elapsed().as_micros() as u64;
            max_gate_us = max_gate_us.max(gate_us);
            max_gen_delay_us = max_gen_delay_us.max(gen_us);
            max_shape_us = max_shape_us.max(shape_us);
            max_send_us = max_send_us.max(send_us);
            max_total_us = max_total_us.max(total_us);

            // Log first 10 and every 1000th iteration.
            if sends <= 10 || sends % 1000 == 0 {
                info!(
                    "send #{}: gate={}us gen={}us shape={}us send={}us total={}us",
                    sends, gate_us, gen_us, shape_us, send_us, total_us
                );
            }
        }

        info!(
            "Max timings: gate={}us gen={}us shape={}us send={}us total={}us",
            max_gate_us, max_gen_delay_us, max_shape_us, max_send_us, max_total_us
        );

        let elapsed = start.elapsed().as_secs_f64();

        // End-of-test timing diagnostic.
        let end_hw_time = rx_hw_clock.load(std::sync::atomic::Ordering::Relaxed);
        let hw_elapsed_ticks = end_hw_time.saturating_sub(hw_time);
        let hw_elapsed_samples =
            hw_elapsed_ticks * sample_rate_hz as u64 / master_clock_rate as u64;
        let expected_sends = hw_elapsed_samples / samples_per_batch as u64;
        let signal_coverage_pct =
            (sends as f64 * samples_per_batch as f64) / hw_elapsed_samples as f64 * 100.0;
        info!(
            "Timing check: hw_elapsed={} samples ({:.2}s), sends={}, expected_sends={}, coverage={:.1}%",
            hw_elapsed_samples,
            hw_elapsed_samples as f64 / sample_rate_hz as f64,
            sends,
            expected_sends,
            signal_coverage_pct,
        );

        // Shutdown RX thread.
        rx_shutdown.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(Some(handle)) = rx_thread {
            let _ = handle.join();
        }
        rx_reads = rx_read_counter.load(std::sync::atomic::Ordering::Relaxed);
        rx_overflows = rx_overflow_counter.load(std::sync::atomic::Ordering::Relaxed);

        // End TX burst.
        if let Ok(eob) = uhd::TransmitMetadata::with_time(0, 0.0, false, true) {
            let empty: &[Complex32] = &[];
            let _ = tx_streamer.send_with_metadata(&mut [empty], &eob, 0.1);
        }

        // Stop stderr capture and get counts.
        let (uhd_lates, uhd_underruns) = stderr_capture.stop();
        info!(
            "UHD stderr: lates={} underruns={}",
            uhd_lates, uhd_underruns
        );

        Ok(BenchResult {
            batch_pcgs,
            samples_per_batch,
            throughput_vs_latency: 0.0,
            fifo_size: 0,
            total_dropped: uhd_lates as u32,
            total_underrun: uhd_underruns as u32,
            total_overrun: 0,
            fifo_avg: 0.0,
            fifo_max: 0,
            sends,
            rx_reads,
            rx_overflows,
            elapsed_secs: elapsed,
        })
    }
}

// ---------------------------------------------------------------------------
// BladeRF backend
// ---------------------------------------------------------------------------

#[cfg(feature = "bladerf-backend")]
mod bladerf_bench {
    use super::*;

    use bladerf::Device;
    use bladerf::device::{rx_channel, tx_channel};
    use bladerf::stream::{Sc16Q11, StreamMeta};

    /// Local estimate of the bladeRF hardware sample counter, calibrated once
    /// against `Instant::now()` and extrapolated from there.  Avoids USB
    /// round-trips on every pacing-gate poll; a periodic resync corrects any
    /// host-clock drift (typically < 50 ppm).
    struct PacingClock {
        ref_hw: u64,
        ref_wall: Instant,
        sample_rate_hz: u64,
        last_sync: Instant,
    }

    impl PacingClock {
        fn new(hw: u64, sample_rate_hz: u64) -> Self {
            let now = Instant::now();
            PacingClock {
                ref_hw: hw,
                ref_wall: now,
                sample_rate_hz,
                last_sync: now,
            }
        }

        /// Estimated current hardware sample count (no USB call).
        fn now(&self) -> u64 {
            let ns = self.ref_wall.elapsed().as_nanos() as u64;
            self.ref_hw
                .saturating_add(ns * self.sample_rate_hz / 1_000_000_000)
        }

        /// Update the calibration point from a fresh get_timestamp reading.
        fn resync(&mut self, hw: u64) {
            self.ref_hw = hw;
            self.ref_wall = Instant::now();
            self.last_sync = self.ref_wall;
        }

        /// True if a USB resync is due.
        fn needs_resync(&self, interval_ms: u64) -> bool {
            self.last_sync.elapsed() >= Duration::from_millis(interval_ms)
        }
    }

    /// BLADERF_FORMAT_SC16_Q11_META
    const FORMAT_SC16_Q11_META: u32 = 2;
    /// BLADERF_META_FLAG_TX_BURST_START
    const META_FLAG_TX_BURST_START: u32 = 1;
    /// BLADERF_META_FLAG_TX_BURST_END
    const META_FLAG_TX_BURST_END: u32 = 2;
    /// BLADERF_META_FLAG_TX_NOW
    const META_FLAG_TX_NOW: u32 = 4;
    /// BLADERF_META_FLAG_TX_UPDATE_TIMESTAMP
    const META_FLAG_TX_UPDATE_TIMESTAMP: u32 = 8;
    /// BLADERF_META_FLAG_RX_NOW
    const META_FLAG_RX_NOW: u32 = 0x8000_0000;
    /// BLADERF_META_STATUS_UNDERRUN
    const META_STATUS_UNDERRUN: u32 = 2;
    /// BLADERF_META_STATUS_OVERRUN
    const META_STATUS_OVERRUN: u32 = 1;

    pub fn run_bladerf_bench(
        cli: &Cli,
        device_str: &str,
        channel: u32,
        tx_antenna: &str,
        rx_antenna: &str,
        tx_gain_db: i32,
        rx_gain_db: i32,
        fpga_path: Option<&str>,
        num_buffers: u32,
        buffer_size: u32,
        num_transfers: u32,
        stream_timeout_ms: u32,
        batch_pcg_list: &[usize],
        margin_list: &[u32],
    ) -> Result<Vec<BenchResult>, Box<dyn std::error::Error>> {
        let sample_rate_hz = DEFAULT_SAMPLE_RATE_HZ;
        let mut results = Vec::new();

        println!("=== SDR Bench: bladeRF ===");
        println!(
            "Sample rate: {} Hz ({}x oversample)",
            sample_rate_hz,
            sample_rate_hz / SR1_CHIP_RATE_HZ as usize
        );
        println!("TX frequency: {:.2} MHz", cli.tx_freq as f64 / 1_000_000.0);
        println!(
            "Full duplex: {}",
            if cli.full_duplex { "yes" } else { "no" }
        );
        println!("Duration per test: {} s", cli.duration);
        println!();

        for &margin_ms in margin_list {
            for &batch_pcgs in batch_pcg_list {
                let samples_per_batch = batch_pcgs * SAMPLES_PER_PCG;

                info!(
                    "Testing: batch_pcgs={} samples={} margin={}ms",
                    batch_pcgs, samples_per_batch, margin_ms
                );

                match run_single_bladerf_test(
                    cli,
                    device_str,
                    channel,
                    tx_antenna,
                    rx_antenna,
                    tx_gain_db,
                    rx_gain_db,
                    fpga_path,
                    num_buffers,
                    buffer_size,
                    num_transfers,
                    stream_timeout_ms,
                    sample_rate_hz,
                    batch_pcgs,
                    samples_per_batch,
                    margin_ms,
                ) {
                    Ok(result) => {
                        println!(
                            "  => pcgs={:<2} margin={:<3}ms | dropped={:<6} underrun={:<4} sends={:<6} | {}",
                            batch_pcgs,
                            margin_ms,
                            result.total_dropped,
                            result.total_underrun,
                            result.sends,
                            result.verdict(),
                        );
                        results.push(result);
                    }
                    Err(e) => {
                        println!(
                            "  => pcgs={:<2} margin={:<3}ms | ERROR: {}",
                            batch_pcgs, margin_ms, e
                        );
                    }
                }

                std::thread::sleep(Duration::from_millis(500));
            }
        }

        Ok(results)
    }

    #[allow(clippy::too_many_arguments)]
    fn run_single_bladerf_test(
        cli: &Cli,
        device_str: &str,
        channel: u32,
        tx_antenna: &str,
        rx_antenna: &str,
        tx_gain_db: i32,
        rx_gain_db: i32,
        fpga_path: Option<&str>,
        num_buffers: u32,
        buffer_size: u32,
        num_transfers: u32,
        stream_timeout_ms: u32,
        sample_rate_hz: usize,
        batch_pcgs: usize,
        samples_per_batch: usize,
        margin_ms: u32,
    ) -> Result<BenchResult, Box<dyn std::error::Error>> {
        let id = if device_str.is_empty() {
            None
        } else {
            Some(device_str)
        };
        let device = Device::open(id).map_err(|e| format!("bladeRF: open: {}", e))?;

        let board = device.board_name();
        info!("bladeRF: opened board={}", board);
        if let Ok(serial) = device.serial() {
            info!("bladeRF: serial={}", serial);
        }

        // Load FPGA if needed.
        let fpga_loaded = device
            .is_fpga_configured()
            .map_err(|e| format!("bladeRF: check FPGA: {}", e))?;
        if !fpga_loaded {
            match fpga_path {
                Some(path) => {
                    info!("bladeRF: loading FPGA from {}", path);
                    device
                        .load_fpga(path)
                        .map_err(|e| format!("bladeRF: load FPGA: {}", e))?;
                }
                None => {
                    return Err("bladeRF: FPGA not configured and no fpga_path specified".into());
                }
            }
        }

        let tx_ch = tx_channel(channel);
        let rx_ch = rx_channel(channel);

        // TX setup.
        if !tx_antenna.is_empty() {
            device
                .set_rf_port(tx_ch, tx_antenna)
                .map_err(|e| format!("bladeRF: set TX RF port: {}", e))?;
        }
        let actual_rate = device
            .set_sample_rate(tx_ch, sample_rate_hz as u32)
            .map_err(|e| format!("bladeRF: set TX sample rate: {}", e))?;
        info!("bladeRF: TX sample rate actual={}", actual_rate);
        device
            .set_frequency(tx_ch, cli.tx_freq)
            .map_err(|e| format!("bladeRF: set TX freq: {}", e))?;
        device
            .set_gain(tx_ch, tx_gain_db)
            .map_err(|e| format!("bladeRF: set TX gain: {}", e))?;

        // RX setup (for full-duplex and timing anchor).
        if !rx_antenna.is_empty() {
            device
                .set_rf_port(rx_ch, rx_antenna)
                .map_err(|e| format!("bladeRF: set RX RF port: {}", e))?;
        }
        device
            .set_sample_rate(rx_ch, sample_rate_hz as u32)
            .map_err(|e| format!("bladeRF: set RX sample rate: {}", e))?;
        device
            .set_frequency(rx_ch, cli.tx_freq)
            .map_err(|e| format!("bladeRF: set RX freq: {}", e))?;
        if cli.full_duplex {
            device
                .set_gain_mode(rx_ch, 1)
                .map_err(|e| format!("bladeRF: set RX gain mode: {}", e))?;
            device
                .set_gain(rx_ch, rx_gain_db)
                .map_err(|e| format!("bladeRF: set RX gain: {}", e))?;
        }

        // Wrap in Arc for stream ownership.
        let device = std::sync::Arc::new(device);

        // Configure streams: RX first, then TX (same order as bladerf_radio.rs).
        device
            .sync_config(
                0u32, // BLADERF_RX_X1
                FORMAT_SC16_Q11_META,
                num_buffers,
                buffer_size,
                num_transfers,
                stream_timeout_ms,
            )
            .map_err(|e| format!("bladeRF: sync_config RX: {}", e))?;
        device
            .sync_config(
                1u32, // BLADERF_TX_X1
                FORMAT_SC16_Q11_META,
                num_buffers,
                buffer_size,
                num_transfers,
                stream_timeout_ms,
            )
            .map_err(|e| format!("bladeRF: sync_config TX: {}", e))?;

        // Enable modules.
        device
            .enable_module(rx_ch, true)
            .map_err(|e| format!("bladeRF: enable RX: {}", e))?;
        device
            .enable_module(tx_ch, true)
            .map_err(|e| format!("bladeRF: enable TX: {}", e))?;

        let tx_sync = bladerf::TxSync::new(&device);
        let rx_sync = bladerf::RxSync::new(&device);

        // Timing.
        let margin_samples = (sample_rate_hz as u64) * (margin_ms as u64) / 1000;
        let lead_samples = (sample_rate_hz as u64) * 80 / 1000; // 80 ms lead

        // Spawn RX thread — reads samples and tracks overflow/read counts.
        // Not used for timing: the pacing gate uses device.get_timestamp()
        // which is a live FPGA register read and never goes stale.
        let rx_shutdown = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let rx_read_counter = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let rx_overflow_counter = Arc::new(std::sync::atomic::AtomicU64::new(0));

        let rx_thread = if cli.full_duplex {
            let shutdown = rx_shutdown.clone();
            let reads = rx_read_counter.clone();
            let overflows = rx_overflow_counter.clone();
            let spr_batch = samples_per_batch;
            std::thread::Builder::new()
                .name("sdr-bench-rx".into())
                .spawn(move || {
                    let mut buf = vec![Sc16Q11::default(); spr_batch];
                    let mut meta = StreamMeta {
                        flags: META_FLAG_RX_NOW,
                        ..Default::default()
                    };
                    while !shutdown.load(std::sync::atomic::Ordering::Relaxed) {
                        meta.flags = META_FLAG_RX_NOW;
                        match rx_sync.recv(&mut buf, &mut meta, stream_timeout_ms) {
                            Ok(n) if n > 0 => {
                                reads.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                if meta.status & META_STATUS_OVERRUN != 0 {
                                    overflows.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
                                }
                            }
                            _ => {}
                        }
                    }
                })
                .ok()
        } else {
            None
        };

        // Anchor tx_timestamp off the live hardware clock so it's always
        // exactly 80 ms ahead of "now", regardless of batch size.
        let hw_time = device
            .get_timestamp(0)
            .map_err(|e| format!("bladeRF: get_timestamp: {}", e))?;
        let mut tx_timestamp = hw_time + lead_samples;
        info!(
            "Timing: hw_time={} first_tx_timestamp={} lead_samples={}",
            hw_time, tx_timestamp, lead_samples,
        );

        // Pacing clock: extrapolates hardware time from Instant::now().
        // USB get_timestamp calls are limited to one every 100 ms instead of
        // every pacing-gate poll, eliminating USB congestion for small batches.
        let mut pacing_clock = PacingClock::new(hw_time, sample_rate_hz as u64);

        let start = Instant::now();
        let duration = Duration::from_secs(cli.duration);

        let mut sends: u64 = 0;
        let mut total_underrun: u32 = 0;
        let mut total_dropped: u32 = 0;

        let mut sig_gen = PilotSignalGenerator::new();

        // Whether a bladeRF burst is currently open. The BTS uses a single
        // long continuous burst for the entire session; we do the same so
        // bladerf_sync_tx returns as soon as data is in the FPGA FIFO rather
        // than blocking until the burst finishes transmitting.
        let mut burst_active = false;

        while start.elapsed() < duration {
            // Resync the pacing clock against USB once every 100 ms.
            if pacing_clock.needs_resync(100) {
                if let Ok(hw) = device.get_timestamp(0) {
                    pacing_clock.resync(hw);
                }
            }

            // Pacing gate: sleep until margin_samples before tx_timestamp,
            // using the local monotonic estimate (no USB calls in the loop).
            let gate_open_ts = tx_timestamp.saturating_sub(margin_samples);
            let now_est = pacing_clock.now();
            if gate_open_ts > now_est {
                let samples_to_wait = gate_open_ts - now_est;
                let wait_ns = samples_to_wait * 1_000_000_000 / sample_rate_hz as u64;
                if wait_ns > 500_000 {
                    std::thread::sleep(Duration::from_nanos(wait_ns - 500_000));
                }
                // Spin-wait for the final ~500 µs for accurate gate-open timing.
                while pacing_clock.now() + margin_samples < tx_timestamp {
                    std::hint::spin_loop();
                }
            }

            if cli.gen_delay_us > 0 {
                let deadline = Instant::now() + Duration::from_micros(cli.gen_delay_us);
                while Instant::now() < deadline {
                    std::hint::spin_loop();
                }
            }

            let tone = sig_gen.generate(samples_per_batch);
            let sc16: Vec<Sc16Q11> = tone.iter().map(|s| Sc16Q11::from_complex32(*s)).collect();

            // Open a new burst on the first send and after any restart.
            // Subsequent sends carry no burst flags so the FPGA streams them
            // back-to-back without per-batch blocking.
            let (flags, ts) = if !burst_active {
                (
                    META_FLAG_TX_BURST_START | META_FLAG_TX_UPDATE_TIMESTAMP,
                    tx_timestamp,
                )
            } else {
                (0, 0)
            };
            let mut meta = StreamMeta {
                timestamp: ts,
                flags,
                ..Default::default()
            };

            match tx_sync.send(&sc16, Some(&mut meta), stream_timeout_ms) {
                Ok(()) => {
                    burst_active = true;
                }
                Err(e) if e.to_string().contains("in the past") => {
                    // Burst start was late — the timestamp has already passed.
                    // Count as a drop and let the cascade guard re-anchor.
                    warn!("bladeRF: TX late @{}: {}", tx_timestamp, e);
                    total_dropped += 1;
                    burst_active = false;
                }
                Err(e) => {
                    rx_shutdown.store(true, std::sync::atomic::Ordering::Relaxed);
                    if let Some(h) = rx_thread {
                        let _ = h.join();
                    }
                    return Err(format!("bladeRF: TX send: {}", e).into());
                }
            }

            if meta.status & META_STATUS_UNDERRUN != 0 {
                // FPGA ran out of buffered data — the host fell behind.
                // End the burst so the next iteration restarts cleanly.
                total_underrun += 1;
                if burst_active {
                    let zero = [Sc16Q11 { i: 0, q: 0 }; 1];
                    let mut end_meta = StreamMeta {
                        flags: META_FLAG_TX_BURST_END | META_FLAG_TX_NOW,
                        ..Default::default()
                    };
                    let _ = tx_sync.send(&zero, Some(&mut end_meta), stream_timeout_ms);
                    burst_active = false;
                }
            }

            tx_timestamp += samples_per_batch as u64;
            sends += 1;

            // Cascade guard: use the local estimate (no extra USB call).
            let now_est = pacing_clock.now();
            while tx_timestamp + margin_samples <= now_est {
                total_dropped += 1;
                tx_timestamp += samples_per_batch as u64;
            }
        }

        // Close the burst cleanly.
        if burst_active {
            let zero = [Sc16Q11 { i: 0, q: 0 }; 1];
            let mut end_meta = StreamMeta {
                flags: META_FLAG_TX_BURST_END | META_FLAG_TX_NOW,
                ..Default::default()
            };
            let _ = tx_sync.send(&zero, Some(&mut end_meta), stream_timeout_ms);
        }

        let elapsed = start.elapsed().as_secs_f64();

        // Stop RX thread.
        rx_shutdown.store(true, std::sync::atomic::Ordering::Relaxed);
        if let Some(h) = rx_thread {
            let _ = h.join();
        }

        let rx_reads = rx_read_counter.load(std::sync::atomic::Ordering::Relaxed);
        let rx_overflows = rx_overflow_counter.load(std::sync::atomic::Ordering::Relaxed);

        // Disable modules.
        let _ = device.enable_module(tx_ch, false);
        let _ = device.enable_module(rx_ch, false);

        // Timing sanity — one final USB sync for an accurate elapsed count.
        if let Ok(hw) = device.get_timestamp(0) {
            pacing_clock.resync(hw);
        }
        let end_hw = pacing_clock.now();
        let hw_elapsed = end_hw.saturating_sub(hw_time);
        let expected_sends = hw_elapsed / samples_per_batch as u64;
        let coverage = (sends as f64 * samples_per_batch as f64) / hw_elapsed as f64 * 100.0;
        info!(
            "Timing check: hw_elapsed={} samples ({:.2}s), sends={}, expected={}, coverage={:.1}%",
            hw_elapsed,
            hw_elapsed as f64 / sample_rate_hz as f64,
            sends,
            expected_sends,
            coverage,
        );

        Ok(BenchResult {
            batch_pcgs,
            samples_per_batch,
            throughput_vs_latency: 0.0,
            fifo_size: 0,
            total_dropped,
            total_underrun,
            total_overrun: 0,
            fifo_avg: 0.0,
            fifo_max: 0,
            sends,
            rx_reads,
            rx_overflows,
            elapsed_secs: elapsed,
        })
    }
}

// ---------------------------------------------------------------------------
// Summary table printer
// ---------------------------------------------------------------------------

fn format_fifo_size(size: u32) -> String {
    if size >= 1_048_576 && size % 1_048_576 == 0 {
        format!("{}M", size / 1_048_576)
    } else if size >= 1024 && size % 1024 == 0 {
        format!("{}K", size / 1024)
    } else if size == 0 {
        "n/a".to_string()
    } else {
        format!("{}", size)
    }
}

fn print_summary_table(results: &[BenchResult]) {
    if results.is_empty() {
        println!("\nNo results to display.");
        return;
    }

    println!();
    println!(
        "{:<11}| {:<8}| {:<11}| {:<10}| {:<8}| {:<9}| {:<9}| {:<9}| {:<6}| {}",
        "batch_pcgs",
        "samples",
        "throughput",
        "fifo_size",
        "dropped",
        "underrun",
        "fifo_avg",
        "fifo_max",
        "sends",
        "verdict"
    );
    println!(
        "{:-<11}|{:-<9}|{:-<12}|{:-<11}|{:-<9}|{:-<10}|{:-<10}|{:-<10}|{:-<7}|{:-<8}",
        "", "", "", "", "", "", "", "", "", ""
    );

    for r in results {
        println!(
            "{:<11}| {:<8}| {:<11.2}| {:<10}| {:<8}| {:<9}| {:<9.0}| {:<9}| {:<6}| {}",
            r.batch_pcgs,
            r.samples_per_batch,
            r.throughput_vs_latency,
            format_fifo_size(r.fifo_size),
            r.total_dropped,
            r.total_underrun,
            r.fifo_avg,
            r.fifo_max,
            r.sends,
            r.verdict()
        );
    }

    // Full-duplex summary if applicable.
    let has_rx = results.iter().any(|r| r.rx_reads > 0 || r.rx_overflows > 0);
    if has_rx {
        println!();
        println!("RX summary (full-duplex):");
        for r in results {
            if r.rx_reads > 0 || r.rx_overflows > 0 {
                println!(
                    "  batch_pcgs={}: rx_reads={} rx_overflows={}",
                    r.batch_pcgs, r.rx_reads, r.rx_overflows
                );
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Main
// ---------------------------------------------------------------------------

fn main() -> Result<(), Box<dyn std::error::Error>> {
    env_logger::Builder::from_env(env_logger::Env::default().default_filter_or("info")).init();

    let mut cli = Cli::parse();

    // --all enables all sweep modes.
    if cli.all {
        cli.sweep_throughput = true;
        cli.sweep_fifo = true;
        cli.sweep_margin = true;
        cli.full_duplex = true;
    }

    // Spawn CPU stress threads if requested.
    if cli.stress_threads > 0 {
        println!("Spawning {} CPU stress threads", cli.stress_threads);
        for i in 0..cli.stress_threads {
            std::thread::Builder::new()
                .name(format!("stress-{}", i))
                .spawn(|| {
                    loop {
                        std::hint::spin_loop();
                    }
                })
                .expect("failed to spawn stress thread");
        }
    }

    // Parse batch PCG list.
    let batch_pcg_list: Vec<usize> = cli
        .batch_pcgs
        .split(',')
        .map(|s| {
            s.trim()
                .parse::<usize>()
                .expect("invalid PCG count in --batch-pcgs")
        })
        .collect();

    if batch_pcg_list.is_empty() {
        return Err("--batch-pcgs must contain at least one value".into());
    }

    // Build throughput sweep list.
    #[allow(unused_variables)]
    let throughput_list: Vec<f32> = if cli.sweep_throughput {
        vec![0.0, 0.25, 0.5, 0.75, 1.0]
    } else {
        vec![0.0]
    };

    // Build margin sweep list.
    let margin_list: Vec<u32> = if cli.sweep_margin {
        vec![0, 1, 2, 5, 10, 20, 50]
    } else {
        vec![cli.margin_ms]
    };

    // Build FIFO size sweep list.
    #[allow(unused_variables)]
    let fifo_list: Vec<u32> = if cli.sweep_fifo {
        vec![
            256 * 1024,
            512 * 1024,
            1024 * 1024,
            2 * 1024 * 1024,
            4 * 1024 * 1024,
        ]
    } else {
        vec![1024 * 1024]
    };

    // Load radio config.
    let raw = fs::read_to_string(&cli.radio)
        .map_err(|e| format!("failed to read radio config {:?}: {}", cli.radio, e))?;
    let config: RadioConfigFile =
        serde_json::from_str(&raw).map_err(|e| format!("failed to parse radio config: {}", e))?;

    let results: Vec<BenchResult> = match config {
        #[cfg(feature = "lime-backend")]
        RadioConfigFile::Lime {
            device,
            channel,
            tx_antenna,
            tx_gain_db,
            rx_antenna,
            rx_gain_db: cfg_rx_gain,
            oversample,
            ..
        } => {
            let device_str = device.unwrap_or_default();
            let channel = channel.unwrap_or(0);
            let tx_ant = tx_antenna.unwrap_or_else(|| "BAND1".to_string());
            let rx_ant = rx_antenna.unwrap_or_else(|| "LNAW".to_string());
            let gain = cli.tx_gain_db.unwrap_or_else(|| tx_gain_db.unwrap_or(50));
            let rx_gain = cli.rx_gain_db.unwrap_or_else(|| cfg_rx_gain.unwrap_or(30));
            let oversample = oversample.unwrap_or(0);

            lime_bench::run_lime_bench(
                &cli,
                &device_str,
                channel,
                &tx_ant,
                &rx_ant,
                gain,
                rx_gain,
                oversample,
                &batch_pcg_list,
                &throughput_list,
                &fifo_list,
                &margin_list,
            )?
        }
        #[cfg(not(feature = "lime-backend"))]
        RadioConfigFile::Lime { .. } => {
            return Err("LimeSDR backend not compiled in (enable 'lime-backend' feature)".into());
        }
        #[cfg(feature = "uhd-backend")]
        RadioConfigFile::Uhd {
            device,
            channel,
            antenna,
            tx_gain_db,
            rx_antenna,
            rx_gain_db: cfg_rx_gain,
            master_clock_rate,
            ..
        } => {
            let channel = channel.unwrap_or(0);
            let tx_ant = antenna.unwrap_or_else(|| "TX/RX".to_string());
            let rx_ant = rx_antenna.unwrap_or_else(|| "RX2".to_string());
            let gain = cli
                .tx_gain_db
                .map(|g| g as f64)
                .unwrap_or_else(|| tx_gain_db.unwrap_or(50.0));
            let rx_gain = cli
                .rx_gain_db
                .map(|g| g as f64)
                .unwrap_or_else(|| cfg_rx_gain.unwrap_or(30.0));
            let mcr = master_clock_rate.unwrap_or(49_152_000);

            if cli.sweep_throughput {
                warn!("--sweep-throughput has no effect for UHD backend");
            }
            if cli.sweep_fifo {
                warn!("--sweep-fifo has no effect for UHD backend");
            }

            uhd_bench::run_uhd_bench(
                &cli,
                &device,
                channel,
                &tx_ant,
                &rx_ant,
                gain,
                rx_gain,
                mcr,
                &batch_pcg_list,
                &margin_list,
            )?
        }
        #[cfg(not(feature = "uhd-backend"))]
        RadioConfigFile::Uhd { .. } => {
            return Err("UHD backend not compiled in (enable 'uhd-backend' feature)".into());
        }
        RadioConfigFile::Soapy { .. } => {
            return Err(
                "SoapySDR backend is not supported by sdr_bench (use a native Lime, UHD, or bladeRF config)"
                    .into(),
            );
        }
        #[cfg(feature = "bladerf-backend")]
        RadioConfigFile::BladeRf {
            device,
            channel,
            tx_antenna,
            rx_antenna,
            tx_gain_db,
            rx_gain_db: cfg_rx_gain,
            fpga_path,
            num_buffers,
            buffer_size,
            num_transfers,
            stream_timeout_ms,
        } => {
            let device_str = device.unwrap_or_default();
            let channel = channel.unwrap_or(0);
            let tx_ant = tx_antenna.unwrap_or_else(|| "TXA".to_string());
            let rx_ant = rx_antenna.unwrap_or_else(|| "B_BALANCED".to_string());
            let gain = cli
                .tx_gain_db
                .map(|g| g as i32)
                .unwrap_or_else(|| tx_gain_db.unwrap_or(60));
            let rx_gain = cli
                .rx_gain_db
                .map(|g| g as i32)
                .unwrap_or_else(|| cfg_rx_gain.unwrap_or(30));

            if cli.sweep_throughput {
                warn!("--sweep-throughput has no effect for bladeRF backend");
            }
            if cli.sweep_fifo {
                warn!("--sweep-fifo has no effect for bladeRF backend");
            }

            bladerf_bench::run_bladerf_bench(
                &cli,
                &device_str,
                channel,
                &tx_ant,
                &rx_ant,
                gain,
                rx_gain,
                fpga_path.as_deref(),
                num_buffers.unwrap_or(16),
                buffer_size.unwrap_or(8192),
                num_transfers.unwrap_or(8),
                stream_timeout_ms.unwrap_or(3500),
                &batch_pcg_list,
                &margin_list,
            )?
        }
        #[cfg(not(feature = "bladerf-backend"))]
        RadioConfigFile::BladeRf { .. } => {
            return Err(
                "bladeRF backend not compiled in (enable 'bladerf-backend' feature)".into(),
            );
        }
        RadioConfigFile::Other => {
            return Err("unsupported radio config kind".into());
        }
    };

    print_summary_table(&results);

    Ok(())
}
