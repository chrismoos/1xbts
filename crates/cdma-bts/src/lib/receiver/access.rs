use cdma_common::{bits::Bitstream, error::Error};

use crate::lac::crc30;

/// Reassembled Access Channel encapsulated PDU.
pub struct AccessFrame {
    /// LAC PDU payload (MSG_LENGTH/CRC stripped).
    pub data: Bitstream,
    /// CRC30 validity over [MSG_LENGTH(8) + payload].
    pub crc_valid: bool,
    /// Encapsulated message length field in octets.
    pub msg_length_octets: usize,
}

/// Access Channel / Traffic Channel reassembly for r-csch PDUs.
///
/// Input is information bits per 20 ms frame (tail bits removed):
/// - Access channel: 88 bits per frame
/// - RC1 traffic (9600 bps): 172 bits per frame
/// Fragments are concatenated until MSG_LENGTH*8 bits are available.
pub struct AccessFrameReader {
    data: Vec<u8>,
    msg_length_octets: usize,
}

impl AccessFrameReader {
    pub fn new() -> Self {
        Self {
            data: Vec::new(),
            msg_length_octets: 0,
        }
    }

    pub fn is_idle(&self) -> bool {
        self.data.is_empty() && self.msg_length_octets == 0
    }

    pub fn reset(&mut self) {
        self.data.clear();
        self.msg_length_octets = 0;
    }

    /// Process one information fragment (88 bits for access, 172 for RC1 traffic).
    pub fn process(&mut self, fragment: &mut Bitstream) -> Result<Option<AccessFrame>, Error> {
        self.data.extend(fragment.bits());

        if self.msg_length_octets == 0 && self.data.len() >= 8 {
            self.msg_length_octets = Bitstream::new_init(&self.data[..8]).read_bits(8)? as usize;
            // C.S0004-E 3.1.1.5.2: for Access Channel, MSG_LENGTH < 6 is invalid.
            if self.msg_length_octets < 6 || self.msg_length_octets > 110 {
                self.reset();
                return Ok(None);
            }
        }

        if self.msg_length_octets == 0 {
            return Ok(None);
        }

        let msg_bits = self.msg_length_octets * 8;
        if self.data.len() < msg_bits {
            return Ok(None);
        }

        let expected_crc = crc30(&Bitstream::new_init(&self.data[0..msg_bits - 30]));
        let observed_crc =
            Bitstream::new_init(&self.data[msg_bits - 30..msg_bits]).read_bits(30)?;
        let crc_valid = observed_crc as u32 == expected_crc;
        let payload = Bitstream::new_init(&self.data[8..msg_bits - 30]);

        let out = AccessFrame {
            data: payload,
            crc_valid,
            msg_length_octets: self.msg_length_octets,
        };
        self.reset();
        Ok(Some(out))
    }
}

/// Reassembled reverse dedicated signaling (r-dsch) regular PDU.
///
/// Input is one RC1/RC2/RC3 traffic-channel information fragment per 20 ms frame:
/// SOM(1) + fragment bits.
/// The encapsulated regular PDU uses:
/// - MSG_LENGTH (8 bits, in octets)
/// - CRC16 over [MSG_LENGTH + LAC PDU]
/// - no CRC30 / EXT_MSG_LENGTH common-channel wrapper
pub struct DedicatedFrameReader {
    data: Vec<u8>,
    msg_length_octets: usize,
}

impl DedicatedFrameReader {
    pub fn new() -> Self {
        Self {
            data: Vec::new(),
            msg_length_octets: 0,
        }
    }

    pub fn is_idle(&self) -> bool {
        self.data.is_empty() && self.msg_length_octets == 0
    }

    pub fn reset(&mut self) {
        self.data.clear();
        self.msg_length_octets = 0;
    }

    fn crc16(data: &[u8]) -> u16 {
        cdma_common::crc::crc16_ccitt(data)
    }

    /// Process one dedicated-channel information fragment:
    /// SOM(1) + encapsulated PDU fragment bits.
    pub fn process(&mut self, fragment: &mut Bitstream) -> Result<Option<AccessFrame>, Error> {
        if fragment.len() == 0 {
            return Ok(None);
        }

        let som = fragment.read_bits(1)? as u8;
        if som == 1 {
            self.reset();
            self.data.extend(fragment.bits());
            if self.data.len() >= 8 {
                self.msg_length_octets =
                    Bitstream::new_init(&self.data[..8]).read_bits(8)? as usize;
                if self.msg_length_octets < 3 {
                    self.reset();
                    return Ok(None);
                }
            }
        } else if self.data.is_empty() {
            return Ok(None);
        } else {
            self.data.extend(fragment.bits());
        }

        if self.msg_length_octets == 0 && self.data.len() >= 8 {
            self.msg_length_octets = Bitstream::new_init(&self.data[..8]).read_bits(8)? as usize;
            if self.msg_length_octets < 3 {
                self.reset();
                return Ok(None);
            }
        }

        if self.msg_length_octets == 0 {
            return Ok(None);
        }

        let msg_bits = self.msg_length_octets * 8;
        if self.data.len() < msg_bits {
            return Ok(None);
        }

        let expected_crc = Self::crc16(&self.data[..msg_bits - 16]);
        let observed_crc =
            Bitstream::new_init(&self.data[msg_bits - 16..msg_bits]).read_bits(16)? as u16;
        let crc_valid = observed_crc == expected_crc;
        let payload = Bitstream::new_init(&self.data[8..msg_bits - 16]);

        let out = AccessFrame {
            data: payload,
            crc_valid,
            msg_length_octets: self.msg_length_octets,
        };
        self.reset();
        Ok(Some(out))
    }
}

