//! HRPD (1xEV-DO Rev 0) Forward Control Channel capsule framing.
//!
//! Capsule layout used here (bit order, MSB first per C.S0024-0 §9.4):
//!
//! ```text
//!   For each message in the capsule:
//!     MessageLength (8 bits, big-endian)  -- length of body in octets
//!     MessageBody   (8 * MessageLength bits)
//!   Concatenation terminator:
//!     MessageLength = 0x00 (8 bits)       -- indicates end of capsule
//!   CRC  (16 bits, MSB first)             -- computed over all bits above
//!   Tail (6 zero bits)                    -- turbo-encoder termination
//! ```
//!
//! `MessageLength = 0` is the spec's "no more messages" sentinel. Each
//! `MessageBody` is an already bit-packed HRPD message body.

use cdma_common::hrpd::air::{
    AccessTerminalIdentifier, AccessTerminalIdentifierType, HrpdSynchronousControlCycle,
};

/// Slots per HRPD Control Channel cycle. C.S0024-300 §9 (Control Channel MAC).
/// 256 slots * 1.667 ms/slot = 426.67 ms.
pub const CTRL_CH_CYCLE_SLOTS: u32 = 256;

/// Forward Control Channel data rate used on air. C.S0024-0 §9.3.1.3.2.4.
///
/// 38.4 kbps (16-slot, MACIndex 3, complemented preamble cover) and 76.8 kbps
/// (8-slot, MACIndex 2) are the two legal rates. The rate is not signaled: the
/// access terminal blind-detects it from the preamble cover, with no
/// negotiation or fallback. 38.4 kbps spreads the same 1024-bit packet over
/// twice the slots and preamble energy, giving the larger acquisition margin,
/// so it is the coverage-safe default; 76.8 kbps trades that margin for
/// control-channel airtime.
pub const CTRL_CH_DEFAULT_KBPS: u32 = 38_400;

/// Forward Control Channel rate used on air.
pub fn ctrl_ch_kbps() -> u32 {
    CTRL_CH_DEFAULT_KBPS
}

/// Number of tail (turbo termination) zero bits appended after the CRC on a
/// Forward Control Channel physical-layer packet. C.S0024-0 v4.0 §9.4.7.3.
const CTRL_CH_TAIL_BITS: usize = 6;

/// HRPD Forward Control Channel CRC-16.
/// g(x) = x^16 + x^12 + x^11 + x^10 + x^8 + x^6 + x^5 + x^2 + 1, poly `0x9D6F`,
/// init `0xFFFF`, no final XOR. C.S0024-0 v4.0 §9.4.7.3.
const CTRL_CH_CRC_POLY: u16 = 0x9D6F;
const CTRL_CH_CRC_INIT: u16 = 0xFFFF;

/// A capsule queued by the Control Channel scheduler. Each entry in
/// `messages` is an already bit-packed body produced upstream (the
/// `OverheadMessage::encode` source); this type only handles capsule-level
/// framing (length octets, terminator, CRC, tail). C.S0024-0 §9.4.5.
#[derive(Debug, Clone)]
pub struct ControlChannelCapsule {
    /// Concatenated message bodies, in transmit order.
    ///
    /// Raw bodies feed the overhead-only path (tests and diagnostics). The
    /// Control MAC encoder instead uses `control_messages` so explicitly-typed
    /// Address Management / Route Update packets carry their protocol and ATI
    /// header.
    pub messages: Vec<Vec<u8>>,
    /// Physical-layer rate for this capsule (typically `CTRL_CH_DEFAULT_KBPS`).
    pub kbps: u32,
    pub synchronous: bool,
    control_messages: Vec<ControlChannelMessage>,
}

impl ControlChannelCapsule {
    /// Build a capsule from a list of already-encoded message bodies.
    pub fn new(messages: Vec<Vec<u8>>, kbps: u32) -> Self {
        let control_messages = messages
            .iter()
            .cloned()
            .map(ControlChannelMessage::InferredOverhead)
            .collect();
        Self {
            messages,
            kbps,
            synchronous: true,
            control_messages,
        }
    }

    /// Build a capsule from Default Signaling messages whose protocol and
    /// Control MAC ATI are already known by the caller.
    pub fn new_default_signaling(
        messages: Vec<ControlChannelDefaultSignalingMessage>,
        kbps: u32,
    ) -> Self {
        let bodies = messages
            .iter()
            .map(|message| message.payload.clone())
            .collect();
        let control_messages = messages
            .into_iter()
            .map(ControlChannelMessage::DefaultSignaling)
            .collect();
        Self {
            messages: bodies,
            kbps,
            synchronous: true,
            control_messages,
        }
    }

