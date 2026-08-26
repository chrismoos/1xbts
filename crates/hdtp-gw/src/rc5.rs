//! RC5-32 block cipher (Rivest, "The RC5 Encryption Algorithm").
//!
//! HDTP Cipher 1 is "RSA RC5, 32-bit words, 16-byte key, block padding" (HDTP
//! 1.1 draft Table A-4), with the round count carried in the Cipher field's
//! second byte (16 minimum). That is RC5-32/r/16: a 64-bit block, a 128-bit
//! key, and `r` rounds.
//!
//! This module is the cipher primitive only. The session key it operates on is
//! the shared secret established by crypto-ignition (the Diffie-Hellman
//! KeyRequest/KeyReply exchange), whose wire format is not published.

const W_BYTES: usize = 4;
/// RC5 block size: two 32-bit words.
pub const BLOCK: usize = 8;

/// Magic constants for a 32-bit word size (P32, Q32).
const P32: u32 = 0xB7E1_5163;
const Q32: u32 = 0x9E37_79B9;

/// An RC5-32/r/b key schedule.
pub struct Rc5 {
    s: Vec<u32>,
    rounds: usize,
}

impl Rc5 {
    /// Expand `key` into a schedule of `2*(rounds+1)` subkey words.
    pub fn new(key: &[u8], rounds: usize) -> Rc5 {
        let t = 2 * (rounds + 1);

        // Load the key bytes little-endian into c words (byte 0 -> LSB of L[0]).
        let c = key.len().div_ceil(W_BYTES).max(1);
        let mut l = vec![0u32; c];
        for i in (0..key.len()).rev() {
            l[i / W_BYTES] = (l[i / W_BYTES] << 8).wrapping_add(key[i] as u32);
        }

        let mut s = vec![0u32; t];
        s[0] = P32;
        for i in 1..t {
            s[i] = s[i - 1].wrapping_add(Q32);
        }

        // Mix the key into the schedule.
        let (mut a, mut b) = (0u32, 0u32);
        let (mut i, mut j) = (0usize, 0usize);
        for _ in 0..3 * t.max(c) {
            a = s[i].wrapping_add(a).wrapping_add(b).rotate_left(3);
            s[i] = a;
            let ab = a.wrapping_add(b);
            b = l[j].wrapping_add(ab).rotate_left(ab & 31);
            l[j] = b;
            i = (i + 1) % t;
            j = (j + 1) % c;
        }
        Rc5 { s, rounds }
    }

    /// Encrypt one 8-byte block in place.
    pub fn encrypt_block(&self, block: &mut [u8; BLOCK]) {
        let mut a = u32::from_le_bytes([block[0], block[1], block[2], block[3]]);
        let mut b = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);
        a = a.wrapping_add(self.s[0]);
        b = b.wrapping_add(self.s[1]);
        for i in 1..=self.rounds {
            a = (a ^ b).rotate_left(b & 31).wrapping_add(self.s[2 * i]);
            b = (b ^ a).rotate_left(a & 31).wrapping_add(self.s[2 * i + 1]);
        }
        block[0..4].copy_from_slice(&a.to_le_bytes());
        block[4..8].copy_from_slice(&b.to_le_bytes());
    }

    /// Decrypt one 8-byte block in place.
    pub fn decrypt_block(&self, block: &mut [u8; BLOCK]) {
        let mut a = u32::from_le_bytes([block[0], block[1], block[2], block[3]]);
        let mut b = u32::from_le_bytes([block[4], block[5], block[6], block[7]]);
        for i in (1..=self.rounds).rev() {
            b = b.wrapping_sub(self.s[2 * i + 1]).rotate_right(a & 31) ^ a;
            a = a.wrapping_sub(self.s[2 * i]).rotate_right(b & 31) ^ b;
        }
        b = b.wrapping_sub(self.s[1]);
        a = a.wrapping_sub(self.s[0]);
        block[0..4].copy_from_slice(&a.to_le_bytes());
        block[4..8].copy_from_slice(&b.to_le_bytes());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn known_answer_rc5_32_12_16() {
        // Rivest's RC5 paper, RC5-32/12/16: a zero key and zero plaintext
        // encrypt to words (0xEEDBA521, 0x6D8F4B15), i.e. the little-endian
        // block 21 A5 DB EE 15 4B 8F 6D.
        let rc5 = Rc5::new(&[0u8; 16], 12);
        let mut block = [0u8; BLOCK];
        rc5.encrypt_block(&mut block);
        assert_eq!(block, [0x21, 0xA5, 0xDB, 0xEE, 0x15, 0x4B, 0x8F, 0x6D]);
        rc5.decrypt_block(&mut block);
        assert_eq!(block, [0u8; BLOCK]);
    }

    #[test]
    fn round_trips_nonzero() {
        let key = [
            0x91, 0x5F, 0x46, 0x19, 0xBE, 0x41, 0xB2, 0x51, 0x63, 0x55, 0xA5, 0x01, 0x10, 0xA9,
            0xCE, 0x91,
        ];
        let rc5 = Rc5::new(&key, 16);
        let pt = [0xDE, 0xAD, 0xBE, 0xEF, 0x01, 0x23, 0x45, 0x67];
        let mut block = pt;
        rc5.encrypt_block(&mut block);
        assert_ne!(block, pt);
        rc5.decrypt_block(&mut block);
        assert_eq!(block, pt);
    }
}
