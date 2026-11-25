//! Length-prefixed JSON frames.
//!
//! Each frame is a 4-byte big-endian payload length followed by a UTF-8 JSON
//! document. Payloads larger than [`MAX_FRAME_LEN`] are rejected before any
//! allocation, so a misbehaving peer cannot exhaust memory.

use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

use crate::{IpcError, Result};

/// Maximum payload length of a frame (1 MiB).
pub const MAX_FRAME_LEN: usize = 1024 * 1024;

const HEADER_LEN: usize = 4;

/// Serializes `message` and writes it as one frame.
///
/// # Errors
///
/// Returns [`IpcError::Encode`], [`IpcError::FrameTooLarge`] or
/// [`IpcError::Io`].
pub async fn write_frame<W, T>(writer: &mut W, message: &T) -> Result<()>
where
    W: AsyncWrite + Unpin,
    T: Serialize,
{
    let payload = serde_json::to_vec(message).map_err(IpcError::Encode)?;
    let too_large = || IpcError::FrameTooLarge {
        len: payload.len(),
        max: MAX_FRAME_LEN,
    };
    if payload.len() > MAX_FRAME_LEN {
        return Err(too_large());
    }
    let len = u32::try_from(payload.len()).map_err(|_| too_large())?;
    // A single write keeps the frame contiguous on the wire.
    let mut frame = Vec::with_capacity(HEADER_LEN + payload.len());
    frame.extend_from_slice(&len.to_be_bytes());
    frame.extend_from_slice(&payload);
    writer.write_all(&frame).await.map_err(IpcError::Io)?;
    writer.flush().await.map_err(IpcError::Io)
}

/// Reads one frame and deserializes it.
///
/// Returns `Ok(None)` when the peer closed the connection cleanly between two
/// frames.
///
/// # Errors
///
/// Returns [`IpcError::ConnectionClosed`] for a truncated frame,
/// [`IpcError::FrameTooLarge`], [`IpcError::Decode`] or [`IpcError::Io`].
pub async fn read_frame<R, T>(reader: &mut R) -> Result<Option<T>>
where
    R: AsyncRead + Unpin,
    T: DeserializeOwned,
{
    let mut header = [0u8; HEADER_LEN];
    let mut filled = 0;
    while filled < HEADER_LEN {
        let read = reader
            .read(&mut header[filled..])
            .await
            .map_err(IpcError::Io)?;
        if read == 0 {
            return if filled == 0 {
                Ok(None)
            } else {
                Err(IpcError::ConnectionClosed)
            };
        }
        filled += read;
    }

    let len = usize::try_from(u32::from_be_bytes(header)).unwrap_or(usize::MAX);
    if len > MAX_FRAME_LEN {
        return Err(IpcError::FrameTooLarge {
            len,
            max: MAX_FRAME_LEN,
        });
    }
    let mut payload = vec![0u8; len];
    reader.read_exact(&mut payload).await.map_err(|err| {
        if err.kind() == std::io::ErrorKind::UnexpectedEof {
            IpcError::ConnectionClosed
        } else {
            IpcError::Io(err)
        }
    })?;
    serde_json::from_slice(&payload)
        .map(Some)
        .map_err(IpcError::Decode)
}

#[cfg(test)]
mod tests {
    use serde::Deserialize;

    use super::*;

    #[derive(Debug, PartialEq, Serialize, Deserialize)]
    struct Message {
        text: String,
    }

    #[tokio::test]
    async fn roundtrip_and_clean_eof() {
        let (mut a, mut b) = tokio::io::duplex(64);
        let sent = Message {
            text: "hello".into(),
        };
        write_frame(&mut a, &sent).await.unwrap();
        write_frame(&mut a, &sent).await.unwrap();
        drop(a);
        assert_eq!(read_frame::<_, Message>(&mut b).await.unwrap(), Some(sent));
        assert!(read_frame::<_, Message>(&mut b).await.unwrap().is_some());
        assert!(read_frame::<_, Message>(&mut b).await.unwrap().is_none());
    }

    #[tokio::test]
    async fn wire_format_is_length_prefixed_json() {
        let mut buf = Vec::new();
        write_frame(&mut buf, &Message { text: "hi".into() })
            .await
            .unwrap();
        assert_eq!(&buf[..4], &[0, 0, 0, 13]);
        assert_eq!(&buf[4..], br#"{"text":"hi"}"#);
    }

    #[tokio::test]
    async fn oversized_frame_is_rejected_without_allocating() {
        let header = u32::try_from(MAX_FRAME_LEN + 1).unwrap().to_be_bytes();
        let mut reader = &header[..];
        let err = read_frame::<_, Message>(&mut reader).await.unwrap_err();
        assert!(matches!(err, IpcError::FrameTooLarge { .. }));

        let big = Message {
            text: "x".repeat(MAX_FRAME_LEN),
        };
        let err = write_frame(&mut Vec::new(), &big).await.unwrap_err();
        assert!(matches!(err, IpcError::FrameTooLarge { .. }));
    }

    #[tokio::test]
    async fn truncated_frames_are_reported() {
        let mut partial_header = &[0u8, 0][..];
        let err = read_frame::<_, Message>(&mut partial_header)
            .await
            .unwrap_err();
        assert!(matches!(err, IpcError::ConnectionClosed));

        let mut partial_payload = &[0u8, 0, 0, 10, b'{'][..];
        let err = read_frame::<_, Message>(&mut partial_payload)
            .await
            .unwrap_err();
        assert!(matches!(err, IpcError::ConnectionClosed));
    }

    #[tokio::test]
    async fn invalid_json_is_a_decode_error() {
        let mut frame = vec![0u8, 0, 0, 3];
        frame.extend_from_slice(b"{x}");
        let err = read_frame::<_, Message>(&mut &frame[..]).await.unwrap_err();
        assert!(matches!(err, IpcError::Decode(_)));
    }
}
