//! CAVE authentication, key generation and CMEA (S.S0053-0 v2.0 §2, C.S0005-E
//! §2.3.12, C.S0004-E §2.1.1.1.2).

mod engine;
pub mod f0;
mod table;

use engine::{Cave, REGISTER_COUNT};
use table::CAVE_TABLE;

/// C.S0005-E §2.3.12.1: the authentication signature `AUTH_SIGNATURE` is 18 bits wide.
pub const AUTH_SIGNATURE_BITS: u32 = 18;
pub const AUTH_SIGNATURE_MASK: u32 = (1 << AUTH_SIGNATURE_BITS) - 1;

const AUTH_DATA_BITS: u32 = 24;
const AUTH_DATA_MASK: u32 = (1 << AUTH_DATA_BITS) - 1;

/// `RANDU` in the Authentication Challenge Message is 24 bits (C.S0005-E §2.3.12.1.4).
pub const RANDU_BITS: u32 = 24;
const RANDU_MASK: u32 = (1 << RANDU_BITS) - 1;

/// Bit width of `RANDSSD`, the random seed for SSD generation (C.S0005-E
/// §2.3.12.1.5). Stored as 7 bytes with the top byte carrying the high 8 bits.
pub const RANDSSD_BITS: u32 = 56;

/// Number of dialed digits folded into `AUTH_DATA` for an origination (C.S0004-E §2.1.1.1.2.2).
pub const DIALED_DIGITS: usize = 6;

/// Length of the Voice Privacy Mask in octets (520 bits, S.S0053-0 §2.5.1).
pub const VPM_LEN: usize = 65;

/// Number of decimal digits in a keypad A-key entry: 20 key digits then 6 check digits (S.S0053-0 §2.2.2).
pub const A_KEY_ENTRY_DIGITS: usize = 26;
const A_KEY_VALUE_DIGITS: usize = 20;
const A_KEY_CHECK_DIGITS: usize = 6;
/// A keypad entry shorter than this is rejected (S.S0053-0 §2.2.2).
pub const A_KEY_MIN_ENTRY_DIGITS: usize = 6;

/// Authentication Algorithm Version (S.S0053-0 Exhibit 2-3).
const AAV: u8 = 0xC7;

const AUTH_ROUNDS: usize = 8;
const KEY_ITERATION_ROUNDS: usize = 4;
const INITIAL_OFFSET: u8 = 128;
/// S.S0053-0 Exhibits 2-7, 2-10, 2-14 and 2-15 place procedure input at R09 and ESN at R12.
const PROCEDURE_DATA_REGISTER_START: usize = 9;
const ESN_REGISTER_START: usize = 12;
const OCTET_BITS: u32 = u8::BITS;
const UNIQUE_CHALLENGE_IMSI_S2_MASK: u16 = u8::MAX as u16;

/// Exhibit 2-15: iterations 4..=13 each yield 6 VPM octets and iteration 14 yields the last 5.
const VPM_FULL_ITERATIONS: usize = 10;
const VPM_OCTETS_PER_ITERATION: usize = 6;
const VPM_LAST_ITERATION_OCTETS: usize = 5;

