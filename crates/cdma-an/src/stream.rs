//! HRPD Stream Layer (C.S0024-500 §6.2 / C.S0024-400 §8).
//!
//! Rev 0 default Stream Protocol carries up to 4 streams in the Stream Layer header:
//! - Stream 0: Signaling (to/from the Connection Layer).
//! - Stream 1: Default Packet Application by default (Simple IP / PPP traffic).
//! - Stream 2/3: Reserved or app-specific.
//! Session negotiation can select packet applications on other stream protocols;
//! this type only encodes the two-bit stream header once the stream is chosen.
//!
//! The Default Stream Protocol prepends a 2-bit StreamID per SDU. We pack
//! the header as the top two bits of the first byte; the remaining 6 bits
//! plus the rest of the payload constitute the SDU. This matches
//! C.S0024-400 §8.2.4 for the Default Stream Protocol.
//!
//! Encapsulate: stream_id || sdu → security_pdu. Decapsulate inverts.
//! Composition with the Security Layer (`Self::send`/`Self::recv`) gives
//! the on-air bit stream once the lower layers are wired.

use crate::security::{SecurityError, SecurityLayer};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StreamId {
    Signaling = 0,
    DefaultPacket = 1,
    Reserved2 = 2,
    Reserved3 = 3,
}

impl StreamId {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v & 0b11 {
            0 => Some(Self::Signaling),
            1 => Some(Self::DefaultPacket),
            2 => Some(Self::Reserved2),
            3 => Some(Self::Reserved3),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamError {
    Empty,
    Security(SecurityError),
}

#[derive(Debug, Clone, Copy)]
pub struct StreamLayer {
    pub security: SecurityLayer,
}

impl StreamLayer {
    pub const fn rev0_default() -> Self {
        Self {
            security: SecurityLayer::rev0_default(),
        }
    }

    /// Wrap an SDU with its StreamID and pass through the Security Layer.
    pub fn send(&self, stream_id: StreamId, sdu: &[u8]) -> Vec<u8> {
        let mut framed = Vec::with_capacity(sdu.len() + 1);
        framed.push((stream_id as u8) << 6);
        framed.extend_from_slice(sdu);
        self.security.encapsulate(&framed)
    }

    /// Decapsulate a Security Layer PDU back to `(StreamID, SDU)`.
    pub fn recv(&self, security_pdu: &[u8]) -> Result<(StreamId, Vec<u8>), StreamError> {
        let framed = self
            .security
            .decapsulate(security_pdu)
            .map_err(StreamError::Security)?;
        if framed.is_empty() {
            return Err(StreamError::Empty);
        }
        let stream_id = StreamId::from_u8(framed[0] >> 6).ok_or(StreamError::Empty)?;
        Ok((stream_id, framed[1..].to_vec()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signaling_round_trip() {
        let s = StreamLayer::rev0_default();
        let wire = s.send(StreamId::Signaling, b"hello");
        let (id, sdu) = s.recv(&wire).unwrap();
        assert_eq!(id, StreamId::Signaling);
        assert_eq!(sdu, b"hello");
    }

    #[test]
    fn default_packet_round_trip() {
        let s = StreamLayer::rev0_default();
        let wire = s.send(StreamId::DefaultPacket, &[0xDE, 0xAD, 0xBE, 0xEF]);
        let (id, sdu) = s.recv(&wire).unwrap();
        assert_eq!(id, StreamId::DefaultPacket);
        assert_eq!(sdu, vec![0xDE, 0xAD, 0xBE, 0xEF]);
    }

    #[test]
    fn empty_pdu_is_error() {
        let s = StreamLayer::rev0_default();
        let err = s.recv(&[]).unwrap_err();
        assert!(matches!(err, StreamError::Empty));
    }

    #[test]
    fn all_four_stream_ids_round_trip() {
        let s = StreamLayer::rev0_default();
        for id in [
            StreamId::Signaling,
            StreamId::DefaultPacket,
            StreamId::Reserved2,
            StreamId::Reserved3,
        ] {
            let wire = s.send(id, b"x");
            let (got, _) = s.recv(&wire).unwrap();
            assert_eq!(got, id);
        }
    }
}