#[cfg(test)]
mod tests {
    use cdma_common::bits::Bitstream;

    use super::{AccessFrameReader, DedicatedFrameReader};
    use crate::lac::crc30;

    #[test]
    fn test_access_frame_reader_roundtrip() {
        let payload_bits = vec![
            0, 1, 0, 1, 1, 0, 1, 0, // PD+MSG_TYPE example
            1, 0, 1, 1, 0, 0, 1, 1, 0, 1, 0, 0, 1, 1, 0, 1, 1, 0,
        ];

        let msg_len_octets = ((8 + payload_bits.len() + 30) / 8) as u8;
        assert!(msg_len_octets >= 6);

        let mut crc_scope = Bitstream::new();
        crc_scope.write_u8(msg_len_octets, 8);
        crc_scope.extend(&Bitstream::new_init(&payload_bits));
        let crc = crc30(&crc_scope);

        let mut body = Bitstream::new();
        body.write_u8(msg_len_octets, 8);
        body.extend(&Bitstream::new_init(&payload_bits));
        body.write_u32(crc, 30);
        let body_bits = body.bits().to_vec();
        assert_eq!(msg_len_octets as usize * 8, body_bits.len());

        let mut fragments = Vec::new();
        let mut rem = body_bits.as_slice();
        while !rem.is_empty() {
            let take = rem.len().min(88);
            let mut frag = rem[..take].to_vec();
            if take < 88 {
                frag.extend(std::iter::repeat(0u8).take(88 - take));
            }
            fragments.push(frag);
            rem = &rem[take..];
        }

        let mut reader = AccessFrameReader::new();
        let mut parsed = None;
        for frag in fragments {
            let mut bs = Bitstream::new_init(&frag);
            if let Some(frame) = reader.process(&mut bs).unwrap() {
                parsed = Some(frame);
            }
        }

        let frame = parsed.expect("expected one parsed access frame");
        assert!(frame.crc_valid);
        assert_eq!(msg_len_octets as usize, frame.msg_length_octets);
        assert_eq!(payload_bits, frame.data.bits());
    }

    #[test]
    fn test_dedicated_frame_reader_roundtrip() {
        let mut payload_bits = vec![
            0, 1, 0, 0, 0, 1, 1, 1, // PD + MSG_TYPE example
            1, 0, 1, 1, 0, 0, 1, 1, 0, 1, 0, 0, 1, 1, 0, 1, 1, 0, 1, 1, 0, 1, 0, 0, 1, 0, 1, 1,
        ];
        while payload_bits.len() % 8 != 0 {
            payload_bits.push(0);
        }

        let msg_len_octets = ((8 + payload_bits.len() + 16) / 8) as u8;
        assert!(msg_len_octets >= 3);

        let mut crc_scope = Bitstream::new();
        crc_scope.write_u8(msg_len_octets, 8);
        crc_scope.extend(&Bitstream::new_init(&payload_bits));
        let crc = DedicatedFrameReader::crc16(crc_scope.bits());

        let mut body = Bitstream::new();
        body.write_u8(msg_len_octets, 8);
        body.extend(&Bitstream::new_init(&payload_bits));
        body.write_u32(crc as u32, 16);
        let body_bits = body.bits().to_vec();
        assert_eq!(msg_len_octets as usize * 8, body_bits.len());

        let mut fragments = Vec::new();
        let mut rem = body_bits.as_slice();
        let fragment_bits = 171usize;
        let mut first = true;
        while !rem.is_empty() {
            let take = rem.len().min(fragment_bits);
            let mut frag = vec![if first { 1u8 } else { 0u8 }];
            frag.extend_from_slice(&rem[..take]);
            if take < fragment_bits {
                frag.extend(std::iter::repeat(0u8).take(fragment_bits - take));
            }
            fragments.push(frag);
            rem = &rem[take..];
            first = false;
        }

        let mut reader = DedicatedFrameReader::new();
        let mut parsed = None;
        for frag in fragments {
            let mut bs = Bitstream::new_init(&frag);
            if let Some(frame) = reader.process(&mut bs).unwrap() {
                parsed = Some(frame);
            }
        }

        let frame = parsed.expect("expected one parsed dedicated frame");
        assert!(frame.crc_valid);
        assert_eq!(msg_len_octets as usize, frame.msg_length_octets);
        assert_eq!(payload_bits, frame.data.bits());
    }
}
