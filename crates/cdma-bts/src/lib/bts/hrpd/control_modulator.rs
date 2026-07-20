//! HRPD Rev 0 Forward Control Channel modulator.
//!
//! C.S0024-0 v4.0 §9.3.1.3.1.1 / §9.3.1.3.2.4. Takes a synchronous Control
//! MAC capsule, wraps each overhead body in the Default Signaling protocol,
//! builds the 1024-bit low-rate Control payload with physical CRC/tail, then
//! emits the physical packet at the capsule's rate (38.4 kbps / 16-slot /
//! MACIndex 3 or 76.8 kbps / 8-slot / MACIndex 2): turbo rate 1/5, Control
//! scrambler, Forward Control interleaver, QPSK, Walsh-16 cover, and the
//! rate's preamble.
//!
//! When the buffer is exhausted (i.e. the capsule has been fully transmitted
//! across however many Control slots its encoded length spans), subsequent
//! `next_slot_chips()` calls return all-zero blocks until a new capsule is
//! loaded via `load_capsule`.

use num::complex::Complex32;

use crate::phy::hrpd::crc::physical_crc16;
use crate::phy::hrpd::scrambler::HrpdForwardScrambler;
use crate::phy::hrpd::turbo::HrpdTurboEncoder;

use cdma_common::hrpd::air::{
    AccessTerminalIdentifierType, encode_default_signaling_packet,
    encode_reliable_default_signaling_packet,
};
use cdma_common::hrpd::messages::{
    AccessParameters, BroadcastReverseRateLimit, DEFAULT_ACCESS_CHANNEL_MAC_PROTOCOL_TYPE,
    DEFAULT_INITIALIZATION_STATE_PROTOCOL_TYPE, DEFAULT_REVERSE_TRAFFIC_CHANNEL_MAC_PROTOCOL_TYPE,
    OVERHEAD_MESSAGES_PROTOCOL_TYPE, QuickConfig, SectorParameters, SyncMessage,
};

use super::control_channel::{ControlChannelCapsule, ControlChannelMessage};
use super::scheduler::DATA_CHIPS_PER_SLOT;

/// QPSK consumes 2 bits per chip.
const BITS_PER_QPSK_SYMBOL: usize = 2;
const DEFAULT_CONTROL_PAYLOAD_BITS: usize = 1024;
pub(crate) const DEFAULT_CONTROL_MAC_BITS: usize = 1002;
const DEFAULT_CONTROL_PREAMBLE_COVER_CHIPS: usize = 32;

/// Physical parameters for one legal Control Channel rate
/// (C.S0024-0 §9.3.1.3.2.1: 76.8 kbps uses MACIndex 2, 38.4 kbps uses
/// MACIndex 3; preamble lengths per Table 9.3.1.3.2.3.1-1).
struct ControlRateParams {
    slots: usize,
    preamble_chips: usize,
    mac_index: u8,
    rate_code: u8,
}

const CONTROL_RATE_76_8: ControlRateParams = ControlRateParams {
    slots: 8,
    preamble_chips: 512,
    mac_index: 2,
    rate_code: 0b0010,
};

const CONTROL_RATE_38_4: ControlRateParams = ControlRateParams {
    slots: 16,
    preamble_chips: 1024,
    mac_index: 3,
    rate_code: 0b0001,
};

fn control_rate_params(kbps: u32) -> Option<&'static ControlRateParams> {
    match kbps {
        76_800 => Some(&CONTROL_RATE_76_8),
        38_400 => Some(&CONTROL_RATE_38_4),
        _ => None,
    }
}

#[derive(Debug, Clone)]
pub struct ControlChannelModulator {
    /// Pre-modulated chip buffer.
    chips: Vec<Complex32>,
    /// Position into `chips` for the next `next_slot_chips` call.
    cursor: usize,
}

impl Default for ControlChannelModulator {
    fn default() -> Self {
        Self::new()
    }
}

impl ControlChannelModulator {
    pub fn new() -> Self {
        Self {
            chips: Vec::new(),
            cursor: 0,
        }
    }

