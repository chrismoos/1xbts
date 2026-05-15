//! CDMA2000 band class + channel number to RF frequency mapping per
//! 3GPP2 C.S0057-F. BC0/BC1 enforce full per-subclass Valid +
//! Conditionally Valid tables; BC2–BC22 enforce only the outer
//! channel-number range. BC17 and BC22 are "Not specified" in the spec
//! and `validate()` rejects them.
use crate::error::Error;
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BandClass {
    /// 800 MHz Cellular.
    Bc0,
    /// 1900 MHz PCS.
    Bc1,
    /// TACS Band.
    Bc2,
    /// JTACS Band.
    Bc3,
    /// Korean PCS.
    Bc4,
    /// 450 MHz NMT.
    Bc5,
    /// 2 GHz IMT-2000.
    Bc6,
    /// Upper 700 MHz.
    Bc7,
    /// 1800 MHz.
    Bc8,
    /// 900 MHz.
    Bc9,
    /// Secondary 800 MHz.
    Bc10,
    /// 400 MHz European PAMR.
    Bc11,
    /// 800 MHz PAMR.
    Bc12,
    /// 2.5 GHz IMT-2000 Extension.
    Bc13,
    /// US PCS Extension.
    Bc14,
    /// AWS.
    Bc15,
    /// US 2.5 GHz.
    Bc16,
    /// US 2.5 GHz Forward Link Only. Not specified per C.S0057-F §2.1.18.
    Bc17,
    /// Public Safety 700 MHz.
    Bc18,
    /// Lower 700 MHz.
    Bc19,
    /// L-Band.
    Bc20,
    /// S-Band MSS.
    Bc21,
    /// Mobile Satellite System Band. Not specified per C.S0057-F §2.1.23.
    Bc22,
}

