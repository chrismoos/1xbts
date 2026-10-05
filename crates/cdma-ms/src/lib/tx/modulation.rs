use cdma_bts::phy::coding::block_interleaver::{self, BitReversalInterleaver};
use cdma_bts::phy::coding::convolutional::get_1_3_k9_encoder;
use cdma_bts::phy::coding::symbol_repeat::SymbolRepetition;
use cdma_bts::phy::spread::PnSequence;
use cdma_bts::phy::walsh::WalshGenerator;
use cdma_bts::sdr::cdma2000_baseband_filter_taps_f64;
use cdma_bts::sdr::fir::ComplexFir32;
use cdma_common::phy::long_code::LongCodeGenerator;
use num_complex::Complex32;

/// Information bits carried per 20 ms access frame (before the 8 tail bits).
pub(super) const ACCESS_INFO_BITS: usize = 88;
/// Convolutional encoder tail (K-1 = 8 zeros to terminate to the zero state).
const TAIL_BITS: usize = 8;
/// Walsh chips per 20 ms frame (96 code symbols × 64 Walsh chips).
const WALSH_CHIPS_PER_FRAME: usize = 6144;
/// PN chips per Walsh chip.
const PN_CHIPS_PER_WALSH_CHIP: usize = 4;
pub(super) const CHIPS_PER_FRAME: usize = WALSH_CHIPS_PER_FRAME * PN_CHIPS_PER_WALSH_CHIP;
const PN_PERIOD: usize = 32_768;
const WALSH_ORDER: usize = 64;

/// Access-channel long-code and short-PN parameters (must match the BTS's
/// `ReverseAccessSettings`).
#[derive(Debug, Clone)]
pub struct AccessChannelConfig {
    pub access_channel_number: u8,
    pub paging_channel: u8,
    pub base_id: u16,
    pub pilot_pn: u16,
    pub oversample: usize,
}

impl Default for AccessChannelConfig {
    fn default() -> Self {
        AccessChannelConfig {
            access_channel_number: 0,
            paging_channel: 1,
            base_id: 1,
            pilot_pn: 0,
            oversample: 4,
        }
    }
}

/// Encode one 20 ms access frame's `88` info bits into `6144` bipolar Walsh
/// chips: append tail, R=1/3 K=9 convolutional encode, 2× repeat, 576-symbol
/// bit-reversal interleave, 64-ary Walsh modulation.
fn frame_walsh_chips(info_bits: &[u8], walsh_matrix: &[[i8; WALSH_ORDER]; WALSH_ORDER]) -> Vec<i8> {
    debug_assert_eq!(info_bits.len(), ACCESS_INFO_BITS);
    let mut frame_bits = Vec::with_capacity(ACCESS_INFO_BITS + TAIL_BITS);
    frame_bits.extend_from_slice(info_bits);
    frame_bits.extend(std::iter::repeat_n(0u8, TAIL_BITS));

    let mut enc = get_1_3_k9_encoder();
    let mut code_symbols = Vec::with_capacity(frame_bits.len() * 3);
    for &bit in &frame_bits {
        code_symbols.extend_from_slice(&enc.encode(bit));
    }

    let mut sr = SymbolRepetition::new(2);
    for &sym in &code_symbols {
        sr.feed(sym);
    }
    let repeated = sr.take_all();

    let interleaved =
        BitReversalInterleaver::new(block_interleaver::SR1_PARAMS_576).encode(&repeated);

    let mut walsh_chips = Vec::with_capacity(WALSH_CHIPS_PER_FRAME);
    for group in interleaved.chunks_exact(6) {
        let index = group[0] as usize
            + 2 * group[1] as usize
            + 4 * group[2] as usize
            + 8 * group[3] as usize
            + 16 * group[4] as usize
            + 32 * group[5] as usize;
        walsh_chips.extend_from_slice(&walsh_matrix[index]);
    }
    walsh_chips
}

/// C.S0002-E §2.1.3.1.16.
const LONG_CODE_PERIOD_CHIPS: u64 = (1u64 << 42) - 1;

/// The public long code register at system time chip `chip` for a base
/// station whose register was `state` at chip `state_chip` (the sync
/// channel's LC_STATE at its validity point).
pub fn lc_state_at(state: u64, state_chip: u64, chip: u64) -> u64 {
    let mut lc = LongCodeGenerator::new(0);
    lc.set_state(state);
    // Sync can announce a long-code state whose validity point is still in the future.
    let delta = (chip % LONG_CODE_PERIOD_CHIPS + LONG_CODE_PERIOD_CHIPS
        - state_chip % LONG_CODE_PERIOD_CHIPS)
        % LONG_CODE_PERIOD_CHIPS;
    lc.advance_chips(delta as usize);
    lc.state()
}

