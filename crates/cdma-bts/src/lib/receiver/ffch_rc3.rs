use num::complex::Complex32;

use cdma_common::bits::Bitstream;
use cdma_common::channel::TrafficRate;
use cdma_common::crc::{crc6, crc8, crc12, crc16_ccitt};

use crate::phy::coding::block_interleaver::{
    ForwardBackwardsBitReversalInterleaver, SR1_PARAMS_768,
};
use crate::phy::coding::convolutional::{get_1_4_k9_encoder, get_1_4_k9_soft_viterbi_decoder};
use crate::phy::coding::long_code::LongCodeGenerator;
use crate::phy::walsh::WalshGenerator;
use crate::receiver::access_layer3::FdschPdu;

pub const FFCH_RC3_FRAME_CHIPS: u64 = 24_576;

const MOD_SYMBOLS_PER_FRAME: usize = 768;
const OUTPUT_SYMBOLS_PER_FRAME: usize = MOD_SYMBOLS_PER_FRAME / 2;
const LC_DECIMATION: usize = 32;
const PCGS_PER_FRAME: usize = 16;
const SYMBOLS_PER_PCG: usize = 48;
const PCG_CHIPS: usize = SYMBOLS_PER_PCG * LC_DECIMATION;
const PC_PUNCTURE_SYMBOLS: usize = 4;
const LONG_CODE_PERIOD: u64 = (1u64 << 42) - 1;
const FULL_RATE_INFO_BITS: usize = 172;
/// MuxPDU Type 1 blank-and-burst signaling prefix: MM=1, TT=0, TM=11, SOM=1.
const SIGNALING_PREFIX: [u8; 5] = [1, 0, 1, 1, 1];
/// SAR PDU framing: an 8-bit MSG_LENGTH in octets, which counts itself and the
/// trailing CRC-16.
const SAR_MSG_LENGTH_BITS: usize = 8;
const SAR_CRC_BITS: usize = 16;

#[derive(Debug, Clone)]
pub struct ForwardRc3Frame {
    pub info_bits: Vec<u8>,
    pub rate: TrafficRate,
    pub fqi_valid: bool,
    /// The reassembled f-dsch PDU, present when this is a CRC-16-valid
    /// single-frame signaling message.
    pub fdsch: Option<FdschPdu>,
}

/// De-cover the assigned W(n,64) Walsh from 64-chip chunks into `384` complex
/// QPSK modulation symbols. `invert_q` conjugates the quadrature axis to match
/// the receiver's forward pilot-reference convention.
pub fn dewalsh_forward_rc3(
    chip_samples: &[Complex32],
    walsh_code: u8,
    invert_q: bool,
) -> Vec<Complex32> {
    let walsh_row = WalshGenerator::generate_matrix::<64>()[walsh_code as usize];
    chip_samples
        .chunks_exact(64)
        .take(OUTPUT_SYMBOLS_PER_FRAME)
        .map(|chunk| {
            let sym = chunk
                .iter()
                .enumerate()
                .fold(Complex32::new(0.0, 0.0), |acc, (i, sample)| {
                    acc + *sample * walsh_row[i] as f32
                });
            if invert_q {
                Complex32::new(sym.re, -sym.im)
            } else {
                sym
            }
        })
        .collect()
}

/// Forward-power-control puncture positions (one per PCG), derived from bits
/// {44..47} of the preceding PCG's decimated long-code outputs (C.S0002-E Table
/// 3.1.3.1.12-1, RC3 non-TD). `lc_at_frame` is positioned at the frame's first
/// chip.
fn pc_positions(mut lc_at_frame: LongCodeGenerator) -> [usize; PCGS_PER_FRAME] {
    lc_at_frame.advance_chips((LONG_CODE_PERIOD - PCG_CHIPS as u64) as usize);
    let mut lc_decimated = [0u8; MOD_SYMBOLS_PER_FRAME];
    for bit in &mut lc_decimated {
        *bit = lc_at_frame.next_chip();
        for _ in 1..LC_DECIMATION {
            lc_at_frame.next_chip();
        }
    }

    let mut positions = [0usize; PCGS_PER_FRAME];
    for (pcg, pos) in positions.iter_mut().enumerate() {
        let base = pcg * SYMBOLS_PER_PCG;
        let b3 = lc_decimated[base + 47] as usize;
        let b2 = lc_decimated[base + 46] as usize;
        let b1 = lc_decimated[base + 45] as usize;
        let b0 = lc_decimated[base + 44] as usize;
        *pos = ((b3 << 3) | (b2 << 2) | (b1 << 1) | b0) * 2;
    }
    positions
}

