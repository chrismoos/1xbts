//! Coordination messages exchanged between the 1x BSC and the HRPD AN.
//!
//! They cover the hybrid-AT coordination role of the A21 reference point
//! (A.S0017-D, A.S0019-A): announcing IMSI presence, cross-paging, and paging
//! suppression so a hybrid AT is paged once across both systems. This is a
//! 1XBTS-internal hand-rolled message set, not the spec's A21 encoding.
//!
//! Wire framing matches the style of `cdma-a11` and `cdma-a9`: a fixed 1-octet
//! type tag followed by big-endian scalar fields and length-prefixed variable
//! fields. Stream framing (4-octet BE length prefix per message) lives in
//! [`crate::transport`].

use crate::error::{A21Error, Result};

/// Source system originating a cross-system page or suppression event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum PagingSource {
    /// 1x BSC (CS / circuit-switched paging origin).
    OneX = 0x01,
    /// HRPD AN (PS / packet-data paging origin).
    Hrpd = 0x02,
}

impl PagingSource {
    fn from_u8(v: u8) -> Result<Self> {
        match v {
            0x01 => Ok(PagingSource::OneX),
            0x02 => Ok(PagingSource::Hrpd),
            other => Err(A21Error::Decode(format!(
                "unknown PagingSource discriminant 0x{other:02x}"
            ))),
        }
    }
}

/// A21 wire message exchanged between the 1x BSC and the HRPD AN.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum A21Message {
    /// Announce that `imsi` is HRPD-attached, so the peer can suppress its own
    /// paging and route cross-pages here. The AN keeps the IMSI↔UATI mapping
    /// internally; the peer only needs IMSI presence.
    IdentityBinding { imsi: u64 },
    /// Release a previously bound IMSI (peer no longer tracks this AT).
    IdentityRelease { imsi: u64 },
    /// "Peer system has a page for this IMSI" — opaque PDU forwarded as `payload`.
    /// Fills the A21 cross-paging role between the 1x and HRPD systems.
    CrossPageRequest {
        imsi: u64,
        source: PagingSource,
        payload: Vec<u8>,
    },
    /// Response to a [`A21Message::CrossPageRequest`].
    CrossPageAck {
        imsi: u64,
        accepted: bool,
        reason: Option<String>,
    },
    /// Suppress peer paging while this AT is in traffic on `source` (e.g. 1x voice).
    SuppressionStart { imsi: u64, source: PagingSource },
    /// Release a previously asserted suppression.
    SuppressionEnd { imsi: u64 },
}

// Type tags. Keep stable; bumping the wire format requires a version bump in
// the transport header, not silent renumbering here.
const TAG_IDENTITY_BINDING: u8 = 0x01;
const TAG_IDENTITY_RELEASE: u8 = 0x02;
const TAG_CROSS_PAGE_REQUEST: u8 = 0x03;
const TAG_CROSS_PAGE_ACK: u8 = 0x04;
const TAG_SUPPRESSION_START: u8 = 0x05;
const TAG_SUPPRESSION_END: u8 = 0x06;

impl A21Message {
    /// Encodes the message payload (without the outer length prefix).
    pub fn encode(&self) -> Vec<u8> {
        let mut out = Vec::with_capacity(32);
        match self {
            A21Message::IdentityBinding { imsi } => {
                out.push(TAG_IDENTITY_BINDING);
                out.extend_from_slice(&imsi.to_be_bytes());
            }
            A21Message::IdentityRelease { imsi } => {
                out.push(TAG_IDENTITY_RELEASE);
                out.extend_from_slice(&imsi.to_be_bytes());
            }
            A21Message::CrossPageRequest {
                imsi,
                source,
                payload,
            } => {
                out.push(TAG_CROSS_PAGE_REQUEST);
                out.extend_from_slice(&imsi.to_be_bytes());
                out.push(*source as u8);
                let plen = payload.len() as u32;
                out.extend_from_slice(&plen.to_be_bytes());
                out.extend_from_slice(payload);
            }
            A21Message::CrossPageAck {
                imsi,
                accepted,
                reason,
            } => {
                out.push(TAG_CROSS_PAGE_ACK);
                out.extend_from_slice(&imsi.to_be_bytes());
                out.push(if *accepted { 1 } else { 0 });
                match reason {
                    Some(s) => {
                        out.push(1);
                        let bytes = s.as_bytes();
                        out.extend_from_slice(&(bytes.len() as u32).to_be_bytes());
                        out.extend_from_slice(bytes);
                    }
                    None => {
                        out.push(0);
                    }
                }
            }
            A21Message::SuppressionStart { imsi, source } => {
                out.push(TAG_SUPPRESSION_START);
                out.extend_from_slice(&imsi.to_be_bytes());
                out.push(*source as u8);
            }
            A21Message::SuppressionEnd { imsi } => {
                out.push(TAG_SUPPRESSION_END);
                out.extend_from_slice(&imsi.to_be_bytes());
            }
        }
        out
    }

