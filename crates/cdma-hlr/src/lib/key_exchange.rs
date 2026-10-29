//! OTASP A-key generation by Diffie-Hellman (C.S0016-D §5.2, §5.4).

use num_bigint::BigUint;

use cdma_cave::f0::{A_KEY_FAMILY_KEY, F0_TYPE_IDENTIFIER, f0};

/// A_KEY_P_REV `00000010`: 2G A-key over a base-station-chosen P and G.
pub const A_KEY_P_REV_2G: u8 = 0b0000_0010;
/// A_KEY_P_REV `00000011`: 2G A-key and 3G root key over the fixed 1024-bit group.
pub const A_KEY_P_REV_2G_3G: u8 = 0b0000_0011;

const P_2G_BITS: u64 = 512;
const P_2G_OCTETS: usize = 64;
/// C.S0016-D §5.2.1: PARAM_G is a 160-bit number.
const G_2G_BITS: u64 = 160;
/// Bit length of the prime factor q of P-1, so P-1 has a large prime factor.
const P_2G_FACTOR_BITS: u64 = 256;
/// §5.2.1 and §5.4.1: the secret exponent Y is 160 bits for rev 2 and 256 bits for rev 3.
const Y_2G_BITS: u64 = 160;
const Y_FIXED_GROUP_BITS: u64 = 256;
const SECRET_MIN: u32 = 4;
// Forty independent Miller-Rabin rounds bound false-prime probability by 2^-80.
const MILLER_RABIN_ROUNDS: usize = 40;
const SMALL_PRIMES: [u32; 12] = [2, 3, 5, 7, 11, 13, 17, 19, 23, 29, 31, 37];
const OCTET_BITS: u64 = u8::BITS as u64;

/// C.S0016-D §5.4.1 fixed 1024-bit prime, most significant word first.
const P_FIXED_GROUP: [u32; 32] = [
    0xFFFFFFFF, 0xFFFFFFFF, 0xC90FDAA2, 0x2168C234, 0xC4C6628B, 0x80DC1CD1, 0x29024E08, 0x8A67CC74,
    0x020BBEA6, 0x3B139B22, 0x514A0879, 0x8E3404DD, 0xEF9519B3, 0xCD3A431B, 0x302B0A6D, 0xF25F1437,
    0x4FE1356D, 0x6D51C245, 0xE485B576, 0x625E7EC6, 0xF44C42E9, 0xA637ED6B, 0x0BFF5CB6, 0xF406B7ED,
    0xEE386BFB, 0x5A899FA5, 0xAE9F2411, 0x7C4B1FE6, 0x49286651, 0xECE65381, 0xFFFFFFFF, 0xFFFFFFFF,
];
const P_FIXED_GROUP_OCTETS: usize = 128;
const G_FIXED_GROUP: u32 = 2;

const A_KEY_OCTETS: usize = 8;
const RAND_SEED_OCTETS: usize = 16;

/// The base station side of one key exchange. `y` is secret.
#[derive(Clone)]
pub struct KeyExchange {
    pub a_key_p_rev: u8,
    p: BigUint,
    g: BigUint,
    y: BigUint,
}

impl std::fmt::Debug for KeyExchange {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("KeyExchange")
            .field("a_key_p_rev", &self.a_key_p_rev)
            .finish_non_exhaustive()
    }
}

/// The values sent to the mobile in the MS Key Request and Key Generation Request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyExchangeOffer {
    pub a_key_p_rev: u8,
    /// PARAM_P and PARAM_G, sent only for A_KEY_P_REV `00000010`.
    pub params: Option<(Vec<u8>, Vec<u8>)>,
    pub bs_result: Vec<u8>,
}

fn random_bits(bits: u64) -> BigUint {
    let octets = bits.div_ceil(OCTET_BITS) as usize;
    let mut bytes = vec![0u8; octets];
    rand::Rng::fill(&mut rand::thread_rng(), bytes.as_mut_slice());
    let excess = octets as u64 * OCTET_BITS - bits;
    bytes[0] &= u8::MAX >> excess;
    BigUint::from_bytes_be(&bytes)
}

fn random_exact_bits(bits: u64) -> BigUint {
    random_bits(bits) | (BigUint::from(1u8) << (bits - 1))
}

