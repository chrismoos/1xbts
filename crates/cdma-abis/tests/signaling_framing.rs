use cdma_abis::signaling_framing::{
    FRAME_FLAG, HEADER_LEN, SignalingFrame, SignalingFrameStreamDecoder,
};

#[test]
fn signaling_frame_roundtrip() {
    let frame = SignalingFrame::new([0x8c, 0x13, 0x04, 0x01, 0x02, 0x03, 0x04]);
    let encoded = frame.encode().unwrap();
    assert_eq!(&encoded[..2], &FRAME_FLAG.to_be_bytes());
    assert_eq!(u16::from_be_bytes([encoded[2], encoded[3]]) as usize, 7);
    assert_eq!(SignalingFrame::decode(&encoded).unwrap(), frame);
}

#[test]
fn signaling_frame_rejects_truncated_header() {
    let error = SignalingFrame::decode(&[0xf6, 0x34, 0x00]).unwrap_err();
    assert_eq!(
        error,
        cdma_abis::Error::Truncated {
            context: "Abis TCP signaling header",
            needed: HEADER_LEN,
            actual: 3,
        }
    );
}

#[test]
fn signaling_frame_rejects_invalid_flag() {
    let error = SignalingFrame::decode(&[0x00, 0x00, 0x00, 0x01, 0xaa]).unwrap_err();
    assert_eq!(
        error,
        cdma_abis::Error::InvalidValue {
            context: "Abis TCP signaling flag",
            reason: "expected 0xf634",
        }
    );
}

#[test]
fn signaling_frame_rejects_truncated_payload() {
    let error = SignalingFrame::decode(&[0xf6, 0x34, 0x00, 0x04, 0xaa, 0xbb]).unwrap_err();
    assert_eq!(
        error,
        cdma_abis::Error::Truncated {
            context: "Abis TCP signaling payload",
            needed: 8,
            actual: 6,
        }
    );
}

#[test]
fn signaling_frame_rejects_length_mismatch() {
    let error = SignalingFrame::decode(&[0xf6, 0x34, 0x00, 0x02, 0xaa, 0xbb, 0xcc]).unwrap_err();
    assert_eq!(
        error,
        cdma_abis::Error::InvalidLength {
            context: "Abis TCP signaling frame",
            expected: 6,
            actual: 7,
        }
    );
}

#[test]
fn signaling_frame_decodes_prefix_from_larger_buffer() {
    let bytes = [0xf6, 0x34, 0x00, 0x02, 0xaa, 0xbb, 0xcc, 0xdd];
    let (frame, consumed) = SignalingFrame::decode_prefix(&bytes).unwrap();
    assert_eq!(frame, SignalingFrame::new([0xaa, 0xbb]));
    assert_eq!(consumed, 6);
}

#[test]
fn signaling_stream_decoder_recovers_after_garbage_prefix() {
    let mut decoder = SignalingFrameStreamDecoder::new();
    decoder.push_bytes(&[0x00, 0x01, 0xf6]);
    assert_eq!(decoder.next_frame().unwrap(), None);
    decoder.push_bytes(&[0x34, 0x00, 0x02, 0xaa, 0xbb]);
    assert_eq!(
        decoder.next_frame().unwrap(),
        Some(SignalingFrame::new([0xaa, 0xbb]))
    );
    assert_eq!(decoder.buffered_len(), 0);
}

#[test]
fn signaling_stream_decoder_waits_for_complete_payload() {
    let mut decoder = SignalingFrameStreamDecoder::new();
    decoder.push_bytes(&[0xf6, 0x34, 0x00, 0x03, 0xaa]);
    assert_eq!(decoder.next_frame().unwrap(), None);
    assert_eq!(decoder.buffered_len(), 5);
    decoder.push_bytes(&[0xbb, 0xcc]);
    assert_eq!(
        decoder.next_frame().unwrap(),
        Some(SignalingFrame::new([0xaa, 0xbb, 0xcc]))
    );
    assert_eq!(decoder.buffered_len(), 0);
}
