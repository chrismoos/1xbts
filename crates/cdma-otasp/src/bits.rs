//! Bit-packing helpers built on `cdma_common::bits::Bitstream`.
//!
//! OTASP wire fields are MSB-first within each field. The shared `Bitstream`
//! type already encodes that way; this module adds two thin wrappers a codec
//! can use without dragging in the unpacked-bit `Vec<u8>` form for byte
//! payloads.

use cdma_common::bits::Bitstream;

use crate::Error;

/// Read a `bits`-wide MSB-first field as `u64`.
pub fn read_u64(bs: &mut Bitstream, bits: usize) -> Result<u64, Error> {
    bs.read_bits(bits)
}

/// Read a `bits`-wide MSB-first field as `u32`, erroring if it exceeds 32.
pub fn read_u32(bs: &mut Bitstream, bits: usize) -> Result<u32, Error> {
    if bits > 32 {
        return Err("read_u32: bits > 32".into());
    }
    Ok(read_u64(bs, bits)? as u32)
}

/// Read a `bits`-wide MSB-first field as `u16`, erroring if it exceeds 16.
pub fn read_u16(bs: &mut Bitstream, bits: usize) -> Result<u16, Error> {
    if bits > 16 {
        return Err("read_u16: bits > 16".into());
    }
    Ok(read_u64(bs, bits)? as u16)
}

/// Read a `bits`-wide MSB-first field as `u8`, erroring if it exceeds 8.
pub fn read_u8(bs: &mut Bitstream, bits: usize) -> Result<u8, Error> {
    if bits > 8 {
        return Err("read_u8: bits > 8".into());
    }
    Ok(read_u64(bs, bits)? as u8)
}

/// Read a single bit as `bool`.
pub fn read_bool(bs: &mut Bitstream) -> Result<bool, Error> {
    Ok(read_u8(bs, 1)? != 0)
}

/// Pack a `Bitstream` to bytes, zero-padding the final byte.
pub fn to_padded_bytes(bs: &Bitstream) -> Vec<u8> {
    bs.to_packed_bytes()
}

/// Build a `Bitstream` from a packed byte slice.
pub fn from_bytes(bytes: &[u8]) -> Bitstream {
    Bitstream::new_bytes(bytes)
}
