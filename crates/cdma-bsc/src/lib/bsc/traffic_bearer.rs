//! Abis bearer transport for traffic channels.
//!
//! This module owns forward bearer frame enqueue and reverse bearer frame
//! draining/decode. Higher-level traffic-channel L3 state remains in
//! `traffic_signaling.rs`.

use std::time::Instant;

use cdma_abis::bearer::{
    ChannelFamily, ForwardFchDcchFrame, ForwardSchFrame, FrameContent, REVERSE_FRAME_CONTENT_NULL,
    ReverseFchDcchFrame, TrafficFrame as AbisTrafficFrame,
};
use cdma_bts::receiver::{
    access::DedicatedFrameReader,
    pipelined::{
        ReverseMux1SignalingLayout, extract_reverse_mux1_full_rate_signaling_block,
        parse_reverse_mux1_full_rate_format,
    },
};
use cdma_common::access::RdschPdu;
use cdma_common::channel::TrafficRate;
use cdma_common::events::AccessChannelEvent;
use cdma_common::lac::message_types::MessageId;
use cdma_common::{bits::Bitstream, error::Error};
use log::{debug, info, warn};
use uuid::Uuid;

use crate::abis_edge::{BearerFrame, BtsControlClient, ForwardBearerQueue};
use crate::addressing::is_packet_data_so;

use super::Bsc;

fn encode_forward_bearer_rate(for_rc: u8, rate: TrafficRate) -> FrameContent {
    match (for_rc, rate) {
        (1, TrafficRate::Full) => FrameContent::FchRc1_9600,
        (1, TrafficRate::Half) => FrameContent::FchRc1_4800,
        (1, TrafficRate::Quarter) => FrameContent::FchRc1_2400,
        (1, TrafficRate::Eighth) => FrameContent::FchRc1_1200,
        (_, TrafficRate::Full) => FrameContent::FchRc3_9600,
        (_, TrafficRate::Half) => FrameContent::FchRc3_4800,
        (_, TrafficRate::Quarter) => FrameContent::FchRc3_2700,
        (_, TrafficRate::Eighth) => FrameContent::FchRc3_1500,
    }
}

pub(crate) fn send_forward_fch_bits_with_bearer_client(
    bts_client: Option<&std::sync::Arc<dyn BtsControlClient>>,
    tx_frame_number: u32,
    walsh_code: u8,
    for_rc: u8,
    bits: Vec<u8>,
    rate: TrafficRate,
    queue: ForwardBearerQueue,
) -> Result<(), Error> {
    let bts_client = bts_client.ok_or("bts_client not configured for Abis bearer send")?;
    let bearer_client = bts_client
        .bearer_client()
        .ok_or("BTS peer has no Abis bearer client configured")?;
    bearer_client
        .send_frame(BearerFrame {
            channel_family: ChannelFamily::Fch,
            bearer_id: walsh_code as u32,
            tx_frame_number,
            traffic_frame: AbisTrafficFrame::ForwardFchDcch(ForwardFchDcchFrame {
                channel_family: ChannelFamily::Fch,
                fpc_slc: 1,
                fsn: 0,
                fpc_gr: 0,
                rpc_olt: 0,
                frame_content: encode_forward_bearer_rate(for_rc, rate),
                forward_link_information: bits,
                message_crc: 0,
            }),
            queue,
        })
        .map_err(|e| Error::from(format!("Abis bearer FCH send failed: {}", e)))
}

pub(crate) struct ReverseBearerMuxReaders {
    pub(crate) suffix_reader: DedicatedFrameReader,
    pub(crate) prefix_reader: DedicatedFrameReader,
    pub(crate) locked_layout: Option<ReverseMux1SignalingLayout>,
}

#[derive(Default)]
pub(crate) struct TrafficBearerService {
    pub(crate) next_tx_frame_number: u32,
    pub(crate) reverse_mux_readers: std::collections::HashMap<u8, ReverseBearerMuxReaders>,
}

