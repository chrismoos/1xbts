//! SSPR Configuration + Download codecs.
//!
//! - **Configuration Request / Response** — C.S0016-D §4.5.1.8 / §3.5.1.8,
//!   `OTASP_MSG_TYPE = 0x07`. Reads the MS's current PRL in two passes:
//!   `BLOCK_ID = 0x00` (PRL Dimensions), then `BLOCK_ID = 0x01` segmented
//!   PRL fetch via `REQUEST_OFFSET` / `REQUEST_MAX_SIZE`, with the MS
//!   returning `LAST_SEGMENT = 1` on the final segment. `BLOCK_ID = 0x02`
//!   is the Extended PRL Dimensions variant (§3.5.3.3).
//! - **Download Request / Response** — C.S0016-D §4.5.1.9 / §3.5.1.9,
//!   `OTASP_MSG_TYPE = 0x08`. Writes a PRL to the MS in BS-driven
//!   segments. `BLOCK_ID = 0x00` classic, `0x01` Extended (Table 4.5.3-1).
//!   Each request carries one PARAM_DATA block whose layout
//!   (`RESERVED + LAST_SEGMENT + SEGMENT_OFFSET + SEGMENT_SIZE +
//!   SEGMENT_DATA`) is built by [`encode_sspr_param_data`].

use cdma_common::bits::Bitstream;

use crate::Error;
use crate::bits::{from_bytes, read_bool, read_u8};
use crate::message::msg_type::{SSPR_CONFIGURATION, SSPR_DOWNLOAD};
use crate::message::require_msg_type;
use crate::message::result_code::ResultCode;

/// SSPR Configuration Request (BS → MS), §4.5.1.8.
///
/// `BLOCK_ID = 0x00` (PRL Dimensions) carries no extra fields. `BLOCK_ID
/// = 0x01` (PRL Parameter Block) carries `REQUEST_OFFSET` / `REQUEST_MAX_SIZE`
/// so the BS can fetch the PRL in segments.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SsprConfigurationRequest {
    pub block_id: u8,
    /// Required for `BLOCK_ID = 0x01`. Ignored otherwise.
    pub request_offset: u16,
    /// Required for `BLOCK_ID = 0x01`. Maximum PRL bytes the MS may
    /// return in `SEGMENT_DATA`.
    pub request_max_size: u8,
}

impl SsprConfigurationRequest {
    /// Build a (classic) PRL Dimensions request (`BLOCK_ID = 0x00`).
    /// MSes storing `SSPR_P_REV >= 3` reject this with `0x23` per
    /// §3.5.1.8 — the caller should follow up with
    /// [`Self::extended_dimensions`].
    pub fn dimensions() -> Self {
        Self {
            block_id: 0x00,
            request_offset: 0,
            request_max_size: 0,
        }
    }

    /// Build an Extended PRL Dimensions request (`BLOCK_ID = 0x02`,
    /// §3.5.3.3). The response carries `CUR_SSPR_P_REV` which tells
    /// the BS whether subsequent segment data decodes as classic or
    /// extended PRL.
    pub fn extended_dimensions() -> Self {
        Self {
            block_id: 0x02,
            request_offset: 0,
            request_max_size: 0,
        }
    }

    /// Build a PRL segment fetch.
    pub fn segment(offset: u16, max_size: u8) -> Self {
        Self {
            block_id: 0x01,
            request_offset: offset,
            request_max_size: max_size,
        }
    }

    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let mut out = Vec::with_capacity(5);
        out.push(SSPR_CONFIGURATION);
        out.push(self.block_id);
        if self.block_id == 0x01 {
            out.extend_from_slice(&self.request_offset.to_be_bytes());
            out.push(self.request_max_size);
        }
        Ok(out)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < 2 {
            return Err("SsprConfigurationRequest too short".into());
        }
        require_msg_type(bytes[0], SSPR_CONFIGURATION)?;
        let block_id = bytes[1];
        let (request_offset, request_max_size) = if block_id == 0x01 {
            if bytes.len() < 5 {
                return Err("SsprConfigurationRequest 0x01 truncated".into());
            }
            (u16::from_be_bytes([bytes[2], bytes[3]]), bytes[4])
        } else {
            (0, 0)
        };
        Ok(Self {
            block_id,
            request_offset,
            request_max_size,
        })
    }
}

