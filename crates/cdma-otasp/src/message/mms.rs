//! MMS Configuration Request — C.S0016-D §4.5.1.23.
//! MMS Configuration Response — §3.5.1.23.
//! MMS Download Request — §4.5.1.24.
//! MMS Download Response — §3.5.1.24.
//!
//! Wire formats mirror the NAM Configuration / Download pair: NUM_BLOCKS
//! prefix + per-block (BLOCK_ID, BLOCK_LEN, PARAM_DATA) tuples + per-
//! block RESULT_CODE on the response, followed by `FRESH_INCL` and
//! trailing reserved bits. Secure Mode is not implemented, so
//! `FRESH_INCL` is always `0` and the trailer reduces to 8 zero bits.
//!
//! Parameter block IDs come from §3.5.12 Table 3.5.12-1:
//!   - `0x00` MMS URI Parameters (§3.5.12.1) — list of (idx, ASCII URI).
//!   - `0x01` MMS URI Capability Parameters (§3.5.12.2) — read-only
//!     MS-reported limits.

use cdma_common::bits::Bitstream;

use crate::Error;
use crate::bits::{from_bytes, read_bool, read_u8};
use crate::message::msg_type::{MMS_CONFIGURATION, MMS_DOWNLOAD};
use crate::message::require_msg_type;
use crate::message::result_code::ResultCode;

// ─── BLOCK_IDs (§3.5.12) ──────────────────────────────────────────

pub const BLOCK_MMS_URI: u8 = 0x00;
pub const BLOCK_MMS_URI_CAPABILITY: u8 = 0x01;

// ─── Configuration Request / Response ────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MmsConfigurationRequest {
    pub block_ids: Vec<u8>,
}