impl TrafficBearerService {
    pub(crate) fn next_bearer_tx_frame_number(&mut self) -> u32 {
        let value = self.next_tx_frame_number;
        self.next_tx_frame_number = self.next_tx_frame_number.wrapping_add(1);
        value
    }
}

impl Default for ReverseBearerMuxReaders {
    fn default() -> Self {
        Self {
            suffix_reader: DedicatedFrameReader::new(),
            prefix_reader: DedicatedFrameReader::new(),
            locked_layout: None,
        }
    }
}

impl Bsc {
    pub(super) fn send_forward_fch_signaling_bits(
        &mut self,
        walsh_code: u8,
        bits: Vec<u8>,
    ) -> Result<(), Error> {
        self.send_forward_fch_bits(
            walsh_code,
            bits,
            TrafficRate::Full,
            ForwardBearerQueue::Signaling,
        )
    }

    pub(super) fn send_forward_fch_traffic_bits(
        &mut self,
        walsh_code: u8,
        bits: Vec<u8>,
        rate: TrafficRate,
    ) -> Result<(), Error> {
        self.send_forward_fch_bits(walsh_code, bits, rate, ForwardBearerQueue::Traffic)
    }

    pub(super) fn send_forward_sch_bits(
        &mut self,
        w32_code: u8,
        bits: Vec<u8>,
    ) -> Result<(), Error> {
        let tx_frame_number = self.traffic_bearer.next_bearer_tx_frame_number();
        let bts_client = self
            .config
            .bts_client
            .as_ref()
            .ok_or("bts_client not configured for Abis SCH bearer send")?;
        let bearer_client = bts_client
            .bearer_client()
            .ok_or("BTS peer has no Abis bearer client configured")?;
        bearer_client
            .send_frame(BearerFrame {
                channel_family: ChannelFamily::Sch,
                bearer_id: w32_code as u32,
                tx_frame_number,
                traffic_frame: AbisTrafficFrame::ForwardSch(ForwardSchFrame {
                    fpc_slc: 1,
                    fsn: 0,
                    fpc_gr: 0,
                    frame_content: FrameContent::Sch20msRc3_9600,
                    forward_link_information: bits,
                    message_crc: 0,
                }),
                queue: ForwardBearerQueue::Traffic,
            })
            .map_err(|e| Error::from(format!("Abis bearer SCH send failed: {}", e)))
    }

    fn send_forward_fch_bits(
        &mut self,
        walsh_code: u8,
        bits: Vec<u8>,
        rate: TrafficRate,
        queue: ForwardBearerQueue,
    ) -> Result<(), Error> {
        let tx_frame_number = self.traffic_bearer.next_bearer_tx_frame_number();
        let for_rc = self
            .mobiles
            .get_traffic_channel(walsh_code)
            .map(|tc| tc.for_rc)
            .unwrap_or(3);
        let result = send_forward_fch_bits_with_bearer_client(
            self.config.bts_client.as_ref(),
            tx_frame_number,
            walsh_code,
            for_rc,
            bits,
            rate,
            queue,
        );
        if result.is_ok() {
            self.mobiles.update_tc(walsh_code, |_, tc| {
                tc.last_forward_enqueue_at = Some(Instant::now());
            });
        }
        result
    }

    /// Drain reverse bearer frames from the BTS and dispatch them.
    ///
    /// The BTS tags each frame with IS-2001 Frame Content values. Reverse FCH
    /// payloads are raw air-interface information bits; the BSC parses MUX1
    /// headers and performs dedicated signaling SAR reassembly here.
    pub async fn poll_reverse_bearer_once(&mut self) {
        self.poll_reverse_bearer_preambles().await;
    }

