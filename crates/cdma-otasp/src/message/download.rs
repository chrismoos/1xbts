//! Download Request — C.S0016-D §4.5.1.2.
//! Download Response — C.S0016-D §3.5.1.2.
//!
//! Phase-1 does not implement Secure Mode, so `FRESH_INCL` is always `0` and
//! the trailing `RESERVED` field is 7 bits.

use cdma_common::bits::Bitstream;

use crate::Error;
use crate::bits::{from_bytes, read_bool, read_u8};
use crate::message::msg_type::DOWNLOAD;
use crate::message::require_msg_type;
use crate::message::result_code::ResultCode;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadParamBlock {
    pub block_id: u8,
    pub param_data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadRequest {
    pub blocks: Vec<DownloadParamBlock>,
}

impl DownloadRequest {
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        if self.blocks.len() > u8::MAX as usize {
            return Err("too many blocks".into());
        }
        let mut bs = Bitstream::new();
        bs.write_u8(DOWNLOAD, 8);
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
        bs.write_u8(0, 1); // FRESH_INCL = 0
        bs.write_u8(0, 7); // RESERVED
        Ok(bs.to_packed_bytes())
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut bs = from_bytes(bytes);
        let msg_type = read_u8(&mut bs, 8)?;
        require_msg_type(msg_type, DOWNLOAD)?;
        let nb = read_u8(&mut bs, 8)? as usize;
        let mut blocks = Vec::with_capacity(nb);
        for _ in 0..nb {
            let block_id = read_u8(&mut bs, 8)?;
            let len = read_u8(&mut bs, 8)? as usize;
            let mut param_data = Vec::with_capacity(len);
            for _ in 0..len {
                param_data.push(read_u8(&mut bs, 8)?);
            }
            blocks.push(DownloadParamBlock {
                block_id,
                param_data,
            });
        }
        let fresh_incl = read_bool(&mut bs)?;
        if fresh_incl {
            return Err("DownloadRequest: FRESH_INCL=1 (Secure Mode) not supported".into());
        }
        Ok(Self { blocks })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DownloadResponse {
    pub results: Vec<(u8, ResultCode)>, // (block_id, result_code)
}

impl DownloadResponse {
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        if self.results.len() > u8::MAX as usize {
            return Err("too many results".into());
        }
        let mut out = Vec::with_capacity(2 + 2 * self.results.len());
        out.push(DOWNLOAD);
        out.push(self.results.len() as u8);
        for (bid, r) in &self.results {
            out.push(*bid);
            out.push(r.to_u8());
        }
        Ok(out)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < 2 {
            return Err("DownloadResponse too short".into());
        }
        require_msg_type(bytes[0], DOWNLOAD)?;
        let n = bytes[1] as usize;
        if bytes.len() < 2 + 2 * n {
            return Err("DownloadResponse truncated".into());
        }
        let mut results = Vec::with_capacity(n);
        for i in 0..n {
            results.push((bytes[2 + 2 * i], ResultCode::from_u8(bytes[2 + 2 * i + 1])));
        }
        Ok(Self { results })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn download_request_round_trip() {
        let r = DownloadRequest {
            blocks: vec![
                DownloadParamBlock {
                    block_id: 0x00,
                    param_data: vec![0x11, 0x22, 0x33],
                },
                DownloadParamBlock {
                    block_id: 0x02,
                    param_data: vec![0xFF],
                },
            ],
        };
        let bytes = r.encode().unwrap();
        let back = DownloadRequest::decode(&bytes).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn download_request_byte_layout_pin() {
        // Single block, BLOCK_ID=0x01, PARAM_DATA=[0xAB,0xCD].
        // Bytes: 0x01 0x01 0x01 0x02 0xAB 0xCD 0x00 (FRESH_INCL=0, RESERVED=7'b0)
        let r = DownloadRequest {
            blocks: vec![DownloadParamBlock {
                block_id: 0x01,
                param_data: vec![0xAB, 0xCD],
            }],
        };
        let bytes = r.encode().unwrap();
        assert_eq!(bytes, vec![0x01, 0x01, 0x01, 0x02, 0xAB, 0xCD, 0x00]);
    }

    #[test]
    fn download_response_round_trip() {
        let r = DownloadResponse {
            results: vec![
                (0x00, ResultCode::Accepted),
                (0x02, ResultCode::RejectedInvalidParameter),
            ],
        };
        let bytes = r.encode().unwrap();
        assert_eq!(bytes, vec![0x01, 0x02, 0x00, 0x00, 0x02, 0x04]);
        assert_eq!(DownloadResponse::decode(&bytes).unwrap(), r);
    }
}
