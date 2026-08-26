use std::ffi::OsStr;
use std::path::{Component, Path, PathBuf};

use cdma_bts::channels::{
    Channel, WalshChannel,
    ftch_rc3::{ConfigRc3, ForwardTrafficChannelRc3, Rc3PcgPcbScheduler},
    pilot::ForwardPilotChannel,
};
use cdma_bts::phy::coding::block_interleaver::{
    BitReversalInterleaver, ForwardBackwardsBitReversalInterleaver, SR1_PARAMS_768,
};
use cdma_bts::phy::coding::convolutional::{get_1_4_k9_encoder, get_1_4_k9_soft_viterbi_decoder};
use cdma_bts::phy::coding::long_code::LongCodeGenerator;
use cdma_bts::phy::spread::PnSequence;
use cdma_bts::phy::spread::Spreader;
use cdma_bts::phy::walsh::WalshGenerator;
use cdma_bts::receiver::access_layer3::{FdschMessage, FdschPdu};
use cdma_common::bits::Bitstream;
use cdma_common::consts::RC3_GATED_REV_PWR_CNTL_DELAY;
use cdma_common::error::Error;
use cdma_common::time::CdmaSystemTime;
use hound::{WavReader, WavSpec, WavWriter};
use num::complex::Complex32;
use rustfft::FftPlanner;

const CHIP_RATE: usize = 1_228_800;
const FRAME_CHIPS: usize = 24_576;
const MOD_SYMBOLS_PER_FRAME: usize = 768;
const OUTPUT_SYMBOLS_PER_FRAME: usize = MOD_SYMBOLS_PER_FRAME / 2;
const LC_DECIMATION: usize = 32;
const PCGS_PER_FRAME: usize = 16;
const SYMBOLS_PER_PCG: usize = 48;
const PC_PUNCTURE_SYMBOLS: usize = 4;
const DEFAULT_LONG_CODE_MASK: u64 = 0x318A_5B0C_1A6;
const DEFAULT_LONG_CODE_STATE: u64 = 0x2123_4567_89A;
const DEFAULT_WALSH_CODE: u8 = 4;
const LONG_CODE_PERIOD: u64 = (1u64 << 42) - 1;
const EXPECTED_INFO_BITS_LEN: usize = 172;
const EXPECTED_BS_ACK_INFO_BITS: &str = "1011100001000000000011110001000000000010000000000000000110010101\
     1001000000000000000000000000000000000000000000000000000000000000\
     00000000000000000000000000000000000000000000";

fn workspace_fixture_path(relative: impl AsRef<Path>) -> PathBuf {
    let relative = relative.as_ref();
    if relative.is_absolute() || relative.exists() {
        return relative.to_path_buf();
    }

    let manifest_dir = Path::new(env!("CARGO_MANIFEST_DIR"));
    let test_relative = relative
        .components()
        .skip_while(
            |component| !matches!(component, Component::Normal(part) if *part == OsStr::new("test")),
        )
        .collect::<PathBuf>();
    let lookup_relative = if test_relative.as_os_str().is_empty() {
        relative.to_path_buf()
    } else {
        test_relative
    };

    manifest_dir
        .ancestors()
        .map(|ancestor| ancestor.join(&lookup_relative))
        .find(|candidate| candidate.exists())
        .unwrap_or_else(|| manifest_dir.join(lookup_relative))
}

fn test_iq_path(file_name: &str) -> PathBuf {
    workspace_fixture_path(Path::new("test").join("iq").join(file_name))
}

fn test_iq_dir() -> PathBuf {
    workspace_fixture_path(Path::new("test").join("iq"))
}

#[derive(Clone, Copy, Debug)]
enum PnMode {
    RepoConvention,
    ConjugateConvention,
}

#[derive(Clone, Copy, Debug)]
enum SampleTransform {
    Identity,
    Conjugate,
    NegateI,
    NegateQ,
    SwapIq,
    SwapIqNegateI,
    SwapIqNegateQ,
    NegateBoth,
}

#[derive(Clone, Copy, Debug)]
enum LongCodeMode {
    None,
    OnePerModSymbol,
    OddUsesPairStart,
    OddUsesRawPreviousChip,
}

#[derive(Clone, Copy, Debug)]
enum PcMode {
    Disabled,
    ErasurePuncture,
}

#[derive(Clone, Copy, Debug)]
enum InterleaverMode {
    FbbrDecode,
    FbbrEncode,
    BitReverseDecode,
    Identity,
}

#[derive(Clone, Copy, Debug)]
enum TxInterleaverMode {
    FbbrEncode,
    BitReverseEncode,
    Identity,
}

#[derive(Clone, Copy, Debug)]
enum PilotReferenceMode {
    Pn,
    ConjugatePn,
}

#[derive(Debug)]
struct DecodedFrame {
    sample_phase: usize,
    chip_offset: usize,
    lc_chip_offset: u64,
    pn_chip_offset: u64,
    sample_transform: SampleTransform,
    pn_mode: PnMode,
    lc_mode: LongCodeMode,
    interleaver_mode: InterleaverMode,
    pc_mode: PcMode,
    invert_q: bool,
    info_bits: Vec<u8>,
    ftch_crc_ok: bool,
    tail_ok: bool,
}

#[derive(Debug)]
struct DecodeAttempt {
    info_bits: Vec<u8>,
    ftch_crc_ok: bool,
    tail_ok: bool,
    fdsch_crc_ok: bool,
    ack_seq: Option<u8>,
    msg_seq: Option<u8>,
    ack_req: Option<bool>,
    order: Option<u8>,
    use_time: Option<bool>,
    action_time: Option<u8>,
    add_record_len: Option<u8>,
}

#[derive(Debug)]
struct BestCandidate {
    sample_phase: usize,
    chip_offset: usize,
    lc_chip_offset: u64,
    walsh_code: u8,
    pn_chip_offset: u64,
    sample_transform: SampleTransform,
    pn_mode: PnMode,
    lc_mode: LongCodeMode,
    interleaver_mode: InterleaverMode,
    pc_mode: PcMode,
    invert_q: bool,
    mismatch: usize,
    ftch_crc_ok: bool,
    tail_ok: bool,
    fdsch_crc_ok: bool,
    prefix: String,
}

fn load_wav_iq_samples(path: &PathBuf) -> Result<(usize, Vec<Complex32>), Error> {
    let mut reader = WavReader::open(path)?;
    let sample_rate = reader.spec().sample_rate as usize;
    let samples = reader.samples::<i16>().collect::<Result<Vec<_>, _>>()?;
    let iq_samples = samples
        .chunks_exact(2)
        .map(|a| Complex32::new(a[0] as f32 / i16::MAX as f32, a[1] as f32 / i16::MAX as f32))
        .collect::<Vec<_>>();
    Ok((sample_rate, iq_samples))
}

fn traffic_lc_with_state(mask: u64, state: u64) -> LongCodeGenerator {
    let mut lc = LongCodeGenerator::new_traffic_channel_raw_mask(mask);
    lc.set_state(state);
    lc
}

fn quantize_i16(sample: f32) -> i16 {
    (sample.clamp(-1.0, 1.0) * i16::MAX as f32).round() as i16
}

fn write_unfiltered_4x_wav(path: &PathBuf, chip_samples: &[Complex32]) -> Result<(), Error> {
    let spec = WavSpec {
        channels: 2,
        sample_rate: (CHIP_RATE * 4) as u32,
        bits_per_sample: 16,
        sample_format: hound::SampleFormat::Int,
    };
    let mut writer = WavWriter::create(path, spec)?;
    for sample in chip_samples {
        for _ in 0..4 {
            writer.write_sample(quantize_i16(sample.re))?;
            writer.write_sample(quantize_i16(sample.im))?;
        }
    }
    writer.finalize()?;
    Ok(())
}

fn alternating_power_control_bits() -> [u8; PCGS_PER_FRAME] {
    let mut bits = [0u8; PCGS_PER_FRAME];
    for (idx, bit) in bits.iter_mut().enumerate() {
        *bit = (idx % 2) as u8;
    }
    bits
}

fn local_generated_pilot_only_chips() -> Vec<Complex32> {
    let pilot = WalshChannel::new(WalshGenerator::new::<64>(0, 1), ForwardPilotChannel::new());
    let walsh_chips = pilot.next_block(FRAME_CHIPS, CdmaSystemTime::default());
    let mut spreader = Spreader::new(PnSequence::new(0, 32768));
    spreader.align_to_chip(0);
    walsh_chips
        .iter()
        .map(|chip| spreader.spread(chip))
        .collect()
}

fn local_generated_ffch_rc3_bs_ack_pc_on_chips(skip_lc_scrambling: bool) -> Vec<Complex32> {
    let ftch = WalshChannel::new(
        WalshGenerator::new::<64>(DEFAULT_WALSH_CODE as usize, 1),
        ForwardTrafficChannelRc3::new(ConfigRc3 {
            encoder: get_1_4_k9_encoder(),
            interleaver: ForwardBackwardsBitReversalInterleaver::new(SR1_PARAMS_768),
            scrambling_lc: traffic_lc_with_state(DEFAULT_LONG_CODE_MASK, DEFAULT_LONG_CODE_STATE),
            puncture_lc: traffic_lc_with_state(DEFAULT_LONG_CODE_MASK, DEFAULT_LONG_CODE_STATE),
            lc_chip_cursor: 0,
            previous_pcg_pc_start: 0,
            pcb_scheduler: Rc3PcgPcbScheduler::new(RC3_GATED_REV_PWR_CNTL_DELAY),
            fpc_subchan_gain_linear: 1.0,
            prev_frame_last_chip: 0,
            disable_lc_scrambling: skip_lc_scrambling,
        }),
    );
    for (pcg, bit) in alternating_power_control_bits().into_iter().enumerate() {
        ftch.channel.schedule_power_control_bit(pcg as u64, bit);
    }
    ftch.channel
        .send_signaling_bits(parse_bit_string(EXPECTED_BS_ACK_INFO_BITS));
    let walsh_chips = ftch.next_block(FRAME_CHIPS, CdmaSystemTime::default());
    let mut spreader = Spreader::new(PnSequence::new(0, 32768));
    spreader.align_to_chip(0);
    walsh_chips
        .iter()
        .map(|chip| spreader.spread(chip))
        .collect()
}

fn local_generated_wav_pair(
    suffix: &str,
    skip_lc_scrambling: bool,
) -> Result<(PathBuf, PathBuf), Error> {
    let base = std::env::temp_dir();
    let pid = std::process::id();
    let ffch_path = base.join(format!("cdma2000_local_ffch_rc3_{}_{}.wav", suffix, pid));
    let pilot_path = base.join(format!(
        "cdma2000_local_ffch_rc3_{}_pilot_only_{}.wav",
        suffix, pid
    ));
    write_unfiltered_4x_wav(
        &ffch_path,
        &local_generated_ffch_rc3_bs_ack_pc_on_chips(skip_lc_scrambling),
    )?;
    write_unfiltered_4x_wav(&pilot_path, &local_generated_pilot_only_chips())?;
    Ok((ffch_path, pilot_path))
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

fn chip_window_padded(
    chips: &[Complex32],
    chip_offset: usize,
    chip_count: usize,
) -> Vec<Complex32> {
    let mut out = vec![Complex32::new(0.0, 0.0); chip_count];
    if chip_offset >= chips.len() {
        return out;
    }
    let available = (chips.len() - chip_offset).min(chip_count);
    out[..available].copy_from_slice(&chips[chip_offset..chip_offset + available]);
    out
}

fn transform_samples(samples: &[Complex32], mode: SampleTransform) -> Vec<Complex32> {
    samples
        .iter()
        .map(|s| match mode {
            SampleTransform::Identity => *s,
            SampleTransform::Conjugate => Complex32::new(s.re, -s.im),
            SampleTransform::NegateI => Complex32::new(-s.re, s.im),
            SampleTransform::NegateQ => Complex32::new(s.re, -s.im),
            SampleTransform::SwapIq => Complex32::new(s.im, s.re),
            SampleTransform::SwapIqNegateI => Complex32::new(-s.im, s.re),
            SampleTransform::SwapIqNegateQ => Complex32::new(s.im, -s.re),
            SampleTransform::NegateBoth => Complex32::new(-s.re, -s.im),
        })
        .collect()
}

fn parse_bit_string(bits: &str) -> Vec<u8> {
    bits.as_bytes()
        .iter()
        .map(|b| match b {
            b'0' => 0,
            b'1' => 1,
            _ => panic!("invalid bit string"),
        })
        .collect()
}

fn bit_prefix(bits: &[u8], len: usize) -> String {
    bits.iter()
        .take(len)
        .map(|bit| if *bit == 0 { '0' } else { '1' })
        .collect()
}

fn hamming_distance(a: &[u8], b: &[u8]) -> usize {
    a.iter().zip(b.iter()).filter(|(x, y)| x != y).count()
}

fn bro_local(m: usize, val: usize) -> usize {
    let mut result = 0usize;
    for i in 0..m {
        if (val >> i) & 1 == 1 {
            result |= 1 << (m - 1 - i);
        }
    }
    result
}

fn fbbr_encode_soft(block: &[f32]) -> Vec<f32> {
    let params = SR1_PARAMS_768;
    assert_eq!(params.block_size, block.len());
    let mut output = Vec::with_capacity(block.len());
    for i in 0..params.block_size {
        let index = if i % 2 == 0 {
            (2usize.pow(params.m as u32) * ((i / 2) % params.j))
                + bro_local(params.m, (i / 2) / params.j)
        } else {
            (2usize.pow(params.m as u32) * ((params.block_size - ((i + 1) / 2)) % params.j))
                + bro_local(params.m, (params.block_size - ((i + 1) / 2)) / params.j)
        };
        output.push(block[index]);
    }
    output
}

fn apply_interleaver_mode(block: &[f32], mode: InterleaverMode) -> Vec<f32> {
    match mode {
        InterleaverMode::FbbrDecode => {
            let interleaver = ForwardBackwardsBitReversalInterleaver::new(SR1_PARAMS_768);
            interleaver.decode_soft(block)
        }
        InterleaverMode::FbbrEncode => fbbr_encode_soft(block),
        InterleaverMode::BitReverseDecode => {
            let interleaver = BitReversalInterleaver::new(SR1_PARAMS_768);
            interleaver.decode_soft(block)
        }
        InterleaverMode::Identity => block.to_vec(),
    }
}

fn pn_despread(samples: &[Complex32], absolute_chip_start: u64, mode: PnMode) -> Vec<Complex32> {
    let mut pn = PnSequence::new(0, 32768);
    pn.advance_chips(absolute_chip_start);

    samples
        .iter()
        .map(|s| {
            let p = pn.generate_iq();
            match mode {
                PnMode::RepoConvention => {
                    Complex32::new(p.re * s.re - p.im * s.im, p.re * s.im + p.im * s.re)
                }
                PnMode::ConjugateConvention => {
                    Complex32::new(p.re * s.re + p.im * s.im, p.re * s.im - p.im * s.re)
                }
            }
        })
        .collect()
}

fn absolute_chip_start(base_chip_offset: u64, chip_offset: usize) -> u64 {
    base_chip_offset + chip_offset as u64
}

fn pilot_reference_despread(
    samples: &[Complex32],
    pilot_reference: &[Complex32],
) -> Vec<Complex32> {
    samples
        .iter()
        .zip(pilot_reference.iter())
        .map(|(sample, pilot)| {
            let denom = pilot.norm_sqr();
            if denom <= 1e-12 {
                Complex32::new(0.0, 0.0)
            } else {
                *sample * pilot.conj() * (1.0 / denom)
            }
        })
        .collect()
}

fn build_pn_reference(period: usize, mode: PilotReferenceMode) -> Vec<Complex32> {
    let mut pn = PnSequence::new(0, period);
    (0..period)
        .map(|_| {
            let v = pn.generate_iq();
            match mode {
                PilotReferenceMode::Pn => v,
                PilotReferenceMode::ConjugatePn => Complex32::new(v.re, -v.im),
            }
        })
        .collect()
}

fn median(values: &[f32]) -> f32 {
    let mut sorted = values.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mid = sorted.len() / 2;
    if sorted.len() % 2 == 0 {
        (sorted[mid - 1] + sorted[mid]) * 0.5
    } else {
        sorted[mid]
    }
}

fn cyclic_correlation_peak(
    signal: &[Complex32],
    reference_period: &[Complex32],
) -> (usize, f32, f32) {
    assert_eq!(signal.len(), FRAME_CHIPS);
    assert_eq!(reference_period.len(), 32_768);

    let period = reference_period.len();
    let mut planner = FftPlanner::<f32>::new();
    let fft_fwd = planner.plan_fft_forward(period);
    let fft_inv = planner.plan_fft_inverse(period);

    let mut signal_fft = vec![Complex32::new(0.0, 0.0); period];
    signal_fft[..signal.len()].copy_from_slice(signal);
    fft_fwd.process(&mut signal_fft);

    let mut reference_fft = reference_period.to_vec();
    fft_fwd.process(&mut reference_fft);

    let mut corr: Vec<Complex32> = signal_fft
        .iter()
        .zip(reference_fft.iter())
        .map(|(s, r)| *s * r.conj())
        .collect();
    fft_inv.process(&mut corr);

    let scale = 1.0 / period as f32;
    let mags: Vec<f32> = corr
        .iter_mut()
        .map(|c| {
            *c *= scale;
            c.norm()
        })
        .collect();

    let (best_offset, best_mag) = mags
        .iter()
        .copied()
        .enumerate()
        .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
        .unwrap();
    (best_offset, best_mag, median(&mags))
}

fn dewalsh_rc3_symbols(
    chip_samples: &[Complex32],
    walsh_code: u8,
    invert_q: bool,
) -> Vec<Complex32> {
    dewalsh_rc3_symbols_with_phase(chip_samples, walsh_code, invert_q, 0)
}

fn dewalsh_rc3_symbols_with_phase(
    chip_samples: &[Complex32],
    walsh_code: u8,
    invert_q: bool,
    walsh_chip_phase: usize,
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
                    acc + *sample * walsh_row[(i + walsh_chip_phase) % 64] as f32
                });
            if invert_q {
                Complex32::new(sym.re, -sym.im)
            } else {
                sym
            }
        })
        .collect()
}

fn crc12_forward_ftch(bits: &[u8]) -> u16 {
    cdma_common::crc::crc12(bits)
}

fn crc16_fdsch_bits(bits: &[u8]) -> u16 {
    cdma_common::crc::crc16_ccitt(bits)
}

fn build_full_rate_frame_bits(info_bits: &[u8]) -> Vec<u8> {
    assert_eq!(info_bits.len(), 172);
    let mut frame = Vec::with_capacity(192);
    frame.extend_from_slice(info_bits);
    let crc = crc12_forward_ftch(info_bits);
    for bit in (0..12).rev() {
        frame.push(((crc >> bit) & 1) as u8);
    }
    frame.extend_from_slice(&[0u8; 8]);
    frame
}

fn qpsk_symbols_to_soft_symbols(qpsk_symbols: &[Complex32]) -> Vec<f32> {
    let mut soft_symbols = Vec::with_capacity(MOD_SYMBOLS_PER_FRAME);
    for symbol in qpsk_symbols {
        soft_symbols.push((1.0 - symbol.re) * 0.5);
        soft_symbols.push((1.0 - symbol.im) * 0.5);
    }
    soft_symbols
}