    pub(super) async fn poll_reverse_bearer_preambles(&mut self) {
        let frames = match self.config.bts_client.as_ref() {
            Some(client) => match client.bearer_client() {
                Some(bearer) => bearer.drain_received_frames(),
                None => return,
            },
            None => return,
        };
        for frame in frames {
            if let AbisTrafficFrame::ReverseFchDcch(ref fch) = frame.traffic_frame {
                let walsh_code = frame.bearer_id as u8;
                if fch.frame_content == REVERSE_FRAME_CONTENT_NULL {
                    info!(
                        "BSC: reverse bearer preamble null frame walsh={}",
                        walsh_code
                    );
                    let event = Self::bearer_null_frame_to_preamble_event(
                        walsh_code,
                        frame.tx_frame_number,
                    );
                    self.handle_access_event(event).await;
                } else if fch.fqi {
                    self.route_reverse_bearer_packet_primary(walsh_code, fch);
                    if let Some(event) = Self::bearer_reverse_primary_to_event(
                        walsh_code,
                        fch,
                        frame.tx_frame_number,
                    ) {
                        debug!(
                            "BSC: reverse bearer primary walsh={} rate={:?} bits={}",
                            walsh_code,
                            event.traffic_primary_rate_bps,
                            event
                                .traffic_primary_bits
                                .as_ref()
                                .map(|bits| bits.len())
                                .unwrap_or(0),
                        );
                        self.handle_access_event(event).await;
                    }

                    match self.bearer_reverse_frame_to_event(walsh_code, fch, frame.tx_frame_number)
                    {
                        Some(event) => {
                            info!(
                                "BSC: reverse bearer traffic walsh={} msg={}",
                                walsh_code, event.pdu_summary
                            );
                            self.events.publish_access_event(event.clone());
                            self.handle_access_event(event).await;
                        }
                        None => {
                            debug!(
                                "BSC: reverse bearer walsh={} frame_content=0x{:02X} rate={:?} bits={} produced no R-DSCH PDU",
                                walsh_code,
                                fch.frame_content.value(),
                                fch.frame_content.rate_bps(),
                                fch.reverse_link_information.len(),
                            );
                        }
                    }
                } else {
                    self.route_reverse_bearer_packet_primary(walsh_code, fch);
                    debug!(
                        "BSC: reverse bearer voice/data walsh={} frame_content=0x{:02X} bits={}",
                        walsh_code,
                        fch.frame_content.value(),
                        fch.reverse_link_information.len(),
                    );
                }
            }
        }
    }

    pub(crate) fn route_reverse_bearer_packet_primary(
        &mut self,
        walsh_code: u8,
        fch: &ReverseFchDcchFrame,
    ) -> bool {
        let Some((primary_bits, primary_rate_bps)) = Self::extract_reverse_bearer_primary_bits(fch)
        else {
            return false;
        };

        let outcome = self.mobiles.update_tc(walsh_code, |_, tc| {
            if !is_packet_data_so(tc.service_option) {
                return None;
            }
            tc.push_primary_frame(&primary_bits, primary_rate_bps);
            let uplink_tx = tc.packet_uplink_tx.as_ref()?.clone();
            let session_id = tc.packet_session_id.clone()?;
            Some((uplink_tx, session_id))
        });
        let Some(Some((uplink_tx, session_id))) = outcome else {
            debug!(
                "BSC: reverse bearer packet primary walsh={} dropped (no packet session/uplink)",
                walsh_code
            );
            return false;
        };
        let num_bits = primary_bits.len() as u32;
        let frame = crate::packet::PacketBearerFrame {
            session_id,
            bits: primary_bits,
            num_bits,
            rate_bps: primary_rate_bps,
        };
        match uplink_tx.try_send(frame) {
            Ok(()) => {
                debug!(
                    "BSC: reverse bearer packet primary walsh={} rate={} bits={}",
                    walsh_code, primary_rate_bps, num_bits
                );
                true
            }
            Err(e) => {
                warn!(
                    "BSC: reverse bearer packet primary walsh={} rate={} enqueue failed: {}",
                    walsh_code, primary_rate_bps, e
                );
                false
            }
        }
    }