impl BandClass {
    pub fn as_str(&self) -> &'static str {
        match self {
            BandClass::Bc0 => "BC0",
            BandClass::Bc1 => "BC1",
            BandClass::Bc2 => "BC2",
            BandClass::Bc3 => "BC3",
            BandClass::Bc4 => "BC4",
            BandClass::Bc5 => "BC5",
            BandClass::Bc6 => "BC6",
            BandClass::Bc7 => "BC7",
            BandClass::Bc8 => "BC8",
            BandClass::Bc9 => "BC9",
            BandClass::Bc10 => "BC10",
            BandClass::Bc11 => "BC11",
            BandClass::Bc12 => "BC12",
            BandClass::Bc13 => "BC13",
            BandClass::Bc14 => "BC14",
            BandClass::Bc15 => "BC15",
            BandClass::Bc16 => "BC16",
            BandClass::Bc17 => "BC17",
            BandClass::Bc18 => "BC18",
            BandClass::Bc19 => "BC19",
            BandClass::Bc20 => "BC20",
            BandClass::Bc21 => "BC21",
            BandClass::Bc22 => "BC22",
        }
    }

    /// 5-bit `BAND_CLASS` field (C.S0057-F Table 1.4-1).
    pub fn field_value(&self) -> u8 {
        match self {
            BandClass::Bc0 => 0,
            BandClass::Bc1 => 1,
            BandClass::Bc2 => 2,
            BandClass::Bc3 => 3,
            BandClass::Bc4 => 4,
            BandClass::Bc5 => 5,
            BandClass::Bc6 => 6,
            BandClass::Bc7 => 7,
            BandClass::Bc8 => 8,
            BandClass::Bc9 => 9,
            BandClass::Bc10 => 10,
            BandClass::Bc11 => 11,
            BandClass::Bc12 => 12,
            BandClass::Bc13 => 13,
            BandClass::Bc14 => 14,
            BandClass::Bc15 => 15,
            BandClass::Bc16 => 16,
            BandClass::Bc17 => 17,
            BandClass::Bc18 => 18,
            BandClass::Bc19 => 19,
            BandClass::Bc20 => 20,
            BandClass::Bc21 => 21,
            BandClass::Bc22 => 22,
        }
    }

    /// Highest legal subclass index per C.S0057-F. Bands with block
    /// designators but no numbered subclasses report 0.
    pub fn max_subclass(&self) -> u8 {
        match self {
            BandClass::Bc0 => 3,
            BandClass::Bc1 => 0,
            BandClass::Bc2 => 3,
            BandClass::Bc3 => 0,
            BandClass::Bc4 => 0,
            BandClass::Bc5 => 13,
            BandClass::Bc6 => 0,
            BandClass::Bc7 => 0,
            BandClass::Bc8 => 0,
            BandClass::Bc9 => 0,
            BandClass::Bc10 => 4,
            BandClass::Bc11 => 11,
            BandClass::Bc12 => 2,
            BandClass::Bc13 => 0,
            BandClass::Bc14 => 0,
            BandClass::Bc15 => 0,
            BandClass::Bc16 => 0,
            BandClass::Bc17 => 0,
            BandClass::Bc18 => 0,
            BandClass::Bc19 => 0,
            BandClass::Bc20 => 0,
            BandClass::Bc21 => 0,
            BandClass::Bc22 => 0,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Validity {
    Valid,
    /// Legal only when the licensee owns the adjacent block.
    Conditional,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChannelPlan {
    pub band_class: BandClass,
    /// 5-bit `BAND_SUBCLASS` broadcast in handoff messages; selects the
    /// channel-validity profile within the band.
    pub band_subclass: u8,
    pub cdma_channel: u16,
}

impl Default for ChannelPlan {
    fn default() -> Self {
        // 881.520 MHz TX / 836.520 MHz RX — historical hardcoded default.
        Self {
            band_class: BandClass::Bc0,
            band_subclass: 0,
            cdma_channel: 384,
        }
    }
}

impl ChannelPlan {
    pub const fn new(band_class: BandClass, band_subclass: u8, cdma_channel: u16) -> Self {
        Self {
            band_class,
            band_subclass,
            cdma_channel,
        }
    }

    /// Base TX (mobile RX) center frequency, in Hz.
    pub fn downlink_hz(&self) -> u64 {
        let n = self.cdma_channel;
        match self.band_class {
            BandClass::Bc0 => bc0_downlink_hz(n),
            BandClass::Bc1 => 1_930_000_000 + 50_000 * n as u64,
            BandClass::Bc2 => bc2_downlink_hz(n),
            BandClass::Bc3 => bc3_downlink_hz(n),
            BandClass::Bc4 => 1_840_000_000 + 50_000 * n as u64,
            BandClass::Bc5 => bc5_downlink_hz(n),
            BandClass::Bc6 => 2_110_000_000 + 50_000 * n as u64,
            BandClass::Bc7 => 746_000_000 + 50_000 * n as u64,
            BandClass::Bc8 => 1_805_000_000 + 50_000 * n as u64,
            BandClass::Bc9 => 925_000_000 + 50_000 * n as u64,
            BandClass::Bc10 => bc10_downlink_hz(n),
            // BC11 reuses BC5's channel formula (C.S0057-F §2.1.12).
            BandClass::Bc11 => bc5_downlink_hz(n),
            BandClass::Bc12 => 915_012_500 + 25_000 * n as u64,
            BandClass::Bc13 => 2_620_000_000 + 50_000 * n as u64,
            BandClass::Bc14 => 1_930_000_000 + 50_000 * n as u64,
            BandClass::Bc15 => 2_110_000_000 + 50_000 * n as u64,
            BandClass::Bc16 => 2_617_000_000 + 50_000 * n as u64,
            BandClass::Bc17 => 0,
            BandClass::Bc18 => 757_000_000 + 50_000 * n as u64,
            BandClass::Bc19 => 728_000_000 + 50_000 * n as u64,
            BandClass::Bc20 => 1_525_000_000 + 50_000 * n as u64,
            BandClass::Bc21 => bc21_downlink_hz(n),
            BandClass::Bc22 => 0,
        }
    }

    /// Base RX (mobile TX) center frequency, in Hz. BC3, BC7, BC18, BC20,
    /// and BC2 subclass 3 have mobile transmitting HIGHER than base.
    pub fn uplink_hz(&self) -> u64 {
        let n = self.cdma_channel;
        match self.band_class {
            BandClass::Bc0 => bc0_uplink_hz(n),
            BandClass::Bc1 => 1_850_000_000 + 50_000 * n as u64,
            BandClass::Bc2 => bc2_uplink_hz(self.band_subclass, n),
            BandClass::Bc3 => bc3_uplink_hz(n),
            BandClass::Bc4 => 1_750_000_000 + 50_000 * n as u64,
            BandClass::Bc5 => bc5_uplink_hz(n),
            BandClass::Bc6 => 1_920_000_000 + 50_000 * n as u64,
            BandClass::Bc7 => 776_000_000 + 50_000 * n as u64,
            BandClass::Bc8 => 1_710_000_000 + 50_000 * n as u64,
            BandClass::Bc9 => 880_000_000 + 50_000 * n as u64,
            BandClass::Bc10 => bc10_uplink_hz(n),
            BandClass::Bc11 => bc5_uplink_hz(n),
            BandClass::Bc12 => 870_012_500 + 25_000 * n as u64,
            BandClass::Bc13 => 2_500_000_000 + 50_000 * n as u64,
            BandClass::Bc14 => 1_850_000_000 + 50_000 * n as u64,
            BandClass::Bc15 => 1_710_000_000 + 50_000 * n as u64,
            BandClass::Bc16 => 2_495_000_000 + 50_000 * n as u64,
            BandClass::Bc17 => 0,
            BandClass::Bc18 => 787_000_000 + 50_000 * n as u64,
            BandClass::Bc19 => 698_000_000 + 50_000 * n as u64,
            BandClass::Bc20 => 1_626_500_000 + 50_000 * n as u64,
            BandClass::Bc21 => bc21_uplink_hz(n),
            BandClass::Bc22 => 0,
        }
    }

    /// Duplex offset magnitude in Hz; use `downlink_hz`/`uplink_hz` when
    /// the sign matters.
    pub fn duplex_offset_hz(&self) -> u64 {
        self.downlink_hz().abs_diff(self.uplink_hz())
    }

    /// 11-bit `CDMA_FREQ` field.
    pub fn cdma_freq_field(&self) -> u16 {
        self.cdma_channel & 0x7FF
    }

    pub fn channel_validity(&self) -> Option<Validity> {
        channel_validity(self.band_class, self.band_subclass, self.cdma_channel)
    }

    /// Emits a `WARN` log for Conditionally Valid channels.
    pub fn validate(&self) -> Result<(), Error> {
        if matches!(self.band_class, BandClass::Bc17 | BandClass::Bc22) {
            return Err(Error::from(format!(
                "{} is reserved / not specified per C.S0057-F",
                self.band_class.as_str()
            )));
        }
        if self.band_subclass > self.band_class.max_subclass() {
            return Err(Error::from(format!(
                "band_subclass {} is out of range for {} (max {})",
                self.band_subclass,
                self.band_class.as_str(),
                self.band_class.max_subclass()
            )));
        }
        match self.channel_validity() {
            Some(Validity::Valid) => Ok(()),
            Some(Validity::Conditional) => {
                log::warn!(
                    "channel plan: {} subclass {} channel {} is Conditionally Valid \
                     per C.S0057-F (legal only when the licensee owns the adjacent block)",
                    self.band_class.as_str(),
                    self.band_subclass,
                    self.cdma_channel
                );
                Ok(())
            }
            None => Err(Error::from(format!(
                "cdma_channel {} is not valid for {} subclass {}",
                self.cdma_channel,
                self.band_class.as_str(),
                self.band_subclass
            ))),
        }
    }
}

// ─── BC0 — C.S0057-F §2.1.1, Table 2.1.1-2 (three piecewise segments) ─────

fn bc0_downlink_hz(n: u16) -> u64 {
    let n = n as i64;
    let mhz_x_1000: i64 = if n <= 799 {
        30 * n + 870_000
    } else if n <= 1023 {
        30 * (n - 1023) + 870_000
    } else {
        30 * (n - 1024) + 860_040
    };
    (mhz_x_1000 as u64) * 1_000
}

fn bc0_uplink_hz(n: u16) -> u64 {
    let n = n as i64;
    let mhz_x_1000: i64 = if n <= 799 {
        30 * n + 825_000
    } else if n <= 1023 {
        30 * (n - 1023) + 825_000
    } else {
        30 * (n - 1024) + 815_040
    };
    (mhz_x_1000 as u64) * 1_000
}

// Table 2.1.1-3. No Conditionally Valid channels.
fn bc0_validity(sub: u8, n: u16) -> Option<Validity> {
    let valid = match sub {
        0 => matches!(
            n,
            1..=311 | 356..=644 | 689..=694 | 739..=777 | 1013..=1023
        ),
        1 => matches!(n, 1..=311 | 356..=644 | 689..=779 | 1013..=1023),
        2 => matches!(n, 1..=142 | 991..=1023),
        3 => matches!(n, 1..=142 | 991..=1023 | 1048..=1323),
        _ => false,
    };
    if valid { Some(Validity::Valid) } else { None }
}

// ─── BC1 — C.S0057-F §2.1.2, Table 2.1.2-3 ────────────────────────────────

fn bc1_validity(sub: u8, n: u16) -> Option<Validity> {
    if sub != 0 {
        return None;
    }
    match n {
        25..=275 | 325..=375 | 425..=675 | 725..=775 | 825..=875 | 925..=1175 => {
            Some(Validity::Valid)
        }
        276..=324 | 376..=424 | 676..=724 | 776..=824 | 876..=924 => Some(Validity::Conditional),
        _ => None,
    }
}

// ─── BC2 — C.S0057-F §2.1.3, Table 2.1.3-3 ────────────────────────────────
// Three piecewise segments; ATG block (2048–2108, subclass 3) reverses
// duplex direction (mobile HIGHER than base).

fn bc2_downlink_hz(n: u16) -> u64 {
    let n = n as i64;
    let hz: i64 = if n <= 1000 {
        25_000 * n + 934_987_500
    } else if n <= 2047 {
        25_000 * (n - 1328) + 916_987_500
    } else {
        25_000 * (n - 2048) + 849_000_000
    };
    hz as u64
}

fn bc2_uplink_hz(sub: u8, n: u16) -> u64 {
    let n_i = n as i64;
    let hz: i64 = if n_i <= 1000 {
        25_000 * n_i + 889_987_500
    } else if n_i <= 2047 {
        25_000 * (n_i - 1328) + 871_987_500
    } else if sub == 3 {
        // ATG: mobile +45 MHz (reverse duplex).
        25_000 * (n_i - 2048) + 894_000_000
    } else {
        // Unreachable: 2048–2108 only legal on subclass 3.
        25_000 * (n_i - 2048) + 804_000_000
    };
    hz as u64
}

fn bc2_validity(sub: u8, n: u16) -> Option<Validity> {
    let in_range = match sub {
        0 => n <= 600,
        1 => n <= 1000,
        2 => n <= 600 || (1329..=2047).contains(&n),
        3 => (2048..=2108).contains(&n),
        _ => false,
    };
    if in_range {
        Some(Validity::Valid)
    } else {
        None
    }
}

// ─── BC3 — C.S0057-F §2.1.4, Table 2.1.4-2 ────────────────────────────────
// Four piecewise segments; reverse duplex (+55 MHz); only even N valid.

fn bc3_downlink_hz(n: u16) -> u64 {
    let n = n as i64;
    let hz: i64 = if (1..=799).contains(&n) {
        12_500 * n + 860_000_000
    } else if (801..=1039).contains(&n) {
        12_500 * (n - 800) + 843_000_000
    } else if (1041..=1199).contains(&n) {
        12_500 * (n - 1040) + 832_000_000
    } else if (1201..=1600).contains(&n) {
        12_500 * (n - 1200) + 838_000_000
    } else {
        0
    };
    hz as u64
}

fn bc3_uplink_hz(n: u16) -> u64 {
    let n = n as i64;
    let hz: i64 = if (1..=799).contains(&n) {
        12_500 * n + 915_000_000
    } else if (801..=1039).contains(&n) {
        12_500 * (n - 800) + 898_000_000
    } else if (1041..=1199).contains(&n) {
        12_500 * (n - 1040) + 887_000_000
    } else if (1201..=1600).contains(&n) {
        12_500 * (n - 1200) + 893_000_000
    } else {
        0
    };
    hz as u64
}

fn bc3_validity(sub: u8, n: u16) -> Option<Validity> {
    if sub != 0 {
        return None;
    }
    if n % 2 != 0 {
        return None;
    }
    let in_range = (1..=799).contains(&n)
        || (801..=1039).contains(&n)
        || (1041..=1199).contains(&n)
        || (1201..=1600).contains(&n);
    if in_range {
        Some(Validity::Valid)
    } else {
        None
    }
}

// ─── BC4 — C.S0057-F §2.1.5 ───────────────────────────────────────────────

fn bc4_validity(sub: u8, n: u16) -> Option<Validity> {
    if sub != 0 || n > 599 {
        return None;
    }
    Some(Validity::Valid)
}

// ─── BC5 — C.S0057-F §2.1.6, Table 2.1.6-2 ────────────────────────────────
// Five piecewise segments + two singleton channels (N=2017, 2018).

fn bc5_downlink_hz(n: u16) -> u64 {
    if n == 2017 {
        return 467_725_000;
    }
    if n == 2018 {
        return 467_725_000;
    }
    let n = n as i64;
    let hz: i64 = if (1..=400).contains(&n) {
        25_000 * (n - 1) + 460_000_000
    } else if (472..=871).contains(&n) {
        25_000 * (n - 472) + 420_000_000
    } else if (1039..=1473).contains(&n) {
        20_000 * (n - 1024) + 461_010_000
    } else if (1536..=1715).contains(&n) {
        25_000 * (n - 1536) + 489_000_000
    } else if (1792..=2016).contains(&n) {
        20_000 * (n - 1792) + 489_000_000
    } else {
        0
    };
    hz as u64
}

fn bc5_uplink_hz(n: u16) -> u64 {
    if n == 2017 {
        return 451_150_000;
    }
    if n == 2018 {
        return 451_475_000;
    }
    let n = n as i64;
    let hz: i64 = if (1..=400).contains(&n) {
        25_000 * (n - 1) + 450_000_000
    } else if (472..=871).contains(&n) {
        25_000 * (n - 472) + 410_000_000
    } else if (1039..=1473).contains(&n) {
        20_000 * (n - 1024) + 451_010_000
    } else if (1536..=1715).contains(&n) {
        25_000 * (n - 1536) + 479_000_000
    } else if (1792..=2016).contains(&n) {
        20_000 * (n - 1792) + 479_000_000
    } else {
        0
    };
    hz as u64
}

// Tables 2.1.6-1, 2.1.6-3: subclass 0..13 → block A..N. Fine-grained
// per-block Conditional sub-ranges are not modeled.
fn bc5_validity(sub: u8, n: u16) -> Option<Validity> {
    let in_block = match sub {
        // A
        0 => (121..=275).contains(&n),
        // B
        1 => (81..=235).contains(&n),
        // C
        2 => (1..=168).contains(&n),
        // D
        3 => (539..=681).contains(&n),
        // E
        4 => (692..=846).contains(&n),
        // F
        5 => (1792..=1985).contains(&n),
        // G
        6 => (1235..=1442).contains(&n),
        // H
        7 => (1039..=1229).contains(&n),
        // I
        8 => (54..=205).contains(&n),
        // J
        9 => (211..=376).contains(&n),
        // K
        10 => (1536..=1690).contains(&n),
        // L
        11 => (472..=646).contains(&n),
        // M
        12 => (1..=375).contains(&n) || n == 2017 || n == 2018,
        // N
        13 => (1..=375).contains(&n) || n == 2017 || n == 2018,
        _ => false,
    };
    if in_block {
        Some(Validity::Valid)
    } else {
        None
    }
}

// ─── BC6 — C.S0057-F §2.1.7, Table 2.1.7-2 ────────────────────────────────

fn bc6_validity(sub: u8, n: u16) -> Option<Validity> {
    if sub != 0 {
        return None;
    }
    if (25..=1175).contains(&n) {
        Some(Validity::Valid)
    } else {
        None
    }
}

// ─── BC7 — C.S0057-F §2.1.8, Table 2.1.8-3 ────────────────────────────────
// Reverse duplex (+30 MHz). Block C 23–198 Valid; block A (220–240)
// Not Valid for SR1.

fn bc7_validity(sub: u8, n: u16) -> Option<Validity> {
    if sub != 0 {
        return None;
    }
    if (23..=198).contains(&n) {
        Some(Validity::Valid)
    } else {
        None
    }
}

// ─── BC8 — C.S0057-F §2.1.9, Table 2.1.9-2 ────────────────────────────────

fn bc8_validity(sub: u8, n: u16) -> Option<Validity> {
    if sub != 0 {
        return None;
    }
    if (25..=1475).contains(&n) {
        Some(Validity::Valid)
    } else {
        None
    }
}

// ─── BC9 — C.S0057-F §2.1.10, Table 2.1.10-2 ──────────────────────────────

fn bc9_validity(sub: u8, n: u16) -> Option<Validity> {
    if sub != 0 {
        return None;
    }
    if (25..=675).contains(&n) {
        Some(Validity::Valid)
    } else {
        None
    }
}

// ─── BC10 — C.S0057-F §2.1.11, Table 2.1.11-2 ─────────────────────────────
// Two piecewise segments. Sub 0–3: 45 MHz duplex. Sub 4 (block E):
// 39 MHz duplex.

fn bc10_downlink_hz(n: u16) -> u64 {
    let n = n as i64;
    let hz: i64 = if n <= 719 {
        25_000 * n + 851_000_000
    } else {
        25_000 * (n - 720) + 935_000_000
    };
    hz as u64
}

fn bc10_uplink_hz(n: u16) -> u64 {
    let n = n as i64;
    let hz: i64 = if n <= 719 {
        25_000 * n + 806_000_000
    } else {
        25_000 * (n - 720) + 896_000_000
    };
    hz as u64
}

// Table 2.1.11-3: subclass 0..4 → System Designator A..E.
fn bc10_validity(sub: u8, n: u16) -> Option<Validity> {
    match sub {
        0 => match n {
            50..=150 => Some(Validity::Valid),
            151..=199 => Some(Validity::Conditional),
            _ => None,
        },
        1 => match n {
            250..=350 => Some(Validity::Valid),
            200..=249 | 351..=399 => Some(Validity::Conditional),
            _ => None,
        },
        2 => match n {
            450..=550 => Some(Validity::Valid),
            400..=449 | 551..=599 => Some(Validity::Conditional),
            _ => None,
        },
        3 => match n {
            650..=670 => Some(Validity::Valid),
            600..=649 => Some(Validity::Conditional),
            _ => None,
        },
        4 => {
            if (770..=870).contains(&n) {
                Some(Validity::Valid)
            } else {
                None
            }
        }
        _ => None,
    }
}

// ─── BC11 — C.S0057-F §2.1.12 ─────────────────────────────────────────────
// Reuses BC5's channel formula. Subclasses 0..11 → blocks A..L; F/G/H
// (sub 5/6/7) are Not specified.

fn bc11_validity(sub: u8, n: u16) -> Option<Validity> {
    let in_block = match sub {
        // A
        0 => (121..=275).contains(&n),
        // B
        1 => (81..=235).contains(&n),
        // C
        2 => (1..=168).contains(&n),
        // D
        3 => (539..=681).contains(&n),
        // E
        4 => (692..=846).contains(&n),
        // F, G, H: Not specified in BC11.
        5 | 6 | 7 => false,
        // I
        8 => (54..=205).contains(&n),
        // J
        9 => (211..=376).contains(&n),
        // K
        10 => (1536..=1690).contains(&n),
        // L
        11 => (472..=646).contains(&n),
        _ => false,
    };
    if in_block {
        Some(Validity::Valid)
    } else {
        None
    }
}

// ─── BC12 — C.S0057-F §2.1.13, Table 2.1.13-3 ─────────────────────────────
// Subclasses 0..2 → blocks A, B, C.

fn bc12_validity(sub: u8, n: u16) -> Option<Validity> {
    match sub {
        0 => {
            if (65..=214).contains(&n) {
                Some(Validity::Valid)
            } else {
                None
            }
        }
        1 => {
            if (94..=144).contains(&n) {
                Some(Validity::Valid)
            } else {
                None
            }
        }
        2 => match n {
            105..=206 => Some(Validity::Valid),
            25..=104 | 207..=214 => Some(Validity::Conditional),
            _ => None,
        },
        _ => None,
    }
}

// ─── BC13 — C.S0057-F §2.1.14, Table 2.1.14-2 ─────────────────────────────

fn bc13_validity(sub: u8, n: u16) -> Option<Validity> {
    if sub != 0 {
        return None;
    }
    if (25..=1375).contains(&n) {
        Some(Validity::Valid)
    } else {
        None
    }
}

// ─── BC14 — C.S0057-F §2.1.15 ─────────────────────────────────────────────
// Same formula as BC1; adds block G (1200–1299).

fn bc14_validity(sub: u8, n: u16) -> Option<Validity> {
    if sub != 0 {
        return None;
    }
    match n {
        25..=275 | 325..=375 | 425..=675 | 725..=775 | 825..=875 | 925..=1175 | 1225..=1275 => {
            Some(Validity::Valid)
        }
        276..=324 | 376..=424 | 676..=724 | 776..=824 | 876..=924 | 1176..=1224 | 1276..=1299 => {
            Some(Validity::Conditional)
        }
        _ => None,
    }
}

// ─── BC15 — C.S0057-F §2.1.16, Table 2.1.16-3 ─────────────────────────────

fn bc15_validity(sub: u8, n: u16) -> Option<Validity> {
    if sub != 0 {
        return None;
    }
    if (25..=875).contains(&n) {
        Some(Validity::Valid)
    } else {
        None
    }
}

// ─── BC16 — C.S0057-F §2.1.17, Table 2.1.16-3 ─────────────────────────────

fn bc16_validity(sub: u8, n: u16) -> Option<Validity> {
    if sub != 0 {
        return None;
    }
    if (165..=1435).contains(&n) {
        Some(Validity::Valid)
    } else {
        None
    }
}

// ─── BC18 — C.S0057-F §2.1.19, Table 2.1.19-3 ─────────────────────────────
// Reverse duplex (+30 MHz).

fn bc18_validity(sub: u8, n: u16) -> Option<Validity> {
    if sub != 0 {
        return None;
    }
    match n {
        45..=95 | 145..=195 => Some(Validity::Valid),
        96..=119 | 120..=144 => Some(Validity::Conditional),
        _ => None,
    }
}

// ─── BC19 — C.S0057-F §2.1.20, Table 2.1.20-3 ─────────────────────────────

fn bc19_validity(sub: u8, n: u16) -> Option<Validity> {
    if sub != 0 {
        return None;
    }
    match n {
        23..=98 | 143..=218 | 263..=338 => Some(Validity::Valid),
        99..=142 | 219..=262 => Some(Validity::Conditional),
        _ => None,
    }
}

// ─── BC20 — C.S0057-F §2.1.21, Table 2.1.21-2 ─────────────────────────────
// Reverse duplex (+101.5 MHz).

fn bc20_validity(sub: u8, n: u16) -> Option<Validity> {
    if sub != 0 {
        return None;
    }
    if (13..=667).contains(&n) {
        Some(Validity::Valid)
    } else {
        None
    }
}

// ─── BC21 — C.S0057-F §2.1.22, Table 2.1.22-1 ─────────────────────────────
// Block A (N=0..200): 190 MHz duplex. Block B (N=201..399): 170 MHz.

fn bc21_downlink_hz(n: u16) -> u64 {
    let n = n as i64;
    let hz: i64 = if n <= 200 {
        50_000 * n + 2_190_000_000
    } else {
        50_000 * (n - 200) + 2_180_000_000
    };
    hz as u64
}

fn bc21_uplink_hz(n: u16) -> u64 {
    let n = n as i64;
    let hz: i64 = if n <= 200 {
        50_000 * n + 2_000_000_000
    } else {
        50_000 * (n - 200) + 2_010_000_000
    };
    hz as u64
}

fn bc21_validity(sub: u8, n: u16) -> Option<Validity> {
    if sub != 0 {
        return None;
    }
    match n {
        25..=175 | 225..=375 => Some(Validity::Valid),
        _ => None,
    }
}

// ─── Dispatcher ──────────────────────────────────────────────────────────

fn channel_validity(band: BandClass, sub: u8, n: u16) -> Option<Validity> {
    match band {
        BandClass::Bc0 => bc0_validity(sub, n),
        BandClass::Bc1 => bc1_validity(sub, n),
        BandClass::Bc2 => bc2_validity(sub, n),
        BandClass::Bc3 => bc3_validity(sub, n),
        BandClass::Bc4 => bc4_validity(sub, n),
        BandClass::Bc5 => bc5_validity(sub, n),
        BandClass::Bc6 => bc6_validity(sub, n),
        BandClass::Bc7 => bc7_validity(sub, n),
        BandClass::Bc8 => bc8_validity(sub, n),
        BandClass::Bc9 => bc9_validity(sub, n),
        BandClass::Bc10 => bc10_validity(sub, n),
        BandClass::Bc11 => bc11_validity(sub, n),
        BandClass::Bc12 => bc12_validity(sub, n),
        BandClass::Bc13 => bc13_validity(sub, n),
        BandClass::Bc14 => bc14_validity(sub, n),
        BandClass::Bc15 => bc15_validity(sub, n),
        BandClass::Bc16 => bc16_validity(sub, n),
        BandClass::Bc17 | BandClass::Bc22 => None,
        BandClass::Bc18 => bc18_validity(sub, n),
        BandClass::Bc19 => bc19_validity(sub, n),
        BandClass::Bc20 => bc20_validity(sub, n),
        BandClass::Bc21 => bc21_validity(sub, n),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn assert_freqs(p: ChannelPlan, expected_dl_hz: u64, expected_ul_hz: u64) {
        assert_eq!(p.downlink_hz(), expected_dl_hz, "downlink mismatch");
        assert_eq!(p.uplink_hz(), expected_ul_hz, "uplink mismatch");
    }

    // ── BC0 ──
    #[test]
    fn bc0_channel_384_matches_historical_defaults() {
        let p = ChannelPlan::new(BandClass::Bc0, 0, 384);
        assert_freqs(p, 881_520_000, 836_520_000);
        p.validate().unwrap();
    }

    #[test]
    fn bc0_segment_991_1023() {
        let p = ChannelPlan::new(BandClass::Bc0, 2, 1023);
        assert_freqs(p, 870_000_000, 825_000_000);
        let p2 = ChannelPlan::new(BandClass::Bc0, 2, 991);
        assert_freqs(p2, 869_040_000, 824_040_000);
    }

    #[test]
    fn bc0_segment_1024_1323() {
        let p = ChannelPlan::new(BandClass::Bc0, 3, 1048);
        assert_freqs(p, 860_760_000, 815_760_000);
        let p2 = ChannelPlan::new(BandClass::Bc0, 3, 1323);
        assert_eq!(p2.downlink_hz(), 869_010_000);
    }

    #[test]
    fn bc0_rejects_subclass_out_of_range() {
        assert!(ChannelPlan::new(BandClass::Bc0, 4, 384).validate().is_err());
    }

    // ── BC1 ──
    #[test]
    fn bc1_block_a_sample() {
        let p = ChannelPlan::new(BandClass::Bc1, 0, 25);
        assert_freqs(p, 1_931_250_000, 1_851_250_000);
        p.validate().unwrap();
    }

    #[test]
    fn bc1_conditional_passes() {
        let p = ChannelPlan::new(BandClass::Bc1, 0, 276);
        assert_eq!(p.channel_validity(), Some(Validity::Conditional));
        p.validate().unwrap();
    }

    // ── BC2 ──
    #[test]
    fn bc2_sub0_sample() {
        // N=79 per preferred set: 25e3·79 + 934_987_500 = 936_962_500
        let p = ChannelPlan::new(BandClass::Bc2, 0, 79);
        assert_eq!(p.downlink_hz(), 936_962_500);
        assert_eq!(p.uplink_hz(), 891_962_500);
        p.validate().unwrap();
    }

    #[test]
    fn bc2_sub3_atg_reverse_duplex() {
        // ATG block: mobile transmits HIGHER than base.
        let p = ChannelPlan::new(BandClass::Bc2, 3, 2078);
        // base = 25e3·30 + 849_000_000 = 849_750_000
        assert_eq!(p.downlink_hz(), 849_750_000);
        // mobile = 25e3·30 + 894_000_000 = 894_750_000 (HIGHER than base)
        assert_eq!(p.uplink_hz(), 894_750_000);
        assert!(p.uplink_hz() > p.downlink_hz());
        p.validate().unwrap();
    }

    // ── BC3 ──
    #[test]
    fn bc3_even_channel_sample() {
        // N=76 (even, valid): base = 12.5e3·76 + 860e6 = 860_950_000
        //                     mobile = 12.5e3·76 + 915e6 = 915_950_000 (HIGHER)
        let p = ChannelPlan::new(BandClass::Bc3, 0, 76);
        assert_eq!(p.downlink_hz(), 860_950_000);
        assert_eq!(p.uplink_hz(), 915_950_000);
        assert!(p.uplink_hz() > p.downlink_hz());
        p.validate().unwrap();
    }

    #[test]
    fn bc3_rejects_odd_channel() {
        assert!(ChannelPlan::new(BandClass::Bc3, 0, 75).validate().is_err());
    }

    // ── BC4 ──
    #[test]
    fn bc4_sample() {
        // N=25: base = 50e3·25 + 1_840e6 = 1_841_250_000
        let p = ChannelPlan::new(BandClass::Bc4, 0, 25);
        assert_freqs(p, 1_841_250_000, 1_751_250_000);
        p.validate().unwrap();
    }

    // ── BC5 ──
    #[test]
    fn bc5_block_a_sample() {
        // Block A (sub 0), preferred N=160: base = 25e3·(160-1) + 460e6
        //                                       = 463_975_000
        let p = ChannelPlan::new(BandClass::Bc5, 0, 160);
        assert_eq!(p.downlink_hz(), 463_975_000);
        assert_eq!(p.uplink_hz(), 453_975_000);
        p.validate().unwrap();
    }

    #[test]
    fn bc5_singleton_channels() {
        let p = ChannelPlan::new(BandClass::Bc5, 12, 2017);
        assert_eq!(p.downlink_hz(), 467_725_000);
        assert_eq!(p.uplink_hz(), 451_150_000);
    }

    // ── BC6 ──
    #[test]
    fn bc6_sample() {
        // N=25: base = 50e3·25 + 2_110e6 = 2_111_250_000
        let p = ChannelPlan::new(BandClass::Bc6, 0, 25);
        assert_freqs(p, 2_111_250_000, 1_921_250_000);
        p.validate().unwrap();
    }

    // ── BC7 ──
    #[test]
    fn bc7_reverse_duplex() {
        // N=23: base = 50e3·23 + 746e6 = 747_150_000
        //       mobile = 50e3·23 + 776e6 = 777_150_000 (HIGHER)
        let p = ChannelPlan::new(BandClass::Bc7, 0, 23);
        assert_eq!(p.downlink_hz(), 747_150_000);
        assert_eq!(p.uplink_hz(), 777_150_000);
        assert!(p.uplink_hz() > p.downlink_hz());
        p.validate().unwrap();
    }

    // ── BC8 ──
    #[test]
    fn bc8_sample() {
        let p = ChannelPlan::new(BandClass::Bc8, 0, 25);
        assert_freqs(p, 1_806_250_000, 1_711_250_000);
    }

    // ── BC9 ──
    #[test]
    fn bc9_sample() {
        let p = ChannelPlan::new(BandClass::Bc9, 0, 25);
        assert_freqs(p, 926_250_000, 881_250_000);
    }

    // ── BC10 ──
    #[test]
    fn bc10_sub0_sample() {
        // N=50: base = 25e3·50 + 851e6 = 852_250_000
        let p = ChannelPlan::new(BandClass::Bc10, 0, 50);
        assert_freqs(p, 852_250_000, 807_250_000);
    }

    #[test]
    fn bc10_sub4_block_e() {
        // N=770: base = 25e3·(770-720) + 935e6 = 936_250_000
        // duplex is 39 MHz (sub 4 special-case)
        let p = ChannelPlan::new(BandClass::Bc10, 4, 770);
        assert_eq!(p.downlink_hz(), 936_250_000);
        assert_eq!(p.uplink_hz(), 897_250_000);
        assert_eq!(p.duplex_offset_hz(), 39_000_000);
    }

    // ── BC11 ──
    #[test]
    fn bc11_block_a_sample() {
        // Same formula as BC5.
        let p = ChannelPlan::new(BandClass::Bc11, 0, 160);
        assert_eq!(p.downlink_hz(), 463_975_000);
    }

    // ── BC12 ──
    #[test]
    fn bc12_sample() {
        // N=89: base = 25e3·89 + 915_012_500 = 917_237_500
        let p = ChannelPlan::new(BandClass::Bc12, 0, 89);
        assert_eq!(p.downlink_hz(), 917_237_500);
        assert_eq!(p.uplink_hz(), 872_237_500);
    }

    // ── BC13 ──
    #[test]
    fn bc13_sample() {
        let p = ChannelPlan::new(BandClass::Bc13, 0, 50);
        assert_freqs(p, 2_622_500_000, 2_502_500_000);
    }

    // ── BC14 ──
    #[test]
    fn bc14_sample() {
        let p = ChannelPlan::new(BandClass::Bc14, 0, 25);
        assert_freqs(p, 1_931_250_000, 1_851_250_000);
    }

    // ── BC15 ──
    #[test]
    fn bc15_sample() {
        let p = ChannelPlan::new(BandClass::Bc15, 0, 25);
        assert_freqs(p, 2_111_250_000, 1_711_250_000);
        assert_eq!(p.duplex_offset_hz(), 400_000_000);
    }

    // ── BC16 ──
    #[test]
    fn bc16_sample() {
        let p = ChannelPlan::new(BandClass::Bc16, 0, 165);
        // base = 50e3·165 + 2_617e6 = 2_625_250_000
        // mobile = 50e3·165 + 2_495e6 = 2_503_250_000
        assert_freqs(p, 2_625_250_000, 2_503_250_000);
    }

    // ── BC17 / BC22 unspecified ──
    #[test]
    fn bc17_rejected() {
        assert!(ChannelPlan::new(BandClass::Bc17, 0, 0).validate().is_err());
    }

    #[test]
    fn bc22_rejected() {
        assert!(ChannelPlan::new(BandClass::Bc22, 0, 0).validate().is_err());
    }

    // ── BC18 ──
    #[test]
    fn bc18_reverse_duplex() {
        let p = ChannelPlan::new(BandClass::Bc18, 0, 45);
        // base = 50e3·45 + 757e6 = 759_250_000
        // mobile = 50e3·45 + 787e6 = 789_250_000 (HIGHER)
        assert_eq!(p.downlink_hz(), 759_250_000);
        assert_eq!(p.uplink_hz(), 789_250_000);
        assert!(p.uplink_hz() > p.downlink_hz());
    }

    // ── BC19 ──
    #[test]
    fn bc19_sample() {
        let p = ChannelPlan::new(BandClass::Bc19, 0, 23);
        assert_freqs(p, 729_150_000, 699_150_000);
    }

    // ── BC20 ──
    #[test]
    fn bc20_reverse_duplex() {
        let p = ChannelPlan::new(BandClass::Bc20, 0, 25);
        // base = 50e3·25 + 1_525e6 = 1_526_250_000
        // mobile = 50e3·25 + 1_626.5e6 = 1_627_750_000 (HIGHER)
        assert_eq!(p.downlink_hz(), 1_526_250_000);
        assert_eq!(p.uplink_hz(), 1_627_750_000);
        assert_eq!(p.duplex_offset_hz(), 101_500_000);
    }

    // ── BC21 ──
    #[test]
    fn bc21_block_a_sample() {
        let p = ChannelPlan::new(BandClass::Bc21, 0, 25);
        // Block A: base = 50e3·25 + 2_190e6 = 2_191_250_000
        //          mobile = 50e3·25 + 2_000e6 = 2_001_250_000
        assert_freqs(p, 2_191_250_000, 2_001_250_000);
        assert_eq!(p.duplex_offset_hz(), 190_000_000);
    }

    #[test]
    fn bc21_block_b_sample() {
        let p = ChannelPlan::new(BandClass::Bc21, 0, 225);
        // Block B: base = 50e3·25 + 2_180e6 = 2_181_250_000
        //          mobile = 50e3·25 + 2_010e6 = 2_011_250_000
        assert_freqs(p, 2_181_250_000, 2_011_250_000);
        assert_eq!(p.duplex_offset_hz(), 170_000_000);
    }

    // ── Cross-cutting ──
    #[test]
    fn default_is_bc0_sub0_384() {
        let p = ChannelPlan::default();
        assert_eq!(p.band_class, BandClass::Bc0);
        assert_eq!(p.band_subclass, 0);
        assert_eq!(p.cdma_channel, 384);
    }

    #[test]
    fn json_round_trip() {
        let p = ChannelPlan::new(BandClass::Bc5, 3, 564);
        let s = serde_json::to_string(&p).unwrap();
        assert!(s.contains("\"band_class\":\"bc5\""));
        let q: ChannelPlan = serde_json::from_str(&s).unwrap();
        assert_eq!(p, q);
    }

    #[test]
    fn field_value_table() {
        for (band, expected) in [
            (BandClass::Bc0, 0u8),
            (BandClass::Bc7, 7),
            (BandClass::Bc15, 15),
            (BandClass::Bc22, 22),
        ] {
            assert_eq!(band.field_value(), expected);
        }
    }
}
