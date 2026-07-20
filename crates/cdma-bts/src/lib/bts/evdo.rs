use cdma_common::band_class::ChannelPlan;
use cdma_common::error::Error;
use cdma_common::hrpd::air::{
    AccessTerminalIdentifierType, DEFAULT_ADDRESS_MANAGEMENT_PROTOCOL_TYPE,
    DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE, HrpdForwardChannel, HrpdForwardSignalingRequest,
    HrpdTrafficChannelAssignment, HrpdUatiAssignment,
};
use log::{info, trace, warn};
use num::complex::Complex32;
use serde::{Deserialize, Serialize, de};
use std::{collections::VecDeque, fmt};

use crate::{
    bts::hrpd::{
        control_channel::{
            CTRL_CH_CYCLE_SLOTS, ControlChannelCapsule, ControlChannelDefaultSignalingMessage,
            ctrl_ch_kbps,
        },
        control_modulator::ControlChannelModulator,
        mac_encoder::HrpdForwardMacEncoder,
        overhead::OverheadSchedule,
        scheduler::{DATA_CHIPS_PER_SLOT, HrpdForwardScheduler, SlotKind},
    },
    phy::{
        hrpd::slot::{SLOT_CHIPS, SlotChannel, channel_for_chip},
        spread::{HrpdForwardPnSequence, Spreader},
    },
    sdr::{PhasorNco, TxPulseShaper},
};
use cdma_common::hrpd::messages::{
    AccessParameters, BroadcastReverseRateLimit, DEFAULT_ACCESS_CHANNEL_MAC_PROTOCOL_TYPE,
    DEFAULT_INITIALIZATION_STATE_PROTOCOL_TYPE, DEFAULT_REVERSE_TRAFFIC_CHANNEL_MAC_PROTOCOL_TYPE,
    HrpdOverheadMessage, OVERHEAD_MESSAGES_PROTOCOL_TYPE, QuickConfig, SectorParameters,
    SyncMessage,
};

/// Conservative occupied half-bandwidth for one SR1 CDMA/HRPD carrier. Used
/// only to validate whether two synthesized carriers fit in one RF bandwidth.
pub const SR1_OCCUPIED_HALF_BW_HZ: i64 = 740_000;
/// Guardrail for single-RF composite 1x + EV-DO operation. This is an SDR
/// tuning minimum, not an HRPD air-interface constant.
const MIN_SINGLE_RF_COMPOSITE_TX_BANDWIDTH_HZ: usize = 5_000_000;
/// Maximum span, in slots, for an in-flight Control packet at the selected
/// Forward Control Channel rate.
fn hrpd_control_packet_slot_span() -> u64 {
    if ctrl_ch_kbps() == 38_400 { 64 } else { 32 }
}
/// Avoid letting directed signaling consume the asynchronous Control capsule;
/// overhead remains the priority in that path.
const MAX_DEFAULT_SIGNALING_PER_CONTROL_CAPSULE: usize = 1;
/// Directed-message slots allowed in one synchronous capsule before capacity
/// accounting rejects additional signaling.
const MAX_DEFAULT_SIGNALING_PER_SYNC_CAPSULE: usize = 4;
/// Sync-copy queue bound for directed Control Channel signaling.
const MAX_PENDING_SYNC_SIGNALING: usize = 32;
const TRAFFIC_ASSIGNMENT_REPEAT_OFFSETS_SLOTS: [u64; 2] = [48, 96];
/// Control Channel MAC capsule header: SynchronousCapsule + FirstPacket +
/// LastPacket + Offset(2) + SleepStateCapsuleDone + Reserved(2).
const CONTROL_CAPSULE_HEADER_BITS: usize = 8;

/// Capsule bits one overhead body consumes: length octet + MAC packet header
/// byte (no ATI for broadcast) + 2-byte SNP/SLP header + body. Mirrors the
/// encoding in `control_modulator::control_mac_packet_parts`.
fn capsule_overhead_body_bits(body: &[u8]) -> usize {
    8 + 8 + (2 + body.len()) * 8
}

/// Capsule bits one directed Default Signaling message consumes. Mirrors the
/// encoding in `control_modulator::control_mac_packet_parts`: length octet +
/// MAC packet header byte + 32-bit ATI (unicast) + SNP/SLP header (3 bytes
/// reliable, 2 best-effort) + payload.
fn capsule_directed_message_bits(message: &ControlChannelDefaultSignalingMessage) -> usize {
    let ati_bits = if message.ati.ati_type == AccessTerminalIdentifierType::Bati {
        0
    } else {
        32
    };
    let snp_octets = if message.reliable_sequence.is_some() {
        3
    } else {
        2
    };
    8 + 8 + ati_bits + (snp_octets + message.payload.len()) * 8
}

fn active_mac_indices_from_encoder(mac_encoder: &HrpdForwardMacEncoder) -> Vec<u8> {
    mac_encoder
        .actives()
        .iter()
        .filter_map(|active| {
            (5..64)
                .contains(&active.mac_index)
                .then_some(active.mac_index)
        })
        .collect()
}

fn format_mac_list(macs: &[u8]) -> String {
    if macs.is_empty() {
        "none".to_string()
    } else {
        macs.iter()
            .map(|mac| format!("m{mac}"))
            .collect::<Vec<_>>()
            .join(",")
    }
}

fn mac_occurrence_and_index(mac_index: u8) -> Option<(usize, usize)> {
    if (5..64).contains(&mac_index) {
        let idx = usize::from(63u8.saturating_sub(mac_index));
        let occurrence = usize::from(64u8.saturating_sub(mac_index));
        Some((occurrence, idx))
    } else {
        None
    }
}