/// Descramble + PC-erase + de-interleave 384 QPSK symbols into `768` pre-Viterbi
/// soft values. `lc_prev` is the traffic long code positioned at the chip just
/// before the frame (`frame_start - 1`), supplying the Q-lane carry-in.
fn pre_viterbi_softs(qpsk_symbols: &[Complex32], mut lc_prev: LongCodeGenerator) -> Vec<f32> {
    let mut soft = Vec::with_capacity(MOD_SYMBOLS_PER_FRAME);
    for symbol in qpsk_symbols {
        soft.push((1.0 - symbol.re) * 0.5);
        soft.push((1.0 - symbol.im) * 0.5);
    }

    let mut lc_at_frame = lc_prev.clone();
    lc_at_frame.next_chip();
    let positions = pc_positions(lc_at_frame);

    // In each 64-chip pair, I uses the first chip and Q the preceding chip, including across frames.
    let mut held_prev = lc_prev.next_chip();
    let mut i_chips = [0u8; OUTPUT_SYMBOLS_PER_FRAME];
    let mut q_chips = [0u8; OUTPUT_SYMBOLS_PER_FRAME];
    for k in 0..OUTPUT_SYMBOLS_PER_FRAME {
        let start = lc_prev.next_chip();
        i_chips[k] = start;
        q_chips[k] = held_prev;
        held_prev = start;
        for _ in 0..(2 * LC_DECIMATION - 1) {
            held_prev = lc_prev.next_chip();
        }
    }

    let descrambled: Vec<f32> = soft
        .into_iter()
        .enumerate()
        .map(|(idx, value)| {
            let pcg = idx / SYMBOLS_PER_PCG;
            let symbol_in_pcg = idx % SYMBOLS_PER_PCG;
            let pc_start = positions[pcg];
            if symbol_in_pcg >= pc_start && symbol_in_pcg < pc_start + PC_PUNCTURE_SYMBOLS {
                return 0.5;
            }
            let k = idx / 2;
            let lc = if idx % 2 == 0 { i_chips[k] } else { q_chips[k] };
            if lc == 0 { value } else { 1.0 - value }
        })
        .collect();

    ForwardBackwardsBitReversalInterleaver::new(SR1_PARAMS_768).decode_soft(&descrambled)
}

fn traffic_lc_before_frame(esn: u32, frame_chip_start: u64) -> LongCodeGenerator {
    let previous = if frame_chip_start == 0 {
        LONG_CODE_PERIOD - 1
    } else {
        frame_chip_start - 1
    };
    let mut lc = LongCodeGenerator::new_traffic_channel(esn);
    lc.advance_chips(previous as usize);
    lc
}

pub fn decode_forward_rc3_signaling_frame(
    qpsk_symbols: &[Complex32],
    esn: u32,
    frame_chip_start: u64,
) -> Option<ForwardRc3Frame> {
    decode_with_lc(qpsk_symbols, traffic_lc_before_frame(esn, frame_chip_start))
}

/// Decode one forward RC3 signaling frame when the mobile has recovered the
/// long-code register from the sync channel rather than an absolute chip index.
/// `lc_prev_state` is the 42-bit long-code register at the chip before the frame
/// (`frame_start - 1`), and `esn` selects the traffic long-code mask.
pub fn decode_forward_rc3_signaling_frame_from_lc_state(
    qpsk_symbols: &[Complex32],
    esn: u32,
    lc_prev_state: u64,
) -> Option<ForwardRc3Frame> {
    let mut lc = LongCodeGenerator::new_traffic_channel(esn);
    lc.set_state(lc_prev_state);
    decode_with_lc(qpsk_symbols, lc)
}