fn qpsk_symbols_to_hard_mod_signs(qpsk_symbols: &[Complex32]) -> Vec<i8> {
    let mut signs = Vec::with_capacity(MOD_SYMBOLS_PER_FRAME);
    for symbol in qpsk_symbols {
        signs.push(if symbol.re >= 0.0 { 1 } else { -1 });
        signs.push(if symbol.im >= 0.0 { 1 } else { -1 });
    }
    signs
}

fn qpsk_symbols_to_scalar_values(qpsk_symbols: &[Complex32]) -> Vec<f32> {
    let mut values = Vec::with_capacity(MOD_SYMBOLS_PER_FRAME);
    for symbol in qpsk_symbols {
        values.push(symbol.re);
        values.push(symbol.im);
    }
    values
}

fn rc3_decimated_lc_bits(lc_mask: u64, lc_initial_state: u64, frame_chip_start: u64) -> Vec<u8> {
    let mut lc = traffic_lc_with_state(lc_mask, lc_initial_state);
    lc.advance_chips(frame_chip_start as usize);
    let mut lc_decimated = vec![0u8; MOD_SYMBOLS_PER_FRAME];
    for bit in &mut lc_decimated {
        *bit = lc.next_chip();
        for _ in 1..LC_DECIMATION {
            lc.next_chip();
        }
    }
    lc_decimated
}

fn rc3_pc_positions(
    lc_mask: u64,
    lc_initial_state: u64,
    frame_chip_start: u64,
) -> [usize; PCGS_PER_FRAME] {
    let lc_decimated = rc3_decimated_lc_bits(lc_mask, lc_initial_state, frame_chip_start);
    let mut pc_positions = [0usize; PCGS_PER_FRAME];
    for pcg in 0..PCGS_PER_FRAME {
        let base = pcg * SYMBOLS_PER_PCG;
        let b3 = lc_decimated[base + 47] as usize;
        let b2 = lc_decimated[base + 46] as usize;
        let b1 = lc_decimated[base + 45] as usize;
        let b0 = lc_decimated[base + 44] as usize;
        pc_positions[pcg] = ((b3 << 3) | (b2 << 2) | (b1 << 1) | b0) * 2;
    }
    pc_positions
}

fn rc3_pipeline_pc_positions(
    lc_mask: u64,
    lc_initial_state: u64,
    frame_chip_start: u64,
) -> [usize; PCGS_PER_FRAME] {
    let current = rc3_pc_positions(lc_mask, lc_initial_state, frame_chip_start);
    let mut pipelined = [0usize; PCGS_PER_FRAME];
    if frame_chip_start >= (SYMBOLS_PER_PCG * LC_DECIMATION) as u64 {
        pipelined[0] = rc3_pc_positions(
            lc_mask,
            lc_initial_state,
            frame_chip_start - (SYMBOLS_PER_PCG * LC_DECIMATION) as u64,
        )[0];
    }
    pipelined[1..].copy_from_slice(&current[..PCGS_PER_FRAME - 1]);
    pipelined
}

fn rc3_scalar_lc_descramble(
    scalar_values: &[f32],
    lc_mask: u64,
    lc_initial_state: u64,
    frame_chip_start: u64,
    lc_mode: LongCodeMode,
) -> Vec<f32> {
    let lc_decimated = rc3_decimated_lc_bits(lc_mask, lc_initial_state, frame_chip_start);

    let previous_chip_start = if frame_chip_start == 0 {
        LONG_CODE_PERIOD - 1
    } else {
        frame_chip_start - 1
    };
    let mut lc_pair = traffic_lc_with_state(lc_mask, lc_initial_state);
    lc_pair.advance_chips(previous_chip_start as usize);
    let mut previous_chip = lc_pair.next_chip();
    let mut lc_pair_start = [0u8; OUTPUT_SYMBOLS_PER_FRAME];
    let mut lc_pair_previous = [0u8; OUTPUT_SYMBOLS_PER_FRAME];
    for pair_idx in 0..OUTPUT_SYMBOLS_PER_FRAME {
        lc_pair_previous[pair_idx] = previous_chip;
        let start_chip = lc_pair.next_chip();
        lc_pair_start[pair_idx] = start_chip;
        previous_chip = start_chip;
        for _ in 0..((2 * LC_DECIMATION) - 1) {
            previous_chip = lc_pair.next_chip();
        }
    }

    scalar_values
        .iter()
        .enumerate()
        .map(|(idx, value)| {
            let lc_scr = match lc_mode {
                LongCodeMode::None => 0,
                LongCodeMode::OnePerModSymbol => lc_decimated[idx],
                LongCodeMode::OddUsesPairStart => {
                    if idx % 2 == 0 {
                        lc_decimated[idx]
                    } else {
                        lc_decimated[idx - 1]
                    }
                }
                LongCodeMode::OddUsesRawPreviousChip => {
                    let pair_idx = idx / 2;
                    if idx % 2 == 0 {
                        lc_pair_start[pair_idx]
                    } else {
                        lc_pair_previous[pair_idx]
                    }
                }
            };
            if lc_scr == 0 { *value } else { -*value }
        })
        .collect()
}

fn extract_pcbs_from_scalar_values_sum(
    scalar_values: &[f32],
    pc_positions: &[usize; PCGS_PER_FRAME],
) -> [u8; PCGS_PER_FRAME] {
    let mut bits = [0u8; PCGS_PER_FRAME];
    for (pcg, start) in pc_positions.iter().enumerate() {
        let metric = (0..PC_PUNCTURE_SYMBOLS)
            .map(|k| scalar_values[(pcg * SYMBOLS_PER_PCG) + start + k])
            .sum::<f32>();
        bits[pcg] = if metric >= 0.0 { 0 } else { 1 };
    }
    bits
}

fn extract_pcbs_from_scalar_values_majority(
    scalar_values: &[f32],
    pc_positions: &[usize; PCGS_PER_FRAME],
) -> [u8; PCGS_PER_FRAME] {
    let mut bits = [0u8; PCGS_PER_FRAME];
    for (pcg, start) in pc_positions.iter().enumerate() {
        let ones = (0..PC_PUNCTURE_SYMBOLS)
            .map(|k| scalar_values[(pcg * SYMBOLS_PER_PCG) + start + k])
            .filter(|v| *v < 0.0)
            .count();
        bits[pcg] = if ones >= 2 { 1 } else { 0 };
    }
    bits
}

fn extract_pcbs_from_scalar_values_subset_sum(
    scalar_values: &[f32],
    pc_positions: &[usize; PCGS_PER_FRAME],
    mask: u8,
) -> [u8; PCGS_PER_FRAME] {
    let mut bits = [0u8; PCGS_PER_FRAME];
    for (pcg, start) in pc_positions.iter().enumerate() {
        let metric = (0..PC_PUNCTURE_SYMBOLS)
            .filter(|k| (mask & (1 << k)) != 0)
            .map(|k| scalar_values[(pcg * SYMBOLS_PER_PCG) + start + k])
            .sum::<f32>();
        bits[pcg] = if metric >= 0.0 { 0 } else { 1 };
    }
    bits
}

fn changed_mod_symbol_positions(a: &[f32], b: &[f32], threshold: f32) -> Vec<usize> {
    a.iter()
        .zip(b.iter())
        .enumerate()
        .filter_map(|(idx, (lhs, rhs))| {
            if (*lhs - *rhs).abs() > threshold {
                Some(idx)
            } else {
                None
            }
        })
        .collect()
}

fn differing_indices_i8(a: &[i8], b: &[i8]) -> Vec<usize> {
    a.iter()
        .zip(b.iter())
        .enumerate()
        .filter_map(|(idx, (lhs, rhs))| if lhs != rhs { Some(idx) } else { None })
        .collect()
}

fn punctured_symbol_indices(pc_positions: &[usize; PCGS_PER_FRAME]) -> Vec<usize> {
    pc_positions
        .iter()
        .enumerate()
        .flat_map(|(pcg, start)| {
            (0..PC_PUNCTURE_SYMBOLS).map(move |k| (pcg * SYMBOLS_PER_PCG) + start + k)
        })
        .collect()
}

fn bits_to_u16(bits: &[u8]) -> u16 {
    bits.iter()
        .fold(0u16, |acc, bit| (acc << 1) | (*bit as u16))
}

fn bits_to_string(bits: &[u8]) -> String {
    bits.iter()
        .map(|bit| if *bit == 0 { '0' } else { '1' })
        .collect()
}

fn matching_bits(a: &[u8], b: &[u8]) -> usize {
    a.iter()
        .zip(b.iter())
        .filter(|(lhs, rhs)| lhs == rhs)
        .count()
}

fn assert_locked_bs_ack_attempt(attempt: &DecodeAttempt) {
    assert!(attempt.ftch_crc_ok, "expected FFCH CRC-12 to pass");
    assert!(attempt.tail_ok, "expected tail bits to be zero");
    assert!(attempt.fdsch_crc_ok, "expected f-dsch CRC-16 to pass");
    assert_eq!(
        attempt.info_bits,
        parse_bit_string(EXPECTED_BS_ACK_INFO_BITS)
    );

    let sar_start = 5usize;
    let msg_length_octets = Bitstream::new_init(&attempt.info_bits[sar_start..sar_start + 8])
        .read_bits(8)
        .unwrap() as usize;
    assert_eq!(msg_length_octets, 8, "MSG_LENGTH");
    let sar_end = sar_start + msg_length_octets * 8;

    let observed_crc = bits_to_u16(&attempt.info_bits[sar_end - 16..sar_end]);
    assert_eq!(observed_crc, 0x32B2, "expected BS Ack CRC-16 = 0x32B2");
    assert_eq!(
        crc16_fdsch_bits(&attempt.info_bits[sar_start..sar_end - 16]),
        observed_crc,
        "recomputed f-dsch CRC-16"
    );

    assert_eq!(attempt.ack_seq, Some(7), "ACK_SEQ");
    assert_eq!(attempt.msg_seq, Some(0), "MSG_SEQ");
    assert_eq!(attempt.ack_req, Some(true), "ACK_REQ");
    assert_eq!(attempt.order, Some(0b010000), "ORDER");
    assert_eq!(attempt.use_time, Some(false), "USE_TIME");
    assert_eq!(attempt.action_time, Some(0), "ACTION_TIME");
    assert_eq!(attempt.add_record_len, Some(0), "ADD_RECORD_LEN");
}

fn decode_rc3_signaling_frame(
    qpsk_symbols: &[Complex32],
    lc_mask: u64,
    lc_initial_state: u64,
    frame_chip_start: u64,
    lc_mode: LongCodeMode,
    interleaver_mode: InterleaverMode,
    pc_mode: PcMode,
) -> Option<DecodeAttempt> {
    if qpsk_symbols.len() != OUTPUT_SYMBOLS_PER_FRAME {
        return None;
    }

    let deinterleaved = rc3_pre_viterbi_softs(
        qpsk_symbols,
        lc_mask,
        lc_initial_state,
        frame_chip_start,
        lc_mode,
        interleaver_mode,
        pc_mode,
        None,
    )?;

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

    let info_bits = decoded[..172].to_vec();
    let expected_crc = crc12_forward_ftch(&info_bits);
    let mut observed_crc: u16 = 0;
    for &bit in &decoded[172..184] {
        observed_crc = (observed_crc << 1) | bit as u16;
    }
    let ftch_crc_ok = expected_crc == observed_crc;

    let tail_ok = decoded[184..192].iter().all(|bit| *bit == 0);

    let mut fdsch_crc_ok = false;
    let mut ack_seq = None;
    let mut msg_seq = None;
    let mut ack_req = None;
    let mut order_value = None;
    let mut use_time = None;
    let mut action_time = None;
    let mut add_record_len = None;

    if info_bits.len() < 13
        || info_bits[0] != 1
        || info_bits[1] != 0
        || info_bits[2] != 1
        || info_bits[3] != 1
        || info_bits[4] != 1
    {
        return Some(DecodeAttempt {
            info_bits,
            ftch_crc_ok,
            tail_ok,
            fdsch_crc_ok,
            ack_seq,
            msg_seq,
            ack_req,
            order: order_value,
            use_time,
            action_time,
            add_record_len,
        });
    }

    let sar_start = 5usize;
    let msg_length_octets = Bitstream::new_init(&info_bits[sar_start..sar_start + 8])
        .read_bits(8)
        .ok()? as usize;
    let sar_end = sar_start + msg_length_octets * 8;
    if sar_end > info_bits.len() || sar_end < sar_start + 24 {
        return Some(DecodeAttempt {
            info_bits,
            ftch_crc_ok,
            tail_ok,
            fdsch_crc_ok,
            ack_seq,
            msg_seq,
            ack_req,
            order: order_value,
            use_time,
            action_time,
            add_record_len,
        });
    }

    let expected_fdsch_crc = crc16_fdsch_bits(&info_bits[sar_start..sar_end - 16]);
    let observed_fdsch_crc = Bitstream::new_init(&info_bits[sar_end - 16..sar_end])
        .read_bits(16)
        .ok()? as u16;
    fdsch_crc_ok = expected_fdsch_crc == observed_fdsch_crc;
    if !fdsch_crc_ok {
        return Some(DecodeAttempt {
            info_bits,
            ftch_crc_ok,
            tail_ok,
            fdsch_crc_ok,
            ack_seq,
            msg_seq,
            ack_req,
            order: order_value,
            use_time,
            action_time,
            add_record_len,
        });
    }

    let pdu = FdschPdu::decode(&Bitstream::new_init(
        &info_bits[sar_start + 8..sar_end - 16],
    ))
    .ok()?;

    let FdschMessage::Order(order) = pdu.body.clone() else {
        return Some(DecodeAttempt {
            info_bits,
            ftch_crc_ok,
            tail_ok,
            fdsch_crc_ok,
            ack_seq,
            msg_seq,
            ack_req,
            order: order_value,
            use_time,
            action_time,
            add_record_len,
        });
    };

    ack_seq = Some(pdu.arq.ack_seq);
    msg_seq = Some(pdu.arq.msg_seq);
    ack_req = Some(pdu.arq.ack_req);
    order_value = Some(order.order);
    use_time = Some(order.use_time);
    action_time = Some(order.action_time);
    add_record_len = Some(order.add_record_len);

    Some(DecodeAttempt {
        info_bits,
        ftch_crc_ok,
        tail_ok,
        fdsch_crc_ok,
        ack_seq,
        msg_seq,
        ack_req,
        order: order_value,
        use_time,
        action_time,
        add_record_len,
    })
}

fn rc3_pre_viterbi_softs(
    qpsk_symbols: &[Complex32],
    lc_mask: u64,
    lc_initial_state: u64,
    frame_chip_start: u64,
    lc_mode: LongCodeMode,
    interleaver_mode: InterleaverMode,
    pc_mode: PcMode,
    pc_positions_override: Option<&[usize; PCGS_PER_FRAME]>,
) -> Option<Vec<f32>> {
    if qpsk_symbols.len() != OUTPUT_SYMBOLS_PER_FRAME {
        return None;
    }

    let soft_symbols = qpsk_symbols_to_soft_symbols(qpsk_symbols);

    let lc_decimated = rc3_decimated_lc_bits(lc_mask, lc_initial_state, frame_chip_start);

    let previous_chip_start = if frame_chip_start == 0 {
        LONG_CODE_PERIOD - 1
    } else {
        frame_chip_start - 1
    };
    let mut lc_pair = traffic_lc_with_state(lc_mask, lc_initial_state);
    lc_pair.advance_chips(previous_chip_start as usize);
    let mut previous_chip = lc_pair.next_chip();
    let mut lc_pair_start = [0u8; OUTPUT_SYMBOLS_PER_FRAME];
    let mut lc_pair_previous = [0u8; OUTPUT_SYMBOLS_PER_FRAME];
    for pair_idx in 0..OUTPUT_SYMBOLS_PER_FRAME {
        lc_pair_previous[pair_idx] = previous_chip;
        let start_chip = lc_pair.next_chip();
        lc_pair_start[pair_idx] = start_chip;
        previous_chip = start_chip;
        for _ in 0..((2 * 32) - 1) {
            previous_chip = lc_pair.next_chip();
        }
    }

    let pc_positions = pc_positions_override
        .copied()
        .unwrap_or_else(|| rc3_pc_positions(lc_mask, lc_initial_state, frame_chip_start));

    let descrambled = soft_symbols
        .into_iter()
        .enumerate()
        .map(|(idx, value)| {
            let pcg_index = idx / SYMBOLS_PER_PCG;
            let symbol_in_pcg = idx % SYMBOLS_PER_PCG;
            let pc_start = pc_positions[pcg_index];
            if matches!(pc_mode, PcMode::ErasurePuncture)
                && symbol_in_pcg >= pc_start
                && symbol_in_pcg < pc_start + PC_PUNCTURE_SYMBOLS
            {
                return 0.5;
            }

            let lc_scr = match lc_mode {
                LongCodeMode::None => 0,
                LongCodeMode::OnePerModSymbol => lc_decimated[idx],
                LongCodeMode::OddUsesPairStart => {
                    if idx % 2 == 0 {
                        lc_decimated[idx]
                    } else {
                        lc_decimated[idx - 1]
                    }
                }
                LongCodeMode::OddUsesRawPreviousChip => {
                    let pair_idx = idx / 2;
                    if idx % 2 == 0 {
                        lc_pair_start[pair_idx]
                    } else {
                        lc_pair_previous[pair_idx]
                    }
                }
            };
            if lc_scr == 0 { value } else { 1.0 - value }
        })
        .collect::<Vec<_>>();

    Some(apply_interleaver_mode(&descrambled, interleaver_mode))
}

