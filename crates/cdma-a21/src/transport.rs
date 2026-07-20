//! Length-prefixed stream framing for A21 messages on TCP.
//!
//! Wire framing per message: 4-octet big-endian length prefix, then the
//! payload produced by [`A21Message::encode`]. The length covers only the
//! payload bytes, not the prefix itself.

use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::error::{A21Error, Result};
use crate::message::A21Message;

/// Upper bound on a single A21 frame payload. CrossPageRequest payloads carry
/// opaque CS/PS paging PDUs; 1 MiB is generous and bounds memory per-peer.
pub const MAX_FRAME_LEN: usize = 1024 * 1024;

/// Writes one length-prefixed A21 frame to `writer` and flushes.
pub async fn write_frame<W: AsyncWrite + Unpin>(writer: &mut W, msg: &A21Message) -> Result<()> {
    let payload = msg.encode();
    if payload.len() > MAX_FRAME_LEN {
        return Err(A21Error::Decode(format!(
            "outbound A21 frame exceeds MAX_FRAME_LEN ({} > {MAX_FRAME_LEN})",
            payload.len()
        )));
    }
    let len = payload.len() as u32;
    writer.write_all(&len.to_be_bytes()).await?;
    writer.write_all(&payload).await?;
    writer.flush().await?;
    Ok(())
}

/// Reads one length-prefixed A21 frame from `reader`.
///
/// Returns [`A21Error::Closed`] when the peer closes the connection between
/// frames. Returns [`A21Error::Decode`] when the prefix declares a length
/// larger than [`MAX_FRAME_LEN`] or the payload fails to decode.
pub async fn read_frame<R: AsyncRead + Unpin>(reader: &mut R) -> Result<A21Message> {
    let mut len_buf = [0u8; 4];
    match reader.read_exact(&mut len_buf).await {
        Ok(_) => {}
        Err(e) if e.kind() == std::io::ErrorKind::UnexpectedEof => return Err(A21Error::Closed),
        Err(e) => return Err(A21Error::Io(e)),
    }
    let len = u32::from_be_bytes(len_buf) as usize;
    if len > MAX_FRAME_LEN {
        return Err(A21Error::Decode(format!(
            "A21 frame length {len} exceeds MAX_FRAME_LEN {MAX_FRAME_LEN}"
        )));
    }
    let mut buf = vec![0u8; len];
    reader.read_exact(&mut buf).await?;
    A21Message::decode(&buf)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::PagingSource;
    use tokio::io::BufReader;

    #[tokio::test]
    async fn concatenated_frames_decode_in_order() {
        let msgs = vec![
            A21Message::IdentityBinding { imsi: 1 },
            A21Message::IdentityRelease { imsi: 4 },
            A21Message::CrossPageRequest {
                imsi: 5,
                source: PagingSource::Hrpd,
                payload: vec![9, 9, 9],
            },
            A21Message::CrossPageAck {
                imsi: 5,
                accepted: true,
                reason: None,
            },
            A21Message::SuppressionStart {
                imsi: 6,
                source: PagingSource::OneX,
            },
            A21Message::SuppressionEnd { imsi: 6 },
        ];

        let mut buf = Vec::new();
        for m in &msgs {
            write_frame(&mut buf, m).await.unwrap();
        }

        let mut reader = BufReader::new(std::io::Cursor::new(buf));
        for expected in &msgs {
            let got = read_frame(&mut reader).await.unwrap();
            assert_eq!(*expected, got);
        }
        let end = read_frame(&mut reader).await.unwrap_err();
        assert!(
            matches!(end, A21Error::Closed),
            "expected Closed, got {end:?}"
        );
    }

    #[tokio::test]
    async fn oversized_length_prefix_is_decode_error() {
        let mut buf = Vec::new();
        buf.extend_from_slice(&((MAX_FRAME_LEN as u32) + 1).to_be_bytes());
        let mut reader = std::io::Cursor::new(buf);
        let err = read_frame(&mut reader).await.unwrap_err();
        assert!(matches!(err, A21Error::Decode(_)));
    }
}
