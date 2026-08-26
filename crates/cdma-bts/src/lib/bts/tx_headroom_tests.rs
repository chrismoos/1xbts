//! Measures the peak-to-average ratio of the pulse-shaped forward composite
//! built from the real channel generators, and checks that the shipped
//! `tx_digital_backoff` keeps it inside the DAC range.

use num::complex::Complex32;

use crate::{
    channels::{
        Channel, WalshChannel,
        fpch::ForwardPagingChannel,
        fsch::ForwardSyncChannel,
        ftch::{Config as FtchConfig, ForwardTrafficChannel, TrafficFrame, TrafficRate},
        pilot::ForwardPilotChannel,
    },
    phy::{
        coding::{
            block_interleaver::{BitReversalInterleaver, SR1_PARAMS_128, SR1_PARAMS_384},
            convolutional::get_1_2_k9_encoder,
            long_code::LongCodeGenerator,
            symbol_repeat::SymbolRepetition,
        },
        spread::{PnSequence, Spreader},
        walsh::WalshGenerator,
    },
    sdr::TxPulseShaper,
};

use super::{
    HRPD_PAPR_HEADROOM,
    evdo::HrpdForwardSlotModulator,
    hrpd::{HarqBus, scheduler::ForwardTrafficPacket},
    settings::BtsRuntimeSettings,
    synth::traffic_amplitudes,
};

use cdma_common::{consts::SR1_CHIPS_PER_FRAME, time::CdmaSystemTime};

const FRAMES: usize = 30;
/// TX sample rates to audit: the shipped 4x and the 8x most radios run at.
const TX_OVERSAMPLES: [usize; 2] = [4, 8];
/// Clip probability the shipped backoff must stay under at the test-model
/// load of six full-rate calls. One clipped sample per 10^5 is below the
/// level where spectral regrowth shows on an adjacent-channel measurement.
const MAX_CLIP_FRACTION_FULL_LOAD: f64 = 1e-5;
/// Looser bound for twice the test-model load, which the composite PAPR
/// grows into as more channels add up.
const MAX_CLIP_FRACTION_OVERLOAD: f64 = 1e-4;
/// The shipped backoff in `config/bts.json`.
const SHIPPED_BACKOFF: f32 = 0.3;

struct Lcg(u64);

impl Lcg {
    fn next(&mut self) -> u32 {
        self.0 = self
            .0
            .wrapping_mul(6_364_136_223_846_793_005)
            .wrapping_add(1_442_695_040_888_963_407);
        (self.0 >> 33) as u32
    }

    fn bits(&mut self, n: usize) -> Vec<u8> {
        (0..n).map(|_| (self.next() & 1) as u8).collect()
    }
}

struct Stats {
    rms: f32,
    peak: f32,
    papr_db: f32,
}

fn stats(samples: &[Complex32]) -> Stats {
    let mut sum = 0.0f64;
    let mut peak = 0.0f32;
    for s in samples {
        sum += (s.re * s.re + s.im * s.im) as f64;
        peak = peak.max(s.re.abs()).max(s.im.abs());
    }
    let rms = (sum / samples.len() as f64).sqrt() as f32;
    Stats {
        rms,
        peak,
        papr_db: 20.0 * (peak / rms).log10(),
    }
}

fn clip_fraction(samples: &[Complex32], scale: f32) -> f64 {
    let clipped = samples
        .iter()
        .filter(|s| (s.re * scale).abs() > 1.0 || (s.im * scale).abs() > 1.0)
        .count();
    clipped as f64 / samples.len() as f64
}

