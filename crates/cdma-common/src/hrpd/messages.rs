//! HRPD (1xEV-DO Rev 0) overhead-message encoders.
//!
//! Spec: 3GPP2 C.S0024-0 v4.0 ("HRPD Air Interface").
//! - QuickConfig          : §6.8.6.2.1 (Overhead Messages Protocol)
//! - SectorParameters     : §6.8.6.2.2 (Overhead Messages Protocol)
//! - AccessParameters     : §8.3.6.2.6 (Default Access Channel MAC Protocol)
//! - Sync                 : §6.3.6.2.1 (Default Initialization State Protocol)
//!
//! Each encoder produces only the message body (MessageID + fields + zero-bit
//! padding to an octet boundary). MAC-layer / Control-Channel capsule framing
//! is handled elsewhere.
//!
//! Bit-packing reuses `crate::bits::Bitstream` (big-endian, MSB-first bit
//! order, which matches how the spec lists fields top-to-bottom).

use crate::bits::Bitstream;

pub const DEFAULT_ACCESS_CHANNEL_MAC_PROTOCOL_TYPE: u8 = 0x02;
pub const DEFAULT_INITIALIZATION_STATE_PROTOCOL_TYPE: u8 = 0x0b;
pub const DEFAULT_REVERSE_TRAFFIC_CHANNEL_MAC_PROTOCOL_TYPE: u8 = 0x04;
pub const OVERHEAD_MESSAGES_PROTOCOL_TYPE: u8 = 0x0f;

// ----- MessageID constants (top byte of each encoded body) -------------------

/// `QuickConfig.MessageID` per C.S0024-0 §6.8.6.2.1 (NOMPDefault, OMP protocol).
pub const QUICK_CONFIG_MESSAGE_ID: u8 = 0x00;
/// `SectorParameters.MessageID` per C.S0024-0 §6.8.6.2.2 (NOMPDefault).
pub const SECTOR_PARAMETERS_MESSAGE_ID: u8 = 0x01;
/// `AccessParameters.MessageID` per C.S0024-0 §8.3.6.2.6 (NACMPDefault).
pub const ACCESS_PARAMETERS_MESSAGE_ID: u8 = 0x01;
/// `BroadcastReverseRateLimit.MessageID` per C.S0024-0 §8.5.6.3.3 (NRTCMPDefault).
pub const BROADCAST_REVERSE_RATE_LIMIT_MESSAGE_ID: u8 = 0x01;
/// `Sync.MessageID` per C.S0024-0 §6.3.6.2.1 (NISPDefault). Two-bit field set
/// to binary 00; the full 8-bit storage is zero.
pub const SYNC_MESSAGE_ID: u8 = 0x00;

/// Number of `APersistence` occurrences in an `AccessParameters` message
/// (NACMPAPersist, C.S0024-0 §8.3.8 protocol numeric constants).
pub const NACMP_A_PERSIST: usize = 4;

// =============================================================================
// QuickConfig — C.S0024-0 §6.8.6.2.1
// =============================================================================

/// QuickConfig message (C.S0024-0 §6.8.6.2.1).
///
/// Sent on the Control Channel by the Overhead Messages Protocol to indicate a
/// change in overhead-message contents and to provide frequently-changing
/// information.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct QuickConfig {
    /// 8-bit color code for this sector.
    pub color_code: u8,
    /// Least significant 24 bits of the 128-bit sector identifier.
    pub sector_id24: u32,
    /// Signature of the next SectorParameters message.
    pub sector_signature: u16,
    /// Signature of the current AccessParameters (Access Channel MAC).
    pub access_signature: u16,
    /// Access-network redirect flag.
    pub redirect: bool,
    /// Maximum number of RPC channels supported by the sector. 6-bit field.
    pub rpc_count: u8,
    /// Per-RPC `ForwardTrafficValid` flags. `forward_traffic_valid[n]` reports
    /// MACIndex `64 - n` per spec.
    pub forward_traffic_valid: Vec<bool>,
    /// Optional C.S0024-A/B extension count for MACIndex 64..127.
    pub rpc_count_127_to_64: Option<u8>,
    /// Optional C.S0024-A/B `ForwardTrafficValid127To64` flags.
    pub forward_traffic_valid_127_to_64: Vec<bool>,
    /// Optional C.S0024-B extension count for MACIndex 130..383.
    pub rpc_count_130_to_383: Option<u8>,
    /// Optional C.S0024-B `ForwardTrafficValid130To383` flags.
    pub forward_traffic_valid_130_to_383: Vec<bool>,
    /// Future revision bits after the known QuickConfig fields. C.S0024 says ATs
    /// ignore fields they do not recognize; retaining them lets live decodes be
    /// audited instead of misclassifying the body as invalid.
    pub trailing_extension_bits: Vec<u8>,
}

impl QuickConfig {
    /// Acquirable default: no redirect, no active forward-traffic MACs.
    pub fn defaults() -> Self {
        Self {
            color_code: 0x00,
            sector_id24: 0x000001,
            sector_signature: 0x0001,
            access_signature: 0x0001,
            redirect: false,
            rpc_count: 0,
            forward_traffic_valid: Vec::new(),
            rpc_count_127_to_64: None,
            forward_traffic_valid_127_to_64: Vec::new(),
            rpc_count_130_to_383: None,
            forward_traffic_valid_130_to_383: Vec::new(),
            trailing_extension_bits: Vec::new(),
        }
    }

    /// Encode the on-air message body (octet-aligned).
    pub fn encode(&self) -> Vec<u8> {
        let mut bs = Bitstream::new();
        bs.write_u8(QUICK_CONFIG_MESSAGE_ID, 8);
        bs.write_u8(self.color_code, 8);
        bs.write_u32(self.sector_id24 & 0x00FF_FFFF, 24);
        bs.write_u32(self.sector_signature as u32, 16);
        bs.write_u32(self.access_signature as u32, 16);
        bs.write_u8(self.redirect as u8, 1);
        bs.write_u8(self.rpc_count & 0x3F, 6);
        // RPCCount is the count field on the wire; the spec text says "RPCCount
        // occurrences of ForwardTrafficValid". We emit one bit per declared
        // entry up to rpc_count.
        let n = self.rpc_count as usize;
        for i in 0..n {
            let v = self.forward_traffic_valid.get(i).copied().unwrap_or(false);
            bs.write_u8(v as u8, 1);
        }
        let has_127_to_64 = self.rpc_count_127_to_64.is_some();
        let has_130_to_383 = self.rpc_count_130_to_383.is_some();
        if has_127_to_64 || has_130_to_383 || !self.trailing_extension_bits.is_empty() {
            bs.write_u8(has_127_to_64 as u8, 1);
            if let Some(count) = self.rpc_count_127_to_64 {
                bs.write_u8(count & 0x3F, 6);
                for i in 0..count as usize {
                    let v = self
                        .forward_traffic_valid_127_to_64
                        .get(i)
                        .copied()
                        .unwrap_or(false);
                    bs.write_u8(v as u8, 1);
                }
            }
            if has_127_to_64 || has_130_to_383 || !self.trailing_extension_bits.is_empty() {
                bs.write_u8(has_130_to_383 as u8, 1);
                if let Some(count) = self.rpc_count_130_to_383 {
                    bs.write_u8(count, 8);
                    for i in 0..count as usize {
                        let v = self
                            .forward_traffic_valid_130_to_383
                            .get(i)
                            .copied()
                            .unwrap_or(false);
                        bs.write_u8(v as u8, 1);
                    }
                }
            }
            for bit in &self.trailing_extension_bits {
                bs.write_u8(bit & 1, 1);
            }
        }
        pad_to_octet(&mut bs);
        bs.to_packed_bytes()
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let mut r = MessageBitReader::new(bytes);
        if r.read_u8(8)? != QUICK_CONFIG_MESSAGE_ID {
            return None;
        }
        let color_code = r.read_u8(8)?;
        let sector_id24 = r.read_u32(24)?;
        let sector_signature = r.read_u16(16)?;
        let access_signature = r.read_u16(16)?;
        let redirect = r.read_bool()?;
        let rpc_count = r.read_u8(6)?;
        let mut forward_traffic_valid = Vec::with_capacity(rpc_count as usize);
        for _ in 0..rpc_count {
            forward_traffic_valid.push(r.read_bool()?);
        }

        let mut rpc_count_127_to_64 = None;
        let mut forward_traffic_valid_127_to_64 = Vec::new();
        let mut rpc_count_130_to_383 = None;
        let mut forward_traffic_valid_130_to_383 = Vec::new();
        let mut trailing_extension_bits = Vec::new();

        if r.remaining() > 0 && !r.remaining_all_zero() {
            if r.read_bool()? {
                let count = r.read_u8(6)?;
                rpc_count_127_to_64 = Some(count);
                forward_traffic_valid_127_to_64 = Vec::with_capacity(count as usize);
                for _ in 0..count {
                    forward_traffic_valid_127_to_64.push(r.read_bool()?);
                }
            }
            if r.remaining() > 0 && !r.remaining_all_zero() {
                if r.read_bool()? {
                    let count = r.read_u8(8)?;
                    rpc_count_130_to_383 = Some(count);
                    forward_traffic_valid_130_to_383 = Vec::with_capacity(count as usize);
                    for _ in 0..count {
                        forward_traffic_valid_130_to_383.push(r.read_bool()?);
                    }
                }
            }
        }
        if r.remaining() > 0 {
            if r.remaining_all_zero() {
                r.expect_zero_padding()?;
            } else {
                trailing_extension_bits = r.read_remaining_bits()?;
            }
        }
        Some(Self {
            color_code,
            sector_id24,
            sector_signature,
            access_signature,
            redirect,
            rpc_count,
            forward_traffic_valid,
            rpc_count_127_to_64,
            forward_traffic_valid_127_to_64,
            rpc_count_130_to_383,
            forward_traffic_valid_130_to_383,
            trailing_extension_bits,
        })
    }
}

