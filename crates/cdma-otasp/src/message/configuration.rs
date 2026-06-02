//! Configuration Request — C.S0016-D §4.5.1.1.
//! Configuration Response — C.S0016-D §3.5.1.1.
//!
//! NAM configuration read-back. Phase-1 does not implement Secure Mode, so
//! `FRESH_INCL` is always `0` and the trailing `RESERVED` is 7 bits.

use cdma_common::bits::Bitstream;

use crate::Error;
use crate::bits::{from_bytes, read_bool, read_u8};
use crate::message::msg_type::CONFIGURATION;
use crate::message::require_msg_type;
use crate::message::result_code::ResultCode;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigurationRequest {
    pub block_ids: Vec<u8>,
}

impl ConfigurationRequest {
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        if self.block_ids.len() > u8::MAX as usize {
            return Err("too many block ids".into());
        }
        let mut out = Vec::with_capacity(2 + self.block_ids.len());
        out.push(CONFIGURATION);
        out.push(self.block_ids.len() as u8);
        out.extend_from_slice(&self.block_ids);
        Ok(out)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < 2 {
            return Err("ConfigurationRequest too short".into());
        }
        require_msg_type(bytes[0], CONFIGURATION)?;
        let n = bytes[1] as usize;
        if bytes.len() < 2 + n {
            return Err("ConfigurationRequest block_ids truncated".into());
        }
        Ok(Self {
            block_ids: bytes[2..2 + n].to_vec(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigurationParamBlock {
    pub block_id: u8,
    pub param_data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfigurationResponse {
    pub blocks: Vec<ConfigurationParamBlock>,
    /// Per-block result codes (one per block, in order).
    pub results: Vec<ResultCode>,
}

impl ConfigurationResponse {
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        if self.blocks.len() != self.results.len() {
            return Err("blocks and results length mismatch".into());
        }
        if self.blocks.len() > u8::MAX as usize {
            return Err("too many blocks".into());
        }
        let mut bs = Bitstream::new();
        bs.write_u8(CONFIGURATION, 8);
        bs.write_u8(self.blocks.len() as u8, 8);
        for b in &self.blocks {
            if b.param_data.len() > u8::MAX as usize {
                return Err("PARAM_DATA too long".into());
            }
            bs.write_u8(b.block_id, 8);
            bs.write_u8(b.param_data.len() as u8, 8);
            for &octet in &b.param_data {
                bs.write_u8(octet, 8);
            }
        }
        for r in &self.results {
            bs.write_u8(r.to_u8(), 8);
        }
        // FRESH_INCL = 0, RESERVED = 7 bits 0 → completes the final octet.
        bs.write_u8(0, 1);
        bs.write_u8(0, 7);
        Ok(bs.to_packed_bytes())
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut bs = from_bytes(bytes);
        let msg_type = read_u8(&mut bs, 8)?;
        require_msg_type(msg_type, CONFIGURATION)?;
        let nb = read_u8(&mut bs, 8)? as usize;
        let mut blocks = Vec::with_capacity(nb);
        for _ in 0..nb {
            let block_id = read_u8(&mut bs, 8)?;
            let len = read_u8(&mut bs, 8)? as usize;
            let mut param_data = Vec::with_capacity(len);
            for _ in 0..len {
                param_data.push(read_u8(&mut bs, 8)?);
            }
            blocks.push(ConfigurationParamBlock {
                block_id,
                param_data,
            });
        }
        let mut results = Vec::with_capacity(nb);
        for _ in 0..nb {
            results.push(ResultCode::from_u8(read_u8(&mut bs, 8)?));
        }
        // Per spec §3.5.1.1 the trailing FRESH_INCL bit + 7 reserved bits
        // is mandatory, but vintage MSes that never implemented Secure Mode
        // routinely omit it. Treat as optional: if absent, assume
        // FRESH_INCL=0 (no Secure Mode).
        if let Ok(fresh_incl) = read_bool(&mut bs) {
            if fresh_incl {
                return Err(
                    "ConfigurationResponse: FRESH_INCL=1 (Secure Mode) not supported".into(),
                );
            }
        }
        Ok(Self { blocks, results })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn configuration_request_round_trip() {
        let r = ConfigurationRequest {
            block_ids: vec![0x00, 0x01, 0x02],
        };
        let bytes = r.encode().unwrap();
        assert_eq!(bytes, vec![0x00, 0x03, 0x00, 0x01, 0x02]);
        assert_eq!(ConfigurationRequest::decode(&bytes).unwrap(), r);
    }

    #[test]
    fn configuration_response_round_trip() {
        let r = ConfigurationResponse {
            blocks: vec![
                ConfigurationParamBlock {
                    block_id: 0x01,
                    param_data: vec![0xAA, 0xBB, 0xCC],
                },
                ConfigurationParamBlock {
                    block_id: 0x02,
                    param_data: vec![0xDE, 0xAD],
                },
            ],
            results: vec![
                ResultCode::Accepted,
                ResultCode::RejectedBlockIdNotSupported,
            ],
        };
        let bytes = r.encode().unwrap();
        let back = ConfigurationResponse::decode(&bytes).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn configuration_response_byte_layout_pin() {
        // One block, BLOCK_ID=0x00, PARAM_DATA = [0x55], RESULT_CODE=0x00,
        // then FRESH_INCL=0, RESERVED=7'b0.
        // Expected: 0x00 0x01 0x00 0x01 0x55 0x00 0x00
        let r = ConfigurationResponse {
            blocks: vec![ConfigurationParamBlock {
                block_id: 0x00,
                param_data: vec![0x55],
            }],
            results: vec![ResultCode::Accepted],
        };
        let bytes = r.encode().unwrap();
        assert_eq!(bytes, vec![0x00, 0x01, 0x00, 0x01, 0x55, 0x00, 0x00]);
    }
}
