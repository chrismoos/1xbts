//! RLP sequence-number arithmetic for the HRPD default packet stream.
//!
//! The sequence space is the modulus defined by
//! [`DEFAULT_PACKET_RLP_SEQUENCE_BITS`]; all comparisons are modular over that
//! window. Both the air-interface receiver and the A8 forward runtime share
//! this one implementation so their sequence arithmetic cannot drift apart.

use std::cmp::Ordering;

use cdma_common::hrpd::traffic::DEFAULT_PACKET_RLP_SEQUENCE_BITS;

/// Number of distinct RLP sequence values.
pub const SEQUENCE_MODULUS: u32 = 1 << DEFAULT_PACKET_RLP_SEQUENCE_BITS;
/// Mask that folds an arbitrary `u32` into the sequence window.
pub const SEQUENCE_MASK: u32 = SEQUENCE_MODULUS - 1;
/// Half the sequence window, the modular ahead/behind boundary.
pub const SEQUENCE_HALF: u32 = SEQUENCE_MODULUS / 2;

/// Next sequence value after `sequence`, wrapped into the window.
pub fn next(sequence: u32) -> u32 {
    sequence.wrapping_add(1) & SEQUENCE_MASK
}

/// Modular distance from `from` to `to` in the forward direction.
pub fn distance(from: u32, to: u32) -> u32 {
    to.wrapping_sub(from) & SEQUENCE_MASK
}

/// Modular comparison: `left` is `Greater` when it is at most half a window
/// ahead of `right`, else `Less` (or `Equal`).
pub fn cmp(left: u32, right: u32) -> Ordering {
    let delta = distance(right, left);
    if delta == 0 {
        Ordering::Equal
    } else if delta < SEQUENCE_HALF {
        Ordering::Greater
    } else {
        Ordering::Less
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn next_wraps_at_modulus() {
        assert_eq!(next(0), 1);
        assert_eq!(next(SEQUENCE_MASK), 0);
    }

    #[test]
    fn distance_is_directional_and_modular() {
        assert_eq!(distance(5, 8), 3);
        assert_eq!(distance(SEQUENCE_MASK, 1), 2);
        assert_eq!(distance(8, 5), SEQUENCE_MODULUS - 3);
    }

    #[test]
    fn cmp_orders_within_half_window() {
        assert_eq!(cmp(5, 5), Ordering::Equal);
        assert_eq!(cmp(6, 5), Ordering::Greater);
        assert_eq!(cmp(5, 6), Ordering::Less);
        // Wrap: 1 is ahead of the max value.
        assert_eq!(cmp(1, SEQUENCE_MASK), Ordering::Greater);
        assert_eq!(cmp(SEQUENCE_MASK, 1), Ordering::Less);
    }
}