// =============================================================================
// SectorParameters — C.S0024-0 §6.8.6.2.2
// =============================================================================

/// 24-bit Channel record (C.S0024-0 §10.1).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelRecord {
    /// `SystemType`. `0x00` = HRPD per this spec.
    pub system_type: u8,
    /// `BandClass`, 5 bits.
    pub band_class: u8,
    /// `ChannelNumber`, 11 bits.
    pub channel_number: u16,
}

impl ChannelRecord {
    pub fn write(&self, bs: &mut Bitstream) {
        bs.write_u8(self.system_type, 8);
        bs.write_u8(self.band_class & 0x1F, 5);
        bs.write_u32((self.channel_number & 0x07FF) as u32, 11);
    }

    fn read(r: &mut MessageBitReader<'_>) -> Option<Self> {
        Some(Self {
            system_type: r.read_u8(8)?,
            band_class: r.read_u8(5)?,
            channel_number: r.read_u16(11)?,
        })
    }
}

/// Encode a decimal MCC as the three-digit BCD wire form required for
/// `SectorParameters.CountryCode` (C.S0024-A §8.9.6.2.2), e.g. 310 → 0x310.
fn mcc_to_bcd(mcc: u16) -> u16 {
    ((mcc / 100) % 10) << 8 | ((mcc / 10) % 10) << 4 | (mcc % 10)
}

fn bcd_to_mcc(bcd: u16) -> u16 {
    ((bcd >> 8) & 0xF) * 100 + ((bcd >> 4) & 0xF) * 10 + (bcd & 0xF)
}

/// Neighbor entry inside SectorParameters.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct NeighborEntry {
    /// `NeighborPilotPN`, 9 bits.
    pub pilot_pn: u16,
    /// Optional per-neighbor channel record.
    pub channel: Option<ChannelRecord>,
    /// `NeighborSearchWindowSize` index (0..=15) if the list-level
    /// `NeighborSearchWindowSizeIncluded` is set.
    pub search_window_size: Option<u8>,
    /// `NeighborSearchWindowOffset` index (0..=7) if the list-level
    /// `NeighborSearchWindowOffsetIncluded` is set.
    pub search_window_offset: Option<u8>,
}

/// SectorParameters message (C.S0024-0 §6.8.6.2.2).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SectorParameters {
    /// `CountryCode`: decimal MCC; encoded on the wire as three-digit BCD.
    pub country_code: u16,
    /// `SectorID`: 128-bit sector identifier.
    pub sector_id: [u8; 16],
    /// `SubnetMask`: number of consecutive 1-bits in the subnet mask.
    pub subnet_mask: u8,
    /// `SectorSignature`.
    pub sector_signature: u16,
    /// `Latitude` in 0.25-second units, two's complement, range -1_296_000 ..= 1_296_000.
    pub latitude: i32,
    /// `Longitude` in 0.25-second units, two's complement, range -2_592_000 ..= 2_592_000.
    pub longitude: i32,
    /// `RouteUpdateRadius`, 11 bits. Zero disables distance-based route updates.
    pub route_update_radius: u16,
    /// `LeapSeconds`.
    pub leap_seconds: u8,
    /// `LocalTimeOffset` in minutes, 11-bit two's complement.
    pub local_time_offset: i16,
    /// `ReverseLinkSilenceDuration` in frames (2-bit code).
    pub reverse_link_silence_duration: u8,
    /// `ReverseLinkSilencePeriod` (2-bit code).
    pub reverse_link_silence_period: u8,
    /// Active CDMA channels for this sector.
    pub channels: Vec<ChannelRecord>,
    /// Neighbor list (may be empty).
    pub neighbors: Vec<NeighborEntry>,
}

impl SectorParameters {
    /// Acquirable default: empty neighbor list, single HRPD channel,
    /// sentinel zero geolocation.
    pub fn defaults() -> Self {
        Self {
            country_code: 310, // MCC 310 (US)
            sector_id: [0u8; 16],
            subnet_mask: 104, // typical /104 HRPD subnet
            sector_signature: 0x0001,
            latitude: 0,
            longitude: 0,
            route_update_radius: 0,
            leap_seconds: 0,
            local_time_offset: 0,
            reverse_link_silence_duration: 0,
            reverse_link_silence_period: 0,
            channels: vec![ChannelRecord {
                system_type: 0x00,
                band_class: 0,
                channel_number: 25, // BC0 channel
            }],
            neighbors: Vec::new(),
        }
    }

