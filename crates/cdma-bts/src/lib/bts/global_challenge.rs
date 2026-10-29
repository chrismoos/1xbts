//! Broadcast AUTH and RAND for the CAVE global challenge (C.S0005-E §2.3.12.1.2).
//!
//! The BTS owns RAND. It is broadcast in the Access Parameters Message and
//! reported to the BSC in the Abis-ACH Msg Transfer Authentication Challenge
//! Parameter. The previous RAND stays valid after a rotation so a mobile that
//! has not yet read the new Access Parameters Message still authenticates.
//! AUTH can change at runtime. The paging and access receive paths read it
//! lock-free.

use std::sync::atomic::{AtomicU8, AtomicU64, Ordering};

/// ACC_MSG_SEQ is a 6-bit field (C.S0005-E §3.7.2.3.2.2).
const ACC_MSG_SEQ_MODULUS: u64 = 64;
const RANDC_SHIFT: u32 = 24;
const RAND_MASK: u64 = 0xFFFF_FFFF;
const GENERATION_SHIFT: u32 = 32;

/// AUTH = '00', no global challenge (C.S0005-E §3.7.2.3.2.2).
pub const AUTH_MODE_NONE: u8 = 0b00;
/// AUTH = '01', standard authentication with the broadcast RAND.
pub const AUTH_MODE_CAVE: u8 = 0b01;

/// RANDC is the 8 most significant bits of RAND (C.S0004-E §2.1.1.1.1.2).
pub fn randc(rand: u32) -> u8 {
    (rand >> RANDC_SHIFT) as u8
}

/// Lock-free AUTH and current and previous RAND. Each RAND value packs the
/// broadcast generation in the upper 32 bits and RAND in the lower 32. The
/// generation advances on every RAND rotation and AUTH change so ACC_MSG_SEQ
/// tells mobiles to re-read the Access Parameters Message.
pub struct GlobalChallenge {
    auth: AtomicU8,
    current: AtomicU64,
    previous: AtomicU64,
    /// Serializes writers. Readers never take it.
    writer: parking_lot::Mutex<()>,
}

fn pack(generation: u32, rand: u32) -> u64 {
    (u64::from(generation) << GENERATION_SHIFT) | u64::from(rand)
}

/// A RAND of zero leaves the LFSR seeded from SSD alone, so it is never broadcast.
fn nonzero_random() -> u32 {
    loop {
        let rand = rand::random::<u32>();
        if rand != 0 {
            return rand;
        }
    }
}

fn random_with_new_randc(avoid: u32) -> u32 {
    loop {
        let rand = nonzero_random();
        if randc(rand) != randc(avoid) {
            return rand;
        }
    }
}

impl Default for GlobalChallenge {
    fn default() -> Self {
        Self::with_auth(AUTH_MODE_NONE)
    }
}

impl GlobalChallenge {
    pub fn new(auth: u8, rand: u32) -> Self {
        Self {
            auth: AtomicU8::new(auth),
            current: AtomicU64::new(pack(0, rand)),
            previous: AtomicU64::new(pack(0, rand)),
            writer: parking_lot::Mutex::new(()),
        }
    }

    /// A random RAND and a random starting generation. Mobiles cache the
    /// Access Parameters Message by ACC_MSG_SEQ across a BTS restart, so a
    /// fixed starting generation would repeat the previous run's ACC_MSG_SEQ
    /// and leave them answering with the previous run's RAND.
    pub fn with_auth(auth: u8) -> Self {
        let challenge = Self::new(auth, nonzero_random());
        let rand = challenge.current();
        challenge
            .current
            .store(pack(rand::random::<u32>(), rand), Ordering::Release);
        challenge
            .previous
            .store(challenge.current.load(Ordering::Acquire), Ordering::Release);
        challenge
    }

    pub fn auth(&self) -> u8 {
        self.auth.load(Ordering::Acquire)
    }

    pub fn enabled(&self) -> bool {
        self.auth() != AUTH_MODE_NONE
    }

    pub fn current(&self) -> u32 {
        (self.current.load(Ordering::Acquire) & RAND_MASK) as u32
    }

    fn generation(&self) -> u32 {
        (self.current.load(Ordering::Acquire) >> GENERATION_SHIFT) as u32
    }

    pub fn acc_msg_seq(&self, base: u8) -> u8 {
        ((u64::from(base) + u64::from(self.generation())) % ACC_MSG_SEQ_MODULUS) as u8
    }

    fn advance(&self, rand: u32) {
        let current = self.current.load(Ordering::Acquire);
        let generation = (current >> GENERATION_SHIFT) as u32;
        self.previous.store(current, Ordering::Release);
        self.current
            .store(pack(generation.wrapping_add(1), rand), Ordering::Release);
    }

