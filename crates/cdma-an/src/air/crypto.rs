//! Diffie-Hellman key-exchange helpers for HRPD AN key setup.
//!
//! Split out of the air module; the controller reaches these through the
//! `super`-imported names.

use super::*;

pub(super) fn new_dh_key_exchange(transaction_id: u8) -> Option<DhKeyExchangeState> {
    let p = dh_prime_768();
    let g = BigUint::from(2u8);
    let an_private = random_dh_private(&p)?;
    let an_public = fixed_width_be(&g.modpow(&an_private, &p), DH_KEY_LENGTH_OCTETS_768);
    Some(DhKeyExchangeState {
        transaction_id,
        an_private,
        an_public,
        session_key: None,
        nonce: None,
        timestamp_long: None,
    })
}

fn random_dh_private(p: &BigUint) -> Option<BigUint> {
    let mut bytes = [0u8; DH_KEY_LENGTH_OCTETS_768];
    getrandom::getrandom(&mut bytes).ok()?;
    let range = p - BigUint::from(2u8);
    let value = BigUint::from_bytes_be(&bytes) % &range;
    Some(value + BigUint::from(1u8))
}

pub(super) fn random_u16() -> u16 {
    let mut bytes = [0u8; 2];
    if getrandom::getrandom(&mut bytes).is_err() {
        return (cdma_system_time_80ms_now() as u16) ^ 0x5a5a;
    }
    u16::from_be_bytes(bytes)
}

pub(super) fn dh_compute_session_key(an_private: &BigUint, at_public: &[u8]) -> Vec<u8> {
    let p = dh_prime_768();
    let at_public = BigUint::from_bytes_be(at_public);
    fixed_width_be(&at_public.modpow(an_private, &p), DH_KEY_LENGTH_OCTETS_768)
}

pub(super) fn dh_key_signature(
    session_key: &[u8],
    transaction_id: u8,
    nonce: u16,
    timestamp_long: u64,
) -> [u8; 20] {
    let mut hasher = Sha1::new();
    hasher.update(session_key);
    hasher.update([transaction_id]);
    hasher.update(nonce.to_be_bytes());
    hasher.update(timestamp_long.to_be_bytes());
    hasher.finalize().into()
}

fn fixed_width_be(value: &BigUint, octets: usize) -> Vec<u8> {
    let bytes = value.to_bytes_be();
    if bytes.len() >= octets {
        return bytes[bytes.len() - octets..].to_vec();
    }
    let mut out = vec![0; octets - bytes.len()];
    out.extend_from_slice(&bytes);
    out
}

fn dh_prime_768() -> BigUint {
    BigUint::from_bytes_be(&[
        0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xc9, 0x0f, 0xda, 0xa2, 0x21, 0x68, 0xc2,
        0x34, 0xc4, 0xc6, 0x62, 0x8b, 0x80, 0xdc, 0x1c, 0xd1, 0x29, 0x02, 0x4e, 0x08, 0x8a, 0x67,
        0xcc, 0x74, 0x02, 0x0b, 0xbe, 0xa6, 0x3b, 0x13, 0x9b, 0x22, 0x51, 0x4a, 0x08, 0x79, 0x8e,
        0x34, 0x04, 0xdd, 0xef, 0x95, 0x19, 0xb3, 0xcd, 0x3a, 0x43, 0x1b, 0x30, 0x2b, 0x0a, 0x6d,
        0xf2, 0x5f, 0x14, 0x37, 0x4f, 0xe1, 0x35, 0x6d, 0x6d, 0x51, 0xc2, 0x45, 0xe4, 0x85, 0xb5,
        0x76, 0x62, 0x5e, 0x7e, 0xc6, 0xf4, 0x4c, 0x42, 0xe9, 0xa6, 0x3a, 0x36, 0x20, 0xff, 0xff,
        0xff, 0xff, 0xff, 0xff, 0xff, 0xff,
    ])
}
