use cdma_bts::channels::rtch_rc3::{self, Rc3Frame, Rc3Rate};
use cdma_bts::lac::sar_fragment_ftch_pdu_dsch;
use cdma_common::access::{AccessMessage, AccessMessageHeader, DataBurstMessage};
use cdma_common::bits::Bitstream;
use cdma_common::consts::BURST_TYPE_SMS;
use cdma_common::lac::message_types::{MessageId, WireChannel};
use cdma_common::phy::long_code::LongCodeGenerator;
use num_complex::Complex32;

const FRAME_CHIPS: usize = rtch_rc3::RC3_FRAME_CHIPS;
const PN_PERIOD: usize = 32_768;

#[derive(Debug, Clone)]
pub struct ReverseTrafficConfig {
    pub esn: u32,
    pub oversample: usize,
}

impl Default for ReverseTrafficConfig {
    fn default() -> Self {
        ReverseTrafficConfig {
            esn: 0,
            oversample: 4,
        }
    }
}

/// Build a reverse dedicated Data Burst PDU carrying `fields` (the CHARi octets
/// of a C.S0015 SMS), returning the r-dsch PDU bits:
/// `MSG_TYPE(8) | ACK_SEQ(3) | MSG_SEQ(3) | ACK_REQ(1) | ENCRYPTION(2) | body`.
pub fn build_reverse_data_burst_pdu(
    fields: &[u8],
    burst_type: u8,
    msg_number: u8,
    msg_seq: u8,
    ack_seq: u8,
    ack_req: bool,
) -> Bitstream {
    let dbm = DataBurstMessage {
        header: AccessMessageHeader {
            pd: 0,
            message_id: MessageId::DataBurst,
        },
        msg_number,
        burst_type,
        num_msgs: 1,
        num_fields: fields.len() as u8,
        fields: fields.to_vec(),
        remaining_bits: 0,
    };
    let body = AccessMessage::DataBurst(dbm)
        .to_sdu()
        .expect("data burst body encodes");

    let msg_type = MessageId::DataBurst
        .wire_type(WireChannel::ReverseDedicated)
        .expect("data burst has a reverse-dedicated wire type");

    let mut pdu = Bitstream::new();
    pdu.write_u32(msg_type as u32, 8);
    pdu.write_u32(ack_seq as u32, 3);
    pdu.write_u32(msg_seq as u32, 3);
    pdu.write_u32(ack_req as u32, 1);
    pdu.write_u32(0, 2); // ENCRYPTION = off
    for &bit in body.bits() {
        pdu.write_u8(bit, 1);
    }
    pdu
}

pub fn reverse_sms_data_burst_info_frames(
    sms_bytes: &[u8],
    msg_number: u8,
    msg_seq: u8,
) -> Vec<Vec<u8>> {
    let mut pdu =
        build_reverse_data_burst_pdu(sms_bytes, BURST_TYPE_SMS, msg_number, msg_seq, 0, false);
    // The reassembler keys the CRC-16 position off MSG_LENGTH in whole octets,
    // so the PDU must be byte-aligned before SAR fragmentation.
    while pdu.len() % 8 != 0 {
        pdu.write_u8(0, 1);
    }
    sar_fragment_ftch_pdu_dsch(&pdu)
        .iter()
        .map(|frame| frame.bits().to_vec())
        .collect()
}

fn traffic_lc_state_at(esn: u32, chip: u64) -> u64 {
    let mut lc = LongCodeGenerator::new_traffic_channel(esn);
    lc.advance_chips(chip as usize);
    lc.state()
}

/// Returns unshaped IQ. Apply `pulse_shape` before RF transmission.
pub fn modulate_reverse_rc3_traffic(
    cfg: &ReverseTrafficConfig,
    data_info_frames: &[Vec<u8>],
    frame_start_chip: u64,
    preamble_frames: usize,
    trailing_frames: usize,
) -> Vec<Complex32> {
    let os = cfg.oversample;
    let mut frames: Vec<Rc3Frame> = Vec::new();
    for _ in 0..preamble_frames {
        frames.push(Rc3Frame::PilotOnly);
    }
    for info_bits in data_info_frames {
        frames.push(Rc3Frame::Traffic {
            rate: Rc3Rate::Full,
            info_bits: info_bits.clone(),
        });
    }
    for _ in 0..trailing_frames {
        frames.push(Rc3Frame::PilotOnly);
    }

    // The HPSK spreader consumes one long-code chip as the delayed Q reference
    // before emitting chip 0, so seed the long code one chip early to land the
    // I-channel long code on the absolute chip the short PN is aligned to.
    let mut lc_state = traffic_lc_state_at(cfg.esn, frame_start_chip.saturating_sub(1));
    let mut pn_offset = (frame_start_chip as usize) % PN_PERIOD;

    const LIVE_IQ_POLARITY: f32 = -1.0;
    let mut raw: Vec<Complex32> = Vec::with_capacity(frames.len() * FRAME_CHIPS * os);
    for frame in &frames {
        raw.extend(rtch_rc3::build_reverse_rc3_frame_samples(
            frame,
            cfg.esn,
            lc_state,
            pn_offset,
            os,
            LIVE_IQ_POLARITY,
        ));
        lc_state =
            rtch_rc3::advance_reverse_rc3_traffic_long_code_state(cfg.esn, lc_state, FRAME_CHIPS);
        pn_offset = (pn_offset + FRAME_CHIPS) % PN_PERIOD;
    }

    raw
}