    fn extract_reverse_bearer_primary_bits(fch: &ReverseFchDcchFrame) -> Option<(Vec<u8>, u32)> {
        let expected_info_bits = fch.frame_content.information_bits();
        if expected_info_bits == 0 || fch.reverse_link_information.len() < expected_info_bits {
            return None;
        }

        let info = &fch.reverse_link_information[..expected_info_bits];
        match fch.frame_content.rate_bps()? {
            9600 => {
                let format = parse_reverse_mux1_full_rate_format(info)?;
                if format.primary_bits == 0 {
                    return None;
                }
                let primary_rate_bps = match format.primary_bits {
                    171 => 9600,
                    80 => 4800,
                    40 => 2700,
                    16 => 1500,
                    _ => return None,
                };
                let primary_start = format.header_bits;
                let primary_end = primary_start + format.primary_bits;
                if primary_end > info.len() {
                    return None;
                }
                Some((info[primary_start..primary_end].to_vec(), primary_rate_bps))
            }
            rate_bps @ (4800 | 2700 | 2400 | 1500 | 1200) => Some((info.to_vec(), rate_bps)),
            _ => None,
        }
    }

    pub(crate) fn bearer_reverse_primary_to_event(
        walsh_code: u8,
        fch: &ReverseFchDcchFrame,
        tx_frame_number: u32,
    ) -> Option<AccessChannelEvent> {
        let (primary_bits, rate_bps) = Self::extract_reverse_bearer_primary_bits(fch)?;
        let now = chrono::Utc::now();

        Some(AccessChannelEvent {
            event_id: Uuid::new_v4().to_string(),
            chip_start: 0,
            absolute_chip_start: Some(tx_frame_number as u64),
            receive_time: Some(cdma_common::time::CdmaSystemTime::from(now)),
            preamble_frames: 0,
            pd: 0,
            message_id: MessageId::GeneralExtension,
            msg_type_name: format!("TrafficPrimaryFrame({}bps)", rate_bps),
            address: None,
            resolved_address: None,
            subscriber_id: None,
            l3_summary: None,
            decoded_l3: None,
            pdu_summary: format!(
                "primary_frame walsh={} rate_bps={} bearer=true",
                walsh_code, rate_bps
            ),
            msg_seq: None,
            ack_seq: None,
            ack_req: false,
            valid_ack: false,
            msid_type: None,
            esn: None,
            imsi: None,
            imsi_m_s1: None,
            imsi_m_s2: None,
            imsi_class: None,
            imsi_addr_num: None,
            imsi_mcc: None,
            imsi_11_12: None,
            mob_p_rev: None,
            slot_cycle_index: None,
            scm: None,
            burst_type: None,
            data_burst_fields: None,
            data_burst_num_msgs: None,
            data_burst_msg_number: None,
            wall_clock_us: now.timestamp_micros() as u64,
            rx_wall_time: Some(Instant::now()),
            rx_hw_time_ns: None,
            snr_db: None,
            signal_power_db: None,
            reverse_pilot_ec_io_db: None,
            raw_power_db: None,
            demod_quality_pct: None,
            pcg_signal_snr_db: None,
            active_pcg_mask: None,
            traffic_phy_valid: Some(true),
            traffic_fqi_valid: Some(true),
            traffic_tail_valid: None,
            traffic_fqi_bits: None,
            traffic_ml_tail_match: None,
            order_code: None,
            service_option: None,
            for_rc_pref: None,
            rev_rc_pref: None,
            rev_fch_gating_req: None,
            traffic_walsh_code: Some(walsh_code),
            is_preamble_only: false,
            is_traffic_pcg_measurement: false,
            is_traffic_phy_status: false,
            traffic_measurement_age_chips: None,
            for_supported_rcs: Vec::new(),
            rev_supported_rcs: Vec::new(),
            decoded_rdsch: None,
            traffic_primary_bits: Some(primary_bits.clone()),
            traffic_primary_rate_bps: Some(rate_bps),
            traffic_primary_bearer_routed: true,
            traffic_voice_bits: Some(primary_bits),
            traffic_voice_rate_bps: Some(rate_bps),
            raw_pdu_bits: None,
        })
    }

