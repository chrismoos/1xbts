use std::collections::VecDeque;

use cdma_common::{bits::Bitstream, error::Error};

use crate::lac::crc30;

pub struct PagingFrame {
    pub data: Bitstream,
    pub crc_valid: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PagingChannelRate {
    Rate4800,
    Rate9600,
}

pub struct PagingFrameReader {
    data: Vec<u8>,
    message_length: usize,
    in_message: bool,
    data_rate: PagingChannelRate,
    completed_frames: VecDeque<PagingFrame>,
}

impl PagingFrameReader {
    const MIN_MSG_LENGTH_OCTETS: usize = 5;
    const MAX_MSG_LENGTH_BITS: usize = 576;

    pub fn new() -> PagingFrameReader {
        Self::new_with_rate(PagingChannelRate::Rate9600)
    }

    pub fn new_with_rate(data_rate: PagingChannelRate) -> PagingFrameReader {
        PagingFrameReader {
            data: Vec::new(),
            message_length: 0,
            in_message: false,
            data_rate,
            completed_frames: VecDeque::new(),
        }
    }

    /// Whether the reader is currently accumulating a multi-frame message.
    pub fn in_message(&self) -> bool {
        self.in_message
    }

    fn half_frame_bits(&self) -> usize {
        match self.data_rate {
            PagingChannelRate::Rate4800 => 48,
            PagingChannelRate::Rate9600 => 96,
        }
    }

    pub fn take_completed_frame(&mut self) -> Option<PagingFrame> {
        self.completed_frames.pop_front()
    }

    fn reset(&mut self) {
        self.data.clear();
        self.message_length = 0;
        self.in_message = false;
    }

    fn parse_msg_length_bits(prefix: &[u8]) -> Option<usize> {
        if prefix.len() < 8 {
            return None;
        }

        let msg_length_octets = prefix[0..8].iter().fold(0u8, |acc, &b| (acc << 1) | b) as usize;
        let msg_length_bits = msg_length_octets * 8;

        if msg_length_octets < Self::MIN_MSG_LENGTH_OCTETS
            || msg_length_bits > Self::MAX_MSG_LENGTH_BITS
        {
            return None;
        }

        Some(msg_length_bits)
    }

    fn extract_completed_frames(&mut self) -> Result<(), Error> {
        while self.in_message && self.data.len() >= self.message_length {
            let expected_crc = crc30(&Bitstream::new_init(
                &self.data[0..self.message_length - 30],
            ));
            let observed_crc =
                Bitstream::new_init(&self.data[self.message_length - 30..self.message_length])
                    .read_bits(30)?;

            let crc_valid = observed_crc as u32 == expected_crc;
            let payload = Bitstream::new_init(&self.data[8..self.message_length - 30]);

            if !crc_valid {
                self.completed_frames.push_back(PagingFrame {
                    data: payload,
                    crc_valid,
                });
                self.reset();
                return Ok(());
            }

            self.completed_frames.push_back(PagingFrame {
                data: payload,
                crc_valid,
            });

            let leftover = self.data.split_off(self.message_length);
            self.data = leftover;

            if let Some(next_message_length) = Self::parse_msg_length_bits(&self.data) {
                self.message_length = next_message_length;
                self.in_message = true;
            } else {
                self.reset();
            }
        }

        Ok(())
    }

    pub fn process(&mut self, half_frame: &mut Bitstream) -> Result<Option<PagingFrame>, Error> {
        let expected_bits = self.half_frame_bits();
        assert_eq!(
            expected_bits,
            half_frame.len(),
            "Half-frame length mismatch: expected {} bits for {:?}, got {}",
            expected_bits,
            self.data_rate,
            half_frame.len()
        );
        let sci = half_frame.read_bits(1)?;

        if sci == 1 {
            self.reset();
            self.data.extend(half_frame.bits());
            if let Some(message_length) = Self::parse_msg_length_bits(&self.data) {
                self.message_length = message_length;
                self.in_message = true;
                self.extract_completed_frames()?;
            } else {
                self.reset();
            }
            return Ok(self.take_completed_frame());
        }

        if !self.in_message {
            return Ok(None);
        }

        self.data.extend(half_frame.bits());
        self.extract_completed_frames()?;
        Ok(self.take_completed_frame())
    }
}

#[cfg(test)]
mod tests {
    use cdma_common::bits::Bitstream;

