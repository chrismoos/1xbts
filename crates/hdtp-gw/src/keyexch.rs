//! Crypto-ignition key agreement (Diffie-Hellman).
//!
//! On a cold start the handset requires an encrypted session. It sends a
//! **KeyRequest** (PDU Type 15) carrying its Diffie-Hellman public value, and
//! expects a **KeyReply** carrying the server's public value; both then derive
//! the shared secret that keys the RC5 session cipher ([`crate::rc5`]).
//!
//! The 512-bit DH group (algorithm id 2, prime `p`, generator `g`) is fixed; the
//! values are big-endian.

use num_bigint::BigUint;

/// DH algorithm id (the parameter-block selector byte).
pub const KEYEXCH_ALGO: u8 = 0x02;
/// DH public value / modulus width in bytes (512-bit).
pub const DH_BYTES: usize = 64;

/// The 512-bit DH prime `p` (big-endian).
const P_HEX: &str = "b917379238052ea6f671f1bc693fe32c992b174abc6685085ff07841f617204c\
d15be71ebdab6f119072a7f5c410936cd2dd432ed76a839b86865cd785c17bab";
/// The 512-bit DH generator `g` (big-endian).
const G_HEX: &str = "4e2eec71007d3f2ef4bc06b733f46ae9d3de065f5d1dacf9239fc91ecbc14b67\
ff64e935f8860b07149b9e8bd8973481ecc51bc74bd04bb4fa82b85b28109262";

fn p() -> BigUint {
    BigUint::parse_bytes(P_HEX.as_bytes(), 16).unwrap()
}
fn g() -> BigUint {
    BigUint::parse_bytes(G_HEX.as_bytes(), 16).unwrap()
}

/// Left-pad a big-endian value to exactly [`DH_BYTES`].
fn to_fixed(n: &BigUint) -> Vec<u8> {
    let mut b = n.to_bytes_be();
    if b.len() < DH_BYTES {
        let mut padded = vec![0u8; DH_BYTES - b.len()];
        padded.extend_from_slice(&b);
        b = padded;
    }
    b
}

/// A completed key agreement: the server public value to send in the KeyReply
/// and the shared secret to key the session cipher.
pub struct KeyAgreement {
    pub server_public: Vec<u8>,
    pub shared_secret: Vec<u8>,
}

/// Parse a KeyRequest body (`algo, DeviceIdLen, PublicValueLen, DeviceId,
/// ClientPublicValue`) and return the client's public value bytes.
pub fn parse_client_public(body: &[u8]) -> Option<Vec<u8>> {
    // body[0] = algo, body[1] = DeviceIdLen, body[2] = PublicValueLen.
    if body.len() < 3 {
        return None;
    }
    let did_len = body[1] as usize;
    let pub_len = body[2] as usize;
    let start = 3 + did_len;
    let end = start + pub_len;
    if body.len() < end {
        return None;
    }
    Some(body[start..end].to_vec())
}

/// Run the server half of the DH exchange against the client's public value.
/// `server_private` is the server's secret exponent (big-endian).
pub fn agree(client_public: &[u8], server_private: &[u8]) -> KeyAgreement {
    let p = p();
    let b = BigUint::from_bytes_be(server_private) % (&p - 1u32);
    let server_public = g().modpow(&b, &p);
    let y = BigUint::from_bytes_be(client_public);
    let shared = y.modpow(&b, &p);
    KeyAgreement {
        server_public: to_fixed(&server_public),
        shared_secret: to_fixed(&shared),
    }
}

/// Derive the 16-byte RC5 session key from the shared secret. The exact
/// derivation the handset uses is still being pinned down; this takes the high
/// 16 bytes as a first hypothesis.
pub fn rc5_key(shared_secret: &[u8]) -> [u8; 16] {
    let mut k = [0u8; 16];
    k.copy_from_slice(&shared_secret[..16]);
    k
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dh_shared_secret_agrees_both_ways() {
        // Both sides derive the same secret from the group: with a
        // client private `a` and the server's `agree`, secret = client_pub^b =
        // server_pub^a.
        let a = [0x11u8; 32];
        let p = p();
        let client_pub = to_fixed(&g().modpow(&BigUint::from_bytes_be(&a), &p));
        let server_priv = [0x22u8; 32];
        let ka = agree(&client_pub, &server_priv);
        let client_secret =
            BigUint::from_bytes_be(&ka.server_public).modpow(&BigUint::from_bytes_be(&a), &p);
        assert_eq!(to_fixed(&client_secret), ka.shared_secret);
    }

    #[test]
    fn client_public_parses_from_captured_keyrequest() {
        // Captured body: algo=02, DIDlen=09, Publen=0x40, DeviceId(9), pub(64).
        let mut full = vec![0x02, 0x09, 0x40];
        full.extend_from_slice(&[0x01, 0x00, 0x06, 0x0a, 0x00, 0x5a, 0xd4, 0x21, 0x36]);
        full.extend_from_slice(&[0xABu8; 64]);
        let cp = parse_client_public(&full).unwrap();
        assert_eq!(cp.len(), 64);
        assert!(cp.iter().all(|&x| x == 0xAB));
    }
}
