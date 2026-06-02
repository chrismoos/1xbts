//! CDMA NAM parameter block — `BLOCK_ID = 0x02`.
//!
//! The Configuration Response (§3.5.2.3) and Download Request
//! (§4.5.2.3) layouts are **different**:
//! - Response leads with `RESERVED(2) | SLOTTED_MODE(1) | RESERVED(5) |
//!   MOB_P_REV(8) | ...` and carries `MAX_SID_NID` plus
//!   `STORED_SID_NID` for the per-pair count.
//! - Download starts at `IMSI_M_CLASS`, omits `SLOTTED_MODE`,
//!   `MOB_P_REV`, `MAX_SID_NID`, and uses `N_SID_NID`.
//!
//! `decode` parses the Response; `encode` emits a Download Request.
//! The response-only fields are populated by `decode` and ignored by
//! `encode`.

use cdma_common::bits::Bitstream;

use crate::Error;
use crate::bits::{from_bytes, read_bool, read_u8, read_u16};
use crate::param::nam_cdma_analog::SidNidPair;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamCdma {
    pub slotted_mode: bool,     // 1 bit, RT
    pub mob_p_rev: u8,          // 8 bits, RO
    pub imsi_m_class: bool,     // 1 bit, W
    pub imsi_m_addr_num: u8,    // 3 bits, W
    pub mcc_m: u16,             // 10 bits, W
    pub imsi_m_11_12: u8,       // 7 bits, W
    pub imsi_m_s: u64,          // 34 bits, W
    pub accolc: u8,             // 4 bits, W
    pub local_control: bool,    // 1 bit, W
    pub mob_term_home: bool,    // 1 bit, W
    pub mob_term_for_sid: bool, // 1 bit, W
    pub mob_term_for_nid: bool, // 1 bit, W
    pub max_sid_nid: u8,        // 8 bits, RO
    pub sid_nid_pairs: Vec<SidNidPair>,
}

impl NamCdma {
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        if self.imsi_m_addr_num >= 1 << 3 {
            return Err("imsi_m_addr_num >= 8".into());
        }
        if self.mcc_m >= 1 << 10 {
            return Err("mcc_m >= 1024".into());
        }
        if self.imsi_m_11_12 >= 1 << 7 {
            return Err("imsi_m_11_12 >= 128".into());
        }
        if self.imsi_m_s >= 1u64 << 34 {
            return Err("imsi_m_s >= 2^34".into());
        }
        if self.accolc >= 1 << 4 {
            return Err("accolc >= 16".into());
        }
        if self.sid_nid_pairs.len() > u8::MAX as usize {
            return Err("too many SID/NID pairs".into());
        }

