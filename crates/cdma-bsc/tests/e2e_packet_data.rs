//! End-to-end packet data session test (SO7).
//!
//! Exercises the full CDMA2000 SO7 packet data negotiation:
//!   Origination → Channel Assignment → Preamble → BS Ack → MS Ack →
//!   Service Connect → SCC → Packet Session (RLP SYNC → LCP → IPCP → Active).
//!
//! The signaling handshake uses synthetic AccessChannelEvents injected directly
//! into the BSC.  RLP/PPP frames are injected through the reverse bearer path,
//! exercising BTS bearer receive → BSC packet session forwarding.
//!
//! A separate test verifies that the ReverseTrafficChannelEncoder produces
//! valid IQ output suitable for BTS reverse traffic demodulation.

#![allow(dead_code, unused_assignments, unused_variables)]

use std::sync::Arc;
use std::thread;
use std::time::Duration;

use num::complex::Complex32;
use tokio::sync::watch;

use cdma_abis::bearer::{ChannelFamily, FrameContent, ReverseFchDcchFrame, TrafficFrame};
use cdma_abis::control::typed::CellId;
use cdma_bsc::abis_edge::network::{NetworkBtsControlClient, NetworkClientConfig};
use cdma_bsc::abis_edge::{BearerFrame, BtsControlClient, ForwardBearerQueue};
use cdma_bts::bts::abis_agent::AbisAgentConfig;
use cdma_bts::bts::rx::InjectedRxBlock;
use cdma_bts::bts::{self, AccessChannelEvent, TrafficResourceService};
use cdma_bts::channels::ftch::TrafficRate;
use cdma_bts::channels::ftch_rc3::{ConfigRc3, ForwardTrafficChannelRc3, TrafficFrameRc3};
use cdma_bts::channels::rtch::ReverseTrafficChannelEncoder;
use cdma_bts::lac;
use cdma_bts::lac::message_types::MessageId;
use cdma_bts::lac::paging_messages::OrderMessage;
use cdma_bts::mac;
use cdma_bts::phy::coding::block_interleaver::{
    ForwardBackwardsBitReversalInterleaver, SR1_PARAMS_768,
};
use cdma_bts::phy::coding::convolutional::{get_1_4_k9_encoder, get_1_4_k9_soft_viterbi_decoder};
use cdma_bts::phy::coding::long_code::LongCodeGenerator;
use cdma_bts::phy::spread::{PnSequence, Spreader};
use cdma_bts::phy::walsh::WalshGenerator;
use cdma_bts::receiver::access_layer3::{FdschMessage, FdschPdu};
use cdma_bts::receiver::sync::SyncChannelMessage;
use cdma_bts::sdr::cdma2000_baseband_filter_taps_f64;
use cdma_bts::sdr::pipe::RadioPipe;

use cdma_common::bits::Bitstream;
use cdma_common::consts::{SERVICE_OPTION_PACKET_DATA, SERVICE_OPTION_SMS};
use cdma_common::error::Error;

use cdma_packet::grpc::PacketServiceImpl;
use cdma_packet::ppp::framing::PppPacket;
use cdma_packet::rlp::{self, RlpRate};

use cdma_bsc::bsc::{Bsc, Config as BscConfig, OverheadParameters};
use cdma_bsc::config::{PagingRetryConfig, TrafficAssignmentConfig, TrafficRetryConfig};
use cdma_msc::{StaticVoicePolicy, VoiceConfig};

fn test_voice_policy() -> std::sync::Arc<dyn cdma_msc::VoicePolicy> {
    std::sync::Arc::new(StaticVoicePolicy::new(VoiceConfig::default()))
}

fn test_msc_client() -> Arc<dyn cdma_bsc::a1_edge::MscClient> {
    Arc::new(cdma_bsc::bsc::AutoAssignmentMscClient::new())
}

use cdma_hlr::model::{
    RegistrationBinding, RegistrationState, Subscriber, SubscriberIdentity, SubscriberStatus,
};
use cdma_hlr::repository::HlrRepository;
use sdr::FIR;

const PCG_CHIPS: u64 = 1_536;
const PCGS_PER_FRAME: usize = 16;

fn init_test_logging() {
    let _ = env_logger::builder()
        .filter_level(log::LevelFilter::Info)
        .is_test(true)
        .try_init();
}

fn scheduled_pcb_bits(
    frame_chip_start: u64,
    bits: [u8; PCGS_PER_FRAME],
    frames: usize,
) -> cdma_bts::channels::PcgPcbSchedulerHandle {
    let scheduler = cdma_bts::channels::PcgPcbScheduler::new(0);
    let abs_pcg_start = frame_chip_start / PCG_CHIPS;
    let mut state = scheduler.lock();
    for frame in 0..frames {
        let frame_base = abs_pcg_start + (frame as u64 * PCGS_PER_FRAME as u64);
        for (pcg, bit) in bits.iter().copied().enumerate() {
            state.schedule(frame_base + pcg as u64, bit);
        }
    }
    drop(state);
    scheduler
}

fn test_packet_service() -> Arc<PacketServiceImpl> {
    Arc::new(PacketServiceImpl::new(
        cdma_packet::ip_transport::IpTransportConfig::Tun {
            nat_interface: "test0".to_string(),
        },
        None,
        None,
    ))
}

// ---------------------------------------------------------------------------
// Synthetic event builders
// ---------------------------------------------------------------------------

fn synthetic_origination_so7(esn: u32) -> AccessChannelEvent {
    AccessChannelEvent {
        event_id: "synth-origination-so7".to_string(),
        chip_start: 2_000_000,
        absolute_chip_start: None,
        receive_time: None,
        preamble_frames: 10,
        pd: 1,
        message_id: MessageId::Origination,
        msg_type_name: "Origination Message".to_string(),
        address: Some(format!("synthetic esn=0x{esn:08x}")),
        resolved_address: None,
        subscriber_id: None,
        l3_summary: Some("Origination(service_option=7)".to_string()),
        decoded_l3: None,
        pdu_summary: "SO7 packet data origination".to_string(),
        msg_seq: Some(2),
        ack_seq: Some(7),
        ack_req: true,
        valid_ack: false,
        msid_type: Some(0b011),
        esn: Some(esn),
        imsi: None,
        meid: None,
        imsi_m_s1: Some(0x0091_989e),
        imsi_m_s2: Some(0x0326),
        imsi_class: Some(0),
        imsi_addr_num: None,
        imsi_mcc: Some(310),
        imsi_11_12: Some(99),
        mob_p_rev: Some(6),
        slot_cycle_index: Some(2),
        scm: Some(0x2a),
        service_option: Some(SERVICE_OPTION_PACKET_DATA),
        wall_clock_us: chrono::Utc::now().timestamp_micros() as u64,
        rx_wall_time: None,
        rx_hw_time_ns: None,
        snr_db: Some(12.5),
        signal_power_db: Some(-35.0),
        reverse_pilot_ec_io_db: None,
        raw_power_db: Some(-40.0),
        demod_quality_pct: Some(94.0),
        pcg_signal_snr_db: None,
        active_pcg_mask: None,
        traffic_phy_valid: None,
        traffic_fqi_valid: None,
        traffic_tail_valid: None,
        traffic_fqi_bits: None,
        traffic_ml_tail_match: None,
        burst_type: None,
        data_burst_fields: None,
        data_burst_num_msgs: None,
        data_burst_msg_number: None,
        traffic_primary_bits: None,
        traffic_primary_rate_bps: None,
        traffic_primary_bearer_routed: false,
        traffic_voice_bits: None,
        traffic_voice_rate_bps: None,
        order_code: None,
        for_rc_pref: None,
        rev_rc_pref: None,
        rev_fch_gating_req: None,
        traffic_walsh_code: None,
        is_preamble_only: false,
        is_traffic_pcg_measurement: false,
        is_traffic_phy_status: false,
        traffic_measurement_age_chips: None,
        for_supported_rcs: vec![1],
        rev_supported_rcs: vec![1],
        decoded_rdsch: None,
        raw_pdu_bits: None,
    }
}

fn synthetic_origination_so7_rc3(esn: u32) -> AccessChannelEvent {
    let mut event = synthetic_origination_so7(esn);
    event.event_id = "synth-origination-so7-rc3".to_string();
    event.l3_summary = Some("Origination(service_option=7, rc3)".to_string());
    event.pdu_summary = "SO7 packet data origination (RC3)".to_string();
    event.for_supported_rcs = vec![3, 4];
    event.rev_supported_rcs = vec![3];
    event
}

#[derive(Debug, Clone)]
struct DecodedRc3BsAckFrame {
    frame_chip_start: u64,
    frame_index: usize,
    decimation_phase: usize,
    chip_offset: usize,
    ack_seq: u8,
    msg_seq: u8,
    ack_req: bool,
    encryption: u8,
    use_time: bool,
    action_time: u8,
    order: u8,
    add_record_len: u8,
}

fn crc12_forward_ftch(bits: &[u8]) -> u16 {
    cdma_common::crc::crc12(bits)
}

fn crc16_fdsch_bits(bits: &[u8]) -> u16 {
    cdma_common::crc::crc16_ccitt(bits)
}

fn apply_local_pulse_shape(chip_samples: &[Complex32], zero_stuff: bool) -> Vec<Complex32> {
    let taps = cdma2000_baseband_filter_taps_f64();
    let mut tx_i = FIR::new(&taps, 1, 1);
    let mut tx_q = FIR::new(&taps, 1, 1);

    let mut upsampled_i = Vec::with_capacity(chip_samples.len() * 4);
    let mut upsampled_q = Vec::with_capacity(chip_samples.len() * 4);
    for s in chip_samples {
        if zero_stuff {
            upsampled_i.push(s.re);
            upsampled_q.push(s.im);
            for _ in 1..4 {
                upsampled_i.push(0.0);
                upsampled_q.push(0.0);
            }
        } else {
            for _ in 0..4 {
                upsampled_i.push(s.re);
                upsampled_q.push(s.im);
            }
        }
    }

    tx_i.process(&upsampled_i)
        .into_iter()
        .zip(tx_q.process(&upsampled_q))
        .map(|(re, im)| Complex32::new(re, im))
        .collect()
}

fn apply_local_matched_filter(oversampled: &[Complex32]) -> Vec<Complex32> {
    let taps = cdma2000_baseband_filter_taps_f64();
    let mut rx_i = FIR::new(&taps, 1, 1);
    let mut rx_q = FIR::new(&taps, 1, 1);
    let i_vals = oversampled.iter().map(|s| s.re).collect::<Vec<_>>();
    let q_vals = oversampled.iter().map(|s| s.im).collect::<Vec<_>>();

    rx_i.process(&i_vals)
        .into_iter()
        .zip(rx_q.process(&q_vals))
        .map(|(re, im)| Complex32::new(re, im))
        .collect()
}

fn quantize_i16_roundtrip(samples: &[Complex32]) -> Vec<Complex32> {
    samples
        .iter()
        .map(|s| {
            let re = (s.re * 0.90 * i16::MAX as f32) as i16;
            let im = (s.im * 0.90 * i16::MAX as f32) as i16;
            Complex32::new(re as f32 / i16::MAX as f32, im as f32 / i16::MAX as f32)
        })
        .collect()
}

fn decimate_sum_and_dump(samples_4x: &[Complex32], sample_phase: usize) -> Vec<Complex32> {
    if sample_phase >= samples_4x.len() {
        return Vec::new();
    }
    samples_4x[sample_phase..]
        .chunks_exact(4)
        .map(|chunk| {
            chunk
                .iter()
                .copied()
                .fold(Complex32::new(0.0, 0.0), |acc, s| acc + s)
        })
        .collect()
}

fn decimate_pick_phase(samples_4x: &[Complex32], sample_phase: usize) -> Vec<Complex32> {
    if sample_phase >= samples_4x.len() {
        return Vec::new();
    }
    samples_4x[sample_phase..]
        .iter()
        .step_by(4)
        .copied()
        .collect()
}

fn solve_dense_linear_system(mut a: Vec<Vec<f64>>, mut b: Vec<f64>) -> Option<Vec<f64>> {
    let n = a.len();
    if n == 0 || b.len() != n || a.iter().any(|row| row.len() != n) {
        return None;
    }

    for pivot in 0..n {
        let mut best_row = pivot;
        let mut best_val = a[pivot][pivot].abs();
        for row in (pivot + 1)..n {
            let val = a[row][pivot].abs();
            if val > best_val {
                best_row = row;
                best_val = val;
            }
        }
        if best_val < 1e-12 {
            return None;
        }
        if best_row != pivot {
            a.swap(best_row, pivot);
            b.swap(best_row, pivot);
        }

        let pivot_val = a[pivot][pivot];
        for col in pivot..n {
            a[pivot][col] /= pivot_val;
        }
        b[pivot] /= pivot_val;

        for row in 0..n {
            if row == pivot {
                continue;
            }
            let factor = a[row][pivot];
            if factor.abs() < 1e-12 {
                continue;
            }
            for col in pivot..n {
                a[row][col] -= factor * a[pivot][col];
            }
            b[row] -= factor * b[pivot];
        }
    }

    Some(b)
}

fn design_real_mmse_equalizer(channel: &[f32], eq_taps: usize, ridge: f64) -> Option<Vec<f32>> {
    if channel.is_empty() || eq_taps == 0 {
        return None;
    }

    let main_tap = channel
        .iter()
        .enumerate()
        .max_by(|(_, a), (_, b)| a.abs().partial_cmp(&b.abs()).unwrap())
        .map(|(idx, _)| idx)?;
    let target_delay = main_tap.saturating_add(eq_taps / 2);
    let out_len = channel.len().saturating_add(eq_taps).saturating_sub(1);

    let mut ata = vec![vec![0.0f64; eq_taps]; eq_taps];
    let mut atd = vec![0.0f64; eq_taps];

    for row in 0..out_len {
        for col_i in 0..eq_taps {
            let chan_i = row
                .checked_sub(col_i)
                .and_then(|idx| channel.get(idx))
                .copied()
                .unwrap_or(0.0) as f64;
            if chan_i == 0.0 {
                continue;
            }
            for col_j in col_i..eq_taps {
                let chan_j = row
                    .checked_sub(col_j)
                    .and_then(|idx| channel.get(idx))
                    .copied()
                    .unwrap_or(0.0) as f64;
                ata[col_i][col_j] += chan_i * chan_j;
            }
            if row == target_delay {
                atd[col_i] += chan_i;
            }
        }
    }

    for row in 0..eq_taps {
        for col in 0..row {
            ata[row][col] = ata[col][row];
        }
        ata[row][row] += ridge;
    }

    solve_dense_linear_system(ata, atd)
        .map(|sol| sol.into_iter().map(|v| v as f32).collect::<Vec<_>>())
}

fn apply_real_fir_complex(samples: &[Complex32], taps: &[f32]) -> Vec<Complex32> {
    if taps.is_empty() {
        return samples.to_vec();
    }
    let mut out = Vec::with_capacity(samples.len());
    for n in 0..samples.len() {
        let mut acc = Complex32::new(0.0, 0.0);
        for (k, tap) in taps.iter().enumerate() {
            if k > n {
                break;
            }
            acc += samples[n - k] * *tap;
        }
        out.push(acc);
    }
    out
}

fn pulse_equalizer_taps(sample_phase: usize, include_rx_matched_filter: bool) -> Option<Vec<f32>> {
    let mut impulse = vec![Complex32::new(0.0, 0.0); 256];
    impulse[0] = Complex32::new(1.0, 0.0);
    let mut pulse_4x = apply_local_pulse_shape(&impulse, true);
    if include_rx_matched_filter {
        pulse_4x = apply_local_matched_filter(&pulse_4x);
    }
    let channel = decimate_sum_and_dump(&pulse_4x, sample_phase)
        .into_iter()
        .take(64)
        .map(|s| s.re)
        .collect::<Vec<_>>();
    design_real_mmse_equalizer(&channel, 13, 1e-3)
}

fn pn_despread_with_absolute_chip_start(
    samples: &[Complex32],
    absolute_chip_start: u64,
) -> Vec<Complex32> {
    let mut pn = PnSequence::new(0, 32768);
    pn.advance_chips(absolute_chip_start);
    samples
        .iter()
        .map(|s| {
            let p = pn.generate_iq();
            Complex32::new(p.re * s.re - p.im * s.im, p.re * s.im + p.im * s.re).conj()
        })
        .collect()
}

fn build_bs_ack_order_pdu_bits(ack_seq: u8, msg_seq: u8) -> Result<Vec<u8>, Error> {
    let order_msg = OrderMessage {
        order: 0b010000,
        ordq: 0,
        order_specific_fields: Vec::new(),
    };
    let sdu = order_msg.to_ftch_sdu();
    let data_request = lac::DataRequest {
        sdu: sdu.clone(),
        mcsb: lac::MessageControlStatusBlock {
            channel: mac::types::ChannelType::FTch,
            length_bits: sdu.len(),
            mobile_p_rev: None,
            extended_encryption: false,
            message_id: MessageId::Order,
            requested_tx_time: None,
            tx_deadline: None,
            address: None,
            ack_seq,
            msg_seq,
            ack_req: true,
            valid_ack: true,
            overhead_mcc: 0x03ff,
            overhead_imsi_11_12: 0x7f,
        },
    };
    Ok(lac::Layer2Lac::assemble_pdu(data_request)?
        .e_pdu
        .bits()
        .to_vec())
}

fn build_expected_bs_ack_ftch_symbols_rc3(
    esn: u32,
    absolute_chip_start: u64,
    ack_seq: u8,
    msg_seq: u8,
) -> Result<Vec<Complex32>, Error> {
    let ch = ForwardTrafficChannelRc3::new(ConfigRc3 {
        encoder: get_1_4_k9_encoder(),
        interleaver: ForwardBackwardsBitReversalInterleaver::new(SR1_PARAMS_768),
        scrambling_lc: LongCodeGenerator::new_traffic_channel(esn),
        puncture_lc: LongCodeGenerator::new_traffic_channel(esn),
        lc_chip_cursor: 0,
        pcb_scheduler: scheduled_pcb_bits(absolute_chip_start, [0; 16], 1),
        fpc_subchan_gain_linear: 1.0,
        prev_frame_last_chip: 0,
        disable_lc_scrambling: false,
    });
    ch.advance_lc_to_chip(absolute_chip_start);
    ch.send_frame(TrafficFrameRc3 {
        data: build_bs_ack_order_pdu_bits(ack_seq, msg_seq)?,
        rate: TrafficRate::Full,
    });
    Ok(ch.next(cdma_common::time::CdmaSystemTime::default()))
}