    /// Encode the capsule into a Control packet at the capsule's rate
    /// (76.8 kbps / 8 slots or 38.4 kbps / 16 slots); replaces any pending
    /// state. Returns `false` when the rate is not a legal Control Channel
    /// rate or the capsule does not fit in the 1002-bit synchronous Control
    /// MAC payload.
    pub fn load_capsule(&mut self, capsule: &ControlChannelCapsule) -> bool {
        let Some(rate) = control_rate_params(capsule.kbps) else {
            return false;
        };
        let Some(payload) = control_mac_physical_payload(capsule) else {
            return false;
        };
        let encoder = match HrpdTurboEncoder::new(DEFAULT_CONTROL_PAYLOAD_BITS as u32) {
            Some(e) => e,
            None => return false,
        };
        let mut scrambled = encoder.encode(&payload, 1, 5);
        let mut scrambler = HrpdForwardScrambler::with_initial_state(
            control_scrambler_initial_state(rate.mac_index, rate.rate_code),
        );
        scrambler.apply_bits(&mut scrambled);
        let interleaved = forward_rate_1_5_interleave_1024(&scrambled);

        let data_chip_count = rate.slots * DATA_CHIPS_PER_SLOT - rate.preamble_chips;
        let symbols = repeat_symbols(&map_qpsk_bits(&interleaved), data_chip_count);
        let data_chips = walsh16_cover_symbols(&symbols);
        let mut tdm = control_preamble_chips(rate.mac_index, rate.preamble_chips);
        tdm.extend_from_slice(&data_chips);
        debug_assert_eq!(tdm.len(), rate.slots * DATA_CHIPS_PER_SLOT);
        self.chips = tdm;
        self.cursor = 0;
        true
    }

    /// Return the next slot's worth of Data chips. Always 1600 chips long;
    /// padded with zero chips when the encoded buffer is exhausted.
    pub fn next_slot_chips(&mut self) -> Vec<Complex32> {
        let mut out = Vec::with_capacity(DATA_CHIPS_PER_SLOT);
        for _ in 0..DATA_CHIPS_PER_SLOT {
            if self.cursor < self.chips.len() {
                out.push(self.chips[self.cursor]);
                self.cursor += 1;
            } else {
                out.push(Complex32::new(0.0, 0.0));
            }
        }
        out
    }

    /// Remaining chips before the buffer is exhausted (returns zero once
    /// fully drained).
    pub fn remaining(&self) -> usize {
        self.chips.len().saturating_sub(self.cursor)
    }
}

fn control_mac_physical_payload(capsule: &ControlChannelCapsule) -> Option<Vec<u8>> {
    let mut mac = Vec::with_capacity(DEFAULT_CONTROL_MAC_BITS);
    push_bits_value(&mut mac, capsule.synchronous as u64, 1); // SynchronousCapsule.
    push_bits_value(&mut mac, 1, 1); // FirstPacket.
    push_bits_value(&mut mac, 1, 1); // LastPacket.
    push_bits_value(&mut mac, 0, 2); // Offset.
    push_bits_value(&mut mac, 1, 1); // SleepStateCapsuleDone.
    push_bits_value(&mut mac, 0, 2); // Reserved.

    for message in capsule.control_messages() {
        let (ati_type, ati, signaling) = control_mac_packet_parts(message)?;
        let ati_octets = if ati_type == 0 { 0usize } else { 4usize };
        let length_octets = signaling.len().checked_add(1)?.checked_add(ati_octets)?;
        if length_octets > u8::MAX as usize {
            return None;
        }
        push_bits_u8(&mut mac, length_octets as u8);
        push_bits_value(&mut mac, 0, 1); // SecurityLayerFormat=false.
        push_bits_value(&mut mac, 0, 1); // ConnectionLayerFormat=false.
        push_bits_value(&mut mac, 0, 4); // MAC header reserved.
        push_bits_value(&mut mac, u64::from(ati_type), 2);
        if ati_type != 0 {
            push_bits_u32(&mut mac, ati);
        }
        mac.extend(bytes_to_bits(&signaling));
        if mac.len() > DEFAULT_CONTROL_MAC_BITS {
            return None;
        }
    }

    mac.resize(DEFAULT_CONTROL_MAC_BITS, 0);
    let mut payload = mac;
    let crc = physical_crc16(&payload);
    push_bits_u16(&mut payload, crc);
    payload.resize(DEFAULT_CONTROL_PAYLOAD_BITS, 0);
    Some(payload)
}

fn control_mac_packet_parts(message: &ControlChannelMessage) -> Option<(u8, u32, Vec<u8>)> {
    match message {
        ControlChannelMessage::InferredOverhead(body) => {
            Some((0, 0, default_signaling_packet(body)?))
        }
        ControlChannelMessage::DefaultSignaling(message) => Some((
            ati_type_bits(message.ati.ati_type),
            message.ati.value,
            if let Some(sequence) = message.reliable_sequence {
                encode_reliable_default_signaling_packet(
                    message.protocol_type,
                    &message.payload,
                    sequence,
                )
            } else {
                // SNP InConfigurationProtocol stays 0: every message sent here
                // (including UATIAssignment, C.S0024-0 §5.3.7) is defined for
                // the InUse protocol instance. The bit is only ever 1 for
                // ConfigurationRequest/Response during config negotiation.
                encode_default_signaling_packet(message.protocol_type, &message.payload)
            },
        )),
    }
}

