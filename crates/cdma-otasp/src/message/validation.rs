//! Validation Request — C.S0016-D §4.5.1.10.
//! Validation Response — C.S0016-D §3.5.1.10.
//!
//! Carries Verify SPC / Change SPC parameter blocks. (Validate SPASM is
//! deferred per phase-1 scope.)

use crate::Error;
use crate::message::msg_type::VALIDATION;
use crate::message::require_msg_type;
use crate::message::result_code::ResultCode;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationParamBlock {
    pub block_id: u8,
    pub param_data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationRequest {
    pub blocks: Vec<ValidationParamBlock>,
}

impl ValidationRequest {
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        if self.blocks.len() > u8::MAX as usize {
            return Err("too many blocks".into());
        }
        let mut out = vec![VALIDATION, self.blocks.len() as u8];
        for b in &self.blocks {
            if b.param_data.len() > u8::MAX as usize {
                return Err("PARAM_DATA too long".into());
            }
            out.push(b.block_id);
            out.push(b.param_data.len() as u8);
            out.extend_from_slice(&b.param_data);
        }
        Ok(out)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < 2 {
            return Err("ValidationRequest too short".into());
        }
        require_msg_type(bytes[0], VALIDATION)?;
        let nb = bytes[1] as usize;
        let mut blocks = Vec::with_capacity(nb);
        let mut i = 2;
        for _ in 0..nb {
            if bytes.len() < i + 2 {
                return Err("ValidationRequest block header truncated".into());
            }
            let block_id = bytes[i];
            let len = bytes[i + 1] as usize;
            i += 2;
            if bytes.len() < i + len {
                return Err("ValidationRequest PARAM_DATA truncated".into());
            }
            blocks.push(ValidationParamBlock {
                block_id,
                param_data: bytes[i..i + len].to_vec(),
            });
            i += len;
        }
        Ok(Self { blocks })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ValidationResponse {
    /// One `(BLOCK_ID, RESULT_CODE)` per block in the request, in order.
    pub results: Vec<(u8, ResultCode)>,
}

impl ValidationResponse {
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        if self.results.len() > u8::MAX as usize {
            return Err("too many results".into());
        }
        let mut out = vec![VALIDATION, self.results.len() as u8];
        for (bid, r) in &self.results {
            out.push(*bid);
            out.push(r.to_u8());
        }
        Ok(out)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < 2 {
            return Err("ValidationResponse too short".into());
        }
        require_msg_type(bytes[0], VALIDATION)?;
        let n = bytes[1] as usize;
        if bytes.len() < 2 + 2 * n {
            return Err("ValidationResponse truncated".into());
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
    use crate::param::verify_spc::VerifySpc;

    #[test]
    fn validation_request_with_verify_spc_round_trip() {
        let block = VerifySpc::new("000000");
        let r = ValidationRequest {
            blocks: vec![ValidationParamBlock {
                block_id: 0x00,
                param_data: block.encode().unwrap(),
            }],
        };
        let bytes = r.encode().unwrap();
        // 0x09 0x01 0x00 0x03 0x00 0x00 0x00
        assert_eq!(bytes, vec![0x09, 0x01, 0x00, 0x03, 0x00, 0x00, 0x00]);
        let back = ValidationRequest::decode(&bytes).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn validation_response_round_trip() {
        let r = ValidationResponse {
            results: vec![
                (0x00, ResultCode::Accepted),
                (0x01, ResultCode::RejectedInvalidSpc),
            ],
        };
        let bytes = r.encode().unwrap();
        assert_eq!(bytes, vec![0x09, 0x02, 0x00, 0x00, 0x01, 0x0B]);
        assert_eq!(ValidationResponse::decode(&bytes).unwrap(), r);
    }
}