fn format_quick_config_active_ftv(qc: &QuickConfig, active_macs: &[u8]) -> String {
    if active_macs.is_empty() {
        return "none".to_string();
    }
    active_macs
        .iter()
        .filter_map(|mac| {
            let (occurrence, idx) = mac_occurrence_and_index(*mac)?;
            let covered = idx < usize::from(qc.rpc_count);
            let valid = qc.forward_traffic_valid.get(idx).copied().unwrap_or(false);
            Some(format!(
                "m{mac}:occ{occurrence}:idx{idx}:covered={covered}:valid={valid}"
            ))
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn format_reverse_rate_active_limits(rr: &BroadcastReverseRateLimit, active_macs: &[u8]) -> String {
    if active_macs.is_empty() {
        return "none".to_string();
    }
    active_macs
        .iter()
        .filter_map(|mac| {
            let (occurrence, idx) = mac_occurrence_and_index(*mac)?;
            let covered = idx < usize::from(rr.rpc_count);
            let limit = rr
                .rate_limit
                .get(idx)
                .map(|value| value.to_string())
                .unwrap_or_else(|| "missing".to_string());
            Some(format!(
                "m{mac}:occ{occurrence}:idx{idx}:covered={covered}:limit={limit}"
            ))
        })
        .collect::<Vec<_>>()
        .join(",")
}

fn decode_overhead_body(body: &[u8]) -> Option<HrpdOverheadMessage> {
    [
        OVERHEAD_MESSAGES_PROTOCOL_TYPE,
        DEFAULT_ACCESS_CHANNEL_MAC_PROTOCOL_TYPE,
        DEFAULT_REVERSE_TRAFFIC_CHANNEL_MAC_PROTOCOL_TYPE,
        DEFAULT_INITIALIZATION_STATE_PROTOCOL_TYPE,
    ]
    .iter()
    .find_map(|protocol| HrpdOverheadMessage::decode_for_protocol(*protocol, body))
}

fn format_overhead_body(body: &[u8], active_macs: &[u8]) -> String {
    match decode_overhead_body(body) {
        Some(HrpdOverheadMessage::QuickConfig(qc)) => format!(
            "overhead:QuickConfig/{}B/rpc_count={}/ftv=[{}]",
            body.len(),
            qc.rpc_count,
            format_quick_config_active_ftv(&qc, active_macs)
        ),
        Some(HrpdOverheadMessage::BroadcastReverseRateLimit(rr)) => format!(
            "overhead:ReverseRate/{}B/rpc_count={}/limits=[{}]",
            body.len(),
            rr.rpc_count,
            format_reverse_rate_active_limits(&rr, active_macs)
        ),
        Some(HrpdOverheadMessage::SectorParameters(_)) => {
            format!("overhead:SectorParameters/{}B", body.len())
        }
        Some(HrpdOverheadMessage::AccessParameters(_)) => {
            format!("overhead:AccessParameters/{}B", body.len())
        }
        Some(HrpdOverheadMessage::Sync(_)) => format!("overhead:Sync/{}B", body.len()),
        None => format!("overhead:Unknown/{}B", body.len()),
    }
}

/// Move queued directed messages into `directed` while they fit the capsule
/// bit budget. Stops at the first message identical to one already included:
/// repeat copies of the same message stay queued for later cycles instead of
/// burning multiple slots of one capsule on duplicates.
fn drain_signaling_into_sync_capsule(
    queue: &mut VecDeque<ControlChannelDefaultSignalingMessage>,
    directed: &mut Vec<ControlChannelDefaultSignalingMessage>,
    capsule_bits: &mut usize,
    cycle_index: u64,
) {
    use crate::bts::hrpd::control_modulator::DEFAULT_CONTROL_MAC_BITS;
    while directed.len() < MAX_DEFAULT_SIGNALING_PER_SYNC_CAPSULE {
        let Some(front) = queue.front() else {
            break;
        };
        if !synchronous_control_cycle_matches(front, cycle_index) {
            break;
        }
        if directed.iter().any(|included| included == front) {
            break;
        }
        let bits = capsule_directed_message_bits(front);
        if *capsule_bits + bits > DEFAULT_CONTROL_MAC_BITS {
            break;
        }
        *capsule_bits += bits;
        directed.push(queue.pop_front().expect("front checked above"));
    }
}

fn has_due_scheduled_sync_signaling(
    queue: &VecDeque<ControlChannelDefaultSignalingMessage>,
    cycle_index: u64,
) -> bool {
    queue.iter().any(|message| {
        message.synchronous_control_cycle.is_some()
            && synchronous_control_cycle_matches(message, cycle_index)
    })
}

fn drain_due_scheduled_sync_signaling(
    queue: &mut VecDeque<ControlChannelDefaultSignalingMessage>,
    directed: &mut Vec<ControlChannelDefaultSignalingMessage>,
    capsule_bits: &mut usize,
    cycle_index: u64,
) {
    use crate::bts::hrpd::control_modulator::DEFAULT_CONTROL_MAC_BITS;
    while directed.len() < MAX_DEFAULT_SIGNALING_PER_SYNC_CAPSULE {
        let Some(pos) = queue.iter().position(|message| {
            message.synchronous_control_cycle.is_some()
                && synchronous_control_cycle_matches(message, cycle_index)
        }) else {
            break;
        };
        let Some(front) = queue.get(pos) else {
            break;
        };
        if directed.iter().any(|included| included == front) {
            break;
        }
        let bits = capsule_directed_message_bits(front);
        if *capsule_bits + bits > DEFAULT_CONTROL_MAC_BITS {
            break;
        }
        *capsule_bits += bits;
        directed.push(queue.remove(pos).expect("position checked above"));
    }
}

fn synchronous_control_cycle_matches(
    message: &ControlChannelDefaultSignalingMessage,
    cycle_index: u64,
) -> bool {
    let Some(schedule) = message.synchronous_control_cycle else {
        return true;
    };
    schedule.modulus != 0
        && cycle_index % u64::from(schedule.modulus) == u64::from(schedule.residue)
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EvdoTxMode {
    AdjacentComposite,
    HrpdOnly,
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvdoMode {
    #[default]
    Composite,
    HrpdOnly,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct EvdoConfig {
    pub enabled: bool,
    /// Explicit HRPD channel number. Required when EVDO is enabled.
    pub channel: Option<u16>,
    pub mode: EvdoMode,
    pub advertise_on_1x: bool,
    pub gain: f32,
    pub overhead: HrpdOverheadConfig,
}

impl Default for EvdoConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            channel: None,
            mode: EvdoMode::Composite,
            advertise_on_1x: true,
            // 1.0 = HRPD at parity with 1x at the composer output (post
            // composite_scale they each get half of the summed budget).
            gain: 1.0,
            overhead: HrpdOverheadConfig::default(),
        }
    }
}

impl EvdoConfig {
    pub fn tx_mode(&self) -> EvdoTxMode {
        match self.mode {
            EvdoMode::Composite => EvdoTxMode::AdjacentComposite,
            EvdoMode::HrpdOnly => EvdoTxMode::HrpdOnly,
        }
    }
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct HrpdOverheadConfig {
    /// Explicit 128-bit SectorID advertised in SectorParameters. QuickConfig
    /// carries the least-significant 24 bits of this same value.
    pub sector_id: Option<HrpdSectorId>,
    /// Explicit HRPD subnet mask length for SectorParameters.
    pub subnet_mask: Option<u8>,
    /// Explicit HRPD color code advertised in QuickConfig and used for access.
    pub color_code: Option<u8>,
    /// Signature advertised in QuickConfig and SectorParameters. Change this
    /// when SectorParameters contents change so ATs refresh cached overhead.
    pub sector_signature: u16,
    /// Signature advertised in QuickConfig and AccessParameters. Change this
    /// when AccessParameters contents change so ATs refresh access config.
    pub access_signature: u16,
}

impl Default for HrpdOverheadConfig {
    fn default() -> Self {
        Self {
            sector_id: None,
            subnet_mask: None,
            color_code: None,
            sector_signature: 0x0001,
            access_signature: 0x0001,
        }
    }
}

impl HrpdOverheadConfig {
    pub fn resolve(self) -> Result<ResolvedHrpdOverheadConfig, Error> {
        let sector_id = self
            .sector_id
            .ok_or_else(|| Error::from("evdo.overhead.sector_id is required when EVDO is enabled"))?
            .bytes();
        let subnet_mask = self.subnet_mask.ok_or_else(|| {
            Error::from("evdo.overhead.subnet_mask is required when EVDO is enabled")
        })?;
        if subnet_mask > 128 {
            return Err(format!(
                "evdo.overhead.subnet_mask must be in 0..=128 (current: {subnet_mask})"
            )
            .into());
        }
        let color_code = self.color_code.ok_or_else(|| {
            Error::from("evdo.overhead.color_code is required when EVDO is enabled")
        })?;
        Ok(ResolvedHrpdOverheadConfig {
            sector_id,
            subnet_mask,
            color_code,
            sector_signature: self.sector_signature,
            access_signature: self.access_signature,
        })
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ResolvedHrpdOverheadConfig {
    pub sector_id: [u8; 16],
    pub subnet_mask: u8,
    pub color_code: u8,
    pub sector_signature: u16,
    pub access_signature: u16,
}

impl ResolvedHrpdOverheadConfig {
    pub fn sector_id24(&self) -> u32 {
        sector_id24_from_sector_id(&self.sector_id)
    }

    /// AccessParameters `PreambleLength` (in frames) the reverse-access RX
    /// finger despreads the capsule at. The resolved overhead does not carry an
    /// explicit AccessParameters block, so this returns the spec default.
    pub fn access_preamble_frames(&self) -> usize {
        crate::receiver::hrpd::access::HRPD_DEFAULT_ACCESS_PREAMBLE_FRAMES
    }

    /// Whether the reverse-access RX should hypothesize the enhanced
    /// 19.2/38.4 kbps capsule rates. Must mirror the broadcast
    /// AccessParameters; the sector currently broadcasts the Rev 0 defaults
    /// (no `SectorAccessMaxRate`), so ATs only transmit 9.6 kbps capsules.
    pub fn enhanced_access_rates(&self) -> bool {
        false
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct HrpdSectorId([u8; 16]);

impl HrpdSectorId {
    pub const fn new(bytes: [u8; 16]) -> Self {
        Self(bytes)
    }

    pub const fn bytes(self) -> [u8; 16] {
        self.0
    }

    pub fn to_hex(self) -> String {
        format_sector_id_hex(&self.0)
    }
}

impl Serialize for HrpdSectorId {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: serde::Serializer,
    {
        serializer.serialize_str(&self.to_hex())
    }
}

impl<'de> Deserialize<'de> for HrpdSectorId {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: serde::Deserializer<'de>,
    {
        struct SectorIdVisitor;

        impl<'a> de::Visitor<'a> for SectorIdVisitor {
            type Value = HrpdSectorId;

            fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                formatter.write_str("a 128-bit HRPD SectorID as 32 hex digits")
            }

            fn visit_str<E>(self, value: &str) -> Result<Self::Value, E>
            where
                E: de::Error,
            {
                parse_sector_id_hex(value)
                    .map(HrpdSectorId)
                    .map_err(E::custom)
            }
        }

        deserializer.deserialize_str(SectorIdVisitor)
    }
}

pub fn sector_id24_from_sector_id(sector_id: &[u8; 16]) -> u32 {
    (u32::from(sector_id[13]) << 16) | (u32::from(sector_id[14]) << 8) | u32::from(sector_id[15])
}

fn format_sector_id_hex(sector_id: &[u8; 16]) -> String {
    let mut out = String::with_capacity(32);
    for byte in sector_id {
        out.push_str(&format!("{byte:02X}"));
    }
    out
}

fn parse_sector_id_hex(input: &str) -> Result<[u8; 16], String> {
    let trimmed = input.trim();
    let body = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
        .unwrap_or(trimmed);
    let mut hex = String::with_capacity(32);
    for ch in body.chars() {
        match ch {
            ':' | '-' | '_' if !hex.is_empty() => {}
            ch if ch.is_ascii_whitespace() => {}
            ch if ch.is_ascii_hexdigit() => hex.push(ch),
            ch => return Err(format!("invalid SectorID character '{ch}'")),
        }
    }
    if hex.len() != 32 {
        return Err(format!(
            "expected 32 hex digits for 128-bit SectorID, got {}",
            hex.len()
        ));
    }
    let mut out = [0u8; 16];
    for idx in 0..16 {
        out[idx] = u8::from_str_radix(&hex[idx * 2..idx * 2 + 2], 16)
            .map_err(|e| format!("invalid SectorID byte {idx}: {e}"))?;
    }
    Ok(out)
}

#[derive(Clone, Debug)]
pub struct ResolvedEvdoConfig {
    pub tx_mode: EvdoTxMode,
    pub one_x_band_class: u8,
    pub one_x_channel: u16,
    pub one_x_frequency_hz: usize,
    pub evdo_band_class: u8,
    pub evdo_channel: u16,
    pub evdo_frequency_hz: usize,
    pub evdo_reverse_frequency_hz: usize,
    pub composite_center_frequency_hz: usize,
    pub one_x_shift_hz: i64,
    pub evdo_shift_hz: i64,
    pub pilot_pn: u16,
    pub advertise_on_1x: bool,
    pub gain: f32,
    pub overhead: ResolvedHrpdOverheadConfig,
}

#[derive(Clone, Copy, Debug)]
pub struct Evdo1xAdvertisement {
    pub hrpd_pn: u16,
    pub hrpd_band_class: u8,
    pub hrpd_channel: u16,
    pub hrpd_color_code: u8,
}

impl ResolvedEvdoConfig {
    pub fn uses_adjacent_composite(&self) -> bool {
        self.tx_mode == EvdoTxMode::AdjacentComposite
    }

    pub fn uses_hrpd_only(&self) -> bool {
        self.tx_mode == EvdoTxMode::HrpdOnly
    }

    pub fn transmits_one_x(&self) -> bool {
        !self.uses_hrpd_only()
    }

    pub fn advertisement(&self) -> Option<Evdo1xAdvertisement> {
        self.advertise_on_1x.then_some(Evdo1xAdvertisement {
            hrpd_pn: self.pilot_pn,
            hrpd_band_class: self.evdo_band_class,
            hrpd_channel: self.evdo_channel,
            hrpd_color_code: self.overhead.color_code,
        })
    }
}

pub fn resolve_evdo_config(
    evdo: &EvdoConfig,
    pilot_offset: usize,
    one_x_channel_plan: ChannelPlan,
    tx_sample_rate_hz: usize,
    tx_bandwidth_hz: usize,
) -> Result<Option<ResolvedEvdoConfig>, Error> {
    if !evdo.enabled {
        return Ok(None);
    }
    if pilot_offset > 511 {
        return Err("evdo: top-level pilot_offset must be in 0..=511".into());
    }
    let overhead = evdo.overhead.resolve()?;
    // Sample rate must be an integer multiple of the chip rate ≥ 4× so the
    // pulse-shape polyphase decomposition is exact. Higher multiples (e.g.
    // 8×) widen the Nyquist window and allow farther-spaced HRPD channels
    // (the actual fit is checked per-shift below).
    let chip_rate = cdma_common::consts::SR1_CHIP_RATE_HZ as usize;
    if tx_sample_rate_hz < chip_rate * 4 || tx_sample_rate_hz % chip_rate != 0 {
        return Err(format!(
            "evdo: tx_sample_rate_hz={tx_sample_rate_hz} must be an integer multiple of \
             the chip rate ({chip_rate}) and at least 4× ({})",
            chip_rate * 4
        )
        .into());
    }
    if evdo.gain < 0.0 {
        return Err("evdo.gain must be non-negative".into());
    }

    let tx_mode = evdo.tx_mode();

    // HRPD shares the 1x band class + subclass; only the channel differs.
    let evdo_plan = match evdo.channel {
        Some(channel) => ChannelPlan::new(
            one_x_channel_plan.band_class,
            one_x_channel_plan.band_subclass,
            channel,
        ),
        None => {
            return Err("evdo.channel is required when EVDO is enabled".into());
        }
    };
    evdo_plan.validate().map_err(|e| {
        Error::from(format!(
            "evdo: derived HRPD channel {} on {} is invalid: {e}",
            evdo_plan.cdma_channel,
            evdo_plan.band_class.as_str()
        ))
    })?;

    let one_x_frequency_hz = one_x_channel_plan.downlink_hz() as usize;
    let evdo_frequency_hz = evdo_plan.downlink_hz() as usize;
    let evdo_reverse_frequency_hz = evdo_plan.uplink_hz() as usize;
    let (composite_center_frequency_hz, one_x_shift_hz, evdo_shift_hz) = match tx_mode {
        EvdoTxMode::AdjacentComposite => {
            let carrier_separation_hz = one_x_frequency_hz.abs_diff(evdo_frequency_hz);
            let required_tx_bandwidth_hz = (carrier_separation_hz
                + (SR1_OCCUPIED_HALF_BW_HZ as usize * 2))
                .max(MIN_SINGLE_RF_COMPOSITE_TX_BANDWIDTH_HZ);
            if tx_bandwidth_hz < required_tx_bandwidth_hz {
                return Err(format!(
                    "evdo requires tx_bandwidth_hz >= {} for single-RF composite mode \
                     (carrier separation={} Hz, occupied half-BW={} Hz, current={tx_bandwidth_hz})",
                    required_tx_bandwidth_hz, carrier_separation_hz, SR1_OCCUPIED_HALF_BW_HZ
                )
                .into());
            }
            let composite_center_frequency_hz = (one_x_frequency_hz + evdo_frequency_hz) / 2;
            let one_x_shift_hz = one_x_frequency_hz as i64 - composite_center_frequency_hz as i64;
            let evdo_shift_hz = evdo_frequency_hz as i64 - composite_center_frequency_hz as i64;
            validate_shift_fit("1x", one_x_shift_hz, tx_sample_rate_hz)?;
            validate_shift_fit("evdo", evdo_shift_hz, tx_sample_rate_hz)?;
            (composite_center_frequency_hz, one_x_shift_hz, evdo_shift_hz)
        }
        EvdoTxMode::HrpdOnly => (evdo_frequency_hz, 0, 0),
    };

    Ok(Some(ResolvedEvdoConfig {
        tx_mode,
        one_x_band_class: one_x_channel_plan.band_class.field_value(),
        one_x_channel: one_x_channel_plan.cdma_channel,
        one_x_frequency_hz,
        evdo_band_class: evdo_plan.band_class.field_value(),
        evdo_channel: evdo_plan.cdma_channel,
        evdo_frequency_hz,
        evdo_reverse_frequency_hz,
        composite_center_frequency_hz,
        one_x_shift_hz,
        evdo_shift_hz,
        pilot_pn: pilot_offset as u16,
        advertise_on_1x: evdo.advertise_on_1x && tx_mode != EvdoTxMode::HrpdOnly,
        gain: evdo.gain,
        overhead,
    }))
}

fn validate_shift_fit(label: &str, shift_hz: i64, sample_rate_hz: usize) -> Result<(), Error> {
    let nyquist_hz = sample_rate_hz as i64 / 2;
    let occupied = shift_hz.abs() + SR1_OCCUPIED_HALF_BW_HZ;
    if occupied >= nyquist_hz {
        let needed_rate = (occupied + SR1_OCCUPIED_HALF_BW_HZ) as usize * 2;
        let chip_rate = cdma_common::consts::SR1_CHIP_RATE_HZ as usize;
        let needed_x = ((needed_rate + chip_rate - 1) / chip_rate)
            .next_power_of_two()
            .max(8);
        return Err(format!(
            "evdo {label} carrier shift ±{} kHz + occupied half-BW ±{} kHz = ±{} kHz \
             does not fit in complex Nyquist (±{} kHz at tx_sample_rate_hz={}). \
             Either move the EVDO channel closer to 1x, or raise tx_sample_rate_hz to \
             at least {} ({}× chip rate).",
            shift_hz.abs() / 1_000,
            SR1_OCCUPIED_HALF_BW_HZ / 1_000,
            occupied / 1_000,
            nyquist_hz / 1_000,
            sample_rate_hz,
            needed_x * chip_rate,
            needed_x,
        )
        .into());
    }
    Ok(())
}

/// HRPD forward-link slot modulator.
///
/// Routes per chip according to `phy::hrpd::slot::channel_for_chip`:
/// - Pilot chips emit `(1+0j)` (then PN-spread by the sector's short code).
/// - MAC chips come from the MAC encoder (RA broadcast + per-AT RPC/DRCLock).
/// - Data chips come from either the forward traffic scheduler (Traffic
///   slots) or the Control Channel modulator (Control slots, every 256-slot
///   cycle boundary), with auto-built overhead capsules from the configured
///   QuickConfig / SectorParameters / AccessParameters / Sync sources.
///
/// Constructed once per BTS launch when EVDO is enabled; the AT depends on
/// continuous transmission of pilot + MAC + Control Channel capsules to
/// finish acquisition (C.S0024-400 §6.3 Initialization State Protocol).
pub struct HrpdForwardSlotModulator {
    spreader: Spreader<HrpdForwardPnSequence>,
    pilot_offset: usize,
    short_code_length_chips: usize,
    next_chip: u64,
    aligned: bool,
    scheduler: HrpdForwardScheduler,
    overhead: OverheadSchedule,
    /// Currently-cached scheduler output and which slot it belongs to. The
    /// Data region of a slot is 1600 chips spanning four chip segments; we
    /// walk `slot_data_cursor` over those 1600 entries as Data chips arrive.
    current_slot: Option<u64>,
    slot_data_chips: Vec<Complex32>,
    slot_data_cursor: usize,
    /// MAC chips for the current slot (256 chips = 4 bursts × 64).
    slot_mac_chips: Vec<Complex32>,
    slot_mac_cursor: usize,
    mac_encoder: HrpdForwardMacEncoder,
    control: ControlChannelModulator,
    /// Owned overhead-message bodies the auto-capsule builder draws from.
    /// Each is the current value; mutate via the public setters when the
    /// sector parameters change. `None` for a slot means that slot is
    /// skipped even when its schedule fires.
    quick_config: Option<QuickConfig>,
    sector_params: Option<SectorParameters>,
    access_params: Option<AccessParameters>,
    reverse_rate: Option<BroadcastReverseRateLimit>,
    sync_msg: Option<SyncMessage>,
    /// Last cycle index for which we built a capsule, so we don't re-build
    /// twice when a Control slot resets cursors mid-cycle.
    last_cycle_loaded: Option<u64>,
    pending_signaling: VecDeque<ControlChannelDefaultSignalingMessage>,
    /// Best-effort repeat copies waiting for their earliest slot. Promoted
    /// into `pending_signaling` once the live slot index reaches the stored
    /// slot, spacing the copies across the AT's response window.
    deferred_signaling: Vec<(u64, ControlChannelDefaultSignalingMessage)>,
    /// Synchronous-capsule copies of directed signaling. A slotted AT only
    /// monitors the synchronous Sleep State capsule at the start of each
    /// Control Channel cycle (C.S0024-0 v4.0 §8.2.6) and never sees the
    /// mid-cycle asynchronous capsules, so every directed message also gets
    /// queued here and rides the next cycle-boundary capsule.
    pending_sync_signaling: VecDeque<ControlChannelDefaultSignalingMessage>,
    /// Active ReverseLinkMACIndex public data has changed and should be
    /// observed in the next synchronous Sleep State QuickConfig capsule.
    active_mac_overhead_pending: bool,
    active_mac_overhead_hold_logged: bool,
}

impl HrpdForwardSlotModulator {
    /// Construct the forward-link modulator. The scheduler is fed a slot
    /// index derived from the live chip cursor; the overhead schedule flags
    /// Control slots at the start of each Control Channel cycle and the
    /// modulator auto-builds a capsule from the overhead-message sources
    /// (pre-populated with defaults; replace via the
    /// `set_overhead_*` setters once real sector parameters are available).
    pub fn new(pilot_offset: usize, short_code_length_chips: usize) -> Self {
        Self {
            spreader: Spreader::new(HrpdForwardPnSequence::new(
                pilot_offset,
                short_code_length_chips,
            )),
            pilot_offset,
            short_code_length_chips,
            next_chip: 0,
            aligned: false,
            scheduler: HrpdForwardScheduler::new(),
            overhead: OverheadSchedule::defaults(),
            current_slot: None,
            slot_data_chips: Vec::new(),
            slot_data_cursor: 0,
            slot_mac_chips: Vec::new(),
            slot_mac_cursor: 0,
            mac_encoder: HrpdForwardMacEncoder::new(),
            control: ControlChannelModulator::new(),
            quick_config: Some(QuickConfig::defaults()),
            sector_params: Some(SectorParameters::defaults()),
            access_params: Some(AccessParameters::defaults()),
            reverse_rate: Some(BroadcastReverseRateLimit {
                rpc_count: 1,
                rate_limit: vec![5],
            }),
            sync_msg: Some(SyncMessage::defaults()),
            last_cycle_loaded: None,
            pending_signaling: VecDeque::new(),
            deferred_signaling: Vec::new(),
            pending_sync_signaling: VecDeque::new(),
            active_mac_overhead_pending: false,
            active_mac_overhead_hold_logged: false,
        }
    }

    /// Load a Forward Control Channel capsule for transmission on the next
    /// Control slot(s). Returns true if the capsule fit a supported turbo
    /// block. Auto-cycle wiring builds capsules itself; this entry point
    /// remains useful for tests and externally-built capsules.
    pub fn load_control_capsule(
        &mut self,
        capsule: &crate::bts::hrpd::control_channel::ControlChannelCapsule,
    ) -> bool {
        self.control.load_capsule(capsule)
    }

    /// Install (or replace) the current overhead-message bodies. The auto
    /// capsule builder draws from these at each Control Channel cycle
    /// boundary; setting any field to `None` skips it in capsules.
    pub fn set_overhead_quick_config(&mut self, m: Option<QuickConfig>) {
        self.quick_config = m;
        self.last_cycle_loaded = None;
    }

    pub fn set_overhead_sector_params(&mut self, m: Option<SectorParameters>) {
        self.sector_params = m;
        self.last_cycle_loaded = None;
    }

    pub fn set_overhead_access_params(&mut self, m: Option<AccessParameters>) {
        self.access_params = m;
        self.last_cycle_loaded = None;
    }

    pub fn set_overhead_reverse_rate(&mut self, m: Option<BroadcastReverseRateLimit>) {
        self.reverse_rate = m;
        self.last_cycle_loaded = None;
    }

    pub fn set_overhead_sync(&mut self, m: Option<SyncMessage>) {
        self.sync_msg = m;
        self.last_cycle_loaded = None;
    }

    /// One-shot installer for the current explicit HRPD overhead values and
    /// optional 1x partner neighbor advert.
    pub fn install_sector_overheads(
        &mut self,
        pilot_pn: u16,
        one_x_partner: Option<(u8, u16, u16)>,
        evdo_band_class: u8,
        evdo_channel: u16,
        overhead: ResolvedHrpdOverheadConfig,
    ) {
        // QuickConfig: short, frequent. The AT uses ColorCode + sector_id24 to
        // detect changes in the overhead bundle; signatures control when the AT
        // re-reads the heavier SectorParameters / AccessParameters.
        let mut qc = QuickConfig::defaults();
        qc.color_code = overhead.color_code;
        qc.sector_id24 = overhead.sector_id24();
        qc.sector_signature = overhead.sector_signature;
        qc.access_signature = overhead.access_signature;
        // Rev 0 ATs monitor ForwardTrafficValid for their assigned MACIndex
        // during Traffic Channel setup. If the overhead omits the bit, the MAC
        // layer treats it as 0 and can tear the reverse pilot down before it
        // accepts RTCAck. Advertise the whole subtype-0 1..63 MAC range valid.
        qc.rpc_count = 63;
        qc.forward_traffic_valid = vec![true; 63];
        self.quick_config = Some(qc);

        // SectorParameters: full sector identity + active HRPD channel. Mixed
        // 1x/HRPD deployments also carry a 1x partner neighbor so a hybrid AT
        // can find the 1x carrier for cross-paging / voice fallback.
        let mut sp = SectorParameters::defaults();
        sp.sector_id = overhead.sector_id;
        sp.subnet_mask = overhead.subnet_mask;
        sp.sector_signature = overhead.sector_signature;
        sp.channels = vec![cdma_common::hrpd::messages::ChannelRecord {
            system_type: 0x00, // HRPD
            band_class: evdo_band_class & 0x1F,
            channel_number: evdo_channel & 0x07FF,
        }];
        if let Some((one_x_band_class, one_x_channel, one_x_pilot_pn)) = one_x_partner {
            sp = sp.with_one_x_neighbor(one_x_band_class, one_x_channel, one_x_pilot_pn);
        }
        self.sector_params = Some(sp);

        // AccessParameters: keep the spec defaults (preamble length, probe
        // sequence, persistence vectors). These are tunable per deployment
        // but the defaults are acquirable.
        let mut ap = AccessParameters::defaults();
        ap.access_signature = overhead.access_signature;
        self.access_params = Some(ap);

        self.reverse_rate = Some(BroadcastReverseRateLimit {
            // C.S0024-300 §1.10.6.3.3 requires RPCCount >= 64 - the smallest
            // assigned subtype-0 MAC index. Cover MAC 1..64 with a uniform
            // 153.6 kbps cap; assigned live MACs are in the low Rev 0 range.
            rpc_count: 63,
            rate_limit: vec![5; 63],
        });

        // Sync: advertise Rev 0 compatibility by default. The live HRPD
        // carriers we decode use 1/1 here; advertising a higher minimum
        // revision can make older ATs reject the sector during acquisition.
        // PilotPN must reflect the actual sector PN offset. SystemTime is
        // updated live per-cycle inside maybe_advance_slot, so it stays 0
        // here until the first capsule build fills it.
        let mut sync = SyncMessage::defaults();
        sync.maximum_revision = 1;
        sync.minimum_revision = 1;
        sync.pilot_pn = pilot_pn & 0x01FF;
        self.sync_msg = Some(sync);

        self.last_cycle_loaded = None;
    }

    /// Build a capsule from the slots fired by the overhead schedule at
    /// `cycle_index`. Returns `None` when no source was configured for any
    /// fired slot.
    fn build_capsule_for_cycle(
        &mut self,
        cycle_index: u64,
        scheduled_overhead_fires: bool,
    ) -> Option<ControlChannelCapsule> {
        let fires = self.overhead.slots_for_cycle(cycle_index);
        let has_scheduled_overhead = scheduled_overhead_fires
            && (fires.sync
                || fires.quick_config
                || fires.sector_params
                || fires.access_params
                || fires.reverse_rate
                || !self.pending_sync_signaling.is_empty());
        if !has_scheduled_overhead {
            let messages = self.pop_pending_signaling_for_capsule();
            if messages.is_empty() {
                return None;
            }
            return Some(ControlChannelCapsule::new_asynchronous_default_signaling(
                messages,
                ctrl_ch_kbps(),
            ));
        }

        let mut bodies: Vec<Vec<u8>> = Vec::new();
        if fires.sync {
            if let Some(m) = &self.sync_msg {
                bodies.push(m.encode());
            }
        }
        if fires.quick_config {
            if let Some(m) = &self.quick_config {
                bodies.push(m.encode());
            }
        }
        if fires.sector_params {
            if let Some(m) = &self.sector_params {
                bodies.push(m.encode());
            }
        }
        if fires.access_params {
            if let Some(m) = &self.access_params {
                bodies.push(m.encode());
            }
        }
        if fires.reverse_rate {
            if let Some(m) = &self.reverse_rate {
                bodies.push(m.encode());
            }
        }
        // Directed signaling rides the synchronous capsule so slotted ATs
        // (which only decode the cycle-boundary Sleep State capsule) receive
        // it. Fill capacity-aware: an oversized capsule is rejected whole by
        // the modulator, losing the overhead messages with it.
        let mut capsule_bits = CONTROL_CAPSULE_HEADER_BITS
            + bodies
                .iter()
                .map(|body| capsule_overhead_body_bits(body))
                .sum::<usize>();
        let mut directed: Vec<ControlChannelDefaultSignalingMessage> = Vec::new();
        let scheduled_sync_due =
            has_due_scheduled_sync_signaling(&self.pending_sync_signaling, cycle_index);
        if scheduled_sync_due {
            // C.S0024-400-C Idle State Page is a sleep-state synchronous
            // control message. Keep that capsule scoped to explicitly
            // scheduled sleep-state traffic; unrelated async responses can
            // ride normal async capsules and should not be coalesced into the
            // AT's paging capsule.
            drain_due_scheduled_sync_signaling(
                &mut self.pending_sync_signaling,
                &mut directed,
                &mut capsule_bits,
                cycle_index,
            );
        } else {
            drain_signaling_into_sync_capsule(
                &mut self.pending_sync_signaling,
                &mut directed,
                &mut capsule_bits,
                cycle_index,
            );
            // Async-queue stragglers enqueued in the last few slots may as
            // well ride this capsule too instead of waiting for the next async
            // slot, as long as this is not an explicitly scheduled sleep-state
            // capsule.
            drain_signaling_into_sync_capsule(
                &mut self.pending_signaling,
                &mut directed,
                &mut capsule_bits,
                cycle_index,
            );
        }
        // A message carried by this capsule doesn't need its asynchronous
        // copy anymore — every AT that decodes async capsules decodes the
        // synchronous capsule too.
        for message in &directed {
            if let Some(pos) = self
                .pending_signaling
                .iter()
                .position(|pending| pending == message)
            {
                self.pending_signaling.remove(pos);
            }
        }
        if bodies.is_empty() && directed.is_empty() {
            return None;
        }
        if directed.is_empty() {
            Some(ControlChannelCapsule::new(bodies, ctrl_ch_kbps()))
        } else {
            Some(ControlChannelCapsule::new_with_default_signaling(
                bodies,
                directed,
                ctrl_ch_kbps(),
            ))
        }
    }

    /// Set the MAC encoder's RA bit (broadcast Reverse Activity).
    pub fn set_ra(&mut self, ra: bool) {
        self.mac_encoder.set_ra(ra);
    }

    /// Replace the active MAC index set on the MAC encoder.
    pub fn set_active_macs(&mut self, actives: Vec<crate::bts::hrpd::mac_encoder::ActiveMac>) {
        self.mac_encoder.set_actives(actives);
        self.refresh_active_mac_overhead();
    }

    /// Enqueue a forward-traffic packet on the scheduler.
    pub fn enqueue_traffic(&mut self, packet: crate::bts::hrpd::scheduler::ForwardTrafficPacket) {
        self.scheduler.enqueue(packet);
    }

    /// Purge queued/active forward traffic and H-ARQ bus state for a released
    /// MAC index.
    pub fn purge_traffic_mac(&mut self, mac_index: u8) -> (usize, usize, usize, usize) {
        self.scheduler.purge_mac(mac_index)
    }

    /// Wire the H-ARQ event bus shared with the per-MAC reverse traffic RX
    /// workers. The scheduler uses it to publish forward-subpacket emissions
    /// and to consume decoded ACK/NAK feedback.
    pub fn set_harq_bus(&mut self, bus: std::sync::Arc<crate::bts::hrpd::HarqBus>) {
        self.mac_encoder.set_harq_bus(bus.clone());
        self.scheduler.set_harq_bus(bus);
    }

    fn refresh_active_mac_overhead(&mut self) {
        let actives = self.mac_encoder.actives();
        let active_mac_indices = active_mac_indices_from_encoder(&self.mac_encoder);
        let Some(min_mac) = actives
            .iter()
            .filter_map(|active| {
                (5..64)
                    .contains(&active.mac_index)
                    .then_some(active.mac_index)
            })
            .min()
        else {
            if let Some(qc) = self.quick_config.as_mut() {
                qc.rpc_count = 0;
                qc.forward_traffic_valid.clear();
            }
            self.reverse_rate = Some(BroadcastReverseRateLimit {
                rpc_count: 1,
                rate_limit: vec![5],
            });
            self.last_cycle_loaded = None;
            self.active_mac_overhead_pending = false;
            info!(
                "HRPD overhead active MAC update: active_macs=[] qc_rpc_count=0 ftv=[] reverse_rate_rpc_count=1"
            );
            return;
        };

        // C.S0024-300 §1.10.6.3.3 requires RPCCount to cover down to the
        // smallest assigned MAC index. The occurrence order is MAC 63, 62, ...
        // so MAC 5 requires 59 occurrences and index 58 in the vectors below.
        let rpc_count = 64u8.saturating_sub(min_mac);
        let mut forward_traffic_valid = vec![false; rpc_count as usize];
        for active in actives {
            if (5..64).contains(&active.mac_index) {
                let idx = 63u8.saturating_sub(active.mac_index) as usize;
                if let Some(slot) = forward_traffic_valid.get_mut(idx) {
                    *slot = true;
                }
            }
        }

        if let Some(qc) = self.quick_config.as_mut() {
            qc.rpc_count = rpc_count;
            qc.forward_traffic_valid = forward_traffic_valid;
        }
        self.reverse_rate = Some(BroadcastReverseRateLimit {
            rpc_count,
            rate_limit: vec![5; rpc_count as usize],
        });
        if let (Some(qc), Some(rr)) = (&self.quick_config, &self.reverse_rate) {
            info!(
                "HRPD overhead active MAC update: active_macs=[{}] min_mac={} qc_rpc_count={} ftv=[{}] reverse_rate_rpc_count={} limits=[{}]",
                format_mac_list(&active_mac_indices),
                min_mac,
                qc.rpc_count,
                format_quick_config_active_ftv(qc, &active_mac_indices),
                rr.rpc_count,
                format_reverse_rate_active_limits(rr, &active_mac_indices),
            );
        }
        self.active_mac_overhead_pending = true;
        self.active_mac_overhead_hold_logged = false;
        self.last_cycle_loaded = None;
    }

    /// Queue one AN-originated Default Signaling message for the Forward
    /// Control Channel. Overhead capsules remain scheduled from the normal
    /// `OverheadSchedule`; these messages are sent as soon as the Control
    /// Channel is free.
    pub fn enqueue_forward_signaling(&mut self, request: HrpdForwardSignalingRequest) {
        let repeat_offsets = if request.channel == HrpdForwardChannel::AsynchronousControl
            && request.reliable_sequence.is_none()
            && is_traffic_channel_assignment_request(&request)
        {
            &TRAFFIC_ASSIGNMENT_REPEAT_OFFSETS_SLOTS[..]
        } else {
            &[][..]
        };
        let repetitions = 1 + repeat_offsets.len();
        info!(
            "HRPD forward signaling queued ati={:?} proto=0x{:02x} bytes={} slp={} repetitions={} synchronous_cycle={:?}",
            request.target_ati,
            request.protocol_type,
            request.payload.len(),
            match request.reliable_sequence {
                Some(seq) => format!("reliable:{seq}"),
                None => "best_effort".to_string(),
            },
            repetitions,
            request.synchronous_control_cycle,
        );
        let message = ControlChannelDefaultSignalingMessage {
            ati: request.target_ati,
            protocol_type: request.protocol_type,
            payload: request.payload.clone(),
            reliable_sequence: request.reliable_sequence,
            synchronous_control_cycle: request.synchronous_control_cycle,
        };
        self.queue_signaling_copies(message.clone(), request.channel.clone());
        if !repeat_offsets.is_empty() {
            let current_slot = self
                .current_slot
                .unwrap_or_else(|| self.next_chip / SLOT_CHIPS);
            // C.S0024-0 §6.6.6.1.3.2 permits retransmitting the same
            // TrafficChannelAssignment for delivery probability, with the
            // same MessageSequence. Keep repeats early in TRTCMPATSetup.
            self.deferred_signaling.extend(
                repeat_offsets
                    .iter()
                    .map(|offset| (current_slot.saturating_add(*offset), message.clone())),
            );
        }
    }

    /// Queue one directed signaling message on both delivery paths: the
    /// asynchronous queue (heard by ATs monitoring the Control Channel
    /// continuously) and the synchronous-capsule queue (the only path a
    /// slotted AT receives). Duplicate delivery is harmless — the AT
    /// deduplicates by message sequence / SLP-D V(R).
    fn queue_signaling_copies(
        &mut self,
        message: ControlChannelDefaultSignalingMessage,
        channel: HrpdForwardChannel,
    ) {
        let directed = message.ati.ati_type != AccessTerminalIdentifierType::Bati;
        let sync_copy_allowed = directed && !is_stateful_setup_message(&message);
        match channel {
            HrpdForwardChannel::SynchronousControl => {
                if directed {
                    self.push_sync_signaling(message);
                }
            }
            HrpdForwardChannel::AsynchronousControl | HrpdForwardChannel::ForwardTraffic => {
                if sync_copy_allowed {
                    self.push_sync_signaling(message.clone());
                }
                self.pending_signaling.push_back(message);
            }
        }
    }

    fn push_sync_signaling(&mut self, message: ControlChannelDefaultSignalingMessage) {
        self.pending_sync_signaling.push_back(message);
        while self.pending_sync_signaling.len() > MAX_PENDING_SYNC_SIGNALING {
            let dropped = self.pending_sync_signaling.pop_front();
            warn!(
                "HRPD sync signaling queue overflow; dropping oldest copy {:?}",
                dropped.map(|m| (m.ati, m.protocol_type, m.payload.len()))
            );
        }
    }

    /// Promote deferred best-effort repeat copies whose earliest slot has
    /// arrived into the live signaling queues.
    fn promote_due_deferred_signaling(&mut self, slot_index: u64) {
        if self.deferred_signaling.is_empty() {
            return;
        }
        let mut idx = 0;
        while idx < self.deferred_signaling.len() {
            if self.deferred_signaling[idx].0 <= slot_index {
                let (_, message) = self.deferred_signaling.remove(idx);
                self.queue_signaling_copies(message, HrpdForwardChannel::AsynchronousControl);
            } else {
                idx += 1;
            }
        }
    }

    /// Generate `out.len()` spread chips starting at `chip_cursor` directly into
    /// the caller's buffer. The TX synth thread uses this to fill its batch
    /// slice without a per-block heap allocation.
    pub fn next_block_into(&mut self, chip_cursor: u64, out: &mut [Complex32]) {
        self.align_to_chip(chip_cursor);
        for (idx, slot) in out.iter_mut().enumerate() {
            let chip = chip_cursor + idx as u64;
            self.maybe_advance_slot(chip);
            let value = match channel_for_chip(chip) {
                SlotChannel::Pilot => Complex32::new(1.0, 0.0),
                SlotChannel::Mac => self.next_mac_chip(),
                SlotChannel::Data => self.next_data_chip(),
            };
            *slot = self.spreader.spread(&value);
        }
        self.next_chip = chip_cursor + out.len() as u64;
    }

    /// Convenience wrapper that allocates and returns the block. Used by tests
    /// and offline helpers; the TX hot path uses `next_block_into`.
    pub fn next_block(&mut self, chip_cursor: u64, block_size: usize) -> Vec<Complex32> {
        let mut block = vec![Complex32::new(0.0, 0.0); block_size];
        self.next_block_into(chip_cursor, &mut block);
        block
    }

    fn maybe_advance_slot(&mut self, chip: u64) {
        let slot_index = chip / SLOT_CHIPS;
        if self.current_slot == Some(slot_index) {
            return;
        }
        self.promote_due_deferred_signaling(slot_index);
        let cycle = u64::from(CTRL_CH_CYCLE_SLOTS);
        let at_cycle_boundary = cycle > 0 && slot_index % cycle == 0;
        let cycle_slot = if cycle > 0 { slot_index % cycle } else { 0 };
        let cycle_index = if cycle > 0 { slot_index / cycle } else { 0 };
        let scheduled_overhead_fires = at_cycle_boundary && {
            let s = self.overhead.slots_for_cycle(cycle_index);
            s.quick_config
                || s.sector_params
                || s.access_params
                || s.sync
                || s.reverse_rate
                // Directed sync-capsule copies also warrant a synchronous
                // capsule this cycle, even if no overhead slot fires.
                || !self.pending_sync_signaling.is_empty()
        };
        let async_control_can_start = (cycle == 0
            || cycle_slot <= cycle.saturating_sub(hrpd_control_packet_slot_span()))
            && slot_index % 4 == 0;
        let starts_control_capsule = scheduled_overhead_fires
            || (!self.pending_signaling.is_empty() && async_control_can_start);

        // Auto-build a capsule from the configured overhead bodies whenever
        // this is a new cycle boundary that fires at least one source. We
        // condition on `last_cycle_loaded` so changing a setter mid-cycle doesn't
        // tear the in-flight capsule.
        if starts_control_capsule
            && self.control.remaining() == 0
            && (scheduled_overhead_fires || !self.pending_signaling.is_empty())
            && (!scheduled_overhead_fires || self.last_cycle_loaded != Some(cycle_index))
        {
            // C.S0024-0 v4.0 §6.3.6.2.1: SystemTime is "the System Time 160 ms
            // after the start of the Control Channel Cycle in which the Sync
            // message is being sent" in units of 26.66… ms.
            //
            // Our `chip` cursor is absolute chips-since-CDMA-epoch (initialized
            // by `timing::compute_initial_tx_anchor` from
            // `chips_since_epoch(start_system_time)` and incremented chip-for-
            // chip from there). At a cycle boundary `chip == cycle_start_chip`,
            // so:
            //   SystemTime [ticks] = (chip + 160 ms × 1.2288 Mcps) / 32768
            //                      = (chip + 196_608) / 32_768
            //
            // 196_608 / 32_768 = 6 exactly, so the +160 ms offset is precisely
            // 6 ticks ahead of cycle start — no rounding error.
            const SYSTEM_TIME_CHIPS_PER_TICK: u64 = 32_768;
            const SYNC_OFFSET_CHIPS: u64 = 196_608;
            const SYSTEM_TIME_MASK: u64 = (1u64 << 37) - 1;
            if let Some(sync) = self.sync_msg.as_mut() {
                sync.system_time =
                    ((chip + SYNC_OFFSET_CHIPS) / SYSTEM_TIME_CHIPS_PER_TICK) & SYSTEM_TIME_MASK;
            }
            if let Some(capsule) =
                self.build_capsule_for_cycle(cycle_index, scheduled_overhead_fires)
            {
                let active_mac_overhead_was_pending = self.active_mac_overhead_pending;
                let has_quick_config = capsule
                    .control_messages()
                    .iter()
                    .any(|message| match message {
                        crate::bts::hrpd::control_channel::ControlChannelMessage::InferredOverhead(body) => {
                            body.first().copied()
                                == Some(cdma_common::hrpd::messages::QUICK_CONFIG_MESSAGE_ID)
                        }
                        crate::bts::hrpd::control_channel::ControlChannelMessage::DefaultSignaling(_) => {
                            false
                        }
                    });
                let loaded = self.control.load_capsule(&capsule);
                // Build the per-message summary lazily: the TX synth thread must
                // not allocate the summary Vec/Strings on every capsule load when
                // the log won't emit. The rejection path is rare; the loaded path
                // only runs when Debug logging is enabled.
                let message_summaries = |encoder: &HrpdForwardMacEncoder| {
                    let active_mac_indices = active_mac_indices_from_encoder(encoder);
                    capsule
                        .control_messages()
                        .iter()
                        .map(|message| match message {
                            crate::bts::hrpd::control_channel::ControlChannelMessage::InferredOverhead(body) => {
                                format_overhead_body(body, &active_mac_indices)
                            }
                            crate::bts::hrpd::control_channel::ControlChannelMessage::DefaultSignaling(message) => {
                                format!(
                                    "default:{:?}/0x{:02x}/{}B/{}",
                                    message.ati.ati_type,
                                    message.protocol_type,
                                    message.payload.len(),
                                    match message.reliable_sequence {
                                        Some(seq) => format!("reliable:{seq}"),
                                        None => "best_effort".to_string(),
                                    }
                                )
                            }
                        })
                        .collect::<Vec<_>>()
                        .join(",")
                };
                if loaded {
                    if active_mac_overhead_was_pending && capsule.synchronous && has_quick_config {
                        self.active_mac_overhead_pending = false;
                        self.active_mac_overhead_hold_logged = false;
                        info!(
                            "HRPD active MAC public data loaded in synchronous QuickConfig slot={} cycle={} messages=[{}]",
                            slot_index,
                            cycle_index,
                            message_summaries(&self.mac_encoder)
                        );
                    }
                    if log::log_enabled!(log::Level::Trace) {
                        trace!(
                            "HRPD control capsule loaded slot={} cycle={} scheduled_overhead={} messages=[{}]",
                            slot_index,
                            cycle_index,
                            scheduled_overhead_fires,
                            message_summaries(&self.mac_encoder)
                        );
                    }
                } else {
                    warn!(
                        "HRPD control capsule rejected slot={} cycle={} scheduled_overhead={} messages=[{}]",
                        slot_index,
                        cycle_index,
                        scheduled_overhead_fires,
                        message_summaries(&self.mac_encoder)
                    );
                }
                if scheduled_overhead_fires {
                    self.last_cycle_loaded = Some(cycle_index);
                }
            }
        }
        let is_control = self.control.remaining() > 0 && slot_index % 4 == 0;
        let hold_traffic_for_active_mac_overhead =
            self.active_mac_overhead_pending && !is_control && !self.scheduler.has_active_packets();
        if hold_traffic_for_active_mac_overhead && !self.active_mac_overhead_hold_logged {
            log::info!(
                "HRPD forward traffic hold: waiting for active MAC public data before starting queued traffic slot={}",
                slot_index
            );
            self.active_mac_overhead_hold_logged = true;
        }
        let out = if hold_traffic_for_active_mac_overhead {
            crate::bts::hrpd::scheduler::ForwardSlotOutput {
                channel: SlotKind::Idle,
                data_chips: Vec::new(),
                mac_bits: Vec::new(),
            }
        } else {
            self.scheduler.next_slot(slot_index, is_control)
        };
        self.current_slot = Some(slot_index);
        self.slot_data_cursor = 0;
        self.slot_data_chips = match out.channel {
            SlotKind::Traffic { .. } => out.data_chips,
            SlotKind::Control => self.control.next_slot_chips(),
            SlotKind::Idle => Vec::new(),
        };
        debug_assert!(
            self.slot_data_chips.is_empty() || self.slot_data_chips.len() == DATA_CHIPS_PER_SLOT
        );
        self.slot_mac_cursor = 0;
        self.mac_encoder
            .next_slot_chips_into(slot_index, &mut self.slot_mac_chips);
    }

    fn next_data_chip(&mut self) -> Complex32 {
        if self.slot_data_cursor < self.slot_data_chips.len() {
            let v = self.slot_data_chips[self.slot_data_cursor];
            self.slot_data_cursor += 1;
            v
        } else {
            Complex32::new(0.0, 0.0)
        }
    }

    fn next_mac_chip(&mut self) -> Complex32 {
        if self.slot_mac_cursor < self.slot_mac_chips.len() {
            let v = self.slot_mac_chips[self.slot_mac_cursor];
            self.slot_mac_cursor += 1;
            v
        } else {
            Complex32::new(0.0, 0.0)
        }
    }

    fn align_to_chip(&mut self, chip_cursor: u64) {
        if !self.aligned {
            self.spreader.align_to_chip(chip_cursor);
            self.next_chip = chip_cursor;
            self.aligned = true;
        } else if chip_cursor >= self.next_chip {
            self.spreader.advance_chips(chip_cursor - self.next_chip);
            self.next_chip = chip_cursor;
        } else {
            self.spreader = Spreader::new(HrpdForwardPnSequence::new(
                self.pilot_offset,
                self.short_code_length_chips,
            ));
            self.spreader.align_to_chip(chip_cursor);
            self.next_chip = chip_cursor;
        }
    }

    fn pop_pending_signaling_for_capsule(&mut self) -> Vec<ControlChannelDefaultSignalingMessage> {
        let mut out = Vec::new();
        while out.len() < MAX_DEFAULT_SIGNALING_PER_CONTROL_CAPSULE
            && let Some(message) = self.pending_signaling.pop_front()
        {
            out.push(message);
        }
        out
    }
}

fn is_stateful_setup_message(message: &ControlChannelDefaultSignalingMessage) -> bool {
    (message.protocol_type == DEFAULT_ADDRESS_MANAGEMENT_PROTOCOL_TYPE
        && message.payload.first() == Some(&HrpdUatiAssignment::MESSAGE_ID))
        || (message.protocol_type == DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE
            && message.payload.first() == Some(&HrpdTrafficChannelAssignment::MESSAGE_ID))
}

fn is_traffic_channel_assignment_request(request: &HrpdForwardSignalingRequest) -> bool {
    request.protocol_type == DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE
        && request.payload.first() == Some(&HrpdTrafficChannelAssignment::MESSAGE_ID)
}

pub struct AdjacentCarrierComposer {
    one_x_shaper: TxPulseShaper,
    evdo_shaper: TxPulseShaper,
    one_x_rotator: PhasorNco,
    evdo_rotator: PhasorNco,
    evdo_gain: f32,
    tx_digital_backoff: f32,
    one_x_buf: Vec<Complex32>,
    evdo_buf: Vec<Complex32>,
}

impl AdjacentCarrierComposer {
    pub fn new(
        config: &ResolvedEvdoConfig,
        tx_sample_rate_hz: usize,
        tx_digital_backoff: f32,
    ) -> Result<Self, Error> {
        Ok(Self {
            one_x_shaper: TxPulseShaper::new(tx_sample_rate_hz)?,
            evdo_shaper: TxPulseShaper::new(tx_sample_rate_hz)?,
            one_x_rotator: PhasorNco::from_offset_hz(config.one_x_shift_hz, tx_sample_rate_hz),
            evdo_rotator: PhasorNco::from_offset_hz(config.evdo_shift_hz, tx_sample_rate_hz),
            evdo_gain: config.gain,
            tx_digital_backoff,
            one_x_buf: Vec::new(),
            evdo_buf: Vec::new(),
        })
    }

    /// Alloc-free [`compose`](Self::compose) writing into a caller buffer.
    pub fn compose_into(
        &mut self,
        one_x_chips: &[Complex32],
        evdo_chips: &[Complex32],
        out: &mut Vec<Complex32>,
    ) {
        self.one_x_shaper
            .shape_into(one_x_chips, &mut self.one_x_buf);
        self.evdo_shaper.shape_into(evdo_chips, &mut self.evdo_buf);
        self.one_x_rotator.rotate_in_place(&mut self.one_x_buf);
        self.evdo_rotator.rotate_in_place(&mut self.evdo_buf);
        // `evdo_gain` is the linear amplitude ratio HRPD : 1x at the chip
        // output. `composite_scale` keeps the summed peak bounded when both
        // carriers are at full scale. `tx_digital_backoff` is applied to BOTH
        // branches symmetrically so it never silently de-rates HRPD relative
        // to 1x.
        let g = self.evdo_gain.max(0.0);
        let composite_scale = self.tx_digital_backoff / (1.0 + g);
        out.clear();
        out.reserve(self.one_x_buf.len());
        out.extend(
            self.one_x_buf
                .iter()
                .zip(&self.evdo_buf)
                .map(|(one_x_sample, evdo_sample)| {
                    (one_x_sample + evdo_sample * g) * composite_scale
                }),
        );
    }

    pub fn compose(
        &mut self,
        one_x_chips: &[Complex32],
        evdo_chips: &[Complex32],
    ) -> Vec<Complex32> {
        let mut out = Vec::new();
        self.compose_into(one_x_chips, evdo_chips, &mut out);
        out
    }
}

#[cfg(test)]
mod modulator_tests {
    use super::*;
    use crate::bts::hrpd::scheduler::ForwardTrafficPacket;
    use crate::phy::hrpd::slot::{SLOT_CHIPS, SlotChannel, channel_for_chip};
    use num::complex::Complex32;

    fn block(modulator: &mut HrpdForwardSlotModulator, start: u64, n: usize) -> Vec<Complex32> {
        modulator.next_block(start, n)
    }

    #[test]
    fn hrpd_overhead_config_resolves_explicit_sector_identity() {
        let cfg: HrpdOverheadConfig = serde_json::from_str(
            r#"{
  "sector_id": "0x0080:0580:0000:0000:0000:0000:0000:0000",
  "subnet_mask": 26,
  "color_code": 26,
  "sector_signature": 2,
  "access_signature": 3
}"#,
        )
        .expect("parse HRPD overhead config");

        let resolved = cfg.resolve().expect("explicit overhead resolves");
        assert_eq!(
            resolved.sector_id,
            [
                0x00, 0x80, 0x05, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00,
            ]
        );
        assert_eq!(resolved.subnet_mask, 26);
        assert_eq!(resolved.color_code, 26);
        assert_eq!(resolved.sector_id24(), 0);
        assert_eq!(resolved.sector_signature, 2);
        assert_eq!(resolved.access_signature, 3);
    }

    #[test]
    fn hrpd_overhead_config_rejects_missing_explicit_identity() {
        let err = HrpdOverheadConfig::default()
            .resolve()
            .expect_err("enabled EVDO should require explicit HRPD identity");
        assert!(
            err.to_string().contains("sector_id is required"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn hrpd_overhead_config_rejects_missing_explicit_color_code() {
        let cfg = HrpdOverheadConfig {
            sector_id: Some(HrpdSectorId::new([0u8; 16])),
            subnet_mask: Some(26),
            color_code: None,
            ..HrpdOverheadConfig::default()
        };
        let err = cfg
            .resolve()
            .expect_err("enabled EVDO should require explicit HRPD color code");
        assert!(
            err.to_string().contains("color_code is required"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn hrpd_overhead_config_rejects_invalid_subnet_mask() {
        let cfg = HrpdOverheadConfig {
            sector_id: Some(HrpdSectorId::new([0u8; 16])),
            subnet_mask: Some(129),
            color_code: Some(26),
            ..HrpdOverheadConfig::default()
        };
        let err = cfg.resolve().expect_err("invalid subnet mask should fail");
        assert!(
            err.to_string().contains("subnet_mask must be in 0..=128"),
            "unexpected error: {err}"
        );
    }

    #[test]
    fn install_sector_overheads_can_omit_one_x_neighbor() {
        let overhead = HrpdOverheadConfig {
            sector_id: Some(HrpdSectorId::new([
                0x00, 0x80, 0x05, 0x80, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00, 0x00,
                0x00, 0x00,
            ])),
            subnet_mask: Some(26),
            color_code: Some(26),
            ..HrpdOverheadConfig::default()
        }
        .resolve()
        .expect("resolve overhead");
        let mut m = HrpdForwardSlotModulator::new(0, 32_768);
        m.install_sector_overheads(0, None, 0, 37, overhead);

        let sector = m.sector_params.as_ref().expect("sector params installed");
        assert_eq!(sector.channels.len(), 1);
        assert_eq!(sector.channels[0].system_type, 0x00);
        assert_eq!(sector.channels[0].channel_number, 37);
        assert!(
            sector.neighbors.is_empty(),
            "HRPD-only sector should not advertise a nonexistent 1x partner"
        );
    }

    #[test]
    fn idle_pilot_mode_emits_zero_outside_pilot_bursts() {
        let mut m = HrpdForwardSlotModulator::new(0, 32_768);
        m.set_overhead_quick_config(None);
        m.set_overhead_sector_params(None);
        m.set_overhead_access_params(None);
        m.set_overhead_reverse_rate(None);
        m.set_overhead_sync(None);
        m.set_ra(false);
        let out = block(&mut m, 0, SLOT_CHIPS as usize);
        for (i, sample) in out.iter().enumerate() {
            let chip = i as u64;
            match channel_for_chip(chip) {
                SlotChannel::Pilot => {
                    // Pilot is spread by PN; magnitude should be ~1.
                    let mag2 = sample.re * sample.re + sample.im * sample.im;
                    assert!(mag2 > 0.5, "pilot chip {chip} mag^2 {mag2}");
                }
                SlotChannel::Mac => {}
                SlotChannel::Data => {
                    let mag2 = sample.re * sample.re + sample.im * sample.im;
                    assert!(mag2 < 1e-6, "idle data chip {chip} should be zero");
                }
            }
        }
    }

    #[test]
    fn scheduler_mode_idle_slot_data_region_carries_overhead_mac_carries_ra() {
        // new() installs default overheads, so slot 0 (cycle boundary) fires
        // a Control capsule. Inspect slot 1 (Idle/Traffic-eligible) instead.
        let mut m = HrpdForwardSlotModulator::new(0, 32_768);
        // Skip slot 0 (Control with auto capsule); inspect slot 1.
        let start = SLOT_CHIPS;
        let out = block(&mut m, start, SLOT_CHIPS as usize);
        let mut mac_nonzero = 0;
        for (i, sample) in out.iter().enumerate() {
            let chip = start + i as u64;
            let mag2 = sample.re * sample.re + sample.im * sample.im;
            match channel_for_chip(chip) {
                SlotChannel::Pilot => assert!(mag2 > 0.5),
                SlotChannel::Data => assert!(mag2 < 1e-6, "data chip {chip} mag^2 {mag2}"),
                SlotChannel::Mac => {
                    if mag2 > 1e-6 {
                        mac_nonzero += 1;
                    }
                }
            }
        }
        assert!(
            mac_nonzero > 0,
            "expected non-zero MAC chips for RA broadcast"
        );
    }

    #[test]
    fn auto_overhead_fills_control_slot_without_manual_load() {
        // new() pre-installs the overhead message defaults so the
        // auto-capsule builder fires immediately on cycle boundary 0.
        let mut m = HrpdForwardSlotModulator::new(0, 32_768);
        // Slot 0 fires the first scheduled overhead capsule (Control slot).
        let out = block(&mut m, 0, SLOT_CHIPS as usize);
        let mut data_nonzero = 0;
        for (i, sample) in out.iter().enumerate() {
            let chip = i as u64;
            if matches!(channel_for_chip(chip), SlotChannel::Data) {
                let mag2 = sample.re * sample.re + sample.im * sample.im;
                if mag2 > 1e-6 {
                    data_nonzero += 1;
                }
            }
        }
        assert!(
            data_nonzero > 0,
            "expected non-zero Data chips from auto-built capsule, got {data_nonzero}"
        );
    }

    #[test]
    fn control_slot_data_region_filled_after_load_capsule() {
        let mut m = HrpdForwardSlotModulator::new(0, 32_768);
        // Clear the pre-installed overheads so the auto-capsule builder
        // doesn't pre-empt our manual capsule.
        m.set_overhead_quick_config(None);
        m.set_overhead_sector_params(None);
        m.set_overhead_access_params(None);
        m.set_overhead_reverse_rate(None);
        m.set_overhead_sync(None);
        let capsule = crate::bts::hrpd::control_channel::ControlChannelCapsule::new(
            vec![SyncMessage::defaults().encode()],
            crate::bts::hrpd::control_channel::ctrl_ch_kbps(),
        );
        assert!(m.load_control_capsule(&capsule));
        // Slot 0 is the Control slot.
        let out = block(&mut m, 0, SLOT_CHIPS as usize);
        let mut data_nonzero = 0;
        for (i, sample) in out.iter().enumerate() {
            let chip = i as u64;
            if matches!(channel_for_chip(chip), SlotChannel::Data) {
                let mag2 = sample.re * sample.re + sample.im * sample.im;
                if mag2 > 1e-6 {
                    data_nonzero += 1;
                }
            }
        }
        assert!(
            data_nonzero > 0,
            "expected non-zero Data chips on control slot, got {data_nonzero}"
        );
    }

    #[test]
    fn queued_forward_signaling_uses_next_async_control_opportunity() {
        let mut m = HrpdForwardSlotModulator::new(0, 32_768);
        m.set_overhead_quick_config(None);
        m.set_overhead_sector_params(None);
        m.set_overhead_access_params(None);
        m.set_overhead_reverse_rate(None);
        m.set_overhead_sync(None);
        let _ = block(&mut m, 0, SLOT_CHIPS as usize);
        m.enqueue_forward_signaling(HrpdForwardSignalingRequest {
            uati: Some(0x8005_8001),
            target_ati: cdma_common::hrpd::air::AccessTerminalIdentifier {
                ati_type: cdma_common::hrpd::air::AccessTerminalIdentifierType::Rati,
                value: 0x50ad_b764,
            },
            protocol_type: cdma_common::hrpd::air::DEFAULT_ADDRESS_MANAGEMENT_PROTOCOL_TYPE,
            payload: vec![0x01, 0x01, 0x00, 0x1a, 0x05, 0x80, 0x01, 0x00],
            channel: cdma_common::hrpd::air::HrpdForwardChannel::AsynchronousControl,
            reliable_sequence: None,
            synchronous_control_cycle: None,
        });

        let out = block(&mut m, SLOT_CHIPS, (SLOT_CHIPS * 7) as usize);
        let mut async_control_data_nonzero = 0;
        for (i, sample) in out.iter().enumerate() {
            let chip = SLOT_CHIPS + i as u64;
            if chip / SLOT_CHIPS == 4 && matches!(channel_for_chip(chip), SlotChannel::Data) {
                let mag2 = sample.re * sample.re + sample.im * sample.im;
                if mag2 > 1e-6 {
                    async_control_data_nonzero += 1;
                }
            }
        }
        assert!(
            async_control_data_nonzero > 0,
            "queued signaling should create an async Control capsule at the next control interlace slot"
        );
        assert_eq!(
            m.pending_signaling.len(),
            0,
            "UATIAssignment should be consumed by the next async Control capsule"
        );
        assert_eq!(
            m.deferred_signaling.len(),
            0,
            "UATIAssignment should not leave stale deferred repeat copies"
        );
    }

    #[test]
    fn uati_addressed_uati_assignment_is_not_repeated() {
        let mut m = HrpdForwardSlotModulator::new(0, 32_768);
        m.enqueue_forward_signaling(HrpdForwardSignalingRequest {
            uati: Some(0x8005_8002),
            target_ati: cdma_common::hrpd::air::AccessTerminalIdentifier {
                ati_type: cdma_common::hrpd::air::AccessTerminalIdentifierType::Uati,
                value: 0x1a05_8001,
            },
            protocol_type: cdma_common::hrpd::air::DEFAULT_ADDRESS_MANAGEMENT_PROTOCOL_TYPE,
            payload: vec![0x01, 0x02, 0x00, 0x1a, 0x05, 0x80, 0x02, 0x00],
            channel: cdma_common::hrpd::air::HrpdForwardChannel::AsynchronousControl,
            reliable_sequence: None,
            synchronous_control_cycle: None,
        });

        assert_eq!(
            m.pending_signaling.len(),
            1,
            "UATI-addressed reassignment should queue the first copy"
        );
        assert_eq!(
            m.pending_sync_signaling.len(),
            0,
            "UATIAssignment should not leave a stale synchronous fallback copy"
        );
        assert_eq!(
            m.deferred_signaling.len(),
            0,
            "UATI-addressed reassignment should not leave stale old-UATI repeat copies"
        );
    }

    #[test]
    fn async_control_does_not_start_too_late_to_finish_before_next_cycle() {
        let mut m = HrpdForwardSlotModulator::new(0, 32_768);
        let _ = block(&mut m, 0, (SLOT_CHIPS * 225) as usize);
        m.enqueue_forward_signaling(HrpdForwardSignalingRequest::access_channel_ack(
            cdma_common::hrpd::air::AccessTerminalIdentifier {
                ati_type: cdma_common::hrpd::air::AccessTerminalIdentifierType::Rati,
                value: 0x50ad_b764,
            },
        ));

        let _ = block(&mut m, SLOT_CHIPS * 225, (SLOT_CHIPS * 31) as usize);
        assert_eq!(
            m.pending_signaling.len(),
            1,
            "async capsule must not start after cycle slot 224 because it would overlap the next synchronous capsule"
        );

        let _ = block(&mut m, SLOT_CHIPS * 256, SLOT_CHIPS as usize);
        assert_eq!(
            m.pending_signaling.len(),
            0,
            "late async signaling may be carried by the next synchronous capsule"
        );
    }

    #[test]
    fn directed_signaling_rides_synchronous_capsule() {
        let mut m = HrpdForwardSlotModulator::new(0, 32_768);
        m.set_overhead_quick_config(None);
        m.set_overhead_sector_params(None);
        m.set_overhead_access_params(None);
        m.set_overhead_reverse_rate(None);
        m.set_overhead_sync(None);
        let _ = block(&mut m, 0, SLOT_CHIPS as usize);
        m.enqueue_forward_signaling(HrpdForwardSignalingRequest::access_channel_ack(
            cdma_common::hrpd::air::AccessTerminalIdentifier {
                ati_type: cdma_common::hrpd::air::AccessTerminalIdentifierType::Rati,
                value: 0x50ad_b764,
            },
        ));
        assert_eq!(
            m.pending_sync_signaling.len(),
            1,
            "directed message should queue a synchronous-capsule copy"
        );

        // The async copy airs at the next async opportunity; the sync copy
        // must survive it and wait for the cycle boundary.
        let _ = block(&mut m, SLOT_CHIPS, (SLOT_CHIPS * 7) as usize);
        assert_eq!(
            m.pending_signaling.len(),
            0,
            "async copy should be consumed by an async capsule"
        );
        assert_eq!(
            m.pending_sync_signaling.len(),
            1,
            "sync copy must wait for the cycle-boundary capsule"
        );

        // Crossing the next cycle boundary carries the sync copy in a
        // synchronous capsule — built even with no overhead slot scheduled.
        let _ = block(&mut m, SLOT_CHIPS * 8, (SLOT_CHIPS * 249) as usize);
        assert_eq!(
            m.pending_sync_signaling.len(),
            0,
            "sync copy should ride the cycle-boundary synchronous capsule"
        );
    }

    #[test]
    fn scheduled_sync_signaling_waits_for_matching_control_cycle() {
        let mut m = HrpdForwardSlotModulator::new(0, 32_768);
        m.set_overhead_quick_config(None);
        m.set_overhead_sector_params(None);
        m.set_overhead_access_params(None);
        m.set_overhead_reverse_rate(None);
        m.set_overhead_sync(None);
        let ati = cdma_common::hrpd::air::AccessTerminalIdentifier {
            ati_type: cdma_common::hrpd::air::AccessTerminalIdentifierType::Uati,
            value: 0x1a05_8001,
        };
        m.enqueue_forward_signaling(
            HrpdForwardSignalingRequest::idle_state_page_for_control_cycle(
                0x1a05_8001,
                ati,
                Some(cdma_common::hrpd::air::HrpdSynchronousControlCycle {
                    modulus: 12,
                    residue: 2,
                }),
            ),
        );

        assert_eq!(m.pending_sync_signaling.len(), 1);
        assert!(
            m.build_capsule_for_cycle(1, true).is_none(),
            "scheduled Page must not ride the wrong control cycle"
        );
        assert_eq!(m.pending_sync_signaling.len(), 1);

        let capsule = m
            .build_capsule_for_cycle(2, true)
            .expect("scheduled Page should ride matching control cycle");
        assert_eq!(capsule.messages.len(), 1);
        assert_eq!(capsule.messages[0], vec![0x00]);
        assert_eq!(m.pending_sync_signaling.len(), 0);
    }

    #[test]
    fn scheduled_sync_page_does_not_coalesce_unscheduled_sync_copies() {
        let mut m = HrpdForwardSlotModulator::new(0, 32_768);
        m.set_overhead_quick_config(None);
        m.set_overhead_sector_params(None);
        m.set_overhead_access_params(None);
        m.set_overhead_reverse_rate(None);
        m.set_overhead_sync(None);
        let ati = cdma_common::hrpd::air::AccessTerminalIdentifier {
            ati_type: cdma_common::hrpd::air::AccessTerminalIdentifierType::Uati,
            value: 0x1a05_8001,
        };
        m.enqueue_forward_signaling(HrpdForwardSignalingRequest::access_channel_ack(ati));
        m.enqueue_forward_signaling(
            HrpdForwardSignalingRequest::idle_state_page_for_control_cycle(
                0x1a05_8001,
                ati,
                Some(cdma_common::hrpd::air::HrpdSynchronousControlCycle {
                    modulus: 12,
                    residue: 2,
                }),
            ),
        );

        let capsule = m
            .build_capsule_for_cycle(2, true)
            .expect("scheduled Page capsule");

        assert!(capsule.synchronous);
        assert_eq!(capsule.messages.len(), 1);
        assert_eq!(capsule.messages[0], vec![0x00]);
        assert_eq!(
            m.pending_sync_signaling.len(),
            1,
            "unscheduled sync copy must wait for a non-page synchronous capsule"
        );
        assert_eq!(
            m.pending_signaling.len(),
            1,
            "async ACAck copy should remain available for async control delivery"
        );
    }

    #[test]
    fn queued_forward_signaling_keeps_one_message_per_capsule() {
        let mut m = HrpdForwardSlotModulator::new(0, 32_768);
        m.set_overhead_quick_config(None);
        m.set_overhead_sector_params(None);
        m.set_overhead_access_params(None);
        m.set_overhead_reverse_rate(None);
        m.set_overhead_sync(None);
        let ati = cdma_common::hrpd::air::AccessTerminalIdentifier {
            ati_type: cdma_common::hrpd::air::AccessTerminalIdentifierType::Uati,
            value: 0x1a05_8001,
        };
        m.enqueue_forward_signaling(HrpdForwardSignalingRequest::access_channel_ack(ati));
        m.enqueue_forward_signaling(HrpdForwardSignalingRequest {
            uati: Some(0x8005_8001),
            target_ati: ati,
            protocol_type: cdma_common::hrpd::air::DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE,
            payload: vec![
                0x01, 0x00, 0x80, 0x01, 0x3b, 0x03, 0xd7, 0x42, 0x00, 0x0a, 0x00,
            ],
            channel: cdma_common::hrpd::air::HrpdForwardChannel::AsynchronousControl,
            reliable_sequence: None,
            synchronous_control_cycle: None,
        });
        assert_eq!(
            m.pending_sync_signaling.len(),
            1,
            "only ACAck should keep a synchronous fallback copy"
        );

        let capsule = m
            .build_capsule_for_cycle(1, false)
            .expect("queued signaling capsule");

        assert_eq!(capsule.messages.len(), 1);
        assert_eq!(capsule.messages[0], vec![0x00]);
        assert_eq!(m.pending_signaling.len(), 1);
        assert_eq!(
            m.deferred_signaling.len(),
            TRAFFIC_ASSIGNMENT_REPEAT_OFFSETS_SLOTS.len(),
            "TrafficChannelAssignment should keep bounded best-effort repeat copies"
        );
    }

    #[test]
    fn traffic_channel_assignment_repeats_use_later_async_capsules() {
        let mut m = HrpdForwardSlotModulator::new(0, 32_768);
        m.set_overhead_quick_config(None);
        m.set_overhead_sector_params(None);
        m.set_overhead_access_params(None);
        m.set_overhead_reverse_rate(None);
        m.set_overhead_sync(None);
        let ati = cdma_common::hrpd::air::AccessTerminalIdentifier {
            ati_type: cdma_common::hrpd::air::AccessTerminalIdentifierType::Uati,
            value: 0x1a05_8001,
        };
        let payload = vec![
            0x01, 0x00, 0x80, 0x01, 0x3b, 0x03, 0xd7, 0x42, 0x00, 0x0a, 0x00,
        ];
        m.enqueue_forward_signaling(HrpdForwardSignalingRequest {
            uati: Some(0x8005_8001),
            target_ati: ati,
            protocol_type: cdma_common::hrpd::air::DEFAULT_ROUTE_UPDATE_PROTOCOL_TYPE,
            payload: payload.clone(),
            channel: cdma_common::hrpd::air::HrpdForwardChannel::AsynchronousControl,
            reliable_sequence: None,
            synchronous_control_cycle: None,
        });

        assert_eq!(m.pending_signaling.len(), 1);
        assert_eq!(
            m.deferred_signaling.len(),
            TRAFFIC_ASSIGNMENT_REPEAT_OFFSETS_SLOTS.len()
        );

        let _ = block(&mut m, 0, (SLOT_CHIPS * 8) as usize);
        assert_eq!(m.pending_signaling.len(), 0);
        assert_eq!(
            m.deferred_signaling.len(),
            TRAFFIC_ASSIGNMENT_REPEAT_OFFSETS_SLOTS.len()
        );

        let first_repeat_slot = TRAFFIC_ASSIGNMENT_REPEAT_OFFSETS_SLOTS[0];
        let _ = block(
            &mut m,
            SLOT_CHIPS * 8,
            (SLOT_CHIPS * (first_repeat_slot - 8 + 8)) as usize,
        );
        assert_eq!(m.pending_signaling.len(), 0);
        assert_eq!(
            m.deferred_signaling.len(),
            TRAFFIC_ASSIGNMENT_REPEAT_OFFSETS_SLOTS.len() - 1
        );
    }

    #[test]
    fn active_mac_updates_quick_config_and_reverse_rate_span() {
        let mut m = HrpdForwardSlotModulator::new(0, 32_768);
        m.set_active_macs(vec![crate::bts::hrpd::mac_encoder::ActiveMac {
            mac_index: 5,
            rpc: true,
            rpc_alternating: false,
            drclock: true,
            frame_offset: 0,
            physical_layer_subtype: 0,
        }]);

        let qc = m.quick_config.as_ref().expect("quick config present");
        assert_eq!(qc.rpc_count, 59);
        assert_eq!(qc.forward_traffic_valid.len(), 59);
        assert!(qc.forward_traffic_valid[58]);
        assert!(qc.forward_traffic_valid[..58].iter().all(|v| !*v));

        let reverse_rate = m.reverse_rate.as_ref().expect("reverse rate present");
        assert_eq!(reverse_rate.rpc_count, 59);
        assert_eq!(reverse_rate.rate_limit, vec![5; 59]);
    }

    #[test]
    fn active_mac_overhead_loads_without_pending_signaling() {
        let mut m = HrpdForwardSlotModulator::new(0, 32_768);
        m.set_overhead_sync(None);
        m.set_overhead_sector_params(None);
        m.set_overhead_access_params(None);
        m.set_active_macs(vec![crate::bts::hrpd::mac_encoder::ActiveMac {
            mac_index: 5,
            rpc: true,
            rpc_alternating: false,
            drclock: true,
            frame_offset: 0,
            physical_layer_subtype: 0,
        }]);

        let cycle_start = SLOT_CHIPS * u64::from(CTRL_CH_CYCLE_SLOTS);
        let out = block(&mut m, cycle_start, SLOT_CHIPS as usize);
        let mut control_data_nonzero = 0;
        for (i, sample) in out.iter().enumerate() {
            let chip = cycle_start + i as u64;
            if matches!(channel_for_chip(chip), SlotChannel::Data) {
                let mag2 = sample.re * sample.re + sample.im * sample.im;
                if mag2 > 1e-6 {
                    control_data_nonzero += 1;
                }
            }
        }
        assert!(
            control_data_nonzero > 0,
            "active-MAC overhead should load a synchronous control capsule even without queued signaling"
        );
        assert!(
            !m.active_mac_overhead_pending,
            "active-MAC overhead should be consumed by synchronous QuickConfig"
        );
    }

    #[test]
    fn active_mac_public_data_rides_synchronous_capsule_with_pending_signaling() {
        let mut m = HrpdForwardSlotModulator::new(0, 32_768);
        m.set_active_macs(vec![crate::bts::hrpd::mac_encoder::ActiveMac {
            mac_index: 5,
            rpc: true,
            rpc_alternating: false,
            drclock: true,
            frame_offset: 0,
            physical_layer_subtype: 0,
        }]);
        m.enqueue_forward_signaling(HrpdForwardSignalingRequest {
            uati: Some(0x8005_8001),
            target_ati: cdma_common::hrpd::air::AccessTerminalIdentifier {
                ati_type: cdma_common::hrpd::air::AccessTerminalIdentifierType::Uati,
                value: 0x1a05_8001,
            },
            protocol_type: cdma_common::hrpd::air::DEFAULT_CONNECTED_STATE_PROTOCOL_TYPE,
            payload: vec![0x01, 0x00],
            channel: cdma_common::hrpd::air::HrpdForwardChannel::AsynchronousControl,
            reliable_sequence: None,
            synchronous_control_cycle: None,
        });

        let first = m
            .build_capsule_for_cycle(1, true)
            .expect("synchronous active-MAC public-data capsule");
        assert!(first.synchronous);
        assert_eq!(
            first.messages[0][0],
            cdma_common::hrpd::messages::QUICK_CONFIG_MESSAGE_ID
        );
        assert!(
            first.messages.iter().any(|body| body == &[0x01, 0x00]),
            "pending directed signaling may share the synchronous capsule after QuickConfig"
        );
        assert_eq!(m.pending_signaling.len(), 0);
    }

    #[test]
    fn queued_traffic_waits_for_active_mac_public_data() {
        const MAC_INDEX: u8 = 5;

        let mut m = HrpdForwardSlotModulator::new(0, 32_768);
        let bus = std::sync::Arc::new(crate::bts::hrpd::HarqBus::new());
        bus.set_current_drc_at_slot(MAC_INDEX, 0, 0x1);
        m.set_harq_bus(bus.clone());

        // Model a mid-cycle assignment before the next synchronous QuickConfig.
        m.current_slot = Some(0);
        m.last_cycle_loaded = Some(0);
        m.set_active_macs(vec![crate::bts::hrpd::mac_encoder::ActiveMac {
            mac_index: MAC_INDEX,
            rpc: true,
            rpc_alternating: false,
            drclock: true,
            frame_offset: 0,
            physical_layer_subtype: 0,
        }]);
        m.enqueue_traffic(ForwardTrafficPacket {
            mac_index: MAC_INDEX,
            physical_layer_subtype: 0,
            forward_traffic_mac_subtype: 0,
            high_priority: false,
            payload: vec![0u8; 1024],
        });

        let held = block(&mut m, SLOT_CHIPS, SLOT_CHIPS as usize);
        let held_data_nonzero = held
            .iter()
            .enumerate()
            .filter(|(i, sample)| {
                let chip = SLOT_CHIPS + *i as u64;
                matches!(channel_for_chip(chip), SlotChannel::Data)
                    && sample.re * sample.re + sample.im * sample.im > 1e-6
            })
            .count();
        assert_eq!(
            held_data_nonzero, 0,
            "queued traffic must not start before active-MAC QuickConfig is public"
        );

        let cycle_start_slot = u64::from(CTRL_CH_CYCLE_SLOTS);
        let cycle_start_chip = cycle_start_slot * SLOT_CHIPS;
        bus.set_current_drc_at_slot(MAC_INDEX, cycle_start_slot - 1, 0x1);
        let _ = block(&mut m, cycle_start_chip, SLOT_CHIPS as usize);
        assert!(
            !m.active_mac_overhead_pending,
            "active-MAC public data should load at the next synchronous QuickConfig"
        );

        let traffic_start_chip = cycle_start_chip + SLOT_CHIPS;
        let traffic = block(&mut m, traffic_start_chip, SLOT_CHIPS as usize);
        let traffic_data_nonzero = traffic
            .iter()
            .enumerate()
            .filter(|(i, sample)| {
                let chip = traffic_start_chip + *i as u64;
                matches!(channel_for_chip(chip), SlotChannel::Data)
                    && sample.re * sample.re + sample.im * sample.im > 1e-6
            })
            .count();
        assert!(
            traffic_data_nonzero > 1500,
            "queued traffic should start after active-MAC QuickConfig is public"
        );
    }

    #[test]
    fn scheduler_mode_traffic_slot_fills_data_region() {
        // Queue a low-rate packet so the scheduler emits Traffic on slot 0.
        // DRC 1 corresponds to 38.4 kbps / 1024-bit payload.
        let mut m = HrpdForwardSlotModulator::new(0, 32_768);
        // The scheduler selects the forward rate from the AT's governing DRC
        // (C.S0024-0 v4.0 §8.4.6.1.4.1.2), read from the H-ARQ bus. Record DRC 1
        // for this MAC so the queued packet is promoted instead of deferred.
        let bus = std::sync::Arc::new(crate::bts::hrpd::HarqBus::new());
        bus.set_current_drc_at_slot(5, 0, 0x1);
        m.set_harq_bus(bus);
        let payload = vec![0u8; 1024];
        m.enqueue_traffic(ForwardTrafficPacket {
            mac_index: 5,
            physical_layer_subtype: 0,
            forward_traffic_mac_subtype: 0,
            high_priority: false,
            payload,
        });
        // Slot 0 fires the QuickConfig overhead capsule (Control slot). Skip
        // ahead to slot 1, which is the first Traffic-eligible slot.
        let start = SLOT_CHIPS;
        let out = block(&mut m, start, SLOT_CHIPS as usize);
        let mut data_chips_nonzero = 0;
        let mut data_chips_total = 0;
        for (i, sample) in out.iter().enumerate() {
            let chip = start + i as u64;
            if matches!(channel_for_chip(chip), SlotChannel::Data) {
                data_chips_total += 1;
                let mag2 = sample.re * sample.re + sample.im * sample.im;
                if mag2 > 1e-6 {
                    data_chips_nonzero += 1;
                }
            }
        }
        assert_eq!(data_chips_total, 1600);
        assert!(
            data_chips_nonzero > 1500,
            "expected nearly all 1600 data chips to be non-zero, got {data_chips_nonzero}"
        );
    }
}