fn default_signaling_packet(body: &[u8]) -> Option<Vec<u8>> {
    let protocol_type = overhead_protocol_type(body)?;
    Some(encode_default_signaling_packet(protocol_type, body))
}

fn ati_type_bits(ati_type: AccessTerminalIdentifierType) -> u8 {
    match ati_type {
        AccessTerminalIdentifierType::Bati => 0b00,
        AccessTerminalIdentifierType::Reserved => 0b01,
        AccessTerminalIdentifierType::Uati => 0b10,
        AccessTerminalIdentifierType::Rati => 0b11,
    }
}

fn overhead_protocol_type(body: &[u8]) -> Option<u8> {
    let sync_len = SyncMessage::defaults().encode().len();
    if body.len() == sync_len && SyncMessage::decode(body).is_some() {
        return Some(DEFAULT_INITIALIZATION_STATE_PROTOCOL_TYPE);
    }
    if QuickConfig::decode(body).is_some() || SectorParameters::decode(body).is_some() {
        return Some(OVERHEAD_MESSAGES_PROTOCOL_TYPE);
    }
    if AccessParameters::decode(body).is_some() {
        return Some(DEFAULT_ACCESS_CHANNEL_MAC_PROTOCOL_TYPE);
    }
    if BroadcastReverseRateLimit::decode(body).is_some() {
        return Some(DEFAULT_REVERSE_TRAFFIC_CHANNEL_MAC_PROTOCOL_TYPE);
    }
    None
}

fn control_scrambler_initial_state(mac_index: u8, rate_code: u8) -> u32 {
    let leading = 0x7fu32 << 10;
    let r = (u32::from(mac_index) & 0x3f) << 4;
    let d = u32::from(rate_code) & 0x0f;
    leading | r | d
}

fn control_preamble_chips(mac_index: u8, preamble_chips: usize) -> Vec<Complex32> {
    let row = usize::from(mac_index >> 1);
    let complement = (mac_index & 1) != 0;
    (0..preamble_chips)
        .map(|idx| {
            let mut sign = walsh_biorthogonal(row, idx % DEFAULT_CONTROL_PREAMBLE_COVER_CHIPS);
            if complement {
                sign = -sign;
            }
            Complex32::new(sign, 0.0)
        })
        .collect()
}

fn map_qpsk_bits(bits: &[u8]) -> Vec<Complex32> {
    let scale = 1.0_f32 / 2.0_f32.sqrt();
    bits.chunks_exact(BITS_PER_QPSK_SYMBOL)
        .map(|pair| {
            let i = if pair[0] == 0 { scale } else { -scale };
            let q = if pair[1] == 0 { scale } else { -scale };
            Complex32::new(i, q)
        })
        .collect()
}

fn repeat_symbols(symbols: &[Complex32], len: usize) -> Vec<Complex32> {
    (0..len).map(|idx| symbols[idx % symbols.len()]).collect()
}

fn walsh16_cover_symbols(symbols: &[Complex32]) -> Vec<Complex32> {
    let mut out = Vec::with_capacity(symbols.len());
    for group in symbols.chunks_exact(16) {
        for col in 0..16 {
            let mut chip = Complex32::new(0.0, 0.0);
            for (row, symbol) in group.iter().enumerate() {
                chip += *symbol * walsh_biorthogonal(row, col) * 0.25;
            }
            out.push(chip);
        }
    }
    out
}

fn walsh_biorthogonal(row: usize, col: usize) -> f32 {
    if ((row & col).count_ones() & 1) == 0 {
        1.0
    } else {
        -1.0
    }
}

fn forward_rate_1_5_interleave_1024(input: &[u8]) -> Vec<u8> {
    forward_rate_1_5_interleave(DEFAULT_CONTROL_PAYLOAD_BITS, input)
}