fn random_secret(bits: u64) -> BigUint {
    loop {
        let v = random_bits(bits);
        if v >= BigUint::from(SECRET_MIN) {
            return v;
        }
    }
}

fn is_probable_prime(n: &BigUint) -> bool {
    let one = BigUint::from(1u8);
    let two = BigUint::from(2u8);
    if *n < two {
        return false;
    }
    for small in SMALL_PRIMES {
        let small = BigUint::from(small);
        if *n == small {
            return true;
        }
        if (n % &small) == BigUint::from(0u8) {
            return false;
        }
    }
    let n_minus_one = n - &one;
    let s = n_minus_one.trailing_zeros().unwrap_or(0);
    let d = &n_minus_one >> s;
    'witness: for _ in 0..MILLER_RABIN_ROUNDS {
        let a = (random_bits(n.bits()) % (n - 3u8)) + &two;
        let mut x = a.modpow(&d, n);
        if x == one || x == n_minus_one {
            continue;
        }
        for _ in 1..s {
            x = x.modpow(&two, n);
            if x == n_minus_one {
                continue 'witness;
            }
        }
        return false;
    }
    true
}

fn random_prime(bits: u64) -> BigUint {
    loop {
        let candidate = random_exact_bits(bits) | BigUint::from(1u8);
        if is_probable_prime(&candidate) {
            return candidate;
        }
    }
}

/// C.S0016-D §5.2.1: a random 512-bit P with its top bit set and a large prime factor of P-1.
fn generate_p_2g() -> BigUint {
    let q = random_prime(P_2G_FACTOR_BITS);
    let one = BigUint::from(1u8);
    loop {
        let k = random_bits(P_2G_BITS - P_2G_FACTOR_BITS - 1);
        let p = ((&k * &q) << 1u8) + &one;
        if p.bits() == P_2G_BITS && is_probable_prime(&p) {
            return p;
        }
    }
}

fn fixed_width(value: &BigUint, octets: usize) -> Vec<u8> {
    let bytes = value.to_bytes_be();
    let mut out = vec![0u8; octets.saturating_sub(bytes.len())];
    out.extend_from_slice(&bytes);
    out
}

fn fixed_group_prime() -> BigUint {
    BigUint::from_slice(&P_FIXED_GROUP.iter().rev().copied().collect::<Vec<_>>())
}

impl KeyExchange {
    /// Generates fresh parameters and the secret exponent. A_KEY_P_REV 2 searches for a
    /// 512-bit prime, so callers on an async runtime run this on a blocking thread.
    pub fn new(a_key_p_rev: u8) -> Result<Self, String> {
        let (p, g, y_bits) = match a_key_p_rev {
            A_KEY_P_REV_2G => {
                let p = generate_p_2g();
                let g = random_exact_bits(G_2G_BITS);
                (p, g, Y_2G_BITS)
            }
            A_KEY_P_REV_2G_3G => (
                fixed_group_prime(),
                BigUint::from(G_FIXED_GROUP),
                Y_FIXED_GROUP_BITS,
            ),
            other => return Err(format!("A_KEY_P_REV {other} is not supported")),
        };
        Ok(Self {
            a_key_p_rev,
            p,
            g,
            y: random_secret(y_bits),
        })
    }

    fn result_octets(&self) -> usize {
        match self.a_key_p_rev {
            A_KEY_P_REV_2G => P_2G_OCTETS,
            _ => P_FIXED_GROUP_OCTETS,
        }
    }

    pub fn offer(&self) -> KeyExchangeOffer {
        let bs_result = self.g.modpow(&self.y, &self.p);
        KeyExchangeOffer {
            a_key_p_rev: self.a_key_p_rev,
            params: (self.a_key_p_rev == A_KEY_P_REV_2G)
                .then(|| (fixed_width(&self.p, P_2G_OCTETS), self.g.to_bytes_be())),
            bs_result: fixed_width(&bs_result, self.result_octets()),
        }
    }

