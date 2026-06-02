//! System Tag Configuration Request — C.S0016-D §4.5.1.20.
//! System Tag Configuration Response — §3.5.1.20.
//! System Tag Download Request — §4.5.1.21.
//! System Tag Download Response — §3.5.1.21.
//!
//! Configuration request carries only `OTASP_MSG_TYPE`, `BLOCK_ID`, and
//! optionally a segment offset/size pair (only for BLOCK_ID=0x01 System
//! Tag List). The Home System Tag (BLOCK_ID=0x00) Configuration Request
//! is just two bytes — the segment fields are always omitted.
//!
//! The Download Request layout is fixed-form (no `FRESH_INCL` field per
//! the spec — Secure Mode is out of scope and not gated on this message).

use crate::Error;
use crate::message::msg_type::{SYSTEM_TAG_CONFIGURATION, SYSTEM_TAG_DOWNLOAD};
use crate::message::require_msg_type;
use crate::message::result_code::ResultCode;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemTagConfigRequest {
    pub block_id: u8,
    /// Required when `block_id == 0x01` (System Tag List); omitted otherwise.
    pub segment: Option<SegmentRange>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentRange {
    pub offset: u16,
    pub max_size: u8,
}

impl SystemTagConfigRequest {
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let mut out = vec![SYSTEM_TAG_CONFIGURATION, self.block_id];
        if self.block_id == 0x01 {
            let s = self
                .segment
                .ok_or("System Tag Config Request: BLOCK_ID=0x01 requires segment range")?;
            out.extend_from_slice(&s.offset.to_be_bytes());
            out.push(s.max_size);
        }
        Ok(out)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < 2 {
            return Err("SystemTagConfigRequest too short".into());
        }
        require_msg_type(bytes[0], SYSTEM_TAG_CONFIGURATION)?;
        let block_id = bytes[1];
        let segment = if block_id == 0x01 {
            if bytes.len() < 5 {
                return Err("SystemTagConfigRequest segment fields truncated".into());
            }
            Some(SegmentRange {
                offset: u16::from_be_bytes([bytes[2], bytes[3]]),
                max_size: bytes[4],
            })
        } else {
            None
        };
        Ok(Self { block_id, segment })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemTagConfigResponse {
    pub block_id: u8,
    pub result: ResultCode,
    pub param_data: Vec<u8>,
}

impl SystemTagConfigResponse {
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        if self.param_data.len() > u8::MAX as usize {
            return Err("PARAM_DATA too long".into());
        }
        let mut out = Vec::with_capacity(4 + self.param_data.len());
        out.push(SYSTEM_TAG_CONFIGURATION);
        out.push(self.block_id);
        out.push(self.result.to_u8());
        out.push(self.param_data.len() as u8);
        out.extend_from_slice(&self.param_data);
        Ok(out)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < 4 {
            return Err("SystemTagConfigResponse too short".into());
        }
        require_msg_type(bytes[0], SYSTEM_TAG_CONFIGURATION)?;
        let block_id = bytes[1];
        let result = ResultCode::from_u8(bytes[2]);
        let len = bytes[3] as usize;
        if bytes.len() < 4 + len {
            return Err("SystemTagConfigResponse PARAM_DATA truncated".into());
        }
        Ok(Self {
            block_id,
            result,
            param_data: bytes[4..4 + len].to_vec(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemTagDownloadRequest {
    pub block_id: u8,
    pub param_data: Vec<u8>,
}

impl SystemTagDownloadRequest {
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        if self.param_data.len() > u8::MAX as usize {
            return Err("PARAM_DATA too long".into());
        }
        let mut out = Vec::with_capacity(3 + self.param_data.len());
        out.push(SYSTEM_TAG_DOWNLOAD);
        out.push(self.block_id);
        out.push(self.param_data.len() as u8);
        out.extend_from_slice(&self.param_data);
        Ok(out)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < 3 {
            return Err("SystemTagDownloadRequest too short".into());
        }
        require_msg_type(bytes[0], SYSTEM_TAG_DOWNLOAD)?;
        let block_id = bytes[1];
        let len = bytes[2] as usize;
        if bytes.len() < 3 + len {
            return Err("SystemTagDownloadRequest PARAM_DATA truncated".into());
        }
        Ok(Self {
            block_id,
            param_data: bytes[3..3 + len].to_vec(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SystemTagDownloadResponse {
    pub block_id: u8,
    pub result: ResultCode,
    /// Required when `block_id` is 0x01 / 0x02 / 0x03 (segmented list types).
    pub segment_progress: Option<SegmentProgress>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SegmentProgress {
    pub offset: u16,
    pub size: u8,
}

impl SystemTagDownloadResponse {
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let mut out = vec![SYSTEM_TAG_DOWNLOAD, self.block_id, self.result.to_u8()];
        let needs_segment = matches!(self.block_id, 0x01 | 0x02 | 0x03);
        match (needs_segment, self.segment_progress) {
            (true, Some(s)) => {
                out.extend_from_slice(&s.offset.to_be_bytes());
                out.push(s.size);
            }
            (true, None) => {
                return Err(
                    "System Tag Download Response: list block needs segment progress".into(),
                );
            }
            (false, Some(_)) => {
                return Err(
                    "System Tag Download Response: segment progress only valid for list blocks"
                        .into(),
                );
            }
            (false, None) => {}
        }
        Ok(out)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < 3 {
            return Err("SystemTagDownloadResponse too short".into());
        }
        require_msg_type(bytes[0], SYSTEM_TAG_DOWNLOAD)?;
        let block_id = bytes[1];
        let result = ResultCode::from_u8(bytes[2]);
        let needs_segment = matches!(block_id, 0x01 | 0x02 | 0x03);
        let segment_progress = if needs_segment {
            if bytes.len() < 6 {
                return Err("SystemTagDownloadResponse segment fields truncated".into());
            }
            Some(SegmentProgress {
                offset: u16::from_be_bytes([bytes[3], bytes[4]]),
                size: bytes[5],
            })
        } else {
            None
        };
        Ok(Self {
            block_id,
            result,
            segment_progress,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::param::home_system_tag::HomeSystemTag;

    #[test]
    fn system_tag_config_request_home_tag_is_two_bytes() {
        let r = SystemTagConfigRequest {
            block_id: 0x00,
            segment: None,
        };
        assert_eq!(r.encode().unwrap(), vec![0x13, 0x00]);
        assert_eq!(SystemTagConfigRequest::decode(&[0x13, 0x00]).unwrap(), r);
    }

    #[test]
    fn system_tag_config_request_list_block_carries_segment() {
        let r = SystemTagConfigRequest {
            block_id: 0x01,
            segment: Some(SegmentRange {
                offset: 0x1234,
                max_size: 0x80,
            }),
        };
        let bytes = r.encode().unwrap();
        assert_eq!(bytes, vec![0x13, 0x01, 0x12, 0x34, 0x80]);
        assert_eq!(SystemTagConfigRequest::decode(&bytes).unwrap(), r);
    }

    #[test]
    fn system_tag_download_request_round_trip() {
        let tag = HomeSystemTag::new_ascii("1xBTS").unwrap();
        let param = tag.encode().unwrap();
        let r = SystemTagDownloadRequest {
            block_id: 0x00,
            param_data: param.clone(),
        };
        let bytes = r.encode().unwrap();
        let mut expected = vec![0x14, 0x00, param.len() as u8];
        expected.extend_from_slice(&param);
        assert_eq!(bytes, expected);
        assert_eq!(SystemTagDownloadRequest::decode(&bytes).unwrap(), r);
    }

    #[test]
    fn system_tag_download_response_home_tag() {
        let r = SystemTagDownloadResponse {
            block_id: 0x00,
            result: ResultCode::Accepted,
            segment_progress: None,
        };
        let bytes = r.encode().unwrap();
        assert_eq!(bytes, vec![0x14, 0x00, 0x00]);
        assert_eq!(SystemTagDownloadResponse::decode(&bytes).unwrap(), r);
    }

    #[test]
    fn system_tag_download_response_list_carries_segment() {
        let r = SystemTagDownloadResponse {
            block_id: 0x01,
            result: ResultCode::Accepted,
            segment_progress: Some(SegmentProgress {
                offset: 0,
                size: 32,
            }),
        };
        let bytes = r.encode().unwrap();
        assert_eq!(bytes, vec![0x14, 0x01, 0x00, 0x00, 0x00, 0x20]);
        assert_eq!(SystemTagDownloadResponse::decode(&bytes).unwrap(), r);
    }
}
