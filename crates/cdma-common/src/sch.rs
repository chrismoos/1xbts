//! Supplemental channel profile helpers shared across BSC, BTS, and packet code.

use crate::traffic::RC3_TRAFFIC_INITIAL_GAIN_LINEAR;

const F_SCH_GAIN_OFFSET_DB: f32 = 3.0;

/// Supported RC3 Forward Supplemental Channel rates for convolutional coding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rc3FschProfile {
    pub rate_bps: u32,
    pub info_bits: usize,
    pub walsh_len: usize,
    pub num_bits_idx: u8,
    pub mux_option: u16,
    pub coding_indicator: u8,
}

impl Rc3FschProfile {
    pub const fn from_rate_bps(rate_bps: u32) -> Option<Self> {
        match rate_bps {
            19_200 => Some(Self {
                rate_bps,
                info_bits: 360,
                walsh_len: 32,
                num_bits_idx: 0x1,
                mux_option: 0x0809,
                coding_indicator: 0,
            }),
            38_400 => Some(Self {
                rate_bps,
                info_bits: 744,
                walsh_len: 16,
                num_bits_idx: 0x2,
                mux_option: 0x0811,
                coding_indicator: 0,
            }),
            76_800 => Some(Self {
                rate_bps,
                info_bits: 1512,
                walsh_len: 8,
                num_bits_idx: 0x3,
                mux_option: 0x0821,
                coding_indicator: 0,
            }),
            153_600 => Some(Self {
                rate_bps,
                info_bits: 3048,
                walsh_len: 4,
                num_bits_idx: 0x4,
                mux_option: 0x0921,
                coding_indicator: 0,
            }),
            _ => None,
        }
    }

    pub const fn default_19k2() -> Self {
        Self {
            rate_bps: 19_200,
            info_bits: 360,
            walsh_len: 32,
            num_bits_idx: 0x1,
            mux_option: 0x0809,
            coding_indicator: 0,
        }
    }

    pub const fn frame_bits(self) -> usize {
        self.info_bits + 16 + 8
    }

    pub const fn coded_symbols(self) -> usize {
        self.frame_bits() * 4
    }

    pub const fn qpsk_symbols(self) -> usize {
        self.coded_symbols() / 2
    }

    pub const fn symbols_per_pcg(self) -> usize {
        self.coded_symbols() / 16
    }

    pub const fn lc_decimation(self) -> usize {
        self.walsh_len / 2
    }

    pub const fn rate_label(self) -> &'static str {
        match self.rate_bps {
            19_200 => "19.2k",
            38_400 => "38.4k",
            76_800 => "76.8k",
            153_600 => "153.6k",
            _ => "unknown",
        }
    }

    pub fn nominal_gain_linear(self) -> f32 {
        let rate_scale = (self.rate_bps as f32 / 19_200.0).sqrt();
        let gain_offset = 10.0_f32.powf(F_SCH_GAIN_OFFSET_DB / 20.0);
        RC3_TRAFFIC_INITIAL_GAIN_LINEAR * rate_scale * gain_offset
    }
}

pub const DEFAULT_RC3_F_SCH_RATE_BPS: u32 = 19_200;