    /// The A-key from the mobile's MS_RESULT (§5.2.3, §5.4.3).
    pub fn a_key(&self, ms_result: &[u8]) -> Result<[u8; A_KEY_OCTETS], String> {
        let ms_result = BigUint::from_bytes_be(ms_result);
        let one = BigUint::from(1u8);
        if ms_result <= one || ms_result >= &self.p - &one {
            return Err("MS_RESULT is outside 1 < MS_RESULT < P-1".into());
        }
        let k = fixed_width(&ms_result.modpow(&self.y, &self.p), self.result_octets());
        let mut a_key = [0u8; A_KEY_OCTETS];
        match self.a_key_p_rev {
            A_KEY_P_REV_2G => a_key.copy_from_slice(&k[k.len() - A_KEY_OCTETS..]),
            _ => {
                let mut seed = [0u8; RAND_SEED_OCTETS];
                seed.copy_from_slice(&k[k.len() - RAND_SEED_OCTETS..]);
                a_key = f0(&seed, F0_TYPE_IDENTIFIER, &A_KEY_FAMILY_KEY, 0);
            }
        }
        Ok(a_key)
    }
}

/// The mobile station side (§5.1, §5.3): A_KEY_TEMP and the MS_RESULT to send.
pub fn mobile_station_key_exchange(
    offer: &KeyExchangeOffer,
) -> Result<([u8; A_KEY_OCTETS], Vec<u8>), String> {
    let (p, g, x_bits, octets) = match (offer.a_key_p_rev, &offer.params) {
        (A_KEY_P_REV_2G, Some((p, g))) => (
            BigUint::from_bytes_be(p),
            BigUint::from_bytes_be(g),
            Y_2G_BITS,
            P_2G_OCTETS,
        ),
        (A_KEY_P_REV_2G_3G, None) => (
            fixed_group_prime(),
            BigUint::from(G_FIXED_GROUP),
            Y_FIXED_GROUP_BITS,
            P_FIXED_GROUP_OCTETS,
        ),
        (rev, _) => return Err(format!("unsupported key exchange A_KEY_P_REV {rev}")),
    };
    let x = random_secret(x_bits);
    let ms_result = fixed_width(&g.modpow(&x, &p), octets);
    let k = fixed_width(
        &BigUint::from_bytes_be(&offer.bs_result).modpow(&x, &p),
        octets,
    );
    let mut a_key = [0u8; A_KEY_OCTETS];
    match offer.a_key_p_rev {
        A_KEY_P_REV_2G => a_key.copy_from_slice(&k[k.len() - A_KEY_OCTETS..]),
        _ => {
            let mut seed = [0u8; RAND_SEED_OCTETS];
            seed.copy_from_slice(&k[k.len() - RAND_SEED_OCTETS..]);
            a_key = f0(&seed, F0_TYPE_IDENTIFIER, &A_KEY_FAMILY_KEY, 0);
        }
    }
    Ok((a_key, ms_result))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rev2_parameters_follow_section_5_2_1() {
        let exchange = KeyExchange::new(A_KEY_P_REV_2G).unwrap();
        let offer = exchange.offer();
        let (p, g) = offer.params.clone().unwrap();
        assert_eq!(p.len(), P_2G_OCTETS);
        assert!(p[0] & 0x80 != 0, "P's most significant bit is set");
        assert!(is_probable_prime(&BigUint::from_bytes_be(&p)));
        assert_eq!(BigUint::from_bytes_be(&g).bits(), G_2G_BITS);
        assert_eq!(offer.bs_result.len(), P_2G_OCTETS);
    }

    #[test]
    fn both_sides_agree_on_the_a_key() {
        for rev in [A_KEY_P_REV_2G, A_KEY_P_REV_2G_3G] {
            let exchange = KeyExchange::new(rev).unwrap();
            let offer = exchange.offer();
            let (mobile, ms_result) = mobile_station_key_exchange(&offer).unwrap();
            assert_eq!(exchange.a_key(&ms_result).unwrap(), mobile, "rev {rev}");
        }
    }

    #[test]
    fn fixed_group_prime_is_the_oakley_1024_bit_prime() {
        let p = fixed_group_prime();
        assert_eq!(p.bits(), 1024);
        assert!(is_probable_prime(&p));
        assert!(
            KeyExchange::new(A_KEY_P_REV_2G_3G)
                .unwrap()
                .offer()
                .params
                .is_none()
        );
    }

    #[test]
    fn degenerate_ms_result_is_rejected() {
        let exchange = KeyExchange::new(A_KEY_P_REV_2G_3G).unwrap();
        assert!(exchange.a_key(&[1]).is_err());
        assert!(KeyExchange::new(7).is_err());
    }
}
