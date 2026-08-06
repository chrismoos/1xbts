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
    access::{AccessFrame, DedicatedFrameReader},
    pipelined::{
        ReverseMux1SignalingLayout, extract_reverse_mux1_full_rate_signaling_block,
        extract_reverse_mux2_signaling_block, parse_reverse_mux1_full_rate_format,
        parse_reverse_mux2_format,
    },
};
use cdma_common::access::RdschPdu;
use cdma_common::channel::TrafficRate;
use cdma_common::events::AccessChannelEvent;
use cdma_common::lac::message_types::MessageId;
use cdma_common::{bits::Bitstream, error::Error};
use cdma_voice::{SAMPLES_PER_FRAME, VoiceCodec, VoiceEncoder};
use log::{debug, info, trace, warn};
use tokio::sync::mpsc::error::TrySendError;
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
        (2, TrafficRate::Full) => FrameContent::FchRc2_14400,
        (2, TrafficRate::Half) => FrameContent::FchRc2_7200,
        (2, TrafficRate::Quarter) => FrameContent::FchRc2_3600,
        (2, TrafficRate::Eighth) => FrameContent::FchRc2_1800,
        (_, TrafficRate::Full) => FrameContent::FchRc3_9600,
        (_, TrafficRate::Half) => FrameContent::FchRc3_4800,
        (_, TrafficRate::Quarter) => FrameContent::FchRc3_2700,
        (_, TrafficRate::Eighth) => FrameContent::FchRc3_1500,
    }
}

/// Map an RC3 SCH downlink rate (in bps) to the BTS-side `FrameContent` tag.
fn encode_sch_bearer_rate(rate_bps: u32) -> Result<FrameContent, Error> {
    match rate_bps {
        153_600 => Ok(FrameContent::Sch20msRc3_153600),
        76_800 => Ok(FrameContent::Sch20msRc3_76800),
        38_400 => Ok(FrameContent::Sch20msRc3_38400),
        19_200 => Ok(FrameContent::Sch20msRc3_19200),
        9_600 => Ok(FrameContent::Sch20msRc3_9600),
        other => Err(Error::from(format!(
            "unsupported RC3 F-SCH bearer rate {} bps",
            other
        ))),
    }
}