    fn bearer_reverse_frame_to_event(
        &mut self,
        walsh_code: u8,
        fch: &ReverseFchDcchFrame,
        tx_frame_number: u32,
    ) -> Option<AccessChannelEvent> {
        let expected_info_bits = fch.frame_content.information_bits();
        if expected_info_bits == 0 {
            return None;
        }
        if fch.reverse_link_information.len() < expected_info_bits {
            warn!(
                "BSC: reverse bearer walsh={} frame_content=0x{:02X} short info bits: have {}, need {}",
                walsh_code,
                fch.frame_content.value(),
                fch.reverse_link_information.len(),
                expected_info_bits
            );
            return None;
        }

        let info = &fch.reverse_link_information[..expected_info_bits];
        let format = parse_reverse_mux1_full_rate_format(info)?;
        debug!(
            "BSC: reverse bearer MUX1 walsh={} frame_content=0x{:02X} mux_header=0b{:04b} primary_bits={} signaling_bits={}",
            walsh_code,
            fch.frame_content.value(),
            format.mux_header,
            format.primary_bits,
            format.signaling_bits,
        );
        if format.signaling_bits == 0 {
            return None;
        }

        let readers = self
            .traffic_bearer
            .reverse_mux_readers
            .entry(walsh_code)
            .or_default();
        let layouts_to_try = match readers.locked_layout {
            Some(ReverseMux1SignalingLayout::Prefix) => [
                ReverseMux1SignalingLayout::Prefix,
                ReverseMux1SignalingLayout::Suffix,
            ],
            _ => [
                ReverseMux1SignalingLayout::Suffix,
                ReverseMux1SignalingLayout::Prefix,
            ],
        };

        for layout in layouts_to_try {
            if let Some(locked) = readers.locked_layout
                && layout != locked
            {
                continue;
            }
            let Some(signaling) = extract_reverse_mux1_full_rate_signaling_block(info, layout)
            else {
                continue;
            };
            let reader = match layout {
                ReverseMux1SignalingLayout::Suffix => &mut readers.suffix_reader,
                ReverseMux1SignalingLayout::Prefix => &mut readers.prefix_reader,
            };
            let mut bits = Bitstream::new_init(&signaling.bits);
            let frame = match reader.process(&mut bits) {
                Ok(Some(frame)) => frame,
                Ok(None) => continue,
                Err(e) => {
                    warn!(
                        "BSC: reverse bearer MUX/SAR decode failed walsh={} layout={:?}: {}",
                        walsh_code, layout, e
                    );
                    continue;
                }
            };
            if !frame.crc_valid {
                debug!(
                    "BSC: reverse bearer R-DSCH CRC invalid walsh={} layout={:?} msg_len={}",
                    walsh_code, layout, frame.msg_length_octets
                );
                continue;
            }

            readers.locked_layout = Some(layout);
            match layout {
                ReverseMux1SignalingLayout::Suffix => readers.prefix_reader.reset(),
                ReverseMux1SignalingLayout::Prefix => readers.suffix_reader.reset(),
            }
            return Self::bearer_reverse_signaling_to_event(
                walsh_code,
                frame.data.bits(),
                tx_frame_number,
            );
        }

        None
    }