    /// Add a 1x partner-sector advertisement as a Neighbor entry with a
    /// cross-system channel record (`system_type = 0x01`, IS-2000 1x). This
    /// is how A.S0019-A Hybrid AT operation tells the HRPD AT which 1x
    /// sector to camp on for cross-paging / voice fallback.
    pub fn with_one_x_neighbor(
        mut self,
        band_class: u8,
        channel_number: u16,
        pilot_pn: u16,
    ) -> Self {
        self.neighbors.push(NeighborEntry {
            pilot_pn: pilot_pn & 0x01FF,
            channel: Some(ChannelRecord {
                system_type: 0x01, // 1x (IS-2000)
                band_class: band_class & 0x1F,
                channel_number: channel_number & 0x07FF,
            }),
            search_window_size: None,
            search_window_offset: None,
        });
        self
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut bs = Bitstream::new();
        bs.write_u8(SECTOR_PARAMETERS_MESSAGE_ID, 8);
        bs.write_u32(mcc_to_bcd(self.country_code) as u32, 12);
        for byte in self.sector_id.iter() {
            bs.write_u8(*byte, 8);
        }
        bs.write_u8(self.subnet_mask, 8);
        bs.write_u32(self.sector_signature as u32, 16);
        bs.write_u32(twos_complement(self.latitude as i64, 22), 22);
        bs.write_u32(twos_complement(self.longitude as i64, 23), 23);
        bs.write_u32((self.route_update_radius & 0x07FF) as u32, 11);
        bs.write_u8(self.leap_seconds, 8);
        bs.write_u32(twos_complement(self.local_time_offset as i64, 11), 11);
        bs.write_u8(self.reverse_link_silence_duration & 0x03, 2);
        bs.write_u8(self.reverse_link_silence_period & 0x03, 2);

        // ChannelCount (5) + records.
        let channel_count = self.channels.len().min(31) as u8;
        bs.write_u8(channel_count, 5);
        for ch in self.channels.iter().take(channel_count as usize) {
            ch.write(&mut bs);
        }

        // NeighborCount (5) + per-neighbor PNs.
        let neighbor_count = self.neighbors.len().min(31);
        bs.write_u8(neighbor_count as u8, 5);
        for n in self.neighbors.iter().take(neighbor_count) {
            bs.write_u32((n.pilot_pn & 0x01FF) as u32, 9);
        }

        // C.S0024-A §8.9.6.2.2: NeighborChannelIncluded and NeighborChannel
        // are one interleaved record per neighbor ({flag, record}, ...), not
        // a flag vector followed by the included records.
        for n in self.neighbors.iter().take(neighbor_count) {
            bs.write_u8(n.channel.is_some() as u8, 1);
            if let Some(ch) = n.channel {
                ch.write(&mut bs);
            }
        }

        // NeighborSearchWindowSizeIncluded list-level flag, then per-neighbor
        // 4-bit search-window-size values when included.
        let sws_included = self
            .neighbors
            .iter()
            .take(neighbor_count)
            .any(|n| n.search_window_size.is_some());
        bs.write_u8(sws_included as u8, 1);
        if sws_included {
            for n in self.neighbors.iter().take(neighbor_count) {
                let v = n.search_window_size.unwrap_or(0);
                bs.write_u8(v & 0x0F, 4);
            }
        }

        // NeighborSearchWindowOffsetIncluded list-level flag, then per-neighbor
        // 3-bit search-window-offset values when included.
        let swo_included = self
            .neighbors
            .iter()
            .take(neighbor_count)
            .any(|n| n.search_window_offset.is_some());
        bs.write_u8(swo_included as u8, 1);
        if swo_included {
            for n in self.neighbors.iter().take(neighbor_count) {
                let v = n.search_window_offset.unwrap_or(0);
                bs.write_u8(v & 0x07, 3);
            }
        }

        pad_to_octet(&mut bs);
        bs.to_packed_bytes()
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let mut r = MessageBitReader::new(bytes);
        if r.read_u8(8)? != SECTOR_PARAMETERS_MESSAGE_ID {
            return None;
        }
        let country_code = bcd_to_mcc(r.read_u16(12)?);
        let mut sector_id = [0u8; 16];
        for byte in &mut sector_id {
            *byte = r.read_u8(8)?;
        }
        let subnet_mask = r.read_u8(8)?;
        let sector_signature = r.read_u16(16)?;
        let latitude = r.read_i32(22)?;
        let longitude = r.read_i32(23)?;
        let route_update_radius = r.read_u16(11)?;
        let leap_seconds = r.read_u8(8)?;
        let local_time_offset = r.read_i16(11)?;
        let reverse_link_silence_duration = r.read_u8(2)?;
        let reverse_link_silence_period = r.read_u8(2)?;

        let channel_count = r.read_usize(5)?;
        let mut channels = Vec::with_capacity(channel_count);
        for _ in 0..channel_count {
            channels.push(ChannelRecord::read(&mut r)?);
        }

        let neighbor_count = r.read_usize(5)?;
        let mut neighbors = Vec::with_capacity(neighbor_count);
        for _ in 0..neighbor_count {
            neighbors.push(NeighborEntry {
                pilot_pn: r.read_u16(9)?,
                channel: None,
                search_window_size: None,
                search_window_offset: None,
            });
        }

        // Interleaved {NeighborChannelIncluded, NeighborChannel} per neighbor
        // (C.S0024-A §8.9.6.2.2).
        for neighbor in neighbors.iter_mut() {
            if r.read_bool()? {
                neighbor.channel = Some(ChannelRecord::read(&mut r)?);
            }
        }

        if r.read_bool()? {
            for neighbor in &mut neighbors {
                neighbor.search_window_size = Some(r.read_u8(4)?);
            }
        }
        if r.read_bool()? {
            for neighbor in &mut neighbors {
                neighbor.search_window_offset = Some(r.read_u8(3)?);
            }
        }
        r.expect_zero_padding()?;

        Some(Self {
            country_code,
            sector_id,
            subnet_mask,
            sector_signature,
            latitude,
            longitude,
            route_update_radius,
            leap_seconds,
            local_time_offset,
            reverse_link_silence_duration,
            reverse_link_silence_period,
            channels,
            neighbors,
        })
    }
}

// =============================================================================
// AccessParameters — C.S0024-0 §8.3.6.2.6
// =============================================================================

/// Rev A enhanced AccessParameters fields (C.S0024-A §10.5.6.2.6), present on
/// the wire only when `EnhancedAccessParametersIncluded` is `'1'`. Each field
/// stores the raw wire code; the accessors map codes to engineering units per
/// Tables 10.5.6.2.6-1 through 10.5.6.2.6-7.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EnhancedAccessParameters {
    /// `PreambleLengthSlots`, 1 bit: '0' = 4 slots, '1' = 16 slots.
    pub preamble_length_slots: u8,
    /// `AccessOffset`, 2 bits: probe-start slot offset = code × 4 slots.
    pub access_offset: u8,
    /// `SectorAccessMaxRate`, 2 bits: '00' 9.6, '01' 19.2, '10' 38.4 kbps.
    pub sector_access_max_rate: u8,
    /// `ProbeTimeOutAdjust`, 3 bits: code × 16 slots.
    pub probe_time_out_adjust: u8,
    /// `PilotStrengthNominal`, 3 bits.
    pub pilot_strength_nominal: u8,
    /// `PilotStrengthCorrectionMin`, 3 bits.
    pub pilot_strength_correction_min: u8,
    /// `PilotStrengthCorrectionMax`, 3 bits.
    pub pilot_strength_correction_max: u8,
}

/// `SectorAccessMaxRate` code for 9.6 kbps (Table 10.5.6.2.6-3).
pub const SECTOR_ACCESS_MAX_RATE_9K6: u8 = 0b00;
/// `SectorAccessMaxRate` code for 19.2 kbps (Table 10.5.6.2.6-3).
pub const SECTOR_ACCESS_MAX_RATE_19K2: u8 = 0b01;
/// `SectorAccessMaxRate` code for 38.4 kbps (Table 10.5.6.2.6-3).
pub const SECTOR_ACCESS_MAX_RATE_38K4: u8 = 0b10;

impl EnhancedAccessParameters {
    /// Preamble length in slots (Table 10.5.6.2.6-1).
    pub const fn preamble_slots(&self) -> u32 {
        if self.preamble_length_slots & 1 == 0 {
            4
        } else {
            16
        }
    }

    /// Probe-start offset in slots (Table 10.5.6.2.6-2).
    pub const fn access_offset_slots(&self) -> u32 {
        ((self.access_offset & 0x03) as u32) * 4
    }

    /// Maximum Access Channel capsule data rate in bps (Table 10.5.6.2.6-3).
    /// `None` for the reserved code.
    pub const fn sector_access_max_rate_bps(&self) -> Option<u32> {
        match self.sector_access_max_rate & 0x03 {
            SECTOR_ACCESS_MAX_RATE_9K6 => Some(9_600),
            SECTOR_ACCESS_MAX_RATE_19K2 => Some(19_200),
            SECTOR_ACCESS_MAX_RATE_38K4 => Some(38_400),
            _ => None,
        }
    }

    /// Probe timeout adjustment in slots (Table 10.5.6.2.6-4).
    pub const fn probe_time_out_adjust_slots(&self) -> u32 {
        ((self.probe_time_out_adjust & 0x07) as u32) * 16
    }

    /// `PilotStrengthNominal` in dB (Table 10.5.6.2.6-5).
    pub const fn pilot_strength_nominal_db(&self) -> i8 {
        match self.pilot_strength_nominal & 0x07 {
            0b000 => 0,
            0b001 => -1,
            0b010 => -2,
            0b011 => -3,
            0b100 => -4,
            0b101 => 1,
            0b110 => 2,
            _ => 3,
        }
    }

    /// `PilotStrengthCorrectionMin` in dB (Table 10.5.6.2.6-6). `None` for
    /// reserved codes.
    pub const fn pilot_strength_correction_min_db(&self) -> Option<i8> {
        match self.pilot_strength_correction_min & 0x07 {
            code @ 0b000..=0b101 => Some(-(code as i8)),
            _ => None,
        }
    }

    /// `PilotStrengthCorrectionMax` in dB (Table 10.5.6.2.6-7). `None` for
    /// reserved codes.
    pub const fn pilot_strength_correction_max_db(&self) -> Option<i8> {
        match self.pilot_strength_correction_max & 0x07 {
            code @ 0b000..=0b101 => Some(code as i8),
            _ => None,
        }
    }
}