    use crate::{
        lac::{DataRequest, Layer2Lac, MessageControlStatusBlock},
        mac::types::ChannelType,
    };

    use super::{PagingChannelRate, PagingFrameReader};

    fn make_test_pdu(
        sdu_bits: &[u8],
        message_id: crate::lac::message_types::MessageId,
    ) -> Bitstream {
        let data_request = DataRequest {
            sdu: Bitstream::new_init(sdu_bits),
            mcsb: MessageControlStatusBlock {
                channel: ChannelType::FPch,
                mobile_p_rev: None,
                extended_encryption: false,
                message_id,
                length_bits: sdu_bits.len(),
                requested_tx_time: None,
                tx_deadline: None,
                address: None,
                ack_seq: 0,
                msg_seq: 0,
                ack_req: false,
                valid_ack: false,
                overhead_mcc: 0x03ff,
                overhead_imsi_11_12: 0x7f,
            },
        };

        Layer2Lac::assemble_pdu(data_request).unwrap().e_pdu
    }

    fn make_half_frame(payload_bits: &[u8]) -> Bitstream {
        let mut half_frame = Bitstream::new();
        half_frame.write_u8(1, 1);
        half_frame.extend(&Bitstream::new_init(payload_bits));
        if half_frame.len() < 96 {
            half_frame.write_u8(0, 96 - half_frame.len());
        }
        half_frame
    }

    #[test]
    fn paging_reader_decodes_message_that_fits_in_first_half_frame() {
        let encapsulated =
            make_test_pdu(&[1, 0], crate::lac::message_types::MessageId::GeneralPage);
        let expected_payload =
            Bitstream::new_init(&encapsulated.bits()[8..encapsulated.len() - 30]);
        let mut reader = PagingFrameReader::new_with_rate(PagingChannelRate::Rate9600);
        let mut half_frame = make_half_frame(encapsulated.bits());

        let frame = reader.process(&mut half_frame).unwrap().unwrap();

        assert!(frame.crc_valid);
        assert_eq!(frame.data.bits(), expected_payload.bits());
        assert!(!reader.in_message());
        assert!(reader.take_completed_frame().is_none());
    }

    #[test]
    fn paging_reader_keeps_chained_message_start_after_crc_valid_frame() {
        let first = make_test_pdu(&[1, 0], crate::lac::message_types::MessageId::GeneralPage);
        let second = make_test_pdu(
            &[0, 1],
            crate::lac::message_types::MessageId::GlobalServiceRedirection,
        );
        let expected_first = Bitstream::new_init(&first.bits()[8..first.len() - 30]);
        let expected_second = Bitstream::new_init(&second.bits()[8..second.len() - 30]);

        let second_prefix_bits = 95usize - first.len();
        let mut first_half_frame = Bitstream::new();
        first_half_frame.write_u8(1, 1);
        first_half_frame.extend(&first);
        first_half_frame.extend_n(&second, second_prefix_bits);

        let mut second_half_frame = Bitstream::new();
        second_half_frame.write_u8(0, 1);
        second_half_frame.extend(&Bitstream::new_init(&second.bits()[second_prefix_bits..]));
        while second_half_frame.len() < 96 {
            second_half_frame.write_u8(0, 1);
        }

        let mut reader = PagingFrameReader::new_with_rate(PagingChannelRate::Rate9600);

        let first_frame = reader.process(&mut first_half_frame).unwrap().unwrap();
        assert!(first_frame.crc_valid);
        assert_eq!(first_frame.data.bits(), expected_first.bits());
        assert!(reader.in_message());

        let second_frame = reader.process(&mut second_half_frame).unwrap().unwrap();
        assert!(second_frame.crc_valid);
        assert_eq!(second_frame.data.bits(), expected_second.bits());
        assert!(!reader.in_message());
    }
}