    /// Decode a post-SAR r-dsch signaling PDU from the Abis bearer.
    ///
    /// The BTS has already performed MUX1 parsing, signaling extraction, and
    /// `DedicatedFrameReader` SAR reassembly with CRC-16 validation. The
    /// `bits` here are the reassembled LAC PDU payload (MSG_TYPE + ARQ + L3),
    /// ready for direct `RdschPdu::decode`.
    fn bearer_reverse_signaling_to_event(
        walsh_code: u8,
        bits: &[u8],
        tx_frame_number: u32,
    ) -> Option<AccessChannelEvent> {
        let bs = Bitstream::new_init(bits);
        let rdsch = match RdschPdu::decode(&bs) {
            Ok(pdu) => pdu,
            Err(e) => {
                warn!(
                    "BSC: failed to decode reverse bearer R-DSCH walsh={}: {}",
                    walsh_code, e
                );
                return None;
            }
        };

        let order_code = rdsch.l3.order_code();
        let data_burst_info = rdsch
            .l3
            .data_burst_fields()
            .map(|(bt, mn, nm, f)| (bt, mn, nm, f.to_vec()));
        let decoded_l3 = Some(rdsch.l3.clone());
        let l3_summary = Some(rdsch.l3.summary());
        let pdu_summary = rdsch.summary();
        let now = chrono::Utc::now();

        Some(AccessChannelEvent {
            event_id: Uuid::new_v4().to_string(),
            chip_start: 0,
            absolute_chip_start: Some(tx_frame_number as u64),
            receive_time: Some(cdma_common::time::CdmaSystemTime::from(now)),
            preamble_frames: 0,
            pd: 0,
            message_id: rdsch.message_id,
            msg_type_name: rdsch.msg_type_name().to_string(),
            address: None,
            resolved_address: None,
            subscriber_id: None,
            l3_summary,
            decoded_l3,
            pdu_summary,
            msg_seq: Some(rdsch.arq.msg_seq),
            ack_seq: Some(rdsch.arq.ack_seq),
            ack_req: rdsch.arq.ack_req,
            valid_ack: true,
            msid_type: None,
            esn: None,
            imsi: None,
            imsi_m_s1: None,
            imsi_m_s2: None,
            imsi_class: None,
            imsi_addr_num: None,
            imsi_mcc: None,
            imsi_11_12: None,
            mob_p_rev: None,
            slot_cycle_index: None,
            scm: None,
            burst_type: data_burst_info.as_ref().map(|(bt, _, _, _)| *bt),
            data_burst_fields: data_burst_info.as_ref().map(|(_, _, _, f)| f.clone()),
            data_burst_num_msgs: data_burst_info.as_ref().map(|(_, _, nm, _)| *nm),
            data_burst_msg_number: data_burst_info.as_ref().map(|(_, mn, _, _)| *mn),
            wall_clock_us: now.timestamp_micros() as u64,
            rx_wall_time: Some(Instant::now()),
            rx_hw_time_ns: None,
            snr_db: None,
            signal_power_db: None,
            reverse_pilot_ec_io_db: None,
            raw_power_db: None,
            demod_quality_pct: None,
            pcg_signal_snr_db: None,
            active_pcg_mask: None,
            traffic_phy_valid: Some(true),
            traffic_fqi_valid: Some(true),
            traffic_tail_valid: None,
            traffic_fqi_bits: None,
            traffic_ml_tail_match: None,
            order_code,
            service_option: None,
            for_rc_pref: None,
            rev_rc_pref: None,
            rev_fch_gating_req: None,
            traffic_walsh_code: Some(walsh_code),
            is_preamble_only: false,
            is_traffic_pcg_measurement: false,
            is_traffic_phy_status: false,
            traffic_measurement_age_chips: None,
            for_supported_rcs: Vec::new(),
            rev_supported_rcs: Vec::new(),
            decoded_rdsch: Some(rdsch),
            traffic_primary_bits: None,
            traffic_primary_rate_bps: None,
            traffic_primary_bearer_routed: true,
            traffic_voice_bits: None,
            traffic_voice_rate_bps: None,
            raw_pdu_bits: None,
        })
    }

