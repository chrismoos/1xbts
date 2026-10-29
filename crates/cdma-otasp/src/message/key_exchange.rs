//! A-key exchange and re-authentication — C.S0016-D §4.5.1.3-§4.5.1.5 (base
//! station) and §3.5.1.3-§3.5.1.5 (mobile station).

use crate::Error;
use crate::bits::{from_bytes, read_u8, read_u32, to_padded_bytes};
use crate::message::msg_type::{KEY_GENERATION, MS_KEY, RE_AUTHENTICATE};
use crate::message::require_msg_type;
use crate::message::result_code::ResultCode;

/// A_KEY_P_REV `00000010`: 2G A-key generation with a base-station-chosen P and G.
pub const A_KEY_P_REV_2G: u8 = 0b0000_0010;
/// A_KEY_P_REV `00000011`: 2G A-key and 3G root key over the fixed 1024-bit group.
pub const A_KEY_P_REV_2G_3G: u8 = 0b0000_0011;

const AUTHR_BITS: u32 = 18;
const RANDC_BITS: u32 = 8;
const COUNT_BITS: u32 = 6;
const AUTH_DATA_BITS: u32 = 24;
const RE_AUTHENTICATE_RESPONSE_LEN: usize = 8;

fn length_prefixed(out: &mut Vec<u8>, field: &[u8], name: &str) -> Result<(), Error> {
    let len = u8::try_from(field.len()).map_err(|_| format!("{name} longer than 255 octets"))?;
    out.push(len);
    out.extend_from_slice(field);
    Ok(())
}