    /// Build a synchronous capsule that carries normal overhead bodies and
    /// one or more typed Default Signaling packets. C.S0024 allows packets
    /// marked for asynchronous capsules to be transmitted in a synchronous
    /// capsule; keeping them typed here preserves the Control MAC ATI header.
    pub fn new_with_default_signaling(
        overhead_messages: Vec<Vec<u8>>,
        signaling_messages: Vec<ControlChannelDefaultSignalingMessage>,
        kbps: u32,
    ) -> Self {
        let mut messages = overhead_messages.clone();
        messages.extend(
            signaling_messages
                .iter()
                .map(|message| message.payload.clone()),
        );

        let mut control_messages = overhead_messages
            .into_iter()
            .map(ControlChannelMessage::InferredOverhead)
            .collect::<Vec<_>>();
        control_messages.extend(
            signaling_messages
                .into_iter()
                .map(ControlChannelMessage::DefaultSignaling),
        );

        Self {
            messages,
            kbps,
            synchronous: true,
            control_messages,
        }
    }

    pub fn new_asynchronous_default_signaling(
        messages: Vec<ControlChannelDefaultSignalingMessage>,
        kbps: u32,
    ) -> Self {
        let mut capsule = Self::new_default_signaling(messages, kbps);
        capsule.synchronous = false;
        capsule
    }

    pub(crate) fn control_messages(&self) -> &[ControlChannelMessage] {
        &self.control_messages
    }

