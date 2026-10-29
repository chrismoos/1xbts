//! The f0 pseudo-random function (S.S0055-A §2.2.2.2, Exhibit 3-9), used by
//! OTASP A_KEY_P_REV `00000011` to derive the A-key and root key from the
//! Diffie-Hellman secret (C.S0016-D §5.3.3, §5.4.3).

const SHA1_IV: [u32; 5] = [
    0x6745_2301,
    0xEFCD_AB89,
    0x98BA_DCFE,
    0x1032_5476,
    0xC3D2_E1F0,
];
const SHA1_K: [u32; 4] = [0x5A82_7999, 0x6ED9_EBA1, 0x8F1B_BCDC, 0xCA62_C1D6];
// S.S0055-A §2.2.2.2 uses the SHA-1 compression function without message padding.
const SHA1_ROUNDS: usize = 80;
const SHA1_INPUT_WORDS: usize = 16;
const SHA1_PHASE_ROUNDS: usize = 20;
const SHA1_THIRD_PHASE_START: usize = 2 * SHA1_PHASE_ROUNDS;
const SHA1_FIRST_PHASE_END: usize = SHA1_PHASE_ROUNDS - 1;
const SHA1_SECOND_PHASE_END: usize = 2 * SHA1_PHASE_ROUNDS - 1;
const SHA1_THIRD_PHASE_END: usize = 3 * SHA1_PHASE_ROUNDS - 1;
const SHA1_A_ROTATION_BITS: u32 = 5;
const SHA1_B_ROTATION_BITS: u32 = 30;
const WORD_OCTETS: usize = size_of::<u32>();
const LIMB_BITS: usize = u64::BITS as usize;
const LIMB_OCTETS: usize = size_of::<u64>();
const WHITEN_LIMBS: usize = 3;
const WHITEN_PRODUCT_LIMBS: usize = 5;
const WHITEN_STORAGE_OCTETS: usize = WHITEN_LIMBS * LIMB_OCTETS;
const SHA1_BLOCK_OCTETS: usize = 64;
const DIGEST_OCTETS: usize = 20;
const DIGEST_BITS: usize = DIGEST_OCTETS * u8::BITS as usize;
const SEED_OCTETS: usize = 16;

/// Exhibit 3-9 payload filler and field offsets.
const PAYLOAD_FILL: u8 = 0x5C;
const COUNTER_OFFSETS: [usize; 4] = [0, 16, 32, 48];
const TYPE_IDENTIFIER_OFFSET: usize = 11;
const FAMILY_KEY_OFFSET: usize = 12;

/// Exhibit 3-9 whitening constants for (A*X + B) mod (T^160 + T^5 + T^3 + T^2 + 1).
const WHITEN_A: [u8; DIGEST_OCTETS] = [
    0x9d, 0xe9, 0xc9, 0xc8, 0xef, 0xd5, 0x78, 0x11, 0x48, 0x23, 0x14, 0x01, 0x90, 0x1f, 0x2d, 0x49,
    0x3f, 0x4c, 0x63, 0x65,
];
const WHITEN_B: [u8; DIGEST_OCTETS] = [
    0x75, 0xef, 0xd1, 0x5c, 0x4b, 0x8f, 0x8f, 0x51, 0x4e, 0xf3, 0xbc, 0xc3, 0x79, 0x4a, 0x76, 0x5e,
    0x7e, 0xec, 0x45, 0xe0,
];
/// The low terms T^5 + T^3 + T^2 + 1 of the reduction polynomial.
const WHITEN_G_LOW: u8 = 0x2d;

/// f0 type identifier and the family key C.S0016-D §5.3.3 specifies for A-key generation.
pub const F0_TYPE_IDENTIFIER: u8 = 0x41;
pub const A_KEY_FAMILY_KEY: [u8; 4] = [0x41, 0x48, 0x41, 0x47];

pub const F0_OUTPUT_OCTETS: usize = 8;

/// One SHA-1 compression over a single 512-bit block from `state`, without padding.
fn sha1_compress(state: &mut [u32; 5], block: &[u8; SHA1_BLOCK_OCTETS]) {
    let mut w = [0u32; SHA1_ROUNDS];
    for (i, word) in block.chunks_exact(WORD_OCTETS).enumerate() {
        w[i] = u32::from_be_bytes([word[0], word[1], word[2], word[3]]);
    }
    for t in SHA1_INPUT_WORDS..SHA1_ROUNDS {
        w[t] = (w[t - 3] ^ w[t - 8] ^ w[t - 14] ^ w[t - 16]).rotate_left(1);
    }
    let [mut a, mut b, mut c, mut d, mut e] = *state;
    for (t, &wt) in w.iter().enumerate() {
        let (f, k) = match t {
            0..=SHA1_FIRST_PHASE_END => ((b & c) | (!b & d), SHA1_K[0]),
            SHA1_PHASE_ROUNDS..=SHA1_SECOND_PHASE_END => (b ^ c ^ d, SHA1_K[1]),
            SHA1_THIRD_PHASE_START..=SHA1_THIRD_PHASE_END => {
                ((b & c) | (b & d) | (c & d), SHA1_K[2])
            }
            _ => (b ^ c ^ d, SHA1_K[3]),
        };
        let temp = a
            .rotate_left(SHA1_A_ROTATION_BITS)
            .wrapping_add(f)
            .wrapping_add(e)
            .wrapping_add(k)
            .wrapping_add(wt);
        e = d;
        d = c;
        c = b.rotate_left(SHA1_B_ROTATION_BITS);
        b = a;
        a = temp;
    }
    for (s, v) in state.iter_mut().zip([a, b, c, d, e]) {
        *s = s.wrapping_add(v);
    }
}