/// AccessParameters message (C.S0024-0 §8.3.6.2.6; enhanced Rev A fields per
/// C.S0024-A §10.5.6.2.6).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessParameters {
    /// Duration of an Access Channel Cycle in slots (8 bits).
    pub access_cycle_duration: u8,
    /// AccessParameters message signature (16 bits).
    pub access_signature: u16,
    /// Open-loop adjust, 8-bit unsigned (the AT applies `-1×` this value).
    pub open_loop_adjust: u8,
    /// `ProbeInitialAdjust`, 5-bit two's complement (-16..=15) in 1 dB units.
    pub probe_initial_adjust: i8,
    /// `ProbeNumStep`, 4 bits, range [1..=15].
    pub probe_num_step: u8,
    /// `PowerStep`, 4 bits, units 0.5 dB.
    pub power_step: u8,
    /// `PreambleLength`, 3 bits, in frames. Ignored by the AT when
    /// `PreambleLengthSlots` is present in the enhanced fields.
    pub preamble_length: u8,
    /// `CapsuleLengthMax`, 4 bits, range [2..=15].
    pub capsule_length_max: u8,
    /// `APersistence` vector. Length must equal [`NACMP_A_PERSIST`] (= 4).
    /// Each value is 6 bits; 0x3F maps to persistence probability zero.
    pub a_persistence: [u8; NACMP_A_PERSIST],
    /// Rev A enhanced fields, gated on the wire by
    /// `EnhancedAccessParametersIncluded`. `None` keeps the Rev 0 wire form.
    pub enhanced: Option<EnhancedAccessParameters>,
}

impl AccessParameters {
    /// Acquirable default: 256-slot cycle (= one Control Channel cycle),
    /// short preamble, mid-range power params, all persistence slots open
    /// (= 2^0 = 1 since field value 0 means probability 2^0).
    pub fn defaults() -> Self {
        Self {
            access_cycle_duration: 16,
            access_signature: 0x0001,
            open_loop_adjust: 81, // typical -81 dBm-style open-loop offset
            probe_initial_adjust: 0,
            probe_num_step: 6,
            power_step: 2, // 1.0 dB steps
            preamble_length: 3,
            capsule_length_max: 8,
            a_persistence: [0, 0, 0, 0],
            enhanced: None,
        }
    }

    /// Effective probe preamble length in slots: `PreambleLengthSlots` when
    /// the enhanced fields are present, else `PreambleLength × 16`
    /// (C.S0024-A §10.5.6.1.4.1.2 rule 7).
    pub fn preamble_length_slots(&self) -> u32 {
        match &self.enhanced {
            Some(enhanced) => enhanced.preamble_slots(),
            None => u32::from(self.preamble_length & 0x07) * 16,
        }
    }

    /// Effective probe-start `AccessOffset` in slots; zero when the enhanced
    /// fields are absent.
    pub fn access_offset_slots(&self) -> u32 {
        self.enhanced
            .as_ref()
            .map(|enhanced| enhanced.access_offset_slots())
            .unwrap_or(0)
    }

    /// Effective maximum Access Channel capsule data rate in bps; 9.6 kbps
    /// when the enhanced fields are absent or the rate code is reserved.
    pub fn sector_access_max_rate_bps(&self) -> u32 {
        self.enhanced
            .as_ref()
            .and_then(|enhanced| enhanced.sector_access_max_rate_bps())
            .unwrap_or(9_600)
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut bs = Bitstream::new();
        bs.write_u8(ACCESS_PARAMETERS_MESSAGE_ID, 8);
        bs.write_u8(self.access_cycle_duration, 8);
        bs.write_u32(self.access_signature as u32, 16);
        bs.write_u8(self.open_loop_adjust, 8);
        bs.write_u32(twos_complement(self.probe_initial_adjust as i64, 5), 5);
        bs.write_u8(self.probe_num_step & 0x0F, 4);
        bs.write_u8(self.power_step & 0x0F, 4);
        bs.write_u8(self.preamble_length & 0x07, 3);
        bs.write_u8(self.capsule_length_max & 0x0F, 4);
        for v in self.a_persistence.iter() {
            bs.write_u8(*v & 0x3F, 6);
        }
        if let Some(enhanced) = &self.enhanced {
            bs.write_u8(1, 1); // EnhancedAccessParametersIncluded
            bs.write_u8(enhanced.preamble_length_slots & 0x01, 1);
            bs.write_u8(enhanced.access_offset & 0x03, 2);
            bs.write_u8(enhanced.sector_access_max_rate & 0x03, 2);
            bs.write_u8(enhanced.probe_time_out_adjust & 0x07, 3);
            bs.write_u8(enhanced.pilot_strength_nominal & 0x07, 3);
            bs.write_u8(enhanced.pilot_strength_correction_min & 0x07, 3);
            bs.write_u8(enhanced.pilot_strength_correction_max & 0x07, 3);
        }
        pad_to_octet(&mut bs);
        bs.to_packed_bytes()
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let mut r = MessageBitReader::new(bytes);
        if r.read_u8(8)? != ACCESS_PARAMETERS_MESSAGE_ID {
            return None;
        }
        let access_cycle_duration = r.read_u8(8)?;
        let access_signature = r.read_u16(16)?;
        let open_loop_adjust = r.read_u8(8)?;
        let probe_initial_adjust = r.read_i8(5)?;
        let probe_num_step = r.read_u8(4)?;
        let power_step = r.read_u8(4)?;
        let preamble_length = r.read_u8(3)?;
        let capsule_length_max = r.read_u8(4)?;
        let mut a_persistence = [0u8; NACMP_A_PERSIST];
        for value in &mut a_persistence {
            *value = r.read_u8(6)?;
        }
        // A Rev 0 body ends here in zero padding; the enhanced fields follow
        // only when EnhancedAccessParametersIncluded reads as '1'. A '0' flag
        // followed by only zero Reserved bits is bit-identical to Rev 0
        // padding, so `enhanced = None` covers both.
        let enhanced = if r.remaining() > 0 && !r.remaining_all_zero() {
            if !r.read_bool()? {
                return None;
            }
            Some(EnhancedAccessParameters {
                preamble_length_slots: r.read_u8(1)?,
                access_offset: r.read_u8(2)?,
                sector_access_max_rate: r.read_u8(2)?,
                probe_time_out_adjust: r.read_u8(3)?,
                pilot_strength_nominal: r.read_u8(3)?,
                pilot_strength_correction_min: r.read_u8(3)?,
                pilot_strength_correction_max: r.read_u8(3)?,
            })
        } else {
            None
        };
        r.expect_zero_padding()?;
        Some(Self {
            access_cycle_duration,
            access_signature,
            open_loop_adjust,
            probe_initial_adjust,
            probe_num_step,
            power_step,
            preamble_length,
            capsule_length_max,
            a_persistence,
            enhanced,
        })
    }
}

// =============================================================================
// BroadcastReverseRateLimit — C.S0024-0 §8.5.6.3.3
// =============================================================================

/// BroadcastReverseRateLimit message (C.S0024-0 §8.5.6.3.3).
///
/// Carried by the Default Reverse Traffic Channel MAC Protocol to advertise
/// reverse-link rate limits per RPC/MAC index.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BroadcastReverseRateLimit {
    /// Number of `RateLimit` occurrences.
    pub rpc_count: u8,
    /// Four-bit `RateLimit` entries. Value `0x5` means 153.6 kbps in Rev 0.
    pub rate_limit: Vec<u8>,
}

impl BroadcastReverseRateLimit {
    pub fn encode(&self) -> Vec<u8> {
        let mut bs = Bitstream::new();
        bs.write_u8(BROADCAST_REVERSE_RATE_LIMIT_MESSAGE_ID, 8);
        bs.write_u8(self.rpc_count & 0x3F, 6);
        for i in 0..self.rpc_count as usize {
            let v = self.rate_limit.get(i).copied().unwrap_or(0);
            bs.write_u8(v & 0x0F, 4);
        }
        pad_to_octet(&mut bs);
        bs.to_packed_bytes()
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let mut r = MessageBitReader::new(bytes);
        if r.read_u8(8)? != BROADCAST_REVERSE_RATE_LIMIT_MESSAGE_ID {
            return None;
        }
        let rpc_count = r.read_u8(6)?;
        let mut rate_limit = Vec::with_capacity(rpc_count as usize);
        for _ in 0..rpc_count {
            rate_limit.push(r.read_u8(4)?);
        }
        r.expect_zero_padding()?;
        Some(Self {
            rpc_count,
            rate_limit,
        })
    }
}

