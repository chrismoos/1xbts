//! OTAPA Start/Stop and response (C.S0016-D §4.5.1.11 and §3.5.1.11).

use super::{msg_type::OTAPA, require_msg_type, result_code::ResultCode};
use crate::Error;

const START: u8 = 0x80;
const NAM_LOCK: u8 = 0x01;
const TYPE_OCTET: usize = 0;
const RESULT_OCTET: usize = 1;
const LOCK_OCTET: usize = 2;
const RESPONSE_OCTETS: usize = 3;
const LOCKED_RESPONSE_OCTETS: usize = RESPONSE_OCTETS + size_of::<u32>();

pub fn start_request() -> Vec<u8> {
    vec![OTAPA, START]
}

#[derive(Debug, PartialEq, Eq)]
pub struct OtapaResponse {
    pub result: ResultCode,
    pub rand_otapa: Option<u32>,
}

impl OtapaResponse {
    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < RESPONSE_OCTETS {
            return Err("OTAPA response is truncated".into());
        }
        require_msg_type(bytes[TYPE_OCTET], OTAPA)?;
        let locked = bytes[LOCK_OCTET] & NAM_LOCK != 0;
        let expected = if locked {
            LOCKED_RESPONSE_OCTETS
        } else {
            RESPONSE_OCTETS
        };
        if bytes.len() != expected {
            return Err("OTAPA response length does not match NAM_LOCK_IND".into());
        }
        let rand_otapa = if locked {
            Some(u32::from_be_bytes(bytes[RESPONSE_OCTETS..].try_into()?))
        } else {
            None
        };
        Ok(Self {
            result: ResultCode::from_u8(bytes[RESULT_OCTET]),
            rand_otapa,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn start_and_responses_match_wire_format() {
        assert_eq!(start_request(), [0x0a, 0x80]);
        assert_eq!(
            OtapaResponse::decode(&[0x0a, 0, 0]).unwrap().rand_otapa,
            None
        );
        assert_eq!(
            OtapaResponse::decode(&[0x0a, 0, 1, 0x12, 0x34, 0x56, 0x78])
                .unwrap()
                .rand_otapa,
            Some(0x12345678)
        );
        assert!(OtapaResponse::decode(&[0x0a, 0, 1]).is_err());
        assert!(OtapaResponse::decode(&[0x0a, 0, 0, 0]).is_err());
    }
}