fn forward_rate_1_5_interleave(payload_bits: usize, input: &[u8]) -> Vec<u8> {
    assert_eq!(input.len(), payload_bits * 5);
    let mut u = vec![0u8; payload_bits];
    let mut v0_vp0 = vec![0u8; payload_bits * 2];
    let mut v1_vp1 = vec![0u8; payload_bits * 2];
    for k in 0..payload_bits {
        u[k] = input[k * 5];
        v0_vp0[k] = input[k * 5 + 1];
        v0_vp0[payload_bits + k] = input[k * 5 + 3];
        v1_vp1[k] = input[k * 5 + 2];
        v1_vp1[payload_bits + k] = input[k * 5 + 4];
    }

    let u = forward_symbol_permute(&u, 2, payload_bits / 2, ForwardInterleaverBlock::U);
    let v0_vp0 = forward_symbol_permute(&v0_vp0, 2, payload_bits, ForwardInterleaverBlock::V);
    let v1_vp1 = forward_symbol_permute(&v1_vp1, 2, payload_bits, ForwardInterleaverBlock::V);
    [u, v0_vp0, v1_vp1].concat()
}

#[derive(Debug, Clone, Copy)]
enum ForwardInterleaverBlock {
    U,
    V,
}

fn forward_symbol_permute(
    input: &[u8],
    k_rows: usize,
    m_cols: usize,
    block: ForwardInterleaverBlock,
) -> Vec<u8> {
    debug_assert_eq!(input.len(), k_rows * m_cols);
    let mut out = vec![0u8; input.len()];
    let bits = m_cols.ilog2();
    for j in 0..m_cols {
        let final_col = bit_reverse(j as u32, bits) as usize;
        let shift = match block {
            ForwardInterleaverBlock::U => j % k_rows,
            ForwardInterleaverBlock::V => (j / 4) % k_rows,
        };
        for final_row in 0..k_rows {
            let input_row = (final_row + k_rows - shift) % k_rows;
            let input_idx = input_row * m_cols + j;
            let output_idx = final_col * k_rows + final_row;
            out[output_idx] = input[input_idx];
        }
    }
    out
}

fn bit_reverse(mut value: u32, bits: u32) -> u32 {
    let mut out = 0;
    for _ in 0..bits {
        out = (out << 1) | (value & 1);
        value >>= 1;
    }
    out
}

fn push_bits_value(bits: &mut Vec<u8>, value: u64, width: usize) {
    for shift in (0..width).rev() {
        bits.push(((value >> shift) & 1) as u8);
    }
}

fn push_bits_u8(bits: &mut Vec<u8>, value: u8) {
    push_bits_value(bits, u64::from(value), 8);
}

fn push_bits_u16(bits: &mut Vec<u8>, value: u16) {
    push_bits_value(bits, u64::from(value), 16);
}

fn push_bits_u32(bits: &mut Vec<u8>, value: u32) {
    push_bits_value(bits, u64::from(value), 32);
}

fn bytes_to_bits(bytes: &[u8]) -> Vec<u8> {
    let mut bits = Vec::with_capacity(bytes.len() * 8);
    for &byte in bytes {
        for shift in (0..8).rev() {
            bits.push((byte >> shift) & 1);
        }
    }
    bits
}

#[cfg(test)]
mod tests {
    use super::*;
    use cdma_common::hrpd::air::{AccessTerminalIdentifier, AccessTerminalIdentifierType};
    use cdma_common::hrpd::messages::SyncMessage;

    fn tiny_capsule() -> ControlChannelCapsule {
        ControlChannelCapsule::new(
            vec![SyncMessage::defaults().encode()],
            super::super::control_channel::CTRL_CH_DEFAULT_KBPS,
        )
    }

    fn read_bits(bits: &[u8], cursor: &mut usize, width: usize) -> u64 {
        let mut out = 0u64;
        for _ in 0..width {
            out = (out << 1) | u64::from(bits[*cursor] & 1);
            *cursor += 1;
        }
        out
    }

    #[test]
    fn empty_modulator_emits_zero_chips() {
        let mut m = ControlChannelModulator::new();
        let chips = m.next_slot_chips();
        assert_eq!(chips.len(), DATA_CHIPS_PER_SLOT);
        assert!(chips.iter().all(|c| c.re == 0.0 && c.im == 0.0));
    }

    #[test]
    fn load_capsule_succeeds_for_tiny_message() {
        let mut m = ControlChannelModulator::new();
        assert!(m.load_capsule(&tiny_capsule()));
        assert!(m.remaining() > 0);
    }