/// C.S0005-E §2.3.6.2: PVTLCM_42 bits M41..M40 are `01` and M39..M0 are the 40 LSBs of the VPM.
const PRIVATE_LONG_CODE_MASK_PREFIX: u64 = 0b01 << 40;
const PRIVATE_LONG_CODE_MASK_VPM_OCTETS: usize = 5;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CaveError {
    #[error("invalid dialed digit {0:?}")]
    InvalidDigit(char),

    #[error("value 0x{value:x} does not fit in a {bits}-bit field")]
    FieldTooWide {
        /// The offending value.
        value: u64,
        /// The permitted field width in bits.
        bits: u32,
    },

    #[error("A-key entry must be {min} to {max} decimal digits, got {got:?}")]
    AKeyEntryFormat { min: usize, max: usize, got: String },

    #[error("A-key entry value does not fit in 64 bits")]
    AKeyOverflow,

    #[error("A-key check digits do not match")]
    AKeyChecksumMismatch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct AKey(pub [u8; 8]);

impl AKey {
    pub fn from_u64(value: u64) -> Self {
        Self(value.to_be_bytes())
    }

    pub fn to_u64(self) -> u64 {
        u64::from_be_bytes(self.0)
    }

    /// The 18-bit A-key checksum (S.S0053-0 §2.2.1, Exhibit 2-7). It depends on the ESN.
    pub fn checksum(&self, esn: Esn) -> u32 {
        let a = &self.0;
        let lfsr = if a[..4].iter().any(|&b| b != 0) {
            [a[0], a[1], a[2], a[3]]
        } else {
            esn.0.to_be_bytes()
        };
        let mut registers = [0u8; REGISTER_COUNT];
        registers[..8].copy_from_slice(a);
        registers[8] = AAV;
        registers[PROCEDURE_DATA_REGISTER_START..ESN_REGISTER_START].copy_from_slice(&a[5..8]);
        registers[ESN_REGISTER_START..].copy_from_slice(&esn.0.to_be_bytes());
        let mut cave = Cave::new(lfsr, registers, INITIAL_OFFSET, INITIAL_OFFSET);
        cave.run(AUTH_ROUNDS);
        cave.signature()
    }

    /// The 26-digit keypad entry: 20 decimal digits of the A-key then 6 check digits (S.S0053-0 §2.2.1).
    pub fn entry_digits(&self, esn: Esn) -> String {
        format!(
            "{:0width_v$}{:0width_c$}",
            self.to_u64(),
            self.checksum(esn),
            width_v = A_KEY_VALUE_DIGITS,
            width_c = A_KEY_CHECK_DIGITS
        )
    }

    /// A-key verification (S.S0053-0 §2.2.2, Exhibit 2-8). Entries of 6 to 26 digits are
    /// left-padded with zeros to 26 digits. The first 20 digits are the A-key and the last 6
    /// are its check digits. Spaces are ignored.
    pub fn from_entry_digits(entry: &str, esn: Esn) -> Result<Self, CaveError> {
        let digits: String = entry.chars().filter(|c| !c.is_whitespace()).collect();
        if digits.len() < A_KEY_MIN_ENTRY_DIGITS
            || digits.len() > A_KEY_ENTRY_DIGITS
            || !digits.bytes().all(|b| b.is_ascii_digit())
        {
            return Err(CaveError::AKeyEntryFormat {
                min: A_KEY_MIN_ENTRY_DIGITS,
                max: A_KEY_ENTRY_DIGITS,
                got: entry.to_string(),
            });
        }
        let padded = format!("{digits:0>A_KEY_ENTRY_DIGITS$}");
        let (value_digits, check_digits) = padded.split_at(A_KEY_VALUE_DIGITS);
        // Exhibit 2-7 mul10 keeps the low 64 bits, so 20-digit values above u64::MAX wrap.
        let mut value = 0u64;
        for b in value_digits.bytes() {
            value = value
                .wrapping_mul(u64::from(DECIMAL_RADIX))
                .wrapping_add(u64::from(b - b'0'));
        }
        let check: u32 = check_digits.parse().map_err(|_| CaveError::AKeyOverflow)?;
        let a_key = AKey::from_u64(value);
        if a_key.checksum(esn) == check {
            Ok(a_key)
        } else {
            Err(CaveError::AKeyChecksumMismatch)
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Esn(pub u32);

/// The 56-bit `RANDSSD` seed, high 8 bits in byte 0.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RandSsd(pub [u8; 7]);

impl RandSsd {
    pub fn from_u64(value: u64) -> Result<Self, CaveError> {
        if value >> RANDSSD_BITS != 0 {
            return Err(CaveError::FieldTooWide {
                value,
                bits: RANDSSD_BITS,
            });
        }
        let bytes = value.to_be_bytes();
        let mut out = [0u8; 7];
        out.copy_from_slice(&bytes[1..]);
        Ok(Self(out))
    }

    pub fn to_u64(self) -> u64 {
        let mut bytes = [0u8; 8];
        bytes[1..].copy_from_slice(&self.0);
        u64::from_be_bytes(bytes)
    }
}

/// `ssd_a` keys the authentication signature. `ssd_b` keys CMEA and VPM derivation (C.S0005-E §2.3.12.1.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Ssd {
    pub ssd_a: [u8; 8],
    pub ssd_b: [u8; 8],
}

/// SSD generation (S.S0053-0 §2.3.1, Exhibit 2-10). The result is SSD_A_NEW / SSD_B_NEW and
/// must only replace the stored SSD once the base station challenge confirms it.
pub fn generate_ssd(a_key: &AKey, esn: Esn, randssd: &RandSsd) -> Ssd {
    let a = &a_key.0;
    let r = &randssd.0;
    let mut lfsr = [0u8; 4];
    for i in 0..4 {
        lfsr[i] = r[i + 3] ^ a[i] ^ a[i + 4];
    }
    if lfsr == [0; 4] {
        lfsr.copy_from_slice(&r[3..7]);
    }
    let mut registers = [0u8; REGISTER_COUNT];
    registers[..8].copy_from_slice(a);
    registers[8] = AAV;
    registers[PROCEDURE_DATA_REGISTER_START..ESN_REGISTER_START].copy_from_slice(&r[..3]);
    registers[ESN_REGISTER_START..].copy_from_slice(&esn.0.to_be_bytes());
    let mut cave = Cave::new(lfsr, registers, INITIAL_OFFSET, INITIAL_OFFSET);
    cave.run(AUTH_ROUNDS);
    let mut ssd_a = [0u8; 8];
    let mut ssd_b = [0u8; 8];
    ssd_a.copy_from_slice(&cave.registers[..8]);
    ssd_b.copy_from_slice(&cave.registers[8..]);
    Ssd { ssd_a, ssd_b }
}

const DIGIT_CODE_ZERO: u32 = 0b1010;
const DECIMAL_RADIX: u32 = 10;

/// Dialed-digit codes for `AUTH_DATA` (C.S0004-E Table 2.1.1.1.2.2-1). Any other
/// character is replaced by the least significant decimal digit of its code.
fn dialed_digit_code(c: char) -> Result<u32, CaveError> {
    let digit_code = |d: u32| if d == 0 { DIGIT_CODE_ZERO } else { d };
    Ok(match c {
        '0'..='9' => digit_code(c as u32 - '0' as u32),
        '*' => DIGIT_CODE_STAR,
        '#' => DIGIT_CODE_HASH,
        _ if c.is_ascii() => digit_code(c as u32 % DECIMAL_RADIX),
        _ => return Err(CaveError::InvalidDigit(c)),
    })
}

const DIGIT_CODE_BITS: u32 = 4;
const DIGIT_CODE_MASK: u32 = (1 << DIGIT_CODE_BITS) - 1;
const DIGIT_CODE_STAR: u32 = 0b1011;
const DIGIT_CODE_HASH: u32 = 0b1100;

/// `AUTH_DATA` input to Auth_Signature (C.S0004-E §2.1.1.1.2.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AuthData {
    /// Registration, page response, reconnect and other IMSI_S1-keyed messages.
    ImsiS1(u32),
    /// Origination. IMSI_S1 is pre-filled, then the last six dialed digits replace the
    /// low nibbles, the last digit in the least significant nibble.
    Dialed { imsi_s1: u32, digits: String },
}

impl AuthData {
    pub fn to_u24(&self) -> Result<u32, CaveError> {
        match self {
            AuthData::ImsiS1(v) => check_u24(*v),
            AuthData::Dialed { imsi_s1, digits } => {
                let mut value = check_u24(*imsi_s1)?;
                let codes = digits
                    .chars()
                    .map(dialed_digit_code)
                    .collect::<Result<Vec<_>, _>>()?;
                let tail = &codes[codes.len().saturating_sub(DIALED_DIGITS)..];
                for (i, code) in tail.iter().rev().enumerate() {
                    let shift = i as u32 * DIGIT_CODE_BITS;
                    value = (value & !(DIGIT_CODE_MASK << shift)) | (code << shift);
                }
                Ok(value)
            }
        }
    }
}

fn check_u24(v: u32) -> Result<u32, CaveError> {
    if v > AUTH_DATA_MASK {
        return Err(CaveError::FieldTooWide {
            value: u64::from(v),
            bits: AUTH_DATA_BITS,
        });
    }
    Ok(v)
}

/// CAVE state saved by Auth_Signature with SAVE_REGISTERS set (S.S0053-0 §2.4). Only the
/// authentication that set up a call may be used to derive that call's CMEA key and VPM.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SavedRegisters {
    lfsr: [u8; 4],
    offset_1: u8,
    offset_2: u8,
    rand: [u8; 4],
    auth_data: [u8; 3],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AuthSignature {
    /// The 18-bit `AUTH_SIGNATURE`.
    pub signature: u32,
    pub saved: SavedRegisters,
}

/// Auth_Signature (S.S0053-0 §2.4, Exhibit 2-14).
pub fn auth_signature(
    rand_challenge: u32,
    auth_data: u32,
    ssd_auth: &[u8; 8],
    esn: Esn,
) -> Result<AuthSignature, CaveError> {
    let auth_data = check_u24(auth_data)?.to_be_bytes();
    let auth_data = [auth_data[1], auth_data[2], auth_data[3]];
    let rand = rand_challenge.to_be_bytes();
    let mut lfsr = [0u8; 4];
    for i in 0..4 {
        lfsr[i] = rand[i] ^ ssd_auth[i] ^ ssd_auth[i + 4];
    }
    if lfsr == [0; 4] {
        lfsr = rand;
    }
    let mut registers = [0u8; REGISTER_COUNT];
    registers[..8].copy_from_slice(ssd_auth);
    registers[8] = AAV;
    registers[PROCEDURE_DATA_REGISTER_START..ESN_REGISTER_START].copy_from_slice(&auth_data);
    registers[ESN_REGISTER_START..].copy_from_slice(&esn.0.to_be_bytes());
    let mut cave = Cave::new(lfsr, registers, INITIAL_OFFSET, INITIAL_OFFSET);
    cave.run(AUTH_ROUNDS);
    Ok(AuthSignature {
        signature: cave.signature(),
        saved: SavedRegisters {
            lfsr: cave.lfsr,
            offset_1: cave.offset_1,
            offset_2: cave.offset_2,
            rand,
            auth_data,
        },
    })
}

/// Global-challenge `AUTHR` (C.S0004-E §2.1.1.1.2.2). `rand` is the broadcast RAND.
pub fn authr(
    ssd_a: &[u8; 8],
    rand: u32,
    auth_data: &AuthData,
    esn: Esn,
) -> Result<AuthSignature, CaveError> {
    auth_signature(rand, auth_data.to_u24()?, ssd_a, esn)
}

/// `RAND_CHALLENGE` for a unique challenge: RANDU in the 24 MSBs and the 8 LSBs of IMSI_S2
/// in the 8 LSBs (C.S0005-E §2.3.12.1.4).
pub fn unique_challenge_rand(randu: u32, imsi_s2: u16) -> Result<u32, CaveError> {
    if randu > RANDU_MASK {
        return Err(CaveError::FieldTooWide {
            value: u64::from(randu),
            bits: RANDU_BITS,
        });
    }
    Ok((randu << OCTET_BITS) | u32::from(imsi_s2 & UNIQUE_CHALLENGE_IMSI_S2_MASK))
}

/// Unique-challenge `AUTHU` (C.S0005-E §2.3.12.1.4, Table 2.3.12.1-1).
pub fn authu(
    ssd_a: &[u8; 8],
    randu: u32,
    imsi_s1: u32,
    imsi_s2: u16,
    esn: Esn,
) -> Result<AuthSignature, CaveError> {
    auth_signature(unique_challenge_rand(randu, imsi_s2)?, imsi_s1, ssd_a, esn)
}

/// Base-station-challenge `AUTHBS`, keyed by SSD_A_NEW (C.S0005-E §2.3.12.1.5, Figure 2.3.12.1.5-3).
pub fn authbs(ssd_a_new: &[u8; 8], randbs: u32, imsi_s1: u32, esn: Esn) -> Result<u32, CaveError> {
    Ok(auth_signature(randbs, imsi_s1, ssd_a_new, esn)?.signature)
}

/// The per-call CMEA key and Voice Privacy Mask (S.S0053-0 §2.5.1).
#[derive(Clone, PartialEq, Eq)]
pub struct CallKeys {
    pub cmea_key: CmeaKey,
    pub vpm: [u8; VPM_LEN],
}

impl std::fmt::Debug for CallKeys {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CallKeys").finish_non_exhaustive()
    }
}

impl CallKeys {
    /// The 42-bit private long code mask PVTLCM_42 (C.S0005-E §2.3.6.2).
    pub fn private_long_code_mask(&self) -> u64 {
        let tail = &self.vpm[VPM_LEN - PRIVATE_LONG_CODE_MASK_VPM_OCTETS..];
        let low40 = tail.iter().fold(0u64, |acc, &b| (acc << 8) | u64::from(b));
        PRIVATE_LONG_CODE_MASK_PREFIX | low40
    }
}

/// Key_VPM_Generation (S.S0053-0 §2.5.1, Exhibit 2-15). `saved` must come from the
/// authentication of the message that set up the call.
pub fn generate_call_keys(saved: &SavedRegisters, ssd_b: &[u8; 8], esn: Esn) -> CallKeys {
    let mut lfsr = [0u8; 4];
    for i in 0..4 {
        lfsr[i] = saved.lfsr[i] ^ ssd_b[i] ^ ssd_b[i + 4];
    }
    if lfsr == [0; 4] {
        lfsr = saved.rand;
    }
    let mut registers = [0u8; REGISTER_COUNT];
    registers[..8].copy_from_slice(ssd_b);
    registers[8] = AAV;
    registers[PROCEDURE_DATA_REGISTER_START..ESN_REGISTER_START].copy_from_slice(&saved.auth_data);
    registers[ESN_REGISTER_START..].copy_from_slice(&esn.0.to_be_bytes());
    let mut cave = Cave::new(lfsr, registers, saved.offset_1, saved.offset_2);
    cave.run(AUTH_ROUNDS);

    let mut key = [0u8; 8];
    for half in key.chunks_mut(4) {
        cave.roll_lfsr(saved.rand);
        cave.run(KEY_ITERATION_ROUNDS);
        for (i, k) in half.iter_mut().enumerate() {
            *k = cave.registers[i + 4] ^ cave.registers[i + 8];
        }
    }

    let mut vpm = [0u8; VPM_LEN];
    let mut vpm_ptr = 0;
    for iteration in 0..=VPM_FULL_ITERATIONS {
        let octets = if iteration < VPM_FULL_ITERATIONS {
            VPM_OCTETS_PER_ITERATION
        } else {
            VPM_LAST_ITERATION_OCTETS
        };
        cave.roll_lfsr(saved.rand);
        cave.run(KEY_ITERATION_ROUNDS);
        for r in 0..octets {
            vpm[vpm_ptr] = cave.registers[r + 2] ^ cave.registers[r + 8];
            vpm_ptr += 1;
        }
    }

    CallKeys {
        cmea_key: CmeaKey(key),
        vpm,
    }
}

/// The 64-bit CMEA session key, octets k0..k7.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct CmeaKey(pub [u8; 8]);

impl std::fmt::Debug for CmeaKey {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CmeaKey(..)")
    }
}