    fn bearer_null_frame_to_preamble_event(
        walsh_code: u8,
        tx_frame_number: u32,
    ) -> AccessChannelEvent {
        let now = chrono::Utc::now();
        AccessChannelEvent {
            event_id: Uuid::new_v4().to_string(),
            chip_start: 0,
            absolute_chip_start: Some(tx_frame_number as u64),
            receive_time: Some(cdma_common::time::CdmaSystemTime::from(now)),
            preamble_frames: 0,
            pd: 0,
            message_id: MessageId::GeneralExtension,
            msg_type_name: "Preamble".to_string(),
            address: None,
            resolved_address: None,
            subscriber_id: None,
            l3_summary: None,
            decoded_l3: None,
            pdu_summary: String::new(),
            msg_seq: None,
            ack_seq: None,
            ack_req: false,
            valid_ack: false,
            msid_type: None,
            esn: None,
            imsi: None,
            imsi_m_s1: None,
            imsi_m_s2: None,
            imsi_class: None,
            imsi_addr_num: None,
            imsi_mcc: None,
            imsi_11_12: None,
            mob_p_rev: None,
            slot_cycle_index: None,
            scm: None,
            burst_type: None,
            data_burst_fields: None,
            data_burst_num_msgs: None,
            data_burst_msg_number: None,
            wall_clock_us: now.timestamp_micros() as u64,
            rx_wall_time: Some(Instant::now()),
            rx_hw_time_ns: None,
            snr_db: None,
            signal_power_db: None,
            reverse_pilot_ec_io_db: None,
            raw_power_db: None,
            demod_quality_pct: None,
            pcg_signal_snr_db: None,
            active_pcg_mask: None,
            traffic_phy_valid: None,
            traffic_fqi_valid: None,
            traffic_tail_valid: None,
            traffic_fqi_bits: None,
            traffic_ml_tail_match: None,
            order_code: None,
            service_option: None,
            for_rc_pref: None,
            rev_rc_pref: None,
            rev_fch_gating_req: None,
            traffic_walsh_code: Some(walsh_code),
            is_preamble_only: true,
            is_traffic_pcg_measurement: false,
            is_traffic_phy_status: false,
            traffic_measurement_age_chips: None,
            for_supported_rcs: Vec::new(),
            rev_supported_rcs: Vec::new(),
            decoded_rdsch: None,
            traffic_primary_bits: None,
            traffic_primary_rate_bps: None,
            traffic_primary_bearer_routed: true,
            traffic_voice_bits: None,
            traffic_voice_rate_bps: None,
            raw_pdu_bits: None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rc1_forward_bearer_rates_use_rc1_frame_content() {
        assert_eq!(
            encode_forward_bearer_rate(1, TrafficRate::Full),
            FrameContent::FchRc1_9600
        );
        assert_eq!(
            encode_forward_bearer_rate(1, TrafficRate::Half),
            FrameContent::FchRc1_4800
        );
        assert_eq!(
            encode_forward_bearer_rate(1, TrafficRate::Quarter),
            FrameContent::FchRc1_2400
        );
        assert_eq!(
            encode_forward_bearer_rate(1, TrafficRate::Eighth),
            FrameContent::FchRc1_1200
        );
    }

    #[test]
    fn rc3_forward_bearer_rates_keep_rc3_frame_content() {
        assert_eq!(
            encode_forward_bearer_rate(3, TrafficRate::Full),
            FrameContent::FchRc3_9600
        );
        assert_eq!(
            encode_forward_bearer_rate(3, TrafficRate::Half),
            FrameContent::FchRc3_4800
        );
        assert_eq!(
            encode_forward_bearer_rate(3, TrafficRate::Quarter),
            FrameContent::FchRc3_2700
        );
        assert_eq!(
            encode_forward_bearer_rate(3, TrafficRate::Eighth),
            FrameContent::FchRc3_1500
        );
    }
}
