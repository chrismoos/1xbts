//! One HDLC frame (RFC 1662) per GRE frame, or Segmentation Indication pieces
//! of a longer one (A.S0008-C §4.2.17, A.S0012-D §2.6.1.4).

use crate::{GreAttribute, GreSegment};

/// Leaves room for IP, GRE and IPsec headers on a 1500-octet path.
pub const DEFAULT_MAX_GRE_PAYLOAD: u16 = 1400;

const HDLC_FLAG: u8 = 0x7e;

/// A longer run without a closing flag is not a PPP frame.
const MAX_HELD_FRAME_LEN: usize = 16 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BearerChunk {
    pub payload: Vec<u8>,
    pub segment: Option<GreSegment>,
}

impl BearerChunk {
    pub fn attributes(&self) -> Vec<GreAttribute> {
        self.segment
            .map(GreAttribute::Segmentation)
            .into_iter()
            .collect()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Continuation {
    Segmented,
    Plain,
}

#[derive(Debug, Clone)]
pub struct PppFrameAligner {
    max_payload: usize,
    segmentation: bool,
    pending: Vec<u8>,
    has_content: bool,
    continuation: Option<Continuation>,
}

impl PppFrameAligner {
    pub fn new(max_payload: u16) -> Self {
        Self {
            max_payload: usize::from(max_payload.max(1)),
            segmentation: false,
            pending: Vec::new(),
            has_content: false,
            continuation: None,
        }
    }

    /// Without segmentation a frame over the cap is sent whole and the outer IP
    /// layer fragments it.
    pub fn set_segmentation(&mut self, accepted: bool) {
        self.segmentation = accepted;
    }

    pub fn push(&mut self, octets: &[u8]) -> Vec<BearerChunk> {
        let mut chunks = Vec::new();
        for &octet in octets {
            if octet == HDLC_FLAG {
                if self.has_content {
                    self.pending.push(HDLC_FLAG);
                    self.finish_frame(&mut chunks);
                } else if self.pending.is_empty() {
                    // Repeated flags are interframe fill (RFC 1662 §4.3).
                    self.pending.push(HDLC_FLAG);
                }
                continue;
            }
            if self.pending.is_empty() && self.continuation.is_none() {
                // The receiver discards the empty frame this extra flag
                // creates (RFC 1662 §4.3).
                self.pending.push(HDLC_FLAG);
            }
            self.pending.push(octet);
            self.has_content = true;
            if self.segmentation && self.pending.len() >= self.max_payload {
                let segment = match self.continuation {
                    Some(Continuation::Segmented) => GreSegment::Continued,
                    _ => GreSegment::Started,
                };
                chunks.push(BearerChunk {
                    payload: std::mem::take(&mut self.pending),
                    segment: Some(segment),
                });
                self.continuation = Some(Continuation::Segmented);
            } else if !self.segmentation && self.pending.len() >= MAX_HELD_FRAME_LEN {
                chunks.push(BearerChunk {
                    payload: std::mem::take(&mut self.pending),
                    segment: None,
                });
                self.continuation = Some(Continuation::Plain);
            }
        }
        chunks
    }

    fn finish_frame(&mut self, chunks: &mut Vec<BearerChunk>) {
        let frame = std::mem::take(&mut self.pending);
        match self.continuation.take() {
            Some(Continuation::Segmented) => chunks.push(BearerChunk {
                payload: frame,
                segment: Some(GreSegment::Ended),
            }),
            Some(Continuation::Plain) => chunks.push(BearerChunk {
                payload: frame,
                segment: None,
            }),
            None if self.segmentation && frame.len() > self.max_payload => {
                let pieces = frame.chunks(self.max_payload).count();
                for (index, piece) in frame.chunks(self.max_payload).enumerate() {
                    let segment = match index {
                        0 => GreSegment::Started,
                        last if last + 1 == pieces => GreSegment::Ended,
                        _ => GreSegment::Continued,
                    };
                    chunks.push(BearerChunk {
                        payload: piece.to_vec(),
                        segment: Some(segment),
                    });
                }
            }
            None => chunks.push(BearerChunk {
                payload: frame,
                segment: None,
            }),
        }
        self.has_content = false;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn frame(content: &[u8]) -> Vec<u8> {
        let mut out = vec![HDLC_FLAG];
        out.extend_from_slice(content);
        out.push(HDLC_FLAG);
        out
    }

    fn push_in_slices(
        aligner: &mut PppFrameAligner,
        stream: &[u8],
        slice: usize,
    ) -> Vec<BearerChunk> {
        stream
            .chunks(slice)
            .flat_map(|octets| aligner.push(octets))
            .collect()
    }

    #[test]
    fn rlp_sized_slices_come_out_as_whole_frames() {
        let first = frame(&[0x21; 45]);
        let second = frame(&[0x45; 7]);
        let stream = [first.clone(), second.clone()].concat();
        let mut aligner = PppFrameAligner::new(DEFAULT_MAX_GRE_PAYLOAD);

        let chunks = push_in_slices(&mut aligner, &stream, 20);

        assert_eq!(
            chunks,
            vec![
                BearerChunk {
                    payload: first,
                    segment: None
                },
                BearerChunk {
                    payload: second,
                    segment: None
                },
            ]
        );
    }

    #[test]
    fn a_frame_is_held_until_its_closing_flag() {
        let mut aligner = PppFrameAligner::new(DEFAULT_MAX_GRE_PAYLOAD);

        assert!(aligner.push(&[HDLC_FLAG, 0xff, 0x03, 0xc0]).is_empty());
        let chunks = aligner.push(&[0x21, HDLC_FLAG]);

        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].payload, frame(&[0xff, 0x03, 0xc0, 0x21]));
    }

    #[test]
    fn a_shared_flag_frame_gets_its_own_opening_flag() {
        let mut aligner = PppFrameAligner::new(DEFAULT_MAX_GRE_PAYLOAD);

        let chunks = aligner.push(&[HDLC_FLAG, 0x01, HDLC_FLAG, 0x02, HDLC_FLAG]);

        let payloads: Vec<_> = chunks.into_iter().map(|chunk| chunk.payload).collect();
        assert_eq!(payloads, vec![frame(&[0x01]), frame(&[0x02])]);
    }

    #[test]
    fn interframe_fill_flags_are_not_sent_alone() {
        let mut aligner = PppFrameAligner::new(DEFAULT_MAX_GRE_PAYLOAD);

        assert!(aligner.push(&[HDLC_FLAG, HDLC_FLAG, HDLC_FLAG]).is_empty());
        let chunks = aligner.push(&[0x01, HDLC_FLAG]);

        assert_eq!(chunks[0].payload, frame(&[0x01]));
    }

    #[test]
    fn a_long_frame_is_segmented_when_the_peer_accepts_it() {
        let long = frame(&[0x42; 30]);
        let mut aligner = PppFrameAligner::new(12);
        aligner.set_segmentation(true);

        let chunks = push_in_slices(&mut aligner, &long, 5);

        let segments: Vec<_> = chunks.iter().map(|chunk| chunk.segment).collect();
        assert_eq!(
            segments,
            vec![
                Some(GreSegment::Started),
                Some(GreSegment::Continued),
                Some(GreSegment::Ended),
            ]
        );
        assert!(chunks.iter().all(|chunk| chunk.payload.len() <= 12));
        let rejoined: Vec<u8> = chunks.into_iter().flat_map(|chunk| chunk.payload).collect();
        assert_eq!(rejoined, long);
    }

    #[test]
    fn a_long_frame_pushed_at_once_is_segmented_within_the_cap() {
        let long = frame(&[0x42; 30]);
        let mut aligner = PppFrameAligner::new(12);
        aligner.set_segmentation(true);

        let chunks = aligner.push(&long);

        assert_eq!(chunks.first().unwrap().segment, Some(GreSegment::Started));
        assert_eq!(chunks.last().unwrap().segment, Some(GreSegment::Ended));
        assert!(chunks.iter().all(|chunk| chunk.payload.len() <= 12));
        let rejoined: Vec<u8> = chunks.into_iter().flat_map(|chunk| chunk.payload).collect();
        assert_eq!(rejoined, long);
    }

    #[test]
    fn a_long_frame_is_sent_whole_without_peer_segmentation() {
        let long = frame(&[0x42; 30]);
        let mut aligner = PppFrameAligner::new(12);

        let chunks = push_in_slices(&mut aligner, &long, 5);

        assert_eq!(
            chunks,
            vec![BearerChunk {
                payload: long,
                segment: None
            }]
        );
    }

    #[test]
    fn the_next_frame_after_a_segmented_one_starts_unsegmented() {
        let long = frame(&[0x42; 30]);
        let short = frame(&[0x01]);
        let mut aligner = PppFrameAligner::new(12);
        aligner.set_segmentation(true);

        let chunks = aligner.push(&[long, short.clone()].concat());

        assert_eq!(
            chunks.last().unwrap(),
            &BearerChunk {
                payload: short,
                segment: None
            }
        );
    }

    #[test]
    fn a_run_without_flags_is_sent_on_as_stream_octets() {
        let mut aligner = PppFrameAligner::new(DEFAULT_MAX_GRE_PAYLOAD);
        let run = vec![0x42; MAX_HELD_FRAME_LEN + 10];

        let mut chunks = aligner.push(&run);
        chunks.extend(aligner.push(&[HDLC_FLAG]));

        assert_eq!(chunks.len(), 2);
        assert!(chunks.iter().all(|chunk| chunk.segment.is_none()));
        let rejoined: Vec<u8> = chunks.into_iter().flat_map(|chunk| chunk.payload).collect();
        let mut expected = vec![HDLC_FLAG];
        expected.extend_from_slice(&run);
        expected.push(HDLC_FLAG);
        assert_eq!(rejoined, expected);
    }

    #[test]
    fn segment_chunks_carry_the_segmentation_attribute() {
        let chunk = BearerChunk {
            payload: vec![HDLC_FLAG],
            segment: Some(GreSegment::Ended),
        };

        assert_eq!(
            chunk.attributes(),
            vec![GreAttribute::Segmentation(GreSegment::Ended)]
        );
        assert!(
            BearerChunk {
                payload: vec![],
                segment: None
            }
            .attributes()
            .is_empty()
        );
    }
}