fn decode_with_lc(
    qpsk_symbols: &[Complex32],
    lc_prev: LongCodeGenerator,
) -> Option<ForwardRc3Frame> {
    if qpsk_symbols.len() != OUTPUT_SYMBOLS_PER_FRAME {
        return None;
    }

    let deinterleaved = pre_viterbi_softs(qpsk_symbols, lc_prev);

    let peak = deinterleaved
        .iter()
        .map(|v| (0.5 - *v).abs())
        .fold(0.0f32, f32::max);
    let inv_peak = if peak > 1e-12 { 1.0 / peak } else { 1.0 };
    let normalized: Vec<f32> = deinterleaved
        .iter()
        .map(|value| (value - 0.5) * inv_peak + 0.5)
        .collect();
    let mut best = None;
    for rate in [
        TrafficRate::Full,
        TrafficRate::Half,
        TrafficRate::Quarter,
        TrafficRate::Eighth,
    ] {
        let repeated_len = rate.rc3_frame_bits() * 4 * rate.rc3_repeat_factor();
        let unpunctured = if repeated_len == MOD_SYMBOLS_PER_FRAME {
            normalized.clone()
        } else {
            let mut expanded = vec![0.5; repeated_len];
            for (output_index, value) in normalized.iter().enumerate() {
                let input_index = output_index * repeated_len / MOD_SYMBOLS_PER_FRAME;
                expanded[input_index] = *value;
            }
            expanded
        };
        let repeat = rate.rc3_repeat_factor();
        let derepeated: Vec<f32> = unpunctured
            .chunks_exact(repeat)
            .map(|chunk| chunk.iter().sum::<f32>() / repeat as f32)
            .collect();
        let metrics: Vec<[f32; 4]> = derepeated
            .chunks_exact(4)
            .map(|chunk| [chunk[0], chunk[1], chunk[2], chunk[3]])
            .collect();
        let decoded = get_1_4_k9_soft_viterbi_decoder().decode_block_from_state(&metrics, 0);
        if decoded.len() < rate.rc3_frame_bits() {
            continue;
        }
        let info_len = rate.info_bits();
        let fqi_len = rate.rc3_fqi_bits();
        let info_bits = decoded[..info_len].to_vec();
        let expected_crc = match fqi_len {
            12 => crc12(&info_bits),
            8 => u16::from(crc8(&info_bits)),
            6 => u16::from(crc6(&info_bits)),
            _ => continue,
        };
        let observed_crc = decoded[info_len..info_len + fqi_len]
            .iter()
            .fold(0u16, |value, bit| (value << 1) | u16::from(*bit));
        if expected_crc != observed_crc {
            continue;
        }
        let mut encoder = get_1_4_k9_encoder();
        let expected_symbols = decoded[..rate.rc3_frame_bits()]
            .iter()
            .flat_map(|bit| encoder.encode(*bit))
            .collect::<Vec<_>>();
        // Compare every rate before derepetition so averaging cannot hide symbol errors.
        let score = normalized
            .iter()
            .enumerate()
            .map(|(index, observed)| {
                let repeated_index = index * repeated_len / MOD_SYMBOLS_PER_FRAME;
                let expected = expected_symbols[repeated_index / repeat];
                let error = *observed - f32::from(expected);
                error * error
            })
            .sum::<f32>()
            / MOD_SYMBOLS_PER_FRAME as f32;
        let fdsch = (rate == TrafficRate::Full)
            .then(|| decode_signaling_pdu(&info_bits))
            .flatten();
        let frame = ForwardRc3Frame {
            info_bits,
            rate,
            fqi_valid: true,
            fdsch,
        };
        if best
            .as_ref()
            .is_none_or(|(best_score, _): &(f32, ForwardRc3Frame)| score < *best_score)
        {
            best = Some((score, frame));
        }
    }
    if let Some((_, frame)) = best {
        return Some(frame);
    }

    Some(ForwardRc3Frame {
        info_bits: vec![0; FULL_RATE_INFO_BITS],
        rate: TrafficRate::Full,
        fqi_valid: false,
        fdsch: None,
    })
}