    pub fn rotate_to(&self, rand: u32) {
        let _writer = self.writer.lock();
        self.advance(rand);
    }

    pub fn rotate(&self) {
        let _writer = self.writer.lock();
        self.advance(random_with_new_randc(self.current()));
    }

    /// Sets AUTH. A change also starts a new RAND generation. Returns true when AUTH changed.
    /// No mobile authenticates with a RAND broadcast while AUTH was '00', so enabling AUTH
    /// keeps no previous RAND. A kept one could share a stale mobile's RANDC and turn a RAND
    /// mismatch into an AUTHR mismatch.
    pub fn set_auth(&self, auth: u8) -> bool {
        let _writer = self.writer.lock();
        let previous_auth = self.auth.swap(auth, Ordering::AcqRel);
        if previous_auth == auth {
            return false;
        }
        self.advance(random_with_new_randc(self.current()));
        if previous_auth == AUTH_MODE_NONE {
            self.previous
                .store(self.current.load(Ordering::Acquire), Ordering::Release);
        }
        true
    }

    /// The broadcast RAND whose 8 MSBs equal `randc`, preferring the current one.
    pub fn rand_for_randc(&self, randc_value: u8) -> Option<u32> {
        [&self.current, &self.previous]
            .into_iter()
            .map(|slot| (slot.load(Ordering::Acquire) & RAND_MASK) as u32)
            .find(|&rand| randc(rand) == randc_value)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn previous_rand_stays_valid_after_rotation() {
        let rand = GlobalChallenge::new(AUTH_MODE_CAVE, 0xAB12_3456);
        assert_eq!(rand.rand_for_randc(0xAB), Some(0xAB12_3456));
        assert_eq!(rand.rand_for_randc(0xCD), None);
        rand.rotate_to(0xCD00_0001);
        assert_eq!(rand.current(), 0xCD00_0001);
        assert_eq!(rand.rand_for_randc(0xCD), Some(0xCD00_0001));
        assert_eq!(rand.rand_for_randc(0xAB), Some(0xAB12_3456));
        rand.rotate_to(0xEF00_0002);
        assert_eq!(rand.rand_for_randc(0xAB), None);
    }

    #[test]
    fn acc_msg_seq_advances_and_wraps() {
        let rand = GlobalChallenge::new(AUTH_MODE_CAVE, 1);
        assert_eq!(rand.acc_msg_seq(62), 62);
        rand.rotate_to(2);
        assert_eq!(rand.acc_msg_seq(62), 63);
        rand.rotate_to(3);
        assert_eq!(rand.acc_msg_seq(62), 0);
    }

    #[test]
    fn random_rand_is_nonzero() {
        assert_ne!(GlobalChallenge::default().current(), 0);
    }

    #[test]
    fn rotation_never_reuses_the_current_randc() {
        let challenge = GlobalChallenge::new(AUTH_MODE_CAVE, 0xAB00_0001);
        for _ in 0..512 {
            let before = challenge.current();
            challenge.rotate();
            assert_ne!(randc(challenge.current()), randc(before));
        }
    }

    #[test]
    fn auth_change_starts_a_new_generation() {
        let challenge = GlobalChallenge::new(AUTH_MODE_NONE, 0xAB00_0001);
        assert!(!challenge.enabled());
        assert_eq!(challenge.acc_msg_seq(5), 5);
        assert!(challenge.set_auth(AUTH_MODE_CAVE));
        assert!(challenge.enabled());
        assert_eq!(challenge.acc_msg_seq(5), 6);
        assert!(!challenge.set_auth(AUTH_MODE_CAVE));
        assert_eq!(challenge.acc_msg_seq(5), 6);
    }

    #[test]
    fn startup_acc_msg_seq_varies_between_runs() {
        let seqs: std::collections::HashSet<u8> = (0..64)
            .map(|_| GlobalChallenge::with_auth(AUTH_MODE_CAVE).acc_msg_seq(0))
            .collect();
        assert!(seqs.len() > 1);
    }

    #[test]
    fn enabling_auth_forgets_the_rand_broadcast_without_auth() {
        let challenge = GlobalChallenge::new(AUTH_MODE_NONE, 0x8B00_0001);
        assert!(challenge.set_auth(AUTH_MODE_CAVE));
        assert_eq!(challenge.rand_for_randc(0x8B), None);
        assert_eq!(
            challenge.rand_for_randc(randc(challenge.current())),
            Some(challenge.current())
        );
    }
}
