//! Private long code transitions at explicit action times (C.S0005-E §3.6.4.1.5).

use std::sync::Mutex;

/// C.S0005-E §3.6.4.1.4 uses 80 ms action-time units.
pub const PRIVACY_ACTION_CHIPS: u64 = crate::consts::SR1_CHIPS_PER_FRAME * 4;
pub const PRIVACY_ACTION_MODULUS: u64 = 64;
pub const PRIVACY_FRAME_CHIPS: u64 = crate::consts::SR1_CHIPS_PER_FRAME;
pub const MOBILE_STATION_REJECT_ORDER: u8 = 0b011111;
pub const LONG_CODE_TRANSITION_ORDER: u8 = 0b010111;
pub const LONG_CODE_RESPONSE_PUBLIC: u8 = 0b00000010;
pub const LONG_CODE_RESPONSE_PRIVATE: u8 = 0b00000011;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LongCodeTransition {
    pub action_chip: u64,
    pub private: bool,
}

struct PrivacyTransitions {
    pending: Option<LongCodeTransition>,
    accepted: Vec<LongCodeTransition>,
}

pub struct LongCodePrivacy {
    public_mask: u64,
    private_mask: u64,
    transitions: Mutex<PrivacyTransitions>,
}

impl std::fmt::Debug for LongCodePrivacy {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LongCodePrivacy").finish_non_exhaustive()
    }
}

impl LongCodePrivacy {
    pub fn new(public_mask: u64, private_mask: u64) -> Self {
        Self {
            public_mask,
            private_mask,
            transitions: Mutex::new(PrivacyTransitions {
                pending: None,
                accepted: Vec::new(),
            }),
        }
    }

    pub fn private_mask(&self) -> u64 {
        self.private_mask
    }

    pub fn request(&self, transition: LongCodeTransition) -> bool {
        let mut state = self.transitions.lock().expect("privacy transition lock");
        if state.pending.is_some()
            || state
                .accepted
                .last()
                .is_some_and(|last| last.action_chip >= transition.action_chip)
        {
            return false;
        }
        state.pending = Some(transition);
        true
    }

    pub fn cancel_pending(&self) {
        self.transitions
            .lock()
            .expect("privacy transition lock")
            .pending = None;
    }

    pub fn pending(&self) -> Option<LongCodeTransition> {
        self.transitions
            .lock()
            .expect("privacy transition lock")
            .pending
    }

    /// Only an accepted response received before the explicit action time arms the switch.
    pub fn respond(&self, private: bool, received_chip: u64) -> bool {
        let mut state = self.transitions.lock().expect("privacy transition lock");
        let Some(pending) = state.pending.take() else {
            return false;
        };
        if pending.private != private || received_chip >= pending.action_chip {
            return false;
        }
        state.accepted.push(pending);
        true
    }

    pub fn is_private_at(&self, chip: u64) -> bool {
        self.transitions
            .lock()
            .expect("privacy transition lock")
            .accepted
            .iter()
            .rev()
            .find(|transition| transition.action_chip <= chip)
            .is_some_and(|transition| transition.private)
    }

    pub fn status_at(&self, chip: u64) -> &'static str {
        let state = self.transitions.lock().expect("privacy transition lock");
        if state
            .pending
            .is_some_and(|pending| pending.action_chip > chip)
            || state
                .accepted
                .last()
                .is_some_and(|accepted| accepted.action_chip > chip)
        {
            return "pending";
        }
        if state
            .accepted
            .iter()
            .rev()
            .find(|transition| transition.action_chip <= chip)
            .is_some_and(|transition| transition.private)
        {
            "private"
        } else {
            "public"
        }
    }

    pub fn mask_at(&self, chip: u64) -> u64 {
        if self.is_private_at(chip) {
            self.private_mask
        } else {
            self.public_mask
        }
    }
}

