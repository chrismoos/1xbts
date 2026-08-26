//! HDTP cipher negotiation and the session trailer.
//!
//! HDTP encrypts each PDU and appends a trailer carrying the encrypted
//! integrity checksum (HDTP 1.1 draft, "Security"). A two-byte Cipher field
//! names the algorithm and its parameters; the assigned algorithms are:
//!
//! | Byte 0 | Algorithm                                             |
//! |--------|-------------------------------------------------------|
//! | 0      | No encryption (byte 1 undefined, trailer length zero) |
//! | 1      | RSA RC5, 32-bit words, 16-byte key, block padding      |
//!
//! Session 0 (creation/meta) is unencrypted by definition, and the handset we
//! target proposes Cipher 0 for the user session as well. This module implements
//! Cipher 0 fully (the identity) and provides the RC5 primitive for Cipher 1
//! ([`Cipher::rc5_encrypt`]/[`rc5_decrypt`], keyed by the shared secret). What
//! remains unimplemented for an end-to-end encrypted session is the key
//! agreement that produces that shared secret (crypto-ignition / Diffie-Hellman
//! KeyRequest/KeyReply, wire format unpublished) and the per-PDU integrity
//! trailer.

use crate::rc5::{BLOCK, Rc5};

/// Assigned cipher-algorithm numbers (HDTP 1.1 draft, Table A-4).
pub const CIPHER_NONE: u8 = 0;
pub const CIPHER_RC5: u8 = 1;
/// Session-key RC5, keyed by the crypto-ignition shared secret for an encrypted
/// session.
pub const CIPHER_RC5_SESSION: u8 = 2;

/// The cipher-2 integrity trailer is a single RC5-32/12 block encryption of a
/// nonce structure (verified against on-wire trailers). Its parameters differ
/// from the Cipher-1 defaults:
///   * 12 rounds (from the Cipher parameter byte, not the 16-round minimum);
///   * a 40-bit key: the first five bytes of the 16-byte session key;
///   * a fixed IV XORed into the block before encryption;
///   * the block is four big-endian 16-bit words — the nonce and three fixed
///     derivatives of it.
const CIPHER2_ROUNDS: usize = 12;
const CIPHER2_KEY_BYTES: usize = 5;
const CIPHER2_IV: [u8; 8] = [0x17, 0xb3, 0x49, 0x9f, 0x8d, 0x0a, 0x15, 0xe8];
const CIPHER2_NONCE_XOR: [u16; 4] = [0x0000, 0x85e2, 0xbdaf, 0x7fa1];

/// XOR constant relating the second block word to the nonce: a valid trailer
/// decrypts to a block whose word 1 equals `nonce ^ CIPHER2_NONCE_MOD`.
pub const CIPHER2_NONCE_MOD: u16 = 0x85e2;

/// Build the 8-byte cipher-2 trailer for `nonce` under `key` (>= 5 bytes).
pub fn cipher2_trailer(key: &[u8], nonce: u16) -> [u8; 8] {
    let mut block = [0u8; 8];
    for (i, xor) in CIPHER2_NONCE_XOR.iter().enumerate() {
        let w = (nonce ^ xor).to_be_bytes();
        block[2 * i] = w[0] ^ CIPHER2_IV[2 * i];
        block[2 * i + 1] = w[1] ^ CIPHER2_IV[2 * i + 1];
    }
    let key5 = &key[..CIPHER2_KEY_BYTES.min(key.len())];
    Rc5::new(key5, CIPHER2_ROUNDS).encrypt_block(&mut block);
    block
}

/// CBC-encrypt `plaintext` (padded up to an 8-byte multiple) under the cipher-2
/// key, chain starting at the fixed IV — the handset CBC-decrypts this region to
/// recover the inner message.
pub fn cipher2_cbc_encrypt(key: &[u8], plaintext: &[u8]) -> Vec<u8> {
    let key5 = &key[..CIPHER2_KEY_BYTES.min(key.len())];
    let rc5 = Rc5::new(key5, CIPHER2_ROUNDS);
    let mut chain = CIPHER2_IV;
    let mut out = Vec::new();
    for blk in plaintext.chunks(BLOCK) {
        let mut b = [0u8; BLOCK];
        b[..blk.len()].copy_from_slice(blk);
        for i in 0..BLOCK {
            b[i] ^= chain[i];
        }
        rc5.encrypt_block(&mut b);
        chain = b;
        out.extend_from_slice(&b);
    }
    out
}