/// Unit-backoff composite (pulse-shaped) for `traffic_channels` active RC1
/// calls carrying random full-rate frames.
fn shaped_composite(
    runtime: &BtsRuntimeSettings,
    traffic_channels: usize,
    seed: u64,
    oversample: usize,
) -> Vec<Complex32> {
    let mut rng = Lcg(seed);
    let total_chips = FRAMES * SR1_CHIPS_PER_FRAME as usize;
    let system_time = CdmaSystemTime::default();

    let pilot = WalshChannel::new(
        WalshGenerator::new::<64>(runtime.downlink.pilot.walsh_code, 1),
        ForwardPilotChannel::new(),
    );
    let sync = WalshChannel::new(
        WalshGenerator::new::<64>(
            runtime.downlink.sync.walsh_code,
            runtime.downlink.sync.walsh_repetition,
        ),
        ForwardSyncChannel::new(crate::channels::fsch::Config {
            data_rate: runtime.downlink.sync.data_rate_bps,
            encoder: get_1_2_k9_encoder(),
            symbol_repeat: SymbolRepetition::new(runtime.downlink.sync.symbol_repeat),
            interleaver: BitReversalInterleaver::new(SR1_PARAMS_128),
            pn_pilot_offset: 0,
        }),
    );
    let paging = WalshChannel::new(
        WalshGenerator::new::<64>(runtime.downlink.paging.walsh_code, 1),
        ForwardPagingChannel::new(crate::channels::fpch::Config {
            data_rate: runtime.downlink.paging.data_rate_bps,
            encoder: get_1_2_k9_encoder(),
            interleaver: BitReversalInterleaver::new(SR1_PARAMS_384),
            long_code_generator: LongCodeGenerator::new_paging_channel(
                runtime.downlink.paging.paging_channel_number,
                0,
            ),
            bypass_long_code: false,
            pn_pilot_offset: 0,
            force_zero_payload_bits: false,
            lc_chip_cursor: 0,
            debug_windows_left: 0,
        }),
    );

    let mut traffic = Vec::new();
    for i in 0..traffic_channels {
        let ch = WalshChannel::new(
            WalshGenerator::new::<64>(8 + i, 1),
            ForwardTrafficChannel::new(FtchConfig {
                encoder: get_1_2_k9_encoder(),
                interleaver: BitReversalInterleaver::new(SR1_PARAMS_384),
                long_code_generator: LongCodeGenerator::new_traffic_channel(rng.next()),
                lc_chip_cursor: 0,
                pcb_scheduler: crate::channels::PcgPcbScheduler::new(0),
                fpc_subchan_gain_linear: 1.0,
                previous_pcg_pc_start: 0,
            }),
        );
        for _ in 0..FRAMES {
            ch.channel.send_frame(TrafficFrame {
                data: rng.bits(TrafficRate::Full.frame_bits()),
                rate: TrafficRate::Full,
            });
        }
        traffic.push(ch);
    }

    let pilot_block = pilot.next_block(total_chips, system_time);
    let sync_block = sync.next_block(total_chips, system_time);
    let paging_block = paging.next_block(total_chips, system_time);
    let traffic_blocks: Vec<(f32, bool, Vec<Complex32>)> = traffic
        .iter()
        .map(|ch| (1.0, true, ch.next_block(total_chips, system_time)))
        .collect();

    let pilot_amp = runtime.downlink.pilot.power_fraction.sqrt();
    let sync_amp = runtime.downlink.sync.power_fraction.sqrt();
    let paging_amp = runtime.downlink.paging.power_fraction.sqrt();
    let mut traffic_amps = Vec::new();
    traffic_amplitudes(
        runtime.downlink.traffic.power_fraction,
        runtime.downlink.traffic.max_channel_power_fraction,
        &traffic_blocks,
        &mut traffic_amps,
    );

    let combined: Vec<Complex32> = (0..total_chips)
        .map(|x| {
            let mut s = pilot_block[x] * pilot_amp
                + sync_block[x] * sync_amp
                + paging_block[x] * paging_amp;
            for ((_, _, block), amp) in traffic_blocks.iter().zip(&traffic_amps) {
                s += block[x] * *amp;
            }
            s
        })
        .collect();

    let mut spreader = Spreader::new(PnSequence::new(0, runtime.short_code_length_chips));
    spreader.align_to_chip(0);
    let chips = spreader.spread_many(&combined);
    shape(&chips, oversample)
}

fn shape(chips: &[Complex32], oversample: usize) -> Vec<Complex32> {
    let mut shaper =
        TxPulseShaper::new(cdma_common::consts::SR1_CHIP_RATE_HZ as usize * oversample).unwrap();
    shaper.shape(chips)
}

#[test]
fn tx_backoff_headroom_audit() {
    for oversample in TX_OVERSAMPLES {
        one_x_headroom_audit(oversample);
    }
}