pub fn resolve_privacy_action_chip(action_time: u8, now_chip: u64) -> u64 {
    let current = now_chip / PRIVACY_ACTION_CHIPS;
    let mut action =
        (current / PRIVACY_ACTION_MODULUS) * PRIVACY_ACTION_MODULUS + u64::from(action_time);
    if action <= current {
        action += PRIVACY_ACTION_MODULUS;
    }
    action * PRIVACY_ACTION_CHIPS
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn private_mask_requires_acceptance_and_changes_only_at_action_time() {
        let privacy = LongCodePrivacy::new(1, 2);
        let transition = LongCodeTransition {
            action_chip: PRIVACY_ACTION_CHIPS,
            private: true,
        };
        assert!(privacy.request(transition));
        assert_eq!(privacy.mask_at(transition.action_chip), 1);
        assert!(privacy.respond(true, transition.action_chip - 1));
        assert_eq!(privacy.mask_at(transition.action_chip - 1), 1);
        assert_eq!(privacy.mask_at(transition.action_chip), 2);
        assert!(privacy.request(LongCodeTransition {
            action_chip: 2 * PRIVACY_ACTION_CHIPS,
            private: false
        }));
        assert!(privacy.respond(false, PRIVACY_ACTION_CHIPS));
        assert_eq!(privacy.mask_at(2 * PRIVACY_ACTION_CHIPS), 1);
        assert_eq!(privacy.mask_at(PRIVACY_ACTION_CHIPS), 2);
    }

    #[test]
    fn rejected_unsolicited_and_late_responses_do_not_enable_privacy() {
        let privacy = LongCodePrivacy::new(1, 2);
        assert!(!privacy.respond(true, 0));
        for (private, received_chip) in [(false, 0), (true, PRIVACY_ACTION_CHIPS)] {
            assert!(privacy.request(LongCodeTransition {
                action_chip: PRIVACY_ACTION_CHIPS,
                private: true
            }));
            assert!(!privacy.respond(private, received_chip));
            assert_eq!(privacy.mask_at(PRIVACY_ACTION_CHIPS), 1);
        }
    }
}

#[cfg(test)]
mod generator_tests {
    use super::*;
    use crate::phy::long_code::LongCodeGenerator;
    use std::sync::Arc;

    #[test]
    fn generator_clones_switch_masks_at_the_same_chip_without_resetting_state() {
        const ESN: u32 = 0x8085_7e58;
        const PRIVATE_MASK: u64 = 0x147_80cc_b8df;
        const WINDOW_CHIPS: usize = 64;
        let public = LongCodeGenerator::new_traffic_channel(ESN);
        let privacy = Arc::new(LongCodePrivacy::new(public.mask(), PRIVATE_MASK));
        let mut tx = public.clone().with_privacy(Some(privacy.clone()));
        let mut rx = tx.clone();
        privacy.request(LongCodeTransition {
            action_chip: PRIVACY_ACTION_CHIPS,
            private: true,
        });
        privacy.respond(true, PRIVACY_FRAME_CHIPS);
        tx.advance_chips(PRIVACY_ACTION_CHIPS as usize - WINDOW_CHIPS);
        rx.advance_chips(PRIVACY_ACTION_CHIPS as usize - WINDOW_CHIPS);
        let mut expected = public;
        expected.advance_chips(PRIVACY_ACTION_CHIPS as usize - WINDOW_CHIPS);
        for _ in 0..WINDOW_CHIPS {
            let chip = expected.next_chip();
            assert_eq!(tx.next_chip(), chip);
            assert_eq!(rx.next_chip(), chip);
        }
        expected.set_mask(PRIVATE_MASK);
        for _ in 0..WINDOW_CHIPS {
            let chip = expected.next_chip();
            assert_eq!(tx.next_chip(), chip);
            assert_eq!(rx.next_chip(), chip);
        }
        let mut jumped = LongCodeGenerator::new_traffic_channel(ESN).with_privacy(Some(privacy));
        jumped.advance_chips(PRIVACY_ACTION_CHIPS as usize + WINDOW_CHIPS);
        assert_eq!(
            jumped.generate_sequence(WINDOW_CHIPS),
            expected.generate_sequence(WINDOW_CHIPS)
        );
    }
}
