//! R-PICH uses the I arm and R-FCH uses the Q arm with Walsh W(4,16).

use num::complex::Complex32;

use cdma_common::bits::Bitstream;

use crate::phy::coding::block_interleaver::{self, BitReversalInterleaver};
use crate::phy::coding::convolutional::get_1_4_k9_encoder;
use crate::phy::coding::long_code::LongCodeGenerator;
use crate::phy::spread::PnSequence;
use crate::phy::walsh::WalshGenerator;

pub const RC3_FRAME_CHIPS: usize = 24_576;
/// PN chips per R-FCH modulation symbol (Walsh W(4,16) cover length).
pub const RC3_CHIPS_PER_SYMBOL: usize = 16;
pub const RC3_MOD_SYMBOLS_PER_FRAME: usize = 1536;

// C.S0002-E Table 2.1.2.3.3.7-1, 20 ms convolutionally coded R-FCH relative to R-PICH.
const FULL_RATE_FCH_GAIN_DB: f32 = 3.75;
const HALF_RATE_FCH_GAIN_DB: f32 = -0.25;
const QUARTER_RATE_FCH_GAIN_DB: f32 = -2.75;
const EIGHTH_RATE_FCH_GAIN_DB: f32 = -5.875;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rc3Rate {
    Full,
    Half,
    Quarter,
    Eighth,
}

impl Rc3Rate {
    fn fch_amplitude(self) -> f32 {
        let gain_db = match self {
            Self::Full => FULL_RATE_FCH_GAIN_DB,
            Self::Half => HALF_RATE_FCH_GAIN_DB,
            Self::Quarter => QUARTER_RATE_FCH_GAIN_DB,
            Self::Eighth => EIGHTH_RATE_FCH_GAIN_DB,
        };
        10.0_f32.powf(gain_db / 20.0)
    }

    pub const fn info_bits(self) -> usize {
        match self {
            Self::Full => 172,
            Self::Half => 80,
            Self::Quarter => 40,
            Self::Eighth => 16,
        }
    }

    pub const fn fqi_bits(self) -> usize {
        match self {
            Self::Full => 12,
            Self::Half => 8,
            Self::Quarter | Self::Eighth => 6,
        }
    }

    pub const fn frame_bits(self) -> usize {
        match self {
            Self::Full => 192,
            Self::Half => 96,
            Self::Quarter => 54,
            Self::Eighth => 30,
        }
    }

    pub const fn repetition_factor(self) -> usize {
        match self {
            Self::Full => 2,
            Self::Half => 4,
            Self::Quarter => 8,
            Self::Eighth => 16,
        }
    }