/// Inverse of [`cipher2_cbc_encrypt`]: recover the plaintext from a cipher-2
/// nested-CBC region (chain starts at the IV, `plain = RC5dec(block) ^ chain`,
/// `chain = block`). Used to read the handset's own encrypted requests.
pub fn cipher2_cbc_decrypt(key: &[u8], ciphertext: &[u8]) -> Vec<u8> {
    let key5 = &key[..CIPHER2_KEY_BYTES.min(key.len())];
    let rc5 = Rc5::new(key5, CIPHER2_ROUNDS);
    let mut chain = CIPHER2_IV;
    let mut out = Vec::new();
    for blk in ciphertext.chunks(BLOCK) {
        if blk.len() < BLOCK {
            break;
        }
        let mut b = [0u8; BLOCK];
        b.copy_from_slice(blk);
        let cipher_block = b;
        rc5.decrypt_block(&mut b);
        for i in 0..BLOCK {
            b[i] ^= chain[i];
        }
        chain = cipher_block;
        out.extend_from_slice(&b);
    }
    out
}

/// The handset's 4-byte message MAC: a plain XOR-fold of `data` into 4 bytes
/// (`mac[i % 4] ^= data[i]`), not keyed.
pub fn xor_fold4(data: &[u8]) -> [u8; 4] {
    let mut mac = [0u8; 4];
    for (i, b) in data.iter().enumerate() {
        mac[i % 4] ^= b;
    }
    mac
}

/// Compute the RC5-CBC-MAC trailer that makes the region `content` followed by
/// this 8-byte trailer verify with a zero residual, matching the handset's
/// cipher-2 integrity check (chain starts at the IV, `chain = RC5enc(block ^
/// chain)`). `content` must be a whole number of 8-byte blocks.
pub fn cipher2_cbcmac_trailer(key: &[u8], content: &[u8]) -> [u8; 8] {
    let key5 = &key[..CIPHER2_KEY_BYTES.min(key.len())];
    let rc5 = Rc5::new(key5, CIPHER2_ROUNDS);
    let mut chain = CIPHER2_IV;
    for blk in content.chunks(BLOCK) {
        let mut b = [0u8; BLOCK];
        b[..blk.len()].copy_from_slice(blk);
        for i in 0..BLOCK {
            b[i] ^= chain[i];
        }
        rc5.encrypt_block(&mut b);
        chain = b;
    }
    // trailer = RC5dec(0) XOR chain, so RC5enc(trailer ^ chain) == RC5enc(RC5dec(0)) == 0.
    let mut zero = [0u8; BLOCK];
    rc5.decrypt_block(&mut zero);
    let mut t = [0u8; BLOCK];
    for i in 0..BLOCK {
        t[i] = zero[i] ^ chain[i];
    }
    t
}

/// Recover the nonce from a received cipher-2 trailer. Returns `(nonce, word1)`;
/// for a valid trailer `word1 == nonce ^ CIPHER2_NONCE_MOD`.
pub fn cipher2_recover_nonce(key: &[u8], trailer: &[u8; 8]) -> (u16, u16) {
    let key5 = &key[..CIPHER2_KEY_BYTES.min(key.len())];
    let mut block = *trailer;
    Rc5::new(key5, CIPHER2_ROUNDS).decrypt_block(&mut block);
    for (b, iv) in block.iter_mut().zip(CIPHER2_IV.iter()) {
        *b ^= iv;
    }
    let nonce = u16::from_be_bytes([block[0], block[1]]);
    let word1 = u16::from_be_bytes([block[2], block[3]]);
    (nonce, word1)
}

/// Minimum RC5 round count (HDTP 1.1 draft: "16 rounds minimum").
pub const RC5_MIN_ROUNDS: u8 = 16;

/// The two-byte Cipher field: algorithm plus one algorithm-specific parameter.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cipher {
    pub algorithm: u8,
    pub param: u8,
}

impl Cipher {
    pub const NONE: Cipher = Cipher {
        algorithm: CIPHER_NONE,
        param: 0,
    };

    pub fn new(algorithm: u8, param: u8) -> Self {
        Cipher { algorithm, param }
    }

    pub fn is_none(&self) -> bool {
        self.algorithm == CIPHER_NONE
    }

    /// Wire bytes of the Cipher field.
    pub fn to_bytes(self) -> [u8; 2] {
        [self.algorithm, self.param]
    }

    pub fn from_bytes(b: [u8; 2]) -> Self {
        Cipher {
            algorithm: b[0],
            param: b[1],
        }
    }