    #[test]
    fn explicit_default_signaling_carries_control_mac_ati() {
        let capsule = ControlChannelCapsule::new_default_signaling(
            vec![
                super::super::control_channel::ControlChannelDefaultSignalingMessage {
                    ati: AccessTerminalIdentifier {
                        ati_type: AccessTerminalIdentifierType::Rati,
                        value: 0x50ad_b764,
                    },
                    protocol_type: 0x11,
                    payload: vec![0x01, 0x07],
                    reliable_sequence: None,
                    synchronous_control_cycle: None,
                },
            ],
            super::super::control_channel::CTRL_CH_DEFAULT_KBPS,
        );
        let payload = control_mac_physical_payload(&capsule).expect("capsule should fit");
        let mut cursor = 0usize;
        assert_eq!(read_bits(&payload, &mut cursor, 1), 1); // synchronous capsule
        assert_eq!(read_bits(&payload, &mut cursor, 1), 1); // first
        assert_eq!(read_bits(&payload, &mut cursor, 1), 1); // last
        assert_eq!(read_bits(&payload, &mut cursor, 2), 0); // offset
        assert_eq!(read_bits(&payload, &mut cursor, 1), 1); // sleep done
        assert_eq!(read_bits(&payload, &mut cursor, 2), 0); // reserved
        assert_eq!(read_bits(&payload, &mut cursor, 8), 9); // MAC hdr + ATI + SLP/SNP/payload
        assert_eq!(read_bits(&payload, &mut cursor, 1), 0); // security
        assert_eq!(read_bits(&payload, &mut cursor, 1), 0); // connection
        assert_eq!(read_bits(&payload, &mut cursor, 4), 0); // reserved
        assert_eq!(read_bits(&payload, &mut cursor, 2), 0b11); // RATI
        assert_eq!(read_bits(&payload, &mut cursor, 32), 0x50ad_b764);
        assert_eq!(read_bits(&payload, &mut cursor, 8), 0x00);
        assert_eq!(read_bits(&payload, &mut cursor, 8), 0x11); // InUse instance + Address Mgmt.
        assert_eq!(read_bits(&payload, &mut cursor, 8), 0x01);
        assert_eq!(read_bits(&payload, &mut cursor, 8), 0x07);
    }

    #[test]
    fn async_default_signaling_clears_synchronous_capsule_header_bit() {
        let capsule = ControlChannelCapsule::new_asynchronous_default_signaling(
            vec![
                super::super::control_channel::ControlChannelDefaultSignalingMessage {
                    ati: AccessTerminalIdentifier {
                        ati_type: AccessTerminalIdentifierType::Rati,
                        value: 0x50ad_b764,
                    },
                    protocol_type: 0x11,
                    payload: vec![0x01, 0x07],
                    reliable_sequence: None,
                    synchronous_control_cycle: None,
                },
            ],
            super::super::control_channel::CTRL_CH_DEFAULT_KBPS,
        );
        let payload = control_mac_physical_payload(&capsule).expect("capsule should fit");
        let mut cursor = 0usize;
        assert_eq!(read_bits(&payload, &mut cursor, 1), 0); // asynchronous capsule
        assert_eq!(read_bits(&payload, &mut cursor, 1), 1); // first
        assert_eq!(read_bits(&payload, &mut cursor, 1), 1); // last
        assert_eq!(read_bits(&payload, &mut cursor, 2), 0); // offset
        assert_eq!(read_bits(&payload, &mut cursor, 1), 1); // sleep done
        assert_eq!(read_bits(&payload, &mut cursor, 2), 0); // reserved
    }

    #[test]
    fn next_slot_returns_1600_chips() {
        let mut m = ControlChannelModulator::new();
        m.load_capsule(&tiny_capsule());
        let chips = m.next_slot_chips();
        assert_eq!(chips.len(), DATA_CHIPS_PER_SLOT);
        // Some non-zero entries from QPSK output.
        assert!(chips.iter().any(|c| c.re != 0.0));
    }

    #[test]
    fn cursor_advances_across_consecutive_slots() {
        let mut m = ControlChannelModulator::new();
        m.load_capsule(&tiny_capsule());
        let initial = m.remaining();
        let _ = m.next_slot_chips();
        let after = m.remaining();
        assert_eq!(initial - after, DATA_CHIPS_PER_SLOT.min(initial));
    }

    #[test]
    fn buffer_exhausts_eventually_and_pads_zeros() {
        let mut m = ControlChannelModulator::new();
        m.load_capsule(&tiny_capsule());
        // Drain.
        while m.remaining() > 0 {
            let _ = m.next_slot_chips();
        }
        // Next call must still return 1600 chips, all zero.
        let chips = m.next_slot_chips();
        assert_eq!(chips.len(), DATA_CHIPS_PER_SLOT);
        assert!(chips.iter().all(|c| c.re == 0.0 && c.im == 0.0));
    }
}