    pub const fn rate_bps(self) -> i64 {
        match self {
            Self::Full => 9600,
            Self::Half => 4800,
            Self::Quarter => 2700,
            Self::Eighth => 1500,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rc3Frame {
    PilotOnly,
    Traffic { rate: Rc3Rate, info_bits: Vec<u8> },
}

fn rc3_crc12(data: &[u8]) -> u16 {
    let poly: u16 = 0x0F13;
    let mut register: u16 = 0x0FFF;
    for &bit in data {
        let feedback = ((register >> 11) & 1) ^ (bit as u16 & 1);
        register = (register << 1) & 0x0FFF;
        if feedback == 1 {
            register ^= poly;
        }
    }
    register
}

fn rc3_crc8(data: &[u8]) -> u8 {
    let poly: u8 = 0x9B;
    let mut register: u8 = 0xFF;
    for &bit in data {
        let feedback = ((register >> 7) & 1) ^ (bit & 1);
        register <<= 1;
        if feedback == 1 {
            register ^= poly;
        }
    }
    register
}

fn rc3_crc6(data: &[u8]) -> u8 {
    let poly: u8 = 0x27;
    let mut register: u8 = 0x3F;
    for &bit in data {
        let feedback = ((register >> 5) & 1) ^ (bit & 1);
        register = (register << 1) & 0x3F;
        if feedback == 1 {
            register ^= poly;
        }
    }
    register
}

/// Build the `frame_bits` of a reverse RC3 frame: `info | FQI CRC | 8 tail`.
pub fn build_reverse_rc3_frame_bits(info_bits: &[u8], rate: Rc3Rate) -> Vec<u8> {
    let mut frame = Vec::with_capacity(rate.frame_bits());
    for i in 0..rate.info_bits() {
        frame.push(*info_bits.get(i).unwrap_or(&0));
    }

    match rate.fqi_bits() {
        12 => {
            let crc = rc3_crc12(&frame[..rate.info_bits()]);
            for bit in (0..12).rev() {
                frame.push(((crc >> bit) & 1) as u8);
            }
        }
        8 => {
            let crc = rc3_crc8(&frame[..rate.info_bits()]);
            for bit in (0..8).rev() {
                frame.push(((crc >> bit) & 1) as u8);
            }
        }
        6 => {
            let crc = rc3_crc6(&frame[..rate.info_bits()]);
            for bit in (0..6).rev() {
                frame.push(((crc >> bit) & 1) as u8);
            }
        }
        _ => unreachable!(),
    }

    frame.extend(std::iter::repeat_n(0u8, 8));
    assert_eq!(frame.len(), rate.frame_bits());
    frame
}

fn puncture_reverse_rc3_repeated_symbols(symbols: &[u8]) -> Vec<u8> {
    let input_len = symbols.len();
    (0..RC3_MOD_SYMBOLS_PER_FRAME)
        .map(|k| symbols[(k * input_len) / RC3_MOD_SYMBOLS_PER_FRAME])
        .collect()
}

/// Encode a reverse RC3 frame's info bits into `1536` bipolar R-FCH modulation
/// symbols (`+1.0`/`-1.0`).
pub fn encode_reverse_rc3_fch_symbols(info_bits: &[u8], rate: Rc3Rate) -> Vec<f32> {
    let frame_bits = build_reverse_rc3_frame_bits(info_bits, rate);
    encode_reverse_rc3_fch_frame_bits_to_symbols(&frame_bits, rate)
}

pub fn encode_reverse_rc3_fch_frame_bits_to_symbols(frame_bits: &[u8], rate: Rc3Rate) -> Vec<f32> {
    assert_eq!(frame_bits.len(), rate.frame_bits());
    let mut encoder = get_1_4_k9_encoder();
    let mut code_symbols = Vec::with_capacity(frame_bits.len() * 4);
    for &bit in frame_bits {
        code_symbols.extend_from_slice(&encoder.encode(bit));
    }

    let repeated: Vec<u8> = code_symbols
        .iter()
        .flat_map(|&symbol| std::iter::repeat_n(symbol, rate.repetition_factor()))
        .collect();

    let punctured = match rate {
        Rc3Rate::Full | Rc3Rate::Half => repeated,
        Rc3Rate::Quarter | Rc3Rate::Eighth => puncture_reverse_rc3_repeated_symbols(&repeated),
    };
    assert_eq!(punctured.len(), RC3_MOD_SYMBOLS_PER_FRAME);

    let mut interleaver = BitReversalInterleaver::new(block_interleaver::SR1_PARAMS_1536);
    interleaver
        .encode(&punctured)
        .into_iter()
        .map(|bit| if bit == 0 { 1.0 } else { -1.0 })
        .collect()
}

/// C.S0002-E §2.1.3.1.17 samples Q PN and delayed long code on the first
/// chip of each frame-aligned W(1,2) period and holds them for both chips.
pub struct Rc3HpskSpreader {
    lc_gen: LongCodeGenerator,
    prev_lc: f32,
    chip_count: usize,
    dec_pn_q: f32,
    dec_lc_q: f32,
}

impl Rc3HpskSpreader {
    pub fn with_state(esn: u32, state: u64) -> Self {
        let mut lc_gen = LongCodeGenerator::new_traffic_channel_with_state(esn, state);
        let prev_lc = if lc_gen.next_chip() == 1 { -1.0 } else { 1.0 };
        Self {
            lc_gen,
            prev_lc,
            chip_count: 0,
            dec_pn_q: 1.0,
            dec_lc_q: 1.0,
        }
    }

    pub fn next_spread_reference(&mut self, pn_at_chip_start: Complex32) -> Complex32 {
        let lc_i = if self.lc_gen.next_chip() == 1 {
            -1.0
        } else {
            1.0
        };
        let pn_i = pn_at_chip_start.re;
        let pn_q = pn_at_chip_start.im;
        let w12 = if self.chip_count % 2 == 0 { 1.0 } else { -1.0 };

        if self.chip_count % 2 == 0 {
            self.dec_pn_q = pn_q;
            self.dec_lc_q = self.prev_lc;
        }

        let s_i = pn_i * lc_i;
        let s_q = w12 * s_i * self.dec_pn_q * self.dec_lc_q;
        self.prev_lc = lc_i;
        self.chip_count += 1;
        Complex32::new(s_i, s_q)
    }
}

/// Fragment a reverse dedicated-channel signaling PDU into a single full-rate
/// R-FCH frame's `172` info bits. Panics if the PDU is too large for one frame.
pub fn encapsulate_reverse_rc3_full_rate_info_bits(mut pdu: Bitstream) -> Vec<u8> {
    while pdu.len() % 8 != 0 {
        pdu.write_u8(0, 1);
    }
    let frames = crate::lac::sar_fragment_ftch_pdu_dsch(&pdu);
    assert_eq!(
        frames.len(),
        1,
        "reverse RC3 signaling frame unexpectedly fragmented",
    );
    let bits = frames[0].bits().to_vec();
    assert_eq!(bits.len(), Rc3Rate::Full.info_bits());
    bits
}

pub fn advance_reverse_rc3_traffic_long_code_state(esn: u32, state: u64, chips: usize) -> u64 {
    let mut lc = LongCodeGenerator::new_traffic_channel_with_state(esn, state);
    lc.advance_chips(chips);
    lc.state()
}

/// Returns rectangular chip samples. Apply the baseband filter before RF transmission.
pub fn build_reverse_rc3_frame_samples(
    frame: &Rc3Frame,
    esn: u32,
    long_code_state: u64,
    pn_chip_offset: usize,
    oversample: usize,
    quadrature_polarity: f32,
) -> Vec<Complex32> {
    let fch_amplitude = match frame {
        Rc3Frame::PilotOnly => 0.0,
        Rc3Frame::Traffic { rate, .. } => rate.fch_amplitude(),
    };
    let encoded_symbols = match frame {
        Rc3Frame::PilotOnly => None,
        Rc3Frame::Traffic { rate, info_bits } => {
            Some(encode_reverse_rc3_fch_symbols(info_bits, *rate))
        }
    };

    let mut pn = PnSequence::new_repeat(0, 32768, oversample.saturating_sub(1));
    pn.advance_chips((oversample as u64) * (pn_chip_offset as u64 % 32768));
    let walsh_cover = WalshGenerator::generate_matrix::<16>()[4];
    let mut spreader = Rc3HpskSpreader::with_state(esn, long_code_state);
    let mut iq_samples = Vec::with_capacity(RC3_FRAME_CHIPS * oversample);

    for chip_idx in 0..RC3_FRAME_CHIPS {
        let prompt_pn = pn.generate_iq();
        for _ in 1..oversample {
            pn.generate_iq();
        }
        let spread_ref = spreader.next_spread_reference(prompt_pn);
        let desired_chip = match &encoded_symbols {
            None => Complex32::new(1.0, 0.0),
            Some(symbols) => {
                let symbol_idx = chip_idx / RC3_CHIPS_PER_SYMBOL;
                let walsh_chip = walsh_cover[chip_idx % RC3_CHIPS_PER_SYMBOL] as f32;
                Complex32::new(1.0, fch_amplitude * symbols[symbol_idx] * walsh_chip)
            }
        };
        let mut tx_chip = desired_chip * spread_ref;
        // C.S0002-E Figure 2.1.3.1.1.1-22 uses +sin. A -sin mixer needs conjugated I/Q.
        tx_chip.im *= quadrature_polarity;
        iq_samples.extend(std::iter::repeat_n(tx_chip, oversample));
    }

    iq_samples
}

#[cfg(test)]
mod tests {
    use super::*;

    const ESN: u32 = 0x1234_5678;
    const LC_STATE: u64 = 0x123_4567_89ab;

    fn spec_reference(chips: usize) -> Vec<Complex32> {
        let mut lc = LongCodeGenerator::new_traffic_channel_with_state(ESN, LC_STATE);
        let long_code: Vec<f32> = (0..=chips)
            .map(|_| if lc.next_chip() == 0 { 1.0 } else { -1.0 })
            .collect();
        let mut pn = PnSequence::new_repeat(0, 32768, 0);
        let pn: Vec<_> = (0..chips).map(|_| pn.generate_iq()).collect();
        (0..chips)
            .map(|chip| {
                let pair_start = chip / 2 * 2;
                let i = pn[chip].re * long_code[chip + 1];
                let held = pn[pair_start].im * long_code[pair_start];
                let walsh = if chip % 2 == 0 { 1.0 } else { -1.0 };
                Complex32::new(i, walsh * i * held)
            })
            .collect()
    }

    #[test]
    fn reverse_rc3_pilot_matches_negative_sine_rf_mapping() {
        let reference = spec_reference(RC3_FRAME_CHIPS);
        let samples =
            build_reverse_rc3_frame_samples(&Rc3Frame::PilotOnly, ESN, LC_STATE, 0, 1, -1.0);
        for (chip, (sample, expected)) in samples.iter().zip(reference).enumerate() {
            assert_eq!(*sample, expected.conj(), "pilot chip {chip}");
        }
    }

    #[test]
    fn reverse_rc3_rf_mapping_conjugates_pilot_and_fch_together() {
        let frame = Rc3Frame::Traffic {
            rate: Rc3Rate::Full,
            info_bits: vec![0; Rc3Rate::Full.info_bits()],
        };
        let positive_sine = build_reverse_rc3_frame_samples(&frame, ESN, LC_STATE, 0, 1, 1.0);
        let negative_sine = build_reverse_rc3_frame_samples(&frame, ESN, LC_STATE, 0, 1, -1.0);
        for (chip, (positive, negative)) in positive_sine.iter().zip(negative_sine).enumerate() {
            assert_eq!(positive.conj(), negative, "traffic chip {chip}");
        }
    }

    #[test]
    fn reverse_rc3_fch_power_matches_nominal_attribute_gain_table() {
        let reference =
            build_reverse_rc3_frame_samples(&Rc3Frame::PilotOnly, ESN, LC_STATE, 0, 1, -1.0);
        for (rate, expected_db) in [
            (Rc3Rate::Full, 3.75_f64),
            (Rc3Rate::Half, -0.25),
            (Rc3Rate::Quarter, -2.75),
            (Rc3Rate::Eighth, -5.875),
        ] {
            let frame = Rc3Frame::Traffic {
                rate,
                info_bits: vec![0; rate.info_bits()],
            };
            let samples = build_reverse_rc3_frame_samples(&frame, ESN, LC_STATE, 0, 1, -1.0);
            let mut pilot_power = 0.0_f64;
            let mut fch_power = 0.0_f64;
            for (sample, spreading) in samples.iter().zip(&reference) {
                let despread = sample * spreading.conj() / spreading.norm_sqr();
                pilot_power += f64::from(despread.re).powi(2);
                fch_power += f64::from(despread.im).powi(2);
            }
            let gain_db = 10.0 * (fch_power / pilot_power).log10();
            assert!(
                (gain_db - expected_db).abs() < 1e-5,
                "{rate:?}: {gain_db} dB, expected {expected_db}"
            );
            assert!((pilot_power / RC3_FRAME_CHIPS as f64 - 1.0).abs() < 1e-6);
        }
    }
}