    /// Length in bytes of the encryption trailer this cipher appends to a PDU.
    /// Cipher 0 carries no trailer; anything else is unimplemented.
    pub fn trailer_len(&self) -> usize {
        match self.algorithm {
            CIPHER_NONE => 0,
            _ => 0,
        }
    }

    /// Encrypt a PDU body in place. Cipher 0 is the identity transform.
    pub fn encrypt(&self, _buf: &mut [u8]) -> Result<(), CipherError> {
        match self.algorithm {
            CIPHER_NONE => Ok(()),
            other => Err(CipherError::Unsupported(other)),
        }
    }

    /// Decrypt a PDU body in place. Cipher 0 is the identity transform.
    pub fn decrypt(&self, _buf: &mut [u8]) -> Result<(), CipherError> {
        match self.algorithm {
            CIPHER_NONE => Ok(()),
            other => Err(CipherError::Unsupported(other)),
        }
    }

    /// Rounds for RC5, from the Cipher parameter byte, clamped to the spec
    /// minimum.
    pub fn rc5_rounds(&self) -> usize {
        self.param.max(RC5_MIN_ROUNDS) as usize
    }
}

/// Encrypt `data` under RC5 (Cipher 1) with `key`, in ECB with zero block
/// padding to the 8-byte block. The round count comes from the Cipher
/// parameter. Returns the padded ciphertext.
pub fn rc5_encrypt(key: &[u8], rounds: usize, data: &[u8]) -> Vec<u8> {
    let rc5 = Rc5::new(key, rounds);
    let mut out = data.to_vec();
    let pad = (BLOCK - out.len() % BLOCK) % BLOCK;
    out.extend(std::iter::repeat_n(0u8, pad));
    let (blocks, _) = out.as_chunks_mut::<BLOCK>();
    for block in blocks {
        rc5.encrypt_block(block);
    }
    out
}

/// Decrypt an RC5 (Cipher 1) buffer with `key`. `data` must be a whole number
/// of 8-byte blocks.
pub fn rc5_decrypt(key: &[u8], rounds: usize, data: &[u8]) -> Result<Vec<u8>, CipherError> {
    if !data.len().is_multiple_of(BLOCK) {
        return Err(CipherError::BadLength(data.len()));
    }
    let rc5 = Rc5::new(key, rounds);
    let mut out = data.to_vec();
    let (blocks, _) = out.as_chunks_mut::<BLOCK>();
    for block in blocks {
        rc5.decrypt_block(block);
    }
    Ok(out)
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum CipherError {
    #[error("unsupported cipher algorithm {0}")]
    Unsupported(u8),
    #[error("ciphertext length {0} is not a whole number of blocks")]
    BadLength(usize),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cipher_none_roundtrips_bytes() {
        let c = Cipher::from_bytes([0, 0]);
        assert!(c.is_none());
        assert_eq!(c.trailer_len(), 0);
        assert_eq!(c.to_bytes(), [0, 0]);
    }

    #[test]
    fn cipher_none_is_identity() {
        let mut buf = *b"hello";
        Cipher::NONE.encrypt(&mut buf).unwrap();
        assert_eq!(&buf, b"hello");
        Cipher::NONE.decrypt(&mut buf).unwrap();
        assert_eq!(&buf, b"hello");
    }

    #[test]
    fn rc5_buffer_round_trips_with_padding() {
        let key = [0x11u8; 16];
        let rounds = Cipher::new(CIPHER_RC5, 12).rc5_rounds(); // clamps to 16
        assert_eq!(rounds, 16);
        let msg = b"HDTP encrypted session body of odd length";
        let ct = rc5_encrypt(&key, rounds, msg);
        assert_eq!(ct.len() % BLOCK, 0);
        let pt = rc5_decrypt(&key, rounds, &ct).unwrap();
        assert_eq!(&pt[..msg.len()], msg);
    }

    #[test]
    fn cipher2_trailer_round_trips_nonce() {
        let key = [0x72, 0xa3, 0xd5, 0x68, 0xb1];
        for nonce in [0x0000u16, 0x1a2b, 0x85e2, 0xffff, 0x5ad4] {
            let t = cipher2_trailer(&key, nonce);
            let (n, w1) = cipher2_recover_nonce(&key, &t);
            assert_eq!(n, nonce);
            assert_eq!(w1, nonce ^ CIPHER2_NONCE_MOD);
        }
    }

    #[test]
    fn rc5_decrypt_rejects_partial_block() {
        assert_eq!(
            rc5_decrypt(&[0u8; 16], 16, &[0u8; 5]),
            Err(CipherError::BadLength(5))
        );
    }
}