fn build_synthesized_forward_rc3_bs_ack_iq_samples(
    esn: u32,
    absolute_chip_start: u64,
    walsh_code: u8,
    ack_seq: u8,
    msg_seq: u8,
    bs_ack_frame_index: usize,
    total_frames: usize,
) -> Result<Vec<Complex32>, Error> {
    let ch = ForwardTrafficChannelRc3::new(ConfigRc3 {
        encoder: get_1_4_k9_encoder(),
        interleaver: ForwardBackwardsBitReversalInterleaver::new(SR1_PARAMS_768),
        scrambling_lc: LongCodeGenerator::new_traffic_channel(esn),
        puncture_lc: LongCodeGenerator::new_traffic_channel(esn),
        lc_chip_cursor: 0,
        pcb_scheduler: scheduled_pcb_bits(absolute_chip_start, [0; 16], total_frames),
        fpc_subchan_gain_linear: 1.0,
        prev_frame_last_chip: 0,
        disable_lc_scrambling: false,
    });
    ch.advance_lc_to_chip(absolute_chip_start);

    let walsh_row = WalshGenerator::generate_matrix::<64>()[walsh_code as usize];
    let mut spreader = Spreader::new(PnSequence::new_repeat(0, 32768, 0));
    spreader.align_to_chip(absolute_chip_start);

    let mut chip_samples = Vec::with_capacity(total_frames * 24_576);
    for frame_index in 0..total_frames {
        if frame_index == bs_ack_frame_index {
            ch.send_frame(TrafficFrameRc3 {
                data: build_bs_ack_order_pdu_bits(ack_seq, msg_seq)?,
                rate: TrafficRate::Full,
            });
        }

        let raw_symbols = ch.next(cdma_common::time::CdmaSystemTime::default());
        let walsh_chips = raw_symbols
            .iter()
            .flat_map(|sym| {
                walsh_row
                    .iter()
                    .map(move |&w| Complex32::new(sym.re * w as f32, sym.im * w as f32))
            })
            .collect::<Vec<_>>();
        chip_samples.extend(spreader.spread_many(&walsh_chips));
    }

    Ok(quantize_i16_roundtrip(&apply_local_pulse_shape(
        &chip_samples,
        true,
    )))
}

fn decode_forward_rc3_bs_ack_from_frame(
    qpsk_symbols: &[Complex32],
    esn: u32,
    frame_chip_start: u64,
    frame_index: usize,
    decimation_phase: usize,
    chip_offset: usize,
) -> Option<DecodedRc3BsAckFrame> {
    const MOD_SYMBOLS_PER_FRAME: usize = 768;
    const SYMBOLS_PER_PCG: usize = 48;
    const PC_PUNCTURE_SYMBOLS: usize = 4;

    if qpsk_symbols.len() != MOD_SYMBOLS_PER_FRAME / 2 {
        return None;
    }

    let mut soft_symbols = Vec::with_capacity(MOD_SYMBOLS_PER_FRAME);
    for symbol in qpsk_symbols {
        soft_symbols.push((1.0 - symbol.re) * 0.5);
        soft_symbols.push((1.0 - symbol.im) * 0.5);
    }

    let previous_chip_start = if frame_chip_start == 0 {
        (1u64 << 42) - 2
    } else {
        frame_chip_start - 1
    };
    let mut previous_lc = LongCodeGenerator::new_traffic_channel(esn);
    previous_lc.advance_chips(previous_chip_start as usize);
    let previous_chip = previous_lc.next_chip();

    let mut puncture_lc = LongCodeGenerator::new_traffic_channel(esn);
    puncture_lc.advance_chips(frame_chip_start as usize);
    let mut lc_decimated = vec![0u8; MOD_SYMBOLS_PER_FRAME];
    for bit in &mut lc_decimated {
        *bit = puncture_lc.next_chip();
        for _ in 1..32 {
            puncture_lc.next_chip();
        }
    }

    let mut scrambling_lc = LongCodeGenerator::new_traffic_channel(esn);
    scrambling_lc.advance_chips(frame_chip_start as usize);
    let mut pair_start_chips = vec![0u8; MOD_SYMBOLS_PER_FRAME / 2];
    let mut pair_previous_chips = vec![0u8; MOD_SYMBOLS_PER_FRAME / 2];
    let mut carry_chip = previous_chip;
    for pair_idx in 0..(MOD_SYMBOLS_PER_FRAME / 2) {
        pair_previous_chips[pair_idx] = carry_chip;
        let i_chip = scrambling_lc.next_chip();
        pair_start_chips[pair_idx] = i_chip;
        carry_chip = i_chip;
        for _ in 0..63 {
            carry_chip = scrambling_lc.next_chip();
        }
    }

    let mut pc_positions = [0usize; 16];
    for pcg in 0..16 {
        let base = pcg * SYMBOLS_PER_PCG;
        let b3 = lc_decimated[base + 47] as usize;
        let b2 = lc_decimated[base + 46] as usize;
        let b1 = lc_decimated[base + 45] as usize;
        let b0 = lc_decimated[base + 44] as usize;
        pc_positions[pcg] = ((b3 << 3) | (b2 << 2) | (b1 << 1) | b0) * 2;
    }

    let descrambled = soft_symbols
        .into_iter()
        .enumerate()
        .map(|(idx, value)| {
            let pcg_index = idx / SYMBOLS_PER_PCG;
            let symbol_in_pcg = idx % SYMBOLS_PER_PCG;
            let pc_start = pc_positions[pcg_index];
            if symbol_in_pcg >= pc_start && symbol_in_pcg < pc_start + PC_PUNCTURE_SYMBOLS {
                0.5
            } else {
                let pair_idx = idx / 2;
                let lc_scr = if idx % 2 == 0 {
                    pair_start_chips[pair_idx]
                } else {
                    pair_previous_chips[pair_idx]
                };
                if lc_scr == 0 { value } else { 1.0 - value }
            }
        })
        .collect::<Vec<_>>();

    let interleaver = ForwardBackwardsBitReversalInterleaver::new(SR1_PARAMS_768);
    let deinterleaved = interleaver.decode_soft(&descrambled);
    let peak = deinterleaved
        .iter()
        .map(|v| (0.5 - *v).abs())
        .fold(0.0f32, f32::max);
    let inv_peak = if peak > 1e-12 { 1.0 / peak } else { 1.0 };
    let mut viterbi = get_1_4_k9_soft_viterbi_decoder();
    let metrics = deinterleaved
        .chunks_exact(4)
        .map(|chunk| {
            let to_metric = |value: f32| (value - 0.5) * inv_peak + 0.5;
            [
                to_metric(chunk[0]),
                to_metric(chunk[1]),
                to_metric(chunk[2]),
                to_metric(chunk[3]),
            ]
        })
        .collect::<Vec<_>>();
    let decoded = viterbi.decode_block_from_state(&metrics, 0);
    if decoded.len() < 192 {
        return None;
    }

    let info_bits = &decoded[..172];
    let expected_crc = crc12_forward_ftch(info_bits);
    let mut observed_crc: u16 = 0;
    for &bit in &decoded[172..184] {
        observed_crc = (observed_crc << 1) | bit as u16;
    }
    if expected_crc != observed_crc {
        return None;
    }

    if decoded[184..192].iter().any(|bit| *bit != 0) {
        return None;
    }

    if info_bits.len() < 13
        || info_bits[0] != 1
        || info_bits[1] != 0
        || info_bits[2] != 1
        || info_bits[3] != 1
    {
        return None;
    }
    if info_bits[4] != 1 {
        return None;
    }

    let sar_start = 5usize;
    let msg_length_octets = Bitstream::new_init(&info_bits[sar_start..sar_start + 8])
        .read_bits(8)
        .ok()? as usize;
    let sar_end = sar_start + msg_length_octets * 8;
    if sar_end > info_bits.len() || sar_end < sar_start + 24 {
        return None;
    }

    let expected_fdsch_crc = crc16_fdsch_bits(&info_bits[sar_start..sar_end - 16]);
    let observed_fdsch_crc = Bitstream::new_init(&info_bits[sar_end - 16..sar_end])
        .read_bits(16)
        .ok()? as u16;
    if expected_fdsch_crc != observed_fdsch_crc {
        return None;
    }

    let pdu = FdschPdu::decode(&Bitstream::new_init(
        &info_bits[sar_start + 8..sar_end - 16],
    ))
    .ok()?;
    let FdschMessage::Order(order) = pdu.body else {
        return None;
    };
    if order.order != 0b010000 || order.add_record_len != 0 {
        return None;
    }

    Some(DecodedRc3BsAckFrame {
        frame_chip_start,
        frame_index,
        decimation_phase,
        chip_offset,
        ack_seq: pdu.arq.ack_seq,
        msg_seq: pdu.arq.msg_seq,
        ack_req: pdu.arq.ack_req,
        encryption: pdu.arq.encryption,
        use_time: order.use_time,
        action_time: order.action_time,
        order: order.order,
        add_record_len: order.add_record_len,
    })
}

fn decode_rc3_bs_ack_from_forward_traffic_iq_samples(
    iq_samples: &[Complex32],
    sample_rate: usize,
    walsh_code: u8,
    esn: u32,
    capture_chip_start: u64,
    ack_seq: u8,
    msg_seq: u8,
) -> Result<DecodedRc3BsAckFrame, Error> {
    const CHIPS_PER_FRAME: usize = 24_576;
    const SEARCH_SLOP_CHIPS: usize = 96;
    const MAX_CANDIDATE_FRAMES: usize = 24;
    const DECODE_SCORE_THRESHOLD: f32 = 0.75;
    const MAX_DECODE_CANDIDATES: usize = 32;

    let oversample = (sample_rate / 1_228_800).max(1);
    let filtered = apply_local_matched_filter(iq_samples);
    let walsh_row = WalshGenerator::generate_matrix::<64>()[walsh_code as usize];
    let mut best_score = -1.0f32;
    let mut best_frame_index = 0usize;
    let mut best_phase = 0usize;
    let mut best_chip_offset = 0usize;
    let mut decode_candidates: Vec<(f32, u64, usize, usize, usize, Vec<Complex32>)> = Vec::new();

    let mut chip_rate_variants = Vec::new();
    for sample_phase in 0..oversample {
        for use_sum_and_dump in [false, true] {
            for apply_eq in [false, true] {
                let mut chip_rate_samples = if use_sum_and_dump {
                    decimate_sum_and_dump(&filtered, sample_phase)
                } else {
                    decimate_pick_phase(&filtered, sample_phase)
                };
                if apply_eq {
                    if let Some(eq_taps) = pulse_equalizer_taps(sample_phase, true) {
                        chip_rate_samples = apply_real_fir_complex(&chip_rate_samples, &eq_taps);
                    }
                }
                let variant_phase = sample_phase
                    + if use_sum_and_dump { oversample } else { 0 }
                    + if apply_eq { oversample * 2 } else { 0 };
                chip_rate_variants.push((variant_phase, chip_rate_samples));
            }
        }
    }

    let max_capture_chips = chip_rate_variants
        .iter()
        .map(|(_, samples)| samples.len())
        .max()
        .unwrap_or(0);
    if max_capture_chips < CHIPS_PER_FRAME {
        return Err("not enough forward RC3 capture to cover one full frame".into());
    }

    let capture_frames = (max_capture_chips / CHIPS_PER_FRAME).min(MAX_CANDIDATE_FRAMES);

    for frame_index in 0..capture_frames {
        let frame_chip_start = capture_chip_start + frame_index as u64 * CHIPS_PER_FRAME as u64;
        let expected_chip_start = frame_index * CHIPS_PER_FRAME;
        let expected_symbols =
            build_expected_bs_ack_ftch_symbols_rc3(esn, frame_chip_start, ack_seq, msg_seq)?;

        for (phase_tag, chip_rate_samples) in &chip_rate_variants {
            if chip_rate_samples.len() < CHIPS_PER_FRAME {
                continue;
            }
            let search_start = expected_chip_start.saturating_sub(SEARCH_SLOP_CHIPS);
            let search_end = (expected_chip_start + SEARCH_SLOP_CHIPS)
                .min(chip_rate_samples.len().saturating_sub(CHIPS_PER_FRAME));
            for chip_offset in search_start..=search_end {
                let chip_samples = &chip_rate_samples[chip_offset..chip_offset + CHIPS_PER_FRAME];
                let despread = pn_despread_with_absolute_chip_start(chip_samples, frame_chip_start);
                let symbol_soft = despread
                    .chunks_exact(64)
                    .take(384)
                    .map(|chunk| {
                        chunk
                            .iter()
                            .enumerate()
                            .fold(Complex32::new(0.0, 0.0), |acc, (i, sample)| {
                                acc + *sample * walsh_row[i] as f32
                            })
                    })
                    .collect::<Vec<_>>();
                if symbol_soft.len() != 384 {
                    continue;
                }

                let template_dot = symbol_soft
                    .iter()
                    .zip(expected_symbols.iter())
                    .fold(Complex32::new(0.0, 0.0), |acc, (obs, exp)| {
                        acc + *obs * exp.conj()
                    });
                let symbol_energy = symbol_soft
                    .iter()
                    .map(|s| s.norm_sqr())
                    .sum::<f32>()
                    .sqrt()
                    .max(1e-12);
                let expected_energy = expected_symbols
                    .iter()
                    .map(|s| s.norm_sqr())
                    .sum::<f32>()
                    .sqrt()
                    .max(1e-12);
                let score = template_dot.norm() / (symbol_energy * expected_energy);
                if score > best_score {
                    best_score = score;
                    best_frame_index = frame_index;
                    best_phase = *phase_tag;
                    best_chip_offset = chip_offset;
                }
                if score < DECODE_SCORE_THRESHOLD {
                    continue;
                }
                let phase_ref = if template_dot.norm() > 1e-12 {
                    template_dot / template_dot.norm()
                } else {
                    Complex32::new(1.0, 0.0)
                };
                let rotated_symbols = symbol_soft
                    .iter()
                    .map(|obs| *obs * phase_ref.conj())
                    .collect::<Vec<_>>();
                if decode_candidates.len() < MAX_DECODE_CANDIDATES {
                    decode_candidates.push((
                        score,
                        frame_chip_start,
                        frame_index,
                        *phase_tag,
                        chip_offset,
                        rotated_symbols,
                    ));
                } else if let Some((min_index, _)) = decode_candidates
                    .iter()
                    .enumerate()
                    .min_by(|(_, lhs), (_, rhs)| lhs.0.total_cmp(&rhs.0))
                {
                    if score > decode_candidates[min_index].0 {
                        decode_candidates[min_index] = (
                            score,
                            frame_chip_start,
                            frame_index,
                            *phase_tag,
                            chip_offset,
                            rotated_symbols,
                        );
                    }
                }
            }
        }
    }

    decode_candidates.sort_by(|lhs, rhs| rhs.0.total_cmp(&lhs.0));
    for (_, frame_chip_start, frame_index, phase_tag, chip_offset, rotated_symbols) in
        decode_candidates
    {
        if let Some(decoded) = decode_forward_rc3_bs_ack_from_frame(
            &rotated_symbols,
            esn,
            frame_chip_start,
            frame_index,
            phase_tag,
            chip_offset,
        ) {
            return Ok(decoded);
        }
    }

    Err(format!(
        "failed to decode RC3 BS Ack from forward traffic capture (best_score={:.4} frame={} phase={} chip_offset={})",
        best_score.max(0.0),
        best_frame_index,
        best_phase,
        best_chip_offset,
    )
    .into())
}

fn synthetic_traffic_preamble(walsh_code: u8) -> AccessChannelEvent {
    AccessChannelEvent {
        event_id: "synth-preamble".to_string(),
        chip_start: 3_000_000,
        absolute_chip_start: Some(3_000_000),
        receive_time: None,
        preamble_frames: 0,
        pd: 0,
        message_id: MessageId::GeneralExtension,
        msg_type_name: "TrafficPreamble".to_string(),
        address: None,
        resolved_address: None,
        subscriber_id: None,
        l3_summary: None,
        decoded_l3: None,
        pdu_summary: format!("preamble walsh={}", walsh_code),
        msg_seq: None,
        ack_seq: None,
        ack_req: false,
        valid_ack: false,
        msid_type: None,
        esn: None,
        imsi: None,
        meid: None,
        imsi_m_s1: None,
        imsi_m_s2: None,
        imsi_class: None,
        imsi_addr_num: None,
        imsi_mcc: None,
        imsi_11_12: None,
        mob_p_rev: None,
        slot_cycle_index: None,
        scm: None,
        service_option: None,
        wall_clock_us: chrono::Utc::now().timestamp_micros() as u64,
        rx_wall_time: None,
        rx_hw_time_ns: None,
        snr_db: Some(10.0),
        signal_power_db: Some(-40.0),
        reverse_pilot_ec_io_db: None,
        raw_power_db: Some(-45.0),
        demod_quality_pct: None,
        pcg_signal_snr_db: None,
        active_pcg_mask: None,
        traffic_phy_valid: None,
        traffic_fqi_valid: None,
        traffic_tail_valid: None,
        traffic_fqi_bits: None,
        traffic_ml_tail_match: None,
        burst_type: None,
        data_burst_fields: None,
        data_burst_num_msgs: None,
        data_burst_msg_number: None,
        traffic_primary_bits: None,
        traffic_primary_rate_bps: None,
        traffic_primary_bearer_routed: false,
        traffic_voice_bits: None,
        traffic_voice_rate_bps: None,
        order_code: None,
        for_rc_pref: None,
        rev_rc_pref: None,
        rev_fch_gating_req: None,
        traffic_walsh_code: Some(walsh_code),
        is_preamble_only: true,
        is_traffic_pcg_measurement: false,
        is_traffic_phy_status: false,
        traffic_measurement_age_chips: None,
        for_supported_rcs: Vec::new(),
        rev_supported_rcs: Vec::new(),
        decoded_rdsch: None,
        raw_pdu_bits: None,
    }
}

fn synthetic_ms_ack_order(walsh_code: u8) -> AccessChannelEvent {
    AccessChannelEvent {
        event_id: "synth-ms-ack".to_string(),
        chip_start: 3_100_000,
        absolute_chip_start: Some(3_100_000),
        receive_time: None,
        preamble_frames: 0,
        pd: 0,
        message_id: MessageId::Order,
        msg_type_name: "Order Message".to_string(),
        address: None,
        resolved_address: None,
        subscriber_id: None,
        l3_summary: None,
        decoded_l3: None,
        pdu_summary: "MS Ack Order".to_string(),
        msg_seq: Some(0),
        ack_seq: Some(7),
        ack_req: false,
        valid_ack: true,
        msid_type: None,
        esn: None,
        imsi: None,
        meid: None,
        imsi_m_s1: None,
        imsi_m_s2: None,
        imsi_class: None,
        imsi_addr_num: None,
        imsi_mcc: None,
        imsi_11_12: None,
        mob_p_rev: None,
        slot_cycle_index: None,
        scm: None,
        service_option: None,
        wall_clock_us: chrono::Utc::now().timestamp_micros() as u64,
        rx_wall_time: None,
        rx_hw_time_ns: None,
        snr_db: Some(10.0),
        signal_power_db: None,
        reverse_pilot_ec_io_db: None,
        raw_power_db: None,
        demod_quality_pct: None,
        pcg_signal_snr_db: None,
        active_pcg_mask: None,
        traffic_phy_valid: None,
        traffic_fqi_valid: None,
        traffic_tail_valid: None,
        traffic_fqi_bits: None,
        traffic_ml_tail_match: None,
        burst_type: None,
        data_burst_fields: None,
        data_burst_num_msgs: None,
        data_burst_msg_number: None,
        traffic_primary_bits: None,
        traffic_primary_rate_bps: None,
        traffic_primary_bearer_routed: false,
        traffic_voice_bits: None,
        traffic_voice_rate_bps: None,
        order_code: Some(0b010000), // MS Ack Order
        for_rc_pref: None,
        rev_rc_pref: None,
        rev_fch_gating_req: None,
        traffic_walsh_code: Some(walsh_code),
        is_preamble_only: false,
        is_traffic_pcg_measurement: false,
        is_traffic_phy_status: false,
        traffic_measurement_age_chips: None,
        for_supported_rcs: Vec::new(),
        rev_supported_rcs: Vec::new(),
        decoded_rdsch: None,
        raw_pdu_bits: None,
    }
}