        let mut bs = Bitstream::new();
        bs.write_u8(self.imsi_m_class as u8, 1);
        bs.write_u8(self.imsi_m_addr_num, 3);
        bs.write_u32(self.mcc_m as u32, 10);
        bs.write_u8(self.imsi_m_11_12, 7);
        bs.write_u64(self.imsi_m_s, 34);
        bs.write_u8(self.accolc, 4);
        bs.write_u8(self.local_control as u8, 1);
        bs.write_u8(self.mob_term_home as u8, 1);
        bs.write_u8(self.mob_term_for_sid as u8, 1);
        bs.write_u8(self.mob_term_for_nid as u8, 1);
        bs.write_u8(self.sid_nid_pairs.len() as u8, 8); // N_SID_NID
        for p in &self.sid_nid_pairs {
            if p.sid >= 1 << 15 {
                return Err("SID >= 32768".into());
            }
            bs.write_u32(p.sid as u32, 15);
            bs.write_u32(p.nid as u32, 16);
        }
        Ok(bs.to_packed_bytes())
    }

    /// Encode this struct as a Configuration Response PARAM_DATA per
    /// §3.5.2.3. Useful for test fixtures that simulate the MS's
    /// read-back; production never emits this format.
    pub fn encode_configuration_response(&self) -> Result<Vec<u8>, Error> {
        if self.imsi_m_addr_num >= 1 << 3 {
            return Err("imsi_m_addr_num >= 8".into());
        }
        if self.mcc_m >= 1 << 10 {
            return Err("mcc_m >= 1024".into());
        }
        if self.imsi_m_11_12 >= 1 << 7 {
            return Err("imsi_m_11_12 >= 128".into());
        }
        if self.imsi_m_s >= 1u64 << 34 {
            return Err("imsi_m_s >= 2^34".into());
        }
        if self.accolc >= 1 << 4 {
            return Err("accolc >= 16".into());
        }
        let mut bs = Bitstream::new();
        bs.write_u8(0, 2); // RESERVED
        bs.write_u8(self.slotted_mode as u8, 1);
        bs.write_u8(0, 5); // RESERVED
        bs.write_u8(self.mob_p_rev, 8);
        bs.write_u8(self.imsi_m_class as u8, 1);
        bs.write_u8(self.imsi_m_addr_num, 3);
        bs.write_u32(self.mcc_m as u32, 10);
        bs.write_u8(self.imsi_m_11_12, 7);
        bs.write_u64(self.imsi_m_s, 34);
        bs.write_u8(self.accolc, 4);
        bs.write_u8(self.local_control as u8, 1);
        bs.write_u8(self.mob_term_home as u8, 1);
        bs.write_u8(self.mob_term_for_sid as u8, 1);
        bs.write_u8(self.mob_term_for_nid as u8, 1);
        bs.write_u8(self.max_sid_nid, 8);
        bs.write_u8(self.sid_nid_pairs.len() as u8, 8); // STORED_SID_NID
        for p in &self.sid_nid_pairs {
            if p.sid >= 1 << 15 {
                return Err("SID >= 32768".into());
            }
            bs.write_u32(p.sid as u32, 15);
            bs.write_u32(p.nid as u32, 16);
        }
        Ok(bs.to_packed_bytes())
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut bs = from_bytes(bytes);
        let _ = read_u8(&mut bs, 2)?; // RESERVED
        let slotted_mode = read_bool(&mut bs)?;
        let _ = read_u8(&mut bs, 5)?; // RESERVED
        let mob_p_rev = read_u8(&mut bs, 8)?;
        let imsi_m_class = read_bool(&mut bs)?;
        let imsi_m_addr_num = read_u8(&mut bs, 3)?;
        let mcc_m = read_u16(&mut bs, 10)?;
        let imsi_m_11_12 = read_u8(&mut bs, 7)?;
        let imsi_m_s = bs.read_bits(34)?;
        let accolc = read_u8(&mut bs, 4)?;
        let local_control = read_bool(&mut bs)?;
        let mob_term_home = read_bool(&mut bs)?;
        let mob_term_for_sid = read_bool(&mut bs)?;
        let mob_term_for_nid = read_bool(&mut bs)?;
        let max_sid_nid = read_u8(&mut bs, 8)?;
        let stored = read_u8(&mut bs, 8)?;
        let mut sid_nid_pairs = Vec::with_capacity(stored as usize);
        for _ in 0..stored {
            let sid = read_u16(&mut bs, 15)?;
            let nid = read_u16(&mut bs, 16)?;
            sid_nid_pairs.push(SidNidPair { sid, nid });
        }
        Ok(Self {
            slotted_mode,
            mob_p_rev,
            imsi_m_class,
            imsi_m_addr_num,
            mcc_m,
            imsi_m_11_12,
            imsi_m_s,
            accolc,
            local_control,
            mob_term_home,
            mob_term_for_sid,
            mob_term_for_nid,
            max_sid_nid,
            sid_nid_pairs,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(pairs: Vec<SidNidPair>) -> NamCdma {
        NamCdma {
            slotted_mode: true,
            mob_p_rev: 6,
            imsi_m_class: true,
            imsi_m_addr_num: 0,
            mcc_m: 209,
            imsi_m_11_12: 44,
            imsi_m_s: 0xDEAD_BEEF,
            accolc: 5,
            local_control: false,
            mob_term_home: true,
            mob_term_for_sid: false,
            mob_term_for_nid: true,
            max_sid_nid: 4,
            sid_nid_pairs: pairs,
        }
    }

    #[test]
    fn download_length_with_no_pairs() {
        // 1+3+10+7+34+4+1+1+1+1+8 = 71 bits → 9 bytes pad.
        let v = fixture(vec![]);
        let bytes = v.encode().unwrap();
        assert_eq!(bytes.len(), 9);
    }

    #[test]
    fn download_length_with_one_pair() {
        // 71 + 31 = 102 bits → 13 bytes (2 pad).
        let v = fixture(vec![SidNidPair {
            sid: 22,
            nid: 65535,
        }]);
        let bytes = v.encode().unwrap();
        assert_eq!(bytes.len(), 13);
    }
}