/// (A*X + B) over GF(2)[T] modulo T^160 + T^5 + T^3 + T^2 + 1, most significant bit first.
fn whiten(x: &[u8; DIGEST_OCTETS]) -> [u8; DIGEST_OCTETS] {
    let to_limbs = |bytes: &[u8; DIGEST_OCTETS]| -> [u64; WHITEN_LIMBS] {
        let mut padded = [0u8; WHITEN_STORAGE_OCTETS];
        padded[WHITEN_STORAGE_OCTETS - DIGEST_OCTETS..].copy_from_slice(bytes);
        [0, 1, 2].map(|i| {
            u64::from_be_bytes(
                padded[i * LIMB_OCTETS..(i + 1) * LIMB_OCTETS]
                    .try_into()
                    .expect("slice contains one complete word"),
            )
        })
    };
    let a = to_limbs(&WHITEN_A);
    let xv = to_limbs(x);
    // Carry-less product of two 160-bit polynomials, held in five 64-bit limbs.
    let mut product = [0u64; WHITEN_PRODUCT_LIMBS];
    let bit =
        |v: &[u64; WHITEN_LIMBS], i: usize| (v[2 - i / LIMB_BITS] >> (i % LIMB_BITS)) & 1 == 1;
    for i in 0..DIGEST_BITS {
        if !bit(&xv, i) {
            continue;
        }
        for j in 0..DIGEST_BITS {
            if bit(&a, j) {
                let k = i + j;
                product[4 - k / LIMB_BITS] ^= 1 << (k % LIMB_BITS);
            }
        }
    }
    let get = |p: &[u64; WHITEN_PRODUCT_LIMBS], k: usize| {
        (p[4 - k / LIMB_BITS] >> (k % LIMB_BITS)) & 1 == 1
    };
    for k in (DIGEST_BITS..2 * DIGEST_BITS).rev() {
        if get(&product, k) {
            product[4 - k / LIMB_BITS] ^= 1 << (k % LIMB_BITS);
            for low in 0..u8::BITS as usize {
                if (WHITEN_G_LOW >> low) & 1 == 1 {
                    let target = k - DIGEST_BITS + low;
                    product[4 - target / LIMB_BITS] ^= 1 << (target % LIMB_BITS);
                }
            }
        }
    }
    let mut out = [0u8; DIGEST_OCTETS];
    let mut bytes = [0u8; WHITEN_STORAGE_OCTETS];
    for i in 0..WHITEN_LIMBS {
        bytes[i * LIMB_OCTETS..(i + 1) * LIMB_OCTETS]
            .copy_from_slice(&product[2 + i].to_be_bytes());
    }
    out.copy_from_slice(&bytes[WHITEN_STORAGE_OCTETS - DIGEST_OCTETS..]);
    for (o, b) in out.iter_mut().zip(WHITEN_B) {
        *o ^= b;
    }
    out
}

/// f0 with an explicit counter. The caller advances `counter` by one per invocation.
pub fn f0(
    seed: &[u8; SEED_OCTETS],
    type_identifier: u8,
    family_key: &[u8; 4],
    counter: u64,
) -> [u8; F0_OUTPUT_OCTETS] {
    let mut iv = [0u8; DIGEST_OCTETS];
    for (i, word) in SHA1_IV.iter().enumerate() {
        iv[i * WORD_OCTETS..(i + 1) * WORD_OCTETS].copy_from_slice(&word.to_be_bytes());
    }
    for (v, s) in iv.iter_mut().zip(seed) {
        *v ^= s;
    }
    let mut state = [0u32; 5];
    for (i, s) in state.iter_mut().enumerate() {
        *s = u32::from_be_bytes(
            iv[i * WORD_OCTETS..(i + 1) * WORD_OCTETS]
                .try_into()
                .expect("slice contains one complete word"),
        );
    }
    let mut block = [PAYLOAD_FILL; SHA1_BLOCK_OCTETS];
    let counter = counter.to_be_bytes();
    for offset in COUNTER_OFFSETS {
        for (b, c) in block[offset..offset + 8].iter_mut().zip(counter) {
            *b ^= c;
        }
    }
    block[TYPE_IDENTIFIER_OFFSET] ^= type_identifier;
    for (b, f) in block[FAMILY_KEY_OFFSET..FAMILY_KEY_OFFSET + 4]
        .iter_mut()
        .zip(family_key)
    {
        *b ^= f;
    }
    sha1_compress(&mut state, &block);
    let mut digest = [0u8; DIGEST_OCTETS];
    for (i, word) in state.iter().enumerate() {
        digest[i * WORD_OCTETS..(i + 1) * WORD_OCTETS].copy_from_slice(&word.to_be_bytes());
    }
    let whitened = whiten(&digest);
    let mut out = [0u8; F0_OUTPUT_OCTETS];
    out.copy_from_slice(&whitened[..F0_OUTPUT_OCTETS]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn f0_matches_s_s0055_vector() {
        // S.S0055-A Exhibit 4-3: two invocations from a zero counter.
        let seed = [
            0xb0, 0xab, 0xb9, 0x9d, 0x6a, 0xc6, 0xa7, 0x4e, 0xb9, 0x8e, 0xb6, 0xc2, 0xda, 0xb1,
            0xa5, 0x51,
        ];
        assert_eq!(
            f0(&seed, F0_TYPE_IDENTIFIER, &A_KEY_FAMILY_KEY, 0),
            [0x4b, 0x05, 0x2b, 0x20, 0xe2, 0xa0, 0x6c, 0x8f]
        );
        assert_eq!(
            f0(&seed, F0_TYPE_IDENTIFIER, &A_KEY_FAMILY_KEY, 1),
            [0xf7, 0x00, 0xda, 0x51, 0x2b, 0x4e, 0x11, 0x1e]
        );
    }
}
