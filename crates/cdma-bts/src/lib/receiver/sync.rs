use cdma_common::{bits::Bitstream, error::Error};

use crate::lac::crc30;

pub struct SyncFrame {
    pub data: Bitstream,
    pub crc_valid: bool,
}

pub struct SyncFrameReader {
    data: Vec<u8>,
    message_length_bits: usize,
}

impl SyncFrameReader {
    pub fn new() -> Self {
        Self {
            data: Vec::new(),
            message_length_bits: 0,
        }
    }

    /// Process one 32-bit sync channel frame.
    ///
    /// The frame format is SOM(1) + payload(31). When SOM=1, payload starts
    /// with MSG_LENGTH(8) in octets for the encapsulated PDU+CRC body.
    pub fn process(&mut self, frame: &mut Bitstream) -> Result<Option<SyncFrame>, Error> {
        assert_eq!(32, frame.len());
        let som = frame.read_bits(1)?;

        if som == 1 {
            if self.message_length_bits != 0 {
                //println!("interrupted while reading frame (not enough bits)");
            }
            self.data.clear();
            self.data.extend(frame.bits());
            self.message_length_bits = frame.read_bits(8)? as usize * 8;

            //println!("SOM Detected, message len: {}", self.message_length_bits);

            // Need at least MSG_LENGTH(8) + CRC30.
            if self.message_length_bits < 38 {
                self.data.clear();
                self.message_length_bits = 0;
            }
            return Ok(None);
        }

        if self.data.is_empty() {
            return Ok(None);
        }

        self.data.extend(frame.bits());
        if self.data.len() < self.message_length_bits {
            return Ok(None);
        }

        if self.message_length_bits < 38 {
            self.data.clear();
            self.message_length_bits = 0;
            return Ok(None);
        }

        let expected_crc = crc30(&Bitstream::new_init(
            &self.data[0..self.message_length_bits - 30],
        ));
        let observed_crc = Bitstream::new_init(
            &self.data[self.message_length_bits - 30..self.message_length_bits],
        )
        .read_bits(30)?;
        let crc_valid = observed_crc as u32 == expected_crc;

        // Payload excludes MSG_LENGTH(8) and CRC30.
        let payload = Bitstream::new_init(&self.data[8..self.message_length_bits - 30]);
        self.data.clear();
        self.message_length_bits = 0;

        Ok(Some(SyncFrame {
            data: payload,
            crc_valid,
        }))
    }
}

#[derive(Clone, Debug)]
pub struct SyncChannelMessage {
    pub pd: u8,
    pub msg_type: u8,
    pub p_rev: u8,
    pub min_p_rev: u8,
    pub sid: u16,
    pub nid: u16,
    pub pilot_pn: u16,
    pub lc_state: u64,
    pub sys_time: u64,
    pub lp_sec: u8,
    pub ltm_off: i8,
    pub daylt: u8,
    pub prat: u8,
    pub cdma_freq: u16,
    pub ext_cdma_freq: u16,
    pub sr1_bcch_non_td_incl: bool,
    pub sr1_td_incl: bool,
    pub sr3_incl: bool,
    pub ds_incl: bool,
}

impl SyncChannelMessage {
    /// Serialize body fields to an SDU bitstream (excludes PD/MSG_TYPE, which
    /// LAC's `utility_assemble_f_csch` prepends). This is the inverse of the
    /// field reads in `parse()`.
    pub fn to_sdu(&self) -> Bitstream {
        let mut bs = Bitstream::new();
        bs.write_u8(self.p_rev, 8);
        bs.write_u8(self.min_p_rev, 8);
        bs.write_u64(self.sid as u64, 15);
        bs.write_u64(self.nid as u64, 16);
        bs.write_u64(self.pilot_pn as u64, 9);
        bs.write_u64(self.lc_state, 42);
        bs.write_u64(self.sys_time, 36);
        bs.write_u8(self.lp_sec, 8);
        bs.write_u8(self.ltm_off as u8, 6);
        bs.write_u8(self.daylt, 1);
        bs.write_u8(self.prat, 2);
        bs.write_u64(self.cdma_freq as u64, 11);
        bs.write_u64(self.ext_cdma_freq as u64, 11);
        bs.write_u8(self.sr1_bcch_non_td_incl as u8, 1);
        bs.write_u8(self.sr1_td_incl as u8, 1);
        bs.write_u8(self.sr3_incl as u8, 1);
        bs.write_u8(self.ds_incl as u8, 1);

        bs
    }