    /// Decodes a message payload (without the outer length prefix).
    pub fn decode(buf: &[u8]) -> Result<Self> {
        let mut cur = Cursor::new(buf);
        let tag = cur.u8()?;
        let msg = match tag {
            TAG_IDENTITY_BINDING => A21Message::IdentityBinding { imsi: cur.u64()? },
            TAG_IDENTITY_RELEASE => A21Message::IdentityRelease { imsi: cur.u64()? },
            TAG_CROSS_PAGE_REQUEST => {
                let imsi = cur.u64()?;
                let source = PagingSource::from_u8(cur.u8()?)?;
                let plen = cur.u32()? as usize;
                let payload = cur.take(plen)?.to_vec();
                A21Message::CrossPageRequest {
                    imsi,
                    source,
                    payload,
                }
            }
            TAG_CROSS_PAGE_ACK => {
                let imsi = cur.u64()?;
                let accepted = cur.u8()? != 0;
                let has_reason = cur.u8()?;
                let reason = match has_reason {
                    0 => None,
                    1 => {
                        let rlen = cur.u32()? as usize;
                        let bytes = cur.take(rlen)?;
                        Some(
                            std::str::from_utf8(bytes)
                                .map_err(|e| A21Error::Decode(format!("ack reason utf-8: {e}")))?
                                .to_string(),
                        )
                    }
                    other => {
                        return Err(A21Error::Decode(format!(
                            "CrossPageAck has_reason flag must be 0 or 1, got {other}"
                        )));
                    }
                };
                A21Message::CrossPageAck {
                    imsi,
                    accepted,
                    reason,
                }
            }
            TAG_SUPPRESSION_START => A21Message::SuppressionStart {
                imsi: cur.u64()?,
                source: PagingSource::from_u8(cur.u8()?)?,
            },
            TAG_SUPPRESSION_END => A21Message::SuppressionEnd { imsi: cur.u64()? },
            other => {
                return Err(A21Error::Decode(format!(
                    "unknown A21 message tag 0x{other:02x}"
                )));
            }
        };
        if !cur.is_empty() {
            return Err(A21Error::Decode(format!(
                "trailing {} bytes after A21 message tag 0x{tag:02x}",
                cur.remaining()
            )));
        }
        Ok(msg)
    }
}

/// Small zero-dep big-endian cursor used by [`A21Message::decode`].
struct Cursor<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }
    fn remaining(&self) -> usize {
        self.buf.len() - self.pos
    }
    fn is_empty(&self) -> bool {
        self.remaining() == 0
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8]> {
        if self.remaining() < n {
            return Err(A21Error::Decode(format!(
                "truncated A21 frame: need {n}, have {}",
                self.remaining()
            )));
        }
        let out = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }
    fn u8(&mut self) -> Result<u8> {
        Ok(self.take(1)?[0])
    }
    fn u32(&mut self) -> Result<u32> {
        let b = self.take(4)?;
        Ok(u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    }
    fn u64(&mut self) -> Result<u64> {
        let b = self.take(8)?;
        Ok(u64::from_be_bytes([
            b[0], b[1], b[2], b[3], b[4], b[5], b[6], b[7],
        ]))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn roundtrip(m: &A21Message) {
        let bytes = m.encode();
        let back = A21Message::decode(&bytes).expect("decode");
        assert_eq!(*m, back);
    }

    #[test]
    fn roundtrip_identity_binding() {
        roundtrip(&A21Message::IdentityBinding {
            imsi: 310_260_123_456_789,
        });
    }

    #[test]
    fn roundtrip_identity_release() {
        roundtrip(&A21Message::IdentityRelease {
            imsi: 310_260_111_222_333,
        });
    }

    #[test]
    fn roundtrip_cross_page_request_with_payload() {
        roundtrip(&A21Message::CrossPageRequest {
            imsi: 1,
            source: PagingSource::OneX,
            payload: vec![1, 2, 3, 4, 5, 6, 7, 8, 9, 10],
        });
        roundtrip(&A21Message::CrossPageRequest {
            imsi: 2,
            source: PagingSource::Hrpd,
            payload: vec![],
        });
    }

    #[test]
    fn roundtrip_cross_page_ack_with_and_without_reason() {
        roundtrip(&A21Message::CrossPageAck {
            imsi: 42,
            accepted: true,
            reason: None,
        });
        roundtrip(&A21Message::CrossPageAck {
            imsi: 42,
            accepted: false,
            reason: Some("AT not registered on HRPD".into()),
        });
    }

    #[test]
    fn roundtrip_suppression_start_end() {
        roundtrip(&A21Message::SuppressionStart {
            imsi: 99,
            source: PagingSource::OneX,
        });
        roundtrip(&A21Message::SuppressionEnd { imsi: 99 });
    }

    #[test]
    fn decode_rejects_unknown_tag() {
        let err = A21Message::decode(&[0xff, 0, 0]).unwrap_err();
        assert!(matches!(err, A21Error::Decode(_)));
    }

    #[test]
    fn decode_rejects_trailing_bytes() {
        let mut bytes = A21Message::IdentityRelease { imsi: 1 }.encode();
        bytes.push(0xaa);
        let err = A21Message::decode(&bytes).unwrap_err();
        assert!(matches!(err, A21Error::Decode(_)));
    }

    #[test]
    fn decode_rejects_bad_paging_source() {
        let bytes = [
            TAG_SUPPRESSION_START,
            0,
            0,
            0,
            0,
            0,
            0,
            0,
            1,    // imsi
            0x55, // bad source
        ];
        let err = A21Message::decode(&bytes).unwrap_err();
        assert!(matches!(err, A21Error::Decode(_)));
    }

    #[test]
    fn decode_rejects_truncated_frame() {
        let bytes = [TAG_IDENTITY_BINDING, 0, 0, 0, 1];
        let err = A21Message::decode(&bytes).unwrap_err();
        assert!(matches!(err, A21Error::Decode(_)));
    }
}