impl CmeaKey {
    /// tbox (S.S0053-0 §2.6.1, Exhibit 2-24).
    fn tbox(&self, z: u8) -> u8 {
        let mut result = z;
        for pair in self.0.chunks(2) {
            result ^= pair[0];
            result = result.wrapping_add(pair[1]);
            result = z.wrapping_add(CAVE_TABLE[usize::from(result)]);
        }
        result
    }

    /// CMEA (S.S0053-0 §2.6.1, Exhibit 2-25). The transform is its own inverse, so the same
    /// call encrypts and decrypts.
    pub fn apply(&self, msg: &mut [u8]) {
        let n = msg.len();
        let mut z = 0u8;
        for i in 0..n {
            let k = self.tbox(z ^ (i as u8));
            msg[i] = msg[i].wrapping_add(k);
            z = z.wrapping_add(msg[i]);
        }
        for i in 0..n / 2 {
            msg[i] ^= msg[n - 1 - i] | 0x01;
        }
        let mut z = 0u8;
        for i in 0..n {
            let k = self.tbox(z ^ (i as u8));
            z = z.wrapping_add(msg[i]);
            msg[i] = msg[i].wrapping_sub(k);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // S.S0053-0 v2.0 §3.1 fixed inputs.
    const ESN: Esn = Esn(0xD75A_96EC);
    const MIN1: u32 = 0x79_2971;
    const RANDSSD: RandSsd = RandSsd([0x4d, 0x18, 0xee, 0xaa, 0x05, 0x89, 0x5c]);
    const A_KEY_ENTRY: &str = "14 1421 3562 3730 9504 8808 6500";
    const A_KEY: u64 = 0xC442_F56B_E9E1_7158;
    const CHECKSUM: u32 = 0x1_51E4;
    const SSD_A: [u8; 8] = [0xCC, 0x38, 0x12, 0x94, 0x9F, 0x4D, 0xCD, 0x0D];
    const SSD_B: [u8; 8] = [0x31, 0x05, 0x02, 0x34, 0x58, 0x0E, 0x63, 0xB4];
    const PLAINTEXT: [u8; 6] = [0xb6, 0x2d, 0xa2, 0x44, 0xfe, 0x9b];

    struct Vector {
        rand: u32,
        auth_signature: u32,
        cmea_key: [u8; 8],
        cmea_output: [u8; 6],
        vpm: [u8; VPM_LEN],
    }

    const VECTORS: [Vector; 3] = [
        Vector {
            rand: 0x34A2_B05F,
            auth_signature: 0x3_66F6,
            cmea_key: [0xA0, 0x7B, 0x1C, 0xD1, 0x02, 0x75, 0x69, 0x14],
            cmea_output: [0xE5, 0x6B, 0x5F, 0x01, 0x65, 0xC6],
            vpm: [
                0x18, 0x93, 0x94, 0x82, 0x4A, 0x1A, 0x2F, 0x99, 0xA5, 0x39, 0xF9, 0x5B, 0x4D, 0x22,
                0xD5, 0x7C, 0xEE, 0x32, 0xAC, 0x21, 0x6B, 0x26, 0x0D, 0x36, 0xA7, 0xC9, 0x63, 0x88,
                0x57, 0x8C, 0xB9, 0x57, 0xE2, 0xD6, 0xCA, 0x1D, 0x77, 0xB6, 0x1F, 0xD5, 0xC7, 0x1A,
                0x73, 0xA4, 0x17, 0xB2, 0x12, 0x1E, 0x95, 0x34, 0x70, 0xE3, 0x9B, 0xCA, 0x3F, 0xD0,
                0x50, 0xBE, 0x4F, 0xD6, 0x47, 0x80, 0xCC, 0xB8, 0xDF,
            ],
        },
        Vector {
            rand: 0x5375_DF99,
            auth_signature: 0x0_255A,
            cmea_key: [0xF0, 0x06, 0xA8, 0x5A, 0x05, 0xCD, 0xB3, 0x2A],
            cmea_output: [0x2B, 0xAD, 0x16, 0xA9, 0x8F, 0x32],
            vpm: [
                0x20, 0x38, 0x01, 0x6B, 0x89, 0x3C, 0xF8, 0xA0, 0x28, 0x48, 0x98, 0x75, 0xAB, 0x18,
                0x65, 0x5A, 0x49, 0x6E, 0x0B, 0xBB, 0xD2, 0xCB, 0xA8, 0x28, 0x46, 0xE6, 0xD5, 0xB4,
                0x12, 0xB3, 0x8C, 0x9E, 0x76, 0x6C, 0x9E, 0xD4, 0x98, 0xC8, 0xA1, 0x4A, 0xD2, 0xDC,
                0x94, 0xB0, 0xF6, 0xD4, 0x3E, 0xE0, 0xD1, 0x6C, 0x7E, 0x9E, 0xAC, 0x6B, 0xCA, 0x43,
                0x02, 0xC9, 0x23, 0x63, 0x6F, 0x61, 0x68, 0xE8, 0x8F,
            ],
        },
        Vector {
            rand: 0x6C00_0258,
            auth_signature: 0x0_8A8A,
            cmea_key: [0x5A, 0xC8, 0x04, 0x25, 0x32, 0xFB, 0x2D, 0x54],
            cmea_output: [0xA3, 0x06, 0x25, 0xD8, 0x3E, 0x21],
            vpm: [
                0xED, 0xA7, 0xAA, 0x63, 0x27, 0xEA, 0xF8, 0x3D, 0x30, 0x26, 0x8C, 0xC5, 0x18, 0x88,
                0x8F, 0x6D, 0xCD, 0x0D, 0x1D, 0x97, 0x21, 0x06, 0x2D, 0x91, 0x1D, 0xCF, 0x47, 0x1F,
                0xDD, 0xBE, 0xE3, 0xE1, 0x71, 0x18, 0x26, 0x73, 0x7A, 0x5F, 0x09, 0xCC, 0x13, 0x2A,
                0x51, 0x69, 0x27, 0x55, 0x2B, 0x2B, 0x0B, 0x30, 0x5A, 0x09, 0xF6, 0x15, 0x8F, 0xA7,
                0xA9, 0x55, 0x7A, 0x00, 0x23, 0xD8, 0xFD, 0x4C, 0x3E,
            ],
        },
    ];

    #[test]
    fn a_key_entry_verifies_and_converts() {
        let a_key = AKey::from_entry_digits(A_KEY_ENTRY, ESN).unwrap();
        assert_eq!(a_key.to_u64(), A_KEY);
        assert_eq!(a_key.checksum(ESN), CHECKSUM);
        assert_eq!(a_key.entry_digits(ESN), A_KEY_ENTRY.replace(' ', ""));
    }

    #[test]
    fn a_key_entry_rejects_bad_check_digits_and_format() {
        assert_eq!(
            AKey::from_entry_digits("14142135623730950488086501", ESN),
            Err(CaveError::AKeyChecksumMismatch)
        );
        assert!(matches!(
            AKey::from_entry_digits("12345", ESN),
            Err(CaveError::AKeyEntryFormat { .. })
        ));
        assert!(matches!(
            AKey::from_entry_digits("1234567890123456789012345x", ESN),
            Err(CaveError::AKeyEntryFormat { .. })
        ));
    }

    #[test]
    fn short_a_key_entry_is_left_padded() {
        let a_key = AKey::from_u64(42);
        let entry = a_key.entry_digits(ESN);
        let short = entry.trim_start_matches('0');
        assert_eq!(AKey::from_entry_digits(short, ESN).unwrap(), a_key);
    }

    #[test]
    fn ssd_generation_matches_vector() {
        let ssd = generate_ssd(&AKey::from_u64(A_KEY), ESN, &RANDSSD);
        assert_eq!(ssd.ssd_a, SSD_A);
        assert_eq!(ssd.ssd_b, SSD_B);
    }

    #[test]
    fn auth_signature_cmea_key_vpm_and_cmea_match_vectors() {
        for v in &VECTORS {
            let auth = auth_signature(v.rand, MIN1, &SSD_A, ESN).unwrap();
            assert_eq!(auth.signature, v.auth_signature, "rand {:08x}", v.rand);
            let keys = generate_call_keys(&auth.saved, &SSD_B, ESN);
            assert_eq!(keys.cmea_key.0, v.cmea_key, "rand {:08x}", v.rand);
            assert_eq!(keys.vpm, v.vpm, "rand {:08x}", v.rand);
            let mut buf = PLAINTEXT;
            keys.cmea_key.apply(&mut buf);
            assert_eq!(buf, v.cmea_output, "rand {:08x}", v.rand);
            keys.cmea_key.apply(&mut buf);
            assert_eq!(buf, PLAINTEXT, "CMEA must be its own inverse");
        }
    }

    #[test]
    fn authr_with_imsi_s1_equals_auth_signature() {
        let v = &VECTORS[0];
        let auth = authr(&SSD_A, v.rand, &AuthData::ImsiS1(MIN1), ESN).unwrap();
        assert_eq!(auth.signature, v.auth_signature);
    }

    #[test]
    fn private_long_code_mask_uses_40_vpm_lsbs() {
        let auth = auth_signature(VECTORS[0].rand, MIN1, &SSD_A, ESN).unwrap();
        let keys = generate_call_keys(&auth.saved, &SSD_B, ESN);
        assert_eq!(keys.private_long_code_mask(), 0x147_80CC_B8DF);
    }

    #[test]
    fn unique_challenge_rand_layout() {
        assert_eq!(unique_challenge_rand(0xABCDEF, 0x3FF).unwrap(), 0xABCD_EFFF);
        assert!(unique_challenge_rand(0x100_0000, 0).is_err());
    }

    #[test]
    fn dialed_auth_data_replaces_low_nibbles() {
        let full = AuthData::Dialed {
            imsi_s1: 0xFFFFFF,
            digits: "18005551203".into(),
        };
        assert_eq!(full.to_u24().unwrap(), 0x551_2A3);
        let short = AuthData::Dialed {
            imsi_s1: 0x123456,
            digits: "*0#".into(),
        };
        assert_eq!(short.to_u24().unwrap(), 0x123_BAC);
        let none = AuthData::Dialed {
            imsi_s1: 0x123456,
            digits: String::new(),
        };
        assert_eq!(none.to_u24().unwrap(), 0x123456);
        let ascii = AuthData::Dialed {
            imsi_s1: 0,
            digits: "1+".into(),
        };
        assert_eq!(
            ascii.to_u24().unwrap(),
            0x00_0013,
            "'+' is 43, so it codes as 3"
        );
        assert_eq!(
            AuthData::Dialed {
                imsi_s1: 0,
                digits: "1é".into()
            }
            .to_u24(),
            Err(CaveError::InvalidDigit('é'))
        );
    }

    #[test]
    fn auth_data_width_checked() {
        assert!(auth_signature(0, 0x100_0000, &SSD_A, ESN).is_err());
        assert!(AuthData::ImsiS1(0x100_0000).to_u24().is_err());
    }

    #[test]
    fn randssd_u64_round_trip() {
        assert_eq!(RANDSSD.to_u64(), 0x4d18_eeaa_0589_5c);
        assert_eq!(RandSsd::from_u64(0x4d18_eeaa_0589_5c).unwrap(), RANDSSD);
        assert!(RandSsd::from_u64(1 << 56).is_err());
    }
}