    /// Frame the capsule into a stream of bits (one bit per output byte,
    /// values `0` or `1`, MSB-first within each source octet). The output
    /// includes:
    ///
    /// - For each message: `Length(8 bits, BE) || Body`.
    /// - Terminator: `Length = 0` (8 bits).
    /// - CRC-16 (16 bits, MSB first) over all bits above.
    /// - 6 tail zero bits.
    ///
    /// This is what feeds the Forward Control Channel turbo encoder /
    /// repetition / scrambler downstream.
    pub fn frame(&self) -> Vec<u8> {
        let mut bits: Vec<u8> = Vec::new();

        for body in &self.messages {
            let len = body.len();
            // MessageLength == 0 is the capsule terminator; refuse to encode a
            // zero-length body here so it cannot be confused with end-of-capsule.
            debug_assert!(len > 0, "capsule message body must be non-empty");
            debug_assert!(len <= 0xFF, "capsule message body exceeds 255 octets");
            push_u8(&mut bits, len as u8);
            push_bytes_msb(&mut bits, body);
        }

        // End-of-capsule terminator (zero-length message).
        push_u8(&mut bits, 0);

        // CRC over everything emitted so far.
        let crc = ctrl_ch_crc16(&bits);
        push_u16(&mut bits, crc);

        // Tail bits (six zeros).
        for _ in 0..CTRL_CH_TAIL_BITS {
            bits.push(0);
        }

        bits
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControlChannelDefaultSignalingMessage {
    pub ati: AccessTerminalIdentifier,
    pub protocol_type: u8,
    pub payload: Vec<u8>,
    pub reliable_sequence: Option<u8>,
    pub synchronous_control_cycle: Option<HrpdSynchronousControlCycle>,
}

impl ControlChannelDefaultSignalingMessage {
    pub fn broadcast(protocol_type: u8, payload: Vec<u8>) -> Self {
        Self {
            ati: AccessTerminalIdentifier {
                ati_type: AccessTerminalIdentifierType::Bati,
                value: 0,
            },
            protocol_type,
            payload,
            reliable_sequence: None,
            synchronous_control_cycle: None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum ControlChannelMessage {
    InferredOverhead(Vec<u8>),
    DefaultSignaling(ControlChannelDefaultSignalingMessage),
}

/// HRPD Forward Control Channel CRC-16 (see module docs / `CTRL_CH_CRC_POLY`).
/// Input `bits` is one bit per byte (0/1), MSB first within the original
/// stream.
pub fn ctrl_ch_crc16(bits: &[u8]) -> u16 {
    let mut reg: u16 = CTRL_CH_CRC_INIT;
    for &b in bits {
        let feedback = ((reg >> 15) & 1) as u8 ^ (b & 1);
        reg <<= 1;
        if feedback == 1 {
            reg ^= CTRL_CH_CRC_POLY;
        }
    }
    reg
}

fn push_u8(bits: &mut Vec<u8>, v: u8) {
    for i in (0..8).rev() {
        bits.push((v >> i) & 1);
    }
}

fn push_u16(bits: &mut Vec<u8>, v: u16) {
    for i in (0..16).rev() {
        bits.push(((v >> i) & 1) as u8);
    }
}

fn push_bytes_msb(bits: &mut Vec<u8>, bytes: &[u8]) {
    for &byte in bytes {
        push_u8(bits, byte);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Mirror of `ctrl_ch_crc16` so the test independently re-derives the
    /// expected CRC rather than re-using the production function's output.
    fn crc_reference(bits: &[u8]) -> u16 {
        let mut reg: u16 = 0xFFFF;
        for &b in bits {
            let top = (reg & 0x8000) != 0;
            let input_bit = (b & 1) != 0;
            reg <<= 1;
            if top ^ input_bit {
                reg ^= 0x9D6F;
            }
        }
        reg
    }

    fn bits_msb(bytes: &[u8]) -> Vec<u8> {
        let mut v = Vec::with_capacity(bytes.len() * 8);
        for &byte in bytes {
            for i in (0..8).rev() {
                v.push((byte >> i) & 1);
            }
        }
        v
    }

    /// Tiny capsule parser used to round-trip the framing.
    fn parse_capsule(bits: &[u8]) -> (Vec<Vec<u8>>, u16) {
        assert!(bits.len() >= 8 + 16 + CTRL_CH_TAIL_BITS);
        // Strip tail.
        let payload = &bits[..bits.len() - CTRL_CH_TAIL_BITS];
        let crc_offset = payload.len() - 16;
        let body_bits = &payload[..crc_offset];
        let crc_bits = &payload[crc_offset..];

        let mut crc: u16 = 0;
        for &b in crc_bits {
            crc = (crc << 1) | (b as u16 & 1);
        }

        let mut messages = Vec::new();
        let mut cur = 0usize;
        loop {
            assert!(cur + 8 <= body_bits.len(), "truncated length field");
            let mut len: u8 = 0;
            for i in 0..8 {
                len = (len << 1) | (body_bits[cur + i] & 1);
            }
            cur += 8;
            if len == 0 {
                break;
            }
            let body_bit_count = len as usize * 8;
            assert!(cur + body_bit_count <= body_bits.len(), "truncated body");
            let mut body = vec![0u8; len as usize];
            for byte_idx in 0..len as usize {
                let mut v: u8 = 0;
                for i in 0..8 {
                    v = (v << 1) | (body_bits[cur + byte_idx * 8 + i] & 1);
                }
                body[byte_idx] = v;
            }
            cur += body_bit_count;
            messages.push(body);
        }
        assert_eq!(cur, body_bits.len(), "trailing bits before CRC");
        (messages, crc)
    }

    #[test]
    fn constants_match_spec() {
        assert_eq!(CTRL_CH_CYCLE_SLOTS, 256);
        assert_eq!(CTRL_CH_DEFAULT_KBPS, 38_400);
        assert_eq!(CTRL_CH_TAIL_BITS, 6);
        assert_eq!(CTRL_CH_CRC_POLY, 0x9D6F);
    }

    #[test]
    fn crc_against_reference() {
        let cases: &[&[u8]] = &[
            &[],
            &[0x00, 0x00, 0x00, 0x00],
            &[0xFF],
            &[0xDE, 0xAD, 0xBE, 0xEF],
            &[0x12, 0x34, 0x56, 0x78, 0x9A],
        ];
        for case in cases {
            let bits = bits_msb(case);
            assert_eq!(ctrl_ch_crc16(&bits), crc_reference(&bits));
        }
    }

    #[test]
    fn short_capsule_round_trips_length_and_crc() {
        let body = vec![0xA5, 0x5A, 0x12];
        let cap = ControlChannelCapsule::new(vec![body.clone()], CTRL_CH_DEFAULT_KBPS);
        let bits = cap.frame();

        // Total bits = 8 (len) + 24 (body) + 8 (terminator) + 16 (crc) + 6 (tail) = 62.
        assert_eq!(bits.len(), 8 + 24 + 8 + 16 + CTRL_CH_TAIL_BITS);

        let (msgs, crc) = parse_capsule(&bits);
        assert_eq!(msgs, vec![body.clone()]);

        // Recompute the expected CRC from the parsed length+body+terminator.
        let mut pre_crc = Vec::new();
        pre_crc.extend_from_slice(&[body.len() as u8]);
        pre_crc.extend_from_slice(&body);
        pre_crc.push(0); // terminator
        assert_eq!(crc, crc_reference(&bits_msb(&pre_crc)));
    }

    #[test]
    fn multi_message_capsule_round_trips() {
        let m1 = vec![0x01, 0x02, 0x03];
        let m2 = vec![0xAA; 7];
        let m3 = vec![0xFF];
        let cap = ControlChannelCapsule::new(
            vec![m1.clone(), m2.clone(), m3.clone()],
            CTRL_CH_DEFAULT_KBPS,
        );
        let (msgs, _crc) = parse_capsule(&cap.frame());
        assert_eq!(msgs, vec![m1, m2, m3]);
    }

    #[test]
    fn tail_is_six_zero_bits() {
        let cap = ControlChannelCapsule::new(vec![vec![0x42]], CTRL_CH_DEFAULT_KBPS);
        let bits = cap.frame();
        let tail = &bits[bits.len() - CTRL_CH_TAIL_BITS..];
        assert!(tail.iter().all(|&b| b == 0));
    }
}