/// SSPR Configuration Response (MS → BS), §3.5.1.8.
///
/// One parameter block per message. `param_data` is the raw PARAM_DATA
/// content; its layout depends on `block_id` (Dimensions vs PRL segment)
/// and is decoded by callers via the `param::prl*` modules.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SsprConfigurationResponse {
    pub block_id: u8,
    pub result_code: ResultCode,
    pub param_data: Vec<u8>,
}

impl SsprConfigurationResponse {
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        if self.param_data.len() > u8::MAX as usize {
            return Err("SsprConfigurationResponse PARAM_DATA too long".into());
        }
        let mut bs = Bitstream::new();
        bs.write_u8(SSPR_CONFIGURATION, 8);
        bs.write_u8(self.block_id, 8);
        bs.write_u8(self.result_code.to_u8(), 8);
        bs.write_u8(self.param_data.len() as u8, 8);
        for &b in &self.param_data {
            bs.write_u8(b, 8);
        }
        // FRESH_INCL = 0 + 7 reserved bits to octet-align.
        bs.write_u8(0, 1);
        bs.write_u8(0, 7);
        Ok(bs.to_packed_bytes())
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut bs = from_bytes(bytes);
        require_msg_type(read_u8(&mut bs, 8)?, SSPR_CONFIGURATION)?;
        let block_id = read_u8(&mut bs, 8)?;
        let result_code = ResultCode::from_u8(read_u8(&mut bs, 8)?);
        let block_len = read_u8(&mut bs, 8)? as usize;
        let mut param_data = Vec::with_capacity(block_len);
        for _ in 0..block_len {
            param_data.push(read_u8(&mut bs, 8)?);
        }
        // Trailing FRESH_INCL + RESERVED are nominally mandatory but
        // vintage MSes that never implemented Secure Mode often omit
        // them. Treat as optional.
        if let Ok(fresh_incl) = read_bool(&mut bs)
            && fresh_incl
        {
            return Err(
                "SsprConfigurationResponse FRESH_INCL=1 (Secure Mode) not supported".into(),
            );
        }
        Ok(Self {
            block_id,
            result_code,
            param_data,
        })
    }
}

// ─── SSPR Download ───────────────────────────────────────────────

/// BLOCK_ID values for the SSPR Download path (C.S0016-D §4.5.3
/// Table 4.5.3-1). Distinct from the Configuration Request BLOCK_IDs
/// (Dimensions / Param-Block / Extended-Dimensions live in
/// Table 3.5.3-1).
pub const BLOCK_PRL_CLASSIC: u8 = 0x00;
pub const BLOCK_PRL_EXTENDED: u8 = 0x01;

/// PARAM_DATA wire-layout constants per §4.5.3.1.
const SSPR_DL_RESERVED_BITS: usize = 7;
const SSPR_DL_LAST_SEGMENT_BITS: usize = 1;
const SSPR_DL_SEGMENT_OFFSET_BITS: usize = 16;
const SSPR_DL_SEGMENT_SIZE_BITS: usize = 8;

/// Encode the PARAM_DATA payload of one SSPR Download Request per
/// §4.5.3.1: `RESERVED(7) + LAST_SEGMENT(1) + SEGMENT_OFFSET(16) +
/// SEGMENT_SIZE(8) + SEGMENT_DATA(8 × len)`. Total length is
/// `4 + segment_data.len()` octets (4-octet header + data).
pub fn encode_sspr_param_data(
    last_segment: bool,
    segment_offset: u16,
    segment_data: &[u8],
) -> Result<Vec<u8>, Error> {
    if segment_data.len() > u8::MAX as usize {
        return Err("SSPR Download segment_data exceeds SEGMENT_SIZE (8 bits)".into());
    }
    let mut bs = Bitstream::new();
    bs.write_u8(0, SSPR_DL_RESERVED_BITS);
    bs.write_u8(last_segment as u8, SSPR_DL_LAST_SEGMENT_BITS);
    bs.write_u32(segment_offset as u32, SSPR_DL_SEGMENT_OFFSET_BITS);
    bs.write_u8(segment_data.len() as u8, SSPR_DL_SEGMENT_SIZE_BITS);
    for &b in segment_data {
        bs.write_u8(b, 8);
    }
    Ok(bs.to_packed_bytes())
}