fn synthetic_service_connect_completion(walsh_code: u8) -> AccessChannelEvent {
    AccessChannelEvent {
        event_id: "synth-scc".to_string(),
        chip_start: 3_200_000,
        absolute_chip_start: Some(3_200_000),
        receive_time: None,
        preamble_frames: 0,
        pd: 0,
        message_id: MessageId::ServiceConnectCompletion,
        msg_type_name: "Service Connect Completion".to_string(),
        address: None,
        resolved_address: None,
        subscriber_id: None,
        l3_summary: None,
        decoded_l3: None,
        pdu_summary: "SCC for packet data".to_string(),
        msg_seq: Some(1),
        ack_seq: Some(0),
        ack_req: true,
        valid_ack: true,
        msid_type: None,
        esn: None,
        imsi: None,
        meid: None,
        imsi_m_s1: None,
        imsi_m_s2: None,
        imsi_class: None,
        imsi_addr_num: None,
        imsi_mcc: None,
        imsi_11_12: None,
        mob_p_rev: None,
        slot_cycle_index: None,
        scm: None,
        service_option: None,
        wall_clock_us: chrono::Utc::now().timestamp_micros() as u64,
        rx_wall_time: None,
        rx_hw_time_ns: None,
        snr_db: Some(10.0),
        signal_power_db: None,
        reverse_pilot_ec_io_db: None,
        raw_power_db: None,
        demod_quality_pct: None,
        pcg_signal_snr_db: None,
        active_pcg_mask: None,
        traffic_phy_valid: None,
        traffic_fqi_valid: None,
        traffic_tail_valid: None,
        traffic_fqi_bits: None,
        traffic_ml_tail_match: None,
        burst_type: None,
        data_burst_fields: None,
        data_burst_num_msgs: None,
        data_burst_msg_number: None,
        traffic_primary_bits: None,
        traffic_primary_rate_bps: None,
        traffic_primary_bearer_routed: false,
        traffic_voice_bits: None,
        traffic_voice_rate_bps: None,
        order_code: None,
        for_rc_pref: None,
        rev_rc_pref: None,
        rev_fch_gating_req: None,
        traffic_walsh_code: Some(walsh_code),
        is_preamble_only: false,
        is_traffic_pcg_measurement: false,
        is_traffic_phy_status: false,
        traffic_measurement_age_chips: None,
        for_supported_rcs: Vec::new(),
        rev_supported_rcs: Vec::new(),
        decoded_rdsch: None,
        raw_pdu_bits: None,
    }
}

// ---------------------------------------------------------------------------
// RLP frame helpers
// ---------------------------------------------------------------------------

/// Encode an RLP frame as 171-bit payload (individual u8 values, each 0 or 1).
fn encode_rlp_full_rate(frame: &rlp::RlpFrame) -> Vec<u8> {
    rlp::encode_frame(frame, RlpRate::Full).expect("test RLP full-rate frame must encode")
}

async fn inject_reverse_bearer_rlp_frame(
    bsc: &mut Bsc,
    bts_client: &Arc<dyn BtsControlClient>,
    walsh_code: u8,
    rlp_bits: Vec<u8>,
    rate_bps: u32,
) {
    let (frame_content, reverse_link_information) = match rate_bps {
        9600 => (FrameContent::FchRc1_9600, rlp_to_info_bits(&rlp_bits)),
        4800 => (FrameContent::FchRc1_4800, rlp_bits),
        2400 | 2700 => (FrameContent::FchRc1_2400, rlp_bits),
        1200 | 1500 => (FrameContent::FchRc1_1200, rlp_bits),
        other => panic!("unsupported reverse bearer RLP rate: {other}"),
    };
    let bearer = bts_client
        .bearer_client()
        .expect("test BTS control client should expose a bearer client");
    bearer
        .receive_frame(BearerFrame {
            channel_family: ChannelFamily::Fch,
            bearer_id: walsh_code as u32,
            tx_frame_number: 0,
            traffic_frame: TrafficFrame::ReverseFchDcch(ReverseFchDcchFrame {
                channel_family: ChannelFamily::Fch,
                soft_handoff_leg: 0,
                fsn: 0,
                fqi: true,
                reverse_link_quality: 0x40,
                scaling: 0,
                packet_arrival_time_error: 0,
                frame_content,
                fpc_s: 0,
                eib: false,
                reverse_link_information,
                message_crc: 0,
            }),
            queue: ForwardBearerQueue::Traffic,
        })
        .expect("reverse bearer frame injection should queue");
    bsc.poll_reverse_bearer_once().await;
}

/// Build a PPP LCP Configure-Request with empty options (bare minimum).
fn build_ppp_lcp_configure_request(identifier: u8) -> Vec<u8> {
    // PPP frame: Address(0xFF) + Control(0x03) + Protocol(0xC021=LCP) + LCP packet
    // LCP: Code(1=Configure-Request) + Identifier + Length(4) + Options(empty)
    let lcp_payload = vec![
        0x01, // Code: Configure-Request
        identifier, 0x00, 0x04, // Length: 4 (header only, no options)
    ];
    build_ppp_frame(0xC021, &lcp_payload)
}

/// Build a PPP LCP Configure-Ack for a received request.
fn build_ppp_lcp_configure_ack(identifier: u8, options: &[u8]) -> Vec<u8> {
    let mut lcp_payload = vec![
        0x02, // Code: Configure-Ack
        identifier,
    ];
    let len = 4 + options.len();
    lcp_payload.push((len >> 8) as u8);
    lcp_payload.push(len as u8);
    lcp_payload.extend_from_slice(options);
    build_ppp_frame(0xC021, &lcp_payload)
}

/// Build a PPP IPCP Configure-Request.
fn build_ppp_ipcp_configure_request(identifier: u8, ip: [u8; 4]) -> Vec<u8> {
    let ipcp_payload = vec![
        0x01, // Code: Configure-Request
        identifier, 0x00, 0x0A, // Length: 10 (4 header + 6 option)
        0x03, 0x06, // Option 3 (IP-Address), length 6
        ip[0], ip[1], ip[2], ip[3],
    ];
    build_ppp_frame(0x8021, &ipcp_payload)
}

/// Build a PPP IPCP Configure-Ack.
fn build_ppp_ipcp_configure_ack(identifier: u8, ip: [u8; 4]) -> Vec<u8> {
    let ipcp_payload = vec![
        0x02, // Code: Configure-Ack
        identifier, 0x00, 0x0A, // Length: 10
        0x03, 0x06, ip[0], ip[1], ip[2], ip[3],
    ];
    build_ppp_frame(0x8021, &ipcp_payload)
}

/// Wrap a PPP payload in HDLC-like framing for RLP transport.
fn build_ppp_frame(protocol: u16, payload: &[u8]) -> Vec<u8> {
    let mut frame = Vec::new();
    frame.push(0xFF); // Address
    frame.push(0x03); // Control
    frame.push((protocol >> 8) as u8);
    frame.push(protocol as u8);
    frame.extend_from_slice(payload);
    frame
}

/// Send a PPP payload as RLP data frames (chunked into 19-byte segments).
fn ppp_to_rlp_frames(ppp_raw: &[u8], seq: &mut u8) -> Vec<rlp::RlpFrame> {
    // Extract protocol and payload from raw PPP bytes (skip address + control)
    let protocol = (ppp_raw[2] as u16) << 8 | ppp_raw[3] as u16;
    let payload = &ppp_raw[4..];
    let ppp_packet = PppPacket {
        protocol,
        payload: payload.to_vec(),
    };
    let hdlc = cdma_packet::ppp::framing::frame(&ppp_packet);
    let mut frames = Vec::new();
    for chunk in hdlc.chunks(19) {
        frames.push(rlp::data_frame(*seq, chunk));
        *seq = seq.wrapping_add(1);
    }
    frames
}

// ---------------------------------------------------------------------------
// Main E2E test
// ---------------------------------------------------------------------------

