//! CDMA/Analog NAM parameter block — `BLOCK_ID = 0x00`.
//!
//! The Configuration Response (§3.5.2.1) and Download Request
//! (§4.5.2.1) layouts are **different**:
//! - Response carries `SCM`, `MOB_P_REV`, `MAX_SID_NID`, and uses
//!   `STORED_SID_NID` for the per-pair count.
//! - Download omits the three response-only fields and uses
//!   `N_SID_NID` instead.
//!
//! `decode` parses the Configuration Response. `encode` emits a
//! Download Request. The response-only fields on this struct
//! (`scm`, `mob_p_rev`, `max_sid_nid`) are populated by `decode` and
//! ignored by `encode`.

use cdma_common::bits::Bitstream;

use crate::Error;
use crate::bits::{from_bytes, read_bool, read_u8, read_u16};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SidNidPair {
    pub sid: u16, // 15 bits
    pub nid: u16, // 16 bits
}

/// CDMA/Analog NAM Parameter Block.
///
/// Field semantics: see `docs/otasp-plan.md` Appendix A.1. Tagging is W
/// (writable F.3 NAM indicator), RO (F.2 permanent — echo on Download), or
/// RT (runtime / derived). Wire format is identical in both directions; the
/// session driver supplies values according to direction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NamCdmaAnalog {
    pub firstchp: u16,          // 11 bits, W
    pub home_sid: u16,          // 15 bits, W
    pub ex: bool,               // 1 bit, W
    pub scm: u8,                // 8 bits, RO
    pub mob_p_rev: u8,          // 8 bits, RO
    pub imsi_m_class: bool,     // 1 bit, W (0=class-0, 1=class-1)
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

impl NamCdmaAnalog {
    pub fn encode(&self) -> Result<Vec<u8>, Error> {
        if self.firstchp >= 1 << 11 {
            return Err("firstchp >= 2048".into());
        }
        if self.home_sid >= 1 << 15 {
            return Err("home_sid >= 32768".into());
        }
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
        for p in &self.sid_nid_pairs {
            if p.sid >= 1 << 15 {
                return Err("SID >= 32768".into());
            }
        }

        let mut bs = Bitstream::new();
        bs.write_u32(self.firstchp as u32, 11);
        bs.write_u32(self.home_sid as u32, 15);
        bs.write_u8(self.ex as u8, 1);
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
            bs.write_u32(p.sid as u32, 15);
            bs.write_u32(p.nid as u32, 16);
        }
        // RESERVED bits to whole octet — to_packed_bytes() right-pads with 0.
        Ok(bs.to_packed_bytes())
    }

    /// Encode this struct as a Configuration Response PARAM_DATA per
    /// §3.5.2.1. Useful for test fixtures that simulate the MS's
    /// read-back; production never emits this format.
    pub fn encode_configuration_response(&self) -> Result<Vec<u8>, Error> {
        if self.firstchp >= 1 << 11 {
            return Err("firstchp >= 2048".into());
        }
        if self.home_sid >= 1 << 15 {
            return Err("home_sid >= 32768".into());
        }
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
        bs.write_u32(self.firstchp as u32, 11);
        bs.write_u32(self.home_sid as u32, 15);
        bs.write_u8(self.ex as u8, 1);
        bs.write_u8(self.scm, 8);
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
            bs.write_u32(p.sid as u32, 15);
            bs.write_u32(p.nid as u32, 16);
        }
        Ok(bs.to_packed_bytes())
    }

    pub fn decode(bytes: &[u8]) -> Result<Self, Error> {
        let mut bs = from_bytes(bytes);
        let firstchp = read_u16(&mut bs, 11)?;
        let home_sid = read_u16(&mut bs, 15)?;
        let ex = read_bool(&mut bs)?;
        let scm = read_u8(&mut bs, 8)?;
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
            firstchp,
            home_sid,
            ex,
            scm,
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

    fn fixture(pairs: Vec<SidNidPair>) -> NamCdmaAnalog {
        NamCdmaAnalog {
            firstchp: 283,
            home_sid: 22,
            ex: false,
            scm: 0x52,
            mob_p_rev: 6,
            imsi_m_class: true,
            imsi_m_addr_num: 0,
            mcc_m: 209,       // "310"
            imsi_m_11_12: 44, // "55"
            imsi_m_s: 0x12345678,
            accolc: 7,
            local_control: false,
            mob_term_home: true,
            mob_term_for_sid: true,
            mob_term_for_nid: true,
            max_sid_nid: 4,
            sid_nid_pairs: pairs,
        }
    }

    #[test]
    fn download_length_with_no_pairs() {
        // 11+15+1+1+3+10+7+34+4+1+1+1+1+1+8 = 99 bits → 13 bytes pad.
        let v = fixture(vec![]);
        let bytes = v.encode().unwrap();
        assert_eq!(bytes.len(), 13);
    }

    #[test]
    fn download_length_with_one_pair() {
        // 99 + 31 = 130 bits → 17 bytes (6 pad).
        let v = fixture(vec![SidNidPair {
            sid: 22,
            nid: 65535,
        }]);
        let bytes = v.encode().unwrap();
        assert_eq!(bytes.len(), 17);
    }
}
