use cdma_common::bits::Bitstream;
use cdma_common::crc::crc12;
use log::{debug, info, warn};
use num_complex::Complex32;

use crate::receiver::access::{AccessFrame, DedicatedFrameReader};

use super::{PipelineProcessor, SampleBlock, chips_per_sample};

/// RC1 traffic channel frame configuration.
/// Full rate 9600 bps: 172 information + 12 FQI (CRC) + 8 tail = 192 bits.
/// Per C.S0002-E 2.1.3.12.1.1, Table 2.1.3.12.1.1-1.
pub const RC1_FRAME_CONFIG: TrafficFrameConfig = TrafficFrameConfig {
    frame_bits: 192,
    info_bits: 172,
    fqi_bits: 12,
    tail_bits: 8,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReverseMux1SignalingLayout {
    Suffix = 0,
    Prefix = 1,
}

impl ReverseMux1SignalingLayout {
    pub const SEARCH_ORDER: [Self; 2] = [Self::Suffix, Self::Prefix];

    pub fn from_tag(value: i64) -> Self {
        match value {
            1 => Self::Prefix,
            _ => Self::Suffix,
        }
    }

    pub fn tag_value(self) -> i64 {
        self as i64
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReverseMux1FullRateFormat {
    pub mux_header: u8,
    pub header_bits: usize,
    pub primary_bits: usize,
    pub signaling_bits: usize,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReverseMux1SignalingBlock {
    pub mux_header: u8,
    pub header_bits: usize,
    pub primary_bits: usize,
    pub signaling_bits: usize,
    pub bits: Vec<u8>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ReverseMux2Format {
    pub mux_header: u8,
    pub header_bits: usize,
    pub primary_bits: usize,
    pub signaling_bits: usize,
}

pub type ReverseMux2FullRateFormat = ReverseMux2Format;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReverseMux2SignalingBlock {
    pub mux_header: u8,
    pub header_bits: usize,
    pub primary_bits: usize,
    pub signaling_bits: usize,
    pub bits: Vec<u8>,
}

pub fn parse_reverse_mux1_full_rate_format(info_bits: &[u8]) -> Option<ReverseMux1FullRateFormat> {
    if info_bits.len() < RC1_FRAME_CONFIG.info_bits {
        return None;
    }

    if info_bits[0] == 0 {
        return Some(ReverseMux1FullRateFormat {
            mux_header: 0,
            header_bits: 1,
            primary_bits: 171,
            signaling_bits: 0,
        });
    }

    let mux_header = ((info_bits[0] & 1) << 3)
        | ((info_bits[1] & 1) << 2)
        | ((info_bits[2] & 1) << 1)
        | (info_bits[3] & 1);
    let (primary_bits, signaling_bits) = match mux_header {
        0b1000 => (80, 88),
        0b1001 => (40, 128),
        0b1010 => (16, 152),
        0b1011 => (0, 168),
        0b1100 => (80, 0),
        0b1101 => (40, 0),
        0b1110 => (16, 0),
        0b1111 => (0, 0),
        _ => return None,
    };

    Some(ReverseMux1FullRateFormat {
        mux_header,
        header_bits: 4,
        primary_bits,
        signaling_bits,
    })
}

pub fn extract_reverse_mux1_full_rate_signaling_block(
    info_bits: &[u8],
    layout: ReverseMux1SignalingLayout,
) -> Option<ReverseMux1SignalingBlock> {
    let format = parse_reverse_mux1_full_rate_format(info_bits)?;
    if format.signaling_bits == 0 {
        return None;
    }

    let after_header = &info_bits[format.header_bits..];
    let bits = match layout {
        ReverseMux1SignalingLayout::Suffix => {
            after_header[format.primary_bits..format.primary_bits + format.signaling_bits].to_vec()
        }
        ReverseMux1SignalingLayout::Prefix => after_header[..format.signaling_bits].to_vec(),
    };

    Some(ReverseMux1SignalingBlock {
        mux_header: format.mux_header,
        header_bits: format.header_bits,
        primary_bits: format.primary_bits,
        signaling_bits: format.signaling_bits,
        bits,
    })
}

pub fn parse_reverse_mux2_format(info_bits: &[u8]) -> Option<ReverseMux2Format> {
    let (header_bits, mixed_mode_primary_bits) = match info_bits.len() {
        267 => (5, 266),
        125 => (4, 124),
        55 => (3, 54),
        21 => (1, 20),
        _ => return None,
    };

    if info_bits[0] == 0 {
        return Some(ReverseMux2Format {
            mux_header: 0,
            header_bits: 1,
            primary_bits: mixed_mode_primary_bits,
            signaling_bits: 0,
        });
    }

    let mux_header = info_bits[..header_bits]
        .iter()
        .fold(0u8, |value, bit| (value << 1) | (bit & 1));
    let (primary_bits, signaling_bits) = match (info_bits.len(), mux_header) {
        (267, 0b10000) => (124, 138),
        (267, 0b10001) => (54, 208),
        (267, 0b10010) => (20, 242),
        (267, 0b10011) => (0, 262),
        (267, 0b10100) => (124, 0),
        (267, 0b10101) => (54, 0),
        (267, 0b10110) => (20, 0),
        (267, 0b10111) => (0, 0),
        (267, 0b11000) => (20, 222),
        (125, 0b1000) => (54, 67),
        (125, 0b1001) => (20, 101),
        (125, 0b1010) => (0, 121),
        (125, 0b1011) => (54, 0),
        (125, 0b1100) => (20, 0),
        (125, 0b1101) => (0, 0),
        (125, 0b1110) => (20, 81),
        (55, 0b100) => (20, 32),
        (55, 0b101) => (0, 52),
        (55, 0b110) => (20, 0),
        (55, 0b111) => (0, 0),
        (21, 0b1) => (0, 0),
        _ => return None,
    };

    Some(ReverseMux2Format {
        mux_header,
        header_bits,
        primary_bits,
        signaling_bits,
    })
}

pub fn extract_reverse_mux2_signaling_block(info_bits: &[u8]) -> Option<ReverseMux2SignalingBlock> {
    let format = parse_reverse_mux2_format(info_bits)?;
    if format.signaling_bits == 0 {
        return None;
    }

    let start = format.header_bits + format.primary_bits;
    let bits = info_bits[start..start + format.signaling_bits].to_vec();
    Some(ReverseMux2SignalingBlock {
        mux_header: format.mux_header,
        header_bits: format.header_bits,
        primary_bits: format.primary_bits,
        signaling_bits: format.signaling_bits,
        bits,
    })
}

pub fn parse_reverse_mux2_full_rate_format(info_bits: &[u8]) -> Option<ReverseMux2FullRateFormat> {
    if info_bits.len() != 267 {
        return None;
    }
    parse_reverse_mux2_format(info_bits)
}

pub fn extract_reverse_mux2_full_rate_signaling_block(
    info_bits: &[u8],
) -> Option<ReverseMux2SignalingBlock> {
    if info_bits.len() != 267 {
        return None;
    }
    extract_reverse_mux2_signaling_block(info_bits)
}

/// Configuration for a traffic channel frame structure.
#[derive(Debug, Clone, Copy)]
pub struct TrafficFrameConfig {
    /// Total bits per frame (info + FQI + tail).
    pub frame_bits: usize,
    /// Information bits per frame (fed to SAR reassembly).
    pub info_bits: usize,
    /// Frame Quality Indicator bits (CRC). 0 = no FQI (access channel).
    pub fqi_bits: usize,
    /// Encoder tail bits.
    pub tail_bits: usize,
}

/// Traffic Channel processor for decoded R-TCH bit streams.
///
/// Supports configurable frame sizes:
/// - RC1 traffic: 192 bits (172 info + 12 FQI + 8 tail) per C.S0002-E 2.1.3.12.1.1
/// - RC3 traffic: 192 bits (same structure at full rate)
/// - Access channel (legacy): 96 bits (88 info + 8 tail)
///
/// Uses the dedicated-channel SAR reassembly for r-dsch regular PDUs.
/// Reverse traffic signaling is not wrapped like r-csch access signaling:
/// it uses SOM + MSG_LENGTH + CRC16 per C.S0004-E 2.2.1.3.1.
///
/// Emits `traffic_event` tags instead of `access_event` tags so the BTS RX
/// loop can distinguish traffic channel events from access channel events.
pub struct TrafficChannelProcessor {
    walsh_code: u8,
    suffix_reader: DedicatedFrameReader,
    prefix_reader: DedicatedFrameReader,
    locked_layout: Option<ReverseMux1SignalingLayout>,
    bits: Vec<u8>,
    next_chip: usize,
    input_sample_rate_hz: f64,
    chips_per_bit: usize,
    message_count: usize,
    preamble_frames: usize,
    preamble_event_sent: bool,
    max_preamble_frames: usize,
    config: TrafficFrameConfig,
}

impl TrafficChannelProcessor {
    /// Create a new processor with RC1 traffic frame configuration (192 bits).
    pub fn new(walsh_code: u8) -> Self {
        Self::with_config_and_preamble_frames(walsh_code, RC1_FRAME_CONFIG, 1)
    }

    pub fn with_expected_preamble_frames(walsh_code: u8, max_preamble_frames: usize) -> Self {
        Self::with_config_and_preamble_frames(walsh_code, RC1_FRAME_CONFIG, max_preamble_frames)
    }

    /// Create a processor with a specific frame configuration.
    pub fn with_config(walsh_code: u8, config: TrafficFrameConfig) -> Self {
        Self::with_config_and_preamble_frames(walsh_code, config, 1)
    }

    fn with_config_and_preamble_frames(
        walsh_code: u8,
        config: TrafficFrameConfig,
        max_preamble_frames: usize,
    ) -> Self {
        Self {
            walsh_code,
            suffix_reader: DedicatedFrameReader::new(),
            prefix_reader: DedicatedFrameReader::new(),
            locked_layout: None,
            bits: Vec::new(),
            next_chip: 0,
            input_sample_rate_hz: 0.0,
            chips_per_bit: 1,
            message_count: 0,
            preamble_frames: 0,
            preamble_event_sent: false,
            max_preamble_frames: max_preamble_frames.max(1),
            config,
        }
    }

    fn copy_context_tags(
        out: &mut SampleBlock,
        upstream_tags: &std::collections::HashMap<&'static str, i64>,
    ) {
        for key in [
            "pilot_phase",
            "pn_phase",
            "absolute_chip_start",
            "absolute_sample_start",
            "finger_snr_mdb",
            "finger_signal_power_mdb",
            "finger_raw_power_mdb",
            "finger_pilot_ec_io_mdb",
            "traffic_pcg_pilot_ec_io_true_mdb",
            "traffic_pcg_pilot_ec_io_legacy_mdb",
            "traffic_ml_tail_match",
            "traffic_phy_valid",
            "traffic_radio_config",
            "traffic_rate_bps",
            "traffic_info_bits",
            "traffic_fqi_bits",
            "traffic_tail_bits",
            "traffic_fqi_valid",
            "traffic_tail_valid",
        ] {
            if let Some(value) = upstream_tags.get(key).copied() {
                out.tags.insert(key, value);
            }
        }
    }

    fn adjust_absolute_chip_tag(
        out: &mut SampleBlock,
        upstream_chip_start: usize,
        event_chip_start: usize,
    ) {
        let Some(absolute_chip_start) = out.tags.get_mut("absolute_chip_start") else {
            return;
        };
        let chip_delta = event_chip_start as i64 - upstream_chip_start as i64;
        *absolute_chip_start = absolute_chip_start.saturating_add(chip_delta);
    }

    fn emit_traffic_event(
        &mut self,
        chip_start: usize,
        upstream_chip_start: usize,
        frame: AccessFrame,
        upstream_tags: &std::collections::HashMap<&'static str, i64>,
    ) -> SampleBlock {
        self.message_count += 1;
        let payload_samples = frame
            .data
            .bits()
            .iter()
            .map(|b| Complex32::new(*b as f32, 0.0))
            .collect::<Vec<_>>();
        let mut out = SampleBlock::new(payload_samples, chip_start)
            .with_sample_rate_hz(self.input_sample_rate_hz);
        out.tags.insert("traffic_event", 1);
        out.tags
            .insert("traffic_walsh_code", self.walsh_code as i64);
        out.tags
            .insert("traffic_message_count", self.message_count as i64);
        out.tags.insert("traffic_crc_valid", frame.crc_valid as i64);
        out.tags
            .insert("traffic_msg_length_octets", frame.msg_length_octets as i64);
        out.tags
            .insert("traffic_payload_bits", frame.data.len() as i64);
        out.tags
            .insert("traffic_preamble_frames", self.preamble_frames as i64);

        if frame.data.len() >= 8 {
            let mut data = frame.data.clone();
            if let Ok(pd_and_type) = data.read_bits(8) {
                out.tags.insert("traffic_pd", (pd_and_type >> 6) as i64);
                out.tags
                    .insert("traffic_msg_type", (pd_and_type & 0x3f) as i64);
            }
        }

        Self::copy_context_tags(&mut out, upstream_tags);
        Self::adjust_absolute_chip_tag(&mut out, upstream_chip_start, chip_start);
        out
    }

    fn emit_traffic_phy_frame(
        &self,
        chip_start: usize,
        upstream_chip_start: usize,
        bits: &[u8],
        upstream_tags: &std::collections::HashMap<&'static str, i64>,
    ) -> SampleBlock {
        let payload_samples = bits
            .iter()
            .map(|b| Complex32::new(*b as f32, 0.0))
            .collect::<Vec<_>>();
        let mut out = SampleBlock::new(payload_samples, chip_start)
            .with_sample_rate_hz(self.input_sample_rate_hz);
        out.tags.insert("traffic_phy_frame", 1);
        out.tags
            .insert("traffic_walsh_code", self.walsh_code as i64);
        out.tags.insert("traffic_frame_bits", bits.len() as i64);
        for key in [
            "traffic_phy_valid",
            "traffic_rate_bps",
            "traffic_info_bits",
            "traffic_fqi_bits",
            "traffic_tail_bits",
            "traffic_fqi_valid",
            "traffic_tail_valid",
            "traffic_ml_tail_match",
            "traffic_mux_header",
            "traffic_mux_header_bits",
            "traffic_mux_primary_bits",
            "traffic_mux_signaling_bits",
            "traffic_mux_signaling_layout",
        ] {
            if let Some(value) = upstream_tags.get(key).copied() {
                out.tags.insert(key, value);
            }
        }
        Self::copy_context_tags(&mut out, upstream_tags);
        Self::adjust_absolute_chip_tag(&mut out, upstream_chip_start, chip_start);
        out
    }

    fn emit_traffic_phy_status(
        &self,
        chip_start: usize,
        upstream_chip_start: usize,
        bits: &[u8],
        upstream_tags: &std::collections::HashMap<&'static str, i64>,
    ) -> SampleBlock {
        let mut out =
            SampleBlock::new(Vec::new(), chip_start).with_sample_rate_hz(self.input_sample_rate_hz);
        out.tags.insert("traffic_phy_status", 1);
        out.tags
            .insert("traffic_walsh_code", self.walsh_code as i64);
        out.tags.insert("traffic_frame_bits", bits.len() as i64);
        Self::copy_context_tags(&mut out, upstream_tags);
        Self::adjust_absolute_chip_tag(&mut out, upstream_chip_start, chip_start);
        out
    }

    fn emit_preamble_event(
        &self,
        chip_start: usize,
        upstream_chip_start: usize,
        upstream_tags: &std::collections::HashMap<&'static str, i64>,
    ) -> SampleBlock {
        let mut out =
            SampleBlock::new(Vec::new(), chip_start).with_sample_rate_hz(self.input_sample_rate_hz);
        out.tags.insert("traffic_preamble_detected", 1);
        out.tags
            .insert("traffic_walsh_code", self.walsh_code as i64);
        out.tags
            .insert("traffic_preamble_frames", self.preamble_frames as i64);
        Self::copy_context_tags(&mut out, upstream_tags);
        Self::adjust_absolute_chip_tag(&mut out, upstream_chip_start, chip_start);
        out
    }

    /// Check the 12-bit Frame Quality Indicator (CRC) on a traffic frame.
    /// Returns true if CRC matches or if no FQI is configured.
    fn check_fqi(&self, frame_bits: &[u8]) -> bool {
        if self.config.fqi_bits == 0 {
            return true;
        }
        let info_end = self.config.info_bits;
        let fqi_end = info_end + self.config.fqi_bits;
        if frame_bits.len() < fqi_end {
            return false;
        }
        let info = &frame_bits[..info_end];
        let fqi = &frame_bits[info_end..fqi_end];

        let computed = crc12(info);
        let mut received: u16 = 0;
        for &bit in fqi {
            received = (received << 1) | (bit as u16 & 1);
        }
        computed == received
    }
}

impl PipelineProcessor for TrafficChannelProcessor {
    fn process_block(&mut self, block: SampleBlock) -> Vec<SampleBlock> {
        // Pass through preamble detection events from upstream (walsh synchronizer).
        // These have traffic_preamble_detected set but no samples to decode.
        if block
            .tags
            .get("traffic_preamble_detected")
            .copied()
            .unwrap_or(0)
            == 1
            && block
                .tags
                .get("traffic_decoded_frame")
                .copied()
                .unwrap_or(0)
                != 1
        {
            self.preamble_frames = self.preamble_frames.max(
                block
                    .tags
                    .get("traffic_preamble_frames")
                    .copied()
                    .unwrap_or(1)
                    .max(1) as usize,
            );
            self.suffix_reader.reset();
            self.prefix_reader.reset();
            self.locked_layout = None;
            if self.preamble_event_sent {
                return Vec::new();
            }
            self.preamble_event_sent = true;
            return vec![block];
        }

        if block
            .tags
            .get("traffic_pcg_measurement")
            .copied()
            .unwrap_or(0)
            == 1
        {
            let mut event = block;
            event
                .tags
                .insert("traffic_walsh_code", self.walsh_code as i64);
            return vec![event];
        }

        if block
            .tags
            .get("traffic_decoded_frame")
            .copied()
            .unwrap_or(0)
            == 1
        {
            let frame_chip = block.chip_start;
            let bits: Vec<u8> = block
                .samples
                .iter()
                .map(|s| if s.re >= 0.5 { 1u8 } else { 0u8 })
                .collect();
            let info_bits = block
                .tags
                .get("traffic_info_bits")
                .copied()
                .unwrap_or(self.config.info_bits as i64) as usize;
            let fqi_bits = block
                .tags
                .get("traffic_fqi_bits")
                .copied()
                .unwrap_or(self.config.fqi_bits as i64) as usize;
            let is_preamble = block.tags.get("traffic_is_preamble").copied().unwrap_or(0) == 1;
            let fqi_valid = block.tags.get("traffic_fqi_valid").copied().unwrap_or(0) == 1;
            let tail_valid = block.tags.get("traffic_tail_valid").copied().unwrap_or(0) == 1;
            let hinted_layout = block
                .tags
                .get("traffic_mux_signaling_layout")
                .copied()
                .map(ReverseMux1SignalingLayout::from_tag);
            let radio_config = block.tags.get("traffic_radio_config").copied();

            log::trace!(
                "traffic_channel_processor: decoded frame walsh={} chip={} bits={} info_bits={} fqi_bits={} preamble={} fqi_valid={} tail_valid={}",
                self.walsh_code,
                frame_chip,
                bits.len(),
                info_bits,
                fqi_bits,
                is_preamble,
                fqi_valid,
                tail_valid,
            );

            self.input_sample_rate_hz = block.sample_rate_hz;

            if is_preamble && self.preamble_frames < self.max_preamble_frames {
                self.preamble_frames = self.preamble_frames.saturating_add(1);
                self.suffix_reader.reset();
                self.prefix_reader.reset();
                self.locked_layout = None;
                log::trace!(
                    "traffic_preamble_frame: walsh={} chip={} preamble_frames={}",
                    self.walsh_code,
                    frame_chip,
                    self.preamble_frames,
                );
                if self.preamble_event_sent {
                    return Vec::new();
                }
                self.preamble_event_sent = true;
                return vec![self.emit_preamble_event(frame_chip, block.chip_start, &block.tags)];
            }

            if !tail_valid || (fqi_bits > 0 && !fqi_valid) {
                log::trace!(
                    "traffic_frame_phy_fail: walsh={} chip={} fqi_valid={} tail_valid={}",
                    self.walsh_code,
                    frame_chip,
                    fqi_valid,
                    tail_valid,
                );
                let mut status =
                    self.emit_traffic_phy_status(frame_chip, block.chip_start, &bits, &block.tags);
                status.pcg_signal_snr_db = block.pcg_signal_snr_db.clone();
                status.active_pcg_mask = block.active_pcg_mask;
                return vec![status];
            }

            let mut phy_block =
                self.emit_traffic_phy_frame(frame_chip, block.chip_start, &bits, &block.tags);
            phy_block.pcg_signal_snr_db = block.pcg_signal_snr_db.clone();
            phy_block.active_pcg_mask = block.active_pcg_mask;
            let mut out = vec![phy_block];

            if bits.len() < info_bits {
                warn!(
                    "traffic_frame_short: walsh={} chip={} bits={} info_bits={}",
                    self.walsh_code,
                    frame_chip,
                    bits.len(),
                    info_bits,
                );
                return out;
            }

            if radio_config == Some(2) {
                let Some(signaling_block) =
                    extract_reverse_mux2_signaling_block(&bits[..info_bits])
                else {
                    return out;
                };
                debug!(
                    "traffic_mux2_candidate: walsh={} chip={} mux_header=0x{:X} header_bits={} primary_bits={} signaling_bits={}",
                    self.walsh_code,
                    frame_chip,
                    signaling_block.mux_header,
                    signaling_block.header_bits,
                    signaling_block.primary_bits,
                    signaling_block.signaling_bits,
                );
                let mut info_bs = Bitstream::new_init(&signaling_block.bits);
                if let Ok(Some(frame)) = self.suffix_reader.process(&mut info_bs) {
                    info!(
                        "traffic_frame: walsh={} chip={} crc={} mux=2 mux_header=0x{:X} header_bits={} signaling_bits={} msg_len={} payload_bits={} preamble_frames={}",
                        self.walsh_code,
                        frame_chip,
                        if frame.crc_valid { "VALID" } else { "INVALID" },
                        signaling_block.mux_header,
                        signaling_block.header_bits,
                        signaling_block.signaling_bits,
                        frame.msg_length_octets,
                        frame.data.len(),
                        self.preamble_frames,
                    );
                    if frame.crc_valid {
                        self.locked_layout = Some(ReverseMux1SignalingLayout::Suffix);
                        self.prefix_reader.reset();
                        let mut event = self.emit_traffic_event(
                            frame_chip,
                            block.chip_start,
                            frame,
                            &block.tags,
                        );
                        event.pcg_signal_snr_db = block.pcg_signal_snr_db.clone();
                        event.active_pcg_mask = block.active_pcg_mask;
                        event.tags.insert(
                            "traffic_mux_signaling_layout",
                            ReverseMux1SignalingLayout::Suffix.tag_value(),
                        );
                        event
                            .tags
                            .insert("traffic_mux_header", signaling_block.mux_header as i64);
                        event.tags.insert(
                            "traffic_mux_header_bits",
                            signaling_block.header_bits as i64,
                        );
                        event.tags.insert(
                            "traffic_mux_primary_bits",
                            signaling_block.primary_bits as i64,
                        );
                        event.tags.insert(
                            "traffic_mux_signaling_bits",
                            signaling_block.signaling_bits as i64,
                        );
                        out.push(event);
                    }
                }
                return out;
            }

            let layouts_to_try: [ReverseMux1SignalingLayout; 2] =
                match self.locked_layout.or(hinted_layout) {
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
                if let Some(locked) = self.locked_layout
                    && layout != locked
                {
                    continue;
                }

                let Some(signaling_block) =
                    extract_reverse_mux1_full_rate_signaling_block(&bits[..info_bits], layout)
                else {
                    continue;
                };

                debug!(
                    "traffic_mux_candidate: walsh={} chip={} layout={:?} mux_header=0b{:04b} primary_bits={} signaling_bits={}",
                    self.walsh_code,
                    frame_chip,
                    layout,
                    signaling_block.mux_header,
                    signaling_block.primary_bits,
                    signaling_block.signaling_bits,
                );

                let reader = match layout {
                    ReverseMux1SignalingLayout::Suffix => &mut self.suffix_reader,
                    ReverseMux1SignalingLayout::Prefix => &mut self.prefix_reader,
                };
                let mut info_bs = Bitstream::new_init(&signaling_block.bits);
                if let Ok(Some(frame)) = reader.process(&mut info_bs) {
                    let crc_status = if frame.crc_valid { "VALID" } else { "INVALID" };
                    info!(
                        "traffic_frame: walsh={} chip={} crc={} layout={:?} mux_header=0b{:04b} signaling_bits={} msg_len={} payload_bits={} preamble_frames={}",
                        self.walsh_code,
                        frame_chip,
                        crc_status,
                        layout,
                        signaling_block.mux_header,
                        signaling_block.signaling_bits,
                        frame.msg_length_octets,
                        frame.data.len(),
                        self.preamble_frames,
                    );

                    if frame.crc_valid {
                        self.locked_layout = Some(layout);
                        match layout {
                            ReverseMux1SignalingLayout::Suffix => self.prefix_reader.reset(),
                            ReverseMux1SignalingLayout::Prefix => self.suffix_reader.reset(),
                        }
                        let mut event = self.emit_traffic_event(
                            frame_chip,
                            block.chip_start,
                            frame,
                            &block.tags,
                        );
                        event.pcg_signal_snr_db = block.pcg_signal_snr_db.clone();
                        event.active_pcg_mask = block.active_pcg_mask;
                        event
                            .tags
                            .insert("traffic_mux_signaling_layout", layout.tag_value());
                        out.push(event);
                        break;
                    }
                }
            }

            return out;
        }

        if self.next_chip == 0 {
            self.next_chip = block.chip_start;
        }
        self.input_sample_rate_hz = block.sample_rate_hz;
        self.chips_per_bit = chips_per_sample(block.sample_rate_hz);

        self.bits.extend(
            block
                .samples
                .iter()
                .map(|s| if s.re >= 0.5 { 1u8 } else { 0u8 }),
        );

        let frame_bits = self.config.frame_bits;
        let info_bits = self.config.info_bits;

        let mut out = Vec::new();
        while self.bits.len() >= frame_bits {
            let frame: Vec<u8> = self.bits.drain(..frame_bits).collect();
            let frame_chip = self.next_chip;
            self.next_chip += frame_bits * self.chips_per_bit;

            let info = &frame[..info_bits];

            // All-zero frame = preamble (RC1: 192 zeros, no FQI per spec 2.1.3.12.1.3.1)
            if self.suffix_reader.is_idle()
                && self.prefix_reader.is_idle()
                && frame.iter().all(|b| *b == 0)
                && self.preamble_frames < self.max_preamble_frames
            {
                self.preamble_frames = self.preamble_frames.saturating_add(1);
                self.suffix_reader.reset();
                self.prefix_reader.reset();
                self.locked_layout = None;
                log::trace!(
                    "traffic_preamble_frame: walsh={} chip={} preamble_frames={}",
                    self.walsh_code,
                    frame_chip,
                    self.preamble_frames,
                );
                if !self.preamble_event_sent {
                    self.preamble_event_sent = true;
                    out.push(self.emit_preamble_event(frame_chip, block.chip_start, &block.tags));
                }
                continue;
            }

            // Check Frame Quality Indicator (CRC) if present
            let fqi_ok = self.check_fqi(&frame);
            if !fqi_ok {
                debug!(
                    "traffic_frame_fqi_fail: walsh={} chip={}",
                    self.walsh_code, frame_chip,
                );
                continue;
            }

            let mut info_bs = Bitstream::new_init(info);
            if let Ok(Some(frame)) = self.suffix_reader.process(&mut info_bs) {
                let crc_status = if frame.crc_valid { "VALID" } else { "INVALID" };
                info!(
                    "traffic_frame: walsh={} chip={} crc={} msg_len={} payload_bits={} preamble_frames={}",
                    self.walsh_code,
                    frame_chip,
                    crc_status,
                    frame.msg_length_octets,
                    frame.data.len(),
                    self.preamble_frames,
                );
                let mut event =
                    self.emit_traffic_event(frame_chip, block.chip_start, frame, &block.tags);
                // Propagate the per-PCG Eb/Nt measurements from the
                // upstream frame aligner so the BSC can drive closed-loop
                // power control. See `docs/power-control.md`.
                event.pcg_signal_snr_db = block.pcg_signal_snr_db.clone();
                event.active_pcg_mask = block.active_pcg_mask;
                out.push(event);
            }
        }
        out
    }

    fn name(&self) -> &'static str {
        "TrafficChannelProcessor"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_crc12_zero_input() {
        // With the spec-required all-ones initial state, all-zero input is non-zero.
        let data = vec![0u8; 172];
        let crc = crc12(&data);
        assert_eq!(crc, 0x3D7);
    }

    #[test]
    fn test_crc12_nonzero() {
        let mut data = vec![0u8; 172];
        data[0] = 1;
        let crc = crc12(&data);
        assert_ne!(crc, 0, "CRC should be non-zero for non-zero input");
        assert!(crc < 0x1000, "CRC should be 12 bits");
    }

    #[test]
    fn test_crc12_verify() {
        // Generate CRC and append it, then verify
        let info = vec![1, 0, 1, 1, 0, 0, 1, 0, 1, 1, 0, 1];
        let crc = crc12(&info);
        let mut frame = info.clone();
        for i in (0..12).rev() {
            frame.push(((crc >> i) & 1) as u8);
        }
        // Verify: recompute CRC on info portion
        assert_eq!(crc12(&frame[..info.len()]), crc);
    }

    #[test]
    fn test_rc1_frame_config() {
        assert_eq!(RC1_FRAME_CONFIG.frame_bits, 192);
        assert_eq!(
            RC1_FRAME_CONFIG.info_bits + RC1_FRAME_CONFIG.fqi_bits + RC1_FRAME_CONFIG.tail_bits,
            192
        );
    }

    #[test]
    fn test_preamble_detection_192() {
        let proc = TrafficChannelProcessor::new(8);
        assert_eq!(proc.config.frame_bits, 192);

        // 192-zero frame should be detected as preamble
        let zeros = vec![Complex32::new(0.0, 0.0); 192];
        let block = SampleBlock::new(zeros, 0);
        let mut proc = TrafficChannelProcessor::new(8);
        let out = proc.process_block(block);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].tags.get("traffic_preamble_detected"), Some(&1));
    }

    #[test]
    fn test_upstream_preamble_event_is_forwarded_once() {
        let mut proc = TrafficChannelProcessor::with_expected_preamble_frames(8, 4);

        let mut first = SampleBlock::new(Vec::new(), 123).with_sample_rate_hz(9_600.0);
        first.tags.insert("traffic_preamble_detected", 1);
        first.tags.insert("traffic_preamble_frames", 3);

        let out = proc.process_block(first);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].tags.get("traffic_preamble_detected"), Some(&1));
        assert_eq!(proc.preamble_frames, 3);
        assert!(proc.preamble_event_sent);

        let mut duplicate = SampleBlock::new(Vec::new(), 456).with_sample_rate_hz(9_600.0);
        duplicate.tags.insert("traffic_preamble_detected", 1);
        duplicate.tags.insert("traffic_preamble_frames", 4);

        let out = proc.process_block(duplicate);
        assert!(out.is_empty());
        assert_eq!(proc.preamble_frames, 4);
    }

    #[test]
    fn test_local_decoded_preamble_only_emits_one_event() {
        let mut proc = TrafficChannelProcessor::with_expected_preamble_frames(8, 4);
        let preamble = vec![Complex32::new(0.0, 0.0); RC1_FRAME_CONFIG.frame_bits];

        let mut first = SampleBlock::new(preamble.clone(), 0).with_sample_rate_hz(9_600.0);
        first.tags.insert("traffic_decoded_frame", 1);
        first.tags.insert("traffic_is_preamble", 1);
        first.tags.insert("traffic_tail_valid", 1);

        let out = proc.process_block(first);
        assert_eq!(out.len(), 1);
        assert_eq!(out[0].tags.get("traffic_preamble_detected"), Some(&1));
        assert_eq!(proc.preamble_frames, 1);
        assert!(proc.preamble_event_sent);

        let mut second =
            SampleBlock::new(preamble, RC1_FRAME_CONFIG.frame_bits).with_sample_rate_hz(9_600.0);
        second.tags.insert("traffic_decoded_frame", 1);
        second.tags.insert("traffic_is_preamble", 1);
        second.tags.insert("traffic_tail_valid", 1);

        let out = proc.process_block(second);
        assert!(out.is_empty());
        assert_eq!(proc.preamble_frames, 2);
    }

    #[test]
    fn test_reverse_mux1_signaling_only_extracts_168_bits() {
        let mut info_bits = vec![0u8; 172];
        info_bits[..4].copy_from_slice(&[1, 0, 1, 1]);
        for (idx, bit) in info_bits[4..].iter_mut().enumerate() {
            *bit = (idx & 1) as u8;
        }

        let format = parse_reverse_mux1_full_rate_format(&info_bits).expect("format");
        assert_eq!(format.mux_header, 0b1011);
        assert_eq!(format.primary_bits, 0);
        assert_eq!(format.signaling_bits, 168);

        let signaling = extract_reverse_mux1_full_rate_signaling_block(
            &info_bits,
            ReverseMux1SignalingLayout::Suffix,
        )
        .expect("signaling block");
        assert_eq!(signaling.bits.len(), 168);
        assert_eq!(signaling.bits, info_bits[4..172].to_vec());
    }

    #[test]
    fn reverse_mux2_signaling_only_extracts_262_bits() {
        let mut info_bits = vec![0u8; 267];
        info_bits[..5].copy_from_slice(&[1, 0, 0, 1, 1]);
        for (idx, bit) in info_bits[5..].iter_mut().enumerate() {
            *bit = (idx & 1) as u8;
        }

        let format = parse_reverse_mux2_full_rate_format(&info_bits).expect("format");
        assert_eq!(format.mux_header, 0b10011);
        assert_eq!(format.primary_bits, 0);
        assert_eq!(format.signaling_bits, 262);

        let signaling =
            extract_reverse_mux2_full_rate_signaling_block(&info_bits).expect("signaling block");
        assert_eq!(signaling.bits, info_bits[5..267].to_vec());
    }

    #[test]
    fn reverse_mux2_formats_cover_all_rc2_rates() {
        let cases = [
            (267, "0", 266, 0),
            (267, "10000", 124, 138),
            (267, "10001", 54, 208),
            (267, "10010", 20, 242),
            (267, "10011", 0, 262),
            (267, "10100", 124, 0),
            (267, "10101", 54, 0),
            (267, "10110", 20, 0),
            (267, "10111", 0, 0),
            (267, "11000", 20, 222),
            (125, "0", 124, 0),
            (125, "1000", 54, 67),
            (125, "1001", 20, 101),
            (125, "1010", 0, 121),
            (125, "1011", 54, 0),
            (125, "1100", 20, 0),
            (125, "1101", 0, 0),
            (125, "1110", 20, 81),
            (55, "0", 54, 0),
            (55, "100", 20, 32),
            (55, "101", 0, 52),
            (55, "110", 20, 0),
            (55, "111", 0, 0),
            (21, "0", 20, 0),
            (21, "1", 0, 0),
        ];

        for (info_len, header, primary_bits, signaling_bits) in cases {
            let header: Vec<u8> = header.bytes().map(|bit| bit - b'0').collect();
            let mut info = vec![0; info_len];
            info[..header.len()].copy_from_slice(&header);

            let format = parse_reverse_mux2_format(&info).expect("valid MuxPDU Type 2 format");
            assert_eq!(format.header_bits, header.len());
            assert_eq!(format.primary_bits, primary_bits);
            assert_eq!(format.signaling_bits, signaling_bits);

            let extracted = extract_reverse_mux2_signaling_block(&info);
            assert_eq!(extracted.is_some(), signaling_bits > 0);
            if let Some(extracted) = extracted {
                assert_eq!(extracted.bits.len(), signaling_bits);
            }
        }
    }

    #[test]
    fn rc2_half_rate_dim_and_burst_emits_traffic_event() {
        let mut pdu = Bitstream::new();
        pdu.write_u8(0x01, 8);
        let full_rate_frames = crate::lac::sar_fragment_ftch_pdu_dsch_rc2(&pdu);
        assert_eq!(full_rate_frames.len(), 1);
        let full_rate_bits = full_rate_frames[0].bits();

        let mut half_rate_bits = vec![1, 0, 0, 0];
        half_rate_bits.extend(std::iter::repeat_n(0, 54));
        half_rate_bits.extend_from_slice(&full_rate_bits[5..5 + 67]);
        assert_eq!(half_rate_bits.len(), 125);

        let samples = half_rate_bits
            .into_iter()
            .map(|bit| Complex32::new(bit as f32, 0.0))
            .collect();
        let mut block = SampleBlock::new(samples, 12_288).with_sample_rate_hz(9_600.0);
        block.tags.insert("traffic_decoded_frame", 1);
        block.tags.insert("traffic_radio_config", 2);
        block.tags.insert("traffic_rate_bps", 7_200);
        block.tags.insert("traffic_info_bits", 125);
        block.tags.insert("traffic_fqi_bits", 10);
        block.tags.insert("traffic_tail_bits", 8);
        block.tags.insert("traffic_fqi_valid", 1);
        block.tags.insert("traffic_tail_valid", 1);

        let mut processor = TrafficChannelProcessor::new(8);
        let out = processor.process_block(block);

        assert_eq!(out.len(), 2);
        let event = out
            .iter()
            .find(|block| block.tags.get("traffic_event") == Some(&1))
            .expect("reassembled traffic event");
        assert_eq!(event.tags.get("traffic_radio_config"), Some(&2));
        assert_eq!(event.tags.get("traffic_mux_header"), Some(&0b1000));
        assert_eq!(event.tags.get("traffic_mux_header_bits"), Some(&4));
        assert_eq!(event.tags.get("traffic_mux_primary_bits"), Some(&54));
        assert_eq!(event.tags.get("traffic_mux_signaling_bits"), Some(&67));
        assert_eq!(
            event
                .samples
                .iter()
                .map(|sample| sample.re as u8)
                .collect::<Vec<_>>(),
            pdu.bits()
        );
    }

    #[test]
    fn test_copy_context_tags_preserves_reverse_pilot_ec_io() {
        let mut out = SampleBlock::new(Vec::new(), 0);
        let mut upstream_tags = std::collections::HashMap::new();
        upstream_tags.insert("finger_pilot_ec_io_mdb", -12050);
        upstream_tags.insert("traffic_pcg_pilot_ec_io_true_mdb", -18750);
        upstream_tags.insert("traffic_pcg_pilot_ec_io_legacy_mdb", -12900);
        upstream_tags.insert("finger_snr_mdb", 12345);
        upstream_tags.insert("traffic_phy_valid", 1);
        upstream_tags.insert("traffic_fqi_bits", 6);
        upstream_tags.insert("traffic_fqi_valid", 1);
        upstream_tags.insert("traffic_tail_valid", 1);

        TrafficChannelProcessor::copy_context_tags(&mut out, &upstream_tags);

        assert_eq!(out.tags.get("finger_pilot_ec_io_mdb"), Some(&-12050));
        assert_eq!(
            out.tags.get("traffic_pcg_pilot_ec_io_true_mdb"),
            Some(&-18750)
        );
        assert_eq!(
            out.tags.get("traffic_pcg_pilot_ec_io_legacy_mdb"),
            Some(&-12900)
        );
        assert_eq!(out.tags.get("finger_snr_mdb"), Some(&12345));
        assert_eq!(out.tags.get("traffic_phy_valid"), Some(&1));
        assert_eq!(out.tags.get("traffic_fqi_bits"), Some(&6));
        assert_eq!(out.tags.get("traffic_fqi_valid"), Some(&1));
        assert_eq!(out.tags.get("traffic_tail_valid"), Some(&1));
    }

    #[test]
    fn invalid_decoded_phy_frame_emits_status_only_event() {
        let mut proc = TrafficChannelProcessor::new(10);
        let mut block = SampleBlock::new(vec![Complex32::new(0.0, 0.0); 576], 1234)
            .with_sample_rate_hz(9_600.0);
        block.tags.insert("traffic_decoded_frame", 1);
        block.tags.insert("absolute_chip_start", 99_000);
        block.tags.insert("traffic_rate_bps", 2_700);
        block.tags.insert("traffic_info_bits", 55);
        block.tags.insert("traffic_fqi_bits", 6);
        block.tags.insert("traffic_tail_bits", 8);
        block.tags.insert("traffic_fqi_valid", 0);
        block.tags.insert("traffic_tail_valid", 1);
        block.tags.insert("traffic_phy_valid", 0);
        block.pcg_signal_snr_db = Some(vec![3.0; 16]);
        block.active_pcg_mask = Some([true; 16]);

        let out = proc.process_block(block);

        assert_eq!(out.len(), 1);
        assert_eq!(out[0].tags.get("traffic_phy_status"), Some(&1));
        assert_eq!(out[0].tags.get("traffic_phy_frame"), None);
        assert_eq!(out[0].tags.get("traffic_event"), None);
        assert_eq!(out[0].tags.get("traffic_walsh_code"), Some(&10));
        assert_eq!(out[0].tags.get("traffic_phy_valid"), Some(&0));
        assert_eq!(out[0].tags.get("traffic_fqi_bits"), Some(&6));
        assert_eq!(out[0].tags.get("traffic_fqi_valid"), Some(&0));
        assert_eq!(out[0].tags.get("traffic_tail_valid"), Some(&1));
        assert_eq!(out[0].samples.len(), 0);
        assert_eq!(out[0].pcg_signal_snr_db.as_ref().map(Vec::len), Some(16));
        assert_eq!(out[0].active_pcg_mask, Some([true; 16]));
    }

    /// Decode a real MS response frame captured from a live mobile station.
    /// The frame was received on the reverse traffic channel after the BS sent
    /// a BS Ack Order on the forward traffic channel.
    #[test]
    fn test_decode_live_ms_reverse_traffic_frame() {
        use crate::receiver::access::DedicatedFrameReader;

        // Real frame from live MS: rc1_traffic_multi_rate_decoder reported
        // rate=9600 fqi_valid=true
        let hex_str = "9e1b1904e47f5fd8bb6f3b86c9b8e131ffcf0abbf090";
        let data: Vec<u8> = (0..hex_str.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex_str[i..i + 2], 16).unwrap())
            .collect();
        let mut info_bits = Vec::new();
        for byte in &data {
            for i in (0..8).rev() {
                info_bits.push((byte >> i) & 1);
            }
        }
        // 172 information bits (22 bytes = 176 bits, last 4 are padding)
        let info_bits = &info_bits[..172];

        // Step 1: Parse MuxPDU Type 1 header
        let format = parse_reverse_mux1_full_rate_format(info_bits).expect("should parse MuxPDU");
        println!(
            "MuxPDU: header=0b{:04b} primary_bits={} signaling_bits={}",
            format.mux_header, format.primary_bits, format.signaling_bits
        );

        // Step 2: Extract signaling block (try both layouts)
        let mut signaling = None;
        let mut found_layout = None;
        for layout in ReverseMux1SignalingLayout::SEARCH_ORDER {
            if let Some(block) = extract_reverse_mux1_full_rate_signaling_block(info_bits, layout) {
                println!(
                    "Layout {:?}: {} signaling bits extracted",
                    layout,
                    block.bits.len()
                );
                // Step 3: Feed to DedicatedFrameReader (SAR reassembly)
                // Print raw signaling bits for debugging
                let sig_hex: String = block
                    .bits
                    .chunks(8)
                    .map(|chunk| {
                        let byte: u8 = chunk
                            .iter()
                            .enumerate()
                            .fold(0u8, |acc, (i, &b)| acc | (b << (7 - i)));
                        format!("{:02x}", byte)
                    })
                    .collect();
                println!("  raw signaling hex: {}", sig_hex);

                // Check SOM and MSG_LENGTH
                if !block.bits.is_empty() {
                    let som = block.bits[0];
                    println!("  SOM={}", som);
                    if block.bits.len() >= 9 {
                        let msg_len: u8 =
                            (0..8).fold(0u8, |acc, i| acc | (block.bits[1 + i] << (7 - i)));
                        println!(
                            "  MSG_LENGTH={} ({} bits needed)",
                            msg_len,
                            msg_len as usize * 8
                        );
                    }
                }

                let mut reader = DedicatedFrameReader::new();
                let mut bs = Bitstream::new_init(&block.bits);
                match reader.process(&mut bs) {
                    Ok(Some(frame)) => {
                        println!(
                            "SAR: crc_valid={} msg_length={} payload_bits={}",
                            frame.crc_valid,
                            frame.msg_length_octets,
                            frame.data.len()
                        );
                        if frame.crc_valid {
                            signaling = Some(frame);
                            found_layout = Some(layout);
                            break;
                        }
                    }
                    Ok(None) => println!("  SAR: incomplete (need more fragments)"),
                    Err(e) => println!("  SAR: error: {}", e),
                }
            }
        }

        // Also try decoding as blank-and-burst (0b1011) in case the header
        // was misread, or dump all bits for manual inspection
        println!("\nFull 172 info bits as hex:");
        let full_hex: String = info_bits
            .chunks(8)
            .map(|chunk| {
                let byte: u8 = chunk
                    .iter()
                    .enumerate()
                    .fold(0u8, |acc, (i, &b)| acc | (b << (7 - i)));
                format!("{:02x}", byte)
            })
            .collect();
        println!("  {}", full_hex);

        // Try the second frame too
        let hex_str2 = "7d424ea3054d5230e0cb07982adda59ac081a410e030";
        let data2: Vec<u8> = (0..hex_str2.len())
            .step_by(2)
            .map(|i| u8::from_str_radix(&hex_str2[i..i + 2], 16).unwrap())
            .collect();
        let mut info_bits2 = Vec::new();
        for byte in &data2 {
            for i in (0..8).rev() {
                info_bits2.push((byte >> i) & 1);
            }
        }
        let info_bits2 = &info_bits2[..172];
        let format2 = parse_reverse_mux1_full_rate_format(info_bits2);
        println!("\nFrame 2:");
        if let Some(f) = &format2 {
            println!(
                "  MuxPDU: header=0b{:04b} primary={} signaling={}",
                f.mux_header, f.primary_bits, f.signaling_bits
            );
        } else {
            println!("  MM=0 (primary traffic only, {} bits)", 171);
        }
        println!("  Full hex: {}", hex_str2);

        // For now, don't fail — we're investigating
        if signaling.is_none() {
            println!(
                "\nNo valid signaling frame found — MS may not have sent MS Ack Order in these frames"
            );
        }

        if let Some(frame) = signaling {
            let layout = found_layout.unwrap();
            println!("Decoded with layout: {:?}", layout);

            // Step 4: Parse the r-dsch PDU fields
            let mut pdu = frame.data;
            let msg_type = pdu.read_bits(8).expect("msg_type") as u8;
            let ack_seq = pdu.read_bits(3).expect("ack_seq") as u8;
            let msg_seq = pdu.read_bits(3).expect("msg_seq") as u8;
            let ack_req = pdu.read_bits(1).expect("ack_req") as u8;
            let encryption = pdu.read_bits(2).expect("encryption") as u8;

            println!(
                "r-dsch PDU: MSG_TYPE=0x{:02x} ACK_SEQ={} MSG_SEQ={} ACK_REQ={} ENCRYPTION={}",
                msg_type, ack_seq, msg_seq, ack_req, encryption
            );

            // Step 5: If it's an Order message (MSG_TYPE=0x01), parse the order fields
            if msg_type == 0x01 {
                let use_time = pdu.read_bits(1).expect("use_time") as u8;
                let action_time = pdu.read_bits(6).expect("action_time") as u8;
                let order = pdu.read_bits(6).expect("order") as u8;
                let add_record_len = pdu.read_bits(3).expect("add_record_len") as u8;
                println!(
                    "Order: USE_TIME={} ACTION_TIME={} ORDER={} (0b{:06b}) ADD_RECORD_LEN={}",
                    use_time, action_time, order, order, add_record_len
                );
                if order == 0b010000 {
                    println!("=> MS Ack Order!");
                }
            }
        }
    }

    #[test]
    fn test_reverse_mux1_mixed_mode_extracts_suffix_signaling() {
        let mut info_bits = vec![0u8; 172];
        info_bits[..4].copy_from_slice(&[1, 0, 0, 0]);
        for bit in &mut info_bits[4..84] {
            *bit = 1;
        }
        for (idx, bit) in info_bits[84..172].iter_mut().enumerate() {
            *bit = (idx & 1) as u8;
        }

        let format = parse_reverse_mux1_full_rate_format(&info_bits).expect("format");
        assert_eq!(format.mux_header, 0b1000);
        assert_eq!(format.primary_bits, 80);
        assert_eq!(format.signaling_bits, 88);

        let suffix = extract_reverse_mux1_full_rate_signaling_block(
            &info_bits,
            ReverseMux1SignalingLayout::Suffix,
        )
        .expect("suffix");
        assert_eq!(suffix.bits, info_bits[84..172].to_vec());

        let prefix = extract_reverse_mux1_full_rate_signaling_block(
            &info_bits,
            ReverseMux1SignalingLayout::Prefix,
        )
        .expect("prefix");
        assert_eq!(prefix.bits, info_bits[4..92].to_vec());
    }
}