fn try_decode_generated_rc3_all_zero(
    iq_samples: &[Complex32],
    sample_rate: usize,
) -> Result<DecodedFrame, Error> {
    let expected_info_bits = vec![0u8; EXPECTED_INFO_BITS_LEN];
    let mut best_candidate: Option<BestCandidate> = None;
    let pn_chip_offsets = [0u64, 32767u64];

    if sample_rate != CHIP_RATE * 4 {
        return Err(format!(
            "unexpected sample rate: got {}, expected {}",
            sample_rate,
            CHIP_RATE * 4
        )
        .into());
    }

    for sample_phase in [0usize] {
        let chip_rate = decimate_pick_phase(iq_samples, sample_phase);
        if chip_rate.len() < FRAME_CHIPS {
            continue;
        }

        for chip_offset in 0..64usize {
            let chip_samples = chip_window_padded(&chip_rate, chip_offset, FRAME_CHIPS);

            for sample_transform in [
                SampleTransform::Identity,
                SampleTransform::SwapIqNegateI,
                SampleTransform::SwapIq,
                SampleTransform::Conjugate,
            ] {
                let transformed = transform_samples(&chip_samples, sample_transform);

                for &pn_chip_offset in &pn_chip_offsets {
                    for pn_mode in [PnMode::RepoConvention, PnMode::ConjugateConvention] {
                        let frame_chip_start = absolute_chip_start(pn_chip_offset, chip_offset);
                        let despread = pn_despread(&transformed, frame_chip_start, pn_mode);

                        for invert_q in [false, true] {
                            let qpsk_symbols =
                                dewalsh_rc3_symbols(&despread, DEFAULT_WALSH_CODE, invert_q);

                            for lc_mode in [
                                LongCodeMode::OnePerModSymbol,
                                LongCodeMode::OddUsesPairStart,
                                LongCodeMode::OddUsesRawPreviousChip,
                            ] {
                                for interleaver_mode in [
                                    InterleaverMode::FbbrDecode,
                                    InterleaverMode::FbbrEncode,
                                    InterleaverMode::BitReverseDecode,
                                    InterleaverMode::Identity,
                                ] {
                                    let pc_mode = PcMode::Disabled;
                                    if let Some(attempt) = decode_rc3_signaling_frame(
                                        &qpsk_symbols,
                                        DEFAULT_LONG_CODE_MASK,
                                        DEFAULT_LONG_CODE_STATE,
                                        chip_offset as u64,
                                        lc_mode,
                                        interleaver_mode,
                                        pc_mode,
                                    ) {
                                        let mismatch = hamming_distance(
                                            &attempt.info_bits,
                                            &expected_info_bits,
                                        );
                                        let candidate = BestCandidate {
                                            sample_phase,
                                            chip_offset,
                                            lc_chip_offset: 0,
                                            walsh_code: DEFAULT_WALSH_CODE,
                                            pn_chip_offset,
                                            sample_transform,
                                            pn_mode,
                                            lc_mode,
                                            interleaver_mode,
                                            pc_mode,
                                            invert_q,
                                            mismatch,
                                            ftch_crc_ok: attempt.ftch_crc_ok,
                                            tail_ok: attempt.tail_ok,
                                            fdsch_crc_ok: attempt.fdsch_crc_ok,
                                            prefix: bit_prefix(&attempt.info_bits, 32),
                                        };
                                        if best_candidate.as_ref().is_none_or(|best| {
                                            (
                                                candidate.mismatch,
                                                !candidate.ftch_crc_ok,
                                                !candidate.fdsch_crc_ok,
                                            ) < (
                                                best.mismatch,
                                                !best.ftch_crc_ok,
                                                !best.fdsch_crc_ok,
                                            )
                                        }) {
                                            best_candidate = Some(candidate);
                                        }

                                        if attempt.ftch_crc_ok
                                            && attempt.tail_ok
                                            && attempt.info_bits == expected_info_bits
                                        {
                                            return Ok(DecodedFrame {
                                                sample_phase,
                                                chip_offset,
                                                lc_chip_offset: 0,
                                                pn_chip_offset,
                                                sample_transform,
                                                pn_mode,
                                                lc_mode,
                                                interleaver_mode,
                                                pc_mode,
                                                invert_q,
                                                info_bits: attempt.info_bits,
                                                ftch_crc_ok: attempt.ftch_crc_ok,
                                                tail_ok: attempt.tail_ok,
                                            });
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    if let Some(seed) = best_candidate.as_ref() {
        let seed_sample_phase = seed.sample_phase;
        let seed_chip_offset = seed.chip_offset;
        let seed_sample_transform = seed.sample_transform;
        let seed_pn_chip_offset = seed.pn_chip_offset;
        let seed_pn_mode = seed.pn_mode;
        let seed_lc_mode = seed.lc_mode;
        let seed_interleaver_mode = seed.interleaver_mode;
        let seed_pc_mode = seed.pc_mode;
        let seed_invert_q = seed.invert_q;

        let chip_rate = decimate_pick_phase(iq_samples, seed_sample_phase);
        if chip_rate.len() >= FRAME_CHIPS {
            let chip_samples = chip_window_padded(&chip_rate, seed_chip_offset, FRAME_CHIPS);
            let transformed = transform_samples(&chip_samples, seed_sample_transform);
            let frame_chip_start = absolute_chip_start(seed_pn_chip_offset, seed_chip_offset);
            let despread = pn_despread(&transformed, frame_chip_start, seed_pn_mode);
            let qpsk_symbols = dewalsh_rc3_symbols(&despread, DEFAULT_WALSH_CODE, seed_invert_q);

            for lc_chip_offset in 0..64u64 {
                if let Some(attempt) = decode_rc3_signaling_frame(
                    &qpsk_symbols,
                    DEFAULT_LONG_CODE_MASK,
                    DEFAULT_LONG_CODE_STATE,
                    seed_chip_offset as u64 + lc_chip_offset,
                    seed_lc_mode,
                    seed_interleaver_mode,
                    seed_pc_mode,
                ) {
                    let mismatch = hamming_distance(&attempt.info_bits, &expected_info_bits);
                    let candidate = BestCandidate {
                        sample_phase: seed_sample_phase,
                        chip_offset: seed_chip_offset,
                        lc_chip_offset,
                        walsh_code: DEFAULT_WALSH_CODE,
                        pn_chip_offset: seed_pn_chip_offset,
                        sample_transform: seed_sample_transform,
                        pn_mode: seed_pn_mode,
                        lc_mode: seed_lc_mode,
                        interleaver_mode: seed_interleaver_mode,
                        pc_mode: seed_pc_mode,
                        invert_q: seed_invert_q,
                        mismatch,
                        ftch_crc_ok: attempt.ftch_crc_ok,
                        tail_ok: attempt.tail_ok,
                        fdsch_crc_ok: attempt.fdsch_crc_ok,
                        prefix: bit_prefix(&attempt.info_bits, 32),
                    };
                    if best_candidate.as_ref().is_none_or(|best| {
                        (
                            candidate.mismatch,
                            !candidate.ftch_crc_ok,
                            !candidate.fdsch_crc_ok,
                        ) < (best.mismatch, !best.ftch_crc_ok, !best.fdsch_crc_ok)
                    }) {
                        best_candidate = Some(candidate);
                    }

                    if attempt.ftch_crc_ok
                        && attempt.tail_ok
                        && attempt.info_bits == expected_info_bits
                    {
                        return Ok(DecodedFrame {
                            sample_phase: seed_sample_phase,
                            chip_offset: seed_chip_offset,
                            lc_chip_offset,
                            pn_chip_offset: seed_pn_chip_offset,
                            sample_transform: seed_sample_transform,
                            pn_mode: seed_pn_mode,
                            lc_mode: seed_lc_mode,
                            interleaver_mode: seed_interleaver_mode,
                            pc_mode: seed_pc_mode,
                            invert_q: seed_invert_q,
                            info_bits: attempt.info_bits,
                            ftch_crc_ok: attempt.ftch_crc_ok,
                            tail_ok: attempt.tail_ok,
                        });
                    }
                }
            }
        }
    }

    let detail = if let Some(best) = best_candidate {
        format!(
            "best_candidate: sample_phase={} chip_offset={} lc_chip_offset={} pn_chip_offset={} sample_transform={:?} pn_mode={:?} lc_mode={:?} interleaver_mode={:?} pc_mode={:?} invert_q={} mismatch={} ftch_crc_ok={} tail_ok={} fdsch_crc_ok={} prefix={}",
            best.sample_phase,
            best.chip_offset,
            best.lc_chip_offset,
            best.pn_chip_offset,
            best.sample_transform,
            best.pn_mode,
            best.lc_mode,
            best.interleaver_mode,
            best.pc_mode,
            best.invert_q,
            best.mismatch,
            best.ftch_crc_ok,
            best.tail_ok,
            best.fdsch_crc_ok,
            best.prefix,
        )
    } else {
        "no decode candidates produced".to_string()
    };
    Err(format!(
        "failed to recover a CRC-valid RC3 FFCH frame with expected info bits from WAV; {}",
        detail
    )
    .into())
}

fn local_tx_rc3_qpsk(info_bits: &[u8]) -> Vec<Complex32> {
    let ch = ForwardTrafficChannelRc3::new(ConfigRc3 {
        encoder: get_1_4_k9_encoder(),
        interleaver: ForwardBackwardsBitReversalInterleaver::new(SR1_PARAMS_768),
        scrambling_lc: traffic_lc_with_state(DEFAULT_LONG_CODE_MASK, DEFAULT_LONG_CODE_STATE),
        puncture_lc: traffic_lc_with_state(DEFAULT_LONG_CODE_MASK, DEFAULT_LONG_CODE_STATE),
        lc_chip_cursor: 0,
        previous_pcg_pc_start: 0,
        pcb_scheduler: Rc3PcgPcbScheduler::new(RC3_GATED_REV_PWR_CNTL_DELAY),
        fpc_subchan_gain_linear: 1.0,
        prev_frame_last_chip: 0,
        disable_lc_scrambling: false,
    });
    for pcg in 0..PCGS_PER_FRAME {
        ch.schedule_power_control_bit(pcg as u64, 0);
    }
    ch.send_signaling_bits(info_bits.to_vec());
    ch.next(CdmaSystemTime::default())
}

fn manual_tx_rc3_mod_symbols(
    info_bits: &[u8],
    interleaver_mode: TxInterleaverMode,
    lc_mode: LongCodeMode,
    lc_chip_start: u64,
    power_control_bits: Option<[u8; PCGS_PER_FRAME]>,
) -> Vec<f32> {
    let frame = build_full_rate_frame_bits(info_bits);
    let mut encoder = get_1_4_k9_encoder();
    encoder.reset();
    let mut encoded = Vec::with_capacity(768);
    for &bit in &frame {
        encoded.extend_from_slice(&encoder.encode(bit));
    }
    assert_eq!(encoded.len(), 768);

    let interleaved = match interleaver_mode {
        TxInterleaverMode::FbbrEncode => {
            let mut interleaver = ForwardBackwardsBitReversalInterleaver::new(SR1_PARAMS_768);
            interleaver.encode(&encoded)
        }
        TxInterleaverMode::BitReverseEncode => {
            let mut interleaver = BitReversalInterleaver::new(SR1_PARAMS_768);
            interleaver.encode(&encoded)
        }
        TxInterleaverMode::Identity => encoded,
    };

    let mut lc_decimated = vec![0u8; MOD_SYMBOLS_PER_FRAME];
    let mut lc = traffic_lc_with_state(DEFAULT_LONG_CODE_MASK, DEFAULT_LONG_CODE_STATE);
    lc.advance_chips(lc_chip_start as usize);
    for bit in &mut lc_decimated {
        *bit = lc.next_chip();
        for _ in 1..32 {
            lc.next_chip();
        }
    }

    let previous_chip_start = if lc_chip_start == 0 {
        LONG_CODE_PERIOD - 1
    } else {
        lc_chip_start - 1
    };
    let mut lc_pair = traffic_lc_with_state(DEFAULT_LONG_CODE_MASK, DEFAULT_LONG_CODE_STATE);
    lc_pair.advance_chips(previous_chip_start as usize);
    let mut previous_chip = lc_pair.next_chip();
    let mut lc_pair_start = [0u8; OUTPUT_SYMBOLS_PER_FRAME];
    let mut lc_pair_previous = [0u8; OUTPUT_SYMBOLS_PER_FRAME];
    for pair_idx in 0..OUTPUT_SYMBOLS_PER_FRAME {
        lc_pair_previous[pair_idx] = previous_chip;
        let start_chip = lc_pair.next_chip();
        lc_pair_start[pair_idx] = start_chip;
        previous_chip = start_chip;
        for _ in 0..((2 * 32) - 1) {
            previous_chip = lc_pair.next_chip();
        }
    }

    let pc_positions = rc3_pipeline_pc_positions(
        DEFAULT_LONG_CODE_MASK,
        DEFAULT_LONG_CODE_STATE,
        lc_chip_start,
    );
    let mut mapped = Vec::with_capacity(MOD_SYMBOLS_PER_FRAME);
    for (idx, sym) in interleaved.into_iter().enumerate() {
        let lc_scr = match lc_mode {
            LongCodeMode::None => 0,
            LongCodeMode::OnePerModSymbol => lc_decimated[idx],
            LongCodeMode::OddUsesPairStart => {
                if idx % 2 == 0 {
                    lc_decimated[idx]
                } else {
                    lc_decimated[idx - 1]
                }
            }
            LongCodeMode::OddUsesRawPreviousChip => {
                let pair_idx = idx / 2;
                if idx % 2 == 0 {
                    lc_pair_start[pair_idx]
                } else {
                    lc_pair_previous[pair_idx]
                }
            }
        };
        let scrambled = sym ^ lc_scr;
        let mapped_symbol = if let Some(pc_bits) = power_control_bits {
            let pcg_index = idx / SYMBOLS_PER_PCG;
            let symbol_in_pcg = idx % SYMBOLS_PER_PCG;
            let pc_start = pc_positions[pcg_index];
            let is_pc = symbol_in_pcg >= pc_start && symbol_in_pcg < pc_start + PC_PUNCTURE_SYMBOLS;
            if is_pc {
                if pc_bits[pcg_index] == 0 {
                    1.0f32
                } else {
                    -1.0f32
                }
            } else if scrambled == 0 {
                1.0f32
            } else {
                -1.0f32
            }
        } else if scrambled == 0 {
            1.0f32
        } else {
            -1.0f32
        };
        mapped.push(mapped_symbol);
    }

    mapped
}

fn manual_tx_rc3_qpsk(
    info_bits: &[u8],
    interleaver_mode: TxInterleaverMode,
    lc_mode: LongCodeMode,
    invert_q_output: bool,
    lc_chip_start: u64,
) -> Vec<Complex32> {
    let mapped =
        manual_tx_rc3_mod_symbols(info_bits, interleaver_mode, lc_mode, lc_chip_start, None);

    mapped
        .chunks_exact(2)
        .map(|pair| {
            let q = if invert_q_output { -pair[1] } else { pair[1] };
            Complex32::new(pair[0], q)
        })
        .collect()
}

fn expected_unpunctured_rc3_mod_signs(info_bits: &[u8]) -> Vec<i8> {
    manual_tx_rc3_mod_symbols(
        info_bits,
        TxInterleaverMode::FbbrEncode,
        LongCodeMode::OddUsesRawPreviousChip,
        0,
        None,
    )
    .into_iter()
    .map(|x| if x >= 0.0 { 1 } else { -1 })
    .collect()
}

fn expected_punctured_rc3_mod_signs(info_bits: &[u8], pc_bits: [u8; PCGS_PER_FRAME]) -> Vec<i8> {
    manual_tx_rc3_mod_symbols(
        info_bits,
        TxInterleaverMode::FbbrEncode,
        LongCodeMode::OddUsesRawPreviousChip,
        0,
        Some(pc_bits),
    )
    .into_iter()
    .map(|x| if x >= 0.0 { 1 } else { -1 })
    .collect()
}

fn encoded_full_rate_rc3_bits(info_bits: &[u8]) -> Vec<u8> {
    let frame = build_full_rate_frame_bits(info_bits);
    let mut encoder = get_1_4_k9_encoder();
    encoder.reset();
    let mut encoded = Vec::with_capacity(768);
    for &bit in &frame {
        encoded.extend_from_slice(&encoder.encode(bit));
    }
    assert_eq!(encoded.len(), 768);
    encoded
}

fn expected_deinterleaved_softs_with_pc_positions(
    info_bits: &[u8],
    pc_positions: &[usize; PCGS_PER_FRAME],
) -> Vec<f32> {
    let encoded = encoded_full_rate_rc3_bits(info_bits);
    let mut interleaver = ForwardBackwardsBitReversalInterleaver::new(SR1_PARAMS_768);
    let interleaved = interleaver.encode(&encoded);
    let mut punctured = interleaved
        .into_iter()
        .map(|b| b as f32)
        .collect::<Vec<_>>();
    for (pcg, start) in pc_positions.iter().enumerate() {
        let base = pcg * SYMBOLS_PER_PCG;
        for k in 0..PC_PUNCTURE_SYMBOLS {
            punctured[base + start + k] = 0.5;
        }
    }
    apply_interleaver_mode(&punctured, InterleaverMode::FbbrDecode)
}

const NIBBLE_PERMUTATIONS: [[usize; 4]; 24] = [
    [0, 1, 2, 3],
    [0, 1, 3, 2],
    [0, 2, 1, 3],
    [0, 2, 3, 1],
    [0, 3, 1, 2],
    [0, 3, 2, 1],
    [1, 0, 2, 3],
    [1, 0, 3, 2],
    [1, 2, 0, 3],
    [1, 2, 3, 0],
    [1, 3, 0, 2],
    [1, 3, 2, 0],
    [2, 0, 1, 3],
    [2, 0, 3, 1],
    [2, 1, 0, 3],
    [2, 1, 3, 0],
    [2, 3, 0, 1],
    [2, 3, 1, 0],
    [3, 0, 1, 2],
    [3, 0, 2, 1],
    [3, 1, 0, 2],
    [3, 1, 2, 0],
    [3, 2, 0, 1],
    [3, 2, 1, 0],
];

fn rc3_pc_positions_with_selector(
    lc_decimated: &[u8],
    window_start: usize,
    significance_perm: [usize; 4],
) -> [usize; PCGS_PER_FRAME] {
    let mut pc_positions = [0usize; PCGS_PER_FRAME];
    for pcg in 0..PCGS_PER_FRAME {
        let base = pcg * SYMBOLS_PER_PCG;
        let bits = [
            lc_decimated[base + window_start + significance_perm[0]] as usize,
            lc_decimated[base + window_start + significance_perm[1]] as usize,
            lc_decimated[base + window_start + significance_perm[2]] as usize,
            lc_decimated[base + window_start + significance_perm[3]] as usize,
        ];
        pc_positions[pcg] = ((bits[0] << 3) | (bits[1] << 2) | (bits[2] << 1) | bits[3]) * 2;
    }
    pc_positions
}

fn mean_abs_error(a: &[f32], b: &[f32]) -> f32 {
    assert_eq!(a.len(), b.len());
    a.iter()
        .zip(b.iter())
        .map(|(x, y)| (x - y).abs())
        .sum::<f32>()
        / a.len() as f32
}

fn manual_tx_rc3_prescramble_qpsk(
    info_bits: &[u8],
    interleaver_mode: TxInterleaverMode,
    invert_q_output: bool,
) -> Vec<Complex32> {
    let frame = build_full_rate_frame_bits(info_bits);
    let mut encoder = get_1_4_k9_encoder();
    encoder.reset();
    let mut encoded = Vec::with_capacity(768);
    for &bit in &frame {
        encoded.extend_from_slice(&encoder.encode(bit));
    }
    assert_eq!(encoded.len(), 768);

    let interleaved = match interleaver_mode {
        TxInterleaverMode::FbbrEncode => {
            let mut interleaver = ForwardBackwardsBitReversalInterleaver::new(SR1_PARAMS_768);
            interleaver.encode(&encoded)
        }
        TxInterleaverMode::BitReverseEncode => {
            let mut interleaver = BitReversalInterleaver::new(SR1_PARAMS_768);
            interleaver.encode(&encoded)
        }
        TxInterleaverMode::Identity => encoded,
    };

    interleaved
        .chunks_exact(2)
        .map(|pair| {
            let i = if pair[0] == 0 { 1.0f32 } else { -1.0f32 };
            let q0 = if pair[1] == 0 { 1.0f32 } else { -1.0f32 };
            let q = if invert_q_output { -q0 } else { q0 };
            Complex32::new(i, q)
        })
        .collect()
}

fn qpsk_symbol_sign_mismatch(a: &[Complex32], b: &[Complex32]) -> usize {
    a.iter()
        .zip(b.iter())
        .filter(|(x, y)| {
            let xr = if x.re >= 0.0 { 1 } else { -1 };
            let xi = if x.im >= 0.0 { 1 } else { -1 };
            let yr = if y.re >= 0.0 { 1 } else { -1 };
            let yi = if y.im >= 0.0 { 1 } else { -1 };
            xr != yr || xi != yi
        })
        .count()
}

fn qpsk_symbol_energy(symbols: &[Complex32]) -> f32 {
    symbols.iter().map(|s| s.norm_sqr()).sum()
}

fn top_walsh_rows(
    chip_samples: &[Complex32],
    pn_chip_offset: u64,
    pn_mode: PnMode,
    invert_q: bool,
    top_k: usize,
) -> Vec<(u8, f32)> {
    let despread = pn_despread(chip_samples, pn_chip_offset, pn_mode);
    let mut rows = (0u8..64u8)
        .map(|walsh_code| {
            let recovered = dewalsh_rc3_symbols(&despread, walsh_code, invert_q);
            (walsh_code, qpsk_symbol_energy(&recovered))
        })
        .collect::<Vec<_>>();
    rows.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    rows.truncate(top_k);
    rows
}

fn top_walsh_rows_from_despread(
    despread: &[Complex32],
    invert_q: bool,
    top_k: usize,
) -> Vec<(u8, f32)> {
    let mut rows = (0u8..64u8)
        .map(|walsh_code| {
            let recovered = dewalsh_rc3_symbols(despread, walsh_code, invert_q);
            (walsh_code, qpsk_symbol_energy(&recovered))
        })
        .collect::<Vec<_>>();
    rows.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
    rows.truncate(top_k);
    rows
}

#[test]
#[ignore = "diagnostic: RC3 FFCH all-zero payload decode from MathWorks waveform not yet aligned"]
fn test_decode_mathworks_generated_ffch_rc3_all_zero_wav() -> Result<(), Error> {
    let wav_path = test_iq_path("ffch_rc3_all_zero.wav");
    if !wav_path.exists() {
        eprintln!("skipping: {} not found", wav_path.display());
        return Ok(());
    }

    let (sample_rate, iq_samples) = load_wav_iq_samples(&wav_path)?;
    let decoded = try_decode_generated_rc3_all_zero(&iq_samples, sample_rate)?;

    eprintln!(
        "decoded_mathworks_rc3_all_zero: sample_phase={} chip_offset={} lc_chip_offset={} pn_chip_offset={} sample_transform={:?} pn_mode={:?} lc_mode={:?} interleaver_mode={:?} pc_mode={:?} invert_q={} ftch_crc_ok={} tail_ok={} prefix={}",
        decoded.sample_phase,
        decoded.chip_offset,
        decoded.lc_chip_offset,
        decoded.pn_chip_offset,
        decoded.sample_transform,
        decoded.pn_mode,
        decoded.lc_mode,
        decoded.interleaver_mode,
        decoded.pc_mode,
        decoded.invert_q,
        decoded.ftch_crc_ok,
        decoded.tail_ok,
        bit_prefix(&decoded.info_bits, 32)
    );

    assert!(decoded.ftch_crc_ok, "expected FFCH CRC-12 to pass");
    assert!(decoded.tail_ok, "expected tail bits to be all zero");
    assert_eq!(decoded.info_bits, vec![0u8; EXPECTED_INFO_BITS_LEN]);

    Ok(())
}

#[test]
fn test_acquire_mathworks_generated_pilot_only_wav() -> Result<(), Error> {
    let wav_path = test_iq_path("ffch_rc3_all_zero_pilot_only.wav");
    if !wav_path.exists() {
        eprintln!("skipping: {} not found", wav_path.display());
        return Ok(());
    }

    let (sample_rate, iq_samples) = load_wav_iq_samples(&wav_path)?;
    assert_eq!(sample_rate, CHIP_RATE * 4);

    let pn_refs = [
        (
            PilotReferenceMode::Pn,
            build_pn_reference(32_768, PilotReferenceMode::Pn),
        ),
        (
            PilotReferenceMode::ConjugatePn,
            build_pn_reference(32_768, PilotReferenceMode::ConjugatePn),
        ),
    ];

    let mut best: Option<(usize, SampleTransform, PilotReferenceMode, usize, f32, f32)> = None;
    for sample_phase in 0..4 {
        let chip_rate = decimate_pick_phase(&iq_samples, sample_phase);
        if chip_rate.len() < FRAME_CHIPS {
            continue;
        }
        let chip_samples = &chip_rate[..FRAME_CHIPS];
        for sample_transform in [
            SampleTransform::Identity,
            SampleTransform::Conjugate,
            SampleTransform::NegateI,
            SampleTransform::NegateQ,
            SampleTransform::SwapIq,
            SampleTransform::SwapIqNegateI,
            SampleTransform::SwapIqNegateQ,
            SampleTransform::NegateBoth,
        ] {
            let transformed = transform_samples(chip_samples, sample_transform);
            for (ref_mode, ref_waveform) in &pn_refs {
                let (offset, peak, med) = cyclic_correlation_peak(&transformed, ref_waveform);
                if best
                    .as_ref()
                    .is_none_or(|(_, _, _, _, best_peak, best_med)| {
                        peak / med > best_peak / best_med
                    })
                {
                    best = Some((sample_phase, sample_transform, *ref_mode, offset, peak, med));
                }
            }
        }
    }

    let (sample_phase, sample_transform, ref_mode, offset, peak, med) =
        best.ok_or_else(|| Error::from("no pilot-only acquisition candidates produced"))?;
    let ratio = peak / med.max(1e-6);

    eprintln!(
        "pilot_only_acquisition: sample_phase={} sample_transform={:?} ref_mode={:?} offset={} peak={} median={} ratio={}",
        sample_phase, sample_transform, ref_mode, offset, peak, med, ratio
    );

    assert!(
        ratio > 20.0,
        "expected a sharp PN acquisition peak, got ratio {}",
        ratio
    );
    assert!(
        peak > 1000.0,
        "expected a strong PN acquisition peak, got {}",
        peak
    );

    Ok(())
}

#[test]
#[ignore = "diagnostic: compare MathWorks RC3 despread/deWalsh symbols to local TX symbols"]
fn test_compare_mathworks_rc3_qpsk_to_local_tx() -> Result<(), Error> {
    let wav_path = test_iq_path("ffch_rc3_all_zero.wav");
    if !wav_path.exists() {
        eprintln!("skipping: {} not found", wav_path.display());
        return Ok(());
    }

    let (sample_rate, iq_samples) = load_wav_iq_samples(&wav_path)?;
    assert_eq!(sample_rate, CHIP_RATE * 4);

    let expected_info_bits = vec![0u8; EXPECTED_INFO_BITS_LEN];
    let local_qpsk = local_tx_rc3_qpsk(&expected_info_bits);
    assert_eq!(local_qpsk.len(), OUTPUT_SYMBOLS_PER_FRAME);

    let chip_rate = decimate_pick_phase(&iq_samples, 0);
    let mut best: Option<(usize, SampleTransform, PnMode, u64, bool, usize)> = None;
    for chip_offset in 0..64usize {
        let chip_samples = chip_window_padded(&chip_rate, chip_offset, FRAME_CHIPS);
        for sample_transform in [
            SampleTransform::Identity,
            SampleTransform::Conjugate,
            SampleTransform::NegateI,
            SampleTransform::NegateQ,
            SampleTransform::SwapIq,
            SampleTransform::SwapIqNegateI,
            SampleTransform::SwapIqNegateQ,
            SampleTransform::NegateBoth,
        ] {
            let transformed = transform_samples(&chip_samples, sample_transform);
            for pn_mode in [PnMode::RepoConvention, PnMode::ConjugateConvention] {
                for pn_chip_offset in [0u64, 32767u64] {
                    let frame_chip_start = absolute_chip_start(pn_chip_offset, chip_offset);
                    let despread = pn_despread(&transformed, frame_chip_start, pn_mode);
                    for invert_q in [false, true] {
                        let qpsk_symbols =
                            dewalsh_rc3_symbols(&despread, DEFAULT_WALSH_CODE, invert_q);
                        let mismatch = qpsk_symbol_sign_mismatch(&qpsk_symbols, &local_qpsk);
                        if best
                            .as_ref()
                            .is_none_or(|(_, _, _, _, _, best_mismatch)| mismatch < *best_mismatch)
                        {
                            best = Some((
                                chip_offset,
                                sample_transform,
                                pn_mode,
                                pn_chip_offset,
                                invert_q,
                                mismatch,
                            ));
                        }
                    }
                }
            }
        }
    }

    let (chip_offset, sample_transform, pn_mode, pn_chip_offset, invert_q, mismatch) =
        best.ok_or_else(|| Error::from("no symbol-comparison candidates produced"))?;
    eprintln!(
        "mathworks_vs_local_tx_qpsk: chip_offset={} sample_transform={:?} pn_mode={:?} pn_chip_offset={} invert_q={} mismatch={} / {}",
        chip_offset,
        sample_transform,
        pn_mode,
        pn_chip_offset,
        invert_q,
        mismatch,
        OUTPUT_SYMBOLS_PER_FRAME
    );

    Ok(())
}

#[test]
#[ignore = "diagnostic: sweep manual RC3 TX hypotheses against MathWorks despread/deWalsh symbols"]
fn test_sweep_manual_rc3_tx_hypotheses_against_mathworks() -> Result<(), Error> {
    let wav_path = test_iq_path("ffch_rc3_all_zero.wav");
    if !wav_path.exists() {
        eprintln!("skipping: {} not found", wav_path.display());
        return Ok(());
    }

    let (sample_rate, iq_samples) = load_wav_iq_samples(&wav_path)?;
    assert_eq!(sample_rate, CHIP_RATE * 4);
    let expected_info_bits = vec![0u8; EXPECTED_INFO_BITS_LEN];
    let chip_rate = decimate_pick_phase(&iq_samples, 0);

    let mut best: Option<(
        usize,
        bool,
        TxInterleaverMode,
        LongCodeMode,
        bool,
        u64,
        usize,
    )> = None;
    for chip_offset in 0..64usize {
        let chip_samples = chip_window_padded(&chip_rate, chip_offset, FRAME_CHIPS);
        let despread = pn_despread(
            &chip_samples,
            absolute_chip_start(32767, chip_offset),
            PnMode::RepoConvention,
        );
        for recovered_invert_q in [false, true] {
            let recovered_qpsk =
                dewalsh_rc3_symbols(&despread, DEFAULT_WALSH_CODE, recovered_invert_q);
            for interleaver_mode in [
                TxInterleaverMode::FbbrEncode,
                TxInterleaverMode::BitReverseEncode,
                TxInterleaverMode::Identity,
            ] {
                for lc_mode in [
                    LongCodeMode::OnePerModSymbol,
                    LongCodeMode::OddUsesPairStart,
                    LongCodeMode::OddUsesRawPreviousChip,
                ] {
                    for invert_q_output in [false, true] {
                        for lc_chip_start in 0..64u64 {
                            let manual_qpsk = manual_tx_rc3_qpsk(
                                &expected_info_bits,
                                interleaver_mode,
                                lc_mode,
                                invert_q_output,
                                lc_chip_start,
                            );
                            let mismatch = qpsk_symbol_sign_mismatch(&recovered_qpsk, &manual_qpsk);
                            if best
                                .as_ref()
                                .is_none_or(|(_, _, _, _, _, _, best_mismatch)| {
                                    mismatch < *best_mismatch
                                })
                            {
                                best = Some((
                                    chip_offset,
                                    recovered_invert_q,
                                    interleaver_mode,
                                    lc_mode,
                                    invert_q_output,
                                    lc_chip_start,
                                    mismatch,
                                ));
                            }
                        }
                    }
                }
            }
        }
    }

    let (
        chip_offset,
        recovered_invert_q,
        interleaver_mode,
        lc_mode,
        invert_q_output,
        lc_chip_start,
        mismatch,
    ) = best.ok_or_else(|| Error::from("no RC3 hypothesis candidates produced"))?;
    eprintln!(
        "mathworks_vs_manual_rc3: chip_offset={} recovered_invert_q={} tx_interleaver={:?} lc_mode={:?} tx_invert_q={} lc_chip_start={} mismatch={} / {}",
        chip_offset,
        recovered_invert_q,
        interleaver_mode,
        lc_mode,
        invert_q_output,
        lc_chip_start,
        mismatch,
        OUTPUT_SYMBOLS_PER_FRAME
    );

    Ok(())
}

#[test]
#[ignore = "diagnostic: lock front-end from pilot-only acquisition and sweep Walsh/manual RC3 hypotheses"]
fn test_lock_frontend_from_pilot_and_find_best_manual_rc3_match() -> Result<(), Error> {
    let wav_path = test_iq_path("ffch_rc3_all_zero.wav");
    if !wav_path.exists() {
        eprintln!("skipping: {} not found", wav_path.display());
        return Ok(());
    }

    let (sample_rate, iq_samples) = load_wav_iq_samples(&wav_path)?;
    assert_eq!(sample_rate, CHIP_RATE * 4);

    let expected_info_bits = vec![0u8; EXPECTED_INFO_BITS_LEN];
    let chip_rate = decimate_pick_phase(&iq_samples, 0);

    let manual_candidates = {
        let mut out = Vec::new();
        for interleaver_mode in [
            TxInterleaverMode::FbbrEncode,
            TxInterleaverMode::BitReverseEncode,
            TxInterleaverMode::Identity,
        ] {
            for lc_mode in [
                LongCodeMode::OnePerModSymbol,
                LongCodeMode::OddUsesPairStart,
                LongCodeMode::OddUsesRawPreviousChip,
            ] {
                for invert_q_output in [false, true] {
                    for lc_chip_start in 0..64u64 {
                        out.push((
                            interleaver_mode,
                            lc_mode,
                            invert_q_output,
                            lc_chip_start,
                            manual_tx_rc3_qpsk(
                                &expected_info_bits,
                                interleaver_mode,
                                lc_mode,
                                invert_q_output,
                                lc_chip_start,
                            ),
                        ));
                    }
                }
            }
        }
        out
    };

    let mut best: Option<(
        usize,
        u8,
        bool,
        f32,
        TxInterleaverMode,
        LongCodeMode,
        bool,
        u64,
        usize,
    )> = None;

    for chip_offset in 0..64usize {
        let chip_samples = chip_window_padded(&chip_rate, chip_offset, FRAME_CHIPS);
        let despread = pn_despread(
            &chip_samples,
            absolute_chip_start(32767, chip_offset),
            PnMode::RepoConvention,
        );

        let mut row_energies = (0u8..64u8)
            .map(|walsh_code| {
                let recovered = dewalsh_rc3_symbols(&despread, walsh_code, false);
                (qpsk_symbol_energy(&recovered), walsh_code)
            })
            .collect::<Vec<_>>();
        row_energies.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());

        for &(_, walsh_code) in row_energies.iter().take(8) {
            for recovered_invert_q in [false, true] {
                let recovered_qpsk = dewalsh_rc3_symbols(&despread, walsh_code, recovered_invert_q);
                let recovered_energy = qpsk_symbol_energy(&recovered_qpsk);
                for (interleaver_mode, lc_mode, invert_q_output, lc_chip_start, manual_qpsk) in
                    &manual_candidates
                {
                    let mismatch = qpsk_symbol_sign_mismatch(&recovered_qpsk, manual_qpsk);
                    if best
                        .as_ref()
                        .is_none_or(|(_, _, _, _, _, _, _, _, best_mismatch)| {
                            mismatch < *best_mismatch
                        })
                    {
                        best = Some((
                            chip_offset,
                            walsh_code,
                            recovered_invert_q,
                            recovered_energy,
                            *interleaver_mode,
                            *lc_mode,
                            *invert_q_output,
                            *lc_chip_start,
                            mismatch,
                        ));
                    }
                }
            }
        }
    }

    let (
        chip_offset,
        walsh_code,
        recovered_invert_q,
        recovered_energy,
        interleaver_mode,
        lc_mode,
        invert_q_output,
        lc_chip_start,
        mismatch,
    ) = best.ok_or_else(|| Error::from("no pilot-locked RC3 candidates produced"))?;

    eprintln!(
        "pilot_locked_mathworks_vs_manual_rc3: chip_offset={} walsh_code={} recovered_invert_q={} recovered_energy={} tx_interleaver={:?} lc_mode={:?} tx_invert_q={} lc_chip_start={} mismatch={} / {}",
        chip_offset,
        walsh_code,
        recovered_invert_q,
        recovered_energy,
        interleaver_mode,
        lc_mode,
        invert_q_output,
        lc_chip_start,
        mismatch,
        OUTPUT_SYMBOLS_PER_FRAME
    );

    Ok(())
}

#[test]
#[ignore = "diagnostic: dump Walsh-row energies for MathWorks RC3 FFCH using pilot-derived front-end"]
fn test_dump_mathworks_rc3_walsh_row_energies() -> Result<(), Error> {
    let wav_path = test_iq_path("ffch_rc3_all_zero.wav");
    if !wav_path.exists() {
        eprintln!("skipping: {} not found", wav_path.display());
        return Ok(());
    }

    let (sample_rate, iq_samples) = load_wav_iq_samples(&wav_path)?;
    assert_eq!(sample_rate, CHIP_RATE * 4);

    let chip_rate = decimate_pick_phase(&iq_samples, 0);
    let mut best: Option<(usize, bool, Vec<(u8, f32)>)> = None;

    for chip_offset in 0..64usize {
        let chip_samples = chip_window_padded(&chip_rate, chip_offset, FRAME_CHIPS);
        let transformed = transform_samples(&chip_samples, SampleTransform::Conjugate);
        for invert_q in [false, true] {
            let rows = top_walsh_rows(
                &transformed,
                absolute_chip_start(32767, chip_offset),
                PnMode::RepoConvention,
                invert_q,
                8,
            );
            let lead = rows.first().copied().unwrap();
            if best
                .as_ref()
                .is_none_or(|(_, _, best_rows)| lead.1 > best_rows.first().unwrap().1)
            {
                best = Some((chip_offset, invert_q, rows));
            }
        }
    }

    let (chip_offset, invert_q, rows) =
        best.ok_or_else(|| Error::from("no Walsh row energy candidates produced"))?;
    eprintln!(
        "mathworks_rc3_walsh_rows: chip_offset={} invert_q={} top_rows={:?}",
        chip_offset, invert_q, rows
    );

    Ok(())
}

#[test]
#[ignore = "diagnostic: compare pilot-referenced MathWorks RC3 QPSK symbols to local TX symbols"]
fn test_compare_pilot_referenced_mathworks_rc3_qpsk_to_local_tx() -> Result<(), Error> {
    let base = test_iq_dir();
    let ffch_path = base.join("ffch_rc3_all_zero.wav");
    let pilot_path = base.join("ffch_rc3_all_zero_pilot_only.wav");
    if !ffch_path.exists() || !pilot_path.exists() {
        eprintln!(
            "skipping: {} or {} not found",
            ffch_path.display(),
            pilot_path.display()
        );
        return Ok(());
    }

    let (ffch_sample_rate, ffch_iq_samples) = load_wav_iq_samples(&ffch_path)?;
    let (pilot_sample_rate, pilot_iq_samples) = load_wav_iq_samples(&pilot_path)?;
    assert_eq!(ffch_sample_rate, CHIP_RATE * 4);
    assert_eq!(pilot_sample_rate, CHIP_RATE * 4);

    let expected_info_bits = vec![0u8; EXPECTED_INFO_BITS_LEN];
    let local_qpsk = local_tx_rc3_qpsk(&expected_info_bits);
    assert_eq!(local_qpsk.len(), OUTPUT_SYMBOLS_PER_FRAME);

    let ffch_chip_rate = decimate_pick_phase(&ffch_iq_samples, 0);
    let pilot_chip_rate = decimate_pick_phase(&pilot_iq_samples, 0);

    let mut best: Option<(usize, u8, bool, SampleTransform, usize)> = None;
    for chip_offset in 0..4usize {
        let ffch_chips = chip_window_padded(&ffch_chip_rate, chip_offset, FRAME_CHIPS);
        let pilot_chips = chip_window_padded(&pilot_chip_rate, chip_offset, FRAME_CHIPS);
        let despread = pilot_reference_despread(&ffch_chips, &pilot_chips);

        for walsh_code in [4u8, 5u8, 6u8, 7u8] {
            for invert_q in [false, true] {
                let recovered = dewalsh_rc3_symbols(&despread, walsh_code, invert_q);
                for qpsk_transform in [
                    SampleTransform::Identity,
                    SampleTransform::Conjugate,
                    SampleTransform::NegateI,
                    SampleTransform::NegateQ,
                    SampleTransform::SwapIq,
                    SampleTransform::SwapIqNegateI,
                    SampleTransform::SwapIqNegateQ,
                    SampleTransform::NegateBoth,
                ] {
                    let recovered = transform_samples(&recovered, qpsk_transform);
                    let mismatch = qpsk_symbol_sign_mismatch(&recovered, &local_qpsk);
                    if best
                        .as_ref()
                        .is_none_or(|(_, _, _, _, best_mismatch)| mismatch < *best_mismatch)
                    {
                        best = Some((chip_offset, walsh_code, invert_q, qpsk_transform, mismatch));
                    }
                }
            }
        }
    }

    let (chip_offset, walsh_code, invert_q, qpsk_transform, mismatch) =
        best.ok_or_else(|| Error::from("no pilot-ref symbol-comparison candidates produced"))?;
    eprintln!(
        "pilot_ref_mathworks_vs_local_tx_qpsk: chip_offset={} walsh_code={} invert_q={} qpsk_transform={:?} mismatch={} / {}",
        chip_offset, walsh_code, invert_q, qpsk_transform, mismatch, OUTPUT_SYMBOLS_PER_FRAME
    );

    Ok(())
}

#[test]
#[ignore = "diagnostic: print pilot-ref RC3 decode mismatches for small chip offsets and Walsh rows"]
fn test_print_pilot_ref_decode_candidates() -> Result<(), Error> {
    let base = test_iq_dir();
    let ffch_path = base.join("ffch_rc3_all_zero.wav");
    let pilot_path = base.join("ffch_rc3_all_zero_pilot_only.wav");
    if !ffch_path.exists() || !pilot_path.exists() {
        eprintln!(
            "skipping: {} or {} not found",
            ffch_path.display(),
            pilot_path.display()
        );
        return Ok(());
    }

    let (ffch_sample_rate, ffch_iq_samples) = load_wav_iq_samples(&ffch_path)?;
    let (pilot_sample_rate, pilot_iq_samples) = load_wav_iq_samples(&pilot_path)?;
    assert_eq!(ffch_sample_rate, CHIP_RATE * 4);
    assert_eq!(pilot_sample_rate, CHIP_RATE * 4);

    let ffch_chip_rate = decimate_pick_phase(&ffch_iq_samples, 0);
    let pilot_chip_rate = decimate_pick_phase(&pilot_iq_samples, 0);
    let expected_info_bits = vec![0u8; EXPECTED_INFO_BITS_LEN];

    for chip_offset in 0..4usize {
        let ffch_chips = chip_window_padded(&ffch_chip_rate, chip_offset, FRAME_CHIPS);
        let pilot_chips = chip_window_padded(&pilot_chip_rate, chip_offset, FRAME_CHIPS);
        let despread = pilot_reference_despread(&ffch_chips, &pilot_chips);
        for walsh_code in [4u8, 5u8, 6u8, 7u8] {
            for invert_q in [false, true] {
                let qpsk_symbols = dewalsh_rc3_symbols(&despread, walsh_code, invert_q);
                if let Some(attempt) = decode_rc3_signaling_frame(
                    &qpsk_symbols,
                    DEFAULT_LONG_CODE_MASK,
                    DEFAULT_LONG_CODE_STATE,
                    chip_offset as u64,
                    LongCodeMode::OnePerModSymbol,
                    InterleaverMode::FbbrDecode,
                    PcMode::Disabled,
                ) {
                    let mismatch = hamming_distance(&attempt.info_bits, &expected_info_bits);
                    eprintln!(
                        "pilot_ref_candidate: chip_offset={} walsh_code={} invert_q={} mismatch={} ftch_crc_ok={} tail_ok={} prefix={}",
                        chip_offset,
                        walsh_code,
                        invert_q,
                        mismatch,
                        attempt.ftch_crc_ok,
                        attempt.tail_ok,
                        bit_prefix(&attempt.info_bits, 32)
                    );
                }
            }
        }
    }

    Ok(())
}

#[test]
#[ignore = "diagnostic: search Walsh chip phase for pilot-ref RC3 all-zero decode"]
fn test_search_pilot_ref_walsh_phase() -> Result<(), Error> {
    let base = test_iq_dir();
    let ffch_path = base.join("ffch_rc3_all_zero.wav");
    let pilot_path = base.join("ffch_rc3_all_zero_pilot_only.wav");
    if !ffch_path.exists() || !pilot_path.exists() {
        eprintln!(
            "skipping: {} or {} not found",
            ffch_path.display(),
            pilot_path.display()
        );
        return Ok(());
    }

    let (ffch_sample_rate, ffch_iq_samples) = load_wav_iq_samples(&ffch_path)?;
    let (pilot_sample_rate, pilot_iq_samples) = load_wav_iq_samples(&pilot_path)?;
    assert_eq!(ffch_sample_rate, CHIP_RATE * 4);
    assert_eq!(pilot_sample_rate, CHIP_RATE * 4);

    let ffch_chip_rate = decimate_pick_phase(&ffch_iq_samples, 0);
    let pilot_chip_rate = decimate_pick_phase(&pilot_iq_samples, 0);
    let expected_info_bits = vec![0u8; EXPECTED_INFO_BITS_LEN];

    let mut best: Option<(
        usize,
        u8,
        usize,
        bool,
        LongCodeMode,
        InterleaverMode,
        u64,
        usize,
        bool,
    )> = None;

    for chip_offset in [0usize] {
        let ffch_chips = chip_window_padded(&ffch_chip_rate, chip_offset, FRAME_CHIPS);
        let pilot_chips = chip_window_padded(&pilot_chip_rate, chip_offset, FRAME_CHIPS);
        let despread = pilot_reference_despread(&ffch_chips, &pilot_chips);
        for (walsh_code, walsh_chip_phase) in [(5u8, 1usize), (7u8, 3usize)] {
            for invert_q in [false, true] {
                let qpsk_symbols = dewalsh_rc3_symbols_with_phase(
                    &despread,
                    walsh_code,
                    invert_q,
                    walsh_chip_phase,
                );
                for lc_mode in [
                    LongCodeMode::OnePerModSymbol,
                    LongCodeMode::OddUsesPairStart,
                    LongCodeMode::OddUsesRawPreviousChip,
                ] {
                    for interleaver_mode in [
                        InterleaverMode::FbbrDecode,
                        InterleaverMode::FbbrEncode,
                        InterleaverMode::BitReverseDecode,
                        InterleaverMode::Identity,
                    ] {
                        for lc_chip_offset in 0..64u64 {
                            if let Some(attempt) = decode_rc3_signaling_frame(
                                &qpsk_symbols,
                                DEFAULT_LONG_CODE_MASK,
                                DEFAULT_LONG_CODE_STATE,
                                chip_offset as u64 + lc_chip_offset,
                                lc_mode,
                                interleaver_mode,
                                PcMode::Disabled,
                            ) {
                                let mismatch =
                                    hamming_distance(&attempt.info_bits, &expected_info_bits);
                                let better = best.as_ref().is_none_or(
                                    |(_, _, _, _, _, _, _, best_mismatch, best_crc_ok)| {
                                        (mismatch, !attempt.ftch_crc_ok)
                                            < (*best_mismatch, !*best_crc_ok)
                                    },
                                );
                                if better {
                                    best = Some((
                                        chip_offset,
                                        walsh_code,
                                        walsh_chip_phase,
                                        invert_q,
                                        lc_mode,
                                        interleaver_mode,
                                        lc_chip_offset,
                                        mismatch,
                                        attempt.ftch_crc_ok,
                                    ));
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    let (
        chip_offset,
        walsh_code,
        walsh_chip_phase,
        invert_q,
        lc_mode,
        interleaver_mode,
        lc_chip_offset,
        mismatch,
        ftch_crc_ok,
    ) = best.ok_or_else(|| Error::from("no Walsh-phase candidates produced"))?;
    eprintln!(
        "pilot_ref_walsh_phase_best: chip_offset={} walsh_code={} walsh_chip_phase={} invert_q={} lc_mode={:?} interleaver_mode={:?} lc_chip_offset={} mismatch={} ftch_crc_ok={}",
        chip_offset,
        walsh_code,
        walsh_chip_phase,
        invert_q,
        lc_mode,
        interleaver_mode,
        lc_chip_offset,
        mismatch,
        ftch_crc_ok
    );

    Ok(())
}

#[test]
#[ignore = "diagnostic: infer MathWorks RC3 long-code bits from pilot-ref recovered symbols"]
fn test_infer_long_code_from_pilot_ref_symbols() -> Result<(), Error> {
    let base = test_iq_dir();
    let ffch_path = base.join("ffch_rc3_all_zero.wav");
    let pilot_path = base.join("ffch_rc3_all_zero_pilot_only.wav");
    if !ffch_path.exists() || !pilot_path.exists() {
        eprintln!(
            "skipping: {} or {} not found",
            ffch_path.display(),
            pilot_path.display()
        );
        return Ok(());
    }

    let (ffch_sample_rate, ffch_iq_samples) = load_wav_iq_samples(&ffch_path)?;
    let (pilot_sample_rate, pilot_iq_samples) = load_wav_iq_samples(&pilot_path)?;
    assert_eq!(ffch_sample_rate, CHIP_RATE * 4);
    assert_eq!(pilot_sample_rate, CHIP_RATE * 4);

    let ffch_chip_rate = decimate_pick_phase(&ffch_iq_samples, 0);
    let pilot_chip_rate = decimate_pick_phase(&pilot_iq_samples, 0);
    let expected_info_bits = vec![0u8; EXPECTED_INFO_BITS_LEN];
    let expected_qpsk =
        manual_tx_rc3_prescramble_qpsk(&expected_info_bits, TxInterleaverMode::FbbrEncode, false);

    let ffch_chips = chip_window_padded(&ffch_chip_rate, 0, FRAME_CHIPS);
    let pilot_chips = chip_window_padded(&pilot_chip_rate, 0, FRAME_CHIPS);
    let despread = pilot_reference_despread(&ffch_chips, &pilot_chips);

    for (walsh_code, walsh_chip_phase) in [(5u8, 1usize), (7u8, 3usize)] {
        let recovered =
            dewalsh_rc3_symbols_with_phase(&despread, walsh_code, false, walsh_chip_phase);
        let inferred_bits = recovered
            .iter()
            .zip(expected_qpsk.iter())
            .flat_map(|(rx, tx)| {
                let i_scr = (rx.re >= 0.0) != (tx.re >= 0.0);
                let q_scr = (rx.im >= 0.0) != (tx.im >= 0.0);
                [u8::from(i_scr), u8::from(q_scr)]
            })
            .collect::<Vec<_>>();

        let mut best: Option<(LongCodeMode, u64, usize)> = None;
        for lc_mode in [
            LongCodeMode::OnePerModSymbol,
            LongCodeMode::OddUsesPairStart,
            LongCodeMode::OddUsesRawPreviousChip,
        ] {
            for lc_chip_start in 0..64u64 {
                let expected = manual_tx_rc3_qpsk(
                    &expected_info_bits,
                    TxInterleaverMode::FbbrEncode,
                    lc_mode,
                    false,
                    lc_chip_start,
                )
                .iter()
                .flat_map(|z| {
                    let i = if z.re >= 0.0 { 0 } else { 1 };
                    let q = if z.im >= 0.0 { 0 } else { 1 };
                    [i, q]
                })
                .collect::<Vec<_>>();

                let pres = expected_qpsk
                    .iter()
                    .flat_map(|z| {
                        let i = if z.re >= 0.0 { 0 } else { 1 };
                        let q = if z.im >= 0.0 { 0 } else { 1 };
                        [i, q]
                    })
                    .collect::<Vec<_>>();

                let expected_scr = expected
                    .iter()
                    .zip(pres.iter())
                    .map(|(a, b)| a ^ b)
                    .collect::<Vec<_>>();

                let mismatch = hamming_distance(&inferred_bits, &expected_scr);
                if best
                    .as_ref()
                    .is_none_or(|(_, _, best_mismatch)| mismatch < *best_mismatch)
                {
                    best = Some((lc_mode, lc_chip_start, mismatch));
                }
            }
        }

        let (lc_mode, lc_chip_start, mismatch) =
            best.ok_or_else(|| Error::from("no inferred long-code candidates produced"))?;
        eprintln!(
            "pilot_ref_inferred_lc: walsh_code={} walsh_chip_phase={} lc_mode={:?} lc_chip_start={} mismatch={}",
            walsh_code, walsh_chip_phase, lc_mode, lc_chip_start, mismatch
        );
    }

    Ok(())
}

#[test]
fn test_decode_mathworks_generated_ffch_rc3_all_zero_with_pilot_reference_locked()
-> Result<(), Error> {
    let attempt = decode_mathworks_generated_ffch_rc3_locked(
        "ffch_rc3_all_zero.wav",
        "ffch_rc3_all_zero_pilot_only.wav",
        PcMode::Disabled,
        true,
    )?;

    assert!(attempt.ftch_crc_ok, "expected FFCH CRC-12 to pass");
    assert!(attempt.tail_ok, "expected tail bits to be zero");
    assert!(
        !attempt.fdsch_crc_ok,
        "all-zero vector is not a real f-dsch PDU"
    );
    assert_eq!(attempt.info_bits, vec![0u8; EXPECTED_INFO_BITS_LEN]);

    Ok(())
}

#[test]
fn test_decode_mathworks_generated_ffch_rc3_all_zero_pc_on_with_pilot_reference_locked()
-> Result<(), Error> {
    let attempt = decode_mathworks_generated_ffch_rc3_locked(
        "ffch_rc3_all_zero_pc_on.wav",
        "ffch_rc3_all_zero_pilot_only.wav",
        PcMode::ErasurePuncture,
        true,
    )?;

    assert!(attempt.ftch_crc_ok, "expected FFCH CRC-12 to pass");
    assert!(attempt.tail_ok, "expected tail bits to be zero");
    assert!(
        !attempt.fdsch_crc_ok,
        "all-zero vector is not a real f-dsch PDU"
    );
    assert_eq!(attempt.info_bits, vec![0u8; EXPECTED_INFO_BITS_LEN]);

    Ok(())
}

#[test]
fn test_decode_mathworks_generated_ffch_rc3_bs_ack_with_pilot_reference_locked() -> Result<(), Error>
{
    let attempt = decode_mathworks_generated_ffch_rc3_locked(
        "ffch_rc3_bs_ack.wav",
        "ffch_rc3_bs_ack_pilot_only.wav",
        PcMode::Disabled,
        true,
    )?;

    assert_locked_bs_ack_attempt(&attempt);

    Ok(())
}

#[test]
fn test_decode_mathworks_generated_ffch_rc3_bs_ack_pc_on_with_pilot_reference_locked()
-> Result<(), Error> {
    let attempt = decode_mathworks_generated_ffch_rc3_locked(
        "ffch_rc3_bs_ack_pc_on.wav",
        "ffch_rc3_bs_ack_pc_on_pilot_only.wav",
        PcMode::ErasurePuncture,
        true,
    )?;

    assert_locked_bs_ack_attempt(&attempt);

    Ok(())
}

#[test]
fn capture_decode_locally_generated_ffch_rc3_bs_ack_pc_on_with_pilot_reference_skip_lc()
-> Result<(), Error> {
    let (ffch_path, pilot_path) = local_generated_wav_pair("bs_ack_pc_on_skip_lc", true)?;
    let attempt = decode_generated_ffch_rc3_with_pilot_reference(
        &ffch_path,
        &pilot_path,
        DEFAULT_WALSH_CODE,
        false,
        SampleTransform::Conjugate,
        PcMode::ErasurePuncture,
        false,
        0,
    )?;

    assert_locked_bs_ack_attempt(&attempt);

    Ok(())
}

#[test]
fn capture_decode_locally_generated_ffch_rc3_bs_ack_pc_on_with_pilot_reference_with_lc()
-> Result<(), Error> {
    let (ffch_path, pilot_path) = local_generated_wav_pair("bs_ack_pc_on_with_lc", false)?;
    let attempt = decode_generated_ffch_rc3_with_pilot_reference(
        &ffch_path,
        &pilot_path,
        DEFAULT_WALSH_CODE,
        false,
        SampleTransform::Conjugate,
        PcMode::ErasurePuncture,
        true,
        0,
    )?;

    assert_locked_bs_ack_attempt(&attempt);

    Ok(())
}

fn decode_mathworks_generated_ffch_rc3_locked(
    ffch_name: &str,
    pilot_name: &str,
    pc_mode: PcMode,
    lc_descramble: bool,
) -> Result<DecodeAttempt, Error> {
    // Empirically, the generated seeded MathWorks RC3 FFCH reference decodes with:
    //   - pilot-only waveform used as the common short-PN reference
    //   - Walsh row 4 at chip offset 0
    //   - Conjugate post-deWalsh symbol convention
    //   - RC3 long-code descrambling using the raw previous chip on the odd lane
    //   - frame_chip_start 0
    let base = test_iq_dir();
    decode_generated_ffch_rc3_with_pilot_reference_and_lc_mode(
        &base.join(ffch_name),
        &base.join(pilot_name),
        4,
        false,
        SampleTransform::Conjugate,
        pc_mode,
        if lc_descramble {
            LongCodeMode::OddUsesRawPreviousChip
        } else {
            LongCodeMode::None
        },
        0,
    )
}

fn load_locked_mathworks_generated_ffch_rc3_qpsk_symbols(
    ffch_name: &str,
    pilot_name: &str,
) -> Result<Vec<Complex32>, Error> {
    let base = test_iq_dir();
    let ffch_path = base.join(ffch_name);
    let pilot_path = base.join(pilot_name);
    if !ffch_path.exists() || !pilot_path.exists() {
        return Err(format!(
            "missing required files: {} or {}",
            ffch_path.display(),
            pilot_path.display()
        )
        .into());
    }

    let (ffch_sample_rate, ffch_iq_samples) = load_wav_iq_samples(&ffch_path)?;
    let (pilot_sample_rate, pilot_iq_samples) = load_wav_iq_samples(&pilot_path)?;
    assert_eq!(ffch_sample_rate, CHIP_RATE * 4);
    assert_eq!(pilot_sample_rate, CHIP_RATE * 4);

    let ffch_chip_rate = decimate_pick_phase(&ffch_iq_samples, 0);
    let pilot_chip_rate = decimate_pick_phase(&pilot_iq_samples, 0);
    assert!(ffch_chip_rate.len() >= FRAME_CHIPS);
    assert!(pilot_chip_rate.len() >= FRAME_CHIPS);

    let ffch_chips = chip_window_padded(&ffch_chip_rate, 0, FRAME_CHIPS);
    let pilot_chips = chip_window_padded(&pilot_chip_rate, 0, FRAME_CHIPS);
    let despread = pilot_reference_despread(&ffch_chips, &pilot_chips);
    Ok(transform_samples(
        &dewalsh_rc3_symbols(&despread, 4, false),
        SampleTransform::Conjugate,
    ))
}

fn load_mathworks_generated_ffch_rc3_qpsk_symbols_with_phase_and_transform(
    ffch_name: &str,
    pilot_name: &str,
    walsh_code: u8,
    invert_q: bool,
    walsh_chip_phase: usize,
    qpsk_transform: SampleTransform,
) -> Result<Vec<Complex32>, Error> {
    let base = test_iq_dir();
    let ffch_path = base.join(ffch_name);
    let pilot_path = base.join(pilot_name);
    if !ffch_path.exists() || !pilot_path.exists() {
        return Err(format!(
            "missing required files: {} or {}",
            ffch_path.display(),
            pilot_path.display()
        )
        .into());
    }

    let (ffch_sample_rate, ffch_iq_samples) = load_wav_iq_samples(&ffch_path)?;
    let (pilot_sample_rate, pilot_iq_samples) = load_wav_iq_samples(&pilot_path)?;
    assert_eq!(ffch_sample_rate, CHIP_RATE * 4);
    assert_eq!(pilot_sample_rate, CHIP_RATE * 4);

    let ffch_chip_rate = decimate_pick_phase(&ffch_iq_samples, 0);
    let pilot_chip_rate = decimate_pick_phase(&pilot_iq_samples, 0);
    assert!(ffch_chip_rate.len() >= FRAME_CHIPS);
    assert!(pilot_chip_rate.len() >= FRAME_CHIPS);

    let ffch_chips = chip_window_padded(&ffch_chip_rate, 0, FRAME_CHIPS);
    let pilot_chips = chip_window_padded(&pilot_chip_rate, 0, FRAME_CHIPS);
    let despread = pilot_reference_despread(&ffch_chips, &pilot_chips);
    Ok(transform_samples(
        &dewalsh_rc3_symbols_with_phase(&despread, walsh_code, invert_q, walsh_chip_phase),
        qpsk_transform,
    ))
}

fn load_mathworks_generated_ffch_rc3_pilot_referenced_chips(
    ffch_name: &str,
    pilot_name: &str,
) -> Result<Vec<Complex32>, Error> {
    let base = test_iq_dir();
    let ffch_path = base.join(ffch_name);
    let pilot_path = base.join(pilot_name);
    if !ffch_path.exists() || !pilot_path.exists() {
        return Err(format!(
            "missing required files: {} or {}",
            ffch_path.display(),
            pilot_path.display()
        )
        .into());
    }

    let (ffch_sample_rate, ffch_iq_samples) = load_wav_iq_samples(&ffch_path)?;
    let (pilot_sample_rate, pilot_iq_samples) = load_wav_iq_samples(&pilot_path)?;
    assert_eq!(ffch_sample_rate, CHIP_RATE * 4);
    assert_eq!(pilot_sample_rate, CHIP_RATE * 4);

    let ffch_chip_rate = decimate_pick_phase(&ffch_iq_samples, 0);
    let pilot_chip_rate = decimate_pick_phase(&pilot_iq_samples, 0);
    assert!(ffch_chip_rate.len() >= FRAME_CHIPS);
    assert!(pilot_chip_rate.len() >= FRAME_CHIPS);

    let ffch_chips = chip_window_padded(&ffch_chip_rate, 0, FRAME_CHIPS);
    let pilot_chips = chip_window_padded(&pilot_chip_rate, 0, FRAME_CHIPS);
    Ok(pilot_reference_despread(&ffch_chips, &pilot_chips))
}

fn load_generated_ffch_rc3_qpsk_symbols_with_pilot_reference(
    ffch_path: &PathBuf,
    pilot_path: &PathBuf,
    walsh_code: u8,
    invert_q: bool,
    qpsk_transform: SampleTransform,
) -> Result<Vec<Complex32>, Error> {
    if !ffch_path.exists() || !pilot_path.exists() {
        return Err(format!(
            "missing required files: {} or {}",
            ffch_path.display(),
            pilot_path.display()
        )
        .into());
    }

    let (ffch_sample_rate, ffch_iq_samples) = load_wav_iq_samples(ffch_path)?;
    let (pilot_sample_rate, pilot_iq_samples) = load_wav_iq_samples(pilot_path)?;
    assert_eq!(ffch_sample_rate, CHIP_RATE * 4);
    assert_eq!(pilot_sample_rate, CHIP_RATE * 4);

    let ffch_chip_rate = decimate_pick_phase(&ffch_iq_samples, 0);
    let pilot_chip_rate = decimate_pick_phase(&pilot_iq_samples, 0);
    assert!(ffch_chip_rate.len() >= FRAME_CHIPS);
    assert!(pilot_chip_rate.len() >= FRAME_CHIPS);

    let ffch_chips = chip_window_padded(&ffch_chip_rate, 0, FRAME_CHIPS);
    let pilot_chips = chip_window_padded(&pilot_chip_rate, 0, FRAME_CHIPS);
    let despread = pilot_reference_despread(&ffch_chips, &pilot_chips);
    Ok(transform_samples(
        &dewalsh_rc3_symbols(&despread, walsh_code, invert_q),
        qpsk_transform,
    ))
}

fn decode_generated_ffch_rc3_with_pilot_reference(
    ffch_path: &PathBuf,
    pilot_path: &PathBuf,
    walsh_code: u8,
    invert_q: bool,
    qpsk_transform: SampleTransform,
    pc_mode: PcMode,
    lc_descramble: bool,
    frame_chip_start: u64,
) -> Result<DecodeAttempt, Error> {
    let lc_mode = if lc_descramble {
        LongCodeMode::OddUsesRawPreviousChip
    } else {
        LongCodeMode::None
    };
    decode_generated_ffch_rc3_with_pilot_reference_and_lc_mode(
        ffch_path,
        pilot_path,
        walsh_code,
        invert_q,
        qpsk_transform,
        pc_mode,
        lc_mode,
        frame_chip_start,
    )
}

fn decode_generated_ffch_rc3_with_pilot_reference_and_lc_mode(
    ffch_path: &PathBuf,
    pilot_path: &PathBuf,
    walsh_code: u8,
    invert_q: bool,
    qpsk_transform: SampleTransform,
    pc_mode: PcMode,
    lc_mode: LongCodeMode,
    frame_chip_start: u64,
) -> Result<DecodeAttempt, Error> {
    let qpsk_symbols = load_generated_ffch_rc3_qpsk_symbols_with_pilot_reference(
        ffch_path,
        pilot_path,
        walsh_code,
        invert_q,
        qpsk_transform,
    )?;
    decode_rc3_signaling_frame(
        &qpsk_symbols,
        DEFAULT_LONG_CODE_MASK,
        DEFAULT_LONG_CODE_STATE,
        frame_chip_start,
        lc_mode,
        InterleaverMode::FbbrDecode,
        pc_mode,
    )
    .ok_or_else(|| Error::from("no RC3 decode attempt produced"))
}

#[test]
fn test_mathworks_generated_ffch_rc3_bs_ack_pc_on_changed_symbols_do_not_match_lc_puncture_positions()
-> Result<(), Error> {
    let base = test_iq_dir();
    let off_path = base.join("ffch_rc3_bs_ack.wav");
    let on_path = base.join("ffch_rc3_bs_ack_pc_on.wav");
    let pilot_path = base.join("ffch_rc3_bs_ack_pilot_only.wav");
    if !off_path.exists() || !on_path.exists() || !pilot_path.exists() {
        eprintln!(
            "skipping: {} or {} or {} not found",
            off_path.display(),
            on_path.display(),
            pilot_path.display()
        );
        return Ok(());
    }

    let qpsk_off = load_locked_mathworks_generated_ffch_rc3_qpsk_symbols(
        "ffch_rc3_bs_ack.wav",
        "ffch_rc3_bs_ack_pilot_only.wav",
    )?;
    let qpsk_on = load_locked_mathworks_generated_ffch_rc3_qpsk_symbols(
        "ffch_rc3_bs_ack_pc_on.wav",
        "ffch_rc3_bs_ack_pilot_only.wav",
    )?;
    let soft_off = qpsk_symbols_to_soft_symbols(&qpsk_off);
    let soft_on = qpsk_symbols_to_soft_symbols(&qpsk_on);
    let changed = changed_mod_symbol_positions(&soft_off, &soft_on, 1e-3);

    let pc_positions = rc3_pc_positions(DEFAULT_LONG_CODE_MASK, DEFAULT_LONG_CODE_STATE, 0);
    let expected = pc_positions
        .iter()
        .enumerate()
        .flat_map(|(pcg, start)| {
            (0..PC_PUNCTURE_SYMBOLS).map(move |k| (pcg * SYMBOLS_PER_PCG) + start + k)
        })
        .collect::<Vec<_>>();

    let missing = expected
        .iter()
        .copied()
        .filter(|idx| !changed.contains(idx))
        .collect::<Vec<_>>();
    let unexpected = changed
        .iter()
        .copied()
        .filter(|idx| !expected.contains(idx))
        .collect::<Vec<_>>();

    eprintln!("lc_pc_positions={pc_positions:?}");
    eprintln!("expected_changed={expected:?}");
    eprintln!("observed_changed={changed:?}");
    eprintln!("missing_expected={missing:?}");
    eprintln!("unexpected_changed={unexpected:?}");

    assert!(
        !missing.is_empty(),
        "expected at least one LC-derived puncture position to be missing from the observed waveform delta"
    );
    assert!(
        !unexpected.is_empty(),
        "expected at least one observed waveform delta outside the LC-derived puncture positions"
    );

    Ok(())
}

#[test]
fn test_mathworks_generated_ffch_rc3_bs_ack_pc_on_punctured_signs_diagnostic() -> Result<(), Error>
{
    let qpsk_on = load_locked_mathworks_generated_ffch_rc3_qpsk_symbols(
        "ffch_rc3_bs_ack_pc_on.wav",
        "ffch_rc3_bs_ack_pc_on_pilot_only.wav",
    )?;
    let observed = qpsk_symbols_to_hard_mod_signs(&qpsk_on);
    let expected = expected_punctured_rc3_mod_signs(
        &parse_bit_string(EXPECTED_BS_ACK_INFO_BITS),
        alternating_power_control_bits(),
    );
    let pc_positions = rc3_pc_positions(DEFAULT_LONG_CODE_MASK, DEFAULT_LONG_CODE_STATE, 0);
    let punctured = punctured_symbol_indices(&pc_positions);
    let mismatches = punctured
        .iter()
        .copied()
        .filter(|&idx| observed[idx] != expected[idx])
        .collect::<Vec<_>>();

    eprintln!("mathworks_pc_positions={pc_positions:?}");
    eprintln!("mathworks_punctured_indices={punctured:?}");
    eprintln!("mathworks_punctured_sign_mismatches={mismatches:?}");
    eprintln!(
        "mathworks_punctured_sign_match_count={} / {}",
        punctured.len() - mismatches.len(),
        punctured.len()
    );

    Ok(())
}

#[test]
fn test_mathworks_generated_ffch_rc3_bs_ack_pc_on_expected_delta_positions_diagnostic()
-> Result<(), Error> {
    let qpsk_off = load_locked_mathworks_generated_ffch_rc3_qpsk_symbols(
        "ffch_rc3_bs_ack.wav",
        "ffch_rc3_bs_ack_pilot_only.wav",
    )?;
    let qpsk_on = load_locked_mathworks_generated_ffch_rc3_qpsk_symbols(
        "ffch_rc3_bs_ack_pc_on.wav",
        "ffch_rc3_bs_ack_pc_on_pilot_only.wav",
    )?;
    let observed_off = qpsk_symbols_to_hard_mod_signs(&qpsk_off);
    let observed_on = qpsk_symbols_to_hard_mod_signs(&qpsk_on);
    let expected_off =
        expected_unpunctured_rc3_mod_signs(&parse_bit_string(EXPECTED_BS_ACK_INFO_BITS));
    let expected_on = expected_punctured_rc3_mod_signs(
        &parse_bit_string(EXPECTED_BS_ACK_INFO_BITS),
        alternating_power_control_bits(),
    );

    let expected_changed = differing_indices_i8(&expected_off, &expected_on);
    let observed_changed = differing_indices_i8(&observed_off, &observed_on);

    let missing = expected_changed
        .iter()
        .copied()
        .filter(|idx| !observed_changed.contains(idx))
        .collect::<Vec<_>>();
    let unexpected = observed_changed
        .iter()
        .copied()
        .filter(|idx| !expected_changed.contains(idx))
        .collect::<Vec<_>>();

    eprintln!("mathworks_expected_changed_positions={expected_changed:?}");
    eprintln!("mathworks_observed_changed_positions={observed_changed:?}");
    eprintln!("mathworks_missing_expected_changed={missing:?}");
    eprintln!("mathworks_unexpected_changed={unexpected:?}");
    eprintln!(
        "mathworks_expected_vs_observed_changed_counts={} expected / {} observed / {} overlap",
        expected_changed.len(),
        observed_changed.len(),
        expected_changed.len() - missing.len()
    );

    Ok(())
}

#[test]
fn capture_local_generated_ffch_rc3_bs_ack_pc_on_punctured_signs_match_expected()
-> Result<(), Error> {
    let (ffch_path, pilot_path) = local_generated_wav_pair("bs_ack_pc_on_with_lc", false)?;
    let qpsk_on = load_generated_ffch_rc3_qpsk_symbols_with_pilot_reference(
        &ffch_path,
        &pilot_path,
        DEFAULT_WALSH_CODE,
        false,
        SampleTransform::Conjugate,
    )?;
    let observed = qpsk_symbols_to_hard_mod_signs(&qpsk_on);
    let expected = expected_punctured_rc3_mod_signs(
        &parse_bit_string(EXPECTED_BS_ACK_INFO_BITS),
        alternating_power_control_bits(),
    );
    let pc_positions = rc3_pc_positions(DEFAULT_LONG_CODE_MASK, DEFAULT_LONG_CODE_STATE, 0);
    let punctured = punctured_symbol_indices(&pc_positions);
    let mismatches = punctured
        .iter()
        .copied()
        .filter(|&idx| observed[idx] != expected[idx])
        .collect::<Vec<_>>();

    assert_eq!(
        mismatches,
        Vec::<usize>::new(),
        "expected local punctured signs to match the alternating PCB symbols exactly"
    );

    Ok(())
}

#[test]
fn test_mathworks_generated_ffch_rc3_bs_ack_pc_on_bruteforce_pc_selector_pre_viterbi()
-> Result<(), Error> {
    let qpsk_on = load_locked_mathworks_generated_ffch_rc3_qpsk_symbols(
        "ffch_rc3_bs_ack_pc_on.wav",
        "ffch_rc3_bs_ack_pc_on_pilot_only.wav",
    )?;
    let observed = rc3_pre_viterbi_softs(
        &qpsk_on,
        DEFAULT_LONG_CODE_MASK,
        DEFAULT_LONG_CODE_STATE,
        0,
        LongCodeMode::OddUsesRawPreviousChip,
        InterleaverMode::FbbrDecode,
        PcMode::ErasurePuncture,
        Some(&rc3_pc_positions(
            DEFAULT_LONG_CODE_MASK,
            DEFAULT_LONG_CODE_STATE,
            0,
        )),
    )
    .ok_or_else(|| Error::from("no pre-viterbi softs from locked MathWorks pc_on"))?;

    let expected_bits = parse_bit_string(EXPECTED_BS_ACK_INFO_BITS);
    let lc_decimated = rc3_decimated_lc_bits(DEFAULT_LONG_CODE_MASK, DEFAULT_LONG_CODE_STATE, 0);

    let mut best: Option<(f32, usize, [usize; 4], [usize; PCGS_PER_FRAME])> = None;
    for window_start in 0..=(SYMBOLS_PER_PCG - 4) {
        for perm in NIBBLE_PERMUTATIONS {
            let pc_positions = rc3_pc_positions_with_selector(&lc_decimated, window_start, perm);
            let expected =
                expected_deinterleaved_softs_with_pc_positions(&expected_bits, &pc_positions);
            let mae = mean_abs_error(&observed, &expected);
            if best
                .as_ref()
                .is_none_or(|(best_mae, _, _, _)| mae < *best_mae)
            {
                best = Some((mae, window_start, perm, pc_positions));
            }
        }
    }

    let (best_mae, best_window, best_perm, best_positions) =
        best.ok_or_else(|| Error::from("no brute-force PC selector candidates"))?;
    eprintln!(
        "mathworks_pc_selector_bruteforce_best: mae={} window_start={} perm={:?} positions={:?}",
        best_mae, best_window, best_perm, best_positions
    );

    let spec_positions = rc3_pc_positions(DEFAULT_LONG_CODE_MASK, DEFAULT_LONG_CODE_STATE, 0);
    let spec_expected =
        expected_deinterleaved_softs_with_pc_positions(&expected_bits, &spec_positions);
    let spec_mae = mean_abs_error(&observed, &spec_expected);
    eprintln!(
        "mathworks_pc_selector_spec: mae={} window_start=44 perm={:?} positions={:?}",
        spec_mae,
        [3usize, 2usize, 1usize, 0usize],
        spec_positions
    );

    Ok(())
}

#[test]
fn test_mathworks_generated_ffch_rc3_bs_ack_pc_on_extract_pcbs_hypotheses_diagnostic()
-> Result<(), Error> {
    let expected = alternating_power_control_bits();
    let qpsk_on = load_locked_mathworks_generated_ffch_rc3_qpsk_symbols(
        "ffch_rc3_bs_ack_pc_on.wav",
        "ffch_rc3_bs_ack_pc_on_pilot_only.wav",
    )?;
    let raw_scalars = qpsk_symbols_to_scalar_values(&qpsk_on);
    let descrambled_scalars = rc3_scalar_lc_descramble(
        &raw_scalars,
        DEFAULT_LONG_CODE_MASK,
        DEFAULT_LONG_CODE_STATE,
        0,
        LongCodeMode::OddUsesRawPreviousChip,
    );
    let pc_positions = rc3_pc_positions(DEFAULT_LONG_CODE_MASK, DEFAULT_LONG_CODE_STATE, 0);

    let raw_sum = extract_pcbs_from_scalar_values_sum(&raw_scalars, &pc_positions);
    let raw_majority = extract_pcbs_from_scalar_values_majority(&raw_scalars, &pc_positions);
    let descrambled_sum = extract_pcbs_from_scalar_values_sum(&descrambled_scalars, &pc_positions);
    let descrambled_majority =
        extract_pcbs_from_scalar_values_majority(&descrambled_scalars, &pc_positions);

    eprintln!("expected_pcbs={}", bits_to_string(&expected));
    eprintln!(
        "mathworks_raw_sum={} matches={}/{}",
        bits_to_string(&raw_sum),
        matching_bits(&raw_sum, &expected),
        PCGS_PER_FRAME
    );
    eprintln!(
        "mathworks_raw_majority={} matches={}/{}",
        bits_to_string(&raw_majority),
        matching_bits(&raw_majority, &expected),
        PCGS_PER_FRAME
    );
    eprintln!(
        "mathworks_descrambled_sum={} matches={}/{}",
        bits_to_string(&descrambled_sum),
        matching_bits(&descrambled_sum, &expected),
        PCGS_PER_FRAME
    );
    eprintln!(
        "mathworks_descrambled_majority={} matches={}/{}",
        bits_to_string(&descrambled_majority),
        matching_bits(&descrambled_majority, &expected),
        PCGS_PER_FRAME
    );

    Ok(())
}

#[test]
fn capture_local_generated_ffch_rc3_bs_ack_pc_on_extract_pcbs_hypotheses_diagnostic()
-> Result<(), Error> {
    let expected = alternating_power_control_bits();
    let (ffch_path, pilot_path) = local_generated_wav_pair("bs_ack_pc_on_with_lc", false)?;
    let qpsk_on = load_generated_ffch_rc3_qpsk_symbols_with_pilot_reference(
        &ffch_path,
        &pilot_path,
        DEFAULT_WALSH_CODE,
        false,
        SampleTransform::Conjugate,
    )?;
    let raw_scalars = qpsk_symbols_to_scalar_values(&qpsk_on);
    let descrambled_scalars = rc3_scalar_lc_descramble(
        &raw_scalars,
        DEFAULT_LONG_CODE_MASK,
        DEFAULT_LONG_CODE_STATE,
        0,
        LongCodeMode::OddUsesRawPreviousChip,
    );
    let pc_positions = rc3_pc_positions(DEFAULT_LONG_CODE_MASK, DEFAULT_LONG_CODE_STATE, 0);

    let raw_sum = extract_pcbs_from_scalar_values_sum(&raw_scalars, &pc_positions);
    let raw_majority = extract_pcbs_from_scalar_values_majority(&raw_scalars, &pc_positions);
    let descrambled_sum = extract_pcbs_from_scalar_values_sum(&descrambled_scalars, &pc_positions);
    let descrambled_majority =
        extract_pcbs_from_scalar_values_majority(&descrambled_scalars, &pc_positions);

    eprintln!("expected_pcbs={}", bits_to_string(&expected));
    eprintln!(
        "local_raw_sum={} matches={}/{}",
        bits_to_string(&raw_sum),
        matching_bits(&raw_sum, &expected),
        PCGS_PER_FRAME
    );
    eprintln!(
        "local_raw_majority={} matches={}/{}",
        bits_to_string(&raw_majority),
        matching_bits(&raw_majority, &expected),
        PCGS_PER_FRAME
    );
    eprintln!(
        "local_descrambled_sum={} matches={}/{}",
        bits_to_string(&descrambled_sum),
        matching_bits(&descrambled_sum, &expected),
        PCGS_PER_FRAME
    );
    eprintln!(
        "local_descrambled_majority={} matches={}/{}",
        bits_to_string(&descrambled_majority),
        matching_bits(&descrambled_majority, &expected),
        PCGS_PER_FRAME
    );

    Ok(())
}

#[test]
fn test_mathworks_generated_ffch_rc3_bs_ack_pc_on_bruteforce_pcb_extraction_frontend()
-> Result<(), Error> {
    let expected = alternating_power_control_bits();
    let despread = load_mathworks_generated_ffch_rc3_pilot_referenced_chips(
        "ffch_rc3_bs_ack_pc_on.wav",
        "ffch_rc3_bs_ack_pc_on_pilot_only.wav",
    )?;
    let mut best: Option<(
        usize,
        bool,
        SampleTransform,
        bool,
        usize,
        [u8; PCGS_PER_FRAME],
    )> = None;

    for walsh_chip_phase in 0..64usize {
        for invert_q in [false, true] {
            let qpsk_base =
                dewalsh_rc3_symbols_with_phase(&despread, 4, invert_q, walsh_chip_phase);
            for qpsk_transform in [
                SampleTransform::Identity,
                SampleTransform::Conjugate,
                SampleTransform::NegateI,
                SampleTransform::NegateQ,
                SampleTransform::SwapIq,
                SampleTransform::SwapIqNegateI,
                SampleTransform::SwapIqNegateQ,
                SampleTransform::NegateBoth,
            ] {
                let qpsk = transform_samples(&qpsk_base, qpsk_transform);
                let raw_scalars = qpsk_symbols_to_scalar_values(&qpsk);
                let pc_positions =
                    rc3_pc_positions(DEFAULT_LONG_CODE_MASK, DEFAULT_LONG_CODE_STATE, 0);

                for use_majority in [false, true] {
                    let observed = if use_majority {
                        extract_pcbs_from_scalar_values_majority(&raw_scalars, &pc_positions)
                    } else {
                        extract_pcbs_from_scalar_values_sum(&raw_scalars, &pc_positions)
                    };
                    let matches = matching_bits(&observed, &expected);
                    if best
                        .as_ref()
                        .is_none_or(|(_, _, _, _, best_matches, _)| matches > *best_matches)
                    {
                        best = Some((
                            walsh_chip_phase,
                            invert_q,
                            qpsk_transform,
                            use_majority,
                            matches,
                            observed,
                        ));
                    }
                }
            }
        }
    }

    let (walsh_chip_phase, invert_q, qpsk_transform, use_majority, matches, observed) =
        best.ok_or_else(|| Error::from("no PCB frontend candidates produced"))?;
    eprintln!(
        "mathworks_pcb_frontend_best: walsh_chip_phase={} invert_q={} qpsk_transform={:?} combiner={} matches={}/{} observed={}",
        walsh_chip_phase,
        invert_q,
        qpsk_transform,
        if use_majority { "majority" } else { "sum" },
        matches,
        PCGS_PER_FRAME,
        bits_to_string(&observed)
    );

    Ok(())
}

#[test]
fn test_mathworks_generated_ffch_rc3_bs_ack_pc_on_bruteforce_pcb_lc_epoch() -> Result<(), Error> {
    let expected = alternating_power_control_bits();
    let qpsk = load_mathworks_generated_ffch_rc3_qpsk_symbols_with_phase_and_transform(
        "ffch_rc3_bs_ack_pc_on.wav",
        "ffch_rc3_bs_ack_pc_on_pilot_only.wav",
        4,
        false,
        0,
        SampleTransform::NegateBoth,
    )?;
    let raw_scalars = qpsk_symbols_to_scalar_values(&qpsk);

    let mut best: Option<(u64, usize, [u8; PCGS_PER_FRAME])> = None;
    for frame_chip_start in 0..64u64 {
        let pc_positions = rc3_pc_positions(
            DEFAULT_LONG_CODE_MASK,
            DEFAULT_LONG_CODE_STATE,
            frame_chip_start,
        );
        let observed = extract_pcbs_from_scalar_values_sum(&raw_scalars, &pc_positions);
        let matches = matching_bits(&observed, &expected);
        if best
            .as_ref()
            .is_none_or(|(_, best_matches, _)| matches > *best_matches)
        {
            best = Some((frame_chip_start, matches, observed));
        }
    }

    let (frame_chip_start, matches, observed) =
        best.ok_or_else(|| Error::from("no PCB LC epoch candidates produced"))?;
    eprintln!(
        "mathworks_pcb_lc_epoch_best: frame_chip_start={} matches={}/{} observed={}",
        frame_chip_start,
        matches,
        PCGS_PER_FRAME,
        bits_to_string(&observed)
    );

    Ok(())
}

#[test]
fn test_mathworks_generated_ffch_rc3_bs_ack_pc_on_bruteforce_full_pcb_hypothesis()
-> Result<(), Error> {
    let expected = alternating_power_control_bits();
    let despread = load_mathworks_generated_ffch_rc3_pilot_referenced_chips(
        "ffch_rc3_bs_ack_pc_on.wav",
        "ffch_rc3_bs_ack_pc_on_pilot_only.wav",
    )?;
    let mut best: Option<(
        usize,
        bool,
        SampleTransform,
        u64,
        usize,
        [u8; PCGS_PER_FRAME],
    )> = None;

    for walsh_chip_phase in 0..64usize {
        for invert_q in [false, true] {
            let qpsk_base =
                dewalsh_rc3_symbols_with_phase(&despread, 4, invert_q, walsh_chip_phase);
            for qpsk_transform in [
                SampleTransform::Identity,
                SampleTransform::Conjugate,
                SampleTransform::NegateI,
                SampleTransform::NegateQ,
                SampleTransform::SwapIq,
                SampleTransform::SwapIqNegateI,
                SampleTransform::SwapIqNegateQ,
                SampleTransform::NegateBoth,
            ] {
                let qpsk = transform_samples(&qpsk_base, qpsk_transform);
                let raw_scalars = qpsk_symbols_to_scalar_values(&qpsk);
                for frame_chip_start in 0..64u64 {
                    let pc_positions = rc3_pc_positions(
                        DEFAULT_LONG_CODE_MASK,
                        DEFAULT_LONG_CODE_STATE,
                        frame_chip_start,
                    );
                    let observed = extract_pcbs_from_scalar_values_sum(&raw_scalars, &pc_positions);
                    let matches = matching_bits(&observed, &expected);
                    if best
                        .as_ref()
                        .is_none_or(|(_, _, _, _, best_matches, _)| matches > *best_matches)
                    {
                        best = Some((
                            walsh_chip_phase,
                            invert_q,
                            qpsk_transform,
                            frame_chip_start,
                            matches,
                            observed,
                        ));
                    }
                }
            }
        }
    }

    let (walsh_chip_phase, invert_q, qpsk_transform, frame_chip_start, matches, observed) =
        best.ok_or_else(|| Error::from("no full PCB hypothesis candidates produced"))?;
    eprintln!(
        "mathworks_full_pcb_hypothesis_best: walsh_chip_phase={} invert_q={} qpsk_transform={:?} frame_chip_start={} matches={}/{} observed={}",
        walsh_chip_phase,
        invert_q,
        qpsk_transform,
        frame_chip_start,
        matches,
        PCGS_PER_FRAME,
        bits_to_string(&observed)
    );

    Ok(())
}

#[test]
fn test_mathworks_generated_ffch_rc3_bs_ack_pc_on_bruteforce_puncture_subset_combiner()
-> Result<(), Error> {
    let expected = alternating_power_control_bits();
    let qpsk = load_mathworks_generated_ffch_rc3_qpsk_symbols_with_phase_and_transform(
        "ffch_rc3_bs_ack_pc_on.wav",
        "ffch_rc3_bs_ack_pc_on_pilot_only.wav",
        4,
        false,
        0,
        SampleTransform::NegateBoth,
    )?;
    let raw_scalars = qpsk_symbols_to_scalar_values(&qpsk);
    let pc_positions = rc3_pc_positions(DEFAULT_LONG_CODE_MASK, DEFAULT_LONG_CODE_STATE, 43);

    let mut best: Option<(u8, usize, [u8; PCGS_PER_FRAME])> = None;
    for mask in 1u8..(1u8 << PC_PUNCTURE_SYMBOLS) {
        let observed =
            extract_pcbs_from_scalar_values_subset_sum(&raw_scalars, &pc_positions, mask);
        let matches = matching_bits(&observed, &expected);
        if best
            .as_ref()
            .is_none_or(|(_, best_matches, _)| matches > *best_matches)
        {
            best = Some((mask, matches, observed));
        }
    }

    let (mask, matches, observed) =
        best.ok_or_else(|| Error::from("no puncture subset candidates produced"))?;
    eprintln!(
        "mathworks_puncture_subset_best: mask={:04b} matches={}/{} observed={}",
        mask,
        matches,
        PCGS_PER_FRAME,
        bits_to_string(&observed)
    );

    Ok(())
}

#[test]
fn test_mathworks_generated_ffch_rc3_bs_ack_pc_on_per_pcg_window_diagnostic() -> Result<(), Error> {
    let expected = alternating_power_control_bits();
    let qpsk_on = load_mathworks_generated_ffch_rc3_qpsk_symbols_with_phase_and_transform(
        "ffch_rc3_bs_ack_pc_on.wav",
        "ffch_rc3_bs_ack_pc_on_pilot_only.wav",
        4,
        false,
        0,
        SampleTransform::NegateBoth,
    )?;
    let qpsk_off = load_mathworks_generated_ffch_rc3_qpsk_symbols_with_phase_and_transform(
        "ffch_rc3_bs_ack.wav",
        "ffch_rc3_bs_ack_pilot_only.wav",
        4,
        false,
        0,
        SampleTransform::NegateBoth,
    )?;
    let raw_on = qpsk_symbols_to_scalar_values(&qpsk_on);
    let raw_off = qpsk_symbols_to_scalar_values(&qpsk_off);
    let pc_positions = rc3_pc_positions(DEFAULT_LONG_CODE_MASK, DEFAULT_LONG_CODE_STATE, 43);
    let observed = extract_pcbs_from_scalar_values_sum(&raw_on, &pc_positions);

    eprintln!(
        "per_pcg_window_diag: expected={} observed={} positions={:?}",
        bits_to_string(&expected),
        bits_to_string(&observed),
        pc_positions
    );

    for pcg in 0..PCGS_PER_FRAME {
        let base = pcg * SYMBOLS_PER_PCG;
        let start = pc_positions[pcg];
        let expected_bit = expected[pcg];
        let observed_bit = observed[pcg];
        let block_on = (0..PC_PUNCTURE_SYMBOLS)
            .map(|k| raw_on[base + start + k])
            .collect::<Vec<_>>();
        let block_off = (0..PC_PUNCTURE_SYMBOLS)
            .map(|k| raw_off[base + start + k])
            .collect::<Vec<_>>();
        let metric = block_on.iter().copied().sum::<f32>();
        let delta_metric = block_on
            .iter()
            .zip(block_off.iter())
            .map(|(on, off)| on - off)
            .sum::<f32>();

        let mut best_start = 0usize;
        let mut best_metric = f32::NEG_INFINITY;
        let mut best_block = Vec::new();
        for candidate_start in 0..=(SYMBOLS_PER_PCG - PC_PUNCTURE_SYMBOLS) {
            let candidate_metric = (0..PC_PUNCTURE_SYMBOLS)
                .map(|k| raw_on[base + candidate_start + k])
                .sum::<f32>();
            let signed_score = if expected_bit == 0 {
                candidate_metric
            } else {
                -candidate_metric
            };
            if signed_score > best_metric {
                best_metric = signed_score;
                best_start = candidate_start;
                best_block = (0..PC_PUNCTURE_SYMBOLS)
                    .map(|k| raw_on[base + candidate_start + k])
                    .collect::<Vec<_>>();
            }
        }

        eprintln!(
            "pcg={} exp={} obs={} start={} block_on={:?} block_off={:?} sum={:+.3} delta_sum={:+.3} best_start={} best_block={:?} best_signed_score={:+.3}",
            pcg,
            expected_bit,
            observed_bit,
            start,
            block_on,
            block_off,
            metric,
            delta_metric,
            best_start,
            best_block,
            best_metric
        );
    }

    Ok(())
}

#[test]
fn test_mathworks_generated_ffch_rc3_bs_ack_pc_on_locked_positions_diagnostic() -> Result<(), Error>
{
    let expected = alternating_power_control_bits();
    let qpsk_on = load_locked_mathworks_generated_ffch_rc3_qpsk_symbols(
        "ffch_rc3_bs_ack_pc_on.wav",
        "ffch_rc3_bs_ack_pc_on_pilot_only.wav",
    )?;
    let qpsk_off = load_locked_mathworks_generated_ffch_rc3_qpsk_symbols(
        "ffch_rc3_bs_ack.wav",
        "ffch_rc3_bs_ack_pilot_only.wav",
    )?;
    let raw_on = qpsk_symbols_to_scalar_values(&qpsk_on);
    let raw_off = qpsk_symbols_to_scalar_values(&qpsk_off);
    let pc_positions = rc3_pc_positions(DEFAULT_LONG_CODE_MASK, DEFAULT_LONG_CODE_STATE, 0);
    let observed = extract_pcbs_from_scalar_values_sum(&raw_on, &pc_positions);

    eprintln!(
        "locked_positions_diag: expected={} observed={} positions={:?}",
        bits_to_string(&expected),
        bits_to_string(&observed),
        pc_positions
    );

    for pcg in 0..PCGS_PER_FRAME {
        let base = pcg * SYMBOLS_PER_PCG;
        let start = pc_positions[pcg];
        let expected_bit = expected[pcg];
        let observed_bit = observed[pcg];
        let block_on = (0..PC_PUNCTURE_SYMBOLS)
            .map(|k| raw_on[base + start + k])
            .collect::<Vec<_>>();
        let block_off = (0..PC_PUNCTURE_SYMBOLS)
            .map(|k| raw_off[base + start + k])
            .collect::<Vec<_>>();
        let metric = block_on.iter().copied().sum::<f32>();
        let delta_metric = block_on
            .iter()
            .zip(block_off.iter())
            .map(|(on, off)| on - off)
            .sum::<f32>();
        if expected_bit != observed_bit || delta_metric.abs() > 1e-3 {
            eprintln!(
                "pcg={} exp={} obs={} start={} block_on={:?} block_off={:?} sum={:+.3} delta_sum={:+.3}",
                pcg, expected_bit, observed_bit, start, block_on, block_off, metric, delta_metric
            );
        }
    }

    Ok(())
}

#[test]
fn test_mathworks_generated_ffch_rc3_bs_ack_pc_on_bruteforce_delta_window_positions()
-> Result<(), Error> {
    let qpsk_on = load_locked_mathworks_generated_ffch_rc3_qpsk_symbols(
        "ffch_rc3_bs_ack_pc_on.wav",
        "ffch_rc3_bs_ack_pc_on_pilot_only.wav",
    )?;
    let qpsk_off = load_locked_mathworks_generated_ffch_rc3_qpsk_symbols(
        "ffch_rc3_bs_ack.wav",
        "ffch_rc3_bs_ack_pilot_only.wav",
    )?;
    let raw_on = qpsk_symbols_to_scalar_values(&qpsk_on);
    let raw_off = qpsk_symbols_to_scalar_values(&qpsk_off);

    let mut detected = [0usize; PCGS_PER_FRAME];
    let mut detected_scores = [0.0f32; PCGS_PER_FRAME];
    for pcg in 0..PCGS_PER_FRAME {
        let base = pcg * SYMBOLS_PER_PCG;
        let mut best_start = 0usize;
        let mut best_score = f32::NEG_INFINITY;
        for candidate_start in 0..=(SYMBOLS_PER_PCG - PC_PUNCTURE_SYMBOLS) {
            let score = (0..PC_PUNCTURE_SYMBOLS)
                .map(|k| {
                    (raw_on[base + candidate_start + k] - raw_off[base + candidate_start + k]).abs()
                })
                .sum::<f32>();
            if score > best_score {
                best_score = score;
                best_start = candidate_start;
            }
        }
        detected[pcg] = best_start;
        detected_scores[pcg] = best_score;
    }

    let mut best_match: Option<(u64, usize, [usize; PCGS_PER_FRAME])> = None;
    for frame_chip_start in 0..64u64 {
        let predicted = rc3_pc_positions(
            DEFAULT_LONG_CODE_MASK,
            DEFAULT_LONG_CODE_STATE,
            frame_chip_start,
        );
        let matches = predicted
            .iter()
            .zip(detected.iter())
            .filter(|(a, b)| a == b)
            .count();
        if best_match
            .as_ref()
            .is_none_or(|(_, best_matches, _)| matches > *best_matches)
        {
            best_match = Some((frame_chip_start, matches, predicted));
        }
    }

    let (best_frame_chip_start, best_matches, best_predicted) =
        best_match.ok_or_else(|| Error::from("no delta-window LC epoch candidates produced"))?;
    eprintln!(
        "mathworks_delta_windows: detected={:?} scores={:?}",
        detected, detected_scores
    );
    eprintln!(
        "mathworks_delta_windows_best_epoch: frame_chip_start={} matches={}/{} predicted={:?}",
        best_frame_chip_start, best_matches, PCGS_PER_FRAME, best_predicted
    );

    Ok(())
}

#[test]
fn test_mathworks_generated_ffch_rc3_bs_ack_pc_on_bruteforce_pcg_shift() -> Result<(), Error> {
    let expected = alternating_power_control_bits();
    let qpsk_on = load_locked_mathworks_generated_ffch_rc3_qpsk_symbols(
        "ffch_rc3_bs_ack_pc_on.wav",
        "ffch_rc3_bs_ack_pc_on_pilot_only.wav",
    )?;
    let raw_on = qpsk_symbols_to_scalar_values(&qpsk_on);
    let base_positions = rc3_pc_positions(DEFAULT_LONG_CODE_MASK, DEFAULT_LONG_CODE_STATE, 0);

    let mut best: Option<(
        usize,
        bool,
        usize,
        [usize; PCGS_PER_FRAME],
        [u8; PCGS_PER_FRAME],
    )> = None;
    for shift in 0..PCGS_PER_FRAME {
        for rotate_right in [false, true] {
            let mut shifted = [0usize; PCGS_PER_FRAME];
            for pcg in 0..PCGS_PER_FRAME {
                let src = if rotate_right {
                    (pcg + PCGS_PER_FRAME - shift) % PCGS_PER_FRAME
                } else {
                    (pcg + shift) % PCGS_PER_FRAME
                };
                shifted[pcg] = base_positions[src];
            }
            let observed = extract_pcbs_from_scalar_values_sum(&raw_on, &shifted);
            let matches = matching_bits(&observed, &expected);
            if best
                .as_ref()
                .is_none_or(|(_, _, best_matches, _, _)| matches > *best_matches)
            {
                best = Some((shift, rotate_right, matches, shifted, observed));
            }
        }
    }

    let (shift, rotate_right, matches, shifted, observed) =
        best.ok_or_else(|| Error::from("no PCG-shift candidates produced"))?;
    eprintln!(
        "mathworks_pcg_shift_best: shift={} direction={} matches={}/{} shifted_positions={:?} observed={}",
        shift,
        if rotate_right { "right" } else { "left" },
        matches,
        PCGS_PER_FRAME,
        shifted,
        bits_to_string(&observed)
    );

    assert_eq!(
        shift, 1,
        "expected MathWorks PCB positions to be delayed by one PCG"
    );
    assert!(
        rotate_right,
        "expected MathWorks PCB positions to match a one-PCG right rotation"
    );
    assert_eq!(
        matches, PCGS_PER_FRAME,
        "expected exact alternating PCB recovery"
    );
    assert_eq!(observed, expected, "expected recovered MathWorks PCB bits");

    Ok(())
}

#[test]
fn capture_local_generated_ffch_rc3_bs_ack_pc_on_bruteforce_pcg_shift() -> Result<(), Error> {
    let expected = alternating_power_control_bits();
    let (ffch_path, pilot_path) = local_generated_wav_pair("bs_ack_pc_on_with_lc", false)?;
    let qpsk_on = load_generated_ffch_rc3_qpsk_symbols_with_pilot_reference(
        &ffch_path,
        &pilot_path,
        DEFAULT_WALSH_CODE,
        false,
        SampleTransform::Conjugate,
    )?;
    let raw_on = qpsk_symbols_to_scalar_values(&qpsk_on);
    let base_positions = rc3_pc_positions(DEFAULT_LONG_CODE_MASK, DEFAULT_LONG_CODE_STATE, 0);

    let mut best: Option<(
        usize,
        bool,
        usize,
        [usize; PCGS_PER_FRAME],
        [u8; PCGS_PER_FRAME],
    )> = None;
    for shift in 0..PCGS_PER_FRAME {
        for rotate_right in [false, true] {
            let mut shifted = [0usize; PCGS_PER_FRAME];
            for pcg in 0..PCGS_PER_FRAME {
                let src = if rotate_right {
                    (pcg + PCGS_PER_FRAME - shift) % PCGS_PER_FRAME
                } else {
                    (pcg + shift) % PCGS_PER_FRAME
                };
                shifted[pcg] = base_positions[src];
            }
            let observed = extract_pcbs_from_scalar_values_sum(&raw_on, &shifted);
            let matches = matching_bits(&observed, &expected);
            if best
                .as_ref()
                .is_none_or(|(_, _, best_matches, _, _)| matches > *best_matches)
            {
                best = Some((shift, rotate_right, matches, shifted, observed));
            }
        }
    }

    let (shift, rotate_right, matches, shifted, observed) =
        best.ok_or_else(|| Error::from("no local PCG-shift candidates produced"))?;
    eprintln!(
        "local_pcg_shift_best: shift={} direction={} matches={}/{} shifted_positions={:?} observed={}",
        shift,
        if rotate_right { "right" } else { "left" },
        matches,
        PCGS_PER_FRAME,
        shifted,
        bits_to_string(&observed)
    );

    assert_eq!(
        shift, 1,
        "expected local PCB positions to use the preceding PCG selector"
    );
    assert!(rotate_right, "expected a one-PCG right rotation");
    assert_eq!(matches, PCGS_PER_FRAME, "expected exact local PCB recovery");
    assert_eq!(observed, expected, "expected recovered local PCB bits");

    Ok(())
}

#[test]
fn test_mathworks_generated_ffch_rc3_all_zero_pc_on_bruteforce_pcg_shift() -> Result<(), Error> {
    let expected = alternating_power_control_bits();
    let qpsk_on = load_locked_mathworks_generated_ffch_rc3_qpsk_symbols(
        "ffch_rc3_all_zero_pc_on.wav",
        "ffch_rc3_all_zero_pc_on_pilot_only.wav",
    )?;
    let raw_on = qpsk_symbols_to_scalar_values(&qpsk_on);
    let base_positions = rc3_pc_positions(DEFAULT_LONG_CODE_MASK, DEFAULT_LONG_CODE_STATE, 0);

    let mut best: Option<(
        usize,
        bool,
        usize,
        [usize; PCGS_PER_FRAME],
        [u8; PCGS_PER_FRAME],
    )> = None;
    for shift in 0..PCGS_PER_FRAME {
        for rotate_right in [false, true] {
            let mut shifted = [0usize; PCGS_PER_FRAME];
            for pcg in 0..PCGS_PER_FRAME {
                let src = if rotate_right {
                    (pcg + PCGS_PER_FRAME - shift) % PCGS_PER_FRAME
                } else {
                    (pcg + shift) % PCGS_PER_FRAME
                };
                shifted[pcg] = base_positions[src];
            }
            let observed = extract_pcbs_from_scalar_values_sum(&raw_on, &shifted);
            let matches = matching_bits(&observed, &expected);
            if best
                .as_ref()
                .is_none_or(|(_, _, best_matches, _, _)| matches > *best_matches)
            {
                best = Some((shift, rotate_right, matches, shifted, observed));
            }
        }
    }

    let (shift, rotate_right, matches, shifted, observed) =
        best.ok_or_else(|| Error::from("no all-zero MathWorks PCG-shift candidates produced"))?;
    eprintln!(
        "mathworks_all_zero_pcg_shift_best: shift={} direction={} matches={}/{} shifted_positions={:?} observed={}",
        shift,
        if rotate_right { "right" } else { "left" },
        matches,
        PCGS_PER_FRAME,
        shifted,
        bits_to_string(&observed)
    );

    assert_eq!(
        shift, 1,
        "expected MathWorks all-zero PCB positions to be delayed by one PCG"
    );
    assert!(
        rotate_right,
        "expected MathWorks all-zero PCB positions to match a one-PCG right rotation"
    );
    assert_eq!(
        matches, PCGS_PER_FRAME,
        "expected exact alternating PCB recovery"
    );
    assert_eq!(
        observed, expected,
        "expected recovered MathWorks all-zero PCB bits"
    );

    Ok(())
}

#[test]
fn test_rc3_pc_positions_previous_pcg_pipeline_model_is_not_simple_lc_epoch_shift()
-> Result<(), Error> {
    let base = rc3_pc_positions(DEFAULT_LONG_CODE_MASK, DEFAULT_LONG_CODE_STATE, 0);
    let previous_pcg = rc3_pc_positions(
        DEFAULT_LONG_CODE_MASK,
        DEFAULT_LONG_CODE_STATE,
        LONG_CODE_PERIOD - ((SYMBOLS_PER_PCG * LC_DECIMATION) as u64),
    );
    let mut delayed = [0usize; PCGS_PER_FRAME];
    delayed[0] = 0;
    delayed[1..PCGS_PER_FRAME].copy_from_slice(&base[..(PCGS_PER_FRAME - 1)]);
    let mut rotated = [0usize; PCGS_PER_FRAME];
    for pcg in 0..PCGS_PER_FRAME {
        rotated[pcg] = base[(pcg + PCGS_PER_FRAME - 1) % PCGS_PER_FRAME];
    }

    eprintln!(
        "pc_positions_pipeline_model: base={:?} rotated={:?} delayed={:?} previous_pcg={:?}",
        base, rotated, delayed, previous_pcg
    );

    assert_ne!(
        previous_pcg, rotated,
        "expected one-PCG-earlier LC origin to differ from a pure within-frame right rotation"
    );
    assert_ne!(
        previous_pcg, delayed,
        "expected one-PCG-earlier LC origin to differ from the delayed-selector pipeline model"
    );
    assert_eq!(
        delayed[1..],
        base[..(PCGS_PER_FRAME - 1)],
        "expected delayed-selector model to use the previous current-frame selector for PCGs 1..15"
    );

    Ok(())
}

#[test]
#[ignore = "diagnostic: sweep real RC3 LC descramble modes on the locked bs_ack front end"]
fn test_decode_mathworks_generated_ffch_rc3_bs_ack_with_locked_frontend_and_lc_descrambling()
-> Result<(), Error> {
    let qpsk_symbols = load_locked_mathworks_generated_ffch_rc3_qpsk_symbols(
        "ffch_rc3_bs_ack.wav",
        "ffch_rc3_bs_ack_pilot_only.wav",
    )?;
    let expected_bits = parse_bit_string(EXPECTED_BS_ACK_INFO_BITS);
    let mut best: Option<(u64, LongCodeMode, u64, usize, bool, bool, bool, String)> = None;

    for frame_chip_start_base in [0u64, 32_767, 32_768, 32_769] {
        for lc_mode in [
            LongCodeMode::OnePerModSymbol,
            LongCodeMode::OddUsesPairStart,
            LongCodeMode::OddUsesRawPreviousChip,
        ] {
            for lc_chip_offset in [0u64, 1, 2, 3, 31, 32, 33, 63] {
                let frame_chip_start = frame_chip_start_base + lc_chip_offset;
                let Some(attempt) = decode_rc3_signaling_frame(
                    &qpsk_symbols,
                    DEFAULT_LONG_CODE_MASK,
                    DEFAULT_LONG_CODE_STATE,
                    frame_chip_start,
                    lc_mode,
                    InterleaverMode::FbbrDecode,
                    PcMode::Disabled,
                ) else {
                    eprintln!(
                        "locked_bs_ack_lc: frame_chip_start_base={} lc_mode={:?} lc_chip_offset={} -> no decode",
                        frame_chip_start_base, lc_mode, lc_chip_offset
                    );
                    continue;
                };

                let mismatch = hamming_distance(&attempt.info_bits, &expected_bits);
                eprintln!(
                    "locked_bs_ack_lc: frame_chip_start_base={} lc_mode={:?} lc_chip_offset={} frame_chip_start={} mismatch={} ftch_crc_ok={} fdsch_crc_ok={} tail_ok={} prefix={}",
                    frame_chip_start_base,
                    lc_mode,
                    lc_chip_offset,
                    frame_chip_start,
                    mismatch,
                    attempt.ftch_crc_ok,
                    attempt.fdsch_crc_ok,
                    attempt.tail_ok,
                    bit_prefix(&attempt.info_bits, 48),
                );

                if best.as_ref().is_none_or(
                    |(_, _, _, best_mismatch, best_ftch, best_fdsch, _, _)| {
                        (mismatch, !attempt.ftch_crc_ok, !attempt.fdsch_crc_ok)
                            < (*best_mismatch, !*best_ftch, !*best_fdsch)
                    },
                ) {
                    best = Some((
                        frame_chip_start_base,
                        lc_mode,
                        lc_chip_offset,
                        mismatch,
                        attempt.ftch_crc_ok,
                        attempt.fdsch_crc_ok,
                        attempt.tail_ok,
                        bit_prefix(&attempt.info_bits, 48),
                    ));
                }
            }
        }
    }

    let (
        best_base,
        best_mode,
        best_offset,
        best_mismatch,
        best_ftch,
        best_fdsch,
        best_tail,
        best_prefix,
    ) = best.expect("at least one LC-descrambled decode candidate");
    eprintln!(
        "locked_bs_ack_lc_best: frame_chip_start_base={} lc_mode={:?} lc_chip_offset={} frame_chip_start={} mismatch={} ftch_crc_ok={} fdsch_crc_ok={} tail_ok={} prefix={}",
        best_base,
        best_mode,
        best_offset,
        best_base + best_offset,
        best_mismatch,
        best_ftch,
        best_fdsch,
        best_tail,
        best_prefix,
    );

    Ok(())
}

#[test]
#[ignore = "diagnostic: broader search around the locked MathWorks RC3 all-zero pilot-referenced decode path"]
fn test_decode_mathworks_generated_ffch_rc3_all_zero_with_pilot_reference() -> Result<(), Error> {
    let base = test_iq_dir();
    let ffch_path = base.join("ffch_rc3_all_zero.wav");
    let pilot_path = base.join("ffch_rc3_all_zero_pilot_only.wav");
    if !ffch_path.exists() || !pilot_path.exists() {
        eprintln!(
            "skipping: {} or {} not found",
            ffch_path.display(),
            pilot_path.display()
        );
        return Ok(());
    }

    let (ffch_sample_rate, ffch_iq_samples) = load_wav_iq_samples(&ffch_path)?;
    let (pilot_sample_rate, pilot_iq_samples) = load_wav_iq_samples(&pilot_path)?;
    assert_eq!(ffch_sample_rate, CHIP_RATE * 4);
    assert_eq!(pilot_sample_rate, CHIP_RATE * 4);

    let ffch_chip_rate = decimate_pick_phase(&ffch_iq_samples, 0);
    let pilot_chip_rate = decimate_pick_phase(&pilot_iq_samples, 0);
    assert!(ffch_chip_rate.len() >= FRAME_CHIPS);
    assert!(pilot_chip_rate.len() >= FRAME_CHIPS);

    let expected_info_bits = vec![0u8; EXPECTED_INFO_BITS_LEN];
    let mut best_candidate: Option<BestCandidate> = None;

    for chip_offset in 0..8usize {
        let ffch_chips = chip_window_padded(&ffch_chip_rate, chip_offset, FRAME_CHIPS);
        let pilot_chips = chip_window_padded(&pilot_chip_rate, chip_offset, FRAME_CHIPS);
        let despread = pilot_reference_despread(&ffch_chips, &pilot_chips);

        for invert_q in [false, true] {
            let walsh_rows = top_walsh_rows_from_despread(&despread, invert_q, 8);
            eprintln!(
                "pilot_ref_walsh_rows: chip_offset={} invert_q={} top_rows={:?}",
                chip_offset, invert_q, walsh_rows
            );
            for walsh_code in [4u8, 5u8, 6u8, 7u8] {
                let qpsk_symbols = dewalsh_rc3_symbols(&despread, walsh_code, invert_q);

                for qpsk_transform in [
                    SampleTransform::Identity,
                    SampleTransform::Conjugate,
                    SampleTransform::NegateI,
                    SampleTransform::NegateQ,
                    SampleTransform::SwapIq,
                    SampleTransform::SwapIqNegateI,
                    SampleTransform::SwapIqNegateQ,
                    SampleTransform::NegateBoth,
                ] {
                    let qpsk_symbols = transform_samples(&qpsk_symbols, qpsk_transform);

                    for lc_mode in [
                        LongCodeMode::None,
                        LongCodeMode::OnePerModSymbol,
                        LongCodeMode::OddUsesPairStart,
                        LongCodeMode::OddUsesRawPreviousChip,
                    ] {
                        for interleaver_mode in [
                            InterleaverMode::FbbrDecode,
                            InterleaverMode::FbbrEncode,
                            InterleaverMode::BitReverseDecode,
                            InterleaverMode::Identity,
                        ] {
                            let lc_chip_offsets: &[u64] = if matches!(lc_mode, LongCodeMode::None) {
                                &[0]
                            } else {
                                &[0, 1, 2, 3, 31, 32, 33, 63]
                            };
                            for &lc_chip_offset in lc_chip_offsets {
                                if let Some(attempt) = decode_rc3_signaling_frame(
                                    &qpsk_symbols,
                                    DEFAULT_LONG_CODE_MASK,
                                    DEFAULT_LONG_CODE_STATE,
                                    chip_offset as u64 + lc_chip_offset,
                                    lc_mode,
                                    interleaver_mode,
                                    PcMode::Disabled,
                                ) {
                                    let mismatch =
                                        hamming_distance(&attempt.info_bits, &expected_info_bits);
                                    let candidate = BestCandidate {
                                        sample_phase: 0,
                                        chip_offset,
                                        lc_chip_offset,
                                        walsh_code,
                                        pn_chip_offset: 0,
                                        sample_transform: qpsk_transform,
                                        pn_mode: PnMode::RepoConvention,
                                        lc_mode,
                                        interleaver_mode,
                                        pc_mode: PcMode::Disabled,
                                        invert_q,
                                        mismatch,
                                        ftch_crc_ok: attempt.ftch_crc_ok,
                                        tail_ok: attempt.tail_ok,
                                        fdsch_crc_ok: attempt.fdsch_crc_ok,
                                        prefix: bit_prefix(&attempt.info_bits, 32),
                                    };
                                    if best_candidate.as_ref().is_none_or(|best| {
                                        (
                                            candidate.mismatch,
                                            !candidate.ftch_crc_ok,
                                            !candidate.fdsch_crc_ok,
                                        ) < (best.mismatch, !best.ftch_crc_ok, !best.fdsch_crc_ok)
                                    }) {
                                        best_candidate = Some(candidate);
                                    }

                                    if attempt.ftch_crc_ok
                                        && attempt.tail_ok
                                        && attempt.info_bits == expected_info_bits
                                    {
                                        eprintln!(
                                            "pilot_ref_decode: chip_offset={} walsh_code={} lc_chip_offset={} invert_q={} qpsk_transform={:?} lc_mode={:?} interleaver_mode={:?}",
                                            chip_offset,
                                            walsh_code,
                                            lc_chip_offset,
                                            invert_q,
                                            qpsk_transform,
                                            lc_mode,
                                            interleaver_mode
                                        );
                                        return Ok(());
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
    }

    if let Some(best) = best_candidate {
        return Err(format!(
            "pilot-ref RC3 decode failed; best mismatch={} chip_offset={} walsh_code={} lc_chip_offset={} invert_q={} qpsk_transform={:?} lc_mode={:?} interleaver_mode={:?} prefix={} ftch_crc_ok={} tail_ok={} fdsch_crc_ok={}",
            best.mismatch,
            best.chip_offset,
            best.walsh_code,
            best.lc_chip_offset,
            best.invert_q,
            best.sample_transform,
            best.lc_mode,
            best.interleaver_mode,
            best.prefix,
            best.ftch_crc_ok,
            best.tail_ok,
            best.fdsch_crc_ok,
        )
        .into());
    }

    Err("pilot-ref RC3 decode produced no candidates".into())
}