fn decode_signaling_pdu(info_bits: &[u8]) -> Option<FdschPdu> {
    let sar_start = SIGNALING_PREFIX.len();
    let body_start = sar_start + SAR_MSG_LENGTH_BITS;
    if info_bits.len() < body_start || info_bits[..sar_start] != SIGNALING_PREFIX {
        return None;
    }
    let msg_length_octets = Bitstream::new_init(&info_bits[sar_start..body_start])
        .read_bits(SAR_MSG_LENGTH_BITS)
        .ok()? as usize;
    let sar_end = sar_start + msg_length_octets * u8::BITS as usize;
    if sar_end > info_bits.len() || sar_end < body_start + SAR_CRC_BITS {
        return None;
    }
    let crc_start = sar_end - SAR_CRC_BITS;
    let expected_crc = crc16_ccitt(&info_bits[sar_start..crc_start]);
    let observed_crc = Bitstream::new_init(&info_bits[crc_start..sar_end])
        .read_bits(SAR_CRC_BITS)
        .ok()? as u16;
    if expected_crc != observed_crc {
        return None;
    }
    FdschPdu::decode(&Bitstream::new_init(&info_bits[body_start..crc_start])).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::channels::ftch_rc3::{
        ConfigRc3, ForwardTrafficChannelRc3, Rc3PcgPcbScheduler, TrafficFrameRc3,
    };
    use crate::lac::{assemble_f_dsch_pdu, sar_fragment_ftch_pdu_dsch};
    use crate::phy::coding::convolutional::get_1_4_k9_encoder;
    use cdma_common::access::{AccessMessage, AccessMessageHeader, DataBurstMessage, FdschMessage};
    use cdma_common::consts::{BURST_TYPE_SMS, RC3_GATED_REV_PWR_CNTL_DELAY};
    use cdma_common::lac::message_types::{MessageId, WireChannel};
    use cdma_common::sms::encode_sms_cause_code;
    use cdma_common::time::CdmaSystemTime;

    #[test]
    fn forward_power_control_symbols_do_not_change_traffic_soft_bits() {
        const ESN: u32 = 0x1234_5678;
        const FRAME_CHIP: usize = FFCH_RC3_FRAME_CHIPS as usize;
        let mut puncture_lc = LongCodeGenerator::new_traffic_channel(ESN);
        puncture_lc.advance_chips(FRAME_CHIP - PCG_CHIPS);
        let mut positive = vec![Complex32::new(1.0, 1.0); OUTPUT_SYMBOLS_PER_FRAME];
        let mut negative = positive.clone();
        for pcg in 0..PCGS_PER_FRAME {
            let start = ForwardTrafficChannelRc3::pc_start_from_pcg_lc(&mut puncture_lc);
            for index in start..start + PC_PUNCTURE_SYMBOLS {
                let symbol = pcg * SYMBOLS_PER_PCG / 2 + index / 2;
                if index % 2 == 0 {
                    positive[symbol].re = 2.0;
                    negative[symbol].re = -2.0;
                } else {
                    positive[symbol].im = 2.0;
                    negative[symbol].im = -2.0;
                }
            }
        }
        let mut lc_prev = LongCodeGenerator::new_traffic_channel(ESN);
        lc_prev.advance_chips(FRAME_CHIP - 1);
        assert_eq!(
            pre_viterbi_softs(&positive, lc_prev.clone()),
            pre_viterbi_softs(&negative, lc_prev),
        );
    }

    #[test]
    fn forward_rc3_sms_cause_code_round_trips() {
        let esn: u32 = 0x4CDC_1D09;
        let lc_start_chip: u64 = 1_792_951_525_063_768;
        let frame_boundary = lc_start_chip.next_multiple_of(FFCH_RC3_FRAME_CHIPS);

        let cause = encode_sms_cause_code(0, 0);
        let dbm = DataBurstMessage {
            header: AccessMessageHeader {
                pd: 0,
                message_id: MessageId::DataBurst,
            },
            msg_number: 1,
            burst_type: BURST_TYPE_SMS,
            num_msgs: 1,
            num_fields: cause.len() as u8,
            fields: cause.clone(),
            remaining_bits: 0,
        };
        let sdu = AccessMessage::DataBurst(dbm).to_sdu().unwrap();
        let wire = MessageId::DataBurst
            .wire_type(WireChannel::ForwardDedicated)
            .unwrap();
        let pdu = assemble_f_dsch_pdu(wire, &sdu, 0, 1, false);
        let frames = sar_fragment_ftch_pdu_dsch(&pdu);
        assert_eq!(frames.len(), 1, "SMS Cause Code fits one full-rate frame");

        let ch = ForwardTrafficChannelRc3::new(ConfigRc3 {
            encoder: get_1_4_k9_encoder(),
            interleaver: ForwardBackwardsBitReversalInterleaver::new(SR1_PARAMS_768),
            scrambling_lc: LongCodeGenerator::new_traffic_channel(esn),
            puncture_lc: LongCodeGenerator::new_traffic_channel(esn),
            lc_chip_cursor: 0,
            pcb_scheduler: Rc3PcgPcbScheduler::new(RC3_GATED_REV_PWR_CNTL_DELAY),
            fpc_subchan_gain_linear: 1.0,
            prev_frame_last_chip: 0,
            previous_pcg_pc_start: 0,
            disable_lc_scrambling: false,
        });
        ch.advance_lc_to_chip(frame_boundary);

        let null_frames = 3usize;
        for _ in 0..null_frames {
            let _ = ch.next(CdmaSystemTime::default());
        }
        ch.send_signaling_bits(frames[0].bits().to_vec());
        let tx_symbols = ch.next(CdmaSystemTime::default());
        assert_eq!(tx_symbols.len(), OUTPUT_SYMBOLS_PER_FRAME);

        let lc_pos = frame_boundary + (null_frames as u64) * FFCH_RC3_FRAME_CHIPS;
        let frame = decode_forward_rc3_signaling_frame(&tx_symbols, esn, lc_pos)
            .expect("valid symbol count");
        assert!(frame.fqi_valid, "FQI CRC must be valid");
        let fdsch = frame.fdsch.expect("signaling PDU reassembles");
        let FdschMessage::DataBurst(m) = fdsch.body else {
            panic!("expected a Data Burst, got {:?}", fdsch.body);
        };
        assert_eq!(m.burst_type, BURST_TYPE_SMS);
        assert_eq!(m.fields, cause);

        let mut reg = LongCodeGenerator::new_traffic_channel(esn);
        reg.advance_chips((lc_pos - 1) as usize);
        let from_state =
            decode_forward_rc3_signaling_frame_from_lc_state(&tx_symbols, esn, reg.state())
                .expect("valid symbol count");
        assert!(from_state.fqi_valid);
        assert_eq!(from_state.info_bits, frame.info_bits);
    }

    #[test]
    fn forward_rc3_full_rate_survives_short_rate_crc_collisions() {
        const ESN: u32 = 0x1234_5678;
        const FRAME_COUNT: u64 = 512;
        let channel = ForwardTrafficChannelRc3::new(ConfigRc3 {
            encoder: get_1_4_k9_encoder(),
            interleaver: ForwardBackwardsBitReversalInterleaver::new(SR1_PARAMS_768),
            scrambling_lc: LongCodeGenerator::new_traffic_channel(ESN),
            puncture_lc: LongCodeGenerator::new_traffic_channel(ESN),
            lc_chip_cursor: 0,
            pcb_scheduler: Rc3PcgPcbScheduler::new(RC3_GATED_REV_PWR_CNTL_DELAY),
            fpc_subchan_gain_linear: 1.0,
            prev_frame_last_chip: 0,
            previous_pcg_pc_start: 0,
            disable_lc_scrambling: false,
        });
        channel.advance_lc_to_chip(FFCH_RC3_FRAME_CHIPS);
        let mut data = LongCodeGenerator::new_traffic_channel(ESN);
        for index in 1..=FRAME_COUNT {
            let mut expected: Vec<_> = (0..FULL_RATE_INFO_BITS).map(|_| data.next_chip()).collect();
            expected[0] = 0;
            channel.send_frame(TrafficFrameRc3 {
                data: expected.clone(),
                rate: TrafficRate::Full,
            });
            let symbols = channel.next(CdmaSystemTime::default());
            let frame =
                decode_forward_rc3_signaling_frame(&symbols, ESN, index * FFCH_RC3_FRAME_CHIPS)
                    .unwrap();
            assert!(frame.fqi_valid, "frame {index}");
            assert_eq!(frame.rate, TrafficRate::Full, "frame {index}");
            assert_eq!(frame.info_bits, expected, "frame {index}");
        }
    }

    #[test]
    fn forward_rc3_detects_every_voice_rate() {
        let esn = 0x4CDC_1D09;
        let frame_chip = FFCH_RC3_FRAME_CHIPS * 100;
        for rate in [
            TrafficRate::Full,
            TrafficRate::Half,
            TrafficRate::Quarter,
            TrafficRate::Eighth,
        ] {
            let channel = ForwardTrafficChannelRc3::new(ConfigRc3 {
                encoder: get_1_4_k9_encoder(),
                interleaver: ForwardBackwardsBitReversalInterleaver::new(SR1_PARAMS_768),
                scrambling_lc: LongCodeGenerator::new_traffic_channel(esn),
                puncture_lc: LongCodeGenerator::new_traffic_channel(esn),
                lc_chip_cursor: 0,
                pcb_scheduler: Rc3PcgPcbScheduler::new(RC3_GATED_REV_PWR_CNTL_DELAY),
                fpc_subchan_gain_linear: 1.0,
                prev_frame_last_chip: 0,
                previous_pcg_pc_start: 0,
                disable_lc_scrambling: false,
            });
            channel.advance_lc_to_chip(frame_chip);
            let mut expected = (0..rate.info_bits())
                .map(|index| (index % 3 == 0) as u8)
                .collect::<Vec<_>>();
            if rate == TrafficRate::Full {
                expected[0] = 0;
            }
            channel.send_frame(TrafficFrameRc3 {
                data: expected.clone(),
                rate,
            });
            let symbols = channel.next(CdmaSystemTime::default());

            let decoded = decode_forward_rc3_signaling_frame(&symbols, esn, frame_chip)
                .expect("valid symbol count");

            assert!(decoded.fqi_valid, "{rate:?} FQI");
            assert_eq!(decoded.rate, rate);
            assert_eq!(decoded.info_bits, expected);
        }
    }
}