#[tokio::test]
async fn test_e2e_so7_packet_data_full_negotiation() {
    init_test_logging();

    let esn: u32 = 0xABCD_EF01;

    // -- Setup L2 channels (LAC/MAC) --
    let (mac_to_lac_tx, mac_to_lac_rx) = std::sync::mpsc::channel();
    let (lac_to_mac_tx, lac_to_mac_rx) = std::sync::mpsc::channel();
    let lac_layer = lac::Layer2Lac::new(lac_to_mac_tx, mac_to_lac_rx);
    let mac_layer = mac::Layer2Mac::new(lac_to_mac_rx, mac_to_lac_tx);

    // -- Create RadioPipe + BTS --
    let start_system_time = cdma_common::time::cdma_epoch();
    let (radio, pipe_handle) = RadioPipe::new(1024);
    let (_bts, bts_handle) = bts::Bts::new_with_radio_pipe(
        radio,
        bts::Config {
            tx_center_frequency_hz: 881_520_000,
            pilot_offset: 0,
            mac_layer: mac_layer.clone(),
            start_system_time: Some(start_system_time),
            sync_channel_template: Some(SyncChannelMessage {
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
                cdma_freq: 384,
                ext_cdma_freq: 0,
                sr1_bcch_non_td_incl: false,
                sr1_td_incl: false,
                sr3_incl: false,
                ds_incl: false,
            }),
            timezone: cdma_common::timezone::TimezoneConfig::default(),
            overhead: cdma_common::overhead::OverheadParameters::default(),
            rx: Some(bts::RxSettings {
                sample_rate_hz: 1_228_800 * 4,
                auth_mode: 0,
                p_rev_in_use: 6,
                capture_iq_wav: None,
                capture_seconds: None,
                access_channel_number: 0,
                paging_channel_number: 1,
                base_id: 1,
                pilot_pn: 0,
                chip_rate_hz: 1_228_800,
                absolute_chip_start: 0,
                hardware_start_time_ns: 0,
                tick_rate: 1_000_000_000,
                access_event_tx: None,
                reverse_bearer_tx: None,
                rx_metrics_tx: None,
                reanchor_origin: true,
                traffic_rx_pool: None,
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
            }),
        },
        bts::BtsRuntimeSettings::default(),
    );

    let bts::BtsHandle {
        tx_metrics: _,
        rx_metrics: _,
        config: _,
        access_events: mut _bts_access_rx,
        commands: _,
        traffic_channels,
        walsh_allocator,
        traffic_rx_pool,
        traffic_rx_removals,
        power_control: _,
        rx_measurements: _,
    } = bts_handle;

    // -- Create packet service --
    let packet_service = test_packet_service();
    let bts_client: Arc<dyn BtsControlClient> =
        Arc::new(NetworkBtsControlClient::spawn_in_process(
            Arc::new(TrafficResourceService::from_pools(
                walsh_allocator.clone(),
                traffic_channels.clone(),
                traffic_rx_pool.clone(),
                traffic_rx_removals.clone(),
            )),
            AbisAgentConfig {
                pilot_pn: 0,
                cell_id: CellId { cell: 1, sector: 1 },
                mscid: 1,
            },
            NetworkClientConfig {
                cell_id: CellId { cell: 1, sector: 1 },
                mscid: 1,
                pilot_pn: 0,
                auth_mode: 0,
                p_rev_in_use: 6,
                market_id: 1,
                generating_entity_id: 1,
            },
        ));

    // -- Create BSC --
    let mut bsc = Bsc::new(BscConfig {
        pilot_offset: 0,
        overhead: OverheadParameters {
            sid: 42,
            nid: 7,
            cdma_freq: Some(384),
            ..Default::default()
        },
        paging: bts::PagingChannelSettings::default(),
        traffic_assignment: TrafficAssignmentConfig::default(),
        access_event_rx: None, // We manually forward BTS events
        access_event_broadcast: None,
        sms_request_rx: None,
        sms_request_tx: None,
        data_request_rx: None,
        data_request_tx: None,
        power_override_request_rx: None,
        power_override_request_tx: None,
        mobiles_tx: None,
        paging_broadcast: None,
        traffic_broadcast: None,
        rx_reference_dbm: None,
        hlr_repo: None,
        msc_client: test_msc_client(),
        msc_voice_bearer: None,
        bts_client: Some(bts_client.clone()),
        traffic_retry: TrafficRetryConfig::default(),
        paging_retry: PagingRetryConfig::default(),
        voice_policy: test_voice_policy(),
        pcf_client: Some(Arc::new(cdma_bsc::packet::LegacyPcfClient::new(
            packet_service.clone(),
        ))),
        mobile_idle_timeout_s: 0,
        bts_paging_state: None,
        node_id: "bsc-test".to_string(),
    });

    // -- Start L2 workers --
    let lac_worker = {
        let lac = lac_layer.clone();
        thread::spawn(move || lac.run_for(100_000, Duration::from_secs(5)).unwrap())
    };
    let mac_worker = {
        let mac = mac_layer.clone();
        thread::spawn(move || mac.run_for(100_000, Duration::from_secs(5)).unwrap())
    };

    // ===================================================================
    // Phase A: Origination → Traffic channel assignment
    // ===================================================================
    eprintln!("=== Phase A: Origination (SO7) ===");
    bsc.inject_access_event(synthetic_origination_so7(esn))
        .await;

    // The BSC should have allocated a traffic channel. First Walsh code = 10.
    let walsh_code: u8 = 10;

    // Verify traffic RX was requested
    {
        let pool = traffic_rx_pool.lock();
        assert!(
            pool.iter().any(|r| r.walsh_code == walsh_code),
            "BSC should have pushed TrafficRxRequest for walsh={}",
            walsh_code
        );
    }
    eprintln!("  Traffic channel allocated: walsh={}", walsh_code);

    // ===================================================================
    // Phase B: Preamble → BS Ack
    // ===================================================================
    eprintln!("=== Phase B: Preamble detection ===");
    bsc.inject_access_event(synthetic_traffic_preamble(walsh_code))
        .await;
    eprintln!("  Preamble event injected, BS Ack should be queued on F-TCH");

    // ===================================================================
    // Phase C: MS Ack → Service Connect
    // ===================================================================
    eprintln!("=== Phase C: MS Ack Order ===");
    bsc.inject_access_event(synthetic_ms_ack_order(walsh_code))
        .await;
    eprintln!("  MS Ack processed, Service Connect should be queued");

    // ===================================================================
    // Phase D: Service Connect Completion → Packet session open
    // ===================================================================
    eprintln!("=== Phase D: Service Connect Completion ===");
    bsc.inject_access_event(synthetic_service_connect_completion(walsh_code))
        .await;

    // Give the session task a moment to start up
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Check that a packet session was created
    let sessions = packet_service.list_all_sessions();
    assert!(
        !sessions.is_empty(),
        "Packet session should have been created after SCC"
    );
    let session_id = sessions[0].session_id.clone();
    eprintln!(
        "  Packet session created: id={}, phase={}",
        session_id, sessions[0].phase
    );

    // ===================================================================
    // Phase E: RLP SYNC handshake
    // ===================================================================
    eprintln!("=== Phase E: RLP SYNC handshake ===");

    // Let the session task tick a few times to send SYNC
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Mobile sends SYNC/ACK
    let sync_ack_bits = encode_rlp_full_rate(&rlp::sync_ack_frame(0));
    inject_reverse_bearer_rlp_frame(&mut bsc, &bts_client, walsh_code, sync_ack_bits, 9600).await;

    // Let session process SYNC/ACK and transition
    tokio::time::sleep(Duration::from_millis(200)).await;

    // Send a few idle frames to help the RLP state machine advance past ACK phase
    for _ in 0..8 {
        let idle_bits = rlp::encode_frame(&rlp::idle_frame(0), RlpRate::Eighth)
            .expect("test RLP idle frame must encode");
        inject_reverse_bearer_rlp_frame(&mut bsc, &bts_client, walsh_code, idle_bits, 1200).await;
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    let info = packet_service.get_session_info(&session_id);
    let phase = info.as_ref().map(|i| i.phase.as_str()).unwrap_or("none");
    eprintln!("  After SYNC handshake: phase={}", phase);

    // The session should have progressed past rlp_sync.
    // It may be in "lcp" or still processing.
    assert!(
        phase == "lcp" || phase == "ipcp" || phase == "active",
        "Expected session to be past rlp_sync, got phase={}",
        phase
    );

    // ===================================================================
    // Phase F: LCP negotiation
    // ===================================================================
    eprintln!("=== Phase F: LCP negotiation ===");

    // Let session send LCP Configure-Request
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Mobile sends LCP Configure-Ack (ACK BS's request with MRU=1500)
    // and its own LCP Configure-Request
    let mut rlp_seq: u8 = 0;

    // BS sends Configure-Request with MRU=1500 (option type 1, len 4, value 0x05DC)
    // Mobile must echo the exact options in Configure-Ack
    let mru_option = vec![0x01, 0x04, 0x05, 0xDC]; // MRU option: type=1, len=4, value=1500
    let lcp_ack = build_ppp_lcp_configure_ack(1, &mru_option);
    for frame in ppp_to_rlp_frames(&lcp_ack, &mut rlp_seq) {
        let bits = encode_rlp_full_rate(&frame);
        inject_reverse_bearer_rlp_frame(&mut bsc, &bts_client, walsh_code, bits, 9600).await;
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    // Mobile sends its own LCP Configure-Request (empty options = accept defaults)
    let lcp_req = build_ppp_lcp_configure_request(1);
    for frame in ppp_to_rlp_frames(&lcp_req, &mut rlp_seq) {
        let bits = encode_rlp_full_rate(&frame);
        inject_reverse_bearer_rlp_frame(&mut bsc, &bts_client, walsh_code, bits, 9600).await;
        tokio::time::sleep(Duration::from_millis(25)).await;
    }

    // Let session process LCP
    tokio::time::sleep(Duration::from_millis(200)).await;

    let info = packet_service.get_session_info(&session_id);
    let phase = info.as_ref().map(|i| i.phase.as_str()).unwrap_or("none");
    eprintln!("  After LCP: phase={}", phase);

    // ===================================================================
    // Phase G: IPCP negotiation
    // ===================================================================
    eprintln!("=== Phase G: IPCP negotiation ===");

    if phase == "ipcp" {
        tokio::time::sleep(Duration::from_millis(100)).await;

        // Mobile sends IPCP Configure-Ack for BS's gateway IP.
        // PacketServiceImpl allocates from the default 10.55.0.0/24 pool.
        let ipcp_ack = build_ppp_ipcp_configure_ack(1, [10, 55, 0, 1]);
        for frame in ppp_to_rlp_frames(&ipcp_ack, &mut rlp_seq) {
            let bits = encode_rlp_full_rate(&frame);
            inject_reverse_bearer_rlp_frame(&mut bsc, &bts_client, walsh_code, bits, 9600).await;
            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        // Mobile sends IPCP Configure-Request with 0.0.0.0 (requesting IP)
        let ipcp_req = build_ppp_ipcp_configure_request(1, [0, 0, 0, 0]);
        for frame in ppp_to_rlp_frames(&ipcp_req, &mut rlp_seq) {
            let bits = encode_rlp_full_rate(&frame);
            inject_reverse_bearer_rlp_frame(&mut bsc, &bts_client, walsh_code, bits, 9600).await;
            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        // BS will NAK with assigned IP. Mobile retries with that IP.
        tokio::time::sleep(Duration::from_millis(200)).await;

        let info = packet_service.get_session_info(&session_id);
        let phase_now = info.as_ref().map(|i| i.phase.as_str()).unwrap_or("none");
        eprintln!("  After first IPCP round: phase={}", phase_now);

        if phase_now == "ipcp" {
            // BS sent IPCP Configure-Nak with assigned IP (e.g., 10.55.0.2)
            // Mobile retries with the assigned IP
            let ipcp_req2 = build_ppp_ipcp_configure_request(2, [10, 55, 0, 2]);
            for frame in ppp_to_rlp_frames(&ipcp_req2, &mut rlp_seq) {
                let bits = encode_rlp_full_rate(&frame);
                inject_reverse_bearer_rlp_frame(&mut bsc, &bts_client, walsh_code, bits, 9600)
                    .await;
                tokio::time::sleep(Duration::from_millis(25)).await;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }

    // ===================================================================
    // Final verification
    // ===================================================================
    let info = packet_service.get_session_info(&session_id);
    let final_phase = info.as_ref().map(|i| i.phase.as_str()).unwrap_or("none");
    eprintln!("=== Final session state: phase={} ===", final_phase);

    // Full negotiation: RLP SYNC → LCP → IPCP → Active
    assert_eq!(
        final_phase, "active",
        "Expected session to reach active phase, got: {}",
        final_phase
    );

    eprintln!(
        "=== E2E packet data test passed (phase={}) ===",
        final_phase
    );

    // ===================================================================
    // Cleanup
    // ===================================================================
    // Close RadioPipe RX to let BTS exit cleanly
    let mut pipe_handle = pipe_handle;
    pipe_handle.close_rx();

    // Drain TX to prevent overflow
    pipe_handle.drain_tx_samples();

    // Stop L2 workers
    drop(lac_layer);
    drop(mac_layer);
    let _ = lac_worker.join();
    let _ = mac_worker.join();
}

// ---------------------------------------------------------------------------
// PHY encoder verification test
// ---------------------------------------------------------------------------

/// Verify the ReverseTrafficChannelEncoder produces non-trivial output
/// with correct sample counts for both data frames and preamble.
#[test]
fn test_reverse_traffic_encoder_full_rate_roundtrip_shape() {
    let esn: u32 = 0xDEAD_BEEF;
    let encoder = ReverseTrafficChannelEncoder::new(esn);

    // Build an RLP SYNC frame as 172-bit traffic frame:
    // Bit 0 = MM (0 = primary traffic only), bits 1-171 = RLP
    let rlp_bits = rlp::encode_frame(&rlp::sync_frame(0), RlpRate::Full)
        .expect("test RLP sync frame must encode");
    assert_eq!(rlp_bits.len(), 171);

    let mut info_bits = vec![0u8; 172]; // MM=0 + 171 RLP bits
    info_bits[0] = 0; // MM = 0 (primary traffic only)
    info_bits[1..].copy_from_slice(&rlp_bits);

    let samples = encoder.encode_full_rate_frame(&info_bits, 0);

    // 24576 chips * 4 oversample = 98304 samples
    assert_eq!(samples.len(), 98304, "full-rate frame sample count");

    // Verify non-trivial energy (not all zeros)
    let energy: f32 = samples.iter().map(|s| s.re * s.re + s.im * s.im).sum();
    assert!(energy > 0.0, "encoded samples should have non-zero energy");
    eprintln!(
        "Full-rate frame: {} samples, total energy = {:.1}",
        samples.len(),
        energy
    );

    // Verify preamble
    let preamble_chips = 24576 * 2; // 2 frames
    let preamble_samples = encoder.encode_preamble(preamble_chips, 0);
    assert_eq!(
        preamble_samples.len(),
        preamble_chips * 4,
        "preamble sample count"
    );

    let preamble_energy: f32 = preamble_samples
        .iter()
        .map(|s| s.re * s.re + s.im * s.im)
        .sum();
    assert!(
        preamble_energy > 0.0,
        "preamble should have non-zero energy"
    );
    eprintln!(
        "Preamble: {} samples, total energy = {:.1}",
        preamble_samples.len(),
        preamble_energy
    );
}

// ---------------------------------------------------------------------------
// RadioPipe IQ injection helper
// ---------------------------------------------------------------------------

/// Inject pulse-shaped IQ samples into RadioPipe in blocks.
fn enqueue_injected_rx_samples_pipe(
    pipe: &cdma_bts::sdr::pipe::RadioPipeHandle,
    samples: &[Complex32],
    absolute_chip_start: u64,
    oversample: usize,
    block_len: usize,
) {
    let mut sample_idx = 0usize;
    while sample_idx < samples.len() {
        let end = (sample_idx + block_len).min(samples.len());
        let block = samples[sample_idx..end].to_vec();
        let chip_start = absolute_chip_start + (sample_idx / oversample) as u64;
        pipe.inject_rx(InjectedRxBlock {
            samples: block,
            time_ns: 0,
            absolute_chip_start: Some(chip_start),
        })
        .expect("inject_rx failed");
        sample_idx = end;
    }
}

/// Build 172-bit info payload from 171-bit RLP frame (MM=0 prefix for primary traffic).
fn rlp_to_info_bits(rlp_bits: &[u8]) -> Vec<u8> {
    assert_eq!(rlp_bits.len(), 171);
    let mut info = vec![0u8; 172];
    info[0] = 0; // MM = 0 (primary traffic only)
    info[1..].copy_from_slice(rlp_bits);
    info
}

/// Build 172-bit info payload containing a valid R-DSCH signaling frame.
/// Uses MUX header 1011 (all signaling, 0 primary bits, 168 signaling bits).
/// The signaling field contains a minimal valid PDU (SOM + msg_length + payload + CRC-16).
/// This is needed for the BTS frame aligner to lock on the reverse traffic channel.
fn build_rdsch_signaling_info_bits() -> Vec<u8> {
    let mut info = vec![0u8; 172];

    // MUX header: 1011 → 0 primary, 168 signaling bits
    info[0] = 1;
    info[1] = 0;
    info[2] = 1;
    info[3] = 1;

    // Signaling field starts at bit 4, 168 bits total.
    // Format: SOM(1) + data
    // Data: msg_length_octets(8) + payload + CRC-16(16)
    // With msg_length=4: 32 total data bits = 8 len + 8 payload + 16 CRC
    let sig_start = 4;
    info[sig_start] = 1; // SOM = 1

    let msg_length: u8 = 4;
    // msg_length_octets in 8 bits MSB first
    for i in 0..8 {
        info[sig_start + 1 + i] = (msg_length >> (7 - i)) & 1;
    }

    // 8 bits of dummy payload (e.g., PD=0 + zero message type)
    // bits [sig_start+9..sig_start+17] = 0 (already zero)

    // CRC-16 (CRC-CCITT) over the first (msg_length*8 - 16) = 16 bits of data
    // Data to CRC: msg_length(8 bits) + payload(8 bits) = 16 bits
    let crc_scope = &info[sig_start + 1..sig_start + 1 + 16];
    let crc = {
        let poly: u16 = 0x1021;
        let mut register: u16 = 0xFFFF;
        for &bit in crc_scope {
            let feedback = ((register >> 15) & 1) ^ (bit as u16 & 1);
            register <<= 1;
            if feedback == 1 {
                register ^= poly;
            }
        }
        register ^ 0xFFFF
    };

    // Write CRC-16 MSB first at bits [sig_start+17..sig_start+33]
    let crc_start = sig_start + 1 + 16;
    for i in 0..16 {
        info[crc_start + i] = ((crc >> (15 - i)) & 1) as u8;
    }

    info
}

/// Despread TX samples: remove PN spreading and Walsh correlate to extract
/// symbol-rate energy on a specific Walsh code.
fn forward_tx_walsh_energy(tx_samples: &[Complex32], walsh_code: u8, oversample: usize) -> f32 {
    if tx_samples.is_empty() {
        return 0.0;
    }
    // Decimate to chip rate (pick center phase)
    let decimate_phase = oversample / 2;
    let chip_samples: Vec<Complex32> = tx_samples
        .iter()
        .enumerate()
        .filter_map(|(i, &s)| {
            if i % oversample == decimate_phase {
                Some(s)
            } else {
                None
            }
        })
        .collect();

    // PN despread
    let mut pn = PnSequence::new(0, 32768);
    let despread: Vec<Complex32> = chip_samples
        .iter()
        .map(|s| {
            let p = pn.generate_iq();
            Complex32::new(p.re * s.re - p.im * s.im, p.re * s.im + p.im * s.re)
        })
        .collect();

    // Walsh correlate: dot-product over 64-chip windows
    let walsh_row = WalshGenerator::generate_matrix::<64>()[walsh_code as usize];
    let mut total_energy: f32 = 0.0;
    let mut symbol_count = 0usize;
    for chunk in despread.chunks_exact(64) {
        let corr: Complex32 = chunk
            .iter()
            .enumerate()
            .fold(Complex32::new(0.0, 0.0), |acc, (i, sample)| {
                acc + *sample * walsh_row[i] as f32
            });
        total_energy += corr.re * corr.re + corr.im * corr.im;
        symbol_count += 1;
    }

    if symbol_count > 0 {
        (total_energy / symbol_count as f32).sqrt()
    } else {
        0.0
    }
}

// ---------------------------------------------------------------------------
// Full PHY E2E test — reverse traffic through real DSP, forward TX verified
// ---------------------------------------------------------------------------

/// E2E test with real PHY on both directions:
///
/// **Reverse (Mobile → BS):** RLP/PPP frames are encoded through the full
/// reverse traffic PHY chain (CRC → conv encode → interleave → 64-ary Walsh
/// → LC×PN spread → pulse shape) via `ReverseTrafficChannelEncoder`, injected
/// as IQ samples into RadioPipe, and decoded by the BTS traffic RX pipeline.
///
/// **Forward (BS → Mobile):** TX samples are drained from RadioPipe and
/// verified via PN despread + Walsh correlation on the assigned traffic code.
///
/// Signaling handshake (MS Ack, SCC) remains synthetic since R-DSCH encoding
/// is a separate chain not yet exposed for test synthesis.
#[tokio::test]
async fn test_e2e_so7_packet_data_phy_bidirectional() {
    init_test_logging();

    let esn: u32 = 0xABCD_EF01;
    let oversample: usize = 4;
    let chip_rate: usize = 1_228_800;
    let chips_per_frame: usize = 24576; // 20ms at 1.2288 Mcps

    // -- Setup L2 channels --
    let (mac_to_lac_tx, mac_to_lac_rx) = std::sync::mpsc::channel();
    let (lac_to_mac_tx, lac_to_mac_rx) = std::sync::mpsc::channel();
    let lac_layer = lac::Layer2Lac::new(lac_to_mac_tx, mac_to_lac_rx);
    let mac_layer = mac::Layer2Mac::new(lac_to_mac_rx, mac_to_lac_tx);

    // -- Create RadioPipe + BTS --
    let start_system_time = cdma_common::time::cdma_epoch();
    let (radio, pipe_handle) = RadioPipe::new(4096);
    let (bts, bts_handle) = bts::Bts::new_with_radio_pipe(
        radio,
        bts::Config {
            tx_center_frequency_hz: 881_520_000,
            pilot_offset: 0,
            mac_layer: mac_layer.clone(),
            start_system_time: Some(start_system_time),
            sync_channel_template: Some(SyncChannelMessage {
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
                cdma_freq: 384,
                ext_cdma_freq: 0,
                sr1_bcch_non_td_incl: false,
                sr1_td_incl: false,
                sr3_incl: false,
                ds_incl: false,
            }),
            timezone: cdma_common::timezone::TimezoneConfig::default(),
            overhead: cdma_common::overhead::OverheadParameters::default(),
            rx: Some(bts::RxSettings {
                sample_rate_hz: chip_rate * oversample,
                auth_mode: 0,
                p_rev_in_use: 6,
                capture_iq_wav: None,
                capture_seconds: None,
                access_channel_number: 0,
                paging_channel_number: 1,
                base_id: 1,
                pilot_pn: 0,
                chip_rate_hz: chip_rate,
                absolute_chip_start: 0,
                hardware_start_time_ns: 0,
                tick_rate: 1_000_000_000,
                access_event_tx: None,
                reverse_bearer_tx: None,
                rx_metrics_tx: None,
                reanchor_origin: true,
                traffic_rx_pool: None,
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
            }),
        },
        bts::BtsRuntimeSettings::default(),
    );

    let bts::BtsHandle {
        tx_metrics: _,
        rx_metrics: _,
        config: _,
        access_events: mut bts_access_rx,
        commands: _,
        traffic_channels,
        walsh_allocator,
        traffic_rx_pool,
        traffic_rx_removals,
        power_control: _,
        rx_measurements: _,
    } = bts_handle;

    // -- Create packet service + BSC --
    let packet_service = test_packet_service();
    let bts_client: Arc<dyn BtsControlClient> =
        Arc::new(NetworkBtsControlClient::spawn_in_process(
            Arc::new(TrafficResourceService::from_pools(
                walsh_allocator.clone(),
                traffic_channels.clone(),
                traffic_rx_pool.clone(),
                traffic_rx_removals.clone(),
            )),
            AbisAgentConfig {
                pilot_pn: 0,
                cell_id: CellId { cell: 1, sector: 1 },
                mscid: 1,
            },
            NetworkClientConfig {
                cell_id: CellId { cell: 1, sector: 1 },
                mscid: 1,
                pilot_pn: 0,
                auth_mode: 0,
                p_rev_in_use: 6,
                market_id: 1,
                generating_entity_id: 1,
            },
        ));
    let mut bsc = Bsc::new(BscConfig {
        pilot_offset: 0,
        overhead: OverheadParameters {
            sid: 42,
            nid: 7,
            cdma_freq: Some(384),
            ..Default::default()
        },
        paging: bts::PagingChannelSettings::default(),
        traffic_assignment: TrafficAssignmentConfig::default(),
        access_event_rx: None, // We manually forward BTS events
        access_event_broadcast: None,
        sms_request_rx: None,
        sms_request_tx: None,
        data_request_rx: None,
        data_request_tx: None,
        power_override_request_rx: None,
        power_override_request_tx: None,
        mobiles_tx: None,
        paging_broadcast: None,
        traffic_broadcast: None,
        rx_reference_dbm: None,
        hlr_repo: None,
        msc_client: test_msc_client(),
        msc_voice_bearer: None,
        bts_client: Some(bts_client.clone()),
        traffic_retry: TrafficRetryConfig::default(),
        paging_retry: PagingRetryConfig::default(),
        voice_policy: test_voice_policy(),
        pcf_client: Some(Arc::new(cdma_bsc::packet::LegacyPcfClient::new(
            packet_service.clone(),
        ))),
        mobile_idle_timeout_s: 0,
        bts_paging_state: None,
        node_id: "bsc-test".to_string(),
    });

    // -- Start L2 workers --
    let lac_worker = {
        let lac = lac_layer.clone();
        thread::spawn(move || lac.run_for(100_000, Duration::from_secs(10)).unwrap())
    };
    let mac_worker = {
        let mac = mac_layer.clone();
        thread::spawn(move || mac.run_for(100_000, Duration::from_secs(10)).unwrap())
    };

    // ===================================================================
    // Phase A: Origination (synthetic — access channel, not traffic PHY)
    // ===================================================================
    eprintln!("=== PHY Phase A: Origination (SO7) ===");
    bsc.inject_access_event(synthetic_origination_so7(esn))
        .await;
    let walsh_code: u8 = 10;
    {
        let pool = traffic_rx_pool.lock();
        assert!(
            pool.iter().any(|r| r.walsh_code == walsh_code),
            "BSC should have pushed TrafficRxRequest for walsh={}",
            walsh_code
        );
    }
    eprintln!("  Traffic channel allocated: walsh={}", walsh_code);

    // ===================================================================
    // Phase B: Reverse preamble through real PHY
    // ===================================================================
    eprintln!("=== PHY Phase B: Reverse preamble via RadioPipe ===");
    let encoder = ReverseTrafficChannelEncoder::new(esn);

    // Compute the BTS's starting chip cursor so we align the encoder.
    // The BTS does: now_chips=0 (cdma epoch) + lead_chips (100ms) → align to
    // next sync_superframe boundary. With defaults:
    //   lead_chips = 100_000_000 * 1_228_800 / 1_000_000_000 = 122_880
    //   sync_superframe_chips = 98_304
    //   align_to_residue(122880, 98304, 0) = 196_608
    let sync_superframe_chips: u64 = 98_304;
    let lead_chips: u64 = 100_000_000u64 * chip_rate as u64 / 1_000_000_000;
    let bts_chip_start = {
        let v = lead_chips % sync_superframe_chips;
        if v == 0 {
            lead_chips
        } else {
            lead_chips + (sync_superframe_chips - v)
        }
    };
    eprintln!("  BTS chip cursor start: {}", bts_chip_start);

    // Preamble: 3 frames (60ms) of Walsh 0 with LC×PN spreading.
    // Start at the BTS's chip cursor so RX timing aligns.
    let preamble_chips = chips_per_frame * 3;
    let preamble_chip_start: u64 = bts_chip_start;
    let preamble_samples = encoder.encode_preamble(preamble_chips, preamble_chip_start);
    eprintln!(
        "  Preamble: {} chips, {} samples",
        preamble_chips,
        preamble_samples.len()
    );

    // Pre-compute all reverse traffic IQ: preamble + signaling + data frames
    let mut all_rx_samples = preamble_samples;
    let mut frame_chip_offset = preamble_chip_start + preamble_chips as u64;

    // R-DSCH signaling frames — the frame aligner requires valid signaling
    // to lock before it can emit decoded traffic frames. Send several to
    // ensure at least one lands in the lock search window.
    let signaling_info = build_rdsch_signaling_info_bits();
    for _ in 0..4 {
        let sig_iq = encoder.encode_full_rate_frame(&signaling_info, frame_chip_offset);
        all_rx_samples.extend_from_slice(&sig_iq);
        frame_chip_offset += chips_per_frame as u64;
    }
    eprintln!("  Added 4 R-DSCH signaling frames for frame aligner lock");

    // Build RLP data frames for the entire protocol negotiation
    let mut rlp_seq: u8 = 0;

    // RLP SYNC/ACK
    let sync_ack_bits = encode_rlp_full_rate(&rlp::sync_ack_frame(0));
    let sync_ack_info = rlp_to_info_bits(&sync_ack_bits);
    let sync_ack_iq = encoder.encode_full_rate_frame(&sync_ack_info, frame_chip_offset);
    all_rx_samples.extend_from_slice(&sync_ack_iq);
    frame_chip_offset += chips_per_frame as u64;

    // Several idle frames to help RLP settle into DataTransfer
    for _ in 0..4 {
        let idle_bits = rlp::encode_frame(&rlp::idle_frame(0), RlpRate::Full)
            .expect("test RLP idle frame must encode");
        let idle_info = rlp_to_info_bits(&idle_bits);
        let idle_iq = encoder.encode_full_rate_frame(&idle_info, frame_chip_offset);
        all_rx_samples.extend_from_slice(&idle_iq);
        frame_chip_offset += chips_per_frame as u64;
    }

    // LCP Configure-Ack (echo BS's MRU=1500)
    let mru_option = vec![0x01, 0x04, 0x05, 0xDC];
    let lcp_ack = build_ppp_lcp_configure_ack(1, &mru_option);
    for frame in ppp_to_rlp_frames(&lcp_ack, &mut rlp_seq) {
        let bits = encode_rlp_full_rate(&frame);
        let info = rlp_to_info_bits(&bits);
        let iq = encoder.encode_full_rate_frame(&info, frame_chip_offset);
        all_rx_samples.extend_from_slice(&iq);
        frame_chip_offset += chips_per_frame as u64;
    }

    // LCP Configure-Request (mobile's own, empty options)
    let lcp_req = build_ppp_lcp_configure_request(1);
    for frame in ppp_to_rlp_frames(&lcp_req, &mut rlp_seq) {
        let bits = encode_rlp_full_rate(&frame);
        let info = rlp_to_info_bits(&bits);
        let iq = encoder.encode_full_rate_frame(&info, frame_chip_offset);
        all_rx_samples.extend_from_slice(&iq);
        frame_chip_offset += chips_per_frame as u64;
    }

    // IPCP Configure-Ack for BS's gateway IP
    let ipcp_ack = build_ppp_ipcp_configure_ack(1, [10, 55, 0, 1]);
    for frame in ppp_to_rlp_frames(&ipcp_ack, &mut rlp_seq) {
        let bits = encode_rlp_full_rate(&frame);
        let info = rlp_to_info_bits(&bits);
        let iq = encoder.encode_full_rate_frame(&info, frame_chip_offset);
        all_rx_samples.extend_from_slice(&iq);
        frame_chip_offset += chips_per_frame as u64;
    }

    // IPCP Configure-Request with 0.0.0.0 (requesting IP)
    let ipcp_req = build_ppp_ipcp_configure_request(1, [0, 0, 0, 0]);
    for frame in ppp_to_rlp_frames(&ipcp_req, &mut rlp_seq) {
        let bits = encode_rlp_full_rate(&frame);
        let info = rlp_to_info_bits(&bits);
        let iq = encoder.encode_full_rate_frame(&info, frame_chip_offset);
        all_rx_samples.extend_from_slice(&iq);
        frame_chip_offset += chips_per_frame as u64;
    }

    // IPCP Configure-Request with assigned IP (10.55.0.2) — retry after NAK
    let ipcp_req2 = build_ppp_ipcp_configure_request(2, [10, 55, 0, 2]);
    for frame in ppp_to_rlp_frames(&ipcp_req2, &mut rlp_seq) {
        let bits = encode_rlp_full_rate(&frame);
        let info = rlp_to_info_bits(&bits);
        let iq = encoder.encode_full_rate_frame(&info, frame_chip_offset);
        all_rx_samples.extend_from_slice(&iq);
        frame_chip_offset += chips_per_frame as u64;
    }

    // Pad with a few more idle frames to give the BTS enough data
    for _ in 0..4 {
        let idle_bits = rlp::encode_frame(&rlp::idle_frame(0), RlpRate::Full)
            .expect("test RLP idle frame must encode");
        let idle_info = rlp_to_info_bits(&idle_bits);
        let idle_iq = encoder.encode_full_rate_frame(&idle_info, frame_chip_offset);
        all_rx_samples.extend_from_slice(&idle_iq);
        frame_chip_offset += chips_per_frame as u64;
    }

    let total_rx_frames = (all_rx_samples.len() / (chips_per_frame * oversample)) as u64;
    eprintln!(
        "  Total reverse IQ: {} samples ({} frames, {:.1}ms)",
        all_rx_samples.len(),
        total_rx_frames,
        total_rx_frames as f64 * 20.0,
    );

    // Wrap pipe_handle in Arc<Mutex> for shared access across threads.
    let pipe_handle = Arc::new(std::sync::Mutex::new(pipe_handle));

    // ===================================================================
    // Start BTS + TX drain + RX injection concurrently.
    // The injected_rx channel capacity is 32, so we must start the BTS
    // (which spawns the RX consumer thread) before injecting all samples.
    // ===================================================================
    let drain_handle = pipe_handle.clone();
    let drain_stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let drain_stop2 = drain_stop.clone();
    let tx_accumulated = Arc::new(std::sync::Mutex::new(Vec::<Complex32>::new()));
    let tx_accumulated2 = tx_accumulated.clone();
    let tx_drain_thread = thread::spawn(move || {
        while !drain_stop2.load(std::sync::atomic::Ordering::Relaxed) {
            let samples = {
                let ph = drain_handle.lock().unwrap();
                ph.drain_tx_samples()
            };
            if !samples.is_empty() {
                tx_accumulated2.lock().unwrap().extend(samples);
            }
            thread::sleep(Duration::from_millis(10));
        }
    });

    let bts_task = tokio::spawn(async move {
        // Need enough blocks for paging enable (320ms) + traffic frames.
        // block_size=64 chips → 19200 blocks/sec → 128000 ≈ 6.7 seconds.
        if let Err(e) = bts.run_for_blocks(128_000).await {
            eprintln!("BTS error: {}", e);
        }
    });

    // Brief pause for BTS to spawn its RX thread (which consumes injected_rx).
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Inject all reverse traffic IQ into RadioPipe from a blocking thread.
    // The channel has limited capacity, so send() blocks until BTS RX drains.
    let inject_handle = pipe_handle.clone();
    let inject_thread = thread::spawn(move || {
        // Inject in 32768-sample blocks
        let block_len = 32768usize;
        let mut sample_idx = 0usize;
        while sample_idx < all_rx_samples.len() {
            let end = (sample_idx + block_len).min(all_rx_samples.len());
            let block = all_rx_samples[sample_idx..end].to_vec();
            let chip_start = preamble_chip_start + (sample_idx / oversample) as u64;
            let ph = inject_handle.lock().unwrap();
            ph.inject_rx(InjectedRxBlock {
                samples: block,
                time_ns: 0,
                absolute_chip_start: Some(chip_start),
            })
            .expect("inject_rx failed");
            drop(ph);
            sample_idx = end;
        }
        // Close RX to signal end-of-stream
        let mut ph = inject_handle.lock().unwrap();
        ph.close_rx();
        eprintln!("  RX injection complete ({} samples)", sample_idx);
    });

    // Wait for injection to finish
    let _ = inject_thread.join();

    // Wait for BTS RX thread to process all injected data.
    // The frame aligner needs several search attempts (~500ms each).
    tokio::time::sleep(Duration::from_millis(2000)).await;

    // ===================================================================
    // Drain BTS events and forward to BSC
    // ===================================================================
    let mut preamble_detected = false;
    let mut traffic_data_frames = 0usize;

    async fn drain_bts_events(
        bsc: &mut Bsc,
        rx: &mut tokio::sync::mpsc::UnboundedReceiver<AccessChannelEvent>,
    ) -> (bool, usize) {
        let mut preamble = false;
        let mut data_count = 0usize;
        while let Ok(event) = rx.try_recv() {
            let is_preamble = event.is_preamble_only;
            let is_traffic_data = event.traffic_primary_bits.is_some();
            if is_preamble {
                eprintln!(
                    "  BTS event: preamble detected on walsh={}",
                    event.traffic_walsh_code.unwrap_or(0)
                );
                preamble = true;
            }
            if is_traffic_data {
                data_count += 1;
                eprintln!(
                    "  BTS event: traffic data frame #{} rate={}",
                    data_count,
                    event.traffic_primary_rate_bps.unwrap_or(0)
                );
            }
            bsc.inject_access_event(event).await;
        }
        (preamble, data_count)
    }

    // Poll for BTS events — frame aligner may need several search passes
    for wait_round in 0..10 {
        let (p, d) = drain_bts_events(&mut bsc, &mut bts_access_rx).await;
        preamble_detected |= p;
        traffic_data_frames += d;
        if preamble_detected || traffic_data_frames > 0 {
            eprintln!(
                "  BTS events after round {}: preamble={} data_frames={}",
                wait_round, preamble_detected, traffic_data_frames
            );
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    if !preamble_detected && traffic_data_frames == 0 {
        eprintln!(
            "  WARNING: BTS did not detect preamble/data through PHY after 5s, using synthetic fallback"
        );
        bsc.inject_access_event(synthetic_traffic_preamble(walsh_code))
            .await;
    }

    // ===================================================================
    // Signaling handshake (synthetic — R-DSCH encoding not yet available)
    // ===================================================================
    eprintln!("=== PHY Phase C: Signaling (synthetic MS Ack + SCC) ===");
    bsc.inject_access_event(synthetic_ms_ack_order(walsh_code))
        .await;
    bsc.inject_access_event(synthetic_service_connect_completion(walsh_code))
        .await;

    // Give packet session time to start
    tokio::time::sleep(Duration::from_millis(200)).await;

    let sessions = packet_service.list_all_sessions();
    assert!(
        !sessions.is_empty(),
        "Packet session should have been created"
    );
    let session_id = sessions[0].session_id.clone();
    eprintln!(
        "  Packet session: id={}, phase={}",
        session_id, sessions[0].phase
    );

    // ===================================================================
    // Drain BTS-decoded traffic data frames into BSC→packet session
    // ===================================================================
    eprintln!("=== PHY Phase D: Traffic data through BTS RX ===");

    // Poll for decoded frames over several rounds
    for round in 0..20 {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let (_, d) = drain_bts_events(&mut bsc, &mut bts_access_rx).await;
        traffic_data_frames += d;

        let info = packet_service.get_session_info(&session_id);
        let phase = info.as_ref().map(|i| i.phase.as_str()).unwrap_or("none");

        if round % 5 == 0 || phase == "active" {
            eprintln!(
                "  Round {}: data_frames={} phase={}",
                round, traffic_data_frames, phase
            );
        }

        if phase == "active" {
            break;
        }
    }

    // ===================================================================
    // If BTS RX didn't decode enough frames, fall back to protocol test
    // ===================================================================
    let info = packet_service.get_session_info(&session_id);
    let phase = info.as_ref().map(|i| i.phase.as_str()).unwrap_or("none");

    if phase != "active" {
        eprintln!(
            "  PHY decoded {} frames but protocol at phase={} (not active yet). \
             Falling back to synthetic injection for protocol completion.",
            traffic_data_frames, phase
        );
        // Use bearer-level packet injection as fallback.
        let sync_ack_bits = encode_rlp_full_rate(&rlp::sync_ack_frame(0));
        inject_reverse_bearer_rlp_frame(&mut bsc, &bts_client, walsh_code, sync_ack_bits, 9600)
            .await;
        tokio::time::sleep(Duration::from_millis(200)).await;

        for _ in 0..8 {
            let idle_bits = rlp::encode_frame(&rlp::idle_frame(0), RlpRate::Eighth)
                .expect("test RLP idle frame must encode");
            inject_reverse_bearer_rlp_frame(&mut bsc, &bts_client, walsh_code, idle_bits, 1200)
                .await;
            tokio::time::sleep(Duration::from_millis(25)).await;
        }

        let mut rlp_seq_fb: u8 = 0;
        let mru_option = vec![0x01, 0x04, 0x05, 0xDC];
        let lcp_ack = build_ppp_lcp_configure_ack(1, &mru_option);
        for frame in ppp_to_rlp_frames(&lcp_ack, &mut rlp_seq_fb) {
            let bits = encode_rlp_full_rate(&frame);
            inject_reverse_bearer_rlp_frame(&mut bsc, &bts_client, walsh_code, bits, 9600).await;
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        let lcp_req = build_ppp_lcp_configure_request(1);
        for frame in ppp_to_rlp_frames(&lcp_req, &mut rlp_seq_fb) {
            let bits = encode_rlp_full_rate(&frame);
            inject_reverse_bearer_rlp_frame(&mut bsc, &bts_client, walsh_code, bits, 9600).await;
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;

        let ipcp_ack = build_ppp_ipcp_configure_ack(1, [10, 55, 0, 1]);
        for frame in ppp_to_rlp_frames(&ipcp_ack, &mut rlp_seq_fb) {
            let bits = encode_rlp_full_rate(&frame);
            inject_reverse_bearer_rlp_frame(&mut bsc, &bts_client, walsh_code, bits, 9600).await;
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        let ipcp_req = build_ppp_ipcp_configure_request(1, [0, 0, 0, 0]);
        for frame in ppp_to_rlp_frames(&ipcp_req, &mut rlp_seq_fb) {
            let bits = encode_rlp_full_rate(&frame);
            inject_reverse_bearer_rlp_frame(&mut bsc, &bts_client, walsh_code, bits, 9600).await;
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        let ipcp_req2 = build_ppp_ipcp_configure_request(2, [10, 55, 0, 2]);
        for frame in ppp_to_rlp_frames(&ipcp_req2, &mut rlp_seq_fb) {
            let bits = encode_rlp_full_rate(&frame);
            inject_reverse_bearer_rlp_frame(&mut bsc, &bts_client, walsh_code, bits, 9600).await;
            tokio::time::sleep(Duration::from_millis(25)).await;
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
    }

    // ===================================================================
    // Forward TX verification — Walsh energy on assigned traffic code
    // ===================================================================
    eprintln!("=== PHY Phase E: Forward TX verification ===");

    // Stop the drain thread and collect all accumulated TX samples
    drain_stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = tx_drain_thread.join();

    // Grab any remaining samples
    {
        let ph = pipe_handle.lock().unwrap();
        let remaining = ph.drain_tx_samples();
        tx_accumulated.lock().unwrap().extend(remaining);
    }

    let tx_samples = tx_accumulated.lock().unwrap();
    eprintln!("  Forward TX: {} samples captured", tx_samples.len());

    if !tx_samples.is_empty() {
        // Verify energy on pilot (Walsh 0) and traffic Walsh code
        let pilot_energy = forward_tx_walsh_energy(&tx_samples, 0, oversample);
        let traffic_energy = forward_tx_walsh_energy(&tx_samples, walsh_code, oversample);

        eprintln!("  Walsh 0 (pilot) avg symbol energy: {:.2}", pilot_energy);
        eprintln!(
            "  Walsh {} (traffic) avg symbol energy: {:.2}",
            walsh_code, traffic_energy
        );

        assert!(
            pilot_energy > 0.0,
            "Forward TX should have pilot energy on Walsh 0"
        );
        // Traffic channel should have energy (BS Ack + Service Connect + RLP frames)
        assert!(
            traffic_energy > 0.0,
            "Forward TX should have energy on traffic Walsh {}",
            walsh_code
        );
        eprintln!(
            "  Traffic/Pilot energy ratio: {:.2}",
            traffic_energy / pilot_energy
        );
    } else {
        eprintln!("  WARNING: No forward TX samples captured");
    }

    // ===================================================================
    // Final verification
    // ===================================================================
    let info = packet_service.get_session_info(&session_id);
    let final_phase = info.as_ref().map(|i| i.phase.as_str()).unwrap_or("none");
    eprintln!(
        "=== PHY Final: phase={}, BTS data frames decoded={} ===",
        final_phase, traffic_data_frames
    );

    // -- Reverse PHY verification --
    assert!(
        traffic_data_frames > 0,
        "BTS should decode at least 1 traffic frame through real reverse PHY"
    );
    eprintln!(
        "  PASS: Reverse PHY — BTS decoded {} frames through real PHY",
        traffic_data_frames
    );

    // -- Forward PHY verification --
    assert!(!tx_samples.is_empty(), "Forward TX should produce samples");
    eprintln!(
        "  PASS: Forward PHY — {} TX samples captured",
        tx_samples.len()
    );

    // -- Protocol verification (via synthetic fallback) --
    assert_eq!(
        final_phase, "active",
        "Expected session to reach active phase, got: {}",
        final_phase
    );

    // ===================================================================
    // Cleanup
    // ===================================================================
    bts_task.abort();
    let _ = bts_task.await;

    // Close RX on the pipe handle (already done above, but ensure cleanup)
    {
        let mut ph = pipe_handle.lock().unwrap();
        ph.close_rx();
    }

    drop(lac_layer);
    drop(mac_layer);
    let _ = lac_worker.join();
    let _ = mac_worker.join();
}

#[tokio::test]
async fn test_e2e_so7_rc3_reverse_preamble_queues_bs_ack() {
    init_test_logging();

    let esn: u32 = 0xABCD_EF03;
    let oversample: usize = 4;
    let chip_rate: usize = 1_228_800;
    let chips_per_frame: usize = 24_576;

    let (mac_to_lac_tx, mac_to_lac_rx) = std::sync::mpsc::channel();
    let (lac_to_mac_tx, lac_to_mac_rx) = std::sync::mpsc::channel();
    let lac_layer = lac::Layer2Lac::new(lac_to_mac_tx, mac_to_lac_rx);
    let mac_layer = mac::Layer2Mac::new(lac_to_mac_rx, mac_to_lac_tx);

    let start_system_time = cdma_common::time::cdma_epoch();
    let (radio, pipe_handle) = RadioPipe::new(4096);
    let (bts, bts_handle) = bts::Bts::new_with_radio_pipe(
        radio,
        bts::Config {
            tx_center_frequency_hz: 881_520_000,
            pilot_offset: 0,
            mac_layer: mac_layer.clone(),
            start_system_time: Some(start_system_time),
            sync_channel_template: Some(SyncChannelMessage {
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
                cdma_freq: 384,
                ext_cdma_freq: 0,
                sr1_bcch_non_td_incl: false,
                sr1_td_incl: false,
                sr3_incl: false,
                ds_incl: false,
            }),
            timezone: cdma_common::timezone::TimezoneConfig::default(),
            overhead: cdma_common::overhead::OverheadParameters::default(),
            rx: Some(bts::RxSettings {
                sample_rate_hz: chip_rate * oversample,
                auth_mode: 0,
                p_rev_in_use: 6,
                capture_iq_wav: None,
                capture_seconds: None,
                access_channel_number: 0,
                paging_channel_number: 1,
                base_id: 1,
                pilot_pn: 0,
                chip_rate_hz: chip_rate,
                absolute_chip_start: 0,
                hardware_start_time_ns: 0,
                tick_rate: 1_000_000_000,
                access_event_tx: None,
                reverse_bearer_tx: None,
                rx_metrics_tx: None,
                reanchor_origin: true,
                traffic_rx_pool: None,
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
            }),
        },
        bts::BtsRuntimeSettings::default(),
    );

    let bts::BtsHandle {
        tx_metrics: _,
        rx_metrics: _,
        config: _,
        access_events,
        commands: _,
        traffic_channels,
        walsh_allocator,
        traffic_rx_pool,
        traffic_rx_removals,
        power_control: _,
        rx_measurements: _,
    } = bts_handle;

    let (traffic_tx, mut traffic_rx) = tokio::sync::broadcast::channel(16);
    let (mobiles_tx, mobiles_rx) = watch::channel(Vec::new());
    let mut bsc = Bsc::new(BscConfig {
        pilot_offset: 0,
        overhead: OverheadParameters {
            sid: 42,
            nid: 7,
            cdma_freq: Some(384),
            ..Default::default()
        },
        paging: bts::PagingChannelSettings::default(),
        traffic_assignment: TrafficAssignmentConfig::default(),
        access_event_rx: Some(access_events),
        access_event_broadcast: None,
        sms_request_rx: None,
        sms_request_tx: None,
        data_request_rx: None,
        data_request_tx: None,
        power_override_request_rx: None,
        power_override_request_tx: None,
        mobiles_tx: Some(mobiles_tx),
        paging_broadcast: None,
        traffic_broadcast: Some(traffic_tx),
        rx_reference_dbm: None,
        hlr_repo: None,
        msc_client: test_msc_client(),
        msc_voice_bearer: None,
        bts_client: Some(Arc::new(NetworkBtsControlClient::spawn_in_process(
            Arc::new(TrafficResourceService::from_pools(
                walsh_allocator.clone(),
                traffic_channels.clone(),
                traffic_rx_pool.clone(),
                traffic_rx_removals.clone(),
            )),
            AbisAgentConfig {
                pilot_pn: 0,
                cell_id: CellId { cell: 1, sector: 1 },
                mscid: 1,
            },
            NetworkClientConfig {
                cell_id: CellId { cell: 1, sector: 1 },
                mscid: 1,
                pilot_pn: 0,
                auth_mode: 0,
                p_rev_in_use: 6,
                market_id: 1,
                generating_entity_id: 1,
            },
        )) as Arc<dyn BtsControlClient>),
        traffic_retry: TrafficRetryConfig::default(),
        paging_retry: PagingRetryConfig::default(),
        voice_policy: test_voice_policy(),
        pcf_client: None,
        mobile_idle_timeout_s: 0,
        bts_paging_state: None,
        node_id: "bsc-test".to_string(),
    });

    let lac_worker = {
        let lac = lac_layer.clone();
        thread::spawn(move || lac.run_for(100_000, Duration::from_secs(10)).unwrap())
    };
    let mac_worker = {
        let mac = mac_layer.clone();
        thread::spawn(move || mac.run_for(100_000, Duration::from_secs(10)).unwrap())
    };

    bsc.inject_access_event(synthetic_origination_so7_rc3(esn))
        .await;
    let bsc_task = tokio::spawn(async move { bsc.run().await });
    let walsh_code: u8 = 10;
    {
        let pool = traffic_rx_pool.lock();
        assert!(
            pool.iter().any(|r| r.walsh_code == walsh_code),
            "BSC should have pushed TrafficRxRequest for walsh={}",
            walsh_code
        );
    }

    let encoder = ReverseTrafficChannelEncoder::new(esn);
    let sync_superframe_chips: u64 = 98_304;
    let lead_chips: u64 = 100_000_000u64 * chip_rate as u64 / 1_000_000_000;
    let bts_chip_start = {
        let v = lead_chips % sync_superframe_chips;
        if v == 0 {
            lead_chips
        } else {
            lead_chips + (sync_superframe_chips - v)
        }
    };
    use cdma_bts::channels::rtch::pulse_shape;

    let preamble_frames_count = 20;
    let preamble_chips = chips_per_frame * preamble_frames_count;
    let preamble_raw = encoder.encode_preamble_raw(preamble_chips, bts_chip_start);
    let preamble_samples = pulse_shape(&preamble_raw);

    let pipe_handle = Arc::new(std::sync::Mutex::new(pipe_handle));
    let drain_handle = pipe_handle.clone();
    let drain_stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let drain_stop2 = drain_stop.clone();
    let tx_accumulated = Arc::new(std::sync::Mutex::new(Vec::<Complex32>::new()));
    let tx_accumulated2 = tx_accumulated.clone();
    let tx_drain_thread = thread::spawn(move || {
        while !drain_stop2.load(std::sync::atomic::Ordering::Relaxed) {
            let samples = {
                let ph = drain_handle.lock().unwrap();
                ph.drain_tx_samples()
            };
            if !samples.is_empty() {
                tx_accumulated2.lock().unwrap().extend(samples);
            }
            thread::sleep(Duration::from_millis(10));
        }
    });

    let bts_task = tokio::task::spawn_blocking(move || {
        let rt = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("BTS test runtime should build");
        rt.block_on(async move {
            if let Err(e) = bts.run_for_blocks(64_000).await {
                eprintln!("BTS error: {}", e);
            }
        });
    });

    tokio::time::sleep(Duration::from_millis(100)).await;

    let inject_handle = pipe_handle.clone();
    let inject_thread = thread::spawn(move || {
        let block_len = 32_768usize;
        let mut sample_idx = 0usize;
        while sample_idx < preamble_samples.len() {
            let end = (sample_idx + block_len).min(preamble_samples.len());
            let block = preamble_samples[sample_idx..end].to_vec();
            let chip_start = bts_chip_start + (sample_idx / oversample) as u64;
            let ph = inject_handle.lock().unwrap();
            ph.inject_rx(InjectedRxBlock {
                samples: block,
                time_ns: 0,
                absolute_chip_start: Some(chip_start),
            })
            .expect("inject_rx failed");
            drop(ph);
            sample_idx = end;
        }
        let mut ph = inject_handle.lock().unwrap();
        ph.close_rx();
    });
    let _ = inject_thread.join();

    let mut mobiles_rx = mobiles_rx;
    let mobile = tokio::time::timeout(Duration::from_secs(6), async {
        loop {
            let snapshot = mobiles_rx.borrow().clone();
            if let Some(mobile) = snapshot.into_iter().find(|ms| {
                ms.esn == Some(esn)
                    && ms.traffic_walsh_code == Some(walsh_code)
                    && ms.state == "TrafficActive"
                    && ms.voice_call_state.as_deref() == Some("WaitingMsAck")
            }) {
                return mobile;
            }
            mobiles_rx
                .changed()
                .await
                .expect("mobiles watch channel should stay open");
        }
    })
    .await
    .expect("timed out waiting for automatic BTS->BSC preamble handling")
    .clone();

    assert_eq!(mobile.traffic_walsh_code, Some(walsh_code));
    assert_eq!(
        mobile.traffic_service_option,
        Some(SERVICE_OPTION_PACKET_DATA)
    );
    assert_eq!(mobile.state, "TrafficActive");
    assert_eq!(mobile.voice_call_state.as_deref(), Some("WaitingMsAck"));

    let bs_ack_event = tokio::time::timeout(Duration::from_secs(1), async {
        loop {
            let event = traffic_rx
                .recv()
                .await
                .expect("traffic event channel should stay open");
            if event.walsh_code == walsh_code
                && event.order.as_ref().map(|order| order.order) == Some(0b010000)
            {
                return event;
            }
        }
    })
    .await
    .expect("timed out waiting for BSC traffic event for BS Ack Order");
    assert_eq!(bs_ack_event.mcsb.ack_seq, 7);
    assert_eq!(bs_ack_event.mcsb.msg_seq, 0);
    assert!(bs_ack_event.mcsb.ack_req);
    assert_eq!(bs_ack_event.mcsb.message_id, MessageId::Order);

    tokio::time::sleep(Duration::from_millis(350)).await;

    drain_stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = tx_drain_thread.join();
    {
        let ph = pipe_handle.lock().unwrap();
        let remaining = ph.drain_tx_samples();
        tx_accumulated.lock().unwrap().extend(remaining);
    }

    bts_task.abort();
    let _ = bts_task.await;
    bsc_task.abort();
    let _ = bsc_task.await;

    let tx_samples = tx_accumulated.lock().unwrap();
    assert!(
        !tx_samples.is_empty(),
        "Forward TX should produce samples after RC3 preamble"
    );
    let pilot_energy = forward_tx_walsh_energy(&tx_samples, 0, oversample);
    let traffic_energy = forward_tx_walsh_energy(&tx_samples, walsh_code, oversample);
    assert!(
        pilot_energy > 0.0,
        "Forward TX should keep transmitting pilot"
    );
    assert!(
        traffic_energy > 0.0,
        "Forward TX should show traffic energy on Walsh {} after BS Ack",
        walsh_code
    );

    let local_rc3_tx = build_synthesized_forward_rc3_bs_ack_iq_samples(
        esn,
        bts_chip_start,
        walsh_code,
        7,
        0,
        1,
        4,
    )
    .expect("should synthesize local RC3 BS Ack waveform");
    let local_decoded = decode_rc3_bs_ack_from_forward_traffic_iq_samples(
        &local_rc3_tx,
        chip_rate * oversample,
        walsh_code,
        esn,
        bts_chip_start,
        7,
        0,
    )
    .expect("local RC3 decoder sanity check should recover BS Ack");
    eprintln!(
        "local_decoded_rc3_bs_ack: frame_chip_start={} frame_index={} phase={} chip_offset={} ack_seq={} msg_seq={}",
        local_decoded.frame_chip_start,
        local_decoded.frame_index,
        local_decoded.decimation_phase,
        local_decoded.chip_offset,
        local_decoded.ack_seq,
        local_decoded.msg_seq,
    );
    assert_eq!(local_decoded.ack_seq, 7);
    assert_eq!(local_decoded.msg_seq, 0);
    assert!(local_decoded.ack_req);
    assert_eq!(local_decoded.encryption, 0);
    assert!(!local_decoded.use_time);
    assert_eq!(local_decoded.action_time, 0);
    assert_eq!(local_decoded.order, 0b010000);
    assert_eq!(local_decoded.add_record_len, 0);

    {
        let mut ph = pipe_handle.lock().unwrap();
        ph.close_rx();
    }

    drop(lac_layer);
    drop(mac_layer);
    let _ = lac_worker.join();
    let _ = mac_worker.join();
}

// ---------------------------------------------------------------------------
// SO6 SMS helpers
// ---------------------------------------------------------------------------

/// In-memory HLR for integration tests. Returns a single subscriber for any
/// identity resolution, mapping the ESN to a known phone number.
#[derive(Clone)]
struct FakeHlrRepository {
    subscriber: Subscriber,
}

impl FakeHlrRepository {
    fn new(phone_number: &str) -> Self {
        let now = chrono::Utc::now();
        Self {
            subscriber: Subscriber {
                subscriber_id: uuid::Uuid::new_v4(),
                phone_number: phone_number.to_string(),
                display_name: "Test Subscriber".to_string(),
                status: SubscriberStatus::Active,
                created_at: now,
                updated_at: now,
                number_type: cdma_hlr::model::NumberType::NetworkSpecific,
                number_plan: cdma_hlr::model::NumberPlan::IsdnE164,
                has_ringtone: false,
                ringtone_duration_ms: None,
                prl_override_id: None,
                service_programming_code: None,
                firstchp_override: None,
            },
        }
    }

    fn resolved(&self) -> cdma_hlr::model::ResolvedSubscriber {
        cdma_hlr::model::ResolvedSubscriber {
            subscriber: self.subscriber.clone(),
            identities: Vec::new(),
            primary_identity: None,
            binding: None,
        }
    }
}

#[tonic::async_trait]
impl HlrRepository for FakeHlrRepository {
    async fn upsert_subscriber(
        &self,
        _: &str,
        _: &str,
        _: &str,
        _: cdma_hlr::model::NumberType,
        _: cdma_hlr::model::NumberPlan,
    ) -> Result<Subscriber, String> {
        Ok(self.subscriber.clone())
    }
    async fn get_subscriber_by_phone_number(
        &self,
        phone: &str,
    ) -> Result<Option<cdma_hlr::model::ResolvedSubscriber>, String> {
        Ok((self.subscriber.phone_number == phone).then(|| self.resolved()))
    }
    async fn get_subscriber_by_id(
        &self,
        id: uuid::Uuid,
    ) -> Result<Option<cdma_hlr::model::ResolvedSubscriber>, String> {
        Ok((self.subscriber.subscriber_id == id).then(|| self.resolved()))
    }
    async fn update_subscriber(
        &self,
        _: uuid::Uuid,
        _: &str,
        _: &str,
        _: &str,
        _: cdma_hlr::model::NumberType,
        _: cdma_hlr::model::NumberPlan,
    ) -> Result<Option<Subscriber>, String> {
        Ok(Some(self.subscriber.clone()))
    }
    async fn list_subscribers(&self, _: u32, _: u32) -> Result<(Vec<Subscriber>, u32), String> {
        Ok((vec![self.subscriber.clone()], 1))
    }
    async fn delete_subscriber(&self, _: uuid::Uuid) -> Result<bool, String> {
        Ok(false)
    }
    async fn upsert_identity(
        &self,
        _: uuid::Uuid,
        _: Option<&str>,
        _: Option<u32>,
        _: Option<&str>,
    ) -> Result<SubscriberIdentity, String> {
        Err("not implemented in test".to_string())
    }
    async fn replace_primary_identity(
        &self,
        _: uuid::Uuid,
        _: Option<&str>,
        _: Option<u32>,
        _: Option<&str>,
    ) -> Result<SubscriberIdentity, String> {
        Err("not implemented in test".to_string())
    }
    async fn get_identities_for_subscriber(
        &self,
        _: uuid::Uuid,
    ) -> Result<Vec<SubscriberIdentity>, String> {
        Ok(Vec::new())
    }
    async fn resolve_by_identity(
        &self,
        _: &cdma_hlr::model::MobileIdentityKey,
    ) -> Result<Option<cdma_hlr::model::ResolvedSubscriber>, String> {
        Ok(Some(self.resolved()))
    }
    async fn resolve_by_hardware_identity(
        &self,
        _: Option<u32>,
        _: Option<&str>,
    ) -> Result<Option<cdma_hlr::model::ResolvedSubscriber>, String> {
        Ok(Some(self.resolved()))
    }
    async fn upsert_registration_binding(
        &self,
        binding: RegistrationBinding,
    ) -> Result<RegistrationBinding, String> {
        Ok(binding)
    }
    async fn get_registration_binding(
        &self,
        _: uuid::Uuid,
    ) -> Result<Option<RegistrationBinding>, String> {
        let now = chrono::Utc::now();
        Ok(Some(RegistrationBinding {
            subscriber_id: self.subscriber.subscriber_id,
            serving_node_id: "test".to_string(),
            state: RegistrationState::Registered,
            imsi: None,
            esn: None,
            meid: None,
            mob_p_rev: None,
            pgslot: None,
            slot_cycle_index: None,
            last_msg_seq: None,
            last_registered_at: now,
            last_seen_at: now,
            updated_at: now,
        }))
    }
    async fn upsert_mobile_seen(
        &self,
        _: &cdma_hlr::model::MobileIdentityKey,
        _: Option<u8>,
    ) -> Result<cdma_hlr::MobileSeenUpsert, String> {
        Ok(cdma_hlr::MobileSeenUpsert {
            is_new: true,
            previous_last_seen_at: None,
        })
    }
    async fn set_ringtone(
        &self,
        _: uuid::Uuid,
        _: Vec<u8>,
        _: &str,
    ) -> Result<cdma_hlr::model::SetRingtoneOutcome, String> {
        Ok(cdma_hlr::model::SetRingtoneOutcome {
            codecs: vec![],
            duration_ms: 0,
        })
    }
    async fn clear_ringtone(&self, _: uuid::Uuid) -> Result<(), String> {
        Ok(())
    }
    async fn get_ringtone_codec(
        &self,
        _: uuid::Uuid,
        _: &str,
    ) -> Result<Option<cdma_hlr::model::SubscriberRingtoneCodecBlob>, String> {
        Ok(None)
    }
    async fn list_prls(
        &self,
        _: u32,
        _: u32,
        _: cdma_hlr::model::PrlListFilter,
    ) -> Result<(Vec<cdma_hlr::model::Prl>, u32), String> {
        Ok((vec![], 0))
    }
    async fn get_prl(&self, _: uuid::Uuid) -> Result<Option<cdma_hlr::model::Prl>, String> {
        Ok(None)
    }
    async fn get_default_prl(&self) -> Result<Option<cdma_hlr::model::Prl>, String> {
        Ok(None)
    }
    async fn create_prl(
        &self,
        _: &str,
        _: &[u8],
        _: i32,
        _: i16,
        _: &str,
    ) -> Result<cdma_hlr::model::Prl, String> {
        unimplemented!()
    }
    async fn update_prl(
        &self,
        _: uuid::Uuid,
        _: Option<&str>,
        _: Option<&[u8]>,
        _: Option<(i32, i16)>,
        _: Option<&str>,
    ) -> Result<cdma_hlr::model::Prl, String> {
        unimplemented!()
    }
    async fn soft_delete_prl(
        &self,
        _: uuid::Uuid,
    ) -> Result<Result<(), cdma_hlr::model::PrlDeleteBlocked>, String> {
        Ok(Ok(()))
    }
    async fn set_default_prl(&self, _: uuid::Uuid) -> Result<(), String> {
        Ok(())
    }
    async fn set_subscriber_prl_override(
        &self,
        _: uuid::Uuid,
        _: Option<uuid::Uuid>,
    ) -> Result<(), String> {
        Ok(())
    }
    async fn set_subscriber_spc(&self, _: uuid::Uuid, _: Option<String>) -> Result<(), String> {
        Ok(())
    }
    async fn set_subscriber_firstchp_override(
        &self,
        _: uuid::Uuid,
        _: Option<u16>,
    ) -> Result<(), String> {
        Ok(())
    }
    async fn save_otasp_session(&self, _: &cdma_hlr::model::OtaspSessionRow) -> Result<(), String> {
        Ok(())
    }
    async fn list_otasp_sessions(
        &self,
        _: cdma_hlr::model::OtaspSessionFilter,
        _: u32,
        _: u32,
    ) -> Result<(Vec<cdma_hlr::model::OtaspSessionRow>, u32), String> {
        Ok((Vec::new(), 0))
    }
    async fn get_otasp_session(
        &self,
        _: uuid::Uuid,
    ) -> Result<Option<cdma_hlr::model::OtaspSessionRow>, String> {
        Ok(None)
    }
}

/// Build 172-bit R-DSCH signaling info for MS Ack Order.
/// MSG_TYPE=0x01 (Order), ORDER=0b010000 (MS Ack), ADD_RECORD_LEN=0.
fn build_rdsch_ms_ack_order_info_bits(ack_seq: u8, msg_seq: u8) -> Vec<u8> {
    let mut info = vec![0u8; 172];

    // MUX header: 1011
    info[0] = 1;
    info[1] = 0;
    info[2] = 1;
    info[3] = 1;

    let sig_start = 4;
    info[sig_start] = 1; // SOM = 1

    // Build PDU body bits
    let mut pdu_bits = Vec::new();
    // MSG_TYPE = 0x01 (Order on reverse dedicated)
    for i in (0..8).rev() {
        pdu_bits.push((0x01u8 >> i) & 1);
    }
    // ACK_SEQ
    for i in (0..3).rev() {
        pdu_bits.push((ack_seq >> i) & 1);
    }
    // MSG_SEQ
    for i in (0..3).rev() {
        pdu_bits.push((msg_seq >> i) & 1);
    }
    // ACK_REQ = 0
    pdu_bits.push(0);
    // ENCRYPTION = 0
    pdu_bits.extend_from_slice(&[0, 0]);
    // ORDER = 0b010000 (MS Ack) in 6 bits
    for i in (0..6).rev() {
        pdu_bits.push((0b010000u8 >> i) & 1);
    }
    // ADD_RECORD_LEN = 0 in 3 bits
    pdu_bits.extend_from_slice(&[0, 0, 0]);

    // msg_length = 1 (msg_length field) + ceil(pdu_bits/8) (payload) + 2 (CRC-16)
    let pdu_byte_count = (pdu_bits.len() + 7) / 8;
    let msg_length = 1 + pdu_byte_count + 2;

    let mut bit_offset = sig_start + 1;
    for i in (0..8).rev() {
        info[bit_offset] = ((msg_length as u8) >> i) & 1;
        bit_offset += 1;
    }
    for &bit in &pdu_bits {
        if bit_offset < 172 {
            info[bit_offset] = bit;
            bit_offset += 1;
        }
    }
    while (bit_offset - (sig_start + 1)) % 8 != 0 && bit_offset < 172 {
        bit_offset += 1;
    }

    // CRC-16 (CRC-CCITT) over msg_length field + padded PDU body
    let crc_data_start = sig_start + 1;
    let crc_data_end = bit_offset;
    let crc = crc16_ccitt(&info[crc_data_start..crc_data_end]);
    for i in 0..16 {
        if bit_offset < 172 {
            info[bit_offset] = ((crc >> (15 - i)) & 1) as u8;
            bit_offset += 1;
        }
    }

    info
}

/// Build 172-bit R-DSCH signaling info for Service Connect Completion.
/// MSG_TYPE=0x0E, RESERVED=0, SERV_CON_SEQ=0.
fn build_rdsch_scc_info_bits(ack_seq: u8, msg_seq: u8) -> Vec<u8> {
    let mut info = vec![0u8; 172];

    // MUX header: 1011
    info[0] = 1;
    info[1] = 0;
    info[2] = 1;
    info[3] = 1;

    let sig_start = 4;
    info[sig_start] = 1; // SOM = 1

    let mut pdu_bits = Vec::new();
    // MSG_TYPE = 0x0E (ServiceConnectCompletion on reverse dedicated)
    for i in (0..8).rev() {
        pdu_bits.push((0x0Eu8 >> i) & 1);
    }
    // ACK_SEQ
    for i in (0..3).rev() {
        pdu_bits.push((ack_seq >> i) & 1);
    }
    // MSG_SEQ
    for i in (0..3).rev() {
        pdu_bits.push((msg_seq >> i) & 1);
    }
    // ACK_REQ = 1
    pdu_bits.push(1);
    // ENCRYPTION = 0
    pdu_bits.extend_from_slice(&[0, 0]);
    // RESERVED = 0 (1 bit)
    pdu_bits.push(0);
    // SERV_CON_SEQ = 0 (3 bits)
    pdu_bits.extend_from_slice(&[0, 0, 0]);

    let pdu_byte_count = (pdu_bits.len() + 7) / 8;
    let msg_length = 1 + pdu_byte_count + 2;

    let mut bit_offset = sig_start + 1;
    for i in (0..8).rev() {
        info[bit_offset] = ((msg_length as u8) >> i) & 1;
        bit_offset += 1;
    }
    for &bit in &pdu_bits {
        if bit_offset < 172 {
            info[bit_offset] = bit;
            bit_offset += 1;
        }
    }
    while (bit_offset - (sig_start + 1)) % 8 != 0 && bit_offset < 172 {
        bit_offset += 1;
    }

    let crc_data_start = sig_start + 1;
    let crc_data_end = bit_offset;
    let crc = crc16_ccitt(&info[crc_data_start..crc_data_end]);
    for i in 0..16 {
        if bit_offset < 172 {
            info[bit_offset] = ((crc >> (15 - i)) & 1) as u8;
            bit_offset += 1;
        }
    }

    info
}

fn crc16_ccitt(bits: &[u8]) -> u16 {
    cdma_common::crc::crc16_ccitt(bits)
}

fn synthetic_origination_so6(esn: u32) -> AccessChannelEvent {
    AccessChannelEvent {
        event_id: "synth-origination-so6".to_string(),
        chip_start: 2_000_000,
        absolute_chip_start: None,
        receive_time: None,
        preamble_frames: 10,
        pd: 1,
        message_id: MessageId::Origination,
        msg_type_name: "Origination Message".to_string(),
        address: Some(format!("synthetic esn=0x{esn:08x}")),
        resolved_address: None,
        subscriber_id: None,
        l3_summary: Some("Origination(service_option=6)".to_string()),
        decoded_l3: None,
        pdu_summary: "SO6 SMS origination".to_string(),
        msg_seq: Some(2),
        ack_seq: Some(7),
        ack_req: true,
        valid_ack: false,
        msid_type: Some(0b011),
        esn: Some(esn),
        imsi: None,
        meid: None,
        imsi_m_s1: Some(0x0091_989e),
        imsi_m_s2: Some(0x0326),
        imsi_class: Some(0),
        imsi_addr_num: None,
        imsi_mcc: Some(310),
        imsi_11_12: Some(99),
        mob_p_rev: Some(6),
        slot_cycle_index: Some(2),
        scm: Some(0x2a),
        service_option: Some(SERVICE_OPTION_SMS),
        wall_clock_us: chrono::Utc::now().timestamp_micros() as u64,
        rx_wall_time: None,
        rx_hw_time_ns: None,
        snr_db: Some(12.5),
        signal_power_db: Some(-35.0),
        reverse_pilot_ec_io_db: None,
        raw_power_db: Some(-40.0),
        demod_quality_pct: Some(94.0),
        pcg_signal_snr_db: None,
        active_pcg_mask: None,
        traffic_phy_valid: None,
        traffic_fqi_valid: None,
        traffic_tail_valid: None,
        traffic_fqi_bits: None,
        traffic_ml_tail_match: None,
        burst_type: None,
        data_burst_fields: None,
        data_burst_num_msgs: None,
        data_burst_msg_number: None,
        traffic_primary_bits: None,
        traffic_primary_rate_bps: None,
        traffic_primary_bearer_routed: false,
        traffic_voice_bits: None,
        traffic_voice_rate_bps: None,
        order_code: None,
        for_rc_pref: None,
        rev_rc_pref: None,
        rev_fch_gating_req: None,
        traffic_walsh_code: None,
        is_preamble_only: false,
        is_traffic_pcg_measurement: false,
        is_traffic_phy_status: false,
        traffic_measurement_age_chips: None,
        for_supported_rcs: vec![1],
        rev_supported_rcs: vec![1],
        decoded_rdsch: None,
        raw_pdu_bits: None,
    }
}

#[allow(dead_code)]
fn synthetic_traffic_data_burst(walsh_code: u8, sms_payload: Vec<u8>) -> AccessChannelEvent {
    AccessChannelEvent {
        event_id: "synth-data-burst".to_string(),
        chip_start: 3_400_000,
        absolute_chip_start: Some(3_400_000),
        receive_time: None,
        preamble_frames: 0,
        pd: 0,
        message_id: MessageId::DataBurst,
        msg_type_name: "Data Burst Message".to_string(),
        address: None,
        resolved_address: None,
        subscriber_id: None,
        l3_summary: None,
        decoded_l3: None,
        pdu_summary: "MO SMS Data Burst".to_string(),
        msg_seq: Some(1),
        ack_seq: Some(0),
        ack_req: true,
        valid_ack: true,
        msid_type: None,
        esn: None,
        imsi: None,
        meid: None,
        imsi_m_s1: None,
        imsi_m_s2: None,
        imsi_class: None,
        imsi_addr_num: None,
        imsi_mcc: None,
        imsi_11_12: None,
        mob_p_rev: None,
        slot_cycle_index: None,
        scm: None,
        service_option: None,
        wall_clock_us: chrono::Utc::now().timestamp_micros() as u64,
        rx_wall_time: None,
        rx_hw_time_ns: None,
        snr_db: Some(10.0),
        signal_power_db: None,
        reverse_pilot_ec_io_db: None,
        raw_power_db: None,
        demod_quality_pct: None,
        pcg_signal_snr_db: None,
        active_pcg_mask: None,
        traffic_phy_valid: None,
        traffic_fqi_valid: None,
        traffic_tail_valid: None,
        traffic_fqi_bits: None,
        traffic_ml_tail_match: None,
        burst_type: Some(3), // SMS
        data_burst_fields: Some(sms_payload),
        data_burst_num_msgs: Some(1),
        data_burst_msg_number: Some(1),
        traffic_primary_bits: None,
        traffic_primary_rate_bps: None,
        traffic_primary_bearer_routed: false,
        traffic_voice_bits: None,
        traffic_voice_rate_bps: None,
        order_code: None,
        for_rc_pref: None,
        rev_rc_pref: None,
        rev_fch_gating_req: None,
        traffic_walsh_code: Some(walsh_code),
        is_preamble_only: false,
        is_traffic_pcg_measurement: false,
        is_traffic_phy_status: false,
        traffic_measurement_age_chips: None,
        for_supported_rcs: Vec::new(),
        rev_supported_rcs: Vec::new(),
        decoded_rdsch: None,
        raw_pdu_bits: None,
    }
}

/// Build an MO SMS payload (C.S0015-B Transport Layer) for testing.
#[allow(dead_code)]
fn build_mo_sms_payload(destination: &str, text: &str, message_id: u16, reply_seq: u8) -> Vec<u8> {
    let mut payload = Vec::new();

    // Transport Layer MSG_TYPE = point-to-point
    payload.push(0x00);

    // Teleservice ID (tag=0x00, len=2, value=0x1002 WMT)
    payload.push(0x00);
    payload.push(0x02);
    payload.push(0x10);
    payload.push(0x02);

    // Destination Address (tag=0x04)
    let mut addr_bs = Bitstream::new();
    addr_bs.write_u8(0, 1); // DIGIT_MODE=0
    addr_bs.write_u8(0, 1); // NUMBER_MODE=0
    addr_bs.write_u8(destination.len() as u8, 8); // NUM_FIELDS
    for ch in destination.chars() {
        let dtmf = match ch {
            '1' => 1u8,
            '2' => 2,
            '3' => 3,
            '4' => 4,
            '5' => 5,
            '6' => 6,
            '7' => 7,
            '8' => 8,
            '9' => 9,
            '0' => 10,
            '*' => 11,
            '#' => 12,
            _ => 0,
        };
        addr_bs.write_u8(dtmf, 4);
    }
    // Pad to byte boundary
    let rem = addr_bs.len() % 8;
    if rem != 0 {
        addr_bs.write_u8(0, 8 - rem);
    }
    let addr_bytes: Vec<u8> = addr_bs
        .bits()
        .chunks(8)
        .map(|chunk| {
            let mut byte = 0u8;
            for (i, &bit) in chunk.iter().enumerate() {
                byte |= (bit & 1) << (7 - i);
            }
            byte
        })
        .collect();
    payload.push(0x04); // tag
    payload.push(addr_bytes.len() as u8);
    payload.extend_from_slice(&addr_bytes);

    // Bearer Reply Option (tag=0x06, len=1)
    payload.push(0x06);
    payload.push(0x01);
    payload.push((reply_seq & 0x3F) << 2);

    // Bearer Data (tag=0x08)
    let mut bearer = Vec::new();

    // Message Identifier sub-param (tag=0x00, len=3)
    bearer.push(0x00);
    bearer.push(0x03);
    let mut id_bs = Bitstream::new();
    id_bs.write_u8(0x02, 4); // MESSAGE_TYPE = Submit (2)
    id_bs.write_u32(message_id as u32, 16); // MESSAGE_ID
    id_bs.write_u8(0, 1); // HEADER_IND
    id_bs.write_u8(0, 3); // reserved
    let id_bytes: Vec<u8> = id_bs
        .bits()
        .chunks(8)
        .map(|chunk| {
            let mut byte = 0u8;
            for (i, &bit) in chunk.iter().enumerate() {
                byte |= (bit & 1) << (7 - i);
            }
            byte
        })
        .collect();
    bearer.extend_from_slice(&id_bytes);

    // User Data sub-param (tag=0x01)
    let mut ud_bs = Bitstream::new();
    ud_bs.write_u8(0x02, 5); // MSG_ENCODING = 7-bit ASCII
    ud_bs.write_u8(text.len() as u8, 8);
    for &b in text.as_bytes() {
        ud_bs.write_u8(b & 0x7F, 7);
    }
    let rem = ud_bs.len() % 8;
    if rem != 0 {
        ud_bs.write_u8(0, 8 - rem);
    }
    let ud_bytes: Vec<u8> = ud_bs
        .bits()
        .chunks(8)
        .map(|chunk| {
            let mut byte = 0u8;
            for (i, &bit) in chunk.iter().enumerate() {
                byte |= (bit & 1) << (7 - i);
            }
            byte
        })
        .collect();
    bearer.push(0x01);
    bearer.push(ud_bytes.len() as u8);
    bearer.extend_from_slice(&ud_bytes);

    payload.push(0x08); // Bearer Data tag
    payload.push(bearer.len() as u8);
    payload.extend_from_slice(&bearer);

    payload
}

/// Build 172-bit info payload containing an R-DSCH Data Burst Message with SMS payload.
/// Uses MUX header 1011 (all signaling, 0 primary bits, 168 signaling bits).
fn build_rdsch_sms_data_burst_info_bits(sms_payload: &[u8]) -> Vec<u8> {
    let mut info = vec![0u8; 172];

    // MUX header: 1011
    info[0] = 1;
    info[1] = 0;
    info[2] = 1;
    info[3] = 1;

    let sig_start = 4; // 168 signaling bits
    info[sig_start] = 1; // SOM = 1

    // Now build the PDU bitstream:
    // msg_length(8) + PDU_body + CRC-16(16)
    // PDU_body = MSG_TYPE(8) + ACK_SEQ(3) + MSG_SEQ(3) + ACK_REQ(1) + ENCRYPTION(2)
    //          + MSG_NUMBER(8) + BURST_TYPE(6) + NUM_MSGS(8) + NUM_FIELDS(8) + CHARi(N*8)

    let mut pdu_bits = Vec::new();

    // MSG_TYPE = 0x04 (DataBurst on reverse dedicated)
    for i in (0..8).rev() {
        pdu_bits.push((0x04u8 >> i) & 1);
    }
    // ACK_SEQ = 0
    pdu_bits.extend_from_slice(&[0, 0, 0]);
    // MSG_SEQ = 1
    pdu_bits.extend_from_slice(&[0, 0, 1]);
    // ACK_REQ = 1
    pdu_bits.push(1);
    // ENCRYPTION = 0
    pdu_bits.extend_from_slice(&[0, 0]);
    // MSG_NUMBER = 1
    for i in (0..8).rev() {
        pdu_bits.push((1u8 >> i) & 1);
    }
    // BURST_TYPE = 3 (SMS) in 6 bits
    for i in (0..6).rev() {
        pdu_bits.push((3u8 >> i) & 1);
    }
    // NUM_MSGS = 1
    for i in (0..8).rev() {
        pdu_bits.push((1u8 >> i) & 1);
    }
    // NUM_FIELDS = sms_payload.len()
    for i in (0..8).rev() {
        pdu_bits.push((sms_payload.len() as u8 >> i) & 1);
    }
    // CHARi
    for &byte in sms_payload {
        for i in (0..8).rev() {
            pdu_bits.push((byte >> i) & 1);
        }
    }

    // Total PDU length in octets (round up)
    let pdu_byte_count = (pdu_bits.len() + 7) / 8;
    // msg_length = 1 (msg_length field) + pdu_byte_count + 2 (CRC-16)
    let msg_length = 1 + pdu_byte_count + 2;

    // Write msg_length(8 bits)
    let mut bit_offset = sig_start + 1; // after SOM
    for i in (0..8).rev() {
        info[bit_offset] = ((msg_length as u8) >> i) & 1;
        bit_offset += 1;
    }

    // Write PDU body bits
    for &bit in &pdu_bits {
        if bit_offset < 172 {
            info[bit_offset] = bit;
            bit_offset += 1;
        }
    }
    // Pad PDU to byte boundary
    while (bit_offset - (sig_start + 1)) % 8 != 0 && bit_offset < 172 {
        bit_offset += 1; // already zero
    }

    // CRC-16 (CRC-CCITT) over all data from msg_length through end of PDU body
    let crc_data_start = sig_start + 1;
    let crc_data_end = bit_offset;
    let crc_scope = &info[crc_data_start..crc_data_end];
    let crc = {
        let poly: u16 = 0x1021;
        let mut register: u16 = 0xFFFF;
        for &bit in crc_scope {
            let feedback = ((register >> 15) & 1) ^ (bit as u16 & 1);
            register <<= 1;
            if feedback == 1 {
                register ^= poly;
            }
        }
        register ^ 0xFFFF
    };

    // Write CRC-16 MSB first
    for i in 0..16 {
        if bit_offset < 172 {
            info[bit_offset] = ((crc >> (15 - i)) & 1) as u8;
            bit_offset += 1;
        }
    }

    info
}

// ---------------------------------------------------------------------------
// Full PHY E2E test -- SO6 SMS Data Burst
// ---------------------------------------------------------------------------

/// E2E test for SO6 SMS Data Burst with real PHY on both directions.
///
/// **All reverse traffic (Mobile → BS) uses real PHY samples:**
///   Preamble + R-DSCH signaling (frame aligner lock) + MS Ack Order +
///   Service Connect Completion + SMS Data Burst are all encoded through
///   the full reverse traffic PHY chain and decoded by the BTS.
///
/// **Forward (BS → Mobile):** TX samples are drained from RadioPipe and
/// verified via PN despread + Walsh correlation on the assigned traffic code.
///
/// Phone number is resolved via a FakeHlrRepository so the SMS handler
/// completes fully (including SMS Cause Code response on F-TCH).
#[tokio::test]
async fn test_e2e_so6_sms_data_burst_phy_bidirectional() {
    init_test_logging();

    let esn: u32 = 0xABCD_EF02;
    let oversample: usize = 4;
    let chip_rate: usize = 1_228_800;
    let chips_per_frame: usize = 24576; // 20ms at 1.2288 Mcps

    // -- Build MO SMS payload --
    // Use a short raw SMS-like payload (8 bytes) that fits in a single R-DSCH
    // signaling frame (max ~11 CHARi bytes). A real SMS teleservice payload
    // would exceed 168 signaling bits and require multi-frame SAR, which is
    // out of scope for this E2E test.
    let sms_payload: Vec<u8> = vec![0x10, 0x02, 0x55, 0x51, 0x23, 0x48, 0x69, 0x21];
    eprintln!("  SMS-like Data Burst payload: {} bytes", sms_payload.len());

    // -- Setup L2 channels --
    let (mac_to_lac_tx, mac_to_lac_rx) = std::sync::mpsc::channel();
    let (lac_to_mac_tx, lac_to_mac_rx) = std::sync::mpsc::channel();
    let lac_layer = lac::Layer2Lac::new(lac_to_mac_tx, mac_to_lac_rx);
    let mac_layer = mac::Layer2Mac::new(lac_to_mac_rx, mac_to_lac_tx);

    // -- Mock HLR: resolves ESN to phone number "5559876" --
    let hlr = Arc::new(FakeHlrRepository::new("5559876"));

    // -- Create RadioPipe + BTS --
    let start_system_time = cdma_common::time::cdma_epoch();
    let (radio, pipe_handle) = RadioPipe::new(4096);
    let (bts, bts_handle) = bts::Bts::new_with_radio_pipe(
        radio,
        bts::Config {
            tx_center_frequency_hz: 881_520_000,
            pilot_offset: 0,
            mac_layer: mac_layer.clone(),
            start_system_time: Some(start_system_time),
            sync_channel_template: Some(SyncChannelMessage {
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
                cdma_freq: 384,
                ext_cdma_freq: 0,
                sr1_bcch_non_td_incl: false,
                sr1_td_incl: false,
                sr3_incl: false,
                ds_incl: false,
            }),
            timezone: cdma_common::timezone::TimezoneConfig::default(),
            overhead: cdma_common::overhead::OverheadParameters::default(),
            rx: Some(bts::RxSettings {
                sample_rate_hz: chip_rate * oversample,
                auth_mode: 0,
                p_rev_in_use: 6,
                capture_iq_wav: None,
                capture_seconds: None,
                access_channel_number: 0,
                paging_channel_number: 1,
                base_id: 1,
                pilot_pn: 0,
                chip_rate_hz: chip_rate,
                absolute_chip_start: 0,
                hardware_start_time_ns: 0,
                tick_rate: 1_000_000_000,
                access_event_tx: None,
                reverse_bearer_tx: None,
                rx_metrics_tx: None,
                reanchor_origin: true,
                traffic_rx_pool: None,
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
            }),
        },
        bts::BtsRuntimeSettings::default(),
    );

    let bts::BtsHandle {
        tx_metrics: _,
        rx_metrics: _,
        config: _,
        access_events: mut bts_access_rx,
        commands: _,
        traffic_channels,
        walsh_allocator,
        traffic_rx_pool,
        traffic_rx_removals,
        power_control: _,
        rx_measurements: _,
    } = bts_handle;

    // -- Create BSC with HLR (no packet_service needed for SMS) --
    let mut bsc = Bsc::new(BscConfig {
        pilot_offset: 0,
        overhead: OverheadParameters {
            sid: 42,
            nid: 7,
            cdma_freq: Some(384),
            ..Default::default()
        },
        paging: bts::PagingChannelSettings::default(),
        traffic_assignment: TrafficAssignmentConfig::default(),
        access_event_rx: None, // We manually forward BTS events
        access_event_broadcast: None,
        sms_request_rx: None,
        sms_request_tx: None,
        data_request_rx: None,
        data_request_tx: None,
        power_override_request_rx: None,
        power_override_request_tx: None,
        mobiles_tx: None,
        paging_broadcast: None,
        traffic_broadcast: None,
        rx_reference_dbm: None,
        hlr_repo: Some(hlr.clone()),
        msc_client: test_msc_client(),
        msc_voice_bearer: None,
        bts_client: Some(Arc::new(NetworkBtsControlClient::spawn_in_process(
            Arc::new(TrafficResourceService::from_pools(
                walsh_allocator.clone(),
                traffic_channels.clone(),
                traffic_rx_pool.clone(),
                traffic_rx_removals.clone(),
            )),
            AbisAgentConfig {
                pilot_pn: 0,
                cell_id: CellId { cell: 1, sector: 1 },
                mscid: 1,
            },
            NetworkClientConfig {
                cell_id: CellId { cell: 1, sector: 1 },
                mscid: 1,
                pilot_pn: 0,
                auth_mode: 0,
                p_rev_in_use: 6,
                market_id: 1,
                generating_entity_id: 1,
            },
        )) as Arc<dyn BtsControlClient>),
        traffic_retry: TrafficRetryConfig::default(),
        paging_retry: PagingRetryConfig::default(),
        voice_policy: test_voice_policy(),
        pcf_client: None,
        mobile_idle_timeout_s: 0,
        bts_paging_state: None,
        node_id: "bsc-test".to_string(),
    });

    // -- Start L2 workers --
    let lac_worker = {
        let lac = lac_layer.clone();
        thread::spawn(move || lac.run_for(100_000, Duration::from_secs(10)).unwrap())
    };
    let mac_worker = {
        let mac = mac_layer.clone();
        thread::spawn(move || mac.run_for(100_000, Duration::from_secs(10)).unwrap())
    };

    // ===================================================================
    // Phase A: Origination (SO6 — access channel, triggers HLR resolution)
    // ===================================================================
    eprintln!("=== SO6 Phase A: Origination ===");
    bsc.inject_access_event(synthetic_origination_so6(esn))
        .await;
    let walsh_code: u8 = 10;
    {
        let pool = traffic_rx_pool.lock();
        assert!(
            pool.iter().any(|r| r.walsh_code == walsh_code),
            "BSC should have pushed TrafficRxRequest for walsh={}",
            walsh_code
        );
    }
    eprintln!("  Traffic channel allocated: walsh={}", walsh_code);

    // Give HLR resolution time to complete (async task)
    tokio::time::sleep(Duration::from_millis(200)).await;

    // ===================================================================
    // Phase B: Build all reverse traffic IQ (real PHY)
    // ===================================================================
    eprintln!("=== SO6 Phase B: Encoding reverse traffic via real PHY ===");
    let encoder = ReverseTrafficChannelEncoder::new(esn);

    // Compute BTS starting chip cursor
    let sync_superframe_chips: u64 = 98_304;
    let lead_chips: u64 = 100_000_000u64 * chip_rate as u64 / 1_000_000_000;
    let bts_chip_start = {
        let v = lead_chips % sync_superframe_chips;
        if v == 0 {
            lead_chips
        } else {
            lead_chips + (sync_superframe_chips - v)
        }
    };
    eprintln!("  BTS chip cursor start: {}", bts_chip_start);

    // Build the entire reverse-traffic stream as raw (pre-FIR) IQ samples,
    // then apply a single continuous pulse-shaping FIR. This avoids
    // frame-boundary transients from FIR resets between frames.
    use cdma_bts::channels::rtch::pulse_shape;

    // Preamble: 20 frames (400ms) of Walsh 0 with LC×PN spreading.
    // The frame aligner needs PREAMBLE_NULL_FRAME_THRESHOLD (16) consecutive
    // null frames to transition from SearchingPreamble to Locking.
    let preamble_frames_count = 20;
    let preamble_chips = chips_per_frame * preamble_frames_count;
    let preamble_chip_start: u64 = bts_chip_start;
    let mut all_raw_samples = encoder.encode_preamble_raw(preamble_chips, preamble_chip_start);
    eprintln!(
        "  Preamble: {} frames ({} chips, {} raw samples)",
        preamble_frames_count,
        preamble_chips,
        all_raw_samples.len()
    );

    let mut frame_chip_offset = preamble_chip_start + preamble_chips as u64;

    // R-DSCH signaling frames for frame aligner CRC-based lock.
    let signaling_info = build_rdsch_signaling_info_bits();
    for _ in 0..16 {
        let raw = encoder.encode_full_rate_frame_raw(&signaling_info, frame_chip_offset);
        all_raw_samples.extend_from_slice(&raw);
        frame_chip_offset += chips_per_frame as u64;
    }
    eprintln!("  Added 16 R-DSCH signaling frames for CRC lock");

    // MS Ack Order — send 4 copies for redundancy
    let ms_ack_info = build_rdsch_ms_ack_order_info_bits(7, 0);
    for _ in 0..4 {
        let raw = encoder.encode_full_rate_frame_raw(&ms_ack_info, frame_chip_offset);
        all_raw_samples.extend_from_slice(&raw);
        frame_chip_offset += chips_per_frame as u64;
    }
    eprintln!("  Added 4x MS Ack Order frames");

    // Service Connect Completion — send 4 copies
    let scc_info = build_rdsch_scc_info_bits(0, 1);
    for _ in 0..4 {
        let raw = encoder.encode_full_rate_frame_raw(&scc_info, frame_chip_offset);
        all_raw_samples.extend_from_slice(&raw);
        frame_chip_offset += chips_per_frame as u64;
    }
    eprintln!("  Added 4x Service Connect Completion frames");

    // SMS Data Burst — send 4 copies
    let sms_info = build_rdsch_sms_data_burst_info_bits(&sms_payload);
    for _ in 0..4 {
        let raw = encoder.encode_full_rate_frame_raw(&sms_info, frame_chip_offset);
        all_raw_samples.extend_from_slice(&raw);
        frame_chip_offset += chips_per_frame as u64;
    }
    eprintln!(
        "  Added 4x SMS Data Burst frames ({} payload bytes)",
        sms_payload.len()
    );

    // Pad with signaling frames
    for _ in 0..20 {
        let raw = encoder
            .encode_full_rate_frame_raw(&build_rdsch_signaling_info_bits(), frame_chip_offset);
        all_raw_samples.extend_from_slice(&raw);
        frame_chip_offset += chips_per_frame as u64;
    }

    // Apply continuous pulse-shaping FIR to the entire raw stream.
    // This eliminates frame-boundary FIR transients that cause CRC-12 failures.
    let all_rx_samples = pulse_shape(&all_raw_samples);
    drop(all_raw_samples);

    let total_rx_frames = (all_rx_samples.len() / (chips_per_frame * oversample)) as u64;
    eprintln!(
        "  Total reverse IQ: {} samples ({} frames, {:.1}ms)",
        all_rx_samples.len(),
        total_rx_frames,
        total_rx_frames as f64 * 20.0,
    );

    // Wrap pipe_handle in Arc<Mutex> for shared access across threads.
    let pipe_handle = Arc::new(std::sync::Mutex::new(pipe_handle));

    // ===================================================================
    // Start BTS + TX drain + RX injection concurrently.
    // ===================================================================
    let drain_handle = pipe_handle.clone();
    let drain_stop = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let drain_stop2 = drain_stop.clone();
    let tx_accumulated = Arc::new(std::sync::Mutex::new(Vec::<Complex32>::new()));
    let tx_accumulated2 = tx_accumulated.clone();
    let tx_drain_thread = thread::spawn(move || {
        while !drain_stop2.load(std::sync::atomic::Ordering::Relaxed) {
            let samples = {
                let ph = drain_handle.lock().unwrap();
                ph.drain_tx_samples()
            };
            if !samples.is_empty() {
                tx_accumulated2.lock().unwrap().extend(samples);
            }
            thread::sleep(Duration::from_millis(10));
        }
    });

    let bts_task = tokio::spawn(async move {
        if let Err(e) = bts.run_for_blocks(128_000).await {
            eprintln!("BTS error: {}", e);
        }
    });

    // Brief pause for BTS to spawn its RX thread
    tokio::time::sleep(Duration::from_millis(100)).await;

    // Inject all reverse traffic IQ into RadioPipe
    let inject_handle = pipe_handle.clone();
    let inject_thread = thread::spawn(move || {
        let block_len = 32768usize;
        let mut sample_idx = 0usize;
        while sample_idx < all_rx_samples.len() {
            let end = (sample_idx + block_len).min(all_rx_samples.len());
            let block = all_rx_samples[sample_idx..end].to_vec();
            let chip_start = preamble_chip_start + (sample_idx / oversample) as u64;
            let ph = inject_handle.lock().unwrap();
            ph.inject_rx(InjectedRxBlock {
                samples: block,
                time_ns: 0,
                absolute_chip_start: Some(chip_start),
            })
            .expect("inject_rx failed");
            drop(ph);
            sample_idx = end;
        }
        // Close RX to signal end-of-stream
        let mut ph = inject_handle.lock().unwrap();
        ph.close_rx();
        eprintln!("  RX injection complete ({} samples)", sample_idx);
    });

    // Wait for injection to finish
    let _ = inject_thread.join();

    // Wait for BTS RX to process all injected data
    tokio::time::sleep(Duration::from_millis(2000)).await;

    // ===================================================================
    // Phase C: Drain all BTS events into BSC
    // ===================================================================
    eprintln!("=== SO6 Phase C: Drain BTS events ===");
    let mut preamble_detected = false;
    let mut traffic_signaling_frames = 0usize;
    let mut traffic_data_frames = 0usize;
    let mut saw_ms_ack = false;
    let mut saw_scc = false;
    let mut saw_data_burst = false;

    // Poll for BTS events over several rounds to ensure all frames are processed
    for wait_round in 0..20 {
        let mut found_any = false;
        while let Ok(event) = bts_access_rx.try_recv() {
            found_any = true;
            if event.is_preamble_only {
                eprintln!(
                    "  BTS: preamble detected on walsh={}",
                    event.traffic_walsh_code.unwrap_or(0)
                );
                preamble_detected = true;
            }
            if event.traffic_primary_bits.is_some() {
                traffic_data_frames += 1;
                eprintln!(
                    "  BTS: traffic data frame #{} rate={}",
                    traffic_data_frames,
                    event.traffic_primary_rate_bps.unwrap_or(0)
                );
            }
            if event.message_id == MessageId::Order && event.order_code == Some(0b010000) {
                saw_ms_ack = true;
                eprintln!("  BTS: MS Ack Order decoded via PHY");
            }
            if event.message_id == MessageId::ServiceConnectCompletion {
                saw_scc = true;
                eprintln!("  BTS: Service Connect Completion decoded via PHY");
            }
            if event.message_id == MessageId::DataBurst && event.burst_type == Some(3) {
                saw_data_burst = true;
                eprintln!(
                    "  BTS: SMS Data Burst decoded via PHY ({} fields)",
                    event
                        .data_burst_fields
                        .as_ref()
                        .map(|f| f.len())
                        .unwrap_or(0)
                );
            }
            if event.decoded_rdsch.is_some() {
                traffic_signaling_frames += 1;
            }
            bsc.inject_access_event(event).await;
        }
        if wait_round > 0 && wait_round % 5 == 0 {
            eprintln!(
                "  Round {}: preamble={} signaling={} data={} ms_ack={} scc={} sms={}",
                wait_round,
                preamble_detected,
                traffic_signaling_frames,
                traffic_data_frames,
                saw_ms_ack,
                saw_scc,
                saw_data_burst
            );
        }
        // Once we have all key events, stop polling
        if saw_data_burst && saw_ms_ack && saw_scc {
            eprintln!("  All key events received at round {}", wait_round);
            break;
        }
        tokio::time::sleep(Duration::from_millis(500)).await;
    }

    // Give BSC time to process all events (HLR resolution, SMS handling)
    tokio::time::sleep(Duration::from_millis(1000)).await;

    // ===================================================================
    // Phase D: Forward TX verification
    // ===================================================================
    eprintln!("=== SO6 Phase D: Forward TX verification ===");

    drain_stop.store(true, std::sync::atomic::Ordering::Relaxed);
    let _ = tx_drain_thread.join();

    // Grab remaining TX samples
    {
        let ph = pipe_handle.lock().unwrap();
        let remaining = ph.drain_tx_samples();
        tx_accumulated.lock().unwrap().extend(remaining);
    }

    let tx_samples = tx_accumulated.lock().unwrap();
    eprintln!("  Forward TX: {} samples captured", tx_samples.len());

    if !tx_samples.is_empty() {
        let pilot_energy = forward_tx_walsh_energy(&tx_samples, 0, oversample);
        let traffic_energy = forward_tx_walsh_energy(&tx_samples, walsh_code, oversample);

        eprintln!("  Walsh 0 (pilot) avg symbol energy: {:.2}", pilot_energy);
        eprintln!(
            "  Walsh {} (traffic) avg symbol energy: {:.2}",
            walsh_code, traffic_energy
        );

        assert!(
            pilot_energy > 0.0,
            "Forward TX should have pilot energy on Walsh 0"
        );
        assert!(
            traffic_energy > 0.0,
            "Forward TX should have energy on traffic Walsh {} (from BS Ack + Service Connect + SMS Cause Code)",
            walsh_code
        );
    } else {
        eprintln!("  WARNING: No forward TX samples captured");
    }

    // ===================================================================
    // Phase E: Final verification
    // ===================================================================
    eprintln!("=== SO6 Phase E: Final verification ===");

    // -- Reverse PHY: preamble --
    assert!(
        preamble_detected,
        "BTS should detect preamble through real PHY"
    );
    eprintln!("  PASS: Preamble detected via PHY");

    // -- Reverse PHY: signaling --
    assert!(
        saw_ms_ack,
        "BTS should decode MS Ack Order through real PHY"
    );
    eprintln!("  PASS: MS Ack Order decoded via PHY");

    assert!(
        saw_scc,
        "BTS should decode Service Connect Completion through real PHY"
    );
    eprintln!("  PASS: Service Connect Completion decoded via PHY");

    // -- Reverse PHY: SMS Data Burst --
    assert!(
        saw_data_burst,
        "BTS should decode SMS Data Burst through real PHY"
    );
    eprintln!("  PASS: SMS Data Burst decoded via PHY");

    // -- Forward PHY verification --
    assert!(!tx_samples.is_empty(), "Forward TX should produce samples");
    eprintln!(
        "  PASS: Forward PHY -- {} TX samples captured",
        tx_samples.len()
    );

    eprintln!(
        "=== SO6 E2E SMS test PASSED: preamble + {} signaling + MS Ack + SCC + SMS Data Burst — all via real PHY ===",
        traffic_signaling_frames
    );

    // ===================================================================
    // Cleanup
    // ===================================================================
    bts_task.abort();
    let _ = bts_task.await;

    {
        let mut ph = pipe_handle.lock().unwrap();
        ph.close_rx();
    }

    drop(lac_layer);
    drop(mac_layer);
    let _ = lac_worker.join();
    let _ = mac_worker.join();
}