// =============================================================================
// Sync — C.S0024-0 §6.3.6.2.1
// =============================================================================

/// Sync message (C.S0024-0 §6.3.6.2.1). Carried on the Control Channel by the
/// Default Initialization State Protocol.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SyncMessage {
    /// Maximum supported air-interface protocol revision.
    pub maximum_revision: u8,
    /// Minimum supported air-interface protocol revision (≤ `maximum_revision`).
    pub minimum_revision: u8,
    /// Pilot PN offset in units of 64 PN chips, 9 bits.
    pub pilot_pn: u16,
    /// System time 160 ms after the start of the Control Channel cycle in
    /// which the message is sent, in units of 26.66... ms. 37 bits.
    pub system_time: u64,
}

impl SyncMessage {
    /// Acquirable default. Note `MessageID` is a 2-bit field (= binary 00);
    /// the encoded body therefore starts with a 2-bit zero MessageID followed
    /// directly by `MaximumRevision`.
    ///
    /// The revision number is 0x01 for the whole IS-856 family: C.S0024-0,
    /// -A, and -B each mandate it in their §1.15, so 0x01/0x01 admits any
    /// compliant terminal. An AT only uses the network when its own revision
    /// falls within [MinimumRevision, MaximumRevision].
    pub fn defaults() -> Self {
        Self {
            maximum_revision: 1,
            minimum_revision: 1,
            pilot_pn: 0,
            system_time: 0,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut bs = Bitstream::new();
        // MessageID is 2 bits in this message (Initialization State Protocol
        // uses a 2-bit type tag, not the 8-bit OMP/ACMP-style MessageID).
        bs.write_u8(SYNC_MESSAGE_ID & 0x03, 2);
        bs.write_u8(self.maximum_revision, 8);
        bs.write_u8(self.minimum_revision, 8);
        bs.write_u32((self.pilot_pn & 0x01FF) as u32, 9);
        bs.write_u64(self.system_time & ((1u64 << 37) - 1), 37);
        pad_to_octet(&mut bs);
        bs.to_packed_bytes()
    }

    pub fn decode(bytes: &[u8]) -> Option<Self> {
        let mut r = MessageBitReader::new(bytes);
        if r.read_u8(2)? != (SYNC_MESSAGE_ID & 0x03) {
            return None;
        }
        let maximum_revision = r.read_u8(8)?;
        let minimum_revision = r.read_u8(8)?;
        let pilot_pn = r.read_u16(9)?;
        let system_time = r.read_u64(37)?;
        if minimum_revision > maximum_revision || pilot_pn > 511 {
            return None;
        }
        r.expect_zero_padding()?;
        Some(Self {
            maximum_revision,
            minimum_revision,
            pilot_pn,
            system_time,
        })
    }
}

// =============================================================================
// Discriminated union
// =============================================================================

/// Discriminated union over the overhead-message bodies needed for HRPD AT
/// acquisition.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HrpdOverheadMessage {
    QuickConfig(QuickConfig),
    SectorParameters(SectorParameters),
    AccessParameters(AccessParameters),
    BroadcastReverseRateLimit(BroadcastReverseRateLimit),
    Sync(SyncMessage),
}

impl HrpdOverheadMessage {
    pub fn encode(&self) -> Vec<u8> {
        match self {
            HrpdOverheadMessage::QuickConfig(m) => m.encode(),
            HrpdOverheadMessage::SectorParameters(m) => m.encode(),
            HrpdOverheadMessage::AccessParameters(m) => m.encode(),
            HrpdOverheadMessage::BroadcastReverseRateLimit(m) => m.encode(),
            HrpdOverheadMessage::Sync(m) => m.encode(),
        }
    }

    pub fn decode_for_protocol(protocol_type: u8, bytes: &[u8]) -> Option<Self> {
        match protocol_type {
            OVERHEAD_MESSAGES_PROTOCOL_TYPE => {
                if bytes.first().copied()? == QUICK_CONFIG_MESSAGE_ID {
                    QuickConfig::decode(bytes).map(HrpdOverheadMessage::QuickConfig)
                } else if bytes.first().copied()? == SECTOR_PARAMETERS_MESSAGE_ID {
                    SectorParameters::decode(bytes).map(HrpdOverheadMessage::SectorParameters)
                } else {
                    None
                }
            }
            DEFAULT_ACCESS_CHANNEL_MAC_PROTOCOL_TYPE => {
                AccessParameters::decode(bytes).map(HrpdOverheadMessage::AccessParameters)
            }
            DEFAULT_REVERSE_TRAFFIC_CHANNEL_MAC_PROTOCOL_TYPE => {
                BroadcastReverseRateLimit::decode(bytes)
                    .map(HrpdOverheadMessage::BroadcastReverseRateLimit)
            }
            DEFAULT_INITIALIZATION_STATE_PROTOCOL_TYPE => {
                SyncMessage::decode(bytes).map(HrpdOverheadMessage::Sync)
            }
            _ => None,
        }
    }
}

// =============================================================================
// Helpers
// =============================================================================

fn pad_to_octet(bs: &mut Bitstream) {
    let rem = bs.len() % 8;
    if rem != 0 {
        bs.write_u64(0, 8 - rem);
    }
}

/// Encode `value` as a `width`-bit two's-complement field, returned in the low
/// `width` bits of a `u32`.
fn twos_complement(value: i64, width: u32) -> u32 {
    debug_assert!(width > 0 && width <= 32);
    let mask: u64 = if width == 64 {
        !0u64
    } else {
        (1u64 << width) - 1
    };
    (value as u64 & mask) as u32
}

#[derive(Debug, Clone)]
struct MessageBitReader<'a> {
    bytes: &'a [u8],
    bit_pos: usize,
}

impl<'a> MessageBitReader<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, bit_pos: 0 }
    }

    fn remaining(&self) -> usize {
        self.bytes.len() * 8 - self.bit_pos
    }

    fn read_u64(&mut self, width: usize) -> Option<u64> {
        if width > 64 || width > self.remaining() {
            return None;
        }
        let mut out = 0u64;
        for _ in 0..width {
            let byte = self.bytes[self.bit_pos / 8];
            let bit = (byte >> (7 - (self.bit_pos % 8))) & 1;
            out = (out << 1) | u64::from(bit);
            self.bit_pos += 1;
        }
        Some(out)
    }

    fn read_u32(&mut self, width: usize) -> Option<u32> {
        self.read_u64(width).map(|v| v as u32)
    }

    fn read_u16(&mut self, width: usize) -> Option<u16> {
        self.read_u64(width).map(|v| v as u16)
    }

    fn read_u8(&mut self, width: usize) -> Option<u8> {
        self.read_u64(width).map(|v| v as u8)
    }

    fn read_usize(&mut self, width: usize) -> Option<usize> {
        self.read_u64(width).map(|v| v as usize)
    }

    fn read_bool(&mut self) -> Option<bool> {
        self.read_u8(1).map(|v| v != 0)
    }

    fn read_remaining_bits(&mut self) -> Option<Vec<u8>> {
        let mut out = Vec::with_capacity(self.remaining());
        while self.remaining() > 0 {
            out.push(self.read_u8(1)?);
        }
        Some(out)
    }

    fn read_i32(&mut self, width: usize) -> Option<i32> {
        self.read_signed(width).map(|v| v as i32)
    }

    fn read_i16(&mut self, width: usize) -> Option<i16> {
        self.read_signed(width).map(|v| v as i16)
    }

    fn read_i8(&mut self, width: usize) -> Option<i8> {
        self.read_signed(width).map(|v| v as i8)
    }

    fn read_signed(&mut self, width: usize) -> Option<i64> {
        if width == 0 || width > 63 {
            return None;
        }
        let raw = self.read_u64(width)?;
        let sign_bit = 1u64 << (width - 1);
        if raw & sign_bit == 0 {
            Some(raw as i64)
        } else {
            Some(raw as i64 - (1i64 << width))
        }
    }

    fn expect_zero_padding(&mut self) -> Option<()> {
        while self.remaining() > 0 {
            if self.read_u8(1)? != 0 {
                return None;
            }
        }
        Some(())
    }

    fn remaining_all_zero(&self) -> bool {
        let mut bit_pos = self.bit_pos;
        while bit_pos < self.bytes.len() * 8 {
            let byte = self.bytes[bit_pos / 8];
            let bit = (byte >> (7 - (bit_pos % 8))) & 1;
            if bit != 0 {
                return false;
            }
            bit_pos += 1;
        }
        true
    }
}