/// SSPR Download Request (BS → MS), §4.5.1.9.
///
/// Wire layout:
/// ```text
/// OTASP_MSG_TYPE  ('00001000')     8 bits
/// BLOCK_ID                          8 bits
/// BLOCK_LEN                         8 bits
/// PARAM_DATA            8 × BLOCK_LEN bits   (§4.5.3.1: RESERVED+LAST_SEGMENT
///                                              +SEGMENT_OFFSET+SEGMENT_SIZE
///                                              +SEGMENT_DATA)
/// FRESH_INCL                        1 bit
/// FRESH                             0 or 15 bits
/// RESERVED                          0 or 7 bits
/// ```
///
/// Secure Mode is not implemented, so `FRESH_INCL = 0` always and the
/// trailer is 8 zero bits.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SsprDownloadRequest {
    pub block_id: u8,
    /// Already framed per §4.5.3.1. Build with [`encode_sspr_param_data`].
    pub param_data: Vec<u8>,
}

impl SsprDownloadRequest {
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        if self.param_data.len() > u8::MAX as usize {
            return Err("SsprDownloadRequest PARAM_DATA too long".into());
        }
        let mut bs = Bitstream::new();
        bs.write_u8(SSPR_DOWNLOAD, 8);
        bs.write_u8(self.block_id, 8);
        bs.write_u8(self.param_data.len() as u8, 8);
        for &b in &self.param_data {
            bs.write_u8(b, 8);
        }
        bs.write_u8(0, 1); // FRESH_INCL
        bs.write_u8(0, 7); // RESERVED to octet boundary
        Ok(bs.to_packed_bytes())
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut bs = from_bytes(bytes);
        require_msg_type(read_u8(&mut bs, 8)?, SSPR_DOWNLOAD)?;
        let block_id = read_u8(&mut bs, 8)?;
        let block_len = read_u8(&mut bs, 8)? as usize;
        let mut param_data = Vec::with_capacity(block_len);
        for _ in 0..block_len {
            param_data.push(read_u8(&mut bs, 8)?);
        }
        if let Ok(fresh_incl) = read_bool(&mut bs)
            && fresh_incl
        {
            return Err("SsprDownloadRequest: FRESH_INCL=1 (Secure Mode) not supported".into());
        }
        Ok(Self {
            block_id,
            param_data,
        })
    }
}

/// SSPR Download Response (MS → BS), §3.5.1.9.
///
/// The MS echoes the BLOCK_ID, RESULT_CODE, and the
/// SEGMENT_OFFSET / SEGMENT_SIZE it just accepted so the BS can
/// detect partial writes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SsprDownloadResponse {
    pub block_id: u8,
    pub result_code: ResultCode,
    pub segment_offset: u16,
    pub segment_size: u8,
}