/// Free-function variant of `send_forward_sch_bits` callable from a spawned
/// task that doesn't hold `&mut Bsc`. Used by the SO33 packet downlink task
/// to deliver rate-specific SCH frames to the BTS bearer without going back
/// through the BSC's `&mut self` API surface.
pub(crate) fn send_forward_sch_bits_with_bearer_client(
    bts_client: Option<&std::sync::Arc<dyn BtsControlClient>>,
    tx_frame_number: u32,
    sch_code: u8,
    rate_bps: u32,
    bits: Vec<u8>,
) -> Result<(), Error> {
    let bts_client = bts_client.ok_or("bts_client not configured for Abis SCH bearer send")?;
    let bearer_client = bts_client
        .bearer_client()
        .ok_or("BTS peer has no Abis bearer client configured")?;
    let frame_content = encode_sch_bearer_rate(rate_bps)?;
    bearer_client
        .send_frame(BearerFrame {
            channel_family: ChannelFamily::Sch,
            bearer_id: sch_code as u32,
            tx_frame_number,
            traffic_frame: AbisTrafficFrame::ForwardSch(ForwardSchFrame {
                fpc_slc: 1,
                fsn: 0,
                fpc_gr: 0,
                frame_content,
                forward_link_information: bits,
                message_crc: 0,
            }),
            queue: ForwardBearerQueue::Traffic,
        })
        .map_err(|e| Error::from(format!("Abis bearer SCH send failed: {}", e)))
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
    pub(crate) reverse_voice_silence_encoders:
        std::collections::HashMap<u8, std::sync::Mutex<(VoiceCodec, VoiceEncoder)>>,
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
    fn is_reverse_mux2_frame(frame_content: FrameContent) -> bool {
        matches!(
            frame_content,
            FrameContent::FchRc2_14400
                | FrameContent::FchRc2_7200
                | FrameContent::FchRc2_3600
                | FrameContent::FchRc2_1800
        )
    }

    fn decode_reverse_mux2_bearer_frame(
        readers: &mut ReverseBearerMuxReaders,
        walsh_code: u8,
        frame_content: FrameContent,
        info: &[u8],
    ) -> Option<AccessFrame> {
        let format = parse_reverse_mux2_format(info)?;
        debug!(
            "BSC: reverse bearer MUX2 walsh={} frame_content=0x{:02X} mux_header=0x{:X} header_bits={} primary_bits={} signaling_bits={}",
            walsh_code,
            frame_content.value(),
            format.mux_header,
            format.header_bits,
            format.primary_bits,
            format.signaling_bits,
        );
        let signaling = extract_reverse_mux2_signaling_block(info)?;
        let mut bits = Bitstream::new_init(&signaling.bits);
        let frame = match readers.suffix_reader.process(&mut bits) {
            Ok(Some(frame)) => frame,
            Ok(None) => return None,
            Err(e) => {
                warn!(
                    "BSC: reverse bearer MUX2/SAR decode failed walsh={}: {}",
                    walsh_code, e
                );
                return None;
            }
        };
        if !frame.crc_valid {
            debug!(
                "BSC: reverse bearer R-DSCH CRC invalid walsh={} mux=2 msg_len={}",
                walsh_code, frame.msg_length_octets
            );
            return None;
        }
        readers.prefix_reader.reset();
        readers.locked_layout = Some(ReverseMux1SignalingLayout::Suffix);
        Some(frame)
    }

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
                    self.route_reverse_bearer_packet_primary(walsh_code, fch)
                        .await;
                    if let Some(event) = Self::bearer_reverse_primary_to_event(
                        walsh_code,
                        fch,
                        frame.tx_frame_number,
                    ) {
                        log::trace!(
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
                            log::trace!(
                                "BSC: reverse bearer walsh={} frame_content=0x{:02X} rate={:?} bits={} produced no R-DSCH PDU",
                                walsh_code,
                                fch.frame_content.value(),
                                fch.frame_content.rate_bps(),
                                fch.reverse_link_information.len(),
                            );
                        }
                    }
                } else {
                    self.route_reverse_bearer_packet_primary(walsh_code, fch)
                        .await;
                    if let Err(error) = self.relay_reverse_silence_to_msc(walsh_code) {
                        debug!(
                            "BSC: bad reverse frame on walsh={} had no silence substitution: {}",
                            walsh_code, error
                        );
                    }
                    log::trace!(
                        "BSC: reverse bearer voice/data walsh={} frame_content=0x{:02X} bits={}",
                        walsh_code,
                        fch.frame_content.value(),
                        fch.reverse_link_information.len(),
                    );
                }
            }
        }
    }

    pub(crate) fn relay_reverse_silence_to_msc(&mut self, walsh_code: u8) -> Result<(), String> {
        if !self.reverse_voice_media_enabled(walsh_code) {
            return Err("reverse MSC voice media is not active".to_string());
        }
        let Some(bearer) = self.config.msc_voice_bearer.clone() else {
            return Err("MSC voice bearer is not configured".to_string());
        };
        let Some((circuit_id, service_option)) =
            self.mobiles.get_traffic_channel(walsh_code).and_then(|tc| {
                let service_option = super::traffic_forward::voice_service_option_for_channel(tc)?;
                Some((tc.msc_circuit_id?, service_option))
            })
        else {
            return Err("traffic channel has no MSC voice circuit".to_string());
        };
        let Some(codec) = VoiceCodec::from_service_option(service_option) else {
            return Err(format!("service option {} is not voice", service_option));
        };

        let encoder_state = match self
            .traffic_bearer
            .reverse_voice_silence_encoders
            .entry(walsh_code)
        {
            std::collections::hash_map::Entry::Occupied(entry) => entry.into_mut(),
            std::collections::hash_map::Entry::Vacant(entry) => {
                let encoder = VoiceEncoder::new(codec)?;
                entry.insert(std::sync::Mutex::new((codec, encoder)))
            }
        };
        let encoder_state = encoder_state
            .get_mut()
            .map_err(|_| "reverse silence encoder state is poisoned".to_string())?;
        if encoder_state.0 != codec {
            let encoder = VoiceEncoder::new(codec)?;
            *encoder_state = (codec, encoder);
        }

        let silence = [0i16; SAMPLES_PER_FRAME];
        let (rate, payload) = encoder_state.1.encode(&silence)?;
        let frame = cdma_ios::VoiceBearerFrame {
            circuit_id,
            rate_bps: codec.rate_bps(rate),
            payload,
        };
        bearer
            .try_send_frame(&frame)
            .map_err(|error| format!("MSC circuit_id={} send failed: {}", circuit_id, error))?;
        debug!(
            "BSC: replaced bad reverse voice frame with silence walsh={} circuit_id={}",
            walsh_code, circuit_id
        );
        Ok(())
    }

    pub(crate) async fn route_reverse_bearer_packet_primary(
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
            session_id: session_id.clone(),
            bits: primary_bits,
            num_bits,
            rate_bps: primary_rate_bps,
        };
        match uplink_tx.try_send(frame) {
            Ok(()) => {
                trace!(
                    "BSC: reverse bearer packet primary walsh={} rate={} bits={}",
                    walsh_code, primary_rate_bps, num_bits
                );
                true
            }
            Err(TrySendError::Closed(_)) => {
                let detached = self
                    .mobiles
                    .update_tc(walsh_code, |_, tc| {
                        if tc.packet_session_id.as_deref() != Some(session_id.as_str()) {
                            return false;
                        }
                        tc.packet_session_id = None;
                        tc.packet_uplink_tx = None;
                        if let Some(task) = tc.packet_downlink_task.take() {
                            task.abort();
                        }
                        true
                    })
                    .unwrap_or(false);
                if detached {
                    info!(
                        "BSC: detached closed packet session {} on walsh={}, initiating traffic release",
                        session_id, walsh_code
                    );
                    self.begin_packet_tch_release(walsh_code, "closed packet session");
                }
                false
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
        if Self::is_reverse_mux2_frame(fch.frame_content) {
            let format = parse_reverse_mux2_format(info)?;
            if format.primary_bits == 0 {
                return None;
            }
            let primary_rate_bps = match format.primary_bits {
                266 => 14_400,
                124 => 7_200,
                54 => 3_600,
                20 => 1_800,
                _ => return None,
            };
            let primary_start = format.header_bits;
            let primary_end = primary_start + format.primary_bits;
            return Some((info[primary_start..primary_end].to_vec(), primary_rate_bps));
        }

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
            meid: None,
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
        let readers = self
            .traffic_bearer
            .reverse_mux_readers
            .entry(walsh_code)
            .or_default();
        if Self::is_reverse_mux2_frame(fch.frame_content) {
            let frame = Self::decode_reverse_mux2_bearer_frame(
                readers,
                walsh_code,
                fch.frame_content,
                info,
            )?;
            return Self::bearer_reverse_signaling_to_event(
                walsh_code,
                frame.data.bits(),
                tx_frame_number,
            );
        }

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
            meid: None,
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
            meid: None,
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

    fn reverse_fch(frame_content: FrameContent, information: Vec<u8>) -> ReverseFchDcchFrame {
        ReverseFchDcchFrame {
            channel_family: ChannelFamily::Fch,
            soft_handoff_leg: 0,
            fsn: 0,
            fqi: true,
            reverse_link_quality: 0,
            scaling: 0,
            packet_arrival_time_error: 0,
            frame_content,
            fpc_s: 0,
            eib: false,
            reverse_link_information: information,
            message_crc: 0,
        }
    }

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
    fn rc2_forward_bearer_rates_use_rc2_frame_content() {
        assert_eq!(
            encode_forward_bearer_rate(2, TrafficRate::Full),
            FrameContent::FchRc2_14400
        );
        assert_eq!(
            encode_forward_bearer_rate(2, TrafficRate::Half),
            FrameContent::FchRc2_7200
        );
        assert_eq!(
            encode_forward_bearer_rate(2, TrafficRate::Quarter),
            FrameContent::FchRc2_3600
        );
        assert_eq!(
            encode_forward_bearer_rate(2, TrafficRate::Eighth),
            FrameContent::FchRc2_1800
        );
    }

    #[test]
    fn rc2_reverse_bearer_extracts_primary_at_all_rates() {
        let cases = [
            (FrameContent::FchRc2_14400, vec![0], 266, 14_400),
            (FrameContent::FchRc2_7200, vec![0], 124, 7_200),
            (FrameContent::FchRc2_3600, vec![0], 54, 3_600),
            (FrameContent::FchRc2_1800, vec![0], 20, 1_800),
        ];

        for (frame_content, header, primary_bits, primary_rate_bps) in cases {
            let mut information = header;
            information.extend(std::iter::repeat_n(1, primary_bits));
            information.resize(frame_content.information_bits(), 0);
            let fch = reverse_fch(frame_content, information);

            let (primary, rate) =
                Bsc::extract_reverse_bearer_primary_bits(&fch).expect("RC2 primary traffic");
            assert_eq!(primary, vec![1; primary_bits]);
            assert_eq!(rate, primary_rate_bps);
        }
    }

    #[test]
    fn extracted_voice_primary_keeps_leading_codec_bit_when_packed_for_a2p() {
        let cases = [
            (FrameContent::FchRc3_9600, 171usize, 9_600),
            (FrameContent::FchRc2_14400, 266, 14_400),
            (FrameContent::FchRc2_7200, 124, 7_200),
            (FrameContent::FchRc2_3600, 54, 3_600),
            (FrameContent::FchRc2_1800, 20, 1_800),
        ];

        for (frame_content, primary_bits, primary_rate_bps) in cases {
            let primary = (0..primary_bits)
                .map(|index| u8::from(index % 3 == 1))
                .collect::<Vec<_>>();
            assert_eq!(primary[0], 0);

            let mut information = vec![0];
            information.extend_from_slice(&primary);
            information.resize(frame_content.information_bits(), 0);
            let fch = reverse_fch(frame_content, information);

            let (extracted, rate) =
                Bsc::extract_reverse_bearer_primary_bits(&fch).expect("primary traffic");
            assert_eq!(extracted, primary);
            assert_eq!(rate, primary_rate_bps);

            let packed =
                crate::voice_bearer_bits::pack_voice_bits_for_bearer(&extracted, rate).unwrap();
            assert_eq!(
                cdma_voice::unpack_voice_bits(&packed, primary_bits),
                primary
            );
        }
    }

    #[test]
    fn rc2_half_rate_reverse_bearer_reassembles_signaling() {
        let mut pdu = Bitstream::new();
        pdu.write_u8(0x01, 8);
        let full_rate_frames = cdma_bts::lac::sar_fragment_ftch_pdu_dsch_rc2(&pdu);
        let full_rate_bits = full_rate_frames[0].bits();

        let mut half_rate_bits = vec![1, 0, 0, 0];
        half_rate_bits.extend(std::iter::repeat_n(0, 54));
        half_rate_bits.extend_from_slice(&full_rate_bits[5..5 + 67]);

        let mut readers = ReverseBearerMuxReaders::default();
        let frame = Bsc::decode_reverse_mux2_bearer_frame(
            &mut readers,
            12,
            FrameContent::FchRc2_7200,
            &half_rate_bits,
        )
        .expect("half-rate R-DSCH frame");

        assert!(frame.crc_valid);
        assert_eq!(frame.data.bits(), pdu.bits());
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

    #[test]
    fn encode_sch_bearer_rate_supports_configurable_rc3_rates() {
        assert_eq!(
            encode_sch_bearer_rate(19_200).unwrap(),
            FrameContent::Sch20msRc3_19200
        );
        assert_eq!(
            encode_sch_bearer_rate(38_400).unwrap(),
            FrameContent::Sch20msRc3_38400
        );
        assert_eq!(
            encode_sch_bearer_rate(76_800).unwrap(),
            FrameContent::Sch20msRc3_76800
        );
        assert_eq!(
            encode_sch_bearer_rate(153_600).unwrap(),
            FrameContent::Sch20msRc3_153600
        );
    }

    #[test]
    fn encode_sch_bearer_rate_rejects_unsupported_rc3_rates() {
        assert!(encode_sch_bearer_rate(307_200).is_err());
    }
}