#[cfg(test)]
mod tests {
    use super::*;
    use cdma_bts::receiver::access::DedicatedFrameReader;
    use cdma_bts::receiver::access_layer3::RdschPdu;
    use cdma_bts::receiver::pipelined::{
        PipelinedReceiver, ReverseTrafficSettings, reverse_traffic_chain_rc3,
    };
    use cdma_common::bits::Bitstream;
    use cdma_common::sms::{decode_mo_sms, encode_mo_sms_submit};

    #[test]
    fn reverse_sms_data_burst_frames_reassemble() {
        let sms = encode_mo_sms_submit("5559876", "Hi", 42);
        let frames = reverse_sms_data_burst_info_frames(&sms, 1, 0);
        assert!(frames.len() >= 2, "SMS should span multiple frames");

        let mut reader = DedicatedFrameReader::new();
        let mut recovered: Option<String> = None;
        for frame_bits in &frames {
            // Strip the 4-bit MuxPDU Type 1 header (MM,TT,TM). the reader
            // consumes SOM(1) + SAR fragment.
            let mut sig = Bitstream::new();
            for &bit in &frame_bits[4..] {
                sig.write_u8(bit, 1);
            }
            if let Some(frame) = reader.process(&mut sig).expect("reader") {
                assert!(frame.crc_valid, "reassembled SAR CRC16 must be valid");
                let pdu = RdschPdu::decode(&frame.data).expect("rdsch decode");
                if let cdma_common::access::AccessMessage::DataBurst(m) = &pdu.l3 {
                    assert_eq!(m.burst_type, BURST_TYPE_SMS);
                    recovered = decode_mo_sms(&m.fields).map(|mo| mo.text);
                }
            }
        }
        assert_eq!(recovered.as_deref(), Some("Hi"));
    }

    #[test]
    fn modulated_reverse_rc3_sms_data_burst_decodes_offline() {
        let esn: u32 = 0x4CDC_1D09;
        let oversample = 4usize;
        let sample_rate = 1_228_800.0 * oversample as f64;
        let walsh_code = 11u8;
        // The finger's pilot CFO loop needs a preamble long enough to converge
        // before the data frames arrive.
        let preamble = 30usize;

        let sms = encode_mo_sms_submit("5559876", "Hi", 42);
        let frames = reverse_sms_data_burst_info_frames(&sms, 1, 0);

        let cfg = ReverseTrafficConfig { esn, oversample };
        let iq =
            crate::tx::pulse_shape(&modulate_reverse_rc3_traffic(&cfg, &frames, 0, preamble, 4));

        let pipeline = reverse_traffic_chain_rc3(ReverseTrafficSettings {
            oversample,
            walsh_code,
            esn,
            reanchor_origin: true,
            snr_threshold: None,
            preamble_num_pcgs: None,
            epl_pilot: true,
            rev_fch_gating_mode: false,
            finger_pool_size: 1,
        });
        let mut rx = PipelinedReceiver::new(iq.into_iter())
            .with_input_sample_rate_hz(sample_rate)
            .with_absolute_sample_start(0);
        let out = rx.add_pipeline(pipeline);
        rx.run_pipeline().unwrap();

        let mut decoded_sms: Option<String> = None;
        for blocks in out {
            for blk in &blocks {
                if blk.tags.get("traffic_event") != Some(&1)
                    || blk.tags.get("traffic_crc_valid") != Some(&1)
                {
                    continue;
                }
                let bytes: Vec<u8> = blk
                    .samples
                    .iter()
                    .map(|s| if s.re >= 0.5 { 1u8 } else { 0u8 })
                    .collect();
                let bs = Bitstream::new_init(&bytes);
                if let Ok(pdu) = RdschPdu::decode(&bs) {
                    if let cdma_common::access::AccessMessage::DataBurst(m) = &pdu.l3 {
                        assert_eq!(m.burst_type, BURST_TYPE_SMS, "burst type is SMS");
                        if let Some(mo) = decode_mo_sms(&m.fields) {
                            decoded_sms = Some(mo.text);
                        }
                    }
                }
            }
        }

        assert_eq!(
            decoded_sms.as_deref(),
            Some("Hi"),
            "BTS reverse RC3 chain should decode the MO SMS Data Burst"
        );
    }
}
