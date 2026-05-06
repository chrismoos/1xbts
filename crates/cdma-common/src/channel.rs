/// Data rate tier for a traffic channel frame.
///
/// RC1: Full=9600, Half=4800, Quarter=2400, Eighth=1200 bps.
/// RC3: Full=9600, Half=4800, Quarter=2700, Eighth=1500 bps.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrafficRate {
    /// Full rate: 9600 bps (RC1 and RC3)
    Full,
    /// Half rate: 4800 bps (RC1 and RC3)
    Half,
    /// Quarter rate: 2400 bps (RC1) / 2700 bps (RC3)
    Quarter,
    /// Eighth rate: 1200 bps (RC1) / 1500 bps (RC3)
    Eighth,
}

impl TrafficRate {
    /// Number of encoder input bits per 20ms frame at this rate.
    pub fn frame_bits(&self) -> usize {
        match self {
            TrafficRate::Full => 192,   // 172 info + 12 CRC + 8 tail
            TrafficRate::Half => 96,    // 80 info + 8 CRC + 8 tail
            TrafficRate::Quarter => 48, // 40 info + 8 CRC (no explicit tail at this size?)
            TrafficRate::Eighth => 24,  // 16 info + 8 tail
        }
    }

    /// Symbol repetition factor to produce 384 symbols from encoded output.
    pub fn repeat_factor(&self) -> usize {
        match self {
            TrafficRate::Full => 1,
            TrafficRate::Half => 2,
            TrafficRate::Quarter => 4,
            TrafficRate::Eighth => 8,
        }
    }

    /// Number of information bits (before CRC/tail) per frame at this rate.
    pub fn info_bits(&self) -> usize {
        match self {
            TrafficRate::Full => 172,
            TrafficRate::Half => 80,
            TrafficRate::Quarter => 40,
            TrafficRate::Eighth => 16,
        }
    }

    /// Number of FQI (CRC) bits for this rate (RC1/RC2).
    pub fn fqi_bits(&self) -> usize {
        match self {
            TrafficRate::Full => 12,
            TrafficRate::Half => 8,
            TrafficRate::Quarter => 0,
            TrafficRate::Eighth => 0,
        }
    }

    /// RC3 encoder input bits (info + FQI + tail) per 20ms frame.
    /// Per C.S0002-E Table 3.1.3.15.2-1.
    pub fn rc3_frame_bits(&self) -> usize {
        match self {
            TrafficRate::Full => 192,   // 172 info + 12 CRC + 8 tail
            TrafficRate::Half => 96,    // 80 info + 8 CRC + 8 tail
            TrafficRate::Quarter => 54, // 40 info + 6 CRC + 8 tail  (2700 bps)
            TrafficRate::Eighth => 30,  // 16 info + 6 CRC + 8 tail  (1500 bps)
        }
    }

    /// RC3 FQI (CRC) bits per C.S0002-E Section 3.1.3.15.2.1.
    pub fn rc3_fqi_bits(&self) -> usize {
        match self {
            TrafficRate::Full => 12,
            TrafficRate::Half => 8,
            TrafficRate::Quarter => 6,
            TrafficRate::Eighth => 6,
        }
    }

    /// RC3 symbol repetition factor per Table 3.1.3.1.2.1-19.
    pub fn rc3_repeat_factor(&self) -> usize {
        match self {
            TrafficRate::Full => 1,
            TrafficRate::Half => 2,
            TrafficRate::Quarter => 4,
            TrafficRate::Eighth => 8,
        }
    }
}