    pub fn parse(payload: &mut Bitstream) -> Result<Option<Self>, Error> {
        let pd = payload.read_bits(2)? as u8;
        let msg_type = payload.read_bits(6)? as u8;
        if msg_type != 1 {
            return Ok(None);
        }

        Ok(Some(Self {
            pd,
            msg_type,
            p_rev: payload.read_bits(8)? as u8,
            min_p_rev: payload.read_bits(8)? as u8,
            sid: payload.read_bits(15)? as u16,
            nid: payload.read_bits(16)? as u16,
            pilot_pn: payload.read_bits(9)? as u16,
            lc_state: payload.read_bits(42)? as u64,
            sys_time: payload.read_bits(36)? as u64,
            lp_sec: payload.read_bits(8)? as u8,
            ltm_off: {
                let raw = payload.read_bits(6)? as u8;
                // Sign-extend 6-bit two's complement to i8
                if raw & 0x20 != 0 {
                    (raw | 0xC0) as i8
                } else {
                    raw as i8
                }
            },
            daylt: payload.read_bits(1)? as u8,
            prat: payload.read_bits(2)? as u8,
            cdma_freq: payload.read_bits(11)? as u16,
            ext_cdma_freq: payload.read_bits(11)? as u16,
            sr1_bcch_non_td_incl: payload.read_bits(1)? == 1,
            sr1_td_incl: payload.read_bits(1)? == 1,
            sr3_incl: payload.read_bits(1)? == 1,
            ds_incl: payload.read_bits(1)? == 1,
        }))
    }

    pub fn parse_frame(frame: SyncFrame) -> Result<Option<Self>, Error> {
        if !frame.crc_valid {
            return Ok(None);
        }
        let mut payload = frame.data;
        Self::parse(&mut payload)
    }
}

#[cfg(test)]
mod tests {
    use cdma_common::bits::Bitstream;

    use super::{SyncChannelMessage, SyncFrameReader};
    use crate::lac::crc30;

    #[test]
    fn test_sync_channel_message_to_sdu_roundtrip() {
        let msg = SyncChannelMessage {
            pd: 0,
            msg_type: 1,
            p_rev: 6,
            min_p_rev: 6,
            sid: 42,
            nid: 7,
            pilot_pn: 123,
            lc_state: 0x123456789ab,
            sys_time: 0xabcdef,
            lp_sec: 0,
            ltm_off: 0,
            daylt: 0,
            prat: 3,
            cdma_freq: 384,
            ext_cdma_freq: 0,
            sr1_bcch_non_td_incl: false,
            sr1_td_incl: false,
            sr3_incl: false,
            ds_incl: false,
        };
        let sdu = msg.to_sdu();

        // Prepend PD + MSG_TYPE like LAC does, then parse
        let mut payload = Bitstream::new();
        payload.write_u8(msg.pd, 2);
        payload.write_u8(msg.msg_type, 6);
        payload.extend(&sdu);

        let parsed = SyncChannelMessage::parse(&mut payload).unwrap().unwrap();
        assert_eq!(parsed.p_rev, 6);
        assert_eq!(parsed.min_p_rev, 6);
        assert_eq!(parsed.sid, 42);
        assert_eq!(parsed.nid, 7);
        assert_eq!(parsed.pilot_pn, 123);
        assert_eq!(parsed.lc_state, 0x123456789ab);
        assert_eq!(parsed.sys_time, 0xabcdef);
        assert_eq!(parsed.prat, 3);
        assert_eq!(parsed.cdma_freq, 384);
        assert_eq!(parsed.ext_cdma_freq, 0);
        assert!(!parsed.sr1_bcch_non_td_incl);
        assert!(!parsed.sr1_td_incl);
        assert!(!parsed.sr3_incl);
        assert!(!parsed.ds_incl);
    }

