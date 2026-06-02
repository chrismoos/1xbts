//! Commit Request — C.S0016-D §4.5.1.6. Commit Response — §3.5.1.6.
//!
//! Both messages share `OTASP_MSG_TYPE = 0x05`. Request is a single byte;
//! Response is the type byte plus a one-byte `RESULT_CODE`.

use crate::Error;
use crate::message::msg_type::COMMIT_REQ_AND_RESP;
use crate::message::require_msg_type;
use crate::message::result_code::ResultCode;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitRequest;

impl CommitRequest {
    pub fn encode(&self) -> Vec<u8> {
        vec![COMMIT_REQ_AND_RESP]
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() != 1 {
            return Err(format!("CommitRequest must be 1 byte, got {}", bytes.len()).into());
        }
        require_msg_type(bytes[0], COMMIT_REQ_AND_RESP)?;
        Ok(Self)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CommitResponse {
    pub result: ResultCode,
}

impl CommitResponse {
    pub fn encode(&self) -> Vec<u8> {
        vec![COMMIT_REQ_AND_RESP, self.result.to_u8()]
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() != 2 {
            return Err(format!("CommitResponse must be 2 bytes, got {}", bytes.len()).into());
        }
        require_msg_type(bytes[0], COMMIT_REQ_AND_RESP)?;
        Ok(Self {
            result: ResultCode::from_u8(bytes[1]),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn commit_request_is_one_byte() {
        assert_eq!(CommitRequest.encode(), vec![0x05]);
        assert_eq!(CommitRequest::decode(&[0x05]).unwrap(), CommitRequest);
    }

    #[test]
    fn commit_response_round_trip() {
        let r = CommitResponse {
            result: ResultCode::Accepted,
        };
        let bytes = r.encode();
        assert_eq!(bytes, vec![0x05, 0x00]);
        assert_eq!(CommitResponse::decode(&bytes).unwrap(), r);
    }

    #[test]
    fn commit_response_rejected_passes_through() {
        let r = CommitResponse {
            result: ResultCode::RejectedMobileStationLocked,
        };
        let bytes = r.encode();
        assert_eq!(bytes, vec![0x05, 0x0A]);
        assert_eq!(CommitResponse::decode(&bytes).unwrap(), r);
    }
}