pub fn default_lc_state_at(chip: u64) -> u64 {
    lc_state_at(1u64 << 41, 0, chip)
}

/// Returns unshaped IQ at `cfg.oversample`. Apply `pulse_shape` before RF transmission.
pub fn modulate_access_probe(
    cfg: &AccessChannelConfig,
    capsule_bits: &[u8],
    frame_start_chip: u64,
    lc_state: u64,
    preamble_frames: usize,
    trailing_frames: usize,
) -> Vec<Complex32> {
    let os = cfg.oversample;
    let walsh_matrix = WalshGenerator::generate_matrix::<WALSH_ORDER>();

    let mut lc = LongCodeGenerator::new_access_channel_with_state(
        cfg.access_channel_number,
        cfg.paging_channel,
        cfg.base_id,
        cfg.pilot_pn,
        lc_state,
    );

    let data_frames: Vec<Vec<u8>> = capsule_bits
        .chunks(ACCESS_INFO_BITS)
        .map(|chunk| {
            let mut f = chunk.to_vec();
            f.resize(ACCESS_INFO_BITS, 0);
            f
        })
        .collect();

    let total_frames = preamble_frames + data_frames.len() + trailing_frames;
    let total_chips = total_frames * CHIPS_PER_FRAME;

    let phase_period = PN_PERIOD * os;
    let mut pn_src = PnSequence::new_repeat(0, PN_PERIOD, os.saturating_sub(1));
    let pn_period: Vec<Complex32> = (0..phase_period).map(|_| pn_src.generate_iq()).collect();
    let pn_rotate = (frame_start_chip as usize * os) % phase_period;
    let mut pn_idx = pn_rotate;
    let mut next_pn = || {
        let s = pn_period[pn_idx % phase_period];
        pn_idx += 1;
        s
    };

    let mut tx_raw: Vec<Complex32> = Vec::with_capacity(total_chips * os);

    // C.S0002-E §2.1.3.1.18.1 applies chip impulses to the baseband filters.
    let mut emit_walsh_chip = |w: f32, tx_raw: &mut Vec<Complex32>, lc: &mut LongCodeGenerator| {
        for _ in 0..PN_CHIPS_PER_WALSH_CHIP {
            let lc_sign = if lc.next_chip() == 1 { -1.0f32 } else { 1.0f32 };
            for sample in 0..os {
                let pn = next_pn();
                tx_raw.push(if sample == 0 {
                    Complex32::new(w * lc_sign * pn.re, w * lc_sign * pn.im)
                } else {
                    Complex32::new(0.0, 0.0)
                });
            }
        }
    };

    for _ in 0..preamble_frames {
        for _ in 0..WALSH_CHIPS_PER_FRAME {
            emit_walsh_chip(1.0, &mut tx_raw, &mut lc);
        }
    }
    for frame in &data_frames {
        for &wchip in &frame_walsh_chips(frame, &walsh_matrix) {
            emit_walsh_chip(wchip as f32, &mut tx_raw, &mut lc);
        }
    }
    for _ in 0..trailing_frames {
        for _ in 0..WALSH_CHIPS_PER_FRAME {
            emit_walsh_chip(1.0, &mut tx_raw, &mut lc);
        }
    }

    offset_quadrature(&mut tx_raw, os);
    tx_raw
}

/// Delays the Q arm by half a PN chip and applies the reverse-link IQ sign.
/// The Access Channel uses offset-QPSK spreading (C.S0002-E §2.1.3.1.17).
fn offset_quadrature(samples: &mut [Complex32], oversample: usize) {
    assert_eq!(
        0,
        oversample % 2,
        "the half-chip Q delay needs an even oversample"
    );
    let q_delay_samples = oversample / 2;
    for k in (0..samples.len()).rev() {
        samples[k].im = if k >= q_delay_samples {
            -samples[k - q_delay_samples].im
        } else {
            0.0
        };
    }
}

pub fn pulse_shape(samples: &[Complex32]) -> Vec<Complex32> {
    let taps = cdma2000_baseband_filter_taps_f64();
    ComplexFir32::new(&taps).process_block(samples)
}
