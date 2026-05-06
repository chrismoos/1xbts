use std::{fmt::Display, ops::Range};

use crate::error::Error;

#[derive(Debug, Clone, Default)]
pub struct Bitstream {
    bits: Vec<u8>,
}

impl Bitstream {
    pub fn new() -> Bitstream {
        Bitstream { bits: vec![] }
    }

    pub fn new_init(bits: &[u8]) -> Bitstream {
        Bitstream {
            bits: bits.to_vec(),
        }
    }

    pub fn new_bytes(bytes: &[u8]) -> Bitstream {
        Bitstream {
            bits: bytes
                .iter()
                .flat_map(|b| {
                    vec![
                        (b >> 7) & 1,
                        (b >> 6) & 1,
                        (b >> 5) & 1,
                        (b >> 4) & 1,
                        (b >> 3) & 1,
                        (b >> 2) & 1,
                        (b >> 1) & 1,
                        b & 1,
                    ]
                })
                .collect(),
        }
    }

    pub fn write_u64(&mut self, val: u64, bits: usize) {
        if bits == 0 {
            return;
        }
        if bits > 64 {
            self.bits
                .extend(std::iter::repeat_n(0u8, bits.saturating_sub(64)));
        }
        let tail_bits = bits.min(64);
        for i in 0..tail_bits {
            self.bits.push(((val >> (tail_bits - i - 1)) as u8) & 1);
        }
    }

    pub fn write_u8(&mut self, val: u8, bits: usize) {
        self.write_u64(val as u64, bits);
    }

    pub fn write_u32(&mut self, val: u32, bits: usize) {
        self.write_u64(val as u64, bits);
    }

    pub fn read_bits(&mut self, bits: usize) -> Result<u64, Error> {
        assert!(bits > 0 && bits <= 64);
        if bits > self.bits.len() {
            return Err("EOF".into());
        }
        let mut result = 0u64;
        for _ in 0..bits {
            result <<= 1;

            let bit = self.take_next().ok_or("EOF")?;
            result |= (bit & 1) as u64;
        }
        Ok(result)
    }

    pub fn len(&self) -> usize {
        self.bits.len()
    }

    pub fn is_empty(&self) -> bool {
        self.bits.is_empty()
    }

    pub fn extend(&mut self, bs: &Bitstream) {
        self.bits.extend(&bs.bits);
    }

    pub fn extend_n(&mut self, bs: &Bitstream, n: usize) {
        self.bits.extend(&bs.bits[0..n]);
    }

    pub fn drain(&mut self, range: Range<usize>) -> Bitstream {
        Bitstream {
            bits: self.bits.drain(range).collect(),
        }
    }

    pub fn take_next(&mut self) -> Option<u8> {
        if self.bits.is_empty() {
            None
        } else {
            // todo - vecdeque
            Some(self.bits.remove(0))
        }
    }

    pub fn bits(&self) -> &[u8] {
        &self.bits
    }

    pub fn to_packed_bytes(&self) -> Vec<u8> {
        self.bits
            .chunks(8)
            .map(|chunk| {
                let val = chunk.iter().fold(0u8, |acc, &b| (acc << 1) | (b & 1));
                if chunk.len() < 8 {
                    val << (8 - chunk.len())
                } else {
                    val
                }
            })
            .collect()
    }
}

impl Display for Bitstream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(
            &self
                .bits
                .iter()
                .map(|b| if *b == 0 { "0" } else { "1" })
                .collect::<String>(),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::Bitstream;

    #[test]
    pub fn test_write() {
        let mut bs = Bitstream::new();
        bs.write_u64(0b1111110101, 6);
        assert_eq!(&[1, 1, 0, 1, 0, 1], &bs.bits[..]);
    }

    #[test]
    pub fn test_extend() {
        let mut bs = Bitstream::new();
        bs.write_u64(0b1111110101, 6);

        let mut bs2 = Bitstream::new();
        bs2.write_u64(0b1111110101, 6);

        bs.extend(&bs2);
        assert_eq!(&[1, 1, 0, 1, 0, 1, 1, 1, 0, 1, 0, 1], &bs.bits[..]);
    }

    #[test]
    pub fn test_to_packed_bytes_pads_short_final_byte_on_right() {
        let mut bs = Bitstream::new();
        bs.write_u64(0b001100, 6);
        assert_eq!(vec![0b00110000], bs.to_packed_bytes());
    }
}