fn take_length_prefixed<'a>(bytes: &'a [u8], name: &str) -> Result<(&'a [u8], &'a [u8]), Error> {
    let (&len, rest) = bytes
        .split_first()
        .ok_or_else(|| format!("{name} length missing"))?;
    let len = usize::from(len);
    if rest.len() < len {
        return Err(format!("{name} truncated: need {len} octets, have {}", rest.len()).into());
    }
    Ok(rest.split_at(len))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MsKeyRequest {
    pub a_key_p_rev: u8,
    /// PARAM_P and PARAM_G, present only for A_KEY_P_REV `00000010`.
    pub params: Option<(Vec<u8>, Vec<u8>)>,
}

impl MsKeyRequest {
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let mut out = vec![MS_KEY, self.a_key_p_rev];
        if let Some((p, g)) = &self.params {
            length_prefixed(&mut out, p, "PARAM_P")?;
            length_prefixed(&mut out, g, "PARAM_G")?;
        }
        Ok(out)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let [msg_type, a_key_p_rev, rest @ ..] = bytes else {
            return Err("MS Key Request shorter than 2 bytes".into());
        };
        require_msg_type(*msg_type, MS_KEY)?;
        let params = if *a_key_p_rev == A_KEY_P_REV_2G {
            let (p, rest) = take_length_prefixed(rest, "PARAM_P")?;
            let (g, _) = take_length_prefixed(rest, "PARAM_G")?;
            Some((p.to_vec(), g.to_vec()))
        } else {
            None
        };
        Ok(Self {
            a_key_p_rev: *a_key_p_rev,
            params,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MsKeyResponse {
    pub result: ResultCode,
}

impl MsKeyResponse {
    pub fn encode(&self) -> Vec<u8> {
        vec![MS_KEY, self.result.to_u8()]
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let [msg_type, result] = bytes else {
            return Err(format!("MS Key Response must be 2 bytes, got {}", bytes.len()).into());
        };
        require_msg_type(*msg_type, MS_KEY)?;
        Ok(Self {
            result: ResultCode::from_u8(*result),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyGenerationRequest {
    pub bs_result: Vec<u8>,
}

impl KeyGenerationRequest {
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let mut out = vec![KEY_GENERATION];
        length_prefixed(&mut out, &self.bs_result, "BS_RESULT")?;
        Ok(out)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let (&msg_type, rest) = bytes
            .split_first()
            .ok_or("Key Generation Request is empty")?;
        require_msg_type(msg_type, KEY_GENERATION)?;
        let (bs_result, _) = take_length_prefixed(rest, "BS_RESULT")?;
        Ok(Self {
            bs_result: bs_result.to_vec(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KeyGenerationResponse {
    pub result: ResultCode,
    pub ms_result: Vec<u8>,
}

impl KeyGenerationResponse {
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let mut out = vec![KEY_GENERATION, self.result.to_u8()];
        length_prefixed(&mut out, &self.ms_result, "MS_RESULT")?;
        Ok(out)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let [msg_type, result, rest @ ..] = bytes else {
            return Err("Key Generation Response shorter than 2 bytes".into());
        };
        require_msg_type(*msg_type, KEY_GENERATION)?;
        let (ms_result, _) = take_length_prefixed(rest, "MS_RESULT")?;
        Ok(Self {
            result: ResultCode::from_u8(*result),
            ms_result: ms_result.to_vec(),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReAuthenticateRequest {
    pub rand: u32,
}

impl ReAuthenticateRequest {
    pub fn encode(&self) -> Vec<u8> {
        let mut out = vec![RE_AUTHENTICATE];
        out.extend_from_slice(&self.rand.to_be_bytes());
        out
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let [msg_type, a, b, c, d] = bytes else {
            return Err(format!(
                "Re-Authenticate Request must be 5 bytes, got {}",
                bytes.len()
            )
            .into());
        };
        require_msg_type(*msg_type, RE_AUTHENTICATE)?;
        Ok(Self {
            rand: u32::from_be_bytes([*a, *b, *c, *d]),
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ReAuthenticateResponse {
    pub authr: u32,
    pub randc: u8,
    pub count: u8,
    pub auth_data: u32,
}

impl ReAuthenticateResponse {
    pub fn encode(&self) -> Vec<u8> {
        let mut bs = cdma_common::bits::Bitstream::new();
        bs.write_u8(RE_AUTHENTICATE, 8);
        bs.write_u32(self.authr, AUTHR_BITS as usize);
        bs.write_u8(self.randc, RANDC_BITS as usize);
        bs.write_u8(self.count, COUNT_BITS as usize);
        bs.write_u32(self.auth_data, AUTH_DATA_BITS as usize);
        to_padded_bytes(&bs)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() != RE_AUTHENTICATE_RESPONSE_LEN {
            return Err(format!(
                "Re-Authenticate Response must be {RE_AUTHENTICATE_RESPONSE_LEN} bytes, got {}",
                bytes.len()
            )
            .into());
        }
        require_msg_type(bytes[0], RE_AUTHENTICATE)?;
        let mut bs = from_bytes(&bytes[1..]);
        Ok(Self {
            authr: read_u32(&mut bs, AUTHR_BITS as usize)?,
            randc: read_u8(&mut bs, RANDC_BITS as usize)?,
            count: read_u8(&mut bs, COUNT_BITS as usize)?,
            auth_data: read_u32(&mut bs, AUTH_DATA_BITS as usize)?,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ms_key_request_2g_carries_p_and_g() {
        let request = MsKeyRequest {
            a_key_p_rev: A_KEY_P_REV_2G,
            params: Some((vec![0xAB; 64], vec![0x05; 20])),
        };
        let bytes = request.encode().unwrap();
        assert_eq!(&bytes[..3], &[0x02, 0x02, 64]);
        assert_eq!(bytes[3 + 64], 20);
        assert_eq!(bytes.len(), 2 + 1 + 64 + 1 + 20);
        assert_eq!(MsKeyRequest::decode(&bytes).unwrap(), request);
    }

    #[test]
    fn ms_key_request_fixed_group_omits_p_and_g() {
        let request = MsKeyRequest {
            a_key_p_rev: A_KEY_P_REV_2G_3G,
            params: None,
        };
        assert_eq!(request.encode().unwrap(), vec![0x02, 0x03]);
        assert_eq!(MsKeyRequest::decode(&[0x02, 0x03]).unwrap(), request);
    }

    #[test]
    fn key_generation_round_trips() {
        let request = KeyGenerationRequest {
            bs_result: vec![1, 2, 3],
        };
        let bytes = request.encode().unwrap();
        assert_eq!(bytes, vec![0x03, 3, 1, 2, 3]);
        assert_eq!(KeyGenerationRequest::decode(&bytes).unwrap(), request);

        let response = KeyGenerationResponse {
            result: ResultCode::Accepted,
            ms_result: vec![9; 64],
        };
        let bytes = response.encode().unwrap();
        assert_eq!(&bytes[..3], &[0x03, 0x00, 64]);
        assert_eq!(KeyGenerationResponse::decode(&bytes).unwrap(), response);
        assert!(KeyGenerationResponse::decode(&bytes[..10]).is_err());
    }

    #[test]
    fn re_authenticate_round_trips() {
        let request = ReAuthenticateRequest { rand: 0x34A2_B05F };
        assert_eq!(request.encode(), vec![0x04, 0x34, 0xA2, 0xB0, 0x5F]);
        assert_eq!(
            ReAuthenticateRequest::decode(&request.encode()).unwrap(),
            request
        );

        let response = ReAuthenticateResponse {
            authr: 0x3_66F6,
            randc: 0x34,
            count: 0x2A,
            auth_data: 0x79_2971,
        };
        let bytes = response.encode();
        assert_eq!(bytes.len(), RE_AUTHENTICATE_RESPONSE_LEN);
        assert_eq!(ReAuthenticateResponse::decode(&bytes).unwrap(), response);
    }

    #[test]
    fn ms_key_response_round_trips() {
        let response = MsKeyResponse {
            result: ResultCode::Accepted,
        };
        assert_eq!(response.encode(), vec![0x02, 0x00]);
        assert_eq!(MsKeyResponse::decode(&[0x02, 0x00]).unwrap(), response);
    }
}