impl MmsConfigurationRequest {
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        if self.block_ids.len() > u8::MAX as usize {
            return Err("too many block ids".into());
        }
        let mut out = Vec::with_capacity(2 + self.block_ids.len());
        out.push(MMS_CONFIGURATION);
        out.push(self.block_ids.len() as u8);
        out.extend_from_slice(&self.block_ids);
        Ok(out)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < 2 {
            return Err("MmsConfigurationRequest too short".into());
        }
        require_msg_type(bytes[0], MMS_CONFIGURATION)?;
        let n = bytes[1] as usize;
        if bytes.len() < 2 + n {
            return Err("MmsConfigurationRequest block_ids truncated".into());
        }
        Ok(Self {
            block_ids: bytes[2..2 + n].to_vec(),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MmsParamBlock {
    pub block_id: u8,
    pub param_data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MmsConfigurationResponse {
    pub blocks: Vec<MmsParamBlock>,
    /// Per-block result codes (one per block, in order).
    pub results: Vec<ResultCode>,
}

impl MmsConfigurationResponse {
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        if self.blocks.len() != self.results.len() {
            return Err("blocks and results length mismatch".into());
        }
        if self.blocks.len() > u8::MAX as usize {
            return Err("too many blocks".into());
        }
        let mut bs = Bitstream::new();
        bs.write_u8(MMS_CONFIGURATION, 8);
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
        bs.write_u8(0, 1); // FRESH_INCL
        bs.write_u8(0, 7); // RESERVED to octet boundary
        Ok(bs.to_packed_bytes())
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut bs = from_bytes(bytes);
        require_msg_type(read_u8(&mut bs, 8)?, MMS_CONFIGURATION)?;
        let nb = read_u8(&mut bs, 8)? as usize;
        let mut blocks = Vec::with_capacity(nb);
        for _ in 0..nb {
            let block_id = read_u8(&mut bs, 8)?;
            let len = read_u8(&mut bs, 8)? as usize;
            let mut param_data = Vec::with_capacity(len);
            for _ in 0..len {
                param_data.push(read_u8(&mut bs, 8)?);
            }
            blocks.push(MmsParamBlock {
                block_id,
                param_data,
            });
        }
        let mut results = Vec::with_capacity(nb);
        for _ in 0..nb {
            results.push(ResultCode::from_u8(read_u8(&mut bs, 8)?));
        }
        if let Ok(fresh_incl) = read_bool(&mut bs)
            && fresh_incl
        {
            return Err(
                "MmsConfigurationResponse: FRESH_INCL=1 (Secure Mode) not supported".into(),
            );
        }
        Ok(Self { blocks, results })
    }
}

// ─── Download Request / Response ─────────────────────────────────

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MmsDownloadRequest {
    pub blocks: Vec<MmsParamBlock>,
}

impl MmsDownloadRequest {
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        if self.blocks.len() > u8::MAX as usize {
            return Err("too many blocks".into());
        }
        let mut bs = Bitstream::new();
        bs.write_u8(MMS_DOWNLOAD, 8);
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
        bs.write_u8(0, 1); // FRESH_INCL
        bs.write_u8(0, 7); // RESERVED
        Ok(bs.to_packed_bytes())
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut bs = from_bytes(bytes);
        require_msg_type(read_u8(&mut bs, 8)?, MMS_DOWNLOAD)?;
        let nb = read_u8(&mut bs, 8)? as usize;
        let mut blocks = Vec::with_capacity(nb);
        for _ in 0..nb {
            let block_id = read_u8(&mut bs, 8)?;
            let len = read_u8(&mut bs, 8)? as usize;
            let mut param_data = Vec::with_capacity(len);
            for _ in 0..len {
                param_data.push(read_u8(&mut bs, 8)?);
            }
            blocks.push(MmsParamBlock {
                block_id,
                param_data,
            });
        }
        if let Ok(fresh_incl) = read_bool(&mut bs)
            && fresh_incl
        {
            return Err("MmsDownloadRequest: FRESH_INCL=1 (Secure Mode) not supported".into());
        }
        Ok(Self { blocks })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MmsDownloadConfirmation {
    pub block_id: u8,
    pub result: ResultCode,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MmsDownloadResponse {
    pub confirmations: Vec<MmsDownloadConfirmation>,
}

impl MmsDownloadResponse {
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        if self.confirmations.len() > u8::MAX as usize {
            return Err("too many confirmations".into());
        }
        let mut out = Vec::with_capacity(2 + 2 * self.confirmations.len());
        out.push(MMS_DOWNLOAD);
        out.push(self.confirmations.len() as u8);
        for c in &self.confirmations {
            out.push(c.block_id);
            out.push(c.result.to_u8());
        }
        Ok(out)
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        if bytes.len() < 2 {
            return Err("MmsDownloadResponse too short".into());
        }
        require_msg_type(bytes[0], MMS_DOWNLOAD)?;
        let n = bytes[1] as usize;
        if bytes.len() < 2 + 2 * n {
            return Err("MmsDownloadResponse confirmations truncated".into());
        }
        let mut confirmations = Vec::with_capacity(n);
        for i in 0..n {
            confirmations.push(MmsDownloadConfirmation {
                block_id: bytes[2 + 2 * i],
                result: ResultCode::from_u8(bytes[2 + 2 * i + 1]),
            });
        }
        Ok(Self { confirmations })
    }
}

// ─── MMS URI Parameters block (§3.5.12.1) ────────────────────────

/// One entry in the MMS URI table.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MmsUriEntry {
    /// MMS_URI_ENTRY_IDX (4 bits) — index in the handset's URI table.
    /// 0 is the primary MMSC slot on most devices.
    pub entry_idx: u8,
    /// ASCII URI. Length on the wire is the octet count, packed into
    /// an 8-bit MMS_URI_LENGTH field.
    pub uri: String,
}

/// MMS URI Parameters Parameter Block (§3.5.12.1).
///
/// Wire layout:
///   NUM_MMS_URI (4)
///   [ MMS_URI_ENTRY_IDX (4) + MMS_URI_LENGTH (8) + URI ASCII (8 × len) ] × NUM_MMS_URI
///   RESERVED (0..=7) to octet boundary
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct MmsUriParameters {
    pub entries: Vec<MmsUriEntry>,
}

impl MmsUriParameters {
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        if self.entries.len() > 0x0F {
            return Err("MMS URI entries exceed 4-bit NUM_MMS_URI".into());
        }
        let mut bs = Bitstream::new();
        bs.write_u8(self.entries.len() as u8, 4);
        for e in &self.entries {
            if e.entry_idx > 0x0F {
                return Err("MMS_URI_ENTRY_IDX exceeds 4 bits".into());
            }
            let bytes = e.uri.as_bytes();
            if bytes.len() > u8::MAX as usize {
                return Err("MMS_URI_LENGTH exceeds 8 bits".into());
            }
            if !bytes.iter().all(|b| b.is_ascii() && *b >= 0x20) {
                return Err("MMS URI must be printable ASCII".into());
            }
            bs.write_u8(e.entry_idx, 4);
            bs.write_u8(bytes.len() as u8, 8);
            for &b in bytes {
                bs.write_u8(b, 8);
            }
        }
        // Pad to octet boundary.
        let pad = (8 - (bs.len() % 8)) % 8;
        if pad != 0 {
            bs.write_u8(0, pad);
        }
        Ok(bs.to_packed_bytes())
    }

    pub fn decode(param_data: &[u8]) -> Result<Self, Error> {
        let mut bs = from_bytes(param_data);
        let num = read_u8(&mut bs, 4)? as usize;
        let mut entries = Vec::with_capacity(num);
        for _ in 0..num {
            let entry_idx = read_u8(&mut bs, 4)?;
            let len = read_u8(&mut bs, 8)? as usize;
            let mut bytes = Vec::with_capacity(len);
            for _ in 0..len {
                bytes.push(read_u8(&mut bs, 8)?);
            }
            let uri = String::from_utf8(bytes)
                .map_err(|_| "MMS URI was not valid UTF-8 (expected ASCII)")?;
            entries.push(MmsUriEntry { entry_idx, uri });
        }
        Ok(Self { entries })
    }
}

// ─── MMS URI Capability Parameters block (§3.5.12.2) ─────────────

/// Read-only block the MS returns to describe its URI table limits.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct MmsUriCapability {
    /// MAX_NUM_MMS_URI (4 bits).
    pub max_num_uri: u8,
    /// MAX_MMS_URI_LENGTH (8 bits) — per-URI byte cap.
    pub max_uri_length: u8,
}

impl MmsUriCapability {
    pub fn decode(param_data: &[u8]) -> Result<Self, Error> {
        let mut bs = from_bytes(param_data);
        let max_num_uri = read_u8(&mut bs, 4)?;
        let max_uri_length = read_u8(&mut bs, 8)?;
        Ok(Self {
            max_num_uri,
            max_uri_length,
        })
    }

    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        if self.max_num_uri > 0x0F {
            return Err("MAX_NUM_MMS_URI exceeds 4 bits".into());
        }
        let mut bs = Bitstream::new();
        bs.write_u8(self.max_num_uri, 4);
        bs.write_u8(self.max_uri_length, 8);
        bs.write_u8(0, 7); // RESERVED to round out the 19-bit body.
        Ok(bs.to_packed_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn mms_uri_round_trip_single_entry() {
        let p = MmsUriParameters {
            entries: vec![MmsUriEntry {
                entry_idx: 0,
                uri: "http://mmsc.local.1xbts.org/".into(),
            }],
        };
        let bytes = p.encode().unwrap();
        let back = MmsUriParameters::decode(&bytes).unwrap();
        assert_eq!(back, p);
    }

    #[test]
    fn mms_uri_round_trip_two_entries() {
        let p = MmsUriParameters {
            entries: vec![
                MmsUriEntry {
                    entry_idx: 0,
                    uri: "http://mms.example.com/".into(),
                },
                MmsUriEntry {
                    entry_idx: 1,
                    uri: "http://mms-alt.example.com/".into(),
                },
            ],
        };
        let bytes = p.encode().unwrap();
        assert_eq!(MmsUriParameters::decode(&bytes).unwrap(), p);
    }

    #[test]
    fn mms_uri_rejects_non_ascii() {
        let p = MmsUriParameters {
            entries: vec![MmsUriEntry {
                entry_idx: 0,
                uri: "http://mmsc.\u{1F600}/".into(),
            }],
        };
        assert!(p.encode().is_err());
    }

    #[test]
    fn mms_config_request_round_trip() {
        let r = MmsConfigurationRequest {
            block_ids: vec![BLOCK_MMS_URI, BLOCK_MMS_URI_CAPABILITY],
        };
        let bytes = r.encode().unwrap();
        assert_eq!(bytes, vec![MMS_CONFIGURATION, 0x02, 0x00, 0x01]);
        assert_eq!(MmsConfigurationRequest::decode(&bytes).unwrap(), r);
    }

    #[test]
    fn mms_download_request_round_trip() {
        let r = MmsDownloadRequest {
            blocks: vec![MmsParamBlock {
                block_id: BLOCK_MMS_URI,
                param_data: vec![0x10, 0x06, b'm', b'm', b's', b':', b'/', b'/'],
            }],
        };
        let bytes = r.encode().unwrap();
        let back = MmsDownloadRequest::decode(&bytes).unwrap();
        assert_eq!(back, r);
    }

    #[test]
    fn mms_download_response_round_trip() {
        let r = MmsDownloadResponse {
            confirmations: vec![MmsDownloadConfirmation {
                block_id: BLOCK_MMS_URI,
                result: ResultCode::Accepted,
            }],
        };
        let bytes = r.encode().unwrap();
        assert_eq!(bytes, vec![MMS_DOWNLOAD, 0x01, 0x00, 0x00]);
        assert_eq!(MmsDownloadResponse::decode(&bytes).unwrap(), r);
    }

    #[test]
    fn mms_capability_round_trip() {
        let c = MmsUriCapability {
            max_num_uri: 2,
            max_uri_length: 64,
        };
        let bytes = c.encode().unwrap();
        assert_eq!(MmsUriCapability::decode(&bytes).unwrap(), c);
    }
}