fn one_x_headroom_audit(oversample: usize) {
    let runtime = BtsRuntimeSettings::default();
    let backoffs = [0.25f32, 0.30, 0.35, 0.40, 0.45, 0.50, 0.55, 0.60];

    println!(
        "forward composite headroom: {} frames, {}x oversampled, power fractions pilot={} sync={} paging={} traffic={} (cap {})",
        FRAMES,
        oversample,
        runtime.downlink.pilot.power_fraction,
        runtime.downlink.sync.power_fraction,
        runtime.downlink.paging.power_fraction,
        runtime.downlink.traffic.power_fraction,
        runtime.downlink.traffic.max_channel_power_fraction,
    );
    println!(
        "{:>6} {:>8} {:>8} {:>8} | {}",
        "calls",
        "rms",
        "peak",
        "papr_dB",
        backoffs
            .iter()
            .map(|b| format!("clip@{b:.2}"))
            .collect::<Vec<_>>()
            .join(" ")
    );

    let mut full_load_clip = 0.0f64;
    let mut overload_clip = 0.0f64;
    for calls in [0usize, 1, 2, 3, 6, 12] {
        let shaped = shaped_composite(&runtime, calls, 0x5EED_0000 + calls as u64, oversample);
        let st = stats(&shaped);
        let clips: Vec<String> = backoffs
            .iter()
            .map(|b| format!("{:>9.1e}", clip_fraction(&shaped, *b)))
            .collect();
        println!(
            "{:>6} {:>8.4} {:>8.4} {:>8.2} | {}",
            calls,
            st.rms,
            st.peak,
            st.papr_db,
            clips.join(" ")
        );
        match calls {
            6 => full_load_clip = clip_fraction(&shaped, SHIPPED_BACKOFF),
            12 => overload_clip = clip_fraction(&shaped, SHIPPED_BACKOFF),
            _ => {}
        }
    }

    assert!(
        full_load_clip <= MAX_CLIP_FRACTION_FULL_LOAD,
        "tx_digital_backoff={SHIPPED_BACKOFF} at {oversample}x clips {full_load_clip:.2e} of samples with six calls (limit {MAX_CLIP_FRACTION_FULL_LOAD:.0e})"
    );
    assert!(
        overload_clip <= MAX_CLIP_FRACTION_OVERLOAD,
        "tx_digital_backoff={SHIPPED_BACKOFF} at {oversample}x clips {overload_clip:.2e} of samples with twelve calls (limit {MAX_CLIP_FRACTION_OVERLOAD:.0e})"
    );
}

const HRPD_SLOTS: usize = 400;
const HRPD_TRAFFIC_MAC: u8 = 5;
/// 2457.6 kbps single-slot 16QAM, the highest-order forward modulation.
const HRPD_DRC_16QAM: u8 = 0xc;
const HRPD_PACKET_PAYLOAD_BITS: usize = 4096;

/// Unit-scale HRPD forward carrier (pulse-shaped): idle pilot/MAC bursts with
/// overhead in the control slots, plus a 16QAM traffic stream when requested.
fn hrpd_shaped(with_traffic: bool, seed: u64, oversample: usize) -> Vec<Complex32> {
    let mut rng = Lcg(seed);
    let mut m = HrpdForwardSlotModulator::new(0, 32_768);
    let slot_chips = crate::phy::hrpd::slot::SLOT_CHIPS;
    let bus = std::sync::Arc::new(HarqBus::new());
    if with_traffic {
        m.set_harq_bus(bus.clone());
    }
    let mut chips = Vec::with_capacity(HRPD_SLOTS * slot_chips as usize);
    for slot in 0..HRPD_SLOTS as u64 {
        if with_traffic {
            // The AT reports its DRC every slot; keep one packet queued so
            // every non-control slot carries traffic.
            bus.set_current_drc_at_slot(HRPD_TRAFFIC_MAC, slot, HRPD_DRC_16QAM);
            m.enqueue_traffic(ForwardTrafficPacket {
                mac_index: HRPD_TRAFFIC_MAC,
                physical_layer_subtype: 0,
                forward_traffic_mac_subtype: 0,
                high_priority: false,
                payload: rng.bits(HRPD_PACKET_PAYLOAD_BITS),
            });
        }
        chips.extend(m.next_block(slot * slot_chips, slot_chips as usize));
    }
    shape(&chips, oversample)
}

/// Frequency shift of each carrier from the composite center when a 1x and
/// an HRPD carrier share one radio on adjacent 1.25 MHz channels.
const COMPOSITE_CARRIER_SHIFT_HZ: f64 = 625_000.0;

