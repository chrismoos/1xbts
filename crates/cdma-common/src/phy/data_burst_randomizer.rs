//! Reverse RC1/RC2 data-burst randomizer.
//!
//! C.S0002-E §2.1.3.1.14.2 defines nested gated-on PCG sets for the four
//! reverse traffic rates. The one-eighth-rate set is therefore guaranteed to
//! be transmitted regardless of the frame's actual rate.

use super::long_code::LongCodeGenerator;

pub const RC12_PCGS_PER_FRAME: usize = 16;
pub const RC12_CHIPS_PER_PCG: u64 = 1536;
const RC12_RANDOMIZER_LC_BITS: usize = 14;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Rc12ReverseRate {
    Full,
    Half,
    Quarter,
    Eighth,
}

/// Return the gated-on reverse PCGs for a frame whose long-code generator
/// starts at absolute chip zero.
pub fn active_pcgs(
    long_code_origin: &LongCodeGenerator,
    frame_chip_start: u64,
    rate: Rc12ReverseRate,
) -> [bool; RC12_PCGS_PER_FRAME] {
    let mut active = [false; RC12_PCGS_PER_FRAME];
    if rate == Rc12ReverseRate::Full {
        active.fill(true);
        return active;
    }

    // The randomizer consumes the last 14 long-code bits in the next-to-last
    // PCG of the preceding frame: exactly 1536 + 14 chips before the boundary.
    let mut generator = long_code_origin.clone();
    let offset =
        frame_chip_start.saturating_sub(RC12_CHIPS_PER_PCG + RC12_RANDOMIZER_LC_BITS as u64);
    generator.advance_chips(
        usize::try_from(offset).expect("absolute CDMA chip index must fit in usize"),
    );
    let mut b = [0u8; RC12_RANDOMIZER_LC_BITS];
    for bit in &mut b {
        *bit = generator.next_chip();
    }

    match rate {
        Rc12ReverseRate::Half => {
            for i in 0..8 {
                active[2 * i + b[i] as usize] = true;
            }
        }
        Rc12ReverseRate::Quarter => {
            active[if b[8] == 0 { b[0] } else { 2 + b[1] } as usize] = true;
            active[(if b[9] == 0 { 4 + b[2] } else { 6 + b[3] }) as usize] = true;
            active[(if b[10] == 0 { 8 + b[4] } else { 10 + b[5] }) as usize] = true;
            active[(if b[11] == 0 { 12 + b[6] } else { 14 + b[7] }) as usize] = true;
        }
        Rc12ReverseRate::Eighth => {
            let lower = if b[12] == 0 {
                if b[8] == 0 {
                    b[0] as usize
                } else {
                    2 + b[1] as usize
                }
            } else if b[9] == 0 {
                4 + b[2] as usize
            } else {
                6 + b[3] as usize
            };
            let upper = if b[13] == 0 {
                if b[10] == 0 {
                    8 + b[4] as usize
                } else {
                    10 + b[5] as usize
                }
            } else if b[11] == 0 {
                12 + b[6] as usize
            } else {
                14 + b[7] as usize
            };
            active[lower] = true;
            active[upper] = true;
        }
        Rc12ReverseRate::Full => unreachable!(),
    }
    active
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lower_rate_pcg_sets_are_nested() {
        let origin = LongCodeGenerator::new_traffic_channel(0xDEAD_BEEF);
        for frame in 1..64 {
            let frame_chip_start = frame * RC12_PCGS_PER_FRAME as u64 * RC12_CHIPS_PER_PCG;
            let full = active_pcgs(&origin, frame_chip_start, Rc12ReverseRate::Full);
            let half = active_pcgs(&origin, frame_chip_start, Rc12ReverseRate::Half);
            let quarter = active_pcgs(&origin, frame_chip_start, Rc12ReverseRate::Quarter);
            let eighth = active_pcgs(&origin, frame_chip_start, Rc12ReverseRate::Eighth);

            assert_eq!(full.iter().filter(|active| **active).count(), 16);
            assert_eq!(half.iter().filter(|active| **active).count(), 8);
            assert_eq!(quarter.iter().filter(|active| **active).count(), 4);
            assert_eq!(eighth.iter().filter(|active| **active).count(), 2);
            for pcg in 0..RC12_PCGS_PER_FRAME {
                assert!(!eighth[pcg] || quarter[pcg]);
                assert!(!quarter[pcg] || half[pcg]);
                assert!(!half[pcg] || full[pcg]);
            }
        }
    }
}