    #[test]
    fn test_ltm_off_negative_roundtrip() {
        // UTC-7 = -14 half-hours. Verify i8 encodes to the same 6-bit
        // two's complement as the old u8 path and round-trips correctly.
        for &offset in &[0i8, 10, -14, -16, 31, -32] {
            let msg = SyncChannelMessage {
                pd: 0,
                msg_type: 1,
                p_rev: 6,
                min_p_rev: 6,
                sid: 1,
                nid: 1,
                pilot_pn: 0,
                lc_state: 0,
                sys_time: 0,
                lp_sec: 0,
                ltm_off: offset,
                daylt: 0,
                prat: 0,
                cdma_freq: 283,
                ext_cdma_freq: 0,
                sr1_bcch_non_td_incl: false,
                sr1_td_incl: false,
                sr3_incl: false,
                ds_incl: false,
            };
            let sdu = msg.to_sdu();

            // Verify the 6-bit wire encoding matches manual two's complement
            let expected_wire = (offset as u8) & 0x3F;
            // ltm_off is at bit offset 8+8+15+16+9+42+36+8 = 142 in the SDU
            let wire_bits = &sdu.bits()[142..148];
            let wire_val = wire_bits.iter().fold(0u8, |acc, &b| (acc << 1) | (b & 1));
            assert_eq!(
                wire_val, expected_wire,
                "wire mismatch for ltm_off={offset}"
            );

            // Parse back and verify round-trip
            let mut payload = Bitstream::new();
            payload.write_u8(msg.pd, 2);
            payload.write_u8(msg.msg_type, 6);
            payload.extend(&sdu);
            let parsed = SyncChannelMessage::parse(&mut payload).unwrap().unwrap();
            assert_eq!(
                parsed.ltm_off, offset,
                "round-trip mismatch for ltm_off={offset}"
            );
        }
    }

    #[test]
    fn test_sync_channel_message_parse_roundtrip() {
        let mut payload = Bitstream::new();
        payload.write_u8(0, 2);
        payload.write_u8(1, 6);
        payload.write_u8(6, 8);
        payload.write_u8(6, 8);
        payload.write_u64(42, 15);
        payload.write_u64(7, 16);
        payload.write_u64(123, 9);
        payload.write_u64(0x123456789ab, 42);
        payload.write_u64(0xabcdef, 36);
        payload.write_u8(0, 8);
        payload.write_u8(0, 6);
        payload.write_u8(0, 1);
        payload.write_u8(3, 2);
        payload.write_u64(384, 11);
        payload.write_u64(0, 11);
        payload.write_u8(0, 1);
        payload.write_u8(0, 1);
        payload.write_u8(0, 1);
        payload.write_u8(0, 1);
        // Align so (MSG_LENGTH(8) + payload + CRC30) lands on octet boundary.
        while payload.len() % 8 != 2 {
            payload.write_u8(0, 1);
        }

        // MSG_LENGTH counts octets of [MSG_LENGTH(8) + payload + CRC30].
        let msg_len_octets = ((8 + payload.len() + 30) / 8) as u8;
        let mut crc_scope = Bitstream::new();
        crc_scope.write_u8(msg_len_octets, 8);
        crc_scope.extend(&payload);
        let crc = crc30(&crc_scope);

        let mut body = Bitstream::new();
        body.write_u8(msg_len_octets, 8);
        body.extend(&payload);
        body.write_u32(crc, 30);

        // Put body into 32-bit frames with SOM on first frame.
        let mut bits = body.bits().to_vec();
        let mut frames = Vec::new();
        let mut first = true;
        while !bits.is_empty() {
            let mut frame = Bitstream::new();
            frame.write_u8(if first { 1 } else { 0 }, 1);
            first = false;
            for _ in 0..31 {
                let b = if bits.is_empty() { 0 } else { bits.remove(0) };
                frame.write_u8(b, 1);
            }
            frames.push(frame);
        }

        let mut reader = SyncFrameReader::new();
        let mut parsed = None;
        for mut frame in frames {
            if let Some(sync_frame) = reader.process(&mut frame).unwrap() {
                parsed = SyncChannelMessage::parse_frame(sync_frame).unwrap();
            }
        }

        let msg = parsed.expect("expected parsed sync message");
        assert_eq!(1, msg.msg_type);
        assert_eq!(42, msg.sid);
        assert_eq!(7, msg.nid);
        assert_eq!(123, msg.pilot_pn);
        assert_eq!(3, msg.prat);
        assert_eq!(384, msg.cdma_freq);
        assert!(!msg.sr1_bcch_non_td_incl);
        assert!(!msg.sr1_td_incl);
        assert!(!msg.sr3_incl);
        assert!(!msg.ds_incl);
    }
}