impl SsprDownloadResponse {
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        let mut bs = Bitstream::new();
        bs.write_u8(SSPR_DOWNLOAD, 8);
        bs.write_u8(self.block_id, 8);
        bs.write_u8(self.result_code.to_u8(), 8);
        bs.write_u32(self.segment_offset as u32, 16);
        bs.write_u8(self.segment_size, 8);
        Ok(bs.to_packed_bytes())
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < 6 {
            return Err("SsprDownloadResponse too short".into());
        }
        require_msg_type(bytes[0], SSPR_DOWNLOAD)?;
        Ok(Self {
            block_id: bytes[1],
            result_code: ResultCode::from_u8(bytes[2]),
            segment_offset: u16::from_be_bytes([bytes[3], bytes[4]]),
            segment_size: bytes[5],
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_dimensions_is_two_bytes() {
        let r = SsprConfigurationRequest::dimensions();
        assert_eq!(r.encode().unwrap(), vec![0x07, 0x00]);
        assert_eq!(SsprConfigurationRequest::decode(&[0x07, 0x00]).unwrap(), r);
    }

    #[test]
    fn request_extended_dimensions_is_two_bytes() {
        let r = SsprConfigurationRequest::extended_dimensions();
        assert_eq!(r.encode().unwrap(), vec![0x07, 0x02]);
        assert_eq!(SsprConfigurationRequest::decode(&[0x07, 0x02]).unwrap(), r);
    }

    #[test]
    fn request_segment_round_trip() {
        let r = SsprConfigurationRequest::segment(0x0102, 200);
        let bytes = r.encode().unwrap();
        assert_eq!(bytes, vec![0x07, 0x01, 0x01, 0x02, 200]);
        assert_eq!(SsprConfigurationRequest::decode(&bytes).unwrap(), r);
    }

    #[test]
    fn response_round_trip() {
        let r = SsprConfigurationResponse {
            block_id: 0x01,
            result_code: ResultCode::Accepted,
            param_data: vec![0xDE, 0xAD, 0xBE, 0xEF],
        };
        let bytes = r.encode().unwrap();
        assert_eq!(SsprConfigurationResponse::decode(&bytes).unwrap(), r);
    }

    #[test]
    fn response_dimensions_layout_pin() {
        // SSPR_MSG_TYPE=0x07, BLOCK_ID=0x00, RESULT_CODE=0x00, BLOCK_LEN=0x07,
        // PARAM_DATA=7 bytes, then FRESH_INCL+RESERVED=0x00.
        let r = SsprConfigurationResponse {
            block_id: 0x00,
            result_code: ResultCode::Accepted,
            param_data: vec![0; 7],
        };
        let bytes = r.encode().unwrap();
        assert_eq!(
            bytes,
            vec![0x07, 0x00, 0x00, 0x07, 0, 0, 0, 0, 0, 0, 0, 0x00]
        );
    }

    #[test]
    fn download_request_round_trip() {
        let param_data = encode_sspr_param_data(false, 0x0200, &[0xAA, 0xBB, 0xCC]).unwrap();
        let r = SsprDownloadRequest {
            block_id: BLOCK_PRL_CLASSIC,
            param_data,
        };
        let bytes = r.encode().unwrap();
        assert_eq!(SsprDownloadRequest::decode(&bytes).unwrap(), r);
    }

    #[test]
    fn download_request_param_data_layout_pin() {
        // 3 segment-data octets: PARAM_DATA = RESERVED(7'0) + LAST_SEGMENT(1)
        // + SEGMENT_OFFSET(16) + SEGMENT_SIZE(8) + SEGMENT_DATA(8*3)
        //                       = 7 octets total.
        // SEGMENT_OFFSET = 0x0102, SEGMENT_SIZE = 3,
        // SEGMENT_DATA = [0xAA, 0xBB, 0xCC], LAST_SEGMENT = 1.
        let pd = encode_sspr_param_data(true, 0x0102, &[0xAA, 0xBB, 0xCC]).unwrap();
        // RESERVED(7) + LAST_SEGMENT(1) packs as bit 0 of octet 0
        // (LAST_SEGMENT in the LSB of the first octet).
        assert_eq!(pd, vec![0x01, 0x01, 0x02, 0x03, 0xAA, 0xBB, 0xCC]);

        let r = SsprDownloadRequest {
            block_id: BLOCK_PRL_EXTENDED,
            param_data: pd,
        };
        // Wire: OTASP_MSG_TYPE=0x08, BLOCK_ID=0x01, BLOCK_LEN=7,
        // PARAM_DATA=7 octets, FRESH_INCL+RESERVED=0x00.
        let bytes = r.encode().unwrap();
        assert_eq!(
            bytes,
            vec![
                0x08, 0x01, 0x07, 0x01, 0x01, 0x02, 0x03, 0xAA, 0xBB, 0xCC, 0x00
            ]
        );
    }

    #[test]
    fn download_response_round_trip() {
        let r = SsprDownloadResponse {
            block_id: BLOCK_PRL_CLASSIC,
            result_code: ResultCode::Accepted,
            segment_offset: 0x0400,
            segment_size: 200,
        };
        let bytes = r.encode().unwrap();
        assert_eq!(bytes, vec![0x08, 0x00, 0x00, 0x04, 0x00, 200]);
        assert_eq!(SsprDownloadResponse::decode(&bytes).unwrap(), r);
    }

    #[test]
    fn encode_sspr_param_data_rejects_oversize_segment() {
        let too_big = vec![0u8; 256];
        assert!(encode_sspr_param_data(false, 0, &too_big).is_err());
    }
}