// =============================================================================
// Tests
// =============================================================================

#[cfg(test)]
mod tests {
    use super::*;

    /// Tiny MSB-first bit reader for tests.
    struct BitReader<'a> {
        bytes: &'a [u8],
        bit_pos: usize,
    }

    impl<'a> BitReader<'a> {
        fn new(bytes: &'a [u8]) -> Self {
            Self { bytes, bit_pos: 0 }
        }
        fn read(&mut self, width: usize) -> u64 {
            let mut v = 0u64;
            for _ in 0..width {
                let byte = self.bytes[self.bit_pos / 8];
                let bit = (byte >> (7 - (self.bit_pos % 8))) & 1;
                v = (v << 1) | bit as u64;
                self.bit_pos += 1;
            }
            v
        }
    }

    fn hex_bytes(hex: &str) -> Vec<u8> {
        hex.as_bytes()
            .chunks_exact(2)
            .map(|pair| {
                let text = std::str::from_utf8(pair).expect("hex pair is utf-8");
                u8::from_str_radix(text, 16).expect("hex pair parses")
            })
            .collect()
    }

    // ----- length checks --------------------------------------------------------

    #[test]
    fn quick_config_default_length_is_octet_aligned() {
        let bytes = QuickConfig::defaults().encode();
        // Fixed header bits: 8 + 8 + 24 + 16 + 16 + 1 + 6 = 79 bits.
        // rpc_count = 0, so 0 ForwardTrafficValid bits.
        // Padded up to 80 bits = 10 octets.
        assert_eq!(bytes.len(), 10);
    }

    #[test]
    fn quick_config_with_rpcs_length() {
        let mut qc = QuickConfig::defaults();
        qc.rpc_count = 3;
        qc.forward_traffic_valid = vec![true, false, true];
        let bytes = qc.encode();
        // 79 + 3 = 82 bits -> 11 octets.
        assert_eq!(bytes.len(), 11);
    }

    #[test]
    fn sector_parameters_default_length() {
        let bytes = SectorParameters::defaults().encode();
        // 8 + 12 + 128 + 8 + 16 + 22 + 23 + 11 + 8 + 11 + 2 + 2 = 251
        // + ChannelCount(5) + 1 channel record(24) = 280
        // + NeighborCount(5) + 0 neighbors                  = 285
        // + NeighborSearchWindowSizeIncluded(1)             = 286
        // + NeighborSearchWindowOffsetIncluded(1)           = 287
        // Pad to 288 = 36 octets.
        assert_eq!(bytes.len(), 36);
    }

    #[test]
    fn access_parameters_default_length() {
        let bytes = AccessParameters::defaults().encode();
        // 8 + 8 + 16 + 8 + 5 + 4 + 4 + 3 + 4 + 4*6 = 84 bits -> 11 octets.
        assert_eq!(bytes.len(), 11);
    }

    #[test]
    fn broadcast_reverse_rate_limit_default_length() {
        let bytes = BroadcastReverseRateLimit {
            rpc_count: 1,
            rate_limit: vec![5],
        }
        .encode();
        // 8 + 6 + 4 = 18 bits -> 3 octets.
        assert_eq!(bytes.len(), 3);
    }

    #[test]
    fn sync_default_length() {
        let bytes = SyncMessage::defaults().encode();
        // 2 + 8 + 8 + 9 + 37 = 64 bits -> 8 octets, no padding needed.
        assert_eq!(bytes.len(), 8);
    }

    // ----- field-placement spot-checks -----------------------------------------

    #[test]
    fn quick_config_field_offsets() {
        let qc = QuickConfig {
            color_code: 0xA5,
            sector_id24: 0x12_3456,
            sector_signature: 0xBEEF,
            access_signature: 0xCAFE,
            redirect: true,
            rpc_count: 2,
            forward_traffic_valid: vec![true, false],
            ..QuickConfig::defaults()
        };
        let bytes = qc.encode();
        let mut r = BitReader::new(&bytes);
        assert_eq!(r.read(8), QUICK_CONFIG_MESSAGE_ID as u64);
        assert_eq!(r.read(8), 0xA5);
        assert_eq!(r.read(24), 0x12_3456);
        assert_eq!(r.read(16), 0xBEEF);
        assert_eq!(r.read(16), 0xCAFE);
        assert_eq!(r.read(1), 1); // redirect
        assert_eq!(r.read(6), 2); // rpc_count
        assert_eq!(r.read(1), 1); // FTV[0]
        assert_eq!(r.read(1), 0); // FTV[1]
    }

    #[test]
    fn sector_parameters_field_offsets() {
        let mut sp = SectorParameters::defaults();
        sp.country_code = 310;
        sp.sector_id = [
            0x01, 0x02, 0x03, 0x04, 0x05, 0x06, 0x07, 0x08, 0x09, 0x0A, 0x0B, 0x0C, 0x0D, 0x0E,
            0x0F, 0x10,
        ];
        sp.subnet_mask = 104;
        sp.sector_signature = 0x1234;
        sp.latitude = -1;
        sp.longitude = 2;
        sp.leap_seconds = 18;
        sp.local_time_offset = -420;
        let bytes = sp.encode();
        let mut r = BitReader::new(&bytes);
        assert_eq!(r.read(8), SECTOR_PARAMETERS_MESSAGE_ID as u64);
        // MCC 310 as three-digit BCD.
        assert_eq!(r.read(12), 0x310);
        for byte in &sp.sector_id {
            assert_eq!(r.read(8), *byte as u64);
        }
        assert_eq!(r.read(8), 104);
        assert_eq!(r.read(16), 0x1234);
        // latitude (22-bit two's complement of -1) = all-ones.
        assert_eq!(r.read(22), (1u64 << 22) - 1);
        // longitude (23-bit two's complement of 2).
        assert_eq!(r.read(23), 2);
        assert_eq!(r.read(11), 0); // route update radius
        assert_eq!(r.read(8), 18); // leap seconds
        assert_eq!(r.read(11), 0x65C); // -420 minutes in 11-bit two's complement
    }

    #[test]
    fn access_parameters_field_offsets() {
        let mut ap = AccessParameters::defaults();
        ap.access_cycle_duration = 32;
        ap.access_signature = 0xABCD;
        ap.open_loop_adjust = 75;
        ap.probe_initial_adjust = -1; // 5-bit two's complement = 0b11111 = 31
        ap.probe_num_step = 6;
        ap.power_step = 2;
        ap.preamble_length = 3;
        ap.capsule_length_max = 8;
        ap.a_persistence = [0x3F, 0x10, 0x00, 0x20];
        let bytes = ap.encode();
        let mut r = BitReader::new(&bytes);
        assert_eq!(r.read(8), ACCESS_PARAMETERS_MESSAGE_ID as u64);
        assert_eq!(r.read(8), 32);
        assert_eq!(r.read(16), 0xABCD);
        assert_eq!(r.read(8), 75);
        assert_eq!(r.read(5), 0x1F); // -1 two's complement
        assert_eq!(r.read(4), 6);
        assert_eq!(r.read(4), 2);
        assert_eq!(r.read(3), 3);
        assert_eq!(r.read(4), 8);
        assert_eq!(r.read(6), 0x3F);
        assert_eq!(r.read(6), 0x10);
        assert_eq!(r.read(6), 0x00);
        assert_eq!(r.read(6), 0x20);
    }

    #[test]
    fn access_parameters_enhanced_field_offsets_and_length() {
        let mut ap = AccessParameters::defaults();
        ap.enhanced = Some(EnhancedAccessParameters {
            preamble_length_slots: 1,
            access_offset: 0b10,
            sector_access_max_rate: SECTOR_ACCESS_MAX_RATE_38K4,
            probe_time_out_adjust: 0b011,
            pilot_strength_nominal: 0b101,
            pilot_strength_correction_min: 0b010,
            pilot_strength_correction_max: 0b100,
        });
        let bytes = ap.encode();
        // Rev 0 body (84 bits) + flag(1) + 1 + 2 + 2 + 3 + 3 + 3 + 3 = 102
        // bits -> 13 octets.
        assert_eq!(bytes.len(), 13);
        let mut r = BitReader::new(&bytes);
        r.read(84); // Rev 0 fields
        assert_eq!(r.read(1), 1); // EnhancedAccessParametersIncluded
        assert_eq!(r.read(1), 1); // PreambleLengthSlots
        assert_eq!(r.read(2), 0b10); // AccessOffset
        assert_eq!(r.read(2), 0b10); // SectorAccessMaxRate
        assert_eq!(r.read(3), 0b011); // ProbeTimeOutAdjust
        assert_eq!(r.read(3), 0b101); // PilotStrengthNominal
        assert_eq!(r.read(3), 0b010); // PilotStrengthCorrectionMin
        assert_eq!(r.read(3), 0b100); // PilotStrengthCorrectionMax
        assert_eq!(r.read(2), 0); // Reserved padding
    }

    #[test]
    fn access_parameters_enhanced_round_trip() {
        for code in 0..=2u8 {
            let mut ap = AccessParameters::defaults();
            ap.enhanced = Some(EnhancedAccessParameters {
                preamble_length_slots: code & 1,
                access_offset: code,
                sector_access_max_rate: code,
                probe_time_out_adjust: code + 1,
                pilot_strength_nominal: 7 - code,
                pilot_strength_correction_min: code,
                pilot_strength_correction_max: code + 3,
            });
            let decoded =
                AccessParameters::decode(&ap.encode()).expect("enhanced AccessParameters decodes");
            assert_eq!(decoded, ap);
        }

        // All-zero enhanced codes still round-trip as Some (the flag bit is
        // the only nonzero content past the Rev 0 body).
        let mut ap = AccessParameters::defaults();
        ap.enhanced = Some(EnhancedAccessParameters {
            preamble_length_slots: 0,
            access_offset: 0,
            sector_access_max_rate: 0,
            probe_time_out_adjust: 0,
            pilot_strength_nominal: 0,
            pilot_strength_correction_min: 0,
            pilot_strength_correction_max: 0,
        });
        let decoded = AccessParameters::decode(&ap.encode()).expect("all-zero enhanced decodes");
        assert_eq!(decoded, ap);
    }

    #[test]
    fn access_parameters_rev0_wire_form_unchanged() {
        let ap = AccessParameters::defaults();
        assert!(ap.enhanced.is_none());
        let bytes = ap.encode();
        assert_eq!(bytes.len(), 11);
        let decoded = AccessParameters::decode(&bytes).expect("Rev 0 AccessParameters decodes");
        assert_eq!(decoded.enhanced, None);
        assert_eq!(decoded, ap);
    }

    #[test]
    fn access_parameters_enhanced_value_mappings() {
        let enhanced = EnhancedAccessParameters {
            preamble_length_slots: 0,
            access_offset: 0b11,
            sector_access_max_rate: SECTOR_ACCESS_MAX_RATE_19K2,
            probe_time_out_adjust: 0b111,
            pilot_strength_nominal: 0b100,
            pilot_strength_correction_min: 0b101,
            pilot_strength_correction_max: 0b110,
        };
        assert_eq!(enhanced.preamble_slots(), 4);
        assert_eq!(enhanced.access_offset_slots(), 12);
        assert_eq!(enhanced.sector_access_max_rate_bps(), Some(19_200));
        assert_eq!(enhanced.probe_time_out_adjust_slots(), 112);
        assert_eq!(enhanced.pilot_strength_nominal_db(), -4);
        assert_eq!(enhanced.pilot_strength_correction_min_db(), Some(-5));
        assert_eq!(enhanced.pilot_strength_correction_max_db(), None);

        let mut ap = AccessParameters::defaults();
        ap.preamble_length = 2;
        assert_eq!(ap.preamble_length_slots(), 32);
        assert_eq!(ap.access_offset_slots(), 0);
        assert_eq!(ap.sector_access_max_rate_bps(), 9_600);
        ap.enhanced = Some(enhanced);
        assert_eq!(ap.preamble_length_slots(), 4);
        assert_eq!(ap.access_offset_slots(), 12);
        assert_eq!(ap.sector_access_max_rate_bps(), 19_200);
    }

    #[test]
    fn sync_field_offsets() {
        let m = SyncMessage {
            maximum_revision: 0x12,
            minimum_revision: 0x07,
            pilot_pn: 0x1A5,
            system_time: 0x1_2345_6789, // < 2^37
        };
        let bytes = m.encode();
        let mut r = BitReader::new(&bytes);
        assert_eq!(r.read(2), 0); // MessageID
        assert_eq!(r.read(8), 0x12);
        assert_eq!(r.read(8), 0x07);
        assert_eq!(r.read(9), 0x1A5);
        assert_eq!(r.read(37), 0x1_2345_6789);
    }

    #[test]
    fn overhead_dispatch_round_trip_lengths() {
        let msgs = [
            HrpdOverheadMessage::QuickConfig(QuickConfig::defaults()),
            HrpdOverheadMessage::SectorParameters(SectorParameters::defaults()),
            HrpdOverheadMessage::AccessParameters(AccessParameters::defaults()),
            HrpdOverheadMessage::BroadcastReverseRateLimit(BroadcastReverseRateLimit {
                rpc_count: 1,
                rate_limit: vec![5],
            }),
            HrpdOverheadMessage::Sync(SyncMessage::defaults()),
        ];
        let expected = [10usize, 36, 11, 3, 8];
        for (m, want) in msgs.iter().zip(expected.iter()) {
            assert_eq!(m.encode().len(), *want, "msg {:?}", m);
        }
    }

    #[test]
    fn overhead_messages_decode_round_trip() {
        let mut qc = QuickConfig::defaults();
        qc.color_code = 0x44;
        qc.rpc_count = 3;
        qc.forward_traffic_valid = vec![true, false, true];
        let decoded_qc = QuickConfig::decode(&qc.encode()).expect("QuickConfig decodes");
        assert_eq!(decoded_qc.color_code, qc.color_code);
        assert_eq!(decoded_qc.rpc_count, qc.rpc_count);
        assert_eq!(decoded_qc.forward_traffic_valid, qc.forward_traffic_valid);

        let mut sp = SectorParameters::defaults().with_one_x_neighbor(1, 384, 17);
        sp.latitude = -1234;
        sp.longitude = 5678;
        sp.local_time_offset = -300;
        let decoded_sp = SectorParameters::decode(&sp.encode()).expect("SectorParameters decodes");
        assert_eq!(decoded_sp.country_code, sp.country_code);
        assert_eq!(decoded_sp.latitude, sp.latitude);
        assert_eq!(decoded_sp.longitude, sp.longitude);
        assert_eq!(decoded_sp.local_time_offset, sp.local_time_offset);
        assert_eq!(decoded_sp.neighbors.len(), 1);
        assert_eq!(decoded_sp.neighbors[0].pilot_pn, 17);
        assert_eq!(
            decoded_sp.neighbors[0]
                .channel
                .expect("neighbor channel")
                .channel_number,
            384
        );

        let mut ap = AccessParameters::defaults();
        ap.probe_initial_adjust = -3;
        ap.a_persistence = [0, 1, 2, 3];
        let decoded_ap = AccessParameters::decode(&ap.encode()).expect("AccessParameters decodes");
        assert_eq!(decoded_ap.probe_initial_adjust, -3);
        assert_eq!(decoded_ap.a_persistence, [0, 1, 2, 3]);

        let rate_limit = BroadcastReverseRateLimit {
            rpc_count: 2,
            rate_limit: vec![4, 5],
        };
        let decoded_rate_limit =
            BroadcastReverseRateLimit::decode(&rate_limit.encode()).expect("rate limit decodes");
        assert_eq!(decoded_rate_limit, rate_limit);

        let sync = SyncMessage {
            maximum_revision: 1,
            minimum_revision: 1,
            pilot_pn: 4,
            system_time: 54_867_849_078,
        };
        let decoded_sync = SyncMessage::decode(&sync.encode()).expect("Sync decodes");
        assert_eq!(decoded_sync.maximum_revision, 1);
        assert_eq!(decoded_sync.minimum_revision, 1);
        assert_eq!(decoded_sync.pilot_pn, 4);
        assert_eq!(decoded_sync.system_time, 54_867_849_078);
    }

    #[test]
    fn overhead_dispatch_decodes_by_protocol_type() {
        let qc = QuickConfig::defaults();
        assert!(matches!(
            HrpdOverheadMessage::decode_for_protocol(OVERHEAD_MESSAGES_PROTOCOL_TYPE, &qc.encode()),
            Some(HrpdOverheadMessage::QuickConfig(_))
        ));

        let access = AccessParameters::defaults();
        assert!(matches!(
            HrpdOverheadMessage::decode_for_protocol(
                DEFAULT_ACCESS_CHANNEL_MAC_PROTOCOL_TYPE,
                &access.encode()
            ),
            Some(HrpdOverheadMessage::AccessParameters(_))
        ));

        let rate_limit = BroadcastReverseRateLimit {
            rpc_count: 1,
            rate_limit: vec![5],
        };
        assert!(matches!(
            HrpdOverheadMessage::decode_for_protocol(
                DEFAULT_REVERSE_TRAFFIC_CHANNEL_MAC_PROTOCOL_TYPE,
                &rate_limit.encode()
            ),
            Some(HrpdOverheadMessage::BroadcastReverseRateLimit(_))
        ));

        let sync = SyncMessage::defaults();
        assert!(matches!(
            HrpdOverheadMessage::decode_for_protocol(
                DEFAULT_INITIALIZATION_STATE_PROTOCOL_TYPE,
                &sync.encode()
            ),
            Some(HrpdOverheadMessage::Sync(_))
        ));
    }

    #[test]
    fn live_884490_overhead_bodies_decode() {
        let quick =
            QuickConfig::decode(&hex_bytes("00010000010001000103835000")).expect("QuickConfig");
        assert_eq!(quick.color_code, 1);
        assert_eq!(quick.sector_id24, 1);
        assert_eq!(quick.sector_signature, 1);
        assert_eq!(quick.access_signature, 1);
        assert!(!quick.redirect);
        assert_eq!(quick.rpc_count, 1);
        assert_eq!(quick.forward_traffic_valid, vec![true]);
        assert_eq!(quick.rpc_count_127_to_64, Some(1));
        assert_eq!(quick.forward_traffic_valid_127_to_64, vec![true]);
        assert_eq!(quick.rpc_count_130_to_383, None);
        assert_eq!(
            quick.trailing_extension_bits,
            vec![1, 0, 1, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0]
        );

        let sector = SectorParameters::decode(&hex_bytes(
            "011360000000000000000000000000000000168000111DA0B31FD50000D0CA6000",
        ))
        .expect("SectorParameters");
        // Capture predates the BCD CountryCode fix: its wire value is raw
        // binary 310 (0x136), which reads back as BCD digits 1-3-6.
        assert_eq!(sector.country_code, 136);
        assert_eq!(
            sector.sector_id,
            [0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 0, 1]
        );
        assert_eq!(sector.subnet_mask, 104);
        assert_eq!(sector.latitude, 292_482);
        assert_eq!(sector.longitude, -1_687_638);
        assert_eq!(sector.leap_seconds, 13);
        assert_eq!(sector.local_time_offset, 101);

        let access =
            AccessParameters::decode(&hex_bytes("014000015607B220000000")).expect("Access");
        assert_eq!(access.access_cycle_duration, 64);
        assert_eq!(access.access_signature, 1);
        assert_eq!(access.open_loop_adjust, 86);
        assert_eq!(access.probe_num_step, 15);
        assert_eq!(access.power_step, 6);
        assert_eq!(access.preamble_length, 2);
        assert_eq!(access.capsule_length_max, 2);

        let reverse_rate =
            BroadcastReverseRateLimit::decode(&hex_bytes("010540")).expect("rate limit");
        assert_eq!(reverse_rate.rpc_count, 1);
        assert_eq!(reverse_rate.rate_limit, vec![5]);

        let sync = SyncMessage::decode(&hex_bytes("0040408CC660EDF6")).expect("Sync");
        assert_eq!(
            sync,
            SyncMessage {
                maximum_revision: 1,
                minimum_revision: 1,
                pilot_pn: 4,
                system_time: 54_867_848_694,
            }
        );
    }

    #[test]
    fn sector_parameters_with_one_x_neighbor_carries_partner_record() {
        let sp = SectorParameters::defaults().with_one_x_neighbor(0, 25, 42);
        assert_eq!(sp.neighbors.len(), 1);
        let n = &sp.neighbors[0];
        assert_eq!(n.pilot_pn, 42);
        let ch = n.channel.expect("channel record present");
        assert_eq!(ch.system_type, 0x01, "1x system_type");
        assert_eq!(ch.band_class, 0);
        assert_eq!(ch.channel_number, 25);
        // Ensure the encoded form still rounds-trips through encode().
        let bytes = sp.encode();
        assert!(!bytes.is_empty());
    }

    #[test]
    fn sector_parameters_with_neighbor() {
        let mut sp = SectorParameters::defaults();
        sp.neighbors.push(NeighborEntry {
            pilot_pn: 0x123,
            channel: Some(ChannelRecord {
                system_type: 0x00,
                band_class: 1,
                channel_number: 50,
            }),
            search_window_size: Some(7),
            search_window_offset: Some(3),
        });
        let bytes = sp.encode();
        // 287 (baseline) + 9 (PN) + 1 (NCI) + 24 (channel) + 4 (search window
        // size) + 3 (search window offset) = 328 bits = 41 octets.
        assert_eq!(bytes.len(), 41);
    }

    #[test]
    fn sector_parameters_neighbor_channel_records_interleave_per_neighbor() {
        // Two neighbors where the FIRST carries a channel record. Per
        // C.S0024-A §8.9.6.2.2 the layout is {flag, record} per neighbor,
        // so neighbor 0's record sits between the two flag bits — a layout
        // a flags-then-records encoding cannot produce.
        let mut sp = SectorParameters::defaults();
        sp.neighbors.push(NeighborEntry {
            pilot_pn: 0x0AA,
            channel: Some(ChannelRecord {
                system_type: 0x01,
                band_class: 1,
                channel_number: 384,
            }),
            search_window_size: None,
            search_window_offset: None,
        });
        sp.neighbors.push(NeighborEntry {
            pilot_pn: 0x155,
            channel: None,
            search_window_size: None,
            search_window_offset: None,
        });
        let bytes = sp.encode();
        // 251 (fixed header) + 5 + 24 (one channel) + 5 (NeighborCount)
        // + 18 (two PNs) + 1 + 24 (neighbor 0 flag+record)
        // + 1 (neighbor 1 flag) + 1 + 1 (window flags) = 331 -> 42 octets.
        assert_eq!(bytes.len(), 42);

        let mut r = BitReader::new(&bytes);
        r.read(8); // MessageID
        r.read(12); // CountryCode
        for _ in 0..16 {
            r.read(8); // SectorID
        }
        r.read(8); // SubnetMask
        r.read(16); // SectorSignature
        r.read(22); // Latitude
        r.read(23); // Longitude
        r.read(11); // RouteUpdateRadius
        r.read(8); // LeapSeconds
        r.read(11); // LocalTimeOffset
        r.read(2); // ReverseLinkSilenceDuration
        r.read(2); // ReverseLinkSilencePeriod
        assert_eq!(r.read(5), 1); // ChannelCount
        r.read(24); // Channel
        assert_eq!(r.read(5), 2); // NeighborCount
        assert_eq!(r.read(9), 0x0AA); // NeighborPilotPN[0], bit offset 285
        assert_eq!(r.read(9), 0x155); // NeighborPilotPN[1], bit offset 294
        // Bit 303: NeighborChannelIncluded[0] = 1.
        assert_eq!(r.read(1), 1);
        // Bits 304..328: neighbor 0 record {0x01, 1, 384} = 0x10980.
        assert_eq!(r.read(24), 0x01_0980);
        // Bit 328: NeighborChannelIncluded[1] = 0.
        assert_eq!(r.read(1), 0);
        assert_eq!(r.read(1), 0); // NeighborSearchWindowSizeIncluded
        assert_eq!(r.read(1), 0); // NeighborSearchWindowOffsetIncluded

        let decoded = SectorParameters::decode(&bytes).expect("interleaved neighbors decode");
        assert_eq!(decoded, sp);
    }
}