fn rotate(samples: &[Complex32], shift_hz: f64, oversample: usize) -> Vec<Complex32> {
    let rate = (cdma_common::consts::SR1_CHIP_RATE_HZ as usize * oversample) as f64;
    let step = 2.0 * std::f64::consts::PI * shift_hz / rate;
    samples
        .iter()
        .enumerate()
        .map(|(n, s)| {
            let (sin, cos) = (step * n as f64).sin_cos();
            s * Complex32::new(cos as f32, sin as f32)
        })
        .collect()
}

#[test]
fn hrpd_backoff_headroom_audit() {
    for oversample in TX_OVERSAMPLES {
        hrpd_headroom_audit(oversample);
    }
}

fn hrpd_headroom_audit(oversample: usize) {
    let backoffs = [0.25f32, 0.30, 0.35, 0.40, 0.45, 0.50, 0.55, 0.60];
    println!(
        "HRPD forward carrier headroom: {} slots, {}x oversampled",
        HRPD_SLOTS, oversample
    );
    println!(
        "{:>8} {:>8} {:>8} {:>8} | {}",
        "traffic",
        "rms",
        "peak",
        "papr_dB",
        backoffs
            .iter()
            .map(|b| format!("clip@{b:.2}"))
            .collect::<Vec<_>>()
            .join(" ")
    );
    let hrpd_only_scale = SHIPPED_BACKOFF * HRPD_PAPR_HEADROOM;
    let mut traffic_clip = 0.0f64;
    for with_traffic in [false, true] {
        let shaped = hrpd_shaped(with_traffic, 0x4E5D_0000 + with_traffic as u64, oversample);
        let st = stats(&shaped);
        let clips: Vec<String> = backoffs
            .iter()
            .map(|b| format!("{:>9.1e}", clip_fraction(&shaped, *b)))
            .collect();
        println!(
            "{:>8} {:>8.4} {:>8.4} {:>8.2} | {}",
            if with_traffic { "16QAM" } else { "idle" },
            st.rms,
            st.peak,
            st.papr_db,
            clips.join(" ")
        );
        if with_traffic {
            traffic_clip = clip_fraction(&shaped, hrpd_only_scale);
        }
    }
    assert!(
        traffic_clip <= MAX_CLIP_FRACTION_FULL_LOAD,
        "HRPD-only scale {hrpd_only_scale} (backoff {SHIPPED_BACKOFF} x {HRPD_PAPR_HEADROOM}) at {oversample}x clips {traffic_clip:.2e} of samples with 16QAM traffic (limit {MAX_CLIP_FRACTION_FULL_LOAD:.0e})"
    );
}

/// Adjacent 1x + HRPD composite: both carriers at full load, rotated to
/// their channel offsets and summed with the composer's
/// `backoff / (1 + gain)` scale at `evdo.gain = 1`.
#[test]
fn composite_backoff_headroom_audit() {
    for oversample in TX_OVERSAMPLES {
        composite_headroom_audit(oversample);
    }
}

fn composite_headroom_audit(oversample: usize) {
    let runtime = BtsRuntimeSettings::default();
    let one_x = rotate(
        &shaped_composite(&runtime, 6, 0x5EED_0006, oversample),
        -COMPOSITE_CARRIER_SHIFT_HZ,
        oversample,
    );
    let hrpd = rotate(
        &hrpd_shaped(true, 0x4E5D_0001, oversample),
        COMPOSITE_CARRIER_SHIFT_HZ,
        oversample,
    );
    let n = one_x.len().min(hrpd.len());
    let evdo_gain = 1.0f32;
    let summed: Vec<Complex32> = (0..n)
        .map(|i| (one_x[i] + hrpd[i] * evdo_gain) / (1.0 + evdo_gain))
        .collect();
    let st = stats(&summed);
    let clip = clip_fraction(&summed, SHIPPED_BACKOFF);
    println!(
        "1x+HRPD composite at {}x: samples={} rms={:.4} peak={:.4} papr={:.2} dB clip@{:.2}={:.1e}",
        oversample, n, st.rms, st.peak, st.papr_db, SHIPPED_BACKOFF, clip
    );
    assert!(
        clip <= MAX_CLIP_FRACTION_FULL_LOAD,
        "composite at tx_digital_backoff={SHIPPED_BACKOFF} at {oversample}x clips {clip:.2e} of samples (limit {MAX_CLIP_FRACTION_FULL_LOAD:.0e})"
    );
}
